use std::fs;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use once_cell::sync::OnceCell;

use eventline::{error, info, warn};
use halley_ipc::{
    DockingCommand, IpcError, NodeMoveDirection, OutputInfo, OutputsResponse, Request, Response,
    decode_request, encode_response, read_frame, write_frame,
};

#[derive(Debug, Clone, Copy)]
pub enum RuntimeIpcCommand {
    Quit,
    Reload,
    Docking(DockingCommand),
    NodeMove(NodeMoveDirection),
}

static IPC_COMMAND_RX: OnceCell<Mutex<mpsc::Receiver<RuntimeIpcCommand>>> = OnceCell::new();
static IPC_OUTPUTS: OnceCell<Arc<Mutex<Vec<OutputInfo>>>> = OnceCell::new();

pub fn init_ipc() -> io::Result<()> {
    if IPC_COMMAND_RX.get().is_some() {
        return Ok(());
    }

    let socket_path = halley_ipc::default_socket_path()?;
    remove_stale_socket(&socket_path)?;

    let listener = UnixListener::bind(&socket_path)?;
    let (command_tx, command_rx) = mpsc::channel::<RuntimeIpcCommand>();
    let outputs = Arc::new(Mutex::new(Vec::<OutputInfo>::new()));

    IPC_COMMAND_RX.set(Mutex::new(command_rx)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "IPC command receiver already initialized",
        )
    })?;

    IPC_OUTPUTS.set(outputs.clone()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "IPC outputs store already initialized",
        )
    })?;

    thread::Builder::new()
        .name("halley-ipc".to_string())
        .spawn(move || {
            info!("halley ipc listening on {}", socket_path.display());

            for stream in listener.incoming() {
                match stream {
                    Ok(mut stream) => {
                        if let Err(err) = handle_client(&mut stream, &command_tx, &outputs) {
                            warn!("halley ipc client failed: {}", err);
                        }
                    }
                    Err(err) => {
                        error!("halley ipc accept failed: {}", err);
                        break;
                    }
                }
            }

            let _ = fs::remove_file(&socket_path);
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
    F: FnMut(RuntimeIpcCommand),
{
    let Some(rx) = IPC_COMMAND_RX.get() else {
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
            Ok(cmd) => f(cmd),
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
}

fn handle_client(
    stream: &mut UnixStream,
    command_tx: &mpsc::Sender<RuntimeIpcCommand>,
    outputs: &Arc<Mutex<Vec<OutputInfo>>>,
) -> io::Result<()> {
    let response = match read_frame(stream).and_then(|bytes| decode_request(&bytes)) {
        Ok(request) => handle_request(request, command_tx, outputs),
        Err(err) => Response::Error(IpcError::InvalidRequest(err.to_string())),
    };

    let response_bytes = encode_response(&response).map_err(io::Error::other)?;
    write_frame(stream, &response_bytes).map_err(io::Error::other)
}

fn handle_request(
    request: Request,
    command_tx: &mpsc::Sender<RuntimeIpcCommand>,
    outputs: &Arc<Mutex<Vec<OutputInfo>>>,
) -> Response {
    match request {
        Request::Quit => match command_tx.send(RuntimeIpcCommand::Quit) {
            Ok(()) => Response::Ok,
            Err(err) => Response::Error(IpcError::Internal(err.to_string())),
        },
        Request::Reload => match command_tx.send(RuntimeIpcCommand::Reload) {
            Ok(()) => Response::Reloaded,
            Err(err) => Response::Error(IpcError::Internal(err.to_string())),
        },
        Request::Docking(command) => match command_tx.send(RuntimeIpcCommand::Docking(command)) {
            Ok(()) => Response::Ok,
            Err(err) => Response::Error(IpcError::Internal(err.to_string())),
        },
        Request::NodeMove(direction) => {
            match command_tx.send(RuntimeIpcCommand::NodeMove(direction)) {
                Ok(()) => Response::Ok,
                Err(err) => Response::Error(IpcError::Internal(err.to_string())),
            }
        }
        Request::Outputs => match outputs.lock() {
            Ok(guard) => Response::Outputs(OutputsResponse {
                outputs: guard.clone(),
            }),
            Err(err) => Response::Error(IpcError::Internal(err.to_string())),
        },
    }
}
fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
