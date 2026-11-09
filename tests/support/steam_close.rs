//! Deliberately uncooperative X11 clients exercising real input and rendering.
use super::fullscreen_multi_output::Input;
use super::*;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, EventMask, MapState, PropMode};

fn atom(connection: &RustConnection, name: &[u8]) -> u32 {
    connection
        .intern_atom(false, name)
        .unwrap()
        .reply()
        .unwrap()
        .atom
}

fn steam_window(connection: &RustConnection, title: &str, pixel: u32) -> u32 {
    let screen = &connection.setup().roots[0];
    let window = connection.generate_id().unwrap();
    connection
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            30,
            30,
            260,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(pixel),
        )
        .unwrap()
        .check()
        .unwrap();
    connection
        .change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .unwrap()
        .check()
        .unwrap();
    connection
        .change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"steamwebhelper\0steam\0",
        )
        .unwrap()
        .check()
        .unwrap();
    connection
        .change_property32(
            PropMode::REPLACE,
            window,
            atom(connection, b"_NET_WM_WINDOW_TYPE"),
            AtomEnum::ATOM,
            &[atom(connection, b"_NET_WM_WINDOW_TYPE_NORMAL")],
        )
        .unwrap()
        .check()
        .unwrap();
    let motif = atom(connection, b"_MOTIF_WM_HINTS");
    connection
        .change_property32(PropMode::REPLACE, window, motif, motif, &[2, 0, 0, 0, 0])
        .unwrap()
        .check()
        .unwrap();
    connection
        .change_property32(
            PropMode::REPLACE,
            window,
            atom(connection, b"WM_PROTOCOLS"),
            AtomEnum::ATOM,
            &[atom(connection, b"WM_DELETE_WINDOW")],
        )
        .unwrap()
        .check()
        .unwrap();
    connection
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::BUTTON_PRESS),
        )
        .unwrap()
        .check()
        .unwrap();
    connection.map_window(window).unwrap().check().unwrap();
    connection.flush().unwrap();
    window
}

fn deletes(connection: &RustConnection) -> Vec<u32> {
    let protocols = atom(connection, b"WM_PROTOCOLS");
    let delete = atom(connection, b"WM_DELETE_WINDOW");
    let mut windows = Vec::new();
    while let Some(event) = connection.poll_for_event().unwrap() {
        if let x11rb::protocol::Event::ClientMessage(event) = event
            && event.type_ == protocols
            && event.data.as_data32()[0] == delete
        {
            windows.push(event.window);
        }
    }
    windows
}

fn withdraw(connection: &RustConnection, window: u32) {
    let root = connection.setup().roots[0].root;
    connection.unmap_window(window).unwrap().check().unwrap();
    connection
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            x11rb::protocol::xproto::UnmapNotifyEvent {
                response_type: x11rb::protocol::xproto::UNMAP_NOTIFY_EVENT,
                sequence: 0,
                event: root,
                window,
                from_configure: false,
            },
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
}

fn white_pixels(fixture: &mut Fixture) -> usize {
    capture_frame_pixels_on(fixture, Some("winit"))
        .0
        .chunks_exact(4)
        .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 250))
        .count()
}

fn paint(connection: &RustConnection, window: u32, pixel: u32) {
    connection
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().background_pixel(pixel),
        )
        .unwrap()
        .check()
        .unwrap();
    connection
        .clear_area(false, window, 0, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
}

