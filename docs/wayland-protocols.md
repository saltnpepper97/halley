# Wayland protocol support

Halley advertises `wl_compositor` version 6 and `wp_fractional_scale_manager_v1`
version 1. Preferred integer and fractional buffer scales follow each window's
assigned output, including popups and subsurfaces, and update on transfer or
display-scale reload. See [display scaling](display-scale.md).

Halley advertises `wp_fifo_manager_v1` version 1 (`fifo-v1`) on native and
nested backends. Opting-in clients can keep a committed update active for a
refresh opportunity before a later update replaces it. FIFO barriers remain
independent of implicit/explicit GPU synchronization and ordinary frame
callbacks. Synchronized subsurfaces follow their parent's transaction.

Native submitted frames release only the barriers captured when that frame
was composed, at its page-flip event. Unchanged native scenes and nested EGL
submissions use a timer for one output refresh interval; nested timing is an
estimate because the host compositor owns physical presentation. A shared
one-second fallback releases barriers that have waited at least one second,
including collapsed, occluded, unmapped, locked-out, powered-off and removed
output cases. Normal event-loop operation therefore bounds these waits below
two seconds. Clients still need frame callbacks or timestamps to throttle
hidden rendering. Destroyed surfaces and disconnected clients are pruned.

This enables FIFO presentation for clients that use it; it does not promise
higher FPS, lower latency, reduced memory usage, or changes to other clients.
Commit timing can add an independent absolute eligibility constraint.

The protocol regression tests run with:

```sh
cargo test -p halley --test client_compatibility_protocol fifo_tests --locked
cargo test -p halley --test xwayland_disconnect fifo_nested --locked -- --ignored --test-threads=1
```

The second command needs a running Wayland desktop and XWayland. It creates
an isolated nested compositor and checks separate refresh opportunities,
collapse/restore and session lock without changing the host session. Socket
tests cover independent acquire blockers, synchronized subsurfaces, stale
frame snapshots across output handoff and the no-output fallback. Physical
TTY presentation, mixed-refresh monitors, DPMS and hotplug still require
native-session checks; nested EGL timing is not hardware presentation proof.
No new configuration or permission grant is needed to use FIFO.

Halley advertises `wp_commit_timing_manager_v1` version 1 (`commit-timing-v1`)
on native and nested backends. Clients can attach an absolute timestamp to a
surface commit using the same `CLOCK_MONOTONIC` domain as presentation feedback.
Future content becomes eligible at its deadline; past and zero timestamps add
no wait. Updates retain surface commit order, including untimed commits behind
a future one. Actual rendering and presentation continue through each output's
existing frame clock, so eligibility does not promise exact physical display
timing or a measured reduction in latency.

One session event source tracks the earliest pending deadline. It is replaced
when an earlier deadline arrives and removed when no timed work remains. Very
distant timestamps retain their full unsigned seconds range and use bounded
rearming without overflowing the event loop clock or becoming eligible early.
Nanoseconds outside `0..1_000_000_000` produce a protocol error. Duplicate
pending timestamps and requests on destroyed surfaces follow protocol errors.
Destroying the timer object preserves its already submitted constraints.

Timing barriers remain independent of FIFO and implicit/explicit acquire
fences. The FIFO fallback cannot release a future timing constraint. Timing
also applies to synchronized subsurfaces: both the parent transaction and the
child's deadline must be ready. Destroying a timed child drops its orphaned
timing wait; client transactions resume after surface-tree cleanup, so a live
parent cannot stay blocked.

Deadlines belong to commits rather than outputs. Transferring a window,
changing refresh rates, DPMS, output removal, or session lock does not reset
or bypass them. Deadlines still expire while no output is rendering; existing
visibility and frame-callback rules continue to throttle hidden clients. Client
or surface destruction removes pending work. If a deadline source cannot be
registered, affected clients receive a server error rather than an early
update or an indefinite live wait. No configuration change is required.

```sh
cargo test -p halley --test client_compatibility_protocol timing_tests --locked
cargo test -p halley --test xwayland_disconnect timing_nested --locked -- --ignored --test-threads=1
```

Socket tests check exact eligibility, past and maximum timestamps, commit order,
FIFO/acquire interactions, destruction, and synchronized child transactions.
The nested tests exercise real event-loop deadlines, output redraw, independent
clients, a collapsed-surface unmap, and lock. Physical TTY presentation,
mixed-refresh transfer, DPMS, and hotplug remain manual hardware checks.

Halley advertises `wp_pointer_warp_v1` version 1 (`pointer-warp-v1`) on native
and nested backends. A client can request a surface-local cursor position
using the current pointer-enter serial for its pointer-focused surface.
Requests follow the same Field, fullscreen, popup/subsurface, layer, and output
presentation as pointer hit testing. The destination must remain inside the
surface's committed bounds and input region, on its current output, and routed
to that surface. Stale serials, unfocused/hidden surfaces, and compositor-owned
grabs or modal interactions are ignored without moving the cursor or stealing
focus. Implicit click grabs remain usable. Active pointer locks reject warps;
confinement permits only positions inside its effective region. Session lock
and powered-off outputs reject requests. Ordinary physical and relative input
are unchanged; this protocol does not replace pointer lock or confinement.
See [pointer warp testing](pointer-warp.md) for a small interactive test client.

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

