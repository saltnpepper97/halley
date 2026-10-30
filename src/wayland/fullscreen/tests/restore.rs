use super::*;

#[test]
fn seeded_restore_geometry_wins_over_buffered_fullscreen_geometry() {
    let seeded_geometry = Rectangle::new((960, 480).into(), (640, 480).into());
    let buffered_geometry = Rectangle::new((0, 0).into(), (2560, 1440).into());
    let seeded = WindowedPlacement {
        location: seeded_geometry.loc,
        geometry: seeded_geometry,
        output: Some("DP-1".to_string()),
    };
    let buffered = WindowedPlacement {
        location: buffered_geometry.loc,
        geometry: buffered_geometry,
        output: Some("DP-1".to_string()),
    };

    let fallback = prefer_seeded_restore(None, Some(buffered.clone())).expect("buffered fallback");
    let restore = prefer_seeded_restore(Some(seeded), Some(buffered)).expect("seeded restore");

    assert_eq!(fallback.geometry, buffered_geometry);
    assert_eq!(restore.geometry, seeded_geometry);
    assert_eq!(restore.location, seeded_geometry.loc);
}

#[test]
fn rapid_fullscreen_reentry_keeps_the_original_live_restore_rectangle() {
    let original = Rectangle::new((120, 90).into(), (800, 600).into());
    let intermediate = Rectangle::new((70, 55).into(), (1200, 800).into());
    let mut entry = test_entry(false);

    retain_restore_presentation_output(&mut entry, original);
    retain_restore_presentation_output(&mut entry, intermediate);

    assert_eq!(entry.presentation_output, Some(original));
    assert_eq!(entry.restore_presentation_output, Some(original));
}

#[test]
fn maximize_to_fullscreen_exit_selects_the_real_windowed_endpoint() {
    let floating_world = Rectangle::<i32, Logical>::new((120, 90).into(), (800, 600).into());
    let floating_output = Rectangle::<i32, Physical>::new((120, 90).into(), (800, 600).into());
    let maximized_world = Rectangle::<i32, Logical>::new((20, 20).into(), (1880, 1040).into());
    let maximized_output = Rectangle::<i32, Physical>::new((20, 20).into(), (1880, 1040).into());
    let mut entry = test_entry(true);
    entry.restore = Some(WindowedPlacement {
        location: floating_world.loc,
        geometry: floating_world,
        output: Some("DP-1".to_string()),
    });
    entry.presentation_windowed = Some(maximized_world);
    entry.presentation_output = Some(maximized_output);
    entry.restore_presentation_output = Some(floating_output);

    select_restore_presentation_endpoint(&mut entry);

    assert_eq!(entry.presentation_windowed, Some(floating_world));
    assert_eq!(entry.presentation_output, Some(floating_output));
}

