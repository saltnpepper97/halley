use std::env;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::ErrorKind;
use std::io::IsTerminal;
use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use eventline::{debug, info, warn};
use rustix::net::{
    bind, listen, socket_with, AddressFamily, SocketAddrUnix, SocketFlags, SocketType,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeBackend {
    Auto,
    Winit,
    Tty,
}

impl RuntimeBackend {
    pub(crate) fn from_env() -> Result<Self, Box<dyn Error>> {
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

pub(crate) fn auto_backend() -> RuntimeBackend {
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

pub(crate) fn ensure_xdg_runtime_dir() -> Result<(), Box<dyn Error>> {
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

pub(crate) fn halley_runtime_dir() -> io::Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        let path = Path::new(&dir).join("halley");
        fs::create_dir_all(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        return Ok(path);
    }

    let fallback = PathBuf::from(format!(
        "/tmp/halley-{}",
        rustix::process::getuid().as_raw()
    ));
    fs::create_dir_all(&fallback)?;
    fs::set_permissions(&fallback, fs::Permissions::from_mode(0o700))?;
    Ok(fallback)
}

pub(crate) fn ensure_dbus_session_bus_address() {
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

pub(crate) fn sync_portal_activation_environment(wayland_display: &str) {
    // SAFETY: Called during tty compositor startup before worker threads are spawned.
    unsafe {
        env::set_var("WAYLAND_DISPLAY", wayland_display);
        env::set_var("XDG_SESSION_TYPE", "wayland");

        // What user-facing apps should see as the desktop
        env::set_var("XDG_SESSION_DESKTOP", "Halley");
        env::set_var("DESKTOP_SESSION", "Halley");

        // What portals can use for backend matching
        env::set_var("XDG_CURRENT_DESKTOP", "Halley");
    }

    let vars = activation_environment_vars();
    if vars.is_empty() {
        return;
    }

    run_activation_env_sync(
        "dbus-update-activation-environment",
        Command::new("dbus-update-activation-environment")
            .arg("--systemd")
            .args(vars.iter().map(String::as_str)),
    );
    run_activation_env_sync(
        "systemctl import-environment",
        Command::new("systemctl")
            .arg("--user")
            .arg("import-environment")
            .args(vars.iter().map(String::as_str)),
    );
}

pub(crate) fn refresh_portal_services_nonblocking() {
    run_portal_service_command(
        "restart xdg-desktop-portal.service",
        Command::new("systemctl")
            .arg("--user")
            .arg("restart")
            .arg("--no-block")
            .arg("xdg-desktop-portal.service"),
    );
    run_portal_service_command(
        "start xdg-desktop-portal-wlr.service",
        Command::new("systemctl")
            .arg("--user")
            .arg("start")
            .arg("--no-block")
            .arg("xdg-desktop-portal-wlr.service"),
    );
}

fn activation_environment_vars() -> Vec<String> {
    [
        "WAYLAND_DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "PATH",
    ]
    .into_iter()
    .filter(|name| {
        env::var(name)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
    .map(str::to_string)
    .collect()
}

fn run_activation_env_sync(label: &str, command: &mut Command) {
    match command.status() {
        Ok(status) if status.success() => {
            debug!("{} completed successfully", label);
        }
        Ok(status) => {
            warn!("{} exited with status {}", label, status);
        }
        Err(err) => {
            warn!("{} failed: {}", label, err);
        }
    }
}

fn run_portal_service_command(label: &str, command: &mut Command) {
    match command.status() {
        Ok(status) if status.success() => {
            info!("portal sync: {} queued", label);
        }
        Ok(status) => {
            warn!("portal sync failed: {} exited with {}", label, status);
        }
        Err(err) => {
            warn!("portal sync failed: {}: {}", label, err);
        }
    }
}

fn terminate_child_with_timeout(child: &mut Child, label: &str, timeout: Duration) {
    let pid = child.id();
    debug!("terminating {} pid={}", label, pid);
    let _ = child.kill();

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                debug!("{} pid={} exited with {}", label, pid, status);
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                warn!(
                    "{} pid={} did not exit within {:?}; waiting after kill",
                    label, pid, timeout
                );
                let _ = child.wait();
                break;
            }
            Err(err) => {
                warn!("failed to reap {} pid={}: {}", label, pid, err);
                break;
            }
        }
    }
}

fn terminate_process_group_with_timeout(child: &mut Child, label: &str, timeout: Duration) {
    let pid = child.id() as i32;
    debug!("terminating {} process group pgid={}", label, pid);
    unsafe {
        let _ = libc::kill(-pid, libc::SIGTERM);
    }

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                debug!("{} pgid={} exited with {}", label, pid, status);
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                warn!(
                    "{} pgid={} ignored SIGTERM for {:?}; sending SIGKILL",
                    label, pid, timeout
                );
                unsafe {
                    let _ = libc::kill(-pid, libc::SIGKILL);
                }
                let _ = child.wait();
                break;
            }
            Err(err) => {
                warn!("failed to reap {} pgid={}: {}", label, pid, err);
                break;
            }
        }
    }
}

pub(crate) struct HostBackendGuard {
    child: Option<Child>,
}

impl Drop for HostBackendGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            terminate_child_with_timeout(child, "host backend", Duration::from_millis(500));
        }
    }
}

