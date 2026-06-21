use std::os::fd::RawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use halley_api::protocol::PortalDmabufPlane;
use halley_api::{
    CaptureMode, CaptureRequest, CaptureStatusResponse, PortalScreenCastRequest,
    PortalScreenCastResponse, PortalSourceSelection, Request, Response,
};
use halley_ipc::{
    CodecError, decode_response, default_socket_path, encode_request, read_frame, write_frame,
    write_frame_with_fds,
};

const IPC_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the source chooser is allowed to stay open before we give up.
const CHOOSER_DEADLINE: Duration = Duration::from_secs(300);
const CHOOSER_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// How long a screenshot session is allowed to stay open before we give up.
const SCREENSHOT_DEADLINE: Duration = Duration::from_secs(300);
const SCREENSHOT_POLL_INTERVAL: Duration = Duration::from_millis(200);

pub struct CompositorClient;

impl CompositorClient {
    pub fn start(
        session_handle: &str,
        output: &str,
        cursor_mode: u32,
    ) -> Result<PortalScreenCastResponse, String> {
        let resp = send_portal_request(PortalScreenCastRequest::Start {
            session_handle: session_handle.to_string(),
            output: output.to_string(),
            cursor_mode,
        })?;
        Ok(resp)
    }

    pub fn start_window(
        session_handle: &str,
        node_id: u64,
        cursor_mode: u32,
    ) -> Result<PortalScreenCastResponse, String> {
        let resp = send_portal_request(PortalScreenCastRequest::StartWindow {
            session_handle: session_handle.to_string(),
            node_id,
            cursor_mode,
        })?;
        Ok(resp)
    }

    pub fn stop(session_handle: &str) -> Result<(), String> {
        let _ = send_portal_request(PortalScreenCastRequest::Stop {
            session_handle: session_handle.to_string(),
        })?;
        Ok(())
    }

