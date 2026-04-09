use super::naming::cluster_mode_selection_banner;
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
        let core_id = self.cluster_bloom_for_monitor(monitor).and_then(|cid| {
            self.model
                .field
                .cluster(cid)
                .and_then(|cluster| cluster.core)
        });
        let closed = self
            .cluster_mutation_controller()
            .close_cluster_bloom_for_monitor(monitor);
        if closed {
            let now = Instant::now();
            if let Some(core_id) = core_id {
                self.set_recent_top_node(core_id, now + std::time::Duration::from_millis(1200));
                self.set_interaction_focus(Some(core_id), 30_000, now);
            }
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
        self.begin_modal_keyboard_capture();
        cluster_mode_selection_banner(self, monitor.as_str());
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
        self.model
            .cluster_state
            .cluster_name_prompt
            .remove(monitor.as_str());
        if self
            .input
            .interaction_state
            .cluster_name_prompt_drag_monitor
            .as_deref()
            == Some(monitor.as_str())
        {
            self.input
                .interaction_state
                .cluster_name_prompt_drag_monitor = None;
        }
        if self
            .input
            .interaction_state
            .cluster_name_prompt_repeat
            .as_ref()
            .is_some_and(|repeat| repeat.monitor == monitor)
        {
            self.input.interaction_state.cluster_name_prompt_repeat = None;
        }
        self.ui
            .render_state
            .clear_persistent_mode_banner(monitor.as_str());
        let focused_surface = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|&id| {
                self.model.field.node(id).is_some_and(|node| {
                    self.model.field.is_visible(id)
                        && node.kind == halley_core::field::NodeKind::Surface
                })
            })
            .or_else(|| self.last_input_surface_node_for_monitor(monitor.as_str()));
        self.schedule_modal_focus_restore(focused_surface, Instant::now());
        true
    }

    pub fn toggle_cluster_mode_selection(&mut self, node_id: NodeId) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.cluster_mutation_controller()
            .toggle_cluster_mode_selection(monitor.as_str(), node_id)
    }

    pub(super) fn order_cluster_creation_members(&self, members: Vec<NodeId>) -> Vec<NodeId> {
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
        self.open_cluster_name_prompt(now)
    }
}
