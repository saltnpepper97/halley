use std::cmp::Ordering;

use halley_ipc::{IpcError, MonitorFocusDirection};

use crate::compositor::root::Halley;

#[derive(Clone, Debug)]
struct IpcMonitorView {
    name: String,
    offset_x: i32,
    offset_y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Debug)]
pub(super) struct IpcView {
    focused_monitor: String,
    monitors: Vec<IpcMonitorView>,
}

impl IpcView {
    pub(super) fn from_halley(st: &Halley) -> Self {
        let mut monitors: Vec<_> = st
            .model
            .monitor_state
            .monitors
            .iter()
            .map(|(name, monitor)| IpcMonitorView {
                name: name.clone(),
                offset_x: monitor.offset_x,
                offset_y: monitor.offset_y,
                width: monitor.width,
                height: monitor.height,
            })
            .collect();
        monitors.sort_by(|a, b| monitor_order(a, b));
        Self {
            focused_monitor: st.focused_monitor().to_string(),
            monitors,
        }
    }

    pub(super) fn focused_monitor(&self) -> &str {
        self.focused_monitor.as_str()
    }

    pub(super) fn validate_output(&self, output: &str) -> Result<(), IpcError> {
        self.monitors
            .iter()
            .any(|monitor| monitor.name == output)
            .then_some(())
            .ok_or_else(|| IpcError::NotFound(format!("output {output} not found")))
    }

    pub(super) fn resolve_output_context(&self, output: Option<&str>) -> Result<String, IpcError> {
        match output {
            Some(name) => {
                self.validate_output(name)?;
                Ok(name.to_string())
            }
            None => Ok(self.focused_monitor.clone()),
        }
    }

    pub(super) fn sorted_outputs(&self) -> Vec<String> {
        self.monitors
            .iter()
            .map(|monitor| monitor.name.clone())
            .collect()
    }

    pub(super) fn adjacent_monitor(&self, direction: MonitorFocusDirection) -> Option<String> {
        let current = self
            .monitors
            .iter()
            .find(|monitor| monitor.name == self.focused_monitor)?;
        let current_center = monitor_center(current);

        let mut candidates: Vec<(String, f32, f32)> = self
            .monitors
            .iter()
            .filter_map(|monitor| {
                if monitor.name == current.name {
                    return None;
                }
                let center = monitor_center(monitor);
                let dx = center.0 - current_center.0;
                let dy = center.1 - current_center.1;
                let (primary, secondary, keep) = match direction {
                    MonitorFocusDirection::Left => (-dx, dy.abs(), dx < 0.0),
                    MonitorFocusDirection::Right => (dx, dy.abs(), dx > 0.0),
                    MonitorFocusDirection::Up => (-dy, dx.abs(), dy < 0.0),
                    MonitorFocusDirection::Down => (dy, dx.abs(), dy > 0.0),
                };
                keep.then_some((monitor.name.clone(), primary, secondary))
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
}

fn monitor_order(a: &IpcMonitorView, b: &IpcMonitorView) -> Ordering {
    a.offset_x
        .cmp(&b.offset_x)
        .then(a.offset_y.cmp(&b.offset_y))
        .then(a.name.cmp(&b.name))
}

fn monitor_center(monitor: &IpcMonitorView) -> (f32, f32) {
    (
        monitor.offset_x as f32 + monitor.width as f32 * 0.5,
        monitor.offset_y as f32 + monitor.height as f32 * 0.5,
    )
}
