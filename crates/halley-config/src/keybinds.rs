
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
    pub reload_config: u32,
    pub minimize_focused: u32,
    pub overview_toggle: u32,
    pub quit_compositor: u32,
    pub primary_left: u32,
    pub primary_right: u32,
    pub primary_up: u32,
    pub primary_down: u32,
    pub secondary_left: u32,
    pub secondary_right: u32,
    pub secondary_up: u32,
    pub secondary_down: u32,
    pub move_left: u32,
    pub move_right: u32,
    pub move_up: u32,
    pub move_down: u32,
}

#[derive(Clone, Debug)]
pub struct LaunchBinding {
    pub modifiers: KeyModifiers,
    pub key: u32,
    pub command: String,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            modifier: KeyModifiers {
                left_alt: true,
                ..KeyModifiers::default()
            },
            reload_config: 19,      // r
            minimize_focused: 49,   // n
            overview_toggle: 24,    // o
            quit_compositor: 16,    // q (with mod+shift hard requirement in key handler)
            primary_left: 105,      // left
            primary_right: 106,     // right
            primary_up: 103,        // up
            primary_down: 108,      // down
            secondary_left: 36,     // j
            secondary_right: 38,    // l
            secondary_up: 23,       // i
            secondary_down: 37,     // k
            move_left: 30,          // a
            move_right: 32,         // d
            move_up: 17,            // w
            move_down: 31,          // s
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
            "super" | "win" | "windows" | "logo" => {
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
        "enter" | "return" => Some(28),
        "space" => Some(57),
        "escape" | "esc" => Some(1),
        "tab" => Some(15),
        "left" => Some(105),
        "right" => Some(106),
        "up" => Some(103),
        "down" => Some(108),
        "f5" => Some(63),
        "f6" => Some(64),
        "f7" => Some(65),
        "f8" => Some(66),
        "f9" => Some(67),
        "p" => Some(25),
        "q" => Some(16),
        "r" => Some(19),
        "w" => Some(17),
        "i" => Some(23),
        "a" => Some(30),
        "s" => Some(31),
        "d" => Some(32),
        "j" => Some(36),
        "k" => Some(37),
        "l" => Some(38),
        "m" => Some(50),
        "n" => Some(49),
        "o" => Some(24),
        "t" => Some(20),
        "u" => Some(22),
        "v" => Some(47),
        "x" => Some(45),
        "y" => Some(21),
        "z" => Some(44),
        _ => None,
    }
}

pub fn evdev_to_key_name(code: u32) -> &'static str {
    match code {
        0 => "none",
        1 => "Escape",
        15 => "Tab",
        16 => "Q",
        17 => "W",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        28 => "Return",
        30 => "A",
        31 => "S",
        32 => "D",
        36 => "J",
        37 => "K",
        38 => "L",
        44 => "Z",
        45 => "X",
        47 => "V",
        49 => "N",
        50 => "M",
        57 => "Space",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        103 => "Up",
        105 => "Left",
        106 => "Right",
        108 => "Down",
        _ => "?",
    }
}
