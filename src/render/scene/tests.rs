use super::*;
use smithay::utils::{Physical, Rectangle};

#[test]
fn raised_cluster_float_crosses_the_exclusive_boundary() {
    assert!(!cluster_float_is_above_exclusive(true, 3, 4));
    assert!(!cluster_float_is_above_exclusive(false, 5, 4));
    assert!(cluster_float_is_above_exclusive(true, 5, 4));
}

#[test]
fn hover_preview_aspect_fit_centers_wide_and_tall_windows() {
    let bounds = Rectangle::<i32, Physical>::new((10, 20).into(), (200, 160).into());
    assert_eq!(
        aspect_fit_rect(bounds, 1_600, 900),
        Rectangle::new((10, 43).into(), (200, 113).into())
    );
    assert_eq!(
        aspect_fit_rect(bounds, 900, 1_600),
        Rectangle::new((65, 20).into(), (90, 160).into())
    );
}

#[test]
fn picker_outline_and_grips_describe_the_whole_selection() {
    let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
    let selection = Rectangle::<i32, Logical>::new((320, 180).into(), (1280, 720).into());

    let layout = picker_layout(output, selection).expect("selection is on this output");
    assert_eq!(
        layout.outline,
        Rectangle::new((320, 180).into(), (1280, 720).into())
    );
    assert_eq!(layout.handles.len(), 4);
    assert_eq!(
        layout.handles[0],
        Rectangle::new((314, 174).into(), (12, 12).into())
    );
    assert_eq!(
        layout.handles[3],
        Rectangle::new((1594, 894).into(), (12, 12).into())
    );
}

/// The reported bug: a selection spanning two outputs used to be clipped
/// per output, so each monitor drew its own box with its own four grips.
/// The outline and grips must instead stay in one global selection, with
/// only the dimmed surround computed per output.
#[test]
fn a_selection_spanning_outputs_keeps_one_outline_and_one_grip_set() {
    let main = Rectangle::<i32, Logical>::new((0, 0).into(), (2560, 1440).into());
    let secondary = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
    let selection = Rectangle::<i32, Logical>::new((2000, 100).into(), (1500, 800).into());

    let on_main = picker_layout(main, selection).expect("overlaps main");
    let on_secondary = picker_layout(secondary, selection).expect("overlaps secondary");

    // Same region, expressed in each output's local coordinates.
    assert_eq!(
        on_main.outline,
        Rectangle::new((2000, 100).into(), (1500, 800).into())
    );
    assert_eq!(
        on_secondary.outline,
        Rectangle::new((-560, 100).into(), (1500, 800).into())
    );
    assert_eq!(
        on_main.outline.loc.x - 2560,
        on_secondary.outline.loc.x,
        "the two outlines must be the same rectangle in global space"
    );

    // The four grips sit at the selection's true corners, so each output
    // renders only the ones that actually land on it.
    let visible = |layout: &super::capture_ui::PickerLayout, bounds: Rectangle<i32, Logical>| {
        let local = Rectangle::<i32, Physical>::from_size(bounds.size.to_physical(1));
        layout
            .handles
            .iter()
            .filter(|handle| local.overlaps(**handle))
            .count()
    };
    assert_eq!(visible(&on_main, main), 2);
    assert_eq!(visible(&on_secondary, secondary), 2);
}

#[test]
fn the_dimmed_surround_is_clipped_to_each_output() {
    let secondary = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
    let selection = Rectangle::<i32, Logical>::new((2000, 100).into(), (1500, 800).into());

    let layout = picker_layout(secondary, selection).expect("overlaps secondary");
    let local = Rectangle::<i32, Physical>::from_size(secondary.size.to_physical(1));

    assert!(
        layout.dim.iter().all(|rect| local.contains_rect(*rect)),
        "dim rects must stay on their own output: {:?}",
        layout.dim
    );
}

#[test]
fn secondary_window_geometry_becomes_output_local() {
    let secondary = Rectangle::<i32, Logical>::new((2560, 0).into(), (1920, 1200).into());
    let camera_center =
        crate::presentation::camera::global_center(Point::from((1060.0, 550.0)), secondary);
    let world_rect = Rectangle::<i32, Physical>::new((3520, 600).into(), (200, 100).into());

    assert_eq!(
        super::super::camera_rect(
            world_rect,
            camera_center,
            secondary.size.to_physical(1),
            0.5,
        ),
        Rectangle::new((910, 625).into(), (100, 50).into())
    );
}

