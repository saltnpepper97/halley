use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Request, Response};

/// Sanity bound on a single frame - these messages are small (a handful of
/// outputs, a version string), so anything claiming to be this large is
/// either a bug or a hostile peer, not a legitimate request/response.
const MAX_FRAME_LEN: usize = 1024 * 1024;

#[derive(Debug)]
pub enum CodecError {
    Io(io::Error),
    Postcard(postcard::Error),
    FrameTooLarge(usize),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::Io(e) => write!(f, "IPC I/O error: {e}"),
            CodecError::Postcard(e) => write!(f, "IPC decode error: {e}"),
            CodecError::FrameTooLarge(len) => write!(f, "IPC frame too large: {len} bytes"),
        }
    }
}

impl std::error::Error for CodecError {}

impl From<io::Error> for CodecError {
    fn from(e: io::Error) -> Self {
        CodecError::Io(e)
    }
}

impl From<postcard::Error> for CodecError {
    fn from(e: postcard::Error) -> Self {
        CodecError::Postcard(e)
    }
}

/// Writes one length-prefixed frame: a 4-byte little-endian length, then
/// that many bytes of payload.
pub fn write_frame(stream: &mut impl Write, bytes: &[u8]) -> Result<(), CodecError> {
    if bytes.len() > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(bytes.len()));
    }
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(bytes)?;
    Ok(())
}

/// Reads one length-prefixed frame written by `write_frame`.
pub fn read_frame(stream: &mut impl Read) -> Result<Vec<u8>, CodecError> {
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(len));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

/// Writes a frame and attaches file descriptors to its first bytes.
///
/// A descriptor passed with `SCM_RIGHTS` is duplicated into the receiving
/// process. The caller retains ownership of every descriptor in `fds`.
pub fn write_frame_with_fds(
    stream: &UnixStream,
    bytes: &[u8],
    fds: &[RawFd],
) -> Result<(), CodecError> {
    if bytes.len() > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(bytes.len()));
    }
    if fds.is_empty() {
        let mut stream = stream;
        return write_frame(&mut stream, bytes);
    }

    let len = (bytes.len() as u32).to_le_bytes();
    let iov = [
        libc::iovec {
            iov_base: len.as_ptr().cast_mut().cast(),
            iov_len: len.len(),
        },
        libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        },
    ];
    let fd_bytes = std::mem::size_of_val(fds);
    let control_len = unsafe { libc::CMSG_SPACE(fd_bytes as libc::c_uint) as usize };
    let mut control = vec![0u8; control_len];

    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = iov.as_ptr().cast_mut();
    // glibc types these as size_t; musl uses int / socklen_t.
    message.msg_iovlen = iov.len() as _;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;

    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(io::Error::other("could not construct IPC descriptor message").into());
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(fd_bytes as libc::c_uint) as _;
        std::ptr::copy_nonoverlapping(
            fds.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header).cast::<u8>(),
            fd_bytes,
        );
    }

    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
    if sent < 0 {
        return Err(io::Error::last_os_error().into());
    }

    let mut frame = Vec::with_capacity(len.len() + bytes.len());
    frame.extend_from_slice(&len);
    frame.extend_from_slice(bytes);
    let sent = sent as usize;
    if sent < frame.len() {
        let mut stream = stream;
        stream.write_all(&frame[sent..])?;
    }
    Ok(())
}

/// Reads one frame and receives the descriptors attached to it.
///
/// `max_fds` is a protocol limit, not merely a buffer hint. Receiving more
/// descriptors is rejected and every received descriptor is still closed.
pub fn read_frame_with_fds(
    stream: &UnixStream,
    max_fds: usize,
) -> Result<(Vec<u8>, Vec<OwnedFd>), CodecError> {
    read_frame_with_fds_until(stream, max_fds, None)
}

/// Read with an absolute deadline, including peers that slowly drip bytes.
pub fn read_frame_with_fds_timeout(
    stream: &UnixStream,
    max_fds: usize,
    timeout: std::time::Duration,
) -> Result<(Vec<u8>, Vec<OwnedFd>), CodecError> {
    let result =
        read_frame_with_fds_until(stream, max_fds, Some(std::time::Instant::now() + timeout));
    let _ = stream.set_read_timeout(None);
    result
}

fn remaining_timeout(stream: &UnixStream, deadline: Option<std::time::Instant>) -> io::Result<()> {
    if let Some(deadline) = deadline {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "IPC frame deadline exceeded")
            })?;
        stream.set_read_timeout(Some(remaining))?;
    }
    Ok(())
}

