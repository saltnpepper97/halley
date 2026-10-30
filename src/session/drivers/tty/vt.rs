use smithay::input::keyboard::ModifiersState;

/// Resolve the standard Ctrl+Alt+Fn rescue chord without extra held modifiers.
///
/// Smithay's libinput backend reports XKB keycodes, which are Linux evdev
/// keycodes plus eight. Handling this below ordinary compositor key routing
/// keeps VT switching available through modal UIs, session lock, and client
/// shortcut inhibition. Extra modifiers leave the event for normal routing;
/// Caps Lock and Num Lock do not disable the rescue chord.
pub(super) fn target_from_keycode(
    xkb_keycode: u32,
    pressed: bool,
    modifiers: ModifiersState,
) -> Option<i32> {
    if !pressed
        || !modifiers.ctrl
        || !modifiers.alt
        || modifiers.shift
        || modifiers.logo
        || modifiers.iso_level3_shift
        || modifiers.iso_level5_shift
    {
        return None;
    }
    match xkb_keycode {
        // KEY_F1..KEY_F10 are evdev 59..68; F11/F12 are 87/88.
        67..=76 => Some((xkb_keycode - 66) as i32),
        95 => Some(11),
        96 => Some(12),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl_alt() -> ModifiersState {
        ModifiersState {
            ctrl: true,
            alt: true,
            ..ModifiersState::default()
        }
    }

    #[test]
    fn ctrl_alt_function_keys_map_to_all_twelve_vts() {
        let expected = [
            (67, 1),
            (68, 2),
            (69, 3),
            (70, 4),
            (71, 5),
            (72, 6),
            (73, 7),
            (74, 8),
            (75, 9),
            (76, 10),
            (95, 11),
            (96, 12),
        ];

        for (keycode, vt) in expected {
            assert_eq!(target_from_keycode(keycode, true, ctrl_alt()), Some(vt));
        }
    }

    #[test]
    fn extra_modifiers_leave_function_keys_for_normal_routing() {
        for mask in 1..16 {
            let modifiers = ModifiersState {
                shift: mask & 1 != 0,
                logo: mask & 2 != 0,
                iso_level3_shift: mask & 4 != 0,
                iso_level5_shift: mask & 8 != 0,
                ..ctrl_alt()
            };
            for keycode in [67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 95, 96] {
                assert_eq!(
                    target_from_keycode(keycode, true, modifiers),
                    None,
                    "extra modifier mask {mask} must not switch VT for keycode {keycode}",
                );
            }
        }
    }

    #[test]
    fn lock_modifiers_do_not_disable_rescue_chords() {
        for mask in 0..4 {
            let modifiers = ModifiersState {
                caps_lock: mask & 1 != 0,
                num_lock: mask & 2 != 0,
                ..ctrl_alt()
            };
            for (keycode, vt) in [67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 95, 96]
                .into_iter()
                .zip(1..=12)
            {
                assert_eq!(target_from_keycode(keycode, true, modifiers), Some(vt));
            }
        }
    }

    #[test]
    fn release_and_incomplete_chords_do_not_switch() {
        assert_eq!(target_from_keycode(67, false, ctrl_alt()), None);
        assert_eq!(
            target_from_keycode(
                67,
                true,
                ModifiersState {
                    ctrl: true,
                    ..ModifiersState::default()
                },
            ),
            None
        );
        assert_eq!(
            target_from_keycode(
                67,
                true,
                ModifiersState {
                    alt: true,
                    ..ModifiersState::default()
                },
            ),
            None
        );
    }

    #[test]
    fn ctrl_alt_non_function_key_is_ignored() {
        assert_eq!(target_from_keycode(38, true, ctrl_alt()), None);
    }
}
