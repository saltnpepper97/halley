use super::*;
use halley_core::cluster::ClusterId;

impl Halley {
    const CLUSTER_OVERFLOW_HIDE_DELAY_MS: u64 = 10_000;
    pub(crate) const CLUSTER_OVERFLOW_REVEAL_EDGE_PX: f32 = 28.0;
    pub(crate) const CLUSTER_OVERFLOW_STRIP_W: i32 = 72;
    const CLUSTER_OVERFLOW_STRIP_PAD: i32 = 14;

    fn preferred_monitor_for_cluster(
        &self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> Option<String> {
        preferred
            .map(str::to_string)
            .or_else(|| {
                self.cluster_state.active_cluster_workspaces
                    .iter()
                    .find_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
            })
            .or_else(|| {
                self.cluster_state.cluster_bloom_open
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

    fn sync_cluster_core_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        let Some(core_id) = self.field.cluster(cid).and_then(|cluster| cluster.core) else {
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
            .cluster_state.workspace_prev_viewports
            .remove(monitor)
        else {
            return;
        };
        self.cluster_state.cluster_overflow_rects.remove(monitor);
        self.cluster_state.cluster_overflow_members.remove(monitor);
        self.cluster_state.cluster_overflow_visible_until_ms
            .remove(monitor);
        if self.monitor_state.current_monitor == monitor {
            self.viewport = vp;
            self.zoom_ref_size = self.viewport.size;
            self.snap_camera_targets_to_live();
            self.tuning.viewport_center = self.viewport.center;
            self.tuning.viewport_size = self.viewport.size;
        }
        if let Some(space) = self.monitor_state.monitors.get_mut(monitor) {
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
        let active_monitors = self
            .cluster_state.active_cluster_workspaces
            .iter()
            .filter_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
            .collect::<Vec<_>>();
        for monitor in &active_monitors {
            for id in self
                .cluster_state.workspace_hidden_nodes
                .remove(monitor.as_str())
                .unwrap_or_default()
            {
                if self.field.node(id).is_some() {
                    let _ = self.field.set_detached(id, false);
                }
            }
            self.restore_cluster_workspace_monitor(monitor.as_str());
        }
        self.cluster_state.active_cluster_workspaces
            .retain(|_, active_cid| *active_cid != cid);
        self.cluster_state.cluster_bloom_open
            .retain(|_, open_cid| *open_cid != cid);
        self.cluster_state.cluster_overflow_members
            .retain(|_, members| !members.contains(&member_id));
        if self
            .interaction_state
            .cluster_join_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.cluster_id == cid)
        {
            self.interaction_state.cluster_join_candidate = None;
        }
        if let Some(core_id) = self.field.cluster(cid).and_then(|cluster| cluster.core) {
            self.monitor_state.node_monitor.remove(&core_id);
            let _ = self.field.remove(core_id);
        }
        let _ = self.field.remove_cluster(cid);

        let _ = self.field.set_detached(member_id, false);
        let _ = self
            .field
            .set_state(member_id, halley_core::field::NodeState::Active);
        if let Some(node) = self.field.node_mut(member_id) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            if let Some(size) = self.workspace_state.last_active_size.get(&member_id).copied() {
                node.intrinsic_size = size;
            }
        }
        if let Some(size) = self.workspace_state.last_active_size.get(&member_id).copied() {
            self.request_toplevel_resize(member_id, size.x.round() as i32, size.y.round() as i32);
        }
        let _ = self.field.touch(member_id, now_ms);
    }

    pub fn cluster_bloom_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        self.cluster_state.cluster_bloom_open
            .get(monitor)
            .copied()
    }

    pub fn toggle_cluster_bloom_by_core(&mut self, core_id: NodeId) -> bool {
        let monitor = self
            .monitor_state
            .node_monitor
            .get(&core_id)
            .cloned()
            .unwrap_or_else(|| self.monitor_state.current_monitor.clone());
        let Some(cid) = self.field.cluster_id_for_core_public(core_id) else {
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
        let Some(cluster) = self.field.cluster(cid) else {
            return false;
        };
        let Some(core_id) = cluster.core else {
            return false;
        };
        let _ = self.sync_cluster_core_monitor(cid, Some(monitor));
        self.cluster_state.cluster_bloom_open
            .retain(|name, open_cid| *open_cid != cid || name == monitor);
        let _ = self.close_cluster_bloom_for_monitor(monitor);
        let _ = self.field.set_pinned(core_id, true);
        self.interaction_state.physics_velocity.remove(&core_id);
        self.cluster_state.cluster_bloom_open
            .insert(monitor.to_string(), cid);
        true
    }

    pub fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(cid) = self.cluster_state.cluster_bloom_open.remove(monitor) else {
            return false;
        };
        if let Some(core_id) = self.field.cluster(cid).and_then(|cluster| cluster.core) {
            let _ = self.field.set_pinned(core_id, false);
        }
        true
    }

    pub fn detach_member_from_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now: Instant,
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
        let _ = self.field.touch(member_id, self.now_ms(now));
        self.cleanup_empty_clusters();
        let _ = self.field.sync_cluster_core_from_members(cid);
        let _ = self.sync_cluster_core_monitor(cid, None);
        if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None)
            && self.active_cluster_workspace_for_monitor(cluster_monitor.as_str()) == Some(cid)
        {
            self.layout_active_cluster_workspace_for_monitor(
                cluster_monitor.as_str(),
                self.now_ms(now),
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
        let _ = self.field.sync_cluster_core_from_members(cid);
        let _ = self.sync_cluster_core_monitor(cid, None);
        if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None) {
            self.assign_node_to_monitor(node_id, cluster_monitor.as_str());
            if self.active_cluster_workspace_for_monitor(cluster_monitor.as_str()) == Some(cid) {
                if let Some(node) = self.field.node_mut(node_id) {
                    node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                }
                self.layout_active_cluster_workspace_for_monitor(
                    cluster_monitor.as_str(),
                    self.now_ms(now),
                );
            }
        }
        if let Some(core_id) = self.field.cluster(cid).and_then(|cluster| cluster.core) {
            let _ = self.field.touch(core_id, self.now_ms(now));
        }
        true
    }

    pub(crate) fn commit_ready_cluster_join_for_node(
        &mut self,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        let Some(candidate) = self
            .interaction_state
            .cluster_join_candidate
            .clone()
            .filter(|candidate| candidate.node_id == node_id && candidate.ready)
        else {
            return false;
        };
        self.interaction_state.cluster_join_candidate = None;
        self.absorb_node_into_cluster(candidate.cluster_id, node_id, now)
    }

    pub fn cleanup_empty_clusters(&mut self) {
        let now_ms = self.now_ms(Instant::now());
        let cluster_ids = self
            .field
            .clusters_iter()
            .map(|cluster| cluster.id)
            .collect::<Vec<_>>();
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
            if live_members.len() != cluster.members.len()
                && let Some(cluster_mut) = self.field.cluster_mut(cid)
            {
                cluster_mut.members = live_members.clone();
            }
            if live_members.len() == 1 {
                self.dissolve_cluster_to_single_member(cid, live_members[0], now_ms);
                continue;
            }
            if live_members.is_empty() {
                let active_monitors = self
                    .cluster_state.active_cluster_workspaces
                    .iter()
                    .filter_map(|(monitor, active_cid)| {
                        (*active_cid == cid).then(|| monitor.clone())
                    })
                    .collect::<Vec<_>>();
                for monitor in &active_monitors {
                    for id in self
                        .cluster_state.workspace_hidden_nodes
                        .remove(monitor.as_str())
                        .unwrap_or_default()
                    {
                        if self.field.node(id).is_some() {
                            let _ = self.field.set_detached(id, false);
                        }
                    }
                    self.restore_cluster_workspace_monitor(monitor.as_str());
                }
                self.cluster_state.active_cluster_workspaces
                    .retain(|_, active_cid| *active_cid != cid);
                self.cluster_state.cluster_bloom_open
                    .retain(|_, open_cid| *open_cid != cid);
                if self
                    .interaction_state
                    .cluster_join_candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.cluster_id == cid)
                {
                    self.interaction_state.cluster_join_candidate = None;
                }
                if let Some(core_id) = cluster.core {
                    self.monitor_state.node_monitor.remove(&core_id);
                    let _ = self.field.remove(core_id);
                }
                let _ = self.field.remove_cluster(cid);
                continue;
            }
            let _ = self.field.sync_cluster_core_from_members(cid);
            let _ = self.sync_cluster_core_monitor(cid, None);
        }
    }

    pub fn active_cluster_workspace_for_monitor(&self, monitor: &str) -> Option<ClusterId> {
        self.cluster_state.active_cluster_workspaces
            .get(monitor)
            .copied()
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        if self.cluster_state.cluster_overflow_rects.contains_key(monitor) {
            self.cluster_state.cluster_overflow_visible_until_ms.insert(
                monitor.to_string(),
                now_ms.saturating_add(Self::CLUSTER_OVERFLOW_HIDE_DELAY_MS),
            );
        }
    }

    pub(crate) fn hide_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        if self.cluster_state.cluster_overflow_rects.contains_key(monitor) {
            self.cluster_state.cluster_overflow_visible_until_ms
                .insert(monitor.to_string(), 0);
        }
    }

    pub(crate) fn cluster_overflow_visible_for_monitor(&self, monitor: &str, now_ms: u64) -> bool {
        self.cluster_state.cluster_overflow_visible_until_ms
            .get(monitor)
            .copied()
            .unwrap_or(0)
            > now_ms
    }

    pub(crate) fn cluster_overflow_rect_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        self.cluster_state.cluster_overflow_rects.get(monitor).copied()
    }

    pub(crate) fn cluster_overflow_member_ids_for_monitor(&self, monitor: &str) -> Vec<NodeId> {
        self.cluster_state.cluster_overflow_members
            .get(monitor)
            .cloned()
            .unwrap_or_default()
    }

    fn workspace_viewport_for_monitor(&self, monitor: &str) -> Option<halley_core::viewport::Viewport> {
        self.monitor_state.monitors.get(monitor).map(|space| space.viewport)
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

    pub(crate) fn cluster_spawn_position_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<Vec2> {
        self.cluster_spawn_rect_for_new_member(monitor, cid)
            .map(|rect| Vec2 {
                x: rect.x + rect.w * 0.5,
                y: rect.y + rect.h * 0.5,
            })
    }

    pub(crate) fn cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
        cid: ClusterId,
    ) -> Option<halley_core::tiling::Rect> {
        let cluster = self.field.cluster(cid)?;
        let count = (cluster.members.len() + 1).min(4);
        let tile_rect = self.opened_cluster_world_rect_for_monitor(monitor)?;
        let tile_inset =
            (self.tuning.tile_gaps_inner_px * 0.5 + crate::render::ACTIVE_WINDOW_FRAME_PAD_PX as f32)
                .clamp(4.0, 28.0);
        self.opened_cluster_tile_rects(tile_rect, count)
            .into_iter()
            .last()
            .map(|rect| rect.inset(tile_inset))
    }

    pub fn has_any_active_cluster_workspace(&self) -> bool {
        !self.cluster_state.active_cluster_workspaces.is_empty()
    }

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        let monitor = self.monitor_state.current_monitor.clone();
        self.exit_cluster_workspace_for_monitor(monitor.as_str(), now)
    }

    pub fn cluster_mode_active(&self) -> bool {
        self.cluster_state.cluster_mode_active
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        if self.cluster_state.cluster_mode_active {
            return true;
        }
        self.cluster_state.cluster_mode_active = true;
        self.cluster_state.cluster_mode_selected_nodes.clear();
        self.set_persistent_mode_banner(
            "Cluster mode",
            Some("Select windows • Enter to create • Esc to cancel"),
        );
        true
    }

    pub fn exit_cluster_mode(&mut self) -> bool {
        if !self.cluster_state.cluster_mode_active {
            return false;
        }
        self.cluster_state.cluster_mode_active = false;
        self.cluster_state.cluster_mode_selected_nodes.clear();
        self.clear_persistent_mode_banner();
        true
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
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
        if !self
            .cluster_state.cluster_mode_selected_nodes
            .insert(node_id)
        {
            self.cluster_state.cluster_mode_selected_nodes
                .remove(&node_id);
        }
        true
    }

    pub fn confirm_cluster_mode(&mut self, now: Instant) -> bool {
        if !self.cluster_state.cluster_mode_active {
            return false;
        }
        if self.cluster_state.cluster_mode_selected_nodes.is_empty() {
            self.show_overlay_toast("No nodes selected; no cluster formed", 2200, now);
            return false;
        }

        let mut members = self
            .cluster_state.cluster_mode_selected_nodes
            .iter()
            .copied()
            .collect::<Vec<_>>();
        members.sort_by_key(|id| id.as_u64());
        if members.len() == 1 {
            self.show_overlay_toast("Clusters require at least two windows", 5000, now);
            return false;
        }
        let created = self.field.create_cluster(members).and_then(|cid| {
            let core = self.field.collapse_cluster(cid);
            if let Some(core_id) = core {
                self.assign_node_to_current_monitor(core_id);
                let _ = self.field.touch(core_id, self.now_ms(now));
                self.set_interaction_focus(Some(core_id), 30_000, now);
            }
            core
        });
        let _ = self.exit_cluster_mode();
        created.is_some()
    }

    pub fn toggle_cluster_workspace_by_core(&mut self, core_id: NodeId, now: Instant) -> bool {
        let monitor = self.monitor_state.current_monitor.clone();
        if let Some(cid) = self.active_cluster_workspace_for_monitor(monitor.as_str())
            && self.field.cluster_id_for_core_public(core_id) == Some(cid)
        {
            return self.exit_cluster_workspace_for_monitor(monitor.as_str(), now);
        }
        self.enter_cluster_workspace_by_core(core_id, monitor.as_str(), now)
    }

    pub fn has_active_cluster_workspace(&self) -> bool {
        self.active_cluster_workspace_for_monitor(self.monitor_state.current_monitor.as_str())
            .is_some()
    }

    pub fn exit_cluster_workspace_if_member(&mut self, member: NodeId, now: Instant) -> bool {
        let monitor = self.monitor_state.current_monitor.clone();
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor.as_str()) else {
            return false;
        };
        let Some(c) = self.field.cluster(cid) else {
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
        let Some(cid) = self.field.cluster_id_for_core_public(core_id) else {
            return false;
        };
        if self.active_cluster_workspace_for_monitor(monitor) == Some(cid) {
            return true;
        }
        if self.active_cluster_workspace_for_monitor(monitor).is_some() {
            let _ = self.exit_cluster_workspace_for_monitor(monitor, now);
        }

        let Some(cluster) = self.field.cluster(cid) else {
            return false;
        };
        let members = cluster.members.clone();
        if members.is_empty() {
            return false;
        }
        let _ = self.sync_cluster_core_monitor(cid, Some(monitor));
        let Some(current_viewport) = self.workspace_viewport_for_monitor(monitor) else {
            return false;
        };
        self.cluster_state.workspace_prev_viewports
            .insert(monitor.to_string(), current_viewport);
        if self.monitor_state.current_monitor == monitor {
            self.interaction_state.viewport_pan_anim = None;
            self.viewport = current_viewport;
            self.zoom_ref_size = current_viewport.size;
            self.camera_target_center = current_viewport.center;
            self.camera_target_view_size = current_viewport.size;
            self.tuning.viewport_center = current_viewport.center;
            self.tuning.viewport_size = current_viewport.size;
        }

        let mut hidden = Vec::new();
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
        for id in ids {
            let is_member = members.contains(&id);
            if is_member || id == core_id {
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
                let _ = self.field.set_detached(id, true);
                hidden.push(id);
            }
        }
        let _ = self.field.set_detached(core_id, true);
        let _ = self.field.expand_cluster(cid);
        if let Some(c) = self.field.cluster_mut(cid) {
            c.enter_active(ActiveLayoutMode::TiledWeighted);
        }

        self.cluster_state.workspace_hidden_nodes
            .insert(monitor.to_string(), hidden);
        self.cluster_state.active_cluster_workspaces
            .retain(|name, active_cid| *active_cid != cid || name == monitor);
        self.cluster_state.active_cluster_workspaces
            .insert(monitor.to_string(), cid);
        self.cluster_state.cluster_bloom_open.remove(monitor);
        self.set_interaction_focus(None, 0, now);
        self.layout_active_cluster_workspace_for_monitor(monitor, self.now_ms(now));
        true
    }

    fn exit_cluster_workspace_for_monitor(&mut self, monitor: &str, now: Instant) -> bool {
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return false;
        };

        for id in self
            .cluster_state.workspace_hidden_nodes
            .remove(monitor)
            .unwrap_or_default()
        {
            let _ = self.field.set_detached(id, false);
        }

        let core_before = self.field.cluster(cid).and_then(|c| c.core);
        if let Some(c) = self.field.cluster_mut(cid) {
            c.set_collapsed(false);
        }
        let core = self.field.collapse_cluster(cid).or(core_before);
        if let Some(core_id) = core {
            let _ = self.field.set_detached(core_id, false);
            self.assign_node_to_monitor(core_id, monitor);
            let _ = self.field.touch(core_id, self.now_ms(now));
        }

        self.restore_cluster_workspace_monitor(monitor);
        self.cluster_state.active_cluster_workspaces
            .remove(monitor);
        self.cluster_state.cluster_overflow_members.remove(monitor);
        self.cluster_state.cluster_overflow_rects.remove(monitor);
        self.cluster_state.cluster_overflow_visible_until_ms
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
        let Some(cluster) = self.field.cluster(cid) else {
            self.cluster_state.active_cluster_workspaces
                .remove(monitor);
            return;
        };
        let members = cluster.members.clone();
        if members.is_empty() {
            return;
        }
        if self
            .fullscreen_state
            .fullscreen_active_node
            .get(monitor)
            .is_some_and(|fullscreen_id| members.contains(fullscreen_id))
        {
            return;
        }
        let Some(world_rect) = self.opened_cluster_world_rect_for_monitor(monitor) else {
            return;
        };
        let tile_inner_gap = self.tuning.tile_gaps_inner_px;

        let mut tile_members = members.iter().rev().copied().take(4).collect::<Vec<_>>();
        tile_members.reverse();
        let overflow_len = members.len().saturating_sub(tile_members.len());
        let overflow_members = members
            .iter()
            .copied()
            .take(overflow_len)
            .collect::<Vec<_>>();
        if overflow_members.is_empty() {
            self.cluster_state.cluster_overflow_members.remove(monitor);
            self.cluster_state.cluster_overflow_rects.remove(monitor);
            self.cluster_state.cluster_overflow_visible_until_ms
                .remove(monitor);
        } else {
            self.cluster_state.cluster_overflow_members
                .insert(monitor.to_string(), overflow_members.clone());
            self.cluster_state.cluster_overflow_visible_until_ms
                .entry(monitor.to_string())
                .or_insert(now_ms.saturating_add(Self::CLUSTER_OVERFLOW_HIDE_DELAY_MS));
            if let Some(space) = self.monitor_state.monitors.get(monitor) {
                let rect = halley_core::tiling::Rect {
                    x: (space.width - Self::CLUSTER_OVERFLOW_STRIP_W - Self::CLUSTER_OVERFLOW_STRIP_PAD)
                        as f32,
                    y: Self::CLUSTER_OVERFLOW_STRIP_PAD as f32,
                    w: Self::CLUSTER_OVERFLOW_STRIP_W as f32,
                    h: (space.height - Self::CLUSTER_OVERFLOW_STRIP_PAD * 2).max(80) as f32,
                };
                self.cluster_state.cluster_overflow_rects
                    .insert(monitor.to_string(), rect);
            }
        }
        let tile_rect = world_rect;

        let tile_layouts = self.opened_cluster_tile_rects(tile_rect, tile_members.len());

        let tile_inset = (tile_inner_gap * 0.5 + crate::render::ACTIVE_WINDOW_FRAME_PAD_PX as f32)
            .clamp(4.0, 28.0);
        for (nid, rect) in tile_members
            .into_iter()
            .zip(tile_layouts.into_iter().map(|rect| rect.inset(tile_inset)))
        {
            let _ = self.field.set_detached(nid, false);
            if let Some(node) = self.field.node_mut(nid) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                node.intrinsic_size.x = rect.w.max(64.0);
                node.intrinsic_size.y = rect.h.max(64.0);
            }
            let _ = self
                .field
                .set_state(nid, halley_core::field::NodeState::Active);
            let _ = self.field.carry(
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
            let _ = self.field.touch(nid, now_ms);
            self.request_toplevel_resize(nid, rect.w.round() as i32, rect.h.round() as i32);
        }

        for nid in overflow_members {
            let _ = self.field.set_detached(nid, false);
            if let Some(node) = self.field.node_mut(nid) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
            }
            let _ = self
                .field
                .set_state(nid, halley_core::field::NodeState::Node);
        }
    }
}
