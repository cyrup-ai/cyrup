//! Hop-2 detached-runner main loop (func-SA §5.4 R-SA-073..077/098..103; arch-SA §6.5).
//!
//! This is the single riskiest file in the crate: it is the integration point every other
//! background-subsystem module (`spawn_detached.rs`, `atomic.rs`, `control.rs`, `reconcile.rs`)
//! and the Phase 3 spawn boundary (`spawn/mod.rs`, `spawn/chain_graph.rs`, `spawn/parallel.rs`)
//! all feed into, and it is the ONE place the R-SA-077 "status.json before ResultFile, on EVERY
//! exit path" invariant must hold without exception. `crates/cyrup/src/subagent_runner_cmd.rs`
//! (a sibling crate, outside this one) is the sole caller: it selects the internal
//! `__subagent-runner --config <path>` subcommand and calls [`run`] directly — no separate
//! loader/interpreter hop, since `cyrup` is already one compiled binary.
//!
//! # The main-loop shape (arch-SA §6.5, restated exactly)
//!
//! ```text
//! read+delete config file
//!   -> resolve fork_context is already done (eager, by the orchestrator, R-SA-137) — this loop
//!      only reads the already-resolved session-file path per step, never re-derives it
//!   -> write initial RunStatus{state:Running,pid:self} via atomic.rs        (R-SA-075)
//!   -> spawn a control-inbox watcher task (uses control.rs)                 (R-SA-082)
//!   -> loop {
//!        check interrupted                                                 (R-SA-084)
//!        consume pending append requests via control.rs (re-scan disk)     (R-SA-095/096)
//!        if step cursor exhausted, break
//!        run the next step via the Phase-3 spawn boundary                  (R-SA-045..069)
//!        write status via atomic.rs
//!        advance cursor
//!      }
//!   -> compute terminal state
//!   -> write status.json THEN ResultFile, in that exact order,             (R-SA-077)
//!      on every single exit path (happy path, early return, error branch)
//!   -> exit
//! ```
//!
//! # R-SA-077's ordering invariant is enforced by construction, not by convention
//!
//! Every code path that can end this function's execution — the happy path (steps exhausted),
//! an interrupt (steps paused mid-flight), and an unrecoverable internal error (e.g. the runner
//! config fails to parse) — funnels through exactly one function, [`finish_run`], which performs
//! the `status.json`-write-THEN-`ResultFile`-write sequence unconditionally and returns `()`
//! (never a `Result` a caller could short-circuit past). [`run`] itself has no `return` statement
//! that bypasses `finish_run`: every `?`/early-return branch inside the loop body is caught by an
//! inner `Result`-returning helper ([`run_inner`]) whose own `Err` is turned into a terminal
//! `Failed` status by [`run`]'s own tail, which then always calls `finish_run`. This mirrors this
//! crate's established "no silent bypass of a load-bearing ordering invariant" convention (compare
//! `exec/mod.rs`'s own R-SA-033 post-hoc-correction-must-run-after-completion-guard ordering,
//! enforced the same way: one funnel function, no early return around it).
//!
//! # Delete-then-act idempotency (R-SA-073's config file, mirroring control.rs's own R-SA-083)
//!
//! Reading the one-shot `runner-config.json` handoff file follows the identical delete-then-act
//! discipline `control.rs::consume_interrupt_request` already established for interrupt requests
//! (R-SA-083): the file's *content* is read first (needed to actually build the run), then the
//! file is deleted — and a SECOND call to [`read_and_delete_config`] against an already-consumed
//! config path (the file no longer exists) returns a typed "already consumed" outcome rather than
//! panicking or erroring loudly, so a hypothetical double-invocation of the runner subcommand
//! against the same config path (a supervisor retry, a test harness bug) degrades gracefully
//! instead of crashing. This is NOT the same ordering as `control.rs`'s interrupt consumption
//! (which reads-then-deletes so a lost race against a concurrent consumer still returns the
//! content) — here there is only ever one reader (the one runner process invoked with this exact
//! `--config` path), so a plain "read, then delete, tolerate NotFound on delete" sequence is
//! sufficient and matches R-SA-073's literal text ("the runner MUST delete this config file
//! immediately after reading it").
//!
//! # ResultsDir filesystem-watch completion notification (R-SA-098..103)
//!
//! This module owns only the *runner-side* half of R-SA-098's contract: [`run`] writes the
//! terminal [`super::ResultFile`] into `ResultsDir` as its very last file-writing act (R-SA-077),
//! which is what makes the orchestrator-side watch observable at all. The ORCHESTRATOR-side watch
//! primitive itself (installing a `notify` watcher over the whole `ResultsDir`, deduping by a
//! seen-set with a bounded TTL, R-SA-099, classifying terminal outcomes, R-SA-100, and bounding
//! retry-in-place on processing failure, R-SA-102) runs in the **orchestrator** process — never
//! the detached runner process this file's main loop (`run`) itself executes in — and lives in
//! the sibling module [`crate::background::watch`], per arch-SA §2.2's module layout. See that
//! module's own docs for the full R-SA-098..103 contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::SubagentError;
use crate::exec::{self, AgentConfig, ResolvedAgentPersona, RunOptions, SingleResult};
use crate::fork_context::{ContextMode, ForkContext};
use crate::spawn::chain_graph::{
    ChainRunContext, OutputRegistry, RunnerStep, SingleStepExecutor, SingleStepSpec, StepResult,
    walk_chain,
};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::parallel::GlobalConcurrencyLimit;

use super::atomic::write_atomic_json;
use super::cascade;
use super::control::{self, ChainAppendRequest};
use super::{
    ParallelGroupStatus, ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, StepState,
    StepStatus,
};
use crate::jsonl::BoundedJsonlWriter;

// =================================================================================================
// RunnerConfig — the one-shot handoff file (func-SA §4.5, arch-SA §4.3, R-SA-073)
// =================================================================================================

/// The one-shot `runner-config.json` handoff file's shape (arch-SA §4.3), read exactly once by
/// [`run`] and deleted immediately afterward (R-SA-073). Every field the orchestrator resolves
/// EAGERLY before spawning hop 2 — including every step's fork-context session-file path
/// (R-SA-137, resolved by [`crate::exec::plan_batch`] and baked into each
/// [`SingleStepSpec::session_file`]/[`SingleStepSpec::context`] before this file is ever written)
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
    /// (walked via [`walk_chain`]), or, for a `Single`/`Parallel` top-level run, a list whose
    /// single [`RunnerStep`] is either one [`RunnerStep::SingleStep`] or one
    /// [`RunnerStep::ParallelGroup`] respectively — [`run_inner`] does not itself branch on `mode`
    /// for step-execution purposes (the difference is purely how `steps` was constructed by the
    /// orchestrator), it only consults `mode` when constructing the initial/terminal
    /// [`RunStatus`]/[`ResultFile`] records.
    pub steps: Vec<RunnerStep>,
    /// The working directory every step without its own `cwd` override runs in.
    pub cwd: PathBuf,
    /// The top-level persisted session-transcript path, if this run's context is `Fork` at the
    /// top level (threaded into the terminal [`ResultFile::session_file`], R-SA-085's resume
    /// target).
    pub session_file: Option<PathBuf>,
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
    /// legacy config omits it; [`run`] then falls back to the caller-derived `run_paths` it was
    /// handed, preserving pre-C7 behavior for such configs. `#[serde(default)]` lets an older
    /// on-disk config without these fields still deserialize.
    #[serde(default)]
    pub async_root: PathBuf,
    /// The run's ABSOLUTE results-dir (`<home>/.cyrup/subagents/results/<cwd_key>`), resolved ONCE
    /// by the orchestrator via [`crate::background::run_artifact_roots`] and carried here verbatim
    /// so the terminal [`ResultFile`] is written into the SAME directory the orchestrator created
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
    /// runner's [`ExecSingleStepExecutor`] dispatches the REAL named persona (its own system
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
    /// [`run`] converts it back to a local deadline once, on entry, by subtracting the current
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
    /// Read together with [`Self::artifacts_dir`] by [`ExecSingleStepExecutor::run_single`], which
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
    /// treated as a hard error by [`read_and_delete_config`] itself; the caller ([`run`]) decides
    /// what a missing config means for its own control flow (in practice: nothing useful can be
    /// done without step data, so [`run`] surfaces this as a [`SubagentError`] via
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

// =================================================================================================
// run — the hop-2 main loop entry point
// =================================================================================================

/// Run the hop-2 detached-runner main loop against the one-shot config at `config_path`
/// (R-SA-073..077).
///
/// `run_paths` locates every well-known file this run writes to
/// ([`super::RunPaths::for_run`], resolved by the caller — `crates/cyrup/src/
/// subagent_runner_cmd.rs` — from the config path's own parent directory structure, per
/// func-SA §4.5's fixed `<AsyncRoot>/<run_id>/` layout, and passed in explicitly here rather than
/// re-derived, since this module has no opinion on where `AsyncRoot`/`ResultsDir` themselves live
/// — that is `registration::SubagentExtensionConfig`'s concern).
///
/// This function's own top-level control flow is exactly the shape documented in this module's
/// header comment: read+delete config -> write initial `Running` status -> spawn the control-inbox
/// watcher -> the step loop -> compute terminal state -> [`finish_run`] (status THEN result, on
/// every exit path) -> return. Every fallible step inside is funneled through [`run_inner`] so a
/// SINGLE tail call to [`finish_run`] is the only place either file gets written to its terminal
/// form, regardless of which branch produced that terminal state (steps-exhausted happy path,
/// mid-loop interrupt, or an internal error surfaced by `run_inner` itself).
///
/// # Errors
///
/// This function itself is effectively infallible from the CALLER's point of view — every
/// internal failure is captured into a terminal `Failed` [`RunStatus`]/[`ResultFile`] pair rather
/// than propagated as a `Result::Err`, since there is no one left to hand an `Err` to once this
/// process is the detached runner (R-SA-078: "the orchestrator MUST NOT assume a live IPC channel
/// to the runner" — there is no return channel for a `Result` to travel back through). The
/// `Result` return type exists purely so `crates/cyrup/src/subagent_runner_cmd.rs` can log a
/// diagnostic and choose its own process exit code; it carries NO information this function's own
/// on-disk writes have not already durably recorded.
pub async fn run(config_path: &Path, run_paths: &RunPaths) -> Result<(), SubagentError> {
    let outcome = read_and_delete_config(config_path).await;

    let config = match outcome {
        Ok(ConfigConsumeOutcome::Consumed(config)) => *config,
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
            return Ok(());
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
            return Ok(());
        }
    };

    // C7: the orchestrator resolved this run's authoritative ABSOLUTE async-root and results-dir
    // (via `super::run_artifact_roots`) and baked them into the config; rebuild `RunPaths` from
    // THOSE roots — never from a re-derivation of the config file's own directory structure — so
    // the terminal ResultFile lands in the SAME directory the orchestrator created and watches.
    // Fall back to the caller-derived `run_paths` only for a (legacy/hand-built) config that
    // carried neither root, preserving pre-C7 behavior for such configs.
    let effective_paths;
    let run_paths: &RunPaths = if config.async_root.as_os_str().is_empty()
        || config.results_dir.as_os_str().is_empty()
    {
        run_paths
    } else {
        effective_paths = RunPaths::for_run(&config.async_root, &config.results_dir, &config.run_id);
        &effective_paths
    };

    // ensureAccessibleDir-equivalent on the RUNNER side (C7's "create the dirs on both sides"):
    // guarantee the run dir (parent of every intermediate status/events write) and the results dir
    // (parent of the terminal ResultFile) both exist up front. `finish_run` re-ensures the results
    // dir as a final guard on every exit path, but creating them here keeps the happy-path
    // status/events writes from failing on a missing directory too.
    let _ = super::ensure_accessible_dir(&run_paths.run_dir).await;
    if let Some(results_dir) = run_paths.result.parent() {
        let _ = super::ensure_accessible_dir(results_dir).await;
    }

    // R-SA-075: initial status.json (state=Running, pid=own pid), written BEFORE any step work.
    let mut status = RunStatus::queued(config.run_id.clone(), config.mode, Some(std::process::id()));
    // pi `...(config.sessionId ? { sessionId: config.sessionId } : {})` (`subagent-runner.ts:2088`):
    // stamp the ORCHESTRATOR session onto the run's own `status.json`, so a later reader can scope
    // the async root to one session (`async-status.ts:432`). cyrup's `RunnerConfig` carries no
    // session id, but the hop-1 spawn already writes the anchor into this process's environment
    // (`parent_anchor::detached_runner_env_overlay`), which is the same value pi's `ctx
    // .currentSessionId` is (`async-execution.ts:1042`).
    status.session_id = crate::background::parent_anchor::resolve_parent_session_anchor();
    status.chain_step_count = Some(config.steps.len());
    status.steps = config
        .steps
        .iter()
        .map(pending_step_status_for)
        .collect();
    // Queued -> Running is always legal (RunState::can_transition_to).
    if status.advance_state(RunState::Running).is_err() {
        // Unreachable in practice (a freshly `queued` status can always advance to Running), but
        // this crate never unwraps a Result — if the transition guard were ever tightened in a
        // way that made this fail, degrade to a terminal Failed record rather than panicking.
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            "internal error: Queued -> Running transition was rejected".to_string(),
        )
        .await;
        return Ok(());
    }
    if let Err(err) = write_atomic_json(&run_paths.status, &status).await {
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            format!("failed to write initial status.json: {err}"),
        )
        .await;
        return Ok(());
    }

    // Install a SIGUSR2 handler BEFORE anything else that could race an interrupt delivery
    // (R-SA-081's wake-up signal, sent by `control::deliver_wakeup_signal`): on both Linux and
    // macOS, SIGUSR2's default disposition is process TERMINATION. Without an installed handler,
    // the very act of a caller trying to softly interrupt this run would instead kill the runner
    // outright — the opposite of R-SA-084's "interrupt is soft, not fatal" guarantee, and the
    // interrupt would never even reach `run_inner`'s own cooperative `interrupted` check. The
    // signal's payload itself is not consulted for anything: `control::watch_control_inbox`'s
    // filesystem-notification mechanism (installed by `spawn_control_watcher` immediately below)
    // is the actual, authoritative "an interrupt/append request landed" signal per DI-SA-9
    // (file-based control, never live IPC) — SIGUSR2 exists purely to nudge that watcher/poll
    // loop awake sooner than its next scheduled tick, so this handle's only job is to keep
    // existing for this function's whole lifetime (held via `_sigusr2_guard`) so the OS routes
    // the signal to a registered handler instead of applying its default terminate action; a
    // received signal is otherwise fully drained/ignored.
    #[cfg(unix)]
    let _sigusr2_guard = install_ignored_sigusr2_handler();

    // The control-inbox directory (`<run_dir>/control/`) MUST exist before
    // `spawn_control_watcher` installs its `notify::PollWatcher` below: that watcher targets the
    // DIRECTORY, not the (not-yet-existing, created-on-first-interrupt) file itself, since
    // watching a not-yet-existing file path is unreliable across platforms (see
    // `control::watch_control_inbox`'s own doc). Watching a directory that does not exist YET
    // fails to install at all on every platform this crate ships to — and `spawn_control_watcher`
    // degrades that failure to a silent no-op (by design, so a watcher failure never strands the
    // run), which would silently make EVERY interrupt delivered after this point unobservable:
    // `run_inner`'s own per-iteration re-check only re-scans pending chain-append requests
    // (R-SA-096), it has no independent interrupt-file poll fallback of its own — the `interrupted`
    // flag is set SOLELY by this watcher task. Creating the directory here, unconditionally,
    // before the watcher is installed, closes that gap.
    //
    // This MUST route through `finish_run` on failure, matching every other pre-loop fallible step
    // immediately above (never a bare `?`, found bypassing `finish_run` entirely in second-pass
    // adversarial review): a bare `?` here would return `Err` straight out of `run` itself, leaving
    // `status.json` permanently stuck at the `Running` record already written above and NO
    // `ResultFile` ever written — directly contradicting this function's own documented "effectively
    // infallible from the caller's point of view" contract (every internal failure captured into a
    // terminal on-disk record, never propagated) and silently violating R-SA-077's ordering
    // invariant by skipping BOTH writes rather than merely reordering them.
    if let Err(err) = tokio::fs::create_dir_all(
        run_paths
            .control_inbox
            .parent()
            .unwrap_or(&run_paths.run_dir),
    )
    .await
    {
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            format!("failed to create control-inbox directory: {err}"),
        )
        .await;
        return Ok(());
    }

    // R-SA-082: control-inbox watcher, installed with the mandatory synchronous startup check
    // performed FIRST (catches a request written in the race window before the watcher attaches),
    // then a background task forwarding every watch notification into `interrupted`.
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // R-SA-084 mid-flight interrupt (`subagent-runner.ts:1583-1609`): the run-wide SHARED soft-
    // interrupt token. The control-inbox watcher cancels it the instant an interrupt lands, which
    // tears down whatever child is running RIGHT NOW (via `run_sync`'s `opts.interrupt` race)
    // rather than only being noticed between steps — the difference between actually stopping a
    // single long-running step's child and a no-op. `ExecSingleStepExecutor` clones this same token
    // into every dispatched step's `RunOptions::interrupt`.
    let interrupt_cancel = cyrup_core::CancelToken::new();
    // The second control-inbox verb (`control/timeout.json`, pi `TimeoutRequest`): an ancestor
    // whose own deadline expired cascades one of these into every live descendant's inbox, and it
    // gets the identical synchronous-startup-check-then-watch treatment the interrupt flag does,
    // for the identical reason — a request written in the race window before the watcher attaches
    // must not be missed.
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // G77 — the THIRD control-inbox verb (`control/stop.json`, pi `StopRequest`): an explicit
    // user/agent stop, or an ancestor's stop cascaded down. Same mandatory
    // synchronous-startup-check-then-watch treatment as the other two, for the same reason.
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if control::check_stop_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        interrupt_cancel.cancel();
    }
    if control::check_timeout_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
        interrupt_cancel.cancel();
    }
    if control::check_control_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
        // An interrupt already pending at startup (written in the race window before the watcher
        // attaches) must likewise pre-cancel the shared token so the very first dispatched step's
        // child is torn down mid-flight, not merely noticed after it finishes.
        interrupt_cancel.cancel();
    }
    let control_flags = ControlFlags {
        interrupted: Arc::clone(&interrupted),
        timed_out: Arc::clone(&timed_out),
        stopped: Arc::clone(&stopped),
    };
    // R-SA-136/146: open the size-capped `events.jsonl` writer for this run, via the SAME shared
    // `BoundedJsonlWriter` primitive `spawn::SpawnedChild`'s per-attempt child-output tee uses
    // (`jsonl.rs`'s own module doc names this exact call site as one of its two intended writers).
    // A failure to open it (e.g. an unwritable run directory) degrades to `None` — `append_event`
    // then silently no-ops on every call — rather than failing this run over a best-effort
    // diagnostic log, mirroring every other non-`status.json`/`ResultFile` write in this function.
    let mut events = BoundedJsonlWriter::create(&run_paths.events).await.ok();
    append_event(
        &mut events,
        "subagent.run.started",
        Some(serde_json::json!({ "runId": config.run_id.as_str() })),
    )
    .await;

    // The run's overall start (for `durationMs` on the terminal run event, pi's
    // `runEndedAt - overallStartTime`), captured before `status` is moved into the shared handle.
    let overall_started_at = status.started_at;

    // Move the initial `Running` status into the shared handle BOTH the step loop and the live-
    // telemetry pump mutate (pi's single `statusPayload`, folded from the per-child event handler
    // AND the 1s `activityTimer`, `subagent-runner.ts:1962`).
    let shared_status: SharedStatus = Arc::new(std::sync::Mutex::new(status));

    // R-SA-082's watcher is installed HERE rather than immediately after the two synchronous
    // startup checks above, because G90's steer routing needs the shared status handle (it accepts
    // a steer only against a currently-`Running` step and records the acceptance on that step). The
    // synchronous startup checks stay where they were — they are what closes the pre-watcher race
    // window, and nothing between them and this line can deliver a control request.
    let _watcher_task = spawn_control_watcher(
        run_paths.clone(),
        control_flags.clone(),
        interrupt_cancel.clone(),
        Arc::clone(&shared_status),
    );

    // The live-telemetry channel: each dispatched step's `RunOptions::live_events` sink forwards
    // raw child NDJSON lines here (tagged with the step's flat index); the telemetry task folds
    // each into the addressed step's `StepTelemetry` + the top-level roll-ups and writes
    // status.json on both a per-event AND a 1s cadence.
    let (telemetry_tx, telemetry_rx) = tokio::sync::mpsc::unbounded_channel::<TelemetryMsg>();
    let telemetry_task =
        spawn_telemetry_task(run_paths.clone(), Arc::clone(&shared_status), telemetry_rx);

    // The step loop itself, all failure modes funneled to a single Result the tail below always
    // routes through `finish_run`.
    let loop_outcome = run_inner(
        &config,
        run_paths,
        &shared_status,
        &control_flags,
        &interrupt_cancel,
        telemetry_tx,
        &mut events,
    )
    .await;

    // `run_inner` has returned, so its executor (holding the last live-telemetry sender) is dropped
    // and the telemetry task observes all-senders-dropped and finishes — await it so no late
    // telemetry status write races the terminal record `finish_run` writes.
    let _ = telemetry_task.await;

    let duration_ms = (super::now_epoch_millis_pub() - overall_started_at).max(0);
    let run_id_str = config.run_id.as_str().to_string();
    let (terminal_state, results, final_error) = match loop_outcome {
        Ok(LoopOutcome::Completed { results }) => {
            let all_ok = results.iter().all(|r| r.exit_code == 0);
            append_event(
                &mut events,
                "subagent.run.completed",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "status": if all_ok { "complete" } else { "failed" },
                    "durationMs": duration_ms,
                })),
            )
            .await;
            (
                if all_ok { RunState::Complete } else { RunState::Failed },
                results,
                None,
            )
        }
        Ok(LoopOutcome::Interrupted { results }) => {
            append_event(
                &mut events,
                "subagent.run.paused",
                Some(serde_json::json!({ "runId": run_id_str })),
            )
            .await;
            (RunState::Paused, results, None)
        }
        Ok(LoopOutcome::TimedOut { results, message }) => {
            // pi `subagent.run.timed_out` (`subagent-runner.ts:2053-2060` @v0.34.0), carrying both
            // the nominal budget and the absolute deadline so a reader can tell a run that used
            // its whole budget from one an ancestor cut short.
            append_event(
                &mut events,
                "subagent.run.timed_out",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "timeoutMs": config.timeout_ms,
                    "deadlineAt": config.deadline_at_ms,
                    "message": message,
                    "durationMs": duration_ms,
                })),
            )
            .await;
            (RunState::Failed, results, Some(message))
        }
        Ok(LoopOutcome::Stopped { results, message }) => {
            // G77 — pi `subagent.run.stopped` (`subagent-runner.ts:2977-2982` @v0.43.0), carrying
            // the stop message so a reader of `events.jsonl` sees WHY the run ended without having
            // to reconstruct it from the terminal record. `durationMs` follows the same shape the
            // sibling terminal events use here.
            append_event(
                &mut events,
                "subagent.run.stopped",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "message": message,
                    "durationMs": duration_ms,
                })),
            )
            .await;
            (RunState::Stopped, results, Some(message))
        }
        Err(err) => {
            append_event(
                &mut events,
                "subagent.run.completed",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "status": "failed",
                    "durationMs": duration_ms,
                    "error": err.to_string(),
                })),
            )
            .await;
            (RunState::Failed, Vec::new(), Some(err.to_string()))
        }
    };

    // Recover the final live status (its accumulated per-step telemetry + workflow-graph snapshot)
    // so the terminal `status.json` `finish_run` writes preserves everything the pump accumulated.
    let final_status = lock_status(&shared_status).clone();

    finish_run(
        run_paths,
        final_status,
        terminal_state,
        results,
        config.cwd.clone(),
        config.session_file.clone(),
        final_error.unwrap_or_default(),
    )
    .await;

    Ok(())
}

