//! The foreground single-run path: a real, synchronous OS-subprocess run driven to completion.

use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, ModelId, ProviderId, ToolUpdateSink};

use crate::background::RunId;
use crate::discovery::types::{AgentDefinition, AgentReadScope, OutputMode};
use crate::error::SubagentError;
use crate::exec::fallback::{provider_of, resolve_model_inheritance};
use crate::exec::{AgentConfig, RunOptions, SingleResult};
use crate::extension::EXTENSION_ID;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::detach::{
    DETACHED_EXIT_CODE, DetachGate, DetachReason, DetachRefusal,
};
use crate::extension::executor::foreground_control::{
    ForegroundChildEntry, begin_foreground_child,
};
use crate::extension::executor::foreground_history::ForegroundHistoryRun;
use crate::extension::executor::notices::{ForegroundControlEntry, ForegroundControlNotifier};
use crate::extension::executor::paths::{
    drive_foreground_run_sync, write_foreground_output_artifacts,
};
use crate::extension::executor::requests::{ForegroundRunRequest, SingleRunOverrides};
use crate::extension::tool::task_items::{
    normalize_single_output_override, parse_tool_output_mode, resolve_single_output_path,
    resolve_single_run_output_base_dir, resolve_single_run_session_root,
};
use crate::fork_context::{ContextMode, ContextRequest, ForkContext, resolve_effective_context};
use crate::registration::SubagentExtensionConfig;
use crate::spawn::depth::{DepthEnvelope, resolve_effective_depth};

/// The persona-and-model half of [`SubagentExecutor::run_foreground_impl`]'s prologue, as resolved
/// by [`SubagentExecutor::resolve_run_agent`].
///
/// Split out as a struct rather than a tuple because the fields divide cleanly in two: `agent`,
/// `agent_config` and `resolved_context` survive the run's [`RunOptions`] and are still needed to
/// drive, register and report it; the remaining five are consumed by
/// [`SubagentExecutor::build_foreground_run_options`] and never read again.
struct ResolvedRunAgent {
    /// The resolved persona (pi's `agentConfig` source), alias-canonicalized by discovery.
    agent: AgentDefinition,
    /// The persona lowered to an executable config, already carrying SUBA-047's caller tool budget.
    agent_config: AgentConfig,
    /// C19 (R-SA-111): the run's *resolved* fork/fresh mode, captured before `fork_context` is
    /// moved into [`RunOptions`].
    resolved_context: ContextMode,
    /// The resolved fork context itself, moved into [`RunOptions::fork_context`].
    fork_context: ForkContext,
    /// SUBA-008's three-rung chain: caller > agent frontmatter > extension config.
    turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-073's two-rung merge: `config.permissions` + the agent's own `permission:`
    /// frontmatter, agent winning on conflict. Resolved at the same seam as `turn_budget` above,
    /// which is the only point where both the live extension config and the resolved agent are
    /// in hand.
    permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
    /// SUBA-003's `subagents.modelScope` policy, carried on so the fallback ladder's own
    /// out-of-scope entries warn.
    model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    /// SUBA-078's `subagents.maxThinking` ceiling, from the same discovery pass. Unlike
    /// `model_scope` this one REFUSES rather than warns for a fallback candidate.
    max_thinking: Option<String>,
    /// R-SA-038's availability set, already widened by the explicit override and any inherited
    /// parent-session model.
    available_models: Vec<ModelId>,
    /// The model override that survived the fail-closed `modelScope` gate.
    effective_override: crate::exec::fallback::ModelOverride,
    /// SUBA-088 — pi `currentProvider = parentModel?.provider` (`subagent-executor.ts:3648`
    /// @v0.64.0): the REMEMBERED parent model's provider, handed to `run_sync` as
    /// [`RunOptions::preferred_provider`] (pi's `preferredModelProvider: currentProvider`,
    /// `:3825`) — the second rung under the agent's own `model_provider`.
    preferred_provider: Option<ProviderId>,
}

/// The run-scoped identity, sinks and directories [`SubagentExecutor::resolve_run_channels`]
/// resolves for one foreground run — everything the run is addressed BY, as opposed to configured
/// by.
struct RunChannels {
    /// R-SA-035: the wall-clock deadline `run_sync` races the child against.
    deadline_at: Option<std::time::Instant>,
    /// This run's own real, stable id (pi `runId`).
    run_id: RunId,
    /// `subagents.control` merged with this call's own `control` override, resolved ONCE so the
    /// notifier's channel gate and [`RunOptions::control_config`] cannot disagree.
    resolved_control: crate::exec::control::ResolvedControlConfig,
    /// The ordered control-notice pump reading that config.
    control_notifier: ForegroundControlNotifier,
    /// T6 artifact-quadruple config (`enabled` already honors SUBA-041's `artifacts: false`).
    art_cfg: crate::artifacts::ArtifactConfig,
    /// The artifacts root, which doubles as the base a relative `output` resolves against.
    art_dir: PathBuf,
    /// The resolved single-run output path (pi `resolveSingleRunOutputBaseDir` + the persona's own
    /// `output:` fallback).
    output_path: Option<PathBuf>,
    /// `inline` (pi's own default) or `file-only`, from the PARAM alone.
    output_mode: OutputMode,
    /// The child's own session directory (`<root>/run-0`), or `None` for pi's `--no-session` branch.
    session_dir: Option<PathBuf>,
}

/// Everything [`SubagentExecutor::build_foreground_run_options`] folds into one [`RunOptions`].
///
/// Bundled into a borrowed request for the same reason [`ForegroundRunRequest`] is — nineteen
/// positional arguments is not an API — and the split between owned and borrowed fields is
/// load-bearing: an owned field is MOVED into the resulting [`RunOptions`], a borrowed one is
/// cloned there because [`SubagentExecutor::run_foreground_impl`] still needs it to drive and tear
/// down the run.
struct ForegroundRunOptionsInput<'a> {
    overrides: SingleRunOverrides,
    cwd: &'a Path,
    timeout_ms: Option<u64>,
    cancel: CancelToken,
    /// Borrowed for [`AgentDefinition::default_reads`] (SUBA-054); the caller drives the run with it.
    agent: &'a AgentDefinition,
    /// SUBA-073's two-rung merge: `config.permissions` + the agent's own `permission:`
    /// frontmatter, agent winning on conflict. Resolved at the same seam as `turn_budget` above,
    /// which is the only point where both the live extension config and the resolved agent are
    /// in hand.
    turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
    /// The extension config's in-process binary override, resolved from the same snapshot as
    /// `turn_budget`/`permission_rules` above. `None` leaves the run resolving its command from
    /// the environment, which is every installed configuration.
    spawn_command: Option<crate::spawn::SpawnCommand>,
    model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    max_thinking: Option<String>,
    available_models: Vec<ModelId>,
    effective_override: crate::exec::fallback::ModelOverride,
    preferred_provider: Option<ProviderId>,
    fork_context: ForkContext,
    deadline_at: Option<std::time::Instant>,
    /// Borrowed: the same id the caller registers, tears down and returns.
    run_id: &'a RunId,
    /// Borrowed: the notifier built from it outlives this call.
    resolved_control: &'a crate::exec::control::ResolvedControlConfig,
    /// Borrowed: the caller flushes the very same pump after the run settles.
    control_notifier: &'a ForegroundControlNotifier,
    output_path: Option<PathBuf>,
    output_mode: OutputMode,
    session_dir: Option<PathBuf>,
    /// `art_cfg.enabled` — G80 gates verify memoization on the SAME flag as the quadruple.
    artifacts_enabled: bool,
    /// `art_cfg.include_transcript` — pi's third transcript gate term (`includeTranscript !==
    /// false`, `execution.ts:1838`), carried beside `artifacts_enabled` so the transcript switch
    /// reads the SAME resolved config as the quadruple.
    include_transcript: bool,
    /// Borrowed: the caller writes the artifact quadruple into the same root.
    art_dir: &'a Path,
    /// WORKFLOW_14 — [`ForegroundRunRequest::workflow_steer`], borrowed. The three
    /// [`RunOptions`] steer paths are derived from it; `register_foreground_controls` stores the
    /// same handle on the child's entry, so the spawn side and the control side cannot disagree
    /// about where this child's inbox is.
    workflow_steer:
        Option<&'a crate::extension::executor::foreground_control::ForegroundChildSteerHandle>,
}

/// The borrowed identity triple [`SubagentExecutor::register_foreground_controls`] stamps onto a
/// fresh [`ForegroundControlEntry`] (WORKFLOW_6 §4.2) — a struct, not three more positional args,
/// because that function is already at five and `clippy::too_many_arguments` is live.
struct ForegroundControlIdentity<'a> {
    /// The run's effective working directory (pi `ForegroundRunControl.cwd`).
    cwd: &'a Path,
    /// `Some` only for a child launched by `WorkflowRunHost::launch`.
    parent_workflow_run_id: Option<&'a RunId>,
    /// The workflow lane key that child was launched under.
    workflow_key: Option<&'a crate::workflows::WorkflowKey>,
    /// WORKFLOW_14 — borrowed from [`ForegroundRunRequest::workflow_steer`], the SAME value
    /// [`SubagentExecutor::build_foreground_run_options`] derived this child's three spawn paths
    /// from. `Some` iff `parent_workflow_run_id` is `Some`.
    workflow_steer:
        Option<&'a crate::extension::executor::foreground_control::ForegroundChildSteerHandle>,
}

impl SubagentExecutor {
    // ---------------------------------------------------------------------------------------
    // Foreground single-run dispatch (the tool's synchronous shape; exec::run_sync end to end)
    // ---------------------------------------------------------------------------------------

    /// Run one subagent task to completion in the foreground, synchronously (func-SA §5.2; the
    /// tool's default/`bg: false` shape). Resolves the agent via real discovery, resolves
    /// fork-context if requested, builds [`AgentConfig`]/[`RunOptions`], and drives
    /// [`crate::exec::run_sync`] — which spawns a REAL child OS process via
    /// [`crate::spawn::SpawnedChild::spawn`] (func-SA §1.1's mandated mechanism).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before agent discovery,
    /// fork-context resolution, or any spawn, so a blocked call touches none of that setup work.
    /// Otherwise returns [`SubagentError`] if the agent cannot be resolved, or fork-context
    /// resolution fails hard (R-SA-137). A subprocess-level failure (nonzero exit, timeout, …) is
    /// NOT an `Err` here — it is reported as a normal (non-`Ok`-gated) field on the returned
    /// [`SingleResult`], matching `run_sync`'s own contract. [`crate::exec::run_sync`] also
    /// independently re-checks this same guard as its own first action (defense in depth, since it
    /// is the sole chokepoint every spawn path in this crate funnels through) — the check here
    /// exists specifically to satisfy R-SA-055's stronger "before discovery" ordering, which
    /// `run_sync`'s own check alone cannot provide since discovery has already happened by the
    /// time `run_sync` is called.
    pub async fn run_foreground(
        &self,
        cwd: &Path,
        agent_name: &str,
        task: &str,
        context: Option<ContextRequest>,
        model_override: Option<ModelId>,
        timeout_ms: Option<u64>,
    ) -> Result<SingleResult, SubagentError> {
        // No host `ToolCallId`/cancellation seam reaches this flat entry point's callers (the slash
        // dispatch path and this crate's own tests) — a fresh, never-cancelled token here matches
        // the pre-existing behavior for those callers exactly; the live host token is threaded
        // through [`ForegroundRunRequest::cancel`] by [`run_foreground_streaming`]'s callers instead
        // (`SubagentTool::execute` -> `route_single`).
        self.run_foreground_impl(
            ForegroundRunRequest {
                // The flat entry point (`/run`, this crate's own tests) exposes no per-call override
                // surface at all, so SUBA-041's bundle is empty here — identical to pre-SUBA-041.
                overrides: SingleRunOverrides::default(),
                cwd,
                agent_name,
                task,
                // pi's slash-command surfaces (`/run`, `/chain`, `/parallel`, `/run-chain`)
                // explicitly set `agentScope: "both"` on every dispatch they build
                // (`slash-commands.ts:999,1017,1047,1071` @v0.34.0) — this flat entry point has no caller
                // that ever narrows the scope, so `Both` here is not a default guess but pi's own
                // explicit, always-supplied value for this exact call shape.
                agent_scope: AgentReadScope::Both,
                context,
                model_override,
                timeout_ms,
                cancel: CancelToken::new(),
                // This flat entry point has no `workflowScript` concept at all.
                parent_workflow_run_id: None,
                workflow_key: None,
                workflow_steer: None,
            },
            None,
        )
        .await
        .map(|(result, _run_id)| result)
    }

