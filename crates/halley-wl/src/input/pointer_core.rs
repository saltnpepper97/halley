use std::time::Instant;

use crate::backend::interface::BackendView;
use crate::interaction::types::{HitNode, NODE_DOUBLE_CLICK_MS, PointerState, TitleClickCtx};
use crate::state::{Halley, PendingCorePress};

use super::pointer_frame::ButtonFrame;

pub(super) fn title_click_is_double(
    ps: &PointerState,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    ps.last_title_click.is_some_and(|last| {
        last.node_id == node_id
            && now.duration_since(last.at).as_millis() as u64 <= NODE_DOUBLE_CLICK_MS
    })
}

pub(super) fn set_title_click(
    ps: &mut PointerState,
    node_id: halley_core::field::NodeId,
    now: Instant,
) {
    ps.last_title_click = Some(TitleClickCtx { node_id, at: now });
}

pub(super) fn clear_pointer_activity(st: &mut Halley, ps: &mut PointerState) {
    if let Some(drag) = ps.drag {
        st.set_drag_authority_node(None);
        st.end_carry_state_tracking(drag.node_id);
    }
    st.clear_grabbed_edge_pan_state();
    st.input.interaction_state.active_drag = None;
    st.input.interaction_state.pending_core_press = None;
    st.input.interaction_state.cluster_overflow_drag_preview = None;
    st.set_cursor_override_icon(None);
    ps.drag = None;
    ps.overflow_drag = None;
    ps.resize = None;
    ps.panning = false;
    ps.pan_monitor = None;
}

pub(super) fn collapse_bloom_for_core_if_open(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
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

pub(super) fn restore_fullscreen_click_focus(
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

pub(super) fn handle_core_left_press(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &dyn BackendView,
    hit: HitNode,
    frame: ButtonFrame,
) {
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
}

pub(super) fn handle_workspace_left_press(
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
