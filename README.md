# Hally
*Named after Halley's comet — periodic, precise, returning.*

![License](https://img.shields.io/badge/license-GPL--3.0--only-blue?style=flat-square)
![Status](https://img.shields.io/badge/status-active-brightgreen?style=flat-square)
![Wayland](https://img.shields.io/badge/display-Wayland-orange?style=flat-square)
![Build](https://img.shields.io/badge/build-passing-brightgreen?style=flat-square)
![Rust](https://img.shields.io/badge/language-Rust-orange?style=flat-square)

Hally is a Wayland compositor that reimagines desktop workspaces.

> "Hally — Windows as nodes. Windows as clusters. Windows as your command center."

---

## The Comet

Hally's design is inspired by Halley's comet — periodic, precise, and returning. Like a comet with a predictable orbit, Hally brings consistency and elegance to window management.

---

## Concepts

### Field
An infinite 2D canvas, one per monitor. Everything lives here — nodes, cluster cores, empty space. Zoomable and pannable; gesture support is planned.

### Nodes
Windows on the Field. A node is either an open window or a collapsed window. Cluster cores also appear as nodes when collapsed.

### Focus Ring
An invisible eye-shaped region on your monitor that defines your active area. Windows that fall significantly outside it are candidates for decay. The ring can be made briefly visible via a config option — it fades out after a moment. Fully configurable in size and shape.

### Decay
When a node sits outside the focus ring beyond a configurable threshold, it decays — dimming or collapsing to reduce clutter. A small overlap tolerance prevents windows that are just barely outside the ring from decaying unexpectedly. Decay is optional and can be disabled entirely.

### Jump
A keybind-driven action to move a grabbed window across monitors, traversing between Fields.

### Clusters
Hally's answer to workspaces — contained layouts that exist outside the Field. See [Clusters](#clusters) below.

---

## The Field

`hally-core` manages one Field per monitor. The Field is zoomable, pannable, and will support gesture input in a future release. Key properties:

- **Per-monitor** — each display has its own independent infinite canvas
- **Max windows** — configurable cap on open nodes at once
- **Decay** — configurable, opt-in clutter management based on focus ring position
- **Jump** — move grabbed windows between monitors with a keybind

---

## Clusters

Clusters are Hally's answer to workspaces — contained layouts that exist outside the infinite Field. They are not auto-formed; you build them intentionally.

### Creating a cluster

Enter cluster mode to begin selection. Click or mark the windows you want — they join the cluster. Once formed, the cluster collapses into a **core node** on the Field.

### The core

A core node is the handle for a collapsed cluster. When expanded, its windows fan out in a **petal arrangement** — clockwise or counter-clockwise — as icon-sized previews around the core. From there you can:

- Pull windows out of the cluster into the Field
- Bring Field windows in
- Collapse it back into the core

Clicking a core within the focus ring **enters** the cluster — opening its contained workspace layout.

### Inside a cluster

Once inside, you are no longer on the Field. The cluster is its own contained space with one of two layout modes:

- **Tiling** — Weighted tiling. Windows arranged by assigned weight and recency.
- **Stacking** — Windows layered in a navigable stack, similar to a mobile app switcher. Navigate with keybinds, reorder the stack as needed.

> Clusters are not yet implemented. Planned for a future release.

---

## IPC Protocol

Unix socket at `$XDG_RUNTIME_DIR/hally.sock`.

---

## Xwayland

Via `xwayland-satellite` (on-demand, configurable).

---

## Configuration

`crates/hally-config`: keybinds, focus ring, decay threshold, max windows, viewports, autostart.

---

## Testing

Tiling geometry, focus decay, cluster entry/exit, core expand/collapse, stack navigation, jump across monitors.

---

## License

**GPL-3.0-only**

---

## Status

- **Production** — Development environment active
- **Clusters** — Coming soon
