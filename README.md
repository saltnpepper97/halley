<h1 align="center">Halley</h1>
*Named after Halley's comet — periodic, precise, returning.*

![License](https://img.shields.io/badge/license-GPL--3.0--only-blueviolet?style=for-the-badge)
![Status](https://img.shields.io/badge/status-active-brightgreen?style=for-the-badge)
![Wayland](https://img.shields.io/badge/display-Wayland-blue?style=for-the-badge)
![Build](https://img.shields.io/badge/build-passing-success?style=for-the-badge)
![Rust](https://img.shields.io/badge/language-Rust-orange?style=for-the-badge)

---

> **Windows as nodes. Windows as clusters. Windows as your command center.**

Halley is a Wayland compositor built from the ground up for multi-monitor setups. Each display gets its own independent infinite canvas. Windows live as nodes on those canvases, group into clusters you build intentionally, and decay gracefully when they drift out of focus. Inspired by the comet it's named after — periodic, precise, and always returning — Halley makes multi-monitor work feel deliberate rather than chaotic.

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
| **IPC** | Unix socket control at `$XDG_RUNTIME_DIR/halley/hally.sock` |
| **Xwayland** | On-demand support via `xwayland-satellite` |

---

## Default Keybinds

Defaults follow `examples/halley.rune`.

| Category | Keybind | Action |
|---|---|---|
| Basic | `Super+Shift+r` | Reload config |
| Basic | `Super+n` | Toggle state |
| Basic | `Super+q` | Close focused window |
| Quit | `Super+Shift+e` | Quit Halley |
| Zoom | `Super+MouseWheelUp` | Zoom in |
| Zoom | `Super+MouseWheelDown` | Zoom out |
| Zoom | `Super+MiddleMouse` | Reset zoom |
| Move | `Super+h` | Move node left |
| Move | `Super+l` | Move node right |
| Move | `Super+k` | Move node up |
| Move | `Super+j` | Move node down |
| Monitor | `Super+Shift+h` | Focus monitor left |
| Monitor | `Super+Shift+l` | Focus monitor right |
| Clusters | `Super+Shift+c` | Enter cluster mode |
| Bearings | `Super+z` | Show bearings |
| Bearings | `Super+Shift+z` | Toggle bearings |
| Trail | `Super+Shift+,` | Trail previous |
| Trail | `Super+Shift+.` | Trail next |
| Launch | `Super+Return` | Launch `kitty` |
| Launch | `Super+d` | Launch `fuzzel` |
| Pointer | `Super+LeftMouse` | Move window |
| Pointer | `Super+RightMouse` | Resize window |
| Pointer | `Super+Shift+LeftMouse` | Field jump |
| Media | `XF86AudioRaiseVolume` | Raise volume |
| Media | `XF86AudioLowerVolume` | Lower volume |
| Media | `XF86AudioMute` | Toggle mute |

---

## Configuration

Handled by `crates/hally-config`. Covers keybinds, focus ring shape and size, decay threshold, max windows per Field, viewports, autostart programs and much **more**.

---

## Testing

Tiling geometry, focus decay, cluster entry/exit, core expand/collapse, stack navigation, jump across monitors.

---

## License

**GPL-3.0-only**

