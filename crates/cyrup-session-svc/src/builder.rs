//! `SessionConfig` + `SessionBuilder` — assemble an [`AgentSession`] from the real subsystems
//! (arch-11 §3.3). One async `build()` resolves settings + trust + auth + model (cyrup-config),
//! discovers resources (cyrup-resources), builds the tool registry with isolation decorators +
//! permission policy (cyrup-tools), opens/creates the session tree and wires compaction
//! (cyrup-session arch-04/05), assembles the system prompt + context store (arch-06), builds the
//! extension host with native built-ins and attaches BOTH ext seams to the agent (cyrup-ext), and
//! resolves the provider into the agent loop (cyrup-provider).

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_agent::{Agent, ProviderStreamFn};
use cyrup_core::{CancelToken, ModelRef, RunCancel, ModelThinkingLevel};
use cyrup_config::{
    decide_trust, has_trust_requiring_resources, AppMode, AuthStore, InMemorySettingsStore,
    ModelResolver, Settings, SettingsManager, SettingsStore, TrustInputs, TrustOutcome,
};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig, NativeExtension};
use cyrup_provider::{Model, Provider};
use cyrup_resources::{discover, DiscoveryConfig, ResourceOverrides, SkillPointer};
use cyrup_session::manager::{NewSessionOpts, SessionManager};
use cyrup_session::prompt::{
    ContextFileLoader, DocsPointers, PromptInputs, ResolvedOverride, SystemPromptBuilder,
    ToolPromptContribution,
};
use cyrup_session::SessionLayout;
use cyrup_tools::{
    Availability, Backend, PermissionPolicy, ProtectedFs, ShellConfig, ToolRegistry, ToolsOptions,
    TraversalFs,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::SessionServiceError;
use crate::event::core_message_to_agent;
use crate::services::AgentSessionServices;
use crate::session::AgentSession;
use crate::subscriber::{Fanout, SvcSubscriber};

/// Which session to start (arch-11 §3.3, `SessionStartEvent` analogue).
#[derive(Clone, Debug, Default)]
pub enum SessionTarget {
    /// A fresh session for the cwd.
    #[default]
    New,
    /// Resume an existing session file by path.
    Resume(PathBuf),
    /// Continue the most recent session for the cwd (or create one if none).
    Continue,
    /// Fork a source session file into a fresh session anchored at the build cwd, optionally with an
    /// explicit id (Pi `SessionManager.forkFrom(sourcePath, cwd, sessionDir, { id })`, main.ts:251).
    /// Used by `--fork <ref>` and by `--session <ref>` when the ref resolves to another project and
    /// the user confirms forking it into the current directory.
    Fork {
        /// The resolved source session file to copy history from.
        source: PathBuf,
        /// The forked session's id (`--session-id`), or `None` to mint a fresh one.
        id: Option<String>,
    },
    /// Create a fresh session with an explicit id (Pi `SessionManager.create(cwd, dir, { id })`,
    /// main.ts:349). Used by `--session-id <id>` when no local session with that exact id exists.
    CreateWithId(String),
}

/// The declarative inputs the builder resolves into a wired session (arch-11 §3.3).
#[derive(Clone)]
pub struct SessionConfig {
    pub cwd: PathBuf,
    /// Manager-cwd override for a `Resume` target (Pi `SessionManager.open(path, _, cwdOverride)`,
    /// runtime.ts:207): when `Some`, the resumed [`SessionManager`]'s own cwd is rebound to this path
    /// instead of being derived from the session file's header. `None` ⇒ derive from the header (the
    /// default). Set by the runtime's `switch_session`/`import` flows; left `None` by one-shot builds.
    pub cwd_override: Option<PathBuf>,
    /// Global agent dir (`~/.cyrup/agent`): global settings, auth, context files, resource roots.
    pub agent_dir: PathBuf,
    /// Home dir for trust-requiring-resource detection (defaults to `agent_dir`).
    pub home: PathBuf,
    /// Sessions root directory (defaults to `agent_dir/sessions`).
    pub session_dir: Option<PathBuf>,
    /// Runtime mode (drives non-prompting trust + the extension `ctx.mode`/`ctx.hasUI`).
    pub app_mode: AppMode,
    /// Model selection pattern (`provider/id[:level]`); `None` ⇒ settings default ⇒ first catalog.
    pub model_pattern: Option<String>,
    /// Whether the CLI named a provider explicitly (`--provider`, Pi `cliProvider`). Lets the model
    /// resolver build a Pi `buildFallbackModel` custom-id model for an unresolvable `--model` id on a
    /// *known* provider even when the pattern carries no `provider/` prefix (Pi model-resolver.ts:475).
    pub cli_provider_explicit: bool,
    /// Thinking level override (`None` ⇒ pattern `:level` ⇒ settings default).
    pub thinking_level: Option<ModelThinkingLevel>,
    /// `--approve` (Some(true)) / `--no-approve` (Some(false)).
    pub trust_override: Option<bool>,
    /// `--no-context-files` / `-nc`.
    pub no_context_files: bool,
    /// `--no-skills`.
    pub no_skills: bool,
    /// `--no-prompt-templates` / `-np`: disable prompt-template discovery (Pi `noPromptTemplates`).
    pub no_prompt_templates: bool,
    /// `--no-themes`: disable theme discovery (Pi `noThemes`).
    pub no_themes: bool,
    /// `--no-extensions` / `-ne`: disable extension *discovery* (the project + global roots). Explicit
    /// `--extension` paths still load (Pi `resourceLoaderOptions.noExtensions`, main.ts:664).
    pub no_extensions: bool,
    /// Explicit `--extension <path>` resources to load as pre-trust *configured* extensions (Pi
    /// `resourceLoaderOptions.additionalExtensionPaths`, main.ts:660). Each may be a single extension
    /// dir or a directory of extensions. Threaded into [`extension_discovery_roots`] regardless of
    /// `no_extensions`.
    pub extra_extension_paths: Vec<PathBuf>,
    /// Explicit `--skill <path>` resources to append to discovery (Pi `additionalSkillPaths`,
    /// resource-loader.ts:421). Merged into the discovered registry before skill-pointer derivation.
    pub extra_skill_paths: Vec<PathBuf>,
    /// Explicit `--prompt-template <path>` resources to append to discovery.
    pub extra_prompt_paths: Vec<PathBuf>,
    /// Explicit `--theme <path>` resources to append to discovery.
    pub extra_theme_paths: Vec<PathBuf>,
    /// Full system-prompt replacement.
    pub system_prompt: Option<String>,
    /// Append text after the assembled prompt.
    pub append_system_prompt: Option<String>,
    /// Persist to disk (`false` ⇒ ephemeral in-memory session; print/json default, R-11-008).
    pub persist: bool,
    /// Parent session file recorded on a freshly-created session (Pi `newSession({parentSession})`,
    /// session-manager.ts; runtime.ts:238). `None` for a top-level session. Threaded into
    /// [`NewSessionOpts::parent_session`] only on the `New` target (resumed sessions keep their own).
    pub parent_session: Option<String>,
    pub target: SessionTarget,
    /// Model-visible tool-set control.
    pub tool_availability: Availability,
    /// Default tool-suppression mode when no explicit `tools` allowlist is given (Pi `noTools`).
    pub no_tools: Option<NoTools>,
    /// Explicit allowlist of tool names; when `Some`, only these tools are active (Pi `tools`).
    pub tools: Option<Vec<String>>,
    /// Denylist of tool names removed after the allowlist/noTools selection (Pi `excludeTools`).
    pub exclude_tools: Vec<String>,
    /// Custom tools registered in addition to the built-ins (Pi `customTools`, sdk.ts:71,384). Added
    /// to the dynamic-tool registry as enable-able tools (not auto-activated).
    pub custom_tools: Vec<Arc<dyn cyrup_core::Tool>>,
    /// Opt-in permission policy gate (empty ⇒ YOLO default, R-12-001).
    pub permission_policy: PermissionPolicy,
    /// Wrap the fs backend in [`ProtectedFs`] (blocks writes to `.env`/`.git`/… R-12-006).
    pub protect_paths: bool,
    /// Wrap the fs backend in [`TraversalFs`] confined to `cwd` (R-03-006).
    pub confine_to_cwd: bool,
    /// Captured extension CLI flag values threaded from the bin (Pi `extensionFlagValues:
    /// parsed.unknownFlags`, main.ts:634 / args.ts:188-201). Forwarded onto [`AgentSessionServices`]
    /// so a loaded extension can read them via `applyExtensionFlagValues`. The WASM-guest *consumption*
    /// rides the ext-host tier (ledgered); the bin-side capture + threading is closed here. Each entry
    /// is `(name, value)` with the leading `--` already stripped.
    pub extension_flag_values: Vec<(String, ExtensionFlagValue)>,
}

/// A captured extension CLI flag value (Pi `unknownFlags` map entry, args.ts:52-53). `Bool(true)` is a
/// bare `--flag`; `Str` is `--flag=value` or `--flag value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionFlagValue {
    Bool(bool),
    Str(String),
}

