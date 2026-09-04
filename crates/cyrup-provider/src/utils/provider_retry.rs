//! Server-directed provider request retry (1:1 with Pi `packages/ai/src/utils/provider-retry.ts`).
//!
//! Pi wraps every provider request in `retryProviderRequest` so the SDK's own retry timers (which
//! ignore the abort signal) can be replaced with an interruptible one. The policy it reproduces —
//! and this module ports — is:
//!
//! - retry on `x-should-retry: true`, on a transport failure (no status), and on `408`/`409`/`429`
//!   and any `5xx`; never on `x-should-retry: false` (`provider-retry.ts:22-34`);
//! - honor a server-requested delay from `retry-after-ms` then `retry-after` (seconds *or* an
//!   HTTP-date), otherwise back off `min(0.5 * 2^retryIndex, 8)` seconds with jitter
//!   (`provider-retry.ts:50-66`);
//! - fail immediately when the server asks for longer than `maxRetryDelayMs` (default
//!   [`DEFAULT_MAX_RETRY_DELAY_MS`]; `0` disables the cap) so a higher retry layer can surface it
//!   with user visibility (`provider-retry.ts:36-48`).
//!
//! **Shape delta, not a semantic one.** Pi can express this as a combinator because the vendor SDKs
//! *throw* a typed error carrying `status` + `headers`. Cyrup speaks the wire directly, so a non-2xx
//! is an ordinary [`reqwest::Response`] value whose headers must be read *before* its body is
//! consumed. The loop therefore lives in [`crate::stream::sse::open_sse`], the single funnel every
//! wire API's streaming request passes through, and this module supplies the three decisions it
//! makes. Retry covers the connect + response-head phase only — exactly the span Pi's
//! `client.chat.completions.create(...).withResponse()` promise covers; a stream that fails after
//! the head is not restarted by either implementation.
//!
//! This is a *different* mechanism from [`crate::utils::retry`] (Pi `utils/retry.ts`), which retries
//! a whole assistant turn on a transient-looking error message. Both exist upstream, at different
//! layers.

use crate::error::ProviderError;
use crate::utils::http_date::parse_http_date_ms;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default ceiling on a server-requested retry delay (Pi `DEFAULT_MAX_RETRY_DELAY_MS`,
/// provider-retry.ts:1).
pub const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// The per-request retry budget (Pi `ProviderRetryOptions`, provider-retry.ts:3-7, sourced from
/// `StreamOptions.maxRetries` / `.maxRetryDelayMs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderRetry {
    /// Retry attempts after the first (Pi `options.maxRetries ?? 0`, provider-retry.ts:108).
    pub max_retries: u32,
    /// Ceiling on a *server-requested* delay. `None` ⇒ [`DEFAULT_MAX_RETRY_DELAY_MS`]; `Some(0)`
    /// disables the ceiling (Pi `maxRetryDelayMs ?? DEFAULT_MAX_RETRY_DELAY_MS`, `maxDelayMs > 0`,
    /// provider-retry.ts:40-41).
    pub max_retry_delay_ms: Option<u64>,
}

impl ProviderRetry {
    /// No retries — Pi's default when neither `StreamOptions.maxRetries` nor
    /// `settings.retry.provider.maxRetries` is set (`options.maxRetries ?? 0`).
    pub const NONE: Self = Self {
        max_retries: 0,
        max_retry_delay_ms: None,
    };

    /// Read the budget off a request's [`crate::stream::StreamOptions`].
    pub fn from_options(options: &crate::stream::StreamOptions) -> Self {
        Self {
            max_retries: options.max_retries.unwrap_or(0),
            max_retry_delay_ms: options.max_retry_delay_ms,
        }
    }

    /// The effective server-delay ceiling in ms (`0` = no ceiling).
    fn delay_ceiling_ms(&self) -> u64 {
        self.max_retry_delay_ms
            .unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS)
    }
}

/// Whether a failed attempt should be retried (Pi `isRetryableProviderError`,
/// provider-retry.ts:22-34).
///
/// `status: None` models Pi's `error.status === undefined` — an SDK connection error, i.e. cyrup's
/// transport failure or header-phase timeout — and is retryable. An explicit `x-should-retry`
/// header wins over the status in both directions.
pub fn is_retryable_provider_error(
    status: Option<u16>,
    headers: Option<&reqwest::header::HeaderMap>,
) -> bool {
    let should_retry = headers
        .and_then(|h| h.get("x-should-retry"))
        .and_then(|v| v.to_str().ok());
    match should_retry {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }

    match status {
        None => true,
        Some(s) => s == 408 || s == 409 || s == 429 || s >= 500,
    }
}

