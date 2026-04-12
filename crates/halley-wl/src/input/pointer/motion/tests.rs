use smithay::reexports::wayland_server::Display;
use std::time::Instant;
use crate::compositor::interaction::HitNode;
use halley_config::InputFocusMode;
use crate::compositor::root::Halley;

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
            on_titlebar: false,
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
            on_titlebar: false,
            is_core: false,
        }),
        false,
        Instant::now(),
    );

    assert_eq!(st.model.focus_state.primary_interaction_focus, None);
}

#[test]
fn hover_focus_gate_disables_focus_follows_mouse_while_layer_shell_is_active() {
    assert!(!super::focus::hover_focus_enabled(InputFocusMode::Hover, false, true));
    assert!(super::focus::hover_focus_enabled(InputFocusMode::Hover, false, false));
}
