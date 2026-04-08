use std::time::{Duration, Instant};

use crate::animation::active_surface_render_scale;
use crate::backend::interface::BackendView;
use crate::compositor::interaction::{HitNode, PointerState, ResizeCtx, ResizeHandle};
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::{
    active_stacking_render_order_for_monitor, current_surface_size_for_node,
    node_allows_interactive_resize, request_toplevel_resize_mode, toplevel_min_size_for_node,
    window_geometry_for_node,
};
use crate::render::active_window_frame_pad_px;
use crate::render::world_to_screen;
use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::button::ButtonFrame;

#[inline]
fn resize_rect_nearly_eq(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.5
}

// `duration-ms` is interpreted as approximate catch-up / settle time for the
// detached resize preview, not as a restartable tween length.
fn resize_smoothing_alpha(st: &Halley, dt: Duration) -> f32 {
    if !st.runtime.tuning.smooth_resize_enabled() {
        return 1.0;
    }

    let dt_secs = dt.as_secs_f32().clamp(0.0, 0.25);
    if dt_secs <= f32::EPSILON {
        return 0.0;
    }
    let duration_secs = (st.runtime.tuning.smooth_resize_duration_ms().max(1) as f32) / 1000.0;
    (1.0 - 0.1f32.powf(dt_secs / duration_secs.max(0.001))).clamp(0.0, 1.0)
}

fn snap_resize_preview_edges(resize: &mut ResizeCtx) {
    if resize_rect_nearly_eq(resize.preview_left_px, resize.target_left_px) {
        resize.preview_left_px = resize.target_left_px;
    }
    if resize_rect_nearly_eq(resize.preview_right_px, resize.target_right_px) {
        resize.preview_right_px = resize.target_right_px;
    }
    if resize_rect_nearly_eq(resize.preview_top_px, resize.target_top_px) {
        resize.preview_top_px = resize.target_top_px;
    }
    if resize_rect_nearly_eq(resize.preview_bottom_px, resize.target_bottom_px) {
        resize.preview_bottom_px = resize.target_bottom_px;
    }
}

fn resize_preview_settled(resize: &ResizeCtx) -> bool {
    resize_rect_nearly_eq(resize.preview_left_px, resize.target_left_px)
        && resize_rect_nearly_eq(resize.preview_right_px, resize.target_right_px)
        && resize_rect_nearly_eq(resize.preview_top_px, resize.target_top_px)
        && resize_rect_nearly_eq(resize.preview_bottom_px, resize.target_bottom_px)
}

fn resize_settle_velocity_done(resize: &ResizeCtx) -> bool {
    resize.preview_velocity_left_pxps.abs() <= 8.0
        && resize.preview_velocity_right_pxps.abs() <= 8.0
        && resize.preview_velocity_top_pxps.abs() <= 8.0
        && resize.preview_velocity_bottom_pxps.abs() <= 8.0
}

fn advance_resize_preview_toward_target(st: &Halley, resize: &mut ResizeCtx, now: Instant) {
    if !st.runtime.tuning.smooth_resize_enabled() {
        resize.preview_left_px = resize.target_left_px;
        resize.preview_right_px = resize.target_right_px;
        resize.preview_top_px = resize.target_top_px;
        resize.preview_bottom_px = resize.target_bottom_px;
        resize.preview_velocity_left_pxps = 0.0;
        resize.preview_velocity_right_pxps = 0.0;
        resize.preview_velocity_top_pxps = 0.0;
        resize.preview_velocity_bottom_pxps = 0.0;
        resize.last_smooth_tick_at = now;
        return;
    }

    let dt = now.saturating_duration_since(resize.last_smooth_tick_at);
    resize.last_smooth_tick_at = now;
    let dt_secs = dt.as_secs_f32().clamp(0.0, 0.25);
    let alpha = resize_smoothing_alpha(st, dt);
    let prev_left = resize.preview_left_px;
    let prev_right = resize.preview_right_px;
    let prev_top = resize.preview_top_px;
    let prev_bottom = resize.preview_bottom_px;
    resize.preview_left_px += (resize.target_left_px - resize.preview_left_px) * alpha;
    resize.preview_right_px += (resize.target_right_px - resize.preview_right_px) * alpha;
    resize.preview_top_px += (resize.target_top_px - resize.preview_top_px) * alpha;
    resize.preview_bottom_px += (resize.target_bottom_px - resize.preview_bottom_px) * alpha;
    if dt_secs > f32::EPSILON {
        resize.preview_velocity_left_pxps = (resize.preview_left_px - prev_left) / dt_secs;
        resize.preview_velocity_right_pxps = (resize.preview_right_px - prev_right) / dt_secs;
        resize.preview_velocity_top_pxps = (resize.preview_top_px - prev_top) / dt_secs;
        resize.preview_velocity_bottom_pxps = (resize.preview_bottom_px - prev_bottom) / dt_secs;
    }
    snap_resize_preview_edges(resize);
}

