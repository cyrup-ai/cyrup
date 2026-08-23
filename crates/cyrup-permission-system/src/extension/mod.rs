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
//! with a [`ForwardingAskChannel`] ([`PermissionSystemExtension::new_forwarding_child`]) — an ask-tier decision writes a
//! nonce-bound request into the PARENT's spool (addressed by the `CYRUP_SUBAGENT_PARENT_SESSION`
//! anchor `cyrup-ext-subagents` emits, `exec/mod.rs` `PARENT_SESSION_ENV_VAR`) and BLOCKS on the bound
//! response under the P-3 `begin_human_wait` guard. The PARENT ([`PermissionSystemExtension::new_forwarding_parent`])
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
//! FOUR SUPPLEMENTARY LAYERS (all wired + enforcing, pi `index.ts:2208-2499`): [`PermissionSystemExtension::decide`] runs
//! the agent + projectAgent policy layers (keyed by [`resolve_agent_name_from_env`], the
//! `CYRUP_SUBAGENT_AGENT_NAME` spawn anchor), the registry / unknown-tool block (against
//! [`cyrup_ext::HostServices::all_tool_names`]), the skill-read bypass (against the `before_agent_start`
//! `<available_skills>` entries, [`crate::skill`]), and the external-directory guard (against
//! [`HostCtx::cwd`]). The `before_agent_start` CONTEXT-HYGIENE layer is ALSO wired (pi
//! `index.ts:2134-2190`, port doc §9): [`PermissionSystemExtension::on_before_agent_start`] shapes the active tool set
//! ([`PermissionSystemExtension::should_expose_tool`] → [`cyrup_ext::HostServices::set_active_tools`]), RETURNS the
//! sanitized system prompt as a `[mutate]` ([`crate::sanitize`]), and surfaces the yolo status pill
//! ([`crate::status`]). Every layer — deciding gate AND context-hygiene — is live here, none stubbed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cyrup_core::ExtensionId;
use cyrup_ext::HostServices;

use crate::agent_start_cache::AgentStartCache;
use crate::ask::AskChannel;
use crate::dedup::DedupCache;
use crate::forwarding;
use crate::manager::PermissionManager;
use crate::skill::SkillPromptEntry;
use crate::stores::SessionApprovalStore;

// Doc-link-only imports: these are named by prose relocated verbatim from the single-file
// `extension.rs`, where they were in scope for real code. `#[cfg(doc)]` keeps those intra-doc
// links resolving without adding an import the compiled build does not use.
#[cfg(doc)]
use crate::ask::{ForwardingAskChannel, LocalAskChannel};
#[cfg(doc)]
use crate::extension::env::resolve_agent_name_from_env;
#[cfg(doc)]
use cyrup_ext::{HostCtx, NativeExtension};

mod agent_start;
mod audit;
mod command;
mod config;
mod construct;
mod consts;
mod decide;
mod env;
mod events;
mod install;
mod native;
mod paths;
mod prompt;
mod warnings;
mod watcher;
mod yolo;

#[cfg(test)]
mod tests;

use warnings::WarningSink;

// Re-exported so every `crate::extension::…` / `cyrup_permission_system::extension::…`
// path that resolved while this was one file keeps resolving: `lib.rs`'s
// `pub use extension::{…}`, `logging.rs` + `status.rs`'s `EXTENSION_ID`, and
// `tests/shipped_artifacts.rs`'s two artifacts.
pub use consts::{
    COMMAND_YOLO_CONTROL_SOURCE, EXTENSION_ID, PERMISSION_REQUEST_EVENT_CHANNEL,
    PERMISSION_SYSTEM_COMMAND, PERMISSIONS_EXAMPLE_CONFIG, PERMISSIONS_JSON_SCHEMA,
};
pub use env::{CHILD_ENV_VAR, INSTALL_ENV_VAR, POLICY_AGENT_DIR_ENV_KEY, SUBAGENT_ENV_HINT_KEYS};
pub use install::{is_installed, permission_extension_for_env};

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
    /// so the audit trail is unconditional — see [`crate::logging::PermissionSystemLogger::review`].
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
    /// The fail-closed FALLBACK ask channel ([`crate::NoOpAskChannel`] in production; a scripted channel in
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
    /// PERM-011 half A — a `Weak` handle on this extension's own `Arc`, installed by
    /// [`Self::into_shared`]. It exists so the published runtime API
    /// ([`crate::runtime_api()`]) can call back into these methods without the process-global
    /// registry OWNING the extension: pi's registered object is a bag of closures over module
    /// scope, whose lifetime is the realm's, and a `Weak` is the Rust spelling that does not
    /// outlive the session. Unset when the extension was built by value (unit tests), in which
    /// case nothing is published — see [`Self::publish_runtime_api`].
    self_ref: OnceLock<std::sync::Weak<PermissionSystemExtension>>,
    /// PERM-011 half A — pi's module-scope `let runtimeApi: PiPermissionSystemRuntimeApi | null`
    /// (`index.ts:159`), which holds what `registerPiPermissionSystemRuntimeApi` returned
    /// (`:1481`) purely so `session_shutdown` can hand it back to the identity-guarded
    /// unregister (`:1868-1870`).
    runtime_api: Mutex<Option<Arc<dyn crate::runtime_api::PermissionSystemRuntimeApi>>>,
}

/// Lock a `Mutex`, recovering from poison rather than panicking (no-panic policy). Held only across
/// synchronous sections — never across an `.await`.
pub(crate) fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
