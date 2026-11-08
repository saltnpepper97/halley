//! Click the first-decay notice through the real pointer handler.
use super::fullscreen_multi_output::Input;
use super::*;
use serde_json::Value;

fn notice_rect(state: &Value, target: u64) -> Option<[i64; 4]> {
    state["notices"]
        .as_array()?
        .iter()
        .find(|notice| notice["target"] == target)
        .map(|notice| {
            let values = notice["rect"].as_array().unwrap();
            std::array::from_fn(|index| values[index].as_i64().unwrap())
        })
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn clicking_the_decay_notice_restores_and_focuses_the_collapsed_window() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ndecay:\n outside-delay-seconds 2\n inside-delay-seconds 2\nend\n",
    );
    let input = Input::new(&fixture);
    let (mut queue_b, mut client_b) = native_window_named(
        &fixture,
        "Decay notice B",
        "halley.test.decay-notice-b",
        [0, u32::MAX, 0, u32::MAX],
    );
    let (mut queue_a, mut client_a) = native_window_named(
        &fixture,
        "Decay notice A",
        "halley.test.decay-notice-a",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let a = fixture.node("Decay notice A").id;
    let b = fixture.node("Decay notice B").id;
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(a)),
        output: None,
    }));

    // B is unfocused, so automatic decay collapses it and explains itself.
    let rect = wait_for("decay notice for B", || {
        queue_a.roundtrip(&mut client_a).unwrap();
        queue_b.roundtrip(&mut client_b).unwrap();
        (fixture.node("Decay notice B").state == halley_ipc::NodeState::Node)
            .then(|| notice_rect(&input.state(), b))
            .flatten()
    });
    let a_before = fixture.node("Decay notice A");
    assert!(a_before.focused, "A keeps focus while B decays");

    input.motion(
        (rect[0] + rect[2] / 2) as f64,
        (rect[1] + rect[3] / 2) as f64,
    );
    input.button(272, true);
    input.button(272, false);

    wait_for("B restored and focused by the click", || {
        queue_a.roundtrip(&mut client_a).unwrap();
        queue_b.roundtrip(&mut client_b).unwrap();
        let node = fixture.node("Decay notice B");
        (node.state != halley_ipc::NodeState::Node && node.focused).then_some(())
    });
    let a_after = fixture.node("Decay notice A");
    assert_ne!(
        a_after.state,
        halley_ipc::NodeState::Node,
        "A is not collapsed"
    );
    assert!(!a_after.focused);
    assert!(
        notice_rect(&input.state(), b).is_none(),
        "the clicked notice is dismissed"
    );
}
