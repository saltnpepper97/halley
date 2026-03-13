use std::cell::{Cell, RefCell};
use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use halley_config::RuntimeTuning;

use calloop::EventLoop;
use calloop::timer::{TimeoutAction, Timer};

use eventline::{FileSetup, LogLevel, LogPolicy, RunHeader, Setup, debug, info, scope, warn};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::OnceCell;

use smithay::reexports::drm::control::{self as drm_control, Device as DrmControlDevice};
use smithay::{
    backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
    backend::allocator::{Format, Fourcc},
    backend::drm::DrmEvent,
    backend::drm::GbmBufferedSurface,
    backend::drm::{DrmDevice, DrmDeviceFd},
    backend::egl::{EGLContext, EGLDisplay},
    backend::input::{
        AbsolutePositionEvent, Axis, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent,
    },
    backend::libinput::LibinputInputBackend,
    backend::libinput::LibinputSessionInterface,
    backend::renderer::gles::GlesRenderer,
    backend::renderer::{Bind, ImportDma},
    backend::session::libseat::LibSeatSession,
    backend::session::{Event as SessionEvent, Session},
    backend::udev::{all_gpus, primary_gpu},
    backend::winit::{self, WinitEvent},
    reexports::input::Libinput,
    reexports::wayland_server::Display,
    utils::{DeviceFd, Transform},
    wayland::socket::ListeningSocketSource,
};

use crate::activity::VisualState;
use crate::backend_iface::{BackendView, RenderBackend, WinitBackendHandle};
use crate::interaction::types::{ModState, PointerState};
use crate::state::{ClientState, HalleyWlState};

use crate::input::{
    BackendInputEventData, advance_node_move_anim, handle_backend_input_event, spawn_command,
};
use crate::render::draw_debug_frame_to_target;
use crate::surface::current_surface_size_for_node;

mod common;
mod drm;
mod input_backend;
mod ipc;
mod tty_backend;
mod winit_backend;

use common::{
    RuntimeBackend, auto_backend, ensure_dbus_session_bus_address, ensure_host_display,
    ensure_xdg_runtime_dir, ensure_xwayland_satellite,
};
pub(crate) use common::{run_autostart_commands, spawn_shell_command};
use drm::probe_tty_drm_device_via_session;
use input_backend::build_tty_libinput_backend;
pub(crate) use ipc::{RuntimeIpcCommand, drain_ipc_commands, init_ipc, publish_outputs};

static XWAYLAND_REQUEST_TX: OnceCell<mpsc::Sender<()>> = OnceCell::new();

pub(crate) fn register_xwayland_request_channel(tx: mpsc::Sender<()>) {
    let _ = XWAYLAND_REQUEST_TX.set(tx);
}

pub(crate) fn request_xwayland_start() {
    if let Some(tx) = XWAYLAND_REQUEST_TX.get() {
        let _ = tx.send(());
    }
}

#[derive(Clone)]
struct TtyBackendHandle {
    size: Rc<Cell<(i32, i32)>>,
}

impl TtyBackendHandle {
    fn new(width: i32, height: i32) -> Self {
        Self {
            size: Rc::new(Cell::new((width, height))),
        }
    }

    fn set_size(&self, width: i32, height: i32) {
        self.size.set((width, height));
    }
}

impl BackendView for TtyBackendHandle {
    fn window_size_i32(&self) -> (i32, i32) {
        self.size.get()
    }

    fn request_redraw(&self) {}
}

pub fn run() -> Result<(), Box<dyn Error>> {
    init_ipc()?;

    match RuntimeBackend::from_env()? {
        RuntimeBackend::Auto => match auto_backend() {
            RuntimeBackend::Tty => run_tty(),
            RuntimeBackend::Winit | RuntimeBackend::Auto => run_winit(),
        },
        RuntimeBackend::Winit => run_winit(),
        RuntimeBackend::Tty => run_tty(),
    }
}

pub fn run_winit() -> Result<(), Box<dyn Error>> {
    winit_backend::run_winit_backend()
}

pub fn run_tty() -> Result<(), Box<dyn Error>> {
    tty_backend::run_tty_backend()
}

fn init_logging() -> Result<(), Box<dyn Error>> {
    scope!("logging-init", success = "ready", {
        let level = env::var("HALLEY_WL_LOG")
            .ok()
            .and_then(|v| parse_log_level(v.as_str()))
            .unwrap_or(LogLevel::Info);

        let log_file = configured_halley_log_file();
        let file = match log_file.as_ref() {
            Some(None) => Some(FileSetup::Off),
            Some(Some(path)) => Some(FileSetup::Rotating {
                path: path.clone(),
                policy: LogPolicy::default(),
                header: Some(RunHeader::new("halley-wl")),
            }),
            None => default_halley_log_path().map(|path| FileSetup::Rotating {
                path,
                policy: LogPolicy::default(),
                header: Some(RunHeader::new("halley-wl")),
            }),
        };

        if let Err(err) = pollster::block_on(eventline::setup(Setup {
            verbose: true,
            level: Some(level),
            file,
        })) {
            warn!("failed to configure logging: {}", err);
        }

        eventline::enable_console_color(true);
        eventline::enable_console_duration(true);

        match log_file {
            Some(None) => info!("file logging disabled via HALLEY_WL_LOG_FILE"),
            Some(Some(path)) => info!("file logging enabled: {}", path.display()),
            None => {
                if let Some(path) = default_halley_log_path() {
                    info!("file logging enabled: {}", path.display());
                }
            }
        }

        Ok(())
    })
}

fn parse_log_level(raw: &str) -> Option<LogLevel> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "trace" | "debug" => Some(LogLevel::Debug),
        "info" => Some(LogLevel::Info),
        "warn" | "warning" => Some(LogLevel::Warning),
        "error" => Some(LogLevel::Error),
        "off" => Some(LogLevel::Off),
        _ => None,
    }
}

fn configured_halley_log_file() -> Option<Option<PathBuf>> {
    let raw = env::var("HALLEY_WL_LOG_FILE").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if matches!(trimmed.to_ascii_lowercase().as_str(), "off" | "false" | "0") {
        return Some(None);
    }
    Some(Some(PathBuf::from(trimmed)))
}

fn default_halley_log_path() -> Option<PathBuf> {
    env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|dir| Path::new(dir.as_str()).join("halley").join("halley.log"))
        .or_else(|| {
            Some(
                PathBuf::from(format!(
                    "/tmp/halley-{}",
                    rustix::process::getuid().as_raw()
                ))
                .join("halley.log"),
            )
        })
}
