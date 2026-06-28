use super::*;
use crate::compositor::actions::window::toggle_focused_active_node_state;
use halley_core::field::{Vec2, Visibility};
use smithay::reexports::wayland_server::Display;

fn single_monitor_tuning() -> halley_config::RuntimeTuning {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
    tuning.tile_gaps_outer_px = 20.0;
    tuning.tile_gaps_inner_px = 20.0;
    tuning.decorations.border.size_px = 0;
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

fn dual_monitor_tuning() -> halley_config::RuntimeTuning {
    let mut tuning = halley_config::RuntimeTuning::default();
    tuning.tty_viewports = vec![
        halley_config::ViewportOutputConfig {
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
        },
        halley_config::ViewportOutputConfig {
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
        },
    ];
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

fn create_named_test_cluster(
    st: &mut Halley,
    monitor: &str,
    labels: [&str; 2],
    x: f32,
) -> (ClusterId, NodeId) {
    let a =
        st.model
            .field
            .spawn_surface(labels[0], Vec2 { x, y: 120.0 }, Vec2 { x: 240.0, y: 180.0 });
    let b = st.model.field.spawn_surface(
        labels[1],
        Vec2 {
            x: x + 260.0,
            y: 120.0,
        },
        Vec2 { x: 240.0, y: 180.0 },
    );
    st.assign_node_to_monitor(a, monitor);
    st.assign_node_to_monitor(b, monitor);
    let cid = st.create_cluster(vec![a, b]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, monitor);
    let _ = super::ensure_cluster_name_record_for_monitor(&mut *st, cid, monitor);
    (cid, core)
}

fn core_pos(st: &Halley, core: NodeId) -> Vec2 {
    st.model.field.node(core).expect("core").pos
}

#[test]
fn generic_cluster_names_are_monitor_local_reclaimable_and_moveable() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, dual_monitor_tuning());

    let (cid_a1, core_a1) = create_named_test_cluster(&mut st, "monitor_a", ["a1", "a2"], 120.0);
    let (_cid_a2, core_a2) = create_named_test_cluster(&mut st, "monitor_a", ["a3", "a4"], 520.0);
    assert_eq!(
        st.model.field.node(core_a1).expect("core a1").label,
        "Cluster 1"
    );
    assert_eq!(
        st.model.field.node(core_a2).expect("core a2").label,
        "Cluster 2"
    );

    let (_cid_b1, core_b1) = create_named_test_cluster(&mut st, "monitor_b", ["b1", "b2"], 920.0);
    let (_cid_b2, core_b2) = create_named_test_cluster(&mut st, "monitor_b", ["b3", "b4"], 1320.0);
    assert_eq!(
        st.model.field.node(core_b1).expect("core b1").label,
        "Cluster 1"
    );
    assert_eq!(
        st.model.field.node(core_b2).expect("core b2").label,
        "Cluster 2"
    );

    let members_to_remove = st
        .model
        .field
        .cluster(cid_a1)
        .expect("cluster a1")
        .members()
        .to_vec();
    for member_to_remove in members_to_remove {
        let _ = st.remove_node_from_field(member_to_remove, st.now_ms(Instant::now()));
    }

    let (_cid_a3, core_a3) = create_named_test_cluster(&mut st, "monitor_a", ["a5", "a6"], 220.0);
    assert_eq!(
        st.model.field.node(core_a3).expect("core a3").label,
        "Cluster 1"
    );

    st.assign_node_to_monitor(core_a3, "monitor_b");
    let _ = st.sync_cluster_monitor(
        st.model
            .field
            .cluster_id_for_core_public(core_a3)
            .expect("cluster for moved core"),
        Some("monitor_b"),
    );
    assert_eq!(
        st.model
            .field
            .node(core_a3)
            .expect("moved generic core")
            .label,
        "Cluster 3"
    );
}

