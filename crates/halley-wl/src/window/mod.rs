use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::cluster_layout::ClusterCycleDirection;
use halley_core::field::{NodeId, Vec2};
use halley_core::tiling::Rect;
use smithay::{
    backend::renderer::{
        Color32F, Texture,
        element::{
            Kind, render_elements, surface::render_elements_from_surface_tree,
            utils::CropRenderElement,
        },
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::{PopupManager, utils::bbox_from_surface_tree},
    reexports::wayland_server::Resource,
    utils::{Logical, Physical, Rectangle, Size},
    wayland::compositor::with_states,
    wayland::shell::xdg::SurfaceCachedState,
};

use crate::animation::{active_surface_render_scale, ease_in_out_cubic, ease_out_back};
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::monitor::layer_shell::layer_output_size_for_monitor;
use crate::compositor::root::Halley;
use crate::compositor::spawn::state::{is_persistent_rule_top, node_floats_over_active_cluster};
use crate::compositor::surface::{
    active_stacking_visible_members_for_monitor, is_active_cluster_workspace_member,
    window_geometry_for_node,
};
use crate::input::active_resize_geometry_screen;
use crate::presentation::world_to_screen;

use crate::render::clipped_surface::ClippedSurfaceRenderElement;
use crate::render::pin_icon::PinBadgeLayout;
use crate::render::surface_capture::render_surface_tree_to_texture;

mod capture;
mod decoration;
mod geometry;
mod stack;

pub(crate) use capture::{
    capture_closing_window_animation, capture_window_to_png_via_renderer,
    prewarm_visible_active_window_offscreen_caches,
};
pub(crate) use decoration::active_window_frame_pad_px;
use decoration::{
    build_window_border_rects, build_window_shadow_rect, scaled_window_border_px,
    window_decoration_metrics,
};
use geometry::{
    log_window_render_path, offscreen_visual_crop_and_dst, rect_from_local_geometry, rect4_str,
    rect4f_str, sync_node_size_from_surface, wrap_direct_surface_elements,
};
use stack::{build_stack_transition_plan, clone_stack_window_unit_for_pose, stack_draw_order_map};

#[cfg(test)]
use capture::world_to_screen_for_view;

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
render_elements! {
    pub(crate) DirectSurfaceElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Clipped=ClippedSurfaceRenderElement,
}
pub(crate) type CroppedClippedSurfaceElement = CropRenderElement<DirectSurfaceElement>;
type CroppedSurfaceElement = CropRenderElement<SurfaceElement>;

#[derive(Clone)]
pub(crate) struct ActiveBorderRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub inner_offset_x: f32,
    pub inner_offset_y: f32,
    pub inner_w: f32,
    pub inner_h: f32,
    pub alpha: f32,
    pub border_px: f32,
    pub corner_radius: f32,
    pub inner_corner_radius: f32,
    pub border_color: Color32F,
}

#[derive(Clone)]
pub(crate) struct OffscreenNodeTexture {
    pub texture: GlesTexture,
    pub alpha: f32,
    pub corner_radius: f32,
    pub src_x: f64,
    pub src_y: f64,
    pub src_w: f64,
    pub src_h: f64,
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: i32,
    pub dst_h: i32,
    pub clip_x: i32,
    pub clip_y: i32,
    pub clip_w: i32,
    pub clip_h: i32,
    /// Geometry rect within the dst rect (in dst-local pixels).
    /// Used by the shader to clip content to the window geometry rather than
    /// the full bbox, so visual margin pixels do not poke past the rounded border.
    pub geo_offset_x: f32,
    pub geo_offset_y: f32,
    pub geo_w: f32,
    pub geo_h: f32,
}

#[derive(Clone)]
pub(crate) struct WindowShadowRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub corner_radius: f32,
    pub alpha: f32,
}

pub(crate) struct StackWindowDrawUnit {
    pub node_id: NodeId,
    pub draw_order: i32,
    pub shadow_rects: Vec<WindowShadowRect>,
    pub border_rects: Vec<ActiveBorderRect>,
    pub pin_badges: Vec<PinBadgeLayout>,
    pub active_elements: Vec<CroppedClippedSurfaceElement>,
    pub offscreen_textures: Vec<OffscreenNodeTexture>,
}

impl StackWindowDrawUnit {
    fn new(node_id: NodeId, draw_order: i32) -> Self {
        Self {
            node_id,
            draw_order,
            shadow_rects: Vec::new(),
            border_rects: Vec::new(),
            pin_badges: Vec::new(),
            active_elements: Vec::new(),
            offscreen_textures: Vec::new(),
        }
    }
}

pub(crate) fn node_is_game_like(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .node_app_ids
        .get(&node_id)
        .is_some_and(|app_id| app_id.starts_with("steam_app_"))
}

pub(crate) fn node_requires_live_surface_render(st: &Halley, node_id: NodeId) -> bool {
    node_is_game_like(st, node_id) || st.fullscreen_monitor_for_node(node_id).is_some()
}

fn rect_covers_output(rect: (i32, i32, i32, i32), output: Rectangle<i32, Physical>) -> bool {
    let tolerance = 2;
    rect.0 <= output.loc.x + tolerance
        && rect.1 <= output.loc.y + tolerance
        && rect.0 + rect.2 >= output.loc.x + output.size.w - tolerance
        && rect.1 + rect.3 >= output.loc.y + output.size.h - tolerance
}

fn active_surface_draw_rank(st: &Halley, node_id: NodeId) -> (u64, u64) {
    let (rank, tie) = st.overlap_policy_stack_rank(node_id);
    if node_floats_over_active_cluster(st, node_id) {
        (rank.saturating_add(1_u64 << 62), tie)
    } else {
        (rank, tie)
    }
}

