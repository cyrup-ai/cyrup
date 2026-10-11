//! Given one already-spawned child's stdout NDJSON event stream, fold it into
//! progress/output/acceptance state until the attempt settles — [`drive_attempt`] and its
//! [`crate::exec::drive_attempt::DriveOutcome`]. Split out of `exec/mod.rs`'s own "SubagentSpawner" section (the per-attempt
//! drive-loop third of it).

use std::time::Duration;

use cyrup_core::CancelToken;

use crate::exec::agent_config::RunOptions;
use crate::exec::child_transcript::ChildTranscriptWriter;
use crate::exec::ndjson::SubagentEvent;
use crate::exec::output::{is_terminal_assistant_stop, message_end_has_error_message};
use crate::exec::progress::AgentProgress;
use crate::spawn::SpawnedChild;
use crate::watchdog::child_status::{
    ChildWatchdogConfig, ChildWatchdogIdentity, ChildWatchdogPhase, ChildWatchdogStateSnapshot,
    ChildWatchdogStatusEvent, accept_child_watchdog_event, child_watchdog_is_active,
    is_child_watchdog_status_event,
};

/// The runtime facts [`crate::exec::fallback::AttemptRunner::run_attempt`]'s exit-0 re-diagnosis (pi
/// `execution.ts:747-790`, T3 group A) needs from [`drive_attempt`] beyond the raw exit status.
pub(crate) struct DriveOutcome {
    /// The orchestrator's own wall-clock deadline expired (R-SA-036) — terminates the ladder.
    pub(crate) timed_out: bool,
    /// `RunOptions.interrupt` fired (pi's soft interrupt, `execution.ts:722-745`) — the paused-
    /// success path, distinct from a timeout or a hard cancel.
    pub(crate) interrupted: bool,
    /// The child emitted a terminal assistant stop but held its stdout open past the final-stop
    /// grace window (or closed stdout yet lingered past `FINAL_DRAIN_TIMEOUT`), so it had to be
    /// force-drained via the real signal ladder — pi's `forcedTerminationSignal`
    /// (`execution.ts:356-362` @v0.34.0).
    pub(crate) forced_termination: bool,
    /// At least one terminal assistant stop observed on this attempt carried no `errorMessage` —
    /// pi's `cleanTerminalAssistantStopReceived` (`execution.ts:557`), the other half of
    /// `forcedDrainAfterFinalSuccess`.
    pub(crate) clean_terminal_stop: bool,
    /// The child emitted `agent_settled` — pi's `agentSettledReceived` (`execution.ts:843`). The
    /// SECOND half of pi's `forcedDrainAfterFinalSuccess` witness (`:1080`:
    /// `(cleanTerminalAssistantStopReceived || agentSettledReceived)`), and the event that arms the
    /// final-stop grace window for a child whose last assistant message was not a clean terminal
    /// stop.
    pub(crate) agent_settled: bool,
    /// The child blew past the per-line stdout cap and the line was not a projectable aggregate
    /// (pi `failProtocol`, `execution.ts:1026-1041`). Set only on that path; when set, the child was
    /// force-terminated through the signal ladder and this diagnostic becomes the attempt's error,
    /// ahead of every other diagnosis.
    pub(crate) protocol_error: Option<crate::exec::child_protocol::ProtocolOutputLimit>,
    /// SUBA-119 — the model-verification failure this attempt observed, if any: the child's assistant
    /// `message_end` reported a model that is not the launch candidate and is not an accepted alias of
    /// it. Folded onto the attempt's error by `attempt_runner`, the way upstream assigns
    /// `result.error` at `execution.ts:1102`.
    pub(crate) model_verification_error: Option<String>,
    /// The child's real exit status once confirmed gone, or a genuine `wait()`/read I/O fault.
    pub(crate) exit_status: std::io::Result<Option<std::process::ExitStatus>>,
    /// R-SA-037: the child's NDJSON stream showed a BLOCKING `contact_supervisor` supervisor-clarify
    /// ask (`need_decision`/`interview`), so the drive loop fired
    /// [`crate::tui::intercom::spawn_clarify`] and this attempt is marked detached (its outcome
    /// bypasses acceptance/completion-guard/truncation, and the fallback ladder does not advance past
    /// it). `false` when no such ask was observed.
    pub(crate) detached: bool,
    /// SUBA-008 — the run's turn-budget latch as it stood when this attempt ended: pi's
    /// `result.turnBudget` / `result.turnBudgetExceeded` / `result.wrapUpRequested` trio
    /// (`execution.ts:1087`/`:1251-1258`), carried out of the drive loop as one value.
    ///
    /// Unarmed (`TurnBudgetTracker::is_armed() == false`) for every run that declared no budget,
    /// which is every run today that does not pass one — so this field changes nothing on those
    /// paths.
    pub(crate) turn_budget: crate::exec::turn_budget::TurnBudgetTracker,
    /// UW-3 — the parent's folded view of an ARMED child's watchdog as the attempt ended: pi's
    /// `result.watchdog` (`execution.ts:563-567` @v0.43.0). `None` for an unarmed child and for an
    /// armed one that never reported.
    pub(crate) watchdog: Option<ChildWatchdogStateSnapshot>,
    /// CFG-067 — a per-tool-call deadline expired and the child was signalled down for it (pi
    /// `terminateForToolTimeout`, `execution.ts:1248-1263`). Upstream assigns the SAME string to
    /// `result.error` and `result.finalOutput` and sets `result.timedOut`, so this rides out beside
    /// [`Self::timed_out`] rather than replacing it.
    pub(crate) tool_timeout_error: Option<String>,
    /// SUBA-118 — pi `result.afterCompactionSettlement` (`execution.ts:1418-1420`, which stamps it
    /// onto the attempt result under the `AFTER_COMPACTION_SETTLEMENT` symbol): the child settled
    /// its drain WHILE a compaction was outstanding. Read by
    /// [`crate::exec::attempt_runner::SpawnedChildAttemptRunner`]'s abort-recovery plan as
    /// `after_compaction_settlement` (`abort-recovery.ts:81`), which is the marker that makes a
    /// compaction-induced abort resumable at all and a terminal-drain cleanup signal
    /// non-authoritative (`:109`).
    pub(crate) after_compaction_settlement: bool,
    /// SUBA-118 — pi `result.toolBudgetBlocked` (`execution.ts:1150`): a tool result this attempt
    /// observed was the run's own hard-block message, so the child was stopped by its budget rather
    /// than by anything resumable. Read by the abort-recovery plan's `tool_budget_exhausted` rung
    /// (`abort-recovery.ts:111`).
    pub(crate) tool_budget_blocked: bool,
    /// SUBA-195 — the text still streaming when this attempt ended abnormally, and why it ended
    /// (pi `partialCause` + `partialOutput.text()`, `src/runs/foreground/execution.ts` and
    /// `src/runs/background/run-child-session.ts` @ad11b7ab). `None` when the attempt ended
    /// normally, or abnormally with no unfinished text newer than its last completed reply.
    /// Computed once, at settle, as upstream extracts it once ("text is extracted once, when a
    /// child ends abnormally").
    pub(crate) streamed_partial: Option<(crate::exec::partial_output::PartialOutputCause, String)>,
}

/// The final-stop grace window (pi `FINAL_STOP_GRACE_MS`, `execution.ts:333`): once a terminal
/// assistant stop is observed, a child that has not exited (released its stdout) within this window
/// is force-drained via [`SpawnedChild::terminate`]'s real SIGINT->SIGTERM->SIGKILL ladder rather
/// than the parent blocking indefinitely on a child that emitted its final answer but never
/// exited. pi's subsequent `HARD_KILL_MS`(3000) SIGKILL step is subsumed by `terminate`'s own
/// SIGTERM->SIGKILL escalation, which this crate routes every forced termination through.
const FINAL_STOP_GRACE_MS: u64 = 1000;

/// SUBA-S06: how long to keep draining stdout after the child process itself has been reaped while
/// its stdout is still held open by a surviving grandchild.
///
/// This is NOT [`FINAL_STOP_GRACE_MS`]'s job and must not be folded into it. That window is armed
/// by a *protocol* event (a terminal assistant stop) and expiring it means force-draining a live
/// process through the signal ladder. This one is armed by an *OS* event (the direct child is
/// already gone, so there is nothing left to signal) and expiring it simply ends the read loop, so
/// the ordinary post-loop path can report the exit status it already has. They coincide at 1000ms
/// today only because both are "give buffered output a beat to arrive".
const POST_EXIT_DRAIN_MS: u64 = 1000;

