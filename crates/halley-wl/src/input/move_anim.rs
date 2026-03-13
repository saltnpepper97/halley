use std::time::{Duration, Instant};

use crate::interaction::types::{NodeMoveAnim, PointerState};
use crate::render::ease_in_out_cubic;
use crate::state::HalleyWlState;

pub(crate) fn advance_node_move_anim(
    st: &mut HalleyWlState,
    ps: &mut PointerState,
    now: Instant,
) -> Option<halley_core::field::NodeId> {
    if ps.move_anim.is_empty() {
        return None;
    }
    let anims: Vec<NodeMoveAnim> = ps.move_anim.values().copied().collect();
    let mut finished: Vec<halley_core::field::NodeId> = Vec::new();
    let mut last_id: Option<halley_core::field::NodeId> = None;
    let now_ms = st.now_ms(now);
    for anim in anims {
        if st.is_recently_resized_node(anim.node_id, now_ms) {
            finished.push(anim.node_id);
            continue;
        }
        let elapsed = now.saturating_duration_since(anim.started_at);
        let dur_s = anim.duration.as_secs_f32().max(0.000_1);
        let t = (elapsed.as_secs_f32() / dur_s).clamp(0.0, 1.0);
        let e = ease_in_out_cubic(t);
        let pos = halley_core::field::Vec2 {
            x: anim.from.x + (anim.to.x - anim.from.x) * e,
            y: anim.from.y + (anim.to.y - anim.from.y) * e,
        };
        if st.tuning.physics_enabled {
            let _ = st.carry_surface_non_overlap(anim.node_id, pos);
        } else {
            let _ = st.field.carry(anim.node_id, pos);
        }
        if t >= 1.0 {
            finished.push(anim.node_id);
        }
        last_id = Some(anim.node_id);
    }
    for id in finished {
        ps.move_anim.remove(&id);
    }
    last_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;

    #[test]
    fn move_anim_uses_constrained_carry_when_physics_is_enabled() {
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
                .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let b =
            state
                .field
                .spawn_surface("b", Vec2 { x: 430.0, y: 0.0 }, Vec2 { x: 400.0, y: 300.0 });
        let mut ps = PointerState::default();
        let now = Instant::now();
        ps.move_anim.insert(
            a,
            NodeMoveAnim {
                node_id: a,
                from: Vec2 { x: 0.0, y: 0.0 },
                to: Vec2 { x: 280.0, y: 0.0 },
                started_at: now,
                duration: Duration::from_millis(100),
            },
        );

        let _ = advance_node_move_anim(&mut state, &mut ps, now + Duration::from_millis(100));

        let na = state.field.node(a).expect("animated window");
        let nb = state.field.node(b).expect("neighbor window");
        let ea = state.collision_extents_for_node(na);
        let eb = state.collision_extents_for_node(nb);
        let gap = state.non_overlap_gap_world();
        let dx = (nb.pos.x - na.pos.x).abs();
        let dy = (nb.pos.y - na.pos.y).abs();
        let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
        let req_y = state.required_sep_y(na.pos.y, ea, nb.pos.y, eb, gap);

        assert!(
            dx >= req_x - 0.5 || dy >= req_y - 0.5,
            "move animation should preserve non-overlap without a post-fix resolver: dx={dx}, req_x={req_x}, dy={dy}, req_y={req_y}"
        );
        assert!(
            ps.move_anim.is_empty(),
            "expected finished move animation to clear"
        );
    }
}
