use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    Capability, GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
    UniformName, UniformType, ffi, link_program,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{ContextId, Offscreen, Renderer, Texture};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size, Transform};

const DOWN_SHADER: &str = include_str!("shaders/blur_down.frag");
const UP_SHADER: &str = include_str!("shaders/blur_up.frag");
const COMPOSITE_SHADER: &str = include_str!("shaders/blur_composite.frag");
const BLUR_VERTEX_SHADER: &str = r#"#version 100
attribute vec2 vert;
varying vec2 v_coords;

void main() {
    v_coords = vert;
    gl_Position = vec4(vert * 2.0 - 1.0, 1.0, 1.0);
}
"#;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurPatch {
    pub rect: Rectangle<i32, Physical>,
    pub radius: f32,
    pub alpha: f32,
    pub clip: Option<(
        Rectangle<i32, Physical>,
        crate::render::window_decoration::CornerRadii,
    )>,
}

struct Programs {
    context: ContextId<GlesTexture>,
    down: RawPassProgram,
    up: RawPassProgram,
    composite: GlesTexProgram,
}

#[derive(Clone, Copy)]
struct RawPassProgram {
    program: ffi::types::GLuint,
    texture: ffi::types::GLint,
    alpha: ffi::types::GLint,
    halfpixel: ffi::types::GLint,
    offset: ffi::types::GLint,
    vertex: ffi::types::GLint,
}

struct BlurTextures {
    size: Size<i32, Physical>,
    accum: GlesTexture,
    chain: Vec<GlesTexture>,
}

struct ElementBlur {
    result: GlesTexture,
    captured: Cell<bool>,
    ready: Cell<bool>,
}

struct OutputResources {
    textures: Option<Rc<RefCell<BlurTextures>>>,
    results: HashMap<BlurIdentity, Rc<ElementBlur>>,
    retry: Rc<RefCell<RetryState>>,
    size: Size<i32, Physical>,
    levels: u32,
    config_fingerprint: u64,
    ids: HashMap<BlurIdentity, Id>,
    scene_identities: HashSet<BlurIdentity>,
}

#[derive(Clone, Copy, Debug, Default)]
struct RetryState {
    attempts: u32,
    retry_at: Option<Instant>,
}

impl RetryState {
    fn blocked(self, now: Instant) -> bool {
        self.retry_at.is_some_and(|retry_at| now < retry_at)
    }

    fn begin_attempt(&mut self) {
        self.retry_at = None;
    }

    fn fail(&mut self, now: Instant) -> Duration {
        self.attempts = self.attempts.saturating_add(1);
        let delay = retry_delay(self.attempts);
        self.retry_at = Some(now + delay);
        delay
    }

    fn recover(&mut self) {
        *self = Self::default();
    }
}

/// Stable identity of one logical framebuffer-effect stack position. A
/// Wayland render-element `Id` is used where possible because protocol object
/// numbers are only unique within one client and formatted IDs can alias.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlurIdentity {
    Window { surface: Id, instance: String },
    Layer(Id),
    Overlay(&'static str),
}

#[derive(Default)]
pub struct BackdropBlurRenderer {
    programs: Option<Programs>,
    program_context: Option<ContextId<GlesTexture>>,
    program_retry: RetryState,
    unsupported_context: Option<ContextId<GlesTexture>>,
    outputs: HashMap<String, OutputResources>,
}

pub struct BackdropBlurElement {
    id: Id,
    commit: CommitCounter,
    size: Size<i32, Physical>,
    patches: Vec<BlurPatch>,
    textures: Rc<RefCell<BlurTextures>>,
    element: Rc<ElementBlur>,
    retry: Rc<RefCell<RetryState>>,
    down: RawPassProgram,
    up: RawPassProgram,
    composite: GlesTexProgram,
    offset: f32,
    saturation: f32,
    noise: f32,
}

impl BackdropBlurRenderer {
    pub fn begin_scene(&mut self, output: &str) {
        if let Some(resources) = self.outputs.get_mut(output) {
            resources.scene_identities.clear();
        }
    }

