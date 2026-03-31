use std::collections::HashSet;
use std::error::Error;
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_states, with_surface_tree_downward,
};
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
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::animation::AnimStyle;
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::overlay::{
    OverlayView, draw_cluster_bloom, draw_cluster_overflow_strip, draw_cluster_selection_markers,
    draw_monitor_hud, draw_overlay_hover_label, ensure_cluster_bloom_icon_resources,
};
use crate::spatial::node_in_active_area_for_monitor;

use super::app_icon::{ensure_app_icon_resources_for_node_ids, ensure_node_app_icon_resources};
use super::bearings::BearingChipLayout;
use super::bearings::{collect_bearing_layouts, draw_bearings, ensure_bearing_icon_resources};
use super::cursor::{cursor_surface_hotspot, draw_cursor_sprite};
use super::cursor_theme::themed_cursor_sprite_with_fallback;
use super::layer_shell::collect_layer_surfaces;
use super::log_rounded_shader_failure;
use super::node::{
    NodeSnapshot, collect_hover_preview, draw_node_hover_labels, draw_node_markers,
    ensure_node_circle_resources,
};
use super::utils::{draw_outline_rect, draw_rect, draw_ring, world_to_screen};
use super::window::{ActiveBorderRect, OffscreenNodeTexture, collect_active_surfaces};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
type CroppedSurfaceElement =
    smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>;

const WINDOW_TEXTURE_SHADER: &str = include_str!("shaders/window_rounded_texture.frag");
const SURFACE_CLIP_SHADER: &str = include_str!("shaders/surface_clipped_texture.frag");

pub(crate) fn monitor_overlay_requires_full_repaint(st: &Halley, monitor: &str) -> bool {
    st.cluster_mode_active_for_monitor(monitor)
        || st.ui.render_state.overlay_banner.contains_key(monitor)
        || st.ui.render_state.overlay_toast.contains_key(monitor)
}

pub(crate) fn begin_render_frame(st: &mut Halley, now: Instant) {
    st.ui.render_state.render_last_tick = now;
    st.platform.popup_manager.cleanup();
    let alive: HashSet<NodeId> = st.model.field.node_ids_all().into_iter().collect();
    st.input
        .interaction_state
        .physics_velocity
        .retain(|id, _| alive.contains(id));
    st.input
        .interaction_state
        .smoothed_render_pos
        .retain(|id, _| alive.contains(id));
    st.ui
        .render_state
        .node_hover_mix
        .retain(|id, _| alive.contains(id));
    st.ui.render_state.node_preview_hover.retain(|_, state| {
        state.node = state.node.filter(|id| alive.contains(id));
        state.node.is_some() || state.mix > 0.002
    });
    st.ui.render_state.bearings_mix.retain(|monitor, mix| {
        st.model.monitor_state.monitors.contains_key(monitor) || *mix > 0.002
    });
    st.ui
        .render_state
        .cluster_bloom_mix
        .retain(|monitor, state| {
            st.model.monitor_state.monitors.contains_key(monitor) || state.mix > 0.002
        });
    st.ui.render_state.prune_window_offscreen_cache(&alive, now);
}

pub(crate) fn anim_style_for(
    st: &Halley,
    id: NodeId,
    state: halley_core::field::NodeState,
    now: Instant,
) -> AnimStyle {
    if !st.runtime.tuning.dev_anim_enabled || !st.runtime.tuning.physics_enabled {
        return AnimStyle::default();
    }

    let now_ms = st.now_ms(now);
    if st.input.interaction_state.resize_active == Some(id)
        || (st.input.interaction_state.resize_static_node == Some(id)
            && now_ms < st.input.interaction_state.resize_static_until_ms)
    {
        return AnimStyle::default();
    }

    st.ui.render_state.animator.style_for(id, state, now)
}

pub(crate) fn tick_animator_frame(st: &mut Halley, now: Instant) {
    st.ui
        .render_state
        .tick_animator_frame(&st.model.field, st.runtime.tuning.physics_enabled, now);
}

