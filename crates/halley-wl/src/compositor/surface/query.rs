use std::collections::HashMap;

use halley_core::cluster_layout::{ClusterWorkspaceLayoutKind, cluster_visible_limit};

use crate::compositor::root::Halley;

pub(crate) fn is_active_cluster_workspace_member(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
    st.model
        .field
        .cluster_id_for_member_public(node_id)
        .zip(st.model.monitor_state.node_monitor.get(&node_id))
        .is_some_and(|(cid, monitor)| st.active_cluster_workspace_for_monitor(monitor) == Some(cid))
}

pub(crate) fn active_stacking_front_member_for_monitor(
    st: &Halley,
    monitor: &str,
) -> Option<halley_core::field::NodeId> {
    if !matches!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Stacking
    ) {
        return None;
    }
    let cid = st.active_cluster_workspace_for_monitor(monitor)?;
    st.model.field.cluster(cid)?.members().first().copied()
}

pub(crate) fn active_stacking_visible_members_for_monitor(
    st: &Halley,
    monitor: &str,
) -> Vec<halley_core::field::NodeId> {
    if !matches!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Stacking
    ) {
        return Vec::new();
    }
    let Some(cid) = st.active_cluster_workspace_for_monitor(monitor) else {
        return Vec::new();
    };
    let Some(cluster) = st.model.field.cluster(cid) else {
        return Vec::new();
    };
    let visible_len = cluster_visible_limit(
        ClusterWorkspaceLayoutKind::Stacking,
        st.runtime.tuning.active_cluster_visible_limit(),
    )
    .min(cluster.members().len());
    cluster
        .members()
        .iter()
        .take(visible_len)
        .copied()
        .collect()
}

pub(crate) fn is_active_stacking_workspace_member(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
    let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id) else {
        return false;
    };
    active_stacking_visible_members_for_monitor(st, monitor.as_str()).contains(&node_id)
}

pub(crate) fn node_allows_interactive_resize(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
    st.model
        .field
        .node(node_id)
        .is_some_and(|node| node.state == halley_core::field::NodeState::Active)
        && !crate::compositor::workspace::state::node_in_maximize_session(st, node_id)
        && !is_active_stacking_workspace_member(st, node_id)
        && !(matches!(
            st.runtime.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) && is_active_cluster_workspace_member(st, node_id))
}

pub(crate) fn stacking_render_order_map(
    members: &[halley_core::field::NodeId],
    max_visible: usize,
) -> HashMap<halley_core::field::NodeId, usize> {
    let visible_len =
        cluster_visible_limit(ClusterWorkspaceLayoutKind::Stacking, max_visible).min(members.len());
    members
        .iter()
        .take(visible_len)
        .enumerate()
        .map(|(index, &node_id)| (node_id, visible_len.saturating_sub(index + 1)))
        .collect()
}

pub(crate) fn active_stacking_render_order_for_monitor(
    st: &Halley,
    monitor: &str,
) -> HashMap<halley_core::field::NodeId, usize> {
    if !matches!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Stacking
    ) {
        return HashMap::new();
    }
    let Some(cid) = st.active_cluster_workspace_for_monitor(monitor) else {
        return HashMap::new();
    };
    let Some(cluster) = st.model.field.cluster(cid) else {
        return HashMap::new();
    };
    stacking_render_order_map(
        cluster.members(),
        st.runtime.tuning.active_cluster_visible_limit(),
    )
}

pub(crate) fn stack_focus_target_for_node(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<halley_core::field::NodeId> {
    let monitor = st.model.monitor_state.node_monitor.get(&node_id)?;
    let cid = st.model.field.cluster_id_for_member_public(node_id)?;
    (st.active_cluster_workspace_for_monitor(monitor.as_str()) == Some(cid))
        .then(|| active_stacking_front_member_for_monitor(st, monitor.as_str()))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;
    use std::time::Instant;

    #[test]
    fn active_cluster_workspace_member_matches_current_monitor_workspace() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());

        let monitor = st.model.monitor_state.current_monitor.clone();
        let master = st.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = st.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(master, monitor.as_str());
        st.assign_node_to_monitor(stack, monitor.as_str());

        let cid = st
            .model
            .field
            .create_cluster(vec![master, stack])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, monitor.as_str());

        assert!(!is_active_cluster_workspace_member(&st, master));
        assert!(st.toggle_cluster_workspace_by_core(core, Instant::now()));
        assert!(is_active_cluster_workspace_member(&st, master));
        assert!(is_active_cluster_workspace_member(&st, stack));
        assert!(!is_active_cluster_workspace_member(&st, core));
    }

    #[test]
    fn active_stacking_helpers_report_front_member_and_render_order() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.stacking_max_visible = 3;
        let mut st = Halley::new_for_test(&dh, tuning);

        let monitor = st.model.monitor_state.current_monitor.clone();
        let a = st.model.field.spawn_surface(
            "A",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let b = st.model.field.spawn_surface(
            "B",
            Vec2 { x: 120.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let c = st.model.field.spawn_surface(
            "C",
            Vec2 { x: 140.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let d = st.model.field.spawn_surface(
            "D",
            Vec2 { x: 160.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [a, b, c, d] {
            st.assign_node_to_monitor(id, monitor.as_str());
        }

        let cid = st
            .model
            .field
            .create_cluster(vec![a, b, c, d])
            .expect("cluster");
        let core = st.model.field.collapse_cluster(cid).expect("core");
        st.assign_node_to_monitor(core, monitor.as_str());
        assert!(st.toggle_cluster_workspace_by_core(core, Instant::now()));

        assert_eq!(
            active_stacking_front_member_for_monitor(&st, monitor.as_str()),
            Some(a)
        );

        let ranks = active_stacking_render_order_for_monitor(&st, monitor.as_str());
        assert_eq!(ranks.get(&a), Some(&2));
        assert_eq!(ranks.get(&b), Some(&1));
        assert_eq!(ranks.get(&c), Some(&0));
        assert_eq!(ranks.get(&d), None);
        assert_eq!(stack_focus_target_for_node(&st, c), Some(a));
    }

    #[test]
    fn maximize_session_blocks_interactive_resize() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);

        let monitor = st.model.monitor_state.current_monitor.clone();
        let window = st.model.field.spawn_surface(
            "window",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(window, monitor.as_str());

        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                window,
                Instant::now(),
                monitor.as_str(),
            )
        );

        assert!(!node_allows_interactive_resize(&st, window));
    }
}
