# Explicit-sync hardware testing

This test covers [issue #264](https://github.com/saltnpepper97/halley/issues/264).
The reporter sees Smudgy's entire window flicker when a custom GPU widget is
visible, including when unrelated windows update. They report Intel Arc integrated
graphics, 3120×2080 at 120 Hz, scale 1.5 (also affected at 1.0), and VRR off.
They report that removing the explicit-sync global stops the flickering.
The exact widget and Smudgy version have not yet been confirmed.

The fix retains outgoing explicit-sync buffer references until earlier GLES
reads finish, so Smithay cannot signal their release points while Halley still
reads their contents. This corrects a lifetime gap found in the code; it is
not yet a confirmed diagnosis of the Intel report.

## Run the test

Use the native session instructions in the test archive's `README.txt`.
The archive needs compatible host shared libraries. For NixOS, use the
commit-pinned Nix command in that file instead. The reporter has public NixOS
configuration, but their current distribution is not specified in the issue.

1. Check `BUILD-INFO.txt` and verify the executable running in the test session
   comes from the extracted archive (or the selected Nix result).
2. Keep explicit sync enabled. A nested `--winit` session cannot test this
   protocol because it does not advertise `wp_linux_drm_syncobj_manager_v1`.
3. Open the same Smudgy version and reproduce the exact affected widget.
   Smudgy 0.5.8 also has an animated CRT cat on its **No active sessions** screen.
   It is a custom iced/wgpu shader and needs no server connection or account,
   but it has not been confirmed as the reporter's widget.
4. Observe Smudgy during its own updates, then while typing or scrolling in
   another window. Compare with installed Halley 0.9.1 using the same settings.
5. Report the commit, GPU and driver/Mesa version, Smudgy version, exact steps,
   display mode/scale/VRR, and whether the whole window flickers. Include
   `$XDG_RUNTIME_DIR/halley-session.log` if available.

Log out and select the normal installed session after testing. No replacement
of the system compositor, desktop entry or system configuration is needed.

## Observed AMD result

Before applying the patch, Smudgy 0.5.8's official x86_64 release executable was
extracted into a temporary folder and launched natively with an isolated profile.
It selected Vulkan on an AMD Radeon RX 7900 XTX using RADV/Mesa 26.2.6. Its
Wayland trace contained acquire and release point requests on the advertised
explicit-sync protocol. The desktop ran Halley 0.9.1, DP-1 at 2560×1440/~180 Hz,
scale 1, VRR off.

Dustin visually checked the animated cat and reported no visible flickering.
This is a clean result for that AMD case, not evidence that the Intel report
is resolved or that every Smudgy custom widget has been exercised. A sampled
capture overlapped window movement and was not used as quantitative evidence.

The inspected local Niri 26.4.0 source does not advertise the DRM syncobj
protocol and uses implicit DMA-BUF readiness blockers. The local Hyprland
source tracks buffer references and attaches GPU-completion fences to explicit
release points. These are source comparisons, not additional live reproductions.
