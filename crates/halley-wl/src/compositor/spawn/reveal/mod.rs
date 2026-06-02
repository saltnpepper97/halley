use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::time::Instant;

mod placement;

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
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::monitor::state::MonitorState;
use crate::compositor::overlap::system::CollisionExtents;
use crate::compositor::root::Halley;
use crate::compositor::spawn::read;
use crate::compositor::spawn::read::RevealNewToplevelPlan;
use crate::compositor::spawn::rules::{InitialWindowIntent, ResolvedInitialWindowRule};
use crate::compositor::spawn::state::{MonitorSpawnState, SpawnState};
use crate::window::active_window_frame_pad_px;

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

    if !defer_rule_resolution
        && !stack_mode_open
        && let Some((width, height)) = intent.rule.initial_size
    {
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

impl<T: DerefMut<Target = Halley>> SpawnRevealController<T> {
    pub(crate) fn maybe_start_pending_spawn_pan(&mut self, now: Instant) {
        if self.model.spawn_state.active_spawn_pan.is_some() {
            return;
        }

        let now_ms = self.now_ms(now);
        while let Some(next) = self.model.spawn_state.pending_spawn_pan_queue.pop_front() {
            if self.model.field.node(next.node_id).is_none() {
                continue;
            }

            let prev_monitor = self.model.monitor_state.current_monitor.clone();
            let needs_monitor_switch = self
                .model
                .monitor_state
                .node_monitor
                .get(&next.node_id)
                .is_some_and(|m| *m != prev_monitor);

            if needs_monitor_switch {
                if let Some(spawn_monitor) = self
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&next.node_id)
                    .cloned()
                {
                    let _ = self.activate_monitor(spawn_monitor.as_str());
                }
            }

            let did_pan = self.animate_viewport_center_to_delayed(
                next.target_center,
                now,
                Halley::VIEWPORT_PAN_PRELOAD_MS,
            );

            if needs_monitor_switch {
                let _ = self.activate_monitor(prev_monitor.as_str());
            }

            let active = crate::compositor::spawn::state::ActiveSpawnPan {
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
            };
            if did_pan {
                self.model.spawn_state.active_spawn_pan = Some(active);
                self.request_maintenance();
            } else {
                self.reveal_completed_spawn_pan(active, now, now_ms);
            }
            break;
        }
    }

    pub(crate) fn tick_pending_spawn_pan(&mut self, now: Instant, now_ms: u64) {
        if let Some((id, at_ms)) = self.model.spawn_state.pending_pan_activate {
            if now_ms >= at_ms {
                self.model.spawn_state.pending_pan_activate = None;
                if self.model.field.node(id).is_some() {
                    self.set_interaction_focus(Some(id), 30_000, now);
                }
            }
        }

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

        self.reveal_completed_spawn_pan(active, now, now_ms);
        self.model.spawn_state.active_spawn_pan = None;
        self.maybe_start_pending_spawn_pan(now);
    }

    fn reveal_completed_spawn_pan(
        &mut self,
        active: crate::compositor::spawn::state::ActiveSpawnPan,
        now: Instant,
        now_ms: u64,
    ) {
        let _ = self.model.field.set_detached(active.node_id, false);
        self.resolve_landmarks_overlapped_by_active_window(active.node_id);
        let _ = self
            .model
            .field
            .set_decay_level(active.node_id, DecayLevel::Hot);
        if let Some(intrinsic_size) = self
            .model
            .field
            .node(active.node_id)
            .map(|n| n.intrinsic_size)
        {
            self.model
                .workspace_state
                .last_active_size
                .insert(active.node_id, intrinsic_size);
        }
        let duration_ms = self.runtime.tuning.window_open_duration_ms();
        if self.runtime.tuning.window_open_animation_enabled() {
            crate::compositor::workspace::state::mark_active_transition(
                &mut **self,
                active.node_id,
                now,
                duration_ms,
            );
        }
        self.record_focus_trail_visit(active.node_id);
        self.model.focus_state.suppress_trail_record_once = true;
        self.model.spawn_state.pending_pan_activate = Some((active.node_id, now_ms + 16));
        self.request_maintenance();
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
            self.resolve_landmarks_overlapped_by_active_window(id);
            self.set_recent_top_node(id, now + std::time::Duration::from_millis(1200));
            self.set_interaction_focus(Some(id), 30_000, now);
            self.model
                .spawn_state
                .pending_spawn_activate_at_ms
                .remove(&id);
            let duration_ms = self.runtime.tuning.window_open_duration_ms();
            if self.runtime.tuning.window_open_animation_enabled() {
                crate::compositor::workspace::state::mark_active_transition(
                    &mut **self,
                    id,
                    now,
                    duration_ms,
                );
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
            self.resolve_landmarks_overlapped_by_active_window(id);
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
                crate::compositor::workspace::state::mark_active_transition(
                    &mut **self,
                    id,
                    now,
                    duration_ms,
                );
            }
            return;
        }
        match self.resolve_spawn_reveal_plan(id, is_transient) {
            RevealNewToplevelPlan::AlreadyQueued => {}
            RevealNewToplevelPlan::ActivateNow => {
                let _ = self.model.field.set_detached(id, false);
                self.resolve_landmarks_overlapped_by_active_window(id);
                self.record_focus_trail_visit(id);
                self.model.focus_state.suppress_trail_record_once = true;
                self.set_interaction_focus(Some(id), 30_000, now);
                self.model
                    .spawn_state
                    .pending_spawn_activate_at_ms
                    .remove(&id);
                let duration_ms = self.runtime.tuning.window_open_duration_ms();
                if self.runtime.tuning.window_open_animation_enabled() {
                    crate::compositor::workspace::state::mark_active_transition(
                        &mut **self,
                        id,
                        now,
                        duration_ms,
                    );
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
                initial_size: None,
                opacity: 1.0,
            },
            builtin_rule: None,
            matched_rule: true,
            is_transient: parent_node.is_some(),
            prefer_app_intent: matches!(spawn_placement, InitialWindowSpawnPlacement::App),
        }
    }

    #[test]
    fn star_offsets_are_center_then_right_left_up_down() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.pan_to_new = halley_config::PanToNewMode::Always;
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
    fn closing_all_windows_resets_default_spawn_to_view_center() {
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
        let first = state.pick_spawn_position(size).1;
        let first_id = state.model.field.spawn_surface("first", first, size);
        state.assign_node_to_current_monitor(first_id);
        state.set_interaction_focus(Some(first_id), 30_000, Instant::now());
        let second = state.pick_spawn_position(size).1;
        let second_id = state.model.field.spawn_surface("second", second, size);
        state.assign_node_to_current_monitor(second_id);
        state.set_interaction_focus(Some(second_id), 30_000, Instant::now());

        let now_ms = state.now_ms(Instant::now());
        assert!(state.remove_node_from_field(first_id, now_ms));
        state.model.monitor_state.node_monitor.remove(&first_id);
        assert!(state.remove_node_from_field(second_id, now_ms));
        state.model.monitor_state.node_monitor.remove(&second_id);

        let (_, pos, _) = state.pick_spawn_position(size);

        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn off_center_focused_window_does_not_anchor_default_spawn() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.focus_ring_rx = 100.0;
        tuning.focus_ring_ry = 100.0;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        let focused = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 400.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_current_monitor(focused);
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());

        let (_, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });

        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn empty_monitor_ignores_stale_spawn_patch() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 500.0, y: 0.0 };
        state.model.viewport.size = Vec2 {
            x: 1600.0,
            y: 1200.0,
        };
        let monitor = state.model.monitor_state.current_monitor.clone();
        state.update_spawn_patch(
            monitor.as_str(),
            Vec2 { x: -800.0, y: 0.0 },
            None,
            Vec2 { x: -800.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
        );

        let (_, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });

        assert_eq!(pos, Vec2 { x: 500.0, y: 0.0 });
    }

    #[test]
    fn stale_spawn_focus_override_is_ignored_after_panning_away() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 300.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        let size = Vec2 { x: 120.0, y: 90.0 };
        let focused = state
            .model
            .field
            .spawn_surface("focused", Vec2 { x: 0.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(focused);
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());
        let monitor = state.model.monitor_state.current_monitor.clone();
        state
            .spawn_monitor_state_mut(monitor.as_str())
            .spawn_focus_override = Some(crate::compositor::spawn::state::SpawnFocusOverride {
            pos: Vec2 { x: 0.0, y: 0.0 },
            size,
        });

        let (_, pos, _) = state.pick_spawn_position(size);

        assert_eq!(pos, state.model.viewport.center);
    }

    #[test]
    fn spawn_focus_override_is_kept_when_view_center_is_on_override() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        state.model.viewport.center = Vec2 { x: 0.0, y: 0.0 };
        state.model.viewport.size = Vec2 { x: 800.0, y: 600.0 };
        let size = Vec2 { x: 120.0, y: 90.0 };
        let focused = state
            .model
            .field
            .spawn_surface("focused", Vec2 { x: 0.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(focused);
        let monitor = state.model.monitor_state.current_monitor.clone();
        state
            .spawn_monitor_state_mut(monitor.as_str())
            .spawn_focus_override = Some(crate::compositor::spawn::state::SpawnFocusOverride {
            pos: Vec2 { x: 0.0, y: 0.0 },
            size,
        });
        let expected = state
            .right_spawn_candidate_for_focus(focused, size)
            .expect("right spawn candidate");

        let (_, pos, _) = state.pick_spawn_position(size);

        assert_eq!(pos, expected);
    }

    #[test]
    fn panning_away_ignores_stale_offscreen_focus_for_exact_view_center() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.focus_ring_rx = 100.0;
        tuning.focus_ring_ry = 100.0;
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
        let focused = state
            .model
            .field
            .spawn_surface("focused", Vec2 { x: -1000.0, y: 0.0 }, size);
        state.assign_node_to_current_monitor(focused);
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());

        let (_, pos, _) = state.pick_spawn_position(size);

        assert_eq!(pos, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn hover_focus_mode_uses_empty_pointer_monitor_for_spawn_target() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.input.focus_mode = halley_config::InputFocusMode::Hover;
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
        state.input.interaction_state.last_pointer_screen_global = Some((200.0, 120.0));

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });

        assert_eq!(state.resolve_spawn_target_monitor(), "left");
        assert_eq!(monitor, "left");
        assert_eq!(pos, state.viewport_center_for_monitor("left"));
    }

    #[test]
    fn hover_focus_mode_uses_non_empty_pointer_monitor_for_spawn_target() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.input.focus_mode = halley_config::InputFocusMode::Hover;
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

        let focused = state.model.field.spawn_surface(
            "focused-left",
            Vec2 { x: 400.0, y: 300.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(focused, "left");
        state.set_interaction_focus(Some(focused), 30_000, Instant::now());
        let existing_right = state.model.field.spawn_surface(
            "existing-right",
            Vec2 {
                x: 1000.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(existing_right, "right");
        state.input.interaction_state.last_pointer_screen_global = Some((900.0, 120.0));

        let (monitor, pos, _) = state.pick_spawn_position(Vec2 { x: 120.0, y: 90.0 });

        assert_eq!(state.resolve_spawn_target_monitor(), "right");
        assert_eq!(monitor, "right");
        assert!(pos.x >= 800.0, "spawn should stay on the pointer monitor");
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
    fn focused_cardinal_spawn_candidates_include_frame_pad() {
        let dirs = [
            ("right", Vec2 { x: 1.0, y: 0.0 }),
            ("left", Vec2 { x: -1.0, y: 0.0 }),
            ("up", Vec2 { x: 0.0, y: -1.0 }),
            ("down", Vec2 { x: 0.0, y: 1.0 }),
        ];

        for (name, dir) in dirs {
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
            let _ = state
                .model
                .field
                .set_state(focused, halley_core::field::NodeState::Active);
            state.set_interaction_focus(Some(focused), 30_000, Instant::now());

            let pos = state
                .spawn_candidate_for_focus_dir(focused, size, dir)
                .expect("spawn candidate");
            let candidate = state.model.field.spawn_surface("candidate", pos, size);
            state.assign_node_to_current_monitor(candidate);
            let _ = state
                .model
                .field
                .set_state(candidate, halley_core::field::NodeState::Active);

            let focused_node = state.model.field.node(focused).expect("focused");
            let candidate_node = state.model.field.node(candidate).expect("candidate");
            let focused_ext = state.surface_window_collision_extents(focused_node);
            let candidate_ext = state.surface_window_collision_extents(candidate_node);
            let gap = state.non_overlap_gap_world();
            let req_x = state.required_sep_x(
                focused_node.pos.x,
                focused_ext,
                candidate_node.pos.x,
                candidate_ext,
                gap,
            );
            let req_y = state.required_sep_y(
                focused_node.pos.y,
                focused_ext,
                candidate_node.pos.y,
                candidate_ext,
                gap,
            );
            let dx = (candidate_node.pos.x - focused_node.pos.x).abs();
            let dy = (candidate_node.pos.y - focused_node.pos.y).abs();

            assert!(
                dx >= req_x || dy >= req_y,
                "{name} candidate should not overlap with frame padding: dx={dx}, dy={dy}, req_x={req_x}, req_y={req_y}"
            );
        }
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
    fn adjacent_overlap_on_fullscreen_monitor_anchors_over_fullscreen() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 320.0, y: 240.0 },
            Vec2 { x: 640.0, y: 480.0 },
        );
        state.assign_node_to_current_monitor(fullscreen);
        let _ = state
            .model
            .field
            .set_state(fullscreen, halley_core::field::NodeState::Active);
        state.model.fullscreen_state.fullscreen_active_node.insert(
            state.model.monitor_state.current_monitor.clone(),
            fullscreen,
        );
        state.set_interaction_focus(Some(fullscreen), 30_000, std::time::Instant::now());

        let intent = test_intent(
            InitialWindowOverlapPolicy::All,
            InitialWindowSpawnPlacement::Adjacent,
            None,
        );
        let (_, pos, _) =
            state.pick_spawn_position_with_intent(Vec2 { x: 220.0, y: 160.0 }, &intent);

        assert_eq!(pos, Vec2 { x: 320.0, y: 240.0 });
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
