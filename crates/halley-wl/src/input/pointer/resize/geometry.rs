use std::time::Instant;

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::animation::active_surface_render_scale;
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::compositor::surface_ops::active_stacking_render_order_for_monitor;
use crate::frame_loop::anim_style_for;
use crate::presentation::world_to_screen;
use crate::window::active_window_frame_pad_px;

#[derive(Clone, Copy)]
pub(crate) struct ActiveNodeSurfaceTransformScreen {
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
    pub(crate) scale: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct ActiveResizeGeometryScreen {
    pub(crate) frame_left: f32,
    pub(crate) frame_top: f32,
    pub(crate) frame_right: f32,
    pub(crate) frame_bottom: f32,
    pub(crate) surface_origin_x: f32,
    pub(crate) surface_origin_y: f32,
    pub(crate) live_geo_w: f32,
    pub(crate) live_geo_h: f32,
}

impl ActiveResizeGeometryScreen {
    pub(crate) fn frame_rect_px(self) -> (i32, i32, i32, i32) {
        let left = self.frame_left.round() as i32;
        let top = self.frame_top.round() as i32;
        let right = self.frame_right.round() as i32;
        let bottom = self.frame_bottom.round() as i32;
        (left, top, (right - left).max(1), (bottom - top).max(1))
    }

    pub(crate) fn surface_origin_px(self) -> (i32, i32) {
        (
            self.surface_origin_x.round() as i32,
            self.surface_origin_y.round() as i32,
        )
    }

