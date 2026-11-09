# Alt+Tab

`Alt+Tab` recalls recent work: it cycles through your windows, most recently
used first, including collapsed nodes. It is one of Halley's ways to find a
window. `Mod+Arrow` moves one step in the Field, Bearings points to work that
is off screen on a monitor, [zoom to fit](zoom-fit.md) shows everything at
once, and Halley Lift searches by name.

## Alt+Tab visibility and presentation policy

This section defines the intended Alt+Tab behavior, including fullscreen and
Field maximize.

`Alt+Tab` and `Alt+Shift+Tab` use the same MRU window set, including collapsed
nodes. The center card is selected, releasing Alt commits, and Escape cancels.
Commit focuses and raises the selected window, minimally reveals it when
needed, then moves the pointer to the selected output's center. Cancellation
does not move the pointer.

The deciding factor is whether revealing the selected target requires camera
movement, not whether focus leaves a fullscreen or maximized window:

- A target whose whole decorated window fits inside its monitor's current
  viewport is raised in place. Occlusion by another window does not count as
  offscreen. The camera, fullscreen, and maximize stay unchanged. A window
  exactly at the viewport edge counts as fitting; reveal padding alone must
  not cause a pan.
- A partially or fully offscreen target is minimally revealed. Only on that
  destination monitor, ordinary apps exit both client and compositor fullscreen
  (including browser/video fullscreen), and Field maximize exits so the Field
  can pan. Identified fullscreen games keep their fullscreen state and buffer
  size while their presentation is parked. A collapsed target uses the bounds
  of its restored decorated window. An oversized target is centered on axes
  where it cannot fit.
- Selecting a fullscreen or maximized target itself preserves its presentation.
  Switching monitors leaves presentations on the departing monitor untouched.
  Previewing and cancelling never change presentation or camera state.

This is one fixed Alt+Tab policy, with no partial-visibility setting.
`field.close-restore-pan` controls focus succession after closing a window;
`node.click-collapsed-pan` controls ordinary collapsed-node restoration. Neither
overrides this Alt+Tab rule.

Parked games resume fullscreen on a direct click. In hover-focus mode,
returning to a parked game from another monitor also resumes its fullscreen
presentation. Hover within the same monitor keeps the click-to-resume policy.
See [window rules](window-rules.md) for game detection and overrides.
The rail shows five cards with preview-hugging chip chrome, an overlaid
app-icon/title band, a monitor badge, and a `NODE` pill on collapsed windows.
The selected preview is refreshed live while neighboring cards keep frozen GPU
stills, avoiding a full-rail redraw or CPU copy. Card title bands use the shared
[`overlays:`](overlays.md) shape, palette and border settings and the exact
configured [`font:`](fonts.md) size.
