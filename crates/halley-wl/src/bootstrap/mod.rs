use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use halley_config::{ConfigLoadDiagnostic, RuntimeTuning, ViewportOutputConfig};

use eventline::{
    FileSetup, LogLevel, LogPolicy, RunHeader, Setup, debug, enable_console_color,
    enable_console_duration, info, scope, warn,
};
use halley_core::field::Vec2;
use halley_core::viewport::Viewport;
use rustix::process::Signal;
use rustix::runtime::{KernelSigSet, KernelSigaction, kernel_sigaction};

use crate::compositor::interaction::state::ViewportPanAnim;
use crate::compositor::root::Halley;
use crate::input::spawn_command;

mod common;
mod ipc;

pub(crate) use common::{
    RuntimeBackend, XwaylandSatellite, auto_backend, ensure_dbus_session_bus_address,
    ensure_host_display, ensure_xdg_runtime_dir, ensure_xwayland_satellite, halley_runtime_dir,
    refresh_portal_services_nonblocking, sync_portal_activation_environment,
};
pub(crate) use ipc::{drain_ipc_commands, init_ipc, publish_outputs, shutdown_ipc};

// Set to true by the SIGTERM/SIGINT handler so the event loop can exit cleanly,
// allowing Drop impls (including the spawned-children cleanup) to run.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub(crate) struct LiveCameraState {
    viewport: Viewport,
    zoom_ref_size: Vec2,
    camera_target_center: Vec2,
    camera_target_view_size: Vec2,
    viewport_pan_anim: Option<ViewportPanAnim>,
    monitors: HashMap<String, LiveMonitorCameraState>,
}

#[derive(Clone, Copy)]
struct LiveMonitorCameraState {
    viewport: Viewport,
    zoom_ref_size: Vec2,
    camera_target_center: Vec2,
    camera_target_view_size: Vec2,
}

