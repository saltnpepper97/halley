use std::error::Error;
use std::time::Instant;

use halley_core::field::Vec2;
use halley_core::viewport::FocusRing;
use smithay::wayland::compositor::{SurfaceAttributes, with_states};
use smithay::{
    backend::renderer::{
        Color32F, Frame, Renderer, Texture,
        element::Kind,
        element::surface::render_elements_from_surface_tree,
        gles::{
            GlesFrame, GlesRenderer, GlesTarget, GlesTexProgram, Uniform, UniformName, UniformType,
        },
        utils::draw_render_elements,
    },
    backend::winit::WinitGraphicsBackend,
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use super::app_icon::{ensure_app_icon_resources_for_node_ids, ensure_node_app_icon_resources};
use super::bearings::BearingChipLayout;
use super::bearings::{collect_bearing_layouts, draw_bearings, ensure_bearing_icon_resources};
use super::cluster_icon::ensure_cluster_core_icon_resources;
use super::cursor::{cursor_surface_hotspot, draw_cursor_sprite};
use super::cursor_theme::themed_cursor_sprite_with_fallback;
use super::layer_shell::collect_layer_surfaces;
use super::log_rounded_shader_failure;
use super::node::{
    NodeSnapshot, collect_hover_preview, draw_closing_node_markers, draw_node_hover_labels,
    draw_node_markers, ensure_node_circle_resources, node_app_icon_texture_allowed,
    node_markers_need_app_icon_resources,
};
use super::screenshot_icon::ensure_screenshot_menu_icon_resources;
use super::state::{ClosingWindowAnimationKind, ClosingWindowAnimationSnapshot};
use super::text::ensure_ui_text_resources;
use super::utils::{draw_outline_rect, draw_rect, draw_ring, world_to_screen};
use super::window::{
    ActiveBorderRect, CroppedClippedSurfaceElement, OffscreenNodeTexture, StackWindowDrawUnit,
    collect_active_surfaces, prewarm_visible_active_window_offscreen_caches,
};
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::overlay::{
    OverlayView, draw_cluster_bloom, draw_cluster_overflow_promotion, draw_cluster_overflow_strip,
    draw_cluster_selection_markers, draw_monitor_hud, draw_overlay_hover_label,
    ensure_cluster_bloom_icon_resources,
};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;

const WINDOW_TEXTURE_SHADER: &str = include_str!("shaders/window_rounded_texture.frag");
const SURFACE_CLIP_SHADER: &str = include_str!("shaders/surface_clipped_texture.frag");

fn focus_ring_screen_radii(
    view_size: Vec2,
    output_size: Size<i32, Physical>,
    focus_ring: FocusRing,
) -> (f32, f32) {
    let px_per_world_x = output_size.w as f32 / view_size.x.max(1.0);
    let px_per_world_y = output_size.h as f32 / view_size.y.max(1.0);
    (
        focus_ring.radius_x * px_per_world_x,
        focus_ring.radius_y * px_per_world_y,
    )
}

fn ensure_window_texture_program(renderer: &mut GlesRenderer, st: &mut Halley) {
    if st.ui.render_state.window_texture_program.is_some()
        || st.ui.render_state.window_texture_program_failed
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
        Ok(program) => st.ui.render_state.window_texture_program = Some(program),
        Err(err) => {
            st.ui.render_state.window_texture_program_failed = true;
            log_rounded_shader_failure(
                "render/shaders/window_rounded_texture.frag",
                "window-content-clip",
                &err,
            );
        }
    }
}

fn ensure_surface_clip_program(renderer: &mut GlesRenderer, st: &mut Halley) {
    if st.ui.render_state.surface_clip_program.is_some()
        || st.ui.render_state.surface_clip_program_failed
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
        Ok(program) => st.ui.render_state.surface_clip_program = Some(program),
        Err(err) => {
            st.ui.render_state.surface_clip_program_failed = true;
            log_rounded_shader_failure(
                "render/shaders/surface_clipped_texture.frag",
                "window-surface-clip",
                &err,
            );
        }
    }
}

struct PreparedFrameState {
    damage: Rectangle<i32, Physical>,
    now: Instant,
}

