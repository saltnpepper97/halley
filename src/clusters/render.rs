use std::collections::HashMap;
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};

use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::{ContextId, ImportMem, Renderer, Texture};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform};

const CIRCLE_SHADER: &str = include_str!("shaders/node_circle_shader.frag");
const CLUSTER_ICON: &[u8] = include_bytes!("assets/clusters.svg");
const EDIT_ICON: &[u8] = include_bytes!("../../assets/edit.svg");
const CLOSE_ICON: &[u8] = include_bytes!("../../assets/titlebars/close.svg");
const ICON_SIZE: u32 = 64;
const JOIN_READY_BORDER_WIDTH_PX: f32 = 5.0;

struct Resources {
    context: ContextId<GlesTexture>,
    texture: GlesTexture,
    circle: GlesTexProgram,
    icon_colors: [[u8; 4]; 2],
    icons: [GlesTexture; 2],
}

#[derive(Default)]
pub struct ClusterRenderer {
    resources: Option<Resources>,
    edit_context: Option<ContextId<GlesTexture>>,
    edit_color: Option<[u8; 4]>,
    edit_texture: Option<GlesTexture>,
    close_context: Option<ContextId<GlesTexture>>,
    close_color: Option<[u8; 4]>,
    close_texture: Option<GlesTexture>,
    ids: crate::render::ids::OutputElementIds<(u8, u32)>,
    active_output: String,
    occurrences: HashMap<u8, u32>,
}

#[derive(Debug)]
pub struct ClusterCoreElement {
    base: TextureRenderElement<GlesTexture>,
    texture: GlesTexture,
    program: GlesTexProgram,
    border: (f32, f32, f32, f32),
    fill: (f32, f32, f32, f32),
    fill_alpha: f32,
    element_alpha: f32,
    flat_fill: f32,
    center_flat_fill: f32,
    commit: CommitCounter,
}

#[derive(Debug)]
pub struct ClusterIconElement {
    base: TextureRenderElement<GlesTexture>,
    commit: CommitCounter,
}

impl ClusterRenderer {
    pub fn begin_scene(&mut self, output: &str) {
        self.active_output.clear();
        self.active_output.push_str(output);
        self.occurrences.clear();
        self.ids.advance(output);
    }

    fn dynamic_id(&mut self, kind: u8) -> Id {
        let occurrence = self.occurrences.entry(kind).or_default();
        let index = *occurrence;
        *occurrence = occurrence.wrapping_add(1);
        self.ids.for_output(&self.active_output).id((kind, index))
    }

    pub fn core(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        border_rgb: (f32, f32, f32),
        fill_rgb: (f32, f32, f32),
        opacity: f32,
        join_ready: bool,
    ) -> Result<ClusterCoreElement, Box<dyn Error>> {
        let mut element =
            self.core_with_alpha(renderer, destination, border_rgb, fill_rgb, opacity, 1.0)?;
        if join_ready {
            element.border.3 = join_ready_border_fraction(destination);
            element.commit = core_commit(
                element.border,
                element.fill,
                element.fill_alpha,
                element.flat_fill,
                element.center_flat_fill,
            );
        }
        Ok(element)
    }

