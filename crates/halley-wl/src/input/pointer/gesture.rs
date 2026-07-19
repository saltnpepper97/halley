use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eventline::debug;
use halley_config::{CompositorGestureScope, GestureBindingAction, GestureSwipeDirection};
use smithay::input::pointer::{
    GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
    MotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::backend::interface::BackendView;
use crate::compositor::interaction::state::{
    ActiveCompositorHold, ActiveCompositorPinch, ActiveCompositorPinchMode, ActiveCompositorSwipe,
    ActiveGestureRoute, SwipeMode,
};
use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;
use crate::input::keyboard::bindings::apply_compositor_action_press;
use crate::input::keyboard::modkeys::modifier_active;
use halley_core::field::Vec2;

use super::focus::{pointer_focus_for_screen, seat_focus_from_local};

const PINCH_ZOOM_ACTIVATE_LOG_DELTA: f32 = 0.12;
const PINCH_ZOOM_NOISE_LOG_DELTA: f32 = 0.04;
const PINCH_ZOOM_STRONG_LOG_DELTA: f32 = 0.18;
const PINCH_PAN_LOCK_PX: f32 = 4.0;
const PINCH_PAN_DEFINITE_LOCK_PX: f32 = 16.0;

/// Smoothing for release-velocity sampling (EMA weight on the newest sample).
const GESTURE_VEL_EMA: f32 = 0.35;

/// Upward travel (screen px) that fully opens Apogee when scrubbing it open with a
/// swipe; progress is `up_travel / this`, clamped to 1.0.
const APOGEE_GESTURE_OPEN_TRAVEL_PX: f32 = 320.0;
/// Upward travel before a swipe starts scrubbing Apogee open (ignores tiny drift).
const APOGEE_GESTURE_START_DEADZONE_PX: f32 = 8.0;
/// Release progress at/above which a scrubbed Apogee open commits (a flick also
/// commits below this).
const APOGEE_GESTURE_COMMIT_PROGRESS: f32 = 0.4;
/// Maximum scrubbed Apogee open speed. A hard flick can still commit the open,
/// but the visual transition will not jump faster than this.
const APOGEE_GESTURE_MAX_PROGRESS_PER_SEC: f32 = 4.0;

/// Whether a swipe of `fingers` going up is bound to open Apogee.
fn swipe_up_opens_apogee(st: &Halley, fingers: u32) -> bool {
    st.runtime
        .tuning
        .input
        .gestures
        .swipe_bindings
        .iter()
        .any(|binding| {
            binding.fingers == fingers
                && binding.direction == GestureSwipeDirection::Up
                && binding.action == GestureBindingAction::ApogeeOpen
        })
}

/// Fold a fresh per-event delta into an exponential-moving-average velocity in
/// screen px/sec. `last_msec`/`now_msec` are wrapping millisecond stamps.
fn sample_velocity(prev: Vec2, last_msec: u32, now_msec: u32, dx: f64, dy: f64) -> Vec2 {
    let dt_ms = now_msec.wrapping_sub(last_msec).max(1) as f32;
    let dt = (dt_ms / 1000.0).clamp(1.0 / 1000.0, 0.1);
    let inst = Vec2 {
        x: dx as f32 / dt,
        y: dy as f32 / dt,
    };
    Vec2 {
        x: prev.x + (inst.x - prev.x) * GESTURE_VEL_EMA,
        y: prev.y + (inst.y - prev.y) * GESTURE_VEL_EMA,
    }
}

#[inline]
fn vec_len(v: Vec2) -> f32 {
    v.x.hypot(v.y)
}

#[cfg(test)]
fn cap_progress_delta(current: f32, target: f32, max_delta: f32) -> f32 {
    let current = current.clamp(0.0, 1.0);
    let target = target.clamp(0.0, 1.0);
    current + (target - current).clamp(-max_delta.max(0.0), max_delta.max(0.0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingPinchIntent {
    Pan,
    Zoom,
}

struct GesturePointerTarget {
    monitor: String,
    local_sx: f32,
    local_sy: f32,
    focus: Option<(WlSurface, Point<f64, Logical>)>,
}

fn now_msec() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

fn gesture_passthrough_enabled(st: &Halley) -> bool {
    st.runtime.tuning.input.gestures.enabled && st.runtime.tuning.input.gestures.client_passthrough
}

fn gesture_target_at_pointer<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
) -> GesturePointerTarget {
    let (sx, sy) = ctx.pointer_state.borrow().screen;
    let monitor = st.monitor_for_screen_or_interaction(sx, sy);
    let (ws_w, ws_h, local_sx, local_sy) = st.local_screen_in_monitor(monitor.as_str(), sx, sy);
    let focus = pointer_focus_for_screen(st, ws_w, ws_h, local_sx, local_sy, Instant::now(), None);
    GesturePointerTarget {
        monitor,
        local_sx,
        local_sy,
        focus,
    }
}

fn focus_pointer_for_client_gesture(st: &mut Halley, target: &GesturePointerTarget) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    let Some(focus) = target.focus.clone() else {
        return;
    };
    let location = if crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, &focus.0)
        || crate::protocol::wayland::session_lock::is_session_lock_surface(st, &focus.0)
    {
        (target.local_sx as f64, target.local_sy as f64).into()
    } else {
        let cam_scale = st.camera_render_scale() as f64;
        (
            target.local_sx as f64 / cam_scale,
            target.local_sy as f64 / cam_scale,
        )
            .into()
    };

    let monitor = st.model.monitor_state.monitors.get(target.monitor.as_str());
    let seat_location = Point::<f64, Logical>::from((
        monitor.map_or(0.0, |monitor| monitor.offset_x as f64) + target.local_sx as f64,
        monitor.map_or(0.0, |monitor| monitor.offset_y as f64) + target.local_sy as f64,
    ));
    let focus = seat_focus_from_local(Some(focus), location, seat_location);
    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location: seat_location,
            serial: SERIAL_COUNTER.next_serial(),
            time: now_msec(),
        },
    );
    pointer.frame(st);
}

