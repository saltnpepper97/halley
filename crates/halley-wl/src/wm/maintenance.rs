use std::collections::{HashMap, HashSet};

use super::*;
use crate::wm::overlap::CollisionExtents;
use halley_core::viewport::{FocusRing, FocusZone};

impl Halley {
    const ACTIVE_RING_OUTSIDE_DECAY_FRAC: f32 = 0.98;

    fn focus_ring_center_for_node(&self, id: NodeId) -> Vec2 {
        self.model
            .monitor_state
            .node_monitor
            .get(&id)
            .and_then(|monitor| self.model.monitor_state.monitors.get(monitor))
            .map(|monitor| monitor.viewport.center)
            .unwrap_or(self.model.viewport.center)
    }

    fn focus_ring_for_node(&self, id: NodeId) -> FocusRing {
        self.model
            .monitor_state
            .node_monitor
            .get(&id)
            .map(|monitor| self.runtime.tuning.focus_ring_for_output(monitor.as_str()))
            .unwrap_or_else(|| self.active_focus_ring())
    }

    fn focus_ring_coverage_for_extents(
        &self,
        pos: Vec2,
        ext: CollisionExtents,
        focus_center: Vec2,
        focus_ring: FocusRing,
    ) -> (f32, f32) {
        let samples = 9usize;
        let width = (ext.left + ext.right).max(1.0);
        let height = (ext.top + ext.bottom).max(1.0);
        let left = pos.x - ext.left;
        let top = pos.y - ext.top;
        let mut inside = 0usize;
        let mut total = 0usize;

        for ix in 0..samples {
            for iy in 0..samples {
                let fx = ix as f32 / (samples - 1) as f32;
                let fy = iy as f32 / (samples - 1) as f32;
                let sample = Vec2 {
                    x: left + fx * width,
                    y: top + fy * height,
                };
                if focus_ring.zone(focus_center, sample) == FocusZone::Inside {
                    inside += 1;
                }
                total += 1;
            }
        }

        if total == 0 {
            return (0.0, 1.0);
        }

        let inside_frac = inside as f32 / total as f32;
        (inside_frac, (1.0 - inside_frac).max(0.0))
    }

    fn surface_ring_coverage(&self, id: NodeId) -> (f32, f32) {
        let Some(node) = self.model.field.node(id) else {
            return (0.0, 1.0);
        };
        let focus_center = self.focus_ring_center_for_node(id);
        let focus_ring = self.focus_ring_for_node(id);

        let ext = match node.state {
            halley_core::field::NodeState::Active => self.surface_window_collision_extents(node),
            _ => self.collision_extents_for_node(node),
        };

        self.focus_ring_coverage_for_extents(node.pos, ext, focus_center, focus_ring)
    }

    fn surface_is_definitively_outside_focus_ring(&self, id: NodeId) -> bool {
        let (_, outside_frac) = self.surface_ring_coverage(id);
        outside_frac >= Self::ACTIVE_RING_OUTSIDE_DECAY_FRAC
    }

    pub(crate) fn enforce_single_primary_active_unit(&mut self) {
        let now_ms = self.now_ms(Instant::now());
        let active_windows_allowed = self.runtime.tuning.active_windows_allowed.max(1);
        let companion = self.companion_surface_node(now_ms);
        let preferred_surface = self.last_input_surface_node();

        let active_ids: Vec<NodeId> = self
            .model
            .field
            .nodes()
            .iter()
            .filter_map(|(&id, n)| {
                (self.model.field.participates_in_field_activity(id)
                    && self.model.field.is_visible(id)
                    && n.kind == halley_core::field::NodeKind::Surface
                    && n.state == halley_core::field::NodeState::Active)
                    .then_some(id)
            })
            .collect();

        let mut active_ids_by_monitor: HashMap<Option<String>, Vec<NodeId>> = HashMap::new();
        for id in active_ids {
            let monitor = self.model.monitor_state.node_monitor.get(&id).cloned();
            active_ids_by_monitor.entry(monitor).or_default().push(id);
        }

        for active_ids in active_ids_by_monitor.into_values() {
            if active_ids.len() <= active_windows_allowed {
                continue;
            }

            let mut keep_set: HashSet<NodeId> = HashSet::new();

            let focused_breakout: Option<NodeId> = active_ids
                .iter()
                .copied()
                .find(|&id| {
                    let monitor = self.model.monitor_state.node_monitor.get(&id);
                    monitor
                        .and_then(|m| self.model.focus_state.monitor_focus.get(m))
                        .copied()
                        == Some(id)
                })
                .or_else(|| {
                    active_ids.iter().copied().max_by_key(|id| {
                        self.model
                            .focus_state
                            .last_surface_focus_ms
                            .get(id)
                            .copied()
                            .unwrap_or(0)
                    })
                });

            if let Some(fid) = focused_breakout {
                keep_set.insert(fid);
            }

            if keep_set.len() < active_windows_allowed {
                let mut ranked = active_ids.clone();
                ranked.sort_by_key(|id| {
                    let preferred_rank = u8::from(preferred_surface == Some(*id));
                    let focus_rank = u8::from({
                        let monitor = self.model.monitor_state.node_monitor.get(id);
                        monitor
                            .and_then(|m| self.model.focus_state.monitor_focus.get(m))
                            .copied()
                            == Some(*id)
                    });
                    let companion_rank = u8::from(companion == Some(*id));
                    let inside_rank =
                        u8::from(!self.surface_is_definitively_outside_focus_ring(*id));
                    let latest_focus = self
                        .model
                        .focus_state
                        .last_surface_focus_ms
                        .get(id)
                        .copied()
                        .unwrap_or(0);
                    (
                        preferred_rank,
                        focus_rank,
                        companion_rank,
                        inside_rank,
                        latest_focus,
                        id.as_u64(),
                    )
                });

                for id in ranked.iter().rev().copied() {
                    keep_set.insert(id);
                    if keep_set.len() >= active_windows_allowed {
                        break;
                    }
                }
            }

            for id in active_ids {
                if keep_set.contains(&id) {
                    continue;
                }
                let _ = self.model.field.set_decay_level(id, DecayLevel::Cold);
            }
        }
    }

