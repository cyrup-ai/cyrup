//! `SessionConfig` + `SessionBuilder` — assemble a [`crate::AgentSession`] from the real
//! subsystems (arch-11 §3.3). `build()` resolves settings + trust + auth + model (cyrup-config),
//! discovers resources (cyrup-resources), builds the tool registry with isolation decorators +
//! permission policy (cyrup-tools), opens/creates the session tree and wires compaction
//! (cyrup-session arch-04/05), assembles the system prompt + context store (arch-06), builds the
//! extension host with native built-ins and attaches BOTH ext seams to the agent (cyrup-ext), and
//! resolves the provider into the agent loop (cyrup-provider).
//!
//! This module is the SHAPE of a run — the config, the target, the builder and its setters. The
//! run itself lives next door: [`build`] holds the ordered walk plus the final assembly, and each
//! numbered step it walks is a function in [`steps`].

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::ModelThinkingLevel;
use cyrup_config::{AppMode, AuthStore, InMemorySettingsStore, Settings, SettingsStore};
use cyrup_config::trust::{TrustEntry, TrustOption, TrustStore};
use cyrup_ext::NativeExtension;
use cyrup_provider::Provider;
use cyrup_resources::SkillPointer;
use cyrup_session::manager::SessionManager;
use cyrup_session::prompt::ContextFile;
use cyrup_tools::{Availability, PermissionPolicy};

use crate::provider_swap::ProviderResolver;

mod build;
mod model;
mod natives;
mod packages;
mod settings_parse;
mod steps;
mod tools;

// The crate-internal helper surface `crate::builder::<name>` call sites reach by path
// (`session/thinking.rs`, `session/mod.rs`, `session/control.rs`, `tools.rs`, `host_services.rs`
// and `src/tests/`), kept here so splitting the file into `builder/` moved no call site.
pub(crate) use model::{thinking_level_from_str, thinking_level_to_str};
pub(crate) use natives::extension_discovery_roots;
pub(crate) use settings_parse::{parse_queue_mode, parse_transport};
pub(crate) use tools::tool_contribution;

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
    /// Whether this build may CLONE a settings-declared git package whose working tree is missing
    /// (CFG-003), i.e. `cyrup_resources::DiscoveryConfig::install_missing_packages` — see that
    /// field for the upstream mapping (`true` = pi's `resolve()` from the resource loader,
    /// resource-loader.ts:403 @v0.83.0; `false` = pi's `resolve(async () => "skip")`,
    /// cli/startup-ui.ts:73).
    ///
    /// Defaults to `false` so an SDK embedder's `build()` performs no network I/O it did not ask
    /// for. The bin sets it to `!(--offline || CYRUP_OFFLINE || PI_OFFLINE)` (`main.rs`), which is
    /// pi's own gate — `isOfflineModeEnabled()`, package-manager.ts:42-46, consulted at `:1261`.
    pub install_missing_packages: bool,
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
    /// `--no-extensions` / `-ne`: reduce the extension set to the explicitly-named `--extension`
    /// paths (Pi `resourceLoaderOptions.noExtensions`, main.ts:664 →
    /// `const extensionPaths = this.noExtensions ? cliEnabledExtensions : this.mergePaths(...)`,
    /// `resource-loader.ts:451-452` and `:555-557` @v0.83.0).
    ///
    /// SEAM-071: "the extension set" means pi's PATH tier in ALL of its forms, not just the
    /// disk-discovery roots. It drops the project + global roots, the package tier
    /// (`ext_crate_paths`, which is pi's `enabledExtensions`), and the AMBIENT native built-ins —
    /// cyrup's `cyrup-permission-system` / `cyrup-intercom` / `subagents` stand in for upstream's
    /// `@gotgenes/pi-permission-system`, pi-intercom and pi-subagents, which are ordinary installed
    /// packages in exactly the tier `noExtensions` removes.
    ///
    /// It does NOT drop pi's INLINE tier — `extensionFactories`, loaded unconditionally
    /// (`resource-loader.ts:579-581` over `main.ts:523`) — which is what a native handed to
    /// [`SessionBuilder::with_native_extension`] by an embedder is. See
    /// `natives::native_survives_no_extensions` for the discriminator and for the one carve-out pi
    /// makes in a subagent child.
    pub no_extensions: bool,
    /// Explicit `--extension <path>` resources to load as pre-trust *configured* extensions (Pi
    /// `resourceLoaderOptions.additionalExtensionPaths`, main.ts:660). Each may be a single extension
    /// dir or a directory of extensions. Threaded into the crate-internal
    /// `extension_discovery_roots` regardless of `no_extensions`.
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
    /// [CYRUP-DELTA] Wrap the **fs** backend in [`ProtectedFs`], refusing `write`/`edit` to
    /// `.env`, `.git/` and `node_modules/` (R-12-006). **Off by default** and embedder-only: there
    /// is deliberately no CLI flag and no `settings.json` key, because pi has no protected-path
    /// concept at all — `pi/packages/coding-agent/src/core/tools/write.ts:195-225` @v0.83.0
    /// resolves the path and calls `ops.writeFile` with no path predicate (ADR-0003 D5/D6).
    ///
    /// Scope is the **fs seam only**: the process seam is passed through undecorated (see the
    /// `Backend { fs, proc: base.proc.clone() }` construction below), so `bash 'echo x >> .env'`
    /// is NOT covered by this flag even when it is on. That is intentional — deciding from command
    /// text alone whether an arbitrary shell command mutates a protected path has no correct
    /// solution — and it is why the default is `false`.
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
            install_missing_packages: false,
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
            // ADR-0003 D5: pi has no protected-path concept (`write.ts:195-225` @v0.83.0 writes
            // whatever path it is given), and `bash` bypassed the guard anyway, so on-by-default
            // bought nothing and cost a failed turn. Inert embedder-only opt-in now.
            protect_paths: false,
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

