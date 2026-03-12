use std::time::Instant;

use halley_core::decay::DecayLevel;
use halley_core::field::{Field, NodeId, Vec2};
use smithay::reexports::wayland_server::{
    Resource, backend::ObjectId, protocol::wl_surface::WlSurface,
};

use crate::activity::CommitActivity;
use crate::state::HalleyWlState;
use crate::wm::overlap::CollisionExtents;

impl HalleyWlState {
    #[inline]
    fn surface_key(surface: &WlSurface) -> ObjectId {
        surface.id()
    }

    pub fn note_commit(&mut self, surface: &WlSurface, now: Instant) {
        let key = Self::surface_key(surface);
        self.surface_activity
            .entry(key)
            .or_insert_with(|| CommitActivity::new(now))
            .on_commit(now);
        if let Some(output) = &self.primary_output {
            output.enter(surface);
        }

        // Grant keyboard focus to layer surfaces (e.g. fuzzel) on their first
        // real commit, when keyboard_interactivity is now populated.
        self.maybe_grant_layer_surface_focus_on_commit(surface);
    }

    pub fn ensure_node_for_surface(
        &mut self,
        surface: &WlSurface,
        label: &str,
        size_px: (i32, i32),
    ) -> NodeId {
        let key = Self::surface_key(surface);
        if let Some(id) = self.surface_to_node.get(&key).copied() {
            return id;
        }

        let n = self.spawn_cursor;
        self.spawn_cursor += 1;
        let size = Vec2 {
            x: size_px.0.max(64) as f32,
            y: size_px.1.max(64) as f32,
        };
        let gap = self.non_overlap_gap_world();

        let pair_gap = gap;
        let conflict_at = |p: Vec2, field: &Field, size: Vec2, pair_gap: f32| -> bool {
            let candidate = CollisionExtents::symmetric(size);
            field.nodes().values().any(|other| {
                if other.kind != halley_core::field::NodeKind::Surface {
                    return false;
                }
                if !field.is_visible(other.id) {
                    return false;
                }
                let other_ext = self.collision_extents_for_node(other);
                let req_x = self.required_sep_x(p.x, candidate, other.pos.x, other_ext, pair_gap);
                let req_y = self.required_sep_y(p.y, candidate, other.pos.y, other_ext, pair_gap);
                (p.x - other.pos.x).abs() < req_x && (p.y - other.pos.y).abs() < req_y
            })
        };

        let previous_active: Vec<NodeId> = if self.tuning.new_window_on_top {
            self.field
                .nodes()
                .iter()
                .filter_map(|(&id, n)| {
                    (self.field.is_visible(id)
                        && n.kind == halley_core::field::NodeKind::Surface
                        && n.state == halley_core::field::NodeState::Active)
                        .then_some(id)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut pos = Vec2 {
            x: self.viewport.center.x,
            y: self.viewport.center.y,
        };
        if !self.tuning.new_window_on_top {
            let anchor = self
                .field
                .nodes()
                .values()
                .filter(|n| n.kind == halley_core::field::NodeKind::Surface)
                .filter(|n| self.field.is_visible(n.id))
                .max_by_key(|n| n.id.as_u64());
            if let Some(a) = anchor {
                let a_ext = self.collision_extents_for_node(a);
                let new_ext = CollisionExtents::symmetric(size);
                let dx_right =
                    self.required_sep_x(a.pos.x, a_ext, a.pos.x + 1.0, new_ext, pair_gap);
                let dx_left = self.required_sep_x(a.pos.x, a_ext, a.pos.x - 1.0, new_ext, pair_gap);
                let dy_down = self.required_sep_y(a.pos.y, a_ext, a.pos.y + 1.0, new_ext, pair_gap);
                let dy_up = self.required_sep_y(a.pos.y, a_ext, a.pos.y - 1.0, new_ext, pair_gap);
                let sign = if n.is_multiple_of(2) { 1.0 } else { -1.0 };
                let candidates = [
                    Vec2 {
                        x: a.pos.x + if sign > 0.0 { dx_right } else { -dx_left },
                        y: a.pos.y,
                    },
                    Vec2 {
                        x: a.pos.x - if sign > 0.0 { dx_left } else { -dx_right },
                        y: a.pos.y,
                    },
                    Vec2 {
                        x: a.pos.x,
                        y: a.pos.y + dy_down,
                    },
                    Vec2 {
                        x: a.pos.x,
                        y: a.pos.y - dy_up,
                    },
                    Vec2 {
                        x: a.pos.x + if sign > 0.0 { dx_right } else { -dx_left },
                        y: a.pos.y + dy_down * 0.5,
                    },
                    Vec2 {
                        x: a.pos.x - if sign > 0.0 { dx_left } else { -dx_right },
                        y: a.pos.y - dy_up * 0.5,
                    },
                ];
                if let Some(p) = candidates
                    .into_iter()
                    .find(|p| !conflict_at(*p, &self.field, size, pair_gap))
                {
                    pos = p;
                } else {
                    for i in 0..24u32 {
                        let ring = 200.0 + ((i / 8) as f32) * 120.0;
                        let theta = (i % 8) as f32 * std::f32::consts::TAU / 8.0;
                        let p = Vec2 {
                            x: self.viewport.center.x + ring * theta.cos(),
                            y: self.viewport.center.y + ring * theta.sin(),
                        };
                        if !conflict_at(p, &self.field, size, pair_gap) {
                            pos = p;
                            break;
                        }
                    }
                }
            }
        }

        let id = self.field.spawn_surface(label.to_string(), pos, size);
        let _ = self
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        let _ = self.field.set_decay_level(id, DecayLevel::Hot);

        if self.tuning.new_window_on_top {
            for old_id in previous_active {
                let Some(old) = self.field.node(old_id) else {
                    continue;
                };
                let old_pos = old.pos;
                let old_ext = self.collision_extents_for_node(old);
                let new_ext = CollisionExtents::symmetric(size);
                let req_x = self.required_sep_x(pos.x, new_ext, old_pos.x, old_ext, pair_gap);
                let req_y = self.required_sep_y(pos.y, new_ext, old_pos.y, old_ext, pair_gap);
                let dx = old_pos.x - pos.x;
                let dy = old_pos.y - pos.y;
                if dx.abs() < req_x && dy.abs() < req_y {
                    let mut target = old_pos;
                    let overlap_x = req_x - dx.abs();
                    let overlap_y = req_y - dy.abs();
                    if overlap_x >= overlap_y {
                        let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
                        target.x = pos.x + dir * (req_x + 1.0);
                    } else {
                        let dir = if dy >= 0.0 { 1.0 } else { -1.0 };
                        target.y = pos.y + dir * (req_y + 1.0);
                    }
                    let _ = self.field.carry(old_id, target);
                }
                let _ = self.field.set_decay_level(old_id, DecayLevel::Cold);
            }
        }

        self.surface_to_node.insert(key, id);
        self.zoom_nominal_size.insert(id, size);
        self.last_active_size.insert(id, size);
        let now = Instant::now();
        self.pending_spawn_activate_at_ms
            .insert(id, self.now_ms(now).saturating_add(220));
        if self.tuning.dev_anim_enabled {
            self.animator.observe_field(&self.field, now);
        }
        id
    }

    pub fn drop_surface(&mut self, surface: &WlSurface) {
        if let Some(output) = &self.primary_output {
            output.leave(surface);
        }
        let key = Self::surface_key(surface);
        self.surface_activity.remove(&key);
        if let Some(id) = self.surface_to_node.remove(&key) {
            if self.pan_restore_active_focus == Some(id) {
                self.pan_restore_active_focus = None;
            }
            let _ = self.field.undock_node(id);
            self.field.clear_dock_preview();
            self.zoom_nominal_size.remove(&id);
            self.zoom_resize_fallback.remove(&id);
            self.zoom_resize_reject_streak.remove(&id);
            self.zoom_last_observed_size.remove(&id);
            self.zoom_resize_static_streak.remove(&id);
            self.last_active_size.remove(&id);
            self.bbox_loc.remove(&id);
            self.window_geometry.remove(&id);
            self.pending_spawn_activate_at_ms.remove(&id);
            self.active_transition_until_ms.remove(&id);
            self.primary_promote_cooldown_until_ms.remove(&id);
            self.last_surface_focus_ms.remove(&id);
            self.carry_zone_hint.remove(&id);
            self.carry_zone_last_change_ms.remove(&id);
            self.carry_zone_pending.remove(&id);
            self.carry_zone_pending_since_ms.remove(&id);
            self.carry_activation_anim_armed.remove(&id);
            if self.resize_active == Some(id) {
                self.resize_active = None;
            }
            if self.resize_static_node == Some(id) {
                self.resize_static_node = None;
                self.resize_static_lock_pos = None;
                self.resize_static_until_ms = 0;
            }
            if self.interaction_focus == Some(id) {
                self.interaction_focus = None;
                self.interaction_focus_until_ms = 0;
            }
            self.smoothed_render_pos.remove(&id);
            let _ = self.field.remove(id);
        }
    }
}