impl SessionConfig {
    /// A config rooted at `cwd` with `agent_dir`, all defaults sensible for an SDK embedder.
    pub fn new(cwd: impl Into<PathBuf>, agent_dir: impl Into<PathBuf>) -> Self {
        let agent_dir = agent_dir.into();
        Self {
            cwd: cwd.into(),
            cwd_override: None,
            home: agent_dir.clone(),
            agent_dir,
            session_dir: None,
            app_mode: AppMode::Print,
            model_pattern: None,
            cli_provider_explicit: false,
            thinking_level: None,
            trust_override: None,
            no_context_files: false,
            no_skills: false,
            no_prompt_templates: false,
            no_themes: false,
            no_extensions: false,
            extra_extension_paths: Vec::new(),
            extra_skill_paths: Vec::new(),
            extra_prompt_paths: Vec::new(),
            extra_theme_paths: Vec::new(),
            system_prompt: None,
            append_system_prompt: None,
            persist: true,
            parent_session: None,
            target: SessionTarget::New,
            tool_availability: Availability::All,
            no_tools: None,
            tools: None,
            exclude_tools: Vec::new(),
            custom_tools: Vec::new(),
            permission_policy: PermissionPolicy::new(),
            protect_paths: true,
            confine_to_cwd: false,
            extension_flag_values: Vec::new(),
        }
    }
}

/// Default tool-suppression mode (Pi `noTools: "all" | "builtin"`, sdk.ts:52-59).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoTools {
    /// Start with no tools enabled.
    All,
    /// Disable the default built-in tools but keep extension/custom tools (Pi default built-ins).
    Builtin,
}

/// Build the provider-scoped env overlay for the configured HTTP proxy (Pi `applyHttpProxySettings`,
/// http-dispatcher.ts:42-47): a non-empty `httpProxy` setting sets both `HTTP_PROXY` and `HTTPS_PROXY`
/// in the overlay (matching Pi's `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`),
/// so the provider's `resolveHttpProxyUrlForTarget` routes requests through it. An absent/blank setting
/// yields `None` (the ambient process env is used unchanged).
fn http_proxy_overlay(proxy: Option<&str>) -> Option<cyrup_provider::ProviderEnv> {
    let proxy = proxy?.trim();
    if proxy.is_empty() {
        return None;
    }
    let mut overlay = cyrup_provider::ProviderEnv::new();
    overlay.insert("HTTP_PROXY".to_string(), proxy.to_string());
    overlay.insert("HTTPS_PROXY".to_string(), proxy.to_string());
    Some(overlay)
}

/// Pi's default active built-in tool names (sdk.ts:244).
const DEFAULT_BUILTIN_TOOLS: [&str; 4] = ["read", "bash", "edit", "write"];

