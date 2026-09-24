//! The live-run control verbs kept at this layer: interrupt, resume and append-step. `stop`,
//! `steer` and `dismiss` moved to [`crate::extension::executor::foreground_actions`] (WORKFLOW_8).

use std::collections::BTreeMap;
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
        // The persisted launch contract (pi `readAsyncRecoveryDescriptor`, `async-resume.ts:310`
        // @v0.68.0, consumed at `subagent-executor.ts:2059-2065`): a terminal async revive
        // REQUIRES it. A missing descriptor refuses with pi's own sentence (`:2060`) rather than
        // falling back to the bare agent name — the capability-WIDENING revive this replaces, where
        // a child launched with a narrowed tool set resumed with the persona file's full default
        // set. Runs launched before the descriptor existed have none and are refused exactly as
        // upstream refuses them. A descriptor for another agent (`async-resume.ts:566`) or another
        // source run (`subagent-executor.ts:1493`) refuses likewise.
        let source_id = RunId::from_token(source_run_id.to_string());
        let descriptor = crate::background::RecoveryDescriptor::read(
            &crate::background::RunDir::new(&async_root, &source_id).recovery_descriptor(),
        )
        .await?
        .ok_or_else(|| crate::background::RecoveryDescriptorError::Missing {
            run_id: source_id.clone(),
        })?;
        descriptor.assert_belongs_to(&status, &agent)?;
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
        // pi `async-resume.ts:579`: `managedWorktreeCwd ?? status?.cwd ?? result?.cwd ??
        // recoveryDescriptor?.cwd`. The descriptor's recorded cwd is the last rung, and because a
        // terminal async revive always HAS a descriptor here (above), pi's final `?? requestCwd`
        // (`subagent-executor.ts:890`) is unreachable in cyrup and is not reproduced.
        let effective_cwd = managed_worktree_cwd
            .or_else(|| status.cwd.clone())
            .unwrap_or_else(|| descriptor.cwd.clone());
        // pi `subagent-executor.ts:1893-1905` + `:2065`: discover the persona as before and, when
        // discovery no longer finds the agent, SYNTHESISE its base from the descriptor rather than
        // refusing — then, either way, the descriptor OVERLAYS the base field-for-field
        // (`applySteeringRecoveryAgentConfig`, `async-resume.ts:604-632`). The file on disk does
        // not win: the revived run keeps the model, tools, budgets, prompt and depth ceiling it was
        // LAUNCHED with, however the persona file has changed since. Only `AgentNotFound` is
        // synthesised over — a malformed file that claims the name still refuses (SUBA-086), as it
        // does on every other launch path.
        //
        // SUBA-103 — the discovered definition also yields the agent's CURRENT `fast`, the rung pi's
        // relaunch falls back to (`params.fast ?? agentConfig.fast`, `async-execution.ts:1967`
        // @v0.68.0, where `agentConfig` is the discovered base under the overlay and
        // `applySteeringRecoveryAgentConfig` never touches `fast`, `async-resume.ts:604-632`). A
        // synthesised base has no `fast` (`subagent-executor.ts:1894-1905`).
        let (mut resolved_agents, current_agent_fast) = match self.resolve_plan_agents(
            &effective_cwd,
            [agent.clone()],
            AgentReadScope::Both,
            &roots,
        ) {
            Ok(agents) => (
                agents
                    .iter()
                    .map(|(name, def)| (name.clone(), crate::exec::resolve_step_agent_config(def)))
                    .collect::<BTreeMap<_, _>>(),
                agents.get(&agent).and_then(|def| def.fast),
            ),
            Err(SubagentError::AgentNotFound(_)) => (
                BTreeMap::from([(agent.clone(), descriptor.synthesised_persona())]),
                None,
            ),
            Err(error) => return Err(error),
        };
        if let Some(persona) = resolved_agents.get_mut(&agent) {
            descriptor.apply_to_persona(persona);
        }
        let revived_task =
            Self::build_revived_async_task(source_run_id, &agent, session_file, follow_up);
        // Every per-call field comes back off the descriptor (pi `subagent-executor.ts:2111-2185`:
        // `structuredOutputSchema`, `acceptance`, `sessionDir`, `context`, `modelOverride`,
        // `outputPath`, `outputMode`, `skills`, `maxSubagentDepth`). `tools`/`extensions` stay
        // `None` on the step: the launch values ARE the persona's and rode the overlay above (pi
        // overlays `agentConfig.tools`/`.extensions`, never a per-call param).
        let mut step = SingleStepSpec {
            // SUBA-100 — pi's relaunch places the revived child as `params.machine ??
            // agentConfig.machine` (`async-execution.ts:1774` @v0.68.0) over the DISCOVERED base
            // (`applySteeringRecoveryAgentConfig` never touches `machine`); a synthesised base has
            // none. Resolved below, before the run exists.
            machine: resolved_agents
                .get(&agent)
                .and_then(|persona| persona.machine.as_deref())
                .map(crate::placement::StepPlacement::requested),
            skills: (!descriptor.skills.is_empty()).then(|| descriptor.skills.clone()),
            session_dir: descriptor.session_dir.clone(),
            agent: agent.clone(),
            task: revived_task,
            cwd: None,
            // pi `modelOverride: recoveryDescriptor?.model` (`:2146`). An EXPLICIT launch model is
            // the per-call override again; a configured/inherited one is already pinned on the
            // persona by the overlay, which is what keeps the revived ladder on the SAME primary
            // rather than the reviving session's.
            model: (descriptor.model_origin == crate::background::ModelOrigin::Explicit)
                .then(|| descriptor.model.clone())
                .flatten(),
            tools: None,
            extensions: None,
            session_file: Some(session_file.to_path_buf()),
            // pi `maxSubagentDepth: recoveryDescriptor?.maxSubagentDepth` (`:2155`): the EFFECTIVE
            // child ceiling the run was launched under, re-applied as a tightening-only override.
            max_depth_override: Some(descriptor.max_subagent_depth),
            structured_output_schema: descriptor.structured_output_schema.clone(),
            output: None,
            output_path: descriptor.output_path.clone(),
            output_mode: Some(descriptor.output_mode),
            // SUBA-103 — pi `fast: recoveryDescriptor?.fast` (`subagent-executor.ts:2147`
            // @v0.68.0), relaunched as `params.fast ?? agentConfig.fast`
            // (`async-execution.ts:1967`): the descriptor's value when it recorded one, else the
            // agent's CURRENT `fast`. The descriptor holds the launch's EFFECTIVE value whenever
            // some rung set it (see `RecoveryDescriptor::fast`), so a run launched fast resumes
            // fast; a launch no rung set records nothing, and — exactly as upstream — its revive
            // takes whatever the agent file says now. `[CYRUP-DELTA]`: a launch whose effective
            // value was an explicit `false` records `false` (pi records only `true`), so that
            // revive stays standard even if the file has since turned `fast` on.
            fast: descriptor.fast.or(current_agent_fast),
            reads: None,
            acceptance: descriptor.acceptance.clone(),
            // pi `recoveryContext = recoveryDescriptor?.context ?? …` (`:1883`). `Fork` is what a
            // revive seeded from a transcript meant before the descriptor recorded the launch's
            // own resolved context.
            context: Some(descriptor.context.unwrap_or(ContextMode::Fork)),
            agent_scope: None,
        };
        // SUBA-100 — the revived step's placement is resolved like any launch's
        // (`buildSeqStep`/`executeAsyncSingle`'s block). A placed revive then meets upstream's own
        // refusal where upstream meets it — the relaunched child resumes a session file, and
        // `serializeHerdrPiLaunch` refuses that in the runner: "Pane-native remote … does not
        // support fork, resume, or revival; use fresh context." It never silently runs locally.
        if step.machine.is_some() {
            let placement_cfg = self.config_snapshot().await;
            let runner = resolved_agents
                .get(&agent)
                .and_then(|persona| persona.runner.clone());
            crate::placement::resolve::fold_step_placement(
                &mut step,
                crate::placement::resolve::PlacementAgent {
                    name: &agent,
                    machine: None,
                    runner: runner.as_ref(),
                },
                None,
                None,
                false,
                &mut crate::placement::resolve::launch_resolver(&effective_cwd, &placement_cfg)?,
            )
            .await
            .map_err(SubagentError::Management)?;
        }
        // Minted here rather than inline in the spec: the revival LEASE names the same run id the
        // launch uses, and a second `RunId::new()` would give the lease's owner record a run id
        // that appears nowhere else — so a reader of a refused revival's conflict sentence could
        // not find the run it names.
        let revived_run_id = RunId::new();
        let new_id = self
            .spawn_background_steps(
                &effective_cwd,
                BackgroundStepsSpec {
                    // The four cyrup-only run-level keys the descriptor carries (SUBA-021/008/073/
                    // N06). Before the descriptor every one of these was `None` on a revive — the
                    // exact degradation this contract exists to remove.
                    usage_budget: descriptor.usage_budget,
                    turn_budget: descriptor.turn_budget,
                    permission_rules: descriptor.permission_rules.clone(),
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
                    transfer_from: Some(source_id.clone()),
                    // VL-S3 — pi `config.revivalLease` (`subagent-runner.ts:219`, acquired at
                    // `:5242`). THIS is the one cyrup path that reopens a stored session
                    // transcript for writing, and the lease is what stops a second revival of the
                    // same file from running beside this one: upstream's own hazard statement is
                    // the conflict sentence itself — *"Wait for that revival to finish or start a
                    // separate continuation without reusing this session file."*
                    //
                    // All four fields are already in hand here and none is derived: the session
                    // file is this call's own argument, the run id is the one just minted for the
                    // revived run, the source is the settled async run being revived FROM, and the
                    // parent session is this orchestrator's. The RUNNER acquires it — see
                    // `RunnerConfig::revival_lease` — because the claim's lifetime is the
                    // runner's, not this call's.
                    revival_lease: Some(crate::background::session_lease::SessionLeaseRequest {
                        session_file: session_file.to_path_buf(),
                        run_id: revived_run_id.clone(),
                        source_run_id: source_id,
                        parent_session_id: self.current_session_id(),
                    }),
                    steps: vec![RunnerStep::SingleStep(step)],
                    mode: RunMode::Single,
                    session_file: Some(session_file.to_path_buf()),
                    resolved_agents,
                    // The revival's follow-up is its `{task}`; a single revived run has no chain dir.
                    original_task: follow_up.to_string(),
                    chain_dir: None,
                    // pi `resolveRevivalControlConfig({ globalConfig, requestedControl,
                    // recoveryControlConfig })` (`subagent-executor.ts:2168`): cyrup's `resume`
                    // action carries no `control` of its own, so the descriptor's resolved config
                    // is the only requested rung; a descriptor without one still gets the
                    // extension-level `subagents.control` block.
                    control: Some(match descriptor.control_config.clone() {
                        Some(control) => control,
                        None => crate::exec::control::resolve_control_config(
                            self.config_snapshot().await.control.as_ref(),
                            None,
                        ),
                    }),
                    // SUBA-N06: the launch's own `includeProgress`, back off the descriptor.
                    include_progress: descriptor.include_progress,
                    run_id: revived_run_id,
                    // pi's plain `resume` does not re-arm the launch deadline
                    // (`subagent-executor.ts:2180-2181`); only the unported steering-recovery path
                    // does. The descriptor's `absoluteDeadlineAt` is evidence, not a limit here.
                    timeout_ms: None,
                    // pi `shareEnabled: recoveryDescriptor?.share` (`:2135`), `artifactsDir` and
                    // `artifactConfig` (`:2106-2107`): the launch's own, never `default()`.
                    share: Some(descriptor.share),
                    artifacts_dir: descriptor.artifacts_dir.clone(),
                    artifact_config: descriptor.artifact_config,
                    // pi `thinkingCeiling: recoveryDescriptor?.thinkingCeiling` (`:2151`) and the
                    // three-way capability intersection (`:2183`): handed over raw, intersected
                    // with the reviver's own inside `spawn_background_steps`, landed on the
                    // runner's env.
                    thinking_ceiling: descriptor.thinking_ceiling.clone(),
                    capability_ceiling: descriptor.capability_ceiling.clone(),
                    // pi `modelOrigin: recoveryDescriptor?.modelOrigin` (`:2149`), consumed as
                    // `storedOrigin` (`model-resolution.ts:382`): the revived run's OWN descriptor
                    // records the origin this run was LAUNCHED with, not a re-derivation over the
                    // persona the overlay just pinned the model on — which would turn `inherited`
                    // into `configured` on the first revive and make a second revive read a
                    // different origin than the first.
                    model_origin: Some(descriptor.model_origin),
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
        let agent_def = self
            .resolve_agent(cwd, agent, AgentReadScope::Both, &roots)
            .map_err(|e| format!("Cannot append step to run '{run_id}': {e}"))?;
        // SUBA-103 — pi builds the appended step through `buildAsyncRunnerSteps`
        // (`subagent-executor.ts:1370` @v0.68.0), whose per-step fast is `s.fast ?? params.fast
        // ?? a.fast` (`async-execution.ts:1118`); the append call carries no call-level `fast`,
        // so it is the step's own key, else the agent's. The runner holds only the step, so an
        // unresolved `None` here would run an agent declaring `fast: true` at the standard tier.
        let fast = step_val
            .get("fast")
            .and_then(serde_json::Value::as_bool)
            .or(agent_def.fast);
        // The same launch-time refusal the async launch applies (pi `async-execution.ts:1012`,
        // inside `buildAsyncRunnerSteps`): a foreign runner cannot honour the priority tier, and
        // an appended step is refused here rather than failing inside the running chain.
        if fast == Some(true)
            && let Some(runner_type) = crate::extension::executor::background::external_runner_type(
                agent_def.runner.as_ref(),
            )
        {
            return Err(format!(
                "Agent '{}' uses runner.type='{runner_type}' and does not support: fast mode.",
                agent_def.name
            ));
        }
        // SUBA-100 — `buildAsyncRunnerSteps` folds each appended step's placement exactly as a
        // launch does (`buildSeqStep`, `async-execution.ts:990-1001` @v0.68.0): `s.machine ??
        // a.machine` (an append carries no call-level `machine`), the runner refusal, then the
        // resolution against the run's own cwd (`cwd: status.cwd ?? input.requestCwd`,
        // `subagent-executor.ts:1379`), with `s.cwd` as the directory ON the machine. Resolved
        // here, before the step is enqueued, so the runner never meets a placed step it cannot
        // run — and an unplaceable one is refused with upstream's sentence instead of enqueued.
        let step_machine = step_val.get("machine").and_then(serde_json::Value::as_str);
        if let Some(machine) = step_machine {
            crate::placement::resolve::validate_machine_name(machine)?;
        }
        let placed = step_machine.is_some() || agent_def.machine.is_some();
        let mut step = SingleStepSpec {
            machine: step_machine.map(crate::placement::StepPlacement::requested),
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task,
            // Only a placed step reads its `cwd` — as the directory ON the machine, moved into the
            // resolved reference by the fold below.
            cwd: step_val
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .filter(|_| placed)
                .map(std::path::PathBuf::from),
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output,
            output_path: None,
            output_mode: None,
            fast,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        };
        let placement_cfg = self.config_snapshot().await;
        let roots = placement_cfg.roots.clone();
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        if placed {
            let paths = RunPaths::for_run(
                &async_root,
                &results_dir,
                &RunId::from_token(run_id.to_string()),
            );
            let run_cwd = control::read_status_file(&paths.status)
                .await
                .ok()
                .flatten()
                .and_then(|status| status.cwd)
                .unwrap_or_else(|| cwd.to_path_buf());
            let mut resolver = crate::placement::resolve::launch_resolver(&run_cwd, &placement_cfg)
                .map_err(|error| format!("Cannot append step to run '{run_id}': {error}"))?;
            crate::placement::resolve::fold_step_placement(
                &mut step,
                crate::placement::resolve::PlacementAgent {
                    name: &agent_def.name,
                    machine: agent_def.machine.as_deref(),
                    runner: agent_def.runner.as_ref(),
                },
                None,
                None,
                false,
                &mut resolver,
            )
            .await?;
        }
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

    /// Every `fast` value found on an agent-running step object anywhere in `value`.
    fn step_fast_values(value: &serde_json::Value, out: &mut Vec<(String, Option<bool>)>) {
        match value {
            serde_json::Value::Object(map) => {
                if let (Some(agent), Some(_task)) = (
                    map.get("agent").and_then(serde_json::Value::as_str),
                    map.get("task"),
                ) {
                    out.push((
                        agent.to_string(),
                        map.get("fast").and_then(serde_json::Value::as_bool),
                    ));
                }
                for child in map.values() {
                    step_fast_values(child, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    step_fast_values(item, out);
                }
            }
            _ => {}
        }
    }

    /// SUBA-100 (append-step half) — `buildAsyncRunnerSteps` resolves every appended step's
    /// placement as a launch does (`buildSeqStep`, `async-execution.ts:990-1001` @v0.68.0):
    /// `s.machine ?? a.machine`, resolved against the run's cwd with `s.cwd` as the directory ON
    /// the machine, BEFORE the step is enqueued; an unknown machine or an unplaceable runner is
    /// refused with upstream's sentence and nothing is enqueued.
    ///
    /// *Gutted by*: `machine: None` on the appended spec (the agent's machine never reaches the
    /// runner, which then refuses a requested-but-unresolved step or runs it locally), or dropping
    /// the fold (the step reaches the runner unresolved).
    #[tokio::test]
    async fn append_step_places_the_step_or_refuses_it_before_enqueueing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agents = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents).expect("mkdir");
        for (name, extra) in [
            ("remote", "machine: fake-box\n"),
            ("plain", ""),
            (
                "generic",
                "machine: fake-box\nrunner: {\"type\": \"external-cli\", \"command\": \"my-cli\"}\n",
            ),
        ] {
            std::fs::write(
                agents.join(format!("{name}.md")),
                format!("---\nname: {name}\ndescription: d\n{extra}---\n\nbody\n"),
            )
            .expect("write persona");
        }
        std::fs::write(
            agents.join("settings.json"),
            serde_json::json!({"subagents":{"machines":{"fake-box":{"cwd":"/srv/repo"}}}})
                .to_string(),
        )
        .expect("settings");
        let herdr = dir.path().join("fake-herdr");
        std::fs::write(
            &herdr,
            "#!/bin/sh\nprintf '%s\\n' '{\"machines\":[{\"id\":\"m-fake\",\"label\":\"fake-box\",\"target\":\"me@fake-box\",\"enabled\":true}]}'\n",
        )
        .expect("herdr");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let executor = SubagentExecutor::new();
        let roots = crate::paths::Roots::sandboxed(dir.path());
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = roots.clone();
            cfg.env_overrides.insert(
                cyrup_herdr::cli::HERDR_BIN.to_string(),
                Some(herdr.display().to_string()),
            );
        }
        let paths = RunPaths::for_run(
            &default_async_root_in(&roots, dir.path()),
            &default_results_dir_in(&roots, dir.path()),
            &RunId::from_token("chainrun0002".to_string()),
        );
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
        let mut status = crate::background::RunStatus::queued(
            RunId::from_token("chainrun0002".to_string()),
            RunMode::Chain,
            Some(std::process::id()),
        );
        status.state = RunState::Running;
        status.current_step = Some(0);
        let mut first = crate::background::StepStatus::pending("plain");
        first.status = crate::background::StepState::Running;
        status.steps = vec![first];
        std::fs::write(&paths.status, serde_json::to_string(&status).expect("json"))
            .expect("write status");

        for step in [
            serde_json::json!({"agent": "remote", "task": "agent rung"}),
            serde_json::json!({"agent": "plain", "task": "step rung", "machine": "fake-box", "cwd": "sub"}),
            serde_json::json!({"agent": "plain", "task": "local"}),
        ] {
            executor
                .control_append_step(
                    dir.path(),
                    Some("chainrun0002"),
                    std::slice::from_ref(&step),
                )
                .await
                .unwrap_or_else(|e| panic!("{step}: {e}"));
        }
        let unknown = executor
            .control_append_step(
                dir.path(),
                Some("chainrun0002"),
                &[serde_json::json!({"agent": "plain", "task": "t", "machine": "nope"})],
            )
            .await
            .expect_err("an unknown machine is refused");
        assert!(
            unknown.starts_with("Herdr machine 'nope' was not found."),
            "{unknown}"
        );
        let generic = executor
            .control_append_step(
                dir.path(),
                Some("chainrun0002"),
                &[serde_json::json!({"agent": "generic", "task": "t"})],
            )
            .await
            .expect_err("a generic command cannot be placed");
        assert!(
            generic.contains("generic external-cli commands cannot be remote-wrapped safely"),
            "{generic}"
        );

        let mut placements = Vec::new();
        for entry in std::fs::read_dir(&paths.append_dir).expect("append dir") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).expect("read");
            let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
            let text = value.to_string();
            let task = if text.contains("agent rung") {
                "agent rung"
            } else if text.contains("step rung") {
                "step rung"
            } else {
                "local"
            };
            let machine = find_key(&value, "machine").cloned();
            placements.push((task, machine));
        }
        placements.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            placements.len(),
            3,
            "three enqueued, two refused: {placements:?}"
        );
        let resolved = |machine: &Option<serde_json::Value>| {
            machine
                .as_ref()
                .and_then(|m| m.get("resolved"))
                .map(|r| (r["id"].clone(), r["cwd"].clone()))
        };
        assert_eq!(
            resolved(&placements[0].1),
            Some((serde_json::json!("m-fake"), serde_json::json!("/srv/repo")))
        );
        assert_eq!(
            placements[1].1, None,
            "an unplaced agent's step stays local"
        );
        assert_eq!(
            resolved(&placements[2].1),
            Some((
                serde_json::json!("m-fake"),
                serde_json::json!("/srv/repo/sub")
            ))
        );
    }

    fn find_key<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        match value {
            serde_json::Value::Object(map) => map
                .get(key)
                .filter(|found| !found.is_null())
                .or_else(|| map.values().find_map(|inner| find_key(inner, key))),
            serde_json::Value::Array(items) => items.iter().find_map(|inner| find_key(inner, key)),
            _ => None,
        }
    }

    /// SUBA-103 (append-step half) — pi builds the appended step through `buildAsyncRunnerSteps`
    /// (`subagent-executor.ts:1370` @v0.68.0): its fast is `s.fast ?? a.fast`
    /// (`async-execution.ts:1118`), and a foreign runner with fast is refused before anything
    /// is enqueued (`:1012`). Mutation killed: `fast: None` on the appended spec (the agent's
    /// `fast: true` never reaches the runner), dropping the step-key read, or dropping the
    /// refusal (the foreign step is enqueued).
    #[tokio::test]
    async fn append_step_resolves_fast_and_refuses_it_for_a_foreign_runner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agents = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents).expect("mkdir");
        for (name, extra) in [
            ("fastie", "fast: true\n"),
            ("plain", ""),
            (
                "foreign",
                "fast: true\nrunner: {\"type\": \"external-cli\", \"command\": \"true\"}\n",
            ),
        ] {
            std::fs::write(
                agents.join(format!("{name}.md")),
                format!("---\nname: {name}\ndescription: d\n{extra}---\n\nbody\n"),
            )
            .expect("write persona");
        }
        let executor = SubagentExecutor::new();
        let roots = crate::paths::Roots::sandboxed(dir.path());
        executor.config_cell().lock().await.roots = roots.clone();
        let paths = RunPaths::for_run(
            &default_async_root_in(&roots, dir.path()),
            &default_results_dir_in(&roots, dir.path()),
            &RunId::from_token("chainrun0001".to_string()),
        );
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
        let mut status = crate::background::RunStatus::queued(
            RunId::from_token("chainrun0001".to_string()),
            RunMode::Chain,
            Some(std::process::id()),
        );
        status.state = RunState::Running;
        status.current_step = Some(0);
        let mut first = crate::background::StepStatus::pending("plain");
        first.status = crate::background::StepState::Running;
        status.steps = vec![first];
        std::fs::write(&paths.status, serde_json::to_string(&status).expect("json"))
            .expect("write status");

        for step in [
            serde_json::json!({"agent": "fastie", "task": "agent rung"}),
            serde_json::json!({"agent": "plain", "task": "step rung", "fast": true}),
            serde_json::json!({"agent": "plain", "task": "neither"}),
        ] {
            executor
                .control_append_step(
                    dir.path(),
                    Some("chainrun0001"),
                    std::slice::from_ref(&step),
                )
                .await
                .unwrap_or_else(|e| panic!("{step}: {e}"));
        }
        let refused = executor
            .control_append_step(
                dir.path(),
                Some("chainrun0001"),
                &[serde_json::json!({"agent": "foreign", "task": "t"})],
            )
            .await
            .expect_err("a foreign runner cannot run fast");
        assert_eq!(
            refused,
            "Agent 'foreign' uses runner.type='external-cli' and does not support: fast mode."
        );

        let mut found = Vec::new();
        for entry in std::fs::read_dir(&paths.append_dir).expect("append dir") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
            step_fast_values(&value, &mut found);
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                ("fastie".to_string(), Some(true)),
                ("plain".to_string(), None),
                ("plain".to_string(), Some(true)),
            ],
            "three enqueued, the foreign one refused"
        );
    }
}
