use std::collections::HashMap;
use std::time::Instant;

pub(crate) mod placement;

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
use crate::compositor::spawn::state::{
    ActiveSpawnPan, MonitorSpawnState, PendingSpawnPan, SpawnState,
};
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

    if let Some(size) = toplevel.with_committed_state(|state| state.and_then(|state| state.size)) {
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

impl SpawnCtx<'_> {
    pub(crate) fn needs_deferred_rule_recheck(&self, intent: &InitialWindowIntent) -> bool {
        crate::compositor::spawn::rules::needs_deferred_rule_recheck(self.st, intent)
    }

    pub(crate) fn spawn_target_monitor_for_intent(&self, intent: &InitialWindowIntent) -> String {
        self.st.spawn_target_monitor_for_intent(intent)
    }

    pub(crate) fn cluster_bloom_open_on_monitor(&self, monitor: &str) -> bool {
        self.st
            .model
            .cluster_state
            .cluster_bloom_open
            .contains_key(monitor)
    }

    pub(crate) fn active_cluster_spawn_rect_for_new_member(
        &self,
        monitor: &str,
    ) -> Option<halley_core::tiling::Rect> {
        let cid = self.st.active_cluster_workspace_for_monitor(monitor)?;
        self.st.cluster_spawn_rect_for_new_member(monitor, cid)
    }

    pub(crate) fn default_initial_toplevel_size(&self) -> (i32, i32) {
        (
            (self.st.model.viewport.size.x * 0.46).round() as i32,
            (self.st.model.viewport.size.y * 0.42).round() as i32,
        )
    }

    pub(crate) fn reveal_new_toplevel_node(
        &mut self,
        id: NodeId,
        is_transient: bool,
        now: Instant,
    ) {
        self.st.reveal_new_toplevel_node(id, is_transient, now);
    }
}

pub(crate) fn initial_toplevel_size(
    ctx: &mut SpawnCtx<'_>,
    toplevel: &ToplevelSurface,
    intent: &InitialWindowIntent,
) -> InitialToplevelSize {
    let defer_rule_resolution = ctx.needs_deferred_rule_recheck(intent);
    let predicted_monitor = ctx.spawn_target_monitor_for_intent(intent);
    let stack_mode_open = ctx.cluster_bloom_open_on_monitor(predicted_monitor.as_str());
    if !defer_rule_resolution
        && !stack_mode_open
        && intent.rule.cluster_participation
            == halley_config::InitialWindowClusterParticipation::Layout
        && let Some(rect) = ctx.active_cluster_spawn_rect_for_new_member(predicted_monitor.as_str())
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
    let node_size = detected.unwrap_or_else(|| ctx.default_initial_toplevel_size());

    InitialToplevelSize {
        node_size,
        configure_size: None,
    }
}

pub(crate) fn reveal_new_toplevel_node_via_ctx(
    ctx: &mut SpawnCtx<'_>,
    id: NodeId,
    is_transient: bool,
    now: Instant,
) {
    ctx.reveal_new_toplevel_node(id, is_transient, now);
}

pub(crate) fn spawn_pan_active(st: &Halley) -> bool {
    st.model.spawn_state.active_spawn_pan.is_some()
}

pub(crate) fn active_spawn_pan(st: &Halley) -> Option<ActiveSpawnPan> {
    st.model.spawn_state.active_spawn_pan
}

pub(crate) fn set_active_spawn_pan(st: &mut Halley, active: ActiveSpawnPan) {
    st.model.spawn_state.active_spawn_pan = Some(active);
}

pub(crate) fn clear_active_spawn_pan(st: &mut Halley) {
    st.model.spawn_state.active_spawn_pan = None;
}

pub(crate) fn pop_pending_spawn_pan(st: &mut Halley) -> Option<PendingSpawnPan> {
    st.model.spawn_state.pending_spawn_pan_queue.pop_front()
}

pub(crate) fn node_exists(st: &Halley, id: NodeId) -> bool {
    st.model.field.node(id).is_some()
}

pub(crate) fn current_monitor_name(st: &Halley) -> String {
    st.model.monitor_state.current_monitor.clone()
}

pub(crate) fn monitor_for_node(st: &Halley, id: NodeId) -> Option<String> {
    st.model.monitor_state.node_monitor.get(&id).cloned()
}

pub(crate) fn monitor_for_node_or_current(st: &Halley, id: NodeId) -> String {
    monitor_for_node(st, id).unwrap_or_else(|| current_monitor_name(st))
}

