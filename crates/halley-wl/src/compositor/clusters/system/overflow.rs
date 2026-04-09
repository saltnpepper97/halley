use super::*;

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
    const CLUSTER_OVERFLOW_REVEAL_MS: u64 = 2200;

    pub(crate) fn adjust_cluster_overflow_scroll_for_monitor(
        &mut self,
        monitor: &str,
        delta: i32,
    ) -> bool {
        let overflow_len = self
            .model
            .cluster_state
            .cluster_overflow_members
            .get(monitor)
            .map(Vec::len)
            .unwrap_or(0);
        let max_offset = overflow_len.saturating_sub(Self::CLUSTER_OVERFLOW_VISIBLE_SLOTS);
        if max_offset == 0 {
            self.model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .remove(monitor);
            return false;
        }
        let current = self
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
        self.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .insert(monitor.to_string(), next);
        true
    }

    pub(super) fn clear_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        self.model
            .cluster_state
            .cluster_overflow_members
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_rects
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_reveal_started_at_ms
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .remove(monitor);
    }

    pub(super) fn refresh_cluster_overflow_for_monitor(
        &mut self,
        monitor: &str,
        now_ms: u64,
        reveal: bool,
    ) {
        let Some(_cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            self.clear_cluster_overflow_for_monitor(monitor);
            return;
        };
        let Some(plan) = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)
        else {
            self.clear_cluster_overflow_for_monitor(monitor);
            return;
        };
        let overflow = plan.overflow_members;
        if overflow.is_empty() {
            self.clear_cluster_overflow_for_monitor(monitor);
            return;
        }

        let was_visible = self
            .model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .get(monitor)
            .is_some_and(|visible_until_ms| *visible_until_ms > now_ms);

        self.model
            .cluster_state
            .cluster_overflow_members
            .insert(monitor.to_string(), overflow.clone());
        let max_offset = overflow
            .len()
            .saturating_sub(Self::CLUSTER_OVERFLOW_VISIBLE_SLOTS);
        if max_offset == 0 {
            self.model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .remove(monitor);
        } else {
            let next = self
                .model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .get(monitor)
                .copied()
                .unwrap_or(0)
                .min(max_offset);
            self.model
                .cluster_state
                .cluster_overflow_scroll_offsets
                .insert(monitor.to_string(), next);
        }
        if let Some(rect) = self
            .cluster_read_controller()
            .overflow_strip_rect_for_monitor(monitor, overflow.len())
        {
            self.model
                .cluster_state
                .cluster_overflow_rects
                .insert(monitor.to_string(), rect);
        }
        if reveal {
            if !was_visible {
                self.model
                    .cluster_state
                    .cluster_overflow_reveal_started_at_ms
                    .insert(monitor.to_string(), now_ms);
            }
            self.model
                .cluster_state
                .cluster_overflow_visible_until_ms
                .insert(
                    monitor.to_string(),
                    now_ms.saturating_add(Self::CLUSTER_OVERFLOW_REVEAL_MS),
                );
            self.request_maintenance();
        }
    }

    pub(crate) fn reveal_cluster_overflow_for_monitor(&mut self, monitor: &str, now_ms: u64) {
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, true);
    }

    pub(crate) fn hide_cluster_overflow_for_monitor(&mut self, monitor: &str) {
        self.model
            .cluster_state
            .cluster_overflow_scroll_offsets
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_reveal_started_at_ms
            .remove(monitor);
        self.model
            .cluster_state
            .cluster_overflow_visible_until_ms
            .remove(monitor);
    }

    pub(crate) fn swap_cluster_overflow_member_with_visible(
        &mut self,
        monitor: &str,
        cid: ClusterId,
        overflow_member: NodeId,
        visible_member: NodeId,
        now_ms: u64,
    ) -> bool {
        if self.active_cluster_workspace_for_monitor(monitor) != Some(cid) {
            return false;
        }
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            return false;
        }
        let max_stack = self.runtime.tuning.tile_max_stack;
        if !self.model.field.swap_cluster_overflow_member_with_visible(
            cid,
            overflow_member,
            visible_member,
            max_stack,
        ) {
            return false;
        }
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        self.reveal_cluster_overflow_for_monitor(monitor, now_ms);
        true
    }

    pub(crate) fn reorder_cluster_overflow_member(
        &mut self,
        monitor: &str,
        cid: ClusterId,
        member: NodeId,
        target_overflow_index: usize,
        now_ms: u64,
    ) -> bool {
        if self.active_cluster_workspace_for_monitor(monitor) != Some(cid) {
            return false;
        }
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        ) {
            return false;
        }
        let max_stack = self.runtime.tuning.tile_max_stack;
        if !self.model.field.reorder_cluster_overflow_member(
            cid,
            member,
            target_overflow_index,
            max_stack,
        ) {
            return false;
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, true);
        true
    }
}