/// R-SA-136/146: append one JSON-shaped line to this run's `events.jsonl` via the shared
/// [`BoundedJsonlWriter`] primitive, if a writer was successfully opened for this run (`events` is
/// `None` only when [`BoundedJsonlWriter::create`] itself failed at startup — see [`run`]'s own
/// construction site — in which case this is a silent no-op, matching this crate's established
/// "a `.jsonl` artifact's own failure never fails the run" convention, restated here at the
/// writer-availability level rather than only the per-line byte-cap level
/// [`BoundedJsonlWriter::write_line`] already enforces internally).
///
/// `kind` is a short, stable event-type tag (`"run.started"`, `"step.started"`, `"step.completed"`,
/// `"run.paused"`, `"run.completed"`) mirroring the shape [`super::tracker`]'s own tailing-consumer
/// doc comment and test fixtures already assume for this file (one JSON object per line, a `kind`
/// field identifying the event). `detail` is folded into the same JSON object as additional fields
/// when present, so a consumer never has to parse a nested string-encoded sub-document.
async fn append_event(
    events: &mut Option<BoundedJsonlWriter>,
    event_type: &str,
    detail: Option<serde_json::Value>,
) {
    let Some(writer) = events.as_mut() else {
        return;
    };
    let mut object = serde_json::Map::new();
    // Field name `type` (NOT `kind`) + `subagent.*` event-type strings, matching pi's
    // `events.jsonl` shape exactly (`subagent-runner.ts` `appendJsonl(eventsPath, { type: … })`).
    object.insert(
        "type".to_string(),
        serde_json::Value::String(event_type.to_string()),
    );
    object.insert(
        "ts".to_string(),
        serde_json::Value::from(super::now_epoch_millis_pub()),
    );
    if let Some(serde_json::Value::Object(fields)) = detail {
        for (key, value) in fields {
            object.insert(key, value);
        }
    }
    let line = serde_json::Value::Object(object).to_string();
    // A write failure here (genuine I/O error while still under the byte cap — the cap itself is
    // always a silent no-op, never an `Err`) is likewise never allowed to fail the run: this event
    // log is a best-effort diagnostic/tailing aid (R-SA-093), not part of R-SA-077's authoritative
    // status.json/ResultFile durability contract.
    let _ = writer.write_line(&line).await;
}

/// Best-effort recovery of a [`RunId`] from `run_paths`' own `run_dir` path (its final component
/// is always the run id, per [`RunPaths::for_run`]'s construction) — used only on the
/// no-config-available error paths above, where no [`RunnerConfig::run_id`] exists to read.
fn run_id_from_paths(run_paths: &RunPaths) -> RunId {
    run_paths
        .run_dir
        .file_name()
        .map(|name| RunId::from_token(name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| RunId::from_token("unknown-run"))
}

/// A freshly declared, `Pending` [`StepStatus`] for one [`RunnerStep`] — the agent name shown is
/// the step's own agent for a [`RunnerStep::SingleStep`], or a synthesized `"<n> parallel
/// tasks>"`-shaped label for a group step (whose own per-child detail lives in
/// `RunStatus::parallel_groups`, not this top-level `steps` list's single entry for the group).
fn pending_step_status_for(step: &RunnerStep) -> StepStatus {
    match step {
        RunnerStep::SingleStep(spec) => StepStatus::pending(spec.agent.clone()),
        RunnerStep::ParallelGroup(group) => {
            StepStatus::pending(format!("<parallel:{} tasks>", group.steps.len()))
        }
        RunnerStep::DynamicGroup(dynamic) => {
            StepStatus::pending(format!("<dynamic:{}>", dynamic.collect))
        }
        RunnerStep::ImportAsyncRoot(spec) => StepStatus::pending(spec.agent.clone()),
    }
}

/// The agent name shown for one [`RunnerStep`] in a `subagent.step.*` `events.jsonl` line — the
/// step's own agent for a single/import step, or a synthesized group label (mirroring
/// [`pending_step_status_for`]'s own display convention).
fn step_display_agent(step: &RunnerStep) -> String {
    match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ImportAsyncRoot(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!("<parallel:{} tasks>", group.steps.len()),
        RunnerStep::DynamicGroup(dynamic) => format!("<dynamic:{}>", dynamic.collect),
    }
}

/// The elapsed wall-clock milliseconds of the step at `flat_index`, from its recorded
/// `started_at`/`ended_at` (pi's `taskEndTime - taskStartTime` on a `subagent.step.*` event).
/// `0` when either timestamp is missing.
fn step_elapsed_ms(status: &RunStatus, flat_index: usize) -> i64 {
    status
        .steps
        .get(flat_index)
        .and_then(|s| s.started_at.zip(s.ended_at))
        .map(|(start, end)| (end - start).max(0))
        .unwrap_or(0)
}

/// Recompute + embed this run's workflow-graph snapshot (pi's `refreshWorkflowGraph`,
/// `subagent-runner.ts:2160-2192`) from the current step list + live per-step statuses, so any
/// `status.json` reader always sees a graph consistent with the run's current progress.
fn refresh_workflow_graph(status: &mut RunStatus, steps: &[RunnerStep]) {
    let graph = super::workflow_graph_from_run(steps, status);
    status.telemetry.workflow_graph = Some(graph);
}

// =================================================================================================
// run_inner — the step loop itself
// =================================================================================================

/// The step loop's own outcome, BEFORE `run`'s tail maps it into a terminal [`RunState`] — kept
/// distinct from a bare `Vec<SingleResult>` so the interrupted-vs-completed distinction (R-SA-084:
/// `Paused`, never `Failed`) survives without `run_inner` itself needing to know how its caller
/// will map either variant onto [`RunState`].
enum LoopOutcome {
    /// The step cursor was exhausted without an interrupt — every step in `results` ran to its
    /// own completion (success or failure; `run`'s tail decides `Complete` vs. `Failed` overall
    /// from `results`' own exit codes).
    Completed { results: Vec<SingleResult> },
    /// An interrupt was observed and consumed before the step cursor was exhausted — `results`
    /// holds every step that DID complete before the interrupt landed; steps that never got to
    /// run are left `Pending` in `status.steps` (R-SA-084: "mark every currently-running step
    /// Paused... before signaling its own actively-spawned child subprocess(es)" — this phase has
    /// no live child to signal mid-step since interrupts are only checked BETWEEN steps, see this
    /// function's own doc note on that scope boundary).
    Interrupted { results: Vec<SingleResult> },
    /// A wall-clock deadline expired — either this run's own (`config.deadline_at_ms`, observed as
    /// a step whose child was killed by the deadline) or an ancestor's, delivered as a
    /// `control/timeout.json` request (pi `timeoutRunner`, `subagent-runner.ts:2987-3025`
    /// @v0.34.0).
    ///
    /// Deliberately NOT folded into `Interrupted`: an interrupt is a resumable pause (`Paused`,
    /// every unfinished step `Paused`, `resume` can pick it up), whereas a timeout is terminal
    /// failure (`Failed`, `timedOut`, every unfinished step `Failed` with the timeout message,
    /// nothing to resume). Collapsing the two would make an expired deadline look resumable and
    /// leave a permanently-`Paused` record nothing ever revisits.
    TimedOut {
        results: Vec<SingleResult>,
        /// The message stamped onto the run's terminal error and every step it failed.
        message: String,
    },
    /// G77 — an explicit stop request (`control/stop.json`, pi `StopRequest`) was observed and
    /// consumed: pi `stopRunner` (`subagent-runner.ts:2955-2984` @v0.43.0). `results` holds every
    /// step that DID complete before the stop landed.
    ///
    /// Deliberately NOT folded into `Interrupted` or `TimedOut`. Against `Interrupted`: a stop is
    /// terminal and `resume` MUST refuse it (`async-resume.ts:406`), where a pause is exactly what
    /// `resume` exists for. Against `TimedOut`: the terminal `state` is `Stopped`, not `Failed`, and
    /// every downstream reader — the notify status word (`notify.ts:210`), the grouped intercom
    /// verdict (`result-intercom.ts:84-87`), the `status` action's `State:` line
    /// (`run-status.ts:478-479`) — prints a different word for it. Collapsing either way loses a
    /// user-visible distinction upstream maintains at four separate sites.
    Stopped {
        results: Vec<SingleResult>,
        /// The message stamped onto the run's terminal error and every step it stopped — always
        /// the request's own `reason` when it carried one, else [`control::STOP_MESSAGE`].
        message: String,
    },
}

/// The two control-inbox verbs' pending flags, shared between the watcher task that SETS them and
/// the step loop that consumes them. Bundled into one struct rather than passed as two loose
/// `Arc<AtomicBool>`s so adding the timeout verb did not push [`run_inner`] past clippy's argument
/// ceiling — and so the pair stays visibly a pair (they are always created, cloned and read
/// together, and the loop's ordering between them is load-bearing).
#[derive(Clone)]
struct ControlFlags {
    /// A `control/interrupt.json` is pending (soft, resumable pause).
    interrupted: Arc<std::sync::atomic::AtomicBool>,
    /// A `control/timeout.json` is pending (terminal deadline failure).
    timed_out: Arc<std::sync::atomic::AtomicBool>,
    /// G77 — a `control/stop.json` is pending (terminal, non-resumable explicit stop). The THIRD
    /// verb, checked before the other two everywhere the three are drained together, matching pi's
    /// own inbox order (`runs/background/control-channel.ts:653-655` @v0.43.0: `consumeStopRequest` → then
    /// `consumeTimeoutRequest` → then `consumeInterruptRequest`) and `stopRunner`'s own
    /// `if (stopped || timedOut || interrupted || state !== "running") return` mutual exclusion
    /// (`subagent-runner.ts:2955-2986`).
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

// =================================================================================================
// Shared status handle + live-telemetry pump (pi `subagent-runner.ts:1430-1581`)
// =================================================================================================

/// The one [`RunStatus`] both the step loop ([`run_inner`], lifecycle transitions) and the live
/// telemetry task ([`spawn_telemetry_task`], per-child-event folds) mutate — a plain
/// `std::sync::Mutex` written atomically to `status.json` via [`write_shared_status`]. Every
/// critical section against it is a short, synchronous read-modify-write with no `.await` held
/// across the guard (the atomic file write clones under the lock, then writes with the lock
/// released), mirroring `background/tracker.rs`'s identical `std::sync::Mutex` discipline.
type SharedStatus = Arc<std::sync::Mutex<RunStatus>>;

/// One raw child NDJSON line, tagged with the flat step index it belongs to, sent from a dispatched
/// step's [`crate::exec::RunOptions::live_events`] sink to the runner's telemetry task.
pub(crate) struct TelemetryMsg {
    /// The flat index of the step whose child produced this line.
    flat_index: usize,
    /// The raw NDJSON line, exactly as read from the child's stdout.
    raw: String,
}

/// Lock the shared status, recovering the guard on a poisoned mutex rather than propagating the
/// panic (the map's contents stay structurally valid), matching `background/tracker.rs`.
fn lock_status(shared: &SharedStatus) -> std::sync::MutexGuard<'_, RunStatus> {
    shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Atomically write the current shared status to `status.json` (R-SA-076): clone under the lock,
/// then write with the lock RELEASED so no `std::sync::Mutex` guard is ever held across the `.await`.
async fn write_shared_status(run_paths: &RunPaths, shared: &SharedStatus) -> std::io::Result<()> {
    let snapshot = lock_status(shared).clone();
    write_atomic_json(&run_paths.status, &snapshot).await
}

/// Spawn the live-telemetry pump (pi's `updateStepFromChildEvent` per-event fold +
/// `activityTimer`'s 1s cadence, `subagent-runner.ts:1430-1581`): drains raw child NDJSON lines off
/// `rx`, parses each into a [`crate::exec::ndjson::SubagentEvent`], folds it into the addressed
/// step's live [`crate::background::StepTelemetry`] plus the top-level roll-ups, and writes
/// `status.json` — both per event AND on a 1s timer — so a reader watching the file sees
/// `currentTool`/`recentTools`/token telemetry advance live during the run. The task ends when every
/// telemetry sender is dropped (the step loop finished and released the executor), which the caller
/// awaits BEFORE writing the terminal record so no late telemetry write races the terminal
/// `status.json`.
fn spawn_telemetry_task(
    run_paths: RunPaths,
    shared: SharedStatus,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<TelemetryMsg>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                message = rx.recv() => {
                    let Some(TelemetryMsg { flat_index, raw }) = message else {
                        break; // every sender dropped — the run's step loop has finished
                    };
                    let Some(event) = crate::exec::ndjson::parse_line(&raw) else {
                        continue; // R-SA-026: a non-event line is tolerated, never fatal
                    };
                    {
                        let mut status = lock_status(&shared);
                        let now = super::now_epoch_millis_pub();
                        if let Some(step) = status.steps.get_mut(flat_index) {
                            crate::background::apply_child_event_to_step(step, &event, now);
                        }
                        status.telemetry.last_activity_at = Some(now);
                        status.sync_top_level_telemetry(flat_index);
                    }
                    let _ = write_shared_status(&run_paths, &shared).await;
                }
                _ = ticker.tick() => {
                    // pi's 1s `activityTimer` cadence: re-flush the current live status so a reader
                    // sees a fresh `lastUpdate` even during a quiet stretch between child events.
                    {
                        let mut status = lock_status(&shared);
                        status.touch();
                    }
                    let _ = write_shared_status(&run_paths, &shared).await;
                }
            }
        }
    })
}

