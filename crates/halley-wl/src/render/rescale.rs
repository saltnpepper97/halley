use glam::{Mat3, Vec2, Vec3};
use smithay::backend::renderer::{
    element::{
        Element, Id, Kind, RenderElement, UnderlyingStorage, surface::WaylandSurfaceTexture,
    },
    gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform},
    utils::{CommitCounter, DamageSet, OpaqueRegions},
};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::window::SurfaceElement;

/// Round a pre-zoom physical rect into a post-zoom physical rect by rounding
/// the two corners independently (not loc + size).
///
/// Smithay's `to_i32_round()` rounds `loc` and `size` independently, so for
/// non-integer `scale` the resulting `right = round(loc*s) + round(size*s)`
/// can differ from `round((loc+size)*s)` by ±1 pixel. That off-by-one is the
/// source of seams between subsurfaces at fractional zoom. Corner rounding is
/// pixel-consistent: adjacent elements sharing a pre-zoom coordinate always
/// meet at the same post-zoom pixel.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn corner_round_rect(
    rect: Rectangle<i32, Physical>,
    scale: f64,
) -> Rectangle<i32, Physical> {
    let x0 = (rect.loc.x as f64 * scale).round() as i32;
    let y0 = (rect.loc.y as f64 * scale).round() as i32;
    let x1 = ((rect.loc.x + rect.size.w) as f64 * scale).round() as i32;
    let y1 = ((rect.loc.y + rect.size.h) as f64 * scale).round() as i32;
    Rectangle::new(
        Point::from((x0, y0)),
        Size::from(((x1 - x0).max(0), (y1 - y0).max(0))),
    )
}

fn map_rect_between(
    rect: Rectangle<i32, Physical>,
    base: Rectangle<i32, Physical>,
    visual: Rectangle<i32, Physical>,
) -> Rectangle<i32, Physical> {
    let sx = visual.size.w.max(1) as f64 / base.size.w.max(1) as f64;
    let sy = visual.size.h.max(1) as f64 / base.size.h.max(1) as f64;
    let x0 = visual.loc.x as f64 + (rect.loc.x - base.loc.x) as f64 * sx;
    let y0 = visual.loc.y as f64 + (rect.loc.y - base.loc.y) as f64 * sy;
    let x1 = visual.loc.x as f64 + (rect.loc.x + rect.size.w - base.loc.x) as f64 * sx;
    let y1 = visual.loc.y as f64 + (rect.loc.y + rect.size.h - base.loc.y) as f64 * sy;
    Rectangle::new(
        Point::from((x0.round() as i32, y0.round() as i32)),
        Size::from((
            (x1.round() as i32 - x0.round() as i32).max(0),
            (y1.round() as i32 - y0.round() as i32).max(0),
        )),
    )
}

/// A surface element rendered in a stable base window coordinate space and mapped
/// into the current visual window rect.
///
/// Replaces baking `cam_scale` into `element_scale` (which causes per-subsurface
/// rounding drift — the "textures rolling under borders" class of bug), and also
/// keeps open/raise/tile visual transforms actor-like: content maps from the same
/// base window rect into the same visual window rect as borders and shadows.
///
/// Also handles corner-radius clipping at post-zoom scale, using the same
/// `surface_clipped_texture` shader as [`ClippedSurfaceRenderElement`] but with
/// `clip_scale` set to the real render scale so AA bandwidth follows the zoom.
pub(crate) struct RescaledSurfaceElement {
    inner: SurfaceElement,
    id: Id,
    base_window_geo: Rectangle<i32, Physical>,
    visual_window_geo: Rectangle<i32, Physical>,
    post_geo: Rectangle<i32, Physical>,
    clip_program: Option<GlesTexProgram>,
    clip_geo_size: (f32, f32),
    clip_corner_radius: (f32, f32, f32, f32),
    clip_input_to_geo_row_0: (f32, f32, f32),
    clip_input_to_geo_row_1: (f32, f32, f32),
    clip_input_to_geo_row_2: (f32, f32, f32),
    clip_scale: f32,
}

