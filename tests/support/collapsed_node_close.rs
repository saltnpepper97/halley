//! Close collapsed windows through the real keyboard and pointer handlers.
use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn close_key_targets_focused_and_hovered_collapsed_windows() {
    for (hovered, hide_when_typing) in [(false, false), (true, false), (true, true)] {
        let mut fixture = Fixture::with_two_outputs(&format!(
            "animations:\n enabled false\nend\ninput:\n focus-mode \"click\"\nend\ncursor:\n hide-when-typing {hide_when_typing}\nend\n",
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
        let node = fixture.node("Close collapsed node");
        assert!(node.focused, "collapsed window must keep logical focus");
        let mut other = None;
        if hovered {
            other = Some(native_window_named(
                &fixture,
                "Other focused window",
                "halley.test.other",
                [u32::MAX, 0, 0, u32::MAX],
            ));
            thread::sleep(Duration::from_millis(700));
            let node = fixture.node("Close collapsed node");
            let state = input.state();
            let window = state["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|window| window["id"] == id)
                .unwrap();
            let viewport = window["viewport"].as_array().unwrap();
            let scale = window["view"][2].as_f64().unwrap();
            let x = (f64::from(node.pos_x) - viewport[0].as_f64().unwrap()) * scale;
            let y = (f64::from(node.pos_y) - viewport[1].as_f64().unwrap()) * scale;
            input.motion(x, y);
        } else {
            input.motion(5.0, 5.0);
        }
        input.key(56, true); // Winit's modifier is Alt.
        input.key(16, true); // Q
        input.key(16, false);
        input.key(56, false);
        wait_for("close request delivered to collapsed window", || {
            node_queue.roundtrip(&mut node_client).unwrap();
            node_client.close_requested.then_some(())
        });
        if let Some((mut queue, mut client)) = other {
            queue.roundtrip(&mut client).unwrap();
            assert!(
                !client.close_requested,
                "unrelated focused window was closed"
            );
        }
    }
}
