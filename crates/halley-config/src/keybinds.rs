#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub super_key: bool,
    pub left_super: bool,
    pub right_super: bool,
    pub alt: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub ctrl: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub shift: bool,
    pub left_shift: bool,
    pub right_shift: bool,
}

#[derive(Clone, Debug)]
pub struct Keybinds {
    pub modifier: KeyModifiers,
}

#[derive(Clone, Debug)]
pub struct LaunchBinding {
    pub modifiers: KeyModifiers,
    pub key: u32,
    pub command: String,
}

pub const WHEEL_UP_CODE: u32 = 0x2000;
pub const WHEEL_DOWN_CODE: u32 = 0x2001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectionalAction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeBindingAction {
    Move(DirectionalAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrailBindingAction {
    Prev,
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorBindingTarget {
    Direction(DirectionalAction),
    Output(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorBindingAction {
    Focus(MonitorBindingTarget),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BearingsBindingAction {
    Show,
    Toggle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositorBindingAction {
    Reload,
    ToggleState,
    CloseFocusedWindow,
    Quit { requires_shift: bool },
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Node(NodeBindingAction),
    Trail(TrailBindingAction),
    Monitor(MonitorBindingAction),
    Bearings(BearingsBindingAction),
}

#[derive(Clone, Debug)]
pub struct CompositorBinding {
    pub modifiers: KeyModifiers,
    pub key: u32,
    pub action: CompositorBindingAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerBindingAction {
    MoveWindow,
    FieldJump,
    ResizeWindow,
}

#[derive(Clone, Debug)]
pub struct PointerBinding {
    pub modifiers: KeyModifiers,
    pub button: u32,
    pub action: PointerBindingAction,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            modifier: KeyModifiers {
                left_alt: true,
                ..KeyModifiers::default()
            },
        }
    }
}

impl Keybinds {
    pub fn modifier_name(&self) -> String {
        let mut parts = Vec::new();

        if self.modifier.left_super {
            parts.push("lsuper");
        }
        if self.modifier.right_super {
            parts.push("rsuper");
        }
        if self.modifier.super_key {
            parts.push("super");
        }

        if self.modifier.left_ctrl {
            parts.push("lctrl");
        }
        if self.modifier.right_ctrl {
            parts.push("rctrl");
        }
        if self.modifier.ctrl {
            parts.push("ctrl");
        }

        if self.modifier.left_alt {
            parts.push("lalt");
        }
        if self.modifier.right_alt {
            parts.push("ralt");
        }
        if self.modifier.alt {
            parts.push("alt");
        }

        if self.modifier.left_shift {
            parts.push("lshift");
        }
        if self.modifier.right_shift {
            parts.push("rshift");
        }
        if self.modifier.shift {
            parts.push("shift");
        }

        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
}

#[inline]
pub fn modifiers_empty(m: KeyModifiers) -> bool {
    !m.super_key
        && !m.left_super
        && !m.right_super
        && !m.alt
        && !m.left_alt
        && !m.right_alt
        && !m.ctrl
        && !m.left_ctrl
        && !m.right_ctrl
        && !m.shift
        && !m.left_shift
        && !m.right_shift
}

pub fn parse_modifiers(text: &str) -> Option<KeyModifiers> {
    let mut out = KeyModifiers::default();
    let mut any = false;

    for raw in text.split('+') {
        let t = raw.trim().to_ascii_lowercase();
        match t.as_str() {
            "" => {}
            "none" => {}
            "super" | "win" | "windows" | "logo" | "meta" => {
                out.super_key = true;
                any = true;
            }
            "lsuper" | "lwin" => {
                out.left_super = true;
                any = true;
            }
            "rsuper" | "rwin" => {
                out.right_super = true;
                any = true;
            }
            "alt" => {
                out.alt = true;
                any = true;
            }
            "lalt" => {
                out.left_alt = true;
                any = true;
            }
            "ralt" => {
                out.right_alt = true;
                any = true;
            }
            "ctrl" | "control" => {
                out.ctrl = true;
                any = true;
            }
            "lctrl" => {
                out.left_ctrl = true;
                any = true;
            }
            "rctrl" => {
                out.right_ctrl = true;
                any = true;
            }
            "shift" => {
                out.shift = true;
                any = true;
            }
            "lshift" => {
                out.left_shift = true;
                any = true;
            }
            "rshift" => {
                out.right_shift = true;
                any = true;
            }
            _ => return None,
        }
    }

    if any {
        Some(out)
    } else {
        Some(KeyModifiers::default())
    }
}

pub fn parse_chord(chord: &str) -> Option<(KeyModifiers, u32)> {
    let mut mods = KeyModifiers::default();
    let mut key: Option<u32> = None;

    for raw in chord.split('+') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }

        if apply_modifier_token(&mut mods, t) {
            continue;
        }

        if key.is_some() {
            return None;
        }

        key = key_name_to_evdev(t);
    }

    key.map(|k| (mods, k))
}

fn apply_modifier_token(mods: &mut KeyModifiers, token: &str) -> bool {
    match token.trim().to_ascii_lowercase().as_str() {
        "lalt" => {
            mods.left_alt = true;
            true
        }
        "ralt" => {
            mods.right_alt = true;
            true
        }
        "alt" => {
            mods.alt = true;
            true
        }
        "lshift" => {
            mods.left_shift = true;
            true
        }
        "rshift" => {
            mods.right_shift = true;
            true
        }
        "shift" => {
            mods.shift = true;
            true
        }
        "lctrl" => {
            mods.left_ctrl = true;
            true
        }
        "rctrl" => {
            mods.right_ctrl = true;
            true
        }
        "ctrl" | "control" => {
            mods.ctrl = true;
            true
        }
        "lsuper" | "lwin" => {
            mods.left_super = true;
            true
        }
        "rsuper" | "rwin" => {
            mods.right_super = true;
            true
        }
        "super" | "win" | "windows" | "logo" | "meta" => {
            mods.super_key = true;
            true
        }
        _ => false,
    }
}

pub fn key_name_to_evdev(name: &str) -> Option<u32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "none" => Some(0),

        "escape" | "esc" => Some(1),
        "1" => Some(2),
        "2" => Some(3),
        "3" => Some(4),
        "4" => Some(5),
        "5" => Some(6),
        "6" => Some(7),
        "7" => Some(8),
        "8" => Some(9),
        "9" => Some(10),
        "0" => Some(11),
        "minus" | "-" => Some(12),
        "equal" | "=" => Some(13),
        "backspace" => Some(14),
        "tab" => Some(15),

        "q" => Some(16),
        "w" => Some(17),
        "e" => Some(18),
        "r" => Some(19),
        "t" => Some(20),
        "y" => Some(21),
        "u" => Some(22),
        "i" => Some(23),
        "o" => Some(24),
        "p" => Some(25),
        "leftbrace" | "[" => Some(26),
        "rightbrace" | "]" => Some(27),
        "enter" | "return" => Some(28),

        "a" => Some(30),
        "s" => Some(31),
        "d" => Some(32),
        "f" => Some(33),
        "g" => Some(34),
        "h" => Some(35),
        "j" => Some(36),
        "k" => Some(37),
        "l" => Some(38),
        "semicolon" | ";" => Some(39),
        "apostrophe" | "'" => Some(40),
        "grave" | "`" => Some(41),
        "backslash" | "\\" => Some(43),

        "z" => Some(44),
        "x" => Some(45),
        "c" => Some(46),
        "v" => Some(47),
        "b" => Some(48),
        "n" => Some(49),
        "m" => Some(50),
        "comma" | "," => Some(51),
        "dot" | "period" | "." => Some(52),
        "slash" | "/" => Some(53),

        "space" => Some(57),

        "f1" => Some(59),
        "f2" => Some(60),
        "f3" => Some(61),
        "f4" => Some(62),
        "f5" => Some(63),
        "f6" => Some(64),
        "f7" => Some(65),
        "f8" => Some(66),
        "f9" => Some(67),
        "f10" => Some(68),
        "f11" => Some(87),
        "f12" => Some(88),

        "home" => Some(102),
        "up" => Some(103),
        "pageup" => Some(104),
        "left" => Some(105),
        "right" => Some(106),
        "end" => Some(107),
        "down" => Some(108),
        "pagedown" => Some(109),
        "insert" => Some(110),
        "delete" => Some(111),

        "mouseleft" | "leftmouse" | "leftbutton" | "btnleft" | "btn_left" => Some(272),
        "mouseright" | "rightmouse" | "rightbutton" | "btnright" | "btn_right" => Some(273),
        "mousemiddle" | "middlemouse" | "middlebutton" | "btnmiddle" | "btn_middle" => Some(274),
        "mouse4" | "mouseback" | "sidebutton" | "btnside" | "btn_side" => Some(275),
        "mouse5" | "mouseforward" | "extrabutton" | "btnextra" | "btn_extra" => Some(276),
        "mousewheelup" | "wheelup" | "scrollup" => Some(WHEEL_UP_CODE),
        "mousewheeldown" | "wheeldown" | "scrolldown" => Some(WHEEL_DOWN_CODE),

        "xf86audiomute" | "audiomute" | "mute" => Some(113),
        "xf86audiolowervolume" | "audiolowervolume" | "volumedown" => Some(114),
        "xf86audioraisevolume" | "audioraisevolume" | "volumeup" => Some(115),
        "xf86audiostop" | "audiostop" | "stopmedia" => Some(166),
        "xf86audioplay" | "audioplay" | "playpause" => Some(164),
        "xf86audioprev" | "audioprev" | "previoussong" => Some(165),
        "xf86audionext" | "audionext" | "nextsong" => Some(163),
        "xf86audiorecord" | "audiorecord" => Some(167),
        "xf86audiorewind" | "audiorewind" | "rewind" => Some(168),
        "xf86homepage" | "homepage" => Some(172),
        "xf86search" | "search" => Some(217),
        "xf86monbrightnessdown" | "brightnessdown" => Some(224),
        "xf86monbrightnessup" | "brightnessup" => Some(225),
        "xf86mail" | "mail" => Some(155),
        "xf86calculator" | "calculator" => Some(140),
        "xf86sleep" | "sleep" => Some(142),
        "xf86audiopause" | "audiopause" => Some(201),
        "xf86audiomicmute" | "audiomicmute" | "micmute" => Some(248),

        _ => None,
    }
}

#[inline]
pub fn is_pointer_button_code(code: u32) -> bool {
    matches!(code, 272..=276)
}

#[inline]
pub fn is_wheel_code(code: u32) -> bool {
    matches!(code, WHEEL_UP_CODE | WHEEL_DOWN_CODE)
}

pub fn evdev_to_key_name(code: u32) -> &'static str {
    match code {
        0 => "None",
        1 => "Escape",
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        12 => "Minus",
        13 => "Equal",
        14 => "Backspace",
        15 => "Tab",
        16 => "Q",
        17 => "W",
        18 => "E",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        26 => "[",
        27 => "]",
        28 => "Return",
        30 => "A",
        31 => "S",
        32 => "D",
        33 => "F",
        34 => "G",
        35 => "H",
        36 => "J",
        37 => "K",
        38 => "L",
        39 => ";",
        40 => "'",
        41 => "`",
        43 => "\\",
        44 => "Z",
        45 => "X",
        46 => "C",
        47 => "V",
        48 => "B",
        49 => "N",
        50 => "M",
        51 => "Comma",
        52 => "Period",
        53 => "Slash",
        57 => "Space",
        59 => "F1",
        60 => "F2",
        61 => "F3",
        62 => "F4",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        68 => "F10",
        87 => "F11",
        88 => "F12",
        102 => "Home",
        103 => "Up",
        104 => "PageUp",
        105 => "Left",
        106 => "Right",
        107 => "End",
        108 => "Down",
        109 => "PageDown",
        110 => "Insert",
        111 => "Delete",
        140 => "XF86Calculator",
        142 => "XF86Sleep",
        155 => "XF86Mail",
        113 => "XF86AudioMute",
        114 => "XF86AudioLowerVolume",
        115 => "XF86AudioRaiseVolume",
        163 => "XF86AudioNext",
        164 => "XF86AudioPlay",
        165 => "XF86AudioPrev",
        166 => "XF86AudioStop",
        167 => "XF86AudioRecord",
        168 => "XF86AudioRewind",
        172 => "XF86HomePage",
        201 => "XF86AudioPause",
        217 => "XF86Search",
        224 => "XF86MonBrightnessDown",
        225 => "XF86MonBrightnessUp",
        248 => "XF86AudioMicMute",
        272 => "MouseLeft",
        273 => "MouseRight",
        274 => "MouseMiddle",
        275 => "MouseBack",
        276 => "MouseForward",
        WHEEL_UP_CODE => "MouseWheelUp",
        WHEEL_DOWN_CODE => "MouseWheelDown",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WHEEL_DOWN_CODE, WHEEL_UP_CODE, evdev_to_key_name, key_name_to_evdev, parse_chord,
        parse_modifiers,
    };

    #[test]
    fn generic_alt_modifier_matches_either_side_in_config() {
        let mods = parse_modifiers("alt").expect("alt should parse");
        assert!(mods.alt);
        assert!(!mods.left_alt);
        assert!(!mods.right_alt);

        let (chord_mods, key) = parse_chord("alt+r").expect("alt+r should parse");
        assert!(chord_mods.alt);
        assert_eq!(key, 19);
    }

    #[test]
    fn mouse_button_aliases_resolve_to_pointer_button_codes() {
        assert_eq!(key_name_to_evdev("mouseleft"), Some(272));
        assert_eq!(key_name_to_evdev("btn_right"), Some(273));
        assert_eq!(key_name_to_evdev("middlemouse"), Some(274));
        assert_eq!(key_name_to_evdev("mouseback"), Some(275));
        assert_eq!(key_name_to_evdev("mouseforward"), Some(276));
        assert_eq!(key_name_to_evdev("mousewheelup"), Some(WHEEL_UP_CODE));
        assert_eq!(key_name_to_evdev("mousewheeldown"), Some(WHEEL_DOWN_CODE));
    }

    #[test]
    fn xf86_media_aliases_resolve_to_expected_codes() {
        assert_eq!(key_name_to_evdev("XF86AudioMute"), Some(113));
        assert_eq!(key_name_to_evdev("XF86AudioStop"), Some(166));
        assert_eq!(key_name_to_evdev("XF86AudioPause"), Some(201));
        assert_eq!(key_name_to_evdev("XF86AudioMicMute"), Some(248));
        assert_eq!(key_name_to_evdev("XF86MonBrightnessUp"), Some(225));
    }

    #[test]
    fn reverse_lookup_uses_canonical_names_for_new_codes() {
        assert_eq!(evdev_to_key_name(272), "MouseLeft");
        assert_eq!(evdev_to_key_name(275), "MouseBack");
        assert_eq!(evdev_to_key_name(WHEEL_UP_CODE), "MouseWheelUp");
        assert_eq!(evdev_to_key_name(WHEEL_DOWN_CODE), "MouseWheelDown");
        assert_eq!(evdev_to_key_name(166), "XF86AudioStop");
        assert_eq!(evdev_to_key_name(248), "XF86AudioMicMute");
    }
}