unsafe extern "C" fn handle_shutdown_signal(_: rustix::ffi::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

pub(crate) fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

/// Spawns autostart commands and pushes the resulting Child handles into
/// `st.runtime.spawned_children` so they are tracked for cleanup on exit.
pub(crate) fn run_autostart_commands(
    st: &mut Halley,
    commands: &[String],
    wayland_display: &str,
    label: &str,
) {
    for command in commands {
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(child) = spawn_command(
            command,
            wayland_display,
            &st.runtime.tuning.cursor,
            None,
            label,
        ) {
            st.runtime.spawned_children.push(child);
        }
    }
}

pub(crate) fn capture_live_camera_state(st: &mut Halley) -> LiveCameraState {
    LiveCameraState {
        viewport: st.model.viewport,
        zoom_ref_size: st.model.zoom_ref_size,
        camera_target_center: st.model.camera_target_center,
        camera_target_view_size: st.model.camera_target_view_size,
        viewport_pan_anim: st.input.interaction_state.viewport_pan_anim.take(),
        monitors: st
            .model
            .monitor_state
            .monitors
            .iter()
            .map(|(name, space)| {
                (
                    name.clone(),
                    LiveMonitorCameraState {
                        viewport: space.viewport,
                        zoom_ref_size: space.zoom_ref_size,
                        camera_target_center: space.camera_target_center,
                        camera_target_view_size: space.camera_target_view_size,
                    },
                )
            })
            .collect(),
    }
}

pub(crate) fn restore_live_camera_state(st: &mut Halley, state: LiveCameraState) {
    st.model.viewport = state.viewport;
    st.model.zoom_ref_size = state.zoom_ref_size;
    st.model.camera_target_center = state.camera_target_center;
    st.model.camera_target_view_size = state.camera_target_view_size;
    st.input.interaction_state.viewport_pan_anim = state.viewport_pan_anim;

    for (name, camera) in state.monitors {
        if let Some(space) = st.model.monitor_state.monitors.get_mut(name.as_str()) {
            space.viewport = camera.viewport;
            space.zoom_ref_size = camera.zoom_ref_size;
            space.camera_target_center = camera.camera_target_center;
            space.camera_target_view_size = camera.camera_target_view_size;
        }
    }
}

pub(crate) fn apply_reloaded_tuning(
    st: &mut Halley,
    next: RuntimeTuning,
    config_path: &str,
    wayland_display: &str,
    reason: &str,
) {
    let live_camera = capture_live_camera_state(st);
    st.apply_tuning(next);
    restore_live_camera_state(st, live_camera);
    let opacity_changed = crate::compositor::spawn::state::recompute_all_node_rule_opacities(st);
    st.ui.render_state.clear_window_offscreen_caches();
    st.request_maintenance();
    if opacity_changed {
        st.runtime.tty_redraw_all = true;
    }
    // Clone to avoid borrow conflict when passing st mutably below.
    let reload_commands = st.runtime.tuning.autostart_on_reload.clone();
    run_autostart_commands(st, &reload_commands, wayland_display, "autostart");
    debug!("{reason}: reloaded config from {}", config_path);
    debug!(
        "resolved zoom: {}",
        st.runtime.tuning.zoom_resolved_summary()
    );
}

pub(crate) fn load_startup_tuning(path: &str) -> (RuntimeTuning, Option<ConfigLoadDiagnostic>) {
    match RuntimeTuning::try_load_from_path_diagnostic(path) {
        Ok(tuning) => (tuning, None),
        Err(diagnostic) => {
            warn!(
                "config load failed at startup from {}: {}; using built-in defaults",
                path, diagnostic.message
            );
            (RuntimeTuning::builtin_defaults(), Some(diagnostic))
        }
    }
}

pub(crate) fn show_config_startup_error(st: &mut Halley, diagnostic: &ConfigLoadDiagnostic) {
    show_config_error_with_title(st, diagnostic, "Config load failed", 15_000);
}

pub(crate) fn show_config_reload_error(st: &mut Halley, diagnostic: &ConfigLoadDiagnostic) {
    show_config_error_with_title(st, diagnostic, "Config reload failed", 9000);
}

fn show_config_error_with_title(
    st: &mut Halley,
    diagnostic: &ConfigLoadDiagnostic,
    title: &str,
    duration_ms: u64,
) {
    let monitor = st.model.monitor_state.current_monitor.clone();
    let now_ms = st.now_ms(std::time::Instant::now());
    let message = config_error_message(diagnostic, title);
    st.ui.render_state.show_overlay_error_toast(
        monitor.as_str(),
        message.as_str(),
        duration_ms,
        now_ms,
    );
}

fn config_error_message(diagnostic: &ConfigLoadDiagnostic, title: &str) -> String {
    let location = match (diagnostic.line, diagnostic.column) {
        (Some(line), Some(column)) => format!("{}:{line}:{column}", diagnostic.path),
        (Some(line), None) => format!("{}:{line}", diagnostic.path),
        _ => diagnostic.path.clone(),
    };
    let mut lines = vec![title.to_string(), location, diagnostic.message.clone()];
    if let Some(source_line) = diagnostic.source_line.as_deref() {
        lines.push(format!("-> {source_line}"));
    } else if let Some(hint) = diagnostic.hint.as_deref() {
        lines.push(format!("Hint: {hint}"));
    }
    lines.join("\n")
}

fn normalized_tty_viewports(
    tuning: &RuntimeTuning,
) -> Vec<(
    String,
    bool,
    i32,
    i32,
    u32,
    u32,
    Option<i64>,
    u16,
    &'static str,
)> {
    let mut out: Vec<_> = tuning
        .tty_viewports
        .iter()
        .map(|viewport| {
            let refresh_millihz = viewport
                .refresh_rate
                .map(|hz: f64| (hz * 1000.0).round() as i64);
            (
                viewport.connector.clone(),
                viewport.enabled,
                viewport.offset_x,
                viewport.offset_y,
                viewport.width,
                viewport.height,
                refresh_millihz,
                viewport.transform_degrees,
                viewport.vrr.as_str(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub(crate) fn viewport_section_changed(prev: &RuntimeTuning, next: &RuntimeTuning) -> bool {
    normalized_tty_viewports(prev) != normalized_tty_viewports(next)
}

pub(crate) fn ensure_default_user_config(tty_viewports: Option<&[ViewportOutputConfig]>) {
    if env::var("HALLEY_WL_CONFIG").is_ok() {
        return;
    }

    let home_path = PathBuf::from(RuntimeTuning::default_home_config_path());
    if home_path.exists() {
        let raw = match fs::read_to_string(&home_path) {
            Ok(raw) => raw,
            Err(err) => {
                warn!(
                    "bootstrap: failed to read existing config {}: {}",
                    home_path.display(),
                    err
                );
                return;
            }
        };

        match RuntimeTuning::update_user_config_text(&raw, tty_viewports.unwrap_or(&[])) {
            Ok(Some(updated)) => {
                if let Err(err) = fs::write(&home_path, updated) {
                    warn!(
                        "bootstrap: failed to update existing config {}: {}",
                        home_path.display(),
                        err
                    );
                } else {
                    info!(
                        "bootstrap: updated existing config {} with missing template entries",
                        home_path.display()
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                warn!(
                    "bootstrap: skipped config update for {}: {}",
                    home_path.display(),
                    err
                );
            }
        }
        return;
    }

    let Some(parent) = home_path.parent() else {
        warn!(
            "bootstrap: unable to determine config directory for {}",
            home_path.display()
        );
        return;
    };
    if let Err(err) = fs::create_dir_all(parent) {
        warn!(
            "bootstrap: failed to create config directory {}: {}",
            parent.display(),
            err
        );
        return;
    }

    let rendered = RuntimeTuning::render_fresh_config(tty_viewports.unwrap_or(&[]));
    if let Err(err) = fs::write(&home_path, rendered) {
        warn!(
            "bootstrap: failed to write {} from internal template: {}",
            home_path.display(),
            err
        );
        return;
    }

    info!(
        "bootstrap: wrote {} using internal template",
        home_path.display()
    );
}

pub fn run() -> Result<(), Box<dyn Error>> {
    // Register signal handlers before anything else so that SIGTERM (the
    // default signal sent by `pkill`/`kill`) triggers a clean shutdown.
    // This lets Drop run, which kills all spawned child process groups.
    // Note: SIGKILL (-9) cannot be caught — use plain `pkill` for clean exit.
    SHUTDOWN_REQUESTED.store(false, Ordering::Relaxed);
    unsafe {
        let action = KernelSigaction {
            sa_handler_kernel: Some(handle_shutdown_signal),
            sa_flags: Default::default(),
            sa_mask: KernelSigSet::empty(),
            ..Default::default()
        };
        let _ = kernel_sigaction(Signal::TERM, Some(action.clone()));
        let _ = kernel_sigaction(Signal::INT, Some(action));
    }

    ensure_xdg_runtime_dir()?;
    init_ipc()?;

    let result = match RuntimeBackend::from_env()? {
        RuntimeBackend::Auto => match auto_backend() {
            RuntimeBackend::Tty => run_tty(),
            RuntimeBackend::Winit | RuntimeBackend::Auto => run_winit(),
        },
        RuntimeBackend::Winit => run_winit(),
        RuntimeBackend::Tty => run_tty(),
    };

    shutdown_ipc();
    result
}

pub fn run_session() -> Result<(), Box<dyn Error>> {
    unsafe {
        env::set_var("HALLEY_WL_BACKEND", "tty");
        env::set_var("XDG_SESSION_TYPE", "wayland");
        env::set_var("XDG_CURRENT_DESKTOP", "Halley");
        env::set_var("XDG_SESSION_DESKTOP", "Halley");
        env::set_var("DESKTOP_SESSION", "Halley");
        env::remove_var("DISPLAY");
        env::remove_var("WAYLAND_DISPLAY");
        env::remove_var("WAYLAND_SOCKET");
    }

    run()
}

pub fn run_winit() -> Result<(), Box<dyn Error>> {
    crate::backend::winit::run_winit_backend()
}

pub fn run_tty() -> Result<(), Box<dyn Error>> {
    crate::backend::tty::run_tty_backend()
}

pub(crate) fn init_logging() -> Result<(), Box<dyn Error>> {
    scope!("logging-init", success = "ready", {
        let shared_level = env::var("HALLEY_WL_LOG")
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
            level: Some(shared_level),
            file,
        })) {
            warn!("failed to configure logging: {}", err);
        }

        enable_console_color(true);
        enable_console_duration(false);

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
    Some(Some(expand_user_path(trimmed)))
}

fn default_halley_log_path() -> Option<PathBuf> {
    halley_runtime_dir().ok().map(|dir| dir.join("halley.log"))
}

fn expand_user_path(raw: &str) -> PathBuf {
    if raw == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| PathBuf::from(raw));
    }
    PathBuf::from(raw)
}
