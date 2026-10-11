//! `SessionConfig` + `SessionBuilder` — assemble an [`AgentSession`] from the real subsystems
//! (arch-11 §3.3). One async `build()` resolves settings + trust + auth + model (cyrup-config),
//! discovers resources (cyrup-resources), builds the tool registry with isolation decorators +
//! permission policy (cyrup-tools), opens/creates the session tree and wires compaction
//! (cyrup-session arch-04/05), assembles the system prompt + context store (arch-06), builds the
//! extension host with native built-ins and attaches BOTH ext seams to the agent (cyrup-ext), and
//! resolves the provider into the agent loop (cyrup-provider).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_config::trust::{TrustEntry, TrustOption, TrustStore, trust_options};
use cyrup_config::{
    AppMode, AuthStore, ExtensionTrust, InMemorySettingsStore, ModelResolver, Settings,
    SettingsManager, SettingsStore, TrustInputs, TrustOutcome, decide_trust_with_extension,
    has_trust_requiring_resources,
};
use cyrup_core::{CancelToken, ModelRef, ModelThinkingLevel, RunCancel};
use cyrup_ext::{EventKind, ExtMode, ExtensionHost, HostConfig, HostEvent, NativeExtension};
use cyrup_provider::{Model, Provider};
use cyrup_resources::{
    ConfiguredPackage, DiscoveryConfig, InstallScope, InstalledPackages, PackageFilter,
    PackageStore, ResourceOverrides, SkillPointer, discover,
};
use cyrup_session::SessionLayout;
use cyrup_session::manager::{NewSessionOpts, SessionManager};
use cyrup_session::prompt::{
    ContextFile, ContextFileLoader, ContextSnapshot, DocsPointers, PromptInputs, ResolvedOverride,
    SystemPromptBuilder, ToolPromptContribution,
};
use cyrup_tools::{
    Availability, Backend, BashOpts, PermissionPolicy, ProtectedFs, ProtectedPaths, ToolRegistry,
    ToolsOptions, TraversalFs,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::SessionServiceError;
use crate::event::raw_message_to_agent;
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
    /// Whether this build may CLONE a settings-declared git package whose working tree is missing
    /// (CFG-003), i.e. `cyrup_resources::DiscoveryConfig::install_missing_packages` — see that
    /// field for the upstream mapping (`true` = pi's `resolve()` from the resource loader,
    /// resource-loader.ts:403 @v0.83.0; `false` = pi's `resolve(async () => "skip")`,
    /// cli/startup-ui.ts:73).
    ///
    /// Defaults to `false` so an SDK embedder's `build()` performs no network I/O it did not ask
    /// for. The bin sets it to `!(--offline || CYRUP_OFFLINE)` (`main.rs`), which is
    /// pi's own gate — `isOfflineModeEnabled()`, package-manager.ts:42-46, consulted at `:1261`.
    pub install_missing_packages: bool,
    /// pi `modelNetworkEnabled` (`core/model-runtime.ts:182`): whether a catalog refresh that does
    /// not say `allowNetwork` may use the network, for the providers extensions registered. Both
    /// the interactive startup refresh and the post-`/login` refresh leave it unsaid
    /// (`options.allowNetwork ?? this.modelNetworkEnabled`, `model-runtime.ts:850`), so with this
    /// off an offline run's `/login llama.cpp` restores the stored catalog and makes no request.
    ///
    /// Defaults to `true` for an SDK embedder. The bin sets it to `!(--offline || CYRUP_OFFLINE)`
    /// (`main.rs`), pi's own and only gate for the runtime catalog refresh.
    pub model_network_enabled: bool,
    /// How long the cache-only restore of the extension providers' stored catalogs may take at
    /// build before it is abandoned (the session then starts with what was restored so far). It is
    /// a local file read, so this only ever bites a provider that does slow work in a phase that is
    /// told it has no network.
    pub provider_restore_timeout: std::time::Duration,
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
    /// [`native_survives_no_extensions`] for the discriminator and for the one carve-out pi makes in
    /// a subagent child.
    pub no_extensions: bool,
    /// Ids of native built-in extensions this session does not load (pi
    /// `resourceLoaderOptions.disabledBuiltinExtensions`, `main.ts` @v1.0.4, which `--no-mcp` sets
    /// to `["mcp"]`: "Disable built-in MCP support: no servers connect and no MCP tools"). Unlike
    /// `--no-extensions` this removes the named built-in whatever tier it is in.
    pub disabled_builtin_extensions: Vec<String>,
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
    /// [CYRUP-DELTA] Wrap the **fs** backend in [`ProtectedFs`], refusing `write`/`edit` to
    /// `.env`, `.git/` and `node_modules/` (R-12-006). **Off by default** and embedder-only: there
    /// is deliberately no CLI flag and no `settings.json` key. pi's *core* has no protected-path
    /// predicate — `write.ts::createWriteToolDefinition` resolves the path and calls
    /// `ops.writeFile` with none, and `edit.ts::execute` does the same — so this `false` default
    /// matches default pi exactly. pi nonetheless SHIPS the guard, as an auto-discovered opt-in
    /// extension over the same three names (`examples/extensions/protected-paths.ts`, catalogued
    /// at `docs/extensions.md:2944`, blocking `write`/`edit` at the `tool_call` gate and equally
    /// blind to `bash`). What is `[CYRUP-DELTA]` is this **configuration surface**, not the
    /// capability: pi's `src/` has no `protectedPaths` option, and cyrup's user-reachable
    /// equivalent of pi's enablement is a `.cyrup/extensions/` extension on `tool_call` — which is
    /// why there is no flag and no settings key here (ADR-0003 D5/D6).
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
            model_network_enabled: true,
            provider_restore_timeout: std::time::Duration::from_secs(10),
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
            disabled_builtin_extensions: Vec::new(),
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
            // ADR-0003 D5: pi's core writes whatever path it is given
            // (`write.ts::createWriteToolDefinition` -> `ops.writeFile`, no predicate), and pi's
            // own opt-in extension is off until a user installs it — so `false` IS default-pi
            // parity. `bash` bypassed the guard anyway, so on-by-default bought nothing and cost a
            // failed turn. Inert embedder-only opt-in now.
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
    /// Start with no built-in tool enabled, but keep extension/custom tools enabled.
    ///
    /// pi's own doc comment says "disable the default built-in tools (read, bash, edit, write)",
    /// but its code disables ALL of them: `options.noTools ? [] : …` (sdk.ts:261-262) tests the
    /// PRESENCE of the option.
    ///
    /// The ONE difference cyrup models is on the line above: extension tools survive `Builtin` and
    /// not `All`, because `_buildRuntime({ includeAllExtensionTools: true })` adds them only when
    /// `allowedToolNames` is `undefined` (`agent-session.ts:406-410` → `:2740-2745`), and
    /// `allowedToolNames` is `[]` under `"all"` and `undefined` under `"builtin"` (`sdk.ts:258`).
    /// That is what `select_active_tools` implements and what its test pins.
    ///
    /// Upstream's OTHER consequence of that same `allowedToolNames` — a built-in stays enable-able
    /// at runtime under `"builtin"` and not under `"all"` — is NOT modelled here: cyrup has no
    /// `allowedToolNames` and no runtime re-enable path, and `no_tools` is read at exactly one site
    /// in the workspace (`select_active_tools`). Registration is independent of the flag under
    /// both modes. The distinction is kept as two variants because cyrup will need it when that
    /// path is ported, not because HEAD behaves differently in any second way.
    Builtin,
}

/// Build the provider-scoped env overlay for the configured HTTP proxy (Pi `applyHttpProxySettings`,
/// http-dispatcher.ts:42-47): a non-empty `httpProxy` setting sets both `HTTP_PROXY` and `HTTPS_PROXY`
/// in the overlay (matching Pi's `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`),
/// so the provider's `resolveHttpProxyUrlForTarget` routes requests through it. An absent/blank setting
/// yields `None` (the ambient process env is used unchanged).
/// Pi `applyHttpProxySettings(settings.httpProxy)` — BOTH of its layers (PROV-047).
///
/// Upstream this is a single function that writes the **process environment**
/// (`process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`,
/// `coding-agent/src/core/http-dispatcher.ts:43-48` @v0.83.0, `:45-50` @v0.84.1), called at startup
/// from `cli.ts:18` / `rpc-entry.ts:10` and re-applied from `main.ts:744`. Because the value lands
/// in the env, EVERY later proxy consultation in the process observes it — the global undici
/// dispatcher (`:79-93`) that `globalThis.fetch` runs on (`:103`), OAuth token exchange, silent
/// token refresh, catalog refreshes, the agent proxy transport, extension HTTP — and
/// `node-http-proxy.ts`'s per-request resolution is a SECOND layer on top, not a replacement.
///
/// cyrup needs both layers explicitly:
///
/// 1. [`cyrup_provider::configure_http_proxy`] is the stand-in for pi's env write
///    (`std::env::set_var` is `unsafe` from edition 2024 and races every concurrently running
///    thread's `getenv`). It is consulted by the ported resolver at exactly the layer pi's env
///    write is observed, so the `??=` precedence survives: an ambient `HTTP_PROXY`/`HTTPS_PROXY`
///    still wins. It is called **unconditionally, including with `None`**, so clearing the setting
///    clears the global rather than leaving the previous value installed.
/// 2. [`http_proxy_overlay`] is pi's second layer — the provider-scoped `StreamOptions.env`
///    returned here for the caller to attach, which is what reaches the streaming wire APIs.
///
/// Layer 1 alone was missing until PROV-047, which is why `httpProxy` reached the streams and
/// nothing else.
fn apply_http_proxy_settings(proxy: Option<String>) -> Option<cyrup_provider::ProviderEnv> {
    cyrup_provider::configure_http_proxy(proxy.clone());
    http_proxy_overlay(proxy.as_deref())
}

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

/// Pi's default active built-in tool names.
///
/// CFG-097 — an ALIAS, not a copy. Upstream exported the baseline from the settings module as
/// `DEFAULT_TOOL_NAMES` (`core/settings-manager.ts:215` @v1.0.0) and deleted `core/sdk.ts`'s local
/// `defaultActiveToolNames` in favour of importing it (`sdk.ts:267`), precisely so the list the
/// `defaultTools` resolver starts from and the list this selector falls back to cannot drift. The
/// `use … as` keeps that single source while leaving every reader below spelled as before.
use cyrup_config::DEFAULT_TOOL_NAMES as DEFAULT_BUILTIN_TOOLS;

/// Every tool `ToolRegistry::with_builtins` installs (`cyrup-tools/src/registry.rs:53-100`).
///
/// Needed to tell "a built-in pi does not activate by default" (`powershell`/`grep`/`find`/`ls`)
/// apart from "a non-built-in tool" (an extension- or embedder-supplied one), which must stay
/// active: pi's `defaultActiveToolNames` gates only its own built-ins and never suppresses a tool
/// the host registered.
///
/// `powershell` MUST be listed here. The default arm of `select_active_tools` keeps any name it
/// does not recognise as a built-in, so omitting it would enable PowerShell in every session —
/// while pi's default set is `read`/`bash`/`edit`/`write` (sdk.ts:256) and `powershell` is reachable
/// only through `--tools` / `defaultTools`.
const ALL_BUILTIN_TOOLS: [&str; 8] = [
    "read",
    "write",
    "edit",
    "bash",
    "powershell",
    "grep",
    "find",
    "ls",
];

/// Apply the `tools`/`noTools`/`excludeTools` selection over the Availability-visible tool set
/// (Pi sdk.ts:256-263), with the `defaultTools` setting standing in for pi's
/// `defaultActiveToolNames` when it is configured.
///
/// Upstream v0.84.4 (`sdk.ts:261-263`) is one expression:
///
/// ```text
/// options.tools ?? (options.noTools ? [] : (configuredDefaultToolNames ?? defaultActiveToolNames))
/// ```
///
/// followed by the `excludeTools` filter — so `default_tools` is consulted only on the arm where
/// neither an explicit allowlist nor a suppression mode is set, and it REPLACES the four defaults
/// rather than adding to them.
///
/// `default_tools` narrows built-ins ONLY. An extension- or embedder-supplied tool is kept by the
/// `!ALL_BUILTIN_TOOLS.contains(name)` leg regardless of what the setting lists, which is upstream's
/// `includeAllExtensionTools: true` at construction (`agent-session.ts:406-410` →
/// `_refreshToolRegistry`'s `:2742-2745` branch). That is deliberate and was a bug fix upstream:
/// `defaultTools` first shipped as an `allowedToolNames` allowlist (`4d9aa837c`) that dropped
/// extension and SDK custom tools, and `541045ae0` ("preserve extension tools with defaults")
/// narrowed it to the built-in selection before v0.84.4 shipped.
///
/// The other half of the setting — a configured name an EXTENSION registered, `codemode` for one —
/// is applied further down `build`, against the registry the extensions filled
/// ([`crate::default_tools`]); this function never sees those tools' names.
fn select_active_tools(
    visible: &[Arc<dyn cyrup_core::Tool>],
    cfg: &SessionConfig,
    default_tools: Option<&[String]>,
) -> Vec<Arc<dyn cyrup_core::Tool>> {
    // Pi `_isActivatedOnRegistration` for the non-built-in tools (`agent-session.ts:3554` @v1.0.1):
    // a `codemode`, `deferred` or `hidden` tool, or one registered `defaultActive: false`, is
    // registered but not active at start.
    let registration_activates =
        |t: &Arc<dyn cyrup_core::Tool>| t.exposure().activated_on_registration(t.default_active());
    let keep = |t: &Arc<dyn cyrup_core::Tool>| -> bool {
        let name = t.name();
        match (&cfg.tools, cfg.no_tools) {
            // Explicit allowlist wins (Pi `options.tools`): naming a tool activates it iff it is
            // declarable, even when it is not active by default (`agent-session.ts:3510-3516`).
            // `--tools` entries are exact names or `*` patterns (`createToolNameMatcher`).
            (Some(allow), _) => {
                cyrup_core::tool_name_matches(allow, name) && t.exposure().declarable()
            }
            (None, Some(NoTools::All)) => false,
            // SEAM-118: `!ALL_BUILTIN_TOOLS`, not `!DEFAULT_BUILTIN_TOOLS`. Upstream's expression
            // branches on the PRESENCE of `noTools`, not on its value —
            // `options.noTools ? [] : (configuredDefaultToolNames ?? defaultActiveToolNames)`
            // (sdk.ts:261-262) — so `"builtin"` starts the session with an EMPTY built-in
            // selection, exactly like `"all"`. The two modes part company one line earlier, at
            // `allowedToolNames = options.tools ?? (options.noTools === "all" ? [] : undefined)`
            // (`:258`): `"all"` also forbids re-enabling anything, `"builtin"` leaves every
            // built-in ENABLE-able at runtime. Extension/embedder tools survive `"builtin"` only
            // through the `!ALL_BUILTIN_TOOLS` leg, which is upstream's
            // `includeAllExtensionTools: true` (`agent-session.ts:406-410` → `:2742-2745`).
            //
            // This arm read `!DEFAULT_BUILTIN_TOOLS`, so `--no-builtin-tools` dropped only pi's
            // four defaults and left `grep`, `find`, `ls` and `powershell` ACTIVE — the flag
            // advertised a smaller tool surface than it delivered, `powershell` included.
            (None, Some(NoTools::Builtin)) => {
                !ALL_BUILTIN_TOOLS.contains(&name) && registration_activates(t)
            }
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
            //
            // CFG-079: `defaultTools` (settings-manager.ts:128, getter `:1273-1276`) replaces
            // `DEFAULT_BUILTIN_TOOLS` on this arm when it is configured. An explicit `[]` is a
            // configured value — it means "no built-ins", not "unset" — which is why the setting is
            // an `Option<&[String]>` and not a `Vec` defaulted to the four names.
            (None, None) => {
                let selected = match default_tools {
                    Some(configured) => configured.iter().any(|t| t == name),
                    None => DEFAULT_BUILTIN_TOOLS.contains(&name),
                };
                selected || (!ALL_BUILTIN_TOOLS.contains(&name) && registration_activates(t))
            }
        }
    };
    visible
        .iter()
        .filter(|t| keep(t) && !cyrup_core::tool_name_matches(&cfg.exclude_tools, t.name()))
        .cloned()
        .collect()
}

/// A `tools` list resolved against pi v1.1.0's `+name`/`-name` modifier form (SEAM-148).
///
/// pi interprets the modifiers in the SDK `tools` option itself (`core/sdk.ts:280-294`
/// @f1b2e77f5): a modifier-only list is applied, in order, to the default selection —
/// `options.noTools ? [] : (settings.getDefaultTools() ?? DEFAULT_TOOL_NAMES)` — instead of being
/// an allowlist, and `allowedToolNames` stays `undefined` unless `noTools === "all"`, where it is
/// the post-modifier set. `cfg` is the selection the three helpers below then see:
///
/// * no modifiers — the caller's config untouched, `default_tools` the `defaultTools` setting;
/// * modifiers with `NoTools::All` — an explicit allowlist of the selected names, which is pi's
///   `allowedToolNames = selectedToolNames` AND `initialActiveToolNames = selectedToolNames`;
/// * otherwise — neither `tools` nor `noTools`, with `default_tools` the selected names. Under
///   `NoTools::Builtin` the base is empty (`defaultToolNames = []`), so only the `+name` built-ins
///   are selected and extension tools keep the `!ALL_BUILTIN_TOOLS` leg, exactly the
///   `noTools: "builtin"` arm plus pi's `initialActiveToolNames`.
///
/// `modifiers` is pi's `defaultToolModifiers` (`sdk.ts:473`), reapplied on `/reload`.
struct ToolSelection<'a> {
    cfg: std::borrow::Cow<'a, SessionConfig>,
    default_tools: Option<Vec<String>>,
    modifiers: Vec<String>,
}

impl ToolSelection<'_> {
    /// pi `usesDefaultTools` (`sdk.ts:472` @f1b2e77f5):
    /// `(options.tools === undefined || toolModifiers !== undefined) && !options.noTools`.
    fn uses_default_tools(&self, original: &SessionConfig) -> bool {
        original.no_tools.is_none() && (original.tools.is_none() || !self.modifiers.is_empty())
    }
}

/// pi `getToolListError` on the SDK `tools` option (`core/sdk.ts:280-281` @f1b2e77f5):
/// `throw new Error(`Invalid tools option: ${toolListError}`)` (a template literal).
fn check_tool_list(cfg: &SessionConfig) -> Result<(), SessionServiceError> {
    match cfg
        .tools
        .as_deref()
        .and_then(cyrup_config::get_tool_list_error)
    {
        Some(err) => Err(SessionServiceError::InvalidToolsOption(err)),
        None => Ok(()),
    }
}

/// Resolve `cfg.tools` per [`ToolSelection`]. `configured` is the resolved `defaultTools` setting.
fn resolve_tool_selection(
    cfg: &SessionConfig,
    configured: Option<Vec<String>>,
) -> ToolSelection<'_> {
    let modifiers: Vec<String> = match &cfg.tools {
        Some(tools) if tools.iter().any(|t| cyrup_config::is_tool_modifier(t)) => tools.clone(),
        _ => Vec::new(),
    };
    if modifiers.is_empty() {
        return ToolSelection {
            cfg: std::borrow::Cow::Borrowed(cfg),
            default_tools: configured,
            modifiers,
        };
    }
    let base: Vec<String> = if cfg.no_tools.is_some() {
        Vec::new()
    } else {
        configured.unwrap_or_else(|| {
            DEFAULT_BUILTIN_TOOLS
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        })
    };
    let selected = cyrup_config::apply_tool_modifiers(&base, &modifiers);
    let mut effective = cfg.clone();
    let default_tools = if cfg.no_tools == Some(NoTools::All) {
        effective.tools = Some(selected);
        None
    } else {
        effective.tools = None;
        effective.no_tools = None;
        Some(selected)
    };
    ToolSelection {
        cfg: std::borrow::Cow::Owned(effective),
        default_tools,
        modifiers,
    }
}

