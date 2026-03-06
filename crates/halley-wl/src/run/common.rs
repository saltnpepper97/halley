use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::io::IsTerminal;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Child;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use eventline::{info, warn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeBackend {
    Auto,
    Winit,
    Tty,
}

impl RuntimeBackend {
    pub(super) fn from_env() -> Result<Self, Box<dyn Error>> {
        let raw = env::var("HALLEY_WL_BACKEND").unwrap_or_else(|_| "auto".to_string());
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "winit" => Ok(Self::Winit),
            "tty" => Ok(Self::Tty),
            other => Err(io::Error::other(format!(
                "invalid HALLEY_WL_BACKEND={other} (expected auto|winit|tty)"
            ))
            .into()),
        }
    }
}

pub(super) fn auto_backend() -> RuntimeBackend {
    if !std::io::stdin().is_terminal() {
        return RuntimeBackend::Winit;
    }

    if env::var("WAYLAND_DISPLAY").is_ok() || env::var("DISPLAY").is_ok() {
        return RuntimeBackend::Winit;
    }

    let session_type = env::var("XDG_SESSION_TYPE")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase());
    if matches!(session_type.as_deref(), Some("wayland" | "x11")) {
        return RuntimeBackend::Winit;
    }

    #[cfg(not(feature = "session-libseat"))]
    {
        RuntimeBackend::Tty
    }

    #[cfg(feature = "session-libseat")]
    {
        if matches!(session_type.as_deref(), Some("tty")) {
            return RuntimeBackend::Tty;
        }

        if env::var("XDG_VTNR")
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
        {
            return RuntimeBackend::Tty;
        }

        RuntimeBackend::Tty
    }
}

pub(super) fn ensure_xdg_runtime_dir() -> Result<(), Box<dyn Error>> {
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        let path = Path::new(&dir);
        if runtime_dir_is_usable(path) {
            return Ok(());
        }
        warn!(
            "XDG_RUNTIME_DIR={} is not usable, trying fallback",
            path.display()
        );
    }

    let uid = rustix::process::getuid().as_raw();
    let run_user_dir = format!("/run/user/{}", uid);
    let run_user_path = Path::new(run_user_dir.as_str());
    if runtime_dir_is_usable(run_user_path) {
        // SAFETY: Called during startup before worker threads are spawned.
        unsafe { env::set_var("XDG_RUNTIME_DIR", run_user_path) };
        info!("XDG_RUNTIME_DIR={}", run_user_path.display());
        return Ok(());
    }

    let fallback = env::temp_dir().join(format!("halley-runtime-{}", uid));
    if !fallback.exists() {
        fs::create_dir_all(&fallback)?;
    }
    fs::set_permissions(&fallback, fs::Permissions::from_mode(0o700))?;
    if runtime_dir_is_usable(&fallback) {
        // SAFETY: Called during startup before worker threads are spawned.
        unsafe { env::set_var("XDG_RUNTIME_DIR", &fallback) };
        warn!(
            "using fallback XDG_RUNTIME_DIR={} (from temp dir)",
            fallback.display()
        );
        return Ok(());
    }

    Err("unable to find a usable XDG_RUNTIME_DIR".into())
}

pub(super) fn ensure_dbus_session_bus_address() {
    if env::var("DBUS_SESSION_BUS_ADDRESS").is_ok() {
        return;
    }
    let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    let bus_path = Path::new(&runtime_dir).join("bus");
    let Ok(meta) = fs::metadata(&bus_path) else {
        return;
    };
    if !meta.file_type().is_socket() {
        return;
    }
    let addr = format!("unix:path={}", bus_path.display());
    // SAFETY: Called during startup before worker threads are spawned.
    unsafe { env::set_var("DBUS_SESSION_BUS_ADDRESS", addr) };
}

pub(super) struct HostBackendGuard {
    child: Option<Child>,
}

impl Drop for HostBackendGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) fn ensure_host_display() -> Result<HostBackendGuard, Box<dyn Error>> {
    if env::var("WAYLAND_DISPLAY").is_ok() || env::var("DISPLAY").is_ok() {
        return Ok(HostBackendGuard { child: None });
    }

