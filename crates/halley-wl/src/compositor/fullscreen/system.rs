use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;

use super::*;

/// Minimum window dimensions used as a floor for the fullscreen grow/shrink
/// animation and for the target/restore sizes requested from clients, so a
/// degenerate (zero/tiny) size never produces an invisible or collapsed window.
const FULLSCREEN_MIN_W: f32 = 96.0;
const FULLSCREEN_MIN_H: f32 = 72.0;

pub(crate) fn on_seat_focus_changed(st: &mut Halley, focused: Option<&WlSurface>, now: Instant) {
    let _ = (st, focused, now);
}

#[cfg(test)]
fn focused_node_preserves_fullscreen_lock(
    st: &Halley,
    focused_node_id: Option<NodeId>,
    monitor: &str,
) -> bool {
    focused_node_id.is_some_and(|focused_node| {
        st.node_draws_above_fullscreen_on_monitor(focused_node, monitor)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::spawn::state::AppliedInitialWindowRule;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
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

    fn cluster_tuning(layout: halley_config::ClusterDefaultLayout) -> halley_config::RuntimeTuning {
        let mut tuning = single_monitor_tuning();
        tuning.cluster_default_layout = layout;
        tuning
    }

    fn active_cluster_monitor(state: &mut Halley) -> (String, halley_core::cluster::ClusterId) {
        let monitor = state.model.monitor_state.current_monitor.clone();
        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 120.0, y: 120.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 400.0, y: 120.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        state.assign_node_to_monitor(a, monitor.as_str());
        state.assign_node_to_monitor(b, monitor.as_str());
        let cid = state.create_cluster(vec![a, b]).expect("cluster");
        state
            .model
            .cluster_state
            .active_cluster_workspaces
            .insert(monitor.clone(), cid);
        (monitor, cid)
    }

    fn spawn_member_candidate(state: &mut Halley, monitor: &str) -> NodeId {
        let node = state.model.field.spawn_surface(
            "game",
            Vec2 { x: 50.0, y: 50.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        state.assign_node_to_monitor(node, monitor);
        node
    }

    #[test]
    fn fullscreen_target_joins_active_tiled_cluster_first() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(
            &dh,
            cluster_tuning(halley_config::ClusterDefaultLayout::Tiling),
        );
        let (monitor, cid) = active_cluster_monitor(&mut state);
        let game = spawn_member_candidate(&mut state, monitor.as_str());
        assert!(
            state
                .model
                .field
                .cluster_id_for_member_public(game)
                .is_none()
        );

        ensure_cluster_membership_before_fullscreen(&mut state, game, Instant::now());

        assert_eq!(
            state.model.field.cluster_id_for_member_public(game),
            Some(cid),
            "a window fullscreening into an active tiled cluster must join it first"
        );
    }

    #[test]
    fn fullscreen_target_joins_active_stacking_cluster_first() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(
            &dh,
            cluster_tuning(halley_config::ClusterDefaultLayout::Stacking),
        );
        let (monitor, cid) = active_cluster_monitor(&mut state);
        let game = spawn_member_candidate(&mut state, monitor.as_str());

        ensure_cluster_membership_before_fullscreen(&mut state, game, Instant::now());

        assert_eq!(
            state.model.field.cluster_id_for_member_public(game),
            Some(cid),
            "a window fullscreening into an active stacking cluster must join it first"
        );
    }

    #[test]
    fn fullscreen_without_active_cluster_does_not_join() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(
            &dh,
            cluster_tuning(halley_config::ClusterDefaultLayout::Tiling),
        );
        // A cluster exists but is NOT the active workspace on this monitor.
        let monitor = state.model.monitor_state.current_monitor.clone();
        let game = spawn_member_candidate(&mut state, monitor.as_str());

        ensure_cluster_membership_before_fullscreen(&mut state, game, Instant::now());

        assert!(
            state
                .model
                .field
                .cluster_id_for_member_public(game)
                .is_none(),
            "with no active cluster workspace there is nothing to join - ordinary fullscreen"
        );
    }

    #[test]
    fn overlap_policy_focus_preserves_same_monitor_fullscreen_lock() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let overlap = state.model.field.spawn_surface(
            "overlap",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );
        for id in [fullscreen, overlap] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor.clone(), fullscreen);
        state.model.spawn_state.applied_window_rules.insert(
            overlap,
            AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                opacity: 1.0,
                parent_node: None,
                suppress_reveal_pan: true,
                builtin_rule: None,
            },
        );

        assert!(focused_node_preserves_fullscreen_lock(
            &state,
            Some(overlap),
            monitor.as_str()
        ));
        assert!(!focused_node_preserves_fullscreen_lock(
            &state,
            Some(fullscreen),
            monitor.as_str()
        ));
    }

    #[test]
    fn overlap_policy_focus_does_not_preserve_fullscreen_lock_in_stacking_layout() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let overlap = state.model.field.spawn_surface(
            "overlap",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );
        for id in [fullscreen, overlap] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor.clone(), fullscreen);
        state.model.spawn_state.applied_window_rules.insert(
            overlap,
            AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                opacity: 1.0,
                parent_node: None,
                suppress_reveal_pan: true,
                builtin_rule: None,
            },
        );

        assert!(!focused_node_preserves_fullscreen_lock(
            &state,
            Some(overlap),
            monitor.as_str()
        ));
    }

    #[test]
    fn live_overlap_pauses_during_fullscreen_motion() {
        let mut tuning = single_monitor_tuning();
        tuning.physics_enabled = false;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 200.0, y: 200.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 220.0, y: 220.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(a, "monitor_a");
        state.assign_node_to_monitor(b, "monitor_a");
        let a_before = state.model.field.node(a).expect("a").pos;
        let b_before = state.model.field.node(b).expect("b").pos;
        let now = Instant::now();
        state.model.fullscreen_state.fullscreen_motion.insert(
            a,
            crate::compositor::fullscreen::state::FullscreenMotion {
                from: a_before,
                to: Vec2 { x: 400.0, y: 300.0 },
                start_ms: state.now_ms(now),
                duration_ms: 320,
            },
        );

        crate::frame_loop::tick_live_overlap(&mut state);

        assert_eq!(state.model.field.node(a).expect("a").pos, a_before);
        assert_eq!(state.model.field.node(b).expect("b").pos, b_before);
    }

    #[test]
    fn fullscreen_does_not_displace_bystanders() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = state.model.field.spawn_surface(
            "bystander",
            Vec2 { x: 520.0, y: 280.0 },
            Vec2 { x: 220.0, y: 160.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        state.assign_node_to_monitor(bystander, "monitor_a");
        let _ = state.model.field.set_pinned(bystander, true);

        let fullscreen_pos = state.model.field.node(fullscreen).expect("fullscreen").pos;
        let bystander_pos = state.model.field.node(bystander).expect("bystander").pos;
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        assert_eq!(
            state.model.field.node(fullscreen).expect("fullscreen").pos,
            fullscreen_pos
        );
        assert_eq!(
            state
                .model
                .field
                .node(fullscreen)
                .expect("fullscreen")
                .intrinsic_size,
            Vec2 { x: 320.0, y: 240.0 }
        );
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        let unchanged_bystander = state.model.field.node(bystander).expect("bystander");
        assert_eq!(unchanged_bystander.pos, bystander_pos);
        assert!(unchanged_bystander.pinned);

        state.exit_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(300));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(700));

        assert_eq!(
            state.model.field.node(fullscreen).expect("fullscreen").pos,
            fullscreen_pos
        );
        let restored_bystander = state.model.field.node(bystander).expect("bystander");
        assert_eq!(restored_bystander.pos, bystander_pos);
        assert!(restored_bystander.pinned);
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_restore
                .contains_key(&bystander)
        );
    }

    #[test]
    fn client_request_on_user_fullscreen_preserves_user_origin() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let window = state.model.field.spawn_surface(
            "window",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(window, "monitor_a");
        let now = Instant::now();

        state.enter_user_fullscreen(window, None, now);
        assert_eq!(
            state.model.fullscreen_state.fullscreen_origin.get(&window),
            Some(&crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind)
        );

        state.enter_xdg_fullscreen(window, None, now + std::time::Duration::from_millis(1));

        assert!(state.is_fullscreen_active(window));
        assert_eq!(
            state.model.fullscreen_state.fullscreen_origin.get(&window),
            Some(&crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind)
        );
    }

    #[test]
    fn fullscreen_enter_starts_visual_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);

        let anim = state
            .model
            .fullscreen_state
            .fullscreen_scale_anim
            .get(&fullscreen)
            .expect("fullscreen animation");
        assert_eq!(anim.monitor, "monitor_a");
        assert_eq!(anim.from_pos, Vec2 { x: 140.0, y: 150.0 });
        assert_eq!(anim.from_size, Vec2 { x: 320.0, y: 240.0 });
        // The window grows in place (centred on itself); the camera recentres onto
        // it, so the fill target is the window's own position, not the monitor centre.
        assert_eq!(anim.to_pos, Vec2 { x: 140.0, y: 150.0 });
        assert_eq!(anim.to_size, Vec2 { x: 800.0, y: 600.0 });
        // The camera is retargeted to centre on the window at zoom 1.0.
        assert_eq!(
            state.model.camera_target_center,
            Vec2 { x: 140.0, y: 150.0 }
        );
        assert_eq!(
            state.model.camera_target_view_size,
            Vec2 { x: 800.0, y: 600.0 }
        );
    }

    #[test]
    fn fullscreen_enter_respects_disabled_visual_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.fullscreen.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");

        state.enter_xdg_fullscreen(fullscreen, None, Instant::now());

        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_scale_anim
                .contains_key(&fullscreen)
        );
    }

    #[test]
    fn fullscreen_visual_interpolates_then_settles_on_output_rect() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        let (start_pos, start_size) =
            fullscreen_visual_for_node_on_current_monitor_at(&state, fullscreen, now)
                .expect("start visual");
        assert_eq!(start_pos, Vec2 { x: 140.0, y: 150.0 });
        assert_eq!(start_size, Vec2 { x: 320.0, y: 240.0 });

        let mid = now + std::time::Duration::from_millis(120);
        let (mid_pos, mid_size) =
            fullscreen_visual_for_node_on_current_monitor_at(&state, fullscreen, mid)
                .expect("mid visual");
        // Grows in place: the centre stays put while only the size eases up.
        assert_eq!(mid_pos, Vec2 { x: 140.0, y: 150.0 });
        assert!(mid_size.x > 320.0 && mid_size.x < 800.0);

        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        let (end_pos, end_size) = fullscreen_visual_for_node_on_current_monitor_at(
            &state,
            fullscreen,
            now + std::time::Duration::from_millis(260),
        )
        .expect("end visual");
        assert_eq!(end_pos, Vec2 { x: 140.0, y: 150.0 });
        assert_eq!(end_size, Vec2 { x: 800.0, y: 600.0 });
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_scale_anim
                .contains_key(&fullscreen)
        );
    }

    #[test]
    fn fullscreen_exit_starts_reverse_visual_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        // Settle the enter animation so the window sits at the output rect.
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));

        let exit_at = now + std::time::Duration::from_millis(300);
        state.exit_xdg_fullscreen(fullscreen, exit_at);

        // Node has left active fullscreen immediately.
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key("monitor_a")
        );

        // A reverse anim shrinks from the output rect back to the restored geometry.
        let anim = state
            .model
            .fullscreen_state
            .fullscreen_scale_anim
            .get(&fullscreen)
            .expect("exit animation");
        assert_eq!(anim.monitor, "monitor_a");
        // Shrinks in place at the window's own centre (the grow did the same).
        assert_eq!(anim.from_pos, Vec2 { x: 140.0, y: 150.0 });
        assert_eq!(anim.from_size, Vec2 { x: 800.0, y: 600.0 });
        assert_eq!(anim.to_pos, Vec2 { x: 140.0, y: 150.0 });
        assert_eq!(anim.to_size, Vec2 { x: 320.0, y: 240.0 });

        // The visual override still applies mid-exit even though the node is not active.
        let mid = exit_at + std::time::Duration::from_millis(120);
        let (mid_pos, mid_size) =
            fullscreen_visual_for_node_on_current_monitor_at(&state, fullscreen, mid)
                .expect("mid exit visual");
        assert_eq!(mid_pos, Vec2 { x: 140.0, y: 150.0 });
        assert!(mid_size.x > 320.0 && mid_size.x < 800.0);

        // Once it expires the override is gone and the window rests at restored geometry.
        let done = exit_at + std::time::Duration::from_millis(260);
        state.tick_fullscreen_motion(done);
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_scale_anim
                .contains_key(&fullscreen)
        );
        assert!(
            fullscreen_visual_for_node_on_current_monitor_at(&state, fullscreen, done).is_none()
        );
    }

    #[test]
    fn fullscreen_exit_respects_disabled_visual_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.fullscreen.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.exit_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(40));

        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_scale_anim
                .contains_key(&fullscreen)
        );
    }

    #[test]
    fn fullscreen_exit_restores_pre_fullscreen_zoom() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");

        // Zoom in before fullscreen: a smaller view size than the viewport.
        let zoomed = Vec2 {
            x: state.model.viewport.size.x * 0.5,
            y: state.model.viewport.size.y * 0.5,
        };
        state.model.zoom_ref_size = zoomed;
        state.model.camera_target_view_size = zoomed;

        let now = Instant::now();
        state.enter_xdg_fullscreen(fullscreen, None, now);
        // Fullscreen reset the live zoom to the viewport (1.0).
        assert_eq!(
            state.model.camera_target_view_size,
            state.model.viewport.size
        );

        state.exit_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(300));
        // Exiting eases the camera back toward the pre-fullscreen zoom, not 1.0.
        assert_eq!(state.model.camera_target_view_size, zoomed);
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_camera_restore
                .contains_key("monitor_a")
        );
    }

    #[test]
    fn fullscreen_soft_suspend_does_not_animate() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());
        let fullscreen = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        state.soft_suspend_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(300));

        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_scale_anim
                .contains_key(&fullscreen)
        );
    }

    #[test]
    fn fullscreen_enter_clears_stale_bystander_restore_without_motion() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let landmark = state.model.field.spawn_surface(
            "landmark",
            Vec2 { x: 520.0, y: 280.0 },
            Vec2 { x: 220.0, y: 160.0 },
        );
        let _ = state
            .model
            .field
            .set_state(landmark, halley_core::field::NodeState::Node);
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        state.assign_node_to_monitor(landmark, "monitor_a");
        let landmark_pos = state.model.field.node(landmark).expect("landmark").pos;

        state.model.fullscreen_state.fullscreen_restore.insert(
            landmark,
            crate::compositor::fullscreen::state::FullscreenSessionEntry {
                pos: Vec2 {
                    x: -900.0,
                    y: -900.0,
                },
                size: Vec2 { x: 220.0, y: 160.0 },
                viewport_center: Vec2 { x: 400.0, y: 300.0 },
                intrinsic_size: Vec2 { x: 220.0, y: 160.0 },
                bbox_loc: None,
                window_geometry: None,
                pinned: false,
            },
        );

        state.enter_xdg_fullscreen(fullscreen, None, Instant::now());

        assert_eq!(
            state.model.field.node(landmark).expect("landmark").pos,
            landmark_pos
        );
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_restore
                .contains_key(&landmark)
        );
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_motion
                .contains_key(&landmark)
        );
    }

    #[test]
    fn fullscreen_exit_clears_stale_bystander_restore_without_motion() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let landmark = state.model.field.spawn_surface(
            "landmark",
            Vec2 { x: 520.0, y: 280.0 },
            Vec2 { x: 220.0, y: 160.0 },
        );
        let _ = state
            .model
            .field
            .set_state(landmark, halley_core::field::NodeState::Node);
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        state.assign_node_to_monitor(landmark, "monitor_a");
        let landmark_pos = state.model.field.node(landmark).expect("landmark").pos;
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.model.fullscreen_state.fullscreen_restore.insert(
            landmark,
            crate::compositor::fullscreen::state::FullscreenSessionEntry {
                pos: Vec2 {
                    x: -900.0,
                    y: -900.0,
                },
                size: Vec2 { x: 220.0, y: 160.0 },
                viewport_center: Vec2 { x: 400.0, y: 300.0 },
                intrinsic_size: Vec2 { x: 220.0, y: 160.0 },
                bbox_loc: None,
                window_geometry: None,
                pinned: false,
            },
        );

        state.exit_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(300));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(700));

        assert_eq!(
            state.model.field.node(landmark).expect("landmark").pos,
            landmark_pos
        );
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_restore
                .contains_key(&landmark)
        );
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_motion
                .contains_key(&landmark)
        );
    }

    #[test]
    fn soft_suspend_resumes_existing_fullscreen_session() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = state.model.field.spawn_surface(
            "bystander",
            Vec2 { x: 520.0, y: 280.0 },
            Vec2 { x: 220.0, y: 160.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        state.assign_node_to_monitor(bystander, "monitor_a");

        let now = Instant::now();
        state.enter_xdg_fullscreen(fullscreen, None, now);
        let restore_count = state.model.fullscreen_state.fullscreen_restore.len();
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("monitor_a"),
            Some(&fullscreen)
        );

        state.soft_suspend_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(40));
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key("monitor_a")
        );
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .get("monitor_a"),
            Some(&fullscreen)
        );
        assert_eq!(
            state.model.fullscreen_state.fullscreen_restore.len(),
            restore_count
        );

        state.enter_xdg_fullscreen(fullscreen, None, now + std::time::Duration::from_millis(80));
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("monitor_a"),
            Some(&fullscreen)
        );
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .contains_key("monitor_a")
        );
        assert_eq!(
            state.model.fullscreen_state.fullscreen_restore.len(),
            restore_count
        );
    }

    #[test]
    fn suppressed_focus_does_not_resume_soft_suspended_fullscreen() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");

        let now = Instant::now();
        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.soft_suspend_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(40));
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .get("monitor_a"),
            Some(&fullscreen)
        );

        // Hover-focus (suppressed): focusing the suspended node must NOT resume it.
        state
            .input
            .interaction_state
            .suppress_fullscreen_resume_on_focus = true;
        state.apply_wayland_focus_state(Some(fullscreen));
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key("monitor_a"),
            "suppressed focus must not resume fullscreen"
        );
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .get("monitor_a"),
            Some(&fullscreen)
        );

        // Deliberate focus (alt+tab / apogee path): resumes the session.
        state
            .input
            .interaction_state
            .suppress_fullscreen_resume_on_focus = false;
        state.apply_wayland_focus_state(Some(fullscreen));
        assert_eq!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .get("monitor_a"),
            Some(&fullscreen),
            "deliberate focus must resume fullscreen"
        );
    }

    #[test]
    fn fullscreen_keybind_works_for_stacking_cluster_member() {
        use halley_config::ClusterDefaultLayout;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.cluster_default_layout = ClusterDefaultLayout::Stacking;
        let mut state = Halley::new_for_test(&dh, tuning);

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let card = state.model.field.spawn_surface(
            "card",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(card, "monitor_a");
        let cid = state.create_cluster(vec![master, card]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.set_interaction_focus(Some(master), 30_000, now);
        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(
            state.is_fullscreen_active(master),
            "master should be fullscreen in a stacking cluster"
        );
        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(!state.is_fullscreen_session_node(master));
    }

    #[test]
    fn fullscreen_action_press_toggles_for_cluster_workspace_member() {
        // Exercises the exact runtime dispatch entry point
        // (`apply_compositor_action_press` with ToggleFullscreen) in a cluster
        // workspace, relying only on the focus that entering the workspace sets
        // — no manual `set_interaction_focus`. This is the closest a unit test
        // gets to the Mod+F keybind at runtime.
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        use halley_config::CompositorBindingAction;
        let applied = crate::input::keyboard::bindings::apply_compositor_action_press(
            &mut state,
            CompositorBindingAction::ToggleFullscreen,
            "",
            "",
        );
        assert!(
            applied,
            "ToggleFullscreen action should apply in a cluster workspace"
        );
        assert!(
            state.is_fullscreen_active(master),
            "master should be fullscreen after the ToggleFullscreen action"
        );

        let applied_exit = crate::input::keyboard::bindings::apply_compositor_action_press(
            &mut state,
            CompositorBindingAction::ToggleFullscreen,
            "",
            "",
        );
        assert!(applied_exit);
        assert!(
            !state.is_fullscreen_session_node(master),
            "second ToggleFullscreen should exit fullscreen"
        );
    }

    #[test]
    fn fullscreen_keybind_toggles_for_cluster_workspace_member() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.set_interaction_focus(Some(master), 30_000, now);

        // Enter fullscreen via the keybind path.
        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(
            state.is_fullscreen_active(master),
            "master should be fullscreen after keybind"
        );

        // Exit fullscreen via the keybind path.
        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(
            !state.is_fullscreen_session_node(master),
            "master should exit fullscreen"
        );
    }

    #[test]
    fn client_fullscreen_rerequest_after_user_exit_stays_windowed_in_cluster() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.set_interaction_focus(Some(master), 30_000, now);
        state.enter_xdg_fullscreen(master, None, now);
        assert!(state.is_fullscreen_active(master));

        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(!state.is_fullscreen_session_node(master));
        assert_eq!(
            state.model.field.cluster_id_for_member_public(master),
            Some(cid)
        );

        state.enter_xdg_fullscreen(master, None, now + std::time::Duration::from_millis(40));
        assert!(
            !state.is_fullscreen_session_node(master),
            "client fullscreen re-request should be suppressed after user exit"
        );
        assert_eq!(
            state.model.field.cluster_id_for_member_public(master),
            Some(cid)
        );

        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(
            state.is_fullscreen_active(master),
            "the user keybind should still be able to re-enter fullscreen"
        );
        assert_eq!(
            state.model.fullscreen_state.fullscreen_origin.get(&master),
            Some(&crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind)
        );

        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        assert!(
            !state.is_fullscreen_session_node(master),
            "second user keybind exit should leave the game windowed"
        );

        state.enter_xdg_fullscreen(master, None, now + std::time::Duration::from_millis(80));
        assert!(
            !state.is_fullscreen_session_node(master),
            "client fullscreen re-request should also be suppressed after exiting a user-owned fullscreen"
        );
    }

    #[test]
    fn fullscreen_keybind_exits_after_soft_suspend() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let node = state.model.field.spawn_surface(
            "win",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(node, "monitor_a");

        let now = Instant::now();
        state.set_interaction_focus(Some(node), 30_000, now);
        state.enter_xdg_fullscreen(node, None, now);
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        assert!(state.is_fullscreen_active(node));

        // Soft-suspend (alt-tab away). The node leaves the active map, so the old
        // `is_fullscreen_active` predicate could no longer see it and the keybind
        // re-entered, wedging a corrupt second session. The session predicate must
        // still see it and exit cleanly.
        state.soft_suspend_xdg_fullscreen(node, now + std::time::Duration::from_millis(300));
        assert!(!state.is_fullscreen_active(node));
        assert!(state.is_fullscreen_session_node(node));

        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(760));

        assert!(!state.is_fullscreen_session_node(node));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .is_empty()
        );
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .is_empty()
        );
        assert!(state.model.fullscreen_state.fullscreen_restore.is_empty());
    }

    #[test]
    fn fullscreen_on_cluster_member_hides_siblings_and_restores_on_exit() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.enter_xdg_fullscreen(master, None, now);

        // The fullscreen member's siblings are recorded as hidden; the fullscreen
        // member itself is not.
        let hidden = state
            .model
            .fullscreen_state
            .fullscreen_hidden_cluster_siblings
            .get(&master)
            .expect("siblings should be hidden while a member is fullscreen");
        assert!(hidden.contains(&stack));
        assert!(cluster_sibling_hidden_for_fullscreen(&state, stack));
        assert!(!cluster_sibling_hidden_for_fullscreen(&state, master));

        // Exiting clears the hidden set and leaves the workspace active.
        state.exit_xdg_fullscreen(master, now + std::time::Duration::from_millis(40));
        assert!(!cluster_sibling_hidden_for_fullscreen(&state, stack));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_hidden_cluster_siblings
                .is_empty()
        );
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );
    }

    #[test]
    fn drop_fullscreen_surface_restores_camera_and_cluster_workspace() {
        // Regression: closing a fullscreened cluster member previously left the
        // monitor camera anchored on the deleted node (because `drop_fullscreen_surface`
        // skipped the camera restore + cluster workspace restore that the mod+f exit
        // path performs), so the subsequent re-layout projected survivors offscreen.
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        // Zoom in before fullscreen so we can detect the camera restore.
        let zoomed = Vec2 {
            x: state.model.viewport.size.x * 0.5,
            y: state.model.viewport.size.y * 0.5,
        };
        state.model.zoom_ref_size = zoomed;
        state.model.camera_target_view_size = zoomed;

        let now = Instant::now();
        state.enter_xdg_fullscreen(master, None, now);
        // Fullscreen reset the live zoom to the viewport (1.0) and hid the sibling.
        assert_eq!(
            state.model.camera_target_view_size,
            state.model.viewport.size
        );
        assert!(cluster_sibling_hidden_for_fullscreen(&state, stack));

        // Closing the fullscreen member must restore the camera and unhide
        // siblings, mirroring `exit_xdg_fullscreen_inner`.
        drop_fullscreen_surface(
            &mut state,
            master,
            now + std::time::Duration::from_millis(40),
        );

        assert_eq!(state.model.camera_target_view_size, zoomed);
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_camera_restore
                .contains_key("monitor_a")
        );
        assert!(!cluster_sibling_hidden_for_fullscreen(&state, stack));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_hidden_cluster_siblings
                .is_empty()
        );

        let remove_now_ms = state.now_ms(now + std::time::Duration::from_millis(80));
        let removed = state.remove_node_from_field(master, remove_now_ms);
        assert!(removed);
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );
        assert!(state.model.field.node(stack).is_some());
        assert!(!cluster_sibling_hidden_for_fullscreen(&state, stack));
    }

    #[test]
    fn fullscreen_cluster_member_exit_snaps_camera_no_pan() {
        // Regression: exiting fullscreen on a cluster member started a camera
        // animation that fought the survivor reflow ("slides from left, stops
        // partway"). The camera must snap synchronously and no viewport pan may
        // remain after the exit.
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.enter_xdg_fullscreen(master, None, now);
        assert!(state.is_fullscreen_active(master));

        // Exiting must snap the camera, not leave a pan animation alive.
        state.exit_xdg_fullscreen(master, now + std::time::Duration::from_millis(20));
        assert!(
            state.input.interaction_state.viewport_pan_anim.is_none(),
            "no viewport pan should remain after a cluster member exits fullscreen"
        );
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );
    }

    #[test]
    fn fullscreen_cluster_member_exit_preserves_frozen_usable_viewport() {
        // Regression: forcing a workarea refresh on fullscreen exit rewrote the
        // active cluster's frozen usable viewport from the current camera base,
        // causing the top gap to grow on repeated fullscreen exits.
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let frozen = halley_core::viewport::Viewport::new(
            Vec2 { x: 400.0, y: 330.0 },
            Vec2 { x: 800.0, y: 540.0 },
        );
        state
            .model
            .monitor_state
            .monitors
            .get_mut("monitor_a")
            .expect("monitor")
            .usable_viewport = frozen;

        let now = Instant::now();
        state.enter_xdg_fullscreen(master, None, now);
        state.exit_xdg_fullscreen(master, now + std::time::Duration::from_millis(20));

        assert_eq!(
            state
                .model
                .monitor_state
                .monitors
                .get("monitor_a")
                .expect("monitor")
                .usable_viewport,
            frozen,
            "fullscreen exit must not rewrite an active cluster's frozen usable viewport"
        );
    }

    #[test]
    fn fullscreen_cluster_member_close_snaps_camera_no_pan() {
        // Regression: closing a fullscreen cluster member while a camera
        // animation is in flight left the survivors sliding. The drop path must
        // settle the camera and clear any pan before the removal reflow.
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.enter_xdg_fullscreen(master, None, now);

        // Simulate the user keybind exit (starts a camera animation), then the
        // game closing mid-animation.
        assert!(
            crate::compositor::actions::window::toggle_focused_fullscreen_node_state(&mut state)
        );
        // A pan may be alive here from the exit. Closing the surface must settle
        // the camera and clear it.
        drop_fullscreen_surface(
            &mut state,
            master,
            now + std::time::Duration::from_millis(10),
        );
        assert!(
            state.input.interaction_state.viewport_pan_anim.is_none(),
            "no viewport pan should remain after a fullscreen cluster member closes"
        );
    }

    #[test]
    fn minimize_of_fullscreen_cluster_member_exits_fullscreen() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 300.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));

        let now = Instant::now();
        state.enter_xdg_fullscreen(master, None, now);
        assert!(state.is_fullscreen_active(master));

        // Minimizing the fullscreen member tears down fullscreen before the
        // workspace collapses — no dangling, corrupt session left behind.
        assert!(crate::compositor::actions::window::toggle_node_state(
            &mut state,
            master,
            now,
            "monitor_a"
        ));

        assert!(!state.is_fullscreen_session_node(master));
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .is_empty()
        );
        assert!(state.model.fullscreen_state.fullscreen_restore.is_empty());
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            None
        );
        let _ = cid;
    }

    #[test]
    fn hard_exit_after_soft_suspend_restores_fullscreen_session() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = state.model.field.spawn_surface(
            "bystander",
            Vec2 { x: 520.0, y: 280.0 },
            Vec2 { x: 220.0, y: 160.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        state.assign_node_to_monitor(bystander, "monitor_a");
        let fullscreen_pos = state.model.field.node(fullscreen).expect("fullscreen").pos;
        let bystander_pos = state.model.field.node(bystander).expect("bystander").pos;

        let now = Instant::now();
        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        state.soft_suspend_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(300));

        state.exit_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(340));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(760));

        assert_eq!(
            state.model.field.node(fullscreen).expect("fullscreen").pos,
            fullscreen_pos
        );
        assert_eq!(
            state.model.field.node(bystander).expect("bystander").pos,
            bystander_pos
        );
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_active_node
                .is_empty()
        );
        assert!(
            state
                .model
                .fullscreen_state
                .fullscreen_suspended_node
                .is_empty()
        );
        assert!(state.model.fullscreen_state.fullscreen_restore.is_empty());
    }

    #[test]
    fn fullscreen_and_maximize_are_mutually_exclusive() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let target = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, "monitor_a");
        let original_pos = state.model.field.node(target).expect("target").pos;
        let original_size = state
            .model
            .field
            .node(target)
            .expect("target")
            .intrinsic_size;

        // Maximize the window.
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                target,
                now,
                "monitor_a"
            )
        );
        assert!(
            crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );

        // Fullscreening the same window must abort the maximize session (mutual
        // exclusivity per monitor), not preserve it underneath.
        state.enter_xdg_fullscreen(target, None, now + std::time::Duration::from_millis(20));
        assert!(
            !crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );
        assert!(state.is_fullscreen_active(target));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));

        // Exiting fullscreen returns to the pre-maximize geometry — the maximize
        // session was aborted on fullscreen entry, so it does not resume.
        state.exit_xdg_fullscreen(target, now + std::time::Duration::from_millis(300));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(700));

        let restored = state.model.field.node(target).expect("target");
        assert_eq!(restored.pos, original_pos);
        assert_eq!(restored.intrinsic_size, original_size);
        assert!(
            !crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );
        assert!(!restored.pinned);
    }

    #[test]
    fn maximize_fullscreen_maximize_unmaximize_restores_windowed_size() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let tuning = single_monitor_tuning();
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let target = state.model.field.spawn_surface(
            "browser",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, "monitor_a");
        let original_pos = state.model.field.node(target).expect("target").pos;
        let original_size = state
            .model
            .field
            .node(target)
            .expect("target")
            .intrinsic_size;

        // 1. maximize
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                target,
                now,
                "monitor_a"
            )
        );
        // Simulate the real client whose committed surface geometry lags behind: the
        // `window_geometry` cache still reports a large (maximized/fullscreen) size that
        // the client has not yet shrunk. This is captured as the fullscreen restore's
        // `window_geometry` and re-applied on exit, so on the re-maximize below
        // `current_surface_size_for_node` would report this stale large size. The
        // re-maximize must instead trust the windowed size pinned into `resize_footprint`.
        state
            .ui
            .render_state
            .cache
            .window_geometry
            .insert(target, (0.0, 0.0, 1920.0, 1080.0));
        // 2. fullscreen
        state.enter_xdg_fullscreen(target, None, now + std::time::Duration::from_millis(20));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        assert!(state.is_fullscreen_active(target));
        // 3. maximize again (must exit fullscreen, re-snapshot the *windowed* size)
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                target,
                now + std::time::Duration::from_millis(300),
                "monitor_a"
            )
        );
        assert!(!state.is_fullscreen_active(target));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(560));
        // 4. unmaximize
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                target,
                now + std::time::Duration::from_millis(600),
                "monitor_a"
            )
        );

        let restored = state.model.field.node(target).expect("target");
        assert_eq!(restored.pos, original_pos, "pos must return to windowed");
        assert_eq!(
            restored.intrinsic_size, original_size,
            "size must return to windowed, not stay maximized"
        );
    }

    #[test]
    fn maximize_exits_active_fullscreen_on_same_monitor() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning.animations.maximize.enabled = false;
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let a = state.model.field.spawn_surface(
            "game",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let b = state.model.field.spawn_surface(
            "terminal",
            Vec2 { x: 500.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(a, "monitor_a");
        state.assign_node_to_monitor(b, "monitor_a");

        // Fullscreen window A.
        state.enter_xdg_fullscreen(a, None, now);
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        assert!(state.is_fullscreen_active(a));

        // Maximizing window B must exit A's fullscreen (mutual exclusivity).
        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                b,
                now + std::time::Duration::from_millis(300),
                "monitor_a"
            )
        );
        assert!(!state.is_fullscreen_active(a));
        assert!(
            crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );
    }
}

