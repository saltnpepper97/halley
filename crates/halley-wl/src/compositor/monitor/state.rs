use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;

use smithay::{
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource},
    utils::Transform,
};

use crate::compositor::root::Halley;
use crate::compositor::spawn::state::MonitorSpawnState;

#[derive(Clone, Debug)]
pub(crate) struct MonitorSpace {
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: i32,
    pub height: i32,
    pub viewport: Viewport,
    pub usable_viewport: Viewport,
    pub zoom_ref_size: Vec2,
    pub camera_target_center: Vec2,
    pub camera_target_view_size: Vec2,
}

pub(crate) struct MonitorState {
    pub(crate) outputs: HashMap<String, Output>,
    pub(crate) current_monitor: String,
    pub(crate) interaction_monitor: String,
    pub(crate) focused_monitor: String,
    pub(crate) monitors: HashMap<String, MonitorSpace>,
    pub(crate) node_monitor: HashMap<NodeId, String>,
    pub(crate) layer_surface_monitor: HashMap<ObjectId, String>,
    pub(crate) layer_keyboard_focus: Option<ObjectId>,
}

fn preferred_monitor_name(monitors: &HashMap<String, MonitorSpace>) -> Option<String> {
    monitors
        .iter()
        .min_by(|a, b| {
            let (_, am) = a;
            let (_, bm) = b;
            am.offset_x
                .cmp(&bm.offset_x)
                .then(am.offset_y.cmp(&bm.offset_y))
                .then(a.0.cmp(b.0))
        })
        .map(|(name, _)| name.clone())
}

impl Halley {
    pub fn view_center_for_monitor(&self, monitor: &str) -> Vec2 {
        self.usable_viewport_for_monitor(monitor).center
    }

    pub fn usable_viewport_for_monitor(&self, monitor: &str) -> Viewport {
        let is_cluster = self.cluster_mode_active_for_monitor(monitor);

        if self.model.monitor_state.current_monitor == monitor {
            if !is_cluster {
                return self.model.viewport;
            }
            self.model
                .monitor_state
                .monitors
                .get(monitor)
                .map(|space| {
                    if space.usable_viewport == space.viewport {
                        return self.model.viewport;
                    }
                    let full = space.viewport;
                    let usable = space.usable_viewport;
                    let full_left = full.center.x - full.size.x * 0.5;
                    let full_right = full.center.x + full.size.x * 0.5;
                    let full_top = full.center.y - full.size.y * 0.5;
                    let full_bottom = full.center.y + full.size.y * 0.5;
                    let usable_left = usable.center.x - usable.size.x * 0.5;
                    let usable_right = usable.center.x + usable.size.x * 0.5;
                    let usable_top = usable.center.y - usable.size.y * 0.5;
                    let usable_bottom = usable.center.y + usable.size.y * 0.5;
                    let left_frac = (usable_left - full_left) / full.size.x.max(1.0);
                    let right_frac = (full_right - usable_right) / full.size.x.max(1.0);
                    let top_frac = (usable_top - full_top) / full.size.y.max(1.0);
                    let bottom_frac = (full_bottom - usable_bottom) / full.size.y.max(1.0);
                    let live = self.model.viewport;
                    let live_left = live.center.x - live.size.x * 0.5 + live.size.x * left_frac;
                    let live_right = live.center.x + live.size.x * 0.5 - live.size.x * right_frac;
                    let live_top = live.center.y - live.size.y * 0.5 + live.size.y * top_frac;
                    let live_bottom = live.center.y + live.size.y * 0.5 - live.size.y * bottom_frac;
                    Viewport::new(
                        Vec2 {
                            x: (live_left + live_right) * 0.5,
                            y: (live_top + live_bottom) * 0.5,
                        },
                        Vec2 {
                            x: (live_right - live_left).max(1.0),
                            y: (live_bottom - live_top).max(1.0),
                        },
                    )
                })
                .unwrap_or(self.model.viewport)
        } else {
            self.model
                .monitor_state
                .monitors
                .get(monitor)
                .map(|space| {
                    if is_cluster {
                        space.usable_viewport
                    } else {
                        space.viewport
                    }
                })
                .unwrap_or(self.model.viewport)
        }
    }