/// Apply the `tools`/`noTools`/`excludeTools` selection over the Availability-visible tool set
/// (Pi sdk.ts:244-251). When none of the three is set the visible set passes through unchanged.
fn select_active_tools(
    visible: &[Arc<dyn cyrup_core::Tool>],
    cfg: &SessionConfig,
) -> Vec<Arc<dyn cyrup_core::Tool>> {
    let exclude: std::collections::HashSet<&str> =
        cfg.exclude_tools.iter().map(String::as_str).collect();
    let keep = |name: &str| -> bool {
        match (&cfg.tools, cfg.no_tools) {
            // Explicit allowlist wins (Pi `options.tools`).
            (Some(allow), _) => allow.iter().any(|a| a == name),
            (None, Some(NoTools::All)) => false,
            (None, Some(NoTools::Builtin)) => !DEFAULT_BUILTIN_TOOLS.contains(&name),
            (None, None) => true,
        }
    };
    visible
        .iter()
        .filter(|t| keep(t.name()) && !exclude.contains(t.name()))
        .cloned()
        .collect()
}

/// Assembles an [`AgentSession`] from a [`SessionConfig`] + injected provider/services (arch-11).
pub struct SessionBuilder {
    provider: Arc<dyn Provider>,
    config: SessionConfig,
    settings_store: Arc<dyn SettingsStore>,
    auth: Option<Arc<AuthStore>>,
    native_extensions: Vec<Arc<dyn NativeExtension>>,
    cli_settings: Settings,
    /// A pre-built session manager to adopt instead of opening/creating one from `config.target`
    /// (Pi `createAgentSessionFromServices` with a caller-supplied `sessionManager`,
    /// agent-session-services.ts:187). Used by the runtime fork path, where the branched manager is
    /// mutated in place and handed over directly (its file write may still be deferred on disk).
    prebuilt_manager: Option<SessionManager>,
}

impl SessionBuilder {
    /// Start a builder over the resolved `provider` and a `config`.
    pub fn new(provider: Arc<dyn Provider>, config: SessionConfig) -> Self {
        Self {
            provider,
            config,
            settings_store: Arc::new(InMemorySettingsStore::new()),
            auth: None,
            native_extensions: Vec::new(),
            cli_settings: Settings::new(),
            prebuilt_manager: None,
        }
    }

    /// Adopt a caller-supplied, already-constructed [`SessionManager`] (the runtime fork path),
    /// overriding `config.target`. The builder uses the manager's cwd verbatim.
    #[must_use]
    pub(crate) fn with_manager(mut self, manager: SessionManager) -> Self {
        self.prebuilt_manager = Some(manager);
        self
    }

    /// Override the settings store (default: in-memory).
    #[must_use]
    pub fn settings_store(mut self, store: Arc<dyn SettingsStore>) -> Self {
        self.settings_store = store;
        self
    }

    /// Override the credential store (default: `auth.json` under the agent dir).
    #[must_use]
    pub fn auth(mut self, auth: Arc<AuthStore>) -> Self {
        self.auth = Some(auth);
        self
    }

    /// CLI-scoped settings overrides (highest precedence).
    #[must_use]
    pub fn cli_settings(mut self, settings: Settings) -> Self {
        self.cli_settings = settings;
        self
    }

    /// Register a native built-in extension (loaded in order; both seams wired to the agent).
    #[must_use]
    pub fn with_native_extension(mut self, ext: Arc<dyn NativeExtension>) -> Self {
        self.native_extensions.push(ext);
        self
    }

