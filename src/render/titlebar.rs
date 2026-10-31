use std::collections::HashMap;
use std::error::Error;

use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ContextId, ImportMem, Renderer};
use smithay::utils::{Physical, Rectangle};

use super::window_decoration::{CornerRadii, RoundedTextureElement, WindowDecorationRenderer};
use super::window_texture::WindowTexture;
use crate::titlebar::Control;

const RASTER_SIZE: u32 = 64;
const CLOSE: &[u8] = include_bytes!("../../assets/titlebars/close.svg");
const MINIMIZE: &[u8] = include_bytes!("../../assets/titlebars/minimize.svg");
const MAXIMIZE: &[u8] = include_bytes!("../../assets/titlebars/maximize.svg");
const UNMAXIMIZE: &[u8] = include_bytes!("../../assets/titlebars/unmaximize.svg");

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Icon {
    Close,
    Minimize,
    Maximize,
    Unmaximize,
}

impl Icon {
    fn for_control(control: Control, maximized: bool) -> Self {
        match control {
            Control::Close => Self::Close,
            Control::Minimize => Self::Minimize,
            Control::Maximize if maximized => Self::Unmaximize,
            Control::Maximize => Self::Maximize,
        }
    }
}

#[derive(Default)]
pub struct TitlebarRenderer {
    context: Option<ContextId<GlesTexture>>,
    icons: HashMap<Icon, WindowTexture>,
    failed: bool,
}

/// One titlebar button glyph: which icon, drawn where, in what tint.
#[derive(Clone)]
pub struct ControlRequest {
    pub id: Id,
    pub control: Control,
    /// Selects the unmaximize glyph in place of the maximize one.
    pub maximized: bool,
    pub destination: Rectangle<i32, Physical>,
    pub color: halley_config::BorderColor,
    pub alpha: f32,
}

impl TitlebarRenderer {
    pub fn control_element(
        &mut self,
        renderer: &mut GlesRenderer,
        decorations: &mut WindowDecorationRenderer,
        request: ControlRequest,
    ) -> Option<RoundedTextureElement> {
        let ControlRequest {
            id,
            control,
            maximized,
            destination,
            color,
            alpha,
        } = request;
        if destination.size.w <= 0 || destination.size.h <= 0 || alpha <= 0.0 {
            return None;
        }
        if let Err(err) = self.ensure(renderer) {
            if !self.failed {
                eventline::warn!("titlebars: failed to prepare bundled button masks: {err}");
                self.failed = true;
            }
            return None;
        }
        let icon = self.icons.get(&Icon::for_control(control, maximized))?;
        let texture = icon.texture.clone();
        let base = icon.render_element(id, destination, alpha);
        decorations.texture_element_with_radii(
            renderer,
            base,
            texture,
            destination,
            CornerRadii::default(),
            (color.r, color.g, color.b, 1.0),
        )
    }

    fn ensure(&mut self, renderer: &mut GlesRenderer) -> Result<(), Box<dyn Error>> {
        let context = renderer.context_id();
        if self.context.as_ref() == Some(&context) && self.icons.len() == 4 {
            return Ok(());
        }
        self.context = Some(context.clone());
        self.icons.clear();
        self.failed = false;
        for (icon, source) in [
            (Icon::Close, CLOSE),
            (Icon::Minimize, MINIMIZE),
            (Icon::Maximize, MAXIMIZE),
            (Icon::Unmaximize, UNMAXIMIZE),
        ] {
            let pixels = raster_mask(source).ok_or("bundled SVG did not produce an alpha mask")?;
            let texture = renderer.import_memory(
                &pixels,
                Fourcc::Abgr8888,
                (RASTER_SIZE as i32, RASTER_SIZE as i32).into(),
                false,
            )?;
            self.icons.insert(
                icon,
                WindowTexture {
                    texture,
                    context: context.clone(),
                },
            );
        }
        Ok(())
    }
}

fn raster_mask(source: &[u8]) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_data(source, &usvg::Options::default()).ok()?;
    let size = tree.size().to_int_size();
    if size.width() == 0 || size.height() == 0 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(RASTER_SIZE, RASTER_SIZE)?;
    let scale =
        (RASTER_SIZE as f32 / size.width() as f32).min(RASTER_SIZE as f32 / size.height() as f32);
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (RASTER_SIZE as f32 - size.width() as f32 * scale) / 2.0,
        (RASTER_SIZE as f32 - size.height() as f32 * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.data().to_vec();
    let mut visible = false;
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        visible |= alpha != 0;
        pixel[0] = alpha;
        pixel[1] = alpha;
        pixel[2] = alpha;
    }
    visible.then_some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximized_control_uses_unmaximize_icon() {
        assert_eq!(Icon::for_control(Control::Maximize, false), Icon::Maximize);
        assert_eq!(Icon::for_control(Control::Maximize, true), Icon::Unmaximize);
        assert_eq!(Icon::for_control(Control::Close, true), Icon::Close);
    }
}
