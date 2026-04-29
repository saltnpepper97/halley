use std::error::Error;

mod draw;
mod scene;

use smithay::{
    backend::{
        renderer::{
            Color32F, Frame, Renderer,
            gles::{GlesRenderer, GlesTarget, UniformName, UniformType},
        },
        winit::WinitGraphicsBackend,
    },
    utils::{Physical, Rectangle, Transform},
};

use super::app_icon::{ensure_app_icon_resources_for_node_ids, ensure_node_app_icon_resources};
use super::cluster_icon::ensure_cluster_core_icon_resources;
use super::log_rounded_shader_failure;
use super::node::{
    ensure_node_circle_resources, node_app_icon_texture_allowed,
    node_markers_need_app_icon_resources,
};
use super::pin_icon::ensure_pin_icon_resources;
use super::screenshot_icon::ensure_screenshot_menu_icon_resources;
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::overlay::ensure_cluster_bloom_icon_resources;
use crate::render::bearings::ensure_bearing_icon_resources;
use crate::text::ensure_ui_text_resources;
use crate::window::prewarm_visible_active_window_offscreen_caches;
use draw::{draw_cursor_layer, draw_debug_frame_scene};
use scene::{collect_cursor_scene, collect_debug_frame_scene, prepare_debug_frame_state};

pub(crate) use draw::{draw_offscreen_textures, draw_window_borders};

const WINDOW_TEXTURE_SHADER: &str = include_str!("../shaders/window_rounded_texture.frag");
const WINDOW_SHADOW_SHADER: &str = include_str!("../shaders/window_shadow.frag");
const SURFACE_CLIP_SHADER: &str = include_str!("../shaders/surface_clipped_texture.frag");

pub(crate) fn ensure_window_texture_program(renderer: &mut GlesRenderer, st: &mut Halley) {
    if st.ui.render_state.gpu.window_texture_program.is_some()
        || st.ui.render_state.gpu.window_texture_program_failed
    {
        return;
    }

    match renderer.compile_custom_texture_shader(
        WINDOW_TEXTURE_SHADER,
        &[
            UniformName::new("rect_size", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_1f),
            UniformName::new("border_px", UniformType::_1f),
            UniformName::new("border_color", UniformType::_4f),
            UniformName::new("fill_color", UniformType::_4f),
            UniformName::new("content_alpha_scale", UniformType::_1f),
            UniformName::new("geo_offset", UniformType::_2f),
            UniformName::new("geo_size", UniformType::_2f),
        ],
    ) {
        Ok(program) => st.ui.render_state.gpu.window_texture_program = Some(program),
        Err(err) => {
            st.ui.render_state.gpu.window_texture_program_failed = true;
            log_rounded_shader_failure(
                "render/shaders/window_rounded_texture.frag",
                "window-content-clip",
                &err,
            );
        }
    }
}

fn ensure_window_shadow_program(renderer: &mut GlesRenderer, st: &mut Halley) {
    if st.ui.render_state.gpu.window_shadow_program.is_some()
        || st.ui.render_state.gpu.window_shadow_program_failed
    {
        return;
    }

    match renderer.compile_custom_texture_shader(
        WINDOW_SHADOW_SHADER,
        &[
            UniformName::new("rect_size", UniformType::_2f),
            UniformName::new("caster_size", UniformType::_2f),
            UniformName::new("caster_center", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_1f),
            UniformName::new("spread", UniformType::_1f),
            UniformName::new("shadow_radius", UniformType::_1f),
            UniformName::new("shadow_color", UniformType::_4f),
        ],
    ) {
        Ok(program) => st.ui.render_state.gpu.window_shadow_program = Some(program),
        Err(err) => {
            st.ui.render_state.gpu.window_shadow_program_failed = true;
            log_rounded_shader_failure("render/shaders/window_shadow.frag", "window-shadow", &err);
        }
    }
}

fn ensure_surface_clip_program(renderer: &mut GlesRenderer, st: &mut Halley) {
    if st.ui.render_state.gpu.surface_clip_program.is_some()
        || st.ui.render_state.gpu.surface_clip_program_failed
    {
        return;
    }

    match renderer.compile_custom_texture_shader(
        SURFACE_CLIP_SHADER,
        &[
            UniformName::new("clip_scale", UniformType::_1f),
            UniformName::new("geo_size", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_4f),
            UniformName::new("input_to_geo_row_0", UniformType::_3f),
            UniformName::new("input_to_geo_row_1", UniformType::_3f),
            UniformName::new("input_to_geo_row_2", UniformType::_3f),
        ],
    ) {
        Ok(program) => st.ui.render_state.gpu.surface_clip_program = Some(program),
        Err(err) => {
            st.ui.render_state.gpu.surface_clip_program_failed = true;
            log_rounded_shader_failure(
                "render/shaders/surface_clipped_texture.frag",
                "window-surface-clip",
                &err,
            );
        }
    }
}

