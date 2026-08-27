//! Retry decisions (pi :130-183).

use super::{BASE_DELAY_MS, DEFAULT_MAX_RETRY_DELAY_MS};
use crate::utils::http_date::parse_http_date_ms;
use crate::utils::provider_plumbing::now_millis;
use serde_json::Value;

/// `haystack` contains `needle`, case-insensitively (pi's `/…/i` literal alternatives).
fn contains_ci(haystack_lower: &str, needle_lower: &str) -> bool {
    haystack_lower.contains(needle_lower)
}

/// `left` followed by `right` with at most one character between them — pi's `.?` in
/// `/rate.?limit/i` and friends. Case-insensitive (the caller lower-cases once).
fn contains_with_optional_gap(haystack_lower: &str, left: &str, right: &str) -> bool {
    let mut from = 0usize;
    while let Some(found) = haystack_lower.get(from..).and_then(|s| s.find(left)) {
        let at = from + found;
        let after = at + left.len();
        for gap in [0usize, 1usize] {
            // `.` never matches a newline in JS without the `s` flag.
            if gap == 1 {
                match haystack_lower.get(after..after + 1) {
                    Some("\n") | None => continue,
                    Some(_) => {}
                }
            }
            if haystack_lower
                .get(after + gap..)
                .is_some_and(|rest| rest.starts_with(right))
            {
                return true;
            }
        }
        from = at + 1;
        if from >= haystack_lower.len() {
            break;
        }
    }
    false
}

/// 1:1 port of pi `isTerminalRateLimitError` (`openai-codex-responses.ts:130-134`): a 429 that says
/// the account is out of money/quota is NOT retryable, however many attempts remain.
pub(super) fn is_terminal_rate_limit_error(error_text: &str) -> bool {
    let lower = error_text.to_lowercase();
    [
        "gousagelimiterror",
        "freeusagelimiterror",
        "monthly usage limit reached",
        "available balance",
        "insufficient_quota",
        "out of budget",
        "quota exceeded",
        "billing",
    ]
    .iter()
    .any(|needle| contains_ci(&lower, needle))
}

/// 1:1 port of pi `isRetryableError` (`openai-codex-responses.ts:136-144`).
pub(super) fn is_retryable_error(status: u16, error_text: &str) -> bool {
    if status == 429 && is_terminal_rate_limit_error(error_text) {
        return false;
    }
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    let lower = error_text.to_lowercase();
    contains_with_optional_gap(&lower, "rate", "limit")
        || contains_ci(&lower, "overloaded")
        || contains_with_optional_gap(&lower, "service", "unavailable")
        || contains_with_optional_gap(&lower, "upstream", "connect")
        || contains_with_optional_gap(&lower, "connection", "refused")
}

/// 1:1 port of pi `getRetryAfterDelayMs` (`openai-codex-responses.ts:146-171`): `retry-after-ms`
/// first, then `retry-after` as seconds, then `retry-after` as an HTTP-date. Every result is
/// clamped at zero (`Math.max(0, …)`), and an unparseable value yields `None`.
pub(super) fn get_retry_after_delay_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

    if let Some(raw) = get("retry-after-ms") {
        // `Number(raw)`: whitespace-only is 0, garbage is NaN (which `Number.isFinite` rejects).
        if let Some(millis) = js_number(raw) {
            return Some(clamp_non_negative(millis));
        }
    }

    // `if (!retryAfter) return undefined` — an empty header value is falsy.
    let raw = get("retry-after").filter(|s| !s.is_empty())?;
    if let Some(seconds) = js_number(raw) {
        return Some(clamp_non_negative(seconds * 1000.0));
    }
    let at = parse_http_date_ms(raw)?;
    Some(clamp_non_negative((at - now_millis()) as f64))
}

/// 1:1 port of pi `validateRetryDelayMs` (`openai-codex-responses.ts:175-183`). `Err` is the
/// `RetryDelayExceededError` message, which the caller must NOT retry.
pub(super) fn validate_retry_delay_ms(
    delay_ms: u64,
    max_retry_delay_ms: Option<u64>,
) -> Result<u64, String> {
    let max = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max > 0 && delay_ms > max {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s)",
            ceil_seconds(delay_ms),
            ceil_seconds(max),
        ));
    }
    Ok(delay_ms)
}

/// pi `BASE_DELAY_MS * 2 ** attempt` (`:433`, `:460`) — no jitter, no ceiling.
pub(super) fn backoff_delay_ms(attempt: u32) -> u64 {
    BASE_DELAY_MS.saturating_mul(1u64.checked_shl(attempt.min(32)).unwrap_or(u64::MAX))
}

/// `Math.ceil(ms / 1000)`.
fn ceil_seconds(ms: u64) -> u64 {
    ms.div_ceil(1000)
}

/// `Math.max(0, value)` on a possibly-NaN JS number.
fn clamp_non_negative(value: f64) -> u64 {
    if value.is_finite() && value > 0.0 {
        value as u64
    } else {
        0
    }
}

/// JS `Number(raw)` restricted to the finite case: leading/trailing whitespace is ignored, the
/// empty/whitespace-only string is `0`, and anything else unparseable is `NaN` ⇒ `None`.
fn js_number(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// 1:1 port of pi `parseErrorResponse` (`openai-codex-responses.ts:1533-1558`) collapsed to the
/// single string its caller throws (`info.friendlyMessage || info.message`, `:446`).
///
/// `now_ms` is pi's `Date.now()`, taken as a parameter so the `resets_at` arithmetic is testable.
pub(super) fn parse_error_response(status: u16, raw: &str, now_ms: i64) -> String {
    // `let message = raw || response.statusText || "Request failed";` — cyrup's transport does not
    // retain `statusText`, so a blank body goes straight to the literal fallback.
    let mut message = if raw.is_empty() {
        "Request failed".to_string()
    } else {
        raw.to_string()
    };
    let mut friendly: Option<String> = None;

    if let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && let Some(err) = parsed.get("error").filter(|e| e.is_object())
    {
        let code = err
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| err.get("type").and_then(Value::as_str))
            .unwrap_or("");
        let code_lower = code.to_lowercase();
        let limit_code = code_lower.contains("usage_limit_reached")
            || code_lower.contains("usage_not_included")
            || code_lower.contains("rate_limit_exceeded");
        if limit_code || status == 429 {
            let plan = err
                .get("plan_type")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|p| format!(" ({} plan)", p.to_lowercase()))
                .unwrap_or_default();
            let when = err
                .get("resets_at")
                .and_then(Value::as_f64)
                .map(|resets_at| {
                    let mins = ((resets_at * 1000.0 - now_ms as f64) / 60_000.0).round();
                    format!(" Try again in ~{} min.", mins.max(0.0) as i64)
                })
                .unwrap_or_default();
            friendly = Some(
                format!("You have hit your ChatGPT usage limit{plan}.{when}")
                    .trim()
                    .to_string(),
            );
        }
        // `message = err.message || friendlyMessage || message`.
        if let Some(m) = err
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            message = m.to_string();
        } else if let Some(f) = &friendly {
            message = f.clone();
        }
    }

    friendly.unwrap_or(message)
}
