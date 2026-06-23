use std::time::Instant;

use smithay::desktop::{
    PopupKind, PopupManager, WindowSurfaceType,
    utils::{bbox_from_surface_tree, under_from_surface_tree},
};
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::{Logical, Point};
use smithay::wayland::{compositor::get_role, shell::xdg::XDG_POPUP_ROLE};

use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::compositor::spawn::state::is_persistent_rule_top;
use crate::spatial::{hit_nodes_at, pick_hit_node_at};

use super::super::resize::active_node_surface_transform_screen_details;

fn clamp_layer_popup_origin(
    popup: &smithay::desktop::PopupKind,
    popup_origin: (i32, i32),
    output_size: (i32, i32),
) -> (i32, i32) {
    let bbox = bbox_from_surface_tree(popup.wl_surface(), (0, 0));
    let mut origin_x = popup_origin.0;
    let mut origin_y = popup_origin.1;

    let left = origin_x + bbox.loc.x;
    let right = left + bbox.size.w;
    if left < 0 {
        origin_x -= left;
    } else if right > output_size.0 {
        origin_x -= right - output_size.0;
    }

    let top = origin_y + bbox.loc.y;
    let bottom = top + bbox.size.h;
    if top < 0 {
        origin_y -= top;
    } else if bottom > output_size.1 {
        origin_y -= bottom - output_size.1;
    }

    (origin_x, origin_y)
}

fn popup_parent_surface(popup: &PopupKind) -> Option<WlSurface> {
    match popup {
        PopupKind::Xdg(popup) => popup.get_parent_surface(),
        PopupKind::InputMethod(popup) => popup.get_parent().map(|parent| parent.surface.clone()),
    }
}

fn popup_depth(st: &Halley, popup: &PopupKind) -> usize {
    let mut depth = 0;
    let mut parent = popup_parent_surface(popup);
    while let Some(surface) = parent {
        if get_role(&surface) != Some(XDG_POPUP_ROLE) {
            break;
        }
        depth += 1;
        parent = st
            .platform
            .popup_manager
            .find_popup(&surface)
            .and_then(|popup| popup_parent_surface(&popup));
    }
    depth
}

fn popups_top_to_bottom(st: &Halley, surface: &WlSurface) -> Vec<(PopupKind, Point<i32, Logical>)> {
    let mut popups: Vec<_> = PopupManager::popups_for_surface(surface)
        .enumerate()
        .map(|(index, (popup, offset))| (popup_depth(st, &popup), index, popup, offset))
        .collect();
    popups.sort_by_key(|(depth, index, _, _)| (*depth, *index));
    popups
        .into_iter()
        .rev()
        .map(|(_, _, popup, offset)| (popup, offset))
        .collect()
}

pub(crate) fn popup_focus_for_screen(
    st: &mut Halley,
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
    let mut toplevels: Vec<_> = st
        .platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|top| {
            let wl = top.wl_surface().clone();
            let node_id = st.model.surface_to_node.get(&wl.id()).copied()?;
            let node = st.model.field.node(node_id)?;
            (node.state == halley_core::field::NodeState::Active
                && st.model.field.is_visible(node_id))
            .then_some((node_id, top, wl, node.intrinsic_size))
        })
        .collect();

    toplevels.sort_by_key(|(node_id, _, _, _)| {
        (
            is_persistent_rule_top(st, *node_id),
            st.overlap_policy_stack_rank(*node_id),
            node_id.as_u64(),
        )
    });

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

        let parent_geo = st
            .ui
            .render_state
            .cache
            .window_geometry
            .get(&node_id)
            .map(|&(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
            .unwrap_or_else(|| {
                top.with_committed_state(|state| state.and_then(|state| state.size))
                    .map(|sz| (0.0, 0.0, sz.w.max(1) as f32, sz.h.max(1) as f32))
                    .unwrap_or((
                        0.0,
                        0.0,
                        intrinsic_size.x.max(1.0),
                        intrinsic_size.y.max(1.0),
                    ))
            });
        let parent_geo_loc = (parent_geo.0.round() as i32, parent_geo.1.round() as i32);

        for (popup, popup_offset) in popups_top_to_bottom(st, &wl) {
            let popup_geo = popup.geometry();
            let popup_sx = xform.origin_x
                + ((parent_geo_loc.0 + popup_offset.x - popup_geo.loc.x) as f32 * scale).round();
            let popup_sy = xform.origin_y
                + ((parent_geo_loc.1 + popup_offset.y - popup_geo.loc.y) as f32 * scale).round();
            let popup_local = Point::<f64, Logical>::from((
                ((sx - popup_sx) / scale) as f64,
                ((sy - popup_sy) / scale) as f64,
            ));
            let Some((surface, surface_loc)) = under_from_surface_tree(
                popup.wl_surface(),
                popup_local,
                (0, 0),
                WindowSurfaceType::ALL,
            ) else {
                continue;
            };
            let cam_scale_f = st.camera_render_scale() as f64;
            let focus_origin = Point::<f64, Logical>::from((
                popup_sx as f64 / cam_scale_f + surface_loc.x as f64,
                popup_sy as f64 / cam_scale_f + surface_loc.y as f64,
            ));

            return Some((surface, focus_origin));
        }
    }

    None
}

