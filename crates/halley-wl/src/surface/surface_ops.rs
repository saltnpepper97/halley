use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::state::HalleyWlState;

pub(crate) fn request_toplevel_resize_mode(
    st: &mut HalleyWlState,
    node_id: halley_core::field::NodeId,
    width: i32,
    height: i32,
    resizing: bool,
) {
    let width = width.max(96);
    let height = height.max(72);
    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(node_id) {
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
        });
        top.send_configure();
        break;
    }
}

pub(crate) fn current_surface_size_for_node(
    st: &HalleyWlState,
    node_id: halley_core::field::NodeId,
) -> Option<halley_core::field::Vec2> {
    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
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
    None
}

pub(crate) fn window_geometry_for_node(
    st: &HalleyWlState,
    node_id: halley_core::field::NodeId,
) -> Option<(f32, f32, f32, f32)> {
    for top in st.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        let geo = with_states(&wl, |states| {
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
        let bbox = bbox_from_surface_tree(&wl, (0, 0));
        return Some((
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w as f32,
            bbox.size.h as f32,
        ));
    }
    None
}
