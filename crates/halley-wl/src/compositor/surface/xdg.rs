use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::compositor::root::Halley;
use halley_core::field::{NodeId, NodeKind, NodeState, Visibility};

const SILENT_CORE_CLOSE_MS: u64 = 2_000;

pub(crate) fn request_close_focused_toplevel(st: &mut Halley) -> bool {
    let focused_monitor = st.focused_monitor().to_string();
    let Some(candidate) = focused_close_candidate(st, focused_monitor.as_str()) else {
        return false;
    };
    let targets = close_targets_for_node(st, candidate);
    if targets.is_empty() {
        return false;
    };

    if close_candidate_is_core(st, candidate) {
        mark_silent_core_close_targets(st, &targets);
    }

    targets.into_iter().fold(false, |closed, node_id| {
        request_close_node_toplevel(st, node_id) || closed
    })
}

#[cfg(test)]
fn close_targets_for_focused_item(st: &Halley, focused_monitor: &str) -> Vec<NodeId> {
    focused_close_candidate(st, focused_monitor)
        .map(|node_id| close_targets_for_node(st, node_id))
        .unwrap_or_default()
}

fn focused_close_candidate(st: &Halley, focused_monitor: &str) -> Option<NodeId> {
    st.model
        .focus_state
        .primary_interaction_focus
        .filter(|&id| node_is_focused_close_candidate(st, id, focused_monitor))
        .or_else(|| {
            st.focused_node_for_monitor(focused_monitor)
                .filter(|&id| node_is_focused_close_candidate(st, id, focused_monitor))
        })
        .or_else(|| st.last_focused_surface_node_for_monitor(focused_monitor))
}

fn node_is_focused_close_candidate(st: &Halley, id: NodeId, focused_monitor: &str) -> bool {
    st.model.field.node(id).is_some()
        && st.model.field.is_visible(id)
        && st
            .model
            .monitor_state
            .node_monitor
            .get(&id)
            .is_none_or(|monitor| monitor == focused_monitor)
}

fn close_targets_for_node(st: &Halley, node_id: NodeId) -> Vec<NodeId> {
    let Some(node) = st.model.field.node(node_id) else {
        return Vec::new();
    };

    match (node.kind.clone(), node.state.clone()) {
        (NodeKind::Surface, _) => vec![node_id],
        (NodeKind::Core, NodeState::Core) => st
            .model
            .field
            .cluster_id_for_core_public(node_id)
            .and_then(|cluster_id| st.model.field.cluster(cluster_id))
            .map(|cluster| cluster.members().to_vec())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn close_candidate_is_core(st: &Halley, node_id: NodeId) -> bool {
    st.model
        .field
        .node(node_id)
        .is_some_and(|node| node.kind == NodeKind::Core && node.state == NodeState::Core)
}

fn mark_silent_core_close_targets(st: &mut Halley, targets: &[NodeId]) {
    let until_ms = st
        .now_ms(std::time::Instant::now())
        .saturating_add(SILENT_CORE_CLOSE_MS);
    for &target in targets {
        st.model
            .workspace_state
            .pending_silent_close_until_ms
            .insert(target, until_ms);
        if let Some(node) = st.model.field.node_mut(target) {
            node.visibility.set(Visibility::HIDDEN_BY_CLUSTER, true);
        }
    }
    st.request_maintenance();
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
    use super::{
        close_targets_for_focused_item, mark_silent_core_close_targets, request_close_node_toplevel,
    };
    use crate::compositor::root::Halley;
    use halley_core::field::{NodeId, Vec2, Visibility};
    use smithay::reexports::wayland_server::Display;

    fn single_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    fn sorted_ids(mut ids: Vec<NodeId>) -> Vec<u64> {
        ids.sort_by_key(|id| id.as_u64());
        ids.into_iter().map(|id| id.as_u64()).collect()
    }

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

    #[test]
    fn close_focused_core_targets_all_cluster_members() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let first = st.model.field.spawn_surface(
            "first",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = st.model.field.spawn_surface(
            "second",
            Vec2 { x: 460.0, y: 260.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(first, "monitor_a");
        st.assign_node_to_monitor(second, "monitor_a");
        let cluster = st.create_cluster(vec![first, second]).expect("cluster");
        let core = st.collapse_cluster(cluster).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        st.set_interaction_focus(Some(core), 30_000, std::time::Instant::now());

        assert_eq!(
            sorted_ids(close_targets_for_focused_item(&st, "monitor_a")),
            sorted_ids(vec![first, second])
        );
    }

    #[test]
    fn silent_core_close_keeps_surviving_members_hidden_after_dissolve() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let first = st.model.field.spawn_surface(
            "first",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = st.model.field.spawn_surface(
            "second",
            Vec2 { x: 460.0, y: 260.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(first, "monitor_a");
        st.assign_node_to_monitor(second, "monitor_a");
        let cluster = st.create_cluster(vec![first, second]).expect("cluster");
        let _core = st.collapse_cluster(cluster).expect("core");

        mark_silent_core_close_targets(&mut st, &[first, second]);
        assert!(st.remove_node_from_field(first, st.now_ms(std::time::Instant::now())));

        assert!(
            st.model
                .field
                .node(second)
                .expect("survivor")
                .visibility
                .has(Visibility::HIDDEN_BY_CLUSTER)
        );
        assert!(!st.model.field.is_visible(second));
    }

    #[test]
    fn close_focused_core_ignores_stale_surface_history() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let stale = st.model.field.spawn_surface(
            "stale",
            Vec2 { x: 40.0, y: 40.0 },
            Vec2 { x: 260.0, y: 180.0 },
        );
        let first = st.model.field.spawn_surface(
            "first",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let second = st.model.field.spawn_surface(
            "second",
            Vec2 { x: 460.0, y: 260.0 },
            Vec2 { x: 240.0, y: 180.0 },
        );
        st.assign_node_to_monitor(stale, "monitor_a");
        st.assign_node_to_monitor(first, "monitor_a");
        st.assign_node_to_monitor(second, "monitor_a");
        st.set_interaction_focus(Some(stale), 30_000, std::time::Instant::now());

        let cluster = st.create_cluster(vec![first, second]).expect("cluster");
        let core = st.collapse_cluster(cluster).expect("core");
        st.assign_node_to_monitor(core, "monitor_a");
        st.set_interaction_focus(Some(core), 30_000, std::time::Instant::now());

        assert_eq!(
            sorted_ids(close_targets_for_focused_item(&st, "monitor_a")),
            sorted_ids(vec![first, second])
        );
    }

    #[test]
    fn close_focused_surface_targets_that_surface() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
        let surface = st.model.field.spawn_surface(
            "surface",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(surface, "monitor_a");
        st.set_interaction_focus(Some(surface), 30_000, std::time::Instant::now());

        assert_eq!(
            close_targets_for_focused_item(&st, "monitor_a"),
            vec![surface]
        );
    }

    #[test]
    fn close_focused_does_not_fall_back_to_other_monitor_history() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = single_monitor_tuning();
        tuning
            .tty_viewports
            .push(halley_config::ViewportOutputConfig {
                connector: "monitor_b".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            });
        let mut st = Halley::new_for_test(&dh, tuning);
        let other = st.model.field.spawn_surface(
            "other-monitor",
            Vec2 { x: 120.0, y: 140.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        st.assign_node_to_monitor(other, "monitor_b");
        st.set_interaction_focus(Some(other), 30_000, std::time::Instant::now());
        st.focus_monitor_view("monitor_a", std::time::Instant::now());

        assert!(close_targets_for_focused_item(&st, "monitor_a").is_empty());
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
