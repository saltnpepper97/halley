use super::*;
use halley_core::viewport::{FocusRing, FocusZone};

#[inline]
pub(crate) fn set_drag_authority_node(st: &mut Halley, id: Option<NodeId>) {
    st.input.interaction_state.drag_authority_node = id;
    if id.is_none() {
        st.input.interaction_state.drag_authority_velocity = Vec2 { x: 0.0, y: 0.0 };
        st.input.interaction_state.active_drag = None;
        crate::compositor::interaction::state::clear_grabbed_edge_pan_state(st);
    }
}

#[inline]
pub(crate) fn mark_direct_carry_node(st: &mut Halley, id: NodeId) {
    st.model.carry_state.carry_direct_nodes.insert(id);
}

#[inline]
pub(crate) fn clear_direct_carry_nodes(st: &mut Halley) {
    st.model.carry_state.carry_direct_nodes.clear();
}

#[inline]
fn zone_eval_footprint_for(st: &Halley, id: NodeId, fallback: Vec2) -> Vec2 {
    if st
        .model
        .field
        .node(id)
        .is_some_and(|n| n.state == halley_core::field::NodeState::Active)
    {
        Vec2 { x: 64.0, y: 64.0 }
    } else {
        fallback
    }
}

fn focus_ring_coverage_fractions(
    st: &Halley,
    pos: Vec2,
    footprint: Vec2,
    focus_ring: FocusRing,
) -> (f32, f32) {
    let sample_fp = Vec2 {
        x: footprint.x.max(48.0),
        y: footprint.y.max(48.0),
    };
    let samples = 7usize;
    let mut c_inside = 0usize;
    let mut c_total = 0usize;
    for ix in 0..samples {
        for iy in 0..samples {
            let fx = (ix as f32 / (samples - 1) as f32) - 0.5;
            let fy = (iy as f32 / (samples - 1) as f32) - 0.5;
            let sp = Vec2 {
                x: pos.x + fx * sample_fp.x,
                y: pos.y + fy * sample_fp.y,
            };
            match focus_ring.zone(
                st.view_center_for_monitor(st.model.monitor_state.current_monitor.as_str()),
                sp,
            ) {
                FocusZone::Inside => c_inside += 1,
                FocusZone::Outside => {}
            }
            c_total += 1;
        }
    }
    if c_total == 0 {
        return (0.0, 1.0);
    }
    let p_inside = c_inside as f32 / c_total as f32;
    let p_outside = (1.0 - p_inside).max(0.0);
    (p_inside, p_outside)
}

fn zone_for_pos_with_hysteresis(
    st: &mut Halley,
    id: NodeId,
    pos: Vec2,
    footprint: Vec2,
) -> FocusZone {
    let focus_ring = st.active_focus_ring();
    let footprint = zone_eval_footprint_for(st, id, footprint);
    let (p_inside, p_outside) = focus_ring_coverage_fractions(st, pos, footprint, focus_ring);
    let prev = st.model.carry_state.carry_zone_hint.get(&id).copied();

    const ACTIVE_RETAIN_FRAC: f32 = 0.04;
    const ACTIVE_ENTER_FRAC: f32 = 0.10;
    const OUTSIDE_ENTER_FRAC: f32 = 0.90;

    let zone = match prev {
        Some(FocusZone::Inside) => {
            if p_inside >= ACTIVE_RETAIN_FRAC {
                FocusZone::Inside
            } else if p_outside >= OUTSIDE_ENTER_FRAC {
                FocusZone::Outside
            } else {
                FocusZone::Inside
            }
        }
        _ => {
            if p_inside >= ACTIVE_ENTER_FRAC {
                FocusZone::Inside
            } else {
                FocusZone::Outside
            }
        }
    };

    let now_ms = st.now_ms(Instant::now());
    st.model
        .carry_state
        .carry_zone_last_change_ms
        .insert(id, now_ms);
    st.model.carry_state.carry_zone_pending.remove(&id);
    st.model.carry_state.carry_zone_pending_since_ms.remove(&id);
    st.model.carry_state.carry_zone_hint.insert(id, zone);
    zone
}

pub(crate) fn finalize_mouse_drag_state(
    st: &mut Halley,
    id: NodeId,
    _pointer_world: Vec2,
    _now: Instant,
) {
    let Some(n) = st.model.field.node(id) else {
        return;
    };
    if n.kind != halley_core::field::NodeKind::Surface || !st.model.field.is_visible(id) {
        return;
    }
    st.input.interaction_state.physics_velocity.remove(&id);
    st.input.interaction_state.drag_authority_velocity = Vec2 { x: 0.0, y: 0.0 };
}

