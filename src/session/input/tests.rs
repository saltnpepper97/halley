use std::time::Duration;

use halley_core::field::Vec2;
use smithay::backend::input::{ButtonState, KeyState, TouchSlot};
use smithay::input::keyboard::Keysym;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::actions::{cluster_blocks_zoom, window_action_output};
use super::keyboard::{BasicsKeyRouting, ModalKeyRouting, basics_key_routing, modal_key_routing};
use super::{BTN_LEFT, BTN_RIGHT, BasicsInputRouting, BasicsTouchPhase, PendingWindowMoveMotion};
use super::{
    activation_shows_cluster_indicator, begin_cluster_core_direct_motion, bloom_drag_handoff,
    collapsed_node_drop_origin, drag_threshold_reached, forward_pointer_button,
    layer_allows_field_pan_always, outside_lift_press_dismisses, pending_window_move_motion,
    plain_background_press_dismisses_bloom, pointer_move_falls_back_to_field_pan,
    preferred_cluster_navigation_focus, releases_field_pan, releases_pending_window_move,
    sampled_drag_velocity, shortcut_policy_allows_bindings, stacking_cycle_direction,
    typing_abandons_bloom,
};
// Model the real two-phase dispatch: consume pending state before interception,
// then call the forwarding hook only for events delivered to the client.
fn steam_button(
    pending: &mut Option<u32>,
    button: u32,
    state: ButtonState,
    target: Option<u32>,
    forwarded: bool,
) -> Option<u32> {
    let previous = super::take_steam_close_press(pending, button);
    if forwarded {
        super::forwarded_steam_close_button(pending, previous, button, state, target)
    } else {
        None
    }
}

#[test]
fn steam_close_scrollbar_press_does_not_arm_close_release() {
    let mut pending = None;
    steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, None, true);
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        None
    );
}

#[test]
fn steam_close_matching_click_fires_once() {
    let mut pending = None;
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true),
        None
    );
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        Some(1)
    );
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        None
    );
}

#[test]
fn steam_close_release_elsewhere_cancels() {
    for target in [None, Some(2)] {
        let mut pending = None;
        steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true);
        assert_eq!(
            steam_button(&mut pending, BTN_LEFT, ButtonState::Released, target, true),
            None
        );
        assert_eq!(pending, None);
    }
}

#[test]
fn steam_close_intercepted_buttons_cannot_arm_or_retain_press() {
    let mut pending = None;
    steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), false);
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        None
    );
    steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true);
    steam_button(
        &mut pending,
        BTN_LEFT,
        ButtonState::Released,
        Some(1),
        false,
    );
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        None
    );
}

#[test]
fn steam_close_new_press_replaces_stale_target() {
    let mut pending = Some(1);
    steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, None, true);
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        None
    );
}

#[test]
fn steam_close_other_buttons_neither_arm_nor_complete_left_click() {
    let mut pending = None;
    steam_button(&mut pending, BTN_RIGHT, ButtonState::Pressed, Some(1), true);
    assert_eq!(pending, None);
    steam_button(&mut pending, BTN_LEFT, ButtonState::Pressed, Some(1), true);
    assert_eq!(
        steam_button(
            &mut pending,
            BTN_RIGHT,
            ButtonState::Released,
            Some(1),
            true
        ),
        None
    );
    assert_eq!(
        steam_button(&mut pending, BTN_LEFT, ButtonState::Released, Some(1), true),
        Some(1)
    );
}

fn sample_constant_motion(report_hz: u32) -> Vec2 {
    let step = Duration::from_secs_f64(1.0 / f64::from(report_hz));
    let mut previous = Vec2 { x: 0.0, y: 0.0 };
    let mut velocity = Vec2 { x: 0.0, y: 0.0 };
    let mut last = Duration::ZERO;
    for index in 1..=report_hz {
        let now = step * index;
        let current = Vec2 {
            x: 400.0 * now.as_secs_f32(),
            y: -180.0 * now.as_secs_f32(),
        };
        velocity = sampled_drag_velocity(previous, current, velocity, last, now);
        previous = current;
        last = now;
    }
    velocity
}

#[test]
fn collapsed_node_drop_origin_is_local_to_the_destination_output() {
    let output = Rectangle::<i32, Logical>::new((1_920, 0).into(), (2_560, 1_440).into());

    assert_eq!(
        collapsed_node_drop_origin(
            Vec2 {
                x: 2_400.0,
                y: 500.0
            },
            Size::from((800, 600)),
            output,
        ),
        Rectangle::new((80, 200).into(), (800, 600).into())
    );
}

