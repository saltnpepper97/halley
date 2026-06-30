use std::collections::{HashMap, HashSet};
use std::time::Instant;

use eventline::debug;
use halley_core::decay::DecayLevel;
use halley_core::field::NodeId;
use halley_core::trail::Trail;
use halley_core::viewport::{FocusRing, FocusZone};

use crate::compositor::root::Halley;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;

pub(crate) struct FocusState {
    pub(crate) primary_interaction_focus: Option<NodeId>,
    pub(crate) monitor_focus: HashMap<String, NodeId>,
    pub(crate) blocked_monitor_focus_restore: HashSet<String>,
    pub(crate) interaction_focus_until_ms: u64,
    pub(crate) last_surface_focus_ms: HashMap<NodeId, u64>,
    pub(crate) outside_focus_ring_since_ms: HashMap<NodeId, u64>,
    pub(crate) focus_trail: HashMap<String, Trail>,
    pub(crate) suppress_trail_record_once: bool,
    pub(crate) pan_restore_active_focus: Option<NodeId>,
    pub(crate) app_focused: bool,
    pub(crate) focus_ring_preview_until_ms: HashMap<String, u64>,
    pub(crate) recent_top_node: Option<NodeId>,
    pub(crate) recent_top_until: Option<Instant>,
    pub(crate) overlap_raise_order: HashMap<NodeId, u64>,
    pub(crate) next_overlap_raise_order: u64,
}

pub(crate) const COMPANION_PROTECT_MS: u64 = 12_000;
pub(crate) const FOCUS_RING_PREVIEW_MS: u64 = 1_500;

pub(crate) fn companion_surface_node(st: &Halley, now_ms: u64) -> Option<NodeId> {
    let focused = st.model.focus_state.primary_interaction_focus;
    st.model
        .focus_state
        .last_surface_focus_ms
        .iter()
        .filter_map(|(&id, &at)| {
            if Some(id) == focused {
                return None;
            }
            if now_ms.saturating_sub(at) > COMPANION_PROTECT_MS {
                return None;
            }
            st.model.field.node(id).and_then(|n| {
                (st.model.field.is_visible(id) && n.kind == halley_core::field::NodeKind::Surface)
                    .then_some((id, at))
            })
        })
        .max_by_key(|(id, at)| (*at, id.as_u64()))
        .map(|(id, _)| id)
}

pub fn active_focus_ring(st: &Halley) -> FocusRing {
    st.runtime
        .tuning
        .focus_ring_for_output(st.model.monitor_state.current_monitor.as_str())
}

pub fn focus_ring_for_monitor(st: &Halley, monitor: &str) -> FocusRing {
    st.runtime.tuning.focus_ring_for_output(monitor)
}

pub fn should_draw_focus_ring_preview(st: &Halley, now: Instant) -> bool {
    st.model
        .focus_state
        .focus_ring_preview_until_ms
        .get(st.model.monitor_state.current_monitor.as_str())
        .is_some_and(|&until_ms| st.now_ms(now) < until_ms)
}

#[allow(dead_code)]
pub(crate) fn focused_node_for_monitor(st: &Halley, monitor: &str) -> Option<NodeId> {
    st.model.focus_state.monitor_focus.get(monitor).copied()
}

#[allow(dead_code)]
pub(crate) fn focused_monitor_for_node(st: &Halley, id: NodeId) -> Option<String> {
    st.model.monitor_state.node_monitor.get(&id).cloned()
}

pub fn overlap_policy_stack_rank(st: &Halley, node_id: NodeId) -> (u64, u64) {
    (
        st.model
            .focus_state
            .overlap_raise_order
            .get(&node_id)
            .copied()
            .unwrap_or(0),
        node_id.as_u64(),
    )
}

fn active_surface_is_frontmost_on_monitor(st: &Halley, node_id: NodeId) -> bool {
    let Some(target_monitor) = st.model.monitor_state.node_monitor.get(&node_id) else {
        return false;
    };
    let target_rank = overlap_policy_stack_rank(st, node_id);
    st.model
        .field
        .nodes()
        .iter()
        .filter(|&(id, node)| {
            *id != node_id
                && node.kind == halley_core::field::NodeKind::Surface
                && node.state == halley_core::field::NodeState::Active
                && st.model.field.is_visible(*id)
                && st
                    .model
                    .monitor_state
                    .node_monitor
                    .get(id)
                    .is_some_and(|monitor| monitor == target_monitor)
        })
        .all(|(&id, _)| overlap_policy_stack_rank(st, id) <= target_rank)
}

