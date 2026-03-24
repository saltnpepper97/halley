use super::*;
use crate::state::ViewportPanAnim;
use eventline::info;
use halley_core::viewport::FocusZone;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

impl Halley {
    pub(crate) const VIEWPORT_PAN_PRELOAD_MS: u64 = 70;
    pub(crate) const VIEWPORT_PAN_DURATION_MS: u64 = 260;
    const SPAWN_VIEW_HANDOFF_PAN_RATIO: f32 = 0.35;
    const SPAWN_VIEW_HANDOFF_FOCUS_RATIO: f32 = 0.25;

    pub fn wl_surface_for_node(&self, id: NodeId) -> Option<WlSurface> {
        for top in self.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface().clone();
            if self.surface_to_node.get(&wl.id()).copied() == Some(id) {
                return Some(wl);
            }
        }
        None
    }

    pub(crate) fn update_selection_focus_from_surface(&self, surface: Option<&WlSurface>) {
        let client = surface.and_then(|wl| wl.client());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
    }

    fn fullscreen_focus_override(&self, requested: Option<NodeId>) -> Option<NodeId> {
        let fullscreen_id = self.fullscreen_active_node.values().next().copied()?;

        if requested == Some(fullscreen_id) {
            return requested;
        }

        let fullscreen_monitor = self
            .fullscreen_monitor_for_node(fullscreen_id)
            .or_else(|| {
                self.monitor_state
                    .node_monitor
                    .get(&fullscreen_id)
                    .map(|m| m.as_str())
            })?;

        match requested {
            None => Some(fullscreen_id),
            Some(requested_id) => {
                let requested_monitor = self
                    .monitor_state
                    .node_monitor
                    .get(&requested_id)
                    .map(|m| m.as_str());
                if requested_monitor != Some(fullscreen_monitor) {
                    Some(fullscreen_id)
                } else {
                    requested
                }
            }
        }
    }

    pub fn apply_wayland_focus_state(&mut self, id: Option<NodeId>) {
        let focus_id = self.fullscreen_focus_override(id).or(id);
        if let Some(fid) = focus_id
            && self
                .fullscreen_suspended_node
                .values()
                .any(|&nid| nid == fid)
        {
            if let Some(entry) = self.fullscreen_restore.get(&fid).copied() {
                let target_monitor = self
                    .monitor_state
                    .node_monitor
                    .get(&fid)
                    .cloned()
                    .unwrap_or_else(|| self.monitor_state.current_monitor.clone());
                if let Some(space) = self.monitor_state.monitors.get_mut(&target_monitor) {
                    space.viewport.center = entry.viewport_center;
                    space.camera_target_center = entry.viewport_center;
                }
                if self.monitor_state.current_monitor == target_monitor {
                    self.viewport.center = entry.viewport_center;
                    self.camera_target_center = self.viewport.center;
                    self.tuning.viewport_center = self.viewport.center;
                    self.interaction_state.viewport_pan_anim = None;
                }
            }
            self.enter_xdg_fullscreen(fid, None, Instant::now());
        }
        self.monitor_state.layer_keyboard_focus = None;
        let requested_focus_surface = focus_id.and_then(|fid| self.wl_surface_for_node(fid));
        let active_locked_surface = self.active_locked_pointer_surface();
        let locked_surface_node = active_locked_surface
            .as_ref()
            .and_then(|surface| self.surface_to_node.get(&surface.id()).copied());
        let keep_locked_focus =
            locked_surface_node.is_some_and(|nid| self.is_fullscreen_active(nid));
        let focus_surface = if keep_locked_focus {
            active_locked_surface
                .clone()
                .or(requested_focus_surface.clone())
        } else {
            requested_focus_surface.clone()
        };
        if !keep_locked_focus
            && active_locked_surface.as_ref().is_some_and(|surface| {
                Some(surface.id()) != focus_surface.as_ref().map(|wl| wl.id())
            })
        {
            self.release_active_pointer_constraint();
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, focus_surface.clone(), SERIAL_COUNTER.next_serial());
        }
        self.update_selection_focus_from_surface(focus_surface.as_ref());

        for top in self.xdg_shell_state.toplevel_surfaces() {
            let key = top.wl_surface().id();
            let node_id = self.surface_to_node.get(&key).copied();

            let activated = node_id.is_some_and(|nid| {
                self.monitor_state
                    .node_monitor
                    .get(&nid)
                    .and_then(|monitor| self.focus_state.monitor_focus.get(monitor))
                    .copied()
                    == Some(nid)
                    || Some(nid) == focus_id
            });

            let state_changed = top.with_pending_state(|s| {
                let was_active = s.states.contains(xdg_toplevel::State::Activated);
                if activated {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
                was_active != activated
            });

            if state_changed {
                top.send_configure();
            }
        }
    }

    pub fn update_focus_tracking_for_surface(&mut self, fid: NodeId, now_ms: u64) {
        let Some(node_state) = self
            .field
            .node(fid)
            .map(|n| (n.kind.clone(), n.state.clone()))
        else {
            return;
        };
        if node_state.0 != halley_core::field::NodeKind::Surface || !self.field.is_visible(fid) {
            return;
        }

        self.focus_state.last_surface_focus_ms.insert(fid, now_ms);
        if self.focus_state.suppress_trail_record_once {
            self.focus_state.suppress_trail_record_once = false;
        } else {
            self.record_focus_trail_visit(fid);
        }

        if node_state.1 == halley_core::field::NodeState::Active {
            let _ = self.field.touch(fid, now_ms);
            let _ = self.field.set_decay_level(fid, DecayLevel::Hot);
            if self.tuning.restore_last_active_on_pan_return {
                self.focus_state.pan_restore_active_focus = Some(fid);
            }
        }
    }

    pub fn note_pan_activity(&mut self, now: Instant) {
        self.interaction_state.viewport_pan_anim = None;
        let now_ms = self.now_ms(now);
        self.interaction_state.pan_dominant_until_ms = now_ms.saturating_add(220);
        self.spawn_last_pan_ms = now_ms;
        self.spawn_pan_start_center
            .get_or_insert(self.viewport.center);
        if self.tuning.restore_last_active_on_pan_return
            && self.focus_state.pan_restore_active_focus.is_none()
        {
            self.focus_state.pan_restore_active_focus = self.last_focused_active_surface_node();
        }
        self.interaction_state.suspend_overlap_resolve = false;
        self.interaction_state.suspend_state_checks = false;
        self.request_maintenance();
    }

    fn spawn_view_handoff_pan_distance(&self) -> f32 {
        self.viewport.size.x.min(self.viewport.size.y) * Self::SPAWN_VIEW_HANDOFF_PAN_RATIO
    }

    fn spawn_view_handoff_focus_distance(&self) -> f32 {
        self.viewport.size.x.hypot(self.viewport.size.y) * Self::SPAWN_VIEW_HANDOFF_FOCUS_RATIO
    }

    pub(crate) fn note_pan_viewport_change(&mut self, _now: Instant) {
        if self.spawn_anchor_mode == crate::state::SpawnAnchorMode::View {
            self.spawn_view_anchor = self.viewport.center;
        }
        self.request_maintenance();
        let Some(start_center) = self.spawn_pan_start_center else {
            return;
        };

        let moved = ((self.viewport.center.x - start_center.x).powi(2)
            + (self.viewport.center.y - start_center.y).powi(2))
        .sqrt();
        if moved < self.spawn_view_handoff_pan_distance() {
            return;
        }

        let focus_far = self
            .last_input_surface_node()
            .and_then(|id| self.field.node(id))
            .map(|node| {
                let dx = self.viewport.center.x - node.pos.x;
                let dy = self.viewport.center.y - node.pos.y;
                dx.hypot(dy) >= self.spawn_view_handoff_focus_distance()
            })
            .unwrap_or(true);
        if !focus_far {
            return;
        }

        self.spawn_anchor_mode = crate::state::SpawnAnchorMode::View;
        self.spawn_view_anchor = self.viewport.center;
        self.spawn_patch = None;
        self.focus_state.pan_restore_active_focus = None;
        self.spawn_pan_start_center = Some(self.viewport.center);
    }

    pub fn set_pan_restore_focus_target(&mut self, id: NodeId) {
        self.focus_state.pan_restore_active_focus = Some(id);
    }

    pub fn animate_viewport_center_to(&mut self, target_center: Vec2, now: Instant) -> bool {
        self.animate_viewport_center_to_delayed(target_center, now, 0)
    }

    pub fn animate_viewport_center_to_delayed(
        &mut self,
        target_center: Vec2,
        now: Instant,
        delay_ms: u64,
    ) -> bool {
        let from = self.viewport.center;
        let dx = target_center.x - from.x;
        let dy = target_center.y - from.y;
        if dx.abs() < 0.25 && dy.abs() < 0.25 {
            return false;
        }
        self.interaction_state.viewport_pan_anim = Some(ViewportPanAnim {
            start_ms: self.now_ms(now),
            delay_ms,
            duration_ms: Self::VIEWPORT_PAN_DURATION_MS,
            from_center: from,
            to_center: target_center,
        });
        true
    }

    pub(crate) fn tick_viewport_pan_animation(&mut self, now_ms: u64) {
        let Some(anim) = &self.interaction_state.viewport_pan_anim else {
            return;
        };
        if now_ms <= anim.start_ms.saturating_add(anim.delay_ms) {
            self.viewport.center = anim.from_center;
            self.camera_target_center = self.viewport.center;
            self.tuning.viewport_center = self.viewport.center;
            return;
        }
        let dur = anim.duration_ms.max(1);
        let elapsed_ms = now_ms.saturating_sub(anim.start_ms.saturating_add(anim.delay_ms));
        let t = (elapsed_ms as f32 / dur as f32).clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        self.viewport.center = Vec2 {
            x: anim.from_center.x + (anim.to_center.x - anim.from_center.x) * e,
            y: anim.from_center.y + (anim.to_center.y - anim.from_center.y) * e,
        };
        self.camera_target_center = self.viewport.center;
        self.tuning.viewport_center = self.viewport.center;
        if t >= 1.0 {
            self.interaction_state.viewport_pan_anim = None;
        }
    }

    fn last_focused_active_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus
            && self.field.node(id).is_some_and(|n| {
                self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Active
            })
        {
            return Some(id);
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && n.state == halley_core::field::NodeState::Active)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub fn begin_resize_interaction(&mut self, id: NodeId, now: Instant) {
        self.interaction_state.resize_active = Some(id);
        self.interaction_state.resize_static_node = Some(id);
        self.interaction_state.resize_static_lock_pos = None;
        self.interaction_state.resize_static_until_ms = self.now_ms(now).saturating_add(60_000);
        self.interaction_state.suspend_overlap_resolve = true;
        self.interaction_state.suspend_state_checks = true;
        self.set_interaction_focus(Some(id), 60_000, now);
        let now_ms = self.now_ms(now);
        let _ = self.field.touch(id, now_ms);
        let _ = self.field.set_decay_level(id, DecayLevel::Hot);
        self.workspace_state.manual_collapsed_nodes.remove(&id);
        self.request_maintenance();
    }

    pub fn end_resize_interaction(&mut self, now: Instant) {
        let ended = self.interaction_state.resize_active.take();
        if let Some(id) = ended {
            self.interaction_state.resize_static_node = Some(id);
            self.interaction_state.resize_static_lock_pos = self.field.node(id).map(|n| n.pos);
            self.interaction_state.resize_static_until_ms = self.now_ms(now).saturating_add(120);
            self.set_interaction_focus(Some(id), 30_000, now);
        } else {
            self.interaction_state.resize_static_lock_pos = None;
            self.set_interaction_focus(None, 0, now);
        }
        self.interaction_state.suspend_state_checks = false;
        self.interaction_state.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.request_maintenance();
    }

    pub fn resolve_overlap_now(&mut self) {
        let saved_suspend = self.interaction_state.suspend_overlap_resolve;
        self.interaction_state.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.interaction_state.suspend_overlap_resolve = saved_suspend;
    }

    pub fn set_last_active_size_now(&mut self, id: NodeId, size: Vec2) {
        self.workspace_state.last_active_size.insert(id, size);
    }

    pub fn last_focused_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus {
            let valid = self.field.node(id).is_some_and(|n| {
                self.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && matches!(
                        n.state,
                        halley_core::field::NodeState::Active | halley_core::field::NodeState::Node
                    )
            });
            if valid {
                return Some(id);
            }
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && matches!(
                            n.state,
                            halley_core::field::NodeState::Active
                                | halley_core::field::NodeState::Node
                        ))
                    .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub fn last_input_surface_node(&self) -> Option<NodeId> {
        if let Some(id) = self.focus_state.primary_interaction_focus {
            let valid = self.field.node(id).is_some_and(|n| {
                self.field.is_visible(id) && n.kind == halley_core::field::NodeKind::Surface
            });
            if valid {
                return Some(id);
            }
        }
        self.focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id) && n.kind == halley_core::field::NodeKind::Surface)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub fn toggle_last_focused_surface_node(&mut self, now: Instant) -> Option<NodeId> {
        let id = self.last_focused_surface_node()?;

        let state = self.field.node(id)?.state.clone();
        match state {
            halley_core::field::NodeState::Active => {
                let _ = self
                    .field
                    .set_state(id, halley_core::field::NodeState::Node);
                let _ = self.field.set_decay_level(id, DecayLevel::Cold);
                self.pending_spawn_activate_at_ms.remove(&id);
                self.workspace_state.manual_collapsed_nodes.insert(id);

                self.set_interaction_focus(None, 0, now);
                self.focus_state.pan_restore_active_focus = None;
                self.request_maintenance();
                Some(id)
            }
            halley_core::field::NodeState::Node => {
                self.workspace_state.manual_collapsed_nodes.remove(&id);
                let _ = self.field.set_decay_level(id, DecayLevel::Hot);
                self.pending_spawn_activate_at_ms.remove(&id);

                self.set_interaction_focus(Some(id), 30_000, now);
                self.request_maintenance();
                Some(id)
            }
            _ => None,
        }
    }
}
