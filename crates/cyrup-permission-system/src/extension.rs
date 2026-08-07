//! The [`PermissionSystemExtension`] `NativeExtension` facade + the `before_tool_call` gate
//! orchestration + the binary wiring entry point (port of pi `index.ts` hook wiring).
//!
//! WIRING (all in this Phase-0 build, no dead primitives):
//! - `init` subscribes `[ToolCall, SessionStart, SessionShutdown]` — so the ToolCall subscription is
//!   honored by the block/mutate dispatcher (`cyrup-ext/src/dispatch.rs`) that `ExtHooks::
//!   before_tool_call` (`hooks.rs:31-44`, R-08-010) drives.
//! - `on_event(ToolCall)` runs [`PermissionSystemExtension::decide`] → `HookOutcome::Block` (deny /
//!   fail-closed ask) or `Noop` (allow), which the dispatcher turns into `BeforeOutcome::Block` /
//!   proceed. THIS is the deciding gate: a real risky tool call (bash/write/edit) is genuinely
//!   intercepted and gated.
//! - `on_event(SessionStart|SessionShutdown)` clears the session store + dedup (pi
//!   `index.ts:2089,2123`).
//! - [`permission_extension_for_env`] is called at all three `crates/cyrup/src/main.rs` session-build
//!   sites (the `.with_native_extension(...)` seam), gated on child-mode (consistent with subagents)
//!   and DI-5 opt-in.
//!
//! LIVE HUMAN DIALOG (this build, P-1/P-3): an `ask` on a tool the interactive human drives now
//! surfaces the real permission dialog. [`PermissionSystemExtension::set_host_services`] captures the
//! late-bound `Arc<dyn HostServices>` (the SAME `LiveHostServices` the builder threads via
//! `load_native_with_services`); when `ctx.has_ui`, `resolve_ask` prompts through
//! [`LocalAskChannel`] (`HostServices::select`/`input`, pi `permission-dialog.ts`). The blocking
//! dialog is held under a [`HostCtx::begin_human_wait`] P-3 guard so the dispatcher's 5s invocation
//! budget is SUSPENDED for the human latency instead of firing and fail-OPENing the gate
//! (reconciliation §2 / port §4). "Allow Always" persists to the session approval store (pi
//! `index.ts:905`); headless / no-UI contexts still fail-CLOSE to `Block`.
//!
//! CHILD→PARENT ASK-FORWARDING (this build, P-4, `forwarding.rs`): a subagent CHILD loads the gate
//! with a [`ForwardingAskChannel`] ([`Self::new_forwarding_child`]) — an ask-tier decision writes a
//! nonce-bound request into the PARENT's spool (addressed by the `CYRUP_SUBAGENT_PARENT_SESSION`
//! anchor `cyrup-ext-subagents` emits, `exec/mod.rs` `PARENT_SESSION_ENV_VAR`) and BLOCKS on the bound
//! response under the P-3 `begin_human_wait` guard. The PARENT ([`Self::new_forwarding_parent`])
//! installs a spawned [`forwarding::spawn_forwarding_watcher`] task — on `SessionStart` AND, from
//! PERM-005, re-entrantly on `BeforeAgentStart` / `Input` / `ToolCall`, matching pi's four
//! `startForwardedPermissionPolling` call sites (`index.ts:2084,2137,2194,2210`) — that surfaces
//! each forwarded prompt to its human (the SAME `select`/`input` dialog + C3 human-interaction lock a
//! local ask uses) and writes the decision back; the child's `apply_decision` then persists an
//! "Allow Always" into the child's session store exactly like a local ask (pi `index.ts:905`).
//!
//! Two PERM-001 repairs sit on that path. (1) The parent role now PUBLISHES its own session id as
//! the process-wide anchor at `SessionStart` and clears it at `SessionShutdown`
//! (`PermissionSystemExtension::publish_parent_session_anchor`) — cyrup's placement of pi's
//! `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId` / `delete`
//! (`pi-subagents/src/extension/index.ts:599,619` @v0.34.0), which `cyrup-ext-subagents` cannot do itself
//! (`#![forbid(unsafe_code)]`). Without it the DETACHED background hop carried no anchor and every
//! background child's ask fail-closed denied against a null target. (2) The child-role predicate is
//! pi's `hasSubagentEnvHint` (`index.ts:93-103`) — ANY of [`SUBAGENT_ENV_HINT_KEYS`] non-empty —
//! not a strict `== "1"` on one key.
//!
//! FOUR SUPPLEMENTARY LAYERS (all wired + enforcing, pi `index.ts:2208-2499`): [`Self::decide`] runs
//! the agent + projectAgent policy layers (keyed by [`resolve_agent_name_from_env`], the
//! `CYRUP_SUBAGENT_AGENT_NAME` spawn anchor), the registry / unknown-tool block (against
//! [`cyrup_ext::HostServices::all_tool_names`]), the skill-read bypass (against the `before_agent_start`
//! `<available_skills>` entries, [`crate::skill`]), and the external-directory guard (against
//! [`HostCtx::cwd`]). The `before_agent_start` CONTEXT-HYGIENE layer is ALSO wired (pi
//! `index.ts:2134-2190`, port doc §9): [`Self::on_before_agent_start`] shapes the active tool set
//! ([`Self::should_expose_tool`] → [`cyrup_ext::HostServices::set_active_tools`]), RETURNS the
//! sanitized system prompt as a `[mutate]` ([`crate::sanitize`]), and surfaces the yolo status pill
//! ([`crate::status`]). Every layer — deciding gate AND context-hygiene — is live here, none stubbed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HostCtx, HostEvent, HookOutcome, HostServices, InitApi,
    NativeExtension, NotifyKind,
};

use crate::ask::{
    AskChannel, AskOutcome, ForwardingAskChannel, LocalAskChannel, NoOpAskChannel,
    PermissionDecisionState, PermissionPromptDecision, PromptOpts,
};
use crate::common::{self, to_record};
use crate::dedup::{DedupCache, DedupDetails};
use crate::ext_config::ExtensionConfig;
use crate::forwarding;
use crate::gate;
use crate::manager::{ManagerPaths, PermissionManager};
use crate::sanitize;
use crate::skill::{self, SkillPromptEntry};
use crate::status;
use crate::stores::{PermanentApprovalStore, SessionApprovalStore};
use crate::types::{CheckSource, PermissionCheckResult, PermissionState};

/// The extension's fixed id (pi `EXTENSION_ID`, `extension-config.ts:8`).
pub const EXTENSION_ID: &str = "cyrup-permission-system";

/// The global policy file (pi `pi-permissions.jsonc`; cyrup analog).
const POLICY_FILE: &str = "cyrup-permissions.jsonc";
/// The read-through permanent approvals file (pi `pi-permission-system-approvals.json`; cyrup analog).
const PERMANENT_APPROVALS_FILE: &str = "cyrup-permission-system-approvals.json";
/// The extension config dir + file (`<agent_dir>/cyrup-permission-system/config.json`).
const CONFIG_DIR: &str = "cyrup-permission-system";
const CONFIG_FILE: &str = "config.json";
/// The project-scoped policy dir (pi `<cwd>/.pi/agent`; cyrup `<cwd>/.cyrup/agent`).
const PROJECT_AGENT_SUBDIR: [&str; 2] = [".cyrup", "agent"];

/// The subagent-child env flag (value `"1"`) — literally the SAME const `cyrup-ext-subagents`
/// writes into every spawned child's env overlay
/// ([`cyrup_ext_subagents::spawn::nested_events::child_role_env`], driven from
/// `exec::build_attempt_spawn_plan`).
///
/// Aliased rather than re-typed as a literal: this crate ALREADY depends on `cyrup-ext-subagents`
/// (P-5, see `Cargo.toml`) and `ask.rs` already reads
/// `cyrup_ext_subagents::PARENT_SESSION_ENV_VAR` through that dependency, so the duplicate string
/// bought nothing and could silently drift out of agreement with the writer — which is exactly the
/// failure mode PERM-001 was: the gate read a name nothing on the spawn path ever wrote.
pub const CHILD_ENV_VAR: &str = cyrup_ext_subagents::spawn::nested_events::CHILD_ENV;

/// pi `SUBAGENT_ENV_HINT_KEYS` (`permission-forwarding.ts:9`) — the env keys whose presence means
/// "this process is running as a subagent child", ORed on any NON-EMPTY value by
/// [`is_subagent_child`] (pi `hasSubagentEnvHint`, `index.ts:93-103`).
///
/// The cyrup analogs, in upstream order, all three written into every child's spawn overlay by the
/// single chokepoint `cyrup_ext_subagents::exec::build_attempt_spawn_plan` (and aliased from that
/// crate rather than re-typed, for the same anti-drift reason as [`CHILD_ENV_VAR`]):
///
/// | pi | cyrup | what writes it |
/// |---|---|---|
/// | `PI_IS_SUBAGENT` | `CYRUP_SUBAGENT_CHILD` | `nested_events::child_role_env`, on EVERY spawn |
/// | `PI_SUBAGENT_SESSION_ID` | `CYRUP_SUBAGENT_RUN_ID` | the run-identity overlay, when the spawn belongs to a run |
/// | `PI_AGENT_ROUTER_SUBAGENT` | `CYRUP_SUBAGENT_AGENT_NAME` | the resolved persona name, when non-blank |
///
/// A ROOT orchestrator has none of them; the detached hop-2 `__subagent-runner` process has none
/// of them either (its hop-1 spawn overlays only the R-SA-P1 anchor), so it correctly keeps the
/// PARENT role and can host the forwarding watcher.
pub const SUBAGENT_ENV_HINT_KEYS: [&str; 3] = [
    CHILD_ENV_VAR,
    cyrup_ext_subagents::spawn::nested_events::RUN_ID_ENV,
    cyrup_ext_subagents::AGENT_NAME_ENV_VAR,
];

/// The explicit opt-in flag (DI-5): set truthy to force-install the gate even with no policy file.
pub const INSTALL_ENV_VAR: &str = "CYRUP_PERMISSION_SYSTEM";

/// pi's `notifyWarning` + `shownWarnings` pair (`index.ts:1573,1586-1592`): the ONE user-visible
/// sink every policy-file / config-file load warning funnels into, deduped for the life of a
/// session so a per-tool-call reload storm cannot spam the same message.
///
/// Before this existed, [`PermissionManager::with_on_warning`] was called only from unit tests, so
/// in production a malformed `cyrup-permissions.jsonc` fell back to `ask`-everything **in total
/// silence** — indistinguishable from a policy that genuinely says `ask`.
///
/// Holds the SAME late-bound `Arc<OnceLock<Arc<dyn HostServices>>>` the extension does, so a
/// manager built during construction (before the host attaches its backend) still delivers once
/// the backend lands — that late binding is why this is a shared handle and not a captured
/// `Arc<dyn HostServices>`.
struct WarningSink {
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// pi `shownWarnings` (`index.ts:1573`).
    shown: Mutex<HashSet<String>>,
}

impl WarningSink {
    fn new(host_services: Arc<OnceLock<Arc<dyn HostServices>>>) -> Self {
        Self { host_services, shown: Mutex::new(HashSet::new()) }
    }

    /// pi `notifyWarning` (`index.ts:1586-1592`): drop a message already shown this session, else
    /// remember it and push it to the host as a `warning` notification.
    ///
    /// \[CYRUP-DELTA] pi's guard is `!runtimeContext?.hasUI` — two conditions rolled into one,
    /// because pi's `ctx.ui.notify` is only reachable through a live context. Cyrup splits those:
    /// "is a host backend attached at all" is `host_services.get()`, which is the direct analog of
    /// pi's `runtimeContext != null` and is what is checked here. The `hasUI` half is NOT
    /// re-imposed: cyrup's [`HostServices::notify`] is already a fire-and-forget effect whose
    /// default implementation is a no-op and whose live implementation routes to whatever sink the
    /// active mode installed, so a headless host drops it on its own — and re-adding the check
    /// would suppress the warning in modes (e.g. RPC) that DO surface notifications.
    fn notify(&self, message: &str) {
        let Some(services) = self.host_services.get() else {
            return;
        };
        if !guard(&self.shown).insert(message.to_string()) {
            return;
        }
        services.notify(message, NotifyKind::Warning);
    }

    /// pi `resetShownWarnings` (`index.ts:1582-1584`), called on session start / reload / shutdown.
    fn reset(&self) {
        guard(&self.shown).clear();
    }
}

/// Build a [`PermissionManager`] whose `onWarning` is bound to `sink` — the analog of pi's
/// `createPermissionManagerForCwd(cwd, notifyWarning)` (`index.ts:1536-1550`), which likewise
/// threads the callback through EVERY construction site (`:1595`, `:2081`, `:2109-2110`). This is
/// the only way this crate builds a manager, so no construction site can silently drop policy-load
/// warnings again.
fn manager_with_warnings(paths: ManagerPaths, sink: &Arc<WarningSink>) -> PermissionManager {
    let sink = Arc::clone(sink);
    PermissionManager::new(paths).with_on_warning(move |message| sink.notify(message))
}

