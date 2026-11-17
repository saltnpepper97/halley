//! Display power state: `halleyctl dpms` requests, the wlr output-power
//! protocol, and keeping both in step with the hardware.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::LoopHandle;
use calloop::timer::{TimeoutAction, Timer};
use halley_ipc::DpmsCommand;
use smithay::output::Output;
use wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1, zwlr_output_power_v1::ZwlrOutputPowerV1,
};

use super::{Session, SessionDriver};
use crate::wayland::wlr_output_power::{GlobalData, Handler, State};

/// How often a deferred `halleyctl dpms` request checks whether held keys
/// have been let go, and how long it waits for them at most, in case a
/// release never arrives.
const HELD_KEYS_POLL: Duration = Duration::from_millis(10);
const HELD_KEYS_WAIT: Duration = Duration::from_secs(1);

/// Whether a `halleyctl dpms` request should keep waiting before it runs.
///
/// Turning displays off clears each CRTC in a blocking commit, which stalls
/// the event loop. Run from a terminal, the Enter that ran `halleyctl` is
/// still held then: its release arrives only after the stall, past the
/// terminal's own repeat delay, so the terminal repeats Enter, and the
/// release itself is keyboard input, which wakes the displays straight
/// back up. So any request that may power off waits until no key is held.
fn waits_for_held_keys(command: DpmsCommand, keys_held: bool, waited: Duration) -> bool {
    command != DpmsCommand::On && keys_held && waited < HELD_KEYS_WAIT
}

/// A deferred request's target output and what to do with its result.
type PendingDpms<D> = (
    Option<String>,
    Box<dyn FnOnce(&mut Session<D>, Result<(), String>)>,
);

/// Runs `halleyctl dpms` requests once no key is held.
pub(crate) struct Requests<D: SessionDriver> {
    loop_handle: LoopHandle<'static, Session<D>>,
}

impl<D: SessionDriver> Requests<D> {
    pub fn new(loop_handle: LoopHandle<'static, Session<D>>) -> Self {
        Self { loop_handle }
    }
}

impl<D: SessionDriver> Session<D> {
    /// Applies a `halleyctl dpms` request, waiting first for held keys to be
    /// let go when it may power displays off, then hands the result to
    /// `done`.
    pub(crate) fn request_dpms(
        &mut self,
        command: DpmsCommand,
        output: Option<String>,
        done: impl FnOnce(&mut Self, Result<(), String>) + 'static,
    ) {
        if !waits_for_held_keys(command, self.keys_held(), Duration::ZERO) {
            let result = self.apply_dpms_request(command, output.as_deref());
            done(self, result);
            return;
        }
        let pending: Rc<RefCell<Option<PendingDpms<D>>>> =
            Rc::new(RefCell::new(Some((output, Box::new(done)))));
        let started = Instant::now();
        let waiting = pending.clone();
        let inserted = self.dpms_requests.loop_handle.insert_source(
            Timer::from_duration(HELD_KEYS_POLL),
            move |_, _, session| {
                if waits_for_held_keys(command, session.keys_held(), started.elapsed()) {
                    return TimeoutAction::ToDuration(HELD_KEYS_POLL);
                }
                if let Some((output, done)) = waiting.borrow_mut().take() {
                    let result = session.apply_dpms_request(command, output.as_deref());
                    done(session, result);
                }
                TimeoutAction::Drop
            },
        );
        if let Err(err) = inserted {
            eventline::warn!("dpms: couldn't wait for held keys: {err}");
            if let Some((output, done)) = pending.borrow_mut().take() {
                let result = self.apply_dpms_request(command, output.as_deref());
                done(self, result);
            }
        }
    }

    fn apply_dpms_request(
        &mut self,
        command: DpmsCommand,
        output: Option<&str>,
    ) -> Result<(), String> {
        let result = self.driver.apply_dpms(command, output);
        self.sync_output_power();
        result
    }

    fn keys_held(&self) -> bool {
        !self.keyboard_handle().pressed_keys().is_empty()
    }

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
                DpmsCommand::On
            } else {
                DpmsCommand::Off
            },
            Some(&output.name()),
        )
    }
}

smithay::reexports::wayland_server::delegate_global_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrOutputPowerManagerV1: GlobalData] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrOutputPowerManagerV1: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrOutputPowerV1: ()] => State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_off_waits_only_while_keys_are_held_and_at_most_the_limit() {
        let none = Duration::ZERO;
        assert!(waits_for_held_keys(DpmsCommand::Off, true, none));
        assert!(waits_for_held_keys(DpmsCommand::Toggle, true, none));
        assert!(!waits_for_held_keys(DpmsCommand::Off, false, none));
        assert!(!waits_for_held_keys(DpmsCommand::Off, true, HELD_KEYS_WAIT));
    }

    #[test]
    fn power_on_never_waits_for_held_keys() {
        assert!(!waits_for_held_keys(DpmsCommand::On, true, Duration::ZERO));
    }
}