fn overlap_policy_draw_order(st: &Halley, node_id: NodeId) -> i32 {
    let rank = st
        .overlap_policy_stack_rank(node_id)
        .0
        .min((i32::MAX / 4) as u64) as i32;
    if node_floats_over_active_cluster(st, node_id) {
        i32::MAX / 2 + rank
    } else {
        rank
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn collect_active_surfaces(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    size: Size<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    now: Instant,
) -> (
    Vec<CroppedClippedSurfaceElement>,
    Vec<CroppedClippedSurfaceElement>,
    Vec<CroppedClippedSurfaceElement>,
    Vec<CroppedClippedSurfaceElement>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    HashMap<
        halley_core::field::NodeId,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    >,
    Vec<StackWindowDrawUnit>,
    Vec<StackWindowDrawUnit>,
    Vec<WindowShadowRect>,
    Vec<WindowShadowRect>,
    Vec<WindowShadowRect>,
    Vec<ActiveBorderRect>,
    Vec<ActiveBorderRect>,
    Vec<ActiveBorderRect>,
    Vec<PinBadgeLayout>,
) {
    let active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let mut resized_active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let fullscreen_active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let mut above_fullscreen_active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut resized_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let fullscreen_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut above_fullscreen_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut above_fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let fullscreen_popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut above_fullscreen_popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut node_surface_map = HashMap::new();
    crate::animation::retain_live_cluster_tile_tracks(
        &mut st.ui.render_state.cluster_tile_tracks,
        &st.model.field,
        now,
    );
    if st.runtime.tuning.tile_animation_enabled()
        && crate::animation::cluster_tile_tracks_animating(
            &st.ui.render_state.cluster_tile_tracks,
            now,
        )
    {
        st.request_maintenance();
    }
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let stack_visible_front_to_back =
        active_stacking_visible_members_for_monitor(st, current_monitor.as_str());
    let stack_cycle_transition = st
        .runtime
        .tuning
        .stack_animation_enabled()
        .then(|| {
            st.ui
                .render_state
                .stack_cycle_transition_for_monitor(current_monitor.as_str(), now)
        })
        .flatten();
    if stack_cycle_transition.is_some() {
        st.request_maintenance();
    }
    let stack_transition_plan = stack_cycle_transition.as_ref().and_then(|transition| {
        build_stack_transition_plan(st, current_monitor.as_str(), transition)
    });
    let stack_render_front_to_back = if let Some(transition) = stack_cycle_transition.as_ref() {
        let mut ids = transition.old_visible.clone();
        for &node_id in &transition.new_visible {
            if !ids.contains(&node_id) {
                ids.push(node_id);
            }
        }
        ids
    } else {
        stack_visible_front_to_back.clone()
    };
    let stack_render_set: HashSet<_> = stack_render_front_to_back.iter().copied().collect();
    let stack_draw_orders = if let Some(plan) = stack_transition_plan.as_ref() {
        plan.poses
            .iter()
            .map(|(&node_id, pose)| (node_id, pose.draw_order))
            .collect::<HashMap<_, _>>()
    } else {
        stack_draw_order_map(&stack_visible_front_to_back)
    };
    let mut stack_window_units: HashMap<NodeId, StackWindowDrawUnit> = HashMap::new();
    let mut above_fullscreen_stack_window_units: HashMap<NodeId, StackWindowDrawUnit> =
        HashMap::new();
    let shadow_rects: Vec<WindowShadowRect> = Vec::new();
    let mut resized_shadow_rects: Vec<WindowShadowRect> = Vec::new();
    let mut above_fullscreen_shadow_rects: Vec<WindowShadowRect> = Vec::new();
    let border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut resized_border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut above_fullscreen_border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut pin_badges: Vec<PinBadgeLayout> = Vec::new();

    let output_clip = Rectangle::<i32, Physical>::new((0, 0).into(), size);
    let mut wl_surfaces: Vec<_> = st
        .platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|t| {
            let wl = t.wl_surface().clone();
            let key = wl.id();
            let node_id = st.model.surface_to_node.get(&key).copied()?;
            let node = st.model.field.node(node_id)?;
            if node.state != halley_core::field::NodeState::Active
                || !st.model.field.is_visible(node_id)
                || !st.node_assigned_to_current_monitor(node_id)
            {
                return None;
            }
            Some((node_id, wl))
        })
        .collect();

    wl_surfaces.sort_by_key(|(id, _)| active_surface_draw_rank(st, *id));

    for (node_id, wl) in wl_surfaces {
        let bbox = if resize_preview.is_some_and(|rz| rz.node_id == node_id) {
            bbox_from_surface_tree(&wl, (0, 0))
        } else {
            sync_node_size_from_surface(st, node_id, &wl)
        };

        let Some(node) = st.model.field.node(node_id) else {
            continue;
        };
        node_surface_map.insert(node_id, wl.clone());
        let stack_transition_pose = stack_transition_plan
            .as_ref()
            .and_then(|plan| plan.poses.get(&node_id).copied());
        let stack_member_rendered = stack_render_set.contains(&node_id);
        if node.state != halley_core::field::NodeState::Active
            || !st.model.field.is_visible(node_id)
            || !st.node_assigned_to_current_monitor(node_id)
        {
            continue;
        }

        let node_pos = node.pos;
        let node_state = node.state.clone();
        let node_intrinsic = node.intrinsic_size;
        let fullscreen_on_current_monitor = st
            .fullscreen_monitor_for_node(node_id)
            .is_some_and(|monitor| monitor == st.model.monitor_state.current_monitor);
        let fullscreen_visual =
            crate::compositor::fullscreen::system::fullscreen_visual_for_node_on_current_monitor_at(
                st, node_id, now,
            );
        let fullscreen_visual_animating = crate::compositor::fullscreen::system::fullscreen_visual_animation_active_for_node_on_current_monitor_at(
            st, node_id, now,
        );
        let exact_fullscreen_output = fullscreen_on_current_monitor && !fullscreen_visual_animating;
        let maximized_visual =
            crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor_at(
                st, node_id, now,
            );

        let active_cluster_member = is_active_cluster_workspace_member(st, node_id);
        let dragging_this_node = st.input.interaction_state.drag_authority_node == Some(node_id);
        let tiling_tile_transition = (active_cluster_member
            && !dragging_this_node
            && st.runtime.tuning.tile_animation_enabled()
            && matches!(
                st.runtime.tuning.cluster_layout_kind(),
                halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
            ))
        .then(|| {
            crate::animation::cluster_tile_rect_for(
                &st.ui.render_state.cluster_tile_tracks,
                node_id,
                now,
            )
        })
        .flatten();
        let frozen_tiling_geometry = tiling_tile_transition.and_then(|_| {
            st.ui
                .render_state
                .cluster_tile_frozen_geometry
                .get(&node_id)
                .copied()
        });
        let transition_alpha =
            crate::compositor::workspace::state::active_transition_alpha(st, node_id, now);
        let anim = crate::frame_loop::anim_style_for(st, node_id, node_state.clone(), now);
        let fullscreen_entry_scale = st.fullscreen_entry_scale(node_id, st.now_ms(now));
        let active_resize = active_resize_geometry_screen(st, node_id, resize_preview);
        let resizing_this_node = active_resize.is_some();
        let dragging_this_node = st.input.interaction_state.drag_authority_node == Some(node_id);
        let persistent_rule_top = is_persistent_rule_top(st, node_id);
        let overlap_policy_stack_this_node = st.node_has_overlap_policy(node_id);
        let draw_top_this_node = resizing_this_node
            || dragging_this_node
            || (persistent_rule_top && !overlap_policy_stack_this_node);
        let draw_above_fullscreen_this_node =
            st.node_draws_above_fullscreen_on_current_monitor(node_id);
        let overlap_policy_draw_order = overlap_policy_draw_order(st, node_id);
        let live_surface_node = node_requires_live_surface_render(st, node_id);
        let raise_anim = st.ui.render_state.raise_animation_for(node_id, now);

        let force_live_surface_scale =
            resizing_this_node || dragging_this_node || active_cluster_member;
        let (scale, live_ramp) = if force_live_surface_scale {
            (1.0f32 * fullscreen_entry_scale, 1.0f32)
        } else {
            let s = active_surface_render_scale(
                anim.scale,
                st.active_zoom_lock_scale(),
                node_intrinsic.x,
                node_intrinsic.y,
                transition_alpha,
            );
            let live_t = ((anim.scale - 0.44) / (1.0 - 0.44)).clamp(0.0, 1.0);
            let live_ramp = if transition_alpha > 0.0 {
                ease_out_back((1.0 - transition_alpha).clamp(0.0, 1.0), 1.42).clamp(0.0, 1.08)
            } else {
                ease_in_out_cubic(live_t).clamp(0.0, 1.0)
            };
            (s * fullscreen_entry_scale, live_ramp)
        };

        // Fit scale for fullscreen windows that don't match the physical monitor resolution.
        let fit_scale = if fullscreen_on_current_monitor {
            1.0
        } else if let Some(monitor) = st.fullscreen_monitor_for_node(node_id) {
            let (target_w, target_h) = st.fullscreen_target_size_for(monitor);
            let sw = (target_w as f32) / node_intrinsic.x.max(1.0);
            let sh = (target_h as f32) / node_intrinsic.y.max(1.0);
            sw.min(sh).max(0.1) // aspect-correct fit
        } else {
            1.0
        };

        let cam_scale = st.camera_render_scale();
        let raise_scale = if fullscreen_on_current_monitor || live_surface_node {
            1.0
        } else {
            raise_anim.scale
        };
        let raise_shadow_boost = if fullscreen_on_current_monitor || live_surface_node {
            0.0
        } else {
            raise_anim.shadow_boost
        };
        let p = stack_transition_pose
            .map(|pose| pose.center)
            .or_else(|| tiling_tile_transition.map(|rect| rect.center))
            .or_else(|| fullscreen_visual.map(|(center, _)| center))
            .or_else(|| maximized_visual.map(|(center, _)| center))
            .unwrap_or(node_pos);
        let local_bbox = (
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w.max(1) as f32,
            bbox.size.h.max(1) as f32,
        );

        let local_geo = if stack_member_rendered {
            let base_geo = window_geometry_for_node(st, node_id).unwrap_or(local_bbox);
            let target_size = stack_transition_pose
                .map(|pose| pose.size)
                .or_else(|| st.model.field.node(node_id).map(|node| node.intrinsic_size))
                .unwrap_or(Vec2 {
                    x: base_geo.2,
                    y: base_geo.3,
                });
            (
                base_geo.0,
                base_geo.1,
                target_size.x.max(1.0),
                target_size.y.max(1.0),
            )
        } else {
            frozen_tiling_geometry
                .unwrap_or_else(|| window_geometry_for_node(st, node_id).unwrap_or(local_bbox))
        };

        let render_scale = if let Some((_, visual_size)) = fullscreen_visual {
            let scale_x = visual_size.x * cam_scale / local_geo.2.max(1.0);
            let scale_y = visual_size.y * cam_scale / local_geo.3.max(1.0);
            scale_x.min(scale_y).max(0.001)
        } else if let Some((_, visual_size)) = maximized_visual {
            let scale_x = visual_size.x * cam_scale / local_geo.2.max(1.0);
            let scale_y = visual_size.y * cam_scale / local_geo.3.max(1.0);
            scale_x.min(scale_y).max(0.001)
        } else {
            scale * cam_scale * fit_scale * raise_scale
        };

        let (_cx, _cy, sx, sy, texture_rect, geometry_rect) =
            if let Some(active_resize) = active_resize {
                let (cx, cy) = active_resize.center_px();
                let (surface_origin_x, surface_origin_y) = active_resize.surface_origin_px();
                let frame = active_resize.frame_rect_px();
                (cx, cy, surface_origin_x, surface_origin_y, frame, frame)
            } else {
                let (cx, cy) = world_to_screen(st, size.w, size.h, p.x, p.y);

                let (render_geo_w, render_geo_h) = tiling_tile_transition
                    .map(|rect| (rect.size.x, rect.size.y))
                    .unwrap_or((local_geo.2, local_geo.3));
                let rw = (render_geo_w * render_scale).round().max(1.0) as i32;
                let rh = (render_geo_h * render_scale).round().max(1.0) as i32;

                let (rx, ry, rw, rh) = if exact_fullscreen_output {
                    (
                        output_clip.loc.x,
                        output_clip.loc.y,
                        output_clip.size.w,
                        output_clip.size.h,
                    )
                } else {
                    let rx = cx - (rw / 2);
                    let ry = cy - (rh / 2);
                    (rx, ry, rw, rh)
                };

                let sx = rx - (local_geo.0 * render_scale).round() as i32;
                let sy = ry - (local_geo.1 * render_scale).round() as i32;

                let texture_rect = rect_from_local_geometry(sx, sy, render_scale, local_bbox);
                let geometry_rect = (rx, ry, rw, rh);

                (cx, cy, sx, sy, texture_rect, geometry_rect)
            };

        let element_scale = if active_resize.is_some() {
            scale
        } else {
            render_scale
        };

        let (gx, gy, gw, gh) = geometry_rect;
        let game_covers_output = live_surface_node
            && node_is_game_like(st, node_id)
            && rect_covers_output((gx, gy, gw.max(1), gh.max(1)), output_clip);
        let fullscreen_like_for_render = fullscreen_on_current_monitor || game_covers_output;

        let alpha = (anim.alpha
            * live_ramp
            * stack_transition_pose.map(|pose| pose.alpha).unwrap_or(1.0)
            * tiling_tile_transition.map(|rect| rect.alpha).unwrap_or(1.0))
        .clamp(0.0, 1.0);
        let window_pin_badge = if st.node_user_pinned(node_id) && alpha > 0.01 {
            let radius = crate::render::pin_icon::scaled_pin_badge_radius(
                st,
                ((14.0 * render_scale.sqrt().clamp(0.85, 1.25)).round() as i32).clamp(10, 18),
            );
            let corner_outset = ((radius as f32) * 0.25).round() as i32;
            let cx = match st.runtime.tuning.pins.corner {
                halley_config::PinBadgeCorner::TopLeft => gx - corner_outset,
                halley_config::PinBadgeCorner::TopRight => gx + gw.max(1) + corner_outset,
            };
            Some(PinBadgeLayout {
                cx,
                cy: gy - corner_outset,
                radius,
                alpha,
            })
        } else {
            None
        };
        let decoration_metrics = if fullscreen_like_for_render {
            window_decoration_metrics(0, 0, 0, 0)
        } else {
            window_decoration_metrics(
                scaled_window_border_px(st.runtime.tuning.window_border_radius_px(), render_scale),
                scaled_window_border_px(
                    st.runtime.tuning.window_primary_border_size_px(),
                    render_scale,
                ),
                scaled_window_border_px(
                    st.runtime.tuning.window_secondary_border_gap_px(),
                    render_scale,
                ),
                scaled_window_border_px(
                    st.runtime.tuning.window_secondary_border_size_px(),
                    render_scale,
                ),
            )
        };
        let logical_content_corner_radius_px = if fullscreen_on_current_monitor {
            0
        } else {
            st.runtime.tuning.window_border_radius_px()
        };
        let lock_dst_to_geometry = decoration_metrics.content_corner_radius_px > 0;
        let clip_geo = local_geo;
        let offscreen_clip = st
            .ui
            .render_state
            .gpu
            .surface_clip_program
            .as_ref()
            .map(|program| {
                let geo_rect = Rectangle::<i32, Logical>::new(
                    (local_geo.0.round() as i32, local_geo.1.round() as i32).into(),
                    (
                        local_geo.2.round().max(1.0) as i32,
                        local_geo.3.round().max(1.0) as i32,
                    )
                        .into(),
                );
                (
                    geo_rect,
                    logical_content_corner_radius_px as f32,
                    program.clone(),
                )
            });
        let preserve_visual_margin = false;
        let window_shadow_rect = build_window_shadow_rect(
            st,
            gx,
            gy,
            gw.max(1),
            gh.max(1),
            (alpha + raise_shadow_boost).clamp(0.0, 1.0),
            decoration_metrics,
            fullscreen_like_for_render,
        );
        if let Some(shadow_rect) = window_shadow_rect {
            if stack_render_set.contains(&node_id) {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| {
                        StackWindowDrawUnit::new(
                            node_id,
                            stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                        )
                    })
                    .shadow_rects
                    .push(shadow_rect);
            } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
                above_fullscreen_stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .shadow_rects
                    .push(shadow_rect);
            } else if overlap_policy_stack_this_node {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .shadow_rects
                    .push(shadow_rect);
            } else if draw_above_fullscreen_this_node {
                above_fullscreen_shadow_rects.push(shadow_rect);
            } else if draw_top_this_node {
                resized_shadow_rects.push(shadow_rect);
            } else {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .shadow_rects
                    .push(shadow_rect);
            }
        }
        let window_border_rects = build_window_border_rects(
            st,
            node_id,
            gx,
            gy,
            gw.max(1),
            gh.max(1),
            alpha,
            render_scale,
            fullscreen_like_for_render,
        );
        if stack_render_set.contains(&node_id) {
            stack_window_units
                .entry(node_id)
                .or_insert_with(|| {
                    StackWindowDrawUnit::new(
                        node_id,
                        stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                    )
                })
                .border_rects = window_border_rects;
        } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
            above_fullscreen_stack_window_units
                .entry(node_id)
                .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                .border_rects = window_border_rects;
        } else if overlap_policy_stack_this_node {
            stack_window_units
                .entry(node_id)
                .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                .border_rects = window_border_rects;
        } else if draw_above_fullscreen_this_node {
            above_fullscreen_border_rects.extend(window_border_rects);
        } else if draw_top_this_node {
            resized_border_rects.extend(window_border_rects);
        } else {
            stack_window_units
                .entry(node_id)
                .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                .border_rects = window_border_rects;
        }
        if let Some(pin_badge) = window_pin_badge {
            if stack_render_set.contains(&node_id) {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| {
                        StackWindowDrawUnit::new(
                            node_id,
                            stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                        )
                    })
                    .pin_badges
                    .push(pin_badge);
            } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
                above_fullscreen_stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .pin_badges
                    .push(pin_badge);
            } else if overlap_policy_stack_this_node {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .pin_badges
                    .push(pin_badge);
            } else if draw_above_fullscreen_this_node || draw_top_this_node {
                pin_badges.push(pin_badge);
            } else {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .pin_badges
                    .push(pin_badge);
            }
        }
        // Games/fullscreen processes bypass offscreen zoom for performance and compatibility.
        let use_offscreen_zoom = !fullscreen_on_current_monitor && !live_surface_node;

        if use_offscreen_zoom {
            let spawn_pan_pending = st
                .model
                .spawn_state
                .active_spawn_pan
                .is_some_and(|active| active.node_id == node_id)
                || st
                    .model
                    .spawn_state
                    .pending_spawn_pan_queue
                    .iter()
                    .any(|pending| pending.node_id == node_id)
                || st
                    .model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .contains_key(&node_id)
                || st
                    .model
                    .spawn_state
                    .pending_initial_reveal
                    .contains(&node_id);
            let open_anim_active = anim.scale < 0.999 || anim.alpha < 0.999;
            let defer_offscreen_rebuild = tiling_tile_transition.is_some()
                || stack_transition_pose.is_some()
                || spawn_pan_pending
                || open_anim_active;
            let stale_cache_available = st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get(&node_id)
                .is_some_and(|cache| cache.texture.is_some() && cache.bbox.is_some());
            let cache_miss = if defer_offscreen_rebuild {
                !stale_cache_available
            } else {
                let cache = st.ui.render_state.ensure_window_offscreen_cache(
                    node_id,
                    bbox.size.w,
                    bbox.size.h,
                    now,
                );
                cache.dirty || cache.texture.is_none() || cache.bbox.is_none()
            };

            if cache_miss {
                if defer_offscreen_rebuild {
                    if tiling_tile_transition.is_some() && !stale_cache_available {
                        continue;
                    }
                    log_window_render_path(
                        st,
                        node_id,
                        "direct-surface-transition",
                        &format!(
                            "texture_rect=({},{} {}x{}) geo_rect=({},{} {}x{})",
                            texture_rect.0,
                            texture_rect.1,
                            texture_rect.2,
                            texture_rect.3,
                            gx,
                            gy,
                            gw.max(1),
                            gh.max(1)
                        ),
                    );
                    let elems = render_elements_from_surface_tree(
                        renderer,
                        &wl,
                        (sx, sy),
                        element_scale as f64,
                        alpha,
                        Kind::Unspecified,
                    );
                    let (tx, ty, tw, th) = if exact_fullscreen_output {
                        (
                            output_clip.loc.x,
                            output_clip.loc.y,
                            output_clip.size.w,
                            output_clip.size.h,
                        )
                    } else {
                        texture_rect
                    };
                    let display_clip = Rectangle::<i32, Physical>::new(
                        (tx, ty).into(),
                        (tw.max(1), th.max(1)).into(),
                    );

                    let (clip_gx, clip_gy, clip_gw, clip_gh) =
                        rect_from_local_geometry(sx, sy, element_scale, clip_geo);
                    let geo_clip_rect = Rectangle::<i32, Physical>::new(
                        (clip_gx, clip_gy).into(),
                        (clip_gw.max(1), clip_gh.max(1)).into(),
                    );
                    let cropped = wrap_direct_surface_elements(
                        elems,
                        display_clip,
                        st.ui.render_state.gpu.surface_clip_program.as_ref(),
                        geo_clip_rect,
                        decoration_metrics.content_corner_radius_px as f32,
                    );

                    if stack_render_set.contains(&node_id) {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(
                                    node_id,
                                    stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                                )
                            })
                            .active_elements
                            .extend(cropped);
                    } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
                        above_fullscreen_stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                            })
                            .active_elements
                            .extend(cropped);
                    } else if overlap_policy_stack_this_node {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                            })
                            .active_elements
                            .extend(cropped);
                    } else if draw_above_fullscreen_this_node {
                        above_fullscreen_active_elements.extend(cropped);
                    } else if draw_top_this_node {
                        resized_active_elements.extend(cropped);
                    } else {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                            })
                            .active_elements
                            .extend(cropped);
                    }
                    continue;
                }

                match render_surface_tree_to_texture(renderer, &wl, 1.0, offscreen_clip.clone()) {
                    Ok(offscreen) => {
                        log_window_render_path(
                            st,
                            node_id,
                            "offscreen-rebuild-ok",
                            &format!(
                                "bbox=({},{} {}x{})",
                                offscreen.bbox.loc.x,
                                offscreen.bbox.loc.y,
                                offscreen.bbox.size.w,
                                offscreen.bbox.size.h
                            ),
                        );
                        let cache = st
                            .ui
                            .render_state
                            .cache
                            .window_offscreen_cache
                            .get_mut(&node_id)
                            .expect("offscreen cache should exist after ensure");
                        cache.texture = Some(offscreen.texture);
                        cache.bbox = Some(offscreen.bbox);
                        cache.has_content = offscreen.has_content;
                        cache.mark_clean(now);
                    }
                    Err(err) => {
                        let can_use_stale_cache = st
                            .ui
                            .render_state
                            .cache
                            .window_offscreen_cache
                            .get(&node_id)
                            .is_some_and(|cache| cache.texture.is_some() && cache.bbox.is_some());
                        log_window_render_path(
                            st,
                            node_id,
                            "offscreen-rebuild-failed",
                            &format!("stale_cache={} err={}", can_use_stale_cache, err),
                        );
                        if !can_use_stale_cache {
                            log_window_render_path(
                                st,
                                node_id,
                                "direct-surface-fallback",
                                &format!(
                                    "texture_rect=({},{} {}x{}) geo_rect=({},{} {}x{})",
                                    texture_rect.0,
                                    texture_rect.1,
                                    texture_rect.2,
                                    texture_rect.3,
                                    gx,
                                    gy,
                                    gw.max(1),
                                    gh.max(1)
                                ),
                            );
                            let elems = render_elements_from_surface_tree(
                                renderer,
                                &wl,
                                (sx, sy),
                                element_scale as f64,
                                alpha,
                                Kind::Unspecified,
                            );
                            let (tx, ty, tw, th) = if exact_fullscreen_output {
                                (
                                    output_clip.loc.x,
                                    output_clip.loc.y,
                                    output_clip.size.w,
                                    output_clip.size.h,
                                )
                            } else {
                                texture_rect
                            };
                            let display_clip = Rectangle::<i32, Physical>::new(
                                (tx, ty).into(),
                                (tw.max(1), th.max(1)).into(),
                            );

                            let (clip_gx, clip_gy, clip_gw, clip_gh) =
                                rect_from_local_geometry(sx, sy, element_scale, clip_geo);
                            let geo_clip_rect = Rectangle::<i32, Physical>::new(
                                (clip_gx, clip_gy).into(),
                                (clip_gw.max(1), clip_gh.max(1)).into(),
                            );
                            let cropped = wrap_direct_surface_elements(
                                elems,
                                display_clip,
                                st.ui.render_state.gpu.surface_clip_program.as_ref(),
                                geo_clip_rect,
                                decoration_metrics.content_corner_radius_px as f32,
                            );

                            if stack_render_set.contains(&node_id) {
                                stack_window_units
                                    .entry(node_id)
                                    .or_insert_with(|| {
                                        StackWindowDrawUnit::new(
                                            node_id,
                                            stack_draw_orders
                                                .get(&node_id)
                                                .copied()
                                                .unwrap_or_default(),
                                        )
                                    })
                                    .active_elements
                                    .extend(cropped);
                            } else if overlap_policy_stack_this_node
                                && draw_above_fullscreen_this_node
                            {
                                above_fullscreen_stack_window_units
                                    .entry(node_id)
                                    .or_insert_with(|| {
                                        StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                                    })
                                    .active_elements
                                    .extend(cropped);
                            } else if overlap_policy_stack_this_node {
                                stack_window_units
                                    .entry(node_id)
                                    .or_insert_with(|| {
                                        StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                                    })
                                    .active_elements
                                    .extend(cropped);
                            } else if draw_above_fullscreen_this_node {
                                above_fullscreen_active_elements.extend(cropped);
                            } else if draw_top_this_node {
                                resized_active_elements.extend(cropped);
                            } else {
                                stack_window_units
                                    .entry(node_id)
                                    .or_insert_with(|| {
                                        StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                                    })
                                    .active_elements
                                    .extend(cropped);
                            }
                            continue;
                        }
                    }
                }
            }

            if let Some(cache) = st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get_mut(&node_id)
            {
                cache.touch(now);
            }

            match st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get(&node_id)
                .map(|cache| (cache.texture.clone(), cache.bbox, cache.has_content))
            {
                Some((texture, bbox, _has_content)) => {
                    let Some(texture) = texture else {
                        continue;
                    };
                    let Some(ob) = bbox else {
                        continue;
                    };
                    // src = full bbox, dst = bbox scaled to screen positioned so geo
                    // lands on frame, clip = frame rect to discard cached visual bleed.
                    let (
                        src_x,
                        src_y,
                        src_w,
                        src_h,
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        clip_x,
                        clip_y,
                        clip_w,
                        clip_h,
                    ) = if let Some(active_resize) = active_resize {
                        // Use live committed geo (updated on every client commit)
                        // as the single source of truth. Falls back to frozen
                        // local_geo before the first commit after resize starts.
                        let (live_gw, live_gh): (f32, f32) = if active_resize.live_geo_w > 0.0 {
                            (active_resize.live_geo_w, active_resize.live_geo_h)
                        } else {
                            // Before first commit: keep the frozen start size.
                            (local_geo.2, local_geo.3)
                        };
                        let frozen_geo_lx =
                            (ob.loc.x + resize_preview.unwrap().start_geo_inset_x) as f32;
                        let frozen_geo_ly =
                            (ob.loc.y + resize_preview.unwrap().start_geo_inset_y) as f32;

                        // Match the normal offscreen path: anchor the destination from the
                        // live geometry rect itself, then let the visual crop helper expand it.
                        // Clipping only to the preview frame was shaving off the recovered edge
                        // margin during resize, which made the resize look slightly tighter than
                        // the steady-state path.
                        // Keep resize anchored to the same screen-space rect the frame/background
                        // uses. At non-1.0 zoom, deriving the destination from surface_origin_px
                        // plus live local geometry can drift by a pixel or two from the preview/frame
                        // rect because those values round in different spaces. Using the frame/top-left
                        // anchor directly keeps the texture locked to its background while still sizing
                        // from the live committed geometry.
                        let _live_gw_px = (live_gw * cam_scale).round().max(1.0) as i32;
                        let _live_gh_px = (live_gh * cam_scale).round().max(1.0) as i32;
                        let preview_gw_px = gw.max(1);
                        let preview_gh_px = gh.max(1);

                        offscreen_visual_crop_and_dst(
                            ob.loc.x,
                            ob.loc.y,
                            ob.size.w.max(1),
                            ob.size.h.max(1),
                            frozen_geo_lx,
                            frozen_geo_ly,
                            live_gw,
                            live_gh,
                            gx,
                            gy,
                            preview_gw_px,
                            preview_gh_px,
                            render_scale,
                            output_clip,
                            preserve_visual_margin,
                            lock_dst_to_geometry,
                        )
                    } else {
                        offscreen_visual_crop_and_dst(
                            ob.loc.x,
                            ob.loc.y,
                            ob.size.w.max(1),
                            ob.size.h.max(1),
                            local_geo.0,
                            local_geo.1,
                            local_geo.2,
                            local_geo.3,
                            gx,
                            gy,
                            gw.max(1),
                            gh.max(1),
                            render_scale,
                            output_clip,
                            preserve_visual_margin,
                            lock_dst_to_geometry,
                        )
                    };
                    log_window_render_path(
                        st,
                        node_id,
                        "offscreen-compose",
                        &format!(
                            "cache_bbox={} local_bbox={} local_geo={} texture_rect={} geometry_rect={} src={} dst={} clip={} preserve_visual_margin={} lock_dst_to_geometry={} tex_size={}x{} radius_px={} border_px={}",
                            rect4_str(ob.loc.x, ob.loc.y, ob.size.w, ob.size.h),
                            rect4f_str(local_bbox.0, local_bbox.1, local_bbox.2, local_bbox.3),
                            rect4f_str(local_geo.0, local_geo.1, local_geo.2, local_geo.3),
                            rect4_str(
                                texture_rect.0,
                                texture_rect.1,
                                texture_rect.2,
                                texture_rect.3
                            ),
                            rect4_str(gx, gy, gw.max(1), gh.max(1)),
                            rect4f_str(src_x as f32, src_y as f32, src_w as f32, src_h as f32),
                            rect4_str(dst_x, dst_y, dst_w, dst_h),
                            rect4_str(clip_x, clip_y, clip_w, clip_h),
                            preserve_visual_margin,
                            lock_dst_to_geometry,
                            texture.size().w,
                            texture.size().h,
                            decoration_metrics.content_corner_radius_px,
                            decoration_metrics.primary_border_px,
                        ),
                    );

                    // Compute the geometry rect in dst-local pixel space so the
                    // shader can clip window content to it when the bbox is
                    // larger than the true geometry rect.
                    // ob = cached bbox in logical surface space (origin at (0,0)).
                    // geo_lx/ly are the geometry origin inside that bbox.
                    // dst maps the bbox to screen: dst_x..dst_x+dst_w covers ob.
                    let (geo_offset_x, geo_offset_y, geo_w_px, geo_h_px) = if lock_dst_to_geometry {
                        // The destination already matches the exact window geometry rect,
                        // so a second geometry clip inside that dst only introduces rounding
                        // drift at zoomed scales.
                        (0.0, 0.0, dst_w.max(1) as f32, dst_h.max(1) as f32)
                    } else {
                        let src_scale_x = if src_w > 0.0 {
                            dst_w as f32 / src_w as f32
                        } else {
                            1.0
                        };
                        let src_scale_y = if src_h > 0.0 {
                            dst_h as f32 / src_h as f32
                        } else {
                            1.0
                        };
                        // local_geo is (geo_lx, geo_ly, geo_w, geo_h) in surface-local logical px.
                        // In bbox-local space the geo origin is (geo_lx - ob.loc.x, geo_ly - ob.loc.y).
                        let geo_local_x = local_geo.0 - ob.loc.x as f32;
                        let geo_local_y = local_geo.1 - ob.loc.y as f32;
                        let geo_src_x = (geo_local_x - src_x as f32).max(0.0);
                        let geo_src_y = (geo_local_y - src_y as f32).max(0.0);
                        (
                            (geo_src_x * src_scale_x).max(0.0),
                            (geo_src_y * src_scale_y).max(0.0),
                            (local_geo.2 * src_scale_x).min(dst_w as f32).max(1.0),
                            (local_geo.3 * src_scale_y).min(dst_h as f32).max(1.0),
                        )
                    };

                    let offscreen = OffscreenNodeTexture {
                        texture,
                        alpha,
                        corner_radius: decoration_metrics.content_corner_radius_px as f32,
                        src_x,
                        src_y,
                        src_w,
                        src_h,
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        clip_x,
                        clip_y,
                        clip_w,
                        clip_h,
                        geo_offset_x,
                        geo_offset_y,
                        geo_w: geo_w_px,
                        geo_h: geo_h_px,
                    };
                    if stack_render_set.contains(&node_id) {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(
                                    node_id,
                                    stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                                )
                            })
                            .offscreen_textures
                            .push(offscreen);
                    } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
                        above_fullscreen_stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                            })
                            .offscreen_textures
                            .push(offscreen);
                    } else if overlap_policy_stack_this_node {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                            })
                            .offscreen_textures
                            .push(offscreen);
                    } else if draw_above_fullscreen_this_node {
                        above_fullscreen_offscreen_textures.push(offscreen);
                    } else if draw_top_this_node {
                        resized_offscreen_textures.push(offscreen);
                    } else {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(node_id, overlap_policy_draw_order)
                            })
                            .offscreen_textures
                            .push(offscreen);
                    }
                }
                None => {
                    continue;
                }
            }
        } else {
            log_window_render_path(
                st,
                node_id,
                "direct-surface-no-offscreen",
                &format!(
                    "texture_rect=({},{} {}x{}) geo_rect=({},{} {}x{})",
                    texture_rect.0,
                    texture_rect.1,
                    texture_rect.2,
                    texture_rect.3,
                    gx,
                    gy,
                    gw.max(1),
                    gh.max(1)
                ),
            );
            let elems = render_elements_from_surface_tree(
                renderer,
                &wl,
                (sx, sy),
                element_scale as f64,
                alpha,
                Kind::Unspecified,
            );
            let (tx, ty, tw, th) = if exact_fullscreen_output {
                (
                    output_clip.loc.x,
                    output_clip.loc.y,
                    output_clip.size.w,
                    output_clip.size.h,
                )
            } else {
                texture_rect
            };
            let display_clip =
                Rectangle::<i32, Physical>::new((tx, ty).into(), (tw.max(1), th.max(1)).into());
            let (clip_gx, clip_gy, clip_gw, clip_gh) =
                rect_from_local_geometry(sx, sy, element_scale, clip_geo);
            let geo_clip_rect = Rectangle::<i32, Physical>::new(
                (clip_gx, clip_gy).into(),
                (clip_gw.max(1), clip_gh.max(1)).into(),
            );
            let cropped = wrap_direct_surface_elements(
                elems,
                display_clip,
                st.ui.render_state.gpu.surface_clip_program.as_ref(),
                geo_clip_rect,
                decoration_metrics.content_corner_radius_px as f32,
            );

            if stack_render_set.contains(&node_id) {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| {
                        StackWindowDrawUnit::new(
                            node_id,
                            stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                        )
                    })
                    .active_elements
                    .extend(cropped);
            } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
                above_fullscreen_stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .active_elements
                    .extend(cropped);
            } else if overlap_policy_stack_this_node {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .active_elements
                    .extend(cropped);
            } else if draw_above_fullscreen_this_node {
                above_fullscreen_active_elements.extend(cropped);
            } else if draw_top_this_node {
                resized_active_elements.extend(cropped);
            } else {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| StackWindowDrawUnit::new(node_id, overlap_policy_draw_order))
                    .active_elements
                    .extend(cropped);
            }
        }

        let parent_geo = window_geometry_for_node(st, node_id).unwrap_or((
            0.0,
            0.0,
            node_intrinsic.x.max(1.0),
            node_intrinsic.y.max(1.0),
        ));
        let parent_geo_loc = (parent_geo.0.round() as i32, parent_geo.1.round() as i32);
        let mut popup_cropped = Vec::new();
        let mut popups: Vec<_> = PopupManager::popups_for_surface(&wl).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let popup_sx = sx
                + ((parent_geo_loc.0 + popup_offset.x - popup_geo.loc.x) as f32 * element_scale)
                    .round() as i32;
            let popup_sy = sy
                + ((parent_geo_loc.1 + popup_offset.y - popup_geo.loc.y) as f32 * element_scale)
                    .round() as i32;
            if use_offscreen_zoom {
                match render_surface_tree_to_texture(renderer, popup.wl_surface(), alpha, None) {
                    Ok(offscreen) => {
                        let src_x = 0.0f64;
                        let src_y = 0.0f64;
                        let src_w = offscreen.bbox.size.w.max(1) as f64;
                        let src_h = offscreen.bbox.size.h.max(1) as f64;
                        let dst_x =
                            popup_sx + (offscreen.bbox.loc.x as f32 * element_scale).round() as i32;
                        let dst_y =
                            popup_sy + (offscreen.bbox.loc.y as f32 * element_scale).round() as i32;
                        let dst_w = (offscreen.bbox.size.w as f32 * element_scale)
                            .round()
                            .max(1.0) as i32;
                        let dst_h = (offscreen.bbox.size.h as f32 * element_scale)
                            .round()
                            .max(1.0) as i32;
                        let offscreen_texture = OffscreenNodeTexture {
                            texture: offscreen.texture,
                            alpha,
                            corner_radius: 0.0,
                            src_x,
                            src_y,
                            src_w,
                            src_h,
                            dst_x,
                            dst_y,
                            dst_w,
                            dst_h,
                            clip_x: output_clip.loc.x,
                            clip_y: output_clip.loc.y,
                            clip_w: output_clip.size.w,
                            clip_h: output_clip.size.h,
                            // Popups have no border rounding, geo == full dst.
                            geo_offset_x: 0.0,
                            geo_offset_y: 0.0,
                            geo_w: 0.0,
                            geo_h: 0.0,
                        };
                        if draw_above_fullscreen_this_node {
                            above_fullscreen_popup_offscreen_textures.push(offscreen_texture);
                        } else {
                            popup_offscreen_textures.push(offscreen_texture);
                        }
                    }
                    Err(_) => {
                        let popup_elems: Vec<SurfaceElement> = render_elements_from_surface_tree(
                            renderer,
                            popup.wl_surface(),
                            (popup_sx, popup_sy),
                            element_scale as f64,
                            alpha,
                            Kind::Unspecified,
                        );
                        popup_cropped.extend(
                            popup_elems.into_iter().filter_map(|e| {
                                CropRenderElement::from_element(e, 1.0, output_clip)
                            }),
                        );
                    }
                }
            } else {
                let popup_elems: Vec<SurfaceElement> = render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    (popup_sx, popup_sy),
                    element_scale as f64,
                    alpha,
                    Kind::Unspecified,
                );
                popup_cropped.extend(
                    popup_elems
                        .into_iter()
                        .filter_map(|e| CropRenderElement::from_element(e, 1.0, output_clip)),
                );
            }
        }

        if draw_above_fullscreen_this_node {
            above_fullscreen_popup_elements.extend(popup_cropped);
        } else {
            popup_elements.extend(popup_cropped);
        }
    }

    let mut stack_window_units = stack_window_units.into_values().collect::<Vec<_>>();
    if let Some(plan) = stack_transition_plan.as_ref() {
        for extra in &plan.extra_instances {
            let Some(from_pose) = plan.poses.get(&extra.node_id).copied() else {
                continue;
            };
            let Some(unit) = stack_window_units
                .iter()
                .find(|unit| unit.node_id == extra.node_id)
            else {
                continue;
            };
            if let Some(extra_unit) =
                clone_stack_window_unit_for_pose(st, size, unit, from_pose, extra.pose)
            {
                stack_window_units.push(extra_unit);
            }
        }
    }
    stack_window_units.sort_by_key(|unit| (unit.draw_order, unit.node_id.as_u64()));
    let mut above_fullscreen_stack_window_units = above_fullscreen_stack_window_units
        .into_values()
        .collect::<Vec<_>>();
    above_fullscreen_stack_window_units
        .sort_by_key(|unit| (unit.draw_order, unit.node_id.as_u64()));

    (
        active_elements,
        resized_active_elements,
        fullscreen_active_elements,
        above_fullscreen_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        fullscreen_offscreen_textures,
        above_fullscreen_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        fullscreen_popup_offscreen_textures,
        fullscreen_popup_elements,
        above_fullscreen_popup_offscreen_textures,
        above_fullscreen_popup_elements,
        node_surface_map,
        stack_window_units,
        above_fullscreen_stack_window_units,
        shadow_rects,
        resized_shadow_rects,
        above_fullscreen_shadow_rects,
        border_rects,
        resized_border_rects,
        above_fullscreen_border_rects,
        pin_badges,
    )
}

