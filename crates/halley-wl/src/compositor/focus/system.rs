use super::*;
use crate::compositor::focus::read;
use crate::compositor::interaction::state::ViewportPanAnim;
use crate::compositor::surface_ops::stack_focus_target_for_node;
use eventline::debug;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

use crate::compositor::ctx::FocusCtx;
use smithay::input::Seat;
use std::ops::{Deref, DerefMut};
use std::time::{Duration, Instant};

pub(crate) fn on_seat_focus_changed(
    ctx: &mut FocusCtx<'_>,
    seat: &Seat<Halley>,
    focused: Option<&WlSurface>,
) {
    let st = &mut ctx.st;
    debug!(
        "seat focus_changed -> {:?}",
        focused.map(|wl| format!("{:?}", wl.id()))
    );

    let client = focused.and_then(|wl| wl.client());
    set_data_device_focus(&st.platform.display_handle, seat, client.clone());
    set_primary_focus(&st.platform.display_handle, seat, client);
}

pub(crate) struct FocusSystemController<T> {
    st: T,
}

pub(crate) fn focus_system_controller<T>(st: T) -> FocusSystemController<T> {
    FocusSystemController { st }
}

impl<T: Deref<Target = Halley>> Deref for FocusSystemController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for FocusSystemController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

pub fn wl_surface_for_node(st: &Halley, id: NodeId) -> Option<WlSurface> {
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface().clone();
        if st.model.surface_to_node.get(&wl.id()).copied() == Some(id) {
            return Some(wl);
        }
    }
    None
}

pub(crate) fn update_selection_focus_from_surface(st: &Halley, surface: Option<&WlSurface>) {
    let client = surface.and_then(|wl| wl.client());
    set_data_device_focus(
        &st.platform.display_handle,
        &st.platform.seat,
        client.clone(),
    );
    set_primary_focus(&st.platform.display_handle, &st.platform.seat, client);
}

pub(crate) fn focus_pointer_target(
    st: &mut Halley,
    node_id: NodeId,
    hold_ms: u64,
    now: Instant,
) -> NodeId {
    let focus_target = stack_focus_target_for_node(st, node_id).unwrap_or(node_id);
    st.set_recent_top_node(focus_target, now + Duration::from_millis(1200));
    st.set_interaction_focus(Some(focus_target), hold_ms, now);
    focus_target
}

pub(crate) fn surface_is_fully_visible_on_monitor(st: &Halley, monitor: &str, id: NodeId) -> bool {
    read::surface_is_fully_visible_on_monitor(st, monitor, id)
}

pub(crate) fn minimal_reveal_center_for_surface_on_monitor(
    st: &Halley,
    monitor: &str,
    id: NodeId,
) -> Option<Vec2> {
    read::minimal_reveal_center_for_surface_on_monitor(st, monitor, id)
}

#[cfg(test)]
pub(crate) fn fullscreen_focus_override(st: &Halley, requested: Option<NodeId>) -> Option<NodeId> {
    read::fullscreen_focus_override(st, requested)
}

pub fn last_focused_surface_node(st: &Halley) -> Option<NodeId> {
    read::last_focused_surface_node(st)
}

pub fn last_focused_surface_node_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    read::last_focused_surface_node_for_monitor(st, monitor)
}

pub fn last_input_surface_node(st: &Halley) -> Option<NodeId> {
    read::last_input_surface_node(st)
}

pub fn last_input_surface_node_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    read::last_input_surface_node_for_monitor(st, monitor)
}

impl<T: DerefMut<Target = Halley>> FocusSystemController<T> {
    pub fn set_app_focused(&mut self, focused: bool) {
        self.model.focus_state.app_focused = focused;
    }

    pub(crate) fn clear_keyboard_focus(&mut self) {
        let Some(keyboard) = self.platform.seat.get_keyboard() else {
            return;
        };
        keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        self.update_selection_focus_from_surface(None);
    }
}

impl<T: DerefMut<Target = Halley>> FocusSystemController<T> {
    pub(crate) const VIEWPORT_PAN_DURATION_MS: u64 = 260;
    const SPAWN_VIEW_HANDOFF_PAN_RATIO: f32 = 0.35;
    const SPAWN_VIEW_HANDOFF_FOCUS_RATIO: f32 = 0.25;

    pub(crate) fn fullscreen_focus_override(&self, requested: Option<NodeId>) -> Option<NodeId> {
        read::fullscreen_focus_override(self, requested)
    }

