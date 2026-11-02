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
