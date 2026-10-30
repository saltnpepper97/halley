use super::*;

#[test]
fn centers_undersized_client_in_output() {
    assert_eq!(
        center_in_rect((1280, 720).into(), (1920, 0).into(), (2560, 1440).into()),
        (2560, 360).into()
    );
}

#[test]
fn output_reassignment_moves_restore_geometry_and_retires_old_output_endpoints() {
    let mut entry = entry_on_disabled_output();
    entry.restore_kind = FullscreenRestoreKind::WindowedFromMaximize;
    entry.preserve_stack = true;
    let owners = entry.native;
    let location = (-900, 120).into();
    reassign_fullscreen_entry(&mut entry, "fallback", (1280, 720).into(), location);
    assert_eq!(entry.target_output, "fallback");
    assert_eq!(entry.fullscreen_size, (1280, 720).into());
    let restore = entry.restore.as_ref().unwrap();
    assert_eq!(restore.output.as_deref(), Some("fallback"));
    assert_eq!(restore.location, location);
    assert_eq!(
        restore.geometry,
        Rectangle::new((-890, 140).into(), (800, 600).into())
    );
    assert_eq!(entry.presentation_windowed, Some(restore.geometry));
    assert!(entry.presentation_output.is_none());
    assert!(entry.restore_presentation_output.is_none());
    assert!(entry.snapshot_serials.is_empty());
    assert!(entry.native_pending.is_none());
    assert!(entry.transition.is_none());
    assert_eq!(entry.native, owners);
    assert_eq!(
        entry.restore_kind,
        FullscreenRestoreKind::WindowedFromMaximize
    );
    assert!(entry.preserve_stack);
    assert!(entry_occupies_output(&entry, "fallback"));
    assert!(!entry_occupies_output(&entry, "disabled"));
}

#[test]
fn reassigned_native_configure_preserves_client_and_compositor_ownership() {
    for origin in [FullscreenOrigin::Client, FullscreenOrigin::Compositor] {
        let mut entry = test_entry(false);
        request_native_owner(&mut entry, origin);
        reassign_fullscreen_entry(&mut entry, "fallback", (1280, 720).into(), (50, 80).into());
        let mut state = ToplevelState::default();
        state.states.set(State::Fullscreen);
        configure_reassigned_native(&entry, &mut state, (1280, 720).into(), (1280, 720).into());
        assert_eq!(
            state.states.contains(State::Fullscreen),
            origin == FullscreenOrigin::Client
        );
        for edge in [
            State::TiledTop,
            State::TiledBottom,
            State::TiledLeft,
            State::TiledRight,
        ] {
            assert_eq!(
                state.states.contains(edge),
                origin == FullscreenOrigin::Compositor
            );
        }
        assert_eq!(state.size, Some((1280, 720).into()));
        assert_eq!(state.bounds, Some((1280, 720).into()));
        assert!(state.fullscreen_output.is_none());
    }
}

#[test]
fn pending_native_exit_keeps_its_restore_size_on_the_fallback() {
    let mut entry = entry_on_disabled_output();
    release_all_native_owners(&mut entry);
    reassign_fullscreen_entry(
        &mut entry,
        "fallback",
        (1280, 720).into(),
        (-900, 120).into(),
    );
    assert!(!entry.desired);
    assert!(
        entry.active,
        "the new restore configure still needs a matching commit"
    );
    let size = entry.restore.as_ref().unwrap().geometry.size;
    let mut state = ToplevelState::default();
    state.states.set(State::Fullscreen);
    configure_reassigned_native(&entry, &mut state, size, (1280, 720).into());
    assert_eq!(state.size, Some((800, 600).into()));
    assert_eq!(state.bounds, Some((1280, 720).into()));
    assert!(!state.states.contains(State::Fullscreen));
    assert!(native_visual_buffer_matches(&entry, Some(size), false));
    settle_visual_commit(&mut entry, &Animations::default(), Duration::ZERO, false);
    assert!(!entry.active);
}

#[test]
fn reassigned_x11_transaction_can_exit_without_the_disabled_outputs_acknowledgement() {
    let mut entry = entry_on_disabled_output();
    entry.native = None;
    let stale = Rectangle::new((1920, 0).into(), (1920, 1080).into());
    entry.external_pending = Some(ExternalPending {
        geometry: stale,
        presentation: ExternalPresentationKind::Animated,
        geometry_acknowledged: false,
        surface_committed: false,
    });
    reassign_fullscreen_entry(
        &mut entry,
        "fallback",
        (1280, 720).into(),
        (-900, 120).into(),
    );
    finish_external_transition(&mut entry);
    assert!(entry.external_pending.is_none());
    let restore = entry.restore.as_ref().unwrap().geometry;
    assert_eq!(
        begin_external_restore(
            &mut entry,
            ExternalPresentationKind::Opening,
            Duration::ZERO
        ),
        Some(ExternalTransactionRequest::Configure(restore))
    );
    assert_eq!(
        acknowledge_external_geometry(&mut entry, stale, &Animations::default(), Duration::ZERO),
        ExternalConfigureResult::Waiting
    );
    assert_eq!(
        acknowledge_external_geometry(&mut entry, restore, &Animations::default(), Duration::ZERO),
        ExternalConfigureResult::Settled {
            fullscreen: false,
            animated: false
        }
    );
    assert!(!entry.active);
    assert_eq!(
        entry.restore.as_ref().unwrap().output.as_deref(),
        Some("fallback")
    );
}

