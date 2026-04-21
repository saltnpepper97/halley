use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::compositor::root::Halley;

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
    if crate::compositor::workspace::state::node_in_maximize_session(st, node_id)
        && let Some(monitor) = st.model.monitor_state.node_monitor.get(&node_id).cloned()
    {
        let _ = crate::compositor::workspace::state::abort_maximize_session_for_monitor(
            st,
            monitor.as_str(),
        );
    }
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

#[cfg(test)]
mod tests {
    use super::request_close_node_toplevel;
    use crate::compositor::root::Halley;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    #[test]
    fn close_request_immediately_aborts_maximize_session() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.animations.maximize.enabled = false;
        let mut st = Halley::new_for_test(&dh, tuning);

        st.model.zoom_ref_size = Vec2 { x: 500.0, y: 375.0 };
        st.model.camera_target_view_size = st.model.zoom_ref_size;
        st.model.viewport.center = Vec2 { x: 430.0, y: 280.0 };
        st.model.camera_target_center = st.model.viewport.center;

        let monitor = st.model.monitor_state.current_monitor.clone();
        let target = st.model.field.spawn_surface(
            "target",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let bystander = st.model.field.spawn_surface(
            "bystander",
            Vec2 { x: 460.0, y: 260.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(target, monitor.as_str());
        st.assign_node_to_monitor(bystander, monitor.as_str());

        assert!(
            crate::compositor::actions::window::toggle_node_maximize_state(
                &mut st,
                target,
                std::time::Instant::now(),
                monitor.as_str(),
            )
        );

        let _ = request_close_node_toplevel(&mut st, target);

        assert!(
            !st.model
                .workspace_state
                .maximize_sessions
                .contains_key(monitor.as_str())
        );
        assert_eq!(
            st.model.field.node(bystander).expect("bystander").pos,
            Vec2 { x: 460.0, y: 260.0 }
        );
        assert_eq!(st.model.zoom_ref_size, Vec2 { x: 500.0, y: 375.0 });
        assert_eq!(st.model.viewport.center, Vec2 { x: 430.0, y: 280.0 });
    }
}

pub(crate) fn toplevel_min_size_for_node(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> (i32, i32) {
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        if st.model.surface_to_node.get(&wl.id()).copied() == Some(node_id) {
            return with_states(wl, |states| {
                let mut cached = states.cached_state.get::<SurfaceCachedState>();
                let state = cached.current();
                (state.min_size.w, state.min_size.h)
            });
        }
    }
    (0, 0)
}

pub(crate) fn request_toplevel_resize_mode(
    st: &mut Halley,
    node_id: halley_core::field::NodeId,
    width: i32,
    height: i32,
    resizing: bool,
) {
    let (min_w, min_h) = toplevel_min_size_for_node(st, node_id);
    let width = width.max(min_w).max(96);
    let height = height.max(min_h).max(72);
    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();
        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }
        let monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| st.focused_monitor().to_string());
        let view = st.usable_viewport_for_monitor(&monitor);
        let bounds_w = view.size.x as i32;
        let bounds_h = view.size.y as i32;
        top.with_pending_state(|s| {
            s.size = Some((width, height).into());
            s.bounds = Some((bounds_w, bounds_h).into());
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
    if let Some(&(_, _, w, h)) = st.ui.render_state.cache.window_geometry.get(&node_id) {
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
    if let Some(&geo) = st.ui.render_state.cache.window_geometry.get(&node_id) {
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
            .cache
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
