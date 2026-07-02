//! Optional out-of-band result delivery ("intercom") and the foreground clarify/ask pause
//! primitive (func-SA §5.5; arch-SA §6.7 "Intercom degradation"; R-SA-119/120/123/124/125).
//!
//! # Scope
//!
//! This file owns exactly two things:
//!
//! 1. **Out-of-band result delivery** ([`IntercomPayload`]/[`deliver`]): a *best-effort*,
//!    gracefully-degrading side-channel a grouped subagent result MAY be pushed through, in
//!    addition to (never instead of) the ordinary inline tool-result payload. Per R-SA-125 and
//!    func-SA §9 item 25 / arch-SA §12 item 7, no `pi-intercom` companion transport is confirmed
//!    ported into this workspace today, so [`deliver`] always resolves to
//!    [`DeliveryOutcome::NotDelivered`] in current builds — this module is written so that once a
//!    real transport exists, wiring it in means supplying a [`DeliveryChannel`] impl and nothing
//!    else changes at any call site (the timeout-race, allowlist-projection, and "never block/
//!    error the caller's turn" contracts are already correct and already tested here).
//! 2. **The foreground clarify/ask pause primitive** ([`ClarifyRequest`]/[`AskLock`]/
//!    [`request_clarify`]): R-SA-119's "visibly pause the affected foreground flow while a child's
//!    clarify request is outstanding" and R-SA-120's "at most one outstanding blocking ask per
//!    orchestrator session" single-slot lock. Per arch-SA §12 item 6, a REAL, wired mechanism for
//!    this now exists (`LiveHostServices::{confirm,input,select,editor}` in
//!    `cyrup-session-svc/src/host_services.rs`, reachable via a constructor-time
//!    `Arc<AgentSessionServices>`/`Arc<LiveHostServices>` handle mirroring this crate's existing
//!    narrow, direct `cyrup-session` dependency for fork-context) — but reaching it requires
//!    adding `cyrup-session-svc` as a dependency of this crate's `Cargo.toml` and threading a
//!    handle through the extension's construction path, both of which are outside this single
//!    file's ownership boundary for this task (this task owns only `tui/intercom.rs`). **This is
//!    therefore explicitly deferred**: [`request_clarify`] implements the documented graceful
//!    no-op fallback (mirrors `HostServices`' own deny-default behavior — "no sink" always
//!    degrades to the deny value without blocking, `host_services.rs` doc comment on
//!    `ui_roundtrip`) rather than reaching into `cyrup-session-svc` from here. The live-dialog
//!    wiring (constructing this module's [`AskLock`] with a real `confirm`/`input`-backed
//!    [`ClarifyChannel`] impl, plus the `Cargo.toml`/constructor-plumbing change) is left to
//!    whichever later phase owns `lib.rs`/`extension.rs`'s construction path and, if needed,
//!    `Cargo.toml` — see the module-level `NOTE(clarify-deferred)` marker below for the exact
//!    seam a future phase fills in.
//!
//! # What this file deliberately does NOT do (mandatory-mechanism guardrails)
//!
//! No in-process nested agent turn loop, no in-process event-relay standing in for a child
//! subprocess's own NDJSON stdout, and no extension-host session-access seam beyond the one
//! narrow exception already justified elsewhere in this crate (fork-context's direct
//! `cyrup-session` dependency, `fork_context.rs`) — this module has **zero** dependency on
//! `cyrup-agent` and does not acquire a `cyrup_session::SessionManager` handle at all. The
//! clarify primitive here is a pure, in-memory single-slot lock plus a pluggable, fallible,
//! timeout-bounded async channel trait; it does not itself talk to any subprocess, session, or
//! WASM guest — a real implementation of [`ClarifyChannel`] (later phase, per the doc above) is
//! where any such wiring would live, never inline in this file.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::background::RunId;

// =================================================================================================
// Allowlisted out-of-band payload (R-SA-123/124)
// =================================================================================================

