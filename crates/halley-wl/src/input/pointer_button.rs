use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eventline::info;
use halley_config::{KeyModifiers, PointerBindingAction};
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::interaction::actions::{
    activate_collapsed_node_from_click, focus_or_reveal_surface_node,
};
use crate::interaction::types::{
    BloomDragCtx, DragAxisMode, DragCtx, HitNode, ModState, NODE_DOUBLE_CLICK_MS,
    OverflowDragCtx, PointerState, ResizeCtx, TitleClickCtx,
};
use crate::overlay::{bloom_token_hit_test, cluster_overflow_icon_hit_test};
use crate::render::bearing_hit_test;
use crate::render::world_to_screen;
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::{ActiveDragState, Halley};
use crate::state::{PendingCoreClick, PendingCorePress};
use crate::surface_ops::{
    current_surface_size_for_node, request_toplevel_resize_mode, window_geometry_for_node,
};
use smithay::backend::input::ButtonState;

use super::key_actions::{
    apply_bound_pointer_input, apply_compositor_action_press, compositor_binding_action_active,
};
use super::pointer_focus::{layer_surface_focus_for_screen, pointer_focus_for_screen};
use super::resize_helpers::{
    active_node_screen_rect, handle_from_press_position, weights_from_handle,
};
use super::utils::modifier_active;

#[inline]
fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

#[inline]
fn clamp_screen_to_workspace(ws_w: i32, ws_h: i32, sx: f32, sy: f32) -> (f32, f32) {
    let max_x = (ws_w.max(1) - 1) as f32;
    let max_y = (ws_h.max(1) - 1) as f32;
    (sx.clamp(0.0, max_x), sy.clamp(0.0, max_y))
}

#[derive(Clone, Copy)]
pub(super) struct ButtonFrame {
    pub(super) ws_w: i32,
    pub(super) ws_h: i32,
    pub(super) global_sx: f32,
    pub(super) global_sy: f32,
    pub(super) sx: f32,
    pub(super) sy: f32,
    pub(super) world_now: halley_core::field::Vec2,
    pub(super) workspace_active: bool,
}

fn dispatch_pointer_button(
    st: &mut Halley,
    frame: ButtonFrame,
    resize_preview: Option<ResizeCtx>,
    button_code: u32,
    button_state: ButtonState,
) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    let focus = pointer_focus_for_screen(
        st,
        frame.ws_w,
        frame.ws_h,
        frame.sx,
        frame.sy,
        Instant::now(),
        resize_preview,
    );
    let motion_serial = SERIAL_COUNTER.next_serial();
    let button_serial = SERIAL_COUNTER.next_serial();
    let location = if focus
        .as_ref()
        .is_some_and(|(surface, _)| st.is_layer_surface(surface))
    {
        (frame.sx as f64, frame.sy as f64).into()
    } else {
        let cam_scale = st.camera_render_scale() as f64;
        (frame.sx as f64 / cam_scale, frame.sy as f64 / cam_scale).into()
    };
    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location,
            serial: motion_serial,
            time: now_millis_u32(),
        },
    );
    pointer.button(
        st,
        &ButtonEvent {
            serial: button_serial,
            time: now_millis_u32(),
            button: button_code,
            state: button_state,
        },
    );
    pointer.frame(st);
}