    pub fn remove_output(&mut self, output: &str) {
        self.outputs.remove(output);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn blur_element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &str,
        identity: BlurIdentity,
        size: Size<i32, Logical>,
        patches: Vec<BlurPatch>,
        config: halley_config::Blur,
        presentation_epoch: u64,
    ) -> Result<Option<BackdropBlurElement>, Box<dyn Error>> {
        if patches.is_empty() {
            return Ok(None);
        }
        let context = renderer.context_id();
        if self.program_context.as_ref() != Some(&context) {
            self.program_context = Some(context.clone());
            self.programs = None;
            self.outputs.clear();
            self.program_retry.recover();
            self.unsupported_context = None;
        }
        if !renderer.capabilities().contains(&Capability::Blit) {
            if self.unsupported_context.as_ref() != Some(&context) {
                eventline::warn!(
                    "backdrop-blur: framebuffer blit is unavailable on this GLES context; effect disabled"
                );
                self.unsupported_context = Some(context);
            }
            return Ok(None);
        }
        let now = Instant::now();
        if self.program_retry.blocked(now) {
            return Ok(None);
        }
        if self.programs.is_none() {
            self.program_retry.begin_attempt();
            if let Err(error) = self.ensure_programs(renderer) {
                let delay = self.program_retry.fail(now);
                eventline::warn!(
                    "backdrop-blur: shader setup failed; retrying in {} ms: {error}",
                    delay.as_millis()
                );
                return Ok(None);
            }
            self.program_retry.recover();
        }
        let physical_size = size.to_physical(1);
        let levels = config.passes.clamp(1, 5);
        let config_fingerprint = blur_config_fingerprint(config);
        let resources = self
            .outputs
            .entry(output.to_string())
            .or_insert_with(|| OutputResources {
                textures: None,
                results: HashMap::new(),
                retry: Rc::new(RefCell::new(RetryState::default())),
                size: physical_size,
                levels,
                config_fingerprint,
                ids: HashMap::new(),
                scene_identities: HashSet::new(),
            });
        if resources.size != physical_size
            || resources.levels != levels
            || resources.config_fingerprint != config_fingerprint
        {
            resources.size = physical_size;
            resources.levels = levels;
            resources.config_fingerprint = config_fingerprint;
            resources.textures = None;
            resources.results.clear();
            resources.retry.borrow_mut().recover();
        }
        if resources.retry.borrow().blocked(now) {
            return Ok(None);
        }
        if resources.retry.borrow().retry_at.is_some() {
            resources.retry.borrow_mut().begin_attempt();
            resources.textures = None;
        }
        if resources.textures.is_none() {
            match create_textures(renderer, physical_size, levels) {
                Ok(textures) => {
                    resources.textures = Some(Rc::new(RefCell::new(textures)));
                }
                Err(error) => {
                    let delay = resources.retry.borrow_mut().fail(now);
                    eventline::warn!(
                        "backdrop-blur: scratch allocation failed on {output}; retrying in {} ms: {error}",
                        delay.as_millis()
                    );
                    return Ok(None);
                }
            }
        }
        let programs = self.programs.as_ref().expect("ensured above");
        debug_assert_eq!(programs.context, context);
        if !resources.scene_identities.insert(identity.clone()) {
            return Err(
                format!("duplicate backdrop blur identity in one scene: {identity:?}").into(),
            );
        }
        let id = resources
            .ids
            .entry(identity.clone())
            .or_insert_with(Id::new)
            .clone();
        let element = if let Some(existing) = resources.results.get(&identity) {
            existing.captured.set(false);
            Rc::clone(existing)
        } else {
            match create_texture(renderer, physical_size) {
                Ok(result) => {
                    let element = Rc::new(ElementBlur {
                        result,
                        captured: Cell::new(false),
                        ready: Cell::new(false),
                    });
                    resources.results.insert(identity, Rc::clone(&element));
                    element
                }
                Err(error) => {
                    let delay = resources.retry.borrow_mut().fail(now);
                    eventline::warn!(
                        "backdrop-blur: result allocation failed on {output}; retrying in {} ms: {error}",
                        delay.as_millis()
                    );
                    return Ok(None);
                }
            }
        };
        let commit = blur_commit(&patches, config, presentation_epoch);
        Ok(Some(BackdropBlurElement {
            // Each stack position keeps a stable identity. Smithay can then
            // retain its per-effect capture cache without ever aliasing two
            // z-ordered surfaces that share the scratch textures.
            id,
            commit,
            size: physical_size,
            patches,
            textures: Rc::clone(resources.textures.as_ref().expect("allocated above")),
            element,
            retry: Rc::clone(&resources.retry),
            down: programs.down,
            up: programs.up,
            composite: programs.composite.clone(),
            offset: blur_offset(config.radius),
            saturation: config.saturation,
            noise: config.noise,
        }))
    }

