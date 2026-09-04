//! Retry decisions.

use super::*;

#[test]
fn terminal_rate_limit_vectors() {
    // pi's /GoUsageLimitError|FreeUsageLimitError|Monthly usage limit reached|available
    // balance|insufficient_quota|out of budget|quota exceeded|billing/i (:131-133).
    for text in [
        "GoUsageLimitError",
        "freeusagelimiterror",
        "Monthly usage limit reached",
        "your available balance is 0",
        "insufficient_quota",
        "you are out of budget",
        "QUOTA EXCEEDED",
        "billing",
    ] {
        assert!(
            is_terminal_rate_limit_error(text),
            "expected terminal: {text}"
        );
    }
    // MIRROR: an ordinary rate-limit body is NOT terminal.
    assert!(!is_terminal_rate_limit_error(
        "Rate limit exceeded, retry soon"
    ));
}

#[test]
fn retryable_status_and_text_vectors() {
    // pi isRetryableError (:136-144).
    assert!(is_retryable_error(429, "Rate limit exceeded"));
    assert!(!is_retryable_error(429, "insufficient_quota"));
    for status in [500u16, 502, 503, 504] {
        assert!(is_retryable_error(status, ""), "status {status}");
    }
    // `.?` = at most one arbitrary character.
    assert!(is_retryable_error(400, "rate limit hit"));
    assert!(is_retryable_error(400, "rate-limit hit"));
    assert!(is_retryable_error(400, "ratelimit hit"));
    assert!(is_retryable_error(400, "OVERLOADED"));
    assert!(is_retryable_error(400, "service unavailable"));
    assert!(is_retryable_error(400, "service_unavailable"));
    assert!(is_retryable_error(400, "upstream connect error"));
    assert!(is_retryable_error(400, "connection refused"));
    // MIRROR: an ordinary 4xx with none of those markers is not retried.
    assert!(!is_retryable_error(400, "invalid request: bad model"));
    assert!(!is_retryable_error(401, "unauthorized"));
    // Two characters between the halves is more than `.?` allows.
    assert!(!is_retryable_error(400, "rate  limit"));
}

#[test]
fn retry_after_header_precedence() {
    // pi getRetryAfterDelayMs (:146-171): retry-after-ms wins over retry-after.
    assert_eq!(
        get_retry_after_delay_ms(&headers(&[
            ("retry-after-ms", "1500"),
            ("retry-after", "9")
        ])),
        Some(1500)
    );
    assert_eq!(
        get_retry_after_delay_ms(&headers(&[("retry-after", "2")])),
        Some(2000)
    );
    // Negative values clamp to zero (`Math.max(0, …)`).
    assert_eq!(
        get_retry_after_delay_ms(&headers(&[("retry-after-ms", "-10")])),
        Some(0)
    );
    // A past HTTP-date clamps to zero rather than going negative.
    assert_eq!(
        get_retry_after_delay_ms(&headers(&[(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT"
        )])),
        Some(0)
    );
    // Unparseable → undefined (the caller then uses the exponential ladder).
    assert_eq!(
        get_retry_after_delay_ms(&headers(&[("retry-after", "soon")])),
        None
    );
    assert_eq!(get_retry_after_delay_ms(&headers(&[])), None);
}

#[test]
fn retry_delay_ceiling_message_is_upstreams() {
    // pi validateRetryDelayMs (:175-183).
    assert_eq!(validate_retry_delay_ms(30_000, None), Ok(30_000));
    assert_eq!(
        validate_retry_delay_ms(90_000, None).unwrap_err(),
        "Server requested 90s retry delay (max: 60s)"
    );
    // `maxRetryDelayMs > 0` gate: zero disables the ceiling entirely.
    assert_eq!(validate_retry_delay_ms(900_000, Some(0)), Ok(900_000));
    assert_eq!(
        validate_retry_delay_ms(11_000, Some(10_000)).unwrap_err(),
        "Server requested 11s retry delay (max: 10s)"
    );
}

#[test]
fn exponential_ladder_has_no_jitter() {
    // pi `BASE_DELAY_MS * 2 ** attempt` (:433).
    assert_eq!(backoff_delay_ms(0), 1_000);
    assert_eq!(backoff_delay_ms(1), 2_000);
    assert_eq!(backoff_delay_ms(2), 4_000);
    assert_eq!(backoff_delay_ms(3), 8_000);
}