#[inline]
fn matching_pointer_binding(
    st: &Halley,
    mods: &ModState,
    button_code: u32,
) -> Option<PointerBindingAction> {
    fn modifier_specificity(modifiers: KeyModifiers) -> u32 {
        [
            modifiers.super_key,
            modifiers.left_super,
            modifiers.right_super,
            modifiers.alt,
            modifiers.left_alt,
            modifiers.right_alt,
            modifiers.ctrl,
            modifiers.left_ctrl,
            modifiers.right_ctrl,
            modifiers.shift,
            modifiers.left_shift,
            modifiers.right_shift,
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count() as u32
    }

    st.runtime
        .tuning
        .pointer_bindings
        .iter()
        .filter(|binding| binding.button == button_code && modifier_active(mods, binding.modifiers))
        .max_by_key(|binding| modifier_specificity(binding.modifiers))
        .map(|binding| binding.action)
}

fn title_click_is_double(
    ps: &PointerState,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    ps.last_title_click.is_some_and(|last| {
        last.node_id == node_id
            && now.duration_since(last.at).as_millis() as u64 <= NODE_DOUBLE_CLICK_MS
    })
}

fn set_title_click(ps: &mut PointerState, node_id: halley_core::field::NodeId, now: Instant) {
    ps.last_title_click = Some(TitleClickCtx { node_id, at: now });
}

fn clear_pointer_activity(st: &mut Halley, ps: &mut PointerState) {
    if let Some(drag) = ps.drag {
        st.set_drag_authority_node(None);
        st.end_carry_state_tracking(drag.node_id);
    }
    st.clear_grabbed_edge_pan_state();
    st.input.interaction_state.active_drag = None;
    st.input.interaction_state.pending_core_press = None;
    ps.drag = None;
    ps.overflow_drag = None;
    ps.resize = None;
    ps.panning = false;
    ps.pan_monitor = None;
}

fn collapse_bloom_for_core_if_open(st: &mut Halley, node_id: halley_core::field::NodeId) -> bool {
    let Some(cid) = st.model.field.cluster_id_for_core_public(node_id) else {
        return false;
    };
    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    if st.cluster_bloom_for_monitor(monitor.as_str()) != Some(cid) {
        return false;
    }
    st.close_cluster_bloom_for_monitor(monitor.as_str())
}

fn node_is_pointer_draggable(st: &Halley, node_id: halley_core::field::NodeId) -> bool {
    st.model.field.node(node_id).is_some_and(|n| match n.kind {
        halley_core::field::NodeKind::Surface => st.model.field.is_visible(node_id),
        halley_core::field::NodeKind::Core => n.state == halley_core::field::NodeState::Core,
    })
}

pub(super) fn begin_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
    world_now: halley_core::field::Vec2,
    allow_monitor_transfer: bool,
) {
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.pending_core_click = None;
    let drag_monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&hit.node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let edge_pan_eligible = st
        .node_fully_visible_on_monitor(drag_monitor.as_str(), hit.node_id)
        .unwrap_or(false);
    let mut drag_ctx = DragCtx {
        node_id: hit.node_id,
        allow_monitor_transfer,
        edge_pan_eligible,
        current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        center_latched: false,
        started_active: false,
        edge_pan_x: DragAxisMode::Free,
        edge_pan_y: DragAxisMode::Free,
        edge_pan_pressure: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        last_pointer_world: world_now,
        last_update_at: Instant::now(),
        release_velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
    };
    if let Some(n) = st.model.field.node(hit.node_id) {
        drag_ctx.started_active = n.state == halley_core::field::NodeState::Active;
        let off = halley_core::field::Vec2 {
            x: world_now.x - n.pos.x,
            y: world_now.y - n.pos.y,
        };
        if st.runtime.tuning.center_window_to_mouse {
            drag_ctx.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
            drag_ctx.center_latched = true;
        } else {
            drag_ctx.current_offset = off;
        }
    }
    ps.drag = Some(drag_ctx);
    let _ = st.model.field.set_pinned(hit.node_id, false);
    st.assign_node_to_monitor(hit.node_id, drag_monitor.as_str());
    st.input
        .interaction_state
        .physics_velocity
        .remove(&hit.node_id);
    st.input.interaction_state.drag_authority_velocity =
        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
    st.clear_grabbed_edge_pan_state();
    st.input.interaction_state.active_drag = Some(ActiveDragState {
        node_id: hit.node_id,
        allow_monitor_transfer,
        edge_pan_eligible,
        current_offset: drag_ctx.current_offset,
        pointer_monitor: drag_monitor,
        pointer_workspace_size: (frame.ws_w, frame.ws_h),
        pointer_screen_local: (frame.sx, frame.sy),
        edge_pan_x: DragAxisMode::Free,
        edge_pan_y: DragAxisMode::Free,
    });
    st.set_drag_authority_node(Some(hit.node_id));
    st.begin_carry_state_tracking(hit.node_id);
    if !hit.is_core {
        st.set_interaction_focus(Some(hit.node_id), 30_000, Instant::now());
    }
    if edge_pan_eligible {
        let to = halley_core::field::Vec2 {
            x: world_now.x - drag_ctx.current_offset.x,
            y: world_now.y - drag_ctx.current_offset.y,
        };
        let _ = st.carry_surface_non_overlap(hit.node_id, to, false);
    }
    backend.request_redraw();
}

