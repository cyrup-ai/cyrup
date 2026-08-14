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

use crate::agent_start_cache::{
    self, AgentStartCache, CachedPromptState, PromptStateKeyInput,
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
use crate::stores::SessionApprovalStore;
use crate::types::{CheckSource, PermissionCheckResult, PermissionState};
use crate::yolo_api::{YoloModeControlOptions, YoloModeControlResult};

/// The extension's fixed id (pi `EXTENSION_ID`, `extension-config.ts:8`).
pub const EXTENSION_ID: &str = "cyrup-permission-system";

/// The slash command this extension registers (pi `pi.registerCommand("permission-system", …)`,
/// v0.8.0 `index.ts:1502`).
pub const PERMISSION_SYSTEM_COMMAND: &str = "permission-system";

/// The `source` label the `/permission-system` handler passes to
/// [`PermissionSystemExtension::set_yolo_mode`], so a `yolo_mode.updated` entry says which surface
/// moved the flag (pi `options.source`, `yolo-mode-api.ts:3`).
pub const COMMAND_YOLO_CONTROL_SOURCE: &str = "permission-system-command";

/// pi `saved.error ?? "Failed to persist pi-permission-system config."` (v0.8.0 `index.ts:1439`),
/// rebranded. Reached only if [`ExtensionConfig::save`] ever reports failure without an error
/// string, which it does not today — carried because upstream carries it.
const YOLO_PERSIST_FALLBACK_ERROR: &str = "Failed to persist cyrup-permission-system config.";

/// PERM-029 — the shipped JSON Schema for `cyrup-permissions.jsonc`, a rebranded port of upstream's
/// `schemas/permissions.schema.json` @v0.8.0 (`$id` and title rebranded; every keyword, `$def`,
/// `patternProperties` entry and description otherwise upstream's). Embedded rather than merely
/// shipped so `/permission-system schema` can emit it from a running binary with no install-layout
/// assumptions, and so [`schema_is_wellformed`] can validate it at test time — cyrup's analog of
/// upstream's `scripts/validate-artifacts.mjs:50` wired into the package `check` script.
pub const PERMISSIONS_JSON_SCHEMA: &str =
    include_str!("../schemas/cyrup-permissions.schema.json");

/// PERM-029 — the starter policy, a rebranded port of upstream's `config/config.example.json`
/// @v0.8.0. Every category the manager understands appears at least once, so an operator has a
/// working template rather than a blank file. Validated against a real [`PermissionManager`] by
/// this crate's own test, which is what upstream's `validate-artifacts.mjs:56` buys.
pub const PERMISSIONS_EXAMPLE_CONFIG: &str = include_str!("../config/config.example.json");

/// The `/permission-system` usage line. The two setting ids and the `on`/`off` value set are pi's
/// (`config-modal.ts:18,27,34`); the textual framing is cyrup's, since upstream renders a modal.
const COMMAND_USAGE: &str = "Usage: /permission-system [debug|yoloMode on|off] [schema] [example]";

/// pi `toOnOff` (`config-modal.ts:20-22`).
fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// The global policy file (pi `pi-permissions.jsonc`; cyrup analog).
const POLICY_FILE: &str = "cyrup-permissions.jsonc";
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

/// pi `PERMISSION_POLICY_AGENT_DIR_ENV_KEY = "PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR"`
/// (v0.8.0 `permission-manager.ts:29`), renamed to this crate's `CYRUP_` env-var convention (see
/// [`INSTALL_ENV_VAR`], [`crate::ext_config::CONFIG_PATH_ENV_KEY`],
/// `forwarding::FORWARDING_AGENT_DIR_ENV`).
///
/// Relocates the **global policy root** — the directory the four global policy artifacts live in
/// (`cyrup-permissions.jsonc`, `agents/`, `settings.json`, `mcp.json`). It does NOT move the
/// project-scoped `<cwd>/.cyrup/agent` tree, matching upstream: `createPermissionManagerForCwd`
/// (`index.ts:1287-1301`) supplies only `projectGlobalConfigPath` / `projectAgentsDir`, so every
/// GLOBAL path in a live session falls back to `defaultPolicyAgentDir()` (`:31-38`).
pub const POLICY_AGENT_DIR_ENV_KEY: &str = "CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR";

/// pi `defaultPolicyAgentDir()` (v0.8.0 `permission-manager.ts:31-33`):
/// `const override = process.env[KEY]?.trim(); return override ? resolve(override) : getAgentDir();`
///
/// The precedence is exactly upstream's: an env value that trims to the empty string is NOT an
/// override (JS `""` is falsy), and a non-empty one is `resolve`d — absolutized against the process
/// cwd — before use. [`std::path::absolute`] is the direct analog of node's `path.resolve` for a
/// single argument: it is purely lexical and never touches the filesystem, so a not-yet-created
/// policy root still resolves. On the (io-error) failure path the trimmed value is used as given,
/// which is what `resolve` would have produced for an already-absolute path.
///
/// **The probe and the engine must both go through this**, or they inspect different trees and
/// disagree — the PERM-018 hazard, one rung up: [`PermissionSystemExtension::manager_paths_for`]
/// builds the enforced paths from it and [`is_installed`] probes it.
#[must_use]
fn policy_agent_dir(agent_dir: &Path) -> PathBuf {
    let Some(raw) = std::env::var(POLICY_AGENT_DIR_ENV_KEY)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return agent_dir.to_path_buf();
    };
    let raw = PathBuf::from(raw);
    std::path::absolute(&raw).unwrap_or(raw)
}

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

/// The permission-system extension: the layered policy engine + the (session-only) approval store +
/// prompt dedup + the fail-closed ask channel, gating every tool call via `before_tool_call`.
pub struct PermissionSystemExtension {
    id: ExtensionId,
    manager: Mutex<PermissionManager>,
    session_approvals: Mutex<SessionApprovalStore>,
    dedup: Mutex<DedupCache>,
    /// The extension `config.json` snapshot. `yolo_mode` is read on the live `ask` path (below);
    /// `debug` gates the DIAGNOSTIC JSONL stream only ([`Self::logger`], v0.8.0 `logging.ts:90-93`)
    /// AND the forwarding "child is waiting" notice (`forwarding.rs`). It does NOT gate the
    /// security `review` stream: v0.8.0 deleted that guard (the v0.7.1 guard was `logging.ts:97`),
    /// so the audit trail is unconditional — see [`crate::logging::Logger::review`].
    /// `forwarded_prompt_timeout_seconds` is
    /// consumed by forwarding (P-4). `Mutex`-wrapped because
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
    /// PERM-013 / pi `lastActiveToolsCacheKey` + `lastPromptStateCacheKey` +
    /// `lastPromptStateCacheResult` (`index.ts:1312-1314`), grouped so
    /// [`PermissionSystemExtension::invalidate_agent_start_cache`] is one assignment (pi
    /// `invalidateAgentStartCache`, `:1326-1331`). Read and written only inside
    /// [`PermissionSystemExtension::on_before_agent_start`].
    agent_start_cache: Mutex<AgentStartCache>,
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
    ///
    /// PERM-007: `Arc`-wrapped so the shared [`crate::config_modal::ConfigController`] clears the
    /// SAME memo the load path sets — the modal's writer is pi's `saveExtensionConfig`, which does
    /// `lastConfigWarning = null` at `index.ts:1414`.
    last_config_warning: Arc<Mutex<Option<String>>>,
    /// PERM-007 — pi's `PermissionSystemConfigController` (`config-modal.ts:8-12`), registered at
    /// `index.ts:1504-1511`. The config WRITER, extracted into a shared object so the `'static`
    /// [`crate::config_modal::PermissionSystemSettingsOverlay`] can hold it without borrowing this
    /// extension. [`Self::save_extension_config`] delegates here, so there is exactly one
    /// implementation of the normalize → write → touch-memory ordering contract.
    controller: Arc<crate::config_modal::ConfigController>,
    /// pi `extensionLogger` (`index.ts:148-150`): the `debug`-gated audit/debug JSONL trail
    /// ([`crate::logging`], pi `logging.ts`). Shares the SAME `config` `Arc` above, so the operator
    /// flipping `"debug": true` in `config.json` arms it on the next
    /// `session_start` / `resources_discover` reload with no restart.
    /// pi's module-scope logging trio ([`crate::logging::AuditTrail`]): the `extensionLogger`,
    /// the `reportedLoggingWarnings` dedup set and the `loggingWarningReporter`
    /// (`index.ts:160-164`). Held as an `Arc` so the detached forwarding watcher writes into the
    /// SAME trail with the SAME dedup set — pi's module scope, made explicit (PERM-008).
    logger: Arc<crate::logging::AuditTrail>,
    /// PERM-031 — the live `ctx.has_ui`, mirrored out of every ctx-bearing event dispatch so the
    /// detached forwarding watcher can re-check it on every scan (pi reads `ctx.hasUI` off the
    /// retained `permissionForwardingContext`, `index.ts:1114`). See
    /// [`crate::forwarding::SharedHasUi`].
    has_ui: forwarding::SharedHasUi,
}

impl PermissionSystemExtension {
    /// The bare constructor (test / non-forwarding): derive every policy/store path from `agent_dir` +
    /// the session `cwd`, and fail-close asks through [`NoOpAskChannel`] (the live in-session dialog
    /// still activates via `ctx.has_ui` + a captured backend). Installs NO forwarding watcher — the
    /// wired PARENT uses [`Self::new_forwarding_parent`].
    #[must_use]
    pub fn new(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let config = Self::load_config(&agent_dir);
        Self::new_with_config(agent_dir, cwd, config)
    }

    /// [`Self::new`] over an ALREADY-LOADED [`ExtensionConfig`] — see [`Self::load_config`] for why
    /// the read is hoisted out of the constructor.
    fn new_with_config(agent_dir: PathBuf, cwd: PathBuf, config: ExtensionConfig) -> Self {
        let paths = Self::manager_paths_for(&agent_dir, &cwd);
        Self::from_parts_full(
            paths,
            config,
            |_| Arc::new(NoOpAskChannel),
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
        let config = Self::load_config(&agent_dir);
        Self::new_forwarding_parent_with_config(agent_dir, cwd, config)
    }

    /// [`Self::new_forwarding_parent`] over an ALREADY-LOADED [`ExtensionConfig`] — see
    /// [`Self::load_config`].
    fn new_forwarding_parent_with_config(
        agent_dir: PathBuf,
        cwd: PathBuf,
        config: ExtensionConfig,
    ) -> Self {
        let paths = Self::manager_paths_for(&agent_dir, &cwd);
        Self::from_parts_full(
            paths,
            config,
            |_| Arc::new(NoOpAskChannel),
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
        let config = Self::load_config(&agent_dir);
        Self::new_forwarding_child_with_config(agent_dir, cwd, config)
    }

    /// [`Self::new_forwarding_child`] over an ALREADY-LOADED [`ExtensionConfig`] — see
    /// [`Self::load_config`].
    fn new_forwarding_child_with_config(
        agent_dir: PathBuf,
        cwd: PathBuf,
        config: ExtensionConfig,
    ) -> Self {
        let paths = Self::manager_paths_for(&agent_dir, &cwd);
        let host_services: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
        let channel_agent_dir = agent_dir.clone();
        let channel_services = Arc::clone(&host_services);
        Self::from_parts_full(
            paths,
            config,
            move |audit| {
                Arc::new(ForwardingAskChannel::new(
                    channel_agent_dir,
                    forwarding::resolve_child_wait_timeout(),
                    channel_services,
                    Arc::clone(audit),
                ))
            },
            agent_dir,
            false,
            host_services,
        )
    }

    /// Derive the [`ManagerPaths`] for `agent_dir` + `cwd` (pi `createPermissionManagerForCwd`'s path
    /// derivation, `index.ts:1536-1573`) — shared by every constructor AND by
    /// [`Self::refresh_config_and_manager`] (a `session_start` / `resources_discover` reload rebuilds
    /// this from the CURRENT cwd, not just the process's original one).
    fn manager_paths_for(agent_dir: &Path, cwd: &Path) -> ManagerPaths {
        let project_dir = PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg));
        // PERM-025 / pi `defaultGlobalConfigPath` / `defaultAgentsDir` /
        // `defaultLegacyGlobalSettingsPath` / `defaultGlobalMcpConfigPath`
        // (v0.8.0 `permission-manager.ts:35-38`): all four GLOBAL artifacts hang off
        // `defaultPolicyAgentDir()`, i.e. the `POLICY_AGENT_DIR_ENV_KEY` override when set. The two
        // PROJECT paths are supplied explicitly upstream too (`index.ts:1296-1300`) and are NOT
        // relocated.
        let policy_dir = policy_agent_dir(agent_dir);
        ManagerPaths {
            global_config_path: policy_dir.join(POLICY_FILE),
            agents_dir: policy_dir.join("agents"),
            project_global_config_path: Some(project_dir.join(POLICY_FILE)),
            project_agents_dir: Some(project_dir.join("agents")),
            legacy_global_settings_path: policy_dir.join("settings.json"),
            global_mcp_config_path: policy_dir.join("mcp.json"),
            mcp_server_names_override: None,
        }
    }

    /// The DEFAULT extension `config.json` path for `agent_dir` — cyrup's analog of pi's
    /// `CONFIG_PATH` constant (`extension-config.ts:41`, `join(EXTENSION_ROOT, "config.json")`).
    ///
    /// This is the *unresolved* default. Nothing outside this crate should read a config from it
    /// directly: `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` can point the extension at a different file
    /// entirely, and only [`Self::resolved_config_path_for`] honours that. Every consumer here
    /// either goes through that helper or through [`ExtensionConfig::load`] /
    /// [`ExtensionConfig::save`], which resolve internally.
    pub(crate) fn config_path_for(agent_dir: &Path) -> PathBuf {
        agent_dir.join(CONFIG_DIR).join(CONFIG_FILE)
    }

    /// The RESOLVED extension `config.json` path for `agent_dir`: [`Self::config_path_for`] after
    /// the `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` override, i.e. pi
    /// `getPermissionSystemConfigPath()` (v0.8.0 `extension-config.ts:51-53`, over
    /// `resolveOverridablePath`, `:46-49`).
    ///
    /// pi has exactly one such accessor and every consumer of the extension config funnels through
    /// it — `loadPermissionSystemConfig`'s default argument (`extension-config.ts:117`),
    /// `savePermissionSystemConfig`'s (`:240`), and the config modal's displayed `Config file:` path
    /// (`index.ts:1509`). cyrup's [`is_installed`] probe was reading the RAW default path instead,
    /// so with the override set the install decision and the `enabled` decision could inspect two
    /// different files and disagree. This helper is the one accessor; use it, not
    /// [`Self::config_path_for`].
    pub(crate) fn resolved_config_path_for(agent_dir: &Path) -> PathBuf {
        ExtensionConfig::resolve_config_path(&Self::config_path_for(agent_dir))
    }

    /// The default audit/debug log directory for `agent_dir` (pi `LOGS_DIR =
    /// join(EXTENSION_ROOT, "logs")`, `extension-config.ts:38`). cyrup's analog of pi's
    /// `EXTENSION_ROOT` is `<agent_dir>/cyrup-permission-system/` — the directory
    /// [`Self::config_path_for`] puts `config.json` in — so the trail lands beside the config that
    /// enables it. Overridable per write via `CYRUP_PERMISSION_SYSTEM_LOGS_DIR`
    /// ([`crate::logging::resolve_logs_dir`]).
    fn logs_dir_for(agent_dir: &Path) -> PathBuf {
        agent_dir.join(CONFIG_DIR).join(crate::logging::LOGS_DIR_NAME)
    }

