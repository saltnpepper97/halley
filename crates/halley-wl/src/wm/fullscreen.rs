use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;

use super::*;

impl HalleyWlState {
    const FULLSCREEN_ENTER_MS: u64 = 220;
    const FULLSCREEN_EXIT_MS: u64 = 320;

    fn viewport_rect_for(&self, center: Vec2, size: Vec2) -> halley_core::field::Rect {
        let half = Vec2 {
            x: size.x * 0.5,
            y: size.y * 0.5,
        };
        halley_core::field::Rect {
            min: Vec2 {
                x: center.x - half.x,
                y: center.y - half.y,
            },
            max: Vec2 {
                x: center.x + half.x,
                y: center.y + half.y,
            },
        }
    }

    fn fullscreen_monitor_name(&self, node_id: NodeId, output: Option<&WlOutput>) -> String {
        output
            .and_then(|requested_output| {
                self.monitor_state
                    .outputs
                    .iter()
                    .find_map(|(name, output)| {
                        output.owns(requested_output).then_some(name.clone())
                    })
            })
            .or_else(|| self.monitor_state.node_monitor.get(&node_id).cloned())
            .unwrap_or_else(|| self.monitor_state.current_monitor.clone())
    }

    fn fullscreen_monitor_view(&self, monitor_name: &str) -> (Vec2, Vec2) {
        self.monitor_state
            .monitors
            .get(monitor_name)
            .map(|monitor| (monitor.viewport.center, monitor.viewport.size))
            .unwrap_or((self.viewport.center, self.viewport.size))
    }

    fn reset_monitor_zoom_once(&mut self, monitor_name: &str) {
        if let Some(monitor) = self.monitor_state.monitors.get_mut(monitor_name) {
            monitor.zoom_ref_size = monitor.viewport.size;
            monitor.camera_target_view_size = monitor.viewport.size;
        }
        if self.monitor_state.current_monitor == monitor_name {
            self.zoom_ref_size = self.viewport.size;
            self.camera_target_view_size = self.viewport.size;
        }
    }

    fn node_intersects_monitor_viewport(&self, id: NodeId, monitor_name: &str) -> bool {
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
        let (center, size) = self.fullscreen_monitor_view(monitor_name);
        rect.intersects(self.viewport_rect_for(center, size))
    }

