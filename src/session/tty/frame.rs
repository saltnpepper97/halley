use std::time::Duration;

use calloop::RegistrationToken;

use crate::frame_clock::FrameClock;

/// Animations repaint conservatively with unmodified Smithay; diagnostics
/// alone still request another sample without resetting buffer ages.
pub(crate) struct FrameDemand {
    pub keep_redrawing: bool,
    pub force_full_repaint: bool,
}

impl FrameDemand {
    pub fn new(geometry_animating: bool, local_animating: bool, fps_overlay_visible: bool) -> Self {
        Self {
            keep_redrawing: geometry_animating || local_animating || fps_overlay_visible,
            force_full_repaint: geometry_animating || local_animating,
        }
    }
}

/// The redraw work an output owes and the kernel/timer event that currently
/// gates it. Keeping these transitions together prevents input, DRM, and
/// timer handlers from independently interpreting the same state.
#[derive(Debug, Default)]
enum RedrawState {
    #[default]
    Idle,
    Suspended,
    Queued,
    WaitingForVBlank {
        redraw_needed: bool,
    },
    WaitingForEstimatedVBlank(RegistrationToken),
    WaitingForEstimatedVBlankAndQueued(RegistrationToken),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EstimatedVblankTimer {
    AlreadyArmed,
    ArmAfter(Duration),
}

/// Work that becomes safe when the kernel reports a page flip.
///
/// A queued scene change takes precedence over releasing another client frame
/// callback: its buffers have not been latched yet. A successful unchanged
/// sample can release callbacks and queue the next continuous sample together.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum VblankAction {
    Ignore,
    Redraw,
    SendCallbacks,
}

/// Result of filtering a kernel VBlank through the output's fixed-refresh
/// cadence. Some drivers occasionally report a second event far too early;
/// delaying its completion avoids turning that glitch into a compositor-side
/// busy loop.
pub(super) struct VblankThrottleResult {
    pub delay: Option<Duration>,
    pub cancel_timer: Option<RegistrationToken>,
    pub warn: bool,
}

pub(super) struct OutputFrameState {
    clock: FrameClock,
    redraw: RedrawState,
    last_camera_sample: Duration,
    keep_redrawing: bool,
    skipped_scene_ready: bool,
    frame_callback_sequence: u32,
    last_vblank_timestamp: Option<Duration>,
    vblank_throttle_timer: Option<RegistrationToken>,
    vblank_throttle_warned: bool,
}

impl OutputFrameState {
    pub fn new(refresh_interval: Duration) -> Self {
        Self {
            clock: FrameClock::new(Some(refresh_interval)),
            redraw: RedrawState::default(),
            last_camera_sample: crate::frame_clock::monotonic_now(),
            keep_redrawing: false,
            skipped_scene_ready: false,
            frame_callback_sequence: 0,
            last_vblank_timestamp: None,
            vblank_throttle_timer: None,
            vblank_throttle_warned: false,
        }
    }

    pub fn advance_frame_callback_sequence(&mut self) {
        self.frame_callback_sequence = self.frame_callback_sequence.wrapping_add(1);
    }

    pub fn frame_callback_sequence(&self) -> u32 {
        self.frame_callback_sequence
    }

    pub fn throttle_vblank(
        &mut self,
        timestamp: Option<Duration>,
        refresh_interval: Duration,
    ) -> VblankThrottleResult {
        let cancel_timer = self.vblank_throttle_timer.take();
        if matches!(self.redraw, RedrawState::Suspended) {
            return VblankThrottleResult {
                delay: None,
                cancel_timer,
                warn: false,
            };
        }
        let Some(timestamp) = timestamp else {
            return VblankThrottleResult {
                delay: None,
                cancel_timer,
                warn: false,
            };
        };

        if let Some(last) = self.last_vblank_timestamp {
            let passed = timestamp.saturating_sub(last);
            if passed < refresh_interval / 2 {
                let warn = !self.vblank_throttle_warned;
                self.vblank_throttle_warned = true;
                return VblankThrottleResult {
                    delay: Some(refresh_interval.saturating_sub(passed)),
                    cancel_timer,
                    warn,
                };
            }
        }

        self.last_vblank_timestamp = Some(timestamp);
        VblankThrottleResult {
            delay: None,
            cancel_timer,
            warn: false,
        }
    }