#[test]
fn client_size_cannot_retarget_a_running_transition() {
    let motion = AnimationMotion::Easing(EasingMotion {
        duration_ms: 400,
        curve: AnimationCurve::Linear,
    });
    let started = Duration::from_secs(1);
    let mut entry = test_entry(true);

    assert!(
        may_adopt_client_size(&entry, started),
        "a settled fullscreen still letterboxes an undersized client"
    );

    entry.transition = Some(MotionTimeline::between(motion, started, 0.0, 1.0, 0.0));
    assert!(!may_adopt_client_size(
        &entry,
        started + Duration::from_millis(200)
    ));
    assert!(may_adopt_client_size(
        &entry,
        started + Duration::from_millis(400)
    ));
}

#[test]
fn global_blur_distinguishes_managed_and_client_fullscreen() {
    assert!(fullscreen_origin_allows_global_blur(
        FullscreenOrigin::Compositor
    ));
    assert!(fullscreen_origin_allows_global_blur(
        FullscreenOrigin::Maximize
    ));
    assert!(!fullscreen_origin_allows_global_blur(
        FullscreenOrigin::Client
    ));
}

#[test]
fn maximize_presentation_reports_maximized_not_fullscreen() {
    let mut state = ToplevelState::default();
    apply_protocol_presentation_state(&mut state, FullscreenOrigin::Maximize, true);

    assert!(state.states.contains(State::Maximized));
    assert!(!state.states.contains(State::Fullscreen));

    apply_protocol_presentation_state(&mut state, FullscreenOrigin::Maximize, false);
    assert!(!state.states.contains(State::Maximized));
    assert!(!state.states.contains(State::Fullscreen));
}

#[test]
fn explicit_fullscreen_still_reports_fullscreen() {
    let mut state = ToplevelState::default();
    apply_protocol_presentation_state(&mut state, FullscreenOrigin::Client, true);

    assert!(state.states.contains(State::Fullscreen));
    assert!(!state.states.contains(State::Maximized));
}

#[test]
fn only_visual_owner_edges_need_a_new_outgoing_snapshot() {
    let mut native = NativeFullscreenState::default();
    assert!(native_owner_change_is_visual(
        &mut native,
        FullscreenOrigin::Compositor,
        true,
    ));
    assert!(!native_owner_change_is_visual(
        &mut native,
        FullscreenOrigin::Client,
        true,
    ));
    assert!(!native_owner_change_is_visual(
        &mut native,
        FullscreenOrigin::Compositor,
        false,
    ));
    assert!(native_owner_change_is_visual(
        &mut native,
        FullscreenOrigin::Client,
        false,
    ));
}

#[test]
fn compositor_fullscreen_stays_protocol_windowed_for_nested_client_reflow() {
    let mut entry = test_entry(false);
    let target = entry.target_output.clone();
    let fullscreen_size = entry.fullscreen_size;

    request_native_owner(&mut entry, FullscreenOrigin::Compositor);
    let compositor_only = entry.native.expect("native state");
    assert!(compositor_only.compositor_requested);
    assert!(!compositor_only.client_requested);
    assert!(!compositor_only.protocol_desired);
    assert!(entry.desired);
    assert_eq!(entry.origin, FullscreenOrigin::Compositor);
    assert_eq!(native_protocol_origin(&entry), FullscreenOrigin::Compositor);
    assert_eq!(
        fullscreen_commit_action(&entry, false),
        FullscreenCommitAction::Visual(true)
    );
    let mut pending = ToplevelState::default();
    apply_protocol_presentation_state(
        &mut pending,
        native_protocol_origin(&entry),
        compositor_only.protocol_desired,
    );
    assert!(!pending.states.contains(State::Fullscreen));
    assert!(!pending.states.contains(State::Maximized));

    entry.active = true;
    entry.presented = true;

    request_native_owner(&mut entry, FullscreenOrigin::Client);
    let nested_set = entry.native.expect("native state");
    assert!(nested_set.compositor_requested);
    assert!(nested_set.client_requested);
    assert!(nested_set.protocol_desired);
    assert!(entry.desired);
    assert_eq!(entry.origin, FullscreenOrigin::Compositor);
    assert_eq!(native_protocol_origin(&entry), FullscreenOrigin::Client);
    assert_eq!(
        fullscreen_commit_action(&entry, true),
        FullscreenCommitAction::ProtocolOnly
    );
    apply_protocol_presentation_state(
        &mut pending,
        native_protocol_origin(&entry),
        nested_set.protocol_desired,
    );
    assert!(pending.states.contains(State::Fullscreen));
    assert!(!pending.states.contains(State::Maximized));

    entry.native.as_mut().expect("native state").protocol_active = true;

    release_native_owner(&mut entry, FullscreenOrigin::Client);
    let nested_unset = entry.native.expect("native state");
    assert!(nested_unset.compositor_requested);
    assert!(!nested_unset.client_requested);
    assert!(!nested_unset.protocol_desired);
    assert!(entry.desired);
    assert!(entry.active);
    assert!(entry.presented);
    assert_eq!(entry.origin, FullscreenOrigin::Compositor);
    assert_eq!(native_protocol_origin(&entry), FullscreenOrigin::Compositor);
    assert_eq!(entry.target_output, target);
    assert_eq!(entry.fullscreen_size, fullscreen_size);
    assert_eq!(
        fullscreen_commit_action(&entry, false),
        FullscreenCommitAction::ProtocolOnly
    );
    apply_protocol_presentation_state(
        &mut pending,
        native_protocol_origin(&entry),
        nested_unset.protocol_desired,
    );
    assert!(!pending.states.contains(State::Fullscreen));
    assert!(!pending.states.contains(State::Maximized));
}

