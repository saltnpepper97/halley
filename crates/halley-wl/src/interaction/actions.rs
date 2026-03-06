use std::time::Instant;
use eventline::info;
use halley_core::decay::DecayLevel;
use crate::state::HalleyWlState;

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

    // If this node is one half of a docked pair, promote both together.
    let partner = st.dock_partner(node_id).filter(|&pid| {
        st.dock_partner(pid) == Some(node_id)
    });

    let in_primary = st.active_rings().zone(st.viewport.center, target_pos)
        == halley_core::viewport::RingZone::Primary;

    if in_primary {
        let _ = st.field.set_decay_level(node_id, DecayLevel::Hot);
        st.mark_active_transition(node_id, now, 360);

        if let Some(pid) = partner {
            if st.field.is_visible(pid)
                && st.field.node(pid).is_some_and(|pn| {
                    pn.kind == halley_core::field::NodeKind::Surface
                        && pn.state == halley_core::field::NodeState::Node
                })
            {
                let _ = st.field.set_decay_level(pid, DecayLevel::Hot);
                st.mark_active_transition(pid, now, 360);
            }
        }

        st.set_interaction_focus(Some(node_id), 30_000, now);
        return true;
    }

    // Out of primary: pan to center. Set restore target to this node so
    // when it arrives both it and its partner will reopen via
    // restore_pan_return_active_focus (which already handles the pair).
    st.set_interaction_focus(Some(node_id), 30_000, now);
    st.set_pan_restore_focus_target(node_id);
    st.animate_viewport_center_to(target_pos, now)
}

pub(crate) fn move_latest_node(st: &mut HalleyWlState, dx: f32, dy: f32) {
    let latest = st.last_input_surface_node().or_else(|| {
        st.surface_to_node
            .values()
            .copied()
            .max_by_key(|id| id.as_u64())
    });
    let Some(id) = latest else {
        return;
    };
    let Some(n) = st.field.node(id) else {
        return;
    };
    let to = halley_core::field::Vec2 {
        x: n.pos.x + dx,
        y: n.pos.y + dy,
    };
    st.begin_carry_state_tracking(id);
    if st.carry_surface_non_overlap(id, to) {
        st.update_carry_state_preview(id, Instant::now());
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
    }
}

pub(crate) fn minimize_focused_active_node(st: &mut HalleyWlState) -> bool {
    st.toggle_last_focused_surface_node(Instant::now())
        .is_some()
}
