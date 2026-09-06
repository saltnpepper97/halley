# Animation configuration

`animations.enabled` is the master switch. Each animation also has its own
`enabled` field, so an individual effect can be disabled without changing the
others.

Interactive pointer resize has its own `animations.smooth-resize` block. It is
enabled by default with a 90 ms response duration: lower values track the
pointer more tightly, while higher values soften rapid changes. Halley advances
the filtered size on its interaction timer for both Wayland and XWayland and
stops at the currently presented size as soon as the button is released.

Fullscreen defaults to a critically damped spring:

```rune
animations:
  fullscreen:
    enabled true
    motion "spring"
    damping-ratio 1.0
    stiffness 800.0
  end
end
```

Spring motion accepts `damping-ratio` and `stiffness`. Lower damping ratios
allow overshoot and higher stiffness moves faster. Halley keeps the settling
threshold internal.

An animation can use duration-based easing instead:

```rune
animations:
  fullscreen:
    enabled true
    motion "easing"
    duration-ms 250
    curve "ease-out-cubic"
  end
end
```

Available curves are `linear`, `ease-in-out-cubic`, `ease-out-quad`,
`ease-out-cubic`, `ease-out-expo`, and `elastic`. The elastic curve overshoots
before settling. `duration-ms` is always the complete timeline from the first
animated frame through the settled final frame. Curves redistribute progress
inside that time: ease-out curves cover more distance near the beginning,
`ease-in-out-cubic` accelerates and then decelerates symmetrically, and
`linear` advances at a constant rate.

Window opening separates visual style from motion:

- `type "center-out"` scales the window outward from its final center.
- `type "fade"` keeps the final geometry and animates opacity.
- `type "launch"` moves from a meaningful origin to the final geometry along
  a restrained arc. It starts at 80% scale and low opacity, briefly reaches
  102%, then settles at full size.

The launch origin is the activating launcher's visual center when the client
uses `xdg-activation`. Otherwise Halley uses the cursor on the target output,
the focused window center, then the output center. Travel is capped so a new
window cannot fly across an entire display. A 220 ms `ease-out-cubic` motion is
the recommended starting point:

```rune
animations:
  window-open:
    enabled true
    type "launch"
    duration-ms 220
    curve "ease-out-cubic"
  end
end
```

Every type can use any easing curve through `duration-ms` and `curve`, or
select `motion "spring"` and use the same spring fields. Changing the type
never selects a different curve or duration implicitly. Launch applies the
selected curve once to spatial travel; its opacity and 80% -> 102% -> 100%
scale choreography remain anchored to elapsed time so they finish exactly at
`duration-ms`.

Once a client actually unmaps, window closing freezes its last visible frame,
removes the real window from input, then animates the inert snapshot. A close
request alone does not freeze the client because applications may show and
cancel their own confirmation first:

```rune
animations:
  window-close:
    enabled true
    type "shrink"
    duration-ms 270
  end
end
```

`type "shrink"` collapses the snapshot into its center without fading.
`type "fade"` keeps the final geometry and fades to transparent.
`type "retract"` is the close counterpart to `launch`: it follows the launch
arc backward, returns from 100% through the same 102%/80% scale choreography,
and reduces opacity toward the launch state. Halley remembers the window's
actual launch origin for its lifetime; if none was recorded, retract uses the
same cursor, focused-window, then output-center fallback chain as launch.
Travel remains capped at 320 pixels.

The intended open/close pairs are `center-out`/`shrink`, `fade`/`fade`, and
`launch`/`retract`. All close styles retain the original ease-in-out cubic
timing. Closing snapshots preserve the window's current opening opacity and
track camera motion while they finish. Layer-shell surfaces and X11
override-redirect popups are not window-close animation targets.

An optional `custom-shader` path on `window-open` or `window-close` replaces
the type's scale and fade with a user fragment shader. Launch and retract
still travel. This is an advanced feature; see `docs/window-shaders.md`.

Node marker appearance uses a short ease-out scale transition. Restoration and optional
camera centering start together; it never centers first and waits for a second
action to restore the window.

When a live window becomes a node, its inert GPU snapshot travels and shrinks
into the legal landmark position while retaining the window's stack depth.
This drop always shrinks into the node even when ordinary window closes are
configured as `fade`; ordinary closes continue to use the selected close type.

```rune
animations:
  node:
    enabled true
    duration-ms 280 # Node marker appearance.
    collapse-duration-ms 280 # Window snapshot shrinking/traveling into the node.
  end
end
```

`duration-ms` controls node marker appearance; `collapse-duration-ms` controls
only the window snapshot shrinking and traveling into its node, for both manual
collapse (`Mod+N`) and automatic decay. Each defaults to 280 ms and is independent
of `window-close.duration-ms` and close custom shaders. Collapse uses the CPU
shrink path; ordinary close shader timing remains unchanged.

`animations.enabled` and `animations.node.enabled` gate both node transitions.
Setting either to `false` makes both immediate. Setting either duration to `0`
skips only its own transition: a zero marker duration does not skip the window
collapse, and a zero collapse duration does not skip marker appearance. Window restoration still uses the configured window-open
animation. Landmark collision relocation uses the old 520ms damped slide;
labels independently use the old back-loaded hover slide/grow/fade and request
frames until settled.

Field maximize uses the same motion controls and live texture crossfade as
fullscreen. Its default remains the original 240 ms ease-in-out cubic motion:

```rune
animations:
  maximize:
    enabled true
    motion "easing"
    duration-ms 240
    curve "ease-in-out-cubic"
  end
end
```

Set `motion "spring"` to use `damping-ratio` and `stiffness`, exactly as for
fullscreen. Reversing a transition keeps the current visual position and
velocity instead of restarting it.

`animations.enabled` and `animations.maximize.enabled` both gate the motion.
Disabling it keeps the maximize state and camera ownership but applies the
endpoints immediately. A zero easing duration also snaps directly to the
endpoint.

Field arrangement has its own slightly slower default so a group of windows
moves as one readable gesture:

```rune
animations:
  arrange:
    enabled true
    motion "easing"
    duration-ms 360
    curve "ease-in-out-cubic"
  end
end
```

`animations.enabled` and `animations.arrange.enabled` both gate the visual
motion. Disabling either keeps `Mod+A` toggle semantics but applies arrange and
restore geometry immediately. Arrangement accepts the same easing and spring
motion knobs as maximize/fullscreen. Window contents, decorations, and input hit
regions share the interpolated rectangle; a rapid toggle starts the return from
the current intermediate presentation rather than snapping to either endpoint.