    /// C19 (live foreground progress): the same foreground single run as [`crate::extension::SubagentExecutor::run_foreground`], but
    /// STREAMING live progress through the host [`ToolUpdateSink`] as the child's NDJSON stdout
    /// arrives — the crate-side of pi's `onUpdate`/`fireUpdate` (`runs/foreground/execution.ts:805-826`).
    /// The tool call still blocks and still returns the same terminal [`SingleResult`]; the
    /// difference is that a still-running child no longer surfaces zero progress until completion.
    /// Each `tool_execution_start`/`tool_execution_end`/assistant `message_end` folds into a
    /// [`crate::tui::events::LiveProgressSnapshot`], is wrapped in a
    /// [`crate::tui::events::SubagentUpdatePayload`] (the `ToolUpdate.details` wire shape `cyrup-tui`
    /// renders as the inline subagent-result surface, C20), and is delivered through `on_update`.
    ///
    /// # Errors
    ///
    /// Identical to [`crate::extension::SubagentExecutor::run_foreground`].
    ///
    /// Returns this run's own real, stable [`RunId`] alongside the result (pi `runId`,
    /// `subagent-executor.ts:4941` @v0.43.0) — the SAME id [`RunOptions::run_id`] threaded through the
    /// child's intercom-bridge registration — so a caller (`route_single`) can cite it verbatim in
    /// an out-of-band result-intercom payload/receipt (R-SA-123/124/125) rather than minting a
    /// second, disconnected id only for that message.
    pub async fn run_foreground_streaming(
        &self,
        req: ForegroundRunRequest<'_>,
        on_update: ToolUpdateSink,
    ) -> Result<(SingleResult, RunId), SubagentError> {
        self.run_foreground_impl(req, Some(on_update)).await
    }

    /// Shared body for [`crate::extension::SubagentExecutor::run_foreground`] / [`crate::extension::SubagentExecutor::run_foreground_streaming`]: resolves the persona +
    /// fork-context, builds the [`AgentConfig`]/[`RunOptions`], and drives [`crate::exec::run_sync`]
    /// — optionally installing a live-progress sink (`on_update = Some`, C19) that folds the child's
    /// NDJSON stream into [`crate::tui::events::SubagentUpdatePayload`] updates. Returns the run's own
    /// [`RunId`] alongside the [`SingleResult`] (see [`crate::extension::SubagentExecutor::run_foreground_streaming`]'s doc).
    pub(crate) async fn run_foreground_impl(
        &self,
        req: ForegroundRunRequest<'_>,
        on_update: Option<ToolUpdateSink>,
    ) -> Result<(SingleResult, RunId), SubagentError> {
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let ResolvedRunAgent {
            agent,
            agent_config,
            resolved_context,
            fork_context,
            turn_budget,
            permission_rules,
            model_scope,
            max_thinking,
            available_models,
            effective_override,
            preferred_provider,
        } = self.resolve_run_agent(&req, &cfg, depth).await?;
        let ForegroundRunRequest {
            overrides,
            cwd,
            task,
            timeout_ms,
            cancel,
            parent_workflow_run_id,
            workflow_key,
            workflow_steer,
            ..
        } = req;

        let RunChannels {
            deadline_at,
            run_id,
            resolved_control,
            control_notifier,
            art_cfg,
            art_dir,
            output_path,
            output_mode,
            session_dir,
        } = self.resolve_run_channels(&cfg, &overrides, cwd, &agent, timeout_ms);

        let run_options = self.build_foreground_run_options(ForegroundRunOptionsInput {
            overrides,
            cwd,
            timeout_ms,
            cancel,
            agent: &agent,
            turn_budget,
            permission_rules,
            spawn_command: cfg.spawn_command.clone(),
            model_scope,
            max_thinking,
            available_models,
            effective_override,
            preferred_provider,
            fork_context,
            deadline_at,
            run_id: &run_id,
            resolved_control: &resolved_control,
            control_notifier: &control_notifier,
            output_path,
            output_mode,
            session_dir,
            artifacts_enabled: art_cfg.enabled,
            include_transcript: art_cfg.include_transcript,
            art_dir: &art_dir,
            workflow_steer: workflow_steer.as_ref(),
        });

        // VL-S11b — the run's detach gate, minted BEFORE its control surface is published so the
        // handle `/subagents-detach` fires and the gate this driver races are one object. The
        // driver keeps the gate; the control entry gets a [`ForegroundDetachHandle`] clone.
        let detach_gate = DetachGate::new();

        self.register_foreground_controls(
            &run_id,
            &run_options,
            &agent,
            task,
            ForegroundControlIdentity {
                cwd,
                parent_workflow_run_id: parent_workflow_run_id.as_ref(),
                workflow_key: workflow_key.as_ref(),
                // The SAME handle the run options above were derived from — one value, both sides.
                workflow_steer: workflow_steer.as_ref(),
            },
            &detach_gate,
        )
        .await;

        let art_paths =
            write_foreground_input_artifact(&art_cfg, &art_dir, &run_id, &agent.name, task);

        // Both read BEFORE `run_options` is moved into the driving future below.
        //
        // `output_path_configured` is pi's `if (options.outputPath)` (`execution.ts:627`), the
        // gate on the receipt's `outputSaveError`; `run_cancel` is pi's `options.signal`, the
        // FOURTH of upstream's four detach guards (`:613`).
        let output_path_configured = run_options.output_path.is_some();
        let run_cancel = run_options.cancel.clone();

        // ── The producer split (VL-S11b §C.5) ────────────────────────────────────────────────
        //
        // Upstream's `detachForeground` is cheap because pi's child is an IN-PROCESS session
        // object: the receipt is a snapshot, the tool call returns, and the session's callbacks
        // keep firing (`execution.ts:612-649`). cyrup's child is a REAL OS PROCESS owned by this
        // `drive_foreground_run_sync` future, so **dropping the future kills the very child the
        // feature exists to keep alive.**
        //
        // The future is therefore built from OWNED inputs and BOXED. A `Pin<Box<dyn Future>>` is
        // `Unpin` and `'static`, so `&mut drive` can be raced in a `select!` here and then the
        // whole value MOVED into a spawned task on a detach — the child is handed over, never
        // dropped. (A stack `tokio::pin!` would be racy-cheap but unmovable, which is exactly the
        // property this needs to NOT have.)
        let mut drive: std::pin::Pin<Box<dyn std::future::Future<Output = SingleResult> + Send>> = {
            let agent_config = agent_config.clone();
            let drive_task = task.to_string();
            let agent_name = agent.name.clone();
            Box::pin(async move {
                drive_foreground_run_sync(
                    &agent_config,
                    &drive_task,
                    run_options,
                    &agent_name,
                    resolved_context,
                    on_update,
                )
                .await
            })
        };

        // The drive loop races its own settle against a detach request — pi's `detachForeground`
        // guard set, expressed as a race because the request arrives from another task.
        //
        // `biased` so the settle arm is polled FIRST on every wake: a run that has already
        // finished must win over a detach request landing in the same tick, which is upstream's
        // `sessionSettled` guard (`:613`) evaluated in upstream's own order.
        let detach_reason = loop {
            tokio::select! {
                biased;
                settled = &mut drive => {
                    // Every settle path closes the gate — a requester parked on a gate nobody
                    // closes would hang forever. `close` is a no-op after `accept`, so this
                    // cannot retract an already-accepted detach (it never runs on that path
                    // anyway: an accepted detach leaves this loop through `break`).
                    detach_gate.close(DetachRefusal::SessionSettled);
                    let mut settled = settled;
                    stamp_intercom_detach_reason(&mut settled);
                    self.settle_attached_foreground_run(
                        &run_id,
                        cwd,
                        &control_notifier,
                        &art_paths,
                        &art_cfg,
                        &settled,
                    )
                    .await;
                    return Ok((settled, run_id));
                }
                reason = detach_gate.requested() => {
                    // pi `options.signal?.aborted` (`:613`): a cancelled run is not detachable,
                    // and refusing it must NOT stop driving — the child is being torn down and
                    // the caller is still owed its terminal result.
                    if run_cancel.is_cancelled() {
                        detach_gate.close(DetachRefusal::Aborted);
                        continue;
                    }
                    break reason;
                }
            }
        };

        // ── The detach branch ────────────────────────────────────────────────────────────────
        //
        // pi `detachForeground` (`execution.ts:615-649`): mint, PUBLISH, and only then mark the
        // run detached. Upstream's ordering is load-bearing and is preserved — `detached = true`
        // is set only after `onDetachReceipt` returned `true` (`:646-648`), so a publish failure
        // leaves the run attached and abortable rather than orphaned.
        let receipt = detach_receipt(&agent.name, task, detach_reason, output_path_configured);

        // The publish: the run becomes an entry in `foreground_runs` whose only child is
        // `"detached"` (minted by `foreground_history_child_status` off `receipt.detached`). That
        // in-memory entry is the WHOLE of what `status` by id, `bg_wait` by id/`all` and
        // `ExecutorForegroundProbe` need — upstream writes nothing to disk for a detached run
        // either (`foreground-history.ts:67-69`), and cyrup's `persist.rs` refuses one outright.
        self.remember_foreground_run(
            &run_id,
            crate::background::RunMode::Single,
            cwd,
            &[&receipt.result],
        );
        if !self.stamp_detached_receipt(&run_id, receipt.output_save_error.as_deref()) {
            // pi `accepted = options.onDetachReceipt?.(receipt) === true` returning false
            // (`:643-645`): the run stays ATTACHED. cyrup's one failure mode is an unattributed
            // run — `remember_foreground_run` refuses a run with no `current_session_id` BY TYPE
            // (`ForegroundHistoryRun::session_id` is a required `SessionId`), so there is no
            // addressable identity to hand back and detaching would orphan the child.
            detach_gate.close(DetachRefusal::LifecycleFinished);
            tracing::warn!(
                run_id = %run_id,
                "detach refused: the run has no session identity, so it cannot be remembered or \
                 recovered by id"
            );
            let mut settled = drive.await;
            stamp_intercom_detach_reason(&mut settled);
            self.settle_attached_foreground_run(
                &run_id,
                cwd,
                &control_notifier,
                &art_paths,
                &art_cfg,
                &settled,
            )
            .await;
            return Ok((settled, run_id));
        }
        detach_gate.accept();

        // pi drops the live control on a detach exactly as on a settle — which is why
        // `fleet-view.ts:406-408` re-checks `!activeForegroundIds.has(runId)` before rendering a
        // remembered run. cyrup's two maps are disjoint by construction and this is the edge that
        // keeps them so.
        self.settle_foreground_run(&run_id, &control_notifier).await;

        // NO `persist_foreground_run_history_for` here, deliberately: `persist.rs`'s
        // `is_persistable` refuses a run whose child is `"detached"` (it is not one of the four
        // `RESTORABLE` statuses), so the call would be a no-op with a config read and a directory
        // resolution attached. The continuation's reconcile is where the run first becomes
        // persistable — see `spawn_detached_foreground_continuation` for what it does and does
        // not do there.
        spawn_detached_foreground_continuation(
            std::sync::Arc::clone(&self.foreground_runs),
            drive,
            run_id.clone(),
            art_paths,
            art_cfg,
        );

        Ok((receipt.result, run_id))
    }

    /// The ATTACHED settle tail — everything `run_foreground_impl` does once a foreground run
    /// reaches a terminal [`SingleResult`], in the one order that cannot be raced.
    ///
    /// Factored out because there are now TWO paths that reach it: the ordinary settle, and a
    /// detach whose receipt could not be published (which leaves the run attached, pi
    /// `execution.ts:643-645`). Two copies of this ordering would be two orderings.
    async fn settle_attached_foreground_run(
        &self,
        run_id: &RunId,
        cwd: &Path,
        control_notifier: &ForegroundControlNotifier,
        art_paths: &crate::artifacts::ArtifactPaths,
        art_cfg: &crate::artifacts::ArtifactConfig,
        result: &SingleResult,
    ) {
        // WORKFLOW_7 — pi `rememberForegroundRun` (`subagent-executor.ts:4057`), on the settle
        // path with the results in hand. BEFORE `settle_foreground_run`, whose own doc calls out
        // an ordering that must not be disturbed: this call touches only `foreground_runs` (a
        // DIFFERENT map from the one `settle_foreground_run` drops an entry from), so it cannot
        // race that ordering either way, but placing it here keeps every foreground-history write
        // together, ahead of teardown, in one place.
        self.remember_foreground_run(run_id, crate::background::RunMode::Single, cwd, &[result]);

        self.settle_foreground_run(run_id, control_notifier).await;

        write_foreground_output_artifacts(art_paths, art_cfg, run_id.as_str(), result);

        // pi `persistForegroundRunHistory` (`foreground-history.ts:136`), after the in-memory
        // record exists. Bounded, 0600, atomic; a write failure never alters the `SingleResult`
        // the caller observes, exactly as `record_run_history`'s own best-effort contract.
        self.persist_foreground_run_history_for(cwd).await;

        // R-SA-058: the per-attempt raw-stdout tee `run_sync` writes to
        // `<attempt_scratch_dir(cwd)>/attempt-<n>.jsonl` (SUBA-072: `<temp_root_dir>/scratch/
        // <cwd_key>`, never under the project tree) is this run's persisted, observable child
        // record and MUST survive the orchestrator, exactly as it does on every other spawn path in
        // this crate (the tool single/parallel/chain fan-outs and the background hop-2 runner all
        // leave it in place — it is the single observation channel the crate's integration tests read
        // back, e.g. `tool_parallel_chain_integration`'s `/run [model=…]` tee check and
        // `companions_wiring_proof`). This mirrors pi, which likewise never deletes its persisted
        // child NDJSON stream — pi only cleans the *transient* per-spawn prompt/task-overflow dir it
        // creates under `os.tmpdir()` (`pi-subagents/src/runs/shared/pi-args.ts:787-855` @v0.64.0
        // build it, `:1052-1059` `cleanupTempDir` removes it, invoked from
        // `pi-subagents/src/runs/foreground/execution.ts:491`/`:560`/`:602`/`:635`/`:1387`/`:1426`),
        // a dir that lives OUTSIDE the working tree and never holds the event stream. An earlier
        // revision erroneously `remove_dir_all`'d the whole scratch dir here, which silently
        // discarded that tee the moment a foreground `/run` completed — defeating the tee's own
        // stated purpose and diverging from every sibling path — so no such deletion is performed.
    }

