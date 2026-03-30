use super::*;
use crate::compositor::clusters::state::ClusterState;
use crate::compositor::interaction::state::InteractionState;
use crate::compositor::clusters::read::{
    ClusterLayoutPlan, ClusterReadController, ClusterTilePlacement, EnterClusterWorkspacePlan,
    ExitClusterWorkspacePlan,
};
use halley_core::cluster::{ClusterId, ClusterRemoveMemberOutcome};
use halley_core::field::RemoveNodeClusterEffect;

struct ClusterMutationController<'a> {
    field: &'a mut Field,
    cluster_state: &'a mut ClusterState,
    interaction_state: &'a mut InteractionState,
}

impl<'a> ClusterMutationController<'a> {
    fn cluster_mode_selected_nodes_for_monitor_mut(
        &mut self,
        monitor: &str,
    ) -> &mut std::collections::HashSet<NodeId> {
        self.cluster_state
            .cluster_mode_selected_nodes
            .entry(monitor.to_string())
            .or_default()
    }

    fn open_cluster_bloom_for_monitor(&mut self, monitor: &str, cid: ClusterId) -> bool {
        let Some(cluster) = self.field.cluster(cid) else {
            return false;
        };
        let Some(core_id) = cluster.core else {
            return false;
        };
        self.cluster_state
            .cluster_bloom_open
            .retain(|name, open_cid| *open_cid != cid || name == monitor);
        let _ = self.close_cluster_bloom_for_monitor(monitor);
        let _ = self.field.set_pinned(core_id, true);
        self.interaction_state.physics_velocity.remove(&core_id);
        self.cluster_state
            .cluster_bloom_open
            .insert(monitor.to_string(), cid);
        true
    }

    fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(cid) = self.cluster_state.cluster_bloom_open.remove(monitor) else {
            return false;
        };
        if let Some(core_id) = self.field.cluster(cid).and_then(|cluster| cluster.core) {
            let _ = self.field.set_pinned(core_id, false);
        }
        true
    }

    fn enter_cluster_mode(&mut self, monitor: &str) -> bool {
        if self
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
        {
            return true;
        }
        self.cluster_state
            .cluster_mode_selected_nodes
            .insert(monitor.to_string(), std::collections::HashSet::new());
        true
    }

    fn exit_cluster_mode(&mut self, monitor: &str) -> bool {
        if !self
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
        {
            return false;
        }
        self.cluster_state
            .cluster_mode_selected_nodes
            .remove(monitor);
        true
    }

    fn toggle_cluster_mode_selection(&mut self, monitor: &str, node_id: NodeId) -> bool {
        if !self
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
        {
            return false;
        }
        let Some(node) = self.field.node(node_id) else {
            return false;
        };
        if node.kind != halley_core::field::NodeKind::Surface
            || node.state == halley_core::field::NodeState::Core
            || !self.field.is_visible(node_id)
        {
            return false;
        }
        if !self
            .cluster_mode_selected_nodes_for_monitor_mut(monitor)
            .insert(node_id)
        {
            self.cluster_mode_selected_nodes_for_monitor_mut(monitor)
                .remove(&node_id);
        }
        true
    }

    fn detach_member_from_cluster(
        &mut self,
        cid: ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now_ms: u64,
    ) -> Option<ClusterRemoveMemberOutcome> {
        let was_active = self
            .field
            .cluster(cid)
            .is_some_and(|cluster| cluster.is_active());
        let outcome = self.field.remove_member_from_cluster(cid, member_id)?;
        if matches!(outcome, ClusterRemoveMemberOutcome::Removed) && was_active {
            let _ = self
                .field
                .move_member_out_of_active_cluster_workspace(cid, member_id);
        }
        let _ = self.field.set_detached(member_id, false);
        let _ = self
            .field
            .set_state(member_id, halley_core::field::NodeState::Active);
        if let Some(node) = self.field.node_mut(member_id) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            node.pos = world_pos;
        }
        let _ = self.field.touch(member_id, now_ms);
        Some(outcome)
    }

    fn absorb_node_into_cluster(&mut self, cid: ClusterId, node_id: NodeId) -> bool {
        let active_workspace = self
            .field
            .cluster(cid)
            .is_some_and(|cluster| cluster.is_active());
        if self.field.add_member_to_cluster(cid, node_id).is_err() {
            return false;
        }
        if active_workspace {
            if !self
                .field
                .move_member_into_active_cluster_workspace(cid, node_id)
            {
                return false;
            }
            if let Some(cluster) = self.field.cluster_mut(cid)
                && let Some(node) = cluster.workspace_member_mut(node_id)
            {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                node.visibility.set(Visibility::DETACHED, false);
                node.state = halley_core::field::NodeState::Active;
            }
        } else {
            let _ = self
                .field
                .set_state(node_id, halley_core::field::NodeState::Node);
            if let Some(node) = self.field.node_mut(node_id) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
            }
            let _ = self.field.set_detached(node_id, false);
        }
        true
    }
}

