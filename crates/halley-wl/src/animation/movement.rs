use std::time::Instant;

use crate::compositor::interaction::{NodeMoveAnim, PointerState};
use crate::compositor::root::Halley;

use super::curves::ease_in_out_cubic;

pub(crate) fn advance_node_move_anim(
    st: &mut Halley,
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
        let _ = st.carry_surface_non_overlap(anim.node_id, pos, false);
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
