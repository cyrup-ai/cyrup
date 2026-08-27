//! The foreground single-run path: a real, synchronous OS-subprocess run driven to completion.

use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, ModelId, ToolUpdateSink};

use crate::background::RunId;
use crate::discovery::types::{AgentDefinition, AgentReadScope, OutputMode};
use crate::error::SubagentError;
use crate::exec::{AgentConfig, RunOptions, SingleResult};
use crate::exec::fallback::resolve_model_inheritance;
use crate::fork_context::{resolve_effective_context, ContextMode, ForkContext};
use crate::registration::SubagentExtensionConfig;
use crate::spawn::depth::{resolve_effective_depth, DepthEnvelope};
use crate::extension::EXTENSION_ID;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::notices::{ForegroundControlEntry, ForegroundControlNotifier};
use crate::extension::executor::paths::{
    drive_foreground_run_sync, write_foreground_output_artifacts,
};
use crate::extension::executor::requests::{ForegroundRunRequest, SingleRunOverrides};
use crate::extension::tool::task_items::{
    normalize_single_output_override, parse_tool_output_mode, resolve_single_output_path,
    resolve_single_run_output_base_dir, resolve_single_run_session_root,
};

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
    /// SUBA-003's `subagents.modelScope` policy, carried on so the fallback ladder's own
    /// out-of-scope entries warn.
    model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    /// R-SA-038's availability set, already widened by the explicit override and any inherited
    /// parent-session model.
    available_models: Vec<ModelId>,
    /// The model override that survived the fail-closed `modelScope` gate.
    effective_override: crate::exec::fallback::ModelOverride,
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
    turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    available_models: Vec<ModelId>,
    effective_override: crate::exec::fallback::ModelOverride,
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
    /// Borrowed: the caller writes the artifact quadruple into the same root.
    art_dir: &'a Path,
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
        context: Option<ContextMode>,
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
            model_scope,
            available_models,
            effective_override,
        } = self.resolve_run_agent(&req, &cfg, depth).await?;
        let ForegroundRunRequest {
            overrides, cwd, task, timeout_ms, cancel, ..
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
            model_scope,
            available_models,
            effective_override,
            fork_context,
            deadline_at,
            run_id: &run_id,
            resolved_control: &resolved_control,
            control_notifier: &control_notifier,
            output_path,
            output_mode,
            session_dir,
            artifacts_enabled: art_cfg.enabled,
            art_dir: &art_dir,
        });

        self.register_foreground_controls(&run_id, &run_options, &agent, task).await;

        let art_paths =
            write_foreground_input_artifact(&art_cfg, &art_dir, &run_id, &agent.name, task);

        let result = drive_foreground_run_sync(
            &agent_config,
            task,
            run_options,
            &agent.name,
            resolved_context,
            on_update,
        )
        .await;

        self.settle_foreground_run(&run_id, &control_notifier).await;

        write_foreground_output_artifacts(&art_paths, &art_cfg, run_id.as_str(), &result);

        // R-SA-058: the per-attempt raw-stdout tee `run_sync` writes to
        // `<cwd>/.cyrup-subagent-scratch/attempt-<n>.jsonl` is this run's persisted, observable child
        // record and MUST survive the orchestrator, exactly as it does on every other spawn path in
        // this crate (the tool single/parallel/chain fan-outs and the background hop-2 runner all
        // leave it in place — it is the single observation channel the crate's integration tests read
        // back, e.g. `tool_parallel_chain_integration`'s `/run [model=…]` tee check and
        // `companions_wiring_proof`). This mirrors pi, which likewise never deletes its persisted
        // child NDJSON stream — pi only cleans the *transient* per-spawn prompt/task-overflow dir it
        // creates under `os.tmpdir()` (`pi-subagents/src/runs/shared/pi-args.ts:143-158` build it,
        // `:233-236` `cleanupTempDir` removes it, invoked from
        // `pi-subagents/src/runs/foreground/execution.ts:1109`), a dir that lives OUTSIDE the working
        // tree and never holds the event stream. An earlier revision erroneously `remove_dir_all`'d
        // the whole `.cyrup-subagent-scratch` dir here, which silently discarded that tee the moment a
        // foreground `/run` completed — defeating the tee's own stated purpose and diverging from
        // every sibling path — so no such deletion is performed.

        Ok((result, run_id))
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
        let (agent, model_scope) =
            self.resolve_agent_with_model_scope(req.cwd, req.agent_name, req.agent_scope)?;
        // Fork default-mode (Tier-2, pi `resolveAgentDefaultContextPolicy`): an OMITTED call-site
        // `context` (`None`) falls back to THIS agent's own `default_context` rather than being forced
        // to `Fresh`; an explicit call-site value still wins (`resolve_effective_context`).
        let effective_context = resolve_effective_context(req.context, agent.default_context);
        let fork_context = self.resolve_context(req.cwd, effective_context).await?;
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
        let effective_override = resolve_model_inheritance(
            req.model_override.as_ref(),
            agent_config.model.as_ref(),
            self.remembered_parent_model().as_ref(),
            &mut available_models,
            model_scope.as_ref(),
        )
        .map_err(|violation| SubagentError::ModelOutOfScope(violation.message))?;

        Ok(ResolvedRunAgent {
            agent,
            agent_config,
            resolved_context,
            fork_context,
            turn_budget,
            model_scope,
            available_models,
            effective_override,
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
        let control_notifier =
            self.foreground_control_notifier(run_id.clone(), agent.name.clone(), resolved_control.clone());

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
            self.host_services().and_then(|s| s.session_file()).as_deref(),
            Some(cwd),
            cwd,
            cfg.artifact_dir_preference(),
        );

        // SUBA-041 / pi `resolveSingleRunOutputBaseDir` (`subagent-executor.ts:2838-2842`): the
        // configured `singleRunOutputBaseDir` (tilde-expanded, `path.resolve`d) wins, else
        // `<artifactsDir>/outputs/<runId>`. This is the base a RELATIVE `output` resolves against —
        // deliberately NOT the run cwd, so a bare `report.md` never lands in the user's repo.
        let output_base_dir =
            resolve_single_run_output_base_dir(cfg, &art_dir, &run_id);
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
            cancel,
            agent,
            turn_budget,
            model_scope,
            available_models,
            effective_override,
            fork_context,
            deadline_at,
            run_id,
            resolved_control,
            control_notifier,
            output_path,
            output_mode,
            session_dir,
            artifacts_enabled,
            art_dir,
        } = input;
        RunOptions {
            // SUBA-021 — pi `config.usageBudget` (`subagent-runner.ts:172`), the caller's single
            // rung. The terminal check lives at `run_sync`'s settle (`exec/mod.rs`).
            usage_budget: overrides.usage_budget,
            // SUBA-008 — the three-rung chain [`SubagentExecutor::resolve_run_agent`] resolved
            // (caller > frontmatter > config).
            turn_budget,
            // pi sets `enforceHardTurnLimit` only from the slash delegation adapter
            // (`slash/delegation-adapters.ts:298`); the tool surface never does, so the
            // mid-tool-work deferral stays armed here exactly as upstream leaves it.
            enforce_hard_turn_limit: false,
            cwd: cwd.to_path_buf(),
            deadline_at,
            timeout_ms,
            output_path,
            output_mode,
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
            preferred_provider: None,
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
            // G90: a FOREGROUND single run has no async run directory and therefore no steer
            // inbox — pi supplies `steerInboxDir` only from the background runner, and
            // `control_steer` refuses a foreground run outright for exactly this reason.
            steer_inbox_dir: None,
            // SUBA-049: same reason, for the return half — a foreground run has no run directory,
            // so there is nowhere to write an acknowledgment or a capability record.
            steer_ack_dir: None,
            steer_capability_path: None,
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
    ) {
        {
            let mut controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.insert(
                run_id.as_str().to_string(),
                ForegroundControlEntry {
                    interrupt: run_options.interrupt.clone(),
                    current_agent: Some(agent.name.clone()),
                    current_index: Some(0),
                    current_activity_state: None,
                    // This entry point is pi's `runSinglePath`; its run shape is `single`.
                    mode: crate::background::RunMode::Single,
                    // pi `description: task` (`runs/foreground/execution.ts`'s control registration)
                    // — the caller's own task text, which is what the roster row identifies the run
                    // by (`fleet.ts:723`) and the detail pane prints as `Task` (`fleet.ts:434-437`).
                    description: Some(task.to_string()).filter(|t| !t.trim().is_empty()),
                    current_tool: None,
                    current_path: None,
                    turn_count: None,
                    tool_count: None,
                    tokens: None,
                    started_at: crate::time::now_epoch_millis(),
                    updated_at: crate::time::now_epoch_millis(),
                },
            );
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
            controls.remove(run_id.as_str());
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::testsupport::seed_scope_fixture;

    #[tokio::test]
    async fn run_foreground_errors_before_any_spawn_when_agent_is_unknown() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .run_foreground(dir.path(), "ghost", "do something", Some(ContextMode::Fresh), None, None)
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
            .run_foreground(dir.path(), "ghost", "do something", Some(ContextMode::Fresh), None, None)
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
                Some(ContextMode::Fresh),
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

}
