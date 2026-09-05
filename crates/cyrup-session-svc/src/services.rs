//! `AgentSessionServices` — the cwd-bound subsystems the facade assembles and the session owns
//! (arch-11 §3.3). Recreated per session by the builder; exposed read-only so front-ends can
//! inspect the wired stack (settings, auth, resources, the extension host) without re-deriving it.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_config::{AuthStore, SettingsManager};
use cyrup_ext::ExtensionHost;
use cyrup_provider::Model;
use cyrup_resources::{ResourceDiagnostic, ResourceRegistry};
use cyrup_session::prompt::ContextStore;

/// One extension that failed to load, kept per-path (Pi `LoadExtensionsResult.errors`, surfaced as
/// `{type: "error", message, path}` in the `[Extension issues]` startup block,
/// interactive-mode.ts:1660-1665).
#[derive(Clone, Debug)]
pub struct ExtensionLoadDiagnostic {
    pub path: PathBuf,
    pub error: String,
    /// Whether this is a genuine LOAD FAILURE — the class Pi lifts onto `runtime.diagnostics` as
    /// `{type:"error", message:'Failed to load extension "<path>": <err>'}` (main.ts:735-738) and
    /// then exits 1 on, in EVERY mode (main.ts:843-849). Read by
    /// [`crate::AgentSessionRuntime::diagnostics`]; the `[Extension issues]` panel shows the whole
    /// vector regardless.
    ///
    /// `false` only for the project-trust skip, which Pi filters out before its loader ever runs
    /// (see [`cyrup_ext::LoadError::fatal`]).
    pub fatal: bool,
}

/// Everything that went wrong (or was shadowed) while this session's resources were discovered and
/// its extensions loaded — Pi's `showLoadedResources` diagnostics half (interactive-mode.ts:
/// 1641-1690), which prints `[Skill conflicts]`, `[Prompt conflicts]`, `[Extension issues]` and
/// `[Theme conflicts]` at startup **even under `quietStartup`** (`showDiagnosticsWhenQuiet: true`,
/// `:1769`).
///
/// The builder used to compute both halves and throw them away (`discover()`'s `DiscoveryReport`
/// carries `diagnostics`; `discover_and_load` returns per-path `errors`), so a shadowed skill, a
/// configured-but-missing prompt path or an extension that failed to instantiate was completely
/// invisible in the TUI. Retaining them here is the data seam the front-end reads (TUI-006).
#[derive(Clone, Debug, Default)]
pub struct StartupDiagnostics {
    /// Structured skill/prompt/theme diagnostics from the discovery pass, split by
    /// `ResourceDiagnostic::resource_type` at render time.
    pub resources: Vec<ResourceDiagnostic>,
    /// Extensions that did not load (world-version mismatch, untrusted project-local, load fault).
    ///
    /// Two consumers, both required (EXT-S01): the `[Extension issues]` startup panel renders the
    /// whole vector, and [`crate::AgentSessionRuntime::diagnostics`] republishes the
    /// [`ExtensionLoadDiagnostic::fatal`] subset as `type: "error"` so the bin reports it on stderr
    /// and exits 1 in every mode, exactly as Pi does (main.ts:735-738 → :843-849). The panel alone
    /// is not enough: it is interactive-only, so a print/json/rpc run would exit 0 in silence.
    pub extensions: Vec<ExtensionLoadDiagnostic>,
    /// `models.json` problems: a load/parse failure, or a provider block rejected during
    /// composition. Pi keeps the exact same channel — `ModelConfig.getError()` for the whole file
    /// (model-config.ts:251/:261/:271) plus `ModelRuntime.compositionErrors` per provider
    /// (model-runtime.ts:104) — and starts normally with the built-in registry either way
    /// (CFG-002).
    pub models: Vec<String>,
    /// CLI extension-flag reconciliation ERRORS (SEAM-S01): `Unknown option(s): --foo` and
    /// `Extension flag "--foo" requires a value`, produced once the loaded extensions' registered
    /// flag specs are known (Pi `applyExtensionFlagValues`, agent-session-services.ts:98-125).
    ///
    /// Unlike every other field here these are `type: "error"` in Pi and are FATAL at the bin tier:
    /// they merge into `services.diagnostics` (:182) → `runtime.diagnostics` → `reportDiagnostics` +
    /// `process.exit(1)` (main.ts:843-848). Surfaced through
    /// [`crate::AgentSessionRuntime::diagnostics`] rather than the `[Extension issues]` panel.
    pub flags: Vec<String>,
}

impl StartupDiagnostics {
    /// Whether there is anything at all to report.
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
            && self.extensions.is_empty()
            && self.models.is_empty()
            && self.flags.is_empty()
    }
}