/// The explicit, closed set of fields an out-of-band delivery attempt is permitted to carry
/// (R-SA-124). This is a **distinct type** from [`crate::background::ResultFile`]/
/// [`crate::exec::SingleResult`] — never a re-export, never `#[serde(flatten)]`, never a
/// generic "serialize the whole record" call — specifically so that the allowlist is enforced
/// **by construction/by type**, not by convention: there is no field on this struct that any
/// constructor could populate with a capability-bearing/secret value, because no such field
/// exists here at all (that is the property the "assert by construction, not just by example"
/// test requirement below is checking).
///
/// In particular, this type has **no** field for:
/// - a control-inbox route ([`crate::background::RunPaths::control_inbox`] or any other
///   filesystem path used to *drive* a run — e.g. `cwd`, `session_file` — which are both
///   plausible-looking-but-excluded fields on [`crate::background::ResultFile`] itself, kept out
///   here because a shared out-of-band transport is not necessarily as trusted as the local
///   filesystem the orchestrator process already has ambient access to);
/// - any capability token, credential, or extension-host handle of any kind (this crate has none
///   to leak today, but the allowlist shape is deliberately closed so that if a future field is
///   ever added to `ResultFile`/`SingleResult` upstream, it does NOT automatically start flowing
///   out-of-band — a maintainer must explicitly add a new field *here* and populate it in
///   [`IntercomPayload::from_result`] for it to ever leave the process this way).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntercomPayload {
    /// The run this payload summarizes.
    pub run_id: RunId,
    /// The top-level agent name (mirrors [`crate::background::ResultFile::agent`]).
    pub agent: String,
    /// Whether the run succeeded overall (mirrors [`crate::background::ResultFile::success`]).
    pub success: bool,
    /// Per-child final textual outputs, in the same fixed order as the run's steps (R-SA-051
    /// ordering preserved) — the "heavy duplicated field" R-SA-123 refers to omitting from the
    /// inline payload once this has been delivered out-of-band. Deliberately `String`, never
    /// `Option<serde_json::Value>` or any other shape that could smuggle a structured field this
    /// allowlist does not otherwise name.
    pub outputs: Vec<String>,
    /// Total additive token usage across the run (a plain numeric summary — not a capability, not
    /// a route).
    pub total_tokens: u64,
}

impl IntercomPayload {
    /// Builds an [`IntercomPayload`] by copying only the allowlisted fields out of a full
    /// [`crate::background::ResultFile`]. This is the **sole** sanctioned constructor
    /// (R-SA-124): every field assignment here is an explicit, individually-named copy, never a
    /// blanket `..` struct-update or a generic serialize/re-deserialize round trip through
    /// `ResultFile`'s own (much wider) shape — the two types are not `From`/`Into` of each other
    /// for exactly this reason, so a future field added to `ResultFile` cannot silently start
    /// flowing out-of-band via an auto-derived conversion.
    #[must_use]
    pub fn from_result(result: &crate::background::ResultFile) -> Self {
        let total_tokens: u64 = result
            .results
            .iter()
            .map(|r| r.usage.total_tokens)
            .fold(0u64, u64::saturating_add);
        Self {
            run_id: result.run_id.clone(),
            agent: result.agent.clone(),
            success: result.success,
            outputs: result
                .results
                .iter()
                .map(|r| r.final_output.clone().unwrap_or_default())
                .collect(),
            total_tokens,
        }
    }
}

// =================================================================================================
// Delivery channel + graceful-degradation race (R-SA-125)
// =================================================================================================

/// A pluggable out-of-band transport. No implementation of this trait ships in the current
/// workspace (func-SA §9 item 25 / arch-SA §12 item 7: the `pi-intercom` companion transport's
/// Rust-port status is unconfirmed) — [`deliver`] is written against this trait so that, if/when
/// such a transport exists, wiring it in is exactly "construct a `DeliveryChannel` impl and pass
/// it to `deliver`", with no change needed to the timeout/degradation contract implemented here.
///
/// Implementations MUST NOT block indefinitely; [`deliver`] applies its own bounded timeout on
/// top regardless, but a well-behaved implementation should still return promptly once it knows
/// there is no receiver (R-SA-125's "unavailable... without blocking").
pub trait DeliveryChannel: Send + Sync {
    /// Attempt one delivery of `payload`. `Ok(true)` means a receiver confirmed receipt; `Ok(false)`
    /// means the channel is reachable but no receiver confirmed (treated identically to `Err` by
    /// [`deliver`] — both degrade to [`DeliveryOutcome::NotDelivered`]); `Err` means the transport
    /// itself failed. None of the three outcomes may panic or block past what the implementation's
    /// own I/O naturally takes — [`deliver`]'s outer timeout is the actual safety net.
    fn send(&self, payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>>;
}

/// The default channel: no transport exists (func-SA §9 item 25). `send` resolves immediately to
/// `Ok(false)` — deliberately not `Err`, since "no transport configured" is not itself a failure
/// condition, it is the documented, expected steady state of this workspace today. Using this
/// channel makes R-SA-123-125 vacuously satisfied exactly as the spec anticipates: [`deliver`]
/// always reports [`DeliveryOutcome::NotDelivered`] and every caller's full result stays inline.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTransportChannel;

impl DeliveryChannel for NoTransportChannel {
    fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        Box::pin(async { Ok(false) })
    }
}