/// The permission-system extension: the layered policy engine + two approval stores + prompt dedup +
/// the fail-closed ask channel, gating every tool call via `before_tool_call`.
pub struct PermissionSystemExtension {
    id: ExtensionId,
    manager: Mutex<PermissionManager>,
    session_approvals: Mutex<SessionApprovalStore>,
    permanent_approvals: Mutex<PermanentApprovalStore>,
    dedup: Mutex<DedupCache>,
    /// The extension `config.json` snapshot. `yolo_mode` is read on the live `ask` path (below);
    /// `debug`/`forwarded_prompt_timeout_seconds` are consumed by later phases (logging / forwarding
    /// P-4) — the public fields carry them without any callerless primitive. `Mutex`-wrapped because
    /// [`Self::refresh_config_and_manager`] re-reads it from disk on `session_start` / a
    /// `resources_discover` reload (pi `refreshExtensionConfig`, `index.ts:1600-1608`).
    ///
    /// PERM-005: `Arc`-wrapped so the spawned forwarding watcher holds the SAME mutex and re-reads
    /// it once per poll iteration, the way pi's polling closure reads the module-scope
    /// `extensionConfig` binding `refreshExtensionConfig` reassigns. Handing the watcher a snapshot
    /// by value froze `yoloMode` / `forwardedPromptTimeoutSeconds` at spawn time.
    config: crate::forwarding::SharedExtensionConfig,
    /// The fail-closed FALLBACK ask channel ([`NoOpAskChannel`] in production; a scripted channel in
    /// unit tests via [`Self::from_parts`]). Used when no live UI is reachable — the live in-session
    /// dialog goes through [`LocalAskChannel`] over [`Self::host_services`] instead.
    ask_channel: Arc<dyn AskChannel>,
    /// The late-bound live capability backend (P-1), captured by [`Self::set_host_services`] BEFORE
    /// `init` (the builder threads its `LiveHostServices` via `load_native_with_services`). `Some` in
    /// an assembled interactive session; the in-session `ask` dialog uses it (guarded by `ctx.has_ui`)
    /// to reach `HostServices::select`/`input`, and the PARENT forwarding watcher uses it (outside any
    /// `HostCtx`) to reach `session_id`/`select`/`input`/`human_interaction_lock`. This is the SAME
    /// `OnceLock` a child's [`ForwardingAskChannel`] shares (so it observes the child's own session id
    /// for the requester metadata). `None` (default host / headless) ⇒ fail-closed / no watcher.
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// The agent dir whose `sessions/permission-forwarding/…` subtree is the shared forwarding spool
    /// root (pi `PI_AGENT_DIR`). The parent watcher resolves its inbox under this.
    agent_dir: PathBuf,
    /// Whether this extension is a PARENT (root) that installs the forwarding watcher on
    /// `SessionStart`. `true` only for [`Self::new_forwarding_parent`]; a child ([`ForwardingAskChannel`]
    /// role) and the bare test constructors ([`Self::new`]/[`Self::from_parts`]) leave it `false`.
    install_watcher: bool,
    /// The live forwarding-watcher task handle (parent role), so `SessionShutdown` can `abort()` it
    /// (pi teardown, `index.ts:2131`) and a session rebuild does not double-spawn.
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The resolved persona/agent name this process runs as (pi `resolveAgentName`, `index.ts:
    /// 2033-2047`), captured ONCE from the `CYRUP_SUBAGENT_AGENT_NAME` env var at construction (the
    /// child IS its persona for its whole lifetime). `Some` threads the agent + projectAgent policy
    /// LAYERS into `check_permission` / `format_*` / dedup; `None` (a top-level process, which never
    /// has the var) matches pi's normalized-`""` top-level (global + project layers still enforce).
    agent_name: Option<String>,
    /// The active-skill enforcement entries parsed out of the `before_agent_start` system prompt's
    /// `<available_skills>` block (pi `activeSkillEntries`, `index.ts:1558` / `resolveSkillPromptEntries`,
    /// built at `before_agent_start`, read at `tool_call` for the skill-read bypass, `index.ts:2232`).
    active_skill_entries: Mutex<Vec<SkillPromptEntry>>,
    /// Skill names the human explicitly invoked via a `/skill:<name>` slash command (pi
    /// `explicitlyRequestedSkillNames`, `index.ts:1559` / `index.ts:2192-2206`): a direct user action,
    /// so its skill-file reads bypass the skill-read ask/deny even under a hiding agent (`index.ts:2243`).
    explicitly_requested_skill_names: Mutex<HashSet<String>>,
    /// pi's `notifyWarning` closure (`index.ts:1586-1592`), shared by value with every
    /// [`PermissionManager`] this extension builds (`onWarning`) and used directly for the
    /// extension-config load warning.
    warnings: Arc<WarningSink>,
    /// pi `lastConfigWarning` (`index.ts:1572`): the extension-config warning already reported, so
    /// a repeated refresh that keeps failing the same way notifies once and a refresh that STOPS
    /// failing re-arms the report (`refreshExtensionConfig`, `index.ts:1610-1618`).
    last_config_warning: Mutex<Option<String>>,
}

impl PermissionSystemExtension {
    /// The bare constructor (test / non-forwarding): derive every policy/store path from `agent_dir` +
    /// the session `cwd`, and fail-close asks through [`NoOpAskChannel`] (the live in-session dialog
    /// still activates via `ctx.has_ui` + a captured backend). Installs NO forwarding watcher — the
    /// wired PARENT uses [`Self::new_forwarding_parent`].
    #[must_use]
    pub fn new(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let (paths, permanent_path, config) = Self::derive_parts(&agent_dir, cwd);
        Self::from_parts_full(
            paths,
            permanent_path,
            config,
            Arc::new(NoOpAskChannel),
            agent_dir,
            false,
            Arc::new(OnceLock::new()),
        )
    }

    /// The wired PARENT (root, `CYRUP_SUBAGENT_DEPTH == 0`) constructor: like [`Self::new`] but marks
    /// `install_watcher` so `on_event` spawns the [`forwarding::spawn_forwarding_watcher`] task that
    /// services subagent children's forwarded asks — from `SessionStart`, `BeforeAgentStart`, `Input`
    /// and `ToolCall` alike (PERM-005; idempotently, so the per-turn hooks do not stack watchers).
    #[must_use]
    pub fn new_forwarding_parent(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let (paths, permanent_path, config) = Self::derive_parts(&agent_dir, cwd);
        Self::from_parts_full(
            paths,
            permanent_path,
            config,
            Arc::new(NoOpAskChannel),
            agent_dir,
            true,
            Arc::new(OnceLock::new()),
        )
    }

    /// The wired CHILD (`CYRUP_SUBAGENT_CHILD`, `DEPTH > 0`) constructor: installs the
    /// [`ForwardingAskChannel`] as the gate's `ask_channel`, so an ask-tier decision forwards UP to the
    /// parent's human (pi `confirmPermission` subagent branch) instead of fail-closing. The channel
    /// shares the extension's `host_services` slot (for the requester session-id metadata) and its wait
    /// bound is [`forwarding::resolve_child_wait_timeout`] (pi's 10-min `PERMISSION_FORWARDING_TIMEOUT`,
    /// ops-overridable). No watcher (a child is a responder to no one).
    #[must_use]
    pub fn new_forwarding_child(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let (paths, permanent_path, config) = Self::derive_parts(&agent_dir, cwd);
        let host_services: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
        let channel: Arc<dyn AskChannel> = Arc::new(ForwardingAskChannel::new(
            agent_dir.clone(),
            forwarding::resolve_child_wait_timeout(),
            host_services.clone(),
        ));
        Self::from_parts_full(paths, permanent_path, config, channel, agent_dir, false, host_services)
    }

