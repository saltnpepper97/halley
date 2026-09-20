# Rune keybind trigger reference

Halley keybinds are entries in the `keybinds:` section of
`~/.config/halley/halley.rune`:

```rune
keybinds:
  mod "super"

  "Print" "grim ~/Pictures/screenshot.png"
  "$var.mod+Page_Up" "some-command"
  "$var.mod+click-forward" "some-other-command"
  "$var.mod+scroll-up" "zoom-in"
end
```

The last `+`-separated part is the trigger. Every earlier part is a modifier.
Trigger and modifier names are case-insensitive. Modifiers are matched exactly:
a bind without `shift`, for example, will not fire while Shift is held.

## Modifiers

| Rune name | Meaning |
|---|---|
| `$var.mod` | The key selected by `mod`, including generic and left/right forms |
| `super`, `logo`, `mod4` | Super/Windows/Command-style logo modifier |
| `lsuper`, `rsuper` | Left or right Super only |
| `alt` | Alt |
| `lalt`, `ralt` | Left or right Alt only |
| `ctrl`, `control` | Control |
| `lctrl`, `rctrl` | Left or right Control only |
| `shift` | Shift |
| `lshift`, `rshift` | Left or right Shift only |

The configured `$var.mod` is remapped to Alt while running nested under
Winit, so development keybinds do not steal the host compositor's Super key.
Its physical side is preserved: `lsuper` becomes `lalt`, for example.

## Context scopes

Bindings are global unless their compositor action has a natural presentation
scope. Field movement and resize actions are Field-scoped, cluster layout
actions are Cluster-scoped, and tile focus/swap actions are Tile-scoped. This
allows the same chord to be declared more than once:

```rune
"$var.mod+ctrl+left" "resize-window-left"
"$var.mod+ctrl+left" "cluster-tile-swap-left"
```

Use `with scope "global|field|cluster|tile|stack"` to override an action's
default scope for a custom layout. Global bindings remain available in every
context; Field bindings are inactive while a cluster workspace owns that
monitor.

## Keyboard triggers

Halley accepts every keysym name recognized by the system XKB library, not
only the names in these tables. The spelling shown below is the conventional
XKB spelling; matching is case-insensitive. This rule also covers uncommon
international, vendor, and legacy keysyms without requiring Halley to carry a
stale copy of XKB's thousands-entry catalog.

### Printable keys

| Physical label or group | Rune trigger |
|---|---|
| Letters | `a` through `z` |
| Number row | `0` through `9` |
| Space | `space` |
| Backtick, tilde | `grave`, `asciitilde` |
| Exclamation, at, number sign | `exclam`, `at`, `numbersign` |
| Dollar, percent, caret | `dollar`, `percent`, `asciicircum` |
| Ampersand, asterisk | `ampersand`, `asterisk` |
| Parentheses | `parenleft`, `parenright` |
| Minus, underscore | `minus`, `underscore` |
| Equals, plus | `equal`, `plus` |
| Square brackets | `bracketleft`, `bracketright` |
| Braces | `braceleft`, `braceright` |
| Backslash, pipe | `backslash`, `bar` |
| Semicolon, colon | `semicolon`, `colon` |
| Apostrophe, quotation mark | `apostrophe`, `quotedbl` |
| Comma, less-than | `comma`, `less` |
| Period, greater-than | `period`, `greater` |
| Slash, question mark | `slash`, `question` |

Shifted symbols still require `shift` in the chord because modifiers match
exactly. For example, use `"shift+plus"` for the `+` symbol. To bind the
physical equals key regardless of layout or produced symbol, use its
`keycode-N` form instead.

### Navigation and editing

| Physical key | Rune trigger |
|---|---|
| Escape | `Escape` |
| Tab | `Tab` |
| Shift+Tab symbol | `ISO_Left_Tab` |
| Enter | `Return` |
| Backspace | `BackSpace` |
| Insert | `Insert` |
| Delete | `Delete` |
| Home | `Home` |
| End | `End` |
| Page Up | `Page_Up` |
| Page Down | `Page_Down` |
| Arrow up | `Up` |
| Arrow down | `Down` |
| Arrow left | `Left` |
| Arrow right | `Right` |
| Menu/application | `Menu` |
| Help | `Help` |
| Undo, redo | `Undo`, `Redo` |
| Find, cancel | `Find`, `Cancel` |

### Function, system, and lock keys

