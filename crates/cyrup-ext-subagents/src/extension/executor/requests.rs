//! The request/override DTOs callers hand to [`SubagentExecutor`]'s run entry points, plus
//! the `status` view selector.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, ModelId};

use crate::background::{RunId, RunMode};
use crate::discovery::types::AgentReadScope;
use crate::exec::ResolvedAgentPersona;
use crate::fork_context::ContextMode;
use crate::spawn::chain_graph::{GroupStepResult, RunnerStep, StepResult};

/// The structured result of [`crate::extension::SubagentExecutor::run_or_background_graph`]: either a detached
/// background run was launched (carrying its [`RunId`]), or the graph was walked to completion in
/// the foreground and its per-step results are returned for the caller to render. Keeping this
/// structured (rather than pre-rendering a string inside the executor) is what lets the tool's
/// PARALLEL mode render pi's `N/M succeeded` summary while the slash commands render their own
/// per-step text, both over the SAME underlying walk.
pub enum GraphRunOutcome {
    /// A background run was spawned (detached hop-1); nothing waited on its completion (R-SA-074).
    Background(RunId),
    /// The graph was walked to completion in the foreground. `results`/`is_group`/`groups` are the
    /// exact triple [`crate::extension::host::slash_render::render_chain_results`]/[`crate::extension::tool::task_items::render_parallel_tool_summary`] consume. `run_id` is
    /// THIS run's own real, stable id (pi `runId`, `subagent-executor.ts:4941` @v0.43.0) — the same one
    /// used to derive this run's `{chain_dir}` — never a fresh id minted only for an out-of-band
    /// intercom payload/receipt (R-SA-123/124/125's "Run: {runId}" must be correlatable).
    Foreground {
        run_id: RunId,
        results: Vec<StepResult>,
        is_group: Vec<bool>,
        groups: Vec<GroupStepResult>,
    },
}