#[cfg(test)]
mod tests {
    use super::{active_surface_draw_rank, window_decoration_metrics, world_to_screen_for_view};
    use crate::compositor::root::Halley;
    use crate::compositor::surface::stacking_render_order_map;
    use halley_core::field::NodeId;
    use halley_core::field::Vec2;
    use std::time::Instant;

    #[test]
    fn stacking_render_order_keeps_front_card_last() {
        let members = vec![
            NodeId::new(1),
            NodeId::new(2),
            NodeId::new(3),
            NodeId::new(4),
        ];
        let ranks = stacking_render_order_map(&members, 3);

        assert_eq!(ranks.get(&NodeId::new(1)), Some(&2));
        assert_eq!(ranks.get(&NodeId::new(2)), Some(&1));
        assert_eq!(ranks.get(&NodeId::new(3)), Some(&0));
        assert_eq!(ranks.get(&NodeId::new(4)), None);
    }

    #[test]
    fn active_surface_draw_rank_uses_raise_then_newest_node() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let old = NodeId::new(10);
        let raised = NodeId::new(2);
        let newest = NodeId::new(11);
        state
            .model
            .focus_state
            .overlap_raise_order
            .insert(raised, 2);

        let mut ids = vec![newest, old, raised];
        ids.sort_by_key(|&id| active_surface_draw_rank(&state, id));