/// Begin an interactive resize.
///
/// **Edge grabs** (pointer within 28 px of a window border): handle is
/// committed immediately from the nearest border, same as before.
///
/// **Interior/binding grabs**: handle starts as `Pending`. The motion handler
/// locks it from the drag direction the first time the pointer travels past
/// the dead zone. Until then the window does not move.
fn begin_resize(
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

    // Commit the handle immediately from where the pointer is in the window.
    // 3×3 grid: outer thirds = edges/corners, centre = nearest edge.
    // Pressing near top-left and dragging any direction pulls that corner.
    let handle = handle_from_press_position(rect, (frame.sx, frame.sy));
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
        info!(
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

fn finalize_resize(st: &mut Halley, ps: &mut PointerState, backend: &dyn BackendView) {
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
        info!(
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

fn handle_workspace_left_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
) {
    let now = Instant::now();
    let monitor = st.model.monitor_state.current_monitor.clone();
    if let Some(rect) = st.cluster_overflow_rect_for_monitor(monitor.as_str()) {
        let (.., local_sx, local_sy) =
            st.local_screen_in_monitor(monitor.as_str(), ps.screen.0, ps.screen.1);
        let inside = local_sx >= rect.x
            && local_sx <= rect.x + rect.w
            && local_sy >= rect.y
            && local_sy <= rect.y + rect.h;
        if inside {
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), st.now_ms(now));
        } else {
            st.hide_cluster_overflow_for_monitor(monitor.as_str());
        }
    }
    let focus_hold_ms = if hit.on_titlebar || hit.is_core {
        700
    } else {
        30_000
    };
    st.set_interaction_focus(Some(hit.node_id), focus_hold_ms, now);
    if hit.on_titlebar || hit.is_core {
        if title_click_is_double(ps, hit.node_id, now) {
            let _ = st.exit_cluster_workspace_if_member(hit.node_id, now);
            ps.last_title_click = None;
            clear_pointer_activity(st, ps);
            backend.request_redraw();
        } else {
            set_title_click(ps, hit.node_id, now);
            backend.request_redraw();
        }
    } else {
        ps.last_title_click = None;
        backend.request_redraw();
    }
}

fn restore_fullscreen_click_focus(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    if !st.is_fullscreen_active(node_id) {
        return false;
    }

    let monitor_name = st
        .fullscreen_monitor_for_node(node_id)
        .map(str::to_owned)
        .or_else(|| st.model.monitor_state.node_monitor.get(&node_id).cloned())
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());

    let entry = st
        .model
        .fullscreen_state
        .fullscreen_restore
        .get(&node_id)
        .copied();
    let fallback_center = st
        .model
        .monitor_state
        .monitors
        .get(monitor_name.as_str())
        .map(|space| space.viewport.center)
        .unwrap_or(st.model.viewport.center);
    let target_center = st
        .model
        .field
        .node(node_id)
        .map(|node| node.pos)
        .or_else(|| entry.map(|e| e.viewport_center))
        .unwrap_or(fallback_center);

    st.set_interaction_monitor(monitor_name.as_str());
    let _ = st.activate_monitor(monitor_name.as_str());
    if let Some(space) = st
        .model
        .monitor_state
        .monitors
        .get_mut(monitor_name.as_str())
    {
        let one_x_zoom = halley_core::field::Vec2 {
            x: space.width as f32,
            y: space.height as f32,
        };
        space.viewport.center = target_center;
        space.camera_target_center = target_center;
        space.viewport.size = one_x_zoom;
        space.zoom_ref_size = one_x_zoom;
        space.camera_target_view_size = one_x_zoom;
    }
    if st.model.monitor_state.current_monitor == monitor_name {
        let one_x_zoom = st
            .model
            .monitor_state
            .monitors
            .get(monitor_name.as_str())
            .map(|space| halley_core::field::Vec2 {
                x: space.width as f32,
                y: space.height as f32,
            })
            .unwrap_or(st.model.viewport.size);
        st.model.viewport.center = target_center;
        st.model.camera_target_center = target_center;
        st.model.viewport.size = one_x_zoom;
        st.model.zoom_ref_size = one_x_zoom;
        st.model.camera_target_view_size = one_x_zoom;
        st.runtime.tuning.viewport_center = target_center;
        st.runtime.tuning.viewport_size = one_x_zoom;
        st.input.interaction_state.viewport_pan_anim = None;
    }

    st.set_interaction_focus(Some(node_id), 30_000, now);
    true
}

