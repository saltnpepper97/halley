use super::*;
use halley_core::cluster::ClusterId;
use crate::state::{ClusterState, InteractionState, MonitorState};

struct ClusterReadController<'a> {
    field: &'a Field,
    cluster_state: &'a ClusterState,
    monitor_state: &'a MonitorState,
    tuning: &'a RuntimeTuning,
}

struct ClusterMutationController<'a> {
    field: &'a mut Field,
    cluster_state: &'a mut ClusterState,
    interaction_state: &'a mut InteractionState,
}

struct EnterClusterWorkspacePlan {
    cid: ClusterId,
    core_id: NodeId,
    current_viewport: halley_core::viewport::Viewport,
    hidden_ids: Vec<NodeId>,
}

struct ExitClusterWorkspacePlan {
    cid: ClusterId,
    core_id: Option<NodeId>,
    hidden_ids: Vec<NodeId>,
}

struct ClusterTilePlacement {
    node_id: NodeId,
    rect: halley_core::tiling::Rect,
}

struct ClusterLayoutPlan {
    overflow_members: Vec<NodeId>,
    overflow_rect: Option<halley_core::tiling::Rect>,
    tiles: Vec<ClusterTilePlacement>,
}

struct ClusterCleanupPlan {
    singleton_clusters: Vec<(ClusterId, NodeId)>,
    empty_clusters: Vec<(ClusterId, Option<NodeId>, Vec<String>)>,
    resync_clusters: Vec<ClusterId>,
}

impl<'a> ClusterReadController<'a> {
    fn cluster_bloom_for_monitor(&self, monitor: &str) -> Option<ClusterId> {
        self.cluster_state.cluster_bloom_open.get(monitor).copied()
    }

    fn preferred_monitor_for_cluster(
        &self,
        cid: ClusterId,
        preferred: Option<&str>,
    ) -> Option<String> {
        preferred
            .map(str::to_string)
            .or_else(|| {
                self.cluster_state
                    .active_cluster_workspaces
                    .iter()
                    .find_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
            })
            .or_else(|| {
                self.cluster_state
                    .cluster_bloom_open
                    .iter()
                    .find_map(|(monitor, open_cid)| (*open_cid == cid).then(|| monitor.clone()))
            })
            .or_else(|| {
                self.field.cluster(cid).and_then(|cluster| {
                    cluster
                        .members
                        .iter()
                        .find_map(|member| self.monitor_state.node_monitor.get(member).cloned())
                })
            })
            .or_else(|| {
                self.field
                    .cluster(cid)
                    .and_then(|cluster| cluster.core)
                    .and_then(|core_id| self.monitor_state.node_monitor.get(&core_id).cloned())
            })
            .or_else(|| Some(self.monitor_state.current_monitor.clone()))
    }

