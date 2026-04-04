use super::*;

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
    pub fn cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
    ) -> Option<halley_core::cluster::ClusterId> {
        self.cluster_read_controller()
            .cluster_bloom_for_monitor(monitor)
    }

    pub fn open_cluster_bloom_for_monitor(
        &mut self,
        monitor: &str,
        cid: halley_core::cluster::ClusterId,
    ) -> bool {
        let _ = self.sync_cluster_monitor(cid, Some(monitor));
        let opened = self
            .cluster_mutation_controller()
            .open_cluster_bloom_for_monitor(monitor, cid);
        if opened
            && let Some(core_id) = self
                .model
                .field
                .cluster(cid)
                .and_then(|cluster| cluster.core)
        {
            self.set_interaction_focus(Some(core_id), 30_000, Instant::now());
        }
        opened
    }

    pub fn close_cluster_bloom_for_monitor(&mut self, monitor: &str) -> bool {
        let closed = self
            .cluster_mutation_controller()
            .close_cluster_bloom_for_monitor(monitor);
        if closed {
            let now = Instant::now();
            let restore = self
                .last_focused_surface_node_for_monitor(monitor)
                .or_else(|| self.last_focused_surface_node());
            self.set_interaction_focus(restore, 30_000, now);
        }
        closed
    }

    pub fn enter_cluster_mode(&mut self) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        if self
            .active_cluster_workspace_for_monitor(monitor.as_str())
            .is_some()
        {
            let now_ms = self.now_ms(Instant::now());
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Cluster mode unavailable\nExit the workspace first",
                3200,
                now_ms,
            );
            return false;
        }
        if !self
            .cluster_mutation_controller()
            .enter_cluster_mode(monitor.as_str())
        {
            return false;
        }
        self.ui.render_state.set_persistent_mode_banner(
            monitor.as_str(),
            "Cluster mode",
            Some("Select windows"),
            &[
                OverlayActionHint {
                    key: "Enter".to_string(),
                    label: "create".to_string(),
                },
                OverlayActionHint {
                    key: "Esc".to_string(),
                    label: "cancel".to_string(),
                },
            ],
        );
        true
    }

    pub fn exit_cluster_mode(&mut self) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        if !self
            .cluster_mutation_controller()
            .exit_cluster_mode(monitor.as_str())
        {
            return false;
        }
        self.ui
            .render_state
            .clear_persistent_mode_banner(monitor.as_str());
        true
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.cluster_mutation_controller()
            .toggle_cluster_mode_selection(monitor.as_str(), node_id)
    }

    fn order_cluster_creation_members(&self, members: Vec<NodeId>) -> Vec<NodeId> {
        if members.len() <= 1 {
            return members;
        }

        let selected = members.iter().copied().collect::<HashSet<_>>();
        let master = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|id| selected.contains(id))
            .or_else(|| {
                members.iter().copied().max_by_key(|id| {
                    (
                        self.model
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

    pub fn confirm_cluster_mode(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        let Some(selected_nodes) = self
            .model
            .cluster_state
            .cluster_mode_selected_nodes
            .get(monitor.as_str())
        else {
            return false;
        };
        let now_ms = self.now_ms(now);
        if selected_nodes.is_empty() {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "No selections\nSelect at least two windows",
                2200,
                now_ms,
            );
            return false;
        }

        let members = selected_nodes.iter().copied().collect::<Vec<_>>();
        if members.len() == 1 {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Not enough selections\nSelect at least two windows",
                5000,
                now_ms,
            );
            return false;
        }
        let members = self.order_cluster_creation_members(members);
        let created = self
            .model
            .field
            .create_cluster(members)
            .ok()
            .and_then(|cid| {
                let core = self.model.field.collapse_cluster(cid);
                if let Some(core_id) = core {
                    self.assign_node_to_current_monitor(core_id);
                    let _ = self.model.field.touch(core_id, now_ms);
                    self.set_interaction_focus(Some(core_id), 30_000, now);
                }
                core
            });
        let _ = self.exit_cluster_mode();
        created.is_some()
    }
}
