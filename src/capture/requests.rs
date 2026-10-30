//! Start and validate local and portal capture requests.

use super::*;

pub fn begin_local<D: SessionDriver>(
    session: &mut Session<D>,
    preferred_output: Option<&str>,
    window_available: bool,
) -> bool {
    if session.session_lock.active() {
        return false;
    }
    if !session
        .capture
        .begin_menu(&session.wayland.space, preferred_output, window_available)
    {
        return false;
    }
    begin_modal_capture(session);
    true
}

pub fn request_screenshot<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::ScreenshotRequest,
    reply: crate::ipc::ReplySender,
) {
    if session.session_lock.active() {
        let _ = reply.send(
            halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                message: "session is locked".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    match request.target {
        halley_ipc::ScreenshotTarget::Area => {
            let preferred = crate::wayland::focus::selected_output(&session.wayland)
                .map(|output| output.name());
            let pending = PendingCapture::Screenshot {
                request_handle: request.request_handle,
                reply,
            };
            if let Err(PendingCapture::Screenshot { reply, .. }) =
                session
                    .capture
                    .begin_area(&session.wayland.space, preferred.as_deref(), pending)
            {
                let _ = reply.send(
                    halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                        message: "another capture is already active".to_string(),
                    }),
                    Vec::new(),
                );
                return;
            }
            begin_modal_capture(session);
        }
        halley_ipc::ScreenshotTarget::Screen => {
            let Some(region) = desktop_bounds(&session.wayland.space) else {
                let _ = reply.send(
                    halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                        message: "no active outputs".to_string(),
                    }),
                    Vec::new(),
                );
                return;
            };
            let captured = save_region(session, region);
            queue_capture(session, captured, PendingCaptureReply::Ipc(reply));
        }
        halley_ipc::ScreenshotTarget::Window => {
            let pending = PendingCapture::Screenshot {
                request_handle: request.request_handle,
                reply,
            };
            if let Err(PendingCapture::Screenshot { reply, .. }) =
                session.capture.begin_window(pending)
            {
                let _ = reply.send(
                    halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                        message: "another capture is already active".to_string(),
                    }),
                    Vec::new(),
                );
                return;
            }
            begin_modal_capture(session);
        }
    }
}

pub(super) fn fail_capture(
    reply: crate::ipc::ReplySender,
    kind: halley_ipc::ServerErrorKind,
    message: impl Into<String>,
) {
    let _ = reply.send(
        halley_ipc::Response::ApiError(halley_ipc::ServerError::new(kind, message)),
        Vec::new(),
    );
}

pub(super) fn capture_unavailable<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(halley_ipc::ServerErrorKind, &'static str)> {
    if session.session_lock.active() {
        Some((
            halley_ipc::ServerErrorKind::InvalidRequest,
            "session is locked",
        ))
    } else if session.capture.is_active() {
        Some((
            halley_ipc::ServerErrorKind::Busy,
            "another capture is already active",
        ))
    } else {
        None
    }
}

pub(super) fn requested_output_geometry<D: SessionDriver>(
    session: &Session<D>,
    name: &str,
) -> Option<Rectangle<i32, Logical>> {
    session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == name)
        .and_then(|output| session.wayland.space.output_geometry(output))
}

/// Validate before translating into desktop coordinates. Never silently clip a target.
pub(super) fn output_relative_rect(
    output: Rectangle<i32, Logical>,
    rect: halley_ipc::CaptureRect,
) -> Result<Rectangle<i32, Logical>, &'static str> {
    if rect.x < 0 || rect.y < 0 || rect.width <= 0 || rect.height <= 0 {
        return Err("capture rectangle requires nonnegative X/Y and positive WIDTH/HEIGHT");
    }
    if i64::from(rect.x) + i64::from(rect.width) > i64::from(output.size.w)
        || i64::from(rect.y) + i64::from(rect.height) > i64::from(output.size.h)
    {
        return Err("capture rectangle is outside the output's logical bounds");
    }
    let x = output
        .loc
        .x
        .checked_add(rect.x)
        .ok_or("capture X coordinate overflow")?;
    let y = output
        .loc
        .y
        .checked_add(rect.y)
        .ok_or("capture Y coordinate overflow")?;
    x.checked_add(rect.width)
        .ok_or("capture right edge overflow")?;
    y.checked_add(rect.height)
        .ok_or("capture bottom edge overflow")?;
    Ok(Rectangle::new(
        (x, y).into(),
        (rect.width, rect.height).into(),
    ))
}

pub fn request_capture<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::CaptureRequest,
    silent: bool,
    reply: crate::ipc::ReplySender,
) {
    use halley_ipc::{CaptureRequest, LocalCaptureMode, ServerErrorKind};
    if let Some((kind, message)) = capture_unavailable(session) {
        fail_capture(reply, kind, message);
        return;
    }
    let (mode, output) = match request {
        CaptureRequest::Window { node_id } => {
            session.nodes.sync_from_space(&session.wayland.space);
            let id = halley_core::field::NodeId::new(node_id);
            let Some((surface, output_name)) = session
                .nodes
                .record(id)
                .map(|record| (record.surface.clone(), record.output.clone()))
            else {
                fail_capture(
                    reply,
                    ServerErrorKind::NotFound,
                    format!("unknown node {node_id}"),
                );
                return;
            };
            session.shell.overlays.clear_screenshot();
            session.request_redraw();
            let captured = save_window(session, &surface);
            queue_capture(
                session,
                captured,
                PendingCaptureReply::local(output_name, reply, silent),
            );
            return;
        }
        CaptureRequest::Region {
            output,
            rect: Some(rect),
        } => {
            let Some(name) = output else {
                fail_capture(
                    reply,
                    ServerErrorKind::InvalidRequest,
                    "capture rectangle requires an output",
                );
                return;
            };
            let Some(geometry) = requested_output_geometry(session, &name) else {
                fail_capture(
                    reply,
                    ServerErrorKind::NotFound,
                    format!("unknown or unavailable output {name:?}"),
                );
                return;
            };
            let region = match output_relative_rect(geometry, rect) {
                Ok(region) => region,
                Err(message) => {
                    fail_capture(reply, ServerErrorKind::InvalidRequest, message);
                    return;
                }
            };
            session.shell.overlays.clear_screenshot();
            session.request_redraw();
            let captured = screenshot::save_output_region(session, &name, region);
            queue_capture(
                session,
                captured,
                PendingCaptureReply::local(name, reply, silent),
            );
            return;
        }
        CaptureRequest::Menu { output } => (LocalCaptureMode::Menu, output),
        CaptureRequest::Screen { output } => (LocalCaptureMode::Screen, output),
        CaptureRequest::Region { output, rect: None } => (LocalCaptureMode::Region, output),
    };
    request_local_capture_with_options(
        session,
        halley_ipc::LocalCaptureRequest { mode, output },
        silent,
        reply,
    );
}

