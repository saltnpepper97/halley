use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::*;

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
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

    pub(super) fn clear_cluster_shell_state(&mut self, cid: ClusterId) {
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
            self.clear_cluster_overflow_for_monitor(monitor.as_str());
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

    pub fn collapse_active_cluster_workspace(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.exit_cluster_workspace_for_monitor(monitor.as_str(), now)
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
            .workspace_hidden_nodes
            .remove(monitor);
        self.clear_cluster_overflow_for_monitor(monitor);
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
        if let Some(core_id) = core {
            self.set_recent_top_node(core_id, now + std::time::Duration::from_millis(1200));
            self.set_interaction_focus(Some(core_id), 30_000, now);
        }
        true
    }

    fn clear_cluster_tile_animation_for_node(&mut self, node_id: NodeId) {
        self.ui.render_state.cluster_tile_tracks.remove(&node_id);
        self.ui
            .render_state
            .cluster_tile_entry_pending
            .remove(&node_id);
        self.ui
            .render_state
            .cluster_tile_frozen_geometry
            .remove(&node_id);
    }

    fn update_tiled_cluster_animation_targets(
        &mut self,
        plan: &ClusterLayoutPlan,
        dragged_member: Option<NodeId>,
        now: Instant,
    ) {
        for placement in &plan.tiles {
            if self
                .model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .contains_key(&placement.node_id)
                || Some(placement.node_id) == dragged_member
            {
                self.clear_cluster_tile_animation_for_node(placement.node_id);
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
                crate::animation::cluster_tile_rect_from_field(&self.model.field, placement.node_id)
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
            let duration_ms = self.runtime.tuning.tile_animation_duration_ms();
            if self.runtime.tuning.tile_animation_enabled() {
                crate::animation::set_cluster_tile_target(
                    &mut self.ui.render_state.cluster_tile_tracks,
                    current_rect,
                    placement.node_id,
                    placement.rect,
                    now,
                    duration_ms,
                );
            } else {
                self.ui
                    .render_state
                    .cluster_tile_tracks
                    .remove(&placement.node_id);
            }
        }
    }

    fn current_surface_size_map_for_members(
        &self,
        members: &HashSet<NodeId>,
    ) -> HashMap<NodeId, Vec2> {
        let mut sizes = HashMap::with_capacity(members.len());
        for (&node_id, &(_, _, w, h)) in &self.ui.render_state.window_geometry {
            if members.contains(&node_id) {
                sizes.insert(
                    node_id,
                    Vec2 {
                        x: w.max(1.0),
                        y: h.max(1.0),
                    },
                );
            }
        }

        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();
            let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
                continue;
            };
            if sizes.contains_key(&node_id) || !members.contains(&node_id) {
                continue;
            }

            let size = with_states(wl, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
            })
            .map(|g| Vec2 {
                x: g.size.w.max(1) as f32,
                y: g.size.h.max(1) as f32,
            })
            .or_else(|| {
                top.current_state().size.map(|sz| Vec2 {
                    x: sz.w.max(1) as f32,
                    y: sz.h.max(1) as f32,
                })
            })
            .unwrap_or_else(|| {
                let bbox = bbox_from_surface_tree(wl, (0, 0));
                Vec2 {
                    x: bbox.size.w.max(1) as f32,
                    y: bbox.size.h.max(1) as f32,
                }
            });
            sizes.insert(node_id, size);
        }

        for &node_id in members {
            sizes.entry(node_id).or_insert_with(|| {
                self.model
                    .field
                    .node(node_id)
                    .map_or(Vec2 { x: 1.0, y: 1.0 }, |node| Vec2 {
                        x: node.intrinsic_size.x.max(1.0),
                        y: node.intrinsic_size.y.max(1.0),
                    })
            });
        }

        sizes
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
        let member_set = members.iter().copied().collect::<HashSet<_>>();
        let dragged_member = self
            .input
            .interaction_state
            .drag_authority_node
            .filter(|id| member_set.contains(id));
        if self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(monitor)
            .is_some_and(|fullscreen_id| member_set.contains(fullscreen_id))
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
            self.update_tiled_cluster_animation_targets(&plan, dragged_member, now);
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
            .collect::<HashSet<_>>();
        let visible_surface_sizes = self.current_surface_size_map_for_members(&visible_members);
        if let Some(cluster) = self.model.field.cluster_mut(cid) {
            for member_id in &members {
                if let Some(node) = cluster.workspace_member_mut(*member_id) {
                    let visible = visible_members.contains(member_id);
                    node.visibility.set(Visibility::DETACHED, !visible);
                    node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, !visible);
                }
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
                && layout_changed
            {
                node.visibility.set(Visibility::DETACHED, false);
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                node.intrinsic_size = target_size;
                node.state = halley_core::field::NodeState::Active;
                node.footprint = node.resize_footprint.unwrap_or(node.intrinsic_size);
                node.pos = target_pos;
            }
            if layout_changed {
                self.set_last_active_size_now(nid, target_size);
            }
            let surface_size_changed = visible_surface_sizes.get(&nid).is_none_or(|size| {
                (size.x - target_size.x).abs() > 0.5 || (size.y - target_size.y).abs() > 0.5
            });
            if surface_size_changed {
                self.request_toplevel_resize(nid, rect.w.round() as i32, rect.h.round() as i32);
            }
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, false);
    }
}
