//! The one-shot `runner-config.json` handoff (func-SA §4.5, arch-SA §4.3, R-SA-073): the
//! [`RunnerConfig`] shape, its delete-then-act consumption ([`read_and_delete_config`]) and the
//! effective run-path resolution. Split out of `background/runner_main.rs`; ports
//! pi `runs/background/subagent-runner.ts`.

use super::entry::run_id_from_paths;
use super::finish::finish_run;
use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus};
use crate::error::SubagentError;
use crate::exec::ResolvedAgentPersona;
use crate::spawn::chain_graph::RunnerStep;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// =================================================================================================
// RunnerConfig — the one-shot handoff file (func-SA §4.5, arch-SA §4.3, R-SA-073)
// =================================================================================================

/// The one-shot `runner-config.json` handoff file's shape (arch-SA §4.3), read exactly once by
/// [`run`](super::run) and deleted immediately afterward (R-SA-073). Every field the orchestrator resolves
/// EAGERLY before spawning hop 2 — including every step's fork-context session-file path
/// (R-SA-137, resolved by [`crate::exec::plan_batch`] and baked into each
/// [`SingleStepSpec::session_file`](crate::spawn::chain_graph::SingleStepSpec::session_file)/[`SingleStepSpec::context`](crate::spawn::chain_graph::SingleStepSpec::context) before this file is ever written)
/// — lives here; the runner process itself never re-derives fork-context, never re-discovers
/// agents, and never re-resolves depth beyond what its own inherited environment
/// ([`crate::spawn::depth::resolve_effective_depth`]) already gives it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerConfig {
    /// This run's identity — MUST match the run id encoded in the `--config` path's own parent
    /// `RunDir` (the caller is responsible for that consistency; this module does not itself
    /// cross-check the two, since the config file's `run_id` is the sole authoritative source
    /// once read).
    pub run_id: RunId,
    /// Which shape of run this is (func-SA §4.5).
    pub mode: RunMode,
    /// The already fully-resolved step list — a flat [`RunnerStep`] sequence for a `Chain` run
    /// (walked via [`walk_chain`](crate::spawn::chain_graph::walk_chain)), or, for a `Single`/`Parallel` top-level run, a list whose
    /// single [`RunnerStep`] is either one [`RunnerStep::SingleStep`] or one
    /// [`RunnerStep::ParallelGroup`] respectively — [`run_inner`](super::turn_loop::run_inner) does not itself branch on `mode`
    /// for step-execution purposes (the difference is purely how `steps` was constructed by the
    /// orchestrator), it only consults `mode` when constructing the initial/terminal
    /// [`RunStatus`]/[`ResultFile`](crate::background::ResultFile) records.
    pub steps: Vec<RunnerStep>,
    /// The working directory every step without its own `cwd` override runs in.
    pub cwd: PathBuf,
    /// The top-level persisted session-transcript path, if this run's context is `Fork` at the
    /// top level (threaded into the terminal [`ResultFile::session_file`](crate::background::ResultFile::session_file), R-SA-085's resume
    /// target).
    pub session_file: Option<PathBuf>,
    /// SUBA-031 — the ORCHESTRATOR session that launched this run, pi's `sessionId:
    /// ctx.currentSessionId` on every detached hand-off (`async-execution.ts:1042`, `:1159`,
    /// `:1459`, `:1542` @v0.43.0), stamped onto `status.json` by the runner
    /// (`...(config.sessionId ? { sessionId: config.sessionId } : {})`, `subagent-runner.ts:2088`)
    /// and read back by every session-scoped listing (`async-status.ts:432`).
    ///
    /// It is carried EXPLICITLY here, exactly as pi carries it, rather than re-derived inside the
    /// runner: the runner's previous source was
    /// [`crate::background::parent_anchor::resolve_parent_session_anchor`], whose register is
    /// published only by `cyrup-permission-system` at its parent-role `SessionStart`. With that
    /// extension absent — a perfectly ordinary configuration — every background run recorded a
    /// `None` session, which a session-scoped listing must drop. `None` here still means "no live
    /// session identity" (headless / SDK embedder), and the runner then falls back to the anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The launching orchestrator PROCESS's identity (pi `completionOwnerId`,
    /// `shared/completion-owner.ts:10-14`), carried through to be stamped onto the run's
    /// `status.json` and terminal `ResultFile`.
    ///
    /// Carried explicitly for the same reason as [`RunnerConfig::session_id`] and one stronger:
    /// the value is meaningless if re-derived here. `current_completion_owner_id()` called inside
    /// the detached runner returns the RUNNER's identity, but the process entitled to consume the
    /// completion is the ORCHESTRATOR. Only the orchestrator can supply it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_owner_id: Option<crate::identity::CompletionOwnerId>,
    /// Run-wide global concurrency ceiling (R-SA-050) — resolved once by the orchestrator from
    /// [`crate::registration::SubagentExtensionConfig::global_concurrency_limit`] and handed
    /// through verbatim rather than re-read from config inside the runner process.
    pub global_concurrency_limit: usize,
    /// Base directory for `worktree: true` group isolation (R-SA-060..064), if any group in
    /// `steps` needs one. `None` is fine for a run with no worktree-isolated group.
    pub worktree_base_dir: Option<PathBuf>,
    /// The depth ceiling this run's own children may inherit (R-SA-054/056) — mirrors the
    /// process's own inherited `CYRUP_SUBAGENT_MAX_DEPTH`, carried here so a runner invoked with
    /// no such env var (e.g. a test harness that only sets `--config`) still gets a sane,
    /// explicit ceiling rather than silently falling back to an unbounded one.
    pub max_subagent_depth: u32,
    /// The run's ABSOLUTE async-root (`<home>/.cyrup/subagents/async/<cwd_key>`), resolved ONCE by
    /// the orchestrator via [`crate::background::run_artifact_roots`] and carried here verbatim so
    /// the detached runner rebuilds its [`RunPaths`] from this exact directory rather than
    /// re-deriving it — the C7 fix. Mirrors pi's `config.asyncDir` (`subagent-runner.ts:1085`).
    ///
    /// Empty (`PathBuf::new()`) means "not supplied by this caller" — only a hand-constructed or
    /// legacy config omits it; [`run`](super::run) then falls back to the caller-derived `run_paths` it was
    /// handed, preserving pre-C7 behavior for such configs. `#[serde(default)]` lets an older
    /// on-disk config without these fields still deserialize.
    #[serde(default)]
    pub async_root: PathBuf,
    /// The run's ABSOLUTE results-dir (`<home>/.cyrup/subagents/results/<cwd_key>`), resolved ONCE
    /// by the orchestrator via [`crate::background::run_artifact_roots`] and carried here verbatim
    /// so the terminal [`ResultFile`](crate::background::ResultFile) is written into the SAME directory the orchestrator created
    /// and watches — the C7 fix (before it, the runner re-derived a divergent, never-created dir
    /// and every real background run's result write failed silently). Mirrors pi's `resultPath`
    /// being passed in the config (`subagent-runner.ts:1077`, `async-execution.ts:650`).
    ///
    /// Empty (`PathBuf::new()`) has the same "fall back to the caller-derived paths" meaning as
    /// [`RunnerConfig::async_root`].
    #[serde(default)]
    pub results_dir: PathBuf,
    /// The fully-resolved persona for every distinct agent named by any step in `steps`, keyed by
    /// the exact [`crate::spawn::chain_graph::SingleStepSpec::agent`] string (T0.1 / C13 fix). The
    /// orchestrator resolves each one ONCE, eagerly, at plan time via
    /// [`crate::exec::resolve_step_agent_config`] (which projects a discovered `AgentDefinition`
    /// into its serializable [`ResolvedAgentPersona`]) and bakes it in here — so the detached
    /// runner's [`ExecSingleStepExecutor`](super::ExecSingleStepExecutor) dispatches the REAL named persona (its own system
    /// prompt, model, fallback ladder, tool allowlist, output spec, completion-guard flag) rather
    /// than the empty-system-prompt / `--model default` / guard-disabled placeholder it previously
    /// synthesized because "the runner has no discovery access". Mirrors pi, where the child always
    /// resolves its agent config from the already-resolved `agents` list handed down to the run,
    /// never re-discovering (`chain-execution.ts:1011`, `parallel-execution.test.ts:134-172`).
    /// This upholds [`RunnerConfig`]'s own "never re-discovers agents" contract: the runner reads
    /// resolved personas, it does not perform discovery.
    ///
    /// `#[serde(default)]` (an empty map) lets an older on-disk config, or a hand-constructed test
    /// config that drives only agents it does not care to fully resolve, still deserialize — a step
    /// whose agent is absent from this map is dispatched as `Unknown agent: <name>` (a step
    /// failure, matching pi's `agents.find` miss), never silently downgraded to a placeholder.
    #[serde(default)]
    pub resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
    /// The chain's overall original task text (pi `originalTask`, `chain-execution.ts:104,536,600` @v0.34.0),
    /// the value every step's `{task}` placeholder resolves to. Resolved ONCE by the orchestrator
    /// (`SubagentExecutor::run_or_background_graph`) from the tool/slash `task` param, else the first
    /// step's first task, and carried here verbatim so the detached hop-2 runner substitutes the SAME
    /// `{task}` value the foreground path does. `#[serde(default)]` (empty) lets an older on-disk
    /// config still deserialize — an empty value keeps `{task}` → `""`.
    #[serde(default)]
    pub original_task: String,
    /// The chain working directory (pi `chainDir`, `chain-execution.ts:654`) that `{chain_dir}`
    /// resolves to. Resolved ONCE by the orchestrator as a dedicated per-run scratch dir under
    /// [`crate::artifacts::chain_runs_dir`] and created before the detached spawn, so the runner
    /// substitutes an already-existing directory. `#[serde(default)]` (`None`) lets an older config
    /// deserialize — `None` keeps `{chain_dir}` → the run cwd.
    #[serde(default)]
    pub chain_dir: Option<PathBuf>,
    /// The launching orchestrator's own intercom presence target (pi `config.controlIntercomTarget`,
    /// `subagent-runner.ts:1823`), resolved ONCE by the orchestrator from
    /// [`crate::extension::SubagentExecutor::orchestrator_intercom_target`] at plan time and carried
    /// verbatim into the detached runner so every step's spawned child activates its
    /// `contact_supervisor` bridge addressed at that supervisor (the detached runner inherits no
    /// useful intercom env, so this is the only channel by which the parent target reaches hop 2).
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize — `None` leaves
    /// each child un-bridged (the clean no-intercom path).
    #[serde(default)]
    pub orchestrator_intercom_target: Option<String>,
    /// The launching orchestrator's live PARENT session model (pi `ctx.model`, `${provider}/${id}`),
    /// resolved ONCE by the orchestrator from
    /// [`crate::extension::SubagentExecutor::inherited_session_model`] at plan time and carried
    /// verbatim into the detached runner so a step whose persona declares no `model:` (and carries no
    /// per-step override) inherits the parent's model — this detached process has NO host-services
    /// backend to read `current_model` from itself, so this config field is the only channel by which
    /// the parent model reaches hop 2. `#[serde(default)]` (`None`) lets an older on-disk config still
    /// deserialize — `None` leaves each inheriting step on its persona's own `model`/`fallback_models`
    /// (the pre-inheritance behavior).
    #[serde(default)]
    pub inherited_session_model: Option<cyrup_core::ModelId>,
    /// SCOPE_19/A1 — the launching orchestrator's live PARENT session reasoning level, the
    /// thinking half of [`Self::inherited_session_model`] and carried for the same reason: this
    /// detached process has NO host-services backend to read `thinking_level` from itself, so this
    /// field is the only channel by which the parent's effort reaches hop 2. Resolved ONCE at plan
    /// time — the caller's explicit `thinking` param when supplied, else
    /// [`crate::extension::SubagentExecutor::remembered_parent_thinking`] — and folded runner-side
    /// as the default BELOW each persona's own `thinking:` (`build_step_agent_config`), so the
    /// resolution order matches the foreground path's. `#[serde(default)]` (`None`) lets an older
    /// on-disk config still deserialize — `None` leaves each step on its persona's own level.
    #[serde(default)]
    pub inherited_session_thinking: Option<String>,
    /// pi `config.hostAvailableBuiltins` (`subagent-runner.ts:702`, read at `:3703`, `:4115`, `:4517`)
    /// — the builtin tool names the LAUNCHING orchestrator's host registry reported, observed ONCE at
    /// plan time by [`crate::exec::tool_surface::host_builtin_tool_names`] and carried verbatim into
    /// the detached runner.
    ///
    /// Carried for the same reason as [`Self::inherited_session_model`] and one stronger: this detached
    /// process has NO host-services backend to observe with, and its own tool registry is NOT the
    /// parent's — re-reading here would answer a different question. This field is the only channel by
    /// which the parent's observation reaches hop 2, and without it every async/background run launches
    /// with `None` and the whole host-availability mechanism (the intersection, the
    /// `unavailableHostBuiltins` diagnostic, the review-lane refusal) is inert for exactly the fan-out
    /// shape it was built for.
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize — `None` is UNKNOWN,
    /// which is the pre-mechanism behaviour.
    #[serde(default)]
    pub host_available_builtins: Option<Vec<String>>,
    /// SUBA-008 — the run-level assistant-TURN budget (pi `params.turnBudget`,
    /// `runs/background/async-execution.ts:165`/`:214`, threaded to the runner as `ctx.turnBudget`,
    /// `subagent-runner.ts:1091`, and from there onto every step's `runSubagentProcess` call at
    /// `:1409`).
    ///
    /// Run-level, NOT per-step, because that is upstream's shape: `AsyncExecutionParams.turnBudget`
    /// is resolved once by the orchestrator and applies to the whole async run — a chain's steps
    /// share one budget rather than each getting a fresh one.
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize, and `None` is
    /// "unbudgeted", which is every run that does not ask for a budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-073 — the run-level, fully-merged permission policy (pi
    /// `resolvePermissionRules(ctx.config?.permissions, agentConfig.permissions)`, resolved once
    /// by the orchestrator — same shape as [`Self::turn_budget`], for the same reason: hop 2 has
    /// neither discovery nor a live extension config to re-derive it from.
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize, and `None` is
    /// "no policy", which is every run that does not resolve one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
    /// SUBA-021 — the run-level USAGE budget (pi `AsyncExecutionParams.usageBudget`,
    /// `runs/background/async-execution.ts:167`/`:216`, carried onto the runner as
    /// `config.usageBudget`, `subagent-runner.ts:172`). Like [`Self::turn_budget`] it is resolved
    /// once by the orchestrator and applies to the WHOLE async run — upstream's own words for the
    /// workflow case are "A workflow usageBudget is enforced once across the workflow".
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize, and `None` is
    /// unbudgeted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
    /// The effective `subagents.modelScope` policy in force for this run (SUBA-003), resolved ONCE
    /// by the orchestrator from its own discovery pass
    /// ([`crate::discovery::AgentDiscoveryResult::model_scope`]) and carried verbatim into the
    /// detached runner.
    ///
    /// This is the ONLY channel by which the policy reaches hop 2: like
    /// [`Self::inherited_session_model`] above, this process has no discovery/settings access by
    /// design, and re-reading `settings.json` here would both violate that contract and risk
    /// enforcing a *different* policy than the one that was on disk when the run was authorized.
    /// pi has no analog to carry — its async path resolves models parent-side in
    /// `async-execution.ts:457` and its own `subagent-runner.ts` never sees a `modelScope` — but
    /// cyrup resolves each step's model inside the runner, so without this field a background run
    /// would be an unpoliced hole in an otherwise-enforced policy. `#[serde(default)]` (`None`)
    /// lets an older on-disk config still deserialize, leaving enforcement off for that run.
    #[serde(default)]
    pub model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    /// The inherited nested-event route (pi `config.nestedRoute`, `async-execution.ts:727,989` @v0.34.0) —
    /// resolved ONCE by the orchestrator from its own inherited env
    /// ([`crate::spawn::nested_events::resolve_inherited_nested_route_from_env`]) and carried here
    /// so a background run started from WITHIN an already-nested run relays its own descendants
    /// through the SAME root route, never re-resolving env itself. `None` means this run is a
    /// top-level (non-nested) run. `#[serde(default)]` lets an older on-disk config still
    /// deserialize.
    #[serde(default)]
    pub nested_route: Option<crate::spawn::nested_events::NestedRoute>,
    /// This run's own resolved ancestry address within `nested_route` (pi `config.nestedSelf`,
    /// `async-execution.ts:728-731,990-993` @v0.34.0) — `None` iff `nested_route` is also `None`.
    /// `#[serde(default)]` lets an older on-disk config still deserialize.
    #[serde(default)]
    pub nested_self: Option<crate::spawn::nested_events::NestedParentAddress>,
    /// The run-wide dynamic-fanout item cap (pi `config.chain.dynamicFanout.maxItems`), resolved
    /// ONCE by the orchestrator (`SubagentExtensionConfig::dynamic_fanout_max_items`) at plan time
    /// and carried here so the detached runner's own [`crate::spawn::chain_graph::ChainRunContext::
    /// dynamic_fanout_max_items`] gets the SAME run-wide cap the foreground path applies — a
    /// background `DynamicGroup` step whose own `expand.maxItems` is absent then falls back to this
    /// value instead of always failing materialization. `#[serde(default)]` (`None`) lets an older
    /// on-disk config still deserialize — `None` keeps the pre-fix "no config cap" behavior.
    #[serde(default)]
    pub dynamic_fanout_max_items: Option<u32>,
    /// SUBA-N05 — pi `config.controlConfig` (`subagent-runner.ts:117,1328` @v0.34.0): the
    /// FULLY-RESOLVED live-control thresholds/channels this run was authorized with.
    ///
    /// Resolved ONCE, parent-side, by the orchestrator
    /// ([`crate::exec::control::resolve_control_config`] over the extension-level
    /// `subagents.control` block plus the call's own `control` override) and carried here verbatim,
    /// exactly as upstream does — `runSinglePath` computes
    /// `resolveControlConfig(deps.config.control, effectiveParams.control)` and passes the RESOLVED
    /// object into `executeAsyncSingle` (`subagent-executor.ts:2845,2868-2870` @v0.34.0), whose runner reads
    /// it back as `config.controlConfig ?? DEFAULT_CONTROL_CONFIG`.
    ///
    /// Parent-side resolution is load-bearing rather than stylistic: this process has no settings
    /// access by design (see [`Self::model_scope`]'s note), so re-resolving here could apply a
    /// *different* `subagents.control` block than the one on disk when the run was authorized.
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize, and is hop 2's
    /// `?? DEFAULT_CONTROL_CONFIG` degrade: control tracking on, with stock thresholds.
    #[serde(default)]
    pub control: Option<crate::exec::control::ResolvedControlConfig>,
    /// SUBA-N06 — this run's `includeProgress` flag (pi `params.includeProgress`,
    /// `extension/schemas.ts:272` @v0.34.0), carried verbatim from the orchestrator and installed
    /// on every dispatched step's [`crate::exec::RunOptions::include_progress`], so each persisted
    /// [`crate::exec::SingleResult`] in the terminal result file carries its own progress snapshot.
    ///
    /// Upstream has no counterpart on this hop: pi never threads `includeProgress` into
    /// `executeAsyncSingle` (`subagent-executor.ts:2845-2874` @v0.34.0) because its async return
    /// is a "started" message with no results attached. cyrup's async run produces a retrievable
    /// `SingleResult`, so the flag has somewhere real to land; dropping it here instead would be
    /// the advertised-and-silently-dropped defect SUBA-041 names.
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize, and is the
    /// pre-existing behaviour: no snapshot, full R-SA-043 compaction.
    #[serde(default)]
    pub include_progress: Option<bool>,
    /// SUBA-N03 — pi `config.timeoutMs` (`subagent-runner.ts:125` @v0.34.0, fed from
    /// `async-execution.ts:982` `timeoutMs: params.timeoutMs`): the NOMINAL run-level timeout
    /// budget in milliseconds this run was started with.
    ///
    /// This is only the figure [`crate::exec::format_timeout_message`] renders into a timed-out
    /// step's error text — the same constant for every step, never a shrinking "time remaining"
    /// value. The instant actually raced against is [`Self::deadline_at_ms`] below. pi keeps the
    /// same two-value split (`timeoutMessage = \`Subagent timed out after ${config.timeoutMs}ms.\``,
    /// `subagent-runner.ts:1339`, vs the `setTimeout(timeoutRunner, config.deadlineAt - Date.now())`
    /// arm at `:2078-2081`).
    ///
    /// `#[serde(default)]` (`None`) lets an older on-disk config still deserialize and is the
    /// pre-SUBA-N03 behaviour: an async run with no wall-clock budget at all.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// SUBA-N03 — pi `config.deadlineAt` (`subagent-runner.ts:126`, fed from
    /// `async-execution.ts:924,983` @v0.34.0 `deadlineAt = Date.now() + params.timeoutMs`): the ABSOLUTE
    /// wall-clock instant this run must be finished by, as milliseconds since the Unix epoch.
    ///
    /// Absolute epoch-milliseconds rather than a `std::time::Instant` for the reason pi's is a
    /// `number`: this value crosses a PROCESS boundary in a JSON file, and `Instant` is an opaque
    /// monotonic reading with no serializable representation and no meaning in another process.
    /// [`run`](super::run) converts it back to a local deadline once, on entry, by subtracting the current
    /// wall clock — pi's own `Math.max(0, config.deadlineAt - Date.now())` (`:2079`) — so time
    /// already burned by the hop-1 spawn and hop-2 startup is charged against the budget rather
    /// than silently refunded.
    ///
    /// `#[serde(default)]` (`None`) = no deadline, the pre-SUBA-N03 behaviour.
    #[serde(default)]
    pub deadline_at_ms: Option<u64>,
    /// SUBA-N03 — pi `config.share` (`subagent-runner.ts` config, fed from `async-execution.ts:965`
    /// `share: shareEnabled`): the run's `share` opt-in, threaded onto every dispatched step's
    /// [`crate::exec::RunOptions::share`].
    ///
    /// Its one load-bearing effect is pi's `sessionEnabled = Boolean(sessionFile || sessionDir) ||
    /// share` term (`execution.ts:1027,1039`, ported at
    /// [`crate::exec::build_attempt_spawn_plan`]): `Some(true)` keeps the child's session store on
    /// where it would otherwise be spawned `--no-session`. `#[serde(default)]` (`None`) is
    /// "omitted", which is NOT an enabling value (pi's term is `options.share === true`).
    #[serde(default)]
    pub share: Option<bool>,
    /// SUBA-N03 — pi `config.artifactsDir` (`subagent-runner.ts:106`, fed from
    /// `async-execution.ts:964` `artifactsDir: artifactConfig.enabled ? artifactsDir : undefined`):
    /// the directory this run's per-step artifact quadruple is written into.
    ///
    /// `None` means "write no artifacts" — pi's own gate is `if (ctx.artifactsDir &&
    /// ctx.artifactConfig?.enabled !== false)` (`subagent-runner.ts:1192`), i.e. an absent dir is
    /// exactly as disabling as `enabled: false`, which is why the orchestrator sets this to `None`
    /// for `artifacts: false`. `#[serde(default)]` (`None`) is therefore also the pre-SUBA-N03
    /// behaviour: before this field existed the hop-2 runner wrote no artifacts at all.
    #[serde(default)]
    pub artifacts_dir: Option<PathBuf>,
    /// SUBA-N03 — pi `config.artifactConfig` (`subagent-runner.ts:107`, fed from
    /// `async-execution.ts:965`): WHICH of the four artifact files each step writes.
    ///
    /// Read together with [`Self::artifacts_dir`] by [`ExecSingleStepExecutor::run_single`](crate::spawn::chain_graph::SingleStepExecutor::run_single), which
    /// gates on `artifacts_dir.is_some() && artifact_config.enabled`, matching pi's own two-term
    /// gate. `#[serde(default)]` is pi's `DEFAULT_ARTIFACT_CONFIG`; the orchestrator sends
    /// [`crate::artifacts::ArtifactConfig::foreground`] so an async run leaves the same full
    /// quadruple (including the `.jsonl` event stream) a foreground run does.
    #[serde(default)]
    pub artifact_config: crate::artifacts::ArtifactConfig,
}

