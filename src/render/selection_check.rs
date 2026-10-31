use std::error::Error;

use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ContextId, ImportMem, Renderer};
use smithay::utils::{Physical, Rectangle};

use super::window_texture::WindowTexture;

const RASTER_SIZE: u32 = 64;
const CHECK_SVG: &[u8] = include_bytes!("../../assets/check.svg");

#[derive(Default)]
pub struct SelectionCheckRenderer {
    context: Option<ContextId<GlesTexture>>,
    rgb: Option<[u8; 3]>,
    texture: Option<WindowTexture>,
    failed: bool,
}

impl SelectionCheckRenderer {
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        id: Id,
        destination: Rectangle<i32, Physical>,
        rgb: [u8; 3],
        alpha: f32,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        if destination.size.w <= 0 || destination.size.h <= 0 || alpha <= 0.0 {
            return None;
        }
        if let Err(err) = self.ensure(renderer, rgb) {
            if !self.failed {
                eventline::warn!("cluster composer: failed to prepare bundled check icon: {err}");
                self.failed = true;
            }
            return None;
        }
        self.texture
            .as_ref()
            .map(|texture| texture.render_element(id, destination, alpha.clamp(0.0, 1.0)))
    }

    fn ensure(&mut self, renderer: &mut GlesRenderer, rgb: [u8; 3]) -> Result<(), Box<dyn Error>> {
        let context = renderer.context_id();
        if self.context.as_ref() == Some(&context)
            && self.rgb == Some(rgb)
            && self.texture.is_some()
        {
            return Ok(());
        }
        let pixels =
            rasterize_svg(CHECK_SVG, rgb).ok_or("bundled check SVG produced no visible pixels")?;
        let texture = renderer.import_memory(
            &pixels,
            Fourcc::Abgr8888,
            (RASTER_SIZE as i32, RASTER_SIZE as i32).into(),
            false,
        )?;
        self.context = Some(context.clone());
        self.rgb = Some(rgb);
        self.texture = Some(WindowTexture { texture, context });
        self.failed = false;
        Ok(())
    }
}

fn rasterize_svg(svg: &[u8], rgb: [u8; 3]) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let svg_size = tree.size().to_int_size();
    if svg_size.width() == 0 || svg_size.height() == 0 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(RASTER_SIZE, RASTER_SIZE)?;
    let scale_x = RASTER_SIZE as f32 / svg_size.width() as f32;
    let scale_y = RASTER_SIZE as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (RASTER_SIZE as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (RASTER_SIZE as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut pixels = pixmap.take();
    let mut visible = false;
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        visible |= alpha != 0;
        pixel[0] = ((u16::from(rgb[0]) * u16::from(alpha)) / 255) as u8;
        pixel[1] = ((u16::from(rgb[1]) * u16::from(alpha)) / 255) as u8;
        pixel[2] = ((u16::from(rgb[2]) * u16::from(alpha)) / 255) as u8;
    }
    visible.then_some(pixels)
}
