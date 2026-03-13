use std::process::Command;

use eventline::{info, warn};
use halley_ipc::NodeMoveDirection;

use super::input_utils::{key_matches, modifier_exact};
use crate::interaction::actions::{
    minimize_focused_active_node, move_latest_node_direction, set_docking_active,
};
use crate::interaction::types::ModState;
use crate::run::request_xwayland_start;
use crate::state::HalleyWlState;
use halley_config::{KeyModifiers, RuntimeTuning};

fn with_extra_shift(base: KeyModifiers) -> KeyModifiers {
    let mut out = base;
    out.shift = true;
    out
}

pub(crate) fn key_is_compositor_binding(
    st: &HalleyWlState,
    key_code: u32,
    mods: &ModState,
) -> bool {
    let kb = &st.tuning.keybinds;

    if key_matches(key_code, kb.quit) {
        let need = if st.tuning.quit_requires_shift {
            with_extra_shift(kb.quit_modifiers)
        } else {
            kb.quit_modifiers
        };
        if modifier_exact(mods, need) {
            return true;
        }
    }

    for binding in &st.tuning.launch_bindings {
        if key_matches(key_code, binding.key) && modifier_exact(mods, binding.modifiers) {
            return true;
        }
    }

    if key_matches(key_code, kb.reload) && modifier_exact(mods, kb.reload_modifiers) {
        return true;
    }

    if key_matches(key_code, kb.minimize_focused)
        && modifier_exact(mods, kb.minimize_focused_modifiers)
    {
        return true;
    }

    if key_matches(key_code, kb.docking) && modifier_exact(mods, kb.docking_modifiers) {
        return true;
    }

    if matches!(
        key_code,
        code if key_matches(code, kb.move_left) && modifier_exact(mods, kb.move_left_modifiers)
            || key_matches(code, kb.move_right) && modifier_exact(mods, kb.move_right_modifiers)
            || key_matches(code, kb.move_up) && modifier_exact(mods, kb.move_up_modifiers)
            || key_matches(code, kb.move_down) && modifier_exact(mods, kb.move_down_modifiers)
    ) {
        return true;
    }

    if !st.tuning.dev_enabled {
        return false;
    }

    matches!(
        key_code,
        code if key_matches(code, kb.primary_left) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.primary_right) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.primary_up) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.primary_down) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.secondary_left) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.secondary_right) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.secondary_up) && modifier_exact(mods, kb.modifier)
            || key_matches(code, kb.secondary_down) && modifier_exact(mods, kb.modifier)
    )
}

pub(crate) fn apply_bound_key(
    st: &mut HalleyWlState,
    key_code: u32,
    mods: &ModState,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    const STEP_RX: f32 = 24.0;
    const STEP_RY: f32 = 16.0;
    const STEP_OFFSET: f32 = 24.0;

    let kb = st.tuning.keybinds.clone();

    if key_matches(key_code, kb.quit) {
        let need = if st.tuning.quit_requires_shift {
            with_extra_shift(kb.quit_modifiers)
        } else {
            kb.quit_modifiers
        };
        if modifier_exact(mods, need) {
            st.request_exit();
            info!("quit requested via keybind");
            return true;
        }
        return false;
    }

    for binding in st.tuning.launch_bindings.clone() {
        if key_matches(key_code, binding.key) && modifier_exact(mods, binding.modifiers) {
            return spawn_command(binding.command.as_str(), wayland_display, "command");
        }
    }

    if key_matches(key_code, kb.reload) && modifier_exact(mods, kb.reload_modifiers) {
        let next = RuntimeTuning::load_from_path(config_path);
        st.apply_tuning(next);
        info!("manual config reload from {}", config_path);
        info!(
            "resolved keybinds: {}",
            st.tuning.keybinds_resolved_summary()
        );
        return true;
    }

    if key_matches(key_code, kb.minimize_focused)
        && modifier_exact(mods, kb.minimize_focused_modifiers)
    {
        return minimize_focused_active_node(st);
    }

    if key_matches(key_code, kb.docking) && modifier_exact(mods, kb.docking_modifiers) {
        return set_docking_active(st, true);
    }

    let changed = match key_code {
        code if key_matches(code, kb.move_left) && modifier_exact(mods, kb.move_left_modifiers) => {
            move_latest_node_direction(st, NodeMoveDirection::Left);
            true
        }
        code if key_matches(code, kb.move_right)
            && modifier_exact(mods, kb.move_right_modifiers) =>
        {
            move_latest_node_direction(st, NodeMoveDirection::Right);
            true
        }
        code if key_matches(code, kb.move_up) && modifier_exact(mods, kb.move_up_modifiers) => {
            move_latest_node_direction(st, NodeMoveDirection::Up);
            true
        }
        code if key_matches(code, kb.move_down) && modifier_exact(mods, kb.move_down_modifiers) => {
            move_latest_node_direction(st, NodeMoveDirection::Down);
            true
        }
        _ if !st.tuning.dev_enabled => false,

        code if key_matches(code, kb.primary_left) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_rx -= STEP_RX;
            true
        }
        code if key_matches(code, kb.primary_right) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_rx += STEP_RX;
            true
        }
        code if key_matches(code, kb.primary_up) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_ry += STEP_RY;
            true
        }
        code if key_matches(code, kb.primary_down) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_ry -= STEP_RY;
            true
        }

        code if key_matches(code, kb.secondary_left) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_offset_x -= STEP_OFFSET;
            true
        }
        code if key_matches(code, kb.secondary_right) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_offset_x += STEP_OFFSET;
            true
        }
        code if key_matches(code, kb.secondary_up) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_offset_y += STEP_OFFSET;
            true
        }
        code if key_matches(code, kb.secondary_down) && modifier_exact(mods, kb.modifier) => {
            st.tuning.focus_ring_offset_y -= STEP_OFFSET;
            true
        }
        _ => false,
    };

    if changed {
        st.tuning.enforce_guards();
        info!(
            "focus-ring {:.0}x{:.0} offset=({:.0},{:.0})",
            st.tuning.focus_ring_rx,
            st.tuning.focus_ring_ry,
            st.tuning.focus_ring_offset_x,
            st.tuning.focus_ring_offset_y
        );
    }

    changed
}

pub(crate) fn release_bound_key(st: &mut HalleyWlState, key_code: u32, mods: &ModState) -> bool {
    let kb = &st.tuning.keybinds;
    if key_matches(key_code, kb.docking) && modifier_exact(mods, kb.docking_modifiers) {
        return set_docking_active(st, false);
    }
    false
}

fn spawn_command(command: &str, wayland_display: &str, label: &str) -> bool {
    request_xwayland_start();
    match Command::new("sh")
        .arg("-lc")
        .arg(command)
        .env("WAYLAND_DISPLAY", wayland_display)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("GDK_BACKEND", "wayland,x11")
        .env("QT_QPA_PLATFORM", "wayland;xcb")
        .env("SDL_VIDEODRIVER", "wayland")
        .env("CLUTTER_BACKEND", "wayland")
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("ELECTRON_OZONE_PLATFORM_HINT", "auto")
        .spawn()
    {
        Ok(_) => {
            info!(
                "spawned {} via `{}` on WAYLAND_DISPLAY={}",
                label, command, wayland_display
            );
            true
        }
        Err(err) => {
            warn!("{} spawn failed via `{}`: {}", label, command, err);
            false
        }
    }
}
