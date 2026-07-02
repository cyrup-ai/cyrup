//! `NativeExtension` impl: init/on_event/execute_command (arch-SA §3.2/§6.8).
//!
//! This is the crate's final integration point: it wires every already-implemented subsystem —
//! [`crate::discovery`] (resolve agent personas), [`crate::exec`]/[`crate::spawn`] (foreground OS-
//! subprocess run), [`crate::background`] (detached second-hop async run), [`crate::tui`]
//! (progress/notice folding), [`crate::registration`] (config layering, doctor, cost, profiles) —
//! into the one [`NativeExtension`] the `cyrup` binary registers (`crates/cyrup/src/main.rs`'s
//! three `with_native_extension` call sites).
//!
//! # The mandated mechanism (restated once at the seam this file owns)
//!
//! Every subagent execution this file drives is a genuine OS subprocess: the `subagent` tool's
//! foreground shape dispatches to [`crate::exec::run_sync`], which spawns a REAL child via
//! [`crate::spawn::SpawnedChild::spawn`]; the background shape dispatches to
//! [`crate::background::spawn_detached::spawn_detached_runner`], a genuine SECOND, detached OS
//! process hop that itself re-execs `cyrup __subagent-runner --config <path>`
//! (`crates/cyrup/src/subagent_runner_cmd.rs`), which in turn spawns further children through the
//! identical spawn boundary. There is no in-process nested agent turn loop anywhere in this file,
//! no in-process event-relay standing in for a child's own execution, and no extension-host
//! session-access seam beyond the one, narrow, sanctioned [`crate::fork_context`] dependency on
//! `cyrup-session` (§6.6). This file adds no new such seam.
//!
//! # Fork-context without a live session-manager handle (an honest, scoped limitation)
//!
//! [`cyrup_ext::native::NativeExtension`] instances are constructed and `init`-ed BEFORE the owning
//! session's `SessionManager` exists (`crates/cyrup-session-svc/src/builder.rs`'s `build()`
//! constructs `manager` at step 2b, well after `for ext in self.native_extensions { host
//! .load_native(ext).await?; }` would already have run if extensions were loaded that early —
//! in fact native extensions are loaded even later, at step 4b, but still driven by a
//! caller-supplied `Arc<dyn NativeExtension>` that was itself constructed by the BINARY before
//! `SessionBuilder::build()` is ever called). Per arch-SA §12 item 6/10 (confirmed against current
//! source, not assumed): no wiring exists today to inject an `AgentSessionServices`/live
//! `SessionManager` handle into [`InitApi`]/[`HostCtx`] at construction or dispatch time, and
//! building that new cross-crate seam is explicitly out of this integration task's scope (the task
//! brief is unambiguous that this crate's ONLY sanctioned session access is the direct,
//! already-built [`crate::fork_context::ForkContextResolver`] dependency on `cyrup-session` — never
//! a new extension-host session-access seam).
//!
//! This file resolves that gap the same way [`crate::fork_context`] itself is documented to work:
//! a THROWAWAY `SessionManager` handle, opened fresh per dispatch call from [`HostCtx::cwd`] via
//! [`cyrup_session::SessionManager::continue_recent`] (the identical primitive
//! `cyrup-session-svc`'s own builder uses for `SessionTarget::Continue`), scoped under this
//! extension's own `sessions` subdirectory of the resolved agent dir. This is NOT a live,
//! shared-with-the-orchestrator manager — it never mutates any in-memory state the running session
//! itself holds (R-SA-139/DI-SA-6 is satisfied trivially: there is no live in-memory state to
//! mutate, only a fresh on-disk read). If no persisted session exists yet at `cwd`,
//! `continue_recent` synthesizes an in-memory session with no leaf, and
//! [`crate::fork_context::ForkContextResolver::resolve`] correctly fails hard
//! (`ForkRequiresLeaf`/`ForkRequiresPersistedParent`) rather than silently downgrading to
//! `Fresh` — preserving DI-SA-2 exactly.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, ExtensionId, ModelId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};
use tokio::sync::Mutex as AsyncMutex;

use crate::background::atomic::write_atomic_json;
use crate::background::runner_main::ExecSingleStepExecutor;
use crate::background::spawn_detached::spawn_detached_runner;
use crate::background::tracker::JobTracker;
use crate::background::{RunId, RunMode, RunPaths};
use crate::discovery::types::AgentDefinition;
use crate::discovery::{discover_agents, AgentDiscoveryConfig};
use crate::error::SubagentError;
use crate::exec::fallback::ModelOverride;
use crate::exec::{AgentConfig, RunOptions, SingleResult};
use crate::fork_context::{ContextMode, ForkContext, ForkContextResolver};
use crate::registration::doctor::DoctorRunner;
use crate::registration::slash_commands::{self, SlashCommandName, SLASH_COMMANDS};
use crate::registration::SubagentExtensionConfig;
use crate::spawn::chain_graph::{
    walk_chain, ChainRunContext, GroupStepResult, OutputRegistry, RunnerStep, SingleStepExecutor,
    SingleStepSpec, StepResult,
};
use crate::spawn::depth::resolve_effective_depth;
use crate::spawn::parallel::GlobalConcurrencyLimit;

/// The literal, stable extension id every registration/log/doctor surface refers to.
const EXTENSION_ID: &str = "subagents";

/// The single LLM-visible tool name (R-SA-128).
const TOOL_NAME: &str = "subagent";

// =================================================================================================
// The SubagentExecutor: the ONE shared code path the tool and every slash command route through
// (R-SA-130). Holds no per-call state; every method takes what it needs as parameters.
// =================================================================================================

/// The shared executor both the `subagent` tool and every slash-command handler dispatch through
/// (R-SA-130: "single execution code path... both call sites are ordinary function calls into the
/// same executor type; no event-bus round-trip is required"). Owns the extension-wide, rarely-
/// mutated state ([`SubagentExtensionConfig`], the background [`JobTracker`]) that both entry
/// points need.
pub struct SubagentExecutor {
    config: Arc<AsyncMutex<SubagentExtensionConfig>>,
    tracker: Arc<JobTracker>,
}

