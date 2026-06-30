pub(crate) mod bindings;
pub(crate) mod modkeys;
pub(crate) mod spawn;

use crate::compositor::exit_confirm;
use crate::compositor::root::Halley;
use crate::compositor::screenshot;
use crate::input::ctx::InputCtx;

use std::time::Instant;

use smithay::input::keyboard::{FilterResult, KeysymHandle};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::compositor::get_parent;

use self::bindings::{
    apply_bound_key, apply_compositor_action_release, compositor_action_allows_repeat,
    compositor_binding_action, key_is_compositor_binding,
    modifiers_keep_focus_cycle_session_active,
};
use self::modkeys::{is_modifier_keycode, update_mod_state};
use halley_config::CompositorBindingAction;
use halley_config::keybinds::key_name_to_evdev;
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

fn flush_trapped_modal_release(st: &mut Halley, code: u32) {
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        st.input.interaction_state.modal_release_keys.remove(&code);
        return;
    };

    let (_, mods_changed) =
        keyboard.input_intercept::<(), _>(st, code.into(), KeyState::Released, |_, _, _| ());
    keyboard.input_forward(
        st,
        code.into(),
        KeyState::Released,
        SERIAL_COUNTER.next_serial(),
        now_millis_u32(),
        mods_changed,
    );
    st.input.interaction_state.modal_release_keys.remove(&code);
    st.input
        .interaction_state
        .forwarded_pressed_keys
        .remove(&code);
}

/// Clear any non-modifier keys we have forwarded to a client as *pressed* but not
/// yet released, by forwarding a synthetic `Released` for each. Call this right
/// before a keyboard focus change to a different surface (see
/// `crate::compositor::focus::system::set_keyboard_focus`).
///
/// This is the general, choke-point fix for the "stuck key repeats forever" bug:
/// smithay replays its `forwarded_pressed_keys` to a newly focused surface on
/// `enter`, so any key whose release was swallowed (intercepted by a modal, or
/// lost because the focused surface died) would otherwise be delivered to the
/// next window as if held, starting client-side key repeat. Emptying the set here
/// closes that window for every cause without enumerating modals/launchers.
///
/// Modifiers are intentionally preserved so a held Ctrl/Alt/Shift/Super still
/// carries across the focus change.
pub(crate) fn flush_stuck_forwarded_keys(st: &mut Halley) {
    let codes: Vec<u32> = st
        .input
        .interaction_state
        .forwarded_pressed_keys
        .iter()
        .copied()
        .filter(|code| !is_modifier_keycode(*code))
        .collect();
    if codes.is_empty() {
        return;
    }
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        for code in &codes {
            st.input
                .interaction_state
                .forwarded_pressed_keys
                .remove(code);
        }
        return;
    };
    for code in codes {
        let (_, mods_changed) =
            keyboard.input_intercept::<(), _>(st, code.into(), KeyState::Released, |_, _, _| ());
        keyboard.input_forward(
            st,
            code.into(),
            KeyState::Released,
            SERIAL_COUNTER.next_serial(),
            now_millis_u32(),
            mods_changed,
        );
        st.input
            .interaction_state
            .forwarded_pressed_keys
            .remove(&code);
        st.input.interaction_state.modal_release_keys.remove(&code);
    }
}

/// Release any key we forwarded to a client as *pressed* that is no longer physically
/// held. This is the general, choke-point guarantee against stuck client-side key repeat:
/// it runs after **every** key event (see `handle_keyboard_input`), so a release swallowed
/// by a modal, lost to a dead surface, or skipped by a deferred focus is still delivered to
/// whoever now holds focus — no per-case `trap_modal_key_release` required. Genuinely held
/// keys (still in `keys_physically_down`, modifiers included) are left untouched, so normal
/// key-holds and held Ctrl/Alt/Super are unaffected.
pub(crate) fn reconcile_forwarded_keys(st: &mut Halley) {
    let codes: Vec<u32> = st
        .input
        .interaction_state
        .forwarded_pressed_keys
        .iter()
        .copied()
        .filter(|code| {
            !st.input
                .interaction_state
                .keys_physically_down
                .contains(code)
        })
        .collect();
    if codes.is_empty() {
        return;
    }
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        for code in &codes {
            st.input
                .interaction_state
                .forwarded_pressed_keys
                .remove(code);
            st.input.interaction_state.modal_release_keys.remove(code);
        }
        return;
    };
    for code in codes {
        let (_, mods_changed) =
            keyboard.input_intercept::<(), _>(st, code.into(), KeyState::Released, |_, _, _| ());
        keyboard.input_forward(
            st,
            code.into(),
            KeyState::Released,
            SERIAL_COUNTER.next_serial(),
            now_millis_u32(),
            mods_changed,
        );
        st.input
            .interaction_state
            .forwarded_pressed_keys
            .remove(&code);
        st.input.interaction_state.modal_release_keys.remove(&code);
    }
}

