//! The concrete production [`crate::exec::fallback::AttemptRunner`] implementor: spawns one real OS child process per
//! model-fallback attempt via [`crate::spawn::SpawnedChild::spawn`] — [`SpawnedChildAttemptRunner`] and its
//! richer [`crate::exec::attempt_runner::AttemptRecord`] payload. Split out of `exec/mod.rs`'s own "SubagentSpawner" section
//! (the process-spawning third of it). `structured_output_absent` lives here rather than in
//! [`crate::exec::drive_attempt`] because its only caller is this file's own `AttemptRunner`
//! impl, not `drive_attempt` itself.

use std::path::PathBuf;
use std::time::Duration;

use cyrup_core::{ModelId, Usage};

use crate::exec::acceptance::AcceptanceContract;
use crate::exec::fallback::{
    AttemptRunner, AttemptSignal, StartupEvidence, StartupOutcome,
    StartupRetryWait,
};
use crate::exec::output::{
    EMPTY_OUTPUT_ERROR, INTERRUPTED_FINAL_OUTPUT, detect_subagent_error,
    extract_final_output,
    trailing_assistant_error,
};
use crate::spawn::SpawnedChild;
use crate::exec::agent_config::{AgentConfig, RunOptions};
use crate::exec::drive_attempt::drive_attempt;
use crate::exec::progress::AgentProgress;
use crate::exec::spawn_plan::{build_attempt_spawn_plan_with_read_requirement, build_task_text};


/// The production [`AttemptRunner`] implementation: spawns a REAL child OS process per
/// model-fallback attempt via [`SpawnedChild::spawn`] (func-SA §1.1's mandated mechanism),
/// consumes its NDJSON stdout through [`crate::exec::ndjson::consume_stdout`], folds R-SA-027/028 progress,
/// and races the whole attempt against `opts.cancel`/`opts.interrupt`/`opts.deadline_at` before
/// returning an [`AttemptSignal`] plus this attempt's own richer [`AttemptRecord`] payload.
pub(crate) struct SpawnedChildAttemptRunner<'a> {
    pub(crate) agent: &'a AgentConfig,
    pub(crate) task: &'a str,
    pub(crate) opts: &'a RunOptions,
    pub(crate) contract: &'a AcceptanceContract,
    /// Scratch directory for `@<tempfile>` task-text overflow (R-SA-047) and the per-attempt
    /// `.jsonl` tee artifact (R-SA-058).
    pub(crate) scratch_dir: PathBuf,
    /// The lazy `<available_skills>` pointer block (T5, C4), resolved ONCE per run by [`crate::exec::run_sync`]
    /// and stable across every fallback attempt (skill resolution never depends on the model), so it
    /// is built once and reused rather than re-resolved per attempt. Empty when no skills apply.
    pub(crate) skill_injection: String,
    pub(crate) attempt_index: u32,
    /// SUBA-S01: the run's structured-output capture runtime (pi `StructuredOutputRuntime`), or
    /// `None` when the step declared no `outputSchema`. Created ONCE by [`crate::exec::run_sync`] and shared
    /// across every fallback attempt so a retry cannot capture into a different file than the one
    /// read back; its two paths become the child's
    /// [`crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV`]/`..._CAPTURE_ENV` overlay.
    pub(crate) structured_runtime: Option<crate::exec::structured::StructuredOutputRuntime>,
    /// SUBA-014: pi's `requireReadTool` (`runs/shared/pi-args.ts:118`), derived ONCE per run from
    /// "did any skill actually resolve" — `Boolean(shared.resolvedSkillNames?.length)`
    /// (`runs/foreground/execution.ts:322,357`). Stable across fallback attempts for the same
    /// reason [`Self::skill_injection`] is: skill resolution never depends on the model.
    pub(crate) require_read_tool: bool,
}

