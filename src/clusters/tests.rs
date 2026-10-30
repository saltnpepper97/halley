use super::*;

#[test]
fn action_buttons_are_compact_stacked_and_flip_inside_the_output_edge() {
    let output = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let [(close, upper), (edit, lower)] = action_button_rects((100, 100).into(), output);
    assert_eq!(close, ClusterActionControl::Close);
    assert_eq!(edit, ClusterActionControl::Edit);
    assert_eq!(upper.size, (26, 26).into());
    assert_eq!(lower.size, (26, 26).into());
    assert!(upper.loc.x > 100);
    assert!(upper.loc.y < lower.loc.y);
    assert!(!upper.overlaps(lower));

    let [(_, upper), (_, lower)] = action_button_rects((980, 100).into(), output);
    assert!(upper.loc.x < 980);
    assert!(output.contains_rect(upper));
    assert!(output.contains_rect(lower));
}

fn test_placement_rect(
    placement: halley_core::cluster::layout::ClusterWorkspacePlacement,
) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (
            placement.rect.x.round() as i32,
            placement.rect.y.round() as i32,
        )
            .into(),
        (
            placement.rect.w.round().max(1.0) as i32,
            placement.rect.h.round().max(1.0) as i32,
        )
            .into(),
    )
}

fn active_test_cluster(
    member_count: usize,
    layout: ClusterWorkspaceLayoutKind,
) -> (Field, ClusterSystem, ClusterId, Vec<NodeId>) {
    let mut field = Field::new();
    let members = (0..member_count)
        .map(|index| {
            field.spawn_surface(
                format!("W{index}"),
                Vec2 {
                    x: index as f32 * 120.0,
                    y: 0.0,
                },
                Vec2 { x: 100.0, y: 80.0 },
            )
        })
        .collect::<Vec<_>>();
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    assert!(system.begin_creation("DP-1".into()));
    for member in &members {
        assert!(system.toggle_creation_member(*member, "DP-1"));
    }
    assert!(system.begin_naming());
    let cluster = system.finish_creation(&mut field).unwrap();
    system.metadata.get_mut(&cluster).unwrap().layout = layout;
    assert!(system.activate_slot("DP-1", 1, Duration::ZERO));
    (field, system, cluster, members)
}

#[test]
fn dissolution_preserves_members_and_clears_cluster_owned_state() {
    let (mut field, mut system, cluster, members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Tiling);
    let core = system.core_node(cluster).expect("synthetic core");
    let first_restore = Rectangle::new((80, 60).into(), (640, 480).into());
    let second_restore = Rectangle::new((760, 90).into(), (700, 520).into());
    assert!(system.prepare_surface_target(
        members[0],
        first_restore,
        Rectangle::new((0, 0).into(), (900, 700).into()),
    ));
    assert!(system.prepare_surface_target(
        members[1],
        second_restore,
        Rectangle::new((900, 0).into(), (700, 700).into()),
    ));
    assert_eq!(
        system.toggle_member_floating(
            "DP-1",
            members[0],
            Rectangle::new((0, 0).into(), (1_600, 900).into()),
            Rectangle::new((120, 100).into(), (600, 450).into()),
            Duration::from_secs(1),
        ),
        Some(true),
    );

    let outcome = system
        .dissolve_cluster(&mut field, cluster)
        .expect("dissolution");

    assert_eq!(outcome.output, "DP-1");
    assert_eq!(outcome.members, members);
    assert_eq!(
        outcome.surface_restores,
        vec![(members[0], first_restore), (members[1], second_restore)]
    );
    assert!(system.registry.cluster(cluster).is_none());
    assert!(system.metadata(cluster).is_none());
    assert!(system.clusters_for_output("DP-1").next().is_none());
    assert_eq!(system.active_on("DP-1"), None);
    assert!(!system.overflow.is_revealed("DP-1"));
    assert!(!system.is_animating_on_output("DP-1", Duration::ZERO));
    assert!(field.node(core).is_none());
    for member in outcome.members {
        assert!(!system.is_member(member));
        assert!(!system.member_floats.is_floating(member));
        assert!(field.is_visible(member));
    }
}

#[test]
fn presentation_workspace_returns_to_field_after_cluster_close_finishes() {
    let (_field, mut system, cluster, _members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Tiling);
    assert_eq!(
        crate::presentation::active_workspace_on_output(&system, "DP-1", Duration::ZERO),
        crate::presentation::PresentationWorkspace::Cluster(cluster)
    );

    let closing_at = Duration::from_secs(1);
    assert!(system.activate_slot("DP-1", 1, closing_at));
    assert_eq!(
        crate::presentation::active_workspace_on_output(&system, "DP-1", closing_at),
        crate::presentation::PresentationWorkspace::Cluster(cluster)
    );
    assert_eq!(
        crate::presentation::active_workspace_on_output(&system, "DP-1", Duration::from_secs(20),),
        crate::presentation::PresentationWorkspace::Field
    );
}