#[test]
fn compositor_fullscreen_from_maximize_keeps_the_maximized_bit() {
    let mut pending = ToplevelState::default();
    apply_protocol_presentation_state_for_request(
        &mut pending,
        FullscreenOrigin::Compositor,
        false,
        true,
    );
    assert!(!pending.states.contains(State::Fullscreen));
    assert!(pending.states.contains(State::Maximized));

    apply_protocol_presentation_state_for_request(
        &mut pending,
        FullscreenOrigin::Compositor,
        false,
        false,
    );
    assert!(!pending.states.contains(State::Fullscreen));
    assert!(!pending.states.contains(State::Maximized));
}

#[test]
fn fullscreen_entry_can_keep_maximized_during_protocol_windowed_mod_f() {
    assert!(!protocol_commit_is_active(false, false, false, true, true));
    assert!(protocol_commit_is_active(false, false, false, true, false));
    assert!(protocol_commit_is_active(false, false, true, false, true));
    assert!(protocol_commit_is_active(true, true, true, false, false));
}

#[test]
fn maximized_buffer_can_start_compositor_fullscreen_from_field_maximize() {
    let mut entering = test_entry(false);
    entering.restore = Some(WindowedPlacement {
        location: (400, 240).into(),
        geometry: Rectangle::new((400, 240).into(), (800, 600).into()),
        output: Some("DP-1".to_string()),
    });
    entering.restore_kind = FullscreenRestoreKind::WindowedFromMaximize;
    assert!(native_visual_buffer_matches(
        &entering,
        Some((1840, 1120).into()),
        true,
    ));
}

#[test]
fn nested_owners_can_be_released_in_either_order() {
    let mut compositor_first = test_entry(false);
    request_native_owner(&mut compositor_first, FullscreenOrigin::Compositor);
    request_native_owner(&mut compositor_first, FullscreenOrigin::Client);
    compositor_first.active = true;

    release_native_owner(&mut compositor_first, FullscreenOrigin::Compositor);
    let retained_client = compositor_first.native.expect("native state");
    assert!(!retained_client.compositor_requested);
    assert!(retained_client.client_requested);
    assert!(retained_client.protocol_desired);
    assert!(compositor_first.desired);
    assert_eq!(compositor_first.origin, FullscreenOrigin::Client);
    assert_eq!(
        fullscreen_commit_action(&compositor_first, true),
        FullscreenCommitAction::ProtocolOnly
    );

    release_native_owner(&mut compositor_first, FullscreenOrigin::Client);
    assert!(!compositor_first.desired);
    assert_eq!(
        fullscreen_commit_action(&compositor_first, false),
        FullscreenCommitAction::Visual(false)
    );

    let mut client_first = test_entry(false);
    request_native_owner(&mut client_first, FullscreenOrigin::Compositor);
    request_native_owner(&mut client_first, FullscreenOrigin::Client);
    client_first.active = true;

    release_native_owner(&mut client_first, FullscreenOrigin::Client);
    assert!(client_first.desired);
    assert_eq!(client_first.origin, FullscreenOrigin::Compositor);
    release_native_owner(&mut client_first, FullscreenOrigin::Compositor);
    assert!(!client_first.desired);
    assert_eq!(
        fullscreen_commit_action(&client_first, false),
        FullscreenCommitAction::Visual(false)
    );
}

#[test]
fn client_owned_fullscreen_can_still_be_released_by_the_client() {
    let mut entry = test_entry(true);

    release_native_owner(&mut entry, FullscreenOrigin::Client);
    assert!(!entry.desired);
    assert_eq!(
        fullscreen_commit_action(&entry, false),
        FullscreenCommitAction::Visual(false)
    );
}
