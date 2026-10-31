use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use halley_config::{Background, BackgroundFit, BackgroundMode};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::{ContextId, ImportMem, Renderer};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

const SPACE_SHADER: &str = include_str!("shaders/space.frag");

struct ImageResource {
    key: String,
    texture: GlesTexture,
    size: Size<i32, Physical>,
}

struct ShaderResource {
    key: String,
    program: GlesTexProgram,
}

struct Resources {
    context: ContextId<GlesTexture>,
    unit_texture: Option<GlesTexture>,
    unit_texture_failed: bool,
    image: Option<ImageResource>,
    shader: Option<ShaderResource>,
    failed_image_key: Option<String>,
    failed_shader_key: Option<String>,
}

#[derive(Default)]
pub struct BackgroundRenderer {
    resources: Option<Resources>,
    ids: HashMap<String, Id>,
}

#[derive(Clone, Copy, Debug)]
struct FieldUniforms {
    resolution: (f32, f32),
    camera_center: (f32, f32),
    camera_size: (f32, f32),
    time: f32,
    intensity: f32,
    base_color: (f32, f32, f32),
    accent_color: (f32, f32, f32),
}

pub struct BackgroundElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    source: Rectangle<f64, Buffer>,
    destination: Rectangle<i32, Physical>,
    alpha: f32,
    program: Option<GlesTexProgram>,
    uniforms: Option<FieldUniforms>,
}

pub(super) struct BackgroundRequest<'a> {
    pub output_name: &'a str,
    pub output_size: Size<i32, Physical>,
    pub camera_center: Point<f32, Physical>,
    pub camera_size: (f32, f32),
    pub now: std::time::Duration,
    pub config: &'a Background,
    pub config_dir: Option<&'a Path>,
}

