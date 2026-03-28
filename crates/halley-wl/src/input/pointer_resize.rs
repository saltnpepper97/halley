use std::time::{Duration, Instant};

use eventline::debug;

use crate::backend::interface::BackendView;
use crate::interaction::types::{HitNode, PointerState, ResizeCtx};
use crate::render::active_window_frame_pad_px;
use crate::render::world_to_screen;
use crate::state::Halley;
use crate::surface_ops::{
    current_surface_size_for_node, request_toplevel_resize_mode, window_geometry_for_node,
};

use super::pointer_frame::ButtonFrame;
use super::resize_helpers::{
    active_node_screen_rect, handle_from_press_position, pick_resize_handle_from_screen,
    press_is_near_edge, weights_from_handle,
};

pub(super) fn begin_resize(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
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
        st.set_drag_authority_node(None);
        st.end_carry_state_tracking(drag.node_id);
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

    let start_w = (start_right - start_left).max(96.0).round() as i32;
    let start_h = (start_bottom - start_top).max(72.0).round() as i32;
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
        start_surface_w: start_surface.x.max(96.0).round() as i32,
        start_surface_h: start_surface.y.max(72.0).round() as i32,
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
        last_sent_w: start_surface.x.max(96.0).round() as i32,
        last_sent_h: start_surface.y.max(72.0).round() as i32,
        last_configure_at: Instant::now(),
        handle,
        press_sx: frame.sx,
        press_sy: frame.sy,
        h_weight_left,
        h_weight_right,
        v_weight_top,
        v_weight_bottom,
        drag_started: false,
        resize_mode_sent: false,
    };

    if st.runtime.tuning.debug_tick_dump {
        debug!(
            "resize-start id={} handle={:?} preview=({:.1},{:.1},{:.1},{:.1}) frozen_geo=({:.1},{:.1}) start_surface=({}, {}) start_bbox=({}, {})",
            resize_ctx.node_id.as_u64(),
            resize_ctx.handle,
            resize_ctx.preview_left_px,
            resize_ctx.preview_top_px,
            resize_ctx.preview_right_px,
            resize_ctx.preview_bottom_px,
            resize_ctx.start_geo_lx,
            resize_ctx.start_geo_ly,
            resize_ctx.start_surface_w,
            resize_ctx.start_surface_h,
            resize_ctx.start_bbox_w,
            resize_ctx.start_bbox_h,
        );
    }

    ps.resize = Some(resize_ctx);
    backend.request_redraw();
}

pub(super) fn finalize_resize(st: &mut Halley, ps: &mut PointerState, backend: &dyn BackendView) {
    let ended_resize = ps.resize.take();
    ps.panning = false;
    let Some(resize) = ended_resize else {
        return;
    };

    let now = Instant::now();
    ps.move_anim.clear();
    st.set_drag_authority_node(None);
    st.input
        .interaction_state
        .physics_velocity
        .insert(resize.node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 });
    if st.runtime.tuning.debug_tick_dump {
        ps.resize_trace_node = Some(resize.node_id);
        ps.resize_trace_until = Some(now + Duration::from_millis(1_200));
        ps.resize_trace_last_at = None;
    } else {
        ps.resize_trace_node = None;
        ps.resize_trace_until = None;
        ps.resize_trace_last_at = None;
    }
    ps.preview_block_until = Some(now + Duration::from_millis(360));
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
        backend.request_redraw();
        return;
    }

    let final_w = resize.last_sent_w.max(96);
    let final_h = resize.last_sent_h.max(72);
    let final_bbox_w =
        ((resize.start_bbox_w as f32) + ((final_w - resize.start_surface_w) as f32)).max(1.0);
    let final_bbox_h =
        ((resize.start_bbox_h as f32) + ((final_h - resize.start_surface_h) as f32)).max(1.0);
    if st.runtime.tuning.debug_tick_dump {
        debug!(
            "resize-end id={} handle={:?} preview=({:.1},{:.1},{:.1},{:.1}) frozen_geo=({:.1},{:.1}) final_surface=({}, {}) final_bbox=({:.1}, {:.1})",
            resize.node_id.as_u64(),
            resize.handle,
            resize.preview_left_px,
            resize.preview_top_px,
            resize.preview_right_px,
            resize.preview_bottom_px,
            resize.start_geo_lx,
            resize.start_geo_ly,
            final_w,
            final_h,
            final_bbox_w,
            final_bbox_h,
        );
    }
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
    backend.request_redraw();
}