    /// Derive the [`ManagerPaths`] for `agent_dir` + `cwd` (pi `createPermissionManagerForCwd`'s path
    /// derivation, `index.ts:1536-1573`) — shared by every constructor AND by
    /// [`Self::refresh_config_and_manager`] (a `session_start` / `resources_discover` reload rebuilds
    /// this from the CURRENT cwd, not just the process's original one).
    fn manager_paths_for(agent_dir: &Path, cwd: &Path) -> ManagerPaths {
        let project_dir = PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg));
        ManagerPaths {
            global_config_path: agent_dir.join(POLICY_FILE),
            agents_dir: agent_dir.join("agents"),
            project_global_config_path: Some(project_dir.join(POLICY_FILE)),
            project_agents_dir: Some(project_dir.join("agents")),
            legacy_global_settings_path: agent_dir.join("settings.json"),
            global_mcp_config_path: agent_dir.join("mcp.json"),
            mcp_server_names_override: None,
        }
    }

    /// The extension `config.json` path for `agent_dir` (pi `getPermissionSystemConfigPath`,
    /// `extension-config.ts:43-46`).
    fn config_path_for(agent_dir: &Path) -> PathBuf {
        agent_dir.join(CONFIG_DIR).join(CONFIG_FILE)
    }

    /// Derive the [`ManagerPaths`] + permanent-store path + extension config from `agent_dir` + `cwd`
    /// (shared by every constructor).
    fn derive_parts(agent_dir: &Path, cwd: PathBuf) -> (ManagerPaths, PathBuf, ExtensionConfig) {
        let paths = Self::manager_paths_for(agent_dir, &cwd);
        let permanent_path = agent_dir.join(PERMANENT_APPROVALS_FILE);
        let config = ExtensionConfig::load(&Self::config_path_for(agent_dir));
        (paths, permanent_path, config)
    }

    /// pi `refreshSessionRuntimeState` (`index.ts:2077-2085`) + the `resources_discover` "reload"
    /// branch (`index.ts:2103-2118`): re-read `config.json` from disk into `self.config`, rebuild
    /// `self.manager`'s policy paths from the CURRENT `cwd` (not the process's original one), and
    /// invalidate the agent-start cache (`invalidateAgentStartCache`, `:1575-1581`) by clearing the
    /// cached active-skill enforcement entries. Shared by both the `session_start` handler and a
    /// `resources_discover` reload.
    /// Also surfaces the extension-config load warning (pi `refreshExtensionConfig`,
    /// `index.ts:1600-1618`) — this is the one place a malformed `config.json` becomes visible,
    /// since construction happens before any host backend is attached.
    fn refresh_config_and_manager(&self, cwd: &Path) {
        // pi order (`refreshSessionRuntimeState`, `index.ts:2077-2085`): config first, manager
        // second.
        let loaded = ExtensionConfig::load_with_result(&Self::config_path_for(&self.agent_dir));
        *guard(&self.config) = loaded.config;
        self.report_config_warning(loaded.warning);
        *guard(&self.manager) =
            manager_with_warnings(Self::manager_paths_for(&self.agent_dir, cwd), &self.warnings);
        guard(&self.active_skill_entries).clear();
    }

    /// pi `refreshExtensionConfig`'s warning branch (`index.ts:1610-1618`): report a NEW warning
    /// once and remember it; clear the memo when the load comes back clean, so a later recurrence
    /// is reported again.
    fn report_config_warning(&self, warning: Option<String>) {
        let Some(warning) = warning else {
            *guard(&self.last_config_warning) = None;
            return;
        };
        // Scoped so the memo lock is released before the sink is touched — `notify` takes its own
        // lock and reaches the host.
        let is_new = {
            let mut last = guard(&self.last_config_warning);
            let is_new = last.as_deref() != Some(warning.as_str());
            if is_new {
                *last = Some(warning.clone());
            }
            is_new
        };
        if is_new {
            self.warnings.notify(&warning);
        }
    }

    /// Assemble from explicit parts (used by tests that point the global policy path at a fixture file
    /// / inject a scripted ask channel). Derives `agent_dir` from the policy path's parent; installs no
    /// watcher and a fresh capability slot.
    #[must_use]
    pub fn from_parts(
        paths: ManagerPaths,
        permanent_path: PathBuf,
        config: ExtensionConfig,
        ask_channel: Arc<dyn AskChannel>,
    ) -> Self {
        let agent_dir = paths
            .global_config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self::from_parts_full(
            paths,
            permanent_path,
            config,
            ask_channel,
            agent_dir,
            false,
            Arc::new(OnceLock::new()),
        )
    }

    /// The one true assembler every constructor funnels through.
    #[must_use]
    fn from_parts_full(
        paths: ManagerPaths,
        permanent_path: PathBuf,
        config: ExtensionConfig,
        ask_channel: Arc<dyn AskChannel>,
        agent_dir: PathBuf,
        install_watcher: bool,
        host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    ) -> Self {
        // Built BEFORE the struct literal: `host_services` is moved into the literal below, and the
        // sink needs its own handle on the same `OnceLock` so the manager's `onWarning` binding
        // observes the backend the host attaches LATER (`set_host_services` runs after
        // construction).
        let warnings = Arc::new(WarningSink::new(Arc::clone(&host_services)));
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            manager: Mutex::new(manager_with_warnings(paths, &warnings)),
            session_approvals: Mutex::new(SessionApprovalStore::new()),
            permanent_approvals: Mutex::new(PermanentApprovalStore::new(permanent_path)),
            dedup: Mutex::new(DedupCache::new()),
            config: Arc::new(Mutex::new(config)),
            ask_channel,
            host_services,
            agent_dir,
            install_watcher,
            watcher: Mutex::new(None),
            agent_name: resolve_agent_name_from_env(),
            active_skill_entries: Mutex::new(Vec::new()),
            explicitly_requested_skill_names: Mutex::new(HashSet::new()),
            warnings,
            last_config_warning: Mutex::new(None),
        }
    }

    /// Override the resolved persona name (deterministic tests / an embedder that resolves the name
    /// itself). Production leaves the env-sourced value from [`resolve_agent_name_from_env`] in place.
    /// Trims; empty → `None` (pi `normalizeAgentName`, `index.ts:277-284`).
    #[must_use]
    pub fn with_agent_name(mut self, agent_name: Option<String>) -> Self {
        self.agent_name = agent_name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self
    }

    /// The gate (pi `index.ts:2208-2499`, the deciding subset): resolve `tool_name` + `input`, fold
    /// the approval stores, then `Block` on deny / fail-closed ask, or proceed on allow. Returns the
    /// `HookOutcome` the dispatcher maps to `BeforeOutcome`.
    async fn decide(&self, call_id: &str, tool_name: &str, input: &Value, ctx: &HostCtx) -> HookOutcome {
        let normalized = tool_name.trim();
        if normalized.is_empty() {
            return HookOutcome::Block { reason: Some(gate::format_missing_tool_name_reason()) };
        }
        let agent_name = self.agent_name.as_deref();

        // (2) REGISTRY / unknown-tool gate (pi `index.ts:2218-2228`): ALWAYS runs, unconditionally,
        // against the full registry BEFORE any permission check — pi has no skip path
        // (`checkRequestedToolRegistration(toolName, pi.getAllTools())` is called every time, and
        // `pi.getAllTools()` never returns `undefined`). When the live backend cannot enumerate the
        // registry (`all_tool_names` returns `None` — no backend attached, or a wiring gap on an
        // attached one) this fails CLOSED against an EMPTY registry rather than skipping the gate:
        // exactly what pi would do if its tool registry were ever empty (nothing matches ⇒ every tool
        // is "unregistered"). An unattached/misconfigured host can no longer silently bypass the
        // unknown-tool allowlist.
        let registered = self.registered_tool_names().unwrap_or_default();
        if let Some(reason) = gate::check_requested_tool_registration(normalized, &registered) {
            return HookOutcome::Block { reason: Some(reason) };
        }

        // pi `index.ts:2305-2309`: anchor a path-bearing input's resource resolution to the SESSION
        // cwd (`HostCtx.cwd`) when the input carries a `path`/`file_path` but no `cwd` of its own. Used
        // for the skill-read + external-directory + main checks below (pi threads this same `input`).
        let cwd: String = ctx.cwd.to_string_lossy().into_owned();
        let injected = gate::inject_cwd(input, &cwd);
        let input: &Value = &injected;

        // (3) SKILL-READ bypass (pi `index.ts:2230-2303`): a `read` whose path lands on a tracked skill
        // is governed by the SKILL policy (allow → proceed; ask → prompt; deny → block), bypassing the
        // read-tool policy. `None` = no skill matched → fall through to the external-dir + main checks.
        if normalized == "read"
            && let Some(outcome) = self.resolve_skill_read(input, agent_name, &cwd, ctx).await
        {
            return outcome;
        }

        // (4) EXTERNAL-DIRECTORY guard (pi `index.ts:2310-2414`): a path-bearing tool targeting a path
        // OUTSIDE the working directory is gated by the `external_directory` special policy first.
        // `None` = allowed / not applicable → fall through to the main check (which uses the SAME
        // `input`); `Some(_)` = a terminal deny / denied-ask / ask-unavailable block.
        if !cwd.is_empty()
            && let Some(path) = gate::get_path_bearing_tool_path(normalized, input)
            && gate::is_path_outside_working_directory(&path, &cwd)
            && let Some(outcome) =
                self.resolve_external_directory(normalized, &path, &cwd, agent_name, ctx).await
        {
            return outcome;
        }

        // Main check + store overlay — fully synchronous; every lock is dropped before any await.
        let check = {
            let session_rules = guard(&self.session_approvals).get_rules();
            let permanent_rules = guard(&self.permanent_approvals).get_rules();
            let raw = guard(&self.manager).check_permission(normalized, input, agent_name);
            gate::apply_pattern_approval_state(raw, input, &session_rules, &permanent_rules)
        };

        match check.state {
            PermissionState::Deny => {
                HookOutcome::Block { reason: Some(gate::format_deny_reason(&check, agent_name)) }
            }
            PermissionState::Allow => HookOutcome::Noop,
            PermissionState::Ask => self.resolve_ask(call_id, input, &check, ctx).await,
        }
    }

    /// The full registry tool names (pi `pi.getAllTools()`, the `getAllTools` analog) via the captured
    /// live backend, or `None` when no live backend is attached (default host / headless). See
    /// [`cyrup_ext::HostServices::all_tool_names`] for why this is the FULL registry, not the exposed
    /// subset [`cyrup_ext::HostServices::active_tools`] returns.
    fn registered_tool_names(&self) -> Option<Vec<String>> {
        self.host_services.get().and_then(|s| s.all_tool_names())
    }

    /// (3) The skill-read bypass (pi `index.ts:2230-2303`). Resolves the `read` path against the
    /// active-skill entries (exact/base-dir match) and, failing that, an inferred skills-root entry;
    /// then, unless the skill was explicitly `/skill:`-requested, enforces its policy: `deny` → block,
    /// `ask` → live prompt (fail-closed / user-deny → block), `allow`/approved → proceed. Returns
    /// `Some(HookOutcome)` when a skill matched (a terminal decision, allow via `Noop`), `None` when no
    /// skill matched (the caller falls through to the external-dir + main checks).
    async fn resolve_skill_read(
        &self,
        input: &Value,
        agent_name: Option<&str>,
        cwd: &str,
        ctx: &HostCtx,
    ) -> Option<HookOutcome> {
        let read_path =
            to_record(input).get("path").and_then(Value::as_str).unwrap_or("").to_string();
        let normalized_read_path = common::normalize_path_for_comparison(&read_path, cwd);

        // A tracked-entry match (pi `findSkillPathMatch`), else an inferred skills-root entry whose
        // state comes from a fresh `checkPermission("skill", {name}, agentName)` (pi `:2236-2241`).
        let matched = {
            let entries = guard(&self.active_skill_entries);
            skill::find_skill_path_match(&normalized_read_path, &entries).cloned()
        };
        let read_skill = match matched {
            Some(m) => m,
            None => {
                let agent_dir = self.agent_dir.to_string_lossy().into_owned();
                // No skill matched (tracked or inferred) → `?` returns `None` so the caller falls
                // through to the external-dir + main checks (pi `:2300` — no `readSkill`).
                let mut inferred = skill::infer_skill_entry_from_read_path(
                    &read_path,
                    cwd,
                    &agent_dir,
                    PermissionState::Ask,
                )?;
                inferred.state = guard(&self.manager)
                    .check_permission("skill", &json!({ "name": inferred.name.clone() }), agent_name)
                    .state;
                inferred
            }
        };

        let explicitly_requested =
            guard(&self.explicitly_requested_skill_names).contains(&read_skill.name);
        if !explicitly_requested {
            match read_skill.state {
                PermissionState::Deny => {
                    return Some(HookOutcome::Block {
                        reason: Some(skill::format_skill_path_deny_reason(&read_skill, agent_name)),
                    });
                }
                PermissionState::Ask => {
                    let message =
                        skill::format_skill_path_ask_prompt(&read_skill, &read_path, agent_name);
                    match self.prompt_decision(&message, ctx).await {
                        AskOutcome::NoLiveChannel => {
                            return Some(HookOutcome::Block {
                                reason: Some(skill::skill_ask_unavailable_reason()),
                            });
                        }
                        AskOutcome::Decided(d) if !d.approved => {
                            return Some(HookOutcome::Block {
                                reason: Some(skill::format_skill_user_denied_reason(
                                    d.denial_reason.as_deref(),
                                )),
                            });
                        }
                        AskOutcome::Decided(_) => {}
                    }
                }
                PermissionState::Allow => {}
            }
        }
        // A skill matched → allow the read, bypassing the read-tool policy (pi `:2300-2302`).
        Some(HookOutcome::Noop)
    }

    /// (4) The external-directory guard (pi `index.ts:2312-2413`). Checks the `external_directory`
    /// special policy for `{path, cwd}` (with the session/permanent overlay applied on an `ask`): `deny`
    /// → block; `ask` → live prompt (fail-closed / user-deny → block; approved-Always → session-persist,
    /// then fall through); `allow` → fall through. `None` = allowed (proceed to the main check).
    async fn resolve_external_directory(
        &self,
        tool_name: &str,
        path: &str,
        cwd: &str,
        agent_name: Option<&str>,
        ctx: &HostCtx,
    ) -> Option<HookOutcome> {
        let ext_input = json!({ "path": path, "cwd": cwd });
        let raw = guard(&self.manager).check_permission("external_directory", &ext_input, agent_name);
        // pi `:2319-2321`: the session/permanent overlay is applied ONLY on an `ask` result.
        let ext_check = if raw.state == PermissionState::Ask {
            let session_rules = guard(&self.session_approvals).get_rules();
            let permanent_rules = guard(&self.permanent_approvals).get_rules();
            gate::apply_pattern_approval_state(raw, &ext_input, &session_rules, &permanent_rules)
        } else {
            raw
        };

        match ext_check.state {
            PermissionState::Deny => Some(HookOutcome::Block {
                reason: Some(gate::format_external_directory_deny_reason(
                    tool_name, path, cwd, agent_name,
                )),
            }),
            PermissionState::Ask => {
                let message =
                    gate::format_external_directory_ask_prompt(tool_name, path, cwd, agent_name);
                match self.prompt_decision(&message, ctx).await {
                    AskOutcome::NoLiveChannel => Some(HookOutcome::Block {
                        reason: Some(gate::format_external_directory_unavailable_reason(path)),
                    }),
                    AskOutcome::Decided(d) if !d.approved => Some(HookOutcome::Block {
                        reason: Some(gate::format_external_directory_user_denied_reason(
                            tool_name,
                            path,
                            d.denial_reason.as_deref(),
                        )),
                    }),
                    AskOutcome::Decided(d) => {
                        // pi `persistPatternApprovalDecision` (`:2391`): an approved-Always persists an
                        // allow rule to the SESSION store, then the call FALLS THROUGH to the main check.
                        if d.state == PermissionDecisionState::Always {
                            let subject = gate::get_pattern_approval_subject(&ext_check, &ext_input);
                            if !subject.is_empty() {
                                guard(&self.session_approvals)
                                    .approve_always(&ext_check.tool_name, &subject);
                            }
                        }
                        None
                    }
                }
            }
            PermissionState::Allow => None,
        }
    }

    /// Prompt the human for a bespoke `message` (the skill-read + external-dir asks, which manage their
    /// own persistence — no dedup/`Always` tail): the pi `canResolveAskPermissionRequest` fail-fast
    /// pre-check (`yolo-mode.ts:21-23`, consulted via `canRequestPermissionConfirmation` BEFORE any
    /// prompt/lock work at `index.ts:2263,2351,2452`) — `hasUI || isSubagent || yoloMode` — then yolo
    /// auto-approve (pi `shouldAutoApprovePermissionState`), the C3 human-interaction lock, the
    /// live-vs-fallback channel selection, and the P-3 dispatch-budget-forgiveness guard held across
    /// the BLOCKING dialog — the SAME machinery [`Self::resolve_ask`] uses. `AskOutcome::NoLiveChannel`
    /// = fail-CLOSED (no reachable human), returned IMMEDIATELY by the pre-check when none of the three
    /// conditions hold — zero lock/dialog work touched, exactly like pi's early return.
    async fn prompt_decision(&self, message: &str, ctx: &HostCtx) -> AskOutcome {
        let yolo_mode = guard(&self.config).yolo_mode;
        if !(ctx.has_ui || is_subagent_child() || yolo_mode) {
            return AskOutcome::NoLiveChannel;
        }
        if yolo_mode {
            return AskOutcome::Decided(PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Approved,
                denial_reason: None,
            });
        }
        let human_lock = self.host_services.get().and_then(|s| s.human_interaction_lock());
        let _human_guard = match human_lock {
            Some(lock) => Some(lock.acquire().await),
            None => None,
        };
        let channel: Arc<dyn AskChannel> = match (ctx.has_ui, self.host_services.get()) {
            (true, Some(services)) => Arc::new(LocalAskChannel::new(services.clone())),
            _ => self.ask_channel.clone(),
        };
        let _human_wait = ctx.begin_human_wait();
        channel.confirm("Permission Required", message, PromptOpts::default()).await
    }

    /// The main-check `ask` branch (pi `:2444-2496` + `promptPermission :1794-1902` + `confirmPermission
    /// :1506-1513`): dedup → the shared [`Self::prompt_decision`] core (yolo → C3 human-interaction lock
    /// → live dialog under a P-3 budget-forgiveness guard) → fail-CLOSED when no human is reachable →
    /// remember + apply (the `Always` session-persist tail). The prompt subject now names the resolved
    /// persona (real `agent_name`, pi `formatAskPrompt(check, agentName, input)`).
    async fn resolve_ask(
        &self,
        call_id: &str,
        input: &Value,
        check: &PermissionCheckResult,
        ctx: &HostCtx,
    ) -> HookOutcome {
        let agent_name = self.agent_name.as_deref();

        let details = dedup_details(call_id, input, check, agent_name);
        let key = details.cache_key();

        // Dedup hit: reuse the prior decision (collapsed to Allow-Once) — zero additional prompts.
        if let Some(k) = &key {
            let cached = guard(&self.dedup).get(k);
            if let Some(decision) = cached {
                return self.apply_decision(decision, check, input);
            }
        }

        // pi `formatAskPrompt` (`index.ts:570-590`) — the human-facing prompt (NOT the headless reason).
        // The shared prompting core applies yolo auto-approve (pi `shouldAutoApprovePermissionState`),
        // the C3 human lock, the live-vs-fallback channel, and the P-3 dispatch-budget guard.
        let message = gate::format_ask_prompt(check, agent_name, input);
        let decision = match self.prompt_decision(&message, ctx).await {
            AskOutcome::Decided(d) => d,
            // Fail-CLOSED: no reachable human (headless / no live UI) → Block, never proceed
            // (pi `confirmPermission` headless `{approved:false}` :1509-1513 / `:2452-2467`).
            AskOutcome::NoLiveChannel => {
                return HookOutcome::Block { reason: Some(gate::format_ask_unavailable_reason(check)) };
            }
        };

        if let Some(k) = &key {
            guard(&self.dedup).remember(k, decision.clone());
        }
        self.apply_decision(decision, check, input)
    }

    /// Apply a resolved decision (pi `:2478-2495`): not-approved → Block (`formatUserDeniedReason`);
    /// approved-Always → persist an allow rule to the SESSION store (pi `index.ts:905`; §8.2: the
    /// permanent on-disk store is NEVER written at runtime — it is read-through only). The `Always`
    /// persist branch fires on a real dialog returning "Allow Always" ([`LocalAskChannel`]); a later
    /// same-subject call then auto-allows via the store overlay with no second dialog (proven by
    /// `tests/human_dialog.rs`). `Once`/`Approved` (yolo) approve without persisting.
    fn apply_decision(
        &self,
        decision: PermissionPromptDecision,
        check: &PermissionCheckResult,
        input: &Value,
    ) -> HookOutcome {
        if !decision.approved {
            return HookOutcome::Block {
                reason: Some(gate::format_user_denied_reason(check, decision.denial_reason.as_deref())),
            };
        }
        if decision.state == PermissionDecisionState::Always {
            let subject = gate::get_pattern_approval_subject(check, input);
            if !subject.is_empty() {
                guard(&self.session_approvals).approve_always(&check.tool_name, &subject);
            }
        }
        HookOutcome::Noop
    }

    /// The `before_agent_start` context-hygiene shaping (pi `index.ts:2134-2190`, port doc §9). Runs
    /// three faithful steps and RETURNS the sanitized system prompt as a `[mutate]` (the system-prompt
    /// MUTATE seam, `contract.rs` `EventPatch::SystemPromptAndInject` → `session.rs`
    /// `assemble_run_messages`):
    /// 1. **Active-tools exposure** (pi `setActiveTools`, `:2155`): for every registered tool
    ///    ([`HostServices::all_tool_names`], pi `getAllTools`), keep it iff [`Self::should_expose_tool`];
    ///    restrict the live agent's tool set via [`HostServices::set_active_tools`] (staged as
    ///    `pending_active_tools`, drained + applied IN-TURN by `AgentSession::assemble_run_messages`, so
    ///    it shapes turn 1 ordered BEFORE the sanitized prompt). Skipped when no live backend can
    ///    enumerate the registry (pi always has `getAllTools`; the default host does not).
    /// 2. **`sanitizeAvailableToolsSection`** ([`crate::sanitize::tools`], `:2174`) over the exposed set.
    /// 3. **`resolveSkillPromptEntries`** ([`crate::sanitize::skills`], `:2175`) — hides `ask`/`deny`
    ///    skills from `<available_skills>` while CACHING the enforcement entries the skill-read gate
    ///    reads at every `tool_call`. ONE parse, both consumers.
    ///
    /// Also syncs the `"yolo"` status pill (pi `syncPermissionSystemStatus`, `:2136`).
    fn on_before_agent_start(&self, system_prompt: &str, ctx: &HostCtx) -> HookOutcome {
        let cwd = ctx.cwd.to_string_lossy().into_owned();
        let agent = self.agent_name.as_deref();
        let services = self.host_services.get();

        // (1) Active-tools exposure — only when the live backend can enumerate the FULL registry.
        let working_prompt = match services.and_then(|s| s.all_tool_names()) {
            Some(tools) => {
                let allowed: Vec<String> = tools
                    .into_iter()
                    .filter(|name| self.should_expose_tool(name, agent))
                    .collect();
                if let Some(s) = services {
                    s.set_active_tools(&allowed);
                }
                // (2) Strip the "Available tools:" section + denied-tool guideline bullets.
                sanitize::tools::sanitize_available_tools_section(system_prompt, &allowed).prompt
            }
            // No live registry (default host / headless): cannot compute the exposed set → leave the
            // tools section intact (pi always has `getAllTools`); still resolve/hide skills below.
            None => system_prompt.to_string(),
        };

        // (3) Hide ask/deny skills from `<available_skills>` + cache the enforcement entries. ONE
        // parse feeds both the enforcement cache (read at every `tool_call`) and the hidden prompt.
        let resolution = {
            let mut mgr = guard(&self.manager);
            sanitize::skills::resolve_skill_prompt_entries(&working_prompt, &mut mgr, agent, &cwd)
        };
        *guard(&self.active_skill_entries) = resolution.entries;

        // pi `syncPermissionSystemStatus` (`:2136`): reflect yolo on the live status bar.
        if let Some(s) = services {
            status::sync_status(s, &guard(&self.config));
        }

        // Return the sanitized prompt as a [mutate] ONLY when it differs from the original (pi
        // `skillPromptResult.prompt !== event.systemPrompt ? { systemPrompt } : {}`, `:2185-2189`).
        if resolution.prompt == system_prompt {
            HookOutcome::Noop
        } else {
            HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                system: Some(resolution.prompt),
                inject: None,
            })
        }
    }

    /// pi `shouldExposeTool` (`index.ts:2049-2075`): keep a tool exposed iff its TOOL-LEVEL permission
    /// ([`PermissionManager::get_tool_permission`]) — with the session/permanent approval overlay (pi
    /// `applyPatternApprovalState(..., {}, ...)`) — is not `deny`. A `deny` `read` is still exposed when
    /// the agent has allowed skills ([`PermissionManager::has_allowed_skills`], pi `:2070`) so it can
    /// reach skill files; a `deny` `bash` is still exposed when the agent has an explicitly allowed bash
    /// command ([`PermissionManager::get_bash_permissions`]) so the agent keeps its permitted commands
    /// (the gate re-checks each — the mandate-directed analog of the read/skills bypass).
    fn should_expose_tool(&self, tool_name: &str, agent_name: Option<&str>) -> bool {
        let session_rules = guard(&self.session_approvals).get_rules();
        let permanent_rules = guard(&self.permanent_approvals).get_rules();
        let mut mgr = guard(&self.manager);

        let raw = PermissionCheckResult {
            tool_name: tool_name.to_string(),
            state: mgr.get_tool_permission(tool_name, agent_name),
            matched_pattern: None,
            command: None,
            target: None,
            source: CheckSource::Tool,
        };
        let state =
            gate::apply_pattern_approval_state(raw, &json!({}), &session_rules, &permanent_rules)
                .state;
        if state != PermissionState::Deny {
            return true;
        }
        if tool_name == "read" && mgr.has_allowed_skills(agent_name) {
            return true;
        }
        if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() {
            return true;
        }
        false
    }

    /// pi `startForwardedPermissionPolling` (`index.ts:1983-2031`): in the PARENT role
    /// (`install_watcher`), on a session WITH a UI and a captured live backend, ensure the forwarding
    /// watcher is running.
    ///
    /// **IDEMPOTENT** — this is the crux of PERM-005. Upstream re-enters this function on FOUR hooks
    /// (`refreshSessionRuntimeState`/`session_start` `:2084`, `before_agent_start` `:2137`, `input`
    /// `:2194`, `tool_call` `:2210`), and cyrup now calls it from the same four places, so it fires on
    /// every turn. The `is_finished()` check below makes N calls yield exactly ONE live watcher — pi's
    /// analog is `if (permissionForwardingWatcher && watchedPermissionForwardingRequestsDir ===
    /// location.requestsDir) { …; return; }` (`index.ts:1996-2000`), which likewise keeps the existing
    /// watcher rather than re-arming one per hook.
    ///
    /// **STOPS on the disqualifying branch** (PERM-005): pi's early return is
    /// `if (!ctx.hasUI || isSubagentExecutionContext(ctx)) { stopForwardedPermissionPolling(); return; }`
    /// (`index.ts:1984-1987`) — it TEARS DOWN a live watcher rather than leaving one orphaned. Cyrup's
    /// guard used to return without stopping, so a UI that detached mid-session left the watcher
    /// prompting into a dead backend.
    ///
    /// A missing `host_services` backend is NOT a disqualifier — it is the "cannot attach yet" case
    /// (pi's `if (!location) return;`, `:1991-1993`), which upstream leaves running for the next hook.
    fn maybe_start_forwarding_watcher(&self, ctx: &HostCtx) {
        if !self.install_watcher || !ctx.has_ui {
            // pi `:1985`: a non-parent / headless context tears the watcher DOWN, it does not merely
            // decline to start one.
            self.stop_forwarding_watcher();
            return;
        }
        let Some(services) = self.host_services.get() else {
            return;
        };
        let mut slot = guard(&self.watcher);
        // Re-entrancy guard: keep a live watcher; only replace a finished one.
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        *slot = Some(forwarding::spawn_forwarding_watcher(
            self.agent_dir.clone(),
            services.clone(),
            Arc::clone(&self.config),
        ));
    }

    /// pi `stopForwardedPermissionPolling` (`index.ts:1970-1981`, called from `session_shutdown`
    /// `:2131` and from the disqualified branch of `startForwardedPermissionPolling` `:1985`): abort
    /// the forwarding watcher task. Idempotent — a no-op when no watcher is installed.
    fn stop_forwarding_watcher(&self) {
        if let Some(handle) = guard(&self.watcher).take() {
            handle.abort();
        }
    }

    /// Test seam: is a live (unfinished) forwarding-watcher task currently installed? Used by the
    /// PERM-005 idempotence regression tests to assert that N `maybe_start_forwarding_watcher` calls
    /// yield exactly one watcher.
    #[cfg(test)]
    fn has_live_forwarding_watcher(&self) -> bool {
        guard(&self.watcher).as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Test seam (PERM-005): how many watcher TASKS currently exist, counted independently of the
    /// `watcher` slot.
    ///
    /// Every spawned watcher moves its own clone of the shared `config` handle into the task future,
    /// synchronously at `tokio::spawn` time, so `Arc::strong_count - 1` (the extension's own handle)
    /// is the number of watcher futures still alive. This is the assertion that would catch a
    /// non-idempotent start: the slot only ever holds ONE `JoinHandle`, so overwriting it would hide
    /// a leaked task, whereas the leaked task's `Arc` clone cannot hide.
    #[cfg(test)]
    fn live_watcher_task_count(&self) -> usize {
        Arc::strong_count(&self.config).saturating_sub(1)
    }

    /// PERM-001 — publish this PARENT session's own id as the process-wide parent-session anchor
    /// (`cyrup_ext_subagents::publish_parent_session_anchor`), the address a subagent child's
    /// forwarded ask writes to.
    ///
    /// This is the cyrup placement of pi's `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`
    /// (`pi-subagents/src/extension/index.ts:599` @v0.34.0). Upstream, the SUBAGENTS extension does it, into
    /// the real process environment, so every descendant — foreground, background, detached, at any
    /// hop — inherits the anchor for free. `cyrup-ext-subagents` cannot: it is
    /// `#![forbid(unsafe_code)]` and `std::env::set_var` is `unsafe` in edition 2024, so it keeps
    /// the captured anchor in a private executor field and threads it explicitly — which reaches
    /// only the FOREGROUND spawn path. A background run crosses two OS process boundaries, and
    /// neither carried the anchor, so a background child's `ask` addressed a null target and
    /// `forwarding::wait_for_forwarded_approval` fail-closed DENIED it with no prompt ever reaching
    /// the operator.
    ///
    /// This crate is the anchor's sole consumer and, unlike `cyrup-ext-subagents`, sits in the root
    /// process with the live session id in hand at exactly pi's moment (`SessionStart`), so it is
    /// the natural publisher of the memory-safe register that stands in for pi's `process.env`
    /// slot. `cyrup_ext_subagents::background::spawn_detached` reads it back when it builds the
    /// hop-1 env overlay, restoring the inheritance chain pi gets for free.
    ///
    /// PARENT role only (`install_watcher`). A CHILD must never publish its own id: a depth-2
    /// grandchild would then address its immediate parent's spool instead of continuing to thread
    /// the root's anchor, which is the direct-parent depth-1 semantics
    /// `cyrup_ext_subagents::PARENT_SESSION_ENV_VAR` documents. Publishing is also unconditional in
    /// `has_ui` (pi's `index.ts:599` is), so a UI-less parent that later gains one still has a
    /// correctly-addressed anchor in place; the watcher, not the anchor, is what `has_ui` gates.
    fn publish_parent_session_anchor(&self) {
        if !self.install_watcher {
            return;
        }
        if let Some(services) = self.host_services.get()
            && let Some(id) = services.session_id()
        {
            cyrup_ext_subagents::publish_parent_session_anchor(&id);
        }
    }
}

