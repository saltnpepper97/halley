use std::collections::HashSet;
use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::activity::CommitActivity;
use crate::compositor::ctx::SurfaceLifecycleCtx;
use crate::compositor::root::Halley;
use crate::compositor::spawn::rules::InitialWindowIntent;
use crate::compositor::surface_ops::is_active_cluster_workspace_member;

pub(crate) fn refresh_surface_identity(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: &WlSurface,
    fallback_label: &str,
) {
    refresh_node_identity_for_surface(ctx.st, surface, fallback_label);
}

pub(crate) fn on_surface_commit(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: &WlSurface,
    now: Instant,
) {
    note_commit(ctx.st, surface, now);
}

pub(crate) fn ensure_node_for_surface(
    ctx: &mut SurfaceLifecycleCtx<'_>,
    surface: &WlSurface,
    label: &str,
    size_px: (i32, i32),
    intent: &InitialWindowIntent,
) -> NodeId {
    ensure_node_for_surface_impl(ctx.st, surface, label, size_px, intent)
}

#[allow(dead_code)]
pub(crate) fn drop_surface(ctx: &mut SurfaceLifecycleCtx<'_>, surface: &WlSurface) {
    drop_surface_impl(ctx.st, surface);
}

pub(crate) fn on_toplevel_destroyed(ctx: &mut SurfaceLifecycleCtx<'_>, surface: ToplevelSurface) {
    let st = &mut ctx.st;
    let key = surface.wl_surface().id();
    let closing_id = st.model.surface_to_node.get(&key).copied();
    let had_keyboard_focus = st
        .platform
        .seat
        .get_keyboard()
        .and_then(|kb| kb.current_focus())
        .is_some_and(|focused| surface_tree_root(&focused).id() == key);
    let had_pointer_focus = st
        .platform
        .seat
        .get_pointer()
        .and_then(|ptr| ptr.current_focus())
        .is_some_and(|focused| surface_tree_root(&focused).id() == key);
    let focused_monitor = st
        .model
        .surface_to_node
        .get(&key)
        .and_then(|id| st.model.monitor_state.node_monitor.get(id))
        .cloned();

    if had_keyboard_focus || had_pointer_focus {
        eventline::info!(
            "toplevel_destroyed with active focus (keyboard={} pointer={}); scheduling input state reset",
            had_keyboard_focus,
            had_pointer_focus
        );
        st.input.interaction_state.reset_input_state_requested = true;
        if let Some(ref focused_monitor) = focused_monitor {
            st.model.spawn_state.pending_spawn_monitor = Some(focused_monitor.clone());
            eventline::info!(
                "pending spawn monitor latched from destroyed toplevel: {}",
                focused_monitor
            );
        }
    }

    if had_keyboard_focus {
        st.clear_keyboard_focus();
    }

    if had_keyboard_focus
        && st.runtime.tuning.close_restore_focus
        && let (Some(closing_id), Some(focused_monitor)) = (closing_id, focused_monitor.as_deref())
    {
        let now = Instant::now();
        if st
            .active_cluster_workspace_for_monitor(focused_monitor)
            .is_some()
        {
            if let Some(previous) =
                st.previous_window_from_trail_on_close(focused_monitor, closing_id)
            {
                st.set_interaction_focus(Some(previous), 30_000, now);
            } else if let Some(fallback) = st
                .last_focused_surface_node_for_monitor(focused_monitor)
                .filter(|&id| id != closing_id)
            {
                st.set_interaction_focus(Some(fallback), 30_000, now);
            }
        } else if let Some(previous) =
            st.previous_window_from_trail_on_close(focused_monitor, closing_id)
        {
            let _ = st.restore_focus_to_node_after_close(focused_monitor, previous, now);
        } else if let Some(fallback) = st
            .last_focused_surface_node_for_monitor(focused_monitor)
            .filter(|&id| id != closing_id)
            .or_else(|| {
                st.last_focused_surface_node()
                    .filter(|&id| id != closing_id)
            })
        {
            let _ = st.restore_focus_to_node_after_close(focused_monitor, fallback, now);
        }
    } else if had_keyboard_focus
        && !st.runtime.tuning.close_restore_focus
        && let Some(focused_monitor) = focused_monitor.as_deref()
    {
        st.model
            .focus_state
            .blocked_monitor_focus_restore
            .insert(focused_monitor.to_string());
    }
    if had_pointer_focus {
        crate::compositor::interaction::pointer::clear_pointer_focus(st);
    }
}