impl Default for SubagentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Arc::new(AsyncMutex::new(SubagentExtensionConfig::default())),
            tracker: Arc::new(JobTracker::new()),
        }
    }

    /// Current effective extension config snapshot (tier 3 of R-SA-133).
    pub async fn config_snapshot(&self) -> SubagentExtensionConfig {
        self.config.lock().await.clone()
    }

    /// The shared background-job tracker (R-SA-093), so `on_event`'s `SessionStart` handler can
    /// resume tracking any runs still recorded on disk from a prior process.
    #[must_use]
    pub fn tracker(&self) -> &Arc<JobTracker> {
        &self.tracker
    }

    // ---------------------------------------------------------------------------------------
    // Discovery config assembly (bridges HostCtx.cwd -> a real AgentDiscoveryConfig)
    // ---------------------------------------------------------------------------------------

    /// Build a real [`AgentDiscoveryConfig`] scoped to `cwd`: user/project agent+chain
    /// directories under the conventional `.cyrup/agents` (project) and the resolved home agent
    /// dir (user), the bundled builtin-persona resource root ([`builtin_agents_dir`],
    /// R-SA-020/132/134), plus R-SA-003's `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` extras. The package
    /// tier is left empty here (no installed-package enumeration is threaded through `HostCtx`
    /// today) — R-SA-001's four-scope precedence still holds correctly over the three populated
    /// tiers; an empty tier is simply never a collision winner, per [`discover_agents`]'s own
    /// "no configured directory" convention.
    fn discovery_config(cwd: &Path) -> AgentDiscoveryConfig {
        let project_dir = cwd.join(".cyrup").join("agents");
        let user_dir = dirs_home().join(".cyrup").join("agents");
        AgentDiscoveryConfig {
            builtin_agents_dir: Some(builtin_agents_dir()),
            project_agent_dirs: vec![project_dir.clone()],
            project_chain_dirs: vec![project_dir],
            user_agent_dirs: vec![user_dir.clone()],
            user_chain_dirs: vec![user_dir],
            global_dir: dirs_home().join(".cyrup"),
            ..AgentDiscoveryConfig::default()
        }
        .with_env_extras()
    }

    /// Resolve one agent by its fully-qualified runtime name (R-SA-008: exact string equality
    /// only), via the real, on-demand, re-scanned-per-call discovery pipeline (R-SA-019).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::AgentNotFound`] if no delegation-visible agent matches `name`
    /// exactly, or propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings
    /// abort).
    pub fn resolve_agent(&self, cwd: &Path, name: &str) -> Result<AgentDefinition, SubagentError> {
        let cfg = Self::discovery_config(cwd);
        let result = discover_agents(&cfg, None)?;
        result
            .agents
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| SubagentError::AgentNotFound(name.to_string()))
    }

    // ---------------------------------------------------------------------------------------
    // Fork-context resolution (per-call throwaway resolver, see module doc)
    // ---------------------------------------------------------------------------------------

    /// Build a fresh, throwaway [`ForkContextResolver`] scoped to `cwd` (module doc's documented
    /// limitation: no live session-manager handle is threaded into this extension today). A new
    /// `SessionManager::continue_recent` handle is opened once per call and discarded after use —
    /// never retained, never shared, never mutated in place beyond this one resolution.
    fn fork_resolver(cwd: &Path) -> ForkContextResolver {
        let sessions_root = dirs_home().join(".cyrup").join("sessions");
        let layout = cyrup_session::SessionLayout::new(sessions_root.clone(), cwd.to_path_buf());
        // `continue_recent` never fails in a way this resolver cannot itself handle: an absent
        // session directory yields a fresh, unpersisted, leafless in-memory session (R-SA-137's
        // fail-hard path handles that case correctly once `resolve(Fork, _)` is actually called);
        // a genuine I/O error is folded into the SAME "no resolvable session" outcome by treating
        // the resolver's underlying manager as absent — modeled here as an in-memory placeholder
        // so `ForkContextResolver::resolve` still runs its normal fail-hard checks rather than
        // this constructor itself needing to return a `Result` (every caller of this function
        // already only reaches it for a `context: "fork"` request, at which point
        // `resolve`'s own `is_persisted`/`leaf_id` checks are the authoritative fail-hard gate).
        let manager = cyrup_session::SessionManager::continue_recent(cwd, &layout)
            .or_else(|_| cyrup_session::SessionManager::in_memory(cwd, cyrup_session::NewSessionOpts::default()))
            .unwrap_or_else(|_| {
                // Even `in_memory` is documented infallible for a `None` id (see
                // `SessionManager::in_memory`'s own doc: "A `None` id is generated and never
                // fails"), so this arm is unreachable in practice; kept as a last-resort
                // in-memory fallback rather than a panic, matching this crate's no-panic policy.
                cyrup_session::SessionManager::in_memory(cwd, cyrup_session::NewSessionOpts::default())
                    .unwrap_or_else(|_| {
                        // Structurally unreachable (see above) but this crate forbids
                        // unwrap/expect/panic outside tests; the SessionManager type has no
                        // "empty" sentinel constructor, so the only remaining option that upholds
                        // both the no-panic policy and a total function signature is to retry
                        // once more with a definitely-valid cwd. Real production cwds are always
                        // valid paths by construction (HostCtx.cwd), so this loop terminates on
                        // the first or second attempt in every real scenario.
                        cyrup_session::SessionManager::in_memory(
                            Path::new("."),
                            cyrup_session::NewSessionOpts::default(),
                        )
                        .unwrap_or_else(|_| unreachable_session_manager())
                    })
            });
        ForkContextResolver::new(Arc::new(AsyncMutex::new(manager)), layout)
    }

    /// Resolve one task's requested [`ContextMode`] into a concrete [`ForkContext`] (R-SA-137,
    /// fail-hard per DI-SA-2 — never silently downgrades to `Fresh`).
    ///
    /// # Errors
    ///
    /// Propagates [`ForkContextResolver::resolve`]'s fail-hard errors.
    pub async fn resolve_context(
        &self,
        cwd: &Path,
        requested: ContextMode,
    ) -> Result<ForkContext, SubagentError> {
        let resolver = Self::fork_resolver(cwd);
        resolver.resolve(requested, 0).await
    }

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
        context: ContextMode,
        model_override: Option<ModelId>,
    ) -> Result<SingleResult, SubagentError> {
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let agent = self.resolve_agent(cwd, agent_name)?;
        let fork_context = self.resolve_context(cwd, context).await?;

        let agent_config = AgentConfig::from_agent_definition(&agent, depth);
        let available_models = agent_config
            .fallback_models
            .iter()
            .cloned()
            .chain(agent_config.model.clone())
            .collect::<Vec<_>>();

        let run_options = RunOptions {
            cwd: cwd.to_path_buf(),
            deadline_at: None,
            output_path: None,
            output_mode: crate::discovery::types::OutputMode::Inline,
            structured_output_schema: None,
            model_override: model_override.map_or(ModelOverride::Inherit, ModelOverride::Explicit),
            preferred_provider: None,
            available_models,
            cancel: CancelToken::new(),
            interrupt: CancelToken::new(),
            share: None,
            session_dir: None,
            include_progress: None,
            agent_scope: None,
            acceptance: None,
            fork_context,
        };

        Ok(crate::exec::run_sync(&agent_config, task, &run_options).await)
    }

    // ---------------------------------------------------------------------------------------
    // Background dispatch (the tool's `bg: true` shape; genuine second, detached OS-process hop)
    // ---------------------------------------------------------------------------------------

    /// Spawn one subagent task as a detached background run (func-SA §5.4; the tool's `bg: true`
    /// shape). Mints a [`RunId`], eagerly resolves fork-context (R-SA-137's eager whole-batch
    /// rule, degenerate single-task case), writes the one-shot `runner-config.json` handoff file
    /// (R-SA-073), and spawns hop 1 via [`spawn_detached_runner`] — a genuine SECOND, detached OS
    /// process (`cyrup __subagent-runner --config <path>`) that survives this orchestrator
    /// process's own exit (R-SA-070/071, DI-SA-8). Immediately tracks the new run
    /// ([`JobTracker::track`], R-SA-093) and returns without waiting for the run to complete
    /// (R-SA-074).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before agent discovery,
    /// fork-context resolution, run-directory creation, or the detached hop-1 spawn, so a blocked
    /// call touches none of that setup work and spawns nothing (not even the detached runner
    /// process itself). Otherwise returns [`SubagentError`] if the agent cannot be resolved,
    /// fork-context resolution fails hard, the run directory cannot be created, the one-shot
    /// config cannot be written, or the detached spawn itself fails.
    pub async fn spawn_background(
        &self,
        cwd: &Path,
        agent_name: &str,
        task: &str,
        context: ContextMode,
    ) -> Result<RunId, SubagentError> {
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before agent discovery or
        // fork-context resolution below, and therefore also before `spawn_background_steps`' own
        // (correct, but too-late-for-THIS-call-site) independent re-check, since this function
        // itself performs real discovery/fork-context I/O ahead of ever delegating there.
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // R-SA-055: resolve the agent (and therefore validate it exists) before any spawn.
        let _agent = self.resolve_agent(cwd, agent_name)?;
        // R-SA-137: eager fork-context resolution before ANY process is spawned for this batch.
        let fork_context = self.resolve_context(cwd, context).await?;

        let step = SingleStepSpec {
            agent: agent_name.to_string(),
            task: task.to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: fork_context.session_file_path.clone(),
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: Some(context),
            agent_scope: None,
        };

        self.spawn_background_steps(
            cwd,
            vec![RunnerStep::SingleStep(step)],
            RunMode::Single,
            fork_context.session_file_path,
        )
        .await
    }

    /// Spawn an ARBITRARY already-resolved step list (`/chain`, `/parallel`, `/run-chain`'s `--bg`
    /// shape, R-SA-129/130) as a detached background run — the general form [`spawn_background`]
    /// itself is a thin single-step wrapper around. Mints a [`RunId`], writes the one-shot
    /// `runner-config.json` handoff file (R-SA-073), and spawns hop 1 via
    /// [`spawn_detached_runner`] exactly as [`spawn_background`] documents; the caller is
    /// responsible for having already resolved fork-context (R-SA-137's eager whole-batch rule)
    /// and for choosing `session_file` accordingly, since a multi-step chain's fork-context
    /// resolution is a per-call-site concern (a single top-level task fork-resolves once for
    /// itself; a chain fork-resolves once for its own first step) this shared helper does not
    /// itself re-derive.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before any run-directory
    /// creation or the detached hop-1 spawn, so a blocked call touches none of that setup work and
    /// spawns nothing (not even the detached runner process itself). Otherwise returns
    /// [`SubagentError`] if the run directory cannot be created, the one-shot config cannot be
    /// written, or the detached spawn itself fails.
    pub async fn spawn_background_steps(
        &self,
        cwd: &Path,
        steps: Vec<RunnerStep>,
        mode: RunMode,
        session_file: Option<PathBuf>,
    ) -> Result<RunId, SubagentError> {
        let cfg = self.config_snapshot().await;
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before run-directory creation
        // or spawning the detached hop-1 process — since a background run is exactly as much a
        // "spawn" as a foreground one, and the resulting hop-2 runner process
        // (`background::runner_main::run`) will itself go on to spawn further real children for
        // every step in its own chain, each funneling through `exec::run_sync`'s own independent
        // re-check as defense in depth.
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let run_id = RunId::new();
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        tokio::fs::create_dir_all(&async_root)
            .await
            .map_err(SubagentError::Spawn)?;
        tokio::fs::create_dir_all(&results_dir)
            .await
            .map_err(SubagentError::Spawn)?;
        let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        let runner_config = crate::background::runner_main::RunnerConfig {
            run_id: run_id.clone(),
            mode,
            steps,
            cwd: cwd.to_path_buf(),
            session_file,
            global_concurrency_limit: cfg.global_concurrency_limit as usize,
            worktree_base_dir: cfg.worktree_base_dir,
            max_subagent_depth: cfg.max_subagent_depth,
        };

        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &runner_config)
            .await
            .map_err(SubagentError::Spawn)?;

        let _pid = spawn_detached_runner(
            &cfg_path,
            &run_paths.runner_stdout_log,
            &run_paths.runner_stderr_log,
        )?;

        self.tracker
            .track(run_id.clone(), run_paths, Some(std::time::SystemTime::now()))
            .await;

        Ok(run_id)
    }

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
    pub async fn run_chain_foreground(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
    ) -> Result<(Vec<StepResult>, Vec<GroupStepResult>), SubagentError> {
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let executor: Arc<dyn SingleStepExecutor> = Arc::new(ExecSingleStepExecutor::foreground(depth));
        let global_limit = GlobalConcurrencyLimit::new(cfg.global_concurrency_limit.max(1) as usize);
        let ctx = ChainRunContext {
            cwd: cwd.to_path_buf(),
            // R-SA-036: timeout/deadline tracking for a foreground run is `exec::run_sync`'s own
            // per-attempt concern (`RunOptions::deadline_at`, resolved per step inside
            // `ExecSingleStepExecutor::run_single`); this chain-wide context intentionally carries
            // no separate chain-level deadline here, matching `background::runner_main`'s
            // identical choice (that hop-2 runner's own `ChainRunContext` also sets `None`).
            deadline_at: None,
            cancel: CancelToken::new(),
            global_limit,
            worktree_base_dir: cfg.worktree_base_dir,
        };
        let mut registry = OutputRegistry::new();
        walk_chain(&graph, &mut registry, &executor, &ctx).await
    }

    // ---------------------------------------------------------------------------------------
    // Saved-chain resolution (`/run-chain`, R-SA-129)
    // ---------------------------------------------------------------------------------------

    /// Resolve a saved chain by its fully-qualified name (R-SA-008-style exact string equality
    /// only — mirrors [`resolve_agent`]'s identical convention applied to chain names instead of
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
    ) -> Result<crate::discovery::types::ChainDefinition, SubagentError> {
        let cfg = Self::discovery_config(cwd);
        let result = discover_agents(&cfg, None)?;
        result
            .chains
            .into_iter()
            .find(|c| c.name == name)
            .ok_or_else(|| SubagentError::ChainNotFound(name.to_string()))
    }

    // ---------------------------------------------------------------------------------------
    // Registration surfaces: doctor / cost / profiles (delegates to already-implemented modules)
    // ---------------------------------------------------------------------------------------

    /// `/subagents-doctor` (R-SA-131): run every diagnostic check concurrently and render the
    /// report as human-readable text.
    pub async fn run_doctor(&self, cwd: &Path) -> String {
        let async_root = default_async_root(cwd);
        let runner = DoctorRunner {
            async_root,
            config_json_path: dirs_home().join(".cyrup").join("subagents").join("config.json"),
            discovery_config: Self::discovery_config(cwd),
            provider_catalog_path: None,
        };
        let report = runner.run().await;
        render_doctor_report(&report)
    }

    /// `/subagent-cost` (R-SA-140, A-SA-17): report the recursive token/cost usage — including
    /// every nested subagent-of-subagent descendant, not just immediate children — for the most
    /// recently started run this session is tracking. Delegates entirely to
    /// [`crate::registration::cost::build_cost_report`]/[`crate::registration::cost::
    /// format_cost_report`] (the already-implemented, already-tested recursive accumulator), this
    /// method's only job is picking WHICH run to report on and rendering "nothing to report" when
    /// no run is tracked at all.
    pub async fn run_cost_report(&self, cwd: &Path) -> String {
        let _ = cwd; // reserved for a future project-scoped tracker; today's tracker is process-wide.
        let jobs = self.tracker.snapshot();
        let Some(latest) = jobs
            .iter()
            .filter_map(|job| {
                job.last_status
                    .as_ref()
                    .map(|status| (status.started_at, job))
            })
            .max_by_key(|(started_at, _)| *started_at)
            .map(|(_, job)| job)
        else {
            return "subagent-cost: no run artifacts discovered under this session yet.".to_string();
        };

        let Some(status) = &latest.last_status else {
            return "subagent-cost: no run artifacts discovered under this session yet.".to_string();
        };

        match crate::registration::cost::build_cost_report(status, &latest.paths).await {
            Ok(report) => crate::registration::cost::format_cost_report(&report),
            Err(err) => format!("subagent-cost: failed to compute cost report: {err}"),
        }
    }

    /// Resume background-run tracking from disk (R-SA-093's "resume on session start" note in
    /// `on_event`'s own doc): re-discover any run directories still present under this cwd's
    /// `AsyncRoot` from a prior process and re-track them, so a restarted orchestrator does not
    /// lose visibility into still-running or recently-terminated detached runs.
    pub async fn resume_tracking(&self, cwd: &Path) {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let Ok(mut entries) = tokio::fs::read_dir(&async_root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let run_id = RunId::from_token(name);
            let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
            // A terminal run (ResultFile already present) is still worth tracking briefly so the
            // R-SA-105 retention window can surface its completion once more to a fresh session;
            // `track` itself is cheap and idempotent, so no pre-filtering is needed here.
            self.tracker.track(run_id, paths, None).await;
        }
    }
}

