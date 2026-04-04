use glam::{Mat3, Vec2, Vec3};
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
    clip_scale: f32,
    geo_size: (f32, f32),
    corner_radius: (f32, f32, f32, f32),
    input_to_geo_row_0: (f32, f32, f32),
    input_to_geo_row_1: (f32, f32, f32),
    input_to_geo_row_2: (f32, f32, f32),
}

impl ClippedSurfaceRenderElement {
    pub(crate) fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        geo_rect: Rectangle<i32, Physical>,
        corner_radius: f32,
    ) -> Self {
        let elem_rect = inner.geometry(Scale::from(1.0));
        let geo_size = Vec2::new(geo_rect.size.w.max(1) as f32, geo_rect.size.h.max(1) as f32);
        let elem_size = Vec2::new(
            elem_rect.size.w.max(1) as f32,
            elem_rect.size.h.max(1) as f32,
        );
        let elem_offset = Vec2::new(
            (elem_rect.loc.x - geo_rect.loc.x) as f32,
            (elem_rect.loc.y - geo_rect.loc.y) as f32,
        );
        let input_to_geo = Mat3::from_cols(
            Vec3::new(elem_size.x / geo_size.x, 0.0, 0.0),
            Vec3::new(0.0, elem_size.y / geo_size.y, 0.0),
            Vec3::new(elem_offset.x / geo_size.x, elem_offset.y / geo_size.y, 1.0),
        );
        Self {
            inner,
            program,
            id: Id::new(),
            clip_scale: 1.0,
            geo_size: (geo_size.x, geo_size.y),
            corner_radius: (corner_radius, corner_radius, corner_radius, corner_radius),
            input_to_geo_row_0: (
                input_to_geo.x_axis.x,
                input_to_geo.x_axis.y,
                input_to_geo.x_axis.z,
            ),
            input_to_geo_row_1: (
                input_to_geo.y_axis.x,
                input_to_geo.y_axis.y,
                input_to_geo.y_axis.z,
            ),
            input_to_geo_row_2: (
                input_to_geo.z_axis.x,
                input_to_geo.z_axis.y,
                input_to_geo.z_axis.z,
            ),
        }
    }

    pub(crate) fn will_clip(
        inner: &WaylandSurfaceRenderElement<GlesRenderer>,
        geo_rect: Rectangle<i32, Physical>,
        corner_radius: f32,
    ) -> bool {
        let elem_rect = inner.geometry(Scale::from(1.0));
        corner_radius > 0.0 || !geo_rect.contains_rect(elem_rect)
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
        let damage = self.inner.damage_since(scale, commit);
        let mut geo = Rectangle::<i32, Physical>::new(
            (0, 0).into(),
            (
                self.geo_size.0.round() as i32,
                self.geo_size.1.round() as i32,
            )
                .into(),
        );
        geo.loc -= self.geometry(scale).loc;
        damage
            .into_iter()
            .filter_map(|rect| rect.intersection(geo))
            .collect()
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
                    Uniform::new("clip_scale", self.clip_scale),
                    Uniform::new("geo_size", self.geo_size),
                    Uniform::new("corner_radius", self.corner_radius),
                    Uniform::new("input_to_geo_row_0", self.input_to_geo_row_0),
                    Uniform::new("input_to_geo_row_1", self.input_to_geo_row_1),
                    Uniform::new("input_to_geo_row_2", self.input_to_geo_row_2),
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