    fn ensure_programs(&mut self, renderer: &mut GlesRenderer) -> Result<(), Box<dyn Error>> {
        let context = renderer.context_id();
        if self
            .programs
            .as_ref()
            .is_some_and(|programs| programs.context == context)
        {
            return Ok(());
        }
        let composite_uniforms = [
            UniformName::new("rect_size", UniformType::_2f),
            UniformName::new("patch_origin_uv", UniformType::_2f),
            UniformName::new("patch_size_uv", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_1f),
            UniformName::new("clip_origin", UniformType::_2f),
            UniformName::new("clip_size", UniformType::_2f),
            UniformName::new("clip_radii", UniformType::_2f),
            UniformName::new("use_clip", UniformType::_1f),
            UniformName::new("saturation", UniformType::_1f),
            UniformName::new("noise", UniformType::_1f),
        ];
        let composite =
            renderer.compile_custom_texture_shader(COMPOSITE_SHADER, &composite_uniforms)?;
        let down_source = raw_fragment_shader(DOWN_SHADER);
        let up_source = raw_fragment_shader(UP_SHADER);
        let (down, up) = renderer.with_context(|gl| unsafe {
            let down = compile_raw_pass_program(gl, &down_source)?;
            match compile_raw_pass_program(gl, &up_source) {
                Ok(up) => Ok::<_, GlesError>((down, up)),
                Err(error) => {
                    gl.DeleteProgram(down.program);
                    Err(error)
                }
            }
        })??;
        self.programs = Some(Programs {
            context,
            down,
            up,
            composite,
        });
        Ok(())
    }
}

fn raw_fragment_shader(source: &str) -> String {
    format!("#version 100\n{}", source.replacen("//_DEFINES\n", "", 1))
}

unsafe fn compile_raw_pass_program(
    gl: &ffi::Gles2,
    fragment: &str,
) -> Result<RawPassProgram, GlesError> {
    let program = unsafe { link_program(gl, BLUR_VERTEX_SHADER, fragment)? };
    let pass = RawPassProgram {
        program,
        texture: unsafe { gl.GetUniformLocation(program, c"tex".as_ptr()) },
        alpha: unsafe { gl.GetUniformLocation(program, c"alpha".as_ptr()) },
        halfpixel: unsafe { gl.GetUniformLocation(program, c"halfpixel".as_ptr()) },
        offset: unsafe { gl.GetUniformLocation(program, c"offset".as_ptr()) },
        vertex: unsafe { gl.GetAttribLocation(program, c"vert".as_ptr()) },
    };
    if [
        pass.texture,
        pass.alpha,
        pass.halfpixel,
        pass.offset,
        pass.vertex,
    ]
    .into_iter()
    .any(|location| location < 0)
    {
        unsafe { gl.DeleteProgram(program) };
        return Err(GlesError::ShaderCompileError);
    }
    Ok(pass)
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 16,
        _ => 30,
    })
}

fn blur_config_fingerprint(config: halley_config::Blur) -> u64 {
    let mut hash = DefaultHasher::new();
    config.passes.hash(&mut hash);
    config.radius.to_bits().hash(&mut hash);
    config.saturation.to_bits().hash(&mut hash);
    config.noise.to_bits().hash(&mut hash);
    hash.finish()
}