fn advance_resize_preview_toward_stop(st: &Halley, resize: &mut ResizeCtx, now: Instant) {
    let dt = now.saturating_duration_since(resize.last_smooth_tick_at);
    resize.last_smooth_tick_at = now;
    let dt_secs = dt.as_secs_f32().clamp(0.0, 0.25);
    if dt_secs <= f32::EPSILON {
        return;
    }

    let duration_secs = (st.runtime.tuning.smooth_resize_duration_ms().max(1) as f32) / 1000.0;
    let decay = 0.01f32.powf(dt_secs / duration_secs.max(0.001));
    resize.preview_left_px += resize.preview_velocity_left_pxps * dt_secs;
    resize.preview_right_px += resize.preview_velocity_right_pxps * dt_secs;
    resize.preview_top_px += resize.preview_velocity_top_pxps * dt_secs;
    resize.preview_bottom_px += resize.preview_velocity_bottom_pxps * dt_secs;
    resize.preview_velocity_left_pxps *= decay;
    resize.preview_velocity_right_pxps *= decay;
    resize.preview_velocity_top_pxps *= decay;
    resize.preview_velocity_bottom_pxps *= decay;
    resize.target_left_px = resize.preview_left_px;
    resize.target_right_px = resize.preview_right_px;
    resize.target_top_px = resize.preview_top_px;
    resize.target_bottom_px = resize.preview_bottom_px;
    if resize_settle_velocity_done(resize) {
        resize.preview_velocity_left_pxps = 0.0;
        resize.preview_velocity_right_pxps = 0.0;
        resize.preview_velocity_top_pxps = 0.0;
        resize.preview_velocity_bottom_pxps = 0.0;
    }
}

fn apply_resize_preview_state(st: &mut Halley, resize: &mut ResizeCtx) -> bool {
    if resize_preview_settled(resize) {
        resize.preview_left_px = resize.target_left_px;
        resize.preview_right_px = resize.target_right_px;
        resize.preview_top_px = resize.target_top_px;
        resize.preview_bottom_px = resize.target_bottom_px;
    }

    let (min_lw, min_lh) = toplevel_min_size_for_node(st, resize.node_id);
    let cam_scale = st.camera_render_scale();
    let min_w = (min_lw as f32 * cam_scale).max(96.0);
    let min_h = (min_lh as f32 * cam_scale).max(72.0);
    let preview_visual_w = (resize.preview_right_px - resize.preview_left_px)
        .round()
        .max(min_w) as i32;
    let preview_visual_h = (resize.preview_bottom_px - resize.preview_top_px)
        .round()
        .max(min_h) as i32;
    let visual_delta_w = preview_visual_w - resize.start_visual_w;
    let visual_delta_h = preview_visual_h - resize.start_visual_h;
    let logical_delta_w = (visual_delta_w as f32 / cam_scale.max(0.001)).round() as i32;
    let logical_delta_h = (visual_delta_h as f32 / cam_scale.max(0.001)).round() as i32;
    let min_logical_w = (min_w / cam_scale.max(0.001)).round() as i32;
    let min_logical_h = (min_h / cam_scale.max(0.001)).round() as i32;
    let target_w = (resize.start_surface_w + logical_delta_w).max(min_logical_w);
    let target_h = (resize.start_surface_h + logical_delta_h).max(min_logical_h);

    if target_w != resize.last_sent_w || target_h != resize.last_sent_h {
        request_toplevel_resize_mode(st, resize.node_id, target_w, target_h, true);
        resize.last_sent_w = target_w;
        resize.last_sent_h = target_h;
    }

    st.input
        .interaction_state
        .physics_velocity
        .insert(resize.node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 });

    let center_sx = (resize.preview_left_px + resize.preview_right_px) * 0.5;
    let center_sy = (resize.preview_top_px + resize.preview_bottom_px) * 0.5;
    let center_world = crate::spatial::screen_to_world(
        st,
        resize.workspace_w,
        resize.workspace_h,
        center_sx,
        center_sy,
    );
    if let Some(n) = st.model.field.node_mut(resize.node_id) {
        n.pos = center_world;
    }
    let _ = st.model.field.set_resize_footprint(
        resize.node_id,
        Some(halley_core::field::Vec2 {
            x: target_w as f32,
            y: target_h as f32,
        }),
    );
    let _ = st
        .model
        .field
        .set_decay_level(resize.node_id, halley_core::decay::DecayLevel::Hot);

    resize_preview_settled(resize)
}

fn refresh_resize_preview_state(st: &mut Halley, resize: &mut ResizeCtx, now: Instant) -> bool {
    if resize.settling {
        advance_resize_preview_toward_stop(st, resize, now);
    } else {
        advance_resize_preview_toward_target(st, resize, now);
    }
    apply_resize_preview_state(st, resize)
}

fn finish_resize_interaction(
    st: &mut Halley,
    ps: &mut PointerState,
    resize: ResizeCtx,
    now: Instant,
) {
    ps.resize_trace_node = None;
    ps.resize_trace_until = None;
    ps.resize_trace_last_at = None;
    ps.preview_block_until = Some(now + Duration::from_millis(360));
    ps.resize = None;

    if !resize.drag_started {
        if resize.resize_mode_sent {
            request_toplevel_resize_mode(
                st,
                resize.node_id,
                resize.last_sent_w,
                resize.last_sent_h,
                false,
            );
        }
        st.set_recent_top_node(resize.node_id, now + Duration::from_millis(600));
        st.end_resize_interaction(now);
        st.resolve_overlap_now();
        return;
    }

    let (min_w, min_h) = toplevel_min_size_for_node(st, resize.node_id);
    let final_w = resize.last_sent_w.max(min_w).max(96);
    let final_h = resize.last_sent_h.max(min_h).max(72);
    let final_bbox_w =
        ((resize.start_bbox_w as f32) + ((final_w - resize.start_surface_w) as f32)).max(1.0);
    let final_bbox_h =
        ((resize.start_bbox_h as f32) + ((final_h - resize.start_surface_h) as f32)).max(1.0);
    request_toplevel_resize_mode(st, resize.node_id, final_w, final_h, true);
    request_toplevel_resize_mode(st, resize.node_id, final_w, final_h, false);
    if let Some(n) = st.model.field.node_mut(resize.node_id) {
        n.intrinsic_size.x = final_bbox_w;
        n.intrinsic_size.y = final_bbox_h;
    }
    let _ = st
        .model
        .field
        .sync_active_footprint_to_intrinsic(resize.node_id);
    st.set_last_active_size_now(
        resize.node_id,
        halley_core::field::Vec2 {
            x: final_bbox_w,
            y: final_bbox_h,
        },
    );
    st.set_recent_top_node(resize.node_id, now + Duration::from_millis(600));
    st.end_resize_interaction(now);
    st.resolve_overlap_now();
}

