use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eventline::debug;

use super::drm::TtyFrameQueueReport;

const FRAME_STATS_LOG_INTERVAL_SECS: u64 = 10;

#[derive(Debug)]
pub(super) struct TtyFrameStats {
    last_report_at: Instant,
    pub(super) queued_frames: u64,
    pub(super) completed_vblanks: u64,
    pub(super) page_flip_timeouts: u64,
    pub(super) page_flip_recoveries: u64,
    pub(super) vblank_mismatches: u64,
    direct_scanout_frames: u64,
    composed_frames: u64,
    sync_wait_count: u64,
    sync_wait_total_ns: u128,
    max_sync_wait: Duration,
}

impl TtyFrameStats {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_report_at: now,
            queued_frames: 0,
            completed_vblanks: 0,
            page_flip_timeouts: 0,
            page_flip_recoveries: 0,
            vblank_mismatches: 0,
            direct_scanout_frames: 0,
            composed_frames: 0,
            sync_wait_count: 0,
            sync_wait_total_ns: 0,
            max_sync_wait: Duration::ZERO,
        }
    }
}

pub(super) fn record_tty_frame_queue(
    frame_stats: Option<&Rc<RefCell<TtyFrameStats>>>,
    report: &TtyFrameQueueReport,
) {
    let Some(frame_stats) = frame_stats else {
        return;
    };
    if !report.queued {
        return;
    }

    let mut stats = frame_stats.borrow_mut();
    stats.queued_frames += 1;
    if report.direct_scanout_active {
        stats.direct_scanout_frames += 1;
    }
    if report.composed {
        stats.composed_frames += 1;
    }
    if let Some(wait) = report.sync_wait {
        stats.sync_wait_count += 1;
        stats.sync_wait_total_ns += wait.as_nanos();
        stats.max_sync_wait = stats.max_sync_wait.max(wait);
    }
}

pub(super) fn maybe_log_tty_frame_stats(
    frame_stats: Option<&Rc<RefCell<TtyFrameStats>>>,
    output_frame_pending_since: &Rc<RefCell<HashMap<String, Instant>>>,
    now: Instant,
) {
    let Some(frame_stats) = frame_stats else {
        return;
    };

    let mut stats = frame_stats.borrow_mut();
    if now.saturating_duration_since(stats.last_report_at)
        < Duration::from_secs(FRAME_STATS_LOG_INTERVAL_SECS)
    {
        return;
    }

    let pending_since = output_frame_pending_since.borrow();
    let pending_frames = pending_since.len();
    let max_pending_age = pending_since
        .values()
        .map(|queued_at| now.saturating_duration_since(*queued_at))
        .max()
        .unwrap_or(Duration::ZERO);
    let avg_sync_wait = if stats.sync_wait_count == 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos((stats.sync_wait_total_ns / stats.sync_wait_count as u128) as u64)
    };

    debug!(
        "tty frame stats: queued={} completed_vblanks={} page_flip_timeouts={} page_flip_recoveries={} vblank_mismatches={} direct_scanout={} composed={} sync_waits={} avg_sync_wait={:?} max_sync_wait={:?} pending_frames={} max_pending_age={:?}",
        stats.queued_frames,
        stats.completed_vblanks,
        stats.page_flip_timeouts,
        stats.page_flip_recoveries,
        stats.vblank_mismatches,
        stats.direct_scanout_frames,
        stats.composed_frames,
        stats.sync_wait_count,
        avg_sync_wait,
        stats.max_sync_wait,
        pending_frames,
        max_pending_age
    );
    stats.last_report_at = now;
}
