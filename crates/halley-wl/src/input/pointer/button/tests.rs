use std::time::{Duration, Instant};

use super::press::handle_left_press;
use super::*;
use crate::backend::interface::TtyBackendHandle;
use crate::compositor::interaction::state::{PendingCollapsedNodeClick, PendingCoreClick};
use crate::compositor::interaction::{HitNode, PointerState};
use smithay::reexports::wayland_server::Display;

fn single_monitor_tuning() -> halley_config::RuntimeTuning {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
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

fn pointer_frame() -> ButtonFrame {
    ButtonFrame {
        ws_w: 800,
        ws_h: 600,
        global_sx: 400.0,
        global_sy: 300.0,
        sx: 400.0,
        sy: 300.0,
        world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
        workspace_active: false,
    }
}

fn spawn_collapsed_surface(
    st: &mut Halley,
    label: &str,
    pos: halley_core::field::Vec2,
) -> halley_core::field::NodeId {
    let node =
        st.model
            .field
            .spawn_surface(label, pos, halley_core::field::Vec2 { x: 320.0, y: 240.0 });
    st.assign_node_to_monitor(node, "monitor_a");
    assert!(
        st.model
            .field
            .set_state(node, halley_core::field::NodeState::Node)
    );
    node
}

#[test]
fn workspace_move_target_double_click_does_not_exit_cluster() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let backend = TtyBackendHandle::new(800, 600);

    let master = st.model.field.spawn_surface(
        "master",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        halley_core::field::Vec2 { x: 500.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    let mut ps = PointerState::default();

    handle_workspace_left_press(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: master,
            move_surface: true,
            is_core: false,
        },
    );

    assert_eq!(
        st.active_cluster_workspace_for_monitor("monitor_a"),
        Some(cid)
    );
}

#[test]
fn core_single_click_only_focuses_without_opening_bloom() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let backend = TtyBackendHandle::new(800, 600);

    let master = st.model.field.spawn_surface(
        "master",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        halley_core::field::Vec2 { x: 500.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let mut ps = PointerState::default();
    handle_core_left_press(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: core,
            move_surface: true,
            is_core: true,
        },
        ButtonFrame {
            ws_w: 800,
            ws_h: 600,
            global_sx: 400.0,
            global_sy: 300.0,
            sx: 400.0,
            sy: 300.0,
            world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
            workspace_active: false,
        },
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));
    let pending_press = st
        .input
        .interaction_state
        .pending_core_press
        .take()
        .expect("pending core press");
    st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
        node_id: pending_press.node_id,
        monitor: pending_press.monitor,
        deadline_ms: st.now_ms(Instant::now()),
    });

    st.run_maintenance(Instant::now());

    assert_eq!(st.cluster_bloom_for_monitor("monitor_a"), None);
    assert_eq!(st.active_cluster_workspace_for_monitor("monitor_a"), None);
}

#[test]
fn core_double_click_enters_cluster_workspace() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let backend = TtyBackendHandle::new(800, 600);

    let master = st.model.field.spawn_surface(
        "master",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        halley_core::field::Vec2 { x: 500.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let frame = ButtonFrame {
        ws_w: 800,
        ws_h: 600,
        global_sx: 400.0,
        global_sy: 300.0,
        sx: 400.0,
        sy: 300.0,
        world_now: halley_core::field::Vec2 { x: 400.0, y: 300.0 },
        workspace_active: false,
    };
    let mut ps = PointerState::default();
    handle_core_left_press(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: core,
            move_surface: true,
            is_core: true,
        },
        frame,
    );
    let pending_press = st
        .input
        .interaction_state
        .pending_core_press
        .take()
        .expect("pending core press");
    st.input.interaction_state.pending_core_click = Some(PendingCoreClick {
        node_id: pending_press.node_id,
        monitor: pending_press.monitor,
        deadline_ms: st.now_ms(Instant::now()) + 350,
    });

    handle_core_left_press(
        &mut st,
        &mut ps,
        &backend,
        HitNode {
            node_id: core,
            move_surface: true,
            is_core: true,
        },
        frame,
    );

    assert_eq!(
        st.active_cluster_workspace_for_monitor("monitor_a"),
        Some(cid)
    );
}