#[test]
fn member_float_preserves_membership_order_and_remembered_geometry() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let original_order = system.registry.cluster(cluster).unwrap().members().to_vec();
    let original_tile = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .find(|placement| placement.node_id == members[0])
        .map(test_placement_rect)
        .unwrap();
    let floating_rect = Rectangle::new((240, 160).into(), (460, 340).into());

    assert_eq!(
        system.toggle_member_floating(
            "DP-1",
            members[0],
            work_area,
            floating_rect,
            Duration::from_secs(2),
        ),
        Some(true)
    );
    assert!(system.is_member_floating(members[0]));
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        original_order
    );
    assert!(
        system
            .workspace_layout(cluster, work_area)
            .unwrap()
            .placements
            .iter()
            .all(|placement| placement.node_id != members[0])
    );
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(2),),
        WindowPresentation::Workspace {
            rect: original_tile,
            depth: usize::MAX,
            alpha: 1.0,
        }
    );
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(20),),
        WindowPresentation::Workspace {
            rect: floating_rect,
            depth: usize::MAX,
            alpha: 1.0,
        }
    );

    assert_eq!(
        system.toggle_member_floating(
            "DP-1",
            members[0],
            work_area,
            floating_rect,
            Duration::from_secs(20),
        ),
        Some(false)
    );
    assert!(!system.is_member_floating(members[0]));
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        original_order
    );
    let WindowPresentation::Workspace { rect, .. } =
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(20))
    else {
        panic!("retiling member should remain visible");
    };
    assert_eq!(rect, floating_rect);

    assert_eq!(
        system.toggle_member_floating(
            "DP-1",
            members[0],
            work_area,
            original_tile,
            Duration::from_secs(40),
        ),
        Some(true)
    );
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(60),),
        WindowPresentation::Workspace {
            rect: floating_rect,
            depth: usize::MAX,
            alpha: 1.0,
        }
    );
}

