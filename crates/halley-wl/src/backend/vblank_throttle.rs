use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use eventline::warn;

use crate::state::HalleyWlState;

#[derive(Debug)]
pub(crate) struct VBlankThrottle {
    event_loop: LoopHandle<'static, HalleyWlState>,
    last_vblank_at: Option<Instant>,
    throttle_timer_token: Option<RegistrationToken>,
    printed_warning: bool,
    output_name: String,
}

impl VBlankThrottle {
    pub(crate) fn new(
        event_loop: LoopHandle<'static, HalleyWlState>,
        output_name: String,
    ) -> Self {
        Self {
            event_loop,
            last_vblank_at: None,
            throttle_timer_token: None,
            printed_warning: false,
            output_name,
        }
    }

    pub(crate) fn throttle(
        &mut self,
        refresh_interval: Option<Duration>,
        timestamp: Instant,
        mut call_vblank: impl FnMut(&mut HalleyWlState) + 'static,
    ) -> bool {
        if let Some(token) = self.throttle_timer_token.take() {
            self.event_loop.remove(token);
        }

        if let Some((last, refresh)) = self.last_vblank_at.zip(refresh_interval) {
            let passed = timestamp.saturating_duration_since(last);
            if passed < refresh / 2 {
                if !self.printed_warning {
                    self.printed_warning = true;
                    warn!(
                        "output {} running faster than expected, throttling vblanks: expected refresh {:?}, got vblank after {:?}",
                        self.output_name,
                        refresh,
                        passed
                    );
                }

                let remaining = refresh.saturating_sub(passed);
                let token = self
                    .event_loop
                    .insert_source(Timer::from_duration(remaining), move |_, _, state| {
                        call_vblank(state);
                        TimeoutAction::Drop
                    })
                    .expect("vblank throttle timer should insert");
                self.throttle_timer_token = Some(token);
                return true;
            }
        }

        self.last_vblank_at = Some(timestamp);
        false
    }
}
