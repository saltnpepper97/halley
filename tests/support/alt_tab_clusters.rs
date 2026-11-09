//! Alt+Tab inside an open cluster is the ordinary switcher: a one-window
//! stacking cluster still reaches windows on the other monitor.
use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; Alt+Tab in a one-window stack"]
fn alt_tab_leaves_a_one_window_stacking_cluster_for_another_monitor() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    wait_for("two advertised outputs", || {
        match fixture.request(Request::Outputs) {
            Response::Outputs(outputs) => (outputs.outputs.len() == 2).then_some(()),
            _ => None,
        }
    });
    let input = Input::new(&fixture);
    let cluster = wait_for("startup cluster", || {
        match fixture.request(Request::Cluster(ClusterRequest::List { output: None })) {
            Response::ClusterList(list) => list.outputs.into_iter().flat_map(|o| o.clusters).next(),
            _ => None,
        }
    });
    let remote_output = match fixture.request(Request::Outputs) {
        Response::Outputs(info) => info
            .outputs
            .into_iter()
            .map(|output| output.name)
            .find(|name| *name != cluster.output)
            .unwrap(),
        response => panic!("unexpected outputs: {response:?}"),
    };
    fixture.ack(Request::Cluster(ClusterRequest::Open {
        target: ClusterTarget::Id(cluster.id),
        output: None,
    }));
    let (_member_queue, _member_state) = native_window_named(
        &fixture,
        "Only stack member",
        "halley.test.stack",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let member = fixture.node("Only stack member");
    wait_for(
        "window admitted into the open stacking cluster",
        || match fixture.request(Request::Cluster(ClusterRequest::Inspect {
            target: ClusterTarget::Id(cluster.id),
            output: None,
        })) {
            Response::ClusterInfo(info) => (info.summary.active
                && info.summary.layout == halley_ipc::ClusterLayoutKind::Stacking
                && info.members.iter().map(|m| m.id).collect::<Vec<_>>() == [member.id])
            .then_some(()),
            _ => None,
        },
    );

    fixture.ack(Request::Control(halley_ipc::ControlRequest::MonitorFocus(
        halley_ipc::MonitorFocusTarget::Output(remote_output.clone()),
    )));
    let (_remote_queue, _remote_state) = native_window_named(
        &fixture,
        "Other-monitor window",
        "halley.test.remote",
        [0, 0, u32::MAX, u32::MAX],
    );
    let remote = fixture.node("Other-monitor window");
    assert_eq!(remote.output.as_deref(), Some(remote_output.as_str()));

    fixture.ack(Request::Control(halley_ipc::ControlRequest::MonitorFocus(
        halley_ipc::MonitorFocusTarget::Output(cluster.output.clone()),
    )));
    request_native_focus(&mut fixture, member.id);
    wait_for("stack member focused", || {
        (input.state()["focused"] == member.id).then_some(())
    });

    // Before the fix the stack swallowed Alt+Tab and, with one member, did
    // nothing; the switcher never opened.
    input.preview(remote.id);
    input.commit(remote.id);
}