/// R-SA-037: does `event` show a child BLOCKING on a `contact_supervisor` supervisor-clarify ask,
/// and if so, what is the human-facing prompt? A blocking ask is `contact_supervisor`'s
/// `need_decision`/`interview` reason (the intercom `ask_and_wait` shapes,
/// `contact_supervisor.rs:81-101`) — NOT the fire-and-forget `progress_update`, which never blocks.
/// The prompt is the ask's `message` (empty string if the child omitted it). No new NDJSON wire
/// variant is needed: a blocking ask surfaces as an ordinary `ToolExecutionStart` for the
/// `contact_supervisor` tool, which this reuses (per `AttemptSignal::detached`'s own recipe).
fn contact_supervisor_block_prompt(event: &crate::exec::ndjson::SubagentEvent) -> Option<String> {
    if let crate::exec::ndjson::SubagentEvent::ToolExecutionStart {
        tool_name, args, ..
    } = event
        && tool_name == "contact_supervisor"
    {
        let reason = args
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if matches!(reason, "need_decision" | "interview") {
            return Some(
                args.get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    None
}

/// SUBA-008 — is this event an ASSISTANT `message_end`? pi's `evt.type === "message_end" &&
/// evt.message && evt.message.role === "assistant"` (`execution.ts:910-912`), which is the ONLY
/// shape that increments a turn.
fn is_assistant_message_end(event: &SubagentEvent) -> bool {
    let SubagentEvent::MessageEnd { message } = event else {
        return false;
    };
    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
}

/// SUBA-008 — every `toolCall` content part of a `message_end`, pi's `toolCalls` filter
/// (`execution.ts:915-918`). Returns an empty slice for any other event shape.
fn message_end_tool_calls(event: &SubagentEvent) -> Vec<&serde_json::Value> {
    let SubagentEvent::MessageEnd { message } = event else {
        return Vec::new();
    };
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| {
                    part.get("type").and_then(serde_json::Value::as_str) == Some("toolCall")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// SUBA-119 — the `model` an ASSISTANT `message_end` reports, when it carries a non-empty one. pi's
/// `evt.message.model` under its `if (evt.message.model)` guard (`execution.ts:1096`), which in JS is
/// falsy for both an absent field and an empty string.
///
/// The role check is upstream's too: `execution.ts:1096` sits inside the assistant-`message_end` arm,
/// and `exec/child_transcript.rs:374` already reads this same field off an assistant message, so the
/// wire is known to carry it.
fn assistant_message_end_model(event: &SubagentEvent) -> Option<&str> {
    if !is_assistant_message_end(event) {
        return None;
    }
    let SubagentEvent::MessageEnd { message } = event else {
        return None;
    };
    let model = message.get("model").and_then(serde_json::Value::as_str)?;
    (!model.is_empty()).then_some(model)
}

/// SUBA-008 — pi's `hasToolCall` (`execution.ts:919`).
fn message_end_has_tool_call(event: &SubagentEvent) -> bool {
    !message_end_tool_calls(event).is_empty()
}

/// SUBA-008 — pi's `terminalStructuredOutputCall` (`execution.ts:921-923`), minus the
/// `Boolean(options.structuredOutput)` half its caller applies: EXACTLY one tool call, and it is
/// `structured_output`.
///
/// This is the second way a turn counts as terminal. Without it, a child that answers by calling
/// the structured-output tool — the normal ending for a `outputSchema` run, where `stopReason` is
/// `toolUse`, not `stop` — would be treated as still working and could be aborted at the exact
/// moment it delivered its answer.
fn is_sole_structured_output_tool_call(event: &SubagentEvent) -> bool {
    let calls = message_end_tool_calls(event);
    calls.len() == 1
        && calls
            .first()
            .and_then(|call| call.get("name"))
            .and_then(serde_json::Value::as_str)
            == Some("structured_output")
}

/// The witnesses [`drive_attempt`]'s read loop accumulates across iterations and hands to every
/// [`DriveOutcome`] it can settle with, gathered into one value so the loop's arms — and the
/// helpers they delegate to — thread one `&mut` rather than six separate locals.
struct DriveState {
    /// Armed on the FIRST terminal assistant stop; once the grace window elapses without the child
    /// exiting, the child is force-drained.
    final_drain_at: Option<tokio::time::Instant>,
    /// SUBA-S06: armed when the child is reaped with stdout still open; expiring it ends the read
    /// loop so the post-loop `wait_final_drain()` can report the already-known exit status.
    exit_drain_at: Option<tokio::time::Instant>,
    /// Accumulates across every terminal stop (pi's `||=`) for `forcedDrainAfterFinalSuccess`.
    clean_terminal_stop: bool,
    /// pi's `agentSettledReceived` (`execution.ts:595,862,1080` @v0.43.0): the child announced the WHOLE run
    /// settled. Like [`Self::clean_terminal_stop`] it is a "the child finished on purpose" witness,
    /// so a forced drain after it is still coerced to success.
    agent_settled: bool,
    /// R-SA-037: set once the child's NDJSON shows a blocking `contact_supervisor` ask; the ask is
    /// surfaced via `spawn_clarify` exactly once (the guard in [`handle_child_line`]), and this flag
    /// then rides out to the attempt's `detached` outcome (bypassing acceptance; the ladder does
    /// not advance past it).
    detached_seen: bool,
    /// SUBA-008 — pi's four `updateTurnBudget` locals (`turnBudgetSoftReached` plus the three
    /// `result.turnBudget*` fields, `execution.ts:483`/`:567-569`/`:759-782`), gathered into one
    /// value. Unarmed (and therefore inert on every path below) unless this run declared a budget.
    turn_budget: crate::exec::turn_budget::TurnBudgetTracker,
    /// UW-3 — the child-watchdog config THIS attempt encoded into the child's env (pi
    /// `childWatchdog`, `execution.ts:298-302`), or `None` when the child runs unarmed. It is the
    /// gate on the whole fold (`if (!childWatchdog) return`, `:847`) and the source of the identity
    /// the child's status events are filtered against.
    child_watchdog: Option<ChildWatchdogConfig>,
    /// pi `childWatchdogState` (`:562`): the last accepted snapshot.
    watchdog_state: Option<ChildWatchdogStateSnapshot>,
    /// pi `watchdogTailTimer` (`:561`): armed while an armed child that has finished its turn is
    /// still reviewing; when it fires the review is declared stale and the drain starts.
    watchdog_tail_at: Option<tokio::time::Instant>,
    /// SUBA-118 — pi `compactionStartedReceived` (`execution.ts:546,970`): a `compaction_start` has
    /// been seen and not yet been cancelled. Cleared by `compaction_end{willRetry:true}` (`:971-974`)
    /// and by `agent_start`/`auto_retry_start` (`:977-980`), because each of those means the child is
    /// starting fresh rather than settling after a compaction.
    compaction_started: bool,
    /// SUBA-118 — pi `afterCompactionSettlement` (`execution.ts:547,996-999`): the child settled its
    /// drain WHILE a compaction was outstanding.
    ///
    /// It rides out on [`DriveOutcome::after_compaction_settlement`] (upstream's
    /// `result.afterCompactionSettlement`, `execution.ts:1418-1420`), where the abort-recovery
    /// dispatch reads it as
    /// [`crate::exec::abort_recovery::AbortRecoveryInput::after_compaction_settlement`] — the marker
    /// that makes a terminal-drain cleanup signal non-authoritative (`abort-recovery.ts:109`).
    after_compaction_settlement: bool,
    /// SUBA-118 — pi `result.toolBudgetBlocked` (`execution.ts:1150`): latched the first time a tool
    /// result this attempt observed is recognized as the run's OWN hard-block message
    /// ([`crate::exec::tool_budget::is_tool_budget_blocked_message`]). Never cleared: upstream only
    /// ever assigns `true`.
    tool_budget_blocked: bool,
    /// SUBA-118 — pi's `options.toolBudget` at the recognition site (`execution.ts:1149`). The budget
    /// lives on [`crate::exec::agent_config::AgentConfig`] in this crate rather than on
    /// [`RunOptions`], so it is handed in through [`DriveAttemptInputs`] instead of being read off
    /// `opts` here. `None` (no budget declared) makes the recognition unreachable, exactly as
    /// upstream's `options.toolBudget &&` guard does.
    tool_budget: Option<crate::discovery::types::ResolvedToolBudget>,
    /// CFG-067 — the per-tool-call deadlines armed for this attempt (pi's `activeToolTimeouts` /
    /// `activeToolTimeoutKeysByName` / `toolTimeoutSequence` trio, `execution.ts:1218-1220`),
    /// seeded with the run's already-resolved `toolTimeoutMs`.
    tool_timeouts: crate::exec::tool_timeout::ToolTimeoutTracker,
    /// SUBA-119 — the first model-verification failure seen on this attempt, if any. Upstream's
    /// `if (modelVerificationError && !result.error) result.error = modelVerificationError;`
    /// (`execution.ts:1102`) never CLOBBERS an earlier error, and the `!result.error` half of that is
    /// why this latches the FIRST one and ignores every later one.
    model_verification_error: Option<String>,
    /// SUBA-195 — pi `const partialOutput = createPartialOutputTracker()`, one per attempt
    /// (`runSingleAttempt` in `src/runs/foreground/execution.ts`; `runChildSession` in
    /// `src/runs/background/run-child-session.ts`), fed every parsed child event.
    partial_output: crate::exec::partial_output::PartialOutputTracker,
}

/// Which of [`drive_attempt`]'s exit paths is settling the attempt — the ONLY thing that differs
/// between its otherwise identical [`DriveOutcome`] constructions, and therefore the only argument
/// [`DriveState::outcome`] needs beyond the exit status itself.
#[derive(Clone, Copy)]
enum Settled {
    /// The child exited (or closed stdout) on its own, or the `wait()`/read itself faulted: none of
    /// the three special conditions applies.
    Exited,
    /// A hard `RunOptions.cancel` fired.
    Cancelled,
    /// `RunOptions.interrupt` fired (pi's soft interrupt, `execution.ts:722-745`) — the paused-
    /// success path, distinct from a timeout or a hard cancel.
    Interrupted,
    /// R-SA-036: the orchestrator's own wall-clock deadline expired. `timed_out` is what
    /// `run_fallback_ladder` (R-SA-036/6.3.2) actually branches on to stop the ladder outright.
    TimedOut,
    /// The child had to be force-drained through the real signal ladder. Whether that is coerced
    /// back to success (`forcedDrainAfterFinalSuccess`) is decided in `run_attempt` from
    /// `forced_termination` + `clean_terminal_stop` + no error.
    ForcedDrain,
    /// SUBA-008: a turn-budget abort. NOT a forced drain — `forcedDrainAfterFinalSuccess` must
    /// never coerce a turn-budget abort to exit 0.
    TurnBudgetAbort,
    /// pi `failProtocol`: NOT a forced *drain* either — upstream deliberately does not set
    /// `forcedTerminationSignal`, so the clean-drain coercion to exit 0 cannot swallow a protocol
    /// failure. (It could not anyway — that coercion also requires no error, and this sets one.)
    ProtocolFailure,
    /// CFG-067 — a per-tool-call deadline expired (pi `terminateForToolTimeout`,
    /// `execution.ts:1248`). It sets `result.timedOut = true` exactly as the run-level timeout does,
    /// which is why it shares [`DriveOutcome::timed_out`] rather than getting a flag of its own.
    ToolTimeout,
}

impl DriveState {
    fn new(opts: &RunOptions, child_watchdog: Option<ChildWatchdogConfig>) -> Self {
        Self {
            child_watchdog,
            watchdog_state: None,
            watchdog_tail_at: None,
            final_drain_at: None,
            exit_drain_at: None,
            clean_terminal_stop: false,
            agent_settled: false,
            detached_seen: false,
            compaction_started: false,
            after_compaction_settlement: false,
            tool_budget_blocked: false,
            // Assigned by `drive_attempt` from `DriveAttemptInputs::tool_budget`; a state built for
            // anything else drives a child that declared no budget.
            tool_budget: None,
            model_verification_error: None,
            tool_timeouts: crate::exec::tool_timeout::ToolTimeoutTracker::new(opts.tool_timeout_ms),
            partial_output: crate::exec::partial_output::PartialOutputTracker::new(),
            turn_budget: crate::exec::turn_budget::TurnBudgetTracker::new(
                opts.turn_budget,
                opts.enforce_hard_turn_limit,
            ),
        }
    }

    /// Settle the attempt: every witness accumulated so far rides out onto the [`DriveOutcome`],
    /// and `reason` supplies the three flags that tell one exit path from another.
    fn outcome(
        &self,
        reason: Settled,
        exit_status: std::io::Result<Option<std::process::ExitStatus>>,
    ) -> DriveOutcome {
        // Extracted only on an abnormal end, as upstream defers `text()` until then.
        let streamed_partial = match self.partial_cause(reason, &exit_status) {
            Some(cause) => self.partial_output.text().map(|text| (cause, text)),
            None => None,
        };
        DriveOutcome {
            streamed_partial,
            // CFG-067 — pi's tool-timeout path sets the same `result.timedOut` the run-level
            // timeout does (`execution.ts:1250` vs `:1199`), so the ladder stops on either.
            timed_out: matches!(reason, Settled::TimedOut | Settled::ToolTimeout),
            tool_timeout_error: None,
            interrupted: matches!(reason, Settled::Interrupted),
            forced_termination: matches!(reason, Settled::ForcedDrain),
            clean_terminal_stop: self.clean_terminal_stop,
            model_verification_error: self.model_verification_error.clone(),
            exit_status,
            detached: self.detached_seen,
            agent_settled: self.agent_settled,
            protocol_error: None,
            turn_budget: self.turn_budget.clone(),
            watchdog: self.watchdog_state.clone(),
            // SUBA-118 — upstream stamps both onto the attempt result on EVERY way out of
            // `runSingleAttempt` (`execution.ts:1418-1420` for the compaction marker, `:1150` for
            // the budget latch), so they ride out of every settle path here too rather than only
            // the ordinary one.
            after_compaction_settlement: self.after_compaction_settlement,
            tool_budget_blocked: self.tool_budget_blocked,
        }
    }

    /// SUBA-195 — pi `partialCause` (`src/runs/foreground/execution.ts` @ad11b7ab):
    ///
    /// ```ts
    /// const partialCause: PartialOutputCause | undefined = result.timedOut
    ///     ? "timeout"
    ///     : childPromptFailed && !abortedBySignal && !result.interrupted && !result.stopped ? "child error" : undefined;
    /// ```
    ///
    /// `result.timedOut` is both of cyrup's timeout settles (the run deadline and a per-tool
    /// deadline set the same flag upstream). `[CYRUP-DELTA]` `childPromptFailed` is upstream's
    /// in-process `session.prompt()` REJECTING; a cyrup child is a subprocess, whose equivalent is
    /// the process ending in failure — a non-zero exit, a fatal signal, or a wait/read fault —
    /// WITHOUT ever emitting `agent_settled`. That second half is what keeps upstream's "ordinary
    /// non-zero exits are unchanged" (`9bc8f2d1`'s message): a child whose run settled and then
    /// exited non-zero (a provider error the agent reported, say) did not throw, and an ordinary
    /// `Exited` settle is the only path a failure like that takes. Cancel (`abortedBySignal`),
    /// interrupt, a forced drain, a turn-budget abort and a protocol failure are their own settles
    /// and so never qualify.
    fn partial_cause(
        &self,
        reason: Settled,
        exit_status: &std::io::Result<Option<std::process::ExitStatus>>,
    ) -> Option<crate::exec::partial_output::PartialOutputCause> {
        match reason {
            Settled::TimedOut | Settled::ToolTimeout => {
                Some(crate::exec::partial_output::PartialOutputCause::Timeout)
            }
            Settled::Exited if !self.agent_settled => {
                let failed = match exit_status {
                    Ok(Some(status)) => !status.success(),
                    Ok(None) => false,
                    Err(_) => true,
                };
                failed.then_some(crate::exec::partial_output::PartialOutputCause::ChildError)
            }
            _ => None,
        }
    }

    /// pi `startFinalDrain` (`execution.ts:584-605` @v0.43.0): an armed child that is still
    /// reviewing is NOT drained — its watchdog tail is armed instead (`:585-588`). Otherwise the
    /// final-stop grace window opens, once.
    fn start_final_drain(&mut self) {
        if child_watchdog_is_active(self.watchdog_state.as_ref()) {
            self.arm_watchdog_tail();
            return;
        }
        if self.final_drain_at.is_none() {
            self.final_drain_at =
                Some(tokio::time::Instant::now() + Duration::from_millis(FINAL_STOP_GRACE_MS));
        }
    }

    /// pi `armWatchdogTail` (`:606-622`): only once the child has finished its turn (a clean
    /// terminal stop or `agent_settled`), and only once. It lasts the child config's
    /// `watchdogTailTimeoutMs` (pi's `?? 120_000`, `:621`, is unreachable here: the tail is only
    /// ever armed off an ACTIVE snapshot, which only an armed child can produce).
    fn arm_watchdog_tail(&mut self) {
        if !(self.clean_terminal_stop || self.agent_settled) || self.watchdog_tail_at.is_some() {
            return;
        }
        let tail_ms = self
            .child_watchdog
            .as_ref()
            .map_or(DEFAULT_WATCHDOG_TAIL_TIMEOUT_MS, |config| {
                config.watchdog_tail_timeout_ms
            });
        self.watchdog_tail_at = Some(tokio::time::Instant::now() + Duration::from_millis(tail_ms));
    }

    /// The tail timer's body (`:608-619`): the review is declared stale by the PARENT
    /// (`timedOut: true` is the parent's mark, never the child's), and the drain starts.
    fn watchdog_tail_expired(&mut self) {
        self.watchdog_tail_at = None;
        let seq = self.watchdog_state.as_ref().map_or(0, |state| state.seq) + 1;
        self.watchdog_state = Some(ChildWatchdogStateSnapshot {
            phase: ChildWatchdogPhase::Stale,
            seq,
            last_update: crate::time::now_epoch_millis(),
            follow_up_pending: false,
            reason: Some(WATCHDOG_TAIL_TIMEOUT_REASON.to_string()),
            timed_out: Some(true),
        });
        self.start_final_drain();
    }

    /// UW-3 — pi's parent fold (`execution.ts:846-864` @v0.43.0; the runner's twin at
    /// `subagent-runner.ts:626-645`), for a line that already parsed to
    /// [`SubagentEvent::Unknown`]. `Some(())` when the line WAS a child-watchdog status event: it
    /// is then fully consumed here, and the caller returns before the activity/progress folds, as
    /// upstream `return`s before `progress.lastActivityAt = now` (`:864-869`). `None` for any other
    /// line.
    ///
    /// The identity filter uses the config this attempt ENCODED, not upstream's literal
    /// `options.index ?? 0`: cyrup omits `childIndex` when a run has no index
    /// (`spawn_plan.rs`'s `opts.child_index.and_then(..)`), and a parent that required `Some(0)`
    /// would reject every event such a child emits (`child_status.rs`'s index check).
    fn fold_child_watchdog_line(&mut self, line: &str) -> Option<()> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if !is_child_watchdog_status_event(&value) {
            return None;
        }
        // `if (!childWatchdog) return;` — an unarmed parent swallows the event.
        let Some(config) = self.child_watchdog.as_ref() else {
            return Some(());
        };
        let Ok(event) = serde_json::from_value::<ChildWatchdogStatusEvent>(value) else {
            return Some(());
        };
        let identity = ChildWatchdogIdentity {
            run_id: config.run_id.clone(),
            agent: config.agent.clone(),
            child_index: config.child_index,
        };
        // `if (!next) return;` — a rejected event (another run's, or not newer) is still consumed
        // here: it is no more child activity than an accepted one.
        let Some(next) =
            accept_child_watchdog_event(self.watchdog_state.as_ref(), &event, &identity)
        else {
            return Some(());
        };
        let active = child_watchdog_is_active(Some(&next));
        self.watchdog_state = Some(next);
        if active {
            // `clearFinalDrainTimers(); armWatchdogTail();`
            self.final_drain_at = None;
            self.arm_watchdog_tail();
        } else {
            // `clearWatchdogTailTimer(); if (clean || settled) startFinalDrain();`
            self.watchdog_tail_at = None;
            if self.clean_terminal_stop || self.agent_settled {
                self.start_final_drain();
            }
        }
        Some(())
    }
}

/// pi's `childWatchdog?.watchdogTailTimeoutMs ?? 120_000` fallback (`execution.ts:621` @v0.43.0),
/// equal to `settings.ts`'s own default for the key (cyrup `watchdog/settings.rs`).
const DEFAULT_WATCHDOG_TAIL_TIMEOUT_MS: u64 = 120_000;

/// The reason the parent stamps on the snapshot when its tail timer fires (`execution.ts:616`).
pub(crate) const WATCHDOG_TAIL_TIMEOUT_REASON: &str = "child watchdog tail timeout";

/// What [`handle_child_line`] tells the drive loop to do once one NDJSON line has been folded in.
enum LineAction {
    /// Keep reading the child's stdout.
    Continue,
    /// SUBA-008 — the run's turn budget is exhausted and pi's terminal `message` has been composed:
    /// the drive loop must abort the child through [`turn_budget_abort`].
    TurnBudgetAbort(String),
}

/// A timer that fires at `at`, or never fires at all while the window is unarmed.
///
/// A fresh `sleep_until` against the fixed instant on each loop iteration is correct: it always
/// resolves at the same absolute time regardless of how often it is reconstructed, and reduces to
/// `pending()` (never fires) until the window is armed.
async fn pending_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

/// One NDJSON line off the child's stdout: tee it to the live sink, parse it exactly once, fold it
/// into the terminal-stop/settled witnesses, arm or disarm the final-stop grace window, surface a
/// blocking `contact_supervisor` ask, append it to the live transcript, feed the control
/// heuristics, and record it on `progress`. Returns what the drive loop should do next.
///
/// `async` only for the transcript append: [`ChildTranscriptWriter::write_child_event`] goes
/// through the crate's async capped appender, the same per-line await the raw `.jsonl` tee
/// already pays in `spawn::SpawnedChild::next_event`.
async fn handle_child_line(
    line: &str,
    state: &mut DriveState,
    progress: &mut AgentProgress,
    control: &mut crate::exec::control::ControlMonitor,
    opts: &RunOptions,
    transcript: Option<&mut ChildTranscriptWriter>,
    expected_model_for_verification: Option<&str>,
) -> LineAction {
    // NOTE: the raw NDJSON envelope deliberately does NOT enter `progress.recent_output` — pi
    // appends only EXTRACTED text, from an assistant `message_end`'s content and a finished tool
    // call's result, and `AgentProgress::record_event` does exactly that a few lines below. A raw
    // line here would put an unrenderable (and, before `RECENT_OUTPUT_LINE_CHARS`, unbounded) JSON
    // blob on the very field `SingleResult::progress` publishes as pi's `recentOutput`.
    // Live-telemetry tee (pi's child-event pump, `subagent-runner.ts:1430`): hand the raw NDJSON
    // line to the background runner's sink, if one is installed, BEFORE this module parses/folds it
    // — so the runner folds it into `status.json` live without this module depending on
    // `background`.
    if let Some(sink) = &opts.live_events {
        sink.emit(line);
    }
    // `SpawnedChild::next_event_or_exit` tees and hands back the raw line without parsing it —
    // `exec::ndjson::parse_line` is the crate's ONE NDJSON parse, so each child stdout line is
    // deserialized exactly once, here, against the single `SubagentEvent` schema (final-output
    // extraction, R-SA-029; completion-guard scanning, R-SA-034).
    let Some(event) = crate::exec::ndjson::parse_line(line) else {
        return LineAction::Continue;
    };
    // SUBA-195 — pi `partialOutput.observe(evt)`, the first statement of `processEvent`
    // (`src/runs/foreground/execution.ts`; `src/runs/background/run-child-session.ts`) @ad11b7ab:
    // every parsed event, before any other fold can return early.
    state.partial_output.observe(&event);
    // Final-stop grace-drain (pi `startFinalDrain`, execution.ts:584-605): open the grace window on
    // the FIRST terminal assistant stop and track whether ANY terminal stop was clean (no
    // errorMessage) for `forcedDrainAfterFinalSuccess`.
    let terminal_stop = is_terminal_assistant_stop(&event);
    if terminal_stop {
        state.clean_terminal_stop =
            state.clean_terminal_stop || !message_end_has_error_message(&event);
    }
    if matches!(event, crate::exec::ndjson::SubagentEvent::AgentSettled) {
        state.agent_settled = true;
    }
    // SUBA-118 — pi's compaction-evidence fold (`execution.ts:970-980`), in upstream's order and
    // BEFORE the lifecycle projection below, because the `agent_settled` arm reads
    // `compaction_started` as it stands at that moment.
    match &event {
        // `:970`
        crate::exec::ndjson::SubagentEvent::CompactionStart { .. } => {
            state.compaction_started = true;
        }
        // `:971-974` — a compaction that will RETRY is not a settled compaction, so both markers
        // clear. `aborted` alone does NOT clear them: upstream gates only on `willRetry === true`.
        crate::exec::ndjson::SubagentEvent::CompactionEnd {
            will_retry: true, ..
        } => {
            state.compaction_started = false;
            state.after_compaction_settlement = false;
        }
        // `:977-980` — a child starting a fresh agent run, or backing off into an auto-retry, is not
        // settling after a compaction.
        crate::exec::ndjson::SubagentEvent::AgentStart
        | crate::exec::ndjson::SubagentEvent::AutoRetryStart { .. } => {
            state.compaction_started = false;
            state.after_compaction_settlement = false;
        }
        _ => {}
    }
    // pi `applyChildLifecycle(projectChildLifecycle(evt))` — run for EVERY event
    // (`execution.ts:844`), plus the terminal-stop form at `:947`. The three arms are:
    // `agent_end{willRetry:true}` DISARMS the window (the child is about to retry — force-killing
    // it there kills a run that is still working); `agent_settled` and a terminal assistant stop
    // ARM it; everything else leaves it alone.
    let will_retry = matches!(
        event,
        crate::exec::ndjson::SubagentEvent::AgentEnd {
            will_retry: true,
            ..
        }
    );
    match crate::exec::child_protocol::project_child_lifecycle(
        event.kind(),
        will_retry,
        terminal_stop,
    ) {
        // pi `applyChildLifecycle` (`execution.ts:623-630` @v0.43.0): `cancel-drain` clears BOTH
        // the drain timers and the watchdog tail — a retrying child is neither finished nor still
        // reviewing its last turn.
        crate::exec::child_protocol::ChildLifecycleAction::CancelDrain => {
            state.final_drain_at = None;
            state.watchdog_tail_at = None;
        }
        crate::exec::child_protocol::ChildLifecycleAction::StartDrain => {
            // SUBA-118 — pi `:996-999`: `if (evt.type === "agent_settled" && lifecycleAction ===
            // "start-drain") afterCompactionSettlement = compactionStartedReceived;`. Both halves of
            // that guard are load-bearing: it is an `agent_settled` that ACTUALLY starts the drain,
            // not merely any `agent_settled` and not any other event that starts one.
            if matches!(event, crate::exec::ndjson::SubagentEvent::AgentSettled) {
                state.after_compaction_settlement = state.compaction_started;
            }
            state.start_final_drain();
        }
        crate::exec::child_protocol::ChildLifecycleAction::None => {}
    }
    // R-SA-037 detach-trigger arm: a child's blocking `contact_supervisor` ask
    // (`need_decision`/`interview`) surfaces the ask to the parent's human via the real
    // `ClarifyChannel` (fired exactly once) and marks this attempt detached. The intercom answer
    // routes back to the still-alive child over the BROKER (independent of this stdout pipe), so
    // the loop keeps driving — it neither kills nor synchronously blocks on the child.
    //
    // VL-S11b — this is cyrup's port of upstream's FIRST detach producer,
    // `detachForeground("intercom coordination")` (`execution.ts:762`). The flag set here is what
    // `run_foreground_impl`'s `stamp_intercom_detach_reason`
    // (`extension/executor/foreground.rs`) reads to write `DetachReason::IntercomCoordination`
    // onto `SingleResult::detached_reason` — up there rather than here because `crate::exec` sits
    // below `crate::extension` and cannot name that module. The SECOND producer,
    // `/subagents-detach`, does not pass through here at all: it mints its receipt in
    // `foreground.rs`'s `detach_receipt` and returns while this loop keeps driving the same child
    // in a continuation task, which is the difference `workflow_detach`'s module doc now states.
    if !state.detached_seen
        && let Some(prompt) = contact_supervisor_block_prompt(&event)
    {
        state.detached_seen = true;
        if let Some(dispatch) = &opts.clarify {
            // Dropping the returned receiver does not cancel the ask (a human may still be
            // answering); it only means this loop does not itself await the outcome — the child
            // unblocks over the broker instead.
            let _rx = crate::tui::intercom::spawn_clarify(
                dispatch.lock.clone(),
                dispatch.session_key.clone(),
                crate::tui::intercom::ClarifyRequest {
                    run_id: dispatch.run_id.clone(),
                    step_index: dispatch.step_index,
                    prompt,
                },
            );
        }
    }
    // pi `shared.transcriptWriter?.writeChildEvent(evt)` (`execution.ts:980`; the async runner's
    // `run-child-session.ts:410`): the live `_transcript.jsonl` is fed from THIS parsed event, at
    // the crate's one parse point, so the foreground executor and the detached runner (which
    // reaches here through `run_sync`) both write it while the child is still running. Placed
    // before the control/progress folds because `record_event` consumes the event by value.
    if let Some(writer) = transcript {
        writer.write_child_event(&event).await;
    }
    // UW-3 — the child-watchdog status fold. AFTER the transcript write (upstream's
    // `writeChildEvent`, `:842`, precedes it) and BEFORE the activity fold: upstream returns before
    // `progress.lastActivityAt = now` (`:864-869`), so a status line is not child activity. Only a
    // line that already degraded to `Unknown` is re-read, so every known event keeps its single
    // parse.
    if matches!(event, SubagentEvent::Unknown) && state.fold_child_watchdog_line(line).is_some() {
        return LineAction::Continue;
    }
    // pi `processLine` (`execution.ts:775-890`): every parsed child event is fresh activity for the
    // control heuristics, and the tool-start / tool-result / assistant-turn folds feed the
    // thresholds. Driven BEFORE `record_event` because that consumes the event by value.
    control.observe_event(&event, crate::time::now_epoch_millis());
    // SUBA-008 — the two per-message inputs `updateTurnBudget` needs, read BEFORE `record_event`
    // consumes the event by value (same reason the control fold above runs here).
    let assistant_turn = is_assistant_message_end(&event);
    let has_tool_call = message_end_has_tool_call(&event);
    // SUBA-119 — pi `execution.ts:1096-1103`: inside `if (evt.message.model)`, gated on
    // `expectedModelForVerification && !hasToolCall`, and assigned with `&& !result.error` so it never
    // clobbers an earlier error. The `!hasToolCall` gate is upstream's: a message that is making a
    // tool call is mid-turn, and a provider may report a different routing id on one of those without
    // the run having actually changed model.
    if let Some(expected) = expected_model_for_verification
        && !has_tool_call
        && state.model_verification_error.is_none()
        && let Some(observed) = assistant_message_end_model(&event)
    {
        state.model_verification_error =
            crate::exec::model_verification::format_subagent_model_verification_error(
                expected,
                observed,
                &crate::extension::models::registry_available_models(),
                opts.model_response_aliases.as_ref(),
            );
    }
    // CFG-067 — pi's per-tool deadline arm/disarm, at upstream's own two moments: armed inside the
    // `tool_execution_start` handler (`execution.ts:1035`) and cleared inside `tool_execution_end`
    // (`:1053`). The terminal-stop `clearAllToolTimeouts()` (`:1116`) follows, because after the
    // child's last assistant message no tool call of its can still be outstanding.
    match &event {
        crate::exec::ndjson::SubagentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            ..
        } => {
            state.tool_timeouts.arm(
                Some(tool_call_id.as_str()),
                tool_name,
                tokio::time::Instant::now(),
                run_remaining_ms(opts),
            );
        }
        crate::exec::ndjson::SubagentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            ..
        } => {
            state
                .tool_timeouts
                .clear(Some(tool_call_id.as_str()), Some(tool_name));
            // SUBA-118 — pi `execution.ts:1149-1156`: a tool RESULT that is this run's own
            // hard-block message latches `result.toolBudgetBlocked`, which the abort-recovery plan
            // reads to refuse a resume for a child its budget stopped (`abort-recovery.ts:111`).
            //
            // [CYRUP-DELTA] upstream reads the result text off the `message_end` for the toolResult
            // message and has to guess the blocked tool's name from a single pending slot, with its
            // own comment that the slot "can describe a different, overlapping call and serves only
            // as a fallback" (`:1145-1148`). cyrup's wire carries the tool NAME and the RESULT on
            // one `tool_execution_end` event, so the authoritative name is always available and the
            // fallback has nothing to do — identical behaviour, no guess.
            if !state.tool_budget_blocked
                && let Some(budget) = state.tool_budget.clone()
            {
                let result_text = crate::tui::events::extract_event_text(result);
                state.tool_budget_blocked =
                    crate::exec::tool_budget::is_tool_budget_blocked_message(
                        &budget,
                        &result_text,
                        Some(tool_name.as_str()),
                    );
            }
        }
        _ => {}
    }
    if terminal_stop {
        state.tool_timeouts.clear_all();
        // SUBA-164 — pi clears `activeToolCalls` on the same terminal assistant stop
        // (`execution.ts:1129-1132` @v0.75.0), one line after `clearAllToolTimeouts()`: the child
        // declared itself done, so no stale open call may trip the open-tool attention threshold
        // afterwards.
        control.clear_active_tool_calls();
    }
    let terminal_structured_output_call =
        opts.structured_output_schema.is_some() && is_sole_structured_output_tool_call(&event);
    progress.record_event(event);

    // pi `execution.ts:910-924`: an ASSISTANT `message_end` is one turn, and the budget is
    // re-evaluated on it. `progress.turn_count()` is this port's `result.usage.turns` — pi keeps
    // the two in lockstep (`:913-914`) and cyrup derives the one from the other rather than
    // carrying a second counter that could drift.
    if assistant_turn && state.turn_budget.is_armed() {
        return observe_turn_budget(
            state,
            progress,
            terminal_stop || terminal_structured_output_call,
            has_tool_call,
        );
    }
    LineAction::Continue
}

/// SUBA-008 — re-evaluate the run's turn budget against the assistant turn just recorded (pi's
/// `updateTurnBudget`, `execution.ts:759-782`). `terminal_turn` is pi's second argument, the
/// terminal-stop OR sole-`structured_output`-call disjunction its caller composes.
fn observe_turn_budget(
    state: &mut DriveState,
    progress: &mut AgentProgress,
    terminal_turn: bool,
    has_tool_call: bool,
) -> LineAction {
    let turn_count = u64::from(progress.turn_count());
    // pi's third argument: `hasToolCall || Boolean(progress.currentTool)` (`:924`) — tool work
    // either STARTING on this very message or still in flight from an earlier one. This is what
    // makes the deferral arm reachable.
    let tool_work_active_or_starting = has_tool_call || progress.current_tool.is_some();
    let effect = state.turn_budget.observe_assistant_turn(
        turn_count,
        terminal_turn,
        tool_work_active_or_starting,
        false,
    );
    match effect {
        crate::exec::turn_budget::TurnBudgetEffect::None => LineAction::Continue,
        crate::exec::turn_budget::TurnBudgetEffect::SoftNote(note) => {
            // pi `appendRecentOutput(progress, [turnBudgetSoftNote(...)])` (`:769`) — the wrap-up
            // request reaches the operator through the run's own output tail, once.
            progress.append_recent_output(&note);
            LineAction::Continue
        }
        crate::exec::turn_budget::TurnBudgetEffect::Abort { message, soft_note } => {
            if let Some(note) = soft_note {
                progress.append_recent_output(&note);
            }
            LineAction::TurnBudgetAbort(message)
        }
    }
}

/// CFG-067 — pi's `runRemaining` term (`execution.ts:1268`):
/// `attemptTimeout ? Math.max(0, attemptTimeout.remainingMs - elapsed) : undefined`. cyrup carries
/// the run's deadline as an absolute instant rather than a budget plus a start time, so the
/// subtraction upstream spells out is `saturating_duration_since` here — the same number, with the
/// `Math.max(0, …)` clamp built in.
fn run_remaining_ms(opts: &RunOptions) -> Option<u64> {
    opts.deadline_at.map(|deadline| {
        u64::try_from(
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_millis(),
        )
        .unwrap_or(u64::MAX)
    })
}

/// CFG-067 — pi `terminateForToolTimeout` (`execution.ts:1248-1263`): the run is marked timed out,
/// the message becomes BOTH `result.error` and `result.finalOutput`, and the child is aborted.
///
/// `[CYRUP-DELTA]` at full feature parity, the same one [`turn_budget_abort`] records: upstream
/// arms a 4 s hard-finish timer and keeps reading the child in the meantime, while this crate's
/// [`SpawnedChild::terminate`] consumes the child and walks the real SIGINT->SIGTERM->SIGKILL
/// ladder. Both settle the attempt as timed out with the same message; the message is carried on
/// the outcome precisely so the child's own late apology cannot displace it.
async fn tool_timeout_abort(
    child: SpawnedChild,
    cancel: &CancelToken,
    message: String,
    state: &DriveState,
) -> DriveOutcome {
    let outcome = child.terminate(cancel).await;
    DriveOutcome {
        tool_timeout_error: Some(message),
        ..state.outcome(Settled::ToolTimeout, outcome.map(|o| Some(o.status)))
    }
}

/// pi `requestTurnBudgetAbort` (`:733-757`): SIGINT now, SIGTERM 1 s later, SIGKILL 4 s after the
/// SIGINT. That is exactly this ladder with the two graces pinned — 1 s to escalate off SIGINT and
/// 3 s more to escalate off SIGTERM lands the kill at t+4 s, upstream's own instant.
///
/// [CYRUP-DELTA]: pi ARMS the two timers and lets the run keep reading the child's stdout in the
/// meantime, so a child that wraps up inside the window still delivers its final output; this
/// ladder blocks the drive loop for the same wall-clock window instead, because
/// [`SpawnedChild::terminate`] consumes the child and cyrup has no seam that signals without taking
/// it. The observed outcome is the same on both timelines — the child either dies on SIGINT (the
/// ladder returns immediately) or is escalated on upstream's schedule — but a late final message
/// written after the SIGINT is dropped here where upstream would have read it, which is why the
/// abort message doubles as `final_output` in `run_attempt`.
async fn turn_budget_abort(
    child: SpawnedChild,
    cancel: &CancelToken,
    message: String,
    state: &DriveState,
) -> DriveOutcome {
    let outcome = child
        .terminate_with_graces(
            cancel,
            crate::spawn::signal::EscalationGraces {
                sigint: Duration::from_millis(
                    crate::exec::turn_budget::TURN_BUDGET_TERMINATION_DELAY_MS,
                ),
                sigterm: Duration::from_millis(
                    crate::exec::turn_budget::TURN_BUDGET_HARD_KILL_DELAY_MS
                        - crate::exec::turn_budget::TURN_BUDGET_TERMINATION_DELAY_MS,
                ),
            },
        )
        .await;
    // `message` is not carried on the outcome: it is recomputed verbatim from the tracker's own
    // state by `TurnBudgetTracker::terminal_note`, which is the single place `run_attempt` reads it
    // from, so there is exactly one producer of upstream's string.
    debug_assert_eq!(
        state.turn_budget.terminal_note(),
        Some(crate::exec::turn_budget::TurnBudgetTerminalNote::Exceeded(
            message.clone()
        ))
    );
    drop(message);
    state.outcome(Settled::TurnBudgetAbort, outcome.map(|o| Some(o.status)))
}

/// pi `failProtocol` (`execution.ts:1026-1041`): the diagnostic becomes the run's error and the
/// child is signalled down (upstream SIGTERM then, 3s later, SIGKILL — cyrup routes every forced
/// termination through [`SpawnedChild::terminate`]'s own SIGINT->SIGTERM->SIGKILL ladder instead of
/// inventing a second one). Nothing further can be read: the reader is permanently closed, so
/// continuing to poll it would spin on `Eof`.
async fn protocol_limit_outcome(
    child: SpawnedChild,
    cancel: &CancelToken,
    limit: crate::exec::child_protocol::ProtocolOutputLimit,
    state: &DriveState,
) -> DriveOutcome {
    let outcome = child.terminate(cancel).await;
    DriveOutcome {
        protocol_error: Some(limit),
        ..state.outcome(Settled::ProtocolFailure, outcome.map(|o| Some(o.status)))
    }
}

/// SUBA-S06: the process is gone but stdout is STILL OPEN, because a surviving grandchild inherited
/// the write end. The EOF the read loop used to wait on can never arrive, and none of the other
/// select arms is guaranteed to fire either — the deadline arm only exists when the caller passed a
/// timeout, the final-drain arm only after a terminal assistant stop the child never emitted, and
/// the activity tick merely re-scores heuristics. So the tool call hung forever, spinning once a
/// second.
///
/// Do NOT break the read loop on this step: lines written before the exit may still be buffered in
/// the pipe, and dropping them would trade a hang for silent output loss. This arms a bounded
/// post-exit window instead and the loop keeps draining; the status itself is deliberately
/// discarded because the post-loop `wait_final_drain()` re-reads it (the child is marked reaped, so
/// that call returns immediately) and routes it through the ONE existing clean path — which is what
/// keeps this a normal exit rather than a `forced_termination`.
fn arm_post_exit_drain(state: &mut DriveState) {
    if state.exit_drain_at.is_none() {
        state.exit_drain_at =
            Some(tokio::time::Instant::now() + Duration::from_millis(POST_EXIT_DRAIN_MS));
    }
}

/// Drive one spawned child to completion, folding every NDJSON line into `progress` (R-SA-027/028)
/// and racing the whole read loop against `opts.cancel`/`opts.interrupt`/an optional deadline
/// timer, plus the final-stop grace-drain window (pi `execution.ts:333-367`, T3 group A). Returns a
/// [`DriveOutcome`].
///
/// On timeout, cancel, interrupt, or a final-stop grace-drain, the child is driven through
/// [`SpawnedChild::terminate`]'s real signal-escalation ladder (R-SA-036/059) — never a bare
/// `kill()`. `child` is taken by value (never `&mut`): [`SpawnedChild::terminate`]/
/// [`SpawnedChild::finish`] both consume `self` to guarantee temp-file cleanup runs exactly once
/// on every exit path (R-SA-067), so this function's own signature is shaped to always be able to
/// hand `child` off to whichever exit path is taken, with no placeholder/`Default` value ever
/// needed to satisfy a borrow.
/// The two per-attempt facts [`drive_attempt`] needs beyond the child, the progress record and the
/// run-wide [`RunOptions`]. Bundled rather than passed loose so the function stays within this
/// workspace's argument bound (`clippy::too_many_arguments`).
pub(crate) struct DriveAttemptInputs<'a> {
    /// UW-3 — the child-watchdog config this attempt encoded into the child's env (pi `childWatchdog`,
    /// `execution.ts:298-302`), or `None` when the child runs unarmed.
    pub(crate) child_watchdog: Option<ChildWatchdogConfig>,
    /// SUBA-119 — pi's `expectedModelForVerification` (`execution.ts:365`): the exact `--model` argv
    /// this attempt launched with, or `None` when verification is off for this run
    /// (`shared.verifyModel` false — no candidate, or the model came from the parent session).
    pub(crate) expected_model_for_verification: Option<&'a str>,
    /// SUBA-118 — pi's `options.toolBudget` (`execution.ts:1149`), the budget whose OWN hard-block
    /// message counts as `result.toolBudgetBlocked`. Comes from the agent's validated `toolBudget:`
    /// ([`crate::exec::agent_config::AgentConfig::tool_budget`]), which is also what the spawn plan
    /// encodes into the child's env, so parent and child are judging the same budget.
    pub(crate) tool_budget: Option<crate::discovery::types::ResolvedToolBudget>,
}

pub(crate) async fn drive_attempt(
    mut child: SpawnedChild,
    progress: &mut AgentProgress,
    opts: &RunOptions,
    deadline_sleep: Option<tokio::time::Sleep>,
    control: &mut crate::exec::control::ControlMonitor,
    mut transcript: Option<&mut ChildTranscriptWriter>,
    inputs: DriveAttemptInputs<'_>,
) -> DriveOutcome {
    let DriveAttemptInputs {
        child_watchdog,
        expected_model_for_verification,
        tool_budget,
    } = inputs;
    tokio::pin!(deadline_sleep);
    let cancel = opts.cancel.clone();
    let interrupt = opts.interrupt.clone();

    // pi's 1s activity timer (`execution.ts:896-905`): while control tracking is enabled, the
    // idle/long-running heuristics are re-evaluated on a fixed tick as well as on every observed
    // child event — otherwise a child that goes SILENT (the exact condition `needs_attention`
    // exists to diagnose) would never trip it, because nothing would arrive to trigger the check.
    // `interval_at` (not `interval`) because tokio's first `interval` tick completes immediately,
    // which would fire a spurious check at t=0.
    let mut activity_tick = control.enabled().then(|| {
        let period = Duration::from_millis(crate::exec::control::ACTIVITY_TICK_MS);
        tokio::time::interval_at(tokio::time::Instant::now() + period, period)
    });

    let mut state = DriveState::new(opts, child_watchdog);
    state.tool_budget = tool_budget;

    loop {
        let deadline_arm = async {
            match deadline_sleep.as_mut().as_pin_mut() {
                Some(sleep) => sleep.await,
                None => std::future::pending::<()>().await,
            }
        };
        let final_drain_arm = pending_until(state.final_drain_at);
        let tool_timeout_arm = pending_until(state.tool_timeouts.next_deadline());
        let watchdog_tail_arm = pending_until(state.watchdog_tail_at);
        let exit_drain_arm = pending_until(state.exit_drain_at);

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let outcome = child.terminate(&cancel).await;
                return state.outcome(Settled::Cancelled, outcome.map(|o| Some(o.status)));
            }
            () = interrupt.cancelled() => {
                // pi `execution.ts:1090`: a soft interrupt CLEARS the activity state, so a
                // needs-attention notice that was raised (and is still sitting in the parent's
                // debounce window) fails its actionability re-check rather than landing in the
                // transcript for a run the caller has already deliberately paused.
                control.clear_activity_state();
                let outcome = child.terminate(&cancel).await;
                return state.outcome(Settled::Interrupted, outcome.map(|o| Some(o.status)));
            }
            () = deadline_arm => {
                // R-SA-036: timeout is a SOFT interrupt, not an immediate hard kill — it still
                // walks the full SIGINT->SIGTERM->SIGKILL ladder via `terminate`, exactly like
                // cancel/interrupt above; what makes it a timeout rather than a plain
                // cancellation is the `timed_out: true` flag, which is what `run_fallback_ladder`
                // (R-SA-036/6.3.2) actually branches on to stop the ladder outright.
                let outcome = child.terminate(&cancel).await;
                return state.outcome(Settled::TimedOut, outcome.map(|o| Some(o.status)));
            }
            () = tool_timeout_arm => {
                // CFG-067 — the earliest armed per-tool deadline came due. `expire` removes it
                // first (upstream's `removeToolTimeoutKey(key)` ahead of the terminate) and hands
                // back the message it was armed to raise.
                if let Some(expiry) = state.tool_timeouts.expire(tokio::time::Instant::now()) {
                    return tool_timeout_abort(child, &cancel, expiry.message, &state).await;
                }
            }
            step = child.next_event_or_exit() => {
                match step {
                    crate::spawn::ChildStep::Line(Ok(line)) => {
                        match handle_child_line(
                            &line,
                            &mut state,
                            progress,
                            control,
                            opts,
                            transcript.as_deref_mut(),
                            expected_model_for_verification,
                        )
                        .await
                        {
                            LineAction::Continue => {}
                            LineAction::TurnBudgetAbort(message) => {
                                return turn_budget_abort(child, &cancel, message, &state).await;
                            }
                        }
                    }
                    crate::spawn::ChildStep::ProtocolLimit(limit) => {
                        return protocol_limit_outcome(child, &cancel, limit, &state).await;
                    }
                    crate::spawn::ChildStep::Line(Err(_)) | crate::spawn::ChildStep::Eof => {
                        // Stdout EOF (child exited/closed stdout) or a genuine read fault — either
                        // way, stop reading and wait for the real exit status below.
                        break;
                    }
                    crate::spawn::ChildStep::Exited(_) => arm_post_exit_drain(&mut state),
                }
            }
            () = exit_drain_arm => {
                // SUBA-S06: the reaped child's buffered stdout has had its beat; whatever still
                // holds the pipe open is not this run's problem. Break (never return) so the exit
                // status flows through the normal post-loop path as an ordinary clean exit.
                break;
            }
            () = watchdog_tail_arm => {
                // UW-3 — pi's `watchdogTailTimer` body (`execution.ts:608-619`): the armed child
                // has been reviewing for its whole tail allowance. Mark it stale and drain.
                state.watchdog_tail_expired();
            }
            () = final_drain_arm => {
                // The child emitted its terminal stop but did not exit within the grace window —
                // force-drain it through the real signal ladder (pi's SIGTERM->SIGKILL). Whether
                // this is coerced back to success (`forcedDrainAfterFinalSuccess`) is decided in
                // `run_attempt` from `forced_termination` + `clean_terminal_stop` + no error.
                let outcome = child.terminate(&cancel).await;
                return state.outcome(Settled::ForcedDrain, outcome.map(|o| Some(o.status)));
            }
            () = async {
                match activity_tick.as_mut() {
                    Some(tick) => { tick.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                // pi's `setInterval(..., 1000)` body (`execution.ts:898-904`), minus the
                // `fireUpdate()` half: this crate's live-progress payload is assembled by
                // `tui::events` off the same NDJSON stream, so the tick's job here is purely to
                // re-evaluate the idle/long-running heuristics on a silent child.
                control.update_activity_state(crate::time::now_epoch_millis());
            }
        }
    }

    match child.wait_final_drain().await {
        Ok(Some(status)) => {
            child.finish(); // R-SA-067: success-path temp-file cleanup.
            state.outcome(Settled::Exited, Ok(Some(status)))
        }
        Ok(None) => {
            // The child closed stdout but did not exit within FINAL_DRAIN_TIMEOUT (R-SA-068) —
            // fall back to the real signal-escalation ladder. This is a forced termination too:
            // combined with a clean terminal stop and no error, `forcedDrainAfterFinalSuccess`
            // still coerces an otherwise-successful, merely-slow-to-teardown run to exit 0.
            let outcome = child.terminate(&cancel).await;
            state.outcome(Settled::ForcedDrain, outcome.map(|o| Some(o.status)))
        }
        Err(err) => state.outcome(Settled::Exited, Err(err)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! UW-3 — the parent fold, below the subprocess harness (`cyrup-it`'s
    //! `child_watchdog_status_integration.rs` drives it end to end).

    use super::*;

    fn armed() -> ChildWatchdogConfig {
        ChildWatchdogConfig {
            enabled: true,
            run_id: Some("run-1".to_string()),
            agent: Some("worker".to_string()),
            child_index: None,
            watchdog_tail_timeout_ms: 1_500,
            agent_end_timeout_ms: 30_000,
            max_warnings: None,
            model: None,
            thinking: None,
            lsp: crate::watchdog::types::WatchdogLspConfig {
                enabled: false,
                timeout_ms: 1_000,
                max_files: 5,
                max_diagnostics: 7,
            },
            auto_follow_blockers: false,
            auto_follow_max_attempts: None,
            stalemate_repeats: 2,
        }
    }

    fn status(seq: u64, phase: &str) -> String {
        serde_json::json!({
            "type": "subagent.watchdog.status",
            "runId": "run-1",
            "agent": "worker",
            "seq": seq,
            "phase": phase,
            "ts": 1_i64,
            "followUpPending": false,
        })
        .to_string()
    }

    /// CFG-067 — the parent-side enforcement seam: a `tool_execution_start` ARMS the call's
    /// deadline (pi `execution.ts:1035`) and its `tool_execution_end` CLEARS it (`:1053`).
    ///
    /// The run configures nothing, so `read` is armed off `DEFAULT_FAST_TOOL_TIMEOUT_TOOLS` alone
    /// and `bash` is not armed at all — which is what makes "the fast builtins have a deadline even
    /// unconfigured" a tested fact rather than a comment. Killing mutations: dropping either arm of
    /// the fold, or arming on the wrong event.
    #[tokio::test]
    async fn a_tool_start_arms_its_deadline_and_its_end_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        let mut state = DriveState::new(&opts, None);
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::disabled();

        let start = |id: &str, tool: &str| {
            serde_json::json!({
                "type": "tool_execution_start", "toolCallId": id, "toolName": tool
            })
            .to_string()
        };
        let end = |id: &str, tool: &str| {
            serde_json::json!({
                "type": "tool_execution_end", "toolCallId": id, "toolName": tool
            })
            .to_string()
        };

        handle_child_line(
            &start("c1", "bash"),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(
            state.tool_timeouts.next_deadline().is_none(),
            "an unconfigured non-fast tool gets no deadline"
        );

        handle_child_line(
            &start("c2", "read"),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(
            state.tool_timeouts.next_deadline().is_some(),
            "a fast builtin is armed off the 5-minute default"
        );

        handle_child_line(
            &end("c2", "read"),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(
            state.tool_timeouts.next_deadline().is_none(),
            "its own end clears it"
        );
    }

    /// pi `clearAllToolTimeouts()` on the terminal assistant stop (`execution.ts:1116`): after the
    /// child's last message no tool call of its can still be outstanding, so nothing may stay armed
    /// to fire during the drain. Killing mutation: dropping the `if terminal_stop` clear — the
    /// deadline survives the stop and can abort a child that already answered.
    #[tokio::test]
    async fn a_terminal_assistant_stop_clears_every_armed_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        let mut state = DriveState::new(&opts, None);
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::disabled();

        handle_child_line(
            &serde_json::json!({
                "type": "tool_execution_start", "toolCallId": "c1", "toolName": "read"
            })
            .to_string(),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(state.tool_timeouts.next_deadline().is_some());

        handle_child_line(
            &serde_json::json!({
                "type": "message_end",
                "message": { "role": "assistant", "stopReason": "stop", "content": [] }
            })
            .to_string(),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(state.clean_terminal_stop, "the stop was observed");
        assert!(
            state.tool_timeouts.next_deadline().is_none(),
            "and it disarmed every per-tool deadline"
        );
    }

    /// The run's configured value reaches the tracker through [`RunOptions::tool_timeout_ms`], and
    /// the expiry carries pi's verbatim sentence for the tool that actually overran. Killing
    /// mutation: seeding the tracker with `None`, which silently restores the fast-tool defaults for
    /// every tool and drops the operator's configured deadline on the floor.
    #[tokio::test]
    async fn the_runs_configured_deadline_reaches_the_tracker_and_names_the_overrunning_tool() {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::RunOptions {
            tool_timeout_ms: Some(25),
            ..crate::exec::testsupport::base_opts(dir.path(), &["m"])
        };
        let mut state = DriveState::new(&opts, None);
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::disabled();
        handle_child_line(
            &serde_json::json!({
                "type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash"
            })
            .to_string(),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        let deadline = state
            .tool_timeouts
            .next_deadline()
            .expect("the configured value arms even a non-fast tool");
        assert_eq!(
            state.tool_timeouts.expire(deadline).map(|e| e.message),
            Some("Tool 'bash' exceeded its timeout of 25ms.".to_string())
        );
    }

    /// U1 — a status line is NOT child activity: upstream returns before `lastActivityAt = now`
    /// (`execution.ts:864-869`). The monitor here raises an active-long-running notice on the very
    /// first activity it sees, so any activity bump is observable. Killing mutation: moving the
    /// fold after `control.observe_event`.
    #[tokio::test]
    async fn a_watchdog_status_line_is_not_child_activity() {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        let mut state = DriveState::new(&opts, Some(armed()));
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::new(
            crate::exec::control::ResolvedControlConfig {
                enabled: true,
                active_notice_after_ms: 1,
                ..crate::exec::control::ResolvedControlConfig::default()
            },
            "run-1".to_string(),
            "worker".to_string(),
            None,
            None,
            0,
        );
        let action = handle_child_line(
            &status(1, "reviewing"),
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(matches!(action, LineAction::Continue));
        assert_eq!(control.activity_state(), None, "no activity was noted");
        assert!(
            progress.all_events.is_empty(),
            "not recorded as a child event either"
        );
        assert_eq!(
            state.watchdog_state.as_ref().map(|s| s.phase),
            Some(ChildWatchdogPhase::Reviewing),
            "but it WAS folded"
        );
        // Control: an ordinary event through the same path IS activity.
        handle_child_line(
            "{\"type\":\"turn_start\"}",
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            None,
        )
        .await;
        assert!(control.activity_state().is_some());
    }

    /// The active phase replaces an armed drain with the tail, and the idle phase that follows
    /// re-arms the drain. Pure state, no subprocess. Killing mutations: an active event that does
    /// not clear `final_drain_at`; an idle event that does not restart the drain after a clean stop.
    #[tokio::test]
    async fn reviewing_swaps_the_drain_for_the_tail_and_idle_swaps_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        let mut state = DriveState::new(&opts, Some(armed()));
        state.clean_terminal_stop = true;
        state.start_final_drain();
        assert!(state.final_drain_at.is_some());
        assert_eq!(
            state.fold_child_watchdog_line(&status(1, "reviewing")),
            Some(())
        );
        assert!(state.final_drain_at.is_none(), "the review holds the run");
        assert!(state.watchdog_tail_at.is_some(), "under the tail");
        assert_eq!(state.fold_child_watchdog_line(&status(2, "idle")), Some(()));
        assert!(state.watchdog_tail_at.is_none());
        assert!(state.final_drain_at.is_some(), "a finished review drains");
        assert_eq!(
            state.fold_child_watchdog_line(&status(1, "reviewing")),
            Some(()),
            "a rejected (not newer) status line is still consumed, never folded as activity"
        );
        assert!(state.watchdog_tail_at.is_none(), "and it changed nothing");
        assert_eq!(
            state.fold_child_watchdog_line("{\"type\":\"turn_start\"}"),
            None
        );
    }

    // ---- SUBA-119: model verification (`execution.ts:1096-1103` @v0.71.0) -----------------------

    /// An assistant `message_end` NDJSON line reporting `model`, optionally with a `toolCall` part.
    fn assistant_message_end(model: &str, tool_call: bool) -> String {
        let mut content = vec![serde_json::json!({ "type": "text", "text": "done" })];
        if tool_call {
            content.push(serde_json::json!({ "type": "toolCall", "name": "bash", "id": "t1" }));
        }
        serde_json::json!({
            "type": "message_end",
            "message": { "role": "assistant", "model": model, "content": content },
        })
        .to_string()
    }

    /// Feed one line through `handle_child_line` with verification armed for `expected`, and report
    /// whatever error was latched.
    async fn verification_error_for(line: &str, expected: Option<&str>) -> Option<String> {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        let mut state = DriveState::new(&opts, None);
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::new(
            crate::exec::control::ResolvedControlConfig::default(),
            "run-1".to_string(),
            "worker".to_string(),
            None,
            None,
            0,
        );
        handle_child_line(
            line,
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            expected,
        )
        .await;
        state.model_verification_error
    }

    /// SUBA-119 test (b). A child whose assistant `message_end` reports a model that is not the launch
    /// candidate latches the verification error. Nothing in this crate compared the two before.
    ///
    /// The models used are real entries of the credential-blind catalog
    /// `crate::extension::models::registry_available_models` reads, because clause `:20` turns the
    /// check off for an EMPTY registry and clause `:26` needs the expected id to be a known `fullId`.
    #[tokio::test]
    async fn a_child_reporting_a_different_model_latches_the_verification_error() {
        let available = crate::extension::models::registry_available_models();
        let expected = available
            .first()
            .map(|e| e.full_id().to_string())
            .expect("the builtin catalog is not empty");
        let observed = available
            .iter()
            .map(|e| e.full_id().to_string())
            .find(|id| {
                crate::exec::spawn_plan::split_known_thinking_suffix(id).0
                    != crate::exec::spawn_plan::split_known_thinking_suffix(&expected).0
                    && !id.ends_with(&format!(
                        "/{}",
                        crate::exec::spawn_plan::split_known_thinking_suffix(&expected).0
                    ))
            })
            .expect("the catalog ships more than one distinct model");

        let err = verification_error_for(
            &assistant_message_end(&observed, false),
            Some(expected.as_str()),
        )
        .await;
        let err = err.expect("a reported model that is not the candidate must be an error");
        assert!(
            err.starts_with(
                "model_verification_failed: native Pi child reported a different model"
            ),
            "{err}"
        );
        assert!(err.contains(&format!("Expected '{expected}'")), "{err}");
        assert!(err.contains(&format!("observed '{observed}'")), "{err}");
    }

    /// SUBA-119 — the row's Verify sentence, second half: *"the same run with
    /// `modelResponseAliases: {'prov/model': ['other/model']}` succeeds."* The SAME child stream
    /// that latches the error above latches nothing once the observed id is declared under the
    /// launch candidate, because `RunOptions::model_response_aliases` — now fed from `config.json`
    /// — reaches the check.
    #[tokio::test]
    async fn a_declared_alias_on_the_run_options_clears_the_verification_error() {
        let available = crate::extension::models::registry_available_models();
        let expected = available
            .first()
            .map(|e| e.full_id().to_string())
            .expect("the builtin catalog is not empty");
        let observed = available
            .iter()
            .map(|e| e.full_id().to_string())
            .find(|id| {
                crate::exec::spawn_plan::split_known_thinking_suffix(id).0
                    != crate::exec::spawn_plan::split_known_thinking_suffix(&expected).0
                    && !id.ends_with(&format!(
                        "/{}",
                        crate::exec::spawn_plan::split_known_thinking_suffix(&expected).0
                    ))
            })
            .expect("the catalog ships more than one distinct model");
        let line = assistant_message_end(&observed, false);

        // Premise: without the declaration this exact stream fails the run.
        assert!(
            verification_error_for(&line, Some(expected.as_str()))
                .await
                .is_some(),
            "premise: the undeclared case must still be an error"
        );

        let dir = tempfile::tempdir().unwrap();
        let mut opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        opts.model_response_aliases = Some(std::collections::BTreeMap::from([(
            crate::exec::spawn_plan::split_known_thinking_suffix(&expected)
                .0
                .to_string(),
            vec![observed.clone()],
        )]));
        let mut state = DriveState::new(&opts, None);
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::new(
            crate::exec::control::ResolvedControlConfig::default(),
            "run-1".to_string(),
            "worker".to_string(),
            None,
            None,
            0,
        );
        handle_child_line(
            &line,
            &mut state,
            &mut progress,
            &mut control,
            &opts,
            None,
            Some(expected.as_str()),
        )
        .await;
        assert_eq!(
            state.model_verification_error, None,
            "a declared alias must clear the run failure"
        );
    }

    /// SUBA-119 test (c). The same stream with a `toolCall` content part latches NOTHING — upstream's
    /// `!hasToolCall` gate (`execution.ts:1099`). A message that is making a tool call is mid-turn.
    #[tokio::test]
    async fn a_message_end_carrying_a_tool_call_is_not_verified() {
        let available = crate::extension::models::registry_available_models();
        let expected = available
            .first()
            .map(|e| e.full_id().to_string())
            .expect("catalog non-empty");
        let observed = available
            .iter()
            .map(|e| e.full_id().to_string())
            .find(|id| id != &expected)
            .expect("more than one model");
        assert_eq!(
            verification_error_for(
                &assistant_message_end(&observed, true),
                Some(expected.as_str())
            )
            .await,
            None,
            "a tool-calling message is exempt from verification"
        );
    }

    /// SUBA-119 test (d). Verification OFF (`expected_model_for_verification: None`, which is what a
    /// parent-inherited launch produces via `!opts.model_override_from_parent` in `attempt_runner`)
    /// latches nothing, however different the reported model is.
    #[tokio::test]
    async fn verification_off_never_latches_an_error() {
        assert_eq!(
            verification_error_for(&assistant_message_end("nothing/like-it", false), None).await,
            None
        );
    }

    /// The matching case: a child reporting the model it was launched with is silent.
    #[tokio::test]
    async fn a_child_reporting_the_launch_candidate_latches_nothing() {
        let available = crate::extension::models::registry_available_models();
        let expected = available
            .first()
            .map(|e| e.full_id().to_string())
            .expect("catalog non-empty");
        assert_eq!(
            verification_error_for(
                &assistant_message_end(&expected, false),
                Some(expected.as_str())
            )
            .await,
            None
        );
    }

    /// A NON-assistant `message_end` is not verified, and neither is a message with no `model`:
    /// upstream's check lives inside the assistant arm, under `if (evt.message.model)`
    /// (`execution.ts:1096`).
    #[tokio::test]
    async fn a_user_message_end_and_a_model_less_one_are_both_exempt() {
        let user = serde_json::json!({
            "type": "message_end",
            "message": { "role": "user", "model": "nothing/like-it", "content": [] },
        })
        .to_string();
        assert_eq!(
            verification_error_for(&user, Some("prov/model")).await,
            None
        );

        let no_model = serde_json::json!({
            "type": "message_end",
            "message": { "role": "assistant", "content": [] },
        })
        .to_string();
        assert_eq!(
            verification_error_for(&no_model, Some("prov/model")).await,
            None
        );
    }

    // ---- SUBA-118: compaction evidence (`execution.ts:970-999` @v0.71.0) ------------------------

    /// Feed a sequence of raw NDJSON lines through `handle_child_line` and report the two compaction
    /// markers as they stand afterwards.
    async fn compaction_markers_after(lines: &[String]) -> (bool, bool) {
        let dir = tempfile::tempdir().unwrap();
        let opts = crate::exec::testsupport::base_opts(dir.path(), &["m"]);
        let mut state = DriveState::new(&opts, None);
        let mut progress = AgentProgress::default();
        let mut control = crate::exec::control::ControlMonitor::new(
            crate::exec::control::ResolvedControlConfig::default(),
            "run-1".to_string(),
            "worker".to_string(),
            None,
            None,
            0,
        );
        for line in lines {
            handle_child_line(
                line,
                &mut state,
                &mut progress,
                &mut control,
                &opts,
                None,
                None,
            )
            .await;
        }
        (state.compaction_started, state.after_compaction_settlement)
    }

    fn ev(value: serde_json::Value) -> String {
        value.to_string()
    }

    /// SUBA-118 test (d). `compaction_start` then `agent_settled` marks the settlement; the same
    /// sequence with an intervening `compaction_end{willRetry:true}` does NOT
    /// (`execution.ts:970-974,996-999`).
    ///
    /// This is the distinction the whole abort-recovery exemption rests on: without it, a
    /// terminal-drain cleanup signal would veto every compaction resume
    /// (`abort-recovery.ts:109`).
    #[tokio::test]
    async fn a_compaction_that_will_retry_does_not_count_as_a_settled_compaction() {
        let start = ev(serde_json::json!({ "type": "compaction_start", "reason": "context" }));
        let settled = ev(serde_json::json!({ "type": "agent_settled" }));
        let end_retry = ev(
            serde_json::json!({ "type": "compaction_end", "reason": "context", "willRetry": true }),
        );

        assert_eq!(
            compaction_markers_after(&[start.clone(), settled.clone()]).await,
            (true, true),
            "compaction_start then agent_settled marks the settlement"
        );
        assert_eq!(
            compaction_markers_after(&[start.clone(), end_retry, settled.clone()]).await,
            (false, false),
            "a compaction that will retry clears both markers before the settlement"
        );
        // …and with no compaction at all, an `agent_settled` marks nothing.
        assert_eq!(compaction_markers_after(&[settled]).await, (false, false));
    }

    /// `:977-980` — `agent_start` and `auto_retry_start` each clear BOTH markers: a child starting
    /// fresh, or backing off into a retry, is not settling after a compaction.
    #[tokio::test]
    async fn a_fresh_agent_start_or_auto_retry_clears_the_compaction_markers() {
        let start = ev(serde_json::json!({ "type": "compaction_start", "reason": "context" }));
        let settled = ev(serde_json::json!({ "type": "agent_settled" }));

        for clearing in [
            ev(serde_json::json!({ "type": "agent_start" })),
            ev(serde_json::json!({
                "type": "auto_retry_start",
                "attempt": 1,
                "maxAttempts": 3,
                "delayMs": 100,
            })),
        ] {
            assert_eq!(
                compaction_markers_after(&[start.clone(), settled.clone(), clearing.clone(),])
                    .await,
                (false, false),
                "{clearing} must clear both markers"
            );
        }
    }

    /// `:971-974` gates on `willRetry === true` alone — a compaction that ENDED without retrying
    /// leaves the marker standing, so the settlement that follows still counts.
    #[tokio::test]
    async fn a_compaction_end_without_will_retry_leaves_the_marker_standing() {
        let start = ev(serde_json::json!({ "type": "compaction_start", "reason": "context" }));
        let end = ev(serde_json::json!({
            "type": "compaction_end",
            "reason": "context",
            "willRetry": false,
        }));
        let settled = ev(serde_json::json!({ "type": "agent_settled" }));
        assert_eq!(
            compaction_markers_after(&[start, end, settled]).await,
            (true, true)
        );
    }
}
