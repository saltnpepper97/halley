use super::surface::{surface_key, surface_tree_root};
use super::*;

pub(super) struct QueuedOverflowPromotion {
    pub(super) monitor: String,
    pub(super) promoted_member: NodeId,
    pub(super) source_strip_rect: halley_core::tiling::Rect,
    pub(super) source_icon_rect: halley_core::tiling::Rect,
}

pub(super) fn capture_queued_overflow_promotion(
    st: &Halley,
    id: NodeId,
) -> Option<QueuedOverflowPromotion> {
    let monitor = st.model.monitor_state.node_monitor.get(&id).cloned()?;
    if !matches!(
        st.runtime.tuning.cluster_layout_kind(),
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
    ) {
        return None;
    }
    let cid = st.model.field.cluster_id_for_member_public(id)?;
    if st.active_cluster_workspace_for_monitor(monitor.as_str()) != Some(cid) {
        return None;
    }
    let cluster = st.model.field.cluster(cid)?;
    let visible_before = cluster.visible_members(st.runtime.tuning.tile_max_stack);
    if !visible_before.contains(&id) {
        return None;
    }
    let overflow_before = cluster.overflow_members(st.runtime.tuning.tile_max_stack);
    let promoted_member = overflow_before.first().copied()?;
    let source_strip_rect = st.cluster_overflow_rect_for_monitor(monitor.as_str())?;
    let source_icon_rect =
        st.cluster_overflow_slot_rect_for_monitor(monitor.as_str(), overflow_before.len(), 0)?;
    Some(QueuedOverflowPromotion {
        monitor,
        promoted_member,
        source_strip_rect,
        source_icon_rect,
    })
}

pub(super) fn arm_queued_overflow_promotion(
    st: &mut Halley,
    promotion: QueuedOverflowPromotion,
    now_ms: u64,
) {
    let Some(target_rect) = st
        .active_cluster_tile_rect_for_member(promotion.monitor.as_str(), promotion.promoted_member)
    else {
        return;
    };
    let reveal_at_ms = now_ms.saturating_add(CLUSTER_OVERFLOW_PROMOTION_ANIM_MS);
    st.model
        .cluster_state
        .cluster_overflow_scroll_offsets
        .remove(promotion.monitor.as_str());
    st.reveal_cluster_overflow_for_monitor(promotion.monitor.as_str(), now_ms);
    st.model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .insert(promotion.promoted_member, reveal_at_ms);
    st.model
        .spawn_state
        .pending_tiled_insert_preserve_focus
        .insert(promotion.promoted_member);
    if let Some(node) = st.model.field.node_mut(promotion.promoted_member) {
        node.visibility.set(Visibility::DETACHED, true);
        node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
    }
    st.layout_active_cluster_workspace_for_monitor(promotion.monitor.as_str(), now_ms);
    let (screen_w, screen_h) = st
        .model
        .monitor_state
        .monitors
        .get(promotion.monitor.as_str())
        .map(|space| (space.width, space.height))
        .unwrap_or((1, 1));
    let previous_monitor = st.begin_temporary_render_monitor(promotion.monitor.as_str());
    let target_center_world = Vec2 {
        x: target_rect.x + target_rect.w * 0.5,
        y: target_rect.y + target_rect.h * 0.5,
    };
    let (target_sx, target_sy) = crate::presentation::world_to_screen(
        st,
        screen_w,
        screen_h,
        target_center_world.x,
        target_center_world.y,
    );
    st.end_temporary_render_monitor(previous_monitor);
    st.model
        .cluster_state
        .cluster_overflow_promotion_anim
        .insert(
            promotion.monitor,
            crate::compositor::clusters::state::ClusterOverflowPromotionAnim {
                member_id: promotion.promoted_member,
                started_at_ms: now_ms,
                reveal_at_ms,
                source_strip_rect: promotion.source_strip_rect,
                source_center: Vec2 {
                    x: promotion.source_icon_rect.x + promotion.source_icon_rect.w * 0.5,
                    y: promotion.source_icon_rect.y + promotion.source_icon_rect.h * 0.5,
                },
                target_center: Vec2 {
                    x: target_sx as f32,
                    y: target_sy as f32,
                },
            },
        );
    st.request_maintenance();
}