#[test]
fn floating_member_surface_target_is_the_remembered_windowed_rect() {
    let (_field, mut system, _cluster, members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let output_geometry = Rectangle::new((0, 0).into(), (1_920, 1_080).into());
    let floating = Rectangle::new((240, 160).into(), (460, 340).into());

    assert_eq!(
        system.toggle_member_floating("DP-1", members[0], work_area, floating, Duration::ZERO,),
        Some(true)
    );
    assert_eq!(
        system.workspace_surface_target_for(members[0], "DP-1", work_area, output_geometry),
        Some(surfaces::WorkspaceSurfaceTarget {
            node_id: members[0],
            geometry: floating,
        })
    );
}

#[test]
fn member_float_is_scoped_to_the_active_cluster() {
    let (_field, mut system, _cluster, members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let rect = Rectangle::new((100, 80).into(), (500, 400).into());

    assert_eq!(
        system.toggle_member_floating("DP-2", members[0], work_area, rect, Duration::ZERO,),
        None
    );
    assert!(!system.is_member_floating(members[0]));
}

#[test]
fn creation_is_single_modal_state() {
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    assert!(system.begin_creation("DP-1".into()));
    assert!(!system.begin_creation("DP-2".into()));
    assert_eq!(system.creation().unwrap().output, "DP-1");
    assert!(system.cancel_creation());
    assert!(system.creation().is_none());
}

#[test]
fn cluster_core_uses_the_old_larger_hit_area_and_hover_animation() {
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    let id = ClusterId::new(1);
    system.metadata.insert(
        id,
        ClusterMetadata {
            name: "Cluster 1".into(),
            output: "DP-1".into(),
            layout: ClusterWorkspaceLayoutKind::Tiling,
            core: None,
            core_position: Vec2 { x: 0.0, y: 0.0 },
        },
    );
    system.slots.insert("DP-1".into(), vec![id]);
    let camera = halley_core::camera::Camera::new(
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 {
            x: 1920.0,
            y: 1080.0,
        },
    );
    let output_geometry = Rectangle::new((0, 0).into(), (1920, 1080).into());

    assert_eq!(
        system.core_hit_test(
            "DP-1",
            &camera,
            output_geometry,
            (960.0 + CORE_DIAMETER_PX as f64 / 2.0 - 1.0, 540.0).into(),
        ),
        Some(id)
    );
    assert_eq!(
        system.core_hit_test(
            "DP-1",
            &camera,
            output_geometry,
            (960.0 + CORE_DIAMETER_PX as f64 / 2.0 + 1.0, 540.0).into(),
        ),
        None
    );
    assert!(system.set_hovered_core(Some(id), Duration::ZERO));
    assert!(system.labels_animating_on_output("DP-1", halley_config::NodeDisplayPolicy::Hover));
    assert!(system.label_hover_mix(id, true) > 0.0);
    system.active.insert("DP-1".into(), id);
    assert!(!system.labels_animating_on_output("DP-1", halley_config::NodeDisplayPolicy::Hover));
}

#[test]
fn collapsed_cluster_core_moves_as_one_cluster_owned_landmark() {
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    let id = ClusterId::new(1);
    system.metadata.insert(
        id,
        ClusterMetadata {
            name: "Cluster 1".into(),
            output: "DP-1".into(),
            layout: ClusterWorkspaceLayoutKind::Tiling,
            core: None,
            core_position: Vec2 { x: 10.0, y: 20.0 },
        },
    );
    system.slots.insert("DP-1".into(), vec![id]);

    assert!(system.move_core(id, "DP-1", Vec2 { x: 30.0, y: 40.0 }));
    assert_eq!(
        system.metadata(id).unwrap().core_position,
        Vec2 { x: 30.0, y: 40.0 }
    );

    assert!(system.move_core(id, "DP-2", Vec2 { x: 50.0, y: 60.0 }));
    assert_eq!(system.metadata(id).unwrap().output, "DP-2");
    assert_eq!(
        system
            .clusters_for_output("DP-1")
            .next()
            .map(|(_, id, _)| id),
        None
    );
    assert_eq!(
        system
            .clusters_for_output("DP-2")
            .next()
            .map(|(_, id, _)| id),
        Some(id)
    );

    system.active.insert("DP-2".into(), id);
    assert!(!system.move_core(id, "DP-1", Vec2 { x: 0.0, y: 0.0 }));
    assert_eq!(system.metadata(id).unwrap().output, "DP-2");
}

#[test]
fn persistent_pin_belongs_to_the_cluster_core_not_a_member() {
    let (mut field, mut system, cluster, members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Tiling);
    let core = system.core_node(cluster).expect("cluster core");

    assert!(system.set_core_pinned(&mut field, core, true));
    assert!(system.registry().cluster(cluster).unwrap().pinned);
    assert!(field.node(core).unwrap().pinned);
    assert!(
        members
            .iter()
            .all(|member| !field.node(*member).unwrap().pinned)
    );

    assert!(!system.set_core_pinned(&mut field, members[0], true));
    assert!(!field.node(members[0]).unwrap().pinned);
}

#[test]
fn destroyed_members_reflow_then_leave_a_reusable_empty_cluster() {
    let mut field = Field::new();
    let a = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
    let b = field.spawn_surface("B", Vec2 { x: 200.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    assert!(system.begin_creation("DP-1".into()));
    assert!(system.toggle_creation_member(a, "DP-1"));
    assert!(system.toggle_creation_member(b, "DP-1"));
    assert!(system.begin_naming());
    assert!(system.edit_name(NameInput::Character('W')));
    let id = system.finish_creation(&mut field).unwrap();
    system.metadata.get_mut(&id).unwrap().layout = ClusterWorkspaceLayoutKind::Tiling;
    assert!(system.activate_slot("DP-1", 1, Duration::ZERO));
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let before = system
        .workspace_layout(id, work_area)
        .unwrap()
        .placements
        .into_iter()
        .find(|placement| placement.node_id == b)
        .unwrap()
        .rect;
    let before = Rectangle::<i32, Logical>::new(
        (before.x.round() as i32, before.y.round() as i32).into(),
        (
            before.w.round().max(1.0) as i32,
            before.h.round().max(1.0) as i32,
        )
            .into(),
    );
    let started = Duration::from_secs(2);

    assert!(system.forget_destroyed_member_animated(&mut field, a, work_area, started));
    assert_eq!(system.registry().cluster(id).unwrap().members(), &[b]);
    assert_eq!(system.active_on("DP-1"), Some(id));
    assert!(system.is_animating_on_output("DP-1", started));
    assert_eq!(
        system.window_presentation(b, "DP-1", work_area, None, started),
        WindowPresentation::Workspace {
            rect: before,
            depth: 0,
            alpha: 1.0,
        }
    );
    let core = system.core_node(id).unwrap();
    assert_eq!(system.close_targets_for_node(core), vec![b]);

    assert!(system.forget_destroyed_member(&mut field, b));
    assert!(system.registry().cluster(id).unwrap().members().is_empty());
    assert!(system.metadata(id).is_some());
    assert_eq!(
        system
            .clusters_for_output("DP-1")
            .map(|(_, cluster, _)| cluster)
            .collect::<Vec<_>>(),
        vec![id]
    );
    assert_eq!(system.active_on("DP-1"), Some(id));
    assert_eq!(system.close_targets_for_node(core), Vec::<NodeId>::new());

    assert!(system.activate("DP-1", id, Duration::from_secs(3)));
    assert!(system.active_on("DP-1").is_none());
    assert_eq!(system.core_node(id), Some(core));
}

#[test]
fn closing_the_front_stack_card_advances_the_next_card_smoothly() {
    let (mut field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let old_second = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .find(|placement| placement.node_id == members[1])
        .map(test_placement_rect)
        .unwrap();
    let started = Duration::from_secs(2);

    assert!(system.forget_destroyed_member_animated(&mut field, members[0], work_area, started,));
    let target = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .find(|placement| placement.node_id == members[1])
        .map(test_placement_rect)
        .unwrap();
    assert_ne!(old_second, target);
    assert_eq!(
        system.window_presentation(members[1], "DP-1", work_area, None, started),
        WindowPresentation::Workspace {
            rect: old_second,
            depth: 1,
            alpha: 1.0,
        }
    );
    let halfway = started
        + Duration::from_millis(u64::from(system.animations.stacking.cycle_duration_ms / 2));
    let WindowPresentation::Workspace { rect, .. } =
        system.window_presentation(members[1], "DP-1", work_area, None, halfway)
    else {
        panic!("the promoted stack card should remain visible");
    };
    assert!(rect.loc.x < old_second.loc.x);
    assert!(rect.loc.x > target.loc.x);
}

#[test]
fn pulled_stack_card_rejoins_its_active_cluster_at_the_front() {
    let (mut field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let front = members[0];
    let now = Duration::from_secs(2);

    assert!(system.begin_workspace_drag(
        "DP-1",
        front,
        Rectangle::new((300, 200).into(), (500, 400).into()),
    ));
    assert!(system.detach_active_member_for_drag(
        &mut field,
        "DP-1",
        ClusterDragMember {
            cluster_id: cluster,
            node_id: front,
        },
        work_area,
        Vec2 { x: 500.0, y: 350.0 },
        now,
    ));
    assert_eq!(system.cluster_for_member(front), None);
    assert_eq!(system.member_ids(cluster), members[1..]);

    assert!(system.join_active_member_front(
        &mut field,
        "DP-1",
        front,
        work_area,
        Rectangle::new((300, 200).into(), (500, 400).into()),
        now + Duration::from_millis(10),
    ));
    assert_eq!(system.first_member(cluster), Some(front));
    assert_eq!(system.member_ids(cluster), members);
}

#[test]
fn restored_collapsed_node_joins_an_open_workspace_in_both_layouts() {
    for layout in [
        ClusterWorkspaceLayoutKind::Tiling,
        ClusterWorkspaceLayoutKind::Stacking,
    ] {
        let (mut field, mut system, cluster, _) = active_test_cluster(2, layout);
        let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
        let joining = field.spawn_surface(
            "joining",
            Vec2 { x: 500.0, y: 350.0 },
            Vec2 { x: 400.0, y: 300.0 },
        );
        assert!(field.set_state(joining, halley_core::field::NodeState::Node));

        // Session restores the real surface before active-workspace admission.
        assert!(field.touch(joining, 2_000));
        assert!(system.join_active_member_front(
            &mut field,
            "DP-1",
            joining,
            work_area,
            Rectangle::new((300, 200).into(), (400, 300).into()),
            Duration::from_secs(2),
        ));

        assert_eq!(
            field.node(joining).unwrap().state,
            halley_core::field::NodeState::Active
        );
        assert_eq!(system.cluster_for_member(joining), Some(cluster));
        assert_eq!(system.first_member(cluster), Some(joining));
        assert!(matches!(
            system.window_presentation(joining, "DP-1", work_area, None, Duration::from_secs(2)),
            WindowPresentation::Workspace { .. }
        ));
    }
}

#[test]
fn field_window_stays_visible_while_dragged_over_an_active_workspace() {
    let (mut field, mut system, _cluster, _members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let joining = field.spawn_surface(
        "joining",
        Vec2 { x: 500.0, y: 350.0 },
        Vec2 { x: 400.0, y: 300.0 },
    );

    assert_eq!(
        system.window_presentation(joining, "DP-1", work_area, None, Duration::ZERO),
        WindowPresentation::Hidden
    );
    assert!(system.begin_field_drag(joining));
    assert_eq!(
        system.window_presentation(joining, "DP-1", work_area, None, Duration::ZERO),
        WindowPresentation::Field
    );
    assert!(system.cancel_window_drag());
}

#[test]
fn provisional_stack_drag_does_not_dissolve_a_two_member_cluster() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let front = members[0];

    assert!(system.begin_workspace_drag(
        "DP-1",
        front,
        Rectangle::new((300, 200).into(), (500, 400).into()),
    ));
    assert_eq!(system.active_on("DP-1"), Some(cluster));
    assert_eq!(system.member_ids(cluster), members);
    assert!(system.finish_workspace_drag(
        "DP-1",
        front,
        work_area,
        Rectangle::new((300, 200).into(), (500, 400).into()),
        Duration::from_secs(2),
    ));
    assert_eq!(system.active_on("DP-1"), Some(cluster));
    assert_eq!(system.first_member(cluster), Some(front));
}

#[test]
fn workspace_drag_keeps_its_screen_size_across_outputs() {
    let (_field, mut system, _cluster, members) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let member = members[0];
    let original = Rectangle::new((140, 90).into(), (620, 510).into());
    let moved_location = Point::from((35, 55));

    assert!(system.begin_workspace_drag("DP-1", member, original));
    assert!(system.update_workspace_drag(member, "DP-2", moved_location));
    assert_eq!(
        system.window_presentation(member, "DP-1", work_area, None, Duration::ZERO),
        WindowPresentation::Hidden
    );
    assert_eq!(
        system.window_presentation(member, "DP-2", work_area, None, Duration::ZERO),
        WindowPresentation::PointerDrag {
            rect: Rectangle::new(moved_location, original.size),
        }
    );
}

#[test]
fn stacked_to_tiled_reflow_keeps_each_cards_original_depth_until_settled() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let started = Duration::from_secs(2);
    let old_depths = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .map(|placement| (placement.node_id, placement.depth))
        .collect::<HashMap<_, _>>();

    assert!(system.cycle_active_layout("DP-1", work_area, started));
    for member in members {
        let WindowPresentation::Workspace { depth, .. } =
            system.window_presentation(member, "DP-1", work_area, None, started)
        else {
            panic!("every transitioning member should remain presented");
        };
        assert_eq!(depth, old_depths[&member]);
    }
}

#[test]
fn tiled_to_stacked_reflow_uses_each_cards_destination_depth_immediately() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let started = Duration::from_secs(2);

    assert!(system.cycle_active_layout("DP-1", work_area, started));
    let target_depths = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .map(|placement| (placement.node_id, placement.depth))
        .collect::<HashMap<_, _>>();
    for member in members {
        let WindowPresentation::Workspace { depth, .. } =
            system.window_presentation(member, "DP-1", work_area, None, started)
        else {
            panic!("every transitioning member should remain presented");
        };
        assert_eq!(depth, target_depths[&member]);
    }
}

#[test]
fn cycling_a_single_card_stack_is_handled_without_animation() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(1, ClusterWorkspaceLayoutKind::Stacking);
    let member = members[0];
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());

    assert_eq!(
        system.cycle_stack(
            "DP-1",
            halley_config::FocusCycleDirection::Forward,
            work_area,
            Duration::from_secs(1),
        ),
        StackCycleOutcome::Unchanged
    );
    assert!(!system.reflows.contains_key("DP-1"));
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        &[member]
    );
}

#[test]
fn stack_cycle_leaves_floating_members_out_of_the_card_order() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Stacking);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let floating = Rectangle::new((140, 100).into(), (480, 360).into());
    assert_eq!(
        system.toggle_member_floating("DP-1", members[0], work_area, floating, Duration::ZERO,),
        Some(true)
    );

    assert_eq!(
        system.cycle_stack(
            "DP-1",
            halley_config::FocusCycleDirection::Forward,
            work_area,
            Duration::from_secs(2),
        ),
        StackCycleOutcome::Cycled(members[2])
    );
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        &[members[0], members[2], members[1]]
    );
    assert!(system.is_member_floating(members[0]));
}

