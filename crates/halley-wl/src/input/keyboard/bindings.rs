use eventline::{debug, info, warn};

use super::modkeys::{key_matches, modifier_exact};
use crate::compositor::actions::window::{
    move_latest_node_direction, toggle_focused_active_node_state,
    toggle_focused_maximize_node_state, toggle_focused_pin_state,
};
use crate::compositor::exit_confirm::exit_confirm_controller;
use crate::compositor::interaction::ModState;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::compositor::surface::request_close_focused_toplevel;
use halley_config::keybinds::{is_pointer_button_code, is_wheel_code};
use halley_config::{
    BearingsBindingAction, ClusterBindingAction, CompositorBindingAction, CompositorBindingScope,
    DirectionalAction, FocusCycleBindingAction, MonitorBindingAction, MonitorBindingTarget,
    NodeBindingAction, RuntimeTuning, StackBindingAction, StackCycleDirection, TileBindingAction,
    TrailBindingAction,
};
use halley_ipc::NodeMoveDirection;
use std::time::Instant;

fn spawn_launch_binding(st: &mut Halley, command: &str, wayland_display: &str) -> bool {
    let activation_token =
        crate::protocol::wayland::activation::issue_external_token(st, st.now_ms(Instant::now()));
    match super::spawn::spawn_command(
        command,
        wayland_display,
        &st.runtime.tuning.cursor,
        Some(activation_token.as_str()),
        "command",
    ) {
        Some(child) => {
            st.runtime.spawned_children.push(child);
            true
        }
        None => false,
    }
}

fn spawn_open_terminal_binding(st: &mut Halley, wayland_display: &str) -> bool {
    let activation_token =
        crate::protocol::wayland::activation::issue_external_token(st, st.now_ms(Instant::now()));
    match super::spawn::spawn_wayland_terminal(
        wayland_display,
        &st.runtime.tuning.cursor,
        Some(activation_token.as_str()),
    ) {
        Some(child) => {
            st.runtime.spawned_children.push(child);
            true
        }
        None => false,
    }
}

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

fn active_binding_scopes(st: &Halley) -> [CompositorBindingScope; 3] {
    if st.has_active_cluster_workspace() {
        match st.runtime.tuning.cluster_layout_kind() {
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling => [
                CompositorBindingScope::Tile,
                CompositorBindingScope::Cluster,
                CompositorBindingScope::Global,
            ],
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking => [
                CompositorBindingScope::Stack,
                CompositorBindingScope::Cluster,
                CompositorBindingScope::Global,
            ],
        }
    } else {
        [
            CompositorBindingScope::Field,
            CompositorBindingScope::Global,
            CompositorBindingScope::Global,
        ]
    }
}

pub(crate) fn compositor_binding_action(
    st: &Halley,
    key_code: u32,
    mods: &ModState,
) -> Option<CompositorBindingAction> {
    for scope in active_binding_scopes(st) {
        for binding in &st.runtime.tuning.compositor_bindings {
            if binding.scope == scope
                && input_matches_binding(key_code, binding.key)
                && modifier_exact(mods, binding.modifiers)
            {
                return Some(binding.action.clone());
            }
        }
    }

    None
}

pub(crate) fn compositor_binding_action_active(
    st: &Halley,
    key_code: u32,
    mods: &ModState,
) -> Option<CompositorBindingAction> {
    compositor_binding_action(st, key_code, mods)
}

pub(crate) fn modifiers_keep_focus_cycle_session_active(st: &Halley, mods: &ModState) -> bool {
    st.runtime.tuning.compositor_bindings.iter().any(|binding| {
        matches!(binding.action, CompositorBindingAction::FocusCycle(_))
            && modifier_exact(mods, binding.modifiers)
    })
}

pub(crate) fn key_is_compositor_binding(st: &Halley, key_code: u32, mods: &ModState) -> bool {
    compositor_binding_action(st, key_code, mods).is_some()
        || st.runtime.tuning.launch_bindings.iter().any(|binding| {
            input_matches_binding(key_code, binding.key) && modifier_exact(mods, binding.modifiers)
        })
}

pub(crate) fn compositor_action_allows_repeat(action: CompositorBindingAction) -> bool {
    matches!(
        action,
        CompositorBindingAction::Node(NodeBindingAction::Move(_))
            | CompositorBindingAction::FocusCycle(FocusCycleBindingAction::Forward)
            | CompositorBindingAction::FocusCycle(FocusCycleBindingAction::Backward)
            | CompositorBindingAction::Stack(StackBindingAction::Cycle(_))
            | CompositorBindingAction::Tile(TileBindingAction::Focus(_))
            | CompositorBindingAction::Cluster(ClusterBindingAction::LayoutCycle)
            | CompositorBindingAction::Trail(TrailBindingAction::Prev)
            | CompositorBindingAction::Trail(TrailBindingAction::Next)
            | CompositorBindingAction::ZoomIn
            | CompositorBindingAction::ZoomOut
    )
}