    pub fn vblank_throttle_timer_armed(&mut self, token: RegistrationToken) {
        self.vblank_throttle_timer = Some(token);
    }

    pub fn vblank_throttle_timer_fired(&mut self) {
        self.vblank_throttle_timer = None;
    }

    pub fn queue_redraw(&mut self) {
        self.redraw = match std::mem::take(&mut self.redraw) {
            RedrawState::Suspended => RedrawState::Suspended,
            RedrawState::Idle => RedrawState::Queued,
            RedrawState::WaitingForEstimatedVBlank(token) => {
                RedrawState::WaitingForEstimatedVBlankAndQueued(token)
            }
            value @ (RedrawState::Queued | RedrawState::WaitingForEstimatedVBlankAndQueued(_)) => {
                value
            }
            RedrawState::WaitingForVBlank { .. } => RedrawState::WaitingForVBlank {
                redraw_needed: true,
            },
        };
    }

    pub fn is_redraw_queued(&self) -> bool {
        matches!(
            self.redraw,
            RedrawState::Queued | RedrawState::WaitingForEstimatedVBlankAndQueued(_)
        )
    }

    pub fn replace_clock(&mut self, refresh_interval: Duration) {
        self.clock = FrameClock::new(Some(refresh_interval));
        self.last_camera_sample = crate::frame_clock::monotonic_now();
        self.keep_redrawing = false;
        self.skipped_scene_ready = false;
    }

    pub fn set_vrr(&mut self, vrr: bool) {
        self.clock.set_vrr(vrr);
    }

    pub fn next_frame_sample(&mut self, now: Duration) -> (Duration, Duration) {
        let target = self.clock.next_presentation_time(now);
        let dt = target.saturating_sub(self.last_camera_sample);
        self.last_camera_sample = target;
        (target, dt)
    }

    pub fn on_vblank(&mut self, presented: Option<Duration>) -> (VblankAction, Option<String>) {
        if matches!(self.redraw, RedrawState::Suspended) {
            return (VblankAction::Ignore, None);
        }
        self.clock.presented(presented);
        let (redraw_needed, unexpected_state) = match std::mem::take(&mut self.redraw) {
            RedrawState::Suspended => {
                self.redraw = RedrawState::Suspended;
                return (VblankAction::Ignore, None);
            }
            RedrawState::WaitingForVBlank { redraw_needed } => {
                (redraw_needed || self.keep_redrawing, None)
            }
            other => (true, Some(format!("{other:?}"))),
        };
        self.redraw = if redraw_needed {
            RedrawState::Queued
        } else {
            RedrawState::Idle
        };

        let action = if redraw_needed {
            VblankAction::Redraw
        } else {
            VblankAction::SendCallbacks
        };
        (action, unexpected_state)
    }

    /// Records a real page flip and returns an estimated-VBlank timer that
    /// became obsolete, if one was armed.
    pub fn frame_submitted(&mut self, keep_redrawing: bool) -> Option<RegistrationToken> {
        let timer = match std::mem::take(&mut self.redraw) {
            RedrawState::WaitingForEstimatedVBlank(token)
            | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => Some(token),
            _ => None,
        };
        self.keep_redrawing = keep_redrawing;
        self.redraw = RedrawState::WaitingForVBlank {
            redraw_needed: keep_redrawing,
        };
        timer
    }

