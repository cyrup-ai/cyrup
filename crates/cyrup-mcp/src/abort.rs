//! The abort helpers — `abort.ts`'s `throwIfAborted`/`abortable` and `runtime-owner.ts`'s
//! `combineAbortSignals`/`isAbortError` (13a §7, MCP-007).
//!
//! Two things depend on getting this right. An in-flight MCP request must abort promptly when
//! **either** the runtime owner or the caller's own signal fires — a request that keeps running
//! after the user pressed Esc holds a subprocess open past the turn. And an abort must be reported
//! **as an abort** rather than as a connection failure: `server-manager.ts` feeds connection
//! failures into a 60-second backoff, so misclassifying a user cancellation poisons the next
//! minute of that server's availability.
//!
//! # The typed replacement for a string compare
//!
//! Upstream's `isAbortError(error, signal?)` is `signal?.aborted` **or**
//! `error.name === "AbortError"` **or** the literal
//! `error.message === "MCP extension runtime stopped"`. The literal compare is why
//! [`crate::owner::DEFAULT_STOP_REASON`] is a constant and why a *custom* stop reason
//! deliberately does not match that arm upstream. Here the first arm is the token and the other two
//! collapse into [`McpError::Aborted`] — a typed variant — with
//! [`crate::owner::McpRuntimeOwner`]'s stored reason keeping the message round-trip intact.
//!
//! # The cost of `combine`, stated once
//!
//! `tokio-util` offers `child_token()` but **no any-of combinator**, so composing two independent
//! tokens costs one spawned task per combined pair. Bound it by combining **once per generation**
//! rather than once per request — which is what `initializeMcp`'s
//! `runtimeSignal = combineAbortSignals(owner.signal, initialSignal)` does (13a §8).
//!
//! # The caller-side half is a poll, not a wake
//!
//! `HostServices::is_run_cancelled` is cyrup's `ctx.signal`, and it is a documented CYRUP-DELTA: a
//! poll rather than a wake. `LiveHostServices` does not override it today, so it returns `false`
//! forever — until it does, the port uses the runtime owner's token alone and a user's Esc during
//! `/mcp reconnect` does not stop the connect (13a "What does not fit cleanly", item 3). That is
//! the one thing in this file a reader would otherwise trip over, because `is_run_cancelled()`
//! compiles, returns `false`, and looks correct.

use std::future::Future;

use cyrup_core::CancelToken;

use crate::errors::{McpError, McpResult};
use crate::owner::{McpRuntimeOwner, DEFAULT_STOP_REASON};

/// `combineAbortSignals(owner.signal, other)`.
///
/// Upstream filters out `undefined`, returns a single signal **unwrapped** (no allocation, and
/// identity is preserved so `isAbortError`'s signal arm still matches), and only calls
/// `AbortSignal.any` for two or more. This reproduces all three cases:
///
/// * `other == None` ⇒ a clone of `owner` — the identity-preserving arm.
/// * `other == Some` ⇒ a fresh token, cancelled by one spawned task the moment **either** parent
///   cancels.
///
/// The spawned task also exits when the combined token itself is cancelled, so a caller that
/// cancels the result directly reaps the task rather than leaving it parked until a parent fires.
/// Without a tokio runtime the combination degrades to the owner's token alone (this crate denies
/// `clippy::panic`, and `tokio::spawn` panics off-runtime) — losing the caller's half of the pair
/// is strictly better than losing the owner's.
#[must_use]
pub fn combine(owner: &CancelToken, other: Option<&CancelToken>) -> CancelToken {
    let Some(other) = other else {
        return owner.clone();
    };
    // Already-cancelled shortcuts: no task, no allocation beyond the clone.
    if owner.is_cancelled() {
        return owner.clone();
    }
    if other.is_cancelled() {
        return other.clone();
    }

    let combined = CancelToken::new();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("MCP: no tokio runtime to combine abort signals on — using the owner's");
        return owner.clone();
    };

    let a = owner.clone();
    let b = other.clone();
    let child = combined.clone();
    handle.spawn(async move {
        tokio::select! {
            () = a.cancelled() => child.cancel(),
            () = b.cancelled() => child.cancel(),
            // The joiner's own exit path: nothing left to observe once the child is cancelled.
            () = child.cancelled() => {}
        }
    });
    combined
}

/// `throwIfAborted(signal)` — return `Ok(())` unless the token has fired, in which case surface the
/// abort as [`McpError::Aborted`] carrying `reason` (upstream's `signal.reason` rethrow, whose
/// fallback text is `"MCP request aborted"`).
pub fn throw_if_aborted(token: &CancelToken, reason: Option<&str>) -> McpResult<()> {
    if !token.is_cancelled() {
        return Ok(());
    }
    Err(McpError::Aborted(reason.unwrap_or(ABORTED_FALLBACK_REASON).to_string()))
}

