//! Chain/graph execution: foreground chain walks and the foreground-or-background graph split.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::CancelToken;

use crate::background::runner_main::ExecSingleStepExecutor;
use crate::background::{RunId, RunMode};
use crate::discovery::discover_agents;
use crate::discovery::types::AgentReadScope;
use crate::error::SubagentError;
use crate::exec::ResolvedAgentPersona;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::requests::{BackgroundStepsSpec, GraphRunOutcome};
use crate::extension::host::slash_render::{
    apply_fork_contexts, first_step_task, plan_step_agent_names,
};
use crate::extension::tool::task_items::resolve_chain_dir;
use crate::fork_context::ContextRequest;
use crate::spawn::chain_graph::{
    ChainRunContext, GroupStepResult, OutputRegistry, RunnerStep, SingleStepExecutor, StepResult,
    walk_chain,
};
use crate::spawn::depth::resolve_effective_depth;
use crate::spawn::parallel::GlobalConcurrencyLimit;

impl SubagentExecutor {
    // ---------------------------------------------------------------------------------------
    // Foreground chain/parallel dispatch (R-SA-130: `/chain`, `/parallel`, `/run-chain`'s
    // synchronous shape — the SAME `walk_chain`/`ExecSingleStepExecutor` machinery
    // `background::runner_main`'s hop-2 detached runner drives, reused rather than reimplemented)
    // ---------------------------------------------------------------------------------------

    /// Run an already-resolved [`RunnerStep`] list to completion in the foreground, synchronously
    /// (func-SA §5.1/§5.3; `/chain` and `/parallel`'s non-`--bg` shape). A bare `/parallel` call
    /// is represented as a ONE-element graph whose sole element is a
    /// [`RunnerStep::ParallelGroup`] — `walk_chain` dispatches that exactly like any other group
    /// step in a longer chain (R-SA-052: chain graphs and standalone parallel groups share the
    /// identical dispatch primitive, never a second parallel-only code path).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055) if this process's own recursion-depth
    /// ceiling is already reached — checked before any step is walked. Otherwise propagates
    /// [`walk_chain`]'s own errors (an unresolvable `DynamicGroup.expand` pointer, a
    /// `worktree: true` group whose setup failed, or a `worktree: true` group with no
    /// `worktree_base_dir` configured, R-SA-060..064).
    #[allow(clippy::too_many_arguments)]
    pub async fn run_chain_foreground(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
        original_task: String,
        chain_dir: Option<PathBuf>,
        cancel: CancelToken,
        // pi `chain-execution.ts:606`: `deadlineAt = params.deadlineAt ?? Date.now() + timeoutMs`,
        // computed ONCE here (never per step) and threaded, alongside the nominal `timeout_ms`
        // itself, into every step this walk dispatches via `ChainRunContext`. `None` (the tool gave
        // no `timeoutMs`/`maxRuntimeMs`, or this is a slash-command chain, which carries no timeout
        // param at all) means no chain-wide deadline, matching pi exactly.
        timeout_ms: Option<u64>,
    ) -> Result<(Vec<StepResult>, Vec<GroupStepResult>), SubagentError> {
        self.run_chain_foreground_with_control(
            cwd,
            graph,
            resolved_agents,
            original_task,
            chain_dir,
            cancel,
            timeout_ms,
            None,
            // SUBA-N06: no `includeProgress` on the slash/integration surface either.
            None,
        )
        .await
    }