fn level_size(size: Size<i32, Physical>, level: u32) -> Size<i32, Physical> {
    let shift = level + 1;
    ((size.w >> shift).max(1), (size.h >> shift).max(1)).into()
}

fn create_texture(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
) -> Result<GlesTexture, GlesError> {
    <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        renderer,
        Fourcc::Argb8888,
        (size.w.max(1), size.h.max(1)).into(),
    )
}

fn create_textures(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    levels: u32,
) -> Result<BlurTextures, GlesError> {
    let levels = levels.clamp(1, 5);
    let mut chain = Vec::with_capacity(levels as usize);
    for level in 0..levels {
        chain.push(create_texture(renderer, level_size(size, level))?);
    }
    Ok(BlurTextures {
        size,
        accum: create_texture(renderer, size)?,
        chain,
    })
}

#[derive(Clone, Copy)]
struct VertexAttribState {
    index: u32,
    enabled: bool,
    buffer: u32,
    size: i32,
    kind: u32,
    normalized: bool,
    stride: i32,
    pointer: *mut std::ffi::c_void,
}

unsafe fn vertex_attrib_state(gl: &ffi::Gles2, index: u32) -> VertexAttribState {
    let mut enabled = 0;
    let mut buffer = 0;
    let mut size = 0;
    let mut kind = 0;
    let mut normalized = 0;
    let mut stride = 0;
    let mut pointer: *mut std::ffi::c_void = std::ptr::null_mut();
    unsafe {
        gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_ENABLED, &mut enabled);
        gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_BUFFER_BINDING, &mut buffer);
        gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_SIZE, &mut size);
        gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_TYPE, &mut kind);
        gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_NORMALIZED, &mut normalized);
        gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_STRIDE, &mut stride);
        gl.GetVertexAttribPointerv(
            index,
            ffi::VERTEX_ATTRIB_ARRAY_POINTER,
            std::ptr::addr_of_mut!(pointer).cast_const(),
        );
    }
    VertexAttribState {
        index,
        enabled: enabled != 0,
        buffer: buffer as u32,
        size,
        kind: kind as u32,
        normalized: normalized != 0,
        stride,
        pointer,
    }
}

unsafe fn restore_vertex_attrib(gl: &ffi::Gles2, state: VertexAttribState) {
    unsafe {
        gl.BindBuffer(ffi::ARRAY_BUFFER, state.buffer);
        gl.VertexAttribPointer(
            state.index,
            state.size,
            state.kind,
            if state.normalized {
                ffi::TRUE
            } else {
                ffi::FALSE
            },
            state.stride,
            state.pointer,
        );
        if state.enabled {
            gl.EnableVertexAttribArray(state.index);
        } else {
            gl.DisableVertexAttribArray(state.index);
        }
    }
}

unsafe fn clear_gl_errors(gl: &ffi::Gles2) {
    for _ in 0..32 {
        if unsafe { gl.GetError() } == ffi::NO_ERROR {
            break;
        }
    }
}

