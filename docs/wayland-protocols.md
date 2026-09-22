# Wayland protocol support

Halley advertises `zwp_text_input_manager_v3` version 1 and
`zwp_input_method_manager_v2` version 1 (`input-method-unstable-v2`). Native Wayland clients bind
text-input to send surrounding text and receive preedit and committed
composition. Input-method is restricted to ordinary compositor clients
(the same `ClientState` filter as virtual-keyboard), so fcitx and ibus
can attach as the IME while XWayland cannot. Composition follows keyboard
focus: a `WlSurface` enter/leave updates text-input automatically, IME
candidate windows track the focused parent through the existing popup
tree, and an IME keyboard grab is not replaced by an xdg-popup grab.
X11 applications keep using X11 IME and do not participate in this pair.
This is interface version 1 of the v3 protocol; the newer interface-version-2
requests and events are not advertised.

Halley's vendored Smithay buffers IME edits until commit, uses each text-input
object's commit count for `done`, resets pending state on enable and focus loss,
and rejects additional IMEs without disturbing the active one. Candidate popups
are visible only while a text input is enabled; all live popups receive caret
updates, including the current rectangle at creation. Destroying an IME releases
its keyboard grab and removes its popups. Old keyboard objects cannot release a
replacement grab. Socket-level
regressions in `tests/text_input_protocol.rs` exercise these transitions with
real Wayland requests and events. Run them with
`cargo test -p halley --test text_input_protocol`. These tests validate protocol
handling; candidate-window rendering and toolkit integration still require a
live IME session.

Halley advertises `ext_background_effect_manager_v1` version 1 with the blur
capability. A committed `set_blur_region` is clipped to the requesting
surface, preserves ordered `wl_region` additions and subtractions, and is
drawn immediately behind that surface at its current layer or window stack
depth. Layer-shell roots and their popups use output-local coordinates;
ordinary toplevel roots follow the same camera, opening, fullscreen, and field
maximize presentation as their window.

An ordinary toplevel's XDG popup tree, and the equivalent X11
override-redirect menu window, is promoted to the desktop popup plane. It
renders and receives input above top-layer panels, while overlay-layer surfaces
and compositor-owned overlays remain above it. This lets context menus extend
across a status bar without making the owning window itself cover the panel.
xdg-positioner constraints inverse-map the output through that window's live
presentation rather than the field camera viewport, so fullscreen and
field-maximize menus stay on their anchor.

When an independent native toplevel closes while fullscreen or field-maximized,
Halley retains a normal client size for that app ID for the current session. A
genuine smaller pre-presentation size is preserved; an already-poisoned
output-sized restore is replaced with a bounded three-quarter-output fallback.
The next ordinary toplevel with that ID receives the size in its initial
configure, preventing clients that persist a fullscreen buffer size from
reopening as an ordinary output-sized window. Explicit initial-size window
rules take precedence, and surfaces already known when the hint is retained
are never resized by it. The hint is cleared after a normal close and is never
written to disk.

All blur effects on one output share one persistent output-sized texture pool.
Each stack depth still performs its own framebuffer capture, so an upper
translucent surface includes lower windows and panels without including
content above itself. No full texture chain is allocated per requesting
surface. The nested winit backend explicitly runs the same framebuffer-effect
sequence as the DRM damage tracker.

Halley also advertises `ext_data_control_manager_v1` version 1. Clipboard and
primary-selection managers use the existing seat selections shared with
`wl_data_device` and `zwp_primary_selection`; Halley does not copy or retain
clipboard payloads. The source client's file descriptor is transferred
directly to the receiver by Smithay's selection implementation.

Halley advertises `ext_idle_notifier_v1` version 2 and reports activity from
the compositor's physical keyboard, pointer, gesture, touch, and tablet input
path before modal routing. Idle managers therefore continue receiving correct
idle/resume edges while compositor-owned interfaces such as screenshot and
portal source selectors consume the input instead of forwarding it to a
client surface.

Halley advertises `zwp_idle_inhibit_manager_v1` version 1. Inhibitors are
reference-counted per surface and suppress ordinary idle notifications only
while at least one inhibited surface is actually visible in a composed output
scene. Fully occluded, unmapped, session-lock-hidden, dead, and disabled-output
surfaces do not keep the session awake. Visibility follows Smithay's
per-surface render-element state, including the primary-output selection for
surfaces spanning outputs.

