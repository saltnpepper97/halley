# Changelog

All notable changes to this project will be documented in this file.

## [v0.5.0] - TBD

### Added
- Add `field.parallax` config for zoomed-out window-drag depth (`enabled`, `strength`, `tau-ms`),
  with eased parallax while moving windows and no background drift from ordinary cursor motion.
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
- Add a Halley Lift `term`/`/term`/`/t` search mode that runs the typed command line in the
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

### Changed
- Animate the Alt+Tab focus-cycle switcher with a quick open fade/scale and smooth carousel-style
  card motion between selections, while keeping the existing bounded snapshot prewarm behavior.
- Open Apogee on every active monitor at once, with each monitor showing only its own windows and
  cluster cores, and close all monitor views together when selecting a target.
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
- Restore the pre-fullscreen camera zoom/center on genuine fullscreen exit, and blend parallax back
  in during fullscreen and maximize exit animations to avoid a visual snap as windows return.
- Improve resize-by-border interaction with a minimum edge grab band, hover resize handles, and
  plain left-press edge resize/release behavior.
- Polish Apogee hovered live-preview feedback with an accent label and transparent focus ring.
- Cap gesture-driven Apogee open scrub speed so hard four-finger flicks still commit the overview
  but no longer visually snap the open animation faster than the configured interaction can read.
- Defer Apogee selection activation until the close animation finishes so the desktop doesn't
  mutate underneath the fading overlay. Maximized and fullscreen windows no longer flash or
  displace when selected from the overview: the close animation flies back to the actual
  presentation visual rect instead of the stale windowed field position.

### Fixed
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
- Keep same-monitor drag parallax held after releasing a moved window; only disabled/Apogee/cross-monitor
  cases return the temporary parallax offset home.
- Preserve the released window's visual position when ending a drag under held parallax, avoiding a
  small snap as the dragged window rejoins the parallaxed field.
- Preserve resized windows' visual position when releasing under held parallax, and use the
  parallax-adjusted position for close-animation captures and collapsed-node hit testing.
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
- Add Halley Lift, a standalone command palette for apps, nodes, clusters, actions, and config search, with slash modes, configurable UI, and cluster draft handoff support.
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
- Restore broader Halley Lift icon coverage with background indexing, support live provider prefixes such as `action open` without badges, show all apps for an empty Apps search, keep cluster draft staging explicit to `cluster`/`/cluster` searches, and ellipsize overlong search text from the left so the latest input remains visible.
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
