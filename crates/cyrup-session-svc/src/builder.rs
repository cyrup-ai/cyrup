//! `SessionConfig` + `SessionBuilder` — assemble an [`AgentSession`] from the real subsystems
//! (arch-11 §3.3). One async `build()` resolves settings + trust + auth + model (cyrup-config),
//! discovers resources (cyrup-resources), builds the tool registry with isolation decorators +
//! permission policy (cyrup-tools), opens/creates the session tree and wires compaction
//! (cyrup-session arch-04/05), assembles the system prompt + context store (arch-06), builds the
//! extension host with native built-ins and attaches BOTH ext seams to the agent (cyrup-ext), and
//! resolves the provider into the agent loop (cyrup-provider).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_agent::Agent;
use cyrup_core::{CancelToken, ModelRef, RunCancel, ModelThinkingLevel};
use cyrup_config::{
    decide_trust_with_extension, has_trust_requiring_resources, AppMode, AuthStore,
    ExtensionTrust, InMemorySettingsStore,
    ModelResolver, Settings, SettingsManager, SettingsStore, TrustInputs, TrustOutcome,
};
use cyrup_ext::{EventKind, ExtMode, ExtensionHost, HostConfig, HostEvent, NativeExtension};
use cyrup_provider::{Model, Provider};
use cyrup_resources::{
    discover, ConfiguredPackage, DiscoveryConfig, InstallScope, InstalledPackages, PackageFilter,
    PackageStore, ResourceOverrides, SkillPointer,
};
use cyrup_session::manager::{NewSessionOpts, SessionManager};
use cyrup_session::prompt::{
    ContextFile, ContextFileLoader, ContextSnapshot, DocsPointers, PromptInputs, ResolvedOverride,
    SystemPromptBuilder, ToolPromptContribution,
};
use cyrup_session::SessionLayout;
use cyrup_tools::{
    Availability, Backend, BashOpts, PermissionPolicy, ProtectedFs, ShellConfig, ToolRegistry,
    ToolsOptions, TraversalFs,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::SessionServiceError;
use crate::event::core_message_to_agent;
use crate::provider_swap::{ProviderResolver, ProviderSwap};
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
    /// Packages root — the value the bin passes as the `PackageStore` global root when it writes
    /// `install` records (`PackageStore::new(dirs.package_dir, …)`, subcommands.rs:396; Pi
    /// `dirs.package_dir`, env.rs:156-160, default `<agent_dir>/packages`). The builder reads the
    /// SAME registry back into `DiscoveryConfig.installed`/`package_global_dir` so an installed
    /// package's resources load into the assembled session (gap-07 #1 / gap-13 C1). Defaults to
    /// `<agent_dir>/packages` — the bin's own default. NOTE: the bin's `to_session_config` currently
    /// leaves this at the default, so a non-default `--package-dir`/`CYRUP_PACKAGE_DIR` is not yet
    /// threaded here (closing that residual is a one-line bin edit, outside this crate's scope).
    pub package_dir: PathBuf,
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
            package_dir: agent_dir.join("packages"),
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

/// Every tool `ToolRegistry::with_builtins` installs (`cyrup-tools/src/registry.rs:45-67`).
///
/// Needed to tell "a built-in pi does not activate by default" (`grep`/`find`/`ls`) apart from "a
/// non-built-in tool" (an extension- or embedder-supplied one), which must stay active: pi's
/// `defaultActiveToolNames` gates only its own built-ins and never suppresses a tool the host
/// registered.
const ALL_BUILTIN_TOOLS: [&str; 7] =
    ["read", "write", "edit", "bash", "grep", "find", "ls"];

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
            // pi `sdk.ts:244-250`: with no `tools`/`noTools` the active set is
            // `defaultActiveToolNames` — read/bash/edit/write — NOT every visible tool. Confirmed
            // at the same tag in `agent-session.ts:2592-2594`, and `_refreshToolRegistry`
            // (`:2524-2546`) only ever WIDENS it.
            //
            // This arm returned `true`, so every cyrup session advertised three tools pi does not
            // (`grep`, `find`, `ls`). That changed the tool array in every provider request AND the
            // system prompt (their `prompt_snippet`/`prompt_guidelines` are injected via
            // `tool_contribution`), so the model routed searches to `grep`/`find` instead of `bash`
            // — different transcripts, different token counts, different tool-call sequences than
            // pi for identical inputs — and it silently widened the surface a permission policy has
            // to cover.
            //
            // `registry.visible(...)` is deliberately NOT narrowed: grep/find/ls remain
            // ENABLE-able at runtime via `set_active_tools_by_name`, exactly as pi's
            // `_refreshToolRegistry` can widen its own active set. This changes the DEFAULT, not
            // what is reachable.
            (None, None) => {
                DEFAULT_BUILTIN_TOOLS.contains(&name) || !ALL_BUILTIN_TOOLS.contains(&name)
            }
        }
    };
    visible
        .iter()
        .filter(|t| keep(t.name()) && !exclude.contains(t.name()))
        .cloned()
        .collect()
}

