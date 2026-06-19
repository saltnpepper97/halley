use std::error::Error;

pub(crate) mod draw;
mod scene;

use smithay::{
    backend::{
        renderer::{
            Color32F, Frame, Renderer,
            gles::{GlesRenderer, GlesTarget, UniformName, UniformType},
        },
        winit::WinitGraphicsBackend,
    },
    utils::{Physical, Rectangle, Size, Transform},
};

use super::app_icon::{ensure_app_icon_resources_for_node_ids, ensure_node_app_icon_resources};
use super::cluster_icon::ensure_cluster_core_icon_resources;
use super::layer_shell::collect_layer_surfaces;
use super::log_rounded_shader_failure;
use super::node::{
    ensure_node_circle_resources, node_app_icon_texture_allowed,
    node_markers_need_app_icon_resources,
};
use super::pin_icon::ensure_pin_icon_resources;
use super::screenshot_icon::ensure_screenshot_menu_icon_resources;
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::overlay::{
    ensure_cluster_bloom_icon_resources, prime_cluster_naming_dialog_text_resources,
};
use crate::render::bearings::ensure_bearing_icon_resources;
use crate::text::ensure_ui_text_resources;
use crate::window::{
    prewarm_apogee_previews, prewarm_focus_cycle_previews,
    prewarm_visible_active_window_offscreen_caches,
};
use draw::{
    FrameBlurContext, draw_apogee_background_layers, draw_cursor_layer, draw_debug_frame_scene,
    draw_scene_below_windows, draw_scene_windows_and_hud,
};
use scene::{collect_cursor_scene, collect_debug_frame_scene, prepare_debug_frame_state};

fn max_layer_mask_size(scene: &scene::SceneCollections) -> Option<Size<i32, Physical>> {
    [
        &scene.layer_background_elements,
        &scene.layer_bottom_elements,
        &scene.layer_top_elements,
        &scene.layer_overlay_elements,
    ]
    .into_iter()
    .flat_map(|groups| groups.iter())
    .filter(|group| group.blur && group.dst.size.w > 0 && group.dst.size.h > 0)
    .fold(None, |acc: Option<Size<i32, Physical>>, group| {
        Some(match acc {
            Some(size) => Size::from((size.w.max(group.dst.size.w), size.h.max(group.dst.size.h))),
            None => group.dst.size,
        })
    })
}

pub(crate) use draw::{draw_offscreen_textures, draw_window_borders};

const WINDOW_TEXTURE_SHADER: &str = include_str!("../shaders/window_rounded_texture.frag");
const WINDOW_SHADOW_SHADER: &str = include_str!("../shaders/window_shadow.frag");
const SURFACE_CLIP_SHADER: &str = include_str!("../shaders/surface_clipped_texture.frag");
const BLUR_DOWN_SHADER: &str = include_str!("../shaders/blur_down.frag");
const BLUR_UP_SHADER: &str = include_str!("../shaders/blur_up.frag");
const BLUR_COMPOSITE_SHADER: &str = include_str!("../shaders/blur_composite.frag");
const BLUR_COMPOSITE_MASKED_SHADER: &str = include_str!("../shaders/blur_composite_masked.frag");

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
            UniformName::new("src_uv_offset", UniformType::_2f),
            UniformName::new("src_uv_scale", UniformType::_2f),
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

