//! The ACP turn: the sole owner of a `session/prompt`'s responder, and the actor that drives it.
//!
//! **Owner: agent A (`ACP-153`, `ACP-121`, `ACP-154`, `ACP-155`, `ACP-057`, `ACP-061`, `ACP-120`,
//! `ACP-124`, `ACP-142`, `ACP-143`).**
//!
//! ADR-0028 finding F1. Port of pi-acp v0.0.33 `src/acp/session.ts`'s `PiAcpSession` fields
//! `pendingTurn`, `turnQueue`, `cancelRequested` and `inAgentLoop`, its `prompt` / `cancel` /
//! `startTurn` / `wasCancelRequested`, the `agent_settled`, `auto_retry_start`, `auto_retry_end`,
//! `auto_compaction_start` and `auto_compaction_end` arms of `handlePiEvent`, its
//! `formatAutoRetryMessage`, plus `agent.ts`'s `prompt`, which computes `stopReason` as
//! `result === 'error' ? (session.wasCancelRequested() ? 'cancelled' : 'end_turn') : result`.
//!
//! # `ACP-160` — `inAgentLoop` is not ported, and the deliverable is its absence
//!
//! Upstream's `inAgentLoop` is **assigned at five sites and read at none**. There is no field for
//! it here; its comment survives as the doc on [`Turn::settle`], which is the transition it was
//! trying to describe.
//!
//! # Why an enum and not typestate — ADR-0028 §5, restated because it will be re-proposed
//!
//! `Turn<Idle>` / `Turn<Running>` is strictly worse here: the turn is stored in the per-session
//! actor's state and mutated by events arriving on a channel, so the state is only knowable at
//! runtime, and a generic marker would force the actor's own type to change on every transition.
//! The guarantee typestate would buy — "the responder is consumed exactly once" — is obtained by
//! [`Turn::settle`] moving the responders out of `Running`, leaving `Idle`.
//!
//! # The three structural rules this module exists to hold
//!
//! 1. **`ACP-153` — the stream is run-scoped.** [`TurnAgent::start_run`] is `AgentSession::prompt`,
//!    which registers `Fanout::subscribe_run()` **before** dispatching and whose `Fanout::end_run`
//!    clears the run-scoped senders immediately after `emit_agent_settled` — *"so a run-scoped
//!    consumer observes the settle as its last event."* Session-wide `subscribe()` is never used
//!    and must not be introduced: it re-creates the correlation problem pi-acp had to invent
//!    `pendingTurn` for, in a codebase that had solved it.
//! 2. **`ACP-121` — a prompt resolves only on `AgentSettled`.** [`TurnActor::on_event`] settles on
//!    exactly one signal, [`crate::translate::TurnSignal::Settled`]. `TurnEnd` and `AgentEnd` are
//!    consumed and dropped.
//! 3. **`ACP-122`/`ACP-155` — one task owns the pump, the notifications and the responder.**
//!    [`TurnActor`] drains the run-scoped stream, writes every `session/update`, and answers the
//!    `Responder` as the **last** statement after the final notification. It never awaits a client
//!    round trip: [`TurnSink::notify`] is a plain `fn`, so "await the dialog inline" does not
//!    typecheck. See [`TurnSink`].

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, Meta, PromptResponse, SessionId, SessionInfoUpdate,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{BoxFuture, Client, ConnectionTo, Responder};
use cyrup_core::EventStream;
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, CompactionReason, PromptAccepted,
    PromptOptions, SessionServiceError, StreamingBehavior, UserInput,
};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::error::AcpFailure;
use crate::ids::AbsCwd;
use crate::ledger::{FileSnapshot, ToolCallLedger};
use crate::sessions::SessionManager;
use crate::translate::{RunTermination, Translated, TurnSignal, snapshot_needed, translate};

// ---------------------------------------------------------------------------------------------
// The reply channel
// ---------------------------------------------------------------------------------------------

/// The `session/prompt` reply channel a [`Turn`] owns.
///
/// **The only production implementor is `Responder<PromptResponse>`**, which is the type ADR-0028
/// F1 names; the trait exists for one reason and it is worth stating plainly.
///
/// # [CYRUP-DELTA] — a one-method trait where the ADR sketch writes a concrete type
///
/// **What differs.** ADR-0028 F1 writes `responders: Vec<Responder<PromptResponse>>`. Here the
/// field is `Vec<R>` with `R: PromptReply` defaulting to exactly that type, so every published
/// signature reads the same at its default instantiation.
///
/// **What it costs, and what it buys.** `Responder::new` is **private** in
/// `agent-client-protocol` 2.1: a responder can only be minted by the connection's own dispatch
/// loop, so with the concrete type there is no way to construct a `Turn::Running` in a unit test
/// and *every* assertion about `ACP-121`'s exactly-once settle would have to be an integration
/// test behind `cyrup-it`'s default-off `it` feature. The cost is one type parameter with a
/// default; the gain is that the correctness core of the crate is testable at the layer the unit's
/// *Verify* line names. The guarantee F1 asks for is untouched — [`PromptReply::deliver`] takes
/// `self` by value, so a reply is consumed exactly once whatever `R` is.
pub trait PromptReply: Send + 'static {
    /// Answer the request. Consumes the channel, so a second answer does not typecheck.
    ///
    /// The transport's own `Err` is swallowed by every implementor: a client that has gone away
    /// must not stop the turn from completing, and a propagated `Err` out of the spawned task
    /// tears down the whole connection (`ACP-057`, `ACP-122`).
    fn deliver(self, result: Result<PromptResponse, agent_client_protocol::Error>);
}

/// Port of pi-acp v0.0.33 `session.ts`'s `PendingTurn { resolve, reject }` pair @v0.0.33 — one
/// channel that carries both, because ACP's `Responder` already is that union.
impl PromptReply for Responder<PromptResponse> {
    fn deliver(self, result: Result<PromptResponse, agent_client_protocol::Error>) {
        // `respond`/`respond_with_error` return `Err` only for a dead transport, which the
        // connection loop is already tearing down. `ACP-122`: swallowed, never `?`.
        let _ = match result {
            Ok(response) => self.respond(response),
            Err(error) => self.respond_with_error(error),
        };
    }
}

// ---------------------------------------------------------------------------------------------
// The turn value (ADR-0028 F1)
// ---------------------------------------------------------------------------------------------

/// Whether a cancel has been requested for the running turn. `Copy`, so it can be read without
/// borrowing the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelState {
    /// No `session/cancel` has arrived for this turn.
    None,
    /// A `session/cancel` arrived. The settle will report [`StopReason::Cancelled`].
    Requested,
}

/// How a `session/prompt` was admitted to the turn. The return of [`Turn::start`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// This submission started the run. The turn went `Idle -> Running`.
    Started,
    /// This submission was folded into a run that was already in flight (`ACP-124`).
    Folded {
        /// 1-based position behind the running submission — upstream's `N` in
        /// `Queued message (position N).`, which is `turnQueue.length` *after* the push.
        position: usize,
    },
}

/// Why an ACP turn ended. **The only producer of an ACP [`StopReason`].**
///
/// pi-acp derives the stop reason in `agent.ts`, *after* awaiting the turn promise, by asking the
/// session whether cancel was requested — by which time `startTurn` may already have cleared the
/// flag for a queued successor. That pattern has no expressible form here: the reason is computed
/// inside [`Turn::settle`] from the state of *that* turn or not at all.
#[derive(Debug)]
pub enum TurnOutcome {
    /// `AgentSessionEvent::AgentSettled`. `ACP-121` — **not** `AgentEnd`: a turn that auto-retries
    /// emits two `AgentEnd`s and one `AgentSettled`, and settling on the former returns
    /// `stopReason: end_turn` to the editor while the retried run is still streaming, so the client
    /// closes the turn and renders the rest of the real answer as orphan chunks outside it. The
    /// user reads a truncated answer as complete and nothing reports an error.
    Settled,
    /// `AgentSessionEvent::AgentSettled`, but the run's terminal `AssistantMessage` reported
    /// `StopReason::Error` (`ACP-022`).
    ///
    /// This is the variant whose absence made every provider and runtime failure reach the editor
    /// as `stopReason: "end_turn"` with no error, no content and no `auth_required` — a
    /// successful, empty turn. The failure is *not* an `Err` from `AgentSession::prompt`:
    /// `ProviderError::into_error_message` (`crates/cyrup-provider/src/error.rs`) flattens request
    /// and stream failures into an `AssistantMessage` rather than throwing, so the prompt call
    /// succeeds on a provider 401 and the terminal message is the only witness.
    /// [`crate::translate::RunTermination`] reads it, [`AcpFailure::classify_terminal`] classifies
    /// it, and this variant carries the verdict to [`Turn::settle`].
    Failed(AcpFailure),
    /// `AgentSessionEvent::AgentSettled` with a terminal `StopReason::Length` — the provider
    /// stopped at its token ceiling (`ACP-022`'s sibling). ACP has `StopReason::MaxTokens` for
    /// exactly this, and `end_turn` would tell the editor a truncated answer is complete.
    MaxTokens,
    /// A `session/cancel` was honoured.
    Cancelled,
    /// **The run-scoped stream ended without a settle** (`ACP-154`) — the third termination pi-acp
    /// does not have.
    ///
    /// The named cause is `AgentSessionEvent::SessionReplaced`: `Fanout::invalidate`
    /// (`crates/cyrup-session-svc/src/subscriber.rs`) emits it and then clears **both** the
    /// run-scoped and the persistent senders, so `AgentSettled` never arrives. The unnamed cause is
    /// a bare stream termination — another run's `Fanout::end_run` clearing the run-scoped senders
    /// wholesale while this turn's subscription is registered. Both are the same fact for the
    /// pending request, which is why they are one variant: the run this responder is waiting on
    /// will never settle, so it must be answered now or it hangs forever with no timeout on the ACP
    /// side.
    Replaced,
    /// A refusal the client sees as an error response rather than a stop reason (`ACP-126`).
    ///
    /// Produced by [`TurnHandle::fail`], which is how a host that tears a session down abnormally
    /// discharges the in-flight responder instead of dropping it.
    Refused(AcpFailure),
}

/// What the shell must do once a turn has settled.
///
/// `#[must_use]` so a settle that is computed and then dropped is a warning, not a silent hang.
/// Rust cannot force delivery — that is stated in ADR-0028 F1 as a guarantee **not** gained, and it
/// is why `ACP-121`'s end-to-end assertion stays. [`SettleAction::deliver`] is the one consumer.
#[must_use]
pub struct SettleAction<R = Responder<PromptResponse>> {
    /// Every responder folded into this run. See [`RunningTurn::responders`].
    pub responders: Vec<R>,
    /// What each of them is answered with.
    pub result: Result<PromptResponse, agent_client_protocol::Error>,
}

impl<R: PromptReply> SettleAction<R> {
    /// Answer every folded request.
    ///
    /// **`ACP-122`: this must be the last statement after the final `send_notification`.** It is
    /// not enforceable in a signature — ADR-0028 F1 says so — so it is enforced by there being
    /// exactly one caller, [`TurnActor::finish`], whose body is ordered that way.
    pub fn deliver(self) {
        let SettleAction { responders, result } = self;
        for responder in responders {
            responder.deliver(result.clone());
        }
    }

    /// How many requests this settle answers. `ACP-124`'s "N settle together" made observable.
    #[must_use]
    pub fn breadth(&self) -> usize {
        self.responders.len()
    }
}

/// The running half of [`Turn`]. All fields private: the invariant is the combination.
pub struct RunningTurn<R = Responder<PromptResponse>> {
    /// The `session/prompt` request(s) folded into this run.
    ///
    /// **ADR-0028 F1's open question, decided: hold them all and settle them together.** pi-acp
    /// holds queued prompts in its own `turnQueue` and answers each separately; cyrup instead
    /// queues into the session's steer/follow-up queues
    /// (`AgentSession::steer`/`follow_up`, `crates/cyrup-session-svc/src/session/queue.rs`) and
    /// emits **one** `AgentSettled` for the whole loop, so there is no per-submission settle to
    /// respond to. Inventing one would mean fabricating a stop reason for a run that had not
    /// stopped.
    ///
    /// **What it costs (`ACP-124`).** N follow-ups drain inside one run and one `AgentSettled`, so
    /// N `session/prompt` requests settle **together** rather than serially, and one
    /// `session/cancel` cancels all of them. A client that renders each prompt's completion
    /// independently sees them all complete at once. Do **not** port `turnQueue` to paper over
    /// this: cyrup's queues plus `AgentSessionEvent::QueueUpdate` already carry the depth pi-acp
    /// publishes by hand in `_meta.piAcp.queueDepth`.
    responders: Vec<R>,
    cancel: CancelState,
}

impl<R> RunningTurn<R> {
    /// How many `session/prompt` requests will settle together.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.responders.len()
    }

    /// Whether a cancel has been requested for this run.
    #[must_use]
    pub fn cancel_state(&self) -> CancelState {
        self.cancel
    }
}

/// The ACP turn for one run, and the sole owner of its responder(s). ADR-0028 F1.
pub enum Turn<R = Responder<PromptResponse>> {
    /// No run is in flight.
    Idle,
    /// A run is in flight and owns at least one responder.
    Running(RunningTurn<R>),
}

// `#[derive(Default)]` with `#[default]` on `Idle` would add an `R: Default` bound, which is
// wrong: a reply channel has no default and `Turn::<Responder<PromptResponse>>::default()` must
// still work. Written out rather than derived for that reason.
#[allow(clippy::derivable_impls)]
impl<R> Default for Turn<R> {
    fn default() -> Self {
        Turn::Idle
    }
}

