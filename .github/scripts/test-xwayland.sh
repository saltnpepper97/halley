#!/usr/bin/env bash
set -euo pipefail

# A private display and software renderer keep this off the user's desktop/GPU.
artifacts="$PWD/target/ci-xwayland"
mkdir -p "$artifacts/tmp"
# Unix socket paths are short: keep fixtures out of long checkout paths.
scratch=$(mktemp -d /tmp/hci.XXXXXX)
export TMPDIR="$scratch"
export XDG_RUNTIME_DIR="$scratch/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
export WAYLAND_DISPLAY=wayland-ci
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER=llvmpipe
unset DISPLAY WAYLAND_SOCKET
weston_pid=''

cleanup() {
    status=$?
    if [[ -n "$weston_pid" ]]; then
        kill "$weston_pid" 2>/dev/null || true
        wait "$weston_pid" 2>/dev/null || true
    fi
    if (( status != 0 )); then
        mv "$scratch" "$artifacts/tmp/" || true
    else
        rm -rf "$scratch"
    fi
}
trap cleanup EXIT

# Cargo reports the hashed executable; do not guess a target/debug/deps path.
export HALLEY_TEST_WINIT_SERVER
HALLEY_TEST_WINIT_SERVER=$(
    cargo test --locked --test nested_fullscreen_server --no-run --message-format=json |
        python3 -c '
import json, sys
paths = [m["executable"] for line in sys.stdin if (m := json.loads(line)).get("reason") == "compiler-artifact" and m["target"]["name"] == "nested_fullscreen_server" and m.get("executable")]
if len(paths) != 1:
    sys.exit("Cargo did not report exactly one nested test server")
print(paths[0])
'
)
test -x "$HALLEY_TEST_WINIT_SERVER"
cargo test --locked --test xwayland_disconnect --no-run

weston --backend=headless --renderer=gl --no-config --idle-time=0 \
    --width=1280 --height=720 --socket="$WAYLAND_DISPLAY" \
    --log="$artifacts/weston.log" > "$artifacts/weston-console.log" 2>&1 &
weston_pid=$!
for ((attempt = 0; attempt < 200; attempt++)); do
    if [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then
        break
    fi
    if ! kill -0 "$weston_pid" 2>/dev/null; then
        cat "$artifacts/weston.log" "$artifacts/weston-console.log"
        exit 1
    fi
    sleep 0.05
done
test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"

# Keep an explicit, finite suite. Manual previews and toolkit tests stay manual.
cases=(
    closing_x11_dialog_returns_real_x_focus_to_parent_before_mru_window
    disconnect_removes_cluster_and_collapsed_x11_windows_preserving_native_windows
    parent_dialog_stacking_survives_focus_and_late_x11_parent_changes
    offscreen_pointer::offscreen_x11_capture_and_recenter_preserve_input_and_live_frames
    x11_fullscreen_negotiation::fullscreen_keeps_its_geometry_when_a_client_requests_an_old_video_mode
    x11_fullscreen_negotiation::fullscreen_off_on_with_the_same_buffer_does_not_freeze_partial_repaints
    x11_fullscreen_negotiation::windowed_game_has_an_ordinary_frame_and_survives_minimize_remap
)
available=$(cargo test --locked --test xwayland_disconnect -- --ignored --list)
for name in "${cases[@]}"; do
    # A renamed or missing test must fail instead of silently running zero tests.
    grep -Fxq "$name: test" <<< "$available"
    cargo test --locked --test xwayland_disconnect "$name" \
        -- --ignored --exact --test-threads=1 --nocapture 2>&1 |
        tee -a "$artifacts/tests.log"
done