#[inline]
fn cluster_mode_allows_keyboard_action(action: &CompositorBindingAction) -> bool {
    let _ = action;
    false
}

fn handle_focus_cycle_session_input<B: crate::backend::interface::BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    code: u32,
    pressed: bool,
) {
    let mods = ctx.mod_state.borrow().clone();
    let matched_action = if pressed && !is_modifier_keycode(code) {
        compositor_binding_action(st, code, &mods)
    } else {
        None
    };
    let forward_modifier_release = !pressed && is_modifier_keycode(code);

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
            |_, _, _| {
                if forward_modifier_release {
                    FilterResult::Forward
                } else {
                    FilterResult::Intercept(())
                }
            },
        );
    }

    if !pressed {
        let mut ms = ctx.mod_state.borrow_mut();
        ms.intercepted_keys.remove(&code);
        ms.intercepted_compositor_actions.remove(&code);
    }

    if pressed {
        let escape = key_name_to_evdev("escape").map(|value| value + 8);
        if Some(code) == escape {
            crate::compositor::interaction::state::trap_modal_key_release(st, code);
            if st.cancel_focus_cycle() {
                ctx.backend.request_redraw();
            }
            return;
        }

        if let Some(CompositorBindingAction::FocusCycle(direction)) = matched_action
            && st.start_or_step_focus_cycle(direction, Instant::now())
        {
            ctx.backend.request_redraw();
        }
        return;
    }

    if !modifiers_keep_focus_cycle_session_active(st, &mods)
        && st.commit_focus_cycle(Instant::now())
    {
        ctx.backend.request_redraw();
    }
}

fn cluster_prompt_input_char(keysym: &KeysymHandle<'_>) -> Option<char> {
    let ch = keysym.modified_sym().key_char()?;
    (!ch.is_control()).then_some(ch)
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

fn latch_keyboard_spawn_monitor(st: &mut Halley) {
    let monitor = st
        .model
        .focus_state
        .primary_interaction_focus
        .and_then(|node_id| st.model.monitor_state.node_monitor.get(&node_id).cloned())
        .or_else(|| keyboard_focus_monitor(st))
        .unwrap_or_else(|| st.focused_monitor().to_string());
    if st
        .model
        .monitor_state
        .monitors
        .contains_key(monitor.as_str())
    {
        st.model.spawn_state.pending_spawn_monitor = Some(monitor);
    }
}

fn keyboard_focus_monitor(st: &Halley) -> Option<String> {
    let mut focus = st.platform.seat.get_keyboard()?.current_focus()?;
    loop {
        if let Some(node_id) = st.model.surface_to_node.get(&focus.id()).copied()
            && let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id)
            && st.model.monitor_state.monitors.contains_key(monitor)
        {
            return Some(monitor.clone());
        }
        if let Some(monitor) = st
            .model
            .monitor_state
            .layer_surface_monitor
            .get(&focus.id())
            .filter(|monitor| st.model.monitor_state.monitors.contains_key(*monitor))
        {
            return Some(monitor.clone());
        }
        let parent = get_parent(&focus)?;
        focus = parent;
    }
}

fn keyboard_has_client_focus(st: &Halley) -> bool {
    st.platform
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .is_some()
}

