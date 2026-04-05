use super::*;

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
    pub(super) fn cluster_mutation_controller(&mut self) -> ClusterMutationController<'_> {
        let crate::compositor::root::Halley {
            model,
            input,
            runtime,
            ..
        } = &mut **self;
        ClusterMutationController {
            field: &mut model.field,
            cluster_state: &mut model.cluster_state,
            interaction_state: &mut input.interaction_state,
            tuning: &runtime.tuning,
        }
    }

    pub(crate) fn sync_cluster_monitor(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        preferred: Option<&str>,
    ) -> bool {
        let Some(target_monitor) = self.preferred_monitor_for_cluster(cid, preferred) else {
            return false;
        };

        let (core_id, members) = if let Some(cluster) = self.model.field.cluster(cid) {
            (cluster.core, cluster.members().to_vec())
        } else {
            return false;
        };

        if let Some(core_id) = core_id {
            self.assign_node_to_monitor(core_id, target_monitor.as_str());
        }
        for member_id in members {
            self.assign_node_to_monitor(member_id, target_monitor.as_str());
        }
        true
    }

    fn dissolve_cluster(&mut self, cid: ClusterId) -> bool {
        let core_id = self
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| cluster.core);
        self.clear_cluster_shell_state(cid);
        if let Some(core_id) = core_id {
            self.model.monitor_state.node_monitor.remove(&core_id);
        }
        self.model.field.dissolve_cluster(cid)
    }

    pub(crate) fn remove_node_from_field(&mut self, id: NodeId, now_ms: u64) -> bool {
        let stack_remove_transition = self
            .model
            .field
            .cluster_id_for_member_public(id)
            .and_then(|cid| self.preferred_monitor_for_cluster(cid, None).map(|monitor| (cid, monitor)))
            .filter(|(cid, monitor)| self.active_cluster_workspace_for_monitor(monitor.as_str()) == Some(*cid))
            .filter(|_| {
                matches!(
                    self.active_cluster_layout_kind(),
                    ClusterWorkspaceLayoutKind::Stacking
                )
            })
            .map(|(_, monitor)| {
                let old_visible = crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                    self,
                    monitor.as_str(),
                );
                (monitor, old_visible)
            });
        let cluster_snapshot = self
            .model
            .field
            .cluster_id_for_member_public(id)
            .and_then(|cid| {
                self.model
                    .field
                    .cluster(cid)
                    .map(|cluster| (cid, cluster.members().to_vec(), cluster.core))
            });
        let (snapshot_cid, snapshot_members, snapshot_core_id) =
            cluster_snapshot.unwrap_or((ClusterId::new(0), Vec::new(), None));
        let Some((_, effect)) = self.model.field.remove_node_cluster_safe(id) else {
            return false;
        };

        match effect {
            Some(RemoveNodeClusterEffect::RemovedMember(cid)) => {
                if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None)
                    && self.active_cluster_workspace_for_monitor(cluster_monitor.as_str())
                        == Some(cid)
                {
                    self.layout_active_cluster_workspace_for_monitor(
                        cluster_monitor.as_str(),
                        now_ms,
                    );
                    if let Some((transition_monitor, old_visible)) = stack_remove_transition.as_ref()
                        && transition_monitor == &cluster_monitor
                    {
                        let new_visible = crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                            self,
                            cluster_monitor.as_str(),
                        );
                        self.ui.render_state.start_stack_cycle_transition(
                            cluster_monitor.as_str(),
                            ClusterCycleDirection::Prev,
                            old_visible.clone(),
                            new_visible,
                            Instant::now(),
                            220,
                        );
                        self.request_maintenance();
                    }
                }
            }
            Some(RemoveNodeClusterEffect::DissolvedCluster(cid)) => {
                let survivors = if snapshot_cid == cid {
                    snapshot_members
                        .iter()
                        .copied()
                        .filter(|member| *member != id && self.model.field.node(*member).is_some())
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                self.clear_cluster_shell_state(cid);
                if let Some(core_id) = snapshot_core_id.filter(|_| snapshot_cid == cid) {
                    self.model.monitor_state.node_monitor.remove(&core_id);
                }
                for survivor in survivors {
                    let _ = self.model.field.set_detached(survivor, false);
                    if let Some(size) = self
                        .model
                        .workspace_state
                        .last_active_size
                        .get(&survivor)
                        .copied()
                    {
                        if let Some(node) = self.model.field.node_mut(survivor) {
                            node.intrinsic_size = size;
                        }
                        self.request_toplevel_resize(
                            survivor,
                            size.x.round() as i32,
                            size.y.round() as i32,
                        );
                    }
                    let _ = self.model.field.touch(survivor, now_ms);
                }
            }
            Some(RemoveNodeClusterEffect::RemovedCore(cid)) => {
                self.model.monitor_state.node_monitor.remove(&id);
                let _ = self.sync_cluster_monitor(cid, None);
            }
            None => {}
        }

        true
    }

    pub fn detach_member_from_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        member_id: NodeId,
        world_pos: Vec2,
        now: Instant,
    ) -> bool {
        let now_ms = self.now_ms(now);
        let Some(outcome) = self
            .cluster_mutation_controller()
            .detach_member_from_cluster(cid, member_id, world_pos, now_ms)
        else {
            return false;
        };
        match outcome {
            ClusterRemoveMemberOutcome::Removed => {
                if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None)
                    && self.active_cluster_workspace_for_monitor(cluster_monitor.as_str())
                        == Some(cid)
                {
                    self.layout_active_cluster_workspace_for_monitor(
                        cluster_monitor.as_str(),
                        now_ms,
                    );
                }
            }
            ClusterRemoveMemberOutcome::RequiresDissolve => {
                if !self.dissolve_cluster(cid) {
                    return false;
                }
            }
        }
        true
    }

    pub fn absorb_node_into_cluster(
        &mut self,
        cid: halley_core::cluster::ClusterId,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        let previous_overflow_len = self.cluster_overflow_len(cid);
        let stack_insert_transition = self
            .preferred_monitor_for_cluster(cid, None)
            .filter(|monitor| {
                self.active_cluster_workspace_for_monitor(monitor.as_str()) == Some(cid)
            })
            .filter(|_| {
                matches!(
                    self.active_cluster_layout_kind(),
                    ClusterWorkspaceLayoutKind::Stacking
                )
            })
            .map(|monitor| {
                let old_visible =
                    crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                        self,
                        monitor.as_str(),
                    );
                (monitor, old_visible)
            });
        if !self
            .cluster_mutation_controller()
            .absorb_node_into_cluster(cid, node_id)
        {
            return false;
        }
        if let Some(cluster_monitor) = self.preferred_monitor_for_cluster(cid, None) {
            self.assign_node_to_monitor(node_id, cluster_monitor.as_str());
            if self.active_cluster_workspace_for_monitor(cluster_monitor.as_str()) == Some(cid) {
                if let Some(node) = self.model.field.node_mut(node_id) {
                    node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
                }
                let now_ms = self.now_ms(now);
                self.layout_active_cluster_workspace_for_monitor(cluster_monitor.as_str(), now_ms);
                if matches!(
                    self.active_cluster_layout_kind(),
                    ClusterWorkspaceLayoutKind::Stacking
                ) {
                    if let Some((transition_monitor, old_visible)) =
                        stack_insert_transition.as_ref()
                        && transition_monitor == &cluster_monitor
                    {
                        let new_visible =
                            crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                                self,
                                cluster_monitor.as_str(),
                            );
                        self.ui.render_state.start_stack_cycle_transition(
                            cluster_monitor.as_str(),
                            ClusterCycleDirection::Prev,
                            old_visible.clone(),
                            new_visible,
                            now,
                            220,
                        );
                        self.request_maintenance();
                    }
                    self.set_recent_top_node(node_id, now + std::time::Duration::from_millis(1200));
                    self.set_interaction_focus(Some(node_id), 30_000, now);
                    self.update_focus_tracking_for_surface(node_id, now_ms);
                }
                let overflow_len = self.cluster_overflow_len(cid);
                if overflow_len > previous_overflow_len {
                    self.reveal_cluster_overflow_for_monitor(cluster_monitor.as_str(), now_ms);
                }
            }
        }
        if let Some(core_id) = self
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| cluster.core)
        {
            let now_ms = self.now_ms(now);
            let _ = self.model.field.touch(core_id, now_ms);
        }
        true
    }

    pub(crate) fn commit_ready_cluster_join_for_node(
        &mut self,
        node_id: NodeId,
        now: Instant,
    ) -> bool {
        let Some(candidate) = self
            .input
            .interaction_state
            .cluster_join_candidate
            .clone()
            .filter(|candidate| candidate.node_id == node_id && candidate.ready)
        else {
            return false;
        };
        self.input.interaction_state.cluster_join_candidate = None;
        self.absorb_node_into_cluster(candidate.cluster_id, node_id, now)
    }
}