/// Drive the step-execution loop itself (R-SA-076 write-ordering per iteration, R-SA-084 interrupt
/// check, R-SA-095/096 append-request consumption, dispatch via the Phase-3 spawn boundary).
///
/// # Interrupt-check granularity (a deliberate, documented scope boundary)
///
/// This loop re-checks `interrupted` BETWEEN steps (the "should I even START the next step" gate),
/// but a step's OWN live child is ALSO interruptible mid-flight: the run-wide shared
/// [`cyrup_core::CancelToken`] (`interrupt_cancel`) is threaded into every dispatched step's
/// [`exec::RunOptions::interrupt`] (see [`ExecSingleStepExecutor::run_single`]), so when the
/// control-inbox watcher cancels it the in-flight child is torn down via the normal
/// `exec::run_sync` -> `SpawnedChild::terminate` signal-escalation path (R-SA-036/059/084) rather
/// than merely noticed after the step finishes — and an interrupted step ends the run `Paused`.
///
/// # Errors
///
/// Returns `Err` only for a genuine I/O failure writing `status.json` mid-loop (R-SA-076) — a
/// single step's own failure (nonzero exit, timeout, etc.) is NOT an `Err` here; it is recorded as
/// a `SingleResult` with a nonzero `exit_code` and the loop continues to the next step exactly as
/// R-SA-052's chain-walk semantics dictate (a chain does not abort on one step's failure unless
/// the group itself is `fail_fast`, which `walk_chain`/`run_bounded` already enforce internally).
async fn run_inner(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    status: &SharedStatus,
    flags: &ControlFlags,
    interrupt_cancel: &cyrup_core::CancelToken,
    telemetry: tokio::sync::mpsc::UnboundedSender<TelemetryMsg>,
    events: &mut Option<BoundedJsonlWriter>,
) -> Result<LoopOutcome, SubagentError> {
    let interrupted = &flags.interrupted;
    let timed_out = &flags.timed_out;
    // G77 — the stop flag, read at the very top of every loop iteration ahead of the other two.
    let stopped = &flags.stopped;
    let mut steps = config.steps.clone();
    let mut cursor = 0usize;
    let mut results: Vec<SingleResult> = Vec::new();
    let mut registry = OutputRegistry::new();

    let depth = crate::spawn::depth::resolve_effective_depth(config.max_subagent_depth);

    // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST in this loop's own setup — before
    // any step's discovery-free-but-still-real worktree setup (`chain_graph::assign_worktree_cwds`
    // -> `spawn::worktree::setup_worktree_group`, which shells out to real `git` subprocesses) or
    // any child OS process is spawned for ANY step in this run's chain. This hop-2 runner process
    // is itself already one recursion hop deep (its own `depth.current_depth` reflects however
    // many ancestors spawned it, propagated via `CYRUP_SUBAGENT_DEPTH`/`_MAX_DEPTH`, R-SA-054) —
    // if that inherited envelope is already at its ceiling, this run must reject EVERY one of its
    // configured steps up front rather than dispatching the first one and only then discovering
    // `ExecSingleStepExecutor::run_single` -> `exec::run_sync`'s own independent re-check rejects
    // it (which would still be correct per R-SA-055's letter for that one step, since no spawn
    // would have happened yet, but would incorrectly leave every LATER step in `steps` looking
    // like it was simply never reached rather than explicitly blocked, and would run any
    // `worktree: true` group's real `git worktree add` setup for nothing before the per-child
    // dispatch inside `run_bounded` ever reached `run_sync`'s own guard). Failing the whole run
    // here, before the loop even starts, keeps the rejection uniform across every step shape
    // (`SingleStep`/`ParallelGroup`/`DynamicGroup`) and guarantees zero worktrees and zero child
    // processes are ever created for a run whose own depth is already exhausted.
    if crate::spawn::depth::is_blocked(&depth) {
        return Err(SubagentError::DepthExceeded {
            current: depth.current_depth,
            max: depth.max_depth,
        });
    }

    let global_limit = GlobalConcurrencyLimit::new(config.global_concurrency_limit.max(1));
    let cancel_root = cyrup_core::CancelToken::new();
    // T0.1 / C13: the per-agent resolved-persona map the orchestrator baked into the one-shot
    // config is threaded straight into the executor so every dispatched step runs its REAL named
    // persona (never re-discovered, never a placeholder). `Arc`-shared so a parallel/dynamic
    // group's fanned-out children share one map rather than cloning it per child.
    let resolved_agents = Arc::new(config.resolved_agents.clone());
    // Published just before each dispatch so the live-telemetry sink tags every child NDJSON line
    // with the step it belongs to (pi's `statusPayload.currentStep`, `subagent-runner.ts:1434`).
    let current_flat_index = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor: Arc<dyn SingleStepExecutor> = Arc::new(ExecSingleStepExecutor {
        depth,
        interrupted: Arc::clone(interrupted),
        interrupt_cancel: interrupt_cancel.clone(),
        current_flat_index: Arc::clone(&current_flat_index),
        telemetry: Some(telemetry),
        resolved_agents,
        // Intercom child-bridge (pi `subagent-runner.ts:779-783`): the orchestrator's presence target
        // + this run's id, carried in the one-shot config, so every step's spawned child activates
        // its `contact_supervisor` bridge addressed at the launching supervisor.
        orchestrator_intercom_target: config.orchestrator_intercom_target.clone(),
        run_id: Some(config.run_id.clone()),
        // Session-model inheritance (pi `ctx.model`): the live parent session model the orchestrator
        // captured at plan time, carried through the one-shot config (this detached process has no
        // host-services backend of its own), so an inheriting step resolves the parent's model.
        inherited_session_model: config.inherited_session_model.clone(),
        // SUBA-003: the model-scope policy the orchestrator authorized this run under, carried in
        // the one-shot config for the same reason as the two fields above — this process performs
        // no discovery and reads no settings.
        model_scope: config.model_scope.clone(),
        // SUBA-N05 (pi `const controlConfig = config.controlConfig ?? DEFAULT_CONTROL_CONFIG`,
        // `subagent-runner.ts:1802` @v0.34.0): the live-control config resolved parent-side and
        // carried in the one-shot config, so an async run's `control` override genuinely changes
        // the thresholds each step's child stream is judged against instead of being dropped.
        control: config.control.clone(),
        // SUBA-N06: R-SA-043 compaction's opt-out, carried the same way and for the same reason.
        include_progress: config.include_progress,
        // SUBA-N03: the run's `share` opt-in and artifact destination/selection, carried from the
        // one-shot config so an async run honours `share`/`artifacts` and leaves the same artifact
        // quadruple a foreground run does (pi `subagent-runner.ts:879-890,1117-1125` @v0.34.0).
        share: config.share,
        artifacts_dir: config.artifacts_dir.clone(),
        artifact_config: config.artifact_config,
        // G90 (pi `subagent-runner.ts:2313,2600,2797` @v0.34.0): the async run dir every
        // dispatched step derives its own `steer-targets/<flatIndex>/` inbox from. This is the
        // detached hop-2 runner, so it is exactly the process upstream gives `steerInboxDir` to.
        run_dir: Some(run_paths.run_dir.clone()),
    });
    // SUBA-N03 — pi `subagent-runner.ts:2078-2081`: `const remainingMs = Math.max(0,
    // config.deadlineAt - Date.now())`. The orchestrator stamped an ABSOLUTE epoch deadline into
    // the one-shot config; convert it back to a local `Instant` ONCE here, charging the elapsed
    // hop-1 spawn + hop-2 startup time against the budget rather than refunding it. An
    // already-passed deadline collapses to `now` (`max(0, …)`), so the first step is refused
    // immediately instead of the subtraction wrapping into a far-future instant.
    //
    // This replaces a hardcoded `None` justified as "R-SA-036: background runs have no built-in
    // wall-clock timeout". That remains true of the DEFAULT — `timeout_ms`/`deadline_at_ms` are
    // `None` unless the caller asked for a timeout — but it was never a reason to DROP an explicit
    // one, and upstream has always honoured `timeoutMs` on the async path (`schemas.ts:265-266`
    // and `tool-description.ts:25,:73` @v0.34.0 both say it applies to "foreground and
    // async/background runs"; `async-execution.ts:1302-1305` arms the deadline).
    let deadline_at = config.deadline_at_ms.map(|deadline_ms| {
        let remaining_ms = deadline_ms.saturating_sub(crate::background::now_epoch_ms());
        std::time::Instant::now() + std::time::Duration::from_millis(remaining_ms)
    });
    let ctx = ChainRunContext {
        cwd: config.cwd.clone(),
        deadline_at,
        // The NOMINAL budget, rendered into a timed-out step's message and never re-derived per
        // step (pi's `timeoutMessage = \`Subagent timed out after ${config.timeoutMs}ms.\``,
        // `subagent-runner.ts:1339`).
        timeout_ms: config.timeout_ms,
        cancel: cancel_root.clone(),
        global_limit,
        worktree_base_dir: config.worktree_base_dir.clone(),
        // The registry (shared across every one-step `walk_chain` call in the loop below) carries
        // the rolling `{previous}` text, so step-to-step piping works even though each `walk_chain`
        // invocation walks a single step. `{task}`/`{chain_dir}` resolve from the run-wide values the
        // orchestrator serialized into the one-shot config (A: pi `originalTask`/`chainDir`), so the
        // detached runner substitutes the SAME values the foreground `/chain` path does.
        original_task: config.original_task.clone(),
        chain_dir: config.chain_dir.clone(),
        // C16 / pi `config.chain.dynamicFanout.maxItems`: the orchestrator resolves this ONCE at
        // plan time (`config_snapshot().dynamic_fanout_max_items()`) and bakes it into the one-shot
        // `RunnerConfig`, so a background dynamic-fanout step whose own `expand.maxItems` is absent
        // falls back to the SAME run-wide cap the foreground path applies, rather than always
        // failing materialization.
        dynamic_fanout_max_items: config.dynamic_fanout_max_items,
    };

    loop {
        // G77 — STOP is checked before BOTH of the others, matching pi's own inbox-drain order
        // (`runs/background/control-channel.ts:653-655` @v0.43.0: `consumeStopRequest` → `consumeTimeoutRequest` →
        // `consumeInterruptRequest`) and `stopRunner`'s mutual-exclusion guard
        // (`subagent-runner.ts:2955-2986`: `if (stopped || timedOut || interrupted || …) return`). The
        // order is load-bearing when several land together: a stop outranks a timeout outranks an
        // interrupt, so the terminal record is always the hardest, least-resumable verdict.
        //
        // Unlike the interrupt arm below there is no `cursor < steps.len()` moot-signal guard: a
        // stop that lands after the last step finished still ends the run `Stopped` upstream
        // (`stopRunner` only checks `statusPayload.state === "running"`, which is still true until
        // `finish_run` writes the terminal record), and — unlike the interrupt case that guard
        // exists for — that is not a downgrade to a permanently-wrong non-terminal record: `Stopped`
        // IS terminal, so nothing is left waiting to be resumed.
        if stopped.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(request) = control::consume_stop_request(run_paths).await? {
                let message = request
                    .reason
                    .clone()
                    .unwrap_or_else(|| control::STOP_MESSAGE.to_string());
                {
                    let mut guard = lock_status(status);
                    let s = &mut *guard;
                    mark_remaining_stopped(s, cursor, steps.len(), &message);
                    refresh_workflow_graph(s, &steps);
                    s.touch();
                }
                write_shared_status(run_paths, status)
                    .await
                    .map_err(SubagentError::Spawn)?;
                // pi `stopNestedAsyncDescendants()` (`subagent-runner.ts:2984`) — stop the whole
                // subtree, not just this run, or every background run this one spawned keeps going
                // detached and unreachable after the user asked for it to stop.
                cascade_to_descendants(config, events, cascade::CascadeVerb::Stop).await;
                promote_interrupted_results_to_stopped(&mut results, &message);
                return Ok(LoopOutcome::Stopped { results, message });
            }
            // Same idempotent absorption the other two arms document: a watch notification with
            // nothing actually pending clears the flag rather than looping.
            stopped.store(false, std::sync::atomic::Ordering::SeqCst);
        }

        // Timeout is checked BEFORE interrupt, matching pi's own inbox-drain order
        // (`runs/background/control-channel.ts:654-655` @v0.43.0: `if (consumeTimeoutRequest(...)) onTimeout();`
        // then `if (consumeInterruptRequest(...)) onInterrupt();`). The order is load-bearing when
        // both land together — an ancestor that timed out cascades a timeout to this run while a
        // user may simultaneously be interrupting it, and the terminal record must be the harder
        // of the two verdicts (`Failed`/timed-out, not a resumable `Paused`).
        if timed_out.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(request) = control::consume_timeout_request(run_paths).await? {
                let message = request.reason.clone().unwrap_or_else(|| {
                    timeout_message(config.timeout_ms, &request.source)
                });
                {
                    let mut guard = lock_status(status);
                    let s = &mut *guard;
                    mark_remaining_timed_out(s, cursor, steps.len(), &message);
                    refresh_workflow_graph(s, &steps);
                    s.touch();
                }
                write_shared_status(run_paths, status)
                    .await
                    .map_err(SubagentError::Spawn)?;
                // Fail the whole subtree, not just this run — see `background::cascade`.
                cascade_to_descendants(config, events, cascade::CascadeVerb::Timeout).await;
                return Ok(LoopOutcome::TimedOut { results, message });
            }
            // Same idempotent absorption the interrupt branch below documents: a watch
            // notification with nothing actually pending clears the flag rather than looping.
            timed_out.store(false, std::sync::atomic::Ordering::SeqCst);
        }

        // R-SA-084: check interrupted FIRST, before consuming appends or dispatching further
        // work — an interrupt that lands must stop new-step dispatch as soon as this loop next
        // observes it, not after one more (possibly append-extended) step has already started.
        //
        // Race guard (found in second-pass adversarial review): a natural completion and an
        // interrupt delivery can land in the same instant — `interrupt()` reads `status.json` and
        // sees `state: Running` (which stays true right up until `finish_run` writes the terminal
        // record), so it can successfully write a control-inbox request and set `interrupted` in
        // the tiny window AFTER this loop's last step already finished (`cursor` already advanced
        // past the final index) but BEFORE this loop's next top-of-iteration check. Without the
        // `cursor < steps.len()` guard below, that late, moot interrupt would still be consumed
        // and reported as `LoopOutcome::Interrupted`, downgrading a run whose every step actually
        // completed into a non-terminal `Paused` `ResultFile` (`success: false`) with no step left
        // to resume from — a permanently-wrong terminal record, since nothing ever reconciles a
        // `Paused` run back to `Complete` after the fact. Only treat the interrupt as a genuine
        // pause when there is still unstarted/unfinished step work for it to actually pause;
        // otherwise silently absorb it (matching R-SA-083's own "duplicate/stale signal MUST be
        // silently absorbed" idempotency principle, applied here to a signal that is stale relative
        // to the run's own already-finished work rather than stale relative to a prior consumption)
        // and let the loop fall through to its normal `Completed` exit on this same iteration.
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) && cursor < steps.len() {
            if let Some(request) = control::consume_interrupt_request(run_paths).await? {
                {
                    let mut guard = lock_status(status);
                    let s = &mut *guard;
                    mark_remaining_paused(s, cursor, steps.len());
                    refresh_workflow_graph(s, &steps);
                    s.touch();
                }
                write_shared_status(run_paths, status)
                    .await
                    .map_err(SubagentError::Spawn)?;
                let _ = request; // consumed; contents already reflected via status/event log.
                // R-SA-084 stops THIS run; without the cascade every background run this one
                // spawned would keep running, detached and unreachable — see `background::cascade`.
                cascade_to_descendants(config, events, cascade::CascadeVerb::Interrupt).await;
                return Ok(LoopOutcome::Interrupted { results });
            }
            // The watcher observed a notification but a synchronous re-check found nothing
            // pending (already consumed by a race, or a stale wake-up) — R-SA-083's idempotent
            // absorption, restated here: clear the flag and keep going rather than looping forever
            // treating a one-shot notification as sticky.
            interrupted.store(false, std::sync::atomic::Ordering::SeqCst);
        }

        // R-SA-095/096: consume pending append requests EVERY iteration, before checking whether
        // the step cursor is exhausted — re-scans disk (never trusts the in-memory `steps` list as
        // the source of truth for what is pending), per R-SA-096's explicit "MUST re-scan disk,
        // not cache" requirement.
        let pending = control::list_pending_appends(&run_paths.append_dir).await?;
        if !pending.is_empty() {
            for (path, parsed) in pending {
                if let Some(request) = parsed {
                    let mut guard = lock_status(status);
                    append_steps(&mut steps, &mut guard, &request);
                }
                // Delete-then-act, at-most-once (R-SA-095: "MUST list, read, and DELETE all
                // pending request files... and only then extend its own in-loop step list").
                let _ = tokio::fs::remove_file(&path).await;
            }
            let pending_count = control::count_pending_appends(&run_paths.append_dir).await?;
            {
                let mut guard = lock_status(status);
                let s = &mut *guard;
                s.pending_appends = Some(pending_count);
                s.chain_step_count = Some(steps.len());
                refresh_workflow_graph(s, &steps);
                s.touch();
            }
            write_shared_status(run_paths, status)
                .await
                .map_err(SubagentError::Spawn)?;
        }

        if cursor >= steps.len() {
            return Ok(LoopOutcome::Completed { results });
        }

        let step = steps
            .get(cursor)
            .cloned()
            .ok_or_else(|| SubagentError::Spawn(std::io::Error::other("step cursor out of range")))?;

        // Publish the current flat index BEFORE dispatch so the live-telemetry sink tags this
        // step's child NDJSON lines with the right index (pi `statusPayload.currentStep = flatIndex`).
        current_flat_index.store(cursor, std::sync::atomic::Ordering::SeqCst);

        {
            let mut guard = lock_status(status);
            let s = &mut *guard;
            mark_step_running(s, cursor);
            s.current_step = Some(cursor);
            refresh_workflow_graph(s, &steps);
            s.touch();
        }
        write_shared_status(run_paths, status)
            .await
            .map_err(SubagentError::Spawn)?;
        append_event(
            events,
            "subagent.step.started",
            Some(serde_json::json!({
                "runId": config.run_id.as_str(),
                "stepIndex": cursor,
                "agent": step_display_agent(&step),
            })),
        )
        .await;

        // R-SA-097 root attachment (chain-root-attachment.ts): an `ImportAsyncRoot` step is NOT
        // dispatched by spawning a child — it is synthesized by POLLING another already-launched
        // run's terminal files (mirroring pi's `runSingleStep` short-circuit `if (step.importAsyncRoot)`,
        // `subagent-runner.ts:1153`). Intercept it here, before the `walk_chain` dispatch, so the
        // runner "calls the poll" (`control::wait_for_imported_async_root`) rather than routing it
        // through the `SingleStepExecutor` spawn seam that would (correctly) have no idea how to run
        // it.
        if let RunnerStep::ImportAsyncRoot(spec) = &step {
            let target_run_id = RunId::from_token(spec.run_id.clone());
            let target_paths =
                RunPaths::for_run(&spec.async_root, &spec.results_dir, &target_run_id);
            let imported = control::wait_for_imported_async_root(
                &target_paths,
                &spec.run_id,
                spec.index,
                &spec.agent,
                control::ROOT_ATTACHMENT_POLL_INTERVAL,
            )
            .await?;

            let step_result = StepResult {
                success: imported.success,
                structured_output: imported.structured_output.clone(),
                final_output: Some(imported.output.clone()),
                error: imported.error.clone(),
                interrupted: false,
                // An IMPORTED async root's control events belong to the run that was attached, and
                // are already recorded on ITS own terminal `ResultFile` — `ImportedAsyncRootResult`
                // deliberately carries only the identity/output fields
                // `imported_root_to_single_result` reproduces, so there is nothing to re-attribute
                // here (matching pi's `runSingleStep`, `subagent-runner.ts:1162-1181`).
                control_events: Vec::new(),
                // Same reasoning for the per-child detail fields: an imported root's real exit code
                // is carried on `ImportedAsyncRootResult` and reproduced by
                // `imported_root_to_single_result`; an `ImportAsyncRoot` step can never be a
                // dynamic-fanout child, so no collect record ever reads these.
                exit_code: None,
                timed_out: false,
                saved_output_path: None,
                artifact_paths: None,
            };
            // Register the imported output under its named key (pi's `outputName`/`as`) so a later
            // `{outputs.name}` reference in this chain resolves to it — a validated structured
            // output when present, otherwise the imported text (R-SA-053).
            if let Some(name) = &spec.output {
                let value = imported
                    .structured_output
                    .clone()
                    .unwrap_or_else(|| serde_json::Value::String(imported.output.clone()));
                registry.register(name.clone(), value);
            }

            let step_duration_ms;
            {
                let mut guard = lock_status(status);
                let s = &mut *guard;
                record_step_outcome(s, cursor, &step, &step_result, None);
                step_duration_ms = step_elapsed_ms(s, cursor);
                refresh_workflow_graph(s, &steps);
                s.touch();
            }
            append_event(
                events,
                if step_result.success {
                    "subagent.step.completed"
                } else {
                    "subagent.step.failed"
                },
                Some(serde_json::json!({
                    "runId": config.run_id.as_str(),
                    "stepIndex": cursor,
                    "agent": step_display_agent(&step),
                    "exitCode": i32::from(!step_result.success),
                    "durationMs": step_duration_ms,
                })),
            )
            .await;
            results.push(imported_root_to_single_result(spec, &imported));

            write_shared_status(run_paths, status)
                .await
                .map_err(SubagentError::Spawn)?;

            cursor += 1;
            continue;
        }

        // Dispatch via the Phase-3 spawn boundary (chain_graph::walk_chain over a ONE-element
        // graph for this single cursor position — reusing the exact same SingleStep/ParallelGroup/
        // DynamicGroup dispatch `walk_chain` already implements, rather than re-implementing group
        // fan-out inline here). `ChainGraph` is a plain `Vec<RunnerStep>` type alias, so the
        // one-element "graph" is just a fresh one-element `Vec`.
        let one_step: Vec<RunnerStep> = vec![step.clone()];
        let (step_results, group_results) =
            walk_chain(&one_step, &mut registry, &executor, &ctx).await?;

        let step_result = step_results.into_iter().next().ok_or_else(|| {
            SubagentError::Spawn(std::io::Error::other(
                "walk_chain produced no result for a single dispatched step",
            ))
        })?;

        // R-SA-084 mid-flight interrupt (`subagent-runner.ts:1583-1609`): a step whose child was
        // signalled and torn down mid-flight (the shared `interrupt_cancel` token this run threaded
        // into `RunOptions::interrupt` fired) is the pause point — the run ends `Paused`, never
        // `Complete`, even though an interrupted `run_sync` reports a paused-success (exit 0).
        let interrupted_mid_flight = step_result.interrupted;
        let step_duration_ms;
        {
            let mut guard = lock_status(status);
            let s = &mut *guard;
            record_step_outcome(s, cursor, &step, &step_result, group_results.first());
            if interrupted_mid_flight {
                // `record_step_outcome` marked this step `Complete` (paused-success exits 0);
                // override it (and every not-yet-run later step) to `Paused` per R-SA-084.
                if let Some(entry) = s.steps.get_mut(cursor) {
                    entry.status = StepState::Paused;
                    entry.error = None;
                }
                mark_remaining_paused(s, cursor + 1, steps.len());
            }
            step_duration_ms = step_elapsed_ms(s, cursor);
            refresh_workflow_graph(s, &steps);
            s.touch();
        }
        let event_type = if interrupted_mid_flight {
            "subagent.step.paused"
        } else if step_result.success {
            "subagent.step.completed"
        } else {
            "subagent.step.failed"
        };
        append_event(
            events,
            event_type,
            Some(serde_json::json!({
                "runId": config.run_id.as_str(),
                "stepIndex": cursor,
                "agent": step_display_agent(&step),
                "exitCode": if interrupted_mid_flight { 0 } else { i32::from(!step_result.success) },
                "durationMs": step_duration_ms,
            })),
        )
        .await;
        results.push(step_result_to_single_result(&step, &step_result));

        write_shared_status(run_paths, status)
            .await
            .map_err(SubagentError::Spawn)?;

        if interrupted_mid_flight {
            // Disambiguate WHICH verb tore this child down. Both share one cancellation token, so
            // `step_result.interrupted` is true for a timeout too — and returning `Interrupted`
            // here would end a timed-out run as a resumable `Paused`, silently swallowing the
            // deadline. A still-pending `control/timeout.json` means it was the timeout: fall
            // through to the top of the loop WITHOUT advancing the cursor and let the timeout
            // branch there produce the terminal record (it re-marks this same step `Failed`,
            // overwriting the `Paused` marking just applied, since `Paused` is not terminal).
            // Checked against the file rather than the flag so a stale flag can never spin this
            // loop: only this branch's own consumption removes the file.
            //
            // G77: the STOP inbox is probed before the timeout inbox, for the identical reason and
            // in pi's identical order (`runs/background/control-channel.ts:653-655`). All THREE verbs share the one
            // cancellation token, so `step_result.interrupted` is equally true for a stop — and
            // returning `Interrupted` here would end an explicitly-stopped run as a resumable
            // `Paused`, which is exactly the bug this gap is about. Falling through without
            // advancing the cursor lets the loop-top stop branch produce the terminal record; it
            // re-marks this same step `Stopped`, overwriting the `Paused` marking just applied,
            // because `Paused` is not terminal.
            if control::check_stop_inbox_now(run_paths).await?.is_some() {
                continue;
            }
            if control::check_timeout_inbox_now(run_paths).await?.is_some() {
                continue;
            }
            // Consume the interrupt request file (idempotent) so it is not left dangling on the run
            // dir, then end the run `Paused` — the child was already torn down mid-flight.
            let _ = control::consume_interrupt_request(run_paths).await;
            cascade_to_descendants(config, events, cascade::CascadeVerb::Interrupt).await;
            return Ok(LoopOutcome::Interrupted { results });
        }

        // A step whose child was killed by the wall clock means the RUN-WIDE deadline
        // (`config.deadline_at_ms`, converted to `ctx.deadline_at` once above) has passed — it is
        // not a per-step budget, so every remaining step would be born already over its deadline.
        // pi ends the whole run here (`timeoutRunner` marks the run `failed`/`timedOut` and fails
        // every still-running-or-pending step, `subagent-runner.ts:2029-2062` @v0.34.0) rather than
        // marching the cursor through steps that cannot succeed. This is the ORIGIN of the timeout
        // cascade: the run whose own deadline expired is what turns a bounded background run into
        // a bounded background SUBTREE.
        if step_result.timed_out {
            let message = timeout_message(config.timeout_ms, "deadline");
            {
                let mut guard = lock_status(status);
                let s = &mut *guard;
                mark_remaining_timed_out(s, cursor + 1, steps.len(), &message);
                refresh_workflow_graph(s, &steps);
                s.touch();
            }
            write_shared_status(run_paths, status)
                .await
                .map_err(SubagentError::Spawn)?;
            cascade_to_descendants(config, events, cascade::CascadeVerb::Timeout).await;
            return Ok(LoopOutcome::TimedOut { results, message });
        }

        cursor += 1;
    }
}

/// Mark every step from `from_index` (inclusive) through `total` as `Paused` with an end
/// timestamp (R-SA-084: "mark every currently-running step Paused... with an end timestamp"),
/// including the step at `from_index` itself (the one that was `Running` — or about to be — at
/// the moment the interrupt was observed) — steps strictly before `from_index` are left however
/// [`record_step_outcome`] already left them (their own genuine terminal/paused state from having
/// actually run), and steps at or after `from_index` that were never even started are likewise
/// moved out of `Pending` into `Paused` rather than left looking like they simply never got a
/// turn, since R-SA-084 does not distinguish "was mid-flight" from "was about to start" for the
/// purpose of this marking.
/// pi's `timeoutMessage` (`subagent-runner.ts:1339` @v0.34.0): `Subagent timed out after
/// ${config.timeoutMs}ms.`, falling back to the bare `"Subagent timed out."` upstream's
/// `timeoutRunner` uses when no nominal budget is known — which is exactly the ancestor-cascade
/// case, where the deadline that expired belonged to a different run and this one has no
/// `timeout_ms` of its own to name.
fn timeout_message(timeout_ms: Option<u64>, source: &str) -> String {
    match timeout_ms {
        Some(ms) => format!("Subagent timed out after {ms}ms."),
        None if source == "ancestor-timeout" => {
            "Subagent timed out: an ancestor run's deadline expired.".to_string()
        }
        None => "Subagent timed out.".to_string(),
    }
}