/// A synthetic-skill override closure (Pi `DefaultResourceLoader.skillsOverride`): transforms the
/// discovered [`SkillPointer`] set before it feeds the system prompt.
type SkillsOverrideFn = Box<dyn FnOnce(Vec<SkillPointer>) -> Vec<SkillPointer> + Send>;
/// A synthetic context-file override closure (Pi `DefaultResourceLoader.agentsFilesOverride`).
type ContextFilesOverrideFn = Box<dyn FnOnce(Vec<ContextFile>) -> Vec<ContextFile> + Send>;

/// Assembles a [`crate::AgentSession`] from a [`SessionConfig`] + injected provider/services (arch-11).
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
    /// The project-trust store (`<agent_dir>/trust.json`). Pi's `resolveProjectTrusted` reads it as
    /// tier 4 — AFTER the extension `project_trust` verdict (`project-trust.ts:72-75` vs `:54-70`) —
    /// and writes to it both when an extension answers with `remember: true` (`:64-66`) and when the
    /// prompt's chosen option carries updates (`:40-44`, `:92-93`). `None` ⇒ no saved decisions are
    /// visible and nothing is persisted (embedders/tests). SEAM-065.
    trust_store: Option<Arc<TrustStore>>,
    /// The interactive project-trust prompt (pi `selectProjectTrustOption` → `ctx.ui.select`,
    /// `project-trust.ts:28-44`, `:90-94`). Invoked **only** when the tiered decision comes back
    /// [`TrustOutcome::NeedsPrompt`], i.e. after `pre_trust_extension_verdict` and the store —
    /// which is the ordering SEAM-065 exists to restore. `None` ⇒ no UI (pi's `hasUI` false branch,
    /// `:86-88`), so the run proceeds untrusted.
    trust_prompt: Option<TrustPromptFn>,
}

/// The interactive project-trust prompt seam (pi `selectProjectTrustOption`,
/// `packages/coding-agent/src/core/project-trust.ts:28-44` @v0.83.0).
///
/// The builder supplies the option set pi builds — `getProjectTrustOptions(cwd, {
/// includeSessionOnly: true })` (`:32`) — plus the nearest saved decision for the header line, and
/// the callback returns the resolved trust flag (`Some(true)`/`Some(false)`) or `None` for a
/// cancelled prompt (pi's `ui.select → undefined`, which falls through to `return false` at `:95`).
///
/// Persisting the chosen option's `updates` is the callback's job, because it is pi's:
/// `saveProjectTrustPromptResult(trustStore, result)` runs inside `selectProjectTrustOption`
/// (`:39`) under the `updates.length > 0` guard (`:40-44`) that makes the two "(this session only)"
/// rows write nothing.
pub type TrustPromptFn =
    Arc<dyn Fn(&[TrustOption], &Option<TrustEntry>) -> Option<bool> + Send + Sync>;

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
            trust_store: None,
            trust_prompt: None,
        }
    }

    /// Wire the project-trust store so the saved-decision tier and the `remember` persist can run
    /// inside the build, where pi runs them (`project-trust.ts:64-66`, `:72-75`). SEAM-065.
    #[must_use]
    pub fn trust_store(mut self, store: Arc<TrustStore>) -> Self {
        self.trust_store = Some(store);
        self
    }

    /// Wire the interactive project-trust prompt (pi `ctx.ui.select`, `project-trust.ts:90-94`).
    /// See [`TrustPromptFn`]. SEAM-065.
    #[must_use]
    pub fn trust_prompt(mut self, prompt: TrustPromptFn) -> Self {
        self.trust_prompt = Some(prompt);
        self
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

    /// Transient settings overrides applied on top of the merged `global ◁ project` view — pi's
    /// `SettingsManager.applyOverrides` (settings-manager.ts:508-510 @v0.83.0), the seam its SDK
    /// example (`examples/sdk/10-settings.ts:17`) and test harness (`test/test-harness.ts:395`)
    /// use. They are NOT a persistent layer: `SettingsManager` holds exactly the two scopes pi
    /// holds, and anything set here is discarded by a later recompute, exactly as upstream's
    /// override of `this.settings` is (CFG-059).
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
}
