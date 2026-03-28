use std::time::Instant;

use crate::state::Halley;
use crate::surface_ops::request_toplevel_resize_mode;

pub(super) fn handle_resize_motion(
    st: &mut Halley,
    ps: &mut crate::interaction::types::PointerState,
    local_w: i32,
    local_h: i32,
    local_sx: f32,
    local_sy: f32,
    backend: &impl crate::backend::interface::BackendView,
) -> bool {
    let Some(resize) = ps.resize else {
        return false;
    };

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
        next.last_configure_at = Instant::now();
    }

    let min_w = 96.0_f32;
    let min_h = 72.0_f32;

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

    let target_visual_w = (right - left).round().max(min_w) as i32;
    let target_visual_h = (bottom - top).round().max(min_h) as i32;
    let cam_scale = st.camera_render_scale();
    let visual_delta_w = target_visual_w - resize.start_visual_w;
    let visual_delta_h = target_visual_h - resize.start_visual_h;
    let logical_delta_w = (visual_delta_w as f32 / cam_scale.max(0.001)).round() as i32;
    let logical_delta_h = (visual_delta_h as f32 / cam_scale.max(0.001)).round() as i32;
    let min_logical_w = (min_w / cam_scale.max(0.001)).round() as i32;
    let min_logical_h = (min_h / cam_scale.max(0.001)).round() as i32;

    let target_w = (resize.start_surface_w + logical_delta_w).max(min_logical_w);
    let target_h = (resize.start_surface_h + logical_delta_h).max(min_logical_h);

    let now = Instant::now();
    let size_changed = target_w != resize.last_sent_w || target_h != resize.last_sent_h;
    if size_changed {
        request_toplevel_resize_mode(st, resize.node_id, target_w, target_h, true);
        next.last_sent_w = target_w;
        next.last_sent_h = target_h;
        next.last_configure_at = now;
    }

    st.input
        .interaction_state
        .physics_velocity
        .insert(resize.node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 });

    let center_sx = (left + right) * 0.5;
    let center_sy = (top + bottom) * 0.5;
    let center_world = crate::spatial::screen_to_world(st, local_w, local_h, center_sx, center_sy);
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

    next.preview_left_px = left;
    next.preview_right_px = right;
    next.preview_top_px = top;
    next.preview_bottom_px = bottom;
    ps.resize = Some(next);

    let _ = st
        .model
        .field
        .set_decay_level(resize.node_id, halley_core::decay::DecayLevel::Hot);
    backend.request_redraw();
    true
}
