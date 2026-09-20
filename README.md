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

Halley 0.6 is a ground-up compositor rewrite. It keeps the Field, nodes,
clusters, decay, Trail, Bearings, Apogee, Lift, and native capture experience,
but rebuilds their foundations around Smithay's GLES renderer, damage-aware
presentation, native embedded XWayland, and a typed public API.

---

## Start here: the normal Field loop

Halley is Field-first. Applications open onto the Field, you position them as
you work, and you clean up when the visible Field becomes cluttered. The whole
daily workflow is one loop:

> Launch freely → position and overlap naturally → arrange when the visible
> Field becomes messy → collapse work intentionally → retrieve it spatially →
> use clusters later only when deliberately configured.

1. **Launch freely.** `Super+D` opens Halley Lift, so whatever you start lands
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

`Super+A` and `Super+N` are ordinary reversible actions, not modes. `Super+A` is
reversible cleanup: it creates no tiling tree, no relationship, and no
persistent layout mode, every window stays independently movable, and pressing
it again restores the exact saved geometry. `Super+N` is a manual collapse you
can see happen, and a collapsed node is restored by clicking it or by pressing
`Super+N` on it again; automatic decay is a separate, conservative safety net
for genuinely abandoned work, described in
[nodes and decay](docs/nodes.md).

Halley does not start you on numbered workspaces. Each monitor owns its own
Field and applications live on it. Clusters and their numbered slots exist for
people who deliberately declare or create them, and the 0.8.0 first-run card
teaches no cluster controls at all.

### Retrieval

Halley keeps five retrieval mechanisms because they answer five different
questions. They are layers, not replacements for each other.

| Mechanism | Binding | Question it answers |
|---|---|---|
| Directional focus | `Super+Arrow` | Nearby spatial navigation — which window or node is next in this direction? |
| Focus carousel | `Alt+Tab` / `Alt+Shift+Tab` | Recent-work navigation — what did I just come from? |
| Bearings | `Super+Z` hold / `Super+Shift+Z` toggle | Offscreen spatial retrieval — where did work go beyond this monitor's view? |
| Apogee | `Super+O` | Visual inventory across monitors — what is open on every display? |
| Halley Lift | `Super+D` | Direct search by application, node, cluster, or compositor action — what is this called? |

The escalation is deliberate: the arrows move one step, `Alt+Tab` recalls recent
work, Bearings and Apogee show where things are, and Lift finds something by
name.

---

## Support Halley

Halley will continue receiving updates, fixes, protocol work, and polish. The
project is active, and its core direction is not being paused or held hostage.

Sponsorship helps fund the unglamorous work that makes a compositor durable:
documentation, testing, packaging, compatibility, triage, hardware debugging,
tooling, and release work. It creates more time to work carefully instead of
turning funding thresholds into product promises.

Sponsorship does **not** buy roadmap control. Halley remains
maintainer-directed.

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

Decay is conservative and optional. A newly generated config waits 10 minutes
outside the focus ring and 90 minutes inside it before an unfocused window
becomes a node; existing configs keep their own values, and a config without a
`decay:` section keeps Halley's shorter built-in delays. The first automatic
collapse explains itself once — `<Application> was collapsed into a node. Click
the node or press Mod+N to restore it.` — in a non-modal notice that never takes
input, and `Mod+N` collapses and restores windows by hand at any time.

A fresh Halley session begins on this empty Field. A newly generated config
declares no startup clusters, so applications you launch open directly into the
Field. Clusters are optional named contexts you add deliberately, either by
declaring startup clusters in `autostart` or by creating them at runtime.

---

## First Run

A newly generated configuration's first native session shows one compositor-owned
**Halley basics** card: the Field-first mental model, plus only the five
operations it depends on.

- `Super+D` — launch or search with Lift.
- `Super+Left-drag` — move a window.
- `Super+A` — arrange visible windows, or restore their saved geometry.
- `Super+N` — collapse or restore a window.
- `Super+O` — see everything in Apogee.

The card names your configured `mod` key, so a nested `halley --winit` session
shows `Alt+D` where a native session shows `Super+D`. It is a primer rather than
a tutorial: it lists no zoom, Bearings, Trail, pinning, or cluster layouts, it
never dims or blocks the desktop, and only its own dismissal keys are captured.
`Enter`, `Escape`, or a click closes it for good. Clusters stay out of first-run
training for 0.8.0 — the card names no cluster action, core, or layout — so you
only meet clusters when you deliberately configure them.