#[test]
fn floating_member_drag_updates_only_its_remembered_cluster_geometry() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let original_order = system.registry.cluster(cluster).unwrap().members().to_vec();
    let initial = Rectangle::new((120, 90).into(), (520, 400).into());
    assert_eq!(
        system.toggle_member_floating("DP-1", members[0], work_area, initial, Duration::ZERO,),
        Some(true)
    );
    assert!(system.begin_floating_member_drag("DP-1", "DP-1", members[0], initial));
    assert_eq!(
        system
            .workspace_surface_targets(
                "DP-1",
                work_area,
                Rectangle::new((0, 0).into(), (1_000, 700).into()),
            )
            .len(),
        2
    );

    let moved = Rectangle::new((340, 220).into(), initial.size);
    assert!(system.update_workspace_drag(members[0], "DP-1", moved.loc));
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(2),),
        WindowPresentation::PointerDrag { rect: moved }
    );
    assert!(system.finish_floating_member_drag("DP-1", "DP-1", members[0], work_area, moved,));
    assert_eq!(system.member_floating_rect(members[0]), Some(moved));
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        original_order
    );
}

#[test]
fn floating_member_can_leave_and_return_without_losing_cluster_state() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let original_order = system.registry.cluster(cluster).unwrap().members().to_vec();
    let initial = Rectangle::new((120, 90).into(), (520, 400).into());
    let external = Rectangle::new((80, 60).into(), initial.size);

    assert_eq!(
        system.toggle_member_floating("DP-1", members[0], work_area, initial, Duration::ZERO,),
        Some(true)
    );
    assert!(system.begin_floating_member_drag("DP-1", "DP-1", members[0], initial));
    assert!(system.update_workspace_drag(members[0], "DP-2", external.loc));
    assert_eq!(
        system.window_presentation(members[0], "DP-2", work_area, None, Duration::ZERO),
        WindowPresentation::PointerDrag { rect: external }
    );
    assert!(system.finish_floating_member_drag("DP-1", "DP-2", members[0], work_area, external,));

    assert_eq!(system.member_floating_output(members[0]), Some("DP-2"));
    assert_eq!(system.member_floating_rect(members[0]), Some(external));
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::MAX),
        WindowPresentation::Hidden
    );
    assert_eq!(
        system.window_presentation(members[0], "DP-2", work_area, None, Duration::MAX),
        WindowPresentation::Workspace {
            rect: external,
            depth: usize::MAX,
            alpha: 1.0,
        }
    );
    assert_eq!(
        system.workspace_surface_targets(
            "DP-2",
            work_area,
            Rectangle::new((1_000, 0).into(), work_area.size),
        ),
        vec![surfaces::WorkspaceSurfaceTarget {
            node_id: members[0],
            geometry: Rectangle::new((1_080, 60).into(), external.size),
        }]
    );

    assert!(system.activate_slot("DP-1", 1, Duration::from_secs(1)));
    assert_eq!(
        system.window_presentation(members[0], "DP-2", work_area, None, Duration::from_secs(20),),
        WindowPresentation::Hidden
    );
    assert!(system.activate_slot("DP-1", 1, Duration::from_secs(20)));
    assert_eq!(
        system.window_presentation(members[0], "DP-2", work_area, None, Duration::from_secs(40),),
        WindowPresentation::Workspace {
            rect: external,
            depth: usize::MAX,
            alpha: 1.0,
        }
    );

    assert!(system.begin_floating_member_drag("DP-1", "DP-2", members[0], external));
    assert!(system.update_workspace_drag(members[0], "DP-1", initial.loc));
    assert!(system.finish_floating_member_drag("DP-1", "DP-1", members[0], work_area, initial,));
    assert_eq!(system.member_floating_output(members[0]), Some("DP-1"));
    assert!(system.is_member_floating(members[0]));
    assert_eq!(system.cluster_for_member(members[0]), Some(cluster));
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        original_order
    );
}