/// The timeout counterpart of [`mark_remaining_paused`] (pi `timeoutRunner`'s step sweep,
/// `subagent-runner.ts:2029-2067` @v0.34.0): every step from `from_index` that is not already
/// terminal becomes `Failed` with the timeout `message` and an end timestamp.
///
/// `Failed`, not `Paused`, is the whole point — see [`LoopOutcome::TimedOut`]. A reader must be
/// able to tell "this run stopped and can be resumed" from "this run ran out of time and is over".
fn mark_remaining_timed_out(
    status: &mut RunStatus,
    from_index: usize,
    total: usize,
    message: &str,
) {
    let now = super::now_epoch_millis_pub();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Failed;
            step.error = Some(message.to_string());
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Failed;
                        child.error = Some(message.to_string());
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

/// G77 — re-label the child that was torn down BY the stop as `stopped` rather than `interrupted`.
///
/// cyrup drives a step's live child off one shared [`cyrup_core::CancelToken`] for every control
/// verb, so a child killed by a stop comes back from `run_sync` carrying `interrupted: true` — the
/// same shape an actual `interrupt` produces. pi does not have that ambiguity because it hands the
/// child two separate abort controllers, and it resolves the child's own record against the STOP
/// signal specifically: `const stoppedAfterAcceptance = finalResult?.stopped === true ||
/// ctx.stopSignal?.aborted === true;` … `stopped: stoppedAfterAcceptance ? true :
/// finalResult?.stopped` (`subagent-runner.ts:1642,1722` @v0.43.0). This function is that same
/// promotion, applied at the one place cyrup knows the stop signal is what fired.
///
/// The rest of each promoted field follows `runSubagent`'s own stopped-result shape
/// (`subagent-runner.ts:1937-4576` @v0.43.0): `exitCode: 1`, `error: stopMessage`, and — only when the
/// child produced no output of its own — `finalOutput: stopMessage`. Children that had ALREADY
/// completed before the stop landed are untouched, exactly as upstream leaves them (their records
/// settled while `stopSignal.aborted` was still false).
fn promote_interrupted_results_to_stopped(results: &mut [SingleResult], message: &str) {
    for result in results.iter_mut().filter(|r| r.interrupted) {
        result.interrupted = false;
        result.stopped = true;
        result.exit_code = 1;
        result.error = Some(message.to_string());
        if result
            .final_output
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
        {
            result.final_output = Some(message.to_string());
        }
    }
}

/// G77 — the STOP counterpart of [`mark_remaining_timed_out`]/[`mark_remaining_paused`], ported
/// from pi `stopRunner`'s own step sweep (`subagent-runner.ts:2955-2986` @v0.43.0):
///
/// ```text
/// for (const step of statusPayload.steps) {
///     if (step.status !== "running" && step.status !== "pending") continue;
///     step.status = "stopped";
///     step.error = stopMessage;
///     step.exitCode = 1;
///     step.stopped = true;
///     …
/// }
/// ```
///
/// Three differences from the timeout sweep, all upstream's:
/// * the terminal step status is [`StepState::Stopped`], never `Failed` — a reader must be able to
///   tell "someone stopped this" from "this crashed";
/// * upstream sweeps EVERY `running`-or-`pending` step in the payload, not only those from the
///   cursor onward — which is the same set here, since a step before the cursor is already
///   terminal and `is_terminal()` skips it either way;
/// * the message is the fixed [`control::STOP_MESSAGE`], not a computed one.
fn mark_remaining_stopped(status: &mut RunStatus, from_index: usize, total: usize, message: &str) {
    let now = super::now_epoch_millis_pub();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Stopped;
            step.error = Some(message.to_string());
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Stopped;
                        child.error = Some(message.to_string());
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

/// Deliver `verb` to every live nested async descendant of this run, logging each failed delivery
/// into this run's own `events.jsonl` under pi's `subagent.nested.{interrupt,timeout}_failed`
/// event types (`subagent-runner.ts:1539-1573` @v0.34.0).
///
/// A run with no `nested_route` has no descendants to reach and this is a no-op — that is the
/// common case (a leaf background run), so the cascade costs nothing on the path that does not
/// need it.
async fn cascade_to_descendants(
    config: &RunnerConfig,
    events: &mut Option<BoundedJsonlWriter>,
    verb: cascade::CascadeVerb,
) {
    let Some(route) = config.nested_route.as_ref() else {
        return;
    };
    let report = cascade::cascade_to_nested_async_descendants(route, verb).await;
    for failure in report.failures {
        let mut payload = serde_json::json!({
            "runId": config.run_id.as_str(),
            "message": failure.message,
        });
        if let (Some(target), Some(map)) = (failure.target_run_id, payload.as_object_mut()) {
            map.insert("targetRunId".to_string(), serde_json::Value::String(target));
        }
        append_event(events, verb.failure_event_type(), Some(payload)).await;
    }
}

fn mark_remaining_paused(status: &mut RunStatus, from_index: usize, total: usize) {
    let now = super::now_epoch_millis_pub();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Paused;
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Paused;
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

fn mark_step_running(status: &mut RunStatus, index: usize) {
    if let Some(step) = status.steps.get_mut(index) {
        step.status = StepState::Running;
        step.started_at.get_or_insert(super::now_epoch_millis_pub());
    }
}

/// Fold one completed step's [`StepResult`] (and, for a group step, its [`GroupStepResult`]'s own
/// per-child detail) back into `status.steps[index]`/`status.parallel_groups`.
fn record_step_outcome(
    status: &mut RunStatus,
    index: usize,
    step: &RunnerStep,
    result: &StepResult,
    group_result: Option<&crate::spawn::chain_graph::GroupStepResult>,
) {
    let now = super::now_epoch_millis_pub();
    if let Some(entry) = status.steps.get_mut(index) {
        entry.status = if result.success { StepState::Complete } else { StepState::Failed };
        entry.ended_at = Some(now);
        entry.error = result.error.clone();
    }

    if let (RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_), Some(group)) =
        (step, group_result)
    {
        let children: Vec<StepStatus> = group
            .children
            .iter()
            .map(|child| {
                let mut s = StepStatus::pending("<group-child>");
                s.started_at = Some(now);
                s.ended_at = Some(now);
                match child {
                    Some(outcome) => {
                        s.status = if outcome.success { StepState::Complete } else { StepState::Failed };
                        s.error = outcome.error.clone();
                    }
                    None => {
                        s.status = StepState::Failed;
                        s.error = Some("skipped (fail-fast or cancellation)".to_string());
                    }
                }
                s
            })
            .collect();
        let entry = ParallelGroupStatus {
            group_step_index: index,
            children,
        };
        status.parallel_groups.get_or_insert_with(Vec::new).push(entry);
    }
}

/// Append a [`ChainAppendRequest`]'s steps to the in-loop `steps` list AND `status.steps`
/// (R-SA-095's "only then extend its own in-loop step list/`status.json`'s `steps`/
/// `chain_step_count`" — both updated together so they never observably diverge).
fn append_steps(steps: &mut Vec<RunnerStep>, status: &mut RunStatus, request: &ChainAppendRequest) {
    for step in &request.steps {
        status.steps.push(pending_step_status_for(step));
        steps.push(step.clone());
    }
}

/// Collapse one [`StepResult`] (this file's narrow, chain-graph-local result shape) into a full
/// [`SingleResult`] (func-SA §4.3's canonical per-run record, the shape [`ResultFile::results`]
/// actually stores) — a group step's aggregate is likewise represented as one [`SingleResult`]
/// entry (per-child detail already folded into `status.parallel_groups` by
/// [`record_step_outcome`]; the terminal [`ResultFile`] carries the same one-entry-per-top-level-
/// step shape `status.steps` does, not a flattened per-child list).
fn step_result_to_single_result(step: &RunnerStep, result: &StepResult) -> SingleResult {
    let agent = match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!("<parallel:{} tasks>", group.steps.len()),
        RunnerStep::DynamicGroup(dynamic) => format!("<dynamic:{}>", dynamic.collect),
        // Never reached: `run_inner` intercepts `ImportAsyncRoot` and builds its `SingleResult`
        // directly via `imported_root_to_single_result` (the imported result's own agent, not this
        // step's display name). Kept for exhaustiveness only.
        RunnerStep::ImportAsyncRoot(spec) => spec.agent.clone(),
    };
    let task = match step {
        RunnerStep::SingleStep(spec) => spec.task.clone(),
        RunnerStep::ImportAsyncRoot(spec) => format!("Attach async root {}", spec.run_id),
        RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_) => String::new(),
    };
    SingleResult {
        agent,
        task,
        // The child's real code when the executor ran one; the success/failure mapping only as the
        // fallback for a step whose executor spawned nothing (mocks, and every group aggregate).
        // pi's async runner records the real code too (`subagent-runner.ts` stores the
        // `SingleResult` its step produced, exit code and all), so a `ResultFile` reader sees `2`
        // or `137` rather than a flattened `1`.
        exit_code: result
            .exit_code
            .unwrap_or_else(|| i32::from(!result.success)),
        usage: cyrup_core::Usage::default(),
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        final_output: result.final_output.clone(),
        structured_output: result.structured_output.clone(),
        acceptance: None,
        detached: false,
        // R-SA-084: carry the mid-flight interrupt flag through to the terminal per-step
        // `SingleResult` (pi's `interrupted` field), so a `ResultFile` reader sees which step was
        // the pause point rather than a hard-coded `false`.
        interrupted: result.interrupted,
        // Same rationale as `interrupted` above, one field over: a deadline kill is now visible on
        // the terminal per-step `SingleResult` instead of being flattened into an anonymous
        // non-zero exit.
        timed_out: result.timed_out,
        stopped: false,
        process_signal: None,
        error: result.error.clone(),
        saved_output_path: result.saved_output_path.clone(),
        tool_calls: Vec::new(),
        output_truncated: false,
        // SUBA-N05: the step's raised control events, carried through `StepResult` rather than
        // dropped — this is the only channel by which an ASYNC run's control events reach the
        // orchestrator, which reads them off the terminal `ResultFile`.
        control_events: result.control_events.clone(),
        progress: None,
    }
}

/// Collapse one [`control::ImportedAsyncRootResult`] (the product of polling an attached async root
/// to a terminal state, R-SA-097) into the [`SingleResult`] this chain records for its synthesized
/// first step. Unlike [`step_result_to_single_result`], the agent/model/attempted-models here come
/// from the IMPORTED result (the target child's own identity), not the `ImportAsyncRoot` step's
/// display spec — matching pi's `runSingleStep` returning `imported.agent`/`imported.model`/… rather
/// than the step's declared values (`subagent-runner.ts:1162-1181`).
fn imported_root_to_single_result(
    spec: &crate::spawn::chain_graph::ImportAsyncRootSpec,
    imported: &control::ImportedAsyncRootResult,
) -> SingleResult {
    SingleResult {
        agent: imported.agent.clone(),
        task: format!("Attach async root {}", spec.run_id),
        exit_code: imported.exit_code,
        usage: cyrup_core::Usage::default(),
        model: imported.model.clone(),
        attempted_models: imported.attempted_models.clone(),
        model_attempts: Vec::new(),
        final_output: Some(imported.output.clone()),
        structured_output: imported.structured_output.clone(),
        acceptance: None,
        detached: false,
        interrupted: false,
        timed_out: false,
        stopped: false,
        process_signal: None,
        error: imported.error.clone(),
        saved_output_path: None,
        tool_calls: Vec::new(),
        output_truncated: false,
        control_events: Vec::new(),
        progress: None,
    }
}

// =================================================================================================
// ExecSingleStepExecutor — the real, subprocess-spawning SingleStepExecutor (func-SA §1.1)
// =================================================================================================

/// The production [`SingleStepExecutor`] this runner's [`walk_chain`] calls dispatch through: runs
/// one [`SingleStepSpec`] to completion via [`exec::run_sync`], which — per func-SA §1.1's
/// mandated mechanism — spawns a genuine OS subprocess re-exec of the `cyrup` binary for every
/// attempt. This struct itself spawns nothing directly; it is a thin adapter translating a
/// [`SingleStepSpec`] (this file's/`chain_graph`'s own data-only step shape) into the
/// [`AgentConfig`]/[`RunOptions`] pair `exec::run_sync` actually consumes.
///
/// `pub(crate)` (rather than private to this module) so `extension.rs`'s FOREGROUND `/chain`,
/// `/parallel`, and `/run-chain` slash-command dispatch (R-SA-130: same executor as every other
/// call site, never a second divergent implementation) can drive the exact same
/// [`SingleStepExecutor`] this hop-2 background runner uses, rather than hand-rolling a second
/// `SingleStepSpec` -> `AgentConfig`/`RunOptions` adapter that could silently drift out of sync
/// with this one.
pub(crate) struct ExecSingleStepExecutor {
    pub(crate) depth: DepthEnvelope,
    pub(crate) interrupted: Arc<std::sync::atomic::AtomicBool>,
    /// The run-wide SHARED soft-interrupt token (R-SA-084). Cloned into every dispatched step's
    /// [`RunOptions::interrupt`] so that when the control-inbox watcher cancels it — an interrupt
    /// landing while a step's child is still running — that child is actually signalled and torn
    /// down mid-flight via `exec::run_sync`'s own `opts.interrupt` race, not merely noticed between
    /// steps. For a foreground executor (no control-inbox watcher) this token is never cancelled.
    pub(crate) interrupt_cancel: cyrup_core::CancelToken,
    /// The flat index of the step currently being dispatched, published here just before each
    /// dispatch so the live-telemetry [`RunOptions::live_events`] sink can tag every child NDJSON
    /// line with the step it belongs to (pi's `statusPayload.currentStep`/per-step fold,
    /// `subagent-runner.ts:1434`). Shared (`Arc`) so the sink closure reads the current value at
    /// event time rather than capturing a stale index.
    pub(crate) current_flat_index: Arc<std::sync::atomic::AtomicUsize>,
    /// The live-telemetry channel (`None` for a foreground executor with no `status.json` to
    /// update): each dispatched step installs a [`RunOptions::live_events`] sink that forwards every
    /// raw child NDJSON line here, tagged with [`Self::current_flat_index`], for the runner's own
    /// telemetry task to fold into `status.json` (pi `updateStepFromChildEvent`).
    pub(crate) telemetry: Option<tokio::sync::mpsc::UnboundedSender<TelemetryMsg>>,
    /// The fully-resolved persona for every agent any dispatched step may name (T0.1 / C13), keyed
    /// by the exact [`SingleStepSpec::agent`] string — resolved EAGERLY at plan time by the
    /// orchestrator (via [`crate::exec::resolve_step_agent_config`]) and threaded in here so
    /// [`Self::run_single`] dispatches the REAL named persona rather than re-discovering (this
    /// executor has, by design, no discovery dependency) or synthesizing a placeholder. `Arc`-wrapped
    /// so it can be cheaply shared across every fanned-out child of a parallel/dynamic group without
    /// cloning the whole map per step. Mirrors pi's already-resolved `agents` list every child
    /// resolves against (`chain-execution.ts:1011`, `parallel-execution.test.ts:134-172`).
    pub(crate) resolved_agents: Arc<BTreeMap<String, ResolvedAgentPersona>>,
    /// The launching orchestrator's own intercom presence target (pi
    /// `config.controlIntercomTarget` / `data.intercomBridge.orchestratorTarget`), threaded into
    /// every dispatched step's [`crate::exec::RunOptions::orchestrator_intercom_target`] so each
    /// spawned child activates its `contact_supervisor` bridge addressed at that supervisor. `None`
    /// (headless runner with no live intercom orchestrator, or a foreground run with no session id)
    /// leaves each child un-bridged — the clean no-intercom path.
    pub(crate) orchestrator_intercom_target: Option<String>,
    /// This run's id (pi `runId`/`config.runId`), folded with each step's agent + flat index into
    /// that child's own deterministic presence label
    /// ([`crate::spawn::intercom_target::resolve_subagent_intercom_target`]) — the address
    /// `control_resume` steers. Paired with [`Self::orchestrator_intercom_target`]: both `Some` is
    /// the child-bridge activation gate.
    pub(crate) run_id: Option<RunId>,
    /// The live PARENT session model (pi `ctx.model`, `${provider}/${id}`), inherited by any step
    /// whose persona declares no `model:` and that carries no per-step `model` override — the
    /// analog of the foreground single-run path's `SubagentExecutor::inherited_session_model()`.
    /// Threaded here (rather than read from a `HostServices` handle) because the detached hop-2
    /// runner is a separate OS process with NO host-services backend at all: the orchestrator
    /// captures it at plan time and carries it verbatim through
    /// [`RunnerConfig::inherited_session_model`]. `None` (headless / no live session, or a detached
    /// runner launched before any model was active) leaves each inheriting step's ladder to fall
    /// through to its persona's own `model`/`fallback_models`, exactly as before this seam existed.
    /// Consumed by [`Self::run_single`] via [`crate::exec::fallback::resolve_model_inheritance`].
    pub(crate) inherited_session_model: Option<cyrup_core::ModelId>,
    /// The effective `subagents.modelScope` policy for this run (SUBA-003), carried from the
    /// orchestrator via [`RunnerConfig::model_scope`] (background) or handed directly by
    /// [`Self::foreground`]. Consumed by [`Self::run_single`], where a per-step `model:` override
    /// outside the scope FAILS the step (fail-closed, pi's `explicit` severity) rather than being
    /// quietly replaced by an allowed model. `None` = enforcement off.
    pub(crate) model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    /// SUBA-N05 — the run's FULLY-RESOLVED live-control config (pi `controlConfig`,
    /// `subagent-runner.ts:1953` / `chain-execution.ts:322,491` @v0.34.0), carried from
    /// [`RunnerConfig::control`] (background) or handed by [`Self::with_control`] (foreground
    /// `/chain`, `/parallel`, `/run-chain`). Threaded onto every dispatched step's
    /// [`crate::exec::RunOptions::control_config`], so an explicit `control` override really does
    /// move the attention/long-running thresholds each child stream is judged against.
    ///
    /// `None` is pi's `?? DEFAULT_CONTROL_CONFIG` degrade, applied inside `run_sync`.
    pub(crate) control: Option<crate::exec::control::ResolvedControlConfig>,
    /// SUBA-N06 — the run's `includeProgress` flag, carried from
    /// [`RunnerConfig::include_progress`] (background) or handed by [`Self::with_control`]'s
    /// sibling [`Self::with_include_progress`] (foreground `/chain`, `/parallel`, `/run-chain`).
    /// Threaded onto every dispatched step's [`crate::exec::RunOptions::include_progress`], so each
    /// step's [`crate::exec::SingleResult`] carries its own progress snapshot.
    pub(crate) include_progress: Option<bool>,
    /// SUBA-N03 — the run's `share` opt-in (pi `config.share` ← `params.share`), threaded onto
    /// every dispatched step's [`crate::exec::RunOptions::share`]. Its one effect is pi's
    /// `sessionEnabled = Boolean(sessionFile || sessionDir) || share` term
    /// (`runs/foreground/execution.ts:1027,1039` @v0.34.0): `Some(true)` keeps a child's session store on
    /// where it would otherwise be spawned `--no-session`. `None`/`Some(false)` is not enabling.
    pub(crate) share: Option<bool>,
    /// SUBA-N03 — where this run's per-step artifact quadruple is written (pi `ctx.artifactsDir`,
    /// `runs/background/subagent-runner.ts:879-890,1117-1125` @v0.34.0 @v0.34.0), paired with
    /// [`Self::artifact_config`]. `None` disables artifact writing outright, which is exactly pi's
    /// own first gate term (`if (ctx.artifactsDir && ctx.artifactConfig?.enabled !== false)`) and
    /// is how an explicit `artifacts: false` reaches this hop.
    pub(crate) artifacts_dir: Option<PathBuf>,
    /// SUBA-N03 — which of the four artifact files each dispatched step writes (pi
    /// `ctx.artifactConfig`). Read together with [`Self::artifacts_dir`]; `enabled: false` disables
    /// the write just as an absent dir does.
    pub(crate) artifact_config: crate::artifacts::ArtifactConfig,
    /// G90 — this run's async run directory, the root of the steer control inbox
    /// (`<run_dir>/control/steer-targets/<flatIndex>/`). Each dispatched step derives its OWN
    /// per-child inbox from it and hands the path to the child in
    /// [`crate::exec::RunOptions::steer_inbox_dir`], which is pi's
    /// `steerInboxDir: stepSteerInboxDir(asyncDir, fi)` (`subagent-runner.ts:2313,2600,2797`
    /// @v0.34.0).
    ///
    /// `None` for a FOREGROUND executor, matching upstream exactly: `steerInboxDir` is supplied
    /// only by the background runner, because the inbox lives inside an async run directory and a
    /// foreground `/chain`//`/parallel` walk has none. That is also why `control_steer` refuses a
    /// foreground run outright ([`crate::extension::STEER_FOREGROUND_RUN_REFUSAL`]) rather than
    /// queueing into a directory nothing would ever read.
    pub(crate) run_dir: Option<PathBuf>,
}

