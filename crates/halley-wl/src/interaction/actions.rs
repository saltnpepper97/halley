use crate::state::HalleyWlState;
use eventline::info;
use halley_core::decay::DecayLevel;
use halley_core::viewport::FocusZone;
use halley_ipc::NodeMoveDirection;
use std::time::Instant;

pub(crate) fn promote_node_level(
    st: &mut HalleyWlState,
    node_id: halley_core::field::NodeId,
    now: Instant,
) -> bool {
    let Some(n) = st.field.node(node_id) else {
        return false;
    };
    if n.kind != halley_core::field::NodeKind::Surface {
        return false;
    }
    if n.state != halley_core::field::NodeState::Node {
        return false;
    }
    let target_pos = n.pos;

    let in_focus_ring =
        st.active_focus_ring().zone(st.viewport.center, target_pos) == FocusZone::Inside;

    if in_focus_ring {
        // This is a deliberate promote, not a stale auto-resurrect.
        st.manual_collapsed_nodes.remove(&node_id);

        let _ = st.field.set_decay_level(node_id, DecayLevel::Hot);
        st.mark_active_transition(node_id, now, 360);

        st.set_interaction_focus(Some(node_id), 30_000, now);
        return true;
    }

    st.set_interaction_focus(Some(node_id), 30_000, now);
    st.set_pan_restore_focus_target(node_id);
    st.animate_viewport_center_to(target_pos, now)
}

pub(crate) fn latest_surface_node(st: &HalleyWlState) -> Option<halley_core::field::NodeId> {
    st.last_input_surface_node().or_else(|| {
        st.surface_to_node
            .values()
            .copied()
            .max_by_key(|id| id.as_u64())
    })
}

pub(crate) fn set_docking_mode(st: &mut HalleyWlState, active: bool) -> bool {
    if active {
        st.docking_hold_count = st.docking_hold_count.saturating_add(1);
        return true;
    }

    let was_active = st.docking_hold_count > 0;
    st.docking_hold_count = st.docking_hold_count.saturating_sub(1);
    was_active
}

pub(crate) fn docking_mode_active(st: &HalleyWlState) -> bool {
    st.docking_hold_count > 0
}

pub(crate) fn move_latest_node(st: &mut HalleyWlState, dx: f32, dy: f32) -> bool {
    let Some(id) = latest_surface_node(st) else {
        return false;
    };
    let Some(n) = st.field.node(id) else {
        return false;
    };
    let to = halley_core::field::Vec2 {
        x: n.pos.x + dx,
        y: n.pos.y + dy,
    };
    let _ = st.field.set_pinned(id, false);
    st.begin_carry_state_tracking(id, false);
    if st.carry_surface_non_overlap(id, to, false) {
        st.update_carry_state_preview(id, Instant::now());
        st.end_carry_state_tracking(id);
        st.set_interaction_focus(Some(id), 30_000, Instant::now());
        if let Some(nn) = st.field.node(id) {
            info!(
                "moved node id={} to ({:.0},{:.0}) state={:?}",
                id.as_u64(),
                nn.pos.x,
                nn.pos.y,
                nn.state
            );
        }
        return true;
    }
    st.end_carry_state_tracking(id);
    false
}

pub(crate) fn move_latest_node_direction(
    st: &mut HalleyWlState,
    direction: NodeMoveDirection,
) -> bool {
    const STEP_NODE: f32 = 80.0;

    match direction {
        NodeMoveDirection::Left => move_latest_node(st, -STEP_NODE, 0.0),
        NodeMoveDirection::Right => move_latest_node(st, STEP_NODE, 0.0),
        NodeMoveDirection::Up => move_latest_node(st, 0.0, STEP_NODE),
        NodeMoveDirection::Down => move_latest_node(st, 0.0, -STEP_NODE),
    }
}

pub(crate) fn toggle_focused_active_node_state(st: &mut HalleyWlState) -> bool {
    let now = Instant::now();

    let Some(id) = st.last_focused_surface_node() else {
        return false;
    };

    let Some(n) = st.field.node(id) else {
        return false;
    };

    if n.kind != halley_core::field::NodeKind::Surface {
        return false;
    }


    match n.state {
        halley_core::field::NodeState::Active => {
            let _ = st.field.set_state(id, halley_core::field::NodeState::Node);
            let _ = st
                .field
                .set_decay_level(id, halley_core::decay::DecayLevel::Cold);
            st.pending_spawn_activate_at_ms.remove(&id);
            st.manual_collapsed_nodes.insert(id);

            st.set_interaction_focus(None, 0, now);
            st.pan_restore_active_focus = None;
            true
        }

        halley_core::field::NodeState::Node => {
            st.manual_collapsed_nodes.remove(&id);
            let _ = st
                .field
                .set_decay_level(id, halley_core::decay::DecayLevel::Hot);
            st.pending_spawn_activate_at_ms.remove(&id);
            st.mark_active_transition(id, now, 360);

            st.set_interaction_focus(Some(id), 30_000, now);
            true
        }

        _ => false,
    }
}