// =================================================================================================
// read_and_delete_config — R-SA-073, delete-then-act idempotency
// =================================================================================================

/// The observable outcome of one [`read_and_delete_config`] call — distinguishes "this call
/// actually read and consumed a fresh config" from "the config file was already gone" so a
/// double-invocation of the runner subcommand against the same `--config` path degrades to a
/// typed, non-panicking outcome rather than crashing (this file's own delete-then-act idempotency
/// obligation, mirroring `control.rs::consume_interrupt_request`'s R-SA-083 contract at the
/// config-handoff layer instead of the interrupt-request layer).
#[derive(Debug)]
pub enum ConfigConsumeOutcome {
    /// The config file existed, parsed successfully, and has now been deleted.
    ///
    /// [`RunnerConfig`] is boxed so this variant does not bloat the whole enum's size to match its
    /// largest member (clippy `large_enum_variant`): the far-more-common `AlreadyConsumed` path
    /// carries no payload, so the config lives behind a single indirection rather than being
    /// stamped inline into every `ConfigConsumeOutcome` value the double-invocation path returns.
    Consumed(Box<RunnerConfig>),
    /// The config file did not exist at all when this call ran — either it was already consumed
    /// by a prior call (double-invocation) or it was never written. Either way, this is NOT
    /// treated as a hard error by [`read_and_delete_config`] itself; the caller ([`run`](super::run)) decides
    /// what a missing config means for its own control flow (in practice: nothing useful can be
    /// done without step data, so [`run`](super::run) surfaces this as a [`SubagentError`] via
    /// [`RunnerConfig`]'s own absence — but the TYPE here stays a plain enum, not a panic, so a
    /// test can assert on this outcome directly without unwinding).
    AlreadyConsumed,
}

