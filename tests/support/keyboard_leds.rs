//! Raw key events must reach the backend LED callback, including without focus.
use super::fullscreen_multi_output::Input;
use super::*;

fn tap(input: &Input, code: u32) -> serde_json::Value {
    input.key(code, true);
    input.key(code, false)
}

fn indicators(snapshot: &serde_json::Value, caps: bool, num: bool) {
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
    indicators(&tap(&input, 58), false, true);
    let before = input.state()["led_update_count"].as_u64().unwrap();
    fs::write(fixture.path.join("halley.rune"), "keybinds:\nend\nanimations:\n enabled false\nend\ninput:\n keyboard:\n layout \"gb\"\n end\nend\n").unwrap();
    fixture.ack(Request::ConfigReload);
    wait_for(
        "keymap reload restores Num Lock LEDs at the backend",
        || {
            let snapshot = input.state();
            let changed = snapshot["led_update_count"].as_u64().unwrap() > before;
            if changed {
                indicators(&snapshot, false, true);
            }
            changed.then_some(())
        },
    );
    indicators(&tap(&input, 30), false, true);
    indicators(&tap(&input, 69), false, false);
}
