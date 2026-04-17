use std::time::Instant;

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::backend::interface::BackendView;
use crate::compositor::exit_confirm::exit_confirm_controller;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::input::ctx::InputCtx;
use crate::input::pointer::focus;
use crate::spatial::pick_hit_node_at;

pub(crate) mod bloom;
pub(crate) mod cluster;
pub(crate) mod drag;
pub(crate) mod pending;
pub(crate) mod routing;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) use drag::{begin_drag, finish_pointer_drag, node_is_pointer_draggable};

use super::button::now_millis_u32;
use super::context::{
    clamp_screen_to_workspace, pointer_screen_context_for_monitor,
};
use super::focus::pointer_focus_for_screen;
use super::resize::handle_resize_motion;
use super::screenshot::handle_screenshot_pointer_motion;
use crate::input::keyboard::modkeys::modifier_active;

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_pointer_motion_absolute<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    delta: (f64, f64),
    delta_unaccel: (f64, f64),
    time_usec: u64,
) {
    if exit_confirm_controller(&*st).active() {
        return;
    }
    if crate::compositor::interaction::state::note_cursor_activity(st, st.now_ms(Instant::now())) {
        ctx.backend.request_redraw();
    }

    let allow_unbounded_screen = {
        let ps = ctx.pointer_state.borrow();
        ps.drag.is_some() || ps.resize.is_some() || ps.panning
    };

    let raw_sx = sx;
    let raw_sy = sy;
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, raw_sx, raw_sy);
    let now = Instant::now();
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let target_monitor = st.monitor_for_screen_or_interaction(sx, sy);
        let context = pointer_screen_context_for_monitor(st, target_monitor, (sx, sy), true, true);
        {
            let mut ps = ctx.pointer_state.borrow_mut();
            ps.screen = (context.global_sx, context.global_sy);
            ps.workspace_size = (context.ws_w, context.ws_h);
            ps.world = context.world;
        }
        if let Some(pointer) = st.platform.seat.get_pointer() {
            let focus = pointer_focus_for_screen(
                st,
                context.ws_w,
                context.ws_h,
                context.local_sx,
                context.local_sy,
                now,
                None,
            );
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
            pointer.motion(
                st,
                focus,
                &MotionEvent {
                    location: (context.local_sx as f64, context.local_sy as f64).into(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: now_millis_u32(),
                },
            );
        }
        return;
    }

    let mods = ctx.mod_state.borrow().clone();
    let routing = {
        let ps = ctx.pointer_state.borrow();
        routing::compute_motion_routing(st, &ps, &mods, raw_sx, raw_sy, sx, sy, allow_unbounded_screen)
    };

    let (desktop_hover, hover_focus_blocked) = {
        let ps = ctx.pointer_state.borrow();
        routing::dispatch_pointer_motion(st, &ps, &routing, delta, delta_unaccel, time_usec, now)
    };

    let p = routing.world;
    let drag_mod_ok = modifier_active(&mods, st.runtime.tuning.keybinds.modifier)
        || matches!(
            super::button::active_pointer_binding(st, &mods, 0x110),
            Some(halley_config::PointerBindingAction::MoveWindow | halley_config::PointerBindingAction::FieldJump)
        );

    let mut ps = ctx.pointer_state.borrow_mut();
    ps.world = p;
    ps.screen = (routing.global_sx, routing.global_sy);
    ps.workspace_size = (routing.ws_w, routing.ws_h);
    st.input.interaction_state.last_pointer_screen_global = Some((routing.global_sx, routing.global_sy));

    if handle_screenshot_pointer_motion(
        st,
        ctx,
        routing.monitor.as_str(),
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
        routing.global_sx,
        routing.global_sy,
        now,
    ) {
        return;
    }

    let prompt_monitor = st.model.monitor_state.current_monitor.clone();
    if crate::compositor::clusters::system::cluster_system_controller(&*st)
        .cluster_name_prompt_active_for_monitor(prompt_monitor.as_str())
    {
        let prompt_hit = if routing.monitor == prompt_monitor {
            crate::overlay::cluster_naming_dialog_hit_test(st, routing.ws_w, routing.ws_h, routing.local_sx, routing.local_sy)
        } else {
            None
        };
        st.input.interaction_state.overlay_hover_target = None;
        st.input.interaction_state.pending_core_hover = None;
        ps.hover_node = None;
        ps.hover_started_at = None;
        if let Some(crate::overlay::ClusterNamingDialogHit::InputCaret(caret_char)) = prompt_hit {
            let _ = crate::compositor::clusters::system::cluster_system_controller(&mut *st)
                .drag_cluster_name_prompt_selection_for_monitor(
                    prompt_monitor.as_str(),
                    caret_char,
                );
        }
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            match prompt_hit {
                Some(crate::overlay::ClusterNamingDialogHit::ConfirmButton) => {
                    Some(smithay::input::pointer::CursorIcon::Pointer)
                }
                Some(crate::overlay::ClusterNamingDialogHit::InputCaret(_)) => {
                    Some(smithay::input::pointer::CursorIcon::Text)
                }
                None => None,
            },
        );
        ctx.backend.request_redraw();
        return;
    }

    pending::maybe_begin_core_drag_from_pending_press(
        st,
        &mut ps,
        ctx.backend,
        routing.ws_w,
        routing.ws_h,
        routing.global_sx,
        routing.global_sy,
        routing.local_sx,
        routing.local_sy,
        routing.world,
    );
    pending::maybe_begin_move_drag_from_pending_press(
        st,
        &mut ps,
        ctx.backend,
        routing.ws_w,
        routing.ws_h,
        routing.global_sx,
        routing.global_sy,
        routing.local_sx,
        routing.local_sy,
        routing.world,
    );

    if bloom::handle_bloom_pull_motion(
        st,
        &mut ps,
        ctx.backend,
        routing.global_sx,
        routing.global_sy,
        now,
    ) {
        return;
    }

    if st.has_active_cluster_workspace() {
        let monitor = st.model.monitor_state.current_monitor.clone();
        let now_ms = st.now_ms(now);
        if let Some(overflow_drag) = ps.overflow_drag.clone() {
            st.input.interaction_state.cluster_overflow_drag_preview = Some(
                crate::compositor::interaction::state::ClusterOverflowDragPreview {
                    member_id: overflow_drag.member_id,
                    monitor: monitor.clone(),
                    screen_local: (routing.local_sx, routing.local_sy),
                },
            );
            crate::compositor::interaction::pointer::set_cursor_override_icon(
                st,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
            st.input.interaction_state.overlay_hover_target = None;
            ps.hover_started_at = None;
            ps.hover_node = None;
            crate::compositor::carry::system::set_drag_authority_node(st, None);
            ps.drag = None;
            ps.resize = None;
            ctx.backend.request_redraw();
            return;
        }
        if ps.drag.is_none() && ps.resize.is_none() {
            let queue_hover = crate::overlay::cluster_overflow_icon_hit_test(
                &crate::overlay::OverlayView::from_halley(st),
                monitor.as_str(),
                routing.local_sx,
                routing.local_sy,
                now_ms,
            );
            if routing.local_sx >= routing.ws_w as f32 - Halley::CLUSTER_OVERFLOW_REVEAL_EDGE_PX {
                st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
            } else if let Some(rect) = st.cluster_overflow_rect_for_monitor(monitor.as_str()) {
                let inside = routing.local_sx >= rect.x
                    && routing.local_sx <= rect.x + rect.w
                    && routing.local_sy >= rect.y
                    && routing.local_sy <= rect.y + rect.h;
                if inside {
                    st.reveal_cluster_overflow_for_monitor(monitor.as_str(), now_ms);
                }
            }
            crate::compositor::interaction::pointer::set_cursor_override_icon(
                st,
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
            st.input.interaction_state.pending_core_hover = None;
            st.input.interaction_state.overlay_hover_target = next_hover.map(|node_id| {
                crate::compositor::interaction::state::OverlayHoverTarget {
                    node_id,
                    monitor: monitor.clone(),
                    screen_anchor: (routing.local_sx.round() as i32, routing.local_sy.round() as i32),
                    prefer_left: true,
                }
            });
            crate::compositor::carry::system::set_drag_authority_node(st, None);
            ps.drag = None;
            ps.resize = None;
            if queue_hover.is_some() {
                ctx.backend.request_redraw();
            }
            return;
        }
    }
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    st.input.interaction_state.overlay_hover_target = None;
    crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);

    let _ = drag::handle_drag_motion(
        st,
        ctx.backend,
        &mods,
        &mut ps,
        drag_mod_ok,
        routing.monitor.as_str(),
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
        routing.world,
        now,
    );

    if handle_resize_motion(
        st,
        &mut ps,
        routing.ws_w,
        routing.ws_h,
        routing.local_sx,
        routing.local_sy,
        ctx.backend,
    ) {
        return;
    }

    if ps.panning {
        crate::compositor::interaction::pointer::set_cursor_override_icon(
            st,
            Some(smithay::input::pointer::CursorIcon::Grabbing),
        );
        let (lsx, lsy) = ps.pan_last_screen;
        let dx_px = routing.global_sx - lsx;
        let dy_px = routing.global_sy - lsy;
        let camera = camera_controller(&*st).view_size();
        let dx_world = dx_px * camera.x.max(1.0) / (routing.ws_w as f32).max(1.0);
        let dy_world = dy_px * camera.y.max(1.0) / (routing.ws_h as f32).max(1.0);
        let now = Instant::now();
        st.note_pan_activity(now);
        camera_controller(&mut *st).pan_target(halley_core::field::Vec2 {
            x: -dx_world,
            y: -dy_world,
        });
        st.note_pan_viewport_change(now);
        ps.pan_last_screen = (routing.global_sx, routing.global_sy);
        ctx.backend.request_redraw();
    }

    let bloom_hover = if ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
        crate::overlay::bloom_token_hit_test(
            st,
            routing.ws_w,
            routing.ws_h,
            routing.monitor.as_str(),
            routing.local_sx,
            routing.local_sy,
        )
        .map(|layout| {
            (
                layout.member_id,
                crate::compositor::interaction::state::OverlayHoverTarget {
                    node_id: layout.member_id,
                    monitor: routing.monitor.clone(),
                    screen_anchor: (layout.center_sx + layout.token_radius + 4, layout.center_sy),
                    prefer_left: false,
                },
            )
        })
    } else {
        None
    };

    let hover_hit =
        if bloom_hover.is_none() && ps.drag.is_none() && ps.resize.is_none() && !ps.panning {
            pick_hit_node_at(st, routing.ws_w, routing.ws_h, routing.local_sx, routing.local_sy, now, ps.resize)
        } else {
            None
        };

    let next_hover = if let Some((node_id, target)) = bloom_hover {
        st.input.interaction_state.overlay_hover_target = Some(target);
        Some(node_id)
    } else {
        hover_hit.and_then(|hit| {
            st.model.field.node(hit.node_id).and_then(|n| {
                matches!(
                    n.state,
                    halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
                )
                .then_some(hit.node_id)
            })
        })
    };

    focus::apply_hover_focus_mode(st, hover_hit, hover_focus_blocked, now);

    if next_hover != ps.hover_node {
        ps.hover_started_at = next_hover.map(|_| now);
    } else if next_hover.is_none() {
        ps.hover_started_at = None;
    }
    ps.hover_node = next_hover;
    let previous_core_hover = st.input.interaction_state.pending_core_hover.clone();
    st.input.interaction_state.pending_core_hover = next_hover.and_then(|node_id| {
        st.model.field.node(node_id).and_then(|node| {
            (node.kind == halley_core::field::NodeKind::Core
                && node.state == halley_core::field::NodeState::Core)
                .then(|| {
                    let monitor = routing.monitor.clone();
                    let started_at_ms = previous_core_hover
                        .as_ref()
                        .filter(|pending| pending.node_id == node_id && pending.monitor == monitor)
                        .map(|pending| pending.started_at_ms)
                        .unwrap_or(st.now_ms(now));
                    crate::compositor::interaction::state::PendingCoreHover {
                        node_id,
                        monitor,
                        started_at_ms,
                    }
                })
        })
    });
    if st.input.interaction_state.pending_core_hover.is_some() {
        st.request_maintenance();
    }

    if ps.drag.is_none()
        && ps.resize.is_none()
        && !ps.panning
        && desktop_hover
        && st.input.interaction_state.overlay_hover_target.is_none()
    {
        crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    }

    ctx.backend.request_redraw();
}
