use super::naming::cluster_mode_selection_banner;
use super::*;

pub fn cluster_bloom_for_monitor(
    st: &mut Halley,
    monitor: &str,
) -> Option<halley_core::cluster::ClusterId> {
    crate::compositor::clusters::read::cluster_bloom_for_monitor(st, monitor)
}

pub fn open_cluster_bloom_for_monitor(
    st: &mut Halley,
    monitor: &str,
    cid: halley_core::cluster::ClusterId,
) -> bool {
    let _ = sync_cluster_monitor(st, cid, Some(monitor));
    let opened = super::open_cluster_bloom_for_monitor_inner(st, monitor, cid);
    if opened && let Some(core_id) = st.model.field.cluster(cid).and_then(|cluster| cluster.core) {
        st.set_interaction_focus(Some(core_id), 30_000, Instant::now());
    }
    opened
}

pub fn close_cluster_bloom_for_monitor(st: &mut Halley, monitor: &str) -> bool {
    let core_id = cluster_bloom_for_monitor(st, monitor)
        .and_then(|cid| st.model.field.cluster(cid).and_then(|cluster| cluster.core));
    let closed = super::close_cluster_bloom_for_monitor_inner(st, monitor);
    if closed {
        let now = Instant::now();
        if let Some(core_id) = core_id {
            st.set_recent_top_node(core_id, now + std::time::Duration::from_millis(1200));
            st.set_interaction_focus(Some(core_id), 30_000, now);
        }
    }
    closed
}

pub fn enter_cluster_mode(st: &mut Halley) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    if active_cluster_workspace_for_monitor(st, monitor.as_str()).is_some() {
        let now_ms = st.now_ms(Instant::now());
        st.ui.render_state.show_overlay_toast(
            monitor.as_str(),
            "Cluster mode unavailable\nExit the workspace first",
            3200,
            now_ms,
        );
        return false;
    }
    if !super::enter_cluster_mode_inner(st, monitor.as_str()) {
        return false;
    }
    st.begin_modal_keyboard_capture();
    cluster_mode_selection_banner(st, monitor.as_str());
    true
}

pub fn exit_cluster_mode(st: &mut Halley) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    if !super::exit_cluster_mode_inner(st, monitor.as_str()) {
        return false;
    }
    st.model
        .cluster_state
        .cluster_name_prompt
        .remove(monitor.as_str());
    if st
        .input
        .interaction_state
        .cluster_name_prompt_drag_monitor
        .as_deref()
        == Some(monitor.as_str())
    {
        st.input.interaction_state.cluster_name_prompt_drag_monitor = None;
    }
    if st
        .input
        .interaction_state
        .cluster_name_prompt_repeat
        .as_ref()
        .is_some_and(|repeat| repeat.monitor == monitor)
    {
        st.input.interaction_state.cluster_name_prompt_repeat = None;
    }
    st.ui
        .render_state
        .clear_persistent_mode_banner(monitor.as_str());
    let focused_surface = st
        .model
        .focus_state
        .primary_interaction_focus
        .filter(|&id| {
            st.model.field.node(id).is_some_and(|node| {
                st.model.field.is_visible(id) && node.kind == halley_core::field::NodeKind::Surface
            })
        })
        .or_else(|| st.last_input_surface_node_for_monitor(monitor.as_str()));
    st.schedule_modal_focus_restore(focused_surface, Instant::now());
    true
}

pub fn toggle_cluster_mode_selection(st: &mut Halley, node_id: NodeId) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    super::toggle_cluster_mode_selection_inner(st, monitor.as_str(), node_id)
}

pub(super) fn order_cluster_creation_members(st: &Halley, members: Vec<NodeId>) -> Vec<NodeId> {
    if members.len() <= 1 {
        return members;
    }

    let selected = members.iter().copied().collect::<HashSet<_>>();
    let master = st
        .model
        .focus_state
        .primary_interaction_focus
        .filter(|id| selected.contains(id))
        .or_else(|| {
            members.iter().copied().max_by_key(|id| {
                (
                    st.model
                        .focus_state
                        .last_surface_focus_ms
                        .get(id)
                        .copied()
                        .unwrap_or(0),
                    std::cmp::Reverse(id.as_u64()),
                )
            })
        })
        .unwrap_or(members[0]);

    let mut secondaries = members
        .into_iter()
        .filter(|id| *id != master)
        .collect::<Vec<_>>();
    secondaries.sort_by_key(|id| id.as_u64());

    let mut ordered = Vec::with_capacity(secondaries.len() + 1);
    ordered.push(master);
    ordered.extend(secondaries);
    ordered
}

pub fn confirm_cluster_mode(st: &mut Halley, now: Instant) -> bool {
    open_cluster_name_prompt(st, now)
}
