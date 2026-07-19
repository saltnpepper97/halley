use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static LAST_EMITTED: OnceLock<Mutex<HashMap<&'static str, Instant>>> = OnceLock::new();

/// Return true at most once per interval for a diagnostic category.
///
/// This is intentionally process-global: repeated failures from multiple windows or
/// outputs describe the same degraded subsystem and should not multiply log volume.
pub(crate) fn should_emit(key: &'static str, interval: Duration) -> bool {
    let now = Instant::now();
    let Ok(mut emitted) = LAST_EMITTED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    else {
        return true;
    };

    if emitted
        .get(key)
        .is_some_and(|last| now.saturating_duration_since(*last) < interval)
    {
        return false;
    }
    emitted.insert(key, now);
    true
}

/// Preserve a recoverable warning without repeating it on every rendered frame.
pub(crate) fn warn_throttled(
    key: &'static str,
    interval: Duration,
    message: impl FnOnce() -> String,
) {
    if should_emit(key, interval) {
        eventline::warn!("{}", message());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_repeated_category_within_interval() {
        let key = "diagnostics-test-repeated-category";
        assert!(should_emit(key, Duration::from_secs(60)));
        assert!(!should_emit(key, Duration::from_secs(60)));
    }
}
