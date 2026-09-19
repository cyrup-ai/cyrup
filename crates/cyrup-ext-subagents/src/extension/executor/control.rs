//! The live-run control verbs kept at this layer: interrupt, resume and append-step. `stop`,
//! `steer` and `dismiss` moved to [`crate::extension::executor::foreground_actions`] (WORKFLOW_8).

use std::path::Path;

use crate::background::control::{self, AppendOutcome, InterruptOutcome, ResumeOutcome};
use crate::background::{RunId, RunMode, RunPaths, RunState, run_status};
use crate::discovery::types::AgentReadScope;
use crate::error::SubagentError;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::executor::requests::BackgroundStepsSpec;
use crate::fork_context::ContextMode;
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

impl SubagentExecutor {
    /// `action: "interrupt"` (C5): deliver a soft, resumable interrupt (R-SA-084 — a *pause*
    /// request, never a kill) to the target async run, or, with no id, to the most-recently-updated
    /// running run in this cwd's async root — pi `subagent-executor.ts:2871-2911`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if no interrupt-capable run is found, if the target is not Running (R-SA-079),
    /// or if the underlying delivery fails.
    pub async fn control_interrupt(
        &self,
        cwd: &Path,
        target: Option<&str>,
    ) -> Result<String, String> {
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        let run_id = match target {
            Some(explicit) => explicit.to_string(),
            None => {
                // No id: interrupt the most-recently-updated running run (the list is already sorted
                // running-first, most-recent-first), mirroring pi's "defaults to the most recently
                // active controllable run" contract for interrupt.
                let runs = run_status::list_active_runs(
                    &async_root,
                    &results_dir,
                    self.current_session_id().as_deref(),
                )
                .await
                .map_err(|e| e.to_string())?;
                runs.iter()
                    .find(|run| run.status.state == RunState::Running)
                    .map(|run| run.status.run_id.as_str().to_string())
                    .ok_or_else(|| "No interrupt-capable run found in this session.".to_string())?
            }
        };
        let current_session =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        match control::interrupt(
            &async_root,
            &results_dir,
            &run_id,
            "interrupt-action",
            None,
            current_session.as_ref(),
        )
        .await
        {
            Ok(InterruptOutcome::Delivered | InterruptOutcome::AlreadyPending) => {
                Ok(format!("Interrupt requested for async run {run_id}."))
            }
            Ok(InterruptOutcome::NotRunning) => Err(format!(
                "No running async run with an interrupt-capable pid was found for '{run_id}'."
            )),
            // S4 (pi `async-stop-action.ts:34`) — upstream's exact refusal sentence.
            Ok(InterruptOutcome::NotInActiveSession) => Err(format!(
                "Async run '{run_id}' was not found in the active session."
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    /// `action: "resume"` (C5): the R-SA-085/086 fork — steer a still-running run's live child, or
    /// revive a terminal run from its persisted transcript — pi `subagent-executor.ts:2865`/
    /// `801-1031`. Requires a follow-up `message` (falling back to `task`) and a run `id`.
    ///
    /// The running-selection branch interrupts the live child, then DELIVERS the follow-up over the
    /// broker to that child's deterministic registered bridge target — pi
    /// `deliverSubagentIntercomMessageEvent(events, target.intercomTarget, …)`
    /// (`subagent-executor.ts:848-878`). The child WAS activated as a bridge participant at its spawn
    /// (the subagents spawn overlay writes `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET`/`_RUN_ID`/
    /// `_CHILD_AGENT`/`_CHILD_INDEX`/`_INTERCOM_SESSION_NAME`, so the child's `IntercomExtension`
    /// registered `contact_supervisor` + a broker presence under
    /// `resolve_subagent_intercom_target(run_id, agent, index)`), so this arm recovers that same
    /// target from the reconciled run status (`steps[step_index].agent` + the step index) and steers
    /// it via the [`crate::tui::intercom::SteerChannel`] threaded in by
    /// `SubagentsExtension::with_channels`. pi's "intercom target is not registered" guidance is
    /// returned ONLY as the genuine delivery-FAILED fallback (no live broker, or no registered
    /// receiver at that target) — the caller then waits for the pause and retries, hitting the
    /// terminal-revival branch. The terminal-revival branch respawns a fresh detached child seeded
    /// from the transcript, running the run's REAL resolved persona (T0.1/C13), and hard-fails (no
    /// silent fresh-session fallback) when no transcript exists.
    ///
    /// S4 — pi `async-resume.ts:477`, reached from `async-steering-action.ts:203`'s `{ sessionId:
    /// input.state.currentSessionId ?? undefined }`: a run owned by a DIFFERENT session refuses
    /// with `Async run '{id}' was not found in the active session.` BEFORE any state branch runs,
    /// so a foreign run is never reported `stopped` or no-transcript. PERMISSIVE, like
    /// `stop`/`interrupt`/`steer` — a headless/SDK host with no session identity of its own still
    /// resumes its own runs.
    ///
    /// # Errors
    ///
    /// Returns `Err` for a missing message/id, a foreign-session refusal, the delivery-failed
    /// intercom-unregistered live-steer notice, a no-transcript revival, or any resolution/spawn
    /// failure.
    pub async fn control_resume(
        &self,
        cwd: &Path,
        target: Option<&str>,
        message: Option<&str>,
        task: Option<&str>,
        index: Option<usize>,
    ) -> Result<String, String> {
        let follow_up = message.or(task).map(str::trim).unwrap_or_default();
        if follow_up.is_empty() {
            return Err("action='resume' requires message.".to_string());
        }
        let Some(run_id) = target else {
            return Err("action='resume' requires id.".to_string());
        };
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        // S4 — pi `async-resume.ts:477`'s `options.sessionId`: the CALLER's own session, the same
        // expression the `control::interrupt` call below already uses.
        let current_session =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        match control::resume(
            &async_root,
            &results_dir,
            run_id,
            index,
            current_session.as_ref(),
        )
        .await
        {
            Ok(ResumeOutcome::SteerRunning { step_index }) => {
                // pi (`subagent-executor.ts:848-878`): interrupt the live child, then DELIVER the
                // follow-up over the broker to that child's registered bridge target. Recover the
                // child's deterministic target from the reconciled run status — the resumed step's
                // REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let source_paths = RunPaths::for_run(
                    &async_root,
                    &results_dir,
                    &RunId::from_token(run_id.to_string()),
                );
                // pi `interruptLiveAsyncResumeTarget` (`background/async-resume.ts:53-56`):
                // re-reconcile and REQUIRE `status.state === "running"` with a numeric pid before
                // even attempting to interrupt — a reconciliation failure, a run that is no longer
                // Running, or a Running status with no known runner pid all abort the WHOLE resume
                // with this exact diagnostic, rather than silently falling through to steer a child
                // that was never confirmed interruptible.
                let status = match control::reconcile_before_control_op(&source_paths).await {
                    Ok(status) if status.state == RunState::Running && status.pid.is_some() => {
                        status
                    }
                    _ => {
                        return Err(format!(
                            "Async run {run_id} is live but no interrupt-capable runner pid was \
                             found."
                        ));
                    }
                };
                // Recover the child's deterministic target from the reconciled run status — the
                // resumed step's REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let (child_target, child_agent) = match status.steps.get(step_index) {
                    Some(step) => (
                        Some(
                            crate::spawn::intercom_target::resolve_subagent_intercom_target(
                                run_id,
                                &step.agent,
                                step_index,
                            ),
                        ),
                        Some(step.agent.clone()),
                    ),
                    None => (None, None),
                };
                // Interrupt the live child (genuine), matching pi's interrupt-then-deliver order
                // (`subagent-executor.ts:846-859`): a FAILED interrupt is returned as the error
                // result immediately, before any follow-up delivery is attempted — it must never be
                // silently swallowed and fall through to steering a child that may still be running
                // its prior turn.
                if let Err(e) = control::interrupt(
                    &async_root,
                    &results_dir,
                    run_id,
                    "async-resume",
                    None,
                    crate::identity::SessionId::parse_opt(self.current_session_id().as_deref())
                        .as_ref(),
                )
                .await
                {
                    return Err(format!("Failed to interrupt async run {run_id}: {e}"));
                }
                // pi's follow-up header includes the resolved agent name (`subagent-executor.ts:863`:
                // `Follow-up for async run ${target.runId} (${target.agent}):`).
                let follow_up_message = match &child_agent {
                    Some(agent) => {
                        format!("Follow-up for async run {run_id} ({agent}):\n\n{follow_up}")
                    }
                    None => format!("Follow-up for async run {run_id}:\n\n{follow_up}"),
                };
                // pi's `deliverSubagentIntercomMessageEvent` bounds EVERY caller (including this
                // live-child follow-up steer, `subagent-executor.ts:860`) to a 500ms default timeout
                // race — the caller's own turn is never blocked longer than that waiting on a
                // delivery ack (`result-intercom.ts:325-358`). Race the raw `SteerChannel::steer`
                // call against that same bound rather than awaiting it unbounded.
                let delivered = match &child_target {
                    Some(target) => {
                        crate::tui::intercom::steer_with_default_timeout(
                            self.steer.as_ref(),
                            target.clone(),
                            follow_up_message,
                        )
                        .await
                    }
                    None => false,
                };
                if delivered {
                    // pi's delivered-follow-up confirmation (`subagent-executor.ts:868-871`).
                    Ok(format!(
                        "Interrupted live async child, then delivered follow-up.\n\
                         Run: {run_id}\n\
                         Intercom target: {}",
                        child_target.unwrap_or_default()
                    ))
                } else {
                    // Delivery-FAILED fallback ONLY (no live broker, or no registered receiver at the
                    // target) — pi's exact intercom-unregistered guidance
                    // (`subagent-executor.ts:873-877`).
                    let target_line = child_target
                        .map(|t| format!("Intercom target: {t}\n"))
                        .unwrap_or_default();
                    Err(format!(
                        "Async child appears live but its intercom target is not registered.\n\
                         Run: {run_id}\n\
                         {target_line}Wait for completion, then retry action='resume'."
                    ))
                }
            }
            Ok(ResumeOutcome::RespawnFromTranscript {
                step_index,
                session_file,
            }) => self
                .revive_from_transcript(cwd, run_id, step_index, &session_file, follow_up)
                .await
                .map_err(|e| e.to_string()),
            Err(SubagentError::ResumeNoTranscript) => Err(format!(
                "Resume unavailable: async run '{run_id}' has no persisted transcript to revive \
                 from."
            )),
            // G77 — pi `async-resume.ts:406` @v0.43.0 throws with this exact sentence, and the
            // thrown message is what the caller surfaces. `SubagentError::ResumeStopped`'s own
            // `Display` IS that sentence, so this arm exists to pin it: a stopped run must never
            // fall into the no-transcript wording above (its children usually DO have transcripts)
            // and must never be silently revived.
            Err(e @ SubagentError::ResumeStopped(_)) => Err(e.to_string()),
            // S4 — pi `async-resume.ts:477`'s refusal, reached via `async-steering-action.ts:203`.
            // `SubagentError::ResumeNotInActiveSession`'s own `Display` IS upstream's sentence, so
            // this arm exists to pin it, exactly as the `ResumeStopped` arm above does.
            Err(e @ SubagentError::ResumeNotInActiveSession(_)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The terminal-revival spawn half of [`Self::control_resume`] (R-SA-085): read the source run's
    /// reconciled status to recover the revived step's agent, resolve its REAL persona (never a
    /// placeholder, C13), and spawn a fresh detached background single-run seeded from
    /// `session_file` (`executeAsyncSingle` in pi, `subagent-executor.ts:987`).
    async fn revive_from_transcript(
        &self,
        cwd: &Path,
        source_run_id: &str,
        step_index: usize,
        session_file: &Path,
        follow_up: &str,
    ) -> Result<String, SubagentError> {
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        let source_paths = RunPaths::for_run(
            &async_root,
            &results_dir,
            &RunId::from_token(source_run_id.to_string()),
        );
        let status = control::reconcile_before_control_op(&source_paths).await?;
        let agent = status
            .steps
            .get(step_index)
            .map(|step| step.agent.clone())
            .ok_or_else(|| {
                SubagentError::AgentNotFound(format!("no step at index {step_index} to revive"))
            })?;
        // pi `effectiveCwd = target.cwd ?? requestCwd` (`subagent-executor.ts:890`, fed by
        // `target.cwd` = `status.cwd ?? result.cwd`, `background/async-resume.ts:373`): the revived
        // child's persona discovery AND its actual spawn cwd prefer the ORIGINAL run's own working
        // directory (persisted onto the reconciled status by `finish_run`,
        // `background/runner_main.rs`) over whatever cwd happens to be current at resume time —
        // never silently reroute a revived agent into a different directory than the one it was
        // originally invoked from.
        // LANES — pi `async-resume.ts:576-579`, where the retained worktree OUTRANKS the run's
        // own recorded cwd:
        //
        // ```ts
        // const managedWorktreeCwd = location.asyncDir
        //     ? resolveRetainedWorktreeCwd(parallelHandoffPath(location.asyncDir), runId, index)
        //     : undefined;
        // const resumeCwd = validateResumeCwd(runId, managedWorktreeCwd ?? status?.cwd ?? …);
        // ```
        //
        // This is the rule `handoff/mod.rs`'s safety table names: a child of a `worktree: true`
        // group whose worktrees were RETAINED must resume inside its own worktree. Falling through
        // to `status.cwd` would resume it in the project checkout, where it would read a tree its
        // transcript never saw and write into the developer's working copy — the same directory
        // the group was isolated from in the first place.
        //
        // Failure to read the manifest is not fatal and must not be: a run with no retained
        // worktrees has no manifest at all, which is the overwhelmingly common case, and a
        // corrupt one must not make an otherwise-revivable child unrevivable. Either way the
        // precedence below falls through to what the run itself recorded.
        let managed_worktree_cwd = match crate::handoff::LaneId::parse(source_run_id) {
            Ok(lane) => {
                let manifest = crate::background::RunDir::new(
                    &async_root,
                    &RunId::from_token(source_run_id.to_string()),
                )
                .handoff();
                match crate::handoff::resolve_retained_worktree_cwd(
                    &manifest,
                    &lane,
                    u32::try_from(step_index).unwrap_or(u32::MAX),
                )
                .await
                {
                    Ok(found) => found,
                    Err(error) => {
                        tracing::debug!(
                            target: "cyrup_ext_subagents::handoff",
                            run_id = %source_run_id,
                            child_index = step_index,
                            %error,
                            "no retained worktree could be resolved for this revive; \
                             falling back to the run's recorded cwd"
                        );
                        None
                    }
                }
            }
            Err(_) => None,
        };
        let effective_cwd = managed_worktree_cwd
            .or_else(|| status.cwd.clone())
            .unwrap_or_else(|| cwd.to_path_buf());
        let resolved_agents = self.resolve_plan_personas(
            &effective_cwd,
            [agent.clone()],
            AgentReadScope::Both,
            &roots,
        )?;
        let revived_task =
            Self::build_revived_async_task(source_run_id, &agent, session_file, follow_up);
        let step = SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.clone(),
            task: revived_task,
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: Some(session_file.to_path_buf()),
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: Some(ContextMode::Fork),
            agent_scope: None,
        };
        let new_id = self
            .spawn_background_steps(
                &effective_cwd,
                BackgroundStepsSpec {
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    // SUBA-073: no policy on this path — same pre-existing incompleteness as
                    // `turn_budget` immediately above; not this task's fix to extend.
                    permission_rules: None,
                    // SCOPE_9 — pi `target.source === "async"` selecting
                    // `transferActiveAsyncCapacity` (`subagent-executor.ts:2086-2093` @v0.68.0).
                    // THIS is cyrup's `source == "async"` moment: `control::resume` resolved this
                    // target out of the per-cwd ASYNC root and reconciled it to a terminal state
                    // before handing back `RespawnFromTranscript`, so the source is exactly the
                    // settled async run whose slot upstream moves. Taking it over (generation
                    // bumped, `source_run_id` breadcrumb written) is what keeps ONE operator-visible
                    // run on ONE slot across a revive — acquiring afresh would double-charge the
                    // session, and at a cap of 1 the source's own retained slot would make this
                    // revive refuse itself with the exhausted sentence.
                    transfer_from: Some(RunId::from_token(source_run_id.to_string())),
                    steps: vec![RunnerStep::SingleStep(step)],
                    mode: RunMode::Single,
                    session_file: Some(session_file.to_path_buf()),
                    resolved_agents,
                    // The revival's follow-up is its `{task}`; a single revived run has no chain dir.
                    original_task: follow_up.to_string(),
                    chain_dir: None,
                    // SUBA-N05: a revival carries no per-call `control` object of its own — pi's
                    // revive path likewise resolves `resolveControlConfig(deps.config.control,
                    // input.params.control)` where `params` is the ACTION's params
                    // (`subagent-executor.ts:1179` @v0.34.0), and cyrup's `resume` action exposes no
                    // `control` field. The extension-level `subagents.control` block still applies.
                    control: Some(crate::exec::control::resolve_control_config(
                        self.config_snapshot().await.control.as_ref(),
                        None,
                    )),
                    // SUBA-N06: a revival exposes no `includeProgress` param either, for the same
                    // reason — the `resume` action's params carry no such field.
                    include_progress: None,
                    // SUBA-N03: the `resume` action's params carry none of the SINGLE-mode
                    // overrides either — a revival re-runs an EXISTING run's persona against a
                    // follow-up, and pi's revive path likewise forwards no `output`/`skill`/
                    // `share`/`sessionDir`/`artifacts`/`timeoutMs`. The revived run's own session
                    // file (above) is what keeps its transcript continuous.
                    run_id: RunId::new(),
                    timeout_ms: None,
                    share: None,
                    artifacts_dir: None,
                    artifact_config: crate::artifacts::ArtifactConfig::default(),
                },
            )
            .await?;
        // pi's confirmation (`subagent-executor.ts:1019-1029`): a source label ("foreground" /
        // "async" / "nested" — cyrub's `control::resume` only ever revives an async source today, so
        // this is always "async" here), then the intercom-target line ONLY when a real bridge is
        // wired (pi `intercomBridge.active`), matching `NoTransportSteerChannel::is_active` ==
        // `false` degrading to omitting the line entirely rather than showing a target nothing will
        // ever deliver to.
        let intercom_target_line = if self.steer.is_active() {
            let target = crate::spawn::intercom_target::resolve_subagent_intercom_target(
                new_id.as_str(),
                &agent,
                0,
            );
            format!("Intercom target: {target} (if registered)\n")
        } else {
            String::new()
        };
        Ok(format!(
            "Revived async subagent from {source_run_id}.\n\
             Revived run: {new_id}\n\
             Agent: {agent}\n\
             Session: {}\n\
             {intercom_target_line}Status if needed: subagent({{ action: \"status\", id: \"{new_id}\" }})",
            session_file.display()
        ))
    }

    /// pi `buildRevivedAsyncTask` (`background/async-resume.ts:526-539`): the revival framing wrapped
    /// AROUND the orchestrator's raw follow-up, rather than sending the follow-up verbatim as the
    /// revived child's `{task}` — the revived agent otherwise has no way to know it is being resumed
    /// from a stored transcript rather than starting fresh.
    fn build_revived_async_task(
        source_run_id: &str,
        agent: &str,
        session_file: &Path,
        follow_up: &str,
    ) -> String {
        let lines: Vec<String> = vec![
            "You are reviving a previous subagent conversation.".to_string(),
            String::new(),
            format!("Original run: {source_run_id}"),
            format!("Original agent: {agent}"),
            format!("Original session file: {}", session_file.display()),
            String::new(),
            "Use the stored session context as background. Answer the orchestrator's follow-up \
             below. Do not assume the original child process is still alive."
                .to_string(),
            String::new(),
            "Follow-up:".to_string(),
            follow_up.to_string(),
        ];
        lines.join("\n")
    }

    /// `action: "append-step"` (C5): validate and enqueue exactly one new step onto a running async
    /// chain (R-SA-094/095/096) — pi `subagent-executor.ts:2868`/`508-686`. The appended agent is
    /// resolved through real discovery first (fail-fast on an unknown agent, matching pi's
    /// `buildAsyncRunnerSteps`), then the step is enqueued via [`crate::background::control::append_step`].
    ///
    /// # Errors
    ///
    /// Returns `Err` for a missing id, a chain that is not exactly one step, an unknown agent, or a
    /// primitive-level rejection (wrong mode/state, output-name collision).
    pub async fn control_append_step(
        &self,
        cwd: &Path,
        target: Option<&str>,
        chain: &[serde_json::Value],
    ) -> Result<String, String> {
        let roots = self.config_snapshot().await.roots;
        let Some(run_id) = target else {
            return Err("action='append-step' requires id.".to_string());
        };
        if chain.len() != 1 {
            return Err("action='append-step' requires chain with exactly one step.".to_string());
        }
        let Some(step_val) = chain.first() else {
            return Err("action='append-step' requires chain with exactly one step.".to_string());
        };
        let Some(agent) = step_val.get("agent").and_then(serde_json::Value::as_str) else {
            return Err("action='append-step' chain step requires an 'agent' field.".to_string());
        };
        let task = step_val
            .get("task")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let output = step_val
            .get("output")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // pi validates every appended agent exists before enqueuing (`buildAsyncRunnerSteps` errors
        // on an unknown agent name); resolve it via real discovery for the same fail-fast behavior.
        self.resolve_agent(cwd, agent, AgentReadScope::Both, &roots)
            .map_err(|e| format!("Cannot append step to run '{run_id}': {e}"))?;
        let step = SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task,
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        };
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        match control::append_step(
            &async_root,
            &results_dir,
            run_id,
            vec![RunnerStep::SingleStep(step)],
        )
        .await
        {
            Ok(AppendOutcome::Enqueued { .. }) => {
                let paths = RunPaths::for_run(
                    &async_root,
                    &results_dir,
                    &RunId::from_token(run_id.to_string()),
                );
                let pending = control::count_pending_appends(&paths.append_dir)
                    .await
                    .unwrap_or(1);
                Ok(format!(
                    "Append queued for chain run {run_id}: 1 step. It becomes eligible after the \
                     chain's already-queued steps finish. Pending appends: {pending}."
                ))
            }
            Err(e) => Err(e.to_string()),
        }
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

    #[tokio::test]
    async fn control_interrupt_with_no_run_reports_none_capable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_interrupt(dir.path(), None)
            .await
            .expect_err("no runs -> no interrupt-capable run");
        assert_eq!(err, "No interrupt-capable run found in this session.");
    }

    #[tokio::test]
    async fn control_resume_requires_a_message_then_an_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // Empty follow-up is rejected before anything else (pi `resume` requires `message`).
        let no_msg = executor
            .control_resume(dir.path(), Some("run00000000"), None, None, None)
            .await
            .expect_err("resume requires a message");
        assert_eq!(no_msg, "action='resume' requires message.");
        // With a message but no id, resume requires an id selector.
        let no_id = executor
            .control_resume(dir.path(), None, Some("carry on"), None, None)
            .await
            .expect_err("resume requires an id");
        assert_eq!(no_id, "action='resume' requires id.");
    }

    /// pi `buildRevivedAsyncTask` (`background/async-resume.ts:526-539`): a revived child's `{task}`
    /// must be the follow-up WRAPPED in the revival framing (source run/agent/session-file context
    /// plus an explicit "you are reviving..." preamble), never the orchestrator's raw follow-up text
    /// verbatim — the revived agent otherwise has no way to know it is resuming from a stored
    /// transcript rather than starting fresh.
    #[test]
    fn build_revived_async_task_wraps_the_follow_up_in_pi_s_revival_framing() {
        let task = SubagentExecutor::build_revived_async_task(
            "run00099",
            "researcher",
            Path::new("/tmp/session-abc.jsonl"),
            "please continue",
        );
        assert_eq!(
            task,
            "You are reviving a previous subagent conversation.\n\
             \n\
             Original run: run00099\n\
             Original agent: researcher\n\
             Original session file: /tmp/session-abc.jsonl\n\
             \n\
             Use the stored session context as background. Answer the orchestrator's follow-up \
             below. Do not assume the original child process is still alive.\n\
             \n\
             Follow-up:\n\
             please continue"
        );
        assert_ne!(
            task, "please continue",
            "the revived task must NOT be the raw follow-up passed through verbatim"
        );
    }

    #[tokio::test]
    async fn control_append_step_validates_shape_before_touching_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // Missing id.
        let no_id = executor
            .control_append_step(dir.path(), None, &[])
            .await
            .expect_err("append-step requires id");
        assert_eq!(no_id, "action='append-step' requires id.");
        // Wrong-cardinality chain (must be exactly one step).
        let bad_chain = executor
            .control_append_step(dir.path(), Some("run00000000"), &[])
            .await
            .expect_err("append-step requires exactly one chain step");
        assert_eq!(
            bad_chain,
            "action='append-step' requires chain with exactly one step."
        );
    }
}
