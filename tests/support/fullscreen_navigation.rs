//! Exercise real protocol state, client sizes, and explicit navigation.
use super::*;
use wayland_protocols::wp::content_type::v1::client::{
    wp_content_type_manager_v1 as content_manager, wp_content_type_v1 as content,
};
delegate_noop!(NativeState: ignore content_manager::WpContentTypeManagerV1);
delegate_noop!(NativeState: ignore content::WpContentTypeV1);

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
