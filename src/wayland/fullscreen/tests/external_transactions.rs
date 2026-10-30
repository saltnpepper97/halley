use super::*;

#[test]
fn fullscreen_transition_owns_the_zero_progress_handoff_frame() {
    assert!(fullscreen_presentation_is_visible(0.0, true));
    assert!(fullscreen_presentation_is_visible(0.5, true));
    assert!(fullscreen_presentation_is_visible(1.0, false));
    assert!(!fullscreen_presentation_is_visible(0.0, false));
}

#[test]
fn acknowledged_fullscreen_exit_remains_available_for_maximize_handoff() {
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
    let surface = client
        .create_resource::<WlSurface, (), State>(&handle, 1, ())
        .unwrap();
    let mut animations = Animations::default();
    animations.fullscreen.motion = AnimationMotion::Easing(EasingMotion {
        duration_ms: 400,
        curve: AnimationCurve::Linear,
    });
    let started = Duration::from_secs(1);
    for external in [false, true] {
        let mut manager = FullscreenManager::new(animations.clone());
        let mut entry = test_entry(true);
        if external {
            entry.native = None;
        }
        entry.desired = false;
        freeze_visual_for_configure(&mut entry, started);
        settle_visual_commit(&mut entry, &animations, started, false);
        manager.windows.insert(surface.clone(), entry);
        assert!(!manager.is_fullscreen_or_pending(&surface));
        for elapsed in [0, 100, 399, 400] {
            let now = started + Duration::from_millis(elapsed);
            assert!(
                manager.has_presentation(&surface, now),
                "maximize must retire the exit presentation, including its last frame; external={external} elapsed={elapsed}"
            );
        }
        manager.cleanup(started + Duration::from_millis(400));
        assert!(!manager.has_presentation(&surface, started + Duration::from_millis(400)));
    }
}

#[test]
fn pending_configure_keeps_the_outgoing_source_presented() {
    let mut entry = test_entry(false);
    entry.desired = true;

    assert!(
        entry_owns_presentation(&entry),
        "the handoff source must survive until the client acknowledges fullscreen"
    );

    entry.active = true;
    assert!(!entry_owns_presentation(&entry));
}

#[test]
fn fullscreen_motion_killswitch_still_applies_state() {
    let mut animations = Animations::default();
    animations.fullscreen.enabled = false;
    let mut entry = test_entry(false);

    retarget_visual(&mut entry, &animations, Duration::ZERO, true);

    assert!(!entry.active);
    assert!(entry.presented);
    assert!(entry.transition.is_none());
}

#[test]
fn external_fullscreen_is_logically_settled_without_animation() {
    let animations = Animations::default();
    let mut entry = test_entry(false);
    retarget_visual(&mut entry, &animations, Duration::from_secs(1), true);

    settle_external_fullscreen(&mut entry, "HDMI-A-1", (2560, 1440).into());

    assert!(entry.desired);
    assert!(entry.active);
    assert_eq!(entry.target_output, "HDMI-A-1");
    assert_eq!(entry.fullscreen_size, Size::from((2560, 1440)));
    assert!(entry.transition.is_none());
}

#[test]
fn external_mod_f_waits_for_x11_geometry_and_surface_commit() {
    let animations = Animations::default();
    let mut entry = test_entry(false);
    let target = Rectangle::new((0, 0).into(), (1920, 1080).into());
    let intermediate = Rectangle::new((0, 0).into(), (1280, 720).into());

    assert_eq!(
        begin_external_transaction(
            &mut entry,
            true,
            target,
            ExternalPresentationKind::Animated,
            Duration::ZERO,
        ),
        ExternalTransactionRequest::Configure(target)
    );
    assert_eq!(
        acknowledge_external_surface(&mut entry, Some(target.size), &animations, Duration::ZERO,),
        ExternalConfigureResult::Waiting
    );
    assert!(!entry.external_pending.unwrap().surface_committed);

    assert_eq!(
        acknowledge_external_geometry(&mut entry, intermediate, &animations, Duration::ZERO),
        ExternalConfigureResult::Waiting
    );
    assert!(entry.transition.is_none());

    assert_eq!(
        acknowledge_external_geometry(&mut entry, target, &animations, Duration::from_secs(1),),
        ExternalConfigureResult::Waiting
    );
    assert!(!entry.active);
    assert!(entry.transition.is_none());
    assert!(!entry.external_pending.unwrap().surface_committed);

    assert_eq!(
        acknowledge_external_surface(
            &mut entry,
            Some(target.size),
            &animations,
            Duration::from_secs(1),
        ),
        ExternalConfigureResult::Settled {
            fullscreen: true,
            animated: true,
        }
    );
    assert!(entry.active);
    assert!(entry.transition.is_some());
    assert!(entry.external_pending.is_none());
}

