use std::error::Error;

use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ContextId, ImportMem, Renderer};
use smithay::utils::{Physical, Rectangle};

use super::ids::OutputElementIds;
use super::overlays::shell::{OverlayRgb, resolve_visuals};
use super::window_texture::WindowTexture;

const RASTER_SIZE: u32 = 64;
const GLYPH_DIAMETER_FRACTION: f32 = 0.62;
const PIN_SVG: &[u8] = include_bytes!("../../assets/pin.svg");

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PinSlot {
    Window(u64),
    Node(u64),
    Cluster(u64),
    BearingNode(u64),
    BearingCluster(u64),
}

#[derive(Default)]
pub struct PinRenderer {
    context: Option<ContextId<GlesTexture>>,
    palette: Option<([u8; 3], [u8; 3])>,
    texture: Option<WindowTexture>,
    ids: OutputElementIds<PinSlot>,
    failed: bool,
}

impl PinRenderer {
    pub fn begin_scene(&mut self, output: &str) {
        self.ids.advance(output);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &str,
        slot: PinSlot,
        destination: Rectangle<i32, Physical>,
        alpha: f32,
        pins: &halley_config::Pins,
        overlays: &halley_config::Overlays,
        decorations: &halley_config::Decorations,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        if destination.size.w <= 0 || destination.size.h <= 0 || alpha <= 0.0 {
            return None;
        }
        if let Err(err) = self.ensure(renderer, pins, overlays, decorations) {
            if !self.failed {
                eventline::warn!("pins: failed to prepare bundled pin badge: {err}");
                self.failed = true;
            }
            return None;
        }
        let id = self.ids.for_output(output).id(slot);
        self.texture
            .as_ref()
            .map(|texture| texture.render_element(id, destination, alpha))
    }

    fn ensure(
        &mut self,
        renderer: &mut GlesRenderer,
        pins: &halley_config::Pins,
        overlays: &halley_config::Overlays,
        decorations: &halley_config::Decorations,
    ) -> Result<(), Box<dyn Error>> {
        let context = renderer.context_id();
        let palette = pin_palette(pins, overlays, decorations);
        if self.context.as_ref() == Some(&context)
            && self.palette == Some(palette)
            && self.texture.is_some()
        {
            return Ok(());
        }
        let pixels = raster_badge(palette.0, palette.1)
            .ok_or("bundled pin SVG did not produce an alpha mask")?;
        let texture = renderer.import_memory(
            &pixels,
            Fourcc::Abgr8888,
            (RASTER_SIZE as i32, RASTER_SIZE as i32).into(),
            false,
        )?;
        self.context = Some(context.clone());
        self.palette = Some(palette);
        self.texture = Some(WindowTexture { texture, context });
        self.failed = false;
        Ok(())
    }
}

pub fn scaled_radius(pins: &halley_config::Pins, radius: i32) -> i32 {
    ((radius as f32) * pins.size.clamp(0.5, 3.0))
        .round()
        .max(1.0) as i32
}

pub fn landmark_badge_rect(
    pins: &halley_config::Pins,
    center: (i32, i32),
    marker_side: i32,
) -> Rectangle<i32, Physical> {
    let marker_radius = marker_side as f32 / 2.0;
    let radius = scaled_radius(pins, ((marker_radius / 3.0).round() as i32).clamp(7, 12));
    let offset = (marker_radius * 0.78).round() as i32;
    let cx = match pins.corner {
        halley_config::PinBadgeCorner::TopLeft => center.0 - offset,
        halley_config::PinBadgeCorner::TopRight => center.0 + offset,
    };
    let cy = center.1 - offset;
    Rectangle::new(
        (cx - radius, cy - radius).into(),
        (radius * 2, radius * 2).into(),
    )
}

pub fn window_badge_rect(
    pins: &halley_config::Pins,
    bounds: Rectangle<i32, Physical>,
    render_scale: f32,
) -> Rectangle<i32, Physical> {
    let radius = scaled_radius(
        pins,
        ((14.0 * render_scale.sqrt().clamp(0.85, 1.25)).round() as i32).clamp(10, 18),
    );
    let outset = (radius as f32 * 0.25).round() as i32;
    let cx = match pins.corner {
        halley_config::PinBadgeCorner::TopLeft => bounds.loc.x - outset,
        halley_config::PinBadgeCorner::TopRight => bounds.loc.x + bounds.size.w.max(1) + outset,
    };
    let cy = bounds.loc.y - outset;
    Rectangle::new(
        (cx - radius, cy - radius).into(),
        (radius * 2, radius * 2).into(),
    )
}

/// Place a window pin inside its server titlebar, opposite the configured
/// control side. Titlebar ownership takes precedence over the Field corner
/// preference because the badge is part of that window's chrome here.
pub fn window_titlebar_badge_rect(
    pins: &halley_config::Pins,
    titlebar: Rectangle<i32, Physical>,
    button_position: halley_config::TitlebarButtonPosition,
    render_scale: f32,
) -> Rectangle<i32, Physical> {
    let preferred = scaled_radius(
        pins,
        ((14.0 * render_scale.sqrt().clamp(0.85, 1.25)).round() as i32).clamp(10, 18),
    );
    let radius = preferred.min((titlebar.size.h.saturating_sub(4) / 2).max(1));
    let inset = radius + 2;
    let cx = match button_position {
        halley_config::TitlebarButtonPosition::Left => {
            titlebar.loc.x + titlebar.size.w.max(1) - inset
        }
        halley_config::TitlebarButtonPosition::Right => titlebar.loc.x + inset,
    };
    let cy = titlebar.loc.y + titlebar.size.h / 2;
    Rectangle::new(
        (cx - radius, cy - radius).into(),
        (radius * 2, radius * 2).into(),
    )
}