fn fullscreen_animation_progress(start_ms: u64, duration_ms: u64, now_ms: u64) -> f32 {
    let elapsed = now_ms.saturating_sub(start_ms);
    let t = (elapsed as f32 / duration_ms.max(1) as f32).clamp(0.0, 1.0);
    crate::animation::ease_in_out_cubic(t)
}

fn fullscreen_animation_rect(
    st: &Halley,
    anim: &crate::compositor::fullscreen::state::FullscreenScaleAnim,
    now: Instant,
) -> (Vec2, Vec2) {
    let e = fullscreen_animation_progress(anim.start_ms, anim.duration_ms, st.now_ms(now));
    let pos = Vec2 {
        x: anim.from_pos.x + (anim.to_pos.x - anim.from_pos.x) * e,
        y: anim.from_pos.y + (anim.to_pos.y - anim.from_pos.y) * e,
    };
    let size = Vec2 {
        x: (anim.from_size.x + (anim.to_size.x - anim.from_size.x) * e).max(FULLSCREEN_MIN_W),
        y: (anim.from_size.y + (anim.to_size.y - anim.from_size.y) * e).max(FULLSCREEN_MIN_H),
    };
    (pos, size)
}

pub(crate) fn fullscreen_entry_scale(_st: &Halley, _node_id: NodeId, _now_ms: u64) -> f32 {
    1.0
}