It appears only for a configuration Halley generated itself. Existing
configurations, nested `halley --winit` sessions, and explicitly selected
`-c PATH` files never show it automatically. Dismissal is remembered in
`$XDG_STATE_HOME/halley/state.rune`, falling back to
`~/.local/state/halley/state.rune`; that file is user state, not configuration,
is never migrated, and is safe to delete. Reopen the card whenever you like from
Halley Lift's **Show Halley basics** action or with `halleyctl basics`.

---

## Clusters

Clusters come last in the Field loop, and only when you deliberately configure
them. A cluster is a named context assembled from windows that are already on
the Field; nothing creates one for you, and a session without clusters is a
complete Halley session.

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

## Systems

| System | Description |
|---|---|
| **Field** | Per-monitor infinite canvases with independent pan, zoom, focus, and presentation state |
| **Nodes** | Stable identities, collapse/restore, pinning, icons, labels, and landmark behavior |
| **Clusters** | Draft creation, core nodes, tiling, stacking, slots, floating members, and animated transitions |
| **Decay** | Focus-ring and timer-driven clutter reduction |
| **Trail** | Per-output recent-focus navigation and remote inspection |
| **Bearings** | Directional overlays and offscreen navigation |
| **Apogee** | Multi-monitor overview and live previews |
| **Lift** | Bundled search and action launcher, bound to `Super+D` in fresh configs |
| **Capture** | Native menu, region, screen, and window screenshots plus portal screencasting |
| **IPC/API** | Persistent typed clients, capability discovery, subscriptions, and `halleyctl` |
| **XWayland** | Native embedded XWayland and compositor-owned X11 window management |

### Rewrite boundaries

The rewrite intentionally does not carry every old policy forward.

- Native embedded XWayland replaces `xwayland-satellite`; game input and
  pointer-lock bugs belong in the native path instead of a special game-mode
  exception layer.
- The old compositor-side game classifier, special game-mode exception layer,
  and Gamescope launch wrapper are not part of the rewritten compositor.
  Launchers can invoke Gamescope directly when desired.
- The supported `halley-api` is for external launchers, panels, automation,
  tests, and desktop components. Halley currently promises no in-process
  extension framework.
- The former adaptive top-edge client is not being ported. Any future shell in
  that space will be prototyped as a new design.
- Zoom tops out at native scale. Active windows are governed by focus-ring and
  timer policy rather than a maximum-window cap.

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

## Build and Install

Build the complete workspace:

```sh
git clone https://github.com/saltnpepper97/halley
cd halley
cargo build --release --workspace
```

The build produces:

```text
target/release/halley
target/release/halleyctl
target/release/halley-lift
target/release/xdg-desktop-portal-halley
```

For user-local testing:

