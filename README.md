<h1 align="center">Halley</h1>

<p align="center"><em>Named after Halley's comet — periodic, precise, returning.</em></p>

<p align="center">
  <a href="https://saltnpepper97.github.io/halley-site/"><strong>Website</strong></a>
</p>

[![Sponsor](https://img.shields.io/badge/%E2%9D%A4-Support_Halley-ff69b4?style=for-the-badge)](#support-halley)
![License](https://img.shields.io/badge/license-GPL--3.0--only-blueviolet?style=for-the-badge)
![Status](https://img.shields.io/badge/status-active-brightgreen?style=for-the-badge)
![Wayland](https://img.shields.io/badge/display-Wayland-blue?style=for-the-badge)
![Rust](https://img.shields.io/badge/language-Rust-orange?style=for-the-badge)

---

> **Windows as nodes. Windows as clusters. Windows as your command center.**

Halley is a spatial Wayland compositor built for multi-monitor desktops. Each
display has an independent infinite Field: a camera over freely overlapping
windows, collapsed node landmarks, and clusters assembled around the work you
actually want to keep together. Windows can decay when they leave your active
area, return through history-aware navigation, or remain pinned as durable
landmarks.

---

## Demo

![Halley Field with overlapping windows](demo/demo1.png)
![Halley desktop with an editor and browser](demo/demo2.png)

---

## Get Started

Before installing, check the [requirements](#requirements).

For Nix and NixOS, see [the flake and NixOS setup guide](docs/nix.md).
For CI coverage and local checks, see [testing](docs/testing.md).

Build the complete workspace:

```sh
git clone https://github.com/saltnpepper97/halley
cd halley
cargo build --release --workspace
```

The compositor workspace builds Halley, its CLI, and its portal. Install
[Lift](https://github.com/saltnpepper97/halley-lift) separately with
`cargo install halley-lift --version 0.3.0 --locked`, or through its own package.

The build produces:

```text
target/release/halley
target/release/halleyctl
target/release/xdg-desktop-portal-halley
```

For user-local testing:

```sh
install -Dm755 target/release/halley ~/.local/bin/halley
install -Dm755 target/release/halleyctl ~/.local/bin/halleyctl
install -Dm755 target/release/xdg-desktop-portal-halley \
  ~/.local/bin/xdg-desktop-portal-halley
```

Run a nested development compositor with:

```sh
halley --winit
```

Use `halley-session` or `halley --session` for a native desktop session. See
[the packaging guide](docs/packaging.md) for display-manager assets, portal
metadata, systemd, dinit, runit, s6, OpenRC, and distribution paths.

---

## First Run

Your first native session shows a **Halley basics** card with the essential
controls. Dismiss it with `Enter`, `Escape`, or a click, and reopen it from
Lift or with `halleyctl basics`. The card uses your configured modifier key.
Nested `halley --winit` sessions do not show it automatically.

---

## Using the Field

Halley is Field-first. Applications open onto the Field, you position them as
you work, and you clean up when the visible Field becomes cluttered. The whole
daily workflow is one loop:

> Launch freely → position and overlap naturally → arrange when the visible
> Field becomes messy → collapse work intentionally → retrieve it spatially →
> use clusters later only when deliberately configured.

1. **Launch freely.** `Super+D` opens [Lift](https://github.com/saltnpepper97/halley-lift), the separately installed launcher, so whatever you start lands
   directly on the Field.
2. **Position and overlap naturally.** `Super+Left-drag` moves a window, and
   ordinary windows are free to overlap instead of being solved into slots.
3. **Arrange when the visible Field becomes messy.** `Super+A` gathers the
   visible windows into a balanced mosaic and remembers the geometry each one
   came from.
4. **Collapse work intentionally.** `Super+N` collapses the focused window into
   its clickable node — the marker that keeps the window's place on the Field —
   and restores it again.
5. **Retrieve it spatially.** `Super+Arrow` walks to the nearest window or node;
   the retrieval layers below cover recent work, offscreen work, and everything
   open across your monitors.
6. **Use clusters later, only when deliberately configured.** A cluster is an
   optional named context, not the way Halley expects you to organize windows.

Each monitor has its own Field. Clusters and their numbered slots are optional;
you can declare them in your configuration or create them as needed.

### Retrieval

Halley keeps five retrieval mechanisms because they answer five different
questions. They are layers, not replacements for each other.

| Mechanism | Binding | Question it answers |
|---|---|---|
| Directional focus | `Super+Arrow` | Nearby spatial navigation — which window or node is next in this direction? |
| Focus carousel | `Alt+Tab` / `Alt+Shift+Tab` | Recent-work navigation — what did I just come from? |
| Bearings | `Super+Z` hold / `Super+Shift+Z` toggle | Offscreen spatial retrieval — where did work go beyond this monitor's view? |
| Apogee | `Super+O` | Visual inventory across monitors — what is open on every display? |
| Lift | `Super+D` | Direct search by application, node, cluster, or compositor action — what is this called? |


---

## Concepts

| Term | What it is |
|---|---|
| **Field** | An infinite, zoomable 2D canvas with an independent camera on each monitor. |
| **Node** | A managed window's stable identity and its collapsed representation on the Field. |
| **Focus Ring** | An elliptical active region used by decay and navigation policy. |
| **Decay** | Configurable transitions that collapse inactive windows outside the focus ring. |
| **Landmark** | A collapsed or pinned object that remains spatially meaningful while ordinary windows overlap freely. |
| **Cluster** | A group of windows that opens into its own tiling or stacking workspace. |
| **Core** | The collapsed Field representation of a cluster. |
| **Trail** | Per-monitor focus history with backward, forward, list, and exact goto controls. |
| **Bearings** | A directional overlay for finding and navigating offscreen nodes and cluster cores. |
| **Apogee** | A multi-monitor overview with live compositor-owned previews. |

---

## The Field

Multi-monitor behavior is not an extension of one shared workspace. Every
output owns its own camera, focus history, active cluster, fullscreen state,
maximize state, and focus-ring policy.

- **Independent cameras** — pan and zoom one monitor without disturbing the
  others.
- **Free overlap** — normal active windows overlap by default instead of being
  forced through a global no-overlap solver.
- **Decay and landmarks** — inactive windows can collapse into durable nodes;
  pinned nodes stay fixed until explicitly moved.
- **Move or carry** — Mod+drag crosses monitor boundaries, while
  Mod+Shift+drag dwells at an edge to carry the window through its current
  output's Field.
- **Trail navigation** — walk backward and forward through each monitor's
  recent Field focus history.
- **Directional focus** — the same action vocabulary adapts to the Field,
  tiling clusters, and stacking clusters.

The focus ring is an invisible ellipse around the useful part of a monitor's
camera. Its dimensions and offset are configurable per output. Windows outside
that ring become candidates for timer-driven decay rather than disappearing
because an arbitrary global window count was exceeded.

See [nodes and decay](docs/nodes.md) for configuration and behavior.

---

## Clusters

Clusters are optional named contexts that you configure deliberately. A runtime-created cluster is a named context assembled from windows that
are already on the Field; nothing creates a cluster implicitly, and a session
without clusters is a complete Halley session. You can instead declare a
startup cluster explicitly under `autostart`.

Enter cluster mode to open the Cluster Composer on the selected monitor.
Eligible windows and collapsed nodes animate into a stable, non-overlapping
mosaic. Use the arrow keys or pointer to focus a card, `Space` or left click to
toggle membership, `Enter` to name the draft, and `Escape` to step back or
cancel. Selected cards keep a persistent tint and checkmark distinct from the
focus frame. Confirming creates a core on the Field; hovering a core can bloom
member previews around it, and opening it enters a monitor-local workspace
while leaving the Field geometry intact behind it.

Two layouts are available:

- **Tiling** — a master-and-stack layout with directional focus, tile swaps,
  overflow, and per-member floating.
- **Stacking** — an overlapping deck with focused-card cycling and retained
  order.

Cluster slots are per monitor. Opening the active slot again returns to the
unchanged Field.

---

## Default Keybinds

The canonical defaults live in
[`examples/halley.rune`](examples/halley.rune). Keybinds are configurable,
context-scoped, side-aware, and shared by keyboard, pointer-button, wheel,
swipe, and hold actions.

| Category | Keybind | Action |
|---|---|---|
| Basic | `Super+Shift+E` | Open Halley's quit confirmation |
| Basic | `Super+Q` | Close the focused window |
| Basic | `Super+F` | Toggle fullscreen |
| Basic | `Super+M` | Toggle Field maximize |
| Basic | `Super+N` | Collapse the focused window into its node, or restore it (a collapsed node also restores on click) |
| Basic | `Super+P` | Pin or unpin the focused window |
| Overview | `Super+O` | Toggle Apogee, the visual inventory across monitors |
| Focus | `Alt+Tab` / `Alt+Shift+Tab` | Recent-work navigation: cycle the focus carousel forward/backward |
| Focus | `Super+Arrow` | Nearby spatial navigation: directional focus in the active context |
| Focus | `Super+H` | Center the last-focused Field window |
| Trail | `Super+,` / `Super+.` | Previous/next Trail entry |
| Monitor | `Super+Shift+Arrow` | Focus an adjacent monitor |
| Move | `Super+Alt+Arrow` | Move the focused Field node; [custom distances](docs/keybinds.md#actions) are optional |
| Resize/Tile | `Super+Ctrl+Arrow` | Resize in the Field or swap in a tiling cluster |
| Arrange | `Super+A` | Reversible cleanup: gather visible Field windows into a mosaic, or press again to restore their saved geometry |
| Clusters | `Super+Shift+C` | Enter cluster creation mode |
| Clusters | `Super+L` | Cycle cluster layout |
| Clusters | `Super+V` | Toggle the focused cluster member floating |
| Clusters | `Super+0..9` | Open a per-monitor cluster slot |
| Bearings | `Super+Z` / `Super+Shift+Z` | Offscreen retrieval: hold or toggle Bearings |
| Launch | `Super+T` | Open the first supported terminal |
| Launch | `Super+D` | Open Lift to search applications, nodes, clusters, and compositor actions (Fuzzel is a commented alternative) |
| Reload | `Super+Shift+R` | Reload the selected configuration |
| Zoom | `Super+-` / `Super+=` / `Super+Shift+0` | Zoom out, in, or reset |
| Pointer | `Super+Left Mouse` | Move a window |
| Pointer | `Super+Right Mouse` | Smoothly resize a window |
| Pointer | `Left Mouse` on empty Field | Pan the Field |
| Screenshot | `Print` | Open native capture |

The same chord may be assigned distinct actions in `field`, `cluster`, `tile`,
and `stack` scopes. Left/right Super, Alt, Ctrl, and Shift can be matched
independently. Compositor move, resize, and pan grabs are ordinary remappable
bindings rather than hardcoded mouse policy.

`Super+D` opens [Lift](https://github.com/saltnpepper97/halley-lift),
the independently packaged search and action launcher. It searches applications,
running nodes, clusters, compositor actions, and config files from one field,
and it can run terminal commands. Prefer a separate launcher? Any non-built-in
action string is a command line, so replacing one line is enough:

```rune
"$var.mod+d" "fuzzel"
```

Existing configurations keep their current launcher binding. Newly generated
configs default to Lift.

---

## Configuration

On first launch Halley creates
`$XDG_CONFIG_HOME/halley/halley.rune`, falling back to
`~/.config/halley/halley.rune`, from the canonical
[`examples/halley.rune`](examples/halley.rune) template. Startup never modifies
an existing config, and configs need no version marker. Halley does not back up,
migrate, or replace existing configs. Update settings and bindings yourself,
using the example as a reference, and check the result with
`halleyctl config verify`. Omitted settings use built-in defaults; an
incompatible config reports an error and remains unchanged.

Pass `-c PATH` or `--config PATH` to select another file. Valid edits reload as
one atomic snapshot; invalid edits leave the last valid runtime state active.
Nested Rune `gather` dependencies are watched recursively, including missing
dependencies that are created after startup.

Per-monitor display scaling uses a numeric multiplier: `scale 1.0` means
100%, `scale 1.5` means 150%. It changes the size of applications and Halley
UI while retaining the native display resolution. See [display scaling](docs/display-scale.md)
for mixed-monitor positioning and examples.

A freshly generated config declares no startup clusters, so windows begin on the
empty Field. The optional `autostart` section can still declare persistent named
clusters using compact command arrays, including empty `members []`
declarations. See
[startup clusters](docs/clusters.md#startup-clusters) for syntax, launch
attribution, output placement, and restart behavior.

Autostart command output and exit status are saved in private, bounded logs
under `~/.local/state/halley/autostart` (or `$XDG_STATE_HOME/halley/autostart`).
See [autostart logs](docs/autostart.md) for startup diagnostics and retention.

Useful controls:

```sh
halleyctl config verify
halleyctl config edit
halleyctl reload
```

`halleyctl` also exposes output and DPMS state, capture modes, node and cluster
control, Trail, named-monitor focus, stack/tile navigation, Bearings, and portal
diagnostics. Run `halleyctl --help` for the current command surface.

For automated screenshots, use `halleyctl capture screen -o OUTPUT`,
`halleyctl capture window NODE_ID` (IDs from `halleyctl node list`), or
`halleyctl capture region -o OUTPUT --rect X Y WIDTH HEIGHT`. These save
immediately without confirmation and show the normal saved-image notification.
Add `--silent` to suppress the notification. Rectangle coordinates
are output-relative logical pixels. Omit `--rect` for interactive region
selection, or use `halleyctl capture menu` for the native menu. See the
[capture API](docs/api.md#screenshots-and-automation).

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

- [Configuration template](examples/halley.rune)
- [Keybind triggers and actions](docs/keybinds.md)
- [Compositor API](docs/api.md)
- [Field behavior, maximize, zoom, and close succession](docs/field.md)
- [Nodes, decay, focus rings, landmarks, and physics](docs/nodes.md)
- [Bearings](docs/bearings.md)
- [Apogee and Alt+Tab](docs/apogee.md)
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

<a id="support-halley"></a>

## Community / Support

Join the [Halley Discord](https://discord.gg/J2ec3nbHYs) for support,
configuration help, bug reports, packaging, and release updates.

Sponsorship helps fund documentation, testing, packaging, compatibility, and
ongoing development. Halley remains maintainer-directed.

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