    /// Read `config.json` ONCE, through the resolved path (pi `loadPermissionSystemConfig()`,
    /// `extension-config.ts:117-138`).
    ///
    /// pi's entry point calls this exactly once at load — `loadExtensionConfigState()`
    /// (`index.ts:1350-1354`) is invoked at `index.ts:1473`, the `enabled` master switch tests the
    /// module-scope `extensionConfig` it just populated (`:1475-1477`), and everything downstream
    /// reuses that same object. cyrup's [`permission_extension_for_env`] is the analog of that entry
    /// point, so it performs THE load and hands the result to the `*_with_config` constructor; the
    /// public constructors keep their standalone signature by doing the load themselves.
    ///
    /// Loading twice was not merely wasteful: [`ExtensionConfig::load`] `eprintln!`s on a malformed
    /// or unreadable config, so an operator with a corrupt `config.json` saw the identical warning
    /// printed twice per session build where pi prints it once.
    ///
    /// v0.7.1's `derive_parts` (which this replaces) also derived a
    /// `cyrup-permission-system-approvals.json` path for the `PermanentApprovalStore`; upstream
    /// deleted that store in v0.8.0 (commit `a33ac2c`), so no such file is read any more — see
    /// [`crate::stores`].
    fn load_config(agent_dir: &Path) -> ExtensionConfig {
        ExtensionConfig::load(&Self::config_path_for(agent_dir))
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
        // pi order (`refreshSessionRuntimeState`, v0.8.0 `index.ts:1819-1826`): config first,
        // manager second, agent-start cache invalidated third.
        self.refresh_extension_config();
        *guard(&self.manager) =
            manager_with_warnings(Self::manager_paths_for(&self.agent_dir, cwd), &self.warnings);
        self.invalidate_agent_start_cache();
    }

    /// pi `refreshExtensionConfig(ctx?)` (v0.8.0 `index.ts:1383-1386`) = `loadExtensionConfigState()`
    /// (`:1350-1354`) + `applyExtensionConfigSideEffects(result, ctx)` (`:1356-1381`), in that
    /// order. The **config half only** — no manager rebuild, no agent-start-cache invalidation.
    ///
    /// Split out of [`Self::refresh_config_and_manager`] for PERM-024: pi calls this on THREE
    /// surfaces (`session_start` via `refreshSessionRuntimeState` `:1821`, the `resources_discover`
    /// reload branch `:1848`, and `before_agent_start` `:1877`) but rebuilds the manager and
    /// invalidates the cache on only the first two. Calling the combined function from
    /// `before_agent_start` would rebuild the `PermissionManager` and blow away the agent-start
    /// cache on every single turn — the exact per-turn cost PERM-013's cache exists to remove.
    ///
    /// The side-effect ORDER inside `applyExtensionConfigSideEffects` is pi's and is load-bearing:
    /// status pill (`:1364-1366`) → warning memo (`:1368-1374`) → `config.loaded` debug entry
    /// (`:1376-1381`). PERM-026 was the status sync being absent from here entirely, so a
    /// `resources_discover` reload changed the live gating behaviour while the pill kept the stale
    /// value until the next `before_agent_start` repainted it.
    fn refresh_extension_config(&self) {
        let loaded = ExtensionConfig::load_with_result(&Self::config_path_for(&self.agent_dir));
        let (created, debug, yolo_mode) =
            (loaded.created, loaded.config.debug, loaded.config.yolo_mode);
        // pi `setExtensionConfig(result.config)` inside `loadExtensionConfigState` (`:1352`).
        *guard(&self.config) = loaded.config;
        // PERM-026 / pi `:1364-1366`: `if (runtimeContext?.hasUI) { syncPermissionSystemStatus(...) }`
        // — reached on EVERY refresh surface, which is why a reload re-syncs the pill upstream.
        // `sync_status_when_possible` is the ported form of that guard (see its doc for why the
        // `hasUI` half collapses into "is a backend attached").
        {
            let config = guard(&self.config).clone();
            self.sync_status_when_possible(&config);
        }
        self.report_config_warning(loaded.warning.clone());
        // pi `writeDebugEntry("config.loaded", …)` (`:1376-1381`) — emitted AFTER the new config
        // is installed, so a reload that turns `debug` ON records its own arrival as the trail's
        // first line.
        self.write_debug_entry(
            "config.loaded",
            &json!({
                "created": created,
                "warning": loaded.warning,
                "debug": debug,
                "yoloMode": yolo_mode,
            }),
        );
    }