pub(crate) fn ensure_blur_programs(renderer: &mut GlesRenderer, st: &mut Halley) {
    let gpu = &st.ui.render_state.gpu;
    if gpu.blur_programs_failed
        || (gpu.blur_down_program.is_some()
            && gpu.blur_up_program.is_some()
            && gpu.blur_composite_program.is_some()
            && gpu.blur_composite_masked_program.is_some())
    {
        return;
    }

    let kawase_uniforms = [
        UniformName::new("halfpixel", UniformType::_2f),
        UniformName::new("offset", UniformType::_1f),
    ];
    let down = renderer.compile_custom_texture_shader(BLUR_DOWN_SHADER, &kawase_uniforms);
    let up = renderer.compile_custom_texture_shader(BLUR_UP_SHADER, &kawase_uniforms);
    let composite = renderer.compile_custom_texture_shader(
        BLUR_COMPOSITE_SHADER,
        &[
            UniformName::new("rect_size", UniformType::_2f),
            UniformName::new("patch_origin_uv", UniformType::_2f),
            UniformName::new("patch_size_uv", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_1f),
            UniformName::new("saturation", UniformType::_1f),
            UniformName::new("noise", UniformType::_1f),
        ],
    );
    let masked_composite = renderer.compile_custom_texture_shader(
        BLUR_COMPOSITE_MASKED_SHADER,
        &[
            UniformName::new("mask_tex", UniformType::_1i),
            UniformName::new("patch_origin_uv", UniformType::_2f),
            UniformName::new("patch_size_uv", UniformType::_2f),
            UniformName::new("mask_uv_scale", UniformType::_2f),
            UniformName::new("saturation", UniformType::_1f),
            UniformName::new("noise", UniformType::_1f),
        ],
    );

    match (down, up, composite, masked_composite) {
        (Ok(down), Ok(up), Ok(composite), Ok(masked_composite)) => {
            st.ui.render_state.gpu.blur_down_program = Some(down);
            st.ui.render_state.gpu.blur_up_program = Some(up);
            st.ui.render_state.gpu.blur_composite_program = Some(composite);
            st.ui.render_state.gpu.blur_composite_masked_program = Some(masked_composite);
        }
        (down, up, composite, masked_composite) => {
            st.ui.render_state.gpu.blur_programs_failed = true;
            for (shader, role, result) in [
                ("render/shaders/blur_down.frag", "blur-down", down.err()),
                ("render/shaders/blur_up.frag", "blur-up", up.err()),
                (
                    "render/shaders/blur_composite.frag",
                    "blur-composite",
                    composite.err(),
                ),
                (
                    "render/shaders/blur_composite_masked.frag",
                    "blur-composite-masked",
                    masked_composite.err(),
                ),
            ] {
                if let Some(err) = result {
                    log_rounded_shader_failure(shader, role, &err);
                }
            }
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
    let prepared = prepare_debug_frame_state(st, size);

    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let scene = collect_debug_frame_scene(
            renderer,
            st,
            size,
            resize_preview,
            hover_node,
            preview_hover_node,
            prepared.now,
        );
        let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);
        let mut frame = renderer.render(framebuffer, size, frame_transform)?;
        frame.clear(Color32F::new(0.04, 0.05, 0.06, 1.0), &[prepared.damage])?;
        draw_debug_frame_scene(&mut frame, st, size, &prepared, &scene, hover_node)?;
        let cursor_config = st.runtime.tuning.cursor.clone();
        draw_cursor_layer(
            &mut frame,
            prepared.damage,
            cursor_screen,
            &cursor,
            &mut st.platform.cursor_manager,
            &cursor_config,
        )?;
        let _ = frame.finish()?;
        return Ok(());
    }

    ensure_node_circle_resources(renderer, st)?;
    ensure_window_texture_program(renderer, st);
    ensure_window_shadow_program(renderer, st);
    ensure_surface_clip_program(renderer, st);
    if st.runtime.tuning.effects.blur.enabled {
        ensure_blur_programs(renderer, st);
    }

    let apogee_fast_path = st
        .input
        .interaction_state
        .apogee_session
        .as_ref()
        .is_some_and(|session| {
            session
                .monitor_session(st.model.monitor_state.current_monitor.as_str())
                .is_some()
        });
    if apogee_fast_path {
        crate::render::app_icon::drain_app_icon_jobs(renderer, st);
        ensure_cluster_core_icon_resources(renderer, st)?;
        ensure_ui_text_resources(renderer, st)?;
        prewarm_apogee_previews(renderer, st, prepared.now);
        let (layer_background, _, _, _) = collect_layer_surfaces(renderer, st, size, prepared.now);
        let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);
        let mut frame = renderer.render(framebuffer, size, frame_transform)?;
        frame.clear(Color32F::new(0.04, 0.05, 0.06, 1.0), &[prepared.damage])?;
        draw_apogee_background_layers(&mut frame, prepared.damage, &layer_background)?;
        crate::overlay::draw_observatory(
            &mut frame,
            st,
            size.w,
            size.h,
            prepared.damage,
            prepared.now,
        )?;
        let cursor_config = st.runtime.tuning.cursor.clone();
        draw_cursor_layer(
            &mut frame,
            prepared.damage,
            cursor_screen,
            &cursor,
            &mut st.platform.cursor_manager,
            &cursor_config,
        )?;
        let _ = frame.finish()?;
        return Ok(());
    }

    let frame_perf_start = crate::perf::start();
    let prewarm_start = crate::perf::start();
    if st.input.interaction_state.apogee_session.is_none() {
        prewarm_visible_active_window_offscreen_caches(renderer, st, prepared.now);
    }
    let prewarm_ms = prewarm_start.map(crate::perf::elapsed_ms);

    let collect_start = crate::perf::start();
    let scene = collect_debug_frame_scene(
        renderer,
        st,
        size,
        resize_preview,
        hover_node,
        preview_hover_node,
        prepared.now,
    );
    let collect_ms = collect_start.map(crate::perf::elapsed_ms);
    let resources_start = crate::perf::start();
    crate::render::app_icon::drain_app_icon_jobs(renderer, st);
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
        && let Some(candidate_ids) =
            st.input
                .interaction_state
                .focus_cycle_session
                .as_ref()
                .map(|session| {
                    session
                        .visible_slots(crate::overlay::FOCUS_CYCLE_VISIBLE_RADIUS)
                        .into_iter()
                        .map(|(_, node_id)| node_id)
                        .collect::<Vec<_>>()
                })
    {
        ensure_app_icon_resources_for_node_ids(renderer, st, candidate_ids.into_iter())?;
    }
    // Capture live/still window textures for the alt+tab switcher cards (no-op
    // unless a focus-cycle session is active).
    prewarm_focus_cycle_previews(renderer, st, prepared.now);
    prewarm_apogee_previews(renderer, st, prepared.now);
    ensure_cluster_bloom_icon_resources(renderer, st, current_monitor.as_str())?;
    ensure_bearing_icon_resources(renderer, st, current_monitor.as_str())?;
    prime_cluster_naming_dialog_text_resources(st, size.w, size.h);
    ensure_ui_text_resources(renderer, st)?;
    let resources_ms = resources_start.map(crate::perf::elapsed_ms);
    let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);
    let draw_start = crate::perf::start();
    let clear_color = Color32F::new(0.04, 0.05, 0.06, 1.0);

    // Backdrop blur is active only when enabled, shaders are compiled, and at
    // least one window, layer-shell surface, or compositor overlay can use it.
    // When inactive we keep the direct-to-framebuffer path with zero extra work.
    let blur_cfg = st.runtime.tuning.effects.blur;
    let blur_ready = blur_cfg.enabled
        && !st.ui.render_state.gpu.blur_programs_failed
        && st.ui.render_state.gpu.blur_down_program.is_some()
        && st.ui.render_state.gpu.blur_up_program.is_some()
        && st.ui.render_state.gpu.blur_composite_program.is_some()
        && st
            .ui
            .render_state
            .gpu
            .blur_composite_masked_program
            .is_some();
    let has_blur_window = scene
        .blur_rects
        .iter()
        .any(|rect| rect.dst.size.w > 0 && rect.dst.size.h > 0 && rect.corner_radius >= 0.0);
    let overlay_blur_enabled = halley_config::overlay_blur_enabled(
        &st.runtime.tuning.effects.blur,
        &st.runtime.tuning.overlay_style,
    );
    let layer_mask_size = max_layer_mask_size(&scene);
    let has_blur_content = has_blur_window || overlay_blur_enabled || layer_mask_size.is_some();

    let down = st.ui.render_state.gpu.blur_down_program.clone();
    let up = st.ui.render_state.gpu.blur_up_program.clone();
    let composite = st.ui.render_state.gpu.blur_composite_program.clone();
    let masked_composite = st.ui.render_state.gpu.blur_composite_masked_program.clone();
    let mut blur_textures = None;
    let mut layer_mask_texture = None;
    if blur_ready && has_blur_content {
        match crate::render::blur::ensure_blur_textures(
            renderer,
            &mut st.ui.render_state.gpu.blur_textures,
            size,
            blur_cfg.passes,
        ) {
            Ok(()) => blur_textures = st.ui.render_state.gpu.blur_textures.take(),
            Err(err) => {
                eventline::warn!("blur disabled this frame: texture allocation failed: {err}")
            }
        }
        if let Some(mask_size) = layer_mask_size {
            match crate::render::blur::ensure_scratch_texture(
                renderer,
                &mut st.ui.render_state.gpu.layer_mask_texture,
                mask_size,
            ) {
                Ok(()) => layer_mask_texture = st.ui.render_state.gpu.layer_mask_texture.take(),
                Err(err) => eventline::warn!(
                    "layer-shell blur disabled this frame: mask texture allocation failed: {err}"
                ),
            }
        }
    }

    let mut frame = renderer.render(framebuffer, size, frame_transform)?;
    frame.clear(clear_color, &[prepared.damage])?;

    if let (Some(down), Some(up), Some(composite), Some(masked_composite), Some(textures)) = (
        down.as_ref(),
        up.as_ref(),
        composite.as_ref(),
        masked_composite.as_ref(),
        blur_textures.as_mut(),
    ) {
        let mut blur_ctx = FrameBlurContext {
            textures,
            down_program: down,
            up_program: up,
            composite_program: composite,
            masked_composite_program: masked_composite,
            offset: crate::render::blur::blur_offset(blur_cfg.radius),
            saturation: blur_cfg.saturation,
            noise: blur_cfg.noise,
            layer_mask_texture: layer_mask_texture.as_mut(),
        };
        draw_scene_below_windows(
            &mut frame,
            st,
            size,
            &prepared,
            &scene,
            hover_node,
            Some(&mut blur_ctx),
        )?;
        draw_scene_windows_and_hud(
            &mut frame,
            st,
            size,
            &prepared,
            &scene,
            hover_node,
            Some(&mut blur_ctx),
        )?;
    } else {
        draw_scene_below_windows(&mut frame, st, size, &prepared, &scene, hover_node, None)?;
        draw_scene_windows_and_hud(&mut frame, st, size, &prepared, &scene, hover_node, None)?;
    }
    let cursor_config = st.runtime.tuning.cursor.clone();
    draw_cursor_layer(
        &mut frame,
        prepared.damage,
        cursor_screen,
        &cursor,
        &mut st.platform.cursor_manager,
        &cursor_config,
    )?;

    let _ = frame.finish()?;
    if let Some(blur_textures) = blur_textures {
        st.ui.render_state.gpu.blur_textures = Some(blur_textures);
    }
    if let Some(layer_mask_texture) = layer_mask_texture {
        st.ui.render_state.gpu.layer_mask_texture = Some(layer_mask_texture);
    }
    let draw_ms = draw_start.map(crate::perf::elapsed_ms);
    if let Some(start) = frame_perf_start {
        // Refresh-aware budget: a fixed 24ms threshold never fires on a 180Hz
        // output (~5.6ms/frame), so the choppy cluster-open slide was invisible.
        // Derive the budget from the current monitor's refresh rate.
        let budget_ms = st
            .runtime
            .tuning
            .tty_viewports
            .iter()
            .find(|vp| vp.enabled && vp.connector == current_monitor)
            .and_then(|vp| vp.refresh_rate)
            .map(|hz| (1000.0 / hz as f32 * 1.5).max(4.0))
            .unwrap_or(16.0);
        // During a cluster tile slide, log every frame regardless of budget so we
        // capture the whole tween, not just the worst spike.
        let tile_anim = crate::animation::cluster_tile_tracks_animating(
            st.ui.render_state.cluster_tile_tracks(),
            prepared.now,
        );
        let total_ms = crate::perf::elapsed_ms(start);
        if total_ms > budget_ms || tile_anim {
            eventline::warn!(
                "perf frame took={:.2}ms budget={:.2} tile_anim={} (prewarm={:.2} collect={:.2} resources={:.2} draw={:.2}) toplevels={} render_nodes={}",
                total_ms,
                budget_ms,
                tile_anim,
                prewarm_ms.unwrap_or_default(),
                collect_ms.unwrap_or_default(),
                resources_ms.unwrap_or_default(),
                draw_ms.unwrap_or_default(),
                st.platform.xdg_shell_state.toplevel_surfaces().len(),
                scene.render_nodes.len(),
            );
        }
    }
    crate::compositor::workspace::state::process_pending_collapses_for_monitor(
        st,
        current_monitor.as_str(),
        prepared.now,
    );
    Ok(())
}