fn run_blur(
    frame: &mut GlesFrame<'_, '_>,
    textures: &mut BlurTextures,
    result: &GlesTexture,
    down: RawPassProgram,
    up: RawPassProgram,
    offset: f32,
) -> Result<(), GlesError> {
    let size = textures.size;
    frame.with_context(|gl| unsafe {
        let mut draw_fbo = 0_i32;
        let mut read_fbo = 0_i32;
        let mut viewport = [0_i32; 4];
        let mut active_texture = 0_i32;
        let mut texture_binding = 0_i32;
        let mut program = 0_i32;
        let mut array_buffer = 0_i32;
        gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut draw_fbo);
        gl.GetIntegerv(ffi::READ_FRAMEBUFFER_BINDING, &mut read_fbo);
        gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());
        gl.GetIntegerv(ffi::ACTIVE_TEXTURE, &mut active_texture);
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.GetIntegerv(ffi::TEXTURE_BINDING_2D, &mut texture_binding);
        gl.GetIntegerv(ffi::CURRENT_PROGRAM, &mut program);
        gl.GetIntegerv(ffi::ARRAY_BUFFER_BINDING, &mut array_buffer);
        let blend_enabled = gl.IsEnabled(ffi::BLEND) == ffi::TRUE;
        let scissor_enabled = gl.IsEnabled(ffi::SCISSOR_TEST) == ffi::TRUE;
        let mut attribs = vec![vertex_attrib_state(gl, down.vertex as u32)];
        if up.vertex != down.vertex {
            attribs.push(vertex_attrib_state(gl, up.vertex as u32));
        }
        clear_gl_errors(gl);

        let mut fbo = 0;
        gl.GenFramebuffers(1, &mut fbo);
        let result = (|| {
            gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
            gl.Disable(ffi::BLEND);
            gl.Disable(ffi::SCISSOR_TEST);

            let vertices: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];
            let render_pass = |source: &GlesTexture,
                               source_size: Size<i32, Physical>,
                               target: &GlesTexture,
                               target_size: Size<i32, Physical>,
                               pass: RawPassProgram|
             -> Result<(), GlesError> {
                gl.UseProgram(pass.program);
                gl.Uniform1i(pass.texture, 0);
                gl.Uniform1f(pass.alpha, 1.0);
                gl.Uniform2f(
                    pass.halfpixel,
                    0.5 / source_size.w.max(1) as f32,
                    0.5 / source_size.h.max(1) as f32,
                );
                gl.Uniform1f(pass.offset, offset);
                gl.Viewport(0, 0, target_size.w, target_size.h);
                gl.FramebufferTexture2D(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    target.tex_id(),
                    0,
                );
                if gl.CheckFramebufferStatus(ffi::DRAW_FRAMEBUFFER) != ffi::FRAMEBUFFER_COMPLETE {
                    return Err(GlesError::FramebufferBindingError);
                }
                gl.BindTexture(ffi::TEXTURE_2D, source.tex_id());
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
                gl.EnableVertexAttribArray(pass.vertex as u32);
                gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
                gl.VertexAttribPointer(
                    pass.vertex as u32,
                    2,
                    ffi::FLOAT,
                    ffi::FALSE,
                    0,
                    vertices.as_ptr().cast(),
                );
                gl.DrawArrays(ffi::TRIANGLES, 0, 6);
                Ok(())
            };

            render_pass(
                &textures.accum,
                size,
                &textures.chain[0],
                level_size(size, 0),
                down,
            )?;
            for index in 1..textures.chain.len() {
                render_pass(
                    &textures.chain[index - 1],
                    level_size(size, index as u32 - 1),
                    &textures.chain[index],
                    level_size(size, index as u32),
                    down,
                )?;
            }
            for index in (1..textures.chain.len()).rev() {
                render_pass(
                    &textures.chain[index],
                    level_size(size, index as u32),
                    &textures.chain[index - 1],
                    level_size(size, index as u32 - 1),
                    up,
                )?;
            }
            render_pass(&textures.chain[0], level_size(size, 0), result, size, up)?;
            Ok(())
        })();
        gl.DeleteFramebuffers(1, &fbo);

        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, read_fbo as u32);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, draw_fbo as u32);
        gl.Viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        gl.BindTexture(ffi::TEXTURE_2D, texture_binding as u32);
        gl.ActiveTexture(active_texture as u32);
        gl.UseProgram(program as u32);
        if blend_enabled {
            gl.Enable(ffi::BLEND);
        } else {
            gl.Disable(ffi::BLEND);
        }
        if scissor_enabled {
            gl.Enable(ffi::SCISSOR_TEST);
        } else {
            gl.Disable(ffi::SCISSOR_TEST);
        }
        for attrib in attribs {
            restore_vertex_attrib(gl, attrib);
        }
        gl.BindBuffer(ffi::ARRAY_BUFFER, array_buffer as u32);
        if gl.GetError() == ffi::NO_ERROR {
            result
        } else {
            Err(GlesError::BlitError)
        }
    })?
}

