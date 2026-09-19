//! The request/override DTOs callers hand to [`crate::extension::SubagentExecutor`]'s run entry points, plus
//! the `status` view selector.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, ModelId};

use crate::background::{RunId, RunMode};
use crate::discovery::types::AgentReadScope;
use crate::exec::ResolvedAgentPersona;
use crate::fork_context::ContextRequest;
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
    /// SCOPE_19/A1 — the caller's explicit `thinking` level for the child, already lowered at the
    /// tool boundary (`"false"` → `"off"`, `"inherit"` → `None`, anything else validated against
    /// [`crate::watchdog::model_selection::THINKING_LEVELS`]). Rung 2 of the launch resolution
    /// ladder: `run_foreground_impl` folds it ABOVE the persona's own `thinking:` and above the
    /// parent-session rung, and below only a `:level` suffix on a caller-supplied `model` (which
    /// `apply_thinking_suffix` never replaces without a fork override's licence). `None` = the
    /// param was omitted (or said `inherit`), which defers to persona-then-parent-session.
    pub thinking: Option<String>,
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
    /// WORKFLOW_2 — extra entries for the CHILD's environment, threaded straight onto
    /// [`crate::exec::RunOptions::child_env`] (`exec/agent_config.rs:706`).
    ///
    /// This is that field's FIRST production caller: its doc carries a "# Zero production callers,
    /// and why it keeps its place" heading saying in so many words that a value which must reach a
    /// CHILD goes on the CHILD's `Command`, never on this process's environment — which is also
    /// why `clippy.toml` bans `std::env::set_var` outright. `WorkflowRunHost` uses it to set
    /// [`crate::extension::executor::workflow::WORKFLOW_CHILD_ENV`], so a workflow child's own
    /// `subagent` tool can refuse a NESTED `workflowScript`.
    ///
    /// Layered onto `SpawnSpec::env_overlay` and applied FIRST, so the crate's own identity, depth
    /// and child-role entries overwrite it: a caller may ADD to the child's environment, it may
    /// not rewrite the invariants that decide what the child is allowed to do.
    ///
    /// Empty (the `Default`) = "no extra child env", which is what every non-workflow caller
    /// passes and reproduces the pre-WORKFLOW_2 behavior exactly.
    pub child_env: std::collections::HashMap<String, String>,
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