#[test]
fn cluster_slot_order_is_creation_order_per_monitor() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, dual_monitor_tuning());

    let (cid_a1, _core_a1) = create_named_test_cluster(&mut st, "monitor_a", ["a1", "a2"], 120.0);
    let (cid_a2, _core_a2) = create_named_test_cluster(&mut st, "monitor_a", ["a3", "a4"], 520.0);
    let (cid_b1, _core_b1) = create_named_test_cluster(&mut st, "monitor_b", ["b1", "b2"], 920.0);

    assert_eq!(
        super::cluster_slot_order_for_monitor(&st, "monitor_a"),
        vec![cid_a1, cid_a2]
    );
    assert_eq!(
        super::cluster_slot_order_for_monitor(&st, "monitor_b"),
        vec![cid_b1]
    );
}

#[test]
fn cluster_slot_action_pans_then_opens_target_cluster() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let (cid, core) = create_named_test_cluster(&mut st, "monitor_a", ["a1", "a2"], 120.0);
    st.model.viewport.center = Vec2 { x: 700.0, y: 300.0 };
    st.model.camera_target_center = st.model.viewport.center;

    assert!(st.activate_cluster_slot_on_current_monitor(1, Instant::now()));
    assert_eq!(st.active_cluster_workspace_for_monitor("monitor_a"), None);
    assert!(
        st.model
            .cluster_state
            .pending_cluster_slot_transition
            .contains_key("monitor_a")
    );

    st.input.interaction_state.viewport_pan_anim = None;
    st.model.viewport.center = core_pos(&st, core);
    st.model.camera_target_center = st.model.viewport.center;
    assert!(st.process_pending_cluster_slot_transition_for_current_monitor(Instant::now()));
    assert_eq!(
        st.active_cluster_workspace_for_monitor("monitor_a"),
        Some(cid)
    );
}

#[test]
fn cluster_slot_action_switches_between_clusters_with_collapse_then_open() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let (_cid_a, core_a) = create_named_test_cluster(&mut st, "monitor_a", ["a1", "a2"], 120.0);
    let (cid_b, core_b) = create_named_test_cluster(&mut st, "monitor_a", ["b1", "b2"], 820.0);

    st.model.viewport.center = core_pos(&st, core_a);
    st.model.camera_target_center = st.model.viewport.center;
    assert!(st.activate_cluster_slot_on_current_monitor(1, Instant::now()));
    assert_eq!(
        st.active_cluster_workspace_for_monitor("monitor_a")
            .is_some(),
        true
    );

    assert!(st.activate_cluster_slot_on_current_monitor(2, Instant::now()));
    assert_eq!(st.active_cluster_workspace_for_monitor("monitor_a"), None);
    assert!(
        st.model
            .cluster_state
            .pending_cluster_slot_transition
            .contains_key("monitor_a")
    );

    st.input.interaction_state.viewport_pan_anim = None;
    st.model.viewport.center = core_pos(&st, core_b);
    st.model.camera_target_center = st.model.viewport.center;
    assert!(st.process_pending_cluster_slot_transition_for_current_monitor(Instant::now()));
    assert_eq!(
        st.active_cluster_workspace_for_monitor("monitor_a"),
        Some(cid_b)
    );
}

#[test]
fn cluster_slot_action_toggles_same_cluster_back_to_core() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let (_cid, core) = create_named_test_cluster(&mut st, "monitor_a", ["a1", "a2"], 120.0);
    st.model.viewport.center = core_pos(&st, core);
    st.model.camera_target_center = st.model.viewport.center;

    assert!(st.activate_cluster_slot_on_current_monitor(1, Instant::now()));
    assert!(
        st.active_cluster_workspace_for_monitor("monitor_a")
            .is_some()
    );
    assert!(st.activate_cluster_slot_on_current_monitor(1, Instant::now()));
    assert_eq!(st.active_cluster_workspace_for_monitor("monitor_a"), None);
    assert_eq!(st.model.focus_state.primary_interaction_focus, Some(core));
}