/// SUBA-041 — the per-call SINGLE-mode override surface pi's `runSinglePath` honors
/// (`subagent-executor.ts:3561-3564` output/outputMode/skill, `:2962` acceptance, `:2874` share,
/// `:3387-3401` artifacts/sessionDir), carried as ONE owned bundle so
/// [`ForegroundRunRequest`] stays within the field budget and every non-tool caller (the `/run`
/// slash surface, tests) can keep saying [`Default::default`] for "no overrides at all".
///
/// The values here are the RAW tool params, not resolved paths: pi resolves an `output` string
/// against `resolveSingleRunOutputBaseDir(deps, artifactsDir, runId)`
/// (`subagent-executor.ts:2838-2842,3666`), a base directory that only exists once the run id has
/// been minted and the artifacts dir computed — i.e. inside `run_foreground_impl`, not at the
/// dispatch site.
#[derive(Debug, Clone, Default)]
pub struct SingleRunOverrides {
    /// pi `params.output` (`OutputOverride`, `schemas.ts:42-48`): a path string, `false`/`"false"`
    /// to disable, or `true`/`"true"` to mean "the persona's own declared output". `None` = the
    /// param was omitted, which defers to the persona's `output:` exactly as pi's
    /// `params.output !== undefined ? params.output : agentConfig.output` does.
    pub output: Option<serde_json::Value>,
    /// pi `params.outputMode` (`schemas.ts:50-53`): `"inline"` (pi's own default) or `"file-only"`.
    pub output_mode: Option<String>,
    /// pi `params.skill` (`SkillOverride`, `schemas.ts:33-40`), already normalized through
    /// [`crate::extension::tool::task_items::normalize_skill_input`]: `Some(names)` replaces the persona's own `skills:`, `Some(vec![])`
    /// is the explicit `skill: false` "no skills" form, `None` inherits the persona's list.
    pub skills: Option<Vec<String>>,
    /// pi `params.acceptance` (`AcceptanceOverride`, `schemas.ts:80-93`), already validated and
    /// lowered by [`crate::exec::acceptance::lower_acceptance_input`]. `None` defers to
    /// [`crate::exec::acceptance::AcceptanceContract::heuristic_default`] (R-SA-023), which is
    /// exactly what pi's `acceptance: "auto"` / omitted means.
    pub acceptance: Option<crate::exec::acceptance::AcceptanceContract>,
    /// pi `params.share` (`subagent-executor.ts:3354` `shareEnabled`).
    pub share: Option<bool>,
    /// pi `params.sessionDir` (`subagent-executor.ts:5044-5052`), still the RAW string: it is
    /// tilde-expanded and `path.resolve`d, then suffixed with pi's own `<runId>/run-0` layout once
    /// the run id exists.
    pub session_dir: Option<String>,
    /// pi `params.artifacts` (`subagent-executor.ts:3387-3390`): `enabled = artifacts !== false`, so
    /// only an explicit `Some(false)` turns the artifact quadruple off.
    pub artifacts: Option<bool>,
    /// pi `params.control` (`ControlOverrides`, `extension/schemas.ts:242-255,339` @v0.43.0),
    /// already lowered from the wire object by
    /// [`crate::exec::control::parse_control_overrides`]. `None` = the param was omitted, which
    /// defers wholly to the extension-level `subagents.control` block and then to
    /// `DEFAULT_CONTROL_CONFIG`, exactly as pi's `resolveControlConfig(deps.config.control,
    /// undefined)` does (`subagent-executor.ts:1179`). Resolution happens inside
    /// `run_foreground_impl`, not at the dispatch site, because the extension-level base is read
    /// off the live `config_snapshot` there.
    pub control: Option<crate::registration::ControlConfig>,
    /// pi `params.includeProgress` (`extension/schemas.ts:272` @v0.34.0): R-SA-043 compaction's
    /// documented opt-out. Threaded straight onto [`crate::exec::RunOptions::include_progress`],
    /// where only `Some(true)` populates [`crate::exec::SingleResult::progress`] — pi's own
    /// truthiness gate (`progress: params.includeProgress ? allProgress : undefined`,
    /// `subagent-executor.ts:3819`).
    pub include_progress: Option<bool>,
    /// SUBA-043: pi `params.outputSchema` (`extension/schemas.ts:351` @v0.43.0), read on the
    /// single path at `runs/foreground/subagent-executor.ts:3651,3671`. Threaded straight onto
    /// [`crate::exec::RunOptions::structured_output_schema`], which is what creates the run's
    /// capture runtime and writes the child's `STRUCTURED_OUTPUT_SCHEMA`/`_CAPTURE` env pair
    /// (`runs/shared/pi-args.ts:759-762`). `None` = the caller declared no schema, and the child
    /// then registers no `structured_output` tool at all.
    pub output_schema: Option<serde_json::Value>,
    /// SUBA-047: pi `params.toolBudget` (`extension/schemas.ts:354` @v0.43.0), already validated by
    /// [`crate::exec::tool_budget::validate_tool_budget_config`]. Applied onto the resolved
    /// persona's own budget with pi's precedence — `params.toolBudget ?? agentConfig.toolBudget ??
    /// params.configToolBudget` (`runs/background/async-execution.ts:1298`) — i.e. caller wins,
    /// then frontmatter. cyrup has no extension-config rung yet, so the chain is two long.
    pub tool_budget: Option<crate::discovery::types::ResolvedToolBudget>,
    /// SUBA-008: pi `params.turnBudget` (`extension/schemas.ts:328` @v0.43.0), already validated by
    /// [`crate::exec::turn_budget::resolve_turn_budget_config`].
    ///
    /// Only the CALLER's rung of upstream's chain — the agent-frontmatter and extension-config
    /// rungs below it are applied by `run_foreground_impl`, which is where the resolved persona and
    /// the live config are both already in hand (pi resolves the same chain at
    /// `subagent-executor.ts:4928`, immediately after `applySingleAgentLaunchDefaults`).
    pub turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-021: pi `params.usageBudget` (`extension/schemas.ts:330` @v0.43.0), already validated
    /// by [`crate::exec::usage_budget::validate_usage_budget_config`] — pi's own
    /// `validateUsageBudgetConfig(params.usageBudget)` — so a malformed budget is refused at the
    /// tool boundary with upstream's message rather than degrading to "unbudgeted".
    ///
    /// Unlike [`Self::turn_budget`] this has exactly ONE rung: upstream carries `usageBudget` from
    /// the call parameters only (`subagent-runner.ts:172`, `async-execution.ts:167`/`:216`) — there
    /// is no `usageBudget:` frontmatter key and no `subagents.usageBudget` config key at either
    /// baseline, so inventing either rung here would be a divergence, not a convenience.
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
}

