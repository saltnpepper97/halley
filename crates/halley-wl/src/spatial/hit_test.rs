use std::time::Instant;

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::Resource;

use crate::interaction::types::HitNode;
use crate::runtime_render::{
    active_surface_render_scale, node_marker_bounds, node_marker_metrics, world_to_screen,
};
use crate::state::HalleyWlState;
use halley_core::viewport::FocusZone;

pub(crate) fn pick_hit_node_at(
    st: &HalleyWlState,
    w: i32,
    h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
) -> Option<HitNode> {
    let mut active: Vec<HitNode> = Vec::new();
    let mut node_dot: Vec<HitNode> = Vec::new();

    for (&id, n) in st.field.nodes() {
        if !st.field.is_visible(id) {
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
                let transition_alpha = st.active_transition_alpha(id, now);
                let s = active_surface_render_scale(
                    anim.scale,
                    st.active_zoom_lock_scale(),
                    n.intrinsic_size.x,
                    n.intrinsic_size.y,
                    transition_alpha,
                );
                let mut bbox_w = n.intrinsic_size.x;
                let mut bbox_h = n.intrinsic_size.y;
                for top in st.xdg_shell_state.toplevel_surfaces() {
                    let wl = top.wl_surface();
                    let key = wl.id();
                    if st.surface_to_node.get(&key).copied() != Some(id) {
                        continue;
                    }
                    let bbox = bbox_from_surface_tree(&wl, (0, 0));
                    bbox_w = bbox.size.w.max(1) as f32;
                    bbox_h = bbox.size.h.max(1) as f32;
                    break;
                }
                let rw = bbox_w * s;
                let rh = bbox_h * s;
                let p = st.smoothed_render_pos_read(id, n.pos, now);
                let (cx, cy) = world_to_screen(st, w, h, p.x, p.y);
                let sw = rw.round();
                let sh = rh.round();
                let x = ((cx as f32) - sw * 0.5).round() as i32;
                let y = ((cy as f32) - sh * 0.5).round() as i32;
                let ww = sw.max(1.0) as i32;
                let hh = sh.max(1.0) as i32;
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
            }
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core => {
                let (cx, cy) = world_to_screen(st, w, h, n.pos.x, n.pos.y);
                let (dot_half, label_gap, label_w, label_h) =
                    node_marker_metrics(st, n.label.len(), anim.scale);
                let (bx, by, bw, bh) =
                    node_marker_bounds(cx, cy, dot_half, label_gap, label_w, label_h, 6);
                let hit_all = sx >= bx as f32
                    && sx <= (bx + bw) as f32
                    && sy >= by as f32
                    && sy <= (by + bh) as f32;
                if hit_all {
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

pub(crate) fn node_in_active_area(st: &HalleyWlState, node_id: halley_core::field::NodeId) -> bool {
    let Some(n) = st.field.node(node_id) else {
        return false;
    };
    let focus_ring = st.active_focus_ring();
    matches!(
        focus_ring.zone(st.viewport.center, n.pos),
        FocusZone::Inside
    )
}
