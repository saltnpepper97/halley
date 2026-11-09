<h1 align="center">Halley</h1>

<p align="center"><em>Named after Halley's comet — periodic, precise, returning.</em></p>

<p align="center">
  <a href="https://saltnpepper97.github.io/halley-site/"><strong>Website</strong></a>
</p>

![License](https://img.shields.io/badge/license-GPL--3.0--only-blueviolet?style=for-the-badge)
![Status](https://img.shields.io/badge/status-active-brightgreen?style=for-the-badge)
![Wayland](https://img.shields.io/badge/display-Wayland-blue?style=for-the-badge)
![Rust](https://img.shields.io/badge/language-Rust-orange?style=for-the-badge)

---

> **A spatial desktop where windows drift away when you're done and return when you need them.**

Halley is a spatial Wayland compositor built for multi-monitor desktops. Each
display has its own infinite Field: a camera over freely overlapping windows.
Windows you stop using collapse into nodes that keep their place, and come back
when you reach for them again.

---

## Demo

![Halley Field with overlapping windows](demo/demo1.png)
![Halley desktop with an editor and browser](demo/demo2.png)

---

## Get Started

Before installing, check the [requirements](#requirements).

For Nix and NixOS, see [the flake and NixOS setup guide](docs/nix.md).
For CI coverage and local checks, see [testing](docs/testing.md).

1. **Build Halley.**

   ```sh
   git clone https://github.com/saltnpepper97/halley
   cd halley
   cargo build --release --workspace
   ```

   This builds the compositor, its CLI, and its portal:

   ```text
   target/release/halley
   target/release/halleyctl
   target/release/xdg-desktop-portal-halley
   ```

2. **Install it.** For user-local testing:

   ```sh
   install -Dm755 target/release/halley ~/.local/bin/halley
   install -Dm755 target/release/halleyctl ~/.local/bin/halleyctl
   install -Dm755 target/release/xdg-desktop-portal-halley \
     ~/.local/bin/xdg-desktop-portal-halley
   ```

3. **Install Lift.** [Lift](https://github.com/saltnpepper97/halley-lift#install)
   is Halley's launcher, packaged separately, and `Super+D` opens it. Without
   it, `Super+T` still opens a terminal.

4. **Run it.** `halley --winit` starts a nested development compositor inside
   your current desktop. Use `halley-session` or `halley --session` for a
   native desktop session. See [the packaging guide](docs/packaging.md) for
   display-manager assets, portal metadata, systemd, dinit, runit, s6, OpenRC,
   and distribution paths.

---

## First Run

Your first native session shows a **Halley basics** card with the essential
controls, labelled with your own keybinds. Dismiss it with `Enter`, `Escape`,
or a click, and reopen it from Lift or with `halleyctl basics`. Nested
`halley --winit` sessions do not show it automatically.

---

## Using the Field

Halley is Field-first. Applications open onto the Field, you position them as
you work, and you clean up when the visible Field becomes cluttered. The whole
daily workflow is one loop:

> Launch freely → position and overlap naturally → arrange when the visible
> Field becomes messy → collapse work intentionally → retrieve it spatially.

1. **Launch freely.** `Super+D` opens Lift, so whatever you start lands
   directly on the Field.
2. **Position and overlap naturally.** `Super+Left-drag` moves a window, and
   ordinary windows are free to overlap instead of being solved into slots.
3. **Arrange when the visible Field becomes messy.** `Super+A` lays out the
   visible windows so none overlap, keeping each one's shape, and remembers
   where each one came from.
4. **Collapse work intentionally.** `Super+N` collapses the focused window into
   its clickable node — the marker that keeps the window's place on the Field —
   and restores it again.
5. **Retrieve it spatially.** See [Retrieval](#retrieval) below.
6. **Optional: name a group of windows as a cluster.** `Super+Shift+C` picks
   windows for a cluster, which opens into its own tiling or stacking
   workspace. See [Clusters](#clusters).

### Retrieval

Finding a window again comes down to four questions:

- **What's next to this?** `Super+Arrow` moves focus to the nearest window or
  node in that direction.
- **What was I just using?** `Alt+Tab` / `Alt+Shift+Tab` cycles recent windows.
  Trail walks this monitor's focus history with `Super+,` and `Super+.`, and
  `Super+H` centers the last-focused window.
- **What's out there?** `Super+O` zooms out until you can see every window and
  node on every monitor; click one to zoom back in on it. For this monitor alone, hold `Super+Z` (or toggle it with
  `Super+Shift+Z`) to show Bearings: chips at the screen edges pointing to
  offscreen nodes and clusters.
- **What's it called?** `Super+D` opens Lift to search applications, running
  nodes, clusters, and compositor actions by name.

---

## Concepts

| Term | What it is |
|---|---|
| **Field** | An infinite, zoomable 2D canvas with an independent camera on each monitor. |
| **Node** | A window collapsed into a small marker that keeps its place on the Field. It is still running. |
| **Decay** | Timers that collapse windows you have stopped using into nodes. |
| **Pin** | Exempts a window from decay. You can still collapse and restore it yourself. |
| **Arrange** | Lays out the visible windows without overlap, and puts them back when pressed again. |
| **Cluster** | An optional named group of windows that opens into its own tiling or stacking workspace. |
| **Trail** | Per-monitor focus history you can walk backward and forward. |
| **Bearings** | Edge chips pointing to offscreen nodes and clusters on this monitor. |
| **Lift** | Halley's separately installed launcher and search. |

---

## The Field

Multi-monitor behavior is not an extension of one shared workspace. Every
output owns its own camera, focus history, active cluster, fullscreen state,
maximize state, and decay policy.

- **Independent cameras** — pan and zoom one monitor without disturbing the
  others.
- **Free overlap** — normal active windows overlap by default instead of being
  forced through a global no-overlap solver.
- **Decay and pins** — windows you stop using collapse into nodes after a
  timer; pinned windows never decay.
- **Move or carry** — Mod+drag crosses monitor boundaries, while
  Mod+Shift+drag dwells at an edge to carry the window through its current
  output's Field.
- **Directional focus** — the same action vocabulary adapts to the Field,
  tiling clusters, and stacking clusters.

See [nodes and decay](docs/nodes.md) for configuration and behavior.

---

## Clusters

Clusters are optional named contexts assembled from windows on the Field or
declared at startup. Each cluster opens into a monitor-local workspace with a
tiling or stacking layout, leaving the Field behind it intact.

Use `Super+Shift+C` to pick windows for a new cluster right on the Field, then `Super+1` through `Super+0` to open a monitor's cluster slots.
See [clusters](docs/clusters.md) for creation, layouts, and startup
configuration.

---

## Essential Keybinds

| Keybind | Action |
|---|---|
| `Super+D` | Open Lift |
| `Super+T` | Open a terminal |
| `Super+Q` | Close the focused window |
| `Super+Left-drag` | Move a window |
| `Super+Right-drag` | Resize a window |
| `Super+A` | Arrange visible windows, or put them back |
| `Super+N` | Collapse the focused window into a node, or restore it |
| `Super+P` | Pin or unpin the focused window |
| `Super+F` | Toggle fullscreen |
| `Super+Arrow` | Focus the nearest window or node in that direction |
| `Alt+Tab` | Cycle recent windows |
| `Super+O` | Zoom out to see everything, or zoom back in |
| `Super+Shift+E` | Quit Halley |

Every binding is configurable. See [keybinds](docs/keybinds.md) for the full
list of defaults, actions, and trigger types.

---

## Configuration

On first launch Halley writes a short [starter config](examples/halley.rune) to
`$XDG_CONFIG_HOME/halley/halley.rune` (or `~/.config/halley/halley.rune`), and
edits to it reload live. Every other option is documented on the
[website](https://saltnpepper97.github.io/halley-site/wiki/config) and in the
[complete reference](examples/halley.reference.rune).

```sh
halleyctl config edit
halleyctl config verify
halleyctl reload
```

---

## Requirements

Halley targets a native Linux Wayland session and expects:

- A DRM/KMS graphics stack with GBM, EGL, and OpenGL ES support
- A seat/session backend through `libseat`, such as `seatd` or logind
- `libinput` and `udev` access for the native TTY backend
- Rust and Cargo when building from source

Optional desktop components:

- The `Xwayland` executable for X11 applications; Halley manages it natively
- `xdg-desktop-portal` and a settings/file-dialog backend such as
  `xdg-desktop-portal-gtk`

Halley does not require systemd. The default build supports systemd user
sessions, while dinit and direct init-agnostic launch paths are also packaged.

---

## API and External Tools

[`halley-api`](docs/api.md) is the supported Rust boundary for launchers,
panels, automation, tests, and desktop components. It provides typed commands
and queries, structured errors, capability negotiation, persistent clients,
and sequenced state subscriptions.

The postcard-based `halley-ipc` crate is Halley's private transport codec, not
an external compatibility contract. External programs should use
`halley-api`; `halleyctl` and Lift are reference consumers of that API.

---

## Portals To Use

- `xdg-desktop-portal-halley` for screenshots and ScreenCast sources
- `xdg-desktop-portal-gtk` for common desktop dialogs Halley does not provide

Check the installed backend and advertised capture support with:

```sh
halleyctl portal status
```

---

## References

- [Starter configuration](examples/halley.rune)
- [Complete configuration reference](examples/halley.reference.rune)
- [Keybind triggers and actions](docs/keybinds.md)
- [Older configuration and action spellings](docs/older-spellings.md)
- [Compositor API](docs/api.md)
- [Field behavior, maximize, zoom, and close succession](docs/field.md)
- [Nodes, decay, decay zones, landmarks, and physics](docs/nodes.md)
- [Bearings](docs/bearings.md)
- [Zoom to fit](docs/zoom-fit.md)
- [Alt+Tab](docs/alt-tab.md)
- [Animations](docs/animations.md)
- [Window decorations](docs/decorations.md)
- [Backdrop blur and shadows](docs/effects.md)
- [Managed-window rules](docs/window-rules.md)
- [Wallpaper](docs/wallpaper.md)
- [Screen sharing and the desktop portal](docs/portal.md)
- [Wayland protocol support](docs/wayland-protocols.md)
- [XWayland policy and conformance](docs/xwayland.md)
- [Building, session managers, and packaging](docs/packaging.md)

---

## Community / Support

Join the [Halley Discord](https://discord.gg/J2ec3nbHYs) for support,
configuration help, bug reports, packaging, and release updates.

---

## Inspirations

- [niri](https://github.com/niri-wm/niri) — compositor architecture and careful Wayland behavior
- [vxwm](https://codeberg.org/wh1tepearl/vxwm) — visual experimentation
- [hevel](https://sr.ht/~dlm/hevel/) — spatial zooming
- [Hyprland](https://github.com/hyprwm/Hyprland) — configuration and visual ideas
- [newm](https://github.com/jbuchermn/newm) — spatial compositing

---

## License

Halley is distributed under the GPL-3.0-only license.