    fn workspace_viewport_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::viewport::Viewport> {
        self.monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport)
    }

    fn opened_cluster_world_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        let viewport = self.workspace_viewport_for_monitor(monitor)?;
        Some(
            halley_core::tiling::Rect {
                x: viewport.center.x - viewport.size.x * 0.5,
                y: viewport.center.y - viewport.size.y * 0.5,
                w: viewport.size.x,
                h: viewport.size.y,
            }
            .inset(self.tuning.tile_gaps_outer_px.max(0.0)),
        )
    }

    fn opened_cluster_tile_rects(
        &self,
        tile_rect: halley_core::tiling::Rect,
        count: usize,
    ) -> Vec<halley_core::tiling::Rect> {
        let tile_inner_gap = self.tuning.tile_gaps_inner_px.max(0.0);
        let split_rect_h = |rect: halley_core::tiling::Rect, parts: usize| {
            let parts = parts.max(1);
            let total_gap = tile_inner_gap * (parts.saturating_sub(1)) as f32;
            let each_h = ((rect.h - total_gap) / parts as f32).max(48.0);
            (0..parts)
                .map(|index| halley_core::tiling::Rect {
                    x: rect.x,
                    y: rect.y + index as f32 * (each_h + tile_inner_gap),
                    w: rect.w,
                    h: each_h,
                })
                .collect::<Vec<_>>()
        };
        let split_rect_w = |rect: halley_core::tiling::Rect, parts: usize| {
            let parts = parts.max(1);
            let total_gap = tile_inner_gap * (parts.saturating_sub(1)) as f32;
            let each_w = ((rect.w - total_gap) / parts as f32).max(64.0);
            (0..parts)
                .map(|index| halley_core::tiling::Rect {
                    x: rect.x + index as f32 * (each_w + tile_inner_gap),
                    y: rect.y,
                    w: each_w,
                    h: rect.h,
                })
                .collect::<Vec<_>>()
        };

        match count {
            0 => Vec::new(),
            1 => vec![tile_rect],
            2 => split_rect_w(tile_rect, 2),
            3 => {
                let left_w = ((tile_rect.w - tile_inner_gap) * 0.58).max(140.0);
                let right_w = (tile_rect.w - left_w - tile_inner_gap).max(120.0);
                let left = halley_core::tiling::Rect {
                    x: tile_rect.x,
                    y: tile_rect.y,
                    w: left_w,
                    h: tile_rect.h,
                };
                let right = halley_core::tiling::Rect {
                    x: tile_rect.x + left_w + tile_inner_gap,
                    y: tile_rect.y,
                    w: right_w,
                    h: tile_rect.h,
                };
                let mut rects = vec![left];
                rects.extend(split_rect_h(right, 2));
                rects
            }
            _ => {
                let rows = split_rect_h(tile_rect, 2);
                let mut rects = Vec::new();
                for row in rows {
                    rects.extend(split_rect_w(row, 2));
                }
                rects
            }
        }
    }

    fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        let cluster = self.field.cluster(cid)?;
        let count = (cluster.members.len() + 1).min(4);
        let tile_rect = self.opened_cluster_world_rect_for_monitor(monitor)?;
        let tile_inset = (self.tuning.tile_gaps_inner_px * 0.5
            + crate::render::ACTIVE_WINDOW_FRAME_PAD_PX as f32)
            .clamp(4.0, 28.0);
        self.opened_cluster_tile_rects(tile_rect, count)
            .into_iter()
            .last()
            .map(|rect| rect.inset(tile_inset))
    }

    fn cluster_spawn_position_for_new_member(&self, monitor: &str, cid: ClusterId) -> Option<Vec2> {
        self.cluster_spawn_rect_for_new_member(monitor, cid)
            .map(|rect| Vec2 {
                x: rect.x + rect.w * 0.5,
                y: rect.y + rect.h * 0.5,
            })
    }

    fn plan_enter_cluster_workspace(
        &self,
        core_id: NodeId,
        monitor: &str,
    ) -> Option<EnterClusterWorkspacePlan> {
        let cid = self.field.cluster_id_for_core_public(core_id)?;
        let cluster = self.field.cluster(cid)?;
        let members = cluster.members.clone();
        if members.is_empty() {
            return None;
        }
        let current_viewport = self.workspace_viewport_for_monitor(monitor)?;
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
        let mut hidden_ids = Vec::new();
        for id in ids {
            if members.contains(&id) || id == core_id {
                continue;
            }
            if self
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|node_monitor| node_monitor != monitor)
            {
                continue;
            }
            let already_detached = self
                .field
                .node(id)
                .is_some_and(|n| n.visibility.has(Visibility::DETACHED));
            if !already_detached {
                hidden_ids.push(id);
            }
        }
        Some(EnterClusterWorkspacePlan {
            cid,
            core_id,
            current_viewport,
            hidden_ids,
        })
    }

    fn plan_exit_cluster_workspace(&self, monitor: &str) -> Option<ExitClusterWorkspacePlan> {
        let cid = self.cluster_state.active_cluster_workspaces.get(monitor).copied()?;
        let hidden_ids = self
            .cluster_state
            .workspace_hidden_nodes
            .get(monitor)
            .cloned()
            .unwrap_or_default();
        let core_id = self.field.cluster(cid).and_then(|c| c.core);
        Some(ExitClusterWorkspacePlan {
            cid,
            core_id,
            hidden_ids,
        })
    }

    fn plan_active_cluster_layout(&self, monitor: &str) -> Option<ClusterLayoutPlan> {
        let cid = self.cluster_state.active_cluster_workspaces.get(monitor).copied()?;
        let cluster = self.field.cluster(cid)?;
        let members = cluster.members.clone();
        if members.is_empty() {
            return None;
        }
        let world_rect = self.opened_cluster_world_rect_for_monitor(monitor)?;
        let mut tile_members = members.iter().rev().copied().take(4).collect::<Vec<_>>();
        tile_members.reverse();
        let overflow_len = members.len().saturating_sub(tile_members.len());
        let overflow_members = members.iter().copied().take(overflow_len).collect::<Vec<_>>();
        let overflow_rect = if overflow_members.is_empty() {
            None
        } else {
            self.monitor_state.monitors.get(monitor).map(|space| halley_core::tiling::Rect {
                x: (space.width - Halley::CLUSTER_OVERFLOW_STRIP_W - Halley::CLUSTER_OVERFLOW_STRIP_PAD)
                    as f32,
                y: Halley::CLUSTER_OVERFLOW_STRIP_PAD as f32,
                w: Halley::CLUSTER_OVERFLOW_STRIP_W as f32,
                h: (space.height - Halley::CLUSTER_OVERFLOW_STRIP_PAD * 2).max(80) as f32,
            })
        };
        let tile_inset = (self.tuning.tile_gaps_inner_px * 0.5
            + crate::render::ACTIVE_WINDOW_FRAME_PAD_PX as f32)
            .clamp(4.0, 28.0);
        let tiles = self
            .opened_cluster_tile_rects(world_rect, tile_members.len())
            .into_iter()
            .map(|rect| rect.inset(tile_inset))
            .zip(tile_members)
            .map(|(rect, node_id)| ClusterTilePlacement { node_id, rect })
            .collect::<Vec<_>>();
        Some(ClusterLayoutPlan {
            overflow_members,
            overflow_rect,
            tiles,
        })
    }
}

