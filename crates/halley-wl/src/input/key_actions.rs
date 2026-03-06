use std::process::Command;
use std::time::Instant;

use eventline::{info, warn};

use super::input_utils::{key_matches, key_matches_xkb_only, modifier_active};
use crate::config::RuntimeTuning;
use crate::interaction::actions::{minimize_focused_active_node, move_latest_node};
use crate::interaction::types::ModState;
use crate::run::request_xwayland_start;
use crate::state::HalleyWlState;

pub(crate) fn apply_bound_key(
    st: &mut HalleyWlState,
    key_code: u32,
    mods: &ModState,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    const STEP_RX: f32 = 24.0;
    const STEP_RY: f32 = 16.0;
    const STEP_NODE: f32 = 80.0;
    let kb = st.tuning.keybinds.clone();
    if key_matches(key_code, kb.quit_compositor) {
        if modifier_active(mods, kb.modifier) && (!st.tuning.quit_requires_shift || mods.shift_down)
        {
            st.request_exit();
            info!("quit requested via keybind");
            return true;
        }
        return false;
    }

    // Custom launch bindings can define their own modifiers; do not gate these
    // behind the global keybind modifier.
    for binding in st.tuning.launch_bindings.clone() {
        if key_matches(key_code, binding.key) && modifier_active(mods, binding.modifiers) {
            return spawn_command(binding.command.as_str(), wayland_display, "command");
        }
    }

    if !modifier_active(mods, kb.modifier) {
        return false;
    }

    if key_matches(key_code, kb.reload_config) {
        let next = RuntimeTuning::load_from_path(config_path);
        st.apply_tuning(next);
        info!("manual config reload from {}", config_path);
        info!(
            "resolved keybinds: {}",
            st.tuning.keybinds_resolved_summary()
        );
        return true;
    }
    if key_matches(key_code, kb.minimize_focused) {
        return minimize_focused_active_node(st);
    }
    if key_matches(key_code, kb.launch_pavucontrol) {
        return spawn_command(
            st.tuning.keybind_launch_command.as_str(),
            wayland_display,
            "command",
        );
    }

    if !st.tuning.dev_enabled {
        return false;
    }
    if key_matches_xkb_only(key_code, kb.overview_toggle) {
        st.toggle_overview_mode(Instant::now());
        return true;
    }

    let changed = match key_code {
        // Keep window movement keys highest priority in dev mode.
        code if key_matches(code, kb.move_left) => {
            move_latest_node(st, -STEP_NODE, 0.0);
            true
        }
        code if key_matches(code, kb.move_right) => {
            move_latest_node(st, STEP_NODE, 0.0);
            true
        }
        code if key_matches(code, kb.move_up) => {
            move_latest_node(st, 0.0, STEP_NODE);
            true
        }
        code if key_matches(code, kb.move_down) => {
            move_latest_node(st, 0.0, -STEP_NODE);
            true
        }
        code if key_matches(code, kb.primary_left) => {
            st.tuning.ring_primary_rx -= STEP_RX;
            true
        }
        code if key_matches(code, kb.primary_right) => {
            st.tuning.ring_primary_rx += STEP_RX;
            true
        }
        code if key_matches(code, kb.primary_up) => {
            st.tuning.ring_primary_ry += STEP_RY;
            true
        }
        code if key_matches(code, kb.primary_down) => {
            st.tuning.ring_primary_ry -= STEP_RY;
            true
        }
        code if key_matches(code, kb.secondary_left) => {
            st.tuning.ring_secondary_rx -= STEP_RX;
            true
        }
        code if key_matches(code, kb.secondary_right) => {
            st.tuning.ring_secondary_rx += STEP_RX;
            true
        }
        code if key_matches(code, kb.secondary_up) => {
            st.tuning.ring_secondary_ry += STEP_RY;
            true
        }
        code if key_matches(code, kb.secondary_down) => {
            st.tuning.ring_secondary_ry -= STEP_RY;
            true
        }
        _ => false,
    };

    if changed {
        st.tuning.enforce_guards();
        info!(
            "rings primary={:.0}x{:.0} secondary={:.0}x{:.0} rot={:.2}",
            st.tuning.ring_primary_rx,
            st.tuning.ring_primary_ry,
            st.tuning.ring_secondary_rx,
            st.tuning.ring_secondary_ry,
            st.tuning.ring_rotation_rad
        );
    }

    changed
}

pub(crate) fn spawn_terminal(wayland_display: &str) -> bool {
    let candidates: Vec<String> = ["kitty", "alacritty", "foot", "wezterm", "xterm"]
        .into_iter()
        .map(str::to_string)
        .collect();

    for cmd in candidates {
        if spawn_command(&cmd, wayland_display, "terminal") {
            return true;
        }
    }

    warn!("no terminal launcher available");
    false
}

fn spawn_command(command: &str, wayland_display: &str, label: &str) -> bool {
    // On-demand xwayland: launcher/spawn activity can request the satellite.
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