#[test]
fn maximize_fullscreen_off_restores_normal_field_and_cluster_placements() {
    let maximized = Rectangle::new((20, 70).into(), (2520, 1350).into());
    let cases = [
        // Field placement is projected through the pre-maximize camera.
        (
            false,
            Rectangle::new((960, 480).into(), (800, 600).into()),
            Rectangle::new((400, 240).into(), (400, 300).into()),
        ),
        // Single-window and multi-window cluster normal placements.
        (true, maximized, maximized.to_physical(1)),
        (
            true,
            Rectangle::new((1040, 70).into(), (1500, 1350).into()),
            Rectangle::new((1040, 70).into(), (1500, 1350).into()),
        ),
    ];
    for (cluster, normal, normal_output) in cases {
        for origin in [FullscreenOrigin::Client, FullscreenOrigin::Compositor] {
            let mut entry = test_entry(false);
            record_maximize_handoff(
                &mut entry,
                WindowedPlacement {
                    location: normal.loc,
                    geometry: normal,
                    output: Some("DP-1".to_string()),
                },
                Some(normal_output),
                maximized,
                Some(maximized.to_physical(1)),
            );
            request_native_owner(&mut entry, origin);
            entry.active = true;
            if cluster {
                // A layout update during fullscreen must not replace the
                // saved pre-maximize placement or its output projection.
                record_cluster_restore(
                    &mut entry,
                    WindowedPlacement {
                        location: maximized.loc,
                        geometry: maximized,
                        output: Some("DP-1".to_string()),
                    },
                    Some(maximized.to_physical(1)),
                );
            }
            release_all_native_owners(&mut entry);
            select_restore_presentation_endpoint(&mut entry);
            let restore = entry.restore.as_ref().unwrap();
            assert_eq!(restore.geometry, normal);
            assert_eq!(restore.location, normal.loc);
            assert_eq!(entry.presentation_windowed, Some(normal));
            assert_eq!(entry.presentation_output, Some(normal_output));
            assert!(!entry.desired);

            let mut state = ToplevelState::default();
            state.states.set(State::Maximized);
            state.states.set(State::Fullscreen);
            apply_protocol_presentation_state(&mut state, origin, false);
            assert!(!state.states.contains(State::Maximized));
            assert!(!state.states.contains(State::Fullscreen));
            assert_eq!(
                fullscreen_commit_action(&entry, false),
                FullscreenCommitAction::Visual(false),
            );

            // A later fullscreen cycle starts from normal and cannot
            // reintroduce a retired maximize protocol state.
            entry.active = false;
            request_native_owner(&mut entry, origin);
            apply_protocol_presentation_state(
                &mut state,
                native_protocol_origin(&entry),
                entry.native.unwrap().protocol_desired,
            );
            assert!(!state.states.contains(State::Maximized));
            release_all_native_owners(&mut entry);
            select_restore_presentation_endpoint(&mut entry);
            assert_eq!(entry.presentation_windowed, Some(normal));
            assert_eq!(entry.presentation_output, Some(normal_output));
        }
    }
}

#[test]
fn compositor_fullscreen_off_from_maximize_waits_for_the_normal_state() {
    let mut entry = test_entry(false);
    entry.restore_kind = FullscreenRestoreKind::WindowedFromMaximize;
    request_native_owner(&mut entry, FullscreenOrigin::Compositor);
    entry.active = true;
    assert!(retains_maximized_protocol(&entry));

    release_all_native_owners(&mut entry);
    assert!(protocol_commit_is_active(
        false,
        false,
        false,
        true,
        retains_maximized_protocol(&entry),
    ));
    assert!(!protocol_commit_is_active(
        false,
        false,
        false,
        false,
        retains_maximized_protocol(&entry),
    ));
}