    /// [`Self::run_chain_foreground`] plus the per-call `control` override (SUBA-N05).
    ///
    /// Split out rather than added as an eighth parameter to the public entry point: that signature
    /// is consumed by integration tests and by the slash surface, neither of which has a `control`
    /// param to supply, and pi's own slash/chain callers likewise pass only the extension-level
    /// config. The override is folded against `subagents.control` here, so a caller passing `None`
    /// still gets the CONFIGURED thresholds rather than the hardcoded defaults — which is itself a
    /// fix: before SUBA-N05 every foreground chain/parallel step ran with `control_config: None`
    /// and the extension-level block reached nothing on this path at all.
    ///
    /// pi: the shared `execute` entry resolves `controlConfig = resolveControlConfig(deps.config.control,
    /// effectiveParams.control)` (`subagent-executor.ts:3385`), `runChainPath` reads it off
    /// `ExecutionContextData.controlConfig` and threads it into `chain-execution.ts`'s per-step
    /// `runSync` calls (`:322,491,733,953,1184`), all @v0.34.0.
    #[allow(clippy::too_many_arguments)]
    async fn run_chain_foreground_with_control(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
        original_task: String,
        chain_dir: Option<PathBuf>,
        cancel: CancelToken,
        timeout_ms: Option<u64>,
        control_override: Option<crate::registration::ControlConfig>,
        // SUBA-N06: pi's `includeProgress`, threaded onto every step this walk dispatches. pi does
        // the same on its chain path — `executeChain({ ..., includeProgress: params.includeProgress })`
        // (`subagent-executor.ts:2012` @v0.34.0) → `progress: input.includeProgress ?
        // input.allProgress : undefined` (`chain-execution.ts:167`). `None` for every
        // slash-command caller (no such param on that surface, matching pi).
        include_progress: Option<bool>,
    ) -> Result<(Vec<StepResult>, Vec<GroupStepResult>), SubagentError> {
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // T0.1/C13: hand the plan-time persona map straight to the foreground executor — the SAME
        // `ExecSingleStepExecutor` the hop-2 detached runner drives — so a foreground `/chain`//
        // `/parallel` step dispatches its REAL persona (never a placeholder), stamped with THIS
        // process's own live depth envelope at dispatch time (`ResolvedAgentPersona::to_agent_config`).
        // Intercom child-bridge activation for the foreground `/chain`//`/parallel` path (pi
        // `data.intercomBridge.orchestratorTarget`): mint a run id for this foreground walk and pass
        // this orchestrator's own presence target so each foreground-spawned child registers its
        // `contact_supervisor` bridge addressed at the live human orchestrator — the SAME activation
        // the background path gets via `RunnerConfig`. `None` target leaves each child un-bridged.
        let executor: Arc<dyn SingleStepExecutor> = Arc::new(
            ExecSingleStepExecutor::foreground(
                depth,
                Arc::new(resolved_agents),
                self.orchestrator_intercom_target(),
                Some(RunId::new()),
                // Session-model inheritance for foreground `/chain`//`/parallel` steps (pi's
                // `data.parentModel`, read at `subagent-executor.ts:3165,3549` @v0.43.0 and fed by the
                // same `requestParentModel`): an inheriting step (no persona `model:`, no per-step
                // override) runs the parent's model, the SAME inheritance — through the SAME
                // [`SubagentExecutor::remembered_parent_model`] memory — the foreground single-run path
                // applies.
                self.remembered_parent_model(),
                // SUBA-003: the cwd's `subagents.modelScope` policy, so a foreground chain/parallel
                // step's own `model:` is policed exactly as a single run's `model` is.
                Self::resolve_model_scope(cwd, &cfg.roots)?,
                // The extension config's in-process binary override, from the same snapshot this
                // function already took. `None` for every ordinary configuration, which leaves each
                // step resolving its command from the environment as before.
                cfg.spawn_command.clone(),
            )
            // SUBA-N05 (pi `controlConfig: input.controlConfig` on every per-step `runSync`,
            // `chain-execution.ts:322,491,733` @v0.34.0): the extension-level `subagents.control`
            // block folded with this call's own override, so a foreground chain/parallel step's child
            // stream is judged against the CONFIGURED attention thresholds instead of the hardcoded
            // defaults this path used to fall back to.
            .with_control(Some(crate::exec::control::resolve_control_config(
                cfg.control.as_ref(),
                control_override.as_ref(),
            )))
            // SUBA-N06 (pi `chain-execution.ts:167`, gated on the same `includeProgress` the SINGLE
            // path uses): each foreground chain/parallel step's own `SingleResult` carries its
            // progress snapshot, which is where cyrup's [CYRUP-DELTA] on placement puts pi's
            // `details.progress` array.
            .with_include_progress(include_progress),
        );
        let global_limit =
            GlobalConcurrencyLimit::new(cfg.global_concurrency_limit.max(1) as usize);
        // R-SA-035/036 (pi `chain-execution.ts:606`): the chain-wide deadline is computed ONCE here,
        // before the walk starts, from the nominal `timeout_ms` budget the caller resolved
        // (`resolve_foreground_timeout`) — never re-derived per step, so it monotonically shrinks
        // across every step/group this walk dispatches. `None` when no timeout was requested.
        let deadline_at =
            timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
        // Read before `cfg.worktree_base_dir` (a non-`Copy` `Option<PathBuf>`) is moved out of
        // `cfg` below by the struct literal — `dynamic_fanout_max_items()` takes `&self` on the
        // whole (by-then-partially-moved) `cfg`, so it must be evaluated first.
        let dynamic_fanout_max_items = cfg.dynamic_fanout_max_items();
        let ctx = ChainRunContext {
            cwd: cwd.to_path_buf(),
            deadline_at,
            timeout_ms,
            // pi threads the host `AbortSignal` into the executor for every mode
            // (`extension/index.ts:498-500`), so an abort of the tool call must reach a
            // foreground `/chain`//`/parallel` walk's children too, not just SINGLE mode.
            cancel,
            global_limit,
            worktree_base_dir: cfg.worktree_base_dir,
            // A (pi `originalTask`/`chainDir`, `chain-execution.ts:536-541,555` @v0.34.0): the chain's real
            // top-level task + dedicated scratch chain dir, resolved once by the orchestrator
            // (`run_or_background_graph`) and threaded straight in, so a foreground `/chain` resolves
            // `{task}`/`{chain_dir}` to the SAME values the detached background runner does.
            original_task,
            chain_dir,
            // C16 (pi `config.chain.dynamicFanout.maxItems`): the SAME run-wide cap the background
            // path's `ChainRunContext` now also carries (via `RunnerConfig::dynamic_fanout_max_items`)
            // — a foreground `DynamicGroup` step whose own `expand.maxItems` is absent falls back to
            // this value instead of always failing materialization.
            dynamic_fanout_max_items,
            // SUBA-093: the foreground walk publishes no `status.json` and runs no control inbox,
            // so nothing reads this slot; it exists for the background runner, which re-stamps it
            // per dispatch.
            step_slot: crate::spawn::chain_graph::StepSlot::Exclusive(0),
        };
        let mut registry = OutputRegistry::new();
        walk_chain(&graph, &mut registry, &executor, &ctx).await
    }