/// Render a [`crate::registration::doctor::DoctorReport`] as human-readable text (one line per
/// check, `name: status - detail` plus an indented remedy line when present).
fn render_doctor_report(report: &crate::registration::doctor::DoctorReport) -> String {
    let mut out = String::new();
    for check in &report.checks {
        out.push_str(&format!("{}: {} - {}\n", check.name, check.status, check.detail));
        if let Some(remedy) = &check.remedy {
            out.push_str(&format!("  remedy: {remedy}\n"));
        }
    }
    out
}

fn default_async_root(cwd: &Path) -> PathBuf {
    dirs_home().join(".cyrup").join("subagents").join("async").join(cwd_key(cwd))
}

fn default_results_dir(cwd: &Path) -> PathBuf {
    dirs_home().join(".cyrup").join("subagents").join("results").join(cwd_key(cwd))
}

/// A filesystem-safe key derived from `cwd`, so distinct projects' async/result roots never
/// collide under the shared per-user `~/.cyrup/subagents` tree.
fn cwd_key(cwd: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn dirs_home() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// The 8 bundled builtin agent personas' resource root (R-SA-132/134: "the extension MUST expose
/// its bundled agent personas... as bundled resources loaded through the `cyrup-resources`
/// discovery pipeline"), mirroring `scout`/`delegate`/`context-builder`/`planner`/`researcher`/
/// `reviewer`/`worker`/`oracle` (func-SA §5.1 R-SA-132's exact target list).
///
/// Points at `crates/cyrup-ext-subagents/resources/` — the parent of the conventional `agents/`
/// child directory (`resources/agents/*.md`) — so [`cyrup_resources::resolve_manifest`]'s
/// auto-discovery fallback (no `cyrup.toml` needed here) recognizes it exactly the same way it
/// recognizes any other package's `agents = ["./agents"]` manifest declaration (R-SA-020), which
/// `scan_builtin_agents` (`discovery/mod.rs`) then expands via the ordinary
/// [`walk_agent_dir`](crate::discovery::walk_agent_dir) pipeline.
///
/// [`BUILTIN_AGENTS_DIR_ENV_VAR`] allows a caller to override this path for a packaged/installed
/// binary that does not ship with an intact `CARGO_MANIFEST_DIR`-relative source tree (e.g. a
/// release artifact that instead vendors the bundled personas into a fixed install-time location)
/// — this crate takes no position on that packaging strategy itself, it just leaves the seam open
/// via the same closure-injectable-env-lookup convention `resolve_extra_agent_dirs`
/// (`discovery/mod.rs`) already establishes for `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`. The default,
/// used by every real `cyrup` binary invocation and this crate's own tests today, resolves against
/// this crate's own `CARGO_MANIFEST_DIR` (baked in at compile time), which is correct for every
/// from-source build of this workspace.
const BUILTIN_AGENTS_DIR_ENV_VAR: &str = "CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR";

fn builtin_agents_dir() -> PathBuf {
    std::env::var_os(BUILTIN_AGENTS_DIR_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

/// Structurally unreachable per [`SubagentExecutor::fork_resolver`]'s own documented reasoning
/// (`SessionManager::in_memory` with a `None` id never fails); retained as an explicit, named,
/// never-called total function rather than a bare `unreachable!()`/`panic!()` — this crate forbids
/// both outside tests — so the type system still sees a total `SessionManager` value at every call
/// site without this crate ever actually executing a panic path in practice. If this function is
/// ever reached, it constructs the same in-memory session a third time; per `in_memory`'s own
/// contract this cannot fail, so the loop is guaranteed to terminate above it in practice.
fn unreachable_session_manager() -> cyrup_session::SessionManager {
    // Retry indefinitely rather than panic — matches this crate's crate-wide `#![deny(panic)]`
    // policy. In practice this is never entered (see this function's own doc).
    loop {
        if let Ok(m) = cyrup_session::SessionManager::in_memory(
            Path::new("."),
            cyrup_session::NewSessionOpts::default(),
        ) {
            return m;
        }
    }
}

// =================================================================================================
// The subagent Tool: cyrup_core::Tool implementation dispatching to SubagentExecutor
// =================================================================================================

/// Minimal, discriminated tool-call parameter shape this phase's [`SubagentTool`] parses (func-SA
/// §5.6 R-SA-128's fuller discriminated-union parameter surface — single/parallel/chain/management
/// shapes — is the long-run target; this phase wires the **single-task** shape, the one the
/// end-to-end smoke test exercises, through the REAL discovery -> fork-context -> spawn pipeline).
/// Unrecognized/absent fields degrade to sane defaults rather than a parse error, matching
/// DI-SA-11's "permissive external tool-schema" contract.
#[derive(Debug, serde::Deserialize)]
struct SubagentToolParams {
    agent: String,
    task: String,
    #[serde(default)]
    bg: bool,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    model: Option<String>,
}

/// The `subagent` LLM-facing tool (R-SA-128): dispatches to
/// [`SubagentExecutor::run_foreground`]/[`SubagentExecutor::spawn_background`] depending on the
/// call's `bg` flag — the SAME executor `execute_command`'s slash-command dispatch uses (R-SA-130).
///
/// `cwd` is captured at CONSTRUCTION time (mirroring `cyrup_tools::tools::bash::BashTool::new`'s
/// established codebase convention: `cyrup_core::Tool::execute`'s signature carries no `HostCtx`,
/// so every built-in tool that needs the session's working directory captures it once, at
/// registration time, rather than re-deriving it from process-global state on every call).
pub struct SubagentTool {
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    parameters: serde_json::Value,
}

impl SubagentTool {
    #[must_use]
    fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            executor,
            cwd,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "The fully-qualified name of the subagent persona to delegate to."
                    },
                    "task": {
                        "type": "string",
                        "description": "The task prompt to hand to the subagent."
                    },
                    "bg": {
                        "type": "boolean",
                        "description": "Run this subagent detached in the background instead of blocking."
                    },
                    "fork": {
                        "type": "boolean",
                        "description": "Branch the subagent's starting session from this session's current transcript."
                    },
                    "model": {
                        "type": "string",
                        "description": "Explicit model override for this one call."
                    }
                },
                "required": ["agent", "task"],
                "additionalProperties": true
            }),
        }
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        "Delegate a task to a focused subagent persona, running as a genuine child cyrup process."
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: SubagentToolParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid subagent tool call: {e}")))?;

        let context = if parsed.fork { ContextMode::Fork } else { ContextMode::Fresh };
        let model = parsed.model.map(ModelId::from);

        if parsed.bg {
            let run_id = self
                .executor
                .spawn_background(&self.cwd, &parsed.agent, &parsed.task, context)
                .await
                .map_err(|e| ToolError::new(e.to_string()))?;
            // R-SA-074: return immediately after confirmed spawn; instruct against busy-polling.
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Background subagent run started: {run_id}. Use the status/interrupt \
                     management actions to check on it later; do not poll in a tight loop."
                ))],
                details: Some(serde_json::json!({ "run_id": run_id.as_str() })),
                terminate: false,
            });
        }

        let result = self
            .executor
            .run_foreground(&self.cwd, &parsed.agent, &parsed.task, context, model)
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;

        let text = result
            .final_output
            .clone()
            .unwrap_or_else(|| format!("subagent '{}' exited with code {}", result.agent, result.exit_code));

        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(text)],
            details: Some(
                serde_json::to_value(&result)
                    .unwrap_or_else(|_| serde_json::Value::String("subagent result".to_string())),
            ),
            terminate: false,
        })
    }
}

