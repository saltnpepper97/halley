use std::time::Instant;

use eventline::{info, warn};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::AxisFrame;
use smithay::input::pointer::MotionEvent;
use smithay::utils::SERIAL_COUNTER;

use crate::backend_iface::BackendView;
use crate::interaction::types::{ModState, PointerState};
use crate::spatial::screen_to_world;
use crate::state::HalleyWlState;

use super::input_utils::{key_matches, modifier_active, update_mod_state};
use super::key_actions::{apply_bound_key, spawn_terminal};
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
#[inline]
fn is_modifier_keycode(code: u32) -> bool {
    matches!(
        code,
        29  // Left Ctrl
        | 97  // Right Ctrl
        | 42  // Left Shift
        | 54  // Right Shift
        | 56  // Left Alt
        | 100 // Right Alt / AltGr
        | 125 // Left Super / Meta
        | 126 // Right Super / Meta
        | 58  // Caps Lock
        | 69  // Num Lock
        | 70 // Scroll Lock
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

    if pressed {
        if let Some(fid) = st.last_input_surface_node() {
            st.set_interaction_focus(Some(fid), 30_000, Instant::now());
        }
    }

    // ------------------------------------------------------------------
    // Decide whether this key event should be intercepted before it
    // reaches any client.
    //
    // Rule:
    //   - Modifier keys (Super, Alt, Ctrl, Shift, Lock) are ALWAYS
    //     forwarded so clients can track modifier state correctly.
    //   - While the compositor modifier is held, all non-modifier
    //     presses are intercepted.
    //   - A release event is intercepted if and only if its matching
    //     press was intercepted, preventing stuck-key artefacts in
    //     clients.
    // ------------------------------------------------------------------
    let mods = mod_state.borrow().clone();
    let modifier_held = modifier_active(&mods, st.tuning.keybinds.modifier);
    let is_mod_key = is_modifier_keycode(code);

    let intercept = if is_mod_key {
        false
    } else if pressed {
        if modifier_held {
            mod_state.borrow_mut().intercepted_keys.insert(code);
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

    // Apply compositor bindings after the client filter so that the
    // intercept decision above is the single authoritative gate.
    // Modifier key events and releases are never compositor actions.
    if pressed && !is_mod_key {
        if apply_bound_key(st, code, &mods, config_path, wayland_display) {
            backend.request_redraw();
            return;
        }

        // Alt+Enter terminal shortcut.
        let alt_enter = key_matches(code, 28) || key_matches(code, 96);
        if mods.alt_down && alt_enter {
            if !spawn_terminal(wayland_display) {
                warn!("terminal launch keybind fired, but no terminal could be spawned");
            }
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
    if let Some(focus) = pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, now) {
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