/// The richer per-attempt payload [`SpawnedChildAttemptRunner::run_attempt`] returns alongside its
/// [`AttemptSignal`] — everything `run_sync`'s completion path (structured-output validation,
/// completion guard, acceptance evaluation, R-SA-033's ordering) needs from the WINNING attempt,
/// without `fallback::run_fallback_ladder` itself needing to know this shape at all (it only ever
/// touches [`AttemptSignal`]).
pub(crate) struct AttemptRecord {
    pub(crate) progress: AgentProgress,
    pub(crate) final_output: Option<String>,
    /// A soft interrupt (`RunOptions.interrupt`) fired on this attempt — pi's paused-success
    /// semantics (`execution.ts:722-761`, T3 group A). Carried on the runner's own per-attempt
    /// payload (not on [`AttemptSignal`], which this crate does not own) so `run_sync` can flip the
    /// terminal [`crate::exec::SingleResult`] to `interrupted: true`, exit 0, cleared error. An interrupted
    /// attempt reports `AttemptSignal { success: true, exit_code: Some(0), .. }`, so the ladder
    /// stops on it exactly like an ordinary success.
    pub(crate) interrupted: bool,
    /// This attempt's live-control state machine, carried out of the ladder so `run_sync` can (a)
    /// raise the post-settlement completion-guard notice against the WINNING attempt's own dedup
    /// set — pi's `emitControlEvent` at `execution.ts:417-423` is a local of the same
    /// `runSingleAttempt` scope — and (b) fold its raised events onto
    /// [`crate::exec::SingleResult::control_events`] (pi `result.controlEvents = allControlEvents`, `:1314`).
    pub(crate) control: crate::exec::control::ControlMonitor,
    /// SUBA-008 — this attempt's turn-budget latch, carried out of [`drive_attempt`] so `run_sync`
    /// can fold pi's terminal composition (`execution.ts:1251-1258`) onto the delivered output and
    /// publish `turnBudget`/`turnBudgetExceeded`/`wrapUpRequested` on the [`crate::exec::SingleResult`].
    ///
    /// Unarmed on every path that never reached the drive loop (a spawn failure) and on every run
    /// that declared no budget.
    pub(crate) turn_budget: crate::exec::turn_budget::TurnBudgetTracker,
}

