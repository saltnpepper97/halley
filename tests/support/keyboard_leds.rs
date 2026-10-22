//! Raw key events must reach the backend LED callback, including without focus.
use super::fullscreen_multi_output::Input;
use super::*;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1 as virtual_manager, zwp_virtual_keyboard_v1 as virtual_keyboard,
};

delegate_noop!(NativeState: ignore virtual_manager::ZwpVirtualKeyboardManagerV1);
delegate_noop!(NativeState: ignore virtual_keyboard::ZwpVirtualKeyboardV1);

fn tap(input: &Input, code: u32) -> serde_json::Value {
    input.key(code, true);
    input.key(code, false)
}

fn indicators(snapshot: &serde_json::Value, caps: bool, num: bool) {
    assert_eq!(
        snapshot["keyboard"]["locks"],
        serde_json::json!([caps, num]),
        "{snapshot}"
    );
    assert_eq!(snapshot["leds"]["caps"], caps, "{snapshot}");
    assert_eq!(snapshot["leds"]["num"], num, "{snapshot}");
    assert_eq!(
        snapshot["last_led_update"], snapshot["leds"],
        "backend missed the current LED state: {snapshot}"
    );
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; raw keyboard LED callbacks"]
fn lock_indicators_reach_the_backend_and_survive_ordinary_keys_and_keymap_reload() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let pressed = input.key(58, true); // Caps Lock
    indicators(&pressed, true, false);
    let duplicate = input.key(58, true); // A repeated down must not toggle twice.
    indicators(&duplicate, true, false);
    assert_eq!(pressed["led_update_count"], duplicate["led_update_count"]);
    input.key(58, false);
    let count = input.state()["led_update_count"].clone();
    for code in [30, 48, 30] {
        // A, B, A do not clear lock state.
        let snapshot = tap(&input, code);
        indicators(&snapshot, true, false);
        assert_eq!(snapshot["led_update_count"], count);
    }
    indicators(&tap(&input, 69), true, true); // Num Lock
    indicators(&tap(&input, 79), true, true); // Keypad 1
    let before = input.state()["led_update_count"].as_u64().unwrap();
    fs::write(fixture.path.join("halley.rune"), "keybinds:\nend\nanimations:\n enabled false\nend\ninput:\n keyboard:\n layout \"gb\"\n end\nend\n").unwrap();
    fixture.ack(Request::ConfigReload);
    wait_for(
        "keymap reload restores both lock LEDs at the backend without focus",
        || {
            let snapshot = input.state();
            let changed = snapshot["led_update_count"].as_u64().unwrap() > before;
            if changed {
                indicators(&snapshot, true, true);
            }
            changed.then_some(())
        },
    );
    indicators(&tap(&input, 30), true, true);
    indicators(&tap(&input, 58), false, true);
    indicators(&tap(&input, 69), false, false);
}

fn reload_keyboard(fixture: &mut Fixture, input: &Input, settings: &str) {
    let before = input.state()["led_update_count"].as_u64().unwrap();
    fs::write(fixture.path.join("halley.rune"), format!(
        "keybinds:\nend\nanimations:\n enabled false\nend\ninput:\n keyboard:\n {settings}\n end\nend\n"
    )).unwrap();
    fixture.ack(Request::ConfigReload);
    wait_for("keyboard reload and backend LED synchronization", || {
        (input.state()["led_update_count"].as_u64().unwrap() > before).then_some(())
    });
}

