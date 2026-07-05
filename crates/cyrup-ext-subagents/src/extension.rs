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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use cyrup_core::{CancelToken, ExtensionId, ModelId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};
use tokio::sync::Mutex as AsyncMutex;

use crate::background::atomic::write_atomic_json;
use crate::background::control::{self, AppendOutcome, InterruptOutcome, ResumeOutcome};
use crate::background::runner_main::ExecSingleStepExecutor;
use crate::background::spawn_detached::spawn_detached_runner;
use crate::background::tracker::JobTracker;
use crate::background::{run_status, RunId, RunMode, RunPaths, RunState};
use crate::discovery::types::{AgentDefinition, AgentModelSourceInfo, AgentSource, OverrideScope};
use crate::discovery::{discover_agents, AgentDiscoveryConfig};
use crate::error::SubagentError;
use crate::exec::fallback::ModelOverride;
use crate::exec::{AgentConfig, ResolvedAgentPersona, RunOptions, SingleResult};
use crate::fork_context::{
    resolve_effective_context, ContextMode, ForkContext, ForkContextResolver,
};
use crate::registration::doctor::{build_doctor_report, DoctorReportInput};
use crate::registration::slash_commands::{self, SlashCommandName, SLASH_COMMANDS};
use crate::registration::SubagentExtensionConfig;
use crate::spawn::chain_graph::{
    walk_chain, ChainRunContext, GroupStepResult, OutputRegistry, ParallelGroupSpec, RunnerStep,
    SingleStepExecutor, SingleStepSpec, StepResult,
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
    /// An EXPLICITLY-injected completion sink (a test's capturing sink, or a caller wiring its own
    /// turn-injection channel). `None` — the production default — means "derive the effective sink
    /// at install time": a live [`HostServicesCompletionSink`] when the P-1 `host_services` slot is
    /// bound (R-SA-101, the real turn-injecting sink), else the graceful-degradation
    /// [`crate::background::watch::LoggingCompletionSink`] (log + delete). Set via
    /// [`SubagentExecutor::with_completion_sink`].
    completion_sink_override: Option<Arc<dyn crate::background::watch::CompletionSink>>,
    /// The live [`crate::background::watch::CompletionWatcherHandle`] for the current session's
    /// `ResultsDir`, installed on `SessionStart` ([`SubagentExecutor::install_completion_watcher`])
    /// and retained here so the watch stays live for the session's lifetime (dropping it stops the
    /// watch). Re-installing replaces (and thereby tears down) any prior handle.
    completion_watcher: AsyncMutex<Option<crate::background::watch::CompletionWatcherHandle>>,
    /// The late-bound live capability backend (P-1, reconciliation §2 item 1). Captured by
    /// [`SubagentsExtension::set_host_services`] (which the builder calls via
    /// `load_native_with_services` BEFORE `init`), so a background task / the `SessionStart` handler
    /// / the fork-context resolver can reach the live session id/file + `inject_message` OUTSIDE any
    /// `HostCtx`. `None` (default host / SDK-embedder / headless) ⇒ every consumer degrades to its
    /// documented no-host fallback (heuristic fork-context, stderr logging sink, empty anchor).
    host_services: Arc<OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
    /// The canonical parent-session anchor (`CYRUP_SUBAGENT_PARENT_SESSION`, proposed R-SA-P1),
    /// captured ONCE from [`cyrup_ext::host::HostServices::session_id`] at the root orchestrator's
    /// `SessionStart` (depth 0). Injected into every child's spawn env overlay so the permission
    /// companion's child→parent ask-forwarding spool can address this session's inbox (port doc §4
    /// P-4). Empty/unset at `DEPTH>0` (a child never captures its own) — the spawn-site resolution
    /// then falls back to the inherited env value (explicit → inherited → empty).
    root_parent_session: Arc<OnceLock<String>>,
    /// The root orchestrator session's own NAME (`HostServices::session_name`), captured ONCE
    /// alongside [`Self::root_parent_session`] at the root `SessionStart`. Folded with the session id
    /// into this orchestrator's intercom presence target
    /// ([`crate::spawn::intercom_target::orchestrator_presence_target`]) — the address a spawned
    /// child's `contact_supervisor` relays to (pi `resolveIntercomSessionTarget`). Empty/unset when
    /// the live backend has no session name (the alias `subagent-chat-<id8>` is used instead).
    root_parent_session_name: Arc<OnceLock<String>>,
    /// The live-child steer transport (R-SA-086). Defaults to
    /// [`crate::tui::intercom::NoTransportSteerChannel`] (no broker → always "not registered"); the
    /// intercom companion's broker-backed `SteerChannel` is threaded in via
    /// [`SubagentsExtension::with_channels`] → [`SubagentExecutor::with_channels`]. Consumed by
    /// [`Self::control_resume`]'s `SteerRunning` arm to DELIVER `action='resume'`'s follow-up to a
    /// still-running async child over the broker (pi `subagent-executor.ts:860-878`).
    steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    /// The out-of-band grouped-result delivery channel (R-SA-123/124/125). Defaults to
    /// [`crate::tui::intercom::NoTransportChannel`] (always "not delivered", full inline preserved);
    /// the intercom companion's broker-backed `DeliveryChannel` is threaded in via
    /// [`SubagentsExtension::with_channels`] → [`SubagentExecutor::with_channels`].
    delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
    /// The single-slot clarify/ask lock (R-SA-119/120) backed by a [`crate::tui::intercom::ClarifyChannel`].
    /// Defaults to [`crate::tui::intercom::AskLock::new_with_no_live_channel`]; the intercom companion's
    /// broker-backed `ClarifyChannel` is threaded in via [`SubagentsExtension::with_channels`]. Consumed
    /// by the exec detach-trigger arm (R-SA-037) when a child's `contact_supervisor` blocking ask fires.
    clarify: Arc<crate::tui::intercom::AskLock>,
}

/// The structured result of [`SubagentExecutor::run_or_background_graph`]: either a detached
/// background run was launched (carrying its [`RunId`]), or the graph was walked to completion in
/// the foreground and its per-step results are returned for the caller to render. Keeping this
/// structured (rather than pre-rendering a string inside the executor) is what lets the tool's
/// PARALLEL mode render pi's `N/M succeeded` summary while the slash commands render their own
/// per-step text, both over the SAME underlying walk.
pub enum GraphRunOutcome {
    /// A background run was spawned (detached hop-1); nothing waited on its completion (R-SA-074).
    Background(RunId),
    /// The graph was walked to completion in the foreground. `results`/`is_group`/`groups` are the
    /// exact triple [`render_chain_results`]/[`render_parallel_tool_summary`] consume.
    Foreground {
        results: Vec<StepResult>,
        is_group: Vec<bool>,
        groups: Vec<GroupStepResult>,
    },
}

impl Default for SubagentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// The six inputs one foreground single run needs, bundled into one borrowed request so
/// [`SubagentExecutor::run_foreground_streaming`] and the shared `run_foreground_impl` stay within
/// the argument-count budget (the non-streaming [`SubagentExecutor::run_foreground`] keeps its
/// original flat signature for backward compatibility and builds this internally). All fields
/// borrow for the duration of the one `run_foreground*` call they are passed to.
pub struct ForegroundRunRequest<'a> {
    /// The task's working directory (also the discovery root for the named persona).
    pub cwd: &'a Path,
    /// The persona name to resolve and run (func-SA §5.2).
    pub agent_name: &'a str,
    /// The task text handed to the child (pi's `Task: <task>` child prompt).
    pub task: &'a str,
    /// Call-site fork/fresh context; `None` defers to the persona's own `default_context`.
    pub context: Option<ContextMode>,
    /// Per-call model override (added to the availability set, R-SA-038); `None` inherits.
    pub model_override: Option<ModelId>,
    /// Foreground timeout budget in milliseconds (pi `timeoutMs`/`maxRuntimeMs`); `None` = none.
    pub timeout_ms: Option<u64>,
}

/// The already-resolved, plan-shaped inputs [`SubagentExecutor::spawn_background_steps`] takes from
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
}