/// The default bounded wait for an out-of-band delivery attempt before giving up and degrading
/// (R-SA-125). A tuning parameter, not a normative numeric requirement (mirrors func-SA §9 item
/// 26's framing for the sibling `1000ms` debounce constant) — chosen short enough that a missing
/// receiver never perceptibly stalls the orchestrator's own turn.
pub const DEFAULT_DELIVERY_TIMEOUT: Duration = Duration::from_millis(750);

/// The result of one [`deliver`] attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// A receiver confirmed receipt within the timeout — the caller MAY now omit the heavy
    /// duplicated fields from its own inline payload (R-SA-123).
    Delivered,
    /// The channel was unavailable, reported no receiver, errored, or did not respond within the
    /// timeout — the caller's full result MUST remain in the ordinary inline tool-result payload
    /// (R-SA-125). This module intentionally does not distinguish *why* delivery failed beyond
    /// this: R-SA-125 treats "unavailable" and "timed out" identically (both fail gracefully to
    /// "not delivered"), so collapsing them into one variant avoids inventing a behavioral
    /// distinction the spec does not draw.
    NotDelivered,
}

/// Attempt one out-of-band delivery of `payload` through `channel`, racing it against a bounded
/// timeout (R-SA-125). Never blocks past `timeout`, never returns `Err` (there is nothing for a
/// caller to handle as an error — every failure mode collapses to
/// [`DeliveryOutcome::NotDelivered`]), and never panics regardless of what `channel` does.
///
/// This function does not need to (and does not) detect receiver presence up front — "no
/// receiver" and "timed out waiting for a receiver" are handled by the identical code path (the
/// `tokio::select!` below simply resolves to `NotDelivered` on either the channel returning
/// `Ok(false)`/`Err`, or the timeout branch firing first), matching R-SA-125's "without needing
/// to detect presence beforehand" framing.
pub async fn deliver(channel: &dyn DeliveryChannel, payload: IntercomPayload, timeout: Duration) -> DeliveryOutcome {
    let attempt = channel.send(payload);
    tokio::select! {
        biased;
        result = attempt => match result {
            Ok(true) => DeliveryOutcome::Delivered,
            Ok(false) | Err(_) => DeliveryOutcome::NotDelivered,
        },
        () = tokio::time::sleep(timeout) => DeliveryOutcome::NotDelivered,
    }
}

/// Convenience wrapper over [`deliver`] using [`DEFAULT_DELIVERY_TIMEOUT`].
pub async fn deliver_with_default_timeout(channel: &dyn DeliveryChannel, payload: IntercomPayload) -> DeliveryOutcome {
    deliver(channel, payload, DEFAULT_DELIVERY_TIMEOUT).await
}

/// Builds the reduced inline tool-result payload a caller SHOULD use once out-of-band delivery is
/// **confirmed** (R-SA-123): the heavy duplicated `outputs` field is dropped, leaving only the
/// identity/summary fields a reader needs to know "this ran, here's where the full detail already
/// went." Callers MUST NOT call this unless [`deliver`] returned
/// [`DeliveryOutcome::Delivered`] for the exact same payload — on any other outcome the full
/// [`IntercomPayload::outputs`] (or, more precisely, the caller's own full
/// [`crate::background::ResultFile`]/[`crate::exec::SingleResult`] payload it was projected from)
/// must remain inline in full (R-SA-125), which this function intentionally provides no way to
/// bypass: it only ever narrows an already-allowlisted payload further, never widens the
/// "delivered" decision itself.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReducedInlinePayload {
    pub run_id: RunId,
    pub agent: String,
    pub success: bool,
    pub total_tokens: u64,
}

impl From<&IntercomPayload> for ReducedInlinePayload {
    fn from(full: &IntercomPayload) -> Self {
        Self {
            run_id: full.run_id.clone(),
            agent: full.agent.clone(),
            success: full.success,
            total_tokens: full.total_tokens,
        }
    }
}