fn begin_client_route(st: &mut Halley, target: &GesturePointerTarget) -> bool {
    if !gesture_passthrough_enabled(st) {
        return false;
    }
    if target.focus.is_none() {
        return false;
    }
    focus_pointer_for_client_gesture(st, target);
    true
}

fn target_is_session_lock(st: &Halley, target: &GesturePointerTarget) -> bool {
    target.focus.as_ref().is_some_and(|(surface, _)| {
        crate::protocol::wayland::session_lock::is_session_lock_surface(st, surface)
    })
}

fn gesture_global_override_active<B: BackendView>(st: &Halley, ctx: &InputCtx<'_, B>) -> bool {
    modifier_active(
        &ctx.mod_state.borrow(),
        st.runtime.tuning.input.gestures.modifier,
    )
}

fn begin_pinch_route<B: BackendView>(st: &mut Halley, ctx: &InputCtx<'_, B>) -> ActiveGestureRoute {
    if !st.runtime.tuning.input.gestures.enabled {
        return ActiveGestureRoute::Ignored;
    }

    let target = gesture_target_at_pointer(st, ctx);
    if crate::compositor::interaction::pointer::active_constrained_pointer_surface(st).is_some() {
        return if begin_client_route(st, &target) {
            ActiveGestureRoute::Client
        } else {
            ActiveGestureRoute::Ignored
        };
    }

    let gestures = &st.runtime.tuning.input.gestures;
    let global_override = gesture_global_override_active(st, ctx);
    if target.focus.is_some()
        && (gestures.pinch_scope != CompositorGestureScope::Global && !global_override
            || target_is_session_lock(st, &target))
    {
        return if begin_client_route(st, &target) {
            ActiveGestureRoute::Client
        } else {
            ActiveGestureRoute::Ignored
        };
    }
    if !gestures.pinch_to_zoom
        || !st.runtime.tuning.zoom_enabled
        || crate::compositor::monitor::camera::zoom_blocked_by_interaction(&*st)
    {
        if target.focus.is_some() {
            return if begin_client_route(st, &target) {
                ActiveGestureRoute::Client
            } else {
                ActiveGestureRoute::Ignored
            };
        }
        return ActiveGestureRoute::Ignored;
    }

    st.activate_monitor(target.monitor.as_str());
    st.model.zoom_log_vel = 0.0;
    st.model.pan_vel = Vec2 { x: 0.0, y: 0.0 };
    debug!("gesture pinch route: compositor zoom");
    ActiveGestureRoute::CompositorPinch(ActiveCompositorPinch {
        monitor: target.monitor,
        start_view_size: st.model.camera_target_view_size,
        mode: ActiveCompositorPinchMode::Pending {
            delta: Vec2 { x: 0.0, y: 0.0 },
        },
        vel: Vec2 { x: 0.0, y: 0.0 },
        last_time_msec: 0,
    })
}

