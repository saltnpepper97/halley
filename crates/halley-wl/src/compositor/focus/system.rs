use super::*;
use crate::compositor::focus::read;
use crate::compositor::interaction::state::ViewportPanAnim;
use crate::compositor::surface::stack_focus_target_for_node;
use eventline::debug;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::{SERIAL_COUNTER, Serial};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;

use crate::compositor::ctx::FocusCtx;
use smithay::input::Seat;
use std::time::{Duration, Instant};

pub(crate) fn on_seat_focus_changed(
    ctx: &FocusCtx<'_>,
    seat: &Seat<Halley>,
    focused: Option<&WlSurface>,
) {
    debug!(
        "seat focus_changed -> {:?}",
        focused.map(|wl| format!("{:?}", wl.id()))
    );

    let client = focused.and_then(|wl| wl.client());
    set_data_device_focus(ctx.display_handle, seat, client.clone());
    set_primary_focus(ctx.display_handle, seat, client);
}

/// Single choke point for every keyboard focus change. When focus actually moves
/// to a *different* surface, it first flushes any stale non-modifier forwarded
/// keys (see `crate::input::keyboard::flush_stuck_forwarded_keys`) so the newly
/// focused surface can never inherit a stuck key that repeats forever. All
/// `keyboard.set_focus(...)` calls in the compositor should go through this.
pub(crate) fn set_keyboard_focus(st: &mut Halley, focus: Option<WlSurface>, serial: Serial) {
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return;
    };
    let current = keyboard.current_focus();
    let changed = match (&current, &focus) {
        (Some(c), Some(n)) => c.id() != n.id(),
        (None, None) => false,
        _ => true,
    };
    if changed {
        crate::input::keyboard::flush_stuck_forwarded_keys(st);
    }
    keyboard.set_focus(st, focus, serial);
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