pub(crate) fn begin_carry_state_tracking(st: &mut Halley, id: NodeId) {
    clear_direct_carry_nodes(st);
    mark_direct_carry_node(st, id);
    if st.input.interaction_state.resize_static_node == Some(id) {
        st.input.interaction_state.resize_static_node = None;
        st.input.interaction_state.resize_static_lock_pos = None;
        st.input.interaction_state.resize_static_until_ms = 0;
    }
    st.input.interaction_state.suspend_overlap_resolve = false;
    st.input.interaction_state.suspend_state_checks = false;
    let _ = st.model.field.set_pinned(id, false);

    if let Some(n) = st.model.field.node(id) {
        st.model
            .carry_state
            .carry_state_hold
            .insert(id, n.state.clone());
        let fp = st.collision_size_for_node(n);
        let z = zone_for_pos_with_hysteresis(st, id, n.pos, fp);
        st.model.carry_state.carry_zone_hint.insert(id, z);
        st.model
            .carry_state
            .carry_zone_last_change_ms
            .insert(id, st.now_ms(Instant::now()));
        st.model.carry_state.carry_zone_pending.remove(&id);
        st.model.carry_state.carry_zone_pending_since_ms.remove(&id);
        st.model.carry_state.carry_activation_anim_armed.insert(id);
    }
    st.request_maintenance();
}

pub(crate) fn end_carry_state_tracking(st: &mut Halley, id: NodeId) {
    if st.input.interaction_state.drag_authority_node == Some(id) {
        st.input.interaction_state.drag_authority_node = None;
    }
    mark_direct_carry_node(st, id);
    st.model.carry_state.carry_zone_hint.remove(&id);
    st.model.carry_state.carry_zone_last_change_ms.remove(&id);
    st.model.carry_state.carry_zone_pending.remove(&id);
    st.model.carry_state.carry_zone_pending_since_ms.remove(&id);
    st.model.carry_state.carry_activation_anim_armed.remove(&id);
    st.model.carry_state.carry_state_hold.remove(&id);
    st.input.interaction_state.suspend_overlap_resolve = false;
    st.input.interaction_state.suspend_state_checks = false;
    clear_direct_carry_nodes(st);
    st.request_maintenance();
}

pub(crate) fn update_carry_state_preview(st: &mut Halley, id: NodeId, now: Instant) {
    let Some(n) = st.model.field.node(id) else {
        return;
    };
    update_carry_state_preview_at(st, id, n.pos, now);
}

pub(crate) fn update_carry_state_preview_at(
    st: &mut Halley,
    id: NodeId,
    source_pos: Vec2,
    now: Instant,
) {
    let Some(n) = st.model.field.node(id) else {
        return;
    };
    let n_kind = n.kind.clone();
    let was_active = n.state == halley_core::field::NodeState::Active;
    let footprint = zone_eval_footprint_for(st, id, st.collision_size_for_node(n));
    if n_kind != halley_core::field::NodeKind::Surface || !st.model.field.is_visible(id) {
        return;
    }
    let zone = zone_for_pos_with_hysteresis(st, id, source_pos, footprint);
    let held_state = st.model.carry_state.carry_state_hold.get(&id);
    let target = match held_state {
        Some(halley_core::field::NodeState::Active) => DecayLevel::Hot,
        Some(halley_core::field::NodeState::Node | halley_core::field::NodeState::Core) => {
            DecayLevel::Cold
        }
        _ => match zone {
            FocusZone::Inside if was_active => DecayLevel::Hot,
            _ => DecayLevel::Cold,
        },
    };
    if matches!(target, DecayLevel::Cold) {
        crate::compositor::workspace::state::start_active_to_node_close_animation(st, id, now);
    }
    let _ = st.model.field.set_decay_level(id, target);
    let is_active = st
        .model
        .field
        .node(id)
        .is_some_and(|nn| nn.state == halley_core::field::NodeState::Active);
    if is_active {
        if let Some(nn) = st.model.field.node(id) {
            st.model
                .workspace_state
                .last_active_size
                .insert(id, nn.intrinsic_size);
        }
        if !was_active
            && st.active_transition_alpha(id, now) <= 0.01
            && st.model.carry_state.carry_activation_anim_armed.remove(&id)
        {
            st.mark_active_transition(id, now, 360);
        }
    }
    st.request_maintenance();
}