/// `abort.ts`'s fallback when `signal.reason` is not an `Error`:
/// `new Error(String(signal.reason ?? "MCP request aborted"))`.
pub const ABORTED_FALLBACK_REASON: &str = "MCP request aborted";

/// `abortable(promise, signal)` — race `fut` against `token`.
///
/// `biased;` puts the cancel arm first, which is upstream's `throwIfAborted(signal)` **before** the
/// race: an already-cancelled token never lets the future take a single poll. Callers that need the
/// stop reason preserved use [`abortable_owned`].
pub async fn abortable<F: Future>(fut: F, token: &CancelToken) -> McpResult<F::Output> {
    tokio::select! {
        biased;
        () = token.cancelled() => Err(McpError::Aborted(ABORTED_FALLBACK_REASON.to_string())),
        out = fut => Ok(out),
    }
}

/// [`abortable`] against a runtime owner, so the surfaced [`McpError::Aborted`] carries the exact
/// reason the owner was stopped with — `signal.reason`'s message, round-tripped.
pub async fn abortable_owned<F: Future>(fut: F, owner: &McpRuntimeOwner) -> McpResult<F::Output> {
    let token = owner.token();
    tokio::select! {
        biased;
        () = token.cancelled() => Err(McpError::Aborted(
            owner.stop_reason().map_or_else(|| DEFAULT_STOP_REASON.to_string(), |r| r.to_string()),
        )),
        out = fut => Ok(out),
    }
}

/// `isAbortError(error, signal?)`.
///
/// The typed first arm ([`McpError::Aborted`]) replaces upstream's `error.name === "AbortError"`
/// and its literal message compare; the optional token is upstream's `signal?.aborted`. Both arms
/// are needed: a cancelled token means the failure the caller is holding is *incidental* to the
/// abort even when it is not itself an abort error, which is exactly the case
/// `server-manager.ts` must not feed into its failure backoff.
#[must_use]
pub fn is_abort_error(error: &McpError, token: Option<&CancelToken>) -> bool {
    if let Some(token) = token
        && token.is_cancelled()
    {
        return true;
    }
    matches!(error, McpError::Aborted(_))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn combine_with_none_preserves_identity() {
        let owner = CancelToken::new();
        let combined = combine(&owner, None);
        owner.cancel();
        assert!(combined.is_cancelled());
    }

    #[tokio::test]
    async fn combined_cancels_when_either_parent_cancels() {
        for cancel_first in [true, false] {
            let a = CancelToken::new();
            let b = CancelToken::new();
            let combined = combine(&a, Some(&b));
            assert!(!combined.is_cancelled());
            if cancel_first { a.cancel() } else { b.cancel() }
            combined.cancelled().await;
            assert!(combined.is_cancelled());
        }
    }

    #[tokio::test]
    async fn cancelling_the_combined_token_does_not_reach_the_parents() {
        let a = CancelToken::new();
        let b = CancelToken::new();
        let combined = combine(&a, Some(&b));
        combined.cancel();
        // The joiner's third arm fires and the task ends; neither parent is disturbed, which is
        // what makes it safe to combine per generation and cancel per request.
        tokio::task::yield_now().await;
        assert!(combined.is_cancelled());
        assert!(!a.is_cancelled() && !b.is_cancelled());
    }

    #[tokio::test]
    async fn abortable_prefers_the_cancel_arm() {
        let token = CancelToken::new();
        token.cancel();
        let out = abortable(async { 7_u8 }, &token).await;
        assert!(matches!(out, Err(McpError::Aborted(_))));
    }

    #[tokio::test]
    async fn abortable_owned_carries_the_stop_reason() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let _ = owner.stop(Some("MCP extension session shutdown")).await;
        let err = abortable_owned(async { 1_u8 }, &owner).await.expect_err("stopped");
        assert_eq!(err.to_string(), "MCP extension session shutdown");
    }

    #[tokio::test]
    async fn abortable_passes_a_completed_future_through() {
        let token = CancelToken::new();
        assert_eq!(abortable(async { 42_u8 }, &token).await.unwrap(), 42);
    }

    #[test]
    fn abort_classification_covers_both_arms() {
        let cancelled = CancelToken::new();
        cancelled.cancel();
        let connect_failure = McpError::Server { server: "s".into(), message: "refused".into() };
        assert!(!is_abort_error(&connect_failure, None));
        assert!(is_abort_error(&connect_failure, Some(&cancelled)), "signal?.aborted arm");
        assert!(is_abort_error(&McpError::aborted_default(), None), "typed arm");
    }

    #[test]
    fn throw_if_aborted_uses_the_upstream_fallback_text() {
        let token = CancelToken::new();
        assert!(throw_if_aborted(&token, None).is_ok());
        token.cancel();
        assert_eq!(
            throw_if_aborted(&token, None).unwrap_err().to_string(),
            ABORTED_FALLBACK_REASON
        );
        assert_eq!(throw_if_aborted(&token, Some("custom")).unwrap_err().to_string(), "custom");
    }
}