pub(crate) fn tick_frame_effects(st: &mut Halley, now: Instant) {
    let now_ms = st.now_ms(now);
    st.tick_viewport_pan_animation(now_ms);
    st.tick_pending_spawn_pan(now, now_ms);
    tick_active_drag(st, now);
    crate::compositor::interaction::state::tick_cluster_join_candidate_ready(st, now_ms);
    st.tick_camera_smoothing(now);
}

fn tick_active_drag(st: &mut Halley, now: Instant) {
    let Some(mut active_drag) = st.input.interaction_state.active_drag.clone() else {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        return;
    };

    let Some(node_id) = st.input.interaction_state.drag_authority_node else {
        st.input.interaction_state.active_drag = None;
        return;
    };
    if node_id != active_drag.node_id {
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        return;
    }

    let pointer_world = crate::spatial::screen_to_world(
        st,
        active_drag.pointer_workspace_size.0,
        active_drag.pointer_workspace_size.1,
        active_drag.pointer_screen_local.0,
        active_drag.pointer_screen_local.1,
    );
    let desired_to = Vec2 {
        x: pointer_world.x - active_drag.current_offset.x,
        y: pointer_world.y - active_drag.current_offset.y,
    };

    let moved = if active_drag.allow_monitor_transfer {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        st.assign_node_to_monitor(node_id, active_drag.pointer_monitor.as_str());
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(active_drag.pointer_monitor.as_str()) == Some(cid))
                .then_some(clamped)
        })
        .unwrap_or(desired_to);
        st.carry_surface_non_overlap(node_id, to, false)
    } else if !active_drag.edge_pan_eligible {
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(active_drag.pointer_monitor.as_str()) == Some(cid))
                .then_some(clamped)
        })
        .unwrap_or(desired_to);
        st.carry_surface_non_overlap(node_id, to, false)
    } else if let Some((clamped_center, edge_contact)) =
        crate::compositor::interaction::state::dragged_node_edge_pan_clamp(
            st,
            active_drag.pointer_monitor.as_str(),
            node_id,
            desired_to,
            Vec2 {
                x: active_drag.edge_pan_x.sign(),
                y: active_drag.edge_pan_y.sign(),
            },
        )
    {
        if active_drag.edge_pan_x.sign() != 0.0 && edge_contact.x != active_drag.edge_pan_x.sign() {
            active_drag.edge_pan_x = crate::compositor::interaction::DragAxisMode::Free;
        }
        if active_drag.edge_pan_y.sign() != 0.0 && edge_contact.y != active_drag.edge_pan_y.sign() {
            active_drag.edge_pan_y = crate::compositor::interaction::DragAxisMode::Free;
        }

        let direction = Vec2 {
            x: active_drag.edge_pan_x.sign(),
            y: active_drag.edge_pan_y.sign(),
        };
        let edge_pan_active = direction.x != 0.0 || direction.y != 0.0;
        st.input.interaction_state.grabbed_edge_pan_active = edge_pan_active;
        st.input.interaction_state.grabbed_edge_pan_direction = direction;
        st.input.interaction_state.grabbed_edge_pan_monitor =
            edge_pan_active.then(|| active_drag.pointer_monitor.clone());

        let mut to = clamped_center;
        if edge_pan_active {
            let dt = now
                .saturating_duration_since(st.ui.render_state.render_last_tick)
                .as_secs_f32()
                .clamp(1.0 / 240.0, 1.0 / 30.0);
            const DRAG_EDGE_PAN_SPEED: f32 = 720.0;
            let pan_delta = Vec2 {
                x: direction.x * DRAG_EDGE_PAN_SPEED * dt,
                y: direction.y * DRAG_EDGE_PAN_SPEED * dt,
            };
            st.note_pan_activity(now);
            st.pan_camera_target(pan_delta);
            st.model.viewport.center = st.model.camera_target_center;
            st.runtime.tuning.viewport_center = st.model.viewport.center;
            st.sync_current_monitor_state();
            st.note_pan_viewport_change(now);

            let post_pan_pointer_world = crate::spatial::screen_to_world(
                st,
                active_drag.pointer_workspace_size.0,
                active_drag.pointer_workspace_size.1,
                active_drag.pointer_screen_local.0,
                active_drag.pointer_screen_local.1,
            );
            let post_pan_desired_to = Vec2 {
                x: post_pan_pointer_world.x - active_drag.current_offset.x,
                y: post_pan_pointer_world.y - active_drag.current_offset.y,
            };
            to = crate::compositor::interaction::state::dragged_node_edge_pan_clamp(
                st,
                active_drag.pointer_monitor.as_str(),
                node_id,
                post_pan_desired_to,
                direction,
            )
            .map(|(clamped, _)| clamped)
            .unwrap_or(post_pan_desired_to);
        }
        let drag_monitor = active_drag.pointer_monitor.clone();
        st.input.interaction_state.active_drag = Some(active_drag.clone());
        let to = crate::compositor::interaction::state::dragged_node_cluster_core_clamp(
            st,
            drag_monitor.as_str(),
            node_id,
            to,
        )
        .and_then(|(clamped, cid, _)| {
            (st.cluster_bloom_for_monitor(drag_monitor.as_str()) == Some(cid)).then_some(clamped)
        })
        .unwrap_or(to);
        st.carry_surface_non_overlap(node_id, to, false)
    } else {
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
        return;
    };
    let live_reordered = if st.model.field.is_active_cluster_member(node_id) {
        st.move_active_cluster_member_to_drop_tile(
            active_drag.pointer_monitor.as_str(),
            node_id,
            pointer_world,
            st.now_ms(now),
        )
    } else {
        false
    };
    if moved || live_reordered {
        st.request_maintenance();
    }
}

