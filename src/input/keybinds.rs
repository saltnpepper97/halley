use halley_config::{Action, BindingScope, Keybinds, ModifierKey, Modifiers};
use smithay::backend::input::Keycode;
use smithay::input::keyboard::{Keysym, xkb};

const XKB_KEYCODE_OFFSET: u32 = 8;

/// A mouse button that can be named in a Rune keybind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButtonTrigger {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    /// An exact Linux evdev button code from `button-N`.
    Code(u32),
}

impl PointerButtonTrigger {
    pub fn matches(self, button: u32) -> bool {
        match self {
            Self::Left => button == 0x110,
            Self::Right => button == 0x111,
            Self::Middle => button == 0x112,
            // Linux exposes the same physical navigation buttons under two
            // pairs of names depending on the device/driver.
            Self::Back => matches!(button, 0x113 | 0x116),
            Self::Forward => matches!(button, 0x114 | 0x115),
            Self::Code(expected) => button == expected,
        }
    }
}

/// A physical mouse-wheel direction that can be named in a Rune keybind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelDirection {
    Up,
    Down,
    Left,
    Right,
}

/// A fully parsed input trigger. Keyboard symbols remain layout-aware while
/// raw codes provide an escape hatch for unusual hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedTrigger {
    Keysym(Keysym),
    /// XKB's internal keycode (`evdev + 8`), resolved from `keycode-N`.
    Keycode(Keycode),
    PointerButton(PointerButtonTrigger),
    Wheel(WheelDirection),
}

/// Which backend a bind table is being resolved for - drives the
/// Alt-vs-Super mod-key convention (design decision 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Winit,
    Tty,
}

/// TTY uses the configured mod key as-is. Winit uses Alt instead unless Alt
/// is configured, in which case it falls back to Super. This avoids stealing
/// the host desktop's Super key during nested development.
pub fn effective_mod(configured: ModifierKey, backend: BackendKind) -> ModifierKey {
    match backend {
        BackendKind::Winit => match configured {
            ModifierKey::Super => ModifierKey::Alt,
            ModifierKey::LeftSuper => ModifierKey::LeftAlt,
            ModifierKey::RightSuper => ModifierKey::RightAlt,
            ModifierKey::Alt => ModifierKey::Super,
            ModifierKey::LeftAlt => ModifierKey::LeftSuper,
            ModifierKey::RightAlt => ModifierKey::RightSuper,
            ModifierKey::Ctrl | ModifierKey::Shift => ModifierKey::Alt,
            ModifierKey::LeftCtrl | ModifierKey::LeftShift => ModifierKey::LeftAlt,
            ModifierKey::RightCtrl | ModifierKey::RightShift => ModifierKey::RightAlt,
        },
        BackendKind::Tty => configured,
    }
}

fn modifier_bit(modifiers: Modifiers, key: ModifierKey) -> bool {
    match key {
        ModifierKey::Super => modifiers.super_key,
        ModifierKey::LeftSuper => modifiers.left_super,
        ModifierKey::RightSuper => modifiers.right_super,
        ModifierKey::Alt => modifiers.alt,
        ModifierKey::LeftAlt => modifiers.left_alt,
        ModifierKey::RightAlt => modifiers.right_alt,
        ModifierKey::Ctrl => modifiers.ctrl,
        ModifierKey::LeftCtrl => modifiers.left_ctrl,
        ModifierKey::RightCtrl => modifiers.right_ctrl,
        ModifierKey::Shift => modifiers.shift,
        ModifierKey::LeftShift => modifiers.left_shift,
        ModifierKey::RightShift => modifiers.right_shift,
    }
}

fn set_modifier_bit(modifiers: &mut Modifiers, key: ModifierKey, value: bool) {
    match key {
        ModifierKey::Super => modifiers.super_key = value,
        ModifierKey::LeftSuper => modifiers.left_super = value,
        ModifierKey::RightSuper => modifiers.right_super = value,
        ModifierKey::Alt => modifiers.alt = value,
        ModifierKey::LeftAlt => modifiers.left_alt = value,
        ModifierKey::RightAlt => modifiers.right_alt = value,
        ModifierKey::Ctrl => modifiers.ctrl = value,
        ModifierKey::LeftCtrl => modifiers.left_ctrl = value,
        ModifierKey::RightCtrl => modifiers.right_ctrl = value,
        ModifierKey::Shift => modifiers.shift = value,
        ModifierKey::LeftShift => modifiers.left_shift = value,
        ModifierKey::RightShift => modifiers.right_shift = value,
    }
}