    /// Assemble the wired [`AgentSession`] (arch-11 §3.3). Async: discovery + context load + native
    /// extension `init` run here.
    pub async fn build(self) -> Result<AgentSession, SessionServiceError> {
        let cfg = self.config;
        let cwd = cfg.cwd.clone();
        let cancel = RunCancel::new();

        // ---- 1. settings + trust (cyrup-config) ------------------------------------------------
        // Load global first (project untrusted) to read defaultProjectTrust, then decide trust.
        let mut settings = SettingsManager::load(
            self.settings_store.clone(),
            self.cli_settings.clone(),
            false,
        );
        let default_trust = settings.effective().default_project_trust();
        let has_resources = has_trust_requiring_resources(&cwd, &cfg.home);
        let outcome = decide_trust(TrustInputs {
            has_resources,
            trust_override: cfg.trust_override,
            saved: None,
            default_trust,
            mode: cfg.app_mode,
            prompt_choice: None,
        });
        let trusted = matches!(outcome, TrustOutcome::Trusted);
        settings.set_project_trusted(trusted);

        // ---- 2. auth (cyrup-config) ------------------------------------------------------------
        let auth = self
            .auth
            .unwrap_or_else(|| Arc::new(AuthStore::at(cfg.agent_dir.join("auth.json"))));

        // ---- 2b. session tree (cyrup-session arch-04) — created BEFORE model resolution so the
        // model/thinking restore can read the resumed branch (Pi sdk.ts:178,187: the SessionManager
        // is constructed, then `buildSessionContext()` feeds `existingSession.model`/`thinkingLevel`).
        let sessions_root =
            cfg.session_dir.clone().unwrap_or_else(|| cfg.agent_dir.join("sessions"));
        let layout = SessionLayout::new(sessions_root, cwd.clone());
        let mut manager = match self.prebuilt_manager {
            Some(m) => m,
            None => match &cfg.target {
                SessionTarget::New => {
                    // Record `parentSession` on a freshly-created session (Pi `newSession`,
                    // runtime.ts:238): the `New` target alone honors it — a resumed/continued
                    // session keeps the parent it was created with.
                    let opts = NewSessionOpts {
                        parent_session: cfg.parent_session.clone(),
                        ..NewSessionOpts::default()
                    };
                    if cfg.persist {
                        SessionManager::create(&cwd, &layout, opts)?
                    } else {
                        SessionManager::in_memory(&cwd, opts)?
                    }
                }
                // Rebind the resumed manager's cwd to the override when the runtime supplied one
                // (Pi `SessionManager.open(path, _, cwdOverride)`, runtime.ts:207); else derive from
                // the file header.
                SessionTarget::Resume(path) => {
                    SessionManager::open_with_cwd(path, cfg.cwd_override.as_deref())?
                }
                SessionTarget::Continue => SessionManager::continue_recent(&cwd, &layout)?,
                // Fork the resolved source file into a fresh session at the build cwd (Pi
                // `forkSessionOrExit`/`SessionManager.forkFrom`, main.ts:251-258). The `--session-id`
                // (when given) becomes the forked session's id; otherwise one is minted.
                SessionTarget::Fork { source, id } => {
                    let opts = NewSessionOpts {
                        id: id.clone().map(cyrup_core::SessionId::from),
                        ..NewSessionOpts::default()
                    };
                    SessionManager::fork_from(source, &cwd, &layout, opts)?
                }
                // Create a fresh session with an explicit id (Pi `SessionManager.create(cwd, dir,
                // { id })`, main.ts:349). Persists like `New`; an ephemeral run goes in-memory.
                SessionTarget::CreateWithId(id) => {
                    let opts = NewSessionOpts {
                        id: Some(cyrup_core::SessionId::from(id.clone())),
                        parent_session: cfg.parent_session.clone(),
                    };
                    if cfg.persist {
                        SessionManager::create(&cwd, &layout, opts)?
                    } else {
                        SessionManager::in_memory(&cwd, opts)?
                    }
                }
            },
        };
        let session_id = manager.session_id().clone();
        let existing = manager.build_context();
        let has_existing_session = !existing.messages.is_empty();
        // Pi `hasThinkingEntry` (sdk.ts:189): does the resumed branch carry a thinking_level_change?
        let has_thinking_entry = manager
            .branch_path(None)
            .iter()
            .any(|e| matches!(e, cyrup_session::Entry::Known(
                cyrup_session::entry::KnownEntry::ThinkingLevelChange { .. })));

        // ---- 3. model resolution (cyrup-config + cyrup-provider) -------------------------------
        // Restore the model + thinking level from the resumed session, seeding a fallback message
        // when the saved model is no longer resolvable (Pi sdk.ts:191-242).
        let (resolved_model, model_ref, thinking, model_fallback_message) =
            resolve_model(&*self.provider, &cfg, &settings, &existing, has_existing_session, has_thinking_entry)?;

        // ---- 4. tools + isolation + policy (cyrup-tools) --------------------------------------
        let shell = ShellConfig::detect();
        let base = Backend::local(shell.clone());
        // The process backend the immediate-bash seam (#8) runs against (kept past `base`'s move).
        let bash_proc = base.proc.clone();
        let mut fs = base.fs.clone();
        if cfg.confine_to_cwd {
            fs = Arc::new(TraversalFs::new(fs, cwd.clone()));
        }
        if cfg.protect_paths {
            fs = Arc::new(ProtectedFs::with_defaults(fs));
        }
        let backend = Backend { fs, proc: base.proc.clone() };
        let registry = ToolRegistry::with_builtins(cwd.clone(), backend, ToolsOptions::default());
        let visible = registry.visible(&cfg.tool_availability);
        // Tool-set selection (Pi sdk.ts:244-251): an explicit `tools` allowlist, or `noTools`
        // ("all" ⇒ none; "builtin" ⇒ drop the default built-ins), then minus the `excludeTools`
        // denylist. Absent all three, the Availability-visible set is kept verbatim.
        let base_tools = select_active_tools(&visible, &cfg);
        let read_available = base_tools.iter().any(|t| t.name() == "read");

        // ---- 4b. extension host (cyrup-ext) — built BEFORE resource discovery so the
        // `resources_discover` aggregate (extendResourcesFromExtensions, Pi agent-session.ts:2112)
        // can merge extension-contributed skill/prompt/theme paths into the registry the skill
        // pointers + system prompt are then derived from.
        let (mode, has_ui) = ext_mode(cfg.app_mode);
        let host_config = HostConfig { mode, has_ui, cwd: cwd.clone() };
        // With `wasm-host`, spin up the Wasmtime engine so live wasm extensions can be loaded with
        // `LiveHostServices` injected (the seam below); otherwise a native-only host (the default).
        #[cfg(feature = "wasm-host")]
        let host = ExtensionHost::with_wasm(host_config)?;
        #[cfg(not(feature = "wasm-host"))]
        let host = ExtensionHost::new(host_config);
        for ext in self.native_extensions {
            host.load_native(ext).await?;
        }
        let ext_host = Arc::new(host);

        // Resolve the on-disk extension discovery roots from `--extension`/`--no-extensions` (Pi
        // `resourceLoaderOptions.additionalExtensionPaths`/`noExtensions`, main.ts:660,664). The roots
        // are computed here (so `-ne` disables project+global discovery and `-e` paths are configured,
        // pre-trust); the live wasm *instantiation* of each discovered extension runs only under the
        // `wasm-host` feature (the Wasmtime engine + the `wasm32-wasip2` guest toolchain — the gated
        // arch-08b live-wasm tail, residual ledger §09 #13). Native built-ins are already loaded above.
        let ext_roots = extension_discovery_roots(&cfg);
        #[cfg(feature = "wasm-host")]
        {
            let host_services_for_load: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(
                crate::host_services::LiveHostServices::new(self.provider.clone()),
            );
            // The per-path `errors` (Pi `LoadExtensionsResult.errors` → "Failed to load extension"
            // diagnostics, main.ts:679-682) surface to the caller once the diagnostics channel reaches
            // the bin; today they are recorded on the result (the wasm-host E2E is tooling-gated).
            let _load_result =
                ext_host.discover_and_load(&ext_roots, trusted, host_services_for_load).await;
        }
        #[cfg(not(feature = "wasm-host"))]
        let _ = &ext_roots;

        // ---- 5. resources discovery (cyrup-resources) -----------------------------------------
        let mut disc = DiscoveryConfig::new(cwd.clone(), cfg.agent_dir.clone());
        disc.trusted_project = trusted;
        disc.enable_skills = !cfg.no_skills;
        disc.enable_prompts = !cfg.no_prompt_templates;
        disc.enable_themes = !cfg.no_themes;
        // Settings-tier resource overrides (cross-layer wiring; Pi `package-manager.ts:2265-2278`):
        // the `skills`/`prompts`/`themes` settings lists are enable/disable patterns over the
        // auto-discovered loose resources. The layered `SettingsManager` exposes the per-layer split
        // (Pi `globalSettings`/`projectSettings`, settings-manager.ts:455-470), so global-scope
        // discovery is gated by the GLOBAL layer's lists and project-scope by the PROJECT layer's —
        // not the merged effective view (which would let a project list silently widen the global
        // scope, or vice-versa). Empty lists — the default — preserve "discover everything".
        disc.global_overrides = ResourceOverrides {
            skills: settings.global().skill_paths(),
            prompts: settings.global().prompt_template_paths(),
            themes: settings.global().theme_paths(),
        };
        disc.project_overrides = ResourceOverrides {
            skills: settings.project().skill_paths(),
            prompts: settings.project().prompt_template_paths(),
            themes: settings.project().theme_paths(),
        };
        let report = discover(&disc, cancel.token()).await?;
        // extendResourcesFromExtensions("startup") (Pi agent-session.ts:2109-2135): fold every
        // `resources_discover` handler's contributed skill/prompt/theme paths into the registry
        // BEFORE the skill pointers + system prompt are derived. An empty aggregate (no handlers, or
        // nothing contributed) leaves the discovered registry untouched (Pi's early returns at
        // :2118/:2124).
        let resources = {
            let agg = ext_host.aggregate_resources(&cancel.token()).await;
            // Fold BOTH the extension-contributed paths AND the explicit CLI `--skill`/
            // `--prompt-template`/`--theme` paths (Pi `additionalSkillPaths` et al.) into the
            // discovered registry before skill-pointer + system-prompt derivation. An empty aggregate
            // (no handlers, no CLI paths) leaves the discovered registry untouched.
            let mut skill_paths: Vec<PathBuf> = agg.skill_paths.iter().map(PathBuf::from).collect();
            let mut prompt_paths: Vec<PathBuf> = agg.prompt_paths.iter().map(PathBuf::from).collect();
            let mut theme_paths: Vec<PathBuf> = agg.theme_paths.iter().map(PathBuf::from).collect();
            skill_paths.extend(cfg.extra_skill_paths.iter().cloned());
            prompt_paths.extend(cfg.extra_prompt_paths.iter().cloned());
            theme_paths.extend(cfg.extra_theme_paths.iter().cloned());
            if skill_paths.is_empty() && prompt_paths.is_empty() && theme_paths.is_empty() {
                report.registry
            } else {
                let extra =
                    cyrup_resources::DiscoveredPaths { skill_paths, prompt_paths, theme_paths };
                report.registry.extend(&extra)
            }
        };
        let resources = Arc::new(resources);
        // Read-gated skill pointers (R-06-010): only when the `read` tool is available.
        let skills: Vec<SkillPointer> = if read_available && !cfg.no_skills {
            resources.skills.winners().map(|s| s.pointer()).collect()
        } else {
            Vec::new()
        };

        // ---- 6. context store + system prompt (cyrup-session arch-06) -------------------------
        let loader = ContextFileLoader::new(
            cwd.clone(),
            cfg.agent_dir.clone(),
            trusted,
            cfg.no_context_files,
        );
        let context_store = Arc::new(cyrup_session::prompt::ContextStore::new());
        context_store
            .reload(&cancel, loader, Arc::from(skills), ResolvedOverride::default())
            .await?;
        let snapshot = context_store.snapshot();

        let selected_tools: Vec<Arc<str>> =
            base_tools.iter().map(|t| Arc::from(t.name())).collect();
        let tool_contributions: Vec<ToolPromptContribution> =
            base_tools.iter().map(|t| tool_contribution(t.name())).collect();

        let prompt_inputs = PromptInputs {
            custom_prompt: cfg.system_prompt.clone().map(Arc::from),
            selected_tools,
            tool_contributions,
            prompt_guidelines: Vec::new(),
            append_system_prompt: cfg.append_system_prompt.clone().map(Arc::from),
            cwd: cwd.clone(),
            context_files: snapshot.context_files.clone(),
            skills: snapshot.skills.clone(),
            docs: DocsPointers::default(),
            today: today(),
        };
        let system_prompt = SystemPromptBuilder::new().build(&prompt_inputs);

        // The dynamic-tool registry (Pi `_toolRegistry`): every Availability-visible tool plus the
        // caller's custom tools are enable-able; the active set starts at the build-time selection.
        let mut registry_tools = visible.clone();
        registry_tools.extend(cfg.custom_tools.iter().cloned());
        let contributions: std::collections::BTreeMap<String, ToolPromptContribution> = registry_tools
            .iter()
            .map(|t| (t.name().to_string(), tool_contribution(t.name())))
            .collect();
        // The rebuilder base = the prompt inputs with the per-run tool fields cleared (re-derived
        // from the active set on each `setActiveToolsByName`).
        let mut rebuild_base = prompt_inputs.clone();
        rebuild_base.selected_tools = Vec::new();
        rebuild_base.tool_contributions = Vec::new();
        let dynamic_tools = crate::tools::DynamicToolState::new(
            registry_tools,
            base_tools.clone(),
            crate::tools::PromptRebuilder::new(rebuild_base, contributions),
        );

        // ---- 7. seed the agent transcript from the resumed branch (R-04-011). The manager was
        // created at step 2b; `existing` already holds its context.
        let seed: Vec<cyrup_agent::AgentMessage> =
            existing.messages.iter().map(core_message_to_agent).collect();

        // ---- 8. extension host seams (cyrup-ext) — the host itself was built at step 4b ---------
        let active_tools = ext_host.active_tools(&base_tools)?;
        let session_cancel = CancelToken::new();
        let ext_subscriber = ext_host.subscriber(session_cancel.clone());
        let ext_hooks = ext_host.hooks();

        // ---- 9. agent loop: provider + tools + composed hooks + both seams --------------------
        // `blockImages` defense-in-depth (Pi sdk.ts:254-289): the convert-to-llm seam strips image
        // content when the setting is on, deduping consecutive placeholders. Folded into PolicyHooks
        // so it rides the agent's single `convertToLlm` slot.
        let block_images = settings.effective().block_images();
        let policy_hooks = Arc::new(crate::hooks::PolicyHooks::new(
            cfg.permission_policy.clone(),
            ext_hooks,
            has_ui,
            block_images,
        ));
        let eff = settings.effective();
        // Provider attribution + opencode session headers (Pi sdk.ts:323-330, #20). Telemetry is the
        // env override (`CYRUP_TELEMETRY`/`PI_TELEMETRY`) else the `enableInstallTelemetry` setting.
        let env = cyrup_config::EnvVars::from_process();
        let telemetry_enabled = env.telemetry.unwrap_or_else(|| eff.enable_install_telemetry());
        let attribution_headers = crate::attribution::merge_provider_attribution_headers(
            &resolved_model,
            telemetry_enabled,
            Some(&session_id),
            &[],
        );
        let mut agent_builder = Agent::builder(
            model_ref.clone(),
            Arc::new(ProviderStreamFn::new(self.provider.clone())),
        )
        .system_prompt(system_prompt.clone())
        .thinking_level(thinking)
        .tools(active_tools)
        .messages(seed)
        .hooks(policy_hooks)
        .session_id(session_id.clone())
        // Settings→Agent wiring (Pi sdk.ts:356-360): queue modes + custom thinking budgets.
        .steering_mode(parse_queue_mode(&eff.steering_mode()))
        .follow_up_mode(parse_queue_mode(&eff.follow_up_mode()));
        if let Some(h) = attribution_headers {
            agent_builder = agent_builder.headers(h);
        }
        if let Some(budgets) = eff.thinking_budgets() {
            // Map the config struct (`i64`) to the provider struct (`u64`); negatives clamp to 0.
            let to_u64 = |v: Option<i64>| v.map(|n| n.max(0) as u64);
            agent_builder = agent_builder.thinking_budgets(cyrup_provider::ThinkingBudgets {
                minimal: to_u64(budgets.minimal),
                low: to_u64(budgets.low),
                medium: to_u64(budgets.medium),
                high: to_u64(budgets.high),
            });
        }
        // HTTP proxy + idle-timeout from settings (Pi `applyHttpProxySettings(settings.httpProxy)` +
        // `configureHttpDispatcher(getHttpIdleTimeoutMs())`, main.ts:744-745). The `httpProxy` setting
        // becomes a provider-scoped `HTTP_PROXY`/`HTTPS_PROXY` overlay (Pi `StreamOptions.env`) that the
        // provider's proxy resolver honors; the idle timeout becomes the request `timeout_ms`. The
        // setting-only read (empty `EnvVars` for the fallback) mirrors Pi reading the setting value.
        if let Some(overlay) =
            http_proxy_overlay(eff.http_proxy(&cyrup_config::EnvVars::default()).as_deref())
        {
            agent_builder = agent_builder.provider_env(overlay);
        }
        if let Ok(timeout_ms) = eff.http_idle_timeout_ms()
            && timeout_ms > 0
        {
            agent_builder = agent_builder.timeout_ms(timeout_ms);
        }
        let agent = agent_builder.build();

        // Seed the model + thinking-level change entries so a future resume can restore them, and
        // backfill a thinking entry for a resumed session that lacks one (Pi sdk.ts:363-375).
        if has_existing_session {
            if !has_thinking_entry {
                manager.append_thinking_level_change(&thinking_level_to_str(thinking))?;
            }
        } else {
            manager.append_model_change(resolved_model.provider.clone(), resolved_model.id.clone())?;
            manager.append_thinking_level_change(&thinking_level_to_str(thinking))?;
        }

        let manager = Arc::new(AsyncMutex::new(manager));
        let fanout = Arc::new(Fanout::new());

        // Attach the extension notify seam, then the facade's persist+fan-out subscriber.
        agent.subscribe(ext_subscriber);
        agent.subscribe(Arc::new(SvcSubscriber::new(fanout.clone(), manager.clone())));
        let agent = Arc::new(agent);

        // ---- 10. assemble the session --------------------------------------------------------
        // The concrete host-services backend, seeded with the active model so a loaded extension's
        // `models`/`current_model`/`context_usage` imports reflect live state (arch-08 §5.6).
        let host_services =
            Arc::new(crate::host_services::LiveHostServices::new(self.provider.clone()));
        host_services.update_model(
            model_ref.clone(),
            resolved_model.context_window,
            Some(thinking_level_to_str(thinking)),
        );
        // Wire the command-tier control channel so a loaded extension's `control` capability
        // (new/switch/fork/compact/set-model/…) reaches a real session effect: the SYNC guest call
        // queues a `ControlOp` that `AgentSession::apply_pending_control` drains + applies (Pi
        // `createCommandContext`, agent-session.ts:1158). The same `host_services` is the
        // `Arc<dyn HostServices>` a wasm host load injects, so guest capabilities reach live state.
        host_services.wire_control_channel();

        // Resolve the settings-driven knobs for the retry / auto-compaction subsystems BEFORE the
        // `settings` value is moved into the services bundle.
        let eff = settings.effective();
        let cfg_compaction = eff.compaction_settings();
        let to_u32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
        let extras = crate::session::SessionExtras {
            telemetry_enabled,
            compaction_settings: cyrup_session::compaction::CompactionSettings {
                enabled: cfg_compaction.enabled,
                reserve_tokens: to_u32(cfg_compaction.reserve_tokens),
                keep_recent_tokens: to_u32(cfg_compaction.keep_recent_tokens),
            },
            branch_summary_settings: cyrup_session::compaction::BranchSummarySettings {
                reserve_tokens: to_u32(eff.branch_summary_reserve_tokens()),
                skip_prompt: eff.branch_summary_skip_prompt(),
            },
            auto_compaction_enabled: eff.compaction_enabled(),
            auto_retry_enabled: eff.retry_enabled(),
            retry_max_retries: to_u32(eff.retry_max_retries()),
            retry_base_delay_ms: u64::try_from(eff.retry_base_delay_ms().max(0)).unwrap_or(0),
            proc: bash_proc,
            shell,
            dynamic_tools,
        };

        let services = AgentSessionServices {
            cwd,
            agent_dir: cfg.agent_dir.clone(),
            home: cfg.home.clone(),
            settings,
            project_trusted: trusted,
            auth,
            resources,
            context: context_store,
            ext_host,
            model: resolved_model,
            system_prompt,
            host_services,
            extension_flag_values: cfg.extension_flag_values.clone(),
        };

        Ok(AgentSession::from_parts(
            agent,
            manager,
            fanout,
            self.provider,
            services,
            model_ref,
            session_cancel,
            session_id,
            model_fallback_message,
            extras,
        ))
    }
}