/// The nine inputs one foreground single run needs, bundled into one borrowed request so
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
    /// SUBA-079 — the call-site `context` REQUEST (`fresh`/`fork`/`profile`); `None` takes the
    /// defaults ladder. Distinct from the RESOLVED `ContextMode`; see [`ContextRequest`].
    pub context: Option<ContextRequest>,
    /// Per-call model override (added to the availability set, R-SA-038); `None` inherits.
    pub model_override: Option<ModelId>,
    /// Foreground timeout budget in milliseconds (pi `timeoutMs`/`maxRuntimeMs`); `None` = none.
    pub timeout_ms: Option<u64>,
    /// The host's own cancellation token for this tool call (pi `execute(id, params, signal, ...)`,
    /// `extension/index.ts:498-500`), threaded straight into [`crate::exec::RunOptions::cancel`] so an abort of
    /// the tool call (user Esc / turn abort) drives the running child through the real
    /// SIGINT→SIGTERM→SIGKILL escalation instead of being silently dropped at this seam.
    pub cancel: CancelToken,
    /// WORKFLOW_6 §4.2 — pi `params.workflowParentRunId` (`subagent-executor.ts:2141`, `:3335`) —
    /// the workflow shell that owns this child, stamped onto its
    /// [`crate::extension::executor::notices::ForegroundControlEntry::parent_workflow_run_id`].
    /// `None` on every non-workflow path, which is every caller except
    /// `WorkflowRunHost::launch`.
    pub parent_workflow_run_id: Option<crate::background::RunId>,
    /// WORKFLOW_6 §4.2 — pi `params.workflowKey` — the lane key this child was launched under.
    pub workflow_key: Option<crate::workflows::WorkflowKey>,
    /// WORKFLOW_14 — where this child's steer requests are written and its answers read.
    ///
    /// `Some` exactly when `parent_workflow_run_id` is `Some`: a WORKFLOW child's control root is
    /// the workflow's own run directory (WORKFLOW_13, `extension/tool/routing.rs`'s
    /// `ensure_accessible_dir`), so
    /// [`crate::background::control::step_steer_inbox_dir`] /
    /// [`crate::background::control::steer_acks_dir`] /
    /// [`crate::background::control::steer_capability_path`] have somewhere to be rooted. A plain
    /// foreground SINGLE run has no run directory and keeps `None` — `foreground.rs`'s G90 note
    /// holds for it, and only for it.
    ///
    /// ONE field feeds BOTH halves: `build_foreground_run_options` derives the three
    /// [`crate::exec::RunOptions`] paths the CHILD is spawned against, and
    /// `register_foreground_controls` stores the same handle on the child's
    /// [`crate::extension::executor::foreground_control::ForegroundChildEntry`] so the PARENT can
    /// address it. Deriving both from one value is what makes the two sides incapable of
    /// disagreeing about the index.
    pub workflow_steer:
        Option<crate::extension::executor::foreground_control::ForegroundChildSteerHandle>,
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
    /// SUBA-079 — the call-site `context` REQUEST (`fresh`/`fork`/`profile`); `None` takes the
    /// defaults ladder. Distinct from the RESOLVED `ContextMode`; see [`ContextRequest`].
    pub context: Option<ContextRequest>,
    /// Per-call model override; `None` inherits (pi `async-execution.ts:1290-1295`).
    pub model_override: Option<ModelId>,
    /// SCOPE_19/A1 — the caller's explicit `thinking` level, lowered exactly as
    /// [`SingleRunOverrides::thinking`] is. `spawn_background` stamps it onto the resolved persona
    /// (the same fold `tool_budget` uses), so hop 2's dispatch sees it as that agent's level —
    /// caller beats persona — while the parent-session rung rides
    /// [`crate::background::runner_main::RunnerConfig::inherited_session_thinking`] and is applied
    /// runner-side only below the persona.
    pub thinking: Option<String>,
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
    /// SUBA-073: pi `resolvePermissionRules(ctx.config?.permissions, agentConfig.permissions)` on
    /// the async path, carried to hop 2 on
    /// [`crate::background::runner_main::RunnerConfig::permission_rules`] and applied by the
    /// runner to EVERY step, exactly as [`Self::turn_budget`] is — already resolved by the
    /// dispatch site for the same reason.
    pub permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
    /// SUBA-021 — the run-level USAGE budget, carried to hop 2 on
    /// [`crate::background::runner_main::RunnerConfig::usage_budget`] and applied by the runner to
    /// EVERY step, exactly as [`Self::turn_budget`] is (pi enforces one `usageBudget` across a
    /// whole async run rather than one per step).
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
    /// SCOPE_9 — the run whose active-async capacity slot this spawn TAKES OVER rather than
    /// charging the session a second time for (pi `target.source === "async"` selecting
    /// `transferActiveAsyncCapacity` over `acquireActiveAsyncCapacity`,
    /// `subagent-executor.ts:2085-2098` @v0.68.0).
    ///
    /// `Some(source)` is the RESUME shape, and today has exactly one producer:
    /// [`crate::extension::SubagentExecutor::control_resume`]'s terminal-revival arm
    /// (`revive_from_transcript`), which IS cyrup's whole `target.source == "async"` population —
    /// `control::resume` resolves its target out of the per-cwd ASYNC root and refuses anything it
    /// cannot reconcile there, so every revive is the resume of an async run. A revive that
    /// acquired afresh would charge the cap twice for what the operator sees as one run, and at
    /// `max_active_async_runs_per_session = 1` the source's own still-held slot would make the
    /// revive refuse itself.
    ///
    /// `None` is every ordinary admission. A `Some` whose source holds no slot is NOT an error:
    /// [`crate::background::active_async_capacity::transfer`] falls through to an ordinary
    /// admission there (pi `:513`), which is what a revive of a run whose slot reconciliation
    /// already reclaimed must do.
    pub transfer_from: Option<RunId>,
    /// The thinking ceiling a REVIVE re-applies to its spawn — pi `thinkingCeiling:
    /// recoveryDescriptor?.thinkingCeiling` on the revived launch (`subagent-executor.ts:2151`
    /// @v0.68.0). `None` from every ordinary producer; `Some` only from
    /// [`crate::extension::SubagentExecutor::control_resume`]'s terminal-revival arm, which hands
    /// over the source run's persisted ceiling. `spawn_background_steps` intersects it with THIS
    /// process's own inherited ceiling (a revive never widens, pi `applySteeringRecoveryAgentConfig`
    /// `async-resume.ts:610`) and, only when this is `Some`, writes the result into the detached
    /// runner's env overlay as [`crate::exec::thinking_ceiling::THINKING_CEILING_ENV`] — the
    /// runner already reads that variable, so no `RunnerConfig` field is needed.
    pub thinking_ceiling: Option<String>,
    /// The capability ceiling a REVIVE re-applies to its spawn — pi
    /// `intersectSubagentCapabilityCeilings(target, recoveryDescriptor?.capabilityCeiling,
    /// resolveCurrentSubagentCapabilityCeiling(...))` (`subagent-executor.ts:2183`), landing as
    /// [`crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV`] on the env overlay exactly as
    /// [`Self::thinking_ceiling`] does. `None` from every ordinary producer.
    pub capability_ceiling: Option<crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
    /// The `modelOrigin` a REVIVE carries forward from the source run's recovery descriptor — pi
    /// `modelOrigin: recoveryDescriptor?.modelOrigin` on the revived launch
    /// (`subagent-executor.ts:2149` @v0.68.0), consumed as `storedOrigin` by `resolveModelOrigin`
    /// (`runs/shared/model-resolution.ts:382`: `if (input.storedOrigin) return
    /// input.storedOrigin;`). `None` from every ordinary producer, which derives the origin from
    /// the launch itself; `Some` only from
    /// [`crate::extension::SubagentExecutor::control_resume`]'s terminal-revival arm.
    ///
    /// Read by `spawn_background_steps` when it writes the revived run's OWN descriptor
    /// ([`crate::background::LaunchInputs::stored_model_origin`]) and by nothing else: the
    /// overlay has already pinned the model itself on the persona, so what the origin decides is
    /// the provenance that descriptor records and the slot the model lands on at the NEXT
    /// revive. Without it an `inherited` launch would be re-recorded as `configured` on its
    /// first revive, and a second revive would read a different origin than the first.
    pub model_origin: Option<crate::background::ModelOrigin>,
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
