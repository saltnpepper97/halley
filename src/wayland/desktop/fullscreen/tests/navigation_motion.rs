use super::*;

#[test]
fn fullscreen_hides_top_layer_by_output_without_focus_dependency() {
    let entry = test_entry(true);
    assert!(entry_covers_top(&entry, "DP-1", Duration::ZERO));
    assert!(!entry_covers_top(&entry, "DP-2", Duration::ZERO));
}

#[test]
fn navigation_exit_releases_output_while_retaining_the_visual_handoff() {
    let mut entry = test_entry(true);
    let saved_output = Rectangle::new((120, 80).into(), (800, 600).into());
    entry.presentation_output = Some(saved_output);
    release_all_native_owners(&mut entry);
    entry.navigation_released = true;
    assert!(!entry_owns_output_presentation(&entry));
    assert!(!entry.presentation_paused);
    assert!(entry_owns_presentation(&entry));
    let presentation = fullscreen_presentation(&entry, Duration::ZERO).unwrap();
    assert_eq!(presentation.windowed_output_rect, None);
    // Keep the saved endpoint for later reversals, but do not use its
    // obsolete projection while the navigation camera is moving.
    assert_eq!(entry.presentation_output, Some(saved_output));

    request_native_owner(&mut entry, FullscreenOrigin::Client);
    assert!(entry_owns_output_presentation(&entry));
    assert_eq!(
        fullscreen_presentation(&entry, Duration::ZERO)
            .unwrap()
            .windowed_output_rect,
        Some(saved_output)
    );
}

#[test]
fn cross_output_navigation_preserves_game_presentation_and_protocol() {
    for external in [false, true] {
        let mut entry = test_entry(true);
        if external {
            entry.native = None;
        }
        let size = entry.fullscreen_size;
        let native = entry.native;
        assert!(!pause_entry_on_output(&mut entry, "DP-2"));
        assert!(entry_owns_output_presentation(&entry));
        assert!(entry_presents_immersive(&entry, "DP-1"));
        assert!(!entry.presentation_paused);
        assert!(entry.active && entry.desired);
        assert_eq!(entry.fullscreen_size, size);
        assert_eq!(entry.native, native);

        assert!(pause_entry_on_output(&mut entry, "DP-1"));
        assert!(!entry_owns_output_presentation(&entry));
        assert!(entry.active && entry.desired);
        assert_eq!(entry.fullscreen_size, size);
        assert_eq!(entry.native, native);
        assert!(!pause_entry_on_output(&mut entry, "DP-1"));
    }
}

#[test]
fn hover_return_resumes_only_the_game_owning_the_entered_output() {
    let entry = test_entry(true);
    assert!(resumes_on_output_return(&entry, "DP-1", Some("DP-2")));
    assert!(!resumes_on_output_return(&entry, "DP-1", Some("DP-1")));
    assert!(!resumes_on_output_return(&entry, "DP-2", Some("DP-1")));
    assert!(!resumes_on_output_return(&entry, "DP-1", None));
}