/// Pi's pre-jitter exponential backoff: `Math.min(0.5 * 2 ** retryIndex, 8) * 1000` ms
/// (provider-retry.ts:64). Saturates at the 8 s plateau rather than overflowing.
pub fn exponential_backoff_ms(retry_index: u32) -> u64 {
    // `0.5 * 2^i` reaches the 8 s cap at i == 4; clamping the exponent keeps the shift finite.
    let capped = retry_index.min(4);
    (500u64 << capped).min(8_000)
}

/// How long to wait before retry number `retry_index` (0-based), given the failing response's
/// headers (Pi `getRetryDelayMs`, provider-retry.ts:50-66).
///
/// Header precedence is Pi's: `retry-after-ms` (milliseconds), then `retry-after` (seconds, or an
/// HTTP-date), then the jittered exponential backoff. A *server-requested* delay above the ceiling
/// returns [`ProviderError::RetryDelay`] instead of sleeping, so the failure is surfaced rather than
/// silently absorbed (Pi `validateServerRetryDelayMs` throws).
///
/// `error_message` is Pi's `error.message`, appended to the ceiling-exceeded text verbatim.
pub fn retry_delay_ms(
    headers: Option<&reqwest::header::HeaderMap>,
    error_message: &str,
    retry_index: u32,
    retry: ProviderRetry,
) -> Result<u64, ProviderError> {
    let header = |name: &str| {
        headers
            .and_then(|h| h.get(name))
            .and_then(|v| v.to_str().ok())
    };

    if let Some(raw) = header("retry-after-ms")
        && !raw.is_empty()
        && let Some(value) = js_parse_float(raw)
    {
        return validate_server_retry_delay_ms(value, retry, error_message);
    }

    if let Some(raw) = header("retry-after")
        && !raw.is_empty()
    {
        // `Number.parseFloat(retryAfter)`; NaN falls through to the HTTP-date form
        // (`Date.parse(retryAfter) - Date.now()`).
        let delay_ms = match js_parse_float(raw) {
            Some(seconds) => seconds * 1000.0,
            // Pi's unparseable-date branch yields NaN, which `validateServerRetryDelayMs` passes
            // through (`NaN > max` is false) and `abortableSleep`'s `Math.max(0, NaN)` then floors
            // to an immediate retry. `0.0` reproduces that without importing NaN into the ladder.
            None => parse_http_date_ms(raw)
                .map(|at| (at - now_ms()) as f64)
                .unwrap_or(0.0),
        };
        return validate_server_retry_delay_ms(delay_ms, retry, error_message);
    }

    // No server guidance: jittered exponential backoff. Pi applies NO ceiling here — the ceiling
    // guards server-requested delays only (`getRetryDelayMs` returns the exponential unvalidated).
    let base = exponential_backoff_ms(retry_index);
    Ok(apply_jitter(base, next_random_unit()))
}

/// Pi `validateServerRetryDelayMs` (provider-retry.ts:36-48): a server-requested delay above the
/// ceiling fails the request instead of sleeping. A ceiling of `0` disables the check.
fn validate_server_retry_delay_ms(
    delay_ms: f64,
    retry: ProviderRetry,
    error_message: &str,
) -> Result<u64, ProviderError> {
    let max_delay_ms = retry.delay_ceiling_ms();
    if max_delay_ms > 0 && delay_ms > max_delay_ms as f64 {
        return Err(ProviderError::RetryDelay(format!(
            "Server requested {}s retry delay (max: {}s). {error_message}",
            ceil_seconds(delay_ms),
            ceil_seconds(max_delay_ms as f64),
        )));
    }
    // Pi sleeps `Math.max(0, ms)`; a past `Retry-After` date therefore retries immediately.
    Ok(if delay_ms.is_finite() && delay_ms > 0.0 {
        delay_ms as u64
    } else {
        0
    })
}

/// `Math.ceil(ms / 1000)` for the ceiling-exceeded message.
fn ceil_seconds(ms: f64) -> i64 {
    if !ms.is_finite() {
        return i64::MAX;
    }
    (ms / 1000.0).ceil() as i64
}

/// Pi's `exponentialDelay * (1 - Math.random() * 0.25)` (provider-retry.ts:65).
fn apply_jitter(base_ms: u64, rand01: f64) -> u64 {
    let factor = 1.0 - rand01.clamp(0.0, 1.0) * 0.25;
    (base_ms as f64 * factor) as u64
}