impl Element for BackdropBlurElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::new(
            (0.0, 0.0).into(),
            (f64::from(self.size.w), f64::from(self.size.h)).into(),
        )
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_size(self.size)
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }

    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl RenderElement<GlesRenderer> for BackdropBlurElement {
    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        let now = Instant::now();
        if self.retry.borrow().blocked(now) {
            return Ok(());
        }
        let textures = self.textures.borrow();
        let size = textures.size;
        let capture = frame.with_context(|gl| unsafe {
            let mut current_draw_fbo = 0_i32;
            let mut current_read_fbo = 0_i32;
            gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut current_draw_fbo);
            gl.GetIntegerv(ffi::READ_FRAMEBUFFER_BINDING, &mut current_read_fbo);
            let scissor_was_enabled = gl.IsEnabled(ffi::SCISSOR_TEST) == ffi::TRUE;
            clear_gl_errors(gl);
            // Finish prior scene draws before sampling the output FBO. Tile
            // GPUs otherwise blit an incomplete frame, which flickers worst
            // on startup and any time the command stream is still in flight.
            gl.Flush();
            gl.Disable(ffi::SCISSOR_TEST);
            let mut fbo = 0;
            gl.GenFramebuffers(1, &mut fbo);
            let result = (|| {
                // Smithay leaves the output framebuffer bound for drawing.
                // Bind it explicitly for reading as well: blur passes use
                // offscreen FBOs, so a stale READ binding can capture the
                // wrong texture.
                gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, current_draw_fbo as u32);
                if gl.CheckFramebufferStatus(ffi::READ_FRAMEBUFFER) != ffi::FRAMEBUFFER_COMPLETE {
                    return Err(GlesError::FramebufferBindingError);
                }
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
                gl.FramebufferTexture2D(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    textures.accum.tex_id(),
                    0,
                );
                if gl.CheckFramebufferStatus(ffi::DRAW_FRAMEBUFFER) != ffi::FRAMEBUFFER_COMPLETE {
                    return Err(GlesError::FramebufferBindingError);
                }
                gl.BlitFramebuffer(
                    0,
                    0,
                    size.w,
                    size.h,
                    0,
                    0,
                    size.w,
                    size.h,
                    ffi::COLOR_BUFFER_BIT,
                    ffi::NEAREST,
                );
                Ok(())
            })();
            gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, current_read_fbo as u32);
            gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, current_draw_fbo as u32);
            if scissor_was_enabled {
                gl.Enable(ffi::SCISSOR_TEST);
            } else {
                gl.Disable(ffi::SCISSOR_TEST);
            }
            gl.DeleteFramebuffers(1, &fbo);
            if gl.GetError() == ffi::NO_ERROR {
                result
            } else {
                Err(GlesError::BlitError)
            }
        })?;
        if let Err(error) = capture {
            suspend_blur(&self.retry, "framebuffer capture", &error);
            return Ok(());
        }
        self.element.captured.set(true);
        Ok(())
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let now = Instant::now();
        if self.retry.borrow().blocked(now) {
            return Ok(());
        }
        if self.element.captured.get() {
            let mut textures = self.textures.borrow_mut();
            if let Err(error) = run_blur(
                frame,
                &mut textures,
                &self.element.result,
                self.down,
                self.up,
                self.offset,
            ) {
                suspend_blur(&self.retry, "render passes", &error);
                return Ok(());
            }
            self.element.captured.set(false);
            self.element.ready.set(true);
            self.retry.borrow_mut().recover();
        }
        if !self.element.ready.get() {
            return Ok(());
        }
        for patch in &self.patches {
            if let Err(error) = composite_patch(
                frame,
                &self.element.result,
                &self.composite,
                *patch,
                damage,
                self.saturation,
                self.noise,
            ) {
                suspend_blur(&self.retry, "composite pass", &error);
                return Ok(());
            }
        }
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

