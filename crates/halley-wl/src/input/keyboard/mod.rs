pub(crate) mod bindings;
pub(crate) mod modkeys;
pub(crate) mod spawn;

use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;

use std::time::Instant;

use smithay::input::keyboard::FilterResult;
use smithay::utils::SERIAL_COUNTER;

use self::bindings::{
    apply_bound_key, apply_compositor_action_release, compositor_action_allows_repeat,
    compositor_binding_action, key_is_compositor_binding,
};
use self::modkeys::{is_modifier_keycode, update_mod_state};
use halley_config::CompositorBindingAction;
use halley_config::keybinds::{evdev_to_key_name, key_name_to_evdev};
use smithay::backend::input::KeyState;

use crate::compositor::interaction::state::ClusterNamePromptRepeatAction;

#[inline]
fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

#[inline]
fn cluster_mode_allows_keyboard_action(action: &CompositorBindingAction) -> bool {
    matches!(
        action,
        CompositorBindingAction::ZoomIn
            | CompositorBindingAction::ZoomOut
            | CompositorBindingAction::ZoomReset
    )
}

fn cluster_prompt_input_char(
    xkb_code: u32,
    mods: &crate::compositor::interaction::ModState,
) -> Option<char> {
    let evdev = xkb_code.saturating_sub(8);
    let shifted = mods.shift_down;
    match evdev_to_key_name(evdev) {
        "A" | "B" | "C" | "D" | "E" | "F" | "G" | "H" | "I" | "J" | "K" | "L" | "M" | "N" | "O"
        | "P" | "Q" | "R" | "S" | "T" | "U" | "V" | "W" | "X" | "Y" | "Z" => {
            let ch = evdev_to_key_name(evdev)
                .chars()
                .next()
                .unwrap_or('a')
                .to_ascii_lowercase();
            Some(if shifted { ch.to_ascii_uppercase() } else { ch })
        }
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "0" => {
            evdev_to_key_name(evdev).chars().next()
        }
        "Minus" => Some(if shifted { '_' } else { '-' }),
        "Equal" => Some(if shifted { '+' } else { '=' }),
        "[" => Some(if shifted { '{' } else { '[' }),
        "]" => Some(if shifted { '}' } else { ']' }),
        ";" => Some(if shifted { ':' } else { ';' }),
        "'" => Some(if shifted { '"' } else { '\'' }),
        "`" => Some(if shifted { '~' } else { '`' }),
        "\\" => Some(if shifted { '|' } else { '\\' }),
        "Comma" => Some(if shifted { '<' } else { ',' }),
        "Period" => Some(if shifted { '>' } else { '.' }),
        "Slash" => Some(if shifted { '?' } else { '/' }),
        "Space" => Some(' '),
        _ => None,
    }
}

fn log_keyboard_binding_resolution(
    st: &Halley,
    code: u32,
    pressed: bool,
    mods: &crate::compositor::interaction::ModState,
    matched_action: Option<&CompositorBindingAction>,
    matched_launch: Option<&str>,
    matched_binding: bool,
    intercept: bool,
) {
    let _ = (
        st,
        code,
        pressed,
        mods,
        matched_action,
        matched_launch,
        matched_binding,
        intercept,
    );
}