    /// Stamp the detach's `outputSaveError` onto the remembered child, and report whether the
    /// receipt was PUBLISHED at all — pi's `onDetachReceipt` return value (`execution.ts:2133`),
    /// whose `false` keeps the run attached.
    ///
    /// Returns `false` only when [`SubagentExecutor::remember_foreground_run`] declined to record
    /// the run, which it does for exactly one reason: no `current_session_id`, which
    /// [`ForegroundHistoryRun`](crate::extension::executor::foreground_history::ForegroundHistoryRun)'s
    /// required `session_id` makes unrepresentable rather than merely unchecked.
    ///
    /// The stamp itself is here and not inside `remember_foreground_run` because cyrup's
    /// [`SingleResult`] carries no `output_save_error` field to thread it through — see
    /// [`DetachReceipt`] for why the receipt carries it separately. This is the
    /// producer the history child's own field doc says it had been waiting for, and its consumer
    /// is the fleet detail pane's `Output warning:` line (`tui/fleet.rs`, pi
    /// `run-status.ts:230`).
    fn stamp_detached_receipt(&self, run_id: &RunId, output_save_error: Option<&str>) -> bool {
        let mut runs = self
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(run) = runs.get_mut(run_id) else {
            return false;
        };
        if let Some(message) = output_save_error
            && let Some(child) = run.children.first_mut()
        {
            child.output_save_error = Some(message.to_string());
        }
        true
    }

    /// Resolve the persona this foreground run will spawn, together with everything about it
    /// the run's [`RunOptions`] is built from: its [`AgentConfig`], the effective fork context,
    /// the three-rung turn budget, the candidate-model ladder and the model override that
    /// survived [`crate::exec::model_scope`]'s fail-closed gate.
    ///
    /// SUBA-003: the persona AND this cwd's effective `subagents.modelScope` policy come back
    /// from ONE discovery pass, so the scope gating this run's model is the scope on disk right
    /// now (pi `discoverAgents` -> `{ agents, modelScope }`, `agents.ts:1727,1780` @v0.43.0).
    ///
    /// # Errors
    ///
    /// Surfaces [`SubagentError::AgentNotFound`]/[`SubagentError::MalformedSettings`] from
    /// discovery, a hard fork-context failure (R-SA-137), a malformed `subagents.turnBudget`, and
    /// [`SubagentError::ModelOutOfScope`] — all of them BEFORE the run id is minted and before
    /// any subprocess is spawned.
    async fn resolve_run_agent(
        &self,
        req: &ForegroundRunRequest<'_>,
        cfg: &SubagentExtensionConfig,
        depth: DepthEnvelope,
    ) -> Result<ResolvedRunAgent, SubagentError> {
        let (agent, model_scope, max_thinking) = self.resolve_agent_with_model_scope(
            req.cwd,
            req.agent_name,
            req.agent_scope,
            &cfg.roots,
        )?;
        // Fork default-mode (Tier-2, pi `resolveAgentDefaultContextPolicy`): an OMITTED call-site
        // `context` (`None`) takes the defaults ladder rather than being forced to `Fresh`; an
        // explicit call-site value still wins (`resolve_effective_context`).
        //
        // SUBA-079: the availability test is consulted only when nothing explicit was named, so the
        // extra session open it costs happens only where an implicit fork is actually in play.
        let can_prefer_fork = match req.context {
            Option::None => self.can_prefer_fork(req.cwd).await,
            Some(_) => false,
        };
        let effective_context = resolve_effective_context(
            req.context,
            req.agent_name,
            agent.default_context,
            crate::fork_context::resolve_default_subagent_context(
                cfg.default_subagent_context.as_ref(),
            )
            .map_err(SubagentError::Management)?,
            can_prefer_fork,
        )?;
        // SUBA-075, re-gated by SCOPE_19/A3 [CYRUP-DELTA]: pi `prepareForkThinking`
        // (`runs/foreground/subagent-executor.ts:5858-5885` @v0.57.0) decides BEFORE the branch is
        // cut whether the child's resolved model ladder contains an Anthropic candidate, and
        // forces the sanitized fork's reasoning off when it does. cyrup deletes that provider
        // walk: the resolver's unconditional stripping already makes the branch safe, and the
        // override now fires inside `ForkContextResolver::resolve` on the one fact that actually
        // settles it — whether the strip disturbed the branch's FINAL assistant entry
        // (`Sanitization::tail_disturbed`). The provider gate disabled reasoning on branches that
        // were already safe, and misaligning the child's effort from the parent's forfeits
        // prompt-prefix reuse on exactly the call shape that inherits the most context.
        //
        // What remains caller-side is only what the resolver cannot know
        // (`fork_requires_thinking_off`): an external (non-cyrup) child resolves no model from
        // this registry, so the crate cannot speak for its continuation semantics and stays on
        // upstream's conservative `?? true` side.
        //
        // The `!= Fork` arm is a short-circuit, not a claim: a `Fresh` request returns from
        // `resolve` before the flag is ever read.
        let force_thinking_off =
            effective_context != ContextMode::Fork || fork_requires_thinking_off(&agent);
        let fork_context = self
            .resolve_context(req.cwd, effective_context, force_thinking_off)
            .await?;
        // C19: the run's *resolved* context (R-SA-111) — captured before `fork_context` is moved
        // into the run's [`RunOptions`] — is what the live-progress payload's `[fork]` badge
        // reflects.
        let resolved_context = fork_context.mode;

        let mut agent_config = AgentConfig::from_agent_definition(&agent, depth);
        // SUBA-047 / pi `const toolBudgetInput = params.toolBudget ?? agentConfig.toolBudget ??
        // params.configToolBudget` (`runs/background/async-execution.ts:1298` @v0.43.0, mirrored on
        // the foreground path). Collapsing the caller override onto the resolved persona here is
        // exactly that `??` chain: the encoder at `exec/mod.rs`'s spawn overlay reads
        // `agent.tool_budget` and needs no new plumbing, and the precedence stays caller >
        // frontmatter. `None` on the override leaves the persona's own budget untouched.
        if let Some(budget) = req.overrides.tool_budget.clone() {
            agent_config.tool_budget = Some(budget);
        }
        // SCOPE_19/A1 — the launch thinking-resolution ladder, folded once here so every consumer
        // downstream (`apply_thinking_suffix`'s per-attempt suffix, the `assert_thinking_within_
        // ceiling` sweep, the spawn overlay) reads ONE resolved value off `agent.thinking`:
        //
        //   1. a `:level` suffix on a caller-supplied model — wins downstream, because
        //      `apply_thinking_suffix` never replaces an existing recognized suffix without a fork
        //      override's licence;
        //   2. the caller's explicit `thinking` param (already lowered/validated at the tool
        //      boundary by `lower_launch_thinking`);
        //   3. the persona's own `thinking:` (as merged by discovery, including `disableThinking`);
        //   4. the live parent session's level (`remembered_parent_thinking` — the thinking twin
        //      of the `remembered_parent_model` read above), the DEFAULT: model and effort aligned
        //      is what lets the child serve the shared prompt prefix from the parent's cache;
        //   5. none — the suffix stays off, today's terminal fallthrough.
        //
        // The ceiling sweep below this fold polices the WINNER, so an explicit or inherited level
        // above the configured ceiling is refused with the existing ceiling error — inheritance
        // moves the default, never the limit.
        agent_config.thinking = req
            .overrides
            .thinking
            .clone()
            .or(agent_config.thinking)
            .or_else(|| self.remembered_parent_thinking());
        // SUBA-008 / pi `resolveTurnBudgetConfig(effectiveParams.turnBudget ?? deps.config.turnBudget)`
        // (`subagent-executor.ts:4928-4929` @v0.43.0), where `effectiveParams.turnBudget` has
        // already absorbed the agent's own frontmatter through `applySingleAgentLaunchDefaults`
        // (`:1940-1942`). Flattened here into the one three-rung chain it really is —
        // caller > agent frontmatter > extension config — resolved at the single point where all
        // three are in hand.
        //
        // The config rung is validated (not merely parsed) at this seam, exactly as upstream
        // validates it here rather than at config load: a malformed `subagents.turnBudget` refuses
        // THIS call with upstream's own message instead of silently disarming the budget.
        let turn_budget = match req.overrides.turn_budget.or(agent.default_turn_budget) {
            Some(budget) => Some(budget),
            None => crate::exec::turn_budget::resolve_turn_budget_config(
                cfg.turn_budget.as_ref(),
                "turnBudget",
            )
            // `Management` is this crate's "the message IS the user-facing text" variant, which is
            // what upstream's `buildRequestedModeError(effectiveParams, turnBudget.error)`
            // (`subagent-executor.ts:4929`) produces: the validation string verbatim, unprefixed.
            .map_err(SubagentError::Management)?,
        };
        // SUBA-073 / pi `resolvePermissionRules(ctx.config?.permissions, agentConfig.permissions)`
        // (`async-execution.ts`, `api/preflight.ts`): the two-rung merge (no per-call tier —
        // permissions has none) resolved here, the single point where both the live extension
        // config (`cfg`) and the resolved agent are in hand — the exact seam `turn_budget`'s own
        // config rung resolves at, immediately above.
        let permission_rules = crate::exec::permissions::resolve_permission_rules(
            crate::exec::permissions::validate_permission_config(
                cfg.permissions.as_ref(),
                "config.permissions",
            )
            .map_err(SubagentError::Management)?
            .as_ref(),
            agent.permission_rules.as_ref(),
        );
        // R-SA-038: `build_model_candidates` filters the ladder to `available_models`, so an
        // explicit `model` override (pi `slash-commands.ts:681` `/run [model=…]`, and the tool's
        // SINGLE-mode `model`) must be ADDED to the availability set — otherwise the override is
        // silently filtered out and the child runs the agent's own default model instead of the
        // requested one. This mirrors `ExecSingleStepExecutor::run_single`, which likewise pushes
        // each step's `model` override into `available_models` before building the ladder.
        let mut available_models = agent_config
            .fallback_models
            .iter()
            .cloned()
            .chain(agent_config.model.clone())
            .collect::<Vec<_>>();
        if let Some(model) = &req.model_override {
            available_models.push(model.clone());
        }
        // Session-model inheritance (pi `resolveEffectiveSubagentModel(params.model, agentConfig.model,
        // parentModel, …)`, `subagent-executor.ts:3553-3560` @v0.43.0; `parentModel` is `ctx.model`
        // captured at `:4344-4345`): when this run has NEITHER a per-call `model`
        // override NOR a persona `model:` of its own, inherit the live PARENT session model
        // (`HostServices::current_model`) as the primary candidate — otherwise an inheriting persona
        // has an EMPTY ladder and the run hard-fails with "no candidate model available"
        // (`exec/mod.rs`). `resolve_model_inheritance` both selects the effective override (per-call >
        // persona > inherited) and pushes the inherited id into `available_models` so it survives the
        // allowlist filter. `None` inherited (headless / no live session) degrades to the persona's
        // own `model`/`fallback_models` exactly as before.
        //
        // SUBA-003 fail-closed gate: when `subagents.modelScope.enforce` is armed and the caller
        // asked for a model no `allow` pattern matches, this returns `Err` and the run is REFUSED
        // here — before `deadline_at`, before the `RunId` is minted, and before any subprocess is
        // spawned. The violation is mapped to `SubagentError::ModelOutOfScope`, whose `Display` is
        // pi's verbatim message, so the caller (tool result / slash command) sees exactly WHY the
        // run did not happen instead of silently getting a different model's output.
        // pi's `requestParentModel` (`subagent-executor.ts:4343-4345` @v0.43.0), threaded into
        // `runSinglePath` and reaching `resolveEffectiveSubagentModel` at `:3553` — the REMEMBERED
        // parent model, not a bare live `ctx.model` read. See
        // [`SubagentExecutor::remembered_parent_model`] for why the two differ.
        let parent_model = self.remembered_parent_model();
        let effective_override = resolve_model_inheritance(
            req.model_override.as_ref(),
            agent_config.model.as_ref(),
            parent_model.as_ref(),
            &mut available_models,
            model_scope.as_ref(),
        )
        .map_err(|violation| SubagentError::ModelOutOfScope(violation.message))?;
        // SUBA-088 / pi `const currentProvider = parentModel?.provider` (`subagent-executor.ts:3648`
        // @v0.64.0), from the same remembered parent model the inheritance above used.
        let preferred_provider = parent_model.as_ref().and_then(provider_of);

        Ok(ResolvedRunAgent {
            agent,
            agent_config,
            resolved_context,
            fork_context,
            turn_budget,
            permission_rules,
            model_scope,
            max_thinking,
            available_models,
            effective_override,
            preferred_provider,
        })
    }