/// Parse the settings `steeringMode`/`followUpMode` string into the agent's [`QueueMode`]
/// (Pi `"all"|"one-at-a-time"`; settings-manager.ts:698-710). Any non-`all` value ⇒ one-at-a-time.
pub(crate) fn parse_queue_mode(s: &str) -> cyrup_agent::QueueMode {
    if s == "all" { cyrup_agent::QueueMode::All } else { cyrup_agent::QueueMode::OneAtATime }
}

/// Serialize a [`ModelThinkingLevel`] to its persisted snake/camel key (`off`/`minimal`/…/`xhigh`).
pub(crate) fn thinking_level_to_str(level: ModelThinkingLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "off".to_string())
}

/// Parse a persisted thinking-level key back into a [`ModelThinkingLevel`] (inverse of
/// [`thinking_level_to_str`]); unknown keys ⇒ `None`.
pub(crate) fn thinking_level_from_str(s: &str) -> Option<ModelThinkingLevel> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// Resolve `(Model, ModelRef, ModelThinkingLevel, modelFallbackMessage)` from the explicit pattern,
/// the resumed session, settings, and finally the catalog (Pi sdk.ts:191-242; R-07-019).
///
/// Precedence mirrors Pi: an explicit `--model` pattern wins; otherwise a resumed session's saved
/// model is restored when it is still resolvable in the catalog (else a `modelFallbackMessage` is
/// produced and we fall back to settings/catalog). The thinking level is likewise restored from the
/// session's `thinking_level_change` entry, then clamped to the chosen model's capabilities.
fn resolve_model(
    provider: &dyn Provider,
    cfg: &SessionConfig,
    settings: &SettingsManager,
    existing: &cyrup_session::context::SessionContext,
    has_existing_session: bool,
    has_thinking_entry: bool,
) -> Result<(Model, ModelRef, ModelThinkingLevel, Option<String>), SessionServiceError> {
    let available = provider.models();
    if available.is_empty() {
        return Err(SessionServiceError::NoModels(provider.id().to_string()));
    }
    let resolver = ModelResolver::new(available);
    let mut fallback: Option<String> = None;

    // 1. An explicit `--model` pattern (Pi `options.model`) takes precedence over restore.
    let (mut model, mut parsed_thinking): (Option<Model>, Option<ModelThinkingLevel>) =
        match &cfg.model_pattern {
            Some(pat) => {
                let parsed = resolver.parse_pattern(pat, true);
                match parsed.model {
                    Some(m) => (Some(m), parsed.thinking_level),
                    // Pi `resolveCliModel` fallback (model-resolver.ts:475-501): an unresolvable
                    // `--model` id on a *known* provider does NOT error — it builds a custom-id model
                    // from the provider's default and proceeds (the bin already emitted the
                    // "Using custom model id." warning). The provider is "known" when `--provider` was
                    // explicit OR the pattern carries a `provider/` prefix naming the resolved
                    // provider; a bare unresolvable id with neither stays a hard `ModelNotFound`.
                    None => match fallback_model(&resolver, provider, cfg, pat) {
                        Some((m, level)) => (Some(m), level),
                        None => return Err(SessionServiceError::ModelNotFound(pat.clone())),
                    },
                }
            }
            None => (None, None),
        };

    // 2. Restore the model from the resumed session (Pi sdk.ts:194-203). The saved model is only
    //    honored when it still resolves in the live catalog (our auth proxy: a model the provider
    //    exposes is usable); otherwise we record the fallback message and keep searching.
    if model.is_none()
        && has_existing_session
        && let Some(saved) = existing.model.as_ref()
    {
        let restored = available.iter().find(|m| {
            m.provider == saved.provider && m.id == saved.model
        });
        match restored {
            Some(m) => model = Some(m.clone()),
            None => {
                fallback = Some(format!(
                    "Could not restore model {}/{}",
                    saved.provider.as_str(),
                    saved.model.as_str()
                ));
            }
        }
    }

    // 3. Settings default → first catalog entry (Pi `findInitialModel`, sdk.ts:205-221).
    if model.is_none() {
        let pat = settings.effective().default_model();
        let resolved = match pat {
            Some(p) => {
                let parsed = resolver.parse_pattern(&p, true);
                parsed_thinking = parsed_thinking.or(parsed.thinking_level);
                parsed.model
            }
            None => None,
        };
        let m = match resolved {
            Some(m) => m,
            None => available.first().cloned().ok_or_else(|| {
                SessionServiceError::NoModels(provider.id().to_string())
            })?,
        };
        if let Some(msg) = fallback.as_mut() {
            msg.push_str(&format!(". Using {}/{}", m.provider.as_str(), m.id.as_str()));
        }
        model = Some(m);
    }
    let model = model.ok_or_else(|| SessionServiceError::NoModels(provider.id().to_string()))?;

    // 4. Thinking level: explicit option → restored from session → settings default; clamped to the
    //    chosen model's supported levels (Pi sdk.ts:223-242).
    let mut thinking = cfg.thinking_level.or(parsed_thinking);
    if thinking.is_none() && has_existing_session {
        thinking = Some(if has_thinking_entry {
            thinking_level_from_str(&existing.thinking_level)
                .unwrap_or_else(|| settings.effective().default_thinking_level())
        } else {
            settings.effective().default_thinking_level()
        });
    }
    let thinking = thinking.unwrap_or_else(|| settings.effective().default_thinking_level());
    let thinking = cyrup_provider::clamp_thinking_level(&model, thinking);

    let model_ref = ModelRef {
        provider: model.provider.clone(),
        api: Some(model.api.clone()),
        model: model.id.clone(),
    };
    Ok((model, model_ref, thinking, fallback))
}

