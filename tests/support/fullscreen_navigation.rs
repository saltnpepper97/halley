//! Exercise real protocol state, client sizes, and explicit navigation.
use super::*;
use wayland_protocols::wp::content_type::v1::client::{
    wp_content_type_manager_v1 as content_manager, wp_content_type_v1 as content,
};
delegate_noop!(NativeState: ignore content_manager::WpContentTypeManagerV1);
delegate_noop!(NativeState: ignore content::WpContentTypeV1);

struct RealTerminal(Child);

impl Drop for RealTerminal {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn terminal(fixture: &Fixture, title: &str, background: &str) -> RealTerminal {
    let socket = fs::read_dir(&fixture.path)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("wayland-")
                && path.extension().is_none()
        })
        .unwrap();
    RealTerminal(Command::new("kitty").args([
        "--config", "NONE", "--title", title,
        "--override", "linux_display_server=wayland",
        "--override", "initial_window_width=480",
        "--override", "initial_window_height=300",
        "--override", "remember_window_size=no",
        "--override", "cursor_blink_interval=0",
        "--override", &format!("background={background}"),
        "/bin/sh", "-c", "printf 'Real terminal fullscreen navigation\\n\\nWatch the window edges return to the Field.\\n'; read -r answer",
    ]).env("WAYLAND_DISPLAY", socket).env("XDG_RUNTIME_DIR", &fixture.path)
        .env_remove("DISPLAY").env_remove("WAYLAND_SOCKET")
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap())
}