struct SceneCollections {
    session_lock_elements: Vec<SurfaceElement>,
    layer_background_elements: Vec<SurfaceElement>,
    layer_bottom_elements: Vec<SurfaceElement>,
    layer_top_elements: Vec<SurfaceElement>,
    layer_overlay_elements: Vec<SurfaceElement>,
    active_elements: Vec<CroppedClippedSurfaceElement>,
    resized_active_elements: Vec<CroppedClippedSurfaceElement>,
    fullscreen_active_elements: Vec<CroppedClippedSurfaceElement>,
    offscreen_textures: Vec<OffscreenNodeTexture>,
    resized_offscreen_textures: Vec<OffscreenNodeTexture>,
    fullscreen_offscreen_textures: Vec<OffscreenNodeTexture>,
    popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    popup_elements:
        Vec<smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>>,
    fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    fullscreen_popup_elements:
        Vec<smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>>,
    stack_window_units: Vec<StackWindowDrawUnit>,
    border_rects: Vec<ActiveBorderRect>,
    resized_border_rects: Vec<ActiveBorderRect>,
    closing_window_animations: Vec<ClosingWindowAnimationSnapshot>,
    overlap_overlay_rects: Vec<(i32, i32, i32, i32)>,
    hover_preview_rect: Option<(i32, i32, i32, i32)>,
    hover_preview_elements: Vec<SurfaceElement>,
    render_nodes: Vec<NodeSnapshot>,
    bearing_layouts: Vec<BearingChipLayout>,
}

struct CursorScene {
    cursor_status: smithay::input::pointer::CursorImageStatus,
    cursor_surface_elements: Vec<
        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
    >,
}

fn draw_clamped_outline_rect<F: smithay::backend::renderer::Frame>(
    frame: &mut F,
    rect: (i32, i32, i32, i32),
    line_width: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
    framebuffer_size: smithay::utils::Size<i32, Physical>,
) -> Result<(), F::Error> {
    let lw = line_width.max(1);
    let w = rect.2.max(1);
    let h = rect.3.max(1);
    let fb = Rectangle::<i32, Physical>::from_size(framebuffer_size);

    let mut draw_intersection = |x: i32, y: i32, w: i32, h: i32| -> Result<(), F::Error> {
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let edge = Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
        if let Some(visible) = edge.intersection(fb) {
            draw_rect(
                frame,
                visible.loc.x,
                visible.loc.y,
                visible.size.w,
                visible.size.h,
                color,
                damage,
            )?;
        }
        Ok(())
    };

    draw_intersection(rect.0, rect.1, w, lw)?;
    draw_intersection(rect.0, rect.1 + h - lw, w, lw)?;
    draw_intersection(rect.0, rect.1, lw, h)?;
    draw_intersection(rect.0 + w - lw, rect.1, lw, h)
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
    ensure_surface_clip_program(renderer, st);

    let prepared = prepare_debug_frame_state(st, size);
    prewarm_visible_active_window_offscreen_caches(renderer, st, prepared.now);

    if node_markers_need_app_icon_resources(st.runtime.tuning.node_show_app_icons) {
        let mut prewarm_icons = Vec::new();
        for (node_id, app_id) in &st.model.node_app_ids {
            if !st.ui.render_state.node_app_icon_cache.contains_key(app_id) {
                prewarm_icons.push(*node_id);
            }
        }
        if !prewarm_icons.is_empty() {
            let _ = ensure_app_icon_resources_for_node_ids(renderer, st, prewarm_icons.into_iter());
        }
    }

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

fn prepare_debug_frame_state(
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
) -> PreparedFrameState {
    let now = Instant::now();
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        crate::protocol::wayland::session_lock::configure_surfaces(st);
    }
    if !st.input.interaction_state.suppress_layer_shell_configure {
        crate::compositor::monitor::layer_shell::configure_layer_shell_surfaces(
            st,
            (size.w, size.h).into(),
        );
    }

    PreparedFrameState {
        damage: Rectangle::<i32, Physical>::from_size(size),
        now,
    }
}

