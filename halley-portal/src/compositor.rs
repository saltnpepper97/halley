use halley_ipc::{
    CaptureFrameRequest, CaptureFrameResponse, Connection, Request, Response, ScreenshotRequest,
    ScreenshotResponse, ScreenshotTarget, SourceChooserRequest, SourceChooserResponse,
};

pub fn screenshot(
    request_handle: String,
    target: ScreenshotTarget,
) -> Result<ScreenshotResponse, String> {
    let mut connection =
        Connection::connect().map_err(|err| format!("connect to compositor: {err}"))?;
    let response = connection
        .request(
            &Request::Screenshot(ScreenshotRequest {
                request_handle,
                target,
            }),
            &[],
        )
        .map_err(|err| format!("compositor screenshot request: {err}"))?;
    if !response.fds.is_empty() {
        return Err("compositor returned unexpected screenshot descriptors".to_string());
    }
    match response.response {
        Response::Screenshot(response) => Ok(response),
        Response::Error(message) => Err(message),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

pub fn choose_source(
    request_handle: String,
    source_types: u32,
) -> Result<SourceChooserResponse, String> {
    let mut connection =
        Connection::connect().map_err(|err| format!("connect to compositor: {err}"))?;
    let response = connection
        .request(
            &Request::ChooseSource(SourceChooserRequest {
                request_handle,
                source_types,
            }),
            &[],
        )
        .map_err(|err| format!("compositor source chooser: {err}"))?;
    match response.response {
        Response::Source(response) => Ok(response),
        Response::Error(message) => Err(message),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

pub fn cancel_source(request_handle: String) -> Result<(), String> {
    request_ack(Request::CancelSourceChooser { request_handle }, &[])
}

pub fn connect() -> Result<Connection, String> {
    Connection::connect().map_err(|err| format!("connect to compositor: {err}"))
}

pub fn begin_window_cast(connection: &mut Connection, surface_id: u32) -> Result<(), String> {
    request_ack_on(connection, Request::BeginWindowCast { surface_id }, &[])
}

pub fn capture_capabilities(
    connection: &mut Connection,
) -> Result<halley_ipc::CaptureCapabilities, String> {
    let response = connection
        .request(&Request::CaptureCapabilities, &[])
        .map_err(|err| format!("query compositor capture capabilities: {err}"))?;
    if !response.fds.is_empty() {
        return Err("compositor returned unexpected capability descriptors".to_string());
    }
    match response.response {
        Response::CaptureCapabilities(capabilities) => Ok(capabilities),
        Response::Error(message) => Err(message),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

pub fn capture_frame(
    connection: &mut Connection,
    request: CaptureFrameRequest,
    fd: Option<std::os::fd::RawFd>,
) -> Result<CaptureFrameResponse, String> {
    let fds = fd.as_slice();
    let response = connection
        .request(&Request::CaptureFrame(request), fds)
        .map_err(|err| format!("capture compositor frame: {err}"))?;
    match response.response {
        Response::Frame(response) => Ok(response),
        Response::Error(message) => Err(message),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}

pub fn register_dmabuf(
    connection: &mut Connection,
    request: halley_ipc::RegisterDmabufRequest,
    fds: &[std::os::fd::RawFd],
) -> Result<(), String> {
    request_ack_on(connection, Request::RegisterDmabuf(request), fds)
}

pub fn remove_dmabuf(
    connection: &mut Connection,
    stream_handle: String,
    buffer_id: u64,
) -> Result<(), String> {
    request_ack_on(
        connection,
        Request::RemoveDmabuf {
            stream_handle,
            buffer_id,
        },
        &[],
    )
}

pub fn cancel_screenshot(request_handle: String) -> Result<(), String> {
    request_ack(Request::CancelScreenshot { request_handle }, &[])
}

fn request_ack(request: Request, fds: &[std::os::fd::RawFd]) -> Result<(), String> {
    let mut connection =
        Connection::connect().map_err(|err| format!("connect to compositor: {err}"))?;
    request_ack_on(&mut connection, request, fds)
}

fn request_ack_on(
    connection: &mut Connection,
    request: Request,
    fds: &[std::os::fd::RawFd],
) -> Result<(), String> {
    let response = connection
        .request(&request, fds)
        .map_err(|err| format!("cancel compositor screenshot: {err}"))?;
    match response.response {
        Response::Ack => Ok(()),
        Response::Error(message) => Err(message),
        other => Err(format!("unexpected compositor response: {other:?}")),
    }
}
