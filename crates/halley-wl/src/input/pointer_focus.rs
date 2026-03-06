use std::time::Instant;

use eventline::info;
use smithay::desktop::{WindowSurfaceType, utils::under_from_surface_tree};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point};

use crate::spatial::pick_hit_node_at;
use crate::state::HalleyWlState;

use super::pointer_map_debug_enabled;
use super::resize_helpers::active_node_surface_transform_screen_details;

/// Resolve the Wayland surface and compositor-space surface origin for a
/// given screen-space pointer position.
///
/// # What Smithay expects for `focus.1`
///
/// `PointerHandle::motion` delivers the surface-local cursor position to the
/// client as:
///
/// ```text
/// surface_local = event.location − focus.1
/// ```
///
/// `event.location` is the raw screen-pixel coordinate `(sx, sy)`.  Therefore
/// `focus.1` **must** be the screen-space position of the found surface's
/// `(0, 0)` origin, not a pre-computed local cursor offset.
///
/// The previous code returned `local_point − surface_loc`, which at scale = 1
/// collapsed to the constant `(origin_x + surface_loc.x, origin_y +
/// surface_loc.y)` — so every click landed at the same spot regardless of
/// where the cursor actually was.
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

    // `xform.origin_x/y` is the screen-space position of the surface-tree root
    // (0, 0).  It already incorporates the scaled bbox offset so no second
    // adjustment is needed here.
    //
    // Convert screen → surface-tree-root-local so `under_from_surface_tree`
    // can locate the exact (sub)surface under the pointer.
    let local = Point::<f64, Logical>::from((
        ((sx - xform.origin_x) / scale) as f64,
        ((sy - xform.origin_y) / scale) as f64,
    ));

    if pointer_map_debug_enabled() {
        info!(
            "ptr-map focus-hit node={} ws={}x{} screen=({:.2},{:.2}) \
             xform origin=({:.2},{:.2}) scale={:.4} local=({:.2},{:.2})",
            hit.node_id.as_u64(),
            ws_w,
            ws_h,
            sx,
            sy,
            xform.origin_x,
            xform.origin_y,
            scale,
            local.x,
            local.y,
        );
    }

    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface().clone();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(hit.node_id) {
            continue;
        }

        let Some((surface, surface_loc)) =
            under_from_surface_tree(&wl, local, (0, 0), WindowSurfaceType::ALL)
        else {
            continue;
        };

        // `surface_loc` is the sub-surface's position relative to the
        // tree root in surface-local (logical) pixels.  Its screen-space
        // position is `origin + surface_loc * scale`.
        //
        // Smithay will deliver:
        //   client_local = event.location − focus.1
        //                = (sx, sy) − (origin_x + surface_loc.x * scale,
        //                                origin_y + surface_loc.y * scale)
        //
        // At scale = 1.0 this is exactly `(sx − origin_x − surface_loc.x,
        // sy − origin_y − surface_loc.y)` — the correct surface-local
        // cursor position.
        let focus_origin = Point::<f64, Logical>::from((
            (xform.origin_x + surface_loc.x as f32 * scale) as f64,
            (xform.origin_y + surface_loc.y as f32 * scale) as f64,
        ));

        if pointer_map_debug_enabled() {
            info!(
                "ptr-map focus-resolved node={} surface_loc=({:.2},{:.2}) \
                 focus_origin=({:.2},{:.2})",
                hit.node_id.as_u64(),
                surface_loc.x as f64,
                surface_loc.y as f64,
                focus_origin.x,
                focus_origin.y,
            );
        }

        return Some((surface, focus_origin));
    }

    if pointer_map_debug_enabled() {
        info!(
            "ptr-map focus-miss node={} ws={}x{} screen=({:.2},{:.2})",
            hit.node_id.as_u64(),
            ws_w,
            ws_h,
            sx,
            sy,
        );
    }
    None
}
