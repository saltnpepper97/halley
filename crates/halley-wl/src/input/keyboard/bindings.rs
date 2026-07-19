use eventline::{debug, warn};

use super::modkeys::{key_matches, modifier_exact};
use crate::compositor::actions::window::{
    move_latest_node_direction, toggle_focused_active_node_state,
    toggle_focused_fullscreen_node_state, toggle_focused_maximize_node_state,
    toggle_focused_pin_state,
};
use crate::compositor::exit_confirm;
use crate::compositor::interaction::ModState;
use crate::compositor::root::Halley;
use crate::compositor::surface::request_close_focused_toplevel;
use halley_api::{
    MonitorFocusDirection, MonitorFocusTarget, MonitorRequest, NodeMoveDirection, Request,
    Response, TrailDirection,
};
use halley_config::keybinds::{is_pointer_button_code, is_wheel_code};
use halley_config::{
    BearingsBindingAction, ClusterBindingAction, CompositorBindingAction, CompositorBindingScope,
    DirectionalAction, FocusCycleBindingAction, MonitorBindingAction, MonitorBindingTarget,
    NodeBindingAction, RuntimeTuning, StackBindingAction, StackCycleDirection, TileBindingAction,
    TrailBindingAction,
};
use smithay::input::pointer::CursorIcon;
use std::time::Instant;