    let runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "<unset>".to_string());
    if let Some(sock) = first_wayland_socket_in_dir(Path::new(runtime_dir.as_str())) {
        // SAFETY: Called during startup before worker threads are spawned.
        unsafe { env::set_var("WAYLAND_DISPLAY", sock) };
        return Ok(HostBackendGuard { child: None });
    }
    if let Some(display) = first_x11_display() {
        // SAFETY: Called during startup before worker threads are spawned.
        unsafe { env::set_var("DISPLAY", display) };
        return Ok(HostBackendGuard { child: None });
    }

    if !std::io::stdin().is_terminal() {
        return Ok(HostBackendGuard { child: None });
    }

    let backend = env::var("HALLEY_WL_HOST_BACKEND").unwrap_or_else(|_| "none".to_string());
    if backend != "none" {
        warn!(
            "HALLEY_WL_HOST_BACKEND={} ignored: automatic host compositor launch is disabled",
            backend
        );
    }

    Ok(HostBackendGuard { child: None })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XwaylandMode {
    OnDemand,
    Auto,
    On,
    Off,
}

impl XwaylandMode {
    fn from_env() -> Result<Self, Box<dyn Error>> {
        let raw = env::var("HALLEY_DEV_WL_XWAYLAND").unwrap_or_else(|_| "ondemand".to_string());
        match raw.trim().to_ascii_lowercase().as_str() {
            "ondemand" | "on_demand" => Ok(Self::OnDemand),
            "auto" => Ok(Self::Auto),
            "on" | "true" | "1" => Ok(Self::On),
            "off" | "false" | "0" => Ok(Self::Off),
            other => Err(io::Error::other(format!(
                "invalid HALLEY_DEV_WL_XWAYLAND={other} (expected ondemand|auto|on|off)"
            ))
            .into()),
        }
    }
}

pub(super) struct XwaylandSatellite {
    mode: XwaylandMode,
    satellite_bin: String,
    wayland_display: String,
    display: String,
    child: Option<Child>,
    restart_delay: Duration,
    restart_after: Option<Instant>,
    request_pending: bool,
    disabled: bool,
}

impl XwaylandSatellite {
    pub(super) fn request_start(&mut self) {
        self.request_pending = true;
    }

    pub(super) fn tick(&mut self) {
        let now = Instant::now();

        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    warn!(
                        "xwayland-satellite exited with status {}; scheduling restart",
                        status
                    );
                    self.child = None;
                    self.restart_after = Some(now + self.restart_delay);
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        "failed to query xwayland-satellite status: {}; scheduling restart",
                        err
                    );
                    self.child = None;
                    self.restart_after = Some(now + self.restart_delay);
                }
            }
        }

        if self.child.is_some() || self.disabled {
            return;
        }

        let should_start = match self.mode {
            XwaylandMode::On => true,
            XwaylandMode::OnDemand => self.request_pending,
            XwaylandMode::Auto => true,
            XwaylandMode::Off => false,
        };
        if !should_start {
            return;
        }
        if self.restart_after.is_some_and(|t| now < t) {
            return;
        }

        match Command::new(self.satellite_bin.as_str())
            .arg(self.display.as_str())
            .env("WAYLAND_DISPLAY", self.wayland_display.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => {
                self.child = Some(child);
                self.request_pending = false;
                self.restart_after = None;
                // SAFETY: Called on the main event-loop thread.
                unsafe { env::set_var("DISPLAY", self.display.as_str()) };
                info!(
                    "xwayland-satellite started via {} on DISPLAY={} (WAYLAND_DISPLAY={})",
                    self.satellite_bin, self.display, self.wayland_display
                );
            }
            Err(err) => {
                warn!(
                    "failed to start xwayland-satellite via {}: {}; retrying",
                    self.satellite_bin, err
                );
                self.restart_after = Some(now + self.restart_delay);
            }
        }
    }
}