#[test]
fn floating_member_dropped_outside_the_workspace_joins_the_field() {
    let (mut field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let initial = Rectangle::new((120, 90).into(), (520, 400).into());

    assert_eq!(
        system.toggle_member_floating("DP-1", members[0], work_area, initial, Duration::ZERO,),
        Some(true)
    );
    assert!(system.begin_floating_member_drag("DP-1", "DP-1", members[0], initial));
    assert!(system.detach_active_member_for_drag(
        &mut field,
        "DP-1",
        ClusterDragMember {
            cluster_id: cluster,
            node_id: members[0],
        },
        work_area,
        Vec2 {
            x: 1_400.0,
            y: 220.0
        },
        Duration::from_secs(2),
    ));

    assert_eq!(system.cluster_for_member(members[0]), None);
    assert!(!system.is_member_floating(members[0]));
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(2),),
        WindowPresentation::Field
    );
    assert_eq!(system.member_ids(cluster), members[1..]);
}

#[test]
fn tiled_drag_floats_the_held_member_and_inserts_it_at_the_drop_slot() {
    let (_field, mut system, cluster, members) =
        active_test_cluster(3, ClusterWorkspaceLayoutKind::Tiling);
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let target = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .find(|placement| placement.node_id == members[2])
        .unwrap()
        .rect;
    let target_center = Point::<f64, Logical>::from((
        f64::from(target.x + target.w * 0.5),
        f64::from(target.y + target.h * 0.5),
    ));

    let held = Rectangle::new((120, 90).into(), (600, 500).into());
    assert!(system.begin_workspace_drag("DP-1", members[0], held));
    assert_eq!(
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(1),),
        WindowPresentation::PointerDrag { rect: held }
    );
    assert_eq!(
        system
            .workspace_surface_targets(
                "DP-1",
                work_area,
                Rectangle::new((0, 0).into(), (1_000, 700).into()),
            )
            .len(),
        2
    );
    assert!(system.move_tiled_drag_to_point(
        "DP-1",
        members[0],
        work_area,
        target_center,
        Duration::from_secs(1),
    ));
    assert_eq!(
        system.registry.cluster(cluster).unwrap().members(),
        &[members[1], members[2], members[0]]
    );
    assert!(!system.move_tiled_drag_to_point(
        "DP-1",
        members[0],
        work_area,
        target_center,
        Duration::from_secs(1),
    ));

    let release = Rectangle::new((320, 180).into(), (400, 300).into());
    assert!(system.finish_workspace_drag(
        "DP-1",
        members[0],
        work_area,
        release,
        Duration::from_secs(2),
    ));
    let WindowPresentation::Workspace { rect, .. } =
        system.window_presentation(members[0], "DP-1", work_area, None, Duration::from_secs(2))
    else {
        panic!("the released member should return to its tile");
    };
    assert_eq!(rect, release);
}

