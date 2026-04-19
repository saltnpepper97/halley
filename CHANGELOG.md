# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Add compositor focus-cycle actions, default `Alt+Tab` and `Alt+Shift+Tab` bindings, and parser aliases for cycling focus forward and backward.
- Add a focus-cycle overlay switcher that previews candidate windows with app icons, monitor labels, and keyboard hints while the selection is active.

### Changed
- Keep focus-cycle state modal until the binding modifiers are released, then commit the selected window, restore or release immersive fullscreen state as needed, and recenter the pointer on the committed target.
- Render overlap-policy windows in a dedicated above-fullscreen bucket so they can draw and hit-test over fullscreen content without dropping out of fullscreen immediately.
- Anchor overlap-policy spawns on fullscreen monitors over the fullscreen target instead of using normal adjacent placement, while preserving stacked-layout behavior as the exception.

### Fixed
- Ensure windows with overlap rules steal open over fullscreen windows on the target monitor, including cases where the overlap rule is resolved later during deferred rule rechecks.
- Stop overlap-policy windows from forcing fullscreen apps windowed just because they open or take focus; only explicit focus-cycle switching now suspends the fullscreen lock.
- Block direct scanout only while a visible overlap-policy window is actually being drawn above the fullscreen app on that monitor.
- Keep zoom and pointer panning locked while a fullscreen lock is still active underneath overlap-policy windows, and only release those locks when the user explicitly switches away from fullscreen interaction.