pub(crate) fn activate_node_monitor_for_spawn_pan(
    st: &mut Halley,
    node_id: NodeId,
) -> Option<String> {
    let previous_monitor = current_monitor_name(st);
    let Some(spawn_monitor) = monitor_for_node(st, node_id) else {
        return None;
    };
    if spawn_monitor == previous_monitor {
        return None;
    }
    let _ = st.activate_monitor(spawn_monitor.as_str());
    Some(previous_monitor)
}

pub(crate) fn restore_spawn_pan_monitor(st: &mut Halley, previous_monitor: Option<String>) {
    if let Some(previous_monitor) = previous_monitor {
        let _ = st.activate_monitor(previous_monitor.as_str());
    }
}

pub(crate) fn build_active_spawn_pan(
    node_id: NodeId,
    did_pan: bool,
    now_ms: u64,
) -> ActiveSpawnPan {
    ActiveSpawnPan {
        node_id,
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
    }
}

pub(crate) fn complete_due_pending_pan_activation(st: &mut Halley, now: Instant, now_ms: u64) {
    let Some((id, at_ms)) = st.model.spawn_state.pending_pan_activate else {
        return;
    };
    if now_ms < at_ms {
        return;
    }
    st.model.spawn_state.pending_pan_activate = None;
    if node_exists(st, id) {
        st.set_interaction_focus(Some(id), 30_000, now);
    }
}

pub(crate) fn viewport_pan_finished_for_spawn(
    st: &Halley,
    active: ActiveSpawnPan,
    now_ms: u64,
) -> bool {
    now_ms >= active.reveal_at_ms
        || (now_ms >= active.pan_start_at_ms
            && st.input.interaction_state.viewport_pan_anim.is_none())
}

pub(crate) fn reveal_spawn_node(st: &mut Halley, id: NodeId) {
    let _ = st.model.field.set_detached(id, false);
    st.resolve_landmarks_overlapped_by_active_window(id);
}

pub(crate) fn remember_spawn_node_size(st: &mut Halley, id: NodeId) {
    if let Some(intrinsic_size) = st.model.field.node(id).map(|n| n.intrinsic_size) {
        st.model
            .workspace_state
            .last_active_size
            .insert(id, intrinsic_size);
    }
}

pub(crate) fn mark_spawn_node_hot(st: &mut Halley, id: NodeId) {
    let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
}

pub(crate) fn mark_spawn_open_transition(st: &mut Halley, id: NodeId, now: Instant) {
    let duration_ms = st.runtime.tuning.window_open_duration_ms();
    if st.runtime.tuning.window_open_animation_enabled() {
        crate::compositor::workspace::state::mark_active_transition(&mut *st, id, now, duration_ms);
    }
}

pub(crate) fn remove_pending_spawn_activation(st: &mut Halley, id: NodeId) {
    st.model
        .spawn_state
        .pending_spawn_activate_at_ms
        .remove(&id);
}

pub(crate) fn suppress_next_focus_trail_record(st: &mut Halley) {
    st.model.focus_state.suppress_trail_record_once = true;
}

pub(crate) fn activate_revealed_spawn_node(
    st: &mut Halley,
    id: NodeId,
    now: Instant,
    record_trail: bool,
    set_recent_top: bool,
) {
    reveal_spawn_node(st, id);
    if set_recent_top {
        st.set_recent_top_node(id, now + std::time::Duration::from_millis(1200));
    }
    if record_trail {
        st.record_focus_trail_visit(id);
        suppress_next_focus_trail_record(st);
    }
    st.set_interaction_focus(Some(id), 30_000, now);
    remove_pending_spawn_activation(st, id);
    mark_spawn_open_transition(st, id, now);
}

pub(crate) fn queue_spawn_pan(st: &mut Halley, id: NodeId, target_center: Vec2) {
    let _ = st.model.field.set_detached(id, true);
    remove_pending_spawn_activation(st, id);
    st.model
        .spawn_state
        .pending_spawn_pan_queue
        .push_back(PendingSpawnPan {
            node_id: id,
            target_center,
        });
}

pub(crate) fn pending_tiled_insert_reveal(st: &Halley, id: NodeId) -> bool {
    st.model
        .spawn_state
        .pending_tiled_insert_reveal_at_ms
        .contains_key(&id)
}