fn begin_swipe_route<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    fingers: u32,
) -> ActiveGestureRoute {
    if !st.runtime.tuning.input.gestures.enabled {
        return ActiveGestureRoute::Ignored;
    }

    let target = gesture_target_at_pointer(st, ctx);
    let apogee_context = st.input.interaction_state.apogee_session.is_some();
    let global_override = gesture_global_override_active(st, ctx);

    if crate::compositor::interaction::pointer::active_constrained_pointer_surface(st).is_some() {
        return if begin_client_route(st, &target) {
            ActiveGestureRoute::Client
        } else {
            ActiveGestureRoute::Ignored
        };
    }

    // Small-trackpad alias: gesture-modifier + pan-finger swipe acts as the
    // (pan_fingers + 1) action set (e.g. Mod+3 == 4-finger Apogee), but only when
    // such a binding actually exists.
    let gestures = &st.runtime.tuning.input.gestures;
    let pan_fingers = gestures.pan_fingers;

    // Continuous viewport pan: the dedicated pan finger-count (not in Apogee). The
    // gesture modifier doesn't remap finger counts — it only forces compositor
    // scope (see `global_override`), so e.g. mod+4-finger is still the 4-finger
    // Apogee action. Falls back to client/ignored when panning is blocked or a
    // client should own the gesture.
    let want_pan = !apogee_context && pan_fingers > 0 && fingers == pan_fingers;
    if want_pan {
        if crate::compositor::monitor::camera::pan_blocked_on_monitor(&*st, target.monitor.as_str())
            || (target.focus.is_some()
                && (gestures.compositor_scope != CompositorGestureScope::Global
                    && !global_override
                    || target_is_session_lock(st, &target)))
        {
            return begin_client_or_ignored_route(st, ctx);
        }
        st.activate_monitor(target.monitor.as_str());
        st.model.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        debug!("gesture swipe route: compositor pan");
        return ActiveGestureRoute::CompositorSwipe(ActiveCompositorSwipe {
            monitor: target.monitor,
            fingers,
            delta: Vec2 { x: 0.0, y: 0.0 },
            apogee_context: false,
            mode: SwipeMode::Pan,
            vel: Vec2 { x: 0.0, y: 0.0 },
            last_time_msec: 0,
            latched: None,
            drives_apogee_open: false,
        });
    }

    let candidate_bindings = if apogee_context {
        &gestures.apogee_swipe_bindings
    } else {
        &gestures.swipe_bindings
    };

    if target.focus.is_some()
        && !apogee_context
        && (gestures.compositor_scope != CompositorGestureScope::Global && !global_override
            || target_is_session_lock(st, &target))
    {
        return if begin_client_route(st, &target) {
            ActiveGestureRoute::Client
        } else {
            ActiveGestureRoute::Ignored
        };
    }

    if !candidate_bindings
        .iter()
        .any(|binding| binding.fingers == fingers)
    {
        if target.focus.is_some() && !apogee_context {
            return if begin_client_route(st, &target) {
                ActiveGestureRoute::Client
            } else {
                ActiveGestureRoute::Ignored
            };
        }
        return ActiveGestureRoute::Ignored;
    }

    debug!("gesture swipe route: compositor action");
    ActiveGestureRoute::CompositorSwipe(ActiveCompositorSwipe {
        monitor: target.monitor,
        fingers,
        delta: Vec2 { x: 0.0, y: 0.0 },
        apogee_context,
        mode: SwipeMode::Action,
        vel: Vec2 { x: 0.0, y: 0.0 },
        last_time_msec: 0,
        latched: None,
        drives_apogee_open: false,
    })
}

