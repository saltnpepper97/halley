use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;

use eventline::{debug, warn};

use crate::bootstrap::request_xwayland_start;

fn apply_spawn_environment(
    cmd: &mut Command,
    wayland_display: &str,
    activation_token: Option<&str>,
) {
    cmd.env("WAYLAND_DISPLAY", wayland_display)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("GDK_BACKEND", "wayland,x11")
        .env("QT_QPA_PLATFORM", "wayland;xcb")
        .env("SDL_VIDEODRIVER", "wayland")
        .env("CLUTTER_BACKEND", "wayland")
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("ELECTRON_OZONE_PLATFORM_HINT", "auto");
    if let Some(token) = activation_token {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
}

pub(crate) fn spawn_command(
    command: &str,
    wayland_display: &str,
    activation_token: Option<&str>,
    label: &str,
) -> Option<Child> {
    request_xwayland_start();
    let mut cmd = Command::new("sh");
    cmd.arg("-lc")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    apply_spawn_environment(&mut cmd, wayland_display, activation_token);

    // Give each spawned app its own process group so we can kill
    // the whole group (including any children it forks) on WM exit.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    match cmd.spawn() {
        Ok(child) => {
            debug!(
                "spawned {} via `{}` on WAYLAND_DISPLAY={} (pid={})",
                label,
                command,
                wayland_display,
                child.id()
            );
            Some(child)
        }
        Err(err) => {
            warn!("{} spawn failed via `{}`: {}", label, command, err);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_spawn_environment;

    #[test]
    fn spawn_environment_sets_activation_token_when_present() {
        let mut cmd = std::process::Command::new("sh");
        apply_spawn_environment(&mut cmd, "wayland-7", Some("token-123"));

        let envs = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            envs.get("WAYLAND_DISPLAY"),
            Some(&Some("wayland-7".to_string()))
        );
        assert_eq!(
            envs.get("XDG_ACTIVATION_TOKEN"),
            Some(&Some("token-123".to_string()))
        );
    }

    #[test]
    fn spawn_environment_skips_activation_token_when_absent() {
        let mut cmd = std::process::Command::new("sh");
        apply_spawn_environment(&mut cmd, "wayland-7", None);

        let has_activation_token = cmd
            .get_envs()
            .any(|(key, _)| key.to_string_lossy() == "XDG_ACTIVATION_TOKEN");
        assert!(!has_activation_token);
    }
}