#[test]
fn fullscreen_rect_interpolates_position_and_size() {
    let windowed = Rectangle::new((100, 50).into(), (800, 600).into());
    let start = crate::wayland::fullscreen::FullscreenPresentation {
        progress: 0.0,
        transition_completion: 0.0,
        windowed_geometry: None,
        windowed_output_rect: None,
        fullscreen_size: (1920, 1080).into(),
    };
    let end = crate::wayland::fullscreen::FullscreenPresentation {
        progress: 1.0,
        ..start
    };
    let middle = crate::wayland::fullscreen::FullscreenPresentation {
        progress: 0.5,
        ..start
    };

    assert_eq!(start.client_rect(windowed, (1920, 1080).into()), windowed);
    assert_eq!(
        end.client_rect(windowed, (1920, 1080).into()),
        Rectangle::new((0, 0).into(), (1920, 1080).into())
    );
    assert_eq!(
        middle.client_rect(windowed, (1920, 1080).into()),
        Rectangle::new((50, 25).into(), (1360, 840).into())
    );
}

#[test]
fn surface_rects_map_into_animated_client_bounds() {
    assert_eq!(
        crate::animation::map_rect(
            Rectangle::new((120, 70).into(), (200, 100).into()),
            Rectangle::new((100, 50).into(), (800, 600).into()),
            Rectangle::new((0, 0).into(), (1600, 1200).into()),
        ),
        Rectangle::new((40, 40).into(), (400, 200).into())
    );
}

#[test]
fn apogee_transition_never_fades_the_handoff_preview() {
    for progress in [0.0, 0.0005, 0.5, 1.0] {
        let visuals = apogee_transition_visuals(progress);
        assert_eq!(visuals.preview_alpha, 1.0);
        assert_eq!(visuals.overlay_alpha, progress);
    }
    assert_eq!(apogee_transition_visuals(0.0).chrome_alpha, 0.0);
    assert_eq!(apogee_transition_visuals(1.0).chrome_alpha, 1.0);
}

#[test]
fn apogee_transition_preserves_source_stack_and_promotes_selection() {
    let back = crate::shell::apogee::Tile {
        id: halley_core::field::NodeId::new(1),
        kind: crate::shell::apogee::TileKind::Window,
        output: "DP-1".into(),
        target: Rectangle::new((0, 0).into(), (100, 100).into()),
        source_stack_index: 0,
        source_stack_order: u64::MAX,
    };
    let middle = crate::shell::apogee::Tile {
        id: halley_core::field::NodeId::new(2),
        kind: crate::shell::apogee::TileKind::Window,
        output: "DP-1".into(),
        target: Rectangle::new((100, 0).into(), (100, 100).into()),
        source_stack_index: 1,
        source_stack_order: 0,
    };
    let front = crate::shell::apogee::Tile {
        id: halley_core::field::NodeId::new(3),
        kind: crate::shell::apogee::TileKind::Window,
        output: "DP-1".into(),
        target: Rectangle::new((200, 0).into(), (100, 100).into()),
        source_stack_index: 2,
        source_stack_order: u64::MAX,
    };
    let mut tiles = vec![&middle, &front, &back];

    sort_apogee_tiles(&mut tiles, None);
    assert_eq!(
        tiles.iter().map(|tile| tile.id).collect::<Vec<_>>(),
        vec![front.id, middle.id, back.id]
    );

    sort_apogee_tiles(&mut tiles, Some(middle.id));
    assert_eq!(
        tiles.iter().map(|tile| tile.id).collect::<Vec<_>>(),
        vec![middle.id, front.id, back.id]
    );
}

#[test]
fn collapsed_node_keeps_front_middle_and_back_stack_depth() {
    let presented = |collapsed_index: usize, remaining: &[(usize, u64)]| {
        let mut groups = remaining
            .iter()
            .copied()
            .map(|(stack_index, order)| StackGroup {
                window: None,
                stack_index,
                order,
                elements: Vec::new(),
            })
            .chain([
                StackGroup {
                    window: None,
                    stack_index: collapsed_index,
                    order: 1,
                    elements: Vec::new(),
                },
                StackGroup {
                    window: None,
                    stack_index: collapsed_index,
                    order: 0,
                    elements: Vec::new(),
                },
            ])
            .collect::<Vec<_>>();
        sort_stack_groups(&mut groups);
        groups
            .into_iter()
            .rev()
            .map(|group| (group.stack_index, group.order))
            .collect::<Vec<_>>()
    };

    // u64::MAX identifies a live window. The close snapshot (1) and
    // resulting node (0) remain on the collapsed window's side of it.
    assert_eq!(
        presented(2, &[(0, u64::MAX), (1, u64::MAX)])[..2],
        [(2, 1), (2, 0)]
    );
    assert_eq!(
        presented(1, &[(0, u64::MAX), (1, u64::MAX)])[..3],
        [(1, u64::MAX), (1, 1), (1, 0)]
    );
    assert_eq!(
        presented(0, &[(0, u64::MAX), (1, u64::MAX)])[..4],
        [(1, u64::MAX), (0, u64::MAX), (0, 1), (0, 0)]
    );
}