#[test]
fn two_output_navigation_and_hover_return_keep_the_fullscreen_camera_and_bounds() {
    use smithay::output::{PhysicalProperties, Subpixel};
    use smithay::reexports::wayland_server::protocol::wl_surface;
    use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, Display, DisplayHandle};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    struct State;
    impl Dispatch<WlSurface, ()> for State {
        fn request(
            _: &mut Self,
            _: &Client,
            _: &WlSurface,
            _: wl_surface::Request,
            _: &(),
            _: &DisplayHandle,
            _: &mut DataInit<'_, Self>,
        ) {
        }
    }
    let display = Display::<State>::new().unwrap();
    let mut handle = display.handle();
    let (socket, _peer) = UnixStream::pair().unwrap();
    let client = handle.insert_client(socket, Arc::new(())).unwrap();
    let surfaces = (0..3)
        .map(|_| {
            client
                .create_resource::<WlSurface, (), State>(&handle, 1, ())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let output = |name: &str| {
        Output::new(
            name.into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        )
    };
    let primary = output("DP-1");
    let secondary = output("DP-2");
    let bounds = Rectangle::new((0, 0).into(), (2560, 1440).into());
    let now = Duration::ZERO;
    for (external, origin, both) in [
        (false, FullscreenOrigin::Client, false),
        (false, FullscreenOrigin::Compositor, false),
        (false, FullscreenOrigin::Client, true),
        (true, FullscreenOrigin::Client, false),
        (true, FullscreenOrigin::Compositor, false),
    ] {
        let mut manager = FullscreenManager::new(Animations::default());
        let mut game = test_entry(true);
        game.origin = origin;
        game.native = Some(NativeFullscreenState {
            client_requested: origin == FullscreenOrigin::Client || both,
            compositor_requested: origin == FullscreenOrigin::Compositor || both,
            protocol_desired: true,
            protocol_active: true,
        });
        game.fullscreen_size = bounds.size;
        // Reproduce KH's offset normal placement and full-size buffer.
        let restore = Rectangle::new((253, 143).into(), (2560, 1418).into());
        game.restore = Some(WindowedPlacement {
            location: restore.loc,
            geometry: restore,
            output: Some("DP-1".into()),
        });
        if external {
            game.native = None;
        }
        let native = game.native;
        manager.windows.insert(surfaces[0].clone(), game);
        let mut other_game = test_entry(true);
        other_game.target_output = "DP-2".into();
        manager.windows.insert(surfaces[1].clone(), other_game);

        // Filter the departed monitor before game detection: ordinary
        // apps must never reach the exit-fullscreen branch either.
        assert!(
            manager
                .navigation_candidates("DP-2", &surfaces[1])
                .is_empty()
        );
        let candidates = manager.navigation_candidates("DP-2", &surfaces[2]);
        assert_eq!(candidates, vec![surfaces[1].clone()]);
        assert_eq!(
            manager.navigation_candidates("DP-1", &surfaces[2]),
            vec![surfaces[0].clone()]
        );
        for candidate in candidates {
            manager.pause_presentation_on_output(&candidate, "DP-2");
        }
        assert!(!manager.is_presentation_paused(&surfaces[0]));
        assert!(manager.is_presentation_paused(&surfaces[1]));
        assert!(manager.camera_frame(&primary, bounds, now).is_some());
        assert!(manager.camera_frame(&secondary, bounds, now).is_none());
        assert!(manager.covers_top(None, &primary, now));
        assert!(!manager.covers_top(None, &secondary, now));

        // Same-output navigation still parks; local hover cannot undo it.
        assert!(manager.pause_presentation_on_output(&surfaces[0], "DP-1"));
        assert!(manager.camera_frame(&primary, bounds, now).is_none());
        assert!(
            manager
                .resume_presentation_on_output_return(&surfaces[0], "DP-1", Some("DP-1"))
                .is_none()
        );
        assert!(
            manager
                .resume_presentation_on_output_return(&surfaces[0], "DP-2", Some("DP-1"))
                .is_none()
        );
        assert!(manager.is_presentation_paused(&surfaces[0]));

        // Returning from the other monitor restores the exact output rect,
        // not the offset Field placement, and leaves the other game parked.
        assert_eq!(
            manager
                .resume_presentation_on_output_return(&surfaces[0], "DP-1", Some("DP-2"))
                .as_deref(),
            Some("DP-1")
        );
        assert!(!manager.is_presentation_paused(&surfaces[0]));
        assert!(manager.is_presentation_paused(&surfaces[1]));
        let presentation = manager.presentation(&surfaces[0], &primary, now).unwrap();
        assert_eq!(presentation.progress, 1.0);
        assert_eq!(
            presentation.client_rect(restore.to_physical(1), bounds.size.to_physical(1)),
            bounds.to_physical(1)
        );
        assert!(manager.camera_frame(&primary, bounds, now).is_some());
        assert_eq!(manager.windows[&surfaces[0]].native, native);
        assert!(manager.windows[&surfaces[0]].desired && manager.windows[&surfaces[0]].active);
    }
}

#[test]
fn immersive_presentation_excludes_parked_and_maximize() {
    let mut client = test_entry(true);
    assert!(entry_presents_immersive(&client, "DP-1"));
    assert!(!entry_presents_immersive(&client, "DP-2"));

    client.presentation_paused = true;
    assert!(!entry_presents_immersive(&client, "DP-1"));

    let mut maximize = test_entry(true);
    maximize.origin = FullscreenOrigin::Maximize;
    assert!(!entry_presents_immersive(&maximize, "DP-1"));

    let mut compositor = test_entry(true);
    compositor.origin = FullscreenOrigin::Compositor;
    assert!(entry_presents_immersive(&compositor, "DP-1"));
}

#[test]
fn parked_fullscreen_releases_only_output_presentation_ownership() {
    let mut entry = test_entry(true);
    let restore_geometry = Rectangle::new((240, 160).into(), (1280, 720).into());
    entry.restore = Some(WindowedPlacement {
        location: restore_geometry.loc,
        geometry: restore_geometry,
        output: Some("DP-1".to_string()),
    });
    entry.presentation_output = Some(Rectangle::new(
        (0, 0).into(),
        entry.fullscreen_size.to_physical(1),
    ));
    assert!(entry_owns_output_presentation(&entry));
    assert!(entry.desired);
    assert!(entry.active);

    entry.presentation_paused = true;
    let parked = fullscreen_presentation(&entry, Duration::ZERO)
        .expect("parked fullscreen retains a spatial presentation");
    assert!(!entry_owns_output_presentation(&entry));
    assert!(entry.desired);
    assert!(entry.active);
    assert_eq!(parked.progress, 0.0);
    assert_eq!(parked.windowed_geometry, Some(restore_geometry));
    assert_eq!(parked.windowed_output_rect, None);
}

#[test]
fn output_occupancy_includes_transitional_fullscreen_windows() {
    let motion = AnimationMotion::Easing(EasingMotion {
        duration_ms: 400,
        curve: AnimationCurve::Linear,
    });
    let mut entry = test_entry(false);
    entry.transition = Some(MotionTimeline::between(
        motion,
        Duration::ZERO,
        1.0,
        0.0,
        0.0,
    ));

    assert!(entry_occupies_output(&entry, "DP-1"));
    assert!(!entry_occupies_output(&entry, "DP-2"));

    entry.transition = None;
    assert!(!entry_occupies_output(&entry, "DP-1"));
}

#[test]
fn local_killswitch_disables_visual_motion() {
    let mut animations = Animations::default();
    animations.fullscreen.enabled = false;
    assert!(!animations_enabled(&animations));
}

#[test]
fn fullscreen_motion_retargets_without_discontinuity() {
    let mut animations = Animations::default();
    animations.fullscreen.motion = AnimationMotion::Easing(EasingMotion {
        duration_ms: 400,
        curve: AnimationCurve::Linear,
    });
    let mut entry = test_entry(false);
    let started = Duration::from_secs(1);

    retarget_visual(&mut entry, &animations, started, true);
    let forward = entry.transition.expect("forward transition");
    let reversed_at = started + Duration::from_millis(100);
    let value_before_reverse = forward.value_at(reversed_at);

    retarget_visual(&mut entry, &animations, reversed_at, false);
    let reverse = entry.transition.expect("reverse transition");

    assert!(!entry.active);
    assert!((reverse.value_at(reversed_at) - value_before_reverse).abs() < f64::EPSILON);
    assert_eq!(
        reverse.value_at(reversed_at + Duration::from_millis(400)),
        0.0
    );
}

#[test]
fn rapid_fullscreen_reversal_before_commit_preserves_motion() {
    let mut animations = Animations::default();
    animations.fullscreen.motion = AnimationMotion::Easing(EasingMotion {
        duration_ms: 400,
        curve: AnimationCurve::Linear,
    });
    let started = Duration::from_secs(1);
    let reversed_at = started + Duration::from_millis(100);
    let mut entry = test_entry(true);
    entry.desired = false;
    entry.pending_motion = (1.0, 0.0);
    settle_visual_commit(&mut entry, &animations, started, false);

    let value_before_reverse = visual_motion_state(&entry, reversed_at).0;
    freeze_visual_for_configure(&mut entry, reversed_at);
    entry.desired = true;

    assert!(entry.transition.is_none());
    assert_eq!(
        visual_motion_state(&entry, reversed_at).0,
        value_before_reverse,
        "waiting for the reversal configure must hold the current rectangle"
    );

    settle_visual_commit(&mut entry, &animations, reversed_at, true);
    let reverse = entry.transition.expect("reverse transition");
    assert_eq!(reverse.value_at(reversed_at), value_before_reverse);
    assert_eq!(
        reverse.value_at(reversed_at + Duration::from_millis(400)),
        1.0
    );
}

#[test]
fn compositor_fullscreen_remains_commit_gated() {
    let animations = Animations::default();
    let mut entry = test_entry(false);

    request_native_owner(&mut entry, FullscreenOrigin::Compositor);
    assert!(entry.transition.is_none());
    assert!(!entry.active);

    settle_visual_commit(&mut entry, &animations, Duration::ZERO, true);
    assert!(entry.active);
    assert!(entry.transition.is_some());
}

#[test]
fn compositor_fullscreen_requires_an_attached_repaint_buffer() {
    let windowed = WindowedPlacement {
        location: (400, 240).into(),
        geometry: Rectangle::new((400, 240).into(), (800, 600).into()),
        output: Some("DP-1".to_string()),
    };
    let mut entering = test_entry(false);
    entering.restore = Some(windowed.clone());
    assert!(!native_visual_buffer_matches(&entering, None, true));
    assert!(!native_visual_buffer_matches(
        &entering,
        Some(entering.restore.as_ref().unwrap().geometry.size),
        true,
    ));
    assert!(native_visual_buffer_matches(
        &entering,
        Some(entering.fullscreen_size),
        true,
    ));

    let mut exiting = test_entry(true);
    exiting.restore = Some(windowed);
    assert!(!native_visual_buffer_matches(&exiting, None, false));
    assert!(!native_visual_buffer_matches(
        &exiting,
        Some(exiting.fullscreen_size),
        false,
    ));
    assert!(native_visual_buffer_matches(
        &exiting,
        Some(exiting.restore.as_ref().unwrap().geometry.size),
        false,
    ));
}

#[test]
fn constrained_native_client_settles_after_a_bounded_size_wait() {
    let mut entry = pending_compositor_entry();
    let started = Duration::from_secs(1);
    let client_size = (800, 600).into();
    let size = Some(client_size);
    assert!(!native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        size,
        true,
        started
    ));
    assert!(!native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        size,
        true,
        started + NATIVE_SIZE_WAIT - Duration::from_nanos(1),
    ));
    assert!(native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        size,
        true,
        started + NATIVE_SIZE_WAIT,
    ));
    assert_eq!(
        entry.fullscreen_size, client_size,
        "letterbox the client's chosen size"
    );
    settle_visual_commit(
        &mut entry,
        &Animations::default(),
        started + NATIVE_SIZE_WAIT,
        true,
    );
    assert!(entry.active);
    assert!(entry.native_pending.is_none());
}

