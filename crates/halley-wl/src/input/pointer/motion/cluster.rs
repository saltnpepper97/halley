use std::time::Instant;

use crate::compositor::root::Halley;

pub(super) fn cluster_join_dwell_ms(st: &Halley) -> u64 {
    st.runtime.tuning.cluster_dwell_ms
}

pub(super) fn update_cluster_join_candidate(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    monitor: &str,
    desired_center: halley_core::field::Vec2,
    now: Instant,
) -> bool {
    if st
        .model
        .field
        .cluster_id_for_member_public(node_id)
        .is_some()
    {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    }
    let Some(node) = st.model.field.node(node_id) else {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    };
    if node.kind != halley_core::field::NodeKind::Surface {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    }

    let mover_ext = if matches!(
        node.state,
        halley_core::field::NodeState::Active | halley_core::field::NodeState::Drifting
    ) {
        st.surface_window_collision_extents(node)
    } else {
        st.collision_extents_for_node(node)
    };
    let candidate = st.cluster_bloom_for_monitor(monitor).and_then(|open_cid| {
        let cluster = st.model.field.cluster(open_cid)?;
        if !cluster.is_collapsed() {
            return None;
        }
        let core_id = cluster.core?;
        let core = st.model.field.node(core_id)?;
        let core_monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&core_id)
            .map(String::as_str)
            .unwrap_or(monitor);
        if core_monitor != monitor {
            return None;
        }
        let core_ext = st.collision_extents_for_node(core);
        let gap = st.non_overlap_gap_world();
        let mover_left = desired_center.x - mover_ext.left;
        let mover_right = desired_center.x + mover_ext.right;
        let mover_top = desired_center.y - mover_ext.top;
        let mover_bottom = desired_center.y + mover_ext.bottom;
        let core_left = core.pos.x - core_ext.left - gap;
        let core_right = core.pos.x + core_ext.right + gap;
        let core_top = core.pos.y - core_ext.top - gap;
        let core_bottom = core.pos.y + core_ext.bottom + gap;
        let touching_gap = mover_right >= core_left
            && mover_left <= core_right
            && mover_bottom >= core_top
            && mover_top <= core_bottom;
        touching_gap.then_some(open_cid)
    });

    let Some(cluster_id) = candidate else {
        st.input.interaction_state.cluster_join_candidate = None;
        return false;
    };
    let now_ms = st.now_ms(now);
    let keep_started_at = st
        .input
        .interaction_state
        .cluster_join_candidate
        .as_ref()
        .filter(|existing| {
            existing.cluster_id == cluster_id
                && existing.node_id == node_id
                && existing.monitor == monitor
        })
        .map(|existing| existing.started_at_ms)
        .unwrap_or(now_ms);
    let dwell_ms = cluster_join_dwell_ms(st);
    st.input.interaction_state.cluster_join_candidate = Some(
        crate::compositor::interaction::state::ClusterJoinCandidate {
            cluster_id,
            node_id,
            monitor: monitor.to_string(),
            started_at_ms: keep_started_at,
            ready: now_ms.saturating_sub(keep_started_at) >= dwell_ms,
        },
    );
    false
}
