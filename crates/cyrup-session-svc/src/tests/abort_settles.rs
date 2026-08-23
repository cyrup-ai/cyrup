//! SEAM-023 + SEAM-024 — `AgentSession::abort()` must cancel the auto-retry backoff, and every
//! caller that depends on "aborted" meaning "stopped" must AWAIT that.
//!
//! Pi ground truth, `packages/coding-agent/src/core/agent-session.ts:1542-1546`:
//!
//! ```text
//! async abort(): Promise<void> {
//!     this.abortRetry();
//!     this.agent.abort();
//!     await this.waitForIdle();
//! }
//! ```
//!
//! Three facts hang off that body.
//!
//!  1. **`abortRetry()` first** (SEAM-023). cyrup's retry backoff sleeps on a child of
//!     `session_cancel`, a DIFFERENT token from the per-run one `agent.abort()` fires — so a plain
//!     `agent.abort()` leaves the backoff running and the retry fires later, against a session the
//!     user already aborted.
//!  2. **`await waitForIdle()`** (SEAM-024). `agent-session-runtime.ts:167-169` spells out why
//!     teardown depends on it: "Settle any active response first so the aborted turn (including tool
//!     results) is persisted to the outgoing session before it is replaced." Pi's RPC `abort` verb
//!     likewise replies only after `await session.abort()` (rpc-mode.ts:427-430).
//!  3. **That exact ORDER.** Awaiting idle before cancelling the retry would block for the whole
//!     remaining backoff — up to `baseDelayMs * 2^attempt`. These tests use a 600 s backoff, so an
//!     implementation that inverted the two would hang rather than fail.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxMessageOptions, FauxProvider, faux_assistant_message, faux_assistant_message_with, faux_text,
};
use cyrup_core::StopReason;
use super::common::{base_config, fixture, Fixture};
use crate::{AgentSession, AgentSessionEvent, InputSource, SessionBuilder, UserInput};
use futures::StreamExt;

/// A 10-minute first retry backoff. Long enough that "the abort did not cancel it" is unambiguous:
/// nothing in these tests waits more than a few seconds.
fn slow_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "retry",
        serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 600_000}),
    )
    .unwrap();
    cli
}

/// Build a bound session whose first turn is a RETRYABLE transient error and whose second turn
/// (the auto-retry continuation, which must never happen here) is a clean success.
async fn session_stuck_in_retry_backoff(fx: &Fixture) -> (Arc<AgentSession>, Arc<FauxProvider>) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions { error_message: Some("overloaded".into()), ..Default::default() },
        ),
        faux_assistant_message(vec![faux_text("retried anyway")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(fx))
        .cli_settings(slow_retry_settings())
        .build()
        .await
        .expect("build")
        .into_shared(); // bind the self-handle: the post-run retry loop is LIVE.
    (session, faux)
}