pub(crate) fn apply_compositor_action_press(
    st: &mut Halley,
    action: CompositorBindingAction,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    match action {
        CompositorBindingAction::Quit { .. } => {
            exit_confirm_controller(st).show();
            info!("quit requested via keybind");
            true
        }
        CompositorBindingAction::Reload => {
            let aperture_path = crate::aperture::default_aperture_config_path();
            let _ = crate::aperture::reload_aperture_config(st, aperture_path.as_path(), "manual");
            if let Some(next) = RuntimeTuning::try_load_from_path(config_path) {
                crate::bootstrap::apply_reloaded_tuning(
                    st,
                    next,
                    config_path,
                    wayland_display,
                    "manual",
                );
                debug!("manual config reload from {}", config_path);
                debug!(
                    "resolved keybinds: {}",
                    st.runtime.tuning.keybinds_resolved_summary()
                );
                debug!(
                    "resolved zoom: {}",
                    st.runtime.tuning.zoom_resolved_summary()
                );
            } else {
                warn!(
                    "manual reload skipped for {} because config parse/load failed",
                    config_path
                );
            }
            true
        }
        CompositorBindingAction::OpenTerminal => spawn_open_terminal_binding(st, wayland_display),
        CompositorBindingAction::ToggleState => {
            if st.has_active_cluster_workspace() {
                st.collapse_active_cluster_workspace(std::time::Instant::now())
            } else {
                toggle_focused_active_node_state(st)
            }
        }
        CompositorBindingAction::MaximizeFocusedWindow => toggle_focused_maximize_node_state(st),
        CompositorBindingAction::ToggleFocusedPin => toggle_focused_pin_state(st),
        CompositorBindingAction::CloseFocusedWindow => request_close_focused_toplevel(st),
        CompositorBindingAction::ClusterMode => st.enter_cluster_mode(),
        CompositorBindingAction::FocusCycle(direction) => {
            st.start_or_step_focus_cycle(direction, Instant::now())
        }
        CompositorBindingAction::Node(NodeBindingAction::Move(direction)) => {
            move_latest_node_direction(st, from_directional_action(direction))
        }
        CompositorBindingAction::Stack(StackBindingAction::Cycle(direction)) => {
            let direction = match direction {
                StackCycleDirection::Forward => {
                    halley_core::cluster_layout::ClusterCycleDirection::Next
                }
                StackCycleDirection::Backward => {
                    halley_core::cluster_layout::ClusterCycleDirection::Prev
                }
            };
            let monitor = st.focused_monitor().to_string();
            st.cycle_active_stack_for_monitor(monitor.as_str(), direction, Instant::now())
        }
        CompositorBindingAction::Tile(TileBindingAction::Focus(direction)) => {
            let monitor = st.focused_monitor().to_string();
            st.tile_focus_active_cluster_member_for_monitor(
                monitor.as_str(),
                direction,
                Instant::now(),
            )
        }
        CompositorBindingAction::Tile(TileBindingAction::Swap(direction)) => {
            let monitor = st.focused_monitor().to_string();
            st.tile_swap_active_cluster_member_for_monitor(
                monitor.as_str(),
                direction,
                Instant::now(),
            )
        }
        CompositorBindingAction::Cluster(ClusterBindingAction::LayoutCycle) => {
            let monitor = st.focused_monitor().to_string();
            st.cycle_active_cluster_layout_for_monitor(monitor.as_str(), Instant::now())
        }
        CompositorBindingAction::Cluster(ClusterBindingAction::Slot(slot)) => {
            st.activate_cluster_slot_on_current_monitor(slot, Instant::now())
        }
        CompositorBindingAction::Trail(TrailBindingAction::Prev) => {
            crate::compositor::actions::window::step_window_trail(
                st,
                halley_ipc::TrailDirection::Prev,
            )
        }
        CompositorBindingAction::Trail(TrailBindingAction::Next) => {
            crate::compositor::actions::window::step_window_trail(
                st,
                halley_ipc::TrailDirection::Next,
            )
        }
        CompositorBindingAction::Monitor(MonitorBindingAction::Focus(target)) => {
            let target = match target {
                MonitorBindingTarget::Direction(DirectionalAction::Left) => {
                    halley_ipc::MonitorFocusTarget::Direction(
                        halley_ipc::MonitorFocusDirection::Left,
                    )
                }
                MonitorBindingTarget::Direction(DirectionalAction::Right) => {
                    halley_ipc::MonitorFocusTarget::Direction(
                        halley_ipc::MonitorFocusDirection::Right,
                    )
                }
                MonitorBindingTarget::Direction(DirectionalAction::Up) => {
                    halley_ipc::MonitorFocusTarget::Direction(halley_ipc::MonitorFocusDirection::Up)
                }
                MonitorBindingTarget::Direction(DirectionalAction::Down) => {
                    halley_ipc::MonitorFocusTarget::Direction(
                        halley_ipc::MonitorFocusDirection::Down,
                    )
                }
                MonitorBindingTarget::Output(output) => {
                    halley_ipc::MonitorFocusTarget::Output(output)
                }
            };
            matches!(
                crate::ipc::handle_request(
                    st,
                    halley_ipc::Request::Monitor(halley_ipc::MonitorRequest::Focus(target)),
                ),
                halley_ipc::Response::Ok
            )
        }
        CompositorBindingAction::Bearings(BearingsBindingAction::Show) => {
            st.ui.render_state.set_bearings_visible(true)
        }
        CompositorBindingAction::Bearings(BearingsBindingAction::Toggle) => {
            st.ui.render_state.toggle_bearings_visible();
            true
        }
        CompositorBindingAction::ZoomIn => {
            if camera_controller(&*st).zoom_blocked_by_interaction() {
                return false;
            }
            camera_controller(st).zoom_by_steps(1.0);
            true
        }
        CompositorBindingAction::ZoomOut => {
            if camera_controller(&*st).zoom_blocked_by_interaction() {
                return false;
            }
            camera_controller(st).zoom_by_steps(-1.0);
            true
        }
        CompositorBindingAction::ZoomReset => {
            if camera_controller(&*st).zoom_blocked_by_interaction() {
                return false;
            }
            camera_controller(st).reset_zoom();
            true
        }
    }
}