fn terminal_bounds(pixels: &[u8], width: usize) -> [i32; 4] {
    let mut bounds = [i32::MAX, i32::MAX, 0, 0];
    let mut count = 0;
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        // Kitty's actual background, excluding its text and the other window.
        if pixel[0].abs_diff(105) < 8 && pixel[1].abs_diff(55) < 8 && pixel[2].abs_diff(156) < 8 {
            let x = (index % width) as i32;
            let y = (index / width) as i32;
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
            count += 1;
        }
    }
    assert!(
        count > 1000,
        "real terminal background missing: {count} pixels"
    );
    bounds
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and Kitty; measures real rendered fullscreen return motion"]
fn nearby_navigation_returns_real_terminal_without_a_cleanup_jump() {
    let mut fixture = Fixture::with_config(
        "stacking",
        r#"
animations:
  window-open:
    enabled false
  end
  fullscreen:
    motion "easing"
    duration-ms 1800
    curve "linear"
  end
end
"#,
    );
    let _source = terminal(&fixture, "Fullscreen motion source", "#9c3769");
    let source = fixture.node("Fullscreen motion source");
    let _destination = terminal(&fixture, "Fullscreen motion destination", "#203040");
    let destination = fixture.node("Fullscreen motion destination");
    fixture.ack(Request::Node(NodeRequest::MoveBy {
        direction: halley_ipc::NodeMoveDirection::Right,
        distance: 260,
        selector: Some(NodeSelector::Id(destination.id)),
        output: None,
    }));
    request_native_focus(&mut fixture, source.id);
    let (mut queue, mut state) = native_connection(&fixture);
    let _manager: foreign_manager::ZwlrForeignToplevelManagerV1 =
        state.registry.as_ref().unwrap().bind(
            state.globals["zwlr_foreign_toplevel_manager_v1"],
            3,
            &queue.handle(),
            (),
        );
    let handle = wait_for("real terminal foreign toplevel", || {
        queue.roundtrip(&mut state).unwrap();
        state
            .foreign_titles
            .get("Fullscreen motion source")
            .cloned()
    });
    thread::sleep(Duration::from_millis(600));
    handle.set_fullscreen(None);
    queue.roundtrip(&mut state).unwrap();
    thread::sleep(Duration::from_millis(2000));
    let (pixels, width) = capture_frame_pixels(&mut fixture);
    let bounds = terminal_bounds(&pixels, width);
    assert!(
        bounds[2] - bounds[0] > 1000,
        "source must really be fullscreen: {bounds:?}"
    );
    request_native_focus(&mut fixture, destination.id);
    let start = Instant::now();
    let mut samples = Vec::new();
    while start.elapsed() < Duration::from_millis(2200) {
        let (mut pixels, width) = capture_frame_pixels(&mut fixture);
        let bounds = terminal_bounds(&pixels, width);
        samples.push((start.elapsed().as_millis(), bounds));
        if let Ok(path) = std::env::var("HALLEY_NAVIGATION_ARTIFACTS") {
            let path = PathBuf::from(path);
            fs::create_dir_all(&path).unwrap();
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            image::save_buffer(
                path.join(format!("motion-{:04}.png", samples.len())),
                &pixels,
                width as u32,
                (pixels.len() / 4 / width) as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        thread::sleep(Duration::from_millis(30));
    }
    eprintln!("real terminal navigation bounds: {samples:?}");
    let worst = samples
        .windows(2)
        .map(|pair| {
            let gap = pair[1].0 - pair[0].0;
            let jump = pair[0]
                .1
                .into_iter()
                .zip(pair[1].1)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap();
            (jump, gap)
        })
        .max()
        .unwrap();
    assert!(
        worst.0 <= 65 + worst.1 as u32 / 2,
        "fullscreen return jumped between consecutive frames: {worst:?}; samples: {samples:?}"
    );
    let last = samples.last().unwrap().1;
    assert!((last[2] - last[0] - source.width as i32).abs() <= 3);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; tests fullscreen navigation in isolated sessions"]
fn native_fullscreen_navigation_restores_apps_and_preserves_games() {
    // client, compositor, and overlapping ownership; both positive hints and
    // explicit overrides. A Wayland game must behave like an X11 game.
    for (app_id, rule, owner, hint, game) in [
        ("halley.navigation.ordinary", "", "client", false, false),
        ("halley.navigation.ordinary", "", "compositor", false, false),
        ("halley.navigation.ordinary", "", "both", false, false),
        ("halley.navigation.game", "game true", "client", false, true),
        ("steam_app_440", "", "client", false, true),
        ("steam_app_440", "game false", "client", false, false),
        ("halley.navigation.hint", "", "client", true, true),
        (
            "halley.navigation.hint",
            "game false",
            "client",
            true,
            false,
        ),
    ] {
        let extra = format!(
            "animations:\n  enabled false\nend\nrules:\n  window-rule:\n    app-id \"{app_id}\"\n    {rule}\n  end\nend\n"
        );
        let mut fixture = Fixture::with_config("stacking", &extra);
        let (mut queue, mut state) = native_window_named(
            &fixture,
            "navigation source",
            app_id,
            [0, u32::MAX, 0, u32::MAX],
        );
        state.resize_for_configures = true;
        let source = fixture.node("navigation source");
        let (mut other_queue, mut other) = native_window_named(
            &fixture,
            "navigation destination",
            "halley.navigation.destination",
            [0, 0, u32::MAX, u32::MAX],
        );
        let destination = fixture.node("navigation destination");
        request_native_focus(&mut fixture, source.id);
        queue.roundtrip(&mut state).unwrap();
        let content_type = hint.then(|| {
            let registry = state.registry.as_ref().unwrap();
            let manager: content_manager::WpContentTypeManagerV1 = registry.bind(
                state.globals["wp_content_type_manager_v1"],
                1,
                &queue.handle(),
                (),
            );
            let content = manager.get_surface_content_type(
                state.surface.as_ref().unwrap(),
                &queue.handle(),
                (),
            );
            content.set_content_type(content::Type::Game);
            state.surface.as_ref().unwrap().commit();
            content
        });
        let _manager: foreign_manager::ZwlrForeignToplevelManagerV1 =
            state.registry.as_ref().unwrap().bind(
                state.globals["zwlr_foreign_toplevel_manager_v1"],
                3,
                &queue.handle(),
                (),
            );
        let handle = wait_for("source foreign toplevel", || {
            queue.roundtrip(&mut state).unwrap();
            state.foreign_titles.get("navigation source").cloned()
        });
        if owner != "client" {
            handle.set_fullscreen(None);
            queue.roundtrip(&mut state).unwrap();
        }
        if owner != "compositor" {
            state.toplevel.as_ref().unwrap().set_fullscreen(None);
        }
        wait_for("source fullscreen configure", || {
            queue.roundtrip(&mut state).unwrap();
            (state.configured_size.0 > source.width as i32
                && (owner == "compositor" || state.configured_fullscreen))
                .then_some(())
        });
        let fullscreen_size = state.configured_size;
        // Explicit navigation uses the same shared path as committed Alt+Tab.
        request_native_focus(&mut fixture, destination.id);
        other_queue.roundtrip(&mut other).unwrap();
        queue.roundtrip(&mut state).unwrap();
        if game {
            assert!(
                state.configured_fullscreen,
                "{app_id} should retain protocol fullscreen"
            );
            assert_eq!(state.configured_size, fullscreen_size);
        } else {
            wait_for("ordinary source restored", || {
                queue.roundtrip(&mut state).unwrap();
                (!state.configured_fullscreen
                    && state.configured_size == (source.width as i32, source.height as i32))
                    .then_some(())
            });
        }
        request_native_focus(&mut fixture, source.id);
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(
            state.configured_fullscreen, game,
            "return must not re-fullscreen an ordinary app"
        );
        // Returning by navigation leaves games parked, with an unlocked Field.
        fixture.ack(Request::Control(halley_ipc::ControlRequest::PanField(
            halley_ipc::ControlDirection::Right,
        )));
        // A new fullscreen cycle must reclaim output ownership after exit.
        if !game {
            state.toplevel.as_ref().unwrap().set_fullscreen(None);
            wait_for("repeat fullscreen", || {
                queue.roundtrip(&mut state).unwrap();
                state.configured_fullscreen.then_some(())
            });
            assert_eq!(state.configured_size, fullscreen_size);
        }
        drop(content_type);
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; checks X11 fullscreen state and resize behavior"]
fn x11_fullscreen_navigation_distinguishes_ordinary_apps_from_games() {
    use x11rb::protocol::xproto::{ClientMessageEvent, EventMask};
    for (class, rule, game) in [
        ("ordinary-app", "", false),
        ("steam_app_440", "", true),
        ("steam_app_440", "game false", false),
        ("ordinary-app", "game true", true),
    ] {
        let extra = format!(
            "animations:\n  enabled false\nend\nrules:\n  window-rule:\n    app-id \"{class}\"\n    {rule}\n  end\nend\n"
        );
        let mut fixture = Fixture::with_config("stacking", &extra);
        let (x11, _) = x11rb::connect(Some(&fixture.display)).unwrap();
        let window = x11_window(&x11, "X11 navigation source");
        x11.change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            format!("{class}\0{class}\0").as_bytes(),
        )
        .unwrap();
        x11.flush().unwrap();
        let source = fixture.node("X11 navigation source");
        let (mut queue, mut state) = native_window_named(
            &fixture,
            "X11 navigation destination",
            "halley.navigation.destination",
            [0, 0, u32::MAX, u32::MAX],
        );
        let destination = fixture.node("X11 navigation destination");
        request_native_focus(&mut fixture, source.id);
        let atom = |name: &[u8]| x11.intern_atom(false, name).unwrap().reply().unwrap().atom;
        let net_state = atom(b"_NET_WM_STATE");
        let fullscreen = atom(b"_NET_WM_STATE_FULLSCREEN");
        let root = x11.setup().roots[0].root;
        x11.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, window, net_state, [1, fullscreen, 0, 1, 0]),
        )
        .unwrap();
        x11.flush().unwrap();
        wait_for("X11 fullscreen geometry", || {
            let geometry = x11.get_geometry(window).unwrap().reply().unwrap();
            (geometry.width > source.width as u16).then_some(())
        });
        let fullscreen_size = x11.get_geometry(window).unwrap().reply().unwrap();
        request_native_focus(&mut fixture, destination.id);
        queue.roundtrip(&mut state).unwrap();
        wait_for("X11 navigation policy", || {
            let property = x11
                .get_property(false, window, net_state, AtomEnum::ATOM, 0, 32)
                .unwrap()
                .reply()
                .unwrap();
            let has_fullscreen = property
                .value32()
                .is_some_and(|mut values| values.any(|atom| atom == fullscreen));
            let geometry = x11.get_geometry(window).unwrap().reply().unwrap();
            let size = if game {
                (fullscreen_size.width, fullscreen_size.height)
            } else {
                (source.width as u16, source.height as u16)
            };
            (has_fullscreen == game && (geometry.width, geometry.height) == size).then_some(())
        });
        request_native_focus(&mut fixture, source.id);
        fixture.ack(Request::Control(halley_ipc::ControlRequest::PanField(
            halley_ipc::ControlDirection::Right,
        )));
    }
}