pub(crate) fn reconcile_surface_bindings(st: &mut Halley) {
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
            if st.model.focus_state.pan_restore_active_focus == Some(id) {
                st.model.focus_state.pan_restore_active_focus = None;
            }
            st.model.workspace_state.manual_collapsed_nodes.remove(&id);
            st.ui.render_state.zoom_nominal_size.remove(&id);
            st.ui.render_state.zoom_resize_fallback.remove(&id);
            st.ui.render_state.zoom_resize_reject_streak.remove(&id);
            st.ui.render_state.zoom_last_observed_size.remove(&id);
            st.ui.render_state.zoom_resize_static_streak.remove(&id);
            st.model.node_app_ids.remove(&id);
            st.model.workspace_state.last_active_size.remove(&id);
            st.ui.render_state.bbox_loc.remove(&id);
            st.ui.render_state.window_geometry.remove(&id);
            st.model
                .spawn_state
                .pending_spawn_activate_at_ms
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
            st.model.focus_state.last_surface_focus_ms.remove(&id);
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
            let _ = st.remove_node_from_field(id, st.now_ms(Instant::now()));
        }
    }

    st.runtime.surface_activity.retain(|k, _| alive.contains(k));
}

fn exit_monitor_fullscreen_for_new_toplevel(st: &mut Halley, monitor: &str, now: Instant) {
    if let Some(existing) = st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(monitor)
        .copied()
    {
        st.exit_xdg_fullscreen(existing, now);
    }
}

fn should_join_active_cluster_layout(
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
fn surface_key(surface: &WlSurface) -> ObjectId {
    surface.id()
}

fn surface_tree_root(surface: &WlSurface) -> WlSurface {
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

fn refresh_node_identity_for_surface(st: &mut Halley, surface: &WlSurface, fallback_label: &str) {
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
            st.model.node_app_ids.insert(node_id, app_id);
        }
        None => {
            st.model.node_app_ids.remove(&node_id);
        }
    }

    maybe_apply_pending_initial_window_rule(st, node_id, &root_surface, Instant::now());
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
    if !intent.matched_rule {
        if !crate::compositor::spawn::rules::needs_deferred_rule_recheck(st, &intent) {
            st.model.spawn_state.pending_rule_rechecks.remove(&node_id);
            st.model.spawn_state.pending_initial_reveal.remove(&node_id);
            st.reveal_new_toplevel_node(node_id, intent.is_transient, now);
        }
        return;
    }
    let monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    if st.cluster_bloom_for_monitor(monitor.as_str()).is_some() {
        st.model.spawn_state.pending_rule_rechecks.remove(&node_id);
        st.model.spawn_state.pending_initial_reveal.remove(&node_id);
        st.reveal_new_toplevel_node(node_id, intent.is_transient, now);
        return;
    }

    if intent.rule.cluster_participation == halley_config::InitialWindowClusterParticipation::Float
        && let Some(cid) = st.model.field.cluster_id_for_member_public(node_id)
        && st.active_cluster_workspace_for_monitor(monitor.as_str()) == Some(cid)
        && let Some(pos) = st.model.field.node(node_id).map(|node| node.pos)
    {
        let _ = st.detach_member_from_cluster(cid, node_id, pos, now);
        st.assign_node_to_monitor(node_id, monitor.as_str());
    }

    if let Some(size) = st.model.field.node(node_id).map(|node| node.intrinsic_size) {
        let (_, pos, _) = st.pick_spawn_position_with_intent(size, &intent);
        let _ = st.model.field.carry(node_id, pos);
    }

    st.set_recent_top_node(node_id, now + std::time::Duration::from_millis(1200));
    st.model
        .spawn_state
        .applied_window_rules
        .insert(node_id, intent.applied_rule_for_node());
    st.model.spawn_state.pending_rule_rechecks.remove(&node_id);
    st.model.spawn_state.pending_initial_reveal.remove(&node_id);
    st.reveal_new_toplevel_node(node_id, intent.is_transient, now);
}