/// Build the [`DedupDetails`] fingerprint inputs (pi `PermissionPromptDetails`, `index.ts:713-726`).
/// `message` is the live prompt (pi `details.message = formatAskPrompt(...)`), so a re-emitted
/// identical `tool_call` fingerprints the same and reuses the cached decision.
fn dedup_details(
    call_id: &str,
    input: &Value,
    check: &PermissionCheckResult,
    agent_name: Option<&str>,
) -> DedupDetails {
    DedupDetails {
        request_id: call_id.to_string(),
        source: source_str(check.source).to_string(),
        agent_name: agent_name.map(str::to_string),
        message: gate::format_ask_prompt(check, agent_name, input),
        tool_call_id: Some(call_id.to_string()),
        tool_name: Some(check.tool_name.clone()),
        skill_name: None,
        path: gate::get_path_bearing_tool_path(&check.tool_name, input),
        command: check.command.clone(),
        target: check.target.clone(),
        tool_input: input.clone(),
    }
}

fn source_str(s: CheckSource) -> &'static str {
    match s {
        CheckSource::Tool => "tool",
        CheckSource::Bash => "bash",
        CheckSource::Mcp => "mcp",
        CheckSource::Skill => "skill",
        CheckSource::Special => "special",
        CheckSource::Default => "default",
    }
}

