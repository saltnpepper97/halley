use crate::keybinds::Modifiers;

/// Parse a chord string like `"super+shift+e"` into its modifier flags and
/// key name. The last `+`-separated segment is always the key; every
/// segment before it must be a recognized modifier name
/// (`super`, `alt`, `ctrl`, `shift`, or their `l*`/`r*` variants - matched
/// case-insensitively). Returns `None` if the key segment is empty or any
/// modifier segment isn't recognized - a chord with a typo'd modifier
/// should fail to parse, not silently drop the typo and bind something the
/// user didn't intend.
///
/// A chord with no `+` at all (e.g. `"print"`) is valid - a bare key with
/// no modifiers.
pub fn parse_chord(chord: &str) -> Option<(Modifiers, String)> {
    let mut parts: Vec<&str> = chord.split('+').collect();
    let key = parts.pop()?.trim();
    if key.is_empty() {
        return None;
    }

    let mut modifiers = Modifiers::default();
    for part in parts {
        match part.trim().to_lowercase().as_str() {
            "super" | "logo" | "mod4" => modifiers.super_key = true,
            "lsuper" | "left-super" | "lwin" | "left-logo" => modifiers.left_super = true,
            "rsuper" | "right-super" | "rwin" | "right-logo" => modifiers.right_super = true,
            "alt" => modifiers.alt = true,
            "lalt" | "left-alt" => modifiers.left_alt = true,
            "ralt" | "right-alt" => modifiers.right_alt = true,
            "ctrl" | "control" => modifiers.ctrl = true,
            "lctrl" | "left-ctrl" | "left-control" => modifiers.left_ctrl = true,
            "rctrl" | "right-ctrl" | "right-control" => modifiers.right_ctrl = true,
            "shift" => modifiers.shift = true,
            "lshift" | "left-shift" => modifiers.left_shift = true,
            "rshift" | "right-shift" => modifiers.right_shift = true,
            _ => return None,
        }
    }

    Some((modifiers, key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifier_combinations_aliases_and_key() {
        for (input, expected, key) in [
            (
                "super+shift+e",
                Modifiers {
                    super_key: true,
                    shift: true,
                    ..Modifiers::default()
                },
                "e",
            ),
            (
                "super+c",
                Modifiers {
                    super_key: true,
                    ..Modifiers::default()
                },
                "c",
            ),
            ("print", Modifiers::default(), "print"),
            (
                "alt+ctrl+q",
                Modifiers {
                    alt: true,
                    ctrl: true,
                    ..Modifiers::default()
                },
                "q",
            ),
            (
                "logo+q",
                Modifiers {
                    super_key: true,
                    ..Modifiers::default()
                },
                "q",
            ),
            (
                "mod4+q",
                Modifiers {
                    super_key: true,
                    ..Modifiers::default()
                },
                "q",
            ),
            (
                "Super+SHIFT+e",
                Modifiers {
                    super_key: true,
                    shift: true,
                    ..Modifiers::default()
                },
                "e",
            ),
            (
                "lsuper+rctrl+lshift+x",
                Modifiers {
                    left_super: true,
                    right_ctrl: true,
                    left_shift: true,
                    ..Modifiers::default()
                },
                "x",
            ),
        ] {
            assert_eq!(
                parse_chord(input),
                Some((expected, key.to_string())),
                "{input}"
            );
        }
    }
}
