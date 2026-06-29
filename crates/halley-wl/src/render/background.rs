use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use halley_config::{BackgroundFit, BackgroundMode};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Texture,
            gles::{GlesFrame, GlesRenderer, Uniform, UniformName, UniformType},
        },
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;

const HALLEY_GESSO_SHADER: &str = include_str!("shaders/gesso_solarsystem.frag");

pub(crate) fn background_animates(st: &Halley) -> bool {
    st.runtime.tuning.background.mode == BackgroundMode::FieldShader
        && st.runtime.tuning.background.animated
}

pub(crate) fn ensure_background_resources(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    match st.runtime.tuning.background.mode {
        BackgroundMode::None => Ok(()),
        BackgroundMode::Classic => ensure_classic_texture(renderer, st),
        BackgroundMode::FieldShader => ensure_field_shader(renderer, st),
    }
}

pub(crate) fn draw_background(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    now: std::time::Instant,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    match st.runtime.tuning.background.mode {
        BackgroundMode::None => Ok(()),
        BackgroundMode::Classic => draw_classic(frame, st, size, damage),
        BackgroundMode::FieldShader => draw_field_shader(frame, st, size, damage, now),
    }
}

fn ensure_unit_texture(renderer: &mut GlesRenderer, st: &mut Halley) -> Result<(), Box<dyn Error>> {
    if st.ui.render_state.gpu.background_unit_texture.is_some() {
        return Ok(());
    }

    let pixel = [255u8, 255, 255, 255];
    st.ui.render_state.gpu.background_unit_texture =
        Some(renderer.import_memory(&pixel, Fourcc::Abgr8888, (1, 1).into(), false)?);
    Ok(())
}

fn ensure_field_shader(renderer: &mut GlesRenderer, st: &mut Halley) -> Result<(), Box<dyn Error>> {
    ensure_unit_texture(renderer, st)?;
    let (key, source) = field_shader_source(st);
    if st.ui.render_state.gpu.background_shader_key.as_deref() == Some(key.as_str())
        && st.ui.render_state.gpu.background_shader_program.is_some()
    {
        return Ok(());
    }
    if st
        .ui
        .render_state
        .gpu
        .background_shader_failed_key
        .as_deref()
        == Some(key.as_str())
    {
        return Ok(());
    }

    match renderer.compile_custom_texture_shader(
        source.as_str(),
        &[
            UniformName::new("u_resolution", UniformType::_2f),
            UniformName::new("u_camera_center", UniformType::_2f),
            UniformName::new("u_camera_size", UniformType::_2f),
            UniformName::new("u_time", UniformType::_1f),
            UniformName::new("u_intensity", UniformType::_1f),
            UniformName::new("u_base_color", UniformType::_3f),
            UniformName::new("u_accent_color", UniformType::_3f),
        ],
    ) {
        Ok(program) => {
            st.ui.render_state.gpu.background_shader_key = Some(key);
            st.ui.render_state.gpu.background_shader_failed_key = None;
            st.ui.render_state.gpu.background_shader_program = Some(program);
        }
        Err(err) => {
            st.ui.render_state.gpu.background_shader_key = None;
            st.ui.render_state.gpu.background_shader_program = None;
            st.ui.render_state.gpu.background_shader_failed_key = Some(key);
            eventline::warn!("gesso shader disabled: {err}");
        }
    }
    Ok(())
}

fn field_shader_source(st: &Halley) -> (String, String) {
    let cfg = &st.runtime.tuning.background;
    if !cfg.path.trim().is_empty() {
        let path = expand_user_path(cfg.path.as_str());
        return match fs::read_to_string(path.as_path()) {
            Ok(source) => (format!("path:{}", path.to_string_lossy()), source),
            Err(err) => {
                eventline::warn!(
                    "gesso shader path '{}' could not be read: {err}; falling back to stars",
                    path.to_string_lossy()
                );
                ("builtin:stars".to_string(), HALLEY_GESSO_SHADER.to_string())
            }
        };
    }

    match cfg.shader.trim().to_ascii_lowercase().as_str() {
        "" | "stars" => ("builtin:stars".to_string(), HALLEY_GESSO_SHADER.to_string()),
        other => {
            let path = expand_user_path(other);
            match fs::read_to_string(path.as_path()) {
                Ok(source) => (format!("path:{}", path.to_string_lossy()), source),
                Err(err) => {
                    eventline::warn!(
                        "unknown gesso shader '{}' ({err}); falling back to stars",
                        cfg.shader
                    );
                    ("builtin:stars".to_string(), HALLEY_GESSO_SHADER.to_string())
                }
            }
        }
    }
}

fn ensure_classic_texture(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    let raw_path = st.runtime.tuning.background.path.trim();
    if raw_path.is_empty() {
        return Ok(());
    }
    let path = expand_user_path(raw_path);
    let key = path.to_string_lossy().to_string();
    if st.ui.render_state.gpu.background_image_key.as_deref() == Some(key.as_str())
        && st.ui.render_state.gpu.background_image_texture.is_some()
    {
        return Ok(());
    }
    if st
        .ui
        .render_state
        .gpu
        .background_image_failed_key
        .as_deref()
        == Some(key.as_str())
    {
        return Ok(());
    }

    match image::open(path.as_path()).map(|image| image.to_rgba8()) {
        Ok(image) => {
            let width = image.width() as i32;
            let height = image.height() as i32;
            let texture = renderer.import_memory(
                image.as_raw(),
                Fourcc::Abgr8888,
                (width, height).into(),
                false,
            )?;
            st.ui.render_state.gpu.background_image_key = Some(key);
            st.ui.render_state.gpu.background_image_failed_key = None;
            st.ui.render_state.gpu.background_image_size = (width, height);
            st.ui.render_state.gpu.background_image_texture = Some(texture);
        }
        Err(err) => {
            st.ui.render_state.gpu.background_image_key = None;
            st.ui.render_state.gpu.background_image_texture = None;
            st.ui.render_state.gpu.background_image_failed_key = Some(key);
            eventline::warn!(
                "classic gesso image '{}' could not be loaded: {err}",
                path.to_string_lossy()
            );
        }
    }
    Ok(())
}

