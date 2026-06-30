<h1 align="center">Halley</h1>

<p align="center"><em>Named after Halley's comet — periodic, precise, returning.</em></p>

<p align="center">
  <a href="https://saltnpepper97.github.io/halley-site/"><strong>Website</strong></a>
</p>

[![Sponsor](https://img.shields.io/badge/%E2%9D%A4-Support_Halley-ff69b4?style=for-the-badge)](#support-the-next-leap)
![License](https://img.shields.io/badge/license-GPL--3.0--only-blueviolet?style=for-the-badge)
![Status](https://img.shields.io/badge/status-active-brightgreen?style=for-the-badge)
![Wayland](https://img.shields.io/badge/display-Wayland-blue?style=for-the-badge)
![Build](https://img.shields.io/badge/build-passing-success?style=for-the-badge)
![Rust](https://img.shields.io/badge/language-Rust-orange?style=for-the-badge)

---

> **Windows as nodes. Windows as clusters. Windows as your command center.**

Halley is a Wayland compositor built from the ground up for multi-monitor setups. Each display gets its own independent infinite canvas. Windows live as nodes on those canvases, group into clusters you build intentionally, and decay gracefully when they drift out of focus. Inspired by the comet it's named after — periodic, precise, and always returning — Halley makes multi-monitor work feel deliberate rather than chaotic.

---

## Support the Next Leap

Halley will continue receiving updates, fixes, protocol work, and polish. The project is active, and the core direction is not being paused or held hostage.

The larger leap is different. A full Wayland desktop ecosystem can only be taken so far as a solo project. Sponsorship helps fund the boring-but-important work that makes Halley more durable, approachable, and useful over time — documentation, testing, packaging, compatibility, triage, tooling, and release work — alongside larger technical improvements.

Sponsorship does **not** buy roadmap control. Halley remains maintainer-directed. Support helps create the time and stability needed to execute on that direction responsibly.

### Sponsorship Stretch Goals

- A real Halley website, beyond a basic GitHub Pages presence.
- A major Rune-CFG upgrade so it can become a larger foundation for future Halley UI and app work, not only a config language.
- A much stronger `halley-api` for plugins, integrations, ecosystem tooling, and external developers.
- A system for creating full Halley ecosystem apps using Rune-CFG plus light Rust, mostly through `halley-api`.
- Documentation, onboarding, examples, migration notes, troubleshooting, and developer guides.
- Packaging, testing, CI, compatibility, hardware/device testing, crash/debug tooling, and other infrastructure work.
- Funding or compensating a community maintainer for triage, Discord/community support, docs cleanup, bug reproduction, and release coordination.
- Better outreach: demos, release posts, videos, dev logs, showcases, and broader Linux desktop visibility.

---

## Demo

![Halley demo](demo/demo.png)
![Halley demo-1](demo/demo-1.png)

---

## Concepts

A quick orientation before diving in.

| Term | What it is |
|---|---|
| **Field** | An infinite 2D canvas, one per monitor. Everything lives here. Zoomable and pannable. |
| **Node** | A window on the Field — open, collapsed, or a cluster core. |
| **Focus Ring** | An invisible eye-shaped region defining your active area. Windows outside it are candidates for decay. |
| **Decay** | Nodes that drift outside the focus ring dim or collapse over time. Optional and configurable. |
| **Cluster** | Halley's answer to workspaces — a contained layout you build intentionally from a set of windows. |
| **Core** | The collapsed form of a cluster on the Field. Expands into a petal arrangement of window previews. |
| **Trail** | History-aware navigation — step backward and forward through recent focus changes. |
| **Bearings** | A lightweight directional overlay for orienting movement and navigation around the current view. |
| **Jump** | Move a grabbed window across monitors, traversing between Fields, with a single keybind. |

---

## The Field

Multi-monitor is a first-class concept in Halley — not an afterthought. Each monitor gets its own infinite canvas, completely independent from every other display. The Field is zoomable, pannable, and isolated per monitor.

- **Per-monitor** — displays don't share state; each Field is its own world
- **Max windows** — configurable cap on open nodes per Field
- **Decay** — opt-in clutter management based on focus ring position; a small overlap tolerance prevents edge-case false positives
- **Jump** — grab a window and send it to another monitor's Field with one keybind; `Super+Shift+LeftMouse` for a pointer-driven field jump

The **Focus Ring** is the heart of the Field. It's an invisible eye-shaped region centered on your view — windows that fall significantly outside it over time become candidates for decay. You can make it briefly visible via config; it fades out after a moment. Size and shape are fully configurable.

---

## Clusters

Clusters are Halley's answer to workspaces — but you build them yourself, intentionally, rather than having them auto-generated.

### Building a cluster

Enter cluster mode, then click or mark the windows you want to group. Press `Enter` to form the cluster, or `Esc` to cancel and return to the Field. Once formed, the cluster collapses into a **core node** on the Field — a single handle representing the whole group.

### The core

Clicking a core within the focus ring **enters** the cluster. Expanding it fans the windows out in a **petal arrangement** — clockwise or counter-clockwise — as icon-sized previews around the core. From there you can:

- Pull windows out into the Field
- Bring Field windows in
- Collapse it back into the core

### Inside a cluster

Once inside, you leave the Field entirely. The cluster is its own contained space with one of two layout modes:

**Tiling** — Weighted tiling. Windows are arranged by assigned weight and recency.

**Stacking** — Windows layered in a navigable stack, similar to a mobile app switcher. Navigate with keybinds, reorder the stack as needed.

---

## Systems

| System | Description |
|---|---|
| **Field** | Per-monitor infinite canvases with zoom and pan |
| **Clusters** | Core nodes, cluster entry/exit, tiling, stacking, drag reordering |
| **Focus Ring** | Configurable active region with optional preview |
| **Decay** | Optional clutter reduction outside the focus ring |
| **Trail** | Recent-focus navigation — back and forward |
| **Bearings** | Directional overlays and navigation cues |
| **Jump / Field Jump** | Fast cross-monitor grabbed-window movement |
| **IPC** | Unix socket control at `$XDG_RUNTIME_DIR/halley/halley.sock` |
| **Xwayland** | On-demand support via `xwayland-satellite` |

---

## Requirements

Halley targets a native Linux Wayland session and expects:

- A DRM/KMS-capable graphics stack with GBM/EGL/OpenGL support
- A seat/session backend through `libseat` such as `seatd` or logind
- `libinput` and `udev` access on a real TTY for the native backend
- Rust and Cargo if you are building from source

Optional but commonly needed:

- `xwayland-satellite` for X11 app support
- Halley's native `xdg-desktop-portal-halley` backend for portal-driven screen/window sharing, plus `xdg-desktop-portal-gtk` for common file/dialog portals
- `fuzzel` plus a Wayland terminal such as `ghostty`, `kitty`, `foot`, `wezterm`, `alacritty`, `rio`, or `contour` if you use the default launch bindings

---

## Install

### AUR

    yay -S halley

or

    paru -S halley

Or for the latest commit:

    yay -S halley-dev

or

    paru -S halley-dev

### From Source

    git clone https://github.com/saltnpepper97/halley
    cd halley
    cargo build --release

The compositor, control CLI, and portal backend binaries will be available at:

    target/release/halley
    target/release/halleyctl
    target/release/xdg-desktop-portal-halley

For local testing without system-wide binaries, install them into `~/.local/bin`:

    install -Dm755 target/release/halley ~/.local/bin/halley
    install -Dm755 target/release/halleyctl ~/.local/bin/halleyctl
    install -Dm755 target/release/xdg-desktop-portal-halley ~/.local/bin/xdg-desktop-portal-halley

Then register the user-local portal service and metadata:

    install -Dm644 packaging/xdg-desktop-portal/portals/halley.portal ~/.local/share/xdg-desktop-portal/portals/halley.portal
    mkdir -p ~/.local/share/dbus-1/services ~/.config/xdg-desktop-portal
    printf '[D-BUS Service]\nName=org.freedesktop.impl.portal.desktop.halley\nExec=%s/.local/bin/xdg-desktop-portal-halley\n' "$HOME" > ~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.halley.service
    install -Dm644 packaging/xdg-desktop-portal/halley-portals.conf ~/.config/xdg-desktop-portal/halley-portals.conf

Check the installed portal path and advertised capture support with:

    halleyctl portal status

### Display Manager Session

Halley's native session needs to start the tty backend rather than the nested `winit` backend. This repo now ships the assets needed for display managers such as SDDM and LightDM:

- `packaging/wayland-sessions/halley-session`
- `packaging/wayland-sessions/halley.desktop`

Install them to the standard system locations alongside the compositor binary:

    sudo install -Dm755 target/release/halley /usr/bin/halley
    sudo install -Dm755 packaging/wayland-sessions/halley-session /usr/bin/halley-session
    sudo install -Dm644 packaging/wayland-sessions/halley.desktop /usr/share/wayland-sessions/halley.desktop
    sudo install -Dm644 packaging/systemd-user/halley.service /usr/lib/systemd/user/halley.service
    sudo install -Dm644 packaging/systemd-user/halley-shutdown.target /usr/lib/systemd/user/halley-shutdown.target

`halley-session` is the recommended public launcher for a full Halley desktop session. It will start `halley.service` when a user systemd instance is available, which makes `graphical-session.target`, `xdg-desktop-autostart.target`, and related user-session units behave correctly under display managers like SDDM. If those units are not installed, the launcher falls back to executing `halley` directly.

The compositor also accepts `halley --session` for session wrappers, packagers, and service files. Normal users should prefer `halley-session`.

After that, `Halley` should appear in Wayland-capable display managers.

---

## Default Keybinds

Defaults follow Halley's shipped fresh-config template.

| Category | Keybind | Action |
|---|---|---|
| Basic | `Super+Shift+r` | Reload config |
| Basic | `Super+n` | Toggle state |
| Basic | `Super+q` | Close focused window |
| Quit | `Super+Shift+e` | Quit Halley |
| Zoom | `Super+MouseWheelUp` | Zoom in |
| Zoom | `Super+MouseWheelDown` | Zoom out |
| Zoom | `Super+MiddleMouse` | Reset zoom |
| Move | `Super+Left` | Move node left |
| Move | `Super+Right` | Move node right |
| Move | `Super+Up` | Move node up |
| Move | `Super+Down` | Move node down |
| Monitor | `Super+Shift+Left` | Focus monitor left |
| Monitor | `Super+Shift+Right` | Focus monitor right |
| Monitor | `Super+Shift+Up` | Focus monitor up |
| Monitor | `Super+Shift+Down` | Focus monitor down |
| Clusters | `Super+Shift+c` | Enter cluster mode |
| Clusters | `Super+l` | Cycle cluster layout |
| Bearings | `Super+z` | Show bearings |
| Bearings | `Super+Shift+z` | Toggle bearings |
| Trail | `Super+,` | Trail previous |
| Trail | `Super+.` | Trail next |
| Launch | `Super+Return` | Open terminal |
| Launch | `Super+d` | Launch `fuzzel` |
| Pointer | `Super+LeftMouse` | Move window |
| Pointer | `Super+RightMouse` | Resize window |
| Pointer | `Super+Shift+LeftMouse` | Field jump |
| Screenshot | `Super+Shift+s` | Open capture menu |
| Tile | `Super+Left/Right/Up/Down` | Focus tile in that direction |
| Tile | `Super+Ctrl+Left/Right/Up/Down` | Swap tile in that direction |
| Stacking | `Super+Left` | Cycle stack forward |
| Stacking | `Super+Right` | Cycle stack backward |
| Media | `XF86AudioRaiseVolume` | Raise volume |
| Media | `XF86AudioLowerVolume` | Lower volume |
| Media | `XF86AudioMute` | Toggle mute |

---

## Configuration

On first launch Halley bootstraps `~/.config/halley/halley.rune` for you from an internal fully documented template, inserting detected tty monitors into the `viewport` section. Normal config precedence is `--config`/`-c`, then `HALLEY_WL_CONFIG`, then `~/.config/halley/halley.rune`, then `/etc/halley/halley.rune`, then generated user config/internal defaults. Use `halley --config /path/to/halley.rune` or `halley -c /path/to/halley.rune` to force a specific file.

Handled by `crates/halley-config`. Covers input settings like repeat/focus mode, keybinds, focus ring shape and size, decay threshold, max windows per Field, viewports, autostart programs and much **more**.

## Community / Support

Halley has a Discord for practical support, bug triage, release updates, packaging discussion, and focused contributor coordination.

Halley remains maintainer-directed. Discord is not a roadmap vote or public steering committee. Please read the rules and start in `#intake` so you can be routed to support, config help, bugs, packaging, contributing, or release-only updates.

Join the Discord: https://discord.gg/cjutpDv6q

## Contributing

View the [contributing](CONTRIBUTING.md) guidelines before making any pull requests.

---

## Portals To Use

- `xdg-desktop-portal-halley` for ScreenCast, including monitor and window sharing
- `xdg-desktop-portal-gtk` for common desktop dialogs not implemented by Halley

---

## Website

**Project website:** [saltnpepper97.github.io/halley-site](https://saltnpepper97.github.io/halley-site/)

---

## Inspirations

- [niri](https://github.com/niri-wm/niri) — for how to do Wayland compositor things in Rust
- [vxwm](https://codeberg.org/wh1tepearl/vxwm) — for studying some of its eyecandy
- [hevel](https://sr.ht/~dlm/hevel/) — for zoooooooom
- [Hyprland](https://github.com/hyprwm/hyprland) — for some config organization and eyecandy
- [newm](https://github.com/jbuchermn/newm) — Godfather of spatial compositing

---

## License

Released under the [**GPL-3.0**](LICENSE) license.
