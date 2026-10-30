use super::*;

// Runs before touch and gesture dispatch, so owned streams never reach clients.
pub(super) fn intercept_early<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
) -> bool {
    if session.shell.overlays.confirmation_modal_active()
        && !matches!(event, InputEvent::Keyboard { .. })
    {
        session.interactions.steam_close_pressed = None;
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                session
                    .pointer
                    .process_input_event(event, &session.wayland.space);
                session.cursor_policy.pointer_activity();
                session.request_redraw();
            }
            InputEvent::PointerButton { event } => match event.state() {
                ButtonState::Pressed => session
                    .interactions
                    .suppressed_buttons
                    .suppress(event.button_code()),
                ButtonState::Released => {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(event.button_code());
                }
            },
            InputEvent::PointerAxis { .. } => session.interactions.wheel_accumulator.reset_all(),
            _ => {}
        }
        return true;
    }
    // The basics card is not modal. It owns only the press/touch stream that
    // dismisses it; card visibility continues through the fade, but ownership
    // does not extend to a new press or touch during that fade.
    let basics_routing = match event {
        InputEvent::PointerButton { event } => basics_pointer_button_routing(
            session.shell.overlays.basics_card_accepts_input(),
            event.button_code(),
            event.state(),
            &mut session.interactions.basics_buttons,
        ),
        InputEvent::TouchDown { event } => basics_touch_routing(
            session.shell.overlays.basics_card_accepts_input(),
            event.slot(),
            BasicsTouchPhase::Down,
            &mut session.interactions.basics_touches,
        ),
        InputEvent::TouchMotion { event } => basics_touch_routing(
            false,
            event.slot(),
            BasicsTouchPhase::Motion,
            &mut session.interactions.basics_touches,
        ),
        InputEvent::TouchUp { event } => basics_touch_routing(
            false,
            event.slot(),
            BasicsTouchPhase::Up,
            &mut session.interactions.basics_touches,
        ),
        InputEvent::TouchCancel { .. } => {
            session.interactions.basics_touches.clear();
            BasicsInputRouting::Forward
        }
        _ => BasicsInputRouting::Forward,
    };
    match basics_routing {
        BasicsInputRouting::Dismiss => {
            session.dismiss_basics_card();
            return true;
        }
        BasicsInputRouting::SuppressOwned => return true,
        BasicsInputRouting::Forward => {}
    }
    false
}

