use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::{Physical, Size, Transform};

use super::*;

#[test]
fn keyboard_source_navigation_wraps_and_handles_no_selection() {
    assert_eq!(next_source_index(0, None, 1), None);
    assert_eq!(next_source_index(3, None, 1), Some(0));
    assert_eq!(next_source_index(3, None, -1), Some(2));
    assert_eq!(next_source_index(3, Some(2), 1), Some(0));
    assert_eq!(next_source_index(3, Some(0), -1), Some(2));
}

fn output(name: &str, size: Size<i32, Physical>) -> Output {
    let output = Output::new(
        name.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "halley-next".into(),
            model: "test".into(),
            serial_number: "test".into(),
        },
    );
    let mode = Mode {
        size,
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output
}

fn outputs() -> Space<Window> {
    let left = output("DP-1", (1920, 1080).into());
    let right = output("DP-2", (2560, 1440).into());
    let mut space = Space::default();
    space.map_output(&left, (0, 0));
    space.map_output(&right, (1920, 0));
    space
}

#[test]
fn explicit_region_stays_on_its_output_after_layout_updates_and_menu_return() {
    let mut space = outputs();
    let right = space
        .outputs()
        .find(|output| output.name() == "DP-2")
        .unwrap()
        .clone();
    let bounds = space.output_geometry(&right).unwrap();
    let mut capture = CaptureState::default();
    assert!(capture.begin_menu(&space, Some("DP-2"), true));
    assert!(capture.activate_region(&space, Some("DP-2")));
    assert!(capture.update_layout(&space));
    let region = capture.picker.region().unwrap();
    capture.press((
        (region.loc.x + region.size.w / 2) as f64,
        (region.loc.y + region.size.h / 2) as f64,
    ));
    capture.motion((-5000.0, -5000.0));
    capture.release();
    assert!(bounds.contains_rect(capture.picker.region().unwrap()));
    assert!(capture.return_to_menu());
    assert!(capture.activate_menu(ScreenshotMode::Region, &space));
    assert_eq!(capture.region_output.as_deref(), Some("DP-2"));
    space.unmap_output(&right);
    assert!(!capture.update_layout(&space));
    capture.cancel();
    assert!(capture.begin_menu(&space, Some("DP-1"), true));
    assert!(capture.activate_menu(ScreenshotMode::Region, &space));
    assert!(capture.region_output.is_none());
    assert!(capture.update_layout(&space));
}

#[test]
fn accepting_a_scoped_region_keeps_its_output_for_pixel_capture() {
    let space = outputs();
    let mut capture = CaptureState::default();
    assert!(capture.begin_menu(&space, Some("DP-2"), true));
    assert!(capture.activate_region(&space, Some("DP-2")));
    let selected = capture.picker.region().unwrap();
    let accepted = capture.accept().unwrap();
    assert!(
        matches!(accepted.target, AcceptedTarget::Area { region, output: Some(name) }
            if region == selected && name == "DP-2")
    );
    assert!(!capture.is_active());
    assert!(capture.region_output.is_none());
}

#[test]
fn rectangles_use_output_relative_logical_coordinates_and_reject_clipping_or_overflow() {
    let bounds = Rectangle::new((-1600, 300).into(), (1600, 900).into());
    let rect = |x, y, width, height| halley_ipc::CaptureRect {
        x,
        y,
        width,
        height,
    };
    assert_eq!(
        output_relative_rect(bounds, rect(100, 50, 400, 200)).unwrap(),
        Rectangle::new((-1500, 350).into(), (400, 200).into())
    );
    assert_eq!(
        output_relative_rect(bounds, rect(0, 0, 1600, 900)).unwrap(),
        bounds
    );
    for invalid in [
        rect(-1, 0, 10, 10),
        rect(0, -1, 10, 10),
        rect(0, 0, 0, 10),
        rect(0, 0, 10, -1),
        rect(1599, 0, 2, 10),
        rect(0, 899, 10, 2),
        rect(i32::MAX, 0, i32::MAX, 10),
    ] {
        assert!(
            output_relative_rect(bounds, invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    let overflow = Rectangle::new((i32::MAX - 5, 0).into(), (1600, 900).into());
    assert!(output_relative_rect(overflow, rect(10, 0, 10, 10)).is_err());
    assert!(output_relative_rect(overflow, rect(0, 0, 10, 10)).is_err());
}

#[test]
fn named_output_capture_excludes_overlapping_outputs_with_other_scales() {
    let mut space = outputs();
    let right = space
        .outputs()
        .find(|output| output.name() == "DP-2")
        .unwrap()
        .clone();
    right.change_current_state(
        None,
        None,
        Some(smithay::output::Scale::Fractional(1.5)),
        None,
    );
    space.map_output(&right, (0, 0));
    let selected = screenshot::screenshot_outputs(&space, Some("DP-1"));
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].0.name(), "DP-1");
    assert_eq!(selected[0].0.current_scale().fractional_scale(), 1.0);
    assert_eq!(selected[0].1.size, (1920, 1080).into());
    assert_eq!(screenshot::screenshot_outputs(&space, None).len(), 2);
    assert!(screenshot::screenshot_outputs(&space, Some("missing")).is_empty());
}

#[test]
fn local_menu_opens_on_the_preferred_output() {
    let space = outputs();
    let mut capture = CaptureState::default();
    assert!(capture.begin_menu(&space, Some("DP-2"), true));
    assert!(matches!(
        capture.overlay(),
        CaptureOverlay::Menu {
            output_name: "DP-2",
            selected: 0,
            window_available: true,
            ..
        }
    ));
}

#[test]
fn escape_from_a_local_selector_returns_to_the_menu() {
    let space = outputs();
    let mut capture = CaptureState::default();
    assert!(capture.begin_menu(&space, Some("DP-2"), true));
    assert!(capture.activate_menu(ScreenshotMode::Region, &space));
    assert_eq!(capture.kind(), Some(CaptureKind::Area));
    assert!(capture.return_to_menu());
    assert_eq!(capture.kind(), Some(CaptureKind::Menu));
    assert!(!capture.return_to_menu());
}

#[test]
fn returning_to_menu_retains_the_adjusted_region() {
    let space = outputs();
    let mut capture = CaptureState::default();
    assert!(capture.begin_menu(&space, Some("DP-2"), true));
    assert!(capture.activate_menu(ScreenshotMode::Region, &space));
    capture.press((3000.0, 700.0));
    capture.motion((3100.0, 750.0));
    capture.release();
    let CaptureOverlay::Region(adjusted) = capture.overlay() else {
        panic!("region selector should be active");
    };

    assert!(capture.return_to_menu());
    assert!(capture.activate_menu(ScreenshotMode::Region, &space));
    assert!(matches!(
        capture.overlay(),
        CaptureOverlay::Region(region) if region == adjusted
    ));
}

#[test]
fn screen_selection_accepts_the_output_last_hovered() {
    let space = outputs();
    let mut capture = CaptureState::default();
    assert!(capture.begin_menu(&space, Some("DP-2"), true));
    assert!(capture.activate_menu(ScreenshotMode::Screen, &space));
    let left = Rectangle::new((0, 0).into(), (1920, 1080).into());
    assert!(capture.hover_screen(left));
    let accepted = capture.accept().expect("screen capture should be ready");
    assert!(matches!(accepted.target, AcceptedTarget::Screen(region) if region == left));
}