#[test]
fn stale_configures_and_unpainted_or_missing_buffers_never_arm_the_size_fallback() {
    let mut entry = pending_compositor_entry();
    for (serial, size, ready) in [
        (None, Some((800, 600).into()), true),
        (Some(19.into()), Some((800, 600).into()), true),
        (Some(20.into()), Some((800, 600).into()), false),
        (Some(20.into()), None, true),
    ] {
        assert!(!native_visual_commit_ready(
            &mut entry,
            serial,
            size,
            ready,
            Duration::from_secs(99)
        ));
        assert!(entry.native_pending.unwrap().size_retry_at.is_none());
    }
}

#[test]
fn resized_native_endpoint_does_not_wait_and_late_invalid_trees_still_fail() {
    let mut entry = pending_compositor_entry();
    let size = Some(entry.fullscreen_size);
    assert!(native_visual_commit_ready(
        &mut entry,
        Some(21.into()),
        size,
        true,
        Duration::ZERO
    ));
    assert!(entry.native_pending.unwrap().size_retry_at.is_none());

    let old_size = Some((800, 600).into());
    assert!(!native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        old_size,
        true,
        Duration::ZERO
    ));
    assert!(!native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        old_size,
        false,
        NATIVE_SIZE_WAIT
    ));
    assert!(!native_visual_commit_ready(
        &mut entry,
        Some(19.into()),
        old_size,
        true,
        NATIVE_SIZE_WAIT
    ));
    assert!(native_visual_commit_ready(
        &mut entry,
        Some(21.into()),
        old_size,
        true,
        NATIVE_SIZE_WAIT
    ));
}

#[test]
fn unchanged_native_exit_can_release_its_camera_ownership() {
    let mut entry = pending_compositor_entry();
    entry.desired = false;
    entry.active = true;
    entry.presented = true;
    entry.pending_motion = (1.0, 0.0);
    let size = Some(entry.fullscreen_size);
    assert!(!native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        size,
        true,
        Duration::ZERO
    ));
    assert!(native_visual_commit_ready(
        &mut entry,
        Some(20.into()),
        size,
        true,
        NATIVE_SIZE_WAIT
    ));
    let animations = Animations {
        enabled: false,
        ..Animations::default()
    };
    settle_visual_commit(&mut entry, &animations, NATIVE_SIZE_WAIT, false);
    assert!(!entry.active);
    assert!(!entry.presented);
    assert!(entry.native_pending.is_none());
    assert!(!entry_owns_presentation(&entry));
}