#[async_trait::async_trait]
impl AttemptRunner for SpawnedChildAttemptRunner<'_> {
    type Attempt = AttemptRecord;

    async fn run_attempt(
        &mut self,
        model: &ModelId,
        attempt_note: Option<&str>,
    ) -> (AttemptSignal, Self::Attempt) {
        let mut progress = AgentProgress {
            // pi's `startTime` local, captured at the very top of `runSingleAttempt` — before the
            // spawn plan is even built — and read back as `progress.durationMs = Date.now() -
            // startTime` at every settle site (`runs/foreground/execution.ts:1177`).
            started_at: Some(std::time::Instant::now()),
            ..AgentProgress::default()
        };
        // pi seeds the ring with the ladder's attempt notes at construction time
        // (`recentOutput: [...shared.attemptNotes]`, `runs/foreground/execution.ts:366`); this
        // crate's ladder hands them down one at a time, so each is appended as it arrives.
        if let Some(note) = attempt_note {
            progress.append_recent_output(note);
            // ...and onto the LIVE surface, which is the only place a user can actually read it:
            // this attempt's own `progress` is compacted (`recent_output` emptied) before it
            // becomes `SingleResult::progress`, exactly as pi's `compactCompletedProgress` does.
            // pi has no second hop here only because its live stream and its settled snapshot are
            // the same mutable object; cyrup's live surface folds the child's NDJSON, which a
            // parent-side note never appears on. See [`LiveEventSink::emit_note`].
            if let Some(sink) = &self.opts.live_events {
                sink.emit_note(note);
            }
        }

        // pi `runSingleAttempt`'s control locals (`execution.ts:245-246` @v0.34.0): the attempt's own
        // start instant, its resolved control config (`options.controlConfig ?? DEFAULT_CONTROL_CONFIG`)
        // and its per-attempt dedup/record state. Built here — before the spawn plan — so every
        // early-return path below still hands a (trivially empty) monitor back to `run_sync`
        // rather than the ladder losing the field entirely.
        let mut control = crate::exec::control::ControlMonitor::new(
            self.opts.control_config.clone().unwrap_or_default(),
            self.opts
                .run_id
                .as_ref()
                .map(|id| id.as_str().to_string())
                .unwrap_or_else(|| self.agent.name.clone()),
            self.agent.name.clone(),
            self.opts
                .child_index
                .and_then(|index| u32::try_from(index).ok()),
            self.opts.on_control_event.clone(),
            crate::time::now_epoch_millis(),
        );

        let task_text =
            build_task_text(self.agent, self.task, self.opts, self.contract, &self.skill_injection);

        // R-SA-054/055/056 (SAFETY-CRITICAL, C15): the CHILD about to be spawned is one recursion
        // hop deeper than THIS process, so its env overlay MUST carry the incremented envelope —
        // `next_envelope(parent, agent_max)` = `{ current_depth: parent.current_depth + 1,
        // max_depth: min(parent.max_depth, agent.max_subagent_depth) }` — never `self.agent.depth`
        // (the parent's OWN envelope) verbatim. Passing the parent envelope through unchanged (the
        // prior bug) meant every descendant inherited depth 0 and the ceiling check
        // (`run_sync`'s `is_blocked`) never tripped across the subprocess boundary, so recursion
        // could run unbounded. This mirrors pi's `getSubagentDepthEnv(maxSubagentDepth)`
        // (`shared/types.ts:1046`, `recursion-guard.test.ts:210-257`), which likewise increments
        // the inherited `PI_SUBAGENT_DEPTH` and applies the tighter per-agent max before rendering
        // the child's spawn env. The parent-side gate (`run_sync`'s own `is_blocked(&agent.depth)`,
        // Step 0) still guards whether THIS process may spawn at all; this line is what makes the
        // NEXT process's own Step-0 gate see a truthful, incremented depth.
        let child_depth =
            crate::spawn::depth::next_envelope(&self.agent.depth, self.agent.max_subagent_depth);

        let plan = match build_attempt_spawn_plan_with_read_requirement(
            self.agent,
            model,
            &task_text,
            self.opts,
            child_depth,
            &self.scratch_dir,
            self.structured_runtime.as_ref(),
            self.require_read_tool,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                return (
                    AttemptSignal {
                        success: false,
                        exit_code: None,
                        error: Some(err.to_string()),
                        usage: Usage::default(),
                        timed_out: false,
                        detached: false,
                        startup: StartupEvidence::default(),
                    },
                    AttemptRecord {
                        turn_budget: Default::default(),
                        progress,
                        final_output: None,
                        interrupted: false,
                        control,
                    },
                );
            }
        };

        let jsonl_path = self
            .scratch_dir
            .join(format!("attempt-{}.jsonl", self.attempt_index));
        self.attempt_index += 1;

        // SUBA-045: taken off the plan BEFORE `plan.spec` is moved into the spawn, and read back in
        // the close-handler chain below (pi's `toolDiagnosticPath` local, `execution.ts:1072`).
        //
        // [CYRUP-DELTA] pi mkdtemps a FRESH dir per attempt (`pi-args.ts:603-604`), so its
        // diagnostic path is unique per attempt by construction. cyrup's attempts share one
        // `scratch_dir`, so the file is cleared HERE, immediately before the spawn, to restore the
        // same guarantee: a child that dies before `agent_start` — the one case where it never gets
        // to write or delete the file itself — must not inherit the previous model attempt's
        // verdict. Without this, the model-fallback ladder could attribute attempt N's missing
        // tools to attempt N+1's startup crash.
        let tool_diagnostic_path = plan.tool_diagnostic_path;
        if let Some(path) = tool_diagnostic_path.as_deref() {
            let _ = std::fs::remove_file(path);
        }

        let mut child = match SpawnedChild::spawn(plan.spec, &jsonl_path).await {
            Ok(child) => child,
            Err(err) => {
                return (
                    AttemptSignal {
                        success: false,
                        exit_code: None,
                        error: Some(err.to_string()),
                        usage: Usage::default(),
                        timed_out: false,
                        detached: false,
                        startup: StartupEvidence::default(),
                    },
                    AttemptRecord {
                        turn_budget: Default::default(),
                        progress,
                        final_output: None,
                        interrupted: false,
                        control,
                    },
                );
            }
        };

        // Move the child's stderr reader out BEFORE `drive_attempt` consumes the child, so its
        // trailing diagnostic output can be surfaced into the run's error on a non-zero exit (pi
        // `execution.ts:686`). `drive_attempt` reads only stdout; the orphaned reader is drained to
        // EOF below (in the non-zero-exit branch), once the child is dead and its closed write end
        // guarantees a prompt EOF.
        let stderr_reader = child.take_stderr();

        let deadline_sleep = self
            .opts
            .deadline_at
            .map(|instant| tokio::time::sleep_until(tokio::time::Instant::from_std(instant)));
        let outcome =
            drive_attempt(child, &mut progress, self.opts, deadline_sleep, &mut control).await;

        // --- Interrupt: paused-success (pi `execution.ts:722-761`, T3 group A bug fix). A soft
        // interrupt is NOT a failure: it terminates the ladder with exit 0, a CLEARED error, and
        // the "paused" sentinel output, recorded under its own flag rather than folded into
        // exit-1/timed-out. pi returns from `runSingleAttempt` here BEFORE any exit-code
        // re-diagnosis, so this branch mirrors that early return exactly. ---
        if outcome.interrupted {
            return (
                AttemptSignal {
                    success: true,
                    exit_code: Some(0),
                    error: None,
                    usage: progress.usage.clone(),
                    timed_out: false,
                    detached: outcome.detached,
                    startup: StartupEvidence::default(),
                },
                AttemptRecord {
                    // SUBA-008: an interrupted attempt still reports the budget it ran under.
                    turn_budget: outcome.turn_budget.clone(),
                    progress,
                    final_output: Some(INTERRUPTED_FINAL_OUTPUT.to_string()),
                    interrupted: true,
                    control,
                },
            );
        }

        let (raw_exit_code, spawn_error, process_signal) = match &outcome.exit_status {
            Ok(Some(status)) => (status.code(), None, process_signal_name(status)),
            Ok(None) => (None, None, None), // terminated via signal escalation (timeout/cancel)
            Err(err) => (None, Some(err.to_string()), None),
        };

        let final_output = extract_final_output(&progress.message_end_events);

        // --- Timeout terminates the ladder outright (R-SA-036); its own flag is what
        // `run_fallback_ladder` branches on. Kept as a distinct early exit so the exit-0
        // re-diagnosis chain below never runs against a timed-out attempt. ---
        if outcome.timed_out {
            return (
                AttemptSignal {
                    success: false,
                    exit_code: Some(raw_exit_code.unwrap_or(1)),
                    error: spawn_error.or_else(|| Some("subagent attempt timed out".to_string())),
                    usage: progress.usage.clone(),
                    timed_out: true,
                    detached: outcome.detached,
                    startup: StartupEvidence::default(),
                },
                AttemptRecord {
                    // SUBA-008: pi's timeout arm wins the terminal composition outright
                    // (`execution.ts:1241`), but the state is still published.
                    turn_budget: outcome.turn_budget.clone(),
                    progress,
                    final_output,
                    interrupted: false,
                    control,
                },
            );
        }

        // --- Exit-0 re-diagnosis (pi `execution.ts:684-790`), in pi's exact order. ---

        // (a) The trailing, still-uncleared assistant `errorMessage` (pi close-handler
        //     `execution.ts:684` sets `result.error = assistantError`).
        // (a.0) The protocol-output-limit diagnostic outranks everything: pi's `failProtocol` sets
        //     `result.error` at the moment the cap trips, and the close handler only fills in a
        //     `closeError` when `result.error` is still unset (`execution.ts:1099`).
        // (a.-1) SUBA-008 — a turn-budget abort sets `result.error = message` at the moment it
        //     fires (`execution.ts:740`), i.e. strictly BEFORE the close handler runs, and the
        //     close handler only fills in a `closeError` when `result.error` is still unset
        //     (`:1099`). So the abort message outranks every diagnosis below it — including the
        //     child's own trailing apology, which is exactly what a child that was signalled
        //     mid-sentence tends to produce.
        //
        //     It cannot collide with the protocol-limit diagnostic below: the drive loop returns
        //     on whichever of the two fires first, so at most one is ever set.
        let mut error = match outcome.turn_budget.terminal_note() {
            Some(crate::exec::turn_budget::TurnBudgetTerminalNote::Exceeded(message)) => {
                Some(message)
            }
            _ => None,
        };
        if error.is_none() {
            error = outcome
                .protocol_error
                .as_ref()
                .map(crate::exec::child_protocol::format_protocol_output_limit);
        }
        if error.is_none() {
            error = spawn_error;
        }
        // (a.1) SUBA-045 — the child tool-availability diagnostic, in pi's exact rank: `closeError =
        //     result.error ?? toolDiagnosticError ?? assistantError` (`execution.ts:1079`). It sits
        //     ABOVE the trailing assistant error deliberately, and that ordering is the whole point
        //     of the item: a child told to use a tool its host never registered produces a
        //     perfectly ordinary model apology, and the apology would otherwise become the run's
        //     error and hide the cause. The file exists only when something was actually missing
        //     (the child DELETES it otherwise), so this is silent on every healthy run.
        if error.is_none() {
            error = crate::exec::tool_availability::read_child_tool_diagnostic_error(
                tool_diagnostic_path.as_deref(),
            );
        }
        if error.is_none() {
            error = trailing_assistant_error(&progress.all_events);
        }

        // (b) `forcedDrainAfterFinalSuccess` (pi `execution.ts:1080`): a child that emitted a CLEAN
        //     terminal stop but had to be force-drained (held stdout open past the grace window)
        //     is coerced to exit 0, not treated as a forced-kill failure.
        //     pi's witness is `(cleanTerminalAssistantStopReceived || agentSettledReceived)`
        //     (`execution.ts:1080`) — a child that announced `agent_settled` finished on purpose
        //     just as much as one that emitted a clean terminal assistant stop, and before
        //     `agent_settled` was parsed at all this crate could only ever see the second half.
        let forced_drain_after_final_success = outcome.forced_termination
            && (outcome.clean_terminal_stop || outcome.agent_settled)
            && error.is_none();

        // (b.1) Surface the child's trailing stderr as the error on a non-zero (or signal-death)
        //     exit, when nothing richer was already diagnosed and this is not a clean forced-drain
        //     success (pi `execution.ts:686`: `if (code !== 0 && stderrBuf.trim() && !result.error
        //     && !forcedDrainAfterFinalSuccess) result.error = stderrBuf.trim()`). `raw_exit_code !=
        //     Some(0)` is pi's `code !== 0` (true for a non-zero code AND for a signal-death `null`
        //     code). Drained here, once the child is dead so its closed write end EOFs the orphaned
        //     reader promptly — never during the read loop (stderr is not protocol data, R-SA-046).
        if error.is_none() && !forced_drain_after_final_success && raw_exit_code != Some(0) {
            let stderr_text = stderr_reader.drain_to_string().await;
            let trimmed = stderr_text.trim();
            if !trimmed.is_empty() {
                error = Some(trimmed.to_string());
            }
        }

        // (c) The forced/final exit code (pi `execution.ts:689`): a forced-termination or a
        //     signal-death (no numeric code) attributes exit 1 unless the clean-drain coercion
        //     above applies; a normal exit keeps its own code (defaulting to 0).
        let mut exit_code: i32 = if forced_drain_after_final_success {
            0
        } else if outcome.forced_termination || raw_exit_code.is_none() {
            raw_exit_code.unwrap_or(1)
        } else {
            raw_exit_code.unwrap_or(0)
        };

        // (d) A set error flips a zero exit to failure (pi `execution.ts:769-771`).
        if error.is_some() && exit_code == 0 {
            exit_code = 1;
        }

        // (e) `detectSubagentError` re-diagnosis of a still-clean zero exit — a trailing failed
        //     tool/bash call the agent did not speak past (pi `execution.ts:772-780`).
        if exit_code == 0
            && error.is_none()
            && let Some(detected) = detect_subagent_error(&progress.all_events)
        {
            exit_code = detected.exit_code;
            error = Some(detected.message());
        }

        // (f) Empty-output (cold-start) classification (pi `execution.ts:781-789`): a zero-exit run
        //     that produced no usable final text is a RETRYABLE failure so the model-fallback
        //     ladder advances (the message matches `is_retryable_model_failure`'s cold-start /
        //     empty-response / no-output patterns). Mirrors pi's
        //     `!finalText?.trim() && (!options.structuredOutput || missingStructuredOutput)`
        //     exactly: when a structured-output schema IS declared, an empty prose is a failure
        //     ONLY if the structured output is ALSO absent — if the child DID produce a
        //     structured-output value, the empty prose is fine and this gate stays silent
        //     (`run_sync`'s own R-SA-030 check then validates that value). cyrup's
        //     `missingStructuredOutput` analog is a pure PRESENCE test over the event stream
        //     ([`structured_output_absent`], pi's `!existsSync(outputPath)`), NOT a validity test:
        //     a present-but-invalid value is diagnosed later by `run_sync`, exactly as pi defers
        //     validity to `readStructuredOutput` (`execution.ts:791`), which runs only after this
        //     empty-output gate has left the exit code clean. Emitting the retryable "no output"
        //     error HERE (per attempt), rather than deferring the whole structured-missing case to
        //     the post-ladder check, is what lets the ladder actually retry a cold-start empty run
        //     that also declared a schema — pi's behavior, which a `structured_output_schema.is_some()`
        //     short-circuit here would silently drop (the ladder would stop on a bare exit-0 attempt
        //     and only `run_sync` would later flag a NON-retryable structured-missing failure).
        if exit_code == 0
            && error.is_none()
            && final_output
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
            && structured_output_absent(
                self.opts.structured_output_schema.as_ref(),
                self.structured_runtime.as_ref(),
            )
        {
            exit_code = 1;
            error = Some(EMPTY_OUTPUT_ERROR.to_string());
        }

        let success = exit_code == 0 && error.is_none();
        // A bare non-zero exit with no diagnosed cause still needs a stable error string for the
        // ladder's record; pi leaves it undefined, but this crate's `ModelAttempt`/`SingleResult`
        // callers surface `error` directly, so a plain "exited with code N" (never matching a
        // retryable pattern) is used rather than a null.
        // ...and the startup-failure classifier has to be told that is what happened, since pi
        // keys "the child failed with nothing to say" on `error` being UNSET
        // (`subagent-startup-retry.ts:52`). See `StartupEvidence::error_is_placeholder`.
        let error_is_placeholder = !success && error.is_none();
        if error_is_placeholder {
            error = Some(format!("subagent attempt exited with code {exit_code}"));
        }

        (
            AttemptSignal {
                success,
                exit_code: Some(exit_code),
                error,
                usage: progress.usage.clone(),
                timed_out: false,
                // R-SA-037: set from the drive loop's detach observation — `true` when the child's
                // NDJSON showed a blocking `contact_supervisor` ask (surfaced via `spawn_clarify`),
                // which bypasses acceptance/completion-guard/truncation and stops the ladder.
                detached: outcome.detached,
                // Startup-failure evidence (pi `execution.ts:1558-1573`). Every field here is a
                // reason NOT to relaunch this model; `is_retryable_subagent_startup_failure`
                // fails closed on any of them.
                startup: StartupEvidence {
                    final_output_present: final_output
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty()),
                    message_count: progress.message_end_events.len(),
                    tool_count: progress.tool_count,
                    duration_ms: Some(progress.duration_ms()),
                    protocol_error: outcome.protocol_error.is_some(),
                    process_signal: process_signal.clone(),
                    observed_mutation_attempt: crate::exec::completion_guard::has_mutation_tool_call(
                        &progress.all_events,
                    ),
                    // cyrup's foreground executor has no `stopped` analog (pi carries it on the
                    // BACKGROUND runner's result). It cannot be true of a child with zero
                    // messages, zero tools and zero usage anyway — it requires the child to have
                    // run turns — so leaving it false cannot widen the predicate.
                    stopped: false,
                    // SUBA-008 — no longer hard-coded: a turn-budget abort is a deliberate
                    // supervisor kill, and `is_retryable_subagent_startup_failure` must fail
                    // closed on it rather than relaunching the model that was over budget.
                    turn_budget_exceeded: outcome.turn_budget.exceeded(),
                    error_is_placeholder,
                },
            },
            AttemptRecord {
                turn_budget: outcome.turn_budget.clone(),
                progress,
                final_output,
                interrupted: false,
                control,
            },
        )
    }

    /// pi `waitForSubagentStartupRetry(delayMs, [options.signal, options.interruptSignal])`
    /// (`execution.ts:1588`): the backoff is raced against BOTH lifecycle signals, and which one
    /// fired decides whether the run is paused or abandoned. Already-aborted is checked first
    /// (upstream `subagent-startup-retry.ts:88`), so a cancelled run never sleeps before giving up.
    async fn wait_startup_retry(&mut self, delay: Duration) -> StartupRetryWait {
        if self.opts.interrupt.is_cancelled() {
            return StartupRetryWait::Interrupted;
        }
        if self.opts.cancel.is_cancelled() {
            return StartupRetryWait::Cancelled;
        }
        tokio::select! {
            biased;
            () = self.opts.interrupt.cancelled() => StartupRetryWait::Interrupted,
            () = self.opts.cancel.cancelled() => StartupRetryWait::Cancelled,
            () = tokio::time::sleep(delay) => StartupRetryWait::Proceed,
        }
    }

    /// pi mutates `result.finalOutput`/`result.interrupted` in place at `execution.ts:1584-1618`;
    /// this crate's ladder cannot reach inside an opaque `Attempt`, so it calls back here instead.
    fn apply_startup_outcome(&mut self, attempt: &mut Self::Attempt, outcome: &StartupOutcome) {
        match outcome {
            StartupOutcome::Interrupted => {
                attempt.interrupted = true;
                attempt.final_output = Some(INTERRUPTED_FINAL_OUTPUT.to_string());
            }
            StartupOutcome::Cancelled(message) | StartupOutcome::Exhausted(message) => {
                // pi also sets `result.progress.status/error` here; cyrup's `AgentProgress` carries
                // neither field (its status/error live on `SingleResult`, which `run_sync` derives
                // from the ladder's own `AttemptSignal::error` — already set by the ladder).
                attempt.final_output = Some(message.clone());
            }
        }
    }

    fn snapshot_output_file(&mut self) {
        // R-SA-031: the actual snapshot value is consulted later, in `finalize_result`'s
        // file-only handoff — `run_fallback_ladder` only requires the snapshot to be TAKEN at
        // the correct point (immediately before each fresh spawn), which this no-op satisfies
        // trivially since `run_sync` itself takes the real snapshot once, outside the ladder, and
        // compares it once after the ladder settles (R-SA-031 is a whole-task stat-snapshot
        // heuristic, not a per-attempt one — a task's `output_path` does not change between
        // fallback attempts, so re-snapshotting per attempt would not observe anything new).
    }
}