#[test]
fn cluster_mode_confirm_opens_name_prompt_before_creating() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let now = Instant::now();

    let first = st.model.field.spawn_surface(
        "first",
        Vec2 { x: 160.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    let second = st.model.field.spawn_surface(
        "second",
        Vec2 { x: 460.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    st.assign_node_to_monitor(first, "monitor_a");
    st.assign_node_to_monitor(second, "monitor_a");

    assert!(st.enter_cluster_mode());
    assert!(st.toggle_cluster_mode_selection(first));
    assert!(st.toggle_cluster_mode_selection(second));
    assert!(crate::compositor::clusters::system::confirm_cluster_mode(
        &mut st, now
    ));
    assert!(
        st.model
            .cluster_state
            .cluster_name_prompt
            .contains_key("monitor_a")
    );
    assert!(st.model.field.cluster_id_for_member_public(first).is_none());
    assert!(
        st.model
            .field
            .cluster_id_for_member_public(second)
            .is_none()
    );

    assert!(super::cancel_cluster_name_prompt_for_monitor(
        &mut st,
        "monitor_a"
    ));
    assert!(
        !st.model
            .cluster_state
            .cluster_name_prompt
            .contains_key("monitor_a")
    );
    assert!(st.cluster_mode_active_for_monitor("monitor_a"));
    assert_eq!(
        st.model
            .cluster_state
            .cluster_mode_selected_nodes
            .get("monitor_a")
            .map(|nodes| nodes.len()),
        Some(2)
    );

    assert!(st.exit_cluster_mode());
    assert!(!st.cluster_mode_active_for_monitor("monitor_a"));
}

#[test]
fn lift_finalize_prompt_selects_existing_matching_apps_without_banner() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let now = Instant::now();

    let first = st.model.field.spawn_surface(
        "Firefox",
        Vec2 { x: 160.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    let second = st.model.field.spawn_surface(
        "Firefox Settings",
        Vec2 { x: 460.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    st.assign_node_to_monitor(first, "monitor_a");
    st.assign_node_to_monitor(second, "monitor_a");
    st.model.node_app_ids.insert(first, "firefox".to_string());
    st.model
        .node_app_ids
        .insert(second, "org.mozilla.firefox".to_string());

    assert!(super::open_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        Some("Browser".to_string()),
        vec!["org.mozilla.firefox.desktop".to_string()],
        Vec::new(),
        Vec::new(),
        now,
    ));
    assert!(
        !st.ui
            .render_state
            .overlays
            .overlay_banner
            .contains_key("monitor_a")
    );
    assert_eq!(
        st.model
            .cluster_state
            .cluster_mode_selected_nodes
            .get("monitor_a")
            .map(|nodes| nodes.len()),
        Some(2)
    );

    assert!(super::confirm_cluster_name_prompt_for_monitor(
        &mut st,
        "monitor_a",
        now,
    ));
    assert_eq!(st.model.field.cluster_ids().len(), 1);
    assert!(
        !st.model
            .cluster_state
            .cluster_name_prompt
            .contains_key("monitor_a")
    );
}

#[test]
fn lift_finalize_cross_monitor_selection_centers_core_on_target_monitor() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, dual_monitor_tuning());
    let now = Instant::now();

    let first = st.model.field.spawn_surface(
        "Left A",
        Vec2 { x: 120.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    let second = st.model.field.spawn_surface(
        "Left B",
        Vec2 { x: 360.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    st.assign_node_to_monitor(first, "monitor_a");
    st.assign_node_to_monitor(second, "monitor_a");

    assert!(super::open_lift_cluster_finalize_draft(
        &mut st,
        "monitor_b",
        Some("Cross".to_string()),
        Vec::new(),
        Vec::new(),
        vec![first, second],
        now,
    ));
    assert!(super::confirm_cluster_name_prompt_for_monitor(
        &mut st,
        "monitor_b",
        now,
    ));

    let cid = st
        .model
        .field
        .cluster_ids()
        .into_iter()
        .next()
        .expect("cluster");
    let core = st
        .model
        .field
        .cluster(cid)
        .and_then(|cluster| cluster.core)
        .expect("core");
    let target = st.view_center_for_monitor("monitor_b");
    let pos = core_pos(&st, core);

    assert_close(pos.x, target.x);
    assert_close(pos.y, target.y);
    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&core)
            .map(String::as_str),
        Some("monitor_b")
    );
    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&first)
            .map(String::as_str),
        Some("monitor_b")
    );
    assert_eq!(
        st.model
            .monitor_state
            .node_monitor
            .get(&second)
            .map(String::as_str),
        Some("monitor_b")
    );
}

#[test]
fn lift_finalize_launches_after_confirm_and_absorbs_matching_windows() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let now = Instant::now();

    assert!(super::open_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        Some("Work".to_string()),
        vec!["alpha.desktop".to_string(), "beta.desktop".to_string()],
        vec![
            crate::compositor::clusters::state::ClusterFinalizeAppLaunch {
                app_id: "alpha.desktop".to_string(),
                command: "true".to_string(),
            },
            crate::compositor::clusters::state::ClusterFinalizeAppLaunch {
                app_id: "beta.desktop".to_string(),
                command: "true".to_string(),
            },
        ],
        Vec::new(),
        now,
    ));
    assert!(super::confirm_cluster_name_prompt_for_monitor(
        &mut st,
        "monitor_a",
        now,
    ));
    assert!(
        st.model
            .cluster_state
            .pending_lift_cluster_builds
            .contains_key("monitor_a")
    );
    assert_eq!(st.model.field.cluster_ids().len(), 0);

    let alpha = st.model.field.spawn_surface(
        "Alpha",
        Vec2 { x: 160.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    let beta = st.model.field.spawn_surface(
        "Beta",
        Vec2 { x: 460.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    st.assign_node_to_monitor(alpha, "monitor_a");
    st.assign_node_to_monitor(beta, "monitor_a");

    assert!(super::note_pending_lift_cluster_candidate_node(
        &mut st,
        "monitor_a",
        alpha
    ));
    assert!(super::pending_lift_cluster_node_staged(&st, alpha));
    st.model.spawn_state.pending_initial_reveal.insert(alpha);
    assert!(super::maybe_add_node_to_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        alpha,
        "alpha",
    ));
    assert_eq!(st.model.field.cluster_ids().len(), 0);
    assert!(
        st.model
            .field
            .node(alpha)
            .is_some_and(|node| node.visibility.has(Visibility::DETACHED))
    );
    assert!(!st.model.spawn_state.pending_initial_reveal.contains(&alpha));
    assert!(super::note_pending_lift_cluster_candidate_node(
        &mut st,
        "monitor_a",
        beta
    ));
    assert!(super::pending_lift_cluster_node_staged(&st, beta));
    st.model.spawn_state.pending_initial_reveal.insert(beta);
    assert!(super::maybe_add_node_to_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        beta,
        "beta",
    ));

    assert_eq!(st.model.field.cluster_ids().len(), 1);
    assert!(
        !st.model
            .cluster_state
            .pending_lift_cluster_builds
            .contains_key("monitor_a")
    );
    assert!(st.model.field.cluster_id_for_member_public(alpha).is_some());
    assert!(st.model.field.cluster_id_for_member_public(beta).is_some());
    assert!(!super::pending_lift_cluster_node_staged(&st, alpha));
    assert!(!super::pending_lift_cluster_node_staged(&st, beta));
    assert!(!st.model.spawn_state.pending_initial_reveal.contains(&beta));
    for member in [alpha, beta] {
        assert!(st.model.field.node(member).is_some_and(|node| {
            node.visibility.has(Visibility::HIDDEN_BY_CLUSTER)
                && !node.visibility.has(Visibility::DETACHED)
        }));
        assert!(!st.model.field.is_visible(member));
    }
    let cid = st.model.field.cluster_ids()[0];
    let core = st.model.field.cluster(cid).and_then(|cluster| cluster.core);
    assert!(core.is_some_and(|core| st.model.field.is_visible(core)));
}

#[test]
fn lift_finalize_app_launches_do_not_select_existing_matching_windows() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let now = Instant::now();

    let alpha = st.model.field.spawn_surface(
        "Alpha",
        Vec2 { x: 160.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    let beta = st.model.field.spawn_surface(
        "Beta",
        Vec2 { x: 460.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    st.assign_node_to_monitor(alpha, "monitor_a");
    st.assign_node_to_monitor(beta, "monitor_a");
    st.model.node_app_ids.insert(alpha, "alpha".to_string());
    st.model.node_app_ids.insert(beta, "beta".to_string());

    assert!(super::open_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        Some("Work".to_string()),
        Vec::new(),
        vec![
            crate::compositor::clusters::state::ClusterFinalizeAppLaunch {
                app_id: "alpha.desktop".to_string(),
                command: "true".to_string(),
            },
            crate::compositor::clusters::state::ClusterFinalizeAppLaunch {
                app_id: "beta.desktop".to_string(),
                command: "true".to_string(),
            },
        ],
        Vec::new(),
        now,
    ));
    assert!(super::confirm_cluster_name_prompt_for_monitor(
        &mut st,
        "monitor_a",
        now,
    ));
    assert_eq!(st.model.field.cluster_ids().len(), 0);
    assert!(
        st.model
            .cluster_state
            .pending_lift_cluster_builds
            .contains_key("monitor_a")
    );
    assert!(st.model.field.cluster_id_for_member_public(alpha).is_none());
    assert!(st.model.field.cluster_id_for_member_public(beta).is_none());
    assert!(!super::maybe_add_node_to_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        alpha,
        "alpha",
    ));
    assert!(st.model.field.cluster_id_for_member_public(alpha).is_none());
    assert_eq!(
        st.ui
            .render_state
            .overlays
            .overlay_toast
            .get("monitor_a")
            .and_then(|toast| toast.message.as_deref()),
        None
    );
}

#[test]
fn lift_finalize_releases_non_matching_staged_candidate() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());
    let now = Instant::now();

    assert!(super::open_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        Some("Work".to_string()),
        Vec::new(),
        vec![
            crate::compositor::clusters::state::ClusterFinalizeAppLaunch {
                app_id: "alpha.desktop".to_string(),
                command: "true".to_string(),
            },
        ],
        Vec::new(),
        now,
    ));
    assert!(super::confirm_cluster_name_prompt_for_monitor(
        &mut st,
        "monitor_a",
        now,
    ));

    let candidate = st.model.field.spawn_surface(
        "Gamma",
        Vec2 { x: 160.0, y: 160.0 },
        Vec2 { x: 220.0, y: 160.0 },
    );
    st.assign_node_to_monitor(candidate, "monitor_a");
    assert!(super::note_pending_lift_cluster_candidate_node(
        &mut st,
        "monitor_a",
        candidate
    ));
    assert!(super::pending_lift_cluster_node_staged(&st, candidate));

    assert!(!super::maybe_add_node_to_lift_cluster_finalize_draft(
        &mut st,
        "monitor_a",
        candidate,
        "gamma",
    ));
    assert!(!super::pending_lift_cluster_node_staged(&st, candidate));
    assert!(
        st.model
            .field
            .cluster_id_for_member_public(candidate)
            .is_none()
    );
}

#[test]
fn custom_cluster_name_stays_unique_and_survives_monitor_move() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, dual_monitor_tuning());
    let (cid, core) = create_named_test_cluster(&mut st, "monitor_a", ["x", "y"], 120.0);
    st.model.cluster_state.cluster_names.insert(
        cid,
        crate::compositor::clusters::state::ClusterNameRecord::Custom {
            name: "Studio".to_string(),
        },
    );
    let _ = super::relabel_cluster_core(&mut st, cid);
    st.assign_node_to_monitor(core, "monitor_b");
    let _ = st.sync_cluster_monitor(cid, Some("monitor_b"));
    assert_eq!(
        st.model.field.node(core).expect("custom core").label,
        "Studio"
    );
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

    let cid = st.create_cluster(vec![n1, n2]).expect("cluster");

    let core_id = st.collapse_cluster(cid).expect("core");
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

    let cid = st.create_cluster(vec![n1, n2]).expect("cluster");
    let core_id = st.collapse_cluster(cid).expect("core");
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));

    let removed =
        super::detach_member_from_cluster(&mut st, cid, stack_a, Vec2 { x: 0.0, y: 0.0 }, now);
    assert!(removed);
    st.layout_active_cluster_workspace_for_monitor("monitor_a", st.now_ms(now));
    assert!(st.focus_active_tiled_cluster_member_for_monitor("monitor_a", Some(1), now));
    assert_eq!(
        st.model.focus_state.primary_interaction_focus,
        Some(stack_b)
    );
}

