use smithay::backend::renderer::{
    element::{
        Element, Id, Kind, RenderElement, UnderlyingStorage,
        surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
    },
    gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform},
    utils::{CommitCounter, DamageSet, OpaqueRegions},
};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

#[derive(Debug)]
pub(crate) struct ClippedSurfaceRenderElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    id: Id,
    geo_size: (f32, f32),
    elem_size: (f32, f32),
    elem_offset: (f32, f32),
    corner_radius: f32,
}

impl ClippedSurfaceRenderElement {
    pub(crate) fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        geo_rect: Rectangle<i32, Physical>,
        corner_radius: f32,
    ) -> Self {
        let elem_rect = inner.geometry(Scale::from(1.0));
        Self {
            inner,
            program,
            id: Id::new(),
            geo_size: (geo_rect.size.w.max(1) as f32, geo_rect.size.h.max(1) as f32),
            elem_size: (
                elem_rect.size.w.max(1) as f32,
                elem_rect.size.h.max(1) as f32,
            ),
            elem_offset: (
                (elem_rect.loc.x - geo_rect.loc.x) as f32,
                (elem_rect.loc.y - geo_rect.loc.y) as f32,
            ),
            corner_radius,
        }
    }
}

impl Element for ClippedSurfaceRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }
}

impl RenderElement<GlesRenderer> for ClippedSurfaceRenderElement {
    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self.inner.texture() {
            WaylandSurfaceTexture::Texture(texture) => {
                let uniforms = [
                    Uniform::new("geo_size", self.geo_size),
                    Uniform::new("elem_size", self.elem_size),
                    Uniform::new("elem_offset", self.elem_offset),
                    Uniform::new("corner_radius", self.corner_radius.max(0.0)),
                ];
                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    self.transform(),
                    self.alpha(),
                    Some(&self.program),
                    &uniforms,
                )
            }
            WaylandSurfaceTexture::SolidColor(color) => {
                frame.draw_solid(dst, damage, *color * self.alpha())
            }
        }
    }
}