pub(crate) fn advance_resize_anim(
    st: &mut Halley,
    ps: &mut PointerState,
    now: Instant,
) -> Option<halley_core::field::NodeId> {
    let Some(mut resize) = ps.resize.take() else {
        return None;
    };
    if !resize.drag_started {
        ps.resize = Some(resize);
        return None;
    }
    if !st.runtime.tuning.smooth_resize_enabled() && !resize.settling {
        ps.resize = Some(resize);
        return None;
    }

    let settled = refresh_resize_preview_state(st, &mut resize, now);
    let node_id = resize.node_id;
    if resize.settling && settled && resize_settle_velocity_done(&resize) {
        finish_resize_interaction(st, ps, resize, now);
        return Some(node_id);
    }

    ps.resize = Some(resize);
    Some(node_id)
}

pub(super) fn begin_resize(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
    if !node_allows_interactive_resize(st, hit.node_id) {
        return;
    }
    let Some(n) = st.model.field.node(hit.node_id) else {
        return;
    };
    let fallback_size = n.intrinsic_size;
    let fallback_pos = n.pos;
    let (start_left, start_top, start_right, start_bottom) = active_node_screen_rect(
        st,
        frame.ws_w,
        frame.ws_h,
        hit.node_id,
        Instant::now(),
        None,
    )
    .unwrap_or_else(|| {
        let center_scr =
            world_to_screen(st, frame.ws_w, frame.ws_h, fallback_pos.x, fallback_pos.y);
        (
            (center_scr.0 as f32) - fallback_size.x * 0.5,
            (center_scr.1 as f32) - fallback_size.y * 0.5,
            (center_scr.0 as f32) + fallback_size.x * 0.5,
            (center_scr.1 as f32) + fallback_size.y * 0.5,
        )
    });

    let rect = (start_left, start_top, start_right, start_bottom);
    let border_slop = active_window_frame_pad_px(&st.runtime.tuning) as f32;
    let handle = if st.runtime.tuning.resize_using_border
        && border_slop > 0.0
        && press_is_near_edge(rect, (frame.sx, frame.sy), border_slop)
    {
        pick_resize_handle_from_screen(rect, (frame.sx, frame.sy), border_slop)
    } else {
        handle_from_press_position(rect, (frame.sx, frame.sy))
    };
    let (h_weight_left, h_weight_right, v_weight_top, v_weight_bottom) =
        weights_from_handle(handle);

    if let Some(drag) = ps.drag {
        crate::compositor::carry::system::set_drag_authority_node(st, None);
        crate::compositor::carry::system::end_carry_state_tracking(st, drag.node_id);
    }
    ps.drag = None;
    ps.panning = false;
    ps.pan_monitor = None;
    ps.move_anim.clear();
    st.input
        .interaction_state
        .physics_velocity
        .insert(hit.node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 });
    st.begin_resize_interaction(hit.node_id, Instant::now());

    let (min_lw, min_lh) = toplevel_min_size_for_node(st, hit.node_id);
    let cam_scale = st.camera_render_scale();
    let start_w = (start_right - start_left)
        .max(min_lw as f32 * cam_scale)
        .max(96.0)
        .round() as i32;
    let start_h = (start_bottom - start_top)
        .max(min_lh as f32 * cam_scale)
        .max(72.0)
        .round() as i32;
    let start_surface =
        current_surface_size_for_node(st, hit.node_id).unwrap_or(halley_core::field::Vec2 {
            x: start_w as f32,
            y: start_h as f32,
        });
    let (start_geo_lx, start_geo_ly, _, _) = window_geometry_for_node(st, hit.node_id).unwrap_or((
        0.0,
        0.0,
        start_surface.x.max(1.0),
        start_surface.y.max(1.0),
    ));
    let (start_bbox_lx, start_bbox_ly) = st
        .ui
        .render_state
        .bbox_loc
        .get(&hit.node_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    let start_bbox = halley_core::field::Vec2 {
        x: fallback_size.x.max(1.0),
        y: fallback_size.y.max(1.0),
    };

    let resize_ctx = ResizeCtx {
        node_id: hit.node_id,
        workspace_w: frame.ws_w,
        workspace_h: frame.ws_h,
        start_surface_w: start_surface.x.max(min_lw as f32).max(96.0).round() as i32,
        start_surface_h: start_surface.y.max(min_lh as f32).max(72.0).round() as i32,
        start_bbox_w: start_bbox.x.round() as i32,
        start_bbox_h: start_bbox.y.round() as i32,
        start_visual_w: start_w,
        start_visual_h: start_h,
        start_geo_lx,
        start_geo_ly,
        start_geo_inset_x: (start_geo_lx.round() - start_bbox_lx.round()) as i32,
        start_geo_inset_y: (start_geo_ly.round() - start_bbox_ly.round()) as i32,
        start_left_px: start_left,
        start_right_px: start_right,
        start_top_px: start_top,
        start_bottom_px: start_bottom,
        preview_left_px: start_left,
        preview_right_px: start_right,
        preview_top_px: start_top,
        preview_bottom_px: start_bottom,
        target_left_px: start_left,
        target_right_px: start_right,
        target_top_px: start_top,
        target_bottom_px: start_bottom,
        preview_velocity_left_pxps: 0.0,
        preview_velocity_right_pxps: 0.0,
        preview_velocity_top_pxps: 0.0,
        preview_velocity_bottom_pxps: 0.0,
        last_sent_w: start_surface.x.max(min_lw as f32).max(96.0).round() as i32,
        last_sent_h: start_surface.y.max(min_lh as f32).max(72.0).round() as i32,
        last_smooth_tick_at: Instant::now(),
        handle,
        press_sx: frame.sx,
        press_sy: frame.sy,
        h_weight_left,
        h_weight_right,
        v_weight_top,
        v_weight_bottom,
        drag_started: false,
        settling: false,
        resize_mode_sent: false,
    };

    ps.resize = Some(resize_ctx);
    backend.request_redraw();
}