| Physical key or group | Rune trigger |
|---|---|
| Function keys | `F1` through `F35` |
| Print Screen | `Print` |
| System Request | `Sys_Req` |
| Pause | `Pause` |
| Break | `Break` |
| Caps Lock | `Caps_Lock` |
| Num Lock | `Num_Lock` |
| Scroll Lock | `Scroll_Lock` |
| Clear | `Clear` |

### Modifier keys as triggers

| Physical key | Rune trigger |
|---|---|
| Left/right Shift | `Shift_L`, `Shift_R` |
| Left/right Control | `Control_L`, `Control_R` |
| Left/right Alt | `Alt_L`, `Alt_R` |
| Left/right Super | `Super_L`, `Super_R` |
| AltGr | `ISO_Level3_Shift` |

For Shift, Control, Alt, and Super, the modifier contributed by the trigger
itself is ignored for exact-modifier matching, so a bare `"Shift_L"` binding
works as expected. Other held modifiers still have to appear in the chord.

### Numeric keypad

| Physical key or group | Rune trigger |
|---|---|
| Keypad digits | `KP_0` through `KP_9` |
| Keypad decimal/separator | `KP_Decimal`, `KP_Separator` |
| Keypad add/subtract | `KP_Add`, `KP_Subtract` |
| Keypad multiply/divide | `KP_Multiply`, `KP_Divide` |
| Keypad equals | `KP_Equal` |
| Keypad enter | `KP_Enter` |
| Keypad navigation | `KP_Home`, `KP_End`, `KP_Left`, `KP_Right`, `KP_Up`, `KP_Down` |
| Keypad page keys | `KP_Page_Up`, `KP_Page_Down` |
| Keypad insert/delete | `KP_Insert`, `KP_Delete` |

### Common media and hardware keys

| Physical key or group | Rune trigger |
|---|---|
| Mute, volume down/up | `XF86AudioMute`, `XF86AudioLowerVolume`, `XF86AudioRaiseVolume` |
| Play, pause, stop | `XF86AudioPlay`, `XF86AudioPause`, `XF86AudioStop` |
| Previous/next track | `XF86AudioPrev`, `XF86AudioNext` |
| Microphone mute | `XF86AudioMicMute` |
| Brightness down/up | `XF86MonBrightnessDown`, `XF86MonBrightnessUp` |
| Keyboard brightness down/up | `XF86KbdBrightnessDown`, `XF86KbdBrightnessUp` |
| Power, sleep, wake | `XF86PowerOff`, `XF86Sleep`, `XF86WakeUp` |
| Display switch | `XF86Display` |
| Calculator, mail, browser home | `XF86Calculator`, `XF86Mail`, `XF86HomePage` |
| Search | `XF86Search` |
| Wi-Fi, Bluetooth | `XF86WLAN`, `XF86Bluetooth` |
| Touchpad toggle/on/off buttons | `XF86TouchpadToggle`, `XF86TouchpadOn`, `XF86TouchpadOff` |

These are keyboard buttons. Touchpad gestures use the action maps under
`input.gestures`: `swipe-DIRECTION-FINGERS`,
`apogee-swipe-DIRECTION-FINGERS`, and `hold-FINGERS`. Their values accept the
same built-in compositor actions as keybinds; arbitrary shell commands and
pointer-grab actions are rejected. `pan-fingers` remains reserved for
continuous camera panning.

## Mouse buttons and wheel

| Input | Rune trigger | Linux evdev code matched |
|---|---|---|
| Left button | `click-left` | 272 (`BTN_LEFT`) |
| Right button | `click-right` | 273 (`BTN_RIGHT`) |
| Middle button | `click-middle` | 274 (`BTN_MIDDLE`) |
| Back/side button | `click-back` | 275 or 278 (`BTN_SIDE`/`BTN_BACK`) |
| Forward/extra button | `click-forward` | 276 or 277 (`BTN_EXTRA`/`BTN_FORWARD`) |
| Wheel up | `scroll-up` | Physical wheel vertical negative |
| Wheel down | `scroll-down` | Physical wheel vertical positive |
| Wheel left | `scroll-left` | Physical wheel horizontal negative |
| Wheel right | `scroll-right` | Physical wheel horizontal positive |

Window movement, window resizing, and Field panning are remappable compositor
actions. The defaults are `"$var.mod+click-left" "move-window"`,
`"$var.mod+click-right" "resize-window"`,
`"$var.mod+shift+click-left" "drag-pan"`, and
`"click-left" "pan-field"`. Remove a binding to disable that grab or assign
the action to another pointer chord. `move-window` is contextual: the same
grab pans when it starts on empty Field, so both bare left-drag and Mod+left-drag
pan the desktop.

