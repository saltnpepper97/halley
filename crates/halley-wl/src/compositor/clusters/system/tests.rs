use super::*;
use crate::compositor::actions::window::toggle_focused_active_node_state;
use halley_core::field::Vec2;
use smithay::reexports::wayland_server::Display;

fn single_monitor_tuning() -> halley_config::RuntimeTuning {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
    tuning.tile_gaps_outer_px = 20.0;
    tuning.tile_gaps_inner_px = 20.0;
    tuning.border_size_px = 0;
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

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.5,
        "expected {expected}, got {actual}"
    );
}

fn node_edges(st: &Halley, id: NodeId) -> (f32, f32, f32, f32) {
    let node = st.model.field.node(id).expect("node");
    let half_w = node.intrinsic_size.x * 0.5;
    let half_h = node.intrinsic_size.y * 0.5;
    (
        node.pos.x - half_w,
        node.pos.y - half_h,
        node.pos.x + half_w,
        node.pos.y + half_h,
    )
}

#[test]
fn test_cluster_monitor_transfer_reopen() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.tty_viewports = vec![
        halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 1920,
            height: 1080,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        },
        halley_config::ViewportOutputConfig {
            connector: "monitor_b".to_string(),
            enabled: true,
            offset_x: 1920,
            offset_y: 0,
            width: 1920,
            height: 1080,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        },
    ];

    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, tuning);

    let n1 = st.model.field.spawn_surface(
        "monitor_a",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 400.0, y: 300.0 },
    );
    let n2 = st.model.field.spawn_surface(
        "monitor_a",
        Vec2 { x: 600.0, y: 100.0 },
        Vec2 { x: 400.0, y: 300.0 },
    );
    st.assign_node_to_monitor(n1, "monitor_a");
    st.assign_node_to_monitor(n2, "monitor_a");

    let cid = st
        .model
        .field
        .create_cluster(vec![n1, n2])
        .expect("cluster");

    let core_id = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core_id, "monitor_a");

    st.assign_node_to_monitor(core_id, "monitor_b");
    let _ = st.model.field.carry(
        core_id,
        Vec2 {
            x: 1920.0 + 500.0,
            y: 500.0,
        },
    );

    let now = Instant::now();
    st.focus_monitor_view("monitor_b", now);
    let success = st.enter_cluster_workspace_by_core(core_id, "monitor_b", now);
    assert!(success);

    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&n1)
            .map(|s| s.as_str()),
        Some("monitor_b")
    );
    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&n2)
            .map(|s| s.as_str()),
        Some("monitor_b")
    );

    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&core_id)
            .map(|s| s.as_str()),
        Some("monitor_b")
    );
}

#[test]
fn test_cluster_monitor_maintenance_sync() {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.tty_viewports = vec![
        halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 1920,
            height: 1080,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        },
        halley_config::ViewportOutputConfig {
            connector: "monitor_b".to_string(),
            enabled: true,
            offset_x: 1920,
            offset_y: 0,
            width: 1920,
            height: 1080,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        },
    ];

    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, tuning);

    let n1 = st.model.field.spawn_surface(
        "monitor_a",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 400.0, y: 300.0 },
    );
    let n2 = st.model.field.spawn_surface(
        "monitor_a",
        Vec2 { x: 600.0, y: 100.0 },
        Vec2 { x: 400.0, y: 300.0 },
    );
    st.assign_node_to_monitor(n1, "monitor_a");
    st.assign_node_to_monitor(n2, "monitor_a");

    let cid = st
        .model
        .field
        .create_cluster(vec![n1, n2])
        .expect("cluster");
    let core_id = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core_id, "monitor_a");

    st.assign_node_to_monitor(n1, "monitor_a");
    st.assign_node_to_monitor(n2, "monitor_a");
    st.assign_node_to_monitor(core_id, "monitor_b");

    let success = st.sync_cluster_monitor(cid, None);
    assert!(success);

    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&n1)
            .map(|s| s.as_str()),
        Some("monitor_b")
    );
    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&n2)
            .map(|s| s.as_str()),
        Some("monitor_b")
    );
    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&core_id)
            .map(|s| s.as_str()),
        Some("monitor_b")
    );
}

