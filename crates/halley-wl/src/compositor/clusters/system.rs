use super::*;
use crate::compositor::clusters::read::{
    ClusterLayoutPlan, ClusterReadController, ClusterTilePlacement, EnterClusterWorkspacePlan,
    ExitClusterWorkspacePlan,
};
use crate::compositor::clusters::state::ClusterState;
use crate::compositor::interaction::state::InteractionState;
use crate::overlay::OverlayActionHint;
use halley_config::DirectionalAction;
use halley_core::cluster::{ClusterId, ClusterRemoveMemberOutcome};
use halley_core::cluster_layout::{ClusterCycleDirection, ClusterWorkspaceLayoutKind};
use halley_core::field::RemoveNodeClusterEffect;
use std::ops::{Deref, DerefMut};

struct ClusterMutationController<'a> {
    field: &'a mut Field,
    cluster_state: &'a mut ClusterState,
    interaction_state: &'a mut InteractionState,
    tuning: &'a halley_config::RuntimeTuning,
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
        let add_result = if matches!(
            self.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) {
            self.field.add_member_to_cluster_front(cid, node_id)
        } else {
            self.field.add_member_to_cluster(cid, node_id)
        };
        if add_result.is_err() {
            return false;
        }
        if self.tuning.tile_new_on_top
            && matches!(
                self.tuning.cluster_layout_kind(),
                ClusterWorkspaceLayoutKind::Tiling
            )
        {
            let _ = self.field.promote_cluster_member_to_master(cid, node_id);
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

fn rect_center(rect: halley_core::tiling::Rect) -> Vec2 {
    Vec2 {
        x: rect.x + rect.w * 0.5,
        y: rect.y + rect.h * 0.5,
    }
}

fn range_gap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    if a1 < b0 {
        b0 - a1
    } else if b1 < a0 {
        a0 - b1
    } else {
        0.0
    }
}

fn directional_candidate_score(
    current: halley_core::tiling::Rect,
    candidate: halley_core::tiling::Rect,
    direction: DirectionalAction,
) -> Option<(f32, f32, f32, f32)> {
    let current_center = rect_center(current);
    let candidate_center = rect_center(candidate);
    let (main_delta, orth_gap, tie_axis) = match direction {
        DirectionalAction::Left => (
            current_center.x - candidate_center.x,
            range_gap(
                current.y,
                current.y + current.h,
                candidate.y,
                candidate.y + candidate.h,
            ),
            candidate_center.y,
        ),
        DirectionalAction::Right => (
            candidate_center.x - current_center.x,
            range_gap(
                current.y,
                current.y + current.h,
                candidate.y,
                candidate.y + candidate.h,
            ),
            candidate_center.y,
        ),
        DirectionalAction::Up => (
            current_center.y - candidate_center.y,
            range_gap(
                current.x,
                current.x + current.w,
                candidate.x,
                candidate.x + candidate.w,
            ),
            candidate_center.x,
        ),
        DirectionalAction::Down => (
            candidate_center.y - current_center.y,
            range_gap(
                current.x,
                current.x + current.w,
                candidate.x,
                candidate.x + candidate.w,
            ),
            candidate_center.x,
        ),
    };
    if main_delta <= 0.5 {
        return None;
    }
    let dx = candidate_center.x - current_center.x;
    let dy = candidate_center.y - current_center.y;
    Some((orth_gap, main_delta, dx * dx + dy * dy, tie_axis))
}

pub(crate) struct ClusterSystemController<T> {
    st: T,
}

pub(crate) fn cluster_system_controller<T>(st: T) -> ClusterSystemController<T> {
    ClusterSystemController { st }
}

pub(crate) fn active_cluster_workspace_for_monitor(
    st: &Halley,
    monitor: &str,
) -> Option<ClusterId> {
    st.model
        .cluster_state
        .active_cluster_workspaces
        .get(monitor)
        .copied()
}

pub(crate) fn stack_layout_rects_for_members(
    st: &Halley,
    monitor: &str,
    members: &[NodeId],
) -> Option<std::collections::HashMap<NodeId, halley_core::tiling::Rect>> {
    cluster_system_controller(st).stack_layout_rects_for_members(monitor, members)
}

impl<T: Deref<Target = Halley>> Deref for ClusterSystemController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for ClusterSystemController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> ClusterSystemController<T> {
    const CLUSTER_OVERFLOW_VISIBLE_SLOTS: usize = 15;