`drag-pan` begins only on a fully visible Field window. It keeps the grabbed
window assigned to its current output, clamps it at that output's edge, and
after a short dwell pans that output's camera while carrying the window. Pulling
the pointer back from the edge stops the pan. Fullscreen, maximized, oversized,
and active-cluster windows do not enter this mode. The legacy action names
`field-jump` and `field_jump` are accepted as aliases for `drag-pan`.

An unbound click keeps its ordinary client, focus, decoration, and
collapsed-node behavior. In an active tiling cluster,
dragging a tile temporarily lifts it, reorders it live as the pointer crosses
another tile, and smoothly returns it to the selected slot on release.

Wheel binds apply only to a physical mouse wheel. High-resolution wheel input
is accumulated to one action per complete notch. Touchpad/finger scrolling
continues to the focused client and is not treated as a keybind.

## Key repeat

Keyboard bindings for continuous built-in actions repeat by default: focus
cycling and directional focus, Field node movement and resize, cluster tile focus and
swapping, monitor focus, and zoom in/out. Destructive, modal, toggle, reset,
screenshot, terminal, and arbitrary command actions are one-shot by default.

Both defaults can be overridden with Rune's inline `with` attributes:

```rune
"$var.mod+left" "focus-left" with repeat false
"XF86AudioRaiseVolume" "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+" with repeat true
```

Repeat fires once immediately, waits for `input.repeat-delay`, then follows
`input.repeat-rate`; rate `0` disables client and compositor key repeat.
Pointer-button and wheel bindings cannot use `repeat true`.

## Raw Linux input codes

Raw codes cover hardware that has no useful XKB name:

| Form | Meaning | Example |
|---|---|---|
| `keycode-N` | Exact decimal Linux evdev keyboard code | `"keycode-99" "some-command"` |
| `button-N` | Exact decimal Linux evdev mouse-button code | `"$var.mod+button-279" "some-command"` |

Use the Linux evdev number as reported by tools such as `wev` or
`libinput debug-events`. Do not add XKB's internal offset; Halley adds it when
resolving `keycode-N`.

## Actions

The built-in action strings are `quit`, `close-focused`, `toggle-fullscreen`,
`maximize-focused`, `toggle-state`, `apogee`, `bearings-show`,
`bearings-toggle`, `cycle-focus`, `cycle-focus-backward`, `open-terminal`,
`center-last-focused`, `arrange-visible`, `undo-arrange`, `reload`, `zoom-in`,
`zoom-out`, `zoom-reset`, `screenshot`, and `cluster-toggle-float`. Parameterized actions also include
`focus-DIRECTION`, `cluster-focus-DIRECTION`, `cluster-tile-swap-DIRECTION`,
`node-move DIRECTION`, `resize-window-DIRECTION`, and `monitor-focus DIRECTION`, where `DIRECTION` is
`left`, `right`, `up`, or `down`. The default `Mod+Arrow` `focus-DIRECTION`
walks spatially among expanded windows, collapsed nodes, and collapsed cluster
cores on the active output. It pans each target minimally into view without
opening node or core handles; a collapsed node reserves its full restored
decorated-window bounds so opening it afterward remains completely visible.
While a `Mod+A` arrangement is active on that output, the same keybind still
moves focus but does not pan the camera.
`node-move` shifts the focused or most-recent Field window/collapsed node by one
legal placement step; the default binding is
`Mod+Alt+Arrow`. Field `resize-window` uses `left`/`up` to
shrink and `right`/`down` to grow, sharing `Mod+Ctrl+Arrow` with scoped tile
swapping. `monitor focus DP-1` targets an exact connector name. `Alt+Tab` and
`Alt+Shift+Tab` open and navigate the focus carousel; releasing Alt commits,
focuses and raises the selected window, and moves the pointer to its final
presentation center. A collapsed target restores first and receives one
pointer warp after its opening transition. Escape cancels without changing
focus or pointer position. `Mod+O` opens or closes
the multi-monitor Apogee overview. Apogee stops trapping keys as soon as its
close transition begins.

The five retrieval layers are meant to be used together, and they are described
in the same order everywhere: `Mod+Arrow` walks to the nearest window or node
for nearby spatial navigation, `Alt+Tab` navigates recent work, Bearings
retrieves offscreen spatial work, `Mod+O` Apogee is the visual inventory across
monitors, and `Mod+D` Lift searches directly by application, node, cluster, or
action. None of them replaces another.