impl<R> Turn<R> {
    /// Adopt the reply channel for an accepted prompt.
    ///
    /// `accepted` is the session's own answer — `AgentSession::prompt_with`'s `PromptAccepted` —
    /// so the turn cannot disagree with the agent about whether a run started. ADR-0028 F1's
    /// sketch takes exactly this pair.
    ///
    /// `Err` hands the reply channel **back** so the caller must decide what to answer; it cannot
    /// be silently dropped. That is the whole point: an early `return Err(..)` path that drops an
    /// `Option<Responder<_>>` without taking it leaves the editor's `session/prompt` permanently
    /// unanswered, and there is no timeout on the ACP side.
    ///
    /// # Errors
    ///
    /// Returns the reply channel unchanged when the acceptance and the turn state disagree:
    /// `Started` with a run already in flight (two runs, which `AgentSession::prompt` refuses
    /// structurally), `Queued` with nothing running, or `Handled` — which started nothing and must
    /// be answered directly rather than parked on a settle that will never come. See
    /// [`TurnActor::admit`], which is the one caller and handles all three.
    pub fn start(&mut self, reply: R, accepted: PromptAccepted) -> Result<Admission, R> {
        match (&mut *self, accepted) {
            (Turn::Idle, PromptAccepted::Started) => {
                *self = Turn::Running(RunningTurn {
                    responders: vec![reply],
                    cancel: CancelState::None,
                });
                Ok(Admission::Started)
            }
            (Turn::Running(running), PromptAccepted::Queued(_)) => {
                running.responders.push(reply);
                // Upstream's N is `turnQueue.length` AFTER the push, i.e. the count of submissions
                // waiting behind the one that is running. Here the running submission is
                // `responders[0]`, so that count is `len() - 1`. The subtraction cannot underflow:
                // this arm is only reachable from `Turn::Running`, which always holds at least one.
                let position = running.responders.len().saturating_sub(1);
                Ok(Admission::Folded { position })
            }
            _ => Err(reply),
        }
    }

    /// Request cancellation. Idempotent; returns `false` when nothing is running.
    ///
    /// `ACP-123`. The `false` return is not a failure — a `session/cancel` for an idle session is
    /// legal and answers nothing, upstream included.
    pub fn request_cancel(&mut self) -> bool {
        match self {
            Turn::Idle => false,
            Turn::Running(running) => {
                running.cancel = CancelState::Requested;
                true
            }
        }
    }

    /// Settle once.
    ///
    /// A second call returns `None`: a late `AgentSettled` from a replaced session is a **no-op**,
    /// not a double-respond and not a panic. The responders are moved out of `Running` here and
    /// nowhere else, leaving [`Turn::Idle`], which is what makes double-respond unrepresentable.
    ///
    /// This is also where upstream's dead `inAgentLoop` field's comment belongs: this transition is
    /// the only thing it was ever trying to describe.
    ///
    /// # The stop-reason mapping, and the one place it diverges from the skeleton
    ///
    /// | outcome | cancel requested | `stopReason` |
    /// |---|---|---|
    /// | `Settled` | no | `end_turn` |
    /// | `Settled` | yes | `cancelled` |
    /// | `MaxTokens` | no | `max_tokens` |
    /// | `MaxTokens` | yes | `cancelled` |
    /// | `Failed(f)` | no | *(an error response, no stop reason)* |
    /// | `Failed(_)` | yes | `cancelled` |
    /// | `Cancelled` | either | `cancelled` |
    /// | `Replaced` | either | `cancelled` |
    /// | `Refused(f)` | either | *(an error response, no stop reason)* |
    ///
    /// **A cancel outranks a failure.** The schema is explicit that `Cancelled` MUST be returned
    /// when the client sent `session/cancel`, "even if the cancellation causes exceptions in
    /// underlying operations" — and a cancel *is* the usual cause of the abort the provider then
    /// reports. Answering an error there would make a successful stop read as a fault.
    ///
    /// **[CYRUP-DELTA] a failed run is an error response, where upstream answers `end_turn`.**
    /// *What differs*: pi-acp's own `agent.ts` comment says it —
    /// `// ACP StopReason does not include "error"; if pi fails we map to end_turn for now` — so a
    /// non-auth provider failure resolves the request successfully there. *What it costs*: a
    /// client that renders a `session/prompt` error as a fatal connection problem now shows one
    /// where upstream showed nothing at all. That is the right trade: the alternative is the
    /// failure this whole variant exists to remove, where the editor renders a complete, empty,
    /// successful turn and the only record that anything went wrong is in the JSONL. The auth half
    /// is not a divergence — upstream rejects with `AUTH_REQUIRED` on the same path.
    ///
    /// **[CYRUP-DELTA] `Replaced` reports `cancelled`, not `end_turn`.** *What differs*: upstream
    /// has no third termination at all, and the obvious Rust port folds `Replaced` into the
    /// `end_turn` arm because nothing was explicitly cancelled. *What it costs, and why it is
    /// wrong*: a replaced session was torn down by `AgentSessionRuntime::install_inner`, which
    /// awaits `abort_and_settle()` on the outgoing session — the run **was** aborted mid-flight, so
    /// `end_turn` tells the editor a truncated answer completed successfully. That is exactly
    /// `ACP-121`'s silent-wrong-output failure reached by `ACP-154`'s route. The cost of
    /// `cancelled` is that a client distinguishing "user cancelled" from "the agent was replaced
    /// under you" cannot; the schema has no third value (`StopReason` is `EndTurn`, `MaxTokens`,
    /// `MaxTurnRequests`, `Refusal`, `Cancelled`) and `Refusal` means something else entirely.
    pub fn settle(&mut self, outcome: TurnOutcome) -> Option<SettleAction<R>> {
        let Turn::Running(running) = std::mem::replace(self, Turn::Idle) else {
            return None;
        };
        let cancelled = running.cancel == CancelState::Requested;
        let result = match outcome {
            // `Refused` is the HOST refusing the turn, not the run reporting on itself
            // ([`TurnHandle::fail`]), so it is answered before the cancel rule: a client that
            // cancelled still needs to be told the session was torn down under it.
            TurnOutcome::Refused(failure) => Err(failure.into()),
            // A cancel outranks everything the RUN reports — see the table above. Placed before
            // the three run outcomes so none of them can be reached with `cancelled` set and
            // quietly answer something else.
            TurnOutcome::Settled | TurnOutcome::MaxTokens | TurnOutcome::Failed(_) if cancelled => {
                Ok(PromptResponse::new(StopReason::Cancelled))
            }
            TurnOutcome::Failed(failure) => Err(failure.into()),
            TurnOutcome::Cancelled | TurnOutcome::Replaced => {
                Ok(PromptResponse::new(StopReason::Cancelled))
            }
            TurnOutcome::MaxTokens => Ok(PromptResponse::new(StopReason::MaxTokens)),
            TurnOutcome::Settled => Ok(PromptResponse::new(StopReason::EndTurn)),
        };
        Some(SettleAction {
            responders: running.responders,
            result,
        })
    }

    /// Whether a run is in flight.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self, Turn::Running(_))
    }

    /// How many requests would settle together right now. `0` when idle.
    #[must_use]
    pub fn depth(&self) -> usize {
        match self {
            Turn::Idle => 0,
            Turn::Running(running) => running.depth(),
        }
    }

    /// The running half, for a caller that wants [`RunningTurn::cancel_state`].
    #[must_use]
    pub fn running(&self) -> Option<&RunningTurn<R>> {
        match self {
            Turn::Idle => None,
            Turn::Running(running) => Some(running),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ACP-142 / ACP-143 / ACP-124 — the status chunks
// ---------------------------------------------------------------------------------------------

/// Port of pi-acp v0.0.33 `session.ts`'s `auto_retry_end` arm @v0.0.33. Byte-for-byte.
pub const RETRY_FINISHED_MESSAGE: &str = "Retry finished, resuming.";

/// Port of pi-acp v0.0.33 `session.ts`'s `auto_compaction_start` arm @v0.0.33. Byte-for-byte.
pub const AUTO_COMPACTION_START_MESSAGE: &str =
    "Context nearing limit, running automatic compaction...";

/// Port of pi-acp v0.0.33 `session.ts`'s `auto_compaction_end` arm @v0.0.33. Byte-for-byte.
pub const AUTO_COMPACTION_END_MESSAGE: &str =
    "Automatic compaction finished; context was summarized to continue the session.";

/// The `_meta` namespace for cyrup's own session-info payload (`ACP-124`).
///
/// **[CYRUP-DELTA]** upstream publishes `_meta.piAcp`; putting another product's name in a cyrup
/// user's transcript is product copy, not protocol, so the namespace is renamed. The *shape*
/// (`queueDepth`, `running`) is upstream's and is kept so a client written against pi-acp needs one
/// key renamed rather than a new reader.
pub const META_NAMESPACE: &str = "cyrupAcp";

/// Port of pi-acp v0.0.33 `session.ts`'s `formatAutoRetryMessage` @v0.0.33.
///
/// `Retrying (attempt {attempt}/{max_attempts}, waiting {n}s)...`, where `n` is `delay_ms` rounded
/// to the nearest second and then bumped to `1` when a non-zero delay rounded to zero — upstream's
/// `if (delayMs > 0 && delaySeconds === 0) delaySeconds = 1`.
///
/// # What the cut removed, and what it did not
///
/// Upstream's three `Number.isFinite` guards and the `'Retrying...'` fallback they protect have no
/// input here: `AgentSessionEvent::AutoRetryStart { attempt: u32, max_attempts: u32, delay_ms: u64,
/// … }` has no optional or stringly-typed field, so the fallback is unreachable and porting it —
/// with `test/component/session-events.test.ts`'s case for it — would be a test that passes by
/// construction. §3's cut list says so explicitly. The sub-second bump is **not** part of that cut
/// and is ported.
///
/// The rounding is integer (`(delay_ms + 500) / 1000`) rather than `f64::round`, which agrees with
/// `Math.round` for every non-negative input and cannot lose precision on a large `u64`.
///
/// `error_message` is deliberately **not** read: upstream's chunk contains only this sentence, and
/// a provider error pasted into the transcript mid-retry is noise the user cannot act on.
#[must_use]
pub fn format_auto_retry_message(attempt: u32, max_attempts: u32, delay_ms: u64) -> String {
    let mut delay_seconds = (delay_ms + 500) / 1000;
    if delay_ms > 0 && delay_seconds == 0 {
        delay_seconds = 1;
    }
    format!("Retrying (attempt {attempt}/{max_attempts}, waiting {delay_seconds}s)...")
}

/// Upstream's `Queued message (position N).` — byte-for-byte from pi-acp v0.0.33 `session.ts`'s
/// `prompt` @v0.0.33 (`ACP-124`).
#[must_use]
pub fn queued_message_text(position: usize) -> String {
    format!("Queued message (position {position}).")
}

/// An `agent_message_chunk` carrying `text`. The only chunk constructor in this module.
///
/// `ContentChunk.message_id` is left `None`: the TS SDK had no such field and upstream emits none,
/// so `ACP-127`'s parity note applies here too.
fn text_chunk(text: impl Into<String>) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
        text.into(),
    ))))
}

/// The `session_info_update` whose only payload is cyrup's queue-depth `_meta` (`ACP-124`).
///
/// Upstream emits `{piAcp: {queueDepth: N, running: bool}}` and its own comment concedes Zed does
/// not render it today (`ACP-Q22`). It is emitted anyway, for the same reason upstream does: it is
/// the only place the depth is published, it costs one notification per queue transition, and a
/// client that *does* read it gets the steering/follow-up split cyrup can see and pi-acp cannot.
#[must_use]
pub fn queue_meta_update(queue_depth: usize, running: bool) -> SessionUpdate {
    let mut payload = serde_json::Map::new();
    payload.insert("queueDepth".into(), serde_json::json!(queue_depth));
    payload.insert("running".into(), serde_json::json!(running));
    let mut meta = Meta::new();
    meta.insert(META_NAMESPACE.into(), serde_json::Value::Object(payload));
    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().meta(meta))
}

/// The retry / compaction / queue status updates for one event. **The single producer of these
/// arms** (`ACP-142`, `ACP-143`, `ACP-124`).
///
/// # Why this lives here and not in [`mod@crate::translate`]
///
/// These four arms are the only ones in upstream's `handlePiEvent` switch that describe the
/// *turn's* progress rather than a message or a tool call, and `ACP-121`'s whole point is that the
/// turn's progress is this module's business. Splitting them across two producers is how the same
/// chunk gets emitted twice. `crate::translate::translate` must therefore emit **nothing** for
/// `AutoRetryStart`, `AutoRetryEnd`, `CompactionStart`, `CompactionEnd` and `QueueUpdate`; the test
/// `the_status_arms_have_exactly_one_producer` is the cross-check.
///
/// # `ACP-143` — the two compaction strings, and the gate upstream cannot express
///
/// `CompactionReason::Manual` emits nothing, which is exactly upstream's behaviour arrived at by a
/// different route: pi-acp handles only `auto_compaction_start`/`_end`, so a manual compaction
/// never reaches its switch at all.
///
/// **[CYRUP-DELTA] the success string is gated.** *What differs*: `CompactionEnd` carries `aborted`
/// and `error_message`, which upstream's event does not, so the end string is emitted only when
/// `!aborted && error_message.is_none()`. *What it costs*: an aborted or failed automatic
/// compaction now produces a start chunk with no end chunk, which reads as unfinished — because it
/// is. Emitting `…context was summarized to continue the session.` for a compaction that did not
/// summarize anything is a factual claim that is false, and the user's next message is the one that
/// overflows.
///
/// # `ACP-142` — the retry-finished string is gated on `success` too
///
/// **[CYRUP-DELTA] `AutoRetryEnd` emits [`RETRY_FINISHED_MESSAGE`] only when `success` is true.**
/// *What differs*: upstream's arm reads neither `success` nor `final_error` (its event carries
/// neither), so it prints `Retry finished, resuming.` on every retry sequence including an
/// exhausted one. *What it costs*: an exhausted ladder now produces three
/// `Retrying (attempt N/3, waiting Ms)...` chunks and no closing line, which reads as unfinished.
/// It is unfinished — driving the exhausted case shows the closing string asserting a continuation
/// that did not happen, followed by no answer at all, and the earlier reasoning that the sentence
/// is "true either way" does not survive contact with that transcript. What resumes is nothing;
/// what the user gets is the run's own failure, which `ACP-022` now reports as an error response
/// on the `session/prompt` this ladder was burning 14 seconds inside of. `final_error` is still
/// not emitted here — it is the same provider text `format_auto_retry_message` deliberately
/// withholds from the retry chunks, and it reaches the client once, in that error.
#[must_use]
pub fn status_updates(ev: &AgentSessionEvent) -> Vec<SessionUpdate> {
    match ev {
        AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts,
            delay_ms,
            // Never emitted. See `format_auto_retry_message`.
            error_message: _,
        } => vec![text_chunk(format_auto_retry_message(
            *attempt,
            *max_attempts,
            *delay_ms,
        ))],
        AgentSessionEvent::AutoRetryEnd { success: true, .. } => {
            vec![text_chunk(RETRY_FINISHED_MESSAGE)]
        }
        // An exhausted ladder. Nothing here — see the CYRUP-DELTA above. `final_error` is not
        // emitted for the same reason `AutoRetryStart`'s `error_message` is not: the provider's
        // sentence reaches the client exactly once, in `ACP-022`'s error response.
        AgentSessionEvent::AutoRetryEnd { .. } => Vec::new(),
        AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Threshold | CompactionReason::Overflow,
        } => vec![text_chunk(AUTO_COMPACTION_START_MESSAGE)],
        AgentSessionEvent::CompactionEnd {
            reason: CompactionReason::Threshold | CompactionReason::Overflow,
            aborted: false,
            error_message: None,
            ..
        } => vec![text_chunk(AUTO_COMPACTION_END_MESSAGE)],
        // `Manual` on either arm, and an aborted/failed automatic compaction: nothing. Named
        // rather than left to the catch-all so the decision is visible at the match.
        AgentSessionEvent::CompactionStart { .. } | AgentSessionEvent::CompactionEnd { .. } => {
            Vec::new()
        }
        // `ACP-124` option (b)'s richer payload: cyrup can see the steering/follow-up split, which
        // pi-acp's flat `queueDepth` cannot express. `running` is true because a queue update only
        // ever fires from a live run.
        AgentSessionEvent::QueueUpdate {
            steering,
            follow_up,
        } => vec![queue_meta_update(steering.len() + follow_up.len(), true)],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// The two seams the actor is written against
