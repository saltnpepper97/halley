use std::cell::RefCell;
use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use halley_config::RuntimeTuning;

use calloop::EventLoop;
use calloop::timer::{TimeoutAction, Timer};

use eventline::{debug, info, scope, warn};
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
    backend::renderer::gles::GlesRenderer,
    backend::renderer::{Bind, ImportDma},
    backend::udev::{all_gpus, primary_gpu},
    backend::winit::{self, WinitEvent},
    reexports::input::{Libinput, LibinputInterface},
    reexports::wayland_server::Display,
    utils::{DeviceFd, Transform},
    wayland::socket::ListeningSocketSource,
};
#[cfg(feature = "session-libseat")]
use smithay::{
    backend::libinput::LibinputSessionInterface,
    backend::session::libseat::LibSeatSession,
    backend::session::{Event as SessionEvent, Session},
};

use crate::activity::VisualState;
use crate::backend_iface::{BackendView, RenderBackend, WinitBackendHandle};
use crate::interaction::types::{ModState, PointerState};
use crate::state::{ClientState, HalleyWlState};

use crate::input::{BackendInputEventData, advance_node_move_anim, handle_backend_input_event};
use crate::runtime_render::draw_debug_frame_to_target;
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
#[cfg(feature = "session-libseat")]
use drm::probe_tty_drm_device_via_session;
pub(crate) use ipc::{RuntimeIpcCommand, drain_ipc_commands, init_ipc, publish_outputs};
#[cfg(feature = "session-libseat")]
use input_backend::build_tty_libinput_backend;

static XWAYLAND_REQUEST_TX: OnceCell<mpsc::Sender<()>> = OnceCell::new();

pub(crate) fn register_xwayland_request_channel(tx: mpsc::Sender<()>) {
    let _ = XWAYLAND_REQUEST_TX.set(tx);
}

pub(crate) fn request_xwayland_start() {
    if let Some(tx) = XWAYLAND_REQUEST_TX.get() {
        let _ = tx.send(());
    }
}

#[derive(Clone, Copy)]
struct TtyBackendHandle {
    width: i32,
    height: i32,
}

impl BackendView for TtyBackendHandle {
    fn window_size_i32(&self) -> (i32, i32) {
        (self.width, self.height)
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
        pollster::block_on(eventline::runtime::init());

        eventline::runtime::enable_console_output(true);
        eventline::runtime::enable_console_color(true);
        eventline::runtime::enable_console_duration(true);
        let level = env::var("HALLEY_WL_LOG")
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .and_then(|v| match v.as_str() {
                "trace" => Some(eventline::runtime::LogLevel::Debug),
                "debug" => Some(eventline::runtime::LogLevel::Debug),
                "info" => Some(eventline::runtime::LogLevel::Info),
                "warn" | "warning" => Some(eventline::runtime::LogLevel::Warning),
                "error" => Some(eventline::runtime::LogLevel::Error),
                "off" => Some(eventline::runtime::LogLevel::Off),
                _ => None,
            })
            .unwrap_or(eventline::runtime::LogLevel::Info);
        eventline::runtime::set_log_level(level);

        let log_file = env::var("HALLEY_WL_LOG_FILE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(default_halley_log_path);
        if let Some(path) = log_file {
            if let Some(parent) = Path::new(path.as_str()).parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    warn!(
                        "failed to create log directory {}: {}",
                        parent.display(),
                        err
                    );
                }
            }
            if let Err(err) = eventline::runtime::enable_file_output(path.as_str()) {
                warn!("failed to enable file logging at {}: {}", path, err);
            } else {
                info!("file logging enabled: {}", path);
            }
        }

        Ok(())
    })
}

fn default_halley_log_path() -> Option<String> {
    let state_home = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var("HOME")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|home| Path::new(home.as_str()).join(".local/state"))
        });
    if let Some(base) = state_home {
        return Some(
            base.join("halley")
                .join("halley.log")
                .to_string_lossy()
                .to_string(),
        );
    }
    env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|dir| {
            Path::new(dir.as_str())
                .join("halley-wl.log")
                .to_string_lossy()
                .to_string()
        })
        .or_else(|| {
            Some(
                PathBuf::from(format!(
                    "/tmp/halley-wl-{}.log",
                    rustix::process::getuid().as_raw()
                ))
                .to_string_lossy()
                .to_string(),
            )
        })
}
