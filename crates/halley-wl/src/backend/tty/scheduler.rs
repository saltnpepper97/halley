use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use smithay::reexports::drm::control as drm_control;

use super::drm::TtyDrmOutput;
use crate::backend::frame_interval_for_refresh_hz;
use crate::compositor::interaction::PointerState;
use crate::compositor::root::Halley;

pub(super) fn tty_animation_redraw_active(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> bool {
    outputs.borrow().iter().any(|output| {
        tty_output_animation_redraw_active(st, pointer_state, output.connector_name.as_str(), now)
    })
}

pub(super) fn tty_animation_redraw_outputs(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> HashSet<String> {
    outputs
        .borrow()
        .iter()
        .filter_map(|output| {
            tty_output_animation_redraw_active(
                st,
                pointer_state,
                output.connector_name.as_str(),
                now,
            )
            .then_some(output.connector_name.clone())
        })
        .collect()
}

pub(super) fn tty_output_animation_redraw_active(
    st: &Halley,
    pointer_state: &Rc<RefCell<PointerState>>,
    output_name: &str,
    now: Instant,
) -> bool {
    let pointer_state = pointer_state.borrow();
    if !pointer_state.move_anim.is_empty() {
        return true;
    }

    crate::frame_loop::tty_output_animation_redraw_state(st, output_name, now).active
}

pub(super) fn tty_animation_output_ready_for_redraw(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> bool {
    let outputs_ref = outputs.borrow();
    let dpms_ref = dpms_enabled.borrow();
    let pending_ref = output_frame_pending.borrow();

    outputs_ref.iter().any(|output| {
        dpms_ref
            .get(output.connector_name.as_str())
            .copied()
            .unwrap_or(true)
            && !pending_ref
                .get(output.connector_name.as_str())
                .copied()
                .unwrap_or(false)
            && tty_output_animation_redraw_active(
                st,
                pointer_state,
                output.connector_name.as_str(),
                now,
            )
    })
}

pub(super) fn tty_ready_animation_redraw_outputs(
    st: &Halley,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    pointer_state: &Rc<RefCell<PointerState>>,
    now: Instant,
) -> HashSet<String> {
    let outputs_ref = outputs.borrow();
    let dpms_ref = dpms_enabled.borrow();
    let pending_ref = output_frame_pending.borrow();

    outputs_ref
        .iter()
        .filter_map(|output| {
            let output_name = output.connector_name.as_str();
            (dpms_ref.get(output_name).copied().unwrap_or(true)
                && !pending_ref.get(output_name).copied().unwrap_or(false)
                && tty_output_animation_redraw_active(st, pointer_state, output_name, now))
            .then(|| output.connector_name.clone())
        })
        .collect()
}

pub(super) fn tty_outputs_include_animation_redraw(
    st: &Halley,
    pointer_state: &Rc<RefCell<PointerState>>,
    output_names: &HashSet<String>,
    now: Instant,
) -> bool {
    output_names.iter().any(|output_name| {
        tty_output_animation_redraw_active(st, pointer_state, output_name.as_str(), now)
    })
}

pub(super) fn tty_due_outputs_for_timer(
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    output_timer_tick_at: &Rc<RefCell<HashMap<String, Instant>>>,
    now: Instant,
) -> HashSet<String> {
    let outputs_ref = outputs.borrow();
    let modes_ref = active_modes.borrow();
    let dpms_ref = dpms_enabled.borrow();
    let pending_ref = output_frame_pending.borrow();
    let mut last_tick_ref = output_timer_tick_at.borrow_mut();

    last_tick_ref.retain(|name, _| {
        outputs_ref
            .iter()
            .any(|output| output.connector_name == *name)
    });

    outputs_ref
        .iter()
        .filter_map(|output| {
            let output_name = output.connector_name.as_str();
            if !dpms_ref.get(output_name).copied().unwrap_or(true)
                || pending_ref.get(output_name).copied().unwrap_or(false)
            {
                return None;
            }

            let refresh_hz = modes_ref
                .get(output_name)
                .map(|mode| mode.vrefresh() as f64)
                .or(Some(output.mode.vrefresh() as f64));
            let interval = frame_interval_for_refresh_hz(refresh_hz);
            let due = last_tick_ref
                .get(output_name)
                .is_none_or(|last| now.saturating_duration_since(*last) >= interval);
            if !due {
                return None;
            }

            last_tick_ref.insert(output.connector_name.clone(), now);
            Some(output.connector_name.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::interaction::state::OverlayHoverTarget;
    use halley_core::field::NodeId;

    fn test_state() -> Halley {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
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
        }];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        Halley::new_for_test(&dh, tuning)
    }

    #[test]
    fn settled_hover_node_does_not_force_animation_redraw() {
        let mut state = test_state();
        let node_id = NodeId::new(777);
        state
            .model
            .monitor_state
            .node_monitor
            .insert(node_id, "left".to_string());
        state
            .ui
            .render_state
            .view
            .node_hover_mix
            .insert(node_id, 1.0);
        let mut pointer = PointerState::default();
        pointer.hover_node = Some(node_id);
        let pointer = Rc::new(RefCell::new(pointer));

        assert!(!tty_output_animation_redraw_active(
            &state,
            &pointer,
            "left",
            Instant::now()
        ));
    }

    #[test]
    fn transitioning_hover_mix_keeps_animation_redraw_active() {
        let mut state = test_state();
        let node_id = NodeId::new(778);
        state
            .model
            .monitor_state
            .node_monitor
            .insert(node_id, "left".to_string());
        state
            .ui
            .render_state
            .view
            .node_hover_mix
            .insert(node_id, 0.5);
        let pointer = Rc::new(RefCell::new(PointerState::default()));

        assert!(tty_output_animation_redraw_active(
            &state,
            &pointer,
            "left",
            Instant::now()
        ));
    }

    #[test]
    fn settled_hover_preview_does_not_force_animation_redraw() {
        let mut state = test_state();
        state.ui.render_state.view.node_preview_hover.insert(
            "left".to_string(),
            crate::render::state::PreviewHoverState {
                node: Some(NodeId::new(779)),
                mix: 1.0,
                overlay_anchor: None,
            },
        );
        let pointer = Rc::new(RefCell::new(PointerState::default()));

        assert!(!tty_output_animation_redraw_active(
            &state,
            &pointer,
            "left",
            Instant::now()
        ));
    }

    #[test]
    fn transitioning_hover_preview_keeps_animation_redraw_active() {
        let mut state = test_state();
        state.ui.render_state.view.node_preview_hover.insert(
            "left".to_string(),
            crate::render::state::PreviewHoverState {
                node: Some(NodeId::new(780)),
                mix: 0.5,
                overlay_anchor: None,
            },
        );
        let pointer = Rc::new(RefCell::new(PointerState::default()));

        assert!(tty_output_animation_redraw_active(
            &state,
            &pointer,
            "left",
            Instant::now()
        ));
    }

    #[test]
    fn overlay_hover_target_alone_does_not_force_animation_redraw() {
        let mut state = test_state();
        state.input.interaction_state.overlay_hover_target = Some(OverlayHoverTarget {
            node_id: NodeId::new(779),
            monitor: "left".to_string(),
            screen_anchor: (100, 100),
            prefer_left: false,
        });
        let pointer = Rc::new(RefCell::new(PointerState::default()));

        assert!(!tty_output_animation_redraw_active(
            &state,
            &pointer,
            "left",
            Instant::now()
        ));
    }
}