// ---------------------------------------------------------------------------------------------

/// Where a `session/update` notification goes.
///
/// # `ACP-155` — this being a plain `fn` is the whole mechanism
///
/// **`notify` is not `async` and returns nothing.** `Fanout::emit` does
/// `let _ = s.send(ev.clone()).await` over an `mpsc::channel(1024)` documented as *"backpressure →
/// slows the agent, never drops"*, so a pump task that awaits anything with unbounded human latency
/// stalls the agent at 1 024 queued events — and the turn cannot settle while the agent is blocked,
/// while the dialog cannot resolve while the task that would service it is blocked. That is a
/// deadlock, and upstream avoids it by detaching with `void this.handleExtensionUiRequest(ev)`.
///
/// Here it is avoided by construction twice over: dialogs never reach this task at all (they arrive
/// on [`crate::permission::PermissionBridge`]'s own channel and are serviced by its own task), and
/// this signature has no `await` for a future maintainer to put a client round trip behind.
/// `ConnectionTo::send_notification` is genuinely synchronous — it enqueues on an mpsc and returns
/// — so nothing is lost.
pub trait TurnSink: Send + 'static {
    /// Enqueue one update for `session_id`. Errors are swallowed (`ACP-122`).
    fn notify(&self, session_id: &SessionId, update: SessionUpdate);
}

impl TurnSink for ConnectionTo<Client> {
    fn notify(&self, session_id: &SessionId, update: SessionUpdate) {
        // `ACP-122` — a `send_notification` that fails must not stop the turn completing, mirroring
        // upstream's unconditional silent `.catch(() => {})`.
        let _ = self.send_notification(SessionNotification::new(session_id.clone(), update));
    }
}

/// What [`TurnAgent::start_run`] produced.
pub struct RunStarted {
    /// The session's own acceptance. See [`TurnAgent::start_run`] for how it is obtained.
    pub accepted: PromptAccepted,
    /// The **run-scoped** stream (`ACP-153`). Dropped without being pumped when `accepted` is
    /// [`PromptAccepted::Handled`], because no run will feed it.
    pub events: EventStream<AgentSessionEvent>,
}

/// The agent the turn drives — `AgentSession`, behind a trait so the actor is testable.
///
/// Every method re-acquires the runtime's **live** session in the production implementor
/// ([`RuntimeAgent`]), which is why there is no `rebind` step that re-caches an `Arc<AgentSession>`
/// the way `cyrup_modes::rpc`'s `rebind_session` does. See [`TurnAgent::rebound`].
pub trait TurnAgent: Send + Sync + 'static {
    /// `AgentSession::prompt` — **the run-scoped subscription** (`ACP-153`).
    ///
    /// `AgentSession::prompt` registers `fanout.subscribe_run()` *before* dispatching and returns
    /// that stream; `Fanout::end_run` clears the run-scoped senders immediately after
    /// `emit_agent_settled`, so the settle is the stream's last event. Session-wide `subscribe()`
    /// must never be substituted here.
    ///
    /// # Errors
    ///
    /// The typed `SessionServiceError` from the preflight, for `AcpFailure::classify` to decide at
    /// the boundary (`ACP-126`).
    fn start_run<'a>(
        &'a self,
        input: UserInput,
    ) -> BoxFuture<'a, Result<RunStarted, SessionServiceError>>;

    /// `AgentSession::prompt_with(.., FollowUp)` — fold a second submission into the live run
    /// (`ACP-124` option (b)).
    ///
    /// # Errors
    ///
    /// As [`TurnAgent::start_run`].
    fn fold_into_run<'a>(
        &'a self,
        input: UserInput,
    ) -> BoxFuture<'a, Result<PromptAccepted, SessionServiceError>>;

    /// `AgentSession::abort` — the **synchronous, fire-and-forget** half (`ACP-123`).
    fn abort<'a>(&'a self) -> BoxFuture<'a, ()>;

    /// Called after `AgentSessionEvent::SessionReplaced` has settled the turn (`ACP-061`,
    /// `ACP-154`).
    ///
    /// The default is a no-op, which is correct for [`RuntimeAgent`]: it re-reads
    /// `AgentSessionRuntime::session()` on every call, so there is no cached handle to refresh and
    /// the next prompt lands on the new session with no bookkeeping. It is a trait method rather
    /// than nothing at all because the **sinks** do need re-installing — a replacement brings a
    /// fresh `LiveHostServices`, so whoever owns the `PermissionBridge` must call
    /// `set_ui_sink`/`set_ui_effect_sink`/`add_error_listener` again, exactly as
    /// `cyrup_modes::rpc`'s `rebind_session` does. That owner implements this.
    fn rebound<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Read one file **through the session's own filesystem backend** (`ACP-131`, `ACP-135`,
    /// `ACP-156`).
    ///
    /// `abs` is the path already resolved against the session cwd
    /// ([`ToolCallLedger::resolve`](crate::ledger::ToolCallLedger::resolve)); `named` is the
    /// string the tool used, which is what the returned [`FileSnapshot`] must carry so a later
    /// `Diff.path` is the one the client will recognise.
    ///
    /// # Why this is on the agent rather than done inline
    ///
    /// `translate::snapshot_needed` deliberately returns a *request*, and its doc says returning a
    /// path is not permission to read it: with `confine_to_cwd` set, `TraversalFs::read` hard-denies
    /// a path outside the root, so a `std::fs::read_to_string` on the pump would ship — inside
    /// `Diff.new_text` — bytes this session's backend refuses to open. Routing the read through the
    /// agent means the production implementor ([`RuntimeAgent`]) uses `AgentSessionServices::fs`,
    /// the same handle the session's own tools hold.
    ///
    /// The default is [`FileSnapshot::unreadable`], which suppresses the diff. That is the honest
    /// answer for a test double with no filesystem, and it is the same answer a real `EACCES`
    /// produces — never a fabricated diff.
    ///
    /// **On `ACP-155`:** this IS awaited on the pump, unlike a dialog. It is a bounded local read
    /// with no client and no human in it — upstream performs the identical read *synchronously*
    /// with `readFileSync` inside the same event arm — so it delays the next `session/update` by
    /// one file read and can never park.
    fn snapshot<'a>(&'a self, abs: PathBuf, named: PathBuf) -> BoxFuture<'a, FileSnapshot> {
        let _ = abs;
        Box::pin(async move { FileSnapshot::unreadable(named) })
    }
}

/// The production [`TurnAgent`]: `AgentSessionRuntime`'s live session, re-read per call.
///
/// # [CYRUP-DELTA] — no cached session, so no `rebind_session`
///
/// **What differs.** `cyrup_modes::rpc`'s loop caches `session: Arc<AgentSession>` and re-acquires
/// it in `rebind_session` on a `watch_generation` bump. This holds only the runtime and calls
/// `runtime.session().await` — one `RwLock` read — at each of the three entry points.
///
/// **What it costs.** One uncontended read-lock acquisition per prompt and per cancel, against the
/// class of bug where a generation bump is missed and the front-end drives the *previous* session:
/// `ACP-154`'s hazard is that an extension's `ctx.newSession()` arrives as an ordinary tool call
/// **during** an ACP turn, so the window between the bump and the rebind is a window in which a
/// cached handle is wrong. Re-reading deletes the window rather than shrinking it.
pub struct RuntimeAgent {
    runtime: Arc<AgentSessionRuntime>,
}

impl RuntimeAgent {
    /// Bind to a runtime.
    #[must_use]
    pub fn new(runtime: Arc<AgentSessionRuntime>) -> Self {
        Self { runtime }
    }

    async fn session(&self) -> Arc<AgentSession> {
        self.runtime.session().await
    }
}

impl TurnAgent for RuntimeAgent {
    fn start_run<'a>(
        &'a self,
        input: UserInput,
    ) -> BoxFuture<'a, Result<RunStarted, SessionServiceError>> {
        Box::pin(async move {
            // `AgentSession::prompt_run`, NOT `prompt`. The difference is the whole reason that
            // method exists: `prompt` returns the run-scoped stream for every preflight outcome and
            // tells the caller nothing about which one happened, but an `input` extension handler
            // that fully services the submission (`PromptAccepted::Handled`) starts **no run**, so
            // no `AgentSettled` will ever reach this stream and a turn parked on it hangs the
            // editor forever with no timeout on either side.
            //
            // Before integration this was inferred from `session.is_run_active()` read immediately
            // after `prompt` returned — exact at that one point (`spawn_run` sets the driver latch
            // before `tokio::spawn`, and there is no `.await` in between) but an inference from
            // another function's internals, which a refactor of `spawn_run` could have broken in
            // silence. It is now the value the preflight itself produced.
            let (accepted, events) = self.session().await.prompt_run(input).await?;
            Ok(RunStarted { accepted, events })
        })
    }

    fn fold_into_run<'a>(
        &'a self,
        input: UserInput,
    ) -> BoxFuture<'a, Result<PromptAccepted, SessionServiceError>> {
        Box::pin(async move {
            let session = self.session().await;
            // `ACP-124` option (b), and FollowUp rather than Steer is the decision: a follow-up is
            // delivered after the current turn's work, which is what a second `session/prompt`
            // means; steering injects mid-turn and redirects the run the first prompt is still
            // waiting on. Upstream's `turnQueue` is FIFO-after-the-current-turn, i.e. follow-up
            // semantics, so this is also the parity choice.
            session
                .prompt_with(
                    input,
                    PromptOptions {
                        streaming_behavior: Some(StreamingBehavior::FollowUp),
                    },
                )
                .await
        })
    }

    /// `ACP-156` — the session's own `FsOps`, never `std::fs`. See [`TurnAgent::snapshot`].
    fn snapshot<'a>(&'a self, abs: PathBuf, named: PathBuf) -> BoxFuture<'a, FileSnapshot> {
        Box::pin(async move {
            let fs = Arc::clone(&self.session().await.services().fs);
            match fs.read(&abs).await {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => FileSnapshot::read(named, text),
                    // A binary file has no `Diff` to show. `unreadable` — not `absent` — is right:
                    // `absent` would claim the file did not exist and produce a whole-file diff
                    // with `oldText` omitted on the next write.
                    Err(_) => FileSnapshot::unreadable(named),
                },
                // `ToolError` is a bare message (`cyrup_core::tool::ToolError`), so a read failure
                // cannot be classified from its type and MUST NOT be classified from its text —
                // parsing a diagnostic turns a copy-edit into a wire regression. The probe is
                // `FsOps::metadata` through the SAME decorator stack: an entry the backend will
                // describe but not read is `Unreadable` (a binary file, an `EACCES`), and one it
                // will not describe at all is `Absent`.
                //
                // A path `TraversalFs` DENIES lands on `Absent` here, since it is denied both
                // times. That is deliberate and it cannot leak: `Absent` only makes a diff
                // *possible*, and the bytes in a diff come from the `After` read, which the same
                // backend denies too — so `after.before` is `None` and `tool_execution_end` emits
                // no diff at all. The confinement decides what is transmitted; this function only
                // decides what is claimed about a file it could not open.
                Err(_) => match fs.metadata(&abs).await {
                    Ok(_) => FileSnapshot::unreadable(named),
                    Err(_) => FileSnapshot::absent(named),
                },
            }
        })
    }

    fn abort<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // [CYRUP-DELTA] `abort()`, NOT `abort_and_settle()`.
            //
            // What differs: `ACP-123` names `abort_and_settle` as pi's `abort()`'s exact analogue,
            // and it is — for a caller that is not also the event pump. This one is. Its
            // `wait_for_idle()` tail cannot complete while the only consumer of the run-scoped
            // stream is parked inside it: the run's remaining events fill `Fanout`'s 1 024-slot
            // bounded channel, `emit` blocks on the awaited `send`, the run never reaches
            // `agent_settled`, and idle never arrives. It is bounded by `ABORT_SETTLE_TIMEOUT`
            // (30s) rather than permanent, which makes it a stall a reviewer will not reproduce.
            //
            // What it costs: nothing this front-end needs. `abort()` is the same two statements
            // minus the wait (`abort_retry(); agent.abort()`), and the settle this turn is waiting
            // for arrives on the stream we are still draining — which is strictly better evidence
            // of settlement than the latch `wait_for_idle` polls.
            self.session().await.abort();
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The actor
// ---------------------------------------------------------------------------------------------

