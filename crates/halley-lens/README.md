# Halley Lens

Halley Lens is a standalone command palette for Halley. It runs as `halley-lens` and talks to the compositor through Halley's existing IPC APIs.

## Run

```bash
halley-lens
```

You can also seed an initial query:

```bash
halley-lens /cluster release
```

## Slash Modes

Lens searches everything by default. Slash tokens switch to a mode and become a removable badge in the search field.

Supported modes:

```text
/app /apps /a
/cluster /clusters /c
/node /nodes /n
/action /actions
/config
```

Example:

```text
/cluster release
```

becomes:

```text
[Clusters ×] release
```

Backspace with an empty query removes the badge and returns to general mode.

## Cluster Drafts

In cluster mode, `Space` stages or unstages apps and running nodes. This is side-effect-free.

`Ctrl+Enter` or activating `Create cluster: <query>` materializes the draft:

```text
Cluster Draft: release · 3 selected
```

At that point Lens opens Halley's existing Cluster Finalize popup with a name hint and selected running node IDs. Staged apps are launched only during this handoff, and the compositor auto-selects matching newly appearing nodes while that finalize prompt is active.

Lens does not directly persist clusters. The finalize popup owns naming, confirmation, and final creation.

## Pins

Lens does not keep its own favorites database. Field/Bearings-pinned nodes come from Halley and rank above normal matching nodes.

## Config

Config path:

```text
~/.config/halley/lens.rune
```

Example config lives at `examples/lens.rune`.

Useful layout keys:

```rune
lens:
  placeholder "Search apps, nodes, clusters, actions..."
  width 760
  max-results 40
  visible-results 8
  icons true
  icon-size 28
  icon-theme "auto"
  icon-search-depth 5
  keyboard-interactivity "exclusive" # exclusive | on-demand
  close-on-focus-loss false
  close-on-click-away false
  alt-number-jump true

  position:
    anchor "center" # center | top | top-left | top-right | bottom | bottom-left | bottom-right
    offset-x 0
    offset-y 0
  end

  rounding:
    panel 18
    dropdown 14
    search 12
    row 12
    badge 10
    draft 10
  end

  colors:
    panel "#151720ee"
    panel-border "#2b3248cc"
    dropdown "#151720ee"
    dropdown-border "#2b3248cc"
    search "#090b12d8"
    row-selected "#2e4575ea"
    divider "#2b324899"
    text "#f2f5ff"
    subtext "#9ea7bf"
    hint "#858fa8"
    accent "#8fb5ff"
    badge "#334875f2"
    danger "#eb9a8f"
  end

  ui:
    top-margin 96
    padding 20
    dropdown-gap 0
    dropdown-padding 10
    search-height 60
    row-height 64
    row-gap 6
    footer-height 0
    font "sans-serif"
    search-font-size 22
    title-font-size 17
    subtitle-font-size 13
    hint-font-size 12
  end
end
```

`max-results` controls how many results Lens computes. `visible-results` controls how many rows are visible at once; keyboard selection scrolls through the full result set.

App icons are read from `.desktop` `Icon=` entries and resolved from common XDG icon locations. Lens builds an icon index once at launch, then uses cached lookups while drawing. PNG, JPEG, and SVG icons are supported. Missing icons fall back to built-in glyphs.

Mouse support includes hover selection, row click activation, and wheel navigation inside the Lens panel. Empty general search shows only the rounded search bar; typing or entering a slash mode expands a connected results body below it. Keyboard navigation supports held direction keys, Left/Right, PageUp/PageDown, Home/End, and Alt+1 through Alt+0 visible-row activation.