/// A synthetic-skill override closure (Pi `DefaultResourceLoader.skillsOverride`): transforms the
/// discovered [`SkillPointer`] set before it feeds the system prompt.
type SkillsOverrideFn = Box<dyn FnOnce(Vec<SkillPointer>) -> Vec<SkillPointer> + Send>;
/// A synthetic context-file override closure (Pi `DefaultResourceLoader.agentsFilesOverride`).
type ContextFilesOverrideFn = Box<dyn FnOnce(Vec<ContextFile>) -> Vec<ContextFile> + Send>;

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
    /// Provider resolver seam (the bin's `select_provider`) enabling live cross-provider `/model`
    /// swaps. `None` ⇒ only same-provider model changes are possible (tests / offline builds).
    provider_resolver: Option<Arc<dyn ProviderResolver>>,
    /// Custom transport override (Pi `AgentOptions.streamFn`, sdk.ts:301-331; the proxy-closure
    /// example, proxy.ts:92-98). When `Some`, the agent loop streams through THIS `StreamFn` (e.g. a
    /// [`cyrup_agent::ProxyStreamFn`] routing through an auth-managing proxy backend, R-11-022)
    /// instead of the default provider-backed [`ProviderSwap`] — the embedder brings its own
    /// transport. `None` ⇒ the provider-backed default (the live-swappable path).
    stream_fn: Option<Arc<dyn cyrup_agent::StreamFn>>,
    /// Dynamic per-request API-key resolution (Pi per-request key resolution). Threaded onto the
    /// agent's `key_resolver` slot (`AgentBuilder::key_resolver`, agent.rs:1585); consulted on every
    /// turn and its result takes precedence over any static key. `None` ⇒ no dynamic override.
    key_resolver: Option<Arc<dyn cyrup_agent::ApiKeyResolver>>,
    /// Synthetic-skill injection closure (Pi `DefaultResourceLoader.skillsOverride`,
    /// resource-loader.ts:143,630). Runs over the discovered [`SkillPointer`]s before they feed the
    /// system prompt / context snapshot, so an embedder can inject in-memory skills not backed by
    /// files on disk. `None` ⇒ the discovered set passes through unchanged.
    skills_override: Option<SkillsOverrideFn>,
    /// Synthetic context-file (`AGENTS.md`/`CLAUDE.md`) injection closure (Pi
    /// `DefaultResourceLoader.agentsFilesOverride`, resource-loader.ts:155,474). Runs over the loaded
    /// [`ContextFile`]s before the system prompt reads them. `None` ⇒ the loaded set is used verbatim.
    context_files_override: Option<ContextFilesOverrideFn>,
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
            provider_resolver: None,
            stream_fn: None,
            key_resolver: None,
            skills_override: None,
            context_files_override: None,
        }
    }

    /// Wire the provider resolver seam (the bin's `select_provider`) so a `/model` selection that
    /// targets a different provider than the current one swaps the owning provider live.
    #[must_use]
    pub fn provider_resolver(mut self, resolver: Arc<dyn ProviderResolver>) -> Self {
        self.provider_resolver = Some(resolver);
        self
    }

    /// Inject a custom transport (Pi `AgentOptions.streamFn`, sdk.ts:301; the `ProxyStreamFn`
    /// proxy-closure example, proxy.ts:92-98). The agent loop streams through `stream_fn` — e.g. a
    /// [`cyrup_agent::ProxyStreamFn`] — instead of the provider-backed default. The injected
    /// `provider` still resolves the model catalog / model-ref; only the wire transport is replaced.
    #[must_use]
    pub fn stream_fn(mut self, stream_fn: Arc<dyn cyrup_agent::StreamFn>) -> Self {
        self.stream_fn = Some(stream_fn);
        self
    }

    /// Inject a dynamic API-key resolver (Pi per-request key resolution). Consulted on every turn
    /// (agent.rs:599); its result overrides any static configured key.
    #[must_use]
    pub fn key_resolver(mut self, resolver: Arc<dyn cyrup_agent::ApiKeyResolver>) -> Self {
        self.key_resolver = Some(resolver);
        self
    }

    /// Inject/transform synthetic skills (Pi `DefaultResourceLoader.skillsOverride`,
    /// resource-loader.ts:143,630). The closure receives the discovered [`SkillPointer`]s and returns
    /// the set that feeds the system prompt (add in-memory skills, drop discovered ones, or replace
    /// the whole set). Skills still render only when the `read` tool is available (R-06-010).
    #[must_use]
    pub fn skills_override(
        mut self,
        f: impl FnOnce(Vec<SkillPointer>) -> Vec<SkillPointer> + Send + 'static,
    ) -> Self {
        self.skills_override = Some(Box::new(f));
        self
    }

    /// Inject/transform synthetic context (`AGENTS.md`/`CLAUDE.md`) files (Pi
    /// `DefaultResourceLoader.agentsFilesOverride`, resource-loader.ts:155,474). The closure receives
    /// the discovered [`ContextFile`]s and returns the set the system prompt reads.
    #[must_use]
    pub fn context_files_override(
        mut self,
        f: impl FnOnce(Vec<ContextFile>) -> Vec<ContextFile> + Send + 'static,
    ) -> Self {
        self.context_files_override = Some(Box::new(f));
        self
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

    /// Load `<agent_dir>/models-store.json` as a model-catalog overlay, WITHOUT any network access
    /// (DRIFT-007).
    ///
    /// Infallible and disk-only. A missing/corrupt cache, an overlay no newer than the compiled-in
    /// catalogs (the post-upgrade case, pi #7016), or an entry that mislabels its provider all yield
    /// `None`, which is byte-identical to the pre-DRIFT-007 behavior. It can never remove a built-in
    /// model, so a session built from a broken cache is never worse off than one built from none.
    async fn load_persisted_catalog_overlay(
        agent_dir: &std::path::Path,
    ) -> Option<Arc<cyrup_provider::CatalogOverlay>> {
        let store: Arc<dyn cyrup_provider::ModelsStore> = Arc::new(
            cyrup_config::models_store::FileModelsStore::new(
                agent_dir.join(cyrup_config::models_store::MODELS_STORE_FILE_NAME),
            ),
        );
        let catalog = cyrup_provider::RemoteCatalog::new(store)
            .with_local_generated_at(cyrup_provider::builtin_model_data_generated_at());
        let ids: Vec<String> = cyrup_provider::all_providers()
            .iter()
            .map(|p| p.id().as_str().to_string())
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let overlay = catalog.load_overlay(&refs).await;
        (!overlay.is_empty()).then(|| Arc::new(overlay))
    }

    /// Assemble the wired [`AgentSession`] (arch-11 §3.3). Async: discovery + context load + native
    /// extension `init` run here.
    pub async fn build(self) -> Result<AgentSession, SessionServiceError> {
        let cfg = self.config;
        // Embedder-supplied seams pulled out before the rest of `self` is consumed piecewise below.
        let custom_stream_fn = self.stream_fn;
        let custom_key_resolver = self.key_resolver;
        let skills_override = self.skills_override;
        let context_files_override = self.context_files_override;
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
        // Pi's `shouldResolveProjectTrust` guard (main.ts:676-678): only pay for a pre-trust
        // extension pass when the answer is actually in doubt — no explicit `--approve/--no-approve`
        // and there IS something to gate. In every other case this is the exact previous code path.
        let ext_trust = if cfg.trust_override.is_none() && has_resources {
            pre_trust_extension_verdict(&cfg, &cwd, &self.native_extensions).await
        } else {
            None
        };
        if let Some(d) = &ext_trust
            && d.remember
        {
            // Pi persists via `trustStore.set` (project-trust.ts:46-95). The builder has no
            // `TrustStore` wired (it also passes `saved: None`), so say so rather than pretending
            // the decision was remembered.
            tracing::warn!(
                extension = %d.by, trusted = d.trusted,
                "extension project_trust asked to `remember` the decision, but no trust store is \
                 wired into the session builder — the verdict applies to this session only"
            );
        }
        let outcome = decide_trust_with_extension(
            TrustInputs {
                has_resources,
                trust_override: cfg.trust_override,
                saved: None,
                default_trust,
                mode: cfg.app_mode,
                prompt_choice: None,
            },
            ext_trust.map(|d| ExtensionTrust { trusted: d.trusted, remember: d.remember }),
        );
        let trusted = matches!(outcome, TrustOutcome::Trusted);
        settings.set_project_trusted(trusted);

        // ---- 2. auth (cyrup-config) ------------------------------------------------------------
        let auth = self
            .auth
            .unwrap_or_else(|| Arc::new(AuthStore::at(cfg.agent_dir.join("auth.json"))));

        // ---- 2b. session tree (cyrup-session arch-04) — created BEFORE model resolution so the
        // model/thinking restore can read the resumed branch (Pi sdk.ts:178,187: the SessionManager
        // is constructed, then `buildSessionContext()` feeds `existingSession.model`/`thinkingLevel`).
        // Pi chooses the session directory per call: an explicit `sessionDir` (`--session-dir`) is
        // used LITERALLY, otherwise the cwd-encoded default `getDefaultSessionDir(cwd)` applies
        // (`sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)`,
        // session-manager.ts:1430,1457,1496). `cfg.session_dir` is `Some` only when `--session-dir`
        // (or its env) was explicitly supplied (the "was it explicit" signal ConfigDirs collapses one
        // layer too early is preserved as this `Option`); `None` ⇒ the encoded default. Using the
        // encoded [`SessionLayout::new`] on an explicit dir would nest one level too deep
        // (gap-analysis 05, Finding 3).
        let default_root = cfg.agent_dir.join("sessions");
        let layout = match &cfg.session_dir {
            Some(dir) => SessionLayout::literal(dir.clone(), cwd.clone()),
            None => SessionLayout::new(default_root.clone(), cwd.clone()),
        };
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
                // Pi `continueRecent` applies a cross-project cwd filter exactly when a custom
                // `sessionDir` is in play and it is not the cwd-default
                // (`filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)`,
                // session-manager.ts:1458), so a shared `--session-dir` holding several projects'
                // sessions only resumes the current project's. The default (encoded) root already
                // isolates by cwd, so it never filters.
                SessionTarget::Continue => {
                    let filter_cwd = match &cfg.session_dir {
                        Some(dir) => {
                            *dir != SessionLayout::new(default_root.clone(), cwd.clone()).dir()
                        }
                        None => false,
                    };
                    SessionManager::continue_recent_filtered(&cwd, &layout, filter_cwd)?
                }
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
        // `shellPath`/`shellCommandPrefix` settings (Pi `getShellPath`/`getShellCommandPrefix`,
        // settings-manager.ts:864-865,895-896), read once here and threaded into BOTH bash seams:
        // the agent-loop `bash` tool (via `ToolsOptions.bash` below, matching Pi's `_buildRuntime`
        // passing `{commandPrefix, shellPath}` into `createAllToolDefinitions`, agent-session.ts:
        // 2436-2448) and the immediate-bash RPC seam (via `SessionExtras` below, matching Pi's
        // `executeBash` re-reading the same two settings, agent-session.ts:2624-2632).
        let shell_path_setting = settings.effective().shell_path();
        let shell_command_prefix_setting = settings.effective().shell_command_prefix();
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
        // The live session metadata every `bash` child gets as `CYRUP_*` (Pi's `resolveSpawnContext`
        // reads the same five values off the per-call `ExtensionContext`, bash.ts:171-181). Pi's
        // values are "resolved when each command starts" (docs/environment-variables.md:27), so this
        // is a shared HANDLE the session mutates on `set_model` / `set_thinking_level`, never a
        // snapshot baked into the tool.
        // `read`'s non-vision-model warning (pi `tools/read.ts`): the handle is seeded from the
        // RESOLVED model's declared input modalities and re-pushed on every `/model` switch, exactly
        // as `bash_session_env` carries provider/model. Without this the tool's
        // `ReadOpts::model_vision` stayed `None` and `supports_images_now()` fell back to `true`,
        // so the warning was unreachable and an image handed to a text-only model produced a
        // provider error instead of the tool's own diagnostic.
        let read_model_vision =
            cyrup_tools::config::ModelVisionHandle::new(resolved_model.supports_image_input());
        let bash_session_env = cyrup_tools::config::SessionEnvHandle::new(
            cyrup_tools::config::SessionEnvInfo {
                session_id: Some(session_id.to_string()),
                // `None` for an ephemeral/in-memory session — Pi leaves `PI_SESSION_FILE` unset
                // rather than empty in that case (bash.ts:173-174).
                session_file: manager.session_file().map(std::path::Path::to_path_buf),
                provider: Some(model_ref.provider.to_string()),
                model: Some(model_ref.model.to_string()),
                reasoning_level: Some(thinking_level_to_str(thinking)),
            },
        );
        let registry = ToolRegistry::with_builtins(
            cwd.clone(),
            backend,
            ToolsOptions {
                read: cyrup_tools::config::ReadOpts {
                    model_vision: Some(read_model_vision.clone()),
                    // `images.autoResize` (Pi `_buildRuntime`: `const autoResizeImages =
                    // this.settingsManager.getImageAutoResize()` → `read: { autoResizeImages }`,
                    // agent-session.ts:2553,2564). Without this the setting had no consumer at all
                    // and `read` downsampled every image to 2000px regardless.
                    auto_resize_images: settings.effective().image_auto_resize(),
                    ..cyrup_tools::config::ReadOpts::default()
                },
                bash: BashOpts {
                    command_prefix: shell_command_prefix_setting.clone(),
                    shell_path: shell_path_setting.clone(),
                    session_env: Some(bash_session_env.clone()),
                    // pi `getShellEnv()` (`utils/shell.ts:122-134`) unconditionally prepends
                    // `getBinDir()` to PATH for EVERY bash child (`tools/bash.ts:100,165`); there is
                    // no pi path where the bash tool spawns without it.
                    //
                    // cyrup set this only on the user-facing `/bash` seam
                    // (`session.rs:4225`, the same `<agent_dir>/bin`), leaving the agent-loop `bash`
                    // tool — the one the MODEL calls — with `bin_dir: None`, which makes
                    // `ops::shell::shell_env` return an empty overlay and inherit the parent PATH
                    // unchanged. So a binary cyrup manages into `<agent_dir>/bin` produced
                    // `command not found` for the model while the identical command succeeded
                    // through `/bash`: two bash paths in one process disagreeing about PATH, which
                    // reads as nondeterminism from the outside.
                    bin_dir: Some(cfg.agent_dir.join("bin")),
                    ..BashOpts::default()
                },
                ..ToolsOptions::default()
            },
        );
        let visible = registry.visible(&cfg.tool_availability);
        // Tool-set selection (Pi sdk.ts:244-251): an explicit `tools` allowlist, or `noTools`
        // ("all" ⇒ none; "builtin" ⇒ drop the default built-ins), then minus the `excludeTools`
        // denylist. Absent all three, the Availability-visible set is kept verbatim.
        let base_tools = select_active_tools(&visible, &cfg);
        let read_available = base_tools.iter().any(|t| t.name() == "read");

        // ---- 4a. the LIVE host-services backend (arch-08 §5.6) — built BEFORE the extension host so
        // the SAME instance is injected into every wasm load (auto-discovery here + an explicit
        // `AgentSession::load_wasm_extension`) AND stored on the session. A single instance is
        // load-bearing: a loaded guest's `control` capability routes to whichever `LiveHostServices`
        // was injected at load time, and `AgentSession::apply_pending_control` drains the one on
        // `services.host_services`; if these differ the guest's `control` op is silently lost. Seed the
        // active model + wire the command-tier control channel up front so guest reads/ops are live.
        // `bash_proc` (the local process ops) + `cwd` back the `exec` capability grant (Pi
        // `execCommand`, exec.ts:34-46): a granted extension execs argv (shell:false) through the
        // SAME process backend the `bash` seam uses, defaulting to the session cwd.
        let host_services = Arc::new(crate::host_services::LiveHostServices::new(
            self.provider.clone(),
            bash_proc.clone(),
            cwd.clone(),
        ));
        host_services.update_model(
            model_ref.clone(),
            resolved_model.context_window,
            Some(thinking_level_to_str(thinking)),
        );
        host_services.wire_control_channel();

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
        // Attach the live `getActiveTools` source BEFORE any tool set is materialized, so every tool
        // `active_tools` hands back is wrapped for `addedToolNames` derivation (Pi
        // `wrapRegisteredTool`, extensions/wrapper.ts:17-36). The source reads the SAME
        // `DynamicToolState` `setActiveTools` mutates, so a tool that widens the active set during its
        // own `execute` is observed by the wrapper's "after" snapshot. It reads `None` until
        // `attach_dynamic_tools` runs further down, which is correct: nothing executes before then.
        host.set_active_tool_source(host_services.clone());
        // P-1 (reconciliation §2 item 1): late-bind the session's OWN `host_services` into every
        // native built-in — the SAME `LiveHostServices` the WASM path gets via `discover_and_load`
        // below — so a native extension can reach the live session id/file, dialogs, and
        // message-injection from a background task OUTSIDE any `HostCtx`. `load_native_with_services`
        // calls `NativeExtension::set_host_services` before `init`; the manager / ui sink / inject sink
        // attach later (steps 6/10 + the mode entry point) and the captured `Arc` observes them.
        let native_services: Arc<dyn cyrup_ext::host::HostServices> = host_services.clone();
        // EXT-S01: CONTAIN a native extension's load/`init` failure. This loop used to propagate the
        // first error with a bare `?`, so one built-in (permission-system, intercom, subagents)
        // failing `init` took the ENTIRE session down — no session at all, and the remaining natives
        // were never even attempted. Pi records a per-extension load failure and keeps building
        // (`LoadExtensionsResult.errors`, surfaced as `Failed to load extension "<path>": <err>`,
        // main.ts:735-738). Collected here and folded into `startup_diagnostics.extensions` below
        // (the same channel the wasm/disk path at step 4c already uses), so a contained failure
        // reaches BOTH the `[Extension issues]` startup panel AND — because it is marked `fatal` —
        // `AgentSessionRuntime::diagnostics()`, where the bin reports it on stderr and exits 1 in
        // every mode (Pi main.ts:843-849). Containment is per-extension, NOT forgiveness: Pi keeps
        // building past the failure and then refuses to run. The natives are cyrup's security
        // built-ins (permission-system, intercom), so anything short of a non-zero exit would turn
        // a failed permission gate into a fail-OPEN session.
        let mut native_load_errors: Vec<crate::services::ExtensionLoadDiagnostic> = Vec::new();
        for ext in self.native_extensions {
            let id = ext.id();
            if let Err(e) = host.load_native_with_services(ext, native_services.clone()).await {
                tracing::error!(extension = %id, error = %e, "native extension failed to load");
                native_load_errors.push(crate::services::ExtensionLoadDiagnostic {
                    // A native built-in has no on-disk path; its id is the display key the panel
                    // shows (Pi's per-extension diagnostics are keyed by the loader's path).
                    path: PathBuf::from(id.as_str()),
                    error: e.to_string(),
                    fatal: true,
                });
            }
        }
        let ext_host = Arc::new(host);

        // ---- 5. resources discovery (cyrup-resources) — RUN FIRST (before disk-extension load) so
        // the package-declared extension dirs discovery collects (`registry.ext_crate_paths`) can be
        // folded into the extension discovery roots below, matching Pi's `resolve()` producing
        // `resolvedPaths.extensions` (the package tier) which is then merged into the loaded
        // extension set (resource-loader.ts:379,403-407). Discovery is a pure fs pass with no
        // dependency on the not-yet-loaded disk extensions; the extension-*contributed* resources are
        // folded in AFTER the load via `aggregate_resources` (unchanged, below).
        let mut disc = DiscoveryConfig::new(cwd.clone(), cfg.agent_dir.clone());
        // R6: plumb the user-tier cross-tool `~/.agents` base (Pi `getHomeDir()/.agents`,
        // package-manager.ts:2286,217) so cyrup-resources loads `~/.agents/skills` (user scope) and
        // dedups the project `.agents/skills` ancestor walk against it.
        disc.user_agents_dir = Some(cfg.home.join(".agents"));
        disc.trusted_project = trusted;
        // C1 (gap-07 #1 / gap-13 C1): read the on-disk install registry back into discovery so an
        // installed package's skills/prompts/themes actually load into the assembled session. Pi's
        // `PackageManager.resolve()` re-reads `projectSettings.packages`/`globalSettings.packages`
        // from the settings store on EVERY call (package-manager.ts:880-897), so an installed package
        // is structurally impossible to forget; cyrup persists installs to a SEPARATE file-backed
        // `packages.json` store, so the builder must take the explicit read step the bin's `install`
        // write mirrors (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))`, subcommands.rs:396).
        // `project_root` + `package_global_dir` are the SAME store roots `install` writes to, so
        // `installed_dir` resolves each record's working tree at the exact on-disk path `install`
        // created (Global at `<package_dir>/packages/<id>`, Project at `<cwd>/.cyrup/packages/<id>`).
        disc.project_root = Some(cwd.clone());
        disc.package_global_dir = cfg.package_dir.clone();
        disc.installed = load_installed_packages(&cfg.package_dir, &cwd);
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
        //
        // The SAME arrays also carry Pi's positive (plain-path) listings, which `resolveLocalEntries`
        // LOADS at the settings tier (package-manager.ts:905-931, :2255-2276) — including the
        // `extensions` array, the first member of Pi's `RESOURCE_TYPES` (:194). cyrup had shipped the
        // filter half only for `extensions`, so a settings-declared extension root was inert (CFG-004).
        disc.global_overrides = ResourceOverrides {
            skills: settings.global().skill_paths(),
            prompts: settings.global().prompt_template_paths(),
            themes: settings.global().theme_paths(),
            extensions: settings.global().extension_paths(),
        };
        disc.project_overrides = ResourceOverrides {
            skills: settings.project().skill_paths(),
            prompts: settings.project().prompt_template_paths(),
            themes: settings.project().theme_paths(),
            extensions: settings.project().extension_paths(),
        };
        // CFG-003: `settings.packages` is Pi's ONLY package channel — `PackageManager.resolve()`
        // re-collects `projectSettings.packages` then `globalSettings.packages` on every call and
        // resolves each entry to a working tree (package-manager.ts:891-901). cyrup read only its own
        // `packages.json` install registry, so a package DECLARED in settings contributed nothing.
        // Project entries are pushed first so they win the shared package precedence rank (:887-893).
        let (configured_packages, package_errors) = configured_packages_from_settings(&settings);
        disc.configured_packages = configured_packages;
        let report = discover(&disc, cancel.token()).await?;
        // TUI-006: the discovery pass's structured diagnostics (shadowed same-name skills, a
        // configured path that does not exist, a malformed frontmatter) used to be dropped on the
        // floor here. Pi shows them at startup even under `quietStartup`
        // (`showDiagnosticsWhenQuiet: true`, interactive-mode.ts:1769), so they now travel on
        // `AgentSessionServices::startup_diagnostics` for the front-end to render.
        let mut startup_diagnostics = crate::services::StartupDiagnostics {
            resources: report.diagnostics.clone(),
            // EXT-S01: the native built-ins that failed to load at step 4b, contained above.
            extensions: native_load_errors,
            ..Default::default()
        };
        // A malformed `packages` entry never takes the settings document (or the session) down; it
        // is reported alongside the discovery diagnostics.
        for message in package_errors {
            startup_diagnostics.resources.push(cyrup_resources::ResourceDiagnostic::error(
                cyrup_resources::ResourceKind::Package,
                cfg.agent_dir.join("settings.json"),
                message,
            ));
        }

        // CFG-002: `<agent_dir>/models.json` — the user's custom-provider / custom-model file. Pi
        // loads it ONCE per runtime (`ModelConfig.load(join(getAgentDir(),"models.json"))`,
        // model-runtime.ts:137-139) and composes it over the built-in provider catalogs
        // (`composeModelProvider`, provider-composer.ts:411-437). cyrup had the reader
        // (`load_models_file`) and the path (`ConfigDirs::models_path`) but NO production caller, so
        // the entire custom-provider surface was dead. A malformed file is reported and skipped —
        // never fatal, never a panic (Pi keeps an empty snapshot + one error string,
        // model-config.ts:248-271).
        let (model_file, model_file_error) =
            cyrup_config::load_models_file_reporting(&cfg.agent_dir.join("models.json"));
        startup_diagnostics.models.extend(model_file_error);
        // The persisted pi.dev catalog overlay (DRIFT-007), loaded from disk ONLY. This is the
        // cache-only restore Pi performs at `agent-session-services.ts:180`
        // (`refresh({ allowNetwork: false })`): a session build must never block on a network call,
        // and an offline run must still see the catalogs it saw last time. A refresh that ADDS to
        // this cache is the running mode's fire-and-forget job (Pi `main.ts:863-866`).
        let catalog_overlay = Self::load_persisted_catalog_overlay(&cfg.agent_dir).await;
        // Surface composition errors (a provider block Pi would `throw` on) once, at startup, rather
        // than on every catalog read.
        {
            let base = cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
                credentials: None,
                auth_context: None,
                catalog_overlay: catalog_overlay.clone(),
            })
            .get_models(None);
            let (_, errors) = model_file.compose(&base);
            startup_diagnostics.models.extend(errors);
        }
        let model_config = Arc::new(model_file);

        // Resolve the on-disk extension discovery roots from `--extension`/`--no-extensions` (Pi
        // `resourceLoaderOptions.additionalExtensionPaths`/`noExtensions`, main.ts:660,664), then
        // fold in the package-declared extension dirs discovery just collected (gap-07 #2: Pi merges
        // the package tier's `resolvedPaths.extensions` into the loaded set, resource-loader.ts:
        // 379,403-407 `mergePaths(cliEnabledExtensions, enabledExtensions)`). `configured` is the
        // pre-trust configured-extension tier — the same shape package extension dirs enter — so
        // appending them here makes an installed package's extension load alongside the
        // project/global/CLI roots. The live wasm *instantiation* of each discovered extension runs
        // only under the `wasm-host` feature (the Wasmtime engine + the `wasm32-wasip2` guest
        // toolchain — the gated arch-08b live-wasm tail, residual ledger §09 #13). Native built-ins
        // are already loaded above.
        let mut ext_roots = extension_discovery_roots(&cfg);
        ext_roots.configured.extend(report.registry.ext_crate_paths.iter().cloned());
        #[cfg(feature = "wasm-host")]
        {
            // Inject the session's OWN `host_services` (built at 4a) so a disk-discovered guest's
            // `control` capability reaches the same queue `apply_pending_control` drains.
            let host_services_for_load: Arc<dyn cyrup_ext::host::HostServices> = host_services.clone();
            // The per-path `errors` (Pi `LoadExtensionsResult.errors` → "Failed to load extension"
            // diagnostics, main.ts:679-682) are retained on `startup_diagnostics` so the TUI can
            // render Pi's `[Extension issues]` block (TUI-006) instead of dropping them here. Each
            // carries its `fatal` flag through unchanged, so a genuine load fault also reaches the
            // bin's exit-1 checkpoint while the project-trust skip does not (`LoadError::fatal`).
            let load_result =
                ext_host.discover_and_load(&ext_roots, trusted, host_services_for_load).await;
            startup_diagnostics.extensions.extend(load_result.errors.iter().map(|e| {
                crate::services::ExtensionLoadDiagnostic {
                    path: e.path.clone(),
                    error: e.error.clone(),
                    fatal: e.fatal,
                }
            }));
        }
        #[cfg(not(feature = "wasm-host"))]
        let _ = &ext_roots;

        // Apply the CLI-captured extension flag overrides now that every loaded extension's
        // `registerFlag` has run (Pi runs `applyExtensionFlagValues` inside
        // `createAgentSessionServices`, agent-session-services.ts:167 — AFTER the extensions load).
        // Without this step the 1:1-ported CLI capture (`cfg.extension_flag_values`, from the bin's
        // `partition_extension_flags` / Pi `unknownFlags`) is dropped one call short of the
        // guest-visible `getFlag` (gap-08 §5.6). The ext-host resolves each value against the
        // registered flag's declared type and stores it in the shared flag store `getFlag` consults.
        if !cfg.extension_flag_values.is_empty() {
            let overrides: Vec<(String, cyrup_ext::ExtensionFlagOverride)> = cfg
                .extension_flag_values
                .iter()
                .map(|(name, v)| {
                    let ov = match v {
                        ExtensionFlagValue::Bool(b) => cyrup_ext::ExtensionFlagOverride::Bool(*b),
                        ExtensionFlagValue::Str(s) => {
                            cyrup_ext::ExtensionFlagOverride::Str(s.clone())
                        }
                    };
                    (name.clone(), ov)
                })
                .collect();
            // SEAM-S01: the reconciliation diagnostics — `Unknown option(s): --foo` and
            // `Extension flag "--foo" requires a value` (Pi agent-session-services.ts:98-125) — are
            // retained here. They used to be `continue`d away inside the ext-host, so a mistyped
            // `--flag` produced no message and no non-zero exit. Pi merges them into
            // `services.diagnostics` (:182), which becomes `runtime.diagnostics` and is reported +
            // `process.exit(1)`-ed at main.ts:843-848.
            startup_diagnostics.flags.extend(ext_host.apply_extension_flag_values(&overrides)?);
        }

        // Bind the shared model-registry sink and FLUSH any provider registrations queued while native
        // + disk extensions loaded (Pi `runner.bindCore` pending-flush, runner.ts:345-362). The SAME
        // `Arc` is the `ext_host` sink (future `registerProvider`s upsert live) and the session's read
        // view (its catalog is UNIONed into the model registry, and its provider installed on select).
        let guest_providers = Arc::new(crate::guest_providers::GuestProviderRegistry::new());
        ext_host.registry().bind_model_registry(guest_providers.clone())?;
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
            // The aggregate now attributes each path to its extension (gap-08 #15); for registry
            // discovery we take the path strings in concatenated load order.
            let mut skill_paths: Vec<PathBuf> =
                agg.skill_paths.iter().map(|p| PathBuf::from(&p.path)).collect();
            let mut prompt_paths: Vec<PathBuf> =
                agg.prompt_paths.iter().map(|p| PathBuf::from(&p.path)).collect();
            let mut theme_paths: Vec<PathBuf> =
                agg.theme_paths.iter().map(|p| PathBuf::from(&p.path)).collect();
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
        let mut skills: Vec<SkillPointer> = if read_available && !cfg.no_skills {
            resources.skills.winners().map(|s| s.pointer()).collect()
        } else {
            Vec::new()
        };
        // Synthetic-skill injection (Pi `skillsOverride`, resource-loader.ts:630): transform the
        // discovered pointer set before it feeds the context snapshot + system prompt. Applied to the
        // (possibly-empty) base so an embedder can inject skills discovery found none of; the emit is
        // still `read`-gated downstream (skills_inject.rs), matching Pi.
        if let Some(f) = skills_override {
            skills = f(skills);
        }

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
        // Synthetic context-file injection (Pi `agentsFilesOverride`, resource-loader.ts:474):
        // transform the loaded `AGENTS.md`/`CLAUDE.md` set before the system prompt reads it.
        if let Some(f) = context_files_override {
            let snap = context_store.snapshot();
            let files = f(snap.context_files.to_vec());
            context_store.store(ContextSnapshot {
                context_files: Arc::from(files),
                skills: snap.skills.clone(),
                override_source: snap.override_source.clone(),
                diagnostics: snap.diagnostics.clone(),
            });
        }
        let snapshot = context_store.snapshot();

        let selected_tools: Vec<Arc<str>> =
            base_tools.iter().map(|t| Arc::from(t.name())).collect();
        let tool_contributions: Vec<ToolPromptContribution> =
            base_tools.iter().map(tool_contribution).collect();

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

        // The extension-shaped active tool set (Pi `pi.getActiveTools()` after extension `active_tools`
        // merge): base build-time selection PLUS any extension additions/overrides (e.g. a native
        // extension that overrides a built-in `bash`). Computed here (moved ahead of the registry) so
        // the dynamic registry below can include these overrides — see the extend below.
        let active_tools = ext_host.active_tools(&base_tools)?;

        // The dynamic-tool registry (Pi `_toolRegistry`): every Availability-visible tool, the caller's
        // custom tools, AND the extension-contributed/override tools are enable-able; the active set
        // starts at the build-time selection. Including the extension tools is load-bearing: (a) the
        // permission companion's registry / unknown-tool gate checks `all_tool_names` against this
        // registry (an extension tool absent here would be falsely blocked as "unknown"), and (b) a
        // `setActiveTools` rebuild (`DynamicToolState::set_active`) looks tools up BY NAME in this
        // registry — an extension override (recording/test double or a real replacement of a built-in)
        // must survive the rebuild rather than being replaced by the shadowed built-in. Extended LAST
        // so an override wins the `BTreeMap`-by-name dedup in `DynamicToolState::new`.
        let mut registry_tools = visible.clone();
        // The SDK-supplied custom tools go through the same registered-tool wrapper (Pi folds them
        // into `_baseToolDefinitions` and wraps that whole map, agent-session.ts:2507-2515), so a
        // custom tool that widens the active set also derives `addedToolNames`. `active_tools`
        // above already returned WRAPPED handles for the built-ins + extension tools.
        registry_tools.extend(cfg.custom_tools.iter().map(|t| ext_host.wrap_tool(t.clone())));
        registry_tools.extend(active_tools.iter().cloned());
        let contributions: std::collections::BTreeMap<String, ToolPromptContribution> = registry_tools
            .iter()
            .map(|t| (t.name().to_string(), tool_contribution(t)))
            .collect();
        // The rebuilder base = the prompt inputs with the per-run tool fields cleared (re-derived
        // from the active set on each `setActiveToolsByName`).
        let mut rebuild_base = prompt_inputs.clone();
        rebuild_base.selected_tools = Vec::new();
        rebuild_base.tool_contributions = Vec::new();
        // Shared with `host_services` so a loaded guest's `setActiveTools`/`getActiveTools`
        // capability read+mutates the SAME authoritative active-tool view the host/CLI toggle uses
        // (Pi binds both to `agent.state.tools`, agent-session.ts:2281,2283).
        let dynamic_tools = Arc::new(std::sync::Mutex::new(crate::tools::DynamicToolState::new(
            registry_tools,
            base_tools.clone(),
            crate::tools::PromptRebuilder::new(rebuild_base, contributions),
        )));
        host_services.attach_dynamic_tools(dynamic_tools.clone());
        // EXT-005: seed the guest-visible `ctx.getSystemPrompt()` / `ctx.isProjectTrusted()` reads
        // from the values this build resolved (Pi binds both straight to the session:
        // `getSystemPrompt: () => this.systemPrompt` and `isProjectTrusted: () =>
        // this.settingsManager.isProjectTrusted()`, agent-session.ts:2410,2434). Without this a
        // guest got the trait defaults — an empty prompt and a confident, wrong `false` for trust,
        // even in a project cyrup had just decided IS trusted.
        host_services.update_prompt_state(Some(system_prompt.clone()), settings.project_trusted());

        // ---- 7. seed the agent transcript from the resumed branch (R-04-011). The manager was
        // created at step 2b; `existing` already holds its context.
        let seed: Vec<cyrup_agent::AgentMessage> =
            existing.messages.iter().map(core_message_to_agent).collect();

        // ---- 8. extension host seams (cyrup-ext) — the host itself was built at step 4b ---------
        // (`active_tools` was computed above, ahead of the dynamic-tool registry.)
        let session_cancel = CancelToken::new();
        let ext_subscriber = ext_host.subscriber(session_cancel.clone());
        let ext_hooks = ext_host.hooks();

        // ---- 9. agent loop: provider + tools + composed hooks + both seams --------------------
        // `blockImages` defense-in-depth (Pi sdk.ts:254-289): the convert-to-llm seam strips image
        // content when the setting is on, deduping consecutive placeholders. Folded into PolicyHooks
        // so it rides the agent's single `convertToLlm` slot.
        let block_images = settings.effective().block_images();
        // The shared self-handle: bound to the owning `Arc<AgentSession>` by `into_shared`, and read
        // by the persist+fan-out subscriber (`_handleAgentEvent`), the post-run driver, and — since
        // the turn-boundary tool refresh — `PolicyHooks::prepare_next_turn`. Declared HERE, ahead of
        // the hooks, because the hooks are built before the session and must capture it; it is an
        // empty `OnceLock` either way, so nothing observable moved with it.
        let handle = Arc::new(crate::session::SessionHandle::default());
        let policy_hooks = Arc::new(crate::hooks::PolicyHooks::new(
            cfg.permission_policy.clone(),
            ext_hooks,
            has_ui,
            block_images,
            handle.clone(),
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
        // The swappable stream source the agent loop streams through: it wraps the resolved provider
        // and the (optional) resolver seam so a cross-provider `/model` select can install a new
        // provider in place without rebuilding the agent (Pi live model+provider switch). The SAME
        // `Arc` is handed to the agent (as its `StreamFn`) and to the session (to mutate on select).
        let provider_swap =
            Arc::new(ProviderSwap::new(self.provider.clone(), self.provider_resolver.clone()));
        // Transport selection (Pi `AgentOptions.streamFn`, sdk.ts:301): an embedder-supplied custom
        // `StreamFn` (e.g. `ProxyStreamFn`) becomes THE transport the agent loop streams through;
        // absent one, the provider-backed `ProviderSwap` is used (the default live-swappable path).
        let agent_stream_fn: Arc<dyn cyrup_agent::StreamFn> = match custom_stream_fn {
            Some(f) => f,
            None => provider_swap.clone(),
        };
        let mut agent_builder = Agent::builder(model_ref.clone(), agent_stream_fn)
        .system_prompt(system_prompt.clone())
        .thinking_level(thinking)
        .tools(active_tools)
        .messages(seed)
        .hooks(policy_hooks)
        .session_id(session_id.clone())
        // Settings→Agent wiring (Pi sdk.ts:356-360): queue modes + transport + custom thinking budgets.
        .steering_mode(parse_queue_mode(&eff.steering_mode()))
        .follow_up_mode(parse_queue_mode(&eff.follow_up_mode()))
        // `transport` (Pi sdk.ts:357 `transport: settingsManager.getTransport()`). The setting was
        // parsed, migrated from the legacy `websockets` boolean and offered in the `/settings` grid,
        // but never reached the agent — so `AgentBuilder::transport` had no non-test caller and the
        // value died in the config layer. It now rides `StreamOptions.transport` into every
        // `StreamFn::stream` call (agent.rs `gen_config.transport`), which is the seam an
        // embedder-supplied `StreamFn` (e.g. `ProxyStreamFn`) and every wire API read from.
        .transport(parse_transport(&eff.transport()));
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
        // PROV-006. Pi's `configureHttpDispatcher(getHttpIdleTimeoutMs())` installs a PROCESS-GLOBAL
        // dispatcher (main.ts:802, interactive-mode.ts:1778) that bounds every outbound HTTP request
        // — provider streams, catalog refreshes, everything — so the equivalent global is installed
        // here, not just threaded onto this agent's requests.
        //
        // `0` is passed through rather than skipped: `httpIdleTimeoutMs: 0` / `"disabled"` means the
        // user turned the timeout OFF, and dropping the call would silently leave the previous value
        // (or the 5-minute default) in place. The old `timeout_ms > 0` guard did exactly that.
        if let Ok(timeout_ms) = eff.http_idle_timeout_ms() {
            cyrup_provider::configure_http_idle_timeout(timeout_ms);
            agent_builder = agent_builder.timeout_ms(timeout_ms);
        }

        // `settings.retry.provider.*` — Pi's `getProviderRetrySettings()`, applied in `sdk.ts`'s
        // `streamFn` as `options?.X ?? providerRetrySettings.X` (sdk.ts:303-317). `timeoutMs` wins
        // over `httpIdleTimeoutMs` when set, which is why it is applied after the block above.
        // Negative values (JSON has no unsigned type) are treated as unset rather than clamped to 0,
        // since `0` is a meaningful "disabled" for the timeout and "no retries" for the budget.
        {
            let retry = eff.provider_retry_settings();
            if let Some(timeout_ms) = retry.timeout_ms.filter(|ms| *ms >= 0) {
                agent_builder = agent_builder.timeout_ms(timeout_ms as u64);
            }
            if let Some(max_retries) = retry.max_retries.filter(|n| *n >= 0) {
                agent_builder = agent_builder.max_retries(max_retries as u32);
            }
            if retry.max_retry_delay_ms >= 0 {
                agent_builder = agent_builder.max_retry_delay_ms(retry.max_retry_delay_ms as u64);
            }
        }

        // gap-08 #2/#3: install the provider transport extension seams. `on_payload` routes the
        // outbound body through the tested `before_provider_request` [mutate] facade (Pi
        // `emitBeforeProviderRequest` in sdk.ts onPayload, :332-338); `on_response` constructs the
        // previously-NOWHERE `HostEvent::AfterProviderResponse` notify ({status, headers}, Pi
        // sdk.ts:339-348). Both are gated on a live subscriber so the common no-extension path pays
        // nothing. The dispatch is async (wasm) — hence the async hook signatures (no block_on).
        {
            let h = ext_host.clone();
            agent_builder = agent_builder.on_payload(Arc::new(move |payload, _model| {
                let h = h.clone();
                Box::pin(async move {
                    if h.dispatcher().no_subscribers(EventKind::BeforeProviderRequest) {
                        return None;
                    }
                    let out =
                        h.emit_before_provider_request(payload.clone(), &CancelToken::new()).await;
                    (out != payload).then_some(out)
                })
            }));
            let h = ext_host.clone();
            agent_builder = agent_builder.on_response(Arc::new(move |resp, _model| {
                let h = h.clone();
                Box::pin(async move {
                    if h.dispatcher().no_subscribers(EventKind::AfterProviderResponse) {
                        return;
                    }
                    let headers = serde_json::to_value(&resp.headers).unwrap_or_default();
                    h.dispatcher()
                        .dispatch_notify(
                            &HostEvent::AfterProviderResponse {
                                status: u32::from(resp.status),
                                headers,
                            },
                            &CancelToken::new(),
                        )
                        .await;
                })
            }));
        }

        // Dynamic per-request key resolution (Pi key resolver): consulted on every turn, overriding
        // any static key. Threaded whether or not a custom transport is installed.
        if let Some(kr) = custom_key_resolver {
            agent_builder = agent_builder.key_resolver(kr);
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

        // The directory THIS session's files live in — Pi's `SessionManager.sessionDir`, exposed as
        // `getSessionDir()` (session-manager.ts:999-1001) and fixed once at construction. Pi resolves
        // it as `sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)` when a session is
        // created (`create`, :1519-1520) and as `sessionDir ?? resolve(path, "..")` — the OPEN FILE's
        // own parent — when one is resumed (`open`, :1547-1548). The interactive `/resume` picker
        // lists exactly this directory (`SessionManager.list(getCwd(), getSessionDir())`,
        // interactive-mode.ts:4867), so it is carried on the services instead of being re-derived
        // from the cwd-encoded default, which is wrong under `--session-dir` and after a resume from
        // elsewhere. An in-memory session has no file, so the resolved layout dir stands in.
        let session_dir = match &cfg.session_dir {
            Some(dir) => dir.clone(),
            None => manager
                .session_file()
                .and_then(std::path::Path::parent)
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| layout.dir()),
        };

        let manager = Arc::new(AsyncMutex::new(manager));
        // Attach the live tree manager to the (already control-wired) host-services backend so a
        // loaded guest's `append_entry`/`set_session_name`/`set_label` capability mutates THIS
        // session's real tree (arch-08 §5.6; Pi appends synchronously, agent-session.ts:2265-2279).
        host_services.attach_session(manager.clone());
        let fanout = Arc::new(Fanout::new());

        // Attach the extension notify seam, then the facade's persist+fan-out subscriber.
        agent.subscribe(ext_subscriber);
        agent.subscribe(Arc::new(SvcSubscriber::new(
            fanout.clone(),
            manager.clone(),
            handle.clone(),
            ext_host.clone(),
            session_cancel.clone(),
        )));
        let agent = Arc::new(agent);

        // ---- 10. assemble the session --------------------------------------------------------
        // `host_services` (the concrete arch-08 §5.6 backend) was built + seeded + control-wired at
        // step 4a and injected into every wasm load; it is moved into the services bundle below so
        // `AgentSession::apply_pending_control` drains the SAME queue guest `control` ops reach (Pi
        // `createCommandContext`, agent-session.ts:1158).

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
            shell_path: shell_path_setting,
            shell_command_prefix: shell_command_prefix_setting,
            dynamic_tools,
            handle,
            bash_session_env,
            read_model_vision,
        };

        let services = AgentSessionServices {
            cwd,
            agent_dir: cfg.agent_dir.clone(),
            session_dir,
            home: cfg.home.clone(),
            settings,
            project_trusted: trusted,
            auth,
            resources,
            startup_diagnostics,
            model_config,
            catalog_overlay,
            context: context_store,
            ext_host,
            guest_providers,
            model: resolved_model,
            system_prompt,
            host_services,
            extension_flag_values: cfg.extension_flag_values.clone(),
        };

        Ok(AgentSession::from_parts(
            agent,
            manager,
            fanout,
            provider_swap,
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

/// Parse the settings `transport` string into the provider [`Transport`] Pi hands the agent
/// (`sdk.ts:357` `transport: settingsManager.getTransport()`; the `TransportSetting` union is
/// `"auto" | "sse" | "websocket" | "websocket-cached"`, types.ts:98). The strings are byte-1:1 with
/// Pi because `Transport` is `#[serde(rename_all = "kebab-case")]`. An unrecognized value falls back
/// to `auto`, matching `getTransport()`'s `?? "auto"` and the settings dialog's fixed choice set.
pub(crate) fn parse_transport(s: &str) -> cyrup_provider::Transport {
    match s {
        "sse" => cyrup_provider::Transport::Sse,
        "websocket" => cyrup_provider::Transport::Websocket,
        "websocket-cached" => cyrup_provider::Transport::WebsocketCached,
        _ => cyrup_provider::Transport::Auto,
    }
}

/// Serialize a [`ModelThinkingLevel`] to its persisted snake/camel key (`off`/`minimal`/…/`xhigh`/`max`).
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
                    None => match fallback_model(provider, cfg, pat) {
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
/// provider's *curated* default (Pi `defaultModelPerProvider`, else its first model) and override
/// `id`/`name` with the requested model id, so an unknown-but-intended model id proceeds as a custom
/// model. The provider is "known" when `--provider` was explicit (`cli_provider_explicit`) or the
/// pattern carries a `provider/` prefix naming the resolved provider. Returns `(model,
/// thinking_level)` or `None` (⇒ the caller keeps Pi's hard `ModelNotFound`). A trailing `:level` is
/// honored only when `--thinking` was not given (Pi `fallbackThinking`, model-resolver.ts:481-490).
fn fallback_model(
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
    // Clone the provider's *curated* default (Pi `defaultModelPerProvider` — e.g. anthropic ->
    // `claude-opus-4-8`), else its first model, overriding id + name (Pi `buildFallbackModel`,
    // model-resolver.ts:163-177). `cyrup_config::build_fallback_model` (model.rs:1033) is the shared
    // helper that mirrors that curated pick exactly. NOTE: `ModelResolver::provider_default` is the
    // WRONG base here — it is alias-preferred + raw-byte-descending (anthropic -> `claude-sonnet-5`),
    // which diverges the cloned model's cost (~2.5x) and compat flags from Pi.
    let model = cyrup_config::build_fallback_model(provider_id.as_str(), base_id, provider.models())?;
    Some((model, level))
}

/// Project a tool's OWN prompt contribution off its `Tool` vtable (arch-06 R-06-012/013). Pi reads
/// `definition.promptSnippet`/`definition.promptGuidelines` straight off the tool definition
/// (agent-session.ts:2490-2504) — never a name-keyed table — so a tool that declares no snippet is
/// simply absent from the "Available tools" section (system-prompt.ts:79-80: `tools.filter(name =>
/// !!toolSnippets?.[name])`), and one that declares guidelines contributes them as bullets.
pub(crate) fn tool_contribution(tool: &Arc<dyn cyrup_core::Tool>) -> ToolPromptContribution {
    ToolPromptContribution {
        tool: Arc::<str>::from(tool.name()),
        snippet: tool.prompt_snippet().map(Arc::<str>::from),
        guidelines: tool.prompt_guidelines().iter().copied().map(Arc::<str>::from).collect(),
    }
}

/// Consult the extensions' `project_trust` verdict BEFORE the trust decision is frozen (EXT-003).
///
/// Pi does this with a deliberate throwaway load: `resource-loader.ts:378-399` calls
/// `loadProjectTrustExtensions()` (which forces `setProjectTrusted(false)` and loads only the global
/// plus CLI-configured tier), awaits `options.resolveProjectTrust({extensionsResult})`, drops that
/// set via `clearExtensionCache()`, then loads everything again against the real verdict. The
/// callback is wired at `main.ts:691-712` → `resolveProjectTrusted` (`core/project-trust.ts:46-95`),
/// which slots the extension verdict between the `--approve` override and the saved decision.
///
/// cyrup had `ExtensionHost::aggregate_project_trust`, the `project_trust` event kind, the WIT
/// `on-project-trust` export AND `cyrup_config::decide_trust_with_extension` — all of them, with
/// zero production callers, because trust was decided in builder step 1 and the `ExtensionHost` was
/// not constructed until step 4b. This is the missing call.
///
/// The pass is a THROWAWAY host: passing `project_trusted = false` is what restricts the loaded set
/// (`DiscoveredExtension::is_trusted` = `origin.is_pre_trust() || project_trusted`, loader.rs:57-60),
/// so a project-local extension cannot vote itself trusted. Natives are loaded WITHOUT the live
/// `HostServices` backend — it does not exist this early, and Pi's `projectTrustContext` likewise
/// carries only ui + cwd.
///
/// NATIVES ARE OPT-IN. Pi's module cache holds FACTORIES, not instances (`loader.ts:148,414-437`),
/// so its second pass calls the factory again against a fresh `Extension` + `ExtensionAPI`. A cyrup
/// native has no such re-instantiation: it is a process-lifetime `Arc<dyn NativeExtension>`, so
/// loading it here would call `init` TWICE ON THE SAME OBJECT. That is not theoretical —
/// `cyrup-ext-subagents`' `ChildSafe` arm spawns a detached nested-control-inbox poller from `init`,
/// and a second one would race the first over the same inbox — and the trigger is the common case
/// (any repo with a `.cyrup/` directory; a subagent child re-execs with no `--approve`). So only
/// natives that answer `NativeExtension::decides_project_trust` — whose contract is "my `init` is
/// idempotent" — take part. WASM guests always do: a guest load builds a fresh instance in a fresh
/// store, which IS Pi's fresh-per-factory-call semantics.
async fn pre_trust_extension_verdict(
    cfg: &SessionConfig,
    cwd: &Path,
    natives: &[Arc<dyn NativeExtension>],
) -> Option<cyrup_ext::ProjectTrustDecision> {
    let (mode, has_ui) = ext_mode(cfg.app_mode);
    let host_config = HostConfig { mode, has_ui, cwd: cwd.to_path_buf() };
    #[cfg(feature = "wasm-host")]
    let host = ExtensionHost::with_wasm(host_config).ok()?;
    #[cfg(not(feature = "wasm-host"))]
    let host = ExtensionHost::new(host_config);

    for ext in natives.iter().filter(|e| e.decides_project_trust()) {
        // A load failure in the throwaway pass must not fail the build — the real load at step 4b
        // surfaces it. Skip and keep polling the rest.
        if let Err(e) = host.load_native(ext.clone()).await {
            tracing::debug!(error = %e, "pre-trust extension load skipped");
        }
    }
    #[cfg(feature = "wasm-host")]
    {
        let roots = extension_discovery_roots(cfg);
        let deny: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(cyrup_ext::DenyServices);
        // `false` = pre-trust tier only (global + CLI-configured), exactly Pi's
        // `loadProjectTrustExtensions()`.
        let _ = host.discover_and_load(&roots, false, deny).await;
    }

    let decision = host.aggregate_project_trust(&CancelToken::new()).await;
    // The host (and every instance it loaded) is dropped here — Pi's `clearExtensionCache()`.
    decision
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

/// Load the on-disk installed-package registries the `install` subcommand writes — Global under
/// `<package_dir>/packages.json`, Project under `<cwd>/.cyrup/packages.json` (the exact paths
/// [`PackageStore::registry_path`] resolves for `PackageStore::new(package_dir, Some(cwd))`, the SAME
/// construction the bin's `install` uses at subcommands.rs:396) — and concatenate them in the fixed
/// project-then-global order discovery re-sorts into anyway (discovery.rs:435-439). This is the READ
/// half of C1 (gap-07 #1 / gap-13 C1): the write half already works (the bin persists correctly);
/// this threads the persisted registry into a live session, the missing wiring that made
/// `cyrup install` a runtime no-op for skill/prompt/theme/extension resources.
///
/// A missing registry file is an empty registry (the common "nothing installed" case) and a
/// malformed one is treated as "no packages from that scope" rather than aborting the whole session
/// build — mirroring the working `cyrup-ext-subagents::enumerate_installed_packages` precedent
/// (extension.rs:1269-1289) and `lock::load`'s own missing-file contract.
fn load_installed_packages(package_dir: &Path, cwd: &Path) -> InstalledPackages {
    let store = PackageStore::new(package_dir.to_path_buf(), Some(cwd.to_path_buf()));
    let mut installed = InstalledPackages::default();
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

/// Collect the packages DECLARED in settings into discovery's settings-package channel (CFG-003).
///
/// 1:1 with the head of Pi's `PackageManager.resolve()` (package-manager.ts:891-900): PROJECT
/// entries first, then GLOBAL, deduped by source identity so a project entry wins a collision — the
/// exact ordering that makes project-scope resources beat global ones under the shared package
/// precedence rank. Each entry's object-form include filters ride along
/// (`const filter = typeof pkg === "object" ? pkg : undefined`, :1231).
///
/// Reads the two RAW LAYERS, never the merged effective view: the merged view cannot say which
/// scope declared an entry, and discovery trust-gates project-scope packages.
///
/// A malformed entry is skipped with a message rather than dropping the array (or the settings
/// document) — the returned `Vec<String>` becomes startup diagnostics.
fn configured_packages_from_settings(
    settings: &SettingsManager,
) -> (Vec<ConfiguredPackage>, Vec<String>) {
    let mut out: Vec<ConfiguredPackage> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (layer, scope) in [
        (settings.project(), InstallScope::Project),
        (settings.global(), InstallScope::Global),
    ] {
        let (declared, layer_errors) = layer.packages_with_errors();
        errors.extend(layer_errors);
        for entry in declared {
            let source = entry.source().trim().to_string();
            if source.is_empty() {
                errors.push("settings `packages` entry has an empty `source`".to_string());
                continue;
            }
            let (extensions, skills, prompts, themes) = entry.filters();
            let built = ConfiguredPackage {
                source,
                scope,
                filter: PackageFilter {
                    // `autoload: false` flips the per-type lists from include filters to a delta
                    // (Pi `collectPackageResources`, package-manager.ts:2084-2085).
                    autoload: entry.autoload(),
                    extensions: extensions.map(<[String]>::to_vec),
                    skills: skills.map(<[String]>::to_vec),
                    prompts: prompts.map(<[String]>::to_vec),
                    themes: themes.map(<[String]>::to_vec),
                },
            };
            // Pi's `dedupePackages` (package-manager.ts:1681-1703), all three branches:
            //
            // - first sighting of an identity — keep it;
            // - the kept entry is PROJECT and this one is USER — normally drop this one, EXCEPT
            //   when the project entry is `autoload: false`, which its doc comment (:1676-1679)
            //   defines as "a delta over the global entry, so both are kept (delta first)". The
            //   base entry has to survive or the delta has nothing to layer over and the project
            //   patterns silently become the whole package;
            // - otherwise, a PROJECT entry replaces whatever is in the slot (`result[index] =
            //   entry`, :1698) — project wins, later project entry wins an intra-scope repeat.
            //
            // [CYRUP-DELTA] the identity is the trimmed source STRING, where Pi normalizes through
            // `getPackageIdentity` (:1660-1674) so `npm:x@1` and `npm:x@2`, or an SSH and an HTTPS
            // URL for one repo, collide. Tracked separately as CFG-026.
            match out.iter().position(|p| p.source == built.source) {
                None => out.push(built),
                Some(index) => {
                    let existing_is_project_delta = out
                        .get(index)
                        .is_some_and(|p| p.scope == InstallScope::Project && p.filter.is_delta());
                    if existing_is_project_delta && built.scope == InstallScope::Global {
                        out.push(built);
                    } else if built.scope == InstallScope::Project
                        && let Some(slot) = out.get_mut(index)
                    {
                        *slot = built;
                    }
                }
            }
        }
    }
    (out, errors)
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

    /// CFG-010 (dedupe half) — Pi's `dedupePackages` keeps BOTH entries, delta first, when a
    /// PROJECT entry carrying `autoload: false` collides with a USER one for the same package
    /// identity: "A project entry with autoload=false is a delta over the global entry, so both
    /// are kept (delta first)" (package-manager.ts:1676-1679, code at :1691-1696). Dropping the
    /// global entry turns the delta form inside out — the project entry's patterns become the
    /// ONLY thing that loads instead of a layer over the full package.
    #[test]
    fn a_project_autoload_false_entry_is_a_delta_over_the_global_entry_not_a_replacement() {
        use cyrup_config::{InMemorySettingsStore, Settings, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{"packages":[{"source":"npm:pi-tools","autoload":false,"extensions":["-extensions/foo.ts"]}]}"#,
        );
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools"]}"#);
        let mgr = SettingsManager::load(store, Settings::new(), true);

        let (pkgs, errors) = super::configured_packages_from_settings(&mgr);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            pkgs.len(),
            2,
            "the global entry must survive so the project delta has something to layer over, got \
             {pkgs:?}"
        );
        assert_eq!(pkgs[0].scope, InstallScope::Project, "delta first");
        assert!(pkgs[0].filter.is_delta());
        assert_eq!(pkgs[1].scope, InstallScope::Global);
        assert!(pkgs[1].filter.is_empty(), "the base entry keeps no filter");
    }

    /// The other side of the same branch: without `autoload: false` a project entry still REPLACES
    /// the global one outright (`else if (entry.scope === "project")` / the plain drop of a later
    /// user entry, package-manager.ts:1694-1698).
    #[test]
    fn a_plain_project_entry_still_shadows_the_global_one() {
        use cyrup_config::{InMemorySettingsStore, Settings, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{"packages":[{"source":"npm:pi-tools","skills":["skills/a"]}]}"#,
        );
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools"]}"#);
        let mgr = SettingsManager::load(store, Settings::new(), true);

        let (pkgs, _) = super::configured_packages_from_settings(&mgr);
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
    }

    /// PROV-002: the persisted session key for the `max` rung. Both directions go through serde,
    /// so this pins that the enum change actually reaches session replay + the `model:max`
    /// fallback suffix path (`fallback_model` calls `thinking_level_from_str`).
    #[test]
    fn thinking_level_max_round_trips_through_the_persisted_key() {
        use super::{thinking_level_from_str, thinking_level_to_str};
        use cyrup_core::ModelThinkingLevel;

        assert_eq!(thinking_level_to_str(ModelThinkingLevel::Max), "max");
        assert_eq!(thinking_level_from_str("max"), Some(ModelThinkingLevel::Max));
        for level in [
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Xhigh,
            ModelThinkingLevel::Max,
        ] {
            assert_eq!(
                thinking_level_from_str(&thinking_level_to_str(level)),
                Some(level),
                "{level:?} must survive a persist/restore round-trip"
            );
        }
        assert_eq!(thinking_level_from_str("ultra"), None);
    }

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

    // Pi `buildFallbackModel` (model-resolver.ts:163-177): a `--model <custom-id>` on a *known*
    // provider clones that provider's **curated** default (`defaultModelPerProvider` — anthropic ->
    // `claude-opus-4-8`), then overrides id/name. The buggy path cloned the alias-preferred,
    // raw-byte-descending pick (`resolver.provider_default` -> `claude-sonnet-5`), diverging cost
    // (~2.5x) and dropping the base's compat flags. This drives the real `fallback_model` site over
    // an assembled two-model anthropic catalog (opus cost 15/75 vs sonnet 6/30).
    #[test]
    fn fallback_model_clones_curated_default_not_alias_preferred_base() {
        use super::{fallback_model, SessionConfig};
        use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
        use cyrup_provider::ModelCost;

        let mk = |id: &str, input: f64, output: f64| {
            let mut d = FauxModelDefinition::new(id);
            d.cost = ModelCost { input, output, cache_read: 0.0, cache_write: 0.0, tiers: None };
            d
        };
        // Order the alias-preferred pick FIRST (byte-descending `s` > `o` -> sonnet), so the naive
        // `providerModels[0]` fallback is ALSO sonnet — only the curated-default lookup rescues opus.
        let provider = FauxProvider::with_config(FauxConfig {
            provider: "anthropic".into(),
            api: "anthropic".into(),
            models: vec![mk("claude-sonnet-5", 6.0, 30.0), mk("claude-opus-4-8", 15.0, 75.0)],
            ..Default::default()
        });
        let mut cfg = SessionConfig::new("/tmp", "/tmp/agent");
        cfg.cli_provider_explicit = true; // provider is "known" -> custom fallback is allowed

        let (model, _lvl) = fallback_model(&provider, &cfg, "my-custom-model")
            .expect("known provider yields a custom fallback model");

        // The requested custom id/name is applied on top of the base.
        assert_eq!(model.id.as_str(), "my-custom-model");
        assert_eq!(model.name, "my-custom-model");
        // The BASE must be the curated default (claude-opus-4-8), so cost matches opus, NOT the
        // alias-preferred claude-sonnet-5. On the buggy code this reads 6.0/30.0 and FAILS.
        assert_eq!(
            model.cost.input, 15.0,
            "fallback must clone curated default claude-opus-4-8, not alias-preferred claude-sonnet-5"
        );
        assert_eq!(model.cost.output, 75.0);
    }
}
