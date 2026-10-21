//! Presentation-only growth must not change permanent Field landmark positions.
use super::*;
use x11rb::protocol::xproto::{ClientMessageEvent, ConfigureWindowAux, EventMask};

fn nearby_node(
    fixture: &mut Fixture,
    window: &halley_ipc::NodeInfo,
) -> (u64, EventQueue<NativeState>, NativeState) {
    let (queue, state) = native_window_named(
        fixture,
        "Nearby landmark",
        "halley.test.landmark",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let node = fixture.node("Nearby landmark");
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: (window.pos_x + window.width / 2.0 + 80.0).round() as i32,
        y: window.pos_y.round() as i32,
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    (node.id, queue, state)
}

fn position(fixture: &mut Fixture, id: u64) -> (f32, f32) {
    let node = fixture.nodes().into_iter().find(|n| n.id == id).unwrap();
    (node.pos_x, node.pos_y)
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; fullscreen/maximize landmark stability"]
fn native_fullscreen_and_maximize_leave_nearby_landmarks_in_place() {
    for game in [false, true] {
        for owner in ["client", "compositor", "both", "maximize"] {
            eprintln!("landmark stability: native game={game} owner={owner}");
            let config = format!(
                "animations:\n enabled false\nend\ninput:\n focus-mode \"click\"\nend\nrules:\n window-rule:\n app-id \"halley.test.presentation\"\n game {game}\n end\nend\n"
            );
            let mut fixture = Fixture::with_config("stacking", &config);
            let (mut queue, mut state) = native_window_named(
                &fixture,
                "Presentation owner",
                "halley.test.presentation",
                [0, u32::MAX, 0, u32::MAX],
            );
            state.resize_for_configures = true;
            let window = fixture.node("Presentation owner");
            let (landmark, _landmark_queue, _landmark_state) = nearby_node(&mut fixture, &window);
            let home = position(&mut fixture, landmark);
            request_native_focus(&mut fixture, window.id);
            let _manager: foreign_manager::ZwlrForeignToplevelManagerV1 =
                state.registry.as_ref().unwrap().bind(
                    state.globals["zwlr_foreign_toplevel_manager_v1"],
                    3,
                    &queue.handle(),
                    (),
                );
            let handle = wait_for("presentation foreign handle", || {
                queue.roundtrip(&mut state).unwrap();
                state.foreign_titles.get("Presentation owner").cloned()
            });
            for enabled in [true, false] {
                let before = state.configure_count;
                let delayed_restore = !enabled && matches!(owner, "compositor" | "maximize");
                if delayed_restore {
                    // Ack the exit but deliberately keep the enlarged buffer:
                    // presentation still owns this geometry until it restores.
                    state.resize_for_configures = false;
                }
                match owner {
                    "client" => {
                        if enabled {
                            state.toplevel.as_ref().unwrap().set_fullscreen(None);
                        } else {
                            state.toplevel.as_ref().unwrap().unset_fullscreen();
                        }
                    }
                    "compositor" => {
                        if enabled {
                            handle.set_fullscreen(None);
                        } else {
                            handle.unset_fullscreen();
                        }
                    }
                    "both" => {
                        if enabled {
                            handle.set_fullscreen(None);
                            state.toplevel.as_ref().unwrap().set_fullscreen(None);
                        } else {
                            state.toplevel.as_ref().unwrap().unset_fullscreen();
                            handle.unset_fullscreen();
                        }
                    }
                    "maximize" => {
                        if enabled {
                            state.toplevel.as_ref().unwrap().set_maximized();
                        } else {
                            state.toplevel.as_ref().unwrap().unset_maximized();
                        }
                    }
                    _ => unreachable!(),
                }
                if delayed_restore {
                    wait_for("exit configure acknowledged before buffer restore", || {
                        queue.roundtrip(&mut state).unwrap();
                        assert_eq!(position(&mut fixture, landmark), home);
                        (state.configure_count > before).then_some(())
                    });
                    for _ in 0..3 {
                        state.surface.as_ref().unwrap().commit();
                        queue.roundtrip(&mut state).unwrap();
                        assert_eq!(position(&mut fixture, landmark), home);
                    }
                    state
                        .viewport
                        .as_ref()
                        .unwrap()
                        .set_destination(window.width as i32, window.height as i32);
                    state.surface.as_ref().unwrap().commit();
                    state.resize_for_configures = true;
                }
                wait_for("presentation geometry committed", || {
                    queue.roundtrip(&mut state).unwrap();
                    assert_eq!(
                        position(&mut fixture, landmark),
                        home,
                        "{owner} enabled={enabled} displaced the landmark"
                    );
                    let current = fixture.node("Presentation owner");
                    let expected = if enabled {
                        current.width > window.width + 100.0
                    } else {
                        (current.width, current.height) == (window.width, window.height)
                    };
                    expected.then_some(())
                });
                // Surface commits continue during video/game playback and must
                // never turn a presentation into a Field obstacle.
                for _ in 0..3 {
                    state.surface.as_ref().unwrap().commit();
                    queue.roundtrip(&mut state).unwrap();
                    assert_eq!(position(&mut fixture, landmark), home);
                }
                if enabled {
                    // All node placement forms use the same Field blockers.
                    for direction in [
                        halley_ipc::NodeMoveDirection::Left,
                        halley_ipc::NodeMoveDirection::Right,
                    ] {
                        fixture.ack(Request::Node(NodeRequest::MoveBy {
                            direction,
                            distance: 10,
                            selector: Some(NodeSelector::Id(landmark)),
                            output: None,
                        }));
                    }
                    assert_eq!(position(&mut fixture, landmark), home);
                    fixture.ack(Request::Node(NodeRequest::MoveTo {
                        x: home.0 as i32,
                        y: home.1 as i32,
                        selector: Some(NodeSelector::Id(landmark)),
                        output: None,
                    }));
                    for direction in [
                        halley_ipc::NodeMoveDirection::Left,
                        halley_ipc::NodeMoveDirection::Right,
                    ] {
                        fixture.ack(Request::Node(NodeRequest::Move {
                            direction,
                            selector: Some(NodeSelector::Id(landmark)),
                            output: None,
                        }));
                    }
                    assert_eq!(position(&mut fixture, landmark), home);
                }
            }
            // Normal client-driven Field resizing must still push the node.
            state
                .viewport
                .as_ref()
                .unwrap()
                .set_destination(window.width as i32 + 200, window.height as i32);
            state.surface.as_ref().unwrap().commit();
            wait_for("ordinary Field resize displaces landmark", || {
                queue.roundtrip(&mut state).unwrap();
                (position(&mut fixture, landmark) != home).then_some(())
            });
        }
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; EWMH fullscreen/maximize landmark stability"]
fn x11_fullscreen_and_maximize_leave_nearby_landmarks_in_place() {
    for game in [false, true] {
        for maximize in [false, true] {
            eprintln!("landmark stability: X11 game={game} maximize={maximize}");
            let mut fixture = Fixture::with_config(
                "stacking",
                "animations:\n enabled false\nend\ninput:\n focus-mode \"click\"\nend\n",
            );
            let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
            let xid = x11_window(&connection, "Presentation owner");
            let class = if game {
                "steam_app_440"
            } else {
                "halley.test.presentation"
            };
            connection
                .change_property8(
                    x11rb::protocol::xproto::PropMode::REPLACE,
                    xid,
                    AtomEnum::WM_CLASS,
                    AtomEnum::STRING,
                    format!("{class}\0{class}\0").as_bytes(),
                )
                .unwrap();
            connection.flush().unwrap();
            let window = fixture.node("Presentation owner");
            let (landmark, _landmark_queue, _landmark_state) = nearby_node(&mut fixture, &window);
            let home = position(&mut fixture, landmark);
            request_native_focus(&mut fixture, window.id);
            let atom = |name: &[u8]| {
                connection
                    .intern_atom(false, name)
                    .unwrap()
                    .reply()
                    .unwrap()
                    .atom
            };
            let (first, second) = if maximize {
                (
                    atom(b"_NET_WM_STATE_MAXIMIZED_HORZ"),
                    atom(b"_NET_WM_STATE_MAXIMIZED_VERT"),
                )
            } else {
                (atom(b"_NET_WM_STATE_FULLSCREEN"), 0)
            };
            for enabled in [true, false] {
                connection
                    .send_event(
                        false,
                        connection.setup().roots[0].root,
                        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                        ClientMessageEvent::new(
                            32,
                            xid,
                            atom(b"_NET_WM_STATE"),
                            [u32::from(enabled), first, second, 1, 0],
                        ),
                    )
                    .unwrap();
                connection.flush().unwrap();
                wait_for("X11 presentation geometry committed", || {
                    assert_eq!(
                        position(&mut fixture, landmark),
                        home,
                        "maximize={maximize} enabled={enabled} displaced the landmark"
                    );
                    let current = fixture.node("Presentation owner");
                    let expected = if enabled {
                        current.width > window.width + 100.0
                    } else {
                        (current.width, current.height) == (window.width, window.height)
                    };
                    expected.then_some(())
                });
                // Make the server process another client commit at this size.
                connection.clear_area(true, xid, 0, 0, 0, 0).unwrap();
                connection.flush().unwrap();
                thread::sleep(Duration::from_millis(50));
                assert_eq!(position(&mut fixture, landmark), home);
            }
            connection
                .configure_window(
                    xid,
                    &ConfigureWindowAux::new().width(window.width as u32 + 200),
                )
                .unwrap();
            connection.flush().unwrap();
            wait_for("ordinary X11 Field resize displaces landmark", || {
                (position(&mut fixture, landmark) != home).then_some(())
            });
        }
    }
}