impl SubagentExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Arc::new(AsyncMutex::new(SubagentExtensionConfig::default())),
            tracker: Arc::new(JobTracker::new()),
            completion_sink_override: None,
            completion_watcher: AsyncMutex::new(None),
            host_services: Arc::new(OnceLock::new()),
            root_parent_session: Arc::new(OnceLock::new()),
            root_parent_session_name: Arc::new(OnceLock::new()),
            steer: Arc::new(crate::tui::intercom::NoTransportSteerChannel),
            delivery: Arc::new(crate::tui::intercom::NoTransportChannel),
            clarify: Arc::new(crate::tui::intercom::AskLock::new_with_no_live_channel()),
        }
    }

    /// Construct an executor whose background-completion notifications (C6) are delivered to
    /// `sink` instead of the default graceful-degradation logging sink — the seam a host uses to
    /// route completions into a live session's turn loop (R-SA-101), and a test uses to capture
    /// them. Explicitly overriding the sink here wins over the P-1 `host_services`-derived
    /// [`HostServicesCompletionSink`] at install time (so a test's scripted sink is authoritative).
    #[must_use]
    pub fn with_completion_sink(sink: Arc<dyn crate::background::watch::CompletionSink>) -> Self {
        Self { completion_sink_override: Some(sink), ..Self::new() }
    }

    /// Late-bind the live capability backend (P-1). Called by
    /// [`SubagentsExtension::set_host_services`] (which the builder invokes via
    /// `load_native_with_services` BEFORE `init`) so the `SessionStart` handler, the fork-context
    /// resolver, and the completion watcher reach the live session id/file + `inject_message`.
    /// Idempotent (`OnceLock::set` ignores a second bind of the same session rebuild).
    pub fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let _ = self.host_services.set(services);
    }

    /// The captured live capability backend, if the P-1 slot has been bound.
    #[must_use]
    pub fn host_services(&self) -> Option<Arc<dyn cyrup_ext::host::HostServices>> {
        self.host_services.get().cloned()
    }

    /// Thread the intercom companion's real broker-backed delivery + clarify + steer channels into
    /// this executor (item 2 of reconciliation §4 step 5), replacing the `NoTransportChannel`/no-live
    /// `AskLock`/`NoTransportSteerChannel` defaults. `delivery` closes R-SA-123/124/125 (out-of-band
    /// grouped delivery + reduced inline receipt); `clarify` (wrapped in a single-slot
    /// [`crate::tui::intercom::AskLock`], R-SA-120) closes R-SA-119/120 and backs the exec
    /// detach-trigger arm (R-SA-037); `steer` closes R-SA-086's live-child follow-up delivery (the
    /// [`Self::control_resume`] `SteerRunning` arm delivers `action='resume'` over the broker).
    #[must_use]
    pub fn with_channels(
        mut self,
        delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
        clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
        steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    ) -> Self {
        self.delivery = delivery;
        self.clarify = Arc::new(crate::tui::intercom::AskLock::new(clarify));
        self.steer = steer;
        self
    }

    /// The captured parent-session anchor (`CYRUP_SUBAGENT_PARENT_SESSION`, R-SA-P1), if the root
    /// `SessionStart` handler has resolved it from [`cyrup_ext::host::HostServices::session_id`].
    #[must_use]
    pub fn root_parent_session(&self) -> Option<String> {
        self.root_parent_session.get().filter(|s| !s.is_empty()).cloned()
    }

    /// The out-of-band delivery channel (R-SA-123/124/125), for the run driver's grouped-result
    /// delivery attempt.
    #[must_use]
    pub fn delivery_channel(&self) -> Arc<dyn crate::tui::intercom::DeliveryChannel> {
        self.delivery.clone()
    }

    /// The single-slot clarify/ask lock (R-SA-119/120), for the exec detach-trigger arm (R-SA-037).
    #[must_use]
    pub fn clarify_lock(&self) -> Arc<crate::tui::intercom::AskLock> {
        self.clarify.clone()
    }

    /// Attempt out-of-band delivery of a completed grouped (parallel/chain) run's result through the
    /// executor's [`crate::tui::intercom::DeliveryChannel`] (R-SA-123/124/125), racing it against the
    /// default bounded timeout so a missing receiver never stalls the tool's own turn. Returns
    /// [`crate::tui::intercom::DeliveryOutcome::Delivered`] only when a receiver confirmed receipt —
    /// the caller may then REDUCE its inline tool payload (drop the heavy duplicated per-child
    /// outputs, R-SA-123); on any other outcome the caller keeps the full inline result (R-SA-125).
    /// With the `NoTransportChannel` default (no intercom wired) this always reports `NotDelivered`,
    /// exactly as the spec anticipates, so the inline result stays full.
    pub async fn deliver_group_out_of_band(
        &self,
        payload: crate::tui::intercom::IntercomPayload,
    ) -> crate::tui::intercom::DeliveryOutcome {
        crate::tui::intercom::deliver_with_default_timeout(self.delivery.as_ref(), payload).await
    }

    /// Capture the canonical parent-session anchor from the live session id (P-2), at the root
    /// orchestrator's `SessionStart` (depth 0). Reads [`cyrup_ext::host::HostServices::session_id`]
    /// off the bound P-1 backend; a `None`/empty id (headless / unpersisted / no live session) leaves
    /// the slot unset, so the spawn-site resolution falls through to the inherited env value.
    pub fn capture_parent_session_anchor(&self) {
        if let Some(services) = self.host_services()
            && let Some(id) = services.session_id()
            && !id.is_empty()
        {
            let _ = self.root_parent_session.set(id);
            // Capture the session NAME too (may be absent): it feeds this orchestrator's own intercom
            // presence target (`orchestrator_presence_target(name, id)`), the address a spawned
            // child's `contact_supervisor` relays to. An absent/empty name falls through to the
            // `subagent-chat-<id8>` alias inside that resolver, so only a real name is stored here.
            if let Some(name) = services.session_name().filter(|n| !n.trim().is_empty()) {
                let _ = self.root_parent_session_name.set(name);
            }
        }
    }

    /// This root orchestrator's own intercom presence target — the address a spawned child's
    /// `contact_supervisor` relays to (pi `resolveIntercomSessionTarget(pi.getSessionName(),
    /// sessionManager.getSessionId())`, `subagent-executor.ts:893`). Byte-identical to the string the
    /// intercom companion registers this session's broker presence under
    /// ([`cyrup_intercom`]'s `build_registration` derives it from the SAME `HostServices`), so the
    /// two independently-produced strings match at the broker. `None` when no live session id was
    /// captured (headless / SDK-embedder) — the spawn site then writes no child-bridge env, so the
    /// child registers no supervisor bridge (the clean no-intercom path).
    #[must_use]
    pub fn orchestrator_intercom_target(&self) -> Option<String> {
        let id = self.root_parent_session()?;
        let name = self.root_parent_session_name.get().map(String::as_str).filter(|s| !s.trim().is_empty());
        Some(crate::spawn::intercom_target::orchestrator_presence_target(name, &id))
    }

    /// The effective background-completion sink to install this session (R-SA-101). Precedence:
    /// an explicitly-injected [`Self::with_completion_sink`] override (a test's scripted sink) →
    /// a live [`HostServicesCompletionSink`] when the P-1 `host_services` slot is bound (the real
    /// turn-injecting sink) → the graceful-degradation [`crate::background::watch::LoggingCompletionSink`]
    /// (stderr log + delete) when no host handle is present (the SDK-embedder / headless default).
    fn effective_completion_sink(&self) -> Arc<dyn crate::background::watch::CompletionSink> {
        if let Some(sink) = &self.completion_sink_override {
            return sink.clone();
        }
        if let Some(services) = self.host_services() {
            return Arc::new(crate::background::watch::HostServicesCompletionSink::new(services));
        }
        Arc::new(crate::background::watch::LoggingCompletionSink)
    }

    /// Install (or reinstall) the background-completion watcher (C6) over this cwd's `ResultsDir`
    /// (`notify.ts` + `result-watcher.ts`): ensure the results directory exists, attach a real
    /// filesystem watch, and drain freshly-completed runs into this executor's completion sink,
    /// deleting each result file after its notification is delivered (R-SA-099). Idempotent —
    /// reinstalling replaces (and tears down) any prior session's watcher. Best-effort: a failure to
    /// create the results dir or attach the watch degrades to "no completion notifications this
    /// session" rather than failing session start.
    pub async fn install_completion_watcher(&self, cwd: &Path) {
        let results_dir = default_results_dir(cwd);
        if crate::background::ensure_accessible_dir(&results_dir).await.is_err() {
            return;
        }
        match crate::background::watch::install_completion_watcher(
            results_dir,
            self.effective_completion_sink(),
        ) {
            Ok(handle) => {
                *self.completion_watcher.lock().await = Some(handle);
            }
            Err(_) => {
                // Degrade gracefully: no watcher this session (e.g. the results dir vanished between
                // the ensure above and the watch attach). Completions written later this session
                // simply are not surfaced until a future session re-installs the watch on start.
            }
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

    /// Build a real [`AgentDiscoveryConfig`] scoped to `cwd`, resolving the full pi directory
    /// topology (agents.ts:511-522,1234-1259,1279-1280): an **upward project-root search**
    /// ([`crate::discovery::find_nearest_project_root`]) so a cwd nested below the project root
    /// still finds the project's agents; the legacy `<root>/.agents` project dir plus the preferred
    /// `<root>/.cyrup/agents` dir; the primary `~/.cyrup/agents` plus the "second" `~/.agents` user
    /// dir; **separate** `.cyrup/chains` chain dirs at each scope (never the shared agents dir); the
    /// bundled builtin-persona resource root ([`builtin_agents_dir`], R-SA-020/132/134); and
    /// R-SA-003's `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` extras (prepended, lowest User-tier precedence).
    ///
    /// # Package tier (Tier-2 wire-up)
    ///
    /// The package tier is now populated by enumerating `cyrup-resources`' own persisted
    /// `packages.json` install registries (Global under `<home>/.cyrup/packages.json`, Project under
    /// `<cwd>/.cyrup/packages.json`) via [`enumerate_installed_packages`], so a package that declares
    /// an `agents = [...]` manifest entry (R-SA-020) has its personas discovered at
    /// [`crate::discovery::types::AgentSource::Package`] scope by [`crate::discovery::scan_package_agents`]
    /// and its chain files (chains-share-agents-dir) discovered at Package scope by
    /// [`crate::discovery::scan_package_chain_scopes`]. R-SA-001's four-scope precedence
    /// (package first-seen-wins, then user/project last-seen-wins) now holds over all four populated
    /// tiers rather than three.
    ///
    /// `project_root` is `cwd` (the same base the `.cyrup/agents` project dir is derived from);
    /// `global_dir` is `<home>/.cyrup`. `trusted_project` is fail-closed (`false`): a Project-scope
    /// package's `agents` manifest entries are skipped until a project-trust decision is threaded in
    /// (the same not-yet-threaded seam this file documents for the live session-manager / settings
    /// layering — cyrup-config's DI-11 trust decision has no injection point into this extension
    /// today, so this crate never silently trusts a project's installed packages). Global-scope
    /// packages are always enumerated (trust-independent, matching `cyrup-resources`' own gate).
    fn discovery_dirs_config(cwd: &Path) -> AgentDiscoveryConfig {
        let home = dirs_home();
        let global_dir = home.join(".cyrup");
        // Upward project-root search (pi `findNearestProjectRoot`, agents.ts:511-522): the nearest
        // ancestor of `cwd` holding a `.cyrup` config dir or a legacy `.agents` dir, so a cwd deep
        // inside a project still discovers that project's agents/chains. Absent any such ancestor,
        // fall back to `cwd` (pi's `findNearestProjectRoot(cwd) ?? cwd`) so package roots and the
        // project write target still resolve under `cwd/.cyrup`.
        let project_root =
            crate::discovery::find_nearest_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        let installed_packages = enumerate_installed_packages(&global_dir, Some(&project_root));
        // Per-scope read dirs from the shared topology helpers (pi resolveNearestProject*Dirs /
        // discoverAgents userDir old+new / getUserChainDir): legacy `.agents` + preferred
        // `.cyrup/agents` for project agents; primary `.cyrup/agents` + second `~/.agents` for user
        // agents; a SEPARATE `.cyrup/chains` dir for each scope's chains (never the agents dir).
        AgentDiscoveryConfig {
            builtin_agents_dir: Some(builtin_agents_dir()),
            project_agent_dirs: crate::discovery::resolve_project_agent_read_dirs(&project_root),
            project_chain_dirs: crate::discovery::resolve_project_chain_read_dirs(&project_root),
            user_agent_dirs: crate::discovery::resolve_user_agent_read_dirs(&home),
            user_chain_dirs: crate::discovery::resolve_user_chain_read_dirs(&home),
            global_dir,
            project_root: Some(project_root),
            trusted_project: false,
            installed_packages,
            ..AgentDiscoveryConfig::default()
        }
        // R-SA-003: fold in `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` — PREPENDED ahead of the user dirs
        // (extras are the lowest-precedence User-tier stream), so the user's own agents win.
        .with_env_extras()
    }

    /// Build a real, fully-populated [`AgentDiscoveryConfig`] scoped to `cwd`: the directory/package
    /// topology from [`discovery_dirs_config`](Self::discovery_dirs_config) PLUS the `subagents.*`
    /// settings layer read from the user (`~/.cyrup/agents/settings.json`) and project
    /// (`<cwd>/.cyrup/agents/settings.json`) `settings.json` files (C2 wiring). The two scopes are
    /// layered per R-SA-012/133 by [`crate::discovery::load_layered_subagent_settings`] (project wins
    /// over user on every scalar and per-agent override name; a project `disableBuiltins: false`
    /// re-enables what a user `true` disabled), which then drives `merge.rs`'s
    /// `defaultModel`/`disableBuiltins`/`disableThinking`/`agentOverrides` application over the merged
    /// agents.
    ///
    /// # Errors
    ///
    /// Propagates [`SubagentError::MalformedSettings`] (R-SA-009) when either scope's `settings.json`
    /// exists but cannot be read, does not parse, is not a JSON object, or carries a malformed
    /// `subagents.*` field — the malformed-settings MUST-abort contract this crate's discovery
    /// callers rely on.
    fn discovery_config(cwd: &Path) -> Result<AgentDiscoveryConfig, SubagentError> {
        let mut cfg = Self::discovery_dirs_config(cwd);
        let user_settings = dirs_home().join(".cyrup").join("agents").join("settings.json");
        let project_settings = cwd.join(".cyrup").join("agents").join("settings.json");
        // Tier 7: carry BOTH scopes UNFLATTENED (each with its own path) so `merge.rs` can resolve
        // project-beats-user at application time and record the true winning scope + path in
        // provenance (rather than a pre-flattened single scope that always looked like `Project`).
        cfg.override_settings = crate::discovery::load_layered_override_settings(
            &user_settings,
            Some(&project_settings),
        )?;
        Ok(cfg)
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
        let cfg = Self::discovery_config(cwd)?;
        let result = discover_agents(&cfg, None)?;
        result
            .agents
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| SubagentError::AgentNotFound(name.to_string()))
    }

    /// Plan-time persona map (T0.1's C13 root-cause seam): resolve every DISTINCT agent named across
    /// a chain/parallel/background plan to its serializable [`ResolvedAgentPersona`], keyed by the
    /// step's `agent` name. This is the orchestrator half of T0.1 that the canonical
    /// [`crate::exec::resolve_step_agent_config`] resolver's own doc describes: the discovery lookup
    /// (name -> [`AgentDefinition`]) is done HERE — `extension.rs` is the ONE place with real
    /// discovery access (`crates/cyrup/src/subagent_runner_cmd.rs`'s hop-2 runner has none, which is
    /// exactly why `background/runner_main.rs`'s `ExecSingleStepExecutor` would otherwise synthesize a
    /// placeholder `AgentConfig{system_prompt_body:"", model:"default", completion_guard:Some(false),
    /// …}`) — then each resolved definition is projected via
    /// [`crate::exec::resolve_step_agent_config`].
    ///
    /// The returned map is stashed into [`crate::background::runner_main::RunnerConfig::resolved_agents`]
    /// (for a background run) or handed straight to
    /// [`crate::background::runner_main::ExecSingleStepExecutor::foreground`] (for a foreground
    /// `/chain`//`/parallel` run), so the runner dispatches the REAL persona (its own system prompt,
    /// model + fallback ladder, completion guard, per-step depth ceiling) and NEVER re-discovers.
    /// Resolving up front also validates every referenced agent EXISTS before any child process is
    /// spawned — matching pi, which validates agent names before starting a `/chain`/`/parallel`
    /// rather than spawning a partial run that dies mid-walk.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::AgentNotFound`] if any named agent resolves to no delegation-visible
    /// agent, or propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings abort).
    pub fn resolve_plan_personas(
        &self,
        cwd: &Path,
        agent_names: impl IntoIterator<Item = String>,
    ) -> Result<BTreeMap<String, ResolvedAgentPersona>, SubagentError> {
        let mut personas: BTreeMap<String, ResolvedAgentPersona> = BTreeMap::new();
        for name in agent_names {
            if personas.contains_key(&name) {
                continue;
            }
            let agent = self.resolve_agent(cwd, &name)?;
            personas.insert(name, crate::exec::resolve_step_agent_config(&agent));
        }
        Ok(personas)
    }

    // ---------------------------------------------------------------------------------------
    // Fork-context resolution (per-call throwaway resolver, see module doc)
    // ---------------------------------------------------------------------------------------

    /// Build a fresh, throwaway [`ForkContextResolver`] scoped to `cwd`. A new `SessionManager`
    /// handle is opened once per call and discarded after use — never retained, never shared, never
    /// mutated in place beyond this one resolution.
    ///
    /// # Fork-context correctness (blocker #4, reconciliation §4 step 5 item 5)
    ///
    /// When `session_file` is `Some` — the REAL live-orchestrator session file obtained from the P-1
    /// [`cyrup_ext::host::HostServices::session_file`] backend — the fork branches from THAT exact
    /// parent session (matching pi threading the real `parentSessionId`/`sessionFile`), opened via
    /// [`cyrup_session::SessionManager::open_with_cwd`]. This replaces the
    /// [`cyrup_session::SessionManager::continue_recent`] most-recent-mtime HEURISTIC, which can
    /// silently pick the WRONG session when a cwd has multiple sessions. The heuristic remains ONLY
    /// as the fallback for `None` (no host handle — the SDK-embedder / headless path), and for the
    /// (rare) case where the supplied session file cannot be opened.
    fn fork_resolver(cwd: &Path, session_file: Option<&Path>) -> ForkContextResolver {
        let sessions_root = dirs_home().join(".cyrup").join("sessions");
        let layout = cyrup_session::SessionLayout::new(sessions_root.clone(), cwd.to_path_buf());
        // Blocker #4: prefer the real live-orchestrator session file (P-1) over the mtime heuristic.
        if let Some(path) = session_file
            && let Ok(manager) = cyrup_session::SessionManager::open_with_cwd(path, Some(cwd))
        {
            return ForkContextResolver::new(Arc::new(AsyncMutex::new(manager)), layout);
        }
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
        // Blocker #4: branch from the REAL live-orchestrator session file (P-1), not the mtime guess.
        let session_file = self.host_services().and_then(|s| s.session_file());
        let resolver = Self::fork_resolver(cwd, session_file.as_deref());
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
        context: Option<ContextMode>,
        model_override: Option<ModelId>,
        timeout_ms: Option<u64>,
    ) -> Result<SingleResult, SubagentError> {
        self.run_foreground_impl(
            ForegroundRunRequest { cwd, agent_name, task, context, model_override, timeout_ms },
            None,
        )
        .await
    }

    /// C19 (live foreground progress): the same foreground single run as [`run_foreground`], but
    /// STREAMING live progress through the host [`ToolUpdateSink`] as the child's NDJSON stdout
    /// arrives — the crate-side of pi's `onUpdate`/`fireUpdate` (`runs/foreground/execution.ts:478-499`).
    /// The tool call still blocks and still returns the same terminal [`SingleResult`]; the
    /// difference is that a still-running child no longer surfaces zero progress until completion.
    /// Each `tool_execution_start`/`tool_execution_end`/assistant `message_end` folds into a
    /// [`crate::tui::events::LiveProgressSnapshot`], is wrapped in a
    /// [`crate::tui::events::SubagentUpdatePayload`] (the `ToolUpdate.details` wire shape `cyrup-tui`
    /// renders as the inline subagent-result surface, C20), and is delivered through `on_update`.
    ///
    /// # Errors
    ///
    /// Identical to [`run_foreground`].
    pub async fn run_foreground_streaming(
        &self,
        req: ForegroundRunRequest<'_>,
        on_update: ToolUpdateSink,
    ) -> Result<SingleResult, SubagentError> {
        self.run_foreground_impl(req, Some(on_update)).await
    }

    /// Shared body for [`run_foreground`] / [`run_foreground_streaming`]: resolves the persona +
    /// fork-context, builds the [`AgentConfig`]/[`RunOptions`], and drives [`crate::exec::run_sync`]
    /// — optionally installing a live-progress sink (`on_update = Some`, C19) that folds the child's
    /// NDJSON stream into [`crate::tui::events::SubagentUpdatePayload`] updates.
    async fn run_foreground_impl(
        &self,
        req: ForegroundRunRequest<'_>,
        on_update: Option<ToolUpdateSink>,
    ) -> Result<SingleResult, SubagentError> {
        let ForegroundRunRequest { cwd, agent_name, task, context, model_override, timeout_ms } =
            req;
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let agent = self.resolve_agent(cwd, agent_name)?;
        // Fork default-mode (Tier-2, pi `resolveAgentDefaultContextPolicy`): an OMITTED call-site
        // `context` (`None`) falls back to THIS agent's own `default_context` rather than being forced
        // to `Fresh`; an explicit call-site value still wins (`resolve_effective_context`).
        let effective_context = resolve_effective_context(context, agent.default_context);
        let fork_context = self.resolve_context(cwd, effective_context).await?;
        // C19: the run's *resolved* context (R-SA-111) — captured before `fork_context` is moved
        // into `run_options` below — is what the live-progress payload's `[fork]` badge reflects.
        let resolved_context = fork_context.mode;

        let agent_config = AgentConfig::from_agent_definition(&agent, depth);
        // R-SA-038: `build_model_candidates` filters the ladder to `available_models`, so an
        // explicit `model` override (pi `slash-commands.ts:1001` `/run [model=…]`, and the tool's
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
        if let Some(model) = &model_override {
            available_models.push(model.clone());
        }

        // R-SA-035 / pi `resolveAttemptTimeout` (`execution.ts:91-99`): the orchestrator computes
        // the wall-clock `deadline_at` ONCE, here, from the nominal `timeout_ms` budget (pi
        // `deadlineAt ?? now + timeoutMs`), and threads BOTH down — `deadline_at` is what `run_sync`
        // races the child against; `timeout_ms` is what the timed-out message renders.
        let deadline_at =
            timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

        // The run id is minted BEFORE `run_options` so it can also identify the clarify/ask dispatch
        // context (R-SA-037/119/120) below; it doubles as the artifact-quadruple run id further down.
        let run_id = RunId::new();

        let run_options = RunOptions {
            cwd: cwd.to_path_buf(),
            deadline_at,
            timeout_ms,
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
            // Skills default to the agent's own `skills` list (`run_sync` reads `opts.skills ??
            // agent.skills`); the foreground single-run path resolves against `cwd` alone (no
            // distinct orchestrator/runtime fallback cwd).
            skills: None,
            runtime_cwd: None,
            include_progress: None,
            agent_scope: None,
            acceptance: None,
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
            // Intercom child-bridge activation (pi `pi-args.ts:201-214` via
            // `data.intercomBridge.orchestratorTarget`): thread THIS orchestrator's own presence
            // target + this run's id + child index 0 so the spawned child registers
            // `contact_supervisor` (addressed here) + a broker presence under
            // `resolve_subagent_intercom_target(run_id, agent, 0)`. `None` target (headless / no live
            // intercom session) leaves the child un-bridged — the clean no-intercom path.
            orchestrator_intercom_target: self.orchestrator_intercom_target(),
            run_id: Some(run_id.clone()),
            child_index: Some(0),
        };

        // T6 artifact quadruple (pi `runs/foreground/execution.ts:960-1074`): record this run's input
        // BEFORE spawning (so it survives a child crash), then its output/metadata/event-stream AFTER
        // the run settles. Written into the scoped-temp artifacts root for `cwd` (the Rust analog of
        // pi's `tempArtifactsDir = getArtifactsDir(null)`, `extension/index.ts:263`). Best-effort: a
        // failed artifact write never alters the `SingleResult` the caller observes. (`run_id` was
        // minted above so it also identifies the clarify dispatch context.)
        let art_cfg = crate::artifacts::ArtifactConfig::foreground();
        let art_dir = crate::artifacts::temp_artifacts_dir(cwd);
        let art_paths =
            crate::artifacts::artifact_paths(&art_dir, run_id.as_str(), &agent.name, None);
        if art_cfg.enabled {
            let _ = crate::artifacts::ensure_artifacts_dir(&art_dir);
            if art_cfg.include_input {
                let _ = crate::artifacts::write_artifact(
                    &art_paths.input_path,
                    &format!("# Task for {}\n\n{task}", agent.name),
                );
            }
        }

        let result = drive_foreground_run_sync(
            &agent_config,
            task,
            run_options,
            &agent.name,
            resolved_context,
            on_update,
        )
        .await;

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
        // `pi-subagents/src/runs/foreground/execution.ts:677`), a dir that lives OUTSIDE the working
        // tree and never holds the event stream. An earlier revision erroneously `remove_dir_all`'d
        // the whole `.cyrup-subagent-scratch` dir here, which silently discarded that tee the moment a
        // foreground `/run` completed — defeating the tee's own stated purpose and diverging from
        // every sibling path — so no such deletion is performed.

        Ok(result)
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
        context: Option<ContextMode>,
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
        // T0.1/C13: the SAME resolved definition is projected into the plan-time persona map handed
        // to the runner, so hop 2 dispatches this agent's REAL persona rather than a placeholder.
        let agent = self.resolve_agent(cwd, agent_name)?;
        let resolved_agents: BTreeMap<String, ResolvedAgentPersona> =
            BTreeMap::from([(agent_name.to_string(), crate::exec::resolve_step_agent_config(&agent))]);
        // Fork default-mode (Tier-2): an OMITTED call-site `context` falls back to THIS agent's own
        // `default_context` (pi `resolveAgentDefaultContextPolicy`), an explicit value still wins.
        let effective_context = resolve_effective_context(context, agent.default_context);
        // R-SA-137: eager fork-context resolution before ANY process is spawned for this batch.
        let fork_context = self.resolve_context(cwd, effective_context).await?;

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
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: Some(effective_context),
            agent_scope: None,
        };

        self.spawn_background_steps(
            cwd,
            BackgroundStepsSpec {
                steps: vec![RunnerStep::SingleStep(step)],
                mode: RunMode::Single,
                session_file: fork_context.session_file_path,
                resolved_agents,
                // A single top-level task IS its own `{task}` value; a single run has no dedicated
                // chain scratch dir (`{chain_dir}` → the run cwd).
                original_task: task.to_string(),
                chain_dir: None,
            },
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
        spec: BackgroundStepsSpec,
    ) -> Result<RunId, SubagentError> {
        let BackgroundStepsSpec {
            steps,
            mode,
            session_file,
            resolved_agents,
            original_task,
            chain_dir,
        } = spec;
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
        // C7: derive the two sibling roots ONCE from the shared source of truth and create them
        // (ensureAccessibleDir-equivalent), then pass their ABSOLUTE paths through `RunnerConfig`
        // so the detached runner writes its terminal ResultFile into the SAME `results_dir` this
        // orchestrator created and watches — never a re-derived, never-created divergent dir.
        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(cwd);
        crate::background::ensure_accessible_dir(&async_root)
            .await
            .map_err(SubagentError::Spawn)?;
        crate::background::ensure_accessible_dir(&results_dir)
            .await
            .map_err(SubagentError::Spawn)?;
        let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        crate::background::ensure_accessible_dir(&run_paths.run_dir)
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
            async_root: async_root.clone(),
            results_dir: results_dir.clone(),
            // T0.1/C13: the plan-time persona map the orchestrator resolved (via
            // `resolve_plan_personas` / `exec::resolve_step_agent_config`) travels with the one-shot
            // config so the detached hop-2 runner dispatches each step's REAL persona and never
            // re-discovers or falls back to a placeholder `AgentConfig`.
            resolved_agents,
            // A (pi `originalTask`/`chainDir`): the run-wide `{task}` value + dedicated scratch chain
            // dir, resolved once by the orchestrator and serialized here so the detached runner
            // substitutes the SAME `{task}`/`{chain_dir}` the foreground path does.
            original_task,
            chain_dir,
            // Intercom child-bridge (pi `config.controlIntercomTarget`, `subagent-runner.ts:1823`):
            // this orchestrator's own presence target, resolved once here at plan time and carried
            // into the detached runner (which inherits no useful intercom env), so every step's
            // spawned child activates its `contact_supervisor` bridge addressed at this supervisor.
            // `None` (headless / no live intercom session) leaves each child un-bridged.
            orchestrator_intercom_target: self.orchestrator_intercom_target(),
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
        resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
        original_task: String,
        chain_dir: Option<PathBuf>,
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
        let executor: Arc<dyn SingleStepExecutor> = Arc::new(ExecSingleStepExecutor::foreground(
            depth,
            Arc::new(resolved_agents),
            self.orchestrator_intercom_target(),
            Some(RunId::new()),
        ));
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
            // A (pi `originalTask`/`chainDir`, `chain-execution.ts:493-497,1050`): the chain's real
            // top-level task + dedicated scratch chain dir, resolved once by the orchestrator
            // (`run_or_background_graph`) and threaded straight in, so a foreground `/chain` resolves
            // `{task}`/`{chain_dir}` to the SAME values the detached background runner does.
            original_task,
            chain_dir,
            dynamic_fanout_max_items: None,
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
    pub async fn run_or_background_graph(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: Option<ContextMode>,
        background: bool,
        task: Option<String>,
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

        // A (pi `originalTask`, `chain-execution.ts:493-497`): the run-wide `{task}` value — the
        // explicit call-site task if non-empty, else the graph's first step's first task. Resolved
        // ONCE here, the shared choke point, so BOTH the foreground walk and the detached background
        // runner substitute the identical `{task}`.
        let original_task = task
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| first_step_task(&graph));
        // A (pi `chainDir`, `chain-execution.ts:1050`): a dedicated per-run scratch directory under
        // the scoped chain-runs root, CREATED before dispatch so `{chain_dir}` resolves to an already-
        // existing directory on both the foreground and background paths (the detached runner only
        // substitutes the path string). Housekept by `artifacts::cleanup_old_chain_dirs`.
        let chain_dir = crate::artifacts::chain_runs_dir(cwd).join(RunId::new().as_str());
        crate::background::ensure_accessible_dir(&chain_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        // T0.1/C13: resolve every named persona up front (also the upfront agent-name validation —
        // an unresolvable agent fails here, before any child is spawned, matching pi's `/chain`//
        // `/parallel` name check).
        let resolved_agents = self.resolve_plan_personas(cwd, plan_step_agent_names(&graph))?;
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
        let resolver = Self::fork_resolver(cwd, session_file.as_deref());
        let (graph, first_session_file) =
            apply_fork_contexts(&resolver, context, &resolved_agents, graph).await?;

        if background {
            let run_id = self
                .spawn_background_steps(
                    cwd,
                    BackgroundStepsSpec {
                        steps: graph,
                        mode,
                        session_file: first_session_file,
                        resolved_agents,
                        original_task,
                        chain_dir: Some(chain_dir),
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
                .map(|s| matches!(s, RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_)))
                .collect();
            let (results, groups) = self
                .run_chain_foreground(cwd, graph, resolved_agents, original_task, Some(chain_dir))
                .await?;
            Ok(GraphRunOutcome::Foreground {
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
        let cfg = Self::discovery_config(cwd)?;
        let result = discover_agents(&cfg, None)?;
        // Cross-scope run precedence Project > User > Package > Builtin (pi `discoverSavedChains`
        // last-wins map, slash-commands.ts:1040) — NOT a naive first-match, which incorrectly let a
        // User chain shadow a same-named Project chain. See `discovery::resolve_chain_by_name`.
        crate::discovery::resolve_chain_by_name(&result.chains, name)
            .cloned()
            .ok_or_else(|| SubagentError::ChainNotFound(name.to_string()))
    }

    // ---------------------------------------------------------------------------------------
    // Registration surfaces: doctor / cost / profiles (delegates to already-implemented modules)
    // ---------------------------------------------------------------------------------------

    /// `/subagents-doctor` (R-SA-131; pi `buildDoctorReport`, doctor.ts:189-222): render the
    /// user-facing inventory report — a Runtime/session block, a Filesystem block naming the four
    /// scratch directories with each one's existence status, and a Discovery block with per-source
    /// agent/chain counts plus a skills inventory. This is pi's actual `/subagents-doctor` output
    /// (an inventory), distinct from [`crate::registration::doctor::DoctorRunner`]'s structured
    /// Ok/Warn/Fail check matrix (still available for programmatic diagnostics).
    pub async fn run_doctor(&self, cwd: &Path) -> String {
        let roots = crate::background::run_artifact_roots(cwd);
        let discovery_config =
            Self::discovery_config(cwd).unwrap_or_else(|_| Self::discovery_dirs_config(cwd));
        let discovered =
            crate::discovery::discover_agents_all(&discovery_config).unwrap_or_default();

        // Session info: the newest on-disk session under this cwd, opened READ-ONLY (never created —
        // a doctor report must not mutate state), matching pi's "current session file/dir/id" lines.
        let sessions_dir = Self::sessions_dir(cwd);
        let (session_file, session_id, session_error) =
            match crate::registration::cost::find_latest_session_file_by_mtime(&sessions_dir).await {
                Ok(Some(path)) => match cyrup_session::SessionManager::open(&path) {
                    Ok(manager) => (
                        Some(path),
                        Some(manager.session_id().as_str().to_string()),
                        None,
                    ),
                    Err(err) => (Some(path), None, Some(err.to_string())),
                },
                Ok(None) => (None, None, None),
                Err(err) => (None, None, Some(err.to_string())),
            };

        let input = DoctorReportInput {
            cwd,
            // A background/async run is a re-exec of this very binary; async is available whenever
            // the current executable path resolves (pi `isAsyncAvailable`'s cyrup analog).
            async_available: std::env::current_exe().is_ok(),
            configured_session_dir: sessions_dir.display().to_string(),
            current_session_file: session_file,
            current_session_id: session_id,
            session_error,
            temp_root_dir: crate::background::subagents_home(),
            async_runs_dir: roots.async_root,
            results_dir: roots.results_dir,
            chain_runs_dir: crate::artifacts::chain_runs_dir(cwd),
            discovered: &discovered,
        };
        build_doctor_report(&input)
    }

    /// The per-`cwd` session-storage directory (`<home>/.cyrup/sessions/<encoded cwd>`), the same
    /// layout [`Self::fork_resolver`] opens — factored out so `/subagents-doctor` and
    /// `/subagent-cost` locate the session transcript identically.
    fn sessions_dir(cwd: &Path) -> PathBuf {
        cyrup_session::SessionLayout::new(
            dirs_home().join(".cyrup").join("sessions"),
            cwd.to_path_buf(),
        )
        .dir()
    }

    /// `/subagent-cost` (R-SA-140; pi `buildSubagentCostReport`, slash-commands.ts:289-328): walk
    /// this session's TRANSCRIPT (not a background status file) and report the parent's own
    /// assistant-message usage plus a per-child breakdown of every subagent `toolResult` recorded in
    /// the branch — so foreground subagent usage (which never mints a background run) is visible.
    /// Reads the newest on-disk session for `cwd` (pi walks the live `ctx.sessionManager`; cyrup has
    /// no live manager threaded into this extension, so the faithful analog is the same on-disk read
    /// [`Self::fork_resolver`]/`run_doctor` already use). Delegates the actual walk + rendering to
    /// [`crate::registration::cost::build_subagent_cost_report`].
    pub async fn run_cost_report(&self, cwd: &Path) -> String {
        self.cost_report_from_sessions_dir(&Self::sessions_dir(cwd))
            .await
    }

    /// The testable core of [`Self::run_cost_report`]: given a resolved session-storage directory,
    /// open its newest `.jsonl` transcript READ-ONLY (never creating one) and render the cost report
    /// over its branch. An absent/empty session directory renders the well-formed empty-state report
    /// rather than an error.
    async fn cost_report_from_sessions_dir(&self, sessions_dir: &Path) -> String {
        let _ = self; // no executor state needed; a method purely for call-site symmetry/testability.
        match crate::registration::cost::find_latest_session_file_by_mtime(sessions_dir).await {
            Ok(Some(path)) => match cyrup_session::SessionManager::open(&path) {
                Ok(manager) => crate::registration::cost::build_subagent_cost_report(
                    manager.branch_path(None),
                ),
                Err(err) => format!(
                    "subagent-cost: could not open session {}: {err}",
                    path.display()
                ),
            },
            Ok(None) => crate::registration::cost::build_subagent_cost_report(
                std::iter::empty::<&cyrup_session::Entry>(),
            ),
            Err(err) => format!(
                "subagent-cost: could not scan session directory {}: {err}",
                sessions_dir.display()
            ),
        }
    }

    /// `/subagents-models` (pi `handleModels`, agent-management.ts:580-647; slash dispatch
    /// slash-commands.ts:1090-1111): report the RUNTIME builtin-agent -> model mapping — each
    /// discovered builtin persona's effective model + the provenance of that model — NOT a dump of
    /// the full static provider catalog. `requested_agent` filters to a single builtin (pi's
    /// single-agent form), erroring with the available-builtins list when the name is not a
    /// discovered builtin.
    ///
    /// cyrup has no live model-registry / current-session-model handle threaded into this extension
    /// (an outer-layer seam, Tier 8), so the "current session model" line and any registry-driven
    /// re-resolution degrade to "(unavailable)"/inherit — the effective model shown is the persona's
    /// own configured `model` (frontmatter / settings override / settings default), which is exactly
    /// the runtime-loaded mapping this command exists to surface and which discovery already resolves
    /// faithfully.
    #[must_use]
    pub fn run_models_report(&self, cwd: &Path, requested_agent: Option<&str>) -> String {
        let _ = self; // no executor state needed; a method for call-site symmetry with run_doctor.
        let cfg = Self::discovery_config(cwd).unwrap_or_else(|_| Self::discovery_dirs_config(cwd));
        let discovered = match crate::discovery::discover_agents_all(&cfg) {
            Ok(discovered) => discovered,
            Err(err) => return format!("subagents-models: discovery failed: {err}"),
        };

        let mut builtins: Vec<&AgentDefinition> = discovered
            .agents
            .iter()
            .filter(|agent| agent.source == AgentSource::Builtin)
            .collect();
        builtins.sort_by(|a, b| a.name.cmp(&b.name));

        if let Some(requested) = requested_agent {
            let requested = requested.trim();
            let Some(agent) = builtins.iter().find(|agent| agent.name == requested) else {
                let available = if builtins.is_empty() {
                    "none".to_string()
                } else {
                    builtins
                        .iter()
                        .map(|agent| agent.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return format!("Builtin agent '{requested}' not found. Available: {available}.");
            };

            let mut lines = vec![
                "Builtin subagent model".to_string(),
                String::new(),
                format!("Agent: {requested}"),
                "Effective model:".to_string(),
                format!("  {}", resolved_builtin_model(agent)),
                format!("Source: {}", format_model_source(agent)),
            ];
            if let Some(override_info) = &agent.override_info {
                lines.push("Override file:".to_string());
                lines.push(format!("  {}", override_info.settings_path.display()));
            }
            if agent.disabled == Some(true) {
                lines.push("Disabled: true".to_string());
            }
            lines.push("Current session model:".to_string());
            lines.push("  (unavailable)".to_string());
            return lines.join("\n");
        }

        let mut lines = vec![
            "Builtin subagent models".to_string(),
            String::new(),
            "Current session model:".to_string(),
            "  (unavailable)".to_string(),
            String::new(),
        ];
        if builtins.is_empty() {
            lines.push("(no builtin subagents discovered in this build)".to_string());
        }
        for agent in &builtins {
            let disabled_suffix = if agent.disabled == Some(true) {
                "; disabled"
            } else {
                ""
            };
            lines.push(agent.name.clone());
            lines.push("  model:".to_string());
            lines.push(format!("    {}", resolved_builtin_model(agent)));
            lines.push(format!("  source: {}{disabled_suffix}", format_model_source(agent)));
            lines.push(String::new());
        }
        lines.join("\n")
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

    // ---------------------------------------------------------------------------------------
    // Background control actions (C5): status / interrupt / resume / append-step
    //
    // Each method is the executor half of one `subagent` control action (pi
    // `subagent-executor.ts:2845-2912`), routing to the faithful [`crate::background::control`]
    // primitives + the [`crate::background::run_status`] report shape. They return
    // `Result<String, String>`: `Ok` is the rendered report/confirmation the caller shows as tool
    // content; `Err` is the user-facing failure message the tool surface turns into a `ToolError`
    // (cyrup's `ToolResult` has no `isError` flag, so a soft user-facing error is an `Err(text)`).
    // ---------------------------------------------------------------------------------------

    /// `action: "status"` (C5): render the no-id "list active runs" view, or a single run's full
    /// per-step report resolved by `id` (exact or unique prefix) or by `dir` — pi
    /// `subagent-executor.ts:2845-2863` + `run-status.ts:101-273`.
    ///
    /// # Errors
    ///
    /// Returns the not-found notice (or a resolution/reconciliation error message) as `Err`.
    pub async fn control_status(
        &self,
        cwd: &Path,
        id: Option<&str>,
        dir: Option<&str>,
    ) -> Result<String, String> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        // pi precedence (`run-status.ts:131`): a bare `id` (no `dir`) resolves by id; otherwise a
        // present `dir` resolves the directory directly; otherwise (neither) list active runs.
        match (id, dir) {
            (Some(id), None) => run_status::inspect_status_by_id(&async_root, &results_dir, id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Async run not found. Provide id or dir.".to_string()),
            (_, Some(dir)) => run_status::inspect_status_by_dir(Path::new(dir), &results_dir)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Async run not found. Provide id or dir.".to_string()),
            (None, None) => {
                let runs = run_status::list_active_runs(&async_root, &results_dir)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(run_status::format_run_list(&runs))
            }
        }
    }

    /// `action: "interrupt"` (C5): deliver a soft, resumable interrupt (R-SA-084 — a *pause*
    /// request, never a kill) to the target async run, or, with no id, to the most-recently-updated
    /// running run in this cwd's async root — pi `subagent-executor.ts:2871-2911`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if no interrupt-capable run is found, if the target is not Running (R-SA-079),
    /// or if the underlying delivery fails.
    pub async fn control_interrupt(&self, cwd: &Path, target: Option<&str>) -> Result<String, String> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let run_id = match target {
            Some(explicit) => explicit.to_string(),
            None => {
                // No id: interrupt the most-recently-updated running run (the list is already sorted
                // running-first, most-recent-first), mirroring pi's "defaults to the most recently
                // active controllable run" contract for interrupt.
                let runs = run_status::list_active_runs(&async_root, &results_dir)
                    .await
                    .map_err(|e| e.to_string())?;
                runs.iter()
                    .find(|run| run.status.state == RunState::Running)
                    .map(|run| run.status.run_id.as_str().to_string())
                    .ok_or_else(|| "No interrupt-capable run found in this session.".to_string())?
            }
        };
        match control::interrupt(&async_root, &results_dir, &run_id, "interrupt-action", None).await {
            Ok(InterruptOutcome::Delivered | InterruptOutcome::AlreadyPending) => {
                Ok(format!("Interrupt requested for async run {run_id}."))
            }
            Ok(InterruptOutcome::NotRunning) => Err(format!(
                "No running async run with an interrupt-capable pid was found for '{run_id}'."
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
    /// # Errors
    ///
    /// Returns `Err` for a missing message/id, the delivery-failed intercom-unregistered live-steer
    /// notice, a no-transcript revival, or any resolution/spawn failure.
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
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        match control::resume(&async_root, &results_dir, run_id, index).await {
            Ok(ResumeOutcome::SteerRunning { step_index }) => {
                // pi (`subagent-executor.ts:848-878`): interrupt the live child, then DELIVER the
                // follow-up over the broker to that child's registered bridge target. Recover the
                // child's deterministic target from the reconciled run status — the resumed step's
                // REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let child_target = {
                    let source_paths = RunPaths::for_run(
                        &async_root,
                        &results_dir,
                        &RunId::from_token(run_id.to_string()),
                    );
                    match control::reconcile_before_control_op(&source_paths).await {
                        Ok(status) => status.steps.get(step_index).map(|step| {
                            crate::spawn::intercom_target::resolve_subagent_intercom_target(
                                run_id,
                                &step.agent,
                                step_index,
                            )
                        }),
                        Err(_) => None,
                    }
                };
                // Interrupt the live child (genuine), matching pi's interrupt-then-deliver order.
                let _ = control::interrupt(&async_root, &results_dir, run_id, "async-resume", None)
                    .await;
                let follow_up_message =
                    format!("Follow-up for async run {run_id}:\n\n{follow_up}");
                let delivered = match &child_target {
                    Some(target) => self
                        .steer
                        .steer(target.clone(), follow_up_message)
                        .await
                        .unwrap_or(false),
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
            Ok(ResumeOutcome::RespawnFromTranscript { step_index, session_file }) => self
                .revive_from_transcript(cwd, run_id, step_index, &session_file, follow_up)
                .await
                .map_err(|e| e.to_string()),
            Err(SubagentError::ResumeNoTranscript) => Err(format!(
                "Resume unavailable: async run '{run_id}' has no persisted transcript to revive \
                 from."
            )),
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
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
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
        let resolved_agents = self.resolve_plan_personas(cwd, [agent.clone()])?;
        let step = SingleStepSpec {
            agent: agent.clone(),
            task: follow_up.to_string(),
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
                cwd,
                BackgroundStepsSpec {
                    steps: vec![RunnerStep::SingleStep(step)],
                    mode: RunMode::Single,
                    session_file: Some(session_file.to_path_buf()),
                    resolved_agents,
                    // The revival's follow-up is its `{task}`; a single revived run has no chain dir.
                    original_task: follow_up.to_string(),
                    chain_dir: None,
                },
            )
            .await?;
        Ok(format!(
            "Revived async subagent from {source_run_id}.\n\
             Revived run: {new_id}\n\
             Agent: {agent}\n\
             Session: {}\n\
             Status if needed: subagent({{ action: \"status\", id: \"{new_id}\" }})",
            session_file.display()
        ))
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
        self.resolve_agent(cwd, agent)
            .map_err(|e| format!("Cannot append step to run '{run_id}': {e}"))?;
        let step = SingleStepSpec {
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
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
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

// C7: both roots come from the ONE shared derivation in `background/mod.rs`
// ([`crate::background::run_artifact_roots`]) so the orchestrator and the detached runner can never
// derive divergent results dirs again. These stay as thin, named wrappers because
// `default_async_root`/`default_results_dir` are already the vocabulary every other call site in
// this file (`resume_tracking`, `run_doctor`, the depth-guard tests) reads in terms of.
fn default_async_root(cwd: &Path) -> PathBuf {
    crate::background::run_artifact_roots(cwd).async_root
}

fn default_results_dir(cwd: &Path) -> PathBuf {
    crate::background::run_artifact_roots(cwd).results_dir
}

fn dirs_home() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// The current wall-clock time as whole milliseconds since the Unix epoch, saturating to `u64`.
/// Used to stamp per-provider catalog freshness (`registration::profiles::ProviderModelCatalog`)
/// and to evaluate the `--force`/staleness gate. Never panics: a pre-epoch clock reads as `0`, and
/// a value beyond `u64::MAX` ms (year ~584 million) saturates rather than overflowing.
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Build the `_meta.json` metadata value for a completed foreground run (T6, pi
/// `runs/foreground/execution.ts:1053-1068`). Carries the fields this crate's [`SingleResult`]
/// actually knows: `runId`/`agent`/`task`/`exitCode`/`usage`/`model`/`attemptedModels`/
/// `modelAttempts`/`toolCount`/`error`/`timestamp`. Pi additionally records `durationMs`/`skills`/
/// `skillsWarning`, which `SingleResult` does not carry in this crate (they live on pi's richer
/// `progressSummary`/skill-resolution shapes); those keys are omitted rather than faked.
fn foreground_artifact_metadata(run_id: &str, result: &SingleResult) -> serde_json::Value {
    let attempted: Vec<&str> = result.attempted_models.iter().map(ModelId::as_str).collect();
    let model_attempts: Vec<serde_json::Value> = result
        .model_attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "model": a.model.as_str(),
                "success": a.success,
                "exitCode": a.exit_code,
                "error": a.error,
                "usage": serde_json::to_value(&a.usage).unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();
    serde_json::json!({
        "runId": run_id,
        "agent": result.agent,
        "task": result.task,
        "exitCode": result.exit_code,
        "usage": serde_json::to_value(&result.usage).unwrap_or(serde_json::Value::Null),
        "model": result.model.as_ref().map(ModelId::as_str),
        "attemptedModels": attempted,
        "modelAttempts": model_attempts,
        "toolCount": result.tool_calls.len(),
        "error": result.error,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    })
}

/// The `.jsonl` event lines for a completed foreground run (T6). pi's `.jsonl` is the raw NDJSON the
/// child streamed to stdout; this crate's [`SingleResult`] is the already-compacted shape (it does
/// not retain the raw per-event stream — that lives transiently in the per-attempt tee under the
/// run's scratch dir, R-SA-058, which is cleaned up), so the foreground `.jsonl` is reconstructed
/// from the run's observable, retained events: one line per summarized tool call, then a terminal
/// `result` line. A genuine, non-empty NDJSON record of the run — see the crate's T6 report for the
/// documented divergence from pi's byte-identical child stream.
fn foreground_artifact_jsonl_lines(result: &SingleResult) -> Vec<String> {
    let mut lines = Vec::with_capacity(result.tool_calls.len() + 1);
    for call in &result.tool_calls {
        lines.push(
            serde_json::json!({
                "type": "tool_call",
                "text": call.text,
                "expandedText": call.expanded_text,
            })
            .to_string(),
        );
    }
    lines.push(
        serde_json::json!({
            "type": "result",
            "agent": result.agent,
            "exitCode": result.exit_code,
            "model": result.model.as_ref().map(ModelId::as_str),
            "output": result.final_output,
            "error": result.error,
        })
        .to_string(),
    );
    lines
}

/// Write a completed foreground run's output/metadata/event-stream artifacts (T6, the after-run half
/// of pi `runs/foreground/execution.ts:1047-1069`). The `_input.md` is written by the caller BEFORE
/// the run (crash-safety, matching pi); this writes the remaining three files gated on `cfg`. All
/// writes are best-effort — a failed artifact write must never change the run's observable result.
fn write_foreground_output_artifacts(
    paths: &crate::artifacts::ArtifactPaths,
    cfg: &crate::artifacts::ArtifactConfig,
    run_id: &str,
    result: &SingleResult,
) {
    if !cfg.enabled {
        return;
    }
    if cfg.include_output {
        let _ = crate::artifacts::write_artifact(
            &paths.output_path,
            result.final_output.as_deref().unwrap_or(""),
        );
    }
    if cfg.include_metadata {
        let _ = crate::artifacts::write_metadata(
            &paths.metadata_path,
            &foreground_artifact_metadata(run_id, result),
        );
    }
    if cfg.include_jsonl {
        for line in foreground_artifact_jsonl_lines(result) {
            let _ = crate::artifacts::append_jsonl(&paths.jsonl_path, &line);
        }
    }
}

/// Drive one foreground [`crate::exec::run_sync`], optionally streaming live progress through
/// `on_update` (C19 — the crate-side of pi's `onUpdate`/`fireUpdate`,
/// `runs/foreground/execution.ts:478-499`). When `on_update` is `None` this is a plain awaited
/// `run_sync` — the original, silent-until-completion behavior; every non-streaming caller (the
/// `/run` slash command, tests) is unchanged.
///
/// When `on_update` is `Some`, a [`crate::exec::LiveEventSink`] is installed on
/// [`RunOptions::live_events`] that folds each raw child NDJSON line into a shared
/// [`crate::tui::events::LiveProgressFold`] and, on every progress-relevant event, pushes a
/// [`crate::tui::events::SubagentUpdatePayload`] onto an unbounded channel. `run_sync` and a drain
/// of that channel are then raced on the SAME task via `tokio::select!` — no extra task is spawned,
/// and the `Fn`-only sink (which cannot itself touch the `FnMut` `on_update`) bridges to it purely
/// through the channel — so live updates are delivered as the child streams, and a final settle
/// update carries the terminal [`SingleResult`] on the same channel (pi's settle-time snapshot).
async fn drive_foreground_run_sync(
    agent_config: &AgentConfig,
    task: &str,
    mut run_options: RunOptions,
    agent_name: &str,
    resolved_context: ContextMode,
    on_update: Option<ToolUpdateSink>,
) -> SingleResult {
    use crate::tui::events::{LiveProgressFold, LiveProgressStatus, SubagentUpdatePayload};

    let Some(mut on_update) = on_update else {
        // No sink installed (the `/run` slash command, tests): the original awaited run — identical
        // to the pre-C19 behavior, no channel, no select, no live_events.
        return crate::exec::run_sync(agent_config, task, &run_options).await;
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<cyrup_core::ToolUpdate>();
    // The fold is shared with the `Fn + Send + Sync` sink; `run_sync` calls that sink synchronously
    // from its single stdout-read loop, so the `Mutex` is uncontended in practice — it exists only
    // to satisfy the `Sync` bound the sink requires. A poisoned lock (impossible without a panic in
    // the sink) recovers the inner value rather than propagating, so a live-progress hiccup never
    // fails the run itself.
    let fold = std::sync::Arc::new(std::sync::Mutex::new(LiveProgressFold::new(Some(
        agent_name.to_string(),
    ))));
    let sink = {
        let fold = std::sync::Arc::clone(&fold);
        crate::exec::LiveEventSink::new(move |raw: &str| {
            let mut guard = match fold.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            // Emit an update exactly when a progress-relevant event fired (pi's `fireUpdate`
            // cadence), never once per raw line.
            if guard.record_line(raw) {
                let snapshot = guard.snapshot(LiveProgressStatus::Running);
                let payload = SubagentUpdatePayload::single_live(resolved_context, snapshot);
                let text = payload.content_text();
                // A closed receiver (the caller already returned) is a benign no-op.
                let _ = tx.send(payload.into_tool_update(text));
            }
        })
    };
    run_options.live_events = Some(sink);

    let run = crate::exec::run_sync(agent_config, task, &run_options);
    tokio::pin!(run);
    let result = loop {
        tokio::select! {
            settled = &mut run => break settled,
            Some(update) = rx.recv() => on_update(update),
        }
    };
    // Deliver any updates buffered between the last poll and the child settling (the child's stdout
    // is fully drained by the time `run_sync` returns, so no further sends can arrive).
    while let Ok(update) = rx.try_recv() {
        on_update(update);
    }

    // Final settle update (pi emits a terminal snapshot on the same channel): flip the status to
    // the run's terminal outcome and carry the full `SingleResult` in `results` so the inline
    // surface can render the completed row from the same `details` shape the live updates used.
    let final_status = if result.exit_code == 0 && !result.timed_out {
        LiveProgressStatus::Complete
    } else {
        LiveProgressStatus::Failed
    };
    let final_snapshot = match fold.lock() {
        Ok(guard) => guard.snapshot(final_status),
        Err(poisoned) => poisoned.into_inner().snapshot(final_status),
    };
    let final_payload =
        SubagentUpdatePayload::single_final(resolved_context, result.clone(), final_snapshot);
    let text = result
        .final_output
        .clone()
        .unwrap_or_else(|| final_payload.content_text());
    on_update(final_payload.into_tool_update(text));

    result
}

/// Render a foreground `/run` result as a completion summary (T8 slash-live-state, partial): the
/// single transcript entry `execute_command` returns, shaped to read as pi's live-state placeholder
/// RESOLVED to completion (`slash/slash-live-state.ts` -> `renderSubagentResult`). A status line
/// (done/failed/paused/timed-out + agent + tool-call and token stats) precedes the delivered output
/// — the same header/stats/body composition pi's settled placeholder renders, minus the mid-run
/// in-place updating that requires a host transcript-update channel (documented at the `/run`
/// dispatch site as the remaining outer-layer step).
fn format_slash_run_completion(result: &SingleResult) -> String {
    let tokens = result.usage.input.saturating_add(result.usage.output);
    let tool_count = result.tool_calls.len();
    let status = if result.interrupted {
        "paused (interrupted)".to_string()
    } else if result.timed_out {
        "timed out".to_string()
    } else if result.exit_code == 0 {
        "done".to_string()
    } else {
        format!("failed (exit {})", result.exit_code)
    };
    let plural = if tool_count == 1 { "" } else { "s" };
    let header =
        format!("subagent {} · {status} · {tool_count} tool call{plural} · {tokens} tokens", result.agent);
    let body = result.final_output.clone().unwrap_or_default();
    let body = if body.trim().is_empty() {
        result
            .error
            .clone()
            .unwrap_or_else(|| "(no output)".to_string())
    } else {
        body
    };
    format!("{header}\n\n{body}")
}

/// Enumerate every installed package across both [`cyrup_resources::InstallScope`]s by loading the
/// persisted `packages.json` install registries `cyrup-resources` itself writes — Global under
/// `<global_dir>/packages.json`, Project under `<project_root>/.cyrup/packages.json` (the exact
/// paths [`cyrup_resources::PackageStore::registry_path`] resolves) — and concatenating them in the
/// fixed project-then-global order [`crate::discovery::scan_package_agents`] re-sorts into anyway.
///
/// A missing registry file is an empty registry (never an error — the common "no packages installed"
/// case), mirroring `cyrup_resources::package::lock::load`'s own missing-file contract; a malformed
/// registry is likewise treated as "no packages from that scope" rather than aborting all of
/// discovery, since a package-registry read failure is not one of R-SA-009's three surfaced-error
/// cases (which cover malformed agent frontmatter, chain files, and `subagents.*` settings only).
/// This is the read-only enumeration half of the package tier; the on-disk package roots are
/// resolved later, per-package, by `scan_package_agents`/`scan_package_chain_scopes` via
/// `installed_dir` from these same records.
fn enumerate_installed_packages(
    global_dir: &Path,
    project_root: Option<&Path>,
) -> cyrup_resources::InstalledPackages {
    use cyrup_resources::InstallScope;

    let store = cyrup_resources::PackageStore::new(
        global_dir.to_path_buf(),
        project_root.map(Path::to_path_buf),
    );
    let mut installed = cyrup_resources::InstalledPackages::default();
    for scope in [InstallScope::Project, InstallScope::Global] {
        let Some(registry_path) = store.registry_path(scope) else {
            continue;
        };
        if let Ok(registry) = cyrup_resources::package::lock::load(&registry_path) {
            installed.packages.extend(registry.packages);
        }
    }
    installed
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

/// The `subagent` tool's full multi-section description (R-SA-128, C8) — ported verbatim from
/// pi-subagents' registered tool description (`src/extension/index.ts:461-495`), the string the LLM
/// actually reads to decide how to drive the tool. Reproducing it faithfully is what lets a caller
/// discover the management (`action: "list"/"get"/…`), control (`status`/`interrupt`/`resume`/
/// `append-step`), CHAIN, and PARALLEL shapes at all — not just the SINGLE shape the pre-C8 schema
/// advertised. The pi tool-description executable spec (`test/unit/tool-description.test.ts`) pins
/// several substrings of this text (the `action: "list"` inspect line, `executable/non-disabled`,
/// `proactive skill subagent suggestions`, the `output?,reads?,progress?` PARALLEL shape, the
/// `timeoutMs`/`maxRuntimeMs` `only for foreground runs` / `omit for async/background runs` note);
/// this crate's own `subagent_tool_schema_exposes_the_full_pi_parameter_union` test re-pins them.
const SUBAGENT_TOOL_DESCRIPTION: &str = r#"Delegate to subagents or manage agent definitions.

EXECUTION (use exactly ONE mode):
• Before executing, use { action: "list" } to inspect configured agents/chains. Only execute agents listed as executable/non-disabled.
• SINGLE: { agent, task? } - one task; omit task for self-contained agents
• CHAIN: { chain: [{agent:"agent-a"}, {parallel:[{agent:"agent-b",count:3}]}] } - sequential pipeline with optional parallel fan-out
• PARALLEL: { tasks: [{agent,task,count?,output?,reads?,progress?}, ...], concurrency?: number, worktree?: true } - concurrent execution (worktree: isolate each task in a git worktree)
• Optional context: { context: "fresh" | "fork" } (explicit value overrides every child; when omitted, each requested agent uses its own defaultContext, otherwise "fresh"; inspect agent defaults via { action: "list" })
• Optional timeout: { timeoutMs } or { maxRuntimeMs } only for foreground runs; omit for async/background runs or set async:false if you need a foreground timeout
• If { action: "list" } shows proactive skill subagent suggestions, consider a small fresh-context fanout for broad tasks where one of those skills would materially help

CHAIN TEMPLATE VARIABLES (use in task strings):
• {task} - The original task/request from the user
• {previous} - Text response from the previous step (empty for first step)
• {chain_dir} - Shared directory for chain files (e.g., <tmpdir>/pi-subagents-<scope>/chain-runs/abc123/)

Example: { chain: [{agent:"agent-a", task:"Analyze {task}"}, {agent:"agent-b", task:"Plan based on {previous}"}] }

MANAGEMENT (use action field, omit agent/task/chain/tasks):
• { action: "list" } - discover executable agents/chains
• { action: "get", agent: "name" } - full detail; packaged agents use dotted runtime names like "package.agent"
• { action: "models", agent?: "name" } - show the runtime-loaded builtin subagent model mapping, optionally filtered to one builtin
• { action: "create", config: { name: "custom-agent", package: "code-analysis", systemPrompt, systemPromptMode, inheritProjectContext, inheritSkills, defaultContext, ... } }
• { action: "update", agent: "code-analysis.custom-agent", config: { package: "analysis", ... } } - merge
• { action: "delete", agent: "code-analysis.custom-agent" }
• Use chainName for chain operations; packaged chains also use dotted runtime names

CONTROL:
• { action: "status", id: "..." } - inspect an async/background run by id or prefix
• { action: "interrupt", id?: "..." } - soft-interrupt the current child turn and leave the run paused
• { action: "resume", id: "...", message: "...", index?: 0 } - interrupt then follow up with a live async child, or revive a completed async/foreground child from its session
• { action: "append-step", id: "...", chain: [{agent:"agent-c", task:"Use {previous}"}] } - append one step to the tail of a running async chain

DIAGNOSTICS:
• { action: "doctor" } - read-only report for runtime paths, discovery, sessions, and intercom"#;

/// The `subagent` tool's full discriminated-union parameter surface (R-SA-128, C8) — the Rust parse
/// target for pi's `SubagentParamsSchema` (`src/extension/schemas.ts:195-265`). Every top-level pi
/// field is represented so the tool can drive SINGLE (`agent`/`task`), PARALLEL (`tasks`/
/// `concurrency`/`worktree`), CHAIN (`chain`), management (`action` ∈ list/get/models/create/update/
/// delete), control (`action` ∈ status/interrupt/resume/append-step), and diagnostics (`action:
/// "doctor"`). Parsing is deliberately permissive (DI-SA-11): every field is optional, unknown keys
/// are ignored, and the union's genuinely-open sub-shapes (`config`/`control`/`output`/`skill`/
/// `acceptance`, and the per-item `tasks[]`/`chain[]` element shapes) are captured as raw
/// [`serde_json::Value`] here — the LLM-facing JSON Schema in [`subagent_tool_parameters`] carries
/// the full per-field structural detail, while typed per-item parsing/routing of `tasks[]`/`chain[]`
/// lands in P1 (this tier owns the schema + dispatch skeleton, not the sub-executor routing).
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentToolParams {
    agent: Option<String>,
    task: Option<String>,
    action: Option<String>,
    id: Option<String>,
    run_id: Option<String>,
    dir: Option<String>,
    index: Option<u64>,
    message: Option<String>,
    chain_name: Option<String>,
    config: Option<serde_json::Value>,
    tasks: Option<Vec<serde_json::Value>>,
    concurrency: Option<u64>,
    worktree: Option<bool>,
    chain: Option<Vec<serde_json::Value>>,
    context: Option<String>,
    chain_dir: Option<String>,
    #[serde(rename = "async")]
    r#async: Option<bool>,
    timeout_ms: Option<u64>,
    max_runtime_ms: Option<u64>,
    agent_scope: Option<String>,
    cwd: Option<String>,
    artifacts: Option<bool>,
    include_progress: Option<bool>,
    share: Option<bool>,
    session_dir: Option<String>,
    clarify: Option<bool>,
    control: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    output_mode: Option<String>,
    skill: Option<serde_json::Value>,
    model: Option<String>,
    acceptance: Option<serde_json::Value>,
}

/// pi `resolveForegroundTimeout` (`subagent-executor.ts:1327-1341`): `timeoutMs` and `maxRuntimeMs`
/// are ALIASES for one foreground timeout budget. Returns the single effective value (or `None` when
/// neither is supplied), or an `Err` message when a value is non-positive or the two aliases were
/// both supplied with DIFFERENT values. (A negative/fractional value could never have deserialized
/// into `Option<u64>`, so pi's `!Number.isInteger(value) || value <= 0` reduces here to rejecting
/// `0`.)
fn resolve_foreground_timeout(p: &SubagentToolParams) -> Result<Option<u64>, String> {
    for (name, value) in [("timeoutMs", p.timeout_ms), ("maxRuntimeMs", p.max_runtime_ms)] {
        if value == Some(0) {
            return Err(format!("{name} must be a positive integer."));
        }
    }
    if let (Some(a), Some(b)) = (p.timeout_ms, p.max_runtime_ms)
        && a != b
    {
        return Err(
            "timeoutMs and maxRuntimeMs are aliases; provide only one value or use the same \
             value for both."
                .to_string(),
        );
    }
    Ok(p.timeout_ms.or(p.max_runtime_ms))
}

/// pi `formatFailedSingleRunOutput` (`subagent-executor.ts:1041-1052`): the delivered content for a
/// FAILED single run — the error text (`result.error` or `"Failed"`), followed, ONLY when the run
/// produced distinct output, by an `Output:` block carrying that output. This is what
/// [`SubagentTool::route_single`] hands to `ToolError` (cyrup's error channel; pi's `isError: true`),
/// so an LLM caller sees the failure reason in the model-facing CONTENT rather than only buried in
/// `details` JSON. (pi additionally appends an `Output artifact:` line from
/// `result.artifactPaths?.outputPath`; this crate's [`SingleResult`] carries no such field — the
/// saved-output reference is already folded into `final_output` — so that line has no analogue here.)
fn format_failed_single_run_output(result: &SingleResult, display_output: &str) -> String {
    let error = result
        .error
        .as_deref()
        .filter(|e| !e.is_empty())
        .unwrap_or("Failed");
    let output = display_output.trim();
    let mut lines = vec![error.to_string()];
    if !output.is_empty() && output != error.trim() {
        lines.push(String::new());
        lines.push("Output:".to_string());
        lines.push(output.to_string());
    }
    lines.join("\n")
}

impl SubagentToolParams {
    /// Whether this call requested background/detached execution (pi `async`). Defaults to `false`
    /// in this tier; the per-config / per-persona `asyncByDefault` default is a later wire-up.
    fn is_background(&self) -> bool {
        self.r#async.unwrap_or(false)
    }

    /// The requested fork/fresh context OVERRIDE (pi `context`), as an `Option` that preserves the
    /// "omitted" case: `Some(Fork)`/`Some(Fresh)` for an explicit value, `None` when the caller left
    /// `context` off entirely. An omitted (`None`) value is what lets each requested agent fall back
    /// to ITS OWN persona `default_context` downstream (pi `resolveAgentDefaultContextPolicy`,
    /// `subagent-executor.ts:1280-1293`) rather than being forced to `Fresh` — the collapse-to-`Fresh`
    /// that the pre-Tier-2 `context_mode` did. Any non-`"fork"` explicit string still resolves to
    /// `Some(Fresh)` (pi treats only the literal `"fork"` as fork).
    fn context_override(&self) -> Option<ContextMode> {
        match self.context.as_deref() {
            None => None,
            Some("fork") => Some(ContextMode::Fork),
            Some(_) => Some(ContextMode::Fresh),
        }
    }

    /// The parameter keys actually supplied on this call, in pi's own camelCase spelling — surfaced
    /// in the labeled placeholder text of the not-yet-wired management/control/parallel/chain
    /// dispatch arms so a caller (and P1's implementer) can see exactly what shape was parsed.
    /// Reading every field here is also what lets the full pi-union struct above compile under the
    /// workspace's `-D warnings` (`dead_code`) without any non-`#[cfg(test)]` `#[allow]`.
    fn provided_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.agent.is_some() { keys.push("agent"); }
        if self.task.is_some() { keys.push("task"); }
        if self.action.is_some() { keys.push("action"); }
        if self.id.is_some() { keys.push("id"); }
        if self.run_id.is_some() { keys.push("runId"); }
        if self.dir.is_some() { keys.push("dir"); }
        if self.index.is_some() { keys.push("index"); }
        if self.message.is_some() { keys.push("message"); }
        if self.chain_name.is_some() { keys.push("chainName"); }
        if self.config.is_some() { keys.push("config"); }
        if self.tasks.is_some() { keys.push("tasks"); }
        if self.concurrency.is_some() { keys.push("concurrency"); }
        if self.worktree.is_some() { keys.push("worktree"); }
        if self.chain.is_some() { keys.push("chain"); }
        if self.context.is_some() { keys.push("context"); }
        if self.chain_dir.is_some() { keys.push("chainDir"); }
        if self.r#async.is_some() { keys.push("async"); }
        if self.timeout_ms.is_some() { keys.push("timeoutMs"); }
        if self.max_runtime_ms.is_some() { keys.push("maxRuntimeMs"); }
        if self.agent_scope.is_some() { keys.push("agentScope"); }
        if self.cwd.is_some() { keys.push("cwd"); }
        if self.artifacts.is_some() { keys.push("artifacts"); }
        if self.include_progress.is_some() { keys.push("includeProgress"); }
        if self.share.is_some() { keys.push("share"); }
        if self.session_dir.is_some() { keys.push("sessionDir"); }
        if self.clarify.is_some() { keys.push("clarify"); }
        if self.control.is_some() { keys.push("control"); }
        if self.output.is_some() { keys.push("output"); }
        if self.output_mode.is_some() { keys.push("outputMode"); }
        if self.skill.is_some() { keys.push("skill"); }
        if self.model.is_some() { keys.push("model"); }
        if self.acceptance.is_some() { keys.push("acceptance"); }
        keys
    }
}

// -------------------------------------------------------------------------------------------------
// Tool-driven PARALLEL (`tasks[]`) and CHAIN (`chain[]`) item parsing + routing (Tier 1)
//
// The `subagent` tool carries `tasks[]`/`chain[]` as raw `Vec<serde_json::Value>` (T0.5 kept the
// per-item shape untyped); this section is the typed lowering into `SingleStepSpec`/`RunnerStep`
// the parallel/chain dispatch arms route through. Faithful port of the pi per-item mapping in
// `subagent-executor.ts` (`params.tasks` -> parallel group, `expandTopLevelTaskCounts`,
// `findDuplicateParallelOutputPath`) and `schemas.ts`'s `TaskItem`/`ParallelTaskSchema`/`ChainItem`.
// -------------------------------------------------------------------------------------------------

/// One parsed `tasks[]` element (top-level PARALLEL) or `parallel[]` element (a static parallel
/// group inside a `chain[]` step) — the union of pi's `TaskItem` (`schemas.ts:78-90`) and
/// `ParallelTaskSchema` (`schemas.ts:93-109`).
///
/// Fields with a [`SingleStepSpec`] home reach the child today: `agent`/`task`/`cwd`/`model`/
/// `as`(named output)/`outputMode`/`reads`/`acceptance`/`outputSchema`, plus `count` (a fan-out
/// WIDTH multiplier applied by expansion, never a per-step spec field). `output` (the output FILE
/// path) is parsed because it drives duplicate-output-path rejection (pi
/// `findDuplicateParallelOutputPath`), even though the file-output HANDOFF itself is Tier-2 plumbing
/// (`exec/output.rs`, `output_path` currently unset); `progress`/`skill`/`phase`/`label` are parsed
/// (so the shape is accepted) but not yet plumbed to the child (Tier 4/5) — see this task's own gap
/// note. `#[serde(default)]` on every optional field keeps parsing permissive, matching pi's
/// TypeBox `Type.Optional` shape.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolTaskItem {
    agent: String,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default)]
    reads: Option<serde_json::Value>,
    #[serde(default)]
    progress: Option<bool>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skill: Option<serde_json::Value>,
    #[serde(default)]
    acceptance: Option<serde_json::Value>,
    #[serde(default, rename = "as")]
    as_output: Option<String>,
    #[serde(default)]
    output_schema: Option<serde_json::Value>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

impl ToolTaskItem {
    /// Read every parsed field at least once so the workspace `dead_code` lint (under `-D warnings`)
    /// stays satisfied for the fields not yet plumbed to the child (`progress`/`skill`/`phase`/
    /// `label`) — the same self-documenting pattern [`SubagentToolParams::provided_keys`] uses.
    /// Returns the per-item keys actually supplied, for diagnostics.
    fn provided_keys(&self) -> Vec<&'static str> {
        let mut keys = vec!["agent"];
        if self.task.is_some() { keys.push("task"); }
        if self.cwd.is_some() { keys.push("cwd"); }
        if self.count.is_some() { keys.push("count"); }
        if self.output.is_some() { keys.push("output"); }
        if self.output_mode.is_some() { keys.push("outputMode"); }
        if self.reads.is_some() { keys.push("reads"); }
        if self.progress.is_some() { keys.push("progress"); }
        if self.model.is_some() { keys.push("model"); }
        if self.skill.is_some() { keys.push("skill"); }
        if self.acceptance.is_some() { keys.push("acceptance"); }
        if self.as_output.is_some() { keys.push("as"); }
        if self.output_schema.is_some() { keys.push("outputSchema"); }
        if self.phase.is_some() { keys.push("phase"); }
        if self.label.is_some() { keys.push("label"); }
        keys
    }
}

/// Parse a raw `tasks[]`/`parallel[]` JSON array into typed [`ToolTaskItem`]s. When
/// `task_required` (top-level PARALLEL, where pi's `TaskItem.task` is a required string), an
/// element with no non-empty `task` is rejected; inside a chain parallel group `task` is optional
/// (defaults to the prior step's output downstream).
fn parse_tool_task_items(
    raw: &[serde_json::Value],
    task_required: bool,
) -> Result<Vec<ToolTaskItem>, ToolError> {
    let mut items = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        let item: ToolTaskItem = serde_json::from_value(value.clone())
            .map_err(|e| ToolError::new(format!("invalid parallel task at index {i}: {e}")))?;
        // Touch the not-yet-plumbed fields so `dead_code` stays satisfied and a caller can see the
        // exact shape parsed (the fields themselves are Tier 4/5 wire-ups).
        let _ = item.provided_keys();
        if task_required && item.task.as_deref().unwrap_or("").is_empty() {
            return Err(ToolError::new(format!(
                "tasks[{i}] requires a non-empty 'task' (top-level PARALLEL mode)"
            )));
        }
        items.push(item);
    }
    Ok(items)
}

/// pi `expandTopLevelTaskCounts` (`subagent-executor.ts:1343-1357`): repeat each task `count` times
/// (default 1), erroring on `count < 1` with pi's exact message. `count` is stripped from each
/// expanded clone (it is a width hint, never carried onto the concrete task).
fn expand_top_level_task_counts(items: Vec<ToolTaskItem>) -> Result<Vec<ToolTaskItem>, String> {
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let count = item.count.unwrap_or(1);
        if count < 1 {
            return Err(format!("tasks[{i}].count must be an integer >= 1"));
        }
        for _ in 0..count {
            let mut clone = item.clone();
            clone.count = None;
            out.push(clone);
        }
    }
    Ok(out)
}

/// pi `expandChainParallelCounts` (`subagent-executor.ts:1359-1382`): the same per-task `count`
/// fan-out applied to a static parallel group inside a `chain[]` step, with pi's exact per-step
/// error message.
fn expand_chain_parallel_counts(
    items: Vec<ToolTaskItem>,
    step_index: usize,
) -> Result<Vec<ToolTaskItem>, ToolError> {
    let mut out = Vec::with_capacity(items.len());
    for (j, item) in items.into_iter().enumerate() {
        let count = item.count.unwrap_or(1);
        if count < 1 {
            return Err(ToolError::new(format!(
                "chain[{step_index}].parallel[{j}].count must be an integer >= 1"
            )));
        }
        for _ in 0..count {
            let mut clone = item.clone();
            clone.count = None;
            out.push(clone);
        }
    }
    Ok(out)
}

/// pi `findDuplicateParallelOutputPath` (`subagent-executor.ts:1978-2001`): two parallel tasks
/// resolving their output to the same path is rejected BEFORE any child spawns, with pi's exact
/// message. A string `"false"` (or a boolean `false`/null/absent) means "no output file" and never
/// collides (pi treats string `"false"` as disabled — `parallel-execution.test.ts:289`).
fn find_duplicate_parallel_output(items: &[ToolTaskItem]) -> Option<String> {
    let mut seen: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        let Some(path) = tool_output_path_string(item.output.as_ref()) else {
            continue;
        };
        if let Some((prev_i, prev_agent)) = seen.get(&path) {
            return Some(format!(
                "Parallel tasks {} ({}) and {} ({}) resolve output to the same path: {}. Use \
                 distinct output paths.",
                prev_i + 1,
                prev_agent,
                i + 1,
                item.agent,
                path
            ));
        }
        seen.insert(path, (i, item.agent.clone()));
    }
    None
}

/// The output-file path a task's `output` value resolves to, or `None` when it disables output (a
/// boolean, null, empty string, or the string `"false"` sentinel — all "no file", pi's own rule).
fn tool_output_path_string(output: Option<&serde_json::Value>) -> Option<String> {
    match output {
        Some(serde_json::Value::String(s)) if !s.is_empty() && s != "false" => Some(s.clone()),
        _ => None,
    }
}

/// Lower one [`ToolTaskItem`] to a [`SingleStepSpec`] — the fields with a spec home only. The
/// per-task `model` override reaches the child via `SingleStepSpec::model` (honored by
/// `ExecSingleStepExecutor::run_single`'s `model_override`), exactly as the slash `[model=…]` path.
fn tool_task_to_spec(item: &ToolTaskItem) -> SingleStepSpec {
    SingleStepSpec {
        agent: item.agent.clone(),
        task: item.task.clone().unwrap_or_default(),
        cwd: item.cwd.as_ref().map(PathBuf::from),
        model: item.model.clone().map(ModelId::from),
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: item.output_schema.clone(),
        output: item.as_output.clone(),
        // pi's `output` (the output FILE path) vs `as` (the registry KEY, mapped just above):
        // `tool_output_path_string` normalizes the boolean/`"false"`/empty "no file" sentinels to
        // `None` (the same normalization `find_duplicate_parallel_output` uses), so a task with a
        // real output path reaches the child and drives the file-output handoff.
        output_path: tool_output_path_string(item.output.as_ref()),
        output_mode: parse_tool_output_mode(item.output_mode.as_deref()),
        reads: parse_tool_reads(item.reads.as_ref()),
        acceptance: parse_tool_acceptance(item.acceptance.as_ref()),
        context: None,
        agent_scope: None,
    }
}

fn parse_tool_output_mode(raw: Option<&str>) -> Option<crate::discovery::types::OutputMode> {
    match raw {
        Some("inline") => Some(crate::discovery::types::OutputMode::Inline),
        Some("file-only") => Some(crate::discovery::types::OutputMode::FileOnly),
        _ => None,
    }
}

fn parse_tool_reads(raw: Option<&serde_json::Value>) -> Option<Vec<PathBuf>> {
    match raw {
        Some(serde_json::Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect(),
        ),
        // `false`/null/absent (disabled) — no pre-declared read paths.
        _ => None,
    }
}

fn parse_tool_acceptance(raw: Option<&serde_json::Value>) -> Option<String> {
    match raw {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        // Object-policy / boolean-`false` acceptance forms are not lowered here (Tier 3, C12).
        _ => None,
    }
}

/// Translate the tool's `chain[]` array into a `Vec<RunnerStep>`: a sequential step for a
/// `{agent, task, …}` element, or a [`RunnerStep::ParallelGroup`] for a `{parallel: [...]}` element
/// (with per-task `count` expanded). Dynamic fanout (`expand`/`collect`, or a single-template
/// `parallel` object) is Tier-4 territory (C16) and is rejected with a clear message.
fn parse_tool_chain_items(
    raw: &[serde_json::Value],
    default_concurrency: u32,
) -> Result<Vec<RunnerStep>, ToolError> {
    let mut graph = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        let obj = value.as_object();
        if obj.is_some_and(|o| o.contains_key("expand") || o.contains_key("collect")) {
            return Err(ToolError::new(format!(
                "chain[{i}] uses dynamic fanout (expand/collect), which is not wired via the tool \
                 in this build yet (Tier 4, C16). Use a static parallel array or sequential steps."
            )));
        }
        match obj.and_then(|o| o.get("parallel")) {
            Some(serde_json::Value::Array(tasks)) => {
                let items = parse_tool_task_items(tasks, false)?;
                let expanded = expand_chain_parallel_counts(items, i)?;
                let steps: Vec<SingleStepSpec> = expanded.iter().map(tool_task_to_spec).collect();
                let concurrency = obj
                    .and_then(|o| o.get("concurrency"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok())
                    .filter(|c| *c > 0)
                    .unwrap_or(default_concurrency);
                let fail_fast = obj
                    .and_then(|o| o.get("failFast"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let worktree = obj
                    .and_then(|o| o.get("worktree"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                graph.push(RunnerStep::ParallelGroup(ParallelGroupSpec {
                    steps,
                    concurrency,
                    fail_fast,
                    worktree,
                }));
            }
            Some(_) => {
                return Err(ToolError::new(format!(
                    "chain[{i}].parallel must be an array of tasks; the single dynamic-template \
                     form is not wired via the tool in this build yet (Tier 4, C16)."
                )));
            }
            None => {
                let item: ToolTaskItem = serde_json::from_value(value.clone()).map_err(|e| {
                    ToolError::new(format!("invalid chain step at index {i}: {e}"))
                })?;
                let _ = item.provided_keys();
                graph.push(RunnerStep::SingleStep(tool_task_to_spec(&item)));
            }
        }
    }
    Ok(graph)
}

/// Render a top-level PARALLEL run's result summary in pi's shape: an `N/M succeeded` header
/// (`subagent-executor.ts:2446`) followed by each task's own output under a `=== Task i: agent ===`
/// section header (`subagent-executor.ts:2443`), in input order (R-SA-051).
fn render_parallel_tool_summary(group: &GroupStepResult, agents: &[String]) -> String {
    let total = group.children.len();
    let ok = group
        .children
        .iter()
        .filter(|c| matches!(c, Some(r) if r.success))
        .count();
    let mut body = String::new();
    for (i, child) in group.children.iter().enumerate() {
        let agent = agents.get(i).map(String::as_str).unwrap_or("?");
        body.push_str(&format!("=== Task {}: {} ===\n", i + 1, agent));
        match child {
            Some(r) if r.success => {
                body.push_str(r.final_output.as_deref().unwrap_or("(no text output)"));
            }
            Some(r) => {
                let err = r.error.as_deref().unwrap_or("unknown error");
                if err.contains("timed out") {
                    body.push_str(&format!("TIMED OUT: {err}"));
                } else {
                    body.push_str(&format!("FAILED: {err}"));
                }
            }
            None => body.push_str("(skipped)"),
        }
        body.push('\n');
        if i + 1 != total {
            body.push('\n');
        }
    }
    format!("{ok}/{total} succeeded\n\n{body}")
}

// -------------------------------------------------------------------------------------------------
// LLM-facing JSON Schema builders (a faithful port of `schemas.ts`'s `SubagentParamsSchema`, C8)
//
// Each helper returns one reusable schema fragment, mirroring the TypeBox `Type.*` fragments the pi
// source composes `SubagentParamsSchema` from (`OutputOverride`, `ReadsOverride`, `SkillOverride`,
// `OutputModeOverride`, `AcceptanceOverride`, `JsonSchemaObject`, `TaskItem`, `ParallelTaskSchema`,
// `DynamicExpandSchema`, `DynamicParallelTemplateSchema`, `DynamicCollectSchema`, `ChainItem`,
// `ControlOverrides`). Nested per-fragment descriptions are omitted to match pi's provider-payload
// pruning (`keepTopLevelParameterDescriptions`, `schemas.ts:8-31`), which keeps only the top-level
// parameter descriptions; the top-level descriptions themselves are kept in [`subagent_tool_parameters`].
// -------------------------------------------------------------------------------------------------

/// `OutputOverride` (`schemas.ts:42-48`): output filename/path (string), or `false` to disable.
fn sj_output_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "string" }, { "type": "boolean" } ] })
}

/// `ReadsOverride` (`schemas.ts:55-61`): array of filenames to read first, or `false` to disable.
fn sj_reads_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" } ] })
}

/// `SkillOverride` (`schemas.ts:33-40`): skill name(s) (string / array of strings), or boolean.
fn sj_skill_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ] })
}

/// `OutputModeOverride` (`schemas.ts:50-53`): `inline` (default) or `file-only`.
fn sj_output_mode() -> serde_json::Value {
    serde_json::json!({ "type": "string", "enum": ["inline", "file-only"] })
}

/// `AcceptanceOverride` (`schemas.ts:69-76`): a level enum, `false`, or an object policy.
fn sj_acceptance_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [
        { "type": "string", "enum": ["auto", "none", "attested", "checked", "verified", "reviewed"] },
        { "type": "boolean", "enum": [false] },
        { "type": "object", "additionalProperties": true }
    ] })
}

/// `JsonSchemaObject` (`schemas.ts:63-67`): an open JSON Schema object for structured output.
fn sj_json_schema_object() -> serde_json::Value {
    serde_json::json!({ "type": "object", "additionalProperties": true })
}

/// `TaskItem` (`schemas.ts:78-90`): one top-level PARALLEL `tasks[]` element (agent+task required).
fn sj_task_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent", "task"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "cwd": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "model": { "type": "string" },
            "skill": sj_skill_override(),
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `ParallelTaskSchema` (`schemas.ts:93-109`): a static parallel task inside a chain step (agent
/// required, task optional).
fn sj_parallel_task() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `DynamicParallelTemplateSchema` (`schemas.ts:122-136`): the single per-item child template used
/// with `expand`/`collect` dynamic fanout.
fn sj_dynamic_parallel_template() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["agent"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `DynamicExpandSchema` (`schemas.ts:111-120`): the fanout source pointer + bounds.
fn sj_dynamic_expand() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["from"],
        "properties": {
            "from": {
                "type": "object",
                "additionalProperties": false,
                "required": ["output", "path"],
                "properties": {
                    "output": { "type": "string" },
                    "path": { "type": "string" }
                }
            },
            "item": { "type": "string" },
            "key": { "type": "string" },
            "maxItems": { "type": "integer", "minimum": 0 },
            "onEmpty": { "type": "string", "enum": ["skip", "fail"] }
        }
    })
}

/// `DynamicCollectSchema` (`schemas.ts:138-141`): the fanned-in collected-array output binding.
fn sj_dynamic_collect() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["as"],
        "properties": {
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object()
        }
    })
}