// =================================================================================================
// Clarify/ask pause primitive (R-SA-119/120)
//
// NOTE(clarify-deferred): a real `confirm`/`input`-backed `ClarifyChannel` implementation that
// forwards through a constructor-time `Arc<cyrup_session_svc::host_services::LiveHostServices>`
// handle (arch-SA §12 item 6) is deferred to whichever later phase owns this crate's
// `Cargo.toml` + `lib.rs`/`extension.rs` construction path — adding the `cyrup-session-svc`
// dependency and threading the handle through `SubagentsExtension::new` is outside this file's
// single-file ownership boundary for this task. Everything in this section is written against
// the `ClarifyChannel` trait below specifically so that wiring in the real implementation later
// is additive (one new `impl ClarifyChannel for LiveHostServicesClarify { .. }` plus passing it
// into `AskLock::new`), with no change needed to the single-slot-lock/pause semantics here.
// =================================================================================================

/// One outstanding blocking clarify/ask interaction a child run is waiting on (R-SA-119).
#[derive(Clone, Debug)]
pub struct ClarifyRequest {
    /// The run whose foreground flow is paused waiting on this clarify interaction.
    pub run_id: RunId,
    /// The step index within that run's flow that is blocked, if applicable (a parallel/chain
    /// flow pauses only its affected step, per R-SA-119, not the whole orchestrator).
    pub step_index: Option<u32>,
    /// The human-facing prompt text the child supplied for this clarify interaction.
    pub prompt: String,
}

/// The outcome a caller sees for one [`request_clarify`] attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClarifyOutcome {
    /// A response was obtained (whatever the pluggable [`ClarifyChannel`] returned).
    Answered(String),
    /// No live clarify UI is wired (the documented graceful fallback — see the module-level
    /// `NOTE(clarify-deferred)` doc above). The affected flow still visibly pauses for the
    /// duration of the attempt (R-SA-119) before this is returned; it is simply never able to
    /// resolve to an actual human answer until a real [`ClarifyChannel`] is wired in.
    NoLiveChannel,
    /// A second concurrent ask was attempted while one was already outstanding for this session
    /// (R-SA-120) and was rejected rather than silently interleaved.
    Rejected,
}

/// A pluggable clarify/ask transport (deliberately trait-based, not a concrete session handle —
/// see `NOTE(clarify-deferred)` above for why this crate does not itself implement one against
/// `cyrup-session-svc` in this file).
pub trait ClarifyChannel: Send + Sync {
    /// Present `request` to a live human/UI and await a response. Implementations should apply
    /// their own reasonable timeout; [`request_clarify`] does not impose an additional one on top
    /// (an ask is, by design, allowed to wait indefinitely for a human — R-SA-119 is about
    /// visibly pausing the flow while this is outstanding, not about bounding how long a human
    /// may take).
    fn ask(&self, request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
}

/// The documented graceful no-op fallback (see the module-level `NOTE(clarify-deferred)` doc):
/// always reports [`ClarifyOutcome::NoLiveChannel`] via an `Err`, so [`request_clarify`]'s
/// single-slot-lock bookkeeping and "visibly pause" contract are exercised even with no real UI
/// wired, exactly mirroring `LiveHostServices::ui_roundtrip`'s own "no sink -> deny default,
/// never block" behavior for the identical situation on the WASM-guest path.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoOpClarifyChannel;

impl ClarifyChannel for NoOpClarifyChannel {
    fn ask(&self, _request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async { Err("no live clarify channel wired".to_string()) })
    }
}

/// The single-slot ask lock (R-SA-120): at most one outstanding blocking clarify/ask interaction
/// is permitted per orchestrator session at a time. Keyed by an opaque session identifier (a
/// plain `String` here — this module has no dependency on `cyrup-core::SessionId` and does not
/// need one; callers pass whatever session-scoping key they already use) so one `AskLock`
/// instance can serve every session an orchestrating extension instance is handling, rather than
/// needing one lock per session wired up externally.
pub struct AskLock {
    channel: Arc<dyn ClarifyChannel>,
    /// `Some` while a session has an outstanding ask; the guard is dropped (clearing the slot)
    /// when [`request_clarify`]'s future completes, on every exit path (including cancellation),
    /// via `slots`' own `AsyncMutex` scoping — never left dangling by an early return.
    slots: AsyncMutex<HashMap<String, ()>>,
}

impl AskLock {
    /// Builds a lock backed by `channel`. Pass [`NoOpClarifyChannel::default()`] to get today's
    /// documented graceful-fallback behavior (no live UI wired); a later phase substitutes a real
    /// [`ClarifyChannel`] impl here (see `NOTE(clarify-deferred)`).
    #[must_use]
    pub fn new(channel: Arc<dyn ClarifyChannel>) -> Self {
        Self { channel, slots: AsyncMutex::new(HashMap::new()) }
    }

