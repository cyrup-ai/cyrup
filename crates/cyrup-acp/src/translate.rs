//! The pure core: `AgentSessionEvent` -> `Vec<SessionUpdate>`, over an explicit ledger.
//!
//! **Owner: agent B (`ACP-122`…`ACP-141`, `ACP-151`, `ACP-156`, `ACP-157`).**
//!
//! ADR-0028 finding F2, behaviour half. Port of pi-acp v0.0.33 `src/acp/session.ts`'s
//! `handlePiEvent` (the whole ~300-line switch), `emit` / `flushEmits` / `lastEmit`,
//! `emitBashToolCall`, `emitBashOutputUpdate` and `cleanupToolCall`, plus
//! `src/acp/translate/{bash,pi-tools,pi-messages}.ts`.
//!
//! **Pure: no `ConnectionTo`, no filesystem, no tokio.** Upstream does a synchronous `readFileSync`
//! *inside* the `tool_execution_start` and `tool_execution_end` arms; in Rust that would block the
//! event pump and therefore delay every subsequent `session/update`. The read is the shell's, and
//! [`snapshot_needed`] tells the shell which read to perform — itself pure, so the decision is
//! table-testable with no tempdir.
//!
//! # `ACP-122` — the `lastEmit` barrier is deleted, not ported, and this signature is why
//!
//! Upstream chains every notification onto `lastEmit` and awaits it before resolving a turn,
//! because `conn.sessionUpdate` is a promise. cyrup's `ConnectionTo::send_notification` is
//! synchronous — it enqueues on an mpsc and returns — so notifications are already ordered among
//! themselves and a barrier would order nothing.
//!
//! What the barrier really bought is that the **response never overtakes a notification**, and that
//! is re-established structurally rather than by a promise chain: [`translate()`] returns the updates
//! and the turn signal *in one value*, so the shell's only sensible loop is "send every update,
//! then act on `turn`". The signal that ends a turn — [`TurnSignal::Settled`] — is returned by an
//! arm that emits **no** updates at all, so there is nothing that could be left unsent behind a
//! response. Asserted by `the_settle_arm_has_nothing_left_to_send`.
//!
//! Two things this does **not** buy, stated plainly because ADR-0028 F1 records them as guarantees
//! not gained: the shell can still send the updates on a different task from the responder (which
//! compiles and races), and a `send_notification` returning `Err` must be swallowed with `let _ =`
//! rather than `?`, since a propagated `Err` out of `cx.spawn` tears down the whole connection.
//! Neither is expressible here; both are `crates/cyrup-it` assertions.
//!
//! # What has no port, and why
//!
//! `translate/bash.ts`'s twelve-key `bashCommand` probe and `translate/pi-tools.ts`'s four-deep
//! `toolResultToText` stdout ladder exist because both read `Record<string, unknown>` off a wire.
//! In-process, `AgentSessionEvent::ToolExecutionStart { args: Value, .. }` comes from cyrup's own
//! tool and `StreamEvent::ToolCallDelta { content_index, partial, .. }` is typed
//! (`crates/cyrup-provider/src/stream.rs` — the fields `session.ts` reaches for as
//! `ame?.partial?.content?.[ame?.contentIndex ?? 0]`). There is nothing to port. Likewise
//! `stripAnsi`: `cyrup_session_svc::bash::strip_ansi` is better (it handles OSC and `ESC \`, which
//! upstream's regex lacks) — and it is applied by the bash tool itself, upstream of this layer, so
//! there is no site here that would call it.
//!
//! `normalizePiMessageText` / `normalizePiAssistantText` are **refuted** as port units (`ACP-152`):
//! `extract_full_content` and `join_text`
//! (`crates/cyrup-session-svc/src/session/transcript.rs`) already are those functions, byte for
//! byte, from the same pi helper pi-acp copied. This module calls neither, because nothing in the
//! *live* event stream needs them — they are `session/load` replay's (`ACP-214`), and a third
//! divergent copy here is exactly what `ACP-152` was struck to prevent. [`tool_result_to_text`]'s
//! step 2 is the one place the same shape appears, and it is `toolResultToText`'s own step, not
//! `extractFullContent`'s.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionUpdate, ToolCallContent, ToolCallId, ToolCallLocation,
};
use cyrup_core::{Content, StopReason};
use cyrup_session_svc::{AgentMessage, AgentSessionEvent, StreamEvent};
use serde_json::Value;

use crate::error::AcpFailure;
use crate::ledger::{
    Announce, FileSnapshot, ToolCallLedger, ToolClass, ToolStatus, UpdatePatch, diff_content,
};

/// What the shell must do next, as a named outcome rather than an `Option`.
#[derive(Debug)]
pub enum TurnSignal {
    /// Keep pumping. The event was handled; [`Translated::updates`] may or may not be empty.
    Continue,
    /// `AgentSessionEvent::AgentSettled` — **the only settle point** (`ACP-121`, ADR-0028 F1). The
    /// shell clears the ledger and settles the turn here and nowhere else.
    Settled,
    /// `AgentSessionEvent::SessionReplaced { generation }` — rebind, do **not** settle from the old
    /// subscription (`ACP-154`). The pending `session/prompt` still gets a response; that is the
    /// turn's job, driven by [`crate::turn::TurnOutcome::Replaced`].
    Rebind {
        /// The runtime's new generation.
        generation: u64,
    },
    /// `AgentSessionEvent::AgentEnd` — one low-level run ended, carrying **how** it ended
    /// (`ACP-022`).
    ///
    /// This is **not** a settle (`ACP-121`): a turn that auto-retries emits two `AgentEnd`s and
    /// exactly one `AgentSettled`, so the shell keeps the turn open and only records the
    /// termination. It exists because the terminal `AssistantMessage` is the *only* place a
    /// mid-turn provider failure is observable — `ProviderError::into_error_message` flattens
    /// request and stream failures into it rather than throwing, so `AgentSession::prompt` returns
    /// `Ok` on a provider 401 — and folding this arm into [`TurnSignal::Continue`], as it was
    /// before, is what made every provider failure reach the client as `stopReason: "end_turn"`
    /// with no error and no content.
    RunEnded(RunTermination),
    /// A cyrup super-set event with no ACP representation, deliberately dropped — mirroring
    /// `cyrup_modes::is_upstream_wire_event`'s own exclusions. Named rather than folded into
    /// [`TurnSignal::Continue`] so "we decided to drop this" and "nothing happened" are
    /// distinguishable in a test.
    Ignored,
}

/// How the run that just ended terminated, read off its terminal `AssistantMessage` (`ACP-022`).
///
/// The two fields this reads — `AssistantMessage::stop_reason` and
/// `AssistantMessage::error_message` (`crates/cyrup-core/src/message/assistant.rs`) — are the same
/// two that are persisted to the session JSONL, so the client and the transcript cannot disagree
/// about whether a turn failed.
///
/// **Last message wins, always.** `AgentEnd.messages` is the run's messages and the terminal
/// assistant turn is the last `AgentMessage::Assistant` in it; the shell then keeps the newest
/// `AgentEnd`'s verdict. That is what makes the auto-retry ladder come out right in both
/// directions: a ladder that recovers ends on a successful `AgentEnd` and settles `end_turn`, and
/// one that exhausts ends on the failing one and settles as a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunTermination {
    /// `StopReason::{Stop, ToolUse, Deferred, Pending}`, or a run with no assistant message at all
    /// (an extension-serviced submission). The turn completed.
    Completed,
    /// `StopReason::Length` — the provider stopped at its token ceiling. ACP has a first-class
    /// `StopReason::MaxTokens` for exactly this, and reporting `end_turn` instead tells the editor
    /// a truncated answer is complete.
    MaxTokens,
    /// `StopReason::Aborted`. `ProviderError::is_aborted` is the only producer, so this is a
    /// cancellation that arrived by a route other than `session/cancel`.
    Aborted,
    /// `StopReason::Error`, classified by [`AcpFailure::classify_terminal`].
    Failed(AcpFailure),
}

impl RunTermination {
    /// Read the verdict off an `AgentEnd`'s messages.
    ///
    /// A run with no assistant turn is [`RunTermination::Completed`], not a failure: that is what
    /// an `input` extension handler that fully services the submission produces, and inventing an
    /// error for it would be worse than the `end_turn` this whole change is removing.
    #[must_use]
    pub fn of(messages: &[std::sync::Arc<AgentMessage>]) -> Self {
        let terminal = messages.iter().rev().find_map(|message| match &**message {
            AgentMessage::Assistant(assistant) => Some(assistant),
            _ => None,
        });
        let Some(assistant) = terminal else {
            return RunTermination::Completed;
        };
        match assistant.stop_reason {
            StopReason::Error => RunTermination::Failed(AcpFailure::classify_terminal(
                assistant.error_message.as_deref().unwrap_or_default(),
            )),
            StopReason::Aborted => RunTermination::Aborted,
            StopReason::Length => RunTermination::MaxTokens,
            // `Pending` is unreachable on a settled message — `StreamEvent::terminal` rewrites it
            // to `Error` — and `Deferred` is a success terminal (the provider took the request and
            // returned a handle). Both are named rather than left to a `_` arm so a new upstream
            // stop reason is a decision someone makes AT THIS MATCH.
            StopReason::Stop | StopReason::ToolUse | StopReason::Deferred | StopReason::Pending => {
                RunTermination::Completed
            }
        }
    }
}

/// What the shell must send, plus what it must do next.
pub struct Translated {
    /// In order. The shell writes each as a `SessionNotification` against the session's id.
    pub updates: Vec<SessionUpdate>,
    /// See [`TurnSignal`].
    pub turn: TurnSignal,
}

impl Translated {
    fn nothing(turn: TurnSignal) -> Self {
        Self {
            updates: Vec::new(),
            turn,
        }
    }

    fn one(update: SessionUpdate) -> Self {
        Self {
            updates: vec![update],
            turn: TurnSignal::Continue,
        }
    }

    fn maybe(update: Option<SessionUpdate>) -> Self {
        Self {
            updates: update.into_iter().collect(),
            turn: TurnSignal::Continue,
        }
    }
}

/// A file read the shell must perform **before** calling [`translate()`] with the same event.
///
/// `ACP-131` / `ACP-135`. Carries the tool-call id alongside the path so the shell cannot attach a
/// read to the wrong call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRequest {
    /// The call the read belongs to.
    pub tool_call_id: ToolCallId,
    /// The path **as the tool named it** — possibly relative. Resolve it with
    /// [`ToolCallLedger::resolve`] before opening it; it is handed back unresolved because that is
    /// the string the [`FileSnapshot`] must carry, and resolving twice is how a `..` gets applied
    /// to an already-absolute path.
    pub path: PathBuf,
    /// Which end of the tool this read is for. The shell does not need to branch on it — both are
    /// "read this path" — but a log line that cannot say which one is a log line nobody can debug.
    pub phase: SnapshotPhase,
}

/// Which end of a file-mutating tool a [`SnapshotRequest`] belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotPhase {
    /// The pre-mutation image, at `tool_execution_start` (`ACP-131`).
    Before,
    /// The re-read at `tool_execution_end`, whose bytes become `Diff.new_text` (`ACP-135`).
    After,
}

