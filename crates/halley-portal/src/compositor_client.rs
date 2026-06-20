use std::os::unix::net::UnixStream;
use std::time::Duration;

use halley_api::{
    PortalScreenCastRequest, PortalScreenCastResponse, PortalSourceSelection, Request, Response,
};
use halley_ipc::{
    CodecError, decode_response, default_socket_path, encode_request, read_frame, write_frame,
};

const IPC_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the source chooser is allowed to stay open before we give up.
const CHOOSER_DEADLINE: Duration = Duration::from_secs(300);
const CHOOSER_POLL_INTERVAL: Duration = Duration::from_millis(200);

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
