## Halley (Comet Desktop)

`halley-wl` can run nested (`winit`) or on tty.

### CLI

`halleyctl` is provided by the `halley-cli` crate.

- `halleyctl outputs` prints current output/connector information from `/sys/class/drm`.
  - On systems where DRM ioctls are available, it also prints available refresh rates.

### Xwayland via xwayland-satellite

Halley now supports X11 apps through `xwayland-satellite` (recommended first step before native XWM integration).

Default behavior is on-demand (no env needed): Halley requests satellite startup when launching apps.

Dev override controls:

- `HALLEY_DEV_WL_XWAYLAND=ondemand|auto|on|off`
- `HALLEY_DEV_WL_XWAYLAND_DISPLAY=:N` (optional, default is first free `:N`)
- `HALLEY_DEV_WL_XWAYLAND_PATH=/path/to/xwayland-satellite` (optional)
- `HALLEY_DEV_WL_XWAYLAND_RESTART_MS=1500` (optional restart backoff in ms)

Behavior:

- When satellite starts, Halley sets `DISPLAY` to that satellite display for child processes.
- When satellite is disabled/unavailable, Halley clears `DISPLAY` to avoid leaking X11 apps to the host desktop.
# halley
