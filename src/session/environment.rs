use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(feature = "dinit")]
use std::fs::File;
#[cfg(feature = "dinit")]
use std::io::Write as _;
#[cfg(feature = "dinit")]
use std::os::fd::FromRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static DIRECT_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);

const SESSION_CLEANUP_VARIABLES: [&str; 7] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "DESKTOP_SESSION",
    "XCURSOR_SIZE",
];

/// A native compositor launched without a session wrapper owns cleanup too.
pub struct NativeSession;

impl Drop for NativeSession {
    fn drop(&mut self) {
        shutdown_session();
    }
}

/// Stop session services while the compositor still owns a working display.
/// Drop remains a fallback for initialization failures and unwinding.
pub fn shutdown_session() {
    if !DIRECT_SESSION_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut shutdown = Command::new("systemctl");
    shutdown.args([
        "--user",
        "start",
        "--job-mode=replace-irreversibly",
        "halley-shutdown.target",
    ]);
    if !run("direct session shutdown", &mut shutdown) {
        let mut stop = Command::new("systemctl");
        stop.args([
            "--user",
            "stop",
            "halley-direct-session.target",
            "graphical-session.target",
            "graphical-session-pre.target",
        ]);
        run("direct session target cleanup", &mut stop);
    }
    let mut clear = Command::new("systemctl");
    clear.args(["--user", "unset-environment"]);
    clear.args(SESSION_CLEANUP_VARIABLES);
    run("direct session environment cleanup", &mut clear);
}

fn launcher_owns_session() -> bool {
    std::env::var_os("HALLEY_SESSION_LAUNCHER_ACTIVE").as_deref() == Some(OsStr::new("1"))
        || (std::env::var_os("MANAGERPID").is_some()
            && std::env::var("SYSTEMD_EXEC_PID")
                .is_ok_and(|pid| pid == std::process::id().to_string()))
}