/// If the `from` mod-key bit is set, clear it and set the `to` bit instead -
/// every other bit (e.g. shift) is untouched. A config chord is always
/// written in terms of the configured mod key; this remaps it to whatever
/// that mod key actually resolves to for the current backend.
pub fn remap_mod_bit(modifiers: Modifiers, from: ModifierKey, to: ModifierKey) -> Modifiers {
    if from == to || !modifier_bit(modifiers, from) {
        return modifiers;
    }
    let mut remapped = modifiers;
    set_modifier_bit(&mut remapped, from, false);
    set_modifier_bit(&mut remapped, to, true);
    remapped
}

/// A keybind fully resolved for matching against live input: modifiers are
/// already remapped for this backend and the config's final chord segment is
/// classified as a keyboard, pointer-button, or wheel trigger.
#[derive(Clone, Debug)]
pub struct ResolvedBind {
    pub scope: BindingScope,
    pub modifiers: Modifiers,
    pub trigger: ResolvedTrigger,
    pub action: Action,
    pub repeat: bool,
}

fn raw_code(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse().ok()
}

pub fn resolve_trigger_name(name: &str) -> Option<ResolvedTrigger> {
    let normalized = name.trim().to_ascii_lowercase();
    let trigger = match normalized.as_str() {
        "click-left" => ResolvedTrigger::PointerButton(PointerButtonTrigger::Left),
        "click-right" => ResolvedTrigger::PointerButton(PointerButtonTrigger::Right),
        "click-middle" => ResolvedTrigger::PointerButton(PointerButtonTrigger::Middle),
        "click-back" => ResolvedTrigger::PointerButton(PointerButtonTrigger::Back),
        "click-forward" => ResolvedTrigger::PointerButton(PointerButtonTrigger::Forward),
        "scroll-up" => ResolvedTrigger::Wheel(WheelDirection::Up),
        "scroll-down" => ResolvedTrigger::Wheel(WheelDirection::Down),
        "scroll-left" => ResolvedTrigger::Wheel(WheelDirection::Left),
        "scroll-right" => ResolvedTrigger::Wheel(WheelDirection::Right),
        _ => {
            if let Some(code) = raw_code(&normalized, "button-") {
                ResolvedTrigger::PointerButton(PointerButtonTrigger::Code(code))
            } else if let Some(code) = raw_code(&normalized, "keycode-") {
                let xkb_code = code.checked_add(XKB_KEYCODE_OFFSET)?;
                ResolvedTrigger::Keycode(Keycode::new(xkb_code))
            } else {
                let keysym = xkb::keysym_from_name(name.trim(), xkb::KEYSYM_CASE_INSENSITIVE);
                if keysym == Keysym::NoSymbol {
                    return None;
                }
                ResolvedTrigger::Keysym(keysym)
            }
        }
    };
    Some(trigger)
}

