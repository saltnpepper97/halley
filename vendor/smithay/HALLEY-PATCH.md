# Halley Smithay snapshot

This directory contains the Smithay crate at revision
`79bbed5e1199090d787115614847a79c76607181` from
https://github.com/Smithay/smithay, licensed under MIT (see `LICENSE.txt`).
The upstream workspace members and examples are omitted. `smithay-drm-extras`
remains pinned to the same upstream revision separately.

Halley's patch is limited to the input-method and text-input implementation:

- Suspend IME activation, text input and effective keyboard grabs while the
  compositor is locked, preserving the connection for recovery.
- Buffer composition until commit, clear pending state at focus and lock
  transitions, and reject serials from before the post-unlock activation.
- Keep duplicate IME instances inert and release only the keyboard grab owned
  by the destroyed object.

The compositor activates the suspension before giving focus to a lock surface
and lifts it only after unlock. Socket tests are in
`tests/text_input_protocol.rs`. When updating Smithay, re-evaluate these changes
against upstream and retain the lock isolation tests.