fn handle_left_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    drag_binding_active: bool,
    allow_monitor_transfer: bool,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    let Some(hit) = hit else {
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        let _ = st.close_cluster_bloom_for_monitor(monitor.as_str());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        ps.last_title_click = None;
        backend.request_redraw();
        return;
    };
    if frame.workspace_active {
        handle_workspace_left_press(st, ps, backend, hit);
        return;
    }

    if !drag_binding_active && hit.is_core {
        let now = Instant::now();
        st.set_interaction_focus(Some(hit.node_id), 700, now);
        let was_bloom_open = collapse_bloom_for_core_if_open(st, hit.node_id);
        let now_ms = st.now_ms(now);
        if st
            .input
            .interaction_state
            .pending_core_click
            .as_ref()
            .is_some_and(|pending| {
                pending.node_id == hit.node_id
                    && pending.monitor == st.model.monitor_state.current_monitor
                    && pending.deadline_ms > now_ms
            })
        {
            let _ = st.toggle_cluster_workspace_by_core(hit.node_id, now);
            st.input.interaction_state.pending_core_click = None;
            ps.last_title_click = None;
        } else {
            st.input.interaction_state.pending_core_press = Some(PendingCorePress {
                node_id: hit.node_id,
                monitor: st.model.monitor_state.current_monitor.clone(),
                press_global_sx: frame.global_sx,
                press_global_sy: frame.global_sy,
                reopen_bloom_on_timeout: !was_bloom_open,
            });
        }
        backend.request_redraw();
        return;
    }

    if !drag_binding_active && restore_fullscreen_click_focus(st, hit.node_id, Instant::now()) {
        ps.last_title_click = None;
        backend.request_redraw();
    }

    if !drag_binding_active
        && st.model.field.node(hit.node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && st.model.field.is_visible(hit.node_id)
        })
    {
        st.set_interaction_focus(Some(hit.node_id), 30_000, Instant::now());
    }

    let mut handled_node_click = false;
    if !drag_binding_active && !hit.on_titlebar && !hit.is_core {
        let is_node = st
            .model
            .field
            .node(hit.node_id)
            .is_some_and(|n| n.state == halley_core::field::NodeState::Node);
        if is_node {
            let now = Instant::now();
            if activate_collapsed_node_from_click(st, hit.node_id, now) {
                backend.request_redraw();
            }
            ps.last_title_click = None;
            handled_node_click = true;
        }
    }

    let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
    if drag_binding_active && drag_target_ok && !handled_node_click {
        if hit.is_core {
            let _ = collapse_bloom_for_core_if_open(st, hit.node_id);
        }
        begin_drag(
            st,
            ps,
            backend,
            hit,
            frame,
            frame.world_now,
            allow_monitor_transfer,
        );
        return;
    }

    if hit.on_titlebar || hit.is_core {
        let now = Instant::now();
        st.set_interaction_focus(Some(hit.node_id), 700, now);
        if !hit.is_core && title_click_is_double(ps, hit.node_id, now) {
            ps.last_title_click = None;
            backend.request_redraw();
        } else {
            if !hit.is_core {
                set_title_click(ps, hit.node_id, now);
            }
        }
    } else if !handled_node_click {
        ps.last_title_click = None;
    }
}

fn handle_right_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    resize_binding_active: bool,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    if frame.workspace_active {
        clear_pointer_activity(st, ps);
        return;
    }

    let Some(hit) = hit else {
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        backend.request_redraw();
        return;
    };
    let can_resize = st
        .model
        .field
        .node(hit.node_id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
    if resize_binding_active && can_resize {
        begin_resize(st, ps, backend, hit, frame);
    }
}

fn handle_move_binding_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: Option<HitNode>,
    frame: ButtonFrame,
    allow_monitor_transfer: bool,
) {
    if frame.workspace_active {
        clear_pointer_activity(st, ps);
        return;
    }

    let Some(hit) = hit else {
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        ps.last_title_click = None;
        backend.request_redraw();
        return;
    };
    let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
    if drag_target_ok {
        if hit.is_core {
            let _ = collapse_bloom_for_core_if_open(st, hit.node_id);
        }
        begin_drag(
            st,
            ps,
            backend,
            hit,
            frame,
            frame.world_now,
            allow_monitor_transfer,
        );
    }
}