    /// Builds a lock using the documented no-op fallback (today's default — see
    /// `NOTE(clarify-deferred)`).
    #[must_use]
    pub fn new_with_no_live_channel() -> Self {
        Self::new(Arc::new(NoOpClarifyChannel))
    }

    /// Requests a blocking clarify/ask interaction for `session_key`, visibly pausing the
    /// affected foreground flow for the duration (R-SA-119) and enforcing the single-slot lock
    /// (R-SA-120): if `session_key` already has an outstanding ask, this returns
    /// [`ClarifyOutcome::Rejected`] immediately rather than interleaving a second one.
    ///
    /// "Visibly pause" here means: the caller (the foreground execution driver, a later phase)
    /// MUST treat any non-[`ClarifyOutcome::Rejected`] return of this function as "the affected
    /// step/run is paused until this future resolves" — this function's own contribution to that
    /// visibility is that it does not return early/optimistically; it awaits the channel's answer
    /// (or its own fallback) to completion before yielding a result, so a caller that renders
    /// "paused" for exactly the lifetime of the awaited call gets correct pause duration for
    /// free.
    pub async fn request_clarify(&self, session_key: &str, request: ClarifyRequest) -> ClarifyOutcome {
        {
            let mut slots = self.slots.lock().await;
            if slots.contains_key(session_key) {
                return ClarifyOutcome::Rejected;
            }
            slots.insert(session_key.to_string(), ());
        }

        // The slot is held for the entire await below (cleared in the `finally`-style block
        // afterward on every path, success or failure) — this is the actual enforcement of
        // R-SA-120's "one outstanding ask at a time per session," not merely a check-then-forget.
        let outcome = match self.channel.ask(request).await {
            Ok(answer) => ClarifyOutcome::Answered(answer),
            Err(_) => ClarifyOutcome::NoLiveChannel,
        };

        let mut slots = self.slots.lock().await;
        slots.remove(session_key);

        outcome
    }
}