pub(super) fn finalize_resize(st: &mut Halley, ps: &mut PointerState, backend: &dyn BackendView) {
    let ended_resize = ps.resize.take();
    ps.panning = false;
    let Some(mut resize) = ended_resize else {
        return;
    };

    let now = Instant::now();
    ps.move_anim.clear();
    crate::compositor::carry::system::set_drag_authority_node(st, None);
    st.input
        .interaction_state
        .physics_velocity
        .insert(resize.node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 });
    ps.resize_trace_node = None;
    ps.resize_trace_until = None;
    ps.resize_trace_last_at = None;
    if st.runtime.tuning.smooth_resize_enabled() && resize.drag_started {
        let _ = refresh_resize_preview_state(st, &mut resize, now);
        resize.preview_velocity_left_pxps = 0.0;
        resize.preview_velocity_right_pxps = 0.0;
        resize.preview_velocity_top_pxps = 0.0;
        resize.preview_velocity_bottom_pxps = 0.0;
        resize.target_left_px = resize.preview_left_px;
        resize.target_right_px = resize.preview_right_px;
        resize.target_top_px = resize.preview_top_px;
        resize.target_bottom_px = resize.preview_bottom_px;
        resize.settling = false;
    }

    finish_resize_interaction(st, ps, resize, now);
    backend.request_redraw();
}

pub(super) fn handle_resize_motion(
    st: &mut Halley,
    ps: &mut crate::compositor::interaction::PointerState,
    _local_w: i32,
    _local_h: i32,
    local_sx: f32,
    local_sy: f32,
    backend: &impl crate::backend::interface::BackendView,
) -> bool {
    let Some(resize) = ps.resize else {
        return false;
    };
    if resize.settling {
        return false;
    }

    let mut next = resize;
    let dx = local_sx - resize.press_sx;
    let dy = local_sy - resize.press_sy;

    const RESIZE_DRAG_START_PX: f32 = 3.0;

    if !next.drag_started {
        if dx.abs().max(dy.abs()) < RESIZE_DRAG_START_PX {
            ps.resize = Some(next);
            return true;
        }
        next.drag_started = true;
    }

    if !next.resize_mode_sent {
        request_toplevel_resize_mode(
            st,
            resize.node_id,
            resize.last_sent_w,
            resize.last_sent_h,
            true,
        );
        next.resize_mode_sent = true;
    }

    let (min_lw, min_lh) = toplevel_min_size_for_node(st, resize.node_id);
    let cam_scale = st.camera_render_scale();
    let min_w = (min_lw as f32 * cam_scale).max(96.0);
    let min_h = (min_lh as f32 * cam_scale).max(72.0);

    let desired_left = resize.start_left_px + next.h_weight_left * dx;
    let desired_right = resize.start_right_px + next.h_weight_right * dx;
    let desired_top = resize.start_top_px + next.v_weight_top * dy;
    let desired_bottom = resize.start_bottom_px + next.v_weight_bottom * dy;

    let (left, right) = if next.h_weight_left != 0.0 && next.h_weight_right == 0.0 {
        let anchored_right = resize.start_right_px;
        let clamped_left = desired_left.min(anchored_right - min_w);
        (clamped_left, anchored_right)
    } else if next.h_weight_right != 0.0 && next.h_weight_left == 0.0 {
        let anchored_left = resize.start_left_px;
        let clamped_right = desired_right.max(anchored_left + min_w);
        (anchored_left, clamped_right)
    } else {
        let raw_w = desired_right - desired_left;
        if raw_w < min_w {
            let shortage = min_w - raw_w;
            let abs_l = next.h_weight_left.abs();
            let abs_r = next.h_weight_right.abs();
            let total_hw = (abs_l + abs_r).max(f32::EPSILON);
            let nudge_l = shortage * abs_l / total_hw;
            let nudge_r = shortage * abs_r / total_hw;
            (desired_left - nudge_l, desired_right + nudge_r)
        } else {
            (desired_left, desired_right)
        }
    };

    let (top, bottom) = if next.v_weight_top != 0.0 && next.v_weight_bottom == 0.0 {
        let anchored_bottom = resize.start_bottom_px;
        let clamped_top = desired_top.min(anchored_bottom - min_h);
        (clamped_top, anchored_bottom)
    } else if next.v_weight_bottom != 0.0 && next.v_weight_top == 0.0 {
        let anchored_top = resize.start_top_px;
        let clamped_bottom = desired_bottom.max(anchored_top + min_h);
        (anchored_top, clamped_bottom)
    } else {
        let raw_h = desired_bottom - desired_top;
        if raw_h < min_h {
            let shortage = min_h - raw_h;
            let abs_t = next.v_weight_top.abs();
            let abs_b = next.v_weight_bottom.abs();
            let total_vw = (abs_t + abs_b).max(f32::EPSILON);
            let nudge_t = shortage * abs_t / total_vw;
            let nudge_b = shortage * abs_b / total_vw;
            (desired_top - nudge_t, desired_bottom + nudge_b)
        } else {
            (desired_top, desired_bottom)
        }
    };

    let now = Instant::now();
    advance_resize_preview_toward_target(st, &mut next, now);
    next.target_left_px = left;
    next.target_right_px = right;
    next.target_top_px = top;
    next.target_bottom_px = bottom;
    next.settling = false;
    if !st.runtime.tuning.smooth_resize_enabled() {
        next.preview_left_px = left;
        next.preview_right_px = right;
        next.preview_top_px = top;
        next.preview_bottom_px = bottom;
    }
    let _ = apply_resize_preview_state(st, &mut next);
    ps.resize = Some(next);
    backend.request_redraw();
    true
}

