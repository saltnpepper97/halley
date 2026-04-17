use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::ImportMem;
use smithay::backend::renderer::gles::GlesRenderer;

use crate::compositor::root::Halley;
use crate::render::icon_tint::tint_alpha_mask_image;
use crate::render::state::{ClusterCoreIconCache, NodeAppIconTexture};

const CLUSTER_ICON_RASTER_PX: u32 = 64;
const CLUSTER_ICON_SVG: &[u8] = include_bytes!("../compositor/clusters/assets/clusters.svg");

pub(crate) fn ensure_cluster_core_icon_resources(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn std::error::Error>> {
    let focused = rgba_bytes_from_border_color(st.runtime.tuning.decorations.border.color_focused);
    let unfocused =
        rgba_bytes_from_border_color(st.runtime.tuning.decorations.border.color_unfocused);
    if st
        .ui
        .render_state
        .cache
        .cluster_core_icon_cache
        .focused_color
        == focused
        && st
            .ui
            .render_state
            .cache
            .cluster_core_icon_cache
            .unfocused_color
            == unfocused
        && st
            .ui
            .render_state
            .cache
            .cluster_core_icon_cache
            .focused
            .is_some()
        && st
            .ui
            .render_state
            .cache
            .cluster_core_icon_cache
            .unfocused
            .is_some()
    {
        return Ok(());
    }

    st.ui.render_state.cache.cluster_core_icon_cache = ClusterCoreIconCache {
        focused_color: focused,
        unfocused_color: unfocused,
        focused: load_cluster_icon_texture(renderer, focused)?,
        unfocused: load_cluster_icon_texture(renderer, unfocused)?,
    };
    Ok(())
}

pub(crate) fn cluster_core_icon_texture(st: &Halley, focused: bool) -> Option<&NodeAppIconTexture> {
    if focused {
        st.ui
            .render_state
            .cache
            .cluster_core_icon_cache
            .focused
            .as_ref()
    } else {
        st.ui
            .render_state
            .cache
            .cluster_core_icon_cache
            .unfocused
            .as_ref()
    }
}

fn rgba_bytes_from_border_color(color: halley_config::DecorationBorderColor) -> [u8; 4] {
    [
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]
}

fn load_cluster_icon_texture(
    renderer: &mut GlesRenderer,
    rgba: [u8; 4],
) -> Result<Option<NodeAppIconTexture>, Box<dyn std::error::Error>> {
    let Some(raster) = load_cluster_icon_raster(rgba) else {
        return Ok(None);
    };
    let texture = renderer.import_memory(
        &raster.into_vec(),
        Fourcc::Abgr8888,
        (CLUSTER_ICON_RASTER_PX as i32, CLUSTER_ICON_RASTER_PX as i32).into(),
        false,
    )?;
    Ok(Some(NodeAppIconTexture {
        texture,
        width: CLUSTER_ICON_RASTER_PX as i32,
        height: CLUSTER_ICON_RASTER_PX as i32,
    }))
}

fn load_cluster_icon_raster(rgba: [u8; 4]) -> Option<RgbaImage> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(CLUSTER_ICON_SVG, &options).ok()?;
    let svg_size = tree.size().to_int_size();
    if svg_size.width() == 0 || svg_size.height() == 0 {
        return None;
    }

    let mut pixmap = tiny_skia::Pixmap::new(CLUSTER_ICON_RASTER_PX, CLUSTER_ICON_RASTER_PX)?;
    let scale_x = CLUSTER_ICON_RASTER_PX as f32 / svg_size.width() as f32;
    let scale_y = CLUSTER_ICON_RASTER_PX as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (CLUSTER_ICON_RASTER_PX as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (CLUSTER_ICON_RASTER_PX as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut image = RgbaImage::from_vec(
        CLUSTER_ICON_RASTER_PX,
        CLUSTER_ICON_RASTER_PX,
        pixmap.data().to_vec(),
    )?;
    tint_cluster_icon(&mut image, rgba);
    Some(image)
}

fn tint_cluster_icon(image: &mut RgbaImage, rgba: [u8; 4]) {
    tint_alpha_mask_image(image, rgba);
}
