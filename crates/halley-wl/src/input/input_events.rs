use std::time::Instant;

use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::interaction::types::{ModState, PointerState};
use crate::spatial::screen_to_world;
use crate::state::Halley;

use super::utils::update_mod_state;
use super::key_actions::{
    apply_bound_key, apply_bound_pointer_input, apply_compositor_action_press,
    apply_compositor_action_release, compositor_binding_action, compositor_binding_action_active,
    key_is_compositor_binding,
};
use super::pointer_focus::pointer_focus_for_screen;
use halley_config::{WHEEL_DOWN_CODE, WHEEL_UP_CODE};
use smithay::backend::input::{Axis, AxisRelativeDirection, AxisSource, ButtonState, KeyState};

#[inline]
fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

/// Returns true for physical modifier keys (Super, Alt, Ctrl, Shift, Lock).
///
/// These are always forwarded to clients so clients can track modifier state
/// correctly — intercepting them would break client-side keymaps and IMEs.
///
/// All codes are XKB (evdev + 8), matching the raw codes delivered by the
/// input backend.
#[inline]
fn is_modifier_keycode(code: u32) -> bool {
    matches!(
        code,
        37        // Left Ctrl   (evdev 29 + 8)
        | 105     // Right Ctrl  (evdev 97 + 8)
        | 50      // Left Shift  (evdev 42 + 8)
        | 62      // Right Shift (evdev 54 + 8)
        | 64      // Left Alt    (evdev 56 + 8)
        | 108     // Right Alt / AltGr (evdev 100 + 8)
        | 133     // Left Super  (evdev 125 + 8)
        | 134     // Right Super (evdev 126 + 8)
        | 66      // Caps Lock   (evdev 58 + 8)
        | 77      // Num Lock    (evdev 69 + 8)
        | 78 // Scroll Lock (evdev 70 + 8)
    )
}

pub(crate) enum BackendInputEventData {
    Keyboard {
        code: u32,
        pressed: bool,
    },
    PointerMotionAbsolute {
        ws_w: i32,
        ws_h: i32,
        sx: f32,
        sy: f32,
        delta_x: f64,
        delta_y: f64,
        delta_x_unaccel: f64,
        delta_y_unaccel: f64,
        time_usec: u64,
    },
    PointerButton {
        button_code: u32,
        state: ButtonState,
    },
    PointerAxis {
        source: AxisSource,
        amount_v120_horizontal: Option<f64>,
        amount_v120_vertical: Option<f64>,
        amount_horizontal: Option<f64>,
        amount_vertical: Option<f64>,
        relative_direction_horizontal: AxisRelativeDirection,
        relative_direction_vertical: AxisRelativeDirection,
    },
}

pub(crate) fn handle_keyboard_input(
    st: &mut Halley,
    mod_state: &std::rc::Rc<std::cell::RefCell<ModState>>,
    backend: &impl BackendView,
    config_path: &str,
    wayland_display: &str,
    code: u32,
    pressed: bool,
) {
    update_mod_state(&mut mod_state.borrow_mut(), code, pressed);

    let mods = mod_state.borrow().clone();
    let is_mod_key = is_modifier_keycode(code);

    // Pure detection only. Do not execute the action yet.
    let matched_action = if pressed && !is_mod_key {
        compositor_binding_action(st, code, &mods)
    } else {
        None
    };
    let matched_binding = matched_action.is_some()
        || (pressed && !is_mod_key && key_is_compositor_binding(st, code, &mods));

    // Refresh interaction focus only for keys that are going to clients.
    // Compositor bindings like toggle-state should not first re-focus / re-heat
    // the surface they are about to collapse.
    if pressed
        && !matched_binding
        && !st.keyboard_focus_is_layer_surface()
        && let Some(fid) = st.last_input_surface_node_for_monitor(st.focused_monitor())
    {
        st.set_interaction_focus(Some(fid), 30_000, Instant::now());
    }

    // Only intercept explicit compositor bindings.
    // Releases are intercepted only if the matching press was intercepted.
    //
    // Also: execute compositor bindings only on the first physical press.
    // Ignore repeated press events while the key remains held, otherwise a
    // toggle binding like toggle-state will collapse and then immediately
    // reopen on key repeat.
    let mut first_binding_press = false;

    let intercept = if is_mod_key {
        false
    } else if pressed {
        if matched_binding {
            let mut ms = mod_state.borrow_mut();
            first_binding_press = ms.intercepted_keys.insert(code);
            if first_binding_press && let Some(action) = matched_action {
                ms.intercepted_compositor_actions.insert(code, action);
            }
            true
        } else {
            false
        }
    } else {
        let mut ms = mod_state.borrow_mut();
        let intercepted = ms.intercepted_keys.remove(&code);
        if intercepted {
            if let Some(action) = ms.intercepted_compositor_actions.remove(&code)
                && apply_compositor_action_release(st, action)
            {
                backend.request_redraw();
            }
        }
        intercepted
    };

    if let Some(keyboard) = st.seat.get_keyboard() {
        let serial = SERIAL_COUNTER.next_serial();
        let key_state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };

        keyboard.input::<(), _>(
            st,
            code.into(),
            key_state,
            serial,
            now_millis_u32(),
            |_, _, _| {
                if intercept {
                    FilterResult::Intercept(())
                } else {
                    FilterResult::Forward
                }
            },
        );
    }

    // Execute compositor action only after the filter path above,
    // and only on the first press (not repeats while held).
    if pressed
        && matched_binding
        && first_binding_press
        && apply_bound_key(st, code, &mods, config_path, wayland_display)
    {
        backend.request_redraw();
    }
}