    /// Resolve everything this run is addressed BY rather than configured by: its wall-clock
    /// deadline, its [`RunId`], the resolved control config and the ordered notice pump that
    /// reads it, the T6 artifact quadruple's config + root, the resolved single-run output
    /// path/mode, and the child's session directory.
    ///
    /// Resolved as one phase because they are mutually dependent in exactly this order: the
    /// artifacts dir is the base a relative `output` resolves against, and the run id scopes
    /// both it and the session root.
    fn resolve_run_channels(
        &self,
        cfg: &SubagentExtensionConfig,
        overrides: &SingleRunOverrides,
        cwd: &Path,
        agent: &AgentDefinition,
        timeout_ms: Option<u64>,
    ) -> RunChannels {
        // R-SA-035 / pi `resolveAttemptTimeout` (`execution.ts:173-181`): the orchestrator computes
        // the wall-clock `deadline_at` ONCE, here, from the nominal `timeout_ms` budget (pi
        // `deadlineAt ?? now + timeoutMs`), and threads BOTH down — `deadline_at` is what `run_sync`
        // races the child against; `timeout_ms` is what the timed-out message renders.
        let deadline_at =
            timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

        // The run id is minted BEFORE `run_options` so it can also identify the clarify/ask
        // dispatch context (R-SA-037/119/120) that
        // [`SubagentExecutor::build_foreground_run_options`] builds; it doubles as the
        // artifact-quadruple run id [`write_foreground_input_artifact`] names the files after.
        let run_id = RunId::new();

        // pi's shared `execute` entry resolves `const controlConfig =
        // resolveControlConfig(deps.config.control, effectiveParams.control)` ONCE
        // (`subagent-executor.ts:5030` @v0.43.0; the merge itself is
        // `subagent-control.ts:37-71`) and threads it onto `ExecutionContextData.controlConfig`,
        // which `runSinglePath` then reads: the extension-level
        // `subagents.control` block is the base, this call's own `control` object overrides it field
        // by field. Resolved ONCE here because BOTH `RunOptions::control_config` (the thresholds the
        // child's stream is judged against) and the notifier's `notifyChannels` gate read it — pi
        // likewise resolves it once into `ExecutionContextData.controlConfig` and hands the same
        // value to `runSingleAttempt` and `createForegroundControlNotifier`.
        let resolved_control = crate::exec::control::resolve_control_config(
            cfg.control.as_ref(),
            overrides.control.as_ref(),
        );
        // pi `createForegroundControlNotifier(data, deps)` (`:1222` @v0.34.0), plus the ordered pump its
        // Rust equivalent needs; see the method doc.
        let control_notifier = self.foreground_control_notifier(
            run_id.clone(),
            agent.name.clone(),
            resolved_control.clone(),
        );

        // T6 artifact quadruple config + root (pi `subagent-executor.ts:3387-3391`). Resolved HERE,
        // ahead of `run_options`, because pi derives the single-run output base directory from the
        // artifacts dir (`resolveSingleRunOutputBaseDir`, `:2203-2207`). SUBA-041: an explicit
        // `artifacts: false` turns the quadruple off — pi's `enabled: params.artifacts !== false`.
        let art_cfg = crate::artifacts::ArtifactConfig {
            enabled: overrides.artifacts != Some(false),
            ..crate::artifacts::ArtifactConfig::foreground()
        };
        // SUBA-048 — pi `artifactsDir: getArtifactsDir(parentSessionFile, effectiveCwd,
        // artifactConfig.dir)` (`subagent-executor.ts:1431`/`:1471`/`:5037` @v0.43.0). This site
        // used to call `temp_artifacts_dir(cwd)` directly, which pinned every foreground run to the
        // temp root and made all three `artifactDir` preferences — including upstream's `project`
        // DEFAULT — unreachable.
        let art_dir = crate::artifacts::resolve_artifacts_dir(
            self.host_services()
                .and_then(|s| s.session_file())
                .as_deref(),
            Some(cwd),
            cwd,
            cfg.artifact_dir_preference(),
        );

        // SUBA-041 / pi `resolveSingleRunOutputBaseDir` (`subagent-executor.ts:2838-2842`): the
        // configured `singleRunOutputBaseDir` (tilde-expanded, `path.resolve`d) wins, else
        // `<artifactsDir>/outputs/<runId>`. This is the base a RELATIVE `output` resolves against —
        // deliberately NOT the run cwd, so a bare `report.md` never lands in the user's repo.
        let output_base_dir = resolve_single_run_output_base_dir(cfg, &art_dir, &run_id);
        // pi `runSinglePath` (`subagent-executor.ts:3562-3564,3666`): the persona's own `output:` is
        // the fallback for an omitted param and the referent of `output: true`; `outputMode` defaults
        // to `inline` from the PARAM alone (pi never consults the persona's own mode here).
        let output_path = resolve_single_output_path(
            normalize_single_output_override(
                overrides.output.as_ref(),
                agent
                    .output
                    .as_ref()
                    .and_then(|spec| spec.path.as_deref())
                    .and_then(Path::to_str),
            )
            .as_deref(),
            &output_base_dir,
        );
        let output_mode = parse_tool_output_mode(overrides.output_mode.as_deref())
            .unwrap_or(crate::discovery::types::OutputMode::Inline);

        // SUBA-041 / pi `subagent-executor.ts:5044-5052`: an explicit `sessionDir` is tilde-expanded
        // and `path.resolve`d and becomes the session ROOT verbatim; a configured
        // `default_session_dir` is instead scoped per run (`path.join(base, runId)`); the child's own
        // directory is then `<root>/run-0` (pi's `sessionDirForIndex(0)`).
        //
        // **[CYRUP-DELTA]** pi's third rung — `deps.getSubagentSessionRoot(parentSessionFile)`, an
        // always-present default derived from the PARENT session file — has no analog at this seam
        // (no parent-session-file plumbing reaches the extension), so with neither an explicit
        // `sessionDir` nor a configured default this stays `None` and
        // [`crate::exec::build_attempt_spawn_plan`] falls to pi's own `--no-session` branch
        // (`runs/shared/pi-args.ts:105-106`). The isolation outcome is the same one pi's scoped root buys: the
        // child never writes into the orchestrator's session store.
        let session_dir =
            resolve_single_run_session_root(cfg, overrides.session_dir.as_deref(), &run_id)
                .map(|root| root.join("run-0"));

        RunChannels {
            deadline_at,
            run_id,
            resolved_control,
            control_notifier,
            art_cfg,
            art_dir,
            output_path,
            output_mode,
            session_dir,
        }
    }

    /// Fold this run's whole resolved setup into the one [`RunOptions`]
    /// [`crate::exec::run_sync`] is driven from.
    ///
    /// Every field is threaded in from [`SubagentExecutor::run_foreground_impl`]'s prologue —
    /// nothing is re-derived here — so the value `run_sync` races the child against is
    /// literally the value the fail-closed gates above it approved.
    fn build_foreground_run_options(&self, input: ForegroundRunOptionsInput<'_>) -> RunOptions {
        let ForegroundRunOptionsInput {
            overrides,
            cwd,
            timeout_ms,
            spawn_command,
            cancel,
            agent,
            turn_budget,
            permission_rules,
            model_scope,
            max_thinking,
            available_models,
            effective_override,
            preferred_provider,
            fork_context,
            deadline_at,
            run_id,
            resolved_control,
            control_notifier,
            output_path,
            output_mode,
            session_dir,
            artifacts_enabled,
            include_transcript,
            art_dir,
            workflow_steer,
        } = input;
        RunOptions {
            spawn_command,
            // WORKFLOW_2 — threaded from the caller's [`SingleRunOverrides::child_env`] instead of
            // being hardcoded empty. Every non-workflow caller still passes the `Default` (an
            // empty map), so this is behaviour-preserving for them; `WorkflowRunHost` uses it to
            // set `WORKFLOW_CHILD_ENV` on the child's `Command`, which is how a workflow child's
            // own `subagent` tool learns to refuse a nested `workflowScript`. `RunOptions`
            // layers this FIRST, so the crate's identity/depth/child-role entries still win.
            child_env: overrides.child_env,
            // pi `hostAvailableBuiltins` (`subagent-executor.ts:3895`): the LIVE host tool registry,
            // observed HERE — the one place on this path that holds the `HostServices` handle — and
            // carried on `RunOptions` to `resolve_tool_surface`, which never re-reads it. `None` (a
            // headless embedder with no host bound) is UNKNOWN and skips the intersection.
            host_available_builtins: crate::exec::tool_surface::host_builtin_tool_names(
                self.host_services().as_deref(),
            ),
            // SUBA-021 — pi `config.usageBudget` (`subagent-runner.ts:172`), the caller's single
            // rung. The terminal check lives at `run_sync`'s settle (`exec/mod.rs`).
            usage_budget: overrides.usage_budget,
            // SCOPE_3j — the executor-owned cached-exclusion registry, cloned per run so every
            // foreground ladder filters against and records into the one store this session has.
            model_exclusions: Some(self.model_exclusions()),
            // SUBA-008 — the three-rung chain [`SubagentExecutor::resolve_run_agent`] resolved
            // (caller > frontmatter > config).
            turn_budget,
            // SUBA-073 — the resolved two-rung merge above (agent frontmatter + config, agent
            // winning on conflict).
            permission_rules,
            // pi sets `enforceHardTurnLimit` only from the slash delegation adapter
            // (`slash/delegation-adapters.ts:298`); the tool surface never does, so the
            // mid-tool-work deferral stays armed here exactly as upstream leaves it.
            enforce_hard_turn_limit: false,
            cwd: cwd.to_path_buf(),
            deadline_at,
            timeout_ms,
            output_path,
            output_mode,
            // pi's FOREGROUND structured-output policy (`subagent-executor.ts:3780-3787`): the
            // capture lives under the swept scratch dir and the cleanup guard stays armed, so
            // `SingleResult::structured_output_path` is `None` and the value travels inline. The
            // async policy (`Some(dir)`, `subagent-runner.ts:783-785`) belongs to the step
            // executor, not this surface.
            structured_output_dir: None,
            // SUBA-054 / pi `const reads = readsOverride !== undefined ? readsOverride :
            // agentConfig.defaultReads ?? false` (`subagent-executor.ts:3869` @v0.47.1). cyrup's
            // SINGLE surface advertises no top-level `reads` — and neither does upstream's
            // (`extension/schemas.ts`'s `SubagentParamProperties` has no `reads` key; the three
            // `reads` entries at `:144`, `:174` and `:204` are all per-ITEM), so the persona's own
            // `defaultReads` IS the whole precedence chain here. Before this it never left
            // frontmatter: the bundled `reviewer` shipped `defaultReads: plan.md, progress.md` and
            // was never told to read either file.
            reads: agent.default_reads.clone(),
            // SUBA-043 / pi `runSinglePath` (`subagent-executor.ts:3651,3671` @v0.43.0): the
            // top-level `outputSchema` param reaches the SINGLE run here. Pinned `None` until now,
            // which is what made SUBA-S01's capture machinery unreachable from the single surface.
            structured_output_schema: overrides.output_schema.clone(),
            model_override: effective_override,
            // SUBA-003: the same policy that just gated the explicit override, carried into
            // `run_sync` so the fallback ladder's own out-of-scope entries warn (pi
            // `execution.ts:1069`).
            model_scope,
            // SUBA-078: the DISCOVERED ceiling. `run_sync` intersects it with whatever this
            // process itself inherited via the env, so the bound can only tighten as the tree
            // deepens.
            thinking_ceiling: max_thinking,
            // SUBA-088: the remembered parent session's provider (pi `preferredModelProvider:
            // currentProvider`, `subagent-executor.ts:3825` @v0.64.0). `run_sync` consults it only
            // when the agent carries no `model_provider` of its own.
            preferred_provider,
            available_models,
            // pi `execute(id, params, signal, ...)` threads the host's own `AbortSignal` into the
            // executor for every mode (`extension/index.ts:498-500` ->
            // `executeSubagentCollapsed:378-381`), so aborting the tool call drives the running
            // child through real SIGINT->SIGTERM->SIGKILL escalation instead of a token that can
            // never fire.
            cancel,
            interrupt: CancelToken::new(),
            // SUBA-041: pi's `shareEnabled` (`subagent-executor.ts:4945`) and `sessionDir`
            // (`:5044-5052`), both consumed by `build_attempt_spawn_plan`'s session branch.
            share: overrides.share,
            session_dir,
            // SUBA-041: the per-call `skill` override (pi `normalizeSkillInput(params.skill)`,
            // `subagent-executor.ts:2788`) replaces the agent's own `skills` list; `None` keeps the
            // pre-existing fallthrough (`run_sync` reads `opts.skills ?? agent.skills`). The
            // foreground single-run path still resolves against `cwd` alone (no distinct
            // orchestrator/runtime fallback cwd).
            skills: overrides.skills,
            runtime_cwd: None,
            // pi `progress: params.includeProgress ? allProgress : undefined`
            // (`subagent-executor.ts:3819` @v0.43.0). `run_sync` assembles the snapshot; this is
            // the only place the caller's flag reaches it on the foreground path.
            include_progress: overrides.include_progress,
            agent_scope: None,
            // SUBA-041: the per-call `acceptance` policy (pi `acceptance: params.acceptance`,
            // `subagent-executor.ts:2962`); `None` (an omitted param, or the explicit `"auto"`)
            // defers to `AcceptanceContract::heuristic_default` inside `run_sync` (R-SA-023).
            acceptance: overrides.acceptance,
            fork_context,
            live_events: None,
            // R-SA-P1: the EXPLICIT anchor — this root orchestrator session's own id, captured at
            // SessionStart via P-2. `None` when no live session id is available (headless / SDK
            // embedder), at which point the child spawn falls through to the inherited env value.
            parent_session_id: self.root_parent_session(),
            // R-SA-037/119/120: hand the executor's single-slot ask lock (backed by the intercom
            // companion's real broker `ClarifyChannel` when `with_channels` wired one, else the
            // no-live-channel degrade default) to the drive loop, so a child's blocking
            // `contact_supervisor` ask fires `spawn_clarify` and marks the attempt detached.
            clarify: Some(crate::tui::intercom::ClarifyDispatch {
                lock: self.clarify_lock(),
                session_key: self
                    .root_parent_session()
                    .unwrap_or_else(|| EXTENSION_ID.to_string()),
                run_id: run_id.clone(),
                step_index: None,
            }),
            // Intercom child-bridge activation (pi `runs/shared/pi-args.ts:201-214` via
            // `data.intercomBridge.orchestratorTarget`): thread THIS orchestrator's own presence
            // target + this run's id + child index 0 so the spawned child registers
            // `contact_supervisor` (addressed here) + a broker presence under
            // `resolve_subagent_intercom_target(run_id, agent, 0)`. `None` target (headless / no live
            // intercom session) leaves the child un-bridged — the clean no-intercom path.
            orchestrator_intercom_target: self.orchestrator_intercom_target(),
            run_id: Some(run_id.clone()),
            child_index: Some(0),
            // G90 held while a foreground run had no async run directory at all, and it still holds
            // for a foreground SINGLE run: `control_steer` refuses one outright
            // (`STEER_FOREGROUND_RUN_REFUSAL`, `foreground_actions/steer.rs`) for exactly that
            // reason, and `workflow_steer` is `None` on every such call. WORKFLOW_14 narrowed it —
            // a foreground WORKFLOW child's control root is the workflow's own run directory
            // (WORKFLOW_13), so all three paths are real for it. `None` here now means "not a
            // workflow child", not "impossible".
            //
            // ⚠ AND IT STAYS `None` FOR ONE, deliberately. An earlier plan for this change gave a
            // plain foreground run its own scratch-rooted control tree
            // (`<temp>/scratch/<cwd_key>/fg-<run_id>/`) so the three paths could be unconditional.
            // That tree would have no writer: the only two routes to a foreground child's steer
            // handle both REQUIRE a workflow — `resolve_workflow_foreground_steering_target` gates
            // on `parent_workflow_run_id` and `runs.steer` is a `workflowScript` verb — and
            // `action: "steer"` refuses a plain foreground run outright by design
            // (`STEER_FOREGROUND_RUN_REFUSAL`, upstream-faithful at `subagent-executor.ts:3217`).
            // So it would have cost three `create_dir_all`s and a teardown per foreground run to
            // create directories nothing could ever address. Widening the refusal is what makes
            // that tree worth building; until then, this `None` is the honest answer.
            //
            // ALL THREE OR NONE — derived from ONE handle so they cannot be populated apart. The
            // ack dir without the inbox is a return path for a request that can never arrive; the
            // capability file without the ack dir tells a parent the child CAN be steered and then
            // never answers. `exec/mod.rs` creates each one pre-spawn from these very fields.
            // CARRIED, not recomputed: this is the same `PathBuf` the parent will write a steer
            // into (`ForegroundChildSteerHandle::deliver`), so parent-writes-where-child-reads is
            // true by construction. Re-deriving it here would make it true only while two
            // derivations agree — see the handle's own doc for what that cost last time.
            steer_inbox_dir: workflow_steer.map(|h| h.inbox_dir.clone()),
            // SUBA-049: the return half. Derived, because the READ side
            // (`control::take_steer_acks`) is a run-dir-scoped scan by design and shares that
            // derivation verbatim with the async path.
            steer_ack_dir: workflow_steer
                .map(|h| crate::background::control::steer_acks_dir(&h.run_dir, h.index)),
            steer_capability_path: workflow_steer
                .map(|h| crate::background::control::steer_capability_path(&h.run_dir, h.index)),
            // pi's shared `execute` entry's `controlConfig = resolveControlConfig(deps.config.control,
            // effectiveParams.control)` (`subagent-executor.ts:3385` @v0.34.0), read by
            // `runSinglePath` off `ExecutionContextData.controlConfig`: the extension-level
            // `subagents.control` block is the base, the call's own `control` object overrides it
            // field by field. Resolved once in [`SubagentExecutor::resolve_run_channels`] so the
            // notifier's channel gate reads the SAME value, exactly as pi shares one
            // `ExecutionContextData.controlConfig`.
            control_config: Some(resolved_control.clone()),
            // pi `onControlEvent: createForegroundControlNotifier(data, deps)` (`:1222-1229`).
            on_control_event: Some(control_notifier.sink()),
            // G80 — pi `artifactsDir: options.artifactsDir` reaching `evaluateAcceptance`
            // (`runs/foreground/execution.ts:1704`), which is `artifactsEnabled ?
            // getArtifactsDir(...) : undefined` (`api/preflight.ts:288`). Same `art_dir` the
            // artifact quadruple in [`SubagentExecutor::run_foreground_impl`] writes into, and
            // gated by the SAME `art_cfg.enabled` the caller passed as `artifacts_enabled`, so
            // SUBA-041's `artifacts: false` turns verify memoization off with everything else.
            artifacts_dir: artifacts_enabled.then(|| art_dir.to_path_buf()),
            // pi `execution.ts:1831-1840`: the live transcript writer exists iff `artifactsDir &&
            // artifactConfig?.enabled !== false && includeTranscript !== false`, as `foreground`.
            // The first two terms are `artifacts_dir` above; this is the third.
            transcript: (artifacts_enabled && include_transcript)
                .then_some(crate::exec::child_transcript::TranscriptSource::Foreground),
        }
    }