fn suspend_blur(retry: &Rc<RefCell<RetryState>>, stage: &str, error: &GlesError) {
    let delay = retry.borrow_mut().fail(Instant::now());
    eventline::warn!(
        "backdrop-blur: {stage} failed; retrying in {} ms: {error}",
        delay.as_millis()
    );
}

fn composite_patch(
    frame: &mut GlesFrame<'_, '_>,
    texture: &GlesTexture,
    program: &GlesTexProgram,
    patch: BlurPatch,
    damage: &[Rectangle<i32, Physical>],
    saturation: f32,
    noise: f32,
) -> Result<(), GlesError> {
    let local_damage = damage
        .iter()
        .filter_map(|damage| {
            patch.rect.intersection(*damage).map(|visible| {
                Rectangle::new(
                    (
                        visible.loc.x - patch.rect.loc.x,
                        visible.loc.y - patch.rect.loc.y,
                    )
                        .into(),
                    visible.size,
                )
            })
        })
        .collect::<Vec<_>>();
    if local_damage.is_empty() || patch.alpha <= 0.0 {
        return Ok(());
    }
    let texture_size = texture.size();
    let (clip_origin, clip_size, clip_radii, use_clip) = patch
        .clip
        .map(|(clip, radii)| {
            (
                (clip.loc.x as f32, clip.loc.y as f32),
                (clip.size.w as f32, clip.size.h as f32),
                (radii.top.max(0.0), radii.bottom.max(0.0)),
                1.0,
            )
        })
        .unwrap_or(((0.0, 0.0), (1.0, 1.0), (0.0, 0.0), 0.0));
    frame.render_texture_from_to(
        texture,
        Rectangle::<f64, Buffer>::new(
            (f64::from(patch.rect.loc.x), f64::from(patch.rect.loc.y)).into(),
            (f64::from(patch.rect.size.w), f64::from(patch.rect.size.h)).into(),
        ),
        patch.rect,
        &local_damage,
        &[],
        Transform::Normal,
        patch.alpha.clamp(0.0, 1.0),
        Some(program),
        &[
            Uniform::new(
                "rect_size",
                (patch.rect.size.w as f32, patch.rect.size.h as f32),
            ),
            Uniform::new(
                "patch_origin_uv",
                (
                    patch.rect.loc.x as f32 / texture_size.w.max(1) as f32,
                    patch.rect.loc.y as f32 / texture_size.h.max(1) as f32,
                ),
            ),
            Uniform::new(
                "patch_size_uv",
                (
                    patch.rect.size.w as f32 / texture_size.w.max(1) as f32,
                    patch.rect.size.h as f32 / texture_size.h.max(1) as f32,
                ),
            ),
            Uniform::new("corner_radius", patch.radius.max(0.0)),
            Uniform::new("clip_origin", clip_origin),
            Uniform::new("clip_size", clip_size),
            Uniform::new("clip_radii", clip_radii),
            Uniform::new("use_clip", use_clip),
            Uniform::new("saturation", saturation.clamp(0.0, 4.0)),
            Uniform::new("noise", noise.clamp(0.0, 0.25)),
        ],
    )
}

fn blur_offset(radius: f32) -> f32 {
    (radius / 16.0).clamp(0.6, 3.0)
}

