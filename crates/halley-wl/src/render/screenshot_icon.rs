use halley_config::{DecorationBorderColor, OverlayColorMode, RuntimeTuning};
use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::ImportMem;
use smithay::backend::renderer::gles::GlesRenderer;

use crate::compositor::root::Halley;
use crate::render::icon_tint::tint_alpha_mask_image;
use crate::render::state::{NodeAppIconTexture, ScreenshotMenuIconCache};

const ICON_RASTER_PX: u32 = 48;
const ACTIVE_ICON_ALPHA: u8 = 255;
const INACTIVE_ICON_ALPHA: u8 = 184;
const REGION_SVG: &[u8] = include_bytes!("../overlay/assets/region.svg");
const SCREEN_SVG: &[u8] = include_bytes!("../overlay/assets/screen.svg");
const WINDOW_SVG: &[u8] = include_bytes!("../overlay/assets/window.svg");

pub(crate) fn ensure_screenshot_menu_icon_resources(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn std::error::Error>> {
    let active = screenshot_menu_active_rgba(&st.runtime.tuning);
    let inactive = screenshot_menu_inactive_rgba(&st.runtime.tuning);
    let cache = &st.ui.render_state.screenshot_menu_icon_cache;
    if cache.active_color == active
        && cache.inactive_color == inactive
        && cache.region_active.is_some()
        && cache.region_inactive.is_some()
        && cache.screen_active.is_some()
        && cache.screen_inactive.is_some()
        && cache.window_active.is_some()
        && cache.window_inactive.is_some()
    {
        return Ok(());
    }

    st.ui.render_state.screenshot_menu_icon_cache = ScreenshotMenuIconCache {
        active_color: active,
        inactive_color: inactive,
        region_active: load_svg_icon_texture(renderer, REGION_SVG, active)?,
        region_inactive: load_svg_icon_texture(renderer, REGION_SVG, inactive)?,
        screen_active: load_svg_icon_texture(renderer, SCREEN_SVG, active)?,
        screen_inactive: load_svg_icon_texture(renderer, SCREEN_SVG, inactive)?,
        window_active: load_svg_icon_texture(renderer, WINDOW_SVG, active)?,
        window_inactive: load_svg_icon_texture(renderer, WINDOW_SVG, inactive)?,
    };
    Ok(())
}

pub(crate) fn screenshot_menu_icon_texture(
    st: &Halley,
    mode: halley_ipc::CaptureMode,
    active: bool,
) -> Option<&NodeAppIconTexture> {
    let cache = &st.ui.render_state.screenshot_menu_icon_cache;
    match (mode, active) {
        (halley_ipc::CaptureMode::Region, true) => cache.region_active.as_ref(),
        (halley_ipc::CaptureMode::Region, false) => cache.region_inactive.as_ref(),
        (halley_ipc::CaptureMode::Screen, true) => cache.screen_active.as_ref(),
        (halley_ipc::CaptureMode::Screen, false) => cache.screen_inactive.as_ref(),
        (halley_ipc::CaptureMode::Window, true) => cache.window_active.as_ref(),
        (halley_ipc::CaptureMode::Window, false) => cache.window_inactive.as_ref(),
        _ => None,
    }
}

const LIGHT_OVERLAY_FILL: DecorationBorderColor = DecorationBorderColor {
    r: 0.92,
    g: 0.95,
    b: 0.98,
};
const DARK_OVERLAY_FILL: DecorationBorderColor = DecorationBorderColor {
    r: 0.15,
    g: 0.18,
    b: 0.22,
};
const LIGHT_OVERLAY_TEXT: DecorationBorderColor = DecorationBorderColor {
    r: 0.08,
    g: 0.10,
    b: 0.12,
};
const DARK_OVERLAY_TEXT: DecorationBorderColor = DecorationBorderColor {
    r: 0.94,
    g: 0.96,
    b: 0.98,
};

pub(crate) fn screenshot_menu_background_color(tuning: &RuntimeTuning) -> DecorationBorderColor {
    match tuning.screenshot.background_color {
        OverlayColorMode::Auto | OverlayColorMode::Light => LIGHT_OVERLAY_FILL,
        OverlayColorMode::Dark => DARK_OVERLAY_FILL,
        OverlayColorMode::Fixed { r, g, b } => DecorationBorderColor { r, g, b },
    }
}

pub(crate) fn screenshot_menu_highlight_color(tuning: &RuntimeTuning) -> DecorationBorderColor {
    let bg = screenshot_menu_background_color(tuning);
    match tuning.screenshot.highlight_color {
        OverlayColorMode::Auto => {
            let luminance = bg.r * 0.2126 + bg.g * 0.7152 + bg.b * 0.0722;
            if luminance < 0.45 {
                DARK_OVERLAY_TEXT
            } else {
                LIGHT_OVERLAY_TEXT
            }
        }
        OverlayColorMode::Light => LIGHT_OVERLAY_TEXT,
        OverlayColorMode::Dark => DARK_OVERLAY_TEXT,
        OverlayColorMode::Fixed { r, g, b } => DecorationBorderColor { r, g, b },
    }
}

pub(crate) fn screenshot_menu_item_fill_color(tuning: &RuntimeTuning) -> DecorationBorderColor {
    mix_color(
        screenshot_menu_background_color(tuning),
        screenshot_menu_highlight_color(tuning),
        0.10,
    )
}

fn mix_color(
    a: DecorationBorderColor,
    b: DecorationBorderColor,
    amount: f32,
) -> DecorationBorderColor {
    let t = amount.clamp(0.0, 1.0);
    DecorationBorderColor {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
    }
}

fn screenshot_menu_active_rgba(tuning: &RuntimeTuning) -> [u8; 4] {
    rgba_bytes_from_overlay_color(screenshot_menu_highlight_color(tuning), ACTIVE_ICON_ALPHA)
}

fn screenshot_menu_inactive_rgba(tuning: &RuntimeTuning) -> [u8; 4] {
    rgba_bytes_from_overlay_color(screenshot_menu_highlight_color(tuning), INACTIVE_ICON_ALPHA)
}

fn rgba_bytes_from_overlay_color(
    color: halley_config::DecorationBorderColor,
    alpha: u8,
) -> [u8; 4] {
    [
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        alpha,
    ]
}

#[cfg(test)]
mod tests {
    use halley_config::{OverlayColorMode, RuntimeTuning};

    use super::{
        screenshot_menu_active_rgba, screenshot_menu_background_color,
        screenshot_menu_highlight_color, screenshot_menu_inactive_rgba,
    };

    #[test]
    fn screenshot_auto_palette_uses_light_overlay_background_and_dark_highlight() {
        let tuning = RuntimeTuning::default();

        let bg = screenshot_menu_background_color(&tuning);
        let highlight = screenshot_menu_highlight_color(&tuning);

        assert_eq!((bg.r, bg.g, bg.b), (0.92, 0.95, 0.98));
        assert_eq!((highlight.r, highlight.g, highlight.b), (0.08, 0.10, 0.12));
    }

    #[test]
    fn screenshot_active_rgba_matches_exact_configured_highlight() {
        let mut tuning = RuntimeTuning::default();
        tuning.screenshot.highlight_color = OverlayColorMode::Fixed {
            r: 0.90,
            g: 0.80,
            b: 0.10,
        };

        assert_eq!(screenshot_menu_active_rgba(&tuning), [230, 204, 26, 255]);
    }

    #[test]
    fn screenshot_inactive_rgba_does_not_mix_with_background_color() {
        let mut tuning_a = RuntimeTuning::default();
        tuning_a.screenshot.background_color = OverlayColorMode::Fixed {
            r: 0.20,
            g: 0.30,
            b: 0.40,
        };
        tuning_a.screenshot.highlight_color = OverlayColorMode::Fixed {
            r: 0.90,
            g: 0.80,
            b: 0.10,
        };

        let mut tuning_b = tuning_a.clone();
        tuning_b.screenshot.background_color = OverlayColorMode::Fixed {
            r: 0.02,
            g: 0.04,
            b: 0.90,
        };

        assert_eq!(
            screenshot_menu_inactive_rgba(&tuning_a),
            [230, 204, 26, 184]
        );
        assert_eq!(
            screenshot_menu_inactive_rgba(&tuning_b),
            [230, 204, 26, 184]
        );
    }
}

fn load_svg_icon_texture(
    renderer: &mut GlesRenderer,
    svg: &[u8],
    rgba: [u8; 4],
) -> Result<Option<NodeAppIconTexture>, Box<dyn std::error::Error>> {
    let Some(raster) = load_svg_raster(svg, rgba) else {
        return Ok(None);
    };
    let texture = renderer.import_memory(
        &raster.into_vec(),
        Fourcc::Abgr8888,
        (ICON_RASTER_PX as i32, ICON_RASTER_PX as i32).into(),
        false,
    )?;
    Ok(Some(NodeAppIconTexture {
        texture,
        width: ICON_RASTER_PX as i32,
        height: ICON_RASTER_PX as i32,
    }))
}

fn load_svg_raster(svg: &[u8], rgba: [u8; 4]) -> Option<RgbaImage> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(svg, &options).ok()?;
    let svg_size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(ICON_RASTER_PX, ICON_RASTER_PX)?;
    let scale_x = ICON_RASTER_PX as f32 / svg_size.width() as f32;
    let scale_y = ICON_RASTER_PX as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (ICON_RASTER_PX as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (ICON_RASTER_PX as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut image = RgbaImage::from_vec(ICON_RASTER_PX, ICON_RASTER_PX, pixmap.data().to_vec())?;
    tint_alpha_mask_image(&mut image, rgba);
    Some(image)
}
