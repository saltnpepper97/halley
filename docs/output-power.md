# Output power management

On the native TTY backend, Halley provides interface version 1 of
`wlr-output-power-management-unstable-v1`. Compatible Wayland tools can create
`zwlr_output_power_v1` controls for individual initialized, enabled outputs.
The nested winit backend leaves display power to its host compositor and does
not create `zwlr_output_power_manager_v1`.

## Access and behavior

Before starting the native session, approve the tool's installed executable
using `HALLEY_ALLOW_OUTPUT_POWER`, a colon-separated list of absolute paths.
For example, set `HALLEY_ALLOW_OUTPUT_POWER=/usr/bin/wlopm` in the environment
that launches Halley. Use the actual installed path. Only approved connections
see the global; observation and writes share this grant. This follows the
[desktop capability permissions](security.md) policy. Reconnect tools after
restarting Halley with a changed grant. Installing Halley does not update the
running compositor, and setting the variable only on the tool does not change
compositor policy.

The protocol uses the same DPMS implementation as `halleyctl dpms` and physical
input wake. Turning an output off preserves its configured display mode,
scale, transform, position, logical membership, and window placement. It does
not disable or remove the output through output management. No idle timeout,
new persistent display setting, or separate power policy is introduced.

Each control immediately receives its current `on` or `off` mode. Existing
controls receive subsequent effective changes, whether initiated through this
protocol, Halley's IPC, or input wake. Multiple controls may observe and change
the same output. Destroying a control or its manager does not restore power;
manager destruction leaves existing controls usable. Repeating an already
matching power request succeeds without a redundant mode-change event.

Power-on follows Halley's ordinary DPMS wake path: the first rendered frame
commits the enable and scan-out buffer together. An `on` event reports the
backend's accepted power state, not proof that the physical monitor has
finished waking or acknowledged a frame. The protocol cannot report physical
panel latency, driver hangs, or later frame-submission failures. It preserves
the existing rendering and driver behavior.

## Failure and lifecycle

A control receives `failed` if its output is unsupported, disabled through
output management, disconnected, or a power request fails. It is then inert
and should be destroyed. It cannot regain authority when an output is
re-enabled or a monitor with the same name reconnects; obtain a new control.
An unknown numeric mode instead raises the protocol's `invalid_mode` error,
which terminates that Wayland connection.

Writes fail while the native seat is paused by a VT switch or system sleep,
even if the requested mode already matches. Other observers remain usable,
and clients can create a fresh control after resume. Recovery and config reload
publish the retained effective DPMS state. Logically disabled outputs must be
re-enabled through output management or configuration before power controls
are available.

Halley still allocates connectors and CRTCs at startup. A DRM device monitor
invalidates controls when an initialized connector disappears; power requests
also check cached connector availability. New monitors, replacement GPUs, and
newly usable connectors are not initialized by this protocol and require a
fresh compositor session. General display-layout hotplug is outside this
implementation.

Session-lock confirmation continues to treat unlit outputs as secure. Waking
an output while locked uses the existing locked render scene; power controls
do not unlock the session or bypass capture permissions or lock-screen capture
guards.

## Validation

`tests/output_power_protocol.rs` drives production protocol dispatch over real
Wayland sockets with a controlled driver. It covers selected-output off/on,
idempotency, multiple controls and outputs, external IPC/wake state changes,
manager/control destruction, unsupported and removed outputs, reconnecting
identities, paused writes, hardware-request failure, invalid modes, global
visibility, and retained output geometry. These tests do not switch physical
monitors or prove DRM behavior.

For hardware validation, start the newly installed native compositor with the
trusted tool grant, create controls for two supported outputs, and power one
selected display off/on. Verify mode events, retained layout and windows,
`halleyctl dpms` notifications, input wake, unplug invalidation, and paused-seat
failure. Repeat with the session locked to verify lock confirmation, safe wake,
and capture rejection. Perform this explicitly in a suitable test session;
installing the binary or passing socket tests is not live-hardware validation.
