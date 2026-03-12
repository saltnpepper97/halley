use smithay::{
    backend::renderer::{Color32F, Frame},
    input::pointer::CursorImageSurfaceData,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Physical, Rectangle},
    wayland::compositor::with_states,
};

use super::cursor_theme::SoftwareCursorSprite;
use super::render_utils::draw_rect;

// ---------------------------------------------------------------------------
// Hotspot
// ---------------------------------------------------------------------------

/// Extract the cursor hotspot advertised by a client-side cursor surface.
pub(crate) fn cursor_surface_hotspot(surface: &WlSurface) -> (i32, i32) {
    with_states(surface, |states| {
        states
            .data_map
            .get::<CursorImageSurfaceData>()
            .and_then(|attrs| {
                attrs
                    .lock()
                    .ok()
                    .map(|attr| (attr.hotspot.x, attr.hotspot.y))
            })
            .unwrap_or((0, 0))
    })
}

// ---------------------------------------------------------------------------
// Software sprite rasterisation
// ---------------------------------------------------------------------------

/// Blit a software cursor sprite to `frame` using run-length compressed rows.
///
/// Coordinates are in the same screen-space used for hit-testing so the
/// rendered cursor stays aligned with pointer events on every backend.
pub(crate) fn draw_cursor_sprite<F: Frame>(
    frame: &mut F,
    damage: Rectangle<i32, Physical>,
    cursor_screen: (f32, f32),
    sprite: &SoftwareCursorSprite,
) -> Result<(), F::Error> {
    let (sx, sy) = cursor_screen;
    let x0 = sx.round() as i32 - sprite.hotspot_x;
    let y0 = sy.round() as i32 - sprite.hotspot_y;
    let w = sprite.width;
    let h = sprite.height;

    for y in 0..h {
        let mut x = 0usize;
        while x < w {
            let base = (y * w + x) * 4;
            let a = sprite.pixels_rgba[base + 3];
            if a == 0 {
                x += 1;
                continue;
            }

            let r = sprite.pixels_rgba[base];
            let g = sprite.pixels_rgba[base + 1];
            let b = sprite.pixels_rgba[base + 2];

            // Merge identical neighbouring pixels into a single rect call.
            let mut run_end = x + 1;
            while run_end < w {
                let i = (y * w + run_end) * 4;
                if sprite.pixels_rgba[i] != r
                    || sprite.pixels_rgba[i + 1] != g
                    || sprite.pixels_rgba[i + 2] != b
                    || sprite.pixels_rgba[i + 3] != a
                {
                    break;
                }
                run_end += 1;
            }

            draw_rect(
                frame,
                x0 + x as i32,
                y0 + y as i32,
                (run_end - x) as i32,
                1,
                Color32F::new(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    a as f32 / 255.0,
                ),
                damage,
            )?;
            x = run_end;
        }
    }
    Ok(())
}
