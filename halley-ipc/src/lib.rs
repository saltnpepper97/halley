mod codec;
mod types;

use std::fs;
use std::io;
use std::os::fd::{OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use codec::{
    CodecError, decode_request, decode_response, encode_request, encode_response, read_frame,
    read_frame_with_fds, read_frame_with_fds_timeout, write_frame, write_frame_with_fds,
};
pub use types::{
    ApiEvent, BearingsRequest, BearingsStatusResponse, CaptureBuffer, CaptureCapabilities,
    CaptureFrameRequest, CaptureFrameResponse, CaptureRect, CaptureRequest, CaptureSource,
    ClusterDraftAppLaunch, ClusterDraftRequest, ClusterDraftSource, ClusterDraftState, ClusterInfo,
    ClusterLayoutKind, ClusterListResponse, ClusterOutputGroup, ClusterRequest, ClusterSummary,
    ClusterTarget, ControlDirection, ControlRequest, CursorMetadata, CursorMode, DmabufFormat,
    DmabufPlane, DpmsCommand, EventTopic, HALLEY_API_VERSION, HALLEY_IPC_VERSION, HelloRequest,
    LocalCaptureMode, LocalCaptureRequest, ModeInfo, MonitorFocusTarget, NodeInfo, NodeKind,
    NodeListResponse, NodeMoveDirection, NodeOutputGroup, NodeProtocolFamily, NodeRelationInfo,
    NodeRequest, NodeRole, NodeSelector, NodeState, OutputInfo, OutputsResponse,
    RegisterDmabufRequest, Request, Response, SOURCE_MONITOR, SOURCE_WINDOW, ScreenshotRequest,
    ScreenshotResponse, ScreenshotTarget, ServerError, ServerErrorKind, ServerInfo,
    SourceChooserRequest, SourceChooserResponse, StackCycleDirection, StateSnapshot,
    SubscribeRequest, TrailEntryInfo, TrailListResponse, TrailRequest, TrailTarget, VersionInfo,
};

fn runtime_dir_from(base: impl AsRef<Path>) -> PathBuf {
    base.as_ref().join("halley")
}

/// `$XDG_RUNTIME_DIR/halley`, shared by Halley's IPC socket and runtime log.
/// Resolving the path has no filesystem side effects.
pub fn halley_runtime_dir() -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    Ok(runtime_dir_from(PathBuf::from(base)))
}

fn ensure_runtime_dir_at(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Creates Halley's private runtime directory and enforces owner-only access.
pub fn ensure_runtime_dir() -> io::Result<PathBuf> {
    let path = halley_runtime_dir()?;
    ensure_runtime_dir_at(&path)?;
    Ok(path)
}

/// `$XDG_RUNTIME_DIR/halley/halley.sock` - matches old halley's own socket
/// path convention. No fallback for a missing `XDG_RUNTIME_DIR`: any real
/// Wayland session already requires it (the Wayland socket itself lives
/// there too), so a missing one means something more fundamental is wrong,
/// not a case worth a synthetic `/tmp` fallback for.
pub fn default_socket_path() -> io::Result<PathBuf> {
    Ok(halley_runtime_dir()?.join("halley.sock"))
}

/// A decoded response and any descriptors whose ownership was transferred
/// with it.
#[derive(Debug)]
pub struct ResponseEnvelope {
    pub response: Response,
    pub fds: Vec<OwnedFd>,
}

/// A reusable connection to the compositor.
pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    pub fn connect() -> Result<Self, CodecError> {
        Self::connect_to(&default_socket_path()?)
    }

    pub fn connect_to(path: &Path) -> Result<Self, CodecError> {
        Ok(Self {
            stream: UnixStream::connect(path)?,
        })
    }

    pub fn request(
        &mut self,
        request: &Request,
        fds: &[RawFd],
    ) -> Result<ResponseEnvelope, CodecError> {
        let bytes = encode_request(request)?;
        write_frame_with_fds(&self.stream, &bytes, fds)?;
        let (bytes, fds) = read_frame_with_fds(&self.stream, 32)?;
        Ok(ResponseEnvelope {
            response: decode_response(&bytes)?,
            fds,
        })
    }

    pub fn receive(&mut self) -> Result<ResponseEnvelope, CodecError> {
        let (bytes, fds) = read_frame_with_fds(&self.stream, 32)?;
        Ok(ResponseEnvelope {
            response: decode_response(&bytes)?,
            fds,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }
}

/// Connects to the running compositor, sends one descriptor-free request,
/// and returns the descriptor-free response used by `halleyctl`.
pub fn send_request(req: &Request) -> Result<Response, CodecError> {
    let path = default_socket_path()?;
    send_request_to(&path, req)
}

pub fn send_request_to(path: &Path, req: &Request) -> Result<Response, CodecError> {
    let mut connection = Connection::connect_to(path)?;
    let envelope = connection.request(req, &[])?;
    if !envelope.fds.is_empty() {
        return Err(io::Error::other("unexpected descriptors in IPC response").into());
    }
    Ok(envelope.response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "halley-ipc-runtime-test-{}-{unique}",
                std::process::id()
            )))
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn runtime_and_socket_paths_share_the_halley_directory() {
        let runtime = runtime_dir_from("/run/user/1000");
        assert_eq!(runtime, PathBuf::from("/run/user/1000/halley"));
        assert_eq!(
            runtime.join("halley.sock"),
            PathBuf::from("/run/user/1000/halley/halley.sock")
        );
        assert_eq!(
            runtime.join("halley.log"),
            PathBuf::from("/run/user/1000/halley/halley.log")
        );
    }

    #[test]
    fn runtime_directory_is_private() {
        let scratch = ScratchDir::new();
        let runtime = scratch.0.join("nested").join("halley");

        ensure_runtime_dir_at(&runtime).unwrap();

        assert!(runtime.is_dir());
        assert_eq!(
            fs::metadata(runtime).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn connection_reuses_one_stream_for_sequential_requests() {
        let scratch = ScratchDir::new();
        fs::create_dir_all(&scratch.0).unwrap();
        let path = scratch.0.join("persistent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            for version in ["first", "second"] {
                let (bytes, fds) = read_frame_with_fds(&stream, 0).unwrap();
                assert!(fds.is_empty());
                assert!(matches!(decode_request(&bytes).unwrap(), Request::Version));
                let response = Response::Version(VersionInfo {
                    version: version.to_string(),
                    ipc_protocol: HALLEY_IPC_VERSION,
                });
                write_frame_with_fds(&stream, &encode_response(&response).unwrap(), &[]).unwrap();
            }
        });

        let mut connection = Connection::connect_to(&path).unwrap();
        for expected in ["first", "second"] {
            let envelope = connection.request(&Request::Version, &[]).unwrap();
            assert!(envelope.fds.is_empty());
            let Response::Version(version) = envelope.response else {
                panic!("expected version response");
            };
            assert_eq!(version.version, expected);
        }
        server.join().unwrap();
    }
}
