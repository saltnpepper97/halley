# Display scaling

Set `scale` inside the monitor's `view.output` block:

```rune
view:
  output:
    name "eDP-1"
    scale 1.5
  end
end
```

`1.0` means 100%, `1.5` means 150%, and `2.0` means 200%. Use a number,
without quotes or a percent sign. The default is `1.0`. Values from `0.25`
through `10.0` are accepted and rounded to the Wayland fractional-scale unit
of 1/120. The effective value appears in `halleyctl outputs`.

Display tools using `wlr-output-management-unstable-v1` can test and apply the
same scales at runtime. For example, use `wlr-randr --output eDP-1 --scale 1.5`
with your connector name. These changes do not rewrite your config; a config
reload or new session restores the configured scale. A rejected transaction
leaves every output unchanged. The nested backend permits local scale changes
but keeps the host's mode, position, transform, and enable state.

A scale-only entry uses the connector's preferred native mode. To choose a
resolution or refresh rate, supply `width` and `height` together as usual.
These remain physical pixels; scaling does not lower the monitor's resolution.
For the nested development backend, use the output name `"winit"`.

Scale increases the size of both applications and Halley's interface. For
example, a 3840×2160 display at `2.0` has a logical desktop of 1920×1080.
Wayland applications receive integer and fractional preferred scales so they
can render at the appropriate buffer resolution. Older applications that only
support integer scaling may be resampled at fractional settings. XWayland
applications retain their existing logical-coordinate behavior and are scaled
by the compositor; their text can look softer than native Wayland text.

Monitor `offset-x` and `offset-y`, window sizes, notification offsets, borders,
and `font.size` are logical pixels. For a 3840-pixel-wide monitor at `2.0`, put
the next monitor at `offset-x 1920`, rather than `3840`. Existing positions
remain unchanged when scale is `1.0`. Position, refresh, VRR, and transform
settings still require an explicit width/height pair.

If existing offsets leave a gap after scaling, relative mouse motion crosses
that gap directly into the nearest monitor along the exited edge. Dragged
windows follow the cursor. Crossing is possible where the monitors overlap
along the other axis; the remaining outer edges still constrain the cursor.

Halley's Field camera zoom is separate: display scale establishes normal UI
size; camera zoom changes your view of the Field. Changing `font.size` affects
only Halley text, while display scale also affects application content.

Valid scale edits reload without restarting the compositor. Window placement
and pointer hit testing stay in logical coordinates. Monitor screenshots and
screen sharing use native framebuffer dimensions. A screenshot spanning
monitors with different scales uses the highest selected scale and resamples
other monitors to keep the desktop layout continuous.

Fresh bootstrap and example configs include `scale 1.0`. Existing configs are
never rewritten automatically. Missing scale and notification offsets use
built-in defaults (`1.0` and `0` respectively); add those fields manually if
you want them explicitly present, then run `halleyctl config verify`.