    /// Publish this run's live control surface — the `foregroundControls` entry and the notice
    /// machine's matching [`crate::tui::notices::LiveRunView`] — BEFORE the child is driven.
    ///
    /// pi `state.foregroundControls.set(runId, {interrupt, currentAgent, currentIndex})`
    /// (`shared/types.ts` + `runs/foreground/execution.ts`): register this run's live control
    /// surface BEFORE driving it, so a nested-control inbox listener polling in the SAME process
    /// (a fanout child's own `foreground_controls`, `fanout-child.ts:53-128`) can resolve an
    /// interrupt/resume request targeting this run's id while it is in flight. Shares the SAME
    /// token `run_options.interrupt` races the running child's attempt loop against, so firing it
    /// here genuinely soft-interrupts the live run rather than a disconnected flag.
    async fn register_foreground_controls(
        &self,
        run_id: &RunId,
        run_options: &RunOptions,
        agent: &AgentDefinition,
        task: &str,
        identity: ForegroundControlIdentity<'_>,
        detach_gate: &DetachGate,
    ) {
        {
            let now = crate::time::now_epoch_millis();
            let description = Some(task.to_string()).filter(|t| !t.trim().is_empty());
            let mut entry = ForegroundControlEntry {
                interrupt: run_options.interrupt.clone(),
                // VL-S11b — the live gate's handle, stamped at registration because the driver
                // already owns the gate by the time it calls this. This is upstream's
                // `onDetachReady` publication (`execution.ts:1372`), lifted onto the control by
                // `syncCurrentChild` (`foreground-control.ts:59`); cyrup's driver has no
                // separate "coordinator became live" moment, so the two collapse into one.
                //
                // Every OTHER construction site of this entry still carries `None` — upstream's
                // own `if (input.detach)` guard (`foreground-control.ts:119`) — and
                // `/subagents-detach` renders those as "not currently detachable" rather than
                // panicking; see `slash_detach.rs`'s `None`-handle arm for which of upstream's
                // four refusals that is and why.
                detach: Some(detach_gate.handle()),
                // Derived below by `begin_foreground_child`, from the child this call registers at
                // flat index 0 — never hand-written here (WORKFLOW_6 §1.3).
                current_agent: None,
                current_index: None,
                current_activity_state: None,
                // This entry point is pi's `runSinglePath`; its run shape is `single`.
                mode: crate::background::RunMode::Single,
                description: None,
                current_tool: None,
                current_path: None,
                turn_count: None,
                tool_count: None,
                tokens: None,
                started_at: now,
                updated_at: now,
                // WORKFLOW_6 SUBTASK1. `current_session_id()` is cyrup's `state.currentSessionId`
                // (`session_state.rs:55`), read off the live P-1 backend, and `parse_opt` is the
                // ONLY way a `SessionId` is constructed from an untyped read.
                session_id: crate::identity::SessionId::parse_opt(
                    self.current_session_id().as_deref(),
                ),
                parent_workflow_run_id: identity.parent_workflow_run_id.cloned(),
                workflow_key: identity.workflow_key.cloned(),
                cwd: Some(identity.cwd.to_path_buf()),
                session_name: None,
                active_children: std::collections::BTreeMap::new(),
            };
            // pi registers the run's control with `activeChildren: new Map()` and then calls
            // `beginForegroundChild` per child (`subagent-executor.ts:7028` + `foreground-control.ts`).
            // A cyrup foreground SINGLE run has exactly one child, at flat index 0.
            begin_foreground_child(
                &mut entry,
                ForegroundChildEntry {
                    index: 0,
                    agent: agent.name.clone(),
                    session_name: None,
                    // pi `description: task` (`runs/foreground/execution.ts`'s control registration)
                    // — the caller's own task text, which is what the roster row identifies the run
                    // by (`fleet.ts:723`) and the detail pane prints as `Task` (`fleet.ts:434-437`).
                    description,
                    started_at: now,
                    updated_at: now,
                    current_activity_state: None,
                    current_tool: None,
                    current_path: None,
                    turn_count: None,
                    tool_count: None,
                    tokens: None,
                    interrupt: run_options.interrupt.clone(),
                    // WORKFLOW_14 — `Some` exactly for a workflow child, cloned from the SAME
                    // handle `build_foreground_run_options` derived this child's three spawn paths
                    // from. This is what turns `steer_workflow_foreground`'s delivery arm from
                    // unreachable into the live path: the parent resolves this entry out of
                    // `foreground_controls`, reads the handle, and writes into the very
                    // `steer-targets/<index>/` the child was spawned watching.
                    steer: identity.workflow_steer.cloned(),
                },
            );
            // Byte-identical to the pre-WORKFLOW_6 hand-written values: after `begin_foreground_child`,
            // `entry.current_agent == Some(agent.name)`, `entry.current_index == Some(0)` and
            // `entry.description == Some(task)` whenever `task` is non-blank — nothing renders
            // differently, and `entry.active_children` is now real instead of absent.
            self.foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(run_id.as_str().to_string(), entry);
        }

        // The herdr status bridge's foreground refcount. ONE call per RUN, never per child: a
        // parallel run with nine children is ONE run in flight, which is the whole point of the
        // refcount (`crate::herdr::state`'s module doc). Paired with the
        // `foreground_run_finished` in `settle_foreground_run` below, which is the ONE place a
        // run's control surface is torn down — so the count cannot leak on a failed, interrupted
        // or stopped run.
        if let Some(bridge) = crate::herdr::bridge() {
            bridge.foreground_run_started();
        }

        // The notice machine's own live-state projection (R-SA-116 check 1: an unknown run is not
        // actionable). Registered alongside the `foregroundControls` entry above and dropped
        // alongside it by [`SubagentExecutor::settle_foreground_run`], so the two views of "is this
        // run still live" cannot disagree — pi reads BOTH off the single `state.foregroundControls`
        // map, which is what `isForegroundNoticeStillActionable` consults
        // (`control-notices.ts:59-65` @v0.34.0).
        self.notice_state().lock().await.observe_run(
            run_id.clone(),
            crate::tui::notices::LiveRunView {
                current_agent: Some(agent.name.clone()),
                current_step_index: Some(0),
                needs_attention: false,
            },
        );
    }