    pub(crate) fn center_px(self) -> (i32, i32) {
        (
            ((self.frame_left + self.frame_right) * 0.5).round() as i32,
            ((self.frame_top + self.frame_bottom) * 0.5).round() as i32,
        )
    }
}

pub(crate) fn active_node_screen_rect(
    st: &Halley,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(active_resize) = active_resize_geometry_screen(st, node_id, resize_preview) {
        return Some((
            active_resize.frame_left,
            active_resize.frame_top,
            active_resize.frame_right,
            active_resize.frame_bottom,
        ));
    }

    let xform = active_node_surface_transform_screen_details(st, w, h, node_id, now, None)?;
    let local_geo = active_node_visual_local_rect(st, node_id).or_else(|| {
        st.model.field.node(node_id).map(|n| {
            (
                0.0,
                0.0,
                n.intrinsic_size.x.max(1.0),
                n.intrinsic_size.y.max(1.0),
            )
        })
    })?;

    let (gx, gy, gw, gh) = local_geo;
    let rw = (gw * xform.scale).round().max(1.0);
    let rh = (gh * xform.scale).round().max(1.0);
    let mut rx = xform.origin_x + (gx * xform.scale).round();
    let mut ry = xform.origin_y + (gy * xform.scale).round();
    let mut rr = rx + rw;
    let mut rb = ry + rh;

    let stack_render_order = active_stacking_render_order_for_monitor(
        st,
        st.model.monitor_state.current_monitor.as_str(),
    );
    if stack_render_order.contains_key(&node_id) {
        let frame_pad_px = active_window_frame_pad_px(&st.runtime.tuning) as f32 * xform.scale;
        rx -= frame_pad_px;
        ry -= frame_pad_px;
        rr += frame_pad_px;
        rb += frame_pad_px;
    }

    Some((rx, ry, rr, rb))
}

pub(crate) fn active_node_surface_transform_screen_details(
    st: &Halley,
    w: i32,
    h: i32,
    node_id: halley_core::field::NodeId,
    now: Instant,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveNodeSurfaceTransformScreen> {
    let n = st.model.field.node(node_id)?;
    if n.state != halley_core::field::NodeState::Active {
        return None;
    }

    let anim = anim_style_for(st, node_id, n.state.clone(), now);
    let transition_alpha =
        crate::compositor::workspace::state::active_transition_alpha(st, node_id, now);
    let cam_scale = st.camera_render_scale();

    let fit_scale = if let Some(monitor) = st.fullscreen_monitor_for_node(node_id) {
        let (target_w, target_h) = if monitor == st.model.monitor_state.current_monitor {
            let ws = st.model.viewport.size;
            (ws.x.round() as i32, ws.y.round() as i32)
        } else {
            st.fullscreen_target_size_for(monitor)
        };
        let sw = (target_w as f32) / n.intrinsic_size.x.max(1.0);
        let sh = (target_h as f32) / n.intrinsic_size.y.max(1.0);
        sw.min(sh).max(0.1)
    } else {
        1.0
    };

    let anim_scale = active_surface_render_scale(
        anim.scale,
        st.active_zoom_lock_scale(),
        n.intrinsic_size.x,
        n.intrinsic_size.y,
        transition_alpha,
    ) * st.fullscreen_entry_scale(node_id, st.now_ms(now))
        * fit_scale
        * cam_scale;

    let (origin_x, origin_y, scale) =
        if let Some(active_resize) = active_resize_geometry_screen(st, node_id, resize_preview) {
            (
                active_resize.surface_origin_x,
                active_resize.surface_origin_y,
                1.0f32,
            )
        } else {
            let p = n.pos;
            let (cx, cy) = world_to_screen(st, w, h, p.x, p.y);

            let bbox_lx = st
                .ui
                .render_state
                .cache
                .bbox_loc
                .get(&node_id)
                .copied()
                .unwrap_or((0.0, 0.0))
                .0;
            let bbox_ly = st
                .ui
                .render_state
                .cache
                .bbox_loc
                .get(&node_id)
                .copied()
                .unwrap_or((0.0, 0.0))
                .1;
            let bbox_w = n.intrinsic_size.x.max(1.0);
            let bbox_h = n.intrinsic_size.y.max(1.0);
            let local_bbox = (bbox_lx, bbox_ly, bbox_w, bbox_h);
            let (gx, gy, gw, gh) = st
                .ui
                .render_state
                .cache
                .window_geometry
                .get(&node_id)
                .copied()
                .map(|(x, y, w, h)| (x, y, w.max(1.0), h.max(1.0)))
                .unwrap_or(local_bbox);

            let (rx, ry) = if st
                .fullscreen_monitor_for_node(node_id)
                .is_some_and(|monitor| monitor == st.model.monitor_state.current_monitor)
            {
                (0, 0)
            } else {
                let rw = (gw * anim_scale).round() as i32;
                let rh = (gh * anim_scale).round() as i32;
                (cx - (rw / 2), cy - (rh / 2))
            };
            let origin_x = (rx as f32) - (gx * anim_scale).round();
            let origin_y = (ry as f32) - (gy * anim_scale).round();

            (origin_x, origin_y, anim_scale)
        };

    Some(ActiveNodeSurfaceTransformScreen {
        origin_x,
        origin_y,
        scale: scale.max(0.001),
    })
}

pub(crate) fn active_resize_geometry_screen(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    resize_preview: Option<ResizeCtx>,
) -> Option<ActiveResizeGeometryScreen> {
    let rz = resize_preview.filter(|rz| rz.node_id == node_id)?;
    if rz.handle == crate::compositor::interaction::ResizeHandle::Pending {
        return None;
    }
    let frame_left = rz.preview_left_px;
    let frame_top = rz.preview_top_px;
    let frame_right = rz.preview_right_px;
    let frame_bottom = rz.preview_bottom_px;
    let (_, _, live_geo_w, live_geo_h) = st
        .ui
        .render_state
        .cache
        .window_geometry
        .get(&node_id)
        .copied()
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    let geo_lx = rz.start_geo_lx;
    let geo_ly = rz.start_geo_ly;

    Some(ActiveResizeGeometryScreen {
        frame_left,
        frame_top,
        frame_right,
        frame_bottom,
        surface_origin_x: frame_left - geo_lx.round(),
        surface_origin_y: frame_top - geo_ly.round(),
        live_geo_w,
        live_geo_h,
    })
}

fn active_node_visual_local_rect(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(&(x, y, w, h)) = st.ui.render_state.cache.window_geometry.get(&node_id) {
        return Some((x, y, w.max(1.0), h.max(1.0)));
    }

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }

        let geo = with_states(wl, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        });
        if let Some(g) = geo {
            return Some((
                g.loc.x as f32,
                g.loc.y as f32,
                g.size.w.max(1) as f32,
                g.size.h.max(1) as f32,
            ));
        }

        let bbox = bbox_from_surface_tree(wl, (0, 0));
        return Some((
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w.max(1) as f32,
            bbox.size.h.max(1) as f32,
        ));
    }

    None
}