// Runs after pointer position updates, in capture, cluster composer, then Apogee order.
pub(super) fn intercept_pointer<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
    proposed_position: (f64, f64),
) -> bool {
    let pointer_handle = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    if session.capture.is_active() {
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                update_capture_pointer(session, proposed_position);
                session.request_redraw();
                return true;
            }
            InputEvent::PointerButton { event } => {
                if event.button_code() == BTN_LEFT {
                    update_capture_pointer(session, proposed_position);
                    match event.state() {
                        ButtonState::Pressed => {
                            session.interactions.suppressed_buttons.suppress(BTN_LEFT);
                            match session.capture.press(proposed_position) {
                                Some(crate::capture::CapturePress::ActivateScreenshot(mode)) => {
                                    if session.capture.activate_menu(mode, &session.wayland.space) {
                                        update_capture_pointer(session, proposed_position);
                                    }
                                }
                                Some(crate::capture::CapturePress::ActivateSource(mode)) => {
                                    if session.capture.activate_source(mode) {
                                        update_capture_pointer(session, proposed_position);
                                    }
                                }
                                Some(crate::capture::CapturePress::Accept) => {
                                    crate::capture::accept_selected(session);
                                }
                                Some(crate::capture::CapturePress::Consumed) | None => {}
                            }
                        }
                        ButtonState::Released => {
                            session
                                .interactions
                                .suppressed_buttons
                                .release_is_suppressed(BTN_LEFT);
                            session.capture.release();
                        }
                    }
                }
                session.request_redraw();
                return true;
            }
            InputEvent::PointerAxis { .. } => return true,
            _ => {}
        }
    }
    if session.shell.cluster_composer.accepts_input() {
        let naming = session
            .clusters
            .creation()
            .is_some_and(|creation| creation.naming);
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                if naming {
                    let output = session
                        .shell
                        .cluster_composer
                        .target_output()
                        .map(str::to_string);
                    let hit = output.as_deref().and_then(|output| {
                        session
                            .render
                            .cluster_creation_overlay
                            .hit_test(output, proposed_position)
                    });
                    if let Some(
                        crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(
                            caret,
                        ),
                    ) = hit
                    {
                        session.clusters.drag_name_selection(caret);
                    }
                    session.cursor.set_override(
                        crate::cursor::OverrideSource::Modal,
                        match hit {
                            Some(
                                crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                            ) => Some(smithay::input::pointer::CursorIcon::Pointer),
                            Some(
                                crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(_),
                            ) => Some(smithay::input::pointer::CursorIcon::Text),
                            None => None,
                        },
                    );
                } else {
                    session
                        .shell
                        .cluster_composer
                        .hover(Point::<f64, Logical>::from(proposed_position));
                }
            }
            InputEvent::PointerButton { event } => {
                let button = event.button_code();
                match event.state() {
                    ButtonState::Pressed => {
                        session.interactions.suppressed_buttons.suppress(button);
                        if button == BTN_LEFT {
                            if naming {
                                let output = session
                                    .shell
                                    .cluster_composer
                                    .target_output()
                                    .map(str::to_string);
                                match output.as_deref().and_then(|output| {
                                    session
                                        .render
                                        .cluster_creation_overlay
                                        .hit_test(output, proposed_position)
                                }) {
                                    Some(
                                        crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                                    ) => {
                                        begin_cluster_commit(session);
                                    }
                                    Some(
                                        crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(caret),
                                    ) => {
                                        session.clusters.begin_name_selection(caret);
                                    }
                                    None => {}
                                }
                            } else {
                                let position = Point::<f64, Logical>::from(proposed_position);
                                let clicked = session
                                    .shell
                                    .cluster_composer
                                    .session()
                                    .and_then(|composer| composer.hit_test(position));
                                session.shell.cluster_composer.hover(position);
                                if let (Some(id), Some(output)) = (
                                    clicked,
                                    session
                                        .shell
                                        .cluster_composer
                                        .target_output()
                                        .map(str::to_string),
                                ) {
                                    session.clusters.toggle_creation_member(id, &output);
                                }
                            }
                        }
                    }
                    ButtonState::Released => {
                        session
                            .interactions
                            .suppressed_buttons
                            .release_is_suppressed(button);
                        if naming && button == BTN_LEFT {
                            session.clusters.end_name_selection();
                        }
                    }
                }
            }
            InputEvent::PointerAxis { .. } => {
                session.interactions.wheel_accumulator.reset_all();
            }
            _ => {}
        }
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            session.request_redraw();
            super::super::pointer::finish_frame(session, &pointer_handle);
            return true;
        }
    }
    if session.shell.cluster_composer.is_active() {
        match event {
            InputEvent::PointerButton { event } => match event.state() {
                ButtonState::Pressed => session
                    .interactions
                    .suppressed_buttons
                    .suppress(event.button_code()),
                ButtonState::Released => {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(event.button_code());
                }
            },
            InputEvent::PointerAxis { .. } => {
                session.interactions.wheel_accumulator.reset_all();
            }
            _ => {}
        }
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            session.request_redraw();
            super::super::pointer::finish_frame(session, &pointer_handle);
            return true;
        }
    }
    if session.shell.apogee.accepts_input() {
        match event {
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => {
                crate::shell::apogee::pointer_motion(session, proposed_position);
            }
            InputEvent::PointerButton { event } if event.button_code() == BTN_LEFT => {
                match event.state() {
                    ButtonState::Pressed => {
                        session.interactions.suppressed_buttons.suppress(BTN_LEFT);
                        crate::shell::apogee::pointer_press(session, proposed_position);
                    }
                    ButtonState::Released => {
                        session
                            .interactions
                            .suppressed_buttons
                            .release_is_suppressed(BTN_LEFT);
                    }
                }
            }
            InputEvent::PointerButton { .. } | InputEvent::PointerAxis { .. } => {}
            _ => {}
        }
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            session.request_redraw();
            return true;
        }
    }
    false
}