/// Which read the shell must perform **before** calling [`translate()`]. Pure.
///
/// Port of the two `readFileSync` calls pi-acp v0.0.33 `session.ts` makes *inside* its
/// `tool_execution_start` and `tool_execution_end` arms (`ACP-131`, `ACP-135`, `ACP-156`).
///
/// # `ACP-156`, decided: the re-read stays, and it must not be `std::fs`
///
/// The unit asks whether the end-of-tool re-read can be sourced from `EditDetails`
/// (`crates/cyrup-tools/src/details.rs`) instead of the filesystem, and says to **evaluate that
/// first**. It was evaluated and it does not work: `EditDetails` is `{diff, patch,
/// first_changed_line}` — a *unified diff*, not the file's new contents — and ACP's `Diff` needs
/// `new_text`, the whole post-image. Worse, `write` has **no `details` payload at all** (a comment
/// in `details.rs` records that pi declares `ToolDefinition<…, undefined>`), and `write` is half of
/// [`ToolClass::Mutation`]. So the structured diff cannot be manufactured from the tool result and
/// the re-read is load-bearing.
///
/// The consequence is the security half, and it is why this function returns a **request** rather
/// than reading: with `confine_to_cwd` set, `TraversalFs::read`
/// (`crates/cyrup-tools/src/isolation/traversal.rs`) hard-denies a path outside the confinement
/// root, and the session builder installs it. A `std::fs::read_to_string` here would read — and at
/// the `After` phase **transmit**, inside `Diff.new_text` — file contents the session's own backend
/// refuses to open, and for a non-local backend it would diff the host filesystem against a file
/// the tool wrote somewhere else entirely. **This function returning a path is not permission to
/// read it**: the shell's reader must be the session's configured `FsOps`, and
/// `AgentSessionServices` exposes no handle to one today. That accessor is the open interface
/// change this module files; until it lands, a shell that reaches for `std::fs` reintroduces
/// exactly the hole `ACP-156` names. When the read is refused, hand back
/// [`FileSnapshot::unreadable`] — `ACP-135`'s no-diff path — never [`FileSnapshot::absent`].
///
/// `ACP-Q24`, decided: **no**, the snapshot does not use ACP's `fs/readTextFile` client capability.
/// It would diff against the user's unsaved editor buffer, which is a *different file* from the one
/// the tool read and wrote through `FsOps`, so the diff would describe a change that never happened
/// on disk. Upstream ignores the capability and here that is the right answer rather than an
/// oversight. Recorded so it is not re-proposed as an obvious improvement.
///
/// # Why this takes the ledger
///
/// `AgentSessionEvent::ToolExecutionEnd` carries `{tool_call_id, tool_name, result, is_error}` and
/// **no `args`**, so the path for the `After` read exists only in the ledger, where the `Before`
/// read put it. That is a divergence from ADR-0028's sketch (`snapshot_needed(ev)`), forced by the
/// event's shape rather than chosen.
#[must_use]
pub fn snapshot_needed(ledger: &ToolCallLedger, ev: &AgentSessionEvent) -> Option<SnapshotRequest> {
    match ev {
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            if !ToolClass::of(tool_name).needs_snapshot() {
                return None;
            }
            Some(SnapshotRequest {
                tool_call_id: acp_id(tool_call_id),
                path: tool_path(args)?,
                phase: SnapshotPhase::Before,
            })
        }
        AgentSessionEvent::ToolExecutionEnd {
            tool_call_id,
            is_error,
            ..
        } => {
            // Upstream's re-read is inside `if (!isError && snapshot)`. A failed tool emits no
            // diff, so reading the file would be pure I/O for a value nothing consumes.
            if *is_error {
                return None;
            }
            let id = acp_id(tool_call_id);
            let snapshot = ledger.get(&id)?.snapshot()?;
            // An unreadable pre-image can produce no diff (`ACP-135`), so the re-read is dead too.
            if !snapshot.is_diffable() {
                return None;
            }
            Some(SnapshotRequest {
                tool_call_id: id,
                path: snapshot.path.clone(),
                phase: SnapshotPhase::After,
            })
        }
        _ => None,
    }
}

/// The pure core. Total over [`AgentSessionEvent`]; every arm is exhaustively matched.
///
/// Port of pi-acp v0.0.33 `session.ts`'s `handlePiEvent` switch, `emitBashToolCall` and
/// `emitBashOutputUpdate` (`ACP-127`, `ACP-128`, `ACP-129`, `ACP-131`, `ACP-134`, `ACP-135`,
/// `ACP-136`, `ACP-138`, `ACP-139`, `ACP-140`, `ACP-141`) — minus the switch's four
/// turn-progress arms, whose single producer is `crate::turn::status_updates`.
///
/// `snapshot` is the read [`snapshot_needed`] asked for, for this same event, or `None` when it
/// asked for none or the read was not performed. The core never touches the filesystem.
///
/// # The invariants this signature exists to make structural
///
/// * For each tool-call id the first update is `tool_call`, all later ones `tool_call_update` —
///   guaranteed by [`ToolCallLedger::announce`] being the only entry constructor.
/// * Status never regresses — guaranteed by [`ToolStatus`] having no backward transition and by
///   [`ToolCallLedger::update`] taking the maximum.
/// * The terminal `_meta` and terminal content accompany the announce **only** — guaranteed by
///   `announce` being their only producer.
/// * Bash output is a **delta** against what was already sent —
///   [`crate::ledger::TerminalAppender`].
/// * A diff is emitted only when a pre-mutation snapshot exists **and** the content actually
///   changed (`ACP-135`).
///
/// What it does **not** guarantee, stated plainly: the returned `Vec` can still be reordered,
/// dropped, or sent against the wrong `SessionId` by the shell, and nothing here proves the
/// *content* of a `Diff` is correct — only that it is emitted under the right precondition.
#[must_use]
pub fn translate(
    ledger: &mut ToolCallLedger,
    ev: &AgentSessionEvent,
    snapshot: Option<FileSnapshot>,
) -> Translated {
    match ev {
        // --- the turn's own shape (`ACP-121`, `ACP-154`, `ACP-160`) ------------------------------
        //
        // `AgentStart` sets upstream's `inAgentLoop`, which is assigned at five sites and read at
        // none; `ACP-160`'s deliverable is that the field does not exist. `TurnEnd` is upstream's
        // empty `case 'turn_end'` — pi uses it for sub-steps and will often start another turn.
        // `AgentEnd` carries `will_retry`, so the retry is visible here and is still not a settle:
        // a turn that auto-retries emits TWO `AgentEnd`s and exactly ONE `AgentSettled`.
        AgentSessionEvent::AgentStart
        | AgentSessionEvent::TurnStart
        | AgentSessionEvent::MessageStart { .. }
        | AgentSessionEvent::MessageEnd { .. }
        | AgentSessionEvent::TurnEnd { .. } => Translated::nothing(TurnSignal::Continue),

        // `ACP-022` — still not a settle, and now not silent either. The run's verdict is reported
        // so the shell can hold it until `AgentSettled`; see [`TurnSignal::RunEnded`] for why the
        // terminal `AssistantMessage` is the only place a mid-turn provider failure exists.
        AgentSessionEvent::AgentEnd { messages, .. } => {
            Translated::nothing(TurnSignal::RunEnded(RunTermination::of(messages)))
        }

        // `ACP-137` — bounded teardown. Upstream never clears these maps, so a tool call whose
        // `tool_execution_end` never arrives leaks for the life of the session. Clearing HERE
        // rather than in the shell means the settle arm and the teardown cannot drift apart.
        //
        // `ACP-122` — and the arm emits nothing, so there is no update the shell could leave
        // behind a response.
        AgentSessionEvent::AgentSettled => {
            ledger.clear();
            Translated::nothing(TurnSignal::Settled)
        }

        AgentSessionEvent::SessionReplaced { generation } => {
            // `ACP-154` — cyrup's third turn termination, which upstream has no analogue for. The
            // ledger belongs to the old runtime generation and every id in it is stale.
            ledger.clear();
            Translated::nothing(TurnSignal::Rebind {
                generation: *generation,
            })
        }

        // --- assistant text and tool-call surfacing (`ACP-127`, `ACP-128`) -----------------------
        AgentSessionEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => message_update(ledger, assistant_message_event),

        // --- the tool lifecycle (`ACP-131`, `ACP-134`, `ACP-135`, `ACP-139`…`ACP-141`) -----------
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => tool_execution_start(ledger, &acp_id(tool_call_id), tool_name, args, snapshot),

        AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => tool_execution_update(ledger, &acp_id(tool_call_id), partial_result),

        AgentSessionEvent::ToolExecutionEnd {
            tool_call_id,
            result,
            is_error,
            ..
        } => tool_execution_end(ledger, &acp_id(tool_call_id), result, *is_error, snapshot),

        // --- the turn's own progress: handled, but NOT by this module -------------------------
        //
        // `ACP-142` / `ACP-143` / `ACP-124`. These five arms are the only ones in upstream's
        // `handlePiEvent` switch that describe the *turn's* progress rather than a message or a
        // tool call, and `crate::turn::status_updates` is their **single producer** — the turn's
        // progress is `ACP-121`'s module's business, and two producers means the client renders
        // each chunk twice. `TurnSignal::Continue` rather than `TurnSignal::Ignored` because these
        // events are handled; they are simply handled somewhere else.
        // `crate::turn::tests::the_status_arms_have_exactly_one_producer` is the cross-check that
        // keeps this arm empty.
        AgentSessionEvent::AutoRetryStart { .. }
        | AgentSessionEvent::AutoRetryEnd { .. }
        | AgentSessionEvent::CompactionStart { .. }
        | AgentSessionEvent::CompactionEnd { .. }
        | AgentSessionEvent::QueueUpdate { .. } => Translated::nothing(TurnSignal::Continue),

        // --- deliberately dropped (`TurnSignal::Ignored`) ----------------------------------------
        //
        // Each of these is a cyrup super-set event with no port decided in THIS module, and each
        // says which unit owns it. `Ignored` rather than `Continue` so "we decided to drop this"
        // and "nothing happened" stay distinguishable, per the variant's own doc.
        //
        // * `ModelChanged` / `ThinkingLevelChanged` — `ACP-077`. These SHOULD become
        //   `config_option_update`s, and the gap analysis is explicit that the pump must be the
        //   single emitter so an extension-originated model switch reaches the client. Building
        //   that here needs `SessionConfigKnob::advertise` (`crate::config_options`), which is
        //   agent E's and is a skeleton; emitting a half-formed option list would be worse than
        //   emitting nothing. Filed in this module's report as a known gap with the exact shape.
        // * `SessionInfoChanged` — `ACP-285` / `ACP-Q20`, which is a **single-emitter** decision
        //   spanning `/name`, the config setters and this pump. `ACP-285`'s verify demands exactly
        //   one `session_info_update` carrying a fresh ISO-8601 `updatedAt`, and a pure function
        //   has no clock, so the translator cannot be that emitter without taking a timestamp
        //   argument. Left to `ACP-285`'s owner rather than pre-empted here.
        // * `SummarizationRetry*` / `BashExecutionUpdate` / `EntryAppended` / `SessionStart` /
        //   `SessionShutdown` — no upstream analogue at all; pi-acp's switch has no case and its
        //   `default: break` drops them. Kept as drops for parity.
        AgentSessionEvent::ModelChanged { .. }
        | AgentSessionEvent::ThinkingLevelChanged { .. }
        | AgentSessionEvent::SessionInfoChanged { .. }
        | AgentSessionEvent::SummarizationRetryScheduled { .. }
        | AgentSessionEvent::SummarizationRetryAttemptStart { .. }
        | AgentSessionEvent::SummarizationRetryFinished
        | AgentSessionEvent::BashExecutionUpdate { .. }
        | AgentSessionEvent::EntryAppended { .. }
        | AgentSessionEvent::SessionStart { .. }
        | AgentSessionEvent::SessionShutdown { .. } => Translated::nothing(TurnSignal::Ignored),
    }
}