/// pi `sdk.ts:258`'s `allowedToolNames` —
/// `options.tools ?? (options.noTools === "all" ? [] : undefined)` — the SESSION-level allowlist.
///
/// A DIFFERENT value from [`select_active_tools`]' initial BUILT-IN selection (`sdk.ts:261-262`),
/// and the arms below are deliberately written in the same shape and order as that function's
/// `keep` closure so the two can be diffed by eye. They part company on exactly one arm:
/// [`NoTools::Builtin`] empties the built-in selection but leaves `allowedToolNames` `undefined`,
/// which is what keeps extension tools active (`agent-session.ts:406-410` → `:2742-2745`). Narrowing
/// that arm to `Some(∅)` is the one way to over-filter and would break plan-mode and the permission
/// companion; see [`NoTools`]'s own doc.
///
/// `None` is "nothing pinned the surface"; `Some(∅)` — which an explicit `tools: []` and
/// `NoTools::All` both produce — is an allowlist that denies everything.
fn resolve_allowed_tool_names(cfg: &SessionConfig) -> Option<std::collections::HashSet<String>> {
    match (&cfg.tools, cfg.no_tools) {
        // Explicit allowlist wins (Pi `options.tools`), INCLUDING an explicit empty one.
        (Some(allow), _) => Some(allow.iter().cloned().collect()),
        (None, Some(NoTools::All)) => Some(std::collections::HashSet::new()),
        (None, Some(NoTools::Builtin)) | (None, None) => None,
    }
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
    /// Where the `codemode` extension finds the session it runs in. Set by [`Self::with_codemode`]
    /// or [`Self::codemode_host_slot`]; each built session binds its own host into it.
    codemode_host_slot: Option<cyrup_codemode_runtime::tool::CodemodeHostSlot>,
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
    /// The shared catalog service this session's [`AgentSessionServices::catalog_overlay`] slot
    /// comes from, and that [`crate::session::AgentSession::refresh_model_catalogs`] triggers
    /// through (XAI_3). `None` ⇒ the session builds its OWN disk-only slot via
    /// [`Self::load_persisted_catalog_overlay`] — today's exact behavior, and what every embedder /
    /// SDK caller / test that never wires one still gets.
    ///
    /// [`AgentSessionServices::catalog_overlay`]: crate::services::AgentSessionServices::catalog_overlay
    model_catalog_service: Option<Arc<cyrup_provider::ModelCatalogService>>,
    /// The virtual-model registry this session will own — pi `ModelRuntime.virtualModels`
    /// (`model-runtime.ts:179`). `None` (the default) builds an empty one, which is a session with
    /// no virtual models and therefore byte-for-byte the pre-feature behaviour.
    ///
    /// An embedder that registers virtual models BEFORE the session opens injects its own here, so
    /// the registrations are already visible to the first catalog read and to the restore step —
    /// upstream's ordering, where `pendingVirtualModelRegistrations` drain in
    /// `createAgentSessionServices` before `createAgentSession` restores the selection
    /// (`agent-session-services.ts:182-194`).
    virtual_models: Option<Arc<cyrup_provider::VirtualModelRegistry>>,
    /// This build is the in-process `/reload` rebuild ([`crate::AgentSessionRuntime::reload`]),
    /// which stands in for pi's `reload` keeping `getActiveToolNames()` (`agent-session.ts:3682-
    /// 3686` @f1b2e77f5). Only then does a `+name`/`-name` `tools` session resume its transcript's
    /// loadout; every other start applies the modifiers, as pi's `sdk.ts:294` does (SEAM-148).
    reload_rebuild: bool,
    /// The interactive project-trust prompt (pi `selectProjectTrustOption` → `ctx.ui.select`,
    /// `project-trust.ts:28-44`, `:90-94`). Invoked **only** when the tiered decision comes back
    /// [`TrustOutcome::NeedsPrompt`], i.e. after `pre_trust_extension_verdict` and the store —
    /// which is the ordering SEAM-065 exists to restore. `None` ⇒ no UI (pi's `hasUI` false branch,
    /// `:86-88`), so the run proceeds untrusted.
    trust_prompt: Option<TrustPromptFn>,
    /// EXT-003, tests only: make the pre-trust pass behave as if `ExtensionHost::with_wasm`
    /// returned `Err`, so the native-only fallback is exercisable. There is no production switch
    /// for this and no production build carries the field — the real trigger is a machine on which
    /// Wasmtime's engine cannot be constructed (`cyrup-ext/src/host/engine.rs:19-26`, the pooling
    /// allocator reserving its slabs), which cannot be staged from inside the test process.
    #[cfg(test)]
    force_pre_trust_wasm_failure: bool,
    /// Tests only: make the session's own extension host behave as if the Wasmtime runtime could
    /// not be constructed at all (neither the pooling nor the on-demand engine), so the
    /// native-only host is exercisable. Same reasoning as the field above.
    #[cfg(test)]
    force_runtime_wasm_failure: bool,
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
///
/// Returns a boxed future because that persist runs through `TrustStore::set_many`, which is async
/// (`cyrup-config/src/trust.rs`) — the host's implementation cannot answer synchronously. The one
/// call site ([`SessionBuilder::build`]) already awaits inside `async fn`, so the box is the only
/// per-prompt cost the *builder* pays.
///
/// Implementors face three constraints, all of them read off the signature above:
///
/// - The returned future borrows `options` and `saved` for `'a`, so it cannot be spawned, stored,
///   or outlive the call. That is deliberate: `build()` cannot settle the trust flag until the
///   prompt answers, so a detached prompt would be meaningless.
/// - `'a` is the *arguments'* lifetime. It cannot name the `Fn`'s own `&self` borrow — that
///   lifetime is elided, separate, and absent from the return type — so nothing the closure
///   captured may be borrowed into the future. Every implementor clones what it needs into the
///   future on each invocation; see `trust_prompt_callback` in `crates/cyrup/src/prelaunch.rs`.
///   Taking the arguments by value would not change this: the `&self` borrow is what is
///   unnameable, not the argument borrows.
/// - `Send` on the future rules out holding a `!Send` UI handle across an await inside the prompt.
///
/// All three are priced for a callback invoked at most once per session build, immediately before
/// blocking on a human at a terminal.
pub type TrustPromptFn = Arc<
    dyn for<'a> Fn(
            &'a [TrustOption],
            &'a Option<TrustEntry>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<bool>> + Send + 'a>>
        + Send
        + Sync,
>;

impl SessionBuilder {
    /// Start a builder over the resolved `provider` and a `config`.
    pub fn new(provider: Arc<dyn Provider>, config: SessionConfig) -> Self {
        Self {
            provider,
            config,
            settings_store: Arc::new(InMemorySettingsStore::new()),
            auth: None,
            native_extensions: Vec::new(),
            codemode_host_slot: None,
            cli_settings: Settings::new(),
            prebuilt_manager: None,
            provider_resolver: None,
            stream_fn: None,
            key_resolver: None,
            skills_override: None,
            context_files_override: None,
            trust_store: None,
            trust_prompt: None,
            model_catalog_service: None,
            virtual_models: None,
            reload_rebuild: false,
            #[cfg(test)]
            force_pre_trust_wasm_failure: false,
            #[cfg(test)]
            force_runtime_wasm_failure: false,
        }
    }

    /// Mark this build as the `/reload` rebuild (see the `reload_rebuild` field).
    #[must_use]
    pub(crate) fn reload_rebuild(mut self) -> Self {
        self.reload_rebuild = true;
        self
    }

    /// Wire the shared [`cyrup_provider::ModelCatalogService`] this session refreshes through and
    /// shares its live overlay slot with (XAI_3, FINDING 3). When set, the session's
    /// `catalog_overlay` is `svc.overlay()` — the SAME slot the service's other trigger(s) install
    /// into — so a refresh completing anywhere reaches this session's very next registry read with
    /// no rebuild. `None` (the default) keeps today's behavior exactly: a disk-only overlay loaded
    /// once at build time into a slot nothing else shares.
    #[must_use]
    pub fn model_catalog_service(mut self, svc: Arc<cyrup_provider::ModelCatalogService>) -> Self {
        self.model_catalog_service = Some(svc);
        self
    }

    /// Adopt a pre-populated virtual-model registry instead of building an empty one — see the
    /// field of the same name. The registry is shared, not cloned, so later registrations through
    /// the same `Arc` reach the built session.
    #[must_use]
    pub fn virtual_models(mut self, registry: Arc<cyrup_provider::VirtualModelRegistry>) -> Self {
        self.virtual_models = Some(registry);
        self
    }

    /// Force the EXT-003 native-only fallback in the pre-trust project-trust pass: the pass behaves
    /// as if the Wasmtime runtime could not be constructed. Tests only — see the field of the same
    /// name; production code has no way to reach this and no build outside `cfg(test)` compiles it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn force_pre_trust_wasm_failure(mut self) -> Self {
        self.force_pre_trust_wasm_failure = true;
        self
    }

    /// Force the native-only fallback of the session's own extension host: it behaves as if the
    /// Wasmtime runtime could not be constructed. Tests only, like
    /// [`Self::force_pre_trust_wasm_failure`].
    #[cfg(test)]
    #[must_use]
    pub(crate) fn force_runtime_wasm_failure(mut self) -> Self {
        self.force_runtime_wasm_failure = true;
        self
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

    /// Register the built-in `codemode` extension (pi's `builtin:codemode`, `extensions/index.ts:9-14`
    /// @v1.0.1): the extension is loaded like any native built-in, and each session built from this
    /// builder is bound into the extension's host slot, so the tool it registers can call tools,
    /// replay the branch's `store()` entries and reach the model registry as pi's `ctx` lets it.
    #[must_use]
    pub fn with_codemode(self, ext: cyrup_codemode_runtime::CodemodeExtension) -> Self {
        let slot = ext.host().clone();
        self.with_native_extension(Arc::new(ext))
            .codemode_host_slot(slot)
    }

    /// Bind each built session into `slot`, for a codemode extension that is already registered
    /// through [`Self::with_native_extension`] (what [`crate::SessionFactory`] does per build).
    #[must_use]
    pub fn codemode_host_slot(
        mut self,
        slot: cyrup_codemode_runtime::tool::CodemodeHostSlot,
    ) -> Self {
        self.codemode_host_slot = Some(slot);
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
        let store: Arc<dyn cyrup_provider::ModelsStore> =
            Arc::new(cyrup_config::models_store::FileModelsStore::new(
                agent_dir.join(cyrup_config::models_store::MODELS_STORE_FILE_NAME),
            ));
        let catalog = cyrup_provider::RemoteCatalog::new(store)
            .with_local_generated_at(cyrup_provider::builtin_model_data_generated_at())
            .with_local_generated_at_by_provider(
                cyrup_provider::builtin_model_data_generated_at_by_provider(),
            );
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
        // SEAM-148: pi rejects a mixed or patterned modifier list before building anything
        // (`core/sdk.ts:280-281` @f1b2e77f5).
        check_tool_list(&cfg)?;
        // The native built-ins the launch flags turned off (pi `disabledBuiltinExtensions`) are
        // never loaded, not even to vote on project trust.
        let native_extensions =
            without_disabled_builtins(self.native_extensions, &cfg.disabled_builtin_extensions);
        // Embedder-supplied seams pulled out before the rest of `self` is consumed piecewise below.
        let custom_stream_fn = self.stream_fn;
        let custom_key_resolver = self.key_resolver;
        let skills_override = self.skills_override;
        let context_files_override = self.context_files_override;
        let cwd = cfg.cwd.clone();
        let cancel = RunCancel::new();

        // Pi `DefaultResourceLoader.reload()` opens with `resetTimings("extensions")`
        // (`core/resource-loader.ts:389` @v0.87.1), ahead of BOTH its pre-trust pass
        // (`loadProjectTrustExtensions`) and the final load — the two passes this build runs at
        // step 1 and step 4b/4c. Resetting here makes the `extensions` startup-timing rows this
        // session's own load, whichever session the process built before it (AGENT-027).
        cyrup_core::timings::reset_timings(cyrup_core::timings::TimingLabel::Extensions);

        // ---- 1. settings + trust (cyrup-config) ------------------------------------------------
        // Load global first (project untrusted) to read defaultProjectTrust, then decide trust.
        let mut settings = SettingsManager::load(self.settings_store.clone(), false);
        let default_trust = settings.effective().default_project_trust();
        let has_resources = has_trust_requiring_resources(&cwd, &cfg.home);
        // Pi's `shouldResolveProjectTrust` guard (main.ts:676-678): only pay for a pre-trust
        // extension pass when the answer is actually in doubt — no explicit `--approve/--no-approve`
        // and there IS something to gate. In every other case this is the exact previous code path.
        let ext_trust = if cfg.trust_override.is_none() && has_resources {
            #[cfg(test)]
            let force_wasm_failure = self.force_pre_trust_wasm_failure;
            #[cfg(not(test))]
            let force_wasm_failure = false;
            pre_trust_extension_verdict(
                &cfg,
                &cwd,
                &native_extensions,
                &settings.global().extension_paths(),
                force_wasm_failure,
            )
            .await
        } else {
            None
        };
        // SEAM-065 — the saved-decision tier is read HERE, not by the caller, because pi reads it
        // at `project-trust.ts:72-75`, i.e. strictly AFTER `emitProjectTrustEvent` (`:54-70`).
        let saved = match self.trust_store.as_ref() {
            Some(store) => store.nearest(&cwd).await.ok().flatten(),
            None => None,
        };
        if let Some(d) = &ext_trust
            && d.remember
        {
            // Pi persists the extension's verdict itself when it asked to be remembered:
            // `if (result.remember) { trustStore.set(cwd, trusted); }` (project-trust.ts:64-66).
            match self.trust_store.as_ref() {
                Some(store) => {
                    let decision = if d.trusted {
                        cyrup_config::TrustDecision::Trusted
                    } else {
                        cyrup_config::TrustDecision::Untrusted
                    };
                    if let Err(e) = store.set(&cwd, Some(decision)).await {
                        tracing::warn!(error = %e, "persisting extension project_trust verdict");
                    }
                }
                None => tracing::warn!(
                    extension = %d.by, trusted = d.trusted,
                    "extension project_trust asked to `remember` the decision, but no trust store \
                     is wired into the session builder — the verdict applies to this session only"
                ),
            }
        }
        let inputs = TrustInputs {
            has_resources,
            trust_override: cfg.trust_override,
            saved: saved.as_ref().map(|e| e.decision),
            default_trust,
            mode: cfg.app_mode,
            prompt_choice: None,
        };
        let outcome = decide_trust_with_extension(
            inputs,
            ext_trust.map(|d| ExtensionTrust {
                trusted: d.trusted,
                remember: d.remember,
            }),
        );
        let trusted = match outcome {
            TrustOutcome::Trusted => true,
            TrustOutcome::Untrusted => false,
            // Pi reaches its prompt LAST — `if (!hasUI) return false;` then
            // `selectProjectTrustOption(...)` (project-trust.ts:86-94). Both the `hasUI` gate and
            // the mode gate are already folded in: `decide_trust` only yields `NeedsPrompt` for an
            // interactive mode (`cyrup-config/src/trust.rs:294-299`), and a host with no terminal
            // wires no callback, which is pi's `hasUI === false` — proceed untrusted.
            TrustOutcome::NeedsPrompt => match self.trust_prompt.as_ref() {
                Some(prompt) => {
                    // `includeSessionOnly: true` — pi's PRE-LAUNCH prompt is the one call site that
                    // asks for the two ephemeral rows (project-trust.ts:32). SEAM-064. pi's IN-APP
                    // selector is the other call site and genuinely passes the default `false`
                    // (trust-selector.ts:44) — do not "fix" that one to match.
                    let options = trust_options(&cwd, true);
                    prompt(&options, &saved).await.unwrap_or(false)
                }
                None => false,
            },
        };
        settings.set_project_trusted(trusted);
        // Embedder-supplied overrides are applied HERE, after the trust decision has settled the
        // two persistent layers — pi's `applyOverrides` (settings-manager.ts:508-510), the same
        // shape its own harness uses (`test/test-harness.ts:395`:
        // `settingsManager.applyOverrides(options.settings)`). CFG-059: these used to be a THIRD
        // persistent layer inside `SettingsManager` that outranked project and survived every
        // recompute; upstream has no CLI settings tier, so the tier is gone and the transient
        // override path is the only one left. Applying after `set_project_trusted` matters: that
        // call recomputes from the layers, which would discard an override applied before it.
        if !self.cli_settings.is_empty() {
            settings.apply_overrides(&self.cli_settings);
        }

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
        // Shared with whoever registered virtual models before the session opened, else empty —
        // pi's `ModelRuntime.virtualModels` (`model-runtime.ts:179`). Taken HERE, ahead of the
        // restore, because the restore READS it: pi drains
        // `pendingVirtualModelRegistrations` in `createAgentSessionServices`
        // (`agent-session-services.ts:182-194`) before `createAgentSession` restores the
        // selection, so a virtual model registered before the session opened is already in the
        // registry when `branch_selection` asks whether the branch's selection is virtual.
        let virtual_models = self
            .virtual_models
            .clone()
            .unwrap_or_else(|| Arc::new(cyrup_provider::VirtualModelRegistry::new()));
        let existing = manager.build_context();
        // `SESS-067` — the VIRTUAL-AWARE selection, pi `getBranchSelection(sessionManager.
        // getBranch(), (p, id) => modelRuntime.getModel(p, id))` (`sdk.ts:207-211`). Taken here
        // beside `existing` under the same SEAM-112 rule, so both reads see one manager state.
        //
        // `existing.model` is the FORWARD last-wins projection value and must not be used for the
        // restore: under a virtual selection it names the physical model that answered, so a
        // resume would silently drop the router the user selected. The lookup answers only for
        // REGISTERED VIRTUAL models, which is provably equivalent to a full catalog lookup here —
        // see `cyrup_session::virtual_models::branch_selection`'s own proof.
        let session_selection = manager.branch_selection(|p, id| virtual_models.model_ref(p, id));
        // SEAM-112 — the RAW projection, taken here beside `existing` so BOTH reads see the same
        // manager state (pi calls `buildSessionContext()` ONCE, sdk.ts:190, and reuses the result
        // at :374). `existing` stays because `resolve_model` below restores the saved model +
        // thinking level from it (pi sdk.ts:191-242); `existing_raw` is what seeds the agent
        // transcript at step 7. See the comment there for why the flattened twin was wrong.
        let existing_raw = manager.build_context_raw();
        // Deliberately still the FLATTENED list. pi reads `existingSession.messages.length > 0`
        // off the raw projection (sdk.ts:191), and the two agree on every session cyrup writes:
        // `push_as_raw` (cyrup-session/src/context.rs:215-246) maps each context-visible entry to
        // exactly ONE raw message, and the single raw arm that can flatten to zero — a
        // `BashExecution` with `excludeFromContext` (`push_llm`, cyrup-session/src/agent_message.rs:
        // 191-197) — is never written by cyrup, whose `!!` executions persist as `custom_message`
        // entries ([`crate::AgentSession::record_bash_result`], session.rs:5550-5562) and so
        // survive the flattening as `custom`. Only a foreign file whose ONLY context entries are
        // `role:"bashExecution"` message entries could split the two, which is its own row.
        let has_existing_session = !existing.messages.is_empty();
        // Pi `hasThinkingEntry` (sdk.ts:189): does the resumed branch carry a thinking_level_change?
        let has_thinking_entry = manager.branch_path(None).iter().any(|e| {
            matches!(
                e,
                cyrup_session::Entry::Known(
                    cyrup_session::entry::KnownEntry::ThinkingLevelChange { .. }
                )
            )
        });

        // ---- 3. model resolution (cyrup-config + cyrup-provider) -------------------------------
        // Restore the model + thinking level from the resumed session, seeding a fallback message
        // when the saved model is no longer resolvable (Pi sdk.ts:191-242).
        //
        // EXT-027, an architectural gap (not a language or host constraint; the ledger owns it) —
        // an explicit `--model` the INSTALLED provider cannot resolve is not yet an error: pi resolves it only after every extension's provider is registered and
        // its persisted catalog restored (`buildSessionOptions` runs after
        // `createAgentSessionServices`, whose `modelRuntime.refresh({ allowNetwork: false })` is
        // `agent-session-services.ts:190-206` @v0.99.2-17), so `--model llama.cpp/<id>` names a
        // provider only the loaded built-in registers. cyrup resolves here, before any native is
        // loaded (this session is built around ONE provider the caller chose up front, where pi's
        // runtime holds every provider at once), so the pattern is set aside and resolved again at
        // "3b" below, once the extension providers exist; until then the session is seeded as if
        // no `--model` had been given. A pattern that still resolves nowhere at 3b fails with the
        // same `ModelNotFound` this arm used to return, carrying the launch path's own
        // unknown-provider text when a resolver is wired. Same resolution, later.
        // CFG-002/CFG-085 — `<agent_dir>/models.json`, read HERE rather than at the diagnostics
        // block below, because the startup model must be resolved against the COMPOSED catalog.
        // Upstream's `resolveCliModel`/`findInitialModel` are handed the `modelRuntime`
        // (`model-resolver.ts:649`, `:675`), and every provider in a `ModelRuntime` has already
        // been through `composeModelProvider` (`model-runtime.ts:215`) — so a `models.json` patch
        // is part of the row pi resolves, not a later amendment to it. cyrup resolved against the
        // bare `provider.models()`, which is why a `modelOverrides` patch reached `/model` (whose
        // registry IS composed, `session/model.rs`'s `compose_model_registry`) but not the model
        // the session started on: the two catalogs disagreed, and `inputLimits` declared on disk
        // was therefore inert for the whole first selection. The errors are still surfaced once,
        // below, where the rest of the startup diagnostics are assembled.
        let (model_file, model_file_error) =
            cyrup_config::load_models_file_reporting(&cfg.agent_dir.join("models.json"));
        let restore = RestoreInputs {
            existing: &existing,
            selection: session_selection.as_ref(),
            virtual_models: &virtual_models,
            has_session: has_existing_session,
            has_thinking_entry,
        };
        let mut deferred_model_pattern: Option<String> = None;
        let (mut resolved_model, mut model_ref, mut thinking, mut model_fallback_message) =
            match resolve_model(&*self.provider, &cfg, &settings, &restore, &model_file) {
                Err(SessionServiceError::ModelNotFound(pattern)) if cfg.model_pattern.is_some() => {
                    let mut without_pattern = cfg.clone();
                    without_pattern.model_pattern = None;
                    deferred_model_pattern = Some(pattern);
                    resolve_model(
                        &*self.provider,
                        &without_pattern,
                        &settings,
                        &restore,
                        &model_file,
                    )?
                }
                resolved => resolved?,
            };
        // The provider the session starts on: the injected one, replaced at 3b when the extension
        // provider that owns a deferred `--model` takes over.
        let mut initial_provider: Arc<dyn Provider> = self.provider.clone();

        // ---- 4. tools + isolation + policy (cyrup-tools) --------------------------------------
        // `shellPath`/`shellCommandPrefix` settings (Pi `getShellPath`/`getShellCommandPrefix`,
        // settings-manager.ts:864-865,895-896), read once here and threaded into BOTH bash seams:
        // the agent-loop `bash` tool (via `ToolsOptions.bash` below, matching Pi's `_buildRuntime`
        // passing `{commandPrefix, shellPath}` into `createAllToolDefinitions`, agent-session.ts:
        // 2436-2448) and the immediate-bash RPC seam (via `SessionExtras` below, matching Pi's
        // `executeBash` re-reading the same two settings, agent-session.ts:2624-2632).
        let shell_path_setting = settings.effective().shell_path();
        let shell_command_prefix_setting = settings.effective().shell_command_prefix();
        // No shell is resolved at session build. Pi resolves inside every `exec` (bash.ts:91) and
        // its session start never fails on a bash-less host; both cyrup bash seams now do the same,
        // so a missing bash surfaces as Pi's `No bash shell found` recipe on the command that
        // needed it, not as a failed session.
        let base = Backend::local();
        // The process backend the immediate-bash seam (#8) runs against (kept past `base`'s move).
        let bash_proc = base.proc.clone();
        let mut fs = base.fs.clone();
        if cfg.confine_to_cwd {
            fs = Arc::new(TraversalFs::new(fs, cwd.clone()));
        }
        if cfg.protect_paths {
            // ROOTED at the session cwd: `write` hands the backend an absolutized path
            // (`tools/write.rs:106`), so an unrooted matcher would test the cwd's own components
            // and refuse every write in a session rooted under e.g. `node_modules/`.
            fs = Arc::new(ProtectedFs::rooted(
                fs,
                cwd.clone(),
                ProtectedPaths::defaults(),
            ));
        }
        let backend = Backend {
            fs,
            proc: base.proc.clone(),
        };
        // `ACP-156` — the SAME handle the tool registry gets, retained on the services so a
        // front-end diffing a file mutation reads through this session's confinement rather than
        // around it. See `AgentSessionServices::fs`.
        let services_fs = Arc::clone(&backend.fs);
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
        // A modelless session (SEAM-075) has no declared modalities to seed from; `read` then keeps
        // its `supports_images_now()` default until the first `/model` re-pushes the real value.
        let read_model_vision = cyrup_tools::config::ModelVisionHandle::new(
            resolved_model
                .as_ref()
                .is_none_or(cyrup_provider::Model::supports_image_input),
        );
        // …and the twin handle for the resize PROFILE (PROV-134's read-tool clause / CFG-085's
        // Verify). Pi reads `ctx?.model?.inputLimits?.images?.resize` per call (`read.ts:138`), so
        // this must track `/model` the same way the vision bool does; seeded here from the resolved
        // model's row and re-pushed at every site below that re-pushes `read_model_vision`. Without
        // it `read` resolved `DEFAULT_IMAGE_RESIZE` for every model, so a row declaring a 512px
        // profile had no effect on the one tool whose whole job is putting images into history.
        let read_model_resize = cyrup_tools::config::ModelResizeHandle::new(
            resolved_model
                .as_ref()
                .and_then(cyrup_provider::Model::image_resize_profile),
        );
        let bash_session_env =
            cyrup_tools::config::SessionEnvHandle::new(cyrup_tools::config::SessionEnvInfo {
                session_id: Some(session_id.to_string()),
                // `None` for an ephemeral/in-memory session — Pi leaves `PI_SESSION_FILE` unset
                // rather than empty in that case (bash.ts:173-174).
                session_file: manager.session_file().map(std::path::Path::to_path_buf),
                // Likewise `None` while the session has no model: pi resolves `CYRUP_PROVIDER`/
                // `CYRUP_MODEL` from `ctx.model` per command (bash.ts:171-181), so a modelless
                // session leaves them unset rather than exporting a placeholder.
                provider: model_ref.as_ref().map(|m| m.provider.to_string()),
                model: model_ref.as_ref().map(|m| m.model.to_string()),
                reasoning_level: Some(thinking_level_to_str(thinking)),
            });
        let registry = ToolRegistry::with_builtins(
            cwd.clone(),
            backend,
            ToolsOptions {
                read: cyrup_tools::config::ReadOpts {
                    model_vision: Some(read_model_vision.clone()),
                    model_resize: Some(read_model_resize.clone()),
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
                // The `powershell` tool shares `resolveSpawnContext` with `bash` (bash.ts:341), so
                // it gets the same live session handle and the same managed `<agent_dir>/bin` on
                // PATH. It does NOT get `shellPath` or `shellCommandPrefix`: `PowerShellOpts` has
                // no such fields, because `createLocalPowerShellOperations()` takes no options
                // (powershell.ts:32-33) and the `shellPath` setting names a bash.
                powershell: cyrup_tools::config::PowerShellOpts {
                    session_env: Some(bash_session_env.clone()),
                    bin_dir: Some(cfg.agent_dir.join("bin")),
                    ..cyrup_tools::config::PowerShellOpts::default()
                },
                ..ToolsOptions::default()
            },
        );
        let visible = registry.visible(&cfg.tool_availability);
        // Tool-set selection (Pi sdk.ts:256-263): an explicit `tools` allowlist, or `noTools`
        // ("all" ⇒ none; "builtin" ⇒ drop the default built-ins), then minus the `excludeTools`
        // denylist. Absent all three, the initial built-in selection is the `defaultTools` setting
        // when configured (CFG-079, `settingsManager.getDefaultTools()` at sdk.ts:257) and pi's four
        // `defaultActiveToolNames` otherwise.
        // CFG-097 — already RESOLVED by the getter: plain names have replaced the baseline and
        // every `+name`/`-name` has been applied (`EffectiveSettings::default_tools`, pi
        // `getDefaultTools` → `resolveDefaultTools`). `select_active_tools` therefore still sees a
        // literal list of tool names, which is what upstream's `sdk.ts:261-263` sees too.
        //
        // SEAM-148: a `+name`/`-name`-only `tools` list is applied to that selection rather than
        // read as an allowlist (pi `sdk.ts:280-294` @f1b2e77f5); see [`ToolSelection`].
        let tool_selection = resolve_tool_selection(&cfg, settings.effective().default_tools());
        let configured_default_tools = tool_selection.default_tools.clone();
        let base_tools = select_active_tools(
            &visible,
            &tool_selection.cfg,
            configured_default_tools.as_deref(),
        );
        // pi `AgentSession._allowedToolNames` / `_excludedToolNames` (`sdk.ts:258-259` →
        // `agent-session.ts:395-396`). Resolved HERE, next to the built-in selection they are
        // derived from and ahead of BOTH consumers — `ext_host.active_tools_filtered` below and the
        // `AgentSessionServices` literal that carries them to `refresh_extension_tools` — so one
        // binding serves both and the two paths cannot drift.
        let allowed_tool_names = resolve_allowed_tool_names(&tool_selection.cfg);
        let excluded_tool_names: std::collections::HashSet<String> =
            cfg.exclude_tools.iter().cloned().collect();

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
        // Only when there IS one: a guest's `getModel()` reads `Option`-shaped state
        // (`host_services.rs`), matching pi's `ctx.model` being `undefined` on a modelless session.
        if let (Some(mr), Some(m)) = (model_ref.as_ref(), resolved_model.as_ref()) {
            host_services.update_model(
                mr.clone(),
                m.clone(),
                Some(thinking_level_to_str(thinking)),
            );
        }
        host_services.wire_control_channel();

        // ---- 4b. extension host (cyrup-ext) — built BEFORE resource discovery so the
        // `resources_discover` aggregate (extendResourcesFromExtensions, Pi agent-session.ts:2112)
        // can merge extension-contributed skill/prompt/theme paths into the registry the skill
        // pointers + system prompt are then derived from.
        let (mode, has_ui) = ext_mode(cfg.app_mode);
        let host_config = HostConfig {
            mode,
            has_ui,
            cwd: cwd.clone(),
        };
        // With `wasm-host`, spin up the Wasmtime engine so live wasm extensions can be loaded with
        // `LiveHostServices` injected (the seam below); otherwise a native-only host (the default).
        //
        // [CYRUP-DELTA] A runtime that cannot be constructed leaves the NATIVE extensions running
        // instead of ending session start (`ExtensionHost::with_wasm(..)?` used to). `codemode`,
        // `mcp`, `flux` and the other built-ins never need wasm, and the pre-trust pass above
        // already degrades this way (EXT-003). A wasm extension that is installed then fails to
        // load and is reported through `startup_diagnostics` like any other load failure
        // (`WasmHostDisabled`). pi has no wasm runtime to lose.
        #[cfg(feature = "wasm-host")]
        let host = {
            #[cfg(test)]
            let force_failure = self.force_runtime_wasm_failure;
            #[cfg(not(test))]
            let force_failure = false;
            wasm_host_or_native_only(host_config, force_failure)
        };
        #[cfg(not(feature = "wasm-host"))]
        let host = ExtensionHost::new(host_config);
        // Attach the live `getActiveTools` source BEFORE any tool set is materialized, so every tool
        // `active_tools` hands back is wrapped for `addedToolNames` derivation (Pi
        // `wrapRegisteredTool`, extensions/wrapper.ts:17-36). The source reads the SAME
        // `DynamicToolState` `setActiveTools` mutates, so a tool that widens the active set during its
        // own `execute` is observed by the wrapper's "after" snapshot. It reads `None` until
        // `attach_dynamic_tools` runs further down, which is correct: nothing executes before then.
        host.set_active_tool_source(host_services.clone());
        // PERM-011 half B: hand the backend the host's ONE `SharedBus`, so a NATIVE extension's
        // `HostServices::emit_event` lands in the same queue a WASM guest's `bus.emit` does and is
        // fanned out by the same drain. Done here because the ordering is forced — the host takes
        // this backend as an argument, so the bus cannot exist at `LiveHostServices::new` — and
        // BEFORE the native load loop below, so no extension can emit into an unattached bus.
        host_services.attach_event_bus(Arc::clone(host.bus()));
        // EXT-075: this backend is pi's shared `ExtensionUIContext`, so its prompts open the host's
        // UI-prompt window — the one a native's dialog goes through.
        host_services.attach_ui_prompts(Arc::clone(host.ui_prompts()));
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
        // The `codemode` tool's `prepare_loadout` hook reads `codemode.mode` / `codemode.inlineBudget`
        // while the loadout is resolved (below), long before the session is shared, so its host
        // exists from here with the settings this session was built with. `into_shared` attaches the
        // session itself. Pi reads the same two settings from `pi.getSettings()` at hook time
        // (`extensions/codemode/index.ts:22-29` @v1.0.1); a session's settings are fixed until
        // `/reload` rebuilds it, so reading them once is the same value.
        let codemode_host = self.codemode_host_slot.as_ref().map(|slot| {
            let eff = settings.effective();
            let host = Arc::new(crate::session::SessionCodemodeHost::new(
                crate::session::CodemodeSettings {
                    mode: eff.codemode_mode(),
                    inline_budget: eff.codemode_inline_budget(),
                },
            ));
            slot.bind(host.clone());
            host
        });
        // SEAM-071: `--no-extensions` gates the AMBIENT natives too. It used to gate only the
        // WASM/disk discovery roots (`extension_discovery_roots`), while this loop loaded every
        // native unconditionally — so `cyrup --no-extensions` still started an intercom broker,
        // which is how the suite accumulated 13 immortal broker processes per run. Upstream, the
        // analogs of these three are installed packages in `resolvedPaths.extensions`, and
        // `noExtensions` reduces THAT tier to the explicit `-e` paths alone
        // (`resource-loader.ts:451-452`, `:555-557` @v0.83.0). It does not touch pi's inline
        // `extensionFactories` tier, and neither does this — see `native_survives_no_extensions`.
        let is_subagent_child = std::env::var_os(SUBAGENT_CHILD_ENV).is_some();
        let keeps_codemode = child_keeps_codemode(&cfg);
        let codemode_available = native_extensions
            .iter()
            .any(|e| e.id().as_str() == cyrup_codemode_runtime::EXTENSION_ID);
        let mut codemode_loaded = false;
        for ext in natives_to_load(
            native_extensions,
            cfg.no_extensions,
            is_subagent_child,
            keeps_codemode,
        ) {
            let id = ext.id();
            codemode_loaded |= id.as_str() == cyrup_codemode_runtime::EXTENSION_ID;
            if let Err(e) = host
                .load_native_with_services(ext, native_services.clone())
                .await
            {
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
        native_load_errors.extend(codemode_skipped_diagnostic(
            &cfg,
            codemode_available && !codemode_loaded,
        ));
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
        let (configured_packages, package_errors) =
            configured_packages_from_settings(&settings, &cwd, &cfg.agent_dir);
        disc.configured_packages = configured_packages;
        // CFG-003: pi's session path calls `packageManager.resolve()` with NO `onMissing`
        // (resource-loader.ts:403 and :549 @v0.83.0), so a declared git package with no working tree
        // is CLONED during assembly unless `isOfflineModeEnabled()` (package-manager.ts:1260-1271).
        // The flag is threaded rather than read here because `cyrup-resources` performs no env
        // lookups and this crate has no `NetworkPolicy`; the bin resolves `--offline`/`CYRUP_OFFLINE`
        // and sets it (`cyrup/src/main.rs`).
        disc.install_missing_packages = cfg.install_missing_packages;
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
            // CFG-088: every load this manager made (the pre-trust global read and the trusted
            // reload) — duplicates are the bin's to collapse, as pi's `deduplicateDiagnostics`
            // does over the merged startup + runtime list.
            settings: settings
                .drain_load_errors()
                .iter()
                .map(cyrup_config::ScopedError::diagnostic_message)
                .collect(),
            ..Default::default()
        };
        // A malformed `packages` entry never takes the settings document (or the session) down; it
        // is reported alongside the discovery diagnostics.
        for message in package_errors {
            startup_diagnostics
                .resources
                .push(cyrup_resources::ResourceDiagnostic::error(
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
        startup_diagnostics.models.extend(model_file_error);
        // The runtime pi.dev catalog overlay slot (DRIFT-007 + XAI_3, FINDING 3). An injected
        // service's slot is SHARED with whatever else refreshes through that service (the binary's
        // background trigger, this session's own `/model` refresh) — that sharing is the whole fix.
        // With no service, the slot is fresh and disk-only: the cache-only restore Pi performs at
        // `agent-session-services.ts:180` (`refresh({ allowNetwork: false })`) — a session build
        // must never block on a network call, and an offline run must still see the catalogs it saw
        // last time.
        let catalog_overlay: Arc<cyrup_provider::CatalogOverlaySlot> =
            match &self.model_catalog_service {
                Some(svc) => svc.overlay(),
                None => Arc::new(cyrup_provider::CatalogOverlaySlot::with_overlay(
                    Self::load_persisted_catalog_overlay(&cfg.agent_dir).await,
                )),
            };
        // Surface composition errors (a provider block Pi would `throw` on) once, at startup, rather
        // than on every catalog read.
        {
            let base = cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
                credentials: None,
                auth_context: None,
                catalog_overlay: catalog_overlay.load(),
            })
            .get_models(None);
            let (_, errors) = model_file.compose(&base);
            startup_diagnostics.models.extend(errors);
        }
        // `model_file` is still borrowed by the 3b re-resolution below, so the shared handle is a
        // clone rather than a move.
        let model_config = Arc::new(model_file.clone());

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
        // The `-pattern` half of the settings `extensions` arrays, resolved by the discovery pass
        // above and honoured here — the ONE production site that turns `DiscoveryRoots` into loaded
        // extensions, and therefore the only place a `cyrup config` disable can actually bite
        // (Pi `toggleTopLevelResource`, config-selector.ts:532-578 → `setExtensionPaths`).
        ext_roots.disabled = disabled_loose_extensions(&report.registry);
        // SEAM-071, second half: the package tier is pi's `enabledExtensions`, the exact operand
        // `noExtensions` drops (`extensionPaths = this.noExtensions ? cliEnabledExtensions :
        // this.mergePaths(cliEnabledExtensions, enabledExtensions)`, resource-loader.ts:451-452
        // @v0.83.0). Appending it into `configured` unconditionally re-admitted every installed
        // package's extension through the one tier `--no-extensions` is defined to keep, so the flag
        // silently did not mean what it says. `cfg.extra_extension_paths` — the real `-e` tier — is
        // still merged by `extension_discovery_roots` either way.
        if !cfg.no_extensions {
            ext_roots
                .configured
                .extend(report.registry.ext_crate_paths.iter().cloned());
        }
        #[cfg(feature = "wasm-host")]
        {
            // Inject the session's OWN `host_services` (built at 4a) so a disk-discovered guest's
            // `control` capability reaches the same queue `apply_pending_control` drains.
            let host_services_for_load: Arc<dyn cyrup_ext::host::HostServices> =
                host_services.clone();
            // The per-path `errors` (Pi `LoadExtensionsResult.errors` → "Failed to load extension"
            // diagnostics, main.ts:679-682) are retained on `startup_diagnostics` so the TUI can
            // render Pi's `[Extension issues]` block (TUI-006) instead of dropping them here. Each
            // carries its `fatal` flag through unchanged, so a genuine load fault also reaches the
            // bin's exit-1 checkpoint while the project-trust skip does not (`LoadError::fatal`).
            let load_result = ext_host
                .discover_and_load(&ext_roots, trusted, host_services_for_load)
                .await;
            startup_diagnostics
                .extensions
                .extend(load_result.errors.iter().map(|e| {
                    crate::services::ExtensionLoadDiagnostic {
                        path: e.path.clone(),
                        error: e.error.clone(),
                        fatal: e.fatal,
                    }
                }));
        }
        #[cfg(not(feature = "wasm-host"))]
        let _ = &ext_roots;

        // Every replaceable built-in another extension displaced is reported as a warning
        // (`omitReplacedExtensions` pushes one onto the load result per left-out built-in,
        // `resource-loader.ts:139-150` @v1.0.1). Not fatal: pi keeps the session and the
        // replacement, and only the collisions it does NOT resolve exit 1.
        startup_diagnostics
            .extensions
            .extend(ext_host.omitted_extensions().iter().map(|omitted| {
                crate::services::ExtensionLoadDiagnostic {
                    path: PathBuf::from(omitted.extension.as_str()),
                    error: omitted.warning(),
                    fatal: false,
                }
            }));

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
            startup_diagnostics
                .flags
                .extend(ext_host.apply_extension_flag_values(&overrides)?);
        }

        // Bind the shared model-registry sink and FLUSH any provider registrations queued while native
        // + disk extensions loaded (Pi `runner.bindCore` pending-flush, runner.ts:345-362). The SAME
        // `Arc` is the `ext_host` sink (future `registerProvider`s upsert live) and the session's read
        // view (its catalog is UNIONed into the model registry, and its provider installed on select).
        let guest_providers = Arc::new(crate::guest_providers::GuestProviderRegistry::new());
        // pi `modelNetworkEnabled`: what a refresh that does not say `allowNetwork` gets. Set before
        // anything can refresh, so an offline run never reaches the network by leaving it unsaid.
        guest_providers.set_network_enabled(cfg.model_network_enabled);
        // EXT-027 — what a native provider's catalog refresh needs, all of it the session's own
        // (pi's `ModelRuntime` holds the same four as constructor arguments, `modelsStore`,
        // `credentials`, `authContext` and the registry behind `ctx.modelRegistry`,
        // `core/model-runtime.ts`). None of it was attached by anything but tests, so a production
        // session never read or wrote `models-store.json`, resolved no credential for the refresh
        // engine, and answered `/llama`'s `provider_auth` with "nothing" and its `refresh_provider`
        // with "no refresh backend". Attached BEFORE the bind and the restore below, so neither
        // runs against the process-local defaults.
        guest_providers.attach_models_store(Arc::new(
            cyrup_config::models_store::FileModelsStore::new(
                cfg.agent_dir
                    .join(cyrup_config::models_store::MODELS_STORE_FILE_NAME),
            ),
        ));
        guest_providers.attach_refresh_auth(
            auth.clone(),
            Arc::new(cyrup_config::login::StoreAuthContext(auth.clone())),
        );
        host_services.attach_provider_refresher(guest_providers.clone());
        // EXT-093 — `HostServices::flag_value` answers from the extension host's registry (the CLI
        // overrides applied above, then the registered default). Weak: the host holds `host_services`.
        host_services.attach_flag_source(Arc::downgrade(&ext_host));
        host_services.attach_provider_auth(auth.clone(), guest_providers.clone());
        // SEAM-144 — the cache-only whole-registry refresh pi fires from BOTH virtual-model
        // mutators (`model-runtime.ts:975`, `:984` @f1b2e77f5). Installed before the flush below, so the
        // extensions' own registrations fire it too, exactly as upstream's do.
        virtual_models.set_listener(Arc::new(
            crate::virtual_models::CachedRestoreOnVirtualChange::new(
                &guest_providers,
                cancel.token().clone(),
            ),
        ));
        // The catalog the flush validates against. pi drains `pendingVirtualModelRegistrations`
        // into a `modelRuntime` that already holds every provider's catalog
        // (`agent-session-services.ts:182-194`), so the physical-conflict check at
        // `model-runtime.ts:957` sees the whole catalog; this is the builder's equivalent, unioned
        // from the same three sources `AgentSession::compose_model_registry` uses and then composed
        // through `models.json`. No network and no disk: `default_models` is compiled in, and
        // `model_config` was already read at startup.
        let virtual_flush_catalog: Arc<dyn cyrup_provider::VirtualModelCatalog> = {
            let mut base: Vec<cyrup_provider::Model> = initial_provider.models().to_vec();
            for m in guest_providers.models() {
                if !base
                    .iter()
                    .any(|e| e.provider == m.provider && e.id == m.id)
                {
                    base.push(m);
                }
            }
            for m in cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
                credentials: None,
                auth_context: None,
                catalog_overlay: None,
            })
            .get_models(None)
            {
                if !base
                    .iter()
                    .any(|e| e.provider == m.provider && e.id == m.id)
                {
                    base.push(m);
                }
            }
            let (composed, _errors) = model_config.compose(&base);
            Arc::new(crate::virtual_models::StartupVirtualCatalog::new(composed))
        };
        guest_providers.attach_virtual_models(virtual_models.clone(), virtual_flush_catalog);
        // Bind, flushing BOTH queues (pi `bindCore`'s two pending loops, `runner.ts:485-514`). A
        // virtual-model registration the registry refuses becomes a startup diagnostic in pi's own
        // shape and does NOT fail the load — `agent-session-services.ts:185-191` pushes
        // `Extension "{path}" error: {message}` and carries on. Non-fatal, like the
        // omitted-extension warnings above: pi keeps the session.
        for failure in ext_host
            .registry()
            .bind_model_registry(guest_providers.clone())?
        {
            startup_diagnostics
                .extensions
                .push(crate::services::ExtensionLoadDiagnostic {
                    path: PathBuf::from(failure.extension_path.clone()),
                    error: failure.diagnostic(),
                    fatal: false,
                });
        }
        // The startup restore: every provider's PERSISTED catalog is handed to its `refresh_models`
        // with no network, now that the extensions' providers are registered (pi
        // `await modelRuntime.refresh({ allowNetwork: false })`, `agent-session-services.ts:206`
        // @v0.99.2-17). A single local file read; the network refresh a mode performs afterwards is
        // the caller's (`main.ts:931-936` for rpc, the interactive host's `run()`). Without it a
        // configured install lists no extension-provider models until something refreshes.
        // Failures stay inside the engine's own result: a broken cache is a cold one, never a
        // failed start.
        {
            // Bounded: this runs inline in `build`, and a provider that does slow work in a phase it
            // was told has no network must not stall session construction. On the deadline the
            // engine is cancelled and the session starts with what was restored.
            let restore_cancel = cancel.token().child_token();
            let restore = guest_providers.restore_cached(restore_cancel.clone());
            tokio::pin!(restore);
            tokio::select! {
                _ = &mut restore => {}
                () = tokio::time::sleep(cfg.provider_restore_timeout) => {
                    restore_cancel.cancel();
                    let _ = restore.await;
                }
            }
        }
        // 3b. A `--model` set aside at step 3, resolved against the extension providers now
        // registered and restored. The model is looked up across their catalogs like `/model`
        // does (`AgentSession::set_model`); the provider owning it becomes the session's provider,
        // and the seeds taken from the step-3 resolution are replaced with its values.
        if let Some(pattern) = deferred_model_pattern {
            let owner = {
                let candidates = guest_providers.models();
                cyrup_config::ModelResolver::new(&candidates)
                    .parse_pattern(&pattern, true)
                    .model
                    .and_then(|model| guest_providers.provider(model.provider.as_str()))
            }
            .or_else(|| {
                // A custom id on a provider whose catalog does not list it
                // (`resolveCliModel`'s fallback, `model-resolver.ts:475-501`).
                pattern
                    .split_once('/')
                    .and_then(|(prefix, _)| guest_providers.provider(prefix))
            });
            let Some(owner) = owner else {
                // Nothing registered the provider either: report what the launch path used to say
                // for a provider nothing knows.
                let unknown = pattern.split_once('/').and_then(|(prefix, _)| {
                    self.provider_resolver
                        .as_ref()
                        .and_then(|resolver| resolver.resolve(prefix).err())
                });
                return Err(SessionServiceError::ModelNotFound(match unknown {
                    Some(message) => format!("{pattern}: {message}"),
                    None => pattern,
                }));
            };
            let (model, reference, level, fallback) =
                resolve_model(&*owner, &cfg, &settings, &restore, &model_file)?;
            read_model_vision.set(
                model
                    .as_ref()
                    .is_none_or(cyrup_provider::Model::supports_image_input),
            );
            read_model_resize.set(
                model
                    .as_ref()
                    .and_then(cyrup_provider::Model::image_resize_profile),
            );
            if let Some(reference) = reference.as_ref() {
                bash_session_env
                    .set_model(reference.provider.to_string(), reference.model.to_string());
            }
            bash_session_env.set_reasoning_level(thinking_level_to_str(level));
            if let (Some(reference), Some(model)) = (reference.as_ref(), model.as_ref()) {
                host_services.update_model(
                    reference.clone(),
                    model.clone(),
                    Some(thinking_level_to_str(level)),
                );
            }
            (resolved_model, model_ref, thinking, model_fallback_message) =
                (model, reference, level, fallback);
            initial_provider = owner;
        }
        // 3c. `SESS-067`, `[CYRUP-DELTA]` in SHAPE only — re-ask the branch selection once the
        // EXTENSION-registered virtual models exist.
        //
        // Upstream has no counterpart because it does not need one: pi drains
        // `pendingVirtualModelRegistrations` inside `createAgentSessionServices`
        // (`agent-session-services.ts:182-194`), which runs BEFORE `createAgentSession` performs the
        // restore, so by the time `getBranchSelection` is called every extension's router is already
        // registered. cyrup resolves the model at step 3, long before the extension host is built
        // (`:1271`-ish) or loaded, so an extension that registers `router/auto` at init cannot be
        // seen there and a resumed session would come up on the physical model that answered.
        // Upstream's own test for this shape — `restores a virtual selection registered right
        // before the session opens` (`test/virtual-models.test.ts:297-317`) — uses a branch whose
        // last entry is a RESPONSE, so the answer turns on the hold rule and therefore on the
        // registry.
        //
        // This is the same deferral step 3b already performs for a `--model` naming an extension
        // provider (EXT-027, documented at the step-3 comment), run for the same reason and in the
        // same place. Three properties matter:
        //
        // * The branch is RE-WALKED rather than reusing step 3's `session_selection`. That value
        //   was computed against an empty registry, so for the hold-rule shape it names the
        //   physical response and carries no trace of the router at all.
        // * Only a VIRTUAL selection is reconsidered. A physical one was fully resolvable at step 3
        //   against `provider.models()`, and re-adopting it here would re-litigate the
        //   `--model`/settings/catalog precedence step 3 already settled.
        // * An empty registry short-circuits before the walk, so a session with no virtual models
        //   does exactly what it did before this block existed — no extra branch walk, no state
        //   touched.
        //
        // Nothing installs a provider for the adopted model: providers never see a virtual model.
        // The routing step installs the ROUTED model's owning provider per request
        // (`session/virtual_models.rs`), and `set_model_resolved` skips the install for a virtual
        // selection for the same reason.
        if cfg.model_pattern.is_none()
            && has_existing_session
            && !virtual_models.is_empty()
            && let Some(held) = manager.branch_selection(|p, id| virtual_models.model_ref(p, id))
            && let Some(virtual_model) =
                virtual_models.get(held.provider.as_str(), held.model.as_str())
            && resolved_model
                .as_ref()
                .is_none_or(|m| m.provider != virtual_model.provider || m.id != virtual_model.id)
        {
            thinking = cyrup_provider::clamp_thinking_level(&virtual_model, thinking);
            model_ref = Some(ModelRef {
                provider: virtual_model.provider.clone(),
                api: Some(virtual_model.api.clone()),
                model: virtual_model.id.clone(),
            });
            // Step 3 may have reported a selection as unrestorable that IS restorable after all.
            model_fallback_message = None;
            // 3b's own side effects, for the same reason it runs them: `read`'s non-vision warning
            // and the `bash` child's `CYRUP_MODEL`/`CYRUP_REASONING_LEVEL` must describe the model
            // the session actually starts on.
            read_model_vision.set(virtual_model.supports_image_input());
            read_model_resize.set(virtual_model.image_resize_profile());
            bash_session_env.set_model(
                virtual_model.provider.to_string(),
                virtual_model.id.to_string(),
            );
            bash_session_env.set_reasoning_level(thinking_level_to_str(thinking));
            if let Some(reference) = model_ref.as_ref() {
                host_services.update_model(
                    reference.clone(),
                    virtual_model.clone(),
                    Some(thinking_level_to_str(thinking)),
                );
            }
            resolved_model = Some(virtual_model);
        }
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
            let mut skill_paths: Vec<PathBuf> = agg
                .skill_paths
                .iter()
                .map(|p| PathBuf::from(&p.path))
                .collect();
            let mut prompt_paths: Vec<PathBuf> = agg
                .prompt_paths
                .iter()
                .map(|p| PathBuf::from(&p.path))
                .collect();
            let mut theme_paths: Vec<PathBuf> = agg
                .theme_paths
                .iter()
                .map(|p| PathBuf::from(&p.path))
                .collect();
            skill_paths.extend(cfg.extra_skill_paths.iter().cloned());
            prompt_paths.extend(cfg.extra_prompt_paths.iter().cloned());
            theme_paths.extend(cfg.extra_theme_paths.iter().cloned());
            if skill_paths.is_empty() && prompt_paths.is_empty() && theme_paths.is_empty() {
                report.registry
            } else {
                let extra = cyrup_resources::DiscoveredPaths {
                    skill_paths,
                    prompt_paths,
                    theme_paths,
                };
                report.registry.extend(&extra)
            }
        };
        let resources = Arc::new(resources);
        // Skill pointers are loaded whatever the tool set, as pi's `resourceLoader.getSkills()` is:
        // whether the prompt advertises them is decided per build by the tool that can read a
        // skill file (`read`, else `bash` — `system-prompt.ts:46` @v0.85.0, SESS-059), and the
        // active set can change after this point.
        let mut skills: Vec<SkillPointer> = if !cfg.no_skills {
            resources.skills.winners().map(|s| s.pointer()).collect()
        } else {
            Vec::new()
        };
        // Synthetic-skill injection (Pi `skillsOverride`, resource-loader.ts:630): transform the
        // discovered pointer set before it feeds the context snapshot + system prompt. Applied to the
        // (possibly-empty) base so an embedder can inject skills discovery found none of; the emit is
        // still gated downstream (`SystemPromptBuilder::build`), matching Pi.
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
            .reload(
                &cancel,
                loader,
                Arc::from(skills),
                ResolvedOverride::default(),
            )
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

        // The extension-shaped active tool set (Pi `pi.getActiveTools()` after the extension
        // `active_tools` merge): base build-time selection PLUS any extension additions/overrides
        // (e.g. a native extension that overrides a built-in `bash`).
        //
        // EXT-038 — this used to be computed AFTER the prompt was built, so the prompt was derived
        // from `base_tools` alone. Upstream builds `_toolPromptSnippets` / `_toolPromptGuidelines`
        // from `definitionRegistry`, which is the base definitions with `allCustomTools` (the
        // extension-contributed ones) merged over them (`agent-session.ts:2471-2504` @v0.83.0) —
        // so an extension tool's `promptSnippet`/`promptGuidelines` DO reach the system prompt, and
        // an extension OVERRIDE of a built-in contributes the override's text, not the built-in's.
        // In cyrup neither happened: a guest could register a fully-described tool and the model
        // was never told it existed.
        // #2835: FILTERED. `--tools`/`--no-tools` must bound the extension-contributed tools too,
        // not only the built-ins already selected into `base_tools` (pi applies `isAllowedTool` to
        // `allCustomTools`, `agent-session.ts:2680-2686`). One filter here is enough for BOTH the
        // model's tool array and the system prompt: `prompt_tools` below starts from the
        // already-filtered `base_tools` and can only GROW by entries drawn from this value.
        let active_tools = ext_host.active_tools_filtered(
            &base_tools,
            allowed_tool_names.as_ref(),
            &excluded_tool_names,
        )?;

        // The REGISTRY (pi `_toolRegistry`): the base selection with extension overrides, plus every
        // allowed extension tool whatever its exposure — a `codemode`, `deferred` or `hidden` tool
        // is registered without being active (pi `_refreshToolRegistry`, `agent-session.ts:3445-3545`
        // @v1.0.1). `active_tools` above is the subset registration activates.
        let registered_tools = ext_host.registered_tools_filtered(
            &base_tools,
            allowed_tool_names.as_ref(),
            &excluded_tool_names,
        )?;
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
        // Each SDK custom tool is also the SDK half of upstream's renderer map: `allCustomTools`
        // spreads `this._customTools` into the very map `getToolDefinition(name)` reads, so a
        // custom tool's own `renderCall`/`renderResult` reaches the transcript
        // (`core/agent-session.ts:2471-2495`, resolved at
        // `modes/interactive/components/tool-execution.ts:83-101` @v0.83.0). Registering here is
        // what gives `Tool::render_call`/`Tool::render_result` a reader at all — before this they
        // were overridable methods nothing in the workspace ever called, so a custom tool that
        // supplied its own rendering had it silently discarded and drew the generic shell.
        // Registered UNWRAPPED: the renderer belongs to the tool the caller configured, and
        // `wrap_tool`'s active-set diffing has nothing to add to a pure render call (the wrapper
        // delegates both methods through anyway, `cyrup-ext/src/wrapper.rs`).
        for tool in &cfg.custom_tools {
            ext_host.register_native_tool_renderer(tool.clone());
        }
        registry_tools.extend(
            cfg.custom_tools
                .iter()
                .map(|t| ext_host.wrap_tool(t.clone())),
        );
        registry_tools.extend(registered_tools.iter().cloned());
        // The initial loadout: the active names resolved against the registry, which is what pi's
        // `_buildRuntime` → `_refreshToolRegistry` → `_applyToolLoadout` computes before the first
        // prompt is built. It decides the system prompt's tool list (a tool whose declaration a
        // `prepare_loadout` hook hides is not listed, `agent-session.ts:1655-1658`) and is what the
        // agent starts with.
        //
        // A session whose caller did not choose its tools resumes with the loadout its transcript
        // declares (pi `_restoreToolsFromTranscript`, `agent-session.ts:1762-1769` @v1.0.1, taken
        // at construction when no `initialActiveToolNames` was given, `:497`): what a `tool_search`
        // load, a `setActiveTools` call or an extension registration recorded survives the
        // process. A transcript with no system message declares nothing, and the selection above
        // stands. The names the session may not expose are dropped (`_isAllowedTool`).
        //
        // `[CYRUP-DELTA]` this construction-time restore is cyrup's own rule, not pi's: pi's
        // `createAgentSession` always passes `initialActiveToolNames` (`sdk.ts:294` @f1b2e77f5), so
        // `agent-session.ts:525`'s `=== undefined` restore never runs for an SDK-built session.
        //
        // SEAM-148: a `+name`/`-name` `tools` list is applied on every start, so a `--continue` or
        // `--resume` launch with `--tools -bash` starts without `bash` whatever the transcript
        // saved (pi `sdk.ts:294` sets `initialActiveToolNames` from `applyToolModifiers`, and
        // `agent-session.ts:525` restores only when it is `undefined`). The one exception is the
        // `/reload` rebuild: pi's `reload` keeps the live active set (`:3682-3686`), which this
        // Resume rebuild reproduces by restoring the transcript's loadout, so a tool turned off
        // stays off and a `tool_search` load survives.
        let uses_default_tools = tool_selection.uses_default_tools(&cfg);
        let default_tool_modifiers = tool_selection.modifiers.clone();
        let restores_loadout =
            uses_default_tools && (default_tool_modifiers.is_empty() || self.reload_rebuild);
        let restored_loadout: Option<Vec<String>> = if restores_loadout {
            crate::tools::declared_tool_names(&existing_raw).map(|names| {
                names
                    .into_iter()
                    .filter(|name| {
                        crate::tools::is_allowed_tool(
                            allowed_tool_names.as_ref(),
                            &excluded_tool_names,
                            name,
                        )
                    })
                    .collect()
            })
        } else {
            None
        };
        //
        // `defaultTools` names that are not built-ins: `select_active_tools` above only
        // walks the built-in registry, so `codemode`, `tool_search` and every other tool an
        // extension registered never saw the configured names. Pi hands ONE list of initial names
        // to the session and activates each registered tool the list names
        // (`sdk.ts:282-294` → `agent-session.ts:3566-3592` @v1.1.0); this is that second half,
        // against the registry the extensions just filled. Only the default arm applies it: an
        // explicit `--tools` list, `--no-tools` or `--no-builtin-tools` replaces the configured
        // list, and a `+name`/`-name` `tools` list has been applied to it already (SEAM-148).
        let default_plan = {
            // The selection pi seeds the session with: once the `+name`/`-name` modifiers were
            // applied to the defaults it carries neither `tools` nor `noTools`, which also holds
            // for `noTools: "builtin"` with modifiers (their base is empty, they fill it), so this
            // is not `uses_default_tools` (pi's `usesDefaultTools` is false there).
            let names_apply =
                tool_selection.cfg.tools.is_none() && tool_selection.cfg.no_tools.is_none();
            let configured = configured_default_tools.clone().unwrap_or_default();
            // What counts as "already offered". The `/reload` rebuild offered every configured
            // name: the names the setting newly adds are activated after it, against the replaced
            // session's own `defaultTools` (`AgentSession::activate_added_default_tools`, pi's
            // `previousDefaultTools`), and a name turned off since stays off. A session restored
            // from its transcript elsewhere has no earlier load to ask, so it is what the
            // transcript's first system message declared (see [`crate::default_tools`]).
            let started_with = if self.reload_rebuild {
                Some(configured.clone())
            } else {
                restored_loadout
                    .as_ref()
                    .and_then(|_| crate::default_tools::declared_at_start(&existing_raw))
            };
            crate::default_tools::plan(
                &crate::default_tools::DefaultToolInputs {
                    names_apply,
                    configured: &configured,
                    excluded: &excluded_tool_names,
                    started_with: started_with.as_deref(),
                },
                &registry_tools,
            )
        };
        if let Some(warning) = default_plan.warning() {
            startup_diagnostics.settings.push(warning);
        }
        let initial_names: Vec<String> = {
            let mut names: Vec<String> = restored_loadout
                .clone()
                .unwrap_or_else(|| active_tools.iter().map(|t| t.name().to_string()).collect());
            for name in &default_plan.activate {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            names
        };
        let initial_loadout = cyrup_core::ToolLoadout::resolve(&initial_names, &registry_tools);
        let selected_tools: Vec<Arc<str>> = initial_loadout
            .executable()
            .iter()
            .map(|t| Arc::from(t.name()))
            .collect();
        // The tool list, the rules and the skills hint follow the DECLARED tools, so the hidden
        // ones are handed to the builder rather than blanked here (pi `hiddenTools`,
        // `system-prompt.ts:16-20` @v1.0.4, CODE-020).
        let tool_contributions: Vec<ToolPromptContribution> = initial_loadout
            .executable()
            .iter()
            .map(tool_contribution)
            .collect();
        let hidden_tools: Vec<Arc<str>> = initial_loadout
            .hidden_declarations()
            .iter()
            .map(|name| Arc::from(name.as_str()))
            .collect();

        // CFG-035 — `SYSTEM.md` / `APPEND_SYSTEM.md` discovery. Pi's `load()` resolves
        // `this.systemPromptSource ?? this.discoverSystemPromptFile()` and, for the append leg,
        // uses the CLI sources when present and otherwise the SINGLE discovered file
        // (`resource-loader.ts:525,531-535` @v0.83.0). Both discoverers are the same two rungs
        // (`:1022-1032`, `:1034-1044`): the project file under the trust gate, then the global one.
        //
        // Neither file was read at all before this: `.cyrup/SYSTEM.md` and `.cyrup/APPEND_SYSTEM.md`
        // were only TRUST-GATE MARKERS (`cyrup-config/src/trust.rs:208-209`), so cyrup asked the
        // user to trust a project because of a file it then never opened.
        //
        // The discoverer itself is `cyrup_resources`' — the SAME function whose result rides out on
        // `DiscoveryReport::{system_prompt_file, append_system_prompt_file}`. A private second copy
        // lived here until this pass; two copies of one upstream function is the `encode_cwd`
        // duplication hazard (two drift-free copies that were both wrong), and here the dead copy
        // was the one covered by tests while the live one was not.
        let discovered_system_prompt = cfg
            .system_prompt
            .is_none()
            .then(|| cyrup_resources::discover_system_prompt_file(&cwd, &cfg.agent_dir, trusted))
            .flatten()
            .and_then(|p| read_discovered_prompt(&p, "system prompt"));
        // pi REPLACES rather than accumulates: `let appendSources = this.appendSystemPromptSource;
        // if (!appendSources) { …discovered… }` (`:531-535`) — a CLI `--append-system-prompt` means
        // the discovered file is not consulted, and discovery itself yields exactly ONE path.
        let discovered_append = cfg
            .append_system_prompt
            .is_none()
            .then(|| {
                cyrup_resources::discover_append_system_prompt_file(&cwd, &cfg.agent_dir, trusted)
            })
            .flatten()
            .and_then(|p| read_discovered_prompt(&p, "append system prompt"));

        let prompt_inputs = PromptInputs {
            sections: Vec::new(),
            custom_prompt: cfg
                .system_prompt
                .clone()
                .or(discovered_system_prompt)
                .map(Arc::from),
            selected_tools: Some(selected_tools),
            hidden_tools,
            tool_contributions,
            prompt_guidelines: Vec::new(),
            append_system_prompt: cfg
                .append_system_prompt
                .clone()
                .or(discovered_append)
                .map(Arc::from),
            cwd: cwd.clone(),
            context_files: snapshot.context_files.clone(),
            skills: snapshot.skills.clone(),
            docs: DocsPointers::default(),
            today: today(),
        };
        // The prompt as the sections the transcript stores (CODE-014): the session writes them to its
        // file and diffs them at the start of every run, and the model is sent their replay. The
        // text is those same sections rendered, for the accessors and guests that read a string.
        let built_prompt = crate::tools::BuiltPrompt::new(
            SystemPromptBuilder::new().build_sections(&prompt_inputs),
        );
        let system_prompt = built_prompt.text().to_owned();

        let contributions: std::collections::BTreeMap<String, ToolPromptContribution> =
            registry_tools
                .iter()
                .map(|t| (t.name().to_string(), tool_contribution(t)))
                .collect();
        // The rebuilder base = the prompt inputs with the per-run tool fields cleared (re-derived
        // from the active set on each `setActiveToolsByName`).
        let mut rebuild_base = prompt_inputs.clone();
        // CLEARED, not "explicitly zero tools": since SESS-016 `None` means "unset — use pi's
        // `selectedTools || [read,bash,edit,write]` default" and `Some(vec![])` means "the caller
        // genuinely restricted the agent to no tools", which suppresses the skills section and every
        // tool guideline. This placeholder is overwritten on every call
        // (`PromptRebuilder::rebuild` assigns `inputs.selected_tools = Some(active…)`, tools.rs:61),
        // so the value is never observed — but it must not READ as the restricted case.
        rebuild_base.selected_tools = None;
        rebuild_base.hidden_tools = Vec::new();
        rebuild_base.tool_contributions = Vec::new();
        // Shared with `host_services` so a loaded guest's `setActiveTools`/`getActiveTools`
        // capability read+mutates the SAME authoritative active-tool view the host/CLI toggle uses
        // (Pi binds both to `agent.state.tools`, agent-session.ts:2281,2283).
        //
        // The active set is seeded from `active_tools` — the MERGED set, which is the very array
        // `.tools(active_tools)` hands the agent below — and NOT from `base_tools`. The two differ
        // by exactly the extension-contributed tools, and `DynamicToolState::set_active` rebuilds
        // `agent.state.tools` from `self.active` alone, with no second merge anywhere. Seeded from
        // `base_tools`, the first live re-registration by ANY extension (`merge_registered` ->
        // `set_active(&self.active + new names)`) therefore rewrote the agent's array WITHOUT every
        // extension tool registered at `init` — silently evicting them for the rest of the session.
        // Pi has no such gap: its `nextActiveToolNames` starts from `getActiveToolNames()`, which
        // reads the live `agent.state.tools` (`core/agent-session.ts:2524-2545`), so an extension
        // tool present at build is present in the rebuild.
        let mut dynamic_tool_state = crate::tools::DynamicToolState::new(
            registry_tools,
            active_tools.clone(),
            crate::tools::PromptRebuilder::new(rebuild_base, contributions),
            // pi filters the base definitions and `allCustomTools` by `_isAllowedTool` before the
            // registry is built (`_refreshToolRegistry`, `agent-session.ts:3497-3536` @v1.0.4).
            // The registry here is built from every Availability-visible built-in, so it is the
            // registry's own constructor that applies the bounds: a built-in the flags do not
            // allow is never registered, which is what keeps the permission system's per-turn
            // `set_active_tools` over `all_tool_names`, `set_active_tools_by_name` and a
            // `codemode` script's `tools.*` from reaching it.
            crate::tools::ToolAccess::new(allowed_tool_names.clone(), excluded_tool_names.clone()),
        );
        // The same names `initial_loadout` was resolved from, now with their pending half: a
        // restored tool that registers after the build (an MCP server still connecting) becomes
        // active when it does.
        //
        // The configured names nothing has registered yet wait too ([CYRUP-DELTA] — see
        // `default_tools`): pi drops an unregistered default name, cyrup keeps it until a tool of
        // that name registers or the first run starts.
        if restored_loadout.is_some()
            || !default_plan.activate.is_empty()
            || !default_plan.pending.is_empty()
        {
            let mut pending = restored_loadout.clone().unwrap_or_default();
            pending.extend(default_plan.pending.iter().cloned());
            dynamic_tool_state.seed(&initial_names, &pending);
        }
        let dynamic_tools = Arc::new(std::sync::Mutex::new(dynamic_tool_state));
        host_services.attach_dynamic_tools(dynamic_tools.clone());
        // EXT-005: seed the guest-visible `ctx.getSystemPrompt()` / `ctx.isProjectTrusted()` reads
        // from the values this build resolved (Pi binds both straight to the session:
        // `getSystemPrompt: () => this.systemPrompt` and `isProjectTrusted: () =>
        // this.settingsManager.isProjectTrusted()`, agent-session.ts:2410,2434). Without this a
        // guest got the trait defaults — an empty prompt and a confident, wrong `false` for trust,
        // even in a project cyrup had just decided IS trusted.
        host_services.update_prompt_state(Some(system_prompt.clone()), settings.project_trusted());

        // ---- 7. seed the agent transcript from the resumed branch (R-04-011). The manager was
        // created at step 2b; `existing_raw` already holds its context.
        //
        // SEAM-112 (SESS-043 residual) — seeded from the RAW projection, roles intact. pi's build
        // path is `const existingSession = sessionManager.buildSessionContext();` (sdk.ts:190) then
        // `agent.state.messages = existingSession.messages;` (sdk.ts:374), and `buildSessionContext`
        // is `buildContextEntries(...).flatMap(sessionEntryToContextMessages)`
        // (session-manager.ts:461-469) — the projection BEFORE `convertToLlm`, whose
        // `sessionEntryToContextMessages` (`:383-407`) returns `custom` / `branchSummary` /
        // `compactionSummary` roles UNTOUCHED. `convertToLlm` is applied one layer out, at the
        // request boundary only (sdk.ts:301).
        //
        // Folding `build_context().messages` through `core_message_to_agent` produced a transcript
        // of a different LENGTH and different roles from pi's: `convertToLlm` DROPS every
        // `excludeFromContext` (`!!`) bash message and rewrites each summary into wrapper prose, so
        // pi's `messages.slice(0, -1)` arithmetic (`agent-session.ts:2008`, `:2188`, `:2703`) and
        // every agent-state token estimate ran over a different list. This was the LAST seed site
        // still on the flattened twin; the three re-seeds (`session.rs`'s `compact`,
        // `navigate_tree` and `run_auto_compaction`) already match pi's `agent-session.ts:1955`,
        // `:3206` and `:2280` respectively.
        let seed: Vec<cyrup_agent::AgentMessage> =
            existing_raw.iter().map(raw_message_to_agent).collect();

        // ---- 8. extension host seams (cyrup-ext) — the host itself was built at step 4b ---------
        // (`active_tools` was computed above, ahead of the dynamic-tool registry.)
        let session_cancel = CancelToken::new();
        let ext_subscriber = ext_host.subscriber();
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
        // The same hooks the agent runs with, for the calls tools make while they run (CODE-006).
        let nested_hooks: Arc<dyn cyrup_agent::Hooks> = policy_hooks.clone();
        let nested_calls = Arc::new(cyrup_agent::NestedToolCallRunner::new());
        let eff = settings.effective();
        // Provider attribution + opencode session headers (Pi sdk.ts:323-330, #20). Telemetry is the
        // env override (`CYRUP_TELEMETRY`) else the `enableInstallTelemetry` setting.
        let env = cyrup_config::EnvVars::from_process();
        let telemetry_enabled = env
            .telemetry
            .unwrap_or_else(|| eff.enable_install_telemetry());
        // No model ⇒ no provider to attribute to; the headers are recomputed on the first
        // `/model` anyway (`apply_model_change`).
        let attribution_headers = resolved_model.as_ref().and_then(|m| {
            crate::attribution::merge_provider_attribution_headers(
                m,
                telemetry_enabled,
                Some(&session_id),
                &[],
            )
        });
        // The swappable stream source the agent loop streams through: it wraps the resolved provider
        // and the (optional) resolver seam so a cross-provider `/model` select can install a new
        // provider in place without rebuilding the agent (Pi live model+provider switch). The SAME
        // `Arc` is handed to the agent (as its `StreamFn`) and to the session (to mutate on select).
        let provider_swap = Arc::new(ProviderSwap::new(
            initial_provider,
            self.provider_resolver.clone(),
        ));
        // A provider the extensions replace LATER under the id this slot holds (`/llama`, a catalog
        // refresh) is stored into the slot too, so the session streams through, lists and filters by
        // the replacement and not by the `Arc` it started on.
        guest_providers.follow_installed(&provider_swap);
        // Transport selection (Pi `AgentOptions.streamFn`, sdk.ts:301): an embedder-supplied custom
        // `StreamFn` (e.g. `ProxyStreamFn`) becomes THE transport the agent loop streams through;
        // absent one, the provider-backed `ProviderSwap` is used (the default live-swappable path).
        let agent_stream_fn: Arc<dyn cyrup_agent::StreamFn> = match custom_stream_fn {
            Some(f) => f,
            None => provider_swap.clone(),
        };
        // GAP-2 (UW-4/UW-5): hand the host-services backend the SAME live swap, so
        // `HostServices::registered_provider` — pi
        // `ctx.modelRegistry.getRegisteredProviderConfig(provider)` — answers with the provider the
        // loop is currently streaming through. Done here and not at `LiveHostServices::new` because
        // the ordering is forced: the swap wraps the provider that constructor already took.
        host_services.attach_provider_swap(Arc::clone(&provider_swap));
        let mut agent_builder = cyrup_agent::AgentBuilder::new(agent_stream_fn)
            // pi builds its `Agent` with `systemPrompt: ""` (`core/sdk.ts:389` @v1.0.0): the prompt is
            // not a field of the agent but the `sections` of the system rows in its transcript, which
            // the session writes at the start of each run. Sending a prompt of its own as well is how
            // a session file pi wrote came to carry its prompt twice.
            .thinking_level(thinking)
            .loadout(initial_loadout)
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
        // SEAM-075 — pi's agent holds `Model | undefined` (`AgentSession.model` is a straight read
        // of `this.agent.state.model`, agent-session.ts:890-892): a modelless session builds an
        // agent with NO model, and the first `/model` sets it through `agent.set_model`. Every
        // reader — the `/model` picker, the footer, `state_view`, the attribution headers, the
        // `CYRUP_*` env — reads the session's `Option<ModelRef>`, which stays `None` until then.
        if let Some(m) = model_ref.clone() {
            agent_builder = agent_builder.model(m);
        }
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
        // read is setting-only, mirroring Pi's `getGlobalSettings().httpProxy`; the ambient-wins half
        // of pi's `??=` lives in `node_http_proxy::get_proxy_env` (CFG-060, which deleted the
        // accessor's `EnvVars` argument — this call passed `EnvVars::default()` to defeat it).
        if let Some(overlay) = apply_http_proxy_settings(eff.http_proxy()) {
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

        // CFG-006 / AGENT-031 — `websocketConnectTimeoutMs`. pi's `streamFn` resolves it as
        // `options?.websocketConnectTimeoutMs ?? settingsManager.getWebSocketConnectTimeoutMs()`
        // (`core/sdk.ts:310-311` @v0.83.0) and spreads it onto every `streamSimple` call.
        //
        // Both halves existed and neither was connected: `Settings::websocket_connect_timeout_ms`
        // parsed and validated the key, `AgentBuilder::websocket_connect_timeout_ms` threaded it to
        // `StreamOptions` (`cyrup-provider/src/stream.rs:201`) — and NOTHING assigned it, so a user
        // who set the key got no error and no effect. Deliberately NOT applied in the retry block
        // above: it is a separate pi rung with its own settings getter, and (unlike `timeoutMs`) no
        // `retry.provider.*` value overrides it.
        //
        // A parse error is dropped exactly as the `http_idle_timeout_ms` rung above drops one — the
        // invalid-setting diagnostic is the settings layer's, not this builder's.
        if let Ok(Some(ms)) = eff.websocket_connect_timeout_ms() {
            agent_builder = agent_builder.websocket_connect_timeout_ms(ms);
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
                    if h.dispatcher()
                        .no_subscribers(EventKind::BeforeProviderRequest)
                    {
                        return None;
                    }
                    let out = h
                        .emit_before_provider_request(payload.clone(), &CancelToken::new())
                        .await;
                    (out != payload).then_some(out)
                })
            }));
            // PROV-042 — the `before_provider_headers` PRODUCER. Every other piece of this hook
            // already existed (the `EventKind`, the WIT export, the SDK `on_before_provider_headers`
            // registration, the in-place/`null`-deletes reducer at `cyrup-ext/src/contract.rs:187`),
            // but nothing in the tree ever constructed the event: an extension could subscribe and
            // would never be called, which is worse than a documented refusal. pi emits it from its
            // session `streamFn`'s `transformHeaders` closure
            // (`packages/coding-agent/src/core/sdk.ts:330-339` @v0.84.4 →
            // `ExtensionRunner.emitBeforeProviderHeaders`, `extensions/runner.ts:1100-1125`), which
            // is exactly this position.
            //
            // ORDERING. pi's closure merges `mergeProviderAttributionHeaders` first and hands the
            // result to the hook, so extensions see the attribution set already present and win over
            // it. cyrup reaches the same guarantee by a different route: attribution rides
            // `StreamOptions::headers` (AGENT-029, `session/model.rs:282` via
            // `Agent::set_header_fn`), and the provider merges that overlay into the assembled set
            // BEFORE running `transform_headers` (`cyrup-provider/src/stream.rs`
            // `apply_transform_headers`, called by every api impl after `build_headers`). Same two
            // facts, same order: attribution first, hook last, hook's return value on the wire.
            //
            // Gated on a live subscriber like its two siblings, so the common no-extension path
            // does no JSON round-trip at all.
            let h = ext_host.clone();
            agent_builder = agent_builder.transform_headers(Arc::new(move |headers| {
                let h = h.clone();
                Box::pin(async move {
                    if h.dispatcher()
                        .no_subscribers(EventKind::BeforeProviderHeaders)
                    {
                        return headers;
                    }
                    // `ProviderHeaders` upstream is `Record<string, string | null>` and cyrup's
                    // `HeaderMap` is `BTreeMap<String, Option<String>>` — the same shape, so the
                    // round-trip is total in both directions and a `null` survives as the `None`
                    // that suppresses the header at `stream/sse.rs:359`.
                    let payload = serde_json::Value::Object(
                        headers
                            .iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    v.clone().map_or(serde_json::Value::Null, |s| {
                                        serde_json::Value::String(s)
                                    }),
                                )
                            })
                            .collect(),
                    );
                    let out = h
                        .emit_before_provider_headers(payload, &CancelToken::new())
                        .await;
                    match out {
                        serde_json::Value::Object(map) => map
                            .into_iter()
                            .map(|(k, v)| match v {
                                serde_json::Value::Null => (k, None),
                                serde_json::Value::String(s) => (k, Some(s)),
                                // A handler that set a non-string value is coerced rather than
                                // dropped: JS would have stringified it on the way out.
                                other => (k, Some(other.to_string())),
                            })
                            .collect(),
                        // A handler that replaced the bag with a non-object is ignored (degrade,
                        // never panic) — pi ignores the return value entirely.
                        _ => headers,
                    }
                })
            }));
            let h = ext_host.clone();
            agent_builder = agent_builder.on_response(Arc::new(move |resp, _model| {
                let h = h.clone();
                Box::pin(async move {
                    if h.dispatcher()
                        .no_subscribers(EventKind::AfterProviderResponse)
                    {
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
            // Pi sdk.ts:370-373 guards this on the model existing —
            // `if (model) { sessionManager.appendModelChange(model.provider, model.id); }` —
            // while the thinking-level entry is appended unconditionally. A modelless session must
            // therefore persist NO `model_change` entry, so a later resume has nothing bogus to
            // restore from.
            if let Some(m) = resolved_model.as_ref() {
                manager.append_model_change(m.provider.clone(), m.id.clone())?;
            }
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

        // SEAM-131 — prompt-cache warming. Installed on the `ProviderSwap` the agent streams
        // through, which is cyrup's counterpart of the pi `streamFn` closure that holds
        // `cacheWarmer.start(...)` (`core/sdk.ts:403-412` @v1.0.4): the one session request site.
        // Built here and not beside the swap itself because the host it needs is this session tree
        // plus this fan-out, and both exist only now; nothing can stream through the swap before
        // the agent is handed out below.
        //
        // The decider is the `cache_warming_decision` extension hook (EXT-085), cyrup's
        // counterpart of pi's `decide` closure (`core/sdk.ts:316-321` @v1.0.4): every subscribed
        // extension's handler runs and the LAST readable `{action}` overrides the economics
        // verdict. With no extension subscribed the fold answers with pi's own action, so this is
        // [`crate::cache_warmer::PiDecision`]'s behaviour plus the hook — `PiDecision` remains the
        // default for a warmer built without an extension host (tests, embedders).
        let cache_warmer = crate::cache_warmer::CacheWarmer::new(
            Arc::new(crate::cache_warmer::SessionCacheWarmingHost::new(
                manager.clone(),
                fanout.clone(),
            )),
            settings.effective().cache_warming_mode(),
            Arc::new(crate::cache_warmer::ExtensionCacheWarmingDecider::new(
                &ext_host,
                session_cancel.clone(),
            )),
            session_cancel.clone(),
        );
        provider_swap.attach_cache_warming(cache_warmer, session_id.clone());

        // pi's `_agentRunAbortRequested`: ONE latch, read by the post-run driver of a bound session
        // and by the subscriber that settles an unbound one, so both report the same `aborted`.
        let run_abort_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Attach the extension notify seam, then the facade's persist+fan-out subscriber.
        agent.subscribe(ext_subscriber);
        agent.subscribe(Arc::new(SvcSubscriber::new(
            fanout.clone(),
            manager.clone(),
            handle.clone(),
            ext_host.clone(),
            session_cancel.clone(),
            nested_calls.clone(),
            Arc::clone(&run_abort_requested),
        )));
        let agent = Arc::new(agent);
        // The `(model, messages)` pair pi's `cacheContextIsCurrent` reads (`sdk.ts:350-357`). Weak,
        // and only now available: the agent was built WITH this swap as its `StreamFn`.
        provider_swap.attach_agent(&agent);

        // ---- 10. assemble the session --------------------------------------------------------
        // `host_services` (the concrete arch-08 §5.6 backend) was built + seeded + control-wired at
        // step 4a and injected into every wasm load; it is moved into the services bundle below so
        // `AgentSession::apply_pending_control` drains the SAME queue guest `control` ops reach (Pi
        // `createCommandContext`, agent-session.ts:1158).

        // Resolve the settings-driven knobs for the retry / auto-compaction subsystems BEFORE the
        // `settings` value is moved into the services bundle.
        // The compaction token budgets are NOT resolved here: since v0.86.0 they depend on the
        // active model (`compaction.modelOverrides`), so the session resolves them per call
        // (`AgentSession::compaction_settings_for_model`, SESS-055).
        let eff = settings.effective();
        let to_u32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
        let extras = crate::session::SessionExtras {
            telemetry_enabled,
            branch_summary_settings: cyrup_session::compaction::BranchSummarySettings {
                reserve_tokens: to_u32(eff.branch_summary_reserve_tokens()),
                skip_prompt: eff.branch_summary_skip_prompt(),
            },
            auto_compaction_enabled: eff.compaction_enabled(),
            auto_retry_enabled: eff.retry_enabled(),
            retry_max_retries: to_u32(eff.retry_max_retries()),
            retry_base_delay_ms: u64::try_from(eff.retry_base_delay_ms().max(0)).unwrap_or(0),
            retry_max_agent_delay_ms: u64::try_from(eff.retry_max_agent_delay_ms().max(0))
                .unwrap_or(0),
            proc: bash_proc,
            shell_path: shell_path_setting,
            shell_command_prefix: shell_command_prefix_setting,
            dynamic_tools,
            handle,
            bash_session_env,
            read_model_vision,
            read_model_resize,
            nested_calls,
            run_abort_requested,
            hooks: nested_hooks,
            codemode_host,
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
            model_catalog: self.model_catalog_service,
            context: context_store,
            ext_host,
            allowed_tool_names,
            excluded_tool_names,
            uses_default_tools,
            default_tool_modifiers,
            guest_providers,
            virtual_models,
            model: resolved_model,
            system_prompt,
            system_prompt_sections: built_prompt.sections().clone(),
            host_services,
            extension_flag_values: cfg.extension_flag_values.clone(),
            fs: services_fs,
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

/// Parse the settings `steeringMode`/`followUpMode` string into the agent's [`cyrup_agent::QueueMode`]
/// (Pi `"all"|"one-at-a-time"`; settings-manager.ts:745-757). This is the ONE settings boundary:
/// the strict [`std::str::FromStr`] on `QueueMode` does the parsing, and pi's leniency
/// (`getSteeringMode()` is `this.settings.steeringMode || "one-at-a-time"`, unvalidated — a
/// misspelt setting silently behaves as one-at-a-time) is preserved here, but no longer silently:
/// an unrecognised value is logged once at parse time and falls back to one-at-a-time as pi does.
pub(crate) fn parse_queue_mode(s: &str) -> cyrup_agent::QueueMode {
    s.parse().unwrap_or_else(|e| {
        tracing::warn!(
            value = %s,
            error = %e,
            "queue mode setting not recognised; using one-at-a-time as pi does"
        );
        cyrup_agent::QueueMode::OneAtATime
    })
}

/// Parse the settings `transport` string into the provider [`cyrup_provider::Transport`] Pi hands the agent
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

/// What [`resolve_model`] hands back: the resolved catalog `Model` and its address (both `None` on
/// a modelless launch, see below), the clamped thinking level, and pi's `modelFallbackMessage`.
type ResolvedModel = (
    Option<Model>,
    Option<ModelRef>,
    ModelThinkingLevel,
    Option<String>,
);

/// Resolve `(Option<Model>, Option<ModelRef>, ModelThinkingLevel, modelFallbackMessage)` from the
/// explicit pattern, the resumed session, settings, and finally the catalog (Pi sdk.ts:191-242;
/// R-07-019).
///
/// Precedence mirrors Pi: an explicit `--model` pattern wins; otherwise a resumed session's saved
/// model is restored when it is still resolvable in the catalog (else a `modelFallbackMessage` is
/// produced and we fall back to settings/catalog). The thinking level is likewise restored from the
/// session's `thinking_level_change` entry, then clamped to the chosen model's capabilities.
///
/// # The modelless result (SEAM-075)
/// The model is an `Option` because pi's is: `findInitialModel` legitimately returns
/// `{ model: undefined }` when nothing is configured (model-resolver.ts:648-650), and `sdk.ts:
/// 216-218` turns that into a `modelFallbackMessage` — **a banner, not an error**:
///
/// ```text
/// model = result.model;
/// if (!model) {
///     modelFallbackMessage = formatNoModelsAvailableMessage();
/// } else if (modelFallbackMessage) { … }
/// ```
///
/// The hard stop lives one tier up and is MODE-GATED — `if (appMode !== "interactive" &&
/// !session.model) { console.error(…); process.exit(1); }` (main.ts:852-855) — precisely so a
/// credential-less first run still gets a TUI to type `/login` and then `/model` into. Making an
/// empty catalog fatal HERE would kill that onboarding for every mode, which is exactly the
/// regression this signature closes.
/// What [`resolve_model`]'s RESTORE arm reads — pi's `existingSession` / `sessionModel` /
/// `hasExistingSession` / `hasThinkingEntry` quartet (`sdk.ts:200-222`), grouped so the function
/// keeps one parameter per concern rather than five positional booleans and borrows.
struct RestoreInputs<'a> {
    /// The projection. Read for [`cyrup_session::context::SessionContext::thinking_level`] ONLY
    /// (pi `existingSession.thinkingLevel`, `sdk.ts:248`); its `model` field is the forward
    /// last-wins value and is deliberately not the restore input — see `selection`.
    existing: &'a cyrup_session::context::SessionContext,
    /// The virtual-aware branch selection — pi `sessionModel` (`sdk.ts:209-211`), i.e.
    /// [`cyrup_session::SessionManager::branch_selection`]'s answer.
    selection: Option<&'a ModelRef>,
    /// Registered virtual models. A virtual selection names no row in any provider's catalog, so it
    /// resolves against this instead.
    virtual_models: &'a cyrup_provider::VirtualModelRegistry,
    /// pi `hasExistingSession` (`sdk.ts:201`).
    has_session: bool,
    /// pi `hasThinkingEntry` (`sdk.ts:202`).
    has_thinking_entry: bool,
}

fn resolve_model(
    provider: &dyn Provider,
    cfg: &SessionConfig,
    settings: &SettingsManager,
    restore: &RestoreInputs<'_>,
    model_config: &cyrup_config::ModelFile,
) -> Result<ResolvedModel, SessionServiceError> {
    let RestoreInputs {
        existing,
        selection: session_selection,
        virtual_models,
        has_session: has_existing_session,
        has_thinking_entry,
    } = *restore;
    // The candidate set EVERY arm below resolves against, with the registered virtual rows merged
    // in — pi's `modelRuntime`, which is what `resolveCliModel` and `findInitialModel` are handed
    // (`model-resolver.ts:649`, `:675`) and which lists virtual models because `ModelRuntime`
    // composes each provider through `withVirtualModels` (`virtual-models.ts:201-238`).
    //
    // PROV-DEFECT-3. This used to be the bare `provider.models()`, i.e. the INSTALLED provider's
    // physical catalog, and the restore arm alone patched around it with its own
    // `virtual_models.get(...)` fallback. The consequence was that a registered virtual model was
    // selectable by `/model` (which goes through `AgentSession::full_model_registry`, whose last
    // step IS `apply_to_catalog`) and restorable on resume, but NOT reachable from `--model` or
    // from a `defaultModel` in settings: `cyrup --model router/auto` answered `ModelNotFound` with
    // the router extension loaded and the row in `/model`. Upstream has no such split — step 1 is
    // `resolveCliModel({ modelRuntime })` and step 3 is `modelRuntime.getModel(defaultProvider,
    // defaultModelId)`, both over the same virtual-aware catalog.
    //
    // `apply_to_catalog` is the SAME overlay the session's composed registry applies, so the two
    // catalogs cannot disagree about which rows exist or in what order. A provider with no
    // physical rows appends at the end, which keeps the `available.first()` fallback of step 3 on
    // a physical model whenever there is one.
    //
    // `models.json` is composed FIRST and the virtual overlay applied after, the same order
    // `AgentSession::compose_model_registry` uses and the same order pi uses (`recomposeProvider`
    // composes the provider and only then calls `withVirtualModels`, `model-runtime.ts:171-215`).
    // Composing here is what makes a declared row or a `modelOverrides` patch — `inputLimits`
    // among them — part of the model the session STARTS on, and therefore part of
    // `limits_model()`'s answer and of the `read` tool's seeded profile. With no `models.json` the
    // composition returns the provider's own rows in their own order, unchanged.
    let (mut available, _errors) = model_config.compose(provider.models());
    virtual_models.apply_to_catalog(&mut available);
    let resolver = ModelResolver::new(&available);
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

    // 2. Restore the model from the resumed session (Pi sdk.ts:213-222). The saved model is only
    //    honored when it still resolves in the live catalog (our auth proxy: a model the provider
    //    exposes is usable); otherwise we record the fallback message and keep searching.
    //
    //    `SESS-067` — the saved model is the branch SELECTION (pi's `sessionModel`, the
    //    `getBranchSelection` walk), NOT `existing.model`'s forward last-wins value. The two differ
    //    exactly when a virtual `model_change` is followed by the physical responses it routed to:
    //    the projection reports the model that ANSWERED, where the selection is the router the user
    //    chose and which still holds. `existing` is still read here — for `.thinking_level` at
    //    step 4, which upstream likewise takes off `existingSession` (`sdk.ts:248`).
    //
    //    A virtual selection resolves out of `available` like any other row, because `available`
    //    is the provider's catalog with `apply_to_catalog`'s virtual overlay on top — see the
    //    comment where it is built. This arm used to carry its own
    //    `.or_else(|| virtual_models.get(...))` because `available` was then the bare physical
    //    catalog; with the overlay in place that fallback returned the same `Model` the lookup
    //    above already finds (`VirtualModelRegistry::get` and `apply_to_catalog` both hand back
    //    `entry.model`), so it is gone rather than left as a second path to the same answer.
    //
    //    `[CYRUP-DELTA]`, inherited rather than introduced: pi additionally gates the restore on
    //    `modelRuntime.hasConfiguredAuth(restoredModel.provider)` (`sdk.ts:216`). cyrup has no auth
    //    snapshot at this point in the build — its proxy is "present in `provider.models()`", which
    //    this function's own doc records — so a virtual model listed under a THIRD physical provider
    //    that has no credentials restores here where upstream would reject it. The practical blast
    //    radius is one request: the routing step re-checks credentials and refuses with pi's own
    //    `… which has no credentials.` text, so the session comes up on the router and the first
    //    prompt reports the real problem instead of the selection silently changing at open.
    //    Upstream's common case needs no check anyway: a provider of only virtual models is marked
    //    configured at registration (`model-runtime.ts:962-968`). The message when it does NOT resolve names the SELECTION, as
    //    pi's does (`Could not restore model ${sessionModel.provider}/${sessionModel.modelId}`,
    //    `sdk.ts:220`), so a dead `router/auto` reports `router/auto` and not the model that
    //    answered under it.
    if model.is_none()
        && has_existing_session
        && let Some(saved) = session_selection
    {
        let restored = available
            .iter()
            .find(|m| m.provider == saved.provider && m.id == saved.model)
            .cloned();
        match restored {
            Some(m) => model = Some(m),
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
        match resolved.or_else(|| available.first().cloned()) {
            Some(m) => {
                if let Some(msg) = fallback.as_mut() {
                    msg.push_str(&format!(
                        ". Using {}/{}",
                        m.provider.as_str(),
                        m.id.as_str()
                    ));
                }
                model = Some(m);
            }
            // Pi sdk.ts:216-218 — `findInitialModel` returned `{model: undefined}`
            // (model-resolver.ts:648-650). The message REPLACES any "Could not restore model …"
            // text set in step 2, because pi's `if (!model)` branch assigns rather than appends.
            None => fallback = Some(crate::auth_guidance::format_no_models_available_message()),
        }
    }

    // 4. Thinking level: explicit option → restored from session → settings default; clamped to the
    //    chosen model's supported levels (Pi sdk.ts:223-242).
    //    CFG-056: `getDefaultThinkingLevel()` returns `ThinkingLevel | undefined` upstream
    //    (settings-manager.ts:740-742) and each of Pi's sites names the fallback explicitly —
    //    `settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL` (sdk.ts:230, :235).
    //    `DEFAULT_THINKING_LEVEL` is `"medium"` (core/defaults.ts:3), NOT the type's `Off` zero.
    let settings_default = || {
        settings
            .effective()
            .default_thinking_level()
            .unwrap_or(cyrup_config::DEFAULT_THINKING_LEVEL)
    };
    let mut thinking = cfg.thinking_level.or(parsed_thinking);
    if thinking.is_none() && has_existing_session {
        thinking = Some(if has_thinking_entry {
            thinking_level_from_str(&existing.thinking_level).unwrap_or_else(settings_default)
        } else {
            settings_default()
        });
    }
    // Pi `sdk.ts:239-247`: the per-model override sits between the restored/explicit level and the
    // global default, so a launch on a model with its own recorded level starts there rather than
    // at the one-size default. Only consulted when nothing more specific already decided.
    let thinking = thinking.or_else(|| {
        model.as_ref().and_then(|m| {
            settings
                .effective()
                .model_thinking_level(m.provider.as_str(), m.id.as_str())
        })
    });
    let thinking = thinking.unwrap_or_else(settings_default);
    // Pi sdk.ts:238-242: `if (!model) { thinkingLevel = "off"; } else { thinkingLevel =
    // clampThinkingLevel(model, thinkingLevel); }` — a modelless session has nothing to clamp
    // against, so the level is forced off rather than carried from settings.
    let thinking = match model.as_ref() {
        Some(m) => cyrup_provider::clamp_thinking_level(m, thinking),
        None => ModelThinkingLevel::Off,
    };

    let model_ref = model.as_ref().map(|m| ModelRef {
        provider: m.provider.clone(),
        api: Some(m.api.clone()),
        model: m.id.clone(),
    });
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
    let has_matching_prefix = pattern
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase());
    if !cfg.cli_provider_explicit && !has_matching_prefix {
        return None;
    }
    // Strip the provider prefix (Pi `pattern = cliModel.substring(slashIndex + 1)`), then peel a
    // trailing `:level` thinking suffix when `--thinking` was not explicitly set.
    let stripped: &str = if has_matching_prefix {
        pattern.get(prefix.len()..).unwrap_or(pattern)
    } else {
        pattern
    };
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
    let model =
        cyrup_config::build_fallback_model(provider_id.as_str(), base_id, provider.models())?;
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
        guidelines: tool
            .prompt_guidelines()
            .iter()
            .copied()
            .map(Arc::<str>::from)
            .collect(),
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
    global_extension_patterns: &[String],
    force_wasm_failure: bool,
) -> Option<cyrup_ext::ProjectTrustDecision> {
    // A globally disabled extension must not vote on trust either: this throwaway pass loads the
    // SAME pre-trust set the real pass loads, so it has to apply the same settings `-pattern`
    // filter. Only the GLOBAL tier is scanned — the project root is excluded by
    // `DiscoveredExtension::is_trusted` at `project_trusted = false` — so only the global layer's
    // `extensions` array is consulted, which is also all that is resolvable before trust is frozen.
    //
    // Without the wasm host this pass loads natives only — no disk scan, nothing to filter.
    #[cfg(not(feature = "wasm-host"))]
    let _ = global_extension_patterns;
    let (mode, has_ui) = ext_mode(cfg.app_mode);
    let host_config = HostConfig {
        mode,
        has_ui,
        cwd: cwd.to_path_buf(),
    };
    // EXT-003 — a wasm-runtime construction failure must NOT discard the native votes. This line
    // used to be `ExtensionHost::with_wasm(host_config).ok()?`, so on a machine where the Wasmtime
    // engine cannot be built the WHOLE pre-trust pass returned `None` and every native
    // `decides_project_trust` extension was silently skipped, even though not one of them needs
    // wasm to vote. The realistic trigger is `build_engine`'s pooling allocator reserving its
    // slabs (`cyrup-ext/src/host/engine.rs:19-26`) — `build_engine_on_demand` (`:31-37`) exists
    // precisely because that reservation fails on constrained hosts, and address-space pressure is
    // transient, so this pass can fail while the real host at step 4b succeeds.
    //
    // Pi has no runtime to construct, but it does have the corresponding partial-failure rule and
    // it is the opposite of fail-everything: `loadProjectTrustExtensions()`
    // (`core/resource-loader.ts:380-386` @v0.84.4) returns a `LoadExtensionsResult` whose
    // per-extension faults live in `errors`, and `resolveProjectTrusted` hands that straight to
    // `emitProjectTrustEvent`, which polls `extensionsResult.extensions` — the ones that DID load
    // (`core/extensions/runner.ts:204-233` @v0.84.4). A failure in part of the extension subsystem
    // never silences the part that still works; here the natives are exactly that part.
    #[cfg(feature = "wasm-host")]
    let (host, wasm_ready) = match try_wasm_host(host_config.clone(), force_wasm_failure) {
        Ok(host) => (host, true),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "pre-trust wasm runtime unavailable; native project_trust deciders vote alone"
            );
            (ExtensionHost::new(host_config), false)
        }
    };
    #[cfg(not(feature = "wasm-host"))]
    let host = {
        let _ = force_wasm_failure;
        ExtensionHost::new(host_config)
    };

    // SEAM-071: a native that `--no-extensions` will not load must not vote on project trust
    // either — pi's pre-trust pass (`loadProjectTrustExtensions()`) runs over the SAME reduced set
    // its main pass does, because both read `extensionPaths` (resource-loader.ts:451-455 @v0.83.0).
    let is_subagent_child = std::env::var_os(SUBAGENT_CHILD_ENV).is_some();
    let voters = natives.iter().filter(|e| {
        !cfg.no_extensions
            || native_survives_no_extensions(e, is_subagent_child, child_keeps_codemode(cfg))
    });
    for ext in voters.filter(|e| e.decides_project_trust()) {
        // A load failure in the throwaway pass must not fail the build — the real load at step 4b
        // surfaces it. Skip and keep polling the rest.
        if let Err(e) = host.load_native(ext.clone()).await {
            tracing::debug!(error = %e, "pre-trust extension load skipped");
        }
    }
    // No engine ⇒ nothing to discover FOR: every `load_wasm_with_caps` on a runtime-less host
    // returns `ExtError::WasmHostDisabled` (`cyrup-ext/src/facade.rs:1834`), so the scan would cost
    // a walk of the global extensions root to produce a `LoadExtensionsResult` of nothing but
    // errors. Pi's equivalent is that a tier it cannot load contributes no voters, not that the
    // remaining voters are dropped.
    #[cfg(feature = "wasm-host")]
    if wasm_ready {
        let mut roots = extension_discovery_roots(cfg);
        roots.disabled = cyrup_resources::scan_loose_extension_root(
            &cfg.agent_dir.join(cyrup_ext::EXTENSIONS_SUBDIR),
            cyrup_resources::ResourceScope::Global,
            global_extension_patterns,
        )
        .into_iter()
        .filter(|e| !e.enabled)
        .map(|e| e.path)
        .collect();
        let deny: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(cyrup_ext::DenyServices);
        // `false` = pre-trust tier only (global + CLI-configured), exactly Pi's
        // `loadProjectTrustExtensions()`.
        let _ = host.discover_and_load(&roots, false, deny).await;
    }

    let decision = host.aggregate_project_trust(&CancelToken::new()).await;
    // The host (and every instance it loaded) is dropped here — Pi's `clearExtensionCache()`.
    decision
}

/// [`ExtensionHost::with_wasm`] with the test-only fault injection
/// `SessionBuilder::force_pre_trust_wasm_failure` / `force_runtime_wasm_failure` folded in so the
/// native-only fallbacks are reachable from a test. Every non-`cfg(test)` build compiles to the
/// bare `with_wasm` call: `force_failure` is dead in production and the `Err` arm is only ever
/// produced by a real engine-construction failure.
#[cfg(feature = "wasm-host")]
fn try_wasm_host(
    config: HostConfig,
    force_failure: bool,
) -> Result<ExtensionHost, cyrup_ext::ExtError> {
    #[cfg(test)]
    if force_failure {
        return Err(cyrup_ext::ExtError::Engine(
            "forced wasm-runtime failure".to_string(),
        ));
    }
    #[cfg(not(test))]
    let _ = force_failure;
    ExtensionHost::with_wasm(config)
}

/// The session's extension host: with the Wasmtime runtime when it can be built, and without it
/// (natives only) when it cannot. `ExtensionHost::with_wasm` already retries with the on-demand
/// engine when the pooling allocator cannot reserve its slabs, so this is the second line: an
/// engine that cannot be built at all.
#[cfg(feature = "wasm-host")]
fn wasm_host_or_native_only(config: HostConfig, force_failure: bool) -> ExtensionHost {
    match try_wasm_host(config.clone(), force_failure) {
        Ok(host) => host,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "wasm runtime unavailable; running the native extensions only"
            );
            ExtensionHost::new(config)
        }
    }
}

/// The subagent-child marker (`cyrup_ext_subagents::spawn::nested_events::CHILD_ENV`). Read as a
/// literal rather than imported because `cyrup-session-svc` sits BELOW `cyrup-ext-subagents` in the
/// crate graph — the natives are injected into the builder from `crates/cyrup/src/main.rs`, which is
/// the only place that depends on both. See [`native_survives_no_extensions`] for why the builder
/// needs to know.
const SUBAGENT_CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";

/// The natives a subagent CHILD keeps across `--no-extensions`, and only a child (SEAM-071).
///
/// pi's subagent launcher passes `--no-extensions` to the child whenever the agent pins an extension
/// allowlist, and in the SAME breath re-adds three extensions as explicit `--extension <path>` args:
/// `PROMPT_RUNTIME_EXTENSION_PATH`, the fanout-child extension when the child is fanout-authorized,
/// and the resolved `@gotgenes/pi-permission-system` — `pi-subagents v0.47.1
/// src/runs/shared/pi-args.ts:413-420` (`runtimeExtensions`) emitted at `:556-560`. So in pi a child
/// under `--no-extensions` keeps exactly these three and loses everything else, pi-intercom included.
///
/// cyrup selects the same three by ENV rather than by path — `subagent_extension_for_env`,
/// `prompt_runtime_extension_for_env`, `permission_extension_for_env` in `crates/cyrup/src/main.rs`
/// — because its child-side runtime is compiled in, not a loadable file (the mechanism note at
/// `cyrup-ext-subagents/src/exec/mod.rs:1495-1499`). Env-selection IS cyrup's re-injection channel,
/// so gating these three in a child would drop what pi explicitly keeps — and for
/// `cyrup-permission-system` that is a permission gate failing OPEN, which is worse than the
/// unfiltered load SEAM-071 was filed about.
const SUBAGENT_CHILD_RUNTIME_NATIVES: [&str; 3] = [
    "cyrup-permission-system",
    "subagent-prompt-runtime",
    "subagents",
];

/// Whether a native built-in survives `--no-extensions` (SEAM-071).
///
/// The discriminator is [`cyrup_ext::NativeExtension::is_ambient`], declared by each built-in on
/// itself (SEAM-074). It USED to be a hardcoded `AMBIENT_NATIVE_IDS` list here, which was unsound:
/// pi's two tiers differ by HOW an extension arrived, not by what it is called, so matching on the
/// id also caught anything that merely shared the name. That is not hypothetical — it gated
/// `build_containment_and_flag_diagnostics.rs`'s hand-injected `FailingExt { id: "subagents" }` out
/// of the load entirely, so a native init failure stopped reaching the startup panel and the exit
/// channel. An extension the embedder passed by value IS pi's inline tier by construction:
/// `loadFinalExtensionSet` calls `loadExtensionFactories` unconditionally (`resource-loader.ts:579-581`
/// @v0.83.0) over `extensionFactories = [...builtInExtensions, ...(options?.extensionFactories ?? [])]`
/// (`main.ts:523`), while only `extensionPaths` — the PATH tier — is collapsed by the flag (`:451-453`).
fn native_survives_no_extensions(
    ext: &Arc<dyn NativeExtension>,
    is_subagent_child: bool,
    keeps_codemode: bool,
) -> bool {
    let id = ext.id();
    let id = id.as_str();
    // pi's inline-factory tier: never gated by a flag about discovery.
    if !ext.is_ambient() {
        return true;
    }
    // The ambient tier, plus pi's one carve-out: a subagent child keeps the extensions its launcher
    // re-injects by path (see [`SUBAGENT_CHILD_RUNTIME_NATIVES`]), and `codemode` when the child's
    // own `--tools` names it (see [`child_keeps_codemode`]).
    is_subagent_child
        && (SUBAGENT_CHILD_RUNTIME_NATIVES.contains(&id)
            || (keeps_codemode && id == cyrup_codemode_runtime::EXTENSION_ID))
}

/// The startup diagnostic for a `codemode` the allowlist names but `--no-extensions` left unloaded.
/// `--tools codemode` then names a tool that does not exist, and nothing said so: the model's first
/// call came back "Tool codemode not found". Not fatal (the session is usable), so it travels on the
/// `[Extension issues]` panel and not on the exit channel.
///
/// **[CYRUP-DELTA]** pi has no such diagnostic; its `builtin:codemode` is dropped just as silently
/// (`package-manager.ts:972-974` @v1.0.1).
fn codemode_skipped_diagnostic(
    cfg: &SessionConfig,
    skipped: bool,
) -> Option<crate::services::ExtensionLoadDiagnostic> {
    let id = cyrup_codemode_runtime::EXTENSION_ID;
    (skipped && cyrup_core::tool_name_matches(cfg.tools.iter().flatten(), id)).then(|| {
        crate::services::ExtensionLoadDiagnostic {
            path: PathBuf::from(id),
            error: format!(
                "--tools names `{id}`, but --no-extensions skipped the built-in extension that provides it, so the tool does not exist in this session. Drop --no-extensions to use it."
            ),
            fatal: false,
        }
    })
}

/// Whether a subagent child under `--no-extensions` keeps the `codemode` built-in: its `--tools`
/// allowlist names it (an exact name or a `*` pattern) and `--exclude-tools` does not.
///
/// **[CYRUP-DELTA]** `codemode` is an ambient built-in, so `--no-extensions` (which a subagent whose
/// agent pins `extensions:` always gets) used to drop it silently even when the agent's `tools:`
/// list named it. pi-subagents decides the same question from the child's tool list alone: its child
/// session registers the host's codemode factory when `launch.tools` is unset or includes
/// `"codemode"`, `codemode` is not in `excludeTools`, and the capability ceiling does not deny
/// extensions (`src/runs/shared/child-session.ts` @HEAD `0c33ec7c`, from `8f90bf1b` "register Pi
/// codemode in native child sessions", first in v0.74.0; the ledger pin v0.71.0 predates it).
/// cyrup keeps only the explicit-name half: an unpinned child (`--tools` absent) gets its own
/// default tool set, which does not include `codemode`, so nothing is gained by keeping it loaded.
/// The `denyExtensions` half is the parent's: the spawn plan leaves `codemode` out of the `--tools`
/// it hands a child under a ceiling that denies extensions (`exec/tool_surface.rs`), and this check
/// reads that list.
fn child_keeps_codemode(cfg: &SessionConfig) -> bool {
    let id = cyrup_codemode_runtime::EXTENSION_ID;
    cfg.tools
        .as_ref()
        .is_some_and(|tools| cyrup_core::tool_name_matches(tools, id))
        && !cyrup_core::tool_name_matches(&cfg.exclude_tools, id)
}

/// `natives` without the built-ins `disabled` names by id (pi `resourceLoaderOptions.
/// disabledBuiltinExtensions`, `core/resource-loader.ts` @v1.0.4: a `builtin:<id>` path whose id is
/// in the set is not loaded). Pure, so the flag's meaning is testable without standing up a session.
fn without_disabled_builtins(
    natives: Vec<Arc<dyn NativeExtension>>,
    disabled: &[String],
) -> Vec<Arc<dyn NativeExtension>> {
    natives
        .into_iter()
        .filter(|ext| {
            let off = disabled.iter().any(|id| id == ext.id().as_str());
            if off {
                tracing::debug!(extension = %ext.id(), "native built-in not loaded: disabled");
            }
            !off
        })
        .collect()
}

/// The native built-ins this session actually loads (SEAM-071). Pure, so the flag's meaning is
/// testable without standing up a session: before this existed the build loop iterated
/// `self.native_extensions` unconditionally and `--no-extensions` reached only the disk roots.
fn natives_to_load(
    natives: Vec<Arc<dyn NativeExtension>>,
    no_extensions: bool,
    is_subagent_child: bool,
    keeps_codemode: bool,
) -> Vec<Arc<dyn NativeExtension>> {
    if !no_extensions {
        return natives;
    }
    natives
        .into_iter()
        .filter(|e| {
            let keep = native_survives_no_extensions(e, is_subagent_child, keeps_codemode);
            if !keep {
                tracing::debug!(extension = %e.id(), "native built-in skipped: --no-extensions");
            }
            keep
        })
        .collect()
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
            // The settings `-pattern` set is filled in by the caller, which is the only place the
            // resolved settings exist — see `disabled_loose_extensions` below.
            disabled: Vec::new(),
        }
    } else {
        cyrup_ext::DiscoveryRoots {
            project_cwd: Some(cfg.cwd.clone()),
            agent_dir: Some(cfg.agent_dir.clone()),
            configured: cfg.extra_extension_paths.clone(),
            disabled: Vec::new(),
        }
    }
}