/// A message to the per-session actor that owns the [`Turn`], the event pump and the ledger.
///
/// **The actor is not optional and it is not an implementation detail.** ADR-0028 F1 constrains
/// `Turn` to sit behind the same lock as whatever drains the event stream, and `ACP-122` requires
/// the response never to overtake a notification — which is true by construction only if the same
/// task that owns the event pump owns the reply channel, so `deliver(..)` is literally the last
/// statement after the final `notify`. A `Mutex<Turn>` shared between the pump and the handlers
/// does **not** give that; a channel to one owner does.
///
/// `ACP-155` — the pump must never await a client round trip. A permission dialog parked
/// unanswered must not stop the agent reaching `AgentSettled`; see [`TurnSink`] for the mechanism.
// `clippy::large_enum_variant` fires on `Prompt`, which carries a ~200-byte
// `Responder<PromptResponse>` plus a `UserInput` while `Cancel`/`Shutdown` are empty. Boxing it is
// refused for the same reason as on [`Turn::start`]: the reply channel is the message's entire
// payload, so a `Box` would move one allocation from the channel to the heap and add a deref, while
// making every consumer write `*reply`. `AgentSessionEvent` IS boxed here, because that one is large
// AND arrives at high frequency — one per stream delta — where a prompt arrives once per turn.
#[allow(clippy::large_enum_variant)]
pub enum TurnMessage<R = Responder<PromptResponse>> {
    /// A `session/prompt` was accepted by the connection; adopt it.
    Prompt {
        /// The session the prompt names. Checked against the actor's own (`ACP-120`).
        session_id: SessionId,
        /// The submission, already translated from `PromptRequest.prompt` by the caller — that
        /// translation is `ACP-158`/`translate/prompt.ts`'s job and deliberately not this
        /// module's.
        input: UserInput,
        /// Moved in, per [`Turn::start`].
        reply: R,
    },
    /// One event off `AgentSession::prompt`'s **run-scoped** stream, injected rather than pumped.
    ///
    /// The actor pumps its own stream; this is the seam that lets a test — and `cyrup-it` — drive
    /// the whole turn state machine with no session behind it. `ACP-153` still governs where the
    /// production events come from: run-scoped, never the session-wide `subscribe()`.
    Event(Box<AgentSessionEvent>),
    /// A `session/cancel` notification arrived (`ACP-123`).
    Cancel,
    /// The connection installed a new session under this actor (`ACP-061`). Re-points the id the
    /// actor answers for; the in-flight turn, if any, was already terminated by the
    /// `SessionReplaced` on its stream (`ACP-154`).
    Bind {
        /// The newly live session's id.
        session_id: SessionId,
    },
    /// Fail the in-flight turn with an error response (`ACP-126`). A no-op when idle.
    Fail {
        /// What the client is told.
        failure: AcpFailure,
    },
    /// The connection is going away; settle anything outstanding and stop.
    Shutdown,
}

/// The handle the `session/*` handlers hold. Cheap to clone.
pub struct TurnHandle<R = Responder<PromptResponse>> {
    tx: mpsc::UnboundedSender<TurnMessage<R>>,
}

impl<R> Clone for TurnHandle<R> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<R: PromptReply> TurnHandle<R> {
    /// Hand a `session/prompt` to the turn. Returns immediately (`ACP-123`'s interleaving rule).
    ///
    /// # Errors
    ///
    /// The reply channel, unchanged, when the actor is gone. Handed **back** rather than dropped
    /// for the same reason [`Turn::start`] does it: a dropped responder is an editor request that
    /// never completes.
    pub fn prompt(&self, session_id: SessionId, input: UserInput, reply: R) -> Result<(), R> {
        match self.tx.send(TurnMessage::Prompt {
            session_id,
            input,
            reply,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::SendError(TurnMessage::Prompt { reply, .. })) => Err(reply),
            // `SendError` is documented to carry back the value that failed to send, which is the
            // `Prompt` built two lines above, so no other variant can appear here. Written as a
            // fall-through rather than an `unreachable!()` because this crate contains no panic:
            // there is no `R` left to hand back, and the actor being gone means the connection is
            // too.
            Err(_) => Ok(()),
        }
    }

    /// `session/cancel` (`ACP-123`). Idempotent, answers nothing itself, never blocks.
    pub fn cancel(&self) {
        let _ = self.tx.send(TurnMessage::Cancel);
    }

    /// Re-point the actor at a newly installed session (`ACP-061`).
    pub fn bind(&self, session_id: SessionId) {
        let _ = self.tx.send(TurnMessage::Bind { session_id });
    }

    /// Fail the in-flight turn (`ACP-126`).
    pub fn fail(&self, failure: AcpFailure) {
        let _ = self.tx.send(TurnMessage::Fail { failure });
    }

    /// Inject an event. The seam [`TurnMessage::Event`] documents.
    pub fn inject(&self, event: AgentSessionEvent) {
        let _ = self.tx.send(TurnMessage::Event(Box::new(event)));
    }

    /// Settle anything outstanding and stop the actor.
    pub fn shutdown(&self) {
        let _ = self.tx.send(TurnMessage::Shutdown);
    }
}

/// The per-connection turn actor: **one** owner of the [`Turn`], the run-scoped event pump, the
/// [`ToolCallLedger`] and the notification sink.
///
/// `ACP-057`'s spawned-task rule lives on [`TurnActor::run`], which returns `()`: a
/// `cx.spawn(async move { actor.run().await; Ok(()) })` cannot propagate an `Err` because there is
/// none to propagate, and `ConnectionTo::spawn`'s own doc is *"if the spawned task returns an
/// error, the entire server will shut down."*
pub struct TurnActor<R = Responder<PromptResponse>> {
    session_id: SessionId,
    agent: Arc<dyn TurnAgent>,
    sink: Box<dyn TurnSink>,
    turn: Turn<R>,
    ledger: ToolCallLedger,
    rx: mpsc::UnboundedReceiver<TurnMessage<R>>,
    /// The run-scoped stream of the live run, or `None` when idle (`ACP-153`).
    events: Option<EventStream<AgentSessionEvent>>,
    /// The one submission currently being offered to the agent. See [`TurnActor::pump_admission`].
    admitting: Option<InFlight<R>>,
    /// Submissions waiting for it, in arrival order.
    waiting: VecDeque<(UserInput, R)>,
    /// A `session/cancel` that arrived while a submission was still being admitted, so there was
    /// no running turn to flag. Applied the moment one exists (`ACP-123`).
    cancel_pending: bool,
    /// `ACP-022` — the verdict of the most recent `AgentEnd` on this run, held until
    /// `AgentSettled` decides the turn.
    ///
    /// **Last one wins**, which is what makes the auto-retry ladder come out right in both
    /// directions: a ladder that recovers ends on a successful `AgentEnd` and the failed attempt's
    /// verdict is overwritten, and one that exhausts ends on the failing one. Reset by
    /// [`TurnActor::finish`], so a failure cannot leak from one turn into the next.
    termination: RunTermination,
}

/// The agent call for one submission, and the reply channel waiting on it.
///
/// The reply is held **beside** the future rather than inside it, which is the difference between
/// "a teardown drops one responder" and "no responder is ever dropped": a future cannot be opened
/// to get a value back out, but a struct field can be taken. [`TurnActor::drain_unadmitted`] is the
/// site that needs it.
struct InFlight<R> {
    future: Pin<Box<dyn Future<Output = Result<Submission, SessionServiceError>> + Send>>,
    reply: R,
}

/// One submission's acceptance, plus the run-scoped stream if it started a run.
struct Submission {
    accepted: PromptAccepted,
    /// `Some` only for a fresh run — a fold rides the stream the running turn already holds.
    events: Option<EventStream<AgentSessionEvent>>,
}

/// What woke the actor's `select!`. Owned, so the borrows of `self`'s three pollable fields end
/// before any handler runs.
// `clippy::large_enum_variant`: `AgentSessionEvent` is ~312 bytes and dominates. Boxing it would
// add a heap allocation PER STREAM DELTA — the highest-frequency path in the crate — to save a
// stack move of the same value the stream already handed us by value. `TurnMessage` boxes its
// event for the opposite reason: that one crosses a channel and is stored.
#[allow(clippy::large_enum_variant)]
enum Wake<R> {
    Message(Option<TurnMessage<R>>),
    Event(Option<AgentSessionEvent>),
    Admitted(Result<Submission, SessionServiceError>),
}

