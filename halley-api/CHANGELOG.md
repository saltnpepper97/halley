# Changelog

Versions below refer to the SDK package, independently of the compositor.
Historical entries are backfilled from the package manifest and source history.

## [Unreleased]

## [0.6.0] - 2026-11-09

### Added
- Optional `wayland-client` and `wayland-server` bindings for
  `halley-surface-dismissal-v1`. Applications can watch their own layer-shell
  surface for a one-shot outside left press, without consuming input or exposing
  global pointer coordinates. Clients own their dismissal policy.
- Opt-in `test-support` socket fixtures using SDK request, snapshot, event, and
  error types. Consumer tests no longer need the private IPC codec or version.

### Fixed
- Connecting to an explicit socket no longer requires `XDG_RUNTIME_DIR` to be
  set. Default socket discovery is performed only when no path is supplied.

Existing socket operations and public semantic API version 1 are unchanged.

## [0.5.0] - 2026-11-01

- Update the SDK's private IPC dependency to 0.8.0 for Halley 0.9 snapshots.

## [0.4.0] - 2026-10-05

- Version the SDK prerequisites for independently packaged ecosystem clients.
- Use private IPC 0.7.0 behind the public SDK boundary.

## [0.3.0] - 2026-08-26

- Advance the SDK package alongside the configuration and compositor updates.
  The public semantic contract remains version 1.

## [0.1.0] - 2026-08-21

- Introduce the typed Rust SDK, persistent command connections, semantic node
  and cluster selectors, structured errors, and sequenced state subscriptions.
- Move `halleyctl` to the public SDK and document external integrations.