fn active_surface_world_rect(st: &Halley, node_id: NodeId) -> Option<(f32, f32, f32, f32)> {
    let node = st.model.field.node(node_id)?;
    if node.kind != halley_core::field::NodeKind::Surface
        || node.state != halley_core::field::NodeState::Active
        || !st.model.field.is_visible(node_id)
    {
        return None;
    }
    let size = node.intrinsic_size;
    let left = node.pos.x - size.x * 0.5;
    let top = node.pos.y - size.y * 0.5;
    Some((left, top, left + size.x.max(1.0), top + size.y.max(1.0)))
}

fn active_surface_overlaps_visible_peer_on_monitor(st: &Halley, node_id: NodeId) -> bool {
    let Some(target_monitor) = st.model.monitor_state.node_monitor.get(&node_id) else {
        return false;
    };
    let Some(target_rect) = active_surface_world_rect(st, node_id) else {
        return false;
    };
    st.model.field.nodes().iter().any(|(&id, node)| {
        id != node_id
            && node.kind == halley_core::field::NodeKind::Surface
            && node.state == halley_core::field::NodeState::Active
            && st.model.field.is_visible(id)
            && st
                .model
                .monitor_state
                .node_monitor
                .get(&id)
                .is_some_and(|monitor| monitor == target_monitor)
            && active_surface_world_rect(st, id)
                .is_some_and(|rect| rects_overlap(target_rect, rect))
    })
}

fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

pub(crate) fn focus_monitor_view(st: &mut Halley, monitor: &str, now: Instant) {
    let open_monitors = st
        .model
        .cluster_state
        .cluster_bloom_open
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for open_monitor in open_monitors {
        if open_monitor != monitor {
            let _ = st.close_cluster_bloom_for_monitor(open_monitor.as_str());
        }
    }
    st.set_interaction_monitor(monitor);
    st.set_focused_monitor(monitor);
    // A deliberate monitor-focus keybind pins the spawn target to this monitor
    // until the pointer moves, so it isn't overridden by hover focus-mode when the
    // cursor is parked on a different monitor.
    st.input.interaction_state.monitor_focus_pinned = true;
    let _ = st.activate_monitor(monitor);
    if !st
        .model
        .focus_state
        .blocked_monitor_focus_restore
        .contains(monitor)
        && let Some(id) = st.last_focused_surface_node_for_monitor(monitor)
    {
        set_interaction_focus(st, Some(id), 30_000, now);
        debug!(
            "monitor focus restored surface: monitor={} node_id={}",
            monitor,
            id.as_u64()
        );
        return;
    }
    set_interaction_focus(st, None, 0, now);
    let view_center = st.view_center_for_monitor(monitor);
    let spawn = st.spawn_monitor_state_mut(monitor);
    spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::View;
    spawn.spawn_view_anchor = view_center;
    spawn.spawn_patch = None;
    spawn.spawn_pan_start_center = None;
    debug!(
        "monitor view focus set: monitor={} interaction_monitor={} focused_monitor={}",
        monitor,
        st.interaction_monitor(),
        st.focused_monitor()
    );
}

