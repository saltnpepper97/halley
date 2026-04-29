use std::time::Instant;

use super::*;
use crate::compositor::overlap::physics::{MAX_PHYSICS_SPEED, PHYSICS_REST_EPSILON};
use crate::frame_loop::anim_style_for;
use crate::presentation::node_render_diameter_px;
use crate::window::active_window_frame_pad_px;

fn overlap_metrics(state: &Halley, a: NodeId, b: NodeId) -> (f32, f32, f32, f32) {
    let na = state.model.field.node(a).expect("node a");
    let nb = state.model.field.node(b).expect("node b");
    let ea = state.collision_extents_for_node(na);
    let eb = state.collision_extents_for_node(nb);
    let gap = state.non_overlap_gap_world();
    let dx = (nb.pos.x - na.pos.x).abs();
    let dy = (nb.pos.y - na.pos.y).abs();
    let req_x = state.required_sep_x(na.pos.x, ea, nb.pos.x, eb, gap);
    let req_y = state.required_sep_y(na.pos.y, ea, nb.pos.y, eb, gap);
    (dx, dy, req_x, req_y)
}

fn nodes_overlap(state: &Halley, a: NodeId, b: NodeId) -> bool {
    let (dx, dy, req_x, req_y) = overlap_metrics(state, a, b);
    dx < req_x && dy < req_y
}

fn tick_overlap_frames(state: &mut Halley, frames: usize) {
    for _ in 0..frames {
        state.resolve_surface_overlap();
    }
}

#[test]
fn resolve_surface_overlap_separates_windows_when_physics_disabled() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.physics_enabled = false;
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });

    state.resolve_surface_overlap();

    assert!(
        !nodes_overlap(&state, a, b),
        "static overlap resolution should separate overlapping windows when physics is disabled"
    );
}

