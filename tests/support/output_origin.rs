//! Root geometry and existing Field state across a shared output-origin shift.
use super::fullscreen_multi_output::Input;
use super::*;
use serde_json::Value;
use x11rb::protocol::randr::ConnectionExt as _;

fn window(input: &Input, id: u64) -> Value {
    input.state()["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id)
        .unwrap()
        .clone()
}
fn geometry(input: &Input, id: u64) -> [i64; 4] {
    let value = window(input, id);
    std::array::from_fn(|index| value["client_geometry"][index].as_i64().unwrap())
}
fn tap(input: &Input, code: u32) {
    input.key(code, true);
    input.key(code, false);
}
fn shifted(mut rect: [i64; 4], x: i64, y: i64) -> [i64; 4] {
    rect[0] += x;
    rect[1] += y;
    rect
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn left_and_above_outputs_fit_the_real_x_root_and_keep_the_primary_monitor() {
    for positions in ["0,0,-1024,0", "0,0,0,-768", "0,0,-1024,-768"] {
        let mut fixture = Fixture::with_output_layout(positions);
        let (x11, screen) = x11rb::connect(Some(&fixture.display)).unwrap();
        let root = x11.setup().roots[screen].root;
        wait_for("RandR root includes every monitor", || {
            let root_geometry = x11.get_geometry(root).unwrap().reply().unwrap();
            let monitors = x11
                .randr_get_monitors(root, true)
                .unwrap()
                .reply()
                .unwrap()
                .monitors;
            (monitors.len() == 2
                && monitors.iter().all(|m| {
                    m.x >= 0
                        && m.y >= 0
                        && i32::from(m.x) + i32::from(m.width) <= i32::from(root_geometry.width)
                        && i32::from(m.y) + i32::from(m.height) <= i32::from(root_geometry.height)
                }))
            .then_some(())
        });
        wait_for(
            "Halley primary is RandR primary even away from zero",
            || {
                let primary = x11
                    .randr_get_output_primary(root)
                    .unwrap()
                    .reply()
                    .unwrap()
                    .output;
                if primary == 0 {
                    return None;
                }
                let resources = x11
                    .randr_get_screen_resources_current(root)
                    .unwrap()
                    .reply()
                    .unwrap();
                let info = x11
                    .randr_get_output_info(primary, resources.config_timestamp)
                    .unwrap()
                    .reply()
                    .unwrap();
                (info.name == b"winit").then_some(())
            },
        );
        let xid = x11_window(&x11, "Translated X11 client");
        let node = fixture.node("Translated X11 client");
        let input = Input::new(&fixture);
        request_native_focus(&mut fixture, node.id);
        let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
            panic!("output inventory");
        };
        let output = outputs
            .outputs
            .iter()
            .find(|output| output.name == "winit")
            .unwrap();
        let origin = (i64::from(output.offset_x), i64::from(output.offset_y));
        wait_for("published X11 position agrees with presentation", || {
            let observed = x11
                .translate_coordinates(xid, root, 0, 0)
                .unwrap()
                .reply()
                .unwrap();
            let visual = window(&input, node.id);
            (i64::from(observed.dst_x) == origin.0 + visual["rect"][0].as_i64().unwrap()
                && i64::from(observed.dst_y) == origin.1 + visual["rect"][1].as_i64().unwrap())
            .then_some(())
        });
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn live_origin_changes_preserve_arrange_collapsed_pinned_maximize_and_fullscreen_restores() {
    let mut fixture = Fixture::with_output_layout("0,0,1280,0");
    let input = Input::new(&fixture);
    let (mut qa, mut a) = native_window_named(
        &fixture,
        "Origin A",
        "halley.test.origin-a",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let (mut qb, mut b) = native_window_named(
        &fixture,
        "Origin B",
        "halley.test.origin-b",
        [0, u32::MAX, 0, u32::MAX],
    );
    a.resize_for_configures = true;
    b.resize_for_configures = true;
    let aid = fixture.node("Origin A").id;
    let bid = fixture.node("Origin B").id;
    let original = geometry(&input, aid);
    tap(&input, 87);
    wait_for("Arrange clients accept their sizes", || {
        qa.roundtrip(&mut a).unwrap();
        qb.roundtrip(&mut b).unwrap();
        (geometry(&input, aid) != original).then_some(())
    });
    let before = geometry(&input, aid);
    let response = input.layout([0, 0, -1024, 0]);
    assert!(response["layout_error"].is_null(), "{response}");
    assert_eq!(geometry(&input, aid), shifted(before, 1024, 0));
    tap(&input, 87);
    wait_for("Arrange restore follows its translated monitor", || {
        qa.roundtrip(&mut a).unwrap();
        qb.roundtrip(&mut b).unwrap();
        (geometry(&input, aid) == shifted(original, 1024, 0)).then_some(())
    });
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(bid)),
        output: None,
    }));
    let collapsed = fixture.node("Origin B");
    request_native_focus(&mut fixture, aid);
    tap(&input, 63);
    assert!(fixture.node("Origin A").pinned);
    let pinned = geometry(&input, aid);
    let response = input.layout([0, 0, 1280, 0]);
    assert!(response["layout_error"].is_null());
    assert_eq!(geometry(&input, aid), shifted(pinned, -1024, 0));
    assert!(fixture.node("Origin A").pinned);
    let moved = fixture.node("Origin B");
    assert_eq!(
        (moved.pos_x, moved.pos_y),
        (collapsed.pos_x - 1024.0, collapsed.pos_y)
    );
    fixture.ack(Request::Node(NodeRequest::Restore {
        selector: Some(NodeSelector::Id(bid)),
        output: None,
    }));
    request_native_focus(&mut fixture, aid);
    for (code, flag) in [(62, "maximized"), (68, "fullscreen")] {
        let saved = geometry(&input, aid);
        tap(&input, code);
        wait_for("presentation active", || {
            qa.roundtrip(&mut a).unwrap();
            (window(&input, aid)[flag] == true).then_some(())
        });
        let response = input.layout([0, 0, -1024, 0]);
        assert!(response["layout_error"].is_null());
        tap(&input, code);
        wait_for("presentation restores translated original geometry", || {
            qa.roundtrip(&mut a).unwrap();
            (window(&input, aid)[flag] == false && geometry(&input, aid) == shifted(saved, 1024, 0))
                .then_some(())
        });
        assert!(input.layout([0, 0, 1280, 0])["layout_error"].is_null());
    }
}
