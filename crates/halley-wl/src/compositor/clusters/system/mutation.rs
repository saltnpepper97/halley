use super::*;

pub(crate) fn monitor_has_visible_surface_node(st: &Halley, monitor: &str) -> bool {
    st.model.field.nodes().values().any(|node| {
        node.kind == halley_core::field::NodeKind::Surface
            && st.model.field.is_visible(node.id)
            && st
                .model
                .monitor_state
                .node_monitor
                .get(&node.id)
                .is_some_and(|node_monitor| node_monitor == monitor)
    })
}

pub(crate) fn reset_empty_monitor_spawn_state(st: &mut Halley, monitor: &str) {
    if monitor_has_visible_surface_node(st, monitor) {
        return;
    }

    let view_anchor =
        crate::compositor::spawn::state::default_spawn_view_anchor_for_monitor(st, monitor);
    let spawn = st.spawn_monitor_state_mut(monitor);
    spawn.spawn_patch = None;
    spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
    spawn.spawn_view_anchor = view_anchor;
    spawn.spawn_focus_override = None;
    spawn.spawn_pan_start_center = None;
    st.model.focus_state.monitor_focus.remove(monitor);
}

pub(crate) fn sync_cluster_monitor(
    st: &mut Halley,
    cid: halley_core::cluster::ClusterId,
    preferred: Option<&str>,
) -> bool {
    let Some(target_monitor) = preferred_monitor_for_cluster(st, cid, preferred) else {
        return false;
    };

    let (core_id, members) = if let Some(cluster) = st.model.field.cluster(cid) {
        (cluster.core, cluster.members().to_vec())
    } else {
        return false;
    };

    if let Some(core_id) = core_id {
        st.assign_node_to_monitor(core_id, target_monitor.as_str());
    }
    for member_id in members {
        st.assign_node_to_monitor(member_id, target_monitor.as_str());
    }
    let _ = sync_cluster_name_for_monitor(st, cid, target_monitor.as_str());
    true
}

pub(crate) fn dissolve_cluster(st: &mut Halley, cid: ClusterId) -> bool {
    let core_id = st.model.field.cluster(cid).and_then(|cluster| cluster.core);
    clear_cluster_shell_state(st, cid);
    if let Some(core_id) = core_id {
        st.model.monitor_state.node_monitor.remove(&core_id);
        st.model.workspace_state.user_pinned_nodes.remove(&core_id);
    }
    remove_cluster_name_record(st, cid);
    st.model.field.dissolve_cluster(cid)
}

