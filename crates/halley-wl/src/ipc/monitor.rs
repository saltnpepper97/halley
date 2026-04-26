use std::time::Instant;

use halley_ipc::{IpcError, MonitorFocusDirection, MonitorFocusTarget, MonitorRequest, Response};

use crate::compositor::root::Halley;

use super::view::IpcView;

pub(super) fn handle_monitor_request(st: &mut Halley, request: MonitorRequest) -> Response {
    match request {
        MonitorRequest::Focus(target) => match resolve_monitor_focus_target(st, &target) {
            Ok(monitor) => {
                st.focus_monitor_view(monitor.as_str(), Instant::now());
                Response::Ok
            }
            Err(err) => Response::Error(err),
        },
    }
}

fn resolve_monitor_focus_target(
    st: &Halley,
    target: &MonitorFocusTarget,
) -> Result<String, IpcError> {
    let view = IpcView::from_halley(st);
    match target {
        MonitorFocusTarget::Output(output) => {
            view.validate_output(output)?;
            Ok(output.to_string())
        }
        MonitorFocusTarget::Direction(direction) => {
            view.adjacent_monitor(*direction).ok_or_else(|| {
                IpcError::NotFound(format!(
                    "no {} output adjacent to {}",
                    monitor_direction_label(*direction),
                    view.focused_monitor()
                ))
            })
        }
    }
}

#[cfg(test)]
pub(super) fn adjacent_monitor(st: &Halley, direction: MonitorFocusDirection) -> Option<String> {
    IpcView::from_halley(st).adjacent_monitor(direction)
}

fn monitor_direction_label(direction: MonitorFocusDirection) -> &'static str {
    match direction {
        MonitorFocusDirection::Left => "left",
        MonitorFocusDirection::Right => "right",
        MonitorFocusDirection::Up => "up",
        MonitorFocusDirection::Down => "down",
    }
}
