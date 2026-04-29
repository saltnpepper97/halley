use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesFrame;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Color32F, ImportMem};
use smithay::utils::{Buffer, Physical, Rectangle, Transform};

use crate::compositor::root::Halley;
use crate::render::icon_tint::tint_alpha_mask_image;
use crate::render::state::{NodeAppIconTexture, PinIconCache};

const PIN_ICON_RASTER_PX: u32 = 64;
const PIN_SVG: &[u8] = include_bytes!("assets/pin.svg");

#[derive(Clone, Copy, Debug)]
pub(crate) struct PinBadgeLayout {
    pub(crate) cx: i32,
    pub(crate) cy: i32,
    pub(crate) radius: i32,
    pub(crate) alpha: f32,
}

pub(crate) fn ensure_pin_icon_resources(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn std::error::Error>> {
    let color = pin_rgba(&st.runtime.tuning);
    if st.ui.render_state.cache.pin_icon_cache.color == color
        && st.ui.render_state.cache.pin_icon_cache.icon.is_some()
    {
        return Ok(());
    }

    st.ui.render_state.cache.pin_icon_cache = PinIconCache {
        color,
        icon: load_pin_icon_texture(renderer, color)?,
    };
    Ok(())
}

pub(crate) fn pin_icon_texture(st: &Halley) -> Option<&NodeAppIconTexture> {
    st.ui.render_state.cache.pin_icon_cache.icon.as_ref()
}

pub(crate) fn pin_badge_fill_color(
    st: &Halley,
    alpha: f32,
) -> smithay::backend::renderer::Color32F {
    let (fill, _) = crate::overlay::overlay_fill_and_text_colors(&st.runtime.tuning);
    smithay::backend::renderer::Color32F::new(fill.r(), fill.g(), fill.b(), alpha)
}

pub(crate) fn draw_pin_badges(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    layouts: &[PinBadgeLayout],
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn std::error::Error>> {
    for layout in layouts {
        draw_pin_badge(frame, st, *layout, damage)?;
    }
    Ok(())
}

pub(crate) fn draw_pin_badge(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    layout: PinBadgeLayout,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn std::error::Error>> {
    let alpha = layout.alpha.clamp(0.0, 1.0);
    if alpha <= 0.01 {
        return Ok(());
    }

    let fill = pin_badge_fill_color(st, 0.90 * alpha);
    super::node::draw_shader_circle(
        frame,
        st,
        layout.cx,
        layout.cy,
        layout.radius.max(1),
        super::node::NodeRoundShape::Circle,
        alpha,
        Color32F::new(fill.r(), fill.g(), fill.b(), 0.0),
        fill,
        false,
        false,
        damage,
    )?;

    let Some(icon) = pin_icon_texture(st) else {
        return Ok(());
    };
    let side = ((layout.radius * 2) as f32 * 0.78).round().max(1.0) as i32;
    let dest = Rectangle::<i32, Physical>::new(
        (layout.cx - side / 2, layout.cy - side / 2).into(),
        (side, side).into(),
    );
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (icon.width as f64, icon.height as f64).into(),
    );
    frame.render_texture_from_to(
        &icon.texture,
        src,
        dest,
        &[damage],
        &[],
        Transform::Normal,
        alpha,
        None,
        &[],
    )?;
    Ok(())
}

fn pin_rgba(tuning: &halley_config::RuntimeTuning) -> [u8; 4] {
    let color = pin_glyph_rgb(tuning);
    [
        (color.0.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.1.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.2.clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]
}

fn pin_glyph_rgb(tuning: &halley_config::RuntimeTuning) -> (f32, f32, f32) {
    let color = match tuning.pins.color {
        halley_config::OverlayColorMode::Auto => {
            let (_, text) = crate::overlay::overlay_fill_and_text_colors(tuning);
            return (text.r(), text.g(), text.b());
        }
        halley_config::OverlayColorMode::Light => (0.08, 0.10, 0.12),
        halley_config::OverlayColorMode::Dark => (0.94, 0.96, 0.98),
        halley_config::OverlayColorMode::Fixed { r, g, b } => (r, g, b),
    };
    (
        color.0.clamp(0.0, 1.0),
        color.1.clamp(0.0, 1.0),
        color.2.clamp(0.0, 1.0),
    )
}

fn load_pin_icon_texture(
    renderer: &mut GlesRenderer,
    rgba: [u8; 4],
) -> Result<Option<NodeAppIconTexture>, Box<dyn std::error::Error>> {
    let Some(raster) = load_pin_icon_raster(rgba) else {
        return Ok(None);
    };
    let texture = renderer.import_memory(
        &raster.into_vec(),
        Fourcc::Abgr8888,
        (PIN_ICON_RASTER_PX as i32, PIN_ICON_RASTER_PX as i32).into(),
        false,
    )?;
    Ok(Some(NodeAppIconTexture {
        texture,
        width: PIN_ICON_RASTER_PX as i32,
        height: PIN_ICON_RASTER_PX as i32,
    }))
}

fn load_pin_icon_raster(rgba: [u8; 4]) -> Option<RgbaImage> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(PIN_SVG, &options).ok()?;
    let svg_size = tree.size().to_int_size();
    if svg_size.width() == 0 || svg_size.height() == 0 {
        return None;
    }

    let mut pixmap = tiny_skia::Pixmap::new(PIN_ICON_RASTER_PX, PIN_ICON_RASTER_PX)?;
    let scale_x = PIN_ICON_RASTER_PX as f32 / svg_size.width() as f32;
    let scale_y = PIN_ICON_RASTER_PX as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (PIN_ICON_RASTER_PX as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (PIN_ICON_RASTER_PX as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut image = RgbaImage::from_vec(
        PIN_ICON_RASTER_PX,
        PIN_ICON_RASTER_PX,
        pixmap.data().to_vec(),
    )?;
    tint_alpha_mask_image(&mut image, rgba);
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_pin_color_controls_glyph_rgba() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.pins.color = halley_config::OverlayColorMode::Fixed {
            r: 1.0,
            g: 0.5,
            b: 0.0,
        };

        assert_eq!(pin_rgba(&tuning), [255, 128, 0, 255]);
    }
}