// -------------------------------------------------------------------------------------------
// message_update
// -------------------------------------------------------------------------------------------

/// Port of pi-acp v0.0.33 `session.ts`'s `case 'message_update'` (`ACP-127`, `ACP-128`).
///
/// Upstream guards each delta on `typeof delta === 'string'`; the typed enum removes the guard
/// entirely. **Every other assistant-message event type produces nothing** — `text_start`,
/// `text_end`, `thinking_start`, `thinking_end`, `start`, `done` and `error` all fall through to a
/// bare `break` upstream and to the catch-all here.
fn message_update(ledger: &mut ToolCallLedger, ev: &StreamEvent) -> Translated {
    match ev {
        // `ContentChunk` also has an optional `message_id` the TS SDK lacked. Left `None` for
        // parity: upstream emits no `messageId`, and a client that groups by it would start
        // grouping cyrup's chunks differently from pi-acp's for no behavioural gain. Recorded here
        // as the option it is, per `ACP-127`.
        StreamEvent::TextDelta { delta, .. } => Translated::one(SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from(delta.clone())),
        )),
        StreamEvent::ThinkingDelta { delta, .. } => Translated::one(
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(delta.clone()))),
        ),

        // `ACP-128` — surface the tool call as soon as the model starts streaming it, so the client
        // can show a spinner while the arguments are still arriving.
        StreamEvent::ToolCallStart {
            content_index,
            partial,
        } => match partial.content.get(*content_index) {
            Some(Content::ToolCall(tc)) => surface_tool_call(ledger, &tc.id, &tc.name, None),
            _ => Translated::nothing(TurnSignal::Continue),
        },

        // `ACP-Q23`, decided: **do not emit on every delta**.
        //
        // [CYRUP-DELTA] — what differs: upstream emits a `tool_call_update` per `toolcall_delta`,
        // refreshing `rawInput` while the arguments stream. Here `ToolCallDelta` emits nothing and
        // `ToolCallEnd` emits one update carrying the complete arguments.
        //
        // What it costs: a client that renders a tool call's arguments *growing* sees them appear
        // in one step instead. Zed's final state is identical, and the announce still happens at
        // `ToolCallStart`, so the spinner — the actual point of surfacing early — is unaffected.
        //
        // Why: `ToolCall.arguments` is a `LazyArgs` whose whole purpose is that a snapshot nobody
        // reads costs nothing (`crates/cyrup-core/src/lazy_args.rs`, PERF-001). Each delta rebuilds
        // `partial` with a fresh `LazyArgs`, so reading it per delta re-runs
        // `parse_streaming_json_object` over the entire accumulated buffer — quadratic in the
        // argument size for a large `write`, on the event pump, for a value the client overwrites
        // microseconds later. Upstream pays this over a wire it does not own; in-process it is
        // simply waste.
        StreamEvent::ToolCallDelta { .. } => Translated::nothing(TurnSignal::Continue),

        // `ToolCallEnd` carries the finished `tool_call` **on the event**, which is upstream's
        // `ame.toolCall ??` half — the one the gap analysis records as having no analogue. It does
        // have one, on exactly this variant, and using it avoids materialising `partial`.
        StreamEvent::ToolCallEnd { tool_call, .. } => surface_tool_call(
            ledger,
            &tool_call.id,
            &tool_call.name,
            Some(Value::Object(tool_call.arguments.as_map().clone())),
        ),

        StreamEvent::Start { .. }
        | StreamEvent::TextStart { .. }
        | StreamEvent::TextEnd { .. }
        | StreamEvent::ThinkingStart { .. }
        | StreamEvent::ThinkingEnd { .. }
        | StreamEvent::Done { .. }
        | StreamEvent::Error { .. } => Translated::nothing(TurnSignal::Continue),
    }
}

/// The `tool_call` / `tool_call_update` decision for a streaming tool call (`ACP-128`, `ACP-129`).
///
/// Upstream: `if (!existingStatus) { … 'tool_call' … } else { … 'tool_call_update', keeping the
/// existing status … }`, with the bash case routed through `emitBashToolCall` and
/// `includeTerminal: !existingStatus`. All three decisions are the ledger's here.
///
/// An empty tool-call id emits nothing, exactly as upstream's `if (toolCallId)` guard does.
/// Upstream additionally defaults the name to the literal `'tool'`; `ToolCall.name` is a `String`
/// on a typed struct and is never absent, so that default is dead and is not written.
fn surface_tool_call(
    ledger: &mut ToolCallLedger,
    id: &cyrup_core::ToolCallId,
    tool_name: &str,
    raw_input: Option<Value>,
) -> Translated {
    if id.as_str().is_empty() {
        return Translated::nothing(TurnSignal::Continue);
    }
    let id = acp_id(id);
    let class = ToolClass::of(tool_name);
    let locations = locations_of(ledger, raw_input.as_ref(), None);

    if ledger.contains(&id) {
        // `ACP-129`: `ToolStatus::Pending` here is a *floor*, not an assignment — the ledger takes
        // the maximum, so a call already `in_progress` stays `in_progress`.
        Translated::maybe(ledger.update(
            &id,
            ToolStatus::Pending,
            UpdatePatch {
                title: raw_input.as_ref().and_then(|args| title_of(class, args)),
                locations: Some(locations),
                raw_input,
                ..UpdatePatch::default()
            },
        ))
    } else {
        let title = raw_input
            .as_ref()
            .and_then(|args| title_of(class, args))
            .unwrap_or_else(|| tool_name.to_string());
        Translated::one(ledger.announce(Announce {
            id,
            class,
            title,
            status: ToolStatus::Pending,
            locations,
            raw_input,
            snapshot: None,
        }))
    }
}

// -------------------------------------------------------------------------------------------
// the tool lifecycle
// -------------------------------------------------------------------------------------------

/// Port of pi-acp v0.0.33 `session.ts`'s `case 'tool_execution_start'` (`ACP-131`, `ACP-139`).
///
/// Upstream's `String((ev as any).toolCallId ?? crypto.randomUUID())` fallback is dead here:
/// `AgentSessionEvent::ToolExecutionStart` carries a typed `ToolCallId` that cannot be absent.
fn tool_execution_start(
    ledger: &mut ToolCallLedger,
    id: &ToolCallId,
    tool_name: &str,
    args: &Value,
    snapshot: Option<FileSnapshot>,
) -> Translated {
    let class = ToolClass::of(tool_name);

    // `ACP-131` — the pre-mutation snapshot, and the `edit`-only line inference it enables. The
    // read itself happened in the shell (`snapshot_needed`); all that is left is where to put it.
    //
    // Upstream restricts the line inference to `toolName === 'edit'`; `write` has no `oldText`
    // needles to search for, so `edit_old_texts` returns an empty list for it and the restriction
    // is expressed by the data rather than by a second name test.
    let line = snapshot
        .as_ref()
        .filter(|_| class.needs_snapshot())
        .and_then(|snapshot| snapshot.before.as_deref())
        .and_then(|before| {
            edit_old_texts(args)
                .iter()
                .find_map(|needle| find_unique_line_number(before, needle))
        });

    let locations = locations_of(ledger, Some(args), line);
    let title = title_of(class, args).unwrap_or_else(|| tool_name.to_string());

    if ledger.contains(id) {
        // Already surfaced while the model streamed it — just transition. `ACP-139`'s
        // `includeTerminal: !existingStatus` is structural: only `announce` emits the terminal.
        if let Some(snapshot) = snapshot.filter(|_| class.needs_snapshot()) {
            ledger.attach_snapshot(id, snapshot);
        }
        Translated::maybe(ledger.update(
            id,
            ToolStatus::InProgress,
            UpdatePatch {
                title: Some(title),
                locations: Some(locations),
                // A terminal's update carries no `rawInput` upstream either — `emitBashToolCall`
                // sends `title`/`kind`/`status`/`locations` and nothing else.
                raw_input: (!class.is_terminal()).then(|| args.clone()),
                ..UpdatePatch::default()
            },
        ))
    } else {
        Translated::one(ledger.announce(Announce {
            id: id.clone(),
            class,
            title,
            status: ToolStatus::InProgress,
            locations,
            raw_input: (!class.is_terminal()).then(|| args.clone()),
            snapshot,
        }))
    }
}

/// Port of pi-acp v0.0.33 `session.ts`'s `case 'tool_execution_update'` (`ACP-134`, `ACP-140`).
///
/// Upstream asks two `Set`s — `bashToolCallIds.has(id)` and `fileMutationToolCallIds.has(id)` —
/// which is how the two can disagree about one call. [`ToolCallLedger::class_of`] is the one
/// question with one answer. `fileMutationToolCallIds` is unnecessary for a second reason:
/// `tool_name` is on the Update and End events too, so the class never had to be remembered at all.
///
/// A file mutation's partial output is suppressed — both `content` and `rawOutput` — because an
/// `edit` streams the whole rewritten file and the client renders the structured diff at the end.
fn tool_execution_update(
    ledger: &mut ToolCallLedger,
    id: &ToolCallId,
    partial_result: &Value,
) -> Translated {
    let Some(class) = ledger.class_of(id) else {
        // Upstream emits an update for an id the client never saw; Zed drops it silently.
        return Translated::nothing(TurnSignal::Continue);
    };
    if class.is_terminal() {
        return Translated::maybe(ledger.terminal_progress(id, &bash_result_text(partial_result)));
    }
    let text = if class.needs_snapshot() {
        String::new()
    } else {
        tool_result_to_text(partial_result)
    };
    Translated::maybe(ledger.update(
        id,
        ToolStatus::InProgress,
        UpdatePatch {
            content: (!text.is_empty()).then(|| vec![text_content(&text)]),
            raw_output: (!class.needs_snapshot()).then(|| partial_result.clone()),
            ..UpdatePatch::default()
        },
    ))
}

/// Port of pi-acp v0.0.33 `session.ts`'s `case 'tool_execution_end'` (`ACP-135`, `ACP-136`,
/// `ACP-140`, `ACP-141`).
///
/// The three properties upstream's own tests pin, all preserved: no diff is synthesised at tool
/// *start* from the requested args (only this arm builds one); the diff reflects the **realized**
/// file contents, because `after` is a re-read rather than the tool's arguments; and `rawOutput` is
/// absent whenever a diff is present.
fn tool_execution_end(
    ledger: &mut ToolCallLedger,
    id: &ToolCallId,
    result: &Value,
    is_error: bool,
    after: Option<FileSnapshot>,
) -> Translated {
    let Some(class) = ledger.class_of(id) else {
        return Translated::nothing(TurnSignal::Continue);
    };

    if class.is_terminal() {
        let text = bash_result_text(result);
        let code = bash_exit_code(result, is_error);
        return Translated::maybe(ledger.terminal_finish(id, &text, is_error, code));
    }

    let text = tool_result_to_text(result);
    let mut content = None;

    // `ACP-135` — `is_error` is authoritative: cyrup's `edit` returns `Err` for a partial batch, so
    // a half-applied edit correctly emits no diff.
    if let (false, Some(before), Some(after)) = (
        is_error,
        ledger.get(id).and_then(|s| s.snapshot()).cloned(),
        after.as_ref().and_then(|s| s.before.as_deref()),
    ) {
        // `Unreadable` produces no diff at all — the divergence from upstream, which stores
        // `oldText: null` on a failed pre-read and then shows the whole file as new. `changed` is
        // upstream's `snapshot.oldText === null || newText !== snapshot.oldText`: an `Absent`
        // pre-image always differs, which is the write-to-a-new-file case.
        if before.is_diffable() && before.before.as_deref() != Some(after) {
            content = Some(vec![diff_content(
                &ledger.resolve(&before.path),
                before.before.as_deref(),
                after,
            )]);
        }
    }

    let has_diff = content.is_some();
    if content.is_none() && !text.is_empty() {
        content = Some(vec![text_content(&text)]);
    }

    Translated::maybe(ledger.finish(
        id,
        is_error,
        UpdatePatch {
            content,
            raw_output: (!has_diff).then(|| result.clone()),
            ..UpdatePatch::default()
        },
    ))
}

