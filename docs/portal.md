# Screen sharing and the desktop portal

Halley's desktop portal is the safe bridge used by OBS, browsers, and chat
applications when they ask to capture the desktop. The application does not
choose a compositor surface directly. Halley opens its own modal chooser and
returns only the source the user confirms.

## Choosing a source

When an application supports both source types, the chooser first shows a
bottom bar:

- **Monitor** enters monitor-picking mode.
- **Window** enters window-picking mode.

If the application already requests only monitors or only windows, Halley
enters that picking mode directly and does not place a second source-category
menu over the application's own chooser. Use the pointer or Left/Right and
Enter. Escape returns from a picking mode to the bottom bar only for requests
that support both source types; otherwise it cancels the request.

Monitor and window picking dim everything outside the candidate. This is only
chooser chrome and is never included in the resulting stream.

## Privacy and window sharing indicators

Set `block-screencast true` in a matching window or layer rule to replace that content with black in portal monitor shares. The first matching rule wins. A protected window cannot be selected for a new window share; enabling its rule during an existing share turns its stream black. Windows and layer surfaces remain visible on the local display. These rules do not apply to screenshots or direct Wayland capture protocols. See [window and layer rules](window-rules.md) for matching syntax.

A directly shared window has a red outline on the local display until its last share ends. The outline is not included in portal streams and does not mark windows merely visible in a shared monitor. Configure its size and colour with:

```rune
decorations:
  screencast-indicator:
    size 4
    colour "#f0333d"
  end
end
```

While privacy rules are active, cached overview and focus-cycle scenes are black in monitor shares, and cached previews and backdrop blur are suppressed. XWayland override-redirect surfaces are also blocked while any window privacy rule is active because they cannot always be reliably associated with their owner.

## OBS

Add a **Screen Capture (PipeWire)** source in OBS. Halley's Monitor/Window bar
should appear, followed by the appropriate click-to-pick mode. Once confirmed,
OBS receives a live PipeWire stream. Closing the OBS source closes the portal
session and removes its PipeWire node.

The portal supports hidden, embedded, and metadata cursor modes. The requesting
application chooses the mode; no Halley configuration key is required.

Halley offers every DMA-BUF modifier that both its renderer can import and its
GBM device can allocate, with the renderer-preferred tiled modifier first, and
acknowledges each such capture only after the renderer fence signals. PipeWire
therefore queues a frame only when its GPU work is complete. The same stream
also offers a producer-owned shared-memory format as a last-resort fallback;
clients can choose a compatible linear DMA-BUF without changing the preferred
zero-copy path used by consumers such as OBS.
The portal queries the compositor's render device and supported modifiers
before allocating, so nested/software sessions use shared memory safely and
multi-GPU systems never guess a render node.

PipeWire frames use the standard variable-framerate/max-framerate fields and
carry header and full-damage metadata. DMA-BUF chunks remain explicitly
nonempty even though their modifier-dependent allocation size is not expressed
as `stride * height`; this matches the established wlr/Hyprland portal
contract used by OBS.

Capture is consumer-driven. A compositor frame is produced only when PipeWire
requests a buffer, at the negotiated rate up to 60 frames per second. There is
no continuously refreshed full-output CPU cache or per-frame compositor
connection.

## Session routing

`XDG_CURRENT_DESKTOP` must include `Halley`, and the installed
`halley-portals.conf` routes the ScreenCast and Screenshot interfaces to
`xdg-desktop-portal-halley`. Distribution packages install the matching D-Bus
and systemd user-service files from `packaging/`.

The compositor and portal use a versioned local capture protocol. The packaged
Halley backend service follows the desktop portal frontend's stop and restart
operations. Halley refreshes that frontend after publishing a new TTY session's
display environment, so the backend also starts fresh instead of retaining an
older executable or session state across logins.

After installing updated user-service files, run `systemctl --user daemon-reload`.
For an existing session, `systemctl --user restart xdg-desktop-portal.service`
refreshes both services. Custom backend units should include
`PartOf=graphical-session.target xdg-desktop-portal.service` too. Without that
relationship, restart `xdg-desktop-portal-halley.service` explicitly after
updating its binary.