#[test]
fn duplicate_external_request_preserves_the_active_transaction() {
    let mut entry = test_entry(false);
    let target = Rectangle::new((0, 0).into(), (1920, 1080).into());
    begin_external_transaction(
        &mut entry,
        true,
        target,
        ExternalPresentationKind::Animated,
        Duration::ZERO,
    );
    let pending = entry.external_pending;

    assert!(desired_matches(Some(&entry), true));
    assert!(!desired_matches(Some(&entry), false));
    assert!(!desired_matches(None, true));
    assert_eq!(entry.external_pending, pending);
}

#[test]
fn live_external_geometry_animates_without_waiting_for_a_resized_buffer() {
    let animations = Animations::default();
    let mut entry = test_entry(false);
    let target = Rectangle::new((0, 0).into(), (1920, 1080).into());
    begin_external_transaction(
        &mut entry,
        true,
        target,
        ExternalPresentationKind::LiveAnimated,
        Duration::ZERO,
    );
    assert_eq!(
        acknowledge_external_geometry(&mut entry, target, &animations, Duration::ZERO),
        ExternalConfigureResult::Settled {
            fullscreen: true,
            animated: true
        }
    );
    assert!(entry.external_pending.is_none());
    let transition = entry
        .transition
        .expect("live fullscreen keeps its geometry timeline");
    let halfway = transition.value_at(Duration::from_millis(50));
    assert!(halfway > 0.0 && halfway < 1.0);
}

#[test]
fn chrome_follows_logical_fullscreen_state_instead_of_visual_progress() {
    let mut entry = test_entry(false);
    assert!(!fullscreen_entry_suppresses_chrome(&entry));

    entry.desired = true;
    assert!(fullscreen_entry_suppresses_chrome(&entry));

    entry.origin = FullscreenOrigin::Compositor;
    assert!(fullscreen_entry_suppresses_chrome(&entry));

    entry.origin = FullscreenOrigin::Maximize;
    assert!(!fullscreen_entry_suppresses_chrome(&entry));

    entry.origin = FullscreenOrigin::Client;
    entry.desired = false;
    entry.active = true;
    entry.presented = true;
    assert!(!fullscreen_entry_suppresses_chrome(&entry));
}

#[test]
fn exit_chrome_fades_during_settling_and_finishes_before_motion_cleanup() {
    let mut entry = test_entry(false);
    let start = Duration::from_secs(1);
    entry.transition = Some(MotionTimeline::between(
        AnimationMotion::Easing(EasingMotion {
            duration_ms: 400,
            curve: AnimationCurve::Linear,
        }),
        start,
        1.0,
        0.0,
        0.0,
    ));
    for (millis, expected) in [
        (0, 0.0),
        (100, 0.0),
        (260, 0.0),
        (310, 0.5),
        (360, 1.0),
        (400, 1.0),
        (500, 1.0),
    ] {
        let alpha =
            fullscreen_entry_chrome_alpha(&entry, true, start + Duration::from_millis(millis));
        assert!((alpha - expected).abs() < 0.0001, "{millis}ms: {alpha}");
    }
    entry.transition = Some(MotionTimeline::between(
        AnimationMotion::Easing(EasingMotion {
            duration_ms: 50,
            curve: AnimationCurve::Linear,
        }),
        start,
        1.0,
        0.0,
        0.0,
    ));
    assert_eq!(fullscreen_entry_chrome_alpha(&entry, true, start), 0.0);
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, start + Duration::from_micros(38750)),
        0.5
    );
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, start + Duration::from_millis(50)),
        1.0
    );
}

#[test]
fn chrome_waits_for_exit_commit_and_respects_disabled_motion_and_maximize() {
    let mut entry = test_entry(true);
    entry.desired = false;
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, Duration::ZERO),
        0.0
    );
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, false, Duration::ZERO),
        1.0
    );
    entry.active = false;
    entry.presented = false;
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, Duration::ZERO),
        1.0
    );
    entry.transition = Some(MotionTimeline::between(
        AnimationMotion::Easing(EasingMotion {
            duration_ms: 0,
            curve: AnimationCurve::Linear,
        }),
        Duration::ZERO,
        1.0,
        0.0,
        0.0,
    ));
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, Duration::ZERO),
        1.0
    );
    entry.desired = true;
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, Duration::ZERO),
        0.0
    );
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, false, Duration::ZERO),
        0.0
    );
    entry.origin = FullscreenOrigin::Maximize;
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, Duration::ZERO),
        1.0
    );
}