fn draw_field_shader(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    now: std::time::Instant,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    let Some(texture) = st.ui.render_state.gpu.background_unit_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = st.ui.render_state.gpu.background_shader_program.as_ref() else {
        return Ok(());
    };
    let view = crate::compositor::monitor::camera::camera_view_size(st);
    let time = if st.runtime.tuning.background.animated {
        st.now_ms(now) as f32 / 1000.0
    } else {
        0.0
    };
    let uniforms = [
        Uniform::new("u_resolution", (size.w.max(1) as f32, size.h.max(1) as f32)),
        Uniform::new(
            "u_camera_center",
            (st.model.viewport.center.x, st.model.viewport.center.y),
        ),
        Uniform::new("u_camera_size", (view.x.max(1.0), view.y.max(1.0))),
        Uniform::new("u_time", time),
        Uniform::new("u_intensity", st.runtime.tuning.background.intensity),
        Uniform::new(
            "u_base_color",
            (
                st.runtime.tuning.background.color.r,
                st.runtime.tuning.background.color.g,
                st.runtime.tuning.background.color.b,
            ),
        ),
        Uniform::new(
            "u_accent_color",
            (
                st.runtime.tuning.background.accent_color.r,
                st.runtime.tuning.background.accent_color.g,
                st.runtime.tuning.background.accent_color.b,
            ),
        ),
    ];
    let result =
        render_fullscreen_texture(frame, texture, size, damage, 1.0, Some(program), &uniforms);
    if result.is_ok() && st.runtime.tuning.background.animated {
        let monitor = st.model.monitor_state.current_monitor.clone();
        st.ui
            .render_state
            .note_background_animation_frame(monitor.as_str(), st.now_ms(now));
    }
    result
}

fn draw_classic(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    let Some(texture) = st.ui.render_state.gpu.background_image_texture.as_ref() else {
        return Ok(());
    };
    let (img_w, img_h) = st.ui.render_state.gpu.background_image_size;
    if img_w <= 0 || img_h <= 0 || size.w <= 0 || size.h <= 0 {
        return Ok(());
    }

    let full_src =
        Rectangle::<f64, Buffer>::new((0.0, 0.0).into(), (img_w as f64, img_h as f64).into());
    let full_dst =
        Rectangle::<i32, Physical>::new((0, 0).into(), (size.w.max(1), size.h.max(1)).into());
    let (src, dst) = match st.runtime.tuning.background.fit {
        BackgroundFit::Stretch => (full_src, full_dst),
        BackgroundFit::Cover => {
            let output_aspect = size.w.max(1) as f64 / size.h.max(1) as f64;
            let image_aspect = img_w.max(1) as f64 / img_h.max(1) as f64;
            let src = if image_aspect > output_aspect {
                let crop_w = img_h as f64 * output_aspect;
                Rectangle::<f64, Buffer>::new(
                    ((img_w as f64 - crop_w) * 0.5, 0.0).into(),
                    (crop_w, img_h as f64).into(),
                )
            } else {
                let crop_h = img_w as f64 / output_aspect;
                Rectangle::<f64, Buffer>::new(
                    (0.0, (img_h as f64 - crop_h) * 0.5).into(),
                    (img_w as f64, crop_h).into(),
                )
            };
            (src, full_dst)
        }
        BackgroundFit::Contain => {
            let scale = (size.w as f32 / img_w as f32).min(size.h as f32 / img_h as f32);
            let dst_w = (img_w as f32 * scale).round() as i32;
            let dst_h = (img_h as f32 * scale).round() as i32;
            let dst = Rectangle::<i32, Physical>::new(
                ((size.w - dst_w) / 2, (size.h - dst_h) / 2).into(),
                (dst_w.max(1), dst_h.max(1)).into(),
            );
            (full_src, dst)
        }
    };
    frame.render_texture_from_to(
        texture,
        src,
        dst,
        &[damage],
        &[],
        Transform::Normal,
        st.runtime.tuning.background.intensity.clamp(0.0, 1.0),
        None,
        &[],
    )
}

fn render_fullscreen_texture(
    frame: &mut GlesFrame<'_, '_>,
    texture: &smithay::backend::renderer::gles::GlesTexture,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
    program: Option<&smithay::backend::renderer::gles::GlesTexProgram>,
    uniforms: &[Uniform<'_>],
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    let tex_size = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let dst = Rectangle::<i32, Physical>::new((0, 0).into(), (size.w.max(1), size.h.max(1)).into());
    frame.render_texture_from_to(
        texture,
        src,
        dst,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        program,
        uniforms,
    )
}

fn expand_user_path(raw: &str) -> PathBuf {
    let mut out = raw.trim().trim_matches('"').to_string();
    if let Some(rest) = out.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        out = Path::new(&home).join(rest).to_string_lossy().to_string();
    }
    expand_env_vars(out.as_str())
}

fn expand_env_vars(raw: &str) -> PathBuf {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if raw[i..].starts_with("$env.") {
            let start = i + 5;
            let mut end = start;
            while end < bytes.len() {
                let ch = bytes[end] as char;
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    end += 1;
                } else {
                    break;
                }
            }
            if end > start {
                let key = &raw[start..end];
                if let Ok(value) = std::env::var(key) {
                    out.push_str(value.as_str());
                }
                i = end;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    PathBuf::from(out)
}