#[derive(Clone, Copy)]
pub(crate) struct ActiveNodeSurfaceTransformScreen {
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
    pub(crate) scale: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct ActiveResizeGeometryScreen {
    pub(crate) frame_left: f32,
    pub(crate) frame_top: f32,
    pub(crate) frame_right: f32,
    pub(crate) frame_bottom: f32,
    pub(crate) surface_origin_x: f32,
    pub(crate) surface_origin_y: f32,

    pub(crate) live_geo_w: f32,
    pub(crate) live_geo_h: f32,
}

impl ActiveResizeGeometryScreen {
    pub(crate) fn frame_rect_px(self) -> (i32, i32, i32, i32) {
        let left = self.frame_left.round() as i32;
        let top = self.frame_top.round() as i32;
        let right = self.frame_right.round() as i32;
        let bottom = self.frame_bottom.round() as i32;
        (left, top, (right - left).max(1), (bottom - top).max(1))
    }

    pub(crate) fn surface_origin_px(self) -> (i32, i32) {
        (
            self.surface_origin_x.round() as i32,
            self.surface_origin_y.round() as i32,
        )
    }

    pub(crate) fn center_px(self) -> (i32, i32) {
        (
            ((self.frame_left + self.frame_right) * 0.5).round() as i32,
            ((self.frame_top + self.frame_bottom) * 0.5).round() as i32,
        )
    }
}

/// Pick a resize handle from the nearest edge/corner to the press point.
/// Only called for direct border grabs (press within edge slop zone).
#[allow(dead_code)]
pub(crate) fn pick_resize_handle_from_screen(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
    edge_slop: f32,
) -> ResizeHandle {
    let (l, t, r, b) = rect;
    let dl = (p.0 - l).abs();
    let dr = (r - p.0).abs();
    let dt = (p.1 - t).abs();
    let db = (b - p.1).abs();
    let edge_slop = edge_slop.max(0.0);
    let near_left = dl <= edge_slop;
    let near_right = dr <= edge_slop;
    let near_top = dt <= edge_slop;
    let near_bottom = db <= edge_slop;

    if near_left && near_top {
        return ResizeHandle::TopLeft;
    }
    if near_right && near_top {
        return ResizeHandle::TopRight;
    }
    if near_left && near_bottom {
        return ResizeHandle::BottomLeft;
    }
    if near_right && near_bottom {
        return ResizeHandle::BottomRight;
    }

    let min_d = dl.min(dr).min(dt).min(db);
    if (min_d - dl).abs() <= f32::EPSILON {
        ResizeHandle::Left
    } else if (min_d - dr).abs() <= f32::EPSILON {
        ResizeHandle::Right
    } else if (min_d - dt).abs() <= f32::EPSILON {
        ResizeHandle::Top
    } else {
        ResizeHandle::Bottom
    }
}

/// Commit a resize handle from where the pointer pressed within the window,
/// using a 3×3 grid split at the 1/3 and 2/3 fractional positions:
///
///   fx:   0..1/3     1/3..2/3    2/3..1
///        ┌──────────┬──────────┬──────────┐
///  0..   │ TopLeft  │   Top    │ TopRight │
/// 1/3    ├──────────┼──────────┼──────────┤
///  1/3.. │  Left    │ nearest  │  Right   │
///  2/3   ├──────────┼──────────┼──────────┤
///  2/3.. │BotLeft   │  Bottom  │ BotRight │
///  1     └──────────┴──────────┴──────────┘
///
/// Pressing near top-left and dragging any direction pulls the top-left corner.
/// The centre cell falls back to whichever edge is nearest.
pub(crate) fn handle_from_press_position(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
) -> ResizeHandle {
    let (l, t, r, b) = rect;
    let w = (r - l).max(1.0);
    let h = (b - t).max(1.0);
    let fx = ((p.0 - l) / w).clamp(0.0, 1.0);
    let fy = ((p.1 - t) / h).clamp(0.0, 1.0);

    #[derive(PartialEq)]
    enum Z {
        Near,
        Mid,
        Far,
    }
    let hz = if fx < 1.0 / 3.0 {
        Z::Near
    } else if fx < 2.0 / 3.0 {
        Z::Mid
    } else {
        Z::Far
    };
    let vz = if fy < 1.0 / 3.0 {
        Z::Near
    } else if fy < 2.0 / 3.0 {
        Z::Mid
    } else {
        Z::Far
    };

    match (hz, vz) {
        (Z::Near, Z::Near) => ResizeHandle::TopLeft,
        (Z::Mid, Z::Near) => ResizeHandle::Top,
        (Z::Far, Z::Near) => ResizeHandle::TopRight,
        (Z::Near, Z::Mid) => ResizeHandle::Left,
        (Z::Mid, Z::Mid) => {
            // Centre: nearest edge
            let dl = p.0 - l;
            let dr = r - p.0;
            let dt = p.1 - t;
            let db = b - p.1;
            let min_d = dl.min(dr).min(dt).min(db);
            if (min_d - dl).abs() <= f32::EPSILON {
                ResizeHandle::Left
            } else if (min_d - dr).abs() <= f32::EPSILON {
                ResizeHandle::Right
            } else if (min_d - dt).abs() <= f32::EPSILON {
                ResizeHandle::Top
            } else {
                ResizeHandle::Bottom
            }
        }
        (Z::Far, Z::Mid) => ResizeHandle::Right,
        (Z::Near, Z::Far) => ResizeHandle::BottomLeft,
        (Z::Mid, Z::Far) => ResizeHandle::Bottom,
        (Z::Far, Z::Far) => ResizeHandle::BottomRight,
    }
}

/// Returns `true` if the press point is within the edge slop zone of `rect`.
#[allow(dead_code)]
pub(crate) fn press_is_near_edge(
    rect: (f32, f32, f32, f32),
    p: (f32, f32),
    edge_slop: f32,
) -> bool {
    let (l, t, r, b) = rect;
    let edge_slop = edge_slop.max(0.0);
    (p.0 - l).abs() <= edge_slop
        || (r - p.0).abs() <= edge_slop
        || (p.1 - t).abs() <= edge_slop
        || (b - p.1).abs() <= edge_slop
}

/// Commit a resize handle from the drag vector `(dx, dy)` using an octant
/// split with a 2:1 aspect-ratio threshold:
///
///   |dy| < |dx| / 2  →  Left or Right   (wide horizontal band)
///   |dx| < |dy| / 2  →  Top or Bottom   (wide vertical band)
///   otherwise         →  corner quadrant
///
/// `dx` positive = rightward, `dy` positive = downward (screen space).
/// Never returns `Pending`.
#[allow(dead_code)]
pub(crate) fn commit_handle_from_drag(dx: f32, dy: f32) -> ResizeHandle {
    let adx = dx.abs();
    let ady = dy.abs();
    let right = dx >= 0.0;
    let down = dy >= 0.0;

    if ady < adx / 2.0 {
        if right {
            ResizeHandle::Right
        } else {
            ResizeHandle::Left
        }
    } else if adx < ady / 2.0 {
        if down {
            ResizeHandle::Bottom
        } else {
            ResizeHandle::Top
        }
    } else {
        match (right, down) {
            (true, true) => ResizeHandle::BottomRight,
            (true, false) => ResizeHandle::TopRight,
            (false, true) => ResizeHandle::BottomLeft,
            (false, false) => ResizeHandle::TopLeft,
        }
    }
}

/// Map a committed handle to its four signed edge weights
/// `(h_weight_left, h_weight_right, v_weight_top, v_weight_bottom)`.
///
/// The preview rect is updated each frame as:
///
///   new_left   = start_left   + h_weight_left  * dx
///   new_right  = start_right  + h_weight_right * dx
///   new_top    = start_top    + v_weight_top   * dy
///   new_bottom = start_bottom + v_weight_bottom * dy
///
/// Weight semantics:
///   +1.0  — this edge tracks the pointer directly in screen space.
///           For example, dragging the top edge downward increases `top`,
///           which shrinks the frame from the top in y-down coordinates.
///   -1.0  — reserved for opposite-direction edge motion.
///    0.0  — this edge is anchored and does not move
///
/// Both weights being 0.0 on an axis means that axis is not resized at all
/// (e.g. a pure Left/Right grab does not change the window height).
pub(crate) fn weights_from_handle(handle: ResizeHandle) -> (f32, f32, f32, f32) {
    // (h_left, h_right, v_top, v_bottom)
    match handle {
        ResizeHandle::Left => (1.0, 0.0, 0.0, 0.0),
        ResizeHandle::Right => (0.0, 1.0, 0.0, 0.0),
        ResizeHandle::Top => (0.0, 0.0, 1.0, 0.0),
        ResizeHandle::Bottom => (0.0, 0.0, 0.0, 1.0),
        ResizeHandle::TopLeft => (1.0, 0.0, 1.0, 0.0),
        ResizeHandle::TopRight => (0.0, 1.0, 1.0, 0.0),
        ResizeHandle::BottomLeft => (1.0, 0.0, 0.0, 1.0),
        ResizeHandle::BottomRight => (0.0, 1.0, 0.0, 1.0),
        ResizeHandle::Pending => (0.0, 0.0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::interface::TtyBackendHandle;
    use crate::compositor::interaction::{HitNode, PointerState};
    use crate::compositor::root::Halley;
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn drag_direction_maps_to_y_down_resize_handles() {
        assert_eq!(commit_handle_from_drag(0.0, -40.0), ResizeHandle::Top);
        assert_eq!(commit_handle_from_drag(0.0, 40.0), ResizeHandle::Bottom);
        assert_eq!(commit_handle_from_drag(40.0, -40.0), ResizeHandle::TopRight);
        assert_eq!(
            commit_handle_from_drag(-40.0, 40.0),
            ResizeHandle::BottomLeft
        );
    }

    #[test]
    fn top_and_bottom_weights_follow_screen_space_motion() {
        assert_eq!(weights_from_handle(ResizeHandle::Top), (0.0, 0.0, 1.0, 0.0));
        assert_eq!(
            weights_from_handle(ResizeHandle::Bottom),
            (0.0, 0.0, 0.0, 1.0)
        );
        assert_eq!(
            weights_from_handle(ResizeHandle::TopLeft),
            (1.0, 0.0, 1.0, 0.0)
        );
        assert_eq!(
            weights_from_handle(ResizeHandle::BottomRight),
            (0.0, 1.0, 0.0, 1.0)
        );
    }

    fn single_monitor_tiling_tuning() -> halley_config::RuntimeTuning {
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
        tuning
    }

    fn resize_button_frame() -> ButtonFrame {
        ButtonFrame {
            ws_w: 800,
            ws_h: 600,
            global_sx: 400.0,
            global_sy: 300.0,
            sx: 400.0,
            sy: 300.0,
            world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            workspace_active: false,
        }
    }

    #[test]
    fn begin_resize_blocks_active_tiled_workspace_members() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
        let backend = TtyBackendHandle::new(800, 600);

        let master = st.model.field.spawn_surface(
            "master",
            halley_core::field::Vec2 { x: 120.0, y: 120.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            halley_core::field::Vec2 { x: 520.0, y: 120.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack] {
            st.assign_node_to_monitor(id, "monitor_a");
        }
        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let mut ps = PointerState::default();
        begin_resize(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: master,
                on_titlebar: false,
                is_core: false,
            },
            resize_button_frame(),
        );

        assert!(ps.resize.is_none());
        assert!(st.input.interaction_state.resize_active.is_none());
    }

    #[test]
    fn begin_resize_allows_non_tiled_active_windows() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
        let backend = TtyBackendHandle::new(800, 600);

        let window = st.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: 300.0, y: 220.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(window, "monitor_a");

        let mut ps = PointerState::default();
        begin_resize(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: window,
                on_titlebar: false,
                is_core: false,
            },
            resize_button_frame(),
        );

        assert!(ps.resize.is_some());
        assert_eq!(st.input.interaction_state.resize_active, Some(window));
    }

    #[test]
    fn smooth_resize_continues_advancing_across_quick_pointer_updates() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tiling_tuning();
        tuning.animations.smooth_resize.enabled = true;
        tuning.animations.smooth_resize.duration_ms = 400;
        let mut st = Halley::new_for_test(&dh, tuning);
        let backend = TtyBackendHandle::new(800, 600);

        let window = st.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: 300.0, y: 220.0 },
            halley_core::field::Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(window, "monitor_a");

