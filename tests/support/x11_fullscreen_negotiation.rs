use super::*;
use x11rb::protocol::xproto::{
    ChangeGCAux, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux, CreateGCAux,
    EventMask, Rectangle,
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
    setup_with_focus(initial_fullscreen, extra, "click")
}

fn setup_with_focus(
    initial_fullscreen: bool,
    extra: &str,
    focus: &str,
) -> (Fixture, RustConnection, u32, u16, u16) {
    let mut fixture =
        Fixture::with_two_outputs(&format!("input:\n focus-mode \"{focus}\"\nend\n{extra}"));
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
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; game focus-loss mode request"]
fn game_fullscreen_hover_to_another_monitor_keeps_the_game_presented() {
    for game in [false, true] {
        let extra = format!(
            "animations:\n enabled false\nend\nrules:\n window-rule:\n title \"Fullscreen negotiation\"\n game {game}\n end\nend\n"
        );
        let (mut fixture, connection, window, width, height) =
            setup_with_focus(false, &extra, "hover");
        let source = fixture.node("Fullscreen negotiation");
        let (_queue, _state) = native_window_named(
            &fixture,
            "Remote focus",
            "halley.test.remote",
            [0, 0, u32::MAX, u32::MAX],
        );
        let remote = fixture.node("Remote focus");
        fullscreen_multi_output::place(&mut fixture, remote.id, true);
        let input = fullscreen_multi_output::Input::new(&fixture);
        input.motion(f64::from(width) / 2.0, f64::from(height) / 2.0);
        wait_for("source hover focus", || {
            (input.state()["focused"] == source.id).then_some(())
        });
        let snapshot = input.state();
        let rect = snapshot["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == remote.id)
            .unwrap()["rect"]
            .as_array()
            .unwrap();
        input.motion(
            f64::from(width) + rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0,
            rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0,
        );
        wait_for("remote hover focus", || {
            (input.state()["focused"] == remote.id).then_some(())
        });
        // Model a game's video-mode reaction to losing focus. The other
        // monitor remains usable without releasing the game's presentation.
        fullscreen(&connection, window, false);
        thread::sleep(Duration::from_millis(120));
        let snapshot = input.state();
        let observed = snapshot["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == source.id)
            .unwrap();
        assert_eq!(observed["fullscreen"], game, "game={game}: {snapshot}");
        if game {
            assert_eq!(observed["rect"], serde_json::json!([0, 0, width, height]));
            input.motion(f64::from(width) / 2.0, f64::from(height) / 2.0);
            wait_for("game regains hover focus", || {
                (input.state()["focused"] == source.id).then_some(())
            });
            fullscreen(&connection, window, false);
            wait_for("focused game may choose windowed mode", || {
                let snapshot = input.state();
                snapshot["windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|w| w["id"] == source.id)
                    .filter(|w| w["fullscreen"] == false)
                    .map(|_| ())
            });
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; initial fullscreen restore placement"]
fn initial_fullscreen_game_centers_its_first_windowed_video_mode() {
    let extra = "animations:\n enabled false\nend\nrules:\n window-rule:\n title \"Fullscreen negotiation\"\n game true\n end\nend\n";
    let (mut fixture, connection, window, width, height) = setup_with_config(true, extra);
    let id = fixture.node("Fullscreen negotiation").id;
    let input = fullscreen_multi_output::Input::new(&fixture);
    fullscreen(&connection, window, false);
    connection
        .configure_window(window, &ConfigureWindowAux::new().width(640).height(400))
        .unwrap()
        .check()
        .unwrap();
    wait_for("first windowed video mode accepted", || {
        let g = connection.get_geometry(window).unwrap().reply().unwrap();
        (g.width == 640 && g.height == 400).then_some(())
    });
    thread::sleep(Duration::from_millis(350));
    let snapshot = input.state();
    let observed = snapshot["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id)
        .unwrap();
    let rect = observed["outer_geometry"].as_array().unwrap();
    let center = (
        rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0,
        rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0,
    );
    assert!(
        (center.0 - f64::from(width) / 2.0).abs() < 1.0
            && (center.1 - f64::from(height) / 2.0).abs() < 1.0,
        "first windowed mode lost its center: {snapshot}"
    );
    let previous = observed["client_geometry"].as_array().unwrap();
    let location = [previous[0].clone(), previous[1].clone()];
    connection
        .configure_window(window, &ConfigureWindowAux::new().width(600).height(380))
        .unwrap()
        .check()
        .unwrap();
    wait_for("later ordinary resize keeps its placement", || {
        let state = input.state();
        let geometry = state["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap()["client_geometry"]
            .as_array()
            .unwrap();
        if geometry[2] != 600 || geometry[3] != 380 {
            return None;
        }
        assert_eq!(
            [geometry[0].clone(), geometry[1].clone()],
            location,
            "restore centering outlived the mode-change quiet period"
        );
        Some(())
    });
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
                let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
                assert_eq!((geometry.width, geometry.height), (width, height));
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
            assert_eq!(state["fullscreen"], game || enabled);
            assert_eq!(state["paused"], game && !enabled);
            assert_eq!(state["animating"], false);
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; client mode changes after Mod+F"]
fn game_mode_requests_work_after_compositor_fullscreen_changes() {
    for game in [false, true] {
        let extra = format!(
            "animations:\n enabled false\nend\nrules:\n window-rule:\n title \"Fullscreen negotiation\"\n game {game}\n end\nend\n"
        );
        let (mut fixture, connection, window, width, height) = setup_with_config(false, &extra);
        let input = fullscreen_multi_output::Input::new(&fixture);
        let id = fixture.node("Fullscreen negotiation").id;
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let net_state = atom(b"_NET_WM_STATE");
        let full = atom(b"_NET_WM_STATE_FULLSCREEN");
        let extents = atom(b"_NET_FRAME_EXTENTS");
        let presented = || {
            let state = input.state();
            state["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["id"] == id)
                .unwrap()
                .clone()
        };
        // Begin in client fullscreen, then exercise both compositor edges.
        for expected in [false, true] {
            input.key(68, true);
            input.key(68, false);
            wait_for("compositor fullscreen change", || {
                let state = presented();
                (state["fullscreen"] == (game || expected)
                    && state["paused"] == (game && !expected))
                    .then_some(())
            });
        }
        // The game's display menu must still be able to choose windowed,
        // borderless-sized, and fullscreen modes after either Mod+F choice.
        for enabled in [false, true, false] {
            fullscreen(&connection, window, enabled);
            if !enabled {
                connection
                    .configure_window(window, &ConfigureWindowAux::new().width(360).height(220))
                    .unwrap()
                    .check()
                    .unwrap();
            }
            wait_for("client mode supersedes previous Mod+F choice", || {
                let observed = presented();
                let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
                let size = if enabled { (width, height) } else { (360, 220) };
                (observed["fullscreen"] == enabled && (geometry.width, geometry.height) == size)
                    .then_some(())
            });
            let state = connection
                .get_property(false, window, net_state, AtomEnum::ATOM, 0, 1024)
                .unwrap()
                .reply()
                .unwrap();
            assert_eq!(state.value32().unwrap().any(|a| a == full), enabled);
            if game && enabled {
                let frame = connection
                    .get_property(false, window, extents, AtomEnum::CARDINAL, 0, 4)
                    .unwrap()
                    .reply()
                    .unwrap();
                assert_eq!(frame.value32().unwrap().collect::<Vec<_>>(), vec![0; 4]);
            }
        }
        fullscreen(&connection, window, true);
        wait_for("client fullscreen before compositor exit", || {
            (presented()["fullscreen"] == true).then_some(())
        });
        input.key(68, true);
        input.key(68, false);
        wait_for("explicit fullscreen exit", || {
            let state = presented();
            (state["fullscreen"] == game && state["paused"] == game).then_some(())
        });
        if game {
            let state = connection
                .get_property(false, window, net_state, AtomEnum::ATOM, 0, 1024)
                .unwrap()
                .reply()
                .unwrap();
            assert!(state.value32().unwrap().any(|atom| atom == full));
            let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
            assert_eq!((geometry.width, geometry.height), (width, height));
            let state = presented();
            let rect = state["rect"].as_array().unwrap();
            input.motion(
                rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0,
                rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0,
            );
            input.button(272, true);
            input.button(272, false);
            assert_eq!(
                presented()["paused"],
                true,
                "clicking the shrunken game must not undo Mod+F"
            );
        }
        fullscreen(&connection, window, true);
        wait_for("client may enter fullscreen after compositor exit", || {
            let state = presented();
            (state["fullscreen"] == true && state["paused"] == false).then_some(())
        });
        // Duplicate requests still get state and geometry replies without
        // creating another presentation transaction.
        connection
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
            )
            .unwrap()
            .check()
            .unwrap();
        for _ in 0..2 {
            while connection.poll_for_event().unwrap().is_some() {}
            fullscreen(&connection, window, true);
            if game {
                let (mut state_reply, mut geometry_reply) = (false, false);
                wait_for(
                    "unchanged game mode gets state and geometry replies",
                    || {
                        while let Some(event) = connection.poll_for_event().unwrap() {
                            match event {
                                x11rb::protocol::Event::PropertyNotify(event) => {
                                    state_reply |= event.window == window && event.atom == net_state
                                }
                                x11rb::protocol::Event::ConfigureNotify(event) => {
                                    geometry_reply |= event.window == window
                                        && event.response_type & 0x80 != 0
                                        && (event.width, event.height) == (width, height)
                                }
                                _ => {}
                            }
                        }
                        (state_reply && geometry_reply).then_some(())
                    },
                );
            }
        }
        connection.unmap_window(window).unwrap().check().unwrap();
        wait_for("game withdrawn", || {
            (!fixture.nodes().iter().any(|n| n.id == id)).then_some(())
        });
        connection.map_window(window).unwrap().check().unwrap();
        fixture.node("Fullscreen negotiation");
        fullscreen(&connection, window, false);
        wait_for("remapped client can leave fullscreen", || {
            let state = input.state();
            state["windows"]
                .as_array()
                .unwrap()
                .first()
                .filter(|w| w["fullscreen"] == false)
                .map(|_| ())
        });
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; smooth negotiated game exit"]
fn game_windowed_size_burst_retargets_the_smooth_exit_endpoint() {
    let (mut fixture, connection, window, _, _) = setup_with_config(
        false,
        r#"
rules:
  window-rule:
    title "Fullscreen negotiation"
    game true
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
"#,
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let id = fixture.node("Fullscreen negotiation").id;
    let presented = || {
        input.state()["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap()
            .clone()
    };
    thread::sleep(Duration::from_millis(1100));
    fullscreen(&connection, window, false);
    connection
        .configure_window(window, &ConfigureWindowAux::new().width(360).height(220))
        .unwrap()
        .check()
        .unwrap();
    wait_for("negotiated game windowed size", || {
        let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
        (geometry.width == 360 && geometry.height == 220).then_some(())
    });
    thread::sleep(Duration::from_millis(850));
    let state = presented();
    let width = state["rect"][2].as_f64().unwrap();
    assert_eq!(state["animating"], true, "{state}");
    assert!(width >= 360.0 && width < 550.0, "{state}");
    thread::sleep(Duration::from_millis(250));
    let state = presented();
    assert_eq!(state["animating"], false, "{state}");
    assert_eq!(state["rect"][2], 360, "{state}");
    assert_eq!(state["rect"][3], 220, "{state}");
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

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; decorated game presentation"]
fn shrunken_fullscreen_game_has_rounded_interactive_window_frame() {
    let (mut fixture, connection, window, width, height) = setup_with_config(
        false,
        r##"
animations:
 enabled false
end
rules:
 window-rule:
  title "Fullscreen negotiation"
  game true
 end
end
decorations:
 border:
  size 0
  radius 20
 end
 resize-using-border true
 titlebars:
  enabled true
  height 32
  radius 20
  colour-focused "#00ff00"
  colour-unfocused "#00ff00"
  show-title false
  show-icons false
  show-buttons true
 end
end
"##,
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let id = fixture.node("Fullscreen negotiation").id;
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let delete = atom(b"WM_DELETE_WINDOW");
    connection
        .change_property32(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            atom(b"WM_PROTOCOLS"),
            AtomEnum::ATOM,
            &[delete],
        )
        .unwrap()
        .check()
        .unwrap();
    let presented = || {
        input.state()["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap()
            .clone()
    };
    let rect = || {
        let s = presented();
        let r = s["rect"].as_array().unwrap();
        (
            r[0].as_i64().unwrap() as i32,
            r[1].as_i64().unwrap() as i32,
            r[2].as_i64().unwrap() as i32,
            r[3].as_i64().unwrap() as i32,
        )
    };
    let click = |x: i32, y: i32| {
        input.motion(f64::from(x), f64::from(y));
        input.button(272, true);
        input.button(272, false);
    };
    input.key(68, true);
    input.key(68, false);
    wait_for("game presented windowed", || {
        (presented()["paused"] == true).then_some(())
    });
    let (x, y, w, h) = rect();
    wait_for("server titlebar and rounded live client", || {
        let (pixels, stride) = capture_frame_pixels_on(&mut fixture, Some("winit"));
        let pixel = |px: i32, py: i32| &pixels[(py as usize * stride + px as usize) * 4..][..3];
        let title = pixel(x + 25, y - 16);
        let center = pixel(x + w / 2, y + h / 2);
        let corner = pixel(x + w - 2, y + h - 2);
        (title[1] > 220
            && title[0] < 40
            && title[2] < 40
            && center.iter().all(|v| *v > 240)
            && corner.iter().all(|v| *v < 180))
        .then_some(())
    });
    // Dragging and resizing manipulate the presentation, preserving the
    // client's fullscreen state and output-sized swapchain throughout.
    input.motion(f64::from(x + 40), f64::from(y - 16));
    input.button(272, true);
    input.motion(f64::from(x + 130), f64::from(y + 34));
    input.button(272, false);
    wait_for("titlebar drag and release", || {
        let r = rect();
        (r.0 == x + 90 && r.1 == y + 50 && input.state()["moving"] == false).then_some(())
    });
    let (x, y, w, h) = rect();
    input.motion(f64::from(x + w - 1), f64::from(y + h - 1));
    input.button(272, true);
    input.motion(f64::from(x + w + 39), f64::from(y + h + 29));
    input.button(272, false);
    wait_for("frame resize", || {
        let r = rect();
        (r.2 == w + 40 && r.3 == h + 30).then_some(())
    });
    let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (width, height));
    assert!(
        connection
            .get_property(
                false,
                window,
                atom(b"_NET_WM_STATE"),
                AtomEnum::ATOM,
                0,
                1024
            )
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .unwrap()
            .any(|a| a == atom(b"_NET_WM_STATE_FULLSCREEN"))
    );
    let (x, y, _, _) = rect();
    click(x + 40, y - 16);
    click(x + 40, y - 16);
    wait_for(
        "double-click restores the immersive game presentation",
        || (presented()["paused"] == false).then_some(()),
    );
    input.key(68, true);
    input.key(68, false);
    let (x, y, w, _) = rect();
    click(x + w - 48, y - 16); // Maximize restores the immersive game presentation.
    wait_for("maximize game presentation", || {
        (presented()["paused"] == false).then_some(())
    });
    input.key(68, true);
    input.key(68, false);
    let (x, y, w, _) = rect();
    click(x + w - 80, y - 16);
    wait_for("minimize from the server frame", || {
        let state = connection
            .get_property(false, window, atom(b"WM_STATE"), atom(b"WM_STATE"), 0, 2)
            .unwrap()
            .reply()
            .unwrap();
        (state.value32().and_then(|mut v| v.next()) == Some(3)).then_some(())
    });
    assert_eq!(
        connection
            .get_window_attributes(window)
            .unwrap()
            .reply()
            .unwrap()
            .map_state,
        x11rb::protocol::xproto::MapState::UNVIEWABLE,
        "minimize must hide the X11 frame as well as the compositor view"
    );
    // A client may repeat the iconify request after observing the WM's own
    // minimize. This is still the accepted state, not a failed new operation.
    connection
        .send_event(
            false,
            connection.setup().roots[0].root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, window, atom(b"WM_CHANGE_STATE"), [3, 0, 0, 0, 0]),
        )
        .unwrap();
    connection.flush().unwrap();
    thread::sleep(Duration::from_millis(200));
    let wm_state = connection
        .get_property(false, window, atom(b"WM_STATE"), atom(b"WM_STATE"), 0, 2)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(
        wm_state.value32().unwrap().next(),
        Some(3),
        "repeated minimize must remain iconic"
    );
    // Wine can unmap/remap the same XID while acknowledging iconification.
    // That is still this minimized window, not a new fullscreen launch.
    connection.unmap_window(window).unwrap().check().unwrap();
    connection.map_window(window).unwrap().check().unwrap();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(presented()["rect"], serde_json::Value::Null);
    fixture.ack(Request::Node(NodeRequest::Restore {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    wait_for("restore remaps the X11 frame", || {
        (connection
            .get_window_attributes(window)
            .unwrap()
            .reply()
            .unwrap()
            .map_state
            == x11rb::protocol::xproto::MapState::VIEWABLE)
            .then_some(())
    });
    wait_for("restored presentation is associated", || {
        presented()["rect"].is_array().then_some(())
    });
    let (x, y, w, _) = rect();
    click(x + w - 80, y - 16);
    wait_for("second minimize", || {
        presented()["rect"].is_null().then_some(())
    });
    connection.unmap_window(window).unwrap().check().unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(presented()["rect"].is_null());
    fixture.ack(Request::Node(NodeRequest::Restore {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    wait_for(
        "restore requests a fresh map when the client is unmapped",
        || {
            (connection
                .get_window_attributes(window)
                .unwrap()
                .reply()
                .unwrap()
                .map_state
                == x11rb::protocol::xproto::MapState::VIEWABLE
                && presented()["rect"].is_array())
            .then_some(())
        },
    );
    let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (width, height));
    assert_eq!(presented()["paused"], true);
    let (x, y, w, _) = rect();
    click(x + w - 16, y - 16);
    wait_for("close from the server frame", || {
        while let Some(event) = connection.poll_for_event().unwrap() {
            if let x11rb::protocol::Event::ClientMessage(event) = event
                && event.window == window
                && event.data.as_data32()[0] == delete
            {
                return Some(());
            }
        }
        None
    });
}