impl Halley {
    pub(crate) const CLUSTER_OVERFLOW_REVEAL_EDGE_PX: f32 = 28.0;
    const CLUSTER_OVERFLOW_REVEAL_MS: u64 = 2200;

    fn cluster_read_controller(&self) -> ClusterReadController<'_> {
        ClusterReadController {
            field: &self.model.field,
            cluster_state: &self.model.cluster_state,
            monitor_state: &self.model.monitor_state,
            tuning: &self.runtime.tuning,
        }
    }

    fn cluster_mutation_controller(&mut self) -> ClusterMutationController<'_> {
        ClusterMutationController {
            field: &mut self.model.field,
            cluster_state: &mut self.model.cluster_state,
            interaction_state: &mut self.input.interaction_state,
        }
    }

    fn preferred_monitor_for_cluster(
        &self,
        cid: ClusterId,
        preferred: Option<&str>,
    ) -> Option<String> {
        self.cluster_read_controller()
            .preferred_monitor_for_cluster(cid, preferred)
    }

    fn sync_cluster_core_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        let Some(core_id) = self
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| cluster.core)
        else {
            return false;
        };
        let Some(target_monitor) = self.preferred_monitor_for_cluster(cid, preferred) else {
            return false;
        };
        self.assign_node_to_monitor(core_id, target_monitor.as_str());
        true
    }

    fn restore_cluster_workspace_monitor(&mut self, monitor: &str) {
        let Some(vp) = self
            .model
            .cluster_state
            .workspace_prev_viewports
            .remove(monitor)
        else {
            return;
        };
        if self.model.monitor_state.current_monitor == monitor {
            self.model.viewport = vp;
            self.model.zoom_ref_size = self.model.viewport.size;
            self.snap_camera_targets_to_live();
            self.runtime.tuning.viewport_center = self.model.viewport.center;
            self.runtime.tuning.viewport_size = self.model.viewport.size;
        }
        if let Some(space) = self.model.monitor_state.monitors.get_mut(monitor) {
            space.viewport = vp;
            space.zoom_ref_size = vp.size;
            space.camera_target_center = vp.center;
            space.camera_target_view_size = vp.size;
        }
    }

    fn clear_cluster_shell_state(&mut self, cid: ClusterId) {
        let active_monitors = self
            .model
            .cluster_state
            .active_cluster_workspaces
            .iter()
            .filter_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
            .collect::<Vec<_>>();
        for monitor in &active_monitors {
            for id in self
                .model
                .cluster_state
                .workspace_hidden_nodes
                .remove(monitor.as_str())
                .unwrap_or_default()
            {
                if self.model.field.node(id).is_some() {
                    let _ = self.model.field.set_detached(id, false);
                }
            }
            self.model
                .cluster_state
                .workspace_core_positions
                .remove(monitor.as_str());
            self.model
                .cluster_state
                .cluster_overflow_members
                .remove(monitor.as_str());
            self.model
                .cluster_state
                .cluster_overflow_rects
                .remove(monitor.as_str());
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor.as_str());
            if self
                .input
                .interaction_state
                .cluster_overflow_drag_preview
                .as_ref()
                .is_some_and(|preview| preview.monitor == *monitor)
            {
                self.input.interaction_state.cluster_overflow_drag_preview = None;
                crate::compositor::interaction::pointer::set_cursor_override_icon(self, None);
            }
            self.restore_cluster_workspace_monitor(monitor.as_str());
        }
        self.model
            .cluster_state
            .active_cluster_workspaces
            .retain(|_, active_cid| *active_cid != cid);
        self.model
            .cluster_state
            .cluster_bloom_open
            .retain(|_, open_cid| *open_cid != cid);
        if self
            .input
            .interaction_state
            .cluster_join_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.cluster_id == cid)
        {
            self.input.interaction_state.cluster_join_candidate = None;
        }
    }

    fn dissolve_cluster(&mut self, cid: ClusterId) -> bool {
        let core_id = self
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| cluster.core);
        self.clear_cluster_shell_state(cid);
        if let Some(core_id) = core_id {
            self.model.monitor_state.node_monitor.remove(&core_id);
        }
        self.model.field.dissolve_cluster(cid)
    }

    pub(crate) fn remove_node_from_field(&mut self, id: NodeId, now_ms: u64) -> bool {
        let cluster_snapshot = self
            .model
            .field
            .cluster_id_for_member_public(id)
            .and_then(|cid| {
                self.model
                    .field
                    .cluster(cid)
                    .map(|cluster| (cid, cluster.members().to_vec(), cluster.core))
            });
        let (snapshot_cid, snapshot_members, snapshot_core_id) =
            cluster_snapshot.unwrap_or((ClusterId::new(0), Vec::new(), None));
        let Some((_, effect)) = self.model.field.remove_node_cluster_safe(id) else {
            return false;
        };

        match effect {
            Some(RemoveNodeClusterEffect::RemovedMember(cid)) => {
                if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None)
                    && self.active_cluster_workspace_for_monitor(cluster_monitor.as_str())
                        == Some(cid)
                {
                    self.layout_active_cluster_workspace_for_monitor(
                        cluster_monitor.as_str(),
                        now_ms,
                    );
                }
            }
            Some(RemoveNodeClusterEffect::DissolvedCluster(cid)) => {
                let survivors = if snapshot_cid == cid {
                    snapshot_members
                        .iter()
                        .copied()
                        .filter(|member| *member != id && self.model.field.node(*member).is_some())
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                self.clear_cluster_shell_state(cid);
                if let Some(core_id) = snapshot_core_id.filter(|_| snapshot_cid == cid) {
                    self.model.monitor_state.node_monitor.remove(&core_id);
                }
                for survivor in survivors {
                    let _ = self.model.field.set_detached(survivor, false);
                    if let Some(size) = self
                        .model
                        .workspace_state
                        .last_active_size
                        .get(&survivor)
                        .copied()
                    {
                        if let Some(node) = self.model.field.node_mut(survivor) {
                            node.intrinsic_size = size;
                        }
                        self.request_toplevel_resize(
                            survivor,
                            size.x.round() as i32,
                            size.y.round() as i32,
                        );
                    }
                    let _ = self.model.field.touch(survivor, now_ms);
                }
            }
            Some(RemoveNodeClusterEffect::RemovedCore(cid)) => {
                self.model.monitor_state.node_monitor.remove(&id);
                let _ = self.sync_cluster_core_monitor(cid, None);
            }
            None => {}
        }

        true
    }

    pub fn cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        self.cluster_read_controller()
            .cluster_bloom_for_monitor(monitor)
    }

    pub fn open_cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
    ) -> bool {
        let _ = self.sync_cluster_core_monitor(cid, Some(monitor));
        let opened = self
            .cluster_mutation_controller()
            .open_cluster_bloom_for_monitor(monitor, cid);
        if opened
            && let Some(core_id) = self
                .model
                .field
                .cluster(cid)
                .and_then(|cluster| cluster.core)
        {
            self.set_interaction_focus(Some(core_id), 30_000, Instant::now());
        }
        opened
    }

    pub fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        let closed = self
            .cluster_mutation_controller()
            .close_cluster_bloom_for_monitor(monitor);
        if closed {
            let now = Instant::now();
            let restore = self
                .last_focused_surface_node_for_monitor(monitor)
                .or_else(|| self.last_focused_surface_node());
            self.set_interaction_focus(restore, 30_000, now);
        }
        closed
    }

    pub fn detach_member_from_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now: Instant,
    ) -> bool {
        let now_ms = self.now_ms(now);
        let Some(outcome) = self
            .cluster_mutation_controller()
            .detach_member_from_cluster(cid, member_id, world_pos, now_ms)
        else {
            return false;
        };
        match outcome {
            ClusterRemoveMemberOutcome::Removed => {
                if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None)
                    && self.active_cluster_workspace_for_monitor(cluster_monitor.as_str())
                        == Some(cid)
                {
                    self.layout_active_cluster_workspace_for_monitor(
                        cluster_monitor.as_str(),
                        now_ms,
                    );
                }
            }
            ClusterRemoveMemberOutcome::RequiresDissolve => {
                if !self.dissolve_cluster(cid) {
                    return false;
                }
            }
        }
        true
    }

    pub fn absorb_node_into_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        let previous_overflow_len = self
            .model
            .field
            .cluster(cid)
            .map(|cluster| cluster.overflow_members().len())
            .unwrap_or(0);
        if !self
            .cluster_mutation_controller()
            .absorb_node_into_cluster(cid, node_id)
        {
            return false;
        }
        if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None) {
            self.assign_node_to_monitor(node_id, cluster_monitor.as_str());
            if self.active_cluster_workspace_for_monitor(cluster_monitor.as_str()) == Some(cid) {
                if let Some(node) = self.model.field.node_mut(node_id) {
                    node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                }
                let now_ms = self.now_ms(now);
                self.layout_active_cluster_workspace_for_monitor(cluster_monitor.as_str(), now_ms);
                let overflow_len = self
                    .model
                    .field
                    .cluster(cid)
                    .map(|cluster| cluster.overflow_members().len())
                    .unwrap_or(0);
                if overflow_len > previous_overflow_len {
                    self.reveal_cluster_overflow_for_monitor(cluster_monitor.as_str(), now_ms);
                }
            }
        }
        if let Some(core_id) = self
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| cluster.core)
        {
            let _ = self.model.field.touch(core_id, self.now_ms(now));
        }
        true
    }

    pub(crate) fn commit_ready_cluster_join_for_node(
        &mut self,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        let Some(candidate) = self
            .input
            .interaction_state
            .cluster_join_candidate
            .clone()
            .filter(|candidate| candidate.node_id == node_id && candidate.ready)
        else {
            return false;
        };
        self.input.interaction_state.cluster_join_candidate = None;
        self.absorb_node_into_cluster(candidate.cluster_id, node_id, now)
    }

    pub fn active_cluster_workspace_for_monitor(&self, monitor: &str) -> Option<ClusterId> {
        self.model
            .cluster_state
            .active_cluster_workspaces
            .get(monitor)
            .copied()
    }

    fn refresh_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64, reveal: bool) {
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            self.model
                .cluster_state
                .cluster_overflow_members
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_rects
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor);
            return;
        };
        let Some(cluster) = self.model.field.cluster(cid) else {
            self.model
                .cluster_state
                .cluster_overflow_members
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_rects
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor);
            return;
        };
        let overflow = cluster.overflow_members().to_vec();
        if overflow.is_empty() {
            self.model
                .cluster_state
                .cluster_overflow_members
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_rects
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor);
            return;
        }

        self.model
            .cluster_state
            .cluster_overflow_members
            .insert(monitor.to_string(), overflow.clone());
        if let Some(rect) = self
            .cluster_read_controller()
            .overflow_strip_rect_for_monitor(monitor, overflow.len())
        {
            self.model
                .cluster_state
                .cluster_overflow_rects
                .insert(monitor.to_string(), rect);
        }
        if reveal {
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .insert(
                    monitor.to_string(),
                    now_ms.saturating_add(Self::CLUSTER_OVERFLOW_REVEAL_MS),
                );
        }
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, true);
    }

    pub(crate) fn hide_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        self.model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .remove(monitor);
    }

    pub(crate) fn cluster_overflow_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        self.model
            .cluster_state
            .cluster_overflow_rects
            .get(monitor)
            .copied()
    }

    pub(crate) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .cluster_spawn_rect_for_new_member(monitor, cid)
    }

    pub fn has_any_active_cluster_workspace(&self) -> bool {
        !self
            .model
            .cluster_state
            .active_cluster_workspaces
            .is_empty()
    }

    pub(crate) fn swap_cluster_overflow_member_with_visible(
        &mut self,
        monitor: &str,
        cid: ClusterId,
        overflow_member: NodeId,
        visible_member: NodeId,
        now_ms: u64,
    ) -> bool {
        if self.active_cluster_workspace_for_monitor(monitor) != Some(cid) {
            return false;
        }
        if !self.model.field.swap_cluster_overflow_member_with_visible(
            cid,
            overflow_member,
            visible_member,
        ) {
            return false;
        }
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        self.reveal_cluster_overflow_for_monitor(monitor, now_ms);
        true
    }

    pub(crate) fn reorder_cluster_overflow_member(
        &mut self,
        monitor: &str,
        cid: ClusterId,
        member: NodeId,
        target_overflow_index: usize,
        now_ms: u64,
    ) -> bool {
        if self.active_cluster_workspace_for_monitor(monitor) != Some(cid) {
            return false;
        }
        if !self
            .model
            .field
            .reorder_cluster_overflow_member(cid, member, target_overflow_index)
        {
            return false;
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, true);
        true
    }

    pub(crate) fn move_active_cluster_member_to_drop_tile(
        &mut self,
        monitor: &str,
        member: NodeId,
        world_pos: Vec2,
        now_ms: u64,
    ) -> bool {
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return false;
        };
        let Some(cluster) = self.model.field.cluster(cid) else {
            return false;
        };
        if !cluster.visible_members().contains(&member) {
            return false;
        }
        let Some(target_member) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
            .and_then(|plan| {
                plan.tiles
                    .into_iter()
                    .find(|tile| {
                        world_pos.x >= tile.rect.x
                            && world_pos.x <= tile.rect.x + tile.rect.w
                            && world_pos.y >= tile.rect.y
                            && world_pos.y <= tile.rect.y + tile.rect.h
                    })
                    .map(|tile| tile.node_id)
            })
        else {
            return false;
        };
        if target_member == member {
            return false;
        }

        let members = cluster.members().to_vec();
        let Some(from_index) = members.iter().position(|&id| id == member) else {
            return false;
        };
        let Some(target_index) = members.iter().position(|&id| id == target_member) else {
            return false;
        };

        let mut reordered = members;
        let moved = reordered.remove(from_index);
        reordered.insert(target_index.min(reordered.len()), moved);
        if self
            .model
            .field
            .reorder_cluster_members(cid, reordered)
            .is_err()
        {
            return false;
        }
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        true
    }

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.exit_cluster_workspace_for_monitor(monitor.as_str(), now)
    }

    pub fn cluster_mode_active(&self) -> bool {
        self.cluster_mode_active_for_monitor(self.model.monitor_state.current_monitor.as_str())
    }

    pub fn cluster_mode_active_for_monitor(&self, monitor: &str) -> bool {
        self.model
            .cluster_state
            .cluster_mode_selected_nodes
            .contains_key(monitor)
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        if self
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Cluster mode unavailable\nExit the workspace first",
                3200,
                self.now_ms(Instant::now()),
            );
            return false;
        }
        if !self
            .cluster_mutation_controller()
            .enter_cluster_mode(monitor.as_str())
        {
            return false;
        }
        self.ui.render_state.set_persistent_mode_banner(
            monitor.as_str(),
            "Cluster mode",
            Some("Select windows • Enter to create • Esc to cancel"),
        );
        true
    }

    pub fn exit_cluster_mode(&mut self) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        if !self
            .cluster_mutation_controller()
            .exit_cluster_mode(monitor.as_str())
        {
            return false;
        }
        self.ui
            .render_state
            .clear_persistent_mode_banner(monitor.as_str());
        true
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.cluster_mutation_controller()
            .toggle_cluster_mode_selection(monitor.as_str(), node_id)
    }

    fn order_cluster_creation_members(&self, members: Vec<NodeId>) -> Vec<NodeId> {
        if members.len() <= 1 {
            return members;
        }

        let master = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|id| members.contains(id))
            .or_else(|| {
                members.iter().copied().max_by_key(|id| {
                    (
                        self.model
                            .focus_state
                            .last_surface_focus_ms
                            .get(id)
                            .copied()
                            .unwrap_or(0),
                        std::cmp::Reverse(id.as_u64()),
                    )
                })
            })
            .unwrap_or(members[0]);

        let mut secondaries = members
            .into_iter()
            .filter(|id| *id != master)
            .collect::<Vec<_>>();
        secondaries.sort_by_key(|id| id.as_u64());

        let mut ordered = Vec::with_capacity(secondaries.len() + 1);
        ordered.push(master);
        ordered.extend(secondaries);
        ordered
    }

    pub fn confirm_cluster_mode(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        let Some(selected_nodes) = self
            .model
            .cluster_state
            .cluster_mode_selected_nodes
            .get(monitor.as_str())
        else {
            return false;
        };
        if selected_nodes.is_empty() {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "No selections\nSelect at least two windows",
                2200,
                self.now_ms(now),
            );
            return false;
        }

        let members = selected_nodes.iter().copied().collect::<Vec<_>>();
        if members.len() == 1 {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Not enough selections\nSelect at least two windows",
                5000,
                self.now_ms(now),
            );
            return false;
        }
        let members = self.order_cluster_creation_members(members);
        let created = self
            .model
            .field
            .create_cluster(members)
            .ok()
            .and_then(|cid| {
                let core = self.model.field.collapse_cluster(cid);
                if let Some(core_id) = core {
                    self.assign_node_to_current_monitor(core_id);
                    let _ = self.model.field.touch(core_id, self.now_ms(now));
                    self.set_interaction_focus(Some(core_id), 30_000, now);
                }
                core
            });
        let _ = self.exit_cluster_mode();
        created.is_some()
    }

    pub fn toggle_cluster_workspace_by_core(&mut self, core_id: NodeId, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        if let Some(cid) = self.active_cluster_workspace_for_monitor(monitor.as_str())
            && self.model.field.cluster_id_for_core_public(core_id) == Some(cid)
        {
            return self.exit_cluster_workspace_for_monitor(monitor.as_str(), now);
        }
        self.enter_cluster_workspace_by_core(core_id, monitor.as_str(), now)
    }

    pub fn has_active_cluster_workspace(&self) -> bool {
        self.active_cluster_workspace_for_monitor(self.model.monitor_state.current_monitor.as_str())
            .is_some()
    }

    pub fn exit_cluster_workspace_if_member(&mut self, member: NodeId, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor.as_str()) else {
            return false;
        };
        let Some(c) = self.model.field.cluster(cid) else {
            return false;
        };
        if !c.contains(member) {
            return false;
        }
        self.exit_cluster_workspace_for_monitor(monitor.as_str(), now)
    }

    fn enter_cluster_workspace_by_core(
        &mut self,
        core_id: NodeId,
        monitor: &str,
        now: Instant,
    ) -> bool {
        let Some(cid) = self.model.field.cluster_id_for_core_public(core_id) else {
            return false;
        };
        if self.active_cluster_workspace_for_monitor(monitor) == Some(cid) {
            return true;
        }
        if self.active_cluster_workspace_for_monitor(monitor).is_some() {
            let _ = self.exit_cluster_workspace_for_monitor(monitor, now);
        }
        let Some(plan) = self
            .cluster_read_controller()
            .plan_enter_cluster_workspace(core_id, monitor)
        else {
            return false;
        };
        let _ = self.sync_cluster_core_monitor(cid, Some(monitor));
        self.model
            .cluster_state
            .workspace_prev_viewports
            .insert(monitor.to_string(), plan.current_viewport);
        self.model
            .cluster_state
            .workspace_core_positions
            .insert(monitor.to_string(), plan.core_pos);
        if self.model.monitor_state.current_monitor == monitor {
            self.input.interaction_state.viewport_pan_anim = None;
            self.model.viewport = plan.current_viewport;
            self.model.zoom_ref_size = plan.current_viewport.size;
            self.model.camera_target_center = plan.current_viewport.center;
            self.model.camera_target_view_size = plan.current_viewport.size;
            self.runtime.tuning.viewport_center = plan.current_viewport.center;
            self.runtime.tuning.viewport_size = plan.current_viewport.size;
        }
        for id in &plan.hidden_ids {
            let _ = self.model.field.set_detached(*id, true);
        }
        let _ = self.model.field.set_detached(plan.core_id, true);
        let _ = self.model.field.activate_cluster_workspace(plan.cid);

        self.model
            .cluster_state
            .workspace_hidden_nodes
            .insert(monitor.to_string(), plan.hidden_ids);
        self.model
            .cluster_state
            .active_cluster_workspaces
            .retain(|name, active_cid| *active_cid != cid || name == monitor);
        self.model
            .cluster_state
            .active_cluster_workspaces
            .insert(monitor.to_string(), cid);
        self.model.cluster_state.cluster_bloom_open.remove(monitor);
        self.set_interaction_focus(None, 0, now);
        let now_ms = self.now_ms(now);
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, false);
        true
    }

    fn exit_cluster_workspace_for_monitor(&mut self, monitor: &str, now: Instant) -> bool {
        let Some(plan) = self
            .cluster_read_controller()
            .plan_exit_cluster_workspace(monitor)
        else {
            return false;
        };

        for id in &plan.hidden_ids {
            let _ = self.model.field.set_detached(*id, false);
        }

        let _ = self.model.field.deactivate_cluster_workspace(plan.cid);
        let core = self.model.field.collapse_cluster(plan.cid).or(plan.core_id);
        if let Some(core_id) = core {
            let preserved_core_pos = self
                .model
                .cluster_state
                .workspace_core_positions
                .remove(monitor)
                .or(plan.core_pos);
            if let Some(core_pos) = preserved_core_pos {
                let _ = self.model.field.carry(core_id, core_pos);
            }
            let _ = self.model.field.set_detached(core_id, false);
            self.assign_node_to_monitor(core_id, monitor);
            let _ = self.model.field.touch(core_id, self.now_ms(now));
        }

        self.restore_cluster_workspace_monitor(monitor);
        self.model
            .cluster_state
            .active_cluster_workspaces
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_members
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_rects
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .remove(monitor);
        if self
            .input
            .interaction_state
            .cluster_overflow_drag_preview
            .as_ref()
            .is_some_and(|preview| preview.monitor == monitor)
        {
            self.input.interaction_state.cluster_overflow_drag_preview = None;
            crate::compositor::interaction::pointer::set_cursor_override_icon(self, None);
        }
        true
    }

    pub(crate) fn layout_active_cluster_workspace_for_monitor(
        &mut self,
        monitor: &str,
        now_ms: u64,
    ) {
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return;
        };
        let Some(cluster) = self.model.field.cluster(cid) else {
            self.model
                .cluster_state
                .active_cluster_workspaces
                .remove(monitor);
            return;
        };
        let members = cluster.members().to_vec();
        let dragged_member = self
            .input
            .interaction_state
            .drag_authority_node
            .filter(|id| members.contains(id));
        if self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(monitor)
            .is_some_and(|fullscreen_id| members.contains(fullscreen_id))
        {
            return;
        }
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
            return;
        };
        let visible_members = plan
            .tiles
            .iter()
            .map(|tile| tile.node_id)
            .collect::<std::collections::HashSet<_>>();
        for member_id in &members {
            if let Some(cluster) = self.model.field.cluster_mut(cid)
                && let Some(node) = cluster.workspace_member_mut(*member_id)
            {
                let visible = visible_members.contains(member_id);
                node.visibility.set(Visibility::DETACHED, !visible);
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, !visible);
            }
        }
        for placement in plan.tiles {
            let nid = placement.node_id;
            if Some(nid) == dragged_member {
                continue;
            }
            let rect = placement.rect;
            if let Some(cluster) = self.model.field.cluster_mut(cid)
                && let Some(node) = cluster.workspace_member_mut(nid)
            {
                node.visibility.set(Visibility::DETACHED, false);
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                node.intrinsic_size.x = rect.w.max(64.0);
                node.intrinsic_size.y = rect.h.max(64.0);
                node.state = halley_core::field::NodeState::Active;
                node.footprint = node.resize_footprint.unwrap_or(node.intrinsic_size);
                node.pos = Vec2 {
                    x: rect.x + rect.w * 0.5,
                    y: rect.y + rect.h * 0.5,
                };
            }
            self.set_last_active_size_now(
                nid,
                Vec2 {
                    x: rect.w.max(64.0),
                    y: rect.h.max(64.0),
                },
            );
            self.request_toplevel_resize(nid, rect.w.round() as i32, rect.h.round() as i32);
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, false);
    }
}
