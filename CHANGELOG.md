# Changelog

All notable changes to this project will be documented in this file.

## [v0.5.0] - TBD

### Added
- On first launch Halley Lift now writes a documented default config template (mirroring
  `examples/lift.rune`) to `~/.config/halley/lift.rune` when none exists; existing files are
  never overwritten.
- Add Halley Lift `colors.icon` (result-list icon tint; empty follows `accent`) and
  `colors.alt-hint` (Alt+<n> jump-label tint; empty follows `hint`) config options.
- Add Halley Lift config-editing actions for both Lift config and Halley compositor config;
  config actions now open files with `$EDITOR` in Lift's configured terminal instead of `xdg-open`.
- Add the Halley Discord community/support invite to the README.
- Add `nodes.opacity` (`0.0`–`1.0`, default `1.0`) to dim the node/core marker *body* (its
  fill) so markers recede into the field; the border ring and app icon stay fully opaque.
  Implemented via a `fill_alpha` shader uniform that only fades the fill region. Node
  backdrop blur was intentionally not added — nodes never overlap windows, so their only
  backdrop is the wallpaper; use `effects.shadows.node` for node depth instead.
- Add frosted-glass backdrop blur behind the aperture-peek clock (`aperture-peek.blur`,
  default `true`) and behind bearing chips (`bearings.blur`, default `true`). The aperture
  panel is blurred via the layer-shell path keyed on its `halley-aperture` namespace; bearing
  blur is drawn as a compositor overlay and fades in lockstep with each chip's distance fade,
  so it disappears as the node recedes. Both honour the global `effects.blur` switches.
- Add a top-level `effects:` config section for renderer-level visual effects. It holds an
  `effects.blur:` block (`enabled`, `overlays`, `windows` = `off`/`auto`/`always`, `method`,
  `radius`, `passes`, `saturation`, `noise`) and the relocated `effects.shadows:` blocks
  (window/node/overlay). Backdrop blur targets compositor overlays and client windows only
  (not nodes/node chrome). Overlays opt in via `overlays.blur`; individual windows opt in/out
  via a per-rule `blur true/false`. Resolution policy: global `effects.blur.enabled` gates
  everything; rule-level `blur false` always wins; rule-level `blur true` opts a window in even
  under `windows "off"`/`"auto"`; `auto` blurs only translucent windows. Wired into the internal
  template, bootstrap backfill, and example configs (defaults to `enabled false`).
- Add `effects.blur.layer-shell` with `off`/`auto`/`always`, enabling compositor-controlled
  backdrop blur for layer-shell clients such as launchers, notifications, and their popups.
- Add Smithay `ext-background-effect` protocol support with the blur capability advertised and
  conservative rendering integration for XDG toplevels, XDG popups, layer-shell surfaces, and
  layer-shell popups.
- Add RGBA overlay background parsing for `#rgba` and `#rrggbbaa`, so compositor overlays can
  use configured translucent backgrounds.
- Add configurable Halley Lift caret settings for visibility, width, blink timing, and
  stop-blink timing.
- Add a Halley Lift `term` search mode that runs the typed command line in the
  configured `terminal` through the user's interactive `$SHELL` (so aliases, pipes, `&&`, and
  quoting work), keeps a shell open afterward, and then closes Lift.
- Add a configurable Halley Lift `border:` block (`enabled`, `width`, `style`). `outline`
  wraps the whole app — the search bar when collapsed and the search bar plus results as one
  unit when expanded — keeping the top-corner radius continuous across the transition (top
  corners use `rounding.search`, the expanded bottom uses `rounding.panel`); `inset` keeps the
  legacy results-only border. Uses `colors.panel-border`.
- Add a Halley Lift search-bar magnifier icon: a `search-icon:` block (`enabled`, `side`
  `left`/`right`, `size`) with a `colors.search-icon` tint (empty follows `hint`). The bundled
  square SVG is rendered to an alpha mask and tinted at draw time.
- Add a dedicated Halley Lift settings glyph (bundled `settings.svg`) shown as the search-bar
  icon while the `config` search mode is active and as the row icon for config-editing results
  (Lift config / Halley config), so config entries are no longer iconless.
- Add a dedicated Halley Lift Apps-mode search glyph, alongside refreshed bundled search and
  terminal glyph strokes for clearer small-size rendering.
- Add drawn fallback glyphs for Halley Lift result rows without a raster icon: a squircle for
  nodes and a ring for `term` entries.
- Add libinput input-device customization to the `input:` config section: per-class
  `touchpad:` and `mouse:` blocks (tap-to-click, natural-scroll, disable-while-typing,
  accel speed/profile, scroll method, click method, tap-button-map, middle-emulation,
  left-handed, send-events) plus per-device override blocks under `input.devices.<name>:`
  matched against the `libinput list-devices` name. Settings apply on device hotplug and
  re-apply live on config reload; unset keys keep libinput's own defaults. Also adds an
  `input.keyboard.model` xkb key. Wired into the internal template, bootstrap backfill, and
  example configs.
- Add a native `xdg-desktop-portal-halley` ScreenCast backend with D-Bus activation,
  portal session state, PipeWire stream creation, and monitor/window source metadata.
  The compositor owns the actual source selection and frame capture through a narrow
  Halley IPC surface.
- Add compositor-paced portal screencast sessions backed by shared-memory frame files.
  Monitor streams receive full output frames, while window streams crop the selected
  live window from its host output before the portal backend publishes the PipeWire
  `Video/Source` stream.
- Add fd-passing support to the Halley IPC socket and wire portal screencast DMA-BUF
  buffer registration/render/remove requests through the compositor, allowing PipeWire
  DMA-BUF buffers to be rendered by the compositor process instead of forcing the
  shared-memory readback path.
- Add a Halley-native ScreenCast source chooser overlay for portal clients, including
  monitor picking, window picking, direct single-source selection for apps that already
  chose a type, and screenshot-style hovered-window highlighting.
- Add `halleyctl portal status` and `halleyctl portal version` diagnostics for checking
  backend discovery, compositor IPC, advertised sources, cursor modes, and versions.
- Add gesture/touch input configuration under `input.gestures`, including touchpad
  gesture passthrough, touchscreen passthrough, compositor pinch-to-zoom, configurable
  swipe bindings, Apogee swipe handling, and gesture scope/modifier controls.
- Add `halley --nested` as an explicit nested compositor launcher. It forces the winit
  backend, creates a visible host window titled `Halley`, opens a nested Wayland socket for
  clients, and avoids full-session startup behavior such as session autostart.
- Add a native `org.freedesktop.impl.portal.Screenshot` backend in
  `xdg-desktop-portal-halley`, supporting Screen, Window, and Area targets via the
  existing Halley capture overlay. Portal requests map to compositor capture modes,
  poll the capture IPC, and return a `file://` URI. `PickColor` is advertised as
  unsupported for now. GTK remains the fallback for other portal interfaces.
- Add `apogee.open-cluster-on-select` (default `true`) so selecting a cluster core in Apogee opens its
  workspace (entering it via `enter_cluster_workspace_by_core`) instead of only panning to the core.
- Surface tile-cluster overflow members in Apogee, flying in from the overflow strip with app-icon
  fallback previews, and promote a selected overflow member into the master slot (re-laying out the
  cluster and shifting the old master into overflow). Selecting any cluster workspace member in Apogee
  now promotes it to master for both tiling and stacking layouts.
- Add frosted-glass backdrop blur behind Apogee tile labels (Dual-Kawase, honouring the
  global `effects.blur` switches) so labels stay legible over the blurred window thumbnail
  behind them; the Apogee render fast path now sets up a `FrameBlurContext` for overlay chrome.