#[test]
fn pinned_node_is_not_displaced_by_overlap_and_unpin_restores_motion() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.physics_enabled = false;
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let pinned = state.model.field.spawn_surface(
        "pinned",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let other = state.model.field.spawn_surface(
        "other",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    assert!(state.set_node_user_pinned(pinned, true));

    state.resolve_surface_overlap();

    assert_eq!(
        state.model.field.node(pinned).expect("pinned").pos,
        Vec2 { x: 0.0, y: 0.0 }
    );
    assert_ne!(
        state.model.field.node(other).expect("other").pos,
        Vec2 { x: 0.0, y: 0.0 }
    );

    assert!(state.set_node_user_pinned(pinned, false));
    state.model.field.node_mut(pinned).expect("pinned").pos = Vec2 { x: 0.0, y: 0.0 };
    state.model.field.node_mut(other).expect("other").pos = Vec2 { x: 0.0, y: 0.0 };
    state.resolve_surface_overlap();

    assert_ne!(
        state.model.field.node(pinned).expect("pinned").pos,
        Vec2 { x: 0.0, y: 0.0 }
    );
}

#[test]
fn physics_carry_clamps_mover_against_pinned_neighbor_immediately() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let pinned = state.model.field.spawn_surface(
        "pinned",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let mover = state.model.field.spawn_surface(
        "mover",
        Vec2 { x: 800.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    assert!(state.set_node_user_pinned(pinned, true));

    assert!(state.carry_surface_non_overlap(mover, Vec2 { x: 0.0, y: 0.0 }, false));

    assert_eq!(
        state.model.field.node(pinned).expect("pinned").pos,
        Vec2 { x: 0.0, y: 0.0 }
    );
    assert!(
        !nodes_overlap(&state, pinned, mover),
        "mover should bump against pinned neighbor without an overlap frame"
    );
}

#[test]
fn collapsed_surface_nodes_use_marker_collision_extents() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let id = state.model.field.spawn_surface(
        "collapsed-firefox",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 {
            x: 1200.0,
            y: 900.0,
        },
    );
    let _ = state
        .model
        .field
        .set_state(id, halley_core::field::NodeState::Node);

    let node = state.model.field.node(id).expect("node");
    let ext = state.collision_extents_for_node(node);

    assert!(
        ext.left + ext.right < 300.0,
        "collapsed node collision width should stay marker-sized, got {:?}",
        ext
    );
    assert!(
        ext.top + ext.bottom < 120.0,
        "collapsed node collision height should stay marker-sized, got {:?}",
        ext
    );
}

#[test]
fn collapsed_surface_nodes_match_rendered_node_diameter() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let id = state.model.field.spawn_surface(
        "collapsed-firefox",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 {
            x: 1200.0,
            y: 900.0,
        },
    );
    let _ = state
        .model
        .field
        .set_state(id, halley_core::field::NodeState::Node);

    let node = state.model.field.node(id).expect("node");
    let ext = state.collision_extents_for_node(node);
    let anim = anim_style_for(&state, id, node.state.clone(), Instant::now());
    let expected =
        node_render_diameter_px(&state, node.intrinsic_size, node.label.len(), anim.scale);

    assert_eq!(ext.left + ext.right, expected.round());
    assert_eq!(ext.top + ext.bottom, expected.round());
}

#[test]
fn resolve_overlap_settles_collapsed_nodes_when_zoomed_out() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 3200.0,
        y: 2400.0,
    };

    let a = state.model.field.spawn_surface(
        "alpha",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let b = state.model.field.spawn_surface(
        "beta",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let _ = state
        .model
        .field
        .set_state(a, halley_core::field::NodeState::Node);
    let _ = state
        .model
        .field
        .set_state(b, halley_core::field::NodeState::Node);

    tick_overlap_frames(&mut state, 64);

    let (dx, dy, req_x, req_y) = overlap_metrics(&state, a, b);

    assert!(
        dx >= req_x || dy >= req_y,
        "collapsed nodes still overlap after zoomed-out settle: a={:?} b={:?} req=({}, {})",
        state.model.field.node(a).expect("node a").pos,
        state.model.field.node(b).expect("node b").pos,
        req_x,
        req_y
    );
}

#[test]
fn overlap_resolution_is_not_limited_to_current_monitor() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.tty_viewports = vec![
        halley_config::ViewportOutputConfig {
            connector: "left".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        },
        halley_config::ViewportOutputConfig {
            connector: "right".to_string(),
            enabled: true,
            offset_x: 800,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        },
    ];
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    let _ = state.activate_monitor("left");

    let a = state.model.field.spawn_surface(
        "right-a",
        Vec2 {
            x: 1200.0,
            y: 300.0,
        },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let b = state.model.field.spawn_surface(
        "right-b",
        Vec2 {
            x: 1200.0,
            y: 300.0,
        },
        Vec2 { x: 320.0, y: 220.0 },
    );
    state.assign_node_to_monitor(a, "right");
    state.assign_node_to_monitor(b, "right");
    let _ = state
        .model
        .field
        .set_state(a, halley_core::field::NodeState::Node);
    let _ = state
        .model
        .field
        .set_state(b, halley_core::field::NodeState::Node);

    tick_overlap_frames(&mut state, 64);

    assert!(
        !nodes_overlap(&state, a, b),
        "right-monitor overlap should resolve even while current monitor is left"
    );
}

#[test]
fn dragged_window_is_authoritative_while_neighbor_yields() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let active = state.model.field.spawn_surface(
        "active",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 400.0, y: 260.0 },
    );
    let node = state.model.field.spawn_surface(
        "collapsed",
        Vec2 { x: 600.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let _ = state
        .model
        .field
        .set_state(node, halley_core::field::NodeState::Node);

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(node));
    assert!(state.carry_surface_non_overlap(node, Vec2 { x: 0.0, y: 0.0 }, false));
    state.resolve_surface_overlap();

    let active_node = state.model.field.node(active).expect("active surface");
    let collapsed_node = state.model.field.node(node).expect("collapsed node");

    assert_eq!(collapsed_node.pos, Vec2 { x: 0.0, y: 0.0 });
    assert!(active_node.pos != Vec2 { x: 0.0, y: 0.0 });
}

#[test]
fn dragged_window_pushes_collapsed_core_and_members_follow() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let dragged = state.model.field.spawn_surface(
        "dragged",
        Vec2 { x: 400.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 200.0, y: 140.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 200.0, y: 140.0 });
    let cid = state
        .create_cluster(vec![a, b])
        .expect("cluster");
    let core = state.collapse_cluster(cid).expect("core");

    let core_before = state.model.field.node(core).expect("core before").pos;
    let a_before = state.model.field.node(a).expect("a before").pos;
    let b_before = state.model.field.node(b).expect("b before").pos;

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
    assert!(state.carry_surface_non_overlap(dragged, Vec2 { x: 0.0, y: 0.0 }, false));
    state.resolve_surface_overlap();

    let dragged_after = state.model.field.node(dragged).expect("dragged after");
    let core_after = state.model.field.node(core).expect("core after");
    let a_after = state.model.field.node(a).expect("a after");
    let b_after = state.model.field.node(b).expect("b after");

    assert_eq!(dragged_after.pos, Vec2 { x: 0.0, y: 0.0 });
    assert!(core_after.pos != core_before);
    assert_eq!(a_after.pos, a_before);
    assert_eq!(b_after.pos, b_before);
}