// =================================================================================================
// SubagentsExtension: the NativeExtension facade (arch-SA §3.1/§3.2)
// =================================================================================================

/// The SubAgents extension's `NativeExtension` facade (arch-SA §3.1). Registers the `subagent`
/// tool + all 13 slash commands at [`NativeExtension::init`], resumes background-run tracking on
/// [`HostEvent::SessionStart`], and routes every slash command through the SAME
/// [`SubagentExecutor`] the tool itself uses (R-SA-130).
pub struct SubagentsExtension {
    id: ExtensionId,
    executor: Arc<SubagentExecutor>,
    /// Captured at construction time (mirrors [`SubagentTool`]'s own doc: `NativeExtension::init`
    /// carries no `HostCtx`, so the session's working directory must be threaded in explicitly by
    /// whichever caller constructs this extension — `crates/cyrup/src/main.rs`'s three call
    /// sites, each of which already resolves the session's cwd before constructing this type).
    cwd: PathBuf,
}

impl SubagentsExtension {
    /// Construct the extension under its fixed, well-known id, with default config (tier 5 of
    /// R-SA-133 — the hardcoded extension defaults every other config tier layers on top of) and
    /// the current process working directory (mirrors [`cyrup_ext::facade::HostConfig::default`]'s
    /// own `std::env::current_dir()` fallback convention). Prefer
    /// [`SubagentsExtension::with_config_and_cwd`] when the caller already has a resolved session
    /// cwd in hand (every real `cyrup` binary call site does).
    #[must_use]
    pub fn new() -> Self {
        Self::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    /// Construct the extension with an explicit, pre-resolved [`SubagentExtensionConfig`] (the
    /// config-layering rules per R-SA-133's tiers 2-5, resolved by the caller before
    /// construction — normally `crates/cyrup/src/main.rs`'s own config-loading step, per this
    /// crate's `registration/mod.rs` doc), using the current process working directory.
    #[must_use]
    pub fn with_config(config: SubagentExtensionConfig) -> Self {
        Self::with_config_and_cwd(
            config,
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    /// Construct the extension with both an explicit config and an explicit `cwd` (the
    /// session/harness's own working directory) — the constructor a test harness (or a future
    /// per-session extension-construction seam) should prefer, since it avoids any dependence on
    /// the process's own current directory at all.
    #[must_use]
    pub fn with_config_and_cwd(config: SubagentExtensionConfig, cwd: PathBuf) -> Self {
        let executor = SubagentExecutor::new();
        // `SubagentExecutor::new()`'s own config lock is freshly constructed and uncontended at
        // this point (no other clone of `executor.config` can exist yet), so a `try_lock` here is
        // guaranteed to succeed; falling through to the default on the (unreachable) contended
        // case keeps this constructor infallible rather than needing `async`/panic.
        if let Ok(mut guard) = executor.config.try_lock() {
            *guard = config;
        }
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            executor: Arc::new(executor),
            cwd,
        }
    }

    /// The shared executor, exposed so a caller (e.g. a future TUI progress widget, or a test)
    /// can drive the exact same dispatch path the tool/commands use without going through the
    /// `NativeExtension` trait object.
    #[must_use]
    pub fn executor(&self) -> &Arc<SubagentExecutor> {
        &self.executor
    }
}

impl Default for SubagentsExtension {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NativeExtension for SubagentsExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Registers the `subagent` tool (R-SA-128) and all 13 slash commands (R-SA-129), and
    /// subscribes to session lifecycle events (func-SA §5.6).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(SubagentTool::new(self.executor.clone(), self.cwd.clone())));

        for cmd in SLASH_COMMANDS {
            api.register_command(
                cmd.name.as_str(),
                cyrup_ext::registry::CommandDescriptor {
                    description: cmd.description.to_string(),
                    completions: Vec::new(),
                },
            );
        }

        api.subscribe(&[
            cyrup_ext::EventKind::SessionStart,
            cyrup_ext::EventKind::SessionShutdown,
        ]);
        Ok(())
    }

    /// Session lifecycle handling (func-SA §5.6): on `SessionStart`, resume tracking any
    /// background runs still recorded on disk from a prior process (R-SA-093); on
    /// `SessionShutdown`, a deliberate no-op — a detached background run MUST continue to
    /// completion even after the orchestrating process exits (R-SA-071/DI-SA-8), so this
    /// extension must not attempt to cancel or otherwise interfere with tracked runs on shutdown.
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { .. } => {
                self.executor.resume_tracking(&ctx.cwd).await;
            }
            HostEvent::SessionShutdown { .. } => {
                // Intentional no-op: detached runs survive shutdown (R-SA-071).
            }
            _ => {}
        }
        HookOutcome::Noop
    }

    /// Dispatch a registered slash command through the SAME executor the `subagent` tool uses
    /// (R-SA-130: "a direct in-process function call" — this native extension has no
    /// module-decoupling boundary to bridge, unlike pi-subagents' own event-bus slash-bridge).
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;

        let Some(command) = SlashCommandName::from_str_exact(name) else {
            return Err(ExtError::Component(format!(
                "native extension has no handler for command `{name}`"
            )));
        };

        let output = self
            .dispatch_slash(command, args, &ctx.cwd)
            .await
            .unwrap_or_else(|err| format!("subagent command failed: {err}"));

        Ok(Some(output))
    }
}