impl BackgroundRenderer {
    pub(super) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        request: BackgroundRequest<'_>,
    ) -> Option<BackgroundElement> {
        if request.config.mode == BackgroundMode::None
            || request.output_size.w <= 0
            || request.output_size.h <= 0
        {
            return None;
        }
        self.ensure_context(renderer);
        match request.config.mode {
            BackgroundMode::None => None,
            BackgroundMode::Classic => self.classic_element(renderer, &request),
            BackgroundMode::FieldShader => self.field_element(renderer, &request),
        }
    }

    fn ensure_context(&mut self, renderer: &GlesRenderer) {
        let context = renderer.context_id();
        if self
            .resources
            .as_ref()
            .is_some_and(|resources| resources.context == context)
        {
            return;
        }
        self.resources = Some(Resources {
            context,
            unit_texture: None,
            unit_texture_failed: false,
            image: None,
            shader: None,
            failed_image_key: None,
            failed_shader_key: None,
        });
    }

    fn classic_element(
        &mut self,
        renderer: &mut GlesRenderer,
        request: &BackgroundRequest<'_>,
    ) -> Option<BackgroundElement> {
        let raw_path = request.config.path.trim();
        if raw_path.is_empty() {
            return None;
        }
        let path = resolve_path(raw_path, request.config_dir);
        let key = path.to_string_lossy().into_owned();
        let needs_image = self
            .resources
            .as_ref()
            .and_then(|resources| resources.image.as_ref())
            .is_none_or(|image| image.key != key);
        if needs_image {
            let failed = self
                .resources
                .as_ref()
                .and_then(|resources| resources.failed_image_key.as_deref())
                == Some(key.as_str());
            if failed {
                return None;
            }
            match image::open(&path).map(|image| image.to_rgba8()) {
                Ok(image) => {
                    let size =
                        Size::<i32, Physical>::from((image.width() as i32, image.height() as i32));
                    let texture = match renderer.import_memory(
                        image.as_raw(),
                        Fourcc::Abgr8888,
                        (size.w, size.h).into(),
                        false,
                    ) {
                        Ok(texture) => texture,
                        Err(error) => {
                            eventline::warn!(
                                "background: image {} could not be uploaded: {error}",
                                path.display()
                            );
                            let resources = self.resources.as_mut().expect("context ensured");
                            resources.image = None;
                            resources.failed_image_key = Some(key);
                            return None;
                        }
                    };
                    let resources = self.resources.as_mut().expect("context ensured");
                    resources.image = Some(ImageResource {
                        key: key.clone(),
                        texture,
                        size,
                    });
                    resources.failed_image_key = None;
                }
                Err(error) => {
                    eventline::warn!(
                        "background: image {} could not be loaded: {error}",
                        path.display()
                    );
                    let resources = self.resources.as_mut().expect("context ensured");
                    resources.image = None;
                    resources.failed_image_key = Some(key);
                    return None;
                }
            }
        }
        let (image_key, texture, image_size) = {
            let image = self
                .resources
                .as_ref()
                .and_then(|resources| resources.image.as_ref())
                .expect("image loaded above");
            (image.key.clone(), image.texture.clone(), image.size)
        };
        let (source, destination) =
            image_layout(image_size, request.output_size, request.config.fit);
        let alpha = request.config.intensity.clamp(0.0, 1.0);
        Some(BackgroundElement {
            id: self.id(request.output_name),
            commit: classic_commit(&image_key, source, destination, alpha),
            texture,
            source,
            destination,
            alpha,
            program: None,
            uniforms: None,
        })
    }

    fn field_element(
        &mut self,
        renderer: &mut GlesRenderer,
        request: &BackgroundRequest<'_>,
    ) -> Option<BackgroundElement> {
        if !self.ensure_unit_texture(renderer) {
            return None;
        }
        let shader = shader_request(request.config.shader.trim(), request.config_dir);
        if !self.ensure_shader(renderer, &shader) {
            return None;
        }

        let resources = self.resources.as_ref().expect("context ensured");
        let texture = resources
            .unit_texture
            .as_ref()
            .expect("unit texture ensured")
            .clone();
        let program = resources
            .shader
            .as_ref()
            .expect("shader ensured")
            .program
            .clone();
        let uniforms = FieldUniforms {
            resolution: (
                request.output_size.w.max(1) as f32,
                request.output_size.h.max(1) as f32,
            ),
            camera_center: (request.camera_center.x, request.camera_center.y),
            camera_size: (
                request.camera_size.0.max(1.0),
                request.camera_size.1.max(1.0),
            ),
            time: if request.config.animated {
                request.now.as_secs_f32()
            } else {
                0.0
            },
            intensity: request.config.intensity,
            base_color: (
                request.config.color.r,
                request.config.color.g,
                request.config.color.b,
            ),
            accent_color: (
                request.config.accent_color.r,
                request.config.accent_color.g,
                request.config.accent_color.b,
            ),
        };
        let destination = Rectangle::from_size(request.output_size);
        Some(BackgroundElement {
            id: self.id(request.output_name),
            commit: field_commit(&shader.key, destination, uniforms),
            texture,
            source: Rectangle::new((0.0, 0.0).into(), (1.0, 1.0).into()),
            destination,
            alpha: 1.0,
            program: Some(program),
            uniforms: Some(uniforms),
        })
    }

    fn ensure_unit_texture(&mut self, renderer: &mut GlesRenderer) -> bool {
        let resources = self.resources.as_mut().expect("context ensured");
        if resources.unit_texture.is_some() {
            return true;
        }
        if resources.unit_texture_failed {
            return false;
        }
        match renderer.import_memory(
            &[255_u8, 255, 255, 255],
            Fourcc::Abgr8888,
            (1, 1).into(),
            false,
        ) {
            Ok(texture) => {
                resources.unit_texture = Some(texture);
                true
            }
            Err(error) => {
                eventline::warn!("background: shader texture could not be created: {error}");
                resources.unit_texture_failed = true;
                false
            }
        }
    }

    fn ensure_shader(&mut self, renderer: &mut GlesRenderer, request: &ShaderRequest) -> bool {
        let resources = self.resources.as_ref().expect("context ensured");
        if resources
            .shader
            .as_ref()
            .is_some_and(|shader| shader.key == request.key)
        {
            return true;
        }
        if resources.failed_shader_key.as_deref() == Some(request.key.as_str()) {
            return false;
        }
        let (source, custom) = match &request.source {
            ShaderSource::Builtin => (SPACE_SHADER.to_string(), false),
            ShaderSource::File(path) => match fs::read_to_string(path) {
                Ok(source) => (source, true),
                Err(error) => {
                    eventline::warn!(
                        "background: shader {} could not be read ({error}); using built-in space",
                        path.display()
                    );
                    (SPACE_SHADER.to_string(), false)
                }
            },
        };
        let uniforms = field_uniform_names();
        let compiled = renderer.compile_custom_texture_shader(&source, &uniforms);
        let program = match compiled {
            Ok(program) => program,
            Err(error) if custom => {
                eventline::warn!(
                    "background: custom shader failed to compile ({error}); using built-in space"
                );
                match renderer.compile_custom_texture_shader(SPACE_SHADER, &uniforms) {
                    Ok(program) => program,
                    Err(error) => {
                        eventline::warn!(
                            "background: built-in space shader failed to compile: {error}"
                        );
                        let resources = self.resources.as_mut().expect("context ensured");
                        resources.shader = None;
                        resources.failed_shader_key = Some(request.key.clone());
                        return false;
                    }
                }
            }
            Err(error) => {
                eventline::warn!("background: built-in space shader failed to compile: {error}");
                let resources = self.resources.as_mut().expect("context ensured");
                resources.shader = None;
                resources.failed_shader_key = Some(request.key.clone());
                return false;
            }
        };
        let resources = self.resources.as_mut().expect("context ensured");
        resources.shader = Some(ShaderResource {
            key: request.key.clone(),
            program,
        });
        resources.failed_shader_key = None;
        true
    }

    fn id(&mut self, output_name: &str) -> Id {
        self.ids
            .entry(output_name.to_string())
            .or_insert_with(Id::new)
            .clone()
    }
}