pub fn request_local_capture<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::LocalCaptureRequest,
    reply: crate::ipc::ReplySender,
) {
    request_local_capture_with_options(session, request, false, reply);
}

pub(super) fn request_local_capture_with_options<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::LocalCaptureRequest,
    silent: bool,
    reply: crate::ipc::ReplySender,
) {
    use halley_ipc::ServerErrorKind;
    if let Some((kind, message)) = capture_unavailable(session) {
        fail_capture(reply, kind, message);
        return;
    }
    let explicit_output = request.output.is_some();
    if let Some(name) = request.output.as_deref() {
        let Some(geometry) = requested_output_geometry(session, name) else {
            fail_capture(
                reply,
                ServerErrorKind::NotFound,
                format!("unknown or unavailable output {name:?}"),
            );
            return;
        };
        if request.mode == halley_ipc::LocalCaptureMode::Screen {
            session.shell.overlays.clear_screenshot();
            session.request_redraw();
            let captured = screenshot::save_output_region(session, name, geometry);
            queue_capture(
                session,
                captured,
                PendingCaptureReply::local(name.to_string(), reply, silent),
            );
            return;
        }
    }
    let preferred = request.output.or_else(|| {
        crate::wayland::focus::selected_output(&session.wayland).map(|output| output.name())
    });
    let window_available = session.wayland.space.elements().any(|window| {
        preferred.as_deref().is_none_or(|output| {
            crate::wayland::window_output_name(window)
                .map(|name| name == output)
                .unwrap_or_else(|| session.driver.primary_output().name() == output)
        })
    });
    if !session.capture.begin_menu(
        &session.wayland.space,
        preferred.as_deref(),
        window_available,
    ) {
        fail_capture(reply, ServerErrorKind::NotFound, "no available output");
        return;
    }
    session.capture.attach_local_reply(reply, silent);
    let mode = match request.mode {
        halley_ipc::LocalCaptureMode::Menu => None,
        halley_ipc::LocalCaptureMode::Region => Some(ScreenshotMode::Region),
        halley_ipc::LocalCaptureMode::Screen => Some(ScreenshotMode::Screen),
        halley_ipc::LocalCaptureMode::Window => Some(ScreenshotMode::Window),
    };
    let activated = match mode {
        Some(ScreenshotMode::Region) if explicit_output => session
            .capture
            .activate_region(&session.wayland.space, preferred.as_deref()),
        Some(mode) => session.capture.activate_menu(mode, &session.wayland.space),
        None => true,
    };
    if !activated {
        if let Some(PendingCapture::Local {
            reply: Some(reply), ..
        }) = session.capture.cancel()
        {
            fail_capture(
                reply,
                ServerErrorKind::NotFound,
                "requested capture mode is unavailable",
            );
        }
        return;
    }
    begin_modal_capture(session);
}

pub fn request_source<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::SourceChooserRequest,
    reply: crate::ipc::ReplySender,
) {
    if session.session_lock.active() {
        let _ = reply.send(
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Failed {
                message: "session is locked".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    let supported = request.source_types & (halley_ipc::SOURCE_MONITOR | halley_ipc::SOURCE_WINDOW);
    if supported == 0 {
        let _ = reply.send(
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Failed {
                message: "no supported source type was requested".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    let pending = PendingCapture::Source {
        request_handle: request.request_handle,
        reply,
    };
    let preferred =
        crate::wayland::focus::selected_output(&session.wayland).map(|output| output.name());
    if let Err(PendingCapture::Source { reply, .. }) = session.capture.begin_source(
        &session.wayland.space,
        preferred.as_deref(),
        supported,
        pending,
    ) {
        let _ = reply.send(
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Failed {
                message: "another capture is already active".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    begin_modal_capture(session);
}

pub fn cancel_portal<D: SessionDriver>(session: &mut Session<D>, request_handle: &str) -> bool {
    let matches = matches!(
        session.capture.pending.as_ref(),
        Some(PendingCapture::Screenshot { request_handle: active, .. }
            | PendingCapture::Source { request_handle: active, .. })
            if active == request_handle
    );
    if !matches {
        return false;
    }
    match session.capture.cancel() {
        Some(PendingCapture::Screenshot { reply, .. }) => {
            let _ = reply.send(
                halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Cancelled),
                Vec::new(),
            );
        }
        Some(PendingCapture::Source { reply, .. }) => {
            let _ = reply.send(
                halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Cancelled),
                Vec::new(),
            );
        }
        _ => {}
    }
    finish_modal_capture(session);
    true
}