/// The loose extensions a settings `extensions` array turned OFF — the paths
/// [`cyrup_ext::DiscoveryRoots::disabled`] must skip so an extension disabled through
/// `cyrup config` is not loaded on the next run.
///
/// `cyrup-resources` records every auto-discovered loose extension with its settings verdict
/// (`ResourceRegistry::loose_extensions`, `LooseExtension::enabled`) rather than filtering the
/// disabled ones out, precisely so this negative half survives the resolve; all that is left here is
/// to project it. Pi has no analogue because its resolve returns `enabled` per path and its loader
/// consumes that list directly (`resource-loader.ts:451-455`); cyrup's loader scans the disk itself,
/// so the verdict has to be handed to it.
fn disabled_loose_extensions(registry: &cyrup_resources::ResourceRegistry) -> Vec<PathBuf> {
    registry
        .loose_extensions
        .iter()
        .filter(|e| !e.enabled)
        .map(|e| e.path.clone())
        .collect()
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
    cwd: &Path,
    agent_dir: &Path,
) -> (Vec<ConfiguredPackage>, Vec<String>) {
    let mut out: Vec<(String, ConfiguredPackage)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (layer, scope) in [
        (settings.project(), InstallScope::Project),
        (settings.global(), InstallScope::Global),
    ] {
        // Pi `getBaseDirForScope(entry.scope)` (package-manager.ts:2071-2080), the base the scope's
        // LOCAL sources resolve against before they can be compared.
        let base = cyrup_resources::scope_base_dir(cwd, agent_dir, scope);
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
            // The key is Pi's `getPackageIdentity(source, entry.scope)` (:1676-1690), NOT the raw
            // source string (CFG-026): `npm:x@1`/`npm:x@2` and an SSH/HTTPS pair for one repo
            // collide, while `"./pack"` declared in BOTH scopes does not — it names
            // `<cwd>/.cyrup/pack` in project scope and `<agent_dir>/pack` in global scope, two
            // different trees, so pi keeps both and cyrup used to drop the global one.
            let identity = cyrup_resources::package_identity(&built.source, &base);
            match out.iter().position(|(id, _)| *id == identity) {
                None => out.push((identity, built)),
                Some(index) => {
                    let existing_is_project_delta = out.get(index).is_some_and(|(_, p)| {
                        p.scope == InstallScope::Project && p.filter.is_delta()
                    });
                    if existing_is_project_delta && built.scope == InstallScope::Global {
                        out.push((identity, built));
                    } else if built.scope == InstallScope::Project
                        && let Some(slot) = out.get_mut(index)
                    {
                        *slot = (identity, built);
                    }
                }
            }
        }
    }
    (out.into_iter().map(|(_, p)| p).collect(), errors)
}