fn keep_locked_focus_for_request(
    st: &Halley,
    locked_surface_node: Option<NodeId>,
    requested_focus_node: Option<NodeId>,
) -> bool {
    let Some(locked_node) = locked_surface_node else {
        return false;
    };
    if !st.is_fullscreen_active(locked_node) {
        return false;
    }
    let Some(requested_node) = requested_focus_node else {
        return true;
    };
    if requested_node == locked_node {
        return true;
    }

    let locked_monitor = st
        .fullscreen_monitor_for_node(locked_node)
        .map(str::to_string)
        .unwrap_or_else(|| st.monitor_for_node_or_current(locked_node));
    let requested_monitor = st.monitor_for_node_or_current(requested_node);
    requested_monitor == locked_monitor
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

pub fn set_app_focused(st: &mut Halley, focused: bool) {
    st.model.focus_state.app_focused = focused;
}

pub(crate) fn clear_keyboard_focus(st: &mut Halley) {
    set_keyboard_focus(st, None, SERIAL_COUNTER.next_serial());
    update_selection_focus_from_surface(st, None);
}

pub(crate) const VIEWPORT_PAN_DURATION_MS: u64 = 260;
const SPAWN_VIEW_HANDOFF_PAN_RATIO: f32 = 0.35;
const SPAWN_VIEW_HANDOFF_FOCUS_RATIO: f32 = 0.25;

/// True if `id` is a fullscreen session that was soft-suspended (e.g. a game you
/// alt+tabbed away from) and is currently sitting windowed on some monitor.
pub(crate) fn node_is_suspended_fullscreen(st: &Halley, id: NodeId) -> bool {
    st.model
        .fullscreen_state
        .fullscreen_suspended_node
        .values()
        .any(|&nid| nid == id)
}

/// Re-enter a soft-suspended fullscreen session for `fid`: restore the monitor's
/// pre-suspend camera center, then re-assert fullscreen. Triggered by deliberate
/// selection only (click, alt+tab, apogee) — never by plain field hover.
pub(crate) fn resume_suspended_fullscreen(st: &mut Halley, fid: NodeId) {
    if let Some(entry) = st
        .model
        .fullscreen_state
        .fullscreen_restore
        .get(&fid)
        .copied()
    {
        let target_monitor = st.monitor_for_node_or_current(fid);
        if st.model.monitor_state.current_monitor == target_monitor {
            // Don't snap the camera centre here. enter_xdg_fullscreen's camera snapshot
            // targets (window centre, zoom 1.0) and `tick_camera_smoothing` eases the
            // live viewport there, pairing the re-centre with the re-zoom so the resume
            // reads as a smooth grow-in-place rather than a centre teleport followed by
            // a separate zoom. Just drop any in-flight pan so smoothing is free to run
            // (an active pan animation otherwise suppresses the smoothing path).
            st.input.interaction_state.viewport_pan_anim = None;
        } else if let Some(space) = st.model.monitor_state.monitors.get_mut(&target_monitor) {
            // Off the active monitor there's no visible glide; restore directly.
            space.viewport.center = entry.viewport_center;
            space.camera_target_center = entry.viewport_center;
        }
    }
    st.enter_xdg_fullscreen(fid, None, Instant::now());
}

pub fn apply_wayland_focus_state(st: &mut Halley, id: Option<NodeId>) {
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        crate::protocol::wayland::session_lock::reassert_keyboard_focus_if_drifted(st);
        return;
    }
    let focus_id = fullscreen_focus_override(st, id).or(id);
    // Field hover-focus and drag set `suppress_fullscreen_resume_on_focus` so that
    // merely moving the pointer into a soft-suspended game's area doesn't yank it back
    // to fullscreen. Deliberate selection — a click, alt+tab, or apogee pick — leaves
    // the flag clear and resumes the session here.
    if !st.input.interaction_state.suppress_fullscreen_resume_on_focus
        && let Some(fid) = focus_id
        && node_is_suspended_fullscreen(st, fid)
    {
        resume_suspended_fullscreen(st, fid);
    }
    st.model.monitor_state.layer_keyboard_focus = None;
    let requested_focus_surface =
        focus_id.and_then(|fid| crate::compositor::focus::system::wl_surface_for_node(st, fid));
    let active_constrained_surface =
        crate::compositor::interaction::pointer::active_constrained_pointer_surface(st)
            .map(|(surface, _)| surface);
    let locked_surface_node = active_constrained_surface
        .as_ref()
        .and_then(|surface| st.model.surface_to_node.get(&surface.id()).copied());
    let keep_locked_focus = keep_locked_focus_for_request(st, locked_surface_node, focus_id);
    let focus_surface = if keep_locked_focus {
        active_constrained_surface
            .clone()
            .or(requested_focus_surface.clone())
    } else {
        requested_focus_surface.clone()
    };
    if !keep_locked_focus
        && active_constrained_surface
            .as_ref()
            .is_some_and(|surface| Some(surface.id()) != focus_surface.as_ref().map(|wl| wl.id()))
    {
        crate::compositor::interaction::pointer::release_active_pointer_constraint(st);
    }
    set_keyboard_focus(st, focus_surface.clone(), SERIAL_COUNTER.next_serial());
    update_selection_focus_from_surface(st, focus_surface.as_ref());

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let key = top.wl_surface().id();
        let node_id = st.model.surface_to_node.get(&key).copied();

        let activated = node_id.is_some_and(|nid| {
            st.model
                .monitor_state
                .node_monitor
                .get(&nid)
                .and_then(|monitor| st.model.focus_state.monitor_focus.get(monitor))
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
            st.apply_toplevel_tiled_hint(s);
            was_active != activated
        });

        if state_changed {
            top.send_configure();
        }
    }
}

pub fn update_focus_tracking_for_surface(st: &mut Halley, fid: NodeId, now_ms: u64) {
    let Some(node_state) = st
        .model
        .field
        .node(fid)
        .map(|n| (n.kind.clone(), n.state.clone()))
    else {
        return;
    };
    if node_state.0 != halley_core::field::NodeKind::Surface
        || !st.model.field.participates_in_field_activity(fid)
        || !st.model.field.is_visible(fid)
    {
        return;
    }

    st.model
        .focus_state
        .last_surface_focus_ms
        .insert(fid, now_ms);
    st.model
        .focus_state
        .outside_focus_ring_since_ms
        .remove(&fid);
    if st.model.focus_state.suppress_trail_record_once {
        st.model.focus_state.suppress_trail_record_once = false;
    } else {
        st.record_focus_trail_visit(fid);
    }

    if node_state.1 == halley_core::field::NodeState::Active {
        let _ = st.model.field.touch(fid, now_ms);
        let _ = st.model.field.set_decay_level(fid, DecayLevel::Hot);
        if st.runtime.tuning.restore_last_active_on_pan_return {
            st.model.focus_state.pan_restore_active_focus = Some(fid);
        }
    }
}

