//! Capture real clients during CLI movement, including an in-flight reversal.
use super::fullscreen_navigation::{terminal, terminal_bounds};
use super::*;

#[test]
#[ignore = "requires a Wayland desktop, XWayland and Kitty; captures actual moving clients"]
fn window_movement_native_and_x11_retarget_without_jumps() {
    for protocol in ["wayland", "x11"] {
        movement(
            protocol,
            "motion \"easing\"\nduration-ms 800\ncurve \"linear\"",
            1.0,
        );
    }
    movement(
        "wayland",
        "motion \"spring\"\ndamping-ratio 1.0\nstiffness 200.0",
        1.25,
    );
}

fn movement(protocol: &str, motion: &str, scale: f32) {
    let mut fixture = Fixture::with_config(
        "stacking",
        &format!(
            "view:\n output:\n name \"winit\"\n scale {scale}\n end\nend\nanimations:\n window-open:\n enabled false\n end\n window-move:\n {motion}\n end\nend\n"
        ),
    );
    let _terminal = terminal(&fixture, "Window movement test", "#9c3769", protocol);
    let node = fixture.node("Window movement test");
    thread::sleep(Duration::from_millis(600));
    let bounds = |fixture: &mut Fixture| {
        let (pixels, width) = capture_frame_pixels(fixture);
        terminal_bounds(&pixels, width)
    };
    let initial = bounds(&mut fixture);
    let nudge = |fixture: &mut Fixture, direction, distance| {
        fixture.ack(Request::Node(NodeRequest::MoveBy {
            direction,
            distance,
            selector: Some(NodeSelector::Id(node.id)),
            output: None,
        }))
    };
    nudge(&mut fixture, halley_ipc::NodeMoveDirection::Right, 200);
    let started = Instant::now();
    let mut forward = vec![(0, initial)];
    while started.elapsed() < Duration::from_millis(200) {
        let frame = bounds(&mut fixture);
        forward.push((started.elapsed().as_millis(), frame));
    }
    assert!(
        forward
            .iter()
            .any(|(_, b)| b[0] > initial[0] + 3 && b[0] < initial[0] + (200.0 * scale) as i32 - 3),
        "no intermediate rendered movement: {forward:?}"
    );
    let before_reverse = forward.last().unwrap().1;
    nudge(&mut fixture, halley_ipc::NodeMoveDirection::Left, 300);
    let started = Instant::now();
    let after_reverse = bounds(&mut fixture);
    assert!(
        (after_reverse[0] - before_reverse[0]).abs() < (75.0 * scale) as i32,
        "reversal jumped: {before_reverse:?} -> {after_reverse:?}"
    );
    let mut reverse = vec![(started.elapsed().as_millis(), after_reverse)];
    while started.elapsed() < Duration::from_millis(1600) {
        let frame = bounds(&mut fixture);
        reverse.push((started.elapsed().as_millis(), frame));
    }
    let final_bounds = reverse.last().unwrap().1;
    let expected_x = initial[0] - (100.0 * scale).round() as i32;
    assert!(
        (final_bounds[0] - expected_x).abs() <= 3,
        "wrong endpoint: initial={initial:?}, final={final_bounds:?}, expected={expected_x}"
    );
    for pair in reverse.windows(2) {
        // A dropped frame can cover more distance; scale the bound by time.
        let dt = (pair[1].0 - pair[0].0).max(1) as f32;
        let allowed = (dt * 2.0 * scale).ceil() as i32 + 20;
        assert!(
            (pair[1].1[0] - pair[0].1[0]).abs() <= allowed,
            "cleanup/retarget jump: {pair:?}"
        );
        assert!(
            (pair[1].1[2] - pair[1].1[0] - (initial[2] - initial[0])).abs() <= 2,
            "window resized during translation: {pair:?}"
        );
    }
    eprintln!("{protocol} scale={scale} forward={forward:?} reverse={reverse:?}");
    // Exact positioning, including negative values, preserves public geometry.
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: -240,
        y: -100,
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    let info = fixture.node("Window movement test");
    assert_eq!(
        (
            info.pos_x - info.width / 2.0,
            info.pos_y - info.height / 2.0
        ),
        (-240.0, -100.0)
    );
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; checks exact placement rejection against real landmarks"]
fn window_movement_exact_placement_rejects_obstacles_and_overflow_without_displacement() {
    let mut fixture = Fixture::with_config(
        "stacking",
        "animations:\n window-open:\n enabled false\n end\n window-move:\n enabled false\n end\nend\n",
    );
    let (_first_queue, _first_state) = native_window_named(
        &fixture,
        "Exact move target",
        "halley.test.move",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let first = fixture.node("Exact move target");
    let (_second_queue, _second_state) = native_window_named(
        &fixture,
        "Exact move blocker",
        "halley.test.blocker",
        [0, u32::MAX, 0, u32::MAX],
    );
    let blocker = fixture.node("Exact move blocker");
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(blocker.id)),
        output: None,
    }));
    let blocker = fixture.node("Exact move blocker");
    let saved = fixture.node("Exact move target");
    for (x, y) in [
        (
            (blocker.pos_x - first.width / 2.0).round() as i32,
            (blocker.pos_y - first.height / 2.0).round() as i32,
        ),
        (i32::MAX, 0),
        (0, i32::MAX),
        (i32::MIN, i32::MIN),
    ] {
        let response = fixture.request(Request::Node(NodeRequest::MoveTo {
            x,
            y,
            selector: Some(NodeSelector::Id(first.id)),
            output: None,
        }));
        assert!(
            matches!(
                response,
                Response::ApiError(halley_ipc::ServerError {
                    kind: halley_ipc::ServerErrorKind::InvalidRequest,
                    ..
                })
            ),
            "blocked exact placement unexpectedly succeeded: {response:?}"
        );
        let after = fixture.node("Exact move target");
        assert_eq!((after.pos_x, after.pos_y), (saved.pos_x, saved.pos_y));
    }
}