fn begin_client_or_ignored_route<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
) -> ActiveGestureRoute {
    let target = gesture_target_at_pointer(st, ctx);
    if begin_client_route(st, &target) {
        ActiveGestureRoute::Client
    } else {
        ActiveGestureRoute::Ignored
    }
}

fn active_route(st: &Halley) -> ActiveGestureRoute {
    st.input
        .interaction_state
        .active_gesture_route
        .clone()
        .unwrap_or(ActiveGestureRoute::Ignored)
}

fn clear_active_route(st: &mut Halley) {
    st.input.interaction_state.active_gesture_route = None;
}

fn pinch_target_view_size(start_view_size: Vec2, scale: f32) -> Vec2 {
    let scale = scale.clamp(0.05, 20.0);
    Vec2 {
        x: start_view_size.x / scale,
        y: start_view_size.y / scale,
    }
}

fn classify_pending_pinch(delta: Vec2, scale: f32) -> Option<PendingPinchIntent> {
    let pan_len = delta.x.hypot(delta.y);
    let zoom_delta = scale.max(0.001).ln().abs();

    if pan_len >= PINCH_PAN_DEFINITE_LOCK_PX && zoom_delta < PINCH_ZOOM_STRONG_LOG_DELTA {
        return Some(PendingPinchIntent::Pan);
    }
    if pan_len >= PINCH_PAN_LOCK_PX && zoom_delta < PINCH_ZOOM_NOISE_LOG_DELTA {
        return Some(PendingPinchIntent::Pan);
    }
    if zoom_delta >= PINCH_ZOOM_ACTIVATE_LOG_DELTA {
        return Some(PendingPinchIntent::Zoom);
    }

    None
}

fn pinch_pan_delta(st: &Halley, monitor: &str, delta_x: f64, delta_y: f64) -> Vec2 {
    let camera = crate::compositor::monitor::camera::camera_view_size(st);
    let (ws_w, ws_h) = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|monitor| (monitor.width, monitor.height))
        .unwrap_or((
            st.model.viewport.size.x as i32,
            st.model.viewport.size.y as i32,
        ));
    Vec2 {
        x: -(delta_x as f32) * camera.x.max(1.0) / (ws_w as f32).max(1.0),
        y: -(delta_y as f32) * camera.y.max(1.0) / (ws_h as f32).max(1.0),
    }
}

fn apply_pinch_pan<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    monitor: &str,
    delta_x: f64,
    delta_y: f64,
) {
    let pan = pinch_pan_delta(st, monitor, delta_x, delta_y);
    if pan.x.abs() < f32::EPSILON && pan.y.abs() < f32::EPSILON {
        return;
    }
    if crate::compositor::monitor::camera::pan_blocked_on_monitor(&*st, monitor) {
        return;
    }
    st.note_pan_activity(Instant::now());
    crate::compositor::monitor::camera::pan_camera_target(&mut *st, pan);
    st.note_pan_viewport_change(Instant::now());
    ctx.backend.request_output_redraw(monitor);
}

fn apply_pinch_zoom<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    monitor: &str,
    start_view_size: Vec2,
    scale: f64,
) {
    if !st.activate_monitor(monitor) {
        return;
    }
    crate::compositor::monitor::camera::set_camera_target_view_size(
        &mut *st,
        pinch_target_view_size(start_view_size, scale as f32),
    );
    ctx.backend.request_output_redraw(monitor);
}

fn classify_swipe_direction(delta: Vec2, threshold_px: f32) -> Option<GestureSwipeDirection> {
    let threshold_px = threshold_px.max(1.0);
    if delta.x.abs().max(delta.y.abs()) < threshold_px {
        return None;
    }
    if delta.x.abs() > delta.y.abs() {
        if delta.x < 0.0 {
            Some(GestureSwipeDirection::Left)
        } else {
            Some(GestureSwipeDirection::Right)
        }
    } else if delta.y < 0.0 {
        Some(GestureSwipeDirection::Up)
    } else {
        Some(GestureSwipeDirection::Down)
    }
}