- Add arrow-key navigation in Apogee: the arrow keys move a highlighted selection across the
  window mosaic and the core rail, Enter activates it (windows fly to focus; cluster cores open
  their workspace), and Escape closes. Navigation is unified with the mouse — pressing an arrow
  warps the cursor onto the target tile so keyboard and pointer drive the same single hover/focus
  (a later mouse move continues from there) — and it crosses monitor boundaries in global screen
  space (jump to the next monitor's tiles, no wrap).
- Show a frosted name label below a hovered or keyboard-selected cluster **core** tile in Apogee
  (cores previously showed only their icon); the label stays up for the whole hover.
- Add a `center-last-focused` keybind action (default `mod+h`) that pans the camera back to centre
  on the last focused node — a quick "go back" after wandering the field. Wired into the internal
  template, bootstrap backfill, and example configs. The bare-defaults field node-move bindings
  also move from vim `hjkl` to the arrow keys, matching the generated config and freeing `mod+h`.
- Add `cursor.hide-on-keyboard-nav` (default `true`): any compositor keybind and keyboard-driven
  window navigation (focus cycle, tile/stack/trail/monitor steps, and Apogee arrow keys) now hides
  the cursor image — the pointer position is preserved so focus-tracking warps keep working — and
  any real pointer activity (motion/button/axis, including inside Apogee) reveals it again.
- Map a client maximize request (e.g. a GTK title-bar maximize button) to fullscreen: edge-to-edge,
  zoom 1.0, no decorations. An app re-request never clobbers an existing fullscreen or flips its
  origin, and an app unmaximize only tears down a fullscreen that began from a client maximize — so
  an app unmaximize can never dismiss a user-initiated (Mod+F) fullscreen.
- Add cluster workspace open/close animations via a new `animations.cluster` block
  (`enabled`, `tiling.{open-duration-ms, stagger-ms, close-duration-ms}`,
  `stacking.{open-duration-ms, close-duration-ms}`). Opening cascades tiling members in
  from the left with a per-member stagger (slaves first, master last) and tunes the
  stacking card grow-in; closing captures each visible member as a shrink/fade ghost that
  glides into the core node ("suck into core"). Wired into defaults, the parser, the
  validator, bootstrap backfill, and the example configs.
- Add fullscreen support inside cluster workspaces. The `toggle-fullscreen` keybind is now
  `Global`-scoped (was `Field`, which is filtered out under cluster scopes) so `Mod+F`
  works there; when a member fullscreens, its sibling tiles hide and the fullscreen member
  grows from its visible tile rect, and exiting restores and re-lays-out the workspace.
- Auto-fullscreen game-like windows (`steam_app_*`, gamescope) on top of whatever layout they
  joined — cluster tiling or free-floating field — once their `app_id` arrives. Most games
  request `xdg_toplevel set_fullscreen` themselves; this mirrors it for the ones that don't,
  so a launched game goes fullscreen without an extra step while still joining the cluster
  like any other window.
- Draw a tinted cluster glyph on a cluster core's bearing chip instead of the app-icon
  fallback box + first letter, independent of `node-show-app-icons`.
- Add an animated background renderer with three modes: `none` (solid fill), `classic`
  (static image with fit/cover/stretch), and `field-shader` (a GLSL fragment shader
  rendered as the compositor background). Configured via `background:` (alias `gesso:`)
  with `mode`, `fit`, `intensity`, `animated`, `colour`, `accent-colour`, and `shader`.
  Animated field-shader backgrounds are per-monitor gated with startup and DPMS-wake
  grace periods and capped to ~10 FPS redraw to avoid high idle CPU.
- Add direct field rendering for normal windows via `RescaledSurfaceElement`, which maps
  the live Wayland surface tree from a stable base-geometry coordinate space into the
  current visual (zoomed) rect using corner-rounded pixel mapping. Eliminates per-window
  offscreen texture allocation for steady-state field and cluster windows.

### Changed
- Treat field node/core markers as fixed landmarks in passive (idle/zoom) overlap resolution:
  neighbouring windows now yield around a marker instead of the marker being pushed aside. A
  marker boxed between two windows used to have nowhere to go and ended up overlapped (most
  visible when zooming out, where each marker's keep-out gap grows in screen-constant space);
  windows now spread to keep its gap clear. Pinning a node is no longer required to make it a
  landmark — it now only affects drag-carry behaviour.
- Maximize and fullscreen are now mutually exclusive per monitor: entering fullscreen aborts any
  active maximize session on that monitor, and maximizing a window exits any active fullscreen on
  it. Previously the two could coexist — a maximize session was preserved underneath fullscreen so
  a maximized window returned from fullscreen still maximized (and maximizing was blocked while
  fullscreen). Cross-window maximization on the same monitor now exits the other window's
  fullscreen too.
- Animate the Alt+Tab focus-cycle switcher with a quick open fade/scale and smooth carousel-style
  card motion between selections, while keeping the existing bounded snapshot prewarm behavior.
- Open Apogee on every active monitor at once, with each monitor showing only its own windows and
  cluster cores, and close all monitor views together when selecting a target.
- Apogee now always renders the field overview, even when a cluster workspace is open. Previously
  opening Apogee over an opened cluster expanded the cluster's member tiles into the mosaic (a
  workspace view); the opened cluster is now shown collapsed as a single core icon while the
  regular field windows hidden behind the workspace reappear at their field positions. The
  per-window "select a cluster member in Apogee to promote it to master" behaviour has been
  removed along with it, since cluster members are no longer surfaced as Apogee tiles.
- Apogee cluster cores now always show their label beneath the icon. Hovering or
  keyboard-focusing a core dissolves the icon away in place and expands the tile into a
  small live window into that cluster: each member's real offscreen thumbnail is laid out
  just like the cluster's workspace (master + stack for tiling, layered cards for stacking,
  with a "+N" tail for overflow). The Apogee core band grew slightly to reserve room for the
  expanded viewport, so the window mosaic starts lower and the hovered cluster expands in
  place rather than as a detached popover. Member textures are kept warm, so collapsed
  clusters preview immediately.
- Rework Apogee preview capture so each active monitor fills missing snapshots in small batches,
  prioritizes the hovered live preview, and rate-limits live refreshes to keep the overview
  responsive with many windows.
- Treat touchpad finger scrolling as high-resolution input: compositor wheel bindings now use an
  accumulated threshold instead of firing on every tiny delta, while two-finger scroll over empty
  field space pans smoothly on both axes without adding gesture handling yet.
- Animate compositor fullscreen exits back to the restored window geometry, using the existing
  fullscreen animation settings and skipping soft-suspend/fullscreen-preservation paths.
- Make camera zoom inertial, like a powered lens. Instead of easing to a fresh target on every
  press, each zoom input injects velocity in log(view-size) space: repeating in the same
  direction stacks velocity into an accelerating ramp, the opposite direction bleeds it off, and
  friction coasts it to a smooth stop. A single press still travels ~one `field.zoom.step`, and
  `field.zoom.smooth-rate` now doubles as the glide friction (higher = snappier, lower = longer
  sweep). `field.zoom.smooth false` keeps instant jumps.
- Move window/node/overlay shadows out of `decorations:` into the new `effects.shadows:` block;
  `decorations:` now holds only compositor chrome (borders, secondary border, resize-using-border).
  Legacy `decorations.shadows` configs are no longer parsed: the loader reports
  `decorations.shadows has moved to effects.shadows`, and the config auto-updater relocates an
  existing `decorations.shadows` block (preserving customized values) into `effects.shadows` on
  reload.
- Rename Halley Lift (`halley-lift`) across the workspace, launcher layer
  namespace, command bindings, README, and lockfile while preserving the command-palette
  implementation. The first-party launcher binding now behaves like a toggle: pressing the
  bound command closes an existing Lift overlay instead of racing a second instance, and
  compositor keybinds dismiss the focused Lift layer fuzzel-style.
- Rename the Aperture binary from `aperture` to `halley-aperture` for consistency with the
  other first-party tools.
- Clean up compositor startup ownership so backend selection happens before fixed IPC
  initialization. TTY/session instances own `/run/user/$UID/halley/halley.sock`; nested winit
  instances no longer create or replace that session IPC socket. `halley --session` now runs
  the TTY/session path directly instead of setting `HALLEY_WL_BACKEND=tty` internally, and IPC
  startup refuses to replace a live socket while still removing stale refused sockets.
- Treat the winit backend as nested by default: create the host winit window before creating
  the nested Wayland socket, skip session `autostart.once` commands on startup, skip reload
  autostart commands on winit config reloads, and log when physical libinput settings are
  configured on winit where the host compositor owns the devices.
- Decode Halley Lift result icons on a background worker thread and persist the resolved icon path index under `$XDG_CACHE_HOME/halley/lift-icons`, so icons no longer block the UI thread while decoding and warm launches skip the icon-directory walk entirely (the index is ready before the first draw on subsequent launches).
- Split TTY output/layout helpers and TTY frame-clock/presentation helpers out of
  `backend/tty/mod.rs` into dedicated modules, reducing the monolithic TTY backend while
  preserving behavior.
- Rework backdrop blur rendering around z-ordered framebuffer capture. Each blurred window,
  layer-shell surface, popup, or compositor overlay now captures the framebuffer immediately before
  drawing its blur patch instead of using a stale global below-window snapshot.
- Render layer-shell blur through an alpha-mask pass so transparent client pixels and rounded
  client shapes cut out the blurred backdrop correctly.
- Track compositor-keybind fullscreen separately from client-requested fullscreen so user-keybind
  fullscreen windows can blur when policy requests it while game-like, gamescope, browser-video,
  and client-requested fullscreen surfaces remain excluded.
- Add configurable raise animation triggering via `animations.raise.trigger "always"` or
  `"overlap"`.
- Prefer Halley's native ScreenCast portal backend in packaged portal configuration instead
  of `xdg-desktop-portal-wlr`, while leaving GTK as the default fallback backend for other
  portal interfaces.
- Add an opportunistic DMA-BUF ScreenCast render path for PipeWire buffers that are already
  delivered as DMA-BUFs, while keeping mapped shared-memory buffers as the default-compatible
  path for OBS, Discord, and PipeWire setups that do not expose suitable DMA-BUF buffers.
- Make `halley-session` start and wait on the systemd user `halley.service` when available,
  so `graphical-session.target`, desktop autostart, and portal services come up correctly
  under display managers.
- Start the compositor-side package track at `0.5.0` for the main compositor, Wayland backend,
  config, IPC, CLI, and capture crates, while keeping `halley-api` on its independent track,
  pinning `halley-core` at `0.4.0`, and leaving `halley-lift`, `halley-aperture`, and
  `halley-portal` on their existing `0.1.0` package tracks.
- Use DRM cursor-plane scanout for TTY composed frames on single-GPU, untransformed outputs when
  available, keeping software cursor rendering for winit, portal captures, transformed outputs,
  multi-GPU outputs, or `HALLEY_DISABLE_CURSOR_PLANE=1`. Cursor elements are submitted as
  `Kind::Cursor` so Smithay can use hardware cursor planes and fall back to primary-plane
  composition if the plane rejects the cursor.
- Add configurable multi-finger hold gesture bindings (`hold-3`, `hold-4`, etc.) under
  `input.gestures`, reusing existing compositor gesture action names. Hold bindings are routed
  through libinput's hold gesture and respect the same `compositor-scope` and gesture modifier
  rules as swipe bindings. Client passthrough is preserved when no matching hold binding exists.
- Restore the pre-fullscreen camera zoom/center on genuine fullscreen exit.
- Improve resize-by-border interaction with a minimum edge grab band, hover resize handles, and
  plain left-press edge resize/release behavior.
- Polish Apogee hovered live-preview feedback with an accent label and transparent focus ring.
- Cap gesture-driven Apogee open scrub speed so hard four-finger flicks still commit the overview
  but no longer visually snap the open animation faster than the configured interaction can read.
- Defer Apogee selection activation until the close animation finishes so the desktop doesn't
  mutate underneath the fading overlay. Maximized and fullscreen windows no longer flash or
  displace when selected from the overview: the close animation flies back to the actual
  presentation visual rect instead of the stale windowed field position.
