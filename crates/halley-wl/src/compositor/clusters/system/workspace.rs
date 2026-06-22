use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::*;

pub(crate) fn restore_cluster_workspace_monitor(st: &mut Halley, monitor: &str) {
    let Some(vp) = st
        .model
        .cluster_state
        .workspace_prev_viewports
        .remove(monitor)
    else {
        return;
    };
    if st.model.monitor_state.current_monitor == monitor {
        st.model.viewport = vp;
        st.model.zoom_ref_size = st.model.viewport.size;
        crate::compositor::monitor::camera::snap_camera_targets_to_live(&mut *st);
        st.runtime.tuning.viewport_center = st.model.viewport.center;
        st.runtime.tuning.viewport_size = st.model.viewport.size;
    }
    if let Some(space) = st.model.monitor_state.monitors.get_mut(monitor) {
        space.viewport = vp;
        space.zoom_ref_size = vp.size;
        space.camera_target_center = vp.center;
        space.camera_target_view_size = vp.size;
    }
}

pub(super) fn clear_cluster_shell_state(st: &mut Halley, cid: ClusterId) {
    let active_monitors = st
        .model
        .cluster_state
        .active_cluster_workspaces
        .iter()
        .filter_map(|(monitor, active_cid)| (*active_cid == cid).then(|| monitor.clone()))
        .collect::<Vec<_>>();
    for monitor in &active_monitors {
        for id in st
            .model
            .cluster_state
            .workspace_hidden_nodes
            .remove(monitor.as_str())
            .unwrap_or_default()
        {
            if st.model.field.node(id).is_some() {
                let _ = st.model.field.set_detached(id, false);
            }
        }
        st.model
            .cluster_state
            .workspace_core_positions
            .remove(monitor.as_str());
        clear_cluster_overflow_for_monitor(st, monitor.as_str());
        if st
            .input
            .interaction_state
            .cluster_overflow_drag_preview
            .as_ref()
            .is_some_and(|preview| preview.monitor == *monitor)
        {
            st.input.interaction_state.cluster_overflow_drag_preview = None;
            crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
        }
        restore_cluster_workspace_monitor(st, monitor.as_str());
    }
    st.model
        .cluster_state
        .active_cluster_workspaces
        .retain(|_, active_cid| *active_cid != cid);
    st.model
        .cluster_state
        .cluster_bloom_open
        .retain(|_, open_cid| *open_cid != cid);
    if st
        .input
        .interaction_state
        .cluster_join_candidate
        .as_ref()
        .is_some_and(|candidate| candidate.cluster_id == cid)
    {
        st.input.interaction_state.cluster_join_candidate = None;
    }
}

pub fn collapse_active_cluster_workspace(st: &mut Halley, now: Instant) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    exit_cluster_workspace_for_monitor(st, monitor.as_str(), now)
}

pub fn toggle_cluster_workspace_by_core(st: &mut Halley, core_id: NodeId, now: Instant) -> bool {
    let monitor = st.model.monitor_state.current_monitor.clone();
    if let Some(cid) = active_cluster_workspace_for_monitor(st, monitor.as_str())
        && st.model.field.cluster_id_for_core_public(core_id) == Some(cid)
    {
        return exit_cluster_workspace_for_monitor(st, monitor.as_str(), now);
    }
    enter_cluster_workspace_by_core(st, core_id, monitor.as_str(), now)
}

