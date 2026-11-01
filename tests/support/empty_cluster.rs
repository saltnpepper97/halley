//! Exercise empty workspace focus and deletion through real pointer/key events.
use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; two-output cluster drag"]
fn last_window_drag_leaves_an_empty_cluster_with_two_press_deletion() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let config = fixture.path.join("halley.rune");
    let text = fs::read_to_string(&config).unwrap().replacen(
        "keybinds:\n",
        "keybinds:\n \"F7\" \"close-focused\"\n \"alt+click-left\" \"move-window\"\n",
        1,
    );
    fs::write(config, text).unwrap();
    fixture.ack(Request::ConfigReload);
    let input = Input::new(&fixture);
    let remote_x = match fixture.request(Request::Outputs) {
        Response::Outputs(info) => {
            f64::from(
                info.outputs
                    .iter()
                    .find(|o| o.name == "winit-2")
                    .unwrap()
                    .offset_x,
            ) + 200.0
        }
        response => panic!("unexpected outputs: {response:?}"),
    };
    let cluster = wait_for("startup cluster", || {
        match fixture.request(Request::Cluster(ClusterRequest::List { output: None })) {
            Response::ClusterList(list) => list.outputs.into_iter().flat_map(|o| o.clusters).next(),
            _ => None,
        }
    });
    fixture.ack(Request::Cluster(ClusterRequest::Open {
        target: ClusterTarget::Id(cluster.id),
        output: None,
    }));
    let (mut queue, mut client) = native_window(&fixture);
    client.resize_for_configures = true;
    thread::sleep(Duration::from_millis(600));
    queue.roundtrip(&mut client).unwrap();
    let id = fixture.node("surviving native window").id;
    let state = input.state();
    let rect = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id)
        .unwrap()["rect"]
        .as_array()
        .unwrap();
    let x = rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0;
    let y = rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0;
    input.motion(x, y);
    input.key(56, true); // Nested backend's compositor modifier: Alt.
    input.button(272, true);
    assert_eq!(input.motion(remote_x, 300.0)["moving"], true);
    input.button(272, false);
    input.key(56, false);
    match fixture.request(Request::Cluster(ClusterRequest::Inspect {
        target: ClusterTarget::Id(cluster.id),
        output: None,
    })) {
        Response::ClusterInfo(info) => assert!(info.members.is_empty(), "{info:?}"),
        response => panic!("cluster disappeared: {response:?}"),
    }
    // Returning to the empty output must not let the remote focused client
    // suppress the hint, even before closing and reopening the workspace.
    input.motion(600.0, 700.0);
    let state = wait_for("empty cluster hint after cross-output drag", || {
        let state = input.state();
        (!state["empty_cluster"].as_array().unwrap().is_empty()).then_some(state)
    });
    assert_eq!(state["empty_cluster"][0]["shortcut"], "F7");
    // Toggle closed, select the other output, then reopen through IPC. The
    // common activation path must select the workspace's owning output.
    fixture.ack(Request::Cluster(ClusterRequest::Slot {
        slot: cluster.slot.unwrap(),
        output: Some(cluster.output.clone()),
    }));
    input.motion(remote_x, 700.0);
    fixture.ack(Request::Cluster(ClusterRequest::Open {
        target: ClusterTarget::Id(cluster.id),
        output: None,
    }));
    let state = input.state();
    assert_eq!(state["selected_output"], "winit");
    input.key(65, true); // F7: configured close binding.
    assert_eq!(input.state()["empty_cluster"][0]["armed"], true);
    input.key(65, true); // Held/repeated presses cannot confirm deletion.
    assert_eq!(input.state()["empty_cluster"][0]["armed"], true);
    input.key(65, false);
    input.key(65, true);
    input.key(65, false);
    match fixture.request(Request::Cluster(ClusterRequest::List { output: None })) {
        Response::ClusterList(list) => assert!(
            list.outputs
                .iter()
                .all(|o| o.clusters.iter().all(|c| c.id != cluster.id))
        ),
        response => panic!("unexpected response: {response:?}"),
    }
    assert!(fixture.nodes().iter().any(|node| node.id == id));
}
