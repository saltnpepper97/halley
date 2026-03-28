use super::*;

use halley_config::RuntimeTuning;

use crate::animation::{AnimSpec, AnimStyle};
use crate::render::{build_debug_scene, DebugScene};

impl Halley {
    const RECENT_INTERACTION_PROTECT_MS: u64 = 7_500;
    const COMPANION_PROTECT_MS: u64 = 12_000;
    const FOCUS_RING_PREVIEW_MS: u64 = 1_500;

    pub fn now_ms(&self, now: Instant) -> u64 {
        now.duration_since(self.runtime.started_at).as_millis() as u64
    }

    #[inline]
    pub(crate) fn is_recently_resized_node(&self, id: NodeId, now_ms: u64) -> bool {
        self.input.interaction_state.resize_static_node == Some(id)
            && now_ms < self.input.interaction_state.resize_static_until_ms
    }

    pub(crate) fn companion_surface_node(&self, now_ms: u64) -> Option<NodeId> {
        let focused = self.model.focus_state.primary_interaction_focus;
        self.model
            .focus_state
            .last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                if Some(id) == focused {
                    return None;
                }
                if now_ms.saturating_sub(at) > Self::COMPANION_PROTECT_MS {
                    return None;
                }
                self.model.field.node(id).and_then(|n| {
                    (self.model.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub(crate) fn is_recently_interacted_surface(&self, id: NodeId, now_ms: u64) -> bool {
        self.model
            .focus_state
            .last_surface_focus_ms
            .get(&id)
            .is_some_and(|&at| now_ms.saturating_sub(at) <= Self::RECENT_INTERACTION_PROTECT_MS)
    }

    pub fn mark_active_transition(&mut self, id: NodeId, now: Instant, duration_ms: u64) {
        if !self.runtime.tuning.physics_enabled {
            return;
        }
        self.model
            .workspace_state
            .active_transition_until_ms
            .insert(id, self.now_ms(now).saturating_add(duration_ms.max(1)));
        self.request_maintenance();
    }

    pub fn active_transition_alpha(&self, id: NodeId, now: Instant) -> f32 {
        if !self.runtime.tuning.physics_enabled {
            return 0.0;
        }
        let now_ms = self.now_ms(now);
        if self.input.interaction_state.resize_active == Some(id)
            || (self.input.interaction_state.resize_static_node == Some(id)
                && now_ms < self.input.interaction_state.resize_static_until_ms)
        {
            return 0.0;
        }
        let Some(&until) = self
            .model
            .workspace_state
            .active_transition_until_ms
            .get(&id)
        else {
            return 0.0;
        };
        if now_ms >= until {
            return 0.0;
        }
        let total = 420.0f32;
        let remaining = (until.saturating_sub(now_ms)) as f32;
        (remaining / total).clamp(0.0, 1.0)
    }

    pub fn pulse_node(&mut self, id: NodeId, now: Instant) {
        let _ = (id, now);
    }

    pub(crate) fn debug_dump(&self) {}

    pub fn build_debug_scene_snapshot(&self) -> DebugScene {
        build_debug_scene(
            &self.model.field,
            &self.model.viewport,
            self.active_focus_ring(),
        )
    }

    pub fn apply_tuning(&mut self, mut tuning: RuntimeTuning) {
        let prev_runtime_viewport = self.model.viewport;
        let prev_config_viewport = self.runtime.tuning.viewport();
        let prev_effective_no_csd = self.runtime.tuning.effective_no_csd();
        let prev_physics_enabled = self.runtime.tuning.physics_enabled;
        let prev_focus = self.last_input_surface_node();
        let previous_output_names: std::collections::HashSet<String> = self
            .model
            .monitor_state
            .monitors
            .keys()
            .cloned()
            .chain(
                self.runtime
                    .tuning
                    .tty_viewports
                    .iter()
                    .map(|v| v.connector.clone()),
            )
            .collect();

        tuning.enforce_guards();
        tuning.apply_process_env();

        let next_viewport = tuning.viewport();
        // Logical viewport geometry is separate from tty/output reconfiguration.
        // Reloading unrelated settings must not rewrite the live camera state.
        let logical_viewport_changed = prev_config_viewport.center != next_viewport.center
            || prev_config_viewport.size != next_viewport.size;
        if logical_viewport_changed {
            self.model.viewport = next_viewport;
            self.model.zoom_ref_size = tuning.viewport_size;
            self.model.camera_target_center = self.model.viewport.center;
            self.model.camera_target_view_size = self.model.zoom_ref_size;
            if prev_runtime_viewport.center != next_viewport.center
                || prev_runtime_viewport.size != next_viewport.size
            {
                self.input.interaction_state.viewport_pan_anim = None;
            }
        }

        self.ui.render_state.animator.set_spec(AnimSpec {
            state_change_ms: tuning.dev_anim_state_change_ms,
            bounce: tuning.dev_anim_bounce,
        });

        if prev_physics_enabled && !tuning.physics_enabled {
            self.model
                .workspace_state
                .active_transition_until_ms
                .clear();
            self.input.interaction_state.drag_authority_node = None;
            self.input.interaction_state.physics_velocity.clear();
            self.input.interaction_state.smoothed_render_pos.clear();
            self.model.camera_target_center = self.model.viewport.center;
            self.model.camera_target_view_size = self.model.zoom_ref_size;
        }

        let next_output_names: std::collections::HashSet<String> = previous_output_names
            .iter()
            .cloned()
            .chain(tuning.tty_viewports.iter().map(|v| v.connector.clone()))
            .collect();
        let now = Instant::now();
        let now_ms = self.now_ms(now);
        for output_name in next_output_names {
            if self
                .runtime
                .tuning
                .focus_ring_for_output(output_name.as_str())
                != tuning.focus_ring_for_output(output_name.as_str())
            {
                self.model.focus_state.focus_ring_preview_until_ms.insert(
                    output_name,
                    now_ms.saturating_add(Self::FOCUS_RING_PREVIEW_MS),
                );
            }
        }

        self.runtime.tuning = tuning;
        if prev_effective_no_csd != self.runtime.tuning.effective_no_csd() {
            self.refresh_xdg_decoration_mode();
        }
        self.request_maintenance();

        if let Some(id) = prev_focus {
            self.set_interaction_focus(Some(id), 30_000, now);
        }
    }

    pub fn anim_style_for(
        &self,
        id: NodeId,
        state: halley_core::field::NodeState,
        now: Instant,
    ) -> AnimStyle {
        if !self.runtime.tuning.dev_anim_enabled || !self.runtime.tuning.physics_enabled {
            return AnimStyle::default();
        }

        let now_ms = self.now_ms(now);
        if self.input.interaction_state.resize_active == Some(id)
            || (self.input.interaction_state.resize_static_node == Some(id)
                && now_ms < self.input.interaction_state.resize_static_until_ms)
        {
            return AnimStyle::default();
        }

        self.ui.render_state.animator.style_for(id, state, now)
    }

    pub fn anim_track_elapsed_for(
        &self,
        id: NodeId,
        state: halley_core::field::NodeState,
        now: Instant,
    ) -> Option<std::time::Duration> {
        self.ui
            .render_state
            .animator
            .track_elapsed_for(id, state, now)
    }

    pub fn active_focus_ring(&self) -> halley_core::viewport::FocusRing {
        self.runtime
            .tuning
            .focus_ring_for_output(self.model.monitor_state.current_monitor.as_str())
    }

    pub fn focus_ring_for_monitor(&self, monitor: &str) -> halley_core::viewport::FocusRing {
        self.runtime.tuning.focus_ring_for_output(monitor)
    }

    pub fn view_center_for_monitor(&self, monitor: &str) -> Vec2 {
        self.usable_viewport_for_monitor(monitor).center
    }

    pub fn usable_viewport_for_monitor(&self, monitor: &str) -> Viewport {
        if self.model.monitor_state.current_monitor == monitor {
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
                .map(|space| space.usable_viewport)
                .unwrap_or(self.model.viewport)
        }
    }

    pub fn should_draw_focus_ring_preview(&self, now: Instant) -> bool {
        self.model
            .focus_state
            .focus_ring_preview_until_ms
            .get(self.model.monitor_state.current_monitor.as_str())
            .is_some_and(|&until_ms| self.now_ms(now) < until_ms)
    }
}