        let mut ps = PointerState::default();
        begin_resize(
            &mut st,
            &mut ps,
            &backend,
            HitNode {
                node_id: window,
                on_titlebar: false,
                is_core: false,
            },
            resize_button_frame(),
        );

        assert!(handle_resize_motion(
            &mut st, &mut ps, 800, 600, 520.0, 380.0, &backend,
        ));

        let first = ps.resize.expect("resize in progress");
        assert!(
            !resize_rect_nearly_eq(first.preview_right_px, first.target_right_px)
                || !resize_rect_nearly_eq(first.preview_bottom_px, first.target_bottom_px),
            "smooth resize should lag behind the cursor-driven target while dragging"
        );
        let first_preview_right = first.preview_right_px;
        let first_target_right = first.target_right_px;

        let tick_one_at = first.last_smooth_tick_at + Duration::from_millis(16);
        let ticked = advance_resize_anim(&mut st, &mut ps, tick_one_at).expect("resize tick one");
        assert_eq!(ticked, window);

        let after_tick_one = ps.resize.expect("resize after first tick");
        assert!(
            after_tick_one.preview_right_px > first_preview_right + 0.1,
            "preview should move continuously toward the target during drag"
        );
        assert!(after_tick_one.preview_right_px < after_tick_one.target_right_px);

