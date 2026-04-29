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
    if !pointer_state.borrow().move_anim.is_empty() {
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
