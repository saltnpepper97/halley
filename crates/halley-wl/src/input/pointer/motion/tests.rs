use crate::compositor::interaction::HitNode;
use crate::compositor::root::Halley;
use halley_config::InputFocusMode;
use smithay::reexports::wayland_server::Display;
use std::time::Instant;

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

#[test]
fn hover_focus_mode_focuses_hovered_surface() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut tuning = single_monitor_tuning();
    tuning.input.focus_mode = InputFocusMode::Hover;
    let mut st = Halley::new_for_test(&dh, tuning);

    let node_id = st.model.field.spawn_surface(
        "surface",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(node_id, "monitor_a");

    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(node_id)
    );
}

#[test]
fn hover_focus_mode_focuses_hovered_collapsed_surface_node() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut tuning = single_monitor_tuning();
    tuning.input.focus_mode = InputFocusMode::Hover;
    let mut st = Halley::new_for_test(&dh, tuning);

    let active = st.model.field.spawn_surface(
        "active",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let collapsed = st.model.field.spawn_surface(
        "collapsed",
        halley_core::field::Vec2 { x: 500.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(active, "monitor_a");
    st.assign_node_to_monitor(collapsed, "monitor_a");
    st.model
        .field
        .set_state(collapsed, halley_core::field::NodeState::Node);
    st.set_interaction_focus(Some(active), 30_000, Instant::now());

    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id: collapsed,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(collapsed)
    );
    assert_eq!(
        st.last_focused_surface_node_for_monitor("monitor_a"),
        Some(collapsed)
    );
}

#[test]
fn hover_focus_mode_focuses_hovered_core_node() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut tuning = single_monitor_tuning();
    tuning.input.focus_mode = InputFocusMode::Hover;
    let mut st = Halley::new_for_test(&dh, tuning);

    let first = st.model.field.spawn_surface(
        "first",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let second = st.model.field.spawn_surface(
        "second",
        halley_core::field::Vec2 { x: 500.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(first, "monitor_a");
    st.assign_node_to_monitor(second, "monitor_a");
    let cluster = st.create_cluster(vec![first, second]).expect("cluster");
    let core = st.collapse_cluster(cluster).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id: core,
            move_surface: false,
            is_core: true,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));
    assert_eq!(st.focused_node_for_monitor("monitor_a"), Some(core));
}

#[test]
fn click_focus_mode_keeps_hover_focus_disabled() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let node_id = st.model.field.spawn_surface(
        "surface",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(node_id, "monitor_a");

    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, None);
}

#[test]
fn hover_focus_gate_disables_focus_follows_mouse_while_layer_shell_is_active() {
    assert!(!super::focus::hover_focus_enabled(
        InputFocusMode::Hover,
        false,
        true
    ));
    assert!(super::focus::hover_focus_enabled(
        InputFocusMode::Hover,
        false,
        false
    ));
}

#[test]
fn hover_focus_mode_works_for_tiled_cluster_members() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut tuning = single_monitor_tuning();
    tuning.input.focus_mode = InputFocusMode::Hover;
    tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
    let mut st = Halley::new_for_test(&dh, tuning);

    let a = st.model.field.spawn_surface(
        "A",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let b = st.model.field.spawn_surface(
        "B",
        halley_core::field::Vec2 { x: 120.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [a, b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }

    let cid = st.create_cluster(vec![a, b]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    assert!(st.toggle_cluster_workspace_by_core(core, Instant::now()));

    // Hover B
    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id: b,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(b));

    // Hover A
    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id: a,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(a));
}

#[test]
fn hover_focus_mode_only_focuses_top_of_stack_in_clusters() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut tuning = single_monitor_tuning();
    tuning.input.focus_mode = InputFocusMode::Hover;
    tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
    let mut st = Halley::new_for_test(&dh, tuning);

    let a = st.model.field.spawn_surface(
        "A",
        halley_core::field::Vec2 { x: 100.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    let b = st.model.field.spawn_surface(
        "B",
        halley_core::field::Vec2 { x: 120.0, y: 100.0 },
        halley_core::field::Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [a, b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }

    let cid = st.create_cluster(vec![a, b]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    assert!(st.toggle_cluster_workspace_by_core(core, Instant::now()));

    // A is front (members[0])
    // Hover B (peeked back member)
    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id: b,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    // Should NOT focus B because it's in stack but not front
    assert_ne!(st.model.focus_state.primary_interaction_focus, Some(b));

    // Hover A (front member)
    super::focus::apply_hover_focus_mode(
        &mut st,
        Some(HitNode {
            node_id: a,
            move_surface: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    // Should focus A because it's front of stack
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(a));
}