/// Map the runtime mode to the extension `(ExtMode, has_ui)` (R-11-002).
fn ext_mode(mode: AppMode) -> (ExtMode, bool) {
    match mode {
        AppMode::Interactive => (ExtMode::Tui, true),
        AppMode::Rpc => (ExtMode::Rpc, true),
        AppMode::Json => (ExtMode::Json, false),
        AppMode::Print => (ExtMode::Print, false),
        // ACP-025 — the ACP host reports itself to extensions as `rpc`, with `has_ui: true`.
        //
        // # CYRUP-DELTA
        //
        // **What differs.** pi-acp's adapter genuinely spawns `pi --mode rpc`, so its guest `ctx.mode`
        // is `"rpc"` as a fact about the child rather than a choice. cyrup's ACP host is a fourth
        // in-process front-end and could report a fifth `ExtMode` — this arm decides it does not.
        // `has_ui` is `true` for the same reason `Rpc`'s is: the host owns a live dialog channel
        // (`UiSink` → `session/request_permission` / `elicitation/create`), which is the only thing
        // `has_ui` claims.
        //
        // **What it costs.** `ctx.mode` is guest-visible, so an extension cannot tell an ACP client
        // from an RPC client and cannot special-case either. Two concrete consequences: an extension
        // that gates a feature on `mode == "rpc"` will enable it under ACP (usually right — both are
        // JSON-RPC-over-stdio hosts with a dialog channel), and an extension that wants ACP-only
        // behaviour has no way to ask for it. Adding an `ExtMode::Acp` variant instead would be a
        // guest-visible wire change to `cyrup_ext::HostConfig` for every already-published
        // extension, whose `mode` matches would then fall to their default arm — a strictly worse
        // trade for a distinction no extension has asked for. Revisit only with a real consumer.
        AppMode::Acp => (ExtMode::Rpc, true),
    }
}

