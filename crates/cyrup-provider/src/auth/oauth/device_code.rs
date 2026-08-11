//! The RFC 8628 device-code polling loop.
//!
//! 1:1 port of pi v0.83.0 `packages/ai/src/auth/oauth/device-code.ts`. Shared by
//! `github-copilot.ts:201`, `xai.ts:162`, `kimi-coding.ts:146`, `openai-codex.ts:235` and
//! `radius.ts:320`, so every timing rule here is load-bearing for five flows.
//!
//! Divergences, both language-forced:
//!
//! * `AbortSignal` is [`CancelToken`] (arch-00 §3.2); `abortableSleep` (`device-code.ts:26-44`)
//!   becomes a `select!` between `tokio::time::sleep` and `CancelToken::cancelled`.
//! * `poll()` may *throw* upstream (`fetchJson` rejects on a transport error) and that rejection
//!   propagates out of `pollOAuthDeviceCodeFlow`. Rust has no throw, so [`DeviceCodePoller::poll`]
//!   returns `Result<_, OAuthError>` and an `Err` propagates identically.

use super::{OAuthError, interaction::AuthEvent};
use cyrup_core::CancelToken;
use std::time::{Duration, Instant};

/// `MINIMUM_INTERVAL_MS` (`device-code.ts:5`).
const MINIMUM_INTERVAL_MS: u64 = 1000;
/// `DEFAULT_POLL_INTERVAL_SECONDS` (`device-code.ts:7`) — RFC 8628 §3.2: absent `interval` means
/// 5 seconds.
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
/// `SLOW_DOWN_INTERVAL_INCREMENT_MS` (`device-code.ts:9`) — RFC 8628 §3.5.
const SLOW_DOWN_INTERVAL_INCREMENT_MS: u64 = 5000;

/// One poll outcome (`OAuthDeviceCodePollResult`, `device-code.ts:11-16`).
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceCodePollResult<T> {
    /// `{ status: "pending" }` — `authorization_pending`; keep waiting.
    Pending,
    /// `{ status: "slow_down" }`. `interval_seconds` carries the server's new minimum when it
    /// supplied one (GitHub does).
    SlowDown { interval_seconds: Option<f64> },
    /// `{ status: "failed", message }` — terminal; `message` becomes the error verbatim.
    Failed { message: String },
    /// `{ status: "complete", value }`.
    Complete(T),
}

/// `OAuthDeviceCodePollOptions` minus `poll` (`device-code.ts:18-24`), which is
/// [`DeviceCodePoller`].
#[derive(Clone, Debug, Default)]
pub struct DeviceCodePollOptions {
    /// The server's `interval`, in seconds.
    pub interval_seconds: Option<f64>,
    /// The server's `expires_in`, in seconds. `None` = no deadline (upstream's
    /// `Number.POSITIVE_INFINITY`, `device-code.ts:50`).
    pub expires_in_seconds: Option<f64>,
    /// Sleep one interval before the first poll (`device-code.ts:57-62`). Every real flow sets
    /// this: the user has not opened the verification page yet.
    pub wait_before_first_poll: bool,
    pub cancel: Option<CancelToken>,
}

impl DeviceCodePollOptions {
    /// Build the options from the `device_code` response the flow just parsed, and from the
    /// [`AuthEvent::DeviceCode`] it is about to emit — the two always carry the same hints, so
    /// this keeps them from drifting.
    pub fn from_device_code_event(event: &AuthEvent) -> Self {
        match event {
            AuthEvent::DeviceCode {
                interval_seconds,
                expires_in_seconds,
                ..
            } => Self {
                interval_seconds: *interval_seconds,
                expires_in_seconds: *expires_in_seconds,
                wait_before_first_poll: true,
                cancel: None,
            },
            _ => Self::default(),
        }
    }

    #[must_use]
    pub fn with_cancel(mut self, cancel: Option<CancelToken>) -> Self {
        self.cancel = cancel;
        self
    }
}

/// The `poll` callback (`device-code.ts:22`).
#[async_trait::async_trait]
pub trait DeviceCodePoller: Send + Sync {
    /// What a completed flow yields — an access token, a credential, a token response.
    type Value: Send;

