use std::time::Instant;

use eventline::info;
use smithay::desktop::{utils::under_from_surface_tree, PopupManager, WindowSurfaceType};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point};

use crate::interaction::types::ResizeCtx;
use crate::spatial::pick_hit_node_at;
use crate::state::HalleyWlState;

use super::pointer_map_debug_enabled;
use super::resize_helpers::active_node_surface_transform_screen_details;

fn popup_focus_for_screen(
    st: &mut HalleyWlState,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
)> {
    let recent_top_node = st.recent_top_node_active(now);
    let mut toplevels: Vec<_> = st
        .xdg_shell_state
        .toplevel_surfaces()
        .into_iter()
        .filter_map(|top| {
            let wl = top.wl_surface().clone();
            let node_id = st.surface_to_node.get(&wl.id()).copied()?;
            let node = st.field.node(node_id)?;
            (node.state == halley_core::field::NodeState::Active && st.field.is_visible(node_id))
                .then_some((node_id, top, wl, node.intrinsic_size))
        })
        .collect();

    toplevels.sort_by_key(|(node_id, _, _, _)| node_id.as_u64());
    if let Some(idx) = toplevels
        .iter()
        .position(|(node_id, _, _, _)| Some(*node_id) == recent_top_node)
    {
        let top = toplevels.remove(idx);
        toplevels.push(top);
    }

    for (node_id, top, wl, intrinsic_size) in toplevels.into_iter().rev() {
        let Some(xform) = active_node_surface_transform_screen_details(
            st,
            ws_w,
            ws_h,
            node_id,
            now,
            resize_preview,
        ) else {
            continue;
        };
        let scale = xform.scale.max(0.001);
        let local = Point::<f64, Logical>::from((
            ((sx - xform.origin_x) / scale) as f64,
            ((sy - xform.origin_y) / scale) as f64,
        ));

        let parent_geo = st
            .window_geometry
            .get(&node_id)
            .map(|&(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
            .unwrap_or_else(|| {
                top.current_state()
                    .size
                    .map(|sz| (0.0, 0.0, sz.w.max(1) as f32, sz.h.max(1) as f32))
                    .unwrap_or((
                        0.0,
                        0.0,
                        intrinsic_size.x.max(1.0),
                        intrinsic_size.y.max(1.0),
                    ))
            });
        let parent_geo_loc = (parent_geo.0.round() as i32, parent_geo.1.round() as i32);

        let mut popups: Vec<_> = PopupManager::popups_for_surface(&wl).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let offset = (
                parent_geo_loc.0 + popup_offset.x - popup_geo.loc.x,
                parent_geo_loc.1 + popup_offset.y - popup_geo.loc.y,
            );
            let Some((surface, surface_loc)) =
                under_from_surface_tree(popup.wl_surface(), local, offset, WindowSurfaceType::ALL)
            else {
                continue;
            };
            let focus_origin = Point::<f64, Logical>::from((
                (xform.origin_x + surface_loc.x as f32 * scale) as f64,
                (xform.origin_y + surface_loc.y as f32 * scale) as f64,
            ));

            if pointer_map_debug_enabled() {
                info!(
                    "ptr-map focus-popup node={} popup_loc=({:.2},{:.2}) focus_origin=({:.2},{:.2})",
                    node_id.as_u64(),
                    surface_loc.x as f64,
                    surface_loc.y as f64,
                    focus_origin.x,
                    focus_origin.y,
                );
            }

            return Some((surface, focus_origin));
        }
    }

    None
}

pub(crate) fn layer_surface_focus_for_screen(
    st: &HalleyWlState,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
)> {
    let mut placements = st.layer_shell_placements((ws_w.max(1), ws_h.max(1)).into());
    placements.sort_by_key(|placement| {
        std::cmp::Reverse(match placement.layer {
            smithay::wayland::shell::wlr_layer::Layer::Background => 0u8,
            smithay::wayland::shell::wlr_layer::Layer::Bottom => 1u8,
            smithay::wayland::shell::wlr_layer::Layer::Top => 2u8,
            smithay::wayland::shell::wlr_layer::Layer::Overlay => 3u8,
        })
    });

    for placement in placements {
        let local = Point::<f64, Logical>::from((
            (sx.round() as i32 - placement.origin.x) as f64,
            (sy.round() as i32 - placement.origin.y) as f64,
        ));
        let Some((surface, surface_loc)) =
            under_from_surface_tree(&placement.wl_surface, local, (0, 0), WindowSurfaceType::ALL)
        else {
            continue;
        };
        let focus_origin = Point::<f64, Logical>::from((
            (placement.origin.x + surface_loc.x) as f64,
            (placement.origin.y + surface_loc.y) as f64,
        ));
        return Some((surface, focus_origin));
    }

    None
}

/// Resolve the Wayland surface and compositor-space surface origin for a
/// given screen-space pointer position.
///
/// `resize_preview` must be the same value passed to the current render frame
/// so that during interactive resize the transform mirrors the render path
/// (preview edges, scale=1.0) rather than the smoothed-position path.
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
pub(crate) fn pointer_focus_for_screen(
    st: &mut HalleyWlState,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
)> {
    if let Some(focus) = layer_surface_focus_for_screen(st, ws_w, ws_h, sx, sy) {
        return Some(focus);
    }
    if let Some(focus) = popup_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview) {
        return Some(focus);
    }

    let hit = pick_hit_node_at(st, ws_w, ws_h, sx, sy, now, resize_preview)?;

    let xform = active_node_surface_transform_screen_details(
        st,
        ws_w,
        ws_h,
        hit.node_id,
        now,
        resize_preview,
    )?;
    let scale = xform.scale.max(0.001);

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

    if resize_preview.is_some_and(|rz| rz.node_id == hit.node_id) {
        for top in st.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface().clone();
            let key = wl.id();
            if st.surface_to_node.get(&key).copied() != Some(hit.node_id) {
                continue;
            }

            let focus_origin =
                Point::<f64, Logical>::from((xform.origin_x as f64, xform.origin_y as f64));

            if pointer_map_debug_enabled() {
                info!(
                    "ptr-map focus-resize-fallback node={} focus_origin=({:.2},{:.2})",
                    hit.node_id.as_u64(),
                    focus_origin.x,
                    focus_origin.y,
                );
            }

            return Some((wl, focus_origin));
        }
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