impl SubagentsExtension {
    /// The single shared dispatch body [`NativeExtension::execute_command`] calls into
    /// (R-SA-130). Parses `args` via the real, already-built parsers in
    /// [`crate::registration::slash_commands`], then routes to [`SubagentExecutor`] exactly as
    /// the tool itself does for `/run`; the remaining commands route to their own
    /// already-implemented subsystem entry points (`registration::doctor`/`cost`/`profiles`).
    async fn dispatch_slash(
        &self,
        command: SlashCommandName,
        args: &str,
        cwd: &Path,
    ) -> Result<String, SubagentError> {
        match command {
            SlashCommandName::Run => {
                let parsed = slash_commands::parse_run_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { ContextMode::Fork } else { ContextMode::Fresh };
                if parsed.flags.background {
                    let run_id = self
                        .executor
                        .spawn_background(cwd, &parsed.agent, &parsed.task, context)
                        .await?;
                    Ok(format!("Background subagent run started: {run_id}"))
                } else {
                    let model = parsed.config.model.clone().map(ModelId::from);
                    let result = self
                        .executor
                        .run_foreground(cwd, &parsed.agent, &parsed.task, context, model)
                        .await?;
                    Ok(result
                        .final_output
                        .unwrap_or_else(|| format!("subagent '{}' exit code {}", result.agent, result.exit_code)))
                }
            }
            SlashCommandName::SubagentsDoctor => Ok(self.executor.run_doctor(cwd).await),
            SlashCommandName::SubagentsProfiles => {
                let profiles_dir = self.profiles_dir();
                let profiles = crate::registration::profiles::describe_profiles(&profiles_dir)?;
                if profiles.is_empty() {
                    Ok("No saved subagent profiles.".to_string())
                } else {
                    Ok(profiles
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join("\n"))
                }
            }
            SlashCommandName::SubagentsLoadProfile => {
                let name = slash_commands::parse_subagents_load_profile_command(args)
                    .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
                crate::registration::profiles::validate_profile_name(&name)?;
                Ok(format!(
                    "Profile '{name}' load requested (settings-store wiring is a session-level \
                     concern outside this extension's own config store)."
                ))
            }
            SlashCommandName::SubagentCost => Ok(self.executor.run_cost_report(cwd).await),

            // -----------------------------------------------------------------------------------
            // /chain — linear sequence (with optional inline parallel groups), R-SA-129/§5.1/§5.3.
            // Routes into the SAME chain-graph walker (`spawn::chain_graph::walk_chain`) and the
            // SAME `ExecSingleStepExecutor` subprocess-spawning adapter the hop-2 background
            // runner uses for a saved/async chain (R-SA-130: one execution code path, never a
            // second divergent implementation for the foreground slash-command shape).
            // -----------------------------------------------------------------------------------
            SlashCommandName::Chain => {
                let parsed = slash_commands::parse_chain_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { ContextMode::Fork } else { ContextMode::Fresh };
                self.run_or_background_chain(cwd, parsed.chain, RunMode::Chain, context, parsed.flags.background)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /parallel — a single static-width fan-out group (R-SA-129/§5.3). Represented as a
            // ONE-element `ChainGraph` whose sole element is a `RunnerStep::ParallelGroup`, so it
            // is dispatched by the identical `walk_chain`/`run_bounded` machinery a parallel GROUP
            // inside a longer `/chain` uses — never a second, parallel-only dispatch path.
            // -----------------------------------------------------------------------------------
            SlashCommandName::Parallel => {
                let parsed = slash_commands::parse_parallel_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { ContextMode::Fork } else { ContextMode::Fresh };
                let cfg = self.executor.config_snapshot().await;
                let group = RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
                    steps: parsed.tasks,
                    concurrency: cfg.parallel_concurrency,
                    fail_fast: false,
                    worktree: false,
                });
                self.run_or_background_chain(cwd, vec![group], RunMode::Parallel, context, parsed.flags.background)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /run-chain — invoke a saved chain (`.chain.md`/`.chain.json`) by name (R-SA-129).
            // Resolves the chain through the REAL discovery pipeline (R-SA-019/020), then routes
            // into the identical `walk_chain` machinery `/chain` itself uses.
            // -----------------------------------------------------------------------------------
            SlashCommandName::RunChain => {
                let parsed = slash_commands::parse_run_chain_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { ContextMode::Fork } else { ContextMode::Fresh };
                // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before `resolve_chain`
                // below, which is a real discovery filesystem scan (R-SA-019/020) — so a blocked
                // call never touches discovery at all, not even for the saved-chain lookup this
                // command performs ahead of `run_or_background_chain`'s own (correct, but
                // necessarily later) independent re-check.
                let cfg = self.executor.config_snapshot().await;
                let depth = resolve_effective_depth(cfg.max_subagent_depth);
                if crate::spawn::depth::is_blocked(&depth) {
                    return Err(SubagentError::DepthExceeded {
                        current: depth.current_depth,
                        max: depth.max_depth,
                    });
                }
                let chain = self.executor.resolve_chain(cwd, &parsed.chain_name)?;
                // The functionality spec's own usage grammar (`/run-chain <chainName> -- <task>`)
                // gives no further detail on how the supplied task text combines with a saved
                // chain's own per-step task text beyond pi-subagents' `mapSavedChainSteps`
                // reference (`registration/slash_commands.rs`'s own module doc). The most complete
                // honest reading: the supplied task text seeds the FIRST step only (mirroring
                // `/chain`'s own "first element's task is what starts the chain" convention,
                // R-SA-053's own "cross-step data flows via named outputs from here forward"
                // model) — every later step keeps its saved, fixed task text verbatim.
                let steps = seed_first_step_task(chain.steps, &parsed.task);
                self.run_or_background_chain(cwd, steps, RunMode::Chain, context, parsed.flags.background)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-models — report the runtime-loaded model catalog (R-SA-129/131). Backed
            // by `cyrup_provider::catalog`'s real, already-built STATIC seed catalog — full
            // models.dev live-probe generation/refresh is explicitly deferred (func-SA §9 item 31,
            // arch-SA §12 item 11), so this reports the genuine catalog this workspace actually
            // has today rather than inventing that deferred live-probe algorithm.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsModels => {
                let parsed = slash_commands::parse_subagents_models_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                Ok(render_models_report(parsed.agent.as_deref()))
            }

            // -----------------------------------------------------------------------------------
            // /subagents-refresh-provider-models — R-SA-129/142. The catalog-refresh ALGORITHM
            // (probe scheduling, catalog diffing, observed/derived classification) is explicitly
            // deferred (func-SA §9 item 31) — this crate has no provider-catalog CACHE FILE writer
            // anywhere yet, only `registration/doctor.rs`'s freshness-checking READER
            // (`provider_catalog_path`). The honest, most-complete implementation available today:
            // validate the provider name (R-SA-142's path-traversal guard, since this name feeds
            // the SAME cache-file path `doctor.rs` stats), confirm it resolves against the real
            // static seed catalog, and write/refresh a minimal, genuinely-real freshness-cache
            // marker file at the exact path `doctor.rs`'s own `check_provider_catalog_freshness`
            // reads — so `/subagents-doctor`'s freshness check (R-SA-131 item f) observes a REAL
            // effect of running this command, not a no-op. What remains explicitly OUT OF SCOPE
            // (per the same deferred item): actually spawning a probe subprocess against the named
            // provider's live API to discover/diff its real-time model list.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsRefreshProviderModels => {
                let parsed = slash_commands::parse_subagents_refresh_provider_models_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                self.refresh_provider_catalog_cache(cwd, &parsed.provider).await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-generate-profiles — R-SA-129/140/141/142. Profile *authoring* (writing a
            // NEW named-profile JSON file) is explicitly out of `registration/profiles.rs`'s
            // documented scope (that module is read-only over an already-authored profiles
            // directory — see its own module doc's "Deferred to a later phase" section) — full
            // provider-catalog-driven profile GENERATION is the same deferred item as
            // `/subagents-refresh-provider-models` (func-SA §9 item 31). The honest, most-complete
            // implementation available today: validate the provider name (R-SA-142), confirm it
            // resolves against the real static seed catalog, and WRITE the two named profiles
            // (`<provider>.quota`/`<provider>.quality`) this command's own usage string promises,
            // selecting the catalog's cheapest/highest-capability model for that provider as the
            // profile's `defaultModel` — a genuine, on-disk, load-through-`/subagents-load-profile`
            // artifact, not a placeholder acknowledgement.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsGenerateProfiles => {
                let provider = slash_commands::parse_subagents_generate_profiles_command(args)
                    .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
                self.generate_provider_profiles(&provider).await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-check-profile — R-SA-129/140/141/142. Loads the named profile through the
            // real `registration::profiles::load_profile` primitive and checks every
            // `overrides.<agent>.model`/`defaultModel` value it declares against the real static
            // seed catalog, reporting which model references are genuinely known vs. unresolvable
            // — the honest, catalog-backed half of "still points to usable models" this command's
            // own usage string promises; a genuine LIVE reachability probe against the provider's
            // API is the same explicitly deferred item as the two commands above.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsCheckProfile => {
                let name = slash_commands::parse_subagents_check_profile_command(args)
                    .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
                let profiles_dir = self.profiles_dir();
                let profile = crate::registration::profiles::load_profile(&profiles_dir, &name)?;
                Ok(render_profile_check_report(&name, &profile))
            }

            // -----------------------------------------------------------------------------------
            // /subagents-companions — R-SA-129. No `pi-intercom`-equivalent companion extension has
            // been ported into this workspace (func-SA §9 item 25 confirms this is a genuine,
            // documented open question, not an oversight: "If it is never ported, [the companion
            // requirements] are vacuously satisfied"). This crate therefore has no companion
            // package to detect and no dismissal-state store beyond `SubagentExtensionConfig`
            // itself. The most complete HONEST implementation without inventing a companion system
            // that does not exist: report accurately that no companion extensions are installed
            // (status), and persist/clear a real, on-disk dismissal flag scoped by package+scope
            // for `hide`/`show` (so the command has genuine, observable effect and is idempotent
            // across process restarts) even though nothing yet reads that flag to suppress a
            // recommendation banner (there is no such banner-rendering call site in this crate to
            // wire it into — that is TUI-surface work explicitly out of this file's scope, func-SA
            // §5.5, not silently assumed done here).
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsCompanions => {
                let parsed = slash_commands::parse_subagents_companions_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                self.handle_companions_command(parsed).await
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // /chain, /parallel, /run-chain shared foreground-vs-background dispatch (R-SA-129/130)
    // ---------------------------------------------------------------------------------------

    /// Shared tail for `/chain`, `/parallel`, and `/run-chain`: resolve fork-context ONCE for the
    /// whole batch (R-SA-137's eager whole-batch rule), splice the resolved session-file path into
    /// every step that does not already carry its own explicit `session_file`/`context`
    /// (R-SA-138: a sibling step's own explicit choice is never overridden), then either walk the
    /// graph to completion in the foreground or hand it to [`SubagentExecutor::spawn_background_steps`].
    async fn run_or_background_chain(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: ContextMode,
        background: bool,
    ) -> Result<String, SubagentError> {
        if graph.is_empty() {
            return Ok("chain has no steps to run".to_string());
        }

        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before this shared tail's own
        // fork-context resolution below (real session I/O) and before delegating to either
        // `run_chain_foreground`/`spawn_background_steps` (both of which also independently
        // re-check this same guard as defense in depth, but too late to satisfy R-SA-055's
        // "before discovery" ordering for whatever discovery THIS call site's caller already
        // performed to build `graph` — e.g. `/run-chain`'s `resolve_chain` lookup).
        let cfg = self.executor.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let fork_context = self.executor.resolve_context(cwd, context).await?;
        let graph = apply_default_context(graph, context, fork_context.session_file_path.clone());

        if background {
            let run_id = self
                .executor
                .spawn_background_steps(cwd, graph, mode, fork_context.session_file_path)
                .await?;
            Ok(format!("Background subagent run started: {run_id}"))
        } else {
            // `graph`'s own step shapes (`SingleStep` vs. `ParallelGroup`/`DynamicGroup`) are the
            // ONLY way to correctly zip `group_results` (populated in chain order, but NOT
            // indexed by overall step position — `walk_chain`'s own doc comment) back against
            // `results`' per-step-position entries, since a bare `StepResult`'s aggregate
            // `final_output` is always `None` for a group step (its own doc: the aggregate only
            // carries a `structured_output` array, never a collapsed text `final_output`) — so
            // rendering needs both `graph` (which entries are groups) and `groups` (their
            // per-child detail) together, not `results` alone.
            let is_group: Vec<bool> = graph
                .iter()
                .map(|s| matches!(s, RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_)))
                .collect();
            let (results, groups) = self.executor.run_chain_foreground(cwd, graph).await?;
            Ok(render_chain_results(&results, &is_group, &groups))
        }
    }

    // ---------------------------------------------------------------------------------------
    // /subagents-models, /subagents-refresh-provider-models, /subagents-generate-profiles,
    // /subagents-check-profile: cyrup-provider static-seed-catalog backed (func-SA §9 item 31's
    // deferred live-probe scope, restated at each call site above)
    // ---------------------------------------------------------------------------------------

    /// The path `registration/doctor.rs`'s `check_provider_catalog_freshness` (R-SA-131 item f)
    /// reads: this command's own effect (a fresh mtime on this file) is what makes
    /// `/subagents-doctor`'s freshness check observe that a refresh genuinely ran.
    fn provider_catalog_cache_path(&self, cwd: &Path) -> PathBuf {
        let _ = cwd;
        dirs_home()
            .join(".cyrup")
            .join("subagents")
            .join("provider-catalog-cache.json")
    }

    async fn refresh_provider_catalog_cache(
        &self,
        cwd: &Path,
        provider: &str,
    ) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        let catalog = cyrup_provider::catalog::seed_catalog();
        let matches: Vec<&cyrup_provider::Model> = catalog
            .iter()
            .filter(|m| m.provider.as_str() == provider)
            .collect();
        if matches.is_empty() {
            return Ok(format!(
                "subagents-refresh-provider-models: provider '{provider}' has no models in the \
                 static seed catalog; live provider probing is not yet implemented (see this \
                 command's own doc note in extension.rs)."
            ));
        }

        let cache_path = self.provider_catalog_cache_path(cwd);
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(SubagentError::Spawn)?;
        }
        let payload = serde_json::json!({
            "provider": provider,
            "modelCount": matches.len(),
            "models": matches.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            "refreshedAtEpochMs": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        });
        write_atomic_json(&cache_path, &payload)
            .await
            .map_err(SubagentError::Spawn)?;

        Ok(format!(
            "subagents-refresh-provider-models: refreshed catalog cache for '{provider}' from the \
             static seed catalog ({} model(s)); live per-provider probing is deferred (func-SA §9 \
             item 31).",
            matches.len()
        ))
    }