#[test]
fn collapsed_node_single_click_only_focuses_without_promoting() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let backend = TtyBackendHandle::new(800, 600);
    let node = spawn_collapsed_surface(
        &mut st,
        "collapsed",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
    );

    let mut ps = PointerState::default();
    handle_left_press(
        &mut st,
        &mut ps,
        &backend,
        false,
        false,
        Some(HitNode {
            node_id: node,
            move_surface: false,
            is_core: false,
        }),
        pointer_frame(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(node));
    assert_eq!(
        st.model.field.node(node).expect("collapsed node").state,
        halley_core::field::NodeState::Node
    );
    assert!(
        !st.model
            .workspace_state
            .active_transition_until_ms
            .contains_key(&node)
    );
    assert!(
        st.input
            .interaction_state
            .pending_collapsed_node_press
            .is_some()
    );
    assert!(st.input.interaction_state.viewport_pan_anim.is_none());
}

#[test]
fn collapsed_node_single_click_reveals_when_offscreen() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let backend = TtyBackendHandle::new(800, 600);
    let node = spawn_collapsed_surface(
        &mut st,
        "collapsed-offscreen",
        halley_core::field::Vec2 {
            x: 3000.0,
            y: 100.0,
        },
    );

    let mut ps = PointerState::default();
    handle_left_press(
        &mut st,
        &mut ps,
        &backend,
        false,
        false,
        Some(HitNode {
            node_id: node,
            move_surface: false,
            is_core: false,
        }),
        pointer_frame(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(node));
    assert_eq!(
        st.model.field.node(node).expect("collapsed node").state,
        halley_core::field::NodeState::Node
    );
    assert!(
        !st.model
            .workspace_state
            .active_transition_until_ms
            .contains_key(&node)
    );
    assert!(st.input.interaction_state.viewport_pan_anim.is_some());
}

#[test]
fn collapsed_node_double_click_promotes() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let backend = TtyBackendHandle::new(800, 600);
    let node = spawn_collapsed_surface(
        &mut st,
        "collapsed",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
    );

    let mut ps = PointerState::default();
    handle_left_press(
        &mut st,
        &mut ps,
        &backend,
        false,
        false,
        Some(HitNode {
            node_id: node,
            move_surface: false,
            is_core: false,
        }),
        pointer_frame(),
    );
    let pending_press = st
        .input
        .interaction_state
        .pending_collapsed_node_press
        .take()
        .expect("pending collapsed node press");
    st.input.interaction_state.pending_collapsed_node_click = Some(PendingCollapsedNodeClick {
        node_id: pending_press.node_id,
        deadline_ms: st.now_ms(Instant::now()) + 350,
    });

    handle_left_press(
        &mut st,
        &mut ps,
        &backend,
        false,
        false,
        Some(HitNode {
            node_id: node,
            move_surface: false,
            is_core: false,
        }),
        pointer_frame(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(node));
    assert!(
        st.model
            .workspace_state
            .active_transition_until_ms
            .contains_key(&node)
    );
    assert!(
        st.input
            .interaction_state
            .pending_collapsed_node_click
            .is_none()
    );
    assert!(
        st.input
            .interaction_state
            .pending_collapsed_node_press
            .is_none()
    );
}

#[test]
fn hovering_core_long_enough_opens_bloom() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        halley_core::field::Vec2 { x: 500.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    st.input.interaction_state.pending_core_hover =
        Some(crate::compositor::interaction::state::PendingCoreHover {
            node_id: core,
            monitor: "monitor_a".to_string(),
            started_at_ms: st.now_ms(Instant::now()),
        });

    crate::frame_loop::tick_frame_effects(
        &mut st,
        Instant::now()
            + Duration::from_millis(crate::compositor::interaction::CORE_BLOOM_HOLD_MS + 1),
    );

    assert_eq!(st.cluster_bloom_for_monitor("monitor_a"), Some(cid));
    assert!(st.input.interaction_state.pending_core_hover.is_none());
}