    /// pi `invalidateAgentStartCache()` (v0.8.0 `index.ts:1326-1331`): drop the cached skill
    /// enforcement entries AND both `before_agent_start` cache keys, so the next turn recomputes
    /// from scratch. Called from `session_start` (`:1823`), the `resources_discover` reload branch
    /// (`:1852`) and `session_shutdown` (`:1871`) — never from `before_agent_start` itself.
    fn invalidate_agent_start_cache(&self) {
        // pi `activeSkillEntries = []` (`:1327`).
        guard(&self.active_skill_entries).clear();
        // pi `:1328-1330`.
        *guard(&self.agent_start_cache) = AgentStartCache::default();
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

    // ===================================================== the two v0.8.0 config WRITERS (G133/F1)
    //
    // `ExtensionConfig::save` (the atomic merge-into-the-existing-document write, v0.8.0
    // `extension-config.ts:240-293`) landed with NO non-test call site, which made all three of the
    // behaviours it exists to guarantee — non-extension keys preserved, a corrupt file refused, a
    // symlinked config written through — unobservable in cyrup, because cyrup never saved this
    // config at all. The two functions below are upstream's two callers of it; `execute_command`
    // (the `/permission-system` handler, pi `index.ts:1502-1512`) is what reaches them.

    /// pi `syncPermissionSystemStatusWhenPossible(config, ctx?)` (v0.8.0 `index.ts:1388-1400`):
    /// reflect `yoloMode` on the live status bar after a config write.
    ///
    /// \[CYRUP-DELTA] pi's two branches — an explicitly-passed `ctx` (`:1392-1395`, the
    /// `saveExtensionConfig` case) versus the module-scope `runtimeContext?.hasUI` fallback
    /// (`:1397-1399`, the `setYoloModeFromRuntimeApi` case) — exist because pi's `ui.setStatus` is
    /// only reachable through whichever `ExtensionContext` object is at hand. cyrup's status seam is
    /// [`crate::status::sync_status`] over the ONE late-bound [`HostServices`] backend the session
    /// attaches, which is the same object no matter which handler is running, so both branches
    /// collapse into this single reachability test. The `hasUI` half is not re-imposed, for the
    /// reason [`WarningSink::notify`] documents: `HostServices::set_status` already no-ops on a
    /// backend with no status surface, and re-imposing it would blank the pill in modes that do
    /// render one. This is the same test the `SessionStart` / `BeforeAgentStart` arms already use.
    fn sync_status_when_possible(&self, config: &ExtensionConfig) {
        if let Some(services) = self.host_services.get() {
            status::sync_status(services, config);
        }
    }

    /// pi `saveExtensionConfig(next, ctx)` (v0.8.0 `index.ts:1402-1420`) — registered as the config
    /// modal's `setConfig` (`index.ts:1508`), i.e. what runs when the human flips a row in
    /// `/permission-system`.
    ///
    /// The ORDER is the contract, and it is the reason this is a function and not three inlined
    /// statements: normalize, WRITE, and only then touch anything in memory. A failed write returns
    /// the cause and has changed NOTHING — no live config, no status pill, no `lastConfigWarning`
    /// reset, no debug entry — so cyrup can never end a turn with an in-memory config that
    /// disagrees with the file the next `session_start` will re-read.
    ///
    /// \[CYRUP-DELTA] Two shape differences, neither behavioural:
    /// - pi takes `ctx: ExtensionCommandContext` purely to reach `ctx.ui.notify` (`:1407`) and to
    ///   pass to `syncPermissionSystemStatusWhenPossible` (`:1413`). Both of those reach the live
    ///   [`HostServices`] backend in cyrup, which this extension already holds, so there is no ctx
    ///   parameter to thread and nothing is lost by its absence.
    /// - pi returns `void`; this returns whether the save landed, and on failure the RAW cause. Its
    ///   one upstream caller recovers the same fact by re-reading `controller.getConfig()` straight
    ///   after (`config-modal.ts:79`), which is exactly what the returned `Result` saves the cyrup
    ///   caller from having to do.
    ///
    /// **The `Err` MUST be surfaced by the caller**, at [`NotifyKind::Error`]. pi notifies inline
    /// here (`:1407`); cyrup hands the cause up one level instead so the human gets ONE error toast
    /// carrying both the what ("YOLO mode is unchanged (off)") and the why (the raw save error),
    /// rather than two toasts saying half each. See [`Self::run_permission_system_command`], the
    /// only caller, and the `Ok(None)` convention documented on
    /// [`cyrup_ext::NativeExtension::execute_command`].
    ///
    /// PERM-007: the BODY now lives on the shared
    /// [`crate::config_modal::ConfigController`] — pi's own `{getConfig, setConfig, getConfigPath}`
    /// indirection (`config-modal.ts:8-12`, registered `index.ts:1504-1511`) — so the `'static`
    /// settings overlay commits through the identical writer this method does. Nothing about the
    /// ordering contract moved with it; see [`crate::config_modal::ConfigController::set_config`].
    pub fn save_extension_config(&self, next: &ExtensionConfig) -> Result<(), String> {
        self.controller.set_config(next)
    }

    /// PERM-007 — the shared config controller, so a caller that needs the WRITER without holding
    /// this extension (an overlay, a runtime-API consumer) can take an `Arc` of it.
    #[must_use]
    pub fn config_controller(&self) -> Arc<crate::config_modal::ConfigController> {
        Arc::clone(&self.controller)
    }

    /// pi `getYoloMode: () => extensionConfig.yoloMode` (v0.8.0 `index.ts:1482`).
    #[must_use]
    pub fn yolo_mode(&self) -> bool {
        guard(&self.config).yolo_mode
    }

    /// pi `setYoloModeFromRuntimeApi(enabled, options)` (v0.8.0 `index.ts:1422-1469`), exposed
    /// upstream as the runtime API's `setYoloMode` (`:1483`).
    ///
    /// The security-relevant property, and the whole reason this is not just "assign the field and
    /// save": when `persist` is on and the write FAILS, the in-memory yolo mode is left exactly as
    /// it was and the result reports `changed: false, persisted: false` with the error
    /// (`:1438-1451`). A caller must never be told that auto-approval was turned on (or off) when
    /// the gate's live config — and the file the next session will load — still says the opposite.
    ///
    /// \[CYRUP-DELTA] pi's first statement is a runtime `typeof enabled !== "boolean"` guard
    /// returning an unchanged result with `"setYoloMode(enabled) requires a boolean value."`
    /// (`:1423-1430`). That branch is **unrepresentable in Rust**: `enabled` is typed `bool`, so no
    /// caller can reach it and there is nothing to check at runtime. It is not ported and no
    /// stand-in is invented for it — the compiler enforces the same precondition earlier and more
    /// completely. This is a language difference, not a dropped behaviour; the `error` field of
    /// [`YoloModeControlResult`] remains, because the persist-failure path (`:1449`) still uses it.
    pub fn set_yolo_mode(
        &self,
        enabled: bool,
        options: &YoloModeControlOptions,
    ) -> YoloModeControlResult {
        // pi `normalizePermissionSystemConfig({ ...extensionConfig, yoloMode: enabled })` (`:1432`).
        // Cloned out of the mutex first so nothing below runs while the live config is locked.
        let current = guard(&self.config).clone();
        let normalized = ExtensionConfig { yolo_mode: enabled, ..current.clone() }.normalized();
        // pi `const persisted = options.persist !== false` (`:1433`).
        let persisted = options.persists();
        // pi `const changed = extensionConfig.yoloMode !== normalized.yoloMode` (`:1434`).
        let changed = current.yolo_mode != normalized.yolo_mode;

        if persisted {
            // pi `const saved = savePermissionSystemConfig(normalized)` (`:1437`).
            let saved = normalized.save(&Self::config_path_for(&self.agent_dir));
            if !saved.success {
                // pi `saved.error ?? "Failed to persist pi-permission-system config."` (`:1439`).
                let error = saved
                    .error
                    .unwrap_or_else(|| YOLO_PERSIST_FALLBACK_ERROR.to_string());
                // pi `writeDebugEntry("yolo_mode.update_failed", {...})` (`:1440-1444`).
                self.write_debug_entry(
                    "yolo_mode.update_failed",
                    &json!({
                        "error": error,
                        "requestedYoloMode": normalized.yolo_mode,
                        "source": options.source_or_default(),
                    }),
                );
                // pi `:1445-1450`: `yoloMode: extensionConfig.yoloMode` — the UNCHANGED live value,
                // read fresh rather than reported from `normalized`.
                return YoloModeControlResult {
                    yolo_mode: guard(&self.config).yolo_mode,
                    changed: false,
                    persisted: false,
                    error: Some(error),
                };
            }
            // pi `lastConfigWarning = null` (`:1452`) — inside the `persisted` branch, so a
            // `persist: false` call deliberately leaves the memo alone (nothing was written).
            *guard(&self.last_config_warning) = None;
        }

        // pi `setExtensionConfig(normalized)` (`:1455`).
        *guard(&self.config) = normalized.clone();
        // pi `syncPermissionSystemStatusWhenPossible(normalized)` — no ctx (`:1456`).
        self.sync_status_when_possible(&normalized);
        // pi `writeDebugEntry("yolo_mode.updated", {...})` (`:1457-1462`).
        self.write_debug_entry(
            "yolo_mode.updated",
            &json!({
                "changed": changed,
                "persisted": persisted,
                "source": options.source_or_default(),
                "yoloMode": normalized.yolo_mode,
            }),
        );
        // pi `:1464-1468` — note `error` is absent, not `null`.
        YoloModeControlResult { yolo_mode: normalized.yolo_mode, changed, persisted, error: None }
    }

    /// pi `toggleYoloMode: (options?) => setYoloModeFromRuntimeApi(!extensionConfig.yoloMode,
    /// options)` (v0.8.0 `index.ts:1484`).
    pub fn toggle_yolo_mode(&self, options: &YoloModeControlOptions) -> YoloModeControlResult {
        self.set_yolo_mode(!self.yolo_mode(), options)
    }

    /// The `/permission-system` handler body (pi `index.ts:1504-1511` via
    /// `createPermissionSystemCommandHandler`, `common.ts:188-198`), reached from
    /// [`NativeExtension::execute_command`].
    ///
    /// \[CYRUP-DELTA] Upstream's body is `openPermissionSystemSettingsModal(ctx, controller)`
    /// (`config-modal.ts:63-123`): a `ctx.ui.custom` overlay rendering pi's own `ZellijSettingsModal`
    /// over two rows. This handler is a textual form of the same controller instead: the same two
    /// setting ids, the same `on`/`off` value set (`config-modal.ts:18`), the same `applySetting`
    /// mapping (`:43-56`), the same `setConfig` writer, and the same `Config file: <path>` help
    /// text (`:85`).
    ///
    /// **PERM-007 — the reason recorded here was STALE and is corrected.** It claimed
    /// "`HostServices` exposes no custom-overlay seam". One exists:
    /// `cyrup_ext::HostServices::open_overlay` (`cyrup-ext/src/host/services.rs`) over
    /// `cyrup_ext::host::overlay::InteractiveOverlay`, with a live implementation in
    /// `cyrup-session-svc`'s `LiveHostServices` and a production caller in
    /// `cyrup-ext-subagents`. The `cyrup-tui` half of the old reason still holds and is why the
    /// seam is shaped as serializable `OverlayLine`s rather than ratatui types, but it is not a
    /// reason the modal cannot be built. What remains is the work itself: an `InteractiveOverlay`
    /// implementation is `'static`, so it cannot borrow `&self` and needs the config writer
    /// extracted into a shared controller object — pi's own
    /// `{getConfig, setConfig, getConfigPath}` (`index.ts:1504-1511`) made explicit. Until that
    /// lands, the operator gets a read-only dump plus two blind toggles.
    ///
    /// Grammar (`<setting> <value>`; no args renders the modal's initial view):
    /// - `/permission-system` — current values + config path.
    /// - `/permission-system debug on|off` — pi `applySetting("debug", …)` → `setConfig`.
    /// - `/permission-system yoloMode on|off` — the yolo row.
    ///
    /// BOTH rows go through [`Self::save_extension_config`], matching upstream: the modal's
    /// `onChange` calls `controller.setConfig` for every setting id (`config-modal.ts:74-76`), and
    /// `setConfig` is registered as `saveExtensionConfig` (`index.ts:1508`). `setYoloMode` is a
    /// DIFFERENT surface — upstream's runtime API (`index.ts:1483-1484`), reachable by other
    /// extensions through `globalThis.__piPermissionSystem`, not by this command.
    ///
    /// An earlier revision routed the yolo row through [`Self::set_yolo_mode`] so that method would
    /// have a caller. That was the wrong trade: it changed the emitted debug event
    /// (`yolo_mode.updated` instead of `config.saved`) and the error surface, distorting ported
    /// behaviour to satisfy a reachability rule. [`Self::set_yolo_mode`],
    /// [`Self::toggle_yolo_mode`] and [`Self::yolo_mode`] are therefore correctly ported and
    /// currently UNREACHABLE — `cyrup-ext` has no extension-provided-API registry for one extension
    /// to call another's methods, which is the actual missing piece. Tracked as G133b in
    /// `docs/gap-analysis/PARITY-GAPS.md`; see [`crate::yolo_api`].
    ///
    /// Returns `Some(text)` for output the session surfaces as an **Info** notification, and `None`
    /// when this handler has ALREADY notified at its own level — the convention documented on
    /// [`cyrup_ext::NativeExtension::execute_command`]. The save-failure branches take the `None`
    /// route: they raise one [`NotifyKind::Error`] toast carrying both the human sentence and the
    /// raw cause, instead of returning a sentence that would arrive as a second, Info-level toast
    /// alongside the error (`cyrup-session-svc/src/session.rs:961-1004` surfaces every
    /// `Ok(Some(..))`).
    fn run_permission_system_command(&self, args: &str) -> Option<String> {
        let mut parts = args.split_whitespace();
        let Some(setting) = parts.next() else {
            // PERM-007 — pi's bare `/permission-system` is
            // `openPermissionSystemSettingsModal(ctx, controller)` (`config-modal.ts:63-122`), a
            // live `ctx.ui.custom(…, { overlay: true, … })`. Hand the host the real overlay; the
            // text dump below is now only the fall-back for a host that owns no interactive
            // surface, which is precisely pi's own `if (!ctx.hasUI)` branch (`common.ts:188-198`)
            // and NOT an error.
            return self.open_settings_overlay();
        };
        // PERM-029 — two zero-argument emitters for the artifacts upstream ships as FILES and
        // documents in its README (`README.md:655`'s CLI validation recipe, `:659`'s "Add
        // `"$schema"`: … to your config for autocomplete support"). cyrup ships them as crate
        // files too, but a Rust binary has no `node_modules` path an operator can point an editor
        // at, so the command is how they are reached from a running install.
        match setting {
            "schema" => return Some(PERMISSIONS_JSON_SCHEMA.to_string()),
            "example" => return Some(PERMISSIONS_EXAMPLE_CONFIG.to_string()),
            _ => {}
        }
        let value = parts.next();
        if parts.next().is_some() {
            return Some(format!("Unexpected extra arguments.\n{COMMAND_USAGE}"));
        }

        // pi `ON_OFF = ["on", "off"]` (`config-modal.ts:18`) — the modal can only ever emit one of
        // these two, so anything else is a usage error rather than `applySetting`'s silent
        // `value === "on"` coercion.
        let enabled = match value {
            Some("on") => true,
            Some("off") => false,
            Some(other) => return Some(format!("Unknown value `{other}`.\n{COMMAND_USAGE}")),
            None => return Some(format!("`{setting}` needs a value.\n{COMMAND_USAGE}")),
        };

        match setting {
            // pi `applySetting` `case "debug"` (`config-modal.ts:49-50`) → `setConfig` (`:78`).
            "debug" => {
                let next = ExtensionConfig { debug: enabled, ..guard(&self.config).clone() };
                match self.save_extension_config(&next) {
                    Ok(()) => Some(format!(
                        "Debug logging {}.\n{}",
                        on_off(enabled),
                        self.config_path_line()
                    )),
                    // pi surfaces this through `ctx.ui.notify(saved.error, "error")` ONLY
                    // (`index.ts:1407`) — one error-level toast, nothing else. Same here: the
                    // sentence and the raw cause go out together at Error, and the handler returns
                    // `None` so the session adds no second Info toast.
                    Err(cause) => {
                        self.notify_save_failure(
                            &format!(
                                "Failed to save the permission-system config; debug logging is \
                                 unchanged ({}).",
                                on_off(guard(&self.config).debug)
                            ),
                            &cause,
                        );
                        None
                    }
                }
            }
            // pi `applySetting` `case "yoloMode"` (`config-modal.ts:51-52`) → `setConfig` (`:75`),
            // the SAME writer the debug row uses. Not `setYoloMode` — that is the runtime API.
            "yoloMode" => {
                let next = ExtensionConfig { yolo_mode: enabled, ..guard(&self.config).clone() };
                match self.save_extension_config(&next) {
                    Ok(()) => {
                        Some(format!("YOLO mode {}.\n{}", on_off(enabled), self.config_path_line()))
                    }
                    // Same failure shape as the debug row: pi notifies through `ctx.ui.notify` and
                    // leaves the live config untouched (`index.ts:1405-1409`), so the value reported
                    // here is the one still in effect.
                    Err(cause) => {
                        self.notify_save_failure(
                            &format!(
                                "Failed to save the permission-system config; YOLO mode is \
                                 unchanged ({}).",
                                on_off(guard(&self.config).yolo_mode)
                            ),
                            &cause,
                        );
                        None
                    }
                }
            }
            // pi `applySetting`'s `default: return config` (`config-modal.ts:53-54`) — the modal
            // cannot emit an unknown id, so cyrup's text form reports it instead of silently
            // no-oping.
            other => Some(format!("Unknown setting `{other}`.\n{COMMAND_USAGE}")),
        }
    }

    /// Raise the ONE [`NotifyKind::Error`] toast a refused config write produces: the human sentence
    /// (what did not change, and what is still in effect), the config path, and the raw cause from
    /// [`Self::save_extension_config`] (why). pi emits only `ctx.ui.notify(saved.error, "error")`
    /// (`index.ts:1407`) — the raw cause alone — because its modal is still on screen to supply the
    /// context; cyrup's command has no modal, so the context has to travel in the toast.
    ///
    /// Silent when no [`HostServices`] backend is attached, which is the same no-op pi's
    /// `noOpUIContext` gives a headless run.
    fn notify_save_failure(&self, summary: &str, cause: &str) {
        if let Some(services) = self.host_services.get() {
            services.notify(
                &format!("{summary}\n{}\n{cause}", self.config_path_line()),
                NotifyKind::Error,
            );
        }
    }

    /// PERM-007 — hand [`crate::config_modal::PermissionSystemSettingsOverlay`] to the host and
    /// block until the human closes it, then report whatever the overlay could not commit.
    ///
    /// Returns `None` when the overlay ran (the human has already seen everything on screen, so a
    /// trailing Info toast would be noise — the `Ok(None)` convention on
    /// [`cyrup_ext::NativeExtension::execute_command`]), and `Some(text)` with the read-only dump
    /// when no interactive surface took it. [`cyrup_ext::HostServices::open_overlay`] returning
    /// `false` is exactly pi's `if (!ctx.hasUI)` case, not a failure.
    fn open_settings_overlay(&self) -> Option<String> {
        let Some(services) = self.host_services.get() else {
            return Some(self.render_settings());
        };
        let overlay = Box::new(crate::config_modal::PermissionSystemSettingsOverlay::new(
            self.config_controller(),
        ));
        // `open_overlay` consumes the box, so the commit failure cannot be read back off it. It is
        // read off the CONTROLLER's own last-error slot instead, which the overlay writes through.
        let controller = self.config_controller();
        if !services.open_overlay(overlay) {
            return Some(self.render_settings());
        }
        // pi's modal notifies inline through `ctx.ui.notify` while it is still on screen
        // (`index.ts:1407`); cyrup's overlay owns the whole screen and has already shown the cause,
        // so the toast here is the SAME one the text path raises and only for a failure that
        // survived to the close.
        if let Some(cause) = controller.take_last_error() {
            self.notify_save_failure(
                "Failed to save the permission-system config; the last change was not applied.",
                &cause,
            );
        }
        None
    }

    /// The modal's initial view (pi `buildSettingItems`, `config-modal.ts:24-41`, plus its
    /// `helpText: \`Config file: ${controller.getConfigPath()}\``, `:85`), as text.
    fn render_settings(&self) -> String {
        let config = guard(&self.config).clone();
        format!(
            "Permission System Settings\n  debug     {:<3}  Debug logging\n  yoloMode  {:<3}  YOLO \
             mode\n{}\n{}\n{COMMAND_USAGE}",
            on_off(config.debug),
            on_off(config.yolo_mode),
            self.config_path_line(),
            // PERM-029: name the policy file and its schema alongside the extension-config path,
            // upstream's `README.md:659` advice made reachable from the app.
            format_args!(
                "Policy file: {}\n  `/permission-system schema` prints the JSON Schema; \
                 `/permission-system example` prints a starter policy.",
                policy_agent_dir(&self.agent_dir).join(POLICY_FILE).display()
            )
        )
    }

    /// pi `helpText: \`Config file: ${controller.getConfigPath()}\`` (`config-modal.ts:85`, over
    /// `getPermissionSystemConfigPath`, `index.ts:1509`) — the RESOLVED path, so the
    /// `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` override is what the human is told about.
    fn config_path_line(&self) -> String {
        format!("Config file: {}", Self::resolved_config_path_for(&self.agent_dir).display())
    }

    /// Assemble from explicit parts (used by tests that point the global policy path at a fixture file
    /// / inject a scripted ask channel). Derives `agent_dir` from the policy path's parent; installs no
    /// watcher and a fresh capability slot.
    #[must_use]
    pub fn from_parts(
        paths: ManagerPaths,
        config: ExtensionConfig,
        ask_channel: Arc<dyn AskChannel>,
    ) -> Self {
        let agent_dir = paths
            .global_config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self::from_parts_full(
            paths,
            config,
            |_| ask_channel,
            agent_dir,
            false,
            Arc::new(OnceLock::new()),
        )
    }

    /// The one true assembler every constructor funnels through.
    #[must_use]
    fn from_parts_full(
        paths: ManagerPaths,
        config: ExtensionConfig,
        // A BUILDER rather than a value: the child's `ForwardingAskChannel` needs the shared
        // `AuditTrail` (PERM-008), which cannot exist until `shared_config` does, which is built
        // here. Every other constructor ignores the argument.
        ask_channel: impl FnOnce(&Arc<crate::logging::AuditTrail>) -> Arc<dyn AskChannel>,
        agent_dir: PathBuf,
        install_watcher: bool,
        host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    ) -> Self {
        // Built BEFORE the struct literal: `host_services` is moved into the literal below, and the
        // sink needs its own handle on the same `OnceLock` so the manager's `onWarning` binding
        // observes the backend the host attaches LATER (`set_host_services` runs after
        // construction).
        let warnings = Arc::new(WarningSink::new(Arc::clone(&host_services)));
        // Built here so the logger and `self.config` are the SAME `Arc` — pi's `extensionLogger`
        // reads the module-scope `extensionConfig` binding `refreshExtensionConfig` reassigns
        // (`index.ts:146-150`), so a reload must be observable through both.
        let shared_config: crate::forwarding::SharedExtensionConfig = Arc::new(Mutex::new(config));
        let logger = Arc::new(crate::logging::AuditTrail::new(
            crate::logging::PermissionSystemLogger::new(
                Arc::clone(&shared_config),
                Self::logs_dir_for(&agent_dir),
            ),
        ));
        // pi `setLoggingWarningReporter(...)` (`index.ts:170-172`): the reporter is the SAME
        // `notifyWarning` sink every other warning uses. Installed here rather than at
        // `set_host_services` because `WarningSink` is itself late-bound on the `OnceLock`.
        {
            let sink = Arc::clone(&warnings);
            logger.set_reporter(Arc::new(move |message: &str| sink.notify(message)));
        }
        let ask_channel = ask_channel(&logger);
        // PERM-007: built here so the controller and this extension share ONE `config` cell, ONE
        // `lastConfigWarning` memo, ONE host-services slot and ONE audit trail — pi's module-scope
        // bindings, made explicit (the same shape PERM-008 gave `AuditTrail`).
        let last_config_warning = Arc::new(Mutex::new(None));
        let controller = Arc::new(crate::config_modal::ConfigController::new(
            Arc::clone(&shared_config),
            agent_dir.clone(),
            Arc::clone(&last_config_warning),
            Arc::clone(&host_services),
            Arc::clone(&logger),
        ));
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            manager: Mutex::new(manager_with_warnings(paths, &warnings)),
            session_approvals: Mutex::new(SessionApprovalStore::new()),
            dedup: Mutex::new(DedupCache::new()),
            config: shared_config,
            ask_channel,
            host_services,
            agent_dir,
            install_watcher,
            watcher: Mutex::new(None),
            agent_name: resolve_agent_name_from_env(),
            active_skill_entries: Mutex::new(Vec::new()),
            agent_start_cache: Mutex::new(AgentStartCache::default()),
            explicitly_requested_skill_names: Mutex::new(HashSet::new()),
            warnings,
            last_config_warning,
            controller,
            logger,
            has_ui: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// pi `writeDebugEntry` (`index.ts:171-176`): the diagnostic stream, with the logger's own
    /// failure funnelled into the dedup-once warning reporter.
    fn write_debug_entry(&self, event: &str, details: &Value) {
        self.logger.debug(event, details);
    }

    /// pi `writeReviewEntry` (v0.8.0 `index.ts:200-202`, via `writeLogEntry` `:183-194`): the
    /// SECURITY-relevant decision stream — the
    /// "why was this blocked / who approved this" trail. Same warning funnel.
    fn write_review_entry(&self, event: &str, details: &Value) {
        self.logger.review(event, details);
    }

    /// pi `reviewPermissionDecision` (`index.ts:1767-1793`): the ONE shaped `review` record every
    /// decision-point entry is built from — the prompt and denial reason accompanied by their
    /// `createSensitiveLogMetadata` digests, plus the resolution / persistence / scope fields.
    ///
    /// `details` is the same [`DedupDetails`] the dedup fingerprint is built from, which already
    /// mirrors pi's `PermissionPromptDetails` field for field (`dedup.rs:36-50`).
    fn review_permission_decision(&self, event: &str, details: &DedupDetails, tail: Value) {
        let mut record = json!({
            "requestId": details.request_id,
            "source": details.source,
            "agentName": details.agent_name,
            "prompt": details.message,
            "promptMetadata": crate::logging::sensitive_log_metadata(Some(&details.message)),
            "toolCallId": details.tool_call_id,
            "toolName": details.tool_name,
            "skillName": details.skill_name,
            "path": details.path,
            "command": details.command,
            "commandMetadata": crate::logging::sensitive_log_metadata(details.command.as_deref()),
            "target": details.target,
            "toolInput": details.tool_input,
        });
        // pi spreads `...details` then the per-call-site resolution/persistence keys; the tail
        // overwrites, matching JS object-literal ordering.
        if let (Value::Object(base), Value::Object(extra)) = (&mut record, &tail) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        self.write_review_entry(event, &record);
    }

    /// pi `getPermissionDecisionScope` (v0.8.0 `index.ts:581-592`): the first non-empty of
    /// `target`, `command`, `path`, `toolName`, `skillName`.
    ///
    /// **PERM-028 — the first three go through `getNonEmptyString`, the last two do not.** Upstream
    /// is `getNonEmptyString(details.target) ?? getNonEmptyString(details.command) ??
    /// getNonEmptyString(details.path) ?? details.toolName ?? details.skillName ?? null`, and
    /// `getNonEmptyString` TRIMS (`common.ts:15-22`). So `command: "  git status  "` keys as
    /// `"git status"` upstream, and a whitespace-ONLY command is skipped entirely rather than
    /// selected. Cyrup previously filtered on a raw `!is_empty()` across all five, which both kept
    /// the padding and let `"   "` win. The asymmetry is deliberate and is upstream's: do not
    /// "tidy" it by trimming `toolName`/`skillName` too.
    fn permission_decision_scope(details: &DedupDetails) -> Value {
        // pi's first three arms — `getNonEmptyString` = trim, then drop if empty
        // (`common::get_non_empty_string`, `common.rs:20`).
        let trimmed = [details.target.as_deref(), details.command.as_deref(), details.path.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty());
        if let Some(s) = trimmed {
            return Value::String(s.to_string());
        }
        // pi's last two arms — RAW `??` fallthrough, no trim and no empty check. `??` skips only
        // `null`/`undefined`, which is `Option::None` here, so an empty-string `toolName` is
        // selected upstream and must be selected here.
        [details.tool_name.as_deref(), details.skill_name.as_deref()]
            .into_iter()
            .flatten()
            .next()
            .map_or(Value::Null, |s| Value::String(s.to_string()))
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
            return HookOutcome::Block { reason: Some(gate::format_missing_tool_name_reason()), terminate: false };
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
            return HookOutcome::Block { reason: Some(reason), terminate: false };
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
        // The per-call identity every gated layer audits against (pi threads `event.toolCallId` /
        // `toolName` / `input` / `ctx.cwd` / `agentName` into each `writeReviewEntry` by hand).
        let call = GateCall { call_id, tool_name: normalized, input, cwd: &cwd, agent_name };

        if normalized == "read"
            && let Some(outcome) = self.resolve_skill_read(&call, ctx).await
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
            && let Some(outcome) = self.resolve_external_directory(&call, &path, ctx).await
        {
            return outcome;
        }

        // Main check + store overlay — fully synchronous; every lock is dropped before any await.
        let check = {
            let session_rules = guard(&self.session_approvals).get_rules();
            let raw = guard(&self.manager).check_permission(normalized, input, agent_name);
            gate::apply_pattern_approval_state(raw, input, &session_rules)
        };

        match check.state {
            PermissionState::Deny => {
                // pi `index.ts:2422-2439`: the policy-denied audit entry, then `flush()` before the
                // block is returned (`[CYRUP-DELTA]` — the write is already durable here).
                let details = dedup_details(call_id, input, &check, agent_name);
                self.review_permission_decision(
                    "permission_request.blocked",
                    &details,
                    json!({
                        "source": "tool_call",
                        "resolution": "policy_denied",
                        "decisionPersistence": "none",
                        "decisionScope": Self::permission_decision_scope(&details),
                    }),
                );
                self.logger.flush();
                HookOutcome::Block { reason: Some(gate::format_deny_reason(&check, agent_name)), terminate: false }
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
    async fn resolve_skill_read(&self, call: &GateCall<'_>, ctx: &HostCtx) -> Option<HookOutcome> {
        let GateCall { call_id, tool_name, input, cwd, agent_name } = *call;
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
                    // pi `index.ts:2243-2255`.
                    self.write_review_entry(
                        "permission_request.blocked",
                        &json!({
                            "source": "skill_read",
                            "toolCallId": call_id,
                            "toolName": tool_name,
                            "skillName": read_skill.name,
                            "agentName": agent_name,
                            "path": read_path,
                            "toolInput": input,
                            "resolution": "policy_denied",
                        }),
                    );
                    return Some(HookOutcome::Block {
                        reason: Some(skill::format_skill_path_deny_reason(&read_skill, agent_name)),
                        terminate: false,
                    });
                }
                PermissionState::Ask => {
                    let message =
                        skill::format_skill_path_ask_prompt(&read_skill, &read_path, agent_name);
                    // pi `index.ts:2282-2291`'s `promptPermission` details record.
                    let details = DedupDetails {
                        request_id: call_id.to_string(),
                        source: "skill_read".to_string(),
                        agent_name: agent_name.map(str::to_string),
                        message: message.clone(),
                        tool_call_id: Some(call_id.to_string()),
                        tool_name: Some(tool_name.to_string()),
                        skill_name: Some(read_skill.name.clone()),
                        path: Some(read_path.clone()),
                        command: None,
                        target: None,
                        tool_input: input.clone(),
                    };
                    match self.prompt_decision(&details, ctx).await {
                        AskOutcome::NoLiveChannel => {
                            // pi `index.ts:2262-2276`.
                            self.write_review_entry(
                                "permission_request.blocked",
                                &json!({
                                    "source": "skill_read",
                                    "toolCallId": call_id,
                                    "toolName": tool_name,
                                    "skillName": read_skill.name,
                                    "agentName": agent_name,
                                    "path": read_path,
                                    "prompt": message,
                                    "promptMetadata": crate::logging::sensitive_log_metadata(Some(&message)),
                                    "toolInput": input,
                                    "resolution": "confirmation_unavailable",
                                }),
                            );
                            return Some(HookOutcome::Block {
                                reason: Some(skill::skill_ask_unavailable_reason()),
                                terminate: false,
                            });
                        }
                        AskOutcome::Decided(d) if !d.approved => {
                            return Some(HookOutcome::Block {
                                reason: Some(skill::format_skill_user_denied_reason(
                                    d.denial_reason.as_deref(),
                                )),
                                terminate: false,
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
    /// special policy for `{path, cwd}` (with the session overlay applied on an `ask`): `deny`
    /// → block; `ask` → live prompt (fail-closed / user-deny → block; approved-Always → session-persist,
    /// then fall through); `allow` → fall through. `None` = allowed (proceed to the main check).
    async fn resolve_external_directory(
        &self,
        call: &GateCall<'_>,
        path: &str,
        ctx: &HostCtx,
    ) -> Option<HookOutcome> {
        let GateCall { call_id, tool_name, input, cwd, agent_name } = *call;
        let ext_input = json!({ "path": path, "cwd": cwd });
        let raw = guard(&self.manager).check_permission("external_directory", &ext_input, agent_name);
        // pi `:2319-2321`: the session overlay is applied ONLY on an `ask` result.
        let ext_check = if raw.state == PermissionState::Ask {
            let session_rules = guard(&self.session_approvals).get_rules();
            gate::apply_pattern_approval_state(raw, &ext_input, &session_rules)
        } else {
            raw
        };

        match ext_check.state {
            PermissionState::Deny => {
                // pi `index.ts:2323-2333`.
                self.write_review_entry(
                    "permission_request.blocked",
                    &json!({
                        "source": "tool_call",
                        "toolCallId": call_id,
                        "toolName": tool_name,
                        "agentName": agent_name,
                        "path": path,
                        "toolInput": input,
                        "resolution": "policy_denied",
                    }),
                );
                Some(HookOutcome::Block {
                    reason: Some(gate::format_external_directory_deny_reason(
                        tool_name, path, cwd, agent_name,
                    )),
                    terminate: false,
                })
            }
            PermissionState::Ask => {
                let message =
                    gate::format_external_directory_ask_prompt(tool_name, path, cwd, agent_name);
                // pi `index.ts:2368-2377`'s `promptPermission` details record — note `source` is
                // `"tool_call"` here, not `"skill_read"`, and no `skillName`/`command`/`target`.
                let details = DedupDetails {
                    request_id: call_id.to_string(),
                    source: "tool_call".to_string(),
                    agent_name: agent_name.map(str::to_string),
                    message: message.clone(),
                    tool_call_id: Some(call_id.to_string()),
                    tool_name: Some(tool_name.to_string()),
                    skill_name: None,
                    path: Some(path.to_string()),
                    command: None,
                    target: None,
                    tool_input: input.clone(),
                };
                match self.prompt_decision(&details, ctx).await {
                    AskOutcome::NoLiveChannel => {
                        // pi `index.ts:2351-2362`.
                        self.write_review_entry(
                            "permission_request.blocked",
                            &json!({
                                "source": "tool_call",
                                "toolCallId": call_id,
                                "toolName": tool_name,
                                "agentName": agent_name,
                                "path": path,
                                "prompt": message,
                                "promptMetadata": crate::logging::sensitive_log_metadata(Some(&message)),
                                "toolInput": input,
                                "resolution": "confirmation_unavailable",
                            }),
                        );
                        Some(HookOutcome::Block {
                            reason: Some(gate::format_external_directory_unavailable_reason(path)),
                            terminate: false,
                        })
                    }
                    AskOutcome::Decided(d) if !d.approved => Some(HookOutcome::Block {
                        reason: Some(gate::format_external_directory_user_denied_reason(
                            tool_name,
                            path,
                            d.denial_reason.as_deref(),
                        )),
                        terminate: false,
                    }),
                    AskOutcome::Decided(d) => {
                        // pi `persistPatternApprovalDecision` (`:2391`): an approved-Always persists an
                        // allow rule to the SESSION store, then the call FALLS THROUGH to the main check.
                        if d.state == PermissionDecisionState::Always {
                            let subject = gate::get_pattern_approval_subject(&ext_check, &ext_input);
                            if !subject.is_empty() {
                                guard(&self.session_approvals)
                                    .approve_always(&ext_check.tool_name, &subject);
                                // pi `index.ts:2397-2409`: the persist is audited only when a
                                // subject was actually recorded, and names the SPECIAL tool
                                // `external_directory` rather than the calling tool.
                                self.write_review_entry(
                                    "permission_request.approval_persisted",
                                    &json!({
                                        "source": "tool_call",
                                        "toolCallId": call_id,
                                        "toolName": "external_directory",
                                        "agentName": agent_name,
                                        "path": path,
                                        "toolInput": input,
                                        "resolution": decision_state_str(d.state),
                                        "decisionPersistence": "session",
                                        "approvalPersistence": "session",
                                        "approvalScope": subject,
                                    }),
                                );
                                self.logger.flush();
                            }
                        }
                        None
                    }
                }
            }
            PermissionState::Allow => None,
        }
    }

    /// Settle an in-flight dedup registration with the decision that resolved it (pi's
    /// `decisionPromise` fulfilling, observed by `rememberPermissionPromptDecision`'s stored promise
    /// at v0.8.0 `index.ts:1633`). A `None` owner is pi's uncacheable case (empty `requestId`,
    /// `createPermissionPromptCacheKey` `index.ts:472-481`) — nothing was registered and nothing is
    /// stored.
    fn resolve_prompt_decision(
        &self,
        owner: Option<crate::dedup::PendingOwner>,
        decision: &PermissionPromptDecision,
    ) {
        if let Some(owner) = owner {
            owner.resolve(&mut guard(&self.dedup), decision.clone());
        }
    }

    /// pi `forgetPermissionPromptDecision` in `promptPermission`'s catch
    /// (v0.8.0 `index.ts:1638-1642`): the prompt never produced a decision, so the in-flight
    /// registration must be dropped rather than left latched — otherwise every later identical
    /// request would await a promise that will never settle.
    fn forget_prompt_decision(&self, owner: Option<crate::dedup::PendingOwner>) {
        if let Some(owner) = owner {
            owner.forget(&mut guard(&self.dedup));
        }
    }

    /// pi `promptPermission` (`index.ts:1794-1902`), the shared prompting core EVERY ask surface goes
    /// through: the dedup cache, then the `canResolveAskPermissionRequest` fail-fast pre-check
    /// (`yolo-mode.ts:21-23`, consulted via `canRequestPermissionConfirmation` BEFORE any prompt/lock
    /// work at `index.ts:2263,2351,2452`) — `hasUI || isSubagent || yoloMode` — then yolo auto-approve
    /// (pi `shouldAutoApprovePermissionState`), the C3 human-interaction lock, the live-vs-fallback
    /// channel selection, and the P-3 dispatch-budget-forgiveness guard held across the BLOCKING
    /// dialog. `AskOutcome::NoLiveChannel` = fail-CLOSED (no reachable human), returned IMMEDIATELY by
    /// the pre-check when none of the three conditions hold — zero lock/dialog work touched, exactly
    /// like pi's early return.
    ///
    /// The DEDUP cache lives here, not in any one caller, because pi puts it inside `promptPermission`
    /// itself (`index.ts:1798-1815` lookup, `:1890-1892` store): all three ask surfaces — skill-read
    /// (`index.ts:2282`), external-directory (`:2369`) and the main check (`:2469`) — are therefore
    /// deduplicated identically, so a re-emitted IDENTICAL `tool_call` renders ZERO additional prompts
    /// on ANY of them (`tests/edit-decision-deduplication-red.test.ts` is upstream's regression proof).
    ///
    /// Also emits pi `promptPermission`'s five audit entries (`index.ts:1805,1820,1843,1855-1857`):
    /// `permission_request.duplicate_reused` (cache hit), `.auto_approved` (yolo), `.waiting` (before
    /// the dialog opens) and `.approved`/`.denied` (after the human answers). `details` is pi's
    /// `PermissionPromptDetails` — `details.message` IS the prompt text, so this takes the record
    /// rather than a bare string, and is also what the cache key is fingerprinted from.
    async fn prompt_decision(&self, details: &DedupDetails, ctx: &HostCtx) -> AskOutcome {
        let message = details.message.as_str();
        let yolo_mode = guard(&self.config).yolo_mode;
        if !(ctx.has_ui || is_subagent_child() || yolo_mode) {
            // The caller's `confirmation_unavailable` entry covers this branch (pi audits it at
            // each of its three `canRequestPermissionConfirmation` sites, not inside
            // `promptPermission`). Ordered BEFORE the cache lookup to match pi, whose callers run
            // `canRequestPermissionConfirmation` before ever entering `promptPermission`.
            return AskOutcome::NoLiveChannel;
        }

        // Dedup hit: reuse the prior decision (collapsed to Allow-Once by `create_duplicate_decision`,
        // so a re-emitted approval never re-persists an `Always` grant) — zero additional prompts.
        //
        // PERM-014 — this is `lookup`, not `get`, and the difference is the whole item. pi's cache
        // stores the still-unsettled `decisionPromise` (`index.ts:1633`, run BEFORE the `await` at
        // `:1637`), so a CONCURRENT identical ask hits `getCachedPermissionPromptDecision`
        // (`:1581-1583`) and `await`s that same promise (`:1585`) instead of opening a second
        // dialog. `get` treated an in-flight entry as a miss, so two concurrently-executing tool
        // calls with the same dedup key each raised their own prompt and the operator answered the
        // same question twice — with nothing making the two answers agree.
        let key = details.cache_key();
        if let Some(k) = &key {
            let cached = guard(&self.dedup).lookup(k);
            let cached = match cached {
                // pi `:1585` `createDuplicatePermissionPromptDecision(await cachedDecision)` — the
                // already-settled arm.
                Some(crate::dedup::Lookup::Ready(decision)) => Some(decision),
                // The same statement's OTHER arm: `cachedDecision` is a pending promise, so the
                // `await` blocks here until the owner settles it. The lock is released first —
                // `lookup` returned an owned `Pending` precisely so nothing is held across it.
                Some(crate::dedup::Lookup::Pending(pending)) => Some(pending.wait().await),
                None => None,
            };
            if let Some(decision) = cached {
                // pi `index.ts:1804-1812`: a reused decision is STILL audited — otherwise a
                // re-emitted tool call looks like it was never gated at all.
                self.review_permission_decision(
                    "permission_request.duplicate_reused",
                    details,
                    json!({
                        "resolution": decision_state_str(decision.state),
                        "denialReason": decision.denial_reason,
                        "denialReasonMetadata":
                            crate::logging::sensitive_log_metadata(decision.denial_reason.as_deref()),
                        "decisionPersistence": "none",
                        "approvalPersistence": "none",
                        "decisionScope": Self::permission_decision_scope(details),
                    }),
                );
                self.logger.flush();
                return AskOutcome::Decided(decision);
            }
        }

        // pi `rememberPermissionPromptDecision(..., decisionPromise)` (v0.8.0 `index.ts:1632-1634`)
        // — registered BEFORE the body below runs, which is why the yolo arm and the dialog arm are
        // BOTH inside the window a concurrent duplicate can join. Settled by
        // `resolve_prompt_decision` on every path that produces a decision, and dropped by
        // `forget_prompt_decision` on the one that does not (pi's catch, `:1638-1642`).
        let owner = key.as_ref().map(|k| guard(&self.dedup).begin_pending(k));

        if yolo_mode {
            // pi `index.ts:1598-1608`.
            self.review_permission_decision(
                "permission_request.auto_approved",
                details,
                json!({
                    "resolution": "auto_response",
                    "decisionPersistence": "none",
                    "decisionScope": "yolo_mode",
                }),
            );
            self.logger.flush();
            let decision = PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Approved,
                denial_reason: None,
            };
            // pi caches the yolo auto-approval too: `rememberPermissionPromptDecision`
            // (`index.ts:1633`) is handed the SAME `decisionPromise` whose body took the
            // `shouldAutoApprovePermissionState` early return at `:1599-1609`.
            self.resolve_prompt_decision(owner, &decision);
            return AskOutcome::Decided(decision);
        }
        // pi `index.ts:1843` — recorded BEFORE the dialog opens, so a session killed mid-prompt
        // still leaves evidence of what was asked.
        self.review_permission_decision("permission_request.waiting", details, json!({}));
        let human_lock = self.host_services.get().and_then(|s| s.human_interaction_lock());
        let _human_guard = match human_lock {
            Some(lock) => Some(lock.acquire().await),
            None => None,
        };
        let channel: Arc<dyn AskChannel> = match (ctx.has_ui, self.host_services.get()) {
            (true, Some(services)) => Arc::new(LocalAskChannel::new(services.clone())),
            _ => self.ask_channel.clone(),
        };
        let outcome = {
            let _human_wait = ctx.begin_human_wait();
            channel.confirm("Permission Required", message, PromptOpts::default()).await
        };

        // pi `index.ts:1855-1868`: the resolved decision, with the "Allow Always" session-persist
        // intent recorded alongside it.
        if let AskOutcome::Decided(ref d) = outcome {
            let always = d.state == PermissionDecisionState::Always;
            let scope = Self::permission_decision_scope(details);
            self.review_permission_decision(
                if d.approved { "permission_request.approved" } else { "permission_request.denied" },
                details,
                json!({
                    "resolution": decision_state_str(d.state),
                    "denialReason": d.denial_reason,
                    "denialReasonMetadata":
                        crate::logging::sensitive_log_metadata(d.denial_reason.as_deref()),
                    "decisionPersistence": if always { "session" } else { "none" },
                    "approvalPersistence": if d.approved && always { "session" } else { "none" },
                    "decisionScope": scope,
                    "approvalScope": if d.approved && always { scope.clone() } else { Value::Null },
                }),
            );
            // pi `:1637` — the `decisionPromise` settles, so the entry registered above flips from
            // pending to resolved and BOTH the next identical request and any concurrent follower
            // already awaiting it see this decision.
            self.resolve_prompt_decision(owner, d);
        } else {
            // No decision was produced (no reachable human). pi's `confirmPermission` cannot reach
            // this shape — it always resolves to `{approved:false}` — but cyrup's channel can
            // return `NoLiveChannel`, which the CALLER turns into a fail-closed block. The
            // registration must not be left latched: pi's catch arm is
            // `forgetPermissionPromptDecision` (`:1638-1642`), and a follower blocked on
            // `Pending::wait` fails CLOSED when the owner's sender drops here.
            self.forget_prompt_decision(owner);
        }
        outcome
    }

    /// The main-check `ask` branch (pi `:2444-2496` + `confirmPermission :1506-1513`): the shared
    /// [`Self::prompt_decision`] core (pi `promptPermission :1794-1902` — dedup lookup → yolo → C3
    /// human-interaction lock → live dialog under a P-3 budget-forgiveness guard → dedup store) →
    /// fail-CLOSED when no human is reachable → apply (the `Always` session-persist tail). The prompt
    /// subject names the resolved persona (real `agent_name`, pi `formatAskPrompt(check, agentName,
    /// input)`). Dedup is NOT done here: pi keeps it inside `promptPermission` so every ask surface
    /// shares it, and cyrup follows (see [`Self::prompt_decision`]).
    async fn resolve_ask(
        &self,
        call_id: &str,
        input: &Value,
        check: &PermissionCheckResult,
        ctx: &HostCtx,
    ) -> HookOutcome {
        let agent_name = self.agent_name.as_deref();

        let details = dedup_details(call_id, input, check, agent_name);

        // pi `formatAskPrompt` (`index.ts:570-590`) — the human-facing prompt (NOT the headless reason).
        // The shared prompting core applies the dedup cache, yolo auto-approve (pi
        // `shouldAutoApprovePermissionState`), the C3 human lock, the live-vs-fallback channel, and the
        // P-3 dispatch-budget guard. `details.message` already IS `format_ask_prompt(check, agent_name,
        // input)` (built by `dedup_details` above), which is what `prompt_decision` prompts with.
        let decision = match self.prompt_decision(&details, ctx).await {
            AskOutcome::Decided(d) => d,
            // Fail-CLOSED: no reachable human (headless / no live UI) → Block, never proceed
            // (pi `confirmPermission` headless `{approved:false}` :1509-1513 / `:2452-2467`).
            AskOutcome::NoLiveChannel => {
                // pi `index.ts:2452-2464`.
                self.review_permission_decision(
                    "permission_request.blocked",
                    &details,
                    json!({ "source": "tool_call", "resolution": "confirmation_unavailable" }),
                );
                return HookOutcome::Block { reason: Some(gate::format_ask_unavailable_reason(check)), terminate: false };
            }
        };

        // pi `index.ts:2481-2494`: audit the SESSION persist an approved-Always produces (only when
        // a real subject was recorded), then `flush()`.
        if decision.approved && decision.state == PermissionDecisionState::Always {
            let subject = gate::get_pattern_approval_subject(check, input);
            if !subject.is_empty() {
                self.review_permission_decision(
                    "permission_request.approval_persisted",
                    &details,
                    json!({
                        "source": "tool_call",
                        "resolution": decision_state_str(decision.state),
                        "decisionPersistence": "session",
                        "approvalPersistence": "session",
                        "approvalScope": subject,
                    }),
                );
            }
        }
        self.logger.flush();
        self.apply_decision(decision, check, input)
    }

    /// Apply a resolved decision (pi `:2478-2495`): not-approved → Block (`formatUserDeniedReason`);
    /// approved-Always → persist an allow rule to the SESSION store — the ONLY approval sink there is
    /// (pi v0.8.0 `index.ts:610`, `persistSessionApprovalDecision`; the cross-session
    /// `PermanentApprovalStore` was deleted upstream in v0.8.0, see [`crate::stores`]). The `Always`
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
                terminate: false,
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
    /// **PERM-013 — both cache layers are ported** (pi `before-agent-start-cache.ts`, consumed at
    /// `index.ts:1894-1898` and `:1900-1913`): step 1's `set_active_tools` fires only when the
    /// exposed tool list actually changed, and steps 2+3 are skipped wholesale on a prompt-state
    /// key hit, replaying the cached entries + prompt. Both keys are invalidated together by
    /// [`Self::invalidate_agent_start_cache`].
    ///
    /// The status pill is NOT synced here: pi's `before_agent_start` reaches it through
    /// `refreshExtensionConfig(ctx)` (`index.ts:1877` → `applyExtensionConfigSideEffects`
    /// `:1364-1366`), which cyrup's `BeforeAgentStart` arm now calls before this function
    /// (PERM-024 / PERM-026). Syncing again here would be a second write of the same value.
    fn on_before_agent_start(&self, system_prompt: &str, ctx: &HostCtx) -> HookOutcome {
        let cwd = ctx.cwd.to_string_lossy().into_owned();
        let agent = self.agent_name.as_deref();
        let services = self.host_services.get();

        // (1) Active-tools exposure — only when the live backend can enumerate the FULL registry.
        //
        // \[CYRUP-DELTA] pi has no "registry unavailable" case (`pi.getAllTools()` always returns a
        // list), so the `None` arm below is cyrup-only: the exposed set cannot be computed, the
        // tools section is left intact, and the agent-start cache is BYPASSED entirely rather than
        // keyed on an empty tool list — which would be indistinguishable from a registry that
        // legitimately exposes nothing, and would replay the wrong prompt if the backend attached
        // between turns.
        let allowed: Option<Vec<String>> = services.and_then(|s| s.all_tool_names()).map(|tools| {
            tools.into_iter().filter(|name| self.should_expose_tool(name, agent)).collect()
        });

        let Some(allowed) = allowed else {
            return self.shape_agent_start_prompt(system_prompt, system_prompt, agent, &cwd, None);
        };

        // pi `:1894-1898`: `setActiveTools` runs ONLY when the tool-list key changed.
        let active_tools_key = agent_start_cache::create_active_tools_cache_key(&allowed);
        {
            let mut cache = guard(&self.agent_start_cache);
            if agent_start_cache::should_apply_cached_agent_start_state(
                cache.last_active_tools_key.as_deref(),
                &active_tools_key,
            ) {
                if let Some(s) = services {
                    s.set_active_tools(&allowed);
                }
                cache.last_active_tools_key = Some(active_tools_key);
            }
        }

        // pi `:1900-1907`: the prompt-state key. `permissionStamp` is what makes a mid-session
        // policy edit invalidate this — see [`PermissionManager::policy_cache_stamp`].
        let permission_stamp = guard(&self.manager).policy_cache_stamp(agent);
        let prompt_state_key = agent_start_cache::create_prompt_state_key(&PromptStateKeyInput {
            agent_name: agent,
            cwd: &cwd,
            permission_stamp: &permission_stamp,
            system_prompt,
            allowed_tool_names: &allowed,
        });

        // pi `:1908-1913`: on a key HIT with a recorded result, restore the skill entries and
        // return the cached prompt without re-running either sanitizer.
        let cached_hit = {
            let cache = guard(&self.agent_start_cache);
            if agent_start_cache::should_apply_cached_agent_start_state(
                cache.last_prompt_state_key.as_deref(),
                &prompt_state_key,
            ) {
                None
            } else {
                cache.last_prompt_state_result.clone()
            }
        };
        if let Some(cached) = cached_hit {
            *guard(&self.active_skill_entries) = cached.entries;
            return match cached.system_prompt {
                None => HookOutcome::Noop,
                Some(prompt) => HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                    system: Some(prompt),
                    inject: None,
                }),
            };
        }

        // (2) Strip the "Available tools:" section + denied-tool guideline bullets (pi `:1915`).
        let working_prompt =
            sanitize::tools::sanitize_available_tools_section(system_prompt, &allowed).prompt;
        self.shape_agent_start_prompt(
            system_prompt,
            &working_prompt,
            agent,
            &cwd,
            Some(prompt_state_key),
        )
    }

    /// The tail of [`Self::on_before_agent_start`] (pi `index.ts:1916-1930`): resolve the skill
    /// prompt entries over `working_prompt`, install them as the enforcement cache, record the
    /// `CachedPromptStateResult` under `prompt_state_key` when one was computed, and return the
    /// sanitized prompt as a `[mutate]` only when it differs from the ORIGINAL `system_prompt`
    /// (pi `skillPromptResult.prompt !== event.systemPrompt`, `:1922`).
    ///
    /// `prompt_state_key: None` is the cyrup-only registry-unavailable path: shape, but record no
    /// cache entry.
    fn shape_agent_start_prompt(
        &self,
        system_prompt: &str,
        working_prompt: &str,
        agent: Option<&str>,
        cwd: &str,
        prompt_state_key: Option<String>,
    ) -> HookOutcome {
        // (3) Hide ask/deny skills from `<available_skills>` + cache the enforcement entries. ONE
        // parse feeds both the enforcement cache (read at every `tool_call`) and the hidden prompt.
        let resolution = {
            let mut mgr = guard(&self.manager);
            sanitize::skills::resolve_skill_prompt_entries(working_prompt, &mut mgr, agent, cwd)
        };
        // pi `:1919` `activeSkillEntries = skillPromptResult.entries`.
        *guard(&self.active_skill_entries) = resolution.entries.clone();

        // pi `:1921-1924`: `systemPrompt` is ABSENT (not null) when the sanitizers changed nothing,
        // which is what decides between `{ systemPrompt }` and `{}` on both the fresh and the
        // cached path.
        let changed = (resolution.prompt != system_prompt).then_some(resolution.prompt);
        if let Some(key) = prompt_state_key {
            let mut cache = guard(&self.agent_start_cache);
            cache.last_prompt_state_key = Some(key);
            cache.last_prompt_state_result = Some(CachedPromptState {
                entries: resolution.entries,
                system_prompt: changed.clone(),
            });
        }

        match changed {
            None => HookOutcome::Noop,
            Some(prompt) => HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                system: Some(prompt),
                inject: None,
            }),
        }
    }

    /// pi `shouldExposeTool` (`index.ts:1791-1816` @v0.8.0; `:2049-2075` @v0.7.1 — the two are the
    /// same function, only `permanentApprovals` was dropped from the `applyPatternApprovalState`
    /// call): keep a tool exposed iff its TOOL-LEVEL permission
    /// ([`PermissionManager::get_tool_permission`]) — with the session approval overlay (pi
    /// `applyPatternApprovalState(..., {}, ...)`, `:1795-1803`) — is not `deny`. There is **exactly
    /// one** bypass below that: a `deny` `read` is still exposed when the agent has allowed skills
    /// ([`PermissionManager::has_allowed_skills`], pi `:1811-1813`) so it can reach skill files.
    /// Everything else denied at the tool level falls through to `false` (pi `:1815`).
    ///
    /// **No bash arm — deliberately (`PERM-009`).** Cyrup previously carried
    /// `if tool_name == "bash" && get_bash_permissions(agent).any_allow() { return true; }` here.
    /// Neither upstream tag has any such branch, and it was a **live permission bypass in the
    /// shipped binary**, reproduced end-to-end (`docs/gap-analysis/REPRO-LOG.md` §`PERM-009`):
    /// `tools.bash: deny` alone correctly withheld the tool, but adding the strictly NARROWER
    /// `bash: {"git status": "allow"}` to the same file re-exposed `bash`, and
    /// [`PermissionManager::check_permission`]'s bash arm then resolved that command rule ABOVE the
    /// tool-level deny (`manager.rs`, pi `permission-manager.ts:944-959`), so real `git status`
    /// output came back. A rule that can only ever NARROW an allow must never widen a deny. The
    /// command-rule-over-`toolMatch` precedence in `manager.rs` is pi's and stays as-is; **this
    /// exposure check is the only thing that made a tool-level `bash` deny stick**, which is why
    /// the arm had to go rather than the precedence.
    ///
    /// The deleted arm's sole justification was an `R-NN-NNN` requirement id whose `spec/` tree is
    /// unrecoverable; `docs/adr/ADR-0008` retires such citations as authority (see its OQ-6, which
    /// independently found this branch justified by prose alone).
    fn should_expose_tool(&self, tool_name: &str, agent_name: Option<&str>) -> bool {
        let session_rules = guard(&self.session_approvals).get_rules();
        let mut mgr = guard(&self.manager);

        let raw = PermissionCheckResult {
            tool_name: tool_name.to_string(),
            state: mgr.get_tool_permission(tool_name, agent_name),
            matched_pattern: None,
            command: None,
            target: None,
            source: CheckSource::Tool,
        };
        let state = gate::apply_pattern_approval_state(raw, &json!({}), &session_rules).state;
        if state != PermissionState::Deny {
            return true;
        }
        // pi `:1811-1813` — the ONE bypass. Do not add a second (see the doc comment: PERM-009).
        if tool_name == "read" && mgr.has_allowed_skills(agent_name) {
            return true;
        }
        // pi `:1815`.
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
        // PERM-031: publish the live `has_ui` for the detached watcher BEFORE the disqualifying
        // branch, so a scan already in flight sees the new value even on the teardown path. pi gets
        // this for free — `permissionForwardingContext` holds the ctx object itself and
        // `processForwardedPermissionRequests` re-reads `ctx.hasUI` (`index.ts:1114`).
        //
        // Called from all four of pi's `startForwardedPermissionPolling` hooks
        // (`session_start`/`before_agent_start`/`input`/`tool_call`), which is every event arm that
        // carries a ctx, so this is the exact set of moments upstream reassigns `runtimeContext`.
        self.has_ui.store(ctx.has_ui, std::sync::atomic::Ordering::Relaxed);
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
            Arc::clone(&self.logger),
            Arc::clone(&self.has_ui),
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
    /// synchronously at `tokio::spawn` time, so `Arc::strong_count` MINUS the non-watcher holders is
    /// the number of watcher futures still alive. This is the assertion that would catch a
    /// non-idempotent start: the slot only ever holds ONE `JoinHandle`, so overwriting it would hide
    /// a leaked task, whereas the leaked task's `Arc` clone cannot hide.
    ///
    /// The non-watcher holders are exactly three and are structural, not incidental:
    ///
    /// 1. the extension's own `self.config` field;
    /// 2. `self.logger`, which must share the SAME handle so a config reload re-arms the audit
    ///    trail (pi's `extensionLogger` reads the module-scope `extensionConfig` binding,
    ///    `index.ts:146-150`);
    /// 3. `self.controller` (PERM-007), which must share the SAME handle so the settings modal's
    ///    writer and this extension's reader are one cell — pi's controller literal closes over the
    ///    same module-scope `extensionConfig` binding the logger reads (`getConfig: () =>
    ///    extensionConfig`, v0.8.0 `index.ts:1507`, in the `registerCommand` at `:1502-1512`).
    ///
    /// Adding a FOURTH holder without updating this constant makes the count read one watcher too
    /// many — which is precisely how this seam is meant to fail: loudly, at the assertion, rather
    /// than silently under-counting a leak. `a_fresh_extension_holds_no_watcher_config_handles`
    /// pins the baseline directly, so drift is caught even when no watcher is armed.
    #[cfg(test)]
    fn live_watcher_task_count(&self) -> usize {
        /// `self.config` + `self.logger` + `self.controller` — see the note above.
        const NON_WATCHER_CONFIG_HOLDERS: usize = 3;
        Arc::strong_count(&self.config).saturating_sub(NON_WATCHER_CONFIG_HOLDERS)
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

/// The per-`tool_call` identity the layered gate threads into every branch — the borrowed subset of
/// pi's `event` + `ctx` its `writeReviewEntry` records are built from (`toolCallId`, `toolName`,
/// `input`, `ctx.cwd`, `agentName`). Bundled rather than passed loose so the layer resolvers keep a
/// two-argument shape as the audit fields grew.
#[derive(Clone, Copy)]
struct GateCall<'a> {
    /// pi `event.toolCallId` — also the `requestId` of any prompt this call raises.
    call_id: &'a str,
    /// The trimmed tool name (pi `toolName`).
    tool_name: &'a str,
    /// The `cwd`-injected input (pi's `input` after `index.ts:2305-2309`).
    input: &'a Value,
    /// pi `ctx.cwd`.
    cwd: &'a str,
    /// The resolved persona (pi `agentName`), `None` at top level.
    agent_name: Option<&'a str>,
}

/// The on-the-wire `state` string pi writes into a `resolution` field — the SAME strings the
/// `serde(rename_all = "snake_case")` derive on [`PermissionDecisionState`] produces
/// (`ask.rs:27-42`, pi `permission-dialog.ts:1`), spelled out here so the audit trail cannot drift
/// from the derive silently.
fn decision_state_str(state: PermissionDecisionState) -> &'static str {
    match state {
        PermissionDecisionState::Approved => "approved",
        PermissionDecisionState::Denied => "denied",
        PermissionDecisionState::DeniedWithReason => "denied_with_reason",
        PermissionDecisionState::Once => "once",
        PermissionDecisionState::Always => "always",
        PermissionDecisionState::Reject => "reject",
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
pub(crate) fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
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

    /// Ambient (SEAM-071/SEAM-074): upstream `@gotgenes/pi-permission-system` is an installed
    /// package in the PATH tier that `noExtensions` collapses (`resource-loader.ts:451-453`
    /// @v0.83.0). A subagent CHILD keeps the gate — pi re-injects it by path via
    /// `resolvePermissionSystemExtension` (`pi-subagents/src/runs/shared/pi-args.ts:413-417`
    /// @v0.47.1) — so the builder's `SUBAGENT_CHILD_RUNTIME_NATIVES` carve-out, not this flag, is
    /// what keeps a pinned-allowlist child from failing OPEN.
    fn is_ambient(&self) -> bool {
        true
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
        // invalidates the agent-start cache. No LLM-visible TOOL is registered — the gate is
        // invisible to the model (pi registers none either). The one HUMAN-visible registration is
        // the `/permission-system` slash command below, which pi registers at `index.ts:1502-1512`.
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::BeforeAgentStart,
            EventKind::Input,
            EventKind::SessionStart,
            EventKind::SessionShutdown,
            EventKind::ResourcesDiscover,
        ]);
        // pi `pi.registerCommand("permission-system", { description, handler })`
        // (v0.8.0 `index.ts:1502-1512`). This is what makes `ExtensionConfig::save` reachable at
        // all: before it, every `.save(` call site in this crate lived inside `#[cfg(test)]`, so the
        // v0.8.0 save semantics (non-extension keys preserved, corrupt file refused, symlink written
        // through) could not be observed by anything a human could run. The registration lands in
        // `ExtensionRegistry`'s command table via `load_native_body`, and `/permission-system` routes
        // back here through `ExtensionHost::execute_native_command` → [`Self::execute_command`].
        api.register_command(
            PERMISSION_SYSTEM_COMMAND,
            cyrup_ext::CommandDescriptor {
                description: crate::common::PERMISSION_SYSTEM_COMMAND_DESCRIPTION.to_string(),
                // The modal's two setting ids (`config-modal.ts:27,34`) as completions.
                completions: vec!["debug".to_string(), "yoloMode".to_string()],
            },
        );
        Ok(())
    }

    /// Service the `/permission-system` command (pi `createPermissionSystemCommandHandler`,
    /// v0.8.0 `common.ts:188-198`).
    ///
    /// The `has_ui` guard is upstream's, verbatim in effect (`common.ts:192-195`): with no
    /// interactive UI the handler notifies a `warning` and returns without touching the config.
    ///
    /// It returns `Ok(None)` afterwards, NOT the sentence it just notified. Per the convention on
    /// [`cyrup_ext::NativeExtension::execute_command`], an `Ok(Some(text))` is surfaced by the
    /// session as an **Info** notification, so returning the same sentence would put it on screen
    /// twice — once as the `warning` this level deliberately chose, once as an Info duplicate. The
    /// handler owns the level here, so it owns the whole notification.
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        if name != PERMISSION_SYSTEM_COMMAND {
            return Err(ExtError::Component(format!(
                "cyrup-permission-system has no handler for command `{name}`"
            )));
        }
        // pi `common.ts:192-195`.
        if !ctx.has_ui {
            if let Some(services) = self.host_services.get() {
                services.notify(
                    crate::common::PERMISSION_SYSTEM_COMMAND_REQUIRES_UI,
                    NotifyKind::Warning,
                );
            }
            return Ok(None);
        }
        // pi `openPermissionSystemSettingsModal(ctx, { getConfig, setConfig, getConfigPath })`
        // (`index.ts:1504-1511`). `None` here means the handler already notified at its own level.
        Ok(self.run_permission_system_command(args))
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
                // PERM-024 / pi `before_agent_start` (v0.8.0 `index.ts:1875-1878`): the handler's
                // first two statements are `runtimeContext = ctx; refreshExtensionConfig(ctx);`,
                // i.e. `config.json` is re-read at the TOP OF EVERY TURN — before the watcher is
                // re-armed and before any shaping. Without it an operator's mid-session
                // `yoloMode`/`debug` edit took effect only at the next session start or resource
                // reload.
                //
                // The CONFIG half only (`refresh_extension_config`, not
                // `refresh_config_and_manager`): pi does not rebuild the `PermissionManager` here
                // and does not invalidate the agent-start cache here — doing either per turn would
                // defeat the cache PERM-013 just landed.
                self.refresh_extension_config();
                // PERM-005 / pi `before_agent_start` (`index.ts:1878`): re-enter
                // `startForwardedPermissionPolling`, so each turn re-arms the forwarding watcher
                // (and tears it down if the UI has gone away). Idempotent.
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
            HostEvent::SessionStart { reason, .. } => {
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
                // PERM-026: the yolo status pill is NO LONGER synced here. Upstream reaches it from
                // inside `refreshExtensionConfig` → `applyExtensionConfigSideEffects`
                // (v0.8.0 `index.ts:1364-1366`), which `refreshSessionRuntimeState` calls at
                // `:1821`; `refresh_config_and_manager` above now does the same, so a second write
                // here would only duplicate it — and keeping it here is exactly what let the
                // `resources_discover` arm, which never had one, go stale.
                //
                // PERM-027 / pi `:1834-1843`: a session_start whose `reason` is `"reload"` records
                // a `lifecycle.reload` line, so an operator diagnosing "did my policy edit take
                // effect" can tell a reload from a fresh start in the debug trail. Gated on the
                // reason exactly as upstream is: a `"startup"` session writes none.
                if reason == "reload" {
                    self.write_debug_entry(
                        "lifecycle.reload",
                        &json!({
                            "triggeredBy": "session_start",
                            "reason": reason,
                            "cwd": ctx.cwd.to_string_lossy(),
                        }),
                    );
                }
                HookOutcome::Noop
            }
            HostEvent::ResourcesDiscover { reason, .. } => {
                // pi `pi.on("resources_discover", …)` (v0.8.0 `index.ts:1844-1859`). The WHOLE body
                // is gated on `event.reason === "reload"` (`:1845`) — a `"startup"` discovery does
                // nothing here, because `session_start` has already refreshed everything.
                //
                // Cyrup's `HostEvent::ResourcesDiscover` used to carry no `reason`, so this arm
                // treated every dispatch as the reload case; the field now exists
                // (`cyrup-ext/src/event.rs:349`, EXT-016) and `facade::aggregate_resources`
                // genuinely sends `"startup"` for the discovery pass, so the gate is both
                // expressible and load-bearing.
                if reason != "reload" {
                    return HookOutcome::Noop;
                }
                // pi `resetShownWarnings()` (`:1846`, the reload branch's first statement).
                self.warnings.reset();
                guard(&self.dedup).clear();
                // pi `refreshExtensionConfig` + `createPermissionManagerForCwd` +
                // `invalidateAgentStartCache` (`:1848-1852`).
                self.refresh_config_and_manager(&ctx.cwd);
                // PERM-027 / pi `writeDebugEntry("lifecycle.reload", …)` (`:1853-1857`). pi's `cwd`
                // is `runtimeContext?.cwd ?? null`; cyrup's `ctx` is always live at dispatch, so
                // the null arm is unreachable rather than dropped.
                self.write_debug_entry(
                    "lifecycle.reload",
                    &json!({
                        "triggeredBy": "resources_discover",
                        "reason": reason,
                        "cwd": ctx.cwd.to_string_lossy(),
                    }),
                );
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
                // pi `invalidateAgentStartCache()` (v0.8.0 `index.ts:1871`) — the WHOLE cache, not
                // just the skill entries: a shutdown must not leave a live prompt-state key that a
                // later session could hit (PERM-013).
                self.invalidate_agent_start_cache();
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

/// True iff `dir` is a readable directory holding at least one entry (PERM-023's install signal).
///
/// An unreadable-but-present directory reports `false` here rather than the fail-safe `true`
/// [`ExtensionConfig::is_pristine_default_file`] uses for the ambiguous case, and deliberately so:
/// there, "I cannot read the file" means "I cannot rule out that it was configured"; here, an
/// `agents/` the process cannot even list is one the `PermissionManager` cannot load frontmatter
/// from either, so attaching the gate on it would advertise enforcement that will not happen.
fn dir_has_entry(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
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
/// (`ExtensionConfig::ensure_on_disk` via the load in [`PermissionSystemExtension::new`]). So a
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
/// master switch in `config.json` (`extension-config.ts:11-12,88` → `index.ts:1473-1477`). That
/// switch is now ported too (see [`permission_extension_for_env`]) and is complementary to — not a
/// substitute for — un-latching this probe: it is an explicit operator decision recorded in the
/// file, whereas this probe is about a file the crate wrote to itself.
///
/// The two compose in the only order that works: `"enabled": false` is by definition NOT the
/// pristine template, so it reads as an install signal here and the `enabled` check downstream is
/// the thing that actually declines to attach.
#[must_use]
pub fn is_installed(agent_dir: &Path, cwd: &Path) -> bool {
    if env_truthy(INSTALL_ENV_VAR) {
        return true;
    }
    let project_dir = PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg));
    // PERM-025: the GLOBAL policy file is probed at the same relocatable root
    // `manager_paths_for` enforces from, so the probe and the engine can never inspect two
    // different trees (the PERM-018 property, one rung up).
    let policy_dir = policy_agent_dir(agent_dir);
    if [policy_dir.join(POLICY_FILE), project_dir.join(POLICY_FILE)].iter().any(|p| p.exists()) {
        return true;
    }
    // PERM-023: agent-scoped `permission:` frontmatter is an ENFORCED policy layer —
    // `manager_paths_for` wires `agents_dir` / `project_agents_dir` and
    // `PermissionManager::load_agent_permissions` reads `<agents_dir>/<agent>.md` on every check
    // (pi `loadAgentPermissionsFrom` via `resolveAgentMarkdownPath`,
    // `permission-manager.ts:582-595`, `:715-745` @v0.8.0). Probing only the two `.jsonc` files
    // left an operator whose ONLY policy artifact is a persona's frontmatter with no extension
    // attached and their deny rules silently inert — a fail-open.
    //
    // "Non-empty", not "exists": neither directory is ever written by this crate (unlike
    // `config.json`, whose auto-materialization produced PERM-002's latch), but an empty
    // `agents/` left behind by another tool is not an authored policy.
    if [policy_dir.join("agents"), project_dir.join("agents")].iter().any(|p| dir_has_entry(p)) {
        return true;
    }
    // The RESOLVED path, not the raw default: `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` can point the
    // extension at a different file entirely, and both `ExtensionConfig::load` (the `enabled`
    // switch, below) and `ExtensionConfig::save` (the `/permission-system` writers) already honour
    // it. Reading the raw path here let the install decision and the on/off decision inspect two
    // different files and disagree — pi has one `getPermissionSystemConfigPath()` and every consumer
    // goes through it (`extension-config.ts:51-53`).
    let config_path = PermissionSystemExtension::resolved_config_path_for(agent_dir);
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
/// Returns `None` (attach nothing → DI-5 zero gating) when the gate is not installed, or when it
/// is installed but `config.json` sets the `enabled` master switch to `false`.
#[must_use]
pub fn permission_extension_for_env(
    agent_dir: PathBuf,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    if !is_installed(&agent_dir, &cwd) {
        return None;
    }
    // pi's `enabled` master switch (`extension-config.ts:11-12` "When false, the extension skips
    // all registrations and startup work"): `index.ts:1473-1477` loads the extension config and
    // then `if (!extensionConfig.enabled) { return; }` — a bare early return out of the extension
    // entry point `piPermissionSystemExtension(pi)` (`index.ts:1308`), before
    // `applyExtensionConfigSideEffects` (`:1479`), before the runtime-API registration (`:1481`)
    // and before every handler / command / status registration.
    //
    // This function is cyrup's analog of that entry point: returning `None` means the binary
    // wiring attaches no `NativeExtension` at all, so nothing subscribes and no startup work runs
    // — the same observable outcome as pi's early return. Only the literal `false` disables; see
    // `ExtensionConfig::normalize`.
    //
    // Deliberately AFTER `is_installed`: an operator with no config at all must not pay a config
    // load (nor have the template materialized on their disk merely by our deciding not to attach),
    // and an `"enabled": false` file is non-pristine, so it passes the install probe and lands here
    // (which is exactly where it should be declined).
    //
    // This is THE load for the whole session — pi's single `loadExtensionConfigState()` at
    // `index.ts:1473`, whose result both the `enabled` test (`:1475-1477`) and every downstream
    // consumer reuse. It is threaded into the constructor below rather than re-read there; see
    // `PermissionSystemExtension::load_config`.
    let config = PermissionSystemExtension::load_config(&agent_dir);
    if !config.enabled {
        return None;
    }
    if is_subagent_child() {
        // CHILD: forward asks up to the parent (§7.4). The parent-session anchor
        // `CYRUP_SUBAGENT_PARENT_SESSION` (emitted by `cyrup-ext-subagents`, `exec/mod.rs`
        // `PARENT_SESSION_ENV_VAR`) addresses the parent's inbox; the `ForwardingAskChannel` reads it.
        return Some(Arc::new(PermissionSystemExtension::new_forwarding_child_with_config(
            agent_dir, cwd, config,
        )));
    }
    // PARENT: in-session dialog + the forwarding watcher (installed on SessionStart).
    Some(Arc::new(PermissionSystemExtension::new_forwarding_parent_with_config(
        agent_dir, cwd, config,
    )))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn not_installed_without_policy_or_env_returns_none() {
        // No policy file, env not set → DI-5 zero gating. `INSTALL_ENV_VAR` is sandboxed (and,
        // crucially, LOCKED) by [`without_install_env`]: it is the same opt-in env var
        // `permission_extension_for_env` reads in production, and a developer/CI shell that has
        // genuinely opted in workspace-wide (exactly as this crate's own module doc documents,
        // "opt-in per DI-5") would otherwise make this "no opt-in" case flake on ambient state that
        // has nothing to do with the code path under test.
        //
        // This test used to save/clear/restore the variable inline with NO lock, on the stated
        // grounds that "no other test in this crate reads or writes `INSTALL_ENV_VAR`". That is
        // false — the PERM-002/v0.8.0 tests below all do, via `without_install_env`. A mutex only
        // serializes the parties that take it, so an unlocked mutator races every locked one in
        // both directions: it can clear the variable out from under a sibling, and a sibling's
        // restore can set it back mid-assertion here.
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            assert!(!is_installed(dir.path(), dir.path()));
        });
    }

    #[test]
    fn installed_when_policy_file_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(POLICY_FILE), "{}").unwrap();
        assert!(is_installed(dir.path(), dir.path()));
    }

    // ---------------------------------------------------------------- PERM-023: the install probe
    // must see the agent-markdown policy layer the manager ENFORCES.

    /// PERM-023 (RED before the fix). `manager_paths_for` wires `agents_dir` and
    /// `PermissionManager::load_agent_permissions` reads `<agents_dir>/<agent>.md` frontmatter as an
    /// enforced layer (pi `loadAgentPermissionsFrom`, `permission-manager.ts:715-745` @v0.8.0), but
    /// `is_installed` looked only at the env var, the two `.jsonc` files and `config.json`. An
    /// operator whose ONLY policy artifact is a persona's frontmatter therefore got no extension
    /// attached and their `permission:` deny rules were silently inert — a fail-open.
    #[test]
    fn agent_markdown_frontmatter_alone_installs_the_gate() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("work");
            std::fs::create_dir_all(&cwd).unwrap();
            // No policy file, no config.json, no env var — before the fix this is `false`.
            assert!(!is_installed(&agent_dir, &cwd), "control: nothing authored yet");

            let agents = agent_dir.join("agents");
            std::fs::create_dir_all(&agents).unwrap();
            // An EMPTY agents dir is not an authored policy.
            assert!(!is_installed(&agent_dir, &cwd), "an empty agents/ is not an install signal");

            std::fs::write(
                agents.join("coder.md"),
                "---\npermission:\n  tools:\n    bash: deny\n---\n\nYou are a coder.\n",
            )
            .unwrap();
            assert!(
                is_installed(&agent_dir, &cwd),
                "agent-scoped `permission:` frontmatter is an ENFORCED layer, so it must install"
            );
        });
    }