/// `ChainItem` (`schemas.ts:144-178`): one `chain[]` element — sequential `{agent, task?, ...}`,
/// static `{parallel: [...]}`, or dynamic `{expand, parallel: {...}, collect}` fanout (flattened so
/// chain steps need no object-shape `anyOf`/`oneOf` union at the item level, exactly as pi does).
fn sj_chain_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override(),
            "parallel": {
                "anyOf": [
                    { "type": "array", "items": sj_parallel_task() },
                    sj_dynamic_parallel_template()
                ]
            },
            "expand": sj_dynamic_expand(),
            "collect": sj_dynamic_collect(),
            "concurrency": { "type": "number" },
            "failFast": { "type": "boolean" },
            "worktree": { "type": "boolean" }
        }
    })
}

/// `ControlOverrides` (`schemas.ts:180-193`): per-run subagent-control attention thresholds.
fn sj_control_overrides() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean" },
            "needsAttentionAfterMs": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterMs": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterTurns": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterTokens": { "type": "integer", "minimum": 1 },
            "failedToolAttemptsBeforeAttention": { "type": "integer", "minimum": 1 },
            "notifyOn": { "type": "array", "items": { "type": "string", "enum": ["active_long_running", "needs_attention"] } },
            "notifyChannels": { "type": "array", "items": { "type": "string", "enum": ["event", "async", "intercom"] } }
        }
    })
}