    /// One token-endpoint round trip. An `Err` propagates out of
    /// [`poll_oauth_device_code_flow`] unchanged, matching a thrown `fetchJson` rejection.
    async fn poll(&self) -> Result<DeviceCodePollResult<Self::Value>, OAuthError>;
}

/// `abortableSleep` (`device-code.ts:26-44`): resolve after `duration`, or reject immediately
/// with `"Login cancelled"` if the signal is (or becomes) aborted.
async fn abortable_sleep(
    duration: Duration,
    cancel: Option<&CancelToken>,
) -> Result<(), OAuthError> {
    match cancel {
        Some(token) => {
            if token.is_cancelled() {
                return Err(OAuthError::Cancelled);
            }
            tokio::select! {
                biased;
                () = token.cancelled() => Err(OAuthError::Cancelled),
                () = tokio::time::sleep(duration) => Ok(()),
            }
        }
        None => {
            tokio::time::sleep(duration).await;
            Ok(())
        }
    }
}

/// The next polling interval after a `slow_down` (`device-code.ts:82-86`).
///
/// Trust the server's `interval` when it supplied a finite positive one — GitHub reports the new
/// required minimum there, and a client-tracked value alone polls early forever under WSL/VM
/// clock drift. Otherwise apply RFC 8628 §3.5 and add 5 seconds. Either way the floor is
/// `MINIMUM_INTERVAL_MS`.
///
/// Extracted as a free function (upstream inlines it) so the rule is assertable without waiting
/// out real seconds.
pub fn next_interval_ms(current_ms: u64, server_interval_seconds: Option<f64>) -> u64 {
    match server_interval_seconds {
        Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
            MINIMUM_INTERVAL_MS.max(floor_ms(seconds))
        }
        _ => MINIMUM_INTERVAL_MS.max(current_ms.saturating_add(SLOW_DOWN_INTERVAL_INCREMENT_MS)),
    }
}

/// `Math.floor(seconds * 1000)`, clamped into `u64`.
fn floor_ms(seconds: f64) -> u64 {
    let ms = (seconds * 1000.0).floor();
    if ms.is_finite() && ms > 0.0 {
        ms as u64
    } else {
        0
    }
}

