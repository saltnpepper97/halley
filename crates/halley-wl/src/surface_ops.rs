use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::state::Halley;

pub(crate) fn request_close_focused_toplevel(st: &mut Halley) -> bool {
    let Some(node_id) = st
        .last_focused_surface_node_for_monitor(st.focused_monitor())
        .or_else(|| st.last_focused_surface_node())
    else {
        return false;
    };

    request_close_node_toplevel(st, node_id)
}

pub(crate) fn request_close_node_toplevel(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
) -> bool {
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        top.send_close();
        return true;
    }

    false
}

pub(crate) fn request_toplevel_resize_mode(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    width: i32,
    height: i32,
    resizing: bool,
) {
    let width = width.max(96);
    let height = height.max(72);
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        top.with_pending_state(|s| {
            s.size = Some((width, height).into());
            // Keep toplevel activated during compositor-driven interactive resize.
            // Some CSD clients behave poorly if activation silently drops.
            s.states.set(xdg_toplevel::State::Activated);
            if resizing {
                s.states.set(xdg_toplevel::State::Resizing);
            } else {
                s.states.unset(xdg_toplevel::State::Resizing);
            }
            st.apply_toplevel_tiled_hint(s);
        });
        top.send_configure();
        break;
    }
}

pub(crate) fn current_surface_size_for_node(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<halley_core::field::Vec2> {
    if let Some(&(_, _, w, h)) = st.ui.render_state.window_geometry.get(&node_id) {
        return Some(halley_core::field::Vec2 {
            x: w.max(1.0),
            y: h.max(1.0),
        });
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
            return Some(halley_core::field::Vec2 {
                x: g.size.w.max(1) as f32,
                y: g.size.h.max(1) as f32,
            });
        }
        if let Some(sz) = top.current_state().size {
            return Some(halley_core::field::Vec2 {
                x: sz.w.max(1) as f32,
                y: sz.h.max(1) as f32,
            });
        }
        let bbox = bbox_from_surface_tree(wl, (0, 0));
        return Some(halley_core::field::Vec2 {
            x: bbox.size.w.max(1) as f32,
            y: bbox.size.h.max(1) as f32,
        });
    }
    st.model
        .field
        .node(node_id)
        .map(|node| halley_core::field::Vec2 {
            x: node.intrinsic_size.x.max(1.0),
            y: node.intrinsic_size.y.max(1.0),
        })
}

pub(crate) fn window_geometry_for_node(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<(f32, f32, f32, f32)> {
    if let Some(&geo) = st.ui.render_state.window_geometry.get(&node_id) {
        return Some(geo);
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
                g.size.w as f32,
                g.size.h as f32,
            ));
        }
        if let Some(sz) = top.current_state().size {
            return Some((0.0, 0.0, sz.w as f32, sz.h as f32));
        }
        let bbox = bbox_from_surface_tree(wl, (0, 0));
        return Some((
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w as f32,
            bbox.size.h as f32,
        ));
    }
    st.model.field.node(node_id).map(|node| {
        let (bbox_lx, bbox_ly) = st
            .ui
            .render_state
            .bbox_loc
            .get(&node_id)
            .copied()
            .unwrap_or((0.0, 0.0));
        (
            bbox_lx,
            bbox_ly,
            node.intrinsic_size.x.max(1.0),
            node.intrinsic_size.y.max(1.0),
        )
    })
}
