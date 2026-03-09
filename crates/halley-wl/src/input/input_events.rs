use std::time::Instant;

use eventline::info;
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::AxisFrame;
use smithay::input::pointer::MotionEvent;
use smithay::utils::SERIAL_COUNTER;

use crate::backend_iface::BackendView;
use crate::interaction::types::{ModState, PointerState};
use crate::spatial::screen_to_world;
use crate::state::HalleyWlState;

use super::input_utils::update_mod_state;
use super::key_actions::{apply_bound_key, key_is_compositor_binding};
use super::pointer_focus::pointer_focus_for_screen;
use super::pointer_map_debug_enabled;
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};

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
        | 78      // Scroll Lock (evdev 70 + 8)
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
    },
    PointerButton {
        button_code: u32,
        state: ButtonState,
    },
    PointerAxis {
        amount_v120_vertical: Option<f64>,
        amount_vertical: Option<f64>,
    },
}

pub(crate) fn handle_keyboard_input(
    st: &mut HalleyWlState,
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
    let matched_binding = if pressed && !is_mod_key {
        key_is_compositor_binding(st, code, &mods)
    } else {
        false
    };

    // Refresh interaction focus only for keys that are going to clients.
    // Compositor bindings like minimize should not first re-focus / re-heat
    // the surface they are about to collapse.
    if pressed && !matched_binding {
        if let Some(fid) = st.last_input_surface_node() {
            st.set_interaction_focus(Some(fid), 30_000, Instant::now());
        }
    }

    // Only intercept explicit compositor bindings.
    // Releases are intercepted only if the matching press was intercepted.
    //
    // Also: execute compositor bindings only on the first physical press.
    // Ignore repeated press events while the key remains held, otherwise a
    // toggle binding like minimize_focused will collapse and then immediately
    // reopen on key repeat.
    let mut first_binding_press = false;

    let intercept = if is_mod_key {
        false
    } else if pressed {
        if matched_binding {
            let mut ms = mod_state.borrow_mut();
            first_binding_press = ms.intercepted_keys.insert(code);
            true
        } else {
            false
        }
    } else {
        mod_state.borrow_mut().intercepted_keys.remove(&code)
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
    if pressed && matched_binding && first_binding_press {
        if apply_bound_key(st, code, &mods, config_path, wayland_display) {
            backend.request_redraw();
        }
    }
}

pub(crate) fn handle_pointer_axis_input(
    st: &mut HalleyWlState,
    pointer_state: &std::rc::Rc<std::cell::RefCell<PointerState>>,
    backend: &impl BackendView,
    amount_v120_vertical: Option<f64>,
    amount_vertical: Option<f64>,
) {
    if st.has_active_cluster_workspace() {
        return;
    }
    let (sx, sy, ws_w, ws_h) = {
        let ps = pointer_state.borrow();
        let (ws_w, ws_h) = backend.window_size_i32();
        (ps.screen.0, ps.screen.1, ws_w, ws_h)
    };
    {
        let mut ps = pointer_state.borrow_mut();
        ps.workspace_size = (ws_w, ws_h);
    }
    let world_now = screen_to_world(st, ws_w, ws_h, sx, sy);
    pointer_state.borrow_mut().world = world_now;
    if pointer_map_debug_enabled() {
        info!(
            "ptr-map axis ws={}x{} screen=({:.2},{:.2}) world=({:.2},{:.2}) v120={:?} px={:?}",
            ws_w, ws_h, sx, sy, world_now.x, world_now.y, amount_v120_vertical, amount_vertical
        );
    }

    let now = Instant::now();
    let resize_preview = pointer_state.borrow().resize;
    if let Some(focus) = pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview) {
        if let Some(pointer) = st.seat.get_pointer() {
            pointer.motion(
                st,
                Some(focus),
                &MotionEvent {
                    location: (sx as f64, sy as f64).into(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: now_millis_u32(),
                },
            );
            let mut frame = AxisFrame::new(now_millis_u32()).source(AxisSource::Wheel);
            if let Some(v120) = amount_v120_vertical {
                frame = frame.v120(Axis::Vertical, v120.round() as i32);
            }
            let value = amount_vertical.or_else(|| amount_v120_vertical.map(|v| v / 8.0));
            if let Some(v) = value {
                frame = frame.value(Axis::Vertical, v);
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
    if steps.abs() < f32::EPSILON {
        return;
    }
    let steps = steps.clamp(-4.0, 4.0);
    pointer_state.borrow_mut().panning = false;
    let step = (steps.abs() * 80.0).max(22.0);
    st.note_pan_activity(Instant::now());
    st.viewport.pan(halley_core::field::Vec2 {
        x: 0.0,
        y: step * steps.signum(),
    });
    st.tuning.viewport_center = st.viewport.center;
    st.tuning.viewport_size = st.viewport.size;
    backend.request_redraw();
}

pub(crate) fn handle_backend_input_event(
    st: &mut HalleyWlState,
    mod_state: &std::rc::Rc<std::cell::RefCell<ModState>>,
    pointer_state: &std::rc::Rc<std::cell::RefCell<PointerState>>,
    backend: &impl BackendView,
    config_path: &str,
    wayland_display: &str,
    event: BackendInputEventData,
) {
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
        BackendInputEventData::PointerMotionAbsolute { ws_w, ws_h, sx, sy } => {
            super::pointer_motion::handle_pointer_motion_absolute(
                st,
                backend,
                mod_state,
                pointer_state,
                ws_w,
                ws_h,
                sx,
                sy,
            );
        }
        BackendInputEventData::PointerButton { button_code, state } => {
            super::pointer_button::handle_pointer_button_input(
                st,
                backend,
                mod_state,
                pointer_state,
                button_code,
                state,
            );
        }
        BackendInputEventData::PointerAxis {
            amount_v120_vertical,
            amount_vertical,
        } => {
            handle_pointer_axis_input(
                st,
                pointer_state,
                backend,
                amount_v120_vertical,
                amount_vertical,
            );
        }
    }
}
