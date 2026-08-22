//! Construction. The three role-specific constructors — bare / forwarding PARENT / forwarding
//! CHILD — their `_with_config` cores, the explicit-parts test seam, and the `Arc` promotion that
//! arms the published runtime API.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_core::ExtensionId;
use cyrup_ext::HostServices;

use crate::agent_start_cache::AgentStartCache;
use crate::ask::{AskChannel, ForwardingAskChannel, NoOpAskChannel};
use crate::dedup::DedupCache;
use crate::ext_config::ExtensionConfig;
use crate::forwarding;
use crate::manager::ManagerPaths;
use crate::stores::SessionApprovalStore;

use super::PermissionSystemExtension;
use super::consts::EXTENSION_ID;
use super::env::resolve_agent_name_from_env;
use super::warnings::{WarningSink, manager_with_warnings};

// Doc-link-only imports: these are named by prose relocated verbatim from the single-file
// `extension.rs`, where they were in scope for real code. `#[cfg(doc)]` keeps those intra-doc
// links resolving without adding an import the compiled build does not use.
#[cfg(doc)]
use super::install::permission_extension_for_env;

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
    pub(super) fn new_forwarding_parent_with_config(
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
    pub(super) fn new_forwarding_child_with_config(
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
            self_ref: OnceLock::new(),
            runtime_api: Mutex::new(None),
        }
    }

    /// PERM-011 half A — wrap in an `Arc` and record a `Weak` back-reference, so `init` can publish
    /// the runtime API on [`crate::runtime_api`].
    ///
    /// This is the ONE constructor step that cannot happen inside [`Self::from_parts_full`]: the
    /// `Weak` can only be taken once the value is inside its `Arc`. Every production path
    /// ([`permission_extension_for_env`]) goes through here; a by-value extension (unit tests)
    /// simply publishes nothing, which is the honest state — pi publishes at extension activation,
    /// and a test that never activates one has no realm-global either.
    #[must_use]
    pub fn into_shared(self) -> Arc<Self> {
        let arc = Arc::new(self);
        let _ = arc.self_ref.set(Arc::downgrade(&arc));
        arc
    }

    /// Override the resolved persona name (deterministic tests / an embedder that resolves the name
    /// itself). Production leaves the env-sourced value from [`resolve_agent_name_from_env`] in place.
    /// Trims; empty → `None` (pi `normalizeAgentName`, `index.ts:277-284`).
    #[must_use]
    pub fn with_agent_name(mut self, agent_name: Option<String>) -> Self {
        self.agent_name = agent_name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self
    }
}
