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
use halley_config::keybinds::key_name_to_evdev;
use smithay::backend::input::KeyState;

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

pub(crate) fn handle_keyboard_input<B: crate::backend::interface::BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    code: u32,
    pressed: bool,
) {
    if pressed && crate::compositor::interaction::state::hide_cursor_for_typing(st) {
        ctx.backend.request_redraw();
    }

    update_mod_state(&mut ctx.mod_state.borrow_mut(), code, pressed);

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
    let matched_binding = matched_action.is_some()
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
