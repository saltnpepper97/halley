//! Create a cluster on the Field through the real keyboard and pointer.
use super::fullscreen_multi_output::Input;
use super::*;
use serde_json::Value;

fn window_rect(state: &Value, id: u64) -> Option<[i64; 4]> {
    let window = state["windows"]
        .as_array()?
        .iter()
        .find(|window| window["id"] == id)?;
    let values = window["rect"].as_array()?;
    Some(std::array::from_fn(|index| values[index].as_i64().unwrap()))
}

/// The camera is at rest and every window has finished opening, so each one
/// is drawn at its client size times the camera scale.
fn settled(state: &Value) -> bool {
    state["windows"].as_array().unwrap().iter().all(|window| {
        let scale = window["view"][2].as_f64().unwrap_or(1.0);
        let drawn = window["rect"][2].as_f64().unwrap_or(0.0);
        let client = window["client_geometry"][2].as_f64().unwrap_or(0.0);
        window["camera_settled"] == true
            && window["animating"] == false
            && (drawn - client * scale).abs() <= 2.0
    })
}

fn tap(input: &Input, code: u32) {
    input.key(code, true);
    input.key(code, false);
}

fn new_cluster_members(fixture: &mut Fixture) -> Option<usize> {
    let Response::ClusterList(list) =
        fixture.request(Request::Cluster(halley_ipc::ClusterRequest::List {
            output: None,
        }))
    else {
        return None;
    };
    list.outputs
        .into_iter()
        .flat_map(|group| group.clusters)
        .find(|cluster| cluster.name != "Disconnect regression")
        .map(|cluster| cluster.member_count)
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn cluster_mode_picks_windows_on_the_field_and_flies_them_into_the_cluster() {
    let mut fixture = Fixture::with_two_outputs("");
    let input = Input::new(&fixture);
    let (mut queue_a, mut client_a) = native_window_named(
        &fixture,
        "Field cluster A",
        "halley.test.field-cluster-a",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let (mut queue_b, mut client_b) = native_window_named(
        &fixture,
        "Field cluster B",
        "halley.test.field-cluster-b",
        [0, u32::MAX, 0, u32::MAX],
    );
    let a = fixture.node("Field cluster A").id;
    let b = fixture.node("Field cluster B").id;
    // New windows open at the same place. Put B far beside A: each click then
    // picks a different window, and creation has to zoom out to show both.
    fixture.ack(Request::Node(NodeRequest::MoveBy {
        direction: halley_ipc::NodeMoveDirection::Right,
        distance: 2_500,
        selector: Some(NodeSelector::Id(b)),
        output: None,
    }));

    // F6 is bound to cluster-mode in this fixture.
    tap(&input, 64);
    let rects = wait_for("both windows on the zoomed-out Field", || {
        queue_a.roundtrip(&mut client_a).unwrap();
        queue_b.roundtrip(&mut client_b).unwrap();
        let state = input.state();
        let zoomed_out = state["windows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|window| window["view"][2].as_f64().is_some_and(|scale| scale < 1.0));
        (settled(&state) && zoomed_out)
            .then(|| window_rect(&state, a).zip(window_rect(&state, b)))
            .flatten()
    });
    for rect in [rects.0, rects.1] {
        input.motion(
            (rect[0] + rect[2] / 2) as f64,
            (rect[1] + rect[3] / 2) as f64,
        );
        input.button(272, true);
        input.button(272, false);
    }
    // Enter names the cluster with its default name, Enter again confirms.
    tap(&input, 28);
    tap(&input, 28);

    wait_for("the new cluster holds both windows", || {
        queue_a.roundtrip(&mut client_a).unwrap();
        queue_b.roundtrip(&mut client_b).unwrap();
        (new_cluster_members(&mut fixture) == Some(2)).then_some(())
    });
    wait_for("the camera back at its earlier zoom", || {
        let state = input.state();
        state["windows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|window| window["camera_settled"] == true)
            .any(|window| window["view"][2].as_f64() == Some(1.0))
            .then_some(())
    });
}