fn handle_resize_binding_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    if frame.workspace_active {
        clear_pointer_activity(st, ps);
        return;
    }

    let Some(hit) = hit else {
        let now = Instant::now();
        let monitor = st
            .monitor_for_screen(frame.global_sx, frame.global_sy)
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
        st.focus_monitor_view(monitor.as_str(), now);
        ps.panning = true;
        ps.pan_monitor = Some(monitor);
        ps.pan_last_screen = (frame.global_sx, frame.global_sy);
        backend.request_redraw();
        return;
    };
    let can_resize = st
        .model
        .field
        .node(hit.node_id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
    if can_resize {
        // Binding-triggered resize always starts Pending regardless of where
        // the cursor is — drag direction picks the handle.
        begin_resize(st, ps, backend, hit, frame);
    }
}

fn handle_button_release(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    button_code: u32,
    action: Option<PointerBindingAction>,
    _world_now: halley_core::field::Vec2,
) {
    match action {
        Some(PointerBindingAction::MoveWindow | PointerBindingAction::FieldJump) => {
            if let Some(d) = ps.drag {
                let now = Instant::now();
                st.clear_grabbed_edge_pan_state();
                st.input.interaction_state.active_drag = None;
                let joined = st.commit_ready_cluster_join_for_node(d.node_id, now);
                if !joined {
                    if d.started_active {
                        st.finalize_mouse_drag_state(
                            d.node_id,
                            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                            now,
                        );
                    } else {
                        st.update_carry_state_preview(d.node_id, now);
                    }
                } else {
                    st.input.interaction_state.cluster_join_candidate = None;
                }
                st.set_drag_authority_node(None);
                st.end_carry_state_tracking(d.node_id);
                ps.preview_block_until = Some(now + Duration::from_millis(360));
            }
            st.input.interaction_state.active_drag = None;
            ps.drag = None;
            ps.panning = false;
            ps.pan_monitor = None;
            if ps.resize.is_some() {
                finalize_resize(st, ps, backend);
            }
        }
        Some(PointerBindingAction::ResizeWindow) => {
            finalize_resize(st, ps, backend);
        }
        None => {
            if button_code == 0x110
                && let Some(d) = ps.drag
            {
                let now = Instant::now();
                st.clear_grabbed_edge_pan_state();
                st.input.interaction_state.active_drag = None;
                let joined = st.commit_ready_cluster_join_for_node(d.node_id, now);
                if !joined {
                    if d.started_active {
                        st.finalize_mouse_drag_state(
                            d.node_id,
                            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                            now,
                        );
                    } else {
                        st.update_carry_state_preview(d.node_id, now);
                    }
                } else {
                    st.input.interaction_state.cluster_join_candidate = None;
                }
                st.set_drag_authority_node(None);
                st.end_carry_state_tracking(d.node_id);
                ps.preview_block_until = Some(now + Duration::from_millis(360));
                st.input.interaction_state.active_drag = None;
                ps.drag = None;
            }
            if button_code == 0x110 || button_code == 0x111 {
                ps.panning = false;
                ps.pan_monitor = None;
            }
        }
    }
}

