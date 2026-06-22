use super::*;
use crate::window::stack::{build_stack_transition_plan, stack_draw_order_map};

pub(super) struct StackRenderLayout {
    pub(super) render_set: HashSet<NodeId>,
    pub(super) draw_orders: HashMap<NodeId, i32>,
    pub(super) transition_plan: Option<StackTransitionPlan>,
}

pub(super) struct WindowRenderLayout {
    pub(super) stack_transition_pose: Option<StackTransitionPose>,
    pub(super) fullscreen_on_current_monitor: bool,
    pub(super) exact_fullscreen_output: bool,
    pub(super) tiling_tile_transition: Option<crate::animation::ClusterTileAnimRect>,
    pub(super) active_resize: Option<crate::input::ActiveResizeGeometryScreen>,
    pub(super) render_route: WindowRenderRoute,
    pub(super) live_surface_node: bool,
    pub(super) raise_shadow_boost: f32,
    pub(super) cam_scale: f32,
    pub(super) local_bbox: (f32, f32, f32, f32),
    pub(super) local_geo: (f32, f32, f32, f32),
    pub(super) render_scale: f32,
    pub(super) sx: i32,
    pub(super) sy: i32,
    pub(super) texture_rect: (i32, i32, i32, i32),
    pub(super) geometry_rect: (i32, i32, i32, i32),
    pub(super) element_scale: f32,
    pub(super) fullscreen_like_for_render: bool,
    pub(super) open_anim_active: bool,
    pub(super) rule_opacity: f32,
    pub(super) animation_alpha: f32,
    pub(super) alpha: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WindowRenderRoute {
    Stack { draw_order: i32 },
    AboveFullscreenStack { draw_order: i32 },
    AboveFullscreen,
    Top,
}

impl WindowRenderRoute {
    pub(super) fn popups_above_fullscreen(self) -> bool {
        matches!(
            self,
            WindowRenderRoute::AboveFullscreen | WindowRenderRoute::AboveFullscreenStack { .. }
        )
    }
}

pub(super) fn build_stack_render_layout(
    st: &mut Halley,
    current_monitor: &str,
    now: Instant,
) -> StackRenderLayout {
    let stack_visible_front_to_back =
        active_stacking_visible_members_for_monitor(st, current_monitor);
    let stack_cycle_transition = st
        .runtime
        .tuning
        .stack_animation_enabled()
        .then(|| {
            st.ui
                .render_state
                .stack_cycle_transition_for_monitor(current_monitor, now)
        })
        .flatten();
    if stack_cycle_transition.is_some() {
        st.request_maintenance();
    }
    let transition_plan = stack_cycle_transition
        .as_ref()
        .and_then(|transition| build_stack_transition_plan(st, current_monitor, transition));
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
    let render_set = stack_render_front_to_back.iter().copied().collect();
    let draw_orders = if let Some(plan) = transition_plan.as_ref() {
        plan.poses
            .iter()
            .map(|(&node_id, pose)| (node_id, pose.draw_order))
            .collect::<HashMap<_, _>>()
    } else {
        stack_draw_order_map(&stack_visible_front_to_back)
    };

    StackRenderLayout {
        render_set,
        draw_orders,
        transition_plan,
    }
}

pub(super) fn resolve_window_render_layout(
    st: &mut Halley,
    node_id: NodeId,
    bbox: Rectangle<i32, Logical>,
    output_size: Size<i32, Physical>,
    output_clip: Rectangle<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    stack_layout: &StackRenderLayout,
    now: Instant,
) -> Option<WindowRenderLayout> {
    let node = st.model.field.node(node_id)?;
    let stack_transition_pose = stack_layout
        .transition_plan
        .as_ref()
        .and_then(|plan| plan.poses.get(&node_id).copied());
    let stack_member_rendered = stack_layout.render_set.contains(&node_id);
    // A node taking part in the stack-cycle transition keeps rendering even after
    // the relayout has already moved it out of the visible set (hidden / no longer
    // Active) — otherwise the outgoing top of a forward cycle vanishes instead of
    // flying out to the left. The transition pose drives its geometry/alpha.
    let in_stack_transition = stack_transition_pose.is_some();
    if !st.node_assigned_to_current_monitor(node_id)
        || (!in_stack_transition
            && (node.state != halley_core::field::NodeState::Active
                || !st.model.field.is_visible(node_id)))
    {
        return None;
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
            st.ui.render_state.cluster_tile_tracks(),
            node_id,
            now,
        )
    })
    .flatten();
    let frozen_tiling_geometry = tiling_tile_transition
        .and_then(|_| st.ui.render_state.cluster_tile_frozen_geometry(node_id));
    let transition_alpha =
        crate::compositor::workspace::state::active_transition_alpha(st, node_id, now);
    let anim = crate::frame_loop::anim_style_for(st, node_id, node_state, now);
    let fullscreen_entry_scale =
        crate::compositor::fullscreen::system::fullscreen_entry_scale(st, node_id, st.now_ms(now));
    let active_resize = active_resize_geometry_screen(st, node_id, resize_preview);
    let resizing_this_node = active_resize.is_some();
    let persistent_rule_top = is_persistent_rule_top(st, node_id);
    let overlap_policy_stack_this_node =
        crate::compositor::spawn::state::node_has_overlap_policy(st, node_id);
    let draw_top_this_node = resizing_this_node
        || dragging_this_node
        || (persistent_rule_top && !overlap_policy_stack_this_node);
    let draw_above_fullscreen_this_node =
        st.node_draws_above_fullscreen_on_current_monitor(node_id);
    let overlap_policy_draw_order = overlap_policy_draw_order(st, node_id);
    let render_route = if stack_member_rendered {
        WindowRenderRoute::Stack {
            draw_order: stack_layout
                .draw_orders
                .get(&node_id)
                .copied()
                .unwrap_or_default(),
        }
    } else if overlap_policy_stack_this_node && draw_above_fullscreen_this_node {
        WindowRenderRoute::AboveFullscreenStack {
            draw_order: overlap_policy_draw_order,
        }
    } else if overlap_policy_stack_this_node {
        WindowRenderRoute::Stack {
            draw_order: overlap_policy_draw_order,
        }
    } else if draw_above_fullscreen_this_node {
        WindowRenderRoute::AboveFullscreen
    } else if draw_top_this_node {
        WindowRenderRoute::Top
    } else {
        WindowRenderRoute::Stack {
            draw_order: overlap_policy_draw_order,
        }
    };
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

    let fit_scale = if fullscreen_on_current_monitor {
        1.0
    } else if let Some(monitor) = st.fullscreen_monitor_for_node(node_id) {
        let (target_w, target_h) = st.fullscreen_target_size_for(monitor);
        let sw = (target_w as f32) / node_intrinsic.x.max(1.0);
        let sh = (target_h as f32) / node_intrinsic.y.max(1.0);
        sw.min(sh).max(0.1)
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
    let base_p = stack_transition_pose
        .map(|pose| pose.center)
        .or_else(|| tiling_tile_transition.map(|rect| rect.center))
        .or_else(|| fullscreen_visual.map(|(center, _)| center))
        .or_else(|| maximized_visual.map(|(center, _)| center))
        .unwrap_or(node_pos);
    let p = base_p;
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

    let (_cx, _cy, sx, sy, texture_rect, geometry_rect) = if let Some(active_resize) = active_resize
    {
        let (cx, cy) = active_resize.center_px();
        let (surface_origin_x, surface_origin_y) = active_resize.surface_origin_px();
        let frame = active_resize.frame_rect_px();
        (cx, cy, surface_origin_x, surface_origin_y, frame, frame)
    } else {
        let (cx, cy) = world_to_screen(st, output_size.w, output_size.h, p.x, p.y);

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

    let rule_opacity = node_rule_opacity(st, node_id);
    let open_anim_active = anim.scale < 0.999 || anim.alpha < 0.999;
    let animation_alpha = (anim.alpha
        * live_ramp
        * stack_transition_pose.map(|pose| pose.alpha).unwrap_or(1.0)
        * tiling_tile_transition.map(|rect| rect.alpha).unwrap_or(1.0))
    .clamp(0.0, 1.0);
    let alpha = (animation_alpha * rule_opacity).clamp(0.0, 1.0);

    Some(WindowRenderLayout {
        stack_transition_pose,
        fullscreen_on_current_monitor,
        exact_fullscreen_output,
        tiling_tile_transition,
        active_resize,
        render_route,
        live_surface_node,
        raise_shadow_boost,
        cam_scale,
        local_bbox,
        local_geo,
        render_scale,
        sx,
        sy,
        texture_rect,
        geometry_rect,
        element_scale,
        fullscreen_like_for_render,
        open_anim_active,
        rule_opacity,
        animation_alpha,
        alpha,
    })
}