- Move the "Open Lift config" and "Open Halley config" entries out of the Halley Lift Actions
  mode and into the Config mode only (where they already existed). Actions now exposes just
  "Reload Halley config"; both open-config entries remain reachable from the default General
  search via the Config provider.
- Flatten the compositor's `*Controller` wrapper structs (ClusterSystem, ClusterRead, ClusterMutation,
  FocusSystem, FocusState, FocusCycle, FocusDecay, FocusTrail, Fullscreen, ExitConfirm, Runtime,
  SpawnReveal, Screenshot, Camera) into free functions taking `&Halley`/`&mut Halley`, and trim the
  thin one-line `root.rs` facade methods that only delegated to those functions, removing the last dead
  code (unused `FullscreenCtx`, empty `debug_dump`) and clearing workspace-wide warnings. Pure
  mechanical transform; no behavior changes.
- Fullscreen windows now grow and shrink in place, centred on the window's own position, with
  the monitor camera easing to centre on the window and zoom to 1.0 together. The old behaviour
  snapped the zoom to 1.0 about the (often off-window) camera centre, shoving every windowed node
  behind it sideways by the zoom delta; the steady-state fullscreen rect also anchors on the node
  centre now, so there is no jump when the grow/shrink animation expires mid-camera-ease.
- Narrow config-reload texture invalidation: offscreen window textures are rebuilt only when the
  baked border-corner radius actually changes (including crossing 0, which toggles the geometry
  clip), not whenever the whole `decorations` block differs. Border sizes/colours, shadows and
  blur are drawn live per frame, so a colour-only theme reload no longer flushes every window's
  offscreen texture.
- Drag-and-drop now always raises the dropped window to the front, independent of
  `input.raise-on-click`, so a window dropped over peers on another monitor no longer lands
  behind them.
- Bar maximize for any cluster member — collapsed under a core or laid out in an active
  workspace — since it conflicts with the cluster's own tiling/stacking session. A maximized
  window joining a cluster drops its maximize session first, and a client maximize request
  (e.g. a GTK title-bar button) on a cluster member is silently ignored rather than mapped
  to fullscreen.
- Smooth the fullscreen resume-from-soft-suspend re-centre: the camera no longer snaps to
  the saved centre, instead easing there in lockstep with the re-zoom for a grow-in-place
  (off the active monitor restores directly).
- Replace inlined `ease_in_out_cubic` curve bodies with the shared
  `crate::animation::ease_in_out_cubic` helper and extract `FULLSCREEN_MIN_W`/`H` constants
  (no behavior change).
- Replace the tile-track-based reflow hold with an independent three-phase material-bridge
  handoff for tiled cluster reflows. When a slave is promoted to master or a remaining slave
  expands into a closed sibling's space, the compositor now: (1) **travels** the old-size
  material body (rounded fill, blur, border, shadow — no client pixels) from the old slot to
  the target area over ~200ms; (2) **expands** the material from old size to target size over
  ~150ms; (3) after a 45ms hold, **reveals** the real target-size client content via a
  non-linear 175ms crossfade (texture eases in, bridge lingers then fades out). Real client
  content is fully suppressed until the client has committed the target-size buffer — no
  stretched textures, no missing bottom halves, no abrupt size snaps.
- Enable EGL hardware acceleration for Wayland clients by binding the compositor's EGL
  display to the Wayland display via `bind_wl_display` in both the TTY/DRM and winit
  backends. This creates the `wl_drm` global and calls `eglBindWaylandDisplayWL`, which
  Mesa's client-side EGL Wayland platform requires. Without it, GPU-accelerated clients
  (Qt/EGL apps such as Quickshell, GL applications, Electron) could not create EGL
  surfaces and crashed with `EGL_BAD_DISPLAY` / `EGL_BAD_SURFACE`; only software
  (`wl_shm`) rendering worked. Requires Smithay's `use_system_lib` feature, now enabled.
- Cover-crop (object-fit: cover) the offscreen texture to the destination aspect
  ratio during tiled cluster reflow transitions, preventing texture squish when
  the animated box's aspect differs from the capture. Only active during tile
  transitions; steady-state rendering keeps the plain fill.
- Render normal field and cluster top-level windows directly via live surface elements
  instead of through per-window offscreen textures. Opacity below 1.0 renders direct
  via alpha, and window backdrop blur renders as direct framebuffer blur patches drawn
  immediately before each window's surface elements. Active resize uses the same live
  direct path. Per-window offscreen is now limited to semantic transitions (tile reflow,
  stack cycle, open animation), close/suck ghosts, Apogee/Alt+Tab previews, hover preview
  cards, and capture/screenshot paths.
- World-anchor close-animation ghosts by capturing the camera center at animation start
  and re-projecting baked screen geometry against the live camera each frame, so ghosts
  stay anchored to their world position during camera pan instead of sliding with the
  screen. Node close markers are captured as screen-local coordinates at close time.
- Keyboard launches now latch to the compositor's `primary_interaction_focus` monitor
  first, then the Wayland keyboard focus surface monitor (walking parent surfaces), then
  `focused_monitor` as fallback. `pending_spawn_monitor` survives focus/interaction churn
  until the next toplevel consumes it, so field-jump keybind spawns land on the focused
  monitor instead of the stale cursor monitor.
- Keep the cursor visible during zoom keybinds (zoom is spatial, not keyboard navigation)
  and show directional `ZoomIn`/`ZoomOut` cursor icons. Zoom keybinds use the current
  interaction monitor so mouse monitor switching mid-zoom keeps working.
- Start close-animation ghosts at compositor close-request time when the offscreen
  snapshot cache is warm, and skip the live surface for nodes with active close ghosts
  so the ghost is not doubled by a camera-following live surface. Close snapshot prewarm
  is limited to focused/keyboard-close candidates instead of all visible windows.
- Remove raw `hover_node` and `overlay_hover_target` as permanent animation-redraw
  triggers from the TTY scheduler. Hover animations still start on pointer-motion redraw
  requests, but settled hover no longer forces continuous vblank redraw. Hover preview
  activity is gated to "mix is moving" only.
- Compute per-monitor camera smoothing activity from each monitor's own saved camera state
  rather than only the current monitor's live viewport, so a non-current monitor caught
  mid-zoom continues settling (and repainting) instead of freezing when the pointer leaves.
- Stop the aggressive DPMS-wake `reset_buffers()` call that slowed monitor recovery; keep
  stale composed-frame cache eviction on DPMS wake.

### Fixed
- Fix window-parented XDG popups (e.g. Firefox context menus, nested menus) staying visually
  at 1.0 zoom and mis-hit-testing when the field camera is zoomed out. Popup origin/transform
  math is now shared between the render and focus paths, and popups are routed through the
  offscreen-texture composition path whenever their scale differs from 1.0 so they scale with
  the camera instead of being drawn as unscaled live surface elements.
- Fix Steam's pinned notification popups (install-complete, etc.) staying visually at 1.0 when
  zooming out. They keep their pan-immune monitor anchor but now apply camera zoom to both size
  and displacement.
- Fix the animated field-shader background (e.g. the builtin stars) stuttering on empty or
  non-current monitors. The animation-redraw gate was restricted to the current monitor and the
  frame cadence was throttled to ~10 FPS, so an idle second monitor's background only advanced
  on unrelated redraws. Both restrictions are removed; startup and DPMS-wake grace pauses are
  preserved.
- Smooth the maximize↔fullscreen transitions, which flashed: switching between the two modes
  tore the outgoing mode down (snapping the window to its small windowed size) before the
  incoming mode's grow animation started. Each direction now captures the outgoing window's
  on-screen rect (the maximized rect when fullscreening, the full-screen rect when maximizing)
  and eases from it, so the window grows/shrinks directly between maximized and full-screen
  with no intermediate snap.
- Fix a window staying stuck at maximized size after a `maximize → fullscreen → maximize →
  unmaximize` sequence. Exiting fullscreen straight into a maximize re-snapshotted the size
  from the still-stale fullscreen/maximized surface geometry, so unmaximize "restored" to that
  size. `restore_fullscreen_snapshot` now pins the restored windowed size into the node's
  `resize_footprint` *after* the footprint sync (the sync was clearing the value the prior fix
  set), so the re-maximize snapshots the true windowed size before the client commits its
  resize.
- Eliminate the full-size buffer flash at the tail of a fullscreen or maximize exit shrink.
  The client is now reconfigured to its windowed size at the *start* of the shrink (while the
  frozen snapshot is still on screen), and the shrink holds that snapshot past its visual
  duration until the client has committed a non-fullscreen/non-maximized buffer (or a 250 ms
  safety timeout). The live surface is revealed only once it is already windowed-sized, so the
  old one-or-two-frame full-size flash never reaches the output.
- Ease the camera back on animated cluster-member fullscreen exits instead of snapping it
  synchronously. The survivor reflow is deferred until the shrink settle lands, so the camera
  pan finishes before the tiles re-lay out — avoiding the old "slides from left, stops partway"
  race between the pan and the reflow.
- `toggle-fullscreen` now prefers a focused overlay window stacked above a fullscreen window on
  the same monitor, so the keybind swaps the overlay into fullscreen rather than redundantly
  toggling the fullscreen window underneath.
- Include fullscreen and maximized nodes in the close-animation snapshot prewarm set so their
  offscreen textures are ready before the exit shrink begins, and skip the border clip during a
  visual shrink animation so the whole surface is captured.
- Stop runaway key repeat (e.g. Enter repeating forever in a terminal after first opening a
  cluster) for good, with a general guard instead of another per-case patch: physical key
  state is now tracked and, after every key event, any key still forwarded to a client as
  pressed but no longer physically held is released (`reconcile_forwarded_keys`). This covers
  modal/overlay interactions that begin and end on the same surface, async opens, and deferred
  focus to freshly-revealed windows — paths the previous focus-change-only flush missed.
  Genuine key-holds and held modifiers are unaffected, and popup-grab focus now routes through
  the same focus choke point.