pub(crate) fn ensure_host_display() -> Result<HostBackendGuard, Box<dyn Error>> {
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

pub(crate) struct XwaylandSatellite {
    mode: XwaylandMode,
    satellite_bin: String,
    wayland_display: String,
    display: String,
    x11_sockets: Option<X11SocketReservation>,
    child: Option<Child>,
    restart_delay: Duration,
    restart_after: Option<Instant>,
    request_pending: bool,
    disabled: bool,
}

struct X11SocketReservation {
    lock_path: PathBuf,
    socket_path: PathBuf,
    _lock_file: File,
    filesystem_listener: UnixListener,
    abstract_listener: OwnedFd,
}

impl X11SocketReservation {
    fn try_new(display: &str) -> io::Result<Self> {
        let Some(display_num) = display.strip_prefix(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid X11 display `{display}`; expected :N"),
            ));
        };
        if display_num.is_empty() || !display_num.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid X11 display `{display}`; expected :N"),
            ));
        }

        let lock_path = PathBuf::from(format!("/tmp/.X{}-lock", display_num));
        let socket_path = PathBuf::from(format!("/tmp/.X11-unix/X{}", display_num));

        reclaim_stale_x11_display(display, &lock_path, &socket_path)?;

        let mut lock_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)?;
        let _ = writeln!(lock_file, "{}", std::process::id());

        let reservation = (|| {
            let filesystem_listener = match UnixListener::bind(&socket_path) {
                Ok(listener) => listener,
                Err(err) if err.kind() == ErrorKind::AddrInUse => {
                    let _ = fs::remove_file(&socket_path);
                    UnixListener::bind(&socket_path)?
                }
                Err(err) => return Err(err),
            };
            filesystem_listener.set_nonblocking(true)?;

            let abstract_listener = socket_with(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
                None,
            )?;
            let abstract_name = socket_path.to_string_lossy().into_owned();
            let abstract_addr = SocketAddrUnix::new_abstract_name(abstract_name.as_bytes())?;
            bind(&abstract_listener, &abstract_addr)?;
            listen(&abstract_listener, 128)?;

            Ok(Self {
                lock_path: lock_path.clone(),
                socket_path: socket_path.clone(),
                _lock_file: lock_file,
                filesystem_listener,
                abstract_listener,
            })
        })();

        if reservation.is_err() {
            let _ = fs::remove_file(&lock_path);
            let _ = fs::remove_file(&socket_path);
        }

        reservation
    }

    fn filesystem_listener_for_event_loop(&self) -> io::Result<UnixListener> {
        self.filesystem_listener.try_clone()
    }

    fn abstract_listener_for_event_loop(&self) -> io::Result<OwnedFd> {
        Ok(rustix::io::dup(&self.abstract_listener)?)
    }

    fn child_listen_fds(&self) -> io::Result<Vec<OwnedFd>> {
        Ok(vec![
            rustix::io::dup(&self.filesystem_listener)?,
            rustix::io::dup(&self.abstract_listener)?,
        ])
    }
}