/// The seven inputs one foreground single run needs, bundled into one borrowed request so
/// [`crate::extension::SubagentExecutor::run_foreground_streaming`] and the shared `run_foreground_impl` stay within
/// the argument-count budget (the non-streaming [`crate::extension::SubagentExecutor::run_foreground`] keeps its
/// original flat signature for backward compatibility and builds this internally). All fields
/// borrow for the duration of the one `run_foreground*` call they are passed to.
pub struct ForegroundRunRequest<'a> {
    /// SUBA-041: the per-call SINGLE-mode override bundle (`output`/`outputMode`/`skill`/
    /// `acceptance`/`share`/`sessionDir`/`artifacts`). [`SingleRunOverrides::default`] is
    /// "no overrides", which reproduces this entry point's pre-SUBA-041 behavior exactly.
    pub overrides: SingleRunOverrides,
    /// The task's working directory (also the discovery root for the named persona).
    pub cwd: &'a Path,
    /// The persona name to resolve and run (func-SA §5.2).
    pub agent_name: &'a str,
    /// The task text handed to the child (pi's `Task: <task>` child prompt).
    pub task: &'a str,
    /// The resolved execution-time agent-discovery scope (pi `resolveExecutionAgentScope`,
    /// `subagent-executor.ts:2973`): narrows the User-vs-Project axis when resolving `agent_name`.
    pub agent_scope: AgentReadScope,
    /// Call-site fork/fresh context; `None` defers to the persona's own `default_context`.
    pub context: Option<ContextMode>,
    /// Per-call model override (added to the availability set, R-SA-038); `None` inherits.
    pub model_override: Option<ModelId>,
    /// Foreground timeout budget in milliseconds (pi `timeoutMs`/`maxRuntimeMs`); `None` = none.
    pub timeout_ms: Option<u64>,
    /// The host's own cancellation token for this tool call (pi `execute(id, params, signal, ...)`,
    /// `extension/index.ts:498-500`), threaded straight into [`crate::exec::RunOptions::cancel`] so an abort of
    /// the tool call (user Esc / turn abort) drives the running child through the real
    /// SIGINT→SIGTERM→SIGKILL escalation instead of being silently dropped at this seam.
    pub cancel: CancelToken,
}