fn blur_commit(
    patches: &[BlurPatch],
    config: halley_config::Blur,
    presentation_epoch: u64,
) -> CommitCounter {
    let mut hash = DefaultHasher::new();
    // Camera zoom/pan can move a window by sub-pixel amounts that round to
    // the same integer patch rects. Smithay only recaptures framebuffer
    // effects when this commit changes or something *behind* the effect is
    // damaged, and the window surface sits in front of its own blur. Mixing
    // the live camera into the commit forces a recapture on every zoom tick
    // instead of compositing a stale blur onto a freshly cleared swapchain
    // buffer.
    presentation_epoch.hash(&mut hash);
    config.passes.hash(&mut hash);
    config.radius.to_bits().hash(&mut hash);
    config.saturation.to_bits().hash(&mut hash);
    config.noise.to_bits().hash(&mut hash);
    for patch in patches {
        patch.rect.loc.x.hash(&mut hash);
        patch.rect.loc.y.hash(&mut hash);
        patch.rect.size.w.hash(&mut hash);
        patch.rect.size.h.hash(&mut hash);
        patch.radius.to_bits().hash(&mut hash);
        patch.alpha.to_bits().hash(&mut hash);
        if let Some((clip, radii)) = patch.clip {
            true.hash(&mut hash);
            clip.loc.x.hash(&mut hash);
            clip.loc.y.hash(&mut hash);
            clip.size.w.hash(&mut hash);
            clip.size.h.hash(&mut hash);
            radii.top.to_bits().hash(&mut hash);
            radii.bottom.to_bits().hash(&mut hash);
        } else {
            false.hash(&mut hash);
        }
    }
    CommitCounter::from(hash.finish() as usize)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use smithay::backend::renderer::element::Id;
    use smithay::utils::{Physical, Size};

    use super::{
        BlurIdentity, DOWN_SHADER, RetryState, level_size, raw_fragment_shader, retry_delay,
    };

    #[test]
    fn typed_identities_do_not_alias_across_scene_roles() {
        let surface = Id::new();
        let identities = [
            BlurIdentity::Layer(surface.clone()),
            BlurIdentity::Window {
                surface,
                instance: "canonical".to_string(),
            },
            BlurIdentity::Overlay("canonical"),
        ];

        assert_eq!(identities.into_iter().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn a_stack_position_keeps_the_same_identity_across_frames() {
        assert_eq!(
            BlurIdentity::Overlay("shell-overlay"),
            BlurIdentity::Overlay("shell-overlay")
        );
    }

    #[test]
    fn blur_levels_stay_nonzero_and_halve_per_pass() {
        let size = Size::<i32, Physical>::from((9, 5));
        assert_eq!(level_size(size, 0), Size::from((4, 2)));
        assert_eq!(level_size(size, 1), Size::from((2, 1)));
        assert_eq!(level_size(size, 4), Size::from((1, 1)));
    }

    #[test]
    fn raw_blur_shader_has_a_gles_version_and_no_smithay_defines_marker() {
        let shader = raw_fragment_shader(DOWN_SHADER);
        assert!(shader.starts_with("#version 100\n"));
        assert!(!shader.contains("//_DEFINES"));
    }

    #[test]
    fn blur_commit_changes_when_the_camera_moves_even_if_patches_do_not() {
        use smithay::utils::Rectangle;

        use super::{BlurPatch, blur_commit};

        let patches = [BlurPatch {
            rect: Rectangle::new((40, 80).into(), (400, 300).into()),
            radius: 0.0,
            alpha: 1.0,
            clip: None,
        }];
        let config = halley_config::Blur::default();

        assert_ne!(
            blur_commit(&patches, config, 0),
            blur_commit(&patches, config, 1)
        );
        assert_eq!(
            blur_commit(&patches, config, 7),
            blur_commit(&patches, config, 7)
        );
    }

    #[test]
    fn transient_failures_back_off_and_recovery_clears_the_budget() {
        assert_eq!(retry_delay(1), std::time::Duration::from_secs(1));
        assert_eq!(retry_delay(2), std::time::Duration::from_secs(2));
        assert_eq!(retry_delay(5), std::time::Duration::from_secs(16));
        assert_eq!(retry_delay(20), std::time::Duration::from_secs(30));

        let now = std::time::Instant::now();
        let mut retry = RetryState::default();
        assert_eq!(retry.fail(now), std::time::Duration::from_secs(1));
        assert!(retry.blocked(now));
        retry.begin_attempt();
        assert!(!retry.blocked(now));
        assert_eq!(retry.fail(now), std::time::Duration::from_secs(2));
        retry.recover();
        assert_eq!(retry.attempts, 0);
        assert!(!retry.blocked(now));
    }
}
