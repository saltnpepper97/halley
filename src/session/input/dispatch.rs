//! Translate configured actions into session commands.

use std::ffi::OsStr;

use halley_config::Action;
use halley_core::camera::Camera;

use crate::session::{environment, spawn};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionControl {
    Continue,
    Quit,
    CloseFocusedWindow,
    Screenshot,
    ToggleFullscreen,
    ToggleFieldMaximize,
    ToggleState,
    ToggleFocusedPin,
    Apogee,
    FocusCycle(halley_config::FocusCycleDirection),
    Trail(halley_config::TrailDirection),
    FocusDirection(halley_config::Direction),
    MoveNode(halley_config::Direction),
    MoveNodeBy {
        direction: halley_config::Direction,
        distance: u32,
    },
    TransferWindow(halley_config::Direction),
    PanField(halley_config::Direction),
    ResizeWindow(halley_config::Direction),
    ArrangeVisible,
    UndoArrange,
    CenterLastFocused,
    BearingsShow,
    BearingsToggle,
    ClusterMode,
    ClusterLayoutCycle,
    ClusterToggleFloat,
    ClusterSlot(u8),
    ClusterTileFocus(halley_config::Direction),
    ClusterTileSwap(halley_config::Direction),
    MonitorFocus(halley_config::MonitorTarget),
    Reload,
}

#[derive(Clone, Copy)]
pub(super) struct SpawnContext<'a> {
    pub(super) socket_name: &'a OsStr,
    pub(super) x11_display: Option<&'a OsStr>,
    pub(super) cursor_size: u8,
    pub(super) environment: &'a environment::LaunchEnvironment,
}

/// Interprets every configured action once for both session backends.
/// Backends provide the camera selected by their own output routing and
/// translate the returned quit request into their loop's native mechanism.
pub(super) fn dispatch_action(
    action: Action,
    terminal_command: Option<&str>,
    spawn_context: SpawnContext<'_>,
    camera: Option<&mut Camera>,
    zoom: &halley_config::Zoom,
) -> SessionControl {
    match action {
        Action::Quit => return SessionControl::Quit,
        Action::CloseFocusedWindow => return SessionControl::CloseFocusedWindow,
        Action::ToggleFullscreen => return SessionControl::ToggleFullscreen,
        Action::ToggleFieldMaximize => return SessionControl::ToggleFieldMaximize,
        Action::ToggleState => return SessionControl::ToggleState,
        Action::ToggleFocusedPin => return SessionControl::ToggleFocusedPin,
        Action::Apogee => return SessionControl::Apogee,
        Action::FocusCycle(direction) => return SessionControl::FocusCycle(direction),
        Action::Trail(direction) => return SessionControl::Trail(direction),
        Action::FocusDirection(direction) => return SessionControl::FocusDirection(direction),
        Action::TransferWindow(direction) => return SessionControl::TransferWindow(direction),
        Action::PanField(direction) => return SessionControl::PanField(direction),
        Action::MoveNode(direction) => return SessionControl::MoveNode(direction),
        Action::MoveNodeBy {
            direction,
            distance,
        } => {
            return SessionControl::MoveNodeBy {
                direction,
                distance,
            };
        }
        Action::ResizeWindow(direction) => return SessionControl::ResizeWindow(direction),
        Action::ArrangeVisible => return SessionControl::ArrangeVisible,
        Action::UndoArrange => return SessionControl::UndoArrange,
        Action::PointerMoveWindow
        | Action::PointerResizeWindow
        | Action::PointerPanField
        | Action::PointerPanFieldAlways
        | Action::PointerDragPan => {
            eventline::warn!("keybinds: pointer grab action used outside a pointer-button binding")
        }
        Action::CenterLastFocused => return SessionControl::CenterLastFocused,
        Action::ClusterMode => return SessionControl::ClusterMode,
        Action::ClusterLayoutCycle => return SessionControl::ClusterLayoutCycle,
        Action::ClusterToggleFloat => return SessionControl::ClusterToggleFloat,
        Action::ClusterSlot(slot) => return SessionControl::ClusterSlot(slot),
        Action::ClusterTileFocus(direction) => return SessionControl::ClusterTileFocus(direction),
        Action::ClusterTileSwap(direction) => return SessionControl::ClusterTileSwap(direction),
        Action::MonitorFocus(direction) => return SessionControl::MonitorFocus(direction),
        Action::Reload => return SessionControl::Reload,
        Action::BearingsShow => return SessionControl::BearingsShow,
        Action::BearingsToggle => return SessionControl::BearingsToggle,
        Action::OpenTerminal => match terminal_command {
            Some(command) => spawn::spawn_detached(
                command,
                spawn_context.socket_name,
                spawn_context.x11_display,
                spawn_context.cursor_size,
                spawn_context.environment,
            ),
            None => eventline::warn!("keybinds: no terminal configured or found on PATH"),
        },
        Action::ZoomOut => {
            if let Some(camera) = camera {
                crate::input::zoom::zoom_out(camera, zoom);
            }
        }
        Action::ZoomIn => {
            if let Some(camera) = camera {
                crate::input::zoom::zoom_in(camera, zoom);
            }
        }
        Action::ZoomReset => {
            if let Some(camera) = camera {
                camera.reset_zoom_target();
            }
        }
        Action::Screenshot => return SessionControl::Screenshot,
        Action::Spawn(command) => spawn::spawn_detached(
            &command,
            spawn_context.socket_name,
            spawn_context.x11_display,
            spawn_context.cursor_size,
            spawn_context.environment,
        ),
    }
    SessionControl::Continue
}