#[test]
fn entering_two_window_cluster_keeps_outer_gap_exact() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    let (master_left, master_top, master_right, master_bottom) = node_edges(&st, master);
    let (stack_left, stack_top, stack_right, stack_bottom) = node_edges(&st, stack);

    assert_close(master_left, 20.0);
    assert_close(master_top, 20.0);
    assert_close(master_bottom, 580.0);
    assert_close(stack_top, 20.0);
    assert_close(stack_bottom, 580.0);
    assert_close(stack_right, 780.0);
    assert_close(stack_left - master_right, 20.0);
}

#[test]
fn entering_three_window_cluster_keeps_master_outer_gap_exact() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_a = st.model.field.spawn_surface(
        "stack-a",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_b = st.model.field.spawn_surface(
        "stack-b",
        Vec2 { x: 500.0, y: 400.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack_a, stack_b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    let (_, master_top, master_right, master_bottom) = node_edges(&st, master);
    let mut stack_edges = [node_edges(&st, stack_a), node_edges(&st, stack_b)];
    stack_edges.sort_by(|a, b| a.1.partial_cmp(&b.1).expect("finite"));
    let upper = stack_edges[0];
    let lower = stack_edges[1];

    assert_close(master_top, 20.0);
    assert_close(master_bottom, 580.0);
    assert_close(upper.1, 20.0);
    assert_close(lower.3, 580.0);
    assert_close(lower.1 - upper.3, 20.0);
    assert_close(upper.0 - master_right, 20.0);
}

#[test]
fn entering_cluster_keeps_current_monitor_live_viewport_full_size() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let full_viewport = st.model.viewport;
    st.model
        .monitor_state
        .monitors
        .get_mut("monitor_a")
        .expect("monitor")
        .usable_viewport = halley_core::viewport::Viewport::new(
        Vec2 { x: 400.0, y: 320.0 },
        Vec2 { x: 800.0, y: 560.0 },
    );

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));
    assert_eq!(st.model.viewport, full_viewport);
    assert_eq!(st.model.camera_target_view_size, full_viewport.size);
}

#[test]
fn entering_tiled_cluster_workspace_focuses_master_tile() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", Instant::now()));
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(master));
}

#[test]
fn tiled_cluster_focus_retargets_replacement_tile_by_index() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_a = st.model.field.spawn_surface(
        "stack-a",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_b = st.model.field.spawn_surface(
        "stack-b",
        Vec2 { x: 500.0, y: 400.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack_a, stack_b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    let removed = cluster_system_controller(&mut st).detach_member_from_cluster(
        cid,
        stack_a,
        Vec2 { x: 0.0, y: 0.0 },
        now,
    );
    assert!(removed);
    st.layout_active_cluster_workspace_for_monitor("monitor_a", st.now_ms(now));
    assert!(st.focus_active_tiled_cluster_member_for_monitor("monitor_a", Some(1), now));
    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(stack_b)
    );
}

#[test]
fn tile_focus_moves_between_visible_neighbors() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_a = st.model.field.spawn_surface(
        "stack-a",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_b = st.model.field.spawn_surface(
        "stack-b",
        Vec2 { x: 500.0, y: 400.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack_a, stack_b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    assert!(st.tile_focus_active_cluster_member_for_monitor(
        "monitor_a",
        DirectionalAction::Right,
        now,
    ));
    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(stack_a)
    );
    assert!(st.tile_focus_active_cluster_member_for_monitor(
        "monitor_a",
        DirectionalAction::Down,
        now,
    ));
    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(stack_b)
    );
    assert!(st.tile_focus_active_cluster_member_for_monitor(
        "monitor_a",
        DirectionalAction::Left,
        now,
    ));
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(master));
}