fn apply_gesture_binding_action<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    action: &GestureBindingAction,
) -> bool {
    match action {
        GestureBindingAction::ApogeeOpen => {
            if st.input.interaction_state.apogee_session.is_none() {
                st.open_apogee(Instant::now());
                ctx.backend.request_redraw();
            }
            true
        }
        GestureBindingAction::ApogeeClose => {
            if st.input.interaction_state.apogee_session.is_some() {
                st.close_apogee(Instant::now());
                ctx.backend.request_redraw();
            }
            true
        }
        GestureBindingAction::Compositor(action) => {
            let changed = apply_compositor_action_press(
                st,
                action.clone(),
                ctx.config_path,
                ctx.wayland_display,
            );
            if changed {
                ctx.backend.request_redraw();
            }
            changed
        }
    }
}

fn commit_swipe_action<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    swipe: &ActiveCompositorSwipe,
) {
    if !st.activate_monitor(swipe.monitor.as_str()) {
        return;
    }
    let threshold = st.runtime.tuning.input.gestures.swipe_threshold_px;
    let flick_min = st.runtime.tuning.input.gestures.flick_min_px_per_s;

    // Direction priority: the latched direction (set when the gesture first crossed
    // the distance threshold) wins; otherwise a fast release flick commits even on a
    // short gesture; otherwise fall back to the net-delta classification. If none of
    // these yields a direction the gesture is treated as accidental and ignored.
    let flick_dir = if vec_len(swipe.vel) >= flick_min {
        classify_swipe_direction(swipe.vel, 1.0)
    } else {
        None
    };
    let direction = swipe
        .latched
        .or(flick_dir)
        .or_else(|| classify_swipe_direction(swipe.delta, threshold));
    let Some(direction) = direction else {
        return;
    };

    let bindings = if swipe.apogee_context {
        &st.runtime.tuning.input.gestures.apogee_swipe_bindings
    } else {
        &st.runtime.tuning.input.gestures.swipe_bindings
    };
    let action = bindings
        .iter()
        .find(|binding| binding.fingers == swipe.fingers && binding.direction == direction)
        .map(|binding| binding.action.clone());
    let Some(action) = action else {
        return;
    };
    let _ = apply_gesture_binding_action(st, ctx, &action);
}

pub(crate) fn handle_gesture_swipe_begin<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    fingers: u32,
    time_msec: u32,
) {
    let mut route = begin_swipe_route(st, ctx, fingers);
    if let ActiveGestureRoute::CompositorSwipe(swipe) = &mut route {
        swipe.last_time_msec = time_msec;
    }
    st.input.interaction_state.active_gesture_route = Some(route.clone());
    if !matches!(route, ActiveGestureRoute::Client) {
        return;
    }
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    debug!("gesture swipe begin: fingers={}", fingers);
    pointer.gesture_swipe_begin(
        st,
        &GestureSwipeBeginEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
            fingers,
        },
    );
}