/// Lock a `Mutex`, recovering from poison rather than panicking (no-panic policy). Held only across
/// synchronous sections — never across an `.await`.
fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// pi `resolveAgentName` (`index.ts:2033-2047`) for cyrup's process-per-subagent model: the resolved
/// persona name this process was spawned as, read from the `CYRUP_SUBAGENT_AGENT_NAME` env var
/// (`cyrup_ext_subagents::AGENT_NAME_ENV_VAR`) — captured ONCE (the child IS its persona for its whole
/// lifetime), the exact equivalent of pi's in-process `active_agent` session entry / `<active_agent>`
/// prompt tag for a separate-process subagent. Trimmed; empty/absent → `None` (pi `normalizeAgentName`
/// null-normalization + the normalized-`""` top-level: a top-level process has no such var, so the
/// agent + projectAgent layers no-op and global + project still enforce). This is the SAME
/// `std::env::var` pattern the crate already uses for the sibling `CYRUP_SUBAGENT_*` anchors
/// (`ask.rs` `PARENT_SESSION_ENV_VAR`, `is_subagent_child`).
fn resolve_agent_name_from_env() -> Option<String> {
    std::env::var(cyrup_ext_subagents::AGENT_NAME_ENV_VAR)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[async_trait]
impl NativeExtension for PermissionSystemExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// P-1 (reconciliation §2 item 1): capture the late-bound live capability backend BEFORE `init`
    /// (the builder threads its `LiveHostServices` via `load_native_with_services`). The in-session
    /// `ask` dialog (`resolve_ask`) prompts through it via [`LocalAskChannel`]. Set-once; a second
    /// bind is ignored (the session's backend is stable).
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.host_services.set(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        // ToolCall is the deciding gate (honored by the block/mutate dispatcher `ExtHooks` drives).
        // BeforeAgentStart runs the context-hygiene shaping (pi `index.ts:2134-2190`): it shapes the
        // active tool set (`setActiveTools`), sanitizes the system prompt (tools section + denied
        // guideline bullets, and hides ask/deny skills while caching the skill-read enforcement entries,
        // pi `:2174-2176`), and syncs the yolo pill — returning the sanitized prompt as a `[mutate]`.
        // Input captures `/skill:<name>` explicit requests (pi `:2192-2206`). Session{Start,Shutdown}
        // clear the in-session store + dedup + skill state (and set/clear the status pill).
        // ResourcesDiscover runs pi's `resources_discover` reload branch (`index.ts:2103-2118`):
        // re-reads `config.json`, rebuilds the `PermissionManager` from the current cwd, and
        // invalidates the agent-start cache. No LLM-visible tool/command is registered — the gate is
        // invisible to the model (pi none).
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::BeforeAgentStart,
            EventKind::Input,
            EventKind::SessionStart,
            EventKind::SessionShutdown,
            EventKind::ResourcesDiscover,
        ]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ToolCall { call_id, name, input } => {
                // PERM-005 / pi `tool_call` (`index.ts:2210`): every tool call re-enters
                // `startForwardedPermissionPolling`, so a watcher that could not attach at session
                // start (unresolved session id, UI attached late) is armed here instead of never.
                // Idempotent — see `maybe_start_forwarding_watcher`.
                self.maybe_start_forwarding_watcher(ctx);
                self.decide(call_id.as_str(), name, input, ctx).await
            }
            HostEvent::BeforeAgentStart { system_prompt, .. } => {
                // pi `before_agent_start` (`index.ts:2134-2190`): shape the active tool set
                // (`setActiveTools`), sanitize the system prompt (tools section + denied guideline
                // bullets, and hide ask/deny skills while caching the enforcement entries the skill-read
                // gate reads at every `tool_call`), and sync the yolo status pill — returning the
                // sanitized prompt as a `[mutate]`.
                //
                // PERM-005 / pi `before_agent_start` (`index.ts:2137`): re-enter
                // `startForwardedPermissionPolling` first, so each turn re-arms the forwarding
                // watcher (and tears it down if the UI has gone away). Idempotent.
                self.maybe_start_forwarding_watcher(ctx);
                self.on_before_agent_start(system_prompt, ctx)
            }
            HostEvent::Input { text, .. } => {
                // PERM-005 / pi `input` (`index.ts:2194`): re-enter `startForwardedPermissionPolling`
                // on every user turn. Idempotent.
                self.maybe_start_forwarding_watcher(ctx);
                // pi `index.ts:2192-2206`: a `/skill:<name>` slash command is a direct user action —
                // remember it so its skill-file reads bypass the skill-read ask/deny (pi `:2243`).
                if let Some(name) = skill::extract_skill_name_from_input(text) {
                    guard(&self.explicitly_requested_skill_names).insert(name);
                }
                HookOutcome::Noop
            }
            HostEvent::SessionStart { .. } => {
                // pi `index.ts:2089,2092`: clear session store + dedup + explicit-skill set; refresh.
                guard(&self.session_approvals).clear();
                guard(&self.dedup).clear();
                guard(&self.explicitly_requested_skill_names).clear();
                // pi `resetShownWarnings()` (`index.ts:2079`, the first statement of
                // `refreshSessionRuntimeState`): a new session re-arms every load warning, so a
                // still-malformed policy file is reported again rather than staying suppressed by
                // the previous session's dedup set.
                self.warnings.reset();
                // pi `refreshSessionRuntimeState` (`index.ts:2077-2085`, called unconditionally from
                // every `session_start`): re-read `config.json` from disk and rebuild the
                // `PermissionManager`'s policy paths from the CURRENT session `ctx.cwd` (not just the
                // process's original cwd) — a session can start in a different working directory than
                // the one the extension was constructed with. Also invalidates the agent-start cache
                // (clears `active_skill_entries`), superseding the plain clear this arm did before.
                self.refresh_config_and_manager(&ctx.cwd);
                // PERM-001 / pi `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`
                // (`pi-subagents/src/extension/index.ts:599` @v0.34.0): publish this parent session's id as
                // the process-wide anchor a subagent child's forwarded ask addresses, BEFORE the
                // watcher that services those asks starts. Without it the detached background hop
                // spawns children with no anchor and every one of their asks fail-closed denies.
                self.publish_parent_session_anchor();
                // pi `startForwardedPermissionPolling` via `refreshSessionRuntimeState`
                // (`index.ts:2084`): in the PARENT role, on a session WITH a UI, spawn the forwarding
                // watcher (a detached tokio task, OUTSIDE the 5s dispatch budget) that services
                // subagent children's forwarded asks. This is the FIRST of four re-entry points
                // (PERM-005) — see the `BeforeAgentStart` / `Input` / `ToolCall` arms.
                self.maybe_start_forwarding_watcher(ctx);
                // pi `syncPermissionSystemStatus` (`index.ts:2091` via `refreshSessionRuntimeState`):
                // reflect the yolo pill on the live status bar at session start.
                if let Some(s) = self.host_services.get() {
                    status::sync_status(s, &guard(&self.config));
                }
                HookOutcome::Noop
            }
            HostEvent::ResourcesDiscover => {
                // pi `pi.on("resources_discover", ...)` reload branch (`index.ts:2103-2118`): reset the
                // dedup cache, re-read `config.json`, rebuild the `PermissionManager` from the current
                // cwd, and invalidate the agent-start cache. Cyrup's `HostEvent::ResourcesDiscover`
                // carries no `reason` field (unlike pi's event), so every dispatch is treated as the
                // "reload" case — the only variant this host event exposes.
                guard(&self.dedup).clear();
                // pi `resetShownWarnings()` (`index.ts:2105`, the reload branch's first statement).
                self.warnings.reset();
                self.refresh_config_and_manager(&ctx.cwd);
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { .. } => {
                // pi `index.ts:2122,2123,2128,2130,2131`: clear the status pill + stores + dedup + skill
                // state; tear down watcher.
                if let Some(s) = self.host_services.get() {
                    status::clear_status(s);
                }
                guard(&self.session_approvals).clear();
                guard(&self.dedup).clear();
                guard(&self.explicitly_requested_skill_names).clear();
                guard(&self.active_skill_entries).clear();
                // pi `resetShownWarnings()` (`index.ts:2125`).
                self.warnings.reset();
                self.stop_forwarding_watcher();
                // PERM-001 / pi `delete process.env[SUBAGENT_PARENT_SESSION_ENV]`
                // (`pi-subagents/src/extension/index.ts:619` @v0.34.0): drop the published anchor so a stale
                // id from the session that just ended never addresses a subsequently-started
                // session's spool on this same long-lived process. PARENT role only, symmetric with
                // `publish_parent_session_anchor` — a CHILD never published and must not clear the
                // anchor its own descendants still need.
                if self.install_watcher {
                    cyrup_ext_subagents::clear_parent_session_anchor();
                }
                HookOutcome::Noop
            }
            _ => HookOutcome::Noop,
        }
    }
}

// ================================================================================= binary wiring

/// pi `hasSubagentEnvHint` (`index.ts:93-103`, over
/// `permission-forwarding.ts:9`'s `SUBAGENT_ENV_HINT_KEYS`): this process is running AS a subagent
/// child if ANY of the hint keys is set to a non-empty (post-trim) value.
///
/// Three deliberate points of fidelity to upstream:
///
/// - **Any of three keys, not one.** pi ORs `PI_IS_SUBAGENT` / `PI_SUBAGENT_SESSION_ID` /
///   `PI_AGENT_ROUTER_SUBAGENT`; [`SUBAGENT_ENV_HINT_KEYS`] is the cyrup analog set, all three
///   written by the same spawn chokepoint (`exec::build_attempt_spawn_plan`).
/// - **Non-empty, not `== "1"`.** pi tests `entry.length > 0`. The old strict `== Some("1")`
///   silently classified a child spawned by any path that wrote a different truthy value (or by an
///   external router setting only the persona/run keys) as a ROOT — which selected the LOCAL ask
///   dialog in a process with no human attached, so its `ask` died instead of forwarding.
/// - **Trimmed.** pi's `process.env[key]?.trim() ?? ""`.
///
/// Not ported: pi's `subagent-sessions` session-directory containment fallback
/// (`index.ts:696-709`). That branch keys on pi's in-process subagent sessions living under a
/// dedicated directory of the agent dir; cyrup's subagent is always a separate OS process carrying
/// these env keys (`lib.rs`'s non-negotiable process-per-subagent mechanism), so there is no
/// same-process session-dir signal to test. Note also that pi's `isSubagentExecutionContext` is a
/// per-`ctx` RUNTIME predicate while this is consulted both at wiring time
/// ([`permission_extension_for_env`]) and per call — the env keys are process-lifetime constants in
/// cyrup, so the two coincide.
fn is_subagent_child() -> bool {
    has_subagent_env_hint(|key| std::env::var(key).ok())
}

