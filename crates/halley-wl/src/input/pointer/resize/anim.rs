use std::time::{Duration, Instant};

use crate::compositor::interaction::{PointerState, ResizeCtx};
use crate::compositor::root::Halley;
use crate::compositor::surface::{request_toplevel_resize_mode, toplevel_min_size_for_node};

use super::resize_rect_nearly_eq;

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

pub(super) fn advance_resize_preview_toward_target(
    st: &Halley,
    resize: &mut ResizeCtx,
    now: Instant,
) {
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

pub(super) fn finish_resize_now(
    st: &mut Halley,
    ps: &mut PointerState,
    resize: ResizeCtx,
    now: Instant,
) {
    finish_resize_interaction(st, ps, resize, now)
}

pub(super) fn refresh_resize_now(st: &mut Halley, resize: &mut ResizeCtx, now: Instant) -> bool {
    refresh_resize_preview_state(st, resize, now)
}

pub(super) fn apply_resize_now(st: &mut Halley, resize: &mut ResizeCtx) -> bool {
    apply_resize_preview_state(st, resize)
}