fn pin_palette(
    pins: &halley_config::Pins,
    overlays: &halley_config::Overlays,
    _decorations: &halley_config::Decorations,
) -> ([u8; 3], [u8; 3]) {
    let overlay = resolve_visuals(overlays);
    let fill = resolve_fill(pins.background_color, overlay.fill);
    let glyph = match pins.color {
        halley_config::OverlayColorMode::Auto | halley_config::OverlayColorMode::System => {
            if matches!(
                pins.background_color,
                halley_config::OverlayColorMode::Auto | halley_config::OverlayColorMode::System
            ) {
                overlay.text
            } else {
                contrast_text(fill)
            }
        }
        halley_config::OverlayColorMode::Light => OverlayRgb {
            r: 0.08,
            g: 0.10,
            b: 0.12,
            a: 1.0,
        },
        halley_config::OverlayColorMode::Dark => OverlayRgb {
            r: 0.94,
            g: 0.96,
            b: 0.98,
            a: 1.0,
        },
        halley_config::OverlayColorMode::Fixed { r, g, b, a } => OverlayRgb { r, g, b, a },
    };
    (glyph.bytes(), fill.bytes())
}

fn resolve_fill(mode: halley_config::OverlayColorMode, auto: OverlayRgb) -> OverlayRgb {
    match mode {
        halley_config::OverlayColorMode::Auto | halley_config::OverlayColorMode::System => auto,
        halley_config::OverlayColorMode::Light => OverlayRgb {
            r: 0.92,
            g: 0.95,
            b: 0.98,
            a: 1.0,
        },
        halley_config::OverlayColorMode::Dark => OverlayRgb {
            r: 0.15,
            g: 0.18,
            b: 0.22,
            a: 1.0,
        },
        halley_config::OverlayColorMode::Fixed { r, g, b, a } => OverlayRgb { r, g, b, a },
    }
}

fn contrast_text(fill: OverlayRgb) -> OverlayRgb {
    if fill.r * 0.2126 + fill.g * 0.7152 + fill.b * 0.0722 < 0.45 {
        OverlayRgb {
            r: 0.94,
            g: 0.96,
            b: 0.98,
            a: 1.0,
        }
    } else {
        OverlayRgb {
            r: 0.08,
            g: 0.10,
            b: 0.12,
            a: 1.0,
        }
    }
}

fn raster_badge(glyph: [u8; 3], fill: [u8; 3]) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_data(PIN_SVG, &usvg::Options::default()).ok()?;
    let size = tree.size().to_int_size();
    if size.width() == 0 || size.height() == 0 {
        return None;
    }

    let mut background = tiny_skia::Pixmap::new(RASTER_SIZE, RASTER_SIZE)?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(fill[0], fill[1], fill[2], 230);
    let mut path = tiny_skia::PathBuilder::new();
    path.push_circle(
        RASTER_SIZE as f32 / 2.0,
        RASTER_SIZE as f32 / 2.0,
        RASTER_SIZE as f32 / 2.0,
    );
    background.fill_path(
        &path.finish()?,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );

    let mut mask = tiny_skia::Pixmap::new(RASTER_SIZE, RASTER_SIZE)?;
    let diameter = RASTER_SIZE as f32 * GLYPH_DIAMETER_FRACTION;
    let scale = (diameter / size.width() as f32).min(diameter / size.height() as f32);
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (RASTER_SIZE as f32 - size.width() as f32 * scale) / 2.0,
        (RASTER_SIZE as f32 - size.height() as f32 * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut mask.as_mut());

    let mut pixels = background.data().to_vec();
    for (pixel, mask_pixel) in pixels.chunks_exact_mut(4).zip(mask.data().chunks_exact(4)) {
        let glyph_alpha = mask_pixel[3] as u16;
        let inverse = 255 - glyph_alpha;
        for channel in 0..3 {
            pixel[channel] = (glyph[channel] as u16 * glyph_alpha / 255
                + pixel[channel] as u16 * inverse / 255) as u8;
        }
        pixel[3] = (glyph_alpha + pixel[3] as u16 * inverse / 255) as u8;
    }
    Some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_corner_and_size_follow_pin_configuration() {
        let pins = halley_config::Pins {
            corner: halley_config::PinBadgeCorner::TopLeft,
            size: 1.5,
            ..halley_config::Pins::default()
        };
        let rect = landmark_badge_rect(&pins, (100, 80), 60);
        assert_eq!(rect, Rectangle::new((62, 42).into(), (30, 30).into()));
    }

    #[test]
    fn titlebar_badge_uses_the_side_opposite_window_controls() {
        let pins = halley_config::Pins::default();
        let titlebar = Rectangle::new((100, 40).into(), (500, 32).into());

        let opposite_left = window_titlebar_badge_rect(
            &pins,
            titlebar,
            halley_config::TitlebarButtonPosition::Left,
            1.0,
        );
        let opposite_right = window_titlebar_badge_rect(
            &pins,
            titlebar,
            halley_config::TitlebarButtonPosition::Right,
            1.0,
        );

        assert!(opposite_left.loc.x > titlebar.loc.x + titlebar.size.w / 2);
        assert!(opposite_right.loc.x < titlebar.loc.x + titlebar.size.w / 2);
        assert!(titlebar.contains_rect(opposite_left));
        assert!(titlebar.contains_rect(opposite_right));
    }
}
