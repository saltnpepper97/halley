use super::*;
use crate::compositor::surface::is_active_cluster_workspace_member;

pub(super) fn exit_monitor_fullscreen_for_new_toplevel(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
) {
    let _ = (st, monitor, now);
}

pub(super) fn exit_monitor_maximize_for_new_toplevel(st: &mut Halley, monitor: &str, now: Instant) {
    let _ = (st, monitor, now);
}

#[inline]
pub(super) fn should_exit_monitor_maximize_for_new_toplevel(intent: &InitialWindowIntent) -> bool {
    let _ = intent;
    false
}

pub(super) fn exit_monitor_fullscreen_for_overlap_intent(
    st: &mut Halley,
    monitor: &str,
    intent: &InitialWindowIntent,
    _now: Instant,
) {
    let _ = (st, monitor, intent);
}

pub(super) fn should_join_active_cluster_layout(
    active_cluster: bool,
    stack_mode_open: bool,
    intent: &InitialWindowIntent,
) -> bool {
    active_cluster
        && !stack_mode_open
        && intent.rule.cluster_participation
            == halley_config::InitialWindowClusterParticipation::Layout
}

#[inline]
pub(super) fn surface_key(surface: &WlSurface) -> ObjectId {
    surface.id()
}

pub(super) fn surface_tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
        root = parent;
    }
    root
}

fn compact_app_id_label(app_id: &str) -> Option<String> {
    let tail = app_id
        .rsplit(['.', '/'])
        .next()
        .unwrap_or(app_id)
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | ' '));
    if tail.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(tail.len());
    let mut upper_next = true;
    for ch in tail.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }

    Some(out.trim().to_string()).filter(|value| !value.is_empty())
}

fn surface_identity(surface: &WlSurface) -> (Option<String>, Option<String>) {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|data| {
                let guard = data.lock().expect("xdg toplevel surface data");
                (
                    guard.title.clone().filter(|value| !value.trim().is_empty()),
                    guard
                        .app_id
                        .clone()
                        .filter(|value| !value.trim().is_empty()),
                )
            })
            .unwrap_or((None, None))
    })
}

pub(super) fn refresh_node_identity_for_surface(
    st: &mut Halley,
    surface: &WlSurface,
    fallback_label: &str,
) {
    let root_surface = surface_tree_root(surface);
    let root_key = surface_key(&root_surface);
    let Some(node_id) = st.model.surface_to_node.get(&root_key).copied() else {
        return;
    };

    let (title, app_id) = surface_identity(&root_surface);
    let label = title
        .or_else(|| app_id.as_deref().and_then(compact_app_id_label))
        .unwrap_or_else(|| fallback_label.to_string());

    if let Some(node) = st.model.field.node_mut(node_id) {
        node.label = label;
    }

    match app_id {
        Some(app_id) => {
            st.model.node_app_ids.insert(node_id, app_id.clone());
            if let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id).cloned() {
                let _ = crate::compositor::clusters::system::maybe_add_node_to_lift_cluster_finalize_draft(&mut *st,
                        monitor.as_str(),
                        node_id,
                        app_id.as_str(),
                    );
            }
        }
        None => {
            st.model.node_app_ids.remove(&node_id);
        }
    }

    if crate::window::node_is_game_like(st, node_id) {
        st.model
            .spawn_state
            .pending_spawn_pan_queue
            .retain(|pending| pending.node_id != node_id);
        if st
            .model
            .spawn_state
            .active_spawn_pan
            .is_some_and(|active| active.node_id == node_id)
        {
            st.model.spawn_state.active_spawn_pan = None;
            st.input.interaction_state.viewport_pan_anim = None;
        }
    }

    if crate::compositor::spawn::state::recompute_node_rule_opacity(st, node_id) {
        st.request_maintenance();
        st.runtime.tty_redraw_all = true;
    }

    let now = Instant::now();
    maybe_apply_pending_initial_window_rule(st, node_id, &root_surface, now);
    let _ = reveal_pending_initial_toplevel_if_ready(st, node_id, false, now);
}