    /// The project-scoped half of the same signal: `<cwd>/.cyrup/agent/agents/` is wired as
    /// `project_agents_dir` and enforced identically.
    #[test]
    fn project_scoped_agent_markdown_also_installs_the_gate() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("work");
            let project_agents =
                PROJECT_AGENT_SUBDIR.iter().fold(cwd.clone(), |acc, seg| acc.join(seg)).join("agents");
            std::fs::create_dir_all(&project_agents).unwrap();
            assert!(!is_installed(&agent_dir, &cwd));
            std::fs::write(project_agents.join("reviewer.md"), "---\npermission: {}\n---\n").unwrap();
            assert!(is_installed(&agent_dir, &cwd));
        });
    }

    // ------------------------------------------------ PERM-025: the relocatable global policy root

    /// PERM-025 (RED before the fix — `POLICY_AGENT_DIR_ENV_KEY` had zero occurrences anywhere in
    /// cyrup). pi `defaultPolicyAgentDir()` (`permission-manager.ts:31-33` @v0.8.0) relocates all
    /// four global policy artifacts, and `createPermissionManagerForCwd` (`index.ts:1287-1301`)
    /// supplies only the PROJECT paths, so in a live pi session every global path comes from that
    /// override. Both the probe and the engine must consult it, or they inspect different trees.
    #[test]
    fn the_policy_agent_dir_override_moves_both_the_probe_and_the_engine() {
        without_install_env(|| {
            let _lock_note = (); // `without_install_env` already holds `env_lock`.
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let elsewhere = dir.path().join("elsewhere");
            let cwd = dir.path().join("work");
            std::fs::create_dir_all(&elsewhere).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            std::fs::write(elsewhere.join(POLICY_FILE), r#"{"tools":{"bash":"deny"}}"#).unwrap();

            // Control: the policy lives somewhere the un-overridden probe cannot see.
            assert!(!is_installed(&agent_dir, &cwd));
            assert_eq!(
                PermissionSystemExtension::manager_paths_for(&agent_dir, &cwd).global_config_path,
                agent_dir.join(POLICY_FILE)
            );

            let previous = std::env::var(POLICY_AGENT_DIR_ENV_KEY).ok();
            // SAFETY: serialized by `env_lock`, held by the enclosing `without_install_env`.
            unsafe { std::env::set_var(POLICY_AGENT_DIR_ENV_KEY, &elsewhere) };
            let installed = is_installed(&agent_dir, &cwd);
            let paths = PermissionSystemExtension::manager_paths_for(&agent_dir, &cwd);
            // SAFETY: same scope/serialization.
            unsafe {
                match previous {
                    Some(v) => std::env::set_var(POLICY_AGENT_DIR_ENV_KEY, v),
                    None => std::env::remove_var(POLICY_AGENT_DIR_ENV_KEY),
                }
            }

            assert!(installed, "the probe must follow the override, or it fails OPEN");
            assert_eq!(paths.global_config_path, elsewhere.join(POLICY_FILE));
            assert_eq!(paths.agents_dir, elsewhere.join("agents"));
            assert_eq!(paths.legacy_global_settings_path, elsewhere.join("settings.json"));
            assert_eq!(paths.global_mcp_config_path, elsewhere.join("mcp.json"));
            // The PROJECT paths are supplied explicitly upstream too and must NOT be relocated.
            let project =
                PROJECT_AGENT_SUBDIR.iter().fold(cwd.clone(), |acc, seg| acc.join(seg));
            assert_eq!(paths.project_global_config_path, Some(project.join(POLICY_FILE)));
        });
    }

    /// pi's precedence detail: `process.env[KEY]?.trim()` and then a JS truthiness test, so a value
    /// that trims to `""` is NOT an override.
    #[test]
    fn a_blank_policy_agent_dir_override_is_not_an_override() {
        let _lock = crate::ext_config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var(POLICY_AGENT_DIR_ENV_KEY).ok();
        // SAFETY: serialized by `env_lock`, restored below.
        unsafe { std::env::set_var(POLICY_AGENT_DIR_ENV_KEY, "   ") };
        let resolved = policy_agent_dir(dir.path());
        // SAFETY: same scope/serialization.
        unsafe {
            match previous {
                Some(v) => std::env::set_var(POLICY_AGENT_DIR_ENV_KEY, v),
                None => std::env::remove_var(POLICY_AGENT_DIR_ENV_KEY),
            }
        }
        assert_eq!(resolved, dir.path(), "a whitespace-only value is falsy in pi and inert here");
    }

    // ----------------------------------------------- PERM-028: `decisionScope` trims like pi's

    /// PERM-028. pi applies `getNonEmptyString` — which TRIMS (`common.ts:15-22`) — to
    /// `target`/`command`/`path` and then falls through to a RAW `toolName ?? skillName`
    /// (v0.8.0 `index.ts:581-592`). Cyrup filtered all five on a raw `!is_empty()`, so it kept the
    /// padding and, worse, SELECTED a whitespace-only command that pi skips.
    #[test]
    fn permission_decision_scope_trims_the_first_three_and_not_the_last_two() {
        let padded = DedupDetails {
            command: Some("  git status  ".to_string()),
            tool_name: Some("bash".to_string()),
            ..DedupDetails::default()
        };
        assert_eq!(
            PermissionSystemExtension::permission_decision_scope(&padded),
            json!("git status"),
            "pi's `getNonEmptyString` trims the command"
        );

        // A whitespace-only command must FALL THROUGH, not be selected.
        let blank = DedupDetails {
            command: Some("   ".to_string()),
            tool_name: Some("bash".to_string()),
            ..DedupDetails::default()
        };
        assert_eq!(PermissionSystemExtension::permission_decision_scope(&blank), json!("bash"));

        // `toolName` is NOT run through `getNonEmptyString` upstream, so its padding survives.
        let raw_tool =
            DedupDetails { tool_name: Some("  bash  ".to_string()), ..DedupDetails::default() };
        assert_eq!(
            PermissionSystemExtension::permission_decision_scope(&raw_tool),
            json!("  bash  "),
            "pi falls through to a RAW `details.toolName`; trimming it here would be a NEW divergence"
        );

        // Nothing at all ⇒ pi returns `undefined`, cyrup's `null`.
        assert_eq!(
            PermissionSystemExtension::permission_decision_scope(&DedupDetails::default()),
            Value::Null
        );
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

        let outcome = ext.on_event(
            &HostEvent::ResourcesDiscover {
                cwd: agent_dir.display().to_string(),
                reason: "reload".to_string(),
            },
            &event_ctx(agent_dir),
        ).await;
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
            ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &start_ctx).await;
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

    // ============================================================================================
    // v0.8.0 `enabled` master switch (pi `extension-config.ts:11-12,88` → `index.ts:1473-1477`).
    // ============================================================================================

    /// pi v0.8.0 added an `enabled` master switch: "When false, the extension skips all
    /// registrations and startup work" (`extension-config.ts:11-12`), enforced by a bare early
    /// return out of the extension entry point before any registration happens
    /// (`index.ts:1473-1477`). cyrup's analog is [`permission_extension_for_env`] returning `None`,
    /// so the binary attaches no `NativeExtension` at all.
    ///
    /// The switch must beat a REAL install signal, which is the whole point of a master switch —
    /// so this test arms the gate with a policy file first (the strongest signal, untouched by the
    /// PERM-002 pristine logic) and then turns it off with the config key alone.
    #[test]
    fn enabled_false_attaches_nothing_even_with_a_policy_file_present() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);

            // An unambiguous, operator-authored install signal.
            write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "deny" } }"#);
            assert!(
                permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some(),
                "precondition: a policy file installs the gate"
            );

            // The master switch off.
            write_file(&config_path, "{\n  \"enabled\": false\n}\n");
            assert!(
                is_installed(&agent_dir, &cwd),
                "`enabled` is NOT the install probe — an edited config still reads as installed; \
                 the switch has to be enforced downstream of it"
            );
            assert!(
                permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none(),
                "`\"enabled\": false` must attach no extension at all (pi `index.ts:1473-1477`)"
            );

            // MIRROR (must stay green): the switch is not over-broad. Only the literal `false`
            // disables (pi `record.enabled !== false`, `extension-config.ts:88`) — an explicit
            // `true`, a non-boolean, and a file with no `enabled` key at all (i.e. every config
            // written before v0.8.0) all keep the gate attached.
            for still_enabled in
                ["{\n  \"enabled\": true\n}\n", "{\n  \"enabled\": 0\n}\n", "{\n  \"yoloMode\": true\n}\n"]
            {
                write_file(&config_path, still_enabled);
                assert!(
                    permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some(),
                    "config {still_enabled:?} must NOT disable the gate"
                );
            }
        });
    }

    /// The upgrade hazard that comes with adding a fourth key to the auto-materialized template.
    ///
    /// [`ExtensionConfig::is_pristine_default_file`] is a BYTE-EXACT compare and it is the third
    /// install signal in [`is_installed`] (see that function's doc / PERM-002). Every cyrup build
    /// before `enabled` existed wrote a three-key `config.json`, and those files are sitting on
    /// disk. If the probe only ever accepted the CURRENT template, every one of them would stop
    /// reading as pristine the moment this key landed — silently re-arming the permission gate, on
    /// upgrade, for exactly the population PERM-002 was fixed for: people who opted in once and
    /// then opted back out.
    ///
    /// So the probe accepts a SET of exact templates, and this test pins the legacy member of it.
    #[test]
    fn a_legacy_three_key_config_template_still_reads_as_pristine() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);

            // Byte-for-byte what an older cyrup build left behind. Written as a literal, not via
            // the constant, so this test still fails if the constant itself is edited.
            write_file(
                &config_path,
                "{\n  \"debug\": false,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n",
            );
            assert!(
                ExtensionConfig::is_pristine_default_file(&config_path),
                "a config.json written by a pre-`enabled` cyrup build is still the crate's own \
                 footprint, not an operator decision"
            );
            assert!(
                !is_installed(&agent_dir, &cwd),
                "upgrading must not re-arm the gate for a user whose only leftover is the old \
                 auto-written template"
            );
            assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none());

            // MIRROR (must stay green): the CURRENT template reads as pristine too — accepting the
            // legacy bytes is additive, it does not replace the live compare.
            write_file(&config_path, &ExtensionConfig::default_config_content());
            assert!(ExtensionConfig::is_pristine_default_file(&config_path));
            assert!(!is_installed(&agent_dir, &cwd));

            // MIRROR (must stay green): the probe did NOT get looser. A file an operator actually
            // touched still reads as configured and still installs — including one that differs
            // from the legacy template by a single character, and one that is a strict subset of
            // the known keys (the semantic "does it normalize to the default" check that was
            // rejected would have wrongly accepted this second one and disabled a real gate).
            for edited in [
                "{\n  \"debug\": true,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n",
                "{\n  \"yoloMode\": false\n}\n",
            ] {
                write_file(&config_path, edited);
                assert!(
                    !ExtensionConfig::is_pristine_default_file(&config_path),
                    "hand-edited config {edited:?} must not read as pristine"
                );
                assert!(is_installed(&agent_dir, &cwd), "...and must still install the gate");
            }
        });
    }

    /// Point `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` at `path` for the duration of `body`, restoring
    /// the ambient value after.
    ///
    /// MUST be called from inside [`without_install_env`], which already holds
    /// [`crate::ext_config::env_lock`]; this helper deliberately does NOT take that lock itself,
    /// because `std::sync::Mutex` is not reentrant and re-taking it here would deadlock.
    fn with_config_path_override<T>(path: &Path, body: impl FnOnce() -> T) -> T {
        let key = crate::ext_config::CONFIG_PATH_ENV_KEY;
        let previous = std::env::var(key).ok();
        // SAFETY: serialized by `env_lock` (held by the enclosing `without_install_env`), and
        // restored below.
        unsafe { std::env::set_var(key, path) };
        let out = body();
        // SAFETY: same scope/serialization; restores whatever the ambient shell had.
        unsafe {
            match previous {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        out
    }

    /// G130(b). The install probe and the `enabled` master switch must read the SAME file.
    ///
    /// `is_installed`'s pristine-template probe read the RAW `config_path_for(agent_dir)` with no
    /// env consultation, while the `enabled` check goes through `ExtensionConfig::load` →
    /// `resolve_config_path`, which honours `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`. With the
    /// override set, the two gates inspected DIFFERENT files, so "is this installed?" and "is it
    /// switched on?" were answered about two different operator intentions. Upstream has one
    /// accessor, `getPermissionSystemConfigPath()` (`extension-config.ts:51-53`), and every
    /// consumer — `loadPermissionSystemConfig` (`:117`), `savePermissionSystemConfig` (`:240`), the
    /// modal's displayed path (`index.ts:1509`) — defaults to it.
    ///
    /// The case neither `enabled` test covered: the override points at a file whose `enabled`
    /// differs from the pristine template sitting at the default path.
    #[test]
    fn the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();

            // The DEFAULT path holds the pristine, crate-written template — the extension's own
            // footprint, therefore NOT an install signal (PERM-002), and `enabled: true`.
            let default_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
            write_file(&default_path, &ExtensionConfig::default_config_content());

            let override_path = dir.path().join("ops").join("permissions.json");
            with_config_path_override(&override_path, || {
                // The operator's own file, at the override path, with the master switch OFF — the
                // opposite of what the default path says.
                write_file(&override_path, "{\n  \"enabled\": false\n}\n");
                assert!(
                    is_installed(&agent_dir, &cwd),
                    "the install probe must read the OVERRIDE file (hand-authored ⇒ installed), \
                     not the pristine template still sitting at the default path"
                );
                assert!(
                    permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none(),
                    "...and that same file's `\"enabled\": false` is what then declines to attach"
                );

                // Same file, switch ON: the two gates agree in the other direction too.
                write_file(&override_path, "{\n  \"enabled\": true,\n  \"yoloMode\": true\n}\n");
                assert!(is_installed(&agent_dir, &cwd));
                assert!(
                    permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some(),
                    "an override file that installs AND enables must attach the gate"
                );

                // The default-path template is inert while the override is in force: nothing reads
                // it and nothing rewrote it.
                assert_eq!(
                    std::fs::read_to_string(&default_path).unwrap(),
                    ExtensionConfig::default_config_content()
                );
            });

            // MIRROR (must stay green): with NO override in force, both gates read the default
            // path exactly as before, and the pristine template there is still not an install
            // signal — resolving the path did not make the probe looser.
            assert!(!is_installed(&agent_dir, &cwd));
            assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none());
            write_file(&default_path, "{\n  \"yoloMode\": true\n}\n");
            assert!(is_installed(&agent_dir, &cwd));
            assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some());
        });
    }

    /// G130(a). Building the gate reads `config.json` ONCE.
    ///
    /// The `enabled` switch landed as its own `ExtensionConfig::load` in
    /// [`permission_extension_for_env`], and the constructor immediately loaded the SAME file
    /// again. `load` `eprintln!`s on a malformed or unreadable config, so an operator with a
    /// corrupt `config.json` saw the identical warning twice per session build where pi — which
    /// holds one `extensionConfig` populated by one `loadExtensionConfigState()` at
    /// `index.ts:1473` — prints it once.
    ///
    /// Counted rather than observed on stderr: `eprintln!` cannot be captured from inside the same
    /// process without redirecting fd 2. See `crate::ext_config::LOAD_COUNT`.
    #[test]
    fn attaching_the_gate_loads_the_extension_config_exactly_once() {
        without_install_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            // An install signal that is NOT the config file, so the probe itself performs no load.
            write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "deny" } }"#);

            crate::ext_config::reset_load_count();
            let attached = permission_extension_for_env(agent_dir.clone(), cwd.clone());
            let loads = crate::ext_config::load_count();
            assert!(attached.is_some(), "precondition: the policy file installs the gate");
            assert_eq!(
                loads, 1,
                "the session build must read config.json once, not once for the `enabled` switch \
                 and again inside the constructor"
            );

            // MIRROR (must stay green): declining to attach still reads it once — the `enabled`
            // switch has to open the file to answer at all, and the constructor never runs.
            write_file(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE), "{\n  \"enabled\": false\n}\n");
            crate::ext_config::reset_load_count();
            assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none());
            assert_eq!(crate::ext_config::load_count(), 1);

            // MIRROR (must stay green): a NOT-installed dir pays no config load at all, and so
            // never materializes the template as a side effect of deciding not to attach.
            let clean = tempfile::tempdir().unwrap();
            crate::ext_config::reset_load_count();
            assert!(permission_extension_for_env(clean.path().to_path_buf(), cwd.clone()).is_none());
            assert_eq!(crate::ext_config::load_count(), 0);
            assert!(!clean.path().join(CONFIG_DIR).join(CONFIG_FILE).exists());
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

    // ==========================================================================================
    // PERM-013 / PERM-024 / PERM-026 / PERM-027 — the lifecycle refresh + agent-start cache.
    // ==========================================================================================

    /// Records `set_active_tools` and `set_status` so the cache's call COUNTS can be asserted, and
    /// enumerates a fixed registry so `should_expose_tool` has something to filter.
    struct LifecycleRecorder {
        names: Vec<String>,
        active_tools: Mutex<Vec<Vec<String>>>,
        statuses: Mutex<Vec<Option<String>>>,
    }

    impl LifecycleRecorder {
        fn new() -> Self {
            Self {
                names: vec!["bash".to_string(), "read".to_string()],
                active_tools: Mutex::new(Vec::new()),
                statuses: Mutex::new(Vec::new()),
            }
        }
    }

    impl HostServices for LifecycleRecorder {
        fn all_tool_names(&self) -> Option<Vec<String>> {
            Some(self.names.clone())
        }
        fn set_active_tools(&self, tools: &[String]) {
            guard(&self.active_tools).push(tools.to_vec());
        }
        fn set_status(&self, _key: &str, value: Option<&str>) {
            guard(&self.statuses).push(value.map(str::to_string));
        }
    }

    fn before_agent_start(prompt: &str) -> HostEvent {
        HostEvent::BeforeAgentStart {
            prompt: "hi".to_string(),
            images: json!([]),
            system_prompt: prompt.to_string(),
            options: json!({}),
            injected: Vec::new(),
        }
    }

    /// PERM-013 (RED before the fix). pi calls `setActiveTools` ONLY when the active-tools cache key
    /// changed (v0.8.0 `index.ts:1894-1898`) and short-circuits the two sanitizers on a prompt-state
    /// key hit (`:1908-1913`). Cyrup recomputed and re-applied everything on every turn.
    #[test]
    fn repeated_before_agent_start_applies_the_active_tool_set_once() {
        with_config_env_lock(async {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().to_path_buf();
            let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
            init_ext(&ext).await;
            let host = Arc::new(LifecycleRecorder::new());
            ext.set_host_services(host.clone());
            let ctx = event_ctx(agent_dir.clone());

            for _ in 0..3 {
                let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
            }
            assert_eq!(
                guard(&host.active_tools).len(),
                1,
                "an unchanged policy + registry must apply the tool set exactly once (pi `:1895`)"
            );

            // A DIFFERENT system prompt changes the prompt-state key but not the tools key, so the
            // sanitizers re-run while `setActiveTools` still does not.
            let _ = ext.on_event(&before_agent_start("SYSTEM v2"), &ctx).await;
            assert_eq!(guard(&host.active_tools).len(), 1);

            // A session_start invalidates the whole cache (pi `invalidateAgentStartCache`,
            // `index.ts:1823`), so the next turn re-applies.
            let _ = ext
                .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
                .await;
            let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
            assert_eq!(
                guard(&host.active_tools).len(),
                2,
                "the cache must be invalidated by session_start"
            );
        });
    }

    /// PERM-013's correctness hinge: a mid-session POLICY edit must invalidate the cached prompt
    /// state even though prompt / cwd / registry are unchanged. That is why
    /// `PermissionManager::policy_cache_stamp` is public upstream (`permission-manager.ts:781`).
    #[test]
    fn a_mid_session_policy_edit_re_applies_the_shaped_tool_set() {
        with_config_env_lock(async {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().to_path_buf();
            let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
            init_ext(&ext).await;
            let host = Arc::new(LifecycleRecorder::new());
            ext.set_host_services(host.clone());
            let ctx = event_ctx(agent_dir.clone());

            let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
            assert_eq!(guard(&host.active_tools).last().map(Vec::len), Some(2));

            // Deny `bash` at the tool level; the exposed set must shrink on the NEXT turn.
            write_file(&agent_dir.join(POLICY_FILE), r#"{"tools":{"bash":"deny"}}"#);
            // The manager is rebuilt at session_start / resources_discover, matching pi — a policy
            // edit takes effect through the same reload path an operator triggers.
            let _ = ext
                .on_event(&HostEvent::SessionStart { reason: "reload".to_string(), previous_session_file: None }, &ctx)
                .await;
            let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
            assert_eq!(
                guard(&host.active_tools).last().cloned(),
                Some(vec!["read".to_string()]),
                "a tool-level bash deny must withhold `bash` (PERM-009's rule, re-applied)"
            );
        });
    }

    /// PERM-024 (RED before the fix). pi's `before_agent_start` handler's SECOND statement is
    /// `refreshExtensionConfig(ctx)` (v0.8.0 `index.ts:1877`), so a mid-session `config.json` edit
    /// takes effect at the top of the very next turn. Cyrup refreshed only at `session_start` and
    /// `resources_discover`.
    #[test]
    fn before_agent_start_re_reads_config_json() {
        with_config_env_lock(async {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().to_path_buf();
            let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
            write_file(&config_path, r#"{"yoloMode": false}"#);

            let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
            init_ext(&ext).await;
            let ctx = event_ctx(agent_dir.clone());
            let _ = ext
                .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
                .await;
            assert!(!ext.yolo_mode(), "control: the session started with yolo off");

            write_file(&config_path, r#"{"yoloMode": true}"#);
            let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
            assert!(
                ext.yolo_mode(),
                "a mid-session config edit must be live at the top of the next turn (pi `:1877`)"
            );
        });
    }

    /// PERM-026 (RED before the fix). pi syncs the status pill from inside
    /// `applyExtensionConfigSideEffects` (v0.8.0 `index.ts:1364-1366`), which EVERY refresh surface
    /// reaches — including the `resources_discover` reload branch (`:1848`). Cyrup's sync lived only
    /// in the `SessionStart` and `before_agent_start` arms, so a reload changed the live gating
    /// behaviour while the pill kept the stale value.
    #[test]
    fn a_resources_discover_reload_re_syncs_the_yolo_pill() {
        with_config_env_lock(async {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().to_path_buf();
            let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
            write_file(&config_path, r#"{"yoloMode": false}"#);

            let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
            init_ext(&ext).await;
            let host = Arc::new(LifecycleRecorder::new());
            ext.set_host_services(host.clone());
            let ctx = event_ctx(agent_dir.clone());
            let _ = ext
                .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
                .await;
            assert_eq!(
                guard(&host.statuses).last().cloned(),
                Some(None),
                "control: yolo off paints no pill"
            );

            write_file(&config_path, r#"{"yoloMode": true}"#);
            let _ = ext
                .on_event(
                    &HostEvent::ResourcesDiscover {
                        cwd: agent_dir.display().to_string(),
                        reason: "reload".to_string(),
                    },
                    &ctx,
                )
                .await;
            assert_eq!(
                guard(&host.statuses).last().cloned().flatten(),
                Some(status::YOLO_STATUS_VALUE.to_string()),
                "the reload must repaint the pill BEFORE any before_agent_start does"
            );
        });
    }

    /// PERM-027 (RED before the fix). pi writes a `lifecycle.reload` debug entry from BOTH reload
    /// surfaces (v0.8.0 `index.ts:1834-1843` and `:1853-1857`) and from NEITHER on a startup
    /// session, so an operator can tell a reload from a fresh start in the trail. Cyrup wrote none.
    #[test]
    fn reload_surfaces_write_lifecycle_reload_debug_entries() {
        with_config_env_lock(async {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().to_path_buf();
            write_file(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE), r#"{"debug": true}"#);

            let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
            init_ext(&ext).await;
            let ctx = event_ctx(agent_dir.clone());

            // A STARTUP session writes no lifecycle line (pi gates on `event.reason === "reload"`).
            let _ = ext
                .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
                .await;
            assert_eq!(lifecycle_reload_entries(&agent_dir).len(), 0);

            let _ = ext
                .on_event(&HostEvent::SessionStart { reason: "reload".to_string(), previous_session_file: None }, &ctx)
                .await;
            let _ = ext
                .on_event(
                    &HostEvent::ResourcesDiscover {
                        cwd: agent_dir.display().to_string(),
                        reason: "reload".to_string(),
                    },
                    &ctx,
                )
                .await;

            let triggers: Vec<String> = lifecycle_reload_entries(&agent_dir)
                .into_iter()
                .filter_map(|e| e["triggeredBy"].as_str().map(str::to_string))
                .collect();
            assert_eq!(
                triggers,
                vec!["session_start".to_string(), "resources_discover".to_string()],
                "both reload surfaces must name themselves in the trail"
            );
        });
    }

    /// Read every `lifecycle.reload` record out of the debug JSONL this extension writes.
    fn lifecycle_reload_entries(agent_dir: &Path) -> Vec<Value> {
        let path = crate::logging::debug_path(
            &PermissionSystemExtension::logs_dir_for(agent_dir),
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["event"] == json!("lifecycle.reload"))
            .collect()
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
        let start = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx).await;
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

        // ...and a NEW session re-arms them (pi `resetShownWarnings`, `index.ts:1819`), so a file
        // that is still broken is reported again rather than silently suppressed forever.
        //
        // PERM-021 — this asserts on the CONTENT of the delta, not on its size. The old
        // `warnings().len() > before` was satisfiable by the POLICY warning alone, so a regression
        // that stopped re-arming it while leaving the config channel alone would still have passed.
        // It also cannot be satisfied by the CONFIG warning: `WarningSink::reset` clears only
        // `shown`, while `last_config_warning` is cleared solely by a clean load or a successful
        // save — a suppression that is pi's own (`index.ts:1370-1374`'s
        // `result.warning !== lastConfigWarning` memo survives `resetShownWarnings`), so pi
        // likewise reports a still-broken `config.json` once per PROCESS. The sibling test below
        // covers the config channel through the clean-load-then-corrupt sequence that legitimately
        // clears that memo.
        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx).await;
        let _ = ext.on_event(&bash_call("call-3"), &ctx).await;
        let after = host.warnings();
        let delta = after.get(before..).unwrap_or_default();
        assert!(
            delta.iter().any(|w| w.starts_with("Failed to parse permission config at")
                && w.contains(POLICY_FILE)),
            "a new session must re-report the still-broken POLICY file; delta was {delta:?}"
        );
        assert!(
            !delta.iter().any(|w| w.starts_with("Failed to parse permission-system config at")),
            "the CONFIG warning is memoized per-process by `last_config_warning` (pi              `index.ts:1370-1374`); a re-report here would mean that memo stopped working"
        );
    }

    /// PERM-021's sibling: the CONFIG warning channel re-arms once `last_config_warning` is
    /// legitimately cleared. pi clears it on a CLEAN load (`index.ts:1373-1374`
    /// `else if (!result.warning) { lastConfigWarning = null; }`), so a session that loads a good
    /// `config.json` and then finds a corrupt one reports the corruption — the case the count-based
    /// assertion above could never distinguish from the policy warning firing twice.
    #[tokio::test]
    async fn a_config_warning_re_arms_after_a_clean_load_clears_the_memo() {
        let _guard = crate::ext_config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
        // A VALID config: the load is clean, so `last_config_warning` is `None`.
        write_file(&config_path, "{ \"debug\": false }");

        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let host = Arc::new(NotifyRecorder::new());
        ext.set_host_services(host.clone());
        let ctx = event_ctx(agent_dir.clone());
        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx).await;
        assert!(
            !host.warnings().iter().any(|w| w.starts_with("Failed to parse permission-system config at")),
            "a valid config must not warn; got {:?}",
            host.warnings()
        );

        // Now corrupt it and reload. The memo is `None`, so the warning is NEW and must surface.
        write_file(&config_path, "{ not json");
        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx).await;
        assert!(
            host.warnings().iter().any(|w| w
                .starts_with("Failed to parse permission-system config at")
                && w.ends_with("using default extension config.")),
            "a corrupt config after a clean load must reach the host; got {:?}",
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
        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx).await;
        assert_eq!(
            cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor()
                .as_deref(),
            Some("session-root-perm001"),
            "a PARENT-role SessionStart must publish the live session id as the spawn anchor"
        );

        let _ = ext
            .on_event(&HostEvent::SessionShutdown { reason: "exit".to_string(), target_session_file: None }, &ctx)
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
            .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
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
            .on_event(&HostEvent::SessionShutdown { reason: "exit".to_string(), target_session_file: None }, &ctx)
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

    /// The BASELINE `live_watcher_task_count` subtracts. It is a hand-maintained constant naming
    /// the structural `config` holders (`self.config`, `self.logger`, `self.controller`), and it
    /// has already gone stale once: PERM-007 added the `ConfigController` as a third holder, and
    /// every watcher-count assertion silently started reading one watcher too many.
    ///
    /// Pinned here, on an extension with NO watcher armed, so a future holder trips this test —
    /// which names the cause — instead of only the PERM-005 tests, which would blame a watcher leak
    /// that never happened.
    #[test]
    fn a_fresh_extension_holds_no_watcher_config_handles() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let ext =
            PermissionSystemExtension::new_forwarding_parent(agent_dir, dir.path().to_path_buf());
        assert_eq!(
            ext.live_watcher_task_count(),
            0,
            "no hook has run, so no watcher exists; a non-zero count means a new structural holder \
             of the shared `config` handle was added without updating NON_WATCHER_CONFIG_HOLDERS"
        );
    }

    /// PERM-005, the crux: the three per-turn hooks fire on EVERY turn, so a non-idempotent start
    /// would spawn one watcher per turn. N calls must yield exactly ONE live watcher task.
    #[tokio::test]
    async fn repeated_hooks_yield_exactly_one_forwarding_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-idem").await;
        let ctx = ui_ctx(dir.path());

        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None }, &ctx).await;
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
                &HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None },
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
            .on_event(&HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None }, &ui_ctx(dir.path()))
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

        let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None }, &ctx).await;
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