        assert!(handle_resize_motion(
            &mut st, &mut ps, 800, 600, 620.0, 460.0, &backend,
        ));

        let second = ps.resize.expect("resize after second pointer update");
        assert!(
            second.target_right_px > first_target_right,
            "second pointer update should move the target farther out"
        );
        assert!(
            second.preview_right_px >= after_tick_one.preview_right_px - 0.5,
            "preview should not reset or jump backward on rapid pointer updates"
        );

        let tick_two_at = second.last_smooth_tick_at + Duration::from_millis(16);
        let ticked = advance_resize_anim(&mut st, &mut ps, tick_two_at).expect("resize tick two");
        assert_eq!(ticked, window);

        let after_tick_two = ps.resize.expect("resize after second tick");
        assert!(
            after_tick_two.preview_right_px > second.preview_right_px + 0.1,
            "preview should keep advancing instead of hesitating after repeated quick motion"
        );
        assert!(after_tick_two.preview_right_px < after_tick_two.target_right_px);
        let release_preview_right = after_tick_two.preview_right_px;
        let release_target_right = after_tick_two.target_right_px;

        finalize_resize(&mut st, &mut ps, &backend);
        assert!(
            ps.resize.is_none(),
            "resize should stop immediately at release instead of entering a settle animation"
        );
        let final_rect = active_node_screen_rect(&st, 800, 600, window, Instant::now(), None)
            .expect("final window rect");
        assert!(
            final_rect.2 < release_target_right - 8.0,
            "release should not finish the full trajectory to the old cursor target"
        );
        assert!(
            final_rect.2 >= release_preview_right - 4.0,
            "release should stop near the current preview instead of snapping backward"
        );
        assert!(
            st.input.interaction_state.resize_active.is_none(),
            "resize interaction should end immediately on release"
        );
    }
}

pub(crate) fn active_node_screen_rect(
    st: &Halley,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(active_resize) = active_resize_geometry_screen(st, node_id, resize_preview) {
        return Some((
            active_resize.frame_left,
            active_resize.frame_top,
            active_resize.frame_right,
            active_resize.frame_bottom,
        ));
    }

    // Mirror the render path exactly: center on local_geo, derive geometry_rect.
    let xform = active_node_surface_transform_screen_details(st, w, h, node_id, now, None)?;
    let local_geo = active_node_visual_local_rect(st, node_id).or_else(|| {
        st.model.field.node(node_id).map(|n| {
            (
                0.0,
                0.0,
                n.intrinsic_size.x.max(1.0),
                n.intrinsic_size.y.max(1.0),
            )
        })
    })?;

    let (gx, gy, gw, gh) = local_geo;
    let rw = (gw * xform.scale).round().max(1.0);
    let rh = (gh * xform.scale).round().max(1.0);
    let mut rx = xform.origin_x + (gx * xform.scale).round();
    let mut ry = xform.origin_y + (gy * xform.scale).round();
    let mut rr = rx + rw;
    let mut rb = ry + rh;

    let stack_render_order = active_stacking_render_order_for_monitor(
        st,
        st.model.monitor_state.current_monitor.as_str(),
    );
    if stack_render_order.contains_key(&node_id) {
        let frame_pad_px = active_window_frame_pad_px(&st.runtime.tuning) as f32 * xform.scale;
        rx -= frame_pad_px;
        ry -= frame_pad_px;
        rr += frame_pad_px;
        rb += frame_pad_px;
    }

    Some((rx, ry, rr, rb))
}