#[test]
fn dragged_window_pushes_neighbor_when_physics_disabled() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.physics_enabled = false;
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let dragged = state.model.field.spawn_surface(
        "dragged",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let passive = state.model.field.spawn_surface(
        "passive",
        Vec2 { x: 430.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let passive_before = state.model.field.node(passive).expect("passive before").pos;

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
    assert!(state.carry_surface_non_overlap(dragged, Vec2 { x: 280.0, y: 0.0 }, false));

    let dragged_after = state.model.field.node(dragged).expect("dragged after");
    let passive_after = state.model.field.node(passive).expect("passive after");

    assert!(dragged_after.pos.x < 280.0);
    assert!(passive_after.pos.x > passive_before.x);
    assert!(!nodes_overlap(&state, dragged, passive));
}

#[test]
fn dragged_window_yields_against_pinned_neighbor_when_physics_disabled() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.physics_enabled = false;
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let dragged = state.model.field.spawn_surface(
        "dragged",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let pinned = state.model.field.spawn_surface(
        "pinned",
        Vec2 { x: 430.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let pinned_before = state.model.field.node(pinned).expect("pinned before").pos;
    let _ = state.model.field.set_pinned(pinned, true);

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
    assert!(state.carry_surface_non_overlap(dragged, Vec2 { x: 280.0, y: 0.0 }, false));

    let dragged_after = state.model.field.node(dragged).expect("dragged after");
    let pinned_after = state.model.field.node(pinned).expect("pinned after");

    assert_eq!(pinned_after.pos, pinned_before);
    assert!(dragged_after.pos.x < 280.0);
    assert!(!nodes_overlap(&state, dragged, pinned));
}

#[test]
fn dragged_window_pushes_collapsed_core_when_physics_disabled() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.physics_enabled = false;
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let dragged = state.model.field.spawn_surface(
        "dragged",
        Vec2 { x: 400.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 200.0, y: 140.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 200.0, y: 140.0 });
    let cid = state
        .create_cluster(vec![a, b])
        .expect("cluster");
    let core = state.collapse_cluster(cid).expect("core");

    let core_before = state.model.field.node(core).expect("core before").pos;
    let a_before = state.model.field.node(a).expect("a before").pos;
    let b_before = state.model.field.node(b).expect("b before").pos;

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
    assert!(state.carry_surface_non_overlap(dragged, Vec2 { x: 0.0, y: 0.0 }, false));

    let core_after = state.model.field.node(core).expect("core after");
    assert!(core_after.pos != core_before);
    assert_eq!(state.model.field.node(a).expect("a after").pos, a_before);
    assert_eq!(state.model.field.node(b).expect("b after").pos, b_before);
    assert!(!nodes_overlap(&state, dragged, core));
}

#[test]
fn active_surface_collision_extents_include_frame_pad() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let id = state.model.field.spawn_surface(
        "active",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 400.0, y: 260.0 },
    );
    let node = state.model.field.node(id).expect("active node");
    let ext = state.surface_window_collision_extents(node);
    let expected_half_w =
        node.intrinsic_size.x * 0.5 + active_window_frame_pad_px(&state.runtime.tuning) as f32;
    let expected_half_h =
        node.intrinsic_size.y * 0.5 + active_window_frame_pad_px(&state.runtime.tuning) as f32;

    assert_eq!(ext.left, expected_half_w);
    assert_eq!(ext.right, expected_half_w);
    assert_eq!(ext.top, expected_half_h);
    assert_eq!(ext.bottom, expected_half_h);
}

#[test]
fn surface_collision_extents_ignore_asymmetric_bbox_offsets() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let id = state.model.field.spawn_surface(
        "gtk-like",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 {
            x: 1200.0,
            y: 920.0,
        },
    );
    state.ui.render_state.cache.bbox_loc.insert(id, (4.0, 6.0));
    state
        .ui
        .render_state
        .cache
        .window_geometry
        .insert(id, (12.0, 18.0, 840.0, 620.0));

    let node = state.model.field.node(id).expect("surface node");
    let ext = state.surface_window_collision_extents(node);
    let expected_half_w = 420.0 + active_window_frame_pad_px(&state.runtime.tuning) as f32;
    let expected_half_h = 310.0 + active_window_frame_pad_px(&state.runtime.tuning) as f32;

    assert_eq!(ext.left, expected_half_w);
    assert_eq!(ext.right, expected_half_w);
    assert_eq!(ext.top, expected_half_h);
    assert_eq!(ext.bottom, expected_half_h);
}