fn rect(input: &Input, id: u64) -> serde_json::Value {
    input.state()["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|window| window["id"] == id)
        .unwrap()["rect"]
        .clone()
}

fn focus(fixture: &mut Fixture, id: u64) {
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn steam_shortcut_close_is_immediate_reversible_and_passes_input_to_survivors() {
    for (animated, mode) in [(true, "click"), (false, "click"), (true, "hover")] {
        let mut fixture = Fixture::with_two_outputs(&format!(
            "animations:\n enabled {animated}\n window-open:\n enabled false\n end\n window-close:\n duration-ms 100\n end\nend\ninput:\n focus-mode \"{mode}\"\nend\nkeybinds:\n \"alt+q\" \"close-focused\" with repeat true\nend\n"
        ));
        let input = Input::new(&fixture);
        let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
        let main = steam_window(&connection, "Steam", 0x004400);
        let main_id = fixture.node("Steam").id;
        let dialog = steam_window(&connection, "Steam Settings", 0xffffff);
        let dialog_id = fixture.node("Steam Settings").id;
        focus(&mut fixture, dialog_id);
        wait_for("Steam fixture has visible client pixels", || {
            (white_pixels(&mut fixture) > 10_000).then_some(())
        });
        let before = rect(&input, dialog_id);
        deletes(&connection);
        input.key(56, true);
        input.key(16, true);
        wait_for(
            "advisory delete delivered before the client cooperates",
            || deletes(&connection).contains(&dialog).then_some(()),
        );
        assert!(
            rect(&input, dialog_id).is_null(),
            "closing window is no longer an input target"
        );
        assert_eq!(input.state()["focused"], main_id);
        wait_for(
            "real X11 keyboard focus handed to surviving Steam window",
            || (connection.get_input_focus().unwrap().reply().unwrap().focus == main).then_some(()),
        );
        wait_for("client disappears while still mapped", || {
            (white_pixels(&mut fixture) < 1000).then_some(())
        });
        assert_eq!(
            connection
                .get_window_attributes(dialog)
                .unwrap()
                .reply()
                .unwrap()
                .map_state,
            MapState::VIEWABLE
        );
        assert!(
            fixture.nodes().iter().any(|node| node.id == dialog_id),
            "visual close must not retire client lifetime"
        );
        // A held close shortcut must not repeat onto the new focus owner.
        thread::sleep(Duration::from_millis(650));
        input.key(16, false);
        input.key(56, false);
        assert!(
            deletes(&connection).is_empty(),
            "held close repeated onto the survivor"
        );
        // Click where the hidden window used to be, while its X11 window is
        // still mapped. Neither keyboard nor pointer may reach it.
        input.motion(
            before[0].as_f64().unwrap() + before[2].as_f64().unwrap() / 2.0,
            before[1].as_f64().unwrap() + before[3].as_f64().unwrap() / 2.0,
        );
        input.button(272, true);
        input.button(272, false);
        connection.get_input_focus().unwrap().reply().unwrap();
        while let Some(event) = connection.poll_for_event().unwrap() {
            if let x11rb::protocol::Event::ButtonPress(event) = event {
                assert_ne!(event.event, dialog, "invisible window intercepted a click");
            }
        }
        focus(&mut fixture, main_id);
        // Refuse the request: timeout must restore live content without taking
        // focus back from the survivor.
        wait_for("refused close restores input eligibility", || {
            (!rect(&input, dialog_id).is_null()).then_some(())
        });
        assert!(!rect(&input, dialog_id).is_null());
        assert_eq!(input.state()["focused"], main_id);
        assert_eq!(
            connection.get_input_focus().unwrap().reply().unwrap().focus,
            main
        );
        // Try again, this time withdrawing after the immediate animation.
        focus(&mut fixture, dialog_id);
        wait_for("restored window has live pixels when raised", || {
            (white_pixels(&mut fixture) > 10_000).then_some(())
        });
        paint(&connection, dialog, 0x0000ff);
        wait_for(
            "recovery restores live repaints rather than a frozen snapshot",
            || {
                let pixels = capture_frame_pixels_on(&mut fixture, Some("winit")).0;
                (pixels
                    .chunks_exact(4)
                    .filter(|pixel| pixel[0] > 250 && pixel[1] < 5 && pixel[2] < 5)
                    .count()
                    > 10_000)
                    .then_some(())
            },
        );
        paint(&connection, dialog, 0xffffff);
        input.key(65, true); // F7: another binding to the real close action.
        input.key(65, false);
        assert!(rect(&input, dialog_id).is_null());
        withdraw(&connection, dialog);
        wait_for("actual withdrawal retires only the closed window", || {
            fixture
                .nodes()
                .iter()
                .all(|node| node.id != dialog_id)
                .then_some(())
        });
        connection.map_window(dialog).unwrap().check().unwrap();
        connection.flush().unwrap();
        let reopened = fixture.node("Steam Settings").id;
        focus(&mut fixture, reopened);
        wait_for("reopened XID has live pixels", || {
            (white_pixels(&mut fixture) > 10_000).then_some(())
        });
        // The previous timeout must not cancel this reopened window's next
        // close. The separate generation regression also covers shared state.
        input.key(65, true);
        input.key(65, false);
        assert!(rect(&input, reopened).is_null());
        wait_for("second refusal restores the reopened window", || {
            (!rect(&input, reopened).is_null()).then_some(())
        });
        assert_eq!(input.state()["focused"], main_id);
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn steam_shortcut_close_preserves_main_collapsed_and_native_advisory_close() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let main = steam_window(&connection, "Steam", 0x004400);
    let main_id = fixture.node("Steam").id;
    focus(&mut fixture, main_id);
    input.key(65, true);
    input.key(65, false);
    wait_for("main window receives normal delete", || {
        deletes(&connection).contains(&main).then_some(())
    });
    assert!(!rect(&input, main_id).is_null());
    assert_eq!(input.state()["focused"], main_id);

    let dialog = steam_window(&connection, "Friends List", 0xffffff);
    let id = fixture.node("Friends List").id;
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    input.key(65, true);
    input.key(65, false);
    wait_for("collapsed Steam window receives normal delete", || {
        deletes(&connection).contains(&dialog).then_some(())
    });
    assert_eq!(
        fixture.node("Friends List").state,
        halley_ipc::NodeState::Node
    );

    let (mut queue, mut client) = native_window(&fixture);
    let native = fixture.node("surviving native window").id;
    focus(&mut fixture, native);
    input.key(65, true);
    input.key(65, false);
    wait_for("native close remains advisory", || {
        queue.roundtrip(&mut client).unwrap();
        client.close_requested.then_some(())
    });
    assert!(!rect(&input, native).is_null());
    assert_eq!(input.state()["focused"], native);
}