/// Read `config_path` as [`RunnerConfig`] JSON, then delete it (R-SA-073: "the runner MUST delete
/// this config file immediately after reading it").
///
/// Read-then-delete (not delete-then-read): the config's CONTENT is what this call exists to
/// obtain, and — unlike `control.rs`'s interrupt-request consumption, where the file's mere
/// *existence* is the entire piece of state being raced over by potentially many concurrent
/// consumers — there is exactly one legitimate reader of a given `runner-config.json` (the one
/// runner process invoked with that exact `--config` path), so there is no concurrent-consumer
/// race to protect against here. What this function DOES guard against is a **double-invocation**
/// of the SAME runner process's own startup path (e.g. a test harness or a supervisor retry
/// re-running `run()` against a config path whose file this process — or an earlier crashed
/// attempt — already consumed): the delete step tolerates the file already being absent
/// (`ErrorKind::NotFound`) as a non-error, silently-absorbed outcome, exactly mirroring
/// `consume_interrupt_request`'s own "duplicate consumption... MUST be silently absorbed, not
/// re-processed" idempotency property, restated here for the config handoff.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the file exists but cannot be read, or exists but fails to
/// parse as valid [`RunnerConfig`] JSON (a malformed config is a genuine anomaly this function
/// surfaces rather than silently treating as "already consumed" — those are two different failure
/// modes and must not be conflated). Never returns an error merely because the file was already
/// absent — that is [`ConfigConsumeOutcome::AlreadyConsumed`], not an `Err`.
pub async fn read_and_delete_config(
    config_path: &Path,
) -> Result<ConfigConsumeOutcome, SubagentError> {
    let bytes = match tokio::fs::read(config_path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigConsumeOutcome::AlreadyConsumed);
        }
        Err(err) => return Err(SubagentError::Spawn(err)),
    };

    let config: RunnerConfig = serde_json::from_slice(&bytes).map_err(|err| {
        SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    })?;

    // Delete immediately after a successful read (R-SA-073). A NotFound here (lost a race against
    // some other process's cleanup, or the file vanished between our read and this delete) is
    // tolerated exactly like `consume_interrupt_request`'s own delete step — we already have the
    // content in hand, so a delete failure of this specific kind changes nothing about what this
    // call returns.
    match tokio::fs::remove_file(config_path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(SubagentError::Spawn(err)),
    }

    Ok(ConfigConsumeOutcome::Consumed(Box::new(config)))
}

