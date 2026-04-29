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

    if had_keyboard_focus
        && st.runtime.tuning.close_restore_focus
        && let (Some(closing_id), Some(focused_monitor)) = (closing_id, focused_monitor.as_deref())
    {
        let now = Instant::now();
        let suppress_restore_pan =
            st.node_has_overlap_policy(closing_id) || st.is_fullscreen_active(closing_id);
        if let Some(cid) = st.active_cluster_workspace_for_monitor(focused_monitor) {
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
        } else if let Some(previous) =
            st.previous_window_from_trail_on_close(focused_monitor, closing_id)
        {
            let _ = st.restore_focus_to_node_after_close(
                focused_monitor,
                previous,
                now,
                suppress_restore_pan,
            );
        } else if let Some(fallback) = st
            .last_focused_surface_node_for_monitor(focused_monitor)
            .filter(|&id| id != closing_id)
            .or_else(|| {
                st.last_focused_surface_node()
                    .filter(|&id| id != closing_id)
            })
        {
            let _ = st.restore_focus_to_node_after_close(
                focused_monitor,
                fallback,
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
    fn maximize_exit_for_new_toplevel_restores_focus_anchor() {
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
            !state
                .model
                .workspace_state
                .maximize_sessions
                .contains_key(monitor.as_str())
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
        assert_eq!(state.current_spawn_focus(monitor.as_str()).0, Some(target));
        assert_eq!(
            state.current_spawn_focus(monitor.as_str()).1,
            Vec2 { x: 100.0, y: 100.0 }
        );
    }

    #[test]
    fn maximize_exit_rule_matches_only_non_overlap_toplevels() {
        let normal_intent = InitialWindowIntent {
            app_id: None,
            title: None,
            parent_node: None,
            rule: ResolvedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::None,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Layout,
            },
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

        assert!(should_exit_monitor_maximize_for_new_toplevel(
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
            .model
            .field
            .create_cluster(vec![master, stack, stack_b, queued])
            .expect("cluster");
        let core = state.model.field.collapse_cluster(cid).expect("core");
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

        let cid = state
            .model
            .field
            .create_cluster(vec![a, b])
            .expect("cluster");
        let core = state.model.field.collapse_cluster(cid).expect("core");
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
}
