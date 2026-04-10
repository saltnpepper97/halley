use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::ImportMem;
use smithay::backend::renderer::gles::GlesRenderer;

use crate::compositor::root::Halley;
use crate::render::icon_tint::tint_alpha_mask_image;
use crate::render::state::{NodeAppIconTexture, ScreenshotMenuIconCache};

const ICON_RASTER_PX: u32 = 48;
const REGION_SVG: &[u8] = include_bytes!("../overlay/assets/region.svg");
const SCREEN_SVG: &[u8] = include_bytes!("../overlay/assets/screen.svg");
const WINDOW_SVG: &[u8] = include_bytes!("../overlay/assets/window.svg");

pub(crate) fn ensure_screenshot_menu_icon_resources(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn std::error::Error>> {
    let active = rgba_bytes_from_overlay_color(resolve_active_color(st));
    let inactive = rgba_bytes_from_overlay_color(resolve_inactive_color(st));
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

fn resolve_active_color(st: &Halley) -> halley_config::DecorationBorderColor {
    st.runtime.tuning.border_color_focused
}

fn resolve_inactive_color(st: &Halley) -> halley_config::DecorationBorderColor {
    let text = resolve_overlay_text_color(st);
    let bg = resolve_overlay_background_color(st);
    halley_config::DecorationBorderColor {
        r: text.r + (bg.r - text.r) * 0.20,
        g: text.g + (bg.g - text.g) * 0.20,
        b: text.b + (bg.b - text.b) * 0.20,
    }
}

fn resolve_overlay_background_color(st: &Halley) -> halley_config::DecorationBorderColor {
    match st.runtime.tuning.overlay_style.background_color {
        halley_config::OverlayColorMode::Auto | halley_config::OverlayColorMode::Light => {
            halley_config::DecorationBorderColor {
                r: 0.92,
                g: 0.95,
                b: 0.98,
            }
        }
        halley_config::OverlayColorMode::Dark => halley_config::DecorationBorderColor {
            r: 0.15,
            g: 0.18,
            b: 0.22,
        },
        halley_config::OverlayColorMode::Fixed { r, g, b } => {
            halley_config::DecorationBorderColor { r, g, b }
        }
    }
}

fn resolve_overlay_text_color(st: &Halley) -> halley_config::DecorationBorderColor {
    let bg = resolve_overlay_background_color(st);
    match st.runtime.tuning.overlay_style.text_color {
        halley_config::OverlayColorMode::Auto => {
            let luminance = bg.r * 0.2126 + bg.g * 0.7152 + bg.b * 0.0722;
            if luminance < 0.45 {
                halley_config::DecorationBorderColor {
                    r: 0.94,
                    g: 0.96,
                    b: 0.98,
                }
            } else {
                halley_config::DecorationBorderColor {
                    r: 0.08,
                    g: 0.10,
                    b: 0.12,
                }
            }
        }
        halley_config::OverlayColorMode::Light => halley_config::DecorationBorderColor {
            r: 0.08,
            g: 0.10,
            b: 0.12,
        },
        halley_config::OverlayColorMode::Dark => halley_config::DecorationBorderColor {
            r: 0.94,
            g: 0.96,
            b: 0.98,
        },
        halley_config::OverlayColorMode::Fixed { r, g, b } => {
            halley_config::DecorationBorderColor { r, g, b }
        }
    }
}

fn rgba_bytes_from_overlay_color(color: halley_config::DecorationBorderColor) -> [u8; 4] {
    [
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]
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