fn collect_debug_frame_scene(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    now: Instant,
) -> SceneCollections {
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let session_lock_elements =
            crate::protocol::wayland::session_lock::current_monitor_surfaces(st)
                .into_iter()
                .flat_map(|surface| {
                    render_elements_from_surface_tree(
                        renderer,
                        &surface,
                        (0, 0),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                })
                .collect();
        return SceneCollections {
            session_lock_elements,
            layer_background_elements: Vec::new(),
            layer_bottom_elements: Vec::new(),
            layer_top_elements: Vec::new(),
            layer_overlay_elements: Vec::new(),
            active_elements: Vec::new(),
            resized_active_elements: Vec::new(),
            fullscreen_active_elements: Vec::new(),
            offscreen_textures: Vec::new(),
            resized_offscreen_textures: Vec::new(),
            fullscreen_offscreen_textures: Vec::new(),
            popup_offscreen_textures: Vec::new(),
            popup_elements: Vec::new(),
            fullscreen_popup_offscreen_textures: Vec::new(),
            fullscreen_popup_elements: Vec::new(),
            stack_window_units: Vec::new(),
            border_rects: Vec::new(),
            resized_border_rects: Vec::new(),
            closing_window_animations: Vec::new(),
            overlap_overlay_rects: Vec::new(),
            hover_preview_rect: None,
            hover_preview_elements: Vec::new(),
            render_nodes: Vec::new(),
            bearing_layouts: Vec::new(),
        };
    }

    let render_monitor = st.model.monitor_state.current_monitor.clone();
    let bearings_mix = st
        .ui
        .render_state
        .bearings_mix_for_monitor(render_monitor.as_str());
    let (
        layer_background_elements,
        layer_bottom_elements,
        layer_top_elements,
        layer_overlay_elements,
    ) = collect_layer_surfaces(renderer, st, size, now);

    let (
        active_elements,
        resized_active_elements,
        fullscreen_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        fullscreen_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        fullscreen_popup_offscreen_textures,
        fullscreen_popup_elements,
        node_surface_map,
        stack_window_units,
        border_rects,
        resized_border_rects,
        overlap_overlay_rects,
    ) = collect_active_surfaces(renderer, st, size, resize_preview, now);
    let closing_window_animations = if st.runtime.tuning.window_close_animation_enabled() {
        st.ui
            .render_state
            .closing_window_animation_snapshots(render_monitor.as_str(), now)
    } else {
        Vec::new()
    };

    let hovered_preview_id = preview_hover_node.and_then(|id| {
        st.model.field.node(id).and_then(|n| {
            matches!(
                n.state,
                halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
            )
            .then_some(id)
        })
    });
    let overlay_hover_preview = st
        .input
        .interaction_state
        .overlay_hover_target
        .as_ref()
        .filter(|target| {
            target.monitor == render_monitor
                && preview_hover_node == Some(target.node_id)
                && st
                    .input
                    .interaction_state
                    .cluster_overflow_drag_preview
                    .is_none()
        })
        .map(|target| (target.node_id, target.screen_anchor, target.prefer_left));
    let (hover_preview_rect, hover_preview_elements) = collect_hover_preview(
        renderer,
        st,
        size,
        render_monitor.as_str(),
        &node_surface_map,
        hovered_preview_id,
        overlay_hover_preview,
        hover_node,
        now,
    );

    let render_nodes = st
        .model
        .field
        .nodes()
        .keys()
        .copied()
        .into_iter()
        .filter_map(|id| {
            let node = st.model.field.node(id)?;
            if !st.model.field.participates_in_field_view(id)
                || !st.model.field.is_visible(id)
                || !st.node_visible_on_current_monitor(id)
            {
                return None;
            }
            Some(NodeSnapshot {
                id,
                state: node.state.clone(),
                pos: node.pos,
                intrinsic_size: node.intrinsic_size,
                label: node.label.clone(),
            })
        })
        .collect();
    let bearing_layouts =
        collect_bearing_layouts(st, size.w, size.h, render_monitor.as_str(), bearings_mix);

    SceneCollections {
        session_lock_elements: Vec::new(),
        layer_background_elements,
        layer_bottom_elements,
        layer_top_elements,
        layer_overlay_elements,
        active_elements,
        resized_active_elements,
        fullscreen_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        fullscreen_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        fullscreen_popup_offscreen_textures,
        fullscreen_popup_elements,
        stack_window_units,
        border_rects,
        resized_border_rects,
        closing_window_animations,
        overlap_overlay_rects,
        hover_preview_rect,
        hover_preview_elements,
        render_nodes,
        bearing_layouts,
    }
}

