use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::time::Instant;

use eventline::debug;
use halley_config::{InitialWindowOverlapPolicy, InitialWindowSpawnPlacement, PanToNewMode};
use halley_core::decay::DecayLevel;
use halley_core::field::{NodeId, Vec2};
use halley_core::viewport::Viewport;
use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelSurface};

use crate::compositor::ctx::SpawnCtx;
use crate::compositor::focus::state::FocusState;
use crate::compositor::monitor::state::MonitorState;
use crate::compositor::overlap::system::CollisionExtents;
use crate::compositor::root::Halley;
use crate::compositor::spawn::read;
use crate::compositor::spawn::read::RevealNewToplevelPlan;
use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};
use crate::compositor::spawn::state::{MonitorSpawnState, SpawnState};
use crate::render::active_window_frame_pad_px;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InitialToplevelSize {
    pub(crate) node_size: (i32, i32),
    pub(crate) configure_size: Option<(i32, i32)>,
}

fn detected_initial_toplevel_size(toplevel: &ToplevelSurface) -> Option<(i32, i32)> {
    let wl = toplevel.wl_surface();
    let min_size = with_states(wl, |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let state = cached.current();
        (state.min_size.w, state.min_size.h)
    });

    let geometry = with_states(wl, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    });
    if let Some(geometry) = geometry {
        return Some((
            geometry.size.w.max(min_size.0).max(96),
            geometry.size.h.max(min_size.1).max(72),
        ));
    }

    if let Some(size) = toplevel.current_state().size {
        return Some((
            size.w.max(min_size.0).max(96),
            size.h.max(min_size.1).max(72),
        ));
    }

    let bbox = bbox_from_surface_tree(wl, (0, 0));
    if bbox.size.w > 0 && bbox.size.h > 0 {
        return Some((
            bbox.size.w.max(min_size.0).max(96),
            bbox.size.h.max(min_size.1).max(72),
        ));
    }

    None
}

pub(crate) fn initial_toplevel_size(
    ctx: &mut SpawnCtx<'_>,
    toplevel: &ToplevelSurface,
    intent: &InitialWindowIntent,
) -> InitialToplevelSize {
    let st = &ctx.st;
    let defer_rule_resolution =
        crate::compositor::spawn::rules::needs_deferred_rule_recheck(st, intent);
    let predicted_monitor = st.spawn_target_monitor_for_intent(intent);
    let stack_mode_open = st
        .model
        .cluster_state
        .cluster_bloom_open
        .contains_key(predicted_monitor.as_str());
    if !defer_rule_resolution
        && !stack_mode_open
        && intent.rule.cluster_participation
            == halley_config::InitialWindowClusterParticipation::Layout
        && let Some(cid) = st.active_cluster_workspace_for_monitor(predicted_monitor.as_str())
        && let Some(rect) = st.cluster_spawn_rect_for_new_member(predicted_monitor.as_str(), cid)
    {
        let width = rect.w.max(64.0).round() as i32;
        let height = rect.h.max(64.0).round() as i32;
        return InitialToplevelSize {
            node_size: (width, height),
            configure_size: Some((width, height)),
        };
    }

    let detected = detected_initial_toplevel_size(toplevel);
    let node_size = detected.unwrap_or_else(|| {
        (
            (st.model.viewport.size.x * 0.46).round() as i32,
            (st.model.viewport.size.y * 0.42).round() as i32,
        )
    });

    InitialToplevelSize {
        node_size,
        configure_size: None,
    }
}

pub(crate) fn reveal_new_toplevel_node(
    ctx: &mut SpawnCtx<'_>,
    id: NodeId,
    is_transient: bool,
    now: Instant,
) {
    ctx.st.reveal_new_toplevel_node(id, is_transient, now);
}

/// Spawn candidates are tried in a deterministic star pattern:
/// center, then right, left, up, down for each ring.
fn spawn_cardinal_dirs() -> [Vec2; 4] {
    [
        Vec2 { x: 1.0, y: 0.0 },  // right
        Vec2 { x: -1.0, y: 0.0 }, // left
        Vec2 { x: 0.0, y: -1.0 }, // up
        Vec2 { x: 0.0, y: 1.0 },  // down
    ]
}

pub(crate) struct SpawnRevealController<T> {
    st: T,
}

pub(crate) fn spawn_reveal_controller<T>(st: T) -> SpawnRevealController<T> {
    SpawnRevealController { st }
}

impl<T: Deref<Target = Halley>> Deref for SpawnRevealController<T> {
    type Target = Halley;

    fn deref(&self) -> &Self::Target {
        self.st.deref()
    }
}

impl<T: DerefMut<Target = Halley>> DerefMut for SpawnRevealController<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.st.deref_mut()
    }
}

impl<T: Deref<Target = Halley>> SpawnRevealController<T> {
    const SPAWN_STAR_RINGS: usize = 24;

    fn default_window_rule() -> ResolvedInitialWindowRule {
        ResolvedInitialWindowRule::default()
    }