pub(crate) fn handle_keyboard_input<B: crate::backend::interface::BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    code: u32,
    pressed: bool,
) {
    let exit_confirm_active = st.exit_confirm_active();
    update_mod_state(&mut ctx.mod_state.borrow_mut(), code, pressed);
    let exit_escape = key_name_to_evdev("escape").map(|code| code + 8);
    let exit_return = key_name_to_evdev("return").map(|code| code + 8);
    if exit_confirm_active {
        if let Some(keyboard) = st.platform.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                st,
                code.into(),
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                serial,
                now_millis_u32(),
                |_, _, _| FilterResult::Intercept(()),
            );
        }
        if pressed {
            if Some(code) == exit_escape {
                st.clear_exit_confirm_overlay();
                ctx.backend.request_redraw();
            } else if Some(code) == exit_return {
                st.clear_exit_confirm_overlay();
                st.request_exit();
                ctx.backend.request_redraw();
            }
        }
        return;
    }

    if pressed {
        let now_ms = st.now_ms(Instant::now());
        if crate::compositor::interaction::state::note_typing_activity(st, now_ms) {
            ctx.backend.request_redraw();
        }
    }

    let prompt_monitor = st.model.monitor_state.current_monitor.clone();
    if crate::compositor::clusters::system::cluster_system_controller(&*st)
        .cluster_name_prompt_active_for_monitor(prompt_monitor.as_str())
    {
        if let Some(keyboard) = st.platform.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                st,
                code.into(),
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                serial,
                now_millis_u32(),
                |_, _, _| FilterResult::Intercept(()),
            );
        }
        if pressed {
            let first_press = ctx.mod_state.borrow_mut().intercepted_keys.insert(code);
            let left = key_name_to_evdev("left").map(|value| value + 8);
            let right = key_name_to_evdev("right").map(|value| value + 8);
            let delete = key_name_to_evdev("delete").map(|value| value + 8);
            let backspace = key_name_to_evdev("backspace").map(|value| value + 8);
            let escape = key_name_to_evdev("escape").map(|value| value + 8);
            let enter = key_name_to_evdev("return").map(|value| value + 8);
            let repeated_char = cluster_prompt_input_char(code, &ctx.mod_state.borrow());
            let repeat_action = if Some(code) == left {
                Some(ClusterNamePromptRepeatAction::MoveLeft)
            } else if Some(code) == right {
                Some(ClusterNamePromptRepeatAction::MoveRight)
            } else if Some(code) == delete {
                Some(ClusterNamePromptRepeatAction::Delete)
            } else if Some(code) == backspace {
                Some(ClusterNamePromptRepeatAction::Backspace)
            } else {
                repeated_char.map(ClusterNamePromptRepeatAction::Insert)
            };
            let handled = if Some(code) == escape {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .cancel_cluster_name_prompt_for_monitor(prompt_monitor.as_str())
            } else if Some(code) == enter {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .confirm_cluster_name_prompt_for_monitor(
                        prompt_monitor.as_str(),
                        Instant::now(),
                    )
            } else if !first_press && repeat_action.is_none() {
                false
            } else if Some(code) == left {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .cluster_name_prompt_move_left_for_monitor(prompt_monitor.as_str())
            } else if Some(code) == right {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .cluster_name_prompt_move_right_for_monitor(prompt_monitor.as_str())
            } else if Some(code) == backspace {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .cluster_name_prompt_backspace_for_monitor(prompt_monitor.as_str())
            } else if Some(code) == delete {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .cluster_name_prompt_delete_for_monitor(prompt_monitor.as_str())
            } else if let Some(ch) = repeated_char {
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .insert_cluster_name_prompt_char_for_monitor(prompt_monitor.as_str(), ch)
            } else {
                false
            };
            if handled && let Some(action) = repeat_action {
                let now_ms = st.now_ms(Instant::now());
                crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                    .start_cluster_name_prompt_repeat_for_monitor(
                        prompt_monitor.as_str(),
                        code,
                        action,
                        now_ms,
                    );
            }
            if handled {
                ctx.backend.request_redraw();
            }
        } else {
            ctx.mod_state.borrow_mut().intercepted_keys.remove(&code);
            crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                .stop_cluster_name_prompt_repeat_for_code(code);
        }
        return;
    }

    let cluster_escape = key_name_to_evdev("escape").map(|code| code + 8);
    let cluster_return = key_name_to_evdev("return").map(|code| code + 8);
    if st.cluster_mode_active() && (Some(code) == cluster_escape || Some(code) == cluster_return) {
        if let Some(keyboard) = st.platform.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                st,
                code.into(),
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                serial,
                now_millis_u32(),
                |_, _, _| FilterResult::Intercept(()),
            );
        }
        if pressed {
            let handled = if Some(code) == cluster_escape {
                st.exit_cluster_mode()
            } else {
                st.confirm_cluster_mode(Instant::now())
            };
            if handled || Some(code) == cluster_return || Some(code) == cluster_escape {
                ctx.backend.request_redraw();
            }
        }
        return;
    }

    let mods = ctx.mod_state.borrow().clone();
    let is_mod_key = is_modifier_keycode(code);
    let matched_action = if pressed && !is_mod_key {
        compositor_binding_action(st, code, &mods)
    } else {
        None
    };
    let matched_launch = if pressed && !is_mod_key {
        st.runtime
            .tuning
            .launch_bindings
            .iter()
            .find(|binding| {
                bindings::input_matches_binding(code, binding.key)
                    && self::modkeys::modifier_exact(&mods, binding.modifiers)
            })
            .map(|binding| binding.command.clone())
    } else {
        None
    };
    let matched_binding = matched_action.is_some()
        || matched_launch.is_some()
        || (pressed && !is_mod_key && key_is_compositor_binding(st, code, &mods));
    let cluster_blocks_key = st.cluster_mode_active() && !is_mod_key;
    let cluster_allowed_action = matched_action
        .as_ref()
        .is_some_and(cluster_mode_allows_keyboard_action);

    if pressed
        && !is_mod_key
        && !matched_binding
        && !cluster_blocks_key
        && !crate::compositor::monitor::layer_shell::keyboard_focus_is_layer_surface(st)
        && let Some(fid) = st.last_input_surface_node_for_monitor(st.focused_monitor())
    {
        let open_monitors = st
            .model
            .cluster_state
            .cluster_bloom_open
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for monitor in open_monitors {
            let open_core = st
                .cluster_bloom_for_monitor(monitor.as_str())
                .and_then(|cid| st.model.field.cluster(cid).and_then(|cluster| cluster.core));
            if open_core != Some(fid) {
                let _ = st.close_cluster_bloom_for_monitor(monitor.as_str());
            }
        }
        st.set_interaction_focus(Some(fid), 30_000, Instant::now());
    }

    let mut first_binding_press = false;
    let mut repeat_binding_press = false;

    let intercept = if is_mod_key {
        false
    } else if cluster_blocks_key {
        if pressed && cluster_allowed_action {
            let mut ms = ctx.mod_state.borrow_mut();
            first_binding_press = ms.intercepted_keys.insert(code);
            repeat_binding_press = !first_binding_press;
            if first_binding_press && let Some(action) = matched_action.clone() {
                ms.intercepted_compositor_actions.insert(code, action);
            }
        } else if !pressed {
            let mut ms = ctx.mod_state.borrow_mut();
            let intercepted = ms.intercepted_keys.remove(&code);
            if intercepted {
                if let Some(action) = ms.intercepted_compositor_actions.remove(&code)
                    && apply_compositor_action_release(st, action)
                {
                    ctx.backend.request_redraw();
                }
            } else {
                ms.intercepted_compositor_actions.remove(&code);
            }
        }
        true
    } else if pressed {
        if matched_binding {
            let mut ms = ctx.mod_state.borrow_mut();
            first_binding_press = ms.intercepted_keys.insert(code);
            repeat_binding_press = !first_binding_press;
            if first_binding_press && let Some(action) = matched_action.clone() {
                ms.intercepted_compositor_actions.insert(code, action);
            }
            true
        } else {
            false
        }
    } else {
        let mut ms = ctx.mod_state.borrow_mut();
        let intercepted = ms.intercepted_keys.remove(&code);
        if intercepted {
            if let Some(action) = ms.intercepted_compositor_actions.remove(&code)
                && apply_compositor_action_release(st, action)
            {
                ctx.backend.request_redraw();
            }
        }
        intercepted
    };

    if let Some(keyboard) = st.platform.seat.get_keyboard() {
        let now = Instant::now();
        let serial = SERIAL_COUNTER.next_serial();
        crate::protocol::wayland::activation::note_input_serial(st, serial, st.now_ms(now));
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

    log_keyboard_binding_resolution(
        st,
        code,
        pressed,
        &mods,
        matched_action.as_ref(),
        matched_launch.as_deref(),
        matched_binding,
        intercept,
    );

    if pressed
        && matched_binding
        && (first_binding_press
            || (repeat_binding_press
                && matched_action
                    .as_ref()
                    .is_some_and(|action| compositor_action_allows_repeat(action.clone()))))
        && apply_bound_key(st, code, &mods, ctx.config_path, ctx.wayland_display)
    {
        ctx.backend.request_redraw();
    }
}