pub fn set_interaction_focus(st: &mut Halley, id: Option<NodeId>, hold_ms: u64, now: Instant) {
    let prev = st.model.focus_state.primary_interaction_focus;
    let now_ms = st.now_ms(now);

    if prev == id {
        if let Some(fid) = id {
            let requested_until = now_ms.saturating_add(hold_ms.max(1));
            st.model.focus_state.interaction_focus_until_ms = st
                .model
                .focus_state
                .interaction_focus_until_ms
                .max(requested_until);
            st.update_focus_tracking_for_surface(fid, now_ms);
            if let Some(monitor) = st.model.monitor_state.node_monitor.get(&fid).cloned() {
                st.model
                    .focus_state
                    .blocked_monitor_focus_restore
                    .remove(&monitor);
                st.set_interaction_monitor(monitor.as_str());
                st.set_focused_monitor(monitor.as_str());
                let _ = st.activate_monitor(monitor.as_str());
                let spawn = st.spawn_monitor_state_mut(monitor.as_str());
                spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::Focus;
                spawn.spawn_pan_start_center = None;
                st.model.focus_state.monitor_focus.insert(monitor, fid);
            } else {
                let current_monitor = st.model.monitor_state.current_monitor.clone();
                let spawn = st.spawn_monitor_state_mut(current_monitor.as_str());
                spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::Focus;
                spawn.spawn_pan_start_center = None;
            }

            reassert_wayland_keyboard_focus_if_drifted(st, id);
        } else {
            st.model.focus_state.interaction_focus_until_ms = 0;
            reassert_wayland_keyboard_focus_if_drifted(st, None);
        }
        st.request_maintenance();
        return;
    }

    st.model.focus_state.primary_interaction_focus = id;
    if let Some(fid) = id {
        st.model.focus_state.interaction_focus_until_ms = now_ms.saturating_add(hold_ms.max(1));
        st.update_focus_tracking_for_surface(fid, now_ms);
        if let Some(monitor) = st.model.monitor_state.node_monitor.get(&fid).cloned() {
            let open_monitors = st
                .model
                .cluster_state
                .cluster_bloom_open
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for open_monitor in open_monitors {
                if open_monitor != monitor {
                    let _ = st.close_cluster_bloom_for_monitor(open_monitor.as_str());
                }
            }
            st.model
                .focus_state
                .blocked_monitor_focus_restore
                .remove(&monitor);
            st.set_interaction_monitor(monitor.as_str());
            st.set_focused_monitor(monitor.as_str());
            let _ = st.activate_monitor(monitor.as_str());
            let spawn = st.spawn_monitor_state_mut(monitor.as_str());
            spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::Focus;
            spawn.spawn_pan_start_center = None;
            st.model.focus_state.monitor_focus.insert(monitor, fid);
        } else {
            let current_monitor = st.model.monitor_state.current_monitor.clone();
            let spawn = st.spawn_monitor_state_mut(current_monitor.as_str());
            spawn.spawn_anchor_mode = crate::compositor::spawn::state::SpawnAnchorMode::Focus;
            spawn.spawn_pan_start_center = None;
        }
    } else {
        st.model.focus_state.interaction_focus_until_ms = 0;
    }

    if prev != id {
        debug!(
            "interaction focus changed: {:?} -> {:?} (hold_ms={})",
            prev.map(|n| n.as_u64()),
            id.map(|n| n.as_u64()),
            hold_ms
        );
    }
    st.apply_wayland_focus_state(id);
    st.request_maintenance();
}

pub(crate) fn restore_pan_return_active_focus(st: &mut Halley, now: Instant) {
    if !st.runtime.tuning.restore_last_active_on_pan_return {
        st.model.focus_state.pan_restore_active_focus = None;
        return;
    }
    let Some(id) = st.model.focus_state.pan_restore_active_focus else {
        return;
    };
    let Some(n) = st.model.field.node(id) else {
        st.model.focus_state.pan_restore_active_focus = None;
        return;
    };
    if !st.model.field.is_visible(id) || n.kind != halley_core::field::NodeKind::Surface {
        st.model.focus_state.pan_restore_active_focus = None;
        return;
    }
    if n.state == halley_core::field::NodeState::Active {
        st.model.focus_state.pan_restore_active_focus = None;
        return;
    }

    if crate::compositor::workspace::state::preserve_collapsed_surface(st, id) {
        st.model.focus_state.pan_restore_active_focus = None;
        return;
    }

    let target_monitor = st
        .model
        .monitor_state
        .node_monitor
        .get(&id)
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone());
    let focus_center = st.view_center_for_monitor(target_monitor.as_str());
    let focus_ring = focus_ring_for_monitor(st, target_monitor.as_str());
    if focus_ring.zone(focus_center, n.pos) != FocusZone::Inside {
        return;
    }

    let _ = st.model.field.set_decay_level(id, DecayLevel::Hot);
    crate::compositor::workspace::state::mark_active_transition(st, id, now, 280);

    set_interaction_focus(st, Some(id), 12_000, now);
    st.model.focus_state.pan_restore_active_focus = None;
}