    pub(crate) fn load_monitor_state(&mut self, name: &str) -> bool {
        let Some(space) = self.model.monitor_state.monitors.get(name).cloned() else {
            return false;
        };
        self.model.monitor_state.current_monitor = name.to_string();
        self.model.viewport = space.viewport;
        self.model.zoom_ref_size = space.zoom_ref_size;
        self.model.camera_target_center = space.camera_target_center;
        self.model.camera_target_view_size = space.camera_target_view_size;
        true
    }

    pub(crate) fn sync_current_monitor_state(&mut self) {
        if let Some(space) = self
            .model
            .monitor_state
            .monitors
            .get_mut(&self.model.monitor_state.current_monitor)
        {
            space.viewport = self.model.viewport;
            space.zoom_ref_size = self.model.zoom_ref_size;
            space.camera_target_center = self.model.camera_target_center;
            space.camera_target_view_size = self.model.camera_target_view_size;
        }
    }

    pub(crate) fn activate_monitor(&mut self, name: &str) -> bool {
        if self.model.monitor_state.current_monitor == name {
            return self.model.monitor_state.monitors.contains_key(name);
        }
        self.sync_current_monitor_state();
        self.load_monitor_state(name)
    }

    pub(crate) fn begin_temporary_render_monitor(&mut self, name: &str) -> Option<String> {
        let previous = self.model.monitor_state.current_monitor.clone();
        if previous != name && self.activate_monitor(name) {
            Some(previous)
        } else {
            None
        }
    }

    pub(crate) fn end_temporary_render_monitor(&mut self, previous: Option<String>) {
        if let Some(previous) = previous {
            let _ = self.activate_monitor(previous.as_str());
        }
    }

    pub(crate) fn interaction_monitor(&self) -> &str {
        if self
            .model
            .monitor_state
            .monitors
            .contains_key(&self.model.monitor_state.interaction_monitor)
        {
            self.model.monitor_state.interaction_monitor.as_str()
        } else {
            self.model.monitor_state.current_monitor.as_str()
        }
    }

    pub(crate) fn focused_monitor(&self) -> &str {
        if self
            .model
            .monitor_state
            .monitors
            .contains_key(&self.model.monitor_state.focused_monitor)
        {
            self.model.monitor_state.focused_monitor.as_str()
        } else {
            self.interaction_monitor()
        }
    }

    pub(crate) fn set_interaction_monitor(&mut self, name: &str) {
        if self.model.monitor_state.monitors.contains_key(name) {
            self.model.monitor_state.interaction_monitor = name.to_string();
        }
    }

    pub(crate) fn set_focused_monitor(&mut self, name: &str) {
        if self.model.monitor_state.monitors.contains_key(name) {
            self.model.monitor_state.focused_monitor = name.to_string();
        }
    }

