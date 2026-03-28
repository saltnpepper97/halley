use super::*;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::backend::tty::drm::collect_outputs_for_ipc;
use crate::backend::tty::drm::TtyDrmOutput;

pub(crate) fn sync_tty_dpms_state(
    outputs: &[TtyDrmOutput],
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
) {
    let mut next = HashMap::new();
    let current = dpms_enabled.borrow();
    for output in outputs {
        next.insert(
            output.connector_name.clone(),
            current
                .get(output.connector_name.as_str())
                .copied()
                .unwrap_or(true),
        );
    }
    drop(current);
    *dpms_enabled.borrow_mut() = next;
}

pub(crate) fn tty_output_dpms_enabled(
    dpms_enabled: &HashMap<String, bool>,
    output_name: &str,
) -> bool {
    dpms_enabled.get(output_name).copied().unwrap_or(true)
}

pub(crate) fn any_tty_output_dpms_enabled(dpms_enabled: &HashMap<String, bool>) -> bool {
    dpms_enabled.values().copied().any(|enabled| enabled)
}

pub(crate) fn publish_tty_outputs_snapshot(
    dev: &DrmDevice,
    active_modes: &HashMap<String, drm_control::Mode>,
    dpms_enabled: &HashMap<String, bool>,
    tuning: &RuntimeTuning,
    st: &Halley,
) {
    let vrr_support: HashMap<String, String> = HashMap::new();
    let mut outputs = collect_outputs_for_ipc(
        dev,
        active_modes,
        tuning,
        &vrr_support,
        &st.model.fullscreen_state.direct_scanout,
    );
    for output in &mut outputs {
        if active_modes.contains_key(&output.name)
            && !tty_output_dpms_enabled(dpms_enabled, output.name.as_str())
        {
            output.enabled = false;
            output.current_mode = None;
            for mode in &mut output.modes {
                mode.current = false;
            }
        }
    }
    publish_outputs(outputs);
}

pub(crate) fn apply_tty_dpms_command(
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    command: halley_ipc::DpmsCommand,
    output: Option<&str>,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    tuning: &RuntimeTuning,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    dpms_just_woke_outputs: &Rc<RefCell<HashSet<String>>>,
    st: &mut Halley,
) -> bool {
    let target_outputs: Vec<String> = {
        let outputs = outputs.borrow();
        match output {
            Some(output) => {
                let Some(found) = outputs
                    .iter()
                    .find(|candidate| candidate.connector_name == output)
                    .map(|candidate| candidate.connector_name.clone())
                else {
                    return false;
                };
                vec![found]
            }
            None => outputs
                .iter()
                .map(|output| output.connector_name.clone())
                .collect(),
        }
    };

    let target_enabled = match command {
        halley_ipc::DpmsCommand::On => true,
        halley_ipc::DpmsCommand::Off => false,
        halley_ipc::DpmsCommand::Toggle => {
            let current = dpms_enabled.borrow();
            !target_outputs
                .iter()
                .all(|name| tty_output_dpms_enabled(&current, name.as_str()))
        }
    };

    let already_matches = {
        let current = dpms_enabled.borrow();
        target_outputs
            .iter()
            .all(|name| tty_output_dpms_enabled(&current, name.as_str()) == target_enabled)
    };
    if already_matches {
        return false;
    }

    if !target_enabled {
        for output in outputs.borrow().iter() {
            if !target_outputs.contains(&output.connector_name) {
                continue;
            }
            if let Err(err) = output.compositor.borrow_mut().clear() {
                warn!(
                    "tty dpms off: clear failed for {}: {}",
                    output.connector_name, err
                );
            }
            output_frame_pending
                .borrow_mut()
                .insert(output.connector_name.clone(), false);
            dpms_just_woke_outputs
                .borrow_mut()
                .remove(output.connector_name.as_str());
        }
        {
            let mut current = dpms_enabled.borrow_mut();
            for output in &target_outputs {
                current.insert(output.clone(), false);
            }
        }
        info!(
            "tty dpms: powered off outputs {}",
            target_outputs.join(", ")
        );
    } else {
        {
            let mut current = dpms_enabled.borrow_mut();
            for output in &target_outputs {
                current.insert(output.clone(), true);
            }
        }
        {
            let mut woke = dpms_just_woke_outputs.borrow_mut();
            for output in &target_outputs {
                woke.insert(output.clone());
            }
        }
        st.input.interaction_state.dpms_just_woke = true;
        info!(
            "tty dpms: powering on outputs {}",
            target_outputs.join(", ")
        );
    }

    publish_tty_outputs_snapshot(
        &dev.borrow(),
        &active_modes.borrow(),
        &dpms_enabled.borrow(),
        tuning,
        st,
    );

    true
}

pub(crate) fn wake_tty_dpms_on_input(
    dev: &Rc<RefCell<DrmDevice>>,
    active_modes: &Rc<RefCell<HashMap<String, drm_control::Mode>>>,
    dpms_enabled: &Rc<RefCell<HashMap<String, bool>>>,
    outputs: &Rc<RefCell<Vec<TtyDrmOutput>>>,
    tuning: &RuntimeTuning,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    dpms_just_woke_outputs: &Rc<RefCell<HashSet<String>>>,
    target_output: Option<&str>,
    st: &mut Halley,
) -> bool {
    let focused_monitor = st.focused_monitor().to_string();
    let output = target_output.unwrap_or(focused_monitor.as_str());
    let current = dpms_enabled.borrow();

    // Preserve the old "wake everything immediately" behavior when the
    // entire tty layout is asleep. Per-output wake is still used when only
    // some outputs are DPMS-disabled.
    let wake_all = !any_tty_output_dpms_enabled(&current);
    if !wake_all && tty_output_dpms_enabled(&current, output) {
        return false;
    }
    drop(current);

    apply_tty_dpms_command(
        dev,
        active_modes,
        dpms_enabled,
        halley_ipc::DpmsCommand::On,
        if wake_all { None } else { Some(output) },
        outputs,
        tuning,
        output_frame_pending,
        dpms_just_woke_outputs,
        st,
    )
}