pub fn note_pan_activity(st: &mut Halley, now: Instant) {
    st.input.interaction_state.viewport_pan_anim = None;
    let now_ms = st.now_ms(now);
    st.input.interaction_state.pan_dominant_until_ms = now_ms.saturating_add(220);
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let viewport_center = st.model.viewport.center;
    let spawn = st.spawn_monitor_state_mut(current_monitor.as_str());
    spawn.spawn_last_pan_ms = now_ms;
    spawn.spawn_pan_start_center.get_or_insert(viewport_center);
    if st.runtime.tuning.restore_last_active_on_pan_return
        && st.model.focus_state.pan_restore_active_focus.is_none()
    {
        st.model.focus_state.pan_restore_active_focus = read::last_focused_active_surface_node(st);
    }
    st.input.interaction_state.suspend_overlap_resolve = false;
    st.input.interaction_state.suspend_state_checks = false;
    st.request_maintenance();
}

fn spawn_view_handoff_pan_distance(st: &Halley) -> f32 {
    st.model.viewport.size.x.min(st.model.viewport.size.y) * SPAWN_VIEW_HANDOFF_PAN_RATIO
}

fn spawn_view_handoff_focus_distance(st: &Halley) -> f32 {
    st.model.viewport.size.x.hypot(st.model.viewport.size.y) * SPAWN_VIEW_HANDOFF_FOCUS_RATIO
}

pub(crate) fn note_pan_viewport_change(st: &mut Halley, _now: Instant) {
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let spawn_pan_active = st.model.spawn_state.active_spawn_pan.is_some()
        || !st.model.spawn_state.pending_spawn_pan_queue.is_empty()
        || st.model.spawn_state.pending_pan_activate.is_some();
    if st
        .spawn_monitor_state(current_monitor.as_str())
        .spawn_anchor_mode
        == crate::compositor::spawn::state::SpawnAnchorMode::View
        && !spawn_pan_active
    {
        st.spawn_monitor_state_mut(current_monitor.as_str())
            .spawn_view_anchor = st.model.viewport.center;
    }
    st.request_maintenance();
    if spawn_pan_active {
        return;
    }
    let Some(start_center) = st
        .spawn_monitor_state(current_monitor.as_str())
        .spawn_pan_start_center
    else {
        return;
    };

    let moved = ((st.model.viewport.center.x - start_center.x).powi(2)
        + (st.model.viewport.center.y - start_center.y).powi(2))
    .sqrt();
    if moved < spawn_view_handoff_pan_distance(st) {
        return;
    }

    let focus_far = st
        .last_input_surface_node()
        .and_then(|id| st.model.field.node(id))
        .map(|node| {
            let dx = st.model.viewport.center.x - node.pos.x;
            let dy = st.model.viewport.center.y - node.pos.y;
            dx.hypot(dy) >= spawn_view_handoff_focus_distance(st)
        })
        .unwrap_or(true);
    if !focus_far {
        return;
    }

    let viewport_center = st.model.viewport.center;
    let spawn = st.spawn_monitor_state_mut(current_monitor.as_str());
    spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
    spawn.spawn_view_anchor = viewport_center;
    spawn.spawn_patch = None;
    spawn.spawn_pan_start_center = Some(viewport_center);
    st.model.focus_state.pan_restore_active_focus = None;
}

pub fn set_pan_restore_focus_target(st: &mut Halley, id: NodeId) {
    st.model.focus_state.pan_restore_active_focus = Some(id);
}

pub fn animate_viewport_center_to(st: &mut Halley, target_center: Vec2, now: Instant) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    animate_viewport_center_to_on_monitor_delayed(st, monitor.as_str(), target_center, now, 0)
}

pub fn animate_viewport_center_to_on_monitor(
    st: &mut Halley,
    monitor: &str,
    target_center: Vec2,
    now: Instant,
) -> bool {
    animate_viewport_center_to_on_monitor_delayed(st, monitor, target_center, now, 0)
}

pub fn animate_viewport_center_to_delayed(
    st: &mut Halley,
    target_center: Vec2,
    now: Instant,
    delay_ms: u64,
) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    animate_viewport_center_to_on_monitor_delayed(
        st,
        monitor.as_str(),
        target_center,
        now,
        delay_ms,
    )
}