pub fn reassert_wayland_keyboard_focus_if_drifted(st: &mut Halley, id: Option<NodeId>) {
    if st.model.monitor_state.layer_keyboard_focus.is_some() {
        crate::compositor::monitor::layer_shell::reassert_layer_surface_keyboard_focus_if_drifted(
            st,
        );
        return;
    }
    let desired_focus =
        id.and_then(|fid| crate::compositor::focus::system::wl_surface_for_node(st, fid));
    if let Some(keyboard) = st.platform.seat.get_keyboard() {
        let current_focus = keyboard.current_focus();
        let matches = match (&current_focus, &desired_focus) {
            (Some(current), Some(desired)) => current.id() == desired.id(),
            (None, None) => true,
            _ => false,
        };
        if !matches {
            debug!(
                "keyboard focus drift detected; reasserting desired focus={:?} current={:?}",
                desired_focus.as_ref().map(|wl| format!("{:?}", wl.id())),
                current_focus.as_ref().map(|wl| format!("{:?}", wl.id()))
            );
            crate::compositor::focus::system::set_keyboard_focus(
                st,
                desired_focus.clone(),
                SERIAL_COUNTER.next_serial(),
            );
            st.update_selection_focus_from_surface(desired_focus.as_ref());
        }
    }
}

#[allow(dead_code)]
pub(crate) fn set_monitor_focus(st: &mut Halley, monitor: &str, id: NodeId) {
    st.model
        .focus_state
        .monitor_focus
        .insert(monitor.to_string(), id);
}

pub fn set_recent_top_node(st: &mut Halley, node_id: NodeId, until: Instant) {
    st.model.focus_state.recent_top_node = Some(node_id);
    st.model.focus_state.recent_top_until = Some(until);
}

pub fn raise_overlap_policy_node(st: &mut Halley, node_id: NodeId) -> bool {
    if !st.model.field.node(node_id).is_some_and(|node| {
        node.kind == halley_core::field::NodeKind::Surface
            && node.state == halley_core::field::NodeState::Active
            && st.model.field.is_visible(node_id)
    }) {
        return false;
    }
    let needs_raise_above_fullscreen = st
        .model
        .monitor_state
        .node_monitor
        .get(&node_id)
        .and_then(|monitor| {
            st.model
                .fullscreen_state
                .fullscreen_active_node
                .get(monitor.as_str())
                .copied()
        })
        .is_some_and(|fullscreen_id| {
            fullscreen_id != node_id
                && overlap_policy_stack_rank(st, node_id).0
                    <= overlap_policy_stack_rank(st, fullscreen_id).0
        });
    if !needs_raise_above_fullscreen && active_surface_is_frontmost_on_monitor(st, node_id) {
        return false;
    }
    st.model.focus_state.next_overlap_raise_order = st
        .model
        .focus_state
        .next_overlap_raise_order
        .saturating_add(1);
    let order = st.model.focus_state.next_overlap_raise_order;
    st.model
        .focus_state
        .overlap_raise_order
        .insert(node_id, order);
    let should_animate_raise = match st.runtime.tuning.raise_animation_trigger() {
        halley_config::RaiseAnimationTrigger::Always => true,
        halley_config::RaiseAnimationTrigger::Overlap => {
            active_surface_overlaps_visible_peer_on_monitor(st, node_id)
        }
    };
    if st.runtime.tuning.raise_animation_enabled() && should_animate_raise {
        let now = Instant::now();
        st.request_window_animation_prewarm(node_id, now);
        let duration_ms = st.runtime.tuning.raise_animation_duration_ms();
        let scale = st.runtime.tuning.raise_animation_scale();
        let shadow_boost = st.runtime.tuning.raise_animation_shadow_boost();
        st.ui
            .render_state
            .start_raise_animation(node_id, now, duration_ms, scale, shadow_boost);
    }
    st.request_maintenance();
    true
}