#[test]
fn active_overlap_extents_stay_symmetric_with_asymmetric_bbox_offsets() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let id = state.model.field.spawn_surface(
        "gtk-like",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 {
            x: 1200.0,
            y: 920.0,
        },
    );
    state.ui.render_state.cache.bbox_loc.insert(id, (4.0, 6.0));
    state
        .ui
        .render_state
        .cache
        .window_geometry
        .insert(id, (12.0, 18.0, 840.0, 620.0));

    let node = state.model.field.node(id).expect("surface node");
    let ext = state.collision_extents_for_node(node);

    assert_eq!(ext.left, ext.right, "expected symmetric x extents: {ext:?}");
    assert_eq!(ext.top, ext.bottom, "expected symmetric y extents: {ext:?}");
}

#[test]
fn core_node_collision_extents_match_rendered_core_size() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let id = state.model.field.spawn_surface(
        "core",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 400.0, y: 260.0 },
    );
    let _ = state
        .model
        .field
        .set_state(id, halley_core::field::NodeState::Core);

    let node = state.model.field.node(id).expect("core node");
    let ext = state.collision_extents_for_node(node);

    assert_eq!(ext.left, 34.0);
    assert_eq!(ext.right, 34.0);
    assert_eq!(ext.top, 34.0);
    assert_eq!(ext.bottom, 34.0);
}

#[test]
fn resolve_overlap_settles_collapsed_nodes() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let a = state.model.field.spawn_surface(
        "alpha",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let b = state.model.field.spawn_surface(
        "beta",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 320.0, y: 220.0 },
    );
    let _ = state
        .model
        .field
        .set_state(a, halley_core::field::NodeState::Node);
    let _ = state
        .model
        .field
        .set_state(b, halley_core::field::NodeState::Node);

    tick_overlap_frames(&mut state, 64);

    let (dx, dy, req_x, req_y) = overlap_metrics(&state, a, b);

    assert!(dx >= req_x || dy >= req_y);
}

#[test]
fn resolve_overlap_settles_active_surface_and_node() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let active = state.model.field.spawn_surface(
        "active",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let node = state.model.field.spawn_surface(
        "node",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 300.0, y: 200.0 },
    );
    let _ = state
        .model
        .field
        .set_state(node, halley_core::field::NodeState::Node);

    tick_overlap_frames(&mut state, 96);

    let (dx, dy, req_x, req_y) = overlap_metrics(&state, active, node);

    assert!(dx >= req_x || dy >= req_y);
}

#[test]
fn resolve_overlap_settles_two_active_surfaces() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);
    state.model.viewport.size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };
    state.model.zoom_ref_size = Vec2 {
        x: 1600.0,
        y: 1200.0,
    };

    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });

    tick_overlap_frames(&mut state, 128);

    let (dx, dy, req_x, req_y) = overlap_metrics(&state, a, b);
    assert!(dx >= req_x || dy >= req_y);
}

#[test]
fn body_velocity_is_bounded_under_contact() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });

    for _ in 0..12 {
        state.resolve_surface_overlap();
        let vel_a = state
            .input
            .interaction_state
            .physics_velocity
            .get(&a)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        let vel_b = state
            .input
            .interaction_state
            .physics_velocity
            .get(&b)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        assert!(
            vel_a.x.abs() <= MAX_PHYSICS_SPEED
                && vel_a.y.abs() <= MAX_PHYSICS_SPEED
                && vel_b.x.abs() <= MAX_PHYSICS_SPEED
                && vel_b.y.abs() <= MAX_PHYSICS_SPEED
        );
    }
}

