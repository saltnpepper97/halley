use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;

use smithay::{
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{Resource, backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::Transform,
};

use crate::state::HalleyWlState;

#[derive(Clone, Debug)]
pub(crate) struct MonitorSpace {
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: i32,
    pub height: i32,
    pub viewport: Viewport,
    pub zoom_ref_size: Vec2,
    pub camera_target_center: Vec2,
    pub camera_target_view_size: Vec2,
}

pub(crate) struct MonitorState {
    pub(crate) outputs: HashMap<String, Output>,
    pub(crate) current_monitor: String,
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

impl HalleyWlState {
    pub(crate) fn load_monitor_state(&mut self, name: &str) -> bool {
        let Some(space) = self.monitor_state.monitors.get(name).cloned() else {
            return false;
        };
        self.monitor_state.current_monitor = name.to_string();
        self.viewport = space.viewport;
        self.zoom_ref_size = space.zoom_ref_size;
        self.camera_target_center = space.camera_target_center;
        self.camera_target_view_size = space.camera_target_view_size;
        true
    }

    pub(crate) fn sync_current_monitor_state(&mut self) {
        if let Some(space) = self
            .monitor_state
            .monitors
            .get_mut(&self.monitor_state.current_monitor)
        {
            space.viewport = self.viewport;
            space.zoom_ref_size = self.zoom_ref_size;
            space.camera_target_center = self.camera_target_center;
            space.camera_target_view_size = self.camera_target_view_size;
        }
    }

    pub(crate) fn activate_monitor(&mut self, name: &str) -> bool {
        if self.monitor_state.current_monitor == name {
            return self.monitor_state.monitors.contains_key(name);
        }
        self.sync_current_monitor_state();
        self.load_monitor_state(name)
    }

    pub(crate) fn reconfigure_active_tty_monitors(&mut self, active_outputs: &[String]) {
        self.sync_current_monitor_state();

        let previous = self.monitor_state.monitors.clone();
        let mut monitors = HashMap::new();

        for viewport in self
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
            let view = self.tuning.viewport();
            monitors.insert(
                "default".to_string(),
                MonitorSpace {
                    offset_x: 0,
                    offset_y: 0,
                    width: self.tuning.viewport_size.x.max(1.0).round() as i32,
                    height: self.tuning.viewport_size.y.max(1.0).round() as i32,
                    viewport: view,
                    zoom_ref_size: self.tuning.viewport_size,
                    camera_target_center: self.tuning.viewport_center,
                    camera_target_view_size: self.tuning.viewport_size,
                },
            );
        }

        self.monitor_state.monitors = monitors;

        if !self
            .monitor_state
            .monitors
            .contains_key(&self.monitor_state.current_monitor)
        {
            self.monitor_state.current_monitor =
                preferred_monitor_name(&self.monitor_state.monitors)
                    .unwrap_or_else(|| "default".to_string());
        }

        let current = self.monitor_state.current_monitor.clone();
        let _ = self.load_monitor_state(current.as_str());
    }

    pub(crate) fn monitor_for_screen(&self, sx: f32, sy: f32) -> Option<String> {
        let mut best: Option<(&String, i64)> = None;
        for (name, monitor) in &self.monitor_state.monitors {
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
        if let Some(monitor) = self.monitor_state.monitors.get(name) {
            (
                monitor.width,
                monitor.height,
                sx - monitor.offset_x as f32,
                sy - monitor.offset_y as f32,
            )
        } else {
            let w = self.tuning.viewport_size.x.max(1.0).round() as i32;
            let h = self.tuning.viewport_size.y.max(1.0).round() as i32;
            (w, h, sx, sy)
        }
    }

    pub(crate) fn node_visible_on_current_monitor(&self, id: NodeId) -> bool {
        self.monitor_state
            .node_monitor
            .get(&id)
            .is_none_or(|monitor| monitor == &self.monitor_state.current_monitor)
    }

    pub(crate) fn assign_node_to_current_monitor(&mut self, id: NodeId) {
        self.monitor_state
            .node_monitor
            .insert(id, self.monitor_state.current_monitor.clone());
    }

    pub(crate) fn assign_layer_surface_to_monitor(&mut self, surface: &WlSurface, monitor: String) {
        self.monitor_state
            .layer_surface_monitor
            .insert(surface.id(), monitor);
    }

    pub(crate) fn output_transform_for(&self, name: &str) -> Transform {
        let degrees = self
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
            .monitor_state
            .monitors
            .get(name)
            .map(|monitor| (monitor.offset_x, monitor.offset_y).into())
            .unwrap_or_else(|| (0, 0).into());
        let output = self
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
                let _ = output.create_global::<HalleyWlState>(&self.display_handle);
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

    pub(crate) fn reconcile_surface_bindings(&mut self) {
        const STALE_SURFACE_GRACE_MS: u64 = 1500;
        let now = Instant::now();

        let alive: HashSet<ObjectId> = self
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .map(|t| t.wl_surface().id())
            .collect();

        let stale: Vec<ObjectId> = self
            .surface_to_node
            .keys()
            .filter(|k| !alive.contains(*k))
            .filter(|k| {
                let Some(activity) = self.surface_activity.get(*k) else {
                    return true;
                };
                now.duration_since(activity.last_commit_at()).as_millis() as u64
                    >= STALE_SURFACE_GRACE_MS
            })
            .cloned()
            .collect();

        for key in stale {
            self.surface_activity.remove(&key);
            if let Some(id) = self.surface_to_node.remove(&key) {
                if self.focus_state.pan_restore_active_focus == Some(id) {
                    self.focus_state.pan_restore_active_focus = None;
                }
                self.workspace_state.manual_collapsed_nodes.remove(&id);
                self.render_state.zoom_nominal_size.remove(&id);
                self.render_state.zoom_resize_fallback.remove(&id);
                self.render_state.zoom_resize_reject_streak.remove(&id);
                self.render_state.zoom_last_observed_size.remove(&id);
                self.render_state.zoom_resize_static_streak.remove(&id);
                self.node_app_ids.remove(&id);
                self.workspace_state.last_active_size.remove(&id);
                self.render_state.bbox_loc.remove(&id);
                self.render_state.window_geometry.remove(&id);
                self.pending_spawn_activate_at_ms.remove(&id);
                self.workspace_state.active_transition_until_ms.remove(&id);
                self.workspace_state
                    .primary_promote_cooldown_until_ms
                    .remove(&id);
                self.focus_state.last_surface_focus_ms.remove(&id);
                self.carry_zone_hint.remove(&id);
                self.carry_zone_last_change_ms.remove(&id);
                self.carry_zone_pending.remove(&id);
                self.carry_zone_pending_since_ms.remove(&id);
                self.carry_activation_anim_armed.remove(&id);
                self.carry_state_hold.remove(&id);
                if self.interaction_state.resize_active == Some(id) {
                    self.interaction_state.resize_active = None;
                }
                if self.interaction_state.resize_static_node == Some(id) {
                    self.interaction_state.resize_static_node = None;
                    self.interaction_state.resize_static_lock_pos = None;
                    self.interaction_state.resize_static_until_ms = 0;
                }
                if self.focus_state.primary_interaction_focus == Some(id) {
                    self.focus_state.primary_interaction_focus = None;
                    self.focus_state.interaction_focus_until_ms = 0;
                }
                let stale_monitors: Vec<String> = self
                    .focus_state
                    .monitor_focus
                    .iter()
                    .filter_map(|(monitor, &focused)| (focused == id).then_some(monitor.clone()))
                    .collect();

                for monitor in stale_monitors {
                    self.focus_state.monitor_focus.remove(&monitor);
                }
                self.interaction_state.smoothed_render_pos.remove(&id);
                let _ = self.field.remove(id);
            }
        }

        self.surface_activity.retain(|k, _| alive.contains(k));
    }
}