- Fix corrupt first-fullscreen rendering of XWayland windows (e.g. Steam) where the live surface
  was blown up so only its top-left corner filled the screen until you toggled fullscreen again.
  A fullscreen surface's render geometry is now derived from its live buffer rather than the cached
  xdg window-geometry, which can lag the buffer right after the client goes fullscreen and made the
  render scale come out far too large. Routed through one `render_window_geometry_for_node` helper
  shared by the field, offscreen-compose, and close-capture paths (Apogee/Alt+Tab previews already
  handled this).
- Stop field node/core markers from being hidden underneath windows. Markers are screen-space
  constant (readable at every zoom) and now draw above window bodies and borders — but below
  popups, overlay HUD, and their own hover labels — so a window grown over a marker can no longer
  occlude it.
- Ensure client-side fullscreen requests still send the xdg fullscreen configure when the window
  was already fullscreened by a Halley keybind, so client fullscreen buttons map cleanly onto
  Halley's fullscreen state.
- Remove the oversized pale proxy marker during collapsed-node field drags, and snap released
  collapsed nodes back to marker animation state to avoid a large white flash.
- Use an Apogee-specific render fast path while the overview is active: skip hidden field window
  rendering, keep background/bottom layer surfaces visible behind the dim, and draw Aperture above
  Apogee in minimal mode.
- Keep Aperture promoted above Apogee regardless of which layer-shell layer it uses.
- Restore Aperture visibility in the Apogee fast path by drawing non-Aperture background/bottom
  layers below Apogee, then promoting Aperture-only layer-shell surfaces above the overview so the
  minimal tab remains visible.
- Pin Steam client notification popups to the monitor output instead of letting them track spatial
  camera pan/zoom, while leaving ordinary window-parented context menus attached to their parent
  windows.
- Crop cached Alt+Tab/Apogee/hover preview textures to the real window aspect before fitting them,
  and remap rounded-texture shader coordinates for cropped sources, avoiding square Firefox/GTK
  thumbnails or square preview corners when their surface-tree cache is padded.
- Capture collapsed surface-node previews for Apogee instead of leaving their card bodies black.
- Keep a dropped window fixed at its exact release point while overlap resolution pushes neighboring
  windows aside, avoiding a post-drop snap of the window being moved.
- Render collapsed surface nodes in Apogee using the original window preview aspect/weight instead
  of the collapsed marker footprint, so they match the shape they had before collapsing.
- Keep a window raised after you resize it instead of snapping it back behind whatever it was
  under. Resizing an occluded window now lifts it forward and *commits* that position on release
  (using the same persistent overlap order as `raise-on-click`), rather than only floating it on
  top for the duration of the drag. Honors the `input.raise-on-click` policy: with it off, the
  window still returns to its stack position once the resize settles.
- Keep a monitor's zoom animation easing to completion when the pointer crosses to another
  monitor mid-zoom. Previously only the active monitor's camera was ticked, so a monitor caught
  mid-zoom would freeze on screen and only resume when the pointer returned; every monitor's
  camera now settles (and keeps repainting until it does), independent of pointer focus.
- Let plain `halley` launched from inside Halley auto-select the nested winit backend by
  removing inherited `HALLEY_WL_BACKEND` from spawned app environments. This keeps the session
  wrapper explicit while preventing spawned nested compositors from being forced back into the
  TTY path.
- Stop forcing `SDL_VIDEODRIVER=wayland` for spawned applications so older SDL/Unity games can
  fall back to X11/Xwayland when their native Wayland path fails.
- Advertise the canonical/main TTY output first so Xwayland/XRandR sees the intended primary
  output ordering for monitor selection.
- Keep the Xwayland RandR primary output synced to the active cursor monitor and advertise complete
  `wl_output` mode, scale, transform, and preferred-surface state before clients bind, so SDL/Unity
  Xwayland games pick the correct monitor and resolution at startup.
- Make winit/nested input-device configuration behavior explicit: `input.touchpad`,
  `input.mouse`, and `input.devices` are applied on the TTY backend that owns libinput devices,
  and winit now warns instead of silently appearing to ignore those settings.
- Keep the Halley Lift result highlight on the first entry while typing, so filtering a query no longer leaves the selection stranded on whatever row the mouse last hovered; the highlight only follows the pointer again once it is physically moved over a different entry.
- Eliminate Halley Lift icon load stalls where some icons appeared instantly while others took seconds, by removing the per-request recursive icon-directory walk that ran on the UI thread before the icon index was ready.
- Fix backdrop blur shader coordinate mapping so rounded and masked blur patches sample local
  surface coordinates correctly on non-origin surfaces.
- Prevent blur render failures from aborting the whole frame by logging and skipping the failed
  blur patch for that frame.
- Allocate blur resources for overlay-only frames so built-in compositor overlays can blur even
  when no client window blur is active.
- Blur raw cluster overlay primitives such as bloom tokens and join-affordance circles that bypass
  the shared overlay chip renderer.
- Fix Halley Lift caret placement so trailing spaces advance the caret immediately and empty input
  still shows the caret.
- Fix native ScreenCast startup by creating screencast SHM files with read/write mappings,
  retaining PipeWire stream listeners for the session lifetime, activating streams after
  connect, and writing buffers through PipeWire's chunk/data APIs so portal consumers receive
  live frames instead of black previews.
- Track PipeWire ScreenCast stream state through compositor IPC, without treating the initial
  `Paused` startup state as a signal to stop producing frames for portal consumers.
- Align the crates.io v0.4 API surface by correcting package metadata and removing stale Gamescope
  config exports from the public config crate.
- Tighten the portal source chooser visuals by removing excess mode-bar padding and matching
  the screenshot overlay's single hovered-window highlight behavior during window selection.
- Place window-parented XDG popups within their parent window's monitor, preventing context menus
  and dropdowns from being constrained by another active monitor.
- Fix Halley Lift cluster creation so the cluster-mode search text is only used for filtering,
  not as the cluster name; Lift-created clusters now keep the compositor's default cluster naming
  behavior unless a real name is submitted.
- Stage app windows launched for a Halley Lift cluster inside the pending cluster build instead of
  letting them enter the normal field lifecycle first. Staged windows skip normal reveal, raise,
  overlap, spawn animation, and render collection paths until they are either absorbed into the
  final cluster or intentionally released.
- Prevent already-open matching apps such as Firefox from being mistaken for newly launched Lift
  cluster members when their app identity refreshes; only windows created as staged candidates for
  the pending Lift build can satisfy app-launch slots.
- Keep Lift-launched cluster members hidden through final cluster creation and collapse, removing
  the standalone field-window flash before the collapsed cluster core appears.
- Block compositor pan/zoom gestures and keyboard zoom actions while cluster mode or an active
  cluster workspace is in control, keeping cluster interactions from moving the field underneath
  the cluster UI.
- Preserve the node icon fade timer when re-snapping an animation track to its current state (as
  drag release does), so the node icon no longer flashes off and back on at the release point.
- Stop raising the just-dropped window on drag release and clear hover state at release, removing
  the residual marker flash; collapsed-node drags no longer borrow the active-window static lock,
  so they do not push neighbouring windows aside on drop.
- Flush stale forwarded (non-modifier) keys on every keyboard focus change through a single
  `set_keyboard_focus` choke point, so the newly focused surface can no longer inherit a stuck key
  from a dead layer-shell launcher, dismissed screenshot/portal overlay, or session-lock transition
  and start repeating it forever. Modifiers are preserved so held Ctrl/Alt/Shift/Super carry across
  the handoff.
- Clear stale modal key-traps on portal chooser teardown so a dismissed screenshot/portal source chooser
  can no longer leave a trapped Enter that repeats forever in the next focused client. The chooser's
  `modal_release_keys` and `forwarded_pressed_keys` are now cleared in `finish_modal_capture`, the single
  choke point every chooser exit (cancel, selected, cancelled) reaches; the redundant open-time Enter
  pre-trap is dropped since `begin_modal_keyboard_capture` already routes through `set_keyboard_focus`.
- Selecting a maximized window in Apogee no longer leaves it off-centre on return (the reveal pan is
  skipped for the monitor's maximize target), closing Apogee without a selection no longer flashes the
  wrong window z-order (each monitor's tiles are reordered to the live desktop draw order before
  fly-back), and selecting a cluster core now opens its workspace via `apogee.open-cluster-on-select`.
- Resolve a Lift-created cluster core dropping on top of a window by running overlap resolution
  immediately in `finish_lift_finalized_cluster` so the core slides clear, and stop selecting a cluster
  member in Apogee from shifting the whole cluster off-centre by skipping the reveal pan when the monitor
  has an active cluster workspace.
- Collapse a cluster core's open bloom when grabbing it to drag, so the bloom no longer stays fanned out
  while the drag does nothing; covers the plain press-to-drag-threshold path in addition to the drag/move
  binding paths that already collapsed it.
- Give Apogee core tiles the same look as normal core nodes: themed cluster fill colour (honouring
  `node_background_color`) and a 5px themed border ring (honouring `node_border_color_*`), plus a hover
  ring driven by new `apogee_hover_node` state. A Lift-created core now also slides clear of windows using
  the animated landmark push instead of a trapped-landmark teleport.
- Keep the outgoing stack member rendering during a forward (Next) stack cycle so it flies out to the
  left instead of vanishing. The stack-transition pose/membership is now computed before the visibility
  gate, letting the departing top render through its transition even after the relayout marks it hidden;
  Prev (whose incoming top becomes visible) already animated correctly.
- Stop reloading config from resetting an active cluster workspace to its default layout. The reload
  path now sets a one-shot `skip_next_cluster_relayout` flag that the next maintenance tick consumes,
  suppressing the tiling re-layout that normally fires on maintenance (preserving manual member
  positions) while leaving all other maintenance work and legitimate re-layouts (cluster entry, member
  add, overflow animations) unaffected.
- Capture real previews for fullscreen and game windows in Apogee and the Alt+Tab focus-cycle
  switcher instead of falling back to the app icon. The immersive fullscreen/game lock is released
  while Apogee is open and during an Alt+Tab cycle (the window is composited, off direct scanout),
  so its surface is sampleable; the per-node fullscreen skip has been dropped from the
  apogee/focus-cycle prewarm and preview paths.
- Deliver a press landing on an overflowing popup (e.g. a context menu spilling past its parent
  window) to the popup without also raising or focusing the toplevel beneath it. The window-side
  raise/drag/resize path is now skipped when a popup is under the pointer.
- Stop field hover-focus and window drags from yanking a soft-suspended fullscreen session (a game
  you alt+tabbed away from) back to fullscreen just by moving the pointer into its windowed area;
  only a deliberate click, alt+tab, or Apogee pick resumes it.
- Fix a back window's border bleeding over a front window when more than one window sits above a
  fullscreen surface. Above-fullscreen windows now render as atomic per-window stack units (content
  + border together, sorted by draw order) instead of a flat batched-content-then-batched-borders
  pass.
- Stop a minimizing/collapse-to-node window from flashing to the front: it now shrinks behind the
  live windows it was stacked under. Field node/core markers likewise draw beneath live windows so
  a collapsing window can't momentarily hide its target marker.
- Fix the first `Mod+Enter` in a freshly-created cluster doing nothing: the cluster-name prompt's
  confirm path trapped the key release but left the key in `intercepted_keys`, swallowing the next
  compositor keybind on the same key until a later release freed it.
- Fix the fullscreen toggle wedging a corrupt second session after a soft-suspend (alt-tab away):
  the toggle now uses the active-OR-suspended session predicate so it always exits a fullscreen
  you're in.
- Fix dangling, unexitable fullscreen state when a fullscreen cluster member is collapsed or
  minimized — fullscreen is now torn down before the workspace collapses.
- Fix a dropped tiled cluster member glitching into place: the cluster layout now owns the dropped
  window's final position (carry authority cleared before re-layout, release-position static lock
  skipped for tiled members), mirroring the keybind-move path.