/// Read-and-delete the one-shot `runner-config.json` handoff file (R-SA-073), or write this run's
/// terminal `Failed` record and report that there is nothing to run.
///
/// `None` means the failure has ALREADY been captured on disk by [`finish_run`] — the caller's
/// only remaining job is to return, exactly as [`run`](super::run)'s "effectively infallible from the
/// CALLER's point of view" contract requires (never an `Err` propagated past this point).
pub(super) async fn load_runner_config(
    config_path: &Path,
    run_paths: &RunPaths,
) -> Option<RunnerConfig> {
    let outcome = read_and_delete_config(config_path).await;

    match outcome {
        Ok(ConfigConsumeOutcome::Consumed(config)) => Some(*config),
        Ok(ConfigConsumeOutcome::AlreadyConsumed) => {
            // R-SA-073's delete-then-act idempotency, restated at the top level: a double
            // invocation against an already-consumed config has nothing to build a run from.
            // There is no prior in-flight run THIS process instance is aware of (a genuinely
            // resumed/steered run goes through `control::resume`, never a second `run()` call
            // against the same one-shot file) — surface a terminal Failed record so a caller
            // polling this run id sees a definitive, non-hanging outcome rather than silence.
            let run_id = run_id_from_paths(run_paths);
            let status = RunStatus::queued(run_id, RunMode::Single, Some(std::process::id()));
            finish_run(
                run_paths,
                status,
                RunState::Failed,
                Vec::new(),
                PathBuf::new(),
                None,
                "runner-config.json was already consumed (double-invocation of the runner \
                 subcommand against the same --config path); nothing to run"
                    .to_string(),
            )
            .await;
            None
        }
        Err(err) => {
            // No config at all to build even a run-id-bearing status from in the ordinary case —
            // but `run_paths` itself still encodes a run id (its own directory name), so a
            // terminal Failed record can still be synthesized and written, giving any orchestrator
            // watching this run id a definitive answer instead of an indefinitely "Queued" ghost.
            let run_id = run_id_from_paths(run_paths);
            let status = RunStatus::queued(run_id, RunMode::Single, Some(std::process::id()));
            finish_run(
                run_paths,
                status,
                RunState::Failed,
                Vec::new(),
                PathBuf::new(),
                None,
                format!("failed to read runner-config.json: {err}"),
            )
            .await;
            None
        }
    }
}

