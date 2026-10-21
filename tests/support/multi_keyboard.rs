//! Model libinput's per-key holder count, then exercise real input and clients.
use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; overlapping physical key counts"]
fn overlapping_keys_keep_client_and_sided_modifiers_until_final_release() {
    let fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (mut queue, mut client) = native_window(&fixture);
    let _keyboard = observe_keyboard(&mut queue, &mut client);
    wait_for("test client keyboard focus", || {
        queue.roundtrip(&mut client).unwrap();
        client.keyboard_focused.then_some(())
    });
    // Both sides of every modifier, plus an ordinary letter. Hold the same
    // physical key on three keyboards, then release them one at a time.
    for (code, modifier, side) in [
        (42, Some(0), Some(0)),
        (54, Some(0), Some(1)),
        (29, Some(1), Some(2)),
        (97, Some(1), Some(3)),
        (56, Some(2), Some(4)),
        (100, Some(2), Some(5)),
        (125, Some(3), Some(6)),
        (126, Some(3), Some(7)),
        (30, None, None),
    ] {
        client.keyboard_events.clear();
        input.seat_key(code, true, 1);
        input.seat_key(code, true, 2);
        input.seat_key(code, true, 3);
        for count in [2, 1] {
            let snapshot = input.seat_key(code, false, count);
            assert!(
                snapshot["keyboard"]["pressed"]
                    .as_array()
                    .unwrap()
                    .contains(&code.into()),
                "premature release: {snapshot}"
            );
            if let (Some(modifier), Some(side)) = (modifier, side) {
                assert_eq!(
                    snapshot["keyboard"]["modifiers"][modifier], true,
                    "{snapshot}"
                );
                assert_eq!(snapshot["keyboard"]["sides"][side], true, "{snapshot}");
            }
            queue.roundtrip(&mut client).unwrap();
            assert_eq!(
                client.keyboard_events,
                [(code, wl_keyboard::KeyState::Pressed)]
            );
        }
        let released = input.seat_key(code, false, 0);
        assert_eq!(released["keyboard"]["pressed"], serde_json::json!([]));
        assert_eq!(
            released["keyboard"]["modifiers"],
            serde_json::json!([false, false, false, false])
        );
        assert_eq!(
            released["keyboard"]["sides"],
            serde_json::json!([false, false, false, false, false, false, false, false])
        );
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(
            client.keyboard_events,
            [
                (code, wl_keyboard::KeyState::Pressed),
                (code, wl_keyboard::KeyState::Released)
            ]
        );
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; X11 overlapping Shift"]
fn x11_client_keeps_shift_until_final_keyboard_releases_it() {
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, EventMask, KeyButMask};
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let window = x11_window(&connection, "overlapping keyboard X11 target");
    connection
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    let id = fixture.node("overlapping keyboard X11 target").id;
    request_native_focus(&mut fixture, id);
    wait_for("X11 keyboard focus", || {
        (connection.get_input_focus().unwrap().reply().unwrap().focus == window).then_some(())
    });
    input.seat_key(42, true, 1);
    input.seat_key(42, true, 2);
    input.seat_key(42, false, 1);
    input.seat_key(30, true, 1);
    input.seat_key(30, false, 0);
    input.seat_key(42, false, 0);
    input.seat_key(30, true, 1);
    input.seat_key(30, false, 0);
    let mut keys = Vec::new();
    wait_for("X11 receives all logical key transitions", || {
        while let Some(event) = connection.poll_for_event().unwrap() {
            match event {
                Event::KeyPress(event) => keys.push((true, event.detail, event.state)),
                Event::KeyRelease(event) => keys.push((false, event.detail, event.state)),
                _ => {}
            }
        }
        (keys.len() >= 6).then_some(())
    });
    assert_eq!(
        keys.iter()
            .map(|&(down, code, _)| (down, code))
            .collect::<Vec<_>>(),
        [
            (true, 50),
            (true, 38),
            (false, 38),
            (false, 50),
            (true, 38),
            (false, 38)
        ]
    );
    assert!(keys[1].2.contains(KeyButMask::SHIFT));
    assert!(!keys[4].2.contains(KeyButMask::SHIFT));
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; overlapping Alt+Tab and lock keys"]
fn overlapping_alt_does_not_commit_switcher_and_lock_keys_toggle_once() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (mut first_queue, mut first) = native_window(&fixture);
    let (mut second_queue, mut second) = native_window_named(
        &fixture,
        "second keyboard target",
        "halley.test.keyboard",
        [0, 0, u32::MAX, u32::MAX],
    );
    wait_for("second client", || {
        first_queue.roundtrip(&mut first).unwrap();
        second_queue.roundtrip(&mut second).unwrap();
        (fixture.nodes().len() == 2).then_some(())
    });
    input.seat_key(56, true, 1);
    input.seat_key(56, true, 2);
    input.seat_key(15, true, 1);
    let opened = input.seat_key(15, false, 0);
    assert_eq!(opened["open"], true);
    let preview = opened["preview"].clone();
    let partial = input.seat_key(56, false, 1);
    assert_eq!(
        partial["open"], true,
        "first keyboard must not commit: {partial}"
    );
    let committed = input.seat_key(56, false, 0);
    assert_eq!(committed["open"], false);
    assert_eq!(committed["focused"], preview);
    for (code, name) in [(58, "caps"), (69, "num")] {
        assert_eq!(input.seat_key(code, true, 1)["leds"][name], true);
        assert_eq!(input.seat_key(code, true, 2)["leds"][name], true);
        assert_eq!(input.seat_key(code, false, 1)["leds"][name], true);
        // Re-press on one keyboard while the other still holds the lock key.
        assert_eq!(input.seat_key(code, true, 2)["leds"][name], true);
        assert_eq!(input.seat_key(code, false, 1)["leds"][name], true);
        assert_eq!(input.seat_key(code, false, 0)["leds"][name], true);
        input.seat_key(code, true, 1);
        assert_eq!(input.seat_key(code, false, 0)["leds"][name], false);
    }
}
