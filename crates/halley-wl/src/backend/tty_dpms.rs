use super::*;
use std::collections::HashMap;

use crate::backend::tty_drm::collect_outputs_for_ipc;
use crate::backend::tty_drm::TtyDrmOutput;

pub(crate) fn publish_tty_outputs_snapshot(
    dev: &DrmDevice,
    active_modes: &HashMap<String, drm_control::Mode>,
    dpms_enabled: bool,
    tuning: &RuntimeTuning,
) {
    let vrr_support: HashMap<String, String> = HashMap::new();
    let mut outputs = collect_outputs_for_ipc(dev, active_modes, tuning, &vrr_support);
    if !dpms_enabled {
        for output in &mut outputs {
            if active_modes.contains_key(&output.name) {
                output.enabled = false;
                output.current_mode = None;
                for mode in &mut output.modes {
                    mode.current = false;
                }
            }
        }
    }
    publish_outputs(outputs);
}

pub(crate) fn apply_tty_dpms_command(
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    command: halley_ipc::DpmsCommand,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    tuning: &RuntimeTuning,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
) {
    let target_enabled = match command {
        halley_ipc::DpmsCommand::On => true,
        halley_ipc::DpmsCommand::Off => false,
        halley_ipc::DpmsCommand::Toggle => !*dpms_enabled.borrow(),
    };

    if target_enabled == *dpms_enabled.borrow() {
        return;
    }

    if !target_enabled {
        // DrmCompositor::clear() disables the CRTC using an atomic
        // ALLOW_MODESET commit — non-blocking, returns immediately.
        // The kernel queues the disable and fires it on the next vblank.
        //
        // This also resets the compositor's internal buffer/damage state so
        // the next queue_frame after wake atomically re-enables the CRTC as
        // part of presenting the first frame — no separate modeset step.
        for output in outputs.borrow().iter() {
            if let Err(err) = output.compositor.borrow_mut().clear() {
                warn!("tty dpms off: clear failed for {}: {}", output.connector_name, err);
            }
        }
        for val in output_frame_pending.borrow_mut().values_mut() {
            *val = false;
        }
        *dpms_enabled.borrow_mut() = false;
        info!("tty dpms: powered off (atomic CRTC disable)");
    } else {
        // Wake: force every output compositor back into a fresh post-clear state
        // so the very next render/queue re-enables scanout immediately even if
        // Smithay reports no damage on that first frame.
        for output in outputs.borrow().iter() {
            let mut compositor = output.compositor.borrow_mut();
            if let Err(err) = compositor.reset_state() {
                warn!(
                    "tty dpms on: reset_state failed for {}: {}",
                    output.connector_name, err
                );
            }
            compositor.reset_buffers();
        }

        for val in output_frame_pending.borrow_mut().values_mut() {
            *val = false;
        }
        *dpms_enabled.borrow_mut() = true;
        info!("tty dpms: powering on (forced fresh frame on next render)");
    }

    publish_tty_outputs_snapshot(
        &dev.borrow(),
        &active_modes.borrow(),
        *dpms_enabled.borrow(),
        tuning,
    );
}

pub(crate) fn wake_tty_dpms_on_input(
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    tuning: &RuntimeTuning,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
) {
    if *dpms_enabled.borrow() {
        return;
    }
    apply_tty_dpms_command(
        dev,
        active_modes,
        dpms_enabled,
        halley_ipc::DpmsCommand::On,
        outputs,
        tuning,
        output_frame_pending,
    );
}
