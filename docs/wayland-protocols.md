# Wayland protocol support

Halley advertises `wl_compositor` version 6 and `wp_fractional_scale_manager_v1`
version 1. Preferred integer and fractional buffer scales follow each window's
assigned output, including popups and subsurfaces, and update on transfer or
display-scale reload. See [display scaling](display-scale.md).

Halley advertises `zxdg_exporter_v2` and `zxdg_importer_v2` interface version 1
(`xdg-foreign-v2`). Applications can export a toplevel handle and pass it to a
desktop portal; the portal imports the handle to attach its dialog to the
original window across separate Wayland connections. Imported parent
relationships use the same placement, stacking, rendering and pointer policy
as ordinary XDG parents. Destroying the owning import or export clears the
relationship. A missing or invalid handle leaves the dialog independent.
The browser and portal must connect to a compositor exposing these globals;
installing a new binary does not change an already-running compositor.

Halley advertises `xdg_wm_dialog_v1` version 1 (`xdg-dialog-v1`). Applications
can mark a parented native toplevel as a dialog and set or remove its modal
hint. Activating a parent with a mapped, eligible modal descendant focuses
and raises that dialog; nested dialogs use the frontmost eligible descendant.
Late modal hints and parent changes also reconcile the currently focused
family. Unrelated windows and layer-shell interfaces retain their focus.
Collapsed, unmapped, destroyed, and inactive-workspace dialogs do not redirect
focus. Non-modal dialogs retain the ordinary parent stacking and focus policy.

Destroying a dialog object or removing its modal hint removes the focus
restriction without automatically switching away from the dialog. A dialog
without a live parent has no modal effect. The existing `xdg-foreign-v2`
relationship supplies the parent for cross-application portal dialogs; it is
independent of the modal hint. Clients remain responsible for filtering input
in their own parent windows. Halley does not globally block other applications
or infer modality from window titles or app IDs. No config option is required.

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

Halley uses unmodified Smithay pinned to revision
`79bbed5e1199090d787115614847a79c76607181`, matching the Niri revision checked
for this change. Native composition and candidate popups use its basic
text-input/input-method implementation. Halley no longer carries the IME
state, commit-buffering, keyboard-grab teardown, or multiple-popup patches.
Socket-level smoke tests live in `tests/text_input_protocol.rs`; toolkit and
candidate-window integration still require a live IME session.

When the screen locks, Halley disconnects clients that created an input method
and rejects new input-method requests until unlock. This prevents the IME from
receiving lock-screen input without modifying Smithay. The IME must reconnect
or be restarted after unlock; composition does not resume automatically on the
old connection. X11 applications continue to use their X11 IME.

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

With the unmodified Smithay pin, Halley uses conservative full-output
framebuffer capture and blur processing. Halley's custom foreground-only invalidation and padded regional
damage hooks have been removed, so blur may require more GPU work. Local
animations likewise repaint their output fully; unrelated outputs and the FPS
overlay alone retain their existing redraw policy.

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
position, transform, fractional scale, enable/disable, and adaptive-sync changes
while retaining at least one enabled output. Custom modes are rejected.
Display apply requests are rejected while the native session is paused;
config-file output changes are deferred until both VT and system-sleep pauses
have ended. The nested backend is host-controlled and accepts only configurations
that leave its output unchanged. Successful TTY changes update `wl_output`
globals, layer layout, camera/fullscreen geometry, gamma ownership, and pending
capture ownership as one compositor transaction.

Native display recovery resets pending compositor frame waits and estimated
frame timers before scheduling a fresh frame. Outputs disabled through output
management or powered off through DPMS remain suspended until explicitly
enabled or woken. Late page-flip events received while suspended do not seed
frame timing or start throttle timers. DPMS power requests are rejected while
the native session is paused. The ordinary DPMS wake path resumes through the
next rendered frame. Re-enabling a display through config reload restores its
`wl_output` advertisement as well as rendering.

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

Halley advertises `wp_single_pixel_buffer_manager_v1` version 1. Clients can
create a solid-color buffer without shared-memory storage; Smithay handles
its lifecycle and rendering.

Exclusive layer-shell focus controls which client receives forwarded keyboard
input. Halley still evaluates compositor shortcuts unless the session is locked
or the focused surface has an active keyboard-shortcuts inhibitor.

Halley advertises `wp_content_type_manager_v1` and
`xdg_toplevel_icon_manager_v1` version 1. Smithay stores content hints and icon
metadata with committed surface state. These hints do not change maximization,
fullscreen, or focus policy, and accepting icon metadata does not yet display
client-supplied icons in Halley UI.

Native outputs use hardware cursor planes when the cursor and driver support
them. Set `disable-hardware-cursor true` in the `cursor` section and reload to
force software composition if cursor artifacts appear. Winit has no DRM cursor
plane, and cross-GPU outputs retain software cursor composition inside the
transferred scene texture.

Halley advertises `ext_foreign_toplevel_list_v1` version 1 and
`zwlr_foreign_toplevel_manager_v1` version 3 for window lists, taskbars and docks.
Both enumerate native Wayland and managed XWayland windows, including collapsed
nodes and windows in inactive clusters. Popups, layer surfaces and X11
override-redirect menus are excluded. Titles and app IDs follow client metadata;
X11 app IDs use the window class. The ext list uses Smithay's stable identifiers.

The wlr protocol publishes activated, minimized (collapsed), maximized and
fullscreen states, owning-output associations, and transient parents. Every
manager binding gets its own handles. Output associations refer to the owning
monitor even while its window is collapsed or its cluster is inactive. Later
`wl_output` bindings receive the association too. Window unmaps and destruction
close the handles; remapping creates fresh handles and an ext identifier.
Destroyed handles are never recreated during the same mapping.

Taskbars can activate, close, minimize/restore, maximize/unmaximize and
fullscreen/unfullscreen windows through Halley's existing window actions.
Activation switches to the window's cluster or Field and restores collapsed
nodes; stack and overflow members are brought into view. Requests are ignored
while the session is locked or an interactive compositor grab is active.
Activation must name Halley's seat. Fullscreen keeps the window on its current
output (the requested output is an optional hint). Taskbar rectangles are
validated but do not replace Halley's spatial node collapse destination.
Minimizing a fullscreen window follows the existing policy and may be declined.

`tests/foreign_toplevel_protocol.rs` checks lifecycle, properties, versions,
multiple bindings and clients, output and parent updates, and request dispatch
through real sockets. Its optional nested-session test additionally exercises
native window actions against a running Halley; provide
`HALLEY_TEST_WAYLAND_DISPLAY` as the absolute path to that test session's socket.

## D-Bus idle inhibition

Native sessions also own `org.freedesktop.ScreenSaver`, at
`/org/freedesktop/ScreenSaver` and the legacy `/ScreenSaver` path. This supplies
the `Inhibit`/`UnInhibit` endpoint used by GTK's portal fallback and combines
those inhibitors with visible Wayland inhibitors when updating idle notifications.
Cookies belong to their calling connection; disconnecting removes all its
cookies. Nested sessions do not claim the host service, and an existing owner
is never replaced or queued behind. Explicit lock or suspend actions remain
available while idle is inhibited.

This API inhibits idleness only. GTK's rejection of logout, user-switching, or
non-idle suspend flags is expected, rather than a compositor protocol failure.
Missing RealtimeKit, AppIndicator deprecation, duplicate registrations inside
external tray clients, and AMD kernel display warnings are separate diagnostics;
Halley does not suppress them.