    fn fullscreen_target_size_for(&self, monitor_name: &str) -> (i32, i32) {
        self.monitor_state
            .outputs
            .get(monitor_name)
            .and_then(|output| output.current_mode())
            .map(|mode| (mode.size.w, mode.size.h))
            .unwrap_or_else(|| {
                let (_, size) = self.fullscreen_monitor_view(monitor_name);
                (
                    size.x.round().max(96.0) as i32,
                    size.y.round().max(72.0) as i32,
                )
            })
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

    fn fullscreen_displaced_target(
        &self,
        pos: Vec2,
        ordinal: usize,
        viewport_center: Vec2,
        viewport_size: Vec2,
    ) -> Vec2 {
        let mut dir = Vec2 {
            x: pos.x - viewport_center.x,
            y: pos.y - viewport_center.y,
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

        let radius = viewport_size.x.hypot(viewport_size.y) * 0.85 + 320.0;
        Vec2 {
            x: viewport_center.x + dir.x * radius,
            y: viewport_center.y + dir.y * radius,
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

    /// Returns the monitor name that `node_id` is currently fullscreened on, if any.
    pub(crate) fn fullscreen_monitor_for_node(&self, node_id: NodeId) -> Option<&str> {
        self.fullscreen_active_node
            .iter()
            .find_map(|(monitor, &id)| (id == node_id).then_some(monitor.as_str()))
    }

    /// True if `node_id` is the active fullscreen on any monitor.
    pub(crate) fn is_fullscreen_active(&self, node_id: NodeId) -> bool {
        self.fullscreen_monitor_for_node(node_id).is_some()
    }

    fn exit_xdg_fullscreen_inner(&mut self, node_id: NodeId, now: Instant, suspend: bool) {
        // Find which monitor this node is fullscreened on.
        let monitor_name = match self.fullscreen_monitor_for_node(node_id) {
            Some(m) => m.to_owned(),
            None => return, // not active fullscreen on any monitor
        };

        self.interaction_state.reset_input_state_requested = true;

        if suspend {
            self.fullscreen_suspended_node
                .insert(monitor_name.clone(), node_id);
        } else {
            // If we're doing a hard exit, clear any suspended state for this monitor too.
            self.fullscreen_suspended_node.remove(&monitor_name);
        }

        let now_ms = self.now_ms(now);

        // Restore all nodes that were displaced when this monitor went fullscreen.
        // We identify bystanders as nodes in fullscreen_restore whose saved
        // viewport_center matches this monitor's viewport center.
        let (monitor_viewport_center, _) = self.fullscreen_monitor_view(&monitor_name);
        let restore_entries: Vec<(NodeId, crate::state::FullscreenSessionEntry)> = self
            .fullscreen_restore
            .iter()
            .filter(|(_, entry)| {
                (entry.viewport_center.x - monitor_viewport_center.x).abs() < 1.0
                    && (entry.viewport_center.y - monitor_viewport_center.y).abs() < 1.0
            })
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

        self.fullscreen_active_node.remove(&monitor_name);
        self.fullscreen_scale_anim.remove(&node_id);
        self.request_maintenance();
    }

    pub(crate) fn suspend_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        self.exit_xdg_fullscreen_inner(node_id, now, true);
    }

    fn restore_fullscreen_snapshot(
        &mut self,
        id: NodeId,
        entry: crate::state::FullscreenSessionEntry,
    ) {
        if let Some(node) = self.field.node_mut(id) {
            node.intrinsic_size = entry.intrinsic_size;
        }
        if let Some(loc) = entry.bbox_loc {
            self.render_state.bbox_loc.insert(id, loc);
        } else {
            self.render_state.bbox_loc.remove(&id);
        }
        if let Some(geo) = entry.window_geometry {
            self.render_state.window_geometry.insert(id, geo);
        } else {
            self.render_state.window_geometry.remove(&id);
        }
        self.set_last_active_size_now(id, entry.intrinsic_size);
    }

    pub(crate) fn enter_xdg_fullscreen(
        &mut self,
        node_id: NodeId,
        output: Option<WlOutput>,
        now: Instant,
    ) {
        let monitor_name = self.fullscreen_monitor_name(node_id, output.as_ref());

        // Already fullscreen on this monitor — no-op.
        if self.fullscreen_active_node.get(&monitor_name) == Some(&node_id) {
            return;
        }

        // Clear any suspended state for this monitor.
        self.fullscreen_suspended_node.remove(&monitor_name);

        // If another window is fullscreened on the same monitor, exit it first.
        if let Some(existing) = self.fullscreen_active_node.get(&monitor_name).copied() {
            self.exit_xdg_fullscreen(existing, now);
        }

        let now_ms = self.now_ms(now);
        let target_size = self.fullscreen_target_size_for(monitor_name.as_str());
        let (viewport_center, viewport_size) = self.fullscreen_monitor_view(monitor_name.as_str());

        // One-time reset of the target monitor's zoom to 1.0. Do not hold or lock it.
        self.reset_monitor_zoom_once(monitor_name.as_str());

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
                bbox_loc: self.render_state.bbox_loc.get(&node_id).copied(),
                window_geometry: self.render_state.window_geometry.get(&node_id).copied(),
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

        // Displace other windows that are on this monitor and intersect its viewport.
        // Windows on other monitors are left completely alone.
        let others: Vec<NodeId> = self
            .field
            .nodes()
            .iter()
            .filter_map(|(&id, n)| {
                (id != node_id
                    && n.kind == halley_core::field::NodeKind::Surface
                    && self.field.is_visible(id)
                    && self
                        .monitor_state
                        .node_monitor
                        .get(&id)
                        .is_none_or(|m| m == &monitor_name)
                    && self.node_intersects_monitor_viewport(id, monitor_name.as_str()))
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
                    bbox_loc: self.render_state.bbox_loc.get(&other_id).copied(),
                    window_geometry: self.render_state.window_geometry.get(&other_id).copied(),
                    pinned: other.pinned,
                },
            );
            let _ = self.field.set_pinned(other_id, false);
            self.queue_fullscreen_motion(
                other_id,
                other.pos,
                self.fullscreen_displaced_target(other.pos, idx, viewport_center, viewport_size),
                now_ms,
                Self::FULLSCREEN_ENTER_MS,
            );
        }

        self.request_toplevel_fullscreen_state(node_id, true, output, Some(target_size));
        self.fullscreen_active_node.insert(monitor_name, node_id);
        self.set_interaction_focus(Some(node_id), 30_000, now);
        self.request_maintenance();
    }

    pub(crate) fn exit_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        // Clear suspended state on whatever monitor this node is on.
        if let Some(monitor) = self
            .fullscreen_monitor_for_node(node_id)
            .map(|s| s.to_owned())
        {
            self.fullscreen_suspended_node.remove(&monitor);
        }
        self.exit_xdg_fullscreen_inner(node_id, now, false);
    }

    pub(crate) fn drop_fullscreen_surface(&mut self, id: NodeId, now: Instant) {
        // Clear suspended state if this node was suspended on any monitor.
        self.fullscreen_suspended_node
            .retain(|_, &mut nid| nid != id);

        if self.is_fullscreen_active(id) {
            let monitor_name = self
                .fullscreen_monitor_for_node(id)
                .map(|s| s.to_owned())
                .unwrap(); // safe: is_fullscreen_active just confirmed it

            self.interaction_state.reset_input_state_requested = true;
            self.fullscreen_active_node.remove(&monitor_name);

            // Restore only bystanders that were displaced for this monitor's fullscreen.
            let (monitor_viewport_center, _) = self.fullscreen_monitor_view(&monitor_name);
            let restore_entries: Vec<(NodeId, crate::state::FullscreenSessionEntry)> = self
                .fullscreen_restore
                .iter()
                .filter(|&(&other_id, ref entry)| {
                    other_id != id
                        && (entry.viewport_center.x - monitor_viewport_center.x).abs() < 1.0
                        && (entry.viewport_center.y - monitor_viewport_center.y).abs() < 1.0
                })
                .map(|(&other_id, &entry)| (other_id, entry))
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
                // A node finishing its motion should be pinned only if the fullscreen
                // it was displaced for is still active — i.e. the monitor it belongs
                // to still has an active fullscreen session.
                let node_monitor = self
                    .monitor_state
                    .node_monitor
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| self.monitor_state.current_monitor.clone());
                let displaced_for_active = self.fullscreen_active_node.contains_key(&node_monitor);

                if displaced_for_active {
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