pub(crate) fn pending_initial_reveal(st: &Halley, id: NodeId) -> bool {
    st.model.spawn_state.pending_initial_reveal.contains(&id)
}

pub(crate) fn suppress_reveal_pan_rule(st: &Halley, id: NodeId) -> bool {
    st.model
        .spawn_state
        .applied_window_rules
        .get(&id)
        .is_some_and(|rule| rule.suppress_reveal_pan)
}

pub(crate) fn node_is_local_active_cluster_member(st: &Halley, id: NodeId, monitor: &str) -> bool {
    st.model
        .field
        .cluster_id_for_member_public(id)
        .is_some_and(|cid| st.active_cluster_workspace_for_monitor(monitor) == Some(cid))
}

pub(crate) fn maybe_start_pending_spawn_pan(st: &mut Halley, now: Instant) {
    if spawn_pan_active(st) {
        return;
    }

    let now_ms = st.now_ms(now);
    while let Some(next) = pop_pending_spawn_pan(st) {
        if !node_exists(st, next.node_id) {
            continue;
        }

        let previous_monitor = activate_node_monitor_for_spawn_pan(st, next.node_id);

        let did_pan = st.animate_viewport_center_to_delayed(
            next.target_center,
            now,
            Halley::VIEWPORT_PAN_PRELOAD_MS,
        );

        restore_spawn_pan_monitor(st, previous_monitor);

        let active = build_active_spawn_pan(next.node_id, did_pan, now_ms);
        if did_pan {
            set_active_spawn_pan(st, active);
            st.request_maintenance();
        } else {
            reveal_completed_spawn_pan(st, active, now, now_ms);
        }
        break;
    }
}

pub(crate) fn tick_pending_spawn_pan(st: &mut Halley, now: Instant, now_ms: u64) {
    complete_due_pending_pan_activation(st, now, now_ms);

    let Some(active) = active_spawn_pan(st) else {
        maybe_start_pending_spawn_pan(st, now);
        return;
    };

    if !node_exists(st, active.node_id) {
        clear_active_spawn_pan(st);
        maybe_start_pending_spawn_pan(st, now);
        return;
    }

    if !viewport_pan_finished_for_spawn(st, active, now_ms) {
        return;
    }

    reveal_completed_spawn_pan(st, active, now, now_ms);
    clear_active_spawn_pan(st);
    maybe_start_pending_spawn_pan(st, now);
}

pub(crate) fn reveal_completed_spawn_pan(
    st: &mut Halley,
    active: ActiveSpawnPan,
    now: Instant,
    now_ms: u64,
) {
    reveal_spawn_node(st, active.node_id);
    mark_spawn_node_hot(st, active.node_id);
    remember_spawn_node_size(st, active.node_id);
    mark_spawn_open_transition(st, active.node_id, now);
    st.record_focus_trail_visit(active.node_id);
    suppress_next_focus_trail_record(st);
    st.model.spawn_state.pending_pan_activate = Some((active.node_id, now_ms + 16));
    st.request_maintenance();
}

pub(crate) fn reveal_new_toplevel_node(
    st: &mut Halley,
    id: NodeId,
    is_transient: bool,
    now: Instant,
) {
    let node_monitor = monitor_for_node_or_current(st, id);
    let cluster_local = node_is_local_active_cluster_member(st, id, node_monitor.as_str());
    if pending_tiled_insert_reveal(st, id) {
        return;
    }
    if cluster_local {
        activate_revealed_spawn_node(st, id, now, false, true);
        return;
    }
    if pending_initial_reveal(st, id) {
        return;
    }
    if suppress_reveal_pan_rule(st, id) {
        activate_revealed_spawn_node(st, id, now, true, true);
        return;
    }
    match resolve_spawn_reveal_plan(st, id, is_transient) {
        RevealNewToplevelPlan::AlreadyQueued => {}
        RevealNewToplevelPlan::ActivateNow => {
            activate_revealed_spawn_node(st, id, now, true, false);
        }
        RevealNewToplevelPlan::QueuePan { target_center } => {
            queue_spawn_pan(st, id, target_center);
            maybe_start_pending_spawn_pan(st, now);
        }
    }
}

pub(crate) fn resolve_spawn_reveal_plan(
    st: &Halley,
    id: NodeId,
    is_transient: bool,
) -> read::RevealNewToplevelPlan {
    read::spawn_read_context(st).reveal_new_toplevel_plan(st, id, is_transient)
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
