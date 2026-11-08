# Wallpaper

Halley's compositor wallpaper is disabled by default:

```rune
wallpaper:
  mode "none"
end
```

`none` creates no wallpaper render element. Pixels with no other scene
content are simply cleared to opaque black; Halley does not enforce a
wallpaper colour.

The wallpaper is part of the shared renderer, so TTY, nested Winit,
screenshots, screencopy, and Cluster Composer use the same scene. It is always appended
behind the protocol `background` layer. Windows, panels, overlays, capture UI,
and the cursor therefore remain above it. An active session lock replaces the
desktop with its fail-closed lock scene and never exposes the wallpaper.

## Classic images

```rune
wallpaper:
  mode "classic"
  path "~/Pictures/wallpaper.png"
  fit "cover"
  intensity 1.0
end
```

`fit` accepts:

- `cover`: fill the output and crop the image's longer axis;
- `contain`: show the whole image, centered with black around it;
- `stretch`: resize independently on both axes.

Relative paths resolve from the directory containing `halley.rune`. `~/`
expands through the current home directory. Images are decoded and uploaded
once per renderer context, then reused across frames and outputs.
`intensity` is clamped to the image opacity range for classic wallpapers.

## Field shaders

```rune
wallpaper:
  mode "field-shader"
  shader "space"
  colour "#181a26"
  accent-colour "#8fa8d8"
  intensity 1.0
  animated true
end
```

`shader "space"` uses the exact old-Halley space shader bundled with Halley.
It maps through the current output camera, so panning and zooming stay spatially
coherent. `animated true` restores the old-Halley star-field motion; use
`animated false` when a completely static field is preferred. Halley requests
continued frames only while an animated field shader is visible; a settled,
opaque fullscreen window suppresses that work and remains eligible for
automatic VRR. Cluster Composer continues the animation because its tiles
intentionally show the wallpaper.

A shader value other than `space` is treated as a fragment-shader path. Custom
shaders use the same Smithay texture-shader interface as the bundled shader and
may consume:

- `v_coords`, `tex`, and `alpha`;
- `u_resolution`, `u_camera_center`, and `u_camera_size`;
- `u_time` and `u_intensity`;
- `u_base_color` and `u_accent_color`.

The standard shader source must retain Smithay's `//_DEFINES` insertion marker.
If a custom shader cannot be read or compiled, Halley logs the failure once and
uses the bundled space shader. If an image or the bundled shader cannot be
created by the active renderer, plain black remains visible instead of failing
the output frame.

The old `background:` and `gesso:` section names remain accepted, but
`wallpaper:` is the current section; see [older spellings](older-spellings.md).