#[test]
fn drag_velocity_is_stable_across_common_mouse_report_rates() {
    for report_hz in [125, 500, 1_000] {
        let sampled = sample_constant_motion(report_hz);
        assert!((sampled.x - 400.0).abs() < 0.1, "{report_hz} Hz");
        assert!((sampled.y + 180.0).abs() < 0.1, "{report_hz} Hz");
    }
}

#[test]
fn drag_velocity_ignores_duplicate_timestamps() {
    let previous_velocity = Vec2 { x: 25.0, y: -15.0 };
    let sampled = sampled_drag_velocity(
        Vec2 { x: 10.0, y: 20.0 },
        Vec2 { x: 50.0, y: 80.0 },
        previous_velocity,
        Duration::from_millis(5),
        Duration::from_millis(5),
    );
    assert_eq!(sampled, previous_velocity);
}

#[test]
fn maximized_titlebar_drag_ignores_double_click_jitter() {
    let press = Point::<f64, Logical>::from((400.0, 250.0));

    assert!(!drag_threshold_reached(press, (403.0, 254.0)));
    assert!(drag_threshold_reached(press, (408.0, 250.0)));
}

#[test]
fn cluster_core_drag_start_commits_its_zoom_displacement() {
    let mut nodes = crate::nodes::NodesState::new(&halley_config::RuntimeConfig::default());
    let core = nodes.field.spawn_surface(
        "cluster-core",
        Vec2 { x: 180.0, y: 120.0 },
        Vec2 { x: 48.0, y: 48.0 },
    );
    assert!(
        nodes
            .field
            .set_state(core, halley_core::field::NodeState::Core)
    );
    nodes.remember_zoom_home(core, Vec2 { x: 100.0, y: 120.0 });

    begin_cluster_core_direct_motion(&mut nodes, Some(core));

    assert_eq!(nodes.zoom_home(core), None);
}

#[test]
fn move_window_grab_becomes_field_pan_on_background() {
    assert!(pointer_move_falls_back_to_field_pan(
        &crate::input::pointer::PointerTarget::Background
    ));
}

#[test]
fn camera_pan_ends_only_with_the_button_that_began_it() {
    use crate::input::grab::Grab;
    const BTN_MIDDLE: u32 = 0x112;

    let pan = Grab::Pan {
        output: "DP-1".into(),
        button: BTN_MIDDLE,
    };
    assert!(releases_field_pan(&pan, BTN_MIDDLE, true));
    assert!(
        !releases_field_pan(&pan, BTN_MIDDLE, false),
        "the press that began the pan must not end it"
    );
    assert!(
        !releases_field_pan(&pan, BTN_LEFT, true),
        "releasing another button must not drop a middle-button pan"
    );
    assert!(!releases_field_pan(&pan, BTN_RIGHT, true));

    // A left-button pan (the historical `pan-field` binding) still ends on
    // the left release, and nothing but a pan is ever released here.
    let left = Grab::Pan {
        output: "DP-1".into(),
        button: BTN_LEFT,
    };
    assert!(releases_field_pan(&left, BTN_LEFT, true));
    assert!(!releases_field_pan(&left, BTN_MIDDLE, true));
    assert!(!releases_field_pan(&Grab::None, BTN_LEFT, true));
}

#[test]
fn always_pan_covers_desktop_layers_but_not_panels_or_overlays() {
    use smithay::wayland::shell::wlr_layer::Layer;

    assert!(layer_allows_field_pan_always(Layer::Background));
    assert!(layer_allows_field_pan_always(Layer::Bottom));
    assert!(!layer_allows_field_pan_always(Layer::Top));
    assert!(!layer_allows_field_pan_always(Layer::Overlay));
}

#[test]
fn client_titlebar_move_forwards_its_consumed_release() {
    assert!(forward_pointer_button(true, true));
    assert!(!forward_pointer_button(true, false));
    assert!(forward_pointer_button(false, false));
}

#[test]
fn left_press_outside_lift_dismisses_without_claiming_inside_clicks() {
    assert!(outside_lift_press_dismisses(
        BTN_LEFT,
        ButtonState::Pressed,
        true,
        false
    ));
    assert!(!outside_lift_press_dismisses(
        BTN_LEFT,
        ButtonState::Pressed,
        true,
        true
    ));
    assert!(!outside_lift_press_dismisses(
        BTN_LEFT,
        ButtonState::Released,
        true,
        false
    ));
    assert!(!outside_lift_press_dismisses(
        BTN_LEFT,
        ButtonState::Pressed,
        false,
        false
    ));
}

