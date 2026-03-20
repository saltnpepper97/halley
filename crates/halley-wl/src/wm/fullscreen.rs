use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;

use super::*;

impl HalleyWlState {
    const FULLSCREEN_ENTER_MS: u64 = 220;
    const FULLSCREEN_EXIT_MS: u64 = 320;

    fn viewport_rect(&self) -> halley_core::field::Rect {
        let half = Vec2 {
            x: self.viewport.size.x * 0.5,
            y: self.viewport.size.y * 0.5,
        };
        halley_core::field::Rect {
            min: Vec2 {
                x: self.viewport.center.x - half.x,
                y: self.viewport.center.y - half.y,
            },
            max: Vec2 {
                x: self.viewport.center.x + half.x,
                y: self.viewport.center.y + half.y,
            },
        }
    }

    fn node_intersects_viewport(&self, id: NodeId) -> bool {
        let Some(node) = self.field.node(id) else {
            return false;
        };
        let ext = self.collision_extents_for_node(node);
        let rect = halley_core::field::Rect {
            min: Vec2 {
                x: node.pos.x - ext.left,
                y: node.pos.y - ext.top,
            },
            max: Vec2 {
                x: node.pos.x + ext.right,
                y: node.pos.y + ext.bottom,
            },
        };
        rect.intersects(self.viewport_rect())
    }

    fn fullscreen_target_size(&self) -> (i32, i32) {
        (
            self.viewport.size.x.round().max(96.0) as i32,
            self.viewport.size.y.round().max(72.0) as i32,
        )
    }

    fn queue_fullscreen_motion(
        &mut self,
        id: NodeId,
        from: Vec2,
        to: Vec2,
        now_ms: u64,
        duration_ms: u64,
    ) {
        self.fullscreen_motion.insert(
            id,
            crate::state::FullscreenMotion {
                from,
                to,
                start_ms: now_ms,
                duration_ms: duration_ms.max(1),
            },
        );
    }

    pub(crate) fn fullscreen_entry_scale(&self, node_id: NodeId, now_ms: u64) -> f32 {
        let Some(anim) = self.fullscreen_scale_anim.get(&node_id).copied() else {
            return 1.0;
        };
        let elapsed = now_ms.saturating_sub(anim.start_ms);
        let t = (elapsed as f32 / anim.duration_ms.max(1) as f32).clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        0.94 + (1.0 - 0.94) * e
    }

    fn fullscreen_displaced_target(&self, pos: Vec2, ordinal: usize) -> Vec2 {
        let mut dir = Vec2 {
            x: pos.x - self.viewport.center.x,
            y: pos.y - self.viewport.center.y,
        };
        let len = dir.x.hypot(dir.y);
        if len < 1.0 {
            let dirs = [
                Vec2 { x: 1.0, y: 0.0 },
                Vec2 { x: -1.0, y: 0.0 },
                Vec2 { x: 0.0, y: -1.0 },
                Vec2 { x: 0.0, y: 1.0 },
            ];
            dir = dirs[ordinal % dirs.len()];
        } else {
            dir.x /= len;
            dir.y /= len;
        }

        let radius = self.viewport.size.x.hypot(self.viewport.size.y) * 0.85 + 320.0;
        Vec2 {
            x: self.viewport.center.x + dir.x * radius,
            y: self.viewport.center.y + dir.y * radius,
        }
    }

