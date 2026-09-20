# Nodes, decay, and the focus ring

Every managed XDG toplevel and normal Xwayland window has one stable node ID
for its lifetime. An active node is a normal window in the compositor space. A
collapsed node remains protocol-alive but is removed from the window/input
space and represented by a compositor-rendered marker at the same field
position.

Collapse preserves visual stack depth. The captured window flies and shrinks
into its final node position at the same layer it occupied: a back window
drops behind every window that was above it, a middle window stays between its
neighbors, and a front window drops in front. The emerging marker shares that
depth instead of jumping to a global node overlay.

Collapse and restore are the deliberate half of the Field loop: `Mod+N`
collapses the focused window on purpose, and one click on the collapsed marker
brings it back. [Automatic decay](#automatic-decay) is the separate conservative
half, and it only reaches work you genuinely left behind.

Click a collapsed marker once to restore and focus its window. This is one
atomic action: Halley does not first center the camera, leave the marker
collapsed, and require a second click. `$var.mod+n` runs the same state toggle
for the focused window.

A client's minimize request is the one-way form of that action: it collapses
the window into its existing node and never restores an already-collapsed
node. Clicking the node (or using `toggle-state`) restores it. XWayland's
`_NET_WM_STATE_HIDDEN` is kept in sync so X11 clients can suspend while
collapsed and resume when restored.

Collapsing the focused window preserves that node as Halley's logical focus,
so node-aware commands such as `close-focused` still target it. The hidden
client's Wayland keyboard focus is cleared before unmapping, so it cannot keep
receiving typed input while collapsed. In hover-focus mode, hovering a marker
makes that node the command target; the default Mod+Q then closes that node.

A plain left press is resolved as a click or grab when the pointer is released
or moves. Releasing before moving 8 screen pixels performs the single-click
restore. Moving at least 8 pixels keeps the window collapsed and grabs its
marker. Mod+left grabs immediately, without restoring even if the button is
released without movement. Both forms preserve the exact point where the
marker was grabbed and can carry it between outputs without centering it under
the pointer.

## Restoration and centering

Centering is optional:

```rune
node:
  click-collapsed-pan "never"
end
```

The accepted values are:

- `never` restores in place and does not move the camera. This is the default.
- `if-offscreen` centers only when the marker is outside the current output.
- `always` centers every restored node.

When centering is selected, camera motion and window restoration begin in the
same action. Moving or restoring a node does not change its stable ID.

## Automatic decay

An unfocused active window becomes a node after its eligibility timer expires:

```rune
decay:
  enabled true
  outside-delay-seconds 600
  inside-delay-seconds 5400
end
```

`outside-delay-seconds` counts from the moment a window becomes ineligible
while it sits outside its output's focus ring; `inside-delay-seconds` counts the
same way for a window still inside the ring. These 10-minute and 90-minute
values are what Halley writes into a **newly generated** configuration, so
genuinely abandoned work survives an ordinary interruption. Decay is not
migrated: an existing configuration keeps the values it states, and one that
omits the `decay:` section keeps Halley's built-in 180-second and
1800-second behavior.

The first automatic collapse explains itself once, in a non-modal notice:
`<Application> was collapsed into a node. Click the node or press Mod+N to
restore it.` The application name is the window title, falling back to the
application id and then to a generic "A window" sentence. The notice never takes
keyboard or pointer input and never changes focus, and it is recorded in user
state (`$XDG_STATE_HOME/halley/state.rune`), so no later collapse — for another
application or in another session — explains anything again. Manual `Mod+N`
collapse is your own deliberate action, is visible as it animates, and never
triggers the notice.

There is no active-window count cap. Focused windows, fullscreen or
fullscreen-pending windows, field-maximized windows, windows in an active Field
arrangement, and windows in an interactive move/resize grab are hard-protected
from decay. Undoing an arrangement starts a fresh timer for each still-eligible
window. Changing between protected, inside-ring, and outside-ring status starts
a fresh timer; stale time from an earlier status is never reused.

Each output has its own camera-centered ellipse. Configure it inside that
connector's `view.output` entry. An entry may contain both hardware settings
and a focus ring, or only a focus ring when no hardware override is wanted:

```rune
view:
  output:
    name "DP-1"
    focus-ring:
      radius-x 820.0
      radius-y 420.0
      offset-x 0.0
      offset-y 0.0
    end
  end

  output:
    name "DP-2"
    focus-ring:
      radius-x 700.0
      radius-y 360.0
      offset-x 0.0
      offset-y 20.0
    end
  end
end
```

A window uses the shorter outside delay only when at least 90% of its footprint
is outside the ellipse for its owning output. Moving it to another output or
changing that output's ring starts a fresh eligibility timer. Editing one
output does not reset timers on the others. An output without a configured
ring uses the built-in 820×420 default. Top-level `focus-ring:` and `output:`
blocks are rejected; both belong under `view:`.

The ring is normally hidden. Saving a changed focus-ring configuration previews
it briefly; `debug.show-focus-ring true` keeps it visible:

```rune
debug:
  show-focus-ring false
  # Keeps unlocked outputs repainting continuously while enabled.
  overlay-fps false
end
```

## Appearance

```rune
font:
  family "monospace"
  size 11
end

node:
  show-labels "hover"
  show-app-icons "always"
  shape "squircle"
  label-shape "squircle"
  icon-size 0.72
  opacity 1.0
  background-colour "auto"
  border-colour "#474d59"
  border-colour-highlighted "#d65d26"
end
```

Shapes accept `square` or `squircle`. `shape` and `label-shape` are the only
supported keys; the redundant `node-shape` and `node-label-shape` spellings
were removed. Labels use dedicated rectangle shaders and the shared Cosmic
Text renderer, including configured font family/weight suffixes, measured
centering, contrast-aware text, edge flipping, and the old hover
slide/grow/fade. See [Fonts](fonts.md) for global typography behavior.

Collapsed nodes and cluster cores own their colours independently of window
decorations. `border-colour` controls the idle ring and icon (`#474d59` by
default); `border-colour-highlighted` controls hover, logical focus, join-ready
feedback, and the highlighted core icon (`#d65d26` by default). American
`color` spellings are accepted. `background-colour` accepts `auto`, `system`,
`light`, `dark`, or a hex RGB colour. `auto` is Halley's deterministic local
palette. `system` explicitly follows the XDG Settings portal appearance
preference live and falls back to `auto` when no preference is available.

Display policies accept `off`, `hover`, or `always`. Real application icons
are resolved from desktop entries and icon themes in a background worker.
The marker stays blank while an icon is loading or unavailable, so a cold
first collapse never flashes a temporary letter before the real icon appears.

With `show-labels "hover"`, only an explicit pointer hover reveals the label;
logical or keyboard focus still highlights the marker but does not open its
label. The old-Halley back-loaded slide/grow/fade appears first, then 1500 ms
of uninterrupted hover replaces it with a live, aspect-fitted window preview.
Leaving the marker, changing hover targets, pressing it, or beginning a node
grab cancels the dwell and closes the hover UI. A grab keeps labels and
previews suppressed until later pointer motion deliberately targets a node
again.

## Active cluster drop admission

While a cluster workspace is open on an output, Mod+left-drag an ordinary Field
window or a collapsed node into that output's work area and release it to add it
to the open cluster. The drop may cross outputs. A collapsed node is restored
before admission, then enters the cluster's current tiling or stacking layout
using the same insertion and reflow behavior as an ordinary window.

## Cluster bloom joining

Rest the pointer on a collapsed cluster core to open its member bloom. While
the bloom is open, its core is temporarily fixed in place. Mod+left-drag a
normal Field window against the core: the window docks at the same non-overlap
distance used by `field.gap` instead of pushing the core away.

Hold the window there for `clusters.join-dwell-ms`. When the dwell completes,
the core's original border changes to `border-colour-highlighted` and thickens
to five
pixels without changing its fill or icon. A light wash of that same colour
marks the dragged window; releasing then adds the window to that cluster.
Moving away, closing the bloom, changing outputs, cancelling the grab, or
releasing before the affordance appears cancels the join. Closed and closing
blooms never accept windows.

Clicking or grabbing another window leaves the bloom open, matching old
Halley. A plain click on the empty Field, clicking or dragging the bloomed core,
activating a cluster, or an explicit keyboard action closes it. Typing into a
different focused window after its drag has ended also closes the abandoned
bloom without changing that window's focus.

The legacy `clusters.join-distance-px` key remains parseable so existing
configurations continue to load, but it no longer affects this interaction.
Contact is determined from the actual window and core bounds plus `field.gap`.

## Landmark non-overlap

Collapsed nodes are the only landmarks. Active windows may overlap other
active windows freely, but nodes remain clear of both nodes and active
windows:

```rune
field:
  gap 20.0
end

placement:
  landmarks:
    strategy "nearest-free"
    normal-blocker "relocate"
  end
end
```

Collapse starts at the window center and slides to the nearest legal location.
A new or restored active window keeps its placement and relocates blocking
nodes. An interactively dragged window is authoritative and pushes unpinned
nodes; `halleyctl node move` remains a discrete legal-placement operation.
Marker collision is screen-constant across camera zoom. As zoom-out grows a
marker's footprint in Field space, unpinned ordinary nodes and collapsed
cluster cores reflow together around each other and stationary active windows.
Transient labels and shadows never reserve space.

That zoom reflow is reversible. The first time a zoom step moves a landmark,
Halley remembers the position it left behind as its pre-zoom home. Further
zoom-out keeps reflowing from where the marker is displayed without replacing
that home, and zooming back in returns the landmark toward it as the shrinking
footprint makes room, sliding from the position actually on screen. Active
windows and pinned landmarks stay where they are, so a landmark whose home is
still blocked waits at the closest legal point and finishes the trip on a later
zoom step.

Direct manipulation is permanent. Dragging a displaced landmark discards its
home immediately, and every landmark a drag physically pushes in the collision
chain discards its home too, so nothing snaps back after a push. Pinning a
landmark, transferring it to another monitor, moving it with `halleyctl node
move`, collapsing or restoring its window, and ordinary placement reflow all
commit the displayed position the same way. Zoom memory is per monitor and
lasts only for the running session.

The same `field.gap` insets field-maximized windows from the usable output
work area. See [Field behavior and maximize](field.md).

## Rigid and physics movement

```rune
physics:
  enabled true
  damping 0.45
end
```

With physics disabled, a dragged window transfers displacement directly: after
contact, moving the window one field unit moves the contacted node or movable
node chain one field unit along the contact normal. There is no slide animation
on interactive displacement.

With physics enabled, grabbed windows and nodes are kinematic authorities and
impart bounded old-Halley momentum to the objects they contact. Pushed objects
use frame-rate-independent damping and continue settling after release; the
grabbed object itself does not fling. A grabbed node slides around an active
window in rigid mode and can bump that window in physics mode. Active windows
still never collide with other active windows. Pinned nodes remain fixed.

Pointer reports only update the latest drag target and sampled authority
velocity. Physics advances once per rendered frame using real elapsed time, so
high-polling mice do not multiply damping. Releasing an active window flushes
its final target and holds that window fixed for 350 ms while displaced nodes
settle, matching old Halley's drop behavior.

## `halleyctl node`

The original old-Halley command surface remains intact, with explicit
collapse, restore, and toggle controls added for complete remote state control:

```text
halleyctl node list [--output OUTPUT] [--json]
halleyctl node info [SELECTOR] [--output OUTPUT] [--json]
halleyctl node focus [SELECTOR] [--output OUTPUT]
halleyctl node move left|right|up|down [SELECTOR] [--output OUTPUT]
halleyctl node collapse [SELECTOR] [--output OUTPUT]
halleyctl node restore [SELECTOR] [--output OUTPUT]
halleyctl node toggle [SELECTOR] [--output OUTPUT]
halleyctl node close [SELECTOR] [--output OUTPUT]
```

Selectors are `focused`, `latest`, a bare numeric ID, `id:NUMBER`,
`title:TEXT`, and `app:TEXT`.
Title and app matching are case-insensitive substrings and return an error when
ambiguous. With no selector, commands use the focused node and otherwise fall
back to the latest node. `--output` validates and limits selection to one
connector.

`list` groups nodes by output and marks the focused node with `*`, the latest
node with `+`, and other nodes with `-`. Each text entry includes its state,
application ID, role, protocol family, modal/parent relationships, child-popup
count, focus/latest flags, field position, and size. `info` prints the same
fields for one node. This makes the default output useful for beginners while
leaving `--json` stable for scripts.

`focus` restores a collapsed node or focuses an active one. `move` requests an
80-field-unit shift and then resolves the nearest legal landmark/window
destination; pinned nodes reject that movement request. `collapse` and
`restore` explicitly set the selected node state
and are idempotent; `toggle` inverts it. `close` sends the appropriate XDG or
X11 close request without restoring first. `--json` is available for `list`
and `info`.

The same movement is available directly from the compositor with
`node-move left|right|up|down`. The default `Mod+Alt+Arrow` bindings target the
focused or most-recent node on the selected output and operate only in the
Field; cluster workspaces retain their own focus and swap controls.

`halleyctl` implements these controls through the versioned [`halley-api`
contract](api.md). External tools can use the same typed node operations and
subscriptions without depending on postcard wire layout.

Offscreen active windows and collapsed nodes are also available through
[Bearings](bearings.md), including the old
`halleyctl bearings show|hide|toggle|status` controls.

Pinning is controlled by the default `Mod+P` binding and documented with its
configuration and cluster-core boundary in [Field behavior](field.md#pinning).

## Lifecycle boundaries

Null-buffer unmaps, remaps, metadata commits, activation, fullscreen requests,
and destruction remain valid while a toplevel is collapsed. A client does not
need to be mapped in the render space for Halley to process those protocol
events. Layer-shell surfaces, popups, and X11 override-redirect windows are not
nodes.