/// `resolvePromptInput(source, description)`'s read leg (`resource-loader.ts:53-68` @v0.83.0), for a
/// source that `cyrup_resources::discover_system_prompt_file` already `exists()`-checked:
///
/// ```ts
/// if (existsSync(input)) {
///     try { return readFileSync(input, "utf-8"); }
///     catch (error) {
///         console.error(chalk.yellow(`Warning: Could not read ${description} file ${input}: ${error}`));
///         return input;
///     }
/// }
/// ```
///
/// The `return input` on a read failure is upstream's literal behaviour and is ported as such: the
/// PATH STRING becomes the prompt body. It looks wrong and it is faithful — `resolvePromptInput`
/// cannot distinguish "a path that failed to read" from "prompt text that happens to name a file",
/// so it falls back to the same branch as the not-a-path case. `cyrup/src/cli.rs`'s
/// `resolve_prompt_input` already ports the identical rung for the `--system-prompt` flag.
///
/// The warning goes to `tracing` rather than to `StartupDiagnostics`: upstream's is a bare
/// `console.error`, not a resource diagnostic, and `ResourceKind` has no variant for a prompt FILE
/// (its `Prompt` is the prompt-template family, which would file this under `[Prompt conflicts]`).
fn read_discovered_prompt(path: &Path, description: &str) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => {
            tracing::warn!(
                "Warning: Could not read {description} file {}: {e}",
                path.display()
            );
            Some(path.to_string_lossy().into_owned())
        }
    }
}

