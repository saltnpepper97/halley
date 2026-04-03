use super::{key_name_to_evdev, KeyModifiers};

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