#[test]
fn directional_tiling_focus_and_swap_share_the_layout_snapshot() {
    let mut field = Field::new();
    let ids = (0..3)
        .map(|index| {
            field.spawn_surface(
                format!("W{index}"),
                Vec2 {
                    x: index as f32 * 120.0,
                    y: 0.0,
                },
                Vec2 { x: 100.0, y: 80.0 },
            )
        })
        .collect::<Vec<_>>();
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    assert!(system.begin_creation("DP-1".into()));
    for id in &ids {
        assert!(system.toggle_creation_member(*id, "DP-1"));
    }
    assert!(system.begin_naming());
    let cluster = system.finish_creation(&mut field).unwrap();
    system.metadata.get_mut(&cluster).unwrap().layout = ClusterWorkspaceLayoutKind::Tiling;
    assert!(system.activate_slot("DP-1", 1, Duration::ZERO));
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());

    assert_eq!(
        system.directional_tile_target(
            "DP-1",
            Some(ids[0]),
            halley_config::ClusterDirection::Right,
            work_area,
        ),
        Some(ids[1])
    );
    assert_eq!(
        system.swap_directional_tile(
            "DP-1",
            Some(ids[0]),
            halley_config::ClusterDirection::Right,
            work_area,
            Duration::ZERO,
        ),
        Some(ids[0])
    );
    assert_eq!(
        system.registry().cluster(cluster).unwrap().master(),
        Some(ids[1])
    );
    let before_join = system.workspace_layout(cluster, work_area).unwrap();

    let joined = field.spawn_surface(
        "joined",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 100.0, y: 80.0 },
    );
    assert!(system.admit_mapped_window(
        &mut field,
        "DP-1",
        joined,
        halley_config::WindowClusterParticipation::Layout,
        work_area,
        Duration::from_secs(2),
    ));
    assert!(system.registry().cluster(cluster).unwrap().contains(joined));
    let after_join = system.workspace_layout(cluster, work_area).unwrap();
    let (moved, old_rect) = before_join
        .placements
        .iter()
        .find_map(|before| {
            let after = after_join
                .placements
                .iter()
                .find(|after| after.node_id == before.node_id)?;
            (before.rect != after.rect).then_some((before.node_id, before.rect))
        })
        .expect("adding a fourth tile should move an existing tile");
    let old_rect = Rectangle::<i32, Logical>::new(
        (old_rect.x.round() as i32, old_rect.y.round() as i32).into(),
        (
            old_rect.w.round().max(1.0) as i32,
            old_rect.h.round().max(1.0) as i32,
        )
            .into(),
    );
    let WindowPresentation::Workspace { rect, .. } =
        system.window_presentation(moved, "DP-1", work_area, None, Duration::from_secs(2))
    else {
        panic!("existing tile should remain visible during insertion reflow");
    };
    assert_eq!(rect, old_rect);
    assert!(system.is_animating_on_output("DP-1", Duration::from_secs(2)));

    let floating = field.spawn_surface(
        "floating",
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 100.0, y: 80.0 },
    );
    assert!(system.admit_mapped_window(
        &mut field,
        "DP-1",
        floating,
        halley_config::WindowClusterParticipation::Float,
        work_area,
        Duration::from_secs(2),
    ));
    assert_eq!(
        system.window_presentation(floating, "DP-1", work_area, None, Duration::MAX),
        WindowPresentation::Field
    );
    assert_eq!(system.cluster_for_member(floating), None);
}

