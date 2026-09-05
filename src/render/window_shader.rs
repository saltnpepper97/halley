use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::{ContextId, Renderer};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::window_texture::WindowTexture;

const PRELUDE: &str = r#"
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

uniform float alpha;
uniform float halley_progress;
uniform float halley_clamped_progress;
uniform float halley_random_seed;
uniform vec2 halley_input_scale;
uniform vec2 halley_input_offset;
uniform vec2 halley_tex_scale;
uniform vec2 halley_tex_offset;
uniform vec2 halley_geo_size;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
"#;

const OPEN_EPILOGUE: &str = r#"
void main() {
    vec2 coords_geo_xy = v_coords * halley_input_scale + halley_input_offset;
    vec4 color = open_color(vec3(coords_geo_xy, 1.0), vec3(halley_geo_size, 1.0));
    gl_FragColor = color * alpha;
}
"#;

const CLOSE_EPILOGUE: &str = r#"
void main() {
    vec2 coords_geo_xy = v_coords * halley_input_scale + halley_input_offset;
    vec4 color = close_color(vec3(coords_geo_xy, 1.0), vec3(halley_geo_size, 1.0));
    gl_FragColor = color * alpha;
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShaderKind {
    Open,
    Close,
}

struct CompiledProgram {
    key: String,
    context: ContextId<GlesTexture>,
    program: GlesTexProgram,
}

/// Compiled user programs for window open and close animations.
#[derive(Default)]
pub struct WindowAnimationShaders {
    open_path: Option<String>,
    close_path: Option<String>,
    open: Option<CompiledProgram>,
    close: Option<CompiledProgram>,
    failed_open: Option<String>,
    failed_close: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderMapping {
    pub draw_area: Rectangle<i32, Physical>,
    pub input_scale: (f32, f32),
    pub input_offset: (f32, f32),
    pub tex_scale: (f32, f32),
    pub tex_offset: (f32, f32),
    pub geo_size: (f32, f32),
}

pub struct WindowShaderRenderElement {
    base: TextureRenderElement<GlesTexture>,
    texture: GlesTexture,
    program: GlesTexProgram,
    progress: f32,
    clamped_progress: f32,
    random_seed: f32,
    mapping: ShaderMapping,
    commit: CommitCounter,
}

impl WindowAnimationShaders {
    pub fn reload(&mut self, animations: &halley_config::Animations) {
        let open = animations.window_open.custom_shader.clone();
        let close = animations.window_close.custom_shader.clone();
        if self.open_path != open {
            self.open = None;
            self.failed_open = None;
            self.open_path = open;
        }
        if self.close_path != close {
            self.close = None;
            self.failed_close = None;
            self.close_path = close;
        }
    }

    pub fn ensure(&mut self, renderer: &mut GlesRenderer, config_dir: Option<&Path>) {
        self.open = compile_requested(
            renderer,
            ShaderKind::Open,
            self.open_path.as_deref(),
            config_dir,
            self.open.take(),
            &mut self.failed_open,
        );
        self.close = compile_requested(
            renderer,
            ShaderKind::Close,
            self.close_path.as_deref(),
            config_dir,
            self.close.take(),
            &mut self.failed_close,
        );
    }

    pub fn open_available(&self) -> bool {
        self.open.is_some()
    }

    pub fn close_available(&self) -> bool {
        self.close.is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_element(
        &self,
        renderer: &GlesRenderer,
        texture: &WindowTexture,
        id: Id,
        geo: Rectangle<i32, Physical>,
        progress: f32,
        clamped_progress: f32,
        random_seed: f32,
        alpha: f32,
    ) -> Option<WindowShaderRenderElement> {
        let program = self
            .open
            .as_ref()
            .filter(|program| program.context == renderer.context_id())?;
        Some(shader_element(
            texture,
            id,
            geo,
            progress,
            clamped_progress,
            random_seed,
            alpha,
            program.program.clone(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn close_element(
        &self,
        renderer: &GlesRenderer,
        texture: &WindowTexture,
        id: Id,
        geo: Rectangle<i32, Physical>,
        progress: f32,
        clamped_progress: f32,
        random_seed: f32,
        alpha: f32,
    ) -> Option<WindowShaderRenderElement> {
        let program = self
            .close
            .as_ref()
            .filter(|program| program.context == renderer.context_id())?;
        Some(shader_element(
            texture,
            id,
            geo,
            progress,
            clamped_progress,
            random_seed,
            alpha,
            program.program.clone(),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn shader_element(
    texture: &WindowTexture,
    id: Id,
    geo: Rectangle<i32, Physical>,
    progress: f32,
    clamped_progress: f32,
    random_seed: f32,
    alpha: f32,
    program: GlesTexProgram,
) -> WindowShaderRenderElement {
    let mapping = shader_mapping(geo, expanded_area(geo));
    let base = texture.render_element(id, mapping.draw_area, alpha.clamp(0.0, 1.0));
    let commit = shader_commit(
        base.current_commit(),
        progress,
        clamped_progress,
        random_seed,
        mapping,
    );
    WindowShaderRenderElement {
        texture: texture.texture.clone(),
        base,
        program,
        progress,
        clamped_progress,
        random_seed,
        mapping,
        commit,
    }
}

pub(crate) fn shader_mapping(
    geo: Rectangle<i32, Physical>,
    draw_area: Rectangle<i32, Physical>,
) -> ShaderMapping {
    let geo_w = geo.size.w.max(1) as f32;
    let geo_h = geo.size.h.max(1) as f32;
    ShaderMapping {
        draw_area,
        input_scale: (
            draw_area.size.w as f32 / geo_w,
            draw_area.size.h as f32 / geo_h,
        ),
        input_offset: (
            (draw_area.loc.x - geo.loc.x) as f32 / geo_w,
            (draw_area.loc.y - geo.loc.y) as f32 / geo_h,
        ),
        tex_scale: (1.0, 1.0),
        tex_offset: (0.0, 0.0),
        geo_size: (geo_w, geo_h),
    }
}

pub(crate) fn expanded_area(geo: Rectangle<i32, Physical>) -> Rectangle<i32, Physical> {
    let pad_w = (geo.size.w / 2).max(128);
    let pad_h = (geo.size.h / 2).max(128);
    Rectangle::new(
        (geo.loc.x - pad_w, geo.loc.y - pad_h).into(),
        (geo.size.w + pad_w * 2, geo.size.h + pad_h * 2).into(),
    )
}

fn compile_requested(
    renderer: &mut GlesRenderer,
    kind: ShaderKind,
    raw: Option<&str>,
    config_dir: Option<&Path>,
    current: Option<CompiledProgram>,
    failed: &mut Option<String>,
) -> Option<CompiledProgram> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = resolve_path(raw, config_dir);
    let key = program_key(kind, &path);
    if current
        .as_ref()
        .is_some_and(|program| program.key == key && program.context == renderer.context_id())
    {
        return current;
    }
    if failed.as_deref() == Some(key.as_str()) {
        return None;
    }
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eventline::warn!(
                "window shader: {} could not be read ({error}); using the configured type",
                path.display()
            );
            *failed = Some(key);
            return None;
        }
    };
    match compile_program(renderer, kind, &source) {
        Ok(program) => {
            *failed = None;
            Some(CompiledProgram {
                key,
                context: renderer.context_id(),
                program,
            })
        }
        Err(error) => {
            eventline::warn!(
                "window shader: {} failed to compile ({error:?}); using the configured type",
                path.display()
            );
            *failed = Some(key);
            None
        }
    }
}

fn compile_program(
    renderer: &mut GlesRenderer,
    kind: ShaderKind,
    user: &str,
) -> Result<GlesTexProgram, GlesError> {
    let mut source = String::from(PRELUDE);
    source.push_str(user);
    source.push_str(match kind {
        ShaderKind::Open => OPEN_EPILOGUE,
        ShaderKind::Close => CLOSE_EPILOGUE,
    });
    renderer.compile_custom_texture_shader(
        &source,
        &[
            UniformName::new("halley_progress", UniformType::_1f),
            UniformName::new("halley_clamped_progress", UniformType::_1f),
            UniformName::new("halley_random_seed", UniformType::_1f),
            UniformName::new("halley_input_scale", UniformType::_2f),
            UniformName::new("halley_input_offset", UniformType::_2f),
            UniformName::new("halley_tex_scale", UniformType::_2f),
            UniformName::new("halley_tex_offset", UniformType::_2f),
            UniformName::new("halley_geo_size", UniformType::_2f),
        ],
    )
}

fn program_key(kind: ShaderKind, path: &Path) -> String {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    format!("{kind:?}:{}:{modified:?}", path.display())
}

fn resolve_path(raw: &str, config_dir: Option<&Path>) -> PathBuf {
    let path = raw
        .strip_prefix("~/")
        .and_then(|rest| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest)))
        .unwrap_or_else(|| PathBuf::from(raw));
    if path.is_absolute() {
        path
    } else {
        config_dir
            .map(|directory| directory.join(&path))
            .unwrap_or(path)
    }
}

fn shader_commit(
    base: CommitCounter,
    progress: f32,
    clamped_progress: f32,
    random_seed: f32,
    mapping: ShaderMapping,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    base.distance(Some(CommitCounter::default()))
        .unwrap_or(usize::MAX)
        .hash(&mut hasher);
    progress.to_bits().hash(&mut hasher);
    clamped_progress.to_bits().hash(&mut hasher);
    random_seed.to_bits().hash(&mut hasher);
    mapping.draw_area.loc.x.hash(&mut hasher);
    mapping.draw_area.loc.y.hash(&mut hasher);
    mapping.draw_area.size.w.hash(&mut hasher);
    mapping.draw_area.size.h.hash(&mut hasher);
    CommitCounter::from(hasher.finish() as usize)
}

impl Element for WindowShaderRenderElement {
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

impl RenderElement<GlesRenderer> for WindowShaderRenderElement {
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
            self.transform(),
            self.alpha(),
            Some(&self.program),
            &[
                Uniform::new("halley_progress", self.progress),
                Uniform::new("halley_clamped_progress", self.clamped_progress),
                Uniform::new("halley_random_seed", self.random_seed),
                Uniform::new("halley_input_scale", self.mapping.input_scale),
                Uniform::new("halley_input_offset", self.mapping.input_offset),
                Uniform::new("halley_tex_scale", self.mapping.tex_scale),
                Uniform::new("halley_tex_offset", self.mapping.tex_offset),
                Uniform::new("halley_geo_size", self.mapping.geo_size),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{expanded_area, shader_mapping};
    use smithay::utils::Rectangle;

    #[test]
    fn mapping_puts_geometry_origin_at_zero() {
        let geo = Rectangle::new((100, 50).into(), (400, 300).into());
        let draw = Rectangle::new((0, 0).into(), (600, 400).into());
        let mapping = shader_mapping(geo, draw);
        let geo_of = |x: f32, y: f32| {
            (
                x * mapping.input_scale.0 + mapping.input_offset.0,
                y * mapping.input_scale.1 + mapping.input_offset.1,
            )
        };

        let origin = (
            (geo.loc.x - draw.loc.x) as f32 / draw.size.w as f32,
            (geo.loc.y - draw.loc.y) as f32 / draw.size.h as f32,
        );
        let far = (
            (geo.loc.x + geo.size.w - draw.loc.x) as f32 / draw.size.w as f32,
            (geo.loc.y + geo.size.h - draw.loc.y) as f32 / draw.size.h as f32,
        );
        let origin_geo = geo_of(origin.0, origin.1);
        let far_geo = geo_of(far.0, far.1);
        assert!((origin_geo.0).abs() < f32::EPSILON && origin_geo.1.abs() < f32::EPSILON);
        assert!((far_geo.0 - 1.0).abs() < 1e-5 && (far_geo.1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn expanded_area_pads_at_least_one_hundred_twenty_eight() {
        let geo = Rectangle::new((10, 20).into(), (40, 30).into());
        let area = expanded_area(geo);
        assert_eq!(area.loc, (-118, -108).into());
        assert_eq!(area.size, (296, 286).into());
    }
}
