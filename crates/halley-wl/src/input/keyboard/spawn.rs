use std::env;
use std::ffi::OsStr;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Child;
use std::process::Command;

use eventline::{debug, warn};
use halley_config::CursorConfig;

use crate::bootstrap::request_xwayland_start;

const WAYLAND_TERMINAL_CANDIDATES: &[&str] = &[
    "ghostty",
    "kitty",
    "footclient",
    "foot",
    "wezterm",
    "alacritty",
    "rio",
    "contour",
];

fn apply_spawn_environment(
    cmd: &mut Command,
    wayland_display: &str,
    cursor: &CursorConfig,
    activation_token: Option<&str>,
) {
    cmd.env("WAYLAND_DISPLAY", wayland_display)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("GDK_BACKEND", "wayland,x11")
        .env("QT_QPA_PLATFORM", "wayland;xcb")
        .env("SDL_VIDEODRIVER", "wayland")
        .env("CLUTTER_BACKEND", "wayland")
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("ELECTRON_OZONE_PLATFORM_HINT", "auto")
        .env("XCURSOR_THEME", cursor.theme.trim())
        .env("XCURSOR_SIZE", cursor.size.to_string());
    if let Some(token) = activation_token {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
}

fn command_exists_in_path(command: &str, path: Option<&OsStr>) -> bool {
    path.is_some_and(|path| {
        env::split_paths(path).any(|dir| {
            let candidate = dir.join(command);
            Path::new(candidate.as_path()).is_file()
        })
    })
}

fn resolve_first_available_terminal_in_path(path: Option<&OsStr>) -> Option<&'static str> {
    WAYLAND_TERMINAL_CANDIDATES
        .iter()
        .copied()
        .find(|command| command_exists_in_path(command, path))
}

pub(crate) fn resolve_first_available_terminal() -> Option<&'static str> {
    resolve_first_available_terminal_in_path(env::var_os("PATH").as_deref())
}

pub(crate) fn spawn_command(
    command: &str,
    wayland_display: &str,
    cursor: &CursorConfig,
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
    apply_spawn_environment(&mut cmd, wayland_display, cursor, activation_token);

    // Give each spawned app its own process group so we can kill
    // the whole group (including any children it forks) on WM exit.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setpgid(None, None).map_err(std::io::Error::from)?;
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

pub(crate) fn spawn_wayland_terminal(
    wayland_display: &str,
    cursor: &CursorConfig,
    activation_token: Option<&str>,
) -> Option<Child> {
    let Some(command) = resolve_first_available_terminal() else {
        warn!(
            "open-terminal could not find a supported Wayland terminal in PATH ({})",
            WAYLAND_TERMINAL_CANDIDATES.join(", ")
        );
        return None;
    };

    spawn_command(
        command,
        wayland_display,
        cursor,
        activation_token,
        "terminal",
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::{
        WAYLAND_TERMINAL_CANDIDATES, apply_spawn_environment,
        resolve_first_available_terminal_in_path,
    };
    use halley_config::CursorConfig;

    #[test]
    fn spawn_environment_sets_activation_token_when_present() {
        let mut cmd = std::process::Command::new("sh");
        apply_spawn_environment(
            &mut cmd,
            "wayland-7",
            &CursorConfig {
                theme: "Bibata".to_string(),
                size: 32,
                hide_while_typing: false,
                hide_after_ms: 2_000,
            },
            Some("token-123"),
        );

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
        assert_eq!(envs.get("XCURSOR_THEME"), Some(&Some("Bibata".to_string())));
        assert_eq!(envs.get("XCURSOR_SIZE"), Some(&Some("32".to_string())));
        assert_eq!(
            envs.get("XDG_ACTIVATION_TOKEN"),
            Some(&Some("token-123".to_string()))
        );
    }

    #[test]
    fn spawn_environment_skips_activation_token_when_absent() {
        let mut cmd = std::process::Command::new("sh");
        apply_spawn_environment(&mut cmd, "wayland-7", &CursorConfig::default(), None);

        let has_activation_token = cmd
            .get_envs()
            .any(|(key, _)| key.to_string_lossy() == "XDG_ACTIVATION_TOKEN");
        assert!(!has_activation_token);
    }

    #[test]
    fn terminal_resolver_returns_first_available_candidate() {
        let base = std::env::temp_dir().join(format!(
            "halley-terminal-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&base).expect("temp dir should be created");

        for command in ["wezterm", "foot"] {
            let path = base.join(command);
            fs::write(&path, "#!/bin/sh\nexit 0\n").expect("stub executable should be written");
            let mut perms = fs::metadata(&path)
                .expect("stub executable metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("stub executable permissions");
        }

        let resolved = resolve_first_available_terminal_in_path(Some(base.as_os_str()));
        assert_eq!(resolved, Some("foot"));

        fs::remove_dir_all(base).expect("temp dir should be removed");
    }

    #[test]
    fn terminal_resolver_returns_none_when_no_candidates_exist() {
        let resolved =
            resolve_first_available_terminal_in_path(Some(OsStr::new("/definitely/not/here")));
        assert_eq!(resolved, None);
        assert!(!WAYLAND_TERMINAL_CANDIDATES.is_empty());
    }
}