struct ShaderRequest {
    key: String,
    source: ShaderSource,
}

enum ShaderSource {
    Builtin,
    File(PathBuf),
}

fn field_uniform_names() -> [UniformName<'static>; 7] {
    [
        UniformName::new("u_resolution", UniformType::_2f),
        UniformName::new("u_camera_center", UniformType::_2f),
        UniformName::new("u_camera_size", UniformType::_2f),
        UniformName::new("u_time", UniformType::_1f),
        UniformName::new("u_intensity", UniformType::_1f),
        UniformName::new("u_base_color", UniformType::_3f),
        UniformName::new("u_accent_color", UniformType::_3f),
    ]
}

fn shader_request(raw: &str, config_dir: Option<&Path>) -> ShaderRequest {
    if raw.is_empty() || raw.eq_ignore_ascii_case("space") {
        return ShaderRequest {
            key: "builtin:space".to_string(),
            source: ShaderSource::Builtin,
        };
    }
    let path = resolve_path(raw, config_dir);
    ShaderRequest {
        key: format!("file:{}", path.display()),
        source: ShaderSource::File(path),
    }
}

fn resolve_path(raw: &str, config_dir: Option<&Path>) -> PathBuf {
    let trimmed = raw.trim();
    let path = trimmed
        .strip_prefix("~/")
        .and_then(|rest| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest)))
        .unwrap_or_else(|| PathBuf::from(trimmed));
    if path.is_absolute() {
        path
    } else {
        config_dir
            .map(|directory| directory.join(&path))
            .unwrap_or(path)
    }
}

fn image_layout(
    image: Size<i32, Physical>,
    output: Size<i32, Physical>,
    fit: BackgroundFit,
) -> (Rectangle<f64, Buffer>, Rectangle<i32, Physical>) {
    let full_source = Rectangle::new(
        (0.0, 0.0).into(),
        (f64::from(image.w.max(1)), f64::from(image.h.max(1))).into(),
    );
    let full_destination = Rectangle::from_size(output);
    match fit {
        BackgroundFit::Stretch => (full_source, full_destination),
        BackgroundFit::Cover => {
            let output_aspect = f64::from(output.w.max(1)) / f64::from(output.h.max(1));
            let image_aspect = f64::from(image.w.max(1)) / f64::from(image.h.max(1));
            let source = if image_aspect > output_aspect {
                let width = f64::from(image.h) * output_aspect;
                Rectangle::new(
                    ((f64::from(image.w) - width) * 0.5, 0.0).into(),
                    (width, f64::from(image.h)).into(),
                )
            } else {
                let height = f64::from(image.w) / output_aspect;
                Rectangle::new(
                    (0.0, (f64::from(image.h) - height) * 0.5).into(),
                    (f64::from(image.w), height).into(),
                )
            };
            (source, full_destination)
        }
        BackgroundFit::Contain => {
            let scale = (output.w as f32 / image.w.max(1) as f32)
                .min(output.h as f32 / image.h.max(1) as f32);
            let width = (image.w as f32 * scale).round().max(1.0) as i32;
            let height = (image.h as f32 * scale).round().max(1.0) as i32;
            (
                full_source,
                Rectangle::new(
                    ((output.w - width) / 2, (output.h - height) / 2).into(),
                    (width, height).into(),
                ),
            )
        }
    }
}

