use std::time::Instant;

use crate::backend::interface::BackendView;
use crate::compositor::actions::window::activate_collapsed_node_from_click;
use crate::compositor::interaction::state::PendingMovePress;
use crate::compositor::interaction::{HitNode, PointerState};
use crate::compositor::root::Halley;
use crate::compositor::surface::node_allows_interactive_resize;

use super::frame::ButtonFrame;
use super::release::{
    clear_pointer_activity, collapse_bloom_for_core_if_open, restore_fullscreen_click_focus,
};
use crate::input::pointer::motion::{begin_drag, node_is_pointer_draggable};
use crate::input::pointer::resize::begin_resize;

fn begin_pan_if_allowed(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    monitor: String,
    global_sx: f32,
    global_sy: f32,
) {
    if crate::compositor::monitor::camera::camera_controller(&*st)
        .pan_blocked_on_monitor(monitor.as_str())
    {
        backend.request_redraw();
        return;
    }
    ps.panning = true;
    ps.pan_monitor = Some(monitor);
    ps.pan_last_screen = (global_sx, global_sy);
    crate::compositor::interaction::pointer::set_cursor_override_icon(
        st,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    backend.request_redraw();
}

pub(super) fn begin_bloom_pull_preview(
    st: &mut Halley,
    cluster_id: halley_core::cluster::ClusterId,
    member_id: halley_core::field::NodeId,
    core_sx: i32,
    core_sy: i32,
    slot_sx: i32,
    slot_sy: i32,
    monitor: &str,
) {
    st.input.interaction_state.bloom_pull_preview =
        Some(crate::compositor::interaction::state::BloomPullPreview {
            cluster_id,
            member_id,
            monitor: monitor.to_string(),
            core_screen: halley_core::field::Vec2 {
                x: core_sx as f32,
                y: core_sy as f32,
            },
            slot_screen: halley_core::field::Vec2 {
                x: slot_sx as f32,
                y: slot_sy as f32,
            },
            pointer_screen: halley_core::field::Vec2 {
                x: slot_sx as f32,
                y: slot_sy as f32,
            },
            display_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            hold_progress: 0.0,
            phase: crate::compositor::interaction::state::BloomPullPhase::Pressed,
        });
}

pub(super) fn handle_left_press(
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
        let monitor = st.monitor_for_screen_or_current(frame.global_sx, frame.global_sy);
        let _ = st.close_cluster_bloom_for_monitor(monitor.as_str());
        st.focus_monitor_view(monitor.as_str(), now);
        begin_pan_if_allowed(st, ps, backend, monitor, frame.global_sx, frame.global_sy);
        return;
    };
    if frame.workspace_active {
        let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
        if drag_binding_active && drag_target_ok && !hit.is_core {
            begin_drag(
                st,
                ps,
                backend,
                hit,
                frame,
                frame.world_now,
                allow_monitor_transfer,
                drag_binding_active,
            );
            return;
        }
        if hit.move_surface && drag_target_ok && !hit.is_core {
            let now = Instant::now();
            st.focus_pointer_target(hit.node_id, 700, now);
            st.input.interaction_state.pending_move_press = Some(PendingMovePress {
                node_id: hit.node_id,
                press_global_sx: frame.global_sx,
                press_global_sy: frame.global_sy,
                workspace_active: true,
            });
            backend.request_redraw();
            return;
        }
        handle_workspace_left_press(st, ps, backend, hit);
        return;
    }

    if !drag_binding_active && hit.is_core {
        handle_core_left_press(st, ps, backend, hit, frame);
        return;
    }

    if !drag_binding_active && restore_fullscreen_click_focus(st, hit.node_id, Instant::now()) {
        backend.request_redraw();
    }

    let drag_target_ok = node_is_pointer_draggable(st, hit.node_id);
    if hit.move_surface && drag_target_ok && !hit.is_core {
        let now = Instant::now();
        st.focus_pointer_target(hit.node_id, 700, now);
        st.input.interaction_state.pending_move_press = Some(PendingMovePress {
            node_id: hit.node_id,
            press_global_sx: frame.global_sx,
            press_global_sy: frame.global_sy,
            workspace_active: false,
        });
        backend.request_redraw();
        return;
    }

    if !drag_binding_active
        && !hit.move_surface
        && st.model.field.node(hit.node_id).is_some_and(|n| {
            n.kind == halley_core::field::NodeKind::Surface
                && st.model.field.is_visible(hit.node_id)
        })
    {
        st.focus_pointer_target(hit.node_id, 30_000, Instant::now());
    }

    let mut handled_node_click = false;
    if !drag_binding_active && !hit.move_surface && !hit.is_core {
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
            handled_node_click = true;
        }
    }

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
            drag_binding_active,
        );
        return;
    }

    if hit.is_core {
        let now = Instant::now();
        st.focus_pointer_target(hit.node_id, 700, now);
        backend.request_redraw();
    }
}