    pub fn core_with_alpha(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        border_rgb: (f32, f32, f32),
        fill_rgb: (f32, f32, f32),
        opacity: f32,
        alpha: f32,
    ) -> Result<ClusterCoreElement, Box<dyn Error>> {
        let id = self.dynamic_id(0);
        self.ensure(renderer, [[255; 4]; 2])?;
        let resources = self.resources.as_ref().expect("resources ensured above");
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (
                resources.texture.size().w as f64,
                resources.texture.size().h as f64,
            )
                .into(),
        );
        Ok(ClusterCoreElement {
            base: TextureRenderElement::from_static_texture(
                id,
                resources.context.clone(),
                destination.loc.to_f64(),
                resources.texture.clone(),
                1,
                Transform::Normal,
                Some(1.0),
                Some(source),
                Some(destination.size.to_logical(1)),
                None,
                Kind::Unspecified,
            ),
            texture: resources.texture.clone(),
            program: resources.circle.clone(),
            border: (border_rgb.0, border_rgb.1, border_rgb.2, 3.0 / 26.0),
            fill: (fill_rgb.0, fill_rgb.1, fill_rgb.2, 1.0),
            fill_alpha: opacity.clamp(0.0, 1.0),
            element_alpha: alpha.clamp(0.0, 1.0),
            flat_fill: 0.0,
            center_flat_fill: 0.0,
            commit: core_commit(
                (border_rgb.0, border_rgb.1, border_rgb.2, 3.0 / 26.0),
                (fill_rgb.0, fill_rgb.1, fill_rgb.2, 1.0),
                opacity.clamp(0.0, 1.0),
                0.0,
                0.0,
            ),
        })
    }

    pub fn icon(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        focused: bool,
        colors: [[u8; 4]; 2],
        alpha: f32,
    ) -> Result<ClusterIconElement, Box<dyn Error>> {
        let id = self.dynamic_id(1);
        self.ensure(renderer, colors)?;
        let resources = self.resources.as_ref().expect("resources ensured above");
        let texture = resources.icons[usize::from(focused)].clone();
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (texture.size().w as f64, texture.size().h as f64).into(),
        );
        Ok(ClusterIconElement {
            base: TextureRenderElement::from_static_texture(
                id,
                resources.context.clone(),
                destination.loc.to_f64(),
                texture,
                1,
                Transform::Normal,
                Some(alpha.clamp(0.0, 1.0)),
                Some(source),
                Some(destination.size.to_logical(1)),
                None,
                Kind::Unspecified,
            ),
            commit: icon_commit(focused, colors),
        })
    }

    pub fn edit_icon(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        color: [u8; 4],
        alpha: f32,
    ) -> Result<ClusterIconElement, Box<dyn Error>> {
        let context = renderer.context_id();
        if self.edit_context.as_ref() != Some(&context)
            || self.edit_color != Some(color)
            || self.edit_texture.is_none()
        {
            let raster =
                raster_svg_icon(EDIT_ICON, color).ok_or("edit SVG could not be rasterized")?;
            let texture = renderer.import_memory(
                raster.as_raw(),
                Fourcc::Abgr8888,
                (ICON_SIZE as i32, ICON_SIZE as i32).into(),
                false,
            )?;
            self.edit_context = Some(context.clone());
            self.edit_color = Some(color);
            self.edit_texture = Some(texture);
        }
        let texture = self
            .edit_texture
            .as_ref()
            .expect("edit texture ensured")
            .clone();
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (texture.size().w as f64, texture.size().h as f64).into(),
        );
        Ok(ClusterIconElement {
            base: TextureRenderElement::from_static_texture(
                self.dynamic_id(2),
                context,
                destination.loc.to_f64(),
                texture,
                1,
                Transform::Normal,
                Some(alpha.clamp(0.0, 1.0)),
                Some(source),
                Some(destination.size.to_logical(1)),
                None,
                Kind::Unspecified,
            ),
            commit: icon_commit(false, [color, color]),
        })
    }

    pub fn close_icon(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        color: [u8; 4],
        alpha: f32,
    ) -> Result<ClusterIconElement, Box<dyn Error>> {
        let context = renderer.context_id();
        if self.close_context.as_ref() != Some(&context)
            || self.close_color != Some(color)
            || self.close_texture.is_none()
        {
            let raster =
                raster_svg_icon(CLOSE_ICON, color).ok_or("close SVG could not be rasterized")?;
            let texture = renderer.import_memory(
                raster.as_raw(),
                Fourcc::Abgr8888,
                (ICON_SIZE as i32, ICON_SIZE as i32).into(),
                false,
            )?;
            self.close_context = Some(context.clone());
            self.close_color = Some(color);
            self.close_texture = Some(texture);
        }
        let texture = self
            .close_texture
            .as_ref()
            .expect("close texture ensured")
            .clone();
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (texture.size().w as f64, texture.size().h as f64).into(),
        );
        Ok(ClusterIconElement {
            base: TextureRenderElement::from_static_texture(
                self.dynamic_id(3),
                context,
                destination.loc.to_f64(),
                texture,
                1,
                Transform::Normal,
                Some(alpha.clamp(0.0, 1.0)),
                Some(source),
                Some(destination.size.to_logical(1)),
                None,
                Kind::Unspecified,
            ),
            commit: icon_commit(false, [color, color]),
        })
    }

    fn ensure(
        &mut self,
        renderer: &mut GlesRenderer,
        icon_colors: [[u8; 4]; 2],
    ) -> Result<(), Box<dyn Error>> {
        let context = renderer.context_id();
        if self.resources.as_ref().is_some_and(|resources| {
            resources.context == context && resources.icon_colors == icon_colors
        }) {
            return Ok(());
        }
        let texture =
            renderer.import_memory(&[255_u8; 4 * 4 * 4], Fourcc::Abgr8888, (4, 4).into(), false)?;
        let uniforms = [
            UniformName::new("node_color", UniformType::_4f),
            UniformName::new("fill_color", UniformType::_4f),
            UniformName::new("flat_fill", UniformType::_1f),
            UniformName::new("center_flat_fill", UniformType::_1f),
            UniformName::new("fill_alpha", UniformType::_1f),
        ];
        let circle = renderer.compile_custom_texture_shader(CIRCLE_SHADER, &uniforms)?;
        let [unfocused, focused] = icon_colors.map(|color| {
            let raster = raster_icon(color).ok_or("cluster SVG could not be rasterized")?;
            renderer
                .import_memory(
                    raster.as_raw(),
                    Fourcc::Abgr8888,
                    (ICON_SIZE as i32, ICON_SIZE as i32).into(),
                    false,
                )
                .map_err(|error| -> Box<dyn Error> { Box::new(error) })
        });
        self.resources = Some(Resources {
            context,
            texture,
            circle,
            icon_colors,
            icons: [unfocused?, focused?],
        });
        Ok(())
    }
}

