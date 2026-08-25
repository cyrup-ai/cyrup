//! Given one already-spawned child's stdout NDJSON event stream, fold it into
//! progress/output/acceptance state until the attempt settles — [`drive_attempt`] and its
//! [`crate::exec::drive_attempt::DriveOutcome`]. Split out of `exec/mod.rs`'s own "SubagentSpawner" section (the per-attempt
//! drive-loop third of it).

use std::time::Duration;


use crate::exec::ndjson::SubagentEvent;
use crate::exec::output::{
    is_terminal_assistant_stop,
    message_end_has_error_message,
};
use crate::spawn::SpawnedChild;
use crate::exec::progress::AgentProgress;
use crate::exec::agent_config::RunOptions;


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
    if let crate::exec::ndjson::SubagentEvent::ToolExecutionStart { tool_name, args, .. } = event
        && tool_name == "contact_supervisor"
    {
        let reason = args.get("reason").and_then(serde_json::Value::as_str).unwrap_or_default();
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
pub(crate) async fn drive_attempt(
    mut child: SpawnedChild,
    progress: &mut AgentProgress,
    opts: &RunOptions,
    deadline_sleep: Option<tokio::time::Sleep>,
    control: &mut crate::exec::control::ControlMonitor,
) -> DriveOutcome {
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

    // Armed on the FIRST terminal assistant stop; once the grace window elapses without the child
    // exiting, the child is force-drained. `clean_terminal_stop` accumulates across every terminal
    // stop (pi's `||=`) for `forcedDrainAfterFinalSuccess`.
    let mut final_drain_at: Option<tokio::time::Instant> = None;
    // SUBA-S06: armed when the child is reaped with stdout still open; expiring it ends the read
    // loop so the post-loop `wait_final_drain()` can report the already-known exit status.
    let mut exit_drain_at: Option<tokio::time::Instant> = None;
    let mut clean_terminal_stop = false;
    // pi's `agentSettledReceived` (`execution.ts:595,862,1080` @v0.43.0): the child announced the WHOLE run
    // settled. Like `clean_terminal_stop` it is a "the child finished on purpose" witness, so a
    // forced drain after it is still coerced to success.
    let mut agent_settled = false;
    // R-SA-037: set once the child's NDJSON shows a blocking `contact_supervisor` ask; the ask is
    // surfaced via `spawn_clarify` exactly once (the guard below), and this flag then rides out to
    // the attempt's `detached` outcome (bypassing acceptance; the ladder does not advance past it).
    let mut detached_seen = false;
    // SUBA-008 — pi's four `updateTurnBudget` locals (`turnBudgetSoftReached` plus the three
    // `result.turnBudget*` fields, `execution.ts:483`/`:567-569`/`:759-782`), gathered into one
    // value. Unarmed (and therefore inert on every path below) unless this run declared a budget.
    let mut turn_budget = crate::exec::turn_budget::TurnBudgetTracker::new(
        opts.turn_budget,
        opts.enforce_hard_turn_limit,
    );

    loop {
        let deadline_arm = async {
            match deadline_sleep.as_mut().as_pin_mut() {
                Some(sleep) => sleep.await,
                None => std::future::pending::<()>().await,
            }
        };
        // A fresh `sleep_until` against the fixed grace instant each iteration is correct: it
        // always resolves at the same absolute time regardless of how often it is reconstructed,
        // and reduces to `pending()` (never fires) until the window is armed.
        let final_drain_arm = async {
            match final_drain_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };

        // SUBA-S06: same fixed-instant reconstruction as `final_drain_arm` above, and `pending()`
        // (never fires) until the child is actually reaped.
        let exit_drain_arm = async {
            match exit_drain_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: false,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            () = interrupt.cancelled() => {
                // pi `execution.ts:1090`: a soft interrupt CLEARS the activity state, so a
                // needs-attention notice that was raised (and is still sitting in the parent's
                // debounce window) fails its actionability re-check rather than landing in the
                // transcript for a run the caller has already deliberately paused.
                control.clear_activity_state();
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: true,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            () = deadline_arm => {
                // R-SA-036: timeout is a SOFT interrupt, not an immediate hard kill — it still
                // walks the full SIGINT->SIGTERM->SIGKILL ladder via `terminate`, exactly like
                // cancel/interrupt above; what makes it a timeout rather than a plain
                // cancellation is the `timed_out: true` flag, which is what `run_fallback_ladder`
                // (R-SA-036/6.3.2) actually branches on to stop the ladder outright.
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: true,
                    interrupted: false,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            step = child.next_event_or_exit() => {
                match step {
                    crate::spawn::ChildStep::Line(Ok(line)) => {
                        // NOTE: the raw NDJSON envelope deliberately does NOT enter
                        // `progress.recent_output` — pi appends only EXTRACTED text, from an
                        // assistant `message_end`'s content and a finished tool call's result, and
                        // `AgentProgress::record_event` does exactly that a few lines below. A raw
                        // line here would put an unrenderable (and, before
                        // `RECENT_OUTPUT_LINE_CHARS`, unbounded) JSON blob on the very field
                        // `SingleResult::progress` publishes as pi's `recentOutput`.
                        // Live-telemetry tee (pi's child-event pump, `subagent-runner.ts:1430`):
                        // hand the raw NDJSON line to the background runner's sink, if one is
                        // installed, BEFORE this module parses/folds it — so the runner folds it
                        // into `status.json` live without this module depending on `background`.
                        if let Some(sink) = &opts.live_events {
                            sink.emit(&line);
                        }
                        // `SpawnedChild::next_event_or_exit` tees and hands back the raw line
                        // without parsing it — `exec::ndjson::parse_line` is the crate's ONE
                        // NDJSON parse, so each child stdout line is deserialized exactly once,
                        // here, against the single `SubagentEvent` schema (final-output
                        // extraction, R-SA-029; completion-guard scanning, R-SA-034).
                        if let Some(event) = crate::exec::ndjson::parse_line(&line) {
                            // Final-stop grace-drain (pi `startFinalDrain`, execution.ts:584-605):
                            // open the grace window on the FIRST terminal assistant stop and track
                            // whether ANY terminal stop was clean (no errorMessage) for
                            // `forcedDrainAfterFinalSuccess`.
                            let terminal_stop = is_terminal_assistant_stop(&event);
                            if terminal_stop {
                                clean_terminal_stop =
                                    clean_terminal_stop || !message_end_has_error_message(&event);
                            }
                            if matches!(event, crate::exec::ndjson::SubagentEvent::AgentSettled) {
                                agent_settled = true;
                            }
                            // pi `applyChildLifecycle(projectChildLifecycle(evt))` — run for EVERY
                            // event (`execution.ts:844`), plus the terminal-stop form at `:947`.
                            // The three arms are: `agent_end{willRetry:true}` DISARMS the window
                            // (the child is about to retry — force-killing it there kills a run
                            // that is still working); `agent_settled` and a terminal assistant stop
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
                                crate::exec::child_protocol::ChildLifecycleAction::CancelDrain => {
                                    final_drain_at = None;
                                }
                                crate::exec::child_protocol::ChildLifecycleAction::StartDrain => {
                                    if final_drain_at.is_none() {
                                        final_drain_at = Some(
                                            tokio::time::Instant::now()
                                                + Duration::from_millis(FINAL_STOP_GRACE_MS),
                                        );
                                    }
                                }
                                crate::exec::child_protocol::ChildLifecycleAction::None => {}
                            }
                            // R-SA-037 detach-trigger arm: a child's blocking `contact_supervisor`
                            // ask (`need_decision`/`interview`) surfaces the ask to the parent's
                            // human via the real `ClarifyChannel` (fired exactly once) and marks this
                            // attempt detached. The intercom answer routes back to the still-alive
                            // child over the BROKER (independent of this stdout pipe), so the loop
                            // keeps driving — it neither kills nor synchronously blocks on the child.
                            if !detached_seen
                                && let Some(prompt) = contact_supervisor_block_prompt(&event)
                            {
                                detached_seen = true;
                                if let Some(dispatch) = &opts.clarify {
                                    // Dropping the returned receiver does not cancel the ask (a human
                                    // may still be answering); it only means this loop does not itself
                                    // await the outcome — the child unblocks over the broker instead.
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
                            // pi `processLine` (`execution.ts:775-890`): every parsed child event
                            // is fresh activity for the control heuristics, and the tool-start /
                            // tool-result / assistant-turn folds feed the thresholds. Driven
                            // BEFORE `record_event` because that consumes the event by value.
                            control.observe_event(
                                &event,
                                crate::time::now_epoch_millis(),
                            );
                            // SUBA-008 — the two per-message inputs `updateTurnBudget` needs, read
                            // BEFORE `record_event` consumes the event by value (same reason the
                            // control fold above runs here).
                            let assistant_turn = is_assistant_message_end(&event);
                            let has_tool_call = message_end_has_tool_call(&event);
                            let terminal_structured_output_call = opts
                                .structured_output_schema
                                .is_some()
                                && is_sole_structured_output_tool_call(&event);
                            progress.record_event(event);

                            // pi `execution.ts:910-924`: an ASSISTANT `message_end` is one turn,
                            // and the budget is re-evaluated on it. `progress.turn_count()` is
                            // this port's `result.usage.turns` — pi keeps the two in lockstep
                            // (`:913-914`) and cyrup derives the one from the other rather than
                            // carrying a second counter that could drift.
                            if assistant_turn && turn_budget.is_armed() {
                                let turn_count = u64::from(progress.turn_count());
                                // pi's third argument: `hasToolCall || Boolean(progress.currentTool)`
                                // (`:924`) — tool work either STARTING on this very message or
                                // still in flight from an earlier one. This is what makes the
                                // deferral arm reachable.
                                let tool_work_active_or_starting =
                                    has_tool_call || progress.current_tool.is_some();
                                let effect = turn_budget.observe_assistant_turn(
                                    turn_count,
                                    terminal_stop || terminal_structured_output_call,
                                    tool_work_active_or_starting,
                                    false,
                                );
                                match effect {
                                    crate::exec::turn_budget::TurnBudgetEffect::None => {}
                                    crate::exec::turn_budget::TurnBudgetEffect::SoftNote(note) => {
                                        // pi `appendRecentOutput(progress, [turnBudgetSoftNote(...)])`
                                        // (`:769`) — the wrap-up request reaches the operator
                                        // through the run's own output tail, once.
                                        progress.append_recent_output(&note);
                                    }
                                    crate::exec::turn_budget::TurnBudgetEffect::Abort {
                                        message,
                                        soft_note,
                                    } => {
                                        if let Some(note) = soft_note {
                                            progress.append_recent_output(&note);
                                        }
                                        // pi `requestTurnBudgetAbort` (`:733-757`): SIGINT now,
                                        // SIGTERM 1 s later, SIGKILL 4 s after the SIGINT. That is
                                        // exactly this ladder with the two graces pinned — 1 s to
                                        // escalate off SIGINT and 3 s more to escalate off SIGTERM
                                        // lands the kill at t+4 s, upstream's own instant.
                                        //
                                        // [CYRUP-DELTA]: pi ARMS the two timers and lets the run
                                        // keep reading the child's stdout in the meantime, so a
                                        // child that wraps up inside the window still delivers its
                                        // final output; this ladder blocks the drive loop for the
                                        // same wall-clock window instead, because
                                        // `SpawnedChild::terminate` consumes the child and cyrup
                                        // has no seam that signals without taking it. The observed
                                        // outcome is the same on both timelines — the child either
                                        // dies on SIGINT (the ladder returns immediately) or is
                                        // escalated on upstream's schedule — but a late final
                                        // message written after the SIGINT is dropped here where
                                        // upstream would have read it, which is why the abort
                                        // message doubles as `final_output` below.
                                        let outcome = child
                                            .terminate_with_graces(
                                                &cancel,
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
                                        // `message` is not carried on the outcome: it is
                                        // recomputed verbatim from the tracker's own state by
                                        // `TurnBudgetTracker::terminal_note`, which is the single
                                        // place `run_attempt` reads it from, so there is exactly
                                        // one producer of upstream's string.
                                        debug_assert_eq!(
                                            turn_budget.terminal_note(),
                                            Some(
                                                crate::exec::turn_budget::TurnBudgetTerminalNote::Exceeded(
                                                    message.clone()
                                                )
                                            )
                                        );
                                        drop(message);
                                        return DriveOutcome {
                                            timed_out: false,
                                            interrupted: false,
                                            // NOT a forced drain: `forcedDrainAfterFinalSuccess`
                                            // must never coerce a turn-budget abort to exit 0.
                                            forced_termination: false,
                                            clean_terminal_stop,
                                            exit_status: outcome.map(|o| Some(o.status)),
                                            detached: detached_seen,
                                            agent_settled,
                                            protocol_error: None,
                                            turn_budget,
                                        };
                                    }
                                }
                            }
                        }
                    }
                    crate::spawn::ChildStep::ProtocolLimit(limit) => {
                        // pi `failProtocol` (`execution.ts:1026-1041`): the diagnostic becomes the
                        // run's error and the child is signalled down (upstream SIGTERM then, 3s
                        // later, SIGKILL — cyrup routes every forced termination through
                        // `terminate`'s own SIGINT->SIGTERM->SIGKILL ladder instead of inventing a
                        // second one). Nothing further can be read: the reader is permanently
                        // closed, so continuing to poll it would spin on `Eof`.
                        let outcome = child.terminate(&cancel).await;
                        return DriveOutcome {
                            timed_out: false,
                            interrupted: false,
                            // NOT a forced *drain*: upstream's `failProtocol` deliberately does not
                            // set `forcedTerminationSignal`, so the clean-drain coercion to exit 0
                            // cannot swallow a protocol failure. (It could not anyway — that
                            // coercion also requires no error, and this sets one.)
                            forced_termination: false,
                            clean_terminal_stop,
                            exit_status: outcome.map(|o| Some(o.status)),
                            detached: detached_seen,
                            agent_settled,
                            protocol_error: Some(limit),
                            turn_budget: turn_budget.clone(),
                        };
                    }
                    crate::spawn::ChildStep::Line(Err(_)) | crate::spawn::ChildStep::Eof => {
                        // Stdout EOF (child exited/closed stdout) or a genuine read fault — either
                        // way, stop reading and wait for the real exit status below.
                        break;
                    }
                    crate::spawn::ChildStep::Exited(_) => {
                        // SUBA-S06: the process is gone but stdout is STILL OPEN, because a
                        // surviving grandchild inherited the write end. The EOF this loop used to
                        // wait on can never arrive, and none of the other arms is guaranteed to
                        // fire either — `deadline_arm` only exists when the caller passed a
                        // timeout, `final_drain_arm` only after a terminal assistant stop the
                        // child never emitted, and the activity tick merely re-scores heuristics.
                        // So the tool call hung forever, spinning once a second.
                        //
                        // Do NOT break here: lines written before the exit may still be buffered
                        // in the pipe, and dropping them would trade a hang for silent output
                        // loss. Arm a bounded post-exit window instead and keep draining; the
                        // status itself is deliberately discarded because the post-loop
                        // `wait_final_drain()` re-reads it (the child is marked reaped, so that
                        // call returns immediately) and routes it through the ONE existing clean
                        // path — which is what keeps this a normal exit rather than a
                        // `forced_termination`.
                        if exit_drain_at.is_none() {
                            exit_drain_at = Some(
                                tokio::time::Instant::now()
                                    + Duration::from_millis(POST_EXIT_DRAIN_MS),
                            );
                        }
                    }
                }
            }
            () = exit_drain_arm => {
                // SUBA-S06: the reaped child's buffered stdout has had its beat; whatever still
                // holds the pipe open is not this run's problem. Break (never return) so the exit
                // status flows through the normal post-loop path as an ordinary clean exit.
                break;
            }
            () = final_drain_arm => {
                // The child emitted its terminal stop but did not exit within the grace window —
                // force-drain it through the real signal ladder (pi's SIGTERM->SIGKILL). Whether
                // this is coerced back to success (`forcedDrainAfterFinalSuccess`) is decided in
                // `run_attempt` from `forced_termination` + `clean_terminal_stop` + no error.
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: false,
                    forced_termination: true,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
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
            DriveOutcome {
                timed_out: false,
                interrupted: false,
                forced_termination: false,
                clean_terminal_stop,
                exit_status: Ok(Some(status)),
                detached: detached_seen,
                agent_settled,
                protocol_error: None,
                turn_budget: turn_budget.clone(),
            }
        }
        Ok(None) => {
            // The child closed stdout but did not exit within FINAL_DRAIN_TIMEOUT (R-SA-068) —
            // fall back to the real signal-escalation ladder. This is a forced termination too:
            // combined with a clean terminal stop and no error, `forcedDrainAfterFinalSuccess`
            // still coerces an otherwise-successful, merely-slow-to-teardown run to exit 0.
            let outcome = child.terminate(&cancel).await;
            DriveOutcome {
                timed_out: false,
                interrupted: false,
                forced_termination: true,
                clean_terminal_stop,
                exit_status: outcome.map(|o| Some(o.status)),
                detached: detached_seen,
                agent_settled,
                protocol_error: None,
                turn_budget: turn_budget.clone(),
            }
        }
        Err(err) => DriveOutcome {
            timed_out: false,
            interrupted: false,
            forced_termination: false,
            clean_terminal_stop,
            exit_status: Err(err),
            detached: detached_seen,
            agent_settled,
            protocol_error: None,
            turn_budget: turn_budget.clone(),
        },
    }
}