fn classic_commit(
    key: &str,
    source: Rectangle<f64, Buffer>,
    destination: Rectangle<i32, Physical>,
    alpha: f32,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    source.loc.x.to_bits().hash(&mut hasher);
    source.loc.y.to_bits().hash(&mut hasher);
    source.size.w.to_bits().hash(&mut hasher);
    source.size.h.to_bits().hash(&mut hasher);
    hash_rectangle(destination, &mut hasher);
    alpha.to_bits().hash(&mut hasher);
    CommitCounter::from(hasher.finish() as usize)
}

fn field_commit(
    key: &str,
    destination: Rectangle<i32, Physical>,
    uniforms: FieldUniforms,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hash_rectangle(destination, &mut hasher);
    for value in [
        uniforms.resolution.0,
        uniforms.resolution.1,
        uniforms.camera_center.0,
        uniforms.camera_center.1,
        uniforms.camera_size.0,
        uniforms.camera_size.1,
        uniforms.time,
        uniforms.intensity,
        uniforms.base_color.0,
        uniforms.base_color.1,
        uniforms.base_color.2,
        uniforms.accent_color.0,
        uniforms.accent_color.1,
        uniforms.accent_color.2,
    ] {
        value.to_bits().hash(&mut hasher);
    }
    CommitCounter::from(hasher.finish() as usize)
}

fn hash_rectangle(rectangle: Rectangle<i32, Physical>, hasher: &mut DefaultHasher) {
    rectangle.loc.x.hash(hasher);
    rectangle.loc.y.hash(hasher);
    rectangle.size.w.hash(hasher);
    rectangle.size.h.hash(hasher);
}

impl Element for BackgroundElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.source
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.destination
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for BackgroundElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        if let Some(uniforms) = self.uniforms {
            frame.render_texture_from_to(
                &self.texture,
                src,
                dst,
                damage,
                opaque_regions,
                Transform::Normal,
                self.alpha,
                self.program.as_ref(),
                &[
                    Uniform::new("u_resolution", uniforms.resolution),
                    Uniform::new("u_camera_center", uniforms.camera_center),
                    Uniform::new("u_camera_size", uniforms.camera_size),
                    Uniform::new("u_time", uniforms.time),
                    Uniform::new("u_intensity", uniforms.intensity),
                    Uniform::new("u_base_color", uniforms.base_color),
                    Uniform::new("u_accent_color", uniforms.accent_color),
                ],
            )
        } else {
            frame.render_texture_from_to(
                &self.texture,
                src,
                dst,
                damage,
                opaque_regions,
                Transform::Normal,
                self.alpha,
                None,
                &[],
            )
        }
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_long_image_axis() {
        let (source, destination) = image_layout(
            Size::from((2000, 1000)),
            Size::from((1000, 1000)),
            BackgroundFit::Cover,
        );
        assert_eq!(
            source,
            Rectangle::new((500.0, 0.0).into(), (1000.0, 1000.0).into())
        );
        assert_eq!(destination, Rectangle::from_size((1000, 1000).into()));
    }

    #[test]
    fn contain_centers_without_cropping() {
        let (source, destination) = image_layout(
            Size::from((2000, 1000)),
            Size::from((1000, 1000)),
            BackgroundFit::Contain,
        );
        assert_eq!(
            source,
            Rectangle::new((0.0, 0.0).into(), (2000.0, 1000.0).into())
        );
        assert_eq!(
            destination,
            Rectangle::new((0, 250).into(), (1000, 500).into())
        );
    }
}