pub fn animate_viewport_center_to_on_monitor_delayed(
    st: &mut Halley,
    monitor: &str,
    target_center: Vec2,
    now: Instant,
    delay_ms: u64,
) -> bool {
    if !st.model.monitor_state.monitors.contains_key(monitor) {
        return false;
    }
    let from = if st.model.monitor_state.current_monitor == monitor {
        st.model.viewport.center
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport.center)
            .unwrap_or(st.model.viewport.center)
    };
    let dx = target_center.x - from.x;
    let dy = target_center.y - from.y;
    if dx.abs() < 0.25 && dy.abs() < 0.25 {
        return false;
    }
    st.input.interaction_state.viewport_pan_anim = Some(ViewportPanAnim {
        monitor: monitor.to_string(),
        start_ms: st.now_ms(now),
        delay_ms,
        duration_ms: VIEWPORT_PAN_DURATION_MS,
        from_center: from,
        to_center: target_center,
        from_view_size: None,
        to_view_size: None,
    });
    true
}

/// Start a fixed-duration camera transition that eases BOTH the center and the
/// zoom (view size) along the same `ease_in_out_cubic`, taking over from the
/// exponential zoom smoothing for its duration. Used by maximize/fullscreen
/// enter+exit so the grow/shrink completes uniformly with the window animation
/// and there is no asymptotic zoom "settling" tail. No-op (returns false) for an
/// unknown monitor.
pub(crate) fn animate_camera_center_zoom_on_monitor(
    st: &mut Halley,
    monitor: &str,
    to_center: Vec2,
    to_view_size: Vec2,
    duration_ms: u64,
    now: Instant,
) -> bool {
    if !st.model.monitor_state.monitors.contains_key(monitor) {
        return false;
    }
    let (from_center, from_view_size) = if st.model.monitor_state.current_monitor == monitor {
        (st.model.viewport.center, st.model.zoom_ref_size)
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| (space.viewport.center, space.zoom_ref_size))
            .unwrap_or((st.model.viewport.center, st.model.zoom_ref_size))
    };
    st.input.interaction_state.viewport_pan_anim = Some(ViewportPanAnim {
        monitor: monitor.to_string(),
        start_ms: st.now_ms(now),
        delay_ms: 0,
        duration_ms: duration_ms.max(1),
        from_center,
        to_center,
        from_view_size: Some(from_view_size),
        to_view_size: Some(to_view_size),
    });
    true
}

pub(crate) fn tick_viewport_pan_animation(st: &mut Halley, now_ms: u64) {
    let Some(anim) = st.input.interaction_state.viewport_pan_anim.clone() else {
        return;
    };

    if !st
        .model
        .monitor_state
        .monitors
        .contains_key(anim.monitor.as_str())
    {
        st.input.interaction_state.viewport_pan_anim = None;
        return;
    }

    let set_center = |st: &mut Halley, center: Vec2| {
        if st.model.monitor_state.current_monitor == anim.monitor {
            st.model.viewport.center = center;
            st.model.camera_target_center = center;
            st.runtime.tuning.viewport_center = center;
        }
        if let Some(space) = st
            .model
            .monitor_state
            .monitors
            .get_mut(anim.monitor.as_str())
        {
            space.viewport.center = center;
            space.camera_target_center = center;
        }
    };

    // Optional zoom track: ease the camera view size along the same cubic so a
    // maximize/fullscreen grow zooms in lockstep with its rect, with no
    // exponential-settle tail. Keeps both the live zoom and its target in sync so
    // the smoothing pass (which yields while a pan anim is live) has nothing to
    // chase once we finish.
    let set_view_size = |st: &mut Halley, size: Vec2| {
        if st.model.monitor_state.current_monitor == anim.monitor {
            st.model.zoom_ref_size = size;
            st.model.camera_target_view_size = size;
            st.runtime.tuning.viewport_size = size;
        }
        if let Some(space) = st
            .model
            .monitor_state
            .monitors
            .get_mut(anim.monitor.as_str())
        {
            // `space.zoom_ref_size` is the live zoom; `space.viewport.size` is the
            // fixed base monitor size (camera_render_scale denominator) and must
            // be left alone.
            space.zoom_ref_size = size;
            space.camera_target_view_size = size;
        }
    };

    if now_ms <= anim.start_ms.saturating_add(anim.delay_ms) {
        set_center(st, anim.from_center);
        if let Some(from_size) = anim.from_view_size {
            set_view_size(st, from_size);
        }
        return;
    }
    let dur = anim.duration_ms.max(1);
    let elapsed_ms = now_ms.saturating_sub(anim.start_ms.saturating_add(anim.delay_ms));
    let t = (elapsed_ms as f32 / dur as f32).clamp(0.0, 1.0);
    let e = crate::animation::ease_in_out_cubic(t);
    let center = Vec2 {
        x: anim.from_center.x + (anim.to_center.x - anim.from_center.x) * e,
        y: anim.from_center.y + (anim.to_center.y - anim.from_center.y) * e,
    };
    set_center(st, center);
    if let (Some(from_size), Some(to_size)) = (anim.from_view_size, anim.to_view_size) {
        let size = Vec2 {
            x: from_size.x + (to_size.x - from_size.x) * e,
            y: from_size.y + (to_size.y - from_size.y) * e,
        };
        set_view_size(st, size);
    }
    if t >= 1.0 {
        st.input.interaction_state.viewport_pan_anim = None;
    }
}