pub fn recent_top_node_active(st: &mut Halley, now: Instant) -> Option<NodeId> {
    if st
        .model
        .focus_state
        .recent_top_until
        .is_some_and(|until| now >= until)
    {
        st.model.focus_state.recent_top_node = None;
        st.model.focus_state.recent_top_until = None;
        return None;
    }
    st.model.focus_state.recent_top_node
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn raising_active_window_starts_raise_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let monitor = state.model.monitor_state.current_monitor.clone();
        let node = state.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        let peer = state.model.field.spawn_surface(
            "peer",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        for node in [node, peer] {
            state.assign_node_to_monitor(node, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(node, halley_core::field::NodeState::Active);
        }

        assert!(state.raise_overlap_policy_node(node));

        assert!(
            state
                .ui
                .render_state
                .window_animations
                .raise_animations
                .contains_key(&node)
        );
    }

    #[test]
    fn raising_non_overlapping_active_window_skips_raise_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        state.runtime.tuning.animations.raise.trigger =
            halley_config::RaiseAnimationTrigger::Overlap;
        let monitor = state.model.monitor_state.current_monitor.clone();
        let node = state.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: -300.0, y: 0.0 },
            halley_core::field::Vec2 { x: 200.0, y: 200.0 },
        );
        let peer = state.model.field.spawn_surface(
            "peer",
            halley_core::field::Vec2 { x: 300.0, y: 0.0 },
            halley_core::field::Vec2 { x: 200.0, y: 200.0 },
        );
        for node in [node, peer] {
            state.assign_node_to_monitor(node, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(node, halley_core::field::NodeState::Active);
        }

        assert!(state.raise_overlap_policy_node(node));
        assert_eq!(state.model.focus_state.next_overlap_raise_order, 1);
        assert!(
            !state
                .ui
                .render_state
                .window_animations
                .raise_animations
                .contains_key(&node)
        );
    }

    #[test]
    fn raising_frontmost_window_does_not_restart_raise_animation() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let monitor = state.model.monitor_state.current_monitor.clone();
        let back = state.model.field.spawn_surface(
            "back",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        let front = state.model.field.spawn_surface(
            "front",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        for node in [back, front] {
            state.assign_node_to_monitor(node, monitor.as_str());
            let _ = state
                .model
                .field
                .set_state(node, halley_core::field::NodeState::Active);
        }

        assert!(!state.raise_overlap_policy_node(front));
        assert!(
            !state
                .ui
                .render_state
                .window_animations
                .raise_animations
                .contains_key(&front)
        );

        assert!(state.raise_overlap_policy_node(back));
        let order_after_raise = state.model.focus_state.next_overlap_raise_order;
        let started_at = state
            .ui
            .render_state
            .window_animations
            .raise_animations
            .get(&back)
            .expect("raise animation")
            .started_at;

        assert!(!state.raise_overlap_policy_node(back));
        assert_eq!(
            state.model.focus_state.next_overlap_raise_order,
            order_after_raise
        );
        assert_eq!(
            state
                .ui
                .render_state
                .window_animations
                .raise_animations
                .get(&back)
                .expect("raise animation")
                .started_at,
            started_at
        );
    }

    #[test]
    fn window_resize_does_not_show_focus_ring_preview() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let node = state.model.field.spawn_surface(
            "window",
            halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            halley_core::field::Vec2 { x: 320.0, y: 200.0 },
        );
        state.input.interaction_state.resize_active = Some(node);

        assert!(!state.should_draw_focus_ring_preview(state.runtime.started_at));
    }

    #[test]
    fn focus_ring_config_resize_preview_follows_debug_config() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let mut next = state.runtime.tuning.clone();
        next.focus_ring_rx += 80.0;
        next.debug.show_ring_when_resizing = true;

        state.apply_tuning(next);

        assert!(state.should_draw_focus_ring_preview(Instant::now()));

        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let mut next = state.runtime.tuning.clone();
        next.focus_ring_rx += 80.0;
        next.debug.show_ring_when_resizing = false;

        state.apply_tuning(next);

        assert!(!state.should_draw_focus_ring_preview(Instant::now()));
    }

    #[test]
    fn disabling_focus_ring_resize_preview_clears_active_preview() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let mut next = state.runtime.tuning.clone();
        next.focus_ring_rx += 80.0;
        next.debug.show_ring_when_resizing = true;
        state.apply_tuning(next);
        assert!(state.should_draw_focus_ring_preview(Instant::now()));

        let mut next = state.runtime.tuning.clone();
        next.debug.show_ring_when_resizing = false;
        state.apply_tuning(next);

        assert!(!state.should_draw_focus_ring_preview(Instant::now()));
    }
}