/// `pollOAuthDeviceCodeFlow` (`device-code.ts:46-98`).
///
/// Polls until the poller reports `complete` (returned), `failed` (its message becomes
/// [`OAuthError::Failed`]), the `expires_in` deadline passes ([`OAuthError::DeviceFlowTimeout`],
/// or [`OAuthError::DeviceFlowSlowDownTimeout`] if any `slow_down` was seen — the WSL/VM clock
/// drift hint, `device-code.ts:97`), or the login is cancelled ([`OAuthError::Cancelled`]).
pub async fn poll_oauth_device_code_flow<P: DeviceCodePoller>(
    options: &DeviceCodePollOptions,
    poller: &P,
) -> Result<P::Value, OAuthError> {
    let start = Instant::now();
    // `Number.POSITIVE_INFINITY` when `expiresInSeconds` is absent (device-code.ts:48-51).
    let deadline: Option<Instant> = options
        .expires_in_seconds
        .map(|seconds| start + Duration::from_millis(floor_ms(seconds.max(0.0))));
    let cancel = options.cancel.as_ref();

    let mut interval_ms = MINIMUM_INTERVAL_MS.max(floor_ms(
        options
            .interval_seconds
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS),
    ));

    let remaining = |deadline: Option<Instant>| -> Option<Duration> {
        deadline.map(|d| d.saturating_duration_since(Instant::now()))
    };

    let mut slow_down_responses = 0usize;
    if options.wait_before_first_poll {
        // `remainingMs > 0` (device-code.ts:58); with no deadline this is always true.
        let wait = match remaining(deadline) {
            Some(rem) if rem.is_zero() => None,
            Some(rem) => Some(rem.min(Duration::from_millis(interval_ms))),
            None => Some(Duration::from_millis(interval_ms)),
        };
        if let Some(wait) = wait {
            abortable_sleep(wait, cancel).await?;
        }
    }

    // `while (Date.now() < deadline)` (device-code.ts:64).
    while deadline.is_none_or(|d| Instant::now() < d) {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(OAuthError::Cancelled);
        }

        match poller.poll().await? {
            DeviceCodePollResult::Complete(value) => return Ok(value),
            DeviceCodePollResult::Failed { message } => return Err(OAuthError::Failed(message)),
            DeviceCodePollResult::SlowDown { interval_seconds } => {
                slow_down_responses += 1;
                interval_ms = next_interval_ms(interval_ms, interval_seconds);
            }
            DeviceCodePollResult::Pending => {}
        }

        // `if (remainingMs <= 0) break` (device-code.ts:89-92).
        let wait = match remaining(deadline) {
            Some(rem) if rem.is_zero() => break,
            Some(rem) => rem.min(Duration::from_millis(interval_ms)),
            None => Duration::from_millis(interval_ms),
        };
        abortable_sleep(wait, cancel).await?;
    }

    Err(if slow_down_responses > 0 {
        OAuthError::DeviceFlowSlowDownTimeout
    } else {
        OAuthError::DeviceFlowTimeout
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A poller that replays a script and counts calls.
    struct Scripted {
        results: Mutex<Vec<DeviceCodePollResult<String>>>,
        calls: AtomicUsize,
        /// When the script runs out: `None` keeps returning `Pending`, `Some(e)` errors.
        exhausted: Option<&'static str>,
    }

    impl Scripted {
        fn new(results: Vec<DeviceCodePollResult<String>>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().rev().collect()),
                calls: AtomicUsize::new(0),
                exhausted: None,
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl DeviceCodePoller for Scripted {
        type Value = String;
        async fn poll(&self) -> Result<DeviceCodePollResult<String>, OAuthError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.results.lock().ok().and_then(|mut r| r.pop());
            match next {
                Some(result) => Ok(result),
                None => match self.exhausted {
                    Some(message) => Err(OAuthError::Failed(message.to_string())),
                    None => Ok(DeviceCodePollResult::Pending),
                },
            }
        }
    }

    fn opts() -> DeviceCodePollOptions {
        DeviceCodePollOptions::default()
    }

    #[tokio::test]
    async fn completes_on_the_first_poll() {
        let poller = Scripted::new(vec![DeviceCodePollResult::Complete("token".into())]);
        let value = poll_oauth_device_code_flow(&opts(), &poller).await.unwrap();
        assert_eq!(value, "token");
        assert_eq!(poller.calls(), 1);
    }

    /// `{ status: "failed" }` surfaces its message verbatim (`device-code.ts:73`) — this is the
    /// exact string `github-copilot.ts:236` builds.
    #[tokio::test]
    async fn failed_status_propagates_the_message_verbatim() {
        let poller = Scripted::new(vec![DeviceCodePollResult::Failed {
            message: "Device flow failed: access_denied: The user denied access".into(),
        }]);
        let err = poll_oauth_device_code_flow(&opts(), &poller)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Device flow failed: access_denied: The user denied access"
        );
    }

    /// A poll that throws propagates unchanged rather than being swallowed as `pending`.
    #[tokio::test]
    async fn poll_errors_propagate() {
        struct Boom;
        #[async_trait::async_trait]
        impl DeviceCodePoller for Boom {
            type Value = String;
            async fn poll(&self) -> Result<DeviceCodePollResult<String>, OAuthError> {
                Err(OAuthError::Failed("fetch failed".into()))
            }
        }
        let err = poll_oauth_device_code_flow(&opts(), &Boom)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "fetch failed");
    }

    /// An already-expired `expires_in` never polls at all and reports the plain timeout message.
    #[tokio::test]
    async fn expired_deadline_times_out_without_polling() {
        let poller = Scripted::new(vec![DeviceCodePollResult::Complete("late".into())]);
        let options = DeviceCodePollOptions {
            expires_in_seconds: Some(0.0),
            ..Default::default()
        };
        let err = poll_oauth_device_code_flow(&options, &poller)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Device flow timed out");
        assert_eq!(poller.calls(), 0, "deadline is checked before polling");
    }

    /// Cancellation before the first poll rejects with upstream's `CANCEL_MESSAGE`.
    #[tokio::test]
    async fn cancelled_login_rejects_with_login_cancelled() {
        let poller = Scripted::new(vec![DeviceCodePollResult::Complete("token".into())]);
        let token = CancelToken::new();
        token.cancel();
        let options = DeviceCodePollOptions {
            cancel: Some(token),
            ..Default::default()
        };
        let err = poll_oauth_device_code_flow(&options, &poller)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
        assert_eq!(poller.calls(), 0);
    }

    /// `waitBeforeFirstPoll` sleeps first, so a token that is cancelled during that sleep aborts
    /// before any request is made.
    #[tokio::test]
    async fn cancel_during_the_initial_wait_aborts_before_polling() {
        let poller = Scripted::new(vec![DeviceCodePollResult::Complete("token".into())]);
        let token = CancelToken::new();
        let options = DeviceCodePollOptions {
            interval_seconds: Some(30.0),
            wait_before_first_poll: true,
            cancel: Some(token.clone()),
            ..Default::default()
        };
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            token.cancel();
        });
        let err = poll_oauth_device_code_flow(&options, &poller)
            .await
            .unwrap_err();
        cancel_task.await.unwrap();
        assert_eq!(err.to_string(), "Login cancelled");
        assert_eq!(poller.calls(), 0);
    }

    /// RFC 8628 §3.5 vs the server-supplied override (`device-code.ts:82-86`), and the 1 s floor
    /// (`MINIMUM_INTERVAL_MS`, `device-code.ts:5`).
    #[test]
    fn slow_down_interval_rules() {
        // No server interval: +5000 ms.
        assert_eq!(next_interval_ms(5000, None), 10_000);
        // Server interval wins, even when it is lower than the current one.
        assert_eq!(next_interval_ms(30_000, Some(7.0)), 7_000);
        // Floored at 1 s.
        assert_eq!(next_interval_ms(1000, Some(0.2)), MINIMUM_INTERVAL_MS);
        // Non-finite / non-positive server values fall back to the RFC rule.
        assert_eq!(next_interval_ms(1000, Some(0.0)), 6_000);
        assert_eq!(next_interval_ms(1000, Some(-3.0)), 6_000);
        assert_eq!(next_interval_ms(1000, Some(f64::INFINITY)), 6_000);
        assert_eq!(next_interval_ms(1000, Some(f64::NAN)), 6_000);
        // `Math.floor` truncates rather than rounds.
        assert_eq!(next_interval_ms(1000, Some(2.9995)), 2_999);
    }

    /// Timing out *after* a `slow_down` swaps in the clock-drift message (`device-code.ts:97`).
    /// The interval is floored at 1 s upstream, so this test genuinely waits ~1 s.
    #[tokio::test]
    async fn slow_down_then_timeout_reports_the_clock_drift_message() {
        let poller = Scripted::new(vec![DeviceCodePollResult::SlowDown {
            interval_seconds: None,
        }]);
        let options = DeviceCodePollOptions {
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(0.4),
            ..Default::default()
        };
        let started = Instant::now();
        let err = poll_oauth_device_code_flow(&options, &poller)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Device flow timed out after one or more slow_down responses. This is often caused by \
             clock drift in WSL or VM environments. Please sync or restart the VM clock and try \
             again."
        );
        assert_eq!(poller.calls(), 1);
        // The post-poll sleep is min(interval, remaining), so it never overruns the deadline.
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "slept past the deadline: {:?}",
            started.elapsed()
        );
    }

    /// Without a `slow_down`, the same shape reports the plain timeout — the two messages are the
    /// only way a caller can tell the cases apart.
    #[tokio::test]
    async fn pending_then_timeout_reports_the_plain_message() {
        let poller = Scripted::new(vec![DeviceCodePollResult::Pending]);
        let options = DeviceCodePollOptions {
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(0.4),
            ..Default::default()
        };
        let err = poll_oauth_device_code_flow(&options, &poller)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Device flow timed out");
        assert_eq!(poller.calls(), 1);
    }

    /// Options lifted straight off the `device_code` event a flow emits.
    #[test]
    fn options_from_device_code_event() {
        let event = AuthEvent::DeviceCode {
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            interval_seconds: Some(5.0),
            expires_in_seconds: Some(900.0),
        };
        let options = DeviceCodePollOptions::from_device_code_event(&event);
        assert_eq!(options.interval_seconds, Some(5.0));
        assert_eq!(options.expires_in_seconds, Some(900.0));
        assert!(options.wait_before_first_poll);
    }
}
