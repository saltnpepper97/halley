//! Detached process launching for session actions.

use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use super::autostart::AutostartChild;
use super::environment::LaunchEnvironment;

/// Spawns a user-provided command line detached from the compositor, with
/// `WAYLAND_DISPLAY` set to Halley's socket rather than the ambient host.
///
/// The shell is intentional: loose keybinds may contain arguments, quoting,
/// pipelines, or substitutions. The config is user-authored and has the
/// same authority as starting a command from their own shell.
pub(super) fn spawn_detached(
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
) {
    spawn_detached_with_env(
        command_line,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        &[],
    );
}

pub(super) fn spawn_detached_with_env(
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
    extra_environment: &[(&str, &str)],
) {
    let mut process = detached_process_with_env(
        command_line,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        extra_environment,
    );

    launch(&mut process, command_line, wayland_display, x11_display);
}

/// Open screenshot paths as literal arguments, including spaces and shell metacharacters.
pub(super) fn spawn_program(
    program: &str,
    argument: &std::path::Path,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
) -> io::Result<()> {
    let mut process = Command::new(program);
    process.arg(argument);
    configure_environment(
        &mut process,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        &[],
    );
    detach(&mut process);
    process.spawn()?.wait()?;
    Ok(())
}

/// Autostart output belongs in persistent files, rather than the terminal or
/// /dev/null. Ordinary keybind launches keep their existing behavior.
pub(super) fn spawn_autostart(
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
) -> Option<AutostartChild> {
    let path = match crate::autostart_log::prepare(command_line) {
        Ok(path) => path,
        Err(err) => {
            eventline::warn!("autostart: output logging unavailable for {command_line:?}: {err}");
            let mut process = Command::new("sh");
            process
                .arg("-c")
                .arg(command_line)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            configure_environment(
                &mut process,
                wayland_display,
                x11_display,
                cursor_size,
                environment,
                &[],
            );
            session_process_group(&mut process);
            return process
                .spawn()
                .map(|child| AutostartChild::new(command_line, child, None))
                .map_err(|error| {
                    eventline::error!("autostart: failed to launch {command_line:?}: {error}");
                })
                .ok();
        }
    };
    // Execute this running binary even if an upgrade replaced its installed
    // pathname. current_exe() can return an unusable "(deleted)" path then.
    let mut process = session_autostart_process(
        std::path::Path::new("/proc/self/exe"),
        &path,
        command_line,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
    );
    eventline::info!("autostart: {command_line:?} output log: {}", path.display());
    match process.spawn() {
        Ok(child) => Some(AutostartChild::new(command_line, child, Some(path))),
        Err(err) => {
            eventline::error!("autostart: failed to launch {command_line:?}: {err}");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn session_autostart_process(
    program: &std::path::Path,
    path: &std::path::Path,
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
) -> Command {
    let mut process = logged_process(
        program,
        path,
        command_line,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
    );
    session_process_group(&mut process);
    process
}

fn session_process_group(process: &mut Command) {
    // Keep the logger as our child and process-group leader so clean logout
    // can terminate only this session's startup command and its descendants.
    unsafe {
        process.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

fn logged_process(
    program: &std::path::Path,
    path: &std::path::Path,
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
) -> Command {
    let mut process = Command::new(program);
    process
        .arg(crate::autostart_log::WORKER_ARG)
        .arg(path)
        .arg(command_line);
    configure_environment(
        &mut process,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        &[],
    );
    process
}

fn launch(
    process: &mut Command,
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
) {
    match process.spawn() {
        Ok(mut child) => {
            // The immediate child exits after its own fork. The command's
            // shell is already reparented and will be reaped independently.
            match child.wait() {
                Ok(_) => eventline::debug!(
                    "spawn: launched {command_line:?} (WAYLAND_DISPLAY={wayland_display:?}, DISPLAY={x11_display:?})"
                ),
                Err(err) => {
                    eventline::warn!("spawn: failed to reap intermediate process: {err}")
                }
            }
        }
        Err(err) => eventline::error!("spawn: failed to launch {command_line:?}: {err}"),
    }
}

#[cfg(test)]
fn detached_process(
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
) -> Command {
    detached_process_with_env(
        command_line,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        &[],
    )
}

fn detached_process_with_env(
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
    extra_environment: &[(&str, &str)],
) -> Command {
    let mut process = configured_process(
        command_line,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        extra_environment,
    );

    detach(&mut process);
    process
}

fn detach(process: &mut Command) {
    // Safety: only async-signal-safe calls between fork and exec - a raw
    // fork() plus an immediate _exit() (never std::process::exit, which
    // isn't safe to run again after a raw fork - it may re-run Rust's
    // normal shutdown machinery a second time).
    unsafe {
        process.pre_exec(|| match libc::fork() {
            -1 => Err(io::Error::last_os_error()),
            0 => Ok(()),
            _ => libc::_exit(0),
        });
    }
}

fn configured_process(
    command_line: &str,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
    extra_environment: &[(&str, &str)],
) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command_line);
    configure_environment(
        &mut process,
        wayland_display,
        x11_display,
        cursor_size,
        environment,
        extra_environment,
    );
    process
}

fn configure_environment(
    process: &mut Command,
    wayland_display: &OsStr,
    x11_display: Option<&OsStr>,
    cursor_size: u8,
    environment: &LaunchEnvironment,
    extra_environment: &[(&str, &str)],
) {
    environment.apply_to(process);
    process
        .env("WAYLAND_DISPLAY", wayland_display)
        .env("XCURSOR_SIZE", cursor_size.to_string())
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(display) = x11_display {
        process.env("DISPLAY", display);
    }
    process.envs(extra_environment.iter().copied());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_use_a_shell_and_halley_display() {
        let process = detached_process(
            "grim -g \"$(slurp)\" ~/shot.png",
            OsStr::new("wayland-9"),
            Some(OsStr::new(":12")),
            32,
            &LaunchEnvironment::default(),
        );
        assert_eq!(process.get_program(), "sh");
        assert_eq!(
            process.get_args().collect::<Vec<_>>(),
            ["-c", "grim -g \"$(slurp)\" ~/shot.png"]
        );
        assert!(process.get_envs().any(|(name, value)| {
            name == "WAYLAND_DISPLAY" && value == Some(OsStr::new("wayland-9"))
        }));
        assert!(
            process
                .get_envs()
                .any(|(name, value)| name == "DISPLAY" && value == Some(OsStr::new(":12")))
        );
        assert!(
            process
                .get_envs()
                .any(|(name, value)| { name == "XCURSOR_SIZE" && value == Some(OsStr::new("32")) })
        );
    }

    #[test]
    fn unavailable_xwayland_removes_ambient_display() {
        let process = detached_process(
            "foot",
            OsStr::new("wayland-2"),
            None,
            24,
            &LaunchEnvironment::default(),
        );
        assert!(
            process
                .get_envs()
                .any(|(name, value)| name == "DISPLAY" && value.is_none())
        );
    }

    #[test]
    fn configured_environment_is_inherited_but_session_values_win() {
        let environment = LaunchEnvironment::new(&std::collections::BTreeMap::from([
            ("CUSTOM".to_string(), "value".to_string()),
            ("DISPLAY".to_string(), ":99".to_string()),
            ("WAYLAND_DISPLAY".to_string(), "wrong".to_string()),
            ("XCURSOR_SIZE".to_string(), "99".to_string()),
        ]));
        let process = detached_process(
            "true",
            OsStr::new("wayland-4"),
            Some(OsStr::new(":8")),
            24,
            &environment,
        );
        let env = process
            .get_envs()
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(
            env.get(OsStr::new("CUSTOM")),
            Some(&Some(OsStr::new("value")))
        );
        assert_eq!(
            env.get(OsStr::new("WAYLAND_DISPLAY")),
            Some(&Some(OsStr::new("wayland-4")))
        );
        assert_eq!(
            env.get(OsStr::new("DISPLAY")),
            Some(&Some(OsStr::new(":8")))
        );
        assert_eq!(
            env.get(OsStr::new("XCURSOR_SIZE")),
            Some(&Some(OsStr::new("24")))
        );
    }
}
