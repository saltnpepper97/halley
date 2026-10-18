# Backdrop blur and shadows

`effects.blur` controls how Halley blurs the scene behind translucent surfaces.
It applies to windows, layer-shell surfaces, client-requested background-effect
regions, and compositor-owned overlays. [Window and layer rules](window-rules.md)
control client activation; `overlays` allows Halley's own overlays to use blur.
Opacity alone does not enable blur.

```rune
effects:
  blur:
    overlays true
    method "gaussian"
    radius 24
    passes 3
    saturation 1.10
    noise 0.012
  end
end
```

## Methods

| `method` | Appearance and implementation |
| --- | --- |
| `"dual-kawase"` | Default. A soft, Gaussian-like blur using a downsample/upsample pyramid. Existing settings retain their behavior. `"dual_kawase"` and `"kawase"` remain aliases. |
| `"gaussian"` | A normalized, weighted Gaussian kernel, applied horizontally and vertically. Weights decrease smoothly away from the center. |
| `"box"` | Uniform averaging along each axis, with fractional weights at the kernel boundary. One pass pair gives the clearest rectangular footprint; repeated pairs produce a smoother result. |

All methods preserve rounded clipping, surface alpha, app-requested regions,
stacking order, saturation, and noise. The method is global: windows, panels,
and overlays share the same selection. Shadows have their own softness controls.

## Controls

| Setting | Range | Default | Meaning |
| --- | --- | --- | --- |
| `overlays` | boolean | `true` | Allow compositor-owned overlays to blur. Individual overlay settings still apply. |
| `radius` | `0`–`128` | `24` | Blur spread. Dual Kawase retains its existing radius-to-sample-offset mapping; for Gaussian/box this is the nominal kernel half-width in output pixels. |
| `passes` | `1`–`5` | `3` | Dual Kawase pyramid depth, or the number of horizontal/vertical filter pairs for Gaussian/box. |
| `saturation` | `0`–`4` | `1.10` | Color saturation of the blurred backdrop; `1` preserves saturation. |
| `noise` | `0`–`0.25` | `0.012` | Fine grain added during composition; `0` disables it. |

For Gaussian/box, each pair uses `radius / sqrt(passes)`, so repeated filtering
keeps the overall spread approximately stable. Gaussian uses that half-width
as three standard deviations and truncates at the next whole texel. Box uses
uniform weights inside the half-width and fractional weights at its boundary.
`radius 0` makes these two filters an identity operation; saturation and noise
still apply. The methods have different profiles, so equal radius values are
not guaranteed to look equally strong.

Large Gaussian/box kernels first downsample until the working half-width is
at most 16 texels, then filter and linearly upscale. Adjacent kernel taps are
paired using linear texture filtering, limiting each axis to at most 17 texture
reads. Downsampling adds some softness and can reduce fine detail. Increasing
passes changes the working resolution at these thresholds as well as the
filter shape. Gaussian and box can cost more than Dual Kawase, especially at
high resolutions; a bounded sampling count does not imply equal frame times.

Scratch textures are reused per output. Gaussian/box need one extra working
texture at full resolution, or two at reduced resolution, rather than a fresh
scratch set for every surface. Each effect keeps its own result and captures
the backdrop at its position in the stack. Rendering retains Halley's
conservative full-output capture policy.

## Trying another method

Change only `method` to `"gaussian"`, `"box"`, or `"dual-kawase"` in your existing
`effects.blur` block. Save the configuration; Halley watches it for changes.
You can also run `halleyctl reload`. Switching methods invalidates the cached
blur result immediately. Blur remains visible only where an active blur rule,
client region, or enabled overlay requests it.

After installing a newer compositor binary, log out and back in before testing
new methods. Reloading configuration does not replace the running executable.

## Shadows

`effects.shadows` has independent `window`, `node`, and `overlay` layers. Each
supports `enabled`, `colour` (`color` is an alias), `blur-radius`, `spread`,
`offset-x`, and `offset-y`.
Backdrop method selection does not change shadow rendering. See the complete
defaults in [the example configuration](../examples/halley.rune).