/// C7: the orchestrator resolved this run's authoritative ABSOLUTE async-root and results-dir
/// (via `crate::background::run_artifact_roots`) and baked them into the config; rebuild `RunPaths` from
/// THOSE roots — never from a re-derivation of the config file's own directory structure — so
/// the terminal ResultFile lands in the SAME directory the orchestrator created and watches.
/// Fall back to the caller-derived `run_paths` only for a (legacy/hand-built) config that
/// carried neither root, preserving pre-C7 behavior for such configs.
///
/// `None` means the config carried neither root and the caller-derived `RunPaths` stands.
pub(super) fn effective_run_paths(config: &RunnerConfig) -> Option<RunPaths> {
    if config.async_root.as_os_str().is_empty() || config.results_dir.as_os_str().is_empty() {
        None
    } else {
        Some(RunPaths::for_run(
            &config.async_root,
            &config.results_dir,
            &config.run_id,
        ))
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

    use super::super::tests::single_step;
    use super::*;
    use crate::background::atomic::write_atomic_json;

    // ---------------------------------------------------------------------------------------
    // read_and_delete_config: R-SA-073 delete-then-act idempotency
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn read_and_delete_config_consumes_and_removes_the_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        let config = RunnerConfig {
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: RunId::from_token("run00001"),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(single_step("worker", "do it"))],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            session_id: None,
            completion_owner_id: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            // Empty roots => `run` falls back to the caller-derived `run_paths` (these unit tests'
            // pre-C7 behavior). The C7 config-driven-rebuild path is exercised end to end in
            // `tests/background_runner_main_integration.rs`.
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            host_available_builtins: None,
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write config");

        let outcome = read_and_delete_config(&cfg_path)
            .await
            .expect("read succeeds");
        match outcome {
            ConfigConsumeOutcome::Consumed(read_back) => assert_eq!(*read_back, config),
            ConfigConsumeOutcome::AlreadyConsumed => panic!("expected Consumed"),
        }

        assert!(
            !tokio::fs::try_exists(&cfg_path)
                .await
                .expect("check exists"),
            "the config file must be deleted immediately after being read (R-SA-073)"
        );
    }

    #[tokio::test]
    async fn read_and_delete_config_double_consume_does_not_panic() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        let config = RunnerConfig {
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: RunId::from_token("run00002"),
            mode: RunMode::Single,
            steps: vec![],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            session_id: None,
            completion_owner_id: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            // Empty roots => `run` falls back to the caller-derived `run_paths` (these unit tests'
            // pre-C7 behavior). The C7 config-driven-rebuild path is exercised end to end in
            // `tests/background_runner_main_integration.rs`.
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            host_available_builtins: None,
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write config");

        let first = read_and_delete_config(&cfg_path)
            .await
            .expect("first read succeeds");
        assert!(matches!(first, ConfigConsumeOutcome::Consumed(_)));

        // The load-bearing idempotency proof this task calls for: a SECOND consume against the
        // now-deleted path must not panic, must not error, and must report AlreadyConsumed.
        let second = read_and_delete_config(&cfg_path)
            .await
            .expect("second read does not error");
        assert!(
            matches!(second, ConfigConsumeOutcome::AlreadyConsumed),
            "a double-consume of the handoff config must degrade to AlreadyConsumed, never panic \
             or re-process: {second:?}"
        );
    }

    #[tokio::test]
    async fn read_and_delete_config_malformed_json_surfaces_as_error_not_already_consumed() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        tokio::fs::write(&cfg_path, b"not valid json")
            .await
            .expect("write garbage");

        let result = read_and_delete_config(&cfg_path).await;
        assert!(
            result.is_err(),
            "a malformed (but PRESENT) config file must surface as a genuine error, distinct \
             from the file simply being absent"
        );
    }
}