The default `Mod+A` `arrange-visible` action reorganizes ordinary Field
windows whose centers are inside the active output's current visible work area.
It excludes collapsed or detached windows, cluster members, pinned windows,
fullscreen and maximized windows, and clients whose size constraints cannot fit
the resulting mosaic. Two windows use equal halves; three use one large region
and two smaller regions; four use a 2×2 grid; larger sets use balanced,
near-square rows. Halley assigns windows to regions by minimum total travel, so
their approximate spatial order is preserved.

Arrangement is reversible cleanup rather than a layout mode: it creates no
tiling tree, no relationship, and no persistent layout state, and every
resulting window remains independently movable and resizable. While its restore
transaction is active, its windows are protected from automatic decay. Pressing
`Mod+A` again restores the exact geometry/output
snapshot captured by that output's arrangement. The restore transaction is
recorded before clients are configured, so an immediate or mid-animation second
press reverses reliably without waiting for clients to commit. `undo-arrange` remains
available as an unbound compatibility action for custom configurations.

`default-terminal` (also accepted as `open-terminal`) launches the first
available built-in terminal in this order:
`alacritty`, `kitty`, `ghostty`, `wezterm`, `foot`, `footclient`, `rio`,
`contour`, `kgx`, `gnome-terminal`, `konsole`, `xfce4-terminal`, `tilix`,
`terminator`, `mate-terminal`, `qterminal`, `lxterminal`, then `xterm`.
To choose an exact terminal instead, bind its command directly—for example,
`"$var.mod+t" "kitty"`.
The default `$var.mod+d` binding launches `halley-lift`, Halley's bundled search
and action launcher. Lift searches applications, running nodes, clusters,
compositor actions, and config files in one field, and its `term` mode runs a
terminal command line. Any other launcher works the same way, because a
non-built-in action string is a command line: replace the binding with
`"$var.mod+d" "fuzzel"` to use Fuzzel instead. Freshly generated configs ship
the Lift binding and keep Fuzzel as a comment. Existing 0.6-or-newer configs keep
their own launcher binding during routine structural migration. The explicit
migration of an incompatible pre-0.6 config instead backs it up and replaces it
with the current default config.
The interactive screenshot menu and its area, screen, and window selectors
force the compositor cursor visible even if a client or inactivity policy had
hidden it.
A freshly generated configuration's first native session also shows the one-time
basics card: the Field-first mental model and only the five essential chords
(`Mod+D` Lift, `Mod+Left-drag` move, `Mod+A` arrange, `Mod+N` collapse/restore,
`Mod+O` Apogee). It appears on that first native session only, captures just
`Enter`, `Escape`, and the first pointer press or touch, and records the
dismissal so it never reappears automatically. Reopen it by hand from Lift's
**Show Halley basics** action or `halleyctl basics`. See
[Compositor overlays](overlays.md) for the card itself and the user-state file
that remembers the dismissal.
`quit` opens Halley's modal exit confirmation instead of stopping the
compositor immediately. Enter confirms and Escape cancels while preserving
the focused client beneath it. Its appearance is configured in
[Compositor overlays](overlays.md).
See [Apogee and Alt+Tab](apogee.md) for navigation, gestures, and preview
performance.
Holding the default `Mod+Z` `bearings-show` binding exposes offscreen nodes and
hides them on the initiating key's release, even if Mod was released first.
`Mod+Shift+Z` toggles the same UI persistently. See
[Bearings](bearings.md) for layout, click behavior, configuration, and
`halleyctl` controls.

