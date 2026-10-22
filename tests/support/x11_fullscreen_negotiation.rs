use super::*;
use x11rb::protocol::xproto::{
    ChangeGCAux, ClientMessageEvent, ConfigureWindowAux, CreateGCAux, EventMask, Rectangle,
};

fn fullscreen(connection: &RustConnection, window: u32, enabled: bool) {
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    connection
        .send_event(
            false,
            connection.setup().roots[0].root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(
                32,
                window,
                atom(b"_NET_WM_STATE"),
                [
                    u32::from(enabled),
                    atom(b"_NET_WM_STATE_FULLSCREEN"),
                    0,
                    1,
                    0,
                ],
            ),
        )
        .unwrap();
    connection.flush().unwrap();
}

fn setup(initial_fullscreen: bool) -> (Fixture, RustConnection, u32, u16, u16) {
    let mut fixture = Fixture::with_two_outputs("input:\n focus-mode \"click\"\nend\n");
    let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
        panic!()
    };
    let output = outputs.outputs.iter().find(|o| o.name == "winit").unwrap();
    let mode = output.modes[output.current_mode.unwrap()];
    let (width, height) = (mode.width as u16, mode.height as u16);
    let window = if initial_fullscreen {
        let screen = &connection.setup().roots[0];
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                screen.root_depth,
                window,
                screen.root,
                0,
                0,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().background_pixel(screen.white_pixel),
            )
            .unwrap();
        connection
            .change_property8(
                x11rb::protocol::xproto::PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"Fullscreen negotiation",
            )
            .unwrap();
        let state = connection
            .intern_atom(false, b"_NET_WM_STATE")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let full = connection
            .intern_atom(false, b"_NET_WM_STATE_FULLSCREEN")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        connection
            .change_property32(
                x11rb::protocol::xproto::PropMode::REPLACE,
                window,
                state,
                AtomEnum::ATOM,
                &[full],
            )
            .unwrap();
        connection.map_window(window).unwrap();
        connection.flush().unwrap();
        window
    } else {
        x11_window(&connection, "Fullscreen negotiation")
    };
    fixture.node("Fullscreen negotiation");
    if !initial_fullscreen {
        fullscreen(&connection, window, true);
    }
    wait_for("initial fullscreen geometry", || {
        let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
        (geometry.width == width && geometry.height == height).then_some(())
    });
    thread::sleep(Duration::from_millis(700));
    (fixture, connection, window, width, height)
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; fullscreen ConfigureRequest"]
fn fullscreen_keeps_its_geometry_when_a_client_requests_an_old_video_mode() {
    let (_fixture, connection, window, width, height) = setup(false);
    connection
        .configure_window(
            window,
            &ConfigureWindowAux::new()
                .x(370)
                .y(210)
                .width(320)
                .height(200),
        )
        .unwrap()
        .check()
        .unwrap();
    thread::sleep(Duration::from_millis(150));
    let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
    assert_eq!(
        (geometry.width, geometry.height),
        (width, height),
        "a fullscreen client resize must receive the compositor-owned size"
    );
    // A repeated in-game fullscreen selection must retain that same size.
    fullscreen(&connection, window, true);
    thread::sleep(Duration::from_millis(150));
    let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (width, height));
    fullscreen(&connection, window, false);
    thread::sleep(Duration::from_millis(400));
    connection
        .configure_window(window, &ConfigureWindowAux::new().width(360).height(220))
        .unwrap()
        .check()
        .unwrap();
    wait_for("ordinary windowed client resizing", || {
        let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
        (geometry.width == 360 && geometry.height == 220).then_some(())
    });
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; same-size fullscreen repaint"]
fn fullscreen_off_on_with_the_same_buffer_does_not_freeze_partial_repaints() {
    let (mut fixture, connection, window, _, _) = setup(true);
    connection
        .change_window_attributes(
            window,
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                .background_pixmap(x11rb::NONE),
        )
        .unwrap()
        .check()
        .unwrap();
    // Video-mode menus commonly remove/add fullscreen without replacing the
    // already fullscreen-sized swapchain. Do not request any repaint yet.
    fullscreen(&connection, window, false);
    fullscreen(&connection, window, true);
    thread::sleep(Duration::from_millis(500));
    let input = fullscreen_multi_output::Input::new(&fixture);
    let snapshot = input.state();
    let node = fixture.node("Fullscreen negotiation");
    let state = snapshot["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|state| state["id"] == node.id)
        .unwrap();
    assert_eq!(
        state["fullscreen_configure_pending"], false,
        "same-size fullscreen must settle without requiring another repaint: {state}"
    );
    assert_eq!(
        state["fullscreen_snapshot"], false,
        "same-size fullscreen must release the captured frame: {state}"
    );
    let gc = connection.generate_id().unwrap();
    connection
        .create_gc(gc, window, &CreateGCAux::new())
        .unwrap();
    // Only repaint a small patch, as a static menu would. Full-buffer damage
    // must not be required when no buffer resize actually happened.
    for colour in [0x00ff0000, 0x0000ff00] {
        connection
            .change_gc(gc, &ChangeGCAux::new().foreground(colour))
            .unwrap();
        connection
            .poly_fill_rectangle(
                window,
                gc,
                &[Rectangle {
                    x: 40,
                    y: 40,
                    width: 80,
                    height: 80,
                }],
            )
            .unwrap();
        connection.flush().unwrap();
        thread::sleep(Duration::from_millis(350));
        let (pixels, stride) = capture_frame_pixels_on(&mut fixture, Some("winit"));
        let pixel = &pixels[(80 * stride + 80) * 4..][..3];
        let expected = if colour == 0x00ff0000 {
            [0, 0, 255]
        } else {
            [0, 255, 0]
        };
        assert_eq!(
            pixel, expected,
            "fullscreen must show the latest partial repaint"
        );
    }
}
