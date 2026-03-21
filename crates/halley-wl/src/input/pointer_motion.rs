use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::interaction::types::{ModState, PointerState, ResizeHandle};
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::HalleyWlState;
use crate::surface::request_toplevel_resize_mode;
use halley_config::{KeyModifiers, PointerBindingAction};

use super::input_utils::modifier_active;
use super::pointer_focus::pointer_focus_for_screen;

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

#[inline]
fn clamp_screen_to_monitor(st: &HalleyWlState, name: &str, sx: f32, sy: f32) -> (f32, f32) {
    if let Some(monitor) = st.monitors.get(name) {
        let max_x = (monitor.offset_x + monitor.width - 1) as f32;
        let max_y = (monitor.offset_y + monitor.height - 1) as f32;
        (
            sx.clamp(monitor.offset_x as f32, max_x),
            sy.clamp(monitor.offset_y as f32, max_y),
        )
    } else {
        (sx, sy)
    }
}

#[inline]
fn pointer_outside_monitor(st: &HalleyWlState, name: &str, sx: f32, sy: f32) -> bool {
    st.monitors.get(name).is_some_and(|monitor| {
        sx < monitor.offset_x as f32
            || sx >= (monitor.offset_x + monitor.width) as f32
            || sy < monitor.offset_y as f32
            || sy >= (monitor.offset_y + monitor.height) as f32
    })
}

#[inline]
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

