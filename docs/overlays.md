# Compositor overlays

The `overlays:` section is the shared style contract for compositor-owned UI:
Apogee title bands, the Alt+Tab rail, Bearings chips, the screenshot picker,
configuration notices, the zoom indicator, the exit confirmation, and the
one-time basics card. It
deliberately does not restyle client window decorations or node labels; those
retain their existing sections.

Overlay text uses the family and size from the global [`font:`](fonts.md)
section. The zoom indicator may override only its size explicitly; every other
overlay continues to use the global typography without private tiers.

```rune
overlays:
  background-colour "auto"
  text-colour "auto"
  error-colour "#fb4934"
  border-colour "#d65d26"
  radius 8
  borders true
  border-size 3

  notifications:
    position "top-center"
    success-duration-ms 4000
    error-duration-ms 9000
  end

  zoom-indicator:
    enabled true
    position "bottom-center"
    hold-duration-ms 750
    fade-duration-ms 180
    background true
    opacity 1.0

    # Optional; omitted values inherit the shared overlay style.
    # text-size 18
    # text-colour "auto"
    # background-colour "auto"
    # border-colour "#d65d26"
    # borders true
    # radius 8
  end
end
```

`background-colour`, `text-colour`, `error-colour`, and `border-colour` accept
`auto`, `system`, `light`, `dark`, or a `#rgb`, `#rgba`, `#rrggbb`, or
`#rrggbbaa` colour. American `color` spellings are accepted as aliases. `auto`
is Halley's deterministic built-in palette; it deliberately does not inspect
the desktop theme. `system` explicitly follows the XDG Settings portal's
`org.freedesktop.appearance` `color-scheme` preference, updates live, and falls
back to `auto` when the portal reports no preference or is unavailable.
`radius` is the overlay content-corner radius in pixels; `0` is square.
`borders false` removes overlay borders. `border-size` sets their width in
pixels and is clamped from `0` to `64`; it is independent of
`decorations.border.size`. `border-colour` is also independent of window
styling and defaults to Halley's orange `#d65d26`.
Apogee and Alt+Tab title bands, monitor badges, `NODE` badges, and tiled-cluster
overflow strips remain borderless regardless of this setting, matching old
Halley's label chrome.
Window preview textures use `decorations.border.radius`; the one surrounding
overlay border uses `overlays.radius`. Both settings use the same content-radius
semantics, so setting both to `8` produces matching curves.

Notification positions are `top-left`, `top-center`, `top-right`,
`bottom-left`, `bottom-center`, and `bottom-right`. Durations are positive
milliseconds. The renderer builds every card at its final pixel dimensions,
so changing its radius or output scale does not stretch a small blurred texture.
After a native screenshot is saved, a success notification shows its destination
directory for `success-duration-ms`.

## Zoom indicator

Camera zoom displays the affected monitor's live scale as `0.75x`, always with
two decimal places. The card appears immediately for keybind, wheel-bound,
reset, or compositor pinch zoom input and continues updating throughout the
smooth camera transition. Repeated input and live scale changes restart its
hold, so it does not begin fading while a longer zoom sequence is still moving.

`enabled false` disables and clears the indicator. `position` accepts the same
six values as notifications. Once input and camera motion have both stopped,
the final value remains fully visible for the positive `hold-duration-ms`, then
fades over the positive `fade-duration-ms`.

`background false` draws only the text. Because there is no card surface in
that mode, its border, blur, and shadow are removed too. `opacity` multiplies
the whole indicator's fade and is clamped from `0.0` to `1.0`. `text-size` is
clamped from `6` to `96` pixels. `text-colour`, `background-colour`,
`border-colour`, `borders`, and `radius`
override the corresponding shared overlay value only when present; otherwise
they inherit it. Both colour spellings are accepted, and colours use the same
`auto`, `system`, `light`, `dark`, and hex formats as the shared palette. A
private `radius` is clamped from `0` to `256` pixels. `borders` and `radius`
remain configured but have no visible effect while `background false`.

## Basics card

A freshly generated configuration's first native session shows one **Halley
basics** card: the Field-first mental model plus the five essential operations
(`Mod+D` Lift, `Mod+Left-drag` move, `Mod+A` arrange, `Mod+N`
collapse/restore, `Mod+O` Apogee). The card spells `Mod` as the configured key
— `Super` on a native session, `Alt` while nested under `--winit`. It reuses the
shared overlay surface,
typography, and transition timing described above, is centered on the focused
output, and never dims the desktop or opens a backdrop.

The card is non-modal. Only `Enter`, `Escape`, and the first pointer press or
touch are captured while it is visible; keys, pointer motion, scroll, and every
other action keep working, and each captured press is matched with its release so
no orphan event reaches a client. `Enter`, `Escape`, or a click fades the card
out and records the dismissal.

Automatic eligibility is deliberately narrow: the configuration must have been
generated by Halley, the session must be a native (non-`--winit`) session, and
the card must not have been dismissed. Dismissal and the pending first-run state
live in `$XDG_STATE_HOME/halley/state.rune` (falling back to
`~/.local/state/halley/state.rune`), which is user state rather than
configuration: it is never migrated, never rewritten by config loading, and safe
to delete. Manual reopening is independent of that state and always works, from
Halley Lift's **Show Halley basics** action or `halleyctl basics`.

## Configuration lifecycle

On the first successful compositor load, including an explicit
`halley -c PATH` or `halley --config PATH`, Halley briefly shows:

```text
Configuration successfully loaded from /absolute/path/to/halley.rune
```

If any live reload is invalid, every rejected reload shows:

```text
Current configuration was unable to load properly. Run `halleyctl config verify` to see why.
```

The last valid configuration remains active. Correcting the file dismisses an
active error notice. File replacement, deletion, recreation, and ordinary
in-place edits are all observed.

Use the terminal verifier for the complete structured diagnostic:

```text
halleyctl config edit
halleyctl config verify
halleyctl config verify -c PATH
halleyctl config verify --config PATH
```

Without `-c`, `halleyctl` asks the running compositor which file it selected.
If no compositor is reachable, it verifies the normal default configuration.
Verification is read-only: success exits 0, a rejected configuration exits 1,
and command or path-discovery errors exit 2.

## Exit confirmation

The `quit` keybind action and `halleyctl quit` open the same compositor-owned
confirmation. Enter stops Halley; Escape cancels. Other keyboard and pointer
input is held away from clients while the modal is active, but the focused
window beneath it is not changed.
