use super::*;
use eventline::debug;

use crate::anim::{AnimSpec, AnimStyle};
use crate::config::RuntimeTuning;
use crate::render::{DebugScene, build_debug_scene};

impl HalleyWlState {
    pub fn now_ms(&self, now: Instant) -> u64 {
        now.duration_since(self.started_at).as_millis() as u64
    }

    #[inline]
    pub(crate) fn is_recently_resized_node(&self, id: NodeId, now_ms: u64) -> bool {
        self.resize_static_node == Some(id) && now_ms < self.resize_static_until_ms
    }

    pub fn mark_active_transition(&mut self, id: NodeId, now: Instant, duration_ms: u64) {
        if !self.tuning.physics_enabled {
            return;
        }
        self.active_transition_until_ms
            .insert(id, self.now_ms(now).saturating_add(duration_ms.max(1)));
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

    pub(super) fn debug_dump(&self) {
        let rings = self.active_rings();

        let mut nodes_total = 0usize;
        let mut visible_total = 0usize;

        let mut zone_primary = 0usize;
        let mut zone_secondary = 0usize;
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

            match rings.zone(self.viewport.center, node.pos) {
                RingZone::Primary => zone_primary += 1,
                RingZone::Secondary => zone_secondary += 1,
                RingZone::Outside => zone_outside += 1,
            }
        }

        debug!(
            "tick-dump nodes={} visible={} state(a/n/c/o)={}/{}/{}/{} zone(p/s/o)={}/{}/{} vp=({:.0},{:.0}) {:.0}x{:.0} rings(pr={:.0}x{:.0} sr={:.0}x{:.0} rot={:.2})",
            nodes_total,
            visible_total,
            state_active,
            state_node,
            state_core,
            state_other,
            zone_primary,
            zone_secondary,
            zone_outside,
            self.viewport.center.x,
            self.viewport.center.y,
            self.viewport.size.x,
            self.viewport.size.y,
            self.tuning.ring_primary_rx,
            self.tuning.ring_primary_ry,
            self.tuning.ring_secondary_rx,
            self.tuning.ring_secondary_ry,
            self.tuning.ring_rotation_rad,
        );
    }

    pub fn build_debug_scene_snapshot(&self) -> DebugScene {
        build_debug_scene(&self.field, &self.viewport, self.active_rings())
    }

    pub fn apply_tuning(&mut self, mut tuning: RuntimeTuning) {
        let had_nodes = !self.field.nodes().is_empty();
        let prev_viewport = self.viewport;
        let prev_focus = self.last_input_surface_node();
        tuning.enforce_guards();
        tuning.apply_process_env();
        if had_nodes {
            // Config reload should not jump the camera away from the current session.
            self.viewport = prev_viewport;
            tuning.viewport_center = prev_viewport.center;
            tuning.viewport_size = prev_viewport.size;
        } else {
            self.viewport = tuning.viewport();
        }
        self.zoom_ref_size = tuning.viewport_size;
        self.animator.set_spec(AnimSpec {
            state_change_ms: tuning.dev_anim_state_change_ms,
            bounce: tuning.dev_anim_bounce,
        });
        if !tuning.physics_enabled {
            self.active_transition_until_ms.clear();
            self.smoothed_render_pos.clear();
        }
        self.tuning = tuning;
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

    pub fn active_rings(&self) -> FocusRings {
        let mut rings = self.tuning.rings();
        let sx = (self.viewport.size.x / self.zoom_ref_size.x.max(1.0)).clamp(0.1, 100.0);
        let sy = (self.viewport.size.y / self.zoom_ref_size.y.max(1.0)).clamp(0.1, 100.0);
        rings.primary.radius_x *= sx;
        rings.primary.radius_y *= sy;
        rings.secondary.radius_x *= sx;
        rings.secondary.radius_y *= sy;
        rings
    }
}
