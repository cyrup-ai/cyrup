//! Dependency-free UTC date parsing for the remote model-catalog overlay (DRIFT-007).
//!
//! Pi reaches for `Date.parse(...)` in two places on this path:
//!
//! - `remote-catalog-provider.ts:107` — `Date.parse(response.headers.get("last-modified") ?? "")`,
//!   an RFC 9110 `IMF-fixdate` (`Sun, 06 Nov 1994 08:49:37 GMT`), with `NaN` folded to `0`.
//! - `providers/all.ts:73` — `Date.parse(modelDataManifest.generatedAt)`, an ISO-8601 UTC instant,
//!   with `NaN` folded to `undefined`.
//!
//! The workspace deliberately avoids adding a dependency where a small pure-Rust routine will do
//! (see the justification comments in the root `Cargo.toml`), and the only two shapes that matter
//! here are fixed-format, so both are parsed by hand. Anything else returns `None`, which the
//! callers map onto Pi's `NaN` branch — never a panic (NO-PANIC policy: no indexing, no unwrap).

/// Days since the Unix epoch for a proleptic-Gregorian civil date (Howard Hinnant's
/// `days_from_civil`, the standard branch-free civil-calendar algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if month > 2 { month - 3 } else { month + 9 }; // March-based month index
    let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Epoch milliseconds for a UTC `Y-M-D H:M:S`, or `None` when any field is out of range.
fn utc_ms(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> Option<i64> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&min)
        // Leap seconds are clamped by the caller's own `60` allowance, matching `Date.parse`.
        || !(0..=60).contains(&sec)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    days.checked_mul(86_400)?
        .checked_add(hour * 3600 + min * 60 + sec)?
        .checked_mul(1000)
}

/// `Jan`…`Dec` → 1…12 (case-sensitive, as HTTP mandates).
fn month_from_abbrev(name: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS
        .iter()
        .position(|m| *m == name)
        .and_then(|i| i64::try_from(i + 1).ok())
}

/// Split `HH:MM:SS` into its three numeric fields.
fn split_clock(clock: &str) -> Option<(i64, i64, i64)> {
    let mut parts = clock.split(':');
    let h = parts.next()?.parse::<i64>().ok()?;
    let m = parts.next()?.parse::<i64>().ok()?;
    let s = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((h, m, s))
}

/// Parse an RFC 9110 `IMF-fixdate` (`Sun, 06 Nov 1994 08:49:37 GMT`) into epoch milliseconds.
///
/// This is the only `Last-Modified` form a conforming origin server may send. The obsolete RFC 850
/// and `asctime()` forms are intentionally NOT accepted: they land on Pi's `NaN` → `0` branch, which
/// only costs one extra revalidation, whereas mis-parsing a two-digit year would silently skew the
/// staleness guard.
pub fn parse_http_date_ms(value: &str) -> Option<i64> {
    let value = value.trim();
    // `day-name "," SP` — the day name is redundant with the date, so it is only skipped.
    let rest = value.split_once(',').map_or(value, |(_, r)| r).trim();
    let mut fields = rest.split_whitespace();
    let day = fields.next()?.parse::<i64>().ok()?;
    let month = month_from_abbrev(fields.next()?)?;
    let year = fields.next()?.parse::<i64>().ok()?;
    let (hour, min, sec) = split_clock(fields.next()?)?;
    // A trailing zone is required to be `GMT`; anything else is not an IMF-fixdate.
    match fields.next() {
        None | Some("GMT") | Some("UTC") => {}
        Some(_) => return None,
    }
    if fields.next().is_some() {
        return None;
    }
    utc_ms(year, month, day, hour, min, sec)
}

/// Parse an ISO-8601 UTC instant (`2026-07-10T16:34:43Z`, optional fractional seconds) into epoch
/// milliseconds. Only the `Z` (or `+00:00`) zone is accepted — the built-in catalog manifest is
/// generated in UTC, and silently reinterpreting a local-time stamp would corrupt the staleness
/// guard in the direction that discards a *valid* overlay.
pub fn parse_iso8601_utc_ms(value: &str) -> Option<i64> {
    let value = value.trim();
    let body = value
        .strip_suffix('Z')
        .or_else(|| value.strip_suffix("+00:00"))
        .or_else(|| value.strip_suffix("+0000"))?;
    let (date, time) = body.split_once('T').or_else(|| body.split_once(' '))?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<i64>().ok()?;
    let day = date_parts.next()?.parse::<i64>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    // Fractional seconds are truncated, not rounded — `Date.parse` keeps millisecond precision but
    // the manifest is second-granular, so a whole-second floor is exact for every real input.
    let (clock, frac) = time
        .split_once('.')
        .map_or((time, None), |(c, f)| (c, Some(f)));
    let (hour, min, sec) = split_clock(clock)?;
    let millis = match frac {
        None => 0,
        Some(f) => {
            let digits: String = f.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() {
                return None;
            }
            let mut padded = digits;
            padded.truncate(3);
            while padded.len() < 3 {
                padded.push('0');
            }
            padded.parse::<i64>().ok()?
        }
    };
    utc_ms(year, month, day, hour, min, sec)?.checked_add(millis)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn imf_fixdate_matches_the_rfc_example() {
        // RFC 9110 §5.6.7's own example; `date -u -d 'Sun, 06 Nov 1994 08:49:37 GMT' +%s` = 784111777.
        assert_eq!(
            parse_http_date_ms("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777_000)
        );
    }

    #[test]
    fn epoch_and_leap_day_round_trip() {
        assert_eq!(parse_http_date_ms("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        // 2024-02-29 is a leap day; `date -u -d '2024-02-29T00:00:00Z' +%s` = 1709164800.
        assert_eq!(
            parse_iso8601_utc_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
    }

    #[test]
    fn iso8601_matches_the_catalog_manifest_stamp() {
        // `date -u -d '2026-07-10T16:34:43Z' +%s` = 1783701283.
        assert_eq!(
            parse_iso8601_utc_ms("2026-07-10T16:34:43Z"),
            Some(1_783_701_283_000)
        );
        assert_eq!(
            parse_iso8601_utc_ms("2026-07-10T16:34:43.250Z"),
            Some(1_783_701_283_250)
        );
        assert_eq!(
            parse_iso8601_utc_ms("2026-07-10T16:34:43+00:00"),
            Some(1_783_701_283_000)
        );
    }

    #[test]
    fn garbage_is_none_never_a_panic() {
        for bad in [
            "",
            "not a date",
            "Sun, 06 Nov 1994 08:49 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT", // RFC 850: deliberately rejected
            "Sun Nov  6 08:49:37 1994",       // asctime: deliberately rejected
            "Sun, 06 Nov 1994 08:49:37 PST",  // non-GMT zone
            "2026-07-10T16:34:43",            // no zone
            "2026-07-10T16:34:43-05:00",      // non-UTC offset
            "2026-13-99T99:99:99Z",           // out of range
            "2026-07-10T16:34:43.Z",          // empty fraction
        ] {
            assert_eq!(parse_http_date_ms(bad).and(parse_iso8601_utc_ms(bad)), None);
        }
    }
}