    /// Tear this run's live control surface back down once it has settled, in the ONE order
    /// that cannot be raced: drain the notice pump, then drop the `foregroundControls` entry,
    /// then drop the notice machine's projection of it.
    ///
    /// SUBA-N05: drain the ordered control-notice pump FIRST. Every event raised by the run that
    /// just settled is now guaranteed to have been applied to the notice machine's live
    /// projection, so the teardown below cannot be raced by a late event re-registering a
    /// finished run (see `foreground_control_notifier`'s "Ordering" section). Bounded — this can
    /// stall the tool call by at most `FOREGROUND_CONTROL_FLUSH_TIMEOUT`, and only if the pump
    /// has genuinely wedged.
    async fn settle_foreground_run(
        &self,
        run_id: &RunId,
        control_notifier: &ForegroundControlNotifier,
    ) {
        control_notifier.flush().await;

        // The run has settled (success, failure, or interrupted-terminal) — pi's foregroundControls
        // entry only exists while a run is live, so a nested-control request arriving after this
        // point must see a lookup miss ("is not active in this fanout child"), never a stale entry.
        {
            let mut controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = controls.remove(run_id.as_str()) {
                // WORKFLOW_6 §1.1 — the identity this entry carried for its lifetime, read HERE
                // (not only stamped at registration) so `workflow_key`/`parent_workflow_run_id`
                // have a genuine production reader ahead of WORKFLOW_7/WORKFLOW_11's own gates on
                // them. Purely observational: the entry was already being dropped on the floor.
                tracing::debug!(
                    run_id = %run_id,
                    session_id = ?entry.session_id.as_ref().map(crate::identity::SessionId::as_str),
                    parent_workflow_run_id = ?entry.parent_workflow_run_id.as_ref().map(RunId::as_str),
                    workflow_key = ?entry.workflow_key.as_ref().map(crate::workflows::WorkflowKey::as_str),
                    "foreground control settled"
                );
            }
        }

        // The herdr status bridge's matching edges, at the ONE teardown point. `clear_attention`
        // is unconditional and idempotent: a run that settles while raised must not leave the
        // pane amber, and a run that was never raised is a no-op.
        if let Some(bridge) = crate::herdr::bridge() {
            bridge.clear_attention(run_id);
            bridge.foreground_run_finished();
        }

        // ...and the notice machine's projection of the same fact (pi's single
        // `state.foregroundControls` map serves both roles), together with the pending-timer abort
        // pi pairs it with (`clearPendingForegroundControlNotices(deps.state, runId)` immediately
        // ahead of `foregroundControls.delete(runId)`, `subagent-executor.ts:3579-3581`). A
        // foreground notice still sitting in its debounce window when the run settles is therefore
        // cancelled outright, and would in any case fail check 1 of the actionability re-check —
        // pi's `if (!control) return false` (`control-notices.ts:60` @v0.34.0).
        self.notice_state().lock().await.forget_run(run_id);
    }
}

/// Write the INPUT leg of this run's T6 artifact quadruple, and return the paths the remaining
/// three legs are written to once the run settles (by
/// [`crate::extension::executor::paths::write_foreground_output_artifacts`]).
///
/// T6 artifact quadruple (pi `runs/foreground/execution.ts:960-1074`): record this run's input
/// BEFORE spawning (so it survives a child crash), then its output/metadata/event-stream AFTER
/// the run settles. Written into the scoped-temp artifacts root for `cwd` (the Rust analog of
/// pi's `tempArtifactsDir = getArtifactsDir(null)`, `extension/index.ts:340`). Best-effort: a
/// failed artifact write never alters the `SingleResult` the caller observes. (`run_id`,
/// `art_cfg` and `art_dir` were all resolved by [`SubagentExecutor::resolve_run_channels`] —
/// `art_cfg.enabled` already honors SUBA-041's `artifacts: false`, and `art_dir` doubles as the
/// relative-output base root.)
/// SUBA-075, narrowed by SCOPE_19/A3: must this fork's branch be forced to `thinking: off` for a
/// reason the resolver itself cannot see?
///
/// One arm survives, and it is upstream's own external-runner short-circuit (pi
/// `prepareForkThinking`, `runs/foreground/subagent-executor.ts:5859-5862` @v0.57.0): an EXTERNAL
/// child is not a pi/cyrup process and resolves no model from this registry, so this crate cannot
/// speak for its continuation semantics and stays on upstream's conservative `?? true` side.
///
/// SCOPE_19/A3 [CYRUP-DELTA] — upstream's second arm walked the resolved model ladder and answered
/// `true` for ANY Anthropic candidate (`candidates.some(forkedChildRequiresThinkingOff)`,
/// `:5880-5883`). That walk is deleted: the resolver's unconditional stripping already makes the
/// branch safe for every provider, and the override decision moved into
/// [`crate::fork_context::ForkContextResolver::resolve`], which fires it on the one fact that
/// actually settles the question — whether the strip disturbed the branch's FINAL assistant entry
/// (`Sanitization::tail_disturbed`). The provider gate disabled reasoning on branches that were
/// already safe, and misaligning a fork child's effort from its parent's forfeits prompt-prefix
/// reuse on exactly the call shape that inherits the most context.
fn fork_requires_thinking_off(agent: &AgentDefinition) -> bool {
    matches!(
        agent.runner,
        Some(
            crate::runner::AgentRunnerConfig::ExternalCli(_)
                | crate::runner::AgentRunnerConfig::ExternalJob(_)
        )
    )
}

/// pi `receipt.detachedReason = "intercom coordination"` (`execution.ts:620`, produced at `:762`),
/// stamped HERE because this is the lowest layer that may name the closed vocabulary.
///
/// [`crate::exec::run_sync`] observes the detach — its ONLY detach observation is the drive loop's
/// R-SA-037 blocking `contact_supervisor` arm (`exec/drive_attempt.rs`), which is upstream's
/// intercom producer and nothing else — but `crate::exec` sits BELOW `crate::extension` and
/// `extension::executor` is a private module of it, so `exec` cannot write
/// [`DetachReason`]'s path. Rather than let a second string literal exist down there (the exact
/// drift the closed enum prevents), `run_sync` leaves the field `None` and this fills it in.
///
/// Idempotent on `detached_reason`: a result that already carries one came from
/// [`detach_receipt`], and a user detach must never be relabelled as an intercom one.
fn stamp_intercom_detach_reason(result: &mut SingleResult) {
    if result.detached && result.detached_reason.is_none() {
        result.detached_reason = Some(DetachReason::IntercomCoordination.as_str().to_string());
    }
}

/// A minted detach receipt — pi's `receipt` object (`execution.ts:615-638`), which upstream hands
/// to `onDetachReceipt` and cyrup hands back to the caller of `run_foreground_impl`.
///
/// Two values rather than one because cyrup's [`SingleResult`] carries no `outputSaveError` field
/// (`exec/run_result.rs` has no such member; `ForegroundHistoryChild::output_save_error`
/// (`foreground_history/record.rs`) is the only slot in this crate that holds one, and its own doc
/// records that it had no producer until now). The receipt therefore carries upstream's
/// `receipt.outputSaveError`
/// (`execution.ts:627-631`) alongside the result, and the foreground driver stamps it onto the
/// remembered child — which is the sole consumer upstream has too (`run-status.ts:230`'s
/// `Output warning:` line, cyrup `tui/fleet.rs:982`).
pub(crate) struct DetachReceipt {
    /// The receipt itself: what `/subagents-detach` returns to its caller while the child keeps
    /// running.
    pub(crate) result: SingleResult,
    /// pi `receipt.outputSaveError` (`execution.ts:627-631`) — `Some` **only** when the run had an
    /// output path configured, because that is the file the detach left unfinalized. A run with no
    /// `output:` has nothing unfinalized to warn about, so upstream stamps nothing and neither
    /// does this.
    pub(crate) output_save_error: Option<String>,
}

/// Mint the receipt a detach hands back — pi `detachForeground`'s snapshot
/// (`execution.ts:615-638`), for the `/subagents-detach` producer.
///
/// # Why this is a separate construction site from [`crate::exec::run_sync`]'s own result
///
/// Upstream can build its receipt by SNAPSHOTTING live progress (`snapshotResult(result,
/// receiptProgress)`, `:617`) because its child is an in-process session object whose accumulated
/// `result`/`progress` are already in hand on the same event loop as the detach call. cyrup's
/// child is a real OS process whose accumulation lives inside the still-running
/// `drive_foreground_run_sync` future, and taking a consistent snapshot out of it would mean
/// synchronizing on the very future the detach exists to leave alone. So the receipt is minted
/// fresh, from the two facts the driver genuinely owns at that instant — the agent and the task —
/// plus the closed [`crate::extension::executor::detach::DetachReason`] vocabulary.
///
/// `[CYRUP-DELTA]` the consequence is that a detach receipt reports zero usage, zero turns and no
/// tool calls, where upstream's reports the partial totals accumulated so far. Nothing downstream
/// reads them as authoritative: the run's terminal totals arrive on the continuation's
/// reconciliation of the same run id, which is the value `subagent({ action: "status" })` and
/// `bg_wait` both render once the child exits.
///
/// `acceptance` is `None` and not upstream's interrupted-acceptance ledger
/// (`execution.ts:2000-2006`): that branch is gated on `result.interrupted && detachedReason ===
/// "user request"`, and a detach receipt is not interrupted — it is a hand-off. The ledger lands on
/// the CONTINUATION's terminal result if the child is later interrupted, which is where upstream
/// builds it too.
pub(crate) fn detach_receipt(
    agent: &str,
    task: &str,
    reason: DetachReason,
    output_path_configured: bool,
) -> DetachReceipt {
    DetachReceipt {
        output_save_error: output_path_configured.then(|| reason.output_save_error().to_string()),
        result: SingleResult {
            agent: agent.to_string(),
            task: task.to_string(),
            // pi `receipt.exitCode = -2` (`:618`).
            exit_code: DETACHED_EXIT_CODE,
            // pi `receipt.detached = true` (`:619`), set here rather than by the caller because a
            // receipt that is not marked detached is indistinguishable from a failed run.
            detached: true,
            // pi `receipt.detachedReason = reason` (`:620`).
            detached_reason: Some(reason.as_str().to_string()),
            // pi `receipt.finalOutput` (`:621-625`), reason-keyed.
            final_output: Some(reason.final_output().to_string()),
            // The sentence above IS substantive text, so the state is `Present` by
            // `derive_output_state`'s own first branch — asserted rather than re-derived, because
            // the only input that could change the answer is a constant.
            output_state: crate::exec::output_state::SubagentOutputState::Present,
            // A detach is a hand-off, not a failure: `error` must stay `None` or
            // `foreground_history_child_status` would mint `"failed"` instead of `"detached"`
            // (`foreground_history/record.rs`), and `routing.rs`'s detached arm would never be
            // reached.
            error: None,
            usage: cyrup_core::Usage::default(),
            turns: 0,
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            model: None,
            attempted_models: Vec::new(),
            child_run_id: None,
            model_attempts: Vec::new(),
            structured_output: None,
            session_file: None,
            structured_output_path: None,
            artifact_paths: None,
            transcript_path: None,
            transcript_error: None,
            acceptance: None,
            interrupted: false,
            timed_out: false,
            timeout_recovery: None,
            context_overflow: false,
            stopped: false,
            process_signal: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
            // pi never truncates a detached run's output (`assemble_delivered_output`'s R-SA-037
            // skip); this text is upstream's own sentence and is under every cap regardless.
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
        },
    }
}

/// Hand the still-running child to a task that OWNS its driving future — the other half of the
/// producer split, and upstream's own post-detach lifetime (`execution.ts:646-648`: the session's
/// callbacks keep firing after the tool call returned).
///
/// # What this task holds, and what it deliberately does not
///
/// It holds the `Pin<Box<…>>` drive future (and with it the `SpawnedChild`), the run's artifact
/// paths, and a clone of the executor's own `foreground_runs` `Arc` — **not** the executor.
///
/// `[CYRUP-DELTA]` the spec called for a `Weak<SubagentExecutor>` here, for the stated reason that
/// a shutdown must not be pinned alive by a detached child. No such handle is obtainable:
/// `run_foreground_impl` takes `&self`, [`SubagentExecutor`] carries no self-`Arc` slot, and
/// switching the receiver to `self: &Arc<Self>` would change the public `&self` contract of
/// `run_foreground`/`run_foreground_streaming` at call sites outside this seam (`resolve.rs`,
/// `refinement.rs`, `workflow.rs`, `routing.rs`, `slash.rs` and three `cyrup-it` suites construct
/// bare `SubagentExecutor` values). Capturing the ONE `Arc` field the reconcile writes is strictly
/// stronger than a `Weak` for the stated purpose — it cannot keep the executor alive at all, only
/// a map bounded at
/// [`MAX_REMEMBERED_FOREGROUND_RUNS`](crate::extension::executor::foreground_history) entries.
///
/// `[CYRUP-DELTA]` the one thing it therefore cannot do is call
/// `persist_foreground_run_history_for`, which upstream reaches through
/// `persistRememberedForegroundRuns` at the end of `updateRememberedForegroundChild`
/// (`subagent-executor.ts:900`): that writer is `&self` on [`SubagentExecutor`]
/// (`foreground_history/persist.rs`) and its merge/eligibility helpers are private to that module.
/// The reconciled run reaches disk at the session's next foreground settle, which calls the same
/// writer over the same map. Nothing an in-session reader consults is affected —
/// `subagent({ action: "status" })`, `bg_wait` and `ExecutorForegroundProbe` all read the
/// in-memory map this task DOES update — only a cross-restart restore of a run that settled after
/// its session's last foreground run is delayed.
fn spawn_detached_foreground_continuation(
    foreground_runs: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<RunId, ForegroundHistoryRun>>,
    >,
    drive: std::pin::Pin<Box<dyn std::future::Future<Output = SingleResult> + Send>>,
    run_id: RunId,
    art_paths: crate::artifacts::ArtifactPaths,
    art_cfg: crate::artifacts::ArtifactConfig,
) {
    tokio::spawn(async move {
        // The future is AWAITED here, in a task that owns it — the child keeps running exactly as
        // it was, on the same `SpawnedChild`, with the same stdout reader and the same deadline.
        let result = drive.await;
        // pi `onDetachedExit`'s artifact leg (`subagent-executor.ts:4077-4090` →
        // `finalizeSingleWorktreeHandoff`/the artifact writes): the output/metadata/JSONL legs of
        // the quadruple are written now, against the paths minted before the run started, so a
        // detached run's artifacts land in the same place an attached one's would.
        write_foreground_output_artifacts(&art_paths, &art_cfg, run_id.as_str(), &result);
        reconcile_detached_foreground_child(&foreground_runs, &run_id, &result);
        tracing::debug!(
            run_id = %run_id,
            exit_code = result.exit_code,
            "detached foreground child settled and was reconciled"
        );
    });
}