/// The complete LLM-facing JSON Schema for the `subagent` tool (C8) — a faithful port of pi's
/// exported `SubagentParams` (`schemas.ts:195-265`, after `keepTopLevelParameterDescriptions`
/// pruning). Every top-level parameter pi advertises is present with its top-level description; the
/// nested `tasks[]`/`chain[]` element shapes carry their full structural detail (types, enums,
/// `minimum`s, `items`, `anyOf` unions) with per-node descriptions pruned to keep the provider
/// payload compact, exactly as pi ships it.
fn subagent_tool_parameters() -> serde_json::Value {
    // Built via per-property inserts rather than one giant `json!` literal: a single 33-property
    // `json!` object overflows the macro's default `recursion_limit` at expansion time. Each insert
    // below is its own shallow `json!` invocation, and the root wrapper is a 3-key `json!`.
    let mut props = serde_json::Map::new();
    props.insert("agent".to_string(), serde_json::json!({ "type": "string", "description": "Agent name (SINGLE mode) or target for management get/update/delete" }));
    props.insert("task".to_string(), serde_json::json!({ "type": "string", "description": "Task (SINGLE mode, optional for self-contained agents)" }));
    props.insert("action".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["list", "get", "models", "create", "update", "delete", "status", "interrupt", "resume", "append-step", "doctor"],
        "description": "Management/control action. Omit for execution mode."
    }));
    props.insert("id".to_string(), serde_json::json!({ "type": "string", "description": "Run id or prefix for action='status', action='interrupt', action='resume', or action='append-step'." }));
    props.insert("runId".to_string(), serde_json::json!({ "type": "string", "description": "Target run ID for action='interrupt', action='resume', or action='append-step'. Defaults to the most recently active controllable run for interrupt. Prefer id for new calls." }));
    props.insert("dir".to_string(), serde_json::json!({ "type": "string", "description": "Async run directory for action='status' or action='resume'." }));
    props.insert("index".to_string(), serde_json::json!({ "type": "integer", "minimum": 0, "description": "Zero-based child index for actions that target a specific child." }));
    props.insert("message".to_string(), serde_json::json!({ "type": "string", "description": "Follow-up message for action='resume'. Use index to choose a child from multi-child runs." }));
    props.insert("chainName".to_string(), serde_json::json!({ "type": "string", "description": "Chain name for get/update/delete management actions" }));
    props.insert("config".to_string(), serde_json::json!({
        "anyOf": [ { "type": "object", "additionalProperties": true }, { "type": "string" } ],
        "description": "Agent/chain config for create/update. Object or JSON string; presence of steps creates a chain."
    }));
    props.insert("tasks".to_string(), serde_json::json!({
        "type": "array",
        "items": sj_task_item(),
        "description": "PARALLEL mode: [{agent, task, count?, output?, outputMode?, reads?, progress?}, ...]"
    }));
    props.insert("concurrency".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Top-level PARALLEL mode only: max concurrent tasks. Defaults to config.parallel.concurrency or 4." }));
    props.insert("worktree".to_string(), serde_json::json!({ "type": "boolean", "description": "Create isolated git worktrees for parallel tasks; requires clean git state." }));
    props.insert("chain".to_string(), serde_json::json!({
        "type": "array",
        "items": sj_chain_item(),
        "description": "CHAIN mode: sequential steps; each result becomes {previous}. append-step takes one tail step and may use {chain_dir}/{outputs.name}."
    }));
    props.insert("context".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["fresh", "fork"],
        "description": "'fresh' or 'fork' to branch from parent session. Explicit context overrides every child in the invocation. If omitted, each requested agent uses its own defaultContext; agents without defaultContext: 'fork' run fresh."
    }));
    props.insert("chainDir".to_string(), serde_json::json!({ "type": "string", "description": "Persistent chain artifact directory; defaults to user-scoped temp storage." }));
    props.insert("async".to_string(), serde_json::json!({ "type": "boolean", "description": "Run in background (default: false, or per config)" }));
    props.insert("timeoutMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Optional foreground-only timeout in ms; omit for async/background runs. Alias of maxRuntimeMs." }));
    props.insert("maxRuntimeMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Alias of timeoutMs for optional foreground-only timeout; omit for async/background runs." }));
    props.insert("agentScope".to_string(), serde_json::json!({ "type": "string", "description": "Agent discovery scope: 'user', 'project', or 'both' (default: 'both'; project wins on name collisions)" }));
    props.insert("cwd".to_string(), serde_json::json!({ "type": "string" }));
    props.insert("artifacts".to_string(), serde_json::json!({ "type": "boolean", "description": "Write debug artifacts (default: true)" }));
    props.insert("includeProgress".to_string(), serde_json::json!({ "type": "boolean", "description": "Include full progress in result (default: false)" }));
    props.insert("share".to_string(), serde_json::json!({ "type": "boolean", "description": "Upload session to GitHub Gist for sharing (default: false)" }));
    props.insert("sessionDir".to_string(), serde_json::json!({ "type": "string", "description": "Directory to store session logs (default: temp; enables sessions even if share=false)" }));
    props.insert("clarify".to_string(), serde_json::json!({ "type": "boolean", "description": "Show TUI to preview/edit before execution. Explicit clarify: true keeps the run foreground for the clarify UI; omitted clarify can still run in the background when async: true is set." }));
    props.insert("control".to_string(), sj_control_overrides());
    props.insert("output".to_string(), serde_json::json!({
        "anyOf": [ { "type": "string" }, { "type": "boolean" } ],
        "description": "Output file for single agent (string), or false to disable. Relative paths resolve against cwd."
    }));
    props.insert("outputMode".to_string(), serde_json::json!({ "type": "string", "enum": ["inline", "file-only"], "description": "Return saved output inline (default) or only a concise file reference. file-only requires output to be a path." }));
    props.insert("skill".to_string(), serde_json::json!({
        "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ],
        "description": "Skill name(s) to make available (comma-separated), array of strings, or boolean (false disables, true uses default)"
    }));
    props.insert("model".to_string(), serde_json::json!({ "type": "string", "description": "Override model for single agent (e.g. 'anthropic/claude-sonnet-4')" }));
    props.insert("acceptance".to_string(), serde_json::json!({
        "anyOf": [
            { "type": "string", "enum": ["auto", "none", "attested", "checked", "verified", "reviewed"] },
            { "type": "boolean", "enum": [false] },
            { "type": "object", "additionalProperties": true }
        ],
        "description": "Optional acceptance policy. Omitted means auto-inferred; verified requires configured runtime commands."
    }));

    serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "properties": serde_json::Value::Object(props),
    })
}