    /// Tell the compositor whether the PipeWire stream is actively being
    /// consumed. When inactive, the compositor can skip fresh captures.
    pub fn set_active(session_handle: &str, active: bool) -> Result<(), String> {
        let _ = send_portal_request(PortalScreenCastRequest::SetActive {
            session_handle: session_handle.to_string(),
            active,
        })?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_dmabuf_buffer(
        session_handle: &str,
        buffer_id: u64,
        width: i32,
        height: i32,
        format: u32,
        modifier: u64,
        flags: u32,
        planes: Vec<PortalDmabufPlane>,
        fds: &[RawFd],
    ) -> Result<(), String> {
        let resp = send_portal_request_with_fds(
            PortalScreenCastRequest::AddDmabufBuffer {
                session_handle: session_handle.to_string(),
                buffer_id,
                width,
                height,
                format,
                modifier,
                flags,
                planes,
            },
            fds,
        )?;
        match resp {
            PortalScreenCastResponse::DmabufBufferAdded => Ok(()),
            PortalScreenCastResponse::Error(err) => Err(err),
            other => Err(format!("unexpected dmabuf add response: {other:?}")),
        }
    }

    pub fn remove_dmabuf_buffer(session_handle: &str, buffer_id: u64) -> Result<(), String> {
        let resp = send_portal_request(PortalScreenCastRequest::RemoveDmabufBuffer {
            session_handle: session_handle.to_string(),
            buffer_id,
        })?;
        match resp {
            PortalScreenCastResponse::DmabufBufferRemoved => Ok(()),
            PortalScreenCastResponse::Error(err) => Err(err),
            other => Err(format!("unexpected dmabuf remove response: {other:?}")),
        }
    }

    pub fn render_dmabuf_buffer(session_handle: &str, buffer_id: u64) -> Result<(), String> {
        let resp = send_portal_request(PortalScreenCastRequest::RenderDmabufBuffer {
            session_handle: session_handle.to_string(),
            buffer_id,
        })?;
        match resp {
            PortalScreenCastResponse::DmabufFrameRendered => Ok(()),
            PortalScreenCastResponse::Error(err) => Err(err),
            other => Err(format!("unexpected dmabuf render response: {other:?}")),
        }
    }

    /// Open the Halley-native source chooser and block until the user confirms
    /// or cancels. Returns the picked source on confirmation.
    pub fn choose_source(
        session_handle: &str,
        source_types: u32,
    ) -> Result<PortalSourceSelection, String> {
        match send_portal_request(PortalScreenCastRequest::StartSourceChooser {
            session_handle: session_handle.to_string(),
            source_types,
        })? {
            PortalScreenCastResponse::SourceChooserStarted => {}
            PortalScreenCastResponse::Error(e) => return Err(e),
            other => return Err(format!("unexpected chooser start response: {other:?}")),
        }

        let deadline = std::time::Instant::now() + CHOOSER_DEADLINE;
        loop {
            if std::time::Instant::now() > deadline {
                let _ = Self::cancel_chooser(session_handle);
                return Err("source chooser timed out waiting for user".into());
            }
            match send_portal_request(PortalScreenCastRequest::PollSourceChooser {
                session_handle: session_handle.to_string(),
            })? {
                PortalScreenCastResponse::SourceChooserPending => {
                    std::thread::sleep(CHOOSER_POLL_INTERVAL);
                }
                PortalScreenCastResponse::SourceChooserSelected(selection) => {
                    return Ok(selection);
                }
                PortalScreenCastResponse::SourceChooserCancelled => {
                    return Err("user cancelled source selection".into());
                }
                PortalScreenCastResponse::Error(e) => return Err(e),
                other => return Err(format!("unexpected chooser poll response: {other:?}")),
            }
        }
    }

    pub fn cancel_chooser(session_handle: &str) -> Result<(), String> {
        let _ = send_portal_request(PortalScreenCastRequest::CancelSourceChooser {
            session_handle: session_handle.to_string(),
        })?;
        Ok(())
    }

    /// Start a Halley-native screenshot session for the given capture mode and
    /// block until the user confirms, cancels, or the deadline expires. Returns
    /// the saved file path on success.
    pub fn screenshot(mode: CaptureMode) -> Result<ScreenshotOutcome, String> {
        let status = send_capture_request(CaptureRequest::Start { mode, output: None })?;
        let serial = status
            .session_serial
            .ok_or_else(|| "no session serial returned".to_string())?;

        let deadline = std::time::Instant::now() + SCREENSHOT_DEADLINE;
        loop {
            if std::time::Instant::now() > deadline {
                return Err("screenshot timed out waiting for user".into());
            }
            let status = send_capture_request(CaptureRequest::Status)?;
            if status.last_finished_serial == Some(serial) {
                if let Some(path) = status.saved_path {
                    return Ok(ScreenshotOutcome::Saved(path));
                }
                if let Some(err) = status.error {
                    if err == "cancelled" {
                        return Ok(ScreenshotOutcome::Cancelled);
                    }
                    return Err(err);
                }
                return Err("screenshot completed with no result".into());
            }
            std::thread::sleep(SCREENSHOT_POLL_INTERVAL);
        }
    }
}

/// Outcome of a portal screenshot request.
pub enum ScreenshotOutcome {
    /// The screenshot was saved; contains the file path.
    Saved(String),
    /// The user dismissed the capture overlay.
    Cancelled,
}

fn send_portal_request(
    request: PortalScreenCastRequest,
) -> Result<PortalScreenCastResponse, String> {
    let response =
        send_request_with_timeout(&Request::PortalScreenCast(request)).map_err(|e| match e {
            CodecError::Io(io_err) => format!("compositor ipc: {io_err}"),
            other => format!("compositor ipc: {other}"),
        })?;

    match response {
        Response::PortalScreenCast(resp) => Ok(resp),
        Response::Error(api_err) => Err(format!("compositor: {api_err:?}")),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

fn send_portal_request_with_fds(
    request: PortalScreenCastRequest,
    fds: &[RawFd],
) -> Result<PortalScreenCastResponse, String> {
    let response = send_request_with_fds_timeout(&Request::PortalScreenCast(request), fds)
        .map_err(|e| match e {
            CodecError::Io(io_err) => format!("compositor ipc: {io_err}"),
            other => format!("compositor ipc: {other}"),
        })?;

    match response {
        Response::PortalScreenCast(resp) => Ok(resp),
        Response::Error(api_err) => Err(format!("compositor: {api_err:?}")),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

fn send_capture_request(request: CaptureRequest) -> Result<CaptureStatusResponse, String> {
    let response = send_request_with_timeout(&Request::Capture(request))
        .map_err(|e| format!("compositor ipc: {e}"))?;

    match response {
        Response::CaptureStatus(status) => Ok(status),
        Response::Error(api_err) => Err(format!("compositor: {api_err:?}")),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

fn send_request_with_timeout(req: &Request) -> Result<Response, CodecError> {
    let path = default_socket_path()?;
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;

    let bytes = encode_request(req)?;
    write_frame(&mut stream, &bytes)?;

    let resp_bytes = read_frame(&mut stream)?;
    decode_response(&resp_bytes)
}

fn send_request_with_fds_timeout(req: &Request, fds: &[RawFd]) -> Result<Response, CodecError> {
    let path = default_socket_path()?;
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;

    let bytes = encode_request(req)?;
    write_frame_with_fds(&stream, &bytes, fds)?;

    let resp_bytes = read_frame(&mut stream)?;
    decode_response(&resp_bytes)
}