    /// Records a render with no page flip. A previously armed fallback is
    /// reused; otherwise the caller receives the delay for one new timer.
    /// `scene_ready` distinguishes successful unchanged scenes from render
    /// failures, whose client callbacks must wait until rendering succeeds.
    pub fn frame_skipped(
        &mut self,
        keep_redrawing: bool,
        scene_ready: bool,
        now: Duration,
    ) -> EstimatedVblankTimer {
        self.keep_redrawing = keep_redrawing;
        self.skipped_scene_ready = scene_ready;
        match std::mem::take(&mut self.redraw) {
            RedrawState::Queued => {}
            RedrawState::WaitingForEstimatedVBlank(token)
            | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => {
                self.redraw = RedrawState::WaitingForEstimatedVBlank(token);
                return EstimatedVblankTimer::AlreadyArmed;
            }
            RedrawState::Suspended | RedrawState::Idle | RedrawState::WaitingForVBlank { .. } => {
                unreachable!("frame_skipped called from an unexpected redraw state")
            }
        }

        let due = self.clock.next_presentation_time(now);
        let delay = due.saturating_sub(now);
        // Without a presentation timestamp the clock samples at `now`.
        // A continuously sampled, unchanged scene must still wait a refresh
        // interval, rather than turning the one-shot callback fallback into
        // a repeating 1 ms timer.
        let delay = if keep_redrawing && delay.is_zero() {
            self.clock
                .refresh_interval()
                .unwrap_or(Duration::from_secs_f64(1.0 / 60.0))
        } else {
            delay
        };
        EstimatedVblankTimer::ArmAfter(delay.max(Duration::from_millis(1)))
    }

    pub fn timer_armed(&mut self, token: RegistrationToken) {
        self.redraw = RedrawState::WaitingForEstimatedVBlank(token);
    }

    pub fn estimated_vblank_fired(&mut self) -> Result<bool, String> {
        match std::mem::take(&mut self.redraw) {
            RedrawState::Suspended => {
                self.redraw = RedrawState::Suspended;
                Ok(false)
            }
            RedrawState::WaitingForEstimatedVBlank(_) if self.keep_redrawing => {
                self.redraw = RedrawState::Queued;
                // No new pixels were needed, so clients can advance while
                // the diagnostic samples again. A failed render cannot do this.
                Ok(self.skipped_scene_ready)
            }
            RedrawState::WaitingForEstimatedVBlank(_) => Ok(true),
            RedrawState::WaitingForEstimatedVBlankAndQueued(_) => {
                self.redraw = RedrawState::Queued;
                Ok(false)
            }
            other => Err(format!("{other:?}")),
        }
    }

    pub fn suspend(&mut self, now: Duration) -> Vec<RegistrationToken> {
        let mut timers = Vec::with_capacity(2);
        if let Some(token) = self.vblank_throttle_timer.take() {
            timers.push(token);
        }
        if let Some(token) = match std::mem::take(&mut self.redraw) {
            RedrawState::WaitingForEstimatedVBlank(token)
            | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => Some(token),
            _ => None,
        } {
            timers.push(token);
        }
        self.clock.reset();
        self.last_vblank_timestamp = None;
        self.vblank_throttle_warned = false;
        self.last_camera_sample = now;
        self.keep_redrawing = false;
        self.skipped_scene_ready = false;
        self.redraw = RedrawState::Suspended;
        timers
    }

    pub fn resume(&mut self, now: Duration) {
        debug_assert!(matches!(self.redraw, RedrawState::Suspended));
        self.clock.reset();
        self.last_vblank_timestamp = None;
        self.last_camera_sample = now;
        self.keep_redrawing = false;
        self.skipped_scene_ready = false;
        self.redraw = RedrawState::Queued;
    }