pub(crate) fn handle_keyboard_input<B: crate::backend::interface::BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    code: u32,
    pressed: bool,
) {
    // Record raw physical key state before any modal routing, then reconcile afterwards so
    // no client is ever left holding a key whose release got swallowed — the general fix
    // for stuck key repeat, independent of which path consumed this event.
    if pressed {
        st.input.interaction_state.keys_physically_down.insert(code);
    } else {
        st.input
            .interaction_state
            .keys_physically_down
            .remove(&code);
    }
    handle_keyboard_input_inner(st, ctx, code, pressed);
    reconcile_forwarded_keys(st);
}

fn handle_keyboard_input_inner<B: crate::backend::interface::BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    code: u32,
    pressed: bool,
) {
    let exit_confirm_active = exit_confirm::active(&*st);
    update_mod_state(&mut ctx.mod_state.borrow_mut(), code, pressed);
    if !pressed
        && st
            .input
            .interaction_state
            .modal_release_keys
            .contains(&code)
    {
        // The matching press was consumed by a modal (e.g. the cluster name prompt,
        // which inserts the key into `intercepted_keys` then traps the release on
        // confirm). This early return skips the normal release cleanup, so clear the
        // key here too — otherwise it stays stuck in `intercepted_keys` and the next
        // compositor keybind on the same key is swallowed (`insert` returns false →
        // `first_binding_press` is false) until a later release frees it. That was the
        // "first mod+enter in a fresh cluster does nothing, second works" glitch.
        {
            let mut ms = ctx.mod_state.borrow_mut();
            ms.intercepted_keys.remove(&code);
            ms.intercepted_compositor_actions.remove(&code);
        }
        flush_trapped_modal_release(st, code);
        return;
    }
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
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                exit_confirm::clear(&mut *st);
                ctx.backend.request_redraw();
            } else if Some(code) == exit_return {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                exit_confirm::clear(&mut *st);
                crate::compositor::runtime::request_exit(st);
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

    if screenshot::screenshot_session_active(&*st) {
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
            let escape = key_name_to_evdev("escape").map(|value| value + 8);
            let enter = key_name_to_evdev("return").map(|value| value + 8);
            let left = key_name_to_evdev("left").map(|value| value + 8);
            let right = key_name_to_evdev("right").map(|value| value + 8);
            let menu_mode = st
                .input
                .interaction_state
                .screenshot_session
                .as_ref()
                .is_some_and(|session| session.mode == halley_api::CaptureMode::Menu);
            if Some(code) == escape {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                if menu_mode {
                    let _ = screenshot::cancel_screenshot_session(&mut *st);
                } else {
                    let _ = screenshot::return_screenshot_session_to_menu(&mut *st);
                }
                ctx.backend.request_redraw();
            } else if Some(code) == enter {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                let _ = screenshot::confirm_screenshot_session(&mut *st, Instant::now());
                ctx.backend.request_redraw();
            } else if Some(code) == left {
                if menu_mode {
                    crate::compositor::interaction::state::trap_modal_key_release(st, code);
                }
                let _ = screenshot::move_screenshot_menu_selection(&mut *st, -1);
                ctx.backend.request_redraw();
            } else if Some(code) == right {
                if menu_mode {
                    crate::compositor::interaction::state::trap_modal_key_release(st, code);
                }
                let _ = screenshot::move_screenshot_menu_selection(&mut *st, 1);
                ctx.backend.request_redraw();
            }
        }
        return;
    }

    if crate::compositor::portal_chooser::portal_chooser_active(&*st) {
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
            let escape = key_name_to_evdev("escape").map(|value| value + 8);
            let enter = key_name_to_evdev("return").map(|value| value + 8);
            let left = key_name_to_evdev("left").map(|value| value + 8);
            let right = key_name_to_evdev("right").map(|value| value + 8);
            let phase = st
                .input
                .interaction_state
                .portal_chooser
                .as_ref()
                .map(|s| s.phase);
            let in_menu =
                phase == Some(crate::compositor::portal_chooser::PortalChooserPhase::Menu);
            if Some(code) == escape {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                if in_menu {
                    let _ = crate::compositor::portal_chooser::cancel_portal_chooser(st);
                } else {
                    let _ = crate::compositor::portal_chooser::return_portal_chooser_to_menu(st);
                }
                ctx.backend.request_redraw();
            } else if Some(code) == enter && in_menu {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                let _ =
                    crate::compositor::portal_chooser::activate_portal_chooser(st, Instant::now());
                ctx.backend.request_redraw();
            } else if Some(code) == enter
                && phase == Some(crate::compositor::portal_chooser::PortalChooserPhase::ScreenPick)
            {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                let _ =
                    crate::compositor::portal_chooser::confirm_portal_screen(st, Instant::now());
                ctx.backend.request_redraw();
            } else if (Some(code) == left || Some(code) == right) && in_menu {
                let delta = if Some(code) == left { -1 } else { 1 };
                let _ = crate::compositor::portal_chooser::move_portal_chooser_selection(st, delta);
                ctx.backend.request_redraw();
            }
        }
        return;
    }

    let prompt_monitor = crate::compositor::clusters::system::active_cluster_name_prompt_monitor(
        &*st,
        st.model.monitor_state.current_monitor.as_str(),
    );
    if let Some(prompt_monitor) = prompt_monitor {
        let mut repeated_char = None;
        if let Some(keyboard) = st.platform.seat.get_keyboard() {
            let _ = keyboard.input::<(), _>(
                st,
                code.into(),
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                SERIAL_COUNTER.next_serial(),
                now_millis_u32(),
                |_, _, keysym| {
                    if pressed {
                        repeated_char = cluster_prompt_input_char(&keysym);
                    }
                    FilterResult::Intercept(())
                },
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
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                crate::compositor::clusters::system::cancel_cluster_name_prompt_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                )
            } else if Some(code) == enter {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                crate::compositor::clusters::system::confirm_cluster_name_prompt_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                    Instant::now(),
                )
            } else if !first_press && repeat_action.is_none() {
                false
            } else if Some(code) == left {
                crate::compositor::clusters::system::cluster_name_prompt_move_left_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                )
            } else if Some(code) == right {
                crate::compositor::clusters::system::cluster_name_prompt_move_right_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                )
            } else if Some(code) == backspace {
                crate::compositor::clusters::system::cluster_name_prompt_backspace_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                )
            } else if Some(code) == delete {
                crate::compositor::clusters::system::cluster_name_prompt_delete_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                )
            } else if let Some(ch) = repeated_char {
                crate::compositor::clusters::system::insert_cluster_name_prompt_char_for_monitor(
                    &mut *st,
                    prompt_monitor.as_str(),
                    ch,
                )
            } else {
                false
            };
            if handled && let Some(action) = repeat_action {
                let now_ms = st.now_ms(Instant::now());
                crate::compositor::clusters::system::start_cluster_name_prompt_repeat_for_monitor(
                    &mut *st,
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
            crate::compositor::clusters::system::stop_cluster_name_prompt_repeat_for_code(
                &mut *st, code,
            );
        }
        return;
    }

    // Apogee keyboard navigation: arrows move a highlighted selection across the window
    // mosaic and the core rail, Enter activates it, Escape closes. These keys are raw
    // (not compositor binds), so they're intercepted here before normal dispatch and
    // kept from leaking to focused clients while the overview is open.
    if st.input.interaction_state.apogee_session.is_some()
        && !crate::protocol::wayland::session_lock::session_lock_active(st)
    {
        let escape = key_name_to_evdev("escape").map(|code| code + 8);
        let enter = key_name_to_evdev("return").map(|code| code + 8);
        let left = key_name_to_evdev("left").map(|code| code + 8);
        let right = key_name_to_evdev("right").map(|code| code + 8);
        let up = key_name_to_evdev("up").map(|code| code + 8);
        let down = key_name_to_evdev("down").map(|code| code + 8);
        let dir = if Some(code) == left {
            Some(halley_config::DirectionalAction::Left)
        } else if Some(code) == right {
            Some(halley_config::DirectionalAction::Right)
        } else if Some(code) == up {
            Some(halley_config::DirectionalAction::Up)
        } else if Some(code) == down {
            Some(halley_config::DirectionalAction::Down)
        } else {
            None
        };
        if dir.is_some() || Some(code) == enter || Some(code) == escape {
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
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                let now = Instant::now();
                if let Some(dir) = dir {
                    // Navigate in global screen space (crosses monitors) and warp the
                    // cursor onto the target tile. The cursor is the single source of
                    // truth: warping it drives the same hover path the mouse uses, and
                    // updates the pointer accumulator so a later physical move continues
                    // from here — one focus, not two.
                    if let Some(target) = crate::compositor::overview::apogee_navigate(st, dir) {
                        crate::compositor::overview::apogee_reveal_tile(st, target);
                        if let Some((gsx, gsy)) =
                            crate::compositor::overview::apogee_tile_global_center(st, target)
                        {
                            crate::input::pointer::motion::handle_pointer_motion_absolute(
                                st,
                                ctx,
                                0,
                                0,
                                gsx,
                                gsy,
                                (0.0, 0.0),
                                (0.0, 0.0),
                                0,
                            );
                        }
                        // The warp above runs the apogee motion branch which
                        // reveals the cursor; re-arm the keyboard-nav hide so the
                        // cursor stays hidden while driving the overview with arrows.
                        crate::compositor::interaction::state::mark_cursor_hidden_by_keyboard_nav(
                            st,
                        );
                    }
                } else if Some(code) == enter {
                    if let Some(node) = st.input.interaction_state.apogee_hover_node {
                        crate::compositor::overview::select_apogee_target(st, node, now);
                    } else {
                        st.close_apogee(now);
                    }
                } else {
                    st.close_apogee(now);
                }
                ctx.backend.request_redraw();
            }
            return;
        }
    }

    if st.focus_cycle_session_active() {
        handle_focus_cycle_session_input(st, ctx, code, pressed);
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
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                st.exit_cluster_mode()
            } else {
                crate::compositor::interaction::state::trap_modal_key_release(st, code);
                crate::compositor::clusters::system::confirm_cluster_mode(st, Instant::now())
            };
            if handled || Some(code) == cluster_return || Some(code) == cluster_escape {
                ctx.backend.request_redraw();
            }
        }
        return;
    }

    let mods = ctx.mod_state.borrow().clone();
    let is_mod_key = is_modifier_keycode(code);
    let layer_shell_keyboard_focus =
        crate::compositor::monitor::layer_shell::keyboard_focus_is_layer_surface(st);
    let lift_keyboard_focus =
        crate::compositor::monitor::layer_shell::keyboard_focus_is_lift_layer_surface(st);
    let session_lock_active = crate::protocol::wayland::session_lock::session_lock_active(st);
    let compositor_shortcuts_blocked = layer_shell_keyboard_focus || session_lock_active;

    // NOTE: this per-case trap is now belt-and-suspenders. The general fix is
    // `flush_stuck_forwarded_keys`, run from `set_keyboard_focus` on every focus
    // change, which clears stale forwarded keys regardless of which path caused
    // them. Kept for now; safe to remove once the general path is verified.
    //
    // A layer-shell launcher (Lift) receives Enter/Escape as a *forwarded* key while it
    // holds keyboard focus. When it launches an app and destroys its surface, focus moves
    // to the new window, whose `enter` event inherits the still-forwarded key from
    // Smithay and starts client-side repeat; the physical release lands on the dead
    // launcher surface, so the window never sees a key-up and repeats forever. Trap the
    // release so the physical key-up flushes through `flush_trapped_modal_release`, which
    // forwards a synthetic Released to whatever holds focus then (the new window) and
    // clears the stale forwarded-pressed key. Persistent layer-shell clients are
    // unaffected: the synthetic release just goes back to them.
    if pressed && !is_mod_key && layer_shell_keyboard_focus {
        let trap_enter = key_name_to_evdev("return").map(|code| code + 8);
        let trap_escape = key_name_to_evdev("escape").map(|code| code + 8);
        if Some(code) == trap_enter || Some(code) == trap_escape {
            crate::compositor::interaction::state::trap_modal_key_release(st, code);
        }
    }
    let matched_action = if pressed && !is_mod_key && !compositor_shortcuts_blocked {
        compositor_binding_action(st, code, &mods)
    } else {
        None
    };
    let matched_launch = if pressed && !is_mod_key && !compositor_shortcuts_blocked {
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
        && !compositor_shortcuts_blocked
        && !keyboard_has_client_focus(st)
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
        if intercepted
            && let Some(action) = ms.intercepted_compositor_actions.remove(&code)
            && apply_compositor_action_release(st, action)
        {
            ctx.backend.request_redraw();
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

    // Mirror smithay's `forwarded_pressed_keys` for the keys we actually forward
    // to a client, so `flush_stuck_forwarded_keys` can clear stale ones on a
    // focus change. Intercepted keys are never forwarded, so they stay out.
    if !intercept {
        if pressed {
            st.input
                .interaction_state
                .forwarded_pressed_keys
                .insert(code);
        } else {
            st.input
                .interaction_state
                .forwarded_pressed_keys
                .remove(&code);
        }
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

    if pressed && matched_binding {
        let should_apply = first_binding_press
            || (repeat_binding_press
                && matched_action
                    .as_ref()
                    .is_some_and(|action: &CompositorBindingAction| {
                        compositor_action_allows_repeat(action.clone())
                    }));
        if should_apply {
            let launch_like = matched_launch.is_some()
                || matches!(matched_action, Some(CompositorBindingAction::OpenTerminal));
            if launch_like {
                latch_keyboard_spawn_monitor(st);
            }
            if apply_bound_key(st, code, &mods, ctx.config_path, ctx.wayland_display) {
                // fuzzel-style: a compositor keybind dismisses the launcher. The
                // binding has already run; now close the lift so it doesn't linger.
                if lift_keyboard_focus && first_binding_press {
                    crate::compositor::monitor::layer_shell::close_any_lift_layer(st);
                }
                if matched_action.as_ref().is_some_and(|action| {
                    matches!(
                        action,
                        CompositorBindingAction::ZoomIn
                            | CompositorBindingAction::ZoomOut
                            | CompositorBindingAction::ZoomReset
                    )
                }) {
                    ctx.backend
                        .request_output_redraw(st.model.monitor_state.current_monitor.as_str());
                } else {
                    ctx.backend.request_redraw();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_forwarded_keys_drains_only_keys_not_physically_down() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        // `stuck` was forwarded as pressed but its release was swallowed (no longer
        // physically down) — it must be drained so the client stops repeating. `held` is
        // still physically down (a genuine key-hold) — it must be left forwarded.
        let stuck = 36; // return
        let held = 45; // arbitrary letter key
        let st = &mut state.input.interaction_state;
        st.forwarded_pressed_keys.insert(stuck);
        st.forwarded_pressed_keys.insert(held);
        st.keys_physically_down.insert(held);

        reconcile_forwarded_keys(&mut state);

        let st = &state.input.interaction_state;
        assert!(!st.forwarded_pressed_keys.contains(&stuck));
        assert!(st.forwarded_pressed_keys.contains(&held));
    }

    #[test]
    fn keyboard_launch_latches_focused_monitor_over_hover_pointer_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.input.focus_mode = halley_config::InputFocusMode::Hover;
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
        let focused = state.model.field.spawn_surface(
            "focused-left",
            halley_core::field::Vec2 { x: 200.0, y: 200.0 },
            halley_core::field::Vec2 { x: 160.0, y: 120.0 },
        );
        state.assign_node_to_monitor(focused, "left");
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.set_focused_monitor("left");
        state.set_interaction_monitor("right");
        state.input.interaction_state.last_pointer_screen_global = Some((900.0, 120.0));

        latch_keyboard_spawn_monitor(&mut state);

        assert_eq!(
            state.model.spawn_state.pending_spawn_monitor.as_deref(),
            Some("left")
        );
        assert_eq!(
            state.input.interaction_state.last_pointer_screen_global,
            Some((900.0, 120.0))
        );
        assert_eq!(state.interaction_monitor(), "right");
    }
}
