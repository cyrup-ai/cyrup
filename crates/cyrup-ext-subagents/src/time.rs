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
//!
//! [`format_iso8601_millis`] is the one ISO-8601 renderer of those stamps (pi's
//! `new Date(ms).toISOString()`), kept beside the clock it renders so every `timestamp` string the
//! crate writes — status views, scheduled runs, missions, the live child transcript — is one
//! spelling.

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

/// Formats an epoch-millisecond timestamp as an ISO-8601 UTC string
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`), matching pi's `new Date(ms).toISOString()` output shape. Pure
/// arithmetic (Howard Hinnant's proleptic-Gregorian `civil_from_days`), so it needs no date-time
/// dependency and cannot panic.
#[must_use]
pub fn format_iso8601_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days`: convert a count of days since the Unix epoch
/// (1970-01-01) into a proleptic-Gregorian `(year, month, day)`. Integer-only, total.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_formats_a_known_epoch() {
        assert_eq!(format_iso8601_millis(0), "1970-01-01T00:00:00.000Z");
        // 2021-01-01T00:00:00.000Z == 1_609_459_200_000 ms.
        assert_eq!(
            format_iso8601_millis(1_609_459_200_000),
            "2021-01-01T00:00:00.000Z"
        );
        // Same day + 12:34:56.789 (== 45_296_789 ms of day) exercises the time + millis fields.
        assert_eq!(
            format_iso8601_millis(1_609_459_200_000 + 45_296_789),
            "2021-01-01T12:34:56.789Z"
        );
    }

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