fn init_integration_enabled() -> bool {
    !matches!(
        std::env::var("HALLEY_NO_INIT_INTEGRATION").as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

const SESSION_VARIABLES: [&str; 6] = [
    "WAYLAND_DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "DESKTOP_SESSION",
    "PATH",
];

/// Effective environment overrides for every process launched by Halley.
///
/// Reloads intentionally merge rather than replace: old Halley applied
/// variables to its process environment, so removing a key from a live
/// config did not unset it until the compositor restarted.
#[derive(Clone, Debug, Default)]
pub(super) struct LaunchEnvironment {
    values: BTreeMap<String, String>,
}

impl LaunchEnvironment {
    pub fn new(values: &BTreeMap<String, String>) -> Self {
        Self {
            values: values.clone(),
        }
    }

    pub fn reload(&mut self, values: &BTreeMap<String, String>) {
        self.values.extend(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }

    pub fn apply_to(&self, command: &mut Command) {
        command.envs(&self.values);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.values
            .iter()
            .map(|(key, value)| (OsStr::new(key), OsStr::new(value)))
    }

    pub fn path(&self) -> Option<OsString> {
        self.values
            .get("PATH")
            .map(OsString::from)
            .or_else(|| std::env::var_os("PATH"))
    }
}

/// Establishes the environment that selects the real session backend.
///
/// This runs before any worker threads exist. Removing inherited display
/// variables is what prevents a display-manager environment from making a
/// full login accidentally choose the nested backend.
pub fn prepare_session() {
    unsafe {
        std::env::set_var("XDG_CURRENT_DESKTOP", "Halley");
        std::env::set_var("XDG_SESSION_DESKTOP", "Halley");
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        std::env::set_var("DESKTOP_SESSION", "Halley");
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("WAYLAND_SOCKET");
    }
}

/// Publish a usable compositor socket before starting graphical services.
pub fn activate_session(wayland_display: &OsStr, cursor_size: u8) {
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", wayland_display);
    }

    let mut dbus = Command::new("dbus-update-activation-environment");
    if systemd_is_enabled() {
        dbus.arg("--systemd");
    }
    dbus.args(SESSION_VARIABLES);
    run("dbus activation environment", &mut dbus);

    if systemd_is_enabled() {
        let mut import = Command::new("systemctl");
        import
            .arg("--user")
            .arg("import-environment")
            .args(SESSION_VARIABLES);
        run("systemd user environment", &mut import);
    } else {
        publish_dinit_variables(&SESSION_VARIABLES);
    }
    publish_cursor_size(cursor_size);
}

/// Refresh portal services only after announcing a usable graphical session.
pub fn refresh_portals() {
    if !systemd_is_enabled() || !init_integration_enabled() {
        return;
    }
    // A portal backend may have exhausted its start limit while the
    // compositor socket was unavailable. Clear that stale failure after
    // publishing the complete environment so the frontend can activate
    // it immediately.
    let mut reset_failed = Command::new("systemctl");
    reset_failed
        .arg("--user")
        .arg("reset-failed")
        .arg("xdg-desktop-portal-gtk.service");
    run_optional("portal failure reset", &mut reset_failed);

    let mut restart = Command::new("systemctl");
    restart
        .arg("--user")
        .arg("restart")
        .arg("--no-block")
        .arg("xdg-desktop-portal.service");
    run("portal refresh", &mut restart);
}

/// Updates the cursor size for applications activated after a config reload.
/// Existing processes retain their launch environment.
pub fn publish_cursor_size(cursor_size: u8) {
    let assignments = cursor_assignments(cursor_size);
    let mut dbus = Command::new("dbus-update-activation-environment");
    if systemd_is_enabled() {
        dbus.arg("--systemd");
    }
    dbus.args(&assignments);
    run("cursor D-Bus environment", &mut dbus);

    if systemd_is_enabled() {
        let mut systemd = Command::new("systemctl");
        systemd
            .arg("--user")
            .arg("set-environment")
            .args(&assignments);
        run("cursor systemd environment", &mut systemd);
    } else {
        publish_dinit_assignments(&assignments);
    }
}

pub fn activate_xwayland(display: &OsStr) {
    let mut assignment = OsString::from("DISPLAY=");
    assignment.push(display);

    let mut dbus = Command::new("dbus-update-activation-environment");
    if systemd_is_enabled() {
        dbus.arg("--systemd");
    }
    dbus.arg(&assignment);
    run("XWayland D-Bus environment", &mut dbus);

    if systemd_is_enabled() {
        let mut systemd = Command::new("systemctl");
        systemd
            .arg("--user")
            .arg("set-environment")
            .arg(&assignment);
        run("XWayland systemd environment", &mut systemd);
    } else {
        publish_dinit_assignments(std::slice::from_ref(&assignment));
    }
}

fn systemd_is_enabled() -> bool {
    cfg!(feature = "systemd") && Path::new("/run/systemd/system").is_dir()
}

#[cfg(feature = "dinit")]
fn publish_dinit_variables(variables: &[&str]) {
    if systemd_is_enabled() {
        return;
    }
    let mut command = Command::new("dinitctl");
    command
        .arg("--quiet")
        .arg("--user")
        .arg("setenv")
        .args(variables);
    run_optional("dinit user environment", &mut command);
}

#[cfg(not(feature = "dinit"))]
fn publish_dinit_variables(_variables: &[&str]) {}

#[cfg(feature = "dinit")]
fn publish_dinit_assignments(assignments: &[OsString]) {
    if systemd_is_enabled() {
        return;
    }
    let mut command = Command::new("dinitctl");
    command
        .arg("--quiet")
        .arg("--user")
        .arg("setenv")
        .args(assignments);
    run_optional("dinit user environment", &mut command);
}

#[cfg(not(feature = "dinit"))]
fn publish_dinit_assignments(_assignments: &[OsString]) {}

/// Announces that the compositor's Wayland and IPC listeners are usable.
pub fn notify_ready() {
    #[cfg(feature = "systemd")]
    if let Err(err) = sd_notify::notify(&[sd_notify::NotifyState::Ready]) {
        eventline::warn!("systemd readiness notification failed: {err}");
    }
    if systemd_is_enabled() && init_integration_enabled() && !launcher_owns_session() {
        // Enqueue rather than wait: services may connect to Wayland before
        // this function returns and the compositor begins dispatching events.
        let mut start = Command::new("systemctl");
        // graphical-session.target refuses direct starts. Pull it in through
        // our own target, without starting halley.service a second time.
        start.args([
            "--user",
            "start",
            "--no-block",
            "halley-direct-session.target",
        ]);
        DIRECT_SESSION_ACTIVE.store(
            run("direct graphical session", &mut start),
            Ordering::SeqCst,
        );
    }

    #[cfg(feature = "dinit")]
    if let Err(err) = notify_dinit_ready() {
        eventline::warn!("dinit readiness notification failed: {err}");
    }
}

#[cfg(feature = "dinit")]
fn notify_dinit_ready() -> Result<(), Box<dyn std::error::Error>> {
    let fd = match std::env::var("NOTIFY_FD") {
        Ok(value) => value.parse::<i32>()?,
        Err(std::env::VarError::NotPresent) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if fd < 0 {
        return Err("NOTIFY_FD must be non-negative".into());
    }
    // SAFETY: dinit gives the service ownership of this readiness pipe. Taking
    // it as a File writes the one readiness record and closes our copy.
    let mut pipe = unsafe { File::from_raw_fd(fd) };
    pipe.write_all(b"READY=1\n")?;
    Ok(())
}

fn run(label: &str, command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.status() {
        Ok(status) if status.success() => {
            eventline::debug!("{label}: complete");
            true
        }
        Ok(status) => {
            eventline::warn!("{label}: exited with {status}");
            false
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eventline::debug!("{label}: helper unavailable");
            false
        }
        Err(err) => {
            eventline::warn!("{label}: {err}");
            false
        }
    }
}

/// Runs best-effort integration cleanup where a missing/inactive optional
/// desktop component is an expected state, not a compositor warning.
fn run_optional(label: &str, command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.status() {
        Ok(status) if status.success() => eventline::debug!("{label}: complete"),
        Ok(status) => eventline::debug!("{label}: optional helper exited with {status}"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eventline::debug!("{label}: helper unavailable")
        }
        Err(err) => eventline::warn!("{label}: {err}"),
    }
}

fn cursor_assignments(cursor_size: u8) -> [OsString; 1] {
    [OsString::from(format!("XCURSOR_SIZE={cursor_size}"))]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::process::Command;

    use super::{LaunchEnvironment, SESSION_VARIABLES, cursor_assignments};

    #[test]
    fn activation_exports_portal_and_wayland_identity() {
        for required in ["WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"] {
            assert!(SESSION_VARIABLES.contains(&required));
        }
    }

    #[test]
    fn cursor_assignments_use_the_validated_runtime_config() {
        assert_eq!(
            cursor_assignments(32),
            [std::ffi::OsString::from("XCURSOR_SIZE=32")]
        );
    }

    #[test]
    fn launch_environment_keeps_live_reload_values_until_restart() {
        let mut environment =
            LaunchEnvironment::new(&BTreeMap::from([("FIRST".to_string(), "one".to_string())]));
        environment.reload(&BTreeMap::from([("SECOND".to_string(), "two".to_string())]));

        let mut command = Command::new("true");
        environment.apply_to(&mut command);
        let env = command.get_envs().collect::<BTreeMap<_, _>>();
        assert_eq!(env.get(OsStr::new("FIRST")), Some(&Some(OsStr::new("one"))));
        assert_eq!(
            env.get(OsStr::new("SECOND")),
            Some(&Some(OsStr::new("two")))
        );
        assert_eq!(
            environment.iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from([
                (OsStr::new("FIRST"), OsStr::new("one")),
                (OsStr::new("SECOND"), OsStr::new("two")),
            ])
        );
    }

    #[test]
    fn configured_path_overrides_the_ambient_path() {
        let environment = LaunchEnvironment::new(&BTreeMap::from([(
            "PATH".to_string(),
            "/configured/bin".to_string(),
        )]));

        assert_eq!(
            environment.path().as_deref(),
            Some(OsStr::new("/configured/bin"))
        );
    }
}