// -------------------------------------------------------------------------------------------
// the small pure helpers
// -------------------------------------------------------------------------------------------

/// The ACP `ToolCallId` for a cyrup one. Both are `Arc<str>` newtypes, so this is a refcount bump.
///
/// ADR-0028 §5 rejects a third wrapper: the two are already different types and will not unify, so
/// the conversion is the whole boundary.
fn acp_id(id: &cyrup_core::ToolCallId) -> ToolCallId {
    ToolCallId::new(id.0.clone())
}

/// `{type: "content", content: {type: "text", text}}`.
fn text_content(text: &str) -> ToolCallContent {
    ToolCallContent::from(text.to_string())
}

/// Port of pi-acp v0.0.33 `session.ts`'s `getToolPath` (`ACP-130`).
///
/// Upstream probes `path` then `file_path`. cyrup's own tools declare `path`
/// (`crates/cyrup-tools/src/tools/{edit,write,read}.rs`); `file_path` is kept because an MCP or
/// extension tool may use it and the probe costs nothing.
fn tool_path(args: &Value) -> Option<PathBuf> {
    for key in ["path", "file_path"] {
        if let Some(p) = args.get(key).and_then(Value::as_str) {
            return Some(PathBuf::from(p));
        }
    }
    None
}

/// Port of pi-acp v0.0.33 `session.ts`'s `toToolCallLocations` (`ACP-130`).
///
/// A missing path yields an empty list, which the ACP builder omits — upstream returns `undefined`
/// for the same case. `line: None` leaves the `line` key out entirely (`ToolCallLocation` is
/// `#[skip_serializing_none]`), so a client cannot read a `null` line as line zero.
fn locations_of(
    ledger: &ToolCallLedger,
    args: Option<&Value>,
    line: Option<u32>,
) -> Vec<ToolCallLocation> {
    let Some(path) = args.and_then(tool_path) else {
        return Vec::new();
    };
    let location = ToolCallLocation::new(ledger.resolve(&path));
    vec![match line {
        Some(line) => location.line(line),
        None => location,
    }]
}

/// The tool-call title (`ACP-138`).
///
/// Port of pi-acp v0.0.33 `translate/bash.ts`'s `bashCommand` and the `title: bashCommand(args) ??
/// toolName` expression in `emitBashToolCall`. Returns `None` when there is no better title than
/// the tool's own name.
///
/// # [CYRUP-DELTA] — one key, not twelve
///
/// **What differs.** `bashCommand` probes twelve key paths (`command`, `cmd`, and both of those
/// under `args`, `input`, `rawInput`, `toolInput` and `details`) because it reads a
/// `Record<string, unknown>` that crossed an NDJSON wire whose shape it does not control.
/// `AgentSessionEvent::ToolExecutionStart { args, .. }` is cyrup's own tool's argument object, and
/// both shell tools declare exactly one required property, `command`
/// (`crates/cyrup-tools/src/tools/bash.rs`'s `parameters`, shared by `ShellTool::powershell`).
///
/// **What it costs.** A *foreign* tool that happens to be named `bash` and nests its command under
/// `input.command` gets its tool name as the title instead of the command. It also gets
/// `ToolKind::Execute` and a terminal, so the row is still correct — only its label is generic.
///
/// The `trim()` guard is upstream's and is kept: a whitespace-only command is not a title. The
/// value returned is the **untrimmed** original, also upstream's.
fn title_of(class: ToolClass, args: &Value) -> Option<String> {
    if !class.is_terminal() {
        return None;
    }
    args.get("command")
        .and_then(Value::as_str)
        .filter(|c| !c.trim().is_empty())
        .map(str::to_string)
}

/// Port of pi-acp v0.0.33 `session.ts`'s `findUniqueLineNumber` (`ACP-132`).
///
/// A needle that appears exactly once yields its 1-based line; empty, absent, or appearing twice
/// yields `None` — **emit no location line at all rather than guess one**, which is the property
/// that keeps a client's follow-along cursor off an unrelated line.
///
/// Upstream counts `charCodeAt(i) === 10` over UTF-16 code units; counting `b'\n'` over UTF-8 bytes
/// gives the same answer, because a `\n` byte cannot occur inside a multi-byte UTF-8 sequence.
fn find_unique_line_number(text: &str, needle: &str) -> Option<u32> {
    if needle.is_empty() {
        return None;
    }
    let first = text.find(needle)?;
    let after = first.checked_add(needle.len())?;
    if text.get(after..).is_some_and(|rest| rest.contains(needle)) {
        return None;
    }
    let newlines = text
        .as_bytes()
        .get(..first)
        .map_or(0, |head| head.iter().filter(|b| **b == b'\n').count());
    u32::try_from(newlines).ok()?.checked_add(1)
}

/// Port of pi-acp v0.0.33 `session.ts`'s `getParsedEdits` + `getEditOldTexts` (`ACP-133`).
///
/// cyrup's `edit` accepts the same three shapes pi does, and for the same reason: `prepare_arguments`
/// (`crates/cyrup-tools/src/tools/edit.rs`) coerces a stringified `edits`, wraps a single-edit
/// object, and appends the legacy top-level `{oldText, newText}` pair. Reading all three here keeps
/// the line inference working on every shape the tool itself accepts.
///
/// **The needle order is pinned by `every_edit_shape_yields_its_needles_in_a_pinned_order`**, which
/// is `ACP-133`'s whole point: a later refactor that flips the order silently changes which line
/// the client's cursor lands on. The order is upstream's — the legacy top-level `oldText` first,
/// then the `edits` array in order — with duplicates removed, first occurrence winning.
fn edit_old_texts(args: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if !out.iter().any(|existing| existing == s) {
            out.push(s.to_string());
        }
    };

    if let Some(old) = args.get("oldText").and_then(Value::as_str) {
        push(old);
    }

    // `edits` may be an array, or the JSON *string* of one — pi normalizes stringified edits and so
    // does cyrup's own `prepare_arguments`, so a stringified batch must still yield its needles.
    let owned;
    let edits = match args.get("edits") {
        Some(Value::Array(items)) => Some(items.as_slice()),
        Some(Value::String(raw)) => {
            owned = serde_json::from_str::<Value>(raw).ok();
            match owned.as_ref() {
                Some(Value::Array(items)) => Some(items.as_slice()),
                _ => None,
            }
        }
        _ => None,
    };
    for edit in edits.unwrap_or_default() {
        if let Some(old) = edit.get("oldText").and_then(Value::as_str) {
            push(old);
        }
    }
    out
}

/// Port of pi-acp v0.0.33 `translate/pi-tools.ts`'s `toolResultToText` (`ACP-136`).
///
/// The ladder, in order:
///
/// 1. `details.diff`, when a non-blank string. cyrup's `edit` returns a terse success line in
///    `content` and the full unified diff in `EditDetails.diff`
///    (`crates/cyrup-tools/src/details.rs`), exactly as pi's does, so the diff wins.
/// 2. The `text` of every `{"type":"text"}` content block, joined with **no separator**.
/// 4. `serde_json::to_string_pretty`, which is `JSON.stringify(result, null, 2)`.
///
/// # [CYRUP-DELTA] — steps 3 and 4's fallback are cut
///
/// **What differs.** Step 3 upstream is a stdout/stderr/exitCode ladder assembled as `stdout`,
/// `stderr:\n…`, `exit code: n` and joined with `\n\n`. It is **dead against every cyrup built-in**:
/// the result shape is `result_value_of`'s `{content, details?, usage?, addedToolNames?,
/// terminate?}` (`crates/cyrup-agent/src/agent/message.rs`), which is the one place that object is
/// built, and no cyrup tool puts `stdout`/`stderr`/`exitCode` at either level. Upstream's own
/// `JSON.stringify` fallback (`String(result)` on a circular-reference throw) is cut too:
/// `serde_json::to_string_pretty` on a `Value` cannot fail.
///
/// **What it costs.** It is `ACP-141`'s consequence in another place: with step 3 gone there is no
/// second route by which an exit code could reach the client as text. There did not have to be —
/// the bash tool's own error body already ends `Command exited with code {n}` and arrives through
/// step 2 — but an MCP server that returns a bare `{stdout, exitCode}` with no content blocks now
/// renders as pretty-printed JSON where upstream would have rendered the stdout. That is a
/// legible fallback, not a lost one.
///
/// A `Value::Null` result yields the empty string, which is upstream's falsy guard.
#[must_use]
pub fn tool_result_to_text(result: &Value) -> String {
    if result.is_null() {
        return String::new();
    }

    if let Some(diff) = result
        .get("details")
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .filter(|d| !d.trim().is_empty())
    {
        return diff.to_string();
    }

    let joined = joined_text_blocks(result);
    if !joined.is_empty() {
        return joined;
    }

    serde_json::to_string_pretty(result).unwrap_or_default()
}

/// The `text` of every `{"type":"text"}` content block, joined with **no separator**.
///
/// Upstream writes this twice — step 2 of `translate/pi-tools.ts`'s `toolResultToText` and the
/// first half of `translate/bash.ts`'s `bashResultText` — with the same `.filter(Boolean)` and the
/// same empty-join guard. One function, because two copies of a joiner is how they drift.
fn joined_text_blocks(result: &Value) -> String {
    let Some(blocks) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect()
}

/// Port of pi-acp v0.0.33 `translate/bash.ts`'s `bashResultText` (`ACP-140`).
///
/// **Not** [`tool_result_to_text`], and the difference is load-bearing rather than cosmetic:
/// `bashResultText` has **no JSON fallback**. A command that produced no output yields the empty
/// string and therefore an empty [`crate::ledger::Push`] and no `terminal_output` at all, whereas
/// `toolResultToText` would fall through to `JSON.stringify(result, null, 2)` and append the
/// serialized *result envelope* into the user's terminal pane. Upstream keeps the two functions
/// apart for exactly this reason and so does this port.
///
/// # [CYRUP-DELTA] — the stdout/stderr probe is cut
///
/// **What differs.** Upstream falls back to `details.stdout ?? stdout ?? details.output ?? output`
/// joined with `stderr`. cyrup's shell tool returns its combined stdout+stderr as content blocks
/// (`build_stream_update`, `crates/cyrup-tools/src/tools/bash.rs`) and its `BashDetails` is
/// `{truncation?, fullOutputPath?}` — there is no key for the probe to find.
///
/// **What it costs.** A foreign tool named `bash` that returned `{stdout: "…"}` with no content
/// blocks would render an empty terminal. It would also have to have been classified
/// [`ToolClass::Terminal`] by name alone to get here at all.
fn bash_result_text(result: &Value) -> String {
    joined_text_blocks(result)
}

