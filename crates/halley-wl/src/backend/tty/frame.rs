use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eventline::warn;
use smithay::backend::drm::{DrmEventMetadata, DrmEventTime};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::wayland::presentation::Refresh;

use crate::backend::frame_interval_for_refresh_hz;
use crate::backend::tty::drm::TtyDrmOutput;
use crate::compositor::root::Halley;

#[derive(Clone, Debug, Default)]
pub(super) struct VBlankMismatchState {
    pub(super) first_seen_at: Option<Instant>,
    pub(super) reported_active: bool,
}

#[derive(Clone, Copy, Debug)]
enum TtyRedrawState {
    Idle,
    Queued,
    WaitingForVBlank { redraw_needed: bool },
    WaitingForEstimatedVBlank { due_at: Instant },
    WaitingForEstimatedVBlankAndQueued { due_at: Instant },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TtyFrameClock {
    refresh_interval: Duration,
    last_presentation_time: Option<Duration>,
    last_presentation_instant: Option<Instant>,
    redraw_state: TtyRedrawState,
}

impl TtyFrameClock {
    pub(super) fn new(refresh_interval: Duration) -> Self {
        Self {
            refresh_interval,
            last_presentation_time: None,
            last_presentation_instant: None,
            redraw_state: TtyRedrawState::Idle,
        }
    }

    fn set_refresh_interval(&mut self, refresh_interval: Duration) {
        self.refresh_interval = refresh_interval;
    }

    pub(super) fn presented(
        &mut self,
        presentation_time: Duration,
        now: Instant,
        output_name: &str,
    ) -> bool {
        // Env-gated (HALLEY_WL_PERF) pacing-jitter detector. Micro-stutter shows
        // up here as an uneven present interval: a gap well beyond one refresh
        // period means at least one vblank was effectively missed (a dropped
        // frame), which the render-budget overrun log does not catch on its own.
        if crate::perf::enabled()
            && let Some(last) = self.last_presentation_instant
            && !self.refresh_interval.is_zero()
        {
            let interval = now.saturating_duration_since(last);
            let jitter_threshold = self.refresh_interval + self.refresh_interval / 2;
            if interval > jitter_threshold {
                let periods = interval.as_secs_f32() / self.refresh_interval.as_secs_f32();
                warn!(
                    "tty frame pacing miss: output={} present_interval={:?} refresh={:?} (~{:.1} refresh periods)",
                    output_name, interval, self.refresh_interval, periods
                );
            }
        }
        if !presentation_time.is_zero() {
            self.last_presentation_time = Some(presentation_time);
            self.last_presentation_instant = Some(now);
        }
        match std::mem::replace(&mut self.redraw_state, TtyRedrawState::Idle) {
            TtyRedrawState::WaitingForVBlank { redraw_needed } => {
                if redraw_needed {
                    self.redraw_state = TtyRedrawState::Queued;
                }
                redraw_needed
            }
            TtyRedrawState::Queued | TtyRedrawState::WaitingForEstimatedVBlankAndQueued { .. } => {
                self.redraw_state = TtyRedrawState::Queued;
                true
            }
            TtyRedrawState::Idle | TtyRedrawState::WaitingForEstimatedVBlank { .. } => false,
        }
    }

    pub(super) fn queue_redraw(&mut self) {
        self.redraw_state = match self.redraw_state {
            TtyRedrawState::Idle => TtyRedrawState::Queued,
            TtyRedrawState::WaitingForVBlank { .. } => TtyRedrawState::WaitingForVBlank {
                redraw_needed: true,
            },
            TtyRedrawState::WaitingForEstimatedVBlank { due_at }
            | TtyRedrawState::WaitingForEstimatedVBlankAndQueued { due_at } => {
                TtyRedrawState::WaitingForEstimatedVBlankAndQueued { due_at }
            }
            TtyRedrawState::Queued => TtyRedrawState::Queued,
        };
    }

    pub(super) fn mark_submitted(&mut self) {
        self.redraw_state = TtyRedrawState::WaitingForVBlank {
            redraw_needed: false,
        };
    }

    fn mark_estimated_wait(&mut self, due_at: Instant) {
        self.redraw_state = match self.redraw_state {
            TtyRedrawState::Queued | TtyRedrawState::WaitingForEstimatedVBlankAndQueued { .. } => {
                TtyRedrawState::WaitingForEstimatedVBlankAndQueued { due_at }
            }
            _ => TtyRedrawState::WaitingForEstimatedVBlank { due_at },
        };
    }

    fn consume_estimated_wait(&mut self) -> bool {
        match std::mem::replace(&mut self.redraw_state, TtyRedrawState::Idle) {
            TtyRedrawState::WaitingForEstimatedVBlankAndQueued { .. } | TtyRedrawState::Queued => {
                self.redraw_state = TtyRedrawState::Queued;
                true
            }
            _ => false,
        }
    }