    pub(crate) fn reconfigure_active_tty_monitors(&mut self, active_outputs: &[String]) {
        self.sync_current_monitor_state();

        let previous = self.model.monitor_state.monitors.clone();
        let mut monitors = HashMap::new();

        for viewport in self
            .runtime
            .tuning
            .tty_viewports
            .iter()
            .filter(|viewport| viewport.enabled)
            .filter(|viewport| {
                active_outputs
                    .iter()
                    .any(|name| name == &viewport.connector)
            })
        {
            let width = viewport.width.max(1) as i32;
            let height = viewport.height.max(1) as i32;
            let center = Vec2 {
                x: viewport.offset_x as f32 + width as f32 * 0.5,
                y: viewport.offset_y as f32 + height as f32 * 0.5,
            };
            let default_view = Viewport::new(
                center,
                Vec2 {
                    x: width as f32,
                    y: height as f32,
                },
            );

            let restored = previous.get(&viewport.connector);
            monitors.insert(
                viewport.connector.clone(),
                MonitorSpace {
                    offset_x: viewport.offset_x,
                    offset_y: viewport.offset_y,
                    width,
                    height,
                    viewport: restored.map(|m| m.viewport).unwrap_or(default_view),
                    usable_viewport: restored.map(|m| m.usable_viewport).unwrap_or(default_view),
                    zoom_ref_size: restored
                        .map(|m| m.zoom_ref_size)
                        .unwrap_or(default_view.size),
                    camera_target_center: restored
                        .map(|m| m.camera_target_center)
                        .unwrap_or(default_view.center),
                    camera_target_view_size: restored
                        .map(|m| m.camera_target_view_size)
                        .unwrap_or(default_view.size),
                },
            );
        }

        if monitors.is_empty() {
            let view = self.runtime.tuning.viewport();
            monitors.insert(
                "default".to_string(),
                MonitorSpace {
                    offset_x: 0,
                    offset_y: 0,
                    width: self.runtime.tuning.viewport_size.x.max(1.0).round() as i32,
                    height: self.runtime.tuning.viewport_size.y.max(1.0).round() as i32,
                    viewport: view,
                    usable_viewport: view,
                    zoom_ref_size: self.runtime.tuning.viewport_size,
                    camera_target_center: self.runtime.tuning.viewport_center,
                    camera_target_view_size: self.runtime.tuning.viewport_size,
                },
            );
        }

        self.model.monitor_state.monitors = monitors;
        crate::compositor::monitor::layer_shell::refresh_monitor_usable_viewports(self);
        self.model.spawn_state.per_monitor = self
            .model
            .monitor_state
            .monitors
            .iter()
            .map(|(name, monitor)| {
                let existing = self.model.spawn_state.per_monitor.get(name).cloned();
                (
                    name.clone(),
                    existing.unwrap_or_else(|| MonitorSpawnState::new(monitor.viewport.center)),
                )
            })
            .collect();

        if !self
            .model
            .monitor_state
            .monitors
            .contains_key(&self.model.monitor_state.current_monitor)
        {
            self.model.monitor_state.current_monitor =
                preferred_monitor_name(&self.model.monitor_state.monitors)
                    .unwrap_or_else(|| "default".to_string());
        }

        if !self
            .model
            .monitor_state
            .monitors
            .contains_key(&self.model.monitor_state.interaction_monitor)
        {
            self.model.monitor_state.interaction_monitor =
                self.model.monitor_state.current_monitor.clone();
        }
        if !self
            .model
            .monitor_state
            .monitors
            .contains_key(&self.model.monitor_state.focused_monitor)
        {
            self.model.monitor_state.focused_monitor =
                self.model.monitor_state.interaction_monitor.clone();
        }

        let current = self.model.monitor_state.current_monitor.clone();
        let _ = self.load_monitor_state(current.as_str());
    }

