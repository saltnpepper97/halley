use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eventline::info;
use halley_config::PointerBindingAction;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::interaction::actions::docking_mode_active;
use crate::interaction::actions::promote_node_level;
use crate::interaction::types::{
    DragCtx, HitNode, ModState, NODE_DOUBLE_CLICK_MS, PointerState, ResizeCtx, TitleClickCtx,
};
use crate::render::world_to_screen;
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::HalleyWlState;
use crate::surface::{
    current_surface_size_for_node, request_toplevel_resize_mode, window_geometry_for_node,
};
use smithay::backend::input::ButtonState;

use super::input_utils::modifier_active;
use super::key_actions::{apply_bound_key, apply_compositor_action_press, compositor_binding_action};
use super::pointer_focus::{layer_surface_focus_for_screen, pointer_focus_for_screen};
use super::pointer_map_debug_enabled;
use super::resize_helpers::{active_node_screen_rect, pick_resize_handle_from_screen};

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
struct ButtonFrame {
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    world_now: halley_core::field::Vec2,
    workspace_active: bool,
}

fn dispatch_pointer_button(
    st: &mut HalleyWlState,
    frame: ButtonFrame,
    resize_preview: Option<ResizeCtx>,
    button_code: u32,
    button_state: ButtonState,
) {
    let Some(pointer) = st.seat.get_pointer() else {
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
    let cam_scale = st.camera_render_scale() as f64;
    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location: (frame.sx as f64 / cam_scale, frame.sy as f64 / cam_scale).into(),
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
    st: &HalleyWlState,
    mods: &ModState,
    button_code: u32,
) -> Option<PointerBindingAction> {
    st.tuning
        .pointer_bindings
        .iter()
        .find(|binding| binding.button == button_code && modifier_active(mods, binding.modifiers))
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

fn clear_pointer_activity(_st: &mut HalleyWlState, ps: &mut PointerState) {
    ps.drag = None;
    ps.resize = None;
    ps.panning = false;
}

fn begin_drag(
    st: &mut HalleyWlState,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    world_now: halley_core::field::Vec2,
) {
    let mut drag_ctx = DragCtx {
        node_id: hit.node_id,
        current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
        center_latched: false,
        started_active: false,
    };
    if let Some(n) = st.field.node(hit.node_id) {
        drag_ctx.started_active = n.state == halley_core::field::NodeState::Active;
        let off = halley_core::field::Vec2 {
            x: world_now.x - n.pos.x,
            y: world_now.y - n.pos.y,
        };
        if st.tuning.center_window_to_mouse {
            drag_ctx.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
            drag_ctx.center_latched = true;
        } else {
            drag_ctx.current_offset = off;
        }
    }
    ps.drag = Some(drag_ctx);
    let _ = st.field.set_pinned(hit.node_id, false);
    st.begin_carry_state_tracking(hit.node_id, docking_mode_active(st));
    st.set_interaction_focus(Some(hit.node_id), 30_000, Instant::now());
    let to = halley_core::field::Vec2 {
        x: world_now.x - drag_ctx.current_offset.x,
        y: world_now.y - drag_ctx.current_offset.y,
    };
    let _ = st.carry_surface_non_overlap(hit.node_id, to, docking_mode_active(st));
    backend.request_redraw();
}

fn begin_resize(
    st: &mut HalleyWlState,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
    let Some(n) = st.field.node(hit.node_id) else {
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
    let handle = pick_resize_handle_from_screen(
        (start_left, start_top, start_right, start_bottom),
        (frame.sx, frame.sy),
    );
    ps.drag = None;
    ps.panning = false;
    ps.move_anim.clear();
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
        press_off_left_px: frame.sx - start_left,
        press_off_right_px: frame.sx - start_right,
        press_off_top_px: frame.sy - start_top,
        press_off_bottom_px: frame.sy - start_bottom,
        drag_started: true,
        resize_mode_sent: false,
        live_geo_lx: 0.0,
        live_geo_ly: 0.0,
        live_geo_w: 0.0,
        live_geo_h: 0.0,
    };
    if st.tuning.debug_tick_dump {
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

fn finalize_resize(st: &mut HalleyWlState, ps: &mut PointerState, backend: &dyn BackendView) {
    let ended_resize = ps.resize.take();
    ps.panning = false;
    let Some(resize) = ended_resize else {
        return;
    };

    let now = Instant::now();
    ps.move_anim.clear();
    if st.tuning.debug_tick_dump {
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
    if st.tuning.debug_tick_dump {
        info!(
            "resize-end id={} preview=({:.1},{:.1},{:.1},{:.1}) frozen_geo=({:.1},{:.1}) final_surface=({}, {}) final_bbox=({:.1}, {:.1})",
            resize.node_id.as_u64(),
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
    if let Some(n) = st.field.node_mut(resize.node_id) {
        n.intrinsic_size.x = final_bbox_w;
        n.intrinsic_size.y = final_bbox_h;
    }
    let _ = st.field.sync_active_footprint_to_intrinsic(resize.node_id);
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
    st: &mut HalleyWlState,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
) {
    if !(hit.on_titlebar || hit.is_core) {
        ps.last_title_click = None;
        return;
    }

    let now = Instant::now();
    st.set_interaction_focus(Some(hit.node_id), 700, now);
    if title_click_is_double(ps, hit.node_id, now) {
        let _ = st.exit_cluster_workspace_if_member(hit.node_id, now);
        ps.last_title_click = None;
        clear_pointer_activity(st, ps);
        backend.request_redraw();
    } else {
        set_title_click(ps, hit.node_id, now);
    }
}

fn handle_left_press(
    st: &mut HalleyWlState,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    drag_binding_active: bool,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    let Some(hit) = hit else {
        ps.panning = true;
        ps.pan_last_screen = (frame.sx, frame.sy);
        ps.last_title_click = None;
        return;
    };
    if frame.workspace_active {
        handle_workspace_left_press(st, ps, backend, hit);
        return;
    }

    if !drag_binding_active
        && st.field.node(hit.node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface && st.field.is_visible(hit.node_id)
        })
    {
        st.set_interaction_focus(Some(hit.node_id), 30_000, Instant::now());
    }

    let mut handled_node_click = false;
    if !drag_binding_active && !hit.on_titlebar && !hit.is_core {
        let is_node = st
            .field
            .node(hit.node_id)
            .is_some_and(|n| n.state == halley_core::field::NodeState::Node);
        if is_node {
            let now = Instant::now();
            if promote_node_level(st, hit.node_id, now) {
                backend.request_redraw();
            }
            ps.last_title_click = None;
            handled_node_click = true;
        }
    }

    let drag_target_ok = st.field.node(hit.node_id).is_some_and(|n| {
        n.kind == halley_core::field::NodeKind::Surface && st.field.is_visible(hit.node_id)
    });
    if drag_binding_active && drag_target_ok && !handled_node_click {
        begin_drag(st, ps, backend, hit, frame.world_now);
    }

    if hit.on_titlebar || hit.is_core {
        let now = Instant::now();
        st.set_interaction_focus(Some(hit.node_id), 700, now);
        if title_click_is_double(ps, hit.node_id, now) {
            if hit.is_core {
                let _ = st.toggle_cluster_workspace_by_core(hit.node_id, now);
            }
            ps.last_title_click = None;
            backend.request_redraw();
        } else {
            set_title_click(ps, hit.node_id, now);
        }
    } else if !handled_node_click {
        ps.last_title_click = None;
    }
}

fn handle_right_press(
    st: &mut HalleyWlState,
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
        ps.panning = true;
        ps.pan_last_screen = (frame.sx, frame.sy);
        return;
    };
    let can_resize = st
        .field
        .node(hit.node_id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
    if resize_binding_active && can_resize {
        begin_resize(st, ps, backend, hit, frame);
    }
}

fn handle_move_binding_press(
    st: &mut HalleyWlState,
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
        ps.panning = true;
        ps.pan_last_screen = (frame.sx, frame.sy);
        ps.last_title_click = None;
        return;
    };
    let drag_target_ok = st.field.node(hit.node_id).is_some_and(|n| {
        n.kind == halley_core::field::NodeKind::Surface && st.field.is_visible(hit.node_id)
    });
    if drag_target_ok {
        begin_drag(st, ps, backend, hit, frame.world_now);
    }
}

fn handle_resize_binding_press(
    st: &mut HalleyWlState,
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
        ps.panning = true;
        ps.pan_last_screen = (frame.sx, frame.sy);
        return;
    };
    let can_resize = st
        .field
        .node(hit.node_id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
    if can_resize {
        begin_resize(st, ps, backend, hit, frame);
    }
}

fn handle_button_release(
    st: &mut HalleyWlState,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    button_code: u32,
    action: Option<PointerBindingAction>,
    world_now: halley_core::field::Vec2,
) {
    match action {
        Some(PointerBindingAction::MoveWindow) => {
            if let Some(d) = ps.drag {
                let now = Instant::now();
                if d.started_active {
                    st.finalize_mouse_drag_state(d.node_id, world_now, now);
                } else {
                    st.update_carry_state_preview_at(d.node_id, world_now, now);
                }
                st.end_carry_state_tracking(d.node_id);
                ps.preview_block_until = Some(now + Duration::from_millis(360));
            }
            ps.drag = None;
            ps.panning = false;
            if ps.resize.is_some() {
                finalize_resize(st, ps, backend);
            }
        }
        Some(PointerBindingAction::ResizeWindow) => {
            finalize_resize(st, ps, backend);
        }
        None => {
            if button_code == 0x110 || button_code == 0x111 {
                ps.panning = false;
            }
        }
    }
}

pub(crate) fn handle_pointer_button_input(
    st: &mut HalleyWlState,
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
    ps.screen = (sx, sy);
    ps.workspace_size = (ws_w, ws_h);
    let layer_focus = layer_surface_focus_for_screen(st, ws_w, ws_h, sx, sy);
    let world_now = screen_to_world(st, ws_w, ws_h, sx, sy);
    let frame = ButtonFrame {
        ws_w,
        ws_h,
        sx,
        sy,
        world_now,
        workspace_active: st.has_active_cluster_workspace(),
    };
    let mods = mod_state.borrow().clone();
    let intercepted_binding = match button_state {
        ButtonState::Pressed => {
            if let Some(action) = compositor_binding_action(st, button_code, &mods) {
                ps.intercepted_binding_buttons.insert(button_code);
                ps.panning = false;
                let _ = apply_compositor_action_press(st, action, config_path, wayland_display);
                backend.request_redraw();
                true
            } else if apply_bound_key(st, button_code, &mods, config_path, wayland_display) {
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
            let hit = pick_hit_node_at(st, ws_w, ws_h, sx, sy, Instant::now(), ps.resize);
            if pointer_map_debug_enabled() {
                info!(
                    "ptr-map button-press code=0x{:x} ws={}x{} screen=({:.2},{:.2}) world=({:.2},{:.2}) hit={:?}",
                    button_code,
                    ws_w,
                    ws_h,
                    sx,
                    sy,
                    world_now.x,
                    world_now.y,
                    hit.map(|h| (h.node_id.as_u64(), h.on_titlebar, h.is_core))
                );
            }
            if left {
                handle_left_press(
                    st,
                    &mut ps,
                    backend,
                    matches!(matched_action, Some(PointerBindingAction::MoveWindow)),
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
                        handle_move_binding_press(st, &mut ps, backend, hit, frame);
                    }
                    Some(PointerBindingAction::ResizeWindow) => {
                        handle_resize_binding_press(st, &mut ps, backend, hit, frame);
                    }
                    None => {}
                }
            }
        }
        ButtonState::Released => {
            if intercepted_binding {
                return;
            }
            if pointer_map_debug_enabled() {
                info!(
                    "ptr-map button-release code=0x{:x} ws={}x{} screen=({:.2},{:.2}) world=({:.2},{:.2}) drag={} resize={} panning={}",
                    button_code,
                    ws_w,
                    ws_h,
                    sx,
                    sy,
                    world_now.x,
                    world_now.y,
                    ps.drag.is_some(),
                    ps.resize.is_some(),
                    ps.panning
                );
            }
            handle_button_release(st, &mut ps, backend, button_code, matched_action, world_now);
        }
    }
}
