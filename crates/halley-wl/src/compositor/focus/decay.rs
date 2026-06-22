use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::*;
use crate::compositor::overlap::system::CollisionExtents;
use halley_core::viewport::{FocusRing, FocusZone};

const ACTIVE_RING_OUTSIDE_DECAY_FRAC: f32 = 0.98;

fn focus_ring_center_for_node(st: &Halley, id: NodeId) -> Vec2 {
    st.model
        .monitor_state
        .node_monitor
        .get(&id)
        .and_then(|monitor| st.model.monitor_state.monitors.get(monitor))
        .map(|monitor| monitor.viewport.center)
        .unwrap_or(st.model.viewport.center)
}

fn focus_ring_for_node(st: &Halley, id: NodeId) -> FocusRing {
    st.model
        .monitor_state
        .node_monitor
        .get(&id)
        .map(|monitor| st.runtime.tuning.focus_ring_for_output(monitor.as_str()))
        .unwrap_or_else(|| st.active_focus_ring())
}

fn focus_ring_coverage_for_extents(
    _st: &Halley,
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

fn surface_ring_coverage(st: &Halley, id: NodeId) -> (f32, f32) {
    let Some(node) = st.model.field.node(id) else {
        return (0.0, 1.0);
    };
    let focus_center = focus_ring_center_for_node(st, id);
    let focus_ring = focus_ring_for_node(st, id);

    let ext = match node.state {
        halley_core::field::NodeState::Active => st.surface_window_collision_extents(node),
        _ => st.collision_extents_for_node(node),
    };

    focus_ring_coverage_for_extents(st, node.pos, ext, focus_center, focus_ring)
}

pub(crate) fn surface_is_definitively_outside_focus_ring(st: &Halley, id: NodeId) -> bool {
    let (_, outside_frac) = surface_ring_coverage(st, id);
    outside_frac >= ACTIVE_RING_OUTSIDE_DECAY_FRAC
}

pub(crate) fn enforce_single_primary_active_unit(st: &mut Halley) {
    let now_ms = st.now_ms(Instant::now());
    let active_windows_allowed = st.runtime.tuning.field_active_windows_allowed;
    if active_windows_allowed == 0 {
        return;
    }
    if spawn_placement_transition_active(st, now_ms) {
        return;
    }
    let companion = st.companion_surface_node(now_ms);
    let preferred_surface = st.last_input_surface_node();

    let active_ids: Vec<NodeId> = st
        .model
        .field
        .nodes()
        .iter()
        .filter_map(|(&id, n)| {
            (st.model.field.participates_in_field_activity(id)
                && st.model.field.is_visible(id)
                && n.kind == halley_core::field::NodeKind::Surface
                && n.state == halley_core::field::NodeState::Active)
                .then_some(id)
        })
        .collect();

    let mut active_ids_by_monitor: HashMap<Option<String>, Vec<NodeId>> = HashMap::new();
    for id in active_ids {
        let monitor = st.model.monitor_state.node_monitor.get(&id).cloned();
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
                let monitor = st.model.monitor_state.node_monitor.get(&id);
                monitor
                    .and_then(|m| st.model.focus_state.monitor_focus.get(m))
                    .copied()
                    == Some(id)
            })
            .or_else(|| {
                active_ids.iter().copied().max_by_key(|id| {
                    st.model
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
                    let monitor = st.model.monitor_state.node_monitor.get(id);
                    monitor
                        .and_then(|m| st.model.focus_state.monitor_focus.get(m))
                        .copied()
                        == Some(*id)
                });
                let companion_rank = u8::from(companion == Some(*id));
                let inside_rank = u8::from(!surface_is_definitively_outside_focus_ring(st, *id));
                let latest_focus = st
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
            if st.is_fullscreen_session_node(id) {
                let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
                continue;
            }
            let now = Instant::now();
            crate::compositor::workspace::state::collapse_active_to_node_or_queue_auto(st, id, now);
        }
    }
}

fn spawn_placement_transition_active(st: &Halley, now_ms: u64) -> bool {
    st.model.spawn_state.active_spawn_pan.is_some()
        || !st.model.spawn_state.pending_spawn_pan_queue.is_empty()
        || st.model.spawn_state.pending_pan_activate.is_some()
        || st
            .model
            .spawn_state
            .pending_spawn_activate_at_ms
            .values()
            .any(|&at_ms| at_ms > now_ms)
        || st
            .model
            .spawn_state
            .pending_tiled_insert_reveal_at_ms
            .values()
            .any(|&at_ms| at_ms > now_ms)
}