pub(super) fn reconcile_surface_bindings(st: &mut Halley) {
    const STALE_SURFACE_GRACE_MS: u64 = 1500;
    let now = Instant::now();

    let alive: HashSet<ObjectId> = st
        .platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .map(|t| t.wl_surface().id())
        .collect();

    let stale: Vec<ObjectId> = st
        .model
        .surface_to_node
        .keys()
        .filter(|k| !alive.contains(*k))
        .filter(|k| {
            let Some(activity) = st.runtime.surface_activity.get(*k) else {
                return true;
            };
            now.duration_since(activity.last_commit_at()).as_millis() as u64
                >= STALE_SURFACE_GRACE_MS
        })
        .cloned()
        .collect();

    for key in stale {
        st.runtime.surface_activity.remove(&key);
        if let Some(id) = st.model.surface_to_node.remove(&key) {
            let queued_promotion = capture_queued_overflow_promotion(st, id);
            let active_tiled_focus_restore = st
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .cloned()
                .and_then(|monitor| {
                    let was_primary_focus =
                        st.model.focus_state.primary_interaction_focus == Some(id);
                    let preferred_index = st
                        .model
                        .field
                        .cluster_id_for_member_public(id)
                        .and_then(|cid| st.model.field.cluster(cid))
                        .and_then(|cluster| {
                            cluster.members().iter().position(|member| *member == id)
                        })?;
                    (was_primary_focus
                        && matches!(
                            st.runtime.tuning.cluster_layout_kind(),
                            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
                        )
                        && st.active_cluster_workspace_for_monitor(monitor.as_str())
                            == st.model.field.cluster_id_for_member_public(id))
                    .then_some((monitor, preferred_index))
                });
            if st.model.focus_state.pan_restore_active_focus == Some(id) {
                st.model.focus_state.pan_restore_active_focus = None;
            }
            st.model.workspace_state.manual_collapsed_nodes.remove(&id);
            st.ui.render_state.cache.zoom_nominal_size.remove(&id);
            st.ui.render_state.cache.zoom_resize_fallback.remove(&id);
            st.ui
                .render_state
                .cache
                .zoom_resize_reject_streak
                .remove(&id);
            st.ui.render_state.cache.zoom_last_observed_size.remove(&id);
            st.ui
                .render_state
                .cache
                .zoom_resize_static_streak
                .remove(&id);
            st.model.node_app_ids.remove(&id);
            st.model.workspace_state.last_active_size.remove(&id);
            st.ui.render_state.cache.bbox_loc.remove(&id);
            st.ui.render_state.cache.window_geometry.remove(&id);
            st.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            st.model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .remove(&id);
            st.model
                .spawn_state
                .pending_tiled_insert_preserve_focus
                .remove(&id);
            crate::protocol::wayland::activation::clear_surface_activation_for_root(
                st,
                key.clone(),
            );
            st.model.spawn_state.applied_window_rules.remove(&id);
            st.model.spawn_state.pending_rule_rechecks.remove(&id);
            st.model.spawn_state.pending_initial_reveal.remove(&id);
            st.model
                .workspace_state
                .active_transition_until_ms
                .remove(&id);
            st.model
                .workspace_state
                .primary_promote_cooldown_until_ms
                .remove(&id);
            crate::compositor::workspace::state::abort_maximize_session_for_node(st, id);
            crate::compositor::workspace::state::clear_maximize_resume_for_node(st, id);
            st.model.focus_state.last_surface_focus_ms.remove(&id);
            st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
            st.model.carry_state.carry_zone_hint.remove(&id);
            st.model.carry_state.carry_zone_last_change_ms.remove(&id);
            st.model.carry_state.carry_zone_pending.remove(&id);
            st.model.carry_state.carry_zone_pending_since_ms.remove(&id);
            st.model.carry_state.carry_activation_anim_armed.remove(&id);
            st.model.carry_state.carry_state_hold.remove(&id);
            if st.input.interaction_state.resize_active == Some(id) {
                st.input.interaction_state.resize_active = None;
            }
            if st.input.interaction_state.resize_static_node == Some(id) {
                st.input.interaction_state.resize_static_node = None;
                st.input.interaction_state.resize_static_lock_pos = None;
                st.input.interaction_state.resize_static_until_ms = 0;
            }
            if st.model.focus_state.primary_interaction_focus == Some(id) {
                st.model.focus_state.primary_interaction_focus = None;
                st.model.focus_state.interaction_focus_until_ms = 0;
            }
            let stale_monitors: Vec<String> = st
                .model
                .focus_state
                .monitor_focus
                .iter()
                .filter_map(|(monitor, &focused)| (focused == id).then_some(monitor.clone()))
                .collect();

            for monitor in stale_monitors {
                st.model.focus_state.monitor_focus.remove(&monitor);
            }
            st.input.interaction_state.smoothed_render_pos.remove(&id);
            let now = Instant::now();
            let now_ms = st.now_ms(now);
            let _ = st.remove_node_from_field(id, now_ms);
            if let Some(promotion) = queued_promotion {
                arm_queued_overflow_promotion(st, promotion, now_ms);
            }
            if let Some((monitor, preferred_index)) = active_tiled_focus_restore {
                let now = Instant::now();
                st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), st.now_ms(now));
                let _ = st.focus_active_tiled_cluster_member_for_monitor(
                    monitor.as_str(),
                    Some(preferred_index),
                    now,
                );
            }
        }
    }

    st.runtime.surface_activity.retain(|k, _| alive.contains(k));
}