pub(crate) fn enter_cluster_workspace_by_core(
    st: &mut Halley,
    core_id: NodeId,
    monitor: &str,
    now: Instant,
) -> bool {
    let Some(cid) = st.model.field.cluster_id_for_core_public(core_id) else {
        return false;
    };
    if active_cluster_workspace_for_monitor(st, monitor) == Some(cid) {
        return true;
    }
    if active_cluster_workspace_for_monitor(st, monitor).is_some() {
        let _ = exit_cluster_workspace_for_monitor(st, monitor, now);
    }
    let perf_start = crate::perf::start();
    let plan_start = crate::perf::start();
    let Some(plan) =
        crate::compositor::clusters::read::plan_enter_cluster_workspace(st, core_id, monitor)
    else {
        return false;
    };
    let plan_ms = plan_start.map(crate::perf::elapsed_ms);
    let _ = sync_cluster_monitor(st, cid, Some(monitor));
    let previous_full_viewport = if st.model.monitor_state.current_monitor == monitor {
        st.model.viewport
    } else {
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport)
            .unwrap_or(plan.current_viewport)
    };
    st.model
        .cluster_state
        .workspace_prev_viewports
        .insert(monitor.to_string(), previous_full_viewport);
    st.model
        .cluster_state
        .workspace_core_positions
        .insert(monitor.to_string(), plan.core_pos);
    if st.model.monitor_state.current_monitor == monitor {
        let live_viewport = st
            .model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport)
            .unwrap_or(plan.current_viewport);
        st.input.interaction_state.viewport_pan_anim = None;
        st.model.viewport = live_viewport;
        st.model.zoom_ref_size = live_viewport.size;
        st.model.camera_target_center = live_viewport.center;
        st.model.camera_target_view_size = live_viewport.size;
        st.runtime.tuning.viewport_center = live_viewport.center;
        st.runtime.tuning.viewport_size = live_viewport.size;
    }
    st.model.spawn_state.pending_spawn_pan_queue.clear();
    st.model.spawn_state.active_spawn_pan = None;
    st.input.interaction_state.viewport_pan_anim = None;
    st.model.spawn_state.pending_spawn_monitor = None;
    let spawn = st.spawn_monitor_state_mut(monitor);
    spawn.spawn_pan_start_center = None;
    for id in &plan.hidden_ids {
        let _ = st.model.field.set_detached(*id, true);
    }
    let _ = st.model.field.set_detached(plan.core_id, true);
    let _ = st.model.field.activate_cluster_workspace(plan.cid);

    st.model
        .cluster_state
        .workspace_hidden_nodes
        .insert(monitor.to_string(), plan.hidden_ids);
    st.model
        .cluster_state
        .active_cluster_workspaces
        .retain(|name, active_cid| *active_cid != cid || name == monitor);
    st.model
        .cluster_state
        .active_cluster_workspaces
        .insert(monitor.to_string(), cid);
    st.model.cluster_state.cluster_bloom_open.remove(monitor);
    st.set_interaction_focus(None, 0, now);
    let now_ms = st.now_ms(now);
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewport_forced(st, monitor);
    let layout_start = crate::perf::start();
    layout_active_cluster_workspace_for_monitor(st, monitor, now_ms);
    let layout_ms = layout_start.map(crate::perf::elapsed_ms);
    if matches!(
        active_cluster_layout_kind(st),
        ClusterWorkspaceLayoutKind::Stacking
    ) && let Some(front) = st
        .model
        .field
        .cluster(plan.cid)
        .and_then(|cluster| cluster.members().first().copied())
    {
        st.set_recent_top_node(front, now + std::time::Duration::from_millis(1200));
        st.set_interaction_focus(Some(front), 30_000, now);
        st.update_focus_tracking_for_surface(front, now_ms);
    } else if matches!(
        active_cluster_layout_kind(st),
        ClusterWorkspaceLayoutKind::Tiling
    ) {
        let _ = focus_active_tiled_cluster_member_for_monitor(st, monitor, Some(0), now);
    }
    let overflow_start = crate::perf::start();
    refresh_cluster_overflow_for_monitor(st, monitor, now_ms, false);
    if let Some(start) = perf_start {
        let members = st
            .model
            .field
            .cluster(cid)
            .map_or(0, |cluster| cluster.members().len());
        eventline::info!(
            "perf enter_cluster_workspace monitor={} members={} took={:.2}ms (plan={:.2} layout={:.2} overflow={:.2})",
            monitor,
            members,
            crate::perf::elapsed_ms(start),
            plan_ms.unwrap_or_default(),
            layout_ms.unwrap_or_default(),
            overflow_start
                .map(crate::perf::elapsed_ms)
                .unwrap_or_default(),
        );
    }
    true
}