fn read_exact_until(
    mut stream: &UnixStream,
    mut bytes: &mut [u8],
    deadline: Option<std::time::Instant>,
) -> io::Result<()> {
    while !bytes.is_empty() {
        remaining_timeout(stream, deadline)?;
        match stream.read(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "IPC peer closed",
                ));
            }
            Ok(count) => bytes = &mut bytes[count..],
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn read_frame_with_fds_until(
    stream: &UnixStream,
    max_fds: usize,
    deadline: Option<std::time::Instant>,
) -> Result<(Vec<u8>, Vec<OwnedFd>), CodecError> {
    remaining_timeout(stream, deadline)?;
    let control_len = if max_fds == 0 {
        0
    } else {
        unsafe {
            libc::CMSG_SPACE(((max_fds + 1) * std::mem::size_of::<RawFd>()) as libc::c_uint)
                as usize
        }
    };
    let mut control = vec![0u8; control_len];
    // Read only the fixed header with recvmsg. That is enough to receive
    // ancillary data and, importantly, never consumes bytes belonging to a
    // later frame on a persistent stream.
    let mut length = [0u8; 4];
    let mut iov = [libc::iovec {
        iov_base: length.as_mut_ptr().cast(),
        iov_len: length.len(),
    }];

    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = iov.as_mut_ptr();
    message.msg_iovlen = iov.len() as _;
    if !control.is_empty() {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len() as _;
    }

    let received =
        unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if received == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "IPC peer closed").into());
    }

    let mut fds = Vec::new();
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data_len =
                    ((*header).cmsg_len as usize).saturating_sub(libc::CMSG_LEN(0) as usize);
                let count = data_len / std::mem::size_of::<RawFd>();
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                for index in 0..count {
                    fds.push(OwnedFd::from_raw_fd(*data.add(index)));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(io::Error::other("IPC descriptor message was truncated").into());
    }
    if fds.len() > max_fds {
        return Err(io::Error::other(format!(
            "IPC frame carried {} descriptors, maximum is {max_fds}",
            fds.len()
        ))
        .into());
    }

    let received = received as usize;
    if received < length.len() {
        read_exact_until(stream, &mut length[received..], deadline)?;
    }
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(length));
    }

    let mut bytes = vec![0u8; length];
    read_exact_until(stream, &mut bytes, deadline)?;
    Ok((bytes, fds))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    Ok(postcard::to_stdvec(value)?)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    Ok(postcard::from_bytes(bytes)?)
}