impl Drop for X11SocketReservation {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.lock_path);
    }
}

impl XwaylandSatellite {
    pub(crate) fn request_start(&mut self) {
        self.request_pending = true;
    }

    pub(crate) fn tick(&mut self) {
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

        let spawn_result = if let Some(sockets) = self.x11_sockets.as_ref() {
            let listen_fds = match sockets.child_listen_fds() {
                Ok(fds) => fds,
                Err(err) => {
                    warn!(
                        "failed to duplicate X11 listen sockets for xwayland-satellite: {}; retrying",
                        err
                    );
                    self.restart_after = Some(now + self.restart_delay);
                    return;
                }
            };

            let mut command = Command::new(self.satellite_bin.as_str());
            command
                .arg(self.display.as_str())
                .env("WAYLAND_DISPLAY", self.wayland_display.as_str())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());

            for idx in 0..listen_fds.len() {
                command.arg("-listenfd").arg((3 + idx).to_string());
            }

            let raw_fds: Vec<i32> = listen_fds.iter().map(AsRawFd::as_raw_fd).collect();
            unsafe {
                command.pre_exec(move || {
                    libc::setpgid(0, 0);
                    for (idx, raw_fd) in raw_fds.iter().enumerate() {
                        let target_fd = 3 + idx as i32;
                        if libc::dup2(*raw_fd, target_fd) == -1 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }

            command.spawn()
        } else {
            let mut command = Command::new(self.satellite_bin.as_str());
            command
                .arg(self.display.as_str())
                .env("WAYLAND_DISPLAY", self.wayland_display.as_str())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());
            unsafe {
                command.pre_exec(move || {
                    libc::setpgid(0, 0);
                    Ok(())
                });
            }
            command.spawn()
        };

        match spawn_result {
            Ok(child) => {
                self.child = Some(child);
                self.request_pending = false;
                self.restart_after = None;
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

    pub(crate) fn filesystem_listener_source(&self) -> io::Result<Option<UnixListener>> {
        self.x11_sockets
            .as_ref()
            .map(X11SocketReservation::filesystem_listener_for_event_loop)
            .transpose()
    }

    pub(crate) fn abstract_listener_source(&self) -> io::Result<Option<OwnedFd>> {
        self.x11_sockets
            .as_ref()
            .map(X11SocketReservation::abstract_listener_for_event_loop)
            .transpose()
    }
}

impl Drop for XwaylandSatellite {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            terminate_process_group_with_timeout(
                child,
                "xwayland-satellite",
                Duration::from_millis(1200),
            );
        }
    }
}

pub(crate) fn ensure_xwayland_satellite(
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
            x11_sockets: None,
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
                x11_sockets: None,
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
                x11_sockets: None,
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
    let x11_sockets = X11SocketReservation::try_new(display.as_str()).map_err(|err| {
        io::Error::other(format!(
            "failed to reserve X11 display {} for xwayland-satellite: {}",
            display, err
        ))
    })?;
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
        x11_sockets: Some(x11_sockets),
        child: None,
        restart_delay: Duration::from_millis(restart_delay_ms),
        restart_after: None,
        request_pending: matches!(mode, XwaylandMode::On | XwaylandMode::Auto),
        disabled: false,
    };

    // SAFETY: Called during startup before worker threads are spawned.
    unsafe { env::set_var("DISPLAY", satellite.display.as_str()) };
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

fn reclaim_stale_x11_display(
    display: &str,
    lock_path: &Path,
    socket_path: &Path,
) -> io::Result<()> {
    let lock_pid = fs::read_to_string(lock_path)
        .ok()
        .and_then(|raw| raw.trim().parse::<i32>().ok());

    if let Some(pid) = lock_pid {
        let alive = unsafe {
            libc::kill(pid, 0) == 0
                || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        };
        if alive {
            return Err(io::Error::new(
                ErrorKind::AddrInUse,
                format!("X11 display {display} is already in use by pid {pid}"),
            ));
        }
    }

    if lock_path.exists() {
        let _ = fs::remove_file(lock_path);
    }
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    Ok(())
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
