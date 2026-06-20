use std::time::Instant;

use eventline::debug;
use smithay::backend::input::TouchSlot;
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::compositor::root::Halley;

use super::pointer::focus::pointer_focus_for_screen;

fn touch_focus_for_screen(
    st: &mut Halley,
    _ws_w: i32,
    _ws_h: i32,
    sx: f32,
    sy: f32,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
    Point<f64, Logical>,
)> {
    let now = Instant::now();
    let monitor = st.monitor_for_screen_or_interaction(sx, sy);
    let (ws_w, ws_h, local_sx, local_sy) = st.local_screen_in_monitor(monitor.as_str(), sx, sy);
    let focus = pointer_focus_for_screen(st, ws_w, ws_h, local_sx, local_sy, now, None)?;
    let location = if crate::compositor::monitor::layer_shell::is_layer_surface_tree(st, &focus.0)
        || crate::protocol::wayland::session_lock::is_session_lock_surface(st, &focus.0)
    {
        (local_sx as f64, local_sy as f64).into()
    } else {
        let cam_scale = st.camera_render_scale() as f64;
        (local_sx as f64 / cam_scale, local_sy as f64 / cam_scale).into()
    };
    Some((focus.0, focus.1, location))
}

pub(crate) fn handle_touch_down(
    st: &mut Halley,
    ws_w: i32,
    ws_h: i32,
    slot: TouchSlot,
    sx: f32,
    sy: f32,
    time_msec: u32,
) {
    if !st.runtime.tuning.input.gestures.enabled
        || !st.runtime.tuning.input.gestures.touch_passthrough
    {
        return;
    }
    let Some(handle) = st.platform.seat.get_touch() else {
        return;
    };
    let touch_focus = touch_focus_for_screen(st, ws_w, ws_h, sx, sy);
    let focus = touch_focus
        .as_ref()
        .map(|(surface, origin, _)| (surface.clone(), *origin));
    let location = touch_focus
        .map(|(_, _, location)| location)
        .unwrap_or_else(|| (sx as f64, sy as f64).into());
    debug!("touch down: slot={}", i32::from(slot));
    handle.down(
        st,
        focus,
        &DownEvent {
            slot,
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
        },
    );
}

pub(crate) fn handle_touch_motion(
    st: &mut Halley,
    ws_w: i32,
    ws_h: i32,
    slot: TouchSlot,
    sx: f32,
    sy: f32,
    time_msec: u32,
) {
    if !st.runtime.tuning.input.gestures.enabled
        || !st.runtime.tuning.input.gestures.touch_passthrough
    {
        return;
    }
    let Some(handle) = st.platform.seat.get_touch() else {
        return;
    };
    let touch_focus = touch_focus_for_screen(st, ws_w, ws_h, sx, sy);
    let focus = touch_focus
        .as_ref()
        .map(|(surface, origin, _)| (surface.clone(), *origin));
    let location = touch_focus
        .map(|(_, _, location)| location)
        .unwrap_or_else(|| (sx as f64, sy as f64).into());
    handle.motion(
        st,
        focus,
        &MotionEvent {
            slot,
            location,
            time: time_msec,
        },
    );
}

pub(crate) fn handle_touch_up(st: &mut Halley, slot: TouchSlot, time_msec: u32) {
    if !st.runtime.tuning.input.gestures.enabled
        || !st.runtime.tuning.input.gestures.touch_passthrough
    {
        return;
    }
    let Some(handle) = st.platform.seat.get_touch() else {
        return;
    };
    debug!("touch up: slot={}", i32::from(slot));
    handle.up(
        st,
        &UpEvent {
            slot,
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
        },
    );
}

pub(crate) fn handle_touch_frame(st: &mut Halley) {
    if !st.runtime.tuning.input.gestures.enabled
        || !st.runtime.tuning.input.gestures.touch_passthrough
    {
        return;
    }
    let Some(handle) = st.platform.seat.get_touch() else {
        return;
    };
    handle.frame(st);
}

pub(crate) fn handle_touch_cancel(st: &mut Halley) {
    if !st.runtime.tuning.input.gestures.enabled
        || !st.runtime.tuning.input.gestures.touch_passthrough
    {
        return;
    }
    let Some(handle) = st.platform.seat.get_touch() else {
        return;
    };
    debug!("touch cancel");
    handle.cancel(st);
}
