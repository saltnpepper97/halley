use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;

use eventline::{debug, info, warn};

use super::input_utils::{key_matches, modifier_active};
use crate::interaction::actions::{move_latest_node_direction, toggle_focused_active_node_state};
use crate::interaction::types::ModState;
use crate::run::request_xwayland_start;
use crate::state::Halley;
use crate::surface_ops::request_close_focused_toplevel;
use halley_config::keybinds::{is_pointer_button_code, is_wheel_code};
use halley_config::{CompositorBindingAction, DirectionalAction, RuntimeTuning};
use halley_ipc::NodeMoveDirection;

pub(crate) fn input_matches_binding(actual: u32, binding_key: u32) -> bool {
    if is_pointer_button_code(binding_key) || is_wheel_code(binding_key) {
        actual == binding_key
    } else {
        key_matches(actual, binding_key)
    }
}

fn from_directional_action(direction: DirectionalAction) -> NodeMoveDirection {
    match direction {
        DirectionalAction::Left => NodeMoveDirection::Left,
        DirectionalAction::Right => NodeMoveDirection::Right,
        DirectionalAction::Up => NodeMoveDirection::Up,
        DirectionalAction::Down => NodeMoveDirection::Down,
    }
}

pub(crate) fn compositor_binding_action(
    st: &Halley,
    key_code: u32,
    mods: &ModState,
) -> Option<CompositorBindingAction> {
    for binding in &st.tuning.compositor_bindings {
        if input_matches_binding(key_code, binding.key) && modifier_active(mods, binding.modifiers) {
            return Some(binding.action);
        }
    }

    None
}

pub(crate) fn compositor_binding_action_active(
    st: &Halley,
    key_code: u32,
    mods: &ModState,
) -> Option<CompositorBindingAction> {
    for binding in &st.tuning.compositor_bindings {
        if input_matches_binding(key_code, binding.key) && modifier_active(mods, binding.modifiers)
        {
            return Some(binding.action);
        }
    }

    None
}

pub(crate) fn key_is_compositor_binding(
    st: &Halley,
    key_code: u32,
    mods: &ModState,
) -> bool {
    compositor_binding_action(st, key_code, mods).is_some()
        || st.tuning.launch_bindings.iter().any(|binding| {
            input_matches_binding(key_code, binding.key) && modifier_active(mods, binding.modifiers)
        })
}

pub(crate) fn apply_compositor_action_press(
    st: &mut Halley,
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
            if let Some(next) = RuntimeTuning::try_load_from_path(config_path) {
                crate::run::apply_reloaded_tuning(st, next, config_path, wayland_display, "manual");
                info!("manual config reload from {}", config_path);
                info!(
                    "resolved keybinds: {}",
                    st.tuning.keybinds_resolved_summary()
                );
            } else {
                warn!(
                    "manual reload skipped for {} because config parse/load failed",
                    config_path
                );
            }
            true
        }
        CompositorBindingAction::ToggleState => toggle_focused_active_node_state(st),
        CompositorBindingAction::CloseFocusedWindow => request_close_focused_toplevel(st),
        CompositorBindingAction::MoveNode(direction) => {
            move_latest_node_direction(st, from_directional_action(direction))
        }
        CompositorBindingAction::TrailPrev => {
            crate::interaction::actions::step_window_trail(st, halley_ipc::TrailDirection::Prev)
        }
        CompositorBindingAction::TrailNext => {
            crate::interaction::actions::step_window_trail(st, halley_ipc::TrailDirection::Next)
        }
        CompositorBindingAction::ZoomIn => {
            st.zoom_by_steps(1.0);
            true
        }
        CompositorBindingAction::ZoomOut => {
            st.zoom_by_steps(-1.0);
            true
        }
        CompositorBindingAction::ZoomReset => {
            st.reset_zoom();
            true
        }
    }
}

pub(crate) fn apply_compositor_action_release(
    _st: &mut Halley,
    _action: CompositorBindingAction,
) -> bool {
    false
}

pub(crate) fn apply_bound_key(
    st: &mut Halley,
    key_code: u32,
    mods: &ModState,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    if let Some(action) = compositor_binding_action(st, key_code, mods) {
        return match action {
            CompositorBindingAction::MoveNode(_)
            | CompositorBindingAction::Reload
            | CompositorBindingAction::ToggleState
            | CompositorBindingAction::CloseFocusedWindow
            | CompositorBindingAction::TrailPrev
            | CompositorBindingAction::TrailNext
            | CompositorBindingAction::Quit { .. }
            | CompositorBindingAction::ZoomIn
            | CompositorBindingAction::ZoomOut
            | CompositorBindingAction::ZoomReset => {
                apply_compositor_action_press(st, action, config_path, wayland_display)
            }
        };
    }

    for binding in st.tuning.launch_bindings.clone() {
        if input_matches_binding(key_code, binding.key) && modifier_active(mods, binding.modifiers) {
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

pub(crate) fn apply_bound_pointer_input(
    st: &mut Halley,
    key_code: u32,
    mods: &ModState,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    if let Some(action) = compositor_binding_action_active(st, key_code, mods) {
        return apply_compositor_action_press(st, action, config_path, wayland_display);
    }

    for binding in st.tuning.launch_bindings.clone() {
        if input_matches_binding(key_code, binding.key) && modifier_active(mods, binding.modifiers)
        {
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
        .env("ELECTRON_OZONE_PLATFORM_HINT", "auto")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

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
            debug!(
                "spawned {} via `{}` on WAYLAND_DISPLAY={} (pid={})",
                label,
                command,
                wayland_display,
                child.id()
            );
            Some(child)
        }
        Err(err) => {
            warn!("{} spawn failed via `{}`: {}", label, command, err);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::input_matches_binding;
    use halley_config::WHEEL_UP_CODE;
    use halley_config::keybinds::key_name_to_evdev;

    #[test]
    fn matcher_accepts_direct_wheel_codes() {
        assert!(input_matches_binding(WHEEL_UP_CODE, WHEEL_UP_CODE));
    }

    #[test]
    fn matcher_keeps_keyboard_xkb_translation() {
        assert!(input_matches_binding(13 + 8, 13));
    }

    #[test]
    fn matcher_does_not_confuse_return_with_j() {
        let return_xkb = key_name_to_evdev("return").expect("return") + 8;
        let j_evdev = key_name_to_evdev("j").expect("j");
        assert!(!input_matches_binding(return_xkb, j_evdev));
    }
}
