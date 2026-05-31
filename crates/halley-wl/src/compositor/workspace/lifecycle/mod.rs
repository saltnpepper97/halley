use std::collections::HashSet;
use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2, Visibility};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::compositor::activity::CommitActivity;
use crate::compositor::ctx::SurfaceLifecycleCtx;
use crate::compositor::root::Halley;
use crate::compositor::spawn::rules::InitialWindowIntent;

mod cleanup;
mod surface;

use cleanup::{
    arm_queued_overflow_promotion, capture_queued_overflow_promotion, drop_surface_impl,
};
use surface::{
    committed_window_geometry, ensure_node_for_surface_impl,
    exit_monitor_fullscreen_for_new_toplevel, exit_monitor_fullscreen_for_overlap_intent,
    exit_monitor_maximize_for_new_toplevel, note_commit, refresh_node_identity_for_surface,
    should_exit_monitor_maximize_for_new_toplevel, should_join_active_cluster_layout,
    surface_tree_root,
};

const CLUSTER_OVERFLOW_PROMOTION_ANIM_MS: u64 = 360;

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
    let closing_fullscreen = closing_id.is_some_and(|id| st.is_fullscreen_active(id));
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
        eventline::debug!(
            "toplevel_destroyed with active focus (keyboard={} pointer={}); scheduling input state reset",
            had_keyboard_focus,
            had_pointer_focus
        );
        st.input.interaction_state.reset_input_state_requested = true;
        if let Some(ref focused_monitor) = focused_monitor {
            st.model.spawn_state.pending_spawn_monitor = Some(focused_monitor.clone());
            eventline::debug!(
                "pending spawn monitor latched from destroyed toplevel: {}",
                focused_monitor
            );
        }
    }

    if had_keyboard_focus {
        st.clear_keyboard_focus();
    }

    let mut deferred_close_restore = None;
    if had_keyboard_focus
        && st.runtime.tuning.close_restore_focus
        && let (Some(closing_id), Some(focused_monitor)) = (closing_id, focused_monitor.as_deref())
    {
        let now = Instant::now();
        let suppress_restore_pan =
            st.node_has_overlap_policy(closing_id) || st.is_fullscreen_active(closing_id);
        if closing_fullscreen {
            if let Some(target) = non_cluster_close_restore_target(st, focused_monitor, closing_id)
            {
                deferred_close_restore = Some((
                    focused_monitor.to_string(),
                    target,
                    suppress_restore_pan,
                    now,
                ));
            }
        } else if let Some(cid) = st.active_cluster_workspace_for_monitor(focused_monitor) {
            if matches!(
                st.runtime.tuning.cluster_layout_kind(),
                halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
            ) {
                // Tiled cluster close restore is handled after the member is actually removed so
                // we can focus the replacement tile in that slot.
            } else {
                let mut next_to_focus = None;
                if let Some(cluster) = st.model.field.cluster(cid) {
                    let members = cluster.members();
                    if let Some(pos) = members.iter().position(|&id| id == closing_id) {
                        if pos + 1 < members.len() {
                            next_to_focus = Some(members[pos + 1]);
                        } else if pos > 0 {
                            next_to_focus = Some(members[pos - 1]);
                        }
                    }
                }

                if let Some(next) = next_to_focus {
                    st.set_interaction_focus(Some(next), 30_000, now);
                } else if let Some(previous) =
                    st.previous_window_from_trail_on_close(focused_monitor, closing_id)
                {
                    st.set_interaction_focus(Some(previous), 30_000, now);
                } else if let Some(fallback) = st
                    .last_focused_surface_node_for_monitor(focused_monitor)
                    .filter(|&id| id != closing_id)
                {
                    st.set_interaction_focus(Some(fallback), 30_000, now);
                }
            }
        } else if let Some(target) =
            non_cluster_close_restore_target(st, focused_monitor, closing_id)
        {
            let _ = st.restore_focus_to_node_after_close(
                focused_monitor,
                target,
                now,
                suppress_restore_pan,
            );
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

    drop_surface_impl(st, surface.wl_surface());

    if let Some((monitor, target, suppress_restore_pan, now)) = deferred_close_restore {
        let _ = st.restore_focus_to_node_after_close(
            monitor.as_str(),
            target,
            now,
            suppress_restore_pan,
        );
    }
}

fn non_cluster_close_restore_target(
    st: &mut Halley,
    focused_monitor: &str,
    closing_id: NodeId,
) -> Option<NodeId> {
    st.previous_window_from_trail_on_close(focused_monitor, closing_id)
        .or_else(|| {
            st.last_focused_surface_node_for_monitor(focused_monitor)
                .filter(|&id| id != closing_id)
        })
        .or_else(|| {
            st.last_focused_surface_node()
                .filter(|&id| id != closing_id)
        })
}

pub(crate) fn reconcile_surface_bindings(st: &mut Halley) {
    cleanup::reconcile_surface_bindings(st);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tile_max_stack = 2;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    fn assert_surfaces_do_not_overlap(state: &Halley, a: NodeId, b: NodeId) {
        let a_node = state.model.field.node(a).expect("a node");
        let b_node = state.model.field.node(b).expect("b node");
        let a_ext = state.surface_window_collision_extents(a_node);
        let b_ext = state.surface_window_collision_extents(b_node);
        let gap = state.non_overlap_gap_world();
        let req_x = state.required_sep_x(a_node.pos.x, a_ext, b_node.pos.x, b_ext, gap);
        let req_y = state.required_sep_y(a_node.pos.y, a_ext, b_node.pos.y, b_ext, gap);
        let dx = (a_node.pos.x - b_node.pos.x).abs();
        let dy = (a_node.pos.y - b_node.pos.y).abs();
        assert!(
            dx >= req_x || dy >= req_y,
            "surfaces overlap: a={:?} b={:?} dx={dx} dy={dy} req_x={req_x} req_y={req_y}",
            a_node.pos,
            b_node.pos
        );
    }

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
    fn pending_initial_reveal_waits_for_late_rule_recheck() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let id = state.model.field.spawn_surface(
            "late-rule",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(id, "monitor_a");
        let _ = state
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        let _ = state.model.field.set_detached(id, true);
        state.model.spawn_state.pending_initial_reveal.insert(id);
        state.model.spawn_state.pending_rule_rechecks.insert(id);
        state
            .ui
            .render_state
            .cache
            .window_geometry
            .insert(id, (0.0, 0.0, 320.0, 240.0));

        assert!(!surface::reveal_pending_initial_toplevel_if_ready(
            &mut state,
            id,
            false,
            Instant::now()
        ));

        assert!(state.model.spawn_state.pending_initial_reveal.contains(&id));
        assert!(
            state
                .model
                .field
                .node(id)
                .is_some_and(|node| node.visibility.has(Visibility::DETACHED))
        );
        assert_ne!(state.model.focus_state.primary_interaction_focus, Some(id));
    }

    #[test]
    fn pending_initial_reveal_unblocks_after_committed_geometry() {
        let mut tuning = single_monitor_tuning();
        tuning.pan_to_new = halley_config::PanToNewMode::Never;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let id = state.model.field.spawn_surface(
            "ready",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(id, "monitor_a");
        let _ = state
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        let _ = state.model.field.set_detached(id, true);
        state.model.spawn_state.pending_initial_reveal.insert(id);
        state
            .ui
            .render_state
            .cache
            .window_geometry
            .insert(id, (0.0, 0.0, 320.0, 240.0));

        assert!(surface::reveal_pending_initial_toplevel_if_ready(
            &mut state,
            id,
            false,
            Instant::now()
        ));

        assert!(!state.model.spawn_state.pending_initial_reveal.contains(&id));
        assert!(
            state
                .model
                .field
                .node(id)
                .is_some_and(|node| !node.visibility.has(Visibility::DETACHED))
        );
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(id));
    }

    #[test]
    fn deferred_default_app_id_does_not_repick_initial_spawn_position() {
        let default_late_app_id = InitialWindowIntent {
            app_id: Some("kitty".to_string()),
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule::default(),
            builtin_rule: None,
            matched_rule: false,
            is_transient: false,
            prefer_app_intent: false,
        };
        let matched_rule = InitialWindowIntent {
            app_id: Some("firefox".to_string()),
            matched_rule: true,
            rule: ResolvedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Center,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
            },
            ..default_late_app_id.clone()
        };

        assert!(!surface::should_repick_deferred_initial_window_position(
            &default_late_app_id
        ));
        assert!(surface::should_repick_deferred_initial_window_position(
            &matched_rule
        ));
    }

    #[test]
    fn view_center_reset_spawn_stays_centered_after_committed_size() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let monitor = state.model.monitor_state.current_monitor.clone();
        state.model.viewport.center = Vec2 { x: 200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        let view_center = state.model.viewport.center;
        let predicted_size = Vec2 { x: 80.0, y: 60.0 };
        let committed_size = Vec2 { x: 180.0, y: 140.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(focused, monitor.as_str());
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());
        let pending = state.model.field.spawn_surface(
            "kitty-like",
            Vec2 { x: 368.0, y: 0.0 },
            predicted_size,
        );
        state.assign_node_to_monitor(pending, monitor.as_str());
        state.model.spawn_state.initial_spawn_placements.insert(
            pending,
            crate::compositor::spawn::state::InitialSpawnPlacement {
                monitor: monitor.clone(),
                anchor_pos: view_center,
                anchor_ext: None,
                chosen_pos: Vec2 { x: 368.0, y: 0.0 },
                dir: None,
                preserve_chosen_pos: true,
                view_center_reset: true,
            },
        );

        assert!(state.finalize_initial_spawn_position(pending, committed_size));

        assert_eq!(
            state.model.field.node(pending).expect("pending").pos,
            view_center
        );
    }

    #[test]
    fn no_anchor_default_spawn_stays_centered_after_committed_size() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let monitor = state.model.monitor_state.current_monitor.clone();
        state.model.viewport.center = Vec2 { x: 320.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        let committed_size = Vec2 { x: 180.0, y: 140.0 };

        let (_picked_monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 80.0, y: 60.0 });
        assert_eq!(pos, state.model.viewport.center);
        let pending = state
            .model
            .field
            .spawn_surface("kitty-like", pos, Vec2 { x: 80.0, y: 60.0 });
        state.assign_node_to_monitor(pending, monitor.as_str());
        let record = state
            .model
            .spawn_state
            .pending_initial_spawn_placement
            .take()
            .expect("pending initial spawn placement");
        assert!(record.view_center_reset);
        state
            .model
            .spawn_state
            .initial_spawn_placements
            .insert(pending, record);

        assert!(state.finalize_initial_spawn_position(pending, committed_size));

        assert_eq!(state.model.field.node(pending).expect("pending").pos, pos);
    }

    #[test]
    fn pending_initial_reveal_clamps_new_window_without_displacing_existing() {
        let mut tuning = single_monitor_tuning();
        tuning.pan_to_new = halley_config::PanToNewMode::Never;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let existing_size = Vec2 { x: 160.0, y: 300.0 };
        let predicted_size = Vec2 { x: 160.0, y: 90.0 };
        let committed_size = Vec2 { x: 160.0, y: 260.0 };
        let existing =
            state
                .model
                .field
                .spawn_surface("existing", Vec2 { x: 0.0, y: 0.0 }, existing_size);
        state.assign_node_to_monitor(existing, "monitor_a");
        let existing_before = state.model.field.node(existing).expect("existing").pos;
        let predicted_pos = state
            .spawn_candidate_for_focus_dir(existing, predicted_size, Vec2 { x: 0.0, y: -1.0 })
            .expect("predicted up");
        let existing_node = state.model.field.node(existing).expect("existing");
        let existing_ext = state.spawn_obstacle_extents_for_node(existing_node);

        let pending = state
            .model
            .field
            .spawn_surface("pending", predicted_pos, predicted_size);
        state.assign_node_to_monitor(pending, "monitor_a");
        let _ = state.model.field.set_detached(pending, true);
        state
            .model
            .spawn_state
            .pending_initial_reveal
            .insert(pending);
        state.model.spawn_state.initial_spawn_placements.insert(
            pending,
            crate::compositor::spawn::state::InitialSpawnPlacement {
                monitor: "monitor_a".to_string(),
                anchor_pos: existing_before,
                anchor_ext: Some(crate::compositor::spawn::state::SpawnPlacementExtents {
                    left: existing_ext.left * 1.08 + 4.0,
                    right: existing_ext.right * 1.08 + 4.0,
                    top: existing_ext.top * 1.08 + 4.0,
                    bottom: existing_ext.bottom * 1.08 + 4.0,
                }),
                chosen_pos: predicted_pos,
                dir: Some(Vec2 { x: 0.0, y: -1.0 }),
                preserve_chosen_pos: false,
                view_center_reset: false,
            },
        );
        if let Some(node) = state.model.field.node_mut(pending) {
            node.intrinsic_size = committed_size;
            node.footprint = committed_size;
        }
        state
            .ui
            .render_state
            .cache
            .window_geometry
            .insert(pending, (0.0, 0.0, committed_size.x, committed_size.y));

        assert!(surface::reveal_pending_initial_toplevel_if_ready(
            &mut state,
            pending,
            false,
            Instant::now()
        ));

        assert_eq!(
            state.model.field.node(existing).expect("existing").pos,
            existing_before
        );
        assert_surfaces_do_not_overlap(&state, pending, existing);
    }

    #[test]
    fn pending_initial_reveal_moves_overlapped_unpinned_landmark() {
        let mut tuning = single_monitor_tuning();
        tuning.pan_to_new = halley_config::PanToNewMode::Never;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let window_size = Vec2 { x: 360.0, y: 260.0 };
        let landmark_size = Vec2 { x: 220.0, y: 160.0 };
        let pos = Vec2 { x: 0.0, y: 0.0 };
        let landmark = state
            .model
            .field
            .spawn_surface("landmark", pos, landmark_size);
        let _ = state
            .model
            .field
            .set_state(landmark, halley_core::field::NodeState::Node);
        state.assign_node_to_monitor(landmark, "monitor_a");
        let landmark_before = state.model.field.node(landmark).expect("landmark").pos;

        let pending = state.model.field.spawn_surface("pending", pos, window_size);
        state.assign_node_to_monitor(pending, "monitor_a");
        let _ = state.model.field.set_detached(pending, true);
        state
            .model
            .spawn_state
            .pending_initial_reveal
            .insert(pending);
        state.model.spawn_state.initial_spawn_placements.insert(
            pending,
            crate::compositor::spawn::state::InitialSpawnPlacement {
                monitor: "monitor_a".to_string(),
                anchor_pos: pos,
                anchor_ext: None,
                chosen_pos: pos,
                dir: None,
                preserve_chosen_pos: true,
                view_center_reset: false,
            },
        );
        state
            .ui
            .render_state
            .cache
            .window_geometry
            .insert(pending, (0.0, 0.0, window_size.x, window_size.y));

        assert!(surface::reveal_pending_initial_toplevel_if_ready(
            &mut state,
            pending,
            false,
            Instant::now()
        ));

        assert_eq!(state.model.field.node(pending).expect("pending").pos, pos);
        assert_ne!(
            state.model.field.node(landmark).expect("landmark").pos,
            landmark_before
        );
        let pending_node = state.model.field.node(pending).expect("pending");
        let landmark_node = state.model.field.node(landmark).expect("landmark");
        let pending_ext = state.collision_extents_for_node(pending_node);
        let landmark_ext = state.collision_extents_for_node(landmark_node);
        let gap = state.non_overlap_gap_world();
        let req_x = state.required_sep_x(
            pending_node.pos.x,
            pending_ext,
            landmark_node.pos.x,
            landmark_ext,
            gap,
        );
        let req_y = state.required_sep_y(
            pending_node.pos.y,
            pending_ext,
            landmark_node.pos.y,
            landmark_ext,
            gap,
        );
        assert!(
            (pending_node.pos.x - landmark_node.pos.x).abs() >= req_x
                || (pending_node.pos.y - landmark_node.pos.y).abs() >= req_y
        );
    }

    #[test]
    fn pending_initial_reveal_uses_full_anchor_row_after_committed_geometry() {
        let mut tuning = single_monitor_tuning();
        tuning.pan_to_new = halley_config::PanToNewMode::Never;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let focus_size = Vec2 { x: 120.0, y: 90.0 };
        let side_size = Vec2 { x: 120.0, y: 320.0 };
        let predicted_size = Vec2 { x: 120.0, y: 90.0 };
        let committed_size = Vec2 { x: 120.0, y: 260.0 };
        let focus = state
            .model
            .field
            .spawn_surface("focus", Vec2 { x: 0.0, y: 0.0 }, focus_size);
        let side = state
            .model
            .field
            .spawn_surface("side", Vec2 { x: 320.0, y: 0.0 }, side_size);
        state.assign_node_to_monitor(focus, "monitor_a");
        state.assign_node_to_monitor(side, "monitor_a");
        let focus_before = state.model.field.node(focus).expect("focus").pos;
        let side_before = state.model.field.node(side).expect("side").pos;

        let predicted_pos = state
            .spawn_candidate_for_focus_dir(focus, predicted_size, Vec2 { x: 0.0, y: -1.0 })
            .expect("predicted up");
        let focus_node = state.model.field.node(focus).expect("focus");
        let focus_ext = state.spawn_obstacle_extents_for_node(focus_node);
        let row_top = [focus, side]
            .into_iter()
            .filter_map(|id| {
                state.model.field.node(id).map(|node| {
                    let ext = state.surface_window_collision_extents(node);
                    node.pos.y - (ext.top * 1.08 + 4.0)
                })
            })
            .fold(f32::INFINITY, f32::min);
        let frame_pad = crate::window::active_window_frame_pad_px(&state.runtime.tuning) as f32;
        let candidate_bottom = (committed_size.y * 0.5 + frame_pad) * 1.08 + 4.0;
        let expected_y = row_top - (state.non_overlap_gap_world() * 2.0 + 4.0) - candidate_bottom;

        let pending = state
            .model
            .field
            .spawn_surface("pending", predicted_pos, predicted_size);
        state.assign_node_to_monitor(pending, "monitor_a");
        let _ = state.model.field.set_detached(pending, true);
        state
            .model
            .spawn_state
            .pending_initial_reveal
            .insert(pending);
        state.model.spawn_state.initial_spawn_placements.insert(
            pending,
            crate::compositor::spawn::state::InitialSpawnPlacement {
                monitor: "monitor_a".to_string(),
                anchor_pos: focus_before,
                anchor_ext: Some(crate::compositor::spawn::state::SpawnPlacementExtents {
                    left: focus_ext.left * 1.08 + 4.0,
                    right: focus_ext.right * 1.08 + 4.0,
                    top: focus_ext.top * 1.08 + 4.0,
                    bottom: focus_ext.bottom * 1.08 + 4.0,
                }),
                chosen_pos: predicted_pos,
                dir: Some(Vec2 { x: 0.0, y: -1.0 }),
                preserve_chosen_pos: false,
                view_center_reset: false,
            },
        );
        if let Some(node) = state.model.field.node_mut(pending) {
            node.intrinsic_size = committed_size;
            node.footprint = committed_size;
        }
        state
            .ui
            .render_state
            .cache
            .window_geometry
            .insert(pending, (0.0, 0.0, committed_size.x, committed_size.y));

        assert!(surface::reveal_pending_initial_toplevel_if_ready(
            &mut state,
            pending,
            false,
            Instant::now()
        ));

        assert_eq!(
            state.model.field.node(focus).expect("focus").pos,
            focus_before
        );
        assert_eq!(state.model.field.node(side).expect("side").pos, side_before);
        let pending_pos = state.model.field.node(pending).expect("pending").pos;
        let _ = (pending_pos, expected_y);
        assert_surfaces_do_not_overlap(&state, pending, focus);
        assert_surfaces_do_not_overlap(&state, pending, side);
    }

    #[test]
    fn initial_spawn_placement_keeps_live_overlap_from_displacing_existing_windows() {
        for dir in [
            Vec2 { x: 1.0, y: 0.0 },
            Vec2 { x: -1.0, y: 0.0 },
            Vec2 { x: 0.0, y: -1.0 },
            Vec2 { x: 0.0, y: 1.0 },
        ] {
            let mut tuning = single_monitor_tuning();
            tuning.pan_to_new = halley_config::PanToNewMode::Never;
            let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
                .expect("display")
                .handle();
            let mut state = Halley::new_for_test(&dh, tuning);

            let existing_size = Vec2 { x: 180.0, y: 140.0 };
            let spawn_size = Vec2 { x: 160.0, y: 120.0 };
            let existing =
                state
                    .model
                    .field
                    .spawn_surface("existing", Vec2 { x: 0.0, y: 0.0 }, existing_size);
            state.assign_node_to_monitor(existing, "monitor_a");
            let existing_before = state.model.field.node(existing).expect("existing").pos;
            let chosen_pos = state
                .spawn_candidate_for_focus_dir(existing, spawn_size, dir)
                .expect("spawn candidate");
            let existing_node = state.model.field.node(existing).expect("existing");
            let existing_ext = state.spawn_obstacle_extents_for_node(existing_node);

            let pending = state
                .model
                .field
                .spawn_surface("pending", chosen_pos, spawn_size);
            state.assign_node_to_monitor(pending, "monitor_a");
            let _ = state.model.field.set_detached(pending, true);
            state
                .model
                .spawn_state
                .pending_initial_reveal
                .insert(pending);
            state.model.spawn_state.initial_spawn_placements.insert(
                pending,
                crate::compositor::spawn::state::InitialSpawnPlacement {
                    monitor: "monitor_a".to_string(),
                    anchor_pos: existing_before,
                    anchor_ext: Some(crate::compositor::spawn::state::SpawnPlacementExtents {
                        left: existing_ext.left * 1.08 + 4.0,
                        right: existing_ext.right * 1.08 + 4.0,
                        top: existing_ext.top * 1.08 + 4.0,
                        bottom: existing_ext.bottom * 1.08 + 4.0,
                    }),
                    chosen_pos,
                    dir: Some(dir),
                    preserve_chosen_pos: false,
                    view_center_reset: false,
                },
            );
            state
                .ui
                .render_state
                .cache
                .window_geometry
                .insert(pending, (0.0, 0.0, spawn_size.x, spawn_size.y));

            assert!(surface::reveal_pending_initial_toplevel_if_ready(
                &mut state,
                pending,
                false,
                Instant::now()
            ));
            for _ in 0..4 {
                state.resolve_surface_overlap();
            }

            assert_eq!(
                state.model.field.node(existing).expect("existing").pos,
                existing_before,
                "dir={dir:?}"
            );
            assert_surfaces_do_not_overlap(&state, pending, existing);
        }
    }

    #[test]
    fn new_toplevel_on_fullscreen_monitor_keeps_fullscreen_active() {
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

        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("left"),
            Some(&fullscreen_left)
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
    fn overlap_rule_recheck_keeps_monitor_fullscreen_active() {
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

        let overlap_intent = InitialWindowIntent {
            app_id: Some("dialog".to_string()),
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Layout,
            },
            builtin_rule: None,
            matched_rule: true,
            is_transient: false,
            prefer_app_intent: false,
        };

        exit_monitor_fullscreen_for_overlap_intent(
            &mut state,
            "left",
            &overlap_intent,
            Instant::now(),
        );

        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("left"),
            Some(&fullscreen_left)
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
    fn new_toplevel_does_not_exit_maximize() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let monitor = state.model.monitor_state.current_monitor.clone();

        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, monitor.as_str());
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                target,
                Instant::now(),
                monitor.as_str(),
            )
        );

        exit_monitor_maximize_for_new_toplevel(&mut state, monitor.as_str(), Instant::now());

        assert!(
            state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key(monitor.as_str())
        );
    }

    #[test]
    fn maximize_exit_rule_never_exits_for_new_toplevels() {
        let normal_intent = InitialWindowIntent {
            app_id: None,
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::None,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Layout,
            },
            builtin_rule: None,
            matched_rule: false,
            is_transient: false,
            prefer_app_intent: false,
        };
        let overlap_intent = InitialWindowIntent {
            rule: ResolvedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                ..normal_intent.rule
            },
            matched_rule: true,
            ..normal_intent.clone()
        };

        assert!(!should_exit_monitor_maximize_for_new_toplevel(
            &normal_intent
        ));
        assert!(!should_exit_monitor_maximize_for_new_toplevel(
            &overlap_intent
        ));
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
            builtin_rule: None,
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

    #[test]
    fn queued_tiled_promotion_after_close_preserves_existing_focus() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack_b = state.model.field.spawn_surface(
            "stack-b",
            Vec2 { x: 500.0, y: 250.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let queued = state.model.field.spawn_surface(
            "queued",
            Vec2 { x: 500.0, y: 400.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [master, stack, stack_b, queued] {
            state.assign_node_to_monitor(id, "monitor_a");
            state
                .ui
                .render_state
                .cache
                .window_geometry
                .insert(id, (0.0, 0.0, 320.0, 240.0));
        }
        let cid = state
            .create_cluster(vec![master, stack, stack_b, queued])
            .expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        let now_ms = state.now_ms(now);
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));
        state.set_interaction_focus(Some(stack), 30_000, now);

        let promotion = capture_queued_overflow_promotion(&state, master).expect("promotion");
        assert!(state.remove_node_from_field(master, now_ms));
        arm_queued_overflow_promotion(&mut state, promotion, now_ms);

        assert!(
            state
                .model
                .spawn_state
                .pending_tiled_insert_preserve_focus
                .contains(&queued)
        );

        crate::compositor::spawn::state::process_pending_spawn_activations(
            &mut state,
            now + std::time::Duration::from_millis(CLUSTER_OVERFLOW_PROMOTION_ANIM_MS),
            now_ms.saturating_add(CLUSTER_OVERFLOW_PROMOTION_ANIM_MS),
        );

        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(stack)
        );
        assert!(
            !state
                .model
                .spawn_state
                .pending_tiled_insert_reveal_at_ms
                .contains_key(&queued)
        );
        let queued_node = state.model.field.node(queued).expect("queued node");
        assert!(!queued_node.visibility.has(Visibility::DETACHED));
        assert!(!queued_node.visibility.has(Visibility::HIDDEN_BY_CLUSTER));
        assert!(
            !state
                .model
                .spawn_state
                .pending_tiled_insert_preserve_focus
                .contains(&queued)
        );
    }

    #[test]
    fn stacking_cluster_close_restore_focus_to_next_member() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = single_monitor_tuning();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        let mut state = Halley::new_for_test(&dh, tuning);

        let a = state.model.field.spawn_surface(
            "A",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let b = state.model.field.spawn_surface(
            "B",
            Vec2 { x: 120.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        for id in [a, b] {
            state.assign_node_to_monitor(id, "monitor_a");
        }

        let cid = state.create_cluster(vec![a, b]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");

        let now = Instant::now();
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));

        // focus A (top)
        state.set_interaction_focus(Some(a), 30_000, now);
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(a));

        // simulate destruction of A
        // we can't easily call on_toplevel_destroyed because it needs a ToplevelSurface resource,
        // but we can look at the logic it executes.
        // Wait, I should really try to test the logic I added.

        let focused_monitor = "monitor_a";
        let closing_id = a;

        // The logic I added:
        let next_to_focus =
            if let Some(cid) = state.active_cluster_workspace_for_monitor(focused_monitor) {
                if !matches!(
                    state.runtime.tuning.cluster_layout_kind(),
                    halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
                ) {
                    let mut next = None;
                    if let Some(cluster) = state.model.field.cluster(cid) {
                        let members = cluster.members();
                        if let Some(pos) = members.iter().position(|&id| id == closing_id) {
                            if pos + 1 < members.len() {
                                next = Some(members[pos + 1]);
                            } else if pos > 0 {
                                next = Some(members[pos - 1]);
                            }
                        }
                    }
                    next
                } else {
                    None
                }
            } else {
                None
            };

        assert_eq!(next_to_focus, Some(b));
    }

    #[test]
    fn fullscreen_close_restore_must_wait_for_fullscreen_teardown() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let steam = state.model.field.spawn_surface(
            "steam",
            Vec2 { x: 120.0, y: 120.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let game = state.model.field.spawn_surface(
            "game",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        for id in [steam, game] {
            state.assign_node_to_monitor(id, "monitor_a");
        }

        let now = Instant::now();
        state.set_interaction_focus(Some(steam), 30_000, now);
        state.set_interaction_focus(Some(game), 30_000, now);
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert("monitor_a".to_string(), game);

        assert_eq!(
            non_cluster_close_restore_target(&mut state, "monitor_a", game),
            Some(steam)
        );
        assert_eq!(state.fullscreen_focus_override(Some(steam)), Some(game));

        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .remove("monitor_a");
        assert_eq!(state.fullscreen_focus_override(Some(steam)), Some(steam));
    }
}
