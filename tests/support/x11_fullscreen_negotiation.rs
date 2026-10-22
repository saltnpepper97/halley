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
    setup_with_config(initial_fullscreen, "")
}

fn setup_with_config(
    initial_fullscreen: bool,
    extra: &str,
) -> (Fixture, RustConnection, u32, u16, u16) {
    let mut fixture =
        Fixture::with_two_outputs(&format!("input:\n focus-mode \"click\"\nend\n{extra}"));
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
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; animated live game fullscreen"]
fn game_fullscreen_keeps_live_frames_during_smooth_geometry_changes() {
    for game in [true, false] {
        let extra = format!(
            r#"
rules:
  window-rule:
    title "Fullscreen negotiation"
    game {game}
  end
end
animations:
  window-open:
    enabled false
  end
  fullscreen:
    motion "easing"
    duration-ms 1000
    curve "linear"
  end
end
"#
        );
        let (mut fixture, connection, window, width, height) = setup_with_config(false, &extra);
        let input = fullscreen_multi_output::Input::new(&fixture);
        let id = fixture.node("Fullscreen negotiation").id;
        let observed = |snapshot: &serde_json::Value| {
            snapshot["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|state| state["id"] == id)
                .unwrap()
                .clone()
        };
        // Finish setup's client-owned entry, then use the actual keybind path
        // for both directions. A rule override is the same positive game
        // classification used by SteamAppId and steam_app_* windows.
        thread::sleep(Duration::from_millis(1100));
        for enabled in [false, true] {
            input.key(68, true); // F10
            let state = observed(&input.key(68, false));
            if game {
                assert_eq!(
                    state["fullscreen_snapshot"], false,
                    "game frames must stay live during geometry motion: {state}"
                );
            } else {
                assert_eq!(
                    state["fullscreen_snapshot"], true,
                    "ordinary X11 windows must retain their existing crossfade: {state}"
                );
            }
            wait_for("fullscreen configure accepted", || {
                let state = observed(&input.state());
                (!state["fullscreen_configure_pending"].as_bool().unwrap()).then_some(())
            });
            thread::sleep(Duration::from_millis(150));
            let state = observed(&input.state());
            let rect = state["rect"].as_array().unwrap();
            let animated_width = rect[2].as_f64().unwrap();
            assert!(
                animated_width > 240.0 && animated_width < f64::from(width),
                "fullscreen must still animate its geometry: {state}"
            );
            let gc = connection.generate_id().unwrap();
            connection
                .create_gc(gc, window, &CreateGCAux::new().foreground(0x0000ff00))
                .unwrap();
            connection
                .poly_fill_rectangle(
                    window,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    }],
                )
                .unwrap();
            connection.free_gc(gc).unwrap();
            connection.flush().unwrap();
            if game {
                wait_for("live game repaint inside the animated geometry", || {
                    let state = observed(&input.state());
                    let rect = state["rect"].as_array().unwrap();
                    let x = (rect[0].as_i64().unwrap() + rect[2].as_i64().unwrap() / 2) as usize;
                    let y = (rect[1].as_i64().unwrap() + rect[3].as_i64().unwrap() / 2) as usize;
                    let (pixels, stride) = capture_frame_pixels_on(&mut fixture, Some("winit"));
                    let pixel = &pixels[(y * stride + x) * 4..][..3];
                    (pixel[1] > 240 && pixel[0] < 10 && pixel[2] < 10).then_some(())
                });
            }
            thread::sleep(Duration::from_millis(1100));
            let state = observed(&input.state());
            assert_eq!(state["fullscreen_snapshot"], false);
            assert_eq!(state["fullscreen"], enabled);
            assert_eq!(state["animating"], false);
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; game fullscreen ownership"]
fn game_mod_f_survives_a_late_client_windowed_request_and_can_still_exit() {
    let extra = "animations:\n enabled false\nend\nrules:\n window-rule:\n title \"Fullscreen negotiation\"\n game true\n end\nend\n";
    let (mut fixture, connection, window, width, height) = setup_with_config(false, extra);
    let input = fullscreen_multi_output::Input::new(&fixture);
    let id = fixture.node("Fullscreen negotiation").id;
    // Exit the initial client-owned fullscreen, then explicitly enter Mod+F.
    for expected in [false, true] {
        input.key(68, true);
        input.key(68, false);
        wait_for("explicit fullscreen keybind settled", || {
            let state = input.state();
            let window = state["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["id"] == id)
                .unwrap();
            (window["fullscreen"] == expected).then_some(())
        });
    }
    // Wine may deliver an older windowed video-mode response after Mod+F,
    // including a ConfigureRequest for the old window size.
    fullscreen(&connection, window, false);
    connection
        .configure_window(window, &ConfigureWindowAux::new().width(320).height(200))
        .unwrap()
        .check()
        .unwrap();
    thread::sleep(Duration::from_millis(150));
    let state = input.state();
    let observed = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id)
        .unwrap();
    assert_eq!(
        observed["fullscreen"], true,
        "client chatter undid explicit fullscreen: {observed}"
    );
    let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (width, height));
    // A second explicit toggle must release the override normally.
    input.key(68, true);
    input.key(68, false);
    wait_for("explicit exit releases game fullscreen", || {
        let state = input.state();
        let window = state["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap();
        (window["fullscreen"] == false).then_some(())
    });
    connection
        .configure_window(window, &ConfigureWindowAux::new().width(360).height(220))
        .unwrap()
        .check()
        .unwrap();
    wait_for("game window resizes after explicit exit", || {
        let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
        (geometry.width == 360 && geometry.height == 220).then_some(())
    });
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