pub fn resolve_binds(keybinds: &Keybinds, backend: BackendKind) -> Vec<ResolvedBind> {
    let effective = effective_mod(keybinds.modifier, backend);

    keybinds
        .binds
        .iter()
        .filter_map(|bind| {
            let trigger = match resolve_trigger_name(&bind.key) {
                Some(trigger) => trigger,
                None => {
                    eventline::warn!(
                        "keybinds: unknown trigger name {:?}, skipping bind",
                        bind.key
                    );
                    return None;
                }
            };
            Some(ResolvedBind {
                scope: bind.scope,
                modifiers: remap_mod_bit(bind.modifiers, keybinds.modifier, effective),
                trigger,
                action: bind.action.clone(),
                repeat: bind.repeat,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_modifier_matches_the_backend() {
        for (configured, backend, expected) in [
            (ModifierKey::Super, BackendKind::Winit, ModifierKey::Alt),
            (ModifierKey::Alt, BackendKind::Winit, ModifierKey::Super),
            (
                ModifierKey::LeftSuper,
                BackendKind::Winit,
                ModifierKey::LeftAlt,
            ),
            (
                ModifierKey::RightAlt,
                BackendKind::Winit,
                ModifierKey::RightSuper,
            ),
            (ModifierKey::Super, BackendKind::Tty, ModifierKey::Super),
            (ModifierKey::Ctrl, BackendKind::Tty, ModifierKey::Ctrl),
        ] {
            assert_eq!(
                effective_mod(configured, backend),
                expected,
                "{configured:?} on {backend:?}"
            );
        }
    }

    #[test]
    fn remap_moves_the_bit_and_leaves_others_alone() {
        let modifiers = Modifiers {
            shift: true,
            super_key: true,
            ..Modifiers::default()
        };
        let remapped = remap_mod_bit(modifiers, ModifierKey::Super, ModifierKey::Alt);
        assert!(remapped.alt);
        assert!(!remapped.super_key);
        assert!(remapped.shift);
    }

    #[test]
    fn remap_is_a_no_op_when_bit_not_set() {
        let modifiers = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let remapped = remap_mod_bit(modifiers, ModifierKey::Super, ModifierKey::Alt);
        assert_eq!(remapped, modifiers);
    }

    #[test]
    fn default_keybinds_resolve_on_native_and_nested_backends() {
        let keybinds = Keybinds::default();
        let nested = resolve_binds(&keybinds, BackendKind::Winit);
        let native = resolve_binds(&keybinds, BackendKind::Tty);
        assert_eq!(nested.len(), keybinds.binds.len());
        assert_eq!(native.len(), keybinds.binds.len());

        let quit = nested
            .iter()
            .find(|bind| bind.action == Action::Quit)
            .unwrap();
        assert!(quit.modifiers.alt);
        assert!(!quit.modifiers.super_key);
        assert!(quit.modifiers.shift);
        assert_eq!(
            quit.trigger,
            ResolvedTrigger::Keysym(xkb::keysym_from_name("e", xkb::KEYSYM_NO_FLAGS))
        );
        let quit = native
            .iter()
            .find(|bind| bind.action == Action::Quit)
            .unwrap();
        assert!(quit.modifiers.super_key);
        assert!(!quit.modifiers.alt);
        assert!(quit.modifiers.shift);

        for (action, key) in [
            (Action::OpenTerminal, "t"),
            (Action::ToggleState, "n"),
            (Action::Spawn("halley-lift".to_string()), "d"),
        ] {
            let bind = native.iter().find(|bind| bind.action == action).unwrap();
            assert!(bind.modifiers.super_key, "{key}");
            assert_eq!(
                bind.trigger,
                ResolvedTrigger::Keysym(xkb::keysym_from_name(key, xkb::KEYSYM_NO_FLAGS)),
                "{key}"
            );
            if key == "t" || key == "d" {
                assert!(!bind.modifiers.shift, "{key}");
            }
            if key == "d" {
                assert!(!bind.modifiers.alt);
            }
        }
        assert!(
            native
                .iter()
                .all(|bind| bind.action != Action::Spawn("fuzzel".to_string())),
            "Fuzzel is an opt-in alternative, not a default binding"
        );
    }

    #[test]
    fn every_configured_action_is_resolved() {
        let keybinds = Keybinds::default();
        let resolved = resolve_binds(&keybinds, BackendKind::Tty);
        assert!(resolved.iter().any(|bind| bind.action == Action::Quit));
        assert!(
            resolved
                .iter()
                .any(|bind| bind.action == Action::CloseFocusedWindow)
        );
        assert!(
            resolved
                .iter()
                .any(|bind| bind.action == Action::ToggleFullscreen)
        );
        assert!(
            resolved
                .iter()
                .any(|bind| bind.action == Action::ToggleFieldMaximize)
        );
        assert!(resolved.iter().any(|bind| bind.action == Action::Apogee));
        assert!(resolved.iter().any(|bind| {
            bind.action == Action::FocusCycle(halley_config::FocusCycleDirection::Forward)
        }));
        assert!(resolved.iter().any(|bind| {
            bind.action == Action::FocusCycle(halley_config::FocusCycleDirection::Backward)
        }));
        assert!(
            resolved
                .iter()
                .any(|bind| bind.action == Action::OpenTerminal)
        );
        assert!(resolved.iter().any(|bind| bind.action == Action::ZoomIn));
        assert!(resolved.iter().any(|bind| bind.action == Action::ZoomOut));
        assert!(resolved.iter().any(|bind| bind.action == Action::ZoomReset));
        assert!(
            resolved
                .iter()
                .any(|bind| bind.action == Action::Screenshot)
        );
    }

    #[test]
    fn loose_command_is_preserved_during_backend_resolution() {
        let mut keybinds = Keybinds::default();
        keybinds.binds.push(halley_config::Keybind {
            scope: halley_config::BindingScope::Global,
            modifiers: Modifiers {
                super_key: true,
                ..Modifiers::default()
            },
            key: "x".to_string(),
            action: Action::Spawn("fuzzel --show-actions".to_string()),
            repeat: false,
        });

        let resolved = resolve_binds(&keybinds, BackendKind::Winit);
        let command = resolved
            .iter()
            .find(|bind| {
                bind.trigger
                    == ResolvedTrigger::Keysym(xkb::keysym_from_name("x", xkb::KEYSYM_NO_FLAGS))
            })
            .expect("spawn bind resolves");
        assert!(command.modifiers.alt);
        assert_eq!(
            command.action,
            Action::Spawn("fuzzel --show-actions".to_string())
        );
    }

    #[test]
    fn standard_xkb_names_are_case_insensitive() {
        for name in [
            "Print",
            "page_up",
            "PAGE_DOWN",
            "delete",
            "F12",
            "XF86AudioMute",
        ] {
            assert!(matches!(
                resolve_trigger_name(name),
                Some(ResolvedTrigger::Keysym(keysym)) if keysym != Keysym::NoSymbol
            ));
        }
    }

    #[test]
    fn resolves_raw_evdev_keycodes_with_xkb_offset() {
        assert_eq!(
            resolve_trigger_name("keycode-111"),
            Some(ResolvedTrigger::Keycode(Keycode::new(119)))
        );
        assert_eq!(resolve_trigger_name("keycode-nope"), None);
        assert_eq!(resolve_trigger_name("keycode-4294967295"), None);
    }

    #[test]
    fn resolves_named_pointer_buttons_and_raw_button_codes() {
        let left = resolve_trigger_name("CLICK-LEFT").unwrap();
        assert_eq!(
            left,
            ResolvedTrigger::PointerButton(PointerButtonTrigger::Left)
        );
        assert!(PointerButtonTrigger::Left.matches(0x110));
        assert_eq!(
            resolve_trigger_name("click-middle"),
            Some(ResolvedTrigger::PointerButton(PointerButtonTrigger::Middle))
        );
        assert!(PointerButtonTrigger::Middle.matches(0x112));
        assert!(PointerButtonTrigger::Back.matches(0x113));
        assert!(PointerButtonTrigger::Back.matches(0x116));
        assert!(PointerButtonTrigger::Forward.matches(0x114));
        assert!(PointerButtonTrigger::Forward.matches(0x115));
        assert_eq!(
            resolve_trigger_name("button-279"),
            Some(ResolvedTrigger::PointerButton(PointerButtonTrigger::Code(
                279
            )))
        );
        assert_eq!(resolve_trigger_name("button-nope"), None);
    }

    #[test]
    fn resolves_all_wheel_directions() {
        assert_eq!(
            resolve_trigger_name("scroll-up"),
            Some(ResolvedTrigger::Wheel(WheelDirection::Up))
        );
        assert_eq!(
            resolve_trigger_name("scroll-down"),
            Some(ResolvedTrigger::Wheel(WheelDirection::Down))
        );
        assert_eq!(
            resolve_trigger_name("scroll-left"),
            Some(ResolvedTrigger::Wheel(WheelDirection::Left))
        );
        assert_eq!(
            resolve_trigger_name("scroll-right"),
            Some(ResolvedTrigger::Wheel(WheelDirection::Right))
        );
    }

    #[test]
    fn rejects_unknown_trigger_names() {
        assert_eq!(resolve_trigger_name("definitely-not-a-real-trigger"), None);
    }
}