pub(crate) fn fullscreen_monitor_for_node(st: &Halley, node_id: NodeId) -> Option<&str> {
    st.model
        .fullscreen_state
        .fullscreen_active_node
        .iter()
        .find_map(|(monitor, &id)| (id == node_id).then_some(monitor.as_str()))
}

pub(crate) fn is_fullscreen_active(st: &Halley, node_id: NodeId) -> bool {
    fullscreen_monitor_for_node(st, node_id).is_some()
}

pub(crate) fn fullscreen_visual_for_node_on_current_monitor(
    st: &Halley,
    node_id: NodeId,
) -> Option<(Vec2, Vec2)> {
    fullscreen_visual_for_node_on_current_monitor_at(st, node_id, Instant::now())
}

pub(crate) fn fullscreen_visual_for_node_on_current_monitor_at(
    st: &Halley,
    node_id: NodeId,
    now: Instant,
) -> Option<(Vec2, Vec2)> {
    let monitor = st.model.monitor_state.current_monitor.as_str();
    fullscreen_visual_for_node_on_monitor_at(st, node_id, monitor, now)
}

pub(crate) fn fullscreen_visual_for_node_on_monitor_at(
    st: &Halley,
    node_id: NodeId,
    monitor: &str,
    now: Instant,
) -> Option<(Vec2, Vec2)> {
    if let Some(anim) = st
        .model
        .fullscreen_state
        .fullscreen_scale_anim
        .get(&node_id)
        .filter(|anim| anim.monitor == monitor)
    {
        return Some(fullscreen_animation_rect(st, anim, now));
    }
    (st.model
        .fullscreen_state
        .fullscreen_active_node
        .get(monitor)
        .copied()
        == Some(node_id))
    .then(|| {
        // Centre the fullscreen window on its OWN position (the camera is recentred
        // onto it on entry), at the monitor's native size. Anchoring to the node's
        // centre rather than the live camera centre keeps the steady-state rect
        // consistent with the grow/shrink animation's endpoint, so there's no jump
        // when the animation expires while the camera is still easing in.
        let size = st
            .model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| space.viewport.size)
            .unwrap_or(st.model.viewport.size);
        let center = st
            .model
            .field
            .node(node_id)
            .map(|node| node.pos)
            .unwrap_or_else(|| {
                st.model
                    .monitor_state
                    .monitors
                    .get(monitor)
                    .map(|space| space.viewport.center)
                    .unwrap_or(st.model.viewport.center)
            });
        (center, size)
    })
}