impl RescaledSurfaceElement {
    /// Construct from a surface element rendered in base-window coordinates.
    ///
    /// - `inner`: the element returned by `render_elements_from_surface_tree` in
    ///   the stable base coordinate space.
    /// - `base_window_geo`: the unanimated window geometry in that base space.
    /// - `visual_window_geo`: the current animated/zoomed window geometry.
    /// - `clip_program`: the `surface_clipped_texture` program, if corner
    ///   clipping is needed.
    /// - `post_zoom_window_geo`: the window's geometry rect in post-zoom physical
    ///   pixels (for computing clip uniforms).
    /// - `corner_radius`: content corner radius in post-zoom pixels.
    /// - `render_scale`: the full render scale (content * cam) for AA bandwidth.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        inner: SurfaceElement,
        base_window_geo: Rectangle<i32, Physical>,
        visual_window_geo: Rectangle<i32, Physical>,
        clip_program: Option<GlesTexProgram>,
        corner_radius: f32,
        render_scale: f32,
    ) -> Self {
        let base_elem = inner.geometry(Scale::from(1.0));
        let post_zoom_elem = map_rect_between(base_elem, base_window_geo, visual_window_geo);

        let geo_size = Vec2::new(
            visual_window_geo.size.w.max(1) as f32,
            visual_window_geo.size.h.max(1) as f32,
        );
        let elem_size = Vec2::new(
            post_zoom_elem.size.w.max(1) as f32,
            post_zoom_elem.size.h.max(1) as f32,
        );
        let elem_offset = Vec2::new(
            (post_zoom_elem.loc.x - visual_window_geo.loc.x) as f32,
            (post_zoom_elem.loc.y - visual_window_geo.loc.y) as f32,
        );
        let input_to_geo = Mat3::from_cols(
            Vec3::new(elem_size.x / geo_size.x, 0.0, 0.0),
            Vec3::new(0.0, elem_size.y / geo_size.y, 0.0),
            Vec3::new(elem_offset.x / geo_size.x, elem_offset.y / geo_size.y, 1.0),
        );

        Self {
            inner,
            id: Id::new(),
            base_window_geo,
            visual_window_geo,
            post_geo: post_zoom_elem,
            clip_program,
            clip_geo_size: (geo_size.x, geo_size.y),
            clip_corner_radius: (corner_radius, corner_radius, corner_radius, corner_radius),
            clip_input_to_geo_row_0: (
                input_to_geo.x_axis.x,
                input_to_geo.x_axis.y,
                input_to_geo.x_axis.z,
            ),
            clip_input_to_geo_row_1: (
                input_to_geo.y_axis.x,
                input_to_geo.y_axis.y,
                input_to_geo.y_axis.z,
            ),
            clip_input_to_geo_row_2: (
                input_to_geo.z_axis.x,
                input_to_geo.z_axis.y,
                input_to_geo.z_axis.z,
            ),
            clip_scale: render_scale,
        }
    }

    pub(crate) fn needs_clip(
        inner: &SurfaceElement,
        base_window_geo: Rectangle<i32, Physical>,
        visual_window_geo: Rectangle<i32, Physical>,
        corner_radius: f32,
    ) -> bool {
        if corner_radius > 0.0 {
            return true;
        }
        let base_elem = inner.geometry(Scale::from(1.0));
        let post_zoom_elem = map_rect_between(base_elem, base_window_geo, visual_window_geo);
        !visual_window_geo.contains_rect(post_zoom_elem)
    }
}

impl Element for RescaledSurfaceElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.post_geo
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let inner_damage = self.inner.damage_since(Scale::from(1.0), commit);
        inner_damage
            .into_iter()
            .map(|rect| map_rect_between(rect, self.base_window_geo, self.visual_window_geo))
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

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.post_geo.loc
    }
}

impl RenderElement<GlesRenderer> for RescaledSurfaceElement {
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
        _cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), GlesError> {
        match self.inner.texture() {
            WaylandSurfaceTexture::Texture(texture) => {
                if let Some(program) = self.clip_program.as_ref() {
                    let uniforms = [
                        Uniform::new("clip_scale", self.clip_scale),
                        Uniform::new("geo_size", self.clip_geo_size),
                        Uniform::new("corner_radius", self.clip_corner_radius),
                        Uniform::new("input_to_geo_row_0", self.clip_input_to_geo_row_0),
                        Uniform::new("input_to_geo_row_1", self.clip_input_to_geo_row_1),
                        Uniform::new("input_to_geo_row_2", self.clip_input_to_geo_row_2),
                    ];
                    frame.render_texture_from_to(
                        texture,
                        src,
                        dst,
                        damage,
                        opaque_regions,
                        self.transform(),
                        self.alpha(),
                        Some(program),
                        &uniforms,
                    )
                } else {
                    frame.render_texture_from_to(
                        texture,
                        src,
                        dst,
                        damage,
                        opaque_regions,
                        self.transform(),
                        self.alpha(),
                        None,
                        &[],
                    )
                }
            }
            WaylandSurfaceTexture::SolidColor(color) => {
                frame.draw_solid(dst, damage, *color * self.alpha())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn corner_round_is_identity_at_scale_1() {
        let r = rect(100, 200, 300, 400);
        assert_eq!(corner_round_rect(r, 1.0), r);
    }

    #[test]
    fn corner_round_shared_edge_meets_at_same_pixel() {
        let scale = 1.33;
        let left = rect(0, 0, 100, 200);
        let right = rect(100, 0, 50, 200);
        let left_scaled = corner_round_rect(left, scale);
        let right_scaled = corner_round_rect(right, scale);
        assert_eq!(
            left_scaled.loc.x + left_scaled.size.w,
            right_scaled.loc.x,
            "adjacent elements sharing pre-zoom x=100 must meet at same post-zoom pixel"
        );
    }

    #[test]
    fn corner_round_differs_from_loc_size_rounding_at_fractional_scale() {
        let scale = 0.5;
        let r = rect(1, 0, 3, 10);
        let corner = corner_round_rect(r, scale);
        let right_corner = corner.loc.x + corner.size.w;
        let right_loc_size =
            ((r.loc.x as f64 * scale).round() as i32) + ((r.size.w as f64 * scale).round() as i32);
        assert_ne!(
            right_corner, right_loc_size,
            "corner rounding must differ from loc+size rounding for some fractional scale"
        );
        assert_eq!(
            right_corner,
            ((r.loc.x + r.size.w) as f64 * scale).round() as i32
        );
    }
}