#[test]
fn client_titlebar_click_releases_without_activating_move() {
    assert!(releases_pending_window_move(BTN_LEFT, BTN_LEFT, true));
    assert!(!releases_pending_window_move(BTN_LEFT, BTN_LEFT, false));
    assert!(!releases_pending_window_move(BTN_LEFT, BTN_RIGHT, true));
}

#[test]
fn bloom_stays_open_for_windows_and_compositor_targets_until_a_blank_click() {
    assert!(!plain_background_press_dismisses_bloom(false, false));
    assert!(!plain_background_press_dismisses_bloom(true, true));
    assert!(plain_background_press_dismisses_bloom(false, true));
}

#[test]
fn typing_away_closes_a_bloom_after_the_window_drag_is_over() {
    assert!(!typing_abandons_bloom(true, false));
    assert!(!typing_abandons_bloom(false, true));
    assert!(typing_abandons_bloom(false, false));
}

#[test]
fn client_titlebar_move_activates_only_after_valid_threshold_motion() {
    let press = Point::<f64, Logical>::from((400.0, 250.0));

    assert_eq!(
        pending_window_move_motion(true, press, (403.0, 254.0)),
        PendingWindowMoveMotion::Wait,
    );
    assert_eq!(
        pending_window_move_motion(true, press, (408.0, 250.0)),
        PendingWindowMoveMotion::Activate,
    );
    assert_eq!(
        pending_window_move_motion(false, press, (420.0, 250.0)),
        PendingWindowMoveMotion::Cancel,
    );
}

#[test]
fn bloom_drag_handoff_preserves_a_centered_source_grip() {
    let location = Point::<i32, Logical>::from((300, 200));
    let size = Size::<i32, Logical>::from((800, 600));
    let pointer = Vec2 { x: 700.0, y: 500.0 };

    let (source_offset, center) = bloom_drag_handoff(location, size, pointer);

    assert_eq!(
        source_offset,
        Vec2 {
            x: -400.0,
            y: -300.0
        }
    );
    assert_eq!(center, pointer);
}

#[test]
fn shortcut_policy_respects_session_lock_and_client_inhibition() {
    assert!(shortcut_policy_allows_bindings(false, false));
    assert!(!shortcut_policy_allows_bindings(true, false));
    assert!(!shortcut_policy_allows_bindings(false, true));
    assert!(!shortcut_policy_allows_bindings(true, true));
}

#[test]
fn keyboard_actions_ignore_stationary_pointer_in_both_focus_modes() {
    for mode in [
        halley_config::FocusMode::Hover,
        halley_config::FocusMode::Click,
    ] {
        assert_eq!(
            window_action_output(
                super::actions::DispatchOrigin::Keyboard,
                mode,
                Some("left"),
                Some("right")
            ),
            Some("right".into())
        );
        assert_eq!(
            window_action_output(
                super::actions::DispatchOrigin::Keyboard,
                mode,
                Some("left"),
                None
            ),
            None
        );
    }
}

#[test]
fn window_actions_follow_pointer_only_in_hover_mode() {
    assert_eq!(
        window_action_output(
            super::actions::DispatchOrigin::Other,
            halley_config::FocusMode::Hover,
            Some("right"),
            Some("left"),
        ),
        Some("right".to_string())
    );
    assert_eq!(
        window_action_output(
            super::actions::DispatchOrigin::Other,
            halley_config::FocusMode::Click,
            Some("right"),
            Some("left"),
        ),
        Some("left".to_string())
    );
    assert_eq!(
        window_action_output(
            super::actions::DispatchOrigin::Other,
            halley_config::FocusMode::Hover,
            None,
            Some("left")
        ),
        Some("left".to_string())
    );
}

#[test]
fn only_empty_cluster_activation_shows_the_centered_indicator() {
    assert!(activation_shows_cluster_indicator(None));
    assert!(!activation_shows_cluster_indicator(Some(
        halley_core::field::NodeId::new(1)
    )));
}

#[test]
fn active_cluster_blocks_every_compositor_zoom_action() {
    for action in [
        halley_config::Action::ZoomIn,
        halley_config::Action::ZoomOut,
        halley_config::Action::ZoomReset,
    ] {
        assert!(cluster_blocks_zoom(&action, true));
        assert!(!cluster_blocks_zoom(&action, false));
    }
    assert!(!cluster_blocks_zoom(
        &halley_config::Action::ToggleState,
        true
    ));
}