/// Port of pi-acp v0.0.33 `translate/bash.ts`'s `bashExitCode` (`ACP-141`).
///
/// Upstream's four probe keys, in upstream's order, then upstream's `isError ? 1 : 0` fallback.
///
/// # `ACP-141`'s three options, and which one landed
///
/// The unit offers (a) faithful-and-broken, (b) parse the trailing `Command exited with code {n}`
/// out of the error text, and (c) add `exit_code: Option<i32>` to `BashDetails` and populate it in
/// the shell tool. It recommends (c) and (c) is what is implemented.
///
/// (b) stays refused: it would turn a human-readable diagnostic into an API, so a copy-edit of
/// that sentence becomes a wire regression.
///
/// (c) needed one thing the unit did not name. A non-zero exit is an `Err(ToolError)` from
/// `crates/cyrup-tools/src/tools/bash.rs`, and `ToolError` carried only a message — so the failing
/// path had no `details` object for a new field to ride on, and `Executed::from`
/// (`crates/cyrup-agent/src/agent/run/tools/finalize.rs`) built pi's
/// `createErrorToolResult` shape, `{content:[text], details:{}}`. `ToolError::details` closes
/// that: it is `None` for every tool that does not opt in — so `{}` is still what a throwing tool
/// produces, exactly as upstream — and the bash tool's non-zero-exit arm now sets
/// `BashDetails { exit_code: Some(code), .. }`. `details.exitCode` is the first key this probes,
/// so `sh -c 'exit 42'` reports 42.
///
/// # [CYRUP-DELTA] — three of the four probe keys are unreachable, and are kept anyway
///
/// **What differs.** `exitCode` at the top level, and `code` at either level, are shapes no cyrup
/// tool produces: upstream probes four because it reads a `Record<string, unknown>` off an NDJSON
/// wire it does not control, and `details.exitCode` is the only one cyrup can populate.
///
/// **What it costs.** Three dead branches. They are kept because an MCP server's tool result
/// reaches this same function through [`ToolClass::of`]'s default arm, and a server that reports
/// `{"exitCode": 42}` is not hypothetical — dropping the probes would silently start reporting 1
/// for it. This is the one surviving piece of upstream's key-probing, and the reason is written
/// here so it is not mistaken for one the in-process design forgot to delete.
///
/// The remaining honest gap is the two arms with no exit code to report: `ExitStatus::TimedOut`
/// and `ExitStatus::Killed` fall to the `isError` fallback and report 1. A timeout has no exit
/// code, so 1 is a summary rather than a wrong number, and the sentence
/// (`Command timed out after N seconds`) still reaches the user as `terminal_output.data`.
#[must_use]
pub fn bash_exit_code(result: &Value, is_error: bool) -> i32 {
    let details = result.get("details");
    for probe in [
        details.and_then(|d| d.get("exitCode")),
        result.get("exitCode"),
        details.and_then(|d| d.get("code")),
        result.get("code"),
    ] {
        if let Some(code) = probe
            .and_then(Value::as_i64)
            .and_then(|c| i32::try_from(c).ok())
        {
            return code;
        }
    }
    i32::from(is_error)
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
    use crate::ids::AbsCwd;
    use crate::ledger::Push;
    use cyrup_core::{AssistantMessage, ProviderId, StopReason, ToolCall};
    use cyrup_session::compaction::CompactionReason;
    use serde_json::json;
    use std::sync::Arc;

    fn fresh() -> ToolCallLedger {
        ToolCallLedger::new(AbsCwd::parse("/work").expect("absolute"))
    }

    fn as_json(update: &SessionUpdate) -> Value {
        serde_json::to_value(update).expect("SessionUpdate serializes")
    }

    fn json_all(t: &Translated) -> Vec<Value> {
        t.updates.iter().map(as_json).collect()
    }

    fn tool_call(id: &str, name: &str, args: Value) -> ToolCall {
        let Value::Object(map) = args else {
            panic!("tool arguments are an object");
        };
        ToolCall {
            id: cyrup_core::ToolCallId::from(id),
            name: name.to_string(),
            arguments: map.into(),
            thought_signature: None,
        }
    }

    /// A minimal `AssistantMessage`. `errored` is the only public constructor
    /// (`crates/cyrup-core/src/message/assistant.rs`); the fields these tests read are set after.
    fn assistant(content: Vec<Content>) -> Arc<AssistantMessage> {
        let mut msg =
            AssistantMessage::errored(ProviderId::from("faux"), "m", None, StopReason::Pending, "");
        msg.content = content;
        Arc::new(msg)
    }

    fn partial_with(call: ToolCall) -> Arc<AssistantMessage> {
        assistant(vec![Content::ToolCall(call)])
    }

    fn user_message() -> cyrup_session_svc::AgentMessage {
        cyrup_session_svc::AgentMessage::User {
            content: Vec::new(),
            timestamp: None,
        }
    }

    /// A terminal assistant message, built through the same `AssistantMessage::errored`
    /// constructor `ProviderError::into_error_message` uses (`ACP-022`).
    fn terminal_assistant(
        stop_reason: cyrup_core::StopReason,
        error_message: Option<&str>,
    ) -> cyrup_session_svc::AgentMessage {
        let mut message = cyrup_core::AssistantMessage::errored(
            cyrup_core::ProviderId::from("anthropic"),
            "claude-test",
            None,
            stop_reason,
            error_message.unwrap_or_default(),
        );
        message.error_message = error_message.map(str::to_string);
        cyrup_session_svc::AgentMessage::Assistant(Arc::new(message))
    }

    fn agent_end_ending_in(
        stop_reason: cyrup_core::StopReason,
        error_message: Option<&str>,
    ) -> AgentSessionEvent {
        AgentSessionEvent::AgentEnd {
            messages: vec![Arc::new(terminal_assistant(stop_reason, error_message))],
            will_retry: false,
        }
    }

    fn text_delta(delta: &str) -> AgentSessionEvent {
        AgentSessionEvent::MessageUpdate {
            message: user_message(),
            assistant_message_event: Box::new(StreamEvent::TextDelta {
                content_index: 0,
                delta: delta.to_string(),
                partial: assistant(Vec::new()),
            }),
        }
    }

    fn stream(ev: StreamEvent) -> AgentSessionEvent {
        AgentSessionEvent::MessageUpdate {
            message: user_message(),
            assistant_message_event: Box::new(ev),
        }
    }

    /// A cyrup tool result, in the shape `result_value_of` builds
    /// (`crates/cyrup-agent/src/agent/message.rs`).
    fn tool_result(text: &str) -> Value {
        json!({ "content": [{ "type": "text", "text": text }] })
    }

    /// ACP-121 / ACP-154 — the two signals the whole shell is written against. Settling on
    /// `AgentEnd` returns `stopReason: end_turn` while a retried run is still streaming; rebinding
    /// on `SessionReplaced` is what keeps the pending request from hanging.
    #[test]
    fn agent_settled_settles_and_session_replaced_rebinds() {
        let mut ledger = fresh();

        let settled = AgentSessionEvent::AgentSettled;
        assert!(matches!(
            translate(&mut ledger, &settled, None).turn,
            TurnSignal::Settled
        ));

        let replaced = AgentSessionEvent::SessionReplaced { generation: 7 };
        assert!(matches!(
            translate(&mut ledger, &replaced, None).turn,
            TurnSignal::Rebind { generation: 7 }
        ));
    }

    /// ACP-121 — the settle point is `AgentSettled` and **nothing else**. A turn that auto-retries
    /// emits two `AgentEnd`s and one `AgentSettled`; this drives upstream's own component scenario
    /// through the translator and asserts nothing before the settle claims to end the turn.
    #[test]
    fn only_agent_settled_settles_across_a_retrying_run() {
        let mut ledger = fresh();
        let retrying = [
            AgentSessionEvent::AgentStart,
            AgentSessionEvent::AutoRetryStart {
                attempt: 1,
                max_attempts: 4,
                delay_ms: 1500,
                error_message: "overloaded".into(),
            },
            AgentSessionEvent::AgentEnd {
                messages: Vec::new(),
                will_retry: true,
            },
            AgentSessionEvent::AgentStart,
            AgentSessionEvent::TurnEnd {
                message: user_message(),
                tool_results: Vec::new(),
            },
            AgentSessionEvent::AgentEnd {
                messages: Vec::new(),
                will_retry: false,
            },
        ];
        for ev in &retrying {
            assert!(
                !matches!(translate(&mut ledger, ev, None).turn, TurnSignal::Settled),
                "{} must not settle the turn",
                ev.kind()
            );
        }
        assert!(matches!(
            translate(&mut ledger, &AgentSessionEvent::AgentSettled, None).turn,
            TurnSignal::Settled
        ));
    }

    /// **ACP-022.** `AgentEnd` reports the run's verdict, and the terminal `AssistantMessage` is
    /// the only place a mid-turn provider failure exists at all.
    ///
    /// Before this unit the arm was `Translated::nothing(TurnSignal::Continue)` and the two
    /// fields it now reads were discarded, which is what made every provider failure reach the
    /// editor as a successful, empty `end_turn`.
    #[test]
    fn an_agent_end_reports_how_its_run_terminated() {
        use cyrup_core::StopReason as CoreStop;

        let terminated = |stop: CoreStop, error: Option<&str>| -> RunTermination {
            let mut ledger = fresh();
            let Translated { updates, turn } =
                translate(&mut ledger, &agent_end_ending_in(stop, error), None);
            assert!(
                updates.is_empty(),
                "ACP-122 — the arm still emits nothing; the verdict rides the signal"
            );
            match turn {
                TurnSignal::RunEnded(t) => t,
                _ => panic!("AgentEnd must report a termination, not {stop:?} silently"),
            }
        };

        assert_eq!(terminated(CoreStop::Stop, None), RunTermination::Completed);
        assert_eq!(
            terminated(CoreStop::ToolUse, None),
            RunTermination::Completed
        );
        assert_eq!(
            terminated(CoreStop::Deferred, None),
            RunTermination::Completed,
            "a deferred turn is a SUCCESS terminal — the provider took the request"
        );
        assert_eq!(
            terminated(CoreStop::Length, None),
            RunTermination::MaxTokens
        );
        assert_eq!(terminated(CoreStop::Aborted, None), RunTermination::Aborted);
        assert_eq!(
            terminated(CoreStop::Error, Some("http 500: upstream exploded")),
            RunTermination::Failed(AcpFailure::Internal {
                message: "http 500: upstream exploded".into()
            })
        );
        assert_eq!(
            terminated(CoreStop::Error, Some("http 401: invalid x-api-key")),
            RunTermination::Failed(AcpFailure::AuthRequired {
                detail: "http 401: invalid x-api-key".into()
            })
        );

        // A run with no assistant turn at all — an `input` extension handler that fully serviced
        // the submission. Not a failure; inventing one would be worse than the `end_turn` this
        // whole unit removes.
        let mut ledger = fresh();
        assert_eq!(
            match translate(
                &mut ledger,
                &AgentSessionEvent::AgentEnd {
                    messages: Vec::new(),
                    will_retry: false,
                },
                None,
            )
            .turn
            {
                TurnSignal::RunEnded(t) => t,
                other => panic!("expected RunEnded, got {other:?}"),
            },
            RunTermination::Completed
        );
    }

    /// ACP-022 — the verdict is the LAST assistant turn's, not the first.
    ///
    /// A run whose provider failed and then recovered inside the same `agent_end` (a tool loop
    /// whose first call 500'd) must not be reported as failed.
    #[test]
    fn the_verdict_is_the_terminal_assistant_turn_not_an_earlier_one() {
        let mut ledger = fresh();
        let messages = vec![
            Arc::new(terminal_assistant(
                cyrup_core::StopReason::Error,
                Some("http 500: transient"),
            )),
            Arc::new(terminal_assistant(cyrup_core::StopReason::Stop, None)),
        ];
        let signal = translate(
            &mut ledger,
            &AgentSessionEvent::AgentEnd {
                messages,
                will_retry: false,
            },
            None,
        )
        .turn;
        assert_eq!(
            match signal {
                TurnSignal::RunEnded(t) => t,
                other => panic!("expected RunEnded, got {other:?}"),
            },
            RunTermination::Completed
        );
    }

    /// ACP-122 — the settle arm emits nothing, so the shell's "send every update, then act on the
    /// signal" loop has nothing that could be stranded behind the response frame. This is the
    /// structural half of the deleted `lastEmit` barrier; the wire half is a `cyrup-it` test.
    #[test]
    fn the_settle_arm_has_nothing_left_to_send() {
        let mut ledger = fresh();
        // A tool call still open at settle — the case that would strand an update if the settle
        // arm emitted one.
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "t1".into(),
                tool_name: "grep".into(),
                args: json!({ "pattern": "x" }),
            },
            None,
        );
        assert_eq!(ledger.len(), 1);

        let settled = translate(&mut ledger, &AgentSessionEvent::AgentSettled, None);
        assert!(settled.updates.is_empty());
        // ACP-137: and the ledger is bounded at the same point.
        assert!(ledger.is_empty(), "the settle is also the teardown");
    }

    /// ACP-127 — exactly one notification per text/thinking delta, and **zero** for every other
    /// assistant-message event type.
    #[test]
    fn only_text_and_thinking_deltas_become_chunks() {
        let mut ledger = fresh();
        let partial = assistant(Vec::new());

        let text = translate(&mut ledger, &text_delta("hel"), None);
        assert_eq!(json_all(&text).len(), 1);
        assert_eq!(json_all(&text)[0]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(json_all(&text)[0]["content"]["type"], "text");
        assert_eq!(json_all(&text)[0]["content"]["text"], "hel");

        let thinking = translate(
            &mut ledger,
            &stream(StreamEvent::ThinkingDelta {
                content_index: 0,
                delta: "hmm".into(),
                partial: Arc::clone(&partial),
            }),
            None,
        );
        assert_eq!(json_all(&thinking).len(), 1);
        assert_eq!(
            json_all(&thinking)[0]["sessionUpdate"],
            "agent_thought_chunk"
        );
        assert_eq!(json_all(&thinking)[0]["content"]["text"], "hmm");
        assert!(
            json_all(&thinking)[0].get("messageId").is_none(),
            "no messageId, for parity with the TS SDK that lacked the field"
        );

        let silent = [
            StreamEvent::Start {
                partial: Arc::clone(&partial),
            },
            StreamEvent::TextStart {
                content_index: 0,
                partial: Arc::clone(&partial),
            },
            StreamEvent::TextEnd {
                content_index: 0,
                content: "hel".into(),
                partial: Arc::clone(&partial),
            },
            StreamEvent::ThinkingStart {
                content_index: 0,
                partial: Arc::clone(&partial),
            },
            StreamEvent::ThinkingEnd {
                content_index: 0,
                content: "hmm".into(),
                partial: Arc::clone(&partial),
            },
        ];
        for ev in silent {
            assert!(
                translate(&mut ledger, &stream(ev), None).updates.is_empty(),
                "every other assistant-message event produces nothing"
            );
        }
    }

    /// ACP-128 / ACP-129 — one `tool_call` then `tool_call_update`s for one id, all `pending`, and
    /// a second `tool_call` for a known id is never produced. Also `ACP-Q23`: the delta itself is
    /// silent and the end carries the complete arguments.
    #[test]
    fn a_streamed_tool_call_is_announced_once_and_then_updated() {
        let mut ledger = fresh();
        let start = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallStart {
                content_index: 0,
                partial: partial_with(tool_call("tc1", "read", json!({}))),
            }),
            None,
        );
        let start = json_all(&start);
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["sessionUpdate"], "tool_call");
        assert_eq!(start[0]["toolCallId"], "tc1");
        assert_eq!(start[0]["title"], "read");
        assert_eq!(start[0]["kind"], "read");
        // `ToolCall.status` is `#[serde(skip_serializing_if = "ToolCallStatus::is_default")]` in the
        // schema crate, so a `pending` announce omits the key — semantically identical to
        // upstream's explicit `status: 'pending'`, and the schema's choice rather than this port's.
        assert_eq!(
            start[0]
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending"),
            "pending"
        );

        // ACP-Q23: a delta emits nothing.
        let delta = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"pa".into(),
                partial: partial_with(tool_call("tc1", "read", json!({}))),
            }),
            None,
        );
        assert!(delta.updates.is_empty());

        let end = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallEnd {
                content_index: 0,
                tool_call: tool_call("tc1", "read", json!({ "path": "src/a.rs" })),
                partial: partial_with(tool_call("tc1", "read", json!({ "path": "src/a.rs" }))),
            }),
            None,
        );
        let end = json_all(&end);
        assert_eq!(end.len(), 1);
        assert_eq!(
            end[0]["sessionUpdate"], "tool_call_update",
            "a second tool_call for a known id re-renders the row"
        );
        assert_eq!(
            end[0]["status"], "pending",
            "still pending: nothing ran yet"
        );
        assert_eq!(end[0]["rawInput"]["path"], "src/a.rs");
        assert_eq!(end[0]["locations"][0]["path"], "/work/src/a.rs");
        assert!(
            end[0]["locations"][0].get("line").is_none(),
            "line: None serialises with no line key: {}",
            end[0]
        );
    }

    /// ACP-128 — a partially-streamed argument buffer materialises as a **real partial object**,
    /// never upstream's `{partialArgs: s}` wrapper, because `LazyArgs` recovers a truncated buffer
    /// through `parse_streaming_json_object`. The wrapper is unreachable and is not written.
    #[test]
    fn a_truncated_argument_buffer_is_a_partial_object_not_a_wrapper() {
        let mut ledger = fresh();
        let truncated = ToolCall {
            id: cyrup_core::ToolCallId::from("tc1"),
            name: "read".to_string(),
            arguments: cyrup_core::LazyArgs::streaming("{\"path\": \"/et".into()),
            thought_signature: None,
        };
        let out = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallEnd {
                content_index: 0,
                tool_call: truncated.clone(),
                partial: partial_with(truncated),
            }),
            None,
        );
        let out = json_all(&out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["rawInput"]["path"], "/et");
        assert!(
            out[0]["rawInput"].get("partialArgs").is_none(),
            "upstream's JSON.parse-failure wrapper has no input to defend against here: {}",
            out[0]
        );
    }

    /// ACP-128 — an empty tool-call id emits nothing, exactly as upstream's `if (toolCallId)`
    /// guard does.
    #[test]
    fn an_empty_tool_call_id_emits_nothing() {
        let mut ledger = fresh();
        let out = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallStart {
                content_index: 0,
                partial: partial_with(tool_call("", "read", json!({}))),
            }),
            None,
        );
        assert!(out.updates.is_empty());
        assert!(ledger.is_empty());
    }

    /// ACP-129's other half at the translator: a late `ToolCallEnd` arriving after
    /// `ToolExecutionStart` must **not** drag the row back to `pending`.
    #[test]
    fn a_late_delta_after_execution_start_does_not_downgrade_the_row() {
        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallStart {
                content_index: 0,
                partial: partial_with(tool_call("tc1", "read", json!({}))),
            }),
            None,
        );
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "tc1".into(),
                tool_name: "read".into(),
                args: json!({ "path": "a.rs" }),
            },
            None,
        );
        let late = translate(
            &mut ledger,
            &stream(StreamEvent::ToolCallEnd {
                content_index: 0,
                tool_call: tool_call("tc1", "read", json!({ "path": "a.rs" })),
                partial: partial_with(tool_call("tc1", "read", json!({ "path": "a.rs" }))),
            }),
            None,
        );
        assert_eq!(json_all(&late)[0]["status"], "in_progress");
    }

    /// ACP-131 — `tool_call` is emitted once at `in_progress` when no stream delta preceded it.
    #[test]
    fn an_unannounced_execution_start_announces_at_in_progress() {
        let mut ledger = fresh();
        let out = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "g1".into(),
                tool_name: "grep".into(),
                args: json!({ "pattern": "fn", "path": "src" }),
            },
            None,
        );
        let out = json_all(&out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "tool_call");
        assert_eq!(out[0]["status"], "in_progress");
        assert_eq!(out[0]["kind"], "search");
        assert_eq!(out[0]["title"], "grep");
        assert_eq!(out[0]["locations"][0]["path"], "/work/src");
    }

    /// ACP-131 / ACP-156 — `snapshot_needed` asks for the pre-read for `edit`/`write` and for
    /// nothing else, and it is asked BEFORE the read, so the shell never reads a file the core did
    /// not ask for.
    #[test]
    fn snapshot_needed_asks_only_for_file_mutations() {
        let mut ledger = fresh();
        assert!(snapshot_needed(&ledger, &AgentSessionEvent::AgentSettled).is_none());
        assert!(
            snapshot_needed(
                &ledger,
                &AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "g1".into(),
                    tool_name: "grep".into(),
                    args: json!({ "path": "src" }),
                }
            )
            .is_none(),
            "a search reads files but never diffs them"
        );
        assert!(
            snapshot_needed(
                &ledger,
                &AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "e1".into(),
                    tool_name: "edit".into(),
                    args: json!({ "edits": [] }),
                }
            )
            .is_none(),
            "no path, no read"
        );

        let req = snapshot_needed(
            &ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                args: json!({ "path": "a.rs", "edits": [] }),
            },
        )
        .expect("edit with a path");
        assert_eq!(req.tool_call_id, ToolCallId::new("e1"));
        assert_eq!(req.path, PathBuf::from("a.rs"));
        assert_eq!(req.phase, SnapshotPhase::Before);

        // The `After` read needs the ledger, because `ToolExecutionEnd` carries no `args`.
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                args: json!({ "path": "a.rs", "edits": [] }),
            },
            Some(FileSnapshot::read("a.rs", "old\n")),
        );
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "e1".into(),
            tool_name: "edit".into(),
            result: tool_result("ok"),
            is_error: false,
        };
        let req = snapshot_needed(&ledger, &end).expect("the pre-read recorded the path");
        assert_eq!(req.path, PathBuf::from("a.rs"));
        assert_eq!(req.phase, SnapshotPhase::After);
    }

    /// ACP-135 / ACP-156 — a failed tool and an unreadable pre-image both cancel the re-read, so
    /// the shell performs no I/O whose result nothing consumes.
    #[test]
    fn a_failed_tool_or_an_unreadable_pre_image_cancels_the_re_read() {
        let mut ledger = fresh();
        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "e1".into(),
            tool_name: "edit".into(),
            args: json!({ "path": "a.rs" }),
        };

        let _ = translate(&mut ledger, &start, Some(FileSnapshot::unreadable("a.rs")));
        assert!(
            snapshot_needed(
                &ledger,
                &AgentSessionEvent::ToolExecutionEnd {
                    tool_call_id: "e1".into(),
                    tool_name: "edit".into(),
                    result: tool_result("ok"),
                    is_error: false,
                }
            )
            .is_none(),
            "an unreadable pre-image can produce no diff, so the re-read is dead"
        );

        let mut ledger = fresh();
        let _ = translate(&mut ledger, &start, Some(FileSnapshot::read("a.rs", "old")));
        assert!(
            snapshot_needed(
                &ledger,
                &AgentSessionEvent::ToolExecutionEnd {
                    tool_call_id: "e1".into(),
                    tool_name: "edit".into(),
                    result: tool_result("boom"),
                    is_error: true,
                }
            )
            .is_none(),
            "a failed tool emits no diff"
        );
    }

    /// ACP-131 — the `edit` line inference: the first `oldText` needle that occurs exactly once in
    /// the pre-image sets the location's line.
    #[test]
    fn an_edit_locates_itself_by_its_unique_old_text() {
        let mut ledger = fresh();
        let out = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                args: json!({
                    "path": "a.rs",
                    "edits": [{ "oldText": "needle", "newText": "thread" }],
                }),
            },
            Some(FileSnapshot::read("a.rs", "one\ntwo\nneedle\nfour\n")),
        );
        let out = json_all(&out);
        assert_eq!(out[0]["kind"], "edit");
        assert_eq!(out[0]["locations"][0]["path"], "/work/a.rs");
        assert_eq!(out[0]["locations"][0]["line"], 3);

        // A `write` has no needles, so it gets a location with no line — upstream restricts the
        // inference to `edit` by name; here the data does it.
        let mut ledger = fresh();
        let out = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "w1".into(),
                tool_name: "write".into(),
                args: json!({ "path": "a.rs", "content": "hi" }),
            },
            Some(FileSnapshot::read("a.rs", "one\ntwo\n")),
        );
        assert!(json_all(&out)[0]["locations"][0].get("line").is_none());
    }

    /// ACP-132 — the line-inference table: unique at line 3, twice, empty, absent.
    #[test]
    fn find_unique_line_number_is_a_four_row_table() {
        let text = "a\nb\nneedle\nd\n";
        assert_eq!(find_unique_line_number(text, "needle"), Some(3));
        assert_eq!(
            find_unique_line_number("x\nneedle\nneedle\n", "needle"),
            None
        );
        assert_eq!(find_unique_line_number(text, ""), None);
        assert_eq!(find_unique_line_number(text, "absent"), None);
        // First line is 1, not 0.
        assert_eq!(find_unique_line_number("needle\nb\n", "needle"), Some(1));
        // Overlapping occurrences count as two — upstream searches from `first + needle.length`,
        // so `aaa` contains `aa` once by its reckoning, and this pins that reading.
        assert_eq!(find_unique_line_number("aaa", "aa"), Some(1));
        assert_eq!(find_unique_line_number("aaaa", "aa"), None);
        // A multi-byte prefix does not shift the line count.
        assert_eq!(find_unique_line_number("é\nneedle\n", "needle"), Some(2));
    }

    /// ACP-133 — the needle order is pinned, so a later refactor cannot silently flip which line
    /// the location points at. All three shapes cyrup's `edit` accepts are covered.
    #[test]
    fn every_edit_shape_yields_its_needles_in_a_pinned_order() {
        // Legacy top-level pair.
        assert_eq!(
            edit_old_texts(&json!({ "oldText": "a", "newText": "b" })),
            vec!["a".to_string()]
        );
        // Current batch schema.
        assert_eq!(
            edit_old_texts(&json!({
                "edits": [{ "oldText": "x", "newText": "1" }, { "oldText": "y", "newText": "2" }]
            })),
            vec!["x".to_string(), "y".to_string()]
        );
        // Both, with the legacy pair FIRST and duplicates removed.
        assert_eq!(
            edit_old_texts(&json!({
                "oldText": "legacy",
                "newText": "n",
                "edits": [{ "oldText": "x", "newText": "1" }, { "oldText": "legacy", "newText": "2" }]
            })),
            vec!["legacy".to_string(), "x".to_string()]
        );
        // A stringified `edits`, which `prepare_arguments` also coerces.
        assert_eq!(
            edit_old_texts(&json!({ "edits": "[{\"oldText\":\"s\",\"newText\":\"t\"}]" })),
            vec!["s".to_string()]
        );
        // Unparseable, and absent.
        assert!(edit_old_texts(&json!({ "edits": "not json" })).is_empty());
        assert!(edit_old_texts(&json!({ "path": "a.rs" })).is_empty());
    }

    /// ACP-134 — an `edit` update produces neither `content` nor `rawOutput`; a `grep` one
    /// produces both.
    #[test]
    fn a_file_mutations_partial_output_is_suppressed_and_a_searchs_is_not() {
        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                args: json!({ "path": "a.rs" }),
            },
            Some(FileSnapshot::read("a.rs", "old")),
        );
        let edit = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                args: json!({ "path": "a.rs" }),
                partial_result: tool_result("half the file"),
            },
            None,
        );
        let edit = json_all(&edit);
        assert_eq!(edit[0]["status"], "in_progress");
        assert!(edit[0].get("content").is_none(), "{}", edit[0]);
        assert!(edit[0].get("rawOutput").is_none(), "{}", edit[0]);

        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "g1".into(),
                tool_name: "grep".into(),
                args: json!({ "pattern": "fn" }),
            },
            None,
        );
        let grep = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: "g1".into(),
                tool_name: "grep".into(),
                args: json!({ "pattern": "fn" }),
                partial_result: tool_result("a.rs:1"),
            },
            None,
        );
        let grep = json_all(&grep);
        assert_eq!(grep[0]["content"][0]["content"]["text"], "a.rs:1");
        assert_eq!(grep[0]["rawOutput"]["content"][0]["text"], "a.rs:1");
    }

    /// ACP-135's three cases: write-to-a-new-file yields `old_text: None`; an edit whose pre-read
    /// failed yields **no diff at all** (the divergence from upstream); and a diff-bearing update
    /// carries no `rawOutput`.
    #[test]
    fn the_structured_diff_has_three_cases_and_suppresses_raw_output() {
        // (1) new file.
        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "w1".into(),
                tool_name: "write".into(),
                args: json!({ "path": "new.rs" }),
            },
            Some(FileSnapshot::absent("new.rs")),
        );
        let out = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: "w1".into(),
                tool_name: "write".into(),
                result: tool_result("wrote new.rs"),
                is_error: false,
            },
            Some(FileSnapshot::read("new.rs", "fresh\n")),
        );
        let out = json_all(&out);
        assert_eq!(out[0]["status"], "completed");
        assert_eq!(out[0]["content"][0]["type"], "diff");
        assert_eq!(out[0]["content"][0]["path"], "/work/new.rs");
        assert_eq!(out[0]["content"][0]["newText"], "fresh\n");
        assert!(
            out[0]["content"][0].get("oldText").is_none(),
            "a new file's old_text is None: {}",
            out[0]
        );
        assert!(
            out[0].get("rawOutput").is_none(),
            "rawOutput is absent whenever a diff is present: {}",
            out[0]
        );

        // (2) the pre-read failed. Upstream treats this as "this is a new file" and ships a diff
        // claiming the whole file is new; here it produces no diff.
        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                args: json!({ "path": "a.rs" }),
            },
            Some(FileSnapshot::unreadable("a.rs")),
        );
        let out = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: "e1".into(),
                tool_name: "edit".into(),
                result: tool_result("edited"),
                is_error: false,
            },
            Some(FileSnapshot::read("a.rs", "after\n")),
        );
        let out = json_all(&out);
        assert_ne!(out[0]["content"][0]["type"], "diff");
        assert_eq!(out[0]["content"][0]["content"]["text"], "edited");
        assert!(
            out[0].get("rawOutput").is_some(),
            "with no diff, rawOutput rides along: {}",
            out[0]
        );

        // (3) a real edit: old_text present, rawOutput suppressed.
        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "e2".into(),
                tool_name: "edit".into(),
                args: json!({ "path": "a.rs" }),
            },
            Some(FileSnapshot::read("a.rs", "before\n")),
        );
        let out = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: "e2".into(),
                tool_name: "edit".into(),
                result: tool_result("edited"),
                is_error: false,
            },
            Some(FileSnapshot::read("a.rs", "after\n")),
        );
        let out = json_all(&out);
        assert_eq!(out[0]["content"][0]["oldText"], "before\n");
        assert_eq!(out[0]["content"][0]["newText"], "after\n");
        assert!(out[0].get("rawOutput").is_none());
    }

    /// ACP-135 — an unchanged file emits no diff, and a failed tool emits none either.
    #[test]
    fn an_unchanged_file_and_a_failed_tool_both_emit_no_diff() {
        for (is_error, after) in [(false, "same\n"), (true, "changed\n")] {
            let mut ledger = fresh();
            let _ = translate(
                &mut ledger,
                &AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "e1".into(),
                    tool_name: "edit".into(),
                    args: json!({ "path": "a.rs" }),
                },
                Some(FileSnapshot::read("a.rs", "same\n")),
            );
            let out = translate(
                &mut ledger,
                &AgentSessionEvent::ToolExecutionEnd {
                    tool_call_id: "e1".into(),
                    tool_name: "edit".into(),
                    result: tool_result("body"),
                    is_error,
                },
                Some(FileSnapshot::read("a.rs", after)),
            );
            let out = json_all(&out);
            assert_ne!(out[0]["content"][0]["type"], "diff", "{out:?}");
            assert_eq!(
                out[0]["status"],
                if is_error { "failed" } else { "completed" }
            );
        }
    }

    /// ACP-138 / ACP-139 / ACP-157 — the bash tool-call title, the terminal `_meta`, and the same
    /// treatment for `powershell`.
    #[test]
    fn a_shell_tool_call_is_a_terminal_titled_by_its_command() {
        for name in ["bash", "powershell"] {
            let mut ledger = fresh();
            let out = translate(
                &mut ledger,
                &AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "sh1".into(),
                    tool_name: name.into(),
                    args: json!({ "command": "ls -la" }),
                },
                None,
            );
            let out = json_all(&out);
            assert_eq!(out[0]["title"], "ls -la", "{name}");
            assert_eq!(out[0]["kind"], "execute", "{name}");
            assert_eq!(out[0]["status"], "in_progress", "{name}");
            assert_eq!(out[0]["content"][0]["terminalId"], "sh1", "{name}");
            assert_eq!(
                out[0]["_meta"]["terminal_info"]["terminal_id"], "sh1",
                "{name}"
            );
            assert!(
                out[0].get("rawInput").is_none(),
                "a terminal's row carries no rawInput: {}",
                out[0]
            );
        }
    }

    /// ACP-138 — a blank or missing command falls back to the tool name, and the title is the
    /// **untrimmed** original when it is not blank.
    #[test]
    fn a_blank_command_falls_back_to_the_tool_name() {
        for (args, expected) in [
            (json!({ "command": "ls" }), "ls"),
            (json!({ "command": "   " }), "bash"),
            (json!({}), "bash"),
            (json!({ "command": " ls " }), " ls "),
        ] {
            let mut ledger = fresh();
            let out = translate(
                &mut ledger,
                &AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "sh1".into(),
                    tool_name: "bash".into(),
                    args,
                },
                None,
            );
            assert_eq!(json_all(&out)[0]["title"], expected);
        }
    }

    /// ACP-140 / ACP-141 — the terminal lifecycle end to end: append-only deltas while running,
    /// then the exit `_meta`, with no `content` and no `rawOutput` anywhere.
    #[test]
    fn a_terminal_streams_deltas_and_then_exits() {
        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "sh1".into(),
                tool_name: "bash".into(),
                args: json!({ "command": "seq 3" }),
            },
            None,
        );

        let first = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: "sh1".into(),
                tool_name: "bash".into(),
                args: json!({ "command": "seq 3" }),
                partial_result: tool_result("1\n"),
            },
            None,
        );
        assert_eq!(
            json_all(&first)[0]["_meta"]["terminal_output"]["data"],
            "1\n"
        );

        let second = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: "sh1".into(),
                tool_name: "bash".into(),
                args: json!({ "command": "seq 3" }),
                partial_result: tool_result("1\n2\n"),
            },
            None,
        );
        assert_eq!(
            json_all(&second)[0]["_meta"]["terminal_output"]["data"],
            "2\n",
            "only the suffix"
        );

        let end = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: "sh1".into(),
                tool_name: "bash".into(),
                result: tool_result("1\n2\n3\n"),
                is_error: false,
            },
            None,
        );
        let end = json_all(&end);
        assert_eq!(end[0]["status"], "completed");
        assert_eq!(end[0]["_meta"]["terminal_output"]["data"], "3\n");
        assert_eq!(end[0]["_meta"]["terminal_exit"]["exit_code"], 0);
        assert!(end[0]["_meta"]["terminal_exit"]["signal"].is_null());
        assert!(end[0].get("content").is_none(), "{}", end[0]);
        assert!(end[0].get("rawOutput").is_none(), "{}", end[0]);
        assert!(ledger.is_empty(), "ACP-137: the terminal was cleaned up");
    }

    /// ACP-141 — the probe, and the fallback that makes `sh -c 'exit 42'` report `1` **today**.
    /// The `details.exitCode` row is the one that goes green the day `BashDetails` grows the field.
    #[test]
    fn the_exit_code_probe_is_upstreams_and_its_fallback_is_the_known_gap() {
        // The `sh -c 'exit 42'` shape, as it now reaches this function: `ToolError::details`
        // carries `BashDetails { exit_code: Some(42), .. }` through `Executed::from` into
        // `ToolResult.details`, and `details.exitCode` is the first key probed. This is the
        // assertion the gap analysis says to write knowing it fails — it passes now, and it fails
        // again the moment either `BashDetails::exit_code` or `ToolError::details` is dropped.
        let failed_command = json!({
            "content": [{ "type": "text", "text": "boom\n\nCommand exited with code 42" }],
            "details": { "exitCode": 42 },
        });
        assert_eq!(bash_exit_code(&failed_command, true), 42);

        // The fallback, which is still reached by the two arms with no exit code to report
        // (`ExitStatus::TimedOut` and `ExitStatus::Killed`) and by any tool that reports none.
        assert_eq!(bash_exit_code(&tool_result("boom"), true), 1);
        assert_eq!(bash_exit_code(&tool_result("fine"), false), 0);
        // The other three probe keys: unreachable from a cyrup built-in, kept for MCP results.
        assert_eq!(
            bash_exit_code(&json!({ "details": { "exitCode": 42 } }), true),
            42
        );
        assert_eq!(bash_exit_code(&json!({ "exitCode": 7 }), false), 7);
        assert_eq!(
            bash_exit_code(&json!({ "details": { "code": 3 } }), true),
            3
        );
        assert_eq!(bash_exit_code(&json!({ "code": 9 }), true), 9);
        // A non-numeric or out-of-range code falls back rather than truncating.
        assert_eq!(bash_exit_code(&json!({ "exitCode": "42" }), true), 1);
        assert_eq!(
            bash_exit_code(&json!({ "exitCode": i64::from(i32::MAX) + 1 }), false),
            0
        );
    }

    /// ACP-140 — the sliding window seen from the translator, which is the seam the real bash tool
    /// feeds. A preview whose head `truncate_tail` dropped resyncs on the overlap and appends only
    /// the genuinely new bytes; only a preview sharing no overlap at all appends nothing.
    #[test]
    fn a_slid_preview_appends_the_new_bytes_and_a_discontinuity_appends_nothing() {
        // A corpus line long enough that one line of overlap clears `MIN_RESYNC_OVERLAP`, as the
        // real 50 KiB / 2 000-line window does by four orders of magnitude.
        let line = |n: usize| format!("line {n:04}: Compiling cyrup-acp v0.0.0 (/home/user)\n");
        let window = |upto: usize| -> String { (upto.saturating_sub(3)..upto).map(line).collect() };

        let mut ledger = fresh();
        let _ = translate(
            &mut ledger,
            &AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "sh1".into(),
                tool_name: "bash".into(),
                args: json!({ "command": "cargo build" }),
            },
            None,
        );
        let preview = |ledger: &mut ToolCallLedger, text: &str| {
            translate(
                ledger,
                &AgentSessionEvent::ToolExecutionUpdate {
                    tool_call_id: "sh1".into(),
                    tool_name: "bash".into(),
                    args: json!({ "command": "cargo build" }),
                    partial_result: tool_result(text),
                },
                None,
            )
        };

        // Fill the window, then slide it. Before ACP-140's fix the pane froze here for good.
        let _ = preview(&mut ledger, &window(3));
        let out = preview(&mut ledger, &window(5));
        assert_eq!(
            json_all(&out)[0]["_meta"]["terminal_output"]["data"],
            [line(3), line(4)].concat(),
            "the two lines past the overlap, and only those: {:?}",
            json_all(&out)
        );

        // A jump larger than one whole window has no overlap to anchor on: the amount of missing
        // output is unknown, so nothing is appended rather than the tail being repeated.
        let out = preview(&mut ledger, "unrelated output\nfrom far ahead\n");
        assert!(
            out.updates.is_empty(),
            "a genuinely discontinuous preview appends nothing and has no status transition left \
             to report, so there is no frame to send at all — upstream would re-append the whole \
             preview here: {:?}",
            json_all(&out)
        );
        // And it re-based, so the very next update is a clean suffix.
        let out = preview(&mut ledger, "unrelated output\nfrom far ahead\nand on\n");
        assert_eq!(
            json_all(&out)[0]["_meta"]["terminal_output"]["data"],
            "and on\n"
        );
        // The unit tests on the appender itself pin the outcome names.
        assert_eq!(
            crate::ledger::TerminalAppender::default().push("x"),
            Push::Append("x".into())
        );
    }

    /// ACP-136 — the ladder: `details.diff` wins, then the joined text blocks, then pretty JSON.
    #[test]
    fn the_tool_result_text_ladder_has_three_live_rungs() {
        // 1. `details.diff` beats the content blocks, which is why an `edit`'s terse success line
        //    is not what the user reads.
        assert_eq!(
            tool_result_to_text(&json!({
                "content": [{ "type": "text", "text": "Edited a.rs" }],
                "details": { "diff": "@@ -1 +1 @@\n-a\n+b\n" }
            })),
            "@@ -1 +1 @@\n-a\n+b\n"
        );
        // A blank diff does not win.
        assert_eq!(
            tool_result_to_text(&json!({
                "content": [{ "type": "text", "text": "Edited" }],
                "details": { "diff": "   " }
            })),
            "Edited"
        );
        // 2. Text blocks join with NO separator, and non-text blocks are skipped.
        assert_eq!(
            tool_result_to_text(&json!({
                "content": [
                    { "type": "text", "text": "a" },
                    { "type": "image", "data": "…", "mimeType": "image/png" },
                    { "type": "text", "text": "b" }
                ]
            })),
            "ab"
        );
        // 4. A result with no text content — an MCP tool returning only `details` — is pretty JSON.
        let fallback = tool_result_to_text(&json!({ "details": { "rows": 3 } }));
        assert!(fallback.contains("\"rows\": 3"), "{fallback}");
        assert!(fallback.contains('\n'), "pretty, not compact: {fallback}");
        // The falsy guard.
        assert_eq!(tool_result_to_text(&Value::Null), "");
    }

    /// ACP-142 / ACP-143 / ACP-124 — this module emits **nothing** for the turn's own progress.
    /// `crate::turn::status_updates` is the single producer of those five arms and owns the
    /// byte-exact strings; two producers would render each chunk twice at the client. The
    /// companion assertion lives beside the producer, as
    /// `crate::turn::tests::the_status_arms_have_exactly_one_producer`.
    #[test]
    fn the_turns_own_progress_is_not_this_modules_to_emit() {
        let mut ledger = fresh();
        let progress = [
            AgentSessionEvent::AutoRetryStart {
                attempt: 1,
                max_attempts: 4,
                delay_ms: 1500,
                error_message: "overloaded".into(),
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
                follow_up: Vec::new(),
            },
        ];
        for ev in &progress {
            let out = translate(&mut ledger, ev, None);
            assert!(
                out.updates.is_empty(),
                "{} has one producer, and it is not this one",
                ev.kind()
            );
            assert!(
                matches!(out.turn, TurnSignal::Continue),
                "{} is handled elsewhere, not dropped",
                ev.kind()
            );
        }
    }

    /// Every event this translator deliberately drops is named `Ignored`, not `Continue`, so "we
    /// decided to drop this" and "nothing happened" stay distinguishable — which is what makes the
    /// known gaps (`ACP-077`, `ACP-124`, `ACP-285`) auditable rather than invisible.
    #[test]
    fn the_deliberate_drops_are_named_rather_than_folded_into_continue() {
        let mut ledger = fresh();
        let dropped = [
            AgentSessionEvent::ModelChanged {
                provider: "anthropic".into(),
                model: "claude".into(),
            },
            AgentSessionEvent::ThinkingLevelChanged {
                level: "high".into(),
            },
            AgentSessionEvent::SessionInfoChanged {
                name: Some("my session".into()),
            },
            AgentSessionEvent::SummarizationRetryFinished,
            AgentSessionEvent::BashExecutionUpdate {
                id: None,
                delta: "x".into(),
            },
            AgentSessionEvent::EntryAppended { entry: Value::Null },
            AgentSessionEvent::SessionStart {
                reason: "new".into(),
                previous_session_file: None,
            },
            AgentSessionEvent::SessionShutdown {
                reason: "quit".into(),
            },
        ];
        for ev in &dropped {
            let out = translate(&mut ledger, ev, None);
            assert!(out.updates.is_empty(), "{} emits nothing", ev.kind());
            assert!(
                matches!(out.turn, TurnSignal::Ignored),
                "{} is a deliberate drop, not a handled no-op",
                ev.kind()
            );
        }

        // …and the handled-but-silent events are `Continue`, not `Ignored`.
        for ev in [AgentSessionEvent::AgentStart, AgentSessionEvent::TurnStart] {
            assert!(matches!(
                translate(&mut ledger, &ev, None).turn,
                TurnSignal::Continue
            ));
        }
    }

    /// ADR-0028 F2's guarantee, asserted where it is actually enforceable: across a full tool
    /// lifecycle the first emission for an id is `tool_call` and every later one is
    /// `tool_call_update`.
    #[test]
    fn a_known_id_is_never_re_announced_by_the_translator() {
        let mut ledger = fresh();
        let lifecycle = [
            stream(StreamEvent::ToolCallStart {
                content_index: 0,
                partial: partial_with(tool_call("t1", "grep", json!({}))),
            }),
            stream(StreamEvent::ToolCallEnd {
                content_index: 0,
                tool_call: tool_call("t1", "grep", json!({ "pattern": "x" })),
                partial: partial_with(tool_call("t1", "grep", json!({ "pattern": "x" }))),
            }),
            AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "t1".into(),
                tool_name: "grep".into(),
                args: json!({ "pattern": "x" }),
            },
            AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: "t1".into(),
                tool_name: "grep".into(),
                args: json!({ "pattern": "x" }),
                partial_result: tool_result("a.rs:1"),
            },
            AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: "t1".into(),
                tool_name: "grep".into(),
                result: tool_result("a.rs:1"),
                is_error: false,
            },
        ];
        let mut kinds = Vec::new();
        for ev in &lifecycle {
            for update in json_all(&translate(&mut ledger, ev, None)) {
                kinds.push(
                    update["sessionUpdate"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        assert_eq!(
            kinds,
            vec![
                "tool_call",
                "tool_call_update",
                "tool_call_update",
                "tool_call_update",
                "tool_call_update"
            ]
        );
        assert!(ledger.is_empty(), "the end closed the row");
    }
}
