use smithay::output::Output;
use wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1, zwlr_output_power_v1::ZwlrOutputPowerV1,
};

use super::{Session, SessionDriver};
use crate::wayland::wlr_output_power::{GlobalData, Handler, State};

impl<D: SessionDriver> Session<D> {
    pub(crate) fn sync_output_power(&mut self) {
        let outputs = self
            .driver
            .output_states()
            .into_iter()
            .filter_map(|state| {
                self.driver
                    .output_power(&state.output)
                    .map(|on| (state.output, on))
            })
            .collect();
        self.wayland.wlr_output_power_state.sync(outputs);
        self.sync_image_capture();
        // Include partial hardware success, IPC, and input wake transitions.
        crate::wayland::session_lock::confirm_unlit_outputs(self);
    }
}

impl<D: SessionDriver> Handler for Session<D> {
    fn output_power_state(&mut self) -> &mut State {
        &mut self.wayland.wlr_output_power_state
    }

    fn refresh_output_power(&mut self) {
        self.driver.refresh_output_power_support();
        self.sync_output_power();
    }

    fn set_output_power(&mut self, output: &Output, on: bool) -> Result<(), String> {
        if self.driver.output_power(output).is_none() {
            return Err("output power is unavailable".into());
        }
        self.driver.apply_dpms(
            if on {
                halley_ipc::DpmsCommand::On
            } else {
                halley_ipc::DpmsCommand::Off
            },
            Some(&output.name()),
        )
    }
}

smithay::reexports::wayland_server::delegate_global_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrOutputPowerManagerV1: GlobalData] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrOutputPowerManagerV1: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrOutputPowerV1: ()] => State);
