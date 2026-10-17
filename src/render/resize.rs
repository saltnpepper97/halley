use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType, ffi,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::{ContextId, Renderer, Texture};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::window_texture::ResizeWindowTexture;

const RESIZE_SHADER: &str = r#"
//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
varying vec2 v_coords;

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform sampler2D halley_next_tex;
uniform float halley_progress;
uniform float alpha;
uniform vec2 halley_input_scale;
uniform vec2 halley_input_offset;
uniform vec2 halley_previous_scale;
uniform vec2 halley_previous_offset;
uniform vec2 halley_next_scale;
uniform vec2 halley_next_offset;
uniform vec2 halley_previous_step;
uniform vec2 halley_next_step;
uniform float halley_previous_opaque;
uniform float halley_next_opaque;
uniform float halley_arrange;
uniform vec2 halley_rect_size;
uniform vec2 halley_corner_radii;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// A fullscreen endpoint drawn into a windowed rectangle is minified by more
// than 2x. One bilinear tap per pixel then skips texels and text shimmers
// while the rectangle moves. Average a 3x3 grid spanning the pixel footprint;
// `step` is zero when the endpoint is not minified.
#define HALLEY_AREA_SAMPLE(result, sampler, coords, step)                      \
    if (step.x == 0.0 && step.y == 0.0) {                                      \
        result = texture2D(sampler, coords);                                   \
    } else {                                                                   \
        result = vec4(0.0);                                                    \
        for (int ix = -1; ix <= 1; ix++) {                                     \
            for (int iy = -1; iy <= 1; iy++) {                                 \
                vec2 tap = coords + vec2(float(ix), float(iy)) * step / 3.0;   \
                result += texture2D(sampler, clamp(tap, 0.0, 1.0));            \
            }                                                                  \
        }                                                                      \
        result /= 9.0;                                                         \
    }

float rounded_alpha(vec2 coords, vec2 size, vec2 radii) {
    float radius = coords.y < size.y * 0.5 ? radii.x : radii.y;
    radius = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    vec2 half_size = size * 0.5;
    vec2 q = abs(coords - half_size) - (half_size - vec2(radius));
    float distance_to_edge =
        length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    return 1.0 - smoothstep(-0.75, 0.75, distance_to_edge);
}