pub(crate) fn handle_gesture_swipe_update<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    delta_x: f64,
    delta_y: f64,
    time_msec: u32,
) {
    match active_route(st) {
        ActiveGestureRoute::Client => {
            let Some(pointer) = st.platform.seat.get_pointer() else {
                return;
            };
            pointer.gesture_swipe_update(
                st,
                &GestureSwipeUpdateEvent {
                    time: time_msec,
                    delta: (delta_x, delta_y).into(),
                },
            );
        }
        ActiveGestureRoute::CompositorSwipe(mut swipe) => {
            let dt_ms = time_msec.wrapping_sub(swipe.last_time_msec).max(1) as f32;
            swipe.vel =
                sample_velocity(swipe.vel, swipe.last_time_msec, time_msec, delta_x, delta_y);
            swipe.last_time_msec = time_msec;
            match swipe.mode {
                SwipeMode::Pan => {
                    apply_pinch_pan(st, ctx, &swipe.monitor.clone(), delta_x, delta_y);
                }
                SwipeMode::Action => {
                    swipe.delta.x += delta_x as f32;
                    swipe.delta.y += delta_y as f32;
                    let up_travel = -swipe.delta.y;

                    // Once the gesture is clearly an upward 4-finger swipe bound to
                    // ApogeeOpen, build the overview and scrub it open with the
                    // finger instead of waiting for release.
                    if !swipe.drives_apogee_open
                        && !swipe.apogee_context
                        && st.input.interaction_state.apogee_session.is_none()
                        && up_travel > APOGEE_GESTURE_START_DEADZONE_PX
                        && up_travel >= swipe.delta.x.abs()
                        && swipe_up_opens_apogee(st, swipe.fingers)
                    {
                        st.begin_apogee_open_gesture(Instant::now());
                        swipe.drives_apogee_open = true;
                    }

                    if swipe.drives_apogee_open {
                        let target_progress =
                            (up_travel / APOGEE_GESTURE_OPEN_TRAVEL_PX).clamp(0.0, 1.0);
                        let max_delta = APOGEE_GESTURE_MAX_PROGRESS_PER_SEC
                            * (dt_ms / 1000.0).clamp(0.001, 0.1);
                        st.set_apogee_open_gesture_progress_capped(target_progress, max_delta);
                    } else if swipe.latched.is_none() {
                        swipe.latched = classify_swipe_direction(
                            swipe.delta,
                            st.runtime.tuning.input.gestures.swipe_threshold_px,
                        );
                    }
                }
            }
            st.input.interaction_state.active_gesture_route =
                Some(ActiveGestureRoute::CompositorSwipe(swipe));
        }
        ActiveGestureRoute::CompositorPinch(_)
        | ActiveGestureRoute::CompositorHold(_)
        | ActiveGestureRoute::Ignored => {}
    }
}

pub(crate) fn handle_gesture_swipe_end<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    cancelled: bool,
    time_msec: u32,
) {
    let route = active_route(st);
    clear_active_route(st);
    if let ActiveGestureRoute::CompositorSwipe(swipe) = route.clone() {
        // A scrubbed Apogee open always needs resolving — even on cancel — so it
        // doesn't get stuck partway open.
        if swipe.drives_apogee_open {
            let up_travel = -swipe.delta.y;
            let progress = (up_travel / APOGEE_GESTURE_OPEN_TRAVEL_PX).clamp(0.0, 1.0);
            let flick_up = swipe.vel.y < 0.0
                && vec_len(swipe.vel) >= st.runtime.tuning.input.gestures.flick_min_px_per_s;
            if !cancelled && (progress >= APOGEE_GESTURE_COMMIT_PROGRESS || flick_up) {
                st.commit_apogee_open_gesture(Instant::now());
            } else {
                st.cancel_apogee_open_gesture(Instant::now());
            }
            return;
        }
        if cancelled {
            return;
        }
        match swipe.mode {
            SwipeMode::Pan => {
                let gestures = &st.runtime.tuning.input.gestures;
                if gestures.pan_momentum && vec_len(swipe.vel) >= gestures.flick_min_px_per_s {
                    let world =
                        pinch_pan_delta(st, &swipe.monitor, swipe.vel.x as f64, swipe.vel.y as f64);
                    crate::compositor::monitor::camera::fling_pan(st, world);
                }
            }
            SwipeMode::Action => {
                commit_swipe_action(st, ctx, &swipe);
            }
        }
        return;
    }
    if !matches!(route, ActiveGestureRoute::Client) {
        return;
    }
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    debug!("gesture swipe end: cancelled={}", cancelled);
    pointer.gesture_swipe_end(
        st,
        &GestureSwipeEndEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
            cancelled,
        },
    );
}

pub(crate) fn handle_gesture_pinch_begin<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    fingers: u32,
    time_msec: u32,
) {
    let route = begin_pinch_route(st, ctx);
    st.input.interaction_state.active_gesture_route = Some(route.clone());
    if !matches!(route, ActiveGestureRoute::Client) {
        return;
    }
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    debug!("gesture pinch begin: fingers={}", fingers);
    pointer.gesture_pinch_begin(
        st,
        &GesturePinchBeginEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
            fingers,
        },
    );
}