/// The inputs one BACKGROUND single run needs, bundled into one borrowed request so
/// [`crate::extension::SubagentExecutor::spawn_background`] stays within the argument-count budget — the same role
/// [`ForegroundRunRequest`] plays for the foreground path and [`BackgroundStepsSpec`] for the
/// general step-graph path. All borrowed fields live for the duration of the one
/// `spawn_background` call they are passed to.
pub struct BackgroundSingleRequest<'a> {
    /// SUBA-043: pi `params.outputSchema` (`extension/schemas.ts:351` @v0.43.0), forwarded to the
    /// async SINGLE step exactly as `runSinglePath`'s foreground twin forwards it. Lands on
    /// [`crate::spawn::chain_graph::SingleStepSpec::structured_output_schema`], which hop 2 lowers
    /// into that step's [`crate::exec::RunOptions::structured_output_schema`]. Upstream:
    /// `outputSchema: params.outputSchema` on the async-single step builder
    /// (`runs/background/async-execution.ts`), the same field the `tasks[]` lowering already fills.
    pub structured_output_schema: Option<serde_json::Value>,
    /// SUBA-047: pi `params.toolBudget` on the async SINGLE path
    /// (`runs/background/async-execution.ts:1298` @v0.43.0), already validated. Folded onto the
    /// resolved persona in `spawn_background` so hop 2's dispatch sees it as that agent's budget.
    pub tool_budget: Option<crate::discovery::types::ResolvedToolBudget>,
    /// SUBA-008: pi `params.turnBudget` on the async SINGLE path (`async-execution.ts:1469`),
    /// already validated. Only the CALLER's rung — `spawn_background` applies the agent-frontmatter
    /// and extension-config rungs below it, exactly as `run_foreground_impl` does.
    pub turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-021: pi `params.usageBudget` on the async SINGLE path (`async-execution.ts:1471`),
    /// already validated. The run-level budget, carried verbatim onto
    /// [`crate::background::runner_main::RunnerConfig::usage_budget`] so hop 2 applies it to every
    /// step (pi enforces ONE `usageBudget` across a whole async run, not one per step).
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
    /// The task's working directory (also the discovery root for the named persona).
    pub cwd: &'a Path,
    /// The persona name to resolve and run.
    pub agent_name: &'a str,
    /// The task text handed to the child.
    pub task: &'a str,
    /// Call-site fork/fresh context; `None` defers to the persona's own `default_context`.
    pub context: Option<ContextMode>,
    /// Per-call model override; `None` inherits (pi `async-execution.ts:1290-1295`).
    pub model_override: Option<ModelId>,
    /// The resolved execution-time agent-discovery scope.
    pub agent_scope: AgentReadScope,
    /// SUBA-N04: the RAW wire `acceptance` policy (pi `AcceptanceOverride`) this run declares, or
    /// `None` for "omitted". pi's async SINGLE path honours it exactly as its foreground one does
    /// (`runs/background/async-execution.ts:1282-1289` resolves `explicit: params.acceptance` with
    /// `async: true`, and `:1319` persists it on the steering recovery descriptor). It rides to the
    /// detached hop-2 runner on the step itself and is lowered there by
    /// [`crate::exec::acceptance::lower_acceptance_input`].
    pub acceptance: Option<serde_json::Value>,
    /// SUBA-N05: the RAW per-call `control` override this run declares, or `None` for "omitted".
    /// [`crate::extension::SubagentExecutor::spawn_background`] folds it against the extension-level
    /// `subagents.control` block via [`crate::exec::control::resolve_control_config`] — the SAME
    /// parent-side resolution the foreground path performs — and carries the RESOLVED value to the
    /// detached runner on [`crate::background::runner_main::RunnerConfig::control`].
    ///
    /// Upstream honours `control` on its async SINGLE path exactly this way:
    /// `executeAsyncSingle(id, { …, controlConfig: resolveControlConfig(deps.config.control,
    /// effectiveParams.control), … })` (`subagent-executor.ts:2845,2868-2870` @v0.34.0). Before this
    /// field existed the param was parsed at the tool boundary, was NOT on `route_single`'s
    /// foreground-only refusal list, and had no `BackgroundSingleRequest` field — i.e. it was
    /// advertised-and-silently-dropped, the exact defect SUBA-041 exists to prevent.
    pub control: Option<crate::registration::ControlConfig>,
    /// SUBA-N06: pi `params.includeProgress` — R-SA-043 compaction's opt-out, carried to the
    /// detached hop-2 runner on [`crate::background::runner_main::RunnerConfig::include_progress`]
    /// and installed on every step's [`crate::exec::RunOptions::include_progress`], so the
    /// persisted `ResultFile`'s `SingleResult`s carry their own progress snapshots.
    ///
    /// **This is deliberately MORE than upstream, and the reason is structural.** pi never passes
    /// `includeProgress` into `executeAsyncSingle` (`subagent-executor.ts:2845-2874` @v0.34.0):
    /// its async return is a "started" message with no results attached, so there is nothing for
    /// the flag to gate. cyrup's async run DOES produce a retrievable `SingleResult` (via
    /// `subagent({action: "status"})` over the terminal result file), so the only two readings
    /// available here are "honour it" and "silently drop it" — and a silent drop is the exact
    /// defect SUBA-041 names.
    pub include_progress: Option<bool>,
    /// SUBA-N03: pi `params.output` (`OutputOverride`, `extension/schemas.ts:42-48`) — the RAW wire
    /// value. `spawn_background` normalizes it against the resolved persona's own `output:` through
    /// the SAME [`crate::extension::tool::task_items::normalize_single_output_override`]/[`crate::extension::tool::task_items::resolve_single_output_path`] pair the
    /// foreground path uses, then lands the resolved absolute path on
    /// [`crate::spawn::chain_graph::SingleStepSpec::output_path`] for hop 2 to honour.
    ///
    /// Upstream does exactly this on its async SINGLE path: `executeAsyncSingle` receives
    /// `output: effectiveOutput` + `outputBaseDir: resolveSingleRunOutputBaseDir(deps, artifactsDir,
    /// id)` (`runs/foreground/subagent-executor.ts:3633-3636` @v0.43.0) and resolves the same
    /// `normalizeSingleOutputOverride`/`resolveSingleOutputPath` pair at
    /// `runs/background/async-execution.ts:905-907`.
    pub output: Option<serde_json::Value>,
    /// SUBA-N03: pi `params.outputMode` (`extension/schemas.ts:50-53`) — `"inline"` (pi's default)
    /// or `"file-only"`. Lands on
    /// [`crate::spawn::chain_graph::SingleStepSpec::output_mode`]. Upstream:
    /// `outputMode: effectiveOutputMode` (`subagent-executor.ts:3637`), consumed at
    /// `async-execution.ts:908-910` where it also drives `validateFileOnlyOutputMode`.
    pub output_mode: Option<String>,
    /// SUBA-N03: pi `params.skill` (`SkillOverride`, `extension/schemas.ts:33-40`), already
    /// normalized through [`crate::extension::tool::task_items::normalize_skill_input`] into the same tri-state
    /// [`SingleRunOverrides::skills`] carries. Lands on
    /// [`crate::spawn::chain_graph::SingleStepSpec::skills`]. Upstream: `skills: skillOverride ===
    /// false ? [] : skillOverride` (`subagent-executor.ts:2856`) → `params.skills ??
    /// agentConfig.skills` (`async-execution.ts:876`) → the runner step's own `skills`
    /// (`async-execution.ts:990`).
    pub skills: Option<Vec<String>>,
    /// SUBA-N03: pi `params.share` (`shareEnabled`, `subagent-executor.ts:4945` @v0.43.0). Carried to hop 2
    /// on [`crate::background::runner_main::RunnerConfig::share`] and thence to every step's
    /// [`crate::exec::RunOptions::share`]. Upstream: `shareEnabled` →
    /// `spawnRunner({ share: shareEnabled })` (`async-execution.ts:965`).
    pub share: Option<bool>,
    /// SUBA-N03: pi `params.sessionDir` (`subagent-executor.ts:5044-5052`), still the RAW string.
    /// `spawn_background` resolves it through the SAME
    /// [`crate::extension::tool::task_items::resolve_single_run_session_root`] the foreground path uses and lands
    /// `<root>/run-0` on [`crate::spawn::chain_graph::SingleStepSpec::session_dir`]. Upstream:
    /// `sessionRoot` → `sessionDir: path.join(sessionRoot, \`async-${id}\`)`
    /// (`async-execution.ts:966`).
    pub session_dir: Option<String>,
    /// SUBA-N03: pi `params.artifacts` (`subagent-executor.ts:3387-3390`): `enabled = artifacts
    /// !== false`, so only an explicit `Some(false)` turns the artifact quadruple off. Reaches hop
    /// 2 as [`crate::background::runner_main::RunnerConfig::artifacts_dir`] = `None` plus an
    /// `enabled: false` [`crate::artifacts::ArtifactConfig`] — pi's own two-term gate
    /// (`artifactsDir: artifactConfig.enabled ? artifactsDir : undefined`,
    /// `async-execution.ts:964`, read back as `if (ctx.artifactsDir &&
    /// ctx.artifactConfig?.enabled !== false)`, `subagent-runner.ts:1192`).
    pub artifacts: Option<bool>,
    /// SUBA-N03: pi `params.timeoutMs`/`params.maxRuntimeMs`, already validated and de-aliased by
    /// [`crate::extension::tool::params::resolve_foreground_timeout`]. Carried to hop 2 as the nominal
    /// [`crate::background::runner_main::RunnerConfig::timeout_ms`] plus an ABSOLUTE
    /// [`crate::background::runner_main::RunnerConfig::deadline_at_ms`] stamped at spawn time.
    ///
    /// **This corrects an inverted claim, not merely a missing feature.** The refusal this field
    /// replaces cited "pi's own precedent of erroring on timeoutMs + async
    /// (subagent-executor.ts:3022)". No such precedent exists at v0.34.0: `:3015-3030` is
    /// foreground intercom-receipt construction, and `git grep` over the whole of v0.34.0 `src/`
    /// finds no timeout-vs-async refusal anywhere. Upstream HONOURS it —
    /// `extension/schemas.ts:265-266` and `extension/tool-description.ts:25,:73` all say `timeoutMs`
    /// applies to "foreground and async/background runs", and `async-execution.ts:924,982-983` @v0.34.0 arms
    /// a real deadline from it.
    pub timeout_ms: Option<u64>,
}