pub(crate) fn fullscreen_visual_animation_active_for_node_on_current_monitor_at(
    st: &Halley,
    node_id: NodeId,
    now: Instant,
) -> bool {
    let monitor = st.model.monitor_state.current_monitor.as_str();
    st.model
        .fullscreen_state
        .fullscreen_scale_anim
        .get(&node_id)
        .is_some_and(|anim| {
            anim.monitor == monitor
                && st.now_ms(now) < anim.start_ms.saturating_add(anim.duration_ms.max(1))
        })
}

pub(crate) fn is_fullscreen_session_node(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .fullscreen_state
        .fullscreen_active_node
        .values()
        .any(|&id| id == node_id)
        || st
            .model
            .fullscreen_state
            .fullscreen_suspended_node
            .values()
            .any(|&id| id == node_id)
}

fn fullscreen_monitor_name(st: &Halley, node_id: NodeId, output: Option<&WlOutput>) -> String {
    output
        .and_then(|requested_output| {
            st.model
                .monitor_state
                .outputs
                .iter()
                .find_map(|(name, output)| output.owns(requested_output).then_some(name.clone()))
        })
        .or_else(|| st.model.monitor_state.node_monitor.get(&node_id).cloned())
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

fn fullscreen_monitor_view(st: &Halley, monitor_name: &str) -> (Vec2, Vec2) {
    st.model
        .monitor_state
        .monitors
        .get(monitor_name)
        .map(|monitor| (monitor.viewport.center, monitor.viewport.size))
        .unwrap_or((st.model.viewport.center, st.model.viewport.size))
}

fn fullscreen_target_size_for(st: &Halley, monitor_name: &str) -> (i32, i32) {
    st.model
        .monitor_state
        .outputs
        .get(monitor_name)
        .and_then(|output| output.current_mode())
        .map(|mode| (mode.size.w, mode.size.h))
        .unwrap_or_else(|| {
            let (_, size) = fullscreen_monitor_view(st, monitor_name);
            (
                size.x.round().max(FULLSCREEN_MIN_W) as i32,
                size.y.round().max(FULLSCREEN_MIN_H) as i32,
            )
        })
}

fn fullscreen_suspended_monitor_for_node(st: &Halley, node_id: NodeId) -> Option<&str> {
    st.model
        .fullscreen_state
        .fullscreen_suspended_node
        .iter()
        .find_map(|(monitor, &id)| (id == node_id).then_some(monitor.as_str()))
}

fn fullscreen_restore_entries_for_monitor(
    st: &Halley,
    monitor_name: &str,
    exclude_node: Option<NodeId>,
) -> Vec<(
    NodeId,
    crate::compositor::fullscreen::state::FullscreenSessionEntry,
)> {
    let (monitor_viewport_center, _) = fullscreen_monitor_view(st, monitor_name);
    st.model
        .fullscreen_state
        .fullscreen_restore
        .iter()
        .filter(|&(&id, entry)| {
            if exclude_node == Some(id) {
                return false;
            }
            let matches_saved_viewport =
                (entry.viewport_center.x - monitor_viewport_center.x).abs() < 1.0
                    && (entry.viewport_center.y - monitor_viewport_center.y).abs() < 1.0;
            let matches_assigned_monitor = st
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|node_monitor| node_monitor == monitor_name);
            matches_saved_viewport || matches_assigned_monitor
        })
        .map(|(&id, &entry)| (id, entry))
        .collect()
}

