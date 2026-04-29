use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::compositor::get_parent;
use std::ops::{Deref, DerefMut};

use super::*;
use crate::compositor::ctx::FullscreenCtx;

pub(crate) fn enter_xdg_fullscreen(
    ctx: &mut FullscreenCtx<'_>,
    node_id: NodeId,
    output: Option<WlOutput>,
    now: Instant,
) {
    ctx.st.enter_xdg_fullscreen(node_id, output, now);
}

pub(crate) fn exit_xdg_fullscreen(ctx: &mut FullscreenCtx<'_>, node_id: NodeId, now: Instant) {
    ctx.st.exit_xdg_fullscreen(node_id, now);
}

pub(crate) fn on_seat_focus_changed(
    ctx: &mut FullscreenCtx<'_>,
    focused: Option<&WlSurface>,
    now: Instant,
) {
    let st = &mut ctx.st;
    let focused_is_layer = focused.is_some_and(|surface| {
        crate::compositor::monitor::layer_shell::is_layer_surface(st, surface)
    });
    if focused_is_layer {
        return;
    }

    let focused_root = focused.map(surface_tree_root);
    let focused_id = focused_root.as_ref().map(|wl| wl.id());
    let focused_node_id = focused_id
        .as_ref()
        .and_then(|fid| st.model.surface_to_node.get(fid).copied());
    let focused_monitor: Option<String> = focused_node_id.as_ref().and_then(|node_id| {
        Some(
            st.model
                .monitor_state
                .node_monitor
                .get(node_id)
                .cloned()
                .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone()),
        )
    });

    let to_suspend: Vec<NodeId> = st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .iter()
        .filter_map(|(monitor, &fullscreen_id)| {
            let same_monitor = focused_monitor
                .as_deref()
                .is_some_and(|fm| fm == monitor.as_str());
            if !same_monitor {
                return None;
            }
            if focused_node_preserves_fullscreen_lock(st, focused_node_id, monitor.as_str()) {
                return None;
            }
            let fullscreen_surface_id = st
                .platform
                .xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .find_map(|top| {
                    (st.model
                        .surface_to_node
                        .get(&top.wl_surface().id())
                        .copied()
                        == Some(fullscreen_id))
                    .then(|| top.wl_surface().id())
                });
            (fullscreen_surface_id.is_some() && fullscreen_surface_id != focused_id)
                .then_some(fullscreen_id)
        })
        .collect();

    for fullscreen_id in to_suspend {
        st.suspend_xdg_fullscreen(fullscreen_id, now);
    }
}

fn surface_tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

