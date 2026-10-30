# Window and layer-shell rules

Window rules apply one shared policy to managed native Wayland and XWayland
windows. X11 override-redirect menus and tooltips are not managed windows and
are deliberately excluded.

```rune
rules:
  window-rule:
    app-id ["firefox", r"org\.mozilla\..*"]
    title r"Picture.*"
    width 720
    height 520
    opacity 0.90
    blur true
    # block-screencast true
    spawn-placement "cursor"
    cluster-participation "float"
  end
end
```

Window rules are evaluated from top to bottom and the first matching rule wins.
Strings match exactly and case-sensitively. `r"..."` values are regular
expressions. An array is an OR-list. When both `app-id` and `title` are
present, both fields must match.

For native Wayland, `app-id` is the XDG toplevel app ID. For XWayland it is
the X11 window class. `title` uses the corresponding toplevel/window title.

## Effects

- `width` and `height` set the requested initial size and must appear together.
  Halley enforces a safe minimum of 96 by 72 logical pixels.
- `opacity` accepts `0.0` through `1.0` and applies to the client and its
  popups. Compositor titlebars, borders, pin badges, and shadows stay
  opaque. Open and close animations still fade the whole window.
- `blur true` explicitly enables full-surface compositor blur for the match;
  a client-provided nonempty background-effect region remains region-limited.
  Full-window blur follows the rendered client rectangle through fullscreen
  and maximize transitions, independently of client buffer resize timing.
- `blur false` disables all blur for the match, including client background-
  effect requests. With no blur rule, Halley honors explicit client regions
  only; opacity never enables blur implicitly.
- `spawn-placement` controls only initial placement. Existing windows are never
  moved by a rule reload or identity change.
- `block-screencast true` omits the window and its popups from portal monitor shares, revealing the content behind them and prevents choosing it for a new window share. A window already being shared becomes black if the rule is enabled. It defaults to `false` and does not block screenshots or direct capture protocols.
- `game true` preserves fullscreen state and client size when explicit
  navigation selects another window on the same monitor and parks the
  presentation. Navigation to other monitors keeps the game fullscreen. A
  direct click resumes a parked game; in hover-focus mode, pointer re-entry
  from another monitor also resumes it. Same-monitor hover does not resume it.
  `game false` forces ordinary application behavior: same-monitor navigation
  exits fullscreen and returning leaves the window restored. Cross-monitor
  navigation preserves fullscreen for every application, regardless of this rule. Omit it for automatic detection
  through Wayland game content hints, Steam game window/launch IDs, or a matching
  desktop entry's `Game` category (desktop ID or `StartupWMClass`). Unknown
  applications use ordinary behavior. Desktop entries mixing `Game` with
  another main category, such as `Network` or `Utility`, are ambiguous and do
  not identify launcher windows as games. Detection does not infer games from
  XWayland, titles, fullscreen, or pointer grabs. Desktop metadata loads in the
  background at startup and configuration reload; launch IDs are read only from
  the window's own process. Rules take precedence over all automatic hints.

Placement values are:

- `viewport-center`: center in the current output camera view;
- `cursor`: center under the pointer;
- `center`: center on the parent window, falling back to the viewport;
- `adjacent`: use the first free right, left, below, or above position beside
  the parent or focused window, then fall back to the viewport;
- `app`: center on the parent, otherwise try adjacent to the focused window,
  then fall back to the viewport.

Omitting `spawn-placement` uses the same viewport-centered placement as
`viewport-center`.

`cluster-participation` accepts `layout` or `float`. When a window maps on an
output with an active cluster, `layout` admits it into that cluster's current
layout and `float` leaves it as an ordinary Field window above the workspace.
This initial admission rule remains separate from `cluster-toggle-float`, which
temporarily floats an existing cluster member without removing its membership.
The old `overlap-policy` field is accepted only as a migration aid and has no
runtime effect; new configurations should omit it.

Live reload re-resolves visual fields such as opacity, blur, and screencast privacy for mapped
windows, and updates game policy for the next navigation. Initial size and
placement remain initial-map decisions.

Blur activation is entirely rule-driven: `effects.blur` configures quality and
whether compositor-owned overlays may blur, but has no global enable switch.
See [backdrop blur and shadows](effects.md) for method selection, radius,
passes, saturation, and noise.
An old `rules.rule` entry must be renamed to `rules.window-rule`; the old
`effects.blur.enabled` setting has been removed.

## Layer-shell rules

Layer-shell roots use `layer-rule` entries in the same ordered `rules:` block.
They match the protocol namespace and/or current layer; strings are exact and
case-sensitive, `r"..."` values are regular expressions, and arrays are OR
lists. When both fields are supplied, both must match. The first matching rule
wins.

```rune
rules:
  layer-rule:
    namespace ["waybar", r"^fuzzel$"]
    layer ["top", "overlay"]
    blur true
    # block-screencast true
  end
end
```

`layer` accepts `background`, `bottom`, `top`, or `overlay`. A layer rule must
have `namespace` and/or `layer`, and must set `blur` and/or `block-screencast`. A protected layer and its popups are omitted from portal monitor shares. The resolved policy also
applies to that layer root's popups; popup background-effect regions remain
region-limited when blur is enabled.