/// Pi `resolveCliModel` custom-fallback (model-resolver.ts:475-501 + `buildFallbackModel`
/// 163-177): when a strict `--model` pattern does not resolve but the provider is *known*, clone the
/// provider's default (alias-preferred, else first) model and override `id`/`name` with the
/// requested model id, so an unknown-but-intended model id proceeds as a custom model. The provider
/// is "known" when `--provider` was explicit (`cli_provider_explicit`) or the pattern carries a
/// `provider/` prefix naming the resolved provider. Returns `(model, thinking_level)` or `None` (⇒
/// the caller keeps Pi's hard `ModelNotFound`). A trailing `:level` is honored only when `--thinking`
/// was not given (Pi `fallbackThinking`, model-resolver.ts:481-490).
fn fallback_model(
    resolver: &ModelResolver,
    provider: &dyn Provider,
    cfg: &SessionConfig,
    pattern: &str,
) -> Option<(Model, Option<ModelThinkingLevel>)> {
    let provider_id = provider.id();
    let prefix = format!("{}/", provider_id.as_str());
    let has_matching_prefix =
        pattern.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase());
    if !cfg.cli_provider_explicit && !has_matching_prefix {
        return None;
    }
    // Strip the provider prefix (Pi `pattern = cliModel.substring(slashIndex + 1)`), then peel a
    // trailing `:level` thinking suffix when `--thinking` was not explicitly set.
    let stripped: &str =
        if has_matching_prefix { pattern.get(prefix.len()..).unwrap_or(pattern) } else { pattern };
    let (base_id, level): (&str, Option<ModelThinkingLevel>) = if cfg.thinking_level.is_some() {
        (stripped, None)
    } else if let Some(idx) = stripped.rfind(':') {
        let suffix = stripped.get(idx + 1..).unwrap_or("");
        match thinking_level_from_str(suffix) {
            Some(lvl) => (stripped.get(..idx).unwrap_or(stripped), Some(lvl)),
            None => (stripped, None),
        }
    } else {
        (stripped, None)
    };
    if base_id.is_empty() {
        return None;
    }
    // Clone the provider default (alias-preferred) or the first model, overriding id + name.
    let base = resolver
        .provider_default(provider_id)
        .cloned()
        .or_else(|| provider.models().first().cloned())?;
    let mut model = base;
    model.id = cyrup_core::ModelId::from(base_id);
    model.name = base_id.to_string();
    Some((model, level))
}