impl<'a> ClusterMutationController<'a> {
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

    fn enter_cluster_mode(&mut self) -> bool {
        if self.cluster_state.cluster_mode_active {
            return true;
        }
        self.cluster_state.cluster_mode_active = true;
        self.cluster_state.cluster_mode_selected_nodes.clear();
        true
    }

    fn exit_cluster_mode(&mut self) -> bool {
        if !self.cluster_state.cluster_mode_active {
            return false;
        }
        self.cluster_state.cluster_mode_active = false;
        self.cluster_state.cluster_mode_selected_nodes.clear();
        true
    }

    fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        if !self.cluster_state.cluster_mode_active {
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
        if !self.cluster_state.cluster_mode_selected_nodes.insert(node_id) {
            self.cluster_state.cluster_mode_selected_nodes.remove(&node_id);
        }
        true
    }

    fn detach_member_from_cluster(
        &mut self,
        cid: ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now_ms: u64,
    ) -> bool {
        if !self.field.remove_member_from_cluster(cid, member_id) {
            return false;
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
        true
    }

    fn absorb_node_into_cluster(&mut self, cid: ClusterId, node_id: NodeId) -> bool {
        if !self.field.add_member_to_cluster(cid, node_id) {
            return false;
        }
        let _ = self
            .field
            .set_state(node_id, halley_core::field::NodeState::Node);
        if let Some(node) = self.field.node_mut(node_id) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
        }
        let _ = self.field.set_detached(node_id, false);
        true
    }

