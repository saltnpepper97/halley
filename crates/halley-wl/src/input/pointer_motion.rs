use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::interaction::types::{ModState, PointerState};
use crate::spatial::{pick_hit_node_at, screen_to_world};
use crate::state::Halley;
use halley_config::PointerBindingAction;

use super::pointer_focus::pointer_focus_for_screen;
use super::pointer_frame::{
    active_pointer_binding, clamp_screen_to_monitor, clamp_screen_to_workspace, now_millis_u32,
};
use super::pointer_motion_drag::{handle_drag_motion, maybe_begin_core_drag_from_pending_press};
use super::pointer_motion_resize::handle_resize_motion;
use super::utils::modifier_active;

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
                .model
                .monitor_state
                .node_monitor
                .get(&drag.node_id)
                .cloned()
                .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
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
            st.model
                .monitor_state
                .node_monitor
                .get(&resize.node_id)
                .cloned()
                .or_else(|| Some(st.model.monitor_state.current_monitor.clone()))
        })
    };
    let locked_pan_monitor = {
        let ps = pointer_state.borrow();
        ps.panning.then(|| ps.pan_monitor.clone()).flatten()
    };
    let locked_bloom_monitor = {
        let ps = pointer_state.borrow();
        ps.bloom_drag.as_ref().map(|drag| drag.monitor.clone())
    };
    let locked_overflow_monitor = {
        let ps = pointer_state.borrow();
        ps.overflow_drag.as_ref().map(|drag| drag.monitor.clone())
    };
    let (effective_sx, effective_sy) = if let Some(owner) = locked_resize_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_bloom_monitor.as_deref() {
        clamp_screen_to_monitor(st, owner, raw_sx, raw_sy)
    } else if let Some(owner) = locked_overflow_monitor.as_deref() {
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
        let node_id = st.model.surface_to_node.get(&surface.id()).copied()?;
        Some(
            st.model
                .monitor_state
                .node_monitor
                .get(&node_id)
                .cloned()
                .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone()),
        )
    });
    let target_monitor = {
        if st.has_active_cluster_workspace() {
            st.model.monitor_state.current_monitor.clone()
        } else if let Some(owner) = locked_overflow_monitor {
            owner
        } else if let Some(owner) = locked_bloom_monitor {
            owner
        } else if let Some(owner) = locked_surface_monitor {
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

    if let Some(pointer) = st.platform.seat.get_pointer() {
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
    let drag_mod_ok = modifier_active(&mods, st.runtime.tuning.keybinds.modifier)
        || matches!(
            active_pointer_binding(st, &mods, 0x110),
            Some(PointerBindingAction::MoveWindow | PointerBindingAction::FieldJump)
        );

    let mut ps = pointer_state.borrow_mut();
    let pointer_world = p;
    ps.world = pointer_world;
    ps.screen = (effective_sx, effective_sy);
    ps.workspace_size = (local_w, local_h);

    maybe_begin_core_drag_from_pending_press(
        st,
        &mut ps,
        backend,
        local_w,
        local_h,
        effective_sx,
        effective_sy,
        local_sx,
        local_sy,
        pointer_world,
    );

    if let Some(bloom_drag) = ps.bloom_drag.clone() {
        let dx = local_sx - bloom_drag.core_screen.0;
        let dy = local_sy - bloom_drag.core_screen.1;
        const BLOOM_DETACH_THRESHOLD_PX: f32 = 96.0;
        let pull_dist = dx.hypot(dy);
        st.input.interaction_state.bloom_pull_preview = Some(crate::state::BloomPullPreview {
            cluster_id: bloom_drag.cluster_id,
            member_id: bloom_drag.member_id,
            mix: (pull_dist / BLOOM_DETACH_THRESHOLD_PX).clamp(0.0, 1.0),
        });
        if pull_dist >= BLOOM_DETACH_THRESHOLD_PX {
            ps.bloom_drag = None;
            st.input.interaction_state.bloom_pull_preview = None;
            let detached = st.detach_member_from_cluster(
                bloom_drag.cluster_id,
                bloom_drag.member_id,
                pointer_world,
                now,
            );
            if detached {
                st.assign_node_to_current_monitor(bloom_drag.member_id);
                st.set_interaction_focus(Some(bloom_drag.member_id), 30_000, now);
            }
            backend.request_redraw();
        }
        return;
    }

    if st.has_active_cluster_workspace() {
        let monitor = st.model.monitor_state.current_monitor.clone();
        let now_ms = st.now_ms(now);
        if let Some(overflow_drag) = ps.overflow_drag.clone() {
            st.input.interaction_state.cluster_overflow_drag_preview =
                Some(crate::state::ClusterOverflowDragPreview {
                    member_id: overflow_drag.member_id,
                    monitor: monitor.clone(),
                    screen_local: (local_sx, local_sy),
                });
            st.set_cursor_override_icon(Some(smithay::input::pointer::CursorIcon::Grabbing));
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
            st.input.interaction_state.overlay_hover_target = None;
            ps.hover_started_at = None;
            ps.hover_node = None;
            st.set_drag_authority_node(None);
            ps.drag = None;
            ps.resize = None;
            backend.request_redraw();
            return;
        }
        if ps.drag.is_none() && ps.resize.is_none() {
            let queue_hover = crate::overlay::cluster_overflow_icon_hit_test(
                &crate::overlay::OverlayView::from_halley(st),
                monitor.as_str(),
                local_sx,
                local_sy,
                now_ms,
            );
            if local_sx >= local_w as f32 - Halley::CLUSTER_OVERFLOW_REVEAL_EDGE_PX {
                st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
            } else if let Some(rect) = st.cluster_overflow_rect_for_monitor(monitor.as_str()) {
                let inside = local_sx >= rect.x
                    && local_sx <= rect.x + rect.w
                    && local_sy >= rect.y
                    && local_sy <= rect.y + rect.h;
                if inside {
                    st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
                }
            }
            st.set_cursor_override_icon(
                queue_hover
                    .map(|_| smithay::input::pointer::CursorIcon::Pointer)
                    .or(None),
            );
            let next_hover = queue_hover.map(|hit| hit.member_id);
            if next_hover != ps.hover_node {
                ps.hover_started_at = next_hover.map(|_| now);
            } else if next_hover.is_none() {
                ps.hover_started_at = None;
            }
            ps.hover_node = next_hover;
            st.input.interaction_state.overlay_hover_target =
                next_hover.map(|node_id| crate::state::OverlayHoverTarget {
                    node_id,
                    monitor: monitor.clone(),
                    screen_anchor: (local_sx.round() as i32, local_sy.round() as i32),
                    prefer_left: true,
                });
            st.set_drag_authority_node(None);
            ps.drag = None;
            ps.resize = None;
            if queue_hover.is_some() {
                backend.request_redraw();
            }
            return;
        }
    }
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    st.input.interaction_state.overlay_hover_target = None;
    st.set_cursor_override_icon(None);

    let _ = handle_drag_motion(
        st,
        backend,
        &mods,
        &mut ps,
        drag_mod_ok,
        target_monitor.as_str(),
        local_w,
        local_h,
        local_sx,
        local_sy,
        pointer_world,
        now,
    );

    if handle_resize_motion(st, &mut ps, local_w, local_h, local_sx, local_sy, backend) {
        return;
    }

    if ps.panning {
        let (lsx, lsy) = ps.pan_last_screen;
        let dx_px = effective_sx - lsx;
        let dy_px = effective_sy - lsy;
        let camera = st.camera_view_size();
        let dx_world = dx_px * camera.x.max(1.0) / (local_w as f32).max(1.0);
        let dy_world = dy_px * camera.y.max(1.0) / (local_h as f32).max(1.0);
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

    let bloom_hover = if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
        crate::overlay::bloom_token_hit_test(
            st,
            local_w,
            local_h,
            target_monitor.as_str(),
            local_sx,
            local_sy,
        )
        .map(|layout| {
            (
                layout.member_id,
                crate::state::OverlayHoverTarget {
                    node_id: layout.member_id,
                    monitor: target_monitor.clone(),
                    screen_anchor: (layout.center_sx, layout.center_sy),
                    prefer_left: false,
                },
            )
        })
    } else {
        None
    };

    let next_hover = if let Some((node_id, target)) = bloom_hover {
        st.input.interaction_state.overlay_hover_target = Some(target);
        Some(node_id)
    } else if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
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
            st.model.field.node(hit.node_id).and_then(|n| {
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