#[inline]
fn active_pointer_binding(
    st: &HalleyWlState,
    mods: &ModState,
    button_code: u32,
) -> Option<PointerBindingAction> {
    st.tuning
        .pointer_bindings
        .iter()
        .filter(|binding| binding.button == button_code && modifier_active(mods, binding.modifiers))
        .max_by_key(|binding| modifier_specificity(binding.modifiers))
        .map(|binding| binding.action)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_pointer_motion_absolute(
    st: &mut HalleyWlState,
    backend: &impl BackendView,
    mod_state: &Rc<RefCell<ModState>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    delta: (f64, f64),
    delta_unaccel: (f64, f64),
    time_usec: u64,
) {
    let allow_unbounded_screen = {
        let ps = pointer_state.borrow();
        ps.drag.is_some() || ps.resize.is_some() || ps.panning
    };

    let raw_sx = sx;
    let raw_sy = sy;
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, raw_sx, raw_sy);
    let now = Instant::now();
    let locked_surface = st.active_locked_pointer_surface();
    let mods = mod_state.borrow().clone();
    let drag_state = {
        let ps = pointer_state.borrow();
        ps.drag.map(|drag| {
            let owner = st
                .node_monitor
                .get(&drag.node_id)
                .cloned()
                .unwrap_or_else(|| st.current_monitor.clone());
            let allow_monitor_transfer =
                active_pointer_binding(st, &mods, 0x110) == Some(PointerBindingAction::FieldJump);
            (drag, owner, allow_monitor_transfer)
        })
    };
    let locked_drag_monitor = drag_state
        .as_ref()
        .and_then(|(_, owner, allow_monitor_transfer)| {
            (!*allow_monitor_transfer).then(|| owner.clone())
        });
    let locked_pan_monitor = {
        let ps = pointer_state.borrow();
        ps.panning.then(|| ps.pan_monitor.clone()).flatten()
    };
    let (effective_sx, effective_sy) = if let Some(owner) = locked_drag_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_pan_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if allow_unbounded_screen {
        (raw_sx, raw_sy)
    } else {
        (sx, sy)
    };

    let target_monitor = {
        if let Some((_, owner, allow_monitor_transfer)) = drag_state.as_ref() {
            if *allow_monitor_transfer {
                st.monitor_for_screen(effective_sx, effective_sy)
                    .unwrap_or_else(|| owner.clone())
            } else {
                owner.clone()
            }
        } else if let Some(owner) = locked_pan_monitor {
            owner
        } else {
            st.monitor_for_screen(effective_sx, effective_sy)
                .unwrap_or_else(|| st.current_monitor.clone())
        }
    };
    let _ = st.activate_monitor(target_monitor.as_str());
    let (local_w, local_h, local_sx, local_sy) =
        st.local_screen_in_monitor(target_monitor.as_str(), effective_sx, effective_sy);

    if let Some(pointer) = st.seat.get_pointer() {
        let resize_preview = pointer_state.borrow().resize;
        let focus = if let Some(surface) = locked_surface.clone() {
            Some((surface, pointer.current_location()))
        } else {
            pointer_focus_for_screen(st, local_w, local_h, local_sx, local_sy, now, resize_preview)
        };

        if delta.0.abs() > f64::EPSILON || delta.1.abs() > f64::EPSILON {
            pointer.relative_motion(
                st,
                focus.clone(),
                &RelativeMotionEvent {
                    delta: delta.into(),
                    delta_unaccel: delta_unaccel.into(),
                    utime: time_usec,
                },
            );
        }

        if locked_surface.is_none() {
            let location = if focus
                .as_ref()
                .is_some_and(|(surface, _)| st.is_layer_surface(surface))
            {
                (local_sx as f64, local_sy as f64).into()
            } else {
                let cam_scale = st.camera_render_scale() as f64;
                (local_sx as f64 / cam_scale, local_sy as f64 / cam_scale).into()
            };
            pointer.motion(
                st,
                focus.clone(),
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: now_millis_u32(),
                },
            );
            if let Some((surface, _)) = focus.as_ref() {
                st.activate_pointer_constraint_for_surface(surface);
            }
        }
        pointer.frame(st);
    }
    let p = screen_to_world(st, local_w, local_h, local_sx, local_sy);
    let drag_mod_ok = modifier_active(&mods, st.tuning.keybinds.modifier);

    let mut ps = pointer_state.borrow_mut();
    let mut pointer_world = p;
    ps.world = pointer_world;
    ps.screen = (effective_sx, effective_sy);
    ps.workspace_size = (local_w, local_h);

    if st.has_active_cluster_workspace() {
        ps.hover_node = None;
        st.set_drag_authority_node(None);
        ps.drag = None;
        ps.resize = None;
        ps.panning = false;
        return;
    }

    if let Some(drag) = ps.drag {
        if ps.resize.is_some() || !drag_mod_ok {
            st.set_drag_authority_node(None);
            st.end_carry_state_tracking(drag.node_id);
            ps.drag = None;
        } else {
            let mut next_drag = drag;
            let drag_allow_monitor_transfer = active_pointer_binding(st, &mods, 0x110)
                == Some(PointerBindingAction::FieldJump);
            next_drag.allow_monitor_transfer = drag_allow_monitor_transfer;
            let edge_pan_active = !drag_allow_monitor_transfer
                && pointer_outside_monitor(st, st.current_monitor.as_str(), raw_sx, raw_sy);
            if edge_pan_active {
                let dx_px = delta.0 as f32;
                let dy_px = delta.1 as f32;
                let camera = st.camera_view_size();
                let dx_world = dx_px * camera.x.max(1.0) / (local_w as f32).max(1.0);
                let dy_world = -dy_px * camera.y.max(1.0) / (local_h as f32).max(1.0);
                let pan_delta = halley_core::field::Vec2 {
                    x: dx_world,
                    y: dy_world,
                };
                st.note_pan_activity(now);
                st.pan_camera_target(pan_delta);
                st.note_pan_viewport_change(now);
                pointer_world = screen_to_world(st, local_w, local_h, local_sx, local_sy);
                ps.world = pointer_world;
            }
            let dt = now
                .saturating_duration_since(next_drag.last_update_at)
                .as_secs_f32()
                .max(1.0 / 240.0);
            let raw_velocity = halley_core::field::Vec2 {
                x: (pointer_world.x - next_drag.last_pointer_world.x) / dt,
                y: (pointer_world.y - next_drag.last_pointer_world.y) / dt,
            };
            let max_drag_speed = 800.0f32;
            let clamp_axis = |v: f32| v.clamp(-max_drag_speed, max_drag_speed);
            next_drag.release_velocity = halley_core::field::Vec2 {
                x: next_drag.release_velocity.x * 0.35 + clamp_axis(raw_velocity.x) * 0.65,
                y: next_drag.release_velocity.y * 0.35 + clamp_axis(raw_velocity.y) * 0.65,
            };
            next_drag.last_pointer_world = pointer_world;
            next_drag.last_update_at = now;
            let mut to = halley_core::field::Vec2 {
                x: pointer_world.x - next_drag.current_offset.x,
                y: pointer_world.y - next_drag.current_offset.y,
            };
            if !drag_allow_monitor_transfer
                && let Some(owner_monitor) = st
                    .node_monitor
                    .get(&drag.node_id)
                    .cloned()
                    .or_else(|| Some(target_monitor.clone()))
                && let Some(monitor) = st.monitors.get(owner_monitor.as_str())
            {
                // Clamp `to` (world space) to the zoomed camera frustum of the
                // owner monitor. We must use zoom_ref_size (the live camera view),
                // NOT viewport.size (the full unzoomed canvas). Using viewport.size
                // creates a dead zone when zoomed out (clamp allows past the
                // visible edge) and a jump when zoomed in (clamp cuts off before
                // the pointer reaches the screen edge).
                let half_w = monitor.zoom_ref_size.x * 0.5;
                let half_h = monitor.zoom_ref_size.y * 0.5;
                let min_x = monitor.camera_target_center.x - half_w;
                let max_x = monitor.camera_target_center.x + half_w;
                let min_y = monitor.camera_target_center.y - half_h;
                let max_y = monitor.camera_target_center.y + half_h;
                to.x = to.x.clamp(min_x, max_x);
                to.y = to.y.clamp(min_y, max_y);
            }
            if st.carry_surface_non_overlap(drag.node_id, to, edge_pan_active) {
                if drag_allow_monitor_transfer {
                    st.node_monitor
                        .insert(drag.node_id, st.current_monitor.clone());
                }
                st.physics_velocity
                    .insert(drag.node_id, next_drag.release_velocity);
                let should_center = st.tuning.center_window_to_mouse
                    && (!next_drag.center_latched
                        || next_drag.current_offset.x.abs() > f32::EPSILON
                        || next_drag.current_offset.y.abs() > f32::EPSILON);
                if should_center {
                    next_drag.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                    next_drag.center_latched = true;
                    let centered = halley_core::field::Vec2 {
                        x: pointer_world.x,
                        y: pointer_world.y,
                    };
                    let _ = st.carry_surface_non_overlap(drag.node_id, centered, false);
                }
                ps.drag = Some(next_drag);
                backend.request_redraw();
            }
        }
    }

    if let Some(resize) = ps.resize {
        let mut next = resize;
        const RESIZE_DRAG_START_PX: f32 = 3.0;

        let drag_dx = (local_sx - resize.press_sx).abs();
        let drag_dy = (local_sy - resize.press_sy).abs();
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

        // Build preview strictly in visual/screen space.
        let mut left = resize.start_left_px;
        let mut right = resize.start_right_px;
        let mut top = resize.start_top_px;
        let mut bottom = resize.start_bottom_px;

        // Horizontal movement
        match resize.handle {
            ResizeHandle::Left | ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                let desired_left = local_sx - resize.press_off_left_px;
                let max_left = resize.start_right_px - min_w;
                left = desired_left.min(max_left);
            }
            ResizeHandle::Right | ResizeHandle::TopRight | ResizeHandle::BottomRight => {
                let desired_right = local_sx - resize.press_off_right_px;
                let min_right = resize.start_left_px + min_w;
                right = desired_right.max(min_right);
            }
            ResizeHandle::Top | ResizeHandle::Bottom => {}
        }

        // Vertical movement
        match resize.handle {
            ResizeHandle::Top | ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                let desired_top = local_sy - resize.press_off_top_px;
                let max_top = resize.start_bottom_px - min_h;
                top = desired_top.min(max_top);
            }
            ResizeHandle::Bottom | ResizeHandle::BottomLeft | ResizeHandle::BottomRight => {
                let desired_bottom = local_sy - resize.press_off_bottom_px;
                let min_bottom = resize.start_top_px + min_h;
                bottom = desired_bottom.max(min_bottom);
            }
            ResizeHandle::Left | ResizeHandle::Right => {}
        }

        let target_visual_w = (right - left).round().max(min_w) as i32;
        let target_visual_h = (bottom - top).round().max(min_h) as i32;

        // Renormalize so the anchored side stays exact.
        match resize.handle {
            ResizeHandle::Left | ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                left = resize.start_right_px - target_visual_w as f32;
                right = resize.start_right_px;
            }
            ResizeHandle::Right | ResizeHandle::TopRight | ResizeHandle::BottomRight => {
                left = resize.start_left_px;
                right = resize.start_left_px + target_visual_w as f32;
            }
            ResizeHandle::Top | ResizeHandle::Bottom => {
                left = resize.start_left_px;
                right = resize.start_right_px;
            }
        }

        match resize.handle {
            ResizeHandle::Top | ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                top = resize.start_bottom_px - target_visual_h as f32;
                bottom = resize.start_bottom_px;
            }
            ResizeHandle::Bottom | ResizeHandle::BottomLeft | ResizeHandle::BottomRight => {
                top = resize.start_top_px;
                bottom = resize.start_top_px + target_visual_h as f32;
            }
            ResizeHandle::Left | ResizeHandle::Right => {
                top = resize.start_top_px;
                bottom = resize.start_bottom_px;
            }
        }

        // Derive client/bbox sizes from visual delta, not the other way around.
        // Visual delta is in screen pixels; divide by cam_scale to get logical
        // pixels for the configure message. At cam_scale=1.0 this is a no-op.
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

        let center_sx = (left + right) * 0.5;
        let center_sy = (top + bottom) * 0.5;
        // Use the local monitor viewport for screen_to_world so the node's
        // world position is computed in the correct coordinate space when the
        // resize is happening on a secondary monitor.
        let center_world = screen_to_world(st, local_w, local_h, center_sx, center_sy);

        if let Some(n) = st.field.node_mut(resize.node_id) {
            n.pos = center_world;
        }
        // Footprint is in world/logical units, not screen pixels.
        let _ = st.field.set_resize_footprint(
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
            .field
            .set_decay_level(resize.node_id, halley_core::decay::DecayLevel::Hot);

        backend.request_redraw();
        return;
    }

    if ps.panning {
        let (lsx, lsy) = ps.pan_last_screen;
        let dx_px = effective_sx - lsx;
        let dy_px = effective_sy - lsy;
        let camera = st.camera_view_size();
        // Use the active (local) monitor dimensions so that pan speed is
        // correct regardless of which monitor the pointer is on.
        let dx_world = dx_px * camera.x.max(1.0) / (local_w as f32).max(1.0);
        let dy_world = -dy_px * camera.y.max(1.0) / (local_h as f32).max(1.0);
        let now = Instant::now();
        st.note_pan_activity(now);
        st.pan_camera_target(halley_core::field::Vec2 {
            x: -dx_world,
            y: -dy_world,
        });
        st.note_pan_viewport_change(now);
        ps.pan_last_screen = (effective_sx, effective_sy);
        backend.request_redraw();
    }

    let next_hover = if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
        pick_hit_node_at(st, local_w, local_h, local_sx, local_sy, Instant::now(), ps.resize).and_then(|hit| {
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
}