pub fn encode_request(req: &Request) -> Result<Vec<u8>, CodecError> {
    encode(req)
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, CodecError> {
    decode(bytes)
}

pub fn encode_response(resp: &Response) -> Result<Vec<u8>, CodecError> {
    encode(resp)
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, CodecError> {
    decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    #[test]
    fn frame_round_trips_through_a_byte_buffer() {
        let payload = b"hello frame".to_vec();
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let read_back = read_frame(&mut cursor).unwrap();
        assert_eq!(read_back, payload);
    }

    #[test]
    fn oversized_frame_is_rejected_on_write() {
        let huge = vec![0u8; MAX_FRAME_LEN + 1];
        assert!(matches!(
            write_frame(&mut Vec::new(), &huge),
            Err(CodecError::FrameTooLarge(_))
        ));
    }

    #[test]
    fn request_round_trips_through_postcard() {
        for req in [
            Request::Outputs,
            Request::Version,
            Request::Screenshot(crate::ScreenshotRequest {
                request_handle: "/org/freedesktop/portal/request/1".to_string(),
                target: crate::ScreenshotTarget::Area,
            }),
            Request::CancelScreenshot {
                request_handle: "/org/freedesktop/portal/request/1".to_string(),
            },
            Request::ChooseSource(crate::SourceChooserRequest {
                request_handle: "/org/freedesktop/portal/request/2".to_string(),
                source_types: crate::SOURCE_MONITOR | crate::SOURCE_WINDOW,
            }),
            Request::CaptureFrame(crate::CaptureFrameRequest {
                stream_handle: "/org/freedesktop/portal/session/1".to_string(),
                source: crate::CaptureSource::Monitor {
                    name: "DP-1".to_string(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                cursor_mode: crate::CursorMode::Metadata,
                buffer: crate::CaptureBuffer::MemFd {
                    fd_index: 0,
                    offset: 0,
                    size: 1920 * 1080 * 4,
                    stride: 1920 * 4,
                },
            }),
            Request::Node(crate::NodeRequest::Collapse {
                selector: Some(crate::NodeSelector::Id(42)),
                output: Some("DP-1".to_string()),
            }),
            Request::Node(crate::NodeRequest::Restore {
                selector: Some(crate::NodeSelector::Latest),
                output: None,
            }),
            Request::Node(crate::NodeRequest::Toggle {
                selector: Some(crate::NodeSelector::App("firefox".to_string())),
                output: None,
            }),
            Request::Bearings(crate::BearingsRequest::Toggle),
            Request::Quit,
            Request::ConfigPath,
            Request::Dpms {
                command: crate::DpmsCommand::Toggle,
                output: Some("DP-1".to_string()),
            },
            Request::CaptureCapabilities,
            Request::Trail(crate::TrailRequest::Goto {
                target: crate::TrailTarget::Index(2),
                output: Some("DP-1".to_string()),
            }),
            Request::LocalCapture(crate::LocalCaptureRequest {
                mode: crate::LocalCaptureMode::Window,
                output: Some("DP-2".to_string()),
            }),
            Request::Capture(crate::CaptureRequest::Menu { output: None }),
            Request::Capture(crate::CaptureRequest::Screen {
                output: Some("DP-1".into()),
            }),
            Request::Capture(crate::CaptureRequest::Window { node_id: 42 }),
            Request::Capture(crate::CaptureRequest::Region {
                output: Some("DP-2".into()),
                rect: Some(crate::CaptureRect {
                    x: 10,
                    y: 20,
                    width: 300,
                    height: 400,
                }),
            }),
            Request::CaptureWithOptions {
                target: crate::CaptureRequest::Window { node_id: 42 },
                silent: true,
            },
            Request::CaptureWithOptions {
                target: crate::CaptureRequest::Menu { output: None },
                silent: false,
            },
            Request::Control(crate::ControlRequest::TileSwap {
                direction: crate::ControlDirection::Left,
                output: None,
            }),
        ] {
            let bytes = encode_request(&req).unwrap();
            let decoded = decode_request(&bytes).unwrap();
            assert_eq!(format!("{decoded:?}"), format!("{req:?}"));
        }
    }

    #[test]
    fn distance_movement_is_appended_without_changing_legacy_node_encoding() {
        let legacy = crate::NodeRequest::Move {
            direction: crate::NodeMoveDirection::Left,
            selector: None,
            output: None,
        };
        assert_eq!(encode(&legacy).unwrap(), [3, 0, 0, 0]);
        let request = Request::Node(crate::NodeRequest::MoveBy {
            direction: crate::NodeMoveDirection::Down,
            distance: 10,
            selector: Some(crate::NodeSelector::Focused),
            output: Some("DP-1".into()),
        });
        let Request::Node(decoded) = decode_request(&encode_request(&request).unwrap()).unwrap()
        else {
            panic!("not a node request")
        };
        let Request::Node(expected) = request else {
            unreachable!()
        };
        assert_eq!(decoded, expected);
    }

    #[test]
    fn exact_movement_roundtrips_without_changing_distance_encoding() {
        assert_eq!(
            encode(&crate::NodeRequest::MoveBy {
                direction: crate::NodeMoveDirection::Right,
                distance: 10,
                selector: None,
                output: None,
            })
            .unwrap(),
            [8, 1, 10, 0, 0]
        );
        let request = Request::Node(crate::NodeRequest::MoveTo {
            x: -300,
            y: 200,
            selector: Some(crate::NodeSelector::Id(17)),
            output: Some("DP-2".into()),
        });
        let decoded = decode_request(&encode_request(&request).unwrap()).unwrap();
        let Request::Node(expected) = request else {
            unreachable!()
        };
        assert!(matches!(decoded, Request::Node(actual) if actual == expected));
    }

    #[test]
    fn direct_capture_extension_preserves_existing_request_indices() {
        // Version 23's local window request encoding must remain readable.
        let bytes = [20, 3, 1, 4, b'D', b'P', b'-', b'2'];
        let request = decode_request(&bytes).unwrap();
        assert!(
            matches!(&request, Request::LocalCapture(crate::LocalCaptureRequest {
            mode: crate::LocalCaptureMode::Window, output: Some(name),
        }) if name == "DP-2")
        );
        assert_eq!(encode_request(&request).unwrap(), bytes);
        // Version 24's direct window request is unchanged by the options extension.
        let direct = [22, 2, 42];
        let request = decode_request(&direct).unwrap();
        assert!(matches!(
            request,
            Request::Capture(crate::CaptureRequest::Window { node_id: 42 })
        ));
        assert_eq!(encode_request(&request).unwrap(), direct);
    }

    #[test]
    fn capture_capabilities_round_trip_through_postcard() {
        let response = Response::CaptureCapabilities(crate::CaptureCapabilities {
            main_device: Some(226 << 8 | 128),
            dmabuf_formats: vec![crate::DmabufFormat {
                fourcc: u32::from_le_bytes(*b"XR24"),
                modifier: 0,
            }],
        });
        let bytes = encode_response(&response).unwrap();
        let Response::CaptureCapabilities(decoded) = decode_response(&bytes).unwrap() else {
            panic!("wrong response variant");
        };
        assert_eq!(decoded.main_device, Some(226 << 8 | 128));
        assert_eq!(decoded.dmabuf_formats[0].modifier, 0);
    }

    #[test]
    fn responses_round_trip_through_postcard() {
        for (name, response) in [
            (
                "bearings_status",
                Response::BearingsStatus(crate::BearingsStatusResponse { visible: true }),
            ),
            (
                "config_path_response",
                Response::ConfigPath(Some("/home/test/.config/halley/halley.rune".to_string())),
            ),
            (
                "screenshot_response",
                Response::Screenshot(crate::ScreenshotResponse::Saved {
                    path: "/tmp/halley screenshot.png".to_string(),
                }),
            ),
            (
                "screencast_response",
                Response::Frame(crate::CaptureFrameResponse {
                    cursor: Some(crate::CursorMetadata {
                        x: 12,
                        y: 34,
                        hotspot_x: 2,
                        hotspot_y: 3,
                        width: 1,
                        height: 1,
                        bgra: vec![10, 20, 30, 255],
                    }),
                }),
            ),
            (
                "output_response",
                Response::Outputs(crate::OutputsResponse {
                    outputs: vec![crate::OutputInfo {
                        name: "DP-1".to_string(),
                        modes: vec![
                            crate::ModeInfo {
                                width: 2560,
                                height: 1440,
                                refresh_millihz: 179_998,
                                preferred: true,
                            },
                            crate::ModeInfo {
                                width: 1920,
                                height: 1080,
                                refresh_millihz: 60_000,
                                preferred: false,
                            },
                        ],
                        current_mode: Some(0),
                        offset_x: 0,
                        offset_y: 0,
                        scale: 1.0,
                        vrr: "auto".to_string(),
                        vrr_supported: true,
                        vrr_active: true,
                    }],
                }),
            ),
        ] {
            let bytes = encode_response(&response).unwrap();
            let decoded = decode_response(&bytes).unwrap();
            assert_eq!(format!("{decoded:?}"), format!("{response:?}"), "{name}");
        }
    }

    #[test]
    fn frame_and_descriptor_round_trip_together() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        let file = File::open("/dev/null").unwrap();

        write_frame_with_fds(&sender, b"buffer", &[file.as_raw_fd()]).unwrap();
        let (bytes, fds) = read_frame_with_fds(&receiver, 1).unwrap();

        assert_eq!(bytes, b"buffer");
        assert_eq!(fds.len(), 1);
        assert_ne!(fds[0].as_raw_fd(), file.as_raw_fd());
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn incomplete_frames_time_out_even_when_bytes_keep_arriving() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(&100u32.to_le_bytes()).unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..20 {
                if client.write_all(&[0]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let start = Instant::now();
        assert!(read_frame_with_fds_timeout(&server, 0, Duration::from_millis(60)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(server);
        writer.join().unwrap();
    }
}

#[cfg(test)]
mod truncated_descriptor_tests {
    use super::*;
    #[test]
    fn truncated_ancillary_messages_close_received_descriptors() {
        let name =
            std::ffi::CString::new(format!("halley-truncated-fds-{}", std::process::id())).unwrap();
        let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        assert!(raw >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let count = || {
            std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .filter_map(Result::ok)
                .filter_map(|entry| std::fs::read_link(entry.path()).ok())
                .filter(|path| path.to_string_lossy().contains(name.to_str().unwrap()))
                .count()
        };
        assert_eq!(count(), 1);
        let (server, client) = UnixStream::pair().unwrap();
        write_frame_with_fds(&client, b"test", &[fd.as_raw_fd(); 8]).unwrap();
        assert!(read_frame_with_fds(&server, 1).is_err());
        assert_eq!(count(), 1, "received descriptors leaked on truncation");
    }
}
