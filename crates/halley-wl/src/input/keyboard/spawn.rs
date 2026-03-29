use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;

use eventline::{debug, warn};

use crate::bootstrap::request_xwayland_start;

pub(crate) fn spawn_command(command: &str, wayland_display: &str, label: &str) -> Option<Child> {
    request_xwayland_start();
    let mut cmd = Command::new("sh");
    cmd.arg("-lc")
        .arg(command)
        .env("WAYLAND_DISPLAY", wayland_display)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("GDK_BACKEND", "wayland,x11")
        .env("QT_QPA_PLATFORM", "wayland;xcb")
        .env("SDL_VIDEODRIVER", "wayland")
        .env("CLUTTER_BACKEND", "wayland")
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("ELECTRON_OZONE_PLATFORM_HINT", "auto")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

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
