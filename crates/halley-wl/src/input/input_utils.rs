use crate::interaction::types::ModState;
use halley_config::KeyModifiers;

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

fn family_exact(
    any_down: bool,
    left_down: bool,
    right_down: bool,
    want_any: bool,
    want_left: bool,
    want_right: bool,
) -> bool {
    // No modifier from this family wanted -> none may be down.
    if !want_any && !want_left && !want_right {
        return !any_down && !left_down && !right_down;
    }

    // Specific sides requested must be down.
    if want_left && !left_down {
        return false;
    }
    if want_right && !right_down {
        return false;
    }

    // If no generic family bit is set, reject extra sides.
    if !want_any {
        if !want_left && left_down {
            return false;
        }
        if !want_right && right_down {
            return false;
        }
    }

    // Some modifier in this family must be down.
    any_down
}

pub(crate) fn modifier_exact(mods: &ModState, need: KeyModifiers) -> bool {
    family_exact(
        mods.super_down,
        mods.left_super_down,
        mods.right_super_down,
        need.super_key,
        need.left_super,
        need.right_super,
    ) && family_exact(
        mods.alt_down,
        mods.left_alt_down,
        mods.right_alt_down,
        need.alt,
        need.left_alt,
        need.right_alt,
    ) && family_exact(
        mods.ctrl_down,
        mods.left_ctrl_down,
        mods.right_ctrl_down,
        need.ctrl,
        need.left_ctrl,
        need.right_ctrl,
    ) && family_exact(
        mods.shift_down,
        mods.left_shift_down,
        mods.right_shift_down,
        need.shift,
        need.left_shift,
        need.right_shift,
    )
}