- Fix a stale shrink-ghost texture lingering over the window that grows to fill a closed tiled
  cluster member's slot (most visible when the master closes) — the reflow now carries the
  transition.
- Fix the "zoom stretch" during a tiled cluster reflow: a size change is never deferred, so the
  offscreen cache rebuilds at the live buffer size and fills the new slot crisply, and a tile
  transition with no warm cache renders the live surface at the transition pose instead of leaving
  the cluster looking empty on open.
- Reject tiny or empty client cursor surfaces (e.g. a broken 1×1 `wl_surface` set as the pointer
  cursor) and fall back to the themed default pointer, eliminating a stray square cursor artifact
  most visible near terminal text inputs.
- Fix remaining flashes and abrupt size snaps during tiled cluster reflows (slave-to-master
  promotion, slave-close expansion) by routing all reflow visuals through the material-bridge
  handoff instead of sharing the tile animation track with real content. The handoff is a fully
  independent state — real window content is hidden entirely during travel and expand, and only
  appears via a non-linear crossfade during reveal, so stale client pixels are never scaled or
  snapped into the new slot.
- Fix hover-focus (`input.focus-mode "hover"`) being permanently disabled when a persistent
  layer-shell client such as Quickshell/Noctalia was running. Halley previously blocked
  hover-focus whenever *any* layer-shell surface held keyboard focus or was hit by the
  pointer, treating persistent shells the same as modal launchers. Layer surfaces are now
  classified: only modal roles (the Lift launcher, `KeyboardInteractivity::Exclusive`, session
  lock) block hover-focus; `OnDemand` persistent shells do not, so moving the mouse over
  windows correctly focuses them even while a panel or bar has keyboard focus.
- Fix GPU-accelerated Wayland clients (Quickshell, Qt/EGL apps) crashing under Halley with
  `EGL_BAD_DISPLAY` / `EGL_BAD_SURFACE` because the compositor never bound its EGL display
  to the Wayland display, so Mesa's EGL Wayland platform had no server-side `wl_drm`
  infrastructure. The TTY and winit backends now call `bind_wl_display` at startup.
