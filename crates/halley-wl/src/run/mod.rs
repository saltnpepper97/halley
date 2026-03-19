use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use halley_config::RuntimeTuning;

use eventline::{FileSetup, LogLevel, LogPolicy, RunHeader, Setup, info, scope, warn};
use once_cell::sync::OnceCell;

use crate::input::spawn_command;
use crate::state::HalleyWlState;

mod common;
mod ipc;

pub(crate) use common::{
    RuntimeBackend, auto_backend, ensure_dbus_session_bus_address, ensure_host_display,
    ensure_xdg_runtime_dir, ensure_xwayland_satellite,
};
pub(crate) use ipc::{RuntimeIpcCommand, drain_ipc_commands, init_ipc, publish_outputs};

static XWAYLAND_REQUEST_TX: OnceCell<mpsc::Sender<()>> = OnceCell::new();

// Set to true by the SIGTERM/SIGINT handler so the event loop can exit cleanly,
// allowing Drop impls (including the spawned-children cleanup) to run.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

pub(crate) struct LiveCameraState {
    viewport: halley_core::viewport::Viewport,
    zoom_ref_size: halley_core::field::Vec2,
    viewport_pan_anim: Option<crate::wm::ViewportPanAnim>,
}

extern "C" fn handle_shutdown_signal(_: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

pub(crate) fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

pub(crate) fn register_xwayland_request_channel(tx: mpsc::Sender<()>) {
    let _ = XWAYLAND_REQUEST_TX.set(tx);
}

pub(crate) fn request_xwayland_start() {
    if let Some(tx) = XWAYLAND_REQUEST_TX.get() {
        let _ = tx.send(());
    }
}

/// Spawns autostart commands and pushes the resulting Child handles into
/// `st.spawned_children` so they are tracked for cleanup on exit.
pub(crate) fn run_autostart_commands(
    st: &mut HalleyWlState,
    commands: &[String],
    wayland_display: &str,
    label: &str,
) {
    for command in commands {
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(child) = spawn_command(command, wayland_display, label) {
            st.spawned_children.push(child);
        }
    }
}

pub(crate) fn capture_live_camera_state(st: &mut HalleyWlState) -> LiveCameraState {
    LiveCameraState {
        viewport: st.viewport,
        zoom_ref_size: st.zoom_ref_size,
        viewport_pan_anim: st.viewport_pan_anim.take(),
    }
}

pub(crate) fn restore_live_camera_state(st: &mut HalleyWlState, state: LiveCameraState) {
    st.viewport = state.viewport;
    st.zoom_ref_size = state.zoom_ref_size;
    st.viewport_pan_anim = state.viewport_pan_anim;
    st.tuning.viewport_center = st.viewport.center;
    st.tuning.viewport_size = st.viewport.size;
}

pub(crate) fn apply_reloaded_tuning(
    st: &mut HalleyWlState,
    next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
) {
    let live_camera = capture_live_camera_state(st);
    st.apply_tuning(next);
    restore_live_camera_state(st, live_camera);
    // Clone to avoid borrow conflict when passing st mutably below.
    let reload_commands = st.tuning.autostart_on_reload.clone();
    run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    info!("{reason}: reloaded config from {}", config_path);
}

fn normalized_tty_viewports(
    tuning: &RuntimeTuning,
) -> Vec<(String, i32, i32, u32, u32, Option<i64>)> {
    let mut out: Vec<_> = tuning
        .tty_viewports
        .iter()
        .map(|viewport| {
            let refresh_millihz = viewport.refresh_rate.map(|hz| (hz * 1000.0).round() as i64);
            (
                viewport.connector.clone(),
                viewport.offset_x,
                viewport.offset_y,
                viewport.width,
                viewport.height,
                refresh_millihz,
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub(crate) fn viewport_section_changed(prev: &RuntimeTuning, next: &RuntimeTuning) -> bool {
    normalized_tty_viewports(prev) != normalized_tty_viewports(next)
}

pub(crate) fn preserve_viewport_section(
    prev: &RuntimeTuning,
    mut next: RuntimeTuning,
) -> RuntimeTuning {
    next.viewport_center = prev.viewport_center;
    next.viewport_size = prev.viewport_size;
    next.tty_viewports = prev.tty_viewports.clone();
    next
}

pub fn run() -> Result<(), Box<dyn Error>> {
    // Register signal handlers before anything else so that SIGTERM (the
    // default signal sent by `pkill`/`kill`) triggers a clean shutdown.
    // This lets Drop run, which kills all spawned child process groups.
    // Note: SIGKILL (-9) cannot be caught — use plain `pkill` for clean exit.
    unsafe {
        let handler = handle_shutdown_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }

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
    crate::backend::winit::run_winit_backend()
}

pub fn run_tty() -> Result<(), Box<dyn Error>> {
    crate::backend::tty::run_tty_backend()
}

pub(crate) fn init_logging() -> Result<(), Box<dyn Error>> {
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