/// Compute the screen-space surface-tree origin and scale for an active node,
/// matching exactly the placement used by the render path.
pub(crate) fn active_node_surface_transform_screen_details(
    st: &Halley,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveNodeSurfaceTransformScreen> {
    let n = st.model.field.node(node_id)?;
    if n.state != halley_core::field::NodeState::Active {
        return None;
    }

    let anim = crate::render::anim_style_for(st, node_id, n.state.clone(), now);
    let transition_alpha = st.active_transition_alpha(node_id, now);
    let cam_scale = st.camera_render_scale();

    // Fit scale for fullscreen windows that don't match the physical monitor resolution.
    // This happens often in XWayland when a game queries the "primary" monitor size
    // and gets it wrong, resulting in a window smaller than the actual monitor.
    let fit_scale = if let Some(monitor) = st.fullscreen_monitor_for_node(node_id) {
        let (target_w, target_h) = if monitor == st.model.monitor_state.current_monitor {
            let ws = st.model.viewport.size;
            (ws.x.round() as i32, ws.y.round() as i32)
        } else {
            st.fullscreen_target_size_for(monitor)
        };
        let sw = (target_w as f32) / n.intrinsic_size.x.max(1.0);
        let sh = (target_h as f32) / n.intrinsic_size.y.max(1.0);
        sw.min(sh).max(0.1) // aspect-correct fit
    } else {
        1.0
    };

    let anim_scale = active_surface_render_scale(
        anim.scale,
        st.active_zoom_lock_scale(),
        n.intrinsic_size.x,
        n.intrinsic_size.y,
        transition_alpha,
    ) * st.fullscreen_entry_scale(node_id, st.now_ms(now))
        * fit_scale
        * cam_scale;

    let (origin_x, origin_y, scale) =
        if let Some(active_resize) = active_resize_geometry_screen(st, node_id, resize_preview) {
            (
                active_resize.surface_origin_x,
                active_resize.surface_origin_y,
                1.0f32,
            )
        } else {
            let p = n.pos;
            let (cx, cy) = world_to_screen(st, w, h, p.x, p.y);

            let bbox_lx = st
                .ui
                .render_state
                .bbox_loc
                .get(&node_id)
                .copied()
                .unwrap_or((0.0, 0.0))
                .0;
            let bbox_ly = st
                .ui
                .render_state
                .bbox_loc
                .get(&node_id)
                .copied()
                .unwrap_or((0.0, 0.0))
                .1;
            let bbox_w = n.intrinsic_size.x.max(1.0);
            let bbox_h = n.intrinsic_size.y.max(1.0);
            let local_bbox = (bbox_lx, bbox_ly, bbox_w, bbox_h);
            let (gx, gy, gw, gh) = st
                .ui
                .render_state
                .window_geometry
                .get(&node_id)
                .copied()
                .map(|(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
                .unwrap_or(local_bbox);

            let (rx, ry) = if st
                .fullscreen_monitor_for_node(node_id)
                .is_some_and(|monitor| monitor == st.model.monitor_state.current_monitor)
            {
                (0, 0)
            } else {
                let rw = (gw * anim_scale).round() as i32;
                let rh = (gh * anim_scale).round() as i32;
                (cx - (rw / 2), cy - (rh / 2))
            };
            let origin_x = (rx as f32) - (gx * anim_scale).round();
            let origin_y = (ry as f32) - (gy * anim_scale).round();

            (origin_x, origin_y, anim_scale)
        };

    Some(ActiveNodeSurfaceTransformScreen {
        origin_x,
        origin_y,
        scale: scale.max(0.001),
    })
}

pub(crate) fn active_resize_geometry_screen(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveResizeGeometryScreen> {
    let rz = resize_preview.filter(|rz| rz.node_id == node_id)?;
    // While Pending the window hasn't moved yet — don't produce a preview rect.
    if rz.handle == ResizeHandle::Pending {
        return None;
    }
    let frame_left = rz.preview_left_px;
    let frame_top = rz.preview_top_px;
    let frame_right = rz.preview_right_px;
    let frame_bottom = rz.preview_bottom_px;
    let (_, _, live_geo_w, live_geo_h) = st
        .ui
        .render_state
        .window_geometry
        .get(&node_id)
        .copied()
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    let geo_lx = rz.start_geo_lx;
    let geo_ly = rz.start_geo_ly;

    Some(ActiveResizeGeometryScreen {
        frame_left,
        frame_top,
        frame_right,
        frame_bottom,
        surface_origin_x: frame_left - geo_lx.round(),
        surface_origin_y: frame_top - geo_ly.round(),
        live_geo_w,
        live_geo_h,
    })
}

fn active_node_visual_local_rect(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(&(x, y, w, h)) = st.ui.render_state.window_geometry.get(&node_id) {
        return Some((x, y, w.max(1.0), h.max(1.0)));
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }

        let geo = with_states(wl, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        });
        if let Some(g) = geo {
            return Some((
                g.loc.x as f32,
                g.loc.y as f32,
                g.size.w.max(1) as f32,
                g.size.h.max(1) as f32,
            ));
        }

        let bbox = bbox_from_surface_tree(wl, (0, 0));
        return Some((
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w.max(1) as f32,
            bbox.size.h.max(1) as f32,
        ));
    }

    None
}
