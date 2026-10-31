# Testing

## Continuous integration

Three GitHub Actions workflows run on pull requests, pushes to `main` and
`dev`, and manual dispatch: [Rust](../.github/workflows/rust.yml),
[Nix](../.github/workflows/nix.yml), and
[XWayland](../.github/workflows/xwayland.yml).
Feature-branch pushes are checked through their pull request rather than
starting a duplicate set of workflow runs. A branch without a pull request
can be checked with manual dispatch. Each job uses a fresh GitHub-hosted VM.
The Ubuntu jobs use 26.04 because the pinned Smithay needs libdisplay-info 0.3.

The checks cover:

- Rust formatting and GitHub Actions workflow linting.
- Strict Clippy across the workspace, all targets and all features, plus Rustdoc
  with warnings treated as errors.
- Workspace tests, strict Clippy for every target, release builds and binary smoke
  tests with default features, no default features, and the documented dinit
  desktop feature set.
- `nix flake check` on native x86_64 and aarch64 runners, including the package,
  its workspace tests, installed resources and NixOS module assertions.
- Seven nested XWayland regressions covering disconnect cleanup, dialog focus
  and stacking, offscreen pointer capture, fullscreen repainting and geometry,
  and minimize/remap. They run sequentially under headless Weston and Mesa
  llvmpipe, on a private D-Bus session, without a physical display or GPU.

Cargo commands use `--locked`. Nix uses the committed `flake.lock` and rejects
lock-file updates. Rust checks use Rust 1.97.1, matching the current development
baseline; toolchain upgrades are explicit. Cargo caches are separated by
job and feature set; only `main` and `dev` save caches. Jobs have read-only
repository access, pinned external actions, timeouts, and cancellation when a
new run supersedes one on the same branch or pull request. Dependabot checks
action pins weekly.

To run the ordinary gates locally with the required native libraries installed:

```sh
rustup toolchain install 1.97.1 --profile minimal --component clippy,rustfmt
cargo +1.97.1 fmt --all -- --check
cargo +1.97.1 clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo +1.97.1 doc --locked --workspace --all-features --no-deps
cargo +1.97.1 test --locked --workspace
cargo +1.97.1 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.97.1 build --locked --release --workspace
```