/// The injectable core of [`is_subagent_child`] — pi `hasSubagentEnvHint`'s body
/// (`index.ts:100`, `values.some((entry) => entry.length > 0)` over the trimmed values).
///
/// Parameterized over the env reader so the predicate is directly testable without
/// `unsafe { std::env::set_var }` and the cross-test races a process-global mutation brings, the
/// same injectable-core convention `cyrup-ext-subagents`' `spawn::depth`/`spawn::mod` use.
///
/// Not ported: pi caches the answer keyed on a `\0`-joined signature of the values
/// (`index.ts:94-102`). That cache exists because pi re-evaluates this on every `ctx` predicate
/// call inside a hot per-tool-call path; cyrup consults it at wiring time plus once per ask, over
/// three `getenv`s, so a cache would be pure complexity — and a stale one is a correctness hazard.
fn has_subagent_env_hint(get: impl Fn(&str) -> Option<String>) -> bool {
    SUBAGENT_ENV_HINT_KEYS
        .iter()
        .any(|key| get(key).is_some_and(|value| !value.trim().is_empty()))
}

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// DI-5 "installed" detection (opt-in): the gate attaches only when the user has installed it —
/// either the [`INSTALL_ENV_VAR`] is truthy, or a policy file exists, or the extension config has
/// been edited away from its auto-generated template. NOT installed → zero gating (unchanged core
/// behavior); installed → default-ASK per category (faithful to pi `permission-manager.ts:44-50`).
/// This keeps the crate compiled + wired at all three sites while never bricking the default
/// (policy-less) app with fail-closed asks on every tool.
///
/// **Every install signal is reversible** (PERM-002). Before this, the probe accepted the bare
/// EXISTENCE of `<agent_dir>/cyrup-permission-system/config.json` — but that file is written by
/// this crate itself, unconditionally, as a side effect of constructing the extension
/// ([`ExtensionConfig::ensure_on_disk`] via [`PermissionSystemExtension::derive_parts`]). So a
/// single `CYRUP_PERMISSION_SYSTEM=1` run left a permanent artifact behind that kept the gate
/// armed forever after, with no supported way to turn it back off: unsetting the env var did
/// nothing, and the file silently reappeared on the very next run if deleted.
///
/// The chosen semantics: `config.json` counts as an install signal only once its bytes DIFFER
/// from the pristine template ([`ExtensionConfig::is_pristine_default_file`]) — i.e. once a human
/// actually configured something. Both directions of the security argument are covered:
/// - It cannot silently DISABLE a gate an operator intended. The env var is untouched; both
///   policy paths are files only a human writes; and a hand-authored (therefore non-pristine)
///   `config.json` still installs, so an operator whose only install signal was that file keeps
///   the gate. An unreadable `config.json` is likewise treated as configured (fail-safe).
/// - It cannot leave an operator PERMANENTLY stuck with a gate they never asked for: the only
///   case it newly returns `false` is the untouched, machine-written template, where no policy
///   file and no env opt-in exist either — a state in which the manager would have had no rules
///   at all and merely defaulted every category to `ask`.
///
/// Upstream `pi-permission-system` has no "installed" probe to copy (the extension gates whatever
/// loads it); its v0.8.0 answer to "how do I turn this off" is a separate `"enabled": false`
/// master switch in `config.json` (`extension-config.ts:12,88` → `index.ts:1475`), which is
/// tracked as its own port item and is complementary to — not a substitute for — un-latching this
/// probe.
#[must_use]
pub fn is_installed(agent_dir: &Path, cwd: &Path) -> bool {
    if env_truthy(INSTALL_ENV_VAR) {
        return true;
    }
    let project_dir = PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg));
    if [agent_dir.join(POLICY_FILE), project_dir.join(POLICY_FILE)].iter().any(|p| p.exists()) {
        return true;
    }
    let config_path = PermissionSystemExtension::config_path_for(agent_dir);
    config_path.exists() && !ExtensionConfig::is_pristine_default_file(&config_path)
}

