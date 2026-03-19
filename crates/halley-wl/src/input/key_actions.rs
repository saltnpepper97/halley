use std::process::Command;
use std::os::unix::process::CommandExt;
use std::process::Child;

use eventline::{info, warn};

use super::input_utils::{key_matches, modifier_exact};
use crate::interaction::actions::{
    docking_mode_active, minimize_focused_active_node, move_latest_node_direction, set_docking_mode,
};
use crate::interaction::types::ModState;
use crate::run::request_xwayland_start;
use crate::state::HalleyWlState;
use halley_config::{CompositorBindingAction, DirectionalAction, RuntimeTuning};
use halley_ipc::NodeMoveDirection;

fn from_directional_action(direction: DirectionalAction) -> NodeMoveDirection {
    match direction {
        DirectionalAction::Left => NodeMoveDirection::Left,
        DirectionalAction::Right => NodeMoveDirection::Right,
        DirectionalAction::Up => NodeMoveDirection::Up,
        DirectionalAction::Down => NodeMoveDirection::Down,
    }
}

pub(crate) fn compositor_binding_action(
    st: &HalleyWlState,
    key_code: u32,
    mods: &ModState,
) -> Option<CompositorBindingAction> {
    for binding in &st.tuning.compositor_bindings {
        if key_matches(key_code, binding.key) && modifier_exact(mods, binding.modifiers) {
            return Some(binding.action);
        }
    }

    None
}

pub(crate) fn key_is_compositor_binding(
    st: &HalleyWlState,
    key_code: u32,
    mods: &ModState,
) -> bool {
    compositor_binding_action(st, key_code, mods).is_some()
        || st.tuning.launch_bindings.iter().any(|binding| {
            key_matches(key_code, binding.key) && modifier_exact(mods, binding.modifiers)
        })
}

pub(crate) fn apply_compositor_action_press(
    st: &mut HalleyWlState,
    action: CompositorBindingAction,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    match action {
        CompositorBindingAction::Quit { .. } => {
            st.request_exit();
            info!("quit requested via keybind");
            true
        }
        CompositorBindingAction::Reload => {
            let next = RuntimeTuning::load_from_path(config_path);
            crate::run::apply_reloaded_tuning(st, next, config_path, wayland_display, "manual");
            info!("manual config reload from {}", config_path);
            info!(
                "resolved keybinds: {}",
                st.tuning.keybinds_resolved_summary()
            );
            true
        }
        CompositorBindingAction::MinimizeFocused => minimize_focused_active_node(st),
        CompositorBindingAction::OverviewToggle => false,
        CompositorBindingAction::Docking => set_docking_mode(st, true),
        CompositorBindingAction::MoveNode(direction) => {
            move_latest_node_direction(st, from_directional_action(direction))
        }
    }
}

pub(crate) fn apply_compositor_action_release(
    st: &mut HalleyWlState,
    action: CompositorBindingAction,
) -> bool {
    match action {
        CompositorBindingAction::Docking if docking_mode_active(st) => {
            set_docking_mode(st, false);
            true
        }
        CompositorBindingAction::Docking => {
            set_docking_mode(st, false);
            false
        }
        _ => false,
    }
}

pub(crate) fn apply_bound_key(
    st: &mut HalleyWlState,
    key_code: u32,
    mods: &ModState,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    if let Some(action) = compositor_binding_action(st, key_code, mods) {
        return match action {
            CompositorBindingAction::MoveNode(_)
            | CompositorBindingAction::Docking
            | CompositorBindingAction::Reload
            | CompositorBindingAction::MinimizeFocused
            | CompositorBindingAction::Quit { .. }
            | CompositorBindingAction::OverviewToggle => {
                apply_compositor_action_press(st, action, config_path, wayland_display)
            }
        };
    }

    for binding in st.tuning.launch_bindings.clone() {
        if key_matches(key_code, binding.key) && modifier_exact(mods, binding.modifiers) {
            // FIX: store the child so it's tracked for cleanup on WM exit,
            // rather than dropping it immediately (which orphaned the process).
            let ok = match spawn_command(binding.command.as_str(), wayland_display, "command") {
                Some(child) => {
                    st.spawned_children.push(child);
                    true
                }
                None => false,
            };
            return ok;
        }
    }
    false
}

pub(crate) fn spawn_command(command: &str, wayland_display: &str, label: &str) -> Option<Child> {
    request_xwayland_start();
    let mut cmd = Command::new("sh");
    cmd.arg("-lc")
        .arg(command)
        .env("WAYLAND_DISPLAY", wayland_display)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("GDK_BACKEND", "wayland,x11")
        .env("QT_QPA_PLATFORM", "wayland;xcb")
        .env("SDL_VIDEODRIVER", "wayland")
        .env("CLUTTER_BACKEND", "wayland")
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("ELECTRON_OZONE_PLATFORM_HINT", "auto");

    // Give each spawned app its own process group so we can kill
    // the whole group (including any children it forks) on WM exit.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    match cmd.spawn() {
        Ok(child) => {
            info!(
                "spawned {} via `{}` on WAYLAND_DISPLAY={} (pid={})",
                label, command, wayland_display, child.id()
            );
            Some(child)
        }
        Err(err) => {
            warn!("{} spawn failed via `{}`: {}", label, command, err);
            None
        }
    }
}
