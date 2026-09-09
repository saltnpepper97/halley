# Building and packaging

Halley's default build preserves the complete desktop and development feature
set:

| Feature | Default | Behavior |
| --- | --- | --- |
| `dbus` | yes | In-process D-Bus services, currently the accessibility keyboard monitor |
| `systemd` | yes | systemd environment publishing and readiness notification |
| `dinit` | no | dinit environment publishing and readiness notification |
| `xwayland` | yes | Native XWayland server and window-manager integration |
| `winit` | yes | Nested compositor backend used for development and testing |

D-Bus activation-environment updates are always available when the external
`dbus-update-activation-environment` helper is installed. Disabling `dbus`
removes Halley's in-process D-Bus service; it does not disable desktop portals.
The portal backend is already isolated in the `halley-portal` workspace
package.

The normal distribution build is:

```sh
cargo build --release --workspace
```

A dinit distribution can replace systemd integration while keeping every
desktop capability:

```sh
cargo build --release --workspace --no-default-features \
  --features dbus,dinit,xwayland,winit
```

A minimal Wayland-only TTY compositor can be checked or built with:

```sh
cargo build --release -p halley --no-default-features
```

Without `winit`, `--winit` and automatic nested-session selection report a
clear error instead of attempting to acquire the real DRM session. An explicit
`--session` still selects the TTY backend.

## Installed resources

Packaged resources use the distribution `/usr` layout:

| Resource | Destination |
| --- | --- |
| `halley`, `halleyctl`, `halley-lift`, `xdg-desktop-portal-halley` | `/usr/bin/` |
| `packaging/wayland-sessions/halley-session` | `/usr/bin/` |
| `packaging/wayland-sessions/halley.desktop` | `/usr/share/wayland-sessions/` |
| `packaging/xdg-desktop-portal/halley-portals.conf` | `/usr/share/xdg-desktop-portal/` |
| `packaging/xdg-desktop-portal/portals/halley.portal` | `/usr/share/xdg-desktop-portal/portals/` |
| `packaging/dbus-1/services/org.freedesktop.impl.portal.desktop.halley.service` | `/usr/share/dbus-1/services/` |
| `packaging/systemd-user/*` | `/usr/lib/systemd/user/` |
| `packaging/dinit/*` | `/usr/lib/dinit.d/user/` |

Only install the systemd resources for a build containing `systemd`, and only
install the dinit resources for a build containing `dinit`. The dinit wrapper
expects the user's dinit daemon to provide a `dbus` service.

`halley-session` loads the user's login-shell environment, then prefers a
booted systemd user manager or an active dinit user manager. It starts the
corresponding graphical target and waits for Halley to exit. OpenRC, runit, s6,
and systems without a supported user manager use the direct
`halley --session` fallback. `HALLEY_NO_INIT_INTEGRATION=1` forces that fallback.
`HALLEY_BIN` overrides `/usr/bin/halley` for development installs and uses the
direct path because the packaged manager services intentionally target
`/usr/bin`.

The systemd service uses `Type=exec`: it reports failure to execute Halley but
does not wait for a readiness notification. Its stdout and stderr append to
`$XDG_RUNTIME_DIR/halley-session.log` (normally under `/run/user/<uid>`).
The log survives service stops and failed starts, so users
can collect the log after returning to the TTY. Runtime files are temporary
and may be removed after logout or reboot. Errors from the launcher or systemd
before Halley starts may instead appear on the terminal or in
`journalctl --user -u halley.service`.

The runit and s6 files under `packaging/` are examples for personal user
supervision trees; those managers do not have a single standard distribution
path for graphical user services. The OpenRC README documents the direct
display-manager/login-shell setup.
