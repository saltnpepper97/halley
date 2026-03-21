use super::*;
use std::collections::HashMap;

use crate::backend::tty_drm::collect_outputs_for_ipc;
const DRM_MODE_DPMS_ON: u64 = 0;
const DRM_MODE_DPMS_OFF: u64 = 3;

fn set_connector_dpms_state(
    dev: &DrmDevice,
    active_modes: &HashMap<String, drm_control::Mode>,
    enabled: bool,
) -> Result<bool, Box<dyn Error>> {
    let resources = dev
        .resource_handles()
        .map_err(|err| io::Error::other(format!("failed to query drm resources: {}", err)))?;
    let mut changed_any = false;

    for conn in resources.connectors() {
        let info = dev.get_connector(*conn, false).map_err(|err| {
            io::Error::other(format!("failed to query drm connector {:?}: {}", conn, err))
        })?;
        if info.state() != drm_control::connector::State::Connected {
            continue;
        }
        if !active_modes.contains_key(&info.to_string()) {
            continue;
        }

        let props = dev.get_properties(*conn).map_err(|err| {
            io::Error::other(format!(
                "failed to get connector properties for {}: {}",
                info, err
            ))
        })?;
        let (handles, _) = props.as_props_and_values();
        for handle in handles {
            let prop = dev.get_property(*handle).map_err(|err| {
                io::Error::other(format!(
                    "failed to query connector property {:?} for {}: {}",
                    handle, info, err
                ))
            })?;
            if !prop
                .name()
                .to_str()
                .is_ok_and(|name| name == "DPMS")
            {
                continue;
            }
            dev.set_property(
                *conn,
                *handle,
                if enabled {
                    DRM_MODE_DPMS_ON.into()
                } else {
                    DRM_MODE_DPMS_OFF.into()
                },
            )
            .map_err(|err| {
                io::Error::other(format!(
                    "failed to set DPMS={} for {}: {}",
                    enabled, info, err
                ))
            })?;
            changed_any = true;
            break;
        }
    }

    Ok(changed_any)
}

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
    gbm_surfaces: &[Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>],
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    command: halley_ipc::DpmsCommand,
    _renderer: &Rc<RefCell<GlesRenderer>>,
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
        let mut clear_failed = false;
        for gbm_surface in gbm_surfaces {
            if let Err(err) = gbm_surface.borrow().surface().clear() {
                warn!("tty dpms off: drm surface clear failed: {}", err);
                clear_failed = true;
            }
        }

        let dpms_property_applied =
            match set_connector_dpms_state(&dev.borrow(), &active_modes.borrow(), false) {
                Ok(applied) => applied,
                Err(err) => {
                    warn!("tty dpms off: connector DPMS failed: {}", err);
                    false
                }
            };

        if dpms_property_applied {
            for val in output_frame_pending.borrow_mut().values_mut() {
                *val = false;
            }
            info!("tty dpms: powered off");
        } else if !clear_failed {
            for val in output_frame_pending.borrow_mut().values_mut() {
                *val = false;
            }
            info!("tty dpms: powered off via drm surface clear");
        } else {
            warn!("tty dpms off: no connector DPMS property found on active outputs");
        }
        *dpms_enabled.borrow_mut() = false;
    } else {
        let dpms_property_applied =
            match set_connector_dpms_state(&dev.borrow(), &active_modes.borrow(), true) {
                Ok(applied) => applied,
                Err(err) => {
                    warn!("tty dpms on: connector DPMS failed: {}", err);
                    false
                }
            };

        for val in output_frame_pending.borrow_mut().values_mut() {
            *val = false;
        }

        *dpms_enabled.borrow_mut() = true;
        if dpms_property_applied {
            info!("tty dpms: powering on via connector DPMS");
        } else {
            info!("tty dpms: powering on and waiting for next compositor frame");
        }
    }

    publish_tty_outputs_snapshot(
        &dev.borrow(),
        &active_modes.borrow(),
        *dpms_enabled.borrow(),
        tuning,
    );
}

pub(crate) fn wake_tty_dpms_on_input(
    gbm_surfaces: &[Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>],
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<bool>>,
    renderer: &Rc<RefCell<GlesRenderer>>,
    tuning: &RuntimeTuning,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
) {
    if *dpms_enabled.borrow() {
        return;
    }

    apply_tty_dpms_command(
        gbm_surfaces,
        dev,
        active_modes,
        dpms_enabled,
        halley_ipc::DpmsCommand::On,
        renderer,
        tuning,
        output_frame_pending,
    );

    // Force immediate repaint on all outputs
    {
        let mut pending = output_frame_pending.borrow_mut();
        for val in pending.values_mut() {
            *val = false; // allow immediate queue
        }
    }
}
