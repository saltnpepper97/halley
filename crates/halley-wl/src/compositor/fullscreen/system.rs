use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use std::ops::{Deref, DerefMut};

use super::*;
use crate::compositor::ctx::FullscreenCtx;

pub(crate) fn enter_xdg_fullscreen(
    ctx: &mut FullscreenCtx<'_>,
    node_id: NodeId,
    output: Option<WlOutput>,
    now: Instant,
) {
    ctx.st.enter_xdg_fullscreen(node_id, output, now);
}

pub(crate) fn exit_xdg_fullscreen(ctx: &mut FullscreenCtx<'_>, node_id: NodeId, now: Instant) {
    ctx.st.exit_xdg_fullscreen(node_id, now);
}

pub(crate) fn on_seat_focus_changed(
    ctx: &mut FullscreenCtx<'_>,
    focused: Option<&WlSurface>,
    now: Instant,
) {
    let _ = (ctx, focused, now);
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
        assert_eq!(anim.to_pos, Vec2 { x: 400.0, y: 300.0 });
        assert_eq!(anim.to_size, Vec2 { x: 800.0, y: 600.0 });
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
        assert!(mid_pos.x > 140.0 && mid_pos.x < 400.0);
        assert!(mid_size.x > 320.0 && mid_size.x < 800.0);

        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));
        let (end_pos, end_size) = fullscreen_visual_for_node_on_current_monitor_at(
            &state,
            fullscreen,
            now + std::time::Duration::from_millis(260),
        )
        .expect("end visual");
        assert_eq!(end_pos, Vec2 { x: 400.0, y: 300.0 });
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
    fn fullscreen_roundtrip_preserves_active_maximize_session() {
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

        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut state,
                target,
                now,
                "monitor_a"
            )
        );
        let maximized = state.model.field.node(target).expect("target").clone();
        assert_eq!(maximized.pos, original_pos);
        assert_eq!(maximized.intrinsic_size, original_size);
        assert!(
            crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );

        state.enter_xdg_fullscreen(target, None, now + std::time::Duration::from_millis(20));
        assert!(
            crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));

        state.exit_xdg_fullscreen(target, now + std::time::Duration::from_millis(300));
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(700));

        let restored = state.model.field.node(target).expect("target");
        assert_eq!(restored.pos, maximized.pos);
        assert_eq!(restored.intrinsic_size, maximized.intrinsic_size);
        assert!(
            crate::compositor::workspace::state::maximize_session_active_on_monitor(
                &state,
                "monitor_a"
            )
        );
        assert_eq!(restored.pos, original_pos);
        assert_eq!(restored.intrinsic_size, original_size);
        assert!(!restored.pinned);
    }
}

pub(crate) struct FullscreenController<T> {
    st: T,
}

pub(crate) fn fullscreen_controller<T>(st: T) -> FullscreenController<T> {
    FullscreenController { st }
}

impl<T: Deref<Target = Halley>> Deref for FullscreenController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for FullscreenController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