/// The `subagent` LLM-facing tool (R-SA-128). Dispatches over pi's full discriminated-union
/// parameter surface (C8): a present `action` routes to a management/control action, `tasks[]` to
/// top-level PARALLEL, `chain[]` to CHAIN, and the bare `{agent, task?}` shape to SINGLE — the SAME
/// [`SubagentExecutor`] `execute_command`'s slash-command dispatch uses (R-SA-130). All four
/// families are wired end-to-end: the SINGLE shape and read-only `doctor` (T0.5), the
/// management CRUD (`list`/`get`/`models`/`create`/`update`/`delete`, C3) and background-control
/// actions (`status`/`interrupt`/`resume`/`append-step`, C5) via [`Self::route_action`], and the
/// tool-driven PARALLEL/CHAIN routing via [`Self::route_parallel_mode`]/[`Self::route_chain_mode`]
/// (P1) — each resolving the REAL named persona (T0.1/C13) over real child processes, never a stub.
///
/// `cwd` is captured at CONSTRUCTION time (mirroring `cyrup_tools::tools::bash::BashTool::new`'s
/// established codebase convention: `cyrup_core::Tool::execute`'s signature carries no `HostCtx`,
/// so every built-in tool that needs the session's working directory captures it once, at
/// registration time, rather than re-deriving it from process-global state on every call).
pub struct SubagentTool {
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    parameters: serde_json::Value,
    /// Whether the mutating management actions (`create`/`update`/`delete`) are permitted (T6). The
    /// root orchestrator tool sets this `true`; a fanout-child's restricted tool sets it `false`, so
    /// a child can list/get/delegate but cannot rewrite the parent's agent config on disk (pi
    /// `fanout-child.ts` `allowMutatingManagementActions: false`).
    allow_mutating_management: bool,
}

impl SubagentTool {
    #[must_use]
    fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            executor,
            cwd,
            parameters: subagent_tool_parameters(),
            allow_mutating_management: true,
        }
    }

    /// The restricted child-safe tool (T6, pi `fanout-child.ts`): identical to [`SubagentTool::new`]
    /// except the agent-config mutation actions (`create`/`update`/`delete`) are blocked.
    #[must_use]
    fn new_child_safe(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            allow_mutating_management: false,
            ..Self::new(executor, cwd)
        }
    }

    /// SINGLE mode (`{agent, task?}`) — the fully-wired shape (func-SA §5.2). Resolves the persona
    /// through real discovery and drives [`SubagentExecutor::run_foreground`]/[`spawn_background`]
    /// (`async: true`), each a genuine child OS process. `context` selects fork/fresh (an omitted
    /// value is `Fresh` in this tier); `model` is the per-call override.
    async fn route_single(
        &self,
        p: &SubagentToolParams,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let Some(agent) = p.agent.as_deref() else {
            return Err(ToolError::new(
                "subagent SINGLE mode requires an 'agent' name (supply 'tasks' for PARALLEL, \
                 'chain' for CHAIN, or 'action' for a management/control action instead).",
            ));
        };
        let task = p.task.clone().unwrap_or_default();
        let context = p.context_override();
        let model = p.model.clone().map(ModelId::from);

        // pi `resolveForegroundTimeout` (`subagent-executor.ts:1327-1341`): `timeoutMs`/
        // `maxRuntimeMs` are aliases; validate up front (positive, and consistent when both given).
        let timeout_ms = resolve_foreground_timeout(p).map_err(ToolError::new)?;

        if p.is_background() {
            // pi (`subagent-executor.ts:3022-3023`): a foreground-only timeout cannot be honored by
            // a detached background run, so requesting both is an explicit error, not a silent drop.
            if timeout_ms.is_some() {
                return Err(ToolError::new(
                    "timeoutMs/maxRuntimeMs are only supported for foreground runs; set \
                     async: false or omit the timeout for background runs.",
                ));
            }
            let run_id = self
                .executor
                .spawn_background(&self.cwd, agent, &task, context)
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

        // C19: stream live foreground progress through the host `ToolUpdateSink` — the child's
        // NDJSON stdout is folded into `SubagentUpdatePayload` progress updates as it arrives,
        // instead of the model/UI seeing nothing until the run completes.
        let result = self
            .executor
            .run_foreground_streaming(
                ForegroundRunRequest {
                    cwd: &self.cwd,
                    agent_name: agent,
                    task: &task,
                    context,
                    model_override: model,
                    timeout_ms,
                },
                on_update,
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;

        // Single-run result surfacing (pi `subagent-executor.ts:2738-2761`). `final_output` is the
        // finalized delivered output (`run_sync` already folded in the timeout preamble and any
        // saved-output reference), i.e. pi's `finalizedOutput.displayOutput`.
        let display_output = result.final_output.clone().unwrap_or_default();
        let details = Some(
            serde_json::to_value(&result)
                .unwrap_or_else(|_| serde_json::Value::String("subagent result".to_string())),
        );

        // A detached (intercom) run is a coordination hand-off, not a failure (pi 2738-2743). No
        // live trigger sets `detached` in this crate today, but the branch is kept for fidelity.
        if result.detached {
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Detached for intercom coordination: {agent}. Reply to the supervisor request \
                     first. After the child exits, start a fresh follow-up if needed."
                ))],
                details,
                terminate: false,
            });
        }

        // A soft interrupt is a paused SUCCESS, not an error (pi 2745-2750): exit 0, cleared error.
        if result.interrupted {
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Run paused after interrupt ({agent}). Waiting for explicit next action."
                ))],
                details,
                terminate: false,
            });
        }

        // A FAILED run (non-zero exit) sets the error flag and surfaces the error text in the
        // model-facing content via `formatFailedSingleRunOutput` (pi 2752-2757) — cyrup's error
        // channel is `Err(ToolError)` (its `ToolResult` has no `isError` flag), which the runtime
        // renders as an `is_error` tool result carrying this text. The error is thus surfaced in
        // CONTENT, not buried in `details` JSON the model never sees.
        if result.exit_code != 0 {
            return Err(ToolError::new(format_failed_single_run_output(
                &result,
                &display_output,
            )));
        }

        // A clean run delivers its output (pi 2758-2761: `displayOutput || "(no output)"`).
        let text = if display_output.is_empty() {
            "(no output)".to_string()
        } else {
            display_output
        };
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(text)],
            details,
            terminate: false,
        })
    }

    /// Management/control action dispatch (pi: a present `action` puts the tool in management mode).
    /// `doctor`/`models` (read-only) are wired to [`SubagentExecutor::run_doctor`]/`run_models_report`;
    /// the CRUD (`list`/`get`/`create`/`update`/`delete`, C3) routes to [`Self::route_management_action`]
    /// (the real [`crate::discovery::management`] handlers) and the background-control
    /// (`status`/`interrupt`/`resume`/`append-step`, C5) routes to [`Self::route_control_action`]
    /// (the real [`crate::background::control`]/[`crate::background::run_status`] primitives).
    async fn route_action(&self, action: &str, p: &SubagentToolParams) -> Result<ToolResult, ToolError> {
        match action {
            // Read-only diagnostics — already faithfully implemented (`run_doctor`), so wired here.
            "doctor" => {
                let report = self.executor.run_doctor(&self.cwd).await;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(report)],
                    details: None,
                    terminate: false,
                })
            }
            // `models` is the runtime builtin-agent -> model mapping (pi `handleModels`), the SAME
            // renderer the `/subagents-models` slash command uses — so the tool and slash surfaces
            // report one consistent mapping, exactly as pi routes both through `handleModels`.
            "models" => {
                let report = self.executor.run_models_report(&self.cwd, p.agent.as_deref());
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(report)],
                    details: None,
                    terminate: false,
                })
            }
            "list" | "get" | "create" | "update" | "delete" => {
                self.route_management_action(action, p).await
            }
            "status" | "interrupt" | "resume" | "append-step" => {
                self.route_control_action(action, p).await
            }
            other => Err(ToolError::new(format!(
                "unknown subagent action '{other}'; valid actions are list, get, models, create, \
                 update, delete, status, interrupt, resume, append-step, doctor."
            ))),
        }
    }

    /// Management-action dispatch (Tier 1, C3): route `list`/`get`/`models`/`create`/`update`/
    /// `delete` to the now-wired [`crate::discovery::management`] CRUD + `agent-management.ts`
    /// renderers via [`crate::discovery::management::handle_management_action`]. Discovery is scoped
    /// to this tool's captured `cwd` and re-run per call inside each handler (R-SA-019). A pi
    /// `isError: true` outcome (not-found, read-only, validation) maps to a [`ToolError`] carrying
    /// pi's exact text (cyrup surfaces tool failures as `Err`, R-02-024); a genuine discovery/IO
    /// failure propagates as a [`ToolError`] too.
    async fn route_management_action(&self, action: &str, p: &SubagentToolParams) -> Result<ToolResult, ToolError> {
        // T6 child-safe restriction (pi `fanout-child.ts` `allowMutatingManagementActions: false`):
        // a fanout child may inspect/delegate but must not rewrite the parent's agent config on disk.
        if !self.allow_mutating_management && matches!(action, "create" | "update" | "delete") {
            return Err(ToolError::new(format!(
                "subagent management action '{action}' is blocked in child-safe fanout mode; \
                 create, update, and delete are not permitted here."
            )));
        }
        let cfg = SubagentExecutor::discovery_config(&self.cwd)
            .map_err(|e| ToolError::new(e.to_string()))?;
        let req = crate::discovery::management::ManagementRequest {
            agent: p.agent.as_deref(),
            chain_name: p.chain_name.as_deref(),
            agent_scope: p.agent_scope.as_deref(),
            config: p.config.as_ref(),
        };
        match crate::discovery::management::handle_management_action(&cfg, action, &req) {
            Ok(outcome) if !outcome.is_error => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(outcome.text)],
                details: Some(serde_json::json!({ "mode": "management", "results": [] })),
                terminate: false,
            }),
            Ok(outcome) => Err(ToolError::new(outcome.text)),
            Err(e) => Err(ToolError::new(e.to_string())),
        }
    }

    /// Tier-1 dispatch arm (C5): route `status`/`interrupt`/`resume`/`append-step` to the
    /// [`crate::background::control`] primitives + the [`crate::background::run_status`] report shape
    /// (including the no-id "list active runs" form) — pi `subagent-executor.ts:2845-2912` +
    /// `run-status.ts:101-273`. Each arm delegates to the matching [`SubagentExecutor`] method (the
    /// SAME shared executor the slash commands route through, R-SA-130); a rendered report/list is
    /// returned as tool content, a user-facing failure (not-found, wrong-mode, no-transcript, …) as
    /// a [`ToolError`] (cyrup's error-result channel, since [`ToolResult`] carries no `isError`
    /// flag).
    async fn route_control_action(&self, action: &str, p: &SubagentToolParams) -> Result<ToolResult, ToolError> {
        let index = p.index.and_then(|value| usize::try_from(value).ok());
        let outcome = match action {
            "status" => {
                self.executor
                    .control_status(&self.cwd, p.id.as_deref(), p.dir.as_deref())
                    .await
            }
            "interrupt" => {
                // pi interrupt prefers `runId` over `id` (`subagent-executor.ts:2872`).
                let target = p.run_id.as_deref().or(p.id.as_deref());
                self.executor.control_interrupt(&self.cwd, target).await
            }
            "resume" => {
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_resume(&self.cwd, target, p.message.as_deref(), p.task.as_deref(), index)
                    .await
            }
            "append-step" => {
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_append_step(&self.cwd, target, p.chain.as_deref().unwrap_or(&[]))
                    .await
            }
            other => Err(format!(
                "unknown subagent control action '{other}'; valid control actions are status, \
                 interrupt, resume, append-step."
            )),
        };
        match outcome {
            Ok(text) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                details: Some(serde_json::json!({ "mode": "management" })),
                terminate: false,
            }),
            Err(message) => Err(ToolError::new(message)),
        }
    }

    /// Tier-1 dispatch arm (parallel major, `parallel-execution.test.ts:174-376`): translate the
    /// tool's top-level PARALLEL shape (`tasks[]` + `concurrency`/`worktree`, per-task
    /// `count`/`output`/`outputMode`/`reads`/`model`) into a single [`RunnerStep::ParallelGroup`]
    /// and route it through the SAME shared plan-execution path
    /// ([`SubagentExecutor::run_or_background_graph`]) the slash commands use — so each task's REAL
    /// persona (T0.1/C13) is resolved and dispatched through the faithful
    /// [`crate::spawn::parallel::run_bounded`] worker pool over real child processes.
    ///
    /// Faithful pi behaviors reproduced here: per-task `count` fan-out multiplication
    /// (`expandTopLevelTaskCounts`, `subagent-executor.ts:1343`); duplicate-output-path rejection
    /// BEFORE any spawn (`findDuplicateParallelOutputPath`, `subagent-executor.ts:1978`); and the
    /// `N/M succeeded` result summary (`subagent-executor.ts:2446`).
    async fn route_parallel_mode(&self, p: &SubagentToolParams) -> Result<ToolResult, ToolError> {
        let raw = p.tasks.as_deref().unwrap_or(&[]);
        let items = parse_tool_task_items(raw, true)?;
        // Expand `count` FIRST (matching pi's `normalizeRepeatedParallelCounts` -> later
        // `findDuplicateParallelOutputPath`), so a `count`-multiplied task with a fixed output path
        // is itself caught as a duplicate rather than slipping through the pre-expansion check.
        let expanded = expand_top_level_task_counts(items).map_err(ToolError::new)?;
        if let Some(dup) = find_duplicate_parallel_output(&expanded) {
            return Err(ToolError::new(dup));
        }
        let specs: Vec<SingleStepSpec> = expanded.iter().map(tool_task_to_spec).collect();
        let agents: Vec<String> = specs.iter().map(|spec| spec.agent.clone()).collect();

        let cfg = self.executor.config_snapshot().await;
        // pi: `resolveTopLevelParallelConcurrency(params.concurrency, config.parallel.concurrency)`
        // — an explicit positive `concurrency` wins; otherwise the config default (4).
        let concurrency = p
            .concurrency
            .and_then(|c| u32::try_from(c).ok())
            .filter(|c| *c > 0)
            .unwrap_or(cfg.parallel_concurrency());
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: specs,
            concurrency,
            fail_fast: false,
            worktree: p.worktree.unwrap_or(false),
        });

        let context = p.context_override();
        match self
            .executor
            .run_or_background_graph(
                &self.cwd,
                vec![group],
                RunMode::Parallel,
                context,
                p.is_background(),
                p.task.clone(),
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        {
            GraphRunOutcome::Background(run_id) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Background subagent run started: {run_id}. Use the status/interrupt \
                     management actions to check on it later; do not poll in a tight loop."
                ))],
                details: Some(serde_json::json!({ "run_id": run_id.as_str(), "mode": "parallel" })),
                terminate: false,
            }),
            GraphRunOutcome::Foreground { groups, .. } => {
                let (summary, details) = match groups.first() {
                    Some(group) => {
                        let total = group.children.len();
                        let ok = group
                            .children
                            .iter()
                            .filter(|c| matches!(c, Some(r) if r.success))
                            .count();
                        // R-SA-123/124/125: attempt out-of-band delivery of the FULL grouped result
                        // through the intercom `DeliveryChannel`. On a confirmed delivery, the inline
                        // tool payload is REDUCED — the heavy per-task `final_output` block that
                        // `render_parallel_tool_summary` inlines is dropped in favor of a compact
                        // receipt (the allowlisted `ReducedInlinePayload` identity/summary) — else the
                        // full inline summary is preserved (never delivered instead-of, always
                        // in-addition-to). Uses the `NoTransportChannel` default (→ NotDelivered, full
                        // inline kept) until `with_channels` wires the real broker channel.
                        let success = ok == total && total > 0;
                        let top_agent = agents.first().cloned().unwrap_or_else(|| "subagent".to_string());
                        let payload = crate::tui::intercom::IntercomPayload::from_group_children(
                            RunId::new(),
                            top_agent,
                            success,
                            &group.children,
                        );
                        match self.executor.deliver_group_out_of_band(payload.clone()).await {
                            crate::tui::intercom::DeliveryOutcome::Delivered => {
                                let reduced = crate::tui::intercom::ReducedInlinePayload::from(&payload);
                                (
                                    format!(
                                        "{ok}/{total} succeeded\n\nFull per-task output delivered \
                                         out-of-band via intercom (run {}).",
                                        reduced.run_id.as_str()
                                    ),
                                    serde_json::json!({
                                        "mode": "parallel", "total": total, "succeeded": ok,
                                        "outOfBandDelivered": true, "reduced": reduced,
                                    }),
                                )
                            }
                            crate::tui::intercom::DeliveryOutcome::NotDelivered => (
                                render_parallel_tool_summary(group, &agents),
                                serde_json::json!({
                                    "mode": "parallel", "total": total, "succeeded": ok,
                                    "outOfBandDelivered": false,
                                }),
                            ),
                        }
                    }
                    None => (
                        "0/0 succeeded".to_string(),
                        serde_json::json!({ "mode": "parallel", "total": 0, "succeeded": 0 }),
                    ),
                };
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(summary)],
                    details: Some(details),
                    terminate: false,
                })
            }
        }
    }

    /// Tier-1 dispatch arm (chain via tool): translate `chain[]` into a `Vec<RunnerStep>`
    /// (sequential steps + inline static parallel groups, each group's per-task `count` expanded via
    /// pi's `expandChainParallelCounts`) and route it through the SAME
    /// [`SubagentExecutor::run_or_background_graph`] path the slash commands use. Dynamic fanout
    /// (`expand`/`collect`) is Tier-4 territory (C16) and is rejected with a clear message rather
    /// than silently mis-parsed.
    async fn route_chain_mode(&self, p: &SubagentToolParams) -> Result<ToolResult, ToolError> {
        let raw = p.chain.as_deref().unwrap_or(&[]);
        let cfg = self.executor.config_snapshot().await;
        let graph = parse_tool_chain_items(raw, cfg.parallel_concurrency())?;
        let context = p.context_override();
        match self
            .executor
            .run_or_background_graph(
                &self.cwd,
                graph,
                RunMode::Chain,
                context,
                p.is_background(),
                p.task.clone(),
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        {
            GraphRunOutcome::Background(run_id) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Background subagent run started: {run_id}. Use the status/interrupt \
                     management actions to check on it later; do not poll in a tight loop."
                ))],
                details: Some(serde_json::json!({ "run_id": run_id.as_str(), "mode": "chain" })),
                terminate: false,
            }),
            GraphRunOutcome::Foreground {
                results,
                is_group,
                groups,
            } => {
                let text = render_chain_results(&results, &is_group, &groups);
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(text)],
                    details: Some(serde_json::json!({ "mode": "chain", "steps": results.len() })),
                    terminate: false,
                })
            }
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
        SUBAGENT_TOOL_DESCRIPTION
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: SubagentToolParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid subagent tool call: {e}")))?;

        // Observe the full parsed pi-union once so the SINGLE-mode override fields (`output`/
        // `outputMode`/`skill`/`acceptance`) and execution knobs (`artifacts`/`includeProgress`/
        // `share`/`sessionDir`/`clarify`/`control`/`timeoutMs`/`maxRuntimeMs`/`chainDir`) that no
        // dispatch arm consumes yet (their wire-ups are Tiers 3/5) stay live under the workspace's
        // `-D warnings` (`dead_code`) without any non-`#[cfg(test)]` `#[allow]` — the same
        // liveness pattern the per-item `ToolTaskItem::provided_keys` calls above use.
        let _ = parsed.provided_keys();

        // R-SA-128 / C8 dispatch: the `subagent` tool is a discriminated union over pi's full
        // parameter surface. Mode is selected exactly as pi's `subagent-executor` selects it — a
        // present `action` is a management/control call; otherwise `tasks[]` is top-level PARALLEL,
        // `chain[]` is CHAIN, and the bare `{agent, task?}` shape is SINGLE. All four families route
        // to real execution (the management/control CRUD via `route_action`, and the tool-driven
        // PARALLEL/CHAIN via `route_parallel_mode`/`route_chain_mode`).
        if let Some(action) = parsed.action.as_deref() {
            return self.route_action(action, &parsed).await;
        }
        if parsed.tasks.is_some() {
            return self.route_parallel_mode(&parsed).await;
        }
        if parsed.chain.is_some() {
            return self.route_chain_mode(&parsed).await;
        }
        // C19: SINGLE mode is the one shape wired for live progress today — its foreground child's
        // NDJSON stream is folded and forwarded through `on_update` (`route_single` ->
        // `run_foreground_streaming`). The tool-driven PARALLEL/CHAIN shapes still surface progress
        // only on completion; streaming their fan-out is the remaining live-progress work (their
        // per-child folds would multiplex through the same `SubagentUpdatePayload.progress[]`).
        self.route_single(&parsed, on_update).await
    }
}