pub(crate) fn remove_node_from_field(st: &mut Halley, id: NodeId, now_ms: u64) -> bool {
    let removed_monitor = st.model.monitor_state.node_monitor.get(&id).cloned();
    let stack_remove_transition = st
        .model
        .field
        .cluster_id_for_member_public(id)
        .and_then(|cid| preferred_monitor_for_cluster(st, cid, None).map(|monitor| (cid, monitor)))
        .filter(|(cid, monitor)| {
            active_cluster_workspace_for_monitor(st, monitor.as_str()) == Some(*cid)
        })
        .filter(|_| {
            matches!(
                active_cluster_layout_kind(st),
                ClusterWorkspaceLayoutKind::Stacking
            )
        })
        .map(|(_, monitor)| {
            let old_visible =
                crate::compositor::surface::active_stacking_visible_members_for_monitor(
                    st,
                    monitor.as_str(),
                );
            (monitor, old_visible)
        });
    let cluster_snapshot = st
        .model
        .field
        .cluster_id_for_member_public(id)
        .and_then(|cid| {
            st.model
                .field
                .cluster(cid)
                .map(|cluster| (cid, cluster.members().to_vec(), cluster.core))
        });
    let (snapshot_cid, snapshot_members, snapshot_core_id) =
        cluster_snapshot.unwrap_or((ClusterId::new(0), Vec::new(), None));
    let Some((_, effect)) = st.model.field.remove_node_cluster_safe(id) else {
        return false;
    };

    match effect {
        Some(RemoveNodeClusterEffect::RemovedMember(cid)) => {
            if let Some(cluster_monitor) = preferred_monitor_for_cluster(st, cid, None)
                && active_cluster_workspace_for_monitor(st, cluster_monitor.as_str()) == Some(cid)
            {
                layout_active_cluster_workspace_for_monitor(st, cluster_monitor.as_str(), now_ms);
                if let Some((transition_monitor, old_visible)) = stack_remove_transition.as_ref()
                    && transition_monitor == &cluster_monitor
                {
                    let duration_ms = st.runtime.tuning.stack_animation_duration_ms();
                    let new_visible =
                        crate::compositor::surface::active_stacking_visible_members_for_monitor(
                            st,
                            cluster_monitor.as_str(),
                        );
                    if st.runtime.tuning.stack_animation_enabled() {
                        let transition_now = Instant::now();
                        for node_id in old_visible.iter().chain(new_visible.iter()).copied() {
                            st.request_window_animation_prewarm(node_id, transition_now);
                        }
                        st.ui.render_state.start_stack_cycle_transition(
                            cluster_monitor.as_str(),
                            ClusterCycleDirection::Prev,
                            old_visible.clone(),
                            new_visible,
                            transition_now,
                            duration_ms,
                        );
                        st.request_maintenance();
                    }
                }
            }
        }
        Some(RemoveNodeClusterEffect::DissolvedCluster(cid)) => {
            remove_cluster_name_record(st, cid);
            let survivors = if snapshot_cid == cid {
                snapshot_members
                    .iter()
                    .copied()
                    .filter(|member| *member != id && st.model.field.node(*member).is_some())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            clear_cluster_shell_state(st, cid);
            if let Some(core_id) = snapshot_core_id.filter(|_| snapshot_cid == cid) {
                st.model.monitor_state.node_monitor.remove(&core_id);
            }
            for survivor in survivors {
                if st
                    .model
                    .workspace_state
                    .pending_silent_close_until_ms
                    .contains_key(&survivor)
                    && let Some(node) = st.model.field.node_mut(survivor)
                {
                    node.visibility
                        .set(halley_core::field::Visibility::HIDDEN_BY_CLUSTER, true);
                }
                let _ = st.model.field.set_detached(survivor, false);
                if let Some(size) = st
                    .model
                    .workspace_state
                    .last_active_size
                    .get(&survivor)
                    .copied()
                {
                    if let Some(node) = st.model.field.node_mut(survivor) {
                        node.intrinsic_size = size;
                    }
                    st.request_toplevel_resize(
                        survivor,
                        size.x.round() as i32,
                        size.y.round() as i32,
                    );
                }
                let _ = st.model.field.touch(survivor, now_ms);
            }
        }
        Some(RemoveNodeClusterEffect::RemovedCore(cid)) => {
            st.model.monitor_state.node_monitor.remove(&id);
            let _ = sync_cluster_monitor(st, cid, None);
        }
        None => {}
    }

    if let Some(monitor) = removed_monitor {
        reset_empty_monitor_spawn_state(st, monitor.as_str());
    }

    true
}

pub fn detach_member_from_cluster(
    st: &mut Halley,
    cid: halley_core::cluster::ClusterId,
    member_id: NodeId,
    world_pos: Vec2,
    now: Instant,
) -> bool {
    let now_ms = st.now_ms(now);
    let Some(outcome) =
        super::detach_member_from_cluster_inner(st, cid, member_id, world_pos, now_ms)
    else {
        return false;
    };
    match outcome {
        ClusterRemoveMemberOutcome::Removed => {
            if let Some(cluster_monitor) = preferred_monitor_for_cluster(st, cid, None)
                && active_cluster_workspace_for_monitor(st, cluster_monitor.as_str()) == Some(cid)
            {
                layout_active_cluster_workspace_for_monitor(st, cluster_monitor.as_str(), now_ms);
            }
        }
        ClusterRemoveMemberOutcome::RequiresDissolve => {
            if !dissolve_cluster(st, cid) {
                return false;
            }
        }
    }
    true
}

pub fn absorb_node_into_cluster(
    st: &mut Halley,
    cid: halley_core::cluster::ClusterId,
    node_id: NodeId,
    now: Instant,
) -> bool {
    let previous_overflow_len = cluster_overflow_len(st, cid);
    let stack_insert_transition = preferred_monitor_for_cluster(st, cid, None)
        .filter(|monitor| active_cluster_workspace_for_monitor(st, monitor.as_str()) == Some(cid))
        .filter(|_| {
            matches!(
                active_cluster_layout_kind(st),
                ClusterWorkspaceLayoutKind::Stacking
            )
        })
        .map(|monitor| {
            let old_visible =
                crate::compositor::surface::active_stacking_visible_members_for_monitor(
                    st,
                    monitor.as_str(),
                );
            (monitor, old_visible)
        });
    let was_pinned = st.node_user_pinned(node_id);
    if !super::absorb_node_into_cluster_inner(st, cid, node_id) {
        return false;
    }

    if was_pinned {
        st.model.workspace_state.user_pinned_nodes.remove(&node_id);
        if let Some(core_id) = st.model.field.cluster(cid).and_then(|c| c.core) {
            st.model.workspace_state.user_pinned_nodes.insert(core_id);
        }
    }

    if let Some(cluster_monitor) = preferred_monitor_for_cluster(st, cid, None) {
        st.assign_node_to_monitor(node_id, cluster_monitor.as_str());
        if active_cluster_workspace_for_monitor(st, cluster_monitor.as_str()) == Some(cid) {
            if let Some(node) = st.model.field.node_mut(node_id) {
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            }
            let now_ms = st.now_ms(now);
            layout_active_cluster_workspace_for_monitor(st, cluster_monitor.as_str(), now_ms);
            if matches!(
                active_cluster_layout_kind(st),
                ClusterWorkspaceLayoutKind::Stacking
            ) {
                if let Some((transition_monitor, old_visible)) = stack_insert_transition.as_ref()
                    && transition_monitor == &cluster_monitor
                {
                    let duration_ms = st.runtime.tuning.stack_animation_duration_ms();
                    let new_visible =
                        crate::compositor::surface::active_stacking_visible_members_for_monitor(
                            st,
                            cluster_monitor.as_str(),
                        );
                    if st.runtime.tuning.stack_animation_enabled() {
                        for node_id in old_visible.iter().chain(new_visible.iter()).copied() {
                            st.request_window_animation_prewarm(node_id, now);
                        }
                        st.ui.render_state.start_stack_cycle_transition(
                            cluster_monitor.as_str(),
                            ClusterCycleDirection::Prev,
                            old_visible.clone(),
                            new_visible,
                            now,
                            duration_ms,
                        );
                        st.request_maintenance();
                    }
                }
                st.set_recent_top_node(node_id, now + std::time::Duration::from_millis(1200));
                st.set_interaction_focus(Some(node_id), 30_000, now);
                st.update_focus_tracking_for_surface(node_id, now_ms);
            }
            let overflow_len = cluster_overflow_len(st, cid);
            if overflow_len > previous_overflow_len {
                reveal_cluster_overflow_for_monitor(st, cluster_monitor.as_str(), now_ms);
            }
        }
    }
    if let Some(core_id) = st.model.field.cluster(cid).and_then(|cluster| cluster.core) {
        let now_ms = st.now_ms(now);
        let _ = st.model.field.touch(core_id, now_ms);
    }
    true
}

pub(crate) fn commit_ready_cluster_join_for_node(
    st: &mut Halley,
    node_id: NodeId,
    now: Instant,
) -> bool {
    let Some(candidate) = st
        .input
        .interaction_state
        .cluster_join_candidate
        .clone()
        .filter(|candidate| candidate.node_id == node_id && candidate.ready)
    else {
        return false;
    };
    st.input.interaction_state.cluster_join_candidate = None;
    absorb_node_into_cluster(st, candidate.cluster_id, node_id, now)
}