/// The already-resolved, plan-shaped inputs [`crate::extension::SubagentExecutor::spawn_background_steps`] takes from
/// its caller, bundled into one owned spec so that entry point stays within the argument-count
/// budget (mirroring [`ForegroundRunRequest`]'s role for the foreground path). Every field here is
/// one the ORCHESTRATOR resolves exactly once — the step graph, its run mode, the fork-context
/// session file, the plan-time persona map, and the run-wide `{task}`/`{chain_dir}` substitution
/// values — and hands verbatim to the detached hop-2 runner via `RunnerConfig`. The pieces no
/// caller can supply (the fresh [`RunId`], plus the process-config-derived concurrency / worktree /
/// depth / async-root / results-dir values read from the live `config_snapshot`) are filled in by
/// `spawn_background_steps` itself and are deliberately NOT carried here.
pub struct BackgroundStepsSpec {
    /// The already-resolved step graph to dispatch (`RunnerConfig::steps`).
    pub steps: Vec<RunnerStep>,
    /// How the detached runner drives that graph (`RunnerConfig::mode`).
    pub mode: RunMode,
    /// The fork-context session file the orchestrator resolved once (`RunnerConfig::session_file`);
    /// `None` for a run that starts no session.
    pub session_file: Option<PathBuf>,
    /// The plan-time persona map (`RunnerConfig::resolved_agents`) so hop 2 dispatches each step's
    /// REAL persona rather than re-discovering or falling back to a placeholder.
    pub resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
    /// The run-wide `{task}` value (`RunnerConfig::original_task`) every step's `{task}` resolves to.
    pub original_task: String,
    /// The dedicated per-run scratch directory `{chain_dir}` resolves to (`RunnerConfig::chain_dir`);
    /// `None` for a single top-level task that has no chain dir (`{chain_dir}` → the run cwd).
    pub chain_dir: Option<PathBuf>,
    /// SUBA-N05: the FULLY-RESOLVED live-control config for this run
    /// (`RunnerConfig::control`), already folded from the extension-level `subagents.control` block
    /// and the call's own `control` override by [`crate::exec::control::resolve_control_config`].
    ///
    /// Resolved by the CALLER, parent-side, exactly as upstream does — `runSinglePath` /
    /// `runChainPath` compute `resolveControlConfig(deps.config.control, params.control)` and hand
    /// the resolved object to `executeAsyncSingle`/`executeAsyncChain`
    /// (`subagent-executor.ts:2845,2868` / `:1312-1313` @v0.34.0), and the detached runner reads it
    /// back as `config.controlConfig ?? DEFAULT_CONTROL_CONFIG` (`subagent-runner.ts:1802`). `None`
    /// means "this caller supplied none", which hop 2 degrades to
    /// [`crate::exec::control::ResolvedControlConfig::default`] — pi's identical `??` fallback.
    pub control: Option<crate::exec::control::ResolvedControlConfig>,
    /// SUBA-N06: this run's `includeProgress` flag (`RunnerConfig::include_progress`), carried
    /// verbatim to hop 2 and installed on every step's
    /// [`crate::exec::RunOptions::include_progress`]. `None`/`Some(false)` is pi's default —
    /// R-SA-043 compaction with no per-step progress snapshot.
    pub include_progress: Option<bool>,
    /// SUBA-N03: the run's identity, MINTED BY THE CALLER rather than by `spawn_background_steps`.
    ///
    /// Hoisted for exactly the reason pi hoists its own (`const id = randomUUID();` at
    /// `subagent-executor.ts:3607`, used at `:2861` to build `outputBaseDir` and only then handed
    /// to `executeAsyncSingle(id, …)`): the run-scoped SINGLE-mode output base directory is
    /// `<artifactsDir>/outputs/<runId>`, so a caller that must resolve `params.output` against it
    /// needs the id BEFORE the spawn call, not after.
    ///
    /// `spawn_background_steps` uses this id verbatim — it never mints its own — so the id a
    /// caller resolved paths against is provably the id the run directory, the results file, the
    /// tracker entry, and every child's intercom target are keyed by. [`RunId::new`] is 128 bits
    /// of fresh entropy per call, so two concurrent callers cannot collide on a run-scoped dir;
    /// and the run directory is created by `ensure_accessible_dir` before the config is written,
    /// which is where a collision would surface as an error rather than a silent share.
    pub run_id: RunId,
    /// SUBA-N03: pi `config.timeoutMs` — the nominal run-level timeout budget, carried to hop 2 on
    /// [`crate::background::runner_main::RunnerConfig::timeout_ms`]. `None` = no budget.
    pub timeout_ms: Option<u64>,
    /// SUBA-N03: pi `params.share` (`shareEnabled`), carried to hop 2 on
    /// [`crate::background::runner_main::RunnerConfig::share`].
    pub share: Option<bool>,
    /// SUBA-N03: pi `artifactsDir: artifactConfig.enabled ? artifactsDir : undefined`
    /// (`async-execution.ts:964`) — `None` is how an explicit `artifacts: false` reaches hop 2.
    pub artifacts_dir: Option<PathBuf>,
    /// SUBA-N03: pi `artifactConfig` (`async-execution.ts:965`) — which of the four files each
    /// step's artifact write emits.
    pub artifact_config: crate::artifacts::ArtifactConfig,
    /// SUBA-008: pi `params.turnBudget` on the async path (`async-execution.ts:1050`/`:1469`),
    /// carried to hop 2 on [`crate::background::runner_main::RunnerConfig::turn_budget`] and
    /// applied by the runner to EVERY step (`subagent-runner.ts:1409`).
    ///
    /// Already resolved (caller > agent frontmatter > extension config) by the dispatch site, for
    /// the same reason the foreground path resolves it before building `RunOptions`: hop 2 has no
    /// discovery and no live config to re-derive the chain from.
    pub turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-021 — the run-level USAGE budget, carried to hop 2 on
    /// [`crate::background::runner_main::RunnerConfig::usage_budget`] and applied by the runner to
    /// EVERY step, exactly as [`Self::turn_budget`] is (pi enforces one `usageBudget` across a
    /// whole async run rather than one per step).
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
}

/// G92: the three optional `status` VIEW selectors pi carries as separate params
/// (`extension/schemas.ts:232-237` @v0.34.0), grouped so
/// [`crate::extension::SubagentExecutor::control_status_view`] stays inside the workspace's argument-count lint —
/// they are always resolved together and always come from the same tool call.
#[derive(Clone, Copy, Debug, Default)]
pub struct StatusViewSelector<'a> {
    /// `"fleet"` | `"transcript"`; anything else is refused with pi's `Unknown status view` text.
    pub view: Option<&'a str>,
    /// The transcript tail's line budget (default 80, clamped 1..=500).
    pub lines: Option<i64>,
    /// The child to inspect, for `view: "transcript"`.
    pub index: Option<usize>,
}