fn maybe_apply_pending_initial_window_rule(
    st: &mut Halley,
    node_id: NodeId,
    root_surface: &WlSurface,
    now: Instant,
) {
    if !st
        .model
        .spawn_state
        .pending_rule_rechecks
        .contains(&node_id)
    {
        return;
    }
    let intent = crate::compositor::spawn::rules::resolve_initial_window_intent_for_surface(
        st,
        root_surface,
    );
    if !intent.matched_rule
        && crate::compositor::spawn::rules::needs_deferred_rule_recheck(st, &intent)
    {
        return;
    }
    let mut monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    if intent.matched_rule {
        let target_monitor = st.spawn_target_monitor_for_intent(&intent);
        if target_monitor != monitor {
            st.assign_node_to_monitor(node_id, target_monitor.as_str());
            monitor = target_monitor;
        }
    }
    let active_cluster = st.active_cluster_workspace_for_monitor(monitor.as_str());
    let mut cluster_local = st
        .model
        .field
        .cluster_id_for_member_public(node_id)
        .is_some_and(|cid| active_cluster == Some(cid));
    if st.cluster_bloom_for_monitor(monitor.as_str()).is_some() {
        st.model.spawn_state.pending_rule_rechecks.remove(&node_id);
        let _ = reveal_pending_initial_toplevel_if_ready(st, node_id, intent.is_transient, now);
        return;
    }

    if should_exit_monitor_maximize_for_new_toplevel(&intent) {
        exit_monitor_maximize_for_new_toplevel(st, monitor.as_str(), now);
    }

    exit_monitor_fullscreen_for_overlap_intent(st, monitor.as_str(), &intent, now);

    let is_stacking = matches!(
        st.runtime.tuning.cluster_layout_kind(),
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
    );

    // Game-like windows (steam_app_*, gamescope) still join the cluster tiling
    // like any other window, but get auto-fullscreened on top of it once their
    // app_id arrives — see the `enter_xdg_fullscreen` call after the absorb
    // block. This mirrors what most games request via xdg_toplevel
    // set_fullscreen anyway.
    let is_game_like = crate::window::node_is_game_like(st, node_id);

    let effective_float = !is_stacking
        && (intent.rule.cluster_participation
            == halley_config::InitialWindowClusterParticipation::Float
            || intent.rule.overlap_policy != halley_config::InitialWindowOverlapPolicy::None);

    if effective_float
        && cluster_local
        && let Some(cid) = st.model.field.cluster_id_for_member_public(node_id)
        && let Some(pos) = st.model.field.node(node_id).map(|node| node.pos)
    {
        let _ = st.detach_member_from_cluster(cid, node_id, pos, now);
        st.assign_node_to_monitor(node_id, monitor.as_str());
        cluster_local = false;
    }

    if !cluster_local
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
        )
        && should_join_active_cluster_layout(active_cluster.is_some(), false, &intent)
        && let Some(cid) = active_cluster
        && st.absorb_node_into_cluster(cid, node_id, now)
    {
        st.assign_node_to_monitor(node_id, monitor.as_str());
        let reveal_at_ms = st.now_ms(now).saturating_add(140);
        st.model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .insert(node_id, reveal_at_ms);
        st.request_window_animation_prewarm(node_id, now);
        if let Some(node) = st.model.field.node_mut(node_id) {
            node.visibility.set(Visibility::DETACHED, true);
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
        }
        st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), st.now_ms(now));
        st.request_maintenance();
        cluster_local = true;
    }

    // Auto-fullscreen game-like windows on top of whatever layout they joined
    // (cluster tiling or free-floating field). Placed after the absorb block so
    // a tiled game is already a cluster member here; the fullscreen system
    // handles hiding its siblings. `enter_xdg_fullscreen` is idempotent and
    // drives the xdg fullscreen protocol state so the client renders at
    // fullscreen size; the `is_none()` guard avoids re-entering if the client
    // already issued its own set_fullscreen request.
    if is_game_like && st.fullscreen_monitor_for_node(node_id).is_none() {
        prepare_game_like_node_for_auto_fullscreen(st, node_id, monitor.as_str(), now);
        crate::compositor::fullscreen::system::enter_xdg_fullscreen(st, node_id, None, now);
    }

    let rule_size = if !cluster_local {
        maybe_apply_deferred_rule_initial_size(st, node_id, &intent)
    } else {
        None
    };

    if should_repick_deferred_initial_window_position(&intent)
        && !cluster_local
        && let Some(size) =
            rule_size.or_else(|| st.model.field.node(node_id).map(|node| node.intrinsic_size))
    {
        let (picked_monitor, pos, _) = st.pick_spawn_position_with_intent(size, &intent);
        if intent.matched_rule && picked_monitor != monitor {
            st.assign_node_to_monitor(node_id, picked_monitor.as_str());
        }
        let _ = st.model.field.carry(node_id, pos);
        if let Some(record) = st.model.spawn_state.pending_initial_spawn_placement.take() {
            st.model
                .spawn_state
                .initial_spawn_placements
                .insert(node_id, record);
        }
    }

    st.set_recent_top_node(node_id, now + std::time::Duration::from_millis(1200));
    if intent.matched_rule {
        st.model
            .spawn_state
            .applied_window_rules
            .insert(node_id, intent.applied_rule_for_node());
    } else {
        st.model.spawn_state.applied_window_rules.remove(&node_id);
    }
    let _ = st.raise_overlap_policy_node(node_id);
    st.model.spawn_state.pending_rule_rechecks.remove(&node_id);
    if !st
        .model
        .spawn_state
        .pending_initial_reveal
        .contains(&node_id)
    {
        st.reveal_new_toplevel_node(node_id, intent.is_transient, now);
    } else {
        let _ = reveal_pending_initial_toplevel_if_ready(st, node_id, intent.is_transient, now);
    }
}