pub(crate) fn handle_pointer_axis_input(
    st: &mut Halley,
    mod_state: &std::rc::Rc<std::cell::RefCell<ModState>>,
    pointer_state: &std::rc::Rc<std::cell::RefCell<PointerState>>,
    backend: &impl BackendView,
    config_path: &str,
    wayland_display: &str,
    source: AxisSource,
    amount_v120_horizontal: Option<f64>,
    amount_v120_vertical: Option<f64>,
    amount_horizontal: Option<f64>,
    amount_vertical: Option<f64>,
    relative_direction_horizontal: AxisRelativeDirection,
    relative_direction_vertical: AxisRelativeDirection,
) {
    if st.has_active_cluster_workspace() {
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
        let mods = mod_state.borrow().clone();
        let wheel_code = if steps > 0.0 {
            WHEEL_UP_CODE
        } else {
            WHEEL_DOWN_CODE
        };
        if let Some(action) = compositor_binding_action_active(st, wheel_code, &mods) {
            pointer_state.borrow_mut().panning = false;
            if apply_compositor_action_press(st, action, config_path, wayland_display) {
                backend.request_redraw();
            }
            return;
        }
        if apply_bound_pointer_input(st, wheel_code, &mods, config_path, wayland_display) {
            pointer_state.borrow_mut().panning = false;
            backend.request_redraw();
            return;
        }
    }

    let (sx, sy) = {
        let ps = pointer_state.borrow();
        (ps.screen.0, ps.screen.1)
    };
    // Apply the same monitor split that handle_pointer_motion_absolute uses so
    // that scroll/axis events on a second monitor compute focus and world
    // coords in that monitor's local space rather than the global layout space.
    let target_monitor = st
        .monitor_for_screen(sx, sy)
        .unwrap_or_else(|| st.interaction_monitor().to_string());
    st.set_interaction_monitor(target_monitor.as_str());
    let _ = st.activate_monitor(target_monitor.as_str());
    let (ws_w, ws_h, sx, sy) = st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
    {
        let mut ps = pointer_state.borrow_mut();
        ps.workspace_size = (ws_w, ws_h);
    }
    let world_now = screen_to_world(st, ws_w, ws_h, sx, sy);
    pointer_state.borrow_mut().world = world_now;
    let now = Instant::now();
    let resize_preview = pointer_state.borrow().resize;
    if let Some(pointer) = st.seat.get_pointer() {
        if pointer.current_focus().is_none()
            && let Some(focus) =
                pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview)
        {
            let location = if st.is_layer_surface(&focus.0) {
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
    let pan_y = camera.y * (steps / 18.0);
    {
        let mut ps = pointer_state.borrow_mut();
        ps.panning = false;
    }
    st.note_pan_activity(now);
    st.pan_camera_target(halley_core::field::Vec2 { x: 0.0, y: pan_y });
    st.note_pan_viewport_change(now);
    backend.request_redraw();
}

pub(crate) fn handle_backend_input_event(
    st: &mut Halley,
    mod_state: &std::rc::Rc<std::cell::RefCell<ModState>>,
    pointer_state: &std::rc::Rc<std::cell::RefCell<PointerState>>,
    backend: &impl BackendView,
    config_path: &str,
    wayland_display: &str,
    event: BackendInputEventData,
) {
    st.note_input_activity();

    match event {
        BackendInputEventData::Keyboard { code, pressed } => {
            handle_keyboard_input(
                st,
                mod_state,
                backend,
                config_path,
                wayland_display,
                code,
                pressed,
            );
        }
        BackendInputEventData::PointerMotionAbsolute {
            ws_w,
            ws_h,
            sx,
            sy,
            delta_x,
            delta_y,
            delta_x_unaccel,
            delta_y_unaccel,
            time_usec,
        } => {
            super::pointer_motion::handle_pointer_motion_absolute(
                st,
                backend,
                mod_state,
                pointer_state,
                ws_w,
                ws_h,
                sx,
                sy,
                (delta_x, delta_y),
                (delta_x_unaccel, delta_y_unaccel),
                time_usec,
            );
        }
        BackendInputEventData::PointerButton { button_code, state } => {
            super::pointer_button::handle_pointer_button_input(
                st,
                backend,
                mod_state,
                pointer_state,
                config_path,
                wayland_display,
                button_code,
                state,
            );
        }
        BackendInputEventData::PointerAxis {
            source,
            amount_v120_horizontal,
            amount_v120_vertical,
            amount_horizontal,
            amount_vertical,
            relative_direction_horizontal,
            relative_direction_vertical,
        } => {
            handle_pointer_axis_input(
                st,
                mod_state,
                pointer_state,
                backend,
                config_path,
                wayland_display,
                source,
                amount_v120_horizontal,
                amount_v120_vertical,
                amount_horizontal,
                amount_vertical,
                relative_direction_horizontal,
                relative_direction_vertical,
            );
        }
    }
    st.run_maintenance_if_needed(Instant::now());
}