pub(crate) fn draw_debug_frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
) -> Result<(), Box<dyn Error>> {
    let size = backend.window_size();
    let damage = Rectangle::<i32, Physical>::from_size(size);
    {
        let (renderer, mut framebuffer) = backend.bind()?;
        draw_debug_frame_to_target(
            renderer,
            &mut framebuffer,
            size,
            st,
            resize_preview,
            hover_node,
            preview_hover_node,
            None,
            None,
            Transform::Flipped180,
        )?;
    }
    backend.submit(Some(&[damage]))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_debug_frame_to_target(
    renderer: &mut GlesRenderer,
    framebuffer: &mut GlesTarget<'_>,
    size: smithay::utils::Size<i32, Physical>,
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
    frame_transform: Transform,
) -> Result<(), Box<dyn Error>> {
    ensure_node_circle_resources(renderer, st)?;
    ensure_window_texture_program(renderer, st);
    ensure_window_shadow_program(renderer, st);
    ensure_surface_clip_program(renderer, st);

    let prepared = prepare_debug_frame_state(st, size);
    prewarm_visible_active_window_offscreen_caches(renderer, st, prepared.now);

    let scene = collect_debug_frame_scene(
        renderer,
        st,
        size,
        resize_preview,
        hover_node,
        preview_hover_node,
        prepared.now,
    );
    ensure_node_app_icon_resources(renderer, st, &scene.render_nodes)?;
    if node_markers_need_app_icon_resources(st.runtime.tuning.node_show_app_icons) {
        ensure_cluster_core_icon_resources(renderer, st)?;
    }
    ensure_screenshot_menu_icon_resources(renderer, st)?;
    ensure_pin_icon_resources(renderer, st)?;
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    if st.runtime.tuning.tile_queue_show_icons
        && node_app_icon_texture_allowed(st.runtime.tuning.node_show_app_icons, false)
    {
        let overflow_ids = st
            .model
            .cluster_state
            .cluster_overflow_members
            .get(current_monitor.as_str())
            .into_iter()
            .flat_map(|ids| ids.iter().copied())
            .chain(
                st.input
                    .interaction_state
                    .cluster_overflow_drag_preview
                    .as_ref()
                    .filter(|preview| preview.monitor == current_monitor)
                    .map(|preview| preview.member_id),
            )
            .chain(
                st.model
                    .cluster_state
                    .cluster_overflow_promotion_anim
                    .get(current_monitor.as_str())
                    .map(|anim| anim.member_id),
            )
            .collect::<Vec<_>>();
        ensure_app_icon_resources_for_node_ids(renderer, st, overflow_ids.into_iter())?;
    }
    if node_app_icon_texture_allowed(st.runtime.tuning.node_show_app_icons, false)
        && let Some(candidate_ids) = st
            .input
            .interaction_state
            .focus_cycle_session
            .as_ref()
            .map(|session| session.candidates.clone())
    {
        ensure_app_icon_resources_for_node_ids(renderer, st, candidate_ids.into_iter())?;
    }
    ensure_cluster_bloom_icon_resources(renderer, st, current_monitor.as_str())?;
    ensure_bearing_icon_resources(renderer, st, current_monitor.as_str())?;
    ensure_ui_text_resources(renderer, st)?;
    let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);
    let mut frame = renderer.render(framebuffer, size, frame_transform)?;
    frame.clear(Color32F::new(0.04, 0.05, 0.06, 1.0), &[prepared.damage])?;

    draw_debug_frame_scene(&mut frame, st, size, &prepared, &scene, hover_node)?;
    draw_cursor_layer(
        &mut frame,
        prepared.damage,
        cursor_screen,
        &cursor,
        &st.runtime.tuning.cursor,
    )?;

    let _ = frame.finish()?;
    crate::compositor::workspace::state::process_pending_manual_collapses_for_monitor(
        st,
        current_monitor.as_str(),
        prepared.now,
    );
    Ok(())
}