Repeat the last three commands with `--no-default-features` for the minimal
build, or with `--no-default-features --features dbus,dinit,xwayland,winit` for
dinit (put Cargo flags before Clippy's `-- -D warnings`). On Ubuntu 26.04,
`bash .github/scripts/install-dependencies.sh` installs
the native build and test packages. On a Nix-enabled host, run
`nix flake check --no-update-lock-file --print-build-logs`.

Rust and Nix run the non-ignored tests. XWayland explicitly selects seven ignored
tests; other display-dependent tests and subprocess helpers remain ignored
unless invoked by their fixtures. To reproduce the headless suite on Ubuntu
26.04, install `weston xwayland libgl1-mesa-dri libegl-mesa0 fonts-dejavu-core`
alongside the build dependencies, select Rust 1.97.1, then run:

```sh
sudo install -d -m 1777 /tmp/.X11-unix
dbus-run-session -- bash .github/scripts/test-xwayland.sh
```

The script checks that every selected test exists, discovers the private server
executable through Cargo, and owns its temporary Weston process and sockets.
On failure, CI retains Weston logs, test output and failed fixture directories
as `xwayland-diagnostics` for seven days.

The remaining nested tests below need a Wayland session, EGL and their listed
tools. Physical GPU, DRM/KMS, multi-monitor and real-application behavior still
need separate live testing. These workflows do not publish or install Halley on
your machine.

## Orca accessibility

With Orca, Python GI/AT-SPI, XWayland and a running Wayland desktop installed,
build `nested_fullscreen_server` as below and build `halleyctl` with
`cargo build --locked -p halley-cli`. Then run:

```sh
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  python tests/support/orca_nested.py
```

The script starts a nested compositor on private D-Bus and AT-SPI buses. It
checks the spoken basics card, screenshot menu selection and configuration
error, rejects an
unapproved keyboard-monitor client, and restarts the supervised Orca process.
If `kitty` is installed, it also opens two nested windows and checks spoken
Apogee selection changes.
It does not publish the nested compositor to the host accessibility bus.
The ignored `accessible_capture` tests in `xwayland_disconnect` exercise
keyboard monitor selection and screenshot-region movement in a nested session.

## Fullscreen and Alt+Tab

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

## Offscreen pointer capture

```sh
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect offscreen_pointer \
  -- --ignored --test-threads=1
```

Build the private server as described above. These tests place a real client
partially offscreen with its centre outside the output. Native pointer-lock
release and recapture run repeatedly at normal and fractional output scales,
checking that the compositor cursor stays on an output, the lock retains its
anchor, and relative input continues without absolute motion. Confinement
checks preserve its visible intersection and release a wholly hidden region.
A real XWayland client repeatedly grabs and recentres the pointer while fresh
client colours continue appearing in output captures. Host input remains
isolated. Actual Minecraft capture and native DRM behaviour require separate
testing in the installed compositor.

## X11 fullscreen video-mode changes

```sh
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect x11_fullscreen_negotiation \
  -- --ignored --test-threads=1
```

The suite uses real X11 requests, raw key/pointer events, and captured pixels.
Game rules and ordinary-app controls cover:

- Output-sized fullscreen geometry, same-size transitions, and partial repaints.
- Smooth fullscreen motion with live game frames and ordinary-app crossfades.
- Game mode changes after Mod+F, unchanged-mode replies, consistent EWMH
  state/frame extents, and cleanup when an XID is unmapped and reused.
- Mod+F clears the fullscreen protocol state and restores the client's windowed
  size. Later client mode requests remain authoritative, without a persistent
  compositor override or a separate scaled-buffer window mode.
- Ordinary windowed game frames, titlebar close, and minimize/restore through
  client unmap/remap cycles.
- Windowed-size requests sent immediately with fullscreen exit.
- Cross-monitor hover or Alt+Tab focus followed by a game fullscreen-exit
  request, selected-monitor bookkeeping without physical mouse movement,
  fullscreen return, focused mode changes, and ordinary-app behavior.
- Centered placement of the first windowed frame and expiry of restore centering.

These clients use server-rendered pixels. Vulkan swapchains, launcher handoffs,
focus-loss reactions, and actual game display menus still require live testing
in the installed compositor. See the [XWayland fullscreen policy](xwayland.md#fullscreen-and-game-mode-changes).

## Alt+Tab target visibility

```sh
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect alt_tab_visibility \
  -- --ignored --test-threads=1
```

The suite exercises raw Alt/Tab/release input with native and XWayland clients.
Fully visible and exact-edge targets must keep their position and camera,
retaining client/compositor/overlapping fullscreen and maximize. Partial and
fully offscreen targets must be revealed, releasing ordinary presentation or
parking fullscreen games. Checks cover protocol state, camera state, client
bounds, preview cancellation and captured foreground pixels. Ordinary Field
targets, collapsed-window restoration, and animated fullscreen/maximize exits
are covered too. A camera regression verifies queued reveals wait for maximize
exit and preserve the restored zoom. The canonical intended behavior is
documented in the [Alt+Tab visibility policy](apogee.md#alttab-visibility-and-presentation-policy).

## Resizing before a client titlebar drag

```sh
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect resize_drag \
  -- --ignored --test-threads=1
```

Build the private server as described above. The real-application test requires
GTK4 Zathura and a PDF plugin. It opens a generated PDF with isolated config,
data and cache directories, resizes through real F8/F9 keybindings and a raw
pointer corner drag, then drags the client-side titlebar after each resize.
The client must keep the resized dimensions through move activation, release
and delayed buffer commits. Test-only motion/button commands use production
pointer routing and grab handling; host pointer input stays isolated.

A protocol client also checks the required configure acknowledgement for a
redundant `unset_fullscreen`, both after a resize commit and while a newer
resize configure is still unacknowledged, and verifies that maximize is
preserved. Neither request may replace the current size with the client's
unconstrained preferred size.

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

## Keyboard lock indicators

```sh
cargo test --locked keyboard_leds
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect keyboard_leds \
  -- --ignored --test-threads=1
```

Build the private nested server as described above. Unit tests exercise LED
broadcasts to multiple keyboard sinks, hotplug initialization, duplicate-device
notifications, removal and reconnect. The nested test sends raw lock and
ordinary key events through production input handling and observes the backend
LED callback, including a keymap reload while Caps Lock and Num Lock remain
enabled without a focused window. Focused-client tests cover all four lock
combinations through layout, variant and option changes, held Shift, rejected
XKB layouts, Wayland modifier events and XWayland key-event flags. The XWayland
test toggles lock keys while the X11 client is focused, then verifies that key
flags and lock state survive reloads. An ordering test checks that physical
keys precede the modifier updates they cause, and an authorized virtual-keyboard
client verifies that independent modifier requests still arrive immediately
before subsequent keys. Only the disposable server grants virtual-keyboard
access to the exact test executable. The backend records indicators without
writing to host keyboards.

Physical validation requires a native Halley session: check lock indicators on
the laptop and an external keyboard, attach another keyboard while a lock is
enabled, and verify indicators after VT switching, suspend/resume and a keyboard
layout reload. No new configuration is required. These checks do not establish
game-specific Caps Lock behavior under Wine or Proton.

## Overlapping physical keyboard keys

```sh
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/the/reported/executable \
  cargo test --locked --test xwayland_disconnect multi_keyboard \
  -- --ignored --test-threads=1
```

The private server can model libinput's per-key holder counts and applies the
same first-press/final-release filter as the native backend. Tests observe real
Wayland key events and compositor state for both sides of Shift, Ctrl, Alt and
Super, an ordinary letter, overlapping Alt+Tab and lock keys. Multiple keyboards
holding the same key must produce one logical press and one final release;
partial releases must preserve sided modifiers and keep the switcher open.
An XWayland client also verifies that letters retain Shift after a partial
release and lose it after the final release. Ordinary winit input retains the
host's existing keyboard stream. These nested checks model holder counts;
they do not operate host keyboards.

For physical validation after starting the installed native compositor, hold
the same Shift key on two keyboards, release either one, and type a letter. It
must remain shifted until both release. Repeat with Ctrl and an ordinary key,
and check that releasing one of two held Alt keys does not commit Alt+Tab.