impl ExecSingleStepExecutor {
    /// Construct one for a FOREGROUND (non-detached-runner) caller: no live interrupt signal
    /// source exists at this call site (a foreground `/chain`/`/parallel`/`/run-chain` slash
    /// command has no control-inbox watcher, R-SA-082, of its own — that machinery is exclusively
    /// the hop-2 detached runner's), so `interrupted` starts (and stays) `false` for the lifetime
    /// of this executor; cancellation for a foreground run is instead carried by
    /// [`crate::spawn::chain_graph::ChainRunContext::cancel`], which every dispatched step's own
    /// `RunOptions::cancel` already threads through `exec::run_sync` regardless of this flag.
    ///
    /// `resolved_agents` is the SAME plan-time persona map a background run carries in
    /// [`RunnerConfig::resolved_agents`] — the foreground orchestrator (`extension.rs`'s `/chain`//
    /// `/parallel` dispatch) resolves every step's persona via
    /// [`crate::exec::resolve_step_agent_config`] up front and hands the map here, so the SAME real
    /// persona reaches the child on both the foreground and background paths (R-SA-130: one
    /// executor, never two divergent resolutions).
    ///
    /// `orchestrator_intercom_target` (the foreground orchestrator's own intercom presence target,
    /// via `SubagentExecutor::orchestrator_intercom_target`) + `run_id` (a fresh id minted for this
    /// foreground walk) activate the child intercom bridge on the foreground `/chain`//`/parallel`
    /// path exactly as [`RunnerConfig`] does on the background path — so a foreground-spawned child's
    /// `contact_supervisor` reaches the live human orchestrator. `None`/absent leaves each child
    /// un-bridged (headless / no live intercom session).
    ///
    /// `inherited_session_model` (the live PARENT session model, via
    /// `SubagentExecutor::inherited_session_model()`) is the model an inheriting foreground chain/
    /// parallel step falls back to when its persona declares no `model:` and it carries no per-step
    /// override — the SAME session-model inheritance the foreground single-run path applies, so a
    /// `## reviewer` step with no configured model runs the parent's live model rather than an empty
    /// ladder. `None` (headless / no live session) leaves each inheriting step on its persona's own
    /// `model`/`fallback_models`, unchanged.
    ///
    /// `model_scope` is the cwd's effective `subagents.modelScope` policy (SUBA-003), resolved by
    /// the same orchestrator discovery pass that produced `resolved_agents`, so a foreground chain
    /// step's `model:` override is policed by exactly the policy the single-run path enforces.
    #[must_use]
    pub(crate) fn foreground(
        depth: DepthEnvelope,
        resolved_agents: Arc<BTreeMap<String, ResolvedAgentPersona>>,
        orchestrator_intercom_target: Option<String>,
        run_id: Option<RunId>,
        inherited_session_model: Option<cyrup_core::ModelId>,
        model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    ) -> Self {
        Self {
            depth,
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            resolved_agents,
            // A foreground executor has no control-inbox watcher, so this token is never cancelled;
            // foreground cancellation flows through `ChainRunContext::cancel`/`RunOptions::cancel`.
            interrupt_cancel: cyrup_core::CancelToken::new(),
            current_flat_index: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            telemetry: None,
            orchestrator_intercom_target,
            run_id,
            inherited_session_model,
            model_scope,
            // Set separately via `with_control` rather than as a seventh positional argument — see
            // that method's doc.
            control: None,
            // Same rationale, via `with_include_progress`.
            include_progress: None,
            // SUBA-N03: a FOREGROUND `/chain`//`/parallel`//`/run-chain` walk carries no run-level
            // `share`/artifacts config of its own — those three slash commands expose no such flag
            // (only the `subagent` tool's SINGLE mode does, and that path never builds this
            // executor), and neither does pi's own foreground chain path. Deliberately NOT given a
            // `with_*` builder: an unused one would be dead code, and the background runner sets
            // these three directly in its own `ExecSingleStepExecutor` literal from `RunnerConfig`.
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            // G90: a foreground walk has no async run directory, hence no steer inbox — the same
            // reason upstream supplies `steerInboxDir` only from the background runner.
            run_dir: None,
        }
    }

    /// Install this run's `includeProgress` flag (SUBA-N06) — R-SA-043 compaction's opt-out,
    /// threaded onto every dispatched step's [`crate::exec::RunOptions::include_progress`].
    ///
    /// A builder step for the same two reasons [`Self::with_control`] is one: [`Self::foreground`]
    /// stays at six positional arguments, and the value is not a product of the single discovery
    /// pass those six all come from.
    #[must_use]
    pub(crate) fn with_include_progress(mut self, include_progress: Option<bool>) -> Self {
        self.include_progress = include_progress;
        self
    }

    /// Install this run's already-resolved live-control config (SUBA-N05).
    ///
    /// A builder step rather than a [`Self::foreground`] parameter for two reasons: it keeps that
    /// constructor at six arguments (clippy's `too_many_arguments` threshold is seven), and it
    /// mirrors how the value actually flows — every caller resolves it with
    /// [`crate::exec::control::resolve_control_config`] at a different point in its own plan phase,
    /// whereas the six positional arguments are all products of the single discovery pass.
    #[must_use]
    pub(crate) fn with_control(
        mut self,
        control: Option<crate::exec::control::ResolvedControlConfig>,
    ) -> Self {
        self.control = control;
        self
    }

    /// G90: the steer inbox the child at flat index `index` must be handed — pi
    /// `steerInboxDir: stepSteerInboxDir(asyncDir, fi)` (`subagent-runner.ts:2313,2600,2797`
    /// @v0.34.0).
    ///
    /// Named rather than inlined because the two halves of the runner hop have to agree on it and
    /// they are written 800 lines apart: `handle_steer_request` routes an accepted request into
    /// `control::enqueue_step_steer(run_dir, index, …)` (which writes to
    /// `step_steer_inbox_dir(run_dir, index)`), and this is where the SAME path is handed to the
    /// child. Deriving it from the run-level `steer_requests_dir`, or from a step index rather
    /// than the FLAT index, would leave both sides individually plausible and the feature silently
    /// dead — the failure mode this whole item is about.
    #[must_use]
    pub(crate) fn steer_inbox_for(&self, index: usize) -> Option<PathBuf> {
        self.run_dir
            .as_deref()
            .map(|run_dir| control::step_steer_inbox_dir(run_dir, index))
    }
}

#[async_trait::async_trait]
impl SingleStepExecutor for ExecSingleStepExecutor {
    async fn run_single(
        &self,
        step: &SingleStepSpec,
        resolved_task: &str,
        ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError> {
        // T0.1 / C13: dispatch the REAL named persona. Every step's agent was resolved to a full
        // persona at plan time by the orchestrator (`extension.rs` via
        // `exec::resolve_step_agent_config`) and threaded in through `self.resolved_agents` — this
        // executor never re-discovers (it has, by design, no discovery dependency). An agent absent
        // from the map is dispatched as `Unknown agent: <name>` (a step FAILURE, mirroring pi's
        // `agents.find((a) => a.name === seqStep.agent)` miss returning `Unknown agent`,
        // `chain-execution.ts:1011-1019` / `execution.ts:898-908`) — never silently downgraded to a
        // placeholder persona. This is what makes `## reviewer` in a chain actually run the
        // reviewer persona (its own system prompt, model, fallback ladder, tools, and
        // completion-guard flag), not an empty-system-prompt / `--model default` / guard-disabled
        // stand-in.
        let Some(persona) = self.resolved_agents.get(&step.agent) else {
            return Ok(StepResult::failure(format!("Unknown agent: {}", step.agent)));
        };

        // Reconstitute the execution-ready config from the persona, stamping THIS process's own
        // live depth envelope (a per-process runtime value the persona deliberately does not carry).
        let mut agent: AgentConfig = persona.to_agent_config(self.depth);
        // Per-step tri-state tool override (func-SA §4.2): `Some(_)` overrides the persona's own
        // allowlist, `None` defers to the persona (which is exactly what `to_agent_config` already
        // copied in). Same shape pi's `resolveStepBehavior` applies for a step-level tool override.
        if step.tools.is_some() {
            agent.tools = step.tools.clone();
        }
        // A per-step depth-ceiling override tightens the agent's own declared ceiling further; when
        // absent, the persona's own `max_subagent_depth` stands. `next_envelope` (at the spawn
        // boundary, `exec::mod`) applies the tightening-only `min()` against the inherited ceiling.
        if step.max_depth_override.is_some() {
            agent.max_subagent_depth = step.max_depth_override;
        }

        // Model-fallback ladder inputs, mirroring the single-run path (`extension.rs::run_foreground`):
        // a per-step `model` override wins (Explicit); else a persona `model:` is primary (Inherit);
        // else the live PARENT session model is inherited (pi `ctx.model`); else the ladder falls
        // through to the persona's own `fallback_models`. `available_models` is the union the
        // availability filter selects from — the persona's fallback ladder + its own model + any
        // per-step override + (when inheriting) the parent session model — so a persona with a real
        // configured model, OR an inheriting persona under a live parent, yields a non-empty ladder
        // without any `--model default` placeholder ever being synthesized (the C13/inheritance
        // defect). `self.inherited_session_model` is `None` for a headless runner or one launched
        // before any model was active, which degrades to the persona's own models exactly as before.
        let mut available_models: Vec<cyrup_core::ModelId> = agent.fallback_models.clone();
        available_models.extend(agent.model.clone());
        if let Some(step_model) = &step.model {
            available_models.push(step_model.clone());
        }
        // SUBA-003 fail-closed gate, per step: a chain/parallel step's own `model:` is an EXPLICIT
        // caller-supplied model (pi `chain-execution.ts:1118` passes `source: explicitStepModel ?
        // "explicit" : "inherited"`), so one outside `subagents.modelScope` FAILS this step with
        // pi's verbatim message rather than silently running some allowed model instead. A step
        // failure — not a `SubagentError` — because that is how this executor reports every other
        // pre-spawn rejection (`Unknown agent: …` directly above), keeping the run's own status
        // record and the surrounding chain semantics intact.
        let model_override = match crate::exec::fallback::resolve_model_inheritance(
            step.model.as_ref(),
            agent.model.as_ref(),
            self.inherited_session_model.as_ref(),
            &mut available_models,
            self.model_scope.as_ref(),
        ) {
            Ok(resolved) => resolved,
            Err(violation) => return Ok(StepResult::failure(violation.message)),
        };

        // SUBA-N04: lower THIS step's declared acceptance contract (pi `chain-execution.ts:400`
        // `acceptance: task.acceptance` for a parallel task / `:1335` `acceptance:
        // seqStep.acceptance` for a sequential step — both handed straight into the same `runSync`
        // call the SINGLE path uses) through the SAME
        // `exec::acceptance::lower_acceptance_input` the `subagent` tool's SINGLE-mode `acceptance`
        // param goes through (`extension.rs::route_single`). `run_sync` then resolves the effective
        // contract (R-SA-023), injects the `## Acceptance Contract` block into the task text,
        // EXECUTES any declared `verify[]` command as a real subprocess (R-SA-032/DI-SA-5), and —
        // because an explicitly-declared contract sets `AcceptanceContract::explicit` — applies
        // R-SA-033's post-hoc exit-code correction, so a rejected gate turns this step's `exit_code`
        // nonzero and therefore its `StepResult` into a FAILURE below.
        //
        // This field was previously a hard `None`. A chain/parallel/background step that declared
        // `acceptance` was parsed, carried all the way here, and then discarded with no warning: the
        // step ran completely UNVERIFIED and reported success on the exact same path an accepted run
        // reports it — silent, unlike a refusal, and reachable through the `tasks:[{…}]` surface
        // SUBA-041 documents as the workaround for the background SINGLE surface.
        //
        // A malformed policy FAILS the step (pi's own verbatim `validateAcceptanceInput` message)
        // rather than degrading to "no contract" — the same fail-closed choice the `modelScope`
        // violation directly above makes, and for the same reason: silently running a gate-less
        // child is the defect, not the remedy. The tool boundary
        // (`extension.rs::execute` -> `validate_execution_acceptance`, pi
        // `subagent-executor.ts:1757`) normally refuses such a policy before any child spawns; this
        // is the last line of defence for a step reaching the runner from a config file that was
        // hand-edited after validation.
        let acceptance = match step.acceptance.as_ref() {
            Some(raw) => match crate::exec::acceptance::lower_acceptance_input(raw) {
                Ok(contract) => contract,
                Err(message) => {
                    return Ok(StepResult::failure(format!(
                        "subagent step '{}' has an invalid acceptance policy: {message}",
                        step.agent
                    )));
                }
            },
            None => None,
        };

        // R-SA-084 mid-flight interrupt (C, `subagent-runner.ts:1333,2002-2005,2069` @v0.34.0): clone the
        // run-wide SHARED interrupt token so an interrupt landing WHILE this child is running (the
        // control-inbox watcher cancels `self.interrupt_cancel`) actually tears the child down via
        // `run_sync`'s `opts.interrupt` race — not merely gets noticed between steps. Previously a
        // fresh per-step token was cancelled only if an interrupt had ALREADY landed at dispatch
        // time, so interrupting a single-step run was a total no-op (the child ran to completion).
        let interrupt_token = self.interrupt_cancel.clone();
        if self.interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            interrupt_token.cancel();
        }

        // Live telemetry (pi's child-event pump, `subagent-runner.ts:1430-1517`): if this is the
        // detached hop-2 runner (a telemetry channel is installed), publish THIS step's flat index
        // and hand `run_sync` a raw-line sink that forwards every child NDJSON line — tagged with
        // that index — to the runner's telemetry task, which folds it into `status.json`.
        // The flat index this sink tags events with is published by `run_inner` into
        // `self.current_flat_index` immediately before each dispatch (a `SingleStepSpec` carries no
        // index of its own), so the sink reads the CURRENT step's index at event time.
        let live_events = self.telemetry.as_ref().map(|sender| {
            let sender = sender.clone();
            let flat = Arc::clone(&self.current_flat_index);
            crate::exec::LiveEventSink::new(move |raw: &str| {
                let flat_index = flat.load(std::sync::atomic::Ordering::SeqCst);
                let _ = sender.send(TelemetryMsg { flat_index, raw: raw.to_string() });
            })
        });

        let fork_context = match &step.session_file {
            Some(path) => ForkContext {
                mode: ContextMode::Fork,
                session_file_path: Some(path.clone()),
            },
            None => ForkContext::fresh(),
        };

        let effective_cwd = step.cwd.clone().unwrap_or_else(|| ctx.cwd.clone());
        // File-output handoff wiring (Tier-2): resolve this step's `output` FILE path (relative
        // against the step's effective cwd, absolute used verbatim — pi's `resolveSingleOutputPath`
        // fallback, `single-output.ts:64-77`) and hand it to `run_sync`, so `exec/output.rs`'s
        // stat-snapshot handoff runs and the saved-output reference message is emitted. Previously
        // hard-`None`, which is exactly why the whole file-output path was dead code.
        let output_path = step.output_path.as_deref().map(|raw| {
            let candidate = std::path::Path::new(raw);
            if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                effective_cwd.join(candidate)
            }
        });
        let opts = RunOptions {
            cwd: effective_cwd,
            deadline_at: ctx.deadline_at,
            // pi `chain-execution.ts:335-336,741-742,1197-1198` @v0.34.0: every step's `runSync` call carries BOTH
            // the chain-wide `deadlineAt` (raced against) and the nominal `timeoutMs` (only used to
            // render the timed-out message) — the same two values for every step, never re-derived
            // per step.
            timeout_ms: ctx.timeout_ms,
            // SUBA-003: carried into `run_sync` so this step's fallback ladder warns on out-of-scope
            // entries, the same way the foreground single-run path does. The step's explicit
            // `model:` was already hard-gated above.
            model_scope: self.model_scope.clone(),
            output_path,
            output_mode: step
                .output_mode
                .unwrap_or(crate::discovery::types::OutputMode::Inline),
            structured_output_schema: step.structured_output_schema.clone(),
            model_override,
            preferred_provider: None,
            available_models,
            cancel: ctx.cancel.clone(),
            interrupt: interrupt_token,
            // SUBA-N03 — pi `share: shareEnabled` (`async-execution.ts:965`) reaching this run's
            // children as one of the two `sessionEnabled` terms (`execution.ts:1027,1039` @v0.34.0). Carried
            // from `RunnerConfig::share`; `None` is "omitted", which is NOT enabling.
            share: self.share,
            // SUBA-N03 — this step's own already-resolved session directory (pi's `--session-dir`,
            // `runs/shared/pi-args.ts:109-111`). Resolved PARENT-side and carried on the step rather than
            // derived here from a run-level root: see `SingleStepSpec::session_dir`'s
            // [CYRUP-DELTA] note for why an index-derived path would be unsafe at this seam.
            session_dir: step.session_dir.clone(),
            // SUBA-N03 — this step's own SKILL override (pi's runner-step `skills`,
            // `subagent-runner.ts:872` ← `async-execution.ts:990`). `run_sync` applies pi's
            // `opts.skills ?? agent.skills` fallthrough, so `None` still defers to the resolved
            // persona's own `skills:` list (carried on the `AgentConfig` built above) and
            // `Some(vec![])` is the explicit `skill: false` "no skills" form. The orchestrator/
            // runtime fallback cwd is not threaded through the one-shot runner config, so a
            // background step resolves skill NAMES against its own step cwd.
            skills: step.skills.clone(),
            runtime_cwd: None,
            // SUBA-N06: the run's `includeProgress`, carried from `RunnerConfig::include_progress`
            // through this executor, so a background step's persisted `SingleResult` carries the
            // same progress snapshot the foreground path returns.
            include_progress: self.include_progress,
            agent_scope: step.agent_scope,
            // SUBA-N04: the step's own lowered contract (resolved above), NOT a hard `None`.
            acceptance,
            fork_context,
            live_events,
            // R-SA-P1 / PERM-001: the anchor the hop-1 spawn injected into THIS runner's own
            // environment (`background::spawn_detached`'s `env_overlay`, sourced from
            // `background::parent_anchor::detached_runner_env_overlay`), resolved explicitly here
            // and threaded on rather than left to the spawn site's fallback.
            //
            // The comment this replaces asserted that the runner "inherited
            // `CYRUP_SUBAGENT_PARENT_SESSION` in its OWN env from the hop-1 spawn" — which was
            // simply untrue: until PERM-001 the hop-1 spawn added NO env overlay whatsoever, and
            // the only writer of that variable anywhere in the workspace is
            // `exec::build_attempt_spawn_plan`, which no process ever runs against itself. A root
            // orchestrator's background run therefore reached here with no anchor in scope, every
            // hop-3 child was spawned without one, and `cyrup-permission-system`'s child gate
            // fail-closed denied every `ask` against a null forwarding target with no prompt ever
            // shown to the operator. Hop 1 now really does inject it, so the claim is finally true
            // — and this call site states the dependency instead of assuming it.
            parent_session_id: crate::background::parent_anchor::resolve_parent_session_anchor(),
            // The detached hop-2 runner has no live orchestrator human session to surface a clarify
            // ask to; a child's blocking `contact_supervisor` ask routes over the broker to whichever
            // supervisor its intercom metadata names, not through this headless runner's exec loop.
            clarify: None,
            // Intercom child-bridge activation (pi `subagent-runner.ts:779-783`): thread the
            // launching orchestrator's presence target + this run's id + THIS step's flat index so the
            // spawned child registers `contact_supervisor` (addressed at that supervisor) + a broker
            // presence under `resolve_subagent_intercom_target(run_id, step.agent, flat_index)` — the
            // SAME string `control_resume`'s `SteerRunning` arm recovers from `status.steps[index]` to
            // steer this child. The flat index is the one `run_inner` publishes into
            // `current_flat_index` immediately before each dispatch (a `SingleStepSpec` carries none
            // of its own), matching the `status.steps` position the steer path indexes by.
            orchestrator_intercom_target: self.orchestrator_intercom_target.clone(),
            run_id: self.run_id.clone(),
            child_index: Some(self.current_flat_index.load(std::sync::atomic::Ordering::SeqCst)),
            // G90 (pi `steerInboxDir: stepSteerInboxDir(asyncDir, fi)`,
            // `subagent-runner.ts:2313,2600,2797` @v0.34.0): THIS step's own per-child steer inbox,
            // handed to the spawned child so its live steering watcher has a path to attach to. The
            // index is the same `current_flat_index` `child_index` above uses — the position the
            // runner's own `deliver_steer_request` routes an accepted request to
            // (`control::enqueue_step_steer`), so the two halves of the hop address the same
            // directory by construction. `None` for a foreground executor (no async run dir).
            steer_inbox_dir: self
                .steer_inbox_for(self.current_flat_index.load(std::sync::atomic::Ordering::SeqCst)),
            // SUBA-N05: the run's resolved live-control config, threaded from
            // [`RunnerConfig::control`] (background) or [`ExecSingleStepExecutor::with_control`]
            // (foreground chain/parallel) — pi `controlConfig: input.controlConfig` on the
            // per-step `runSync` call (`chain-execution.ts:322,491,733` @v0.34.0), and
            // `config.controlConfig ?? DEFAULT_CONTROL_CONFIG` in the async runner
            // (`subagent-runner.ts:1802`). `None` still degrades to `DEFAULT_CONTROL_CONFIG` inside
            // `run_sync`, so an omitted config keeps control tracking ON with stock thresholds
            // rather than turning it off.
            control_config: self.control.clone(),
            // No live notice SINK on this path: the detached runner has no orchestrator transcript
            // to inject into, and a foreground chain/parallel walk's notices are surfaced by the
            // parent from `SingleResult::control_events`. Events are still RAISED — they land on
            // each step's `SingleResult::control_events` and travel back in the result file — which
            // is what `notifyChannels: ["async"]` describes upstream, where the runner appends them
            // to the async dir's control-event log for the parent tracker to replay
            // (`subagent-runner.ts:2270-2280` → `async-job-tracker.ts:138-166` @v0.34.0). That
            // replay hop is not ported; the events themselves are not lost.
            on_control_event: None,
            // G80 — pi `artifactsDir: ctx.artifactsDir` on the background hop's own
            // `evaluateAcceptance` call (`runs/background/subagent-runner.ts:1638-1639` @v0.43.0),
            // which is how a step's verify[] results get memoized under
            // `<artifactsDir>/acceptance/verify/<runId>/`. Gated by the SAME two-term gate every
            // other artifact write on this hop uses (`ctx.artifactsDir && ctx.artifactConfig
            // ?.enabled !== false`, `subagent-runner.ts:1192`), so `artifacts: false` disarms
            // memoization along with the quadruple.
            artifacts_dir: self
                .artifacts_dir
                .clone()
                .filter(|_| self.artifact_config.enabled),
        };