pub(crate) fn exit_cluster_workspace_for_monitor(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
) -> bool {
    let Some(plan) = crate::compositor::clusters::read::plan_exit_cluster_workspace(st, monitor)
    else {
        return false;
    };

    for id in &plan.hidden_ids {
        let _ = st.model.field.set_detached(*id, false);
    }

    let _ = st.model.field.deactivate_cluster_workspace(plan.cid);
    let core = st.collapse_cluster(plan.cid).or(plan.core_id);
    if let Some(core_id) = core {
        let _ = ensure_cluster_name_record_for_monitor(st, plan.cid, monitor);
        let _ = relabel_cluster_core(st, plan.cid);
        let preserved_core_pos = st
            .model
            .cluster_state
            .workspace_core_positions
            .remove(monitor)
            .or(plan.core_pos);
        if let Some(core_pos) = preserved_core_pos {
            let _ = st.model.field.carry(core_id, core_pos);
        }
        let _ = st.model.field.set_detached(core_id, false);
        st.assign_node_to_monitor(core_id, monitor);
        let now_ms = st.now_ms(now);
        let _ = st.model.field.touch(core_id, now_ms);
    }

    restore_cluster_workspace_monitor(st, monitor);
    st.model
        .cluster_state
        .active_cluster_workspaces
        .remove(monitor);
    st.model
        .cluster_state
        .workspace_hidden_nodes
        .remove(monitor);
    crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
    clear_cluster_overflow_for_monitor(st, monitor);
    if st
        .input
        .interaction_state
        .cluster_overflow_drag_preview
        .as_ref()
        .is_some_and(|preview| preview.monitor == monitor)
    {
        st.input.interaction_state.cluster_overflow_drag_preview = None;
        crate::compositor::interaction::pointer::set_cursor_override_icon(st, None);
    }
    if let Some(core_id) = core {
        st.set_recent_top_node(core_id, now + std::time::Duration::from_millis(1200));
        st.set_interaction_focus(Some(core_id), 30_000, now);
    }
    true
}

pub(crate) fn clear_cluster_tile_animation_for_node(st: &mut Halley, node_id: NodeId) {
    st.ui
        .render_state
        .clear_cluster_tile_animation_for_node(node_id);
}

pub(crate) fn update_tiled_cluster_animation_targets(
    st: &mut Halley,
    plan: &ClusterLayoutPlan,
    dragged_member: Option<NodeId>,
    now: Instant,
) {
    for placement in &plan.tiles {
        if st
            .model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .contains_key(&placement.node_id)
            || Some(placement.node_id) == dragged_member
        {
            clear_cluster_tile_animation_for_node(st, placement.node_id);
            continue;
        }

        let current_rect = if st
            .ui
            .render_state
            .remove_cluster_tile_entry_pending(placement.node_id)
        {
            None
        } else {
            crate::animation::cluster_tile_rect_from_field(&st.model.field, placement.node_id)
        };
        let frozen_geo = st
            .ui
            .render_state
            .cache
            .window_geometry
            .get(&placement.node_id)
            .copied();
        if current_rect.is_some_and(|rect| rect.alpha > 0.01)
            && let Some(geo) = frozen_geo
        {
            st.ui
                .render_state
                .remember_cluster_tile_frozen_geometry(placement.node_id, geo);
        }
        let duration_ms = st.runtime.tuning.tile_animation_duration_ms();
        if st.runtime.tuning.tile_animation_enabled() {
            crate::animation::set_cluster_tile_target(
                st.ui.render_state.cluster_tile_tracks_mut(),
                current_rect,
                placement.node_id,
                placement.rect,
                now,
                duration_ms,
            );
            st.request_window_animation_prewarm(placement.node_id, now);
        } else {
            st.ui
                .render_state
                .remove_cluster_tile_track(placement.node_id);
        }
    }
}

pub(crate) fn current_surface_size_map_for_members(
    st: &Halley,
    members: &HashSet<NodeId>,
) -> HashMap<NodeId, Vec2> {
    let mut sizes = HashMap::with_capacity(members.len());
    for (&node_id, &(_, _, w, h)) in &st.ui.render_state.cache.window_geometry {
        if members.contains(&node_id) {
            sizes.insert(
                node_id,
                Vec2 {
                    x: w.max(1.0),
                    y: h.max(1.0),
                },
            );
        }
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        let Some(node_id) = st.model.surface_to_node.get(&key).copied() else {
            continue;
        };
        if sizes.contains_key(&node_id) || !members.contains(&node_id) {
            continue;
        }

        let size = with_states(wl, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        })
        .map(|g| Vec2 {
            x: g.size.w.max(1) as f32,
            y: g.size.h.max(1) as f32,
        })
        .or_else(|| {
            top.with_committed_state(|state| state.and_then(|state| state.size))
                .map(|sz| Vec2 {
                    x: sz.w.max(1) as f32,
                    y: sz.h.max(1) as f32,
                })
        })
        .unwrap_or_else(|| {
            let bbox = bbox_from_surface_tree(wl, (0, 0));
            Vec2 {
                x: bbox.size.w.max(1) as f32,
                y: bbox.size.h.max(1) as f32,
            }
        });
        sizes.insert(node_id, size);
    }

    for &node_id in members {
        sizes.entry(node_id).or_insert_with(|| {
            st.model
                .field
                .node(node_id)
                .map_or(Vec2 { x: 1.0, y: 1.0 }, |node| Vec2 {
                    x: node.intrinsic_size.x.max(1.0),
                    y: node.intrinsic_size.y.max(1.0),
                })
        });
    }

    sizes
}

