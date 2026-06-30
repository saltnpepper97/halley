# Halley Lift

Halley Lift is a standalone command palette for Halley. It is built from the `halley-lift` crate, installs as the `halley-lift` binary, and talks to the compositor through Halley's existing IPC APIs.

## Run

```bash
halley-lift
```

You can also seed an initial query:

```bash
halley-lift cluster release
```

## Search Prefixes

Lift searches everything by default. Prefixing the query with a provider name filters results without changing the search text into a badge.

Supported modes:

```text
app apps
cluster clusters
node nodes
action actions
config
term
```

Example:

```text
cluster release
```

searches clusters for `release` while leaving the full text visible in the search field.

`term` runs the typed command line in the configured `terminal` through your
interactive `$SHELL` (so aliases, pipes, and `&&` work), keeps a shell open afterward, and then
closes Lift:

```text
term journalctl -f | grep halley
```

## Cluster Drafts

In `cluster` searches, `Space` stages or unstages the selected app or running node. This is side-effect-free. Outside cluster searches, Space is normal search text.

After at least one item is staged in cluster mode, `Ctrl+Enter` or activating `Create cluster: <query>` materializes the draft:

```text
Cluster Draft: release · 3 selected
```

At that point Lift opens Halley's existing Cluster Finalize popup with a name hint and selected running node IDs. Staged apps are launched only during this handoff, and the compositor auto-selects matching newly appearing nodes while that finalize prompt is active.

Lift does not directly persist clusters. The finalize popup owns naming, confirmation, and final creation.

## Pins

Lift does not keep its own favorites database. Field/Bearings-pinned nodes come from Halley and rank above normal matching nodes.

## Config

Config path:

```text
~/.config/halley/lift.rune
```

On first launch, if no config is present Lift writes a documented default
template (mirroring `examples/lift.rune`) to that path so you have a starting
point. Existing files are never overwritten.

Example config lives at `examples/lift.rune`.

Useful layout keys:

```rune
lift:
  placeholder "Search apps, nodes, clusters, actions..."
  width 760
  max-results 40
  visible-results 8
  icons true
  icon-size 28
  icon-theme "auto"
  icon-search-depth 5
  terminal "x-terminal-emulator -e"
  close-on-focus-loss false
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
    search-icon ""  # magnifier tint; empty = follow `hint`
    icon ""         # result-list icons; empty = follow `accent`
    alt-hint ""     # Alt+<n> jump labels; empty = follow `hint`
  end

  border:
    enabled true
    width 1          # thickness in px
    style "outline"  # "outline" wraps the whole app; "inset" borders only the results
  end

  search-icon:
    enabled true
    side "left"      # "left" or "right" of the search text
    size 22
  end

  cursor:
    enabled true
    width 2
    blink-ms 500
    stop-blink-after-ms 5000
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

`max-results` controls how many results Lift computes. `visible-results` controls how many rows are visible at once; keyboard selection scrolls through the full result set.

App icons are read from `.desktop` `Icon=` entries and resolved lazily from common XDG icon locations while drawing visible rows. Lift builds a broader icon index only after the first draw, then refreshes cached misses. PNG, JPEG, and SVG icons are supported. Missing icons fall back to built-in glyphs.

`terminal` is prepended to `.desktop` apps with `Terminal=true`, so terminal apps such as `micro` or `nvim` open in the configured terminal.

Mouse support includes hover selection, row click activation, and wheel navigation inside the Lift panel. Empty general search shows only the rounded search bar; typing or entering a mode prefix expands a connected results body below it. Keyboard navigation supports held direction keys, Left/Right, PageUp/PageDown, Home/End, and Alt+1 through Alt+0 visible-row activation.
