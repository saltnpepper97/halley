use super::fullscreen_multi_output::Input;
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn keyboard_selects_a_portal_monitor_without_pointer_motion() {
    let fixture = Fixture::with_two_outputs("");
    let input = Input::new(&fixture);
    let path = fixture.path.join("halley/halley.sock");
    let request = thread::spawn(move || {
        let mut connection = halley_ipc::Connection::connect_to(&path).unwrap();
        connection
            .request(
                &Request::ChooseSource(halley_ipc::SourceChooserRequest {
                    request_handle: "keyboard-monitor-choice".into(),
                    source_types: halley_ipc::SOURCE_MONITOR,
                }),
                &[],
            )
            .unwrap()
            .response
    });
    let before = wait_for("portal chooser", || {
        let state = input.state();
        state["capture_active"].as_bool().unwrap().then_some(state)
    });
    let original = before["capture_source"]["Monitor"]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let pointer = before["pointer"].clone();
    input.key(106, true); // Right arrow
    let after = input.key(106, false);
    let selected = after["capture_source"]["Monitor"]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(selected, original);
    assert_eq!(after["pointer"], pointer);
    input.key(28, true); // Enter explicitly grants the selected source.
    input.key(28, false);
    assert!(
        matches!(request.join().unwrap(), Response::Source(halley_ipc::SourceChooserResponse::Selected(halley_ipc::CaptureSource::Monitor { name, .. })) if name == selected)
    );
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn keyboard_moves_and_resizes_a_screenshot_region() {
    let fixture = Fixture::with_two_outputs("");
    let input = Input::new(&fixture);
    let path = fixture.path.join("halley/halley.sock");
    let request = thread::spawn(move || {
        let mut connection = halley_ipc::Connection::connect_to(&path).unwrap();
        connection
            .request(
                &Request::Screenshot(halley_ipc::ScreenshotRequest {
                    request_handle: "keyboard-region".into(),
                    target: halley_ipc::ScreenshotTarget::Area,
                }),
                &[],
            )
            .unwrap()
            .response
    });
    let before = wait_for("screenshot region", || {
        let state = input.state();
        state["capture_region"].is_array().then_some(state)
    });
    let region = &before["capture_region"];
    input.key(106, true);
    let moved = input.key(106, false);
    assert_eq!(
        moved["capture_region"][0].as_i64().unwrap(),
        region[0].as_i64().unwrap() + 10
    );
    input.key(42, true); // Shift
    input.key(106, true);
    let resized = input.key(106, false);
    input.key(42, false);
    assert_eq!(
        resized["capture_region"][2].as_i64().unwrap(),
        region[2].as_i64().unwrap() + 10
    );
    input.key(1, true); // Escape cancels without saving.
    input.key(1, false);
    assert!(matches!(
        request.join().unwrap(),
        Response::Screenshot(halley_ipc::ScreenshotResponse::Cancelled)
    ));
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn keyboard_window_sharing_skips_protected_windows() {
    let mut fixture = Fixture::with_two_outputs(
        "rules:\n  window-rule:\n    app-id \"halley.privacy.secret\"\n    block-screencast true\n  end\nend\n",
    );
    let (_secret_queue, _secret) = native_window_named(
        &fixture,
        "A secret",
        "halley.privacy.secret",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let (_public_queue, _public) = native_window_named(
        &fixture,
        "B public",
        "halley.privacy.public",
        [0, u32::MAX, 0, u32::MAX],
    );
    fixture.node("A secret");
    let public_id = fixture.node("B public").id;
    let input = Input::new(&fixture);
    let path = fixture.path.join("halley/halley.sock");
    let request = thread::spawn(move || {
        let mut connection = halley_ipc::Connection::connect_to(&path).unwrap();
        connection
            .request(
                &Request::ChooseSource(halley_ipc::SourceChooserRequest {
                    request_handle: "keyboard-window-choice".into(),
                    source_types: halley_ipc::SOURCE_WINDOW,
                }),
                &[],
            )
            .unwrap()
            .response
    });
    wait_for("window chooser", || {
        input.state()["capture_active"]
            .as_bool()
            .unwrap()
            .then_some(())
    });
    input.key(106, true);
    let state = input.key(106, false);
    assert_eq!(state["capture_source"]["Window"]["window_id"], public_id);
    input.key(28, true);
    input.key(28, false);
    assert!(
        matches!(request.join().unwrap(), Response::Source(halley_ipc::SourceChooserResponse::Selected(halley_ipc::CaptureSource::Window { window_id, .. })) if window_id == public_id)
    );
}
