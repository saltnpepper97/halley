use super::*;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;

impl HalleyWlState {
    #[inline]
    fn world_units_per_px_xy(&self) -> (f32, f32) {
        let wx = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let wy = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);
        (wx.max(0.01), wy.max(0.01))
    }

    pub(super) fn non_overlap_gap_world(&self) -> f32 {
        let (wx, wy) = self.world_units_per_px_xy();
        self.tuning.non_overlap_gap_px.max(0.0) * ((wx + wy) * 0.5)
    }

    pub(crate) fn carry_surface_non_overlap(&mut self, id: NodeId, to: Vec2) -> bool {
        if self.suspend_overlap_resolve || self.suspend_state_checks {
            return self.carry_surface_no_overlap_static(id, to);
        }
        if !self.tuning.physics_enabled {
            return self.field.carry(id, to);
        }

        let Some(n) = self.field.node(id) else {
            return false;
        };
        if n.kind != halley_core::field::NodeKind::Surface {
            return self.field.carry(id, to);
        }

        let mover_size = self.collision_size_for_node(n);
        let gap = self.non_overlap_gap_world();
        let damping = self.tuning.non_overlap_bump_damping.clamp(0.05, 1.0);
        let mut mover_pos = to;

        const MAX_PUSH_STEP: f32 = 6.0;
        const MAX_PUSHES_PER_PASS: usize = 2;

        for _ in 0..16 {
            let others: Vec<(NodeId, Vec2, Vec2, bool)> = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == id || !self.field.is_visible(oid) {
                        return None;
                    }
                    if other.kind != halley_core::field::NodeKind::Surface {
                        return None;
                    }
                    Some((
                        oid,
                        other.pos,
                        self.collision_size_for_node(other),
                        other.pinned,
                    ))
                })
                .collect();

            let mut changed = false;
            let mut pushes_this_pass = 0usize;

            for (oid, opos, osize, opinned) in others {
                if pushes_this_pass >= MAX_PUSHES_PER_PASS {
                    break;
                }

                let dx = opos.x - mover_pos.x;
                let dy = opos.y - mover_pos.y;
                let req_x = mover_size.x * 0.5 + osize.x * 0.5 + gap;
                let req_y = mover_size.y * 0.5 + osize.y * 0.5 + gap;

                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();

                // Small soft zone so windows begin easing apart before true overlap.
                let soft_zone = 1.0;
                let sx = (req_x + soft_zone) - dx.abs();
                let sy = (req_y + soft_zone) - dy.abs();

                if sx > 0.0 && sy > 0.0 {
                    let soft = damping * 0.14;

                    if sx < sy {
                        let step = (sx * soft).max(0.35).min(MAX_PUSH_STEP);
                        if opinned {
                            let s = if dx >= 0.0 { -1.0 } else { 1.0 };
                            mover_pos.x += s * step;
                        } else {
                            let s = if dx >= 0.0 { 1.0 } else { -1.0 };
                            let split_other = step * 0.45;
                            let split_mover = step * 0.55;

                            let _ = self.field.carry(
                                oid,
                                Vec2 {
                                    x: opos.x + s * split_other,
                                    y: opos.y,
                                },
                            );
                            mover_pos.x -= s * split_mover;
                        }
                        changed = true;
                        pushes_this_pass += 1;
                    } else {
                        let step = (sy * soft).max(0.35).min(MAX_PUSH_STEP);
                        if opinned {
                            let s = if dy >= 0.0 { -1.0 } else { 1.0 };
                            mover_pos.y += s * step;
                        } else {
                            let s = if dy >= 0.0 { 1.0 } else { -1.0 };
                            let split_other = step * 0.45;
                            let split_mover = step * 0.55;

                            let _ = self.field.carry(
                                oid,
                                Vec2 {
                                    x: opos.x,
                                    y: opos.y + s * split_other,
                                },
                            );
                            mover_pos.y -= s * split_mover;
                        }
                        changed = true;
                        pushes_this_pass += 1;
                    }
                }

                if ox > 0.0 && oy > 0.0 {
                    let hard_gain = 0.24;

                    if opinned {
                        if ox < oy {
                            let s = if dx >= 0.0 { -1.0 } else { 1.0 };
                            mover_pos.x += s * ((ox + 2.0) * hard_gain).min(MAX_PUSH_STEP);
                        } else {
                            let s = if dy >= 0.0 { -1.0 } else { 1.0 };
                            mover_pos.y += s * ((oy + 2.0) * hard_gain).min(MAX_PUSH_STEP);
                        }
                        changed = true;
                    } else {
                        let target = if ox < oy {
                            let s = if dx >= 0.0 { 1.0 } else { -1.0 };
                            let step = ((ox + 2.0) * hard_gain).min(MAX_PUSH_STEP);
                            mover_pos.x -= s * (step * 0.55);
                            Vec2 {
                                x: opos.x + s * (step * 0.45),
                                y: opos.y,
                            }
                        } else {
                            let s = if dy >= 0.0 { 1.0 } else { -1.0 };
                            let step = ((oy + 2.0) * hard_gain).min(MAX_PUSH_STEP);
                            mover_pos.y -= s * (step * 0.55);
                            Vec2 {
                                x: opos.x,
                                y: opos.y + s * (step * 0.45),
                            }
                        };

                        if self.field.carry(oid, target) {
                            changed = true;
                            pushes_this_pass += 1;
                        }
                    }
                }
            }

            if !changed {
                break;
            }
        }

        self.field.carry(id, mover_pos)
    }

    fn carry_surface_no_overlap_static(&mut self, id: NodeId, to: Vec2) -> bool {
        let Some(n) = self.field.node(id) else {
            return false;
        };
        if n.kind != halley_core::field::NodeKind::Surface {
            return self.field.carry(id, to);
        }

        let mover_size = self.collision_size_for_node(n);
        let gap = self.non_overlap_gap_world();
        let mut mover_pos = to;

        for _ in 0..24 {
            let others: Vec<(NodeId, Vec2, Vec2)> = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == id || !self.field.is_visible(oid) {
                        return None;
                    }
                    if other.kind != halley_core::field::NodeKind::Surface {
                        return None;
                    }
                    Some((oid, other.pos, self.collision_size_for_node(other)))
                })
                .collect();

            let mut changed = false;

            for (oid, opos, osize) in others {
                let dx = mover_pos.x - opos.x;
                let dy = mover_pos.y - opos.y;
                let req_x = mover_size.x * 0.5 + osize.x * 0.5 + gap;
                let req_y = mover_size.y * 0.5 + osize.y * 0.5 + gap;
                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();

                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }

                if ox < oy {
                    let s = if dx.abs() > f32::EPSILON {
                        dx.signum()
                    } else if oid.as_u64() < id.as_u64() {
                        1.0
                    } else {
                        -1.0
                    };
                    mover_pos.x += s * (ox + 0.3);
                } else {
                    let s = if dy.abs() > f32::EPSILON {
                        dy.signum()
                    } else {
                        1.0
                    };
                    mover_pos.y += s * (oy + 0.3);
                }

                changed = true;
            }

            if !changed {
                break;
            }
        }

        self.field.carry(id, mover_pos)
    }

    fn preview_collision_size(real_w: f32, real_h: f32) -> Vec2 {
        let w = real_w.max(1.0);
        let h = real_h.max(1.0);
        let aspect = w / h;
        let base_h = 160.0f32;
        let mut out_w = base_h * aspect;
        let mut out_h = base_h;

        if out_w < 180.0 {
            out_w = 180.0;
            out_h = out_w / aspect.max(0.1);
        }
        if out_w > 360.0 {
            out_w = 360.0;
            out_h = out_w / aspect.max(0.1);
        }

        out_h = out_h.clamp(100.0, 220.0);
        Vec2 { x: out_w, y: out_h }
    }

    #[inline]
    fn active_collision_scale(anim_scale: f32, real_w: f32, real_h: f32) -> f32 {
        let base = Self::preview_collision_size(real_w, real_h);
        let start = (base.x / real_w.max(1.0))
            .min(base.y / real_h.max(1.0))
            .clamp(0.24, 1.0);
        let t = ((anim_scale - 0.30) / (1.0 - 0.30)).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);

        let mut out = start + (1.0 - start) * e;
        if anim_scale > 1.0 {
            out += (anim_scale - 1.0) * 0.30;
        }

        out.clamp(0.24, 1.08)
    }

    #[inline]
    fn proxy_collision_scale(anim_scale: f32) -> f32 {
        anim_scale.clamp(0.22, 1.4)
    }

    fn node_collision_size(&self, label: &str, anim_scale: f32) -> Vec2 {
        let zx = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let zy = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);
        let z = ((zx + zy) * 0.5).clamp(1.0, 8.0);
        let g = z.sqrt() * Self::proxy_collision_scale(anim_scale);

        let dot_half_px = (4.0 * g).round().clamp(4.0, 18.0);
        let label_h_px = (4.0 * g).round().clamp(4.0, 14.0);
        let label_gap_px = (8.0 + (g - 1.0) * 8.0).round().clamp(8.0, 28.0);
        let label_w_px = ((label.len() as f32 * 6.0) * (0.9 + 0.6 * g))
            .round()
            .clamp(24.0, 320.0);
        let pad_px = 6.0;

        let left_half_px = dot_half_px + pad_px;
        let right_half_px = label_gap_px + label_w_px + pad_px;
        let half_w_px = left_half_px.max(right_half_px).max(4.0);
        let half_h_px = (dot_half_px.max(label_h_px * 0.5) + pad_px).max(4.0);
        let bounds_w_px = (half_w_px * 2.0).max(8.0);
        let bounds_h_px = (half_h_px * 2.0).max(8.0);

        let world_per_px_x = self.viewport.size.x / self.zoom_ref_size.x.max(1.0);
        let world_per_px_y = self.viewport.size.y / self.zoom_ref_size.y.max(1.0);

        Vec2 {
            x: bounds_w_px * world_per_px_x.max(0.01),
            y: bounds_h_px * world_per_px_y.max(0.01),
        }
    }

    pub(super) fn collision_size_for_node(&self, n: &halley_core::field::Node) -> Vec2 {
        let now = Instant::now();
        let anim = self.anim_style_for(n.id, n.state.clone(), now);

        match n.state {
            halley_core::field::NodeState::Active => {
                let basis = self
                    .last_active_size
                    .get(&n.id)
                    .copied()
                    .unwrap_or(n.intrinsic_size);
                let s = Self::active_collision_scale(anim.scale, basis.x, basis.y);
                let (world_per_px_x, world_per_px_y) = self.world_units_per_px_xy();
                let base_w = (basis.x - 4.0).max(32.0);
                let base_h = (basis.y - 14.0).max(32.0);

                Vec2 {
                    x: base_w * s * world_per_px_x,
                    y: base_h * s * world_per_px_y,
                }
            }
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                self.node_collision_size(&n.label, anim.scale)
            }
            halley_core::field::NodeState::Preview => {
                self.node_collision_size(&n.label, anim.scale)
            }
            halley_core::field::NodeState::Drifting => n.footprint,
        }
    }

    pub(super) fn resolve_surface_overlap(&mut self) {
        if !self.tuning.physics_enabled {
            return;
        }
        if self.suspend_overlap_resolve {
            return;
        }

        let now_ms = self.now_ms(Instant::now());
        let mut ids: Vec<NodeId> = self
            .field
            .nodes()
            .keys()
            .copied()
            .filter(|&id| self.field.is_visible(id))
            .filter(|&id| {
                self.field
                    .node(id)
                    .is_some_and(|n| n.kind == halley_core::field::NodeKind::Surface)
            })
            .collect();

        if ids.len() < 2 {
            return;
        }

        ids.sort_by_key(|id| id.as_u64());

        let gap = self.non_overlap_gap_world();
        let focus_id = self.interaction_focus;
        let damping = self.tuning.non_overlap_bump_damping.clamp(0.05, 0.35);

        const MAX_RESOLVE_STEP: f32 = 18.0;

        for _ in 0..24 {
            let mut changed = false;

            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    let a = ids[i];
                    let b = ids[j];

                    if self.is_recently_resized_node(a, now_ms)
                        || self.is_recently_resized_node(b, now_ms)
                    {
                        continue;
                    }

                    if self
                        .docked_links
                        .get(&a)
                        .is_some_and(|link| link.partner == b)
                        || self
                            .docked_links
                            .get(&b)
                            .is_some_and(|link| link.partner == a)
                    {
                        continue;
                    }

                    let Some(na) = self.field.node(a) else {
                        continue;
                    };
                    let Some(nb) = self.field.node(b) else {
                        continue;
                    };

                    let a_pos = na.pos;
                    let b_pos = nb.pos;
                    let a_pinned = na.pinned;
                    let b_pinned = nb.pinned;

                    if a_pinned && b_pinned {
                        continue;
                    }

                    let sa = self.collision_size_for_node(na);
                    let sb = self.collision_size_for_node(nb);

                    let dx = b_pos.x - a_pos.x;
                    let dy = b_pos.y - a_pos.y;

                    let req_x = sa.x * 0.5 + sb.x * 0.5 + gap;
                    let req_y = sa.y * 0.5 + sb.y * 0.5 + gap;
                    let ox = req_x - dx.abs();
                    let oy = req_y - dy.abs();

                    let soft_zone = 1.0;
                    let sx = (req_x + soft_zone) - dx.abs();
                    let sy = (req_y + soft_zone) - dy.abs();

                    let mut primary_id = if focus_id == Some(a) {
                        b
                    } else if focus_id == Some(b) {
                        a
                    } else {
                        b
                    };

                    if primary_id == a && a_pinned && !b_pinned {
                        primary_id = b;
                    } else if primary_id == b && b_pinned && !a_pinned {
                        primary_id = a;
                    } else if a_pinned && b_pinned {
                        continue;
                    }

                    let (mover_id, mover_pos, anchor_pos, mx, my, mover_pinned) = if primary_id == a {
                        (a, a_pos, b_pos, if dx >= 0.0 { -1.0 } else { 1.0 }, if dy >= 0.0 { -1.0 } else { 1.0 }, a_pinned)
                    } else {
                        (b, b_pos, a_pos, if dx >= 0.0 { 1.0 } else { -1.0 }, if dy >= 0.0 { 1.0 } else { -1.0 }, b_pinned)
                    };

                    if mover_pinned {
                        continue;
                    }

                    if ox <= 0.0 || oy <= 0.0 {
                        if sx > 0.0 && sy > 0.0 {
                            let nudge = (sx.min(sy) * damping * 0.18).clamp(0.15, 2.4);
                            let target = if sx < sy {
                                Vec2 {
                                    x: mover_pos.x + mx * nudge,
                                    y: mover_pos.y,
                                }
                            } else {
                                Vec2 {
                                    x: mover_pos.x,
                                    y: mover_pos.y + my * nudge,
                                }
                            };

                            if self.field.carry(mover_id, target) {
                                changed = true;
                            }
                        }
                        continue;
                    }

                    let full_target = if ox < oy {
                        Vec2 {
                            x: anchor_pos.x + mx * (req_x + 0.3),
                            y: mover_pos.y,
                        }
                    } else {
                        Vec2 {
                            x: mover_pos.x,
                            y: anchor_pos.y + my * (req_y + 0.3),
                        }
                    };

                    let mut step = Vec2 {
                        x: (full_target.x - mover_pos.x) * damping,
                        y: (full_target.y - mover_pos.y) * damping,
                    };

                    step.x = step.x.clamp(-MAX_RESOLVE_STEP, MAX_RESOLVE_STEP);
                    step.y = step.y.clamp(-MAX_RESOLVE_STEP, MAX_RESOLVE_STEP);

                    if step.x.abs() < 0.5 && (full_target.x - mover_pos.x).abs() > 0.5 {
                        step.x = 0.5 * step.x.signum();
                    }
                    if step.y.abs() < 0.5 && (full_target.y - mover_pos.y).abs() > 0.5 {
                        step.y = 0.5 * step.y.signum();
                    }

                    let target = Vec2 {
                        x: mover_pos.x + step.x,
                        y: mover_pos.y + step.y,
                    };

                    if self.field.carry(mover_id, target) {
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }
    }

    pub(super) fn request_toplevel_resize(&mut self, node_id: NodeId, width: i32, height: i32) {
        let width = width.max(96);
        let height = height.max(72);
        let focused_node = self.last_input_surface_node();

        for top in self.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();

            if self.surface_to_node.get(&key).copied() != Some(node_id) {
                continue;
            }

            top.with_pending_state(|s| {
                s.size = Some((width, height).into());
                if focused_node == Some(node_id) {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
            });
            top.send_configure();
            break;
        }
    }
}
