use super::types::KeyModifiers;

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
