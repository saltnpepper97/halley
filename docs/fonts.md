# Fonts

Halley uses one global font for compositor-owned text:

```rune
font:
  family "monospace"
  size 11
end
```

`family` accepts Cosmic Text generic families (`monospace`, `sans-serif`,
`serif`, `cursive`, and `fantasy`) or an installed family name. Weight and
style suffixes are supported, including `Bold`, `Semi Bold`, `Extra Bold`,
`Light`, `Italic`, and `Oblique`. Cosmic Text supplies fallback fonts when the
selected family does not contain a requested glyph.

`size` is the compositor UI size in logical pixels and is clamped from 6 through 96. Every
compositor-owned label, badge, dialog, notification, and overlay uses this
exact configured size. Individual overlays do not apply hidden legacy size
offsets; hierarchy comes from colour, chrome, and placement instead. Node
markers reserve their center for real application icons and remain blank while
an icon is loading or unavailable. Text is rasterized for each monitor's [display scale](display-scale.md),
so a larger display scale also enlarges text without relying on a small bitmap.

The section live-reloads with the rest of `halley.rune`. A successful font
change clears the text cache and redraws immediately; it does not require a
restart. There is no `halleyctl` command for fonts; the configuration file is
the only place to change them.