/// `[CYRUP-DELTA]` Pi calls `Math.random()`; the workspace has no RNG dependency (and
/// `cyrup-provider` deliberately keeps its dependency set minimal — see `faux.rs`, which makes the
/// same substitution for the same reason). This is a xorshift64* seeded once from the wall clock:
/// uniform in `[0, 1)`, good enough for backoff jitter, and it keeps the crate dependency-free.
fn next_random_unit() -> f64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        // Any non-zero seed works; the clock only decorrelates concurrent processes.
        x = (now_ms() as u64) ^ 0x9E37_79B9_7F4A_7C15;
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    STATE.store(x, Ordering::Relaxed);
    let bits = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    // Top 53 bits → the same [0, 1) resolution as a JS double.
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// `Date.now()`.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `Number.parseFloat` semantics: skip leading whitespace, then consume the longest numeric prefix
/// and ignore whatever follows (`parseFloat("1.5s") === 1.5`). `None` is JS's `NaN`.
///
/// Rust's `f64::from_str` rejects trailing garbage, so the longest candidate run of numeric
/// characters is tried and shortened until one parses. Header values are a handful of bytes, so the
/// quadratic worst case is irrelevant, and every slice goes through `str::get` — no indexing, no
/// panic on a char boundary.
fn js_parse_float(value: &str) -> Option<f64> {
    let text = value.trim_start();
    let mut end = text
        .char_indices()
        .take_while(|(_, c)| matches!(c, '0'..='9' | '+' | '-' | '.' | 'e' | 'E'))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    while end > 0 {
        if let Some(prefix) = text.get(..end)
            && let Ok(parsed) = prefix.parse::<f64>()
        {
            return Some(parsed);
        }
        end -= 1;
    }
    None
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
    use reqwest::header::{HeaderMap, HeaderValue};

    fn headers<const N: usize>(pairs: [(&'static str, &str); N]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(k, HeaderValue::from_str(v).unwrap());
        }
        map
    }

    // ------------------------------------------------------- classification -------------------

    #[test]
    fn a_transport_failure_has_no_status_and_is_retryable() {
        assert!(is_retryable_provider_error(None, None));
    }

    #[test]
    fn only_pis_status_set_is_retryable() {
        for status in [408, 409, 429, 500, 502, 503, 504, 599] {
            assert!(
                is_retryable_provider_error(Some(status), None),
                "{status} must be retryable"
            );
        }
        for status in [400, 401, 403, 404, 422, 451] {
            assert!(
                !is_retryable_provider_error(Some(status), None),
                "{status} must NOT be retryable"
            );
        }
    }

    #[test]
    fn x_should_retry_wins_over_the_status_in_both_directions() {
        let force_on = headers([("x-should-retry", "true")]);
        assert!(
            is_retryable_provider_error(Some(400), Some(&force_on)),
            "an explicit `true` retries a status that never would"
        );
        let force_off = headers([("x-should-retry", "false")]);
        assert!(
            !is_retryable_provider_error(Some(503), Some(&force_off)),
            "an explicit `false` blocks a status that always would"
        );
        // Any other value falls through to the status check.
        let noise = headers([("x-should-retry", "maybe")]);
        assert!(is_retryable_provider_error(Some(503), Some(&noise)));
        assert!(!is_retryable_provider_error(Some(400), Some(&noise)));
    }

    // ------------------------------------------------------- delay ----------------------------

    #[test]
    fn exponential_backoff_matches_pis_ladder() {
        // `Math.min(0.5 * 2 ** i, 8) * 1000`
        assert_eq!(exponential_backoff_ms(0), 500);
        assert_eq!(exponential_backoff_ms(1), 1_000);
        assert_eq!(exponential_backoff_ms(2), 2_000);
        assert_eq!(exponential_backoff_ms(3), 4_000);
        assert_eq!(exponential_backoff_ms(4), 8_000);
        assert_eq!(exponential_backoff_ms(5), 8_000, "plateaus at 8s");
        assert_eq!(exponential_backoff_ms(u32::MAX), 8_000, "never overflows");
    }

    #[test]
    fn jitter_only_ever_shortens_the_backoff_by_at_most_a_quarter() {
        assert_eq!(apply_jitter(8_000, 0.0), 8_000);
        assert_eq!(apply_jitter(8_000, 1.0), 6_000);
        for _ in 0..256 {
            let d = retry_delay_ms(None, "boom", 3, ProviderRetry::NONE).unwrap();
            assert!(
                (3_000..=4_000).contains(&d),
                "jittered 4s backoff was {d}ms"
            );
        }
    }

    #[test]
    fn retry_after_ms_header_wins_and_is_taken_as_milliseconds() {
        let h = headers([("retry-after-ms", "1500"), ("retry-after", "30")]);
        assert_eq!(
            retry_delay_ms(Some(&h), "boom", 0, ProviderRetry::NONE).unwrap(),
            1_500
        );
    }

    #[test]
    fn retry_after_seconds_are_scaled_to_milliseconds() {
        let h = headers([("retry-after", "2")]);
        assert_eq!(
            retry_delay_ms(Some(&h), "boom", 0, ProviderRetry::NONE).unwrap(),
            2_000
        );
    }

    #[test]
    fn a_retry_after_http_date_becomes_a_delay_from_now() {
        // Two seconds in the future, formatted as the RFC 9110 IMF-fixdate `Retry-After` form.
        let at = now_ms() + 2_000;
        let stamp = imf_fixdate(at);
        let h = headers([("retry-after", &stamp)]);
        let d = retry_delay_ms(Some(&h), "boom", 0, ProviderRetry::NONE).unwrap();
        assert!(
            (500..=2_000).contains(&d),
            "an HTTP-date 2s out should yield ~2000ms, got {d}"
        );

        // A date already in the past clamps to an immediate retry (Pi `Math.max(0, ms)`).
        let past = imf_fixdate(now_ms() - 60_000);
        let h = headers([("retry-after", &past)]);
        assert_eq!(
            retry_delay_ms(Some(&h), "boom", 0, ProviderRetry::NONE).unwrap(),
            0
        );
    }

    #[test]
    fn a_server_delay_over_the_ceiling_fails_with_pis_exact_sentence() {
        let h = headers([("retry-after", "120")]);
        let err = retry_delay_ms(Some(&h), "http 429: slow down", 0, ProviderRetry::NONE)
            .expect_err("120s exceeds the 60s default ceiling");
        assert_eq!(
            err.to_string(),
            "Server requested 120s retry delay (max: 60s). http 429: slow down"
        );
        assert_eq!(err.code(), "http");
    }

    #[test]
    fn a_zero_ceiling_disables_the_check() {
        let h = headers([("retry-after", "3600")]);
        let retry = ProviderRetry {
            max_retries: 1,
            max_retry_delay_ms: Some(0),
        };
        assert_eq!(
            retry_delay_ms(Some(&h), "boom", 0, retry).unwrap(),
            3_600_000
        );
    }

    #[test]
    fn the_ceiling_does_not_apply_to_the_exponential_backoff() {
        // Pi validates only server-requested delays; a 1ms ceiling must not reject the local ladder.
        let retry = ProviderRetry {
            max_retries: 5,
            max_retry_delay_ms: Some(1),
        };
        let d = retry_delay_ms(None, "boom", 4, retry).unwrap();
        assert!((6_000..=8_000).contains(&d));
    }

    #[test]
    fn an_unparseable_retry_after_retries_immediately() {
        let h = headers([("retry-after", "not-a-date")]);
        assert_eq!(
            retry_delay_ms(Some(&h), "boom", 0, ProviderRetry::NONE).unwrap(),
            0
        );
    }

    // ------------------------------------------------------- parseFloat -----------------------

    #[test]
    fn js_parse_float_takes_the_numeric_prefix() {
        assert_eq!(js_parse_float("100"), Some(100.0));
        assert_eq!(js_parse_float("  2.5  "), Some(2.5));
        assert_eq!(js_parse_float("1.5s"), Some(1.5));
        assert_eq!(js_parse_float("-3"), Some(-3.0));
        assert_eq!(js_parse_float("1e3"), Some(1000.0));
        assert_eq!(js_parse_float("1e"), Some(1.0), "trailing `e` is dropped");
        assert_eq!(js_parse_float(".5"), Some(0.5));
        assert_eq!(js_parse_float(""), None);
        assert_eq!(js_parse_float("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(js_parse_float("abc"), None);
    }

    #[test]
    fn from_options_reads_the_stream_option_budget() {
        let opts = crate::stream::StreamOptions {
            max_retries: Some(3),
            max_retry_delay_ms: Some(1234),
            ..Default::default()
        };
        assert_eq!(
            ProviderRetry::from_options(&opts),
            ProviderRetry {
                max_retries: 3,
                max_retry_delay_ms: Some(1234)
            }
        );
        assert_eq!(
            ProviderRetry::from_options(&crate::stream::StreamOptions::default()),
            ProviderRetry::NONE
        );
    }

    /// Format epoch-ms as an RFC 9110 IMF-fixdate, the form `parse_http_date_ms` reads back.
    fn imf_fixdate(epoch_ms: i64) -> String {
        let secs = epoch_ms.div_euclid(1000);
        let days = secs.div_euclid(86_400);
        let tod = secs.rem_euclid(86_400);
        let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
        let (y, mo, d) = civil_from_days(days);
        let dow = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
        let mon = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ][(mo - 1) as usize];
        format!("{dow}, {d:02} {mon} {y} {h:02}:{mi:02}:{s:02} GMT")
    }

    /// Inverse of `days_from_civil` (Howard Hinnant's `civil_from_days`).
    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
}