        assert_eq!(ids, vec![old, newest, raised]);
    }

    #[test]
    fn active_surface_draw_rank_keeps_cluster_float_above_layout_member() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        let mut state = Halley::new_for_test(&dh, tuning);
        let layout_a = state.model.field.spawn_surface(
            "layout-a",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let layout_b = state.model.field.spawn_surface(
            "layout-b",
            Vec2 { x: 360.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let floating = state.model.field.spawn_surface(
            "floating",
            Vec2 { x: 180.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [layout_a, layout_b, floating] {
            state.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = state
            .create_cluster(vec![layout_a, layout_b])
            .expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));
        state.model.spawn_state.applied_window_rules.insert(
            floating,
            crate::compositor::spawn::state::AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::None,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                parent_node: None,
                suppress_reveal_pan: true,
                builtin_rule: None,
            },
        );

        assert!(
            active_surface_draw_rank(&state, floating) > active_surface_draw_rank(&state, layout_a)
        );
    }

    #[test]
    fn window_decoration_metrics_treat_radius_as_content_edge_radius() {
        let metrics = window_decoration_metrics(20, 3, 2, 1);

        assert_eq!(metrics.content_corner_radius_px, 20);
        assert_eq!(metrics.primary_outer_corner_radius_px, 23);
        assert_eq!(metrics.secondary_inner_corner_radius_px, 25);
        assert_eq!(metrics.secondary_outer_corner_radius_px, 26);
    }

    #[test]
    fn zero_radius_keeps_all_decoration_corners_square() {
        let metrics = window_decoration_metrics(0, 3, 0, 3);

        assert_eq!(metrics.content_corner_radius_px, 0);
        assert_eq!(metrics.primary_outer_corner_radius_px, 0);
        assert_eq!(metrics.secondary_inner_corner_radius_px, 0);
        assert_eq!(metrics.secondary_outer_corner_radius_px, 0);
    }

    #[test]
    fn world_to_screen_for_view_uses_supplied_monitor_camera() {
        let center = Vec2 { x: 500.0, y: 200.0 };
        let view_size = Vec2 { x: 400.0, y: 200.0 };

        assert_eq!(
            world_to_screen_for_view(center, view_size, 1920, 1080, 500.0, 200.0),
            (960, 540)
        );
        assert_eq!(
            world_to_screen_for_view(center, view_size, 1920, 1080, 300.0, 100.0),
            (0, 0)
        );
    }
}
