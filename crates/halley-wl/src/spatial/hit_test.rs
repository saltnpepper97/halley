use std::time::Instant;

use crate::input::active_node_screen_rect;
use crate::interaction::types::HitNode;
use crate::interaction::types::ResizeCtx;
use crate::render::{node_marker_metrics, world_to_screen};
use crate::state::Halley;
use halley_core::viewport::FocusZone;

pub(crate) fn pick_hit_node_at(
    st: &Halley,
    w: i32,
    h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<HitNode> {
    let mut active: Vec<HitNode> = Vec::new();
    let mut node_dot: Vec<HitNode> = Vec::new();

    for (&id, n) in st.field.nodes() {
        if !st.field.is_visible(id) || !st.node_visible_on_current_monitor(id) {
            continue;
        }
        if !matches!(
            n.state,
            halley_core::field::NodeState::Active
                | halley_core::field::NodeState::Node
                | halley_core::field::NodeState::Core
        ) {
            continue;
        }
        let anim = st.anim_style_for(id, n.state.clone(), now);
        let hit = match n.state {
            halley_core::field::NodeState::Active => {
                if let Some((left, top, right, bottom)) =
                    active_node_screen_rect(st, w, h, id, now, resize_preview)
                {
                    let x = left.round() as i32;
                    let y = top.round() as i32;
                    let ww = (right - left).max(1.0).round() as i32;
                    let hh = (bottom - top).max(1.0).round() as i32;
                    if sx >= x as f32
                        && sx <= (x + ww) as f32
                        && sy >= y as f32
                        && sy <= (y + hh) as f32
                    {
                        let title_h = ((hh as f32) * 0.20).round().clamp(28.0, 56.0) as i32;
                        Some(HitNode {
                            node_id: id,
                            on_titlebar: sy <= (y + title_h) as f32,
                            is_core: false,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                let (cx, cy) = world_to_screen(st, w, h, n.pos.x, n.pos.y);
                let (dot_half, _, _, _) = node_marker_metrics(st, n.label.len(), anim.scale);
                let radius = (dot_half + 10).max(1) as f32;
                let dx = sx - cx as f32;
                let dy = sy - cy as f32;
                if dx * dx + dy * dy <= radius * radius {
                    Some(HitNode {
                        node_id: id,
                        on_titlebar: false,
                        is_core: n.state == halley_core::field::NodeState::Core,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(hit) = hit else { continue };
        match n.state {
            halley_core::field::NodeState::Active => active.push(hit),
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                node_dot.push(hit)
            }
            _ => {}
        };
    }

    active.sort_by_key(|h| std::cmp::Reverse(h.node_id.as_u64()));
    node_dot.sort_by_key(|h| std::cmp::Reverse(h.node_id.as_u64()));

    active
        .into_iter()
        .next()
        .or_else(|| node_dot.into_iter().next())
}

pub(crate) fn node_in_active_area(st: &Halley, node_id: halley_core::field::NodeId) -> bool {
    node_in_active_area_for_monitor(st, node_id, st.monitor_state.current_monitor.as_str())
}

pub(crate) fn node_in_active_area_for_monitor(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    monitor: &str,
) -> bool {
    let Some(n) = st.field.node(node_id) else {
        return false;
    };
    if st
        .monitor_state
        .node_monitor
        .get(&node_id)
        .is_some_and(|owner| owner != monitor)
    {
        return false;
    }
    let focus_ring = st.focus_ring_for_monitor(monitor);
    let focus_center = st.view_center_for_monitor(monitor);
    matches!(focus_ring.zone(focus_center, n.pos), FocusZone::Inside)
}