Halley uses a local Smithay snapshot based on revision
`79bbed5e1199090d787115614847a79c76607181`. Its input-method integration
contains a small lock-suspension and composition-state patch; see
`vendor/smithay/HALLEY-PATCH.md` for its scope.
Socket-level smoke tests live in `tests/text_input_protocol.rs`; toolkit and
candidate-window integration still require a live IME session.

When the screen locks, Halley suspends the desktop input method without
disconnecting it. The IME cannot receive lock-screen keystrokes or text-input
state, acquire an effective keyboard grab, or send composition to the lock
screen. Pending composition is discarded. After unlock the existing IME can
resume when a desktop text field is focused and enabled. A partly composed
string may need to be entered again. X11 applications continue to use their
X11 IME. The existing input-method executable grant remains necessary.
Fcitx5 also needs the separate virtual-keyboard grant to forward keys; see
[sensitive Wayland protocols](security.md#sensitive-wayland-protocols).

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

Startup maximize requests do not take Field-maximize ownership. Halley offers
those clients a windowed size at three quarters of the output's usable width
and height, rather than an unspecified size that can produce a small first
frame. The initial configure waits for the first surface commit so explicit
initial-size rules and session recovery hints can take precedence.

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

All blur effects on one output share one persistent scratch texture pool.
The [configured blur method](effects.md) selects either a Dual Kawase pyramid
or separable Gaussian/box filtering; all use the same surface regions,
clipping, and composition policy.
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
while retaining at least one enabled output. Scale requests use the same
`0.25` through `10.0` range and 1/120 rounding as monitor configuration.
Positive out-of-range values fail the complete transaction; zero or negative
values are protocol errors. Custom modes are rejected.
Display apply requests are rejected while the native session is paused;
config-file output changes are deferred until both VT and system-sleep pauses
have ended. The nested backend accepts local display-scale changes while its
mode, position, transform, enable state, and adaptive sync remain host-controlled.
Successful changes update `wl_output`
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

The TTY backend exposes `zwlr_output_power_manager_v1` version 1
(`wlr-output-power-management-unstable-v1`) to clients approved through
`HALLEY_ALLOW_OUTPUT_POWER`. Controls use existing DPMS and receive the current
mode immediately, followed by effective changes from protocol clients,
`halleyctl`, and input wake. Off retains logical output membership, configured
mode, scale, position, and window placement. Unavailable outputs invalidate
controls; invalid modes are protocol errors. Writes while the seat is paused
fail, including requests for an already matching mode. The nested backend
creates no power global. See [output power management](output-power.md) for
permissions, lifecycle behavior, limitations, and hardware validation.

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

Halley advertises `ext_output_image_capture_source_manager_v1` and
`ext_image_copy_capture_manager_v1` version 1 on both backends, visible only to
clients with the existing [screen-capture grant](security.md#sensitive-wayland-protocols).
Output sources support XRGB8888 SHM buffers at the output's transformed native
pixel dimensions, including fractional display scaling. The image is already
upright, so frame metadata reports the normal transform. Embedded cursor
composition follows the session's `paint_cursors` option.

Each capture initializes the entire buffer and reports full-frame damage;
client buffer-damage hints do not restrict the copy. A static desktop's first
frame completes without waiting for a content change. Exact dimensions and the
advertised format are required; bounded offsets and padded strides are accepted.
Constraints are resent when the output mode, scale, or transform changes.
Clients must destroy each frame object before creating the next one. Session
and manager destruction leave existing frames usable; frame destruction cancels
its copy. Destroyed buffers fail safely. Lock, source destruction, output loss,
logical disable, power-off, and seat pause permanently stop affected sessions.
After resuming, clients need a new session.

This first implementation advertises no DMA-BUF formats or toplevel-source
global. Unsupported buffer types fail with `buffer_constraints`; a cursor-only
session's derived capture session is stopped. Capture uses the existing composed
output renderer and a CPU SHM readback, so this adds client compatibility rather
than promising faster recording or reduced memory use. Existing wlr-screencopy
and portal capture keep their current paths.

```sh
cargo test -p halley --test image_copy_capture_protocol --locked
cargo test -p halley --test xwayland_disconnect image_copy_nested --locked -- --ignored --test-threads=1
```

The socket tests cover validation, padded SHM copies, constraint updates, and
object lifetimes. Nested tests require a running Wayland desktop, XWayland,
and Kitty. They compare a real terminal's captures with the portal path at
1.0, 1.25, 1.5, and 2.0 display scales, exercise legacy screencopy alongside
modern sessions, and check a visible SHM cursor, grants, and session lock.
Native TTY transform, DPMS, hotplug, and VT pause still need hardware checks.

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