fn fullscreen_animation_progress(start_ms: u64, duration_ms: u64, now_ms: u64) -> f32 {
    let elapsed = now_ms.saturating_sub(start_ms);
    let t = (elapsed as f32 / duration_ms.max(1) as f32).clamp(0.0, 1.0);
    let e = if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
    };
    e
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
        x: (anim.from_size.x + (anim.to_size.x - anim.from_size.x) * e).max(96.0),
        y: (anim.from_size.y + (anim.to_size.y - anim.from_size.y) * e).max(72.0),
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
    (st.model
        .fullscreen_state
        .fullscreen_active_node
        .get(monitor)
        .copied()
        == Some(node_id))
    .then(|| {
        if let Some(anim) = st
            .model
            .fullscreen_state
            .fullscreen_scale_anim
            .get(&node_id)
            .filter(|anim| anim.monitor == monitor)
        {
            return fullscreen_animation_rect(st, anim, now);
        }
        st.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| (space.viewport.center, space.viewport.size))
            .unwrap_or((st.model.viewport.center, st.model.viewport.size))
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

impl<T: DerefMut<Target = Halley>> FullscreenController<T> {
    fn fullscreen_monitor_name(&self, node_id: NodeId, output: Option<&WlOutput>) -> String {
        output
            .and_then(|requested_output| {
                self.model
                    .monitor_state
                    .outputs
                    .iter()
                    .find_map(|(name, output)| {
                        output.owns(requested_output).then_some(name.clone())
                    })
            })
            .or_else(|| self.model.monitor_state.node_monitor.get(&node_id).cloned())
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone())
    }

    fn fullscreen_monitor_view(&self, monitor_name: &str) -> (Vec2, Vec2) {
        self.model
            .monitor_state
            .monitors
            .get(monitor_name)
            .map(|monitor| (monitor.viewport.center, monitor.viewport.size))
            .unwrap_or((self.model.viewport.center, self.model.viewport.size))
    }

    fn reset_monitor_zoom_once(&mut self, monitor_name: &str) {
        if let Some(monitor) = self.model.monitor_state.monitors.get_mut(monitor_name) {
            monitor.zoom_ref_size = monitor.viewport.size;
            monitor.camera_target_view_size = monitor.viewport.size;
        }
        if self.model.monitor_state.current_monitor == monitor_name {
            self.model.zoom_ref_size = self.model.viewport.size;
            self.model.camera_target_view_size = self.model.viewport.size;
        }
    }

    fn fullscreen_target_size_for(&self, monitor_name: &str) -> (i32, i32) {
        self.model
            .monitor_state
            .outputs
            .get(monitor_name)
            .and_then(|output| output.current_mode())
            .map(|mode| (mode.size.w, mode.size.h))
            .unwrap_or_else(|| {
                let (_, size) = self.fullscreen_monitor_view(monitor_name);
                (
                    size.x.round().max(96.0) as i32,
                    size.y.round().max(72.0) as i32,
                )
            })
    }

    fn fullscreen_suspended_monitor_for_node(&self, node_id: NodeId) -> Option<&str> {
        self.model
            .fullscreen_state
            .fullscreen_suspended_node
            .iter()
            .find_map(|(monitor, &id)| (id == node_id).then_some(monitor.as_str()))
    }

    fn fullscreen_restore_entries_for_monitor(
        &self,
        monitor_name: &str,
        exclude_node: Option<NodeId>,
    ) -> Vec<(
        NodeId,
        crate::compositor::fullscreen::state::FullscreenSessionEntry,
    )> {
        let (monitor_viewport_center, _) = self.fullscreen_monitor_view(monitor_name);
        self.model
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
                let matches_assigned_monitor = self
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

    fn clear_non_target_fullscreen_restore_entries(&mut self, monitor_name: &str, target: NodeId) {
        let stale = self
            .fullscreen_restore_entries_for_monitor(monitor_name, Some(target))
            .into_iter()
            .collect::<Vec<_>>();
        for (id, entry) in stale {
            let _ = self.model.field.set_pinned(id, entry.pinned);
            self.input.interaction_state.physics_velocity.remove(&id);
            self.model.fullscreen_state.fullscreen_restore.remove(&id);
            self.model.fullscreen_state.fullscreen_motion.remove(&id);
            self.model
                .fullscreen_state
                .fullscreen_scale_anim
                .remove(&id);
        }
    }

    fn request_toplevel_fullscreen_state(
        &mut self,
        node_id: NodeId,
        fullscreen: bool,
        output: Option<WlOutput>,
        size: Option<(i32, i32)>,
    ) {
        let monitor_name = if fullscreen {
            self.fullscreen_monitor_name(node_id, output.as_ref())
        } else {
            self.fullscreen_monitor_for_node(node_id)
                .map(str::to_string)
                .or_else(|| self.model.monitor_state.node_monitor.get(&node_id).cloned())
                .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone())
        };
        let focused_node = self
            .last_input_surface_node_for_monitor(monitor_name.as_str())
            .or_else(|| self.last_input_surface_node());
        let bounds_size = if fullscreen {
            size.unwrap_or_else(|| self.fullscreen_target_size_for(monitor_name.as_str()))
        } else {
            let view = self.usable_viewport_for_monitor(&monitor_name);
            (view.size.x as i32, view.size.y as i32)
        };
        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();
            if self.model.surface_to_node.get(&key).copied() != Some(node_id) {
                continue;
            }
            let (min_w, min_h) =
                crate::compositor::surface::toplevel_min_size_for_node(self, node_id);
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
                self.apply_toplevel_tiled_hint(s);
            });
            top.send_configure();
            break;
        }
    }

    /// Returns the monitor name that `node_id` is currently fullscreened on, if any.
    fn exit_xdg_fullscreen_inner(
        &mut self,
        node_id: NodeId,
        _now: Instant,
        suspend: bool,
        preserve_client_fullscreen: bool,
    ) {
        // Find which monitor this node is fullscreened on.
        let monitor_name = match self.fullscreen_monitor_for_node(node_id) {
            Some(m) => m.to_owned(),
            None => return, // not active fullscreen on any monitor
        };

        self.model
            .fullscreen_state
            .clear_direct_scanout_for_monitor(&monitor_name);

        self.input.interaction_state.reset_input_state_requested = true;

        if suspend {
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .insert(monitor_name.clone(), node_id);
            if preserve_client_fullscreen {
                self.model
                    .fullscreen_state
                    .fullscreen_soft_suspended_node
                    .insert(monitor_name.clone(), node_id);
            } else {
                self.model
                    .fullscreen_state
                    .fullscreen_soft_suspended_node
                    .remove(&monitor_name);
            }
        } else {
            // If we're doing a hard exit, clear any suspended state for this monitor too.
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor_name);
            self.model
                .fullscreen_state
                .fullscreen_soft_suspended_node
                .remove(&monitor_name);
        }

        self.clear_non_target_fullscreen_restore_entries(&monitor_name, node_id);

        if let Some(entry) = self
            .model
            .fullscreen_state
            .fullscreen_restore
            .get(&node_id)
            .copied()
        {
            self.restore_fullscreen_snapshot(node_id, entry);
        }

        if preserve_client_fullscreen {
            // Keep the xdg-toplevel protocol fullscreen state intact while the
            // compositor releases its local fullscreen layout lock.
        } else if let Some(entry) = self
            .model
            .fullscreen_state
            .fullscreen_restore
            .get(&node_id)
            .copied()
        {
            let (min_w, min_h) =
                crate::compositor::surface::toplevel_min_size_for_node(self, node_id);
            self.request_toplevel_fullscreen_state(
                node_id,
                false,
                None,
                Some((
                    entry.size.x.round().max(min_w as f32).max(96.0) as i32,
                    entry.size.y.round().max(min_h as f32).max(72.0) as i32,
                )),
            );
        } else {
            self.request_toplevel_fullscreen_state(node_id, false, None, None);
        }

        self.model
            .fullscreen_state
            .fullscreen_active_node
            .remove(&monitor_name);
        self.model
            .fullscreen_state
            .fullscreen_scale_anim
            .remove(&node_id);
        if !suspend {
            self.model
                .fullscreen_state
                .fullscreen_restore
                .remove(&node_id);
        }
        self.request_maintenance();
    }

    #[cfg(test)]
    pub(crate) fn soft_suspend_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        self.exit_xdg_fullscreen_inner(node_id, now, true, true);
    }

    fn restore_fullscreen_snapshot(
        &mut self,
        id: NodeId,
        entry: crate::compositor::fullscreen::state::FullscreenSessionEntry,
    ) {
        if let Some(node) = self.model.field.node_mut(id) {
            node.pos = entry.pos;
            node.intrinsic_size = entry.intrinsic_size;
        }
        let _ = self.model.field.sync_active_footprint_to_intrinsic(id);
        if let Some(loc) = entry.bbox_loc {
            self.ui.render_state.cache.bbox_loc.insert(id, loc);
        } else {
            self.ui.render_state.cache.bbox_loc.remove(&id);
        }
        if let Some(geo) = entry.window_geometry {
            self.ui.render_state.cache.window_geometry.insert(id, geo);
        } else {
            self.ui.render_state.cache.window_geometry.remove(&id);
        }
        self.set_last_active_size_now(id, entry.intrinsic_size);
    }

    pub(crate) fn enter_xdg_fullscreen(
        &mut self,
        node_id: NodeId,
        output: Option<WlOutput>,
        now: Instant,
    ) {
        let monitor_name = self.fullscreen_monitor_name(node_id, output.as_ref());

        self.model
            .fullscreen_state
            .clear_direct_scanout_for_monitor(&monitor_name);

        // Already fullscreen on this monitor — no-op.
        if self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(&monitor_name)
            == Some(&node_id)
        {
            return;
        }

        let soft_resume = self
            .model
            .fullscreen_state
            .fullscreen_soft_suspended_node
            .get(&monitor_name)
            == Some(&node_id);

        if soft_resume {
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor_name);
            self.model
                .fullscreen_state
                .fullscreen_soft_suspended_node
                .remove(&monitor_name);
        } else {
            // Clear any suspended state for this monitor.
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor_name);
            self.model
                .fullscreen_state
                .fullscreen_soft_suspended_node
                .remove(&monitor_name);
        }

        // If another window is fullscreened on the same monitor, exit it first.
        if let Some(existing) = self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(&monitor_name)
            .copied()
        {
            self.exit_xdg_fullscreen(existing, now);
        }

        let target_size = self.fullscreen_target_size_for(monitor_name.as_str());
        let (viewport_center, viewport_size) = self.fullscreen_monitor_view(monitor_name.as_str());
        self.clear_non_target_fullscreen_restore_entries(&monitor_name, node_id);

        // One-time reset of the target monitor's zoom to 1.0. Do not hold or lock it.
        self.reset_monitor_zoom_once(monitor_name.as_str());

        let Some(node) = self.model.field.node(node_id).cloned() else {
            return;
        };

        let soft_resume_entry = soft_resume
            .then(|| {
                self.model
                    .fullscreen_state
                    .fullscreen_restore
                    .get(&node_id)
                    .copied()
            })
            .flatten();
        let saved_size = soft_resume_entry
            .map(|entry| entry.size)
            .unwrap_or_else(|| {
                crate::compositor::surface::current_surface_size_for_node(self, node_id)
                    .unwrap_or(node.intrinsic_size)
            });
        let saved_bbox_loc = soft_resume_entry
            .and_then(|entry| entry.bbox_loc)
            .or_else(|| self.ui.render_state.cache.bbox_loc.get(&node_id).copied());
        let saved_window_geometry = soft_resume_entry
            .and_then(|entry| entry.window_geometry)
            .or_else(|| {
                self.ui
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

        self.model.fullscreen_state.fullscreen_restore.insert(
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
        if self.runtime.tuning.fullscreen_animation_enabled() && !soft_resume {
            self.request_window_animation_prewarm(node_id, now);
            let from = (self.model.monitor_state.current_monitor == monitor_name)
                .then(|| {
                    crate::compositor::workspace::state::maximized_visual_for_node_on_current_monitor_at(
                        self,
                        node_id,
                        now,
                    )
                })
                .flatten()
                .unwrap_or((saved_pos, saved_size));
            let start_ms = self.now_ms(now);
            let duration_ms = self.runtime.tuning.fullscreen_animation_duration_ms();
            self.model.fullscreen_state.fullscreen_scale_anim.insert(
                node_id,
                crate::compositor::fullscreen::state::FullscreenScaleAnim {
                    monitor: monitor_name.clone(),
                    from_pos: from.0,
                    to_pos: viewport_center,
                    from_size: from.1,
                    to_size: viewport_size,
                    start_ms,
                    duration_ms,
                },
            );
        } else {
            self.model
                .fullscreen_state
                .fullscreen_scale_anim
                .remove(&node_id);
        }
        if !soft_resume {
            self.request_toplevel_fullscreen_state(node_id, true, output, Some(target_size));
        }
        self.assign_node_to_monitor(node_id, monitor_name.as_str());
        self.model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor_name, node_id);
        self.set_interaction_focus(Some(node_id), 30_000, now);
        let _ = self.raise_overlap_policy_node(node_id);
        self.request_maintenance();
    }

    pub(crate) fn exit_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        if !self.is_fullscreen_active(node_id)
            && let Some(monitor) = self
                .fullscreen_suspended_monitor_for_node(node_id)
                .map(str::to_string)
        {
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor);
            self.model
                .fullscreen_state
                .fullscreen_active_node
                .insert(monitor, node_id);
        }
        // Clear suspended state on whatever monitor this node is on.
        if let Some(monitor) = self
            .fullscreen_monitor_for_node(node_id)
            .map(|s| s.to_owned())
        {
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor);
        }
        self.exit_xdg_fullscreen_inner(node_id, now, false, false);
    }

    pub(crate) fn drop_fullscreen_surface(&mut self, id: NodeId, _now: Instant) {
        if !self.is_fullscreen_active(id)
            && let Some(monitor) = self
                .fullscreen_suspended_monitor_for_node(id)
                .map(str::to_string)
        {
            self.model
                .fullscreen_state
                .fullscreen_active_node
                .insert(monitor, id);
        }

        // Clear suspended state if this node was suspended on any monitor.
        self.model
            .fullscreen_state
            .fullscreen_suspended_node
            .retain(|_, &mut nid| nid != id);
        self.model
            .fullscreen_state
            .fullscreen_soft_suspended_node
            .retain(|_, &mut nid| nid != id);

        if self.is_fullscreen_active(id) {
            let monitor_name = self
                .fullscreen_monitor_for_node(id)
                .map(|s| s.to_owned())
                .unwrap(); // safe: is_fullscreen_active just confirmed it

            self.input.interaction_state.reset_input_state_requested = true;
            self.model
                .fullscreen_state
                .fullscreen_active_node
                .remove(&monitor_name);

            self.clear_non_target_fullscreen_restore_entries(&monitor_name, id);
        }

        self.model.fullscreen_state.fullscreen_restore.remove(&id);
        self.model.fullscreen_state.fullscreen_motion.remove(&id);
        self.model
            .fullscreen_state
            .fullscreen_scale_anim
            .remove(&id);
        self.model
            .fullscreen_state
            .clear_direct_scanout_for_node(id);
    }

    pub(crate) fn tick_fullscreen_motion(&mut self, now: Instant) {
        if self.model.fullscreen_state.fullscreen_motion.is_empty()
            && self.model.fullscreen_state.fullscreen_scale_anim.is_empty()
        {
            return;
        }

        let now_ms = self.now_ms(now);
        let motions: Vec<(
            NodeId,
            crate::compositor::fullscreen::state::FullscreenMotion,
        )> = self
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
            let e = if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
            };
            let pos = Vec2 {
                x: motion.from.x + (motion.to.x - motion.from.x) * e,
                y: motion.from.y + (motion.to.y - motion.from.y) * e,
            };
            let _ = self.model.field.carry(id, pos);
            if t >= 1.0 {
                finished.push((id, motion));
            }
        }

        for (id, motion) in finished {
            self.model.fullscreen_state.fullscreen_motion.remove(&id);
            if let Some(node) = self.model.field.node_mut(id) {
                node.pos = motion.to;
            }
            self.input.interaction_state.physics_velocity.remove(&id);
            if let Some(entry) = self
                .model
                .fullscreen_state
                .fullscreen_restore
                .get(&id)
                .copied()
            {
                // A node finishing its motion should be pinned only if the fullscreen
                // it was displaced for is still active — i.e. the monitor it belongs
                // to still has an active fullscreen session.
                let node_monitor = self
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
                let displaced_for_active = self
                    .model
                    .fullscreen_state
                    .fullscreen_active_node
                    .contains_key(&node_monitor);

                if displaced_for_active {
                    let _ = self.model.field.set_pinned(id, true);
                } else {
                    let _ = self.model.field.set_pinned(id, entry.pinned);
                    self.model.fullscreen_state.fullscreen_restore.remove(&id);
                }
            }
        }

        self.model
            .fullscreen_state
            .fullscreen_scale_anim
            .retain(|_, anim| now_ms < anim.start_ms.saturating_add(anim.duration_ms));
        if !self.model.fullscreen_state.fullscreen_scale_anim.is_empty() {
            self.request_maintenance();
        }
    }
}