fn clear_non_target_fullscreen_restore_entries(
    st: &mut Halley,
    monitor_name: &str,
    target: NodeId,
) {
    let stale = fullscreen_restore_entries_for_monitor(st, monitor_name, Some(target))
        .into_iter()
        .collect::<Vec<_>>();
    for (id, entry) in stale {
        let _ = st.model.field.set_pinned(id, entry.pinned);
        st.input.interaction_state.physics_velocity.remove(&id);
        st.model.fullscreen_state.fullscreen_restore.remove(&id);
        st.model.fullscreen_state.fullscreen_motion.remove(&id);
        st.model.fullscreen_state.fullscreen_scale_anim.remove(&id);
    }
}

fn request_toplevel_fullscreen_state(
    st: &mut Halley,
    node_id: NodeId,
    fullscreen: bool,
    output: Option<WlOutput>,
    size: Option<(i32, i32)>,
) {
    let monitor_name = if fullscreen {
        fullscreen_monitor_name(st, node_id, output.as_ref())
    } else {
        fullscreen_monitor_for_node(st, node_id)
            .map(str::to_string)
            .or_else(|| st.model.monitor_state.node_monitor.get(&node_id).cloned())
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
    };
    let focused_node = st
        .last_input_surface_node_for_monitor(monitor_name.as_str())
        .or_else(|| st.last_input_surface_node());
    let bounds_size = if fullscreen {
        size.unwrap_or_else(|| fullscreen_target_size_for(st, monitor_name.as_str()))
    } else {
        let view = st.usable_viewport_for_monitor(&monitor_name);
        (view.size.x as i32, view.size.y as i32)
    };
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        let (min_w, min_h) = crate::compositor::surface::toplevel_min_size_for_node(st, node_id);
        top.with_pending_state(|s| {
            s.size = size.map(|(w, h)| (w.max(min_w).max(96), h.max(min_h).max(72)).into());
            s.bounds = Some((bounds_size.0.max(96), bounds_size.1.max(72)).into());
            if focused_node == Some(node_id) {
                s.states.set(xdg_toplevel::State::Activated);
            } else {
                s.states.unset(xdg_toplevel::State::Activated);
            }
            if fullscreen {
                s.states.set(xdg_toplevel::State::Fullscreen);
                s.fullscreen_output = output;
            } else {
                s.states.unset(xdg_toplevel::State::Fullscreen);
                s.fullscreen_output = None;
            }
            st.apply_toplevel_tiled_hint(s);
        });
        top.send_configure();
        break;
    }
}

/// Returns the monitor name that `node_id` is currently fullscreened on, if any.
fn exit_xdg_fullscreen_inner(
    st: &mut Halley,
    node_id: NodeId,
    now: Instant,
    suspend: bool,
    preserve_client_fullscreen: bool,
    skip_animation: bool,
) {
    // Find which monitor this node is fullscreened on.
    let monitor_name = match fullscreen_monitor_for_node(st, node_id) {
        Some(m) => m.to_owned(),
        None => return, // not active fullscreen on any monitor
    };

    // Invalidate the offscreen texture so the next capture (Apogee/Alt+Tab or the
    // main render path) rebuilds it at the post-fullscreen geometry instead of
    // reusing the fullscreen-sized snapshot for a now-windowed surface.
    st.ui.render_state.clear_window_offscreen_cache_for(node_id);

    st.model
        .fullscreen_state
        .clear_direct_scanout_for_monitor(&monitor_name);

    st.input.interaction_state.reset_input_state_requested = true;

    if suspend {
        st.model
            .fullscreen_state
            .fullscreen_suspended_node
            .insert(monitor_name.clone(), node_id);
        if preserve_client_fullscreen {
            st.model
                .fullscreen_state
                .fullscreen_soft_suspended_node
                .insert(monitor_name.clone(), node_id);
        } else {
            st.model
                .fullscreen_state
                .fullscreen_soft_suspended_node
                .remove(&monitor_name);
        }
    } else {
        // If we're doing a hard exit, clear any suspended state for this monitor too.
        st.model
            .fullscreen_state
            .fullscreen_suspended_node
            .remove(&monitor_name);
        st.model
            .fullscreen_state
            .fullscreen_soft_suspended_node
            .remove(&monitor_name);
    }

    // Genuine exit (not a suspend or client-fullscreen release): set the monitor
    // camera target back to the zoom/center captured on entry so we don't stay
    // plopped at 1.0 zoom.
    let restored_camera = if !suspend && !preserve_client_fullscreen {
        restore_monitor_camera_after_fullscreen(st, &monitor_name)
    } else {
        None
    };

    clear_non_target_fullscreen_restore_entries(st, &monitor_name, node_id);

    // A hard exit on the current monitor animates the window shrinking back
    // to its restored geometry. Capture the current full visual rect now,
    // while the node is still the active fullscreen node, so the anim can
    // ease from it (the node leaves active state below).
    let should_animate = !suspend
        && !preserve_client_fullscreen
        && !skip_animation
        && st.runtime.tuning.fullscreen_animation_enabled()
        && st.model.monitor_state.current_monitor == monitor_name;

    // For a cluster member, the active cluster layout owns the camera: snap it
    // synchronously to the restored target and cancel any in-flight pan so the
    // survivor reflow projects against a settled viewport instead of one still
    // easing back from fullscreen. Otherwise, ease the camera back when the exit
    // itself is animated. A non-animated exit (notably the fullscreen→maximize
    // handoff via `exit_xdg_fullscreen_no_anim`) must leave the camera to
    // whatever takes over next, so we don't fight it with a transition toward
    // the pre-fullscreen zoom.
    let cluster_member_exit = node_is_active_cluster_member_on_monitor(st, node_id, &monitor_name);
    if cluster_member_exit {
        settle_cluster_camera_after_fullscreen(st, &monitor_name, restored_camera);
    } else if should_animate && let Some(camera) = restored_camera {
        animate_camera_restore_after_fullscreen(st, &monitor_name, camera, now);
    }
    let exit_anim_from = should_animate.then(|| {
        fullscreen_visual_for_node_on_current_monitor_at(st, node_id, now)
            .unwrap_or_else(|| fullscreen_monitor_view(st, &monitor_name))
    });

    let restore_entry = st
        .model
        .fullscreen_state
        .fullscreen_restore
        .get(&node_id)
        .copied();
    if let Some(entry) = restore_entry {
        restore_fullscreen_snapshot(st, node_id, entry);
    }

    if preserve_client_fullscreen {
        // Keep the xdg-toplevel protocol fullscreen state intact while the
        // compositor releases its local fullscreen layout lock.
    } else if let Some(entry) = st
        .model
        .fullscreen_state
        .fullscreen_restore
        .get(&node_id)
        .copied()
    {
        let (min_w, min_h) = crate::compositor::surface::toplevel_min_size_for_node(st, node_id);
        request_toplevel_fullscreen_state(
            st,
            node_id,
            false,
            None,
            Some((
                entry.size.x.round().max(min_w as f32).max(FULLSCREEN_MIN_W) as i32,
                entry.size.y.round().max(min_h as f32).max(FULLSCREEN_MIN_H) as i32,
            )),
        );
    } else {
        request_toplevel_fullscreen_state(st, node_id, false, None, None);
    }

    st.model
        .fullscreen_state
        .fullscreen_active_node
        .remove(&monitor_name);
    if let Some(from) = exit_anim_from {
        // Reverse anim: shrink from the full rect back to the restored
        // geometry. The node is no longer active, but the lingering
        // scale anim keeps driving `fullscreen_visual_*` as a pure visual
        // overlay until it expires in `tick_fullscreen_motion`.
        let to =
            crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor_at(
                st, node_id, now,
            )
            .or_else(|| restore_entry.map(|entry| (entry.pos, entry.size)))
            .unwrap_or(from);
        let start_ms = st.now_ms(now);
        let duration_ms = st.runtime.tuning.fullscreen_animation_duration_ms();
        st.model.fullscreen_state.fullscreen_scale_anim.insert(
            node_id,
            crate::compositor::fullscreen::state::FullscreenScaleAnim {
                monitor: monitor_name.clone(),
                from_pos: from.0,
                to_pos: to.0,
                from_size: from.1,
                to_size: to.1,
                start_ms,
                duration_ms,
            },
        );
    } else {
        st.model
            .fullscreen_state
            .fullscreen_scale_anim
            .remove(&node_id);
    }
    st.model.fullscreen_state.fullscreen_origin.remove(&node_id);
    if !suspend {
        st.model
            .fullscreen_state
            .fullscreen_restore
            .remove(&node_id);
    }
    // On a genuine exit (not soft-suspend), bring back the hidden cluster tiles
    // and re-lay out the workspace. Soft-suspend keeps the siblings hidden because
    // the fullscreen session is still alive from the user's point of view.
    if !suspend && !preserve_client_fullscreen {
        restore_cluster_workspace_after_fullscreen(st, node_id, now, true);
    }
    st.request_maintenance();
}