/// pi `updateRememberedForegroundChild` (`subagent-executor.ts:850-899`) — the SECOND of
/// upstream's two moments, reduced to the fields
/// [`ForegroundHistoryChild`](crate::extension::executor::foreground_history) carries.
///
/// This is the reconciler for a detached `/run` single, and `workflow_detach/` is deliberately not
/// it: that module is a port of `workflow-detach-reconcile.ts` and is workflow-scoped (it needs a
/// `WorkflowKey`, a workflow `status.json` and `find_workflow_settlement_step`). A plain `/run`
/// single has no workflow status to settle, so upstream's own reconciler for it is this one.
///
/// # Why the status is recomputed rather than re-derived from `SingleResult::detached`
///
/// Upstream calls `resolveSubagentResultStatus({ …, detached: false })` (`:863`) — it FORCES the
/// detached term off, because the child has now exited and the `"detached"` status is exactly what
/// this write exists to clear. The three remaining arms mirror
/// `foreground_history_child_status`'s own (`foreground_history/record.rs`) with its first arm
/// removed; they are restated here rather than called because that function is private to the
/// history facade and its `detached` arm is the one thing that must NOT apply.
///
/// `output_save_error` is CLEARED for upstream's reason (`:880` assigns
/// `input.result.outputSaveError`, which `omitUndefinedProperties` then drops): the warning said
/// the output file was left unfinalized by the detach, and this write is the moment it stopped
/// being true.
fn reconcile_detached_foreground_child(
    foreground_runs: &std::sync::Mutex<std::collections::HashMap<RunId, ForegroundHistoryRun>>,
    run_id: &RunId,
    result: &SingleResult,
) {
    let now = crate::time::now_epoch_millis();
    let mut runs = foreground_runs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // pi `:853-857` — a run the map no longer holds (evicted, or the process moved on) is simply
    // not reconciled. Upstream's `run.children[index]` lookup degrades the same way.
    let Some(run) = runs.get_mut(run_id) else {
        return;
    };
    run.updated_at = now;
    // A foreground SINGLE run has exactly one child, at flat index 0 — pi's `input.index`, which
    // `subagent-executor.ts:4086` passes as `0` for this call.
    let Some(child) = run.children.first_mut() else {
        return;
    };
    child.status = if result.stopped {
        "stopped"
    } else if result.exit_code != 0 || result.error.is_some() {
        "failed"
    } else {
        "completed"
    }
    .to_string();
    child.agent = result.agent.clone();
    child.updated_at = Some(now);
    child.model = result.model.as_ref().map(|m| m.as_str().to_string());
    child.session_file = result.session_file.clone();
    child.transcript_path = result.transcript_path.clone();
    child.saved_output_path = result.saved_output_path.as_ref().map(PathBuf::from);
    child.artifact_output_path = result
        .artifact_paths
        .as_ref()
        .map(|paths| paths.output_path.clone());
    child.error = result.error.clone();
    child.transcript_error = result.transcript_error.clone();
    child.output_save_error = None;
    // pi `:887` — `finalOutput` is spread only when the terminal result HAS one, so a child that
    // exited silently keeps the detach receipt's sentence rather than blanking it.
    if result.final_output.is_some() {
        child.final_output = result.final_output.clone();
    }
    child.tokens = (result.usage.total_tokens > 0).then_some(result.usage.total_tokens);
    child.tool_count = {
        let calls = result.tool_calls.len() as u64;
        (calls > 0).then_some(calls)
    };
}

fn write_foreground_input_artifact(
    art_cfg: &crate::artifacts::ArtifactConfig,
    art_dir: &Path,
    run_id: &RunId,
    agent_name: &str,
    task: &str,
) -> crate::artifacts::ArtifactPaths {
    let art_paths = crate::artifacts::artifact_paths(art_dir, run_id.as_str(), agent_name, None);
    if art_cfg.enabled {
        let _ = crate::artifacts::ensure_artifacts_dir(art_dir);
        if art_cfg.include_input {
            let _ = crate::artifacts::write_artifact(
                &art_paths.input_path,
                &format!("# Task for {}\n\n{task}", agent_name),
            );
        }
    }
    art_paths
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
    use crate::extension::testsupport::seed_scope_fixture;

    // -----------------------------------------------------------------------------------------
    // SUBA-075 / SCOPE_19/A3 — the fork thinking gate. Only the external-runner arm survives
    // caller-side (pi `prepareForkThinking`'s `:5859-5862` @v0.57.0); the provider ladder walk is
    // deleted, and the branch-tail decision it stood in for lives — and is tested — in
    // `fork_context::ForkContextResolver::resolve` (`Sanitization::tail_disturbed`).
    // -----------------------------------------------------------------------------------------

    /// A model that resolves cleanly from the catalog; under SCOPE_19/A3 the gate no longer reads
    /// it, which `the_fork_gate_ignores_the_model_ladder` pins deliberately.
    const NON_ANTHROPIC: &str = "amazon-bedrock/amazon.nova-pro-v1:0";
    const ANTHROPIC: &str = "anthropic/claude-opus-4-6";

    /// A minimal persona carrying only the fields the gate reads: `runner` (plus the ladder fields
    /// the DELETED provider walk used to read, kept so the ignores-the-ladder pin below can
    /// construct the exact shapes that used to flip the old gate). Spelled out rather than
    /// borrowed from another subsystem's fixture so a change to THAT fixture's field choices can
    /// never quietly make these assertions pass for the wrong reason.
    fn gate_agent(model: Option<&str>, fallbacks: &[&str]) -> AgentDefinition {
        AgentDefinition {
            name: "worker".to_string(),
            local_name: "worker".to_string(),
            package_name: None,
            description: String::new(),
            aliases: Vec::new(),
            tools: None,
            extensions: None,
            extensions_from_default: false,
            subagent_only_extensions: Vec::new(),
            exclude_tools: None,
            allow_nested_subagents: None,
            model: model.map(ModelId::from),
            fallback_models: fallbacks.iter().map(|m| ModelId::from(*m)).collect(),
            thinking: None,
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            default_reads: None,
            default_progress: None,
            output: None,
            completion_guard: None,
            interactive: None,
            max_subagent_depth: None,
            default_context: None,
            default_async: None,
            default_timeout_ms: None,
            default_turn_budget: None,
            default_acceptance: None,
            acceptance_role: None,
            permission_rules: None,
            runner: None,
            memory: None,
            tool_budget: None,
            disabled: None,
            system_prompt_body: String::new(),
            source: crate::discovery::types::AgentSource::Project,
            file_path: PathBuf::from("/proj/.cyrup/agents/worker.md"),
            present_fields: std::collections::HashSet::new(),
            extra_fields: std::collections::BTreeMap::new(),
            override_info: None,
            model_source: None,
            model_provider: None,
        }
    }

    /// SCOPE_19/A3 — the gate no longer consults the model ladder AT ALL: the exact shapes that
    /// used to flip the old provider walk (an Anthropic primary, an Anthropic fallback, an empty
    /// ladder) all clear it now, because the branch-safety decision moved into the resolver's
    /// tail rule. If this pin ever fails, a provider/ladder check has crept back in — read the
    /// `[CYRUP-DELTA]` on `fork_requires_thinking_off` before "fixing" it.
    #[test]
    fn the_fork_gate_ignores_the_model_ladder() {
        for agent in [
            gate_agent(Some(NON_ANTHROPIC), &[]),
            gate_agent(Some(ANTHROPIC), &[]),
            gate_agent(Some(NON_ANTHROPIC), &[ANTHROPIC]),
            gate_agent(None, &[]),
            gate_agent(Some(crate::exec::fallback::INHERIT_MODEL_SENTINEL), &[]),
        ] {
            assert!(
                !fork_requires_thinking_off(&agent),
                "an in-crate runner never forces thinking off from the caller side — the \
                 tail-disturbance decision belongs to the resolver (model: {:?}, fallbacks: {:?})",
                agent.model,
                agent.fallback_models
            );
        }
    }

    /// pi `:5859-5862`: an external runner's child is not a cyrup process and resolves no model
    /// from this registry, so the crate cannot speak for its continuation semantics — the one
    /// caller-side arm that survives SCOPE_19/A3.
    #[test]
    fn an_external_runner_forces_thinking_off() {
        let mut agent = gate_agent(Some(NON_ANTHROPIC), &[]);
        assert!(
            !fork_requires_thinking_off(&agent),
            "precondition: an in-crate runner clears the gate"
        );
        agent.runner = Some(crate::runner::AgentRunnerConfig::ExternalCli(
            crate::runner::ExternalCliRunner {
                adapter: None,
                command: "claude".to_string(),
                args: Vec::new(),
                prompt_delivery_stdin: false,
                capabilities: None,
            },
        ));
        assert!(
            fork_requires_thinking_off(&agent),
            "the runner arm is the gate's whole remaining job"
        );
    }

    #[tokio::test]
    async fn run_foreground_errors_before_any_spawn_when_agent_is_unknown() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .run_foreground(
                dir.path(),
                "ghost",
                "do something",
                Some(ContextRequest::Fresh),
                None,
                None,
            )
            .await
            .expect_err("unresolvable agent must fail before any subprocess spawn");
        assert!(matches!(err, SubagentError::AgentNotFound(_)));
    }

    /// R-SA-055 (SAFETY-CRITICAL): `run_foreground`'s depth guard must run BEFORE agent discovery
    /// — proven by supplying a completely unresolvable agent name (`"ghost"`, exactly the same
    /// name [`run_foreground_errors_before_any_spawn_when_agent_is_unknown`] above uses to prove
    /// discovery's own independent failure mode) alongside a config whose `max_subagent_depth` is
    /// already exhausted. If the depth guard ran AFTER discovery (or not at all), this call would
    /// surface `AgentNotFound` — exactly like the sibling test above — since `"ghost"` never
    /// resolves either way; observing `DepthExceeded` instead is structural proof the guard
    /// short-circuited before `resolve_agent` (and therefore before any discovery filesystem scan)
    /// ever ran.
    #[tokio::test]
    async fn run_foreground_rejects_on_depth_before_agent_discovery_ever_runs() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.max_subagent_depth = 0; // current_depth (0, absent env) >= max_depth (0): blocked
        }
        let dir = tempfile::tempdir().expect("tempdir");
        // No `.cyrup/agents` directory is even created under `dir` — if discovery ran at all it
        // would find nothing and (for a real agent name) still fail with AgentNotFound; using the
        // exact same "ghost" name as the sibling discovery-failure test isolates this test's
        // assertion to purely WHICH error surfaces first.
        let err = executor
            .run_foreground(
                dir.path(),
                "ghost",
                "do something",
                Some(ContextRequest::Fresh),
                None,
                None,
            )
            .await
            .expect_err("a blocked depth ceiling must reject before agent discovery runs");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded (proving the guard ran BEFORE discovery could report its own \
             AgentNotFound for the same unresolvable name), got: {err:?}"
        );
    }

    /// SUBA-003, the load-bearing observable behavior: with `subagents.modelScope.enforce` armed,
    /// a run that EXPLICITLY asks for a model outside the `allow` list is REFUSED — the call
    /// returns `Err(SubagentError::ModelOutOfScope)` carrying pi's verbatim violation message, and
    /// no child process is ever spawned.
    ///
    /// Before this fix `modelScope` was not even a field on `SubagentSettings`, so serde dropped
    /// the whole block silently and this call ran the out-of-scope model to completion.
    #[tokio::test]
    async fn an_explicit_out_of_scope_model_refuses_the_run_with_pis_verbatim_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(
            dir.path(),
            "scoped",
            Some(
                r#"{"subagents":{"modelScope":{"enforce":true,"allow":["anthropic/*","together/*"]}}}"#,
            ),
        );

        let executor = SubagentExecutor::new();
        let err = executor
            .run_foreground(
                dir.path(),
                "scoped",
                "do something",
                Some(ContextRequest::Fresh),
                Some(ModelId::from("openai/gpt-5-nano")),
                None,
            )
            .await
            .expect_err("an out-of-scope explicit model must REFUSE the run, not run it");

        assert!(
            matches!(err, SubagentError::ModelOutOfScope(_)),
            "the refusal must be its own error kind, not folded into a generic failure: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Model 'openai/gpt-5-nano' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/*, together/*.",
            "the caller must see pi's verbatim violation text, naming the model AND the patterns"
        );
    }

    /// WORKFLOW_6 §1.3/§4.2 — `register_foreground_controls` DERIVES `current_agent`/
    /// `current_index`/`description` from the child it registers at flat index 0 (never
    /// hand-writes them), and stamps the [`ForegroundControlIdentity`] WORKFLOW_6 threads down
    /// from `WorkflowRunHost::launch` onto the entry — the exact invariant §5#2 names.
    #[tokio::test]
    async fn register_foreground_controls_derives_the_entry_and_stamps_workflow_identity() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        let agent = gate_agent(Some(NON_ANTHROPIC), &[]);
        let cwd = PathBuf::from("/proj");
        let run_options = crate::exec::testsupport::base_opts(&cwd, &[NON_ANTHROPIC]);
        let parent = RunId::new();
        let key = crate::workflows::WorkflowKey::parse("lane.a").expect("valid key");
        let steer = crate::extension::executor::foreground_control::ForegroundChildSteerHandle {
            inbox_dir: PathBuf::from("/runs/wf-1/control/steer-targets/3"),
            run_dir: PathBuf::from("/runs/wf-1"),
            index: 3,
        };

        executor
            .register_foreground_controls(
                &run_id,
                &run_options,
                &agent,
                "do the thing",
                ForegroundControlIdentity {
                    cwd: &cwd,
                    parent_workflow_run_id: Some(&parent),
                    workflow_key: Some(&key),
                    workflow_steer: Some(&steer),
                },
                // VL-S11b — the driver's gate; the entry publishes its handle.
                &crate::extension::executor::detach::DetachGate::new(),
            )
            .await;

        let controls = executor
            .foreground_controls()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = controls.get(run_id.as_str()).expect("registered");
        // Derived, not hand-written — byte-identical to the pre-WORKFLOW_6 inline values.
        assert_eq!(entry.current_agent.as_deref(), Some("worker"));
        assert_eq!(entry.current_index, Some(0));
        assert_eq!(entry.description.as_deref(), Some("do the thing"));
        assert_eq!(entry.active_children.len(), 1, "one child, at flat index 0");
        // Stamped from the identity WORKFLOW_6 threads through `ForegroundRunRequest` ->
        // `run_foreground_impl` -> `ForegroundControlIdentity`.
        assert_eq!(entry.parent_workflow_run_id, Some(parent.clone()));
        assert_eq!(
            entry
                .workflow_key
                .as_ref()
                .map(crate::workflows::WorkflowKey::as_str),
            Some("lane.a")
        );
        assert_eq!(entry.cwd, Some(cwd.clone()));
        // WORKFLOW_14 — the steer handle rides the SAME identity, and its index is the child's flat
        // index within the WORKFLOW, deliberately distinct from the `active_children` key (0).
        let child = entry
            .active_children
            .get(&0)
            .expect("child at flat index 0");
        let handle = child
            .steer
            .as_ref()
            .expect("a workflow child carries a steer handle");
        assert_eq!(handle.run_dir, PathBuf::from("/runs/wf-1"));
        assert_eq!(handle.index, 3);
        assert_eq!(
            handle.inbox_dir,
            PathBuf::from("/runs/wf-1/control/steer-targets/3"),
            "the handle must carry the child's OWN inbox — the runner's `steer-requests/` intake \
             queue is drained by a watch loop a foreground workflow does not have"
        );
    }

    /// A plain (non-workflow) registration carries `parent_workflow_run_id == None` — §5#3: a
    /// future steering gate keyed on this field must never mistake a bare foreground run for a
    /// workflow child.
    #[tokio::test]
    async fn register_foreground_controls_carries_no_workflow_identity_for_a_plain_run() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        let agent = gate_agent(Some(NON_ANTHROPIC), &[]);
        let cwd = PathBuf::from("/proj");
        let run_options = crate::exec::testsupport::base_opts(&cwd, &[NON_ANTHROPIC]);

        executor
            .register_foreground_controls(
                &run_id,
                &run_options,
                &agent,
                "do the thing",
                ForegroundControlIdentity {
                    cwd: &cwd,
                    parent_workflow_run_id: None,
                    workflow_key: None,
                    workflow_steer: None,
                },
                &crate::extension::executor::detach::DetachGate::new(),
            )
            .await;

        let controls = executor
            .foreground_controls()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = controls.get(run_id.as_str()).expect("registered");
        assert_eq!(entry.parent_workflow_run_id, None);
        assert_eq!(entry.workflow_key, None);
        // WORKFLOW_14's invariant, the other half: `steer.is_some() == parent_workflow_run_id
        // .is_some()`. A plain foreground run has no run directory, so it carries no handle and
        // `steer_workflow_foreground` refuses it with upstream's optional-`steer` sentence.
        let child = entry
            .active_children
            .get(&0)
            .expect("child at flat index 0");
        assert!(child.steer.is_none());
    }
}