/// Today's date (UTC) for the prompt footer; falls back to the epoch on a clock fault.
fn today() -> time::Date {
    time::OffsetDateTime::now_utc().date()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::{ALL_BUILTIN_TOOLS, DEFAULT_BUILTIN_TOOLS, ext_mode, http_proxy_overlay};
    use cyrup_config::AppMode;
    use cyrup_ext::ExtMode;
    use std::path::Path;

    /// ACP-025 — the guest-visible mode string an ACP session reports. `AppMode::Acp` is
    /// deliberately projected onto the EXISTING `ExtMode::Rpc` rather than a fifth variant; the
    /// `CYRUP-DELTA` at [`ext_mode`] states what that costs. The other four rows are asserted here
    /// too so the new arm cannot be added by rewriting one of them.
    #[test]
    fn acp_reports_itself_to_extensions_as_rpc_with_a_ui() {
        assert_eq!(ext_mode(AppMode::Acp), (ExtMode::Rpc, true));
        assert_eq!(ext_mode(AppMode::Interactive), (ExtMode::Tui, true));
        assert_eq!(ext_mode(AppMode::Rpc), (ExtMode::Rpc, true));
        assert_eq!(ext_mode(AppMode::Json), (ExtMode::Json, false));
        assert_eq!(ext_mode(AppMode::Print), (ExtMode::Print, false));
    }

    /// `select_active_tools`'s no-flags arm is
    /// `DEFAULT_BUILTIN_TOOLS.contains(name) || !ALL_BUILTIN_TOOLS.contains(name)` — it KEEPS any
    /// name it does not recognise as a built-in, because that is how an extension- or
    /// embedder-supplied tool stays active. So every name `ToolRegistry::with_builtins` installs
    /// must appear in `ALL_BUILTIN_TOOLS` or it is silently on by default. `powershell` is the
    /// case this guards: pi's `defaultActiveToolNames` is `read`/`bash`/`edit`/`write`
    /// (sdk.ts:256) and `powershell` is reachable only through `--tools` / `defaultTools`.
    #[test]
    fn every_builtin_is_gated_and_powershell_is_not_a_default() {
        for name in cyrup_tools::BUILTIN_NAMES {
            assert!(
                ALL_BUILTIN_TOOLS.contains(&name),
                "`{name}` is registered by `with_builtins` but absent from ALL_BUILTIN_TOOLS, so \
                 the default arm of `select_active_tools` would treat it as an extension tool and \
                 enable it in EVERY session"
            );
        }
        assert_eq!(ALL_BUILTIN_TOOLS.len(), cyrup_tools::BUILTIN_NAMES.len());
        assert!(ALL_BUILTIN_TOOLS.contains(&"powershell"));
        assert!(
            !DEFAULT_BUILTIN_TOOLS.contains(&"powershell"),
            "pi's default active set is read/bash/edit/write (sdk.ts:256)"
        );
        assert_eq!(DEFAULT_BUILTIN_TOOLS, ["read", "bash", "edit", "write"]);
        // The reverse direction: nothing is gated that the registry does not actually install.
        for name in DEFAULT_BUILTIN_TOOLS {
            assert!(cyrup_tools::BUILTIN_NAMES.contains(&name));
        }
    }

    // ---- CFG-079: the `defaultTools` setting is the initial BUILT-IN selection ----------------
    //
    // pi v0.84.4 `sdk.ts:256-263`. Ported from `test/default-tools-setting.test.ts` at the same
    // tag, case for case: "uses the configured list as the initial built-in selection",
    // "can select powershell instead of bash", "keeps extension and SDK custom tools enabled" and
    // "preserves explicit tool option precedence".

    /// A name-only tool: `select_active_tools` reads nothing else off the trait.
    struct NamedTool(&'static str, serde_json::Value);

    #[async_trait::async_trait]
    impl cyrup_core::Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.1
        }
        async fn execute(
            &self,
            _id: cyrup_core::ToolCallId,
            _args: serde_json::Value,
            _cancel: cyrup_core::CancelToken,
            _on_update: Box<dyn FnMut(cyrup_core::ToolUpdate) + Send>,
        ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
            Err(cyrup_core::ToolError::new("not executable"))
        }
    }

    /// Every built-in `ToolRegistry::with_builtins` installs, plus one extension-supplied tool —
    /// upstream's `getAllTools()` in the same test file, which lists all eight built-ins whatever
    /// `defaultTools` says, plus the extension/SDK tools.
    fn visible_tools() -> Vec<std::sync::Arc<dyn cyrup_core::Tool>> {
        ALL_BUILTIN_TOOLS
            .iter()
            .copied()
            .chain(std::iter::once("static_tool"))
            .map(|name| {
                std::sync::Arc::new(NamedTool(name, serde_json::json!({})))
                    as std::sync::Arc<dyn cyrup_core::Tool>
            })
            .collect()
    }

    fn selected(cfg: &super::SessionConfig, default_tools: Option<&[String]>) -> Vec<String> {
        super::select_active_tools(&visible_tools(), cfg, default_tools)
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// `--tools` and `--exclude-tools` entries with a `*` are patterns (`createToolNameMatcher`,
    /// `core/mcp-servers.ts` @v1.0.4): the allowlist activates what matches, the denylist removes
    /// what matches from the default selection.
    #[test]
    fn tools_and_exclude_tools_entries_may_be_patterns() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.tools = Some(names(&["re*", "gr*"]));
        let mut picked = selected(&cfg, None);
        picked.sort();
        assert_eq!(picked, names(&["grep", "read"]));

        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.exclude_tools = names(&["ba*", "*it"]);
        let mut picked = selected(&cfg, None);
        picked.sort();
        assert_eq!(
            picked,
            names(&["read", "static_tool", "write"]),
            "`bash` and `edit` are excluded by their patterns"
        );
    }

    /// A non-built-in tool with a settable exposure and `defaultActive`.
    struct ExposedTool {
        name: &'static str,
        exposure: cyrup_core::ToolExposure,
        default_active: bool,
        params: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl cyrup_core::Tool for ExposedTool {
        fn name(&self) -> &str {
            self.name
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.params
        }
        fn exposure(&self) -> cyrup_core::ToolExposure {
            self.exposure
        }
        fn default_active(&self) -> bool {
            self.default_active
        }
        async fn execute(
            &self,
            _id: cyrup_core::ToolCallId,
            _args: serde_json::Value,
            _cancel: cyrup_core::CancelToken,
            _on_update: Box<dyn FnMut(cyrup_core::ToolUpdate) + Send>,
        ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
            Err(cyrup_core::ToolError::new("not executable"))
        }
    }

    /// pi `_isActivatedOnRegistration` (`agent-session.ts:3554` @v1.0.1) decides which NON-built-in
    /// visible tools start active: `direct`/`model-only` unless `defaultActive: false`. An explicit
    /// `tools` allowlist activates a named tool iff it is declarable (`:3510-3516`).
    #[test]
    fn a_non_builtin_tool_starts_active_only_when_registration_activates_it() {
        use cyrup_core::ToolExposure as E;
        let tool = |name, exposure, default_active| {
            std::sync::Arc::new(ExposedTool {
                name,
                exposure,
                default_active,
                params: serde_json::json!({}),
            }) as std::sync::Arc<dyn cyrup_core::Tool>
        };
        let visible = vec![
            tool("x_direct", E::Direct, true),
            tool("x_model_only", E::ModelOnly, true),
            tool("x_codemode", E::Codemode, true),
            tool("x_deferred", E::Deferred, true),
            tool("x_hidden", E::Hidden, true),
            tool("x_inactive", E::Direct, false),
        ];
        let pick = |cfg: &super::SessionConfig| -> Vec<String> {
            super::select_active_tools(&visible, cfg, None)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        };

        let cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        assert_eq!(pick(&cfg), names(&["x_direct", "x_model_only"]));

        let mut builtin_off = super::SessionConfig::new("/tmp", "/tmp/agent");
        builtin_off.no_tools = Some(super::NoTools::Builtin);
        assert_eq!(pick(&builtin_off), names(&["x_direct", "x_model_only"]));

        // Naming activates iff declarable, even when not active by default; a codemode or hidden
        // tool named in the allowlist is still not activated.
        let mut allow = super::SessionConfig::new("/tmp", "/tmp/agent");
        allow.tools = Some(names(&[
            "x_inactive",
            "x_codemode",
            "x_hidden",
            "x_model_only",
        ]));
        assert_eq!(pick(&allow), names(&["x_model_only", "x_inactive"]));
    }

    #[test]
    fn the_default_tools_setting_replaces_pis_four_built_ins() {
        let cfg = super::SessionConfig::new("/tmp", "/tmp/agent");

        // Unset: pi's `defaultActiveToolNames` (sdk.ts:256), extension tool alongside.
        assert_eq!(
            selected(&cfg, None),
            names(&["read", "write", "edit", "bash", "static_tool"])
        );

        // "uses the configured list as the initial built-in selection": grep/find are built-ins pi
        // does NOT activate by default, and the four defaults are REPLACED, not widened.
        assert_eq!(
            selected(&cfg, Some(&names(&["grep", "find"]))),
            names(&["grep", "find", "static_tool"])
        );

        // "can select powershell instead of bash".
        assert_eq!(
            selected(&cfg, Some(&names(&["read", "powershell", "edit", "write"]))),
            names(&["read", "write", "edit", "powershell", "static_tool"])
        );

        // An explicit `[]` is a configured value, not "unset": no built-ins at all. The extension
        // tool still survives — this is the `541045ae0` fix, and the reason the parameter is an
        // `Option` rather than a `Vec` defaulted to the four names.
        assert_eq!(selected(&cfg, Some(&[])), names(&["static_tool"]));

        // A name that matches no tool is carried through and simply selects nothing: pi's getter
        // validates nothing (settings-manager.ts:1273-1276).
        assert_eq!(
            selected(&cfg, Some(&names(&["read", "no-such-tool"]))),
            names(&["read", "static_tool"])
        );
    }

    /// CFG-097 — the reachable break, end to end: pi's own documented `"defaultTools":
    /// ["+codemode"]` (`docs/settings.md:50`) must ADD to the baseline, not wipe it.
    ///
    /// This is the observed-at side of the resolution that `EffectiveSettings::default_tools` now
    /// performs. Before it, the setting reached `select_active_tools` verbatim, no visible tool was
    /// named `+grep`, and the session started with the extension tool and NOTHING else — a silent,
    /// total loss of the default loadout from a documented config line.
    #[test]
    fn a_modifier_default_tools_setting_adds_to_the_baseline_at_the_selector() {
        let cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        let eff = |json: &str| {
            cyrup_config::EffectiveSettings::from_settings(
                cyrup_config::Settings::parse(json).unwrap(),
            )
        };

        let added = eff(r#"{"defaultTools":["+grep"]}"#).default_tools();
        assert_eq!(
            selected(&cfg, added.as_deref()),
            names(&["read", "write", "edit", "bash", "grep", "static_tool"])
        );

        let removed = eff(r#"{"defaultTools":["-bash"]}"#).default_tools();
        assert_eq!(
            selected(&cfg, removed.as_deref()),
            names(&["read", "write", "edit", "static_tool"])
        );

        // Plain names still replace, so the pre-modifier contract is untouched.
        let replaced = eff(r#"{"defaultTools":["grep","find"]}"#).default_tools();
        assert_eq!(
            selected(&cfg, replaced.as_deref()),
            names(&["grep", "find", "static_tool"])
        );
    }

    #[test]
    fn explicit_tool_flags_still_outrank_the_default_tools_setting() {
        // "preserves explicit tool option precedence" — pi's
        // `options.tools ?? (options.noTools ? [] : (configuredDefaultToolNames ?? …))`, then the
        // `excludeTools` filter (sdk.ts:261-263).
        let default_tools = names(&["read", "grep"]);

        let mut allowlisted = super::SessionConfig::new("/tmp", "/tmp/agent");
        allowlisted.tools = Some(names(&["read"]));
        assert_eq!(
            selected(&allowlisted, Some(&default_tools)),
            names(&["read"])
        );

        let mut excluded = super::SessionConfig::new("/tmp", "/tmp/agent");
        excluded.exclude_tools = names(&["read"]);
        assert_eq!(
            selected(&excluded, Some(&default_tools)),
            names(&["grep", "static_tool"])
        );

        let mut none = super::SessionConfig::new("/tmp", "/tmp/agent");
        none.no_tools = Some(super::NoTools::All);
        assert!(selected(&none, Some(&default_tools)).is_empty());
    }

    // ---- SEAM-118: `--no-builtin-tools` starts a session with NO built-in active -------------
    //
    // pi v0.84.4 `sdk.ts:261-262`:
    //
    // ```text
    // options.tools ?? (options.noTools ? [] : (configuredDefaultToolNames ?? defaultActiveToolNames))
    // ```
    //
    // ANY `noTools` — `"all"` or `"builtin"` — yields an EMPTY initial active set; the two modes
    // differ only in `allowedToolNames` (`sdk.ts:258`, `options.noTools === "all" ? [] : undefined`),
    // i.e. in whether a tool can be re-enabled later, not in what starts active. Extension and SDK
    // tools survive `"builtin"` solely through `_buildRuntime`'s `includeAllExtensionTools: true`
    // (`agent-session.ts:406-410` → the `:2742-2745` branch), which is what cyrup's
    // `!ALL_BUILTIN_TOOLS.contains(name)` leg spells.
    //
    // Ported from upstream's own regression test at the same tag,
    // `test/suite/regressions/3592-no-builtin-tools-keeps-extension-tools.test.ts`: with
    // `noTools: "builtin"` all eight built-ins are still REGISTERED
    // (`getAllTools()` → bash/dynamic_tool/edit/find/grep/ls/powershell/read/write) while
    // `getActiveToolNames()` is exactly `["dynamic_tool"]`.
    #[test]
    fn no_builtin_tools_leaves_only_extension_tools_active() {
        let mut builtin = super::SessionConfig::new("/tmp", "/tmp/agent");
        builtin.no_tools = Some(super::NoTools::Builtin);

        // The extension tool alone. `grep`/`find`/`ls`/`powershell` are built-ins pi does not
        // activate by default, but they are still built-ins: `noTools` drops them too.
        assert_eq!(selected(&builtin, None), names(&["static_tool"]));

        // `noTools` short-circuits BEFORE `configuredDefaultToolNames` in that expression, so a
        // `defaultTools` setting cannot resurrect a built-in through this arm.
        assert_eq!(
            selected(&builtin, Some(&names(&["read", "grep", "powershell"]))),
            names(&["static_tool"])
        );

        // `excludeTools` still applies after the selection (`sdk.ts:263`).
        let mut excluded = super::SessionConfig::new("/tmp", "/tmp/agent");
        excluded.no_tools = Some(super::NoTools::Builtin);
        excluded.exclude_tools = names(&["static_tool"]);
        assert!(selected(&excluded, None).is_empty());

        // An explicit allowlist still outranks `noTools` (`options.tools ??` is first): pi keeps
        // `--tools read --no-builtin-tools` meaning `read`.
        let mut allowlisted = super::SessionConfig::new("/tmp", "/tmp/agent");
        allowlisted.no_tools = Some(super::NoTools::Builtin);
        allowlisted.tools = Some(names(&["read"]));
        assert_eq!(selected(&allowlisted, None), names(&["read"]));

        // The sibling mode is unchanged and stricter: `"all"` takes the extension tool too.
        let mut all = super::SessionConfig::new("/tmp", "/tmp/agent");
        all.no_tools = Some(super::NoTools::All);
        assert!(selected(&all, None).is_empty());
    }

    // ---- SEAM-071: `--no-extensions` gates the native built-ins ----------------------------
    //
    // These were RED before the fix: the build loop iterated `self.native_extensions` with no
    // reference to `cfg.no_extensions` at all (the flag reached only `extension_discovery_roots`),
    // so `cyrup --no-extensions` still loaded the permission system, subagents and intercom — and
    // an intercom load starts a detached broker, which is how the suite accumulated 13 immortal
    // broker processes per run.

    /// A minimal native whose only job is to have an id and a tier. The second field is
    /// `is_ambient`: `true` stands in for one of pi's INSTALLED packages (the PATH tier
    /// `noExtensions` collapses), `false` for its INLINE `extensionFactories` tier. Ambience is a
    /// property of the extension, never of its name — see [`super::native_survives_no_extensions`].
    struct StubNative(cyrup_core::ExtensionId, bool);

    #[async_trait::async_trait]
    impl cyrup_ext::NativeExtension for StubNative {
        fn id(&self) -> cyrup_core::ExtensionId {
            self.0.clone()
        }
        fn is_ambient(&self) -> bool {
            self.1
        }
        async fn init(&self, _api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
            Ok(())
        }
        async fn on_event(
            &self,
            _ev: &cyrup_ext::HostEvent,
            _ctx: &cyrup_ext::HostCtx,
        ) -> cyrup_ext::HookOutcome {
            cyrup_ext::HookOutcome::Noop
        }
    }

    /// The four shipped built-ins (pi's INSTALLED-package tier) plus one embedder-supplied
    /// extension (pi's INLINE-factory tier) — the two tiers `noExtensions` treats differently.
    fn stubs() -> Vec<std::sync::Arc<dyn cyrup_ext::NativeExtension>> {
        [
            ("cyrup-permission-system", true),
            ("subagents", true),
            ("subagent-prompt-runtime", true),
            ("cyrup-intercom", true),
            ("an-embedders-own-extension", false),
        ]
        .into_iter()
        .map(|(id, ambient)| {
            std::sync::Arc::new(StubNative(cyrup_core::ExtensionId::from(id), ambient))
                as std::sync::Arc<dyn cyrup_ext::NativeExtension>
        })
        .collect()
    }

    fn ids(v: &[std::sync::Arc<dyn cyrup_ext::NativeExtension>]) -> Vec<String> {
        v.iter().map(|e| e.id().to_string()).collect()
    }

    /// Without the flag nothing changes — the whole point is that this is a FLAG, not a policy.
    #[test]
    fn every_native_loads_without_no_extensions() {
        assert_eq!(
            ids(&super::natives_to_load(stubs(), false, false, false)).len(),
            5
        );
        assert_eq!(
            ids(&super::natives_to_load(stubs(), false, true, false)).len(),
            5
        );
    }

    /// `--no-mcp` is `disabledBuiltinExtensions: ["mcp"]` (`main.ts` @v1.0.4): the named built-in
    /// is not loaded, in any tier, and the rest are. Without the flag nothing is dropped.
    #[test]
    fn a_disabled_builtin_is_not_loaded_and_the_rest_are() {
        let disabled = vec!["subagents".to_string(), "no-such-extension".to_string()];
        assert_eq!(
            ids(&super::without_disabled_builtins(stubs(), &disabled)),
            [
                "cyrup-permission-system",
                "subagent-prompt-runtime",
                "cyrup-intercom",
                "an-embedders-own-extension"
            ]
        );
        assert_eq!(
            ids(&super::without_disabled_builtins(stubs(), &[])).len(),
            5
        );
    }

    /// A ROOT session under `--no-extensions` loads none of the four shipped built-ins. pi's
    /// `noExtensions` reduces the path tier to the explicit `-e` paths (`const extensionPaths =
    /// this.noExtensions ? cliEnabledExtensions : this.mergePaths(...)`, resource-loader.ts:451-452
    /// @v0.83.0), and `@gotgenes/pi-permission-system`, pi-intercom and pi-subagents are ordinary
    /// installed packages living in exactly that tier upstream.
    #[test]
    fn no_extensions_drops_every_ambient_native_in_a_root_session() {
        let kept = ids(&super::natives_to_load(stubs(), true, false, false));
        assert_eq!(
            kept,
            vec!["an-embedders-own-extension".to_string()],
            "the four shipped built-ins go; the inline one stays"
        );
    }

    /// The half that is NOT gated, and the reason SEAM-071's own preferred fix would have been
    /// wrong here: pi loads its inline tier unconditionally — `loadFinalExtensionSet` calls
    /// `loadExtensionFactories(...)` with no `noExtensions` check (`resource-loader.ts:579-581`)
    /// over `[...builtInExtensions, ...(options?.extensionFactories ?? [])]` (`main.ts:523`). An
    /// extension the caller handed over by value is not something a flag about discovery is about.
    /// Ten test files in this workspace rely on exactly that combination.
    #[test]
    fn an_extension_the_embedder_passed_by_hand_survives_no_extensions() {
        for child in [false, true] {
            assert!(
                ids(&super::natives_to_load(stubs(), true, child, false))
                    .contains(&"an-embedders-own-extension".to_string()),
                "inline tier survives (is_subagent_child={child})"
            );
        }
    }

    /// SEAM-074, the regression that motivated it: an INLINE extension whose id happens to collide
    /// with a shipped built-in's still survives `--no-extensions`. The predicate used to match on a
    /// hardcoded id list, so a hand-injected `FailingExt { id: "subagents" }` was silently dropped
    /// from the load — which is exactly what `tests/build_containment_and_flag_diagnostics.rs`'s
    /// `the_failure_reaches_the_panel_and_the_exit_channel_together` does, and it went RED: the
    /// native never loaded, so its init failure reached neither the `[Extension issues]` panel nor
    /// the fatal exit channel. pi cannot have that bug — it separates the tiers by ORIGIN
    /// (`extensionPaths` is collapsed, `resource-loader.ts:451-453` @v0.83.0; `loadExtensionFactories`
    /// is not, `:579-581`), never by name.
    #[test]
    fn an_inline_extension_that_shares_a_built_ins_id_is_still_inline() {
        let inline_double = |id: &str| -> std::sync::Arc<dyn cyrup_ext::NativeExtension> {
            std::sync::Arc::new(StubNative(cyrup_core::ExtensionId::from(id), false))
        };
        for id in [
            "subagents",
            "cyrup-permission-system",
            "cyrup-intercom",
            "subagent-prompt-runtime",
        ] {
            for child in [false, true] {
                assert!(
                    super::native_survives_no_extensions(&inline_double(id), child, false),
                    "a by-value extension named {id} is pi's inline tier and must load \
                     (is_subagent_child={child})"
                );
            }
        }
    }

    /// A subagent CHILD keeps exactly the three pi re-injects by path, and pi-intercom is not one of
    /// them: `runtimeExtensions = [PROMPT_RUNTIME_EXTENSION_PATH, fanout-child when authorized,
    /// permSystemExt]` (pi-subagents v0.47.1 `src/runs/shared/pi-args.ts:413-417`), emitted as
    /// `--extension <path>` right after the `--no-extensions` it pairs with (`:556-560`). Dropping
    /// the permission system here would be a permission gate failing OPEN.
    #[test]
    fn a_subagent_child_keeps_exactly_the_natives_pi_re_injects() {
        let kept = ids(&super::natives_to_load(stubs(), true, true, false));
        assert_eq!(
            kept,
            vec![
                "cyrup-permission-system".to_string(),
                "subagents".to_string(),
                "subagent-prompt-runtime".to_string(),
                "an-embedders-own-extension".to_string(),
            ],
            "load order preserved, intercom dropped"
        );
        assert!(
            !kept.contains(&"cyrup-intercom".to_string()),
            "pi re-injects no intercom"
        );
    }

    /// The exemption is CHILD-only. A root session that happens to have the permission system
    /// installed still drops it under `--no-extensions`, exactly as pi does — pi only re-adds it
    /// from the subagent launcher, never from `main.ts`.
    #[test]
    fn the_child_exemption_does_not_leak_into_a_root_session() {
        let one = |id: &str| -> std::sync::Arc<dyn cyrup_ext::NativeExtension> {
            std::sync::Arc::new(StubNative(cyrup_core::ExtensionId::from(id), true))
        };
        for id in super::SUBAGENT_CHILD_RUNTIME_NATIVES {
            assert!(
                super::native_survives_no_extensions(&one(id), true, false),
                "{id} survives in a child"
            );
            assert!(
                !super::native_survives_no_extensions(&one(id), false, false),
                "{id} drops at the root"
            );
        }
        assert!(!super::native_survives_no_extensions(
            &one("cyrup-intercom"),
            true,
            false
        ));
    }

    /// A subagent child whose `--tools` names `codemode` keeps the `codemode` built-in across
    /// `--no-extensions` (an agent that pins `extensions:` always gets that flag); without the
    /// name it is dropped as before, and a ROOT session never keeps it this way.
    #[test]
    fn a_subagent_child_keeps_codemode_across_no_extensions_only_when_its_tools_name_it() {
        let stubs_with_codemode = || {
            let mut all = stubs();
            all.push(std::sync::Arc::new(StubNative(
                cyrup_core::ExtensionId::from("codemode"),
                true,
            )));
            all
        };
        let kept = ids(&super::natives_to_load(
            stubs_with_codemode(),
            true,
            true,
            true,
        ));
        assert!(kept.contains(&"codemode".to_string()), "kept: {kept:?}");
        let dropped = ids(&super::natives_to_load(
            stubs_with_codemode(),
            true,
            true,
            false,
        ));
        assert!(
            !dropped.contains(&"codemode".to_string()),
            "kept: {dropped:?}"
        );
        let root = ids(&super::natives_to_load(
            stubs_with_codemode(),
            true,
            false,
            true,
        ));
        assert!(
            !root.contains(&"codemode".to_string()),
            "the carve-out is a subagent child's only: {root:?}"
        );
    }

    /// What "its `--tools` names `codemode`" means: an exact name or a `*` pattern in `--tools`,
    /// and not removed by `--exclude-tools`. No `--tools` at all is an unpinned child, which keeps
    /// nothing extra.
    #[test]
    fn a_child_keeps_codemode_when_tools_name_it_and_exclude_tools_does_not() {
        let cfg = |tools: Option<&[&str]>, exclude: &[&str]| {
            let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
            cfg.tools = tools.map(|t| t.iter().map(|s| (*s).to_string()).collect());
            cfg.exclude_tools = exclude.iter().map(|s| (*s).to_string()).collect();
            cfg
        };
        assert!(super::child_keeps_codemode(&cfg(
            Some(&["read", "codemode"]),
            &[]
        )));
        assert!(super::child_keeps_codemode(&cfg(Some(&["code*"]), &[])));
        assert!(!super::child_keeps_codemode(&cfg(
            Some(&["read", "bash"]),
            &[]
        )));
        assert!(!super::child_keeps_codemode(&cfg(None, &[])));
        assert!(!super::child_keeps_codemode(&cfg(
            Some(&["read", "codemode"]),
            &["codemode"]
        )));
    }

    /// CFG-010 (dedupe half) — Pi's `dedupePackages` keeps BOTH entries, delta first, when a
    /// PROJECT entry carrying `autoload: false` collides with a USER one for the same package
    /// identity: "A project entry with autoload=false is a delta over the global entry, so both
    /// are kept (delta first)" (package-manager.ts:1676-1679, code at :1691-1696). Dropping the
    /// global entry turns the delta form inside out — the project entry's patterns become the
    /// ONLY thing that loads instead of a layer over the full package.
    #[test]
    fn a_project_autoload_false_entry_is_a_delta_over_the_global_entry_not_a_replacement() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{"packages":[{"source":"npm:pi-tools","autoload":false,"extensions":["-extensions/foo.ts"]}]}"#,
        );
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, errors) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
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
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{"packages":[{"source":"npm:pi-tools","skills":["skills/a"]}]}"#,
        );
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, _) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
    }

    /// CFG-026. The dedupe key is Pi `getPackageIdentity(source, entry.scope)`
    /// (package-manager.ts:1676-1690 @v0.83.0), which resolves a LOCAL source against that scope's
    /// base dir (`getBaseDirForScope`, :2071-2080). `"./pack"` in both scopes therefore names two
    /// different trees — `<cwd>/.cyrup/pack` and `<agent_dir>/pack` — and pi keeps BOTH.
    ///
    /// RED before the fix: the key was the trimmed source string, the two entries collided, and the
    /// project one replaced the global one, so `<agent_dir>/pack`'s skills/prompts/themes vanished.
    #[test]
    fn the_same_relative_local_source_in_both_scopes_is_two_packages() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Project, r#"{"packages":["./pack"]}"#);
        store.seed(SettingsScope::Global, r#"{"packages":["./pack"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, errors) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            pkgs.len(),
            2,
            "`./pack` means /proj/.cyrup/pack in project scope and /home/u/.cyrup/agent/pack in \
             global scope — two packages, not one; got {pkgs:?}"
        );
        assert_eq!(pkgs[0].scope, InstallScope::Project);
        assert_eq!(pkgs[1].scope, InstallScope::Global);
    }

    /// The other direction of the same key change: an ABSOLUTE local source is scope-independent,
    /// so the two scopes still collide and the project entry wins — the behaviour a raw
    /// source-string key got right and must not lose.
    #[test]
    fn the_same_absolute_local_source_in_both_scopes_is_still_one_package() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Project, r#"{"packages":["/shared/pack"]}"#);
        // A different SPELLING of the same absolute path: `resolvePath` normalizes `.`/`..` before
        // the comparison, which a string key cannot do.
        store.seed(
            SettingsScope::Global,
            r#"{"packages":["/shared/sub/../pack"]}"#,
        );
        let mgr = SettingsManager::load(store, true);

        let (pkgs, _) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
    }

    /// And the version-ignoring half of `getPackageIdentity` (`npm:${parsed.name}`, :1678-1680):
    /// two entries pinning different versions of one npm package are ONE package, where a raw
    /// source-string key loaded both.
    #[test]
    fn two_npm_versions_of_one_package_dedupe_to_the_project_entry() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Project, r#"{"packages":["npm:pi-tools@2"]}"#);
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools@1"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, _) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
        assert_eq!(pkgs[0].source, "npm:pi-tools@2");
    }

    /// PROV-002: the persisted session key for the `max` rung. Both directions go through serde,
    /// so this pins that the enum change actually reaches session replay + the `model:max`
    /// fallback suffix path (`fallback_model` calls `thinking_level_from_str`).
    #[test]
    fn thinking_level_max_round_trips_through_the_persisted_key() {
        use super::{thinking_level_from_str, thinking_level_to_str};
        use cyrup_core::ModelThinkingLevel;

        assert_eq!(thinking_level_to_str(ModelThinkingLevel::Max), "max");
        assert_eq!(
            thinking_level_from_str("max"),
            Some(ModelThinkingLevel::Max)
        );
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
        assert_eq!(
            overlay.get("HTTP_PROXY").map(String::as_str),
            Some("http://proxy.local:8080")
        );
        assert_eq!(
            overlay.get("HTTPS_PROXY").map(String::as_str),
            Some("http://proxy.local:8080")
        );
        // A blank / whitespace / absent setting yields no overlay (ambient env unchanged).
        assert!(http_proxy_overlay(Some("   ")).is_none());
        assert!(http_proxy_overlay(Some("")).is_none());
        assert!(http_proxy_overlay(None).is_none());
    }

    /// PROV-047 — the setting must reach pi's FIRST layer too, the one every non-streaming egress
    /// path consults. Before the fix, `build()` attached the overlay and never called
    /// `configure_http_proxy`, so `httpProxy` reached the streaming wire APIs and nothing else:
    /// OAuth login, silent token refresh, catalog refreshes, the agent proxy transport and
    /// extension HTTP all connected direct and failed on a proxy-only network, with nothing in the
    /// error naming the proxy that had been configured and ignored.
    ///
    /// The clearing half is asserted as well: `applyHttpProxySettings` is re-invoked on every
    /// rebuild (pi calls it again from `main.ts:744`), so a setting that was removed must not leave
    /// the previous proxy installed process-wide.
    #[test]
    fn apply_http_proxy_settings_installs_the_process_global_not_just_the_overlay() {
        let overlay = super::apply_http_proxy_settings(Some("http://proxy.local:8080".to_string()))
            .expect("a non-empty setting still yields pi's second-layer overlay");
        assert_eq!(
            overlay.get("HTTPS_PROXY").map(String::as_str),
            Some("http://proxy.local:8080")
        );
        assert_eq!(
            cyrup_provider::configured_http_proxy().as_deref(),
            Some("http://proxy.local:8080"),
            "the setting must also be installed process-wide, or it reaches only the streams"
        );

        assert!(super::apply_http_proxy_settings(None).is_none());
        assert_eq!(
            cyrup_provider::configured_http_proxy(),
            None,
            "clearing the setting must clear the global, not leave the previous proxy installed"
        );
    }

    // Pi `buildFallbackModel` (model-resolver.ts:163-177): a `--model <custom-id>` on a *known*
    // provider clones that provider's **curated** default (`defaultModelPerProvider` — anthropic ->
    // `claude-opus-4-8`), then overrides id/name. The buggy path cloned the alias-preferred,
    // raw-byte-descending pick (`resolver.provider_default` -> `claude-sonnet-5`), diverging cost
    // (~2.5x) and dropping the base's compat flags. This drives the real `fallback_model` site over
    // an assembled two-model anthropic catalog (opus cost 15/75 vs sonnet 6/30).
    #[test]
    fn fallback_model_clones_curated_default_not_alias_preferred_base() {
        use super::{SessionConfig, fallback_model};
        use cyrup_provider::ModelCost;
        use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};

        let mk = |id: &str, input: f64, output: f64| {
            let mut d = FauxModelDefinition::new(id);
            d.cost = ModelCost {
                input,
                output,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            };
            d
        };
        // Order the alias-preferred pick FIRST (byte-descending `s` > `o` -> sonnet), so the naive
        // `providerModels[0]` fallback is ALSO sonnet — only the curated-default lookup rescues opus.
        let provider = FauxProvider::with_config(FauxConfig {
            provider: "anthropic".into(),
            api: "anthropic".into(),
            models: vec![
                mk("claude-sonnet-5", 6.0, 30.0),
                mk("claude-opus-4-8", 15.0, 75.0),
            ],
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

    // ================================================================================================
    // #2835 — `--tools` must bound the EXTENSION tools too, not only the built-ins.
    //
    // Driven against the real seam rather than a whole `SessionBuilder::build()`, which is this
    // crate's house pattern for tool-surface tests (`host_services.rs`'s `all_tools` pair calls
    // `attach_dynamic_tools` directly for the same reason). `session_surface` below reproduces
    // `build()`'s own three steps verbatim — `select_active_tools`, then
    // `resolve_allowed_tool_names` + `active_tools_filtered`, then the `prompt_tools` merge — so a
    // change to any of them reaches these assertions.
    // ================================================================================================

    struct DynamicToolExt;
    #[async_trait::async_trait]
    impl cyrup_ext::NativeExtension for DynamicToolExt {
        fn id(&self) -> cyrup_core::ExtensionId {
            "dynamic".into()
        }
        async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
            api.register_tool(std::sync::Arc::new(NamedTool(
                "dynamic_tool",
                serde_json::json!({}),
            )));
            Ok(())
        }
        async fn on_event(
            &self,
            _ev: &cyrup_ext::HostEvent,
            _ctx: &cyrup_ext::HostCtx,
        ) -> cyrup_ext::HookOutcome {
            cyrup_ext::HookOutcome::Noop
        }
    }

    /// The active tool set and the prompt-visible tool set a session would build from `cfg`, with
    /// one extension contributing `dynamic_tool`.
    ///
    /// Returns `(active, prompt)`. They are separate because pi's #2835 test asserts on
    /// `getActiveToolNames()` AND on `systemPrompt`; in cyrup both derive from `prompt_tools`, and
    /// the point of the second is that ONE filter at the `active_tools_filtered` call covers both.
    async fn session_surface(cfg: &super::SessionConfig) -> (Vec<String>, Vec<String>) {
        let host = cyrup_ext::ExtensionHost::new(cyrup_ext::HostConfig {
            mode: cyrup_ext::ExtMode::Tui,
            has_ui: false,
            cwd: std::path::PathBuf::from("."),
        });
        host.load_native(std::sync::Arc::new(DynamicToolExt))
            .await
            .unwrap();

        // `builder.rs`: the built-in selection, then the session-level allow/deny pair.
        let visible: Vec<std::sync::Arc<dyn cyrup_core::Tool>> = ALL_BUILTIN_TOOLS
            .iter()
            .copied()
            .map(|name| {
                std::sync::Arc::new(NamedTool(name, serde_json::json!({})))
                    as std::sync::Arc<dyn cyrup_core::Tool>
            })
            .collect();
        let base_tools = super::select_active_tools(&visible, cfg, None);
        let allowed = super::resolve_allowed_tool_names(cfg);
        let excluded: std::collections::HashSet<String> =
            cfg.exclude_tools.iter().cloned().collect();

        let active_tools = host
            .active_tools_filtered(&base_tools, allowed.as_ref(), &excluded)
            .unwrap();

        // `builder.rs`'s `prompt_tools`: base first, then each extension tool `set` over it by name.
        let mut prompt_tools: Vec<std::sync::Arc<dyn cyrup_core::Tool>> = base_tools.clone();
        for t in active_tools.iter() {
            if let Some(slot) = prompt_tools.iter_mut().find(|b| b.name() == t.name()) {
                *slot = t.clone();
            } else {
                prompt_tools.push(t.clone());
            }
        }

        let mut active: Vec<String> = active_tools.iter().map(|t| t.name().to_string()).collect();
        let mut prompt: Vec<String> = prompt_tools.iter().map(|t| t.name().to_string()).collect();
        active.sort();
        prompt.sort();
        (active, prompt)
    }

    /// pi #2835, "allows only explicitly listed built-in and extension tools". The allowlist names
    /// one built-in and one EXTENSION tool; nothing else survives, in the prompt either.
    ///
    /// **Failed before this change**: `active_tools` appended every registered extension tool
    /// unconditionally, so `dynamic_tool` arrived whether or not it was listed — and so did every
    /// other ambient extension tool.
    #[tokio::test]
    async fn a_tools_allowlist_filters_extension_tools() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.tools = Some(names(&["read", "dynamic_tool"]));

        let (active, prompt) = session_surface(&cfg).await;
        assert_eq!(active, names(&["dynamic_tool", "read"]));
        assert_eq!(
            prompt,
            names(&["dynamic_tool", "read"]),
            "one filter must narrow the system prompt too: `prompt_tools` starts from the \
             already-filtered base and grows only by `active_tools` entries"
        );
        assert!(!prompt.contains(&"bash".to_string()));
        assert!(!prompt.contains(&"edit".to_string()));
    }

    /// pi #2835, "disables all tools when the allowlist is empty". `tools: []` is a CONFIGURED empty
    /// allowlist (`Some(∅)`), not "unset" — it denies everything, extension tools included.
    ///
    /// **Failed before this change**: the built-ins went to zero and every extension tool survived.
    #[tokio::test]
    async fn an_empty_allowlist_disables_extension_tools_too() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.tools = Some(Vec::new());

        let (active, prompt) = session_surface(&cfg).await;
        assert!(active.is_empty(), "got {active:?}");
        assert!(
            prompt.is_empty(),
            "the prompt would say `(none)`; got {prompt:?}"
        );
    }

    /// The ONE way to over-filter, guarded. `--no-builtin-tools` empties the BUILT-IN selection but
    /// leaves pi's `allowedToolNames` `undefined` (`sdk.ts:258`), so every extension tool stays
    /// active. Narrowing this arm to `Some(∅)` would break plan-mode and the permission companion.
    #[tokio::test]
    async fn no_builtin_tools_still_keeps_extension_tools() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.no_tools = Some(super::NoTools::Builtin);

        assert!(
            super::resolve_allowed_tool_names(&cfg).is_none(),
            "`NoTools::Builtin` must resolve to NO allowlist, not to an empty one"
        );
        let (active, _) = session_surface(&cfg).await;
        assert_eq!(active, names(&["dynamic_tool"]));
    }

    /// `NoTools::All` is the arm that DOES deny everything — the pair that proves the two variants
    /// are not interchangeable.
    #[tokio::test]
    async fn no_tools_all_denies_extension_tools() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.no_tools = Some(super::NoTools::All);

        assert_eq!(
            super::resolve_allowed_tool_names(&cfg),
            Some(std::collections::HashSet::new())
        );
        let (active, _) = session_surface(&cfg).await;
        assert!(active.is_empty(), "got {active:?}");
    }

    /// Patterns reach the extension tools too: the allowlist's `dyn*` names `dynamic_tool`, and the
    /// denylist's removes it.
    #[tokio::test]
    async fn patterns_bound_extension_tools() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.tools = Some(names(&["read", "dyn*"]));
        let (active, _) = session_surface(&cfg).await;
        assert_eq!(active, names(&["dynamic_tool", "read"]));

        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.exclude_tools = names(&["dyn*"]);
        let (active, _) = session_surface(&cfg).await;
        assert!(
            !active.contains(&"dynamic_tool".to_string()),
            "got {active:?}"
        );
    }

    /// pi `excludedToolNames = options.excludeTools` (`sdk.ts:259`) applies to extension tools with
    /// no allowlist in play at all.
    #[tokio::test]
    async fn exclude_tools_removes_an_extension_tool() {
        let mut cfg = super::SessionConfig::new("/tmp", "/tmp/agent");
        cfg.exclude_tools = names(&["dynamic_tool"]);

        let (active, _) = session_surface(&cfg).await;
        assert!(
            !active.contains(&"dynamic_tool".to_string()),
            "got {active:?}"
        );
        assert!(active.contains(&"read".to_string()), "got {active:?}");
    }
}