#[test]
fn stacking_admission_pushes_the_front_back_and_enters_from_the_left() {
    let mut field = Field::new();
    let first = field.spawn_surface("first", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 80.0 });
    let second = field.spawn_surface(
        "second",
        Vec2 { x: 120.0, y: 0.0 },
        Vec2 { x: 100.0, y: 80.0 },
    );
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    assert!(system.begin_creation("DP-1".into()));
    assert!(system.toggle_creation_member(first, "DP-1"));
    assert!(system.toggle_creation_member(second, "DP-1"));
    assert!(system.begin_naming());
    let cluster = system.finish_creation(&mut field).unwrap();
    assert!(system.activate("DP-1", cluster, Duration::ZERO));
    let work_area = Rectangle::new((0, 0).into(), (1_000, 700).into());
    let inserted = field.spawn_surface(
        "inserted",
        Vec2 { x: 240.0, y: 0.0 },
        Vec2 { x: 100.0, y: 80.0 },
    );
    let started = Duration::from_secs(2);

    assert!(system.admit_mapped_window(
        &mut field,
        "DP-1",
        inserted,
        halley_config::WindowClusterParticipation::Layout,
        work_area,
        started,
    ));
    assert_eq!(
        system.registry().cluster(cluster).unwrap().members()[0],
        inserted
    );
    let target = system
        .workspace_layout(cluster, work_area)
        .unwrap()
        .placements
        .into_iter()
        .find(|placement| placement.node_id == inserted)
        .unwrap()
        .rect;
    let WindowPresentation::Workspace { rect, depth, .. } =
        system.window_presentation(inserted, "DP-1", work_area, None, started)
    else {
        panic!("inserted stack member should be visible");
    };
    assert!(rect.loc.x < target.x.round() as i32);
    assert_eq!(depth, 2);
}