pub(crate) fn layout_active_cluster_workspace_for_monitor(
    st: &mut Halley,
    monitor: &str,
    now_ms: u64,
) {
    let Some(cid) = active_cluster_workspace_for_monitor(st, monitor) else {
        return;
    };
    let Some(cluster) = st.model.field.cluster(cid) else {
        // The cluster dissolved (e.g. its last window closed) while its
        // workspace was still active. Drop the stale workspace entry and
        // recompute the work area: with no active cluster the aperture
        // reservation falls to zero, so the frozen top gap is released
        // immediately instead of lingering (`refresh` re-checks the now-
        // unlocked monitor). The explicit exit path already refreshes; this
        // covers the implicit-dissolve path.
        st.model
            .cluster_state
            .active_cluster_workspaces
            .remove(monitor);
        crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(st);
        return;
    };
    let members = cluster.members().to_vec();
    let member_set = members.iter().copied().collect::<HashSet<_>>();
    let dragged_member = st
        .input
        .interaction_state
        .drag_authority_node
        .filter(|id| member_set.contains(id));
    if st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(monitor)
        .is_some_and(|fullscreen_id| member_set.contains(fullscreen_id))
    {
        return;
    }
    let Some(plan) = crate::compositor::clusters::read::plan_active_cluster_layout(st, monitor)
    else {
        return;
    };
    let now = Instant::now();
    if matches!(plan.kind, ClusterWorkspaceLayoutKind::Tiling) {
        update_tiled_cluster_animation_targets(st, &plan, dragged_member, now);
    }
    let visible_members = plan
        .tiles
        .iter()
        .map(|tile| tile.node_id)
        .filter(|id| {
            !st.model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .contains_key(id)
        })
        .collect::<HashSet<_>>();
    let visible_surface_sizes = current_surface_size_map_for_members(st, &visible_members);
    if let Some(cluster) = st.model.field.cluster_mut(cid) {
        for member_id in &members {
            if let Some(node) = cluster.workspace_member_mut(*member_id) {
                let visible = visible_members.contains(member_id);
                node.visibility.set(Visibility::DETACHED, !visible);
                node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, !visible);
            }
        }
    }
    for placement in plan.tiles {
        let nid = placement.node_id;
        if Some(nid) == dragged_member
            || st
                .model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .contains_key(&nid)
        {
            continue;
        }
        let rect = placement.rect;
        let target_size = Vec2 {
            x: rect.w.max(64.0),
            y: rect.h.max(64.0),
        };
        let target_pos = Vec2 {
            x: rect.x + rect.w * 0.5,
            y: rect.y + rect.h * 0.5,
        };
        let layout_changed = st.model.field.node(nid).is_none_or(|node| {
            (node.intrinsic_size.x - target_size.x).abs() > 0.5
                || (node.intrinsic_size.y - target_size.y).abs() > 0.5
                || (node.pos.x - target_pos.x).abs() > 0.5
                || (node.pos.y - target_pos.y).abs() > 0.5
                || node.state != halley_core::field::NodeState::Active
                || node.visibility.has(Visibility::DETACHED)
                || node.visibility.has(Visibility::HIDDEN_BY_CLUSTER)
        });
        if let Some(cluster) = st.model.field.cluster_mut(cid)
            && let Some(node) = cluster.workspace_member_mut(nid)
            && layout_changed
        {
            node.visibility.set(Visibility::DETACHED, false);
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, false);
            node.intrinsic_size = target_size;
            node.state = halley_core::field::NodeState::Active;
            node.footprint = node.resize_footprint.unwrap_or(node.intrinsic_size);
            node.pos = target_pos;
        }
        if layout_changed {
            st.set_last_active_size_now(nid, target_size);
        }
        let surface_size_changed = visible_surface_sizes.get(&nid).is_none_or(|size| {
            (size.x - target_size.x).abs() > 0.5 || (size.y - target_size.y).abs() > 0.5
        });
        if surface_size_changed {
            st.request_toplevel_resize(nid, rect.w.round() as i32, rect.h.round() as i32);
        }
    }
    refresh_cluster_overflow_for_monitor(st, monitor, now_ms, false);
}