fn prepare_game_like_node_for_auto_fullscreen(
    st: &mut Halley,
    node_id: NodeId,
    monitor: &str,
    now: Instant,
) {
    let had_tiled_insert_reveal = st
        .model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .remove(&node_id)
        .is_some();
    st.model
        .spawn_state
        .pending_tiled_insert_preserve_focus
        .remove(&node_id);
    if let Some(node) = st.model.field.node_mut(node_id) {
        node.visibility.set(Visibility::DETACHED, false);
        node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
    }
    if had_tiled_insert_reveal
        && st
            .model
            .field
            .cluster_id_for_member_public(node_id)
            .is_some_and(|cid| st.active_cluster_workspace_for_monitor(monitor) == Some(cid))
    {
        st.layout_active_cluster_workspace_for_monitor(monitor, st.now_ms(now));
    }
}

fn maybe_apply_deferred_rule_initial_size(
    st: &mut Halley,
    node_id: NodeId,
    intent: &InitialWindowIntent,
) -> Option<Vec2> {
    let (width, height) = intent.rule.initial_size?;
    let size = Vec2 {
        x: width.max(96) as f32,
        y: height.max(72) as f32,
    };
    let size_changed = st.model.field.node(node_id).is_some_and(|node| {
        (node.intrinsic_size.x - size.x).abs() > 0.5 || (node.intrinsic_size.y - size.y).abs() > 0.5
    });
    if !size_changed {
        return Some(size);
    }
    if let Some(node) = st.model.field.node_mut(node_id) {
        node.intrinsic_size = size;
        if node.state == halley_core::field::NodeState::Active {
            node.footprint = size;
        }
    }
    st.model
        .workspace_state
        .last_active_size
        .insert(node_id, size);
    st.ui
        .render_state
        .cache
        .zoom_nominal_size
        .insert(node_id, size);
    st.request_toplevel_resize(node_id, width, height);
    Some(size)
}

pub(super) fn should_repick_deferred_initial_window_position(intent: &InitialWindowIntent) -> bool {
    intent.matched_rule
}

