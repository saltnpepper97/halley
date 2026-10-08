# Building and packaging

Halley's default build preserves the complete desktop and development feature
set:

| Feature | Default | Behavior |
| --- | --- | --- |
| `dbus` | yes | In-process D-Bus services, accessibility keyboard monitoring and native idle inhibition |
| `systemd` | yes | systemd environment publishing and readiness notification |
| `dinit` | no | dinit environment publishing and readiness notification |
| `xwayland` | yes | Native XWayland server and window-manager integration |
| `winit` | yes | Nested compositor backend used for development and testing |

D-Bus activation-environment updates are always available when the external
`dbus-update-activation-environment` helper is installed. Disabling `dbus`
removes Halley's in-process D-Bus services; it does not disable desktop portals.
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

Halley Lift is built and packaged separately from
[its own repository](https://github.com/saltnpepper97/halley-lift). The default
launcher binding still calls `halley-lift`; ecosystem bundles should depend on
that package. Historical monorepo release tags continue to contain older Lift sources.

## Installed resources

Packaged resources use the distribution `/usr` layout:

| Resource | Destination |
| --- | --- |
| `halley`, `halleyctl`, `xdg-desktop-portal-halley` | `/usr/bin/` |
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
The launcher selects a sibling `halley` binary when installed together (normally
`/usr/bin/halley`); `HALLEY_BIN` overrides that for development installs. Updated
systemd units enter through the launcher, which passes that binary to the
service without re-entering the login shell. Older units and explicit
compositor arguments use the direct path so the selected binary and arguments
are preserved. The dinit service still targets `/usr/bin/halley`.

The systemd service uses `Type=notify`: graphical-session services start after
Halley publishes its display environment and announces usable listeners. The
launcher waits for compositor exit, stops the graphical session through
`halley-shutdown.target`, and clears its environment. Its stdout and stderr append to
`$XDG_RUNTIME_DIR/halley-session.log` (normally under `/run/user/<uid>`).
The log survives service stops and failed starts, so users
can collect the log after returning to the TTY. Runtime files are temporary
and may be removed after logout or reboot. Errors from the launcher or systemd
before Halley starts may instead appear on the terminal or in
`journalctl --user -u halley.service`.

A direct native `halley` or `halley --session` launch also owns the systemd
graphical-session lifecycle: it starts `halley-direct-session.target` after its
listeners are ready. That target pulls in `graphical-session.target`, which
refuses manual starts, without launching another compositor. Install the direct
session target alongside the other systemd resources. Halley stops session
services and clears display variables
on exit. A managed launch leaves that cleanup to the launcher. Nested sessions
do not start or stop host session targets. `HALLEY_NO_INIT_INTEGRATION=1`
disables this target management too.

The runit and s6 files under `packaging/` are examples for personal user
supervision trees; those managers do not have a single standard distribution
path for graphical user services. The OpenRC README documents the direct
display-manager/login-shell setup.

The systemd portal drop-ins shipped under `packaging/systemd-user/` require an
active `graphical-session.target`. Install their directories alongside the main
units, then reload the user manager. They prevent D-Bus activation from starting
GTK without a display between sessions. Keep the same prerequisite on the
Halley backend when overriding its executable path. These drop-ins are for
sessions using the systemd graphical target; omit them for standalone sessions
with init integration disabled.

Native startup publishes its display environment and announces session readiness
before querying portal settings. The settings query runs on a worker so portal
startup can make Wayland requests without blocking the compositor event loop.
Direct logout stops configured startup groups and graphical-session services
before releasing the display. The native session guard remains an idempotent
cleanup fallback. Managed-session target cleanup remains owned by the launcher.
