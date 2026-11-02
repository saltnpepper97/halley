//! Close a focused collapsed window through the real keyboard handler.
use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn close_key_targets_focused_and_hovered_collapsed_windows() {
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