fn focused_node_preserves_fullscreen_lock(
    st: &Halley,
    focused_node_id: Option<NodeId>,
    monitor: &str,
) -> bool {
    focused_node_id.is_some_and(|focused_node| {
        st.node_draws_above_fullscreen_on_monitor(focused_node, monitor)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::spawn::state::AppliedInitialWindowRule;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    #[test]
    fn overlap_policy_focus_preserves_same_monitor_fullscreen_lock() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let overlap = state.model.field.spawn_surface(
            "overlap",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );
        for id in [fullscreen, overlap] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor.clone(), fullscreen);
        state.model.spawn_state.applied_window_rules.insert(
            overlap,
            AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                parent_node: None,
                suppress_reveal_pan: true,
            },
        );

        assert!(focused_node_preserves_fullscreen_lock(
            &state,
            Some(overlap),
            monitor.as_str()
        ));
        assert!(!focused_node_preserves_fullscreen_lock(
            &state,
            Some(fullscreen),
            monitor.as_str()
        ));
    }

    #[test]
    fn overlap_policy_focus_does_not_preserve_fullscreen_lock_in_stacking_layout() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let overlap = state.model.field.spawn_surface(
            "overlap",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );
        for id in [fullscreen, overlap] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor.clone(), fullscreen);
        state.model.spawn_state.applied_window_rules.insert(
            overlap,
            AppliedInitialWindowRule {
                overlap_policy: halley_config::InitialWindowOverlapPolicy::All,
                spawn_placement: halley_config::InitialWindowSpawnPlacement::Adjacent,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Float,
                parent_node: None,
                suppress_reveal_pan: true,
            },
        );

        assert!(!focused_node_preserves_fullscreen_lock(
            &state,
            Some(overlap),
            monitor.as_str()
        ));
    }

    #[test]
    fn live_overlap_pauses_during_fullscreen_motion() {
        let mut tuning = single_monitor_tuning();
        tuning.physics_enabled = false;
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let a = state.model.field.spawn_surface(
            "a",
            Vec2 { x: 200.0, y: 200.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let b = state.model.field.spawn_surface(
            "b",
            Vec2 { x: 220.0, y: 220.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(a, "monitor_a");
        state.assign_node_to_monitor(b, "monitor_a");
        let a_before = state.model.field.node(a).expect("a").pos;
        let b_before = state.model.field.node(b).expect("b").pos;
        let now = Instant::now();
        state.model.fullscreen_state.fullscreen_motion.insert(
            a,
            crate::compositor::fullscreen::state::FullscreenMotion {
                from: a_before,
                to: Vec2 { x: 400.0, y: 300.0 },
                start_ms: state.now_ms(now),
                duration_ms: 320,
            },
        );

        crate::frame_loop::tick_live_overlap(&mut state);

        assert_eq!(state.model.field.node(a).expect("a").pos, a_before);
        assert_eq!(state.model.field.node(b).expect("b").pos, b_before);
    }

    #[test]
    fn fullscreen_exit_restores_displaced_nodes_after_motion() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tuning());

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 140.0, y: 150.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = state.model.field.spawn_surface(
            "bystander",
            Vec2 { x: 520.0, y: 280.0 },
            Vec2 { x: 220.0, y: 160.0 },
        );
        state.assign_node_to_monitor(fullscreen, "monitor_a");
        state.assign_node_to_monitor(bystander, "monitor_a");
        let _ = state.model.field.set_pinned(bystander, true);

        let fullscreen_pos = state.model.field.node(fullscreen).expect("fullscreen").pos;
        let bystander_pos = state.model.field.node(bystander).expect("bystander").pos;
        let bystander_size = state
            .model
            .field
            .node(bystander)
            .expect("bystander")
            .intrinsic_size;
        let now = Instant::now();

        state.enter_xdg_fullscreen(fullscreen, None, now);
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(260));

        if let Some(space) = state.model.monitor_state.monitors.get_mut("monitor_a") {
            space.viewport.center = Vec2 {
                x: 1234.0,
                y: -567.0,
            };
            space.camera_target_center = space.viewport.center;
        }

        assert_ne!(
            state.model.field.node(bystander).expect("bystander").pos,
            bystander_pos
        );
        assert!(state.model.field.node(bystander).expect("bystander").pinned);

        state.exit_xdg_fullscreen(fullscreen, now + std::time::Duration::from_millis(300));
        state.input.interaction_state.physics_velocity.insert(
            bystander,
            Vec2 {
                x: 1200.0,
                y: -800.0,
            },
        );
        state.tick_fullscreen_motion(now + std::time::Duration::from_millis(700));

        assert_eq!(
            state.model.field.node(fullscreen).expect("fullscreen").pos,
            fullscreen_pos
        );
        let restored_bystander = state.model.field.node(bystander).expect("bystander");
        assert_eq!(restored_bystander.pos, bystander_pos);
        assert_eq!(restored_bystander.intrinsic_size, bystander_size);
        assert!(restored_bystander.pinned);
        assert!(
            !state
                .model
                .fullscreen_state
                .fullscreen_restore
                .contains_key(&bystander)
        );
        assert!(
            !state
                .input
                .interaction_state
                .physics_velocity
                .contains_key(&bystander)
        );
    }
}

pub(crate) struct FullscreenController<T> {
    st: T,
}

pub(crate) fn fullscreen_controller<T>(st: T) -> FullscreenController<T> {
    FullscreenController { st }
}

impl<T: Deref<Target = Halley>> Deref for FullscreenController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for FullscreenController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

