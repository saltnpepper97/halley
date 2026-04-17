use std::cmp::Ordering;
use std::time::Instant;

use halley_ipc::{IpcError, MonitorFocusDirection, MonitorFocusTarget, MonitorRequest, Response};

use crate::compositor::root::Halley;

use super::validate_output;

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
    match target {
        MonitorFocusTarget::Output(output) => Ok(validate_output(st, output)?.to_string()),
        MonitorFocusTarget::Direction(direction) => {
            adjacent_monitor(st, *direction).ok_or_else(|| {
                IpcError::NotFound(format!(
                    "no {} output adjacent to {}",
                    monitor_direction_label(*direction),
                    st.focused_monitor()
                ))
            })
        }
    }
}

pub(super) fn adjacent_monitor(st: &Halley, direction: MonitorFocusDirection) -> Option<String> {
    let current_name = st.focused_monitor();
    let current = st.model.monitor_state.monitors.get(current_name)?;
    let current_center = (
        current.offset_x as f32 + current.width as f32 * 0.5,
        current.offset_y as f32 + current.height as f32 * 0.5,
    );

    let mut candidates: Vec<(String, f32, f32)> = st
        .model
        .monitor_state
        .monitors
        .iter()
        .filter_map(|(name, monitor)| {
            if name == current_name {
                return None;
            }
            let center = (
                monitor.offset_x as f32 + monitor.width as f32 * 0.5,
                monitor.offset_y as f32 + monitor.height as f32 * 0.5,
            );
            let dx = center.0 - current_center.0;
            let dy = center.1 - current_center.1;
            let (primary, secondary, keep) = match direction {
                MonitorFocusDirection::Left => (-dx, dy.abs(), dx < 0.0),
                MonitorFocusDirection::Right => (dx, dy.abs(), dx > 0.0),
                MonitorFocusDirection::Up => (-dy, dx.abs(), dy < 0.0),
                MonitorFocusDirection::Down => (dy, dx.abs(), dy > 0.0),
            };
            keep.then_some((name.clone(), primary, secondary))
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(Ordering::Equal)
            .then(a.2.partial_cmp(&b.2).unwrap_or(Ordering::Equal))
            .then(a.0.cmp(&b.0))
    });
    candidates.into_iter().next().map(|(name, _, _)| name)
}

fn monitor_direction_label(direction: MonitorFocusDirection) -> &'static str {
    match direction {
        MonitorFocusDirection::Left => "left",
        MonitorFocusDirection::Right => "right",
        MonitorFocusDirection::Up => "up",
        MonitorFocusDirection::Down => "down",
    }
}