/// A single outstanding-ask handle usable with `tokio::select!`/cancellation call sites that need
/// to observe "the ask finished" as a plain oneshot rather than awaiting [`AskLock::request_clarify`]
/// directly in-line — e.g. a foreground driver that must simultaneously keep consuming a child's
/// NDJSON stdout while a clarify request is outstanding for one of that child's steps. Spawns the
/// actual [`AskLock::request_clarify`] call onto the current runtime and forwards its result
/// through the returned receiver; dropping the receiver before it resolves does not cancel the
/// underlying ask (a human may still be mid-answer), it only stops that particular caller from
/// observing the outcome.
pub fn spawn_clarify(lock: Arc<AskLock>, session_key: String, request: ClarifyRequest) -> oneshot::Receiver<ClarifyOutcome> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let outcome = lock.request_clarify(&session_key, request).await;
        // A dropped receiver (caller no longer cares) is not an error condition here.
        let _ = tx.send(outcome);
    });
    rx
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use cyrup_core::Usage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    // ---------------------------------------------------------------------------------------
    // R-SA-124: allowlist projection never leaks a disallowed field — asserted by construction/
    // type, not merely by example.
    // ---------------------------------------------------------------------------------------

    /// This test asserts the allowlist **by type structure**: it exhaustively destructures an
    /// [`IntercomPayload`] by field name. If a future edit added a new field to the struct (e.g.
    /// accidentally re-introducing `cwd`, `session_file`, or any capability-token-shaped field),
    /// this destructuring pattern would fail to compile (missing-field / unknown-field error)
    /// rather than the test silently continuing to pass — that compile-time exhaustiveness is
    /// the "by construction, not just by example" guarantee this test exists to provide. Compare
    /// with a plain `assert_eq!(payload.run_id, ...)` style test, which would NOT fail if an
    /// unrelated extra field were added.
    #[test]
    fn intercom_payload_field_set_is_closed_and_exhaustive() {
        let payload = IntercomPayload {
            run_id: RunId::from_token("deadbeefcafef00d"),
            agent: "researcher".to_string(),
            success: true,
            outputs: vec!["done".to_string()],
            total_tokens: 42,
        };

        // Exhaustive destructure: the `let IntercomPayload { .. } = payload;` form below would
        // still compile if a field were added (the `..` pattern), so instead we name every field
        // explicitly with no `..` — this is what makes the assertion compile-time-exhaustive.
        let IntercomPayload { run_id, agent, success, outputs, total_tokens } = payload;
        assert_eq!(run_id.as_str(), "deadbeefcafef00d");
        assert_eq!(agent, "researcher");
        assert!(success);
        assert_eq!(outputs, vec!["done".to_string()]);
        assert_eq!(total_tokens, 42);
    }

    /// The allowlist is enforced by the projection function itself never having access to
    /// disallowed fields in the first place: build a [`crate::background::ResultFile`] whose
    /// `cwd`/`session_file` are populated with values that would be an obvious, embarrassing leak
    /// if they ever ended up in the wire payload (a path containing a fake "secret"-looking
    /// token), project it through [`IntercomPayload::from_result`], and assert neither of those
    /// values appears ANYWHERE in the projected payload's serialized form. This is a
    /// belt-and-suspenders behavioral check on top of the type-level exhaustiveness test above.
    #[test]
    fn from_result_never_copies_cwd_or_session_file_into_the_wire_payload() {
        let secret_cwd = PathBuf::from("/Users/nobody/.ssh/CAPABILITY_TOKEN_LEAK_CANARY");
        let secret_session = PathBuf::from("/var/run/CONTROL_INBOX_ROUTE_LEAK_CANARY.json");

        let result = crate::background::ResultFile {
            id: RunId::from_token("run00000000000001"),
            run_id: RunId::from_token("run00000000000001"),
            agent: "delegate".to_string(),
            mode: crate::background::RunMode::Single,
            state: crate::background::RunState::Complete,
            success: true,
            cwd: secret_cwd.clone(),
            session_file: Some(secret_session.clone()),
            results: vec![sample_single_result("delegate", "did the thing")],
        };

        let payload = IntercomPayload::from_result(&result);
        let wire = serde_json::to_string(&payload).expect("serializes");

        assert!(
            !wire.contains("CAPABILITY_TOKEN_LEAK_CANARY"),
            "cwd must never appear in the out-of-band payload: {wire}"
        );
        assert!(
            !wire.contains("CONTROL_INBOX_ROUTE_LEAK_CANARY"),
            "session_file must never appear in the out-of-band payload: {wire}"
        );
        assert!(wire.contains("did the thing"), "the allowlisted output must still be present");
    }

    #[test]
    fn from_result_sums_token_usage_across_all_steps() {
        let mut a = sample_single_result("scout", "found stuff");
        a.usage = Usage { total_tokens: 15, ..Usage::default() };
        let mut b = sample_single_result("worker", "did stuff");
        b.usage = Usage { total_tokens: 150, ..Usage::default() };

        let result = crate::background::ResultFile {
            id: RunId::from_token("run00000000000002"),
            run_id: RunId::from_token("run00000000000002"),
            agent: "orchestrator".to_string(),
            mode: crate::background::RunMode::Chain,
            state: crate::background::RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp/irrelevant-here"),
            session_file: None,
            results: vec![a, b],
        };

        let payload = IntercomPayload::from_result(&result);
        assert_eq!(payload.total_tokens, 15 + 150);
        assert_eq!(payload.outputs, vec!["found stuff".to_string(), "did stuff".to_string()]);
    }

    fn sample_single_result(agent: &str, output: &str) -> crate::exec::SingleResult {
        crate::exec::SingleResult {
            agent: agent.to_string(),
            task: "task".to_string(),
            exit_code: 0,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: Some(output.to_string()),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            error: None,
            tool_calls: Vec::new(),
            output_truncated: false,
        }
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-125: bounded-timeout degrade-to-undelivered path with no receiver present.
    // ---------------------------------------------------------------------------------------

    /// A channel that never resolves — the real-world shape of "no receiver present" (nobody
    /// ever answers) as opposed to "explicitly declines" ([`NoTransportChannel`]). Used to prove
    /// [`deliver`]'s timeout branch actually fires and bounds the wait, rather than [`deliver`]
    /// merely happening to return quickly because the default channel resolves instantly.
    struct HangingChannel;

    impl DeliveryChannel for HangingChannel {
        fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_degrades_to_not_delivered_when_no_receiver_ever_responds() {
        let payload = sample_payload();
        let timeout = Duration::from_millis(80);

        let started = Instant::now();
        let outcome = deliver(&HangingChannel, payload, timeout).await;
        let elapsed = started.elapsed();

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        assert!(
            elapsed >= timeout,
            "must actually wait out the bound, not return early: {elapsed:?} < {timeout:?}"
        );
        assert!(
            elapsed < timeout * 5,
            "must not block far past the bound (never block the caller's turn): {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_degrades_to_not_delivered_when_channel_reports_no_receiver() {
        // NoTransportChannel::send resolves immediately to Ok(false) — the "channel reachable,
        // explicitly no receiver" case, which must ALSO degrade gracefully without needing the
        // timeout branch to fire at all (R-SA-125's "without needing to detect presence
        // beforehand": both this case and the hanging case above land on the identical outcome).
        let payload = sample_payload();
        let started = Instant::now();

        let outcome = deliver(&NoTransportChannel, payload, Duration::from_secs(5)).await;

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "an explicit 'no receiver' answer must resolve promptly, not wait out the full bound"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_degrades_to_not_delivered_on_channel_error() {
        struct ErroringChannel;
        impl DeliveryChannel for ErroringChannel {
            fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
                Box::pin(async { Err("transport exploded".to_string()) })
            }
        }

        let outcome = deliver(&ErroringChannel, sample_payload(), Duration::from_secs(5)).await;
        assert_eq!(outcome, DeliveryOutcome::NotDelivered, "an error must never propagate to the caller as Err");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_reports_delivered_when_a_receiver_confirms_promptly() {
        struct ConfirmingChannel;
        impl DeliveryChannel for ConfirmingChannel {
            fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
                Box::pin(async { Ok(true) })
            }
        }

        let outcome = deliver(&ConfirmingChannel, sample_payload(), Duration::from_secs(5)).await;
        assert_eq!(outcome, DeliveryOutcome::Delivered);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_with_default_timeout_uses_the_documented_constant() {
        let started = Instant::now();
        let outcome = deliver_with_default_timeout(&HangingChannel, sample_payload()).await;
        let elapsed = started.elapsed();

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        assert!(elapsed >= DEFAULT_DELIVERY_TIMEOUT);
        assert!(elapsed < DEFAULT_DELIVERY_TIMEOUT * 5);
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-125 (second half): full-output preservation in the local result when undelivered.
    // ---------------------------------------------------------------------------------------

    /// The defining behavioral contract of "graceful degradation": whatever the caller already
    /// held locally (the full [`IntercomPayload`], standing in here for the caller's own full
    /// inline tool-result payload) is completely untouched by an undelivered attempt — `deliver`
    /// takes the payload by value only to hand it to the channel; nothing about a `NotDelivered`
    /// outcome mutates or discards any field the caller still has. This test rebuilds the exact
    /// value passed in and confirms every field survives identically after a failed delivery.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn undelivered_attempt_leaves_the_original_payload_fully_reconstructable_by_the_caller() {
        let payload = sample_payload();
        let payload_for_local_use = payload.clone();

        let outcome = deliver(&HangingChannel, payload, Duration::from_millis(50)).await;

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        // The caller's own retained copy (what an exec/background call site would put in the
        // ordinary inline tool-result payload per R-SA-125) is exactly the full payload, field
        // for field — nothing was truncated, stripped, or replaced by the failed attempt.
        assert_eq!(payload_for_local_use.outputs.len(), 2);
        assert_eq!(payload_for_local_use.outputs[0], "first step output, in full");
        assert_eq!(payload_for_local_use.outputs[1], "second step output, in full, unabridged");
        assert_eq!(payload_for_local_use.total_tokens, 999);
        assert_eq!(payload_for_local_use.agent, "orchestrator");
        assert!(payload_for_local_use.success);
    }

    /// [`ReducedInlinePayload`] must only ever be constructed by a caller that already confirmed
    /// [`DeliveryOutcome::Delivered`] — this test documents (and pins) that the reduction itself
    /// is a pure, always-available projection (it does not gate on delivery status internally;
    /// gating is the caller's job per the type's own doc comment), while separately proving the
    /// FULL payload remains available and unaffected regardless of whether a caller chooses to
    /// build a reduced view from it.
    #[test]
    fn reduced_inline_payload_drops_only_the_heavy_outputs_field() {
        let full = sample_payload();
        let reduced = ReducedInlinePayload::from(&full);

        assert_eq!(reduced.run_id, full.run_id);
        assert_eq!(reduced.agent, full.agent);
        assert_eq!(reduced.success, full.success);
        assert_eq!(reduced.total_tokens, full.total_tokens);
        // The full payload itself is untouched by having built a reduced view from a reference.
        assert_eq!(full.outputs.len(), 2);
    }

    fn sample_payload() -> IntercomPayload {
        IntercomPayload {
            run_id: RunId::from_token("sample0000000001"),
            agent: "orchestrator".to_string(),
            success: true,
            outputs: vec![
                "first step output, in full".to_string(),
                "second step output, in full, unabridged".to_string(),
            ],
            total_tokens: 999,
        }
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-119/120: clarify/ask single-slot lock + graceful no-live-channel fallback.
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_clarify_with_no_live_channel_degrades_gracefully_without_panicking() {
        let lock = AskLock::new_with_no_live_channel();
        let outcome = lock
            .request_clarify(
                "session-a",
                ClarifyRequest { run_id: RunId::from_token("run0000000000000a"), step_index: Some(2), prompt: "ok?".to_string() },
            )
            .await;
        assert_eq!(outcome, ClarifyOutcome::NoLiveChannel);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_clarify_frees_the_slot_after_completion_so_a_later_ask_succeeds() {
        let lock = AskLock::new_with_no_live_channel();
        let req = |n: u32| ClarifyRequest { run_id: RunId::from_token("run0000000000000b"), step_index: Some(n), prompt: "ok?".to_string() };

        let first = lock.request_clarify("session-b", req(1)).await;
        assert_eq!(first, ClarifyOutcome::NoLiveChannel);

        // The slot must have been released after the first request completed — a second,
        // sequential ask for the SAME session must not be rejected.
        let second = lock.request_clarify("session-b", req(2)).await;
        assert_eq!(second, ClarifyOutcome::NoLiveChannel, "must not be Rejected once the prior ask completed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn request_clarify_rejects_a_second_concurrent_ask_for_the_same_session() {
        // A channel whose `ask` blocks until manually released, so we can deterministically
        // observe TWO concurrent in-flight asks for the same session.
        struct GateChannel {
            gate: tokio::sync::Notify,
            entered: AtomicUsize,
        }
        impl ClarifyChannel for GateChannel {
            fn ask(&self, _request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async move {
                    self.entered.fetch_add(1, Ordering::SeqCst);
                    self.gate.notified().await;
                    Ok("answered".to_string())
                })
            }
        }

        let channel = Arc::new(GateChannel { gate: tokio::sync::Notify::new(), entered: AtomicUsize::new(0) });
        let lock = Arc::new(AskLock::new(channel.clone()));

        let req = |n: u32| ClarifyRequest { run_id: RunId::from_token("run0000000000000c"), step_index: Some(n), prompt: "ok?".to_string() };

        let lock_a = lock.clone();
        let first = tokio::spawn(async move { lock_a.request_clarify("session-c", req(1)).await });

        // Wait until the first ask is genuinely in-flight (inside the channel, holding the slot)
        // before firing the second — avoids a race where the second could win the lock first.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while channel.entered.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(channel.entered.load(Ordering::SeqCst), 1, "first ask must have entered the channel");

        let second = lock.request_clarify("session-c", req(2)).await;
        assert_eq!(second, ClarifyOutcome::Rejected, "R-SA-120: a second concurrent ask must be rejected");

        // Release the first ask and confirm it completes normally (the lock's own bookkeeping
        // is unaffected by the rejected second attempt).
        channel.gate.notify_one();
        let first_outcome = first.await.expect("task join");
        assert_eq!(first_outcome, ClarifyOutcome::Answered("answered".to_string()));

        // Only one ask ever actually entered the channel — the rejected attempt never called
        // `ask` at all.
        assert_eq!(channel.entered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn request_clarify_permits_concurrent_asks_for_different_sessions() {
        let lock = Arc::new(AskLock::new_with_no_live_channel());
        let req = |n: u32| ClarifyRequest { run_id: RunId::from_token("run0000000000000d"), step_index: Some(n), prompt: "ok?".to_string() };

        let lock_a = lock.clone();
        let a = tokio::spawn(async move { lock_a.request_clarify("session-d1", req(1)).await });
        let lock_b = lock.clone();
        let b = tokio::spawn(async move { lock_b.request_clarify("session-d2", req(2)).await });

        let (a_outcome, b_outcome) = tokio::join!(a, b);
        assert_eq!(a_outcome.expect("join"), ClarifyOutcome::NoLiveChannel);
        assert_eq!(b_outcome.expect("join"), ClarifyOutcome::NoLiveChannel);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_clarify_delivers_the_outcome_through_the_returned_receiver() {
        let lock = Arc::new(AskLock::new_with_no_live_channel());
        let rx = spawn_clarify(
            lock,
            "session-e".to_string(),
            ClarifyRequest { run_id: RunId::from_token("run0000000000000e"), step_index: None, prompt: "ok?".to_string() },
        );
        let outcome = rx.await.expect("the spawned ask completes and sends its outcome");
        assert_eq!(outcome, ClarifyOutcome::NoLiveChannel);
    }
}