#[test]
fn tile_swap_exchanges_adjacent_visible_tiles() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_a = st.model.field.spawn_surface(
        "stack-a",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_b = st.model.field.spawn_surface(
        "stack-b",
        Vec2 { x: 500.0, y: 400.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack_a, stack_b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
    st.set_interaction_focus(Some(stack_a), 30_000, now);

    let before_a = st
        .active_cluster_tile_rect_for_member("monitor_a", stack_a)
        .expect("stack a rect");
    let before_b = st
        .active_cluster_tile_rect_for_member("monitor_a", stack_b)
        .expect("stack b rect");

    assert!(st.tile_swap_active_cluster_member_for_monitor(
        "monitor_a",
        DirectionalAction::Down,
        now,
    ));
    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(stack_a)
    );

    let after_a = st
        .active_cluster_tile_rect_for_member("monitor_a", stack_a)
        .expect("stack a rect after swap");
    let after_b = st
        .active_cluster_tile_rect_for_member("monitor_a", stack_b)
        .expect("stack b rect after swap");
    assert!((after_a.y - before_b.y).abs() <= 0.5);
    assert!((after_b.y - before_a.y).abs() <= 0.5);
}

#[test]
fn cluster_layout_cycle_toggles_active_workspace_layout() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
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
    assert_eq!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Tiling
    );

    assert!(st.cycle_active_cluster_layout_for_monitor("monitor_a", now));
    assert_eq!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Stacking
    );

    assert!(st.cycle_active_cluster_layout_for_monitor("monitor_a", now));
    assert_eq!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Tiling
    );
}

#[test]
fn switching_from_tiling_to_stacking_focuses_front_stack_member() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_a = st.model.field.spawn_surface(
        "stack-a",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack_b = st.model.field.spawn_surface(
        "stack-b",
        Vec2 { x: 500.0, y: 400.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, stack_a, stack_b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    assert!(st.tile_focus_active_cluster_member_for_monitor(
        "monitor_a",
        DirectionalAction::Right,
        now,
    ));
    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(stack_a)
    );

    assert!(st.cycle_active_cluster_layout_for_monitor("monitor_a", now));
    assert_eq!(
        st.runtime.tuning.cluster_layout_kind(),
        ClusterWorkspaceLayoutKind::Stacking
    );
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(master));
}

#[test]
fn cluster_exit_restores_full_viewport_not_usable_viewport() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let full_viewport = st.model.viewport;
    let reduced_usable = halley_core::viewport::Viewport::new(
        Vec2 { x: 400.0, y: 320.0 },
        Vec2 { x: 800.0, y: 560.0 },
    );
    st.model
        .monitor_state
        .monitors
        .get_mut("monitor_a")
        .expect("monitor")
        .usable_viewport = reduced_usable;

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
    assert_eq!(
        st.model
            .cluster_state
            .workspace_prev_viewports
            .get("monitor_a"),
        Some(&full_viewport)
    );

    assert!(st.exit_cluster_workspace_for_monitor("monitor_a", now));
    assert_eq!(st.model.viewport, full_viewport);
    assert_eq!(
        st.model
            .monitor_state
            .monitors
            .get("monitor_a")
            .expect("monitor")
            .viewport,
        full_viewport
    );
    assert_eq!(
        st.model
            .monitor_state
            .monitors
            .get("monitor_a")
            .expect("monitor")
            .usable_viewport,
        reduced_usable
    );
}

#[test]
fn closing_cluster_bloom_refocuses_core() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    assert!(st.open_cluster_bloom_for_monitor("monitor_a", cid));
    st.set_interaction_focus(Some(master), 30_000, Instant::now());

    assert!(st.close_cluster_bloom_for_monitor("monitor_a"));
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));
    assert_eq!(st.focused_node_for_monitor("monitor_a"), Some(core));
}

#[test]
fn collapsing_cluster_workspace_keeps_core_focused() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
    assert!(st.collapse_active_cluster_workspace(now));

    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));
    assert_eq!(st.focused_node_for_monitor("monitor_a"), Some(core));
}

#[test]
fn toggle_state_reopens_cluster_from_focused_core() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let stack = st.model.field.spawn_surface(
        "stack",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    st.assign_node_to_monitor(master, "monitor_a");
    st.assign_node_to_monitor(stack, "monitor_a");
    let cid = st
        .model
        .field
        .create_cluster(vec![master, stack])
        .expect("cluster");
    let core = st.model.field.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
    assert!(st.collapse_active_cluster_workspace(now));
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));

    assert!(toggle_focused_active_node_state(&mut st));
    assert_eq!(
        st.active_cluster_workspace_for_monitor("monitor_a"),
        Some(cid)
    );
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(master));
}