pub(super) fn handle_right_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    resize_binding_active: bool,
    hit: Option<HitNode>,
    frame: ButtonFrame,
) {
    let Some(hit) = hit else {
        if frame.workspace_active {
            clear_pointer_activity(st, ps);
            return;
        }
        let now = Instant::now();
        let monitor = st.monitor_for_screen_or_current(frame.global_sx, frame.global_sy);
        st.focus_monitor_view(monitor.as_str(), now);
        begin_pan_if_allowed(st, ps, backend, monitor, frame.global_sx, frame.global_sy);
        return;
    };
    let can_resize = node_allows_interactive_resize(st, hit.node_id);
    if resize_binding_active && can_resize {
        begin_resize(st, ps, backend, hit, frame);
    }
}

pub(super) fn handle_move_binding_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: Option<HitNode>,
    frame: ButtonFrame,
    allow_monitor_transfer: bool,
) {
    let Some(hit) = hit else {
        if frame.workspace_active {
            clear_pointer_activity(st, ps);
            return;
        }
        let now = Instant::now();
        let monitor = st.monitor_for_screen_or_current(frame.global_sx, frame.global_sy);
        st.focus_monitor_view(monitor.as_str(), now);
        begin_pan_if_allowed(st, ps, backend, monitor, frame.global_sx, frame.global_sy);
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
            true,
        );
    }
}

pub(super) fn handle_resize_binding_press(
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
        let monitor = st.monitor_for_screen_or_current(frame.global_sx, frame.global_sy);
        st.focus_monitor_view(monitor.as_str(), now);
        begin_pan_if_allowed(st, ps, backend, monitor, frame.global_sx, frame.global_sy);
        return;
    };
    let can_resize = node_allows_interactive_resize(st, hit.node_id);
    if can_resize {
        begin_resize(st, ps, backend, hit, frame);
    }
}

pub(crate) fn handle_core_left_press(
    st: &mut Halley,
    _ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
    let now = Instant::now();
    let now_ms = st.now_ms(now);
    st.input.interaction_state.pending_core_hover = None;
    st.set_interaction_focus(Some(hit.node_id), 700, now);
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
    } else {
        st.input.interaction_state.pending_core_press =
            Some(crate::compositor::interaction::state::PendingCorePress {
                node_id: hit.node_id,
                monitor: st.model.monitor_state.current_monitor.clone(),
                press_global_sx: frame.global_sx,
                press_global_sy: frame.global_sy,
            });
    }
    backend.request_redraw();
}

pub(crate) fn handle_workspace_left_press(
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
    if hit.move_surface && !hit.is_core {
        backend.request_redraw();
        return;
    }
    let focus_hold_ms = if hit.is_core { 700 } else { 30_000 };
    st.focus_pointer_target(hit.node_id, focus_hold_ms, now);
    backend.request_redraw();
}