    fn has_default_window_rule(intent: &InitialWindowIntent) -> bool {
        intent.rule == Self::default_window_rule()
            && intent.parent_node.is_none()
            && !intent.prefer_app_intent
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_center_for_monitor(&self, monitor: &str) -> Vec2 {
        read::spawn_read_context(self).viewport_center_for_monitor(monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn resolve_spawn_target_monitor(&self) -> String {
        read::spawn_read_context(self).resolve_spawn_target_monitor()
    }

    #[cfg(test)]
    pub(crate) fn current_spawn_focus(&self, monitor: &str) -> (Option<NodeId>, Vec2) {
        read::spawn_read_context(self).current_spawn_focus(monitor)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn viewport_fully_contains_surface_on_monitor(
        &self,
        monitor: &str,
        id: NodeId,
    ) -> bool {
        self.surface_is_fully_visible_on_monitor(monitor, id)
    }

    #[cfg(test)]
    pub(crate) fn right_spawn_candidate_for_focus(&self, id: NodeId, size: Vec2) -> Option<Vec2> {
        self.spawn_candidate_for_focus_dir(id, size, Vec2 { x: 1.0, y: 0.0 })
    }

    pub(crate) fn spawn_candidate_for_focus_dir(
        &self,
        id: NodeId,
        size: Vec2,
        dir: Vec2,
    ) -> Option<Vec2> {
        let node = self.model.field.node(id)?;
        let focus_ext = self.spawn_obstacle_extents_for_node(node);
        let candidate_ext = CollisionExtents::symmetric(size);
        let gap = self.non_overlap_gap_world();
        let pos = if dir.x > 0.0 {
            Vec2 {
                x: node.pos.x + focus_ext.right + candidate_ext.left + gap,
                y: node.pos.y,
            }
        } else if dir.x < 0.0 {
            Vec2 {
                x: node.pos.x - focus_ext.left - candidate_ext.right - gap,
                y: node.pos.y,
            }
        } else if dir.y > 0.0 {
            Vec2 {
                x: node.pos.x,
                y: node.pos.y + focus_ext.bottom + candidate_ext.top + gap,
            }
        } else {
            Vec2 {
                x: node.pos.x,
                y: node.pos.y - focus_ext.top - candidate_ext.bottom - gap,
            }
        };
        Some(pos)
    }

    pub(crate) fn spawn_star_step_x(&self, size: Vec2) -> f32 {
        size.x
            + (active_window_frame_pad_px(&self.runtime.tuning) as f32 * 2.0)
            + self.non_overlap_gap_world()
    }

    pub(crate) fn spawn_star_step_y(&self, size: Vec2) -> f32 {
        size.y
            + (active_window_frame_pad_px(&self.runtime.tuning) as f32 * 2.0)
            + self.non_overlap_gap_world()
    }

    #[cfg(test)]
    pub(crate) fn spawn_star_step(&self, size: Vec2) -> f32 {
        self.spawn_star_step_x(size)
            .max(self.spawn_star_step_y(size))
    }

    pub(crate) fn star_candidate_offsets(&self, size: Vec2) -> Vec<Vec2> {
        let step_x = self.spawn_star_step_x(size);
        let step_y = self.spawn_star_step_y(size);
        let mut out = Vec::with_capacity(1 + Self::SPAWN_STAR_RINGS * spawn_cardinal_dirs().len());

        out.push(Vec2 { x: 0.0, y: 0.0 });

        for ring in 1..=Self::SPAWN_STAR_RINGS {
            for dir in spawn_cardinal_dirs() {
                out.push(Vec2 {
                    x: dir.x * step_x * ring as f32,
                    y: dir.y * step_y * ring as f32,
                });
            }
        }

        out
    }

    fn viewport_for_monitor(&self, monitor: &str) -> Option<Viewport> {
        if self.model.monitor_state.current_monitor == monitor {
            return Some(Viewport::new(
                self.model.viewport.center,
                self.camera_view_size(),
            ));
        }
        self.model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|space| Viewport::new(space.viewport.center, space.zoom_ref_size))
    }

    fn world_from_monitor_screen(&self, monitor: &str, sx: f32, sy: f32) -> Option<Vec2> {
        let (w, h, local_sx, local_sy) = self.local_screen_in_monitor(monitor, sx, sy);
        let viewport = self.viewport_for_monitor(monitor)?;
        let w = (w as f32).max(1.0);
        let h = (h as f32).max(1.0);
        let nx = (local_sx / w) - 0.5;
        let ny = (local_sy / h) - 0.5;
        Some(Vec2 {
            x: viewport.center.x + nx * viewport.size.x.max(1.0),
            y: viewport.center.y + ny * viewport.size.y.max(1.0),
        })
    }

    fn spawn_candidate_fits(
        &self,
        monitor: &str,
        pos: Vec2,
        size: Vec2,
        skip_node: Option<NodeId>,
    ) -> bool {
        self.spawn_candidate_fits_with_policy(
            monitor,
            pos,
            size,
            skip_node,
            InitialWindowOverlapPolicy::None,
            None,
        )
    }

    fn spawn_candidate_fits_with_policy(
        &self,
        monitor: &str,
        pos: Vec2,
        size: Vec2,
        skip_node: Option<NodeId>,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
    ) -> bool {
        if overlap_policy == InitialWindowOverlapPolicy::All {
            return true;
        }
        let pair_gap = self.non_overlap_gap_world();
        let candidate = CollisionExtents::symmetric(size);
        !self.model.field.nodes().values().any(|other| {
            if Some(other.id) == skip_node
                || other.kind != halley_core::field::NodeKind::Surface
                || !self.model.field.is_visible(other.id)
            {
                return false;
            }
            if self
                .model
                .monitor_state
                .node_monitor
                .get(&other.id)
                .is_some_and(|other_monitor| other_monitor != monitor)
            {
                return false;
            }
            if overlap_policy == InitialWindowOverlapPolicy::ParentOnly
                && parent_node == Some(other.id)
            {
                return false;
            }
            let other_ext = self.spawn_obstacle_extents_for_node(other);
            let req_x = self.required_sep_x(pos.x, candidate, other.pos.x, other_ext, pair_gap);
            let req_y = self.required_sep_y(pos.y, candidate, other.pos.y, other_ext, pair_gap);
            (pos.x - other.pos.x).abs() < req_x && (pos.y - other.pos.y).abs() < req_y
        })
    }

    fn try_spawn_star(&self, monitor: &str, center: Vec2, size: Vec2) -> Option<Vec2> {
        self.try_spawn_star_with_policy(
            monitor,
            center,
            size,
            InitialWindowOverlapPolicy::None,
            None,
        )
    }

    fn try_spawn_star_with_policy(
        &self,
        monitor: &str,
        center: Vec2,
        size: Vec2,
        overlap_policy: InitialWindowOverlapPolicy,
        parent_node: Option<NodeId>,
    ) -> Option<Vec2> {
        for offset in self.star_candidate_offsets(size) {
            let pos = Vec2 {
                x: center.x + offset.x,
                y: center.y + offset.y,
            };
            if self.spawn_candidate_fits_with_policy(
                monitor,
                pos,
                size,
                None,
                overlap_policy,
                parent_node,
            ) {
                return Some(pos);
            }
        }
        None
    }

    fn resolve_parent_monitor(&self, parent_node: Option<NodeId>) -> Option<String> {
        parent_node.and_then(|id| self.model.monitor_state.node_monitor.get(&id).cloned())
    }

    pub(crate) fn spawn_target_monitor_for_intent(&self, intent: &InitialWindowIntent) -> String {
        let default_monitor = read::spawn_read_context(self).resolve_spawn_target_monitor();
        match intent.effective_spawn_placement() {
            InitialWindowSpawnPlacement::Center
            | InitialWindowSpawnPlacement::Adjacent
            | InitialWindowSpawnPlacement::App => self
                .resolve_parent_monitor(intent.parent_node)
                .unwrap_or(default_monitor),
            InitialWindowSpawnPlacement::Cursor => {
                if let Some((sx, sy)) = self.input.interaction_state.last_pointer_screen_global {
                    self.monitor_for_screen(sx, sy).unwrap_or(default_monitor)
                } else {
                    default_monitor
                }
            }
            InitialWindowSpawnPlacement::ViewportCenter => default_monitor,
        }
    }

    fn pick_cluster_growth_dir(&self, monitor: &str, center: Vec2) -> Vec2 {
        let dirs = spawn_cardinal_dirs();
        let local = self
            .model
            .monitor_state
            .monitors
            .get(monitor)
            .map(|monitor| Vec2 {
                x: center.x - monitor.offset_x as f32,
                y: center.y - monitor.offset_y as f32,
            })
            .unwrap_or(center);
        let idx = ((self.spawn_monitor_state(monitor).spawn_cursor as usize)
            .wrapping_add(local.x.abs() as usize)
            .wrapping_add((local.y.abs() * 3.0) as usize))
            % dirs.len();
        dirs[idx]
    }
}

impl<T: DerefMut<Target = Halley>> SpawnRevealController<T> {
    pub(crate) fn update_spawn_patch(
        &mut self,
        monitor: &str,
        anchor: Vec2,
        focus_node: Option<NodeId>,
        focus_pos: Vec2,
        growth_dir: Vec2,
    ) {
        self.spawn_monitor_state_mut(monitor).spawn_patch =
            Some(crate::compositor::spawn::state::SpawnPatch {
                anchor,
                focus_node,
                focus_pos,
                growth_dir,
                placements_in_patch: 0,
                frontier: Vec::new(),
            });
    }

    fn default_pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        self.pick_spawn_position_impl(size)
    }

    /// Returns `(monitor, position, needs_pan)`.
    #[allow(dead_code)]
    pub(crate) fn pick_spawn_position(&mut self, size: Vec2) -> (String, Vec2, bool) {
        self.default_pick_spawn_position(size)
    }

    pub(crate) fn pick_spawn_position_with_intent(
        &mut self,
        size: Vec2,
        intent: &InitialWindowIntent,
    ) -> (String, Vec2, bool) {
        if Self::has_default_window_rule(intent) {
            return self.default_pick_spawn_position(size);
        }

        let target_monitor = self.spawn_target_monitor_for_intent(intent);
        let overlap_policy = intent.effective_overlap_policy();
        let placement = intent.effective_spawn_placement();
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_cursor += 1;
        let viewport_center =
            read::spawn_read_context(self).viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) =
            read::spawn_read_context(self).current_spawn_focus(target_monitor.as_str());
        let parent_anchor = intent
            .parent_node
            .and_then(|id| self.model.field.node(id).map(|node| node.pos));
        let cursor_anchor = self
            .input
            .interaction_state
            .last_pointer_screen_global
            .and_then(|(sx, sy)| self.world_from_monitor_screen(target_monitor.as_str(), sx, sy));

        let chosen = match placement {
            InitialWindowSpawnPlacement::Adjacent => {
                if let Some(parent_id) = intent.parent_node {
                    for dir in spawn_cardinal_dirs() {
                        if let Some(pos) = self.spawn_candidate_for_focus_dir(parent_id, size, dir)
                            && self.spawn_candidate_fits_with_policy(
                                target_monitor.as_str(),
                                pos,
                                size,
                                None,
                                overlap_policy,
                                intent.parent_node,
                            )
                        {
                            return (target_monitor, pos, false);
                        }
                    }
                    return self.default_pick_spawn_position(size);
                }
                if overlap_policy == InitialWindowOverlapPolicy::All {
                    if let Some(id) = focus_id {
                        for dir in spawn_cardinal_dirs() {
                            if let Some(pos) = self.spawn_candidate_for_focus_dir(id, size, dir) {
                                return (target_monitor, pos, false);
                            }
                        }
                    }
                    return (target_monitor, focus_pos, false);
                }
                return self.default_pick_spawn_position(size);
            }
            InitialWindowSpawnPlacement::Center => parent_anchor.unwrap_or(viewport_center),
            InitialWindowSpawnPlacement::ViewportCenter => viewport_center,
            InitialWindowSpawnPlacement::Cursor => cursor_anchor.unwrap_or(viewport_center),
            InitialWindowSpawnPlacement::App => parent_anchor.unwrap_or(viewport_center),
        };

        let pos = if overlap_policy == InitialWindowOverlapPolicy::All {
            chosen
        } else {
            self.try_spawn_star_with_policy(
                target_monitor.as_str(),
                chosen,
                size,
                overlap_policy,
                intent.parent_node,
            )
            .unwrap_or(chosen)
        };
        let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), chosen);
        self.update_spawn_patch(
            target_monitor.as_str(),
            chosen,
            intent.parent_node,
            chosen,
            growth_dir,
        );
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_view_anchor = chosen;
        (target_monitor, pos, false)
    }

    fn pick_spawn_position_impl(&mut self, size: Vec2) -> (String, Vec2, bool) {
        let target_monitor = self
            .model
            .spawn_state
            .pending_spawn_monitor
            .take()
            .filter(|monitor| self.model.monitor_state.monitors.contains_key(monitor))
            .unwrap_or_else(|| read::spawn_read_context(self).resolve_spawn_target_monitor());
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_cursor += 1;
        let monitor_spawn = self.spawn_monitor_state(target_monitor.as_str());
        let viewport_center =
            read::spawn_read_context(self).viewport_center_for_monitor(target_monitor.as_str());
        let (focus_id, focus_pos) =
            read::spawn_read_context(self).current_spawn_focus(target_monitor.as_str());
        debug!(
            "spawn target resolved: target_monitor={} focused_monitor={} interaction_monitor={} anchor_mode={:?} focus_id={:?}",
            target_monitor,
            self.focused_monitor(),
            self.interaction_monitor(),
            monitor_spawn.spawn_anchor_mode,
            focus_id.map(|id| id.as_u64())
        );
        let focus_visible = focus_id.is_some_and(|id| {
            self.surface_is_fully_visible_on_monitor(target_monitor.as_str(), id)
        });

        if let Some(id) = focus_id {
            for dir in spawn_cardinal_dirs() {
                if let Some(pos) = self.spawn_candidate_for_focus_dir(id, size, dir)
                    && self.spawn_candidate_fits(target_monitor.as_str(), pos, size, None)
                {
                    self.update_spawn_patch(
                        target_monitor.as_str(),
                        focus_pos,
                        Some(id),
                        focus_pos,
                        dir,
                    );
                    debug!(
                        "spawn position picked: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
                        target_monitor,
                        focus_pos.x,
                        focus_pos.y,
                        focus_pos.x,
                        focus_pos.y,
                        pos.x,
                        pos.y,
                        size.x,
                        size.y
                    );
                    return (target_monitor, pos, false);
                }
            }
        }

        let anchor = if focus_visible {
            focus_pos
        } else {
            viewport_center
        };
        if let Some(pos) = self.try_spawn_star(target_monitor.as_str(), anchor, size) {
            let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), anchor);
            self.update_spawn_patch(
                target_monitor.as_str(),
                anchor,
                None,
                viewport_center,
                growth_dir,
            );
            self.spawn_monitor_state_mut(target_monitor.as_str())
                .spawn_view_anchor = anchor;
            debug!(
                "spawn position picked: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
                target_monitor,
                anchor.x,
                anchor.y,
                focus_pos.x,
                focus_pos.y,
                pos.x,
                pos.y,
                size.x,
                size.y
            );
            return (target_monitor, pos, false);
        }

        let fallback_anchor = viewport_center;
        let growth_dir = self.pick_cluster_growth_dir(target_monitor.as_str(), fallback_anchor);
        self.update_spawn_patch(
            target_monitor.as_str(),
            fallback_anchor,
            None,
            viewport_center,
            growth_dir,
        );
        self.spawn_monitor_state_mut(target_monitor.as_str())
            .spawn_view_anchor = fallback_anchor;
        debug!(
            "spawn fallback used: target_monitor={} anchor=({:.1},{:.1}) focus_pos=({:.1},{:.1}) chosen=({:.1},{:.1}) size=({:.1},{:.1})",
            target_monitor,
            fallback_anchor.x,
            fallback_anchor.y,
            focus_pos.x,
            focus_pos.y,
            fallback_anchor.x,
            fallback_anchor.y,
            size.x,
            size.y
        );
        (target_monitor, fallback_anchor, false)
    }

    pub(crate) fn maybe_start_pending_spawn_pan(&mut self, now: Instant) {
        if self.model.spawn_state.active_spawn_pan.is_some() {
            return;
        }

        let now_ms = self.now_ms(now);
        while let Some(next) = self.model.spawn_state.pending_spawn_pan_queue.pop_front() {
            if self.model.field.node(next.node_id).is_none() {
                continue;
            }

            let did_pan = self.animate_viewport_center_to_delayed(
                next.target_center,
                now,
                Halley::VIEWPORT_PAN_PRELOAD_MS,
            );
            self.model.spawn_state.active_spawn_pan =
                Some(crate::compositor::spawn::state::ActiveSpawnPan {
                    node_id: next.node_id,
                    pan_start_at_ms: now_ms.saturating_add(if did_pan {
                        Halley::VIEWPORT_PAN_PRELOAD_MS
                    } else {
                        0
                    }),
                    reveal_at_ms: now_ms.saturating_add(if did_pan {
                        Halley::VIEWPORT_PAN_PRELOAD_MS + Halley::VIEWPORT_PAN_DURATION_MS
                    } else {
                        0
                    }),
                });
            break;
        }
    }

    pub(crate) fn tick_pending_spawn_pan(&mut self, now: Instant, now_ms: u64) {
        let Some(active) = self.model.spawn_state.active_spawn_pan else {
            self.maybe_start_pending_spawn_pan(now);
            return;
        };

        if self.model.field.node(active.node_id).is_none() {
            self.model.spawn_state.active_spawn_pan = None;
            self.maybe_start_pending_spawn_pan(now);
            return;
        }

        let pan_finished = now_ms >= active.reveal_at_ms
            || (now_ms >= active.pan_start_at_ms
                && self.input.interaction_state.viewport_pan_anim.is_none());
        if !pan_finished {
            return;
        }

        let _ = self.model.field.set_detached(active.node_id, false);
        let _ = self
            .model
            .field
            .set_decay_level(active.node_id, DecayLevel::Hot);
        let intrinsic_size = self
            .model
            .field
            .node(active.node_id)
            .map(|node| node.intrinsic_size);
        if let Some(intrinsic_size) = intrinsic_size {
            self.model
                .workspace_state
                .last_active_size
                .insert(active.node_id, intrinsic_size);
        }
        let duration_ms = self.runtime.tuning.window_open_duration_ms();
        if self.runtime.tuning.window_open_animation_enabled() {
            self.mark_active_transition(active.node_id, now, duration_ms);
        }
        self.record_focus_trail_visit(active.node_id);
        self.model.focus_state.suppress_trail_record_once = true;
        self.set_interaction_focus(Some(active.node_id), 30_000, now);
        self.model.spawn_state.active_spawn_pan = None;
        self.maybe_start_pending_spawn_pan(now);
    }

    pub(crate) fn reveal_new_toplevel_node(
        &mut self,
        id: NodeId,
        is_transient: bool,
        now: Instant,
    ) {
        let node_monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());
        let cluster_local = self
            .model
            .field
            .cluster_id_for_member_public(id)
            .is_some_and(|cid| {
                self.active_cluster_workspace_for_monitor(node_monitor.as_str()) == Some(cid)
            });
        if self
            .model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .contains_key(&id)
        {
            return;
        }
        if cluster_local {
            let _ = self.model.field.set_detached(id, false);
            self.set_recent_top_node(id, now + std::time::Duration::from_millis(1200));
            self.set_interaction_focus(Some(id), 30_000, now);
            self.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            let duration_ms = self.runtime.tuning.window_open_duration_ms();
            if self.runtime.tuning.window_open_animation_enabled() {
                self.mark_active_transition(id, now, duration_ms);
            }
            return;
        }
        if self.model.spawn_state.pending_initial_reveal.contains(&id) {
            return;
        }
        if self
            .model
            .spawn_state
            .applied_window_rules
            .get(&id)
            .is_some_and(|rule| rule.suppress_reveal_pan)
        {
            let _ = self.model.field.set_detached(id, false);
            self.set_recent_top_node(id, now + std::time::Duration::from_millis(1200));
            self.record_focus_trail_visit(id);
            self.model.focus_state.suppress_trail_record_once = true;
            self.set_interaction_focus(Some(id), 30_000, now);
            self.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            let duration_ms = self.runtime.tuning.window_open_duration_ms();
            if self.runtime.tuning.window_open_animation_enabled() {
                self.mark_active_transition(id, now, duration_ms);
            }
            return;
        }
        match self.resolve_spawn_reveal_plan(id, is_transient) {
            RevealNewToplevelPlan::AlreadyQueued => {}
            RevealNewToplevelPlan::ActivateNow => {
                let _ = self.model.field.set_detached(id, false);
                self.record_focus_trail_visit(id);
                self.model.focus_state.suppress_trail_record_once = true;
                self.set_interaction_focus(Some(id), 30_000, now);
                self.model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .remove(&id);
                let duration_ms = self.runtime.tuning.window_open_duration_ms();
                if self.runtime.tuning.window_open_animation_enabled() {
                    self.mark_active_transition(id, now, duration_ms);
                }
            }
            RevealNewToplevelPlan::QueuePan { target_center } => {
                let _ = self.model.field.set_detached(id, true);
                self.model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .remove(&id);
                self.model.spawn_state.pending_spawn_pan_queue.push_back(
                    crate::compositor::spawn::state::PendingSpawnPan {
                        node_id: id,
                        target_center,
                    },
                );
                self.maybe_start_pending_spawn_pan(now);
            }
        }
    }

    fn resolve_spawn_reveal_plan(
        &self,
        id: NodeId,
        is_transient: bool,
    ) -> read::RevealNewToplevelPlan {
        read::spawn_read_context(self).reveal_new_toplevel_plan(self, id, is_transient)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};

    fn test_intent(
        overlap_policy: InitialWindowOverlapPolicy,
        spawn_placement: InitialWindowSpawnPlacement,
        parent_node: Option<NodeId>,
    ) -> InitialWindowIntent {
        InitialWindowIntent {
            app_id: Some("firefox".to_string()),
            title: None,
            parent_node,
            rule: ResolvedInitialWindowRule {
                overlap_policy,
                spawn_placement,
                cluster_participation: halley_config::InitialWindowClusterParticipation::Layout,
            },
            matched_rule: true,
            is_transient: parent_node.is_some(),
            prefer_app_intent: matches!(spawn_placement, InitialWindowSpawnPlacement::App),
        }
    }

    #[test]
    fn star_offsets_are_center_then_right_left_up_down() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let state = Halley::new_for_test(&dh, tuning);

        let offsets = state.star_candidate_offsets(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(offsets[0], Vec2 { x: 0.0, y: 0.0 });

        let step_x = state.spawn_star_step_x(Vec2 { x: 100.0, y: 80.0 });
        let step_y = state.spawn_star_step_y(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(offsets[1], Vec2 { x: step_x, y: 0.0 });
        assert_eq!(offsets[2], Vec2 { x: -step_x, y: 0.0 });
        assert_eq!(offsets[3], Vec2 { x: 0.0, y: -step_y });
        assert_eq!(offsets[4], Vec2 { x: 0.0, y: step_y });
    }

    #[test]
    fn first_spawn_in_star_is_center() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let (_, pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn second_spawn_uses_first_available_star_slot() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state
            .model
            .field
            .spawn_surface("first", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .model
            .field
            .set_state(first, halley_core::field::NodeState::Active);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(first, 1);
        state.model.focus_state.primary_interaction_focus = Some(first);
        state.assign_node_to_current_monitor(first);
        let current_monitor = state.model.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            current_monitor.as_str(),
            Vec2 { x: 0.0, y: 0.0 },
            Some(first),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let (_, pos, needs_pan) = state.pick_spawn_position(size);
        let expected = state
            .right_spawn_candidate_for_focus(first, size)
            .expect("right spawn candidate");
        assert_eq!(pos, expected);
        assert!(!needs_pan);
    }

    #[test]
    fn current_spawn_focus_keeps_focused_window_anchor() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(focused, 1);
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        state
            .model
            .focus_state
            .monitor_focus
            .insert(state.model.monitor_state.current_monitor.clone(), focused);

        assert_eq!(
            state.current_spawn_focus(state.model.monitor_state.current_monitor.as_str()),
            (Some(focused), Vec2 { x: 0.0, y: 0.0 })
        );
    }

    #[test]
    fn view_mode_spawns_near_viewport_center_without_pan() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );
        let _ = state
            .model
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(focused, 1);
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        {
            let current_monitor = state.model.monitor_state.current_monitor.clone();
            let viewport_center = state.model.viewport.center;
            let spawn = state.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = viewport_center;
        }

        let (_, pos, needs_pan) = state.pick_spawn_position(Vec2 { x: 100.0, y: 80.0 });
        assert!(!needs_pan);
        assert_eq!(pos, state.model.viewport.center);
    }

    #[test]
    fn focus_mode_uses_next_free_neighbor_around_last_focus() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 500.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let _ = state
            .model
            .field
            .set_state(focused, halley_core::field::NodeState::Active);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(focused, 1);
        state.model.focus_state.primary_interaction_focus = Some(focused);
        state.assign_node_to_current_monitor(focused);
        state
            .model
            .focus_state
            .monitor_focus
            .insert(state.model.monitor_state.current_monitor.clone(), focused);
        let current_monitor = state.model.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            current_monitor.as_str(),
            Vec2 { x: 0.0, y: 0.0 },
            Some(focused),
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let size = Vec2 { x: 120.0, y: 90.0 };
        let existing = state
            .model
            .field
            .spawn_surface("existing", Vec2 { x: 143.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(existing);
        let (_, pos, needs_pan) = state.pick_spawn_position(size);
        assert_eq!(pos, Vec2 { x: -143.0, y: 0.0 });
        assert!(!needs_pan);
    }

    #[test]
    fn view_mode_continues_local_build_up_around_new_area() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 1200.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        {
            let current_monitor = state.model.monitor_state.current_monitor.clone();
            let viewport_center = state.model.viewport.center;
            let spawn = state.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = viewport_center;
        }

        let size = Vec2 { x: 100.0, y: 80.0 };
        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_current_monitor(first_id);
        let second = state.pick_spawn_position(size).1;
        let step = state.spawn_star_step(size);
        assert_eq!(first, Vec2 { x: 1200.0, y: 0.0 });
        assert_eq!(
            second,
            Vec2 {
                x: 1200.0 + step,
                y: 0.0
            }
        );
    }

    #[test]
    fn focused_monitor_drives_spawn_even_when_current_monitor_differs() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let _ = state.activate_monitor("left");

        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 {
                x: 1200.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(focused, "right");
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        let expected = state
            .right_spawn_candidate_for_focus(focused, Vec2 { x: 120.0, y: 90.0 })
            .expect("right spawn candidate");
        assert_eq!(monitor, "right");
        assert_eq!(pos, expected);
    }

    #[test]
    fn monitor_local_last_input_beats_stale_monitor_focus_for_spawn_anchor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let stale = state.model.field.spawn_surface(
            "stale",
            Vec2 {
                x: 1040.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        let latest = state.model.field.spawn_surface(
            "latest",
            Vec2 {
                x: 1320.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(stale, "right");
        state.assign_node_to_monitor(latest, "right");
        state
            .model
            .focus_state
            .monitor_focus
            .insert("right".to_string(), stale);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(stale, 1);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(latest, 2);
        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        let expected = state
            .right_spawn_candidate_for_focus(latest, Vec2 { x: 120.0, y: 90.0 })
            .expect("right spawn candidate");
        assert_eq!(monitor, "right");
        assert_eq!(pos, expected);
    }

    #[test]
    fn spawn_buildup_stays_isolated_per_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let size = Vec2 { x: 100.0, y: 80.0 };
        let step = state.spawn_star_step(size);

        let _ = state.activate_monitor("left");
        state.set_interaction_monitor("left");
        state.set_focused_monitor("left");
        {
            let spawn = state.spawn_monitor_state_mut("left");
            spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = Vec2 { x: 400.0, y: 300.0 };
        }
        let first_left = state.pick_spawn_position(size).1;
        let left_id = state.model.field.spawn_surface("left-1", first_left, size);
        state.assign_node_to_monitor(left_id, "left");

        let _ = state.activate_monitor("right");
        state.set_interaction_monitor("right");
        state.set_focused_monitor("right");
        {
            let spawn = state.spawn_monitor_state_mut("right");
            spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
            spawn.spawn_view_anchor = Vec2 {
                x: 1200.0,
                y: 300.0,
            };
        }
        let first_right = state.pick_spawn_position(size).1;
        let right_id = state
            .model
            .field
            .spawn_surface("right-1", first_right, size);
        state.assign_node_to_monitor(right_id, "right");

        let _ = state.activate_monitor("left");
        state.set_interaction_monitor("left");
        state.set_focused_monitor("left");
        let second_left = state.pick_spawn_position(size).1;

        assert_eq!(first_left, Vec2 { x: 400.0, y: 300.0 });
        assert_eq!(
            first_right,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
        assert_eq!(
            second_left,
            Vec2 {
                x: 400.0 + step,
                y: 300.0,
            }
        );
    }

    #[test]
    fn focus_mode_keeps_monitor_local_patch_after_auto_focusing_new_spawn() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let size = Vec2 { x: 120.0, y: 90.0 };
        let anchor = state
            .model
            .field
            .spawn_surface("anchor", Vec2 { x: 0.0, y: 0.0 }, size);
        let _ = state
            .model
            .field
            .set_state(anchor, halley_core::field::NodeState::Active);
        state.assign_node_to_current_monitor(anchor);
        state.model.focus_state.primary_interaction_focus = Some(anchor);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(anchor, 1);
        state
            .model
            .focus_state
            .monitor_focus
            .insert(state.model.monitor_state.current_monitor.clone(), anchor);

        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_current_monitor(first_id);
        state.set_interaction_focus(Some(first_id), 30_000, Instant::now());

        let second = state.pick_spawn_position(size).1;
        let first_expected = state
            .right_spawn_candidate_for_focus(anchor, size)
            .expect("right spawn candidate");
        let second_expected = state
            .right_spawn_candidate_for_focus(first_id, size)
            .expect("right spawn candidate");

        assert_eq!(first, first_expected);
        assert_eq!(second, second_expected);
    }

    #[test]
    fn focus_monitor_view_switches_spawn_to_clicked_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());

        state.focus_monitor_view("right", Instant::now());

        assert_eq!(state.interaction_monitor(), "right");
        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
        assert_eq!(
            state.spawn_monitor_state("right").spawn_anchor_mode,
            crate::compositor::spawn::state::SpawnAnchorMode::View
        );

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
    }

    #[test]
    fn focused_monitor_beats_interaction_monitor_drift_for_spawn_target() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());
        state.focus_monitor_view("right", Instant::now());

        state.set_interaction_monitor("left");
        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });

        assert_eq!(state.focused_monitor(), "right");
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
    }

    #[test]
    fn focused_monitor_beats_stale_primary_focus_monitor_for_spawn_target() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.model.focus_state.primary_interaction_focus = Some(left);
        state
            .model
            .focus_state
            .last_surface_focus_ms
            .insert(left, 1);

        state.focus_monitor_view("right", Instant::now());

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
    }

    #[test]
    fn pending_spawn_monitor_beats_focus_churn_for_next_toplevel() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.set_interaction_focus(Some(left), 30_000, Instant::now());

        state.model.spawn_state.pending_spawn_monitor = Some("right".to_string());

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });
        assert_eq!(monitor, "right");
        assert_eq!(
            pos,
            Vec2 {
                x: 1200.0,
                y: 300.0
            }
        );
        assert!(state.model.spawn_state.pending_spawn_monitor.is_none());
    }

    #[test]
    fn shorter_secondary_keeps_building_to_the_right_offscreen() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 2560,
                height: 1440,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 2560,
                offset_y: 0,
                width: 1920,
                height: 1200,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.focus_monitor_view("right", Instant::now());

        let size = Vec2 { x: 883.0, y: 504.0 };
        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_monitor(first_id, "right");
        state.set_interaction_focus(Some(first_id), 30_000, Instant::now());

        let second = state.pick_spawn_position(size).1;
        let second_id = state.model.field.spawn_surface("second", second, size);
        state.assign_node_to_monitor(second_id, "right");
        state.set_interaction_focus(Some(second_id), 30_000, Instant::now());
        let third = state.pick_spawn_position(size).1;

        let second_expected = state
            .right_spawn_candidate_for_focus(first_id, size)
            .expect("right spawn candidate");
        let third_expected = state
            .right_spawn_candidate_for_focus(second_id, size)
            .expect("right spawn candidate");
        assert_eq!(
            first,
            Vec2 {
                x: 3520.0,
                y: 600.0
            }
        );
        assert_eq!(second, second_expected);
        assert_eq!(third, third_expected);
    }

    #[test]
    fn focus_mode_checks_neighbors_in_right_left_up_down_order() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let size = Vec2 { x: 120.0, y: 90.0 };
        let focused = state
            .model
            .field
            .spawn_surface("focused", Vec2 { x: 0.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(focused);
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());

        let right = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: 1.0, y: 0.0 })
            .expect("right");
        let left = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: -1.0, y: 0.0 })
            .expect("left");
        let up = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: 0.0, y: -1.0 })
            .expect("up");
        let down = state
            .spawn_candidate_for_focus_dir(focused, size, Vec2 { x: 0.0, y: 1.0 })
            .expect("down");

        let right_id = state.model.field.spawn_surface("right", right, size);
        let left_id = state.model.field.spawn_surface("left", left, size);
        let up_id = state.model.field.spawn_surface("up", up, size);
        state.assign_node_to_current_monitor(right_id);
        state.assign_node_to_current_monitor(left_id);
        state.assign_node_to_current_monitor(up_id);

        let chosen = state.pick_spawn_position(size).1;
        assert_eq!(chosen, down);
    }

    #[test]
    fn reveal_new_toplevel_skips_pan_when_spawn_is_already_visible() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.model.field.spawn_surface(
            "new",
            Vec2 { x: 920.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert!(state.model.spawn_state.active_spawn_pan.is_none());
        assert!(state.model.spawn_state.pending_spawn_pan_queue.is_empty());
        assert!(state.input.interaction_state.viewport_pan_anim.is_none());
        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(id));
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_partially_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 700.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };

        let id = state.model.field.spawn_surface(
            "partial",
            Vec2 { x: 1460.0, y: 0.0 },
            Vec2 { x: 240.0, y: 160.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(
            state
                .model
                .spawn_state
                .active_spawn_pan
                .map(|pan| pan.node_id),
            Some(id)
        );
    }

    #[test]
    fn reveal_new_toplevel_pans_when_spawn_is_offscreen() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };

        let id = state.model.field.spawn_surface(
            "new",
            Vec2 { x: 1200.0, y: 0.0 },
            Vec2 { x: 100.0, y: 80.0 },
        );

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert_eq!(
            state
                .model
                .spawn_state
                .active_spawn_pan
                .map(|pan| pan.node_id),
            Some(id)
        );
    }

    #[test]
    fn center_all_can_overlap_parent_anchor_directly() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let size = Vec2 { x: 120.0, y: 90.0 };
        let parent = state
            .model
            .field
            .spawn_surface("parent", Vec2 { x: 0.0, y: 0.0 }, size);
        let other = state
            .model
            .field
            .spawn_surface("other", Vec2 { x: 0.0, y: 0.0 }, size);
        for id in [parent, other] {
            state.assign_node_to_current_monitor(id);
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }

        let intent = test_intent(
            InitialWindowOverlapPolicy::All,
            InitialWindowSpawnPlacement::Center,
            Some(parent),
        );
        let (_, pos, _) = state.pick_spawn_position_with_intent(size, &intent);

        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn adjacent_parent_only_avoids_unrelated_windows() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let size = Vec2 { x: 120.0, y: 90.0 };
        let parent = state
            .model
            .field
            .spawn_surface("parent", Vec2 { x: 0.0, y: 0.0 }, size);
        let blocked = state.model.field.spawn_surface(
            "blocked",
            state
                .spawn_candidate_for_focus_dir(parent, size, Vec2 { x: 1.0, y: 0.0 })
                .expect("right candidate"),
            size,
        );
        for id in [parent, blocked] {
            state.assign_node_to_current_monitor(id);
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }

        let intent = test_intent(
            InitialWindowOverlapPolicy::ParentOnly,
            InitialWindowSpawnPlacement::Adjacent,
            Some(parent),
        );
        let (_, pos, _) = state.pick_spawn_position_with_intent(size, &intent);

        assert_eq!(
            pos,
            state
                .spawn_candidate_for_focus_dir(parent, size, Vec2 { x: -1.0, y: 0.0 })
                .expect("left candidate")
        );
    }

    #[test]
    fn cursor_placement_uses_pointer_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.input.interaction_state.last_pointer_screen_global = Some((900.0, 120.0));

        let intent = test_intent(
            InitialWindowOverlapPolicy::None,
            InitialWindowSpawnPlacement::Cursor,
            None,
        );

        assert_eq!(state.spawn_target_monitor_for_intent(&intent), "right");
    }

    #[test]
    fn pending_initial_reveal_blocks_initial_reveal() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "window",
            Vec2 {
                x: 5000.0,
                y: 5000.0,
            },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_current_monitor(id);
        let _ = state
            .model
            .field
            .set_state(id, halley_core::field::NodeState::Active);
        state.model.spawn_state.pending_initial_reveal.insert(id);

        state.reveal_new_toplevel_node(id, false, Instant::now());

        assert!(state.model.spawn_state.active_spawn_pan.is_none());
        assert!(state.model.spawn_state.pending_spawn_pan_queue.is_empty());
        assert_ne!(state.model.focus_state.primary_interaction_focus, Some(id));
    }
}
