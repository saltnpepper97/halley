//! Alt+Tab changes presentation only when revealing a target needs camera motion.
use super::fullscreen_multi_output::Input;
use super::*;
use serde_json::Value;
use x11rb::protocol::xproto::{ClientMessageEvent, EventMask};

fn window(input: &Input, id: u64) -> Value {
    input.state()["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|window| window["id"] == id)
        .unwrap()
        .clone()
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; Alt+Tab visibility"]
fn native_alt_tab_preserves_visible_presentations_and_reveals_offscreen_targets() {
    for owner in ["client", "compositor", "both", "maximize", "none"] {
        for visibility in ["visible", "edge", "partial", "outside"] {
            scenario(false, false, owner, visibility);
        }
    }
    for visibility in ["visible", "partial", "outside"] {
        scenario(false, true, "client", visibility);
    }
    for owner in ["client", "maximize"] {
        for visibility in ["collapsed-visible", "collapsed-partial"] {
            scenario(false, false, owner, visibility);
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; X11 Alt+Tab visibility"]
fn x11_alt_tab_preserves_visible_presentations_and_reveals_offscreen_targets() {
    for owner in ["client", "maximize"] {
        for visibility in ["visible", "partial", "outside"] {
            scenario(true, false, owner, visibility);
        }
    }
    for visibility in ["visible", "partial", "outside"] {
        scenario(true, true, "client", visibility);
    }
}

fn scenario(x11: bool, game: bool, owner: &str, visibility: &str) {
    scenario_with_animations(x11, game, owner, visibility, false);
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; animated Alt+Tab exits"]
fn animated_alt_tab_reveal_finishes_after_presentation_exit() {
    for owner in ["client", "maximize"] {
        for visibility in ["visible", "partial", "outside"] {
            scenario_with_animations(false, false, owner, visibility, true);
        }
    }
}

fn scenario_with_animations(x11: bool, game: bool, owner: &str, visibility: &str, animated: bool) {
    eprintln!("Alt+Tab visibility: x11={x11} game={game} owner={owner} target={visibility}");
    let config = format!(
        "animations:\n enabled {animated}\n window-open:\n enabled false\n end\n window-move:\n enabled false\n end\nend\ninput:\n focus-mode \"click\"\nend\nrules:\n window-rule:\n app-id \"halley.test.visibility\"\n game {game}\n end\nend\n"
    );
    let mut fixture = Fixture::with_two_outputs(&config);
    let input = Input::new(&fixture);
    let mut native = None;
    let xsource = if x11 {
        let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
        let source = x11_window(&connection, "Visibility owner");
        let class = if game {
            "steam_app_440"
        } else {
            "halley.test.x11.visibility"
        };
        connection
            .change_property8(
                x11rb::protocol::xproto::PropMode::REPLACE,
                source,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                format!("{class}\0{class}\0").as_bytes(),
            )
            .unwrap();
        connection.flush().unwrap();
        Some((connection, source))
    } else {
        let (queue, mut state) = native_window_named(
            &fixture,
            "Visibility owner",
            "halley.test.visibility",
            [0, u32::MAX, 0, u32::MAX],
        );
        state.resize_for_configures = true;
        native = Some((queue, state));
        None
    };
    let source = fixture.node("Visibility owner");
    let (_target_queue, _target_state) = native_window_named(
        &fixture,
        "Visibility target",
        "halley.test.visibility.target",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let target = fixture.node("Visibility target");
    let collapsed = visibility.starts_with("collapsed-");
    let visibility = visibility.strip_prefix("collapsed-").unwrap_or(visibility);
    if collapsed {
        fixture.ack(Request::Node(NodeRequest::Collapse {
            selector: Some(NodeSelector::Id(target.id)),
            output: None,
        }));
    }
    assert_eq!(source.output, target.output);
    request_native_focus(&mut fixture, source.id);
    if let Some((connection, source)) = &xsource {
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let (first, second) = if owner == "maximize" {
            (
                atom(b"_NET_WM_STATE_MAXIMIZED_HORZ"),
                atom(b"_NET_WM_STATE_MAXIMIZED_VERT"),
            )
        } else {
            (atom(b"_NET_WM_STATE_FULLSCREEN"), 0)
        };
        connection
            .send_event(
                false,
                connection.setup().roots[0].root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(
                    32,
                    *source,
                    atom(b"_NET_WM_STATE"),
                    [1, first, second, 1, 0],
                ),
            )
            .unwrap();
        connection.flush().unwrap();
    } else {
        let (queue, state) = native.as_mut().unwrap();
        if matches!(owner, "compositor" | "both") {
            let _manager: foreign_manager::ZwlrForeignToplevelManagerV1 =
                state.registry.as_ref().unwrap().bind(
                    state.globals["zwlr_foreign_toplevel_manager_v1"],
                    3,
                    &queue.handle(),
                    (),
                );
            let handle = wait_for("visibility source foreign handle", || {
                queue.roundtrip(state).unwrap();
                state.foreign_titles.get("Visibility owner").cloned()
            });
            handle.set_fullscreen(None);
            queue.roundtrip(state).unwrap();
        }
        match owner {
            "client" | "both" => state.toplevel.as_ref().unwrap().set_fullscreen(None),
            "maximize" => state.toplevel.as_ref().unwrap().set_maximized(),
            "compositor" | "none" => (),
            _ => unreachable!(),
        }
    }
    let pump = |native: &mut Option<(EventQueue<NativeState>, NativeState)>| {
        if let Some((queue, state)) = native {
            queue.roundtrip(state).unwrap();
        }
    };
    wait_for("source presentation settled", || {
        pump(&mut native);
        let snapshot = window(&input, source.id);
        let ready = if owner == "maximize" {
            snapshot["maximized"] == true
        } else if owner == "none" {
            true
        } else {
            snapshot["fullscreen"] == true && snapshot["rect"][2].as_i64().unwrap() > 1000
        };
        (ready && snapshot["animating"] == false && snapshot["camera_settled"] == true)
            .then_some(())
    });
    // Let configure/commit finish before measuring the camera and window bounds.
    for _ in 0..3 {
        pump(&mut native);
        thread::sleep(Duration::from_millis(20));
    }
    let before = window(&input, source.id);
    let viewport: [i32; 4] =
        std::array::from_fn(|i| before["viewport"][i].as_i64().unwrap() as i32);
    let target_width = target.width as i32;
    let target_snapshot = window(&input, target.id);
    let decorated_right = target_snapshot["outer_geometry"][0].as_i64().unwrap()
        + target_snapshot["outer_geometry"][2].as_i64().unwrap()
        - target_snapshot["client_geometry"][0].as_i64().unwrap();
    let x = match visibility {
        "visible" => viewport[0] + 90,
        "edge" => viewport[0] + viewport[2] - decorated_right as i32,
        "partial" => viewport[0] + viewport[2] - target_width / 2,
        "outside" => viewport[0] + viewport[2] + 80,
        _ => unreachable!(),
    };
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: x + if collapsed { target_width / 2 } else { 0 },
        y: viewport[1]
            + 120
            + if collapsed {
                target.height as i32 / 2
            } else {
                0
            },
        selector: Some(NodeSelector::Id(target.id)),
        output: None,
    }));
    let target_before = window(&input, target.id);
    let needs_pan = matches!(visibility, "partial" | "outside");
    input.preview(target.id);
    assert_eq!(window(&input, source.id)["view"], before["view"]);
    assert_eq!(
        window(&input, source.id)["fullscreen"],
        before["fullscreen"]
    );
    input.cancel();
    input.preview(target.id);
    input.commit(target.id);
    wait_for("Alt+Tab visibility policy and client restore", || {
        pump(&mut native);
        let current = window(&input, source.id);
        let expected_fullscreen = owner != "none" && owner != "maximize" && (!needs_pan || game);
        let expected_maximize = owner == "maximize" && !needs_pan;
        if current["fullscreen"] != expected_fullscreen
            || current["maximized"] != expected_maximize
            || current["paused"] != (needs_pan && game)
        {
            return None;
        }
        if current["animating"] == true || current["camera_settled"] == false {
            return None;
        }
        let selected = window(&input, target.id);
        let rect: [i64; 4] = std::array::from_fn(|i| selected["rect"][i].as_i64().unwrap());
        let fits = rect[0] >= 0
            && rect[1] >= 0
            && rect[0] + rect[2] <= viewport[2] as i64
            && rect[1] + rect[3] <= viewport[3] as i64;
        if !fits {
            return None;
        }
        if !needs_pan {
            assert_eq!(
                current["view"], before["view"],
                "visible target moved the camera"
            );
            if !collapsed {
                assert_eq!(
                    selected["rect"], target_before["rect"],
                    "visible target was centered"
                );
            }
        }
        Some(())
    });
    if let Some((_, state)) = &native {
        assert_eq!(
            state.configured_fullscreen,
            matches!(owner, "client" | "both") && (!needs_pan || game)
        );
        assert_eq!(
            state.configured_maximized,
            owner == "maximize" && !needs_pan
        );
    }
    if let Some((connection, source)) = &xsource {
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let property = connection
            .get_property(
                false,
                *source,
                atom(b"_NET_WM_STATE"),
                AtomEnum::ATOM,
                0,
                32,
            )
            .unwrap()
            .reply()
            .unwrap();
        let flags = property.value32().unwrap().collect::<Vec<_>>();
        assert_eq!(
            flags.contains(&atom(b"_NET_WM_STATE_FULLSCREEN")),
            owner == "client" && (!needs_pan || game)
        );
        for name in [
            b"_NET_WM_STATE_MAXIMIZED_HORZ",
            b"_NET_WM_STATE_MAXIMIZED_VERT",
        ] {
            assert_eq!(
                flags.contains(&atom(name)),
                owner == "maximize" && !needs_pan
            );
        }
    }
    // State alone cannot prove the raised target is visible above the presentation.
    let selected = window(&input, target.id);
    let (pixels, width) = capture_frame_pixels(&mut fixture);
    let x = selected["rect"][0].as_i64().unwrap() as usize
        + selected["rect"][2].as_i64().unwrap() as usize / 2;
    let y = selected["rect"][1].as_i64().unwrap() as usize
        + selected["rect"][3].as_i64().unwrap() as usize / 2;
    let pixel = &pixels[(y * width + x) * 4..][..3];
    assert!(
        pixel[2] > 240 && pixel[0] < 15 && pixel[1] < 15,
        "raised target is covered: {pixel:?}"
    );
}
