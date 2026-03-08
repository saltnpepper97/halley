use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use eventline::info;
use smithay::input::pointer::MotionEvent;
use smithay::utils::SERIAL_COUNTER;

use crate::backend_iface::BackendView;
use crate::interaction::types::{ModState, PointerState, ResizeHandle};
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::HalleyWlState;
use crate::surface::request_toplevel_resize_mode;

use super::input_utils::modifier_active;
use super::pointer_focus::pointer_focus_for_screen;
use super::pointer_map_debug_enabled;

#[inline]
fn screen_to_world_with_view(
    view_center: halley_core::field::Vec2,
    view_size: halley_core::field::Vec2,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
) -> halley_core::field::Vec2 {
    let vw = view_size.x.max(1.0);
    let vh = view_size.y.max(1.0);
    let nx = (sx / (ws_w as f32).max(1.0)) - 0.5;
    let ny = 0.5 - (sy / (ws_h as f32).max(1.0));
    halley_core::field::Vec2 {
        x: view_center.x + nx * vw,
        y: view_center.y + ny * vh,
    }
}

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

pub(crate) fn handle_pointer_motion_absolute(
    st: &mut HalleyWlState,
    backend: &impl BackendView,
    mod_state: &Rc<RefCell<ModState>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
) {
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, sx, sy);
    let now = Instant::now();
    if let Some(pointer) = st.seat.get_pointer() {
        let resize_preview = pointer_state.borrow().resize;
        let focus = pointer_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview);
        pointer.motion(
            st,
            focus,
            &MotionEvent {
                location: (sx as f64, sy as f64).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: now_millis_u32(),
            },
        );
        pointer.frame(st);
    }
    let p = screen_to_world(st, ws_w, ws_h, sx, sy);
    let mods = mod_state.borrow().clone();
    let drag_mod_ok = modifier_active(&mods, st.tuning.keybinds.modifier);
    let mut ps = pointer_state.borrow_mut();
    ps.world = p;
    ps.screen = (sx, sy);
    ps.workspace_size = (ws_w, ws_h);
    if st.has_active_cluster_workspace() {
        ps.hover_node = None;
        if ps.drag.is_some() {
            st.field.clear_dock_preview();
        }
        ps.drag = None;
        ps.resize = None;
        ps.panning = false;
        return;
    }
    if let Some(drag) = ps.drag {
        if ps.resize.is_some() || !drag_mod_ok {
            st.end_carry_state_tracking(drag.node_id);
            st.field.clear_dock_preview();
            ps.drag = None;
        } else {
            let mut next_drag = drag;
            let to = halley_core::field::Vec2 {
                x: p.x - next_drag.current_offset.x,
                y: p.y - next_drag.current_offset.y,
            };
            if st.carry_surface_non_overlap(drag.node_id, to) {
                let should_center = st.tuning.center_window_to_mouse
                    && (!next_drag.center_latched
                        || next_drag.current_offset.x.abs() > f32::EPSILON
                        || next_drag.current_offset.y.abs() > f32::EPSILON);
                if should_center {
                    next_drag.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                    next_drag.center_latched = true;
                    let centered = halley_core::field::Vec2 { x: p.x, y: p.y };
                    let _ = st.carry_surface_non_overlap(drag.node_id, centered);
                }
                ps.drag = Some(next_drag);
                let _ = st.field.update_dock_preview(
                    drag.node_id,
                    st.viewport.center,
                    st.viewport.size,
                );
                backend.request_redraw();
            }
        }
    }
    if let Some(resize) = ps.resize {
        let mut next = resize;
        const RESIZE_DRAG_START_PX: f32 = 3.0;
        let drag_dx = (sx - resize.press_sx).abs();
        let drag_dy = (sy - resize.press_sy).abs();
        if !next.drag_started && drag_dx.max(drag_dy) < RESIZE_DRAG_START_PX {
            ps.resize = Some(next);
            return;
        }
        if !next.drag_started {
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

        let delta_w = match resize.handle {
            ResizeHandle::Left | ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                let desired_left = sx - resize.press_off_left_px;
                resize.start_left_px - desired_left
            }
            ResizeHandle::Right | ResizeHandle::TopRight | ResizeHandle::BottomRight => {
                let desired_right = sx - resize.press_off_right_px;
                desired_right - resize.start_right_px
            }
            ResizeHandle::Top | ResizeHandle::Bottom => 0.0,
        };
        let delta_h = match resize.handle {
            ResizeHandle::Top | ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                let desired_top = sy - resize.press_off_top_px;
                resize.start_top_px - desired_top
            }
            ResizeHandle::Bottom | ResizeHandle::BottomLeft | ResizeHandle::BottomRight => {
                let desired_bottom = sy - resize.press_off_bottom_px;
                desired_bottom - resize.start_bottom_px
            }
            ResizeHandle::Left | ResizeHandle::Right => 0.0,
        };
        let target_w = ((resize.start_surface_w as f32) + delta_w)
            .max(min_w)
            .round() as i32;
        let target_h = ((resize.start_surface_h as f32) + delta_h)
            .max(min_h)
            .round() as i32;

        let (left, right) = match resize.handle {
            ResizeHandle::Left | ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                let r = resize.start_right_px;
                (r - target_w as f32, r)
            }
            ResizeHandle::Right | ResizeHandle::TopRight | ResizeHandle::BottomRight => {
                let l = resize.start_left_px;
                (l, l + target_w as f32)
            }
            ResizeHandle::Top | ResizeHandle::Bottom => {
                (resize.start_left_px, resize.start_right_px)
            }
        };
        let (top, bottom) = match resize.handle {
            ResizeHandle::Top | ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                let b = resize.start_bottom_px;
                (b - target_h as f32, b)
            }
            ResizeHandle::Bottom | ResizeHandle::BottomLeft | ResizeHandle::BottomRight => {
                let t = resize.start_top_px;
                (t, t + target_h as f32)
            }
            ResizeHandle::Left | ResizeHandle::Right => {
                (resize.start_top_px, resize.start_bottom_px)
            }
        };
        let now = Instant::now();
        let size_changed = target_w != resize.last_sent_w || target_h != resize.last_sent_h;
        if size_changed {
            request_toplevel_resize_mode(st, resize.node_id, target_w, target_h, true);
            next.last_sent_w = target_w;
            next.last_sent_h = target_h;
            next.last_configure_at = now;
        }
        let center_sx = (left + right) * 0.5;
        let center_sy = (top + bottom) * 0.5;
        let center_world = screen_to_world_with_view(
            resize.press_view_center,
            resize.press_view_size,
            resize.press_ws_w,
            resize.press_ws_h,
            center_sx,
            center_sy,
        );
        if let Some(n) = st.field.node_mut(resize.node_id) {
            n.intrinsic_size.x = target_w as f32;
            n.intrinsic_size.y = target_h as f32;
            n.pos = center_world;
        }
        st.set_last_active_size_now(
            resize.node_id,
            halley_core::field::Vec2 {
                x: target_w as f32,
                y: target_h as f32,
            },
        );
        next.preview_left_px = left;
        next.preview_right_px = right;
        next.preview_top_px = top;
        next.preview_bottom_px = bottom;
        ps.resize = Some(next);
        let _ = st
            .field
            .set_decay_level(resize.node_id, halley_core::decay::DecayLevel::Hot);
        backend.request_redraw();
        return;
    }
    if ps.panning {
        let (lsx, lsy) = ps.pan_last_screen;
        let dx_px = sx - lsx;
        let dy_px = sy - lsy;
        let dx_world = dx_px * st.viewport.size.x.max(1.0) / (ws_w as f32).max(1.0);
        let dy_world = -dy_px * st.viewport.size.y.max(1.0) / (ws_h as f32).max(1.0);
        st.note_pan_activity(Instant::now());
        st.viewport.pan(halley_core::field::Vec2 {
            x: -dx_world,
            y: -dy_world,
        });
        st.tuning.viewport_center = st.viewport.center;
        st.tuning.viewport_size = st.viewport.size;
        ps.pan_last_screen = (sx, sy);
        backend.request_redraw();
    }
    let next_hover = if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
        pick_hit_node_at(st, ws_w, ws_h, sx, sy, Instant::now()).and_then(|hit| {
            st.field.node(hit.node_id).and_then(|n| {
                matches!(
                    n.state,
                    halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
                )
                .then_some(hit.node_id)
            })
        })
    } else {
        None
    };
    if next_hover != ps.hover_node {
        ps.hover_started_at = next_hover.map(|_| now);
    } else if next_hover.is_none() {
        ps.hover_started_at = None;
    }
    ps.hover_node = next_hover;
    if pointer_map_debug_enabled() {
        info!(
            "ptr-map motion ws={}x{} screen=({:.2},{:.2}) world=({:.2},{:.2}) hover={:?} drag={} resize={} panning={}",
            ws_w,
            ws_h,
            sx,
            sy,
            p.x,
            p.y,
            ps.hover_node.map(|id| id.as_u64()),
            ps.drag.is_some(),
            ps.resize.is_some(),
            ps.panning
        );
    }
}
