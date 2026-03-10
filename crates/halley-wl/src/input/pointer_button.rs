use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eventline::info;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend_iface::BackendView;
use crate::interaction::actions::promote_node_level;
use crate::interaction::types::{
    DragCtx, ModState, NODE_DOUBLE_CLICK_MS, PointerState, ResizeCtx, TitleClickCtx,
};
use crate::runtime_render::world_to_screen;
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::HalleyWlState;
use crate::surface::{current_surface_size_for_node, request_toplevel_resize_mode};
use smithay::backend::input::ButtonState;

use super::input_utils::modifier_active;
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

pub(crate) fn handle_pointer_button_input(
    st: &mut HalleyWlState,
    backend: &impl BackendView,
    mod_state: &Rc<RefCell<ModState>>,
    pointer_state: &Rc<RefCell<PointerState>>,
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
    if let Some(pointer) = st.seat.get_pointer() {
        let resize_preview = ps.resize;
        let focus =
            pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, Instant::now(), resize_preview);
        let motion_serial = SERIAL_COUNTER.next_serial();
        let button_serial = SERIAL_COUNTER.next_serial();
        pointer.motion(
            st,
            focus,
            &MotionEvent {
                location: (sx as f64, sy as f64).into(),
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
    let world_now = screen_to_world(st, ws_w, ws_h, sx, sy);
    ps.world = world_now;
    if !left && !right {
        return;
    }
    if matches!(button_state, ButtonState::Pressed) {
        if let Some((surface, _)) = layer_focus {
            let _ = st.focus_layer_surface(&surface);
            ps.last_title_click = None;
            return;
        }
    }
    let workspace_active = st.has_active_cluster_workspace();
    match button_state {
        ButtonState::Pressed => {
            let mods = mod_state.borrow().clone();
            let hit = pick_hit_node_at(st, ws_w, ws_h, sx, sy, Instant::now());
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
                if let Some(h) = hit {
                    if workspace_active {
                        if h.on_titlebar || h.is_core {
                            let now = Instant::now();
                            st.set_interaction_focus(Some(h.node_id), 700, now);
                            let double_click = ps.last_title_click.is_some_and(|last| {
                                last.node_id == h.node_id
                                    && now.duration_since(last.at).as_millis() as u64
                                        <= NODE_DOUBLE_CLICK_MS
                            });
                            if double_click {
                                let _ = st.exit_cluster_workspace_if_member(h.node_id, now);
                                ps.last_title_click = None;
                                ps.drag = None;
                                st.field.clear_dock_preview();
                                ps.resize = None;
                                ps.panning = false;
                                backend.request_redraw();
                            } else {
                                ps.last_title_click = Some(TitleClickCtx {
                                    node_id: h.node_id,
                                    at: now,
                                });
                            }
                        } else {
                            ps.last_title_click = None;
                        }
                        return;
                    }
                    let drag_mod_ok = modifier_active(&mods, st.tuning.keybinds.modifier);
                    if !drag_mod_ok
                        && st.field.node(h.node_id).is_some_and(|n| {
                            n.kind == halley_core::field::NodeKind::Surface
                                && st.field.is_visible(h.node_id)
                        })
                    {
                        st.set_interaction_focus(Some(h.node_id), 30_000, Instant::now());
                    }
                    let mut handled_node_click = false;
                    if !drag_mod_ok && !h.on_titlebar && !h.is_core {
                        let is_node = st
                            .field
                            .node(h.node_id)
                            .is_some_and(|n| n.state == halley_core::field::NodeState::Node);
                        if is_node {
                            let now = Instant::now();
                            if promote_node_level(st, h.node_id, now) {
                                backend.request_redraw();
                            }
                            ps.last_title_click = None;
                            handled_node_click = true;
                        }
                    }
                    let drag_target_ok = st.field.node(h.node_id).is_some_and(|n| {
                        n.kind == halley_core::field::NodeKind::Surface
                            && st.field.is_visible(h.node_id)
                    });
                    if drag_mod_ok && drag_target_ok && !handled_node_click {
                        let mut drag_ctx = DragCtx {
                            node_id: h.node_id,
                            current_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                            center_latched: false,
                            started_active: false,
                        };
                        if let Some(n) = st.field.node(h.node_id) {
                            let is_active = n.state == halley_core::field::NodeState::Active;
                            drag_ctx.started_active = is_active;
                            let off = halley_core::field::Vec2 {
                                x: world_now.x - n.pos.x,
                                y: world_now.y - n.pos.y,
                            };
                            if st.tuning.center_window_to_mouse {
                                drag_ctx.current_offset =
                                    halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                                drag_ctx.center_latched = true;
                            } else {
                                drag_ctx.current_offset = off;
                                drag_ctx.center_latched = false;
                            }
                        }
                        ps.drag = Some(drag_ctx);
                        st.begin_carry_state_tracking(h.node_id);
                        st.set_interaction_focus(Some(h.node_id), 30_000, Instant::now());
                        let to = halley_core::field::Vec2 {
                            x: world_now.x - drag_ctx.current_offset.x,
                            y: world_now.y - drag_ctx.current_offset.y,
                        };
                        let _ = st.carry_surface_non_overlap(h.node_id, to);
                        backend.request_redraw();
                    }

                    if h.on_titlebar || h.is_core {
                        let now = Instant::now();
                        st.set_interaction_focus(Some(h.node_id), 700, now);
                        let double_click = ps.last_title_click.is_some_and(|last| {
                            last.node_id == h.node_id
                                && now.duration_since(last.at).as_millis() as u64
                                    <= NODE_DOUBLE_CLICK_MS
                        });
                        if double_click {
                            if h.is_core {
                                let _ = st.toggle_cluster_workspace_by_core(h.node_id, now);
                            }
                            ps.last_title_click = None;
                            backend.request_redraw();
                        } else {
                            ps.last_title_click = Some(TitleClickCtx {
                                node_id: h.node_id,
                                at: now,
                            });
                        }
                    } else if !handled_node_click {
                        ps.last_title_click = None;
                    }
                } else {
                    ps.panning = true;
                    ps.pan_last_screen = (sx, sy);
                    ps.last_title_click = None;
                }
            } else if right {
                if workspace_active {
                    ps.drag = None;
                    st.field.clear_dock_preview();
                    ps.resize = None;
                    ps.panning = false;
                    return;
                }
                if let Some(h) = hit {
                    let can_resize = st
                        .field
                        .node(h.node_id)
                        .is_some_and(|n| n.state == halley_core::field::NodeState::Active);
                    let resize_mod_ok = modifier_active(&mods, st.tuning.keybinds.modifier);
                    if resize_mod_ok && can_resize {
                        if let Some(n) = st.field.node(h.node_id) {
                            let fallback_size = n.intrinsic_size;
                            let fallback_pos = n.pos;
                            let (start_left, start_top, start_right, start_bottom) =
                                active_node_screen_rect(
                                    st,
                                    ws_w,
                                    ws_h,
                                    h.node_id,
                                    Instant::now(),
                                    None,
                                )
                                .unwrap_or_else(|| {
                                    let center_scr = world_to_screen(
                                        st,
                                        ws_w,
                                        ws_h,
                                        fallback_pos.x,
                                        fallback_pos.y,
                                    );
                                    (
                                        (center_scr.0 as f32) - fallback_size.x * 0.5,
                                        (center_scr.1 as f32) - fallback_size.y * 0.5,
                                        (center_scr.0 as f32) + fallback_size.x * 0.5,
                                        (center_scr.1 as f32) + fallback_size.y * 0.5,
                                    )
                                });
                            let handle = pick_resize_handle_from_screen(
                                (start_left, start_top, start_right, start_bottom),
                                (sx, sy),
                            );
                            ps.drag = None;
                            st.field.clear_dock_preview();
                            ps.panning = false;
                            ps.move_anim.clear();
                            st.begin_resize_interaction(h.node_id, Instant::now());
                            let start_w = (start_right - start_left).max(96.0).round() as i32;
                            let start_h = (start_bottom - start_top).max(72.0).round() as i32;
                            let start_surface = current_surface_size_for_node(st, h.node_id)
                                .unwrap_or(halley_core::field::Vec2 {
                                    x: start_w as f32,
                                    y: start_h as f32,
                                });
                            let start_bbox = halley_core::field::Vec2 {
                                x: fallback_size.x.max(1.0),
                                y: fallback_size.y.max(1.0),
                            };
                            ps.resize = Some(ResizeCtx {
                                node_id: h.node_id,
                                start_surface_w: start_surface.x.max(96.0).round() as i32,
                                start_surface_h: start_surface.y.max(72.0).round() as i32,
                                start_bbox_w: start_bbox.x.round() as i32,
                                start_bbox_h: start_bbox.y.round() as i32,
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
                                press_sx: sx,
                                press_sy: sy,
                                press_off_left_px: sx - start_left,
                                press_off_right_px: sx - start_right,
                                press_off_top_px: sy - start_top,
                                press_off_bottom_px: sy - start_bottom,
                                press_ws_w: ws_w,
                                press_ws_h: ws_h,
                                press_view_center: st.viewport.center,
                                press_view_size: st.viewport.size,
                                drag_started: true,
                                resize_mode_sent: false,
                            });
                            backend.request_redraw();
                        }
                    }
                } else {
                    ps.panning = true;
                    ps.pan_last_screen = (sx, sy);
                }
            }
        }
        ButtonState::Released => {
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
            let finalize_resize = |st: &mut HalleyWlState,
                                   ps: &mut PointerState,
                                   backend: &dyn BackendView| {
                let ended_resize = ps.resize.take();
                ps.panning = false;
                if let Some(resize) = ended_resize {
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
                    let final_bbox_w = ((resize.start_bbox_w as f32)
                        + ((final_w - resize.start_surface_w) as f32))
                        .max(1.0);
                    let final_bbox_h = ((resize.start_bbox_h as f32)
                        + ((final_h - resize.start_surface_h) as f32))
                        .max(1.0);
                    request_toplevel_resize_mode(st, resize.node_id, final_w, final_h, true);
                    request_toplevel_resize_mode(st, resize.node_id, final_w, final_h, false);
                    if let Some(n) = st.field.node_mut(resize.node_id) {
                        n.intrinsic_size.x = final_bbox_w;
                        n.intrinsic_size.y = final_bbox_h;
                    }
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
            };
            if left {
                if let Some(d) = ps.drag {
                    let now = Instant::now();
                    if d.started_active {
                        st.finalize_mouse_drag_state(d.node_id, world_now, now);
                    } else {
                        st.update_carry_state_preview_at(d.node_id, world_now, now);
                    }
                    let _ = st.field.finalize_dock_on_drag_release(d.node_id);
                    st.end_carry_state_tracking(d.node_id);
                    ps.preview_block_until = Some(now + Duration::from_millis(360));
                }
                ps.drag = None;
                st.field.clear_dock_preview();
                ps.panning = false;
                if ps.resize.is_some() {
                    finalize_resize(st, &mut ps, backend);
                }
            }
            if right {
                finalize_resize(st, &mut ps, backend);
            }
        }
    }
}