fn join_ready_border_fraction(destination: Rectangle<i32, Physical>) -> f32 {
    let radius = destination.size.w.min(destination.size.h).max(1) as f32 * 0.5;
    JOIN_READY_BORDER_WIDTH_PX / radius
}

fn core_commit(
    border: (f32, f32, f32, f32),
    fill: (f32, f32, f32, f32),
    fill_alpha: f32,
    flat_fill: f32,
    center_flat_fill: f32,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    for value in [
        border.0,
        border.1,
        border.2,
        border.3,
        fill.0,
        fill.1,
        fill.2,
        fill.3,
        fill_alpha,
        flat_fill,
        center_flat_fill,
    ] {
        value.to_bits().hash(&mut hasher);
    }
    CommitCounter::from(hasher.finish() as usize)
}

fn icon_commit(focused: bool, colors: [[u8; 4]; 2]) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    focused.hash(&mut hasher);
    colors.hash(&mut hasher);
    CommitCounter::from(hasher.finish() as usize)
}

fn raster_icon(color: [u8; 4]) -> Option<RgbaImage> {
    raster_svg_icon(CLUSTER_ICON, color)
}

fn raster_svg_icon(svg: &[u8], color: [u8; 4]) -> Option<RgbaImage> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg, &options).ok()?;
    let svg_size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE)?;
    let scale = (ICON_SIZE as f32 / svg_size.width() as f32)
        .min(ICON_SIZE as f32 / svg_size.height() as f32);
    let dx = (ICON_SIZE as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (ICON_SIZE as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut image = RgbaImage::from_vec(ICON_SIZE, ICON_SIZE, pixmap.data().to_vec())?;
    for pixel in image.pixels_mut() {
        let alpha = pixel[3] as u16;
        if alpha == 0 {
            continue;
        }
        let tinted_alpha = ((alpha * color[3] as u16) / 255) as u8;
        pixel[0] = ((color[0] as u16 * tinted_alpha as u16) / 255) as u8;
        pixel[1] = ((color[1] as u16 * tinted_alpha as u16) / 255) as u8;
        pixel[2] = ((color[2] as u16 * tinted_alpha as u16) / 255) as u8;
        pixel[3] = tinted_alpha;
    }
    Some(image)
}

impl Element for ClusterCoreElement {
    fn id(&self) -> &Id {
        self.base.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.base.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.element_alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ClusterCoreElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.element_alpha,
            Some(&self.program),
            &[
                Uniform::new("node_color", self.border),
                Uniform::new("fill_color", self.fill),
                Uniform::new("flat_fill", self.flat_fill),
                Uniform::new("center_flat_fill", self.center_flat_fill),
                Uniform::new("fill_alpha", self.fill_alpha),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl Element for ClusterIconElement {
    fn id(&self) -> &Id {
        self.base.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.base.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.base.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.base.alpha()
    }

    fn kind(&self) -> Kind {
        self.base.kind()
    }
}

impl RenderElement<GlesRenderer> for ClusterIconElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        <TextureRenderElement<GlesTexture> as RenderElement<GlesRenderer>>::draw(
            &self.base,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        <TextureRenderElement<GlesTexture> as RenderElement<GlesRenderer>>::underlying_storage(
            &self.base, renderer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_commits_follow_visual_state() {
        let border = (0.8, 0.2, 0.1, 1.0);
        let fill = (0.1, 0.2, 0.3, 1.0);
        assert_eq!(
            core_commit(border, fill, 0.8, 0.0, 0.0),
            core_commit(border, fill, 0.8, 0.0, 0.0)
        );
        assert_ne!(
            core_commit(border, fill, 0.8, 0.0, 0.0),
            core_commit(border, fill, 0.8, 0.0, 1.0)
        );
        assert_ne!(
            icon_commit(false, [[1, 2, 3, 255], [4, 5, 6, 255]]),
            icon_commit(true, [[1, 2, 3, 255], [4, 5, 6, 255]])
        );
    }

    #[test]
    fn bundled_edit_icon_rasterizes_as_a_tinted_alpha_mask() {
        let color = [32, 64, 96, 255];
        let image = raster_svg_icon(EDIT_ICON, color).expect("edit icon should rasterize");
        assert_eq!(image.dimensions(), (ICON_SIZE, ICON_SIZE));
        assert!(image.pixels().any(|pixel| pixel[3] != 0));
        assert!(
            image.pixels().filter(|pixel| pixel[3] != 0).all(|pixel| {
                pixel[0] <= color[0] && pixel[1] <= color[1] && pixel[2] <= color[2]
            })
        );
    }
}