Halley advertises `ext_session_lock_manager_v1` version 1. A lock request
immediately replaces every powered output with an opaque black scene; the
`locked` event is sent only after that generation has actually been submitted
on winit or page-flipped on every TTY output. Powered-off outputs are already
secure and do not delay confirmation. Lock surfaces are configured to each
output's logical size and are the only client surfaces rendered or given
keyboard, pointer, and touch focus until the owning, confirmed lock object
unlocks. Compositor bindings and ordinary client input are bypassed, and
screenshot plus screencast reads fail while locked. If the locker crashes or
destroys its surfaces, Halley deliberately remains locked with black outputs;
a second or unconfirmed lock object cannot unlock the session.

Halley advertises `wp_presentation` version 2 using `CLOCK_MONOTONIC`. On the
TTY backend, feedback is retained with the submitted DRM frame and completed
from its page-flip sequence and timestamp. Kernel monotonic timestamps carry
the `vsync`, `hw_clock`, and `hw_completion` flags, while zero-copy is reported
per surface from the DRM render-element state. The nested winit backend
completes feedback after host submission with monotonic time, fixed-refresh
metadata, and the `vsync` flag. Feedback is taken only for surface elements
actually included in the submitted frame, so compositor textures and hidden
or collapsed windows do not receive false presentation events.

On the TTY backend, Halley conditionally advertises
`wp_linux_drm_syncobj_manager_v1` version 1 only when the primary DRM device
supports syncobj eventfd notification. Support is demand-driven: advertising
the global does not allocate timelines, install event sources, or add waits.
Those resources are created only when an opting-in client imports a timeline
and commits a DMA-BUF with acquire and release points. The acquire point
blocks only that surface transaction without stalling the compositor event
loop; Smithay signals the release point when the compositor drops its final
reference to the buffer. The nested winit backend never advertises this
hardware protocol, and a surface that opts into explicit sync never also waits
on implicit fences.

Implicit-sync clients are covered separately. A newly committed DMA-BUF whose
planes are not yet readable is withheld from composited state until every
plane's readiness fence has signalled, so an unfinished buffer is never
imported or sampled and the compositor's event loop keeps running while it is
pending. If the readiness source cannot be registered, the buffer is committed
normally with a logged warning instead of installing a blocker that could never
be cleared, which would freeze the surface permanently.

Halley advertises `zwlr_output_manager_v1` version 4 as a writable output
management interface. Every request is validated as one complete, one-head-
per-output configuration before test or apply. The TTY backend supports mode,
position, transform, enable/disable, and adaptive-sync changes while retaining
at least one enabled output. Scale remains fixed at 1 and custom modes are
rejected. The nested backend is host-controlled and accepts only configurations
that leave its output unchanged. Successful TTY changes update `wl_output`
globals, layer layout, camera/fullscreen geometry, gamma ownership, and pending
capture ownership as one compositor transaction.

The TTY backend advertises `zwlr_gamma_control_manager_v1` version 1. Each
output with DRM gamma-ramp support has at most one active controller; requests
for an unavailable output fail.
Ramp file descriptors must contain exactly three native-endian `u16` channels
of the advertised gamma size; short and trailing data are rejected. Halley uses
atomic `GAMMA_LUT` blobs when supported, falls back to the legacy CRTC gamma
ioctl, and restores a linear ramp when control ends, an output is disabled, or
the compositor leaves its virtual terminal. The nested backend does not
advertise this hardware-only global.

Halley advertises `zwlr_screencopy_manager_v1` version 3 on both backends.
Whole-output and clamped output-region captures support optional cursor
composition and exact-size XRGB8888 SHM or DMA-BUF targets. SHM targets may
occupy a correctly bounded subrange of a larger pool. Ordinary copies complete
after composition; `copy_with_damage` waits for that output to submit a changed
frame and reports the captured buffer as damaged. DMA-BUF completion is sent
only after the renderer fence signals. Invalid, disabled-output, destroyed, or
session-lock capture requests fail rather than exposing stale or protected
content.

These globals are intended for shells, tools, and latency-sensitive clients.
They are independent: data-control, idle notification/inhibition, presentation
timing, output control, capture, and blur do not require one another, and
clients that do not bind them follow Halley's existing rendering, clipboard,
and input paths.

To test input-method-v2, start the newly installed Halley in a fresh compositor
session, run a Wayland-capable IME, and focus a native Wayland text-input-v3
application. Check preedit, candidate selection, committed text, moving between
fields, and restarting the IME. An already running compositor keeps its old
protocol implementation until it is restarted. `wayland-info` should list
`zwp_input_method_manager_v2` at version 1; the `v2` in the interface name is the
protocol generation, not the advertised interface version.
