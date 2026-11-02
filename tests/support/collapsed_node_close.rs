//! Close a focused collapsed window through the real keyboard handler.
use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn close_key_targets_focused_collapsed_window() {
    for mode in ["click", "hover"] {
        let mut fixture = Fixture::with_two_outputs(&format!(
            "animations:\n enabled false\nend\ninput:\n focus-mode \"{mode}\"\nend\n",
        ));
        let input = Input::new(&fixture);
        let (mut node_queue, mut node_client) = native_window_named(
            &fixture,
            "Close collapsed node",
            "halley.test.node-close",
            [0, u32::MAX, 0, u32::MAX],
        );
        let id = fixture.node("Close collapsed node").id;
        fixture.ack(Request::Node(NodeRequest::Collapse {
            selector: Some(NodeSelector::Id(id)),
            output: None,
        }));
        assert!(fixture.node("Close collapsed node").focused);
        input.key(56, true); // Winit's modifier is Alt.
        input.key(16, true); // Q
        input.key(16, false);
        input.key(56, false);
        wait_for(
            "close request delivered to focused collapsed window",
            || {
                node_queue.roundtrip(&mut node_client).unwrap();
                node_client.close_requested.then_some(())
            },
        );
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn close_key_sends_x11_delete_to_focused_collapsed_window() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let window = x11_window(&connection, "Close collapsed X11 node");
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let delete = atom(b"WM_DELETE_WINDOW");
    connection
        .change_property32(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            atom(b"WM_PROTOCOLS"),
            AtomEnum::ATOM,
            &[delete],
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    let id = fixture.node("Close collapsed X11 node").id;
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    assert!(fixture.node("Close collapsed X11 node").focused);
    input.key(56, true);
    input.key(16, true);
    input.key(16, false);
    input.key(56, false);
    wait_for("WM_DELETE_WINDOW delivered to collapsed X11 window", || {
        while let Some(event) = connection.poll_for_event().unwrap() {
            if let x11rb::protocol::Event::ClientMessage(event) = event
                && event.window == window
                && event.type_ == atom(b"WM_PROTOCOLS")
                && event.data.as_data32()[0] == delete
            {
                return Some(());
            }
        }
        None
    });
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn collapsed_x11_window_that_withdraws_on_close_leaves_the_field() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let title = "Close withdraws collapsed X11 node";
    let window = x11_window(&connection, title);
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let delete = atom(b"WM_DELETE_WINDOW");
    let protocols = atom(b"WM_PROTOCOLS");
    connection
        .change_property32(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            protocols,
            AtomEnum::ATOM,
            &[delete],
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    let id = fixture.node(title).id;
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    assert_eq!(fixture.node(title).state, halley_ipc::NodeState::Node);
    input.key(56, true); // Winit's modifier is Alt.
    input.key(16, true); // Q
    input.key(16, false);
    input.key(56, false);
    wait_for("WM_DELETE_WINDOW delivered to collapsed X11 window", || {
        while let Some(event) = connection.poll_for_event().unwrap() {
            if let x11rb::protocol::Event::ClientMessage(event) = event
                && event.window == window
                && event.type_ == protocols
                && event.data.as_data32()[0] == delete
            {
                return Some(());
            }
        }
        None
    });

    // Answer like Steam: an ICCCM withdrawal that keeps the XID for reuse.
    let root = connection.setup().roots[0].root;
    connection.unmap_window(window).unwrap().check().unwrap();
    let unmap = x11rb::protocol::xproto::UnmapNotifyEvent {
        response_type: x11rb::protocol::xproto::UNMAP_NOTIFY_EVENT,
        sequence: 0,
        event: root,
        window,
        from_configure: false,
    };
    connection
        .send_event(
            false,
            root,
            x11rb::protocol::xproto::EventMask::SUBSTRUCTURE_REDIRECT
                | x11rb::protocol::xproto::EventMask::SUBSTRUCTURE_NOTIFY,
            unmap,
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    wait_for("withdrawn collapsed window leaves the Field", || {
        fixture
            .nodes()
            .iter()
            .all(|node| node.title != title)
            .then_some(())
    });
    assert!(
        connection
            .get_window_attributes(window)
            .unwrap()
            .reply()
            .is_ok(),
        "the client kept its window; only its node was retired"
    );

    // Reopening (e.g. from a tray) maps the same XID as a fresh window.
    connection.map_window(window).unwrap().check().unwrap();
    connection.flush().unwrap();
    wait_for("reopened window is an expanded window", || {
        fixture
            .nodes()
            .into_iter()
            .find(|node| node.title == title && node.state == halley_ipc::NodeState::Active)
            .map(|_| ())
    });
}