/// The binary-side wiring entry point `crates/cyrup/src/main.rs` calls at each of its three
/// session-build sites (mirrors `cyrup_ext_subagents::extension::subagent_extension_for_env`).
///
/// Role is selected by the `CYRUP_SUBAGENT_CHILD` / depth signal (port doc §3.1 item 4, pi's `hasUI`
/// vs `isSubagentExecutionContext` split, `index.ts:1506-1519`):
/// - **CHILD** (`CYRUP_SUBAGENT_CHILD`): loads the gate with a [`ForwardingAskChannel`]
///   ([`PermissionSystemExtension::new_forwarding_child`]) — an ask-tier decision FORWARDS up to the
///   parent's human via the spool instead of dying. (P-4; previously this returned `None`, leaving a
///   child's `ask` with no reachable human — the exact gap this build closes.)
/// - **PARENT** (root, `DEPTH == 0`): loads the gate with the [`LocalAskChannel`] in-session dialog +
///   the forwarding WATCHER ([`PermissionSystemExtension::new_forwarding_parent`]).
///
/// Returns `None` (attach nothing → DI-5 zero gating) only when the gate is not installed.
#[must_use]
pub fn permission_extension_for_env(
    agent_dir: PathBuf,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    if !is_installed(&agent_dir, &cwd) {
        return None;
    }
    if is_subagent_child() {
        // CHILD: forward asks up to the parent (§7.4). The parent-session anchor
        // `CYRUP_SUBAGENT_PARENT_SESSION` (emitted by `cyrup-ext-subagents`, `exec/mod.rs`
        // `PARENT_SESSION_ENV_VAR`) addresses the parent's inbox; the `ForwardingAskChannel` reads it.
        return Some(Arc::new(PermissionSystemExtension::new_forwarding_child(agent_dir, cwd)));
    }
    // PARENT: in-session dialog + the forwarding watcher (installed on SessionStart).
    Some(Arc::new(PermissionSystemExtension::new_forwarding_parent(agent_dir, cwd)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn not_installed_without_policy_or_env_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        // No policy file, env not set → DI-5 zero gating. Explicitly sandbox (save/clear/restore)
        // `INSTALL_ENV_VAR` for this assertion: it is the same opt-in env var
        // `permission_extension_for_env` reads in production, and a developer/CI shell that has
        // genuinely opted in workspace-wide (exactly as this crate's own module doc documents,
        // "opt-in per DI-5") would otherwise make this "no opt-in" case flake on ambient state
        // that has nothing to do with the code path under test. No other test in this crate reads
        // or writes `INSTALL_ENV_VAR`, so this scoped mutation cannot race a sibling test.
        let previous = std::env::var(INSTALL_ENV_VAR).ok();
        // SAFETY: scoped to this test; restored immediately below before any other assertion runs.
        unsafe {
            std::env::remove_var(INSTALL_ENV_VAR);
        }
        assert!(!is_installed(dir.path(), dir.path()));
        // SAFETY: restores whatever the ambient shell had (or leaves it unset), symmetric with the
        // removal above.
        unsafe {
            match previous {
                Some(v) => std::env::set_var(INSTALL_ENV_VAR, v),
                None => std::env::remove_var(INSTALL_ENV_VAR),
            }
        }
    }

    #[test]
    fn installed_when_policy_file_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(POLICY_FILE), "{}").unwrap();
        assert!(is_installed(dir.path(), dir.path()));
    }

    // ============================================================================================
    // Wave1b pi-parity regression tests (dossier: cyrup-permission-system/src/extension.rs).
    // ============================================================================================

    /// A scripted [`HostServices`] whose ONLY override is `all_tool_names` — the full registry the
    /// registry / unknown-tool gate checks against (pi `pi.getAllTools()`). Mirrors the identical
    /// helper in `tests/layers_wired.rs`.
    struct FakeRegistry {
        names: Vec<String>,
    }
    impl HostServices for FakeRegistry {
        fn all_tool_names(&self) -> Option<Vec<String>> {
            Some(self.names.clone())
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn event_ctx(cwd: PathBuf) -> HostCtx {
        HostCtx::event(cyrup_ext::ExtMode::Print, false, cwd)
    }

    async fn init_ext(ext: &PermissionSystemExtension) {
        let mut api = InitApi::new();
        ext.init(&mut api).await.unwrap();
    }

    /// Drive `body` to completion with the crate-wide env lock held for the WHOLE test.
    ///
    /// Any test that asserts on config it wrote into its own tempdir must take this lock.
    /// `ExtensionConfig::load` resolves its path through `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`
    /// first ([`crate::ext_config::ExtensionConfig::resolve_config_path`]), and
    /// `ext_config::tests::env_var_overrides_default_config_path` sets that variable PROCESS-WIDE
    /// while it runs. A concurrent test then loads the OTHER test's fixture instead of its own and
    /// fails on an assertion that has nothing to do with the code under test. Measured on this
    /// binary: 8 failures in 300 runs before this guard, 0 in 300 after; and exporting the variable
    /// by hand reproduces the same failure 100% of the time.
    ///
    /// The lock is `crate::ext_config::env_lock` — the same one the mutator holds — and it is taken
    /// in a SYNCHRONOUS frame around `block_on` rather than inside an `async` test body, so the
    /// guard is never held across an `.await` point.
    fn with_config_env_lock<F: std::future::Future>(body: F) -> F::Output {
        let _lock =
            crate::ext_config::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(body)
    }

    fn bash_call(call_id: &str) -> HostEvent {
        HostEvent::ToolCall {
            call_id: cyrup_core::ToolCallId::from(call_id),
            name: "bash".to_string(),
            input: json!({ "command": "echo hi" }),
        }
    }

    /// pi `resources_discover` reload branch (`index.ts:2103-2118`): re-reads `config.json` and
    /// invalidates the agent-start cache. BEFORE this fix, `EventKind::ResourcesDiscover` was never
    /// subscribed and `on_event` fell through to its catch-all `Noop` arm, so neither the config nor
    /// the cached skill-enforcement entries ever refreshed — this test fails against that behavior.
    #[test]
    fn resources_discover_reloads_config_and_invalidates_skill_cache() {
        with_config_env_lock(resources_discover_reloads_config_and_invalidates_skill_cache_body());
    }

    async fn resources_discover_reloads_config_and_invalidates_skill_cache_body() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;

        // The constructor auto-materializes the default config (yolo_mode: false).
        assert!(!guard(&ext.config).yolo_mode, "default config starts with yolo off");

        // Seed the agent-start skill cache as `before_agent_start` would.
        *guard(&ext.active_skill_entries) = vec![SkillPromptEntry {
            name: "demo".into(),
            state: PermissionState::Ask,
            normalized_location: "/skills/demo".into(),
            normalized_base_dir: "/skills".into(),
        }];

        // Flip yoloMode on disk directly (simulating an external edit to config.json betwen the
        // extension's construction and a later `resources_discover` reload).
        write_file(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE), r#"{ "yoloMode": true }"#);

        let outcome = ext.on_event(&HostEvent::ResourcesDiscover, &event_ctx(agent_dir)).await;
        assert!(matches!(outcome, HookOutcome::Noop));

        assert!(guard(&ext.config).yolo_mode, "resources_discover reload must re-read config.json");
        assert!(
            guard(&ext.active_skill_entries).is_empty(),
            "resources_discover reload must invalidate the agent-start skill cache"
        );
    }

    /// pi `refreshSessionRuntimeState` (`index.ts:2077-2085`): every `session_start` unconditionally
    /// re-derives `permissionManager`'s policy paths from the CURRENT session `ctx.cwd`. BEFORE this
    /// fix `self.manager` was frozen at construction time and never re-derived on `session_start`, so
    /// a session starting under a DIFFERENT project directory never picked up that project's policy
    /// override — this test fails against that behavior.
    #[tokio::test]
    async fn session_start_rebuilds_manager_from_current_session_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        // Global policy: bash is allowed everywhere by default.
        write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "allow" } }"#);

        // The extension is CONSTRUCTED against `cwd1`, which has no project-level override.
        let cwd1 = dir.path().join("cwd1");
        std::fs::create_dir_all(&cwd1).unwrap();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), cwd1);
        init_ext(&ext).await;
        ext.set_host_services(Arc::new(FakeRegistry { names: vec!["bash".to_string()] }));

        // A NEW session starts under `cwd2`, which HAS a project-scoped override denying bash.
        let cwd2 = dir.path().join("cwd2");
        std::fs::create_dir_all(&cwd2).unwrap();
        write_file(
            &PROJECT_AGENT_SUBDIR.iter().fold(cwd2.clone(), |acc, seg| acc.join(seg)).join(POLICY_FILE),
            r#"{ "bash": { "*": "deny" } }"#,
        );

        let start_ctx = event_ctx(cwd2);
        let start_outcome =
            ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &start_ctx).await;
        assert!(matches!(start_outcome, HookOutcome::Noop));

        // A bash call now, under `cwd2`, must be DENIED by the project override the rebuilt manager
        // picked up — proving the manager was rebuilt against the CURRENT session cwd, not left
        // stale against `cwd1`.
        let outcome = ext.on_event(&bash_call("call-1"), &start_ctx).await;
        assert!(
            matches!(outcome, HookOutcome::Block { .. }),
            "the cwd2 project-scoped deny must enforce once session_start rebuilds the manager"
        );
    }

    /// pi `checkRequestedToolRegistration(toolName, pi.getAllTools())` (`index.ts:2218-2228`) runs
    /// UNCONDITIONALLY — pi has no skip path. BEFORE this fix, when the live registry could not be
    /// enumerated (no `HostServices` attached), cyrup silently SKIPPED the registry gate entirely,
    /// letting ANY tool name through the allowlist with zero enforcement — this test fails against
    /// that fail-open behavior.
    #[tokio::test]
    async fn registry_gate_fails_closed_with_no_attached_registry() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        // Global policy allows bash everywhere — a fail-OPEN gate would let this proceed (Noop).
        write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "allow" } }"#);
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        // Deliberately never call `set_host_services` — the registry cannot be enumerated.

        let outcome = ext.on_event(&bash_call("call-1"), &event_ctx(agent_dir)).await;
        assert!(
            matches!(outcome, HookOutcome::Block { .. }),
            "an unenumerable registry must fail CLOSED, never silently let the tool through"
        );
    }

    /// pi `canResolveAskPermissionRequest` (`yolo-mode.ts:21-23`), consulted via
    /// `canRequestPermissionConfirmation` BEFORE any prompt/lock work (`index.ts:2263,2351,2452`):
    /// `hasUI || isSubagent || yoloMode`. BEFORE this fix `prompt_decision` always attempted the
    /// human-interaction lock + channel selection whenever a live backend was attached, even when
    /// none of the three conditions held — this test fails against that behavior (it would hang/lock
    /// against a live backend instead of failing closed immediately).
    #[tokio::test]
    async fn ask_fails_fast_without_ui_subagent_or_yolo() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "ask" } }"#);
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        ext.set_host_services(Arc::new(FakeRegistry { names: vec!["bash".to_string()] }));

        // has_ui=false, no `CYRUP_SUBAGENT_CHILD` env, config.yolo_mode default false ⇒ the
        // pre-check must fail CLOSED immediately, never touching the lock/dialog machinery.
        let outcome = ext.on_event(&bash_call("call-1"), &event_ctx(agent_dir)).await;
        assert!(matches!(outcome, HookOutcome::Block { .. }));
    }

    // ============================================================================================
    // PERM-002 / PERM-003 regression tests.
    // ============================================================================================

    /// Run `body` with [`INSTALL_ENV_VAR`] guaranteed unset, restoring the ambient value after —
    /// serialized against every other env-mutating test in the crate by the shared
    /// [`crate::ext_config::env_lock`].
    fn without_install_env<T>(body: impl FnOnce() -> T) -> T {
        let _lock = crate::ext_config::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var(INSTALL_ENV_VAR).ok();
        // SAFETY: serialized by `env_lock`, and restored below before the guard drops.
        unsafe { std::env::remove_var(INSTALL_ENV_VAR) };
        let out = body();
        // SAFETY: same scope/serialization; restores whatever the ambient shell had.
        unsafe {
            match previous {
                Some(v) => std::env::set_var(INSTALL_ENV_VAR, v),
                None => std::env::remove_var(INSTALL_ENV_VAR),
            }
        }
        out
    }

    /// PERM-002. Merely CONSTRUCTING the extension materializes
    /// `<agent_dir>/cyrup-permission-system/config.json` on disk (`ExtensionConfig::ensure_on_disk`),
    /// and `is_installed` used to accept that file's bare existence as an install signal. So one
    /// `CYRUP_PERMISSION_SYSTEM=1` run permanently latched the gate on for every later run in that
    /// agent dir, with no way to turn it back off — deleting the file did not help either, because
    /// the next construction re-created it.
    ///
    /// Observable contract, all three directions in one test:
    ///  1. after a full construct-and-materialize cycle with no env opt-in and no policy file,
    ///     `is_installed` is false again and `permission_extension_for_env` attaches NOTHING;
    ///  2. an operator-edited `config.json` still installs (the fix cannot silently disable a gate
    ///     whose only install signal was a hand-written config);
    ///  3. a policy file still installs, unaffected.
    #[test]
    fn auto_materialized_config_does_not_latch_the_gate_on() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);

            assert!(!is_installed(&agent_dir, &cwd), "clean agent dir must not be installed");

            // The opt-in run: build the extension exactly as the binary wiring does. This is the
            // step that writes `config.json`.
            let installed = PermissionSystemExtension::new(agent_dir.clone(), cwd.clone());
            drop(installed);
            assert!(
                config_path.exists(),
                "constructing the extension must still materialize the editable config template"
            );

            // The NEXT run, with the env opt-in gone and no policy file anywhere. The leftover
            // template is the extension's own footprint, not an operator decision.
            assert!(
                !is_installed(&agent_dir, &cwd),
                "the auto-written config template must not latch the gate on for every later run"
            );
            assert!(
                permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none(),
                "an un-opted-in run must attach no gate at all"
            );

            // An operator who configures the extension by hand IS opting in — that signal must
            // survive, or un-latching would have become a way to silently disable a real gate.
            std::fs::write(&config_path, "{\n  \"yoloMode\": true\n}\n").unwrap();
            assert!(
                is_installed(&agent_dir, &cwd),
                "a hand-edited config.json must still install the gate"
            );

            // ...and reverting it to the pristine template turns it back off: the switch is
            // two-way, which is the whole point.
            std::fs::write(&config_path, ExtensionConfig::default_config_content()).unwrap();
            assert!(!is_installed(&agent_dir, &cwd), "reverting the config must turn the gate off");

            // A policy file remains an install signal regardless of the config file.
            write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "deny" } }"#);
            assert!(is_installed(&agent_dir, &cwd), "a policy file must still install the gate");
        });
    }

    /// A [`HostServices`] double that enumerates a registry (so the unknown-tool gate lets the call
    /// through to the policy engine) AND records every `notify` the extension pushes at the host.
    struct NotifyRecorder {
        names: Vec<String>,
        notifications: Mutex<Vec<(String, NotifyKind)>>,
    }

    impl NotifyRecorder {
        fn new() -> Self {
            Self { names: vec!["bash".to_string()], notifications: Mutex::new(Vec::new()) }
        }
        fn warnings(&self) -> Vec<String> {
            guard(&self.notifications)
                .iter()
                .filter(|(_, kind)| *kind == NotifyKind::Warning)
                .map(|(message, _)| message.clone())
                .collect()
        }
    }

    impl HostServices for NotifyRecorder {
        fn all_tool_names(&self) -> Option<Vec<String>> {
            Some(self.names.clone())
        }
        fn notify(&self, message: &str, kind: NotifyKind) {
            guard(&self.notifications).push((message.to_string(), kind));
        }
    }

    /// PERM-003. pi threads `notifyWarning` into EVERY `PermissionManager` it builds
    /// (`createPermissionManagerForCwd(cwd, notifyWarning)`, `index.ts:1595,2081,2109-2110`) and
    /// into `refreshExtensionConfig` (`index.ts:1614`), so a policy or config file that exists but
    /// does not parse reaches the human as a `warning` notification.
    ///
    /// BEFORE this fix `PermissionManager::with_on_warning` had no caller outside this crate's own
    /// unit tests and `refresh_config_and_manager` used the warning-discarding `ExtensionConfig::
    /// load`, so both failures degraded in TOTAL SILENCE: a typo'd `cyrup-permissions.jsonc` fell
    /// back to "ask everything", which looks exactly like a policy that genuinely says ask. This
    /// test drives a real session lifecycle + tool call and asserts the messages actually arrive at
    /// the host boundary.
    #[test]
    fn malformed_policy_and_config_files_notify_the_host() {
        with_config_env_lock(malformed_policy_and_config_files_notify_the_host_body());
    }

    async fn malformed_policy_and_config_files_notify_the_host_body() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        // Present, but truncated mid-object: exists (so it is not the silent ENOENT case) and does
        // not parse.
        write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "allow" "#);
        write_file(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE), "{ not json");

        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let host = Arc::new(NotifyRecorder::new());
        ext.set_host_services(host.clone());

        let ctx = event_ctx(agent_dir.clone());
        let start = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &ctx).await;
        assert!(matches!(start, HookOutcome::Noop));
        // A real tool call is what forces the policy layers to be read.
        let _ = ext.on_event(&bash_call("call-1"), &ctx).await;

        let warnings = host.warnings();
        assert!(
            warnings.iter().any(|w| w.starts_with("Failed to parse permission config at")
                && w.contains(POLICY_FILE)
                && w.ends_with("using ask fallback.")),
            "the unparseable policy file must reach the host as a warning; got {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.starts_with("Failed to parse permission-system config at")
                && w.ends_with("using default extension config.")),
            "the unparseable extension config must reach the host as a warning; got {warnings:?}"
        );

        // pi `shownWarnings` (`index.ts:1573,1586-1592`): each distinct message is reported at most
        // once per session, so a reload storm cannot spam the user. Re-running the whole refresh +
        // tool-call cycle must not duplicate anything already shown.
        let before = warnings.len();
        let _ = ext.on_event(&bash_call("call-2"), &ctx).await;
        assert_eq!(host.warnings().len(), before, "warnings must be deduped within a session");

        // ...and a NEW session re-arms them (pi `resetShownWarnings`, `index.ts:2079`), so a file
        // that is still broken is reported again rather than silently suppressed forever.
        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &ctx).await;
        let _ = ext.on_event(&bash_call("call-3"), &ctx).await;
        assert!(
            host.warnings().len() > before,
            "a new session must re-report a still-broken file; got {:?}",
            host.warnings()
        );
    }
    // ------------------------------------------------------------ PERM-001: subagent env hints

    /// PERM-001 (second gap). pi ORs the three [`SUBAGENT_ENV_HINT_KEYS`] on ANY non-empty value
    /// (`index.ts:93-103`, `permission-forwarding.ts:9`). The pre-fix predicate was a strict
    /// `std::env::var(CHILD_ENV_VAR) == Some("1")` on ONE key, so every case below except the very
    /// first classified a real subagent child as a ROOT — which wires the LOCAL ask dialog into a
    /// process with no human attached, and its `ask` dies there instead of forwarding to the
    /// parent's spool.
    #[test]
    fn subagent_env_hint_ors_any_non_empty_value_across_all_three_keys() {
        let hint = |pairs: &[(&str, &str)]| {
            let owned: Vec<(String, String)> =
                pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
            has_subagent_env_hint(|key| {
                owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
            })
        };

        // The one case the old strict `== Some("1")` predicate already got right.
        assert!(hint(&[(CHILD_ENV_VAR, "1")]));

        // pi tests `entry.length > 0`, not equality with "1" — any non-empty value is a hint.
        assert!(hint(&[(CHILD_ENV_VAR, "0")]));
        assert!(hint(&[(CHILD_ENV_VAR, "true")]));
        assert!(hint(&[(CHILD_ENV_VAR, "yes")]));

        // ...and either of the two sibling keys alone is enough (pi's OR over three keys).
        assert!(hint(&[(SUBAGENT_ENV_HINT_KEYS[1], "run-abc123")]));
        assert!(hint(&[(SUBAGENT_ENV_HINT_KEYS[2], "reviewer")]));

        // Negatives: nothing set, and set-but-blank (pi trims before the length test).
        assert!(!hint(&[]));
        assert!(!hint(&[(CHILD_ENV_VAR, "")]));
        assert!(!hint(&[(CHILD_ENV_VAR, "   ")]));
        assert!(!hint(&[
            (CHILD_ENV_VAR, ""),
            (SUBAGENT_ENV_HINT_KEYS[1], "  "),
            (SUBAGENT_ENV_HINT_KEYS[2], "\t"),
        ]));

        // An unrelated var is never a hint.
        assert!(!hint(&[("CYRUP_SOMETHING_ELSE", "1")]));
    }

    /// The hint keys are exactly the strings `cyrup-ext-subagents` writes into a child's spawn
    /// overlay. Pinned so a rename on either side fails here rather than silently producing a gate
    /// that never recognizes a child (aliasing already prevents drift for two of the three; this
    /// pins the resulting VALUES, which are also the cross-crate contract).
    #[test]
    fn subagent_env_hint_keys_match_the_spawn_overlay_contract() {
        assert_eq!(
            SUBAGENT_ENV_HINT_KEYS,
            ["CYRUP_SUBAGENT_CHILD", "CYRUP_SUBAGENT_RUN_ID", "CYRUP_SUBAGENT_AGENT_NAME"]
        );
        assert_eq!(CHILD_ENV_VAR, SUBAGENT_ENV_HINT_KEYS[0]);
    }

    /// PERM-001 (first gap), the publisher half: a PARENT-role extension publishes its live session
    /// id into `cyrup-ext-subagents`' process-wide anchor register on `SessionStart` (pi
    /// `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `pi-subagents/src/extension/
    /// index.ts:599` @v0.34.0) and clears it on `SessionShutdown` (`:619`), so the hop-1 detached spawn has
    /// an anchor to overlay onto the background runner. Before this, nothing in the workspace ever
    /// published the root's id anywhere a spawn could read it, and the detached path resolved an
    /// empty target on every hop.
    ///
    /// The anchor register (`cyrup_ext_subagents::background::parent_anchor`) is PROCESS-global and
    /// cargo runs this crate's unit tests as parallel threads of one process, so every test that
    /// mutates it must hold this lock for its whole body. (This module used to carry a single
    /// anchor test for exactly that reason — "one test, not several". A lock is the honest version
    /// of that constraint, and lets the CHILD-role gate below be its own test rather than an
    /// appendix to the PARENT-role one. Mirrors `parent_anchor.rs`'s own `REGISTER_LOCK`.)
    ///
    /// A `tokio::sync::Mutex`, not a `std` one: every holder below is an `async` test that awaits
    /// `on_event` while holding the guard, and a `std::sync::MutexGuard` held across an await point
    /// is `clippy::await_holding_lock`. (`parent_anchor.rs`'s `REGISTER_LOCK` can be a `std` mutex
    /// because its tests are synchronous.) It also drops the poison handling `std` would force at
    /// every call site — a tokio mutex has no poisoning, so a panicking test releases the lock
    /// cleanly instead of leaving siblings to recover from a `PoisonError`.
    static ANCHOR_REGISTER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A [`HostServices`] whose only override is a fixed `session_id` — the single input
    /// `publish_parent_session_anchor` reads.
    struct AnchorHost(&'static str);
    impl HostServices for AnchorHost {
        fn session_id(&self) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[tokio::test]
    async fn parent_role_publishes_and_clears_the_process_parent_session_anchor() {
        let _guard = ANCHOR_REGISTER_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");

        let ext = PermissionSystemExtension::new_forwarding_parent(
            agent_dir.clone(),
            dir.path().to_path_buf(),
        );
        ext.set_host_services(Arc::new(AnchorHost("session-root-perm001")));
        let ctx = event_ctx(dir.path().to_path_buf());

        cyrup_ext_subagents::clear_parent_session_anchor();
        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &ctx).await;
        assert_eq!(
            cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor()
                .as_deref(),
            Some("session-root-perm001"),
            "a PARENT-role SessionStart must publish the live session id as the spawn anchor"
        );

        let _ = ext
            .on_event(&HostEvent::SessionShutdown { reason: "exit".to_string() }, &ctx)
            .await;
        assert_eq!(
            cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor(),
            None,
            "SessionShutdown must clear the anchor (pi's `delete process.env[...]`)"
        );
    }

    /// PERM-001 follow-up — the CHILD half of the publisher gate, and the cross-crate invariant the
    /// published-first anchor ladder rests on.
    ///
    /// `cyrup_ext_subagents::background::parent_anchor::resolve_parent_session_anchor` resolves
    /// PUBLISHED before INHERITED, emulating pi's single-cell ASSIGNMENT
    /// (`process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `pi-subagents/src/extension/
    /// index.ts:599` @v0.34.0). That ordering is safe for a NESTED orchestrator — one that was
    /// itself spawned as a subagent and must keep threading the ROOT's anchor downward rather than
    /// substituting its own id — for exactly ONE reason: such a process never publishes, so its
    /// register stays empty and the inherited root anchor wins regardless of rung order.
    ///
    /// Upstream enforces that with `if (!process.env[SUBAGENT_CHILD_ENV])` wrapped around the
    /// assignment (`index.ts:596-601` @v0.34.0). Cyrup's analog is `install_watcher`, which
    /// [`PermissionSystemExtension::new_forwarding_child`] sets to `false` and which
    /// `publish_parent_session_anchor` early-returns on — and `permission_extension_for_env` builds
    /// exactly that role whenever [`is_subagent_child`] sees a [`SUBAGENT_ENV_HINT_KEYS`] hint.
    ///
    /// NOTHING pinned that gate. If it regressed — a flipped flag, a second publisher, a refactor
    /// of `new_forwarding_child` — a nested orchestrator would publish its own id, the register
    /// would shadow the inherited root anchor, and a depth-2 grandchild would address its immediate
    /// parent's forwarding spool instead of the root's. Every forwarded ask from that subtree would
    /// then land on a spool with no watcher on it and fail-closed DENY, silently and with no
    /// prompt. This test is the guard for that.
    #[tokio::test]
    async fn a_subagent_child_never_publishes_or_clears_the_parent_session_anchor() {
        let _guard = ANCHOR_REGISTER_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");

        // A nested orchestrator: itself a subagent child, so `permission_extension_for_env` builds
        // it with `new_forwarding_child` (`install_watcher: false`). It has a perfectly good live
        // session id of its own — the gate, not the absence of an id, is what must stop it.
        let child = PermissionSystemExtension::new_forwarding_child(
            agent_dir.clone(),
            dir.path().to_path_buf(),
        );
        child.set_host_services(Arc::new(AnchorHost("nested-orchestrator-own-id")));
        let ctx = event_ctx(dir.path().to_path_buf());

        cyrup_ext_subagents::clear_parent_session_anchor();
        let _ = child
            .on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &ctx)
            .await;
        assert_eq!(
            cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor(),
            None,
            "a CHILD-role SessionStart must NOT publish its own id (pi's \
             `if (!process.env[SUBAGENT_CHILD_ENV])` guard, `index.ts:596`) — publishing here is \
             what would make the published-first ladder hand a grandchild the WRONG ancestor"
        );

        // The consequence that makes the reorder safe, asserted directly rather than inferred: with
        // nothing published, the inherited ROOT anchor is what a spawn from this process resolves.
        assert_eq!(
            cyrup_ext_subagents::background::parent_anchor::resolve_parent_session_anchor_from(
                Some("root-session-anchor".to_string())
            ),
            Some("root-session-anchor".to_string()),
            "a nested orchestrator keeps threading the ROOT's anchor downward — this is the case \
             the published-first reorder had to leave untouched, and it holds because the register \
             above is empty"
        );

        // The mirror gate (`SessionShutdown`): a child never published, so it must never CLEAR
        // either — otherwise a child sharing a process with a parent-role session would wipe the
        // anchor out from under it.
        cyrup_ext_subagents::publish_parent_session_anchor("root-session-anchor");
        let _ = child
            .on_event(&HostEvent::SessionShutdown { reason: "exit".to_string() }, &ctx)
            .await;
        assert_eq!(
            cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor()
                .as_deref(),
            Some("root-session-anchor"),
            "a CHILD-role SessionShutdown must leave a published anchor alone (it never published \
             one), or it would clear an anchor that is not its to clear"
        );

        cyrup_ext_subagents::clear_parent_session_anchor();
    }

    // ============================================================================================
    // PERM-005 — the forwarding watcher must be (re)armed on EVERY hook pi arms it on, idempotently,
    // and torn down when the context stops qualifying.
    //
    // Upstream calls `startForwardedPermissionPolling(ctx)` from four places —
    // `refreshSessionRuntimeState` (`index.ts:2084`, reached from `session_start`),
    // `before_agent_start` (`:2137`), `input` (`:2194`) and `tool_call` (`:2210`) — and calls
    // `stopForwardedPermissionPolling()` from `session_shutdown` (`:2131`) AND from the
    // disqualified branch of the start function itself (`:1985`).
    //
    // Cyrup had exactly ONE caller (`SessionStart`) and a guard that returned without stopping.
    // ============================================================================================

    /// A [`HostServices`] with a fixed session id, standing in for a live parent backend.
    struct WatcherHost(String);
    impl HostServices for WatcherHost {
        fn session_id(&self) -> Option<String> {
            Some(self.0.clone())
        }
    }

    fn ui_ctx(cwd: &Path) -> HostCtx {
        HostCtx::event(cyrup_ext::ExtMode::Tui, true, cwd.to_path_buf())
    }

    fn headless_ctx(cwd: &Path) -> HostCtx {
        HostCtx::event(cyrup_ext::ExtMode::Tui, false, cwd.to_path_buf())
    }

    /// Builds a PARENT-role extension AND takes [`ANCHOR_REGISTER_LOCK`], returning the guard the
    /// caller must hold for the rest of the test.
    ///
    /// The guard is bundled rather than left to each caller because the coupling is INVISIBLE at
    /// the call site: none of these PERM-005 watcher tests mentions the parent-session anchor, but
    /// every one of them fires a PARENT-role `SessionStart`, and that hook calls
    /// `publish_parent_session_anchor` as a SIDE EFFECT — writing the process-global register that
    /// `parent_role_publishes_and_clears_the_process_parent_session_anchor` and
    /// `a_subagent_child_never_publishes_or_clears_the_parent_session_anchor` assert on. Four
    /// unsynchronized writers against two asserting readers in one test binary is a live race: it
    /// was observed failing the child-gate assertion with `Some("perm005-detach")` — this helper's
    /// own session id — leaking in from `a_detaching_ui_tears_the_forwarding_watcher_down`.
    ///
    /// Returning the guard makes that safety automatic for any FUTURE watcher test too, instead of
    /// depending on its author noticing an anchor coupling nothing in the test text mentions.
    async fn parent_ext(
        dir: &Path,
        session: &str,
    ) -> (tokio::sync::MutexGuard<'static, ()>, PermissionSystemExtension) {
        let guard = ANCHOR_REGISTER_LOCK.lock().await;
        let agent_dir = dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let ext = PermissionSystemExtension::new_forwarding_parent(agent_dir, dir.to_path_buf());
        ext.set_host_services(Arc::new(WatcherHost(session.to_string())));
        (guard, ext)
    }

    /// PERM-005, the crux: the three per-turn hooks fire on EVERY turn, so a non-idempotent start
    /// would spawn one watcher per turn. N calls must yield exactly ONE live watcher task.
    #[tokio::test]
    async fn repeated_hooks_yield_exactly_one_forwarding_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-idem").await;
        let ctx = ui_ctx(dir.path());

        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".into() }, &ctx).await;
        assert!(ext.has_live_forwarding_watcher(), "SessionStart must arm the watcher");

        // Ten more turns' worth of hooks — the exact re-entry pi performs.
        for _ in 0..10 {
            let _ = ext
                .on_event(
                    &HostEvent::BeforeAgentStart {
                        prompt: String::new(),
                        images: json!(null),
                        system_prompt: String::new(),
                        options: json!(null),
                        injected: Vec::new(),
                    },
                    &ctx,
                )
                .await;
            let _ = ext
                .on_event(&HostEvent::Input {
                    text: "hello".into(),
                    images: Vec::new(),
                    source: cyrup_ext::InputEventSource::Interactive,
                    streaming_behavior: None,
                }, &ctx)
                .await;
            let _ = ext
                .on_event(
                    &HostEvent::ToolCall {
                        call_id: cyrup_core::ToolCallId::from("c1"),
                        name: "read".into(),
                        input: json!({}),
                    },
                    &ctx,
                )
                .await;
        }

        assert!(ext.has_live_forwarding_watcher(), "the watcher must still be live");
        assert_eq!(
            ext.live_watcher_task_count(),
            1,
            "31 hook re-entries must leave EXACTLY one watcher task — a non-idempotent start would \
             have leaked one per turn (pi `index.ts:1996-2000` keeps the existing watcher)"
        );

        ext.stop_forwarding_watcher();
    }

    /// PERM-005 failure mode (2): a UI that attaches AFTER `SessionStart` must still get a watcher.
    /// Pre-fix, `SessionStart` was the only caller, so a session that was headless at start never
    /// armed one for its whole life and every forwarded child ask sat in the spool until it failed
    /// closed.
    #[tokio::test]
    async fn a_later_hook_arms_the_watcher_a_headless_session_start_could_not() {
        let dir = tempfile::tempdir().unwrap();
        let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-late-ui").await;

        let _ = ext
            .on_event(
                &HostEvent::SessionStart { reason: "startup".into() },
                &headless_ctx(dir.path()),
            )
            .await;
        assert!(
            !ext.has_live_forwarding_watcher(),
            "a headless SessionStart must not arm a watcher (pi `:1726`)"
        );

        // The UI attaches; the very next turn's `tool_call` re-enters the start function.
        let _ = ext
            .on_event(
                &HostEvent::ToolCall {
                    call_id: cyrup_core::ToolCallId::from("c1"),
                    name: "read".into(),
                    input: json!({}),
                },
                &ui_ctx(dir.path()),
            )
            .await;
        assert!(
            ext.has_live_forwarding_watcher(),
            "pi re-enters `startForwardedPermissionPolling` from `tool_call` (`index.ts:2210`), so \
             a late-attaching UI must arm the watcher"
        );

        ext.stop_forwarding_watcher();
    }

    /// PERM-005 failure mode (3): a UI that DETACHES mid-session must tear the watcher down. pi's
    /// disqualified branch calls `stopForwardedPermissionPolling()` before returning
    /// (`index.ts:1984-1987`); cyrup's guard used to `return` and leave the task prompting into a
    /// backend with no human behind it.
    #[tokio::test]
    async fn a_detaching_ui_tears_the_forwarding_watcher_down() {
        let dir = tempfile::tempdir().unwrap();
        let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-detach").await;

        let _ = ext
            .on_event(&HostEvent::SessionStart { reason: "startup".into() }, &ui_ctx(dir.path()))
            .await;
        assert!(ext.has_live_forwarding_watcher(), "SessionStart with a UI arms the watcher");

        let _ = ext
            .on_event(
                &HostEvent::Input {
                    text: "hello".into(),
                    images: Vec::new(),
                    source: cyrup_ext::InputEventSource::Interactive,
                    streaming_behavior: None,
                },
                &headless_ctx(dir.path()),
            )
            .await;
        assert!(
            !ext.has_live_forwarding_watcher(),
            "a hook on a no-UI context must STOP the watcher, not merely decline to start one"
        );
    }

    /// PERM-005 failure mode (4): the watcher must observe a mid-session `config.json` change.
    /// It now shares the extension's `config` mutex instead of capturing a snapshot by value, so
    /// `refresh_config_and_manager` (pi `refreshExtensionConfig`, `index.ts:1600-1608`) reaches the
    /// running task. Asserted structurally — the running watcher and the extension must be looking
    /// at the SAME `ExtensionConfig`.
    #[tokio::test]
    async fn the_running_watcher_shares_the_extensions_live_config() {
        let dir = tempfile::tempdir().unwrap();
        let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-config").await;
        let ctx = ui_ctx(dir.path());

        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".into() }, &ctx).await;
        assert_eq!(ext.live_watcher_task_count(), 1, "one watcher, holding the shared handle");

        // The watcher's handle IS the extension's handle: a write here is visible to the task.
        assert!(!guard(&ext.config).yolo_mode);
        guard(&ext.config).yolo_mode = true;
        assert!(
            guard(&ext.config).yolo_mode,
            "the config the watcher reads per poll iteration is the one the extension mutates"
        );

        ext.stop_forwarding_watcher();
    }
}
