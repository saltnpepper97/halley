use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use super::environment::LaunchEnvironment;

enum OnceState {
    Unarmed,
    Pending(Vec<String>),
    Finished,
}

pub(super) struct AutostartChild {
    command: String,
    child: std::process::Child,
    log_path: Option<PathBuf>,
    wait_error_reported: bool,
}

impl AutostartChild {
    pub fn new(command: &str, child: std::process::Child, log_path: Option<PathBuf>) -> Self {
        Self {
            command: command.to_owned(),
            child,
            log_path,
            wait_error_reported: false,
        }
    }

    fn reap_finished(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    eventline::debug!("autostart: {:?} completed with {status}", self.command);
                } else if let Some(path) = &self.log_path {
                    eventline::warn!(
                        "autostart: {:?} exited with {status}; output log: {}",
                        self.command,
                        path.display()
                    );
                } else {
                    eventline::warn!(
                        "autostart: {:?} exited with {status}; output logging was unavailable",
                        self.command
                    );
                }
                true
            }
            Ok(None) => false,
            Err(err) => {
                if !self.wait_error_reported {
                    eventline::warn!("autostart: could not reap {:?}: {err}", self.command);
                    self.wait_error_reported = true;
                }
                false
            }
        }
    }
}

/// Launches configured startup commands in separate process groups.
///
/// `autostart` is a convenience launcher, not a service manager. Long-lived
/// session services should use the user service manager when they need
/// restart, ordering, or management of deliberately daemonized processes.
pub(super) struct Autostart {
    enabled: bool,
    once: OnceState,
    wayland_display: Option<OsString>,
    children: Vec<AutostartChild>,
    next_reap: std::time::Instant,
}

impl Autostart {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            once: OnceState::Unarmed,
            wayland_display: None,
            children: Vec::new(),
            next_reap: std::time::Instant::now(),
        }
    }

    #[cfg_attr(not(feature = "winit"), allow(dead_code))]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            once: OnceState::Finished,
            wayland_display: None,
            children: Vec::new(),
            next_reap: std::time::Instant::now(),
        }
    }

    pub fn arm_once(&mut self, wayland_display: &OsStr, mut commands: Vec<String>) {
        if !self.enabled || !matches!(self.once, OnceState::Unarmed) {
            return;
        }
        let mut seen = std::collections::HashSet::new();
        commands.retain(|command| {
            let command = command.trim();
            if command.is_empty() {
                return false;
            }
            if !seen.insert(command.to_owned()) {
                eventline::debug!("autostart: skipping duplicate once command {command:?}");
                return false;
            }
            true
        });
        self.wayland_display = Some(wayland_display.to_os_string());
        self.once = OnceState::Pending(commands);
    }

    pub fn run_once(
        &mut self,
        x11_display: Option<&OsStr>,
        cursor_size: u8,
        environment: &LaunchEnvironment,
    ) {
        let OnceState::Pending(commands) = std::mem::replace(&mut self.once, OnceState::Finished)
        else {
            return;
        };
        self.run_commands(&commands, x11_display, cursor_size, environment);
    }

    pub fn run_reload(
        &mut self,
        commands: &[String],
        x11_display: Option<&OsStr>,
        cursor_size: u8,
        environment: &LaunchEnvironment,
    ) {
        if self.enabled {
            self.run_commands(commands, x11_display, cursor_size, environment);
        }
    }

    fn run_commands(
        &mut self,
        commands: &[String],
        x11_display: Option<&OsStr>,
        cursor_size: u8,
        environment: &LaunchEnvironment,
    ) {
        let Some(wayland_display) = self.wayland_display.as_deref() else {
            return;
        };
        for command in commands {
            let command = command.trim();
            if command.is_empty() {
                continue;
            }
            eventline::debug!(
                "autostart: launching {command:?} (WAYLAND_DISPLAY={wayland_display:?}, DISPLAY={x11_display:?})"
            );
            if let Some(child) = super::spawn::spawn_autostart(
                command,
                wayland_display,
                x11_display,
                cursor_size,
                environment,
            ) {
                self.children.push(child);
            }
        }
    }

    /// Give startup services a chance to exit before their Wayland socket
    /// disappears. Never signal unrelated processes by name or an old PID.
    pub fn shutdown(&mut self) {
        let mut children = self.children.drain(..).map(|child| child.child).collect();
        stop_children(&mut children);
    }

    pub fn reap_finished(&mut self) {
        let now = std::time::Instant::now();
        if now >= self.next_reap {
            self.children.retain_mut(|child| !child.reap_finished());
            self.next_reap = now + std::time::Duration::from_secs(1);
        }
    }
}

pub(super) fn stop_children(children: &mut Vec<std::process::Child>) {
    children.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
    for child in children.iter() {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGTERM);
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !children.is_empty() && std::time::Instant::now() < deadline {
        children.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
        if !children.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    for mut child in children.drain(..) {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
}