/// Poll until the session has actually entered its retry backoff (Pi `isRetrying`,
/// agent-session.ts:2553). Fails the test rather than hanging if the run never gets there.
async fn await_retry_backoff(session: &AgentSession) {
    for _ in 0..600 {
        if session.is_retrying() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("session never entered the retry backoff — fixture is wrong, not the code under test");
}

// ================================================================================== SEAM-023 ====

/// THE SEAM-023 proof, at the level a user observes it: Escape / SIGINT / the RPC `abort` verb all
/// land on `AgentSession::abort()`, and after it the pending provider-retry must be DEAD — the
/// provider must never be called again and the retry must report itself cancelled.
///
/// Pre-fix `abort()` was `self.agent.abort();` alone, so the 600 s backoff kept sleeping.
#[tokio::test]
async fn abort_cancels_the_retry_backoff() {
    let fx = fixture();
    let (session, faux) = session_stuck_in_retry_backoff(&fx).await;

    let stream = session
        .prompt(UserInput::text("please do the thing", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    await_retry_backoff(&session).await;
    assert_eq!(faux.call_count(), 1, "only the failing turn has run so far");

    session.abort();

    // The whole run must settle promptly. Pre-fix this expires: the backoff owns a separate token.
    tokio::time::timeout(Duration::from_secs(5), session.wait_for_idle())
        .await
        .expect("abort() must cancel the retry backoff so the run settles (Pi abortRetry() first)");

    assert!(!session.is_retrying(), "no retry may still be in flight after abort()");
    assert_eq!(
        faux.call_count(),
        1,
        "the aborted retry must NOT fire a second provider call against an aborted session"
    );

    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let cancelled = events.iter().any(|e| {
        matches!(e, AgentSessionEvent::AutoRetryEnd { success: false, final_error: Some(m), .. }
            if m == "Retry cancelled")
    });
    assert!(
        cancelled,
        "auto_retry_end{{success:false, final_error:\"Retry cancelled\"}} must be emitted: {:?}",
        events.iter().map(AgentSessionEvent::kind).collect::<Vec<_>>()
    );
}

// ================================================================================== SEAM-024 ====

/// `abort_and_settle()` is Pi's full `await abort()`: when it returns, the run has STOPPED. The
/// order is the deadlock-sensitive part — `abortRetry()` before `waitForIdle()`. If it were
/// inverted this call would block on a 600 s sleep and the timeout below would expire.
#[tokio::test]
async fn abort_and_settle_returns_only_once_the_run_is_idle() {
    let fx = fixture();
    let (session, _faux) = session_stuck_in_retry_backoff(&fx).await;

    let _stream = session
        .prompt(UserInput::text("please do the thing", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    await_retry_backoff(&session).await;
    assert!(!session.is_idle(), "precondition: the run is live (in retry backoff)");

    tokio::time::timeout(Duration::from_secs(5), session.abort_and_settle())
        .await
        .expect("abort_and_settle must not block on the backoff it is supposed to cancel");

    assert!(
        session.is_idle(),
        "abort_and_settle() must return only after the run has settled (Pi `await this.waitForIdle()`)"
    );
}

/// THE SEAM-024 proof, as a teardown-ordering assertion rather than a timing one.
///
/// Pi's `teardownCurrent` is `await this.session.abort(); await emitSessionShutdownEvent(...);
/// this.session.dispose();` (agent-session-runtime.ts:167-176) — the aborted turn is fully settled
/// BEFORE `session_shutdown` is announced, "so the aborted turn (including tool results) is
/// persisted to the outgoing session before it is replaced".
///
/// So on the wire, `session_shutdown` must be the LAST event of the session: everything the dying
/// run still had to say (`auto_retry_end`, `agent_end`, `agent_settled`) precedes it. Pre-fix
/// `dispose` called the fire-and-forget `abort()` and emitted `session_shutdown` immediately, so it
/// landed in the MIDDLE of the run's own terminal events.
#[tokio::test]
async fn dispose_announces_session_shutdown_after_the_run_has_settled() {
    let fx = fixture();
    let (session, _faux) = session_stuck_in_retry_backoff(&fx).await;

    // One session-wide subscription, exactly what a front-end holds.
    let seen: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let mut events = session.subscribe();
    let sink = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            if let Ok(mut g) = sink.lock() {
                g.push(ev.kind());
            }
        }
    });

    let _stream = session
        .prompt(UserInput::text("please do the thing", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    await_retry_backoff(&session).await;

    tokio::time::timeout(Duration::from_secs(5), session.dispose("quit"))
        .await
        .expect("dispose must settle the aborted run, not block on its backoff");

    // Let the recorder task drain the events dispose emitted.
    for _ in 0..200 {
        if seen.lock().unwrap().contains(&"session_shutdown") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let kinds = seen.lock().unwrap().clone();

    let shutdown_at = kinds
        .iter()
        .position(|k| *k == "session_shutdown")
        .unwrap_or_else(|| panic!("session_shutdown must be emitted by dispose: {kinds:?}"));
    let settled_at = kinds
        .iter()
        .position(|k| *k == "agent_settled")
        .unwrap_or_else(|| panic!("the aborted run must still reach agent_settled: {kinds:?}"));
    let retry_end_at = kinds
        .iter()
        .position(|k| *k == "auto_retry_end")
        .unwrap_or_else(|| panic!("the cancelled backoff must emit auto_retry_end: {kinds:?}"));

    assert!(
        retry_end_at < shutdown_at,
        "the aborted turn's auto_retry_end must precede session_shutdown: {kinds:?}"
    );
    assert!(
        settled_at < shutdown_at,
        "the run must be settled BEFORE session_shutdown is announced \
         (Pi teardownCurrent, agent-session-runtime.ts:167-174): {kinds:?}"
    );
    assert_eq!(
        kinds.last().copied(),
        Some("session_shutdown"),
        "session_shutdown must be the session's final event: {kinds:?}"
    );
}