pub fn apply_single_surface_decay_policy(
    st: &mut Halley,
    id: NodeId,
    now_ms: u64,
    active_delay_ms: u64,
    inactive_delay_ms: u64,
) {
    let Some(n) = st.model.field.node(id) else {
        return;
    };
    if !st.model.field.participates_in_field_activity(id)
        || !st.model.field.is_visible(id)
        || n.kind != halley_core::field::NodeKind::Surface
    {
        return;
    }

    if st.runtime.tuning.field_active_windows_allowed == 0 {
        st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
        return;
    }

    if st.is_fullscreen_session_node(id) {
        st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
        return;
    }

    if crate::compositor::workspace::state::preserve_collapsed_surface(st, id) {
        st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
        return;
    }

    if is_hard_decay_protected(st, id, now_ms) {
        st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
        return;
    }

    let outside_ring = surface_is_definitively_outside_focus_ring(st, id);
    if !outside_ring {
        st.model.focus_state.outside_focus_ring_since_ms.remove(&id);
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
        return;
    }

    let is_primary = st.model.focus_state.primary_interaction_focus == Some(id);
    let delay_ms = if is_primary {
        active_delay_ms
    } else {
        inactive_delay_ms
    };

    let outside_since_ms = *st
        .model
        .focus_state
        .outside_focus_ring_since_ms
        .entry(id)
        .or_insert(now_ms);

    if now_ms.saturating_sub(outside_since_ms) >= delay_ms {
        let now = Instant::now();
        crate::compositor::workspace::state::collapse_active_to_node_or_queue_auto(st, id, now);
    } else {
        let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
    }
}

