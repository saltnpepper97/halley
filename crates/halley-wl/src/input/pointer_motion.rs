use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::interaction::types::{DragAxisMode, ModState, PointerState};
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::{ActiveDragState, Halley};
use crate::surface_ops::request_toplevel_resize_mode;
use halley_config::{KeyModifiers, PointerBindingAction};

use super::utils::modifier_active;
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
fn clamp_screen_to_monitor(st: &Halley, name: &str, sx: f32, sy: f32) -> (f32, f32) {
    if let Some(monitor) = st.monitor_state.monitors.get(name) {
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
    st: &Halley,
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
    st: &mut Halley,
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
                .monitor_state
                .node_monitor
                .get(&drag.node_id)
                .cloned()
                .unwrap_or_else(|| st.monitor_state.current_monitor.clone());
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
    let locked_resize_monitor = {
        let ps = pointer_state.borrow();
        ps.resize.and_then(|resize| {
            st.monitor_state
                .node_monitor
                .get(&resize.node_id)
                .cloned()
                .or_else(|| Some(st.monitor_state.current_monitor.clone()))
        })
    };
    let locked_pan_monitor = {
        let ps = pointer_state.borrow();
        ps.panning.then(|| ps.pan_monitor.clone()).flatten()
    };
    let (effective_sx, effective_sy) = if let Some(owner) = locked_resize_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_drag_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_pan_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if allow_unbounded_screen {
        (raw_sx, raw_sy)
    } else {
        (sx, sy)
    };

    let locked_surface_monitor = locked_surface.as_ref().and_then(|surface| {
        let node_id = st.surface_to_node.get(&surface.id()).copied()?;
        Some(
            st.monitor_state
                .node_monitor
                .get(&node_id)
                .cloned()
                .unwrap_or_else(|| st.monitor_state.current_monitor.clone()),
        )
    });
    let target_monitor = {
        if let Some(owner) = locked_surface_monitor {
            owner
        } else if let Some(owner) = locked_resize_monitor {
            owner
        } else if let Some((_, owner, allow_monitor_transfer)) = drag_state.as_ref() {
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
                .unwrap_or_else(|| st.interaction_monitor().to_string())
        }
    };
    st.set_interaction_monitor(target_monitor.as_str());
    let _ = st.activate_monitor(target_monitor.as_str());
    let (local_w, local_h, local_sx, local_sy) =
        st.local_screen_in_monitor(target_monitor.as_str(), effective_sx, effective_sy);

    if let Some(pointer) = st.seat.get_pointer() {
        let resize_preview = pointer_state.borrow().resize;
        let focus = if let Some(surface) = locked_surface.clone() {
            Some((surface, pointer.current_location()))
        } else {
            pointer_focus_for_screen(
                st,
                local_w,
                local_h,
                local_sx,
                local_sy,
                now,
                resize_preview,
            )
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
    let pointer_world = p;
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
            let drag_allow_monitor_transfer =
                active_pointer_binding(st, &mods, 0x110) == Some(PointerBindingAction::FieldJump);
            next_drag.allow_monitor_transfer = drag_allow_monitor_transfer;
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
            let desired_to = halley_core::field::Vec2 {
                x: pointer_world.x - next_drag.current_offset.x,
                y: pointer_world.y - next_drag.current_offset.y,
            };
            if !drag_allow_monitor_transfer
                && next_drag.edge_pan_eligible
                && let Some(owner_monitor) = st
                    .monitor_state
                    .node_monitor
                    .get(&drag.node_id)
                    .cloned()
                    .or_else(|| Some(target_monitor.clone()))
            {
                if let Some((clamped_center, edge_contact)) = st.dragged_node_edge_pan_clamp(
                    owner_monitor.as_str(),
                    drag.node_id,
                    desired_to,
                    halley_core::field::Vec2 {
                        x: next_drag.edge_pan_x.sign(),
                        y: next_drag.edge_pan_y.sign(),
                    },
                )
                {
                    const EDGE_PAN_PRESSURE_THRESHOLD: f32 = 28.0;
                    const EDGE_PAN_PRESSURE_DECAY_PER_SEC: f32 = 120.0;
                    const EDGE_PAN_RELEASE_DISTANCE: f32 = 42.0;

                    next_drag.edge_pan_pressure.x = (next_drag.edge_pan_pressure.x
                        - EDGE_PAN_PRESSURE_DECAY_PER_SEC * dt)
                        .max(0.0);
                    next_drag.edge_pan_pressure.y = (next_drag.edge_pan_pressure.y
                        - EDGE_PAN_PRESSURE_DECAY_PER_SEC * dt)
                        .max(0.0);

                    if edge_contact.x < 0.0 {
                        next_drag.edge_pan_pressure.x += (clamped_center.x - desired_to.x).max(0.0);
                    } else if edge_contact.x > 0.0 {
                        next_drag.edge_pan_pressure.x += (desired_to.x - clamped_center.x).max(0.0);
                    } else {
                        next_drag.edge_pan_pressure.x = 0.0;
                    }

                    if edge_contact.y < 0.0 {
                        next_drag.edge_pan_pressure.y += (clamped_center.y - desired_to.y).max(0.0);
                    } else if edge_contact.y > 0.0 {
                        next_drag.edge_pan_pressure.y += (desired_to.y - clamped_center.y).max(0.0);
                    } else {
                        next_drag.edge_pan_pressure.y = 0.0;
                    }

                    next_drag.edge_pan_x = match next_drag.edge_pan_x {
                        DragAxisMode::Free => {
                            if edge_contact.x < 0.0
                                && next_drag.edge_pan_pressure.x >= EDGE_PAN_PRESSURE_THRESHOLD
                            {
                                DragAxisMode::EdgePanNeg
                            } else if edge_contact.x > 0.0
                                && next_drag.edge_pan_pressure.x >= EDGE_PAN_PRESSURE_THRESHOLD
                            {
                                DragAxisMode::EdgePanPos
                            } else {
                                DragAxisMode::Free
                            }
                        }
                        DragAxisMode::EdgePanNeg => {
                            if desired_to.x > clamped_center.x + EDGE_PAN_RELEASE_DISTANCE {
                                next_drag.edge_pan_pressure.x = 0.0;
                                DragAxisMode::Free
                            } else {
                                DragAxisMode::EdgePanNeg
                            }
                        }
                        DragAxisMode::EdgePanPos => {
                            if desired_to.x < clamped_center.x - EDGE_PAN_RELEASE_DISTANCE {
                                next_drag.edge_pan_pressure.x = 0.0;
                                DragAxisMode::Free
                            } else {
                                DragAxisMode::EdgePanPos
                            }
                        }
                    };
                    next_drag.edge_pan_y = match next_drag.edge_pan_y {
                        DragAxisMode::Free => {
                            if edge_contact.y < 0.0
                                && next_drag.edge_pan_pressure.y >= EDGE_PAN_PRESSURE_THRESHOLD
                            {
                                DragAxisMode::EdgePanNeg
                            } else if edge_contact.y > 0.0
                                && next_drag.edge_pan_pressure.y >= EDGE_PAN_PRESSURE_THRESHOLD
                            {
                                DragAxisMode::EdgePanPos
                            } else {
                                DragAxisMode::Free
                            }
                        }
                        DragAxisMode::EdgePanNeg => {
                            if desired_to.y > clamped_center.y + EDGE_PAN_RELEASE_DISTANCE {
                                next_drag.edge_pan_pressure.y = 0.0;
                                DragAxisMode::Free
                            } else {
                                DragAxisMode::EdgePanNeg
                            }
                        }
                        DragAxisMode::EdgePanPos => {
                            if desired_to.y < clamped_center.y - EDGE_PAN_RELEASE_DISTANCE {
                                next_drag.edge_pan_pressure.y = 0.0;
                                DragAxisMode::Free
                            } else {
                                DragAxisMode::EdgePanPos
                            }
                        }
                    };

                    let engage_x = next_drag.edge_pan_x.sign();
                    let engage_y = next_drag.edge_pan_y.sign();
                    let edge_pan_direction = halley_core::field::Vec2 {
                        x: engage_x,
                        y: engage_y,
                    };
                    let edge_pan_active = edge_pan_direction.x != 0.0 || edge_pan_direction.y != 0.0;

                    st.interaction_state.grabbed_edge_pan_active = edge_pan_active;
                    st.interaction_state.grabbed_edge_pan_direction = edge_pan_direction;
                    st.interaction_state.grabbed_edge_pan_monitor =
                        edge_pan_active.then(|| owner_monitor.clone());
                } else {
                    st.interaction_state.grabbed_edge_pan_active = false;
                    st.interaction_state.grabbed_edge_pan_direction =
                        halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                    st.interaction_state.grabbed_edge_pan_monitor = None;
                    next_drag.edge_pan_x = DragAxisMode::Free;
                    next_drag.edge_pan_y = DragAxisMode::Free;
                    next_drag.edge_pan_pressure = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                }
            } else {
                st.interaction_state.grabbed_edge_pan_active = false;
                st.interaction_state.grabbed_edge_pan_direction =
                    halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                st.interaction_state.grabbed_edge_pan_monitor = None;
                next_drag.edge_pan_x = DragAxisMode::Free;
                next_drag.edge_pan_y = DragAxisMode::Free;
                next_drag.edge_pan_pressure = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
            }
            let should_center = st.tuning.center_window_to_mouse
                && (!next_drag.center_latched
                    || next_drag.current_offset.x.abs() > f32::EPSILON
                    || next_drag.current_offset.y.abs() > f32::EPSILON);
            if should_center {
                next_drag.current_offset = halley_core::field::Vec2 { x: 0.0, y: 0.0 };
                next_drag.center_latched = true;
            }
            st.interaction_state.drag_authority_velocity = next_drag.release_velocity;
            st.interaction_state.active_drag = Some(ActiveDragState {
                node_id: drag.node_id,
                allow_monitor_transfer: drag_allow_monitor_transfer,
                edge_pan_eligible: next_drag.edge_pan_eligible,
                current_offset: next_drag.current_offset,
                pointer_monitor: target_monitor.clone(),
                pointer_workspace_size: (local_w, local_h),
                pointer_screen_local: (local_sx, local_sy),
                edge_pan_x: next_drag.edge_pan_x,
                edge_pan_y: next_drag.edge_pan_y,
            });
            ps.drag = Some(next_drag);
            backend.request_redraw();
        }
    }

    if let Some(resize) = ps.resize {
        let mut next = resize;

        // Total pointer displacement from the original press position.
        // dx positive = rightward, dy positive = downward (screen space).
        let dx = local_sx - resize.press_sx;
        let dy = local_sy - resize.press_sy;

        const RESIZE_DRAG_START_PX: f32 = 3.0;

        // Handle is committed at press time from the press position.
        // Just wait for the dead zone then start moving.
        if !next.drag_started {
            if dx.abs().max(dy.abs()) < RESIZE_DRAG_START_PX {
                ps.resize = Some(next);
                return;
            }
            next.drag_started = true;
        }

        // ── Send initial resize-mode configure ───────────────────────────────
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

        // ── Phase 4: compute new preview rect ───────────────────────────────
        //
        // `weights_from_handle` uses +1.0 for the edge that follows the pointer
        // directly and 0.0 for the anchored opposite edge. For example, a left
        // grab has `(h_weight_left, h_weight_right) = (1.0, 0.0)`, so dragging
        // right increases `left` and shrinks the window while `right` stays put.
        let desired_left = resize.start_left_px + next.h_weight_left * dx;
        let desired_right = resize.start_right_px + next.h_weight_right * dx;
        let desired_top = resize.start_top_px + next.v_weight_top * dy;
        let desired_bottom = resize.start_bottom_px + next.v_weight_bottom * dy;

        // Preserve the anchored edge whenever the minimum size is reached.
        // This stops the preview from translating toward the cursor once the
        // dragged edge can no longer move any farther inward.
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

        // Derive logical (client) size from visual delta / cam_scale.
        // At cam_scale = 1.0 this is a no-op.
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

        // While resizing, keep normal motion physics inert for this node.
        st.interaction_state
            .physics_velocity
            .insert(resize.node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 });

        // Keep node world position at the visual center of the preview rect so
        // overlap resolution and footprint tracking stay accurate regardless of
        // which corner or edge is moving.
        let center_sx = (left + right) * 0.5;
        let center_sy = (top + bottom) * 0.5;
        let center_world = screen_to_world(st, local_w, local_h, center_sx, center_sy);
        if let Some(n) = st.field.node_mut(resize.node_id) {
            n.pos = center_world;
        }
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
        pick_hit_node_at(
            st,
            local_w,
            local_h,
            local_sx,
            local_sy,
            Instant::now(),
            ps.resize,
        )
        .and_then(|hit| {
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
