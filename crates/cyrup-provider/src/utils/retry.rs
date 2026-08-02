//! Transient provider/transport error classification + the policy-driven retry loop (1:1 with Pi
//! `utils/retry.ts`).
//!
//! [`is_retryable_assistant_error`] classifies whether a failed assistant turn looks like a
//! transient provider or transport error; the two pattern sets and the classifier are a faithful
//! port of `retry.ts:7-96,222-227`. [`retry_assistant_call`] is the bounded-attempt/exponential-
//! backoff loop built on top of it (`retry.ts:162-211`) — Pi keeps the classifier and the loop in
//! the same module so both stay reusable by the SDK and other callers, and so does this port.

use crate::utils::regexlite::Regex;
use cyrup_core::{AssistantMessage, CancelToken, StopReason};
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

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

/// Retry policy: bounded attempts with exponential backoff (`base_delay_ms * 2^(attempt-1)`).
/// 1:1 with Pi `RetryPolicy` (`retry.ts:97-103`), which mirrors coding-agent's `settings.retry`
/// (`enabled` / `maxRetries` / `baseDelayMs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub enabled: bool,
    /// Max retry attempts (`0` = no retries). The initial call never counts as a retry.
    pub max_retries: u32,
    /// Base delay in ms. Per-attempt delay is `base_delay_ms * 2^(attempt-1)`.
    pub base_delay_ms: u64,
}

impl RetryPolicy {
    /// Pi's `retry: undefined` / `enabled: false` — the first response is returned unchanged
    /// (`retry.ts:159-160,168`).
    pub const DISABLED: Self = Self { enabled: false, max_retries: 0, base_delay_ms: 0 };

    pub fn new(enabled: bool, max_retries: u32, base_delay_ms: u64) -> Self {
        Self { enabled, max_retries, base_delay_ms }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::DISABLED
    }
}

/// Observation points emitted by [`retry_assistant_call`] around each retry — Pi `RetryCallbacks`
/// (`retry.ts:105-118`). Pi's callbacks are `void | Promise<void>` but every production
/// implementation is a synchronous event emit (`agent-session.ts:2647-2670`), so this port keeps
/// them synchronous. All three default to no-ops, matching Pi's optional members.
pub trait RetryObserver: Send + Sync {
    /// Before the backoff sleep of each retry attempt (1-indexed).
    fn on_retry_scheduled(
        &self,
        _attempt: u32,
        _max_attempts: u32,
        _delay_ms: u64,
        _error_message: &str,
    ) {
    }
    /// After the backoff sleep, immediately before the retried call starts.
    fn on_retry_attempt_start(&self) {}
    /// Once when the loop ends: `success` iff a later call completed normally.
    fn on_retry_finished(&self, _success: bool, _attempt: u32, _final_error: Option<&str>) {}
}