pub(super) fn reveal_pending_initial_toplevel_if_ready(
    st: &mut Halley,
    node_id: NodeId,
    is_transient: bool,
    now: Instant,
) -> bool {
    if !st
        .model
        .spawn_state
        .pending_initial_reveal
        .contains(&node_id)
        || st
            .model
            .spawn_state
            .pending_rule_rechecks
            .contains(&node_id)
        || pending_lift_cluster_build_should_hold_unidentified_node(st, node_id)
        || !st
            .ui
            .render_state
            .cache
            .window_geometry
            .contains_key(&node_id)
    {
        return false;
    }

    let finalized = st
        .model
        .field
        .node(node_id)
        .map(|node| node.intrinsic_size)
        .is_some_and(|size| st.finalize_initial_spawn_position(node_id, size));
    if !finalized && let Some(pos) = st.model.field.node(node_id).map(|node| node.pos) {
        let _ = st.carry_surface_non_overlap(node_id, pos, true);
    }
    st.model.spawn_state.pending_initial_reveal.remove(&node_id);
    st.reveal_new_toplevel_node(node_id, is_transient, now);
    st.resolve_landmarks_overlapped_by_active_window(node_id);
    if !st
        .model
        .field
        .node(node_id)
        .is_some_and(|node| node.visibility.has(Visibility::DETACHED))
        && !finalized
    {
        st.resolve_surface_overlap();
    }
    st.request_maintenance();
    true
}

fn pending_lift_cluster_build_should_hold_unidentified_node(st: &Halley, node_id: NodeId) -> bool {
    if st.model.node_app_ids.contains_key(&node_id) {
        return false;
    }
    let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id) else {
        return false;
    };
    crate::compositor::clusters::system::pending_lift_cluster_build_waits_for_candidate_identity(
        st,
        monitor.as_str(),
        node_id,
    )
}

