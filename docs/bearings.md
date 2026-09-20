# Bearings

Bearings is the offscreen layer of Halley's retrieval. Directional `Mod+Arrow`
focus moves one step at a time, `Alt+Tab` recalls recent work, Bearings retrieves
the spatial work that left this monitor's viewport, `Mod+O` Apogee is the visual
inventory across monitors, and Halley Lift searches directly by application,
node, cluster, or action. All five stay available; only the question they answer
differs.

Bearings makes every offscreen node reachable without turning navigation into
a center-then-activate sequence. It is computed independently for each output
from that output's live camera viewport.

The default bindings are:

- Hold `Mod+Z` to show Bearings until the `Z` key is released. Releasing Mod
  first does not leave the overlay stuck open.
- Press `Mod+Shift+Z` to toggle Bearings persistently.

Each offscreen active window or collapsed node gets a title chip on the nearest
of eight edge lanes. Chips retain the node's projected edge position. Crowded
neighbors are grouped, and a forward/backward lane pass prevents chips from
overlapping. This affects Bearings UI only: active windows remain free to
overlap one another, while the separate landmark policy continues to keep
collapsed nodes clear.

Bearings stay hidden on an output while immersive fullscreen owns it, including
games, browser video, and compositor `Mod+F`. Chips return when that
presentation is parked (for example by Alt-Tab) or left. Field-maximize does
not hide them.

Clicking a chip performs one action:

- A collapsed node restores and focuses immediately. Optional
  `node.click-collapsed-pan` movement begins in that same action.
- An active window focuses and raises immediately. If it is offscreen, the
  camera moves only far enough to reveal its bounds rather than centering it.

## Configuration

```rune
bearings:
  show-distance true
  show-icons true
  show-pinned true
  fade-distance 1200.0
  blur true
end
```

`show-distance` adds the distance beyond the current viewport edge.
`show-icons` reserves a 16-pixel application-icon slot for nodes with an app
ID. Icon lookup is asynchronous; the slot remains blank until the real icon is
ready and never flashes a temporary letter. `show-pinned` keeps pinned field
nodes and pinned collapsed-cluster cores at full overlay opacity instead of
applying distance fade. Their chips reserve space for and display the same
original-Halley pin badge used on the field. Cluster members are never
pinnable. `fade-distance` controls
the old distance fade and is clamped from 120 through 100000 field pixels.

Labels and distance text use the shared Cosmic Text renderer and the global
`font:` section. Titles are Unicode-safe and shorten after 24 characters.
Crowded groups use a count such as `3 nodes`. Chip shape, palette, and borders
come from the shared [`overlays:`](overlays.md) section; `bearings.blur`
continues to control the Bearings-only backdrop effect.

With `blur true`, all chips on one output share a single persistent backdrop
capture and Dual Kawase blur chain. The blurred result is recomputed when the
backdrop changes and then reused for every chip; Bearings does not run one
full-frame capture or blur chain per label.

## `halleyctl bearings`

The old remote controls are available unchanged:

```text
halleyctl bearings show
halleyctl bearings hide
halleyctl bearings toggle
halleyctl bearings status
```

`status` prints `visible` or `hidden`. These commands use the versioned
[`halley-api` contract](api.md), including its connection handshake and
structured failures.