fn create_cluster(
    field: &mut Field,
    system: &mut ClusterSystem,
    output: &str,
    label: &str,
    members: usize,
) -> ClusterId {
    assert!(system.begin_creation(output.to_string()));
    for index in 0..members {
        let member = field.spawn_surface(
            format!("{label}-{index}"),
            Vec2 { x: 40.0, y: 40.0 },
            Vec2 { x: 200.0, y: 150.0 },
        );
        assert!(system.toggle_creation_member(member, output));
    }
    assert!(system.begin_naming());
    system.finish_creation(field).unwrap()
}

fn two_clusters_on_one_output() -> (Field, ClusterSystem, ClusterId, ClusterId) {
    let mut field = Field::new();
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    let first = create_cluster(&mut field, &mut system, "DP-1", "first", 1);
    let second = create_cluster(&mut field, &mut system, "DP-1", "second", 1);
    (field, system, first, second)
}

#[test]
fn repeated_activation_does_not_toggle_a_workspace_closed() {
    let (_field, mut system, cluster, _) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Tiling);
    assert_eq!(system.active_on("DP-1"), Some(cluster));

    // `activate` toggles an already-active workspace closed; `activate_only`
    // must leave it open so a repeated protocol request is idempotent.
    assert!(!system.activate_only("DP-1", cluster, Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), Some(cluster));

    assert!(system.activate("DP-1", cluster, Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), None);
}

#[test]
fn deactivating_an_inactive_cluster_leaves_the_active_one_alone() {
    let (_field, mut system, first, second) = two_clusters_on_one_output();
    assert!(system.activate_only("DP-1", first, Duration::ZERO));
    assert!(!system.activate_only("DP-1", first, Duration::ZERO));

    assert!(!system.deactivate_only("DP-1", second, Duration::ZERO));
    assert_eq!(
        system.active_on("DP-1"),
        Some(first),
        "deactivating an inactive workspace must not clear the active one"
    );

    assert!(system.deactivate_only("DP-1", first, Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), None);
    assert!(!system.deactivate_only("DP-1", first, Duration::ZERO));
}

#[test]
fn activation_helpers_reject_a_foreign_output_without_mutation() {
    let (_field, mut system, cluster, _) =
        active_test_cluster(2, ClusterWorkspaceLayoutKind::Tiling);

    assert!(!system.activate_only("DP-2", cluster, Duration::ZERO));
    assert_eq!(system.active_on("DP-2"), None);
    assert_eq!(system.active_on("DP-1"), Some(cluster));

    assert!(!system.deactivate_only("DP-2", cluster, Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), Some(cluster));

    assert!(system.deactivate_output("DP-1", Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), None);
    assert!(!system.deactivate_output("DP-1", Duration::ZERO));
}

#[test]
fn an_empty_cluster_stays_enumerable_and_activatable() {
    // Losing the last member window leaves the workspace published but
    // empty, so the protocol must still enumerate and switch it. This is how
    // "Field" clusters reach a taskbar.
    let (mut field, mut system, cluster, members) =
        active_test_cluster(1, ClusterWorkspaceLayoutKind::Tiling);
    assert!(system.deactivate_output("DP-1", Duration::ZERO));
    assert!(system.forget_destroyed_member(&mut field, members[0]));

    assert!(system.registry().cluster(cluster).is_some());
    assert!(system.member_ids(cluster).is_empty());
    assert_eq!(system.first_member(cluster), None);
    assert_eq!(
        system
            .clusters_for_output("DP-1")
            .map(|(slot, id, _)| (slot, id))
            .collect::<Vec<_>>(),
        vec![(1, cluster)],
        "an empty workspace is still published"
    );

    assert!(system.activate_only("DP-1", cluster, Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), Some(cluster));
    assert!(system.deactivate_output("DP-1", Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), None);
}

#[test]
fn per_output_selections_are_independent() {
    let mut field = Field::new();
    let mut system = ClusterSystem::new(
        halley_config::Clusters::default(),
        halley_config::ClusterAnimation::default(),
    );
    let left = create_cluster(&mut field, &mut system, "DP-1", "left", 1);
    let right = create_cluster(&mut field, &mut system, "DP-2", "right", 1);

    assert!(system.activate_only("DP-1", left, Duration::ZERO));
    assert!(system.activate_only("DP-2", right, Duration::ZERO));
    assert!(!system.deactivate_only("DP-1", right, Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), Some(left));
    assert_eq!(system.active_on("DP-2"), Some(right));

    assert!(system.deactivate_output("DP-2", Duration::ZERO));
    assert_eq!(system.active_on("DP-1"), Some(left));
    assert_eq!(system.active_on("DP-2"), None);
}