/// Poll the run-scoped stream when there is one, and park forever when there is not.
///
/// `mpsc::Receiver::recv` and a `ReceiverStream`'s `next` are both cancel-safe, which is what makes
/// the arms of the actor's `select!` safe to lose a race.
async fn next_run_event(
    events: &mut Option<EventStream<AgentSessionEvent>>,
) -> Option<AgentSessionEvent> {
    match events.as_mut() {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

/// Poll the in-flight admission when there is one, and park forever when there is not.
///
/// A future polled to completion inside `select!` is only cancel-safe if losing the race does not
/// lose work — here it cannot, because the future is held in `self.admitting` across iterations and
/// is only taken out when it resolves.
async fn next_admission<R>(
    admitting: &mut Option<InFlight<R>>,
) -> Result<Submission, SessionServiceError> {
    match admitting.as_mut() {
        Some(in_flight) => (&mut in_flight.future).await,
        None => std::future::pending().await,
    }
}

impl<R: PromptReply> TurnActor<R> {
    /// Build the actor and its handle.
    ///
    /// `session_id` is the id the connection told the client; a `Prompt` naming any other id is
    /// answered with [`SessionManager::unknown_session`] (`ACP-120`) rather than routed.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        cwd: AbsCwd,
        agent: Arc<dyn TurnAgent>,
        sink: Box<dyn TurnSink>,
    ) -> (TurnHandle<R>, Self) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            TurnHandle { tx },
            Self {
                session_id,
                agent,
                sink,
                turn: Turn::Idle,
                // `ACP-130` — the ledger resolves every tool-call location against the session
                // cwd, so it is constructed with it rather than reaching for a process-wide one.
                ledger: ToolCallLedger::new(cwd),
                rx,
                events: None,
                admitting: None,
                waiting: VecDeque::new(),
                cancel_pending: false,
                termination: RunTermination::Completed,
            },
        )
    }

    /// [`TurnActor::new`] plus `tokio::spawn`. The handle is all a handler needs.
    #[must_use]
    pub fn spawn(
        session_id: SessionId,
        cwd: AbsCwd,
        agent: Arc<dyn TurnAgent>,
        sink: Box<dyn TurnSink>,
    ) -> TurnHandle<R> {
        let (handle, actor) = Self::new(session_id, cwd, agent, sink);
        tokio::spawn(actor.run());
        handle
    }

    /// Drive the turn until the handle is dropped or [`TurnMessage::Shutdown`] arrives.
    ///
    /// Returns `()`, never `Result` — see the type's doc for why (`ACP-057`).
    pub async fn run(mut self) {
        loop {
            let wake = {
                // Disjoint field borrows: the receiver, the stream and the in-flight admission.
                // Every one of the three futures is cancel-safe, so losing a race drops nothing.
                let Self {
                    rx,
                    events,
                    admitting,
                    ..
                } = &mut self;
                tokio::select! {
                    message = rx.recv() => Wake::Message(message),
                    event = next_run_event(events) => Wake::Event(event),
                    result = next_admission(admitting) => Wake::Admitted(result),
                }
            };
            match wake {
                // The handle was dropped, or the connection asked to stop.
                Wake::Message(None) | Wake::Message(Some(TurnMessage::Shutdown)) => break,
                Wake::Message(Some(message)) => self.on_message(message).await,
                Wake::Event(Some(event)) => self.on_event(event).await,
                // `ACP-154`'s unnamed cause: the run-scoped stream ended with no settle.
                Wake::Event(None) => {
                    tracing::debug!(
                        session_id = %self.session_id,
                        "the run-scoped stream ended without an AgentSettled"
                    );
                    self.finish(TurnOutcome::Replaced);
                }
                Wake::Admitted(result) => self.on_admitted(result).await,
            }
        }
        // Never leave on a dropped reply channel: an unanswered `session/prompt` is a spinner
        // forever. The in-flight and waiting submissions are discharged first, then the turn.
        self.drain_unadmitted();
        self.finish(TurnOutcome::Cancelled);
    }

    /// Answer everything that was accepted but never reached the [`Turn`].
    ///
    /// Both the submission being admitted and everything queued behind it. This is why
    /// [`InFlight`] keeps the reply beside its future instead of inside it — at teardown the
    /// future is abandoned, and a reply inside it would go with it.
    fn drain_unadmitted(&mut self) {
        if let Some(in_flight) = self.admitting.take() {
            in_flight
                .reply
                .deliver(Ok(PromptResponse::new(StopReason::Cancelled)));
        }
        while let Some((_, reply)) = self.waiting.pop_front() {
            reply.deliver(Ok(PromptResponse::new(StopReason::Cancelled)));
        }
    }

    async fn on_message(&mut self, message: TurnMessage<R>) {
        match message {
            TurnMessage::Prompt {
                session_id,
                input,
                reply,
            } => self.on_prompt(session_id, input, reply),
            TurnMessage::Event(event) => self.on_event(*event).await,
            TurnMessage::Cancel => self.on_cancel().await,
            TurnMessage::Bind { session_id } => self.session_id = session_id,
            TurnMessage::Fail { failure } => self.finish(TurnOutcome::Refused(failure)),
            // Handled by the loop so that `run` can `break` out of it.
            TurnMessage::Shutdown => {}
        }
    }

    /// `session/prompt` (`ACP-121`, `ACP-124`, `ACP-126`, `ACP-153`).
    fn on_prompt(&mut self, session_id: SessionId, input: UserInput, reply: R) {
        // `ACP-120` — a prompt for an id this connection never issued is `Unknown sessionId: <id>`
        // at -32602. The message is `SessionManager::unknown_session`'s, called rather than
        // rebuilt so there is exactly one copy of a string Zed shows the user verbatim.
        if session_id != self.session_id {
            reply.deliver(Err(SessionManager::unknown_session(&session_id).into()));
            return;
        }
        self.waiting.push_back((input, reply));
        self.pump_admission();
    }

    /// Offer the next waiting submission to the agent — **off the pump, one at a time**.
    ///
    /// # `ACP-155` — why the agent call is not simply awaited here
    ///
    /// `AgentSession::prompt_with`'s fold path runs `prepare` (which fires the `input` extension
    /// event, so a wasm guest can open a dialog and sit there for as long as a human takes) and
    /// then `follow_up`, whose `emit_queue_update` does an **awaited** send onto `Fanout`'s
    /// 1 024-slot bounded channel — and this actor is the only consumer of the run-scoped end of
    /// that channel. Awaiting the fold inline therefore stops the drain while the running turn
    /// keeps producing; at 1 024 queued events `emit` blocks, the run can never reach
    /// `AgentSettled`, and the turn can never settle. That is a genuine deadlock with no timeout,
    /// and it is `ACP-155`'s scenario reached from the queue path rather than the dialog path.
    ///
    /// So the call becomes a future held in `self.admitting` and polled by the same `select!` that
    /// drains the stream: concurrent, but still on one task, so `Turn` needs no lock.
    ///
    /// # Why exactly one at a time
    ///
    /// The fresh-run and fold paths are different agent calls, and which one is right depends on
    /// whether a run is live. If two submissions were admitted concurrently, both would read
    /// "idle", both would call `AgentSession::prompt`, and the second would be refused with
    /// `StreamingNeedsBehavior` — an error where the user expected their message to queue.
    /// Serializing admission makes the read and the call atomic with respect to each other, and
    /// preserves arrival order into the queue, which is what the `position` in
    /// `Queued message (position N).` claims.
    fn pump_admission(&mut self) {
        if self.admitting.is_some() {
            return;
        }
        let Some((input, reply)) = self.waiting.pop_front() else {
            return;
        };
        let agent = Arc::clone(&self.agent);
        let folding = self.turn.is_running();
        self.admitting = Some(InFlight {
            future: Box::pin(async move {
                if folding {
                    agent.fold_into_run(input).await.map(|accepted| Submission {
                        accepted,
                        events: None,
                    })
                } else {
                    agent.start_run(input).await.map(|started| Submission {
                        accepted: started.accepted,
                        events: Some(started.events),
                    })
                }
            }),
            reply,
        });
    }

    /// The agent answered one submission.
    async fn on_admitted(&mut self, result: Result<Submission, SessionServiceError>) {
        let Some(in_flight) = self.admitting.take() else {
            // `next_admission` parks forever when there is nothing in flight, so this arm cannot
            // fire without one. Returning is the no-op an impossible state deserves; there is no
            // reply to lose.
            return;
        };
        self.admit(result, in_flight.reply).await;
        // A cancel that arrived while this submission was in the air had no running turn to flag.
        // Apply it now, so a user who pressed stop during preflight is not ignored (`ACP-123`).
        // The latch is TAKEN whether or not it lands: if this submission produced no turn (an
        // extension handled it), the cancel was for that submission and must not carry over onto
        // the next prompt, which the user sent afterwards.
        if std::mem::take(&mut self.cancel_pending) && self.turn.request_cancel() {
            self.agent.abort().await;
        }
        self.pump_admission();
    }

    async fn admit(&mut self, result: Result<Submission, SessionServiceError>, reply: R) {
        let submission = match result {
            Ok(submission) => submission,
            // `ACP-126` — the preflight refusal reaches the client as a JSON-RPC error, never as a
            // fabricated `end_turn`, and the connection stays open. `AcpFailure::classify` decides
            // whether that is the auth banner (-32000) or an ordinary failure. Only THIS
            // submission is refused: a queue refusal is not a reason to fail the turn the user is
            // already watching.
            Err(err) => return reply.deliver(Err(AcpFailure::classify(&err).into())),
        };

        if matches!(submission.accepted, PromptAccepted::Handled) {
            // An `input` extension handler fully serviced the submission: no run was started, so
            // no `AgentSettled` will ever arrive. Answering here is the difference between a slash
            // command handled by an extension and a hung editor. Any stream is dropped unpumped,
            // which is correct — `Fanout` prunes closed senders.
            return reply.deliver(Ok(PromptResponse::new(StopReason::EndTurn)));
        }

        // A fold whose run ended while the submission was in the air: `prompt_with`'s `prepare`
        // saw an idle session and started a FRESH run, for which this actor holds no run-scoped
        // subscription — so its settle is unobservable here. Answering `cancelled` is the honest
        // report: the submission was accepted by the session, but this request's turn cannot be
        // followed to completion. `end_turn` would claim a completion nobody watched.
        if matches!(submission.accepted, PromptAccepted::Started) && submission.events.is_none() {
            tracing::warn!(
                session_id = %self.session_id,
                "a queued submission started a run this turn cannot observe"
            );
            return reply.deliver(Ok(PromptResponse::new(StopReason::Cancelled)));
        }

        match self.turn.start(reply, submission.accepted) {
            Ok(Admission::Started) => {
                self.events = submission.events;
                self.emit(queue_meta_update(0, true));
            }
            Ok(Admission::Folded { position }) => {
                // Byte-exact, upstream's two updates in upstream's order.
                self.emit(text_chunk(queued_message_text(position)));
                self.emit(queue_meta_update(position, true));
            }
            // The turn and the acceptance disagree — a `Queued` whose run settled between the
            // dispatch and this line. The submission is in the session's queue and will ride the
            // next run, but no `AgentSettled` this turn observes will answer it, so it is answered
            // here rather than parked. Same reasoning as the arm above.
            Err(reply) => {
                tracing::warn!(
                    session_id = %self.session_id,
                    "a submission was accepted for a run that had already settled"
                );
                reply.deliver(Ok(PromptResponse::new(StopReason::Cancelled)));
            }
        }
    }

    /// `session/cancel` (`ACP-123`, `ACP-159`).
    ///
    /// Sets the flag and aborts; it answers **nothing**. The `stopReason: "cancelled"` is produced
    /// by the run's own settle, from the state of *that* turn — which is the whole point of
    /// [`Turn::settle`] owning the mapping. Upstream instead resolves its queued turns here and
    /// reads a session-wide flag afterwards, from a scope where `startTurn` may already have
    /// cleared it.
    ///
    /// **`ACP-159`, pinned:** under the folded-queue decision there is no separate queue to resolve
    /// without flushing, so the cancel path's order is: flag, abort, then a single settle on the
    /// run's own `AgentSettled` that answers every folded responder with `cancelled`. That is one
    /// order for both paths, where upstream has two.
    async fn on_cancel(&mut self) {
        if self.turn.request_cancel() {
            self.agent.abort().await;
            return;
        }
        // Upstream installs `pendingTurn` synchronously in `startTurn`, so a cancel issued between
        // the prompt and the first event still finds a turn to flag — its own test calls `cancel`
        // before any event and still expects `'cancelled'`. Here the turn is adopted only once the
        // agent has accepted, so that window would otherwise swallow the cancel and the run would
        // settle as `end_turn`: the user pressed stop and nothing stopped. The latch closes it;
        // `on_admitted` applies it.
        //
        // Idle with nothing in the air is legal and answers nothing — upstream included.
        self.cancel_pending = self.admitting.is_some() || !self.waiting.is_empty();
    }

    /// One event off the run-scoped stream (or injected).
    async fn on_event(&mut self, event: AgentSessionEvent) {
        // `ACP-142`/`ACP-143`/`ACP-124` — this module's own arms, emitted first so a status chunk
        // precedes anything the translator derives from the same event.
        for update in status_updates(&event) {
            self.emit(update);
        }

        // `ACP-131` / `ACP-135` / `ACP-156` — ask the pure decision function which read (if any)
        // this event needs, resolve the path against the session cwd, and perform the read through
        // the SESSION'S filesystem backend. The order is upstream's and is load-bearing: the
        // `Before` image must be taken before `translate` mutates the ledger, and the `After`
        // re-read finds its path in what the `Before` recorded there.
        let snapshot = match snapshot_needed(&self.ledger, &event) {
            Some(request) => {
                let abs = self.ledger.resolve(&request.path);
                Some(self.agent.snapshot(abs, request.path).await)
            }
            None => None,
        };

        // The pure core (agent B).
        let Translated { updates, turn } = translate(&mut self.ledger, &event, snapshot);
        for update in updates {
            self.emit(update);
        }

        match turn {
            TurnSignal::Continue | TurnSignal::Ignored => {}
            // `ACP-022` — record how this low-level run ended and keep pumping. Still NOT a
            // settle: a turn that auto-retries emits two `AgentEnd`s and one `AgentSettled`.
            TurnSignal::RunEnded(termination) => self.termination = termination,
            // `ACP-121` — the ONE settle point. `TurnEnd` reaches `TurnSignal::Continue` and
            // `AgentEnd` reaches `RunEnded`, `will_retry` included; neither settles.
            TurnSignal::Settled => {
                let outcome =
                    match std::mem::replace(&mut self.termination, RunTermination::Completed) {
                        RunTermination::Completed => TurnOutcome::Settled,
                        RunTermination::MaxTokens => TurnOutcome::MaxTokens,
                        // An abort that arrived by a route other than `session/cancel` — a replaced
                        // session, a host teardown. `cancelled` is what the schema has for it, and it
                        // is what `Turn::settle` answers a cancelled turn with anyway.
                        RunTermination::Aborted => TurnOutcome::Cancelled,
                        RunTermination::Failed(failure) => TurnOutcome::Failed(failure),
                    };
                self.finish(outcome);
            }
            // `ACP-154` — respond, then rebind. In that order: the pending request must be
            // discharged before anything else can fail.
            TurnSignal::Rebind { generation } => {
                tracing::debug!(generation, "session replaced under the turn");
                self.finish(TurnOutcome::Replaced);
                self.agent.rebound().await;
            }
        }
    }

    /// Terminate the turn, in the one order `ACP-122` allows.
    fn finish(&mut self, outcome: TurnOutcome) {
        // The run this turn was bound to is over on every path that reaches here.
        self.events = None;
        self.cancel_pending = false;
        // `ACP-022` — a verdict belongs to exactly one turn. Reset here rather than only on the
        // settle path so a turn ended by `Replaced`/`Refused` cannot leave a failure behind for
        // the next prompt to be answered with.
        self.termination = RunTermination::Completed;
        // `ACP-137` — teardown at the end of the run, which upstream's `cleanupToolCall` only
        // performs per tool-end and therefore leaks for a tool that never ends.
        self.ledger.clear();

        let Some(action) = self.turn.settle(outcome) else {
            // A second settle is a no-op, not a double-respond and not a panic (ADR-0028 §7).
            return;
        };
        // The last notification…
        self.emit(queue_meta_update(0, false));
        // …and then the response. `ACP-122`: nothing may be sent between these two statements.
        action.deliver();
    }

    fn emit(&self, update: SessionUpdate) {
        self.sink.notify(&self.session_id, update);
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
    use cyrup_session_svc::InputSource;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------------------------
    // Test doubles
    // -----------------------------------------------------------------------------------------

    /// A [`PromptReply`] that records what it was answered with. Stands in for
    /// `Responder<PromptResponse>`, whose constructor is private to the SDK's dispatch loop.
    struct Recorder {
        answers: Arc<Mutex<Vec<Result<StopReason, i32>>>>,
        /// The error frames in full. Kept beside the code-only log rather than replacing it so
        /// every existing `Err(-32603)` assertion still reads as one value, while `ACP-022`'s
        /// tests can assert the message and `data` the client actually receives.
        errors: Arc<Mutex<Vec<agent_client_protocol::Error>>>,
    }

    impl PromptReply for Recorder {
        fn deliver(self, result: Result<PromptResponse, agent_client_protocol::Error>) {
            let recorded = match result {
                Ok(response) => Ok(response.stop_reason),
                Err(error) => {
                    let code = i32::from(error.code);
                    self.errors
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(error);
                    Err(code)
                }
            };
            self.answers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(recorded);
        }
    }

    /// A shared answer log plus a factory for reply channels that write into it.
    #[derive(Clone, Default)]
    struct Answers {
        codes: Arc<Mutex<Vec<Result<StopReason, i32>>>>,
        errors: Arc<Mutex<Vec<agent_client_protocol::Error>>>,
    }

    impl Answers {
        fn reply(&self) -> Recorder {
            Recorder {
                answers: Arc::clone(&self.codes),
                errors: Arc::clone(&self.errors),
            }
        }
        fn taken(&self) -> Vec<Result<StopReason, i32>> {
            self.codes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
        /// Every error frame delivered so far, in order.
        fn errors(&self) -> Vec<agent_client_protocol::Error> {
            self.errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// An `AgentEnd` whose terminal assistant message failed (`ACP-022`).
    ///
    /// Built through `AssistantMessage::errored` — the same constructor
    /// `ProviderError::into_error_message` uses — so the shape under test is the one production
    /// produces, not one this test invented.
    fn failed_run(error_message: &str) -> AgentSessionEvent {
        run_ending_in(cyrup_core::StopReason::Error, Some(error_message))
    }

    /// An `AgentEnd` whose terminal assistant message completed normally.
    fn settled_run() -> AgentSessionEvent {
        run_ending_in(cyrup_core::StopReason::Stop, None)
    }

    fn run_ending_in(
        stop_reason: cyrup_core::StopReason,
        error_message: Option<&str>,
    ) -> AgentSessionEvent {
        let mut assistant = cyrup_core::AssistantMessage::errored(
            cyrup_core::ProviderId::from("anthropic"),
            "claude-test",
            None,
            stop_reason,
            error_message.unwrap_or_default(),
        );
        assistant.error_message = error_message.map(str::to_string);
        AgentSessionEvent::AgentEnd {
            messages: vec![Arc::new(cyrup_session_svc::AgentMessage::Assistant(
                Arc::new(assistant),
            ))],
            will_retry: false,
        }
    }

    /// A [`TurnSink`] that collects notifications in order.
    #[derive(Clone, Default)]
    struct Recording(Arc<Mutex<Vec<SessionUpdate>>>);

    impl TurnSink for Recording {
        fn notify(&self, _session_id: &SessionId, update: SessionUpdate) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(update);
        }
    }

    impl Recording {
        fn texts(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter_map(|u| match u {
                    SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                        ContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        }
        fn count(&self) -> usize {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
        }
    }

    /// A [`TurnAgent`] whose run-scoped stream the test feeds by hand.
    struct FakeAgent {
        /// Handed out by `start_run`, one per test.
        stream: Mutex<Option<EventStream<AgentSessionEvent>>>,
        /// What `start_run` reports, so the `Handled` path is reachable.
        accepted: PromptAccepted,
        /// `None` means "accept"; `Some` is the preflight refusal.
        refuse: Option<fn() -> SessionServiceError>,
        /// Parks `start_run` / `fold_into_run` until the test releases it, standing in for a
        /// `prepare` that runs an `input` extension handler with a human behind it.
        block_start: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        block_fold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        aborts: Arc<Mutex<usize>>,
        starts: Arc<Mutex<usize>>,
        folds: Arc<Mutex<usize>>,
        rebinds: Arc<Mutex<usize>>,
    }

    impl FakeAgent {
        fn new(stream: EventStream<AgentSessionEvent>) -> Self {
            Self {
                stream: Mutex::new(Some(stream)),
                accepted: PromptAccepted::Started,
                refuse: None,
                block_start: Mutex::new(None),
                block_fold: Mutex::new(None),
                aborts: Arc::new(Mutex::new(0)),
                starts: Arc::new(Mutex::new(0)),
                folds: Arc::new(Mutex::new(0)),
                rebinds: Arc::new(Mutex::new(0)),
            }
        }

        /// Take and await a gate, if the test armed one.
        async fn gate(slot: &Mutex<Option<tokio::sync::oneshot::Receiver<()>>>) {
            let gate = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
        }
        fn bump(counter: &Mutex<usize>) {
            *counter
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        }
        fn read(counter: &Mutex<usize>) -> usize {
            *counter
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl TurnAgent for FakeAgent {
        fn start_run<'a>(
            &'a self,
            _input: UserInput,
        ) -> BoxFuture<'a, Result<RunStarted, SessionServiceError>> {
            Box::pin(async move {
                Self::bump(&self.starts);
                Self::gate(&self.block_start).await;
                if let Some(make) = self.refuse {
                    return Err(make());
                }
                let events = self
                    .stream
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .unwrap_or_else(|| Box::pin(futures::stream::pending()));
                Ok(RunStarted {
                    accepted: self.accepted,
                    events,
                })
            })
        }

        fn fold_into_run<'a>(
            &'a self,
            _input: UserInput,
        ) -> BoxFuture<'a, Result<PromptAccepted, SessionServiceError>> {
            Box::pin(async move {
                Self::bump(&self.folds);
                Self::gate(&self.block_fold).await;
                if let Some(make) = self.refuse {
                    return Err(make());
                }
                Ok(PromptAccepted::Queued(StreamingBehavior::FollowUp))
            })
        }

        fn abort<'a>(&'a self) -> BoxFuture<'a, ()> {
            Box::pin(async move { Self::bump(&self.aborts) })
        }

        fn rebound<'a>(&'a self) -> BoxFuture<'a, ()> {
            Box::pin(async move { Self::bump(&self.rebinds) })
        }
    }

    /// A driven turn: the actor on its own task, plus everything a test needs to poke it.
    struct Harness {
        handle: TurnHandle<Recorder>,
        events: mpsc::Sender<AgentSessionEvent>,
        answers: Answers,
        sink: Recording,
        agent: Arc<FakeAgent>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(|agent| agent)
        }

        fn with(configure: impl FnOnce(FakeAgent) -> FakeAgent) -> Self {
            let (events_tx, events_rx) = mpsc::channel(64);
            // `futures::stream::unfold` rather than `tokio_stream::wrappers::ReceiverStream`:
            // `tokio-stream` is not a dependency of this crate and adding one is not this
            // module's to make.
            let stream: EventStream<AgentSessionEvent> =
                Box::pin(futures::stream::unfold(events_rx, |mut rx| async move {
                    rx.recv().await.map(|event| (event, rx))
                }));
            let agent = Arc::new(configure(FakeAgent::new(stream)));
            let sink = Recording::default();
            let answers = Answers::default();
            let (handle, actor) = TurnActor::<Recorder>::new(
                SessionId::new("s1"),
                AbsCwd::parse("/tmp/cyrup-acp-turn-tests").expect("absolute"),
                Arc::clone(&agent) as Arc<dyn TurnAgent>,
                Box::new(sink.clone()),
            );
            let task = tokio::spawn(actor.run());
            Self {
                handle,
                events: events_tx,
                answers,
                sink,
                agent,
                task,
            }
        }

        fn prompt(&self) {
            let ok = self.handle.prompt(
                SessionId::new("s1"),
                UserInput::text("hello", InputSource::Rpc),
                self.answers.reply(),
            );
            assert!(ok.is_ok(), "the actor is alive");
        }

        async fn feed(&self, event: AgentSessionEvent) {
            self.events
                .send(event)
                .await
                .expect("the actor is draining");
        }

        /// Let the actor drain everything queued. `yield_now` in a loop rather than a sleep: the
        /// actor is on the same single-threaded test runtime, so a yield is a full drain.
        async fn settle_scheduler(&self) {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        }

        async fn shutdown(self) {
            self.handle.shutdown();
            let _ = self.task.await;
        }
    }

    // -----------------------------------------------------------------------------------------
    // ADR-0028 F1 — the turn value
    // -----------------------------------------------------------------------------------------

    /// ADR-0028 §7 — "a test asserting the prompt responder is answered exactly once degenerates to
    /// a test that `Turn::settle` returns `None` on the second call". This is that canary.
    #[test]
    fn settling_twice_is_a_no_op_not_a_double_respond() {
        let answers = Answers::default();
        let mut turn = Turn::default();
        assert!(!turn.is_running());
        assert!(turn.settle(TurnOutcome::Settled).is_none());

        turn.start(answers.reply(), PromptAccepted::Started)
            .map_err(|_| "adopted")
            .expect("idle turns adopt a started run");
        assert!(turn.is_running());

        let action = turn.settle(TurnOutcome::Settled).expect("the first settle");
        assert_eq!(action.breadth(), 1);
        action.deliver();

        assert!(
            turn.settle(TurnOutcome::Settled).is_none(),
            "a late AgentSettled from a replaced session is a no-op"
        );
        assert!(turn.settle(TurnOutcome::Cancelled).is_none());
        assert_eq!(answers.taken(), vec![Ok(StopReason::EndTurn)]);
    }

    /// ACP-123 / ACP-159 — cancel is idempotent, and a cancel with nothing running is a legal
    /// no-op rather than an error.
    #[test]
    fn cancel_is_idempotent_and_legal_when_idle() {
        let answers = Answers::default();
        let mut turn = Turn::default();
        assert!(!turn.request_cancel());
        assert!(!turn.request_cancel());

        turn.start(answers.reply(), PromptAccepted::Started)
            .map_err(|_| "adopted")
            .expect("adopted");
        assert!(turn.request_cancel());
        assert!(turn.request_cancel(), "idempotent");
        assert_eq!(
            turn.running().map(RunningTurn::cancel_state),
            Some(CancelState::Requested)
        );
    }

    /// ACP-121 / ACP-123 — the full stop-reason table, computed inside `settle` and nowhere else.
    #[test]
    fn the_stop_reason_is_a_function_of_the_outcome_and_this_turns_own_cancel_flag() {
        let table: Vec<(TurnOutcome, bool, Result<StopReason, i32>)> = vec![
            (TurnOutcome::Settled, false, Ok(StopReason::EndTurn)),
            (TurnOutcome::Settled, true, Ok(StopReason::Cancelled)),
            (TurnOutcome::Cancelled, false, Ok(StopReason::Cancelled)),
            (TurnOutcome::Cancelled, true, Ok(StopReason::Cancelled)),
            // ACP-154: a replaced session did NOT complete the turn, so `end_turn` would be
            // ACP-121's silent-wrong-output failure by another route.
            (TurnOutcome::Replaced, false, Ok(StopReason::Cancelled)),
            (
                TurnOutcome::Refused(AcpFailure::Internal {
                    message: "boom".into(),
                }),
                false,
                Err(crate::error::INTERNAL_ERROR_CODE),
            ),
            (
                TurnOutcome::Refused(AcpFailure::AuthRequired {
                    detail: "no key".into(),
                }),
                false,
                Err(crate::error::AUTH_REQUIRED_CODE),
            ),
            // `ACP-022` — a failed run is an ERROR, never a successful empty turn…
            (
                TurnOutcome::Failed(AcpFailure::Internal {
                    message: "http 500: upstream exploded".into(),
                }),
                false,
                Err(crate::error::INTERNAL_ERROR_CODE),
            ),
            (
                TurnOutcome::Failed(AcpFailure::AuthRequired {
                    detail: "http 401: invalid x-api-key".into(),
                }),
                false,
                Err(crate::error::AUTH_REQUIRED_CODE),
            ),
            // …except when the client cancelled, which the schema says MUST win even when the
            // cancellation is what caused the underlying exception.
            (
                TurnOutcome::Failed(AcpFailure::AuthRequired {
                    detail: "http 401: invalid x-api-key".into(),
                }),
                true,
                Ok(StopReason::Cancelled),
            ),
            // A `Refused` is the HOST tearing the turn down, not the run reporting on itself, so
            // it is answered even to a client that cancelled.
            (
                TurnOutcome::Refused(AcpFailure::Internal {
                    message: "torn down".into(),
                }),
                true,
                Err(crate::error::INTERNAL_ERROR_CODE),
            ),
            (TurnOutcome::MaxTokens, false, Ok(StopReason::MaxTokens)),
            (TurnOutcome::MaxTokens, true, Ok(StopReason::Cancelled)),
        ];

        for (outcome, cancelled, expected) in table {
            let answers = Answers::default();
            let mut turn = Turn::default();
            turn.start(answers.reply(), PromptAccepted::Started)
                .map_err(|_| "adopted")
                .expect("adopted");
            if cancelled {
                turn.request_cancel();
            }
            let label = format!("{outcome:?} cancelled={cancelled}");
            turn.settle(outcome)
                .unwrap_or_else(|| panic!("{label} must settle"))
                .deliver();
            assert_eq!(answers.taken(), vec![expected], "{label}");
        }
    }

    /// ADR-0028 F1 — `start` hands the reply channel BACK rather than dropping it whenever the
    /// acceptance and the turn state disagree. A dropped one is a `session/prompt` that never
    /// completes, with no timeout on the ACP side.
    #[test]
    fn a_rejected_admission_returns_the_reply_channel_instead_of_dropping_it() {
        let answers = Answers::default();

        // Queued with nothing running.
        let mut idle: Turn<Recorder> = Turn::default();
        let returned = idle
            .start(
                answers.reply(),
                PromptAccepted::Queued(StreamingBehavior::FollowUp),
            )
            .expect_err("a queued submission needs a live run");
        returned.deliver(Ok(PromptResponse::new(StopReason::EndTurn)));

        // Handled: nothing was started, so there is no settle to park on.
        let mut idle2: Turn<Recorder> = Turn::default();
        let returned = idle2
            .start(answers.reply(), PromptAccepted::Handled)
            .expect_err("a handled submission starts no run");
        returned.deliver(Ok(PromptResponse::new(StopReason::EndTurn)));

        // Started twice.
        let mut running: Turn<Recorder> = Turn::default();
        running
            .start(answers.reply(), PromptAccepted::Started)
            .map_err(|_| "adopted")
            .expect("adopted");
        let returned = running
            .start(answers.reply(), PromptAccepted::Started)
            .expect_err("two runs cannot start on one session");
        returned.deliver(Ok(PromptResponse::new(StopReason::EndTurn)));

        assert_eq!(answers.taken().len(), 3, "every one was answered");
        assert_eq!(running.depth(), 1, "and none of them was adopted");
    }

    /// ACP-124 — the folded position is upstream's N: the count of submissions waiting BEHIND the
    /// running one, 1-based.
    #[test]
    fn folding_numbers_the_queue_the_way_upstream_does() {
        let answers = Answers::default();
        let mut turn = Turn::default();
        assert_eq!(
            turn.start(answers.reply(), PromptAccepted::Started)
                .map_err(|_| "not adopted"),
            Ok(Admission::Started)
        );
        for expected in 1..=3usize {
            assert_eq!(
                turn.start(
                    answers.reply(),
                    PromptAccepted::Queued(StreamingBehavior::FollowUp)
                )
                .map_err(|_| "not adopted"),
                Ok(Admission::Folded { position: expected })
            );
        }
        assert_eq!(turn.depth(), 4);
        let action = turn.settle(TurnOutcome::Settled).expect("settles");
        assert_eq!(
            action.breadth(),
            4,
            "ACP-124's cost, asserted rather than implied: N requests settle together"
        );
        action.deliver();
        assert_eq!(answers.taken(), vec![Ok(StopReason::EndTurn); 4]);
    }

    // -----------------------------------------------------------------------------------------
    // ACP-142 / ACP-143 — the byte-exact status strings
    // -----------------------------------------------------------------------------------------

    /// ACP-142 — `formatAutoRetryMessage`'s exact output, including the sub-second bump the cut
    /// list explicitly preserves.
    #[test]
    fn the_retry_message_is_byte_exact_including_the_sub_second_bump() {
        assert_eq!(
            format_auto_retry_message(1, 4, 1500),
            "Retrying (attempt 1/4, waiting 2s)..."
        );
        assert_eq!(
            format_auto_retry_message(1, 3, 400),
            "Retrying (attempt 1/3, waiting 1s)...",
            "a non-zero delay that rounds to zero is bumped to 1"
        );
        assert_eq!(
            format_auto_retry_message(1, 3, 0),
            "Retrying (attempt 1/3, waiting 0s)...",
            "an actually-zero delay stays 0"
        );
        // Math.round agreement at the half-second boundaries.
        assert!(format_auto_retry_message(2, 5, 2500).ends_with("waiting 3s)..."));
        assert!(format_auto_retry_message(2, 5, 2499).ends_with("waiting 2s)..."));
        // No overflow on a delay no backoff would ever produce.
        assert!(format_auto_retry_message(1, 1, u64::MAX / 2).contains("waiting "));
    }

    /// ACP-142 — the emitted chunk carries the sentence and NOTHING of the provider's error text.
    #[test]
    fn no_retry_chunk_leaks_the_error_message() {
        let updates = status_updates(&AgentSessionEvent::AutoRetryStart {
            attempt: 1,
            max_attempts: 4,
            delay_ms: 1500,
            error_message: "sk-live-DEADBEEF rejected by provider".into(),
        });
        assert_eq!(updates.len(), 1);
        let json = serde_json::to_string(&updates[0]).expect("serialises");
        assert!(json.contains("Retrying (attempt 1/4, waiting 2s)..."));
        assert!(
            !json.contains("DEADBEEF") && !json.contains("rejected by provider"),
            "the error_message field is never emitted: {json}"
        );

        // A ladder that RECOVERED says so, and still says nothing about the attempt that failed.
        let ended = status_updates(&AgentSessionEvent::AutoRetryEnd {
            success: true,
            attempt: 4,
            final_error: None,
        });
        let json = serde_json::to_string(&ended).expect("serialises");
        assert!(json.contains("Retry finished, resuming."));

        // `ACP-142`'s gate: an EXHAUSTED ladder says nothing at all. The closing sentence asserts
        // that the run resumed, and on this path nothing resumed — the failure reaches the client
        // as `ACP-022`'s error response on the `session/prompt` instead, once, and `final_error`
        // is not leaked here any more than `error_message` is above.
        let exhausted = status_updates(&AgentSessionEvent::AutoRetryEnd {
            success: false,
            attempt: 4,
            final_error: Some("still failing".into()),
        });
        assert!(
            exhausted.is_empty(),
            "an exhausted ladder must not claim it resumed: {exhausted:?}"
        );
    }

    /// ACP-143 — the compaction table, exactly as the unit's *Verify* line writes it.
    #[test]
    fn the_compaction_strings_are_byte_exact_and_the_success_string_is_gated() {
        let text = |ev: &AgentSessionEvent| -> Vec<String> {
            status_updates(ev)
                .iter()
                .filter_map(|u| match u {
                    SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                        ContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        };

        for reason in [CompactionReason::Threshold, CompactionReason::Overflow] {
            assert_eq!(
                text(&AgentSessionEvent::CompactionStart { reason }),
                vec!["Context nearing limit, running automatic compaction...".to_string()],
                "{reason:?}"
            );
        }
        assert!(
            text(&AgentSessionEvent::CompactionStart {
                reason: CompactionReason::Manual
            })
            .is_empty(),
            "a manual compaction produces nothing — upstream handles no such event at all"
        );

        let end = |aborted: bool, error: Option<&str>, reason: CompactionReason| {
            text(&AgentSessionEvent::CompactionEnd {
                reason,
                result: None,
                aborted,
                will_retry: false,
                error_message: error.map(str::to_owned),
            })
        };
        assert_eq!(
            end(false, None, CompactionReason::Threshold),
            vec![
                "Automatic compaction finished; context was summarized to continue the session."
                    .to_string()
            ]
        );
        assert!(
            end(true, None, CompactionReason::Threshold).is_empty(),
            "an aborted compaction summarized nothing, so the success string would be a lie"
        );
        assert!(end(false, Some("out of tokens"), CompactionReason::Overflow).is_empty());
        assert!(end(false, None, CompactionReason::Manual).is_empty());
    }

    /// ACP-142 / ACP-143 / ACP-124 — exactly one producer for these arms. If this fails because
    /// `translate` grew the same arms, delete them THERE: the turn's own progress is this module's
    /// business (`ACP-121`), and two producers means the client sees each chunk twice.
    #[test]
    fn the_status_arms_have_exactly_one_producer() {
        let mut ledger =
            ToolCallLedger::new(AbsCwd::parse("/tmp/cyrup-acp-turn-tests").expect("absolute"));
        let events = [
            AgentSessionEvent::AutoRetryStart {
                attempt: 1,
                max_attempts: 2,
                delay_ms: 100,
                error_message: String::new(),
            },
            AgentSessionEvent::AutoRetryEnd {
                success: true,
                attempt: 1,
                final_error: None,
            },
            AgentSessionEvent::CompactionStart {
                reason: CompactionReason::Threshold,
            },
            AgentSessionEvent::CompactionEnd {
                reason: CompactionReason::Threshold,
                result: None,
                aborted: false,
                will_retry: false,
                error_message: None,
            },
            AgentSessionEvent::QueueUpdate {
                steering: vec!["a".into()],
                follow_up: vec![],
            },
        ];
        for event in &events {
            assert!(
                translate(&mut ledger, event, None).updates.is_empty(),
                "translate must not also emit this arm: {event:?}"
            );
            assert!(
                !status_updates(event).is_empty(),
                "…and this module must: {event:?}"
            );
        }
    }

    /// ACP-124 — the queue `_meta` shape, and the namespace rename recorded on `META_NAMESPACE`.
    #[test]
    fn the_queue_meta_is_namespaced_to_cyrup_and_keeps_upstreams_shape() {
        let json = serde_json::to_value(queue_meta_update(2, true)).expect("serialises");
        assert_eq!(json["sessionUpdate"], "session_info_update");
        assert_eq!(json["_meta"]["cyrupAcp"]["queueDepth"], 2);
        assert_eq!(json["_meta"]["cyrupAcp"]["running"], true);
        assert!(
            json.get("_meta").and_then(|m| m.get("piAcp")).is_none(),
            "another product's name does not go in a cyrup transcript"
        );
        assert_eq!(queued_message_text(1), "Queued message (position 1).");
        assert_eq!(queued_message_text(12), "Queued message (position 12).");
    }

    // -----------------------------------------------------------------------------------------
    // ACP-121 / ACP-153 — the actor
    // -----------------------------------------------------------------------------------------

    /// **ACP-121, the critical.** pi-acp's own component scenario, driven through the actor:
    /// `AgentStart / AutoRetryStart / AgentEnd{will_retry} / AgentStart / TurnEnd /
    /// AgentEnd{!will_retry}` must leave the prompt UNRESOLVED, and only `AgentSettled` resolves
    /// it — exactly once, despite two `AgentEnd`s.
    #[tokio::test]
    async fn a_prompt_resolves_only_on_agent_settled_and_exactly_once() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        for event in [
            AgentSessionEvent::AgentStart,
            AgentSessionEvent::AutoRetryStart {
                attempt: 1,
                max_attempts: 4,
                delay_ms: 1500,
                error_message: "transient".into(),
            },
            AgentSessionEvent::AgentEnd {
                messages: Vec::new(),
                will_retry: true,
            },
            AgentSessionEvent::AgentStart,
            AgentSessionEvent::AgentEnd {
                messages: Vec::new(),
                will_retry: false,
            },
        ] {
            h.feed(event).await;
        }
        h.settle_scheduler().await;
        assert!(
            h.answers.taken().is_empty(),
            "two AgentEnds and a retry must NOT resolve the prompt"
        );
        assert_eq!(
            h.sink.texts(),
            vec!["Retrying (attempt 1/4, waiting 2s)...".to_string()],
            "the retry chunk did reach the client while the turn stayed open"
        );

        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken(), vec![Ok(StopReason::EndTurn)]);

        // A second settle — a late one from a replaced session — must not respond again. Injected
        // rather than fed, because in production the run-scoped stream is already gone by now:
        // `Fanout::end_run` clears it immediately after `emit_agent_settled`.
        h.handle.inject(AgentSessionEvent::AgentSettled);
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken().len(), 1, "exactly one PromptResponse");
        h.shutdown().await;
    }

    /// **ACP-022, the critical.** A provider failure mid-turn must NOT be `stopReason: end_turn`.
    ///
    /// This is the exact transcript the field report reproduced three ways (a real Bedrock 403, an
    /// injected 401, an injected 500 after a tool result): the run streams nothing, `AgentEnd`
    /// carries an `AssistantMessage` with `StopReason::Error` and the provider's sentence, and
    /// `AgentSettled` follows. Before this unit the client was answered
    /// `{"stopReason":"end_turn"}` — a successful, empty turn — while the JSONL recorded
    /// `stopReason='error'`. The two must agree.
    #[tokio::test]
    async fn a_provider_failure_is_an_error_response_not_an_empty_end_turn() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        h.feed(AgentSessionEvent::AgentStart).await;
        h.feed(failed_run("http 500: upstream exploded")).await;
        h.settle_scheduler().await;
        assert!(
            h.answers.taken().is_empty(),
            "ACP-121 still holds: AgentEnd does not settle"
        );

        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::INTERNAL_ERROR_CODE)],
            "a failed run is an error response, never a successful empty turn"
        );
        let errors = h.answers.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message, "http 500: upstream exploded",
            "the provider's own sentence reaches the client, which is the whole point"
        );
        h.shutdown().await;
    }

    /// ACP-022 — a mid-turn 401/403 is the auth-required error upstream's third
    /// `maybeAuthRequiredError` call site raises, with `ACP-016`'s payload.
    #[tokio::test]
    async fn a_mid_turn_401_asks_the_client_to_authenticate() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        h.feed(failed_run("http 401: invalid x-api-key")).await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;

        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::AUTH_REQUIRED_CODE)]
        );
        let error = &h.answers.errors()[0];
        assert_eq!(
            error.message,
            crate::error::AUTH_REQUIRED_MESSAGE,
            "`message` is upstream's sentence; the provider detail rides in `data`"
        );
        let data = error.data.clone().expect("ACP-016 attaches data");
        assert_eq!(data["detail"], "http 401: invalid x-api-key");
        assert!(
            data["authMethods"].as_array().is_some_and(|m| m.len() == 1),
            "ACP-016 — the full method list, so a client can render the button from the error \
             alone: {data}"
        );
        h.shutdown().await;
    }

    /// ACP-022 / ACP-142 — a retry ladder that RECOVERS settles `end_turn`, and the failed
    /// attempt's verdict is not the one that answers.
    ///
    /// This is why the verdict is last-`AgentEnd`-wins rather than sticky: the first `AgentEnd`
    /// here carries the very error the ladder then recovers from.
    #[tokio::test]
    async fn a_recovered_retry_ladder_still_settles_end_turn() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        h.feed(AgentSessionEvent::AgentStart).await;
        h.feed(AgentSessionEvent::AutoRetryStart {
            attempt: 1,
            max_attempts: 3,
            delay_ms: 2000,
            error_message: "http 500: transient".into(),
        })
        .await;
        h.feed(failed_run("http 500: transient")).await;
        h.feed(AgentSessionEvent::AgentStart).await;
        h.feed(AgentSessionEvent::AutoRetryEnd {
            success: true,
            attempt: 1,
            final_error: None,
        })
        .await;
        h.feed(settled_run()).await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;

        assert_eq!(h.answers.taken(), vec![Ok(StopReason::EndTurn)]);
        assert!(
            h.sink
                .texts()
                .contains(&"Retry finished, resuming.".to_string()),
            "the recovery DID resume, so the closing sentence is true here: {:?}",
            h.sink.texts()
        );
        h.shutdown().await;
    }

    /// ACP-022 / ACP-142 — an EXHAUSTED ladder reports the failure and never claims it resumed.
    ///
    /// The observed transcript this replaces: three byte-exact retry chunks, then
    /// `Retry finished, resuming.`, then `{"stopReason":"end_turn"}` fourteen seconds later with
    /// no answer anywhere.
    #[tokio::test]
    async fn an_exhausted_retry_ladder_reports_the_failure_and_never_claims_it_resumed() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        for attempt in 1..=3u32 {
            h.feed(AgentSessionEvent::AgentStart).await;
            h.feed(AgentSessionEvent::AutoRetryStart {
                attempt,
                max_attempts: 3,
                delay_ms: 1000 * u64::from(attempt) * 2,
                error_message: "http 500: still down".into(),
            })
            .await;
            h.feed(failed_run("http 500: still down")).await;
        }
        h.feed(AgentSessionEvent::AutoRetryEnd {
            success: false,
            attempt: 3,
            final_error: Some("http 500: still down".into()),
        })
        .await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;

        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::INTERNAL_ERROR_CODE)]
        );
        let texts = h.sink.texts();
        assert!(
            !texts.iter().any(|t| t == "Retry finished, resuming."),
            "nothing resumed: {texts:?}"
        );
        assert_eq!(
            texts
                .iter()
                .filter(|t| t.starts_with("Retrying (attempt "))
                .count(),
            3,
            "the three retry chunks are unchanged and still byte-exact: {texts:?}"
        );
        h.shutdown().await;
    }

    /// ACP-022 — a cancel outranks the run's own failure.
    ///
    /// The schema says `Cancelled` MUST be returned when the client cancelled, "even if the
    /// cancellation causes exceptions in underlying operations" — and a cancel IS the usual cause
    /// of the abort the provider then reports.
    #[tokio::test]
    async fn a_cancelled_turn_reports_cancelled_even_when_the_run_failed() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        h.handle.cancel();
        h.settle_scheduler().await;
        h.feed(failed_run("http 401: invalid x-api-key")).await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;

        assert_eq!(h.answers.taken(), vec![Ok(StopReason::Cancelled)]);
        assert!(
            h.answers.errors().is_empty(),
            "a cancelled turn is not an auth prompt"
        );
        h.shutdown().await;
    }

    /// ACP-022's sibling — `StopReason::Length` is `max_tokens`, not `end_turn`.
    #[tokio::test]
    async fn a_truncated_run_reports_max_tokens() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        h.feed(run_ending_in(cyrup_core::StopReason::Length, None))
            .await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken(), vec![Ok(StopReason::MaxTokens)]);
        h.shutdown().await;
    }

    /// ACP-022 — a verdict belongs to exactly one turn.
    ///
    /// Without the reset in `TurnActor::finish`, the next prompt on the same connection is
    /// answered with the previous turn's provider error — a failure that would look like a
    /// flapping provider and be impossible to reproduce.
    #[tokio::test]
    async fn a_failure_does_not_leak_into_the_next_turn() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        h.feed(failed_run("http 500: once")).await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::INTERNAL_ERROR_CODE)]
        );

        // The second turn's events are INJECTED: the harness hands out one run-scoped stream and
        // the first settle dropped it, which is exactly what `Fanout::end_run` does in production.
        h.prompt();
        h.settle_scheduler().await;
        h.handle.inject(settled_run());
        h.handle.inject(AgentSessionEvent::AgentSettled);
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![
                Err(crate::error::INTERNAL_ERROR_CODE),
                Ok(StopReason::EndTurn)
            ],
            "the second turn is answered on its own merits"
        );
        h.shutdown().await;
    }

    /// ACP-122 — the response is written after the final notification. Asserted structurally: the
    /// settle's own `session_info_update` is in the sink *before* the answer is recorded, which is
    /// only observable because both are recorded in order by the same task.
    #[tokio::test]
    async fn the_response_never_overtakes_a_notification() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        let before_settle = h.sink.count();

        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;

        assert_eq!(h.answers.taken(), vec![Ok(StopReason::EndTurn)]);
        assert!(
            h.sink.count() > before_settle,
            "the final queue-depth notification was written before the response"
        );
        h.shutdown().await;
    }

    /// ACP-123 — a cancel mid-run aborts the agent, answers nothing itself, and the run's own
    /// `AgentSettled` carries `stopReason: cancelled`.
    #[tokio::test]
    async fn a_cancelled_turn_settles_as_cancelled_on_its_own_agent_settled() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        h.handle.cancel();
        h.settle_scheduler().await;
        assert_eq!(FakeAgent::read(&h.agent.aborts), 1, "the agent was aborted");
        assert!(
            h.answers.taken().is_empty(),
            "cancel answers nothing itself — the settle does"
        );

        // Idempotent, and still nothing answered.
        h.handle.cancel();
        h.settle_scheduler().await;
        assert!(h.answers.taken().is_empty());

        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken(), vec![Ok(StopReason::Cancelled)]);
        h.shutdown().await;
    }

    /// ACP-123 — a `session/cancel` for an idle session is a legal no-op that neither answers nor
    /// aborts.
    #[tokio::test]
    async fn cancelling_an_idle_session_is_a_no_op() {
        let h = Harness::new();
        h.handle.cancel();
        h.settle_scheduler().await;
        assert_eq!(FakeAgent::read(&h.agent.aborts), 0);
        assert!(h.answers.taken().is_empty());
        h.shutdown().await;
    }

    /// **ACP-154.** The session is replaced mid-turn: the pending request gets a response rather
    /// than hanging, and the driver rebinds.
    #[tokio::test]
    async fn session_replaced_answers_the_pending_prompt_and_rebinds() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        h.feed(AgentSessionEvent::SessionReplaced { generation: 7 })
            .await;
        h.settle_scheduler().await;

        assert_eq!(
            h.answers.taken(),
            vec![Ok(StopReason::Cancelled)],
            "a replaced session did not complete the turn"
        );
        assert_eq!(FakeAgent::read(&h.agent.rebinds), 1, "the driver rebound");
        h.shutdown().await;
    }

    /// **ACP-154's unnamed cause.** The run-scoped stream ends with no settle at all — another
    /// run's `Fanout::end_run` clearing the run-scoped senders wholesale. The request must still
    /// be answered.
    #[tokio::test]
    async fn a_stream_that_ends_without_a_settle_still_answers_the_prompt() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        // Dropping the harness's sender is what ends the stream; it is the only one.
        let Harness {
            handle,
            events,
            answers,
            task,
            ..
        } = h;
        drop(events);
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert_eq!(answers.taken(), vec![Ok(StopReason::Cancelled)]);
        handle.shutdown();
        let _ = task.await;
    }

    /// ACP-153 — the turn is bound to ONE run's stream. After a settle the actor holds no stream,
    /// so a later event from any other run cannot reach a turn that is not running.
    #[tokio::test]
    async fn a_settle_unbinds_the_run_scoped_stream() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken().len(), 1);

        // Injected directly, i.e. as if a session-wide subscription had delivered it: with no turn
        // running there is nothing to settle and nothing is answered.
        h.handle.inject(AgentSessionEvent::AgentSettled);
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken().len(),
            1,
            "a settle from a run this turn did not start must not resolve anything"
        );
        h.shutdown().await;
    }

    /// ACP-126 — a preflight refusal is a JSON-RPC error, never a fabricated `end_turn`, and the
    /// actor survives it and serves the next prompt.
    #[tokio::test]
    async fn a_refused_preflight_answers_with_an_error_and_the_actor_survives() {
        let h = Harness::with(|mut agent| {
            agent.refuse = Some(|| SessionServiceError::NoModelSelected);
            agent
        });
        h.prompt();
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::AUTH_REQUIRED_CODE)],
            "NoModelSelected classifies as auth-required (ADR-0028 F4)"
        );

        // Still alive: a second prompt is answered too.
        h.prompt();
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken().len(), 2);
        h.shutdown().await;
    }

    /// ACP-120 — a prompt naming a session this connection never issued is `Unknown sessionId:
    /// <id>` at -32602, and does not disturb the live turn.
    #[tokio::test]
    async fn a_prompt_for_an_unknown_session_is_rejected_by_the_turn() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;

        let ok = h.handle.prompt(
            SessionId::new("bogus"),
            UserInput::text("hi", InputSource::Rpc),
            h.answers.reply(),
        );
        assert!(ok.is_ok());
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::INVALID_PARAMS_CODE)]
        );
        assert_eq!(
            FakeAgent::read(&h.agent.folds),
            0,
            "and it never reached the agent"
        );
        h.shutdown().await;
    }

    /// ACP-124 — a second prompt while a run is in flight folds into it: exactly one run starts,
    /// the queued chunk is byte-exact, and both requests settle on the same `AgentSettled`.
    #[tokio::test]
    async fn two_overlapping_prompts_fold_into_one_run_and_settle_together() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        h.prompt();
        h.settle_scheduler().await;

        assert_eq!(
            FakeAgent::read(&h.agent.folds),
            1,
            "exactly one run started"
        );
        assert_eq!(
            h.sink.texts(),
            vec!["Queued message (position 1).".to_string()]
        );
        assert!(h.answers.taken().is_empty(), "neither has settled yet");

        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Ok(StopReason::EndTurn), Ok(StopReason::EndTurn)],
            "ACP-124's recorded cost: N requests settle on one AgentSettled"
        );
        h.shutdown().await;
    }

    /// The `Prepared::Handled` hole: an `input` extension handler services the submission, no run
    /// starts, and no `AgentSettled` will ever arrive on the stream. The prompt must be answered
    /// anyway — otherwise a slash command handled by an extension hangs the editor forever.
    #[tokio::test]
    async fn a_submission_an_extension_handled_is_answered_rather_than_parked() {
        let h = Harness::with(|mut agent| {
            agent.accepted = PromptAccepted::Handled;
            agent
        });
        h.prompt();
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken(), vec![Ok(StopReason::EndTurn)]);
        h.shutdown().await;
    }

    /// ACP-126 — a host tearing a session down discharges the in-flight responder instead of
    /// dropping it, and a `fail` with nothing running is a no-op.
    #[tokio::test]
    async fn the_host_can_fail_an_in_flight_turn_and_failing_an_idle_one_is_a_no_op() {
        let h = Harness::new();
        h.handle.fail(AcpFailure::Internal {
            message: "nothing running".into(),
        });
        h.settle_scheduler().await;
        assert!(h.answers.taken().is_empty());

        h.prompt();
        h.settle_scheduler().await;
        h.handle.fail(AcpFailure::Internal {
            message: "the host tore the session down".into(),
        });
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::INTERNAL_ERROR_CODE)]
        );
        h.shutdown().await;
    }

    /// ACP-057 / ACP-121 — the actor never leaves a responder unanswered, including on the
    /// shutdown path. A dropped `Responder` is an editor request that never completes.
    #[tokio::test]
    async fn shutdown_settles_an_outstanding_turn_rather_than_dropping_it() {
        let h = Harness::new();
        h.prompt();
        h.settle_scheduler().await;
        assert!(h.answers.taken().is_empty());

        let answers = h.answers.clone();
        h.shutdown().await;
        assert_eq!(answers.taken(), vec![Ok(StopReason::Cancelled)]);
    }

    /// ACP-061 — `Bind` re-points the actor at a newly installed session, so a prompt for the new
    /// id is served and one for the old id is `Unknown sessionId`.
    #[tokio::test]
    async fn bind_repoints_the_actor_at_the_newly_installed_session() {
        let h = Harness::new();
        h.handle.bind(SessionId::new("s2"));
        h.settle_scheduler().await;

        let ok = h.handle.prompt(
            SessionId::new("s1"),
            UserInput::text("stale", InputSource::Rpc),
            h.answers.reply(),
        );
        assert!(ok.is_ok());
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Err(crate::error::INVALID_PARAMS_CODE)],
            "the old id is no longer this connection's session"
        );

        let ok = h.handle.prompt(
            SessionId::new("s2"),
            UserInput::text("fresh", InputSource::Rpc),
            h.answers.reply(),
        );
        assert!(ok.is_ok());
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken().len(), 1, "the new id was accepted");
        h.shutdown().await;
    }

    /// **ACP-155, the queue path.** A fold must not be awaited on the pump: while a submission is
    /// being admitted, the run-scoped stream is still drained. Asserted by parking the agent's
    /// admission and then pushing events through — they must arrive, and the turn must be able to
    /// settle, while the second prompt is still in the air.
    #[tokio::test]
    async fn the_pump_keeps_draining_while_a_submission_is_being_admitted() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let h = Harness::with(|mut agent| {
            agent.block_fold = Mutex::new(Some(release_rx));
            agent
        });
        h.prompt();
        h.settle_scheduler().await;

        // The second prompt parks inside `fold_into_run`.
        h.prompt();
        h.settle_scheduler().await;
        assert!(h.sink.texts().is_empty(), "the fold has not been admitted");

        // The pump is still alive: a status chunk gets through, and so does the settle.
        h.feed(AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Threshold,
        })
        .await;
        h.settle_scheduler().await;
        assert_eq!(
            h.sink.texts(),
            vec!["Context nearing limit, running automatic compaction...".to_string()],
            "the event pump kept draining while the fold was in flight"
        );

        let _ = release_tx.send(());
        h.settle_scheduler().await;
        h.shutdown().await;
    }

    /// ACP-123 — a `session/cancel` that lands while the submission is still being admitted is
    /// applied to the turn the moment one exists. Upstream installs `pendingTurn` synchronously
    /// and its own test cancels before any event; without the latch the run would settle as
    /// `end_turn` after the user pressed stop.
    #[tokio::test]
    async fn a_cancel_during_preflight_is_not_swallowed() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let h = Harness::with(|mut agent| {
            agent.block_start = Mutex::new(Some(release_rx));
            agent
        });
        h.prompt();
        h.settle_scheduler().await;
        h.handle.cancel();
        h.settle_scheduler().await;
        assert_eq!(
            FakeAgent::read(&h.agent.aborts),
            0,
            "there is nothing to abort yet"
        );

        let _ = release_tx.send(());
        h.settle_scheduler().await;
        assert_eq!(
            FakeAgent::read(&h.agent.aborts),
            1,
            "the latched cancel reached the turn as soon as it existed"
        );

        h.feed(AgentSessionEvent::AgentSettled).await;
        h.settle_scheduler().await;
        assert_eq!(h.answers.taken(), vec![Ok(StopReason::Cancelled)]);
        h.shutdown().await;
    }

    /// A latched cancel belongs to the submission that was in the air when it arrived, not to the
    /// next prompt the user sends afterwards.
    #[tokio::test]
    async fn a_latched_cancel_does_not_carry_over_to_the_next_prompt() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let h = Harness::with(|mut agent| {
            agent.accepted = PromptAccepted::Handled;
            agent.block_start = Mutex::new(Some(release_rx));
            agent
        });
        h.prompt();
        h.settle_scheduler().await;
        h.handle.cancel();
        let _ = release_tx.send(());
        h.settle_scheduler().await;
        assert_eq!(
            h.answers.taken(),
            vec![Ok(StopReason::EndTurn)],
            "the extension handled it before the cancel could reach a turn"
        );

        h.prompt();
        h.settle_scheduler().await;
        assert_eq!(
            FakeAgent::read(&h.agent.aborts),
            0,
            "the stale latch must not cancel a prompt the user sent after it"
        );
        h.shutdown().await;
    }

    /// Admission is serialized, so two prompts arriving back to back cannot both read "idle" and
    /// both call `AgentSession::prompt` — the second would be refused with
    /// `StreamingNeedsBehavior`, an error where the user expected their message to queue.
    #[tokio::test]
    async fn two_prompts_arriving_together_produce_one_run_and_one_fold() {
        let h = Harness::new();
        h.prompt();
        h.prompt();
        h.settle_scheduler().await;
        assert_eq!(FakeAgent::read(&h.agent.starts), 1);
        assert_eq!(FakeAgent::read(&h.agent.folds), 1);
        assert_eq!(
            h.sink.texts(),
            vec!["Queued message (position 1).".to_string()]
        );
        h.shutdown().await;
    }

    /// Nothing accepted is ever dropped on the teardown path, including a submission still waiting
    /// behind an in-flight admission.
    #[tokio::test]
    async fn teardown_answers_a_submission_that_never_reached_the_turn() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let h = Harness::with(|mut agent| {
            agent.block_start = Mutex::new(Some(release_rx));
            agent
        });
        h.prompt();
        h.prompt();
        h.settle_scheduler().await;
        assert!(h.answers.taken().is_empty());

        let answers = h.answers.clone();
        h.shutdown().await;
        assert_eq!(
            answers.taken(),
            vec![Ok(StopReason::Cancelled), Ok(StopReason::Cancelled)],
            "both are answered: the one still being admitted (its reply lives beside the \
             future, not inside it) and the one queued behind it"
        );
        let _ = release_tx.send(());
    }

    /// The handle hands the reply channel back when the actor is gone, rather than dropping it.
    #[tokio::test]
    async fn a_prompt_to_a_dead_actor_returns_the_reply_channel() {
        let answers = Answers::default();
        let (handle, actor) = TurnActor::<Recorder>::new(
            SessionId::new("s1"),
            AbsCwd::parse("/tmp/cyrup-acp-turn-tests").expect("absolute"),
            Arc::new(FakeAgent::new(Box::pin(futures::stream::pending()))) as Arc<dyn TurnAgent>,
            Box::new(Recording::default()),
        );
        drop(actor);
        let returned = handle
            .prompt(
                SessionId::new("s1"),
                UserInput::text("hi", InputSource::Rpc),
                answers.reply(),
            )
            .expect_err("the actor is gone");
        returned.deliver(Err(AcpFailure::Internal {
            message: "the connection is closing".into(),
        }
        .into()));
        assert_eq!(
            answers.taken(),
            vec![Err(crate::error::INTERNAL_ERROR_CODE)],
            "the request was answered, not dropped"
        );
    }
}