pub(crate) fn maybe_pan_to_restored_focus_on_close(
    st: &mut Halley,
    monitor: &str,
    id: NodeId,
    now: Instant,
) -> bool {
    read::maybe_pan_to_restored_focus_on_close(st, monitor, id, now)
}

pub fn begin_resize_interaction(st: &mut Halley, id: NodeId, now: Instant) {
    st.input.interaction_state.resize_active = Some(id);
    st.input.interaction_state.resize_static_node = Some(id);
    st.input.interaction_state.resize_static_lock_pos = None;
    st.input.interaction_state.resize_static_until_ms = st.now_ms(now).saturating_add(60_000);
    st.input.interaction_state.suspend_overlap_resolve = true;
    st.input.interaction_state.suspend_state_checks = true;
    st.set_interaction_focus(Some(id), 60_000, now);
    let now_ms = st.now_ms(now);
    if st.model.field.participates_in_field_activity(id) {
        let _ = st.model.field.touch(id, now_ms);
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
    }
    st.model.workspace_state.manual_collapsed_nodes.remove(&id);
    st.request_maintenance();
}

pub fn end_resize_interaction(st: &mut Halley, now: Instant) {
    let ended = st.input.interaction_state.resize_active.take();
    if let Some(id) = ended {
        st.input.interaction_state.resize_static_node = Some(id);
        st.input.interaction_state.resize_static_lock_pos = st.model.field.node(id).map(|n| n.pos);
        st.input.interaction_state.resize_static_until_ms = st.now_ms(now).saturating_add(120);
        st.set_interaction_focus(Some(id), 30_000, now);
    } else {
        st.input.interaction_state.resize_static_lock_pos = None;
        st.set_interaction_focus(None, 0, now);
    }
    st.input.interaction_state.suspend_state_checks = false;
    st.input.interaction_state.suspend_overlap_resolve = false;
    st.resolve_surface_overlap();
    st.request_maintenance();
}

pub fn resolve_overlap_now(st: &mut Halley) {
    let saved_suspend = st.input.interaction_state.suspend_overlap_resolve;
    st.input.interaction_state.suspend_overlap_resolve = false;
    st.resolve_surface_overlap();
    st.input.interaction_state.suspend_overlap_resolve = saved_suspend;
}

