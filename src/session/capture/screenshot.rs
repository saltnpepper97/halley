//! Input and actions for the non-modal screenshot notification.
use std::sync::Arc;

use smithay::backend::input::{ButtonState, Event, InputBackend, InputEvent, PointerButtonEvent};
use smithay::input::pointer::CursorIcon;
use smithay::utils::{Logical, Point};
use smithay::wayland::selection::data_device::set_data_device_selection;

use super::{Session, SessionDriver};
use crate::capture::preview::ScreenshotPreview;
use crate::shell::screenshot::ScreenshotAction;

pub struct Hit {
    pub output: smithay::output::Output,
    pub preview: Arc<ScreenshotPreview>,
    pub action: Option<ScreenshotAction>,
}

pub fn hit<D: SessionDriver>(session: &Session<D>, position: Point<f64, Logical>) -> Option<Hit> {
    if session.session_lock.active()
        || session.capture.is_active()
        || session.shell.overlays.confirmation_modal_active()
        || session.shell.cluster_composer.is_active()
        || session.shell.apogee.accepts_input()
        || !matches!(session.interactions.grab, crate::input::grab::Grab::None)
        || session.seat.get_pointer().is_some_and(|p| p.is_grabbed())
        || super::pointer::has_active_constraint(session)
    {
        return None;
    }
    let now = crate::frame_clock::monotonic_now();
    for output in session.wayland.space.outputs() {
        let geometry = session.wayland.space.output_geometry(output)?;
        if !geometry.to_f64().contains(position) {
            continue;
        }
        if !session
            .shell
            .overlays
            .screenshot_accepts_input(&output.name(), now)
        {
            continue;
        }
        let notification = session
            .shell
            .overlays
            .snapshot(&output.name(), now)
            .notification?;
        let preview = notification.screenshot?;
        let layout = crate::shell::screenshot::layout(
            geometry,
            session.settings.overlays.notifications,
            notification.mix,
            session.settings.font.size,
        );
        if layout.card.to_f64().contains(position) {
            return Some(Hit {
                output: output.clone(),
                preview,
                action: layout.action_at(position),
            });
        }
    }
    None
}

pub fn activate<D: SessionDriver>(session: &mut Session<D>, hit: Hit) {
    let now = crate::frame_clock::monotonic_now();
    match hit.action {
        Some(ScreenshotAction::Copy) => {
            set_data_device_selection(
                &session.wayland.display_handle,
                &session.seat,
                vec!["image/png".into()],
                crate::wayland::screenshot_clipboard::SelectionData::Png(hit.preview.png.clone()),
            );
            #[cfg(feature = "xwayland")]
            session.xwayland.update_selection(
                smithay::wayland::selection::SelectionTarget::Clipboard,
                Some(vec!["image/png".into()]),
            );
            session
                .shell
                .overlays
                .screenshot_feedback("Copied to clipboard", now);
        }
        Some(ScreenshotAction::Open) => {
            let x11_display = session.xwayland.display_name();
            let result = session
                .wayland_display
                .as_deref()
                .ok_or_else(|| std::io::Error::other("Wayland display is unavailable"))
                .and_then(|display| {
                    super::spawn::spawn_program(
                        "xdg-open",
                        &hit.preview.path,
                        display,
                        x11_display.as_deref(),
                        session.cursor.size(),
                        &session.launch_environment,
                    )
                });
            match result {
                Ok(()) => session
                    .shell
                    .overlays
                    .screenshot_feedback("Opening screenshot", now),
                Err(err) => {
                    eventline::warn!("screenshot open: {err}");
                    session
                        .shell
                        .overlays
                        .screenshot_feedback("Could not open screenshot", now);
                }
            }
        }
        None => {}
    }
    session.request_redraw();
}

pub fn handle_pointer<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
) -> bool {
    if !matches!(
        event,
        InputEvent::PointerMotion { .. }
            | InputEvent::PointerMotionAbsolute { .. }
            | InputEvent::PointerButton { .. }
            | InputEvent::PointerAxis { .. }
    ) {
        return false;
    }
    let hit = hit(session, Point::from(session.pointer.position()));
    let hovered_action = hit.as_ref().and_then(|hit| hit.action);
    if session.shell.overlays.screenshot_hover(
        hit.is_some(),
        hovered_action,
        crate::frame_clock::monotonic_now(),
    ) {
        session.cursor.set_override(
            crate::cursor::OverrideSource::Hover,
            hit.as_ref().map(|_| {
                if hovered_action.is_some() {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                }
            }),
        );
        session.request_redraw();
    }
    let Some(hit) = hit else {
        return false;
    };
    // No client beneath the card receives motion to trigger a surface commit.
    // Queue every cursor move even when the hovered button is unchanged.
    if matches!(
        event,
        InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. }
    ) {
        session.request_output_redraw(&hit.output);
    }
    // The route shares this hit test, so clients receive a leave rather than
    // retaining hover/focus beneath the card. Keyboard focus is untouched.
    let time = match event {
        InputEvent::PointerMotion { event } => event.time().millis(),
        InputEvent::PointerMotionAbsolute { event } => event.time().millis(),
        InputEvent::PointerButton { event } => event.time().millis(),
        InputEvent::PointerAxis { event } => event.time().millis(),
        _ => unreachable!(),
    };
    super::pointer::route_for_motion(session, time);
    if let InputEvent::PointerButton { event } = event
        && event.state() == ButtonState::Pressed
    {
        session
            .interactions
            .screenshot_buttons
            .suppress(event.button_code());
        if event.button_code() == 0x110 {
            activate(session, hit);
        }
    }
    if let Some(pointer) = session.seat.get_pointer() {
        super::pointer::finish_frame(session, &pointer);
    }
    true
}