pub(super) fn drop_surface_impl(st: &mut Halley, surface: &WlSurface) {
    for output in st.model.monitor_state.outputs.values() {
        output.leave(surface);
    }
    let pointer_focused_surface = st
        .platform
        .seat
        .get_pointer()
        .and_then(|pointer| pointer.current_focus());
    if pointer_focused_surface
        .as_ref()
        .is_some_and(|focused| focused.id() == surface.id())
    {
        crate::compositor::interaction::pointer::clear_pointer_focus(st);
    }
    let key = surface_key(surface);
    st.runtime.surface_activity.remove(&key);
    crate::protocol::wayland::activation::clear_surface_activation(st, surface);
    if let Some(id) = st.model.surface_to_node.remove(&key) {
        let close_anim_duration_ms = st.runtime.tuning.window_close_duration_ms();
        let close_anim_style = st.runtime.tuning.window_close_style();
        let closing_monitor = st.model.monitor_state.node_monitor.get(&id).cloned();
        let closing_node_snapshot = st.model.field.node(id).and_then(|node| {
            matches!(node.state, halley_core::field::NodeState::Node)
                .then(|| (node.pos, node.label.clone(), node.state.clone()))
        });
        if st.runtime.tuning.window_close_animation_enabled()
            && let Some(monitor) = closing_monitor.as_deref()
        {
            if let Some((pos, label, state)) = closing_node_snapshot {
                st.ui.render_state.start_closing_node_animation(
                    id,
                    monitor,
                    Instant::now(),
                    close_anim_duration_ms,
                    pos,
                    label,
                    state,
                );
            } else if let Some((border_rects, offscreen_textures)) =
                crate::window::capture_closing_window_animation(st, monitor, id)
            {
                st.ui.render_state.start_closing_window_animation(
                    id,
                    monitor,
                    Instant::now(),
                    close_anim_duration_ms,
                    close_anim_style,
                    border_rects,
                    offscreen_textures,
                );
            }
        }
        let queued_promotion = capture_queued_overflow_promotion(st, id);
        let active_tiled_focus_restore = st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .and_then(|monitor| {
                let was_primary_focus = st.model.focus_state.primary_interaction_focus == Some(id);
                let preferred_index = st
                    .model
                    .field
                    .cluster_id_for_member_public(id)
                    .and_then(|cid| st.model.field.cluster(cid))
                    .and_then(|cluster| {
                        cluster.members().iter().position(|member| *member == id)
                    })?;
                (was_primary_focus
                    && matches!(
                        st.runtime.tuning.cluster_layout_kind(),
                        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
                    )
                    && st.active_cluster_workspace_for_monitor(monitor.as_str())
                        == st.model.field.cluster_id_for_member_public(id))
                .then_some((monitor, preferred_index))
            });
        st.drop_fullscreen_surface(id, Instant::now());
        if st.model.focus_state.pan_restore_active_focus == Some(id) {
            st.model.focus_state.pan_restore_active_focus = None;
        }
        st.ui.render_state.cache.zoom_nominal_size.remove(&id);
        st.ui.render_state.cache.zoom_resize_fallback.remove(&id);
        st.ui
            .render_state
            .cache
            .zoom_resize_reject_streak
            .remove(&id);
        st.ui.render_state.cache.zoom_last_observed_size.remove(&id);
        st.ui
            .render_state
            .cache
            .zoom_resize_static_streak
            .remove(&id);
        st.model.node_app_ids.remove(&id);
        for trail in st.model.focus_state.focus_trail.values_mut() {
            trail.forget_node(id);
        }
        st.model.workspace_state.last_active_size.remove(&id);
        st.ui.render_state.cache.bbox_loc.remove(&id);
        st.ui.render_state.cache.window_geometry.remove(&id);
        st.model
            .spawn_state
            .pending_spawn_activate_at_ms
            .remove(&id);
        st.model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .remove(&id);
        st.model
            .spawn_state
            .pending_tiled_insert_preserve_focus
            .remove(&id);
        st.model.spawn_state.applied_window_rules.remove(&id);
        st.model.spawn_state.pending_rule_rechecks.remove(&id);
        st.model.spawn_state.pending_initial_reveal.remove(&id);
        st.model
            .workspace_state
            .active_transition_until_ms
            .remove(&id);
        st.model
            .workspace_state
            .primary_promote_cooldown_until_ms
            .remove(&id);
        crate::compositor::workspace::state::clear_maximize_resume_for_node(st, id);
        st.model.focus_state.last_surface_focus_ms.remove(&id);
        st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
        st.model.monitor_state.node_monitor.remove(&id);
        st.model.carry_state.carry_zone_hint.remove(&id);
        st.model.carry_state.carry_zone_last_change_ms.remove(&id);
        st.model.carry_state.carry_zone_pending.remove(&id);
        st.model.carry_state.carry_zone_pending_since_ms.remove(&id);
        st.model.carry_state.carry_activation_anim_armed.remove(&id);
        if st.input.interaction_state.resize_active == Some(id) {
            st.input.interaction_state.resize_active = None;
        }
        if st.input.interaction_state.resize_static_node == Some(id) {
            st.input.interaction_state.resize_static_node = None;
            st.input.interaction_state.resize_static_lock_pos = None;
            st.input.interaction_state.resize_static_until_ms = 0;
        }
        if st.model.focus_state.primary_interaction_focus == Some(id) {
            st.model.focus_state.primary_interaction_focus = None;
            st.model.focus_state.interaction_focus_until_ms = 0;
        }
        st.model.focus_state.suppress_trail_record_once = false;
        st.input.interaction_state.smoothed_render_pos.remove(&id);
        st.ui.render_state.clear_window_offscreen_cache_for(id);
        let now = Instant::now();
        let now_ms = st.now_ms(now);
        let _ = st.remove_node_from_field(id, now_ms);
        if let Some(promotion) = queued_promotion {
            arm_queued_overflow_promotion(st, promotion, now_ms);
        }
        if let Some((monitor, preferred_index)) = active_tiled_focus_restore {
            let now = Instant::now();
            st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), st.now_ms(now));
            let _ = st.focus_active_tiled_cluster_member_for_monitor(
                monitor.as_str(),
                Some(preferred_index),
                now,
            );
        }
    }
    st.request_maintenance();
}