/// Synthesize a one-line prompt snippet for a built-in tool so the "Available tools" section is
/// populated (arch-06 R-06-012). Mirrors the built-ins' own `prompt_snippet` intent.
fn tool_contribution(name: &str) -> ToolPromptContribution {
    let snippet = match name {
        "read" => "Read a file from the workspace",
        "write" => "Write a file to the workspace",
        "edit" => "Edit a file with a find/replace",
        "bash" => "Run a shell command",
        "grep" => "Search file contents",
        "find" => "Find files by glob",
        "ls" => "List a directory",
        _ => "Tool",
    };
    ToolPromptContribution::snippet(Arc::<str>::from(name), Arc::<str>::from(snippet))
}

/// Build the extension discovery roots from the config (Pi `resourceLoaderOptions`
/// `additionalExtensionPaths` + `noExtensions`, main.ts:660,664). `--no-extensions`/`-ne` disables the
/// project (`<cwd>/.cyrup/extensions`) + global (`<agentDir>/extensions`) discovery roots; explicit
/// `--extension`/`-e` paths are always loaded (Pi: "explicit -e paths still work" — they are pre-trust
/// *configured* roots). Pure + side-effect-free so it is unit-testable without a wasm host.
pub fn extension_discovery_roots(cfg: &SessionConfig) -> cyrup_ext::DiscoveryRoots {
    if cfg.no_extensions {
        cyrup_ext::DiscoveryRoots {
            project_cwd: None,
            agent_dir: None,
            configured: cfg.extra_extension_paths.clone(),
        }
    } else {
        cyrup_ext::DiscoveryRoots {
            project_cwd: Some(cfg.cwd.clone()),
            agent_dir: Some(cfg.agent_dir.clone()),
            configured: cfg.extra_extension_paths.clone(),
        }
    }
}