/// The OS signal that killed a child, named the way pi names it (`proc.on("close", (code,
/// signal))` hands Node's signal NAME straight through), for
/// [`crate::exec::fallback::StartupEvidence::process_signal`]. `None` on a normal exit, and `None`
/// on non-Unix, where no such concept crosses `ExitStatus`.
/// SUBA-023 (consumer half) — the name comes from [`crate::spawn::signal::signal_name_of`], the
/// single crate-wide mapping that also populates
/// [`crate::spawn::signal::TerminationOutcome::signal_name`]. This function used to carry its OWN
/// three-entry table (`SIGINT`/`SIGKILL`/`SIGTERM`) and render everything else as `SIG<number>`, so
/// a child that segfaulted reported `SIG11` and one that aborted reported `SIG6` where pi — which
/// hands Node's signal NAME straight through — reports `SIGSEGV` and `SIGABRT`. Those are precisely
/// the deaths worth diagnosing, and they are what the ladder's own three signals are not.
///
/// The numeric `SIG<n>` form survives only as the fall-back for a signal the shared table does not
/// name, so no previously-reported value is lost.
#[cfg(unix)]
fn process_signal_name(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    crate::spawn::signal::signal_name_of(status)
        .map(ToString::to_string)
        .or_else(|| status.signal().map(|signal| format!("SIG{signal}")))
}