fn fullscreen_hit_blocks_non_overlay_layers(
    st: &mut Halley,
    ws_w: i32,
    ws_h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> bool {
    let Some(hit) = pick_hit_node_at(st, ws_w, ws_h, sx, sy, now, resize_preview) else {
        return false;
    };
    if !st.is_fullscreen_active(hit.node_id) {
        return false;
    }

    let pointer_monitor = st.monitor_for_screen_or_current(sx, sy);
    let node_monitor = st
        .fullscreen_monitor_for_node(hit.node_id)
        .map(str::to_owned)
        .unwrap_or_else(|| st.monitor_for_node_or_current(hit.node_id));

    pointer_monitor == node_monitor
}

pub(crate) fn layer_surface_focus_for_screen(
    st: &mut Halley,
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
    let block_non_overlay =
        fullscreen_hit_blocks_non_overlay_layers(st, ws_w, ws_h, sx, sy, now, resize_preview);

    let mut placements = crate::compositor::monitor::layer_shell::layer_shell_placements(
        st,
        (ws_w.max(1), ws_h.max(1)).into(),
    );
    placements.sort_by_key(|placement| {
        std::cmp::Reverse(match placement.layer {
            smithay::wayland::shell::wlr_layer::Layer::Background => 0u8,
            smithay::wayland::shell::wlr_layer::Layer::Bottom => 1u8,
            smithay::wayland::shell::wlr_layer::Layer::Top => 2u8,
            smithay::wayland::shell::wlr_layer::Layer::Overlay => 3u8,
        })
    });

    for placement in placements {
        if block_non_overlay
            && !matches!(
                placement.layer,
                smithay::wayland::shell::wlr_layer::Layer::Top
                    | smithay::wayland::shell::wlr_layer::Layer::Overlay
            )
        {
            continue;
        }

        for (popup, popup_offset) in popups_top_to_bottom(st, &placement.wl_surface) {
            let popup_geo = popup.geometry();
            let (popup_origin_x, popup_origin_y) = clamp_layer_popup_origin(
                &popup,
                (
                    placement.origin.x + popup_offset.x - popup_geo.loc.x,
                    placement.origin.y + popup_offset.y - popup_geo.loc.y,
                ),
                (ws_w, ws_h),
            );
            let popup_local = Point::<f64, Logical>::from((
                (sx.round() as i32 - popup_origin_x) as f64,
                (sy.round() as i32 - popup_origin_y) as f64,
            ));
            let Some((surface, surface_loc)) = under_from_surface_tree(
                popup.wl_surface(),
                popup_local,
                (0, 0),
                WindowSurfaceType::ALL,
            ) else {
                continue;
            };
            let focus_origin = Point::<f64, Logical>::from((
                (popup_origin_x + surface_loc.x) as f64,
                (popup_origin_y + surface_loc.y) as f64,
            ));
            return Some((surface, focus_origin));
        }

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

pub(crate) fn grabbed_layer_surface_focus(
    st: &mut Halley,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
)> {
    let root =
        crate::compositor::monitor::layer_shell::layer_surface_root_for_surface(st, surface)?;
    let monitor = crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, &root);
    let placement = crate::compositor::monitor::layer_shell::layer_shell_placements_for_monitor(
        st,
        monitor.as_str(),
    )
    .into_iter()
    .find(|placement| placement.wl_surface == root)?;
    Some((
        surface.clone(),
        Point::<f64, Logical>::from((placement.origin.x as f64, placement.origin.y as f64)),
    ))
}

pub(crate) fn pointer_focus_for_screen(
    st: &mut Halley,
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
    if let Some(focus) = crate::protocol::wayland::session_lock::focus_for_screen(st, sx, sy) {
        return Some(focus);
    }
    if let Some(focus) = layer_surface_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview)
    {
        return Some(focus);
    }
    if let Some(focus) = popup_focus_for_screen(st, ws_w, ws_h, sx, sy, now, resize_preview) {
        return Some(focus);
    }

    let hits = hit_nodes_at(st, ws_w, ws_h, sx, sy, now, resize_preview);
    for hit in hits {
        let Some(xform) = active_node_surface_transform_screen_details(
            st,
            ws_w,
            ws_h,
            hit.node_id,
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

        for top in st.platform.xdg_shell_state.toplevel_surfaces() {
            let wl = top.wl_surface().clone();
            let key = wl.id();
            if st.model.surface_to_node.get(&key).copied() != Some(hit.node_id) {
                continue;
            }

            let Some((surface, surface_loc)) =
                under_from_surface_tree(&wl, local, (0, 0), WindowSurfaceType::ALL)
            else {
                continue;
            };

            let cam_scale_f = st.camera_render_scale() as f64;
            let focus_origin = Point::<f64, Logical>::from((
                xform.origin_x as f64 / cam_scale_f + surface_loc.x as f64,
                xform.origin_y as f64 / cam_scale_f + surface_loc.y as f64,
            ));

            return Some((surface, focus_origin));
        }

        if resize_preview.is_some_and(|rz| rz.node_id == hit.node_id) {
            for top in st.platform.xdg_shell_state.toplevel_surfaces() {
                let wl = top.wl_surface().clone();
                let key = wl.id();
                if st.model.surface_to_node.get(&key).copied() != Some(hit.node_id) {
                    continue;
                }

                let cam_scale_f = st.camera_render_scale() as f64;
                let focus_origin = Point::<f64, Logical>::from((
                    xform.origin_x as f64 / cam_scale_f,
                    xform.origin_y as f64 / cam_scale_f,
                ));

                return Some((wl, focus_origin));
            }
        }
    }
    None
}