/// Map the runtime mode to the extension `(ExtMode, has_ui)` (R-11-002).
fn ext_mode(mode: AppMode) -> (ExtMode, bool) {
    match mode {
        AppMode::Interactive => (ExtMode::Tui, true),
        AppMode::Rpc => (ExtMode::Rpc, true),
        AppMode::Json => (ExtMode::Json, false),
        AppMode::Print => (ExtMode::Print, false),
    }
}

/// Today's date (UTC) for the prompt footer; falls back to the epoch on a clock fault.
fn today() -> time::Date {
    time::OffsetDateTime::now_utc().date()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::http_proxy_overlay;

    #[test]
    fn http_proxy_overlay_sets_both_proxy_keys_or_none() {
        // Pi `applyHttpProxySettings` (http-dispatcher.ts:42-47): a non-empty setting sets both
        // HTTP_PROXY and HTTPS_PROXY (so the provider proxy resolver routes through it).
        let overlay = http_proxy_overlay(Some("http://proxy.local:8080")).expect("an overlay");
        assert_eq!(overlay.get("HTTP_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
        assert_eq!(overlay.get("HTTPS_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
        // A blank / whitespace / absent setting yields no overlay (ambient env unchanged).
        assert!(http_proxy_overlay(Some("   ")).is_none());
        assert!(http_proxy_overlay(Some("")).is_none());
        assert!(http_proxy_overlay(None).is_none());
    }
}