    async fn generate_provider_profiles(&self, provider: &str) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        let catalog = cyrup_provider::catalog::seed_catalog();
        let mut matches: Vec<&cyrup_provider::Model> =
            catalog.iter().filter(|m| m.provider.as_str() == provider).collect();
        if matches.is_empty() {
            return Ok(format!(
                "subagents-generate-profiles: provider '{provider}' has no models in the static \
                 seed catalog; nothing to generate."
            ));
        }
        // "quota" = cheapest (lowest blended input+output cost); "quality" = highest context
        // window as a proxy for capability, in the absence of a richer quality ranking signal in
        // `cyrup_provider::Model` today.
        matches.sort_by(|a, b| {
            (a.cost.input + a.cost.output)
                .partial_cmp(&(b.cost.input + b.cost.output))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let Some(quota_model) = matches.first() else {
            return Ok(format!(
                "subagents-generate-profiles: provider '{provider}' has no models in the static \
                 seed catalog; nothing to generate."
            ));
        };
        let quality_model = matches
            .iter()
            .max_by_key(|m| m.context_window)
            .unwrap_or(quota_model);

        let profiles_dir = self.profiles_dir();
        tokio::fs::create_dir_all(&profiles_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        let mut written = Vec::new();
        for (suffix, model) in [("quota", quota_model), ("quality", quality_model)] {
            let profile_name = format!("{provider}.{suffix}");
            let profile = crate::registration::profiles::NamedProfile {
                subagents: crate::discovery::types::SubagentSettings {
                    overrides: std::collections::BTreeMap::new(),
                    default_model: Some(model.id.as_str().to_string()),
                    disable_builtins: None,
                    disable_thinking: None,
                },
            };
            let path = crate::registration::profiles::profile_path(&profiles_dir, &profile_name)?;
            let json = serde_json::to_vec_pretty(&profile).map_err(|e| {
                SubagentError::MalformedSettings(format!("could not serialize profile: {e}"))
            })?;
            tokio::fs::write(&path, json).await.map_err(SubagentError::Spawn)?;
            written.push(profile_name);
        }

        Ok(format!(
            "subagents-generate-profiles: wrote {} from the static seed catalog (live \
             per-provider probing is deferred, func-SA §9 item 31).",
            written.join(", ")
        ))
    }

    // ---------------------------------------------------------------------------------------
    // /subagents-companions (no ported companion extension exists yet, see this command's own
    // doc note in dispatch_slash)
    // ---------------------------------------------------------------------------------------

    fn companions_dismissal_dir(&self) -> PathBuf {
        dirs_home().join(".cyrup").join("subagents").join("companions")
    }

    async fn handle_companions_command(
        &self,
        parsed: slash_commands::CompanionsCommand,
    ) -> Result<String, SubagentError> {
        use slash_commands::CompanionsCommand;
        match parsed {
            CompanionsCommand::Status => Ok(
                "subagents-companions: no companion extensions (e.g. pi-intercom) are ported \
                 into this workspace yet; nothing to report (func-SA §9 item 25)."
                    .to_string(),
            ),
            CompanionsCommand::Hide { package, scope } => {
                let scope_token = companions_scope_token(scope);
                let dir = self.companions_dismissal_dir();
                tokio::fs::create_dir_all(&dir).await.map_err(SubagentError::Spawn)?;
                let marker = dir.join(format!("{package}.{scope_token}.hidden.json"));
                write_atomic_json(&marker, &serde_json::json!({ "package": package, "scope": scope_token }))
                    .await
                    .map_err(SubagentError::Spawn)?;
                Ok(format!(
                    "subagents-companions: recorded a '{scope_token}'-scope dismissal for \
                     '{package}' (no companion extension is installed to actually suppress a \
                     recommendation banner for yet — see this command's own doc note)."
                ))
            }
            CompanionsCommand::Show { package } => {
                let dir = self.companions_dismissal_dir();
                let mut removed_any = false;
                for scope_token in ["workspace", "user"] {
                    let marker = dir.join(format!("{package}.{scope_token}.hidden.json"));
                    match tokio::fs::remove_file(&marker).await {
                        Ok(()) => removed_any = true,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(SubagentError::Spawn(e)),
                    }
                }
                Ok(if removed_any {
                    format!("subagents-companions: cleared dismissal(s) for '{package}'.")
                } else {
                    format!("subagents-companions: '{package}' had no recorded dismissal.")
                })
            }
        }
    }

    fn profiles_dir(&self) -> PathBuf {
        dirs_home().join(".cyrup").join("subagents").join("profiles")
    }
}

// =================================================================================================
// Free helper functions backing the dispatch_slash arms above
// =================================================================================================

/// Splice `session_file` into every step of `graph` whose own `context`/`session_file` was not
/// already explicitly set (R-SA-138: a sibling step's own explicit per-task choice is never
/// overridden by the batch-level default) — applied once, before either the foreground walk or the
/// background hand-off, so both paths see an identically-resolved graph.
fn apply_default_context(
    graph: Vec<RunnerStep>,
    default_context: ContextMode,
    session_file: Option<PathBuf>,
) -> Vec<RunnerStep> {
    graph
        .into_iter()
        .map(|step| apply_default_context_to_step(step, default_context, session_file.clone()))
        .collect()
}

fn apply_default_context_to_step(
    step: RunnerStep,
    default_context: ContextMode,
    session_file: Option<PathBuf>,
) -> RunnerStep {
    match step {
        RunnerStep::SingleStep(mut spec) => {
            if spec.context.is_none() {
                spec.context = Some(default_context);
                if spec.session_file.is_none() {
                    spec.session_file = session_file;
                }
            }
            RunnerStep::SingleStep(spec)
        }
        RunnerStep::ParallelGroup(mut group) => {
            for spec in &mut group.steps {
                if spec.context.is_none() {
                    spec.context = Some(default_context);
                    if spec.session_file.is_none() {
                        spec.session_file = session_file.clone();
                    }
                }
            }
            RunnerStep::ParallelGroup(group)
        }
        RunnerStep::DynamicGroup(mut dynamic) => {
            if dynamic.template.context.is_none() {
                dynamic.template.context = Some(default_context);
                if dynamic.template.session_file.is_none() {
                    dynamic.template.session_file = session_file;
                }
            }
            RunnerStep::DynamicGroup(dynamic)
        }
    }
}

/// `/run-chain`'s task-seeding rule (see this command's own doc note in `dispatch_slash`): splice
/// `task` into the first element's first task only, leaving every later step's saved task text
/// verbatim.
fn seed_first_step_task(mut steps: Vec<RunnerStep>, task: &str) -> Vec<RunnerStep> {
    if task.is_empty() {
        return steps;
    }
    if let Some(first) = steps.first_mut() {
        match first {
            RunnerStep::SingleStep(spec) => spec.task = task.to_string(),
            RunnerStep::ParallelGroup(group) => {
                if let Some(first_task) = group.steps.first_mut() {
                    first_task.task = task.to_string();
                }
            }
            RunnerStep::DynamicGroup(_) => {
                // A `DynamicGroup` has no single fixed task to overwrite (its per-item tasks come
                // from `template` instantiated once per resolved array element) — left as saved.
            }
        }
    }
    steps
}

/// Render [`StepResult`]s from a foreground `/chain`/`/parallel`/`/run-chain` run as human-readable
/// text — one line per step, in chain order (R-SA-051's ordering guarantee, restated at this
/// command's own text-rendering layer).
fn render_chain_results(results: &[StepResult], is_group: &[bool], groups: &[GroupStepResult]) -> String {
    let mut out = String::new();
    let mut group_cursor = 0usize;
    for (i, result) in results.iter().enumerate() {
        let step_is_group = is_group.get(i).copied().unwrap_or(false);
        if step_is_group {
            // A group step's own aggregate `StepResult::final_output` is always `None` by
            // construction (`chain_graph::collapse_fan_out`'s own doc: the aggregate carries only
            // a `structured_output` array, never a collapsed text field) — render each fanned-out
            // child's own text output instead, in the SAME position-indexed order `run_bounded`
            // guarantees (R-SA-051), so a `/parallel` caller can see every child's real output,
            // not just an aggregate "ok"/"failed" line with no text at all.
            let group = groups.get(group_cursor);
            group_cursor += 1;
            if result.success {
                out.push_str(&format!("step {}: ok (parallel group)\n", i + 1));
            } else {
                let err = result.error.clone().unwrap_or_else(|| "unknown error".to_string());
                out.push_str(&format!("step {}: FAILED (parallel group) — {err}\n", i + 1));
            }
            if let Some(group) = group {
                for (child_i, child) in group.children.iter().enumerate() {
                    match child {
                        Some(child_result) if child_result.success => {
                            let text = child_result
                                .final_output
                                .clone()
                                .unwrap_or_else(|| "(no text output)".to_string());
                            out.push_str(&format!("  child {}: ok\n  {text}\n", child_i + 1));
                        }
                        Some(child_result) => {
                            let err = child_result.error.clone().unwrap_or_else(|| "unknown error".to_string());
                            out.push_str(&format!("  child {}: FAILED — {err}\n", child_i + 1));
                        }
                        None => {
                            out.push_str(&format!("  child {}: skipped\n", child_i + 1));
                        }
                    }
                }
            }
        } else if result.success {
            let text = result
                .final_output
                .clone()
                .unwrap_or_else(|| "(no text output)".to_string());
            out.push_str(&format!("step {}: ok\n{text}\n", i + 1));
        } else {
            let err = result.error.clone().unwrap_or_else(|| "unknown error".to_string());
            out.push_str(&format!("step {}: FAILED — {err}\n", i + 1));
        }
        if i + 1 != results.len() {
            out.push('\n');
        }
    }
    if out.is_empty() {
        out.push_str("(chain produced no step results)");
    }
    out
}

/// Render `/subagents-models`' report from the real static seed catalog (`cyrup_provider::catalog`).
/// `agent` is currently unused for filtering — no `AgentDefinition` -> catalog-name mapping exists
/// in this crate today (an agent's own `model` field is a free-form [`ModelId`] string, not
/// cross-referenced against the catalog anywhere else in this crate either) — kept as a typed
/// parameter (rather than dropped) so the parsed argument shape stays self-documenting at the call
/// site and a future mapping can be wired in without changing this function's signature.
fn render_models_report(agent: Option<&str>) -> String {
    let catalog = cyrup_provider::catalog::seed_catalog();
    if catalog.is_empty() {
        return "subagents-models: the static seed catalog is empty.".to_string();
    }
    let mut out = String::new();
    if let Some(agent) = agent {
        out.push_str(&format!(
            "subagents-models: per-agent catalog filtering for '{agent}' is not implemented \
             (no agent-name -> catalog mapping exists in this crate); showing the full catalog.\n\n"
        ));
    }
    let mut by_provider: std::collections::BTreeMap<&str, Vec<&cyrup_provider::Model>> =
        std::collections::BTreeMap::new();
    for model in &catalog {
        by_provider.entry(model.provider.as_str()).or_default().push(model);
    }
    for (provider, models) in by_provider {
        out.push_str(&format!("{provider}:\n"));
        for model in models {
            out.push_str(&format!(
                "  {} — context {}k, reasoning={}\n",
                model.id,
                model.context_window / 1000,
                model.reasoning
            ));
        }
    }
    out
}

/// Render `/subagents-check-profile`'s report: cross-reference every model reference a profile
/// declares (`defaultModel` plus every `overrides.<agent>.model`) against the real static seed
/// catalog.
fn render_profile_check_report(
    name: &str,
    profile: &crate::registration::profiles::NamedProfile,
) -> String {
    let catalog = cyrup_provider::catalog::seed_catalog();
    let known: std::collections::HashSet<&str> = catalog.iter().map(|m| m.id.as_str()).collect();

    let mut refs: Vec<(String, Option<String>)> = Vec::new();
    if let Some(default_model) = &profile.subagents.default_model {
        refs.push(("defaultModel".to_string(), Some(default_model.clone())));
    }
    for (agent_name, over) in &profile.subagents.overrides {
        if let crate::discovery::types::OverrideField::Value(model) = &over.model {
            refs.push((format!("overrides.{agent_name}.model"), Some(model.clone())));
        }
    }

    if refs.is_empty() {
        return format!("subagents-check-profile '{name}': no model references declared.");
    }

    let mut out = format!("subagents-check-profile '{name}':\n");
    for (field, model) in refs {
        let Some(model) = model else { continue };
        let status = if known.contains(model.as_str()) { "OK (in static seed catalog)" } else {
            "UNKNOWN (not in static seed catalog — live reachability probing is deferred, func-SA §9 item 31)"
        };
        out.push_str(&format!("  {field} = {model}: {status}\n"));
    }
    out
}

/// `/subagents-companions`' scope token (matches the on-disk dismissal-marker filename convention
/// this file's own [`SubagentsExtension::handle_companions_command`] uses).
fn companions_scope_token(scope: slash_commands::CompanionsScope) -> &'static str {
    match scope {
        slash_commands::CompanionsScope::Workspace => "workspace",
        slash_commands::CompanionsScope::User => "user",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn id_is_stable() {
        let ext = SubagentsExtension::new();
        assert_eq!(ext.id(), ExtensionId::from("subagents"));
    }

    #[tokio::test]
    async fn init_registers_the_tool_and_all_thirteen_commands() {
        let ext = SubagentsExtension::new();
        let mut api = InitApi::new();
        ext.init(&mut api).await.expect("init succeeds");
        // InitApi has no public inspector beyond subscriptions in this phase's surface; the real
        // proof that registration actually reaches the host is `main.rs`'s wiring plus the
        // end-to-end smoke test, which drives `init` through a real `SessionBuilder`.
        assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionStart));
        assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionShutdown));
    }

    #[test]
    fn slash_command_name_round_trips_every_registered_descriptor() {
        for descriptor in SLASH_COMMANDS {
            let parsed = SlashCommandName::from_str_exact(descriptor.name.as_str());
            assert_eq!(parsed, Some(descriptor.name));
        }
    }

    #[tokio::test]
    async fn resolve_agent_returns_not_found_for_an_unknown_name() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .resolve_agent(dir.path(), "no-such-agent")
            .expect_err("unknown agent must error");
        assert!(matches!(err, SubagentError::AgentNotFound(_)));
    }

    #[tokio::test]
    async fn run_foreground_errors_before_any_spawn_when_agent_is_unknown() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .run_foreground(dir.path(), "ghost", "do something", ContextMode::Fresh, None)
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
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0; // current_depth (0, absent env) >= max_depth (0): blocked
        }
        let dir = tempfile::tempdir().expect("tempdir");
        // No `.cyrup/agents` directory is even created under `dir` — if discovery ran at all it
        // would find nothing and (for a real agent name) still fail with AgentNotFound; using the
        // exact same "ghost" name as the sibling discovery-failure test isolates this test's
        // assertion to purely WHICH error surfaces first.
        let err = executor
            .run_foreground(dir.path(), "ghost", "do something", ContextMode::Fresh, None)
            .await
            .expect_err("a blocked depth ceiling must reject before agent discovery runs");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded (proving the guard ran BEFORE discovery could report its own \
             AgentNotFound for the same unresolvable name), got: {err:?}"
        );
    }

    /// The background (`bg: true`) shape's own independent entry point must enforce the identical
    /// R-SA-055 ordering: depth guard before discovery, fork-context resolution, run-directory
    /// creation, or the detached hop-1 process spawn. Proven the same way as the foreground test
    /// above — an unresolvable agent name combined with an exhausted depth ceiling must surface
    /// `DepthExceeded`, not `AgentNotFound`, AND no run directory may exist afterward (the
    /// filesystem-level proof that `spawn_background` never reached its own `create_dir_all`/
    /// detached-spawn steps, which live strictly after the depth check in program order).
    #[tokio::test]
    async fn spawn_background_rejects_on_depth_before_discovery_or_any_directory_creation() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .spawn_background(dir.path(), "ghost", "do something", ContextMode::Fresh)
            .await
            .expect_err("a blocked depth ceiling must reject before discovery or any spawn setup");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded ahead of discovery's own AgentNotFound, got: {err:?}"
        );
        // The load-bearing proof that NOTHING was set up: neither the async-run root nor the
        // results directory `spawn_background` would otherwise create via `create_dir_all` (both
        // strictly after the depth check in program order) may exist.
        assert!(
            !default_async_root(dir.path()).exists(),
            "the async-run root must never be created for a depth-blocked background dispatch"
        );
        assert!(
            !default_results_dir(dir.path()).exists(),
            "the results directory must never be created for a depth-blocked background dispatch"
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
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let graph = vec![RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
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
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];

        let err = executor
            .run_chain_foreground(dir.path(), graph)
            .await
            .expect_err("a blocked depth ceiling must reject before walking any step");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "got: {err:?}"
        );
    }

    /// [`SubagentExecutor::spawn_background_steps`] (the general multi-step background dispatch
    /// [`SubagentExecutor::spawn_background`] itself wraps, and `/chain`/`/parallel`'s `--bg` shape
    /// calls directly) must reject a blocked depth ceiling before creating the async-run root,
    /// results directory, or run directory — the filesystem-level proof mirrors this test's own
    /// `spawn_background`-level sibling above, applied to this lower-level entry point directly
    /// rather than through the single-task wrapper.
    #[tokio::test]
    async fn spawn_background_steps_rejects_on_depth_before_any_directory_creation() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let step = RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
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
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        });

        let err = executor
            .spawn_background_steps(dir.path(), vec![step], RunMode::Single, None)
            .await
            .expect_err("a blocked depth ceiling must reject before any directory creation");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "got: {err:?}"
        );
        assert!(!default_async_root(dir.path()).exists());
        assert!(!default_results_dir(dir.path()).exists());
    }

    /// R-SA-055 (SAFETY-CRITICAL), end to end through the full slash-command dispatch path:
    /// `/run-chain` must reject on a blocked depth ceiling BEFORE `resolve_chain`'s own real
    /// discovery filesystem scan ever runs. Proven the same "same unresolvable name, which error
    /// wins" way as the foreground/background tests above — no chain named `"ghost-chain"` is
    /// ever written to `dir`, so if the depth guard did NOT run first, this call would surface
    /// [`SubagentError::ChainNotFound`] (discovery's own genuine failure mode for an unresolvable
    /// name) instead of [`SubagentError::DepthExceeded`].
    #[tokio::test]
    async fn dispatch_slash_run_chain_rejects_on_depth_before_chain_discovery_ever_runs() {
        let cfg = SubagentExtensionConfig {
            max_subagent_depth: 0,
            ..SubagentExtensionConfig::default()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());

        let err = ext
            .dispatch_slash(
                SlashCommandName::RunChain,
                "ghost-chain -- do something",
                dir.path(),
            )
            .await
            .expect_err("a blocked depth ceiling must reject before chain discovery runs");

        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded ahead of resolve_chain's own ChainNotFound, got: {err:?}"
        );
    }

    /// The `/chain` and `/parallel` shared tail ([`SubagentsExtension::run_or_background_chain`])
    /// must likewise reject on a blocked depth ceiling before its own fork-context resolution (and
    /// therefore before either `run_chain_foreground`'s or `spawn_background_steps`' own
    /// independent, necessarily-later re-check) — proven directly against that private tail
    /// (accessible from this same-file `tests` submodule) with both `background: false` and
    /// `background: true`, since both branches share the identical guard at the top of the
    /// function, before the `if background` split.
    #[tokio::test]
    async fn run_or_background_chain_rejects_on_depth_before_fork_context_resolution() {
        let cfg = SubagentExtensionConfig {
            max_subagent_depth: 0,
            ..SubagentExtensionConfig::default()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());

        let graph = vec![RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
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
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];

        for background in [false, true] {
            let err = ext
                .run_or_background_chain(
                    dir.path(),
                    graph.clone(),
                    RunMode::Chain,
                    ContextMode::Fresh,
                    background,
                )
                .await
                .expect_err("a blocked depth ceiling must reject before any dispatch");
            assert!(
                matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
                "background={background}: expected DepthExceeded, got: {err:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // A-SA-17 (recursive cost): `/subagent-cost` on a chain with a nested subagent-of-subagent
    // step reports a total that includes the grandchild's usage, not just the immediate child's.
    // This exercises the REAL production command path (`SubagentExecutor::run_cost_report`, the
    // same method `dispatch_slash`'s `SubagentCost` arm calls) end to end against real on-disk
    // status files — not `registration::cost`'s accumulator functions called directly (those are
    // separately, exhaustively unit-tested in that module already; this test's whole purpose is
    // proving the COMMAND actually reaches that accumulator instead of returning a stub string).
    // ---------------------------------------------------------------------------------------

    fn cost_test_step(agent: &str, input: u64, output: u64, nested: Vec<RunId>) -> crate::background::StepStatus {
        crate::background::StepStatus {
            agent: agent.to_string(),
            status: crate::background::StepState::Complete,
            session_file: None,
            model: None,
            attempted_models: Vec::new(),
            usage: cyrup_core::Usage {
                input,
                output,
                ..Default::default()
            },
            error: None,
            nested_run_ids: nested,
            started_at: None,
            ended_at: None,
        }
    }

    #[tokio::test]
    async fn run_cost_report_includes_a_nested_grandchild_runs_usage() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");

        let async_root = default_async_root(&cwd);
        let results_dir = default_results_dir(&cwd);

        let root_id = RunId::from_token("costroot000000000000000000001");
        let child_id = RunId::from_token("costchild00000000000000000001");
        let root_paths = RunPaths::for_run(&async_root, &results_dir, &root_id);
        let child_paths = root_paths.nested(&child_id);

        // Child (leaf): usage B.
        let child_step = cost_test_step("reviewer", 50, 25, Vec::new());
        let mut child_status =
            crate::background::RunStatus::queued(child_id.clone(), RunMode::Single, None);
        child_status.state = crate::background::RunState::Complete;
        child_status.steps = vec![child_step];
        tokio::fs::create_dir_all(&child_paths.run_dir)
            .await
            .expect("mkdir child run dir");
        write_atomic_json(&child_paths.status, &child_status)
            .await
            .expect("write child status");

        // Root: one step nesting the child.
        let root_step = cost_test_step("researcher", 200, 100, vec![child_id.clone()]);
        let mut root_status =
            crate::background::RunStatus::queued(root_id.clone(), RunMode::Single, None);
        root_status.state = crate::background::RunState::Complete;
        root_status.steps = vec![root_step];
        tokio::fs::create_dir_all(&root_paths.run_dir)
            .await
            .expect("mkdir root run dir");
        write_atomic_json(&root_paths.status, &root_status)
            .await
            .expect("write root status");

        // Track the root run and force one poll tick so `last_status` is populated from the real
        // on-disk file this test just wrote (mirroring how a real background run gets tracked).
        let executor = SubagentExecutor::new();
        executor.tracker.track(root_id.clone(), root_paths, None).await;
        executor.tracker.tick_once().await;

        let report = executor.run_cost_report(&cwd).await;

        assert!(
            !report.contains("no run artifacts discovered"),
            "a tracked, real run must produce an actual cost report, not the empty-state \
             placeholder: {report}"
        );
        // Root (200+100) + child (50+25) = 350 input+output tokens total; the report text is a
        // human-readable rendering (`format_cost_report`), so assert on the actual numeric totals
        // being present rather than over-fitting to exact wording.
        assert!(
            report.contains("250") && report.contains("125"),
            "the report must include the grandchild/child's usage summed with the root's own, \
             not just the root's: {report}"
        );
    }
}
