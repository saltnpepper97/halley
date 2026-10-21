# Nested fullscreen regression tests

The two-output Alt+Tab suite runs real Wayland and XWayland clients against the
production session, keyboard handler, switcher, fullscreen policy and renderer.
A private harness-free server keeps the window/event loop on its main thread.
It exposes the usual nested output plus a second offscreen logical output.

Build the server and use the executable path Cargo reports:

```sh
cargo test --locked --test nested_fullscreen_server --no-run
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect fullscreen_multi_output \
  -- --ignored --test-threads=1
```

This requires a running Wayland desktop, EGL and XWayland. The server is a test
executable, not `target/release/halley`. Its second output and private input socket
are compiled under `cfg(test)` and are absent from normal compositor builds.
Running ordinary workspace tests does not launch this server.

The three tests cover twelve scenarios across both monitor directions:

- Ordinary native apps with client, compositor and overlapping fullscreen ownership.
- Native games using the normal game-rule classification.
- Ordinary X11 apps and Steam-identified game windows.

Every scenario sends actual Alt/Tab/Escape press and release events through the
production input handler. It checks preview and cancellation, release-to-commit,
return navigation, and a same-monitor control (ordinary apps exit; games park).
Assertions cover client fullscreen protocol state and dimensions, keyboard-focus
selection, fullscreen camera ownership, exact presented bounds, and captured
pixels at the output corners and center. Native configure/commit handshakes are
pumped explicitly instead of relying on fixed startup sleeps.

The private input socket observes switcher and presentation state but cannot
select or commit a window directly. Host pointer input is ignored in this isolated
server so physical desktop activity cannot steer the test. Each server and its
XWayland child are owned by its fixture. Diagnostics stay in its temporary
`halley-xwayland-disconnect-*` directory on failure; successful runs remove it.

These are two-logical-output integration tests. They do not validate physical
monitor modesetting, DRM scanout or real game-engine input behavior.

## Fullscreen and maximize landmark stability

```sh
cargo test --locked --test xwayland_disconnect landmark_presentation \
  -- --ignored --test-threads=1
```

These tests require a Wayland desktop and XWayland. Twelve scenarios place a
collapsed node near a real protocol window, then verify its Field position
through fullscreen or maximize entry, repeated surface commits, and exit.
Coverage includes native client/compositor/overlapping fullscreen ownership,
maximize, game rules, and X11 EWMH fullscreen/maximize with ordinary and Steam
game classes. Native compositor fullscreen and maximize also delay the restored
buffer after acknowledging the exit configure. Node placement commands remain
usable underneath the presentation, and normal Field client resizing must
still displace the nearby landmark after every scenario.

Captured pixels must also show the opaque fullscreen/maximize client covering
the node, including when the node's remembered stack slot exceeds the remaining
live window slots. Native cases send a real client pointer-warp request into
the covered marker area and verify the client receives the requested position
without losing pointer focus.