    fn request_toplevel_fullscreen_state(
        &mut self,
        node_id: NodeId,
        fullscreen: bool,
        output: Option<WlOutput>,
        size: Option<(i32, i32)>,
    ) {
        let focused_node = self.last_input_surface_node();
        for top in self.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();
            if self.surface_to_node.get(&key).copied() != Some(node_id) {
                continue;
            }
            top.with_pending_state(|s| {
                s.size = size.map(|(w, h)| (w.max(96), h.max(72)).into());
                if focused_node == Some(node_id) {
                    s.states.set(xdg_toplevel::State::Activated);
                } else {
                    s.states.unset(xdg_toplevel::State::Activated);
                }
                if fullscreen {
                    s.states.set(xdg_toplevel::State::Fullscreen);
                    s.fullscreen_output = output;
                } else {
                    s.states.unset(xdg_toplevel::State::Fullscreen);
                    s.fullscreen_output = None;
                }
            });
            top.send_configure();
            break;
        }
    }

    fn exit_xdg_fullscreen_inner(&mut self, node_id: NodeId, now: Instant, suspend: bool) {
        if self.fullscreen_active_node != Some(node_id) {
            return;
        }

        self.reset_input_state_requested = true;
        self.fullscreen_suspended_node = suspend.then_some(node_id);
        let now_ms = self.now_ms(now);
        let restore_entries: Vec<(NodeId, crate::state::FullscreenSessionEntry)> = self
            .fullscreen_restore
            .iter()
            .map(|(&id, &entry)| (id, entry))
            .collect();

        for (id, entry) in &restore_entries {
            let _ = self.field.set_pinned(*id, false);
            let from = self.field.node(*id).map(|n| n.pos).unwrap_or(entry.pos);
            self.restore_fullscreen_snapshot(*id, *entry);
            self.queue_fullscreen_motion(*id, from, entry.pos, now_ms, Self::FULLSCREEN_EXIT_MS);
        }

        if let Some(entry) = self.fullscreen_restore.get(&node_id).copied() {
            self.request_toplevel_fullscreen_state(
                node_id,
                false,
                None,
                Some((
                    entry.size.x.round().max(96.0) as i32,
                    entry.size.y.round().max(72.0) as i32,
                )),
            );
        } else {
            self.request_toplevel_fullscreen_state(node_id, false, None, None);
        }

        self.fullscreen_active_node = None;
        self.fullscreen_scale_anim.remove(&node_id);
        self.request_maintenance();
    }

    pub(crate) fn suspend_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        self.exit_xdg_fullscreen_inner(node_id, now, true);
    }

    fn restore_fullscreen_snapshot(&mut self, id: NodeId, entry: crate::state::FullscreenSessionEntry) {
        if let Some(node) = self.field.node_mut(id) {
            node.intrinsic_size = entry.intrinsic_size;
        }
        if let Some(loc) = entry.bbox_loc {
            self.bbox_loc.insert(id, loc);
        } else {
            self.bbox_loc.remove(&id);
        }
        if let Some(geo) = entry.window_geometry {
            self.window_geometry.insert(id, geo);
        } else {
            self.window_geometry.remove(&id);
        }
        self.set_last_active_size_now(id, entry.intrinsic_size);
    }

    pub(crate) fn enter_xdg_fullscreen(
        &mut self,
        node_id: NodeId,
        output: Option<WlOutput>,
        now: Instant,
    ) {
        if self.fullscreen_active_node == Some(node_id) {
            return;
        }
        self.fullscreen_suspended_node = None;
        if let Some(active) = self.fullscreen_active_node {
            self.exit_xdg_fullscreen(active, now);
        }

        let now_ms = self.now_ms(now);
        let target_size = self.fullscreen_target_size();
        let viewport_center = self.viewport.center;
        self.zoom_ref_size = self.viewport.size;
        self.camera_target_view_size = self.zoom_ref_size;

        let Some(node) = self.field.node(node_id).cloned() else {
            return;
        };

        let saved_size = crate::surface::current_surface_size_for_node(self, node_id)
            .unwrap_or(node.intrinsic_size);
        self.fullscreen_restore.insert(
            node_id,
            crate::state::FullscreenSessionEntry {
                pos: node.pos,
                size: saved_size,
                viewport_center,
                intrinsic_size: node.intrinsic_size,
                bbox_loc: self.bbox_loc.get(&node_id).copied(),
                window_geometry: self.window_geometry.get(&node_id).copied(),
                pinned: node.pinned,
            },
        );
        let _ = self.field.set_pinned(node_id, false);
        self.queue_fullscreen_motion(
            node_id,
            node.pos,
            viewport_center,
            now_ms,
            Self::FULLSCREEN_ENTER_MS,
        );
        self.fullscreen_scale_anim.insert(
            node_id,
            crate::state::FullscreenScaleAnim {
                start_ms: now_ms,
                duration_ms: Self::FULLSCREEN_ENTER_MS,
            },
        );

        let others: Vec<NodeId> = self
            .field
            .nodes()
            .iter()
            .filter_map(|(&id, n)| {
                (id != node_id
                    && n.kind == halley_core::field::NodeKind::Surface
                    && self.field.is_visible(id)
                    && self.node_intersects_viewport(id))
                .then_some(id)
            })
            .collect();

        for (idx, other_id) in others.into_iter().enumerate() {
            let Some(other) = self.field.node(other_id).cloned() else {
                continue;
            };
            self.fullscreen_restore.insert(
                other_id,
                crate::state::FullscreenSessionEntry {
                    pos: other.pos,
                    size: crate::surface::current_surface_size_for_node(self, other_id)
                        .unwrap_or(other.intrinsic_size),
                    viewport_center,
                    intrinsic_size: other.intrinsic_size,
                    bbox_loc: self.bbox_loc.get(&other_id).copied(),
                    window_geometry: self.window_geometry.get(&other_id).copied(),
                    pinned: other.pinned,
                },
            );
            let _ = self.field.set_pinned(other_id, false);
            self.queue_fullscreen_motion(
                other_id,
                other.pos,
                self.fullscreen_displaced_target(other.pos, idx),
                now_ms,
                Self::FULLSCREEN_ENTER_MS,
            );
        }

        self.request_toplevel_fullscreen_state(node_id, true, output, Some(target_size));
        self.fullscreen_active_node = Some(node_id);
        self.set_interaction_focus(Some(node_id), 30_000, now);
        self.request_maintenance();
    }

    pub(crate) fn exit_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        self.fullscreen_suspended_node = None;
        self.exit_xdg_fullscreen_inner(node_id, now, false);
    }

    pub(crate) fn drop_fullscreen_surface(&mut self, id: NodeId, now: Instant) {
        if self.fullscreen_suspended_node == Some(id) {
            self.fullscreen_suspended_node = None;
        }
        if self.fullscreen_active_node == Some(id) {
            self.reset_input_state_requested = true;
            self.fullscreen_active_node = None;
            let restore_entries: Vec<(NodeId, crate::state::FullscreenSessionEntry)> = self
                .fullscreen_restore
                .iter()
                .filter_map(|(&other_id, &entry)| (other_id != id).then_some((other_id, entry)))
                .collect();
            let now_ms = self.now_ms(now);
            for (other_id, entry) in restore_entries {
                let _ = self.field.set_pinned(other_id, false);
                let from = self
                    .field
                    .node(other_id)
                    .map(|n| n.pos)
                    .unwrap_or(entry.pos);
                self.restore_fullscreen_snapshot(other_id, entry);
                self.queue_fullscreen_motion(
                    other_id,
                    from,
                    entry.pos,
                    now_ms,
                    Self::FULLSCREEN_EXIT_MS,
                );
            }
        }
        self.fullscreen_restore.remove(&id);
        self.fullscreen_motion.remove(&id);
        self.fullscreen_scale_anim.remove(&id);
    }

    pub(crate) fn tick_fullscreen_motion(&mut self, now: Instant) {
        if self.fullscreen_active_node.is_some() || !self.fullscreen_motion.is_empty() {
            self.zoom_ref_size = self.viewport.size;
            self.camera_target_view_size = self.zoom_ref_size;
        }
        if self.fullscreen_motion.is_empty() {
            return;
        }

        let now_ms = self.now_ms(now);
        let motions: Vec<(NodeId, crate::state::FullscreenMotion)> = self
            .fullscreen_motion
            .iter()
            .map(|(&id, &motion)| (id, motion))
            .collect();
        let mut finished = Vec::new();

        for (id, motion) in motions {
            let elapsed = now_ms.saturating_sub(motion.start_ms);
            let t = (elapsed as f32 / motion.duration_ms.max(1) as f32).clamp(0.0, 1.0);
            let e = if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
            };
            let pos = Vec2 {
                x: motion.from.x + (motion.to.x - motion.from.x) * e,
                y: motion.from.y + (motion.to.y - motion.from.y) * e,
            };
            let _ = self.field.carry(id, pos);
            if t >= 1.0 {
                finished.push(id);
            }
        }

        for id in finished {
            self.fullscreen_motion.remove(&id);
            if let Some(entry) = self.fullscreen_restore.get(&id).copied() {
                if self.fullscreen_active_node.is_some() {
                    let _ = self.field.set_pinned(id, true);
                } else {
                    let _ = self.field.set_pinned(id, entry.pinned);
                    self.fullscreen_restore.remove(&id);
                }
            }
        }

        self.fullscreen_scale_anim
            .retain(|_, anim| now_ms < anim.start_ms.saturating_add(anim.duration_ms));
    }
}