fn note_commit(st: &mut Halley, surface: &WlSurface, now: Instant) {
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
    } else {
        None
    }
    .unwrap_or_else(|| st.model.monitor_state.focused_monitor.clone());

    for (name, output) in &st.model.monitor_state.outputs {
        if *name == target_monitor {
            output.enter(surface);
        } else {
            output.leave(surface);
        }
    }

    crate::compositor::monitor::layer_shell::maybe_grant_layer_surface_focus_on_commit(
        &mut st.layer_shell_ctx(),
        surface,
    );

    if let Some(node_id) = st.model.surface_to_node.get(&root_key).copied() {
        st.ui.render_state.mark_window_offscreen_dirty(node_id);
        refresh_node_identity_for_surface(st, &root_surface, "Window");
        use smithay::desktop::utils::bbox_from_surface_tree;
        use smithay::wayland::shell::xdg::SurfaceCachedState;

        let bbox = bbox_from_surface_tree(&root_surface, (0, 0));
        st.ui
            .render_state
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
        st.ui
            .render_state
            .window_geometry
            .insert(node_id, window_geometry);
        if is_active_cluster_workspace_member(st, node_id) {
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
                        st.layout_active_cluster_workspace_for_monitor(
                            monitor.as_str(),
                            st.now_ms(now),
                        );
                    }
                } else {
                    st.resolve_overlap_now();
                }
            }
        }
    }
}

