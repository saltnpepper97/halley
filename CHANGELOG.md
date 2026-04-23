# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Add compositor focus-cycle actions, default `Alt+Tab` and `Alt+Shift+Tab` bindings, and parser aliases for cycling focus forward and backward.
- Add a focus-cycle overlay switcher that previews candidate windows with app icons, monitor labels, and keyboard hints while the selection is active.
- Add a field-scoped maximize toggle bound by default to `mod+m`, plus dedicated `animations.maximize` config support across defaults, parsing, templates, and the example config.
- Add monitor-local maximize sessions that center and maximize the focused window, snapshot and restore displaced windows, and preserve pinned state while staying out of cluster workspaces and fullscreen sessions.
- Add per-monitor cluster slot actions for slots 1 through 10, default `mod+1..9` and `mod+0` binds, and parser support for remapping those actions through config.
- Add bootstrap config merging so existing user configs pick up newly introduced template sections, options, and default keybinds without overwriting custom `rules`, `env`, or `autostart` blocks.
- Add shipped Wayland session assets and a native `halley --session` entry path so SDDM and other display managers can launch Halley directly.

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

### Fixed
- Ensure windows with overlap rules steal open over fullscreen windows on the target monitor, including cases where the overlap rule is resolved later during deferred rule rechecks.
- Stop overlap-policy windows from forcing fullscreen apps windowed just because they open or take focus; only explicit focus-cycle switching now suspends the fullscreen lock.
- Block direct scanout only while a visible overlap-policy window is actually being drawn above the fullscreen app on that monitor.
- Keep zoom and pointer panning locked while a fullscreen lock is still active underneath overlap-policy windows, and only release those locks when the user explicitly switches away from fullscreen interaction.
- Base drag edge-pan timing on the active drag state instead of the last render tick, preventing inconsistent pan jumps when render timing varies.
- Make `input.focus-mode "hover"` treat the empty monitor under the pointer as the default spawn target for new windows, while keeping existing hover-to-focus behavior for windows under the cursor.
- Delay maximize teardown for deferred-rule toplevels until their final overlap policy is known, so overlap-rule windows no longer break maximize mode just because their rule resolved late.
- Fix display-manager launches so direct Halley sessions survive SDDM startup correctly and autostart commands can resolve user-bin apps such as `gessod`, `stasis`, and `halley-aperture`.
