//! Transient provider/transport error classification (1:1 with Pi `utils/retry.ts`).
//!
//! Classifies whether a failed assistant turn looks like a transient provider or transport error,
//! so callers can decide whether to restart the last assistant turn. This is *only* a classifier —
//! it implements no retry policy, backoff, or budget (Pi `retry.ts:82-90`). The two pattern sets
//! and [`is_retryable_assistant_error`] are a faithful port of `retry.ts:7-96`.

use crate::utils::regexlite::Regex;
use cyrup_core::{AssistantMessage, StopReason};
use std::sync::OnceLock;

/// Provider/account-limit errors that are NOT transient and must never be retried (Pi
/// `NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN`, retry.ts:7-24).
const NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS: &[&str] = &[
    // OpenCode Go/free-tier subscription limits returned as 429 JSON error types.
    "GoUsageLimitError",
    "FreeUsageLimitError",
    // OpenCode Go subscription-limit text.
    "Monthly usage limit reached",
    "available balance",
    // Generic quota/budget/billing exhaustion.
    "insufficient_quota",
    "out of budget",
    "quota exceeded",
    "billing",
];

/// Transient provider/transport errors that ARE retryable (Pi `RETRYABLE_PROVIDER_ERROR_PATTERN`,
/// retry.ts:26-80).
const RETRYABLE_PROVIDER_PATTERNS: &[&str] = &[
    // Generic provider load, HTTP status, and server-side transient failures.
    "overloaded",
    "rate.?limit",
    "too many requests",
    "429",
    "500",
    "502",
    "503",
    "504",
    "service.?unavailable",
    "server.?error",
    "internal.?error",
    // Wrapper/provider text for transient upstream failures (OpenRouter "Provider returned error").
    "provider.?returned.?error",
    // Network, proxy, and fetch transport failures.
    "network.?error",
    "connection.?error",
    "connection.?refused",
    "connection.?lost",
    "other side closed",
    "fetch failed",
    "upstream.?connect",
    "reset before headers",
    "socket hang up",
    "timed? out",
    "timeout",
    "terminated",
    // WebSocket transports.
    "websocket.?closed",
    "websocket.?error",
    // Premature stream endings.
    "ended without",
    "stream ended before message_stop",
    "http2 request did not get a response",
    // Provider-requested retry delay cap failures.
    "retry delay",
    // Explicit retry guidance emitted mid-stream.
    "you can retry your request",
    "try your request again",
    "please retry your request",
];

fn non_retryable_regex() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| Regex::new(&NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS.join("|")))
}

fn retryable_regex() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| Regex::new(&RETRYABLE_PROVIDER_PATTERNS.join("|")))
}

/// Classifies whether a failed assistant message looks like a transient provider or transport error
/// (Pi `isRetryableAssistantError`, retry.ts:91-96).
///
/// Returns `false` unless the turn ended in `error` with an `errorMessage`; a known
/// non-retryable provider/account-limit pattern always wins (returns `false`); otherwise returns
/// whether the message matches a retryable provider/transport pattern.
///
/// This does not implement retry policy: callers should handle context overflow first
/// ([`super::overflow::is_context_overflow`]), then apply their own retry budget/backoff/reporting
/// before restarting the assistant turn.
pub fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let Some(error_message) = message.error_message.as_deref() else {
        return false;
    };
    if error_message.is_empty() {
        return false;
    }
    if non_retryable_regex().is_match(error_message) {
        return false;
    }
    retryable_regex().is_match(error_message)
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
    use cyrup_core::ProviderId;

    fn err(stop: StopReason, message: Option<&str>) -> AssistantMessage {
        let mut m = AssistantMessage::errored(
            ProviderId::from("openai"),
            "gpt",
            None,
            stop,
            message.unwrap_or(""),
        );
        m.error_message = message.map(str::to_string);
        m
    }

    #[test]
    fn non_error_turns_are_not_retryable() {
        assert!(!is_retryable_assistant_error(&err(
            StopReason::Stop,
            Some("overloaded")
        )));
        assert!(!is_retryable_assistant_error(&err(StopReason::Error, None)));
        assert!(!is_retryable_assistant_error(&err(
            StopReason::Error,
            Some("")
        )));
    }

    #[test]
    fn retryable_transient_errors() {
        for msg in [
            "Error: model overloaded, please try again",
            "429 Too Many Requests",
            "rate limit reached",
            "503 Service Unavailable",
            "internal error",
            "Provider returned error",
            "connection refused",
            "fetch failed",
            "socket hang up",
            "request timed out",
            "websocket closed unexpectedly",
            "Anthropic stream ended before message_stop",
            "http2 request did not get a response",
            "you can retry your request",
        ] {
            assert!(
                is_retryable_assistant_error(&err(StopReason::Error, Some(msg))),
                "should be retryable: {msg}"
            );
        }
    }

    #[test]
    fn non_retryable_account_limits_win() {
        for msg in [
            "GoUsageLimitError: limit reached",
            "FreeUsageLimitError",
            "Monthly usage limit reached, enable available balance",
            "insufficient_quota",
            "out of budget",
            "quota exceeded",
            "billing issue",
        ] {
            assert!(
                !is_retryable_assistant_error(&err(StopReason::Error, Some(msg))),
                "should NOT be retryable: {msg}"
            );
        }
    }

    #[test]
    fn non_retryable_wins_even_with_retryable_wording() {
        // "Monthly usage limit reached (429)" matches retryable "429" but the non-retryable limit
        // pattern takes precedence (Pi checks non-retryable first).
        let m = err(
            StopReason::Error,
            Some("Monthly usage limit reached (429 rate limit)"),
        );
        assert!(!is_retryable_assistant_error(&m));
    }

    #[test]
    fn unrelated_error_is_not_retryable() {
        assert!(!is_retryable_assistant_error(&err(
            StopReason::Error,
            Some("invalid api key")
        )));
    }
}