pub(crate) fn tick_live_overlap(st: &mut Halley) {
    if st.input.interaction_state.suspend_state_checks
        || st.input.interaction_state.resize_active.is_some()
    {
        return;
    }
    st.resolve_surface_overlap();
}

pub(crate) fn send_frame_callbacks(st: &mut Halley, now: Instant) {
    let elapsed_ms = now.duration_since(st.runtime.started_at).as_millis();
    let time_ms = elapsed_ms.min(u32::MAX as u128) as u32;
    for layer in st.platform.wlr_layer_shell_state.layer_surfaces() {
        send_frames_surface_tree(layer.wl_surface(), time_ms);
    }
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        send_frames_surface_tree(top.wl_surface(), time_ms);
    }
    for popup in st.platform.xdg_shell_state.popup_surfaces() {
        send_frames_surface_tree(popup.wl_surface(), time_ms);
    }
}

fn send_frames_surface_tree(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    time_ms: u32,
) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time_ms);
            }
        },
        |_, _, &()| true,
    );
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
            UniformName::new("geo_size", UniformType::_2f),
            UniformName::new("elem_size", UniformType::_2f),
            UniformName::new("elem_offset", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_1f),
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
    layer_background_elements: Vec<SurfaceElement>,
    layer_bottom_elements: Vec<SurfaceElement>,
    layer_top_elements: Vec<SurfaceElement>,
    layer_overlay_elements: Vec<SurfaceElement>,
    active_elements: Vec<CroppedSurfaceElement>,
    resized_active_elements: Vec<CroppedSurfaceElement>,
    fullscreen_active_elements: Vec<CroppedSurfaceElement>,
    offscreen_textures: Vec<OffscreenNodeTexture>,
    resized_offscreen_textures: Vec<OffscreenNodeTexture>,
    fullscreen_offscreen_textures: Vec<OffscreenNodeTexture>,
    popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    popup_elements: Vec<CroppedSurfaceElement>,
    fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    fullscreen_popup_elements: Vec<CroppedSurfaceElement>,
    border_rects: Vec<ActiveBorderRect>,
    resized_border_rects: Vec<ActiveBorderRect>,
    overlay_rects: Vec<(i32, i32, i32, i32, Color32F)>,
    overlay_points: Vec<(i32, i32, Color32F)>,
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
    let current_monitor = st.model.monitor_state.current_monitor.clone();
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
        .collect::<Vec<_>>();
    ensure_app_icon_resources_for_node_ids(renderer, st, overflow_ids.into_iter())?;
    ensure_cluster_bloom_icon_resources(renderer, st, current_monitor.as_str())?;
    ensure_bearing_icon_resources(renderer, st, current_monitor.as_str())?;
    let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);
    let mut frame = renderer.render(framebuffer, size, frame_transform)?;
    frame.clear(Color32F::new(0.04, 0.05, 0.06, 1.0), &[prepared.damage])?;

    draw_debug_frame_scene(&mut frame, st, size, &prepared, &scene, hover_node)?;
    draw_cursor_layer(&mut frame, prepared.damage, cursor_screen, &cursor)?;

    let _ = frame.finish()?;
    Ok(())
}