pub(super) fn note_commit(st: &mut Halley, surface: &WlSurface, now: Instant) {
    let key = surface_key(surface);
    let root_surface = surface_tree_root(surface);
    let root_key = surface_key(&root_surface);
    st.runtime
        .surface_activity
        .entry(key.clone())
        .or_insert_with(|| CommitActivity::new(now))
        .on_commit(now);
    let target_monitor = if let Some(node_id) = st.model.surface_to_node.get(&root_key) {
        st.model.monitor_state.node_monitor.get(node_id).cloned()
    } else if crate::protocol::wayland::session_lock::is_session_lock_surface(st, &root_surface) {
        crate::protocol::wayland::session_lock::monitor_for_surface(st, &root_surface)
    } else {
        st.model
            .monitor_state
            .layer_surface_monitor
            .get(&root_key)
            .cloned()
    }
    .unwrap_or_else(|| st.model.monitor_state.focused_monitor.clone());
    st.request_tty_redraw_for_monitor(target_monitor.as_str());

    crate::compositor::monitor::state::assign_surface_to_monitor(
        st,
        &root_surface,
        target_monitor.as_str(),
    );

    crate::compositor::monitor::layer_shell::maybe_grant_layer_surface_focus_on_commit(
        &mut st.layer_shell_ctx(),
        surface,
    );
    crate::protocol::wayland::session_lock::maybe_focus_surface_on_commit(st, surface);

    if let Some(node_id) = st.model.surface_to_node.get(&root_key).copied() {
        st.ui.render_state.mark_window_offscreen_dirty(node_id);
        refresh_node_identity_for_surface(st, &root_surface, "Window");
        if st
            .model
            .fullscreen_state
            .fullscreen_restore
            .contains_key(&node_id)
            && !st.is_fullscreen_active(node_id)
        {
            return;
        }
        use smithay::desktop::utils::bbox_from_surface_tree;
        use smithay::wayland::shell::xdg::SurfaceCachedState;

        let bbox = bbox_from_surface_tree(&root_surface, (0, 0));
        st.ui
            .render_state
            .cache
            .bbox_loc
            .insert(node_id, (bbox.loc.x as f32, bbox.loc.y as f32));

        let geo = with_states(&root_surface, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        });
        let (window_geometry, new_size) = committed_window_geometry(
            (bbox.loc.x, bbox.loc.y),
            (bbox.size.w, bbox.size.h),
            geo.map(|g| (g.loc.x, g.loc.y, g.size.w, g.size.h)),
        );
        let first_geometry_commit = !st
            .ui
            .render_state
            .cache
            .window_geometry
            .contains_key(&node_id);
        st.ui
            .render_state
            .cache
            .window_geometry
            .insert(node_id, window_geometry);
        if is_active_cluster_workspace_member(st, node_id) {
            // If this member is mid-reflow (has a live tile track — e.g. a growing
            // tile held at its old slot waiting for exactly this bigger buffer), run
            // the cluster layout now so the commit can release it into its morph.
            if st
                .ui
                .render_state
                .cluster_tile_tracks()
                .contains_key(&node_id)
                && let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id).cloned()
            {
                st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), st.now_ms(now));
            } else {
                st.request_maintenance();
            }
            let _ = reveal_pending_initial_toplevel_if_ready(st, node_id, false, now);
            return;
        }
        let pending_initial_reveal = st
            .model
            .spawn_state
            .pending_initial_reveal
            .contains(&node_id);
        if !pending_initial_reveal
            && crate::compositor::workspace::state::node_in_maximize_session(st, node_id)
        {
            st.model
                .workspace_state
                .last_active_size
                .insert(node_id, new_size);
            st.request_maintenance();
            return;
        }
        if !pending_initial_reveal && st.is_fullscreen_active(node_id) {
            st.model
                .workspace_state
                .last_active_size
                .insert(node_id, new_size);
            st.request_maintenance();
            return;
        }
        let size_changed = st.model.field.node(node_id).is_some_and(|node| {
            (node.intrinsic_size.x - new_size.x).abs() > 0.5
                || (node.intrinsic_size.y - new_size.y).abs() > 0.5
        });
        if size_changed && st.input.interaction_state.resize_active != Some(node_id) {
            if let Some(node) = st.model.field.node_mut(node_id) {
                node.intrinsic_size = new_size;
                if node.state == halley_core::field::NodeState::Active {
                    node.footprint = new_size;
                }
            }
            st.model
                .workspace_state
                .last_active_size
                .insert(node_id, new_size);
            if !pending_initial_reveal
                && st
                    .model
                    .workspace_state
                    .active_transitions
                    .contains_key(&node_id)
            {
                st.resolve_landmarks_overlapped_by_active_window(node_id);
            }
            let finalized_initial_spawn =
                !pending_initial_reveal && st.finalize_initial_spawn_position(node_id, new_size);
            st.request_maintenance();
            if st.input.interaction_state.resize_static_node != Some(node_id) {
                let node_monitor = st.model.monitor_state.node_monitor.get(&node_id).cloned();
                let active_cluster = st
                    .model
                    .field
                    .cluster_id_for_member_public(node_id)
                    .zip(node_monitor.as_deref())
                    .is_some_and(|(cid, monitor)| {
                        st.active_cluster_workspace_for_monitor(monitor) == Some(cid)
                    });
                if active_cluster {
                    if let Some(monitor) = node_monitor {
                        let tile_animation_active = crate::animation::cluster_tile_rect_for(
                            st.ui.render_state.cluster_tile_tracks(),
                            node_id,
                            now,
                        )
                        .is_some();
                        if !tile_animation_active {
                            st.layout_active_cluster_workspace_for_monitor(
                                monitor.as_str(),
                                st.now_ms(now),
                            );
                        }
                    }
                } else if !pending_initial_reveal && !finalized_initial_spawn {
                    st.resolve_overlap_now();
                }
            }
        }

        let _ = reveal_pending_initial_toplevel_if_ready(st, node_id, false, now);
        if st.input.interaction_state.resize_active != Some(node_id)
            && st.model.field.is_visible(node_id)
            && first_geometry_commit
        {
            st.resolve_landmarks_overlapped_by_active_window(node_id);
        }
    }
}