/// Run a single assistant-producing call with bounded retry on transient errors — a 1:1 port of Pi
/// `retryAssistantCall` (`retry.ts:144-211`).
///
/// Behavior:
/// - A successful response is returned immediately. Aborts are terminal and never retried, but
///   reported as unsuccessful if they happen after a retry was scheduled. An abort during the
///   backoff sleep is normalized to an aborted [`AssistantMessage`] too, so callers do not need to
///   care when cancellation happened.
/// - A non-retryable error (per [`is_retryable_assistant_error`], including quota/billing
///   exhaustion) is returned immediately so deterministic errors fail fast.
/// - Otherwise retries up to `policy.max_retries` times with exponential backoff.
///
/// When `policy` is disabled the first response is returned unchanged (equivalent to awaiting
/// `produce()` directly).
pub async fn retry_assistant_call<F, Fut>(
    produce: F,
    policy: RetryPolicy,
    cancel: Option<&CancelToken>,
    callbacks: Option<&dyn RetryObserver>,
) -> AssistantMessage
where
    F: Fn() -> Fut,
    Fut: Future<Output = AssistantMessage>,
{
    let max_attempts = if policy.enabled { policy.max_retries } else { 0 };
    let mut attempt: u32 = 0;
    // Pi's `lastRetry`: `onRetryFinished` fires only when at least one retry was scheduled.
    let mut last_retry: Option<u32> = None;

    loop {
        let response = produce().await;

        // Abort: terminal but not successful. Never retry an aborted message (`retry.ts:175-179`).
        if response.stop_reason == StopReason::Aborted {
            if let (Some(cb), Some(a)) = (callbacks, last_retry) {
                cb.on_retry_finished(false, a, None);
            }
            return response;
        }

        // Success: non-error, non-abort responses return as-is (`retry.ts:181-185`).
        if response.stop_reason != StopReason::Error {
            if let (Some(cb), Some(a)) = (callbacks, last_retry) {
                cb.on_retry_finished(true, a, None);
            }
            return response;
        }

        // Non-retryable, or budget exhausted: return the final error (`retry.ts:187-191`).
        if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
            if let (Some(cb), Some(a)) = (callbacks, last_retry) {
                cb.on_retry_finished(false, a, response.error_message.as_deref());
            }
            return response;
        }

        attempt = attempt.saturating_add(1);
        // `response.errorMessage || "Unknown error"` — an empty string is falsy in JS.
        let error_message = response
            .error_message
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown error".to_string());
        last_retry = Some(attempt);
        let delay_ms = policy
            .base_delay_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));
        if let Some(cb) = callbacks {
            cb.on_retry_scheduled(attempt, max_attempts, delay_ms, &error_message);
        }

        let slept = match cancel {
            Some(token) => token
                .run_until_cancelled(tokio::time::sleep(Duration::from_millis(delay_ms)))
                .await
                .is_some(),
            None => {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                true
            }
        };
        if !slept {
            if let Some(cb) = callbacks {
                cb.on_retry_finished(false, attempt, Some(&error_message));
            }
            // Normalize an abort during the backoff to the same shape as a provider stream abort
            // (`retry.ts:198-206`), so callers do not need to care when cancellation happened.
            let mut aborted = response;
            aborted.stop_reason = StopReason::Aborted;
            aborted.error_message = None;
            return aborted;
        }
        if let Some(cb) = callbacks {
            cb.on_retry_attempt_start();
        }
    }
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

    // ------------------------------------------------------- retry_assistant_call ------------

    /// A producer that pops scripted responses, recording how many attempts were made.
    struct Script {
        queue: std::sync::Mutex<std::collections::VecDeque<AssistantMessage>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Script {
        fn new(msgs: Vec<AssistantMessage>) -> Self {
            Self {
                queue: std::sync::Mutex::new(msgs.into()),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn next(&self) -> AssistantMessage {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.queue.lock().unwrap().pop_front().expect("scripted response")
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn ok_message() -> AssistantMessage {
        err(StopReason::Stop, None)
    }

    #[derive(Default)]
    struct Recorder {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl RetryObserver for Recorder {
        fn on_retry_scheduled(&self, attempt: u32, max: u32, delay: u64, msg: &str) {
            self.events.lock().unwrap().push(format!("scheduled {attempt}/{max} {delay}ms {msg}"));
        }
        fn on_retry_attempt_start(&self) {
            self.events.lock().unwrap().push("start".to_string());
        }
        fn on_retry_finished(&self, success: bool, attempt: u32, final_error: Option<&str>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("finished {success} {attempt} {final_error:?}"));
        }
    }

    #[tokio::test]
    async fn a_transient_error_is_retried_until_it_succeeds() {
        let script = Script::new(vec![err(StopReason::Error, Some("terminated")), ok_message()]);
        let rec = Recorder::default();
        let out = retry_assistant_call(
            || async { script.next() },
            RetryPolicy::new(true, 3, 0),
            None,
            Some(&rec),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::Stop);
        assert_eq!(script.calls(), 2);
        assert_eq!(
            *rec.events.lock().unwrap(),
            vec![
                "scheduled 1/3 0ms terminated".to_string(),
                "start".to_string(),
                "finished true 1 None".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn the_budget_bounds_the_attempts_and_the_last_error_is_returned() {
        let script = Script::new(vec![
            err(StopReason::Error, Some("overloaded")),
            err(StopReason::Error, Some("overloaded")),
            err(StopReason::Error, Some("503 last")),
        ]);
        let out = retry_assistant_call(
            || async { script.next() },
            RetryPolicy::new(true, 2, 0),
            None,
            None,
        )
        .await;
        assert_eq!(script.calls(), 3, "initial call + 2 retries");
        assert_eq!(out.error_message.as_deref(), Some("503 last"));
    }

    #[tokio::test]
    async fn a_disabled_policy_returns_the_first_response_unchanged() {
        let script = Script::new(vec![err(StopReason::Error, Some("terminated")), ok_message()]);
        let out = retry_assistant_call(
            || async { script.next() },
            RetryPolicy::DISABLED,
            None,
            None,
        )
        .await;
        assert_eq!(script.calls(), 1);
        assert_eq!(out.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn aborted_and_non_retryable_responses_are_terminal() {
        let policy = RetryPolicy::new(true, 3, 0);

        let aborted = Script::new(vec![err(StopReason::Aborted, None), ok_message()]);
        let out =
            retry_assistant_call(|| async { aborted.next() }, policy, None, None).await;
        assert_eq!(aborted.calls(), 1, "an abort is never retried");
        assert_eq!(out.stop_reason, StopReason::Aborted);

        let quota =
            Script::new(vec![err(StopReason::Error, Some("insufficient_quota")), ok_message()]);
        let out = retry_assistant_call(|| async { quota.next() }, policy, None, None).await;
        assert_eq!(quota.calls(), 1, "a deterministic error fails fast");
        assert_eq!(out.error_message.as_deref(), Some("insufficient_quota"));
    }

    #[tokio::test]
    async fn cancelling_during_the_backoff_yields_an_aborted_message() {
        let script = Script::new(vec![err(StopReason::Error, Some("terminated"))]);
        let cancel = CancelToken::new();
        cancel.cancel();
        let rec = Recorder::default();
        let out = retry_assistant_call(
            || async { script.next() },
            RetryPolicy::new(true, 3, 60_000),
            Some(&cancel),
            Some(&rec),
        )
        .await;
        assert_eq!(script.calls(), 1, "the retried call never starts");
        assert_eq!(out.stop_reason, StopReason::Aborted, "normalized like a stream abort");
        assert!(out.error_message.is_none());
        assert_eq!(
            *rec.events.lock().unwrap(),
            vec![
                "scheduled 1/3 60000ms terminated".to_string(),
                "finished false 1 Some(\"terminated\")".to_string(),
            ]
        );
    }
}
