//! Click a collapsed node's Bearings chip through the real pointer handler.
use super::fullscreen_multi_output::Input;
use super::*;
use serde_json::Value;

fn window(state: &Value, id: u64) -> Value {
    state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|window| window["id"] == id)
        .unwrap()
        .clone()
}

fn rect(value: &Value) -> [i64; 4] {
    let values = value.as_array().unwrap();
    std::array::from_fn(|index| values[index].as_i64().unwrap())
}

fn intersects(a: [i64; 4], b: [i64; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

fn on_screen(state: &Value, id: u64) -> bool {
    let window = window(state, id);
    intersects(rect(&window["viewport"]), rect(&window["client_geometry"]))
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn bearings_click_restores_an_offscreen_node_into_view() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\nnode:\n click-collapsed-pan \"never\"\nend\n",
    );
    let input = Input::new(&fixture);
    let title = "Bearings restore";
    let (mut queue, mut client) = native_window_named(
        &fixture,
        title,
        "halley.test.bearings-restore",
        [0, u32::MAX, 0, u32::MAX],
    );
    let id = fixture.node(title).id;
    fixture.ack(Request::Node(NodeRequest::MoveBy {
        direction: halley_ipc::NodeMoveDirection::Right,
        // Within the default 1,200 px Bearings fade, so the chip is drawn.
        distance: 1_000,
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    wait_for("window moved off screen", || {
        queue.roundtrip(&mut client).unwrap();
        let state = input.state();
        let window = window(&state, id);
        (window["camera_settled"] == true && !on_screen(&state, id)).then_some(())
    });
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    assert_eq!(fixture.node(title).state, halley_ipc::NodeState::Node);
    fixture.ack(Request::Bearings(halley_ipc::BearingsRequest::Show));
    let chip = wait_for("Bearings chip for the collapsed node", || {
        queue.roundtrip(&mut client).unwrap();
        input.state()["bearings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|output| output["mix"].as_f64().unwrap() > 0.99)
            .flat_map(|output| output["chips"].as_array().unwrap().clone())
            .find(|chip| chip["id"] == id)
            .map(|chip| rect(&chip["rect"]))
    });
    input.motion(
        (chip[0] + chip[2] / 2) as f64,
        (chip[1] + chip[3] / 2) as f64,
    );
    input.button(272, true);
    input.button(272, false);
    wait_for("restored window revealed by one click", || {
        queue.roundtrip(&mut client).unwrap();
        let state = input.state();
        (window(&state, id)["camera_settled"] == true && on_screen(&state, id)).then_some(())
    });
    let node = fixture.node(title);
    assert_ne!(node.state, halley_ipc::NodeState::Node);
    assert!(node.focused);
}