pub(crate) fn fullscreen_entry_scale(st: &Halley, node_id: NodeId, now_ms: u64) -> f32 {
    let Some(anim) = st
        .model
        .fullscreen_state
        .fullscreen_scale_anim
        .get(&node_id)
        .copied()
    else {
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

pub(crate) fn fullscreen_monitor_for_node(st: &Halley, node_id: NodeId) -> Option<&str> {
    st.model
        .fullscreen_state
        .fullscreen_active_node
        .iter()
        .find_map(|(monitor, &id)| (id == node_id).then_some(monitor.as_str()))
}

pub(crate) fn is_fullscreen_active(st: &Halley, node_id: NodeId) -> bool {
    fullscreen_monitor_for_node(st, node_id).is_some()
}

pub(crate) fn is_fullscreen_session_node(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .fullscreen_state
        .fullscreen_active_node
        .values()
        .any(|&id| id == node_id)
        || st
            .model
            .fullscreen_state
            .fullscreen_suspended_node
            .values()
            .any(|&id| id == node_id)
}

impl<T: DerefMut<Target = Halley>> FullscreenController<T> {
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
                self.model
                    .monitor_state
                    .outputs
                    .iter()
                    .find_map(|(name, output)| {
                        output.owns(requested_output).then_some(name.clone())
                    })
            })
            .or_else(|| self.model.monitor_state.node_monitor.get(&node_id).cloned())
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone())
    }

    fn fullscreen_monitor_view(&self, monitor_name: &str) -> (Vec2, Vec2) {
        self.model
            .monitor_state
            .monitors
            .get(monitor_name)
            .map(|monitor| (monitor.viewport.center, monitor.viewport.size))
            .unwrap_or((self.model.viewport.center, self.model.viewport.size))
    }

    fn reset_monitor_zoom_once(&mut self, monitor_name: &str) {
        if let Some(monitor) = self.model.monitor_state.monitors.get_mut(monitor_name) {
            monitor.zoom_ref_size = monitor.viewport.size;
            monitor.camera_target_view_size = monitor.viewport.size;
        }
        if self.model.monitor_state.current_monitor == monitor_name {
            self.model.zoom_ref_size = self.model.viewport.size;
            self.model.camera_target_view_size = self.model.viewport.size;
        }
    }

    fn node_intersects_monitor_viewport(&self, id: NodeId, monitor_name: &str) -> bool {
        let Some(node) = self.model.field.node(id) else {
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
        self.model
            .monitor_state
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
        self.model.fullscreen_state.fullscreen_motion.insert(
            id,
            crate::compositor::fullscreen::state::FullscreenMotion {
                from,
                to,
                start_ms: now_ms,
                duration_ms: duration_ms.max(1),
            },
        );
    }

    fn fullscreen_restore_entries_for_monitor(
        &self,
        monitor_name: &str,
        exclude_node: Option<NodeId>,
    ) -> Vec<(
        NodeId,
        crate::compositor::fullscreen::state::FullscreenSessionEntry,
    )> {
        let (monitor_viewport_center, _) = self.fullscreen_monitor_view(monitor_name);
        self.model
            .fullscreen_state
            .fullscreen_restore
            .iter()
            .filter(|&(&id, entry)| {
                if exclude_node == Some(id) {
                    return false;
                }
                let matches_saved_viewport =
                    (entry.viewport_center.x - monitor_viewport_center.x).abs() < 1.0
                        && (entry.viewport_center.y - monitor_viewport_center.y).abs() < 1.0;
                let matches_assigned_monitor = self
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&id)
                    .is_some_and(|node_monitor| node_monitor == monitor_name);
                matches_saved_viewport || matches_assigned_monitor
            })
            .map(|(&id, &entry)| (id, entry))
            .collect()
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
        let monitor_name = self
            .fullscreen_monitor_for_node(node_id)
            .map(str::to_string)
            .or_else(|| self.model.monitor_state.node_monitor.get(&node_id).cloned())
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
        let focused_node = self
            .last_input_surface_node_for_monitor(monitor_name.as_str())
            .or_else(|| self.last_input_surface_node());
        for top in self.platform.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface();
            let key = wl.id();
            if self.model.surface_to_node.get(&key).copied() != Some(node_id) {
                continue;
            }
            let (min_w, min_h) =
                crate::compositor::surface::toplevel_min_size_for_node(self, node_id);
            let monitor = self
                .model
                .monitor_state
                .node_monitor
                .get(&node_id)
                .cloned()
                .unwrap_or_else(|| self.focused_monitor().to_string());
            let view = self.usable_viewport_for_monitor(&monitor);
            let bounds_w = view.size.x as i32;
            let bounds_h = view.size.y as i32;
            top.with_pending_state(|s| {
                s.size = size.map(|(w, h)| (w.max(min_w).max(96), h.max(min_h).max(72)).into());
                s.bounds = Some((bounds_w, bounds_h).into());
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
                self.apply_toplevel_tiled_hint(s);
            });
            top.send_configure();
            break;
        }
    }

    /// Returns the monitor name that `node_id` is currently fullscreened on, if any.
    fn exit_xdg_fullscreen_inner(&mut self, node_id: NodeId, now: Instant, suspend: bool) {
        // Find which monitor this node is fullscreened on.
        let monitor_name = match self.fullscreen_monitor_for_node(node_id) {
            Some(m) => m.to_owned(),
            None => return, // not active fullscreen on any monitor
        };

        self.model
            .fullscreen_state
            .clear_direct_scanout_for_monitor(&monitor_name);

        self.input.interaction_state.reset_input_state_requested = true;

        if suspend {
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .insert(monitor_name.clone(), node_id);
        } else {
            // If we're doing a hard exit, clear any suspended state for this monitor too.
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor_name);
        }

        let now_ms = self.now_ms(now);

        // Restore all nodes that were displaced when this monitor went fullscreen.
        // We identify bystanders as nodes in fullscreen_restore whose saved
        // viewport_center matches this monitor's viewport center.
        let restore_entries = self.fullscreen_restore_entries_for_monitor(&monitor_name, None);

        for (id, entry) in &restore_entries {
            let _ = self.model.field.set_pinned(*id, false);
            let from = self
                .model
                .field
                .node(*id)
                .map(|n| n.pos)
                .unwrap_or(entry.pos);
            self.restore_fullscreen_snapshot(*id, *entry);
            self.queue_fullscreen_motion(*id, from, entry.pos, now_ms, Self::FULLSCREEN_EXIT_MS);
        }

        if let Some(entry) = self
            .model
            .fullscreen_state
            .fullscreen_restore
            .get(&node_id)
            .copied()
        {
            let (min_w, min_h) =
                crate::compositor::surface::toplevel_min_size_for_node(self, node_id);
            self.request_toplevel_fullscreen_state(
                node_id,
                false,
                None,
                Some((
                    entry.size.x.round().max(min_w as f32).max(96.0) as i32,
                    entry.size.y.round().max(min_h as f32).max(72.0) as i32,
                )),
            );
        } else {
            self.request_toplevel_fullscreen_state(node_id, false, None, None);
        }

        self.model
            .fullscreen_state
            .fullscreen_active_node
            .remove(&monitor_name);
        self.model
            .fullscreen_state
            .fullscreen_scale_anim
            .remove(&node_id);
        self.request_maintenance();
    }

    pub(crate) fn suspend_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        self.exit_xdg_fullscreen_inner(node_id, now, true);
    }

    fn restore_fullscreen_snapshot(
        &mut self,
        id: NodeId,
        entry: crate::compositor::fullscreen::state::FullscreenSessionEntry,
    ) {
        if let Some(node) = self.model.field.node_mut(id) {
            node.intrinsic_size = entry.intrinsic_size;
        }
        let _ = self.model.field.sync_active_footprint_to_intrinsic(id);
        if let Some(loc) = entry.bbox_loc {
            self.ui.render_state.cache.bbox_loc.insert(id, loc);
        } else {
            self.ui.render_state.cache.bbox_loc.remove(&id);
        }
        if let Some(geo) = entry.window_geometry {
            self.ui.render_state.cache.window_geometry.insert(id, geo);
        } else {
            self.ui.render_state.cache.window_geometry.remove(&id);
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

        self.model
            .fullscreen_state
            .clear_direct_scanout_for_monitor(&monitor_name);

        // Already fullscreen on this monitor — no-op.
        if self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(&monitor_name)
            == Some(&node_id)
        {
            return;
        }

        // Clear any suspended state for this monitor.
        self.model
            .fullscreen_state
            .fullscreen_suspended_node
            .remove(&monitor_name);

        // If another window is fullscreened on the same monitor, exit it first.
        if let Some(existing) = self
            .model
            .fullscreen_state
            .fullscreen_active_node
            .get(&monitor_name)
            .copied()
        {
            self.exit_xdg_fullscreen(existing, now);
        }

        let now_ms = self.now_ms(now);
        let target_size = self.fullscreen_target_size_for(monitor_name.as_str());
        let (viewport_center, viewport_size) = self.fullscreen_monitor_view(monitor_name.as_str());

        // One-time reset of the target monitor's zoom to 1.0. Do not hold or lock it.
        self.reset_monitor_zoom_once(monitor_name.as_str());

        let Some(node) = self.model.field.node(node_id).cloned() else {
            return;
        };

        crate::compositor::workspace::state::abort_maximize_session_for_node(self, node_id);

        let saved_size = crate::compositor::surface::current_surface_size_for_node(self, node_id)
            .unwrap_or(node.intrinsic_size);
        let saved_bbox_loc = self.ui.render_state.cache.bbox_loc.get(&node_id).copied();
        let saved_window_geometry = self
            .ui
            .render_state
            .cache
            .window_geometry
            .get(&node_id)
            .copied();

        self.model.fullscreen_state.fullscreen_restore.insert(
            node_id,
            crate::compositor::fullscreen::state::FullscreenSessionEntry {
                pos: node.pos,
                size: saved_size,
                viewport_center,
                intrinsic_size: node.intrinsic_size,
                bbox_loc: saved_bbox_loc,
                window_geometry: saved_window_geometry,
                pinned: node.pinned,
            },
        );
        let _ = self.model.field.set_pinned(node_id, false);
        self.queue_fullscreen_motion(
            node_id,
            node.pos,
            viewport_center,
            now_ms,
            Self::FULLSCREEN_ENTER_MS,
        );
        self.model.fullscreen_state.fullscreen_scale_anim.insert(
            node_id,
            crate::compositor::fullscreen::state::FullscreenScaleAnim {
                start_ms: now_ms,
                duration_ms: Self::FULLSCREEN_ENTER_MS,
            },
        );

        // Displace other windows that are on this monitor and intersect its viewport.
        // Windows on other monitors are left completely alone.
        let others: Vec<NodeId> = self
            .model
            .field
            .node_ids_all()
            .into_iter()
            .filter_map(|id| {
                let n = self.model.field.node(id)?;
                (id != node_id
                    && n.kind == halley_core::field::NodeKind::Surface
                    && self.model.field.is_visible(id)
                    && !self.node_user_pinned(id)
                    && self
                        .model
                        .monitor_state
                        .node_monitor
                        .get(&id)
                        .is_none_or(|m| m == &monitor_name)
                    && self.node_intersects_monitor_viewport(id, monitor_name.as_str()))
                .then_some(id)
            })
            .collect();

        for (idx, other_id) in others.into_iter().enumerate() {
            let Some(other) = self.model.field.node(other_id).cloned() else {
                continue;
            };
            let other_size =
                crate::compositor::surface::current_surface_size_for_node(self, other_id)
                    .unwrap_or(other.intrinsic_size);
            let other_bbox_loc = self.ui.render_state.cache.bbox_loc.get(&other_id).copied();
            let other_window_geometry = self
                .ui
                .render_state
                .cache
                .window_geometry
                .get(&other_id)
                .copied();
            self.model.fullscreen_state.fullscreen_restore.insert(
                other_id,
                crate::compositor::fullscreen::state::FullscreenSessionEntry {
                    pos: other.pos,
                    size: other_size,
                    viewport_center,
                    intrinsic_size: other.intrinsic_size,
                    bbox_loc: other_bbox_loc,
                    window_geometry: other_window_geometry,
                    pinned: other.pinned,
                },
            );
            self.assign_node_to_monitor(other_id, monitor_name.as_str());
            let _ = self.model.field.set_pinned(other_id, false);
            self.queue_fullscreen_motion(
                other_id,
                other.pos,
                self.fullscreen_displaced_target(other.pos, idx, viewport_center, viewport_size),
                now_ms,
                Self::FULLSCREEN_ENTER_MS,
            );
        }

        self.request_toplevel_fullscreen_state(node_id, true, output, Some(target_size));
        self.assign_node_to_monitor(node_id, monitor_name.as_str());
        self.model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor_name, node_id);
        self.set_interaction_focus(Some(node_id), 30_000, now);
        self.request_maintenance();
    }

    pub(crate) fn exit_xdg_fullscreen(&mut self, node_id: NodeId, now: Instant) {
        // Clear suspended state on whatever monitor this node is on.
        if let Some(monitor) = self
            .fullscreen_monitor_for_node(node_id)
            .map(|s| s.to_owned())
        {
            self.model
                .fullscreen_state
                .fullscreen_suspended_node
                .remove(&monitor);
        }
        self.exit_xdg_fullscreen_inner(node_id, now, false);
    }

    pub(crate) fn drop_fullscreen_surface(&mut self, id: NodeId, now: Instant) {
        // Clear suspended state if this node was suspended on any monitor.
        self.model
            .fullscreen_state
            .fullscreen_suspended_node
            .retain(|_, &mut nid| nid != id);

        if self.is_fullscreen_active(id) {
            let monitor_name = self
                .fullscreen_monitor_for_node(id)
                .map(|s| s.to_owned())
                .unwrap(); // safe: is_fullscreen_active just confirmed it

            self.input.interaction_state.reset_input_state_requested = true;
            self.model
                .fullscreen_state
                .fullscreen_active_node
                .remove(&monitor_name);

            // Restore only bystanders that were displaced for this monitor's fullscreen.
            let restore_entries =
                self.fullscreen_restore_entries_for_monitor(&monitor_name, Some(id));

            let now_ms = self.now_ms(now);
            for (other_id, entry) in restore_entries {
                let _ = self.model.field.set_pinned(other_id, false);
                let from = self
                    .model
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

        self.model.fullscreen_state.fullscreen_restore.remove(&id);
        self.model.fullscreen_state.fullscreen_motion.remove(&id);
        self.model
            .fullscreen_state
            .fullscreen_scale_anim
            .remove(&id);
        self.model
            .fullscreen_state
            .clear_direct_scanout_for_node(id);
    }

    pub(crate) fn tick_fullscreen_motion(&mut self, now: Instant) {
        if self.model.fullscreen_state.fullscreen_motion.is_empty() {
            return;
        }

        let now_ms = self.now_ms(now);
        let motions: Vec<(
            NodeId,
            crate::compositor::fullscreen::state::FullscreenMotion,
        )> = self
            .model
            .fullscreen_state
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
            let _ = self.model.field.carry(id, pos);
            if t >= 1.0 {
                finished.push((id, motion));
            }
        }

        for (id, motion) in finished {
            self.model.fullscreen_state.fullscreen_motion.remove(&id);
            if let Some(node) = self.model.field.node_mut(id) {
                node.pos = motion.to;
            }
            self.input.interaction_state.physics_velocity.remove(&id);
            if let Some(entry) = self
                .model
                .fullscreen_state
                .fullscreen_restore
                .get(&id)
                .copied()
            {
                // A node finishing its motion should be pinned only if the fullscreen
                // it was displaced for is still active — i.e. the monitor it belongs
                // to still has an active fullscreen session.
                let node_monitor = self
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
                let displaced_for_active = self
                    .model
                    .fullscreen_state
                    .fullscreen_active_node
                    .contains_key(&node_monitor);

                if displaced_for_active {
                    let _ = self.model.field.set_pinned(id, true);
                } else {
                    let _ = self.model.field.set_pinned(id, entry.pinned);
                    self.model.fullscreen_state.fullscreen_restore.remove(&id);
                }
            }
        }

        self.model
            .fullscreen_state
            .fullscreen_scale_anim
            .retain(|_, anim| now_ms < anim.start_ms.saturating_add(anim.duration_ms));
    }
}