pub(crate) fn soft_suspend_xdg_fullscreen(st: &mut Halley, node_id: NodeId, now: Instant) {
    exit_xdg_fullscreen_inner(st, node_id, now, true, true, false);
}

fn restore_fullscreen_snapshot(
    st: &mut Halley,
    id: NodeId,
    entry: crate::compositor::fullscreen::state::FullscreenSessionEntry,
) {
    if let Some(node) = st.model.field.node_mut(id) {
        node.pos = entry.pos;
        node.intrinsic_size = entry.intrinsic_size;
    }
    // Reset the footprint to the restored intrinsic size first (this also clears any
    // stale `resize_footprint`), THEN pin `resize_footprint` to the restored windowed
    // size so it survives. `sync_active_footprint_to_intrinsic` sets `resize_footprint
    // = None`, so doing it the other way around wipes the value we need: until the
    // client commits the windowed resize, `current_surface_size_for_node` still reports
    // the fullscreen geometry, and `current_window_size_for_node` prefers
    // `resize_footprint` over that stale surface size. Without this, exiting fullscreen
    // straight into a maximize snapshots the fullscreen size and "restores" to it on
    // unmaximize.
    let _ = st.model.field.sync_active_footprint_to_intrinsic(id);
    let _ = st
        .model
        .field
        .set_resize_footprint(id, Some(entry.intrinsic_size));
    if let Some(loc) = entry.bbox_loc {
        st.ui.render_state.cache.bbox_loc.insert(id, loc);
    } else {
        st.ui.render_state.cache.bbox_loc.remove(&id);
    }
    if let Some(geo) = entry.window_geometry {
        st.ui.render_state.cache.window_geometry.insert(id, geo);
    } else {
        st.ui.render_state.cache.window_geometry.remove(&id);
    }
    st.set_last_active_size_now(id, entry.intrinsic_size);
}

/// True if `node_id` is a cluster workspace sibling currently hidden because
/// another member of the same workspace is fullscreen. Consulted by the render
/// layout path so only the fullscreen tile shows while a cluster member is
/// fullscreen.
pub(crate) fn cluster_sibling_hidden_for_fullscreen(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .fullscreen_state
        .fullscreen_hidden_cluster_siblings
        .values()
        .any(|siblings| siblings.contains(&node_id))
}