    fn next_presentation_instant(&self, now: Instant) -> Instant {
        let Some(last) = self.last_presentation_instant else {
            return now + self.refresh_interval;
        };
        let mut next = last + self.refresh_interval;
        while next <= now {
            next += self.refresh_interval;
        }
        next
    }
}

pub(super) fn sync_tty_frame_clocks(
    frame_clocks: &Rc<RefCell<HashMap<String, TtyFrameClock>>>,
    outputs: &[TtyDrmOutput],
) {
    let mut clocks = frame_clocks.borrow_mut();
    clocks.retain(|output_name, _| {
        outputs
            .iter()
            .any(|output| output.connector_name == *output_name)
    });
    for output in outputs {
        let interval = output_frame_interval(output);
        clocks
            .entry(output.connector_name.clone())
            .and_modify(|clock| clock.set_refresh_interval(interval))
            .or_insert_with(|| TtyFrameClock::new(interval));
    }
}

pub(super) fn monotonic_now_duration() -> Duration {
    smithay::utils::Clock::<smithay::utils::Monotonic>::new()
        .now()
        .into()
}

pub(super) fn drm_vblank_timestamp(metadata: Option<&DrmEventMetadata>) -> Duration {
    if let Some(metadata) = metadata
        && let DrmEventTime::Monotonic(timestamp) = metadata.time
    {
        return timestamp;
    }

    monotonic_now_duration()
}

pub(super) fn present_tty_frame_feedback<E: std::fmt::Display>(
    output_name: &str,
    submitted: Result<Option<smithay::desktop::utils::OutputPresentationFeedback>, E>,
    presentation_time: Duration,
    refresh_interval: Option<Duration>,
    sequence: u64,
) {
    let Some(mut feedback) = (match submitted {
        Ok(feedback) => feedback,
        Err(err) => {
            warn!(
                "failed to mark drm frame submitted for {}: {}",
                output_name, err
            );
            return;
        }
    }) else {
        return;
    };

    let refresh = refresh_interval
        .map(Refresh::Fixed)
        .unwrap_or(Refresh::Unknown);
    let mut flags =
        wp_presentation_feedback::Kind::Vsync | wp_presentation_feedback::Kind::HwCompletion;
    if !presentation_time.is_zero() {
        flags.insert(wp_presentation_feedback::Kind::HwClock);
    }
    feedback.presented::<_, smithay::utils::Monotonic>(presentation_time, refresh, sequence, flags);
}

pub(super) fn output_frame_interval(output: &TtyDrmOutput) -> Duration {
    frame_interval_for_refresh_hz(Some(output.mode.vrefresh() as f64))
}

pub(super) fn schedule_estimated_frame_callback(
    estimated_frame_callbacks: &Rc<RefCell<HashMap<String, Instant>>>,
    frame_clocks: &Rc<RefCell<HashMap<String, TtyFrameClock>>>,
    output: &TtyDrmOutput,
    now: Instant,
) {
    let due_at = frame_clocks
        .borrow()
        .get(output.connector_name.as_str())
        .map(|clock| clock.next_presentation_instant(now))
        .unwrap_or_else(|| now + output_frame_interval(output));
    estimated_frame_callbacks
        .borrow_mut()
        .entry(output.connector_name.clone())
        .or_insert(due_at);
    frame_clocks
        .borrow_mut()
        .entry(output.connector_name.clone())
        .or_insert_with(|| TtyFrameClock::new(output_frame_interval(output)))
        .mark_estimated_wait(due_at);
}

pub(super) fn send_due_estimated_frame_callbacks(
    estimated_frame_callbacks: &Rc<RefCell<HashMap<String, Instant>>>,
    frame_clocks: &Rc<RefCell<HashMap<String, TtyFrameClock>>>,
    output_frame_pending: &Rc<RefCell<HashMap<String, bool>>>,
    st: &mut Halley,
    now: Instant,
) {
    let pending = output_frame_pending.borrow();
    let due_outputs: Vec<String> = estimated_frame_callbacks
        .borrow()
        .iter()
        .filter_map(|(output_name, due_at)| {
            (now >= *due_at && !pending.get(output_name.as_str()).copied().unwrap_or(false))
                .then_some(output_name.clone())
        })
        .collect();
    drop(pending);

    if due_outputs.is_empty() {
        return;
    }

    let mut estimated = estimated_frame_callbacks.borrow_mut();
    for output_name in due_outputs {
        estimated.remove(output_name.as_str());
        let redraw_queued = frame_clocks
            .borrow_mut()
            .get_mut(output_name.as_str())
            .is_some_and(TtyFrameClock::consume_estimated_wait);
        if redraw_queued {
            st.runtime.tty_redraw_outputs.insert(output_name);
        } else {
            st.advance_tty_frame_callback_sequence(output_name.as_str());
            crate::frame_loop::send_frame_callbacks_for_output(st, output_name.as_str(), now);
        }
    }
}
