//! The crate's SINGLE epoch-millisecond clock.
//!
//! Every `Date.now()` in the upstream port lands here. One helper, one integer width (`i64`, the
//! width every status/telemetry record on disk already uses), one clamp-never-panic policy: a
//! pre-epoch host clock reads as `0` and a time beyond `i64::MAX` milliseconds saturates, so
//! neither can panic a run. Callers that need a different width cast at the call site rather than
//! reimplementing the conversion.
//!
//! Timestamps minted here are compared against each other across process boundaries (a child's
//! steering ack against the parent's steer request, a step's `ended_at` against the run's
//! `started_at`), which is why they must come from one implementation: two independently written
//! clocks make an "the ack predates the request" diagnostic meaningless.

use std::time::{SystemTime, UNIX_EPOCH};

/// `Date.now()` — the current wall-clock time in whole milliseconds since the Unix epoch.
#[must_use]
pub fn now_epoch_millis() -> i64 {
    epoch_millis(SystemTime::now())
}

/// [`now_epoch_millis`]'s explicit-[`SystemTime`] form, for the callers that convert a timestamp
/// they were handed (a file's `modified()`, an injected test clock) rather than reading the
/// process clock.
#[must_use]
pub fn epoch_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        // A system clock set before the Unix epoch is not something this crate can do anything
        // sane about; 0 is a safe, non-panicking floor rather than propagating an error type
        // through every timestamp-stamping call site for a condition that indicates a broken host
        // clock, not a bug in this crate's own logic.
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_epoch_millis_is_positive_and_monotonic_enough_for_ordering() {
        let a = now_epoch_millis();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = now_epoch_millis();
        assert!(a > 0);
        assert!(b >= a);
    }

    #[test]
    fn epoch_millis_agrees_with_now_for_the_current_instant() {
        let before = now_epoch_millis();
        let stamped = epoch_millis(SystemTime::now());
        let after = now_epoch_millis();
        assert!(stamped >= before && stamped <= after);
    }
}
