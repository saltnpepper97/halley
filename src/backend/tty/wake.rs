//! TEMPORARY WORKAROUND for an amdgpu bug, not a fix:
//! <https://gitlab.freedesktop.org/drm/amd/-/issues/3504>.
//!
//! On RDNA3 cards, waking two displays from DPMS can hang the second
//! display's wake modeset inside the kernel for about 25 seconds, freezing
//! the compositor. It happens when that modeset follows the first display's
//! wake modeset with no ordinary commit on the first display in between.
//! One completed ordinary commit there (a plain page flip is enough) avoids
//! it, so a DPMS wake brings outputs up one at a time: each later output's
//! wake modeset waits until every earlier woken output has made its wake
//! modeset and one ordinary page flip that reached VBlank.
//!
//! The real fix belongs in amdgpu. Remove this module, and the calls that
//! drive it, once that bug is fixed upstream.

use std::time::{Duration, Instant};

/// The longest an output's wake modeset waits for earlier outputs to
/// settle. A wake that never settles (a failed render, a lost VBlank)
/// must not keep the other displays dark.
const HOLD_LIMIT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    /// Not waking.
    #[default]
    Settled,
    /// Powered on; its wake modeset has not been submitted yet.
    ModesetOwed,
    /// Its wake modeset was submitted; one ordinary page flip is owed.
    FlipOwed,
    /// That page flip was submitted and awaits its VBlank.
    FlipInFlight,
}

/// One output's progress through a DPMS wake.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WakeProgress {
    stage: Stage,
    since: Option<Instant>,
}

impl WakeProgress {
    pub fn woken(&mut self, now: Instant) {
        *self = Self {
            stage: Stage::ModesetOwed,
            since: Some(now),
        };
    }

    /// Powered off, paused, or reconfigured: no wake is in progress.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// A frame was queued on this output.
    pub fn submitted(&mut self) {
        self.stage = match self.stage {
            Stage::ModesetOwed => Stage::FlipOwed,
            Stage::FlipOwed => Stage::FlipInFlight,
            stage => stage,
        };
    }

    /// The kernel reported this output's VBlank.
    pub fn vblank(&mut self) {
        if self.stage == Stage::FlipInFlight {
            self.reset();
        }
    }

    /// Whether this output's next frame is the ordinary page flip that
    /// lets the next output wake. It must not be skipped as empty, so the
    /// caller repaints it in full.
    pub fn owes_flip(&self) -> bool {
        self.stage == Stage::FlipOwed
    }

    fn unsettled(&self, now: Instant) -> bool {
        self.stage != Stage::Settled
            && self
                .since
                .is_some_and(|since| now.saturating_duration_since(since) < HOLD_LIMIT)
    }
}

/// The index of the earlier output that the output at `index` must wait
/// for before its wake modeset, if any. Outputs commit in slice order, so
/// a later output also waits for an earlier one whose own wake modeset has
/// not been made yet.
pub(super) fn held_by(progress: &[WakeProgress], index: usize, now: Instant) -> Option<usize> {
    if progress[index].stage != Stage::ModesetOwed {
        return None;
    }
    progress.iter().enumerate().position(|(other, wake)| {
        other != index && wake.unsettled(now) && (wake.stage != Stage::ModesetOwed || other < index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn woken(now: Instant) -> WakeProgress {
        let mut wake = WakeProgress::default();
        wake.woken(now);
        wake
    }

    #[test]
    fn a_later_output_waits_until_the_earlier_ones_flip_reaches_vblank() {
        let now = Instant::now();
        let mut outputs = [woken(now), woken(now)];
        assert_eq!(
            held_by(&outputs, 0, now),
            None,
            "the first output goes first"
        );
        assert_eq!(held_by(&outputs, 1, now), Some(0));

        outputs[0].submitted(); // wake modeset
        assert!(outputs[0].owes_flip());
        assert_eq!(held_by(&outputs, 1, now), Some(0));
        outputs[0].vblank(); // the modeset's own VBlank settles nothing
        assert_eq!(held_by(&outputs, 1, now), Some(0));

        outputs[0].submitted(); // ordinary flip
        assert!(!outputs[0].owes_flip());
        assert_eq!(held_by(&outputs, 1, now), Some(0));
        outputs[0].vblank();
        assert_eq!(held_by(&outputs, 1, now), None);
    }

    #[test]
    fn an_output_woken_alone_or_already_lit_is_never_held() {
        let now = Instant::now();
        let outputs = [WakeProgress::default(), woken(now)];
        assert_eq!(held_by(&outputs, 1, now), None);
        let mut lit = woken(now);
        lit.submitted();
        assert_eq!(held_by(&[lit, woken(now)], 0, now), None);
    }

    #[test]
    fn a_wake_that_never_settles_stops_holding_after_the_limit() {
        let start = Instant::now();
        let outputs = [woken(start), woken(start)];
        assert_eq!(held_by(&outputs, 1, start), Some(0));
        assert_eq!(held_by(&outputs, 1, start + HOLD_LIMIT), None);
    }

    #[test]
    fn powering_off_ends_a_wake() {
        let now = Instant::now();
        let mut outputs = [woken(now), woken(now)];
        outputs[0].reset();
        assert_eq!(held_by(&outputs, 1, now), None);
        assert!(!outputs[0].owes_flip());
    }
}