    pub fn apply_wayland_focus_state(&mut self, id: Option<NodeId>) {
        if crate::protocol::wayland::session_lock::session_lock_active(self) {
            crate::protocol::wayland::session_lock::reassert_keyboard_focus_if_drifted(self);
            return;
        }
        let focus_id = self.fullscreen_focus_override(id).or(id);
        if let Some(fid) = focus_id
            && self
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .values()
                .any(|&nid| nid == fid)
        {
            if let Some(entry) = self
                .model
                .fullscreen_state
                .fullscreen_restore
                .get(&fid)
                .copied()
            {
                let target_monitor = self.monitor_for_node_or_current(fid);
                if let Some(space) = self.model.monitor_state.monitors.get_mut(&target_monitor) {
                    space.viewport.center = entry.viewport_center;
                    space.camera_target_center = entry.viewport_center;
                }
                if self.model.monitor_state.current_monitor == target_monitor {
                    self.model.viewport.center = entry.viewport_center;
                    self.model.camera_target_center = self.model.viewport.center;
                    self.runtime.tuning.viewport_center = self.model.viewport.center;
                    self.input.interaction_state.viewport_pan_anim = None;
                }
            }
            self.enter_xdg_fullscreen(fid, None, Instant::now());
        }
        self.model.monitor_state.layer_keyboard_focus = None;
        let requested_focus_surface = focus_id.and_then(|fid| self.wl_surface_for_node(fid));
        let active_constrained_surface =
            crate::compositor::interaction::pointer::active_constrained_pointer_surface(self)
                .map(|(surface, _)| surface);
        let locked_surface_node = active_constrained_surface
            .as_ref()
            .and_then(|surface| self.model.surface_to_node.get(&surface.id()).copied());
        let keep_locked_focus =
            locked_surface_node.is_some_and(|nid| self.is_fullscreen_active(nid));
        let focus_surface = if keep_locked_focus {
            active_constrained_surface
                .clone()
                .or(requested_focus_surface.clone())
        } else {
            requested_focus_surface.clone()
        };
        if !keep_locked_focus
            && active_constrained_surface.as_ref().is_some_and(|surface| {
                Some(surface.id()) != focus_surface.as_ref().map(|wl| wl.id())
            })
        {
            crate::compositor::interaction::pointer::release_active_pointer_constraint(self);
        }
        if let Some(keyboard) = self.platform.seat.get_keyboard() {
            keyboard.set_focus(self, focus_surface.clone(), SERIAL_COUNTER.next_serial());
        }
        self.update_selection_focus_from_surface(focus_surface.as_ref());

        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let key = top.wl_surface().id();
            let node_id = self.model.surface_to_node.get(&key).copied();

            let activated = node_id.is_some_and(|nid| {
                self.model
                    .monitor_state
                    .node_monitor
                    .get(&nid)
                    .and_then(|monitor| self.model.focus_state.monitor_focus.get(monitor))
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
                self.apply_toplevel_tiled_hint(s);
                was_active != activated
            });

            if state_changed {
                top.send_configure();
            }
        }
    }

    pub fn update_focus_tracking_for_surface(&mut self, fid: NodeId, now_ms: u64) {
        let Some(node_state) = self
            .model
            .field
            .node(fid)
            .map(|n| (n.kind.clone(), n.state.clone()))
        else {
            return;
        };
        if node_state.0 != halley_core::field::NodeKind::Surface
            || !self.model.field.participates_in_field_activity(fid)
            || !self.model.field.is_visible(fid)
        {
            return;
        }

        self.model
            .focus_state
            .last_surface_focus_ms
            .insert(fid, now_ms);
        self.model
            .focus_state
            .outside_focus_ring_since_ms
            .remove(&fid);
        if self.model.focus_state.suppress_trail_record_once {
            self.model.focus_state.suppress_trail_record_once = false;
        } else {
            self.record_focus_trail_visit(fid);
        }

        if node_state.1 == halley_core::field::NodeState::Active {
            let _ = self.model.field.touch(fid, now_ms);
            let _ = self.model.field.set_decay_level(fid, DecayLevel::Hot);
            if self.runtime.tuning.restore_last_active_on_pan_return {
                self.model.focus_state.pan_restore_active_focus = Some(fid);
            }
        }
    }

    pub fn note_pan_activity(&mut self, now: Instant) {
        self.input.interaction_state.viewport_pan_anim = None;
        let now_ms = self.now_ms(now);
        self.input.interaction_state.pan_dominant_until_ms = now_ms.saturating_add(220);
        let current_monitor = self.model.monitor_state.current_monitor.clone();
        let viewport_center = self.model.viewport.center;
        let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
        spawn.spawn_last_pan_ms = now_ms;
        spawn.spawn_pan_start_center.get_or_insert(viewport_center);
        if self.runtime.tuning.restore_last_active_on_pan_return
            && self.model.focus_state.pan_restore_active_focus.is_none()
        {
            self.model.focus_state.pan_restore_active_focus =
                read::last_focused_active_surface_node(self);
        }
        self.input.interaction_state.suspend_overlap_resolve = false;
        self.input.interaction_state.suspend_state_checks = false;
        self.request_maintenance();
    }

    fn spawn_view_handoff_pan_distance(&self) -> f32 {
        self.model.viewport.size.x.min(self.model.viewport.size.y)
            * Self::SPAWN_VIEW_HANDOFF_PAN_RATIO
    }

    fn spawn_view_handoff_focus_distance(&self) -> f32 {
        self.model.viewport.size.x.hypot(self.model.viewport.size.y)
            * Self::SPAWN_VIEW_HANDOFF_FOCUS_RATIO
    }

    pub(crate) fn note_pan_viewport_change(&mut self, _now: Instant) {
        let current_monitor = self.model.monitor_state.current_monitor.clone();
        if self
            .spawn_monitor_state(current_monitor.as_str())
            .spawn_anchor_mode
            == crate::compositor::spawn::state::SpawnAnchorMode::View
        {
            self.spawn_monitor_state_mut(current_monitor.as_str())
                .spawn_view_anchor = self.model.viewport.center;
        }
        self.request_maintenance();
        let Some(start_center) = self
            .spawn_monitor_state(current_monitor.as_str())
            .spawn_pan_start_center
        else {
            return;
        };

        let moved = ((self.model.viewport.center.x - start_center.x).powi(2)
            + (self.model.viewport.center.y - start_center.y).powi(2))
        .sqrt();
        if moved < self.spawn_view_handoff_pan_distance() {
            return;
        }

        let focus_far = self
            .last_input_surface_node()
            .and_then(|id| self.model.field.node(id))
            .map(|node| {
                let dx = self.model.viewport.center.x - node.pos.x;
                let dy = self.model.viewport.center.y - node.pos.y;
                dx.hypot(dy) >= self.spawn_view_handoff_focus_distance()
            })
            .unwrap_or(true);
        if !focus_far {
            return;
        }

        let viewport_center = self.model.viewport.center;
        let spawn = self.spawn_monitor_state_mut(current_monitor.as_str());
        spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
        spawn.spawn_view_anchor = viewport_center;
        spawn.spawn_patch = None;
        spawn.spawn_pan_start_center = Some(viewport_center);
        self.model.focus_state.pan_restore_active_focus = None;
    }

    pub fn set_pan_restore_focus_target(&mut self, id: NodeId) {
        self.model.focus_state.pan_restore_active_focus = Some(id);
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
        let from = self.model.viewport.center;
        let dx = target_center.x - from.x;
        let dy = target_center.y - from.y;
        if dx.abs() < 0.25 && dy.abs() < 0.25 {
            return false;
        }
        self.input.interaction_state.viewport_pan_anim = Some(ViewportPanAnim {
            start_ms: self.now_ms(now),
            delay_ms,
            duration_ms: Self::VIEWPORT_PAN_DURATION_MS,
            from_center: from,
            to_center: target_center,
        });
        true
    }

    pub(crate) fn tick_viewport_pan_animation(&mut self, now_ms: u64) {
        let Some(anim) = &self.input.interaction_state.viewport_pan_anim else {
            return;
        };
        if now_ms <= anim.start_ms.saturating_add(anim.delay_ms) {
            self.model.viewport.center = anim.from_center;
            self.model.camera_target_center = self.model.viewport.center;
            self.runtime.tuning.viewport_center = self.model.viewport.center;
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
        self.model.viewport.center = Vec2 {
            x: anim.from_center.x + (anim.to_center.x - anim.from_center.x) * e,
            y: anim.from_center.y + (anim.to_center.y - anim.from_center.y) * e,
        };
        self.model.camera_target_center = self.model.viewport.center;
        self.runtime.tuning.viewport_center = self.model.viewport.center;
        if t >= 1.0 {
            self.input.interaction_state.viewport_pan_anim = None;
        }
    }

    pub(crate) fn maybe_pan_to_restored_focus_on_close(
        &mut self,
        monitor: &str,
        id: NodeId,
        now: Instant,
    ) -> bool {
        read::maybe_pan_to_restored_focus_on_close(self, monitor, id, now)
    }

    pub fn begin_resize_interaction(&mut self, id: NodeId, now: Instant) {
        self.input.interaction_state.resize_active = Some(id);
        self.input.interaction_state.resize_static_node = Some(id);
        self.input.interaction_state.resize_static_lock_pos = None;
        self.input.interaction_state.resize_static_until_ms =
            self.now_ms(now).saturating_add(60_000);
        self.input.interaction_state.suspend_overlap_resolve = true;
        self.input.interaction_state.suspend_state_checks = true;
        self.set_interaction_focus(Some(id), 60_000, now);
        let now_ms = self.now_ms(now);
        if self.model.field.participates_in_field_activity(id) {
            let _ = self.model.field.touch(id, now_ms);
            let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
        }
        self.model
            .workspace_state
            .manual_collapsed_nodes
            .remove(&id);
        self.request_maintenance();
    }

    pub fn end_resize_interaction(&mut self, now: Instant) {
        let ended = self.input.interaction_state.resize_active.take();
        if let Some(id) = ended {
            self.input.interaction_state.resize_static_node = Some(id);
            self.input.interaction_state.resize_static_lock_pos =
                self.model.field.node(id).map(|n| n.pos);
            self.input.interaction_state.resize_static_until_ms =
                self.now_ms(now).saturating_add(120);
            self.set_interaction_focus(Some(id), 30_000, now);
        } else {
            self.input.interaction_state.resize_static_lock_pos = None;
            self.set_interaction_focus(None, 0, now);
        }
        self.input.interaction_state.suspend_state_checks = false;
        self.input.interaction_state.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.request_maintenance();
    }

    pub fn resolve_overlap_now(&mut self) {
        let saved_suspend = self.input.interaction_state.suspend_overlap_resolve;
        self.input.interaction_state.suspend_overlap_resolve = false;
        self.resolve_surface_overlap();
        self.input.interaction_state.suspend_overlap_resolve = saved_suspend;
    }

    pub fn set_last_active_size_now(&mut self, id: NodeId, size: Vec2) {
        self.model.workspace_state.last_active_size.insert(id, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_input_surface_prefers_current_monitor_local_focus() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: -200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let right = state.model.field.spawn_surface(
            "right",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_monitor(left, "default");
        state.assign_node_to_monitor(right, "other");
        state.model.focus_state.primary_interaction_focus = Some(right);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(left, 1);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(right, 2);
        state
            .model
            .focus_state
            .monitor_focus
            .insert("default".to_string(), left);
        state
            .model
            .focus_state
            .monitor_focus
            .insert("other".to_string(), right);

        assert_eq!(
            state.last_input_surface_node_for_monitor("default"),
            Some(left)
        );
        assert_eq!(
            state.last_input_surface_node_for_monitor("other"),
            Some(right)
        );
    }

    #[test]
    fn fullscreen_focus_override_stays_on_requested_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let fullscreen_left = state.model.field.spawn_surface(
            "fullscreen-left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let right = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(fullscreen_left, "left");
        state.assign_node_to_monitor(right, "right");
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);
        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");

        assert_eq!(state.fullscreen_focus_override(Some(right)), Some(right));
        assert_eq!(state.fullscreen_focus_override(None), None);
    }

    #[test]
    fn fullscreen_focus_override_keeps_same_monitor_fullscreen() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let fullscreen_left = state.model.field.spawn_surface(
            "fullscreen-left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let other_left = state.model.field.spawn_surface(
            "other-left",
            Vec2 { x: 500.0, y: 300.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(fullscreen_left, "left");
        state.assign_node_to_monitor(other_left, "left");
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);
        state.set_interaction_monitor("left");
        state.set_focused_monitor("left");

        assert_eq!(
            state.fullscreen_focus_override(Some(other_left)),
            Some(fullscreen_left)
        );
        assert_eq!(state.fullscreen_focus_override(None), Some(fullscreen_left));
    }

    #[test]
    fn setting_interaction_focus_switches_current_monitor_to_focused_node_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let _ = state.activate_monitor("left");

        let right = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(right, "right");

        state.set_interaction_focus(Some(right), 30_000, Instant::now());

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.model.monitor_state.current_monitor, "right");
    }

    #[test]
    fn focus_monitor_view_restores_last_focused_surface_on_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let right = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(right, "right");
        state.set_interaction_focus(Some(right), 30_000, Instant::now());

        state.focus_monitor_view("left", Instant::now());
        assert_eq!(state.model.focus_state.primary_interaction_focus, None);

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.model.monitor_state.current_monitor, "right");
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(right)
        );
    }

    #[test]
    fn focus_monitor_view_uses_bare_monitor_view_when_no_surface_exists() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.model.monitor_state.current_monitor, "right");
        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
    }

    #[test]
    fn focus_monitor_view_does_not_restore_blocked_monitor_focus() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.close_restore_focus = false;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "right".to_string(),
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
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let right = state.model.field.spawn_surface(
            "right",
            Vec2 { x: 200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state.assign_node_to_monitor(right, "right");
        state
            .model
            .focus_state
            .monitor_focus
            .insert("right".to_string(), right);
        state
            .model
            .focus_state
            .blocked_monitor_focus_restore
            .insert("right".to_string());

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
    }
}