/// When a cluster workspace member enters fullscreen, record its sibling members
/// so the render path can hide them (only the fullscreen tile shows). The
/// fullscreen member keeps its cluster membership; the layout is restored by
/// `restore_cluster_workspace_after_fullscreen` on exit. No-op for non-cluster
/// or non-active-workspace nodes.
fn hide_cluster_workspace_siblings_for_fullscreen(st: &mut Halley, node_id: NodeId) {
    let Some(cid) = st.model.field.cluster_id_for_member_public(node_id) else {
        return;
    };
    let monitor = st.monitor_for_node_or_current(node_id);
    if st.active_cluster_workspace_for_monitor(&monitor) != Some(cid) {
        return;
    }
    let siblings = st
        .model
        .field
        .cluster(cid)
        .map(|cluster| {
            cluster
                .members()
                .iter()
                .copied()
                .filter(|member| *member != node_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if siblings.is_empty() {
        return;
    }
    st.model
        .fullscreen_state
        .fullscreen_hidden_cluster_siblings
        .insert(node_id, siblings);
}

/// Ease the monitor camera back to the pre-fullscreen zoom/center snapshot
/// captured on entry. Shared by the fullscreen-exit path and the surface-close
/// teardown path so the camera doesn't stay anchored on a fullscreen node that
/// is no longer active. No-op if no snapshot was recorded for `monitor_name`.
/// Set the monitor camera target back to the pre-fullscreen snapshot and return
/// it (so an animated exit can ease the live camera there). No-op + `None` if no
/// snapshot was recorded. Does NOT start the easing animation itself — the caller
/// decides whether to animate, so a non-animated exit (e.g. handing off to a
/// maximize that drives its own camera) doesn't leave a conflicting transition.
fn restore_monitor_camera_after_fullscreen(
    st: &mut Halley,
    monitor_name: &str,
) -> Option<crate::compositor::workspace::state::MaximizeCameraSnapshot> {
    let camera = st
        .model
        .fullscreen_state
        .fullscreen_camera_restore
        .remove(monitor_name)?;
    crate::compositor::workspace::state::set_monitor_camera_target_snapshot(
        st,
        monitor_name,
        camera,
    );
    Some(camera)
}

/// Ease the live camera back to a restored fullscreen snapshot along the same
/// fixed cubic the entry used (matching maximize), instead of the exponential
/// zoom smoothing whose tail makes the shrink "stick" near the end.
fn animate_camera_restore_after_fullscreen(
    st: &mut Halley,
    monitor_name: &str,
    camera: crate::compositor::workspace::state::MaximizeCameraSnapshot,
    now: Instant,
) {
    let duration_ms = st.runtime.tuning.fullscreen_animation_duration_ms();
    crate::compositor::focus::system::animate_camera_center_zoom_on_monitor(
        st,
        monitor_name,
        camera.center,
        camera.view_size,
        duration_ms,
        now,
    );
}

/// Whether `node_id` is a member of the active cluster workspace on `monitor`.
fn node_is_active_cluster_member_on_monitor(st: &Halley, node_id: NodeId, monitor: &str) -> bool {
    st.model
        .field
        .cluster_id_for_member_public(node_id)
        .is_some_and(|cid| st.active_cluster_workspace_for_monitor(monitor) == Some(cid))
}

/// Snap the monitor camera to a restored fullscreen snapshot synchronously
/// (no easing) and cancel any in-flight viewport pan for that monitor. Used
/// when a fullscreen cluster member exits or dies: the active cluster layout
/// owns the camera, so a lingering camera animation would slide the survivor
/// tiles as the cluster re-lays out ("slides from the left and stops partway").
/// If `camera` is `None` (snapshot already consumed by an earlier exit), snaps
/// the live viewport to the current camera targets instead.
fn settle_cluster_camera_after_fullscreen(
    st: &mut Halley,
    monitor_name: &str,
    camera: Option<crate::compositor::workspace::state::MaximizeCameraSnapshot>,
) {
    if let Some(camera) = camera {
        crate::compositor::workspace::state::apply_monitor_camera_snapshot(
            st,
            monitor_name,
            camera,
        );
    } else if st.model.monitor_state.current_monitor == monitor_name {
        st.model.viewport.center = st.model.camera_target_center;
        st.model.zoom_ref_size = st.model.camera_target_view_size;
    } else if let Some(space) = st.model.monitor_state.monitors.get_mut(monitor_name) {
        space.viewport.center = space.camera_target_center;
        space.zoom_ref_size = space.camera_target_view_size;
    }
    if st
        .input
        .interaction_state
        .viewport_pan_anim
        .as_ref()
        .is_some_and(|anim| anim.monitor == monitor_name)
    {
        st.input.interaction_state.viewport_pan_anim = None;
    }
}

/// Reverse of `hide_cluster_workspace_siblings_for_fullscreen`: clear the hidden
/// sibling set for `node_id` and optionally re-lay out the active cluster. The
/// close path skips the immediate layout because member removal will reflow the
/// survivors after the closing node is actually gone.
fn restore_cluster_workspace_after_fullscreen(
    st: &mut Halley,
    node_id: NodeId,
    now: Instant,
    relayout: bool,
) {
    let Some(cid) = st.model.field.cluster_id_for_member_public(node_id) else {
        st.model
            .fullscreen_state
            .fullscreen_hidden_cluster_siblings
            .remove(&node_id);
        return;
    };
    let monitor = st.monitor_for_node_or_current(node_id);
    let workspace_still_active = st.active_cluster_workspace_for_monitor(&monitor) == Some(cid);
    st.model
        .fullscreen_state
        .fullscreen_hidden_cluster_siblings
        .remove(&node_id);
    if workspace_still_active && relayout {
        st.layout_active_cluster_workspace_for_monitor(&monitor, st.now_ms(now));
    }
}

fn client_fullscreen_blocked_for_cluster_node(st: &mut Halley, node_id: NodeId) -> bool {
    if !st
        .model
        .fullscreen_state
        .client_fullscreen_blocked_nodes
        .contains(&node_id)
    {
        return false;
    }

    let Some(cid) = st.model.field.cluster_id_for_member_public(node_id) else {
        st.model
            .fullscreen_state
            .client_fullscreen_blocked_nodes
            .remove(&node_id);
        return false;
    };
    let monitor = st.monitor_for_node_or_current(node_id);
    if st.active_cluster_workspace_for_monitor(&monitor) == Some(cid) {
        return true;
    }

    st.model
        .fullscreen_state
        .client_fullscreen_blocked_nodes
        .remove(&node_id);
    false
}

pub(crate) fn block_client_fullscreen_for_cluster_node(st: &mut Halley, node_id: NodeId) {
    let Some(cid) = st.model.field.cluster_id_for_member_public(node_id) else {
        return;
    };
    let monitor = st.monitor_for_node_or_current(node_id);
    if st.active_cluster_workspace_for_monitor(&monitor) == Some(cid) {
        st.model
            .fullscreen_state
            .client_fullscreen_blocked_nodes
            .insert(node_id);
    }
}

pub(crate) fn enter_xdg_fullscreen(
    st: &mut Halley,
    node_id: NodeId,
    output: Option<WlOutput>,
    now: Instant,
) {
    enter_fullscreen(
        st,
        node_id,
        output,
        now,
        crate::compositor::fullscreen::state::FullscreenOrigin::ClientRequest,
    )
}

pub(crate) fn enter_user_fullscreen(
    st: &mut Halley,
    node_id: NodeId,
    output: Option<WlOutput>,
    now: Instant,
) {
    enter_fullscreen(
        st,
        node_id,
        output,
        now,
        crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind,
    )
}

/// A window must join the active cluster layout *before* fullscreening, so the fullscreen
/// (a game going into "game mode") sits inside the cluster as a real tile/stack member:
/// siblings hidden, camera grows from its slot, and exiting or closing returns cleanly to
/// the cluster. Games drive their own `set_fullscreen`, often before they've joined the
/// cluster: a late app_id (Wine/XWayland/gamescope), a `Float` rule, or simply spawning
/// then immediately fullscreening can all leave the window a non-member. When that
/// happens the fullscreen sits *outside* the cluster and, on close,
/// `restore_cluster_workspace_after_fullscreen` finds no membership and never re-lays the
/// cluster out, so it gets "stuck". This chokepoint guarantees membership regardless of
/// how the window got here.
///
/// Idempotent and safe: a no-op when the node is already a cluster member or when its
/// monitor has no active cluster workspace (an ordinary fullscreen with nothing to join).
/// `absorb_node_into_cluster` handles both Tiling and Stacking layouts and performs the
/// monitor-assign + re-layout internally.
fn ensure_cluster_membership_before_fullscreen(st: &mut Halley, node_id: NodeId, now: Instant) {
    if st
        .model
        .field
        .cluster_id_for_member_public(node_id)
        .is_some()
    {
        return;
    }
    let monitor = st.monitor_for_node_or_current(node_id);
    let Some(cid) = st.active_cluster_workspace_for_monitor(&monitor) else {
        return;
    };
    let _ = st.absorb_node_into_cluster(cid, node_id, now);
}

fn enter_fullscreen(
    st: &mut Halley,
    node_id: NodeId,
    output: Option<WlOutput>,
    now: Instant,
    origin: crate::compositor::fullscreen::state::FullscreenOrigin,
) {
    // Join the active cluster first so the fullscreen lands inside the cluster layout.
    ensure_cluster_membership_before_fullscreen(st, node_id, now);

    if origin == crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind {
        st.model
            .fullscreen_state
            .client_fullscreen_blocked_nodes
            .remove(&node_id);
    } else if client_fullscreen_blocked_for_cluster_node(st, node_id) {
        request_toplevel_fullscreen_state(st, node_id, false, None, None);
        st.request_maintenance();
        return;
    }

    let monitor_name = fullscreen_monitor_name(st, node_id, output.as_ref());

    st.model
        .fullscreen_state
        .clear_direct_scanout_for_monitor(&monitor_name);

    // Already fullscreen on this monitor — no-op.
    if st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(&monitor_name)
        == Some(&node_id)
    {
        let existing_origin = st
            .model
            .fullscreen_state
            .fullscreen_origin
            .get(&node_id)
            .copied();
        if existing_origin
            != Some(crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind)
            || origin == crate::compositor::fullscreen::state::FullscreenOrigin::UserKeybind
        {
            st.model
                .fullscreen_state
                .fullscreen_origin
                .insert(node_id, origin);
        }
        if origin == crate::compositor::fullscreen::state::FullscreenOrigin::ClientRequest {
            let target_size = fullscreen_target_size_for(st, monitor_name.as_str());
            request_toplevel_fullscreen_state(st, node_id, true, output, Some(target_size));
        }
        return;
    }

    let soft_resume = st
        .model
        .fullscreen_state
        .fullscreen_soft_suspended_node
        .get(&monitor_name)
        == Some(&node_id);

    if soft_resume {
        st.model
            .fullscreen_state
            .fullscreen_suspended_node
            .remove(&monitor_name);
        st.model
            .fullscreen_state
            .fullscreen_soft_suspended_node
            .remove(&monitor_name);
    } else {
        // Clear any suspended state for this monitor.
        st.model
            .fullscreen_state
            .fullscreen_suspended_node
            .remove(&monitor_name);
        st.model
            .fullscreen_state
            .fullscreen_soft_suspended_node
            .remove(&monitor_name);
    }

    // If another window is fullscreened on the same monitor, exit it first.
    if let Some(existing) = st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(&monitor_name)
        .copied()
    {
        exit_xdg_fullscreen(st, existing, now);
    }

    // Maximize and fullscreen are mutually exclusive on a monitor: abort any active
    // maximize session before taking over. This must precede the pre-fullscreen camera
    // capture below so exit-fullscreen returns to the pre-maximize view.
    // Capture the node's pre-maximize geometry first: after the abort the surface
    // buffer is still at the (larger) maximize size until the client commits the
    // resize, so `current_surface_size_for_node` would return a stale size for
    // `fullscreen_restore`. The snapshot has the true original windowed size.
    let maximize_pre_size = st
        .model
        .workspace_state
        .maximize_sessions
        .get(monitor_name.as_str())
        .and_then(|session| session.node_snapshots.get(&node_id))
        .map(|snapshot| snapshot.size);
    // Also capture the maximized window's current on-screen rect before the abort, so
    // the fullscreen grow eases from the maximized rect up to full-screen instead of
    // snapping to the small windowed size first (the abort restores windowed geometry).
    let maximize_pre_visual =
        crate::compositor::workspace::state::maximized_visual_for_node_on_monitor_at(
            st,
            node_id,
            monitor_name.as_str(),
            now,
        );
    if crate::compositor::workspace::state::maximize_session_active_on_monitor(
        st,
        monitor_name.as_str(),
    ) {
        let _ = crate::compositor::workspace::state::abort_maximize_session_for_monitor(
            st,
            monitor_name.as_str(),
        );
    }

    let target_size = fullscreen_target_size_for(st, monitor_name.as_str());
    let (viewport_center, viewport_size) = fullscreen_monitor_view(st, monitor_name.as_str());
    clear_non_target_fullscreen_restore_entries(st, &monitor_name, node_id);

    // Capture the pre-fullscreen camera (zoom + center) once per monitor so exiting
    // fullscreen returns to it instead of staying at 1.0. `or_insert` keeps the
    // original across fullscreen→fullscreen swaps and soft suspend/resume.
    let pre_fullscreen_camera =
        crate::compositor::workspace::state::snapshot_monitor_camera(st, monitor_name.as_str());
    st.model
        .fullscreen_state
        .fullscreen_camera_restore
        .entry(monitor_name.clone())
        .or_insert(pre_fullscreen_camera);

    let Some(node) = st.model.field.node(node_id).cloned() else {
        return;
    };

    // If the fullscreen target is an active cluster workspace member, hide its
    // sibling tiles so only the fullscreen window shows while the session is up.
    hide_cluster_workspace_siblings_for_fullscreen(st, node_id);

    // Invalidate any stale windowed offscreen texture so the next Apogee/Alt+Tab
    // capture rebuilds it at the fullscreen surface geometry instead of reusing
    // the smaller windowed snapshot. Defense-in-depth: the offscreen cache also
    // self-heals on size change (`ensure_window_offscreen_cache` → `matches_size`).
    // NOTE: this does NOT fix the live field render — that corruption is the scale
    // math using stale CSD geometry, handled by `render_window_geometry_for_node`.
    st.ui.render_state.clear_window_offscreen_cache_for(node_id);

    // Animate the monitor camera to centre on the window AND ease the zoom to 1.0
    // together, so the window grows in place to fill the screen. The old behaviour
    // snapped the zoom to 1.0 about the (off-window) camera centre, which shoved
    // every windowed node behind it sideways by the zoom delta.
    crate::compositor::workspace::state::set_monitor_camera_target_snapshot(
        st,
        monitor_name.as_str(),
        crate::compositor::workspace::state::MaximizeCameraSnapshot {
            center: node.pos,
            view_size: viewport_size,
        },
    );

    let soft_resume_entry = soft_resume
        .then(|| {
            st.model
                .fullscreen_state
                .fullscreen_restore
                .get(&node_id)
                .copied()
        })
        .flatten();
    let saved_size = soft_resume_entry
        .map(|entry| entry.size)
        .or(maximize_pre_size)
        .unwrap_or_else(|| {
            crate::compositor::surface::current_surface_size_for_node(st, node_id)
                .unwrap_or(node.intrinsic_size)
        });
    let saved_bbox_loc = soft_resume_entry
        .and_then(|entry| entry.bbox_loc)
        .or_else(|| st.ui.render_state.cache.bbox_loc.get(&node_id).copied());
    let saved_window_geometry = soft_resume_entry
        .and_then(|entry| entry.window_geometry)
        .or_else(|| {
            st.ui
                .render_state
                .cache
                .window_geometry
                .get(&node_id)
                .copied()
        });
    let saved_pos = soft_resume_entry.map(|entry| entry.pos).unwrap_or(node.pos);
    let saved_intrinsic_size = soft_resume_entry
        .map(|entry| entry.intrinsic_size)
        .unwrap_or(node.intrinsic_size);
    let saved_pinned = soft_resume_entry
        .map(|entry| entry.pinned)
        .unwrap_or(node.pinned);

    st.model.fullscreen_state.fullscreen_restore.insert(
        node_id,
        crate::compositor::fullscreen::state::FullscreenSessionEntry {
            pos: saved_pos,
            size: saved_size,
            viewport_center,
            intrinsic_size: saved_intrinsic_size,
            bbox_loc: saved_bbox_loc,
            window_geometry: saved_window_geometry,
            pinned: saved_pinned,
        },
    );
    if st.runtime.tuning.fullscreen_animation_enabled() && !soft_resume {
        st.request_window_animation_prewarm(node_id, now);
        // Prefer the maximized window's pre-abort rect (captured above) so a
        // maximize→fullscreen grows smoothly from maximized to full-screen. The abort
        // has already torn down the maximize session, so re-querying it here returns
        // nothing — hence capturing before the abort.
        // If this window is an active cluster-workspace tile, grow from the tile's
        // current on-screen rect rather than its raw surface size. The surface buffer
        // can differ from the tile's displayed (intrinsic) size, which otherwise snaps
        // the window to that size at t=0 and reads as an over-exaggerated fly-in.
        // Starting from the visible tile makes it a continuous "this tile zooms to
        // fullscreen".
        let cluster_tile_from = st
            .model
            .field
            .cluster_id_for_member_public(node_id)
            .filter(|cid| st.active_cluster_workspace_for_monitor(&monitor_name) == Some(*cid))
            .and_then(|_| {
                crate::animation::cluster_tile_rect_for(
                    st.ui.render_state.cluster_tile_tracks(),
                    node_id,
                    now,
                )
                .or_else(|| {
                    crate::animation::cluster_tile_rect_from_field(&st.model.field, node_id)
                })
            })
            .map(|rect| (rect.center, rect.size));
        let from = maximize_pre_visual
            .or(cluster_tile_from)
            .or_else(|| {
                (st.model.monitor_state.current_monitor == monitor_name)
                    .then(|| {
                        crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor_at(
                            st, node_id, now,
                        )
                    })
                    .flatten()
            })
            .unwrap_or((saved_pos, saved_size));
        let start_ms = st.now_ms(now);
        let duration_ms = st.runtime.tuning.fullscreen_animation_duration_ms();
        st.model.fullscreen_state.fullscreen_scale_anim.insert(
            node_id,
            crate::compositor::fullscreen::state::FullscreenScaleAnim {
                monitor: monitor_name.clone(),
                from_pos: from.0,
                // Grow in place (centred on the window itself), matching the camera
                // recentring above — not toward the old camera centre.
                to_pos: node.pos,
                from_size: from.1,
                to_size: viewport_size,
                start_ms,
                duration_ms,
            },
        );
        // Drive the camera recenter+zoom on the SAME fixed cubic as the scale
        // anim (matching maximize), instead of letting the exponential zoom
        // smoothing chase the target set above — its asymptotic tail is what made
        // the grow "stick" near the end, worse the further the zoom had to settle.
        crate::compositor::focus::system::animate_camera_center_zoom_on_monitor(
            st,
            monitor_name.as_str(),
            node.pos,
            viewport_size,
            duration_ms,
            now,
        );
    } else {
        st.model
            .fullscreen_state
            .fullscreen_scale_anim
            .remove(&node_id);
    }
    if !soft_resume {
        request_toplevel_fullscreen_state(st, node_id, true, output, Some(target_size));
    }
    st.assign_node_to_monitor(node_id, monitor_name.as_str());
    st.model
        .fullscreen_state
        .fullscreen_active_node
        .insert(monitor_name, node_id);
    st.model
        .fullscreen_state
        .fullscreen_origin
        .insert(node_id, origin);
    st.set_interaction_focus(Some(node_id), 30_000, now);
    let _ = st.raise_overlap_policy_node(node_id);
    st.request_maintenance();
}

pub(crate) fn exit_xdg_fullscreen(st: &mut Halley, node_id: NodeId, now: Instant) {
    resume_and_clear_fullscreen_suspended_state(st, node_id);
    exit_xdg_fullscreen_inner(st, node_id, now, false, false, false);
}

/// Exit fullscreen without the shrink animation — used when transitioning directly
/// to maximize so the maximize grow animation is the only visible motion (no
/// conflicting shrink-then-grow flash).
pub(crate) fn exit_xdg_fullscreen_no_anim(st: &mut Halley, node_id: NodeId, now: Instant) {
    resume_and_clear_fullscreen_suspended_state(st, node_id);
    exit_xdg_fullscreen_inner(st, node_id, now, false, false, true);
}

/// Promote a soft-suspended fullscreen node back to active and clear the suspended
/// entry, so the inner exit processes it as a genuine active-fullscreen exit.
fn resume_and_clear_fullscreen_suspended_state(st: &mut Halley, node_id: NodeId) {
    if !is_fullscreen_active(st, node_id)
        && let Some(monitor) =
            fullscreen_suspended_monitor_for_node(st, node_id).map(str::to_string)
    {
        st.model
            .fullscreen_state
            .fullscreen_suspended_node
            .remove(&monitor);
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor, node_id);
    }
    // Clear suspended state on whatever monitor this node is on.
    if let Some(monitor) = fullscreen_monitor_for_node(st, node_id).map(|s| s.to_owned()) {
        st.model
            .fullscreen_state
            .fullscreen_suspended_node
            .remove(&monitor);
    }
}

pub(crate) fn drop_fullscreen_surface(st: &mut Halley, id: NodeId, now: Instant) {
    if !is_fullscreen_active(st, id)
        && let Some(monitor) = fullscreen_suspended_monitor_for_node(st, id).map(str::to_string)
    {
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor, id);
    }

    // Clear suspended state if this node was suspended on any monitor.
    st.model
        .fullscreen_state
        .fullscreen_suspended_node
        .retain(|_, &mut nid| nid != id);
    st.model
        .fullscreen_state
        .fullscreen_soft_suspended_node
        .retain(|_, &mut nid| nid != id);

    if is_fullscreen_active(st, id) {
        let monitor_name = fullscreen_monitor_for_node(st, id)
            .map(|s| s.to_owned())
            .unwrap(); // safe: is_fullscreen_active just confirmed it

        st.input.interaction_state.reset_input_state_requested = true;
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .remove(&monitor_name);

        clear_non_target_fullscreen_restore_entries(st, &monitor_name, id);

        // Mirror `exit_xdg_fullscreen_inner`: restore the monitor camera and
        // unhide cluster siblings before the member-removal path performs the
        // survivor reflow. For a cluster member the camera is snapped
        // synchronously and any in-flight pan is cancelled so the survivor
        // reflow projects against a settled viewport.
        let camera = restore_monitor_camera_after_fullscreen(st, &monitor_name);
        if node_is_active_cluster_member_on_monitor(st, id, &monitor_name) {
            settle_cluster_camera_after_fullscreen(st, &monitor_name, camera);
        }
        restore_cluster_workspace_after_fullscreen(st, id, now, false);
    }

    st.model
        .fullscreen_state
        .client_fullscreen_blocked_nodes
        .remove(&id);
    st.model.fullscreen_state.fullscreen_restore.remove(&id);
    st.model.fullscreen_state.fullscreen_origin.remove(&id);
    st.model.fullscreen_state.fullscreen_motion.remove(&id);
    st.model.fullscreen_state.fullscreen_scale_anim.remove(&id);
    st.model.fullscreen_state.clear_direct_scanout_for_node(id);
    // Clear any cluster-sibling hide state keyed on this node (it was the
    // fullscreen member) or referencing it as a hidden sibling. The cluster
    // member-removal path re-lays out the workspace for the survivors.
    st.model
        .fullscreen_state
        .fullscreen_hidden_cluster_siblings
        .remove(&id);
    st.model
        .fullscreen_state
        .fullscreen_hidden_cluster_siblings
        .values_mut()
        .for_each(|siblings| siblings.retain(|sibling| *sibling != id));
}

pub(crate) fn tick_fullscreen_motion(st: &mut Halley, now: Instant) {
    if st.model.fullscreen_state.fullscreen_motion.is_empty()
        && st.model.fullscreen_state.fullscreen_scale_anim.is_empty()
    {
        return;
    }

    let now_ms = st.now_ms(now);
    let motions: Vec<(
        NodeId,
        crate::compositor::fullscreen::state::FullscreenMotion,
    )> = st
        .model
        .fullscreen_state
        .fullscreen_motion
        .iter()
        .map(|(&id, &motion)| (id, motion))
        .collect();
    let mut finished = Vec::new();

    for (id, motion) in motions {
        let elapsed = now_ms.saturating_sub(motion.start_ms);
        let t = (elapsed as f32 / motion.duration_ms.max(1) as f32).clamp(0.0, 1.0);
        let e = crate::animation::ease_in_out_cubic(t);
        let pos = Vec2 {
            x: motion.from.x + (motion.to.x - motion.from.x) * e,
            y: motion.from.y + (motion.to.y - motion.from.y) * e,
        };
        let _ = st.model.field.carry(id, pos);
        if t >= 1.0 {
            finished.push((id, motion));
        }
    }

    for (id, motion) in finished {
        st.model.fullscreen_state.fullscreen_motion.remove(&id);
        if let Some(node) = st.model.field.node_mut(id) {
            node.pos = motion.to;
        }
        st.input.interaction_state.physics_velocity.remove(&id);
        if let Some(entry) = st
            .model
            .fullscreen_state
            .fullscreen_restore
            .get(&id)
            .copied()
        {
            // A node finishing its motion should be pinned only if the fullscreen
            // it was displaced for is still active — i.e. the monitor it belongs
            // to still has an active fullscreen session.
            let node_monitor = st
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .cloned()
                .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
            let displaced_for_active = st
                .model
                .fullscreen_state
                .fullscreen_active_node
                .contains_key(&node_monitor);

            if displaced_for_active {
                let _ = st.model.field.set_pinned(id, true);
            } else {
                let _ = st.model.field.set_pinned(id, entry.pinned);
                st.model.fullscreen_state.fullscreen_restore.remove(&id);
            }
        }
    }

    st.model
        .fullscreen_state
        .fullscreen_scale_anim
        .retain(|_, anim| now_ms < anim.start_ms.saturating_add(anim.duration_ms));
    if !st.model.fullscreen_state.fullscreen_scale_anim.is_empty() {
        st.request_maintenance();
    }
}
