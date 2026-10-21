# Apogee and Alt+Tab

Apogee is the inventory layer of Halley's retrieval: `Mod+Arrow` moves one step
in the Field, `Alt+Tab` recalls recent work, Bearings retrieves offscreen spatial
work within a monitor, Apogee shows what is open across every monitor, and
Halley Lift searches by application, node, cluster, or action. Every layer stays
available, and they are meant to be used together.

`Mod+O` opens Apogee across every configured output. Each monitor lays out its
own attached windows and collapsed nodes, preserves their aspect ratios, and
animates them directly from their desktop presentation into the mosaic. The
old-Halley spatial packer keeps field-space reading order, weights slots by
window area, prevents tile overlap, and reserves the original upper core band.
`max-rows` is limited to the old supported range of 1 through 5.

Apogee replaces the desktop scene while it is visible. The only scene behind
its tiles is the wallpaper layer: normal windows, nodes, panels,
and other desktop layers never poke through the dimming backdrop.
Its tile title bands and Alt+Tab's cards use the shared
[`overlays:`](overlays.md) shape, palette, and border settings.
Apogee window titles, badges, hints, and Alt+Tab labels all use the exact
configured [`font:`](fonts.md) size.

Arrow keys navigate spatially across monitor geometry. Enter or a tile click
selects a window, Escape or an outside click cancels, and `Mod+O` toggles the
overview. Selection is applied only after the closing animation finishes.
Collapsed selections restore through the normal one-action node policy.
Once closing begins Apogee stops consuming keyboard input, so a key pressed
during the fly-back belongs to the restored client instead of becoming stuck
in the overlay. Its compositor cursor is forced visible for the whole session,
including when the previously focused client had locked or hidden its cursor.

```rune
apogee:
  enabled true
  live-previews true
  preview-max-fps 30
  transition-ms 320
  gap 24.0
  max-rows 3
  background-dim 0.85
end
```

`background-dim` darkens the wallpaper without adding a color tint. `0.0`
leaves it unchanged and `1.0` makes it black when Apogee is fully open; the
dimming fades smoothly with the opening and closing transition. Cluster
Composer uses the same setting.

Tiles use reusable GPU-local offscreen textures. DMA-BUF client buffers remain
on the GPU through import and composition; there is no CPU readback. A client
commit only dirties that window's cached tile, commits are coalesced, and
preview frame callbacks are limited independently per output to
`preview-max-fps`. Once transitions settle and clients stop committing,
Apogee produces no continuing render frames.

The default four-finger swipe up opens Apogee interactively. Progress follows
net finger travel in both directions, so reversing a slow swipe returns through
the exact same geometry and opacity state without rebuilding the overlay or
flashing. Release commits at 40% progress or on an upward flick; otherwise the
remaining distance animates back to zero. Four-finger swipe down closes an
already-open overview after the configured threshold.

```rune
input:
  gestures:
    swipe-threshold-px 120.0
    swipe-up-4 "apogee-open"
    apogee-swipe-down-4 "apogee-close"
  end
end
```

`Alt+Tab` and `Alt+Shift+Tab` use the same MRU window set, including collapsed
nodes. The center card is selected, releasing Alt commits, and Escape cancels.
Commit focuses and raises the selected window, minimally reveals it when
needed, then moves the pointer once to the center of its actual window,
fullscreen, or field-maximized presentation. A collapsed selection waits for
its restore animation before that final pointer handoff; cancellation does not
move the pointer.
Committing a different window on the same monitor exits fullscreen for ordinary apps, restoring
their previous normal placement. This releases both client and compositor
fullscreen, including nested browser/video fullscreen; returning leaves the app
windowed. Identified games instead keep their fullscreen state and buffer size
while their presentation is parked, and resume fullscreen on a direct click.
Games are parked only when selecting another window on their own monitor;
switching to a different monitor keeps the game fullscreen. In hover-focus
mode, returning to a parked game from another monitor also resumes its
fullscreen presentation. Hover within the same monitor keeps the click-to-resume
policy.
Fullscreen changes apply only to presentations on the destination monitor.
Switching to another monitor preserves the departing window's fullscreen,
client size, and presentation for every application, including browsers and games. Previewing or cancelling the switcher,
temporary focus changes, and maximize do not trigger this policy.
See [window rules](window-rules.md) for game detection and overrides.
The old five-card rail is restored: preview-hugging chip chrome, an overlaid
app-icon/title band, monitor badge, and `NODE` pill. The selected preview is
refreshed live while neighboring cards keep frozen GPU stills, avoiding a
full-rail redraw or CPU copy. The carousel and Apogee are mutually exclusive.
Cluster/core presentation is reserved for the later cluster runtime and is not
synthesized here.
