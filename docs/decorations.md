# Window decorations

Halley draws one compositor-owned primary border around each managed top-level
window. The border and client content share a concentric rounded shape:

```rune
decorations:
  border:
    size 3
    radius 8
    colour-focused "#f4f5f7"
    colour-unfocused "#474d59"
  end

  resize-using-border true

  titlebars:
    enabled true
    button-position "right"
    title-position "center"
    show-buttons true
    show-icons false
    show-title true
    # text-size 11
    radius 8
    height 32

    colour-focused "#d65d26"
    colour-unfocused "#474d59"
    foreground-colour-focused "#101418"
    foreground-colour-unfocused "#f4f5f7"
    button-hover-colour "#ffffff"
    button-pressed-colour "#101418"
  end
end
```

`size` and `radius` are logical pixels, multiplied by the display scale, and follow camera zoom. `radius` describes
the client-content corner; the outer border radius is increased by `size`.
Setting `radius` to `0` restores square corners. Setting `size` to `0` hides the
border while retaining rounded client content.

`resize-using-border` lets a managed window be resized by left-dragging its
outer edge or corner. The pointer uses an eight-pixel minimum on-screen grab
band, so thin and hidden borders remain usable. Set it to `false` to keep only
the compositor's modifier-based resize gesture.

An enabled titlebar supplies the top edge of a server-decorated window. The
border then paints only its left, right, and bottom edges. `titlebars.radius`
rounds only the titlebar's top corners; `border.radius` continues to round only
the client body's bottom corners. The requested titlebar `height` is clamped to
1-96 pixels and is raised internally when enabled buttons, the application
icon, or title text need more room. `titlebars.text-size` is clamped to 1-96
pixels and overrides the global `font.size` only for window titles; when omitted,
titles continue to inherit the global size.

The titlebar's outer top edge and corners participate in border resizing.
On resizable floating windows, the outermost eight screen pixels take priority
over an overlapping titlebar button, leaving the button interior clickable.
When border resizing is disabled or unavailable, buttons retain their full
hitboxes. The rest of the titlebar keeps its existing move and double-click
behavior.

Title text is ellipsized at 240 pixels, scaled with the window, or sooner when
buttons and the application icon leave less room. A centered title group stays
at the titlebar's true geometric center; controls and the pin badge reduce its
width symmetrically rather than displacing it.

Buttons are ordered close/maximize/minimize on the left and
minimize/maximize/close on the right. Hover and pressed colors tint both the
button glyph and a translucent backplate. A field-maximized window uses the
unmaximize glyph so the button reflects the action it will perform.

`title-position` accepts `"left"`, `"center"`, or `"right"`. The application
icon, when enabled, sits immediately before the title and follows it as one
aligned group. Left- and right-aligned groups use the space not occupied by
window buttons. Centered groups are fixed at the true titlebar center and are
ellipsized as needed so they cannot overlap controls or the pin badge.

Field-maximized windows retain their rounded border. Entering true fullscreen
removes compositor chrome immediately, while the client content and geometry
continue animating. The fullscreen surface is square and eligible for direct
scanout from its first presented frame.

Leaving fullscreen keeps compositor chrome hidden during the early return
motion, then gradually fades it into place from 35% to 85% of the existing
animation timeline. Chrome is fully visible before the final settling tail,
and the fade scales with the configured motion duration. The titlebar, border,
shadow, and pin badge share this opacity; their geometry follows the window
without a separate slide. Fully invisible chrome has no pointer hitboxes.
Disabling fullscreen animations restores chrome immediately. Client-drawn
titlebars remain part of the application's own content.

Window screenshots, window screencasts, Alt+Tab previews, and Apogee previews
include the server titlebar and border. Popups, override-redirect X11
surfaces, and layer-shell surfaces are not decorated.

## Future custom button SVGs

Halley's built-in button artwork and future user-provided artwork are alpha
masks. Source RGB colors are ignored; Halley applies normal, hover, pressed,
and disabled colors. Author each button on a square canvas with a finite square
`viewBox`, a transparent background, one monochrome silhouette, and comfortable
padding around every edge. Convert text and strokes to paths and export as
plain SVG when practical.

Button files must not contain scripts, animation, external references,
embedded bitmap images, fonts, filters, gradients, or masks. Inkscape metadata
is harmless and may remain. Custom file-path settings are not exposed yet;
when they are added, invalid assets will fall back to the matching bundled
button and emit a precise warning.
