use std::time::Instant;

use eventline::info;
use smithay::desktop::{WindowSurfaceType, utils::under_from_surface_tree};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point};

use crate::spatial::pick_hit_node_at;
use crate::state::HalleyWlState;

use super::pointer_map_debug_enabled;
use super::resize_helpers::active_node_surface_transform_screen_details;

pub(crate) fn pointer_focus_for_screen(
    st: &mut HalleyWlState,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
)> {
    let hit = pick_hit_node_at(st, ws_w, ws_h, sx, sy, now)?;
    let node = st.field.node(hit.node_id)?;
    if node.state != halley_core::field::NodeState::Active {
        return None;
    }

    let xform = active_node_surface_transform_screen_details(st, ws_w, ws_h, hit.node_id, now)?;
    let scale = xform.scale.max(0.001);
    if pointer_map_debug_enabled() {
        info!(
            "ptr-map focus-hit node={} ws={}x{} screen=({:.2},{:.2}) xform origin=({:.2},{:.2}) bbox_off=({:.2},{:.2}) scale={:.4}",
            hit.node_id.as_u64(),
            ws_w,
            ws_h,
            sx,
            sy,
            xform.origin_x,
            xform.origin_y,
            xform.bbox_offset_x,
            xform.bbox_offset_y,
            scale
        );
    }
    // Try both transform anchors:
    // 1) tree-root origin
    // 2) bbox origin
    // Some clients/toolkits expose non-zero bbox offsets and differ in where
    // input regions are anchored; probing both avoids scale-dependent drift.
    let local_candidates = [
        Point::<f64, Logical>::from((
            ((sx - xform.origin_x) / scale) as f64,
            ((sy - xform.origin_y) / scale) as f64,
        )),
        Point::<f64, Logical>::from((
            ((sx - (xform.origin_x + xform.bbox_offset_x)) / scale) as f64,
            ((sy - (xform.origin_y + xform.bbox_offset_y)) / scale) as f64,
        )),
    ];

    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface().clone();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(hit.node_id) {
            continue;
        }
        for local_point in local_candidates {
            if let Some((surface, surface_loc)) =
                under_from_surface_tree(&wl, local_point, (0, 0), WindowSurfaceType::ALL)
            {
                if pointer_map_debug_enabled() {
                    info!(
                        "ptr-map focus-local node={} local=({:.2},{:.2}) surface_off=({:.2},{:.2})",
                        hit.node_id.as_u64(),
                        local_point.x,
                        local_point.y,
                        surface_loc.x as f64,
                        surface_loc.y as f64
                    );
                }
                return Some((surface, local_point - surface_loc.to_f64()));
            }
        }
    }

    if pointer_map_debug_enabled() {
        info!(
            "ptr-map focus-miss node={} ws={}x{} screen=({:.2},{:.2})",
            hit.node_id.as_u64(),
            ws_w,
            ws_h,
            sx,
            sy
        );
    }
    None
}
