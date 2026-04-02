use std::fs;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use once_cell::sync::OnceCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::bootstrap::common::halley_runtime_dir;
use eventline::{error, info, warn};
use halley_ipc::{
    IpcError, OutputInfo, OutputsResponse, Request, Response, decode_request, encode_response,
    read_frame, write_frame,
};

#[derive(Debug)]
pub struct RuntimeIpcRequest {
    pub request: Request,
    pub reply_tx: mpsc::Sender<Response>,
}

static IPC_REQUEST_RX: OnceCell<Mutex<mpsc::Receiver<RuntimeIpcRequest>>> = OnceCell::new();
static IPC_OUTPUTS: OnceCell<Arc<Mutex<Vec<OutputInfo>>>> = OnceCell::new();
static IPC_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static IPC_SOCKET_PATH: OnceCell<std::path::PathBuf> = OnceCell::new();

pub fn init_ipc() -> io::Result<()> {
    if IPC_REQUEST_RX.get().is_some() {
        return Ok(());
    }

    let socket_path = halley_runtime_dir()?.join("socket");
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    remove_stale_socket(&socket_path)?;

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    let (request_tx, request_rx) = mpsc::channel::<RuntimeIpcRequest>();
    let outputs = Arc::new(Mutex::new(Vec::<OutputInfo>::new()));

    IPC_REQUEST_RX.set(Mutex::new(request_rx)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "IPC request receiver already initialized",
        )
    })?;

    IPC_OUTPUTS.set(outputs.clone()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "IPC outputs store already initialized",
        )
    })?;

    let _ = IPC_SOCKET_PATH.set(socket_path.clone());
    IPC_SHUTDOWN_REQUESTED.store(false, Ordering::Relaxed);

    thread::Builder::new()
        .name("halley-ipc".to_string())
        .spawn(move || {
            info!("halley ipc listening on {}", socket_path.display());

            loop {
                if IPC_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
                    break;
                }

                match listener.accept() {
                    Ok((mut stream, _addr)) => {
                        if let Err(err) = handle_client(&mut stream, &request_tx, &outputs) {
                            warn!("halley ipc client failed: {}", err);
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(err) => {
                        error!("halley ipc accept failed: {}", err);
                        break;
                    }
                }
            }

            let _ = fs::remove_file(&socket_path);
            info!("halley ipc listener stopped");
        })?;

    Ok(())
}

pub fn publish_outputs(outputs: Vec<OutputInfo>) {
    let Some(store) = IPC_OUTPUTS.get() else {
        return;
    };

    match store.lock() {
        Ok(mut guard) => {
            *guard = outputs;
        }
        Err(err) => {
            warn!("halley ipc outputs lock poisoned: {}", err);
        }
    }
}

pub fn drain_ipc_commands<F>(mut f: F)
where
    F: FnMut(Request) -> Response,
{
    let Some(rx) = IPC_REQUEST_RX.get() else {
        return;
    };

    let guard = match rx.lock() {
        Ok(guard) => guard,
        Err(err) => {
            warn!("halley ipc command receiver lock poisoned: {}", err);
            return;
        }
    };

    loop {
        match guard.try_recv() {
            Ok(request) => {
                let response = f(request.request);
                let _ = request.reply_tx.send(response);
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
}

fn handle_client(
    stream: &mut UnixStream,
    request_tx: &mpsc::Sender<RuntimeIpcRequest>,
    outputs: &Arc<Mutex<Vec<OutputInfo>>>,
) -> io::Result<()> {
    let response = match read_frame(stream).and_then(|bytes| decode_request(&bytes)) {
        Ok(request) => handle_request(request, request_tx, outputs),
        Err(err) => Response::Error(IpcError::InvalidRequest(err.to_string())),
    };

    let response_bytes = encode_response(&response).map_err(io::Error::other)?;
    write_frame(stream, &response_bytes).map_err(io::Error::other)
}

fn handle_request(
    request: Request,
    request_tx: &mpsc::Sender<RuntimeIpcRequest>,
    outputs: &Arc<Mutex<Vec<OutputInfo>>>,
) -> Response {
    match request {
        Request::Compositor(halley_ipc::CompositorRequest::Outputs) => match outputs.lock() {
            Ok(guard) => Response::Outputs(OutputsResponse {
                outputs: guard.clone(),
            }),
            Err(err) => Response::Error(IpcError::Internal(err.to_string())),
        },
        request => {
            let (reply_tx, reply_rx) = mpsc::channel();
            let envelope = RuntimeIpcRequest { request, reply_tx };
            if let Err(err) = request_tx.send(envelope) {
                return Response::Error(IpcError::Internal(err.to_string()));
            }
            match reply_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(response) => response,
                Err(err) => Response::Error(IpcError::Internal(format!(
                    "timed out waiting for compositor response: {err}"
                ))),
            }
        }
    }
}
fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn shutdown_ipc() {
    IPC_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
    if let Some(path) = IPC_SOCKET_PATH.get() {
        let _ = fs::remove_file(path);
    }
}