    /// A lost display epoch invalidates both real and estimated completion
    /// events, even if the notification preceding the loss was missed.
    pub fn recover(&mut self, now: Duration, renderable: bool) -> Vec<RegistrationToken> {
        let timers = self.suspend(now);
        if renderable {
            self.resume(now);
        }
        timers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> OutputFrameState {
        OutputFrameState::new(Duration::from_millis(10))
    }

    fn registration_token() -> RegistrationToken {
        let event_loop = calloop::EventLoop::<()>::try_new().expect("test event loop");
        event_loop
            .handle()
            .insert_source(calloop::timer::Timer::immediate(), |_, _, _| {
                calloop::timer::TimeoutAction::Drop
            })
            .expect("test timer")
    }

    #[test]
    fn queued_requests_are_idempotent() {
        let mut state = state();
        state.queue_redraw();
        state.queue_redraw();

        assert!(state.is_redraw_queued());
    }

    #[test]
    fn request_while_waiting_for_vblank_is_not_lost() {
        let mut state = state();
        state.queue_redraw();
        assert_eq!(state.frame_submitted(false), None);

        state.queue_redraw();
        let (action, unexpected) = state.on_vblank(None);

        assert_eq!(unexpected, None);
        assert_eq!(action, VblankAction::Redraw);
        assert!(state.is_redraw_queued());
    }

    #[test]
    fn unfinished_animation_requests_the_next_real_frame() {
        let mut state = state();
        state.queue_redraw();
        state.frame_submitted(true);

        assert_eq!(state.on_vblank(None), (VblankAction::Redraw, None));
        assert!(state.is_redraw_queued());
    }

    #[test]
    fn local_animation_keeps_vblank_cadence_with_conservative_repaints() {
        let animation = FrameDemand::new(false, true, false);
        assert!(animation.force_full_repaint);
        let mut state = state();
        state.queue_redraw();
        state.frame_submitted(animation.keep_redrawing);
        assert_eq!(state.on_vblank(None), (VblankAction::Redraw, None));
        // A label may still be advancing below its visible threshold. Such a
        // sample owes another paced frame even when no pixels changed.
        assert_eq!(
            state.frame_skipped(animation.keep_redrawing, true, Duration::from_millis(100)),
            EstimatedVblankTimer::ArmAfter(Duration::from_millis(10))
        );
        state.timer_armed(registration_token());
        assert_eq!(state.estimated_vblank_fired(), Ok(true));
        let settled = FrameDemand::new(false, false, false);
        state.frame_submitted(settled.keep_redrawing);
        assert_eq!(state.on_vblank(None), (VblankAction::SendCallbacks, None));
        assert!(!state.is_redraw_queued());
    }

    #[test]
    fn fps_samples_continue_after_submitted_and_unchanged_frames_then_stop_when_disabled() {
        let fps = FrameDemand::new(false, false, true);
        assert!(!fps.force_full_repaint);
        let mut state = state();
        state.queue_redraw();
        state.frame_submitted(fps.keep_redrawing);
        assert!(!state.is_redraw_queued());
        assert_eq!(
            state.on_vblank(Some(Duration::from_millis(100))),
            (VblankAction::Redraw, None)
        );

        // Selective damage can submit no pixels while FPS sampling remains
        // active. Every sample still waits for a refresh-paced timer.
        for tick in 0..100 {
            let now = Duration::from_millis(100 + tick * 10);
            assert_eq!(
                state.frame_skipped(fps.keep_redrawing, true, now),
                EstimatedVblankTimer::ArmAfter(Duration::from_millis(10))
            );
            state.timer_armed(registration_token());
            assert!(!state.is_redraw_queued());
            assert_eq!(
                state.frame_skipped(fps.keep_redrawing, true, now + Duration::from_millis(1)),
                EstimatedVblankTimer::AlreadyArmed
            );
            assert_eq!(state.estimated_vblank_fired(), Ok(true));
            assert!(state.is_redraw_queued());
        }

        let disabled = FrameDemand::new(false, false, false);
        state.frame_skipped(disabled.keep_redrawing, true, Duration::from_millis(1100));
        state.timer_armed(registration_token());
        assert_eq!(state.estimated_vblank_fired(), Ok(true));
        assert!(!state.is_redraw_queued());
    }

    #[test]
    fn continuous_samples_without_presentation_timestamps_wait_for_current_refresh() {
        let fps = FrameDemand::new(false, false, true);
        let mut state = state();
        state.queue_redraw();
        for tick in 0..3 {
            assert_eq!(
                state.frame_skipped(
                    fps.keep_redrawing,
                    true,
                    Duration::from_millis(100 + tick * 10)
                ),
                EstimatedVblankTimer::ArmAfter(Duration::from_millis(10))
            );
            state.timer_armed(registration_token());
            assert!(!state.is_redraw_queued());
            assert_eq!(state.estimated_vblank_fired(), Ok(true));
        }
        state.replace_clock(Duration::from_millis(20));
        assert_eq!(
            state.frame_skipped(fps.keep_redrawing, true, Duration::from_millis(130)),
            EstimatedVblankTimer::ArmAfter(Duration::from_millis(20))
        );
    }

    #[test]
    fn failed_sample_and_queued_scene_change_wait_for_rendering_before_callbacks() {
        let fps = FrameDemand::new(false, false, true);
        let mut state = state();
        state.queue_redraw();
        state.frame_skipped(fps.keep_redrawing, false, Duration::from_secs(5));
        state.timer_armed(registration_token());
        assert_eq!(state.estimated_vblank_fired(), Ok(false));
        assert!(state.is_redraw_queued());

        state.frame_skipped(fps.keep_redrawing, true, Duration::from_secs(5));
        state.timer_armed(registration_token());
        // Input or a client commit after the sample owes a fresh scene first.
        state.queue_redraw();
        assert_eq!(state.estimated_vblank_fired(), Ok(false));
        assert!(state.is_redraw_queued());
    }

    #[test]
    fn settled_frame_returns_to_idle_after_vblank() {
        let mut state = state();
        state.queue_redraw();
        state.frame_submitted(false);

        assert_eq!(state.on_vblank(None), (VblankAction::SendCallbacks, None));
        assert!(!state.is_redraw_queued());
    }

    #[test]
    fn skipped_frame_arms_a_nonzero_fallback_delay() {
        let mut state = state();
        state.queue_redraw();

        assert_eq!(
            state.frame_skipped(false, true, Duration::from_secs(5)),
            EstimatedVblankTimer::ArmAfter(Duration::from_millis(1))
        );
    }

    #[test]
    fn estimated_vblank_releases_callbacks_after_a_settled_skipped_frame() {
        let mut state = state();
        state.queue_redraw();
        state.frame_skipped(false, true, Duration::from_secs(5));
        state.timer_armed(registration_token());

        assert_eq!(state.estimated_vblank_fired(), Ok(true));
        assert!(!state.is_redraw_queued());
    }

    #[test]
    fn redraw_queued_during_estimated_wait_takes_precedence_over_callbacks() {
        let mut state = state();
        state.queue_redraw();
        state.frame_skipped(false, true, Duration::from_secs(5));
        state.timer_armed(registration_token());
        state.queue_redraw();

        assert_eq!(state.estimated_vblank_fired(), Ok(false));
        assert!(state.is_redraw_queued());
    }

    #[test]
    fn vblank_throttle_delays_only_implausibly_early_events() {
        let mut state = state();
        let refresh = Duration::from_millis(10);

        let first = state.throttle_vblank(Some(Duration::from_millis(100)), refresh);
        assert_eq!(first.delay, None);
        assert!(!first.warn);

        let early = state.throttle_vblank(Some(Duration::from_millis(103)), refresh);
        assert_eq!(early.delay, Some(Duration::from_millis(7)));
        assert!(early.warn);

        let normal = state.throttle_vblank(Some(Duration::from_millis(110)), refresh);
        assert_eq!(normal.delay, None);
        assert!(!normal.warn);
    }

    #[test]
    fn vblank_throttle_warning_is_bounded_and_replaces_its_timer() {
        let mut state = state();
        let refresh = Duration::from_millis(10);
        assert_eq!(
            state
                .throttle_vblank(Some(Duration::from_millis(100)), refresh)
                .delay,
            None
        );

        let early = state.throttle_vblank(Some(Duration::from_millis(102)), refresh);
        assert!(early.warn);
        state.vblank_throttle_timer_armed(registration_token());

        let repeated = state.throttle_vblank(Some(Duration::from_millis(103)), refresh);
        assert_eq!(repeated.delay, Some(Duration::from_millis(7)));
        assert!(!repeated.warn);
        assert!(repeated.cancel_timer.is_some());
    }

    #[test]
    fn suspended_output_ignores_redraws_and_late_vblanks_until_resume() {
        let mut state = state();
        state.queue_redraw();
        state.frame_submitted(false);
        drop(state.suspend(Duration::from_secs(1)));

        state.queue_redraw();
        assert!(!state.is_redraw_queued());
        assert_eq!(state.on_vblank(None), (VblankAction::Ignore, None));
        assert!(!state.is_redraw_queued());

        state.resume(Duration::from_secs(2));
        assert!(state.is_redraw_queued());
    }

    #[test]
    fn recovery_without_prepare_notification_releases_a_lost_page_flip() {
        let mut state = state();
        state.queue_redraw();
        state.on_vblank(Some(Duration::from_millis(100)));
        state.next_frame_sample(Duration::from_millis(104));
        state.advance_frame_callback_sequence();
        state.frame_submitted(true);
        // The backend invalidated this submitted frame during sleep. An
        // ordinary redraw request cannot release its completion gate.
        state.queue_redraw();
        assert!(!state.is_redraw_queued());

        let now = Duration::from_secs(20);
        assert!(state.recover(now, true).is_empty());
        assert!(state.is_redraw_queued());
        assert_eq!(state.next_frame_sample(now), (now, Duration::ZERO));
        assert_eq!(state.frame_callback_sequence(), 1);
        state.frame_submitted(false);
        assert_eq!(
            state.on_vblank(Some(now + Duration::from_millis(10))),
            (VblankAction::SendCallbacks, None)
        );
        assert!(!state.is_redraw_queued());
    }

    #[test]
    fn recovery_cancels_estimated_and_throttle_timers_before_a_fresh_frame() {
        let mut event_loop = calloop::EventLoop::<usize>::try_new().unwrap();
        let timer = || {
            event_loop
                .handle()
                .insert_source(calloop::timer::Timer::immediate(), |_, _, fired| {
                    *fired += 1;
                    calloop::timer::TimeoutAction::Drop
                })
                .unwrap()
        };
        let mut state = state();
        state.queue_redraw();
        state.frame_skipped(true, true, Duration::from_secs(1));
        let estimated = timer();
        let throttle = timer();
        let _unrelated = timer();
        assert_ne!(estimated, throttle);
        state.timer_armed(estimated);
        state.queue_redraw();
        state.vblank_throttle_timer_armed(throttle);

        let now = Duration::from_secs(30);
        let cancelled = state.recover(now, true);
        assert_eq!(cancelled, vec![throttle, estimated]);
        for token in cancelled {
            event_loop.handle().remove(token);
        }
        let mut fired = 0;
        event_loop
            .dispatch(Duration::from_millis(5), &mut fired)
            .unwrap();
        assert_eq!(fired, 1, "only the unrelated event source may run");
        assert_eq!(state.frame_submitted(false), None);
        assert_eq!(
            state.on_vblank(Some(now)),
            (VblankAction::SendCallbacks, None)
        );
        assert!(!state.is_redraw_queued());
    }

    #[test]
    fn powered_off_output_remains_suspended_across_recovery_until_wake() {
        let mut state = state();
        state.queue_redraw();
        state.frame_submitted(true);
        assert!(state.recover(Duration::from_secs(1), false).is_empty());
        state.queue_redraw();
        assert!(!state.is_redraw_queued());
        assert_eq!(state.estimated_vblank_fired(), Ok(false));
        assert_eq!(
            state.on_vblank(Some(Duration::from_secs(1))),
            (VblankAction::Ignore, None)
        );
        assert!(state.recover(Duration::from_secs(2), false).is_empty());
        assert!(!state.is_redraw_queued());
        let now = Duration::from_secs(10);
        assert!(state.recover(now, true).is_empty());
        assert!(state.is_redraw_queued());
        assert_eq!(state.next_frame_sample(now), (now, Duration::ZERO));
    }

    #[test]
    fn suspended_late_vblanks_do_not_seed_timing_or_arm_throttle_timers() {
        let mut state = state();
        state.recover(Duration::from_millis(100), false);
        for timestamp in [101, 102] {
            let time = Duration::from_millis(timestamp);
            let throttle = state.throttle_vblank(Some(time), Duration::from_millis(10));
            assert_eq!(throttle.delay, None);
            assert!(!throttle.warn);
            assert_eq!(state.on_vblank(Some(time)), (VblankAction::Ignore, None));
            assert_eq!(state.clock.next_presentation_time(time), time);
        }
    }

    #[test]
    fn recovering_one_output_preserves_the_other_outputs_page_flip() {
        let mut first = state();
        let mut second = OutputFrameState::new(Duration::from_millis(20));
        for state in [&mut first, &mut second] {
            state.queue_redraw();
            state.on_vblank(Some(Duration::from_millis(100)));
            state.frame_submitted(false);
        }
        assert!(first.recover(Duration::from_secs(10), true).is_empty());
        assert!(first.is_redraw_queued());
        assert!(!second.is_redraw_queued());
        assert_eq!(
            second.on_vblank(Some(Duration::from_millis(120))),
            (VblankAction::SendCallbacks, None)
        );
    }
}