        // SUBA-N03 / T6 on the SECOND hop — pi `runs/background/subagent-runner.ts:877-889`
        // @v0.34.0: the artifact quadruple is written by the ASYNC runner too, not only by the
        // foreground path, and its `_input.md` is written BEFORE the child spawns (`:882-885`,
        // `mkdirSync` then `writeFileSync(inputPath, …)`) precisely so a child that crashes still
        // leaves a record of what it was asked to do. The gate is pi's own two-term one:
        // `ctx.artifactsDir && ctx.artifactConfig?.enabled !== false` (`:879`) — an absent dir is
        // exactly as disabling as `enabled: false`, which is how the SINGLE-mode `artifacts: false`
        // param reaches this hop.
        //
        // Best-effort throughout: a failed artifact write must never alter the `StepResult` the
        // walker observes, matching pi (whose artifact writes are un-guarded side-effects) and the
        // foreground path's identical convention.
        //
        // Index: pi passes the step's own index into `getArtifactPaths` so a chain's steps do not
        // overwrite each other's files. `current_flat_index` is the value `run_inner` publishes
        // immediately before each dispatch — the SAME index `RunOptions::child_index` above uses.
        let artifact_paths = self.artifacts_dir.as_ref().filter(|_| self.artifact_config.enabled).map(
            |dir| {
                let index = self.current_flat_index.load(std::sync::atomic::Ordering::SeqCst);
                let run_token = self.run_id.as_ref().map_or("run", RunId::as_str).to_string();
                let paths =
                    crate::artifacts::artifact_paths(dir, &run_token, &step.agent, Some(index));
                let _ = crate::artifacts::ensure_artifacts_dir(dir);
                if self.artifact_config.include_input {
                    let _ = crate::artifacts::write_artifact(
                        &paths.input_path,
                        &format!("# Task for {}\n\n{resolved_task}", step.agent),
                    );
                }
                (paths, run_token)
            },
        );

        let result = exec::run_sync(&agent, resolved_task, &opts).await;

        // SUBA-N03 / T6: the after-run half (pi `subagent-runner.ts:1117-1134` — `_output.md`,
        // `_meta.json`, and this crate's reconstructed `.jsonl`). Shares ONE implementation with
        // the foreground path via `artifacts::run_artifact_metadata`/`run_artifact_jsonl_lines`,
        // so an async run's artifacts are byte-shaped identically to a foreground run's rather
        // than being a second, drifting hand-rolled emitter.
        if let Some((paths, run_token)) = &artifact_paths {
            if self.artifact_config.include_output {
                let _ = crate::artifacts::write_artifact(
                    &paths.output_path,
                    result.final_output.as_deref().unwrap_or(""),
                );
            }
            if self.artifact_config.include_metadata {
                let _ = crate::artifacts::write_metadata(
                    &paths.metadata_path,
                    &crate::artifacts::run_artifact_metadata(run_token, &result),
                );
            }
            if self.artifact_config.include_jsonl {
                for line in crate::artifacts::run_artifact_jsonl_lines(&result) {
                    let _ = crate::artifacts::append_jsonl(&paths.jsonl_path, &line);
                }
            }
        }

        // R-SA-084: carry the mid-flight interrupt flag up so `run_inner` treats an interrupted
        // step as the pause point (`Paused`, not `Complete`). An interrupted `run_sync` reports
        // `exit_code == 0` (pi's paused-success), so it maps to `StepResult::success` here, with
        // `interrupted` set from the winning attempt's own flag.
        let mut step_result = if result.exit_code == 0 {
            StepResult::success(result.final_output, result.structured_output)
        } else {
            StepResult::failure(result.error.unwrap_or_else(|| {
                format!("subagent step '{}' exited with code {}", agent.name, result.exit_code)
            }))
        };
        step_result.interrupted = result.interrupted;
        // Carry the per-child detail pi's `collectDynamicResults` copies verbatim onto a dynamic
        // fan-out's collect records (`runs/shared/dynamic-fanout.ts:278-284` @v0.34.0). All four
        // are known HERE and nowhere upstream of here: the walker sees only `StepResult`, so
        // without this hop a timed-out child is indistinguishable from an ordinary failure, every
        // failure reports exactly `1` rather than its real code, and a later chain step cannot
        // locate the files its fanned-out siblings wrote.
        step_result.exit_code = Some(result.exit_code);
        step_result.timed_out = result.timed_out;
        step_result.saved_output_path = result.saved_output_path;
        // pi stamps `result.artifactPaths` from the quadruple it computed for this same step
        // (`runs/foreground/execution.ts:1114`, gated on the run having an artifacts dir at all).
        // `artifact_paths` above is precisely that quadruple, under precisely pi's gate
        // (`artifactsDir && artifactConfig?.enabled !== false`), so reuse it rather than
        // recomputing — a second `artifact_paths()` call would have to re-read `current_flat_index`
        // after it has already advanced.
        step_result.artifact_paths = artifact_paths
            .as_ref()
            .and_then(|(paths, _)| serde_json::to_value(paths).ok());
        // SUBA-N05: carry the events this step's control monitor raised out of `run_sync` so
        // `step_result_to_single_result` can put them on the terminal `ResultFile`. Without this
        // hop the whole async control path is inert: the thresholds are honoured, the events are
        // raised, and then they die here.
        step_result.control_events = result.control_events;
        Ok(step_result)
    }
}

// =================================================================================================
// install_ignored_sigusr2_handler — survive R-SA-081's best-effort wake-up signal
// =================================================================================================

/// Install a handler for `SIGUSR2` (R-SA-081's best-effort wake-up signal, sent by
/// [`control::deliver_wakeup_signal`] to nudge this runner's control-inbox watcher awake sooner)
/// that does nothing but drain and discard every received signal, for as long as the returned
/// task handle is kept alive.
///
/// This is REQUIRED, not defensive-programming excess: `SIGUSR2`'s default disposition on every
/// Unix target this crate ships to (Linux, macOS) is process TERMINATION. Without a registered
/// handler, `interrupt()`'s signal send would kill this runner process outright — silently
/// converting every "soft" R-SA-084 interrupt into a hard crash before `run_inner`'s own
/// cooperative `interrupted` flag ever gets a chance to observe anything, which would make a
/// `Paused` outcome unreachable by the very code path that is supposed to produce it.
///
/// The handler itself does nothing with the signal's payload — `control::watch_control_inbox`'s
/// filesystem notification (started by [`spawn_control_watcher`] immediately after this function
/// is called) and `run_inner`'s own per-iteration re-check are the actual authoritative source of
/// "an interrupt/append request landed" (DI-SA-9). This handler's only job is to exist for the
/// life of the run so the OS never falls back to terminating the process on receipt.
///
/// # Errors / fallback
///
/// If installing the signal listener itself fails (e.g. resource exhaustion), this degrades the
/// SAME way `spawn_control_watcher`'s own installation failure already degrades: the run
/// continues without the wake-up-signal fast path, relying purely on the poll-interval side of
/// `control::watch_control_inbox`'s `PollWatcher` and `run_inner`'s own per-iteration re-check —
/// never a hard failure of the run itself. In that one failure case, this function returns `None`
/// and the caller simply holds nothing (no guard needed: no handler was installed, so there is
/// nothing this crate did to make `SIGUSR2`'s default disposition worse than it already was
/// before this function was ever added).
#[cfg(unix)]
fn install_ignored_sigusr2_handler() -> Option<SigUsr2Guard> {
    let mut stream = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2()).ok()?;
    let handle = tokio::spawn(async move {
        loop {
            // `recv()` returning `None` means the underlying signal stream has been torn down
            // (process-wide signal-handling shutdown) — nothing further to drain in that case.
            if stream.recv().await.is_none() {
                return;
            }
        }
    });
    Some(SigUsr2Guard { handle })
}

/// RAII wrapper aborting the SIGUSR2-draining task on drop, mirroring
/// [`ControlWatcherHandle`]'s identical pattern immediately below.
#[cfg(unix)]
struct SigUsr2Guard {
    handle: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl Drop for SigUsr2Guard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// =================================================================================================
// spawn_control_watcher — background task forwarding control-inbox notifications
// =================================================================================================

/// Spawn a background task that installs [`control::watch_control_inbox`] and sets `interrupted`
/// whenever a notification arrives, for the duration of the returned [`ControlWatcherHandle`]
/// (dropping it stops the watch — the underlying `notify::PollWatcher` is dropped inside the
/// spawned task once the task itself is aborted, which happens automatically when
/// [`ControlWatcherHandle`] is dropped, since it wraps a [`tokio::task::JoinHandle`] with
/// `abort_on_drop`-equivalent semantics achieved via an explicit `Drop` impl below rather than
/// relying on any external crate).
///
/// # R-SA-082's two mechanisms, both present
///
/// This satisfies R-SA-082's "MUST watch its control inbox via both a filesystem-notification
/// mechanism and a fixed-interval poll fallback" via [`control::watch_control_inbox`]'s own
/// `notify::PollWatcher`-based implementation (that module's own doc comment explains why
/// `PollWatcher` IS simultaneously both halves: it does not depend on a native OS notification
/// backend being available, so there is no separate native-vs-poll branch to maintain here). The
/// mandatory synchronous startup check (the other half of R-SA-082) is performed by [`run`]
/// itself, BEFORE this function is called — never inside this function — matching
/// `control::check_control_inbox_now`'s own documented "caller MUST invoke this once before
/// installing any asynchronous watch" contract.
fn spawn_control_watcher(
    run_paths: RunPaths,
    flags: ControlFlags,
    interrupt_cancel: cyrup_core::CancelToken,
    shared_status: SharedStatus,
) -> ControlWatcherHandle {
    let ControlFlags {
        interrupted,
        timed_out,
        stopped,
    } = flags;
    let handle = tokio::spawn(async move {
        // G90: the steer queue's own `events.jsonl` writer. A second `BoundedJsonlWriter` on the
        // same file is safe and does NOT double the 50MB budget: `create` opens in append mode and
        // seeds `bytes_written` from the file's CURRENT length, so each writer's cap is measured
        // against the file as it actually is, not against its own contribution.
        let mut events = BoundedJsonlWriter::create(&run_paths.events).await.ok();
        // pi's in-memory `pendingStepSteers` (`subagent-runner.ts:1332,2071-2075` @v0.34.0): a steer that
        // arrives while its target child is still `pending` is HELD, not dropped, and re-attempted.
        //
        // [CYRUP-DELTA] pi flushes the pending queue from an explicit per-step
        // `flushPendingStepSteers(flatIndex)` hook at each dispatch site; this task instead
        // re-attempts on the SAME fixed interval pi's own `watchAsyncControlInbox` runs its poll
        // safety net at (`runs/background/control-channel.ts:625-692`). Same guarantee — a held steer lands as soon as
        // its child starts running — reached through the polling half of R-SA-082 rather than a new
        // hook threaded through three dispatch sites. It also closes a real gap: cyrup's watcher
        // previously had NO interval at all, so it depended entirely on `notify` firing.
        let mut pending: Vec<control::SteerRequest> = Vec::new();
        let mut ticker = tokio::time::interval(control::CONTROL_INBOX_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let (watcher, mut rx) = match control::watch_control_inbox(&run_paths) {
            Ok(pair) => pair,
            Err(_) => {
                // R-SA-082's watch is best-effort defense in depth on top of `run_inner`'s own
                // per-iteration `control::list_pending_appends`/interrupt re-check — a watcher
                // that fails to install (e.g. EMFILE/ENOSPC-class resource exhaustion) does not
                // strand the run: the step loop still re-checks `interrupted`/pending appends on
                // every iteration regardless of whether this watcher is alive at all. This task
                // simply has nothing further to do.
                return;
            }
        };
        // Keep the watcher alive for the lifetime of this task (dropping it would stop the watch)
        // — held in this local binding rather than discarded.
        let _watcher = watcher;
        loop {
            // G90 turned this from a bare `rx.recv()` await into a two-arm select: the interval arm
            // is what retries a HELD steer whose target child was still `pending` last time round,
            // and is also the poll safety net pi's `watchAsyncControlInbox` has always had.
            tokio::select! {
                message = rx.recv() => {
                    if message.is_none() {
                        break;
                    }
                }
                _ = ticker.tick() => {}
            }
            // G90: drain the steer queue and route it, BEFORE the interrupt/timeout checks. Order
            // matters and is pi's: a steer is non-terminal guidance for a child that is expected to
            // keep running, so it must be handed over before an interrupt landing in the same tick
            // tears that child down.
            route_steer_requests(&run_paths, &shared_status, &mut events, &mut pending).await;
            // The control inbox now holds TWO distinct request files (`interrupt.json` and
            // `timeout.json`), so a notification is no longer self-describing: this task must ask
            // WHICH one is pending rather than blindly assuming "interrupt". Blindly setting
            // `interrupted` on a timeout delivery would tear the live child down under the wrong
            // verdict and end the run `Paused` (resumable) when it must end `Failed`/timed-out.
            //
            // Timeout is checked first, matching pi's own drain order (`runs/background/control-channel.ts:608-609`
            // @v0.34.0) and this run loop's own top-of-iteration ordering.
            let mut wake = false;
            // G77: stop is probed FIRST, matching pi's fixed drain order
            // (`runs/background/control-channel.ts:653-655`) — when a stop and a timeout/interrupt land in the same
            // tick, the run must end `Stopped`, the hardest and least-resumable of the three.
            if control::check_stop_inbox_now(&run_paths)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                wake = true;
            }
            if control::check_timeout_inbox_now(&run_paths)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
                wake = true;
            }
            if control::check_control_inbox_now(&run_paths)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
                wake = true;
            }
            if wake {
                // R-SA-084 mid-flight interrupt: cancelling the run-wide shared interrupt token
                // tears down whatever child is running RIGHT NOW (via `run_sync`'s
                // `opts.interrupt` race), rather than waiting for the step loop's next
                // between-steps check — the difference between a control request that actually
                // stops a single long-running step's child and one that is a no-op until the
                // (never-arriving) next step. pi does the same for both verbs (`interruptRunner`
                // signals the live children; `timeoutRunner` aborts via `timeoutAbortController`).
                interrupt_cancel.cancel();
            }
        }
    });
    ControlWatcherHandle { handle }
}

/// G90 — the runner's steer router, pi `deliverSteerRequest` + the `onSteer` inbox handler
/// (`subagent-runner.ts:1740-1790,2066-2076` @v0.34.0).
///
/// One tick: drain `<run_dir>/control/steer-requests/`, merge whatever was HELD from previous ticks,
/// and for each request in `ts` order decide per target child whether it can be handed over right
/// now. An accepted request is copied into that child's own inbox
/// ([`control::enqueue_step_steer`]), counted on the step and on the run
/// ([`crate::background::StepTelemetry::steer_count`]), and logged to `events.jsonl` as
/// `subagent.steer.requested` with pi's exact `acceptedIndexes`/`rejected` payload — so
/// `subagent({ action: "status", id })` shows the acceptance and `events.jsonl` shows the full
/// decision, which is what makes `action: "steer"` observable rather than fire-and-forget.
///
/// A request whose target is still `pending` is put back on `pending` for the next tick, matching
/// pi's `pendingStepSteers`. A request that lands while the run is not `Running` at all is held the
/// same way rather than discarded: pi returns early from `deliverSteerRequest`, and its request has
/// already been removed from the run-level queue by `consumeSteerRequests`, so holding it here is
/// strictly closer to pi's INTENT (`pendingStepSteers` exists precisely so early steers survive) —
/// and the whole queue is dropped with this task when the run ends either way.
async fn route_steer_requests(
    run_paths: &RunPaths,
    shared: &SharedStatus,
    events: &mut Option<BoundedJsonlWriter>,
    pending: &mut Vec<control::SteerRequest>,
) {
    let mut queue = std::mem::take(pending);
    queue.extend(control::consume_steer_requests(&run_paths.run_dir).await);
    if queue.is_empty() {
        return;
    }
    queue.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));

    let mut status_dirty = false;
    for request in queue {
        // Snapshot the decision inputs under the lock, then release it — `enqueue_step_steer` is
        // `.await`-ing filesystem work and a `std::sync::Mutex` guard must never cross an await.
        let (run_state, step_states): (RunState, Vec<StepState>) = {
            let status = lock_status(shared);
            (status.state, status.steps.iter().map(|s| s.status).collect())
        };
        if run_state != RunState::Running {
            pending.push(request);
            continue;
        }
        let targets: Vec<usize> = match request.target_index {
            Some(index) => vec![index],
            None => step_states
                .iter()
                .enumerate()
                .filter(|(_, state)| **state == StepState::Running)
                .map(|(index, _)| index)
                .collect(),
        };
        // No running child yet and no explicit target: hold rather than reject, so a steer racing
        // the very first dispatch is not lost (pi's `else pendingStepSteers.push(request)`).
        if targets.is_empty() {
            pending.push(request);
            continue;
        }

        let mut accepted: Vec<usize> = Vec::new();
        let mut rejected: Vec<serde_json::Value> = Vec::new();
        let mut held = false;
        for index in targets {
            match step_states.get(index) {
                None => rejected.push(
                    serde_json::json!({ "index": index, "reason": "child index out of range" }),
                ),
                Some(StepState::Pending) => held = true,
                Some(StepState::Running) => {
                    if control::enqueue_step_steer(&run_paths.run_dir, index, &request)
                        .await
                        .is_ok()
                    {
                        accepted.push(index);
                    } else {
                        rejected.push(serde_json::json!({
                            "index": index,
                            "reason": "child inbox write failed"
                        }));
                    }
                }
                Some(other) => rejected.push(serde_json::json!({
                    "index": index,
                    "reason": format!("child is {}", super::run_status::step_state_label(*other))
                })),
            }
        }
        if held && accepted.is_empty() && rejected.is_empty() {
            pending.push(request);
            continue;
        }

        let now = super::now_epoch_millis_pub();
        if !accepted.is_empty() {
            let mut status = lock_status(shared);
            for index in &accepted {
                if let Some(step) = status.steps.get_mut(*index) {
                    step.telemetry.steer_count =
                        Some(step.telemetry.steer_count.unwrap_or(0).saturating_add(1));
                    step.telemetry.last_steer_at = Some(now);
                }
            }
            let total = u64::try_from(accepted.len()).unwrap_or(0);
            status.telemetry.steer_count =
                Some(status.telemetry.steer_count.unwrap_or(0).saturating_add(total));
            status.telemetry.last_steer_at = Some(now);
            status.last_update = now;
            status_dirty = true;
        }

        let mut payload = serde_json::json!({
            "runId": run_paths
                .run_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "requestId": request.id,
            "message": request.message,
            "acceptedIndexes": accepted,
        });
        if let Some(map) = payload.as_object_mut() {
            if let Some(source) = request.source.as_ref() {
                map.insert("source".to_string(), serde_json::json!(source));
            }
            if let Some(target) = request.target_index {
                map.insert("targetIndex".to_string(), serde_json::json!(target));
            }
            if !rejected.is_empty() {
                map.insert("rejected".to_string(), serde_json::json!(rejected));
            }
        }
        append_event(events, "subagent.steer.requested", Some(payload)).await;
    }

    if status_dirty {
        let _ = write_shared_status(run_paths, shared).await;
    }
}