#[cfg(not(unix))]
fn process_signal_name(_status: &std::process::ExitStatus) -> Option<String> {
    None
}


/// pi's `missingStructuredOutput` analog (`execution.ts:1189-1191`) for the empty-output
/// (cold-start) gate: is the child's structured output ABSENT from its event stream? Returns
/// `true` when NO structured-output schema was requested at all (pi's `!options.structuredOutput`
/// leg, where empty prose is unconditionally an empty-output failure), OR when a schema WAS
/// requested but the child never delivered a value. A present-but-invalid value is deliberately NOT
/// "absent" here — this is a pure presence test, exactly like pi's `existsSync`; the value's
/// validity is a separate concern [`crate::exec::run_sync`]'s own R-SA-030 structured-output check diagnoses
/// afterward (pi `readStructuredOutput`, `execution.ts:1204-1224`).
///
/// The presence channel is pi's literally: `!existsSync(options.structuredOutput.outputPath)`
/// (`execution.ts:1189-1191`) — the CAPTURE FILE the child's `structured_output` tool writes, and
/// nothing else. Consulting the transcript instead would fail a perfectly good run: a child that
/// calls `structured_output` and then stops without prose has `finalText` empty and a written
/// capture file, which pi passes (`missingStructuredOutput === false`) and a fenced-block scan
/// would classify as missing, flipping the attempt to a retryable "produced no output" failure the
/// ladder then burns a fallback model on.
///
/// SUBA-S01 residual: the no-runtime case (`run_sync` could not create the capture runtime at all)
/// used to fall through to that fenced-block scan. It no longer does — with no runtime there is no
/// capture file, so the value is absent by definition, which is also exactly what [`crate::exec::run_sync`]'s
/// own read-back now concludes for the same state. The two can no longer disagree.
fn structured_output_absent(
    schema: Option<&serde_json::Value>,
    runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
) -> bool {
    match schema {
        None => true,
        Some(_) => match runtime {
            Some(runtime) => !runtime.output_path.exists(),
            None => true,
        },
    }
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


    /// SUBA-023 (consumer half): the signal name published on a run record must come from the
    /// crate's ONE mapping (`spawn::signal::signal_name_of`, which also fills
    /// `TerminationOutcome::signal_name`), not from a local three-entry table.
    ///
    /// RED before the fix: `process_signal_name` named only SIGINT/SIGKILL/SIGTERM, so a crashed
    /// child's `SingleResult.process_signal` read `SIG11`/`SIG6` where pi (passing Node's signal
    /// NAME through at `execution.ts:1081`) reports `SIGSEGV`/`SIGABRT`.
    #[cfg(unix)]
    #[test]
    fn a_crashed_child_reports_the_posix_signal_name_not_a_number() {
        use std::os::unix::process::ExitStatusExt as _;

        for (signal, expected) in [
            (2, "SIGINT"),
            (6, "SIGABRT"),
            (9, "SIGKILL"),
            (11, "SIGSEGV"),
            (15, "SIGTERM"),
            (1, "SIGHUP"),
        ] {
            assert_eq!(
                process_signal_name(&std::process::ExitStatus::from_raw(signal)).as_deref(),
                Some(expected),
                "signal {signal} must be named the way pi names it"
            );
        }

        // A normal exit names no signal at all (pi's `if (signal) result.processSignal = signal`).
        assert_eq!(process_signal_name(&std::process::ExitStatus::from_raw(0)), None);

        // …and a signal the shared table does not name still falls back to the numeric form rather
        // than disappearing, so nothing previously reported is lost.
        assert_eq!(
            process_signal_name(&std::process::ExitStatus::from_raw(64)).as_deref(),
            Some("SIG64")
        );
    }


    /// SUBA-S01 residual — the per-attempt cold-start gate's presence test is pi's `existsSync` on
    /// the CAPTURE FILE (`execution.ts:1189-1191`) and nothing else.
    ///
    /// The `None`-runtime arm used to fall through to the fenced-```json-block scan, so a child
    /// that produced only prose containing an incidental JSON fence looked like it had "delivered"
    /// a structured value to this gate. That both suppressed a retryable empty-output failure and
    /// disagreed with `run_sync`'s own post-ladder read-back, which had no file to read. With no
    /// runtime there is no capture file, so the value is absent by definition and the two agree.
    #[test]
    fn a_declared_schema_with_no_capture_runtime_is_absent_and_never_consults_the_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let schema = serde_json::json!({"type": "object"});

        // No schema declared at all: pi's `!options.structuredOutput` leg — unconditionally absent,
        // which is what makes empty prose an empty-output failure on its own.
        assert!(structured_output_absent(None, None));

        // Declared, but the runtime could not be created. Absent — no file, no value. (Pre-fix this
        // arm scanned the transcript, where a stray ```json fence read as "present".)
        assert!(structured_output_absent(Some(&schema), None));

        // Declared WITH a runtime: strictly the capture file's existence, both ways. Asserting the
        // present case first keeps the absent assertion from passing vacuously.
        let runtime = crate::exec::structured::create_structured_output_runtime(&schema, dir.path())
            .expect("runtime is created");
        assert!(
            structured_output_absent(Some(&schema), Some(&runtime)),
            "no capture file written yet => absent"
        );
        std::fs::write(&runtime.output_path, b"{}").expect("child writes its capture file");
        assert!(
            !structured_output_absent(Some(&schema), Some(&runtime)),
            "a written capture file => present, even with no prose at all"
        );
    }

}