void main() {
    // The shader element encompasses both textures, including client-side
    // decorations. Convert that element coordinate back into the animated
    // window geometry before independently mapping each endpoint texture.
    vec2 current_coords =
        v_coords * halley_input_scale + halley_input_offset;
    vec2 previous_coords =
        current_coords * halley_previous_scale + halley_previous_offset;
    vec2 next_coords =
        current_coords * halley_next_scale + halley_next_offset;

    bool previous_inside =
        all(greaterThanEqual(previous_coords, vec2(0.0))) &&
        all(lessThanEqual(previous_coords, vec2(1.0)));
    bool next_inside =
        all(greaterThanEqual(next_coords, vec2(0.0))) &&
        all(lessThanEqual(next_coords, vec2(1.0)));

    vec4 previous = vec4(0.0);
    if (previous_inside) {
        HALLEY_AREA_SAMPLE(previous, tex, previous_coords, halley_previous_step);
#if defined(NO_ALPHA)
        previous.a = 1.0;
#endif
        if (halley_previous_opaque > 0.5)
            previous.a = 1.0;
    }

    vec4 next = vec4(0.0);
    if (next_inside) {
        HALLEY_AREA_SAMPLE(next, halley_next_tex, next_coords, halley_next_step);
        if (halley_next_opaque > 0.5)
            next.a = 1.0;
    }

    vec2 size = max(halley_rect_size, vec2(1.0));
    float mask = rounded_alpha(current_coords * size, size, halley_corner_radii);
    previous *= mask;
    next *= mask;
    vec4 color;
    if (halley_arrange > 0.5) {
        // Reflowed text has different line breaks at each endpoint. Keep
        // the double image brief, around the fastest part of the motion.
        color = mix(previous, next, smoothstep(0.4, 0.6, halley_progress));
    } else {
        // Both endpoints are stretched over the same rectangle, so a long
        // linear mix shows two differently scaled images at once. Hold each
        // endpoint near its end and pass through the double image quickly.
        color = mix(previous, next, smoothstep(0.15, 0.85, halley_progress));
    }
    color *= alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

#[derive(Default)]
pub struct ResizeRenderer {
    program: Option<(ContextId<GlesTexture>, GlesTexProgram)>,
}

#[derive(Debug)]
pub struct ResizeRenderElement {
    base: TextureRenderElement<GlesTexture>,
    previous_texture: GlesTexture,
    next_texture: GlesTexture,
    program: GlesTexProgram,
    progress: f32,
    mapping: ResizeMapping,
    previous_opaque: f32,
    next_opaque: f32,
    arrange: f32,
    previous_step: (f32, f32),
    next_step: (f32, f32),
    size: (f32, f32),
    radii: super::window_decoration::CornerRadii,
    commit: CommitCounter,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeMapping {
    draw_area: Rectangle<i32, Physical>,
    input_scale: (f32, f32),
    input_offset: (f32, f32),
    previous_scale: (f32, f32),
    previous_offset: (f32, f32),
    next_scale: (f32, f32),
    next_offset: (f32, f32),
}

impl ResizeRenderer {
    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        id: Id,
        previous: &ResizeWindowTexture,
        next: ResizeWindowTexture,
        destination: Rectangle<i32, Physical>,
        progress: f32,
        alpha: f32,
        radii: super::window_decoration::CornerRadii,
        generation: CommitCounter,
    ) -> Result<ResizeRenderElement, GlesError> {
        let mapping = resize_mapping(previous, &next, destination);
        self.element_with_mapping(
            renderer,
            id,
            previous,
            next,
            destination,
            progress,
            alpha,
            radii,
            generation,
            mapping,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn arrange_element(
        &mut self,
        renderer: &mut GlesRenderer,
        id: Id,
        previous: &ResizeWindowTexture,
        next: ResizeWindowTexture,
        destination: Rectangle<i32, Physical>,
        progress: f32,
        alpha: f32,
        radii: super::window_decoration::CornerRadii,
        generation: CommitCounter,
    ) -> Result<ResizeRenderElement, GlesError> {
        let mapping = arrange_resize_mapping(previous, &next, destination);
        self.element_with_mapping(
            renderer,
            id,
            previous,
            next,
            destination,
            progress,
            alpha,
            radii,
            generation,
            mapping,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn element_with_mapping(
        &mut self,
        renderer: &mut GlesRenderer,
        id: Id,
        previous: &ResizeWindowTexture,
        next: ResizeWindowTexture,
        destination: Rectangle<i32, Physical>,
        progress: f32,
        alpha: f32,
        radii: super::window_decoration::CornerRadii,
        generation: CommitCounter,
        mapping: ResizeMapping,
        arrange: bool,
    ) -> Result<ResizeRenderElement, GlesError> {
        let context = renderer.context_id();
        let program = match self.program.as_ref() {
            Some((program_context, program)) if program_context == &context => program.clone(),
            _ => {
                let program = renderer.compile_custom_texture_shader(
                    RESIZE_SHADER,
                    &[
                        UniformName::new("halley_next_tex", UniformType::_1i),
                        UniformName::new("halley_progress", UniformType::_1f),
                        UniformName::new("halley_input_scale", UniformType::_2f),
                        UniformName::new("halley_input_offset", UniformType::_2f),
                        UniformName::new("halley_previous_scale", UniformType::_2f),
                        UniformName::new("halley_previous_offset", UniformType::_2f),
                        UniformName::new("halley_next_scale", UniformType::_2f),
                        UniformName::new("halley_next_offset", UniformType::_2f),
                        UniformName::new("halley_previous_step", UniformType::_2f),
                        UniformName::new("halley_next_step", UniformType::_2f),
                        UniformName::new("halley_previous_opaque", UniformType::_1f),
                        UniformName::new("halley_next_opaque", UniformType::_1f),
                        UniformName::new("halley_arrange", UniformType::_1f),
                        UniformName::new("halley_rect_size", UniformType::_2f),
                        UniformName::new("halley_corner_radii", UniformType::_2f),
                    ],
                )?;
                self.program = Some((context.clone(), program.clone()));
                program
            }
        };

        let base =
            texture_element_for_window(previous, id, mapping.draw_area, alpha.clamp(0.0, 1.0));
        let radii = super::window_decoration::CornerRadii {
            top: fit_radius(radii.top, destination),
            bottom: fit_radius(radii.bottom, destination),
        };
        let previous_step = minified_sample_step(
            mapping.previous_scale,
            mapping.input_scale,
            mapping.draw_area,
            previous.texture.size(),
        );
        let next_step = minified_sample_step(
            mapping.next_scale,
            mapping.input_scale,
            mapping.draw_area,
            next.texture.size(),
        );
        let commit = resize_commit(
            base.current_commit(),
            generation,
            progress,
            destination,
            radii,
        );

        Ok(ResizeRenderElement {
            base,
            previous_texture: previous.texture.clone(),
            next_texture: next.texture,
            program,
            progress: progress.clamp(0.0, 1.0),
            mapping,
            previous_opaque: if previous.client_opaque { 1.0 } else { 0.0 },
            next_opaque: if next.client_opaque { 1.0 } else { 0.0 },
            arrange: if arrange { 1.0 } else { 0.0 },
            previous_step,
            next_step,
            size: (destination.size.w as f32, destination.size.h as f32),
            radii,
            commit,
        })
    }
}

/// Texture-coordinate span of one output pixel, or zero when the endpoint
/// is shown at or above native resolution and a single tap is exact enough.
fn minified_sample_step(
    texture_scale: (f32, f32),
    input_scale: (f32, f32),
    draw_area: Rectangle<i32, Physical>,
    texture_size: smithay::utils::Size<i32, Buffer>,
) -> (f32, f32) {
    let step_x = texture_scale.0 * input_scale.0 / draw_area.size.w.max(1) as f32;
    let step_y = texture_scale.1 * input_scale.1 / draw_area.size.h.max(1) as f32;
    let texels_x = step_x * texture_size.w.max(1) as f32;
    let texels_y = step_y * texture_size.h.max(1) as f32;
    if texels_x.max(texels_y) <= 1.25 {
        (0.0, 0.0)
    } else {
        (step_x, step_y)
    }
}

fn fit_radius(radius: f32, destination: Rectangle<i32, Physical>) -> f32 {
    radius
        .max(0.0)
        .min(destination.size.w.min(destination.size.h).max(0) as f32 * 0.5)
}

pub(crate) fn texture_element_for_window(
    texture: &ResizeWindowTexture,
    id: Id,
    destination: Rectangle<i32, Physical>,
    alpha: f32,
) -> TextureRenderElement<GlesTexture> {
    let source = Rectangle::from_size(
        texture
            .texture
            .size()
            .to_logical(1, Transform::Normal)
            .to_f64(),
    );
    TextureRenderElement::from_static_texture(
        id,
        texture.context.clone(),
        destination.loc.to_f64(),
        texture.texture.clone(),
        1,
        Transform::Normal,
        Some(alpha),
        Some(source),
        Some(destination.size.to_logical(1)),
        None,
        Kind::Unspecified,
    )
}

fn resize_mapping(
    previous: &ResizeWindowTexture,
    next: &ResizeWindowTexture,
    destination: Rectangle<i32, Physical>,
) -> ResizeMapping {
    resize_mapping_from_metadata(
        previous.surface_geometry,
        previous.window_size,
        next.surface_geometry,
        next.window_size,
        destination,
    )
}

fn arrange_resize_mapping(
    previous: &ResizeWindowTexture,
    next: &ResizeWindowTexture,
    destination: Rectangle<i32, Physical>,
) -> ResizeMapping {
    arrange_resize_mapping_from_metadata(
        previous.surface_geometry,
        previous.window_size,
        next.surface_geometry,
        next.window_size,
        destination,
    )
}

fn arrange_resize_mapping_from_metadata(
    previous_surface: Rectangle<i32, Physical>,
    previous_window: smithay::utils::Size<i32, Physical>,
    next_surface: Rectangle<i32, Physical>,
    next_window: smithay::utils::Size<i32, Physical>,
    destination: Rectangle<i32, Physical>,
) -> ResizeMapping {
    // Each client fills the animated rectangle. Native-size cropping and
    // edge extension repeat toolbar/scrollbar pixels through uncovered bands.
    // Keep CSD margins outside this rectangle and apply one shared mask.
    let (previous_scale, previous_offset) =
        texture_coordinate_mapping(previous_surface, previous_window);
    let (next_scale, next_offset) = texture_coordinate_mapping(next_surface, next_window);
    ResizeMapping {
        draw_area: destination,
        input_scale: (1.0, 1.0),
        input_offset: (0.0, 0.0),
        previous_scale,
        previous_offset,
        next_scale,
        next_offset,
    }
}

fn resize_mapping_from_metadata(
    previous_surface: Rectangle<i32, Physical>,
    previous_window: smithay::utils::Size<i32, Physical>,
    next_surface: Rectangle<i32, Physical>,
    next_window: smithay::utils::Size<i32, Physical>,
    destination: Rectangle<i32, Physical>,
) -> ResizeMapping {
    let previous_bounds = scaled_surface_bounds(previous_surface, previous_window, destination);
    let next_bounds = scaled_surface_bounds(next_surface, next_window, destination);
    let draw_area = previous_bounds.merge(next_bounds);
    let width = destination.size.w.max(1) as f32;
    let height = destination.size.h.max(1) as f32;
    // Each client endpoint fills the same animated client rectangle. Mapping
    // either through an interpolated buffer size leaves its smaller edge
    // inside the window, where blending against empty space exposes a panel.
    // Keep each endpoint's CSD margins relative to its own client geometry.
    let (previous_scale, previous_offset) =
        texture_coordinate_mapping(previous_surface, previous_window);
    let (next_scale, next_offset) = texture_coordinate_mapping(next_surface, next_window);

    ResizeMapping {
        draw_area,
        input_scale: (
            draw_area.size.w as f32 / width,
            draw_area.size.h as f32 / height,
        ),
        input_offset: (
            (draw_area.loc.x - destination.loc.x) as f32 / width,
            (draw_area.loc.y - destination.loc.y) as f32 / height,
        ),
        previous_scale,
        previous_offset,
        next_scale,
        next_offset,
    }
}

fn scaled_surface_bounds(
    surface: Rectangle<i32, Physical>,
    window: smithay::utils::Size<i32, Physical>,
    destination: Rectangle<i32, Physical>,
) -> Rectangle<i32, Physical> {
    let scale_x = f64::from(destination.size.w) / f64::from(window.w.max(1));
    let scale_y = f64::from(destination.size.h) / f64::from(window.h.max(1));
    let left = f64::from(destination.loc.x) + f64::from(surface.loc.x) * scale_x;
    let top = f64::from(destination.loc.y) + f64::from(surface.loc.y) * scale_y;
    let right = f64::from(destination.loc.x)
        + f64::from(surface.loc.x.saturating_add(surface.size.w)) * scale_x;
    let bottom = f64::from(destination.loc.y)
        + f64::from(surface.loc.y.saturating_add(surface.size.h)) * scale_y;
    let left = left.floor() as i32;
    let top = top.floor() as i32;
    let right = right.ceil() as i32;
    let bottom = bottom.ceil() as i32;
    Rectangle::new(
        (left, top).into(),
        (right.saturating_sub(left), bottom.saturating_sub(top)).into(),
    )
}

fn texture_coordinate_mapping(
    surface: Rectangle<i32, Physical>,
    current_size: smithay::utils::Size<i32, Physical>,
) -> ((f32, f32), (f32, f32)) {
    let width = surface.size.w.max(1) as f32;
    let height = surface.size.h.max(1) as f32;
    (
        (
            current_size.w as f32 / width,
            current_size.h as f32 / height,
        ),
        (
            -(surface.loc.x as f32) / width,
            -(surface.loc.y as f32) / height,
        ),
    )
}

fn resize_commit(
    base: CommitCounter,
    generation: CommitCounter,
    progress: f32,
    destination: Rectangle<i32, Physical>,
    radii: super::window_decoration::CornerRadii,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    base.distance(Some(CommitCounter::default()))
        .unwrap_or(usize::MAX)
        .hash(&mut hasher);
    generation
        .distance(Some(CommitCounter::default()))
        .unwrap_or(usize::MAX)
        .hash(&mut hasher);
    progress.to_bits().hash(&mut hasher);
    destination.loc.x.hash(&mut hasher);
    destination.loc.y.hash(&mut hasher);
    destination.size.w.hash(&mut hasher);
    destination.size.h.hash(&mut hasher);
    radii.top.to_bits().hash(&mut hasher);
    radii.bottom.to_bits().hash(&mut hasher);
    CommitCounter::from(hasher.finish() as usize)
}

impl Element for ResizeRenderElement {
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
        self.base.alpha()
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ResizeRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let (previous_active_texture, previous_texture_1) = frame.with_context(|gl| unsafe {
            let mut active_texture = 0_i32;
            gl.GetIntegerv(ffi::ACTIVE_TEXTURE, &mut active_texture);
            gl.ActiveTexture(ffi::TEXTURE1);
            let mut texture_1 = 0_i32;
            gl.GetIntegerv(ffi::TEXTURE_BINDING_2D, &mut texture_1);
            gl.BindTexture(ffi::TEXTURE_2D, self.next_texture.tex_id());
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_S,
                ffi::CLAMP_TO_EDGE as i32,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_T,
                ffi::CLAMP_TO_EDGE as i32,
            );
            gl.ActiveTexture(active_texture as u32);
            (active_texture, texture_1)
        })?;

        let result = frame.render_texture_from_to(
            &self.previous_texture,
            src,
            dst,
            damage,
            opaque_regions,
            self.transform(),
            self.alpha(),
            Some(&self.program),
            &[
                Uniform::new("halley_next_tex", 1_i32),
                Uniform::new("halley_progress", self.progress),
                Uniform::new("halley_input_scale", self.mapping.input_scale),
                Uniform::new("halley_input_offset", self.mapping.input_offset),
                Uniform::new("halley_previous_scale", self.mapping.previous_scale),
                Uniform::new("halley_previous_offset", self.mapping.previous_offset),
                Uniform::new("halley_next_scale", self.mapping.next_scale),
                Uniform::new("halley_next_offset", self.mapping.next_offset),
                Uniform::new("halley_previous_step", self.previous_step),
                Uniform::new("halley_next_step", self.next_step),
                Uniform::new("halley_previous_opaque", self.previous_opaque),
                Uniform::new("halley_next_opaque", self.next_opaque),
                Uniform::new("halley_arrange", self.arrange),
                Uniform::new("halley_rect_size", self.size),
                Uniform::new("halley_corner_radii", (self.radii.top, self.radii.bottom)),
            ],
        );

        frame.with_context(|gl| unsafe {
            gl.ActiveTexture(ffi::TEXTURE1);
            gl.BindTexture(ffi::TEXTURE_2D, previous_texture_1 as u32);
            gl.ActiveTexture(previous_active_texture as u32);
        })?;
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        arrange_resize_mapping_from_metadata, resize_commit, resize_mapping_from_metadata,
    };
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::{Physical, Rectangle};

    #[test]
    #[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1"]
    fn shared_bounds_blend_has_no_inner_edge_and_matches_the_settled_mask() {
        use super::*;
        use smithay::backend::allocator::Fourcc;
        use smithay::backend::egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay};
        use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen};
        let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
        let snapshot = |renderer: &mut GlesRenderer, size: (i32, i32), color: [u8; 4]| {
            let pixels = color.repeat((size.0 * size.1) as usize);
            ResizeWindowTexture {
                texture: renderer
                    .import_memory(&pixels, Fourcc::Abgr8888, size.into(), false)
                    .unwrap(),
                context: renderer.context_id(),
                surface_geometry: Rectangle::from_size(size.into()),
                window_size: size.into(),
                client_opaque: true,
                surface_layers: Vec::new(),
            }
        };
        let previous = snapshot(&mut renderer, (160, 120), [255, 0, 0, 255]);
        let next = snapshot(&mut renderer, (80, 60), [0, 255, 0, 255]);
        let mut resize = ResizeRenderer::default();
        let radii = crate::render::window_decoration::CornerRadii::all(10.0);
        let render = |renderer: &mut GlesRenderer, element: ResizeRenderElement| {
            let rect = element.geometry(Scale::from(1.0));
            let size = rect.size.to_logical(1).to_buffer(1, Transform::Normal);
            let mut target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, size).unwrap();
            let mut fbo = renderer.bind(&mut target).unwrap();
            {
                let mut frame = renderer
                    .render(&mut fbo, rect.size, Transform::Normal)
                    .unwrap();
                frame.clear(Color32F::BLACK, &[rect]).unwrap();
                element
                    .draw(&mut frame, element.src(), rect, &[rect], &[], None)
                    .unwrap();
                frame.finish().unwrap().wait().unwrap();
            }
            let mapping = renderer
                .copy_framebuffer(&fbo, Rectangle::from_size(size), Fourcc::Abgr8888)
                .unwrap();
            renderer.map_texture(&mapping).unwrap().to_vec()
        };
        for (outgoing, incoming) in [(&previous, next.clone()), (&next, previous.clone())] {
            let middle = resize
                .element(
                    &mut renderer,
                    Id::new(),
                    outgoing,
                    incoming,
                    Rectangle::from_size((120, 90).into()),
                    0.5,
                    0.9,
                    radii,
                    CommitCounter::default(),
                )
                .unwrap();
            let pixels = render(&mut renderer, middle);
            // Both colours must contribute uniformly across the client. The
            // former smaller endpoint ended around (80, 60), leaving a hard
            // interior boundary and fading the rest toward the background.
            for y in 12..78 {
                for x in 12..108 {
                    let pixel = &pixels[(y * 120 + x) * 4..][..4];
                    assert!(
                        (105..125).contains(&pixel[0]) && (105..125).contains(&pixel[1]),
                        "uneven blend or opacity at ({x}, {y}): {pixel:?}"
                    );
                    assert!(pixel[2] < 8);
                }
            }
        }

        let settled = Rectangle::from_size((80, 60).into());
        let blend = resize
            .element(
                &mut renderer,
                Id::new(),
                &previous,
                next.clone(),
                settled,
                1.0,
                0.9,
                radii,
                CommitCounter::default(),
            )
            .unwrap();
        let blend_pixels = render(&mut renderer, blend);
        let reference = resize
            .arrange_element(
                &mut renderer,
                Id::new(),
                &next,
                next.clone(),
                settled,
                1.0,
                0.9,
                radii,
                CommitCounter::default(),
            )
            .unwrap();
        assert_eq!(
            blend_pixels,
            render(&mut renderer, reference),
            "the endpoint must apply one antialias mask, without a cleanup jump"
        );

        // A one-pixel checkerboard reduced by 3x aliases to alternating black
        // and white pixels with a single bilinear tap. Area sampling should
        // retain its average brightness in both axes instead.
        let mut pixels = Vec::with_capacity(240 * 180 * 4);
        for y in 0..180 {
            for x in 0..240 {
                let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let checkerboard = ResizeWindowTexture {
            texture: renderer
                .import_memory(&pixels, Fourcc::Abgr8888, (240, 180).into(), false)
                .unwrap(),
            context: renderer.context_id(),
            surface_geometry: Rectangle::from_size((240, 180).into()),
            window_size: (240, 180).into(),
            client_opaque: true,
            surface_layers: Vec::new(),
        };
        let minified = resize
            .element(
                &mut renderer,
                Id::new(),
                &checkerboard,
                checkerboard.clone(),
                settled,
                0.5,
                1.0,
                super::super::window_decoration::CornerRadii::all(0.0),
                CommitCounter::default(),
            )
            .unwrap();
        let pixels = render(&mut renderer, minified);
        for y in 2..58 {
            for x in 2..78 {
                let pixel = &pixels[(y * 80 + x) * 4..][..4];
                assert!(
                    pixel[..3]
                        .iter()
                        .all(|channel| (105..150).contains(channel)),
                    "minified detail must retain its average brightness: {pixel:?}"
                );
                assert_eq!(pixel[3], 255);
            }
        }

        // Detail must span the whole animated client rather than repeat the
        // smaller endpoint's last row/column. Check both resize directions
        // and verify that only the middle of the gesture blends layouts.
        let gradient = |renderer: &mut GlesRenderer, size: (i32, i32), blue: bool| {
            let mut pixels = Vec::new();
            for y in 0..size.1 {
                for x in 0..size.0 {
                    let horizontal = ((x as f32 + 0.5) * 255.0 / size.0 as f32) as u8;
                    let vertical = ((y as f32 + 0.5) * 255.0 / size.1 as f32) as u8;
                    let pixel = if blue {
                        [0, vertical, horizontal, 255]
                    } else {
                        [horizontal, vertical, 0, 255]
                    };
                    pixels.extend_from_slice(&pixel);
                }
            }
            ResizeWindowTexture {
                texture: renderer
                    .import_memory(&pixels, Fourcc::Abgr8888, size.into(), false)
                    .unwrap(),
                context: renderer.context_id(),
                surface_geometry: Rectangle::from_size(size.into()),
                window_size: size.into(),
                client_opaque: true,
                surface_layers: Vec::new(),
            }
        };
        let wide = gradient(&mut renderer, (160, 120), false);
        let small = gradient(&mut renderer, (80, 60), true);
        for (outgoing, incoming, reversed) in [(&wide, &small, false), (&small, &wide, true)] {
            for (progress, weight) in [(0.35, 0.0), (0.5, 0.5), (0.65, 1.0)] {
                let element = resize
                    .arrange_element(
                        &mut renderer,
                        Id::new(),
                        outgoing,
                        incoming.clone(),
                        Rectangle::from_size((120, 90).into()),
                        progress,
                        1.0,
                        crate::render::window_decoration::CornerRadii::default(),
                        CommitCounter::default(),
                    )
                    .unwrap();
                let pixels = render(&mut renderer, element);
                let blue_weight = if reversed { 1.0 - weight } else { weight };
                for y in [12, 45, 77] {
                    for x in [12, 60, 107] {
                        let horizontal = (x as f32 + 0.5) * 255.0 / 120.0;
                        let expected = [
                            horizontal * (1.0 - blue_weight),
                            (y as f32 + 0.5) * 255.0 / 90.0,
                            horizontal * blue_weight,
                            255.0,
                        ];
                        let pixel = &pixels[(y * 120 + x) * 4..][..4];
                        for (actual, expected) in pixel.iter().zip(expected) {
                            assert!(
                                (*actual as f32 - expected).abs() < 4.0,
                                "arrange detail at ({x}, {y}), progress={progress}: {pixel:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn arrangement_fits_both_clients_and_excludes_their_csd_margins() {
        let destination = Rectangle::new((10, 20).into(), (1000, 750).into());
        let mapping = arrange_resize_mapping_from_metadata(
            Rectangle::new((-20, -10).into(), (840, 620).into()),
            (800, 600).into(),
            Rectangle::new((0, 0).into(), (1200, 900).into()),
            (1200, 900).into(),
            destination,
        );
        assert_eq!(mapping.draw_area, destination);
        assert_eq!(mapping.input_scale, (1.0, 1.0));
        assert_eq!(mapping.input_offset, (0.0, 0.0));
        assert_eq!(mapping.previous_scale, (800.0 / 840.0, 600.0 / 620.0));
        assert_eq!(mapping.previous_offset, (20.0 / 840.0, 10.0 / 620.0));
        assert_eq!(mapping.next_scale, (1.0, 1.0));
        assert_eq!(mapping.next_offset, (0.0, 0.0));
    }

    #[test]
    fn firefox_csd_bounds_are_kept_separate_from_window_geometry() {
        let previous_surface = Rectangle::new((-20, -20).into(), (1036, 704).into());
        let next_surface = Rectangle::new((0, 0).into(), (2560, 1440).into());
        let destination = Rectangle::new((0, 0).into(), (1280, 720).into());

        let mapping = resize_mapping_from_metadata(
            previous_surface,
            (996, 664).into(),
            next_surface,
            (2560, 1440).into(),
            destination,
        );

        assert_eq!(mapping.draw_area.loc, (-26, -22).into());
        assert_eq!(mapping.draw_area.size, (1332, 764).into());
        assert!(mapping.input_offset.0 < 0.0);
        assert!(mapping.input_offset.1 < 0.0);
        assert!((mapping.previous_scale.0 - 996.0 / 1036.0).abs() < 0.01);
        assert!((mapping.previous_scale.1 - 664.0 / 704.0).abs() < 0.01);
        assert!((mapping.previous_offset.0 - 20.0 / 1036.0).abs() < f32::EPSILON);
        assert_eq!(mapping.next_scale, (1.0, 1.0));
        assert_eq!(mapping.next_offset, (0.0, 0.0));
    }

    #[test]
    fn both_endpoint_clients_fill_the_animated_rectangle() {
        let previous_surface = Rectangle::new((0, 0).into(), (1000, 600).into());
        let next_surface = Rectangle::new((0, 0).into(), (2000, 1200).into());
        let midway = Rectangle::new((0, 0).into(), (1500, 900).into());

        let mapping = resize_mapping_from_metadata(
            previous_surface,
            (1000, 600).into(),
            next_surface,
            (2000, 1200).into(),
            midway,
        );

        assert_eq!(mapping.previous_scale, (1.0, 1.0));
        assert_eq!(mapping.next_scale, (1.0, 1.0));
    }

    #[test]
    fn cluster_and_field_destinations_use_the_same_local_mapping() {
        let surface = Rectangle::new((-20, -20).into(), (1036, 704).into());
        let field = resize_mapping_from_metadata(
            surface,
            (996, 664).into(),
            surface,
            (996, 664).into(),
            Rectangle::new((100, 200).into(), (996, 664).into()),
        );
        let cluster = resize_mapping_from_metadata(
            surface,
            (996, 664).into(),
            surface,
            (996, 664).into(),
            Rectangle::new((500, 50).into(), (996, 664).into()),
        );

        assert_eq!(field.input_scale, cluster.input_scale);
        assert_eq!(field.input_offset, cluster.input_offset);
        assert_eq!(field.previous_scale, cluster.previous_scale);
        assert_eq!(field.previous_offset, cluster.previous_offset);
    }

    #[test]
    fn resize_commits_follow_location_generation_and_progress() {
        let destination = Rectangle::<i32, Physical>::new((10, 20).into(), (1920, 1080).into());
        let radii = crate::render::window_decoration::CornerRadii::all(8.0);
        let base = CommitCounter::from(7);
        let first = resize_commit(base, CommitCounter::from(1), 0.25, destination, radii);

        assert_eq!(
            first,
            resize_commit(base, CommitCounter::from(1), 0.25, destination, radii)
        );
        assert_ne!(
            first,
            resize_commit(
                base,
                CommitCounter::from(1),
                0.25,
                Rectangle::new((11, 20).into(), destination.size),
                radii,
            )
        );
        assert_ne!(
            first,
            resize_commit(base, CommitCounter::from(2), 0.25, destination, radii)
        );
        assert_ne!(
            first,
            resize_commit(base, CommitCounter::from(1), 0.5, destination, radii)
        );
    }

    #[test]
    fn zoomed_field_samples_native_cluster_space_then_scales_to_the_screen() {
        let previous_surface = Rectangle::new((0, 0).into(), (1000, 600).into());
        let next_surface = Rectangle::new((0, 0).into(), (2000, 1200).into());
        let zoomed = Rectangle::new((80, 40).into(), (350, 210).into());

        let mapping = resize_mapping_from_metadata(
            previous_surface,
            (1000, 600).into(),
            next_surface,
            (2000, 1200).into(),
            zoomed,
        );

        // Zoom scales the shared client rectangle, not a cropped corner of
        // either endpoint. The whole fullscreen buffer remains represented.
        assert_eq!(mapping.previous_scale, (1.0, 1.0));
        assert_eq!(mapping.next_scale, (1.0, 1.0));
    }
}
