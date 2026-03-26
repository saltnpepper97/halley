use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use eventline::debug;

use crate::state::Halley;

#[derive(Debug)]
pub(crate) struct VBlankThrottle {
    event_loop: LoopHandle<'static, Halley>,
    last_vblank_at: Option<Instant>,
    throttle_timer_token: Option<RegistrationToken>,
    printed_warning: bool,
    output_name: String,
    samples: u64,
    throttled_count: u64,
    estimated_missed_vblanks: u64,
    smoothed_interval: Option<Duration>,
    max_jitter: Duration,
    min_interval: Option<Duration>,
    max_interval: Duration,
    last_report_at: Option<Instant>,
}

impl VBlankThrottle {
    pub(crate) fn new(event_loop: LoopHandle<'static, Halley>, output_name: String) -> Self {
        Self {
            event_loop,
            last_vblank_at: None,
            throttle_timer_token: None,
            printed_warning: false,
            output_name,
            samples: 0,
            throttled_count: 0,
            estimated_missed_vblanks: 0,
            smoothed_interval: None,
            max_jitter: Duration::ZERO,
            min_interval: None,
            max_interval: Duration::ZERO,
            last_report_at: None,
        }
    }

    fn update_metrics(
        &mut self,
        refresh_interval: Option<Duration>,
        passed: Duration,
        timestamp: Instant,
    ) {
        self.samples = self.samples.saturating_add(1);
        self.min_interval = Some(match self.min_interval {
            Some(current) => current.min(passed),
            None => passed,
        });
        self.max_interval = self.max_interval.max(passed);

        let smoothed = match self.smoothed_interval {
            Some(current) => {
                let current_ns = current.as_nanos();
                let passed_ns = passed.as_nanos();
                let blended = ((current_ns * 7) + passed_ns) / 8;
                let blended = blended.min(u128::from(u64::MAX));
                Duration::from_nanos(blended as u64)
            }
            None => passed,
        };
        self.smoothed_interval = Some(smoothed);

        if let Some(refresh) = refresh_interval {
            let jitter = if passed >= refresh {
                passed - refresh
            } else {
                refresh - passed
            };
            self.max_jitter = self.max_jitter.max(jitter);

            let refresh_ns = refresh.as_nanos();
            let passed_ns = passed.as_nanos();
            if refresh_ns > 0 && passed_ns > refresh_ns + (refresh_ns / 2) {
                let missed = (passed_ns / refresh_ns).saturating_sub(1);
                self.estimated_missed_vblanks = self
                    .estimated_missed_vblanks
                    .saturating_add(missed.min(u128::from(u64::MAX)) as u64);
            }
        }

        let should_report = match self.last_report_at {
            Some(last) => timestamp.saturating_duration_since(last) >= Duration::from_secs(5),
            None => self.samples >= 120,
        };
        if should_report {
            self.last_report_at = Some(timestamp);
            debug!(
                "vblank pacing [{}]: samples={} throttled={} est_missed={} avg={:?} min={:?} max={:?} max_jitter={:?}",
                self.output_name,
                self.samples,
                self.throttled_count,
                self.estimated_missed_vblanks,
                self.smoothed_interval.unwrap_or(Duration::ZERO),
                self.min_interval.unwrap_or(Duration::ZERO),
                self.max_interval,
                self.max_jitter,
            );
        }
    }

    pub(crate) fn throttle(
        &mut self,
        refresh_interval: Option<Duration>,
        timestamp: Instant,
        mut call_vblank: impl FnMut(&mut Halley) + 'static,
    ) -> bool {
        if let Some(token) = self.throttle_timer_token.take() {
            self.event_loop.remove(token);
        }

        if let Some(last) = self.last_vblank_at {
            let passed = timestamp.saturating_duration_since(last);
            self.update_metrics(refresh_interval, passed, timestamp);

            if let Some(refresh) = refresh_interval {
                let min_interval_ns = (refresh.as_nanos() * 3) / 4;
                if passed.as_nanos() < min_interval_ns {
                    if !self.printed_warning {
                        self.printed_warning = true;
                        debug!(
                            "output {} running faster than expected, throttling vblanks: expected refresh {:?}, got vblank after {:?}",
                            self.output_name, refresh, passed
                        );
                    }

                    self.throttled_count = self.throttled_count.saturating_add(1);
                    let remaining = refresh.saturating_sub(passed);
                    let token = self
                        .event_loop
                        .insert_source(Timer::from_duration(remaining), move |_, _, state| {
                            call_vblank(state);
                            TimeoutAction::Drop
                        })
                        .expect("vblank throttle timer should insert");
                    self.throttle_timer_token = Some(token);
                    self.last_vblank_at = Some(timestamp);
                    return true;
                }
            }
        }

        self.last_vblank_at = Some(timestamp);
        false
    }
}