    fn build_cluster_cleanup_plan(&mut self) -> ClusterCleanupPlan {
        let cluster_ids = self.field.clusters_iter().map(|cluster| cluster.id).collect::<Vec<_>>();
        let mut singleton_clusters = Vec::new();
        let mut empty_clusters = Vec::new();
        let mut resync_clusters = Vec::new();

        for cid in cluster_ids {
            let Some(cluster) = self.field.cluster(cid).cloned() else {
                continue;
            };
            let live_members = cluster
                .members
                .iter()
                .copied()
                .filter(|member| self.field.node(*member).is_some())
                .collect::<Vec<_>>();
            if live_members.len() != cluster.members.len() {
                if let Some(cluster_mut) = self.field.cluster_mut(cid) {
                    cluster_mut.members = live_members.clone();
                }
            }
            match live_members.len() {
                0 => {
                    let active_monitors = self
                        .cluster_state
                        .active_cluster_workspaces
                        .iter()
                        .filter_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
                        .collect::<Vec<_>>();
                    empty_clusters.push((cid, cluster.core, active_monitors));
                }
                1 => singleton_clusters.push((cid, live_members[0])),
                _ => resync_clusters.push(cid),
            }
        }

        ClusterCleanupPlan {
            singleton_clusters,
            empty_clusters,
            resync_clusters,
        }
    }
}

impl Halley {
    const CLUSTER_OVERFLOW_HIDE_DELAY_MS: u64 = 10_000;
    pub(crate) const CLUSTER_OVERFLOW_REVEAL_EDGE_PX: f32 = 28.0;
    pub(crate) const CLUSTER_OVERFLOW_STRIP_W: i32 = 72;
    const CLUSTER_OVERFLOW_STRIP_PAD: i32 = 14;

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

    fn preferred_monitor_for_cluster(&self, cid: ClusterId, preferred: Option<&str>) -> Option<String> {
        self.cluster_read_controller()
            .preferred_monitor_for_cluster(cid, preferred)
    }

