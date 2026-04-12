use std::time::Instant;

use crate::backend::interface::BackendView;
use crate::compositor::interaction::{HitNode, PointerState, ResizeCtx};
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::{
    current_surface_size_for_node, node_allows_interactive_resize, request_toplevel_resize_mode,
    toplevel_min_size_for_node, window_geometry_for_node,
};
use crate::render::active_window_frame_pad_px;
use crate::render::world_to_screen;

use super::anim::{apply_resize_now, advance_resize_preview_toward_target, finish_resize_now, refresh_resize_now};
use super::geometry::active_node_screen_rect;
use super::handles::{
    cursor_icon_for_resize_handle, handle_from_press_position, pick_resize_handle_from_screen,
    press_is_near_edge, weights_from_handle,
};
use crate::input::pointer::button::ButtonFrame;

pub(crate) fn begin_resize(
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
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(cursor_icon_for_resize_handle(handle)),
    );
    backend.request_redraw();
}

pub(crate) fn finalize_resize(st: &mut Halley, ps: &mut PointerState, backend: &dyn BackendView) {
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
        let _ = refresh_resize_now(st, &mut resize, now);
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

    finish_resize_now(st, ps, resize, now);
    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    backend.request_redraw();
}

pub(crate) fn handle_resize_motion(
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
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(cursor_icon_for_resize_handle(resize.handle)),
        );
        return false;
    }

    let mut next = resize;
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(cursor_icon_for_resize_handle(next.handle)),
    );
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
    let _ = apply_resize_now(st, &mut next);
    ps.resize = Some(next);
    backend.request_redraw();
    true
}