pub(crate) fn handle_gesture_pinch_update<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    delta_x: f64,
    delta_y: f64,
    scale: f64,
    rotation: f64,
    time_msec: u32,
) {
    match active_route(st) {
        ActiveGestureRoute::Client => {
            let Some(pointer) = st.platform.seat.get_pointer() else {
                return;
            };
            pointer.gesture_pinch_update(
                st,
                &GesturePinchUpdateEvent {
                    time: time_msec,
                    delta: (delta_x, delta_y).into(),
                    scale,
                    rotation,
                },
            );
        }
        ActiveGestureRoute::CompositorPinch(mut pinch) => {
            pinch.vel =
                sample_velocity(pinch.vel, pinch.last_time_msec, time_msec, delta_x, delta_y);
            pinch.last_time_msec = time_msec;
            match pinch.mode.clone() {
                ActiveCompositorPinchMode::Pending { mut delta } => {
                    delta.x += delta_x as f32;
                    delta.y += delta_y as f32;
                    match classify_pending_pinch(delta, scale as f32) {
                        Some(PendingPinchIntent::Pan) => {
                            pinch.mode = ActiveCompositorPinchMode::Pan;
                            apply_pinch_pan(st, ctx, pinch.monitor.as_str(), delta_x, delta_y);
                        }
                        Some(PendingPinchIntent::Zoom) => {
                            pinch.mode = ActiveCompositorPinchMode::Zoom;
                            apply_pinch_zoom(
                                st,
                                ctx,
                                pinch.monitor.as_str(),
                                pinch.start_view_size,
                                scale,
                            );
                        }
                        None => {
                            pinch.mode = ActiveCompositorPinchMode::Pending { delta };
                        }
                    }
                }
                ActiveCompositorPinchMode::Pan => {
                    apply_pinch_pan(st, ctx, pinch.monitor.as_str(), delta_x, delta_y);
                }
                ActiveCompositorPinchMode::Zoom => {
                    apply_pinch_zoom(
                        st,
                        ctx,
                        pinch.monitor.as_str(),
                        pinch.start_view_size,
                        scale,
                    );
                }
            }
            st.input.interaction_state.active_gesture_route =
                Some(ActiveGestureRoute::CompositorPinch(pinch));
        }
        ActiveGestureRoute::CompositorSwipe(_)
        | ActiveGestureRoute::CompositorHold(_)
        | ActiveGestureRoute::Ignored => {}
    }
}

pub(crate) fn handle_gesture_pinch_end<B: BackendView>(
    st: &mut Halley,
    _ctx: &InputCtx<'_, B>,
    cancelled: bool,
    time_msec: u32,
) {
    let route = active_route(st);
    clear_active_route(st);
    if let ActiveGestureRoute::CompositorPinch(pinch) = route.clone() {
        if !cancelled && matches!(pinch.mode, ActiveCompositorPinchMode::Pan) {
            let gestures = &st.runtime.tuning.input.gestures;
            if gestures.pan_momentum && vec_len(pinch.vel) >= gestures.flick_min_px_per_s {
                let world =
                    pinch_pan_delta(st, &pinch.monitor, pinch.vel.x as f64, pinch.vel.y as f64);
                crate::compositor::monitor::camera::fling_pan(st, world);
            }
        }
        return;
    }
    if !matches!(route, ActiveGestureRoute::Client) {
        return;
    }
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    debug!("gesture pinch end: cancelled={}", cancelled);
    pointer.gesture_pinch_end(
        st,
        &GesturePinchEndEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
            cancelled,
        },
    );
}

fn begin_hold_route<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    fingers: u32,
) -> ActiveGestureRoute {
    if !st.runtime.tuning.input.gestures.enabled {
        return ActiveGestureRoute::Ignored;
    }

    let Some(action) = hold_binding_for_fingers(st, fingers) else {
        return begin_client_or_ignored_route(st, ctx);
    };

    let target = gesture_target_at_pointer(st, ctx);
    let global_override = gesture_global_override_active(st, ctx);

    if crate::compositor::interaction::pointer::active_constrained_pointer_surface(st).is_some() {
        return begin_client_or_ignored_route(st, ctx);
    }

    if target.focus.is_some()
        && (st.runtime.tuning.input.gestures.compositor_scope != CompositorGestureScope::Global
            && !global_override
            || target_is_session_lock(st, &target))
    {
        return begin_client_or_ignored_route(st, ctx);
    }

    st.activate_monitor(target.monitor.as_str());
    debug!("gesture hold route: compositor action fingers={}", fingers);
    ActiveGestureRoute::CompositorHold(ActiveCompositorHold {
        monitor: target.monitor,
        fingers,
        action,
    })
}