pub fn set_last_active_size_now(st: &mut Halley, id: NodeId, size: Vec2) {
    st.model.workspace_state.last_active_size.insert(id, size);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
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
        tuning
    }

    #[test]
    fn focusing_pointer_target_does_not_raise_window() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let id = state.model.field.spawn_surface(
            "hovered",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        state.assign_node_to_current_monitor(id);

        let before = state.overlap_policy_stack_rank(id);
        state.focus_pointer_target(id, 30_000, Instant::now());

        assert_eq!(state.overlap_policy_stack_rank(id), before);
    }

    #[test]
    fn spawn_pan_does_not_clear_active_spawn_patch() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let monitor = state.model.monitor_state.current_monitor.clone();
        let anchor = Vec2 { x: 0.0, y: 0.0 };
        let id = state.model.field.spawn_surface(
            "spawned",
            Vec2 { x: 300.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_current_monitor(id);
        state.update_spawn_patch(
            monitor.as_str(),
            anchor,
            None,
            anchor,
            Vec2 { x: 1.0, y: 0.0 },
        );
        state.model.spawn_state.active_spawn_pan =
            Some(crate::compositor::spawn::state::ActiveSpawnPan {
                node_id: id,
                pan_start_at_ms: 0,
                reveal_at_ms: 0,
            });
        state.model.viewport.center = Vec2 { x: 300.0, y: 0.0 };

        state.note_pan_viewport_change(Instant::now());

        assert_eq!(
            state
                .spawn_monitor_state(monitor.as_str())
                .spawn_patch
                .as_ref()
                .map(|patch| patch.anchor),
            Some(anchor)
        );
    }

    #[test]
    fn locked_fullscreen_focus_is_not_kept_for_different_monitor_request() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, two_monitor_tuning());

        let game = state.model.field.spawn_surface(
            "game",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let steam = state.model.field.spawn_surface(
            "steam",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 400.0, y: 300.0 },
        );
        let same_monitor = state.model.field.spawn_surface(
            "same-monitor",
            Vec2 { x: 320.0, y: 240.0 },
            Vec2 { x: 300.0, y: 220.0 },
        );
        state.assign_node_to_monitor(game, "left");
        state.assign_node_to_monitor(steam, "right");
        state.assign_node_to_monitor(same_monitor, "left");
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), game);

        assert!(keep_locked_focus_for_request(&state, Some(game), None));
        assert!(keep_locked_focus_for_request(
            &state,
            Some(game),
            Some(game)
        ));
        assert!(keep_locked_focus_for_request(
            &state,
            Some(game),
            Some(same_monitor)
        ));
        assert!(!keep_locked_focus_for_request(
            &state,
            Some(game),
            Some(steam)
        ));
    }

    #[test]
    fn viewport_pan_animation_stays_on_origin_monitor_after_monitor_switch() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, two_monitor_tuning());
        let _ = state.activate_monitor("right");

        let left_center = state
            .model
            .monitor_state
            .monitors
            .get("left")
            .expect("left monitor")
            .viewport
            .center;
        let target = Vec2 {
            x: 1500.0,
            y: 300.0,
        };
        let now = Instant::now();
        assert!(state.animate_viewport_center_to(target, now));
        assert_eq!(
            state
                .input
                .interaction_state
                .viewport_pan_anim
                .as_ref()
                .map(|anim| anim.monitor.as_str()),
            Some("right")
        );

        let _ = state.activate_monitor("left");
        state.tick_viewport_pan_animation(state.now_ms(now) + 1_000);

        assert_eq!(state.model.viewport.center, left_center);
        assert_eq!(
            state
                .model
                .monitor_state
                .monitors
                .get("right")
                .expect("right monitor")
                .viewport
                .center,
            target
        );

        let _ = state.activate_monitor("right");
        assert_eq!(state.model.viewport.center, target);
    }

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

        let _ = state.raise_overlap_policy_node(fullscreen_left);
        assert!(state.raise_overlap_policy_node(other_left));
        assert!(state.node_draws_above_fullscreen_on_monitor(other_left, "left"));
        assert_eq!(
            state.fullscreen_focus_override(Some(other_left)),
            Some(other_left)
        );
    }

    #[test]
    fn fullscreen_focus_override_allows_same_monitor_overlap_policy_window() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let overlap = state.model.field.spawn_surface(
            "overlap",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        state.assign_node_to_monitor(fullscreen, monitor.as_str());
        state.assign_node_to_monitor(overlap, monitor.as_str());
        let _ = state
            .model
            .field
            .set_state(fullscreen, halley_core::field::NodeState::Active);
        let _ = state
            .model
            .field
            .set_state(overlap, halley_core::field::NodeState::Active);
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor, fullscreen);
        state.model.spawn_state.applied_window_rules.insert(
            overlap,
            crate::compositor::spawn::state::AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                opacity: 1.0,
                parent_node: None,
                suppress_reveal_pan: true,
                builtin_rule: None,
            },
        );

        assert_eq!(
            state.fullscreen_focus_override(Some(overlap)),
            Some(overlap)
        );
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