pub(crate) fn apply_compositor_action_release(
    st: &mut Halley,
    action: CompositorBindingAction,
) -> bool {
    match action {
        CompositorBindingAction::Bearings(BearingsBindingAction::Show) => {
            st.ui.render_state.set_bearings_visible(false)
        }
        _ => false,
    }
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
            CompositorBindingAction::Node(NodeBindingAction::Move(_))
            | CompositorBindingAction::Reload
            | CompositorBindingAction::OpenTerminal
            | CompositorBindingAction::ToggleState
            | CompositorBindingAction::MaximizeFocusedWindow
            | CompositorBindingAction::ToggleFocusedPin
            | CompositorBindingAction::CloseFocusedWindow
            | CompositorBindingAction::ClusterMode
            | CompositorBindingAction::FocusCycle(_)
            | CompositorBindingAction::Stack(_)
            | CompositorBindingAction::Tile(_)
            | CompositorBindingAction::Cluster(_)
            | CompositorBindingAction::Trail(TrailBindingAction::Prev)
            | CompositorBindingAction::Trail(TrailBindingAction::Next)
            | CompositorBindingAction::Monitor(_)
            | CompositorBindingAction::Bearings(_)
            | CompositorBindingAction::Quit { .. }
            | CompositorBindingAction::ZoomIn
            | CompositorBindingAction::ZoomOut
            | CompositorBindingAction::ZoomReset => {
                apply_compositor_action_press(st, action, config_path, wayland_display)
            }
        };
    }

    for binding in st.runtime.tuning.launch_bindings.clone() {
        if input_matches_binding(key_code, binding.key) && modifier_exact(mods, binding.modifiers) {
            return spawn_launch_binding(st, binding.command.as_str(), wayland_display);
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

    for binding in st.runtime.tuning.launch_bindings.clone() {
        if input_matches_binding(key_code, binding.key) && modifier_exact(mods, binding.modifiers) {
            return spawn_launch_binding(st, binding.command.as_str(), wayland_display);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{compositor_action_allows_repeat, input_matches_binding};
    use halley_config::WHEEL_UP_CODE;
    use halley_config::keybinds::key_name_to_evdev;
    use halley_config::{CompositorBindingAction, TrailBindingAction};

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

    #[test]
    fn repeat_policy_is_limited_to_safe_actions() {
        assert!(compositor_action_allows_repeat(
            CompositorBindingAction::ZoomIn
        ));
        assert!(compositor_action_allows_repeat(
            CompositorBindingAction::Trail(TrailBindingAction::Next,)
        ));
        assert!(!compositor_action_allows_repeat(
            CompositorBindingAction::CloseFocusedWindow
        ));
        assert!(!compositor_action_allows_repeat(
            CompositorBindingAction::ToggleState
        ));
        assert!(!compositor_action_allows_repeat(
            CompositorBindingAction::MaximizeFocusedWindow
        ));
        assert!(!compositor_action_allows_repeat(
            CompositorBindingAction::OpenTerminal
        ));
    }
}