#[test]
fn angled_drag_contact_does_not_create_unbounded_velocity() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let passive = state.model.field.spawn_surface(
        "passive",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let dragged = state.model.field.spawn_surface(
        "dragged",
        Vec2 {
            x: -420.0,
            y: -280.0,
        },
        Vec2 { x: 320.0, y: 220.0 },
    );

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
    for step in 0..48 {
        let to = Vec2 {
            x: -180.0 + step as f32 * 9.0,
            y: -120.0 + step as f32 * 5.5,
        };
        let _ = state.carry_surface_non_overlap(dragged, to, false);
        state.resolve_surface_overlap();
        let vel = state
            .input
            .interaction_state
            .physics_velocity
            .get(&passive)
            .copied()
            .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        assert!(vel.x.abs() <= MAX_PHYSICS_SPEED && vel.y.abs() <= MAX_PHYSICS_SPEED);
    }
}

#[test]
fn release_clears_grabbed_window_momentum() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let id = state.model.field.spawn_surface(
        "release",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    state
        .input
        .interaction_state
        .physics_velocity
        .insert(id, Vec2 { x: 480.0, y: 120.0 });
    crate::compositor::carry::system::finalize_mouse_drag_state(
        &mut state,
        id,
        Vec2 { x: 0.0, y: 0.0 },
        Instant::now(),
    );

    assert!(
        !state
            .input
            .interaction_state
            .physics_velocity
            .contains_key(&id)
    );
}

#[test]
fn direct_border_hit_triggers_physics_response() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
    let ea = state.collision_extents_for_node(state.model.field.node(a).expect("a"));
    let eb = state.collision_extents_for_node(state.model.field.node(b).expect("b"));
    let req_x = state.required_sep_x(0.0, ea, 1.0, eb, state.non_overlap_gap_world());
    let _ = state.model.field.carry(b, Vec2 { x: req_x, y: 0.0 });
    state
        .input
        .interaction_state
        .physics_velocity
        .insert(a, Vec2 { x: 320.0, y: 0.0 });
    state
        .input
        .interaction_state
        .physics_velocity
        .insert(b, Vec2 { x: 0.0, y: 0.0 });

    state.resolve_surface_overlap();

    let vb = state
        .input
        .interaction_state
        .physics_velocity
        .get(&b)
        .copied()
        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
    assert!(vb.x > 0.0);
}

#[test]
fn grabbed_window_kinematic_velocity_pushes_neighbor_without_retaining_momentum() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let dragged = state.model.field.spawn_surface(
        "dragged",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let passive = state.model.field.spawn_surface(
        "passive",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 420.0, y: 280.0 },
    );
    let ea = state.collision_extents_for_node(state.model.field.node(dragged).expect("dragged"));
    let eb = state.collision_extents_for_node(state.model.field.node(passive).expect("passive"));
    let req_x = state.required_sep_x(0.0, ea, 1.0, eb, state.non_overlap_gap_world());
    let _ = state.model.field.carry(
        passive,
        Vec2 {
            x: req_x - 1.0,
            y: 0.0,
        },
    );

    crate::compositor::carry::system::set_drag_authority_node(&mut state, Some(dragged));
    state.input.interaction_state.drag_authority_velocity = Vec2 { x: 420.0, y: 0.0 };

    state.resolve_surface_overlap();

    let passive_velocity = state
        .input
        .interaction_state
        .physics_velocity
        .get(&passive)
        .copied()
        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
    assert!(passive_velocity.x > 0.0);
    assert!(
        !state
            .input
            .interaction_state
            .physics_velocity
            .contains_key(&dragged)
    );
}

#[test]
fn windows_settle_back_to_rest_after_contact_clears() {
    let tuning = halley_config::RuntimeTuning::default();
    let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
        .expect("display")
        .handle();
    let mut state = Halley::new_for_test(&dh, tuning);

    let a =
        state
            .model
            .field
            .spawn_surface("a", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });
    let b =
        state
            .model
            .field
            .spawn_surface("b", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 420.0, y: 280.0 });

    tick_overlap_frames(&mut state, 12);
    let _ = state.carry_surface_non_overlap(b, Vec2 { x: 700.0, y: 0.0 }, false);
    tick_overlap_frames(&mut state, 24);

    let va = state
        .input
        .interaction_state
        .physics_velocity
        .get(&a)
        .copied()
        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
    let vb = state
        .input
        .interaction_state
        .physics_velocity
        .get(&b)
        .copied()
        .unwrap_or(Vec2 { x: 0.0, y: 0.0 });

    assert!(
        va.x.abs() <= PHYSICS_REST_EPSILON
            && va.y.abs() <= PHYSICS_REST_EPSILON
            && vb.x.abs() <= PHYSICS_REST_EPSILON
            && vb.y.abs() <= PHYSICS_REST_EPSILON
    );
    assert!(!nodes_overlap(&state, a, b));
}