    pub(crate) fn monitor_for_screen(&self, sx: f32, sy: f32) -> Option<String> {
        let mut best: Option<(&String, i64)> = None;
        for (name, monitor) in &self.model.monitor_state.monitors {
            let inside = sx >= monitor.offset_x as f32
                && sx < (monitor.offset_x + monitor.width) as f32
                && sy >= monitor.offset_y as f32
                && sy < (monitor.offset_y + monitor.height) as f32;
            let dx = if sx < monitor.offset_x as f32 {
                (monitor.offset_x as f32 - sx).round() as i64
            } else if sx >= (monitor.offset_x + monitor.width) as f32 {
                (sx - (monitor.offset_x + monitor.width) as f32).round() as i64
            } else {
                0
            };
            let dy = if sy < monitor.offset_y as f32 {
                (monitor.offset_y as f32 - sy).round() as i64
            } else if sy >= (monitor.offset_y + monitor.height) as f32 {
                (sy - (monitor.offset_y + monitor.height) as f32).round() as i64
            } else {
                0
            };
            let distance = dx * dx + dy * dy;
            if inside {
                return Some(name.clone());
            }
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((name, distance));
            }
        }
        best.map(|(name, _)| name.clone())
    }

    pub(crate) fn local_screen_in_monitor(
        &self,
        name: &str,
        sx: f32,
        sy: f32,
    ) -> (i32, i32, f32, f32) {
        if let Some(monitor) = self.model.monitor_state.monitors.get(name) {
            (
                monitor.width,
                monitor.height,
                sx - monitor.offset_x as f32,
                sy - monitor.offset_y as f32,
            )
        } else {
            let w = self.runtime.tuning.viewport_size.x.max(1.0).round() as i32;
            let h = self.runtime.tuning.viewport_size.y.max(1.0).round() as i32;
            (w, h, sx, sy)
        }
    }

    pub(crate) fn node_visible_on_current_monitor(&self, id: NodeId) -> bool {
        self.model
            .monitor_state
            .node_monitor
            .get(&id)
            .is_none_or(|monitor| monitor == &self.model.monitor_state.current_monitor)
    }

    pub(crate) fn assign_node_to_current_monitor(&mut self, id: NodeId) {
        let monitor = self.model.monitor_state.current_monitor.clone();
        self.assign_node_to_monitor(id, monitor.as_str());
    }

    pub(crate) fn assign_node_to_monitor(&mut self, id: NodeId, monitor: &str) {
        let _ = self.spawn_monitor_state_mut(monitor);
        self.model
            .monitor_state
            .node_monitor
            .insert(id, monitor.to_string());
    }

    pub(crate) fn assign_layer_surface_to_monitor(&mut self, surface: &WlSurface, monitor: String) {
        self.model
            .monitor_state
            .layer_surface_monitor
            .insert(surface.id(), monitor);
    }

    pub(crate) fn output_transform_for(&self, name: &str) -> Transform {
        let degrees = self
            .runtime
            .tuning
            .tty_viewports
            .iter()
            .find(|viewport| viewport.connector == name)
            .map(|viewport| viewport.transform_degrees)
            .unwrap_or(0);
        match degrees {
            90 => Transform::_90,
            180 => Transform::_180,
            270 => Transform::_270,
            _ => Transform::Normal,
        }
    }

    pub(crate) fn advertise_output(&mut self, name: &str, mode: OutputMode) {
        let transform = self.output_transform_for(name);
        let location = self
            .model
            .monitor_state
            .monitors
            .get(name)
            .map(|monitor| (monitor.offset_x, monitor.offset_y).into())
            .unwrap_or_else(|| (0, 0).into());
        let output = self
            .model
            .monitor_state
            .outputs
            .entry(name.to_string())
            .or_insert_with(|| {
                let output = Output::new(
                    name.to_string(),
                    PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: Subpixel::Unknown,
                        make: "halley".to_string(),
                        model: name.to_string(),
                    },
                );
                let _ = output.create_global::<Halley>(&self.platform.display_handle);
                output
            });
        output.add_mode(mode);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(transform),
            Some(Scale::Integer(1)),
            Some(location),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconfigure_active_monitors_preserves_focused_monitor_when_still_present() {
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
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        state.set_interaction_monitor("left");
        state.set_focused_monitor("right");
        state.reconfigure_active_tty_monitors(&["left".to_string(), "right".to_string()]);

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.interaction_monitor(), "left");
    }

    #[test]
    fn reconfigure_active_monitors_falls_back_when_focused_monitor_disappears() {
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
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        state.set_interaction_monitor("left");
        state.set_focused_monitor("right");
        state.reconfigure_active_tty_monitors(&["left".to_string()]);

        assert_eq!(state.focused_monitor(), "left");
        assert_eq!(state.interaction_monitor(), "left");
    }
}