#[test]
fn spring_exit_chrome_is_monotonic_even_when_window_motion_overshoots() {
    let mut entry = test_entry(false);
    let transition = MotionTimeline::between(
        AnimationMotion::Spring(halley_config::SpringMotion {
            damping_ratio: 0.4,
            stiffness: 800.0,
        }),
        Duration::ZERO,
        1.0,
        0.0,
        0.0,
    );
    entry.transition = Some(transition);
    let mut previous = 0.0;
    let mut overshot = false;
    for millis in 0..=transition.duration().as_millis() as u64 + 1 {
        let now = Duration::from_millis(millis);
        overshot |= transition.value_at(now) < 0.0;
        let alpha = fullscreen_entry_chrome_alpha(&entry, true, now);
        assert!(alpha >= previous && alpha <= 1.0);
        previous = alpha;
    }
    assert!(overshot);
    assert_eq!(previous, 1.0);
    // Re-entering fullscreen hides chrome immediately, even mid-fade.
    entry.desired = true;
    assert_eq!(
        fullscreen_entry_chrome_alpha(&entry, true, transition.duration()),
        0.0
    );
}

#[test]
fn settled_windowed_resize_can_replace_the_saved_restore() {
    let mut entry = test_entry(false);
    assert!(can_update_external_restore(&entry));

    entry.desired = true;
    assert!(!can_update_external_restore(&entry));

    entry.desired = false;
    entry.external_pending = Some(ExternalPending {
        geometry: Rectangle::new((0, 0).into(), (1920, 1080).into()),
        presentation: ExternalPresentationKind::Animated,
        geometry_acknowledged: false,
        surface_committed: false,
    });
    assert!(!can_update_external_restore(&entry));
}

#[test]
fn external_state_churn_keeps_only_the_latest_geometry() {
    let mut entry = test_entry(false);
    let fullscreen = Rectangle::new((0, 0).into(), (1920, 1080).into());
    let restore = Rectangle::new((320, 180).into(), (1280, 720).into());

    begin_external_transaction(
        &mut entry,
        true,
        fullscreen,
        ExternalPresentationKind::Opening,
        Duration::ZERO,
    );
    begin_external_transaction(
        &mut entry,
        false,
        restore,
        ExternalPresentationKind::Opening,
        Duration::ZERO,
    );
    begin_external_transaction(
        &mut entry,
        true,
        fullscreen,
        ExternalPresentationKind::Opening,
        Duration::ZERO,
    );

    assert_eq!(
        entry.external_pending,
        Some(ExternalPending {
            geometry: fullscreen,
            presentation: ExternalPresentationKind::Opening,
            geometry_acknowledged: false,
            surface_committed: true,
        })
    );
    assert!(entry.desired);
    assert!(entry.transition.is_none());
}

#[test]
fn opening_transaction_settles_without_a_second_animation() {
    let animations = Animations::default();
    let mut entry = test_entry(false);
    let target = Rectangle::new((0, 0).into(), (1920, 1080).into());

    begin_external_transaction(
        &mut entry,
        true,
        target,
        ExternalPresentationKind::Opening,
        Duration::ZERO,
    );

    assert_eq!(
        acknowledge_external_geometry(&mut entry, target, &animations, Duration::ZERO),
        ExternalConfigureResult::Settled {
            fullscreen: true,
            animated: false,
        }
    );
    assert!(entry.active);
    assert!(entry.transition.is_none());
}

#[test]
fn finishing_external_animation_snaps_to_desired_state() {
    let animations = Animations::default();
    let mut entry = test_entry(false);
    let target = Rectangle::new((0, 0).into(), (1920, 1080).into());
    begin_external_transaction(
        &mut entry,
        true,
        target,
        ExternalPresentationKind::Animated,
        Duration::ZERO,
    );
    acknowledge_external_geometry(&mut entry, target, &animations, Duration::from_secs(1));
    acknowledge_external_surface(
        &mut entry,
        Some(target.size),
        &animations,
        Duration::from_secs(1),
    );
    assert!(entry.transition.is_some());

    finish_external_transition(&mut entry);

    assert!(entry.active);
    assert!(entry.desired);
    assert!(entry.external_pending.is_none());
    assert!(entry.transition.is_none());
}