fn is_hard_decay_protected(st: &Halley, id: NodeId, now_ms: u64) -> bool {
    st.model.focus_state.primary_interaction_focus == Some(id)
        || st.is_fullscreen_session_node(id)
        || st.input.interaction_state.resize_active == Some(id)
        || crate::compositor::interaction::state::is_recently_resized_node(st, id, now_ms)
        || st.model.carry_state.carry_zone_hint.contains_key(&id)
        || st
            .model
            .workspace_state
            .active_transitions
            .contains_key(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outside_ring_test_state() -> (Halley, NodeId) {
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
            .cache
            .window_geometry
            .insert(id, (-50.0, -50.0, 100.0, 100.0));
        state.ui.render_state.cache.bbox_loc.insert(id, (0.0, 0.0));

        (state, id)
    }

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
            .cache
            .window_geometry
            .insert(id, (-50.0, -50.0, 100.0, 100.0));
        state.ui.render_state.cache.bbox_loc.insert(id, (0.0, 0.0));

        assert!(!state.surface_is_definitively_outside_focus_ring(id));
    }

    #[test]
    fn active_surface_fully_clear_of_ring_is_treated_as_outside() {
        let (state, id) = outside_ring_test_state();

        assert!(state.surface_is_definitively_outside_focus_ring(id));
    }

    #[test]
    fn outside_ring_decay_waits_for_exit_delay_not_last_focus_time() {
        let (mut state, id) = outside_ring_test_state();
        state.model.focus_state.last_surface_focus_ms.insert(id, 0);

        state.apply_single_surface_decay_policy(id, 100_000, 120_000, 30_000);

        assert_eq!(
            state.model.field.node(id).map(|n| n.decay),
            Some(DecayLevel::Hot)
        );
        assert_eq!(
            state.model.focus_state.outside_focus_ring_since_ms.get(&id),
            Some(&100_000)
        );
    }

    #[test]
    fn outside_ring_decay_turns_cold_after_delay_from_exit() {
        let (mut state, id) = outside_ring_test_state();

        state.apply_single_surface_decay_policy(id, 100_000, 120_000, 30_000);
        state.apply_single_surface_decay_policy(id, 129_999, 120_000, 30_000);
        assert_eq!(
            state.model.field.node(id).map(|n| n.decay),
            Some(DecayLevel::Hot)
        );

        state.apply_single_surface_decay_policy(id, 130_000, 120_000, 30_000);
        assert_eq!(
            state.model.field.node(id).map(|n| n.decay),
            Some(DecayLevel::Hot)
        );
        assert!(
            state
                .model
                .workspace_state
                .pending_collapses
                .contains_key(&id)
        );

        let monitor = state.model.monitor_state.current_monitor.clone();
        crate::compositor::workspace::state::process_pending_collapses_for_monitor(
            &mut state,
            monitor.as_str(),
            Instant::now() + std::time::Duration::from_millis(140),
        );
        assert_eq!(
            state.model.field.node(id).map(|n| n.decay),
            Some(DecayLevel::Cold)
        );
    }

    #[test]
    fn fullscreen_active_surface_never_decays_to_node() {
        let (mut state, id) = outside_ring_test_state();
        let monitor = state.model.monitor_state.current_monitor.clone();
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor, id);
        state
            .model
            .focus_state
            .outside_focus_ring_since_ms
            .insert(id, 5);

        state.apply_single_surface_decay_policy(id, 130_000, 120_000, 30_000);

        let node = state.model.field.node(id).expect("fullscreen node");
        assert_eq!(node.decay, DecayLevel::Hot);
        assert_eq!(node.state, halley_core::field::NodeState::Active);
        assert!(
            !state
                .model
                .focus_state
                .outside_focus_ring_since_ms
                .contains_key(&id)
        );
    }

    #[test]
    fn fullscreen_active_surface_is_exempt_from_active_window_limit_decay() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.field_active_windows_allowed = 1;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "game",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 800.0, y: 600.0 },
        );
        let other = state.model.field.spawn_surface(
            "other",
            Vec2 { x: 160.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        for id in [fullscreen, other] {
            state.assign_node_to_monitor(id, monitor.as_str());
        }
        state
            .model
            .fullscreen_state
            .fullscreen_active_node
            .insert(monitor, fullscreen);

        state.enforce_single_primary_active_unit();

        assert_eq!(
            state.model.field.node(fullscreen).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
    }

    #[test]
    fn active_window_limit_collapse_places_node_out_from_under_active_window() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.field_active_windows_allowed = 1;
        tuning.animations.window_close.enabled = false;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let monitor = state.model.monitor_state.current_monitor.clone();
        let keeper = state.model.field.spawn_surface(
            "keeper",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        for id in [keeper, target] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .focus_state
            .monitor_focus
            .insert(monitor.clone(), keeper);
        let origin = state.model.field.node(target).expect("target").pos;

        state.enforce_single_primary_active_unit();

        let resolved = state.model.field.node(target).expect("target").pos;
        assert_eq!(
            state
                .model
                .field
                .node(keeper)
                .map(|node| node.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
        assert_eq!(
            state
                .model
                .field
                .node(target)
                .map(|node| node.state.clone()),
            Some(halley_core::field::NodeState::Node)
        );
        assert_ne!(resolved, origin);
        assert!(
            !state
                .model
                .workspace_state
                .manual_collapsed_nodes
                .contains(&target)
        );
        let slide = state
            .ui
            .render_state
            .window_animations
            .landmark_slide_animations
            .get(&target)
            .expect("landmark slide animation");
        assert_eq!(slide.from, origin);
        assert_eq!(slide.to, resolved);
    }

    #[test]
    fn active_window_limit_waits_for_close_capture_before_auto_collapse() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.field_active_windows_allowed = 1;
        tuning.animations.window_close.enabled = true;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let monitor = state.model.monitor_state.current_monitor.clone();
        let keeper = state.model.field.spawn_surface(
            "keeper",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 420.0, y: 280.0 },
        );
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        for id in [keeper, target] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .focus_state
            .monitor_focus
            .insert(monitor.clone(), keeper);
        let now = Instant::now();

        state.enforce_single_primary_active_unit();

        assert_eq!(
            state.model.field.node(target).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
        assert!(
            state
                .model
                .workspace_state
                .pending_collapses
                .contains_key(&target)
        );

        crate::compositor::workspace::state::process_pending_collapses_for_monitor(
            &mut state,
            monitor.as_str(),
            now + std::time::Duration::from_millis(140),
        );

        assert_eq!(
            state.model.field.node(target).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Node)
        );
        assert!(
            !state
                .model
                .workspace_state
                .manual_collapsed_nodes
                .contains(&target)
        );
    }

    #[test]
    fn active_window_limit_waits_during_open_transition() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.field_active_windows_allowed = 1;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let first = state.model.field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        let second = state.model.field.spawn_surface(
            "second",
            Vec2 { x: 180.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        let monitor = state.model.monitor_state.current_monitor.clone();
        for id in [first, second] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .spawn_state
            .pending_spawn_activate_at_ms
            .insert(second, u64::MAX);

        state.enforce_single_primary_active_unit();

        assert_eq!(
            state.model.field.node(first).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
        assert_eq!(
            state.model.field.node(second).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
    }

    #[test]
    fn active_window_limit_does_not_wait_for_visual_active_transition() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.field_active_windows_allowed = 1;
        tuning.animations.window_close.enabled = false;
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);
        let monitor = state.model.monitor_state.current_monitor.clone();
        let keeper = state.model.field.spawn_surface(
            "keeper",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 180.0, y: 0.0 },
            Vec2 { x: 100.0, y: 100.0 },
        );
        for id in [keeper, target] {
            state.assign_node_to_monitor(id, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(id, halley_core::field::NodeState::Active);
        }
        state
            .model
            .focus_state
            .monitor_focus
            .insert(monitor, keeper);
        state.model.workspace_state.active_transitions.insert(
            target,
            crate::compositor::workspace::state::ActiveTransition {
                started_at_ms: 0,
                duration_ms: u64::MAX,
            },
        );

        state.enforce_single_primary_active_unit();

        assert_eq!(
            state.model.field.node(keeper).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Active)
        );
        assert_eq!(
            state.model.field.node(target).map(|n| n.state.clone()),
            Some(halley_core::field::NodeState::Node)
        );
    }

    #[test]
    fn zero_active_window_limit_disables_decay() {
        let (mut state, id) = outside_ring_test_state();
        state.runtime.tuning.field_active_windows_allowed = 0;
        state
            .model
            .focus_state
            .outside_focus_ring_since_ms
            .insert(id, 5);
        let _ = state.model.field.set_decay_level(id, DecayLevel::Cold);

        state.apply_single_surface_decay_policy(id, 100_000, 120_000, 30_000);

        assert_eq!(
            state.model.field.node(id).map(|n| n.decay),
            Some(DecayLevel::Hot)
        );
        assert!(
            !state
                .model
                .focus_state
                .outside_focus_ring_since_ms
                .contains_key(&id)
        );
    }
}
