use super::*;
use crate::compositor::clusters::state::PendingClusterSlotTransition;

pub(crate) fn activate_cluster_slot_on_current_monitor(
    st: &mut Halley,
    slot: u8,
    now: Instant,
) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    st.model
        .cluster_state
        .pending_cluster_slot_transition
        .remove(monitor.as_str());
    let Some(cid) = cluster_slot_cluster_for_monitor(st, monitor.as_str(), slot) else {
        return false;
    };

    if active_cluster_workspace_for_monitor(st, monitor.as_str()) == Some(cid) {
        return exit_cluster_workspace_for_monitor(st, monitor.as_str(), now);
    }

    if active_cluster_workspace_for_monitor(st, monitor.as_str()).is_some() {
        let _ = exit_cluster_workspace_for_monitor(st, monitor.as_str(), now);
    }

    let Some(core_id) = st.model.field.cluster(cid).and_then(|cluster| cluster.core) else {
        return false;
    };
    let Some(target_center) = st.model.field.node(core_id).map(|node| node.pos) else {
        return false;
    };

    if st.animate_viewport_center_to(target_center, now) {
        st.model
            .cluster_state
            .pending_cluster_slot_transition
            .insert(monitor, PendingClusterSlotTransition { slot, cid, core_id });
        st.request_maintenance();
        true
    } else {
        enter_cluster_workspace_by_core(st, core_id, monitor.as_str(), now)
    }
}

pub(crate) fn process_pending_cluster_slot_transition_for_current_monitor(
    st: &mut Halley,
    now: Instant,
) -> bool {
    if st
        .input
        .interaction_state
        .viewport_pan_anim
        .as_ref()
        .is_some_and(|anim| anim.is_focus_pan())
    {
        return false;
    }
    let monitor = st.model.monitor_state.current_monitor.clone();
    let Some(pending) = st
        .model
        .cluster_state
        .pending_cluster_slot_transition
        .remove(monitor.as_str())
    else {
        return false;
    };
    if cluster_slot_cluster_for_monitor(st, monitor.as_str(), pending.slot) != Some(pending.cid) {
        return false;
    }
    enter_cluster_workspace_by_core(st, pending.core_id, monitor.as_str(), now)
}

pub(crate) fn visible_tiled_cluster_tiles_for_monitor(
    st: &Halley,
    monitor: &str,
) -> Option<(ClusterId, Vec<ClusterTilePlacement>)> {
    let cid = active_cluster_workspace_for_monitor(st, monitor)?;
    let plan = crate::compositor::clusters::read::plan_active_cluster_layout(st, monitor)?;
    if !matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
        return None;
    }

    let visible_tiles = plan
        .tiles
        .into_iter()
        .filter(|tile| {
            !st.model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .contains_key(&tile.node_id)
        })
        .collect::<Vec<_>>();
    Some((cid, visible_tiles))
}

/// Whether `node_id` is currently a visible tile in the monitor's active *tiled*
/// cluster workspace. Used by the pointer-drop path to let the cluster layout own
/// the dropped window's position instead of the free-window static lock.
pub(crate) fn node_is_active_tiled_cluster_member(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
) -> bool {
    visible_tiled_cluster_tiles_for_monitor(st, monitor)
        .is_some_and(|(_, tiles)| tiles.iter().any(|tile| tile.node_id == node_id))
}