    // ---------------------------------------------------------------------------------------
    // Shared chain/parallel plan execution (R-SA-130): the ONE path both the `subagent` tool's
    // `chain[]`/`tasks[]` shapes AND the `/chain`//`/parallel`//`/run-chain` slash commands funnel
    // through. Resolves every step's REAL persona at plan time (T0.1/C13), resolves fork-context
    // once for the whole batch (R-SA-137), then either walks the graph to completion in the
    // foreground or hands it to the detached hop-1 runner — never a second divergent code path.
    // ---------------------------------------------------------------------------------------

    /// Resolve personas + fork-context for `graph`, then run it foreground (walk to completion) or
    /// background (detached hop-1 runner), returning a structured [`GraphRunOutcome`] the CALLER
    /// renders (the slash commands render sequential/`N`-step text; the tool's PARALLEL mode renders
    /// pi's `N/M succeeded` summary — see `render_parallel_tool_summary`). Sharing this method is
    /// what lets the tool's `route_parallel_mode`/`route_chain_mode` reuse the identical
    /// persona-resolution + fork-context + walk machinery the slash surface already uses, rather
    /// than reimplementing it (R-SA-130).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055) when the recursion ceiling is already
    /// reached, [`SubagentError::AgentNotFound`] when any step names an unresolvable agent (fail
    /// fast at plan time, matching pi's upfront agent-name validation), or propagates fork-context /
    /// background-spawn / chain-walk errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_or_background_graph(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: Option<ContextRequest>,
        background: bool,
        task: Option<String>,
        cancel: CancelToken,
        // pi `timeoutMs` — the run-level wall-clock budget, honoured on BOTH outcomes: the
        // foreground walk races it via `ChainRunContext::deadline_at`, and (SUBA-N03) a background
        // run carries it to the detached hop-2 runner on `RunnerConfig::timeout_ms` +
        // `deadline_at_ms`, exactly as upstream's `executeAsyncChain(id, { …, timeoutMs:
        // data.timeoutMs, … })` does (`subagent-executor.ts:2568` @v0.34.0; the deadline is armed
        // at `async-execution.ts:677`).
        //
        // `None` for every slash-command caller (which exposes no timeout param at all) and for
        // `route_parallel_mode` (timeout wiring for bare PARALLEL is a separate unit);
        // `route_chain_mode` is the one caller that resolves a real value from the tool's
        // `timeoutMs`/`maxRuntimeMs` params.
        timeout_ms: Option<u64>,
        // SUBA-N05: the RAW per-call `control` override (pi `params.control`), folded here against
        // the extension-level `subagents.control` block by
        // [`crate::exec::control::resolve_control_config`] and threaded into BOTH outcomes — the
        // detached runner's `RunnerConfig::control` and the foreground walk's per-step
        // `RunOptions::control_config`. `None` for every slash-command caller (no `control` param
        // on that surface, matching pi, whose slash path likewise passes only the config).
        control_override: Option<crate::registration::ControlConfig>,
        // SUBA-N06: pi's `includeProgress`, threaded into BOTH outcomes — the detached runner's
        // `RunnerConfig::include_progress` and the foreground walk's per-step
        // `RunOptions::include_progress`. `None` for every slash-command caller (no such param on
        // that surface, matching pi).
        include_progress: Option<bool>,
        // pi `params.chainDir` — the caller's explicit chain artifact directory, honoured verbatim
        // when given and otherwise defaulted below: `chainDir: params.chainDir ??
        // getProjectChainRunsDir(effectiveCwd)` (`subagent-executor.ts:2623` @v0.43.0). That line
        // lives in `runChainPath`, so this is CHAIN-mode-only upstream and every other caller here
        // passes `None` — `route_parallel_mode`, the slash surface (which exposes no `chainDir`
        // param at all), and the tests.
        //
        // Before this parameter existed the tool ADVERTISED `chainDir` with pi's description
        // copied verbatim (`schemas.ts:263`), deserialized it into `SubagentToolParams::chain_dir`,
        // counted it in `provided_keys()` — and then dropped it on the floor, because this boundary
        // had nowhere to put it. Same defect shape as SUBA-041/SUBA-N03: a narrow seam silently
        // eating an advertised param.
        chain_dir_override: Option<PathBuf>,
    ) -> Result<GraphRunOutcome, SubagentError> {
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before persona resolution (real
        // discovery I/O) or fork-context resolution (real session I/O).
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // R-SA-053 (pi `chain-execution.ts:584-596`): validate EVERY chain's output bindings
        // (duplicate `as` names, malformed/unknown `{outputs.x}` references, dynamic-fanout `expand`
        // source) up front, before persona resolution, chain-dir creation, or ANY step is dispatched
        // — a tool `chain[]`/slash `/chain`//`/run-chain` graph gets the SAME upfront check a saved
        // chain file already gets at parse time (`discovery::chains::validate_chain_output_bindings`),
        // so a later-step defect fails immediately instead of only once an earlier step (which may
        // have already spawned real children and spent real tokens) reaches the bad reference.
        crate::spawn::chain_graph::validate_runner_step_output_bindings(&graph)
            .map_err(SubagentError::ChainOutputInvalid)?;

        // A (pi `originalTask`, `chain-execution.ts:632-640` @v0.43.0): the run-wide `{task}` value — the
        // explicit call-site task if non-empty, else the graph's first step's first task. Resolved
        // ONCE here, the shared choke point, so BOTH the foreground walk and the detached background
        // runner substitute the identical `{task}`.
        let original_task = task
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| first_step_task(&graph));
        // A (pi `chainDir`, `chain-execution.ts:654` @v0.43.0): a dedicated per-run scratch directory under
        // the scoped chain-runs root, CREATED before dispatch so `{chain_dir}` resolves to an already-
        // existing directory on both the foreground and background paths (the detached runner only
        // substitutes the path string). Housekept by `artifacts::cleanup_old_chain_dirs`.
        //
        // This SAME id also identifies the run itself on the FOREGROUND path (pi `runId`,
        // `subagent-executor.ts:4941`/`result-intercom.ts:246` @v0.43.0): a foreground parallel/chain run
        // that attempts out-of-band intercom delivery must cite its own real run id in the payload/
        // receipt (`"Run: {runId}"`), never a second, disconnected id minted only for that message —
        // an orchestrator correlating a follow-up status/resume action against the id it just saw in
        // the receipt would otherwise find nothing. See [`GraphRunOutcome::Foreground::run_id`].
        let foreground_run_id = RunId::new();
        // pi `chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`
        // (`subagent-executor.ts:2623` @v0.43.0): an explicit caller value WINS and is used exactly
        // as given — pi does not rewrite it either, and `chain-execution.ts:283` is what resolves a
        // step's relative `output` against it. The `unwrap_or_else` fallback keeps cyrup's existing
        // per-run subdirectory ([CYRUP-DELTA] vs pi's flat project chain-runs dir), which the block
        // below relies on for `{chain_dir}` uniqueness and which `cleanup_old_chain_dirs` housekeeps.
        let chain_dir = resolve_chain_dir(
            chain_dir_override,
            cwd,
            &foreground_run_id,
            cfg.artifact_dir_preference(),
        );
        crate::background::ensure_accessible_dir(&chain_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        // T0.1/C13: resolve every named persona up front (also the upfront agent-name validation —
        // an unresolvable agent fails here, before any child is spawned, matching pi's `/chain`//
        // `/parallel` name check).
        let resolved_agents = self.resolve_plan_personas(
            cwd,
            plan_step_agent_names(&graph),
            AgentReadScope::Both,
            &cfg.roots,
        )?;
        // Fork default-mode + per-index branch (Tier-2, R-SA-137/R-SA-138, pi
        // `resolveAgentDefaultContextPolicy` + `preflightForkSessionsForStaticTasks`): resolve EACH
        // step's effective context independently (an omitted call-site `context` defers to THAT
        // step's agent's own `default_context`, never a batch-wide forced `Fresh`), then, for every
        // forking step, mint its OWN per-flat-index branch off a SINGLE shared resolver — two sibling
        // parallel tasks that both fork get two DISTINCT branch session files, not one shared branch.
        // `first_session_file` is the run-level session recorded for resume metadata only.
        // Blocker #4: branch every forking step from the REAL live-orchestrator session file (P-1),
        // not the continue_recent(cwd) mtime heuristic.
        let session_file = self.host_services().and_then(|s| s.session_file());
        let resolver = Self::fork_resolver(cwd, session_file.as_deref(), &cfg.roots);
        // SUBA-079: the `subagents.defaultSubagentContext` rung, validated here — this is the only
        // side of the graph path holding the live extension config.
        let config_default = crate::fork_context::resolve_default_subagent_context(
            self.config_snapshot()
                .await
                .default_subagent_context
                .as_ref(),
        )
        .map_err(SubagentError::Management)?;
        let (graph, first_session_file) =
            apply_fork_contexts(&resolver, context, config_default, &resolved_agents, graph)
                .await?;

        if background {
            let run_id = self
                .spawn_background_steps(
                    cwd,
                    BackgroundStepsSpec {
                        // SUBA-021: unbudgeted on this path (see the field doc).
                        usage_budget: None,
                        turn_budget: None,
                        // SUBA-073: no policy on this path — same pre-existing incompleteness as
                        // `turn_budget` immediately above; not this task's fix to extend.
                        permission_rules: None,
                        steps: graph,
                        mode,
                        session_file: first_session_file,
                        resolved_agents,
                        original_task,
                        chain_dir: Some(chain_dir),
                        // SUBA-N05 (pi `executeAsyncChain(..., { controlConfig, ... })`,
                        // `subagent-executor.ts:1312-1313` @v0.34.0).
                        control: Some(crate::exec::control::resolve_control_config(
                            cfg.control.as_ref(),
                            control_override.as_ref(),
                        )),
                        // SUBA-N06.
                        include_progress,
                        // SUBA-N03: no path needs to be resolved against this run's id before the
                        // spawn call (a chain step declares its own `output:` per step), so it is
                        // simply minted here.
                        run_id: RunId::new(),
                        // SUBA-N03: the run-level timeout the caller resolved, honoured on the
                        // BACKGROUND outcome too — pi `executeAsyncChain(id, { …, timeoutMs:
                        // data.timeoutMs, … })` (`subagent-executor.ts:2568` @v0.34.0) →
                        // `deadlineAt = Date.now() + params.timeoutMs` (`async-execution.ts:677`) →
                        // `spawnRunner({ timeoutMs, deadlineAt })` (`:723,798`). `route_chain_mode`
                        // used to REFUSE this combination outright, citing a pi precedent that does
                        // not exist; see the note at `route_single`'s background branch.
                        timeout_ms,
                        // The `/chain`//`/parallel`//`/run-chain --bg` surface and the tool's CHAIN/
                        // PARALLEL modes expose no `share`/`artifacts` param — those are
                        // `subagent`-tool SINGLE-mode only, per pi's own schema.
                        share: None,
                        artifacts_dir: None,
                        artifact_config: crate::artifacts::ArtifactConfig::default(),
                    },
                )
                .await?;
            Ok(GraphRunOutcome::Background(run_id))
        } else {
            // `is_group` must be computed BEFORE `graph` is moved into `run_chain_foreground` —
            // `group_results` is populated in chain order but NOT indexed by overall step position
            // (walk_chain's own doc), so a renderer needs both the graph's per-step shape and the
            // per-group child detail to zip them back together.
            let is_group: Vec<bool> = graph
                .iter()
                .map(|s| {
                    matches!(
                        s,
                        RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_)
                    )
                })
                .collect();
            let (results, groups) = self
                .run_chain_foreground_with_control(
                    cwd,
                    graph,
                    resolved_agents,
                    original_task,
                    Some(chain_dir),
                    cancel,
                    timeout_ms,
                    control_override,
                    include_progress,
                )
                .await?;
            Ok(GraphRunOutcome::Foreground {
                run_id: foreground_run_id,
                results,
                is_group,
                groups,
            })
        }
    }

    // ---------------------------------------------------------------------------------------
    // Saved-chain resolution (`/run-chain`, R-SA-129)
    // ---------------------------------------------------------------------------------------

    /// Resolve a saved chain by its fully-qualified name (R-SA-008-style exact string equality
    /// only — mirrors [`crate::extension::SubagentExecutor::resolve_agent`]'s identical convention applied to chain names instead of
    /// agent names), via the real, on-demand, re-scanned-per-call discovery pipeline (R-SA-019).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::ChainNotFound`] if no discovered chain matches `name` exactly, or
    /// propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings abort).
    pub fn resolve_chain(
        &self,
        cwd: &Path,
        name: &str,
        roots: &crate::paths::Roots,
    ) -> Result<crate::discovery::types::ChainDefinition, SubagentError> {
        let cfg = self.discovery_config(cwd, roots)?;
        let result = discover_agents(&cfg, None)?;
        // Cross-scope run precedence Project > User > Package > Builtin (pi `discoverSavedChains`
        // last-wins map, slash-commands.ts:172-177 @v0.34.0) — NOT a naive first-match, which incorrectly let a
        // User chain shadow a same-named Project chain. See `discovery::resolve_chain_by_name`.
        crate::discovery::resolve_chain_by_name(&result.chains, name)
            .cloned()
            .ok_or_else(|| SubagentError::ChainNotFound(name.to_string()))
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
    use crate::spawn::chain_graph::SingleStepSpec;

    /// T6 upfront agent-name validation for `/chain` (pi validates every named agent before starting
    /// a chain rather than spawning a partial run that dies mid-walk): a chain naming an agent that
    /// resolves to nothing in the discovery scope fails fast with [`SubagentError::AgentNotFound`]
    /// BEFORE any child process — and therefore before any spawn scratch directory — is ever created.
    #[tokio::test]
    async fn unknown_agent_in_a_chain_is_rejected_upfront_before_any_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let graph = vec![RunnerStep::SingleStep(SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: "does-not-exist".to_string(),
            task: "do the thing".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];
        // `GraphRunOutcome` (the Ok type) is not `Debug`, so match manually rather than `expect_err`.
        match executor
            .run_or_background_graph(
                dir.path(),
                graph,
                RunMode::Chain,
                None,
                false,
                None,
                CancelToken::new(),
                None,
                None,
                None,
                None,
            )
            .await
        {
            Err(SubagentError::AgentNotFound(name)) => assert_eq!(name, "does-not-exist"),
            Err(other) => panic!("expected AgentNotFound(does-not-exist), got: {other}"),
            Ok(_) => panic!("an unknown agent in /chain must be rejected before running"),
        }
        assert!(
            !crate::background::attempt_scratch_dir(dir.path()).exists(),
            "upfront rejection must happen before any child (and its scratch dir) is created"
        );
    }

    /// [`SubagentExecutor::run_chain_foreground`] (the foreground `/chain`/`/parallel` walker) must
    /// reject a blocked depth ceiling before walking a single [`RunnerStep`] — proven with a
    /// non-empty graph so that, absent the guard, `walk_chain` would otherwise attempt to dispatch
    /// at least one step (and, for a real agent, spawn at least one real child process).
    #[tokio::test]
    async fn run_chain_foreground_rejects_on_depth_before_walking_any_step() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let graph = vec![RunnerStep::SingleStep(
            crate::spawn::chain_graph::SingleStepSpec {
                skills: None,
                session_dir: None,
                agent: "worker".to_string(),
                task: "do something".to_string(),
                cwd: None,
                model: None,
                tools: None,
                extensions: None,
                session_file: None,
                max_depth_override: None,
                structured_output_schema: None,
                output: None,
                output_path: None,
                output_mode: None,
                reads: None,
                acceptance: None,
                context: None,
                agent_scope: None,
            },
        )];

        let err = executor
            .run_chain_foreground(
                dir.path(),
                graph,
                BTreeMap::new(),
                String::new(),
                None,
                CancelToken::new(),
                None,
            )
            .await
            .expect_err("a blocked depth ceiling must reject before walking any step");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "got: {err:?}"
        );
    }
}