    fn cluster_read_controller(&self) -> ClusterReadController<'_> {
        ClusterReadController {
            field: &self.model.field,
            cluster_state: &self.model.cluster_state,
            monitor_state: &self.model.monitor_state,
            tuning: &self.runtime.tuning,
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

    pub(crate) fn cluster_overflow_slot_rect_for_monitor(
        &self,
        monitor: &str,
        overflow_len: usize,
        slot_index: usize,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .overflow_strip_slot_rect_for_monitor(monitor, overflow_len, slot_index)
    }

    pub(crate) fn active_cluster_tile_rect_for_member(
        &self,
        monitor: &str,
        member_id: NodeId,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .plan_active_cluster_layout(monitor)?
            .tiles
            .into_iter()
            .find(|tile| tile.node_id == member_id)
            .map(|tile| tile.rect)
    }

    pub(crate) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_read_controller()
            .cluster_spawn_rect_for_new_member(monitor, cid)
    }

    pub(crate) fn stack_layout_rects_for_members(
        &self,
        monitor: &str,
        members: &[NodeId],
    ) -> Option<std::collections::HashMap<NodeId, halley_core::tiling::Rect>> {
        self.cluster_read_controller()
            .stack_layout_rects_for_members(monitor, members)
    }

    pub fn has_any_active_cluster_workspace(&self) -> bool {
        !self
            .model
            .cluster_state
            .active_cluster_workspaces
            .is_empty()
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

    pub fn has_active_cluster_workspace(&self) -> bool {
        self.active_cluster_workspace_for_monitor(self.model.monitor_state.current_monitor.as_str())
            .is_some()
    }

    fn active_cluster_layout_kind(&self) -> ClusterWorkspaceLayoutKind {
        self.runtime.tuning.cluster_layout_kind()
    }

    fn cluster_overflow_len(&self, cid: ClusterId) -> usize {
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            return 0;
        }
        self.model
            .field
            .cluster(cid)
            .map(|cluster| {
                cluster
                    .overflow_members(self.runtime.tuning.tile_max_stack)
                    .len()
            })
            .unwrap_or(0)
    }
}

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
    const CLUSTER_OVERFLOW_REVEAL_MS: u64 = 2200;

    pub(crate) fn adjust_cluster_overflow_scroll_for_monitor(
        &mut self,
        monitor: &str,
        delta: i32,
    ) -> bool {
        let overflow_len = self
            .model
            .cluster_state
            .cluster_overflow_members
            .get(monitor)
            .map(Vec::len)
            .unwrap_or(0);
        let max_offset = overflow_len.saturating_sub(Self::CLUSTER_OVERFLOW_VISIBLE_SLOTS);
        if max_offset == 0 {
            self.model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .remove(monitor);
            return false;
        }
        let current = self
            .model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .get(monitor)
            .copied()
            .unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, max_offset as i32) as usize;
        if next == current as usize {
            return false;
        }
        self.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .insert(monitor.to_string(), next);
        true
    }

    fn cluster_mutation_controller(&mut self) -> ClusterMutationController<'_> {
        let crate::compositor::root::Halley {
            model,
            input,
            runtime,
            ..
        } = &mut **self;
        ClusterMutationController {
            field: &mut model.field,
            cluster_state: &mut model.cluster_state,
            interaction_state: &mut input.interaction_state,
            tuning: &runtime.tuning,
        }
    }

    pub(crate) fn sync_cluster_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        let Some(target_monitor) = self.preferred_monitor_for_cluster(cid, preferred) else {
            return false;
        };

        let (core_id, members) = if let Some(cluster) = self.model.field.cluster(cid) {
            (cluster.core, cluster.members().to_vec())
        } else {
            return false;
        };

        if let Some(core_id) = core_id {
            self.assign_node_to_monitor(core_id, target_monitor.as_str());
        }
        for member_id in members {
            self.assign_node_to_monitor(member_id, target_monitor.as_str());
        }
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
                let _ = self.sync_cluster_monitor(cid, None);
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
        let _ = self.sync_cluster_monitor(cid, Some(monitor));
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
        let previous_overflow_len = self.cluster_overflow_len(cid);
        let stack_insert_transition = self
            .preferred_monitor_for_cluster(cid, None)
            .filter(|monitor| {
                self.active_cluster_workspace_for_monitor(monitor.as_str()) == Some(cid)
            })
            .filter(|_| {
                matches!(
                    self.active_cluster_layout_kind(),
                    ClusterWorkspaceLayoutKind::Stacking
                )
            })
            .map(|monitor| {
                let old_visible =
                    crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                        self,
                        monitor.as_str(),
                    );
                (monitor, old_visible)
            });
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
                if matches!(
                    self.active_cluster_layout_kind(),
                    ClusterWorkspaceLayoutKind::Stacking
                ) {
                    if let Some((transition_monitor, old_visible)) =
                        stack_insert_transition.as_ref()
                        && transition_monitor == &cluster_monitor
                    {
                        let new_visible = crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                            self,
                            cluster_monitor.as_str(),
                        );
                        self.ui.render_state.start_stack_cycle_transition(
                            cluster_monitor.as_str(),
                            ClusterCycleDirection::Prev,
                            old_visible.clone(),
                            new_visible,
                            now,
                            220,
                        );
                        self.request_maintenance();
                    }
                    self.set_recent_top_node(node_id, now + std::time::Duration::from_millis(1200));
                    self.set_interaction_focus(Some(node_id), 30_000, now);
                    self.update_focus_tracking_for_surface(node_id, now_ms);
                }
                let overflow_len = self.cluster_overflow_len(cid);
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
            let now_ms = self.now_ms(now);
            let _ = self.model.field.touch(core_id, now_ms);
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
        let Some(_cid) = self.active_cluster_workspace_for_monitor(monitor) else {
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
                .cluster_overflow_scroll_offsets
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_reveal_started_at_ms
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor);
            return;
        };
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
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
                .cluster_overflow_scroll_offsets
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_reveal_started_at_ms
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor);
            return;
        };
        let overflow = plan.overflow_members;
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
                .cluster_overflow_scroll_offsets
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_reveal_started_at_ms
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .remove(monitor);
            return;
        }

        let was_visible = self
            .model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .get(monitor)
            .is_some_and(|visible_until_ms| *visible_until_ms > now_ms);

        self.model
            .cluster_state
            .cluster_overflow_members
            .insert(monitor.to_string(), overflow.clone());
        let max_offset = overflow
            .len()
            .saturating_sub(Self::CLUSTER_OVERFLOW_VISIBLE_SLOTS);
        if max_offset == 0 {
            self.model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .remove(monitor);
        } else {
            let next = self
                .model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .get(monitor)
                .copied()
                .unwrap_or(0)
                .min(max_offset);
            self.model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .insert(monitor.to_string(), next);
        }
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
            if !was_visible {
                self.model
                    .cluster_state
                    .cluster_overflow_reveal_started_at_ms
                    .insert(monitor.to_string(), now_ms);
            }
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .insert(
                    monitor.to_string(),
                    now_ms.saturating_add(Self::CLUSTER_OVERFLOW_REVEAL_MS),
                );
            self.request_maintenance();
        }
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, true);
    }

    pub(crate) fn hide_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        self.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_reveal_started_at_ms
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .remove(monitor);
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
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            return false;
        }
        let max_stack = self.runtime.tuning.tile_max_stack;
        if !self.model.field.swap_cluster_overflow_member_with_visible(
            cid,
            overflow_member,
            visible_member,
            max_stack,
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
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            return false;
        }
        let max_stack = self.runtime.tuning.tile_max_stack;
        if !self.model.field.reorder_cluster_overflow_member(
            cid,
            member,
            target_overflow_index,
            max_stack,
        ) {
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
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
            return false;
        };
        if !matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling)
            || !plan.tiles.iter().any(|tile| tile.node_id == member)
        {
            return false;
        }
        let Some(target_member) = plan
            .tiles
            .into_iter()
            .find(|tile| {
                world_pos.x >= tile.rect.x
                    && world_pos.x <= tile.rect.x + tile.rect.w
                    && world_pos.y >= tile.rect.y
                    && world_pos.y <= tile.rect.y + tile.rect.h
            })
            .map(|tile| tile.node_id)
        else {
            return false;
        };
        if target_member == member {
            return false;
        }

        let Some(members) = self
            .model
            .field
            .cluster(cid)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
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

    pub(crate) fn focus_active_tiled_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        preferred_index: Option<usize>,
        now: Instant,
    ) -> bool {
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
            return false;
        };
        if !matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
            return false;
        }
        let visible_members = plan
            .tiles
            .into_iter()
            .map(|tile| tile.node_id)
            .filter(|id| {
                !self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(id)
            })
            .collect::<Vec<_>>();
        let Some(target) = visible_members
            .get(
                preferred_index
                    .unwrap_or(0)
                    .min(visible_members.len().saturating_sub(1)),
            )
            .copied()
        else {
            return false;
        };
        let now_ms = self.now_ms(now);
        self.set_interaction_focus(Some(target), 30_000, now);
        self.update_focus_tracking_for_surface(target, now_ms);
        true
    }

    pub(crate) fn tile_focus_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: DirectionalAction,
        now: Instant,
    ) -> bool {
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
            return false;
        };
        if !matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
            return false;
        }
        let visible_tiles = plan
            .tiles
            .into_iter()
            .filter(|tile| {
                !self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(&tile.node_id)
            })
            .collect::<Vec<_>>();
        if visible_tiles.is_empty() {
            return false;
        }
        let current_member = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|id| visible_tiles.iter().any(|tile| tile.node_id == *id))
            .unwrap_or(visible_tiles[0].node_id);
        let Some(current_rect) = visible_tiles
            .iter()
            .find(|tile| tile.node_id == current_member)
            .map(|tile| tile.rect)
        else {
            return false;
        };
        let Some(target) = visible_tiles
            .iter()
            .filter(|tile| tile.node_id != current_member)
            .filter_map(|tile| {
                directional_candidate_score(current_rect, tile.rect, direction)
                    .map(|score| (score, tile.node_id))
            })
            .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, node_id)| node_id)
        else {
            return false;
        };
        let now_ms = self.now_ms(now);
        self.set_interaction_focus(Some(target), 30_000, now);
        self.update_focus_tracking_for_surface(target, now_ms);
        true
    }

    pub(crate) fn tile_swap_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: DirectionalAction,
        now: Instant,
    ) -> bool {
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return false;
        };
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
            return false;
        };
        if !matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
            return false;
        }
        let visible_tiles = plan
            .tiles
            .into_iter()
            .filter(|tile| {
                !self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(&tile.node_id)
            })
            .collect::<Vec<_>>();
        if visible_tiles.len() < 2 {
            return false;
        }
        let current_member = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|id| visible_tiles.iter().any(|tile| tile.node_id == *id))
            .unwrap_or(visible_tiles[0].node_id);
        let Some(current_rect) = visible_tiles
            .iter()
            .find(|tile| tile.node_id == current_member)
            .map(|tile| tile.rect)
        else {
            return false;
        };
        let Some(target_member) = visible_tiles
            .iter()
            .filter(|tile| tile.node_id != current_member)
            .filter_map(|tile| {
                directional_candidate_score(current_rect, tile.rect, direction)
                    .map(|score| (score, tile.node_id))
            })
            .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, node_id)| node_id)
        else {
            return false;
        };

        let Some(mut members) = self
            .model
            .field
            .cluster(cid)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
        let Some(current_index) = members.iter().position(|id| *id == current_member) else {
            return false;
        };
        let Some(target_index) = members.iter().position(|id| *id == target_member) else {
            return false;
        };
        members.swap(current_index, target_index);
        if self
            .model
            .field
            .reorder_cluster_members(cid, members)
            .is_err()
        {
            return false;
        }
        let now_ms = self.now_ms(now);
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        self.set_interaction_focus(Some(current_member), 30_000, now);
        self.update_focus_tracking_for_surface(current_member, now_ms);
        true
    }

    pub(crate) fn cycle_active_stack_for_monitor(
        &mut self,
        monitor: &str,
        direction: ClusterCycleDirection,
        now: Instant,
    ) -> bool {
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) {
            return false;
        }
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return false;
        };
        let old_visible =
            crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                self, monitor,
            );
        let Some(front) = self
            .model
            .field
            .cycle_cluster_stacking_members(cid, direction)
        else {
            return false;
        };
        if self.focused_monitor() != monitor {
            self.focus_monitor_view(monitor, now);
        }
        let now_ms = self.now_ms(now);
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        let new_visible =
            crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                self, monitor,
            );
        self.ui.render_state.start_stack_cycle_transition(
            monitor,
            direction,
            old_visible,
            new_visible,
            now,
            220,
        );
        self.request_maintenance();
        self.set_recent_top_node(front, now + std::time::Duration::from_millis(1200));
        self.set_interaction_focus(Some(front), 30_000, now);
        self.update_focus_tracking_for_surface(front, now_ms);
        true
    }

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.exit_cluster_workspace_for_monitor(monitor.as_str(), now)
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        if self
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            let now_ms = self.now_ms(Instant::now());
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Cluster mode unavailable\nExit the workspace first",
                3200,
                now_ms,
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
            Some("Select windows"),
            &[
                OverlayActionHint {
                    key: "Enter".to_string(),
                    label: "create".to_string(),
                },
                OverlayActionHint {
                    key: "Esc".to_string(),
                    label: "cancel".to_string(),
                },
            ],
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
        let now_ms = self.now_ms(now);
        if selected_nodes.is_empty() {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "No selections\nSelect at least two windows",
                2200,
                now_ms,
            );
            return false;
        }

        let members = selected_nodes.iter().copied().collect::<Vec<_>>();
        if members.len() == 1 {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Not enough selections\nSelect at least two windows",
                5000,
                now_ms,
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
                    let _ = self.model.field.touch(core_id, now_ms);
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

    pub(crate) fn enter_cluster_workspace_by_core(
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
        let _ = self.sync_cluster_monitor(cid, Some(monitor));
        let previous_full_viewport = if self.model.monitor_state.current_monitor == monitor {
            self.model.viewport
        } else {
            self.model
                .monitor_state
                .monitors
                .get(monitor)
                .map(|space| space.viewport)
                .unwrap_or(plan.current_viewport)
        };
        self.model
            .cluster_state
            .workspace_prev_viewports
            .insert(monitor.to_string(), previous_full_viewport);
        self.model
            .cluster_state
            .workspace_core_positions
            .insert(monitor.to_string(), plan.core_pos);
        if self.model.monitor_state.current_monitor == monitor {
            let live_viewport = self
                .model
                .monitor_state
                .monitors
                .get(monitor)
                .map(|space| space.viewport)
                .unwrap_or(plan.current_viewport);
            self.input.interaction_state.viewport_pan_anim = None;
            self.model.viewport = live_viewport;
            self.model.zoom_ref_size = live_viewport.size;
            self.model.camera_target_center = live_viewport.center;
            self.model.camera_target_view_size = live_viewport.size;
            self.runtime.tuning.viewport_center = live_viewport.center;
            self.runtime.tuning.viewport_size = live_viewport.size;
        }
        self.model.spawn_state.pending_spawn_pan_queue.clear();
        self.model.spawn_state.active_spawn_pan = None;
        self.input.interaction_state.viewport_pan_anim = None;
        self.model.spawn_state.pending_spawn_monitor = None;
        let spawn = self.spawn_monitor_state_mut(monitor);
        spawn.spawn_pan_start_center = None;
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
        if matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) && let Some(front) = self
            .model
            .field
            .cluster(plan.cid)
            .and_then(|cluster| cluster.members().first().copied())
        {
            self.set_recent_top_node(front, now + std::time::Duration::from_millis(1200));
            self.set_interaction_focus(Some(front), 30_000, now);
            self.update_focus_tracking_for_surface(front, now_ms);
        } else if matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            let _ = self.focus_active_tiled_cluster_member_for_monitor(monitor, Some(0), now);
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, false);
        true
    }

    pub(crate) fn exit_cluster_workspace_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> bool {
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
            let now_ms = self.now_ms(now);
            let _ = self.model.field.touch(core_id, now_ms);
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
        let now = Instant::now();
        if matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
            for placement in &plan.tiles {
                if self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(&placement.node_id)
                {
                    self.ui
                        .render_state
                        .cluster_tile_tracks
                        .remove(&placement.node_id);
                    self.ui
                        .render_state
                        .cluster_tile_entry_pending
                        .remove(&placement.node_id);
                    self.ui
                        .render_state
                        .cluster_tile_frozen_geometry
                        .remove(&placement.node_id);
                    continue;
                }
                if Some(placement.node_id) == dragged_member {
                    self.ui
                        .render_state
                        .cluster_tile_tracks
                        .remove(&placement.node_id);
                    self.ui
                        .render_state
                        .cluster_tile_entry_pending
                        .remove(&placement.node_id);
                    self.ui
                        .render_state
                        .cluster_tile_frozen_geometry
                        .remove(&placement.node_id);
                    continue;
                }
                let current_rect = if self
                    .ui
                    .render_state
                    .cluster_tile_entry_pending
                    .remove(&placement.node_id)
                {
                    None
                } else {
                    crate::animation::cluster_tile_rect_from_field(
                        &self.model.field,
                        placement.node_id,
                    )
                };
                let frozen_geo = self
                    .ui
                    .render_state
                    .window_geometry
                    .get(&placement.node_id)
                    .copied();
                if current_rect.is_some_and(|rect| rect.alpha > 0.01)
                    && let Some(geo) = frozen_geo
                {
                    self.ui
                        .render_state
                        .cluster_tile_frozen_geometry
                        .entry(placement.node_id)
                        .or_insert(geo);
                }
                crate::animation::set_cluster_tile_target(
                    &mut self.ui.render_state.cluster_tile_tracks,
                    current_rect,
                    placement.node_id,
                    placement.rect,
                    now,
                );
            }
        }
        let visible_members = plan
            .tiles
            .iter()
            .map(|tile| tile.node_id)
            .filter(|id| {
                !self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(id)
            })
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
            if Some(nid) == dragged_member
                || self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(&nid)
            {
                continue;
            }
            let rect = placement.rect;
            let target_size = Vec2 {
                x: rect.w.max(64.0),
                y: rect.h.max(64.0),
            };
            let target_pos = Vec2 {
                x: rect.x + rect.w * 0.5,
                y: rect.y + rect.h * 0.5,
            };
            let layout_changed = self.model.field.node(nid).is_none_or(|node| {
                (node.intrinsic_size.x - target_size.x).abs() > 0.5
                    || (node.intrinsic_size.y - target_size.y).abs() > 0.5
                    || (node.pos.x - target_pos.x).abs() > 0.5
                    || (node.pos.y - target_pos.y).abs() > 0.5
                    || node.state != halley_core::field::NodeState::Active
                    || node.visibility.has(Visibility::DETACHED)
                    || node.visibility.has(Visibility::HIDDEN_BY_CLUSTER)
            });
            if let Some(cluster) = self.model.field.cluster_mut(cid)
                && let Some(node) = cluster.workspace_member_mut(nid)
            {
                if layout_changed {
                    node.visibility.set(Visibility::DETACHED, false);
                    node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                    node.intrinsic_size = target_size;
                    node.state = halley_core::field::NodeState::Active;
                    node.footprint = node.resize_footprint.unwrap_or(node.intrinsic_size);
                    node.pos = target_pos;
                }
            }
            if layout_changed {
                self.set_last_active_size_now(nid, target_size);
            }
            let surface_size_changed =
                crate::compositor::surface_ops::current_surface_size_for_node(self, nid)
                    .is_none_or(|size| {
                        (size.x - target_size.x).abs() > 0.5 || (size.y - target_size.y).abs() > 0.5
                    });
            if surface_size_changed {
                self.request_toplevel_resize(nid, rect.w.round() as i32, rect.h.round() as i32);
            }
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tile_gaps_outer_px = 20.0;
        tuning.tile_gaps_inner_px = 20.0;
        tuning.border_size_px = 0;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 0.5,
            "expected {expected}, got {actual}"
        );
    }

    fn node_edges(st: &Halley, id: NodeId) -> (f32, f32, f32, f32) {
        let node = st.model.field.node(id).expect("node");
        let half_w = node.intrinsic_size.x * 0.5;
        let half_h = node.intrinsic_size.y * 0.5;
        (
            node.pos.x - half_w,
            node.pos.y - half_h,
            node.pos.x + half_w,
            node.pos.y + half_h,
        )
    }

    #[test]
    fn test_cluster_monitor_transfer_reopen() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "monitor_a".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "monitor_b".to_string(),
                enabled: true,
                offset_x: 1920,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];

        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, tuning);

        // 1. Create two surfaces on monitor_a
        let n1 = st.model.field.spawn_surface(
            "monitor_a",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        let n2 = st.model.field.spawn_surface(
            "monitor_a",
            Vec2 { x: 600.0, y: 100.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        st.assign_node_to_monitor(n1, "monitor_a");
        st.assign_node_to_monitor(n2, "monitor_a");

        // 2. Create a cluster
        let cid = st
            .model
            .field
            .create_cluster(vec![n1, n2])
            .expect("cluster");

        // 3. Collapse to core
        let core_id = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core_id, "monitor_a");

        // 4. Move core to monitor_b
        st.assign_node_to_monitor(core_id, "monitor_b");
        // Also move its position to monitor_b's space
        let _ = st.model.field.carry(
            core_id,
            Vec2 {
                x: 1920.0 + 500.0,
                y: 500.0,
            },
        );

        // 5. Reopen/expand cluster on monitor_b
        // We simulate the double-click/enter behavior
        let now = Instant::now();
        st.focus_monitor_view("monitor_b", now);
        let success = st.enter_cluster_workspace_by_core(core_id, "monitor_b", now);
        assert!(success);

        // 6. Verify cluster members are now on monitor_b
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&n1)
                .map(|s| s.as_str()),
            Some("monitor_b")
        );
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&n2)
                .map(|s| s.as_str()),
            Some("monitor_b")
        );

        // 7. Verify core is also on monitor_b
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&core_id)
                .map(|s| s.as_str()),
            Some("monitor_b")
        );
    }

    #[test]
    fn test_cluster_monitor_maintenance_sync() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "monitor_a".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "monitor_b".to_string(),
                enabled: true,
                offset_x: 1920,
                offset_y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];

        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, tuning);

        let n1 = st.model.field.spawn_surface(
            "monitor_a",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        let n2 = st.model.field.spawn_surface(
            "monitor_a",
            Vec2 { x: 600.0, y: 100.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        st.assign_node_to_monitor(n1, "monitor_a");
        st.assign_node_to_monitor(n2, "monitor_a");

        let cid = st
            .model
            .field
            .create_cluster(vec![n1, n2])
            .expect("cluster");
        let core_id = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core_id, "monitor_a");

        // Stale members monitor
        st.assign_node_to_monitor(n1, "monitor_a");
        st.assign_node_to_monitor(n2, "monitor_a");
        // Move core to monitor_b
        st.assign_node_to_monitor(core_id, "monitor_b");

        // MAINTENANCE SYNC (without preferred monitor)
        let success = st.sync_cluster_monitor(cid, None);
        assert!(success);

        // Should have picked monitor_b from core, NOT monitor_a from members
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&n1)
                .map(|s| s.as_str()),
            Some("monitor_b")
        );
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&n2)
                .map(|s| s.as_str()),
            Some("monitor_b")
        );
        assert_eq!(
            st.model
                .monitor_state
                .node_monitor
                .get(&core_id)
                .map(|s| s.as_str()),
            Some("monitor_b")
        );
    }

    #[test]
    fn entering_two_window_cluster_keeps_outer_gap_exact() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, "monitor_a");
        st.assign_node_to_monitor(stack, "monitor_a");
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

        let (master_left, master_top, master_right, master_bottom) = node_edges(&st, master);
        let (stack_left, stack_top, stack_right, stack_bottom) = node_edges(&st, stack);

        assert_close(master_left, 20.0);
        assert_close(master_top, 20.0);
        assert_close(master_bottom, 580.0);
        assert_close(stack_top, 20.0);
        assert_close(stack_bottom, 580.0);
        assert_close(stack_right, 780.0);
        assert_close(stack_left - master_right, 20.0);
    }

    #[test]
    fn entering_three_window_cluster_keeps_master_outer_gap_exact() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_a = st.model.field.spawn_surface(
            "stack-a",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_b = st.model.field.spawn_surface(
            "stack-b",
            Vec2 { x: 500.0, y: 400.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack_a, stack_b] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack_a, stack_b])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

        let (_, master_top, master_right, master_bottom) = node_edges(&st, master);
        let mut stack_edges = [node_edges(&st, stack_a), node_edges(&st, stack_b)];
        stack_edges.sort_by(|a, b| a.1.partial_cmp(&b.1).expect("finite"));
        let upper = stack_edges[0];
        let lower = stack_edges[1];

        assert_close(master_top, 20.0);
        assert_close(master_bottom, 580.0);
        assert_close(upper.1, 20.0);
        assert_close(lower.3, 580.0);
        assert_close(lower.1 - upper.3, 20.0);
        assert_close(upper.0 - master_right, 20.0);
    }

    #[test]
    fn entering_cluster_keeps_current_monitor_live_viewport_full_size() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let full_viewport = st.model.viewport;
        st.model
            .monitor_state
            .monitors
            .get_mut("monitor_a")
            .expect("monitor")
            .usable_viewport = halley_core::viewport::Viewport::new(
            Vec2 { x: 400.0, y: 320.0 },
            Vec2 { x: 800.0, y: 560.0 },
        );

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, "monitor_a");
        st.assign_node_to_monitor(stack, "monitor_a");
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));
        assert_eq!(st.model.viewport, full_viewport);
        assert_eq!(st.model.camera_target_view_size, full_viewport.size);
    }

    #[test]
    fn entering_tiled_cluster_workspace_focuses_master_tile() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, "monitor_a");
        st.assign_node_to_monitor(stack, "monitor_a");
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));
        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(master));
    }

    #[test]
    fn tiled_cluster_focus_retargets_replacement_tile_by_index() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_a = st.model.field.spawn_surface(
            "stack-a",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_b = st.model.field.spawn_surface(
            "stack-b",
            Vec2 { x: 500.0, y: 400.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack_a, stack_b] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack_a, stack_b])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

        let removed = cluster_system_controller(&mut st).detach_member_from_cluster(
            cid,
            stack_a,
            Vec2 { x: 0.0, y: 0.0 },
            now,
        );
        assert!(removed);
        st.layout_active_cluster_workspace_for_monitor("monitor_a", st.now_ms(now));
        assert!(st.focus_active_tiled_cluster_member_for_monitor("monitor_a", Some(1), now));
        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(stack_b)
        );
    }

    #[test]
    fn tile_focus_moves_between_visible_neighbors() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_a = st.model.field.spawn_surface(
            "stack-a",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_b = st.model.field.spawn_surface(
            "stack-b",
            Vec2 { x: 500.0, y: 400.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack_a, stack_b] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack_a, stack_b])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

        assert!(st.tile_focus_active_cluster_member_for_monitor(
            "monitor_a",
            DirectionalAction::Right,
            now,
        ));
        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(stack_a)
        );
        assert!(st.tile_focus_active_cluster_member_for_monitor(
            "monitor_a",
            DirectionalAction::Down,
            now,
        ));
        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(stack_b)
        );
        assert!(st.tile_focus_active_cluster_member_for_monitor(
            "monitor_a",
            DirectionalAction::Left,
            now,
        ));
        assert_eq!(st.model.focus_state.primary_interaction_focus, Some(master));
    }

    #[test]
    fn tile_swap_exchanges_adjacent_visible_tiles() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_a = st.model.field.spawn_surface(
            "stack-a",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_b = st.model.field.spawn_surface(
            "stack-b",
            Vec2 { x: 500.0, y: 400.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack_a, stack_b] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack_a, stack_b])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
        st.set_interaction_focus(Some(stack_a), 30_000, now);

        let before_a = st
            .active_cluster_tile_rect_for_member("monitor_a", stack_a)
            .expect("stack a rect");
        let before_b = st
            .active_cluster_tile_rect_for_member("monitor_a", stack_b)
            .expect("stack b rect");

        assert!(st.tile_swap_active_cluster_member_for_monitor(
            "monitor_a",
            DirectionalAction::Down,
            now,
        ));
        assert_eq!(
            st.model.focus_state.primary_interaction_focus,
            Some(stack_a)
        );

        let after_a = st
            .active_cluster_tile_rect_for_member("monitor_a", stack_a)
            .expect("stack a rect after swap");
        let after_b = st
            .active_cluster_tile_rect_for_member("monitor_a", stack_b)
            .expect("stack b rect after swap");
        assert!((after_a.y - before_b.y).abs() <= 0.5);
        assert!((after_b.y - before_a.y).abs() <= 0.5);
    }

    #[test]
    fn cluster_exit_restores_full_viewport_not_usable_viewport() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

        let full_viewport = st.model.viewport;
        let reduced_usable = halley_core::viewport::Viewport::new(
            Vec2 { x: 400.0, y: 320.0 },
            Vec2 { x: 800.0, y: 560.0 },
        );
        st.model
            .monitor_state
            .monitors
            .get_mut("monitor_a")
            .expect("monitor")
            .usable_viewport = reduced_usable;

        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, "monitor_a");
        st.assign_node_to_monitor(stack, "monitor_a");
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
        assert_eq!(
            st.model
                .cluster_state
                .workspace_prev_viewports
                .get("monitor_a"),
            Some(&full_viewport)
        );

        assert!(st.exit_cluster_workspace_for_monitor("monitor_a", now));
        assert_eq!(st.model.viewport, full_viewport);
        assert_eq!(
            st.model
                .monitor_state
                .monitors
                .get("monitor_a")
                .expect("monitor")
                .viewport,
            full_viewport
        );
        assert_eq!(
            st.model
                .monitor_state
                .monitors
                .get("monitor_a")
                .expect("monitor")
                .usable_viewport,
            reduced_usable
        );
    }
}
