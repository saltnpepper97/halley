# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Add user-pinned window/node/core support with default `mod+p`, `field.pins` badge styling, pinned Bearings visibility, and pin badge rendering from the bundled SVG asset.
- Add `field.pins.size` for scaling pin badges, with more padding between the pin glyph and circular badge background.

### Changed
- Treat pinning as a property of the active entity by transferring pinned state from windows into clusters and collapsed cluster cores, keeping pinned core visibility and IPC state consistent across create, absorb, collapse, expand, and dissolve flows.

### Fixed
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