    fn sync_cluster_core_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        let Some(core_id) = self.model.field.cluster(cid).and_then(|cluster| cluster.core) else {
            return false;
        };
        let Some(target_monitor) = self.preferred_monitor_for_cluster(cid, preferred) else {
            return false;
        };
        self.assign_node_to_monitor(core_id, target_monitor.as_str());
        true
    }

    fn restore_cluster_workspace_monitor(&mut self, monitor: &str) {
        let Some(vp) = self.model.cluster_state.workspace_prev_viewports
            .remove(monitor)
        else {
            return;
        };
        self.model.cluster_state.cluster_overflow_rects.remove(monitor);
        self.model.cluster_state.cluster_overflow_members.remove(monitor);
        self.model.cluster_state.cluster_overflow_visible_until_ms
            .remove(monitor);
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

    fn dissolve_cluster_to_single_member(
        &mut self,
        cid: ClusterId,
        member_id: NodeId,
        now_ms: u64,
    ) {
        let active_monitors = self.model.cluster_state.active_cluster_workspaces
            .iter()
            .filter_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
            .collect::<Vec<_>>();
        for monitor in &active_monitors {
            for id in self.model.cluster_state.workspace_hidden_nodes
                .remove(monitor.as_str())
                .unwrap_or_default()
            {
                if self.model.field.node(id).is_some() {
                    let _ = self.model.field.set_detached(id, false);
                }
            }
            self.restore_cluster_workspace_monitor(monitor.as_str());
        }
        self.model.cluster_state.active_cluster_workspaces
            .retain(|_, active_cid| *active_cid != cid);
        self.model.cluster_state.cluster_bloom_open
            .retain(|_, open_cid| *open_cid != cid);
        self.model.cluster_state.cluster_overflow_members
            .retain(|_, members| !members.contains(&member_id));
        if self.input.interaction_state
            .cluster_join_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.cluster_id == cid)
        {
            self.input.interaction_state.cluster_join_candidate = None;
        }
        if let Some(core_id) = self.model.field.cluster(cid).and_then(|cluster| cluster.core) {
            self.model.monitor_state.node_monitor.remove(&core_id);
            let _ = self.model.field.remove(core_id);
        }
        let _ = self.model.field.remove_cluster(cid);

        let _ = self.model.field.set_detached(member_id, false);
        let _ = self.model.field
            .set_state(member_id, halley_core::field::NodeState::Active);
        if let Some(node) = self.model.field.node_mut(member_id) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            if let Some(size) = self.model.workspace_state.last_active_size.get(&member_id).copied() {
                node.intrinsic_size = size;
            }
        }
        if let Some(size) = self.model.workspace_state.last_active_size.get(&member_id).copied() {
            self.request_toplevel_resize(member_id, size.x.round() as i32, size.y.round() as i32);
        }
        let _ = self.model.field.touch(member_id, now_ms);
    }

    pub fn cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        self.cluster_read_controller().cluster_bloom_for_monitor(monitor)
    }

    pub fn toggle_cluster_bloom_by_core(&mut self, core_id: NodeId) -> bool {
        let monitor = self.model.monitor_state
            .node_monitor
            .get(&core_id)
            .cloned()
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
        let Some(cid) = self.model.field.cluster_id_for_core_public(core_id) else {
            return false;
        };
        if self.cluster_bloom_for_monitor(monitor.as_str()) == Some(cid) {
            return self.close_cluster_bloom_for_monitor(monitor.as_str());
        }
        self.open_cluster_bloom_for_monitor(monitor.as_str(), cid)
    }

    pub fn open_cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
    ) -> bool {
        let _ = self.sync_cluster_core_monitor(cid, Some(monitor));
        self.cluster_mutation_controller()
            .open_cluster_bloom_for_monitor(monitor, cid)
    }

    pub fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        self.cluster_mutation_controller()
            .close_cluster_bloom_for_monitor(monitor)
    }

    pub fn detach_member_from_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now: Instant,
    ) -> bool {
        let now_ms = self.now_ms(now);
        if !self
            .cluster_mutation_controller()
            .detach_member_from_cluster(cid, member_id, world_pos, now_ms)
        {
            return false;
        }
        self.cleanup_empty_clusters();
        let _ = self.model.field.sync_cluster_core_from_members(cid);
        let _ = self.sync_cluster_core_monitor(cid, None);
        if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None)
            && self.active_cluster_workspace_for_monitor(cluster_monitor.as_str()) == Some(cid)
        {
            self.layout_active_cluster_workspace_for_monitor(
                cluster_monitor.as_str(),
                now_ms,
            );
        }
        true
    }

    pub fn absorb_node_into_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        if !self
            .cluster_mutation_controller()
            .absorb_node_into_cluster(cid, node_id)
        {
            return false;
        }
        let _ = self.model.field.sync_cluster_core_from_members(cid);
        let _ = self.sync_cluster_core_monitor(cid, None);
        if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None) {
            self.assign_node_to_monitor(node_id, cluster_monitor.as_str());
            if self.active_cluster_workspace_for_monitor(cluster_monitor.as_str()) == Some(cid) {
                if let Some(node) = self.model.field.node_mut(node_id) {
                    node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                }
                self.layout_active_cluster_workspace_for_monitor(
                    cluster_monitor.as_str(),
                    self.now_ms(now),
                );
            }
        }
        if let Some(core_id) = self.model.field.cluster(cid).and_then(|cluster| cluster.core) {
            let _ = self.model.field.touch(core_id, self.now_ms(now));
        }
        true
    }

    pub(crate) fn commit_ready_cluster_join_for_node(
        &mut self,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        let Some(candidate) = self.input.interaction_state
            .cluster_join_candidate
            .clone()
            .filter(|candidate| candidate.node_id == node_id && candidate.ready)
        else {
            return false;
        };
        self.input.interaction_state.cluster_join_candidate = None;
        self.absorb_node_into_cluster(candidate.cluster_id, node_id, now)
    }

    pub fn cleanup_empty_clusters(&mut self) {
        let now_ms = self.now_ms(Instant::now());
        let plan = self.cluster_mutation_controller().build_cluster_cleanup_plan();
        for (cid, member_id) in plan.singleton_clusters {
            self.dissolve_cluster_to_single_member(cid, member_id, now_ms);
        }
        for (cid, core_id, active_monitors) in plan.empty_clusters {
            for monitor in &active_monitors {
                for id in self.model.cluster_state.workspace_hidden_nodes
                    .remove(monitor.as_str())
                    .unwrap_or_default()
                {
                    if self.model.field.node(id).is_some() {
                        let _ = self.model.field.set_detached(id, false);
                    }
                }
                self.restore_cluster_workspace_monitor(monitor.as_str());
            }
            self.model.cluster_state.active_cluster_workspaces
                .retain(|_, active_cid| *active_cid != cid);
            self.model.cluster_state.cluster_bloom_open
                .retain(|_, open_cid| *open_cid != cid);
            if self.input.interaction_state
                .cluster_join_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.cluster_id == cid)
            {
                self.input.interaction_state.cluster_join_candidate = None;
            }
            if let Some(core_id) = core_id {
                self.model.monitor_state.node_monitor.remove(&core_id);
                let _ = self.model.field.remove(core_id);
            }
            let _ = self.model.field.remove_cluster(cid);
        }
        for cid in plan.resync_clusters {
            let _ = self.model.field.sync_cluster_core_from_members(cid);
            let _ = self.sync_cluster_core_monitor(cid, None);
        }
    }

    pub fn active_cluster_workspace_for_monitor(&self, monitor: &str) -> Option<ClusterId> {
        self.model
            .cluster_state
            .active_cluster_workspaces
            .get(monitor)
            .copied()
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        if self.model.cluster_state.cluster_overflow_rects.contains_key(monitor) {
            self.model.cluster_state.cluster_overflow_visible_until_ms.insert(
                monitor.to_string(),
                now_ms.saturating_add(Self::CLUSTER_OVERFLOW_HIDE_DELAY_MS),
            );
        }
    }

    pub(crate) fn hide_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        if self.model.cluster_state.cluster_overflow_rects.contains_key(monitor) {
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .insert(monitor.to_string(), 0);
        }
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

    pub(crate) fn cluster_spawn_position_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<Vec2> {
        self.cluster_read_controller()
            .cluster_spawn_position_for_new_member(monitor, cid)
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
        !self.model.cluster_state.active_cluster_workspaces.is_empty()
    }

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.exit_cluster_workspace_for_monitor(monitor.as_str(), now)
    }

    pub fn cluster_mode_active(&self) -> bool {
        self.model.cluster_state.cluster_mode_active
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        if !self.cluster_mutation_controller().enter_cluster_mode() {
            return false;
        }
        self.set_persistent_mode_banner(
            "Cluster mode",
            Some("Select windows • Enter to create • Esc to cancel"),
        );
        true
    }

    pub fn exit_cluster_mode(&mut self) -> bool {
        if !self.cluster_mutation_controller().exit_cluster_mode() {
            return false;
        }
        self.clear_persistent_mode_banner();
        true
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        self.cluster_mutation_controller()
            .toggle_cluster_mode_selection(node_id)
    }

    pub fn confirm_cluster_mode(&mut self, now: Instant) -> bool {
        if !self.model.cluster_state.cluster_mode_active {
            return false;
        }
        if self.model.cluster_state.cluster_mode_selected_nodes.is_empty() {
            self.show_overlay_toast("No nodes selected; no cluster formed", 2200, now);
            return false;
        }

        let mut members = self.model.cluster_state.cluster_mode_selected_nodes
            .iter()
            .copied()
            .collect::<Vec<_>>();
        members.sort_by_key(|id| id.as_u64());
        if members.len() == 1 {
            self.show_overlay_toast("Clusters require at least two windows", 5000, now);
            return false;
        }
        let created = self.model.field.create_cluster(members).and_then(|cid| {
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
        if !c.members.contains(&member) {
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
        self.model.cluster_state.workspace_prev_viewports
            .insert(monitor.to_string(), plan.current_viewport);
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
        let _ = self.model.field.expand_cluster(plan.cid);
        if let Some(c) = self.model.field.cluster_mut(cid) {
            c.enter_active(ActiveLayoutMode::TiledWeighted);
        }

        self.model.cluster_state.workspace_hidden_nodes
            .insert(monitor.to_string(), plan.hidden_ids);
        self.model.cluster_state.active_cluster_workspaces
            .retain(|name, active_cid| *active_cid != cid || name == monitor);
        self.model.cluster_state.active_cluster_workspaces
            .insert(monitor.to_string(), cid);
        self.model.cluster_state.cluster_bloom_open.remove(monitor);
        self.set_interaction_focus(None, 0, now);
        self.layout_active_cluster_workspace_for_monitor(monitor, self.now_ms(now));
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

        if let Some(c) = self.model.field.cluster_mut(plan.cid) {
            c.set_collapsed(false);
        }
        let core = self.model.field.collapse_cluster(plan.cid).or(plan.core_id);
        if let Some(core_id) = core {
            let _ = self.model.field.set_detached(core_id, false);
            self.assign_node_to_monitor(core_id, monitor);
            let _ = self.model.field.touch(core_id, self.now_ms(now));
        }

        self.restore_cluster_workspace_monitor(monitor);
        self.model.cluster_state.active_cluster_workspaces
            .remove(monitor);
        self.model.cluster_state.cluster_overflow_members.remove(monitor);
        self.model.cluster_state.cluster_overflow_rects.remove(monitor);
        self.model.cluster_state.cluster_overflow_visible_until_ms
            .remove(monitor);
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
            self.model.cluster_state.active_cluster_workspaces
                .remove(monitor);
            return;
        };
        let members = cluster.members.clone();
        if members.is_empty() {
            return;
        }
        if self.model.fullscreen_state
            .fullscreen_active_node
            .get(monitor)
            .is_some_and(|fullscreen_id| members.contains(fullscreen_id))
        {
            return;
        }
        let Some(plan) = self.cluster_read_controller().plan_active_cluster_layout(monitor) else {
            return;
        };
        if plan.overflow_members.is_empty() {
            self.model.cluster_state.cluster_overflow_members.remove(monitor);
            self.model.cluster_state.cluster_overflow_rects.remove(monitor);
            self.model.cluster_state.cluster_overflow_visible_until_ms
                .remove(monitor);
        } else {
            self.model.cluster_state.cluster_overflow_members
                .insert(monitor.to_string(), plan.overflow_members.clone());
            self.model.cluster_state.cluster_overflow_visible_until_ms
                .entry(monitor.to_string())
                .or_insert(now_ms.saturating_add(Self::CLUSTER_OVERFLOW_HIDE_DELAY_MS));
            if let Some(rect) = plan.overflow_rect {
                self.model.cluster_state.cluster_overflow_rects
                    .insert(monitor.to_string(), rect);
            }
        }
        for placement in plan.tiles {
            let nid = placement.node_id;
            let rect = placement.rect;
            let _ = self.model.field.set_detached(nid, false);
            if let Some(node) = self.model.field.node_mut(nid) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                node.intrinsic_size.x = rect.w.max(64.0);
                node.intrinsic_size.y = rect.h.max(64.0);
            }
            let _ = self.model.field
                .set_state(nid, halley_core::field::NodeState::Active);
            let _ = self.model.field.carry(
                nid,
                Vec2 {
                    x: rect.x + rect.w * 0.5,
                    y: rect.y + rect.h * 0.5,
                },
            );
            self.set_last_active_size_now(
                nid,
                Vec2 {
                    x: rect.w.max(64.0),
                    y: rect.h.max(64.0),
                },
            );
            let _ = self.model.field.touch(nid, now_ms);
            self.request_toplevel_resize(nid, rect.w.round() as i32, rect.h.round() as i32);
        }

        for nid in plan.overflow_members {
            let _ = self.model.field.set_detached(nid, false);
            if let Some(node) = self.model.field.node_mut(nid) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
            }
            let _ = self.model.field
                .set_state(nid, halley_core::field::NodeState::Node);
        }
    }
}