fn committed_window_geometry(
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

fn ensure_node_for_surface_impl(
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
    exit_monitor_fullscreen_for_new_toplevel(st, predicted_monitor.as_str(), now);
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
            st.model.field.cluster(cid).map(|cluster| {
                cluster
                    .overflow_members(st.runtime.tuning.tile_max_stack)
                    .len()
            })
        })
        .unwrap_or(0);
    let join_cluster_layout = should_join_active_cluster_layout(
        active_cluster.is_some(),
        stack_mode_open,
        &effective_intent,
    );
    let (monitor, id, spawned_in_active_cluster) = if join_cluster_layout {
        let cid = active_cluster.expect("checked");
        match st
            .model
            .field
            .spawn_surface_in_active_cluster(cid, label.to_string(), size)
        {
            Ok(id) => {
                if st.runtime.tuning.tile_new_on_top {
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
    st.assign_node_to_monitor(id, monitor.as_str());
    if effective_intent.matched_rule {
        st.model
            .spawn_state
            .applied_window_rules
            .insert(id, effective_intent.applied_rule_for_node());
    } else if crate::compositor::spawn::rules::needs_deferred_rule_recheck(st, &effective_intent) {
        st.model.spawn_state.pending_rule_rechecks.insert(id);
        st.model.spawn_state.pending_initial_reveal.insert(id);
    }
    let _ = st
        .model
        .field
        .set_state(id, halley_core::field::NodeState::Active);
    if !spawned_in_active_cluster {
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
    }

    st.model.surface_to_node.insert(key, id);
    st.ui.render_state.zoom_nominal_size.insert(id, size);
    st.model.workspace_state.last_active_size.insert(id, size);
    let joined_active_cluster = spawned_in_active_cluster;
    if st.runtime.tuning.dev_anim_enabled {
        st.ui
            .render_state
            .animator
            .observe_field(&st.model.field, now);
    }
    if let Some(cid) = active_cluster.filter(|_| joined_active_cluster) {
        let overflow_len = st
            .model
            .field
            .cluster(cid)
            .map(|cluster| {
                cluster
                    .overflow_members(st.runtime.tuning.tile_max_stack)
                    .len()
            })
            .unwrap_or(0);
        if overflow_len > previous_overflow_len {
            st.reveal_cluster_overflow_for_monitor(monitor.as_str(), st.now_ms(now));
        }
    }
    refresh_node_identity_for_surface(st, surface, label);
    id
}

fn drop_surface_impl(st: &mut Halley, surface: &WlSurface) {
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
        st.drop_fullscreen_surface(id, Instant::now());
        if st.model.focus_state.pan_restore_active_focus == Some(id) {
            st.model.focus_state.pan_restore_active_focus = None;
        }
        st.ui.render_state.zoom_nominal_size.remove(&id);
        st.ui.render_state.zoom_resize_fallback.remove(&id);
        st.ui.render_state.zoom_resize_reject_streak.remove(&id);
        st.ui.render_state.zoom_last_observed_size.remove(&id);
        st.ui.render_state.zoom_resize_static_streak.remove(&id);
        st.model.node_app_ids.remove(&id);
        for trail in st.model.focus_state.focus_trail.values_mut() {
            trail.forget_node(id);
        }
        st.model.workspace_state.last_active_size.remove(&id);
        st.ui.render_state.bbox_loc.remove(&id);
        st.ui.render_state.window_geometry.remove(&id);
        st.model
            .spawn_state
            .pending_spawn_activate_at_ms
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
        st.model.focus_state.last_surface_focus_ms.remove(&id);
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
        let _ = st.remove_node_from_field(id, st.now_ms(Instant::now()));
    }
    st.request_maintenance();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};

    #[test]
    fn committed_window_geometry_prefers_xdg_geometry_size() {
        let (geometry, size) =
            committed_window_geometry((4, 6), (1200, 920), Some((12, 18, 840, 620)));

        assert_eq!(geometry, (12.0, 18.0, 840.0, 620.0));
        assert_eq!(size, Vec2 { x: 840.0, y: 620.0 });
    }

    #[test]
    fn committed_window_geometry_falls_back_to_bbox_size() {
        let (geometry, size) = committed_window_geometry((4, 6), (1200, 920), None);

        assert_eq!(geometry, (4.0, 6.0, 1200.0, 920.0));
        assert_eq!(
            size,
            Vec2 {
                x: 1200.0,
                y: 920.0
            }
        );
    }

    #[test]
    fn new_toplevel_on_fullscreen_monitor_exits_only_that_monitor_fullscreen() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let fullscreen_left = state.model.field.spawn_surface(
            "fullscreen-left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 200.0, y: 140.0 },
        );
        let fullscreen_right = state.model.field.spawn_surface(
            "fullscreen-right",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 200.0, y: 140.0 },
        );
        state.assign_node_to_monitor(fullscreen_left, "left");
        state.assign_node_to_monitor(fullscreen_right, "right");
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), fullscreen_left);
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("right".to_string(), fullscreen_right);

        exit_monitor_fullscreen_for_new_toplevel(&mut state, "left", Instant::now());

        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key("left")
        );
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("right"),
            Some(&fullscreen_right)
        );
    }

    #[test]
    fn tiled_cluster_layout_participation_honors_layout_and_float() {
        let layout_intent = InitialWindowIntent {
            app_id: Some("firefox".to_string()),
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::None,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Layout,
            },
            matched_rule: true,
            is_transient: false,
            prefer_app_intent: false,
        };
        let float_intent = InitialWindowIntent {
            rule: ResolvedInitialWindowRule {
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                ..layout_intent.rule
            },
            ..layout_intent.clone()
        };

        assert!(should_join_active_cluster_layout(
            true,
            false,
            &layout_intent
        ));
        assert!(!should_join_active_cluster_layout(
            true,
            false,
            &float_intent
        ));
        assert!(!should_join_active_cluster_layout(
            true,
            true,
            &layout_intent
        ));
    }
}