pub(crate) fn handle_pointer_button_input(
    st: &mut Halley,
    backend: &impl BackendView,
    mod_state: &Rc<RefCell<ModState>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    config_path: &str,
    wayland_display: &str,
    button_code: u32,
    button_state: ButtonState,
) {
    let left = button_code == 0x110;
    let right = button_code == 0x111;
    let mut ps = pointer_state.borrow_mut();
    let (ws_w, ws_h) = backend.window_size_i32();
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, ps.screen.0, ps.screen.1);
    let target_monitor = st
        .active_locked_pointer_surface()
        .and_then(|surface| {
            let node_id = st.model.surface_to_node.get(&surface.id()).copied()?;
            Some(
                st.model
                    .monitor_state
                    .node_monitor
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone()),
            )
        })
        .unwrap_or_else(|| {
            st.monitor_for_screen(sx, sy)
                .unwrap_or_else(|| st.interaction_monitor().to_string())
        });
    st.set_interaction_monitor(target_monitor.as_str());
    let _ = st.activate_monitor(target_monitor.as_str());
    let (local_w, local_h, local_sx, local_sy) =
        st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
    ps.screen = (sx, sy);
    ps.workspace_size = (local_w, local_h);
    let layer_focus = layer_surface_focus_for_screen(
        st,
        local_w,
        local_h,
        local_sx,
        local_sy,
        Instant::now(),
        ps.resize,
    );
    if matches!(button_state, ButtonState::Pressed)
        && let Some((surface, _)) = layer_focus.as_ref()
    {
        let monitor = st.layer_surface_monitor_name(surface);
        st.model.spawn_state.pending_spawn_monitor = Some(monitor.clone());
        info!(
            "pending spawn monitor latched from layer press: {}",
            monitor
        );
    }
    let world_now = screen_to_world(st, local_w, local_h, local_sx, local_sy);
    let frame = ButtonFrame {
        ws_w: local_w,
        ws_h: local_h,
        global_sx: sx,
        global_sy: sy,
        sx: local_sx,
        sy: local_sy,
        world_now,
        workspace_active: st.has_active_cluster_workspace(),
    };
    if st.cluster_mode_active() {
        ps.world = world_now;
        match button_state {
            ButtonState::Pressed if left => {
                if let Some((surface, _)) = layer_focus.as_ref() {
                    let _ = st.focus_layer_surface(surface);
                    ps.last_title_click = None;
                    return;
                }
                let hit = pick_hit_node_at(
                    st,
                    local_w,
                    local_h,
                    local_sx,
                    local_sy,
                    Instant::now(),
                    ps.resize,
                );
                if let Some(hit) = hit {
                    let _ = st.toggle_cluster_mode_selection(hit.node_id);
                    ps.last_title_click = None;
                    backend.request_redraw();
                    return;
                }

                let now = Instant::now();
                let monitor = st
                    .monitor_for_screen(frame.global_sx, frame.global_sy)
                    .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
                st.focus_monitor_view(monitor.as_str(), now);
                ps.panning = true;
                ps.pan_monitor = Some(monitor);
                ps.pan_last_screen = (frame.global_sx, frame.global_sy);
                ps.last_title_click = None;
                backend.request_redraw();
                return;
            }
            ButtonState::Released if left || right => {
                handle_button_release(st, &mut ps, backend, button_code, None, world_now);
                return;
            }
            ButtonState::Pressed | ButtonState::Released => {
                return;
            }
        }
    }
    if matches!(button_state, ButtonState::Pressed)
        && left
        && !frame.workspace_active
        && let Some(layout) = bloom_token_hit_test(
            st,
            local_w,
            local_h,
            target_monitor.as_str(),
            local_sx,
            local_sy,
        )
    {
        ps.bloom_drag = Some(BloomDragCtx {
            cluster_id: layout.cluster_id,
            member_id: layout.member_id,
            monitor: target_monitor.clone(),
            core_screen: (layout.core_sx as f32, layout.core_sy as f32),
        });
        st.input.interaction_state.bloom_pull_preview = Some(crate::state::BloomPullPreview {
            cluster_id: layout.cluster_id,
            member_id: layout.member_id,
            mix: 0.0,
        });
        ps.last_title_click = None;
        backend.request_redraw();
        return;
    }
    if matches!(button_state, ButtonState::Pressed)
        && left
        && !frame.workspace_active
        && let Some(node_id) = bearing_hit_test(
            st,
            local_w,
            local_h,
            target_monitor.as_str(),
            local_sx,
            local_sy,
        )
    {
        let now = Instant::now();
        let _ = focus_or_reveal_surface_node(st, node_id, now);
        ps.last_title_click = None;
        ps.panning = false;
        backend.request_redraw();
        return;
    }
    if matches!(button_state, ButtonState::Pressed)
        && left
        && frame.workspace_active
        && let Some(cid) = st.active_cluster_workspace_for_monitor(target_monitor.as_str())
        && let Some(member_id) = cluster_overflow_icon_hit_test(
            &crate::overlay::OverlayView::from_halley(st),
            target_monitor.as_str(),
            local_sx,
            local_sy,
            st.now_ms(Instant::now()),
        )
    {
        ps.overflow_drag = Some(OverflowDragCtx {
            cluster_id: cid,
            member_id,
            monitor: target_monitor.clone(),
        });
        ps.last_title_click = None;
        backend.request_redraw();
        return;
    }
    let mods = mod_state.borrow().clone();
    let intercepted_binding = match button_state {
        ButtonState::Pressed => {
            if let Some(action) = compositor_binding_action_active(st, button_code, &mods) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                let _ = apply_compositor_action_press(st, action, config_path, wayland_display);
                backend.request_redraw();
                true
            } else if apply_bound_pointer_input(
                st,
                button_code,
                &mods,
                config_path,
                wayland_display,
            ) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                backend.request_redraw();
                true
            } else {
                false
            }
        }
        ButtonState::Released => ps.intercepted_binding_buttons.remove(&button_code),
    };
    let matched_action = match button_state {
        ButtonState::Pressed => matching_pointer_binding(st, &mods, button_code),
        ButtonState::Released => ps.intercepted_buttons.remove(&button_code),
    };
    let intercepted = intercepted_binding || matched_action.is_some();
    if matches!(button_state, ButtonState::Pressed)
        && let Some(action) = matched_action
    {
        ps.intercepted_buttons.insert(button_code, action);
    }
    if !intercepted {
        dispatch_pointer_button(st, frame, ps.resize, button_code, button_state);
    }
    ps.world = world_now;
    if matches!(button_state, ButtonState::Pressed)
        && !intercepted
        && let Some((surface, _)) = layer_focus
    {
        let _ = st.focus_layer_surface(&surface);
        ps.last_title_click = None;
        return;
    }
    match button_state {
        ButtonState::Pressed => {
            if intercepted_binding {
                return;
            }
            let hit = pick_hit_node_at(
                st,
                local_w,
                local_h,
                local_sx,
                local_sy,
                Instant::now(),
                ps.resize,
            );
            if left {
                handle_left_press(
                    st,
                    &mut ps,
                    backend,
                    matches!(
                        matched_action,
                        Some(PointerBindingAction::MoveWindow | PointerBindingAction::FieldJump)
                    ),
                    matches!(matched_action, Some(PointerBindingAction::FieldJump)),
                    hit,
                    frame,
                );
            } else if right {
                handle_right_press(
                    st,
                    &mut ps,
                    backend,
                    matches!(matched_action, Some(PointerBindingAction::ResizeWindow)),
                    hit,
                    frame,
                );
            } else {
                match matched_action {
                    Some(PointerBindingAction::MoveWindow) => {
                        handle_move_binding_press(st, &mut ps, backend, hit, frame, false);
                    }
                    Some(PointerBindingAction::FieldJump) => {
                        handle_move_binding_press(st, &mut ps, backend, hit, frame, true);
                    }
                    Some(PointerBindingAction::ResizeWindow) => {
                        handle_resize_binding_press(st, &mut ps, backend, hit, frame);
                    }
                    None => {}
                }
            }
        }
        ButtonState::Released => {
            if left
                && let Some(pending_press) = st.input.interaction_state.pending_core_press.take()
            {
                let now = Instant::now();
                st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
                    node_id: pending_press.node_id,
                    monitor: pending_press.monitor,
                    deadline_ms: st.now_ms(now).saturating_add(180),
                    reopen_bloom_on_timeout: pending_press.reopen_bloom_on_timeout,
                });
                st.request_maintenance();
                backend.request_redraw();
                return;
            }
            if left && ps.bloom_drag.take().is_some() {
                st.input.interaction_state.bloom_pull_preview = None;
                backend.request_redraw();
                return;
            }
            if left
                && let Some(overflow_drag) = ps.overflow_drag.take()
            {
                let now = Instant::now();
                let release_hit = pick_hit_node_at(
                    st,
                    local_w,
                    local_h,
                    local_sx,
                    local_sy,
                    now,
                    ps.resize,
                );
                if overflow_drag.monitor == target_monitor
                    && let Some(hit) = release_hit
                    && let Some(cluster) = st.model.field.cluster(overflow_drag.cluster_id)
                    && cluster.visible_members().contains(&hit.node_id)
                {
                    let swapped = st.swap_cluster_overflow_member_with_visible(
                        overflow_drag.monitor.as_str(),
                        overflow_drag.cluster_id,
                        overflow_drag.member_id,
                        hit.node_id,
                        st.now_ms(now),
                    );
                    if swapped {
                        backend.request_redraw();
                    }
                }
                return;
            }
            if intercepted_binding {
                return;
            }
            handle_button_release(st, &mut ps, backend, button_code, matched_action, world_now);
        }
    }
}