fn set_locks(input: &Input, caps: bool, num: bool) {
    for (name, code, expected) in [("caps", 58, caps), ("num", 69, num)] {
        if input.state()["leds"][name] != expected {
            tap(input, code);
        }
    }
    indicators(&input.state(), caps, num);
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; focused lock state across keymaps"]
fn focused_wayland_lock_state_survives_layout_variant_options_and_failed_reload() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (mut queue, mut client) = native_window(&fixture);
    let _keyboard = observe_keyboard(&mut queue, &mut client);
    wait_for("Wayland keyboard focus", || {
        queue.roundtrip(&mut client).unwrap();
        client.keyboard_focused.then_some(())
    });
    for (caps, num, settings) in [
        (true, false, "layout \"gb\""),
        (false, true, "layout \"us\"\n options \"compose:ralt\""),
        (
            true,
            true,
            "layout \"us\"\n variant \"intl\"\n options \"compose:ralt\"",
        ),
        (false, false, "layout \"gb\""),
    ] {
        set_locks(&input, caps, num);
        input.key(42, true); // Held Shift must survive the keymap replacement too.
        queue.roundtrip(&mut client).unwrap();
        let modifiers = client.keyboard_modifiers;
        let keymaps = client.keyboard_keymaps;
        reload_keyboard(&mut fixture, &input, settings);
        indicators(&input.state(), caps, num);
        wait_for("new keymap and restored Wayland modifiers", || {
            queue.roundtrip(&mut client).unwrap();
            (client.keyboard_keymaps > keymaps && client.keyboard_modifiers == modifiers)
                .then_some(())
        });
        let held = tap(&input, 30);
        indicators(&held, caps, num);
        assert_eq!(held["keyboard"]["modifiers"][0], true);
        input.key(42, false);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.keyboard_modifiers.0, 0);
        assert_eq!(client.keyboard_modifiers.2, modifiers.2);
    }
    set_locks(&input, true, true);
    queue.roundtrip(&mut client).unwrap();
    let modifiers = client.keyboard_modifiers;
    let keymaps = client.keyboard_keymaps;
    reload_keyboard(
        &mut fixture,
        &input,
        "layout \"halley-invalid-test-layout\"",
    );
    indicators(&input.state(), true, true);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.keyboard_keymaps, keymaps);
    assert_eq!(client.keyboard_modifiers, modifiers);
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; X11 locks across keymaps"]
fn x11_lock_state_survives_keyboard_reload() {
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, EventMask, KeyButMask};
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (connection, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let window = x11_window(&connection, "lock reload X11 target");
    connection
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    let id = fixture.node("lock reload X11 target").id;
    request_native_focus(&mut fixture, id);
    wait_for("X11 keyboard focus", || {
        (connection.get_input_focus().unwrap().reply().unwrap().focus == window).then_some(())
    });
    let letter_locks = || {
        tap(&input, 30);
        wait_for("X11 letter and lock flags", || {
            while let Some(event) = connection.poll_for_event().unwrap() {
                if let Event::KeyPress(event) = event {
                    if event.detail == 38 {
                        return Some(
                            u16::from(event.state & (KeyButMask::LOCK | KeyButMask::MOD2)) as u64,
                        );
                    }
                }
            }
            None
        })
    };
    for (caps, num, settings) in [
        (true, false, "layout \"gb\""),
        (false, true, "layout \"us\"\n options \"compose:ralt\""),
        (true, true, "layout \"us\"\n variant \"intl\""),
        (false, false, "layout \"gb\""),
    ] {
        set_locks(&input, caps, num);
        let locked = input.state()["keyboard"]["locked"].as_u64().unwrap();
        assert_eq!(letter_locks(), locked, "X11 locks before reload");
        reload_keyboard(&mut fixture, &input, settings);
        indicators(&input.state(), caps, num);
        assert_eq!(letter_locks(), locked, "X11 locks after reload");
        indicators(&input.state(), caps, num);
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; keyboard event order"]
fn wayland_modifier_changes_follow_the_key_that_caused_them() {
    let fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (mut queue, mut client) = native_window(&fixture);
    let _keyboard = observe_keyboard(&mut queue, &mut client);
    for code in [42, 29, 56, 125, 58, 69] {
        for pressed in [true, false] {
            client.keyboard_delivery_events.clear();
            input.key(code, pressed);
            queue.roundtrip(&mut client).unwrap();
            assert_eq!(
                client.keyboard_delivery_events.first(),
                Some(&"key"),
                "key {code}, pressed={pressed}"
            );
            assert_eq!(
                client
                    .keyboard_delivery_events
                    .iter()
                    .filter(|&&event| event == "key")
                    .count(),
                1
            );
            if client.keyboard_delivery_events.contains(&"modifiers") {
                assert_eq!(client.keyboard_delivery_events, ["key", "modifiers"]);
            }
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; standalone virtual modifiers"]
fn standalone_virtual_modifiers_remain_immediate_before_following_keys() {
    use smithay::input::keyboard::xkb;
    use std::io::Write;
    use std::os::fd::AsFd;
    let fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let (mut queue, mut client) = native_window(&fixture);
    let _keyboard = observe_keyboard(&mut queue, &mut client);
    let handle = queue.handle();
    let registry = client.registry.as_ref().unwrap();
    let seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 1, &handle, ());
    let manager: virtual_manager::ZwpVirtualKeyboardManagerV1 = registry.bind(
        client.globals["zwp_virtual_keyboard_manager_v1"],
        1,
        &handle,
        (),
    );
    let keyboard = manager.create_virtual_keyboard(&seat, &handle, ());
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "",
        "pc105",
        "us",
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .unwrap();
    let mut keymap_text = keymap
        .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
        .into_bytes();
    keymap_text.push(0);
    let mut keymap_file = File::create(fixture.path.join("virtual-keymap.xkb")).unwrap();
    keymap_file.write_all(&keymap_text).unwrap();
    let keymap_file = File::open(fixture.path.join("virtual-keymap.xkb")).unwrap();
    keyboard.keymap(1, keymap_file.as_fd(), keymap_text.len() as u32);
    queue.roundtrip(&mut client).unwrap();
    for locked in [1 << keymap.mod_get_index(xkb::MOD_NAME_CAPS), 0] {
        client.keyboard_delivery_events.clear();
        keyboard.modifiers(0, 0, locked, 0);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.keyboard_modifiers.2, locked);
        assert_eq!(client.keyboard_delivery_events, ["modifiers"]);
        client.keyboard_delivery_events.clear();
        keyboard.key(1, 30, 1);
        keyboard.key(2, 30, 0);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.keyboard_delivery_events, ["key", "key"]);
        assert_eq!(client.keyboard_modifiers.2, locked);
    }
    keyboard.destroy();
    queue.roundtrip(&mut client).unwrap();
}