fn prepare_debug_frame_state(
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
) -> PreparedFrameState {
    let now = Instant::now();
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
        border_rects,
        resized_border_rects,
        overlay_rects,
        overlay_points,
        overlap_overlay_rects,
    ) = collect_active_surfaces(renderer, st, size, resize_preview, now);

    let hovered_preview_id = preview_hover_node.and_then(|id| {
        st.model.field.node(id).and_then(|n| {
            (node_in_active_area_for_monitor(st, id, render_monitor.as_str())
                && matches!(
                    n.state,
                    halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
                ))
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
        border_rects,
        resized_border_rects,
        overlay_rects,
        overlay_points,
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
    draw_overlap_overlays(frame, prepared.damage, &scene.overlap_overlay_rects)?;
    draw_window_borders(
        frame,
        size,
        prepared.damage,
        &scene.resized_border_rects,
        st,
    )?;

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
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.popup_offscreen_textures,
        st.ui.render_state.window_texture_program.as_ref(),
    )?;

    if !scene.popup_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.popup_elements, &[prepared.damage]);
    }

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
        let base_px_per_world_x = size.w as f32 / st.model.viewport.size.x.max(1.0);
        let base_px_per_world_y = size.h as f32 / st.model.viewport.size.y.max(1.0);
        let screen_rx = focus_ring.radius_x * base_px_per_world_x;
        let screen_ry = focus_ring.radius_y * base_px_per_world_y;
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

fn draw_window_borders(
    frame: &mut GlesFrame<'_, '_>,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    border_rects: &[ActiveBorderRect],
    st: &Halley,
) -> Result<(), Box<dyn Error>> {
    let border_texture = st.ui.render_state.node_circle_texture.as_ref();
    let border_program = st.ui.render_state.node_label_program.as_ref();
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
                    Uniform::new("fill_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
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
                    Uniform::new("fill_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
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

        draw_rect(
            frame,
            visible.loc.x,
            visible.loc.y,
            visible.size.w,
            visible.size.h,
            Color32F::new(
                rect.border_color.r(),
                rect.border_color.g(),
                rect.border_color.b(),
                rect.border_color.a() * rect.alpha.clamp(0.0, 1.0),
            ),
            damage,
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
    frame: &mut F,
    st: &Halley,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    scene: &SceneCollections,
) -> Result<(), F::Error>
where
    F: Frame,
{
    if st.runtime.tuning.dev_enabled && st.runtime.tuning.dev_show_geometry_overlay {
        for &(x, y, w, h, color) in &scene.overlay_rects {
            draw_clamped_outline_rect(frame, (x, y, w, h), 2, color, damage, size)?;
        }
        for &(x, y, color) in &scene.overlay_points {
            draw_rect(frame, x - 2, y - 2, 5, 5, color, damage)?;
        }
    }

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
) -> Result<(), Box<dyn Error>> {
    if let Some((sx, sy)) = cursor_screen {
        let draw_fallback_arrow = match &cursor.cursor_status {
            smithay::input::pointer::CursorImageStatus::Hidden => false,
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                if let Some(sprite) = themed_cursor_sprite_with_fallback(*icon) {
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
