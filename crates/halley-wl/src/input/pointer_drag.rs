use std::time::{Duration, Instant};

use crate::backend::interface::BackendView;
use crate::interaction::types::{DragAxisMode, DragCtx, HitNode, PointerState};
use crate::state::{ActiveDragState, Halley};

use super::pointer_frame::ButtonFrame;

pub(super) fn node_is_pointer_draggable(st: &Halley, node_id: halley_core::field::NodeId) -> bool {
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

pub(super) fn finish_pointer_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    node_id: halley_core::field::NodeId,
    started_active: bool,
    world_now: halley_core::field::Vec2,
    now: Instant,
) {
    let now_ms = st.now_ms(now);
    let drag_monitor = st
        .input
        .interaction_state
        .active_drag
        .as_ref()
        .map(|drag| drag.pointer_monitor.clone())
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    st.clear_grabbed_edge_pan_state();
    st.input.interaction_state.active_drag = None;
    let joined = st.commit_ready_cluster_join_for_node(node_id, now);
    if !joined {
        let moved_in_cluster = if started_active {
            st.move_active_cluster_member_to_drop_tile(
                drag_monitor.as_str(),
                node_id,
                world_now,
                now_ms,
            )
        } else {
            false
        };
        if started_active {
            st.finalize_mouse_drag_state(node_id, halley_core::field::Vec2 { x: 0.0, y: 0.0 }, now);
        } else if !moved_in_cluster {
            st.update_carry_state_preview(node_id, now);
        }
    } else {
        st.input.interaction_state.cluster_join_candidate = None;
    }
    st.set_drag_authority_node(None);
    st.end_carry_state_tracking(node_id);
    ps.preview_block_until = Some(now + Duration::from_millis(360));
    ps.drag = None;
}