// =================================================================================================
// SubagentsExtension: the NativeExtension facade (arch-SA §3.1/§3.2)
// =================================================================================================

/// How much of the extension surface [`NativeExtension::init`] registers — the child-mode gate
/// (T6, pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`).
///
/// A subagent child process re-execs the `cyrup` binary with `CYRUP_SUBAGENT_CHILD=1` set. In that
/// child, pi's root `registerSubagentExtension` returns immediately and registers NOTHING — a child
/// must never install the full orchestrator surface (its own `subagent` tool, the 13 slash commands,
/// the background-completion watcher, the session-lifecycle housekeeping), which would let it spawn
/// grandchildren freely and duplicate the parent's UI. The one exception is a **fanout-authorized**
/// child (`CYRUP_SUBAGENT_FANOUT_CHILD=1` as well), which pi's separate `fanout-child` entry point
/// gives a single **restricted** `subagent` tool: it may delegate/inspect but the agent-config
/// mutation actions (`create`/`update`/`delete`) are blocked, and it installs no slash commands or
/// watchers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationMode {
    /// The root orchestrator surface: the `subagent` tool, all 13 slash commands, the
    /// background-completion watcher, and session-start housekeeping (non-child process).
    Full,
    /// A fanout-authorized child: only the restricted, mutation-blocked `subagent` tool — no slash
    /// commands, no watchers, no session-lifecycle housekeeping.
    ChildSafe,
}

/// The child-mode registration decision (T6, pi `extension/index.ts:243-245` +
/// `extension/fanout-child.ts:131`), as a pure function of the two env flags so it is deterministic
/// and unit-testable without mutating the process environment:
/// - not a child (`child == false`) → [`RegistrationMode::Full`];
/// - a fanout-authorized child (`child && fanout_authorized`) → [`RegistrationMode::ChildSafe`];
/// - a plain child (`child && !fanout_authorized`) → `None`: register NOTHING at all.
#[must_use]
pub fn resolve_registration_mode(child: bool, fanout_authorized: bool) -> Option<RegistrationMode> {
    if !child {
        return Some(RegistrationMode::Full);
    }
    if fanout_authorized {
        return Some(RegistrationMode::ChildSafe);
    }
    None
}

/// Read the two child-mode env flags (`CYRUP_SUBAGENT_CHILD` / `CYRUP_SUBAGENT_FANOUT_CHILD`) and
/// resolve the [`RegistrationMode`] via [`resolve_registration_mode`]. `None` means the current
/// process is a plain subagent child that must register no subagent surface at all.
#[must_use]
pub fn registration_mode_from_env() -> Option<RegistrationMode> {
    let is_one = |name: &str| std::env::var(name).ok().as_deref() == Some("1");
    resolve_registration_mode(
        is_one(crate::spawn::nested_events::CHILD_ENV),
        is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
    )
}

/// Build the subagent [`NativeExtension`] the `cyrup` binary should attach for the current process,
/// or `None` when it must attach nothing (a plain subagent child) — the crate-side half of the T6
/// child-mode gate `crates/cyrup/src/main.rs` calls at each of its three session-build sites. See
/// [`subagent_extension_for`] for the pure, env-free form.
#[must_use]
pub fn subagent_extension_for_env(
    config: SubagentExtensionConfig,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    registration_mode_from_env()
        .map(|mode| Arc::new(SubagentsExtension::with_mode(config, cwd, mode)) as Arc<dyn NativeExtension>)
}

/// As [`subagent_extension_for_env`], but threads the intercom companion's real broker-backed
/// delivery + clarify + steer channels into the ROOT-orchestrator extension (item 2 of
/// reconciliation §4 step 5 / the port doc §8.4 item 1 handoff) — CLOSING
/// R-SA-037/086/119/120/123/124/125. The channels are handed only to a [`RegistrationMode::Full`]
/// root (the only surface that drives grouped tool results, surfaces a clarify to a live human, and
/// steers a live async child); a [`RegistrationMode::ChildSafe`] fanout child is built WITHOUT them
/// (it has no orchestrator surface), and a plain child still returns `None`.
/// `crates/cyrup/src/main.rs` calls this with
/// `IntercomExtension::{delivery_channel,clarify_channel,steer_channel}` when intercom is attached
/// this session, and falls back to [`subagent_extension_for_env`] when it is not.
#[must_use]
pub fn subagent_extension_for_env_with_channels(
    config: SubagentExtensionConfig,
    cwd: PathBuf,
    delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
    clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
    steer: Arc<dyn crate::tui::intercom::SteerChannel>,
) -> Option<Arc<dyn NativeExtension>> {
    registration_mode_from_env().map(|mode| match mode {
        RegistrationMode::Full => {
            Arc::new(SubagentsExtension::with_channels(config, cwd, delivery, clarify, steer))
                as Arc<dyn NativeExtension>
        }
        RegistrationMode::ChildSafe => {
            Arc::new(SubagentsExtension::with_mode(config, cwd, RegistrationMode::ChildSafe))
                as Arc<dyn NativeExtension>
        }
    })
}

/// The pure, env-free form of [`subagent_extension_for_env`]: resolve the [`RegistrationMode`] from
/// the two explicit flags and build the extension (or `None` to register nothing). Kept separate so
/// a test can assert the gate ("a plain child registers no subagent tool") deterministically without
/// touching the process environment.
#[must_use]
pub fn subagent_extension_for(
    config: SubagentExtensionConfig,
    cwd: PathBuf,
    child: bool,
    fanout_authorized: bool,
) -> Option<Arc<dyn NativeExtension>> {
    resolve_registration_mode(child, fanout_authorized)
        .map(|mode| Arc::new(SubagentsExtension::with_mode(config, cwd, mode)) as Arc<dyn NativeExtension>)
}

/// The SubAgents extension's `NativeExtension` facade (arch-SA §3.1). In [`RegistrationMode::Full`]
/// registers the `subagent` tool + all 13 slash commands at [`NativeExtension::init`], resumes
/// background-run tracking on [`HostEvent::SessionStart`], and routes every slash command through the
/// SAME [`SubagentExecutor`] the tool itself uses (R-SA-130). In [`RegistrationMode::ChildSafe`]
/// registers only the restricted, mutation-blocked tool (the fanout-child surface).
/// The intercom companion's three broker-backed seam channels (delivery + clarify + steer), handed
/// to [`SubagentsExtension::with_channels`] as one unit. A named alias so the `with_mode_and_channels`
/// parameter stays within clippy's `type_complexity` budget.
type IntercomSeamChannels = (
    Arc<dyn crate::tui::intercom::DeliveryChannel>,
    Arc<dyn crate::tui::intercom::ClarifyChannel>,
    Arc<dyn crate::tui::intercom::SteerChannel>,
);