- Make same-layer layer-shell placement deterministic: exclusive-zone surfaces (e.g. a
  reserved bar) now establish their reservation before non-exclusive decorative surfaces on
  the same layer, so split-shell UIs such as Quickshell place panels and decoration correctly
  instead of depending on incidental iterator order. Non-modal background layer surfaces also
  no longer swallow pointer focus meant for toplevels and the desktop. Thanks to
  @binarylinuxx (#148).
- Maximize and fullscreen camera zoom now eases on the same fixed `ease_in_out_cubic` as the
  window grow/shrink instead of the exponential zoom smoothing, whose asymptotic tail made
  the grow/shrink visibly "stick" near the end (worse the further the zoom had to settle).
- Fix exact-fullscreen rendering filling only the top-left of the output with black margins on
  entry from a zoomed-out camera (worst on XWayland/Steam, where buffer geometry also lags):
  once the grow animation ends the live buffer is scaled to fill the output directly, instead
  of via `visual_size * cam_scale` while the camera is still easing to 1.0.
- Closing a fullscreened cluster member (e.g. surface destroy) now restores the monitor
  camera target and re-lays out the cluster workspace, matching the `Mod+F` exit path.
  Previously the camera stayed anchored on the deleted node and the subsequent re-layout
  projected surviving members offscreen.
- Fix fullscreen games (Wine/Proton/gamescope) launched inside a cluster tiling workspace
  landing *outside* the cluster layout: a window that requests fullscreen before its app_id
  resolves or before it joins the cluster is now absorbed into the active cluster as a real
  tile/stack member first, so siblings are hidden, the camera grows from its slot, and
  exiting or closing returns cleanly to the cluster. Previously the fullscreen sat outside
  the cluster and on close `restore_cluster_workspace_after_fullscreen` found no membership,
  so the workspace got "stuck".
- Suppress client fullscreen re-requests after the user explicitly exits fullscreen via the
  `Mod+F` keybind on a cluster member. Games (gamescope, SDL, Wine) frequently call
  `xdg_toplevel::set_fullscreen` immediately after the compositor un-fullscreens them,
  trapping the window back in fullscreen and making the keybind feel like it did nothing.
  The block is cleared when the user re-enters fullscreen via the keybind, and expires when
  the node leaves the active cluster context, so only automatic re-requests are affected.
- Fix the `Mod+F` keybind requiring two presses to exit fullscreen after the first cycle:
  the re-request block was only installed for `ClientRequest`-origin sessions, so after
  re-entering via the keybind (which sets `UserKeybind` origin and clears the block) a
  subsequent game re-request slipped back in. The keybind exit now always installs the
  block for active cluster members regardless of session origin.
- A client `set_fullscreen` request no longer converts a user-owned (`Mod+F`) fullscreen
  session into a client-owned one: the origin is preserved so the compositor keeps
  authority over sessions the user initiated.
- Fix survivor tiles "sliding from the left and stopping partway" when a fullscreen cluster
  member exits or closes: the fullscreen camera restore animation (a viewport pan with a
  zoom track) kept running while the cluster re-laid out, projecting tiles against a moving
  camera. For active cluster members the camera now snaps synchronously to the restored
  target and any in-flight viewport pan for that monitor is cancelled before the reflow.
- Fix the cluster top gap growing larger on every fullscreen exit: a forced work-area
  refresh on fullscreen exit/drop rewrote the active cluster's frozen `usable_viewport`
  from the current camera base, compounding the reservation offset. The forced refresh has
  been removed from both the fullscreen exit and surface-destroy paths; the active cluster
  work-area lock now stays frozen through fullscreen for the whole session.
- Fix the `Mod+F` keybind sometimes acting on the wrong cluster member instead of the
  fullscreen session node: the toggle now resolves its target by checking the monitor's
  active and then suspended fullscreen node before falling back to normal focus or the
  fullscreen focus override, so a stale monitor focus (e.g. a chat window) can no longer
  intercept the keybind.
- Closing a fullscreened cluster member now uses cluster-aware close-restore (focus the next
  cluster member) instead of the non-cluster close-restore path that could restore focus to
  a field window and re-trigger a pan.
- Stale-surface reaping (Wine/Proton/gamescope crash paths) now tears down fullscreen state
  before removing the dead node, mirroring the normal destroy path. Previously a fullscreen
  cluster member killed via the stale-surface reaper left `fullscreen_active_node` stale,
  the camera anchored on the gone window, and the cluster siblings hidden.
- Remove temporary diagnostic logging from the cleanup and spawn-rule paths that was added
  during fullscreen-cluster debugging.
- Fix high idle CPU (~4%) in tiled cluster workspaces that never settled: the tile grow-wait
  hold track was endlessly refreshing its `started_at` on every maintenance relayout and
  counted as an animation-active track, forcing continuous vblank redraw. Fixed hold tracks
  (`from == to`) no longer count as animating and repeated identical holds preserve their
  original start time.
- Fix close-animation ghosts following the camera during pan. Ghosts are now world-anchored
  by capturing the camera center at animation start and offsetting baked screen geometry by
  the camera's screen-space displacement each frame.
- Fix keyboard spawn landing on the cursor/hover monitor instead of the focused monitor
  after a field-jump keybind. `primary_interaction_focus` now wins for keyboard launches,
  and `pending_spawn_monitor` survives focus churn until the new toplevel maps.
- Fix cursor hiding during zoom keybinds. Zoom is spatial navigation, not keyboard
  navigation, so the cursor now stays visible with directional zoom cursor icons.
- Fix zoom keybinds breaking mid-zoom monitor switching by using the current interaction
  monitor instead of forcing the stale focused monitor.
- Fix settled hover permanently forcing continuous vblank redraw. Raw `hover_node` and
  `overlay_hover_target` presence no longer pin an output as animation-active; only
  actively transitioning hover mix keeps redraw alive.
- Fix DPMS wake causing slow secondary monitor recovery from an aggressive `reset_buffers()`
  call. The buffer reset is removed; only stale composed-frame cache eviction remains.
- Fix animated background causing startup delay and high idle CPU by gating per-monitor
  with startup/DPMS grace periods and capping animated redraws to ~10 FPS.

## [v0.4.0] - 2026-06-12

### Added
- Add a `mod+f` keybind that toggles compositor-initiated fullscreen on the focused window, configurable via the `toggle-fullscreen` action keyword, with bootstrap backfill and example config coverage.
- Add `halley -h` / `halley --help` output documenting config selection and session startup options.
- Add `halley -c` / `halley --config` support for selecting an explicit config file, with CLI config taking precedence over `HALLEY_WL_CONFIG`.
- Add numeric `opacity` window-rule support using a `0.0` through `1.0` scale, applying matched opacity to window content, borders, shadows, popups, badges, close snapshots, and captures while blocking direct scanout for translucent windows.
- Add optional `width` and `height` window-rule keys for fixed initial sizes on matched windows.
- Add configurable fullscreen entry animation via `animations.fullscreen`, including bootstrap migration and example config coverage, so browser videos such as YouTube tween into fullscreen instead of snapping.
- Add an Aperture `Minimal` mode across IPC, compositor status, and the standalone clock so maximized windows and tiled cluster workspaces can use a compact top tab instead of the larger collapsed clock.
- Add `HALLEY_WL_PERF`-gated slow-frame and cluster-workspace entry timing logs for diagnosing render hitches without hot-path timestamp overhead when disabled.
- Add a `debug:` config section with `overlay-fps` and `show-ring-when-resizing` toggles, including a legible top-left FPS HUD and control over focus-ring config-change previews.
- Add Halley Lift, a standalone command palette for apps, nodes, clusters, actions, and config search, with mode prefixes, configurable UI, and cluster draft handoff support.
- Add a Halley Lift `terminal` config key for launching `.desktop` apps that require a terminal.
- Add first-class Gamescope integration through a top-level `gamescope:` config section, including global defaults, repeated per-game profiles, and per-game opt-outs, wired through `halleyctl gamescope run -- <command>` for use in Steam launch options.
- Add automatic Gamescope resolution selection from the selected Halley viewport (`monitor` selector `focused`/`cursor`/`primary`/connector), so matching games launch with monitor-sized output and game dimensions by default.
- Add clear diagnostics when Gamescope is enabled but the `gamescope` binary is unavailable, falling back to launching the game unwrapped instead of blocking it.

### Changed
- Keep `halley-session` as the recommended public full-session launcher while documenting `halley --session` as a session-wrapper, packager, and service-file flag.
- Resolve the effective Halley config path once at startup and reuse it for reload/watch behavior, with precedence of explicit config, `HALLEY_WL_CONFIG`, user config, system config, then generated user config/internal defaults.
- Freeze Aperture work-area updates for the whole field maximize session — through both the enter and restore animations — after applying the initial reservation baseline, matching cluster workspace behavior and avoiding mid-animation `usable_viewport` re-basing (and the un-maximize top-strip pop) on lower-refresh displays. The deferred-flush maintenance pass now only runs when a pending monitor is actually unlocked, so a locked session no longer re-runs the work-area refresh or invalidates the Aperture mode cache every frame.
- Resolve active-window render routing in `window::layout` with a `WindowRenderRoute` so surface collection appends shadows, borders, badges, surfaces, textures, and popups through a layout-provided route instead of repeating stack/top/fullscreen routing checks.
- Add focused `RenderState` accessors for tile animation state, overlay toast lookup, view-state retention, and render tick telemetry to reduce direct bucket access from frame, layout, cluster, overlay, and camera code.
- Move spawn reveal pan state and immediate activation paths behind named `SpawnRevealController` capability methods, reducing direct spawn-state manipulation in the reveal flow without changing placement behavior.
- Split toplevel-destroy surface lifecycle handling into focused fact collection, input/focus cleanup, close-restore planning, and restore application helpers while preserving existing teardown behavior.
- Split `RenderState` into cohesive view, overlay, window-animation, telemetry, cache, and GPU buckets so render state ownership better matches subsystem responsibilities.
- Extract frame-loop output activity and full-repaint decisions into a dedicated `frame_loop::activity` module so frame ticking, callbacks, and presentation feedback are separated from read-only redraw policy.
- Move the frame-loop module root from `frame_loop.rs` to `frame_loop/mod.rs`, keeping it colocated with its `frame_loop/` submodules.
- Replace direct `ctx.st` access in compositor context wrappers with named capability methods for spawn, surface lifecycle, layer shell, pointer, and fullscreen paths, narrowing context call sites ahead of deeper subsystem splits.
- Extract active-window stack and per-window render layout resolution behind a `window::layout` boundary so surface collection consumes named layout data instead of deriving stack, tiling, fullscreen, maximize, resize, and scale policy inline.
- Replace the active-window render collector's positional tuple with a named render plan so frame scene assembly depends on explicit window-layer fields instead of tuple ordering.
- Render minimal Aperture as a clipped top tab with smaller clock sizing and tab-specific padding, while preserving normal and collapsed Aperture presentation.
- Centralize animation offscreen prewarm requests so close, tile, stack, maximize, fullscreen, raise, active-transition, and slide animations can declare texture-cache needs through one path.
- Keep first-collapse marker rendering non-blocking by skipping cold app-icon lookup/raster/import during frame rendering and falling back until the icon cache is already warm.
- Use the reserved usable viewport for maximize targets and maximized visuals so top clearance reservations are honored consistently.
- Soften window shadows with a Gaussian/error-function falloff for a more natural shadow tail.
- Treat Gamescope-managed games (and `steam_app_*` windows) as contained sessions: while they hold a pointer lock/confine, Halley suppresses its own overlay reveals so desktop UI cannot pop over the game (config-gated via `gamescope.suppress-overlays`).
- Archive the experimental launcher prototype and remove its public IPC surface ahead of Lift work.

### Fixed
- Make Halley Lift startup and general search responsive by removing synchronous icon indexing, caching live IPC snapshots outside keystroke search, and precomputing app search text.
- Restore broader Halley Lift icon coverage with background indexing, support live provider prefixes such as `action open` without badges, show all apps for an empty Apps search, keep cluster draft staging explicit to `cluster` searches, and ellipsize overlong search text from the left so the latest input remains visible.
- Avoid spatial-camera input remapping for Gamescope-managed pointer surfaces (config-gated via `gamescope.bypass-spatial-camera`) so the nested game receives a 1:1 pointer mapping while normal output and buffer scale handling are preserved.
- Avoid auto-creating `~/.config/halley/halley.rune` when `/etc/halley/halley.rune` exists, preventing system configs from being shadowed on first startup.
- Treat empty or whitespace-only `HALLEY_WL_CONFIG` as unset.
- Snap `halley-aperture` transitions into Minimal mode immediately so maximize work-area reservation and Aperture layer size stay in sync.
- Recompute live window-rule opacity for already-open windows on config reload and title/app-id refreshes, without reapplying placement or cluster behavior.
- Keep maximized windows visually maximized while closing by preserving the maximize session through `xdg_toplevel.close`, capturing close animations from maximized geometry, and cleaning up maximize state after the surface is dropped.
- Skip close-restore panning while a maximize session is present on the monitor, avoiding unnecessary viewport movement when focus is restored during maximized flows.
- Make focus-cycle and trail navigation out of maximized or fullscreen sessions preserve the selected target's state: visible active windows are raised in place, offscreen active windows exit the presentation mode and pan to center, and collapsed nodes exit the presentation mode and center without uncollapsing.
- Restore async app icon loading for normal node markers so app icons can appear without depending on other overlays warming the icon cache first.
- Let Bearings clicks on collapsed cluster core chips focus and center the core like other bearing targets without opening the cluster workspace.
- Wait briefly for the close-animation capture before automatic active-to-node collapses, fixing the first overlapped auto-collapse snapping to a node while preserving immediate fallback for no-content windows.
- Reserve Aperture top clearance as a deficit against the user's configured field or tile gap instead of stacking extra padding on top of those gaps.
- Base Aperture clearance on the actual minimal tab height plus a small after-gap, reject placeholder or expanded Aperture heights, and avoid phantom top gaps when `halley-aperture` is not running.
- Refresh usable viewports when maximize, tiled cluster, layout mode, config, or Aperture sizing changes can affect the reserve, while avoiding unnecessary refreshes from irrelevant Aperture commits.
- Prewarm requested animation textures for detached, pending, or off-current-monitor windows instead of relying only on the opportunistic visible-active-window cache pass.
- Avoid relayouting active tiled cluster members while tile animations are in flight, preserving transition geometry until the animation completes.
- Keep tiled transition rendering on stale offscreen caches when fresh captures are deferred, avoiding blank frames during tile movement.
- Deduplicate repeated tiled `xdg_toplevel` configures during maintenance relayouts to reduce client lag and avoid serial churn crashes.
- Detach active cluster members from their source cluster when monitor-transfer drags move them away, so the source layout recalculates without the missing window.
- Absorb transferred standalone windows into the target monitor's active cluster layout by default, while keeping `cluster-participation "float"` and overlap-policy windows freely floating and resizable above the tiled cluster plane.
- Restore stacking-cluster drag/drop behavior so hit-testing selects the visual top card, stack card extraction stays reliable after layout updates, only the top card can be dragged out, in-stack drops snap back to the stack, outside drops detach or dissolve two-window stacks, and standalone windows dropped on an active stack rejoin at the top instead of floating over it.
- Apply `xdg_popup` reposition geometry before acknowledging reposition requests, fixing Steam dropdown menus that could appear at the parent window's top-left with stale popup placement.

## [v0.3.2] - 2026-05-31

### Fixed
- Clear pending initial reveal state for tiled cluster members once committed geometry arrives, preventing focused terminals in tiled clusters from keeping stale rendered textures while input continues to reach the client.

## [v0.3.1] - 2026-05-31

### Fixed
- Restore expanded-window and landmark transfer behavior during drag overlap resolution.
- Restore initial reveal geometry updates for fullscreen/maximize-like surfaces, fixing game reveal behavior.

## [v0.3.0] - 2026-05-30

### Added
- Add `wp_presentation` support and send presentation feedback after TTY and winit frames so Wayland clients such as gamescope can receive frame timing instead of falling back to X11 behavior.
- Add a global `placement:` config block for expanded-window spawn strategy, landmark placement behavior, and post-placement reveal settings, with generated defaults and example configs updated for bootstrap migration.
- Add `input.raise-on-click` so clicking a window can bring it forward independently from click/hover focus mode.
- Add a cursor redraw hook and targeted TTY output redraw requests so pointer-only motion can repaint the affected output instead of forcing every monitor through a redraw.
- Add fractional scale protocol support, including DPI-based output scale guesses and preferred scale updates for surfaces as they move between monitors.
- Add configurable Aperture placement for cursor-following, a fixed monitor, or every output, including per-output Aperture status IPC and CLI output.
- Add `aperture-peek` styling for corner, rounded background, radius, and clock appearance, plus an `examples/aperture.rune` sample config.
- Add user-pinned window/node/core support with default `mod+p`, `field.pins` badge styling, pinned Bearings visibility, and pin badge rendering from the bundled SVG asset.
- Add `field.pins.size` for scaling pin badges, with more padding between the pin glyph and circular badge background.
- Add `field.pins.background-colour` for configuring the circular pin badge background independently from the pin glyph colour.
- Add top-right config error overlays for startup, manual reload, IPC reload, and file-watch reload failures, including scrollable diagnostics, hover pause, right-click dismissal, wheel and shift-wheel scrolling, and configurable `overlays.error-colour` styling.
- Add strict config validation diagnostics for unknown Halley keys and invalid literals, with path, line, source text, and suggestions when available.
- Add a selectable `animations.window-close.style "fade"` close animation that fades captured closing windows without shrinking them.
- Add visual-only maximize/unmaximize animation using `animations.maximize`, preserving field geometry while tweening the presented rect.

### Changed
- Switch the field placement model so expanded windows may overlap other expanded windows while collapsed nodes and core landmarks remain non-overlapping map objects; pinned landmarks remain solid blockers during spawn, drag, and resize.
- Deprecate rule `overlap-policy` as a no-op during config migration; use `spawn-placement` for per-rule placement overrides and `cluster-participation "float"` for floating dialog behavior.
- Keep active pinned windows immovable, including drag/resize/maximize paths, while still allowing them to overlap other expanded windows; collapsed pinned nodes/cores remain non-overlappable solid landmarks.
- Remove the resize overlap overlay now that overlap is normal expanded-window behavior.
- Split the large overlay renderer module into focused banner, toast, focus-cycle, cluster-overflow, chip, action-row, hover-label, selection-marker, and text-helper modules while preserving the existing overlay API and behavior.
- Move TTY `wp_presentation` delivery to the DRM vblank completion path, carrying feedback as frame data and reporting `Vsync`, `HwCompletion`, and real `HwClock` timestamps when available.
- Expand TTY DRM compositor setup for stricter drivers by supporting `Xbgr8888`/`Abgr8888` scanout formats and retrying compositor creation with invalid modifiers when advertised modifiers fail; high-priority EGL remains an explicit `HALLEY_TTY_HIGH_PRIORITY_EGL=1` opt-in.
- Cache cursor sprites by theme, size, and icon so cursor changes avoid repeatedly reloading the same theme images.
- Rework Xwayland socket startup around event-loop socket watchers, safer listener handoff, close-on-exec lock files, `-listenfd` capability detection, and portal `DISPLAY` activation environment export.
- Rework `halley-aperture` standalone rendering to maintain per-output layer surfaces, redraw clocks on a timed Wayland poll loop, and keep animations advancing without busy sleeping.
- Treat pinning as a property of the active entity by transferring pinned state from windows into clusters and collapsed cluster cores, keeping pinned core visibility and IPC state consistent across create, absorb, collapse, expand, and dissolve flows.
- Move pure overlap contact physics into `halley-core` so the Wayland compositor only wires it into runtime state.
- Remove empty npm package manifests from the repository root.
- Rename the default explicit field-drag pointer action from `field-jump` to `pan-field`, keeping `field-jump` and `drag-pan` as config aliases for compatibility.
- Treat maximize and fullscreen as presentation states: they now preserve field geometry, do not shove other windows or pinned landmarks, and participate in normal focus/raise ordering so other windows can appear above them until the maximized/fullscreen window is explicitly raised again.

### Fixed
- Keep window borders at the same z-depth as their owning window so a background window's border cannot draw over the foreground window.
- Clamp dragged collapsed nodes/cores against expanded windows at the configured field gap and move trapped unpinned landmarks to the nearest free readable spot after overlap resolution.
- Keep hover focus from changing window stacking; only explicit raise events such as new active windows and click-to-raise alter overlap order.
- Treat trail navigation as an intentional selection and raise the selected active window.
- Make dragged collapsed nodes/cores slide along expanded-window edges and flip sides only after crossing the window midpoint instead of snapping into corners.
- Draw active-window pin badges with the owning window's z-order, matching borders instead of staying globally above overlapping windows.
- Preserve existing keyboard focus for overlay/popup text input instead of restoring last input focus on every unbound typing key.
- Keep maximized windows active when new or transferred windows overlap them, while allowing click raise, trail navigation, and focus cycling to bring maximized windows forward again through normal stacking.
- Preserve the original active-window position for delayed manual collapses so the first collapse over another window visibly slides the resulting node out from under the blocker.
- Apply the same collapsed-node placement and slide animation to automatic active-window-limit and focus-ring decay collapses.
- Let active-window-limit collapse enforcement run during visual active-transition animations so the first automatic collapse resolves without waiting for later pointer movement.
- Remove initial-spawn push-away authority so opening a new expanded window does not shove existing expanded windows out of the way.
- Limit new-window reveal panning to the one case where a pinned landmark blocks the current spawn center.
- Apply live config reloads directly and force active window render caches/full redraw after reload.
- Watch gathered config files for reloads, so saving files included with `gather` updates the live config.
- Fix Tiny Glade/native Wayland pointer-lock camera spins by avoiding fresh absolute pointer-motion refreshes while `new_constraint` is creating a lock, preserving the existing focus/location instead.
- Block interactive move and resize for fullscreen-like game surfaces, including output-covering borderless clients and active pointer-constrained surfaces, so games such as Tiny Glade cannot be compositor-resized while grabbed.
- Restore normal EGL priority as the default TTY GBM/GLES path after high-priority EGL caused AMD game flicker/stutter; keep the high-priority path available only through `HALLEY_TTY_HIGH_PRIORITY_EGL=1`.
- Reset stale default spawn anchors when a monitor is empty or the focused window has been panned out of the active spawn area, so new windows start at the current viewport center instead of continuing an old left/right pattern.
- Keep pan-away reset spawns centered against the current usable view once the view center leaves the focused window footprint, ignoring stale/off-center focus for fit and candidate generation while still avoiding windows in the current view.
- Preserve view-center reset placement through late app-id and real-size commits, fixing kitty-style terminals being shifted off-center after their final geometry arrives.
- Preserve no-anchor default/view-mode spawn placement through late size commits, covering intermittent terminal launches after spawn state has already switched to view anchoring.
- Ignore stale spawn focus overrides after manual pan-away unless the current view center is still over the override footprint, preventing terminals from being pulled back toward the last focused window.
- Avoid cursorless direct scanout for active fullscreen outputs that are waiting on frame callbacks, preventing fullscreen video from freezing when the cursor leaves or hides.
- Make hover-mode keybind/default spawns follow the pointer monitor even when that monitor already has windows, avoiding terminals opening on the stale focused monitor at edge positions.
- Latch the live pointer monitor for keyboard launch actions so stale pending spawn monitor state cannot route terminals to the previous output.
- Keep visible TTY clients with pending frame callbacks paced on refresh ticks so unfocused fullscreen video continues advancing frames.
- Restore direct game client cursor handoff by sending frame callbacks and presentation feedback to client cursor surfaces, refreshing pointer contents when the surface under the cursor changes, and falling through transparent helper hit nodes to the actual surface below.
- Keep Steam's built-in startup/login overlap behavior from leaking onto the main Steam client by expiring the startup rule once the surface no longer matches the login window.
- Route layer-shell commits through the monitor assigned to that layer surface so layer state updates no longer use the wrong active monitor context.
- Throttle TTY redraws and frame callbacks per output so fullscreen/video timer frames and cursor motion avoid unnecessary cross-monitor redraw work.
- Reduce per-output render work by filtering active surfaces before sorting/syncing them and scoping maximize animation redraws to the affected monitor.
- Use the launch or window-rule spawn target monitor for initial `xdg_toplevel` configure bounds so new clients receive bounds for the output they will actually open on.
- Prevent focus decay while spawn or open transitions are still active, avoiding premature collapse during window launch and reveal animations.
- Preserve fullscreen and pointer state across monitor changes by separating soft fullscreen suspension from client fullscreen exits, refreshing pointer constraints from the last screen position, releasing constraints when crossing monitors, and keeping cursor surface output state current.
- Smooth new-window reveal timing by waiting for committed geometry before starting open animations, preserving late rule rechecks, keeping offscreen spawn-panned windows detached until the pan reaches the reveal point, and scheduling delayed reveal timers.
- Make initial spawn placement anchor-aware once real window geometry is known, including row-aware vertical placement, conservative open-animation extents, and anchored overlap resolution that keeps the focused anchor fixed while the new spawn yields.
- Stabilize spawn patch placement by reanchoring patches on the focused window and resetting patch state when a monitor becomes empty.
- Respect late user window rules in deferred rule rechecks and avoid resolving overlap for windows still pending their initial reveal.
- Scope viewport pan animations to the monitor that created them so quick pointer movement to another output cannot retarget an in-flight spawn or close-restore pan and move the wrong monitor.
- Defer spawn-pan focus activation until the tick after a spawn reveal pan completes, while applying reveal state immediately so new windows are undetached, hot, transition-tracked, and recorded in focus history as soon as the reveal begins.
- Fix cross-monitor spawn reveal pans in click-to-focus mode by creating the pan from the spawn monitor's viewport center instead of whichever monitor was current when the reveal started.
- Add built-in handling for Steam's startup login window so it opens floating, centered, and allowed to overlap without applying the same rule to the main Steam client; defer Steam rule rechecks until late-arriving titles are available.
- Treat browser portal "Save Image" dialogs like other portal save dialogs so they open with the expected floating dialog behavior.
- Route clicks to the topmost nested popup, including layer-surface popups, so nested Firefox menus receive clicks before their parent menu items underneath.
- Stabilize locked and confined pointer constraints for fullscreen games by routing locked constraints through relative motion only, honoring confinement regions, redirecting focus to constrained ancestor or descendant surfaces, and preventing desktop hover/focus updates from also consuming constrained motion.
- Keep global monitor and viewport state synchronized with pointer movement across outputs so hit-testing, focus, buttons, and axis events use the monitor under the cursor before a click or focus change occurs.
- Improve fullscreen game cursor handling by applying cursor position hints internally, accounting for cursor-surface buffer deltas when computing hotspots, and falling back cleanly when client cursor surfaces are destroyed.
- Allow direct scanout with fullscreen client cursor surfaces or hidden 1x1 layer placeholders while still blocking scanout for real visible top and overlay layers.
- Uncollapse a noded surface before maximizing it, so maximize opens the window instead of resizing the collapsed node marker.
- Make close-focused target the currently focused item before surface history, silently closing every member of a focused collapsed cluster core without briefly revealing survivors or using stale cross-monitor fallback closes.
- Keep running with built-in defaults when startup config loading fails while surfacing the preserved diagnostic in the error overlay instead of silently discarding the failure.
- Keep focus-ring preview repainting while active so focus-ring size and offset reload changes are visible immediately.
- Preserve maximize sessions when a maximized window enters and exits XDG fullscreen, so fullscreen videos return to the still-maximized window instead of dropping maximize state while keeping maximized geometry.
- Make `input.focus-mode "hover"` focus collapsed cluster core nodes the same way it focuses regular windows and collapsed surface nodes.
- Prevent fullscreen/game surfaces from being collapsed into nodes by automatic decay, active-window-limit pruning, carry previews, or manual collapse toggles while the fullscreen session is active or suspended.
- Deep-merge unaliased `gather` config sections through `rune-cfg` 0.4.6 so gathered exact keys override local values without replacing unrelated local config keys.
- Render configured window shadows for maximized windows instead of suppressing them during maximize sessions.
- Treat XDG modal/transient dialogs as floating overlap windows by default, centering them over their parent window when available or the viewport otherwise.
- Keep fullscreen video timer frames from delaying ready pan/zoom redraws on other monitors by including animation-active outputs in tty timer redraw eligibility before servicing video scanout.
- Stop valid `xdg-activation` requests from revealing or panning existing windows, preventing Steam cover clicks from recentring the Steam window.
- Prevent close-focus restore from panning toward a fullscreen-displaced window while a fullscreen app is closing, so exiting games restores the previous window without flinging the viewport past it.
- Make `input.focus-mode "hover"` focus collapsed surface nodes when the pointer hovers them, so close-window actions target the hovered node instead of the previously focused window.
- Prevent fullscreen timer frames from advancing camera smoothing on monitors that are still pending presentation by queuing animation-active outputs before non-animation outputs and skipping shared camera-smoothing ticks from fullscreen/direct-scanout timer frames in that case.
- Fix zoom/pan progress being consumed by invisible fullscreen or game frames, which could cause the next visible frame to jump.
- Preserve the existing NVIDIA and direct-scanout behavior with no changes to direct scanout, `HALLEY_FORCE_COMPOSED`, `HALLEY_DISABLE_DIRECT_SCANOUT`, sync waits, or frame stats.
- Stop `move-window` from implicitly panning the field at monitor edges; `pan-field` keeps the old edge-pan window drag behavior, active drags can switch between the two modes as modifiers change, and empty-field left drag still pans the camera.
- Prevent right-click holds on empty field space from starting a camera pan.
- Make overlap-policy windows stack like normal windows, drawing and hit-testing the clicked or newest overlapped window on top while hover focus does not raise it.
- Animate unpinned collapsed nodes sliding out from under explicitly spawned or resized active windows while keeping logical overlap resolution immediate.
- Delay manual-collapse node slide-out until the captured close animation finishes, so noding an overlapped behind window visibly slides the collapsed marker out instead of snapping under the closing snapshot.

## [v0.2.0] - 2026-04-28

### Added
- Add compositor focus-cycle actions, default `Alt+Tab` and `Alt+Shift+Tab` bindings, and parser aliases for cycling focus forward and backward.
- Add a focus-cycle overlay switcher that previews candidate windows with app icons, monitor labels, and keyboard hints while the selection is active.
- Add a field-scoped maximize toggle bound by default to `mod+m`, plus dedicated `animations.maximize` config support across defaults, parsing, templates, and the example config.
- Add monitor-local maximize sessions that center and maximize the focused window, snapshot and restore displaced windows, and preserve pinned state while staying out of cluster workspaces and fullscreen sessions.
- Add per-monitor cluster slot actions for slots 1 through 10, default `mod+1..9` and `mod+0` binds, and parser support for remapping those actions through config.
- Add bootstrap config merging so existing user configs pick up newly introduced template sections, options, and default keybinds without overwriting custom `rules`, `env`, or `autostart` blocks.
- Add shipped Wayland session assets and a native `halley --session` entry path so SDDM and other display managers can launch Halley directly.
- Add configurable compositor-drawn shadows for windows, nodes, and overlays, including generated config defaults and parser support for per-layer blur, spread, offset, and color.

### Changed
- Keep focus-cycle state modal until the binding modifiers are released, then commit the selected window, restore or release immersive fullscreen state as needed, and recenter the pointer on the committed target.
- Render overlap-policy windows in a dedicated above-fullscreen bucket so they can draw and hit-test over fullscreen content without dropping out of fullscreen immediately.
- Anchor overlap-policy spawns on fullscreen monitors over the fullscreen target instead of using normal adjacent placement, while preserving stacked-layout behavior as the exception.
- Use camera smoothing for drag edge panning instead of snapping the viewport directly to the target each tick, so window drags stay visually smooth while the camera catches up.
- Scale drag edge-pan speed by zoom level and edge pressure so zoomed-in views move more deliberately without losing responsive edge scrolling.
- Smooth the monitor camera into 1.0 zoom on maximize and back to the saved zoom and center on restore, block pan and zoom while the maximize session is active, and make rapid re-toggle and cleanup behavior reliable.
- Keep maximize mode singular by disabling move, resize, and trail navigation for windows in an active maximize session.
- Unmaximize non-overlap field windows before spawning a new top-level on that monitor so the restored focused window becomes the deterministic spawn anchor, while overlap-rule windows continue opening without breaking maximize mode.
- Remember when the maximized target is intentionally collapsed into a node, restore the displaced windows immediately, and re-enter maximize when that same node is explicitly reopened.
- Let cluster slot actions pan to a target cluster core before opening it, collapse the current cluster before switching slots, and toggle the current slot back to a core when the same slot is activated twice.
- Tidy aperture module exports and config parse formatting without changing aperture behavior.
- Make collapsed surface nodes follow the same two-step click flow as core nodes, with matching pending-click, drag-cancel, and double-click promotion behavior.
- Simplify the display-manager startup path so the session launcher and user service both run `halley --session` directly instead of depending on the older wrapper chain.
- Refresh shipped config examples and bootstrap defaults with split-config `gather` guidance and the current window, node, and overlay shadow defaults.

### Fixed
- Recover native tty scanout immediately after DRM page-flip failures by clearing the affected DRM surface and forcing the next frame through a clean repaint path.
- Replace periodic tty DRM topology polling with udev-driven rescans so hotplug handling no longer stalls the compositor render loop.
- Pace tty vblank throttling from DRM event timestamps instead of wall-clock receipt time, avoiding false throttling after delayed or batched vblank delivery.
- Ensure windows with overlap rules steal open over fullscreen windows on the target monitor, including cases where the overlap rule is resolved later during deferred rule rechecks.
- Stop overlap-policy windows from forcing fullscreen apps windowed just because they open or take focus; only explicit focus-cycle switching now suspends the fullscreen lock.
- Block direct scanout only while a visible overlap-policy window is actually being drawn above the fullscreen app on that monitor.
- Keep zoom and pointer panning locked while a fullscreen lock is still active underneath overlap-policy windows, and only release those locks when the user explicitly switches away from fullscreen interaction.
- Base drag edge-pan timing on the active drag state instead of the last render tick, preventing inconsistent pan jumps when render timing varies.
- Make `input.focus-mode "hover"` treat the empty monitor under the pointer as the default spawn target for new windows, while keeping existing hover-to-focus behavior for windows under the cursor.
- Delay maximize teardown for deferred-rule toplevels until their final overlap policy is known, so overlap-rule windows no longer break maximize mode just because their rule resolved late.
- Fix display-manager launches so direct Halley sessions survive SDDM startup correctly and autostart commands can resolve user-bin apps such as `gessod`, `stasis`, and `halley-aperture`.
- Recover tty scanout when DRM vblank routing goes sideways by actually releasing recoverable pending outputs and timing out frames that never report completion.
- Wait for compositor frames that require explicit sync before queueing DRM work, reducing native tty stalls on stricter drivers.
- Reuse per-output composed textures and log EGL/GL renderer details during tty startup to reduce driver churn and make native rendering failures easier to diagnose.
- Preserve `gather` resolution when config files need Halley's inline keybind fallback, including recursively gathered files that also contain inline keybind blocks.