#[test]
fn horizontal_cluster_focus_maps_to_old_stack_cycle_directions() {
    assert_eq!(
        stacking_cycle_direction(halley_config::Direction::Left),
        Some(halley_config::FocusCycleDirection::Forward)
    );
    assert_eq!(
        stacking_cycle_direction(halley_config::Direction::Right),
        Some(halley_config::FocusCycleDirection::Backward)
    );
    assert_eq!(stacking_cycle_direction(halley_config::Direction::Up), None);
    assert_eq!(
        stacking_cycle_direction(halley_config::Direction::Down),
        None
    );
}

#[test]
fn live_client_focus_wins_for_hidden_cluster_members() {
    let active_member = halley_core::field::NodeId::new(1);
    let stale_logical = halley_core::field::NodeId::new(2);

    assert_eq!(
        preferred_cluster_navigation_focus(Some(active_member), Some(stale_logical)),
        Some(active_member)
    );
    assert_eq!(
        preferred_cluster_navigation_focus(None, Some(stale_logical)),
        Some(stale_logical)
    );
}

#[test]
fn basics_pointer_dismissal_owns_only_its_press_and_release() {
    let mut owned = crate::input::SuppressedButtons::default();

    assert_eq!(
        super::basics_pointer_button_routing(true, BTN_LEFT, ButtonState::Pressed, &mut owned),
        BasicsInputRouting::Dismiss
    );
    assert_eq!(
        super::basics_pointer_button_routing(false, BTN_RIGHT, ButtonState::Pressed, &mut owned),
        BasicsInputRouting::Forward,
        "a new press during the dismissal fade reaches its target"
    );
    assert_eq!(
        super::basics_pointer_button_routing(false, BTN_LEFT, ButtonState::Released, &mut owned),
        BasicsInputRouting::SuppressOwned,
        "the release paired with the dismissing press stays suppressed"
    );
    assert_eq!(
        super::basics_pointer_button_routing(false, BTN_RIGHT, ButtonState::Released, &mut owned),
        BasicsInputRouting::Forward
    );
}

#[test]
fn basics_touch_dismissal_owns_only_its_touch_stream() {
    let dismissing = TouchSlot::from(Some(1));
    let next = TouchSlot::from(Some(2));
    let mut owned = crate::input::SuppressedReleases::default();

    assert_eq!(
        super::basics_touch_routing(true, dismissing, BasicsTouchPhase::Down, &mut owned),
        BasicsInputRouting::Dismiss
    );
    assert_eq!(
        super::basics_touch_routing(false, next, BasicsTouchPhase::Down, &mut owned),
        BasicsInputRouting::Forward,
        "a new touch during the dismissal fade reaches its target"
    );
    assert_eq!(
        super::basics_touch_routing(false, dismissing, BasicsTouchPhase::Motion, &mut owned),
        BasicsInputRouting::SuppressOwned
    );
    assert_eq!(
        super::basics_touch_routing(false, dismissing, BasicsTouchPhase::Up, &mut owned),
        BasicsInputRouting::SuppressOwned
    );
    assert_eq!(
        super::basics_touch_routing(false, next, BasicsTouchPhase::Up, &mut owned),
        BasicsInputRouting::Forward
    );
}

#[test]
fn basics_card_only_intercepts_its_dismissal_keys() {
    assert_eq!(
        basics_key_routing(true, KeyState::Pressed, Some(Keysym::Return)),
        BasicsKeyRouting::Dismiss
    );
    assert_eq!(
        basics_key_routing(true, KeyState::Pressed, Some(Keysym::Escape)),
        BasicsKeyRouting::Dismiss
    );
    assert_eq!(
        basics_key_routing(true, KeyState::Pressed, Some(Keysym::d)),
        BasicsKeyRouting::EvaluateNormally,
        "non-dismissal presses must reach compositor keybind matching"
    );
    assert_eq!(
        basics_key_routing(true, KeyState::Released, Some(Keysym::Return)),
        BasicsKeyRouting::EvaluateNormally
    );
}

#[test]
fn modal_releases_retire_preexisting_forwarded_keys() {
    assert_eq!(
        modal_key_routing(true, KeyState::Released, false),
        ModalKeyRouting::RetireUnfocusedRelease
    );
    assert_eq!(
        modal_key_routing(false, KeyState::Released, true),
        ModalKeyRouting::SuppressRelease
    );
    assert_eq!(
        modal_key_routing(true, KeyState::Pressed, false),
        ModalKeyRouting::Evaluate
    );
}
