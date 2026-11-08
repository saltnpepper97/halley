# Zoom to fit

`Mod+O` zooms out until you can see every window and node on each monitor.
Click the one you want and Halley zooms back in on it. Press `Mod+O` again, or
Escape, to go back to where you were.

## Details

Zoom to fit is the Field itself, zoomed out. Nothing moves and no separate
screen opens: each monitor's camera eases out until all of its windows and
collapsed nodes fit, with a small margin. Monitors that are showing a cluster
are left alone. Cluster members stay in their cluster and are not included.

While zoomed out:

- A plain click on a window zooms back in centred on it and focuses it.
- A plain click on a collapsed node zooms back in and restores it.
- A click on empty Field, `Mod+O` again, or Escape returns every monitor to the
  view it had before.
- `Mod`-clicks and drags still work, so you can move windows around while you
  can see everything.
- Panning or zooming by hand keeps the new view; Halley forgets the old one.

Zoom to fit may zoom further out than ordinary zoom (`zoom.min`), down to 10%,
because it is a quick look rather than a place to work. It never magnifies
past 100%, so a few small windows are shown at their real size. It needs zoom
to be enabled (`zoom.enabled true`, the default).

The action is `zoom-fit`. `apogee` and `overview` are older spellings of it.

## Gestures

A four-finger swipe up zooms out, and a four-finger swipe down while zoomed
out goes back. While zoomed out, swipes use the separate
`zoom-fit-swipe-DIRECTION-FINGERS` map:

```rune
input:
  gestures:
    swipe-up-4 "zoom-fit-open"
    zoom-fit-swipe-down-4 "zoom-fit-close"
  end
end
```

`apogee-open`, `apogee-close` and `apogee-swipe-DIRECTION-FINGERS` are older
spellings and keep working.

## Removed: Apogee

Zoom to fit replaces Apogee, the separate overview screen. `Mod+O` and any
binding to `apogee` keep working and now zoom to fit. The `apogee:` config
section is no longer read and can be deleted; Cluster Composer keeps the same
timing, spacing and dimming it used to take from that section.
