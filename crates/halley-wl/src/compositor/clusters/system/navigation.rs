use super::*;

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
    fn visible_tiled_cluster_tiles_for_monitor(
        &self,
        monitor: &str,
    ) -> Option<(ClusterId, Vec<ClusterTilePlacement>)> {
        let cid = self.active_cluster_workspace_for_monitor(monitor)?;
        let plan = self
            .cluster_read_controller()
            .plan_active_cluster_layout(monitor)?;
        if !matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
            return None;
        }

        let visible_tiles = plan
            .tiles
            .into_iter()
            .filter(|tile| {
                !self
                    .model
                    .spawn_state
                    .pending_tiled_insert_reveal_at_ms
                    .contains_key(&tile.node_id)
            })
            .collect::<Vec<_>>();
        Some((cid, visible_tiles))
    }

    fn directional_target_member(
        &self,
        visible_tiles: &[ClusterTilePlacement],
        direction: DirectionalAction,
    ) -> Option<(NodeId, NodeId)> {
        if visible_tiles.is_empty() {
            return None;
        }

        let current_member = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|id| visible_tiles.iter().any(|tile| tile.node_id == *id))
            .unwrap_or(visible_tiles[0].node_id);
        let current_rect = visible_tiles
            .iter()
            .find(|tile| tile.node_id == current_member)
            .map(|tile| tile.rect)?;
        let target = visible_tiles
            .iter()
            .filter(|tile| tile.node_id != current_member)
            .filter_map(|tile| {
                directional_candidate_score(current_rect, tile.rect, direction)
                    .map(|score| (score, tile.node_id))
            })
            .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, node_id)| node_id)?;
        Some((current_member, target))
    }

    pub(crate) fn move_active_cluster_member_to_drop_tile(
        &mut self,
        monitor: &str,
        member: NodeId,
        world_pos: Vec2,
        now_ms: u64,
    ) -> bool {
        let Some((cid, visible_tiles)) = self.visible_tiled_cluster_tiles_for_monitor(monitor)
        else {
            return false;
        };
        if !visible_tiles.iter().any(|tile| tile.node_id == member) {
            return false;
        }

        let Some(target_member) = visible_tiles
            .iter()
            .find(|tile| {
                world_pos.x >= tile.rect.x
                    && world_pos.x <= tile.rect.x + tile.rect.w
                    && world_pos.y >= tile.rect.y
                    && world_pos.y <= tile.rect.y + tile.rect.h
            })
            .map(|tile| tile.node_id)
        else {
            return false;
        };
        if target_member == member {
            return false;
        }

        let Some(members) = self
            .model
            .field
            .cluster(cid)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
        let Some(from_index) = members.iter().position(|&id| id == member) else {
            return false;
        };
        let Some(target_index) = members.iter().position(|&id| id == target_member) else {
            return false;
        };

        let mut reordered = members;
        let moved = reordered.remove(from_index);
        reordered.insert(target_index.min(reordered.len()), moved);
        if self
            .model
            .field
            .reorder_cluster_members(cid, reordered)
            .is_err()
        {
            return false;
        }
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        true
    }

    pub(crate) fn focus_active_tiled_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        preferred_index: Option<usize>,
        now: Instant,
    ) -> bool {
        let Some((_, visible_tiles)) = self.visible_tiled_cluster_tiles_for_monitor(monitor) else {
            return false;
        };
        let visible_members = visible_tiles
            .into_iter()
            .map(|tile| tile.node_id)
            .collect::<Vec<_>>();
        let Some(target) = visible_members
            .get(
                preferred_index
                    .unwrap_or(0)
                    .min(visible_members.len().saturating_sub(1)),
            )
            .copied()
        else {
            return false;
        };
        let now_ms = self.now_ms(now);
        self.set_interaction_focus(Some(target), 30_000, now);
        self.update_focus_tracking_for_surface(target, now_ms);
        true
    }

    pub(crate) fn tile_focus_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: DirectionalAction,
        now: Instant,
    ) -> bool {
        let Some((_, visible_tiles)) = self.visible_tiled_cluster_tiles_for_monitor(monitor) else {
            return false;
        };
        let Some((_, target)) = self.directional_target_member(&visible_tiles, direction) else {
            return false;
        };
        let now_ms = self.now_ms(now);
        self.set_interaction_focus(Some(target), 30_000, now);
        self.update_focus_tracking_for_surface(target, now_ms);
        true
    }

    pub(crate) fn tile_swap_active_cluster_member_for_monitor(
        &mut self,
        monitor: &str,
        direction: DirectionalAction,
        now: Instant,
    ) -> bool {
        let Some((cid, visible_tiles)) = self.visible_tiled_cluster_tiles_for_monitor(monitor)
        else {
            return false;
        };
        if visible_tiles.len() < 2 {
            return false;
        }
        let Some((current_member, target_member)) =
            self.directional_target_member(&visible_tiles, direction)
        else {
            return false;
        };

        let Some(mut members) = self
            .model
            .field
            .cluster(cid)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
        let Some(current_index) = members.iter().position(|id| *id == current_member) else {
            return false;
        };
        let Some(target_index) = members.iter().position(|id| *id == target_member) else {
            return false;
        };
        members.swap(current_index, target_index);
        if self
            .model
            .field
            .reorder_cluster_members(cid, members)
            .is_err()
        {
            return false;
        }
        let now_ms = self.now_ms(now);
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        self.set_interaction_focus(Some(current_member), 30_000, now);
        self.update_focus_tracking_for_surface(current_member, now_ms);
        true
    }

    pub(crate) fn cycle_active_stack_for_monitor(
        &mut self,
        monitor: &str,
        direction: ClusterCycleDirection,
        now: Instant,
    ) -> bool {
        if !matches!(
            self.active_cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) {
            return false;
        }
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return false;
        };
        let old_visible =
            crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                self, monitor,
            );
        let Some(front) = self
            .model
            .field
            .cycle_cluster_stacking_members(cid, direction)
        else {
            return false;
        };
        if self.focused_monitor() != monitor {
            self.focus_monitor_view(monitor, now);
        }
        let now_ms = self.now_ms(now);
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);
        let new_visible =
            crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                self, monitor,
            );
        self.ui.render_state.start_stack_cycle_transition(
            monitor,
            direction,
            old_visible,
            new_visible,
            now,
            220,
        );
        self.request_maintenance();
        self.set_recent_top_node(front, now + std::time::Duration::from_millis(1200));
        self.set_interaction_focus(Some(front), 30_000, now);
        self.update_focus_tracking_for_surface(front, now_ms);
        true
    }

    pub(crate) fn cycle_active_cluster_layout_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> bool {
        let Some(cid) = self.active_cluster_workspace_for_monitor(monitor) else {
            return false;
        };
        let current_focus = self
            .model
            .focus_state
            .primary_interaction_focus
            .filter(|id| self.model.field.cluster_id_for_member_public(*id) == Some(cid));
        let previous_layout_kind = self.active_cluster_layout_kind();
        let tile_to_stack_transition =
            if matches!(previous_layout_kind, ClusterWorkspaceLayoutKind::Tiling) {
                self.cluster_read_controller()
                    .plan_active_cluster_layout(monitor)
                    .map(|plan| {
                        let source_rects = plan
                            .tiles
                            .iter()
                            .map(|tile| (tile.node_id, tile.rect))
                            .collect::<HashMap<_, _>>();
                        let old_visible = plan
                            .tiles
                            .into_iter()
                            .map(|tile| tile.node_id)
                            .collect::<Vec<_>>();
                        (!source_rects.is_empty()).then_some((old_visible, source_rects))
                    })
                    .flatten()
            } else {
                None
            };

        self.runtime.tuning.cluster_default_layout =
            match self.runtime.tuning.cluster_default_layout {
                ClusterDefaultLayout::Tiling => ClusterDefaultLayout::Stacking,
                ClusterDefaultLayout::Stacking => ClusterDefaultLayout::Tiling,
            };

        let now_ms = self.now_ms(now);
        self.layout_active_cluster_workspace_for_monitor(monitor, now_ms);

        match self.runtime.tuning.cluster_layout_kind() {
            ClusterWorkspaceLayoutKind::Tiling => {
                let preferred_index = current_focus.and_then(|id| {
                    self.model.field.cluster(cid).and_then(|cluster| {
                        cluster.members().iter().position(|member| *member == id)
                    })
                });
                let _ = self.focus_active_tiled_cluster_member_for_monitor(
                    monitor,
                    preferred_index,
                    now,
                );
            }
            ClusterWorkspaceLayoutKind::Stacking => {
                let visible =
                    crate::compositor::surface_ops::active_stacking_visible_members_for_monitor(
                        self, monitor,
                    );
                if let Some((old_visible, source_rects)) = tile_to_stack_transition {
                    self.ui
                        .render_state
                        .start_stack_cycle_transition_from_rects(
                            monitor,
                            ClusterCycleDirection::Prev,
                            old_visible,
                            visible.clone(),
                            source_rects,
                            now,
                            240,
                        );
                    self.request_maintenance();
                }
                if let Some(target) = current_focus
                    .filter(|id| visible.contains(id))
                    .or_else(|| visible.first().copied())
                {
                    self.set_recent_top_node(target, now + std::time::Duration::from_millis(1200));
                    self.set_interaction_focus(Some(target), 30_000, now);
                    self.update_focus_tracking_for_surface(target, now_ms);
                }
            }
        }
        self.refresh_cluster_overflow_for_monitor(monitor, now_ms, true);
        true
    }
}