pub(crate) fn directional_target_member(
    st: &Halley,
    visible_tiles: &[ClusterTilePlacement],
    direction: DirectionalAction,
) -> Option<(NodeId, NodeId)> {
    if visible_tiles.is_empty() {
        return None;
    }

    let current_member = st
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
    st: &mut Halley,
    monitor: &str,
    member: NodeId,
    world_pos: Vec2,
    now_ms: u64,
) -> bool {
    let Some((cid, visible_tiles)) = visible_tiled_cluster_tiles_for_monitor(st, monitor) else {
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

    let Some(members) = st
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
    if st
        .model
        .field
        .reorder_cluster_members(cid, reordered)
        .is_err()
    {
        return false;
    }
    layout_active_cluster_workspace_for_monitor(st, monitor, now_ms);
    true
}

pub(crate) fn focus_active_tiled_cluster_member_for_monitor(
    st: &mut Halley,
    monitor: &str,
    preferred_index: Option<usize>,
    now: Instant,
) -> bool {
    let Some((_, visible_tiles)) = visible_tiled_cluster_tiles_for_monitor(st, monitor) else {
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
    let now_ms = st.now_ms(now);
    st.set_interaction_focus(Some(target), 30_000, now);
    st.update_focus_tracking_for_surface(target, now_ms);
    true
}

pub(crate) fn tile_focus_active_cluster_member_for_monitor(
    st: &mut Halley,
    monitor: &str,
    direction: DirectionalAction,
    now: Instant,
) -> bool {
    let Some((_, visible_tiles)) = visible_tiled_cluster_tiles_for_monitor(st, monitor) else {
        return false;
    };
    let Some((_, target)) = directional_target_member(st, &visible_tiles, direction) else {
        return false;
    };
    let now_ms = st.now_ms(now);
    st.set_interaction_focus(Some(target), 30_000, now);
    st.update_focus_tracking_for_surface(target, now_ms);
    true
}

pub(crate) fn tile_swap_active_cluster_member_for_monitor(
    st: &mut Halley,
    monitor: &str,
    direction: DirectionalAction,
    now: Instant,
) -> bool {
    let Some((cid, visible_tiles)) = visible_tiled_cluster_tiles_for_monitor(st, monitor) else {
        return false;
    };
    if visible_tiles.len() < 2 {
        return false;
    }
    let Some((current_member, target_member)) =
        directional_target_member(st, &visible_tiles, direction)
    else {
        return false;
    };

    let Some(mut members) = st
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
    if st
        .model
        .field
        .reorder_cluster_members(cid, members)
        .is_err()
    {
        return false;
    }
    let now_ms = st.now_ms(now);
    layout_active_cluster_workspace_for_monitor(st, monitor, now_ms);
    st.set_interaction_focus(Some(current_member), 30_000, now);
    st.update_focus_tracking_for_surface(current_member, now_ms);
    true
}

pub(crate) fn cycle_active_stack_for_monitor(
    st: &mut Halley,
    monitor: &str,
    direction: ClusterCycleDirection,
    now: Instant,
) -> bool {
    if !matches!(
        active_cluster_layout_kind(st),
        ClusterWorkspaceLayoutKind::Stacking
    ) {
        return false;
    }
    let Some(cid) = active_cluster_workspace_for_monitor(st, monitor) else {
        return false;
    };
    let old_visible =
        crate::compositor::surface::active_stacking_visible_members_for_monitor(st, monitor);
    let Some(front) = st
        .model
        .field
        .cycle_cluster_stacking_members(cid, direction)
    else {
        return false;
    };
    if st.focused_monitor() != monitor {
        st.focus_monitor_view(monitor, now);
    }
    let now_ms = st.now_ms(now);
    layout_active_cluster_workspace_for_monitor(st, monitor, now_ms);
    let new_visible =
        crate::compositor::surface::active_stacking_visible_members_for_monitor(st, monitor);
    let duration_ms = st.runtime.tuning.stack_animation_duration_ms();
    if st.runtime.tuning.stack_animation_enabled() {
        for node_id in old_visible.iter().chain(new_visible.iter()).copied() {
            st.request_window_animation_prewarm(node_id, now);
        }
        st.ui.render_state.start_stack_cycle_transition(
            monitor,
            direction,
            old_visible,
            new_visible,
            now,
            duration_ms,
        );
        st.request_maintenance();
    }
    st.set_recent_top_node(front, now + std::time::Duration::from_millis(1200));
    st.set_interaction_focus(Some(front), 30_000, now);
    st.update_focus_tracking_for_surface(front, now_ms);
    true
}

pub(crate) fn cycle_active_cluster_layout_for_monitor(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
) -> bool {
    let Some(cid) = active_cluster_workspace_for_monitor(st, monitor) else {
        return false;
    };
    let current_focus = st
        .model
        .focus_state
        .primary_interaction_focus
        .filter(|id| st.model.field.cluster_id_for_member_public(*id) == Some(cid));
    let previous_layout_kind = active_cluster_layout_kind(st);
    let tile_to_stack_transition =
        if matches!(previous_layout_kind, ClusterWorkspaceLayoutKind::Tiling) {
            crate::compositor::clusters::read::plan_active_cluster_layout(st, monitor).and_then(
                |plan| {
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
                },
            )
        } else {
            None
        };

    st.runtime.tuning.cluster_default_layout = match st.runtime.tuning.cluster_default_layout {
        ClusterDefaultLayout::Tiling => ClusterDefaultLayout::Stacking,
        ClusterDefaultLayout::Stacking => ClusterDefaultLayout::Tiling,
    };

    let is_tiling = matches!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Tiling
    );

    if is_tiling {
        let members = st
            .model
            .field
            .cluster(cid)
            .map(|c| c.members().to_vec())
            .unwrap_or_default();
        for member in members {
            let should_float = st
                .model
                .spawn_state
                .applied_window_rules
                .get(&member)
                .is_some_and(|rule| {
                    rule.cluster_participation
                        == halley_config::InitialWindowClusterParticipation::Float
                        || rule.overlap_policy != halley_config::InitialWindowOverlapPolicy::None
                });
            if should_float {
                let pos = st
                    .model
                    .field
                    .node(member)
                    .map(|n| n.pos)
                    .unwrap_or(halley_core::field::Vec2 { x: 0.0, y: 0.0 });
                let _ = detach_member_from_cluster(st, cid, member, pos, now);
            }
        }
    } else {
        let monitor_nodes: Vec<_> = st
            .model
            .monitor_state
            .node_monitor
            .iter()
            .filter_map(|(&id, m)| if m == monitor { Some(id) } else { None })
            .collect();
        for node in monitor_nodes {
            if st.model.field.cluster_id_for_member_public(node).is_none() {
                let should_be_in_cluster = st
                    .model
                    .spawn_state
                    .applied_window_rules
                    .get(&node)
                    .is_some_and(|rule| {
                        rule.cluster_participation
                            == halley_config::InitialWindowClusterParticipation::Float
                            || rule.overlap_policy
                                != halley_config::InitialWindowOverlapPolicy::None
                    });
                if should_be_in_cluster {
                    let _ = absorb_node_into_cluster(st, cid, node, now);
                }
            }
        }
    }

    let now_ms = st.now_ms(now);
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
    layout_active_cluster_workspace_for_monitor(st, monitor, now_ms);

    match st.runtime.tuning.cluster_layout_kind() {
        ClusterWorkspaceLayoutKind::Tiling => {
            let preferred_index = current_focus.and_then(|id| {
                st.model
                    .field
                    .cluster(cid)
                    .and_then(|cluster| cluster.members().iter().position(|member| *member == id))
            });
            let _ =
                focus_active_tiled_cluster_member_for_monitor(st, monitor, preferred_index, now);
        }
        ClusterWorkspaceLayoutKind::Stacking => {
            let visible = crate::compositor::surface::active_stacking_visible_members_for_monitor(
                st, monitor,
            );
            if let Some((old_visible, source_rects)) = tile_to_stack_transition {
                for node_id in old_visible.iter().chain(visible.iter()).copied() {
                    st.request_window_animation_prewarm(node_id, now);
                }
                st.ui.render_state.start_stack_cycle_transition_from_rects(
                    monitor,
                    ClusterCycleDirection::Prev,
                    old_visible,
                    visible.clone(),
                    source_rects,
                    now,
                    240,
                );
                st.request_maintenance();
            }
            if let Some(target) =
                crate::compositor::surface::active_stacking_front_member_for_monitor(st, monitor)
                    .or_else(|| visible.first().copied())
            {
                st.set_recent_top_node(target, now + std::time::Duration::from_millis(1200));
                st.set_interaction_focus(Some(target), 30_000, now);
                st.update_focus_tracking_for_surface(target, now_ms);
            }
        }
    }
    refresh_cluster_overflow_for_monitor(st, monitor, now_ms, true);
    true
}