pub(super) fn committed_window_geometry(
    bbox_loc: (i32, i32),
    bbox_size: (i32, i32),
    geometry: Option<(i32, i32, i32, i32)>,
) -> ((f32, f32, f32, f32), Vec2) {
    let (x, y, w, h) = geometry.unwrap_or((bbox_loc.0, bbox_loc.1, bbox_size.0, bbox_size.1));
    let width = w.max(1) as f32;
    let height = h.max(1) as f32;
    (
        (x as f32, y as f32, width, height),
        Vec2 {
            x: width,
            y: height,
        },
    )
}

pub(super) fn ensure_node_for_surface_impl(
    st: &mut Halley,
    surface: &WlSurface,
    label: &str,
    size_px: (i32, i32),
    intent: &InitialWindowIntent,
) -> NodeId {
    let key = surface_key(surface);
    if let Some(id) = st.model.surface_to_node.get(&key).copied() {
        return id;
    }

    let size = Vec2 {
        x: size_px.0.max(64) as f32,
        y: size_px.1.max(64) as f32,
    };
    let predicted_monitor = st.spawn_target_monitor_for_intent(intent);
    let now = Instant::now();
    st.model.spawn_state.pending_initial_spawn_placement = None;
    let stack_mode_open = st
        .cluster_bloom_for_monitor(predicted_monitor.as_str())
        .is_some();
    let effective_intent = if stack_mode_open {
        intent.bypassed()
    } else {
        intent.clone()
    };
    let active_cluster = st.active_cluster_workspace_for_monitor(predicted_monitor.as_str());
    let previous_overflow_len = active_cluster
        .and_then(|cid| {
            matches!(
                st.runtime.tuning.cluster_layout_kind(),
                halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
            )
            .then(|| {
                st.model.field.cluster(cid).map(|cluster| {
                    cluster
                        .overflow_members(st.runtime.tuning.tile_max_stack)
                        .len()
                })
            })
            .flatten()
        })
        .unwrap_or(0);
    let defer_rule_resolution =
        crate::compositor::spawn::rules::needs_deferred_rule_recheck(st, &effective_intent);
    let should_defer = defer_rule_resolution;
    if effective_intent.effective_overlap_policy()
        == halley_config::InitialWindowOverlapPolicy::None
        && !defer_rule_resolution
    {
        exit_monitor_fullscreen_for_new_toplevel(st, predicted_monitor.as_str(), now);
    }
    if should_exit_monitor_maximize_for_new_toplevel(&effective_intent) && !defer_rule_resolution {
        exit_monitor_maximize_for_new_toplevel(st, predicted_monitor.as_str(), now);
    }
    let defer_active_tiled_cluster_join = defer_rule_resolution
        && active_cluster.is_some()
        && !stack_mode_open
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
        );
    let join_cluster_layout = should_join_active_cluster_layout(
        active_cluster.is_some(),
        stack_mode_open,
        &effective_intent,
    ) && !defer_active_tiled_cluster_join;
    let stack_spawn_transition = active_cluster
        .filter(|_| {
            join_cluster_layout
                && matches!(
                    st.runtime.tuning.cluster_layout_kind(),
                    halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
                )
        })
        .map(|_| {
            crate::compositor::surface::active_stacking_visible_members_for_monitor(
                st,
                predicted_monitor.as_str(),
            )
        });
    let (monitor, id, spawned_in_active_cluster) = if join_cluster_layout {
        let cid = active_cluster.expect("checked");
        let spawn_result = if matches!(
            st.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
        ) {
            st.model
                .field
                .spawn_surface_in_active_cluster_front(cid, label.to_string(), size)
        } else {
            st.model
                .field
                .spawn_surface_in_active_cluster(cid, label.to_string(), size)
        };
        match spawn_result {
            Ok(id) => {
                if st.runtime.tuning.tile_new_on_top
                    && matches!(
                        st.runtime.tuning.cluster_layout_kind(),
                        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
                    )
                {
                    let _ = st.model.field.promote_cluster_member_to_master(cid, id);
                }
                (predicted_monitor, id, true)
            }
            Err(_) => {
                let (monitor, pos, _needs_pan) =
                    st.pick_spawn_position_with_intent(size, &effective_intent);
                let id = st.model.field.spawn_surface(label.to_string(), pos, size);
                (monitor, id, false)
            }
        }
    } else {
        let (monitor, pos, _needs_pan) =
            st.pick_spawn_position_with_intent(size, &effective_intent);
        let id = st.model.field.spawn_surface(label.to_string(), pos, size);
        (monitor, id, false)
    };
    st.model.surface_to_node.insert(key, id);
    st.assign_node_to_monitor(id, monitor.as_str());
    let lift_cluster_candidate = !spawned_in_active_cluster
        && crate::compositor::clusters::system::note_pending_lift_cluster_candidate_node(
            &mut *st,
            monitor.as_str(),
            id,
        );
    if lift_cluster_candidate {
        st.model.spawn_state.pending_initial_reveal.insert(id);
        let _ = st.model.field.set_detached(id, true);
    }
    if !lift_cluster_candidate
        && !spawned_in_active_cluster
        && let Some(record) = st.model.spawn_state.pending_initial_spawn_placement.take()
    {
        st.model
            .spawn_state
            .initial_spawn_placements
            .insert(id, record);
    }
    if !lift_cluster_candidate && effective_intent.matched_rule {
        st.model
            .spawn_state
            .applied_window_rules
            .insert(id, effective_intent.applied_rule_for_node());
    } else if !lift_cluster_candidate && should_defer {
        st.model.spawn_state.pending_rule_rechecks.insert(id);
        st.model.spawn_state.pending_initial_reveal.insert(id);
    }
    let _ = st
        .model
        .field
        .set_state(id, halley_core::field::NodeState::Active);
    if !lift_cluster_candidate {
        let _ = st.raise_overlap_policy_node(id);
    }
    if !lift_cluster_candidate && !spawned_in_active_cluster {
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
    }

    st.ui.render_state.cache.zoom_nominal_size.insert(id, size);
    st.model.workspace_state.last_active_size.insert(id, size);
    let joined_active_cluster = spawned_in_active_cluster;
    if !lift_cluster_candidate && st.runtime.tuning.animations_enabled() {
        st.ui
            .render_state
            .animator
            .observe_field(&st.model.field, now);
    }
    if !lift_cluster_candidate && should_defer && !joined_active_cluster {
        let _ = st.model.field.set_detached(id, true);
    }
    if let Some(cid) = active_cluster.filter(|_| joined_active_cluster) {
        if matches!(
            st.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
        ) {
            st.model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .insert(id, st.now_ms(now).saturating_add(140));
            st.request_window_animation_prewarm(id, now);
            if let Some(node) = st.model.field.node_mut(id) {
                node.visibility.set(Visibility::DETACHED, true);
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
            }
            st.layout_active_cluster_workspace_for_monitor(monitor.as_str(), st.now_ms(now));
        }
        if let Some(old_visible) = stack_spawn_transition.as_ref()
            && matches!(
                st.runtime.tuning.cluster_layout_kind(),
                halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking
            )
        {
            let new_visible =
                crate::compositor::surface::active_stacking_visible_members_for_monitor(
                    st,
                    monitor.as_str(),
                );
            let duration_ms = st.runtime.tuning.stack_animation_duration_ms();
            if st.runtime.tuning.stack_animation_enabled() {
                for node_id in old_visible.iter().chain(new_visible.iter()).copied() {
                    st.request_window_animation_prewarm(node_id, now);
                }
                st.ui.render_state.start_stack_cycle_transition(
                    monitor.as_str(),
                    halley_core::cluster_layout::ClusterCycleDirection::Prev,
                    old_visible.clone(),
                    new_visible,
                    now,
                    duration_ms,
                );
                st.request_maintenance();
            }
        }
        let overflow_len = st
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| {
                matches!(
                    st.runtime.tuning.cluster_layout_kind(),
                    halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
                )
                .then(|| {
                    cluster
                        .overflow_members(st.runtime.tuning.tile_max_stack)
                        .len()
                })
            })
            .unwrap_or(0);
        if overflow_len > previous_overflow_len {
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), st.now_ms(now));
        }
    }
    refresh_node_identity_for_surface(st, surface, label);
    id
}