fn hold_binding_for_fingers(st: &Halley, fingers: u32) -> Option<GestureBindingAction> {
    st.runtime
        .tuning
        .input
        .gestures
        .hold_bindings
        .iter()
        .find(|binding| binding.fingers == fingers)
        .map(|binding| binding.action.clone())
}

pub(crate) fn handle_gesture_hold_begin<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    fingers: u32,
    time_msec: u32,
) {
    let route = begin_hold_route(st, ctx, fingers);
    st.input.interaction_state.active_gesture_route = Some(route.clone());
    if !matches!(route, ActiveGestureRoute::Client) {
        return;
    }
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    debug!("gesture hold begin: fingers={}", fingers);
    pointer.gesture_hold_begin(
        st,
        &GestureHoldBeginEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
            fingers,
        },
    );
}

pub(crate) fn handle_gesture_hold_end<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    cancelled: bool,
    time_msec: u32,
) {
    let route = active_route(st);
    clear_active_route(st);
    if let ActiveGestureRoute::CompositorHold(hold) = route {
        if !cancelled {
            debug!(
                "gesture hold commit: fingers={} monitor={}",
                hold.fingers, hold.monitor
            );
            st.activate_monitor(hold.monitor.as_str());
            let _ = apply_gesture_binding_action(st, ctx, &hold.action);
        }
        return;
    }
    if !matches!(route, ActiveGestureRoute::Client) {
        return;
    }
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    debug!("gesture hold end: cancelled={}", cancelled);
    pointer.gesture_hold_end(
        st,
        &GestureHoldEndEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
            cancelled,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinch_scale_shrinks_view_when_zooming_in() {
        let start = Vec2 {
            x: 1600.0,
            y: 900.0,
        };
        let target = pinch_target_view_size(start, 2.0);

        assert_eq!(target.x, 800.0);
        assert_eq!(target.y, 450.0);
    }

    #[test]
    fn pinch_scale_grows_view_when_zooming_out() {
        let start = Vec2 {
            x: 1600.0,
            y: 900.0,
        };
        let target = pinch_target_view_size(start, 0.5);

        assert_eq!(target.x, 3200.0);
        assert_eq!(target.y, 1800.0);
    }

    #[test]
    fn pending_pinch_zoom_allows_small_pan_drift() {
        let intent = classify_pending_pinch(Vec2 { x: 3.0, y: 4.0 }, 1.13);

        assert_eq!(intent, Some(PendingPinchIntent::Zoom));
    }

    #[test]
    fn pending_pinch_pan_wins_when_scale_is_noise() {
        let intent = classify_pending_pinch(Vec2 { x: 6.0, y: 0.0 }, 1.03);

        assert_eq!(intent, Some(PendingPinchIntent::Pan));
    }

    #[test]
    fn pending_pinch_definite_pan_wins_over_moderate_scale() {
        let intent = classify_pending_pinch(Vec2 { x: 16.0, y: 0.0 }, 1.10);

        assert_eq!(intent, Some(PendingPinchIntent::Pan));
    }

    #[test]
    fn pending_pinch_waits_when_movement_is_ambiguous() {
        let intent = classify_pending_pinch(Vec2 { x: 2.0, y: 0.0 }, 1.03);

        assert_eq!(intent, None);
    }

    #[test]
    fn apogee_progress_delta_is_capped() {
        assert!((cap_progress_delta(0.10, 0.90, 0.08) - 0.18).abs() < 0.001);
        assert!((cap_progress_delta(0.90, 0.10, 0.08) - 0.82).abs() < 0.001);
        assert!((cap_progress_delta(0.40, 0.44, 0.08) - 0.44).abs() < 0.001);
    }
}
