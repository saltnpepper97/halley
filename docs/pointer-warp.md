# Pointer warp testing

`pointer-warp-v1` lets a native Wayland client request a cursor move within its
pointer-focused surface. Halley maps the requested surface coordinates through
its current presentation, including display scale and Field zoom. It preserves
pointer locks and confines accepted positions to active confinement regions.
See [protocol support](wayland-protocols.md) for the request policy.

Build the small manual client with:

```sh
cargo build --release --locked --example pointer-warp
./target/release/examples/pointer-warp
```

For local testing installs, the same client is available as
`halley-pointer-warp-test`. Start the newly installed compositor first; replacing
the binary does not change an existing session.

1. Move the cursor into the blue window and left click. The cursor should jump
   to one of the two crosses, alternating on subsequent clicks. The title
   changes from “Warp requested” to “Warp received” when the client receives
   the expected coordinates. This also exercises an implicit click grab.
2. Zoom or pan the Field, then click again. The cursor should land on the
   displayed cross rather than its old screen position.
3. Right click inside the window to toggle client fullscreen, and repeat.
4. Move the window between outputs with different scales and repeat. Pointer
   clicks and target placement should continue to agree.
5. Close the test window normally to exit. It changes no persistent settings
   and writes no diagnostic files.

The automated nested checks use a disposable compositor and cover fractional
scale/fullscreen coordinates, stale serials and invalid/unfocused targets,
subsurface offsets, shaped input regions, lock/confinement, collapse, and
session lock. Native multi-monitor and Field interaction checks remain manual.