/// RAII wrapper aborting the spawned control-inbox watcher task on drop, so a caller ([`run`])
/// never needs to remember to clean it up explicitly — the watcher's only useful lifetime is the
/// duration of [`run`]'s own step loop.
struct ControlWatcherHandle {
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for ControlWatcherHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// =================================================================================================
// finish_run — R-SA-077's write-ordering invariant, the ONE funnel every exit path routes through
// =================================================================================================

/// Compute the terminal [`RunState`], write the final `status.json`, THEN write the terminal
/// [`ResultFile`] — in that EXACT order, unconditionally, on every single call site (R-SA-077).
///
/// This is the sole function in this module that writes a run's TERMINAL records. Every call site
/// in [`run`] funnels through here rather than writing either file directly, which is what makes
/// the ordering invariant structural rather than a convention every future edit must remember to
/// preserve: adding a new early-return branch to [`run`] in the future still cannot skip this
/// ordering unless that branch also skips calling this function entirely (in which case NEITHER
/// file is written, which is a strictly safer failure mode than writing them out of order — a
/// caller polling this run id sees "still Queued/Running" rather than an observably-ahead-of-
/// itself `ResultFile` with no matching terminal `status.json`).
///
/// `error` is folded into every result's own `error` field (if `results` is non-empty) OR, if
/// `results` is empty (the run never got far enough to produce even one step outcome), into a
/// single synthesized placeholder [`SingleResult`] so [`ResultFile::results`] is never silently
/// empty for a run that reached a terminal Failed state — a downstream reader walking `results`
/// should always find at least one entry explaining what happened, mirroring
/// `reconcile.rs::synthesize_step_results`'s identical "never leave results empty" contract for
/// the stale-dead-reconciliation path (this is the runner's OWN, first-hand analogue of that same
/// contract, not a re-derivation of `reconcile.rs`'s logic).
///
/// # Double-invocation idempotency (a `finish_run`-level guard, not just `read_and_delete_config`'s)
///
/// Before writing anything, this function checks whether `run_paths.result` ALREADY exists on
/// disk. If it does, some earlier call — this same process's own [`run`] invocation, or (per this
/// module's documented double-invocation scenario) a wholly separate, later `run()` invocation
/// against the same `--config`/`run_paths` pair after the first has already reached a terminal
/// write — has already produced the authoritative terminal record for this run id, and this call
/// is a no-op: neither `status.json` nor the `ResultFile` is touched. Without this guard, a second
/// `run()` invocation whose `read_and_delete_config` call observes
/// [`ConfigConsumeOutcome::AlreadyConsumed`] (module docs: "degrades gracefully instead of
/// crashing") would still reach `finish_run` and — since the terminal-transition write below is
/// deliberately unconditional/guard-bypassing precisely so it can ALWAYS reach a terminal state —
/// silently overwrite a genuinely-completed run's `Complete`/`success: true` result with a
/// synthesized `Failed`/`success: false` one. R-SA-077 already establishes that `ResultFile`
/// presence is the single authoritative "truly done" signal for every OTHER reader in this
/// subsystem (`reconcile.rs`, `control.rs`); this guard applies that identical principle
/// reflexively to the runner's own terminal-write path, so "no panic on double-invocation" (this
/// module's literal contract) also means "no silent data corruption of an already-final result"
/// (the property that contract exists to protect in the first place).
async fn finish_run(
    run_paths: &RunPaths,
    mut status: RunStatus,
    terminal_state: RunState,
    mut results: Vec<SingleResult>,
    cwd: PathBuf,
    session_file: Option<PathBuf>,
    error: String,
) {
    if matches!(tokio::fs::try_exists(&run_paths.result).await, Ok(true)) {
        tracing::warn!(
            run_id = %status.run_id,
            "finish_run called again after a terminal ResultFile already exists on disk \
             (double-invocation of the runner); leaving the existing authoritative result \
             untouched rather than overwriting it"
        );
        return;
    }

    // Force the terminal transition directly (mirrors `reconcile.rs::synthesize_failure`'s own
    // rationale): `finish_run` must be able to reach ANY of Complete/Failed/Paused regardless of
    // which (possibly already-illegal-from-here) state `status` currently holds, since this is the
    // authoritative "this run is now over" write, not a normal forward-progress transition subject
    // to the ordinary transition guard.
    let now = super::now_epoch_millis_pub();
    status.state = terminal_state;
    status.last_update = now;
    status.ended_at = Some(now);
    // pi `statusPayload.cwd`/`statusPayload.sessionFile` (`subagent-runner.ts:3021` @v0.34.0): the
    // terminal `status.json` write carries the SAME `cwd`/`sessionFile` the terminal `ResultFile`
    // below does, so `resume`'s terminal-revival branch (R-SA-085) can read `status.cwd ??
    // result.cwd` (`background/async-resume.ts:323,345,373`) straight off the reconciled status
    // without needing a second, separate ResultFile read.
    status.cwd = Some(cwd.clone());
    status.session_file = session_file.clone();

    if !error.is_empty() && results.is_empty() {
        results.push(SingleResult {
            agent: status
                .steps
                .first()
                .map(|s| s.agent.clone())
                .unwrap_or_else(|| status.run_id.as_str().to_string()),
            task: String::new(),
            exit_code: 1,
            usage: cyrup_core::Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            // G77 — pi's `stoppedStepResult` fills BOTH halves (`subagent-runner.ts:2358-2365`:
            // `output: stopMessage, error: stopMessage`), and the sibling live-child path here
            // ([`promote_interrupted_results_to_stopped`]) already does the same. Without it a
            // stopped run whose stop landed before any step produced a result delivers an EMPTY
            // output alongside a populated error, which every output-shaped reader (the notify
            // completion message, the intercom payload's `outputs`, the status report) renders as
            // "the run produced nothing" rather than "the run was stopped". Only for `Stopped`:
            // upstream's other synthesized shapes carry their own messages and cyrup's `finish_run`
            // has no way to tell a plain `Failed` apart from a timed-out one.
            final_output: (terminal_state == RunState::Stopped).then(|| error.clone()),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: terminal_state == RunState::Paused,
            timed_out: false,
            // G77 — pi `runSubagent`'s stopped result carries `stopped: true` and
            // `exitCode: 1` (`subagent-runner.ts:2358-2365`), which is what
            // `resolveSubagentResultStatus`/`buildCompletionDetails`/`resultState` all read to
            // classify the child as stopped rather than merely failed.
            stopped: terminal_state == RunState::Stopped,
            process_signal: None,
            error: Some(error.clone()),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        });
    }

    // `success` iff the run reached `Complete` AND every recorded result exited cleanly.
    // `Iterator::all` is vacuously `true` over an empty `results` list (a `Complete` run that
    // produced zero step results — e.g. a `Chain` run whose `steps` list was itself empty — is
    // treated as a success, matching this crate's general "no work attempted, no work failed"
    // convention rather than requiring a nonsensical "at least one result" precondition).
    let success =
        terminal_state == RunState::Complete && results.iter().all(|r| r.exit_code == 0);

    // R-SA-077: status.json THEN ResultFile, in that exact order. Both writes are best-effort at
    // the OUTER level (a failure writing `status.json` here still attempts the `ResultFile` write,
    // rather than leaving the run in an indefinite non-terminal state on disk merely because ONE
    // of the two writes hit a transient I/O error) — but the ORDER between the two calls is never
    // reordered, which is the actual invariant R-SA-077 requires; `write_atomic_json`'s own
    // temp-then-rename guarantee (R-SA-076) means a reader never observes a torn write of either
    // individual file, only ever "old status, no result" or "new status, no result yet" or "new
    // status, new result" — never "new result, old status", since the result write is issued
    // strictly after the status write is issued here.
    let status_write = write_atomic_json(&run_paths.status, &status).await;

    // ensureAccessibleDir-equivalent, final guard (C7): the terminal ResultFile's directory MUST
    // exist for the authoritative "done" signal to land. This covers every exit path — including
    // the config-less pre-read error branches that pass in the caller-derived `run_paths`, whose
    // results dir the orchestrator may or may not have created — so a run can never silently fail
    // to record its terminal result merely because its results dir was absent (the exact C7
    // failure mode: the runner's divergent, never-created results dir).
    if let Some(results_dir) = run_paths.result.parent() {
        let _ = super::ensure_accessible_dir(results_dir).await;
    }

    let result_file = ResultFile {
        id: status.run_id.clone(),
        run_id: status.run_id.clone(),
        agent: status
            .steps
            .first()
            .map(|s| s.agent.clone())
            .unwrap_or_else(|| status.run_id.as_str().to_string()),
        mode: status.mode,
        state: terminal_state,
        success,
        cwd,
        session_file,
        results,
    };
    let result_write = write_atomic_json(&run_paths.result, &result_file).await;

    if let Err(err) = status_write {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write terminal status.json (R-SA-077); ResultFile write was still \
             attempted per this function's own best-effort-both-writes contract"
        );
    }
    if let Err(err) = result_write {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write terminal ResultFile (R-SA-077)"
        );
    }

    // Best-effort run-history recording (pi's `recordRun`, `run-history.ts`): one line per
    // top-level result appended to `<subagents_home>/run-history.jsonl`. Placed AFTER the
    // authoritative status/ResultFile writes (and inside the double-invocation guard above, so a
    // no-op re-invocation never double-records) — a history-write failure never affects the run.
    super::record_run_history(status.started_at, &result_file.results).await;
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
    use crate::background::atomic::write_atomic_json;
    use crate::spawn::chain_graph::SingleStepSpec;

    // ---------------------------------------------------------------------------------------
    // G90 — the runner's steer router, driven as the FULL LIFECYCLE rather than one rendered
    // block: parent writes into the run queue -> router decides per child -> per-child inbox +
    // status counters + `events.jsonl` decision record.
    // ---------------------------------------------------------------------------------------

    /// A `Running` run with `agents.len()` steps, the ones named in `running` marked `Running` and
    /// the rest left `Pending`.
    fn steer_fixture(dir: &Path, agents: &[&str], running: &[usize]) -> (RunPaths, SharedStatus) {
        let paths = RunPaths::for_run(dir, dir, &RunId::from_token("steerroute01".to_string()));
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
        let mut status =
            RunStatus::queued(paths.run_dir.file_name().map(|n| RunId::from_token(n.to_string_lossy().into_owned())).expect("run id"), RunMode::Parallel, Some(std::process::id()));
        status.state = RunState::Running;
        status.steps = agents
            .iter()
            .enumerate()
            .map(|(i, agent)| {
                let mut step = crate::background::StepStatus::pending(*agent);
                if running.contains(&i) {
                    step.status = StepState::Running;
                }
                step
            })
            .collect();
        (paths, Arc::new(std::sync::Mutex::new(status)))
    }

    /// G90, the runner's two halves must address the SAME directory.
    ///
    /// `route_steer_requests` writes an accepted request into
    /// `control::step_steer_inbox_dir(run_dir, index)`; `run_single` hands the child
    /// `steer_inbox_for(index)`. If those two ever diverge — a run-level queue dir on one side, a
    /// per-child target dir on the other; a step index on one side and a flat index on the other —
    /// each half stays individually correct and the feature is silently dead again, with no test
    /// failing. This asserts the agreement directly, at the real write site.
    #[tokio::test]
    async fn the_inbox_the_runner_writes_is_the_inbox_the_child_is_handed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b"], &[1]);
        control::request_async_steer(&paths.run_dir, "look at step two", None, Some("steer-action"))
            .await
            .expect("parent write");

        let mut events = BoundedJsonlWriter::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;

        // The executor built for THIS run, exactly as `run` builds it.
        let executor = ExecSingleStepExecutor {
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt_cancel: cyrup_core::CancelToken::new(),
            current_flat_index: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            telemetry: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            resolved_agents: Arc::new(BTreeMap::new()),
            orchestrator_intercom_target: None,
            run_id: None,
            inherited_session_model: None,
            model_scope: None,
            control: None,
            include_progress: None,
            run_dir: Some(paths.run_dir.clone()),
        };

        let handed = executor
            .steer_inbox_for(1)
            .expect("a background executor must hand its children an inbox");
        let written: Vec<_> = std::fs::read_dir(&handed)
            .unwrap_or_else(|e| {
                panic!(
                    "the runner must have written into the very directory the child is handed \
                     ({}): {e}",
                    handed.display()
                )
            })
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            written.len(),
            1,
            "the routed request must be sitting in the child's own inbox at {}",
            handed.display()
        );

        // A FOREGROUND executor has no run dir and therefore hands no inbox — the same condition
        // that makes `control_steer` refuse a foreground run outright.
        let foreground = ExecSingleStepExecutor::foreground(
            DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            Arc::new(BTreeMap::new()),
            None,
            None,
            None,
            None,
        );
        assert!(foreground.steer_inbox_for(0).is_none());
    }

    #[tokio::test]
    async fn steer_routing_fans_an_untargeted_request_to_every_running_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b", "c"], &[0, 2]);
        control::request_async_steer(&paths.run_dir, "tighten the scope", None, Some("steer-action"))
            .await
            .expect("parent write");

        let mut events = BoundedJsonlWriter::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;

        // Every RUNNING child got its own copy; the pending one did not.
        for index in [0usize, 2] {
            let inbox = control::step_steer_inbox_dir(&paths.run_dir, index);
            let files: Vec<_> = std::fs::read_dir(&inbox)
                .unwrap_or_else(|e| panic!("child {index} inbox must exist: {e}"))
                .filter_map(Result::ok)
                .collect();
            assert_eq!(files.len(), 1, "child {index} must receive exactly one steer");
            let raw = std::fs::read_to_string(files[0].path()).expect("read");
            let request: control::SteerRequest = serde_json::from_str(&raw).expect("parse");
            assert_eq!(request.target_index, Some(index), "the copy must be PINNED to its child");
            assert_eq!(request.message, "tighten the scope");
        }
        assert!(
            !control::step_steer_inbox_dir(&paths.run_dir, 1).exists(),
            "a pending child must NOT be handed an untargeted steer"
        );

        // The run-level queue was drained exactly once (delete-before-deliver).
        assert!(
            control::consume_steer_requests(&paths.run_dir).await.is_empty(),
            "the run queue must be empty after routing"
        );
        assert!(pending.is_empty(), "nothing was held");

        // Counters landed on the accepted steps AND the run, and were persisted.
        let status = lock_status(&shared).clone();
        assert_eq!(status.steps[0].telemetry.steer_count, Some(1));
        assert_eq!(status.steps[1].telemetry.steer_count, None);
        assert_eq!(status.steps[2].telemetry.steer_count, Some(1));
        assert_eq!(status.telemetry.steer_count, Some(2));
        assert!(status.telemetry.last_steer_at.is_some());
        let persisted: RunStatus =
            serde_json::from_slice(&std::fs::read(&paths.status).expect("status.json written"))
                .expect("parse status.json");
        assert_eq!(persisted.telemetry.steer_count, Some(2));

        // And the decision is on the event log, with pi's payload keys.
        drop(events);
        let log = std::fs::read_to_string(&paths.events).expect("events.jsonl");
        let line = log
            .lines()
            .find(|l| l.contains("subagent.steer.requested"))
            .expect("the router must log its decision");
        let event: serde_json::Value = serde_json::from_str(line).expect("parse event");
        assert_eq!(event["acceptedIndexes"], serde_json::json!([0, 2]));
        assert_eq!(event["source"], serde_json::json!("steer-action"));
        assert_eq!(event["message"], serde_json::json!("tighten the scope"));
        assert!(event.get("rejected").is_none(), "nothing was rejected: {event}");
    }

    #[tokio::test]
    async fn a_steer_aimed_at_a_pending_child_is_held_then_delivered_once_it_starts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b"], &[0]);
        control::request_async_steer(&paths.run_dir, "wait for me", Some(1), None)
            .await
            .expect("parent write");

        let mut events = BoundedJsonlWriter::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        assert_eq!(pending.len(), 1, "a pending target must be HELD, never dropped");
        assert!(
            !control::step_steer_inbox_dir(&paths.run_dir, 1).exists(),
            "nothing may be delivered while the child is pending"
        );

        // The child starts; the next tick delivers the held request without the parent resending.
        lock_status(&shared).steps[1].status = StepState::Running;
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        assert!(pending.is_empty(), "the held request must be released");
        let inbox = control::step_steer_inbox_dir(&paths.run_dir, 1);
        assert_eq!(
            std::fs::read_dir(&inbox).expect("inbox").count(),
            1,
            "the held request must land on the child that just started"
        );
        assert_eq!(lock_status(&shared).steps[1].telemetry.steer_count, Some(1));
    }

    #[tokio::test]
    async fn a_steer_aimed_at_a_finished_child_is_rejected_with_that_childs_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b"], &[0]);
        lock_status(&shared).steps[1].status = StepState::Complete;
        control::request_async_steer(&paths.run_dir, "too late", Some(1), None)
            .await
            .expect("parent write");

        let mut events = BoundedJsonlWriter::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        assert!(pending.is_empty(), "a terminal child is a rejection, not a hold");
        assert_eq!(lock_status(&shared).telemetry.steer_count, None);

        drop(events);
        let log = std::fs::read_to_string(&paths.events).expect("events.jsonl");
        let line = log
            .lines()
            .find(|l| l.contains("subagent.steer.requested"))
            .expect("a rejection is still logged");
        let event: serde_json::Value = serde_json::from_str(line).expect("parse event");
        assert_eq!(event["acceptedIndexes"], serde_json::json!([]));
        assert_eq!(event["rejected"][0]["index"], serde_json::json!(1));
        assert_eq!(event["rejected"][0]["reason"], serde_json::json!("child is complete"));
    }

    #[tokio::test]
    async fn steer_requests_are_routed_in_timestamp_order_not_readdir_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a"], &[0]);
        // Written newest-first on purpose: the queue file names are zero-padded by `ts`, and the
        // consumer re-sorts, so delivery order must still be oldest-first.
        for (ts, message) in [(2_000_i64, "second"), (1_000, "first")] {
            let request = control::SteerRequest {
                kind: "steer".to_string(),
                id: format!("id-{ts}"),
                ts,
                message: message.to_string(),
                target_index: None,
                source: None,
            };
            control::write_steer_request_to_dir(
                &control::steer_requests_dir(&paths.run_dir),
                &request,
            )
            .await
            .expect("write");
        }

        let mut events = BoundedJsonlWriter::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        drop(events);

        let log = std::fs::read_to_string(&paths.events).expect("events.jsonl");
        let messages: Vec<String> = log
            .lines()
            .filter(|l| l.contains("subagent.steer.requested"))
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|e| e["message"].as_str().map(str::to_string))
            .collect();
        assert_eq!(messages, vec!["first".to_string(), "second".to_string()]);
        assert_eq!(lock_status(&shared).steps[0].telemetry.steer_count, Some(2));
    }

    fn single_step(agent: &str, task: &str) -> SingleStepSpec {
        SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task: task.to_string(),
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
        }
    }

    // ---------------------------------------------------------------------------------------
    // T0.1 / C13: an unresolved step agent is dispatched as `Unknown agent: <name>` (a step
    // FAILURE) BEFORE any spawn setup — never silently downgraded to a placeholder persona.
    // Provable without the fixture binary: the persona-map miss short-circuits ahead of every
    // filesystem side effect (`run_sync`'s scratch-dir creation, the first thing any real spawn
    // attempt does), mirroring pi's `agents.find(...)` miss returning `Unknown agent`
    // (`chain-execution.ts:1011-1019`).
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn run_single_rejects_an_unresolved_agent_as_unknown_before_any_spawn() {
        let dir = tempfile::tempdir().expect("real tempdir");
        // The executor carries an EMPTY persona map — exactly the state that must NOT dispatch a
        // placeholder.
        let executor = ExecSingleStepExecutor {
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt_cancel: cyrup_core::CancelToken::new(),
            current_flat_index: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            telemetry: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            resolved_agents: Arc::new(BTreeMap::new()),
            orchestrator_intercom_target: None,
            run_id: None,
            inherited_session_model: None,
            model_scope: None,
            control: None,
            include_progress: None,
            run_dir: None,
        };
        let ctx = ChainRunContext {
            cwd: dir.path().to_path_buf(),
            deadline_at: None,
            timeout_ms: None,
            cancel: cyrup_core::CancelToken::new(),
            global_limit: GlobalConcurrencyLimit::new(4),
            worktree_base_dir: None,
            original_task: String::new(),
            chain_dir: None,
            dynamic_fanout_max_items: None,
        };
        let step = single_step("nonexistent-reviewer", "review the change");

        let result = executor
            .run_single(&step, "review the change", &ctx)
            .await
            .expect("run_single itself returns Ok, carrying the step-level failure in StepResult");

        assert!(!result.success, "an unresolved agent must be a step failure: {result:?}");
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Unknown agent: nonexistent-reviewer"),
            "expected an `Unknown agent` failure naming the missing persona, got: {:?}",
            result.error
        );
        assert!(
            !dir.path().join(".cyrup-subagent-scratch").exists(),
            "an unresolved-agent rejection must happen before run_sync's spawn-scratch dir is ever \
             created — proving no placeholder child was ever spawned"
        );
    }

    // ---------------------------------------------------------------------------------------
    // read_and_delete_config: R-SA-073 delete-then-act idempotency
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn read_and_delete_config_consumes_and_removes_the_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        let config = RunnerConfig {
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
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let outcome = read_and_delete_config(&cfg_path).await.expect("read succeeds");
        match outcome {
            ConfigConsumeOutcome::Consumed(read_back) => assert_eq!(*read_back, config),
            ConfigConsumeOutcome::AlreadyConsumed => panic!("expected Consumed"),
        }

        assert!(
            !tokio::fs::try_exists(&cfg_path).await.expect("check exists"),
            "the config file must be deleted immediately after being read (R-SA-073)"
        );
    }

    #[tokio::test]
    async fn read_and_delete_config_double_consume_does_not_panic() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        let config = RunnerConfig {
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
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let first = read_and_delete_config(&cfg_path).await.expect("first read succeeds");
        assert!(matches!(first, ConfigConsumeOutcome::Consumed(_)));

        // The load-bearing idempotency proof this task calls for: a SECOND consume against the
        // now-deleted path must not panic, must not error, and must report AlreadyConsumed.
        let second = read_and_delete_config(&cfg_path).await.expect("second read does not error");
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
        tokio::fs::write(&cfg_path, b"not valid json").await.expect("write garbage");

        let result = read_and_delete_config(&cfg_path).await;
        assert!(
            result.is_err(),
            "a malformed (but PRESENT) config file must surface as a genuine error, distinct \
             from the file simply being absent"
        );
    }

    // ---------------------------------------------------------------------------------------
    // run(): full run through the scripted fixture — status-then-result ordering (happy path,
    // forced-error path, missing-config path) and R-SA-096 disk-re-scan append consumption.
    //
    // These live in `tests/background_runner_main_integration.rs`, NOT here, for the identical
    // reason `spawn_detached.rs`'s own module docs give for `spawn_detached_runner`'s fixture-
    // backed proof: `CARGO_BIN_EXE_cyrup-subagent-fixture` is only defined for ordinary Cargo
    // integration tests (files under `tests/`), never for a library's own `#[cfg(test)]` unit
    // tests compiled into `src/`, so `env!("CARGO_BIN_EXE_cyrup-subagent-fixture")` cannot resolve
    // in this module at all. Separately (and independently sufficient on its own), those tests
    // must mutate `CYRUP_SUBAGENT_BINARY`/`CYRUP_SUBAGENT_FIXTURE_SCRIPT` via `unsafe { std::env::
    // set_var/remove_var }` (Rust 2024 requires `unsafe` for either), which this crate's own
    // `#![forbid(unsafe_code)]` (`src/lib.rs`) blocks even inside a `#[cfg(test)]` module — a
    // `tests/*.rs` file is its own separate compilation unit, not subject to the library crate's
    // `forbid` attribute, exactly like `tests/background_spawn_detached_integration.rs`'s own
    // established precedent for the identical constraint.
    // ---------------------------------------------------------------------------------------

    // ---------------------------------------------------------------------------------------
    // finish_run: double-invocation idempotency — a second terminal write against a run id that
    // ALREADY has an authoritative ResultFile on disk must be a no-op, never an overwrite. This is
    // the `finish_run`-level half of this module's double-invocation contract (the
    // `read_and_delete_config`-level half is proven above); together they cover the full `run()`
    // double-invocation scenario without needing the fixture binary, since `finish_run` is called
    // directly here rather than driving the whole `run_inner` step loop.
    // ---------------------------------------------------------------------------------------

    fn run_paths_in(dir: &std::path::Path, run_id: &RunId) -> RunPaths {
        let async_root = dir.join("async");
        let results_dir = dir.join("results");
        RunPaths::for_run(&async_root, &results_dir, run_id)
    }

    // ---------------------------------------------------------------------------------------
    // Second-pass adversarial-review regression: `run()`'s control-inbox-directory creation step
    // (between the initial status.json write and `run_inner`) must route ANY failure through
    // `finish_run` exactly like every other pre-loop fallible step, never bypass it via a bare
    // `?`. This is provable WITHOUT the fixture binary (never reaches `run_inner`/subprocess
    // dispatch at all): pre-creating a plain FILE at the exact path `run()` needs to
    // `create_dir_all` as a directory forces that call to fail deterministically on every
    // platform (`ENOTDIR`/`AlreadyExists`-as-non-directory), with no timing dependency.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn control_inbox_dir_creation_failure_still_reaches_a_terminal_failed_state_via_finish_run(
    ) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-badcontrol");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        // `run_paths.control_inbox` is `<run_dir>/control/interrupt.json`, so its parent is
        // `<run_dir>/control`. Pre-create a plain FILE at exactly that path so `run()`'s own
        // `tokio::fs::create_dir_all(.../control)` call is guaranteed to fail.
        tokio::fs::write(run_paths.run_dir.join("control"), b"not a directory")
            .await
            .expect("pre-create a blocking file where the control dir needs to go");

        let config = RunnerConfig {
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: run_id.clone(),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(single_step("worker", "do it"))],
            cwd: dir.path().to_path_buf(),
            session_file: None,
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
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let outcome = run(&cfg_path, &run_paths).await;
        assert!(
            outcome.is_ok(),
            "run() itself never returns Err to its own caller, even when the control-inbox \
             directory cannot be created: {outcome:?}"
        );

        let status_bytes = tokio::fs::read(&run_paths.status).await.expect(
            "status.json must exist and be terminal — a bare `?` bypassing finish_run would \
             leave it permanently stuck at the initial Running record written earlier in run()",
        );
        let status: RunStatus = serde_json::from_slice(&status_bytes).expect("valid JSON");
        assert_eq!(
            status.state,
            RunState::Failed,
            "the control-inbox-directory-creation failure must reach a terminal Failed status \
             via finish_run, not leave the run stuck Running forever: {status:?}"
        );

        let result_bytes = tokio::fs::read(&run_paths.result).await.expect(
            "ResultFile must exist too — finish_run's own status-then-result ordering must still \
             hold on this exit path, not skip both writes entirely",
        );
        let result_file: ResultFile = serde_json::from_slice(&result_bytes).expect("valid JSON");
        assert_eq!(result_file.state, RunState::Failed);
        assert!(!result_file.success);
    }

    #[tokio::test]
    async fn finish_run_second_call_after_terminal_result_exists_does_not_overwrite_it() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-double-invoke");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(111));
        status.advance_state(RunState::Running).expect("Queued -> Running");

        // First call: a genuine successful completion.
        finish_run(
            &run_paths,
            status.clone(),
            RunState::Complete,
            vec![SingleResult {
                agent: "researcher".to_string(),
                task: "do the thing".to_string(),
                exit_code: 0,
                usage: cyrup_core::Usage::default(),
                model: None,
                attempted_models: Vec::new(),
                model_attempts: Vec::new(),
                final_output: Some("done".to_string()),
                structured_output: None,
                acceptance: None,
                detached: false,
                interrupted: false,
                timed_out: false,
                stopped: false,
                process_signal: None,
                error: None,
                saved_output_path: None,
                tool_calls: Vec::new(),
                output_truncated: false,
                control_events: Vec::new(),
                progress: None,
            }],
            dir.path().to_path_buf(),
            None,
            String::new(),
        )
        .await;

        let first_result_bytes = tokio::fs::read(&run_paths.result)
            .await
            .expect("ResultFile exists after the first finish_run call");
        let first_result: ResultFile =
            serde_json::from_slice(&first_result_bytes).expect("valid JSON");
        assert_eq!(first_result.state, RunState::Complete);
        assert!(first_result.success, "first call recorded a genuine success");

        let first_status_bytes = tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json exists after the first finish_run call");

        // Second call: simulates a double-invocation of `run()` against the same config/run_paths
        // (e.g. `read_and_delete_config` observing `AlreadyConsumed` and `run`'s own tail routing
        // that outcome to `finish_run` with a freshly synthesized `Failed` status, exactly as
        // `run`'s `AlreadyConsumed` match arm does). This must NOT clobber the already-terminal,
        // already-successful result with a spurious failure.
        let second_status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(222));
        finish_run(
            &run_paths,
            second_status,
            RunState::Failed,
            Vec::new(),
            PathBuf::new(),
            None,
            "runner-config.json was already consumed".to_string(),
        )
        .await;

        let result_bytes_after_second_call = tokio::fs::read(&run_paths.result)
            .await
            .expect("ResultFile still exists after the second finish_run call");
        let result_after_second_call: ResultFile =
            serde_json::from_slice(&result_bytes_after_second_call).expect("valid JSON");
        assert_eq!(
            result_after_second_call, first_result,
            "a second finish_run call against a run id with an already-terminal ResultFile must \
             leave it byte-for-byte untouched, never overwrite a genuine success with a \
             synthesized double-invocation failure"
        );

        let status_bytes_after_second_call = tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json still exists after the second finish_run call");
        assert_eq!(
            status_bytes_after_second_call, first_status_bytes,
            "status.json must likewise be left untouched by a no-op double-invocation finish_run call"
        );
    }

    #[tokio::test]
    async fn finish_run_first_call_writes_normally_when_no_result_file_exists_yet() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-first-call");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let status = RunStatus::queued(run_id, RunMode::Single, Some(1));

        finish_run(
            &run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            dir.path().to_path_buf(),
            None,
            "boom".to_string(),
        )
        .await;

        assert!(
            tokio::fs::try_exists(&run_paths.result).await.expect("check exists"),
            "the double-invocation guard must not block a genuine FIRST terminal write"
        );
        let result: ResultFile = serde_json::from_slice(
            &tokio::fs::read(&run_paths.result).await.expect("read result"),
        )
        .expect("valid JSON");
        assert_eq!(result.state, RunState::Failed);
        assert!(!result.success);
    }

    /// pi `statusPayload.cwd`/`statusPayload.sessionFile` (`subagent-runner.ts:3021` @v0.34.0): the
    /// terminal `status.json` write must carry the SAME `cwd`/`sessionFile` the terminal
    /// `ResultFile` does, so `resume`'s terminal-revival branch (R-SA-085,
    /// `background/async-resume.ts:323,345,373`) can read `status.cwd ?? result.cwd` straight off
    /// the reconciled status. Pre-fix, `finish_run` stamped `cwd`/`session_file` only onto the
    /// `ResultFile`, leaving `status.json`'s own (newly added) fields permanently `None`.
    #[tokio::test]
    async fn finish_run_stamps_cwd_and_session_file_onto_the_terminal_status_too() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-cwd-stamp");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let status = RunStatus::queued(run_id, RunMode::Single, Some(1));
        let run_cwd = dir.path().join("the-actual-run-cwd");
        let session_file = dir.path().join("session.jsonl");

        finish_run(
            &run_paths,
            status,
            RunState::Complete,
            Vec::new(),
            run_cwd.clone(),
            Some(session_file.clone()),
            String::new(),
        )
        .await;

        let written_status: RunStatus = serde_json::from_slice(
            &tokio::fs::read(&run_paths.status).await.expect("read status"),
        )
        .expect("valid JSON");
        assert_eq!(
            written_status.cwd,
            Some(run_cwd),
            "the terminal status.json write must carry the run's own cwd, matching the ResultFile"
        );
        assert_eq!(
            written_status.session_file,
            Some(session_file),
            "the terminal status.json write must carry the run's own session_file, matching the \
             ResultFile"
        );
    }

    // ---------------------------------------------------------------------------------------
    // G77 — the `stopped` widenings that had no coverage of their own: `finish_run`'s synthesized
    // child, `mark_remaining_stopped`'s parallel-GROUP child sweep (the flat step sweep was
    // covered; the group half never was), `promote_interrupted_results_to_stopped`'s
    // already-settled-child filter, and the stop message itself. The fourth claimed ordering — a
    // stop landing together with a timeout must win — needs a real runner and lives in
    // `tests/run_state_signal_and_stop_parity.rs`.
    // ---------------------------------------------------------------------------------------

    /// G77 — pi's stop message, pinned VERBATIM.
    ///
    /// `"Subagent stopped by user."` is the literal `stopMessage` upstream defines once
    /// (`runs/background/subagent-runner.ts:1972` @v0.43.0) and then repeats as the `??` default at
    /// `:779`, `:915`, `:917`, `:952`, `:1151`, `:1596`, `:1636`, `:1658`,
    /// `runs/shared/external-cli-runner.ts:108` and `runs/background/chain-root-attachment.ts:100,147`.
    /// It is stamped onto a stopped run's terminal `error`, onto every step the stop swept, and onto
    /// the child's `finalOutput` when the child produced none of its own — so a drifting copy would
    /// silently change three separate observable records at once. Nothing pinned the text itself;
    /// every existing assertion compared it against the constant, which cannot catch a drifted
    /// constant.
    #[test]
    fn the_stop_message_is_pis_verbatim_text() {
        assert_eq!(
            control::STOP_MESSAGE,
            "Subagent stopped by user.",
            "pi `subagent-runner.ts:1972`'s literal `stopMessage`"
        );
    }

    /// G77 widening 1 — `finish_run`'s SYNTHESIZED child.
    ///
    /// When a run reaches a terminal state having produced no step results at all, `finish_run`
    /// invents one placeholder [`SingleResult`] so the `ResultFile` is never silently empty. That
    /// placeholder must carry `stopped: true` for a `Stopped` run (pi `runSubagent`'s stopped result
    /// shape, `subagent-runner.ts:4448-4454`: `stopped: true`, `exitCode: 1`) — it is the ONLY thing
    /// that lets `resolveSubagentResultStatus` classify the child as stopped rather than merely
    /// failed, and therefore the only thing that makes the grouped intercom verdict `stopped`.
    ///
    /// The three sibling terminal states are asserted in the same test so the flag cannot be
    /// widened into an unconditional `true`.
    #[tokio::test]
    async fn finish_runs_synthesized_child_carries_stopped_only_for_a_stopped_run() {
        for (terminal_state, expect_stopped, expect_interrupted) in [
            (RunState::Stopped, true, false),
            (RunState::Paused, false, true),
            (RunState::Failed, false, false),
        ] {
            let dir = tempfile::tempdir().expect("real tempdir");
            let run_id = RunId::from_token(format!("synth-{terminal_state:?}").to_lowercase());
            let run_paths = run_paths_in(dir.path(), &run_id);
            tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
            tokio::fs::create_dir_all(dir.path().join("results"))
                .await
                .expect("mkdir results_dir");

            let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
            status.steps = vec![crate::background::StepStatus::pending("scout")];

            finish_run(
                &run_paths,
                status,
                terminal_state,
                // Empty: this is exactly the input that makes `finish_run` synthesize a child.
                Vec::new(),
                dir.path().to_path_buf(),
                None,
                control::STOP_MESSAGE.to_string(),
            )
            .await;

            let result: ResultFile = serde_json::from_slice(
                &tokio::fs::read(&run_paths.result).await.expect("read result"),
            )
            .expect("valid JSON");
            assert_eq!(result.state, terminal_state);
            assert_eq!(
                result.results.len(),
                1,
                "a terminal run with an error and no step results must still explain itself"
            );
            let child = &result.results[0];
            assert_eq!(child.agent, "scout", "the placeholder inherits the first step's agent");
            assert_eq!(child.exit_code, 1);
            assert_eq!(
                child.error.as_deref(),
                Some(control::STOP_MESSAGE),
                "{terminal_state:?}: the terminal error is folded onto the placeholder"
            );
            // pi `stoppedStepResult` fills `output: stopMessage` alongside `error: stopMessage`
            // (`subagent-runner.ts:2358-2365`). Only the stopped shape does — a plain `Failed`
            // placeholder carries no output, exactly as before.
            assert_eq!(
                child.final_output.as_deref(),
                if expect_stopped { Some(control::STOP_MESSAGE) } else { None },
                "{terminal_state:?}: {child:?}"
            );
            assert_eq!(
                child.stopped, expect_stopped,
                "{terminal_state:?}: the synthesized child's `stopped` flag must track the terminal \
                 state, not be hard-coded either way: {child:?}"
            );
            assert_eq!(
                child.interrupted, expect_interrupted,
                "{terminal_state:?}: `interrupted` is the PAUSED verdict and must never coincide \
                 with `stopped`: {child:?}"
            );

            // The downstream consequence, on the same real record: the grouped intercom verdict.
            let payload = crate::tui::intercom::IntercomPayload::from_result(&result);
            let expected = if expect_stopped {
                crate::tui::intercom::SubagentResultStatus::Stopped
            } else if expect_interrupted {
                crate::tui::intercom::SubagentResultStatus::Paused
            } else {
                crate::tui::intercom::SubagentResultStatus::Failed
            };
            assert_eq!(
                payload.status, expected,
                "{terminal_state:?}: the synthesized child's flags are what `resolveGroupedStatus` \
                 reads: {payload:?}"
            );
        }
    }

    /// G77 widening 2 — `mark_remaining_stopped`'s PARALLEL-GROUP child sweep.
    ///
    /// A parallel group's children live on `RunStatus::parallel_groups`, not in the flat `steps`
    /// list, and upstream's `stopRunner` sweep marks every non-terminal one `"stopped"` with the
    /// stop message (`subagent-runner.ts:2955-2986`, whose `statusPayload.steps` walk covers the
    /// normalized parallel children too). The flat half was covered by the mid-flight stop
    /// integration test; the group half never was — a single-step run has no groups at all.
    ///
    /// Three properties, all upstream's: already-terminal children are LEFT ALONE (a child that
    /// genuinely completed before the stop landed is not relabelled), groups strictly before the
    /// cursor are untouched, and every swept child gets [`control::STOP_MESSAGE`] plus an end
    /// timestamp.
    #[test]
    fn mark_remaining_stopped_sweeps_parallel_group_children_without_relabelling_finished_ones() {
        let mut status = RunStatus::queued(
            RunId::from_token("stopgroups01".to_string()),
            RunMode::Chain,
            Some(1),
        );
        status.state = RunState::Running;
        status.steps = vec![
            crate::background::StepStatus::pending("group-a"),
            crate::background::StepStatus::pending("group-b"),
        ];
        let group = |index: usize, statuses: &[StepState]| crate::background::ParallelGroupStatus {
            group_step_index: index,
            children: statuses
                .iter()
                .map(|s| {
                    let mut child = crate::background::StepStatus::pending("kid");
                    child.status = *s;
                    child
                })
                .collect(),
        };
        status.parallel_groups = Some(vec![
            // Strictly BEFORE the cursor: already settled, must not be touched.
            group(0, &[StepState::Complete]),
            // At the cursor: one mid-flight, one never started, one already finished.
            group(1, &[StepState::Running, StepState::Pending, StepState::Complete]),
        ]);

        mark_remaining_stopped(&mut status, 1, 2, control::STOP_MESSAGE);

        let groups = status.parallel_groups.as_ref().expect("groups survive the sweep");
        assert_eq!(
            groups[0].children[0].status,
            StepState::Complete,
            "a group before the cursor is not part of the sweep at all"
        );
        assert!(
            groups[0].children[0].error.is_none(),
            "and picks up no stop message either"
        );

        let swept = &groups[1].children;
        assert_eq!(
            swept[0].status,
            StepState::Stopped,
            "the mid-flight parallel child must be marked Stopped, never Failed and never Paused"
        );
        assert_eq!(
            swept[1].status,
            StepState::Stopped,
            "a never-started parallel child is swept too (upstream sweeps `running` OR `pending`)"
        );
        assert_eq!(
            swept[2].status,
            StepState::Complete,
            "a child that genuinely finished before the stop landed keeps its own verdict"
        );
        assert_eq!(swept[0].error.as_deref(), Some(control::STOP_MESSAGE));
        assert_eq!(swept[1].error.as_deref(), Some(control::STOP_MESSAGE));
        assert!(
            swept[2].error.is_none(),
            "the finished child is not restamped with a stop message it never earned"
        );
        assert!(swept[0].ended_at.is_some(), "a swept child gets an end timestamp");
        assert!(swept[1].ended_at.is_some(), "a swept child gets an end timestamp");

        // The flat step list is swept by the same call, from the same cursor.
        assert_eq!(status.steps[0].status, StepState::Pending, "before the cursor");
        assert_eq!(status.steps[1].status, StepState::Stopped);
        assert_eq!(status.steps[1].error.as_deref(), Some(control::STOP_MESSAGE));
    }

    /// G77 widening 3, half one — `promote_interrupted_results_to_stopped` must NOT touch a child
    /// that had already settled before the stop landed.
    ///
    /// pi's own promotion is `stopped: stoppedAfterAcceptance ? true : finalResult?.stopped`
    /// (`subagent-runner.ts:1642,1722` @v0.43.0), applied to the child the stop signal tore down — a child
    /// whose record settled while `stopSignal.aborted` was still false keeps its own verdict.
    /// cyrup's witness for "torn down by the stop" is `interrupted` (all three control verbs share
    /// one cancellation token), so this asserts the filter, not just the rewrite.
    #[test]
    fn promoting_stopped_children_leaves_already_settled_ones_alone() {
        let settled = |agent: &str, interrupted: bool, output: Option<&str>| SingleResult {
            agent: agent.to_string(),
            task: String::new(),
            exit_code: 0,
            usage: cyrup_core::Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: output.map(str::to_string),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted,
            timed_out: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };
        let mut results = vec![
            settled("finished-first", false, Some("I completed before the stop.")),
            settled("torn-down", true, None),
            settled("torn-down-with-output", true, Some("partial work")),
        ];

        promote_interrupted_results_to_stopped(&mut results, control::STOP_MESSAGE);

        assert!(
            !results[0].stopped && !results[0].interrupted && results[0].exit_code == 0,
            "a child that settled before the stop keeps its own clean record: {:?}",
            results[0]
        );
        assert_eq!(
            results[0].final_output.as_deref(),
            Some("I completed before the stop.")
        );

        for promoted in &results[1..] {
            assert!(promoted.stopped, "{promoted:?}");
            assert!(
                !promoted.interrupted,
                "`interrupted` must be CLEARED, or the run reads as resumable: {promoted:?}"
            );
            assert_eq!(promoted.exit_code, 1, "pi `subagent-runner.ts:909`");
            assert_eq!(promoted.error.as_deref(), Some(control::STOP_MESSAGE));
        }
        assert_eq!(
            results[1].final_output.as_deref(),
            Some(control::STOP_MESSAGE),
            "a torn-down child with NO output of its own gets the stop message as its output \
             (pi `subagent-runner.ts:917`)"
        );
        assert_eq!(
            results[2].final_output.as_deref(),
            Some("partial work"),
            "a torn-down child that DID produce output keeps it (pi's `!finalOutput.trim()` guard)"
        );
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-097 root attachment: an ImportAsyncRoot step becomes a chain's first step by POLLING
    // another already-completed run — no subprocess spawned, so provable in-module without the
    // fixture binary (mirrors pi chain-root-attachment.ts / subagent-runner.ts:1153).
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn an_attached_async_root_becomes_a_chains_first_step() {
        let dir = tempfile::tempdir().expect("real tempdir");

        // The TARGET (already-launched) run: write its terminal status + ResultFile into its own
        // async-root/results-dir, distinct from THIS chain's own artifact roots.
        let target_async = dir.path().join("target-async");
        let target_results = dir.path().join("target-results");
        let target_id = RunId::from_token("target-root");
        let target_paths = RunPaths::for_run(&target_async, &target_results, &target_id);
        tokio::fs::create_dir_all(&target_paths.run_dir).await.expect("mkdir target run_dir");
        tokio::fs::create_dir_all(&target_results).await.expect("mkdir target results_dir");

        let mut target_status = RunStatus::queued(target_id.clone(), RunMode::Single, Some(4321));
        target_status.advance_state(RunState::Running).expect("Queued -> Running");
        target_status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&target_paths.status, &target_status)
            .await
            .expect("write target status");
        let target_result = ResultFile {
            id: target_id.clone(),
            run_id: target_id.clone(),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: dir.path().to_path_buf(),
            session_file: None,
            results: vec![SingleResult {
                agent: "researcher".to_string(),
                task: "research the topic".to_string(),
                exit_code: 0,
                usage: cyrup_core::Usage::default(),
                model: None,
                attempted_models: Vec::new(),
                model_attempts: Vec::new(),
                final_output: Some("root output".to_string()),
                structured_output: None,
                acceptance: None,
                detached: false,
                interrupted: false,
                timed_out: false,
                stopped: false,
                process_signal: None,
                error: None,
                saved_output_path: None,
                tool_calls: Vec::new(),
                output_truncated: false,
                control_events: Vec::new(),
                progress: None,
            }],
        };
        write_atomic_json(&target_paths.result, &target_result)
            .await
            .expect("write target result");

        // THIS chain: a single ImportAsyncRoot step attaching the target as its first step.
        let run_id = RunId::from_token("attaching-chain");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let config = RunnerConfig {
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: run_id.clone(),
            mode: RunMode::Chain,
            steps: vec![RunnerStep::ImportAsyncRoot(
                crate::spawn::chain_graph::ImportAsyncRootSpec {
                    run_id: "target-root".to_string(),
                    async_root: target_async.clone(),
                    results_dir: target_results.clone(),
                    index: 0,
                    agent: "attached-root".to_string(),
                    output: Some("rootOut".to_string()),
                },
            )],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let outcome = run(&cfg_path, &run_paths).await;
        assert!(outcome.is_ok(), "run() never returns Err to its caller: {outcome:?}");

        let result_file: ResultFile = serde_json::from_slice(
            &tokio::fs::read(&run_paths.result).await.expect("terminal ResultFile must exist"),
        )
        .expect("valid JSON");

        assert_eq!(result_file.state, RunState::Complete, "attached root imported as success");
        assert!(result_file.success);
        assert_eq!(result_file.results.len(), 1, "the attached root IS the chain's first step");
        let first = &result_file.results[0];
        assert_eq!(
            first.agent, "researcher",
            "the imported step takes the TARGET child's own agent, not the step's display name"
        );
        assert_eq!(first.final_output.as_deref(), Some("root output"));
        assert_eq!(first.exit_code, 0);
    }
}