// A growing tile (e.g. a slave promoted to master when the master closes) holds at
// its old slot until the client commits the bigger buffer — moving the footprint
// past the still-small capture would upscale it. Once the buffer lands it morphs
// from the old size up to the new size in one continuous track (no placeholder).
#[test]
fn tiled_cluster_reflow_grow_holds_then_morphs_from_old_size() {
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
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
    for id in [master, stack_a, stack_b] {
        st.ui.render_state.remove_cluster_tile_track(id);
    }

    let old_rect = st
        .active_cluster_tile_rect_for_member("monitor_a", stack_a)
        .expect("old stack rect");
    let old_visual = crate::animation::cluster_tile_rect_from_field(&st.model.field, stack_a)
        .expect("old visual stack rect");
    st.ui.render_state.cache.window_geometry.insert(
        stack_a,
        (0.0, 0.0, old_rect.w, old_rect.h),
    );
    st.ui.render_state.cache.window_geometry.insert(
        stack_b,
        (0.0, 0.0, old_rect.w, old_rect.h),
    );

    assert!(super::detach_member_from_cluster(
        &mut st,
        cid,
        master,
        Vec2 { x: 0.0, y: 0.0 },
        now,
    ));
    st.layout_active_cluster_workspace_for_monitor("monitor_a", st.now_ms(now));
    let target_size = st
        .model
        .field
        .node(stack_a)
        .expect("promoted stack node")
        .intrinsic_size;
    assert!(target_size.y > old_rect.h + 1.0);

    // Phase 1: the grow is held at the old slot (a hold track whose target is still
    // the old size) while it waits for the bigger committed buffer.
    let held_target = crate::animation::cluster_tile_target_rect(
        st.ui.render_state.cluster_tile_tracks(),
        stack_a,
    )
    .expect("grow should hold a tile track while waiting");
    assert!(
        (held_target.h - old_visual.size.y).abs() <= 0.5,
        "held grow target {} should still be the old size {} (waiting for buffer)",
        held_target.h,
        old_visual.size.y,
    );

    // Commit the bigger buffer, then run the layout again: the hold releases into a
    // real morph that targets the new master size but starts from the old size.
    st.ui.render_state.cache.window_geometry.insert(
        stack_a,
        (0.0, 0.0, target_size.x, target_size.y),
    );
    let stack_b_target = st
        .model
        .field
        .node(stack_b)
        .expect("remaining stack node")
        .intrinsic_size;
    st.ui.render_state.cache.window_geometry.insert(
        stack_b,
        (0.0, 0.0, stack_b_target.x, stack_b_target.y),
    );
    st.layout_active_cluster_workspace_for_monitor("monitor_a", st.now_ms(now));

    let morph_target = crate::animation::cluster_tile_target_rect(
        st.ui.render_state.cluster_tile_tracks(),
        stack_a,
    )
    .expect("grow should morph after commit");
    assert!(
        (morph_target.h - target_size.y).abs() <= 0.5,
        "morph target {} should be the promoted master size {}",
        morph_target.h,
        target_size.y,
    );
    let started = crate::animation::cluster_tile_rect_for(
        st.ui.render_state.cluster_tile_tracks(),
        stack_a,
        Instant::now(),
    )
    .expect("started grow track");
    assert!(
        started.size.y < target_size.y - 1.0,
        "grow should start from the old size {} and morph up to {}, got {}",
        old_visual.size.y,
        target_size.y,
        started.size.y,
    );
}