fn collect_cursor_scene(
    renderer: &mut GlesRenderer,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> CursorScene {
    let cursor_status = cursor_image
        .cloned()
        .unwrap_or_else(smithay::input::pointer::CursorImageStatus::default_named);

    let mut cursor_surface_elements = Vec::new();
    if let (Some((sx, sy)), smithay::input::pointer::CursorImageStatus::Surface(surface)) =
        (cursor_screen, cursor_status.clone())
    {
        let scale = with_states(&surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_scale as f64
        });
        let (hotspot_x, hotspot_y) = cursor_surface_hotspot(&surface);
        let loc = (sx.round() as i32 - hotspot_x, sy.round() as i32 - hotspot_y);
        cursor_surface_elements = render_elements_from_surface_tree(
            renderer,
            &surface,
            loc,
            scale,
            1.0,
            Kind::Unspecified,
        );
    }

    CursorScene {
        cursor_status,
        cursor_surface_elements,
    }
}

fn draw_debug_frame_scene(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    prepared: &PreparedFrameState,
    scene: &SceneCollections,
    hover_node: Option<halley_core::field::NodeId>,
) -> Result<(), Box<dyn Error>> {
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        if !scene.session_lock_elements.is_empty() {
            let _ =
                draw_render_elements(frame, 1.0, &scene.session_lock_elements, &[prepared.damage]);
        }
        return Ok(());
    }

    if !scene.layer_background_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_background_elements,
            &[prepared.damage],
        );
    }

    if !scene.layer_bottom_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_bottom_elements, &[prepared.damage]);
    }

    draw_node_markers(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    draw_window_borders(frame, size, prepared.damage, &scene.border_rects, st)?;

    if !scene.active_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.active_elements, &[prepared.damage]);
    }

    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.offscreen_textures,
        st.ui.render_state.window_texture_program.as_ref(),
    )?;
    draw_stack_window_units(frame, size, prepared.damage, &scene.stack_window_units, st)?;
    draw_overlap_overlays(frame, prepared.damage, &scene.overlap_overlay_rects)?;
    if !scene.resized_active_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.resized_active_elements,
            &[prepared.damage],
        );
    }

    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.resized_offscreen_textures,
        st.ui.render_state.window_texture_program.as_ref(),
    )?;
    draw_window_borders(
        frame,
        size,
        prepared.damage,
        &scene.resized_border_rects,
        st,
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.popup_offscreen_textures,
        st.ui.render_state.window_texture_program.as_ref(),
    )?;

    if !scene.popup_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.popup_elements, &[prepared.damage]);
    }

    draw_closing_window_animations(
        frame,
        size,
        prepared.damage,
        &scene.closing_window_animations,
        st,
    )?;

    draw_geometry_overlays(frame, st, size, prepared.damage, scene)?;

    if !scene.bearing_layouts.is_empty() {
        draw_bearings(frame, st, prepared.damage, &scene.bearing_layouts)?;
    }
    let bloom_monitor = st.model.monitor_state.current_monitor.clone();
    draw_cluster_bloom(
        frame,
        st,
        size.w,
        size.h,
        bloom_monitor.as_str(),
        prepared.damage,
    )?;
    let overlay = OverlayView::from_halley(st);
    draw_cluster_overflow_strip(
        frame,
        &overlay,
        bloom_monitor.as_str(),
        prepared.damage,
        st.now_ms(prepared.now),
    )?;
    draw_cluster_overflow_promotion(
        frame,
        &overlay,
        bloom_monitor.as_str(),
        prepared.damage,
        st.now_ms(prepared.now),
    )?;
    drop(overlay);
    draw_overlay_hover_label(frame, st, size.w, size.h, prepared.damage)?;

    if st.cluster_mode_active() {
        let overlay = OverlayView::from_halley(st);
        draw_cluster_selection_markers(frame, &overlay, size.w, size.h, prepared.damage)?;
    }

    draw_hover_preview(frame, prepared.damage, scene)?;
    draw_node_hover_labels(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    if !scene.layer_top_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_top_elements, &[prepared.damage]);
    }

    if !scene.fullscreen_active_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.fullscreen_active_elements,
            &[prepared.damage],
        );
    }
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.fullscreen_offscreen_textures,
        st.ui.render_state.window_texture_program.as_ref(),
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.fullscreen_popup_offscreen_textures,
        st.ui.render_state.window_texture_program.as_ref(),
    )?;
    if !scene.fullscreen_popup_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.fullscreen_popup_elements,
            &[prepared.damage],
        );
    }

    if !scene.layer_overlay_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_overlay_elements,
            &[prepared.damage],
        );
    }

    if st.should_draw_focus_ring_preview(prepared.now) {
        let focus_ring = st.active_focus_ring();
        let ring_world_cx = st.model.viewport.center.x + focus_ring.offset_x;
        let ring_world_cy = st.model.viewport.center.y + focus_ring.offset_y;
        let (ring_sx, ring_sy) = world_to_screen(st, size.w, size.h, ring_world_cx, ring_world_cy);
        let (screen_rx, screen_ry) =
            focus_ring_screen_radii(camera_controller(&*st).view_size(), size, focus_ring);
        draw_ring(
            frame,
            ring_sx as f32,
            ring_sy as f32,
            screen_rx,
            screen_ry,
            Color32F::new(0.15, 0.85, 0.85, 0.9),
            prepared.damage,
        )?;
    }

    draw_monitor_hud(frame, st, size.w, size.h, prepared.damage, prepared.now)?;
    Ok(())
}

