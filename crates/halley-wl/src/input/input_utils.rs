use crate::config::KeyModifiers;
use crate::interaction::types::ModState;

pub(crate) fn key_matches(actual: u32, evdev_code: u32) -> bool {
    actual == evdev_code || actual == evdev_code + 8
}

pub(crate) fn key_matches_xkb_only(actual: u32, evdev_code: u32) -> bool {
    actual == evdev_code + 8
}

pub(crate) fn update_mod_state(mods: &mut ModState, code: u32, pressed: bool) {
    if code == 125 || code == 133 {
        mods.left_super_down = pressed;
        mods.super_down = mods.left_super_down || mods.right_super_down;
    } else if code == 126 || code == 134 {
        mods.right_super_down = pressed;
        mods.super_down = mods.left_super_down || mods.right_super_down;
    } else if code == 56 || code == 64 {
        mods.left_alt_down = pressed;
        mods.alt_down = mods.left_alt_down || mods.right_alt_down;
    } else if code == 100 || code == 108 {
        mods.right_alt_down = pressed;
        mods.alt_down = mods.left_alt_down || mods.right_alt_down;
    } else if code == 29 || code == 37 {
        mods.left_ctrl_down = pressed;
        mods.ctrl_down = mods.left_ctrl_down || mods.right_ctrl_down;
    } else if code == 97 || code == 105 {
        mods.right_ctrl_down = pressed;
        mods.ctrl_down = mods.left_ctrl_down || mods.right_ctrl_down;
    } else if code == 42 || code == 50 {
        mods.left_shift_down = pressed;
        mods.shift_down = mods.left_shift_down || mods.right_shift_down;
    } else if code == 54 || code == 62 {
        mods.right_shift_down = pressed;
        mods.shift_down = mods.left_shift_down || mods.right_shift_down;
    }
}

pub(crate) fn modifier_active(mods: &ModState, need: KeyModifiers) -> bool {
    (!need.super_key || mods.super_down)
        && (!need.left_super || mods.left_super_down)
        && (!need.right_super || mods.right_super_down)
        && (!need.alt || mods.alt_down)
        && (!need.left_alt || mods.left_alt_down)
        && (!need.right_alt || mods.right_alt_down)
        && (!need.ctrl || mods.ctrl_down)
        && (!need.left_ctrl || mods.left_ctrl_down)
        && (!need.right_ctrl || mods.right_ctrl_down)
        && (!need.shift || mods.shift_down)
        && (!need.left_shift || mods.left_shift_down)
        && (!need.right_shift || mods.right_shift_down)
}