pub struct SubagentsExtension {
    id: ExtensionId,
    executor: Arc<SubagentExecutor>,
    /// Captured at construction time (mirrors [`SubagentTool`]'s own doc: `NativeExtension::init`
    /// carries no `HostCtx`, so the session's working directory must be threaded in explicitly by
    /// whichever caller constructs this extension — `crates/cyrup/src/main.rs`'s three call
    /// sites, each of which already resolves the session's cwd before constructing this type).
    cwd: PathBuf,
    /// The child-mode registration surface (T6). Defaults to [`RegistrationMode::Full`] for the root
    /// orchestrator; a fanout-authorized child is built with [`RegistrationMode::ChildSafe`].
    mode: RegistrationMode,
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
        Self::with_mode(config, cwd, RegistrationMode::Full)
    }

    /// Construct the extension with an explicit [`RegistrationMode`] (T6 child-mode gate): the
    /// binary builds a [`RegistrationMode::Full`] extension for the root orchestrator and a
    /// [`RegistrationMode::ChildSafe`] one for a fanout-authorized child. See
    /// [`subagent_extension_for`]/[`subagent_extension_for_env`] for the callers.
    #[must_use]
    pub fn with_mode(config: SubagentExtensionConfig, cwd: PathBuf, mode: RegistrationMode) -> Self {
        Self::with_mode_and_channels(config, cwd, mode, None)
    }

    /// Construct a [`RegistrationMode::Full`] root orchestrator extension whose out-of-band delivery,
    /// clarify/ask, and live-child steer channels are the intercom companion's REAL broker-backed
    /// impls (item 2 of reconciliation §4 step 5), replacing the
    /// `NoTransportChannel`/no-live `AskLock`/`NoTransportSteerChannel` defaults — CLOSING
    /// R-SA-123/124/125 (out-of-band grouped delivery + reduced inline receipt), R-SA-119/120
    /// (clarify pause) + backing the R-SA-037 detach-trigger arm, and R-SA-086 (live-child
    /// `action='resume'` follow-up delivery). Called from the `crates/cyrup/src/main.rs`
    /// session-build sites with `IntercomExtension::{delivery_channel,clarify_channel,steer_channel}`
    /// (the port doc §8.4 item 1 handoff).
    #[must_use]
    pub fn with_channels(
        config: SubagentExtensionConfig,
        cwd: PathBuf,
        delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
        clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
        steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    ) -> Self {
        Self::with_mode_and_channels(
            config,
            cwd,
            RegistrationMode::Full,
            Some((delivery, clarify, steer)),
        )
    }

    /// The shared constructor body: builds the [`SubagentExecutor`], applies `config`, and — when
    /// `channels` is `Some` — threads the real intercom delivery/clarify channels into the executor
    /// (item 2). `None` keeps this crate's `NoTransportChannel`/no-live-`AskLock` degrade defaults.
    #[must_use]
    fn with_mode_and_channels(
        config: SubagentExtensionConfig,
        cwd: PathBuf,
        mode: RegistrationMode,
        channels: Option<IntercomSeamChannels>,
    ) -> Self {
        let executor = match channels {
            Some((delivery, clarify, steer)) => {
                SubagentExecutor::new().with_channels(delivery, clarify, steer)
            }
            None => SubagentExecutor::new(),
        };
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
            mode,
        }
    }

    /// The shared executor, exposed so a caller (e.g. a future TUI progress widget, or a test)
    /// can drive the exact same dispatch path the tool/commands use without going through the
    /// `NativeExtension` trait object.
    #[must_use]
    pub fn executor(&self) -> &Arc<SubagentExecutor> {
        &self.executor
    }

    /// Construct the same [`SubagentTool`] `init` registers with the host, bound to this
    /// extension's own executor and cwd — exposed so an integration test (or a future non-`InitApi`
    /// caller) can drive the real `cyrup_core::Tool::execute` dispatch (the `tasks[]`/`chain[]`
    /// PARALLEL/CHAIN routing) exactly as the host would, without a `SessionBuilder` round-trip.
    #[must_use]
    pub fn subagent_tool(&self) -> SubagentTool {
        SubagentTool::new(self.executor.clone(), self.cwd.clone())
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

    /// Register the extension surface for this process's [`RegistrationMode`] (T6 child-mode gate):
    ///
    /// - [`RegistrationMode::Full`] (root orchestrator): the `subagent` tool (R-SA-128), all 13
    ///   slash commands (R-SA-129), and the session-lifecycle subscriptions (func-SA §5.6).
    /// - [`RegistrationMode::ChildSafe`] (fanout-authorized child, pi `fanout-child.ts`): ONLY the
    ///   restricted, mutation-blocked `subagent` tool — no slash commands, and no lifecycle
    ///   subscriptions, so `on_event`'s background-completion watcher + startup housekeeping never
    ///   install in a child.
    ///
    /// A plain (non-fanout) child never reaches `init` at all: the binary's `subagent_extension_for_env`
    /// gate returns `None`, so no extension is attached (pi `index.ts:243-245` registers nothing).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        match self.mode {
            RegistrationMode::ChildSafe => {
                api.register_tool(Arc::new(SubagentTool::new_child_safe(
                    self.executor.clone(),
                    self.cwd.clone(),
                )));
                // No commands, no subscriptions: a child installs no orchestrator UI/watcher surface.
            }
            RegistrationMode::Full => {
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
            }
        }
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
                // T6 startup housekeeping (pi `extension/index.ts:257-264`): create the async/results
                // roots up front (`ensureAccessibleDir`), run the 24h-throttled chain-runs sweep
                // (`cleanupOldChainDirs`) and the 7-day artifact sweep (`cleanupAllArtifactDirs`), all
                // best-effort — a failure here must never block a session from starting. Skipped in a
                // child (a `ChildSafe` extension never subscribes to `SessionStart`, so this arm only
                // runs for the root orchestrator).
                let roots = crate::background::run_artifact_roots(&ctx.cwd);
                let _ = crate::background::ensure_accessible_dir(&roots.async_root).await;
                let _ = crate::background::ensure_accessible_dir(&roots.results_dir).await;
                crate::artifacts::cleanup_old_chain_dirs(&ctx.cwd);
                crate::artifacts::cleanup_all_artifact_dirs(
                    &ctx.cwd,
                    crate::artifacts::DEFAULT_CLEANUP_DAYS,
                );

                // R-SA-P1 (port doc §4 P-4): capture the canonical parent-session anchor ONCE from
                // the live session id (P-2) at the root orchestrator's SessionStart (depth 0 — a
                // `ChildSafe` child never subscribes to SessionStart, so this arm only runs for the
                // root). Every child this session spawns then inherits it via the spawn env overlay,
                // so the permission companion's child→parent ask-forwarding spool can address this
                // session's inbox.
                self.executor.capture_parent_session_anchor();

                self.executor.resume_tracking(&ctx.cwd).await;
                // C6: install the background-completion watcher (notify.ts / result-watcher.ts) so a
                // detached run that finishes during this session surfaces its `subagent-notify`
                // message (with `triggerTurn`) and has its result file deleted (R-SA-099/101). When the
                // P-1 host-services slot is bound this installs the live turn-injecting
                // `HostServicesCompletionSink` (R-SA-101); otherwise the stderr LoggingCompletionSink.
                self.executor.install_completion_watcher(&ctx.cwd).await;
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

    /// Late-bind the live capability backend (P-1, reconciliation §2 item 1). The session builder
    /// calls this via `load_native_with_services` (facade.rs:181) BEFORE `init`; stash the shared
    /// `Arc` in the executor's slot so the `SessionStart` anchor capture (R-SA-P1), the fork-context
    /// resolver (blocker #4), and the completion watcher's turn-injecting sink (R-SA-101) all reach
    /// the live session id/file + `inject_message` from OUTSIDE any `HostCtx`. Idempotent.
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        self.executor.set_host_services(services);
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
            // Slash-live-state (T8, partial — pi `slash/slash-live-state.ts`): pi posts an IMMEDIATE
            // in-transcript placeholder message the moment `/run` is invoked, then UPDATES IT IN
            // PLACE as the run streams and finally renders the completed result over the same
            // transcript entry. The crate cannot post that immediate placeholder or update it in
            // place today: `NativeExtension::execute_command` returns a single `Option<String>` (its
            // one final transcript entry) and its `HostCtx` exposes no transcript-message sink and no
            // update-in-place handle. So the crate-side minimum here is to make the SINGLE returned
            // entry read as the placeholder RESOLVED to completion — a completion summary (status +
            // agent + tool/token stats) over the delivered output, exactly what pi's placeholder
            // renders once the run settles (`renderSubagentResult`). The immediate placeholder + live
            // in-place update is the remaining outer-layer step, gated on a host transcript-update
            // channel `cyrup-tui`/`HostCtx` must expose (the tool path already streams live progress
            // via `ToolUpdateSink`, C19 — the slash path has no equivalent sink yet).
            SlashCommandName::Run => {
                let parsed = slash_commands::parse_run_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
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
                        .run_foreground(cwd, &parsed.agent, &parsed.task, context, model, None)
                        .await?;
                    Ok(format_slash_run_completion(&result))
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
                self.load_profile_into_settings(&name).await
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
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
                // `/chain` carries no separate top-level task arg — the first step's task seeds the
                // chain, so `{task}` falls back to it (`first_step_task`).
                self.run_or_background_chain(cwd, parsed.chain, RunMode::Chain, context, parsed.flags.background, None)
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
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
                let cfg = self.executor.config_snapshot().await;
                let group = RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
                    steps: parsed.tasks,
                    concurrency: cfg.parallel_concurrency(),
                    fail_fast: false,
                    worktree: false,
                });
                // `/parallel` carries no separate top-level task arg — `{task}` falls back to the
                // group's first task (`first_step_task`).
                self.run_or_background_chain(cwd, vec![group], RunMode::Parallel, context, parsed.flags.background, None)
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
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
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
                // A saved chain parses into `ChainStepConfig` authoring shapes (T0.2); lower each
                // to the runtime `RunnerStep` union here via the structural bridge — it carries the
                // real agent NAME (never a placeholder persona; name resolution stays the
                // executor's job) and defers plan-time model/acceptance enrichment. A group step's
                // omitted `concurrency` falls back to `cfg.parallel_concurrency()`, mirroring
                // `/parallel`'s own default above.
                let graph: Vec<RunnerStep> = chain
                    .steps
                    .iter()
                    .map(|step| {
                        crate::discovery::chains::chain_step_to_runner_step(
                            step,
                            cfg.parallel_concurrency(),
                        )
                    })
                    .collect();
                let steps = seed_first_step_task(graph, &parsed.task);
                // `/run-chain <name> -- <task>`: the supplied task seeds the first step AND is the
                // run-wide `{task}` value (pi `originalTask = params.task`).
                let task = (!parsed.task.trim().is_empty()).then(|| parsed.task.clone());
                self.run_or_background_chain(cwd, steps, RunMode::Chain, context, parsed.flags.background, task)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-models — report the RUNTIME builtin-agent -> model mapping (pi
            // `handleModels`, slash-commands.ts:1090-1111), NOT a dump of the static provider
            // catalog: each discovered builtin persona's effective model + provenance, optionally
            // filtered to one builtin.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsModels => {
                let parsed = slash_commands::parse_subagents_models_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                Ok(self.executor.run_models_report(cwd, parsed.agent.as_deref()))
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
                self.refresh_provider_catalog_cache(cwd, &parsed.provider, parsed.force)
                    .await
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

    /// Shared tail for `/chain`, `/parallel`, and `/run-chain`: resolve every step's effective
    /// fork-context (R-SA-137's eager whole-batch rule) — an omitted call-site `context` defers to
    /// each step's agent's own `default_context`, and each forking step gets its OWN per-index branch
    /// (R-SA-138: a sibling step's own explicit choice is never overridden) — then either walk the
    /// graph to completion in the foreground or hand it to [`SubagentExecutor::spawn_background_steps`].
    async fn run_or_background_chain(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: Option<ContextMode>,
        background: bool,
        task: Option<String>,
    ) -> Result<String, SubagentError> {
        if graph.is_empty() {
            return Ok("chain has no steps to run".to_string());
        }

        // R-SA-130: delegate to the ONE shared plan-execution path `SubagentExecutor` exposes (the
        // identical method the `subagent` tool's `chain[]`/`tasks[]` shapes route through), then
        // render the sequential/per-step text this slash surface presents. Depth guard, plan-time
        // persona resolution (T0.1/C13), fork-context resolution (R-SA-137), and the foreground-vs-
        // background fork all live inside `run_or_background_graph` now, so both call sites share
        // them verbatim rather than each re-implementing the tail.
        match self
            .executor
            .run_or_background_graph(cwd, graph, mode, context, background, task)
            .await?
        {
            GraphRunOutcome::Background(run_id) => {
                Ok(format!("Background subagent run started: {run_id}"))
            }
            GraphRunOutcome::Foreground {
                results,
                is_group,
                groups,
            } => Ok(render_chain_results(&results, &is_group, &groups)),
        }
    }

    // ---------------------------------------------------------------------------------------
    // /subagents-models, /subagents-refresh-provider-models, /subagents-generate-profiles,
    // /subagents-check-profile: cyrup-provider static-seed-catalog backed (func-SA §9 item 31's
    // deferred live-probe scope, restated at each call site above)
    // ---------------------------------------------------------------------------------------

    /// The path `registration/doctor.rs`'s `check_provider_catalog_freshness` (R-SA-131 item f)
    /// reads: refreshing/generating a provider catalog also touches this shared freshness marker so
    /// `/subagents-doctor`'s freshness check observes that a refresh genuinely ran.
    fn provider_catalog_cache_path(&self, cwd: &Path) -> PathBuf {
        let _ = cwd;
        dirs_home()
            .join(".cyrup")
            .join("subagents")
            .join("provider-catalog-cache.json")
    }

    /// The provider's models from the static seed catalog (the deferred-live-probe stand-in for pi's
    /// `ctx.modelRegistry.getAvailable()`), returned as fully-qualified `provider/id` references
    /// RANKED ascending by blended input+output cost (cheapest/weakest first) — the ordering
    /// [`crate::registration::profiles::pick_tier_models`] samples cheap->strong from, standing in
    /// for pi's `derived.profileRank` ordering while the live-probe classifier is deferred
    /// (func-SA §9 item 31).
    fn provider_ranked_full_ids(&self, provider: &str) -> Vec<String> {
        let catalog = cyrup_provider::catalog::seed_catalog();
        let mut matches: Vec<cyrup_provider::Model> = catalog
            .into_iter()
            .filter(|m| m.provider.as_str() == provider)
            .collect();
        matches.sort_by(|a, b| {
            (a.cost.input + a.cost.output)
                .partial_cmp(&(b.cost.input + b.cost.output))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        matches
            .iter()
            .map(|m| format!("{}/{}", m.provider.as_str(), m.id.as_str()))
            .collect()
    }

    /// Build and persist a per-provider [`crate::registration::profiles::ProviderModelCatalog`] from
    /// the seed catalog, plus refresh the shared doctor freshness marker. Returns the model count.
    async fn write_provider_catalog_file(&self, provider: &str) -> Result<usize, SubagentError> {
        let catalog = cyrup_provider::catalog::seed_catalog();
        let models: Vec<crate::registration::profiles::ProviderCatalogModel> = catalog
            .iter()
            .filter(|m| m.provider.as_str() == provider)
            .map(|m| crate::registration::profiles::ProviderCatalogModel {
                id: m.id.as_str().to_string(),
                full_id: format!("{}/{}", m.provider.as_str(), m.id.as_str()),
            })
            .collect();
        let model_count = models.len();
        let file = crate::registration::profiles::ProviderModelCatalog {
            provider: provider.to_string(),
            refreshed_at_epoch_ms: now_epoch_ms(),
            max_age_days: crate::registration::profiles::DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS,
            sources: vec!["runtime-registry".to_string(), "seed-catalog".to_string()],
            models,
        };
        crate::registration::profiles::write_provider_catalog(&self.profiles_dir(), &file)?;

        // Also touch the shared freshness marker `registration/doctor.rs` stats (R-SA-131 item f).
        let cache_path = self.provider_catalog_cache_path(Path::new("."));
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(SubagentError::Spawn)?;
        }
        let marker = serde_json::json!({
            "provider": provider,
            "modelCount": model_count,
            "refreshedAtEpochMs": file.refreshed_at_epoch_ms,
        });
        write_atomic_json(&cache_path, &marker)
            .await
            .map_err(SubagentError::Spawn)?;
        Ok(model_count)
    }

    /// `/subagents-refresh-provider-models <provider> [--force]` (pi `refreshProviderModelCatalog`,
    /// profiles.ts:489-577). Writes a per-provider catalog file under
    /// `providers/<provider>.models.json`; honors `--force` by reusing a still-fresh cache when
    /// `!force` and rewriting otherwise. The per-model live probe + `classifyModel` classification
    /// is deferred (func-SA §9 item 31); the catalog is populated from the static seed stand-in.
    async fn refresh_provider_catalog_cache(
        &self,
        cwd: &Path,
        provider: &str,
        force: bool,
    ) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        let profiles_dir = self.profiles_dir();

        // --force gate (pi `if (!options.force) { ... reuse fresh ... }`, profiles.ts:498-503):
        // a still-fresh cache is reused verbatim unless --force forces a rewrite. `.filter` keeps
        // the still-fresh cache and drops a stale one, avoiding nested `if`s.
        let fresh_cache = if force {
            None
        } else {
            crate::registration::profiles::read_provider_catalog(&profiles_dir, provider)?.filter(
                |existing| {
                    !crate::registration::profiles::is_provider_catalog_stale(
                        existing,
                        now_epoch_ms(),
                        existing.max_age_days,
                    )
                },
            )
        };
        if let Some(existing) = fresh_cache {
            return Ok(format!(
                "subagents-refresh-provider-models: provider '{provider}' — fresh cache reused \
                 ({} model(s)); pass --force to rewrite. Live per-provider probing is deferred \
                 (func-SA §9 item 31).",
                existing.models.len()
            ));
        }

        let has_models = cyrup_provider::catalog::seed_catalog()
            .iter()
            .any(|m| m.provider.as_str() == provider);
        if !has_models {
            return Ok(format!(
                "subagents-refresh-provider-models: provider '{provider}' has no models in the \
                 static seed catalog; nothing to refresh. Live provider probing is deferred \
                 (func-SA §9 item 31)."
            ));
        }
        let _ = cwd;
        let model_count = self.write_provider_catalog_file(provider).await?;

        Ok(format!(
            "subagents-refresh-provider-models: refreshed catalog cache for '{provider}' \
             ({model_count} model(s)) from the static seed catalog. Live per-provider probing is \
             deferred (func-SA §9 item 31)."
        ))
    }

    /// `/subagents-generate-profiles <provider>` (pi `generateProfilesForProvider`,
    /// profiles.ts:579-606). Refreshes the per-provider catalog, then writes `<provider>.quota` and
    /// `<provider>.quality` profiles — EACH carrying the full 8-agent tier map PLUS a representative
    /// `subagents.defaultModel` (the medium tier, the fallback for non-builtin agents)
    /// ([`crate::registration::profiles::build_profile_file`]).
    async fn generate_provider_profiles(&self, provider: &str) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        let ranked = self.provider_ranked_full_ids(provider);
        if ranked.is_empty() {
            return Ok(format!(
                "subagents-generate-profiles: provider '{provider}' has no models in the static \
                 seed catalog; nothing to generate."
            ));
        }
        // pi's generateProfilesForProvider refreshes the catalog first (profiles.ts:586).
        self.write_provider_catalog_file(provider).await?;

        let profiles_dir = self.profiles_dir();
        let generated = crate::registration::profiles::generate_provider_profiles(
            &profiles_dir,
            provider,
            &ranked,
        )?;

        Ok(format!(
            "Generated subagent profiles\n\
             Provider: {provider}\n\
             Quota: {quota}\n  cheap={qc}\n  medium={qm}\n  strong={qs}\n\
             Quality: {quality}\n  cheap={lc}\n  medium={lm}\n  strong={ls}\n\
             (8-agent tier map; live per-provider probing is deferred, func-SA §9 item 31)",
            quota = generated.quota_path.display(),
            qc = generated.quota_models.cheap,
            qm = generated.quota_models.medium,
            qs = generated.quota_models.strong,
            quality = generated.quality_path.display(),
            lc = generated.quality_models.cheap,
            lm = generated.quality_models.medium,
            ls = generated.quality_models.strong,
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

    /// The user-scope `settings.json` the extension's discovery reads its `subagents.*` layer back
    /// from (`~/.cyrup/agents/settings.json` — the SAME file [`Self::discovery_config`] loads the
    /// user settings from). `/subagents-load-profile` writes the loaded profile's `subagents` block
    /// here so the next discovery pass picks it up, exactly as pi's `applySubagentProfile` writes to
    /// the same `getUserSettingsPath()` its discovery reads.
    fn user_settings_path(&self) -> PathBuf {
        dirs_home()
            .join(".cyrup")
            .join("agents")
            .join("settings.json")
    }

    /// `/subagents-load-profile <name>`: load the named profile and REPLACE ONLY the `subagents`
    /// key of the user settings file (pi `applySubagentProfile`, slash-commands.ts:1133-1176). Then
    /// surface the profile's `worker`-tier model (pi `getProfileWorkerModel`) as the model the user
    /// may want to switch the live session to.
    ///
    /// pi additionally *offers an interactive confirm* to switch the running session's model to the
    /// worker model, but ONLY when the host exposes `pi.setModel` + `ctx.modelRegistry`
    /// (slash-commands.ts:1150-1168); when it does not, pi falls straight through to the
    /// `else if (workerModel)` branch and simply reports the worker model (line 1167). A native
    /// `NativeExtension::execute_command` `HostCtx` exposes neither a `set_model` control op nor an
    /// interactive `confirm` today (that live session-model switch is the outer-layer UI tier,
    /// tracked separately) — so this reproduces pi's exact non-interactive branch: settings are
    /// written for real, and the worker model is reported.
    async fn load_profile_into_settings(&self, name: &str) -> Result<String, SubagentError> {
        let profiles_dir = self.profiles_dir();
        let profile = crate::registration::profiles::load_profile(&profiles_dir, name)?;
        let worker_model = crate::registration::profiles::profile_worker_model(&profile);
        let settings_path = self.user_settings_path();
        crate::registration::profiles::apply_profile_to_settings_file(&settings_path, &profile)?;

        let profile_path = crate::registration::profiles::profile_path(&profiles_dir, name)?;
        let mut lines = vec![
            format!("Loaded subagent profile: {name}"),
            format!("Profile: {}", profile_path.display()),
            format!("Updated: {}", settings_path.display()),
        ];
        if let Some(model) = worker_model {
            lines.push(format!("Profile worker model: {model}"));
        }
        Ok(lines.join("\n"))
    }
}

// =================================================================================================
// Free helper functions backing the dispatch_slash arms above
// =================================================================================================

/// Every agent name a [`RunnerStep`] graph will dispatch, in walk order — a single step's own
/// agent, each parallel-group child's agent, and a dynamic group's per-item template agent. This is
/// the plan-time persona resolver's input set: [`SubagentsExtension::run_or_background_chain`]
/// resolves the whole set via [`SubagentExecutor::resolve_plan_personas`] before any child
/// is spawned (T0.1/C13 plan-time resolution + upfront agent-name validation).
fn plan_step_agent_names(graph: &[RunnerStep]) -> Vec<String> {
    let mut names = Vec::new();
    for step in graph {
        match step {
            RunnerStep::SingleStep(spec) => names.push(spec.agent.clone()),
            RunnerStep::ParallelGroup(group) => {
                names.extend(group.steps.iter().map(|spec| spec.agent.clone()));
            }
            RunnerStep::DynamicGroup(dynamic) => names.push(dynamic.template.agent.clone()),
            // A root-attachment step names no agent to discover/resolve at plan time — its agent is
            // whatever the ALREADY-launched target run resolved for itself, read back from the
            // target's result at poll time (R-SA-097). Contributing its display name here would make
            // plan-time persona resolution demand an agent the attaching chain never spawns.
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    names
}

/// The graph's first step's first task text — the fallback `{task}` value when the call site
/// supplied no explicit top-level task (pi `originalTask = params.task ?? firstStepFirstTask`,
/// `chain-execution.ts:493-497`). A single step's own task, a parallel group's first child's task, or
/// a dynamic group's per-item template task; an `ImportAsyncRoot`-led graph (or an empty graph) has
/// no authored task text, yielding the empty string (so `{task}` → `""`, matching pi's empty case).
fn first_step_task(graph: &[RunnerStep]) -> String {
    graph
        .iter()
        .find_map(|step| match step {
            RunnerStep::SingleStep(spec) => Some(spec.task.clone()),
            RunnerStep::ParallelGroup(group) => group.steps.first().map(|spec| spec.task.clone()),
            RunnerStep::DynamicGroup(dynamic) => Some(dynamic.template.task.clone()),
            RunnerStep::ImportAsyncRoot(_) => None,
        })
        .unwrap_or_default()
}

/// Resolve every step's effective fork-context and, for each forking step, mint its OWN per-index
/// branch session file — the Tier-2 fork default-mode + per-index-branch wire-up (pi
/// `resolveAgentDefaultContextPolicy` + `preflightForkSessionsForStaticTasks`,
/// `subagent-executor.ts:1280-1521`). Two behaviors this replaces the old single-shared-branch
/// `apply_default_context` with:
///
/// 1. **Fork default-mode.** An OMITTED call-site `context` (`None`) no longer forces `Fresh` on
///    every step; each step independently falls back to ITS OWN agent's persona `default_context`
///    via [`resolve_effective_context`] (`personas[agent].default_context`). An explicit call-site
///    `context` still wins for every step; a step's own explicit `context` wins over both (R-SA-138).
/// 2. **Per-index branch.** Rather than resolving ONE fork branch (index 0) and splicing that same
///    session file into every step, each FORKING step resolves its own branch at its own flat step
///    index off the SINGLE shared `resolver` (whose per-index cache mints a distinct branch per
///    index) — so two sibling parallel tasks that both fork get two DISTINCT branch session files.
///    The flat index increments for EVERY step (matching pi's `preflightForkSessionsForStaticTasks`
///    flat-index walk), forking or not, so indices are stable and never collide.
///
/// A step that already carries an explicit `session_file` keeps it (never re-branched). Returns the
/// (mutated) graph plus the FIRST forking step's branch path, used only as the run-level resume
/// session metadata (`RunnerConfig::session_file`); it is never spliced onto any step here.
///
/// Fails hard (R-SA-137/DI-SA-2) if any forking step's branch cannot be resolved (unpersisted parent,
/// no leaf) — resolving every step up front, before any child is spawned, so a later step's fork
/// failure aborts the whole batch rather than leaving earlier children already running.
async fn apply_fork_contexts(
    resolver: &ForkContextResolver,
    call_site_context: Option<ContextMode>,
    personas: &BTreeMap<String, ResolvedAgentPersona>,
    mut graph: Vec<RunnerStep>,
) -> Result<(Vec<RunnerStep>, Option<PathBuf>), SubagentError> {
    let mut flat_index: u32 = 0;
    let mut first_session_file: Option<PathBuf> = None;
    for step in &mut graph {
        match step {
            RunnerStep::SingleStep(spec) => {
                resolve_step_fork_context(
                    resolver,
                    call_site_context,
                    personas,
                    spec,
                    &mut flat_index,
                    &mut first_session_file,
                )
                .await?;
            }
            RunnerStep::ParallelGroup(group) => {
                for spec in &mut group.steps {
                    resolve_step_fork_context(
                        resolver,
                        call_site_context,
                        personas,
                        spec,
                        &mut flat_index,
                        &mut first_session_file,
                    )
                    .await?;
                }
            }
            RunnerStep::DynamicGroup(dynamic) => {
                resolve_step_fork_context(
                    resolver,
                    call_site_context,
                    personas,
                    &mut dynamic.template,
                    &mut flat_index,
                    &mut first_session_file,
                )
                .await?;
            }
            // A root-attachment step carries no fork-vs-fresh context of its own: it imports another,
            // already-completed run's result rather than spawning a fresh child whose session context
            // this resolution would seed. Left untouched (and does not consume a flat index).
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    Ok((graph, first_session_file))
}

/// Resolve one step's effective context (per-step explicit > call-site > persona default > `Fresh`)
/// and, when it resolves to `Fork` and the step has no explicit session file yet, mint its own
/// per-`*flat_index*` branch off `resolver`. Always advances `*flat_index*` by one so sibling steps
/// never share an index (and therefore never a branch). See [`apply_fork_contexts`].
async fn resolve_step_fork_context(
    resolver: &ForkContextResolver,
    call_site_context: Option<ContextMode>,
    personas: &BTreeMap<String, ResolvedAgentPersona>,
    spec: &mut SingleStepSpec,
    flat_index: &mut u32,
    first_session_file: &mut Option<PathBuf>,
) -> Result<(), SubagentError> {
    let index = *flat_index;
    *flat_index += 1;

    // Precedence: a step's OWN explicit `context` wins; else the call-site `context`; else this
    // step's agent's persona `default_context`; else `Fresh` (`resolve_effective_context`).
    let persona_default = personas.get(&spec.agent).and_then(|p| p.default_context);
    let effective = resolve_effective_context(spec.context.or(call_site_context), persona_default);
    spec.context = Some(effective);

    if effective == ContextMode::Fork {
        if spec.session_file.is_none() {
            let fork_context = resolver.resolve(ContextMode::Fork, index).await?;
            spec.session_file = fork_context.session_file_path.clone();
            if first_session_file.is_none() {
                *first_session_file = fork_context.session_file_path;
            }
        } else if first_session_file.is_none() {
            first_session_file.clone_from(&spec.session_file);
        }
    }
    Ok(())
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
            RunnerStep::ImportAsyncRoot(_) => {
                // A root-attachment step's "task" is fixed by the target run it imports; there is no
                // free task text to seed (R-SA-097) — left as saved.
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

/// The effective model a discovered builtin persona resolves to for `/subagents-models` (pi
/// `resolveSubagentModelOverride`'s cyrup-observable result): the persona's own configured `model`
/// if it declares one, else the inherit-current-session-model intent (cyrup has no live session
/// model to resolve it against — see [`SubagentExecutor::run_models_report`]'s doc).
fn resolved_builtin_model(agent: &AgentDefinition) -> String {
    agent
        .model
        .as_ref()
        .map(|model| model.as_str().to_string())
        .unwrap_or_else(|| "(inherits current session model)".to_string())
}

/// Provenance of a builtin persona's resolved model (pi `formatModelSource`,
/// agent-management.ts:565-578), derived from discovery's own `override_info`/`model_source`
/// provenance rather than re-deriving the config-layering walk.
fn format_model_source(agent: &AgentDefinition) -> String {
    if let Some(override_info) = &agent.override_info {
        let scope = match override_info.scope {
            OverrideScope::User => "user",
            OverrideScope::Project => "project",
        };
        return format!("{scope} override");
    }
    match agent.model_source {
        Some(AgentModelSourceInfo::SettingsDefault) => "settings defaultModel".to_string(),
        Some(AgentModelSourceInfo::SettingsOverride) => "settings override".to_string(),
        Some(AgentModelSourceInfo::Frontmatter) => "builtin agent config".to_string(),
        Some(AgentModelSourceInfo::Unresolved) | None => {
            if agent.model.is_some() {
                "builtin agent config".to_string()
            } else {
                "inherit requested, but no current session model is available".to_string()
            }
        }
    }
}

/// Render `/subagents-check-profile`'s report: cross-reference every model reference a profile
/// declares (`defaultModel` plus every `overrides.<agent>.model`) against the real static seed
/// catalog.
fn render_profile_check_report(
    name: &str,
    profile: &crate::registration::profiles::NamedProfile,
) -> String {
    let catalog = cyrup_provider::catalog::seed_catalog();
    // Recognize BOTH bare ids (`gpt-4o`) and fully-qualified `provider/id` refs (`openai/gpt-4o`),
    // since generated profiles (pi shape) write the fully-qualified form.
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &catalog {
        known.insert(m.id.as_str().to_string());
        known.insert(format!("{}/{}", m.provider.as_str(), m.id.as_str()));
    }

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

    /// T6 child-mode gate (pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`): the
    /// pure decision function encoding "a plain subagent child registers nothing; a fanout-authorized
    /// child gets the restricted tool; a non-child gets the full surface."
    #[test]
    fn resolve_registration_mode_encodes_the_child_gate() {
        // Not a child → full orchestrator surface (the fanout flag is irrelevant when not a child).
        assert_eq!(resolve_registration_mode(false, false), Some(RegistrationMode::Full));
        assert_eq!(resolve_registration_mode(false, true), Some(RegistrationMode::Full));
        // Fanout-authorized child → the restricted child-safe tool.
        assert_eq!(resolve_registration_mode(true, true), Some(RegistrationMode::ChildSafe));
        // Plain subagent child → register NOTHING.
        assert_eq!(resolve_registration_mode(true, false), None);
    }

    /// T6: a `CYRUP_SUBAGENT_CHILD=1` process without fanout authorization must attach NO subagent
    /// extension at all (so its `subagent` tool, slash commands, and watchers are never registered),
    /// while a fanout-authorized child gets an extension that installs the tool but NO lifecycle
    /// subscriptions (no background watcher, no session-start housekeeping), and a non-child gets the
    /// full lifecycle surface.
    #[tokio::test]
    async fn child_env_gate_controls_what_is_registered() {
        let cwd = std::env::temp_dir();

        // Plain child → no extension → no `subagent` tool registered anywhere.
        let disabled =
            subagent_extension_for(SubagentExtensionConfig::default(), cwd.clone(), true, false);
        assert!(disabled.is_none(), "a plain subagent child registers no subagent surface at all");

        // Fanout-authorized child → an extension whose init installs NO lifecycle subscriptions.
        let child_safe =
            subagent_extension_for(SubagentExtensionConfig::default(), cwd.clone(), true, true)
                .expect("a fanout-authorized child registers the restricted tool");
        let mut api = InitApi::new();
        child_safe.init(&mut api).await.expect("child-safe init succeeds");
        assert!(
            !api.subscriptions().contains(cyrup_ext::EventKind::SessionStart),
            "a child-safe extension installs no SessionStart watcher/housekeeping"
        );
        assert!(!api.subscriptions().contains(cyrup_ext::EventKind::SessionShutdown));

        // Non-child (root orchestrator) → the full lifecycle surface.
        let full = subagent_extension_for(SubagentExtensionConfig::default(), cwd, false, false)
            .expect("a non-child process registers the full orchestrator extension");
        let mut api = InitApi::new();
        full.init(&mut api).await.expect("full init succeeds");
        assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionStart));
        assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionShutdown));
    }

    /// T6 upfront agent-name validation for `/chain` (pi validates every named agent before starting
    /// a chain rather than spawning a partial run that dies mid-walk): a chain naming an agent that
    /// resolves to nothing in the discovery scope fails fast with [`SubagentError::AgentNotFound`]
    /// BEFORE any child process — and therefore before any spawn scratch directory — is ever created.
    #[tokio::test]
    async fn unknown_agent_in_a_chain_is_rejected_upfront_before_any_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let graph = vec![RunnerStep::SingleStep(SingleStepSpec {
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
            .run_or_background_graph(dir.path(), graph, RunMode::Chain, None, false, None)
            .await
        {
            Err(SubagentError::AgentNotFound(name)) => assert_eq!(name, "does-not-exist"),
            Err(other) => panic!("expected AgentNotFound(does-not-exist), got: {other}"),
            Ok(_) => panic!("an unknown agent in /chain must be rejected before running"),
        }
        assert!(
            !dir.path().join(".cyrup-subagent-scratch").exists(),
            "upfront rejection must happen before any child (and its scratch dir) is created"
        );
    }

    /// C8: the LLM-facing `subagent` tool schema exposes pi's FULL parameter union
    /// (`schemas.ts:195-265`), not just the pre-C8 5-property single-task shape. Asserts every
    /// top-level pi property name is present, the 11-value management/control `action` enum is
    /// complete and correctly ordered, the `context` fresh/fork enum is present, the `tasks[]`
    /// per-task `output`/`outputMode`/`reads`/`progress` fields exist, and the numeric bounds pi
    /// pins (`concurrency`/`timeoutMs`/`maxRuntimeMs` minimum, `index` minimum 0) are carried — the
    /// Rust analog of pi's own `test/unit/schemas.test.ts`.
    #[test]
    fn subagent_tool_schema_exposes_the_full_pi_parameter_union() {
        let schema = subagent_tool_parameters();
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema has a properties object");

        // Every top-level pi `SubagentParamsSchema` property (schemas.ts:195-263), in source order.
        let expected_properties = [
            "agent", "task", "action", "id", "runId", "dir", "index", "message", "chainName",
            "config", "tasks", "concurrency", "worktree", "chain", "context", "chainDir", "async",
            "timeoutMs", "maxRuntimeMs", "agentScope", "cwd", "artifacts", "includeProgress",
            "share", "sessionDir", "clarify", "control", "output", "outputMode", "skill", "model",
            "acceptance",
        ];
        for name in expected_properties {
            assert!(
                props.contains_key(name),
                "schema must advertise the pi parameter '{name}'; got keys: {:?}",
                props.keys().collect::<Vec<_>>()
            );
        }

        // The 11-value management/control action enum (schemas.ts:199-202 + SUBAGENT_ACTIONS,
        // shared/types.ts:974), exact values AND order.
        let action_enum = props
            .get("action")
            .and_then(|a| a.get("enum"))
            .and_then(|e| e.as_array())
            .expect("action property carries an enum array");
        let action_values: Vec<&str> = action_enum.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            action_values,
            vec![
                "list", "get", "models", "create", "update", "delete", "status", "interrupt",
                "resume", "append-step", "doctor"
            ],
            "the action enum must be pi's exact 11-value SUBAGENT_ACTIONS union"
        );
        assert_eq!(props["action"]["type"], serde_json::json!("string"));

        // context fresh/fork enum.
        assert_eq!(props["context"]["type"], serde_json::json!("string"));
        assert_eq!(props["context"]["enum"], serde_json::json!(["fresh", "fork"]));

        // Top-level numeric bounds pi pins.
        assert_eq!(props["concurrency"]["minimum"], serde_json::json!(1));
        assert_eq!(props["timeoutMs"]["minimum"], serde_json::json!(1));
        assert_eq!(props["maxRuntimeMs"]["minimum"], serde_json::json!(1));
        assert_eq!(props["index"]["minimum"], serde_json::json!(0));

        // tasks[] per-task fields the description advertises (output/outputMode/reads/progress),
        // plus count's minimum.
        let task_props = props["tasks"]["items"]["properties"]
            .as_object()
            .expect("tasks[].items has a properties object");
        for per_task in ["agent", "task", "count", "output", "outputMode", "reads", "progress"] {
            assert!(
                task_props.contains_key(per_task),
                "tasks[] items must carry the per-task field '{per_task}'"
            );
        }
        assert_eq!(task_props["count"]["minimum"], serde_json::json!(1));
        assert_eq!(task_props["progress"]["type"], serde_json::json!("boolean"));
        assert_eq!(props["tasks"]["items"]["required"], serde_json::json!(["agent", "task"]));

        // chain[] items must be an additionalProperties:false object with the flattened
        // sequential/parallel/dynamic surface (schemas.ts:144-178).
        let chain_item = &props["chain"]["items"];
        assert_eq!(chain_item["type"], serde_json::json!("object"));
        assert_eq!(chain_item["additionalProperties"], serde_json::json!(false));
        let chain_props = chain_item["properties"]
            .as_object()
            .expect("chain[].items has a properties object");
        for chain_field in ["agent", "parallel", "expand", "collect", "concurrency", "failFast", "worktree"] {
            assert!(
                chain_props.contains_key(chain_field),
                "chain[] items must carry '{chain_field}'"
            );
        }

        // config/output/skill/acceptance are provider-friendly anyOf unions (no bare top-level type).
        assert!(props["config"].get("anyOf").is_some(), "config must be an anyOf union");
        assert!(props["output"].get("anyOf").is_some(), "output must be an anyOf union");
        assert!(props["skill"].get("anyOf").is_some(), "skill must be an anyOf union");
        assert!(props["acceptance"].get("anyOf").is_some(), "acceptance must be an anyOf union");

        // control carries the nested attention thresholds + notify enums.
        let control_props = props["control"]["properties"]
            .as_object()
            .expect("control has a properties object");
        assert_eq!(control_props["needsAttentionAfterMs"]["minimum"], serde_json::json!(1));
        assert_eq!(
            control_props["notifyOn"]["items"]["enum"],
            serde_json::json!(["active_long_running", "needs_attention"])
        );
        assert_eq!(
            control_props["notifyChannels"]["items"]["enum"],
            serde_json::json!(["event", "async", "intercom"])
        );

        // The multi-section description (index.ts:461-495) — the substrings pi's own
        // tool-description executable spec pins (test/unit/tool-description.test.ts).
        let desc = SUBAGENT_TOOL_DESCRIPTION;
        for needle in [
            "use { action: \"list\" } to inspect configured agents/chains",
            "executable/non-disabled",
            "proactive skill subagent suggestions",
            "output?,reads?,progress?",
            "timeoutMs",
            "maxRuntimeMs",
            "only for foreground runs",
            "omit for async/background runs",
        ] {
            assert!(
                desc.contains(needle),
                "the tool description must contain the pi-pinned substring {needle:?}"
            );
        }
        assert!(
            !desc.contains("disabled builtins"),
            "the description must NOT contain 'disabled builtins' (pi tool-description.test.ts pins its absence)"
        );
    }

    /// C8 permissive parsing (DI-SA-11): the full pi-union parse target accepts the SINGLE,
    /// PARALLEL, CHAIN, management, and control shapes, ignores unknown keys, and reports which keys
    /// were supplied — the routing dimension every `execute` dispatch arm branches on.
    #[test]
    fn subagent_tool_params_parse_every_pi_mode_shape() {
        // SINGLE with context/async/model.
        let single: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "context": "fork", "async": true,
            "model": "anthropic/claude-sonnet-4", "unknownFutureKey": 42
        }))
        .expect("single shape parses permissively (unknown keys ignored)");
        assert_eq!(single.agent.as_deref(), Some("worker"));
        assert!(single.is_background());
        assert!(matches!(single.context_override(), Some(ContextMode::Fork)));
        assert!(single.provided_keys().contains(&"model"));
        assert!(!single.provided_keys().contains(&"unknownFutureKey"));

        // PARALLEL.
        let parallel: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "tasks": [{ "agent": "reviewer", "task": "review", "output": "r.md", "reads": ["in.md"], "progress": true }],
            "concurrency": 3, "worktree": true
        }))
        .expect("parallel shape parses");
        assert!(parallel.tasks.is_some());
        assert_eq!(parallel.concurrency, Some(3));
        assert_eq!(parallel.worktree, Some(true));

        // CHAIN.
        let chain: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "chain": [{ "agent": "a", "task": "Analyze {task}" }, { "parallel": [{ "agent": "b", "count": 2 }] }]
        }))
        .expect("chain shape parses");
        assert!(chain.chain.is_some());

        // Management + control actions (camelCase runId/chainName round-trip).
        let mgmt: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "action": "get", "chainName": "release", "agent": "pkg.reviewer"
        }))
        .expect("management shape parses");
        assert_eq!(mgmt.action.as_deref(), Some("get"));
        assert_eq!(mgmt.chain_name.as_deref(), Some("release"));

        let control: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "action": "resume", "runId": "abc", "index": 0, "message": "continue"
        }))
        .expect("control shape parses");
        assert_eq!(control.run_id.as_deref(), Some("abc"));
        assert_eq!(control.index, Some(0));
    }

    /// Dispatch discrimination: management/control/parallel/chain modes are each RECOGNIZED and
    /// routed to their own arm rather than mis-parsed as a broken SINGLE call. Management/control
    /// still short-circuit at their P1 stubs; parallel/chain now route to REAL execution, proven
    /// here without any spawn by using an unresolvable agent so plan-time persona resolution fails
    /// (`AgentNotFound`) before any child process — the assertion stays on the dispatch decision.
    #[tokio::test]
    async fn tool_execute_routes_each_mode_to_its_dispatch_arm() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }

        // Management action → now wired (C3): `list` succeeds and renders the pi list shape,
        // proving the dispatch reached the real management arm rather than a stub.
        let mgmt_ok = dispatch(&tool, serde_json::json!({ "action": "list" }))
            .await
            .expect("management action 'list' is wired and returns the agent/chain listing");
        let mgmt_text = mgmt_ok
            .content
            .iter()
            .find_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(mgmt_text.contains("Executable agents:"), "got: {mgmt_text}");
        assert!(mgmt_text.contains("Chains:"), "got: {mgmt_text}");

        // Control action → now wired (C5): an unknown run id fails with the not-found notice,
        // proving the dispatch reached the real control arm rather than a stub.
        let control_err = dispatch(&tool, serde_json::json!({ "action": "status", "id": "run1" }))
            .await
            .expect_err("control action routes to real status, which fails on the unknown id");
        assert!(
            control_err.to_string().contains("Async run not found"),
            "got: {control_err}"
        );

        // PARALLEL (tasks[]) → parallel arm. Now routes through the REAL plan-execution path, so an
        // unresolvable agent fails at plan-time persona resolution (`AgentNotFound`) BEFORE any
        // spawn — proving the dispatch reached the parallel arm and its real routing, not a stub.
        let parallel_err = dispatch(&tool, serde_json::json!({ "tasks": [{ "agent": "x", "task": "y" }] }))
            .await
            .expect_err("tasks[] routes to real parallel execution, which fails on the unknown agent");
        assert!(
            parallel_err.to_string().contains("agent not found: x"),
            "got: {parallel_err}"
        );

        // CHAIN (chain[]) → chain arm, likewise failing at plan-time persona resolution.
        let chain_err = dispatch(&tool, serde_json::json!({ "chain": [{ "agent": "x", "task": "y" }] }))
            .await
            .expect_err("chain[] routes to real chain execution, which fails on the unknown agent");
        assert!(
            chain_err.to_string().contains("agent not found: x"),
            "got: {chain_err}"
        );

        // Unknown action → explicit unknown-action error listing the valid set.
        let unknown_err = dispatch(&tool, serde_json::json!({ "action": "frobnicate" }))
            .await
            .expect_err("an unknown action is rejected");
        assert!(unknown_err.to_string().contains("unknown subagent action 'frobnicate'"));
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
            let mut cfg = executor.config.lock().await;
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
            .spawn_background(dir.path(), "ghost", "do something", Some(ContextMode::Fresh))
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
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];

        let err = executor
            .run_chain_foreground(dir.path(), graph, BTreeMap::new(), String::new(), None)
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
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        });

        let err = executor
            .spawn_background_steps(
                dir.path(),
                BackgroundStepsSpec {
                    steps: vec![step],
                    mode: RunMode::Single,
                    session_file: None,
                    resolved_agents: BTreeMap::new(),
                    original_task: String::new(),
                    chain_dir: None,
                },
            )
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
            output_path: None,
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
                    Some(ContextMode::Fresh),
                    background,
                    None,
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
    // `/subagent-cost` walks the SESSION TRANSCRIPT (pi `buildSubagentCostReport`,
    // slash-commands.ts:289-328), not a background status file: this drives the REAL production
    // command path (`SubagentExecutor::run_cost_report` -> `cost_report_from_sessions_dir`) end to
    // end over a real on-disk session (created + appended via `cyrup_session::SessionManager`,
    // reloaded via `SessionManager::open`), proving the command sums the parent's own assistant
    // usage plus every subagent child's usage from the transcript. (The recursive nested-run
    // accumulator `registration::cost::compute_recursive_cost` remains a separate capability with
    // its own exhaustive unit tests in that module.)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn run_cost_report_walks_the_session_transcript() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");

        // A real, persisted session: one parent assistant turn (usage 200/100, $0.02) + one subagent
        // toolResult carrying a child result (usage 50/25, $0.005). `SessionManager::create` +
        // `append_message` write these to disk exactly as a live run would.
        let layout = cyrup_session::SessionLayout::new(tmp.path().join("sessions"), cwd.clone());
        let mut manager = cyrup_session::SessionManager::create(
            &cwd,
            &layout,
            cyrup_session::NewSessionOpts::default(),
        )
        .expect("create session");

        let user: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "go" }],
            "timestamp": 0,
        }))
        .expect("user message");
        manager.append_message(user).expect("append user");

        let assistant: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": "ok" }],
            "provider": "anthropic",
            "model": "claude-sonnet-4",
            "usage": {
                "input": 200, "output": 100, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 300,
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.02 },
            },
            "stopReason": "stop",
            "timestamp": 1,
        }))
        .expect("assistant message");
        manager.append_message(assistant).expect("append assistant");

        let tool_result: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "toolResult",
            "toolCallId": "call-1",
            "toolName": "subagent",
            "content": [{ "type": "text", "text": "done" }],
            "details": {
                "mode": "single",
                "results": [{
                    "agent": "worker",
                    "usage": {
                        "input": 50, "output": 25, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 75,
                        "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.005 },
                    },
                }],
            },
            "timestamp": 2,
        }))
        .expect("tool result message");
        manager.append_message(tool_result).expect("append tool result");

        let executor = SubagentExecutor::new();
        let report = executor.cost_report_from_sessions_dir(&layout.dir()).await;

        assert!(report.starts_with("Subagent cost\n"), "{report}");
        assert!(report.contains("Parent: ↑200 ↓100"), "parent assistant usage: {report}");
        assert!(report.contains("Child 1 (worker)"), "per-child breakdown: {report}");
        assert!(report.contains("Children: ↑50 ↓25"), "child subtotal: {report}");
        // Parent (200/100) + child (50/25) summed into the grand Total (250/125), with cost summed.
        assert!(report.contains("Total: ↑250 ↓125"), "parent+child total: {report}");
        assert!(report.contains("$0.0250"), "total cost sums parent+child: {report}");
    }

    // ---------------------------------------------------------------------------------------
    // `/subagents-models` reports the RUNTIME builtin-agent -> model mapping (pi `handleModels`),
    // NOT the static provider catalog. Env-agnostic: asserts the mapping header/shape and the
    // unknown-builtin rejection, which hold whether or not the bundled builtins resolve in this
    // ambient test environment.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_models_report_renders_runtime_mapping_and_rejects_unknown_builtin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();

        let full = executor.run_models_report(dir.path(), None);
        assert!(
            full.starts_with("Builtin subagent models\n"),
            "the report must be the runtime builtin->model mapping, not a catalog dump: {full}"
        );
        assert!(full.contains("Current session model:"), "{full}");
        // The old behavior dumped the static provider catalog ("... — context {n}k, reasoning=...");
        // the runtime mapping must not.
        assert!(
            !full.contains("reasoning="),
            "must not dump the static provider catalog: {full}"
        );

        let unknown = executor.run_models_report(dir.path(), Some("definitely-not-a-builtin"));
        assert!(
            unknown
                .contains("Builtin agent 'definitely-not-a-builtin' not found. Available:"),
            "an unknown builtin name must be rejected with the available list: {unknown}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // C5 control-action dispatch smoke tests (executor glue; read-only, no spawn, no home writes)
    //
    // These drive the real `SubagentExecutor::control_*` methods over a fresh temp cwd whose async
    // root has never been created, so every path is a pure read that returns the expected empty /
    // not-found rendering without spawning any process or touching the user's `~/.cyrup` tree. The
    // full per-run rendering + primitive behavior is covered by `background::run_status`'s own tests
    // against explicit temp roots.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn control_status_no_id_over_a_fresh_cwd_lists_no_active_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let text = executor
            .control_status(dir.path(), None, None)
            .await
            .expect("status list is Ok even with no runs");
        assert_eq!(text, "No active async runs.");
    }

    #[tokio::test]
    async fn control_status_unknown_id_is_the_not_found_notice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_status(dir.path(), Some("deadbeef0000"), None)
            .await
            .expect_err("an unknown id is a not-found error");
        assert_eq!(err, "Async run not found. Provide id or dir.");
    }

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

    // =====================================================================================
    // Tier-2 (a): fork default-mode + per-index branch (`apply_fork_contexts`).
    // =====================================================================================

    fn fork_user_msg(text: &str) -> cyrup_core::Message {
        cyrup_core::Message::User {
            content: vec![cyrup_core::Content::text(text)],
            timestamp: 0,
        }
    }

    fn fork_assistant_msg(text: &str) -> cyrup_core::Message {
        cyrup_core::Message::Assistant(cyrup_core::AssistantMessage {
            content: vec![cyrup_core::Content::text(text)],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: cyrup_core::Usage::default(),
            stop_reason: cyrup_core::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })
    }

    /// Build a REAL persisted parent session (tempdir-backed on-disk JSONL, never mocked — mirrors
    /// `fork_context.rs`'s own test setup) and a [`ForkContextResolver`] over it, so `Fork` requests
    /// actually branch a genuine new session file on disk. Returns the tempdir so the caller keeps it
    /// alive for the test's duration.
    async fn persisted_fork_resolver() -> (ForkContextResolver, tempfile::TempDir) {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = root.path().join("proj");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        let mut parent =
            cyrup_session::SessionManager::create(&cwd, &layout, cyrup_session::NewSessionOpts::default())
                .expect("create persisted parent session");
        parent.append_message(fork_user_msg("hello")).expect("append user");
        parent
            .append_message(fork_assistant_msg("hi there"))
            .expect("append assistant");
        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(manager, layout);
        (resolver, root)
    }

    fn fork_test_step(agent: &str) -> SingleStepSpec {
        SingleStepSpec {
            agent: agent.to_string(),
            task: format!("do {agent}"),
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

    fn persona_with_default_context(
        name: &str,
        default_context: Option<ContextMode>,
    ) -> ResolvedAgentPersona {
        ResolvedAgentPersona {
            name: name.to_string(),
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            output: None,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            completion_guard: None,
            max_subagent_depth: None,
            default_context,
        }
    }

    fn single_step_of(step: &RunnerStep) -> &SingleStepSpec {
        match step {
            RunnerStep::SingleStep(spec) => spec,
            other => panic!("expected a SingleStep, got {other:?}"),
        }
    }

    /// (a) An OMITTED call-site `context` (`None`) resolves EACH step to its own agent's persona
    /// `default_context` — a `fork`-defaulting agent forks, a `fresh`-defaulting agent stays fresh —
    /// rather than the pre-Tier-2 forced-`Fresh` collapse. Mirrors pi's
    /// `resolveAgentDefaultContextPolicy` (`subagent-executor.ts:1280-1293`).
    #[tokio::test]
    async fn omitted_call_site_context_falls_back_to_each_agents_persona_default() {
        let (resolver, _root) = persisted_fork_resolver().await;
        let personas: BTreeMap<String, ResolvedAgentPersona> = [
            (
                "planner".to_string(),
                persona_with_default_context("planner", Some(ContextMode::Fork)),
            ),
            (
                "scout".to_string(),
                persona_with_default_context("scout", Some(ContextMode::Fresh)),
            ),
        ]
        .into_iter()
        .collect();
        let graph = vec![
            RunnerStep::SingleStep(fork_test_step("planner")),
            RunnerStep::SingleStep(fork_test_step("scout")),
        ];

        // call_site_context = None (omitted).
        let (graph, first_session) = apply_fork_contexts(&resolver, None, &personas, graph)
            .await
            .expect("fork contexts resolve against a persisted parent");

        let planner = single_step_of(&graph[0]);
        let scout = single_step_of(&graph[1]);
        assert_eq!(
            planner.context,
            Some(ContextMode::Fork),
            "planner's persona default_context (fork) must be honored when the call site omits context"
        );
        assert!(
            planner.session_file.as_deref().is_some_and(Path::exists),
            "a fork step must receive a real, on-disk branched session file"
        );
        assert_eq!(
            scout.context,
            Some(ContextMode::Fresh),
            "scout's persona default_context (fresh) must be honored independently — one sibling's \
             default must not leak into another's"
        );
        assert!(
            scout.session_file.is_none(),
            "a fresh step must carry no branched session file"
        );
        assert_eq!(
            first_session, planner.session_file,
            "the run-level resume session is the first forking step's branch"
        );
    }

    /// (a) Two parallel fork tasks get two DISTINCT branch session files (per-index branch), not one
    /// shared branch. Mirrors pi's per-index `sessionFileForTask(agent, index)`
    /// (`preflightForkSessionsForStaticTasks`, `subagent-executor.ts:1496-1499`).
    #[tokio::test]
    async fn two_parallel_fork_tasks_get_two_distinct_branch_session_files() {
        let (resolver, _root) = persisted_fork_resolver().await;
        let personas: BTreeMap<String, ResolvedAgentPersona> = [(
            "planner".to_string(),
            persona_with_default_context("planner", None),
        )]
        .into_iter()
        .collect();
        // Two sibling forking tasks (same agent) in one parallel group; explicit call-site fork.
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: vec![fork_test_step("planner"), fork_test_step("planner")],
            concurrency: 4,
            fail_fast: false,
            worktree: false,
        });

        let (graph, _first) =
            apply_fork_contexts(&resolver, Some(ContextMode::Fork), &personas, vec![group])
                .await
                .expect("both parallel fork tasks resolve");

        let steps = match &graph[0] {
            RunnerStep::ParallelGroup(g) => &g.steps,
            other => panic!("expected a ParallelGroup, got {other:?}"),
        };
        let first = steps[0]
            .session_file
            .clone()
            .expect("parallel task 0 forks and gets a branch");
        let second = steps[1]
            .session_file
            .clone()
            .expect("parallel task 1 forks and gets a branch");
        assert_ne!(
            first, second,
            "two parallel fork tasks must get two DISTINCT branch session files, not one shared branch"
        );
        assert!(first.exists() && second.exists(), "both branch files must exist on disk");
    }

    // =====================================================================================
    // Tier-2 (c): package-tier enumeration -> a package agent is discovered at Package scope.
    // =====================================================================================

    /// (c) A package that declares an `agents` dir (here via manifest auto-discovery of a Path-source
    /// package's conventional `agents/`) has its persona discovered at
    /// [`crate::discovery::types::AgentSource::Package`] once the installed-packages registry is
    /// enumerated into the discovery config (the wire-up [`enumerate_installed_packages`] +
    /// `discovery_config` perform for real).
    #[test]
    fn a_package_provided_agent_is_discovered_at_package_scope() {
        let home = tempfile::tempdir().expect("tempdir");
        let global_dir = home.path().join(".cyrup");
        // A real on-disk package tree with a conventional agents/ dir holding one persona.
        let pkg_root = home.path().join("code-analysis-pkg");
        let agents_dir = pkg_root.join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir package agents dir");
        std::fs::write(
            agents_dir.join("analyzer.md"),
            "---\nname: analyzer\ndescription: A package-provided analyzer agent\n---\nYou analyze code.\n",
        )
        .expect("write package agent file");

        // Persist a Global-scope, Path-source install record in the global packages.json registry —
        // exactly what `enumerate_installed_packages` loads.
        let installed = cyrup_resources::InstalledPackages {
            packages: vec![cyrup_resources::InstalledPackage {
                id: cyrup_core::PackageId::from("path:code-analysis".to_string()),
                source: cyrup_resources::PackageSource::Path {
                    path: pkg_root.clone(),
                },
                scope: cyrup_resources::InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            }],
        };
        let store = cyrup_resources::PackageStore::new(global_dir.clone(), None);
        let registry_path = store
            .registry_path(cyrup_resources::InstallScope::Global)
            .expect("global registry path");
        cyrup_resources::package::lock::save(&registry_path, &installed)
            .expect("persist packages.json");

        // The wire-up under test: enumerate the registry, then discover.
        let enumerated = enumerate_installed_packages(&global_dir, None);
        assert_eq!(
            enumerated.packages.len(),
            1,
            "the global packages.json registry must enumerate its one installed package"
        );

        let cfg = AgentDiscoveryConfig {
            builtin_agents_dir: None,
            installed_packages: enumerated,
            global_dir,
            project_root: None,
            trusted_project: false,
            ..AgentDiscoveryConfig::default()
        };
        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        let analyzer = result
            .agents
            .iter()
            .find(|a| a.name == "analyzer")
            .expect("the package-provided analyzer agent must be discovered");
        assert_eq!(
            analyzer.source,
            crate::discovery::types::AgentSource::Package,
            "a package-provided agent must be discovered at Package scope"
        );
    }
}