The default `Mod+Arrow` bindings focus the nearest window in that direction in
the Field and the nearest tile in a tiling cluster. In a stacking cluster,
`Mod+Left` cycles forward and `Mod+Right` cycles backward; `Alt+Tab` and
`Alt+Shift+Tab` use that same stack cycle. The vertical arrows are inert there,
and cycling a stack with fewer than two windows is a handled no-op. The explicit
`cluster-focus-DIRECTION` aliases remain available for cluster-only bindings.
`Mod+H` focuses the output's most recently focused Field window and pans the
camera to its center without changing client geometry or restoring a collapsed
window. It is inert while fullscreen, maximize, or an open cluster owns the
output camera.
`Mod+Ctrl+Arrow` swaps the focused tile with its directional neighbour.
The default `Mod+V` `cluster-toggle-float` binding moves the focused member of
an active cluster between its layout and a cluster-owned floating layer. The
remaining tiles or cards reflow without changing membership order. A floating
member stays above the layout and supports the normal Mod+left move and
Mod+right resize gestures. Dropping it inside the cluster workspace keeps it as
a cluster float. Dropping it outside that workspace, including on another
output, leaves the cluster and the window becomes an ordinary Field window.
Its output, geometry, floating state, and membership survive closing and
reopening that cluster and are forgotten only when the member leaves or the
cluster is dissolved. Raising the member, including through `Alt+Tab`, can place it above
a fullscreen or maximized window using the normal window stack. The action is
inert outside an active cluster and while the member is fullscreen, maximized,
or already held by a compositor grab.
`Mod+Shift+Arrow` works in either the Field or a cluster: it selects the adjacent
monitor in that direction and focuses that monitor's most recently focused
window. Monitor selection follows configured output geometry, so non-row
layouts work without connector-specific bindings.
`toggle-state` collapses the focused window to a node or restores
it in one action; its default binding is `$var.mod+n`. Optional
`node.click-collapsed-pan` camera movement begins in that same action and never
creates a center-first second-click interaction. Any other action string is
launched as a command line.

The default `$var.mod+m` binding runs `maximize-focused`. It is the reversible
field maximize state documented in [Field behavior and maximize](field.md);
it keeps panels and the existing stack visible. The compatibility action
names `maximize_focused`, `toggle-maximize`, and `toggle_maximize` are
equivalent.

Like a conventional desktop, a client decoration's maximize button and a
double-click on its titlebar both toggle Halley's field maximize. The client is
configured with the standard `Maximized` state. `toggle-fullscreen`, explicit
client fullscreen requests, and initial fullscreen hints remain the separate
fullscreen path and are configured with `Fullscreen`.
Top layer-shell
surfaces are suppressed per fullscreen output, independent of pointer or
keyboard focus on another monitor. Fullscreen also owns that output's complete
camera: zoom and pan ease to the window center and native 1.0 scale on the same
transition, every window stacked above it uses that camera, and camera input
remains locked until the pre-fullscreen monitor view has been restored.
Global window blur configured as `effects.blur.windows "always"` remains active
for `Mod+F` presentations. Client-requested fullscreen
does not gain global blur, preserving its immersive composition fast path;
an explicit client background-effect request is still honored.
For X11 windows, entering or leaving fullscreen preserves the window's current
stack slot; a window already above the fullscreen target stays above until the
user explicitly raises the fullscreen window.

Inside an active cluster workspace, field maximize and fullscreen temporarily
promote only the selected member above the desktop and cover every sibling,
floating window, node, and cluster overlay behind it. On exit, the window eases
back to its current tile and rejoins the cluster at its original stack slot.

## Keyboard Field panning and monitor transfer

`window-transfer left|right|up|down` sends the selected Field window to the
adjacent physical monitor and follows it. Defaults are `Super+Alt+Shift+Arrow`.
The destination is centered in that monitor's current Field view; window size
and collapsed state are preserved. Both monitors must show the Field. Cluster
members, fullscreen and maximized windows are excluded; restore those first.
If no adjacent monitor exists, nothing moves. The selected monitor, rather
than the pointer position, determines which Field is used.

`pan-field left|right|up|down` pans the selected monitor's Field by ten percent
of the current view width or height per activation. Right reveals space to the
right, and down reveals space below. Held keyboard bindings repeat using the
configured keyboard repeat settings; camera easing follows the queued target.
Window geometry and keyboard focus do not change. Fullscreen, maximize, and
active cluster views block Field panning.

Panning actions intentionally have no default bindings and are not added by
migration. Assign them to your preferred chords; existing default bindings are
preserved. Bare `pan-field` remains the pointer-drag action.

Scripting uses the same operations and returns an error when unavailable:

```sh
halleyctl monitor transfer right
halleyctl pan left
```

The client API exposes `transfer_window(direction)` and `pan_field(direction)`.
Transfer bindings are included in new configs; existing configs can receive
unoccupied bindings through `halleyctl config migrate`.

Keyboard actions and binding scopes follow the selected monitor even in hover
focus mode. Successful window transfers move the pointer to the transferred
window at the destination view center. Warps do not synthesize hover focus;
physical mouse movement resumes normal hover selection. Transfers are blocked
while a pointer grab, lock, or confinement is active. Failed transfers do not
move the pointer. Keyboard panning alone never warps the pointer.
