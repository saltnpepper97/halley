use super::*;

enum KeyboardOutcome {
    Action(crate::input::keybinds::ResolvedBind),
    ExitConfirm,
    ExitCancel,
    ExitIntercept,
    ClusterDeleteConfirm,
    ClusterDeleteCancel,
    ClusterDeleteIntercept,
    AccessibilityIntercept,
    ApogeeCancel,
    ApogeeAccept,
    ApogeeMove(crate::shell::apogee::Direction),
    ApogeeIntercept,
    FocusCycleCancel,
    FocusCycleIntercept,
    CaptureAccept,
    CaptureCancel,
    CapturePrevious,
    CaptureNext,
    CaptureIntercept,
    BearingsRelease,
    ClusterAccept,
    ClusterCancel,
    ClusterBackspace,
    ClusterDelete,
    ClusterMoveLeft,
    ClusterMoveRight,
    ClusterComposerToggle,
    ClusterComposerMove(crate::shell::cluster_composer::Direction),
    ClusterCharacter(char),
    ClusterIntercept,
    BasicsDismiss,
    BasicsIntercept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModalKeyRouting {
    Evaluate,
    RetireUnfocusedRelease,
    SuppressRelease,
}

pub(super) fn modal_key_routing(
    modal_active: bool,
    state: KeyState,
    release_is_suppressed: bool,
) -> ModalKeyRouting {
    if state == KeyState::Released && release_is_suppressed {
        ModalKeyRouting::SuppressRelease
    } else if modal_active && state == KeyState::Released {
        ModalKeyRouting::RetireUnfocusedRelease
    } else {
        ModalKeyRouting::Evaluate
    }
}

pub(super) fn handle<D, B>(
    session: &mut Session<D>,
    key_event: &B::KeyboardKeyEvent,
    socket_name: &OsStr,
) where
    D: SessionDriver,
    B: InputBackend,
{
    session.interactions.wheel_accumulator.reset_all();
    let keycode = key_event.key_code();
    let state = key_event.state();
    session.keyboard.side_modifiers.update(keycode, state);
    let time = key_event.time_msec();
    if state == KeyState::Released {
        session.key_repeat.release(keycode);
        session.clusters.stop_name_repeat(keycode.raw());
    } else {
        session.key_repeat.cancel();
    }
    if state == KeyState::Pressed && session.cursor_policy.keyboard_press() {
        session.request_redraw();
    }
    let release_is_suppressed = state == KeyState::Released
        && session
            .interactions
            .suppressed_keys
            .release_is_suppressed(keycode);
    let accessibility = if session.shell.overlays.confirmation_modal_active() {
        crate::accessibility::KeyboardDisposition::Pass
    } else {
        crate::accessibility::process_key(
            session,
            std::time::Duration::from_millis(u64::from(time)),
            keycode,
            state,
        )
    };
    let keyboard = session
        .seat
        .get_keyboard()
        .expect("keyboard capability added at seat setup");
    let bindings_enabled = bindings_enabled(session);
    let mut forwarded_non_modifier_press = false;
    let action = keyboard.input::<KeyboardOutcome, _>(
        session,
        keycode,
        state,
        SERIAL_COUNTER.next_serial(),
        time,
        |data, modifiers, handle| {
            if data.shell.overlays.exit_modal_active() {
                if state == KeyState::Released {
                    return if release_is_suppressed {
                        FilterResult::Intercept(KeyboardOutcome::ExitIntercept)
                    } else {
                        FilterResult::Forward
                    };
                }
                return match handle.raw_latin_sym_or_raw_current_sym() {
                    Some(Keysym::Return | Keysym::KP_Enter) => {
                        FilterResult::Intercept(KeyboardOutcome::ExitConfirm)
                    }
                    Some(Keysym::Escape) => FilterResult::Intercept(KeyboardOutcome::ExitCancel),
                    _ => FilterResult::Intercept(KeyboardOutcome::ExitIntercept),
                };
            }
            if data.shell.overlays.cluster_delete_modal_active() {
                if state == KeyState::Released {
                    return if release_is_suppressed {
                        FilterResult::Intercept(KeyboardOutcome::ClusterDeleteIntercept)
                    } else {
                        FilterResult::Forward
                    };
                }
                return match handle.raw_latin_sym_or_raw_current_sym() {
                    Some(Keysym::Return | Keysym::KP_Enter) => {
                        FilterResult::Intercept(KeyboardOutcome::ClusterDeleteConfirm)
                    }
                    Some(Keysym::Escape) => {
                        FilterResult::Intercept(KeyboardOutcome::ClusterDeleteCancel)
                    }
                    _ => FilterResult::Intercept(KeyboardOutcome::ClusterDeleteIntercept),
                };
            }
            if data.shell.overlays.basics_card_accepts_input() {
                if state == KeyState::Released {
                    return if release_is_suppressed {
                        FilterResult::Intercept(KeyboardOutcome::BasicsIntercept)
                    } else {
                        FilterResult::Forward
                    };
                }
                // The card is a one-time primer, not a modal: only its own
                // dismissal keys are swallowed, everything else keeps working.
                return match handle.raw_latin_sym_or_raw_current_sym() {
                    Some(Keysym::Return | Keysym::KP_Enter | Keysym::Escape) => {
                        FilterResult::Intercept(KeyboardOutcome::BasicsDismiss)
                    }
                    _ => FilterResult::Forward,
                };
            }
            if data.clusters.accepts_modal_input() {
                if state == KeyState::Released {
                    return if release_is_suppressed {
                        FilterResult::Intercept(KeyboardOutcome::ClusterIntercept)
                    } else {
                        FilterResult::Forward
                    };
                }
                if data.shell.cluster_composer.is_active()
                    && !data.shell.cluster_composer.accepts_input()
                {
                    return FilterResult::Intercept(KeyboardOutcome::ClusterIntercept);
                }
                let sym = handle.modified_sym();
                let composer_selection = data.shell.cluster_composer.accepts_input()
                    && data
                        .clusters
                        .creation()
                        .is_some_and(|creation| !creation.naming);
                let outcome = if composer_selection {
                    match sym {
                        Keysym::Escape => KeyboardOutcome::ClusterCancel,
                        Keysym::Return | Keysym::KP_Enter => KeyboardOutcome::ClusterAccept,
                        Keysym::space => KeyboardOutcome::ClusterComposerToggle,
                        Keysym::Left => KeyboardOutcome::ClusterComposerMove(
                            crate::shell::cluster_composer::Direction::Left,
                        ),
                        Keysym::Right => KeyboardOutcome::ClusterComposerMove(
                            crate::shell::cluster_composer::Direction::Right,
                        ),
                        Keysym::Up => KeyboardOutcome::ClusterComposerMove(
                            crate::shell::cluster_composer::Direction::Up,
                        ),
                        Keysym::Down => KeyboardOutcome::ClusterComposerMove(
                            crate::shell::cluster_composer::Direction::Down,
                        ),
                        _ => KeyboardOutcome::ClusterIntercept,
                    }
                } else {
                    match sym {
                        Keysym::Escape => KeyboardOutcome::ClusterCancel,
                        Keysym::Return | Keysym::KP_Enter => KeyboardOutcome::ClusterAccept,
                        Keysym::BackSpace => KeyboardOutcome::ClusterBackspace,
                        Keysym::Delete => KeyboardOutcome::ClusterDelete,
                        Keysym::Left => KeyboardOutcome::ClusterMoveLeft,
                        Keysym::Right => KeyboardOutcome::ClusterMoveRight,
                        _ => sym
                            .key_char()
                            .map(KeyboardOutcome::ClusterCharacter)
                            .unwrap_or(KeyboardOutcome::ClusterIntercept),
                    }
                };
                return FilterResult::Intercept(outcome);
            }
            if state == KeyState::Released && data.shell.bearings.is_show_key_held(keycode.raw()) {
                return FilterResult::Intercept(KeyboardOutcome::BearingsRelease);
            }
            if data.shell.apogee.accepts_input() {
                let sym = handle.raw_latin_sym_or_raw_current_sym();
                if state == KeyState::Released {
                    // Mod was pressed before Apogee opened and was forwarded to
                    // the focused client. Forward its release while swallowing
                    // releases for keys whose presses Apogee intercepted; this
                    // prevents clients from being left in a stuck modifier or
                    // terminal keyboard-protocol state after Mod+O closes.
                    return match modal_key_routing(true, state, release_is_suppressed) {
                        ModalKeyRouting::SuppressRelease => {
                            FilterResult::Intercept(KeyboardOutcome::ApogeeIntercept)
                        }
                        ModalKeyRouting::RetireUnfocusedRelease => FilterResult::Forward,
                        ModalKeyRouting::Evaluate => unreachable!("release routing was requested"),
                    };
                }
                let outcome = match sym {
                    Some(Keysym::Escape) => KeyboardOutcome::ApogeeCancel,
                    Some(Keysym::Return | Keysym::KP_Enter) => KeyboardOutcome::ApogeeAccept,
                    Some(Keysym::Left) => {
                        KeyboardOutcome::ApogeeMove(crate::shell::apogee::Direction::Left)
                    }
                    Some(Keysym::Right) => {
                        KeyboardOutcome::ApogeeMove(crate::shell::apogee::Direction::Right)
                    }
                    Some(Keysym::Up) => {
                        KeyboardOutcome::ApogeeMove(crate::shell::apogee::Direction::Up)
                    }
                    Some(Keysym::Down) => {
                        KeyboardOutcome::ApogeeMove(crate::shell::apogee::Direction::Down)
                    }
                    _ => {
                        if let Some(bind) = match_keyboard_binding(
                            &data.keyboard.binds,
                            modifiers,
                            data.keyboard.side_modifiers,
                            crate::input::BindingContext::default(),
                            sym,
                            keycode,
                        ) && matches!(
                            bind.action,
                            halley_config::Action::Apogee
                                | halley_config::Action::Quit
                                | halley_config::Action::OpenTerminal
                        ) {
                            KeyboardOutcome::Action(bind.clone())
                        } else {
                            KeyboardOutcome::ApogeeIntercept
                        }
                    }
                };
                return FilterResult::Intercept(outcome);
            }
            if data.shell.focus_cycle.is_open() {
                let sym = handle.raw_latin_sym_or_raw_current_sym();
                if state == KeyState::Released && matches!(sym, Some(Keysym::Alt_L | Keysym::Alt_R))
                {
                    return FilterResult::Forward;
                }
                if state == KeyState::Pressed && sym == Some(Keysym::Escape) {
                    return FilterResult::Intercept(KeyboardOutcome::FocusCycleCancel);
                }
                if state == KeyState::Pressed
                    && let Some(bind) = match_keyboard_binding(
                        &data.keyboard.binds,
                        modifiers,
                        data.keyboard.side_modifiers,
                        crate::input::BindingContext::default(),
                        sym,
                        keycode,
                    )
                    && matches!(bind.action, halley_config::Action::FocusCycle(_))
                {
                    return FilterResult::Intercept(KeyboardOutcome::Action(bind.clone()));
                }
                return FilterResult::Intercept(KeyboardOutcome::FocusCycleIntercept);
            }
            if accessibility == crate::accessibility::KeyboardDisposition::Intercept {
                return FilterResult::Intercept(KeyboardOutcome::AccessibilityIntercept);
            }
            match modal_key_routing(data.capture.is_active(), state, release_is_suppressed) {
                ModalKeyRouting::SuppressRelease => {
                    return FilterResult::Intercept(KeyboardOutcome::CaptureIntercept);
                }
                ModalKeyRouting::RetireUnfocusedRelease => {
                    // Focus is cleared for the lifetime of the overlay. Forwarding
                    // releases here reaches no client, but lets Smithay retire keys
                    // whose presses were forwarded before the modal opened.
                    return FilterResult::Forward;
                }
                ModalKeyRouting::Evaluate => {}
            }
            if data.capture.is_active() {
                return match handle.raw_latin_sym_or_raw_current_sym() {
                    Some(Keysym::Escape) => FilterResult::Intercept(KeyboardOutcome::CaptureCancel),
                    Some(Keysym::Return | Keysym::KP_Enter) => {
                        FilterResult::Intercept(KeyboardOutcome::CaptureAccept)
                    }
                    Some(Keysym::Left) if data.capture.menu_is_active() => {
                        FilterResult::Intercept(KeyboardOutcome::CapturePrevious)
                    }
                    Some(Keysym::Right) if data.capture.menu_is_active() => {
                        FilterResult::Intercept(KeyboardOutcome::CaptureNext)
                    }
                    _ => FilterResult::Intercept(KeyboardOutcome::CaptureIntercept),
                };
            }
            if state != KeyState::Pressed {
                return FilterResult::Forward;
            }
            let sym = handle.raw_latin_sym_or_raw_current_sym();
            let non_modifier = sym.is_some_and(|sym| !is_modifier_keysym(sym));
            if !bindings_enabled {
                forwarded_non_modifier_press = non_modifier;
                return FilterResult::Forward;
            }
            let context = super::keyboard_binding_context(data);
            match match_keyboard_binding(
                &data.keyboard.binds,
                modifiers,
                data.keyboard.side_modifiers,
                context,
                sym,
                keycode,
            ) {
                Some(bind) => FilterResult::Intercept(KeyboardOutcome::Action(bind.clone())),
                None => {
                    forwarded_non_modifier_press = non_modifier;
                    FilterResult::Forward
                }
            }
        },
    );
    if forwarded_non_modifier_press {
        close_blooms_for_typing_away(session);
    }
    let pointer_output = session
        .wayland
        .space
        .output_under(session.pointer.position())
        .next()
        .map(Output::name);

    match action {
        Some(KeyboardOutcome::ExitConfirm) => {
            session.interactions.suppressed_keys.suppress(keycode);
            session.confirm_exit();
        }
        Some(KeyboardOutcome::ExitCancel) => {
            session.interactions.suppressed_keys.suppress(keycode);
            session.cancel_exit_confirmation();
        }
        Some(KeyboardOutcome::ExitIntercept) => {
            if state == KeyState::Pressed {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::ClusterDeleteConfirm) => {
            session.interactions.suppressed_keys.suppress(keycode);
            session.confirm_cluster_dissolution();
        }
        Some(KeyboardOutcome::ClusterDeleteCancel) => {
            session.interactions.suppressed_keys.suppress(keycode);
            session.cancel_cluster_dissolution();
        }
        Some(KeyboardOutcome::ClusterDeleteIntercept) => {
            if state == KeyState::Pressed {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::BasicsDismiss) => {
            session.interactions.suppressed_keys.suppress(keycode);
            session.dismiss_basics_card();
        }
        Some(KeyboardOutcome::BasicsIntercept) => {}
        Some(KeyboardOutcome::Action(bind)) => {
            session.interactions.suppressed_keys.suppress(keycode);
            close_blooms_for_keybind(session, pointer_output.as_deref());
            actions::dispatch(
                session,
                bind.action.clone(),
                socket_name,
                pointer_output.as_deref(),
                Some(keycode.raw()),
                actions::DispatchOrigin::Keyboard,
            );
            session.key_repeat.start(
                keycode,
                bind,
                session.settings.input.repeat_delay,
                session.settings.input.repeat_rate,
            );
        }
        Some(KeyboardOutcome::BearingsRelease) => {
            if session.shell.bearings.release_show_key(keycode.raw()) {
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterCancel) => {
            session.interactions.suppressed_keys.suppress(keycode);
            let draft_id = session.clusters.creation_draft_id();
            let naming = session
                .clusters
                .creation()
                .is_some_and(|creation| creation.naming);
            if session.shell.cluster_composer.is_active()
                && !session.shell.cluster_composer.accepts_input()
            {
                // Commit, endpoint hold, and reveal are atomic modal phases.
            } else if session.shell.cluster_composer.accepts_input() && !naming {
                session
                    .shell
                    .cluster_composer
                    .close(session.settings.apogee, crate::frame_clock::monotonic_now());
                session.request_redraw();
            } else if session.clusters.back_or_cancel_creation() {
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Modal, None);
                session.request_redraw();
                if session.clusters.creation().is_none()
                    && let Some(draft_id) = draft_id
                {
                    crate::ipc::publish_cluster_draft(
                        session,
                        draft_id,
                        halley_ipc::ClusterDraftState::Cancelled,
                        None,
                    );
                }
            }
        }
        Some(KeyboardOutcome::ClusterAccept) => {
            session.interactions.suppressed_keys.suppress(keycode);
            if session
                .clusters
                .creation()
                .is_some_and(|creation| creation.naming)
            {
                if session.shell.cluster_composer.is_active() {
                    begin_cluster_commit(session);
                } else {
                    finish_cluster_creation(session);
                }
            } else if !session.clusters.begin_naming()
                && let Some(output) = session
                    .clusters
                    .creation()
                    .map(|creation| creation.output.clone())
            {
                session.shell.overlays.show_error(
                    output,
                    "Not enough selections\nSelect at least one window",
                    3_000,
                    crate::frame_clock::monotonic_now(),
                );
            }
            session.request_redraw();
        }
        Some(KeyboardOutcome::ClusterBackspace) => {
            session.interactions.suppressed_keys.suppress(keycode);
            let input = crate::clusters::NameInput::Backspace;
            if session.clusters.edit_name(input) {
                session.clusters.start_name_repeat(
                    keycode.raw(),
                    input,
                    crate::frame_clock::monotonic_now(),
                    session.settings.input.repeat_delay,
                    session.settings.input.repeat_rate,
                );
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterDelete) => {
            session.interactions.suppressed_keys.suppress(keycode);
            let input = crate::clusters::NameInput::Delete;
            if session.clusters.edit_name(input) {
                session.clusters.start_name_repeat(
                    keycode.raw(),
                    input,
                    crate::frame_clock::monotonic_now(),
                    session.settings.input.repeat_delay,
                    session.settings.input.repeat_rate,
                );
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterComposerToggle) => {
            session.interactions.suppressed_keys.suppress(keycode);
            if let (Some(id), Some(output)) = (
                session.shell.cluster_composer.focused(),
                session
                    .shell
                    .cluster_composer
                    .target_output()
                    .map(str::to_string),
            ) && session.clusters.toggle_creation_member(id, &output)
            {
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterComposerMove(direction)) => {
            session.interactions.suppressed_keys.suppress(keycode);
            if session.shell.cluster_composer.move_focus(direction) {
                session.cursor_policy.keyboard_navigation();
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterMoveLeft) => {
            session.interactions.suppressed_keys.suppress(keycode);
            let input = crate::clusters::NameInput::MoveLeft;
            if session.clusters.edit_name(input) {
                session.clusters.start_name_repeat(
                    keycode.raw(),
                    input,
                    crate::frame_clock::monotonic_now(),
                    session.settings.input.repeat_delay,
                    session.settings.input.repeat_rate,
                );
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterMoveRight) => {
            session.interactions.suppressed_keys.suppress(keycode);
            let input = crate::clusters::NameInput::MoveRight;
            if session.clusters.edit_name(input) {
                session.clusters.start_name_repeat(
                    keycode.raw(),
                    input,
                    crate::frame_clock::monotonic_now(),
                    session.settings.input.repeat_delay,
                    session.settings.input.repeat_rate,
                );
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterCharacter(ch)) => {
            session.interactions.suppressed_keys.suppress(keycode);
            let input = crate::clusters::NameInput::Character(ch);
            if session.clusters.edit_name(input) {
                session.clusters.start_name_repeat(
                    keycode.raw(),
                    input,
                    crate::frame_clock::monotonic_now(),
                    session.settings.input.repeat_delay,
                    session.settings.input.repeat_rate,
                );
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ClusterIntercept) => {
            if state == KeyState::Pressed {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::AccessibilityIntercept) => {}
        Some(KeyboardOutcome::ApogeeCancel) => {
            session.interactions.suppressed_keys.suppress(keycode);
            crate::shell::apogee::cancel(session);
        }
        Some(KeyboardOutcome::ApogeeAccept) => {
            session.interactions.suppressed_keys.suppress(keycode);
            crate::shell::apogee::select(session);
        }
        Some(KeyboardOutcome::ApogeeMove(direction)) => {
            session.interactions.suppressed_keys.suppress(keycode);
            crate::shell::apogee::move_selection(session, direction);
            if session.cursor_policy.keyboard_navigation() {
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::ApogeeIntercept) => {
            if state == KeyState::Pressed {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::FocusCycleCancel) => {
            session.interactions.suppressed_keys.suppress(keycode);
            crate::shell::focus_cycle::cancel(session);
        }
        Some(KeyboardOutcome::FocusCycleIntercept) => {
            if state == KeyState::Pressed {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::CaptureAccept) => {
            if session.capture.menu_is_active() {
                if session
                    .capture
                    .activate_selected_menu(&session.wayland.space)
                {
                    update_capture_pointer(session, session.pointer.position());
                    session.request_redraw();
                }
            } else if crate::capture::accept_selected(session) {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::CaptureCancel) => {
            if session.capture.return_to_menu() {
                session.request_redraw();
            } else if crate::capture::cancel_selected(session) {
                session.interactions.suppressed_keys.suppress(keycode);
            }
        }
        Some(KeyboardOutcome::CapturePrevious) => {
            if session.capture.move_menu_selection(-1) {
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::CaptureNext) => {
            if session.capture.move_menu_selection(1) {
                session.request_redraw();
            }
        }
        Some(KeyboardOutcome::CaptureIntercept) | None => {}
    }

    if state == KeyState::Released
        && session.shell.focus_cycle.is_open()
        && !keyboard.modifier_state().alt
    {
        crate::shell::focus_cycle::commit(session, SERIAL_COUNTER.next_serial());
    }
}
