//! Two real logical outputs, protocol clients, captured pixels and raw keyboard
//! events through the production handler. No direct calls to focus-cycle commit.
use super::*;
use serde_json::Value;
use std::os::unix::net::UnixDatagram;
use x11rb::protocol::xproto::{ClientMessageEvent, EventMask};

pub(super) struct Input(UnixDatagram);
impl Input {
    pub(super) fn new(fixture: &Fixture) -> Self {
        let socket = UnixDatagram::bind(fixture.path.join("input-client.sock")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket.connect(fixture.path.join("input.sock")).unwrap();
        Self(socket)
    }
    fn command(&self, command: &str) -> Value {
        self.0.send(command.as_bytes()).unwrap();
        let mut bytes = [0; 32_768];
        let size = self.0.recv(&mut bytes).unwrap();
        serde_json::from_slice(&bytes[..size]).unwrap()
    }
    pub(super) fn state(&self) -> Value {
        self.command("state")
    }
    pub(super) fn key(&self, code: u32, pressed: bool) -> Value {
        self.command(&format!(
            "key {code} {}",
            if pressed { "down" } else { "up" }
        ))
    }
    pub(super) fn seat_key(&self, code: u32, pressed: bool, count: u32) -> Value {
        self.command(&format!(
            "seat-key {code} {} {count}",
            if pressed { "down" } else { "up" }
        ))
    }
    fn preview(&self, id: u64) {
        self.key(56, true); // Left Alt
        for _ in 0..8 {
            self.key(15, true); // Tab
            let snapshot = self.key(15, false);
            assert_eq!(snapshot["open"], true, "Alt+Tab failed to open: {snapshot}");
            if snapshot["preview"] == id {
                return;
            }
        }
        panic!("Alt+Tab never previewed {id}: {}", self.state());
    }
    fn closed(&self) {
        wait_for("switcher closing animation", || {
            (!self.state()["active"].as_bool().unwrap()).then_some(())
        });
    }
    fn cancel(&self) {
        self.key(1, true); // Escape while Alt is held
        self.key(1, false);
        self.key(56, false);
        self.closed();
    }
    fn commit(&self, id: u64) {
        let reply = self.key(56, false); // The real Alt-release commit path.
        assert_eq!(
            reply["focused"], id,
            "Alt release did not activate selection: {reply}"
        );
        self.closed();
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; two-output raw-key Alt+Tab"]
fn native_apps_cross_monitor_alt_tab_preserves_all_fullscreen_owners() {
    for owner in ["client", "compositor", "both"] {
        for second in [false, true] {
            scenario(false, false, owner, second);
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; two-output game Alt+Tab"]
fn native_games_cross_monitor_alt_tab_preserves_fullscreen_and_local_parking() {
    for second in [false, true] {
        scenario(false, true, "client", second);
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; two-output X11 Alt+Tab"]
fn x11_apps_and_games_cross_monitor_alt_tab_preserve_fullscreen() {
    for game in [false, true] {
        for second in [false, true] {
            scenario(true, game, "client", second);
        }
    }
}

fn place(fixture: &mut Fixture, id: u64, second: bool) {
    let target = if second { "winit-2" } else { "winit" };
    if fixture
        .nodes()
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .output
        .as_deref()
        != Some(target)
    {
        request_native_focus(fixture, id);
        let direction = if second {
            halley_ipc::ControlDirection::Right
        } else {
            halley_ipc::ControlDirection::Left
        };
        fixture.ack(Request::Control(
            halley_ipc::ControlRequest::WindowTransfer(direction),
        ));
    }
    wait_for("window transferred to intended output", || {
        fixture
            .nodes()
            .into_iter()
            .find(|n| n.id == id && n.output.as_deref() == Some(target))
            .map(|_| ())
    });
}

fn scenario(x11_source: bool, game: bool, owner: &str, second: bool) {
    eprintln!("two-output fullscreen: x11={x11_source} game={game} owner={owner} second={second}");
    let extra = format!(
        "animations:\n enabled false\nend\ninput:\n focus-mode \"click\"\nend\nrules:\n window-rule:\n app-id \"halley.test.fullscreen\"\n game {game}\n end\nend\n"
    );
    let mut fixture = Fixture::with_two_outputs(&extra);
    wait_for("two advertised outputs", || {
        match fixture.request(Request::Outputs) {
            Response::Outputs(outputs) => (outputs.outputs.len() == 2).then_some(()),
            _ => None,
        }
    });
    let input = Input::new(&fixture);
    let mut native = None;
    let x11 = if x11_source {
        let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
        let window = x11_window(&connection, "Multi-output fullscreen source");
        let class = if game {
            "steam_app_440"
        } else {
            "halley.test.x11.app"
        };
        connection
            .change_property8(
                x11rb::protocol::xproto::PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                format!("{class}\0{class}\0").as_bytes(),
            )
            .unwrap();
        connection.flush().unwrap();
        Some((connection, window))
    } else {
        let (queue, mut state) = native_window_named(
            &fixture,
            "Multi-output fullscreen source",
            "halley.test.fullscreen",
            [0, u32::MAX, 0, u32::MAX],
        );
        state.resize_for_configures = true;
        native = Some((queue, state));
        None
    };
    let source = fixture.node("Multi-output fullscreen source");
    place(&mut fixture, source.id, second);
    let (_remote_queue, _remote_state) = native_window_named(
        &fixture,
        "Other-monitor destination",
        "halley.test.remote",
        [0, 0, u32::MAX, u32::MAX],
    );
    let remote = fixture.node("Other-monitor destination");
    place(&mut fixture, remote.id, !second);
    let (_local_queue, _local_state) = native_window_named(
        &fixture,
        "Same-monitor control",
        "halley.test.local",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let local = fixture.node("Same-monitor control");
    place(&mut fixture, local.id, second);
    request_native_focus(&mut fixture, source.id);
    let source = fixture.node("Multi-output fullscreen source");
    let remote = fixture.node("Other-monitor destination");
    assert_ne!(source.output, remote.output);
    assert_eq!(source.output, fixture.node("Same-monitor control").output);
    let output_name = source.output.as_ref().unwrap().clone();
    let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
        panic!()
    };
    let output = outputs
        .outputs
        .iter()
        .find(|o| o.name == output_name)
        .unwrap();
    let mode = output.modes[output.current_mode.unwrap()];
    let full_size = (mode.width as i32, mode.height as i32);
    if let Some((connection, window)) = &x11 {
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
                    *window,
                    atom(b"_NET_WM_STATE"),
                    [1, atom(b"_NET_WM_STATE_FULLSCREEN"), 0, 1, 0],
                ),
            )
            .unwrap();
        connection.flush().unwrap();
    } else {
        let (queue, state) = native.as_mut().unwrap();
        if owner != "client" {
            let registry = state.registry.as_ref().unwrap();
            let _manager: foreign_manager::ZwlrForeignToplevelManagerV1 = registry.bind(
                state.globals["zwlr_foreign_toplevel_manager_v1"],
                3,
                &queue.handle(),
                (),
            );
            let handle = wait_for("foreign source handle", || {
                queue.roundtrip(state).unwrap();
                state
                    .foreign_titles
                    .get("Multi-output fullscreen source")
                    .cloned()
            });
            handle.set_fullscreen(None);
            queue.roundtrip(state).unwrap();
        }
        if owner != "compositor" {
            state.toplevel.as_ref().unwrap().set_fullscreen(None);
        }
    }
    let protocol_fullscreen = owner != "compositor";
    wait_for("source fullscreen configure", || {
        protocol_matches(&mut native, &x11, protocol_fullscreen, full_size).then_some(())
    });
    wait_for("fullscreen buffer committed and presented", || {
        let ready = protocol_matches(&mut native, &x11, protocol_fullscreen, full_size);
        let snapshot = input.state();
        let window = snapshot["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == source.id)
            .unwrap();
        (ready && window["rect"] == serde_json::json!([0, 0, full_size.0, full_size.1]))
            .then_some(())
    });
    let verify = |input: &Input| {
        let snapshot = input.state();
        let window = snapshot["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == source.id)
            .unwrap();
        assert_eq!(
            window["fullscreen"], true,
            "departing fullscreen was exited: {snapshot}"
        );
        assert_eq!(
            window["paused"], false,
            "departing fullscreen was parked: {snapshot}"
        );
        assert_eq!(
            window["camera"], true,
            "departing fullscreen camera was released: {snapshot}"
        );
        assert_eq!(
            window["rect"],
            serde_json::json!([0, 0, full_size.0, full_size.1]),
            "wrong fullscreen bounds: {snapshot}"
        );
    };
    verify(&input);
    assert_fullscreen_pixels(&mut fixture, &output_name, x11_source);
    // Preview and Escape must leave both focus and fullscreen untouched.
    input.preview(remote.id);
    assert_eq!(input.state()["focused"], source.id);
    verify(&input);
    assert!(protocol_matches(
        &mut native,
        &x11,
        protocol_fullscreen,
        full_size
    ));
    input.cancel();
    assert_eq!(input.state()["focused"], source.id);
    verify(&input);
    assert_fullscreen_pixels(&mut fixture, &output_name, x11_source);
    // Cross-monitor commit and the real Alt+Tab return preserve presentation.
    input.preview(remote.id);
    input.commit(remote.id);
    verify(&input);
    assert!(protocol_matches(
        &mut native,
        &x11,
        protocol_fullscreen,
        full_size
    ));
    assert_fullscreen_pixels(&mut fixture, &output_name, x11_source);
    input.preview(source.id);
    input.commit(source.id);
    verify(&input);
    assert!(protocol_matches(
        &mut native,
        &x11,
        protocol_fullscreen,
        full_size
    ));
    assert_fullscreen_pixels(&mut fixture, &output_name, x11_source);
    // Same-monitor control prevents preserving everything from passing the test.
    input.preview(local.id);
    input.commit(local.id);
    let expected_size = if game {
        full_size
    } else {
        (source.width as i32, source.height as i32)
    };
    wait_for("same-monitor protocol state and committed resize", || {
        if !protocol_matches(&mut native, &x11, game, expected_size) {
            return None;
        }
        let snapshot = input.state();
        let window = snapshot["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == source.id)
            .unwrap();
        (window["fullscreen"] == game && window["paused"] == game && window["camera"] == false)
            .then_some(())
    });
}

fn protocol_matches(
    native: &mut Option<(EventQueue<NativeState>, NativeState)>,
    x11: &Option<(RustConnection, u32)>,
    fullscreen: bool,
    size: (i32, i32),
) -> bool {
    if let Some((queue, state)) = native {
        queue.roundtrip(state).unwrap();
        state.configured_fullscreen == fullscreen && state.configured_size == size
    } else {
        let (connection, window) = x11.as_ref().unwrap();
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
                *window,
                atom(b"_NET_WM_STATE"),
                AtomEnum::ATOM,
                0,
                32,
            )
            .unwrap()
            .reply()
            .unwrap();
        let flag = property.value32().is_some_and(|mut values| {
            values.any(|value| value == atom(b"_NET_WM_STATE_FULLSCREEN"))
        });
        let geometry = connection.get_geometry(*window).unwrap().reply().unwrap();
        flag == fullscreen && (i32::from(geometry.width), i32::from(geometry.height)) == size
    }
}

fn assert_fullscreen_pixels(fixture: &mut Fixture, output: &str, white: bool) {
    let (pixels, width) = capture_frame_pixels_on(fixture, Some(output));
    let height = pixels.len() / 4 / width;
    for (x, y) in [
        (16, 16),
        (width - 17, 16),
        (16, height - 17),
        (width - 17, height - 17),
        (width / 2, height / 2),
    ] {
        let pixel = &pixels[(y * width + x) * 4..][..3];
        assert!(
            if white {
                pixel.iter().all(|v| *v > 240)
            } else {
                pixel[1] > 240 && pixel[0] < 15 && pixel[2] < 15
            },
            "fullscreen no longer fills {output} at {x},{y}: {pixel:?}"
        );
    }
}