#[cfg(test)]
mod detach_producer_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::RunMode;
    use crate::extension::executor::detach::{DetachGate, DetachReason};
    use crate::extension::executor::notices::ForegroundControlEntry;
    use crate::extension::testsupport::FixedSessionIdHost;
    use std::sync::Arc;

    fn executor_with_session(session: &str) -> SubagentExecutor {
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some(session.to_string()),
            file: None,
        }));
        executor
    }

    /// Remember the receipt the way `run_foreground_impl`'s detach branch does, and hand back the
    /// receipt itself.
    fn publish_detach(
        executor: &SubagentExecutor,
        run_id: &RunId,
        reason: DetachReason,
        output_path_configured: bool,
    ) -> DetachReceipt {
        let receipt = detach_receipt("scout", "hold the line", reason, output_path_configured);
        executor.remember_foreground_run(
            run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&receipt.result],
        );
        assert!(executor.stamp_detached_receipt(run_id, receipt.output_save_error.as_deref()));
        receipt
    }

    /// The receipt shape, field for field — pi `detachForeground` (`execution.ts:615-638`).
    ///
    /// **Gutting mutation this fails on:** return a plain `pre_spawn_failure`-shaped result (exit
    /// 1, `detached: false`, no reason) and call it a receipt. Every assert below fires, and the
    /// history status it mints becomes `"failed"` instead of `"detached"` — which is what makes a
    /// detached run addressable at all.
    #[test]
    fn the_detach_receipt_carries_upstreams_exact_shape() {
        let receipt = detach_receipt("scout", "hold the line", DetachReason::UserRequest, false);
        assert_eq!(
            receipt.result.exit_code,
            crate::extension::executor::detach::DETACHED_EXIT_CODE
        );
        assert_eq!(receipt.result.exit_code, -2, "pi `receipt.exitCode = -2`");
        assert!(receipt.result.detached);
        assert_eq!(
            receipt.result.detached_reason.as_deref(),
            Some("user request")
        );
        assert_eq!(
            receipt.result.final_output.as_deref(),
            Some("Detached at user request before task completion.")
        );
        // A detach is a HAND-OFF: an `error` here would mint `"failed"` instead of `"detached"`.
        assert!(receipt.result.error.is_none());
        assert!(!receipt.result.interrupted && !receipt.result.timed_out);

        // pi `:627` — `outputSaveError` is stamped ONLY when an output path was configured.
        assert_eq!(
            receipt.output_save_error, None,
            "no output path ⇒ nothing was left unfinalized ⇒ no warning"
        );
        let with_output = detach_receipt("scout", "hold the line", DetachReason::UserRequest, true);
        assert_eq!(
            with_output.output_save_error.as_deref(),
            Some("Output file was not finalized because the subagent detached at user request.")
        );

        // The intercom producer's own arm, from the same closed vocabulary.
        let intercom = detach_receipt(
            "scout",
            "hold the line",
            DetachReason::IntercomCoordination,
            true,
        );
        assert_eq!(
            intercom.result.detached_reason.as_deref(),
            Some("intercom coordination")
        );
        assert_eq!(
            intercom.result.final_output.as_deref(),
            Some("Detached for intercom coordination before task completion.")
        );
    }

    /// Publishing the receipt makes the run a `"detached"` entry in `foreground_runs` and stamps
    /// the output warning onto its child — the producer the history child's own `output_save_error`
    /// field doc says it had been waiting for.
    ///
    /// **Gutting mutation this fails on:** drop the `stamp_detached_receipt` call and the warning
    /// is `None`, so the fleet detail pane's `Output warning:` line never renders for a detach.
    #[test]
    fn publishing_a_receipt_remembers_a_detached_run_and_stamps_its_output_warning() {
        let executor = executor_with_session("session-a");
        let run_id = RunId::from_token("fgdetach0001".to_string());
        publish_detach(&executor, &run_id, DetachReason::UserRequest, true);

        let runs = executor
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let run = runs.get(&run_id).expect("the receipt was published");
        assert_eq!(run.children.len(), 1);
        assert_eq!(run.children[0].status, "detached");
        assert_eq!(
            run.children[0].output_save_error.as_deref(),
            Some("Output file was not finalized because the subagent detached at user request.")
        );
    }

    /// An unattributed run cannot be published, so the detach must be REFUSED rather than
    /// accepted — pi `onDetachReceipt` returning `false` (`execution.ts:643-645`), which leaves
    /// the run attached and abortable rather than orphaned.
    ///
    /// **Gutting mutation this fails on:** make `stamp_detached_receipt` return `true`
    /// unconditionally. The driver then accepts a detach for a run nothing can address — no
    /// `foreground_runs` entry means `status`, `bg_wait` and the probe all report it missing.
    #[test]
    fn a_run_with_no_session_identity_cannot_publish_a_receipt() {
        // No host services ⇒ no `current_session_id` ⇒ `remember_foreground_run` declines.
        let executor = SubagentExecutor::new();
        let run_id = RunId::from_token("fgdetach0002".to_string());
        let receipt = detach_receipt("scout", "hold the line", DetachReason::UserRequest, false);
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&receipt.result],
        );
        assert!(
            !executor.stamp_detached_receipt(&run_id, receipt.output_save_error.as_deref()),
            "an unpublishable receipt must report the refusal, not a silent success"
        );
    }

    /// The continuation's reconcile — pi `updateRememberedForegroundChild`
    /// (`subagent-executor.ts:850-899`) — flips the child OFF `"detached"` and clears the now-stale
    /// output warning.
    ///
    /// **Gutting mutation this fails on:** never spawn the continuation (or drop its reconcile
    /// call). The child stays `"detached"` forever, so `bg_wait({ id })` never returns and the
    /// fleet shows a finished child as still handed off.
    #[test]
    fn the_continuations_reconcile_takes_the_child_off_detached() {
        let executor = executor_with_session("session-a");
        let run_id = RunId::from_token("fgdetach0003".to_string());
        publish_detach(&executor, &run_id, DetachReason::UserRequest, true);

        // The child's real terminal result, built off the receipt shape so the test needs no
        // `AgentConfig`: a clean exit with no error, which pi's `resolveSubagentResultStatus`
        // with `detached: false` (`subagent-executor.ts:863`) resolves to `"completed"`.
        let mut settled =
            detach_receipt("scout", "hold the line", DetachReason::UserRequest, false).result;
        settled.detached = false;
        settled.detached_reason = None;
        settled.exit_code = 0;
        settled.error = None;
        settled.final_output = Some("the real answer".to_string());

        reconcile_detached_foreground_child(&executor.foreground_runs, &run_id, &settled);

        let runs = executor
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let run = runs.get(&run_id).expect("still remembered");
        assert_eq!(run.children[0].status, "completed");
        assert_eq!(
            run.children[0].final_output.as_deref(),
            Some("the real answer")
        );
        assert_eq!(
            run.children[0].output_save_error, None,
            "the file IS finalized now, so the detach's warning must not survive"
        );
    }

    /// A settle closes the gate, so a `/subagents-detach` arriving afterwards REFUSES instead of
    /// parking on a gate nobody will ever answer.
    ///
    /// The two statements exercised here are exactly the two `run_foreground_impl`'s settle arm
    /// runs, in that order.
    ///
    /// **Gutting mutation this fails on:** delete the `close` from the settle arm. `request`
    /// never resolves, the `timeout` below expires, and the assert fires — which is the
    /// user-visible hang this closes.
    #[tokio::test]
    async fn a_settled_run_closes_its_gate_so_a_later_request_refuses_instead_of_hanging() {
        let executor = executor_with_session("session-a");
        let run_id = RunId::from_token("fgdetach0004".to_string());
        let gate = DetachGate::new();
        executor.insert_foreground_control_for_test(
            run_id.as_str(),
            ForegroundControlEntry::for_test(RunMode::Single, 1, Some(gate.handle())),
        );
        let handle = executor
            .foreground_detach_control(run_id.as_str())
            .expect("the control is live")
            .1
            .expect("its detach handle is published");

        // The settle arm, verbatim.
        gate.close(DetachRefusal::SessionSettled);
        let notifier = executor.foreground_control_notifier(
            run_id.clone(),
            "scout".to_string(),
            crate::exec::control::ResolvedControlConfig::default(),
        );
        executor.settle_foreground_run(&run_id, &notifier).await;

        assert!(
            executor
                .foreground_detach_control(run_id.as_str())
                .is_none(),
            "the live control is dropped on settle"
        );
        let verdict = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle.request(DetachReason::UserRequest),
        )
        .await
        .expect("a request after settlement must not hang");
        assert_eq!(verdict, Err(DetachRefusal::SessionSettled));
    }
}