#[test]
fn external_maximize_fullscreen_off_restores_normal_and_can_repeat() {
    let animations = Animations::default();
    let maximized = Rectangle::new((20, 70).into(), (2520, 1350).into());
    let fullscreen = Rectangle::new((0, 0).into(), (2560, 1440).into());
    for (normal, normal_output) in [
        (
            Rectangle::new((960, 480).into(), (800, 600).into()),
            Rectangle::new((400, 240).into(), (400, 300).into()),
        ),
        (maximized, maximized.to_physical(1)),
        (
            Rectangle::new((1040, 70).into(), (1500, 1350).into()),
            Rectangle::new((1040, 70).into(), (1500, 1350).into()),
        ),
    ] {
        let mut entry = test_entry(true);
        entry.native = None;
        entry.fullscreen_size = fullscreen.size;
        record_maximize_handoff(
            &mut entry,
            WindowedPlacement {
                location: normal.loc,
                geometry: normal,
                output: Some("DP-1".to_string()),
            },
            Some(normal_output),
            maximized,
            Some(maximized.to_physical(1)),
        );
        for cycle in 1..=2 {
            let now = Duration::from_secs(cycle);
            assert_eq!(
                begin_external_restore(&mut entry, ExternalPresentationKind::Animated, now),
                Some(ExternalTransactionRequest::Configure(normal)),
            );
            assert_eq!(entry.presentation_windowed, Some(normal));
            assert_eq!(entry.presentation_output, Some(normal_output));
            assert_eq!(
                acknowledge_external_geometry(&mut entry, normal, &animations, now),
                ExternalConfigureResult::Waiting,
            );
            assert_eq!(
                acknowledge_external_surface(&mut entry, Some(fullscreen.size), &animations, now,),
                ExternalConfigureResult::Waiting,
            );
            assert_eq!(
                acknowledge_external_surface(&mut entry, Some(normal.size), &animations, now),
                ExternalConfigureResult::Settled {
                    fullscreen: false,
                    animated: true,
                },
            );
            assert!(!entry.desired);
            assert!(!entry.active);
            assert_eq!(entry.restore.as_ref().unwrap().geometry, normal);

            begin_external_transaction(
                &mut entry,
                true,
                fullscreen,
                ExternalPresentationKind::Animated,
                now,
            );
            acknowledge_external_geometry(&mut entry, fullscreen, &animations, now);
            assert_eq!(
                acknowledge_external_surface(&mut entry, Some(fullscreen.size), &animations, now,),
                ExternalConfigureResult::Settled {
                    fullscreen: true,
                    animated: true,
                },
            );
            assert!(entry.active);
        }
    }
}

#[test]
fn ordinary_cluster_fullscreen_exit_still_tracks_the_latest_normal_tile() {
    let mut entry = test_entry(true);
    let normal = Rectangle::new((1040, 70).into(), (1500, 1350).into());
    record_cluster_restore(
        &mut entry,
        WindowedPlacement {
            location: normal.loc,
            geometry: normal,
            output: Some("DP-1".to_string()),
        },
        Some(normal.to_physical(1)),
    );
    release_all_native_owners(&mut entry);
    select_restore_presentation_endpoint(&mut entry);
    assert_eq!(entry.restore.as_ref().unwrap().geometry, normal);
    assert_eq!(entry.presentation_output, Some(normal.to_physical(1)));
    assert!(entry.preserve_stack);
}

#[test]
fn native_exit_ack_keeps_windowed_endpoint_when_buffer_is_still_fullscreen() {
    let windowed_geometry = Rectangle::new((960, 480).into(), (640, 480).into());
    let fullscreen_geometry = Rectangle::new((0, 0).into(), (1920, 1080).into());
    let windowed = WindowedPlacement {
        location: windowed_geometry.loc,
        geometry: windowed_geometry,
        output: Some("DP-1".to_string()),
    };
    let observed = WindowedPlacement {
        location: windowed_geometry.loc,
        geometry: fullscreen_geometry,
        output: Some("DP-1".to_string()),
    };
    let mut entry = test_entry(true);
    entry.desired = false;

    record_native_exit_placement(&mut entry, Some(windowed), observed);

    assert_eq!(entry.restore.as_ref().unwrap().geometry, windowed_geometry);
    assert_eq!(entry.presentation_windowed, Some(windowed_geometry));
}

#[test]
fn native_exit_uses_a_client_constrained_restore_size() {
    let requested = Rectangle::new((960, 480).into(), (252, 304).into());
    let constrained = Rectangle::new((960, 480).into(), (500, 304).into());
    let seeded = WindowedPlacement {
        location: requested.loc,
        geometry: requested,
        output: Some("DP-1".to_string()),
    };
    let observed = WindowedPlacement {
        location: constrained.loc,
        geometry: constrained,
        output: Some("DP-1".to_string()),
    };
    let mut entry = test_entry(true);
    entry.desired = false;

    record_native_exit_placement(&mut entry, Some(seeded), observed);

    assert_eq!(entry.restore.as_ref().unwrap().geometry, constrained);
    assert_eq!(entry.presentation_windowed, Some(constrained));
}