// A shrinking tile (a slave making room for a newly added window) moves immediately
// — no wait — morphing the footprint from the old (bigger) size down to the new
// size. The render path holds the old bigger capture and downscales it, so the size
// can animate smoothly without ever upscaling.
#[test]
fn tiled_cluster_reflow_shrink_morphs_from_old_size_immediately() {
    let dh = Display::<Halley>::new().expect("display").handle();
    let mut st = Halley::new_for_test(&dh, single_monitor_tuning());

    let master = st.model.field.spawn_surface(
        "master",
        Vec2 { x: 100.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let slave_a = st.model.field.spawn_surface(
        "slave-a",
        Vec2 { x: 500.0, y: 100.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    let slave_b = st.model.field.spawn_surface(
        "slave-b",
        Vec2 { x: 500.0, y: 400.0 },
        Vec2 { x: 320.0, y: 240.0 },
    );
    for id in [master, slave_a, slave_b] {
        st.assign_node_to_monitor(id, "monitor_a");
    }
    let cid = st
        .create_cluster(vec![master, slave_a])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");
    let now = Instant::now();
    assert!(st.enter_cluster_workspace_by_core(core, "monitor_a", now));
    for id in [master, slave_a] {
        st.ui.render_state.remove_cluster_tile_track(id);
    }

    // slave_a currently owns the full stack column. Mark both members' committed
    // surface sizes as matching their current slots so nothing is mid-resize yet.
    let slave_a_full = st
        .active_cluster_tile_rect_for_member("monitor_a", slave_a)
        .expect("full-column slave rect");
    let master_rect = st
        .active_cluster_tile_rect_for_member("monitor_a", master)
        .expect("master rect");
    st.ui.render_state.cache.window_geometry.insert(
        slave_a,
        (0.0, 0.0, slave_a_full.w, slave_a_full.h),
    );
    st.ui.render_state.cache.window_geometry.insert(
        master,
        (0.0, 0.0, master_rect.w, master_rect.h),
    );

    // Add a third window: slave_a now shares the column and must shrink.
    assert!(super::absorb_node_into_cluster(&mut st, cid, slave_b, now));
    st.layout_active_cluster_workspace_for_monitor("monitor_a", st.now_ms(now));

    let target = st
        .model
        .field
        .node(slave_a)
        .expect("shrunk slave node")
        .intrinsic_size;
    assert!(
        target.y < slave_a_full.h - 1.0,
        "slave_a should shrink (full {} -> target {})",
        slave_a_full.h,
        target.y,
    );

    // No wait: the shrink morphs immediately. The track targets the new (smaller)
    // size but starts from the old (bigger) size, animating down.
    let morph_target = crate::animation::cluster_tile_target_rect(
        st.ui.render_state.cluster_tile_tracks(),
        slave_a,
    )
    .expect("shrink should morph immediately");
    assert!(
        (morph_target.h - target.y).abs() <= 0.5,
        "shrink morph target {} should be the new smaller size {}",
        morph_target.h,
        target.y,
    );
    let started = crate::animation::cluster_tile_rect_for(
        st.ui.render_state.cluster_tile_tracks(),
        slave_a,
        Instant::now(),
    )
    .expect("started shrink track");
    assert!(
        started.size.y > target.y + 1.0,
        "shrink should start from the old size {} and morph down to {}, got {}",
        slave_a_full.h,
        target.y,
        started.size.y,
    );
    // The old geometry is pinned so the held bigger capture's crop stays correct
    // while the footprint shrinks.
    assert!(
        st.ui
            .render_state
            .cluster_tile_frozen_geometry(slave_a)
            .is_some(),
        "shrink should pin the old frozen geometry for the held capture"
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
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
        .create_cluster(vec![master, stack_a, stack_b])
        .expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
    st.assign_node_to_monitor(core, "monitor_a");

    assert!(
        crate::compositor::clusters::system::open_cluster_bloom_for_monitor(
            &mut st,
            "monitor_a",
            cid
        )
    );
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
    let cid = st.create_cluster(vec![master, stack]).expect("cluster");
    let core = st.collapse_cluster(cid).expect("core");
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
