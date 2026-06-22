use std::time::Instant;

use smithay::input::pointer::{AxisFrame, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::compositor::exit_confirm;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::compositor::screenshot::screenshot_controller;
use crate::input::ctx::InputCtx;

use super::context::pointer_screen_context_for_monitor;
use super::focus::pointer_focus_for_screen;
use crate::input::keyboard::bindings::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};
use crate::input::keyboard::modkeys::modifier_active;
use halley_config::{
    CompositorBindingAction, GestureScrollPanMode, WHEEL_DOWN_CODE, WHEEL_UP_CODE,
};
use smithay::backend::input::{Axis, AxisRelativeDirection, AxisSource};

const TOUCHPAD_DISCRETE_STEP_PX: f32 = 96.0;

#[inline]
fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

fn axis_scroll_delta(amount_v120: Option<f64>, amount_px: Option<f64>) -> i32 {
    let raw = if let Some(v120) = amount_v120 {
        -(v120 / 120.0) * 48.0
    } else {
        -amount_px.unwrap_or(0.0)
    };
    if raw.abs() < 1.0 {
        0
    } else {
        raw.round() as i32
    }
}

fn axis_logical_value(amount_v120: Option<f64>, amount_px: Option<f64>) -> f32 {
    amount_px
        .or_else(|| amount_v120.map(|v| v / 8.0))
        .unwrap_or(0.0) as f32
}

fn axis_stopped(amount_v120: Option<f64>, amount_px: Option<f64>) -> bool {
    amount_px.unwrap_or(0.0).abs() < f64::EPSILON && amount_v120.unwrap_or(0.0).abs() < f64::EPSILON
}

fn wheel_steps_for_axis(amount_v120: Option<f64>, amount_px: Option<f64>) -> f32 {
    let mut steps = (amount_v120.unwrap_or(0.0) as f32) / 120.0;
    if steps.abs() < f32::EPSILON {
        let px = amount_px.unwrap_or(0.0) as f32;
        if px.abs() > f32::EPSILON {
            steps = px / 40.0;
        }
    }
    steps
}

fn touchpad_step_delta(amount_v120: Option<f64>, amount_px: Option<f64>) -> f32 {
    if let Some(v120) = amount_v120 {
        (v120 as f32) / 120.0
    } else {
        (amount_px.unwrap_or(0.0) as f32) / TOUCHPAD_DISCRETE_STEP_PX
    }
}

fn take_touchpad_discrete_step(accum: &mut f32, delta_steps: f32, stopped: bool) -> i32 {
    if stopped {
        *accum = 0.0;
        return 0;
    }
    if delta_steps.abs() < f32::EPSILON {
        return 0;
    }
    if accum.abs() > f32::EPSILON && accum.signum() != delta_steps.signum() {
        *accum = 0.0;
    }
    *accum += delta_steps;
    if *accum >= 1.0 {
        *accum -= 1.0;
        1
    } else if *accum <= -1.0 {
        *accum += 1.0;
        -1
    } else {
        0
    }
}

fn touchpad_pan_delta(
    amount_v120_horizontal: Option<f64>,
    amount_v120_vertical: Option<f64>,
    amount_horizontal: Option<f64>,
    amount_vertical: Option<f64>,
    camera: halley_core::field::Vec2,
    ws_w: i32,
    ws_h: i32,
) -> halley_core::field::Vec2 {
    let dx_px = axis_logical_value(amount_v120_horizontal, amount_horizontal);
    let dy_px = axis_logical_value(amount_v120_vertical, amount_vertical);
    halley_core::field::Vec2 {
        x: -dx_px * camera.x.max(1.0) / (ws_w as f32).max(1.0),
        y: -dy_px * camera.y.max(1.0) / (ws_h as f32).max(1.0),
    }
}