```sh
install -Dm755 target/release/halley ~/.local/bin/halley
install -Dm755 target/release/halleyctl ~/.local/bin/halleyctl
install -Dm755 target/release/halley-lift ~/.local/bin/halley-lift
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
| Move | `Super+Alt+Arrow` | Move the focused Field node |
| Resize/Tile | `Super+Ctrl+Arrow` | Resize in the Field or swap in a tiling cluster |
| Arrange | `Super+A` | Reversible cleanup: gather visible Field windows into a mosaic, or press again to restore their saved geometry |
| Clusters | `Super+Shift+C` | Enter cluster creation mode |
| Clusters | `Super+L` | Cycle cluster layout |
| Clusters | `Super+V` | Toggle the focused cluster member floating |
| Clusters | `Super+0..9` | Open a per-monitor cluster slot |
| Bearings | `Super+Z` / `Super+Shift+Z` | Offscreen retrieval: hold or toggle Bearings |
| Launch | `Super+T` | Open the first supported terminal |
| Launch | `Super+D` | Open Halley Lift to search applications, nodes, clusters, and compositor actions (Fuzzel is a commented alternative) |
| Reload | `Super+Shift+R` | Reload the selected configuration |
| Zoom | `Super+-` / `Super+=` / `Super+Shift+0` | Zoom out, in, or reset |
| Pointer | `Super+Left Mouse` | Move a window |
| Pointer | `Super+Right Mouse` | Smoothly resize a window |
| Pointer | `Left Mouse` on empty Field | Pan the Field |
| Screenshot | `Print` | Open native capture |

A few rows carry the framing from the loop above. `Super+A` is reversible
cleanup rather than a tiling mode: it writes no layout, no relationship, and no
persistent mode, and a second press restores each window's saved geometry.
`Super+N` collapses the focused window into its clickable node and restores it
again, while automatic decay is a separate conservative timer for genuinely
abandoned work. `Alt+Tab`, `Super+Arrow`, Bearings, Apogee, and Lift are the
five retrieval layers described in [Retrieval](#retrieval). The cluster rows act
only on clusters you declared or created yourself.

The same chord may be assigned distinct actions in `field`, `cluster`, `tile`,
and `stack` scopes. Left/right Super, Alt, Ctrl, and Shift can be matched
independently. Compositor move, resize, and pan grabs are ordinary remappable
bindings rather than hardcoded mouse policy.

`Super+D` opens Halley Lift, the bundled search and action launcher documented
in [`halley-lift/README.md`](halley-lift/README.md). It searches applications,
running nodes, clusters, compositor actions, and config files from one field,
and it can run terminal commands. Prefer a separate launcher? Any non-built-in
action string is a command line, so replacing one line is enough:

```rune
"$var.mod+d" "fuzzel"
```

Existing 0.6-or-newer configurations keep whatever launcher they already bind.
Only a newly generated config defaults to Halley Lift, and routine structural
migration does not rewrite a launcher binding. Migrating an incompatible
pre-0.6 config is the exception: after making a timestamped backup, Halley
replaces that file with the current default config.

---

## Configuration

On first launch Halley creates
`$XDG_CONFIG_HOME/halley/halley.rune`, falling back to
`~/.config/halley/halley.rune`, from the canonical
[`examples/halley.rune`](examples/halley.rune) template. Startup never modifies
an existing config, and configs need no version marker. Optional compatibility
updates are explicit and structurally detected: use `halleyctl config migrate
--dry-run` to inspect them before running `halleyctl config migrate`. Migration
adds only a finite set of known missing bindings or sections to compatible
0.6-or-newer configs, skips conflicting custom chords, validates the complete
candidate, writes atomically, and retains a timestamped backup. An incompatible
pre-0.6 config is instead backed up and replaced with the current default. A
gathered root reports that the file owning the affected section must be migrated
directly rather than guessing where to write.

Pass `-c PATH` or `--config PATH` to select another file. Valid edits reload as
one atomic snapshot; invalid edits leave the last valid runtime state active.
Nested Rune `gather` dependencies are watched recursively, including missing
dependencies that are created after startup.

A freshly generated config declares no startup clusters, so windows begin on the
empty Field. The optional `autostart` section can still declare persistent named
clusters using compact command arrays, including empty `members []`
declarations. See
[startup clusters](docs/clusters.md#startup-clusters) for syntax, launch
attribution, output placement, and restart behavior.

Useful controls:

```sh
halleyctl config verify
halleyctl config edit
halleyctl config migrate --dry-run
halleyctl reload
```

`halleyctl` also exposes output and DPMS state, capture modes, node and cluster
control, Trail, named-monitor focus, stack/tile navigation, Bearings, and portal
diagnostics. Run `halleyctl --help` for the current command surface.

---

## API and External Tools

[`halley-api`](docs/api.md) is the supported Rust boundary for launchers,
panels, automation, tests, and desktop components. It provides typed commands
and queries, structured errors, capability negotiation, persistent clients,
and sequenced state subscriptions.

The postcard-based `halley-ipc` crate is Halley's private transport codec, not
an external compatibility contract. External programs should use
`halley-api`; `halleyctl` and Halley Lift are reference consumers of that API.

---

## Community / Support

Halley's Discord is for practical support, config help, bug triage, packaging,
release updates, and focused contributor coordination. Halley remains
maintainer-directed; Discord is not a roadmap vote.

Join the Discord: https://discord.gg/J2ec3nbHYs

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
- [Managed-window rules](docs/window-rules.md)
- [Wallpaper](docs/wallpaper.md)
- [Screen sharing and the desktop portal](docs/portal.md)
- [Wayland protocol support](docs/wayland-protocols.md)
- [XWayland policy and conformance](docs/xwayland.md)
- [Building, session managers, and packaging](docs/packaging.md)

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
