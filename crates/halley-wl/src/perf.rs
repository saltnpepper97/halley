//! Lightweight, env-gated performance instrumentation.
//!
//! Enabled by setting `HALLEY_WL_PERF` to a truthy value (`1`, `true`, `yes`,
//! `on`). When enabled, timing helpers emit `info!`/`warn!` lines through the
//! normal `eventline` logger, so they land in the halley log file. When
//! disabled (the default) every helper is a cheap no-op guarded by a single
//! cached bool, so it is safe to leave the call sites in place.

use std::sync::OnceLock;
use std::time::Instant;

static PERF_ENABLED: OnceLock<bool> = OnceLock::new();

/// Whether perf instrumentation is enabled. Reads `HALLEY_WL_PERF` once and
/// caches the result for the lifetime of the process.
pub(crate) fn enabled() -> bool {
    *PERF_ENABLED.get_or_init(|| {
        std::env::var("HALLEY_WL_PERF")
            .ok()
            .map(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

/// Returns `Some(Instant::now())` only when perf is enabled, so callers can
/// avoid taking timestamps on the hot path when instrumentation is off.
#[inline]
pub(crate) fn start() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// Elapsed milliseconds since `start`, as an `f32` for compact logging.
#[inline]
pub(crate) fn elapsed_ms(start: Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}