fn handle_touchpad_scroll_pan<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    context: &super::context::PointerScreenContext,
    now: Instant,
    amount_v120_horizontal: Option<f64>,
    amount_v120_vertical: Option<f64>,
    amount_horizontal: Option<f64>,
    amount_vertical: Option<f64>,
) -> bool {
    let camera = camera_controller(&*st).view_size();
    let pan = touchpad_pan_delta(
        amount_v120_horizontal,
        amount_v120_vertical,
        amount_horizontal,
        amount_vertical,
        camera,
        context.ws_w,
        context.ws_h,
    );
    if pan.x.abs() < f32::EPSILON && pan.y.abs() < f32::EPSILON {
        return true;
    }
    if camera_controller(&*st).pan_blocked_on_monitor(context.monitor.as_str()) {
        return true;
    }

    ctx.pointer_state.borrow_mut().panning = false;
    st.note_pan_activity(now);
    camera_controller(&mut *st).pan_target(pan);
    st.note_pan_viewport_change(now);
    ctx.backend.request_output_redraw(context.monitor.as_str());
    true
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
    if exit_confirm::active(&*st) {
        return;
    }
    if screenshot_controller(&mut *st).screenshot_session_active() {
        return;
    }
    if crate::compositor::portal_chooser::portal_chooser_active(&*st) {
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
        let target_monitor = st.monitor_for_screen_or_interaction(sx, sy);
        st.activate_monitor(target_monitor.as_str());
        let context = pointer_screen_context_for_monitor(st, target_monitor, (sx, sy), true, true);
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.workspace_size = (context.ws_w, context.ws_h);
            ps.world = context.world;
        }
        if let Some(pointer) = st.platform.seat.get_pointer() {
            if pointer.current_focus().is_none()
                && let Some(focus) = pointer_focus_for_screen(
                    st,
                    context.ws_w,
                    context.ws_h,
                    context.local_sx,
                    context.local_sy,
                    Instant::now(),
                    None,
                )
            {
                pointer.motion(
                    st,
                    Some(focus),
                    &MotionEvent {
                        location: (context.local_sx as f64, context.local_sy as f64).into(),
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

    let (sx, sy) = {
        let ps = ctx.pointer_state.borrow();
        (ps.screen.0, ps.screen.1)
    };
    if st.input.interaction_state.apogee_session.is_some() {
        let target_monitor = st.monitor_for_screen_or_interaction(sx, sy);
        let (_, local_h, _local_sx, local_sy) =
            st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
        let region = crate::compositor::overview::apogee_region_for_point(local_h, local_sy);
        let delta = amount_vertical
            .or_else(|| amount_v120_vertical.map(|v| v / 8.0))
            .unwrap_or(0.0) as f32
            * 48.0;
        if delta.abs() > f32::EPSILON
            && st.adjust_apogee_orbit(target_monitor.as_str(), delta, region)
        {
            ctx.backend.request_redraw();
        }
        return;
    }
    let target_monitor = st.monitor_for_screen_or_interaction(sx, sy);
    st.activate_monitor(target_monitor.as_str());
    let context = pointer_screen_context_for_monitor(st, target_monitor, (sx, sy), true, true);
    {
        let mut ps = ctx.pointer_state.borrow_mut();
        ps.workspace_size = (context.ws_w, context.ws_h);
        ps.world = context.world;
    }
    let now = Instant::now();
    let now_ms = st.now_ms(now);

    let touchpad_scroll = source == AxisSource::Finger;
    let vertical_stopped = axis_stopped(amount_v120_vertical, amount_vertical);
    let steps = if touchpad_scroll {
        let delta_steps = touchpad_step_delta(amount_v120_vertical, amount_vertical);
        let mut ps = ctx.pointer_state.borrow_mut();
        take_touchpad_discrete_step(
            &mut ps.touchpad_scroll_step_accum_y,
            delta_steps,
            vertical_stopped,
        ) as f32
    } else {
        wheel_steps_for_axis(amount_v120_vertical, amount_vertical)
    };
    let mut scroll_dx = axis_scroll_delta(amount_v120_horizontal, amount_horizontal);
    let mut scroll_dy = -axis_scroll_delta(amount_v120_vertical, amount_vertical);
    let mods = ctx.mod_state.borrow().clone();
    if mods.shift_down && scroll_dx == 0 && scroll_dy != 0 {
        scroll_dx = scroll_dy;
        scroll_dy = 0;
    }
    if (scroll_dx != 0 || scroll_dy != 0)
        && crate::overlay::error_toast_hit_test(
            st,
            context.monitor.as_str(),
            context.ws_w,
            context.ws_h,
            context.local_sx as f64,
            context.local_sy as f64,
        )
    {
        st.ui
            .render_state
            .set_overlay_error_toast_hovered(context.monitor.as_str(), true, now_ms);
        let changed = crate::overlay::scroll_error_toast(
            st,
            context.monitor.as_str(),
            context.ws_w,
            context.ws_h,
            scroll_dx,
            scroll_dy,
        );
        if changed {
            ctx.backend.request_redraw();
        }
        return;
    }
    if touchpad_scroll
        && st.runtime.tuning.input.gestures.scroll_pan != GestureScrollPanMode::Off
        && modifier_active(&mods, st.runtime.tuning.input.gestures.modifier)
        && crate::compositor::interaction::pointer::active_constrained_pointer_surface(st).is_none()
        && handle_touchpad_scroll_pan(
            st,
            ctx,
            &context,
            now,
            amount_v120_horizontal,
            amount_v120_vertical,
            amount_horizontal,
            amount_vertical,
        )
    {
        return;
    }
    if steps.abs() >= f32::EPSILON {
        let wheel_code = if steps > 0.0 {
            WHEEL_UP_CODE
        } else {
            WHEEL_DOWN_CODE
        };
        if let Some(action) = compositor_binding_action_active(st, wheel_code, &mods) {
            ctx.pointer_state.borrow_mut().panning = false;
            let zoom_action = matches!(
                action,
                CompositorBindingAction::ZoomIn | CompositorBindingAction::ZoomOut
            );
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
                if zoom_action {
                    ctx.backend.request_output_redraw(context.monitor.as_str());
                } else {
                    ctx.backend.request_redraw();
                }
            }
            return;
        }
        if apply_bound_pointer_input(st, wheel_code, &mods, ctx.config_path, ctx.wayland_display) {
            ctx.pointer_state.borrow_mut().panning = false;
            ctx.backend.request_redraw();
            return;
        }
    }
    if steps.abs() >= f32::EPSILON {
        let overlay = crate::overlay::OverlayView::from_halley(st);
        let overflow_scrollable = overlay
            .cluster_overflow_member_ids_for_monitor(context.monitor.as_str())
            .len()
            > 15;
        let over_overflow_strip = overflow_scrollable
            && crate::overlay::cluster_overflow_strip_slot_at(
                &overlay,
                context.monitor.as_str(),
                context.local_sx,
                context.local_sy,
                now_ms,
            )
            .is_some();
        if over_overflow_strip {
            let delta = if steps > 0.0 { 1 } else { -1 };
            let changed =
                st.adjust_cluster_overflow_scroll_for_monitor(context.monitor.as_str(), delta);
            st.reveal_cluster_overflow_for_monitor(context.monitor.as_str(), now_ms);
            if changed {
                ctx.backend.request_redraw();
            }
            return;
        }
    }
    let resize_preview = ctx.pointer_state.borrow().resize;
    if let Some(pointer) = st.platform.seat.get_pointer() {
        let constrained_surface_info =
            crate::compositor::interaction::pointer::active_constrained_pointer_surface(st);
        let locked_surface = constrained_surface_info
            .as_ref()
            .and_then(|(s, locked)| if *locked { Some(s.clone()) } else { None });
        let mut target_focus = pointer_focus_for_screen(
            st,
            context.ws_w,
            context.ws_h,
            context.local_sx,
            context.local_sy,
            now,
            resize_preview,
        );
        if let Some(focus) = target_focus.as_mut()
            && let Some(constrained_focus) =
                crate::compositor::interaction::pointer::constrained_focus_in_hierarchy(st, focus)
            && constrained_focus.0 != focus.0
        {
            *focus = constrained_focus;
        }

        let scroll_pan_mode = st.runtime.tuning.input.gestures.scroll_pan;
        let pan_empty_field = scroll_pan_mode == GestureScrollPanMode::EmptyField
            && target_focus.is_none()
            && constrained_surface_info.is_none();
        let pan_with_modifier = scroll_pan_mode != GestureScrollPanMode::Off
            && modifier_active(&mods, st.runtime.tuning.input.gestures.modifier)
            && constrained_surface_info.is_none();
        if touchpad_scroll
            && (pan_empty_field || pan_with_modifier)
            && handle_touchpad_scroll_pan(
                st,
                ctx,
                &context,
                now,
                amount_v120_horizontal,
                amount_v120_vertical,
                amount_horizontal,
                amount_vertical,
            )
        {
            return;
        }

        if pointer.current_focus().is_none() {
            if let Some(focus) = target_focus {
                if locked_surface.is_none() {
                    let location = if crate::compositor::monitor::layer_shell::is_layer_surface_tree(
                        st, &focus.0,
                    )
                        || crate::protocol::wayland::session_lock::is_session_lock_surface(
                            st, &focus.0,
                        ) {
                        (context.local_sx as f64, context.local_sy as f64).into()
                    } else {
                        let cam_scale = st.camera_render_scale() as f64;
                        (
                            context.local_sx as f64 / cam_scale,
                            context.local_sy as f64 / cam_scale,
                        )
                            .into()
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
                } else {
                    // Locked, just set the focus without motion events if possible
                    // Smithay usually needs motion() to set focus.
                    pointer.motion(
                        st,
                        Some((locked_surface.unwrap(), pointer.current_location())),
                        &MotionEvent {
                            location: pointer.current_location(),
                            serial: SERIAL_COUNTER.next_serial(),
                            time: now_millis_u32(),
                        },
                    );
                }
            }
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

    if touchpad_scroll {
        let camera = camera_controller(&*st).view_size();
        let pan = touchpad_pan_delta(
            amount_v120_horizontal,
            amount_v120_vertical,
            amount_horizontal,
            amount_vertical,
            camera,
            context.ws_w,
            context.ws_h,
        );
        if pan.x.abs() < f32::EPSILON && pan.y.abs() < f32::EPSILON {
            return;
        }
        if camera_controller(&*st).pan_blocked_on_monitor(context.monitor.as_str()) {
            return;
        }
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.panning = false;
        }
        st.note_pan_activity(now);
        camera_controller(&mut *st).pan_target(pan);
        st.note_pan_viewport_change(now);
        ctx.backend.request_output_redraw(context.monitor.as_str());
        return;
    }

    if steps.abs() < f32::EPSILON {
        return;
    }

    if camera_controller(&*st)
        .pan_blocked_on_monitor(st.model.monitor_state.current_monitor.as_str())
    {
        return;
    }

    let steps = steps.clamp(-4.0, 4.0);
    let camera = camera_controller(&*st).view_size();
    let pan_y = -camera.y * (steps / 18.0);
    {
        let mut ps = ctx.pointer_state.borrow_mut();
        ps.panning = false;
    }
    st.note_pan_activity(now);
    camera_controller(&mut *st).pan_target(halley_core::field::Vec2 { x: 0.0, y: pan_y });
    st.note_pan_viewport_change(now);
    ctx.backend.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;

    #[test]
    fn touchpad_discrete_steps_accumulate_until_threshold() {
        let mut accum = 0.0;

        assert_eq!(take_touchpad_discrete_step(&mut accum, 0.25, false), 0);
        assert_eq!(take_touchpad_discrete_step(&mut accum, 0.25, false), 0);
        assert_eq!(take_touchpad_discrete_step(&mut accum, 0.25, false), 0);
        assert_eq!(take_touchpad_discrete_step(&mut accum, 0.25, false), 1);
        assert!(accum.abs() <= f32::EPSILON);
    }

    #[test]
    fn touchpad_discrete_steps_reset_on_stop_and_direction_change() {
        let mut accum = 0.75;

        assert_eq!(take_touchpad_discrete_step(&mut accum, 0.0, true), 0);
        assert_eq!(accum, 0.0);

        assert_eq!(take_touchpad_discrete_step(&mut accum, 0.75, false), 0);
        assert_eq!(take_touchpad_discrete_step(&mut accum, -0.5, false), 0);
        assert_eq!(accum, -0.5);
        assert_eq!(take_touchpad_discrete_step(&mut accum, -0.5, false), -1);
    }

    #[test]
    fn touchpad_step_delta_uses_v120_or_pixel_threshold() {
        assert_eq!(touchpad_step_delta(Some(120.0), Some(1.0)), 1.0);
        assert!((touchpad_step_delta(None, Some(24.0)) - 0.25).abs() <= f32::EPSILON);
    }

    #[test]
    fn touchpad_pan_delta_scales_pixels_to_world_space() {
        let pan = touchpad_pan_delta(
            None,
            None,
            Some(80.0),
            Some(-60.0),
            Vec2 {
                x: 1600.0,
                y: 1200.0,
            },
            800,
            600,
        );

        assert_eq!(pan.x, -160.0);
        assert_eq!(pan.y, 120.0);
    }
}
