use std::time::Duration;

use smithay::utils::{Clock, Monotonic};

/// Presentation history for one output.
///
/// Fixed-refresh outputs use the last real VBlank timestamp to predict the
/// next presentation. Backends without presentation metadata (currently
/// nested winit) leave `refresh_interval` unset and sample at `now`.
#[derive(Clone, Debug)]
pub struct FrameClock {
    refresh_interval: Option<Duration>,
    last_presentation_time: Option<Duration>,
    vrr: bool,
}

impl FrameClock {
    pub fn new(refresh_interval: Option<Duration>) -> Self {
        Self {
            refresh_interval: refresh_interval.filter(|interval| !interval.is_zero()),
            last_presentation_time: None,
            vrr: false,
        }
    }

    pub fn presented(&mut self, presentation_time: Option<Duration>) {
        if let Some(presentation_time) = presentation_time.filter(|time| !time.is_zero()) {
            self.last_presentation_time = Some(presentation_time);
        }
    }

    pub fn reset(&mut self) {
        self.last_presentation_time = None;
    }

    pub fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval
    }

    pub fn set_vrr(&mut self, vrr: bool) {
        if self.vrr != vrr {
            self.vrr = vrr;
            self.reset();
        }
    }

    pub fn next_presentation_time(&self, now: Duration) -> Duration {
        let Some(interval) = self.refresh_interval else {
            return now;
        };
        let Some(last) = self.last_presentation_time else {
            return now;
        };

        if now <= last {
            return last + interval;
        }

        let elapsed_ns = (now - last).as_nanos();
        let interval_ns = interval.as_nanos();
        let periods = elapsed_ns / interval_ns + 1;
        if self.vrr && periods > 1 {
            return now;
        }
        last + interval.mul_f64(periods as f64)
    }
}

pub fn monotonic_now() -> Duration {
    Clock::<Monotonic>::new().now().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_uses_now_with_or_without_a_refresh_rate() {
        let now = Duration::from_secs(12);
        for interval in [None, Some(Duration::from_millis(10))] {
            let clock = FrameClock::new(interval);
            assert_eq!(clock.next_presentation_time(now), now, "{interval:?}");
        }
    }

    #[test]
    fn prediction_retains_cadence_after_missing_cycles_or_timestamp() {
        let interval = Duration::from_millis(10);
        let mut clock = FrameClock::new(Some(interval));
        clock.presented(Some(Duration::from_millis(100)));

        assert_eq!(
            clock.next_presentation_time(Duration::from_millis(104)),
            Duration::from_millis(110)
        );
        assert_eq!(
            clock.next_presentation_time(Duration::from_millis(125)),
            Duration::from_millis(130)
        );
        clock.presented(None);
        assert_eq!(
            clock.next_presentation_time(Duration::from_millis(104)),
            Duration::from_millis(110)
        );
    }

    #[test]
    fn reset_discards_stale_presentation_history() {
        let mut clock = FrameClock::new(Some(Duration::from_millis(10)));
        clock.presented(Some(Duration::from_millis(100)));
        clock.reset();

        let now = Duration::from_secs(12);
        assert_eq!(clock.next_presentation_time(now), now);
    }

    #[test]
    fn outputs_with_different_refresh_rates_remain_independent() {
        let mut fast = FrameClock::new(Some(Duration::from_secs_f64(1.0 / 179.998)));
        let mut slow = FrameClock::new(Some(Duration::from_secs_f64(1.0 / 60.0)));
        let presented = Duration::from_secs(5);
        fast.presented(Some(presented));
        slow.presented(Some(presented));

        let now = presented + Duration::from_millis(1);
        assert!(fast.next_presentation_time(now) < slow.next_presentation_time(now));
    }

    #[test]
    fn vrr_can_present_immediately_after_missing_a_refresh_period() {
        let mut clock = FrameClock::new(Some(Duration::from_millis(10)));
        clock.set_vrr(true);
        clock.presented(Some(Duration::from_millis(100)));

        assert_eq!(
            clock.next_presentation_time(Duration::from_millis(125)),
            Duration::from_millis(125)
        );
        assert!(clock.vrr);
    }
}
