use super::*;

const CLUSTER_OVERFLOW_REVEAL_MS: u64 = 2200;

pub(crate) fn adjust_cluster_overflow_scroll_for_monitor(
    st: &mut Halley,
    monitor: &str,
    delta: i32,
) -> bool {
    let overflow_len = st
        .model
        .cluster_state
        .cluster_overflow_members
        .get(monitor)
        .map(Vec::len)
        .unwrap_or(0);
    let max_offset = overflow_len.saturating_sub(CLUSTER_OVERFLOW_VISIBLE_SLOTS);
    if max_offset == 0 {
        st.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .remove(monitor);
        return false;
    }
    let current = st
        .model
        .cluster_state
        .cluster_overflow_scroll_offsets
        .get(monitor)
        .copied()
        .unwrap_or(0) as i32;
    let next = (current + delta).clamp(0, max_offset as i32) as usize;
    if next == current as usize {
        return false;
    }
    st.model
        .cluster_state
        .cluster_overflow_scroll_offsets
        .insert(monitor.to_string(), next);
    true
}

pub(super) fn clear_cluster_overflow_for_monitor(st: &mut Halley, monitor: &str) {
    st.model
        .cluster_state
        .cluster_overflow_members
        .remove(monitor);
    st.model
        .cluster_state
        .cluster_overflow_rects
        .remove(monitor);
    st.model
        .cluster_state
        .cluster_overflow_scroll_offsets
        .remove(monitor);
    st.model
        .cluster_state
        .cluster_overflow_reveal_started_at_ms
        .remove(monitor);
    st.model
        .cluster_state
        .cluster_overflow_visible_until_ms
        .remove(monitor);
}

pub(super) fn refresh_cluster_overflow_for_monitor(
    st: &mut Halley,
    monitor: &str,
    now_ms: u64,
    reveal: bool,
) {
    let Some(_cid) = active_cluster_workspace_for_monitor(st, monitor) else {
        clear_cluster_overflow_for_monitor(st, monitor);
        return;
    };
    let Some(plan) = crate::compositor::clusters::read::plan_active_cluster_layout(st, monitor)
    else {
        clear_cluster_overflow_for_monitor(st, monitor);
        return;
    };
    let overflow = plan.overflow_members;
    if overflow.is_empty() {
        clear_cluster_overflow_for_monitor(st, monitor);
        return;
    }

    let was_visible = st
        .model
        .cluster_state
        .cluster_overflow_visible_until_ms
        .get(monitor)
        .is_some_and(|visible_until_ms| *visible_until_ms > now_ms);

    st.model
        .cluster_state
        .cluster_overflow_members
        .insert(monitor.to_string(), overflow.clone());
    let max_offset = overflow
        .len()
        .saturating_sub(CLUSTER_OVERFLOW_VISIBLE_SLOTS);
    if max_offset == 0 {
        st.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .remove(monitor);
    } else {
        let next = st
            .model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .get(monitor)
            .copied()
            .unwrap_or(0)
            .min(max_offset);
        st.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .insert(monitor.to_string(), next);
    }
    if let Some(rect) = crate::compositor::clusters::read::overflow_strip_rect_for_monitor(
        st,
        monitor,
        overflow.len(),
    ) {
        st.model
            .cluster_state
            .cluster_overflow_rects
            .insert(monitor.to_string(), rect);
    }
    if reveal {
        if !was_visible {
            st.model
                .cluster_state
                .cluster_overflow_reveal_started_at_ms
                .insert(monitor.to_string(), now_ms);
        }
        st.model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .insert(
                monitor.to_string(),
                now_ms.saturating_add(CLUSTER_OVERFLOW_REVEAL_MS),
            );
        st.request_maintenance();
    }
}

pub(crate) fn reveal_cluster_overflow_for_monitor(st: &mut Halley, monitor: &str, now_ms: u64) {
    refresh_cluster_overflow_for_monitor(st, monitor, now_ms, true);
}

pub(crate) fn hide_cluster_overflow_for_monitor(st: &mut Halley, monitor: &str) {
    st.model
        .cluster_state
        .cluster_overflow_scroll_offsets
        .remove(monitor);
    st.model
        .cluster_state
        .cluster_overflow_reveal_started_at_ms
        .remove(monitor);
    st.model
        .cluster_state
        .cluster_overflow_visible_until_ms
        .remove(monitor);
}

pub(crate) fn swap_cluster_overflow_member_with_visible(
    st: &mut Halley,
    monitor: &str,
    cid: ClusterId,
    overflow_member: NodeId,
    visible_member: NodeId,
    now_ms: u64,
) -> bool {
    if active_cluster_workspace_for_monitor(st, monitor) != Some(cid) {
        return false;
    }
    if !matches!(
        active_cluster_layout_kind(st),
        ClusterWorkspaceLayoutKind::Tiling
    ) {
        return false;
    }
    let max_stack = st.runtime.tuning.tile_max_stack;
    if !st.model.field.swap_cluster_overflow_member_with_visible(
        cid,
        overflow_member,
        visible_member,
        max_stack,
    ) {
        return false;
    }
    layout_active_cluster_workspace_for_monitor(st, monitor, now_ms);
    reveal_cluster_overflow_for_monitor(st, monitor, now_ms);
    true
}

pub(crate) fn reorder_cluster_overflow_member(
    st: &mut Halley,
    monitor: &str,
    cid: ClusterId,
    member: NodeId,
    target_overflow_index: usize,
    now_ms: u64,
) -> bool {
    if active_cluster_workspace_for_monitor(st, monitor) != Some(cid) {
        return false;
    }
    if !matches!(
        active_cluster_layout_kind(st),
        ClusterWorkspaceLayoutKind::Tiling
    ) {
        return false;
    }
    let max_stack = st.runtime.tuning.tile_max_stack;
    if !st.model.field.reorder_cluster_overflow_member(
        cid,
        member,
        target_overflow_index,
        max_stack,
    ) {
        return false;
    }
    refresh_cluster_overflow_for_monitor(st, monitor, now_ms, true);
    true
}