    pub fn apply_single_surface_decay_policy(
        &mut self,
        id: NodeId,
        now_ms: u64,
        active_delay_ms: u64,
        inactive_delay_ms: u64,
    ) {
        let Some(n) = self.model.field.node(id) else {
            return;
        };
        if !self.model.field.participates_in_field_activity(id)
            || !self.model.field.is_visible(id)
            || n.kind != halley_core::field::NodeKind::Surface
        {
            return;
        }

        if self.preserve_collapsed_surface(id) {
            return;
        }

        if self.is_hard_decay_protected(id, now_ms) {
            let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
            return;
        }

        let outside_ring = self.surface_is_definitively_outside_focus_ring(id);
        if !outside_ring {
            let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
            return;
        }

        let is_primary = self.model.focus_state.primary_interaction_focus == Some(id);
        let delay_ms = if is_primary {
            active_delay_ms
        } else {
            inactive_delay_ms
        };

        let last_focus_ms = self
            .model
            .focus_state
            .last_surface_focus_ms
            .get(&id)
            .copied()
            .unwrap_or(0);

        if now_ms.saturating_sub(last_focus_ms) >= delay_ms {
            let _ = self.model.field.set_decay_level(id, DecayLevel::Cold);
        } else {
            let _ = self.model.field.set_decay_level(id, DecayLevel::Hot);
        }
    }

    fn is_hard_decay_protected(&self, id: NodeId, now_ms: u64) -> bool {
        self.model.focus_state.primary_interaction_focus == Some(id)
            || self.input.interaction_state.resize_active == Some(id)
            || self.is_recently_resized_node(id, now_ms)
            || self.model.carry_state.carry_zone_hint.contains_key(&id)
            || self
                .model
                .workspace_state
                .active_transition_until_ms
                .contains_key(&id)
    }

    pub fn surface_intersects_viewport(&self, id: NodeId) -> bool {
        let Some(n) = self.model.field.node(id) else {
            return false;
        };
        if !self.model.field.participates_in_field_activity(id)
            || n.kind != halley_core::field::NodeKind::Surface
            || !self.model.field.is_visible(id)
        {
            return false;
        }

        let ext = self.collision_extents_for_node(n);
        let half_vw = self.model.viewport.size.x * 0.5;
        let half_vh = self.model.viewport.size.y * 0.5;

        let view_left = self.model.viewport.center.x - half_vw;
        let view_right = self.model.viewport.center.x + half_vw;
        let view_top = self.model.viewport.center.y - half_vh;
        let view_bottom = self.model.viewport.center.y + half_vh;

        let node_left = n.pos.x - ext.left;
        let node_right = n.pos.x + ext.right;
        let node_top = n.pos.y - ext.top;
        let node_bottom = n.pos.y + ext.bottom;

        node_right > view_left
            && node_left < view_right
            && node_bottom > view_top
            && node_top < view_bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_surface_with_small_ring_overlap_is_not_treated_as_outside() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.focus_ring_rx = 100.0;
        tuning.focus_ring_ry = 100.0;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "edge-overlap",
            Vec2 { x: 145.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        state
            .model
            .workspace_state
            .last_active_size
            .insert(id, Vec2 { x: 100.0, y: 100.0 });
        state
            .ui
            .render_state
            .window_geometry
            .insert(id, (-50.0, -50.0, 100.0, 100.0));
        state.ui.render_state.bbox_loc.insert(id, (0.0, 0.0));

        assert!(!state.surface_is_definitively_outside_focus_ring(id));
    }

    #[test]
    fn active_surface_fully_clear_of_ring_is_treated_as_outside() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.focus_ring_rx = 100.0;
        tuning.focus_ring_ry = 100.0;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "outside",
            Vec2 { x: 260.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        state
            .model
            .workspace_state
            .last_active_size
            .insert(id, Vec2 { x: 100.0, y: 100.0 });
        state
            .ui
            .render_state
            .window_geometry
            .insert(id, (-50.0, -50.0, 100.0, 100.0));
        state.ui.render_state.bbox_loc.insert(id, (0.0, 0.0));

        assert!(state.surface_is_definitively_outside_focus_ring(id));
    }
}