fn spawn_launch_binding(st: &mut Halley, command: &str, wayland_display: &str) -> bool {
    if st.input.interaction_state.apogee_session.is_some() {
        st.close_apogee(Instant::now());
    }
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
    if st.input.interaction_state.apogee_session.is_some() {
        st.close_apogee(Instant::now());
    }
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
            | CompositorBindingAction::Focus(_)
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

fn compositor_action_hides_cursor(action: &CompositorBindingAction) -> bool {
    !matches!(
        action,
        CompositorBindingAction::ZoomIn
            | CompositorBindingAction::ZoomOut
            | CompositorBindingAction::ZoomReset
    )
}

fn show_zoom_cursor(st: &mut Halley, icon: CursorIcon) {
    st.input.interaction_state.cursor_hidden_by_typing = false;
    st.input.interaction_state.cursor_hidden_by_keyboard_nav = false;
    let now = Instant::now();
    let now_ms = st.now_ms(now);
    let _ = crate::compositor::interaction::state::note_cursor_activity(st, now_ms);
    crate::compositor::interaction::pointer::set_temporary_cursor_override_icon(st, icon, now, 240);
}

pub(crate) fn apply_compositor_action_press(
    st: &mut Halley,
    action: CompositorBindingAction,
    config_path: &str,
    wayland_display: &str,
) -> bool {
    if st.input.interaction_state.apogee_session.is_some()
        && !apogee_allows_compositor_action(&action)
    {
        return true;
    }

    // Navigation keybinds hide the cursor image, but zoom is spatial and should
    // keep the pointer visible as feedback for the monitor being zoomed.
    if compositor_action_hides_cursor(&action) {
        crate::compositor::interaction::state::mark_cursor_hidden_by_keyboard_nav(st);
    }

    match action {
        CompositorBindingAction::Quit { .. } => {
            exit_confirm::show(st);
            debug!("quit requested via keybind");
            true
        }
        CompositorBindingAction::Reload => {
            let aperture_path = crate::aperture::default_aperture_config_path();
            let _ = crate::aperture::reload_aperture_config(st, aperture_path.as_path(), "manual");
            match RuntimeTuning::try_load_from_path_diagnostic(config_path) {
                Ok(next) => {
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
                }
                Err(err) => {
                    crate::bootstrap::show_config_reload_error(st, &err);
                    warn!(
                        "manual reload skipped for {} because {}",
                        config_path, err.message
                    );
                }
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
        CompositorBindingAction::ToggleFullscreen => toggle_focused_fullscreen_node_state(st),
        CompositorBindingAction::ToggleFocusedPin => toggle_focused_pin_state(st),
        CompositorBindingAction::CloseFocusedWindow => request_close_focused_toplevel(st),
        CompositorBindingAction::ClusterMode => st.enter_cluster_mode(),
        CompositorBindingAction::Apogee => st.toggle_apogee(Instant::now()),
        CompositorBindingAction::Screenshot => {
            crate::compositor::screenshot::start_screenshot_session(
                st,
                halley_api::CaptureMode::Menu,
                None,
                Instant::now(),
            )
        }
        CompositorBindingAction::Focus(direction) => {
            crate::compositor::focus::directional::focus_directional(st, direction, Instant::now())
        }
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
            crate::compositor::actions::window::step_window_trail(st, TrailDirection::Prev)
        }
        CompositorBindingAction::Trail(TrailBindingAction::Next) => {
            crate::compositor::actions::window::step_window_trail(st, TrailDirection::Next)
        }
        CompositorBindingAction::Monitor(MonitorBindingAction::Focus(target)) => {
            let target = match target {
                MonitorBindingTarget::Direction(DirectionalAction::Left) => {
                    MonitorFocusTarget::Direction(MonitorFocusDirection::Left)
                }
                MonitorBindingTarget::Direction(DirectionalAction::Right) => {
                    MonitorFocusTarget::Direction(MonitorFocusDirection::Right)
                }
                MonitorBindingTarget::Direction(DirectionalAction::Up) => {
                    MonitorFocusTarget::Direction(MonitorFocusDirection::Up)
                }
                MonitorBindingTarget::Direction(DirectionalAction::Down) => {
                    MonitorFocusTarget::Direction(MonitorFocusDirection::Down)
                }
                MonitorBindingTarget::Output(output) => MonitorFocusTarget::Output(output),
            };
            matches!(
                crate::ipc::handle_request(st, Request::Monitor(MonitorRequest::Focus(target)),),
                Response::Ok
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
            if crate::compositor::monitor::camera::zoom_blocked_by_interaction(&*st) {
                return false;
            }
            crate::compositor::monitor::camera::zoom_by_steps(st, 1.0);
            show_zoom_cursor(st, CursorIcon::ZoomIn);
            true
        }
        CompositorBindingAction::ZoomOut => {
            if crate::compositor::monitor::camera::zoom_blocked_by_interaction(&*st) {
                return false;
            }
            crate::compositor::monitor::camera::zoom_by_steps(st, -1.0);
            show_zoom_cursor(st, CursorIcon::ZoomOut);
            true
        }
        CompositorBindingAction::ZoomReset => {
            if crate::compositor::monitor::camera::zoom_blocked_by_interaction(&*st) {
                return false;
            }
            crate::compositor::monitor::camera::reset_zoom(st);
            show_zoom_cursor(st, CursorIcon::Default);
            true
        }
        CompositorBindingAction::CenterLastFocused => center_on_last_focused(st),
    }
}

/// Pan the camera back to centre on the last focused node — a quick "go back" after
/// wandering the field. Uses the live interaction focus, falling back to the monitor's
/// last focused surface node. Mirrors the focus-and-pan path used by Apogee selection
/// (`set_interaction_focus` + `set_pan_restore_focus_target` + `animate_viewport_center_to`).
fn center_on_last_focused(st: &mut Halley) -> bool {
    let now = Instant::now();
    let monitor = st.focused_monitor().to_string();
    let Some(node_id) = st
        .model
        .focus_state
        .primary_interaction_focus
        .or_else(|| st.last_focused_surface_node_for_monitor(monitor.as_str()))
    else {
        return false;
    };
    let Some(pos) = st.model.field.node(node_id).map(|node| node.pos) else {
        return false;
    };
    let node_monitor = st.monitor_for_node_or_current(node_id);
    if st.focused_monitor() != node_monitor {
        st.focus_monitor_view(node_monitor.as_str(), now);
    }
    st.set_interaction_focus(Some(node_id), 30_000, now);
    st.set_pan_restore_focus_target(node_id);
    st.animate_viewport_center_to(pos, now)
}

fn apogee_allows_compositor_action(action: &CompositorBindingAction) -> bool {
    matches!(
        action,
        CompositorBindingAction::Apogee
            | CompositorBindingAction::OpenTerminal
            | CompositorBindingAction::Reload
            | CompositorBindingAction::Quit { .. }
    )
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
            | CompositorBindingAction::ToggleFullscreen
            | CompositorBindingAction::ToggleFocusedPin
            | CompositorBindingAction::CloseFocusedWindow
            | CompositorBindingAction::ClusterMode
            | CompositorBindingAction::Apogee
            | CompositorBindingAction::Screenshot
            | CompositorBindingAction::CenterLastFocused
            | CompositorBindingAction::Focus(_)
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
            if command_is_launcher(binding.command.as_str()) {
                return toggle_launcher(st, binding.command.as_str(), wayland_display);
            }
            return spawn_launch_binding(st, binding.command.as_str(), wayland_display);
        }
    }
    false
}

/// The first-party launcher binary. A launch binding whose program resolves to this is
/// treated as a toggle rather than a plain spawn, so the bound key opens the launcher when
/// closed and dismisses it when open — without ever doing both (which would race the
/// single-instance guard).
const LAUNCHER_PROGRAM: &str = "halley-lift";

/// Returns true when `command`'s program (first shell token, basename) is the launcher.
/// Handles arguments (`halley-lift cluster`) and absolute paths (`/usr/bin/halley-lift`).
fn command_is_launcher(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .map(|program| program.rsplit('/').next().unwrap_or(program))
        .is_some_and(|program| program == LAUNCHER_PROGRAM)
}

/// Toggle the launcher: if a Lift overlay is currently open, close it and do not spawn;
/// otherwise spawn it. Doing exactly one of the two keeps the bound key deterministic.
fn toggle_launcher(st: &mut Halley, command: &str, wayland_display: &str) -> bool {
    if crate::compositor::monitor::layer_shell::close_any_lift_layer(st) {
        return true;
    }
    spawn_launch_binding(st, command, wayland_display)
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
    use super::{
        apply_compositor_action_press, command_is_launcher, compositor_action_allows_repeat,
        input_matches_binding,
    };
    use halley_config::WHEEL_UP_CODE;
    use halley_config::keybinds::key_name_to_evdev;
    use halley_config::{CompositorBindingAction, TrailBindingAction};

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        tuning
    }

    #[test]
    fn matcher_accepts_direct_wheel_codes() {
        assert!(input_matches_binding(WHEEL_UP_CODE, WHEEL_UP_CODE));
    }

    #[test]
    fn launcher_command_is_recognized_with_args_and_paths() {
        assert!(command_is_launcher("halley-lift"));
        assert!(command_is_launcher("halley-lift cluster"));
        assert!(command_is_launcher("/usr/bin/halley-lift"));
        assert!(command_is_launcher("/usr/local/bin/halley-lift apps"));
        assert!(!command_is_launcher("halley-lift-extra"));
        assert!(!command_is_launcher("fuzzel"));
        assert!(!command_is_launcher(""));
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

    #[test]
    fn keyboard_zoom_uses_current_interaction_monitor_not_stale_focused_monitor() {
        let dh =
            smithay::reexports::wayland_server::Display::<crate::compositor::root::Halley>::new()
                .expect("display")
                .handle();
        let mut state = crate::compositor::root::Halley::new_for_test(&dh, two_monitor_tuning());
        assert!(state.activate_monitor("right"));
        state.set_focused_monitor("left");
        state.set_interaction_monitor("right");

        assert!(apply_compositor_action_press(
            &mut state,
            CompositorBindingAction::ZoomOut,
            "",
            "wayland-test"
        ));

        assert_eq!(state.model.monitor_state.current_monitor, "right");
        assert!(state.model.zoom_log_vel > 0.0);
        assert_eq!(
            state
                .model
                .monitor_state
                .monitors
                .get("left")
                .expect("left monitor")
                .zoom_log_vel,
            0.0
        );
        assert!(!state.input.interaction_state.cursor_hidden_by_keyboard_nav);
        assert_eq!(
            crate::compositor::interaction::cursor::effective_override(&state),
            Some(smithay::input::pointer::CursorIcon::ZoomOut)
        );
    }
}