fn draw_offscreen_textures(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    offscreen_textures: &[OffscreenNodeTexture],
    window_texture_program: Option<&GlesTexProgram>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for tex in offscreen_textures {
        let tex_size = tex.texture.size();
        let max_src_w = (tex_size.w as f64 - tex.src_x).max(1.0);
        let max_src_h = (tex_size.h as f64 - tex.src_y).max(1.0);

        let src = Rectangle::<f64, Buffer>::new(
            (tex.src_x, tex.src_y).into(),
            (
                tex.src_w.min(max_src_w).max(1.0),
                tex.src_h.min(max_src_h).max(1.0),
            )
                .into(),
        );

        let dst = Rectangle::<i32, Physical>::new(
            (tex.dst_x, tex.dst_y).into(),
            (tex.dst_w.max(1), tex.dst_h.max(1)).into(),
        );

        let visible = Rectangle::<i32, Physical>::new(
            (tex.clip_x, tex.clip_y).into(),
            (tex.clip_w.max(1), tex.clip_h.max(1)).into(),
        )
        .intersection(damage)
        .unwrap_or_else(|| Rectangle::<i32, Physical>::new((0, 0).into(), (0, 0).into()));
        if visible.size.w <= 0 || visible.size.h <= 0 {
            continue;
        }
        let local_damage = Rectangle::<i32, Physical>::new(
            (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
            visible.size,
        );
        let uniforms = [
            Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
            Uniform::new("corner_radius", tex.corner_radius.max(0.0)),
            Uniform::new("border_px", 0.0f32),
            Uniform::new("border_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
            Uniform::new("fill_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
            Uniform::new("content_alpha_scale", 1.0f32),
            Uniform::new("geo_offset", (tex.geo_offset_x, tex.geo_offset_y)),
            Uniform::new("geo_size", (tex.geo_w, tex.geo_h)),
        ];

        frame.render_texture_from_to(
            &tex.texture,
            src,
            dst,
            &[local_damage],
            &[],
            Transform::Normal,
            tex.alpha,
            window_texture_program,
            if window_texture_program.is_some() {
                &uniforms
            } else {
                &[]
            },
        )?;
    }

    Ok(())
}

fn transform_rect_about_center(
    _x: i32,
    _y: i32,
    w: i32,
    h: i32,
    center: (f32, f32),
    scale: f32,
) -> (i32, i32, i32, i32) {
    let new_w = (w as f32 * scale).round().max(1.0) as i32;
    let new_h = (h as f32 * scale).round().max(1.0) as i32;
    (
        (center.0 - new_w as f32 * 0.5).round() as i32,
        (center.1 - new_h as f32 * 0.5).round() as i32,
        new_w,
        new_h,
    )
}

fn draw_closing_window_animations(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    animations: &[ClosingWindowAnimationSnapshot],
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    draw_closing_node_markers(frame, st, size, animations, damage)?;

    for animation in animations {
        let ClosingWindowAnimationKind::Window {
            style,
            border_rects,
            offscreen_textures,
        } = &animation.kind
        else {
            continue;
        };

        let scale = match style {
            halley_config::WindowCloseAnimationStyle::Shrink => {
                (1.0 - crate::animation::ease_in_out_cubic(animation.progress)).clamp(0.0, 1.0)
            }
        };
        if scale <= 0.001 {
            continue;
        }

        if !border_rects.is_empty() {
            let scaled_border_rects = border_rects
                .iter()
                .map(|border_rect| {
                    let center = (
                        border_rect.x as f32 + border_rect.w as f32 * 0.5,
                        border_rect.y as f32 + border_rect.h as f32 * 0.5,
                    );
                    let (x, y, w, h) = transform_rect_about_center(
                        border_rect.x,
                        border_rect.y,
                        border_rect.w,
                        border_rect.h,
                        center,
                        scale,
                    );
                    ActiveBorderRect {
                        x,
                        y,
                        w,
                        h,
                        inner_offset_x: border_rect.inner_offset_x * scale,
                        inner_offset_y: border_rect.inner_offset_y * scale,
                        inner_w: (border_rect.inner_w * scale).max(1.0),
                        inner_h: (border_rect.inner_h * scale).max(1.0),
                        alpha: border_rect.alpha,
                        border_px: border_rect.border_px * scale,
                        corner_radius: border_rect.corner_radius * scale,
                        inner_corner_radius: border_rect.inner_corner_radius * scale,
                        border_color: border_rect.border_color,
                    }
                })
                .collect::<Vec<_>>();
            draw_window_borders(
                frame,
                size,
                damage,
                &scaled_border_rects,
                st,
            )?;
        }

        let scaled_textures = offscreen_textures
            .iter()
            .cloned()
            .map(|mut tex| {
                let center = (
                    tex.dst_x as f32 + tex.dst_w as f32 * 0.5,
                    tex.dst_y as f32 + tex.dst_h as f32 * 0.5,
                );
                let (dst_x, dst_y, dst_w, dst_h) = transform_rect_about_center(
                    tex.dst_x, tex.dst_y, tex.dst_w, tex.dst_h, center, scale,
                );
                tex.dst_x = dst_x;
                tex.dst_y = dst_y;
                tex.dst_w = dst_w;
                tex.dst_h = dst_h;
                tex.geo_offset_x *= scale;
                tex.geo_offset_y *= scale;
                tex.geo_w *= scale;
                tex.geo_h *= scale;
                tex.corner_radius *= scale;
                tex
            })
            .collect::<Vec<_>>();
        draw_offscreen_textures(
            frame,
            damage,
            &scaled_textures,
            st.ui.render_state.window_texture_program.as_ref(),
        )?;
    }
    Ok(())
}

fn draw_stack_window_units(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    stack_window_units: &[StackWindowDrawUnit],
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    for unit in stack_window_units {
        if !unit.border_rects.is_empty() {
            draw_window_borders(frame, size, damage, &unit.border_rects, st)?;
        }
        if !unit.active_elements.is_empty() {
            let _ = draw_render_elements(frame, 1.0, &unit.active_elements, &[damage]);
        }
        draw_offscreen_textures(
            frame,
            damage,
            &unit.offscreen_textures,
            st.ui.render_state.window_texture_program.as_ref(),
        )?;
    }
    Ok(())
}

fn draw_window_borders(
    frame: &mut GlesFrame<'_, '_>,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    border_rects: &[ActiveBorderRect],
    st: &Halley,
) -> Result<(), Box<dyn Error>> {
    let border_texture = st.ui.render_state.node_circle_texture.as_ref();
    let border_program = st.ui.render_state.ui_rect_rounded_program.as_ref();
    let window_program = st.ui.render_state.window_texture_program.as_ref();
    let framebuffer = Rectangle::<i32, Physical>::from_size(size);

    for rect in border_rects {
        let border_px = rect.border_px.max(0.0).round() as i32;
        if border_px <= 0 || rect.alpha <= 0.0 {
            continue;
        }

        let dst = Rectangle::<i32, Physical>::new(
            (rect.x - border_px, rect.y - border_px).into(),
            (
                (rect.w + border_px * 2).max(1),
                (rect.h + border_px * 2).max(1),
            )
                .into(),
        );
        let Some(visible) = dst
            .intersection(framebuffer)
            .and_then(|r| r.intersection(damage))
        else {
            continue;
        };
        let local_damage = Rectangle::<i32, Physical>::new(
            (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
            visible.size,
        );
        let fill_color = (0.0f32, 0.0f32, 0.0f32, 0.0f32);

        if rect.corner_radius > 0.0 {
            if let (Some(texture), Some(program)) = (border_texture, border_program) {
                let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
                let src = Rectangle::<f64, Buffer>::new(
                    (0.0, 0.0).into(),
                    (tex_size.w as f64, tex_size.h as f64).into(),
                );
                let uniforms = [
                    Uniform::new(
                        "node_color",
                        (
                            rect.border_color.r(),
                            rect.border_color.g(),
                            rect.border_color.b(),
                            rect.border_color.a(),
                        ),
                    ),
                    Uniform::new("fill_color", fill_color),
                    Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
                    Uniform::new("inner_rect_size", (rect.inner_w, rect.inner_h)),
                    Uniform::new(
                        "inner_rect_offset",
                        (rect.inner_offset_x, rect.inner_offset_y),
                    ),
                    Uniform::new("corner_radius", rect.corner_radius),
                    Uniform::new("inner_corner_radius", rect.inner_corner_radius),
                    Uniform::new("border_px", rect.border_px),
                ];

                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    &[local_damage],
                    &[],
                    Transform::Normal,
                    rect.alpha.clamp(0.0, 1.0),
                    Some(program),
                    &uniforms,
                )?;
                continue;
            }

            if let (Some(texture), Some(program)) = (border_texture, window_program) {
                let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
                let src = Rectangle::<f64, Buffer>::new(
                    (0.0, 0.0).into(),
                    (tex_size.w as f64, tex_size.h as f64).into(),
                );
                let uniforms = [
                    Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
                    Uniform::new("corner_radius", rect.corner_radius),
                    Uniform::new("border_px", rect.border_px),
                    Uniform::new(
                        "border_color",
                        (
                            rect.border_color.r(),
                            rect.border_color.g(),
                            rect.border_color.b(),
                            rect.border_color.a(),
                        ),
                    ),
                    Uniform::new("fill_color", fill_color),
                    Uniform::new("content_alpha_scale", 0.0f32),
                    // Border-only draw: no content geo offset needed.
                    Uniform::new("geo_offset", (0.0f32, 0.0f32)),
                    Uniform::new("geo_size", (0.0f32, 0.0f32)),
                ];

                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    &[local_damage],
                    &[],
                    Transform::Normal,
                    rect.alpha.clamp(0.0, 1.0),
                    Some(program),
                    &uniforms,
                )?;
                continue;
            }
        }

        draw_clamped_outline_rect(
            frame,
            (dst.loc.x, dst.loc.y, dst.size.w, dst.size.h),
            border_px,
            Color32F::new(
                rect.border_color.r(),
                rect.border_color.g(),
                rect.border_color.b(),
                rect.border_color.a() * rect.alpha.clamp(0.0, 1.0),
            ),
            damage,
            size,
        )?;
    }

    Ok(())
}

fn draw_overlap_overlays<F>(
    frame: &mut F,
    damage: Rectangle<i32, Physical>,
    overlap_overlay_rects: &[(i32, i32, i32, i32)],
) -> Result<(), F::Error>
where
    F: Frame,
{
    for &(x, y, w, h) in overlap_overlay_rects {
        draw_rect(
            frame,
            x,
            y,
            w,
            h,
            Color32F::new(0.45, 0.45, 0.45, 0.34),
            damage,
        )?;
        draw_outline_rect(
            frame,
            x,
            y,
            w,
            h,
            Color32F::new(0.72, 0.72, 0.72, 0.78),
            damage,
        )?;
    }

    Ok(())
}

fn draw_geometry_overlays<F>(
    _frame: &mut F,
    _st: &Halley,
    _size: smithay::utils::Size<i32, Physical>,
    _damage: Rectangle<i32, Physical>,
    _scene: &SceneCollections,
) -> Result<(), F::Error>
where
    F: Frame,
{
    Ok(())
}

fn draw_hover_preview(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    scene: &SceneCollections,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    if let Some((px, py, pw, ph)) = scene.hover_preview_rect
        && !scene.hover_preview_elements.is_empty()
    {
        let dl = damage.loc.x;
        let dt = damage.loc.y;
        let dr = damage.loc.x + damage.size.w;
        let db = damage.loc.y + damage.size.h;
        let l = px.max(dl);
        let t = py.max(dt);
        let r = (px + pw).min(dr);
        let b = (py + ph).min(db);
        if r > l && b > t {
            let clip = Rectangle::new((l, t).into(), ((r - l), (b - t)).into());
            let _ = draw_render_elements(frame, 1.0, &scene.hover_preview_elements, &[clip]);
        }
    }

    Ok(())
}

fn draw_cursor_layer(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    cursor_screen: Option<(f32, f32)>,
    cursor: &CursorScene,
    cursor_config: &halley_config::CursorConfig,
) -> Result<(), Box<dyn Error>> {
    if let Some((sx, sy)) = cursor_screen {
        let draw_fallback_arrow = match &cursor.cursor_status {
            smithay::input::pointer::CursorImageStatus::Hidden => false,
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                if let Some(sprite) = themed_cursor_sprite_with_fallback(cursor_config, *icon) {
                    draw_cursor_sprite(frame, damage, (sx, sy), sprite.as_ref())?;
                    false
                } else {
                    true
                }
            }
            smithay::input::pointer::CursorImageStatus::Surface(_) => {
                if !cursor.cursor_surface_elements.is_empty() {
                    let _ = draw_render_elements(
                        frame,
                        1.0,
                        &cursor.cursor_surface_elements,
                        &[damage],
                    );
                }
                false
            }
        };

        if draw_fallback_arrow {
            draw_fallback_cursor_arrow(frame, sx, sy, damage)?;
        }
    }

    Ok(())
}

fn draw_fallback_cursor_arrow<F>(
    frame: &mut F,
    sx: f32,
    sy: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>>
where
    F: smithay::backend::renderer::Frame,
    F::Error: std::error::Error + 'static,
{
    let cx = sx.round() as i32;
    let cy = sy.round() as i32;
    let shadow = Color32F::new(0.0, 0.0, 0.0, 0.40);
    let outline = Color32F::new(0.0, 0.0, 0.0, 0.98);
    let fill = Color32F::new(1.0, 1.0, 1.0, 0.96);

    draw_rect(frame, cx + 2, cy + 2, 2, 14, shadow, damage)?;
    draw_rect(frame, cx + 2, cy + 2, 10, 2, shadow, damage)?;
    draw_rect(frame, cx + 4, cy + 8, 8, 2, shadow, damage)?;
    draw_rect(frame, cx, cy, 2, 14, outline, damage)?;
    draw_rect(frame, cx, cy, 10, 2, outline, damage)?;
    draw_rect(frame, cx + 3, cy + 7, 8, 2, outline, damage)?;
    draw_rect(frame, cx + 1, cy + 1, 1, 12, fill, damage)?;
    draw_rect(frame, cx + 1, cy + 1, 8, 1, fill, damage)?;
    draw_rect(frame, cx + 4, cy + 8, 6, 1, fill, damage)?;
    Ok(())
}