/// Everything bound to a single cwd / session (arch-11 §3.3).
pub struct AgentSessionServices {
    pub cwd: PathBuf,
    /// The agent config dir (`~/.cyrup`), the root of the trust store, sessions, and auth (Pi
    /// `CONFIG_DIR`). Retained so front-ends can derive the trust-store path (`agent_dir/trust.json`,
    /// Pi `EnvVars::trustPath`) and the sessions root for `/trust` and `/resume` without re-resolving
    /// config (an additive L6↔L5 data seam — read-only).
    pub agent_dir: PathBuf,
    /// The directory THIS session's `.jsonl` files live in — Pi's `SessionManager.sessionDir`,
    /// exposed as `getSessionDir()` (session-manager.ts:999-1001) and fixed once at manager
    /// construction: an explicit `--session-dir` verbatim, else the resumed file's own parent, else
    /// the cwd-encoded default `<agent_dir>/sessions/--<encoded-cwd>--`. It is therefore NOT always
    /// derivable from `agent_dir` + `cwd`, which is why it is carried rather than recomputed — the
    /// `/resume` listing seam ([`crate::session::AgentSession::list_sessions`]) reads exactly this
    /// directory, mirroring Pi's `SessionManager.list(getCwd(), getSessionDir())`
    /// (interactive-mode.ts:4867).
    pub session_dir: PathBuf,
    /// The home dir used for trust-requiring-resource detection (defaults to `agent_dir`).
    pub home: PathBuf,
    /// Layered settings (global ◁ project ◁ cli), reflecting the resolved trust decision.
    pub settings: SettingsManager,
    /// Whether the project scope is trusted (gates project settings + post-trust resources).
    pub project_trusted: bool,
    /// Credential store (request-time auth resolution lives in `cyrup-config`).
    pub auth: Arc<AuthStore>,
    /// Discovered resources snapshot (skills / prompts / themes).
    pub resources: Arc<ResourceRegistry>,
    /// Load-time problems collected while assembling this session (TUI-006). See
    /// [`StartupDiagnostics`].
    pub startup_diagnostics: StartupDiagnostics,
    /// The immutable, credential-blind `<agent_dir>/models.json` snapshot (Pi `ModelConfig`,
    /// model-config.ts:232-279, loaded once by `ModelRuntime.create`, model-runtime.ts:137-139).
    /// Composed over the built-in registry by
    /// [`crate::session::AgentSession::full_model_catalog`], so a user-declared provider or model —
    /// or a `baseUrl`/`compat`/`modelOverrides` patch on a built-in — is live in the session
    /// (CFG-002). Empty when the file is absent or unreadable; failures land in
    /// [`StartupDiagnostics::models`].
    pub model_config: Arc<cyrup_config::ModelFile>,
    /// The persisted pi.dev model-catalog overlay for this session, loaded ONCE from
    /// `<agent_dir>/models-store.json` at build time (DRIFT-007; Pi `ModelRuntime`'s
    /// `modelsStore` + `withRemoteCatalog`, model-runtime.ts:139-151).
    ///
    /// Loaded from DISK ONLY — building a session never touches the network. `None` (no cache, no
    /// agent dir, stale-vs-builtins, unreadable file) means "embedded catalogs only", i.e. exactly
    /// the pre-DRIFT-007 behavior. The overlay can only add or replace models by id, so it is
    /// structurally incapable of shrinking the registry
    /// ([`cyrup_provider::remote_catalog::merge_models`]).
    ///
    /// Held as an `Arc` because [`crate::session::AgentSession::full_model_catalog`] is SYNC and hot:
    /// it rebuilds the registry on every read, so the overlay must already be in memory.
    pub catalog_overlay: Option<Arc<cyrup_provider::CatalogOverlay>>,
    /// Session-scoped context cache (context files + skill pointers).
    pub context: Arc<ContextStore>,
    /// The extension host with native built-ins loaded; both seams are wired to the agent.
    pub ext_host: Arc<ExtensionHost>,
    /// The shared model-registry sink bound to `ext_host` (Pi `bindCore`): guest-registered providers
    /// realized as concrete `Provider`s. The session UNIONs their catalogs into the model registry and
    /// installs the owning provider on a matching `set_model` (arch-08 §5.6). Empty until a guest
    /// `registerProvider` fires.
    pub guest_providers: Arc<crate::guest_providers::GuestProviderRegistry>,
    /// The resolved active model for this session, or `None` when the session launched with no
    /// model — pi `AgentSession.model: Model | undefined`, the state `findInitialModel` produces
    /// when nothing is configured (sdk.ts:216-218 ⇒ `modelFallbackMessage`, a banner rather than an
    /// error; model-resolver.ts:648-650). See [`crate::session::AgentSession::model`] (SEAM-075).
    pub model: Option<Model>,
    /// The assembled system prompt for this session (arch-06).
    pub system_prompt: String,
    /// The concrete [`cyrup_ext::host::HostServices`] backend wired to this session's provider +
    /// active model (arch-08 §5.6). A loaded WASM extension's `models`/`session`/`control` imports
    /// resolve through this instead of the deny-all default.
    pub host_services: Arc<crate::host_services::LiveHostServices>,
    /// Captured extension CLI flag values threaded from the CLI (Pi `extensionFlagValues`,
    /// main.ts:634). Read-only seam a loaded extension consumes via `applyExtensionFlagValues`.
    pub extension_flag_values: Vec<(String, crate::builder::ExtensionFlagValue)>,
    /// **This session's** filesystem seam — the same `Arc<dyn FsOps>`
    /// [`crate::builder::SessionBuilder::build`] hands the tool registry, `TraversalFs` /
    /// `ProtectedFs` wrappers included.
    ///
    /// Retained (gap-analysis 15 `ACP-156`) so a *front-end* can read a file the way the session's
    /// own tools would. It is not a convenience: with `confine_to_cwd` set, `TraversalFs::read`
    /// hard-denies a path outside the root, so a front-end that reached for `std::fs` instead
    /// would transmit bytes this session's backend refuses to open — which is exactly what the ACP
    /// adapter's pre/post-mutation diff snapshot would have done. The only in-tree consumer today
    /// is `cyrup_acp::sessions`' snapshot read; the tool registry keeps its own clone, so this is
    /// a second handle on one backend rather than a new authority.
    pub fs: Arc<dyn cyrup_tools::FsOps>,
}
