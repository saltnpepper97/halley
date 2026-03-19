use super::*;

use halley_config::RuntimeTuning;

use crate::animation::{AnimSpec, AnimStyle};
use crate::render::{DebugScene, build_debug_scene};

impl HalleyWlState {
    const RECENT_INTERACTION_PROTECT_MS: u64 = 7_500;
    const COMPANION_PROTECT_MS: u64 = 12_000;

    pub fn now_ms(&self, now: Instant) -> u64 {
        now.duration_since(self.started_at).as_millis() as u64
    }

    #[inline]
    pub(crate) fn is_recently_resized_node(&self, id: NodeId, now_ms: u64) -> bool {
        self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms
    }

    pub(crate) fn companion_surface_node(&self, now_ms: u64) -> Option<NodeId> {
        let focused = self.interaction_focus;
        self.last_surface_focus_ms
            .iter()
            .filter_map(|(&id, &at)| {
                if Some(id) == focused {
                    return None;
                }
                if now_ms.saturating_sub(at) > Self::COMPANION_PROTECT_MS {
                    return None;
                }
                self.field.node(id).and_then(|n| {
                    (self.field.is_visible(id) && n.kind == halley_core::field::NodeKind::Surface)
                        .then_some((id, at))
                })
            })
            .max_by_key(|(id, at)| (*at, id.as_u64()))
            .map(|(id, _)| id)
    }

    pub(crate) fn is_recently_interacted_surface(&self, id: NodeId, now_ms: u64) -> bool {
        self.last_surface_focus_ms
            .get(&id)
            .is_some_and(|&at| now_ms.saturating_sub(at) <= Self::RECENT_INTERACTION_PROTECT_MS)
    }

    pub fn mark_active_transition(&mut self, id: NodeId, now: Instant, duration_ms: u64) {
        if !self.tuning.physics_enabled {
            return;
        }
        self.active_transition_until_ms
            .insert(id, self.now_ms(now).saturating_add(duration_ms.max(1)));
        self.request_maintenance();
    }

    pub fn active_transition_alpha(&self, id: NodeId, now: Instant) -> f32 {
        if !self.tuning.physics_enabled {
            return 0.0;
        }
        let now_ms = self.now_ms(now);
        if self.resize_active == Some(id)
            || (self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms)
        {
            return 0.0;
        }
        let Some(&until) = self.active_transition_until_ms.get(&id) else {
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

    pub(crate) fn debug_dump(&self) {
        let focus_ring = self.active_focus_ring();

        let mut nodes_total = 0usize;
        let mut visible_total = 0usize;

        let mut zone_inside = 0usize;
        let mut zone_outside = 0usize;

        let mut state_active = 0usize;
        let mut state_node = 0usize;
        let mut state_core = 0usize;
        let mut state_other = 0usize;

        for (&id, node) in self.field.nodes() {
            nodes_total += 1;
            if !self.field.is_visible(id) {
                continue;
            }
            visible_total += 1;

            match node.state {
                halley_core::field::NodeState::Active => state_active += 1,
                halley_core::field::NodeState::Node => state_node += 1,
                halley_core::field::NodeState::Core => state_core += 1,
                _ => state_other += 1,
            }

            match focus_ring.zone(self.viewport.center, node.pos) {
                halley_core::viewport::FocusZone::Inside => zone_inside += 1,
                halley_core::viewport::FocusZone::Outside => zone_outside += 1,
            }
        }
    }

    pub fn build_debug_scene_snapshot(&self) -> DebugScene {
        build_debug_scene(&self.field, &self.viewport, self.active_focus_ring())
    }

    pub fn apply_tuning(&mut self, mut tuning: RuntimeTuning) {
        let prev_runtime_viewport = self.viewport;
        let prev_config_viewport = self.tuning.viewport();
        let prev_physics_enabled = self.tuning.physics_enabled;
        let prev_focus = self.last_input_surface_node();

        tuning.enforce_guards();
        tuning.apply_process_env();

        let next_viewport = tuning.viewport();
        // Logical viewport geometry is separate from tty/output reconfiguration.
        // Reloading unrelated settings must not rewrite the live camera state.
        let logical_viewport_changed = prev_config_viewport.center != next_viewport.center
            || prev_config_viewport.size != next_viewport.size;
        if logical_viewport_changed {
            self.viewport = next_viewport;
            self.zoom_ref_size = tuning.viewport_size;
            if prev_runtime_viewport.center != next_viewport.center
                || prev_runtime_viewport.size != next_viewport.size
            {
                self.viewport_pan_anim = None;
            }
        }

        self.animator.set_spec(AnimSpec {
            state_change_ms: tuning.dev_anim_state_change_ms,
            bounce: tuning.dev_anim_bounce,
        });

        if prev_physics_enabled && !tuning.physics_enabled {
            self.active_transition_until_ms.clear();
            self.smoothed_render_pos.clear();
        }

        self.tuning = tuning;
        self.request_maintenance();

        if let Some(id) = prev_focus {
            self.set_interaction_focus(Some(id), 30_000, Instant::now());
        }
    }

    pub fn anim_style_for(
        &self,
        id: NodeId,
        state: halley_core::field::NodeState,
        now: Instant,
    ) -> AnimStyle {
        if !self.tuning.dev_anim_enabled || !self.tuning.physics_enabled {
            return AnimStyle::default();
        }

        let now_ms = self.now_ms(now);
        if self.resize_active == Some(id)
            || (self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms)
        {
            return AnimStyle::default();
        }

        self.animator.style_for(id, state, now)
    }

    pub fn anim_track_elapsed_for(
        &self,
        id: NodeId,
        state: halley_core::field::NodeState,
        now: Instant,
    ) -> Option<std::time::Duration> {
        self.animator.track_elapsed_for(id, state, now)
    }

    pub fn active_focus_ring(&self) -> halley_core::viewport::FocusRing {
        self.tuning.focus_ring()
    }
}