impl Drop for XwaylandSatellite {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) fn ensure_xwayland_satellite(
    wayland_display: &str,
) -> Result<XwaylandSatellite, Box<dyn Error>> {
    let mode = XwaylandMode::from_env()?;
    if mode == XwaylandMode::Off {
        info!("xwayland integration disabled (HALLEY_DEV_WL_XWAYLAND=off)");
        // SAFETY: Called during startup before worker threads are spawned.
        unsafe { env::remove_var("DISPLAY") };
        return Ok(XwaylandSatellite {
            mode,
            satellite_bin: String::new(),
            wayland_display: wayland_display.to_string(),
            display: String::new(),
            child: None,
            restart_delay: Duration::from_millis(1500),
            restart_after: None,
            request_pending: false,
            disabled: true,
        });
    }

    let satellite_bin = env::var("HALLEY_DEV_WL_XWAYLAND_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "xwayland-satellite".to_string());

    match Command::new(satellite_bin.as_str())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if mode == XwaylandMode::On {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "HALLEY_DEV_WL_XWAYLAND=on but `xwayland-satellite` was not found in PATH",
                )
                .into());
            }
            warn!(
                "xwayland-satellite not found in PATH; X11 apps are unavailable (set HALLEY_DEV_WL_XWAYLAND=off to silence)"
            );
            // SAFETY: Called during startup before worker threads are spawned.
            unsafe { env::remove_var("DISPLAY") };
            return Ok(XwaylandSatellite {
                mode,
                satellite_bin,
                wayland_display: wayland_display.to_string(),
                display: String::new(),
                child: None,
                restart_delay: Duration::from_millis(1500),
                restart_after: None,
                request_pending: false,
                disabled: true,
            });
        }
        Err(err) => {
            if mode == XwaylandMode::On {
                return Err(io::Error::other(format!(
                    "failed to probe xwayland-satellite: {}",
                    err
                ))
                .into());
            }
            warn!(
                "failed to probe xwayland-satellite: {}; continuing without X11 support",
                err
            );
            // SAFETY: Called during startup before worker threads are spawned.
            unsafe { env::remove_var("DISPLAY") };
            return Ok(XwaylandSatellite {
                mode,
                satellite_bin,
                wayland_display: wayland_display.to_string(),
                display: String::new(),
                child: None,
                restart_delay: Duration::from_millis(1500),
                restart_after: None,
                request_pending: false,
                disabled: true,
            });
        }
    }

    let display = env::var("HALLEY_DEV_WL_XWAYLAND_DISPLAY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(next_free_x11_display);
    let restart_delay_ms = env::var("HALLEY_DEV_WL_XWAYLAND_RESTART_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(1500)
        .max(50);
    let mut satellite = XwaylandSatellite {
        mode,
        satellite_bin,
        wayland_display: wayland_display.to_string(),
        display,
        child: None,
        restart_delay: Duration::from_millis(restart_delay_ms),
        restart_after: None,
        request_pending: matches!(mode, XwaylandMode::On | XwaylandMode::Auto),
        disabled: false,
    };

    // In on-demand mode, keep DISPLAY unset until first request.
    if matches!(satellite.mode, XwaylandMode::OnDemand) {
        // SAFETY: Called during startup before worker threads are spawned.
        unsafe { env::remove_var("DISPLAY") };
    }
    satellite.tick();
    Ok(satellite)
}

fn first_wayland_socket_in_dir(dir: &Path) -> Option<String> {
    let entries = fs::read_dir(dir).ok()?;
    let mut names = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy().to_string();
            if !name.starts_with("wayland-") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.file_type().is_socket() {
                return None;
            }
            Some(name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.into_iter().next()
}

fn first_x11_display() -> Option<String> {
    let dir = Path::new("/tmp/.X11-unix");
    let entries = fs::read_dir(dir).ok()?;
    let mut displays = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('X') {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.file_type().is_socket() {
                return None;
            }
            Some(format!(":{}", name.trim_start_matches('X')))
        })
        .collect::<Vec<_>>();
    displays.sort();
    displays.into_iter().next()
}

fn next_free_x11_display() -> String {
    let mut used = std::collections::BTreeSet::new();
    let dir = Path::new("/tmp/.X11-unix");
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(num) = name.strip_prefix('X').and_then(|n| n.parse::<u32>().ok()) {
                used.insert(num);
            }
        }
    }
    let next = (0u32..4096u32)
        .find(|idx| !used.contains(idx))
        .unwrap_or(4096);
    format!(":{}", next)
}

fn runtime_dir_is_usable(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        return false;
    }
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path.join(".halley-runtime-check"))
        .and_then(|_| fs::remove_file(path.join(".halley-runtime-check")))
        .is_ok()
}
