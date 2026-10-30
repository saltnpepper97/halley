//! Workspace activation and layout cycling.

use super::*;

impl ClusterSystem {
    pub fn cycle_active_layout(
        &mut self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let Some(id) = self.active_on(output) else {
            return false;
        };
        let Some(before) = self.workspace_layout(id, work_area) else {
            return false;
        };
        let Some(metadata) = self.metadata.get_mut(&id) else {
            return false;
        };
        metadata.layout = match metadata.layout {
            ClusterWorkspaceLayoutKind::Tiling => ClusterWorkspaceLayoutKind::Stacking,
            ClusterWorkspaceLayoutKind::Stacking => ClusterWorkspaceLayoutKind::Tiling,
        };
        let layout = metadata.layout;
        let duration_ms = match layout {
            ClusterWorkspaceLayoutKind::Tiling => self.animations.tiling.reflow_duration_ms,
            ClusterWorkspaceLayoutKind::Stacking => self.animations.stacking.cycle_duration_ms,
        };
        match layout {
            ClusterWorkspaceLayoutKind::Tiling => {
                self.overflow.reveal(output, now);
            }
            ClusterWorkspaceLayoutKind::Stacking => {
                self.hide_overflow(output);
            }
        }
        self.begin_layout_reflow(output, id, before, layout, now, duration_ms);
        true
    }

    pub fn cycle_stack(
        &mut self,
        output: &str,
        direction: halley_config::FocusCycleDirection,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> StackCycleOutcome {
        let Some(id) = self.active_on(output) else {
            return StackCycleOutcome::NotActive;
        };
        if self.metadata(id).map(|metadata| metadata.layout)
            != Some(ClusterWorkspaceLayoutKind::Stacking)
        {
            return StackCycleOutcome::NotActive;
        }
        let Some(mut members) = self
            .registry
            .cluster(id)
            .map(|cluster| cluster.members().to_vec())
        else {
            return StackCycleOutcome::Unchanged;
        };
        let layout_indices = members
            .iter()
            .enumerate()
            .filter_map(|(index, member)| {
                (!self.member_floats.is_floating(*member)).then_some(index)
            })
            .collect::<Vec<_>>();
        if layout_indices.len() < 2 {
            return StackCycleOutcome::Unchanged;
        }
        let Some(before) = self.workspace_layout(id, work_area) else {
            return StackCycleOutcome::Unchanged;
        };
        let direction = match direction {
            halley_config::FocusCycleDirection::Forward => {
                halley_core::cluster::layout::ClusterCycleDirection::Next
            }
            halley_config::FocusCycleDirection::Backward => {
                halley_core::cluster::layout::ClusterCycleDirection::Prev
            }
        };
        let mut layout_members = layout_indices
            .iter()
            .map(|index| members[*index])
            .collect::<Vec<_>>();
        let Some(member) =
            halley_core::cluster::stacking::cycle_stacking_members(&mut layout_members, direction)
        else {
            return StackCycleOutcome::Unchanged;
        };
        for (index, member) in layout_indices.into_iter().zip(layout_members) {
            members[index] = member;
        }
        if self.registry.reorder_cluster_members(id, members).is_err() {
            return StackCycleOutcome::Unchanged;
        }
        let Some(after) = self.workspace_layout(id, work_area) else {
            return StackCycleOutcome::Unchanged;
        };
        self.begin_stack_cycle_reflow(output, id, before, after, direction, now);
        StackCycleOutcome::Cycled(member)
    }

    pub fn activate_slot(&mut self, output: &str, slot: u8, now: Duration) -> bool {
        let Some(id) = self
            .slots
            .get(output)
            .and_then(|slots| slots.get(usize::from(slot.saturating_sub(1))))
            .copied()
        else {
            return false;
        };
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        if self.active_on(output) == Some(id) {
            self.hide_overflow(output);
            self.active.remove(output);
            self.registry.deactivate_cluster_workspace(id);
            self.begin_transition(output, id, transition::TransitionKind::Closing, now);
        } else {
            self.hide_overflow(output);
            if let Some(previous) = self.active.insert(output.to_string(), id) {
                self.registry.deactivate_cluster_workspace(previous);
            }
            self.registry.activate_cluster_workspace(id);
            self.overflow.reveal(output, now);
            self.begin_transition(output, id, transition::TransitionKind::Opening, now);
        }
        true
    }

    pub fn activate(&mut self, output: &str, id: ClusterId, now: Duration) -> bool {
        if self
            .metadata
            .get(&id)
            .is_none_or(|metadata| metadata.output != output)
        {
            return false;
        }
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        if self.active_on(output) == Some(id) {
            self.hide_overflow(output);
            self.active.remove(output);
            self.registry.deactivate_cluster_workspace(id);
            self.begin_transition(output, id, transition::TransitionKind::Closing, now);
        } else {
            self.hide_overflow(output);
            if let Some(previous) = self.active.insert(output.to_string(), id) {
                self.registry.deactivate_cluster_workspace(previous);
            }
            self.registry.activate_cluster_workspace(id);
            self.overflow.reveal(output, now);
            self.begin_transition(output, id, transition::TransitionKind::Opening, now);
        }
        true
    }

    /// Activates `id` on `output` when it is not already the active cluster.
    ///
    /// This is the idempotent counterpart of [`ClusterSystem::activate`], which
    /// toggles an already-active workspace closed. External controllers (the
    /// `ext-workspace-v1` global, scripts, and future automation) ask for a
    /// workspace to *be* active, so repeating the request must not close it,
    /// restart its transition, or move focus again.
    ///
    /// Returns whether the activation state changed. A missing cluster or a
    /// cluster owned by another output is rejected without mutation.
    pub fn activate_only(&mut self, output: &str, id: ClusterId, now: Duration) -> bool {
        if self
            .metadata
            .get(&id)
            .is_none_or(|metadata| metadata.output != output)
        {
            return false;
        }
        if self.active_on(output) == Some(id) {
            return false;
        }
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        self.hide_overflow(output);
        if let Some(previous) = self.active.insert(output.to_string(), id) {
            self.registry.deactivate_cluster_workspace(previous);
        }
        self.registry.activate_cluster_workspace(id);
        self.overflow.reveal(output, now);
        self.begin_transition(output, id, transition::TransitionKind::Opening, now);
        true
    }

    /// Deactivates `output` only when `id` is the cluster currently active
    /// there.
    ///
    /// Returns whether the activation state changed. Deactivating an inactive
    /// cluster, or a cluster owned by a different output, leaves every other
    /// clique's active selection untouched instead of clearing it.
    pub fn deactivate_only(&mut self, output: &str, id: ClusterId, now: Duration) -> bool {
        if self.active_on(output) != Some(id) {
            return false;
        }
        if self
            .metadata
            .get(&id)
            .is_none_or(|metadata| metadata.output != output)
        {
            return false;
        }
        self.close_bloom(output, now);
        self.set_hovered_core(None, now);
        self.hide_overflow(output);
        self.active.remove(output);
        self.registry.deactivate_cluster_workspace(id);
        self.begin_transition(output, id, transition::TransitionKind::Closing, now);
        true
    }

    /// Deactivates whatever cluster is currently active on `output`, leaving
    /// the output in "Field" (no workspace active) mode.
    ///
    /// Returns whether the activation state changed.
    pub fn deactivate_output(&mut self, output: &str, now: Duration) -> bool {
        let Some(id) = self.active_on(output) else {
            return false;
        };
        self.deactivate_only(output, id, now)
    }
}
