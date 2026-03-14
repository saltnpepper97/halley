use super::*;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CollisionExtents {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl CollisionExtents {
    #[inline]
    pub(crate) fn symmetric(size: Vec2) -> Self {
        Self {
            left: size.x * 0.5,
            right: size.x * 0.5,
            top: size.y * 0.5,
            bottom: size.y * 0.5,
        }
    }

    #[inline]
    fn size(self) -> Vec2 {
        Vec2 {
            x: (self.left + self.right).max(0.0),
            y: (self.top + self.bottom).max(0.0),
        }
    }
}

impl HalleyWlState {
    #[inline]
    fn node_participates_in_overlap(&self, id: NodeId) -> bool {
        self.field.node(id).is_some_and(|n| {
            self.field.is_visible(id)
                && matches!(
                    n.state,
                    halley_core::field::NodeState::Active
                        | halley_core::field::NodeState::Node
                        | halley_core::field::NodeState::Core
                        | halley_core::field::NodeState::Drifting
                )
        })
    }

    #[inline]
    fn node_is_docked_pair_member(&self, a: NodeId, b: NodeId) -> bool {
        self.field.dock_partner(a) == Some(b) || self.field.dock_partner(b) == Some(a)
    }

    pub(crate) fn non_overlap_gap_world(&self) -> f32 {
        // Overlap resolution must live purely in stable world-space. Camera
        // zoom must never change the required separation between nodes.
        self.tuning.non_overlap_gap_px.max(0.0)
    }

    #[inline]
    pub(crate) fn required_sep_x(
        &self,
        a_pos_x: f32,
        a_ext: CollisionExtents,
        b_pos_x: f32,
        b_ext: CollisionExtents,
        gap: f32,
    ) -> f32 {
        if b_pos_x >= a_pos_x {
            a_ext.right + b_ext.left + gap
        } else {
            a_ext.left + b_ext.right + gap
        }
    }

    #[inline]
    pub(crate) fn required_sep_y(
        &self,
        a_pos_y: f32,
        a_ext: CollisionExtents,
        b_pos_y: f32,
        b_ext: CollisionExtents,
        gap: f32,
    ) -> f32 {
        if b_pos_y >= a_pos_y {
            a_ext.bottom + b_ext.top + gap
        } else {
            a_ext.top + b_ext.bottom + gap
        }
    }

    pub(crate) fn carry_surface_non_overlap(
        &mut self,
        id: NodeId,
        to: Vec2,
        clamp_only: bool,
    ) -> bool {
        if clamp_only || self.suspend_overlap_resolve || self.suspend_state_checks {
            return self.carry_surface_no_overlap_static(id, to);
        }

        let Some(n) = self.field.node(id) else {
            return false;
        };
        let mover_ext = self.collision_extents_for_node(n);
        let gap = self.non_overlap_gap_world();
        let mut mover_pos = to;
        for _ in 0..24 {
            let others: Vec<(NodeId, Vec2, CollisionExtents, bool)> = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == id || !self.node_participates_in_overlap(oid) {
                        return None;
                    }
                    if self.node_is_docked_pair_member(id, oid) {
                        return None;
                    }
                    Some((
                        oid,
                        other.pos,
                        self.collision_extents_for_node(other),
                        other.pinned,
                    ))
                })
                .collect();

            let mut changed = false;

            for (oid, opos, oext, opinned) in others {
                let dx = mover_pos.x - opos.x;
                let dy = mover_pos.y - opos.y;
                let req_x = self.required_sep_x(mover_pos.x, mover_ext, opos.x, oext, gap);
                let req_y = self.required_sep_y(mover_pos.y, mover_ext, opos.y, oext, gap);
                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }

                if ox < oy {
                    let sign = if dx.abs() > f32::EPSILON {
                        dx.signum()
                    } else if oid.as_u64() < id.as_u64() {
                        1.0
                    } else {
                        -1.0
                    };
                    let step = ox + 0.3;
                    if opinned {
                        mover_pos.x += sign * step;
                    } else {
                        let half = sign * (step * 0.5);
                        mover_pos.x += half;
                        let _ = self.field.carry(
                            oid,
                            Vec2 {
                                x: opos.x - half,
                                y: opos.y,
                            },
                        );
                    }
                } else {
                    let sign = if dy.abs() > f32::EPSILON {
                        dy.signum()
                    } else {
                        1.0
                    };
                    let step = oy + 0.3;
                    if opinned {
                        mover_pos.y += sign * step;
                    } else {
                        let half = sign * (step * 0.5);
                        mover_pos.y += half;
                        let _ = self.field.carry(
                            oid,
                            Vec2 {
                                x: opos.x,
                                y: opos.y - half,
                            },
                        );
                    }
                }

                changed = true;
            }

            if self.field.node(id).is_some_and(|node| node.pinned) {
                return false;
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

        let mover_ext = self.collision_extents_for_node(n);
        let gap = self.non_overlap_gap_world();
        let mut mover_pos = to;

        for _ in 0..24 {
            let others: Vec<(NodeId, Vec2, CollisionExtents)> = self
                .field
                .nodes()
                .iter()
                .filter_map(|(&oid, other)| {
                    if oid == id || !self.node_participates_in_overlap(oid) {
                        return None;
                    }
                    if self.node_is_docked_pair_member(id, oid) {
                        return None;
                    }
                    Some((oid, other.pos, self.collision_extents_for_node(other)))
                })
                .collect();

            let mut changed = false;

            for (oid, opos, oext) in others {
                let dx = mover_pos.x - opos.x;
                let dy = mover_pos.y - opos.y;
                let req_x = self.required_sep_x(mover_pos.x, mover_ext, opos.x, oext, gap);
                let req_y = self.required_sep_y(mover_pos.y, mover_ext, opos.y, oext, gap);
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

    fn node_collision_extents(&self, label: &str, anim_scale: f32) -> CollisionExtents {
        let g = Self::proxy_collision_scale(anim_scale);

        let dot_half_px = (4.0 * g).round().clamp(4.0, 18.0);
        let label_h_px = (4.0 * g).round().clamp(4.0, 14.0);
        let label_gap_px = (8.0 + (g - 1.0) * 8.0).round().clamp(8.0, 28.0);
        let label_w_px = ((label.len() as f32 * 6.0) * (0.9 + 0.6 * g))
            .round()
            .clamp(24.0, 320.0);
        let pad_px = 6.0;

        let dot_d_px = (dot_half_px * 2.0).max(1.0);
        let marker_w_px = (dot_d_px + label_gap_px + label_w_px + pad_px * 2.0).max(8.0);
        let marker_h_px = (dot_d_px.max(label_h_px) + pad_px * 2.0).max(8.0);

        CollisionExtents::symmetric(Vec2 {
            // Keep collision bounds aligned with the rendered pill instead of the
            // larger hover proxy so node-to-window spacing matches window spacing.
            x: marker_w_px.max(1.0),
            y: marker_h_px.max(1.0),
        })
    }

    fn surface_window_collision_extents(&self, n: &halley_core::field::Node) -> CollisionExtents {
        let basis = self
            .last_active_size
            .get(&n.id)
            .copied()
            .unwrap_or(n.intrinsic_size);
        let bbox_w = n.intrinsic_size.x.max(1.0);
        let bbox_h = n.intrinsic_size.y.max(1.0);
        let (bbox_lx, bbox_ly) = self.bbox_loc.get(&n.id).copied().unwrap_or((0.0, 0.0));
        let (geo_lx, geo_ly, geo_w, geo_h) = self
            .window_geometry
            .get(&n.id)
            .copied()
            .unwrap_or((bbox_lx, bbox_ly, bbox_w, bbox_h));

        let left = (bbox_w * 0.5 + bbox_lx - geo_lx).max(16.0);
        let right = (geo_lx + geo_w - bbox_lx - bbox_w * 0.5).max(16.0);
        let top = (bbox_h * 0.5 + bbox_ly - geo_ly).max(16.0);
        let bottom = (geo_ly + geo_h - bbox_ly - bbox_h * 0.5).max(16.0);

        CollisionExtents {
            left: left * basis.x.max(1.0) / bbox_w,
            right: right * basis.x.max(1.0) / bbox_w,
            top: top * basis.y.max(1.0) / bbox_h,
            bottom: bottom * basis.y.max(1.0) / bbox_h,
        }
    }

    pub(crate) fn spawn_obstacle_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
        if n.kind == halley_core::field::NodeKind::Surface {
            self.surface_window_collision_extents(n)
        } else {
            self.collision_extents_for_node(n)
        }
    }

    pub(crate) fn collision_extents_for_node(
        &self,
        n: &halley_core::field::Node,
    ) -> CollisionExtents {
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
                let ext = self.surface_window_collision_extents(n);

                CollisionExtents {
                    left: ext.left * s,
                    right: ext.right * s,
                    top: ext.top * s,
                    bottom: ext.bottom * s,
                }
            }
            halley_core::field::NodeState::Node => {
                self.node_collision_extents(&n.label, anim.scale)
            }
            halley_core::field::NodeState::Core => {
                self.node_collision_extents(&n.label, anim.scale)
            }
            halley_core::field::NodeState::Drifting => CollisionExtents::symmetric(n.footprint),
        }
    }

    pub(super) fn collision_size_for_node(&self, n: &halley_core::field::Node) -> Vec2 {
        self.collision_extents_for_node(n).size()
    }

    pub(crate) fn resolve_surface_overlap(&mut self) {
        if !self.tuning.physics_enabled {
            return;
        }
        if self.suspend_overlap_resolve {
            return;
        }

        let mut ids: Vec<NodeId> = self
            .field
            .nodes()
            .keys()
            .copied()
            .filter(|&id| self.node_participates_in_overlap(id))
            .collect();

        if ids.len() < 2 {
            return;
        }

        ids.sort_by_key(|id| id.as_u64());

        let gap = self.non_overlap_gap_world();
        let focus_id = self.interaction_focus;
        for _ in 0..24 {
            let mut changed = false;

            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    let a = ids[i];
                    let b = ids[j];

                    if self.node_is_docked_pair_member(a, b) {
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
                    let a_pinned = na.pinned || self.resize_static_node == Some(a);
                    let b_pinned = nb.pinned || self.resize_static_node == Some(b);

                    if a_pinned && b_pinned {
                        continue;
                    }

                    let ea = self.collision_extents_for_node(na);
                    let eb = self.collision_extents_for_node(nb);

                    let dx = b_pos.x - a_pos.x;
                    let dy = b_pos.y - a_pos.y;

                    let req_x = self.required_sep_x(a_pos.x, ea, b_pos.x, eb, gap);
                    let req_y = self.required_sep_y(a_pos.y, ea, b_pos.y, eb, gap);
                    let ox = req_x - dx.abs();
                    let oy = req_y - dy.abs();

                    if self.resize_active == Some(a) || self.resize_active == Some(b) {
                        continue;
                    }

                    if ox <= 0.0 || oy <= 0.0 {
                        continue;
                    }

                    let sep_on_x = ox < oy;
                    let sep = if sep_on_x { ox + 0.3 } else { oy + 0.3 };
                    let dir_x = if dx.abs() > f32::EPSILON {
                        dx.signum()
                    } else if a.as_u64() < b.as_u64() {
                        -1.0
                    } else {
                        1.0
                    };
                    let dir_y = if dy.abs() > f32::EPSILON {
                        dy.signum()
                    } else {
                        1.0
                    };

                    if focus_id == Some(a) && !b_pinned {
                        let target = if sep_on_x {
                            Vec2 {
                                x: b_pos.x + dir_x * sep,
                                y: b_pos.y,
                            }
                        } else {
                            Vec2 {
                                x: b_pos.x,
                                y: b_pos.y + dir_y * sep,
                            }
                        };
                        if self.field.carry(b, target) {
                            changed = true;
                        }
                    } else if focus_id == Some(b) && !a_pinned {
                        let target = if sep_on_x {
                            Vec2 {
                                x: a_pos.x - dir_x * sep,
                                y: a_pos.y,
                            }
                        } else {
                            Vec2 {
                                x: a_pos.x,
                                y: a_pos.y - dir_y * sep,
                            }
                        };
                        if self.field.carry(a, target) {
                            changed = true;
                        }
                    } else if !a_pinned && !b_pinned {
                        let half = sep * 0.5;
                        let a_target = if sep_on_x {
                            Vec2 {
                                x: a_pos.x - dir_x * half,
                                y: a_pos.y,
                            }
                        } else {
                            Vec2 {
                                x: a_pos.x,
                                y: a_pos.y - dir_y * half,
                            }
                        };
                        let b_target = if sep_on_x {
                            Vec2 {
                                x: b_pos.x + dir_x * half,
                                y: b_pos.y,
                            }
                        } else {
                            Vec2 {
                                x: b_pos.x,
                                y: b_pos.y + dir_y * half,
                            }
                        };
                        let moved_a = self.field.carry(a, a_target);
                        let moved_b = self.field.carry(b, b_target);
                        if moved_a || moved_b {
                            changed = true;
                        }
                    } else if !a_pinned {
                        let target = if sep_on_x {
                            Vec2 {
                                x: a_pos.x - dir_x * sep,
                                y: a_pos.y,
                            }
                        } else {
                            Vec2 {
                                x: a_pos.x,
                                y: a_pos.y - dir_y * sep,
                            }
                        };
                        if self.field.carry(a, target) {
                            changed = true;
                        }
                    } else if !b_pinned {
                        let target = if sep_on_x {
                            Vec2 {
                                x: b_pos.x + dir_x * sep,
                                y: b_pos.y,
                            }
                        } else {
                            Vec2 {
                                x: b_pos.x,
                                y: b_pos.y + dir_y * sep,
                            }
                        };
                        if self.field.carry(b, target) {
                            changed = true;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_surface_nodes_use_marker_collision_extents() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.field.spawn_surface(
            "collapsed-firefox",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1200.0,
                y: 900.0,
            },
        );
        let _ = state
            .field
            .set_state(id, halley_core::field::NodeState::Node);

        let node = state.field.node(id).expect("node");
        let ext = state.collision_extents_for_node(node);

        assert!(
            ext.left + ext.right < 300.0,
            "collapsed node collision width should stay marker-sized, got {:?}",
            ext
        );
        assert!(
            ext.top + ext.bottom < 120.0,
            "collapsed node collision height should stay marker-sized, got {:?}",
            ext
        );
    }

    #[test]
    fn dragging_collapsed_node_cannot_overlap_active_surface() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let active = state.field.spawn_surface(
            "active",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 400.0, y: 260.0 },
        );
        let node = state.field.spawn_surface(
            "collapsed",
            Vec2 { x: 600.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let _ = state
            .field
            .set_state(node, halley_core::field::NodeState::Node);

        assert!(state.carry_surface_non_overlap(node, Vec2 { x: 0.0, y: 0.0 }, false));

        let active_node = state.field.node(active).expect("active surface");
        let collapsed_node = state.field.node(node).expect("collapsed node");
        let active_ext = state.collision_extents_for_node(active_node);
        let node_ext = state.collision_extents_for_node(collapsed_node);
        let gap = state.non_overlap_gap_world();
        let dx = (collapsed_node.pos.x - active_node.pos.x).abs();
        let dy = (collapsed_node.pos.y - active_node.pos.y).abs();
        let req_x = state.required_sep_x(
            active_node.pos.x,
            active_ext,
            collapsed_node.pos.x,
            node_ext,
            gap,
        );
        let req_y = state.required_sep_y(
            active_node.pos.y,
            active_ext,
            collapsed_node.pos.y,
            node_ext,
            gap,
        );

        assert!(
            dx >= req_x || dy >= req_y,
            "collapsed node overlapped active surface after drag: active={:?} node={:?} req=({}, {})",
            active_node.pos,
            collapsed_node.pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn resolve_overlap_now_separates_collapsed_nodes() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let a = state.field.spawn_surface(
            "alpha",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let b =
            state
                .field
                .spawn_surface("beta", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 320.0, y: 220.0 });
        let _ = state
            .field
            .set_state(a, halley_core::field::NodeState::Node);
        let _ = state
            .field
            .set_state(b, halley_core::field::NodeState::Node);

        state.resolve_overlap_now();

        let na = state.field.node(a).expect("node a");
        let nb = state.field.node(b).expect("node b");
        let ea = state.collision_extents_for_node(na);
        let eb = state.collision_extents_for_node(nb);
        let gap = state.non_overlap_gap_world();
        let dx = (nb.pos.x - na.pos.x).abs();
        let dy = (nb.pos.y - na.pos.y).abs();
        let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
        let req_y = state.required_sep_y(na.pos.y, ea, nb.pos.y, eb, gap);

        assert!(
            dx >= req_x || dy >= req_y,
            "collapsed nodes still overlap after resolve: a={:?} b={:?} req=({}, {})",
            na.pos,
            nb.pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn resolve_overlap_now_separates_active_surface_and_node_immediately() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let active = state.field.spawn_surface(
            "active",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let node =
            state
                .field
                .spawn_surface("node", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 300.0, y: 200.0 });
        let _ = state
            .field
            .set_state(node, halley_core::field::NodeState::Node);

        state.resolve_overlap_now();

        let active_node = state.field.node(active).expect("active");
        let collapsed_node = state.field.node(node).expect("node");
        let active_ext = state.collision_extents_for_node(active_node);
        let node_ext = state.collision_extents_for_node(collapsed_node);
        let gap = state.non_overlap_gap_world();
        let dx = (collapsed_node.pos.x - active_node.pos.x).abs();
        let dy = (collapsed_node.pos.y - active_node.pos.y).abs();
        let req_x = state.required_sep_x(
            active_node.pos.x,
            active_ext,
            collapsed_node.pos.x,
            node_ext,
            gap,
        );
        let req_y = state.required_sep_y(
            active_node.pos.y,
            active_ext,
            collapsed_node.pos.y,
            node_ext,
            gap,
        );

        assert!(
            dx >= req_x || dy >= req_y,
            "active surface and node still overlap after resolve: active={:?} node={:?} req=({}, {})",
            active_node.pos,
            collapsed_node.pos,
            req_x,
            req_y
        );
    }

    #[test]
    fn resolve_overlap_now_separates_two_active_surfaces_immediately() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<HalleyWlState>::new()
            .expect("display")
            .handle();
        let mut state = HalleyWlState::new(&dh, tuning);
        state.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        state.zoom_ref_size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let a =
            state
                .field
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });

        state.resolve_overlap_now();

        let na = state.field.node(a).expect("a");
        let nb = state.field.node(b).expect("b");
        let ea = state.collision_extents_for_node(na);
        let eb = state.collision_extents_for_node(nb);
        let gap = state.non_overlap_gap_world();
        let dx = (nb.pos.x - na.pos.x).abs();
        let dy = (nb.pos.y - na.pos.y).abs();
        let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
        let req_y = state.required_sep_y(na.pos.y, ea, nb.pos.y, eb, gap);

        assert!(
            dx >= req_x || dy >= req_y,
            "active surfaces still overlap after resolve: a={:?} b={:?} req=({}, {})",
            na.pos,
            nb.pos,
            req_x,
            req_y
        );
    }
}
