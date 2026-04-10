use std::time::Instant;

use smithay::input::pointer::{AxisFrame, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;
use crate::spatial::screen_to_world;

use super::focus::pointer_focus_for_screen;
use crate::input::keyboard::bindings::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};
use halley_config::{CompositorBindingAction, WHEEL_DOWN_CODE, WHEEL_UP_CODE};
use smithay::backend::input::{Axis, AxisRelativeDirection, AxisSource};

#[inline]
fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_pointer_axis_input<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    source: AxisSource,
    amount_v120_horizontal: Option<f64>,
    amount_v120_vertical: Option<f64>,
    amount_horizontal: Option<f64>,
    amount_vertical: Option<f64>,
    relative_direction_horizontal: AxisRelativeDirection,
    relative_direction_vertical: AxisRelativeDirection,
) {
    if st.exit_confirm_active() {
        return;
    }
    if st.screenshot_session_active() {
        return;
    }
    if crate::compositor::interaction::state::note_cursor_activity(st, st.now_ms(Instant::now())) {
        ctx.backend.request_redraw();
    }

    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let (sx, sy) = {
            let ps = ctx.pointer_state.borrow();
            (ps.screen.0, ps.screen.1)
        };
        let target_monitor = st
            .monitor_for_screen(sx, sy)
            .unwrap_or_else(|| st.interaction_monitor().to_string());
        st.set_interaction_monitor(target_monitor.as_str());
        let _ = st.activate_monitor(target_monitor.as_str());
        let (ws_w, ws_h, sx, sy) = st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.workspace_size = (ws_w, ws_h);
            ps.world = screen_to_world(st, ws_w, ws_h, sx, sy);
        }
        if let Some(pointer) = st.platform.seat.get_pointer() {
            if pointer.current_focus().is_none()
                && let Some(focus) =
                    pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, Instant::now(), None)
            {
                pointer.motion(
                    st,
                    Some(focus),
                    &MotionEvent {
                        location: (sx as f64, sy as f64).into(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: now_millis_u32(),
                    },
                );
            }
            if pointer.current_focus().is_some() {
                let mut frame = AxisFrame::new(now_millis_u32())
                    .source(source)
                    .relative_direction(Axis::Horizontal, relative_direction_horizontal)
                    .relative_direction(Axis::Vertical, relative_direction_vertical);
                if let Some(v120) = amount_v120_horizontal {
                    frame = frame.v120(Axis::Horizontal, v120.round() as i32);
                }
                if let Some(v120) = amount_v120_vertical {
                    frame = frame.v120(Axis::Vertical, v120.round() as i32);
                }
                let horizontal_value =
                    amount_horizontal.or_else(|| amount_v120_horizontal.map(|v| v / 8.0));
                let vertical_value =
                    amount_vertical.or_else(|| amount_v120_vertical.map(|v| v / 8.0));
                if let Some(v) = horizontal_value {
                    frame = frame.value(Axis::Horizontal, v);
                }
                if let Some(v) = vertical_value {
                    frame = frame.value(Axis::Vertical, v);
                }
                pointer.axis(st, frame);
                pointer.frame(st);
            }
        }
        return;
    }

    let mut steps = (amount_v120_vertical.unwrap_or(0.0) as f32) / 120.0;
    if steps.abs() < f32::EPSILON {
        let px = amount_vertical.unwrap_or(0.0) as f32;
        if px.abs() > f32::EPSILON {
            steps = px / 40.0;
        }
    }
    if steps.abs() >= f32::EPSILON {
        let steps = steps.clamp(-4.0, 4.0);
        let mods = ctx.mod_state.borrow().clone();
        let wheel_code = if steps > 0.0 {
            WHEEL_UP_CODE
        } else {
            WHEEL_DOWN_CODE
        };
        if let Some(action) = compositor_binding_action_active(st, wheel_code, &mods) {
            ctx.pointer_state.borrow_mut().panning = false;
            if matches!(
                action,
                CompositorBindingAction::ZoomIn | CompositorBindingAction::ZoomOut
            ) {
                crate::compositor::interaction::pointer::set_temporary_cursor_override_icon(
                    st,
                    if matches!(action, CompositorBindingAction::ZoomIn) {
                        smithay::input::pointer::CursorIcon::ZoomIn
                    } else {
                        smithay::input::pointer::CursorIcon::ZoomOut
                    },
                    Instant::now(),
                    220,
                );
            }
            if apply_compositor_action_press(st, action, ctx.config_path, ctx.wayland_display) {
                ctx.backend.request_redraw();
            }
            return;
        }
        if apply_bound_pointer_input(st, wheel_code, &mods, ctx.config_path, ctx.wayland_display) {
            ctx.pointer_state.borrow_mut().panning = false;
            ctx.backend.request_redraw();
            return;
        }
    }

    let (sx, sy) = {
        let ps = ctx.pointer_state.borrow();
        (ps.screen.0, ps.screen.1)
    };
    let target_monitor = st
        .monitor_for_screen(sx, sy)
        .unwrap_or_else(|| st.interaction_monitor().to_string());
    st.set_interaction_monitor(target_monitor.as_str());
    let _ = st.activate_monitor(target_monitor.as_str());
    let (ws_w, ws_h, sx, sy) = st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
    {
        let mut ps = ctx.pointer_state.borrow_mut();
        ps.workspace_size = (ws_w, ws_h);
    }
    let world_now = screen_to_world(st, ws_w, ws_h, sx, sy);
    ctx.pointer_state.borrow_mut().world = world_now;
    let now = Instant::now();
    let now_ms = st.now_ms(now);
    if steps.abs() >= f32::EPSILON {
        let overlay = crate::overlay::OverlayView::from_halley(st);
        let overflow_scrollable = overlay
            .cluster_overflow_member_ids_for_monitor(target_monitor.as_str())
            .len()
            > 15;
        let over_overflow_strip = overflow_scrollable
            && crate::overlay::cluster_overflow_strip_slot_at(
                &overlay,
                target_monitor.as_str(),
                sx,
                sy,
                now_ms,
            )
            .is_some();
        if over_overflow_strip {
            let delta = if steps > 0.0 { 1 } else { -1 };
            let changed =
                st.adjust_cluster_overflow_scroll_for_monitor(target_monitor.as_str(), delta);
            st.reveal_cluster_overflow_for_monitor(target_monitor.as_str(), now_ms);
            if changed {
                ctx.backend.request_redraw();
            }
            return;
        }
    }
    let resize_preview = ctx.pointer_state.borrow().resize;
    if let Some(pointer) = st.platform.seat.get_pointer() {
        if pointer.current_focus().is_none()
            && let Some(focus) =
                pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview)
        {
            let location =
                if crate::compositor::monitor::layer_shell::is_layer_surface(st, &focus.0)
                    || crate::protocol::wayland::session_lock::is_session_lock_surface(st, &focus.0)
                {
                    (sx as f64, sy as f64).into()
                } else {
                    let cam_scale = st.camera_render_scale() as f64;
                    (sx as f64 / cam_scale, sy as f64 / cam_scale).into()
                };
            pointer.motion(
                st,
                Some(focus),
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: now_millis_u32(),
                },
            );
        }
        if pointer.current_focus().is_some() {
            let mut frame = AxisFrame::new(now_millis_u32())
                .source(source)
                .relative_direction(Axis::Horizontal, relative_direction_horizontal)
                .relative_direction(Axis::Vertical, relative_direction_vertical);
            if let Some(v120) = amount_v120_horizontal {
                frame = frame.v120(Axis::Horizontal, v120.round() as i32);
            }
            if let Some(v120) = amount_v120_vertical {
                frame = frame.v120(Axis::Vertical, v120.round() as i32);
            }
            let horizontal_value =
                amount_horizontal.or_else(|| amount_v120_horizontal.map(|v| v / 8.0));
            let vertical_value = amount_vertical.or_else(|| amount_v120_vertical.map(|v| v / 8.0));
            if let Some(v) = horizontal_value {
                frame = frame.value(Axis::Horizontal, v);
            }
            if let Some(v) = vertical_value {
                frame = frame.value(Axis::Vertical, v);
            }
            if source == AxisSource::Finger {
                let horizontal_stopped = amount_horizontal.unwrap_or(0.0).abs() < f64::EPSILON
                    && amount_v120_horizontal.unwrap_or(0.0).abs() < f64::EPSILON;
                let vertical_stopped = amount_vertical.unwrap_or(0.0).abs() < f64::EPSILON
                    && amount_v120_vertical.unwrap_or(0.0).abs() < f64::EPSILON;
                if horizontal_stopped {
                    frame = frame.stop(Axis::Horizontal);
                }
                if vertical_stopped {
                    frame = frame.stop(Axis::Vertical);
                }
            }
            if horizontal_value.is_some()
                || vertical_value.is_some()
                || amount_v120_horizontal.is_some()
                || amount_v120_vertical.is_some()
                || frame.stop.0
                || frame.stop.1
            {
                pointer.axis(st, frame);
                pointer.frame(st);
            }
        }
        return;
    }

    if steps.abs() < f32::EPSILON {
        return;
    }

    let steps = steps.clamp(-4.0, 4.0);
    let camera = st.camera_view_size();
    let pan_y = -camera.y * (steps / 18.0);
    {
        let mut ps = ctx.pointer_state.borrow_mut();
        ps.panning = false;
    }
    st.note_pan_activity(now);
    st.pan_camera_target(halley_core::field::Vec2 { x: 0.0, y: pan_y });
    st.note_pan_viewport_change(now);
    ctx.backend.request_redraw();
}
