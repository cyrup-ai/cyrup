//! `buildMcpPanelCallbacks` + `openMcpSetup`'s callback object — the adapter between the `/mcp`
//! dispatcher and the two panels (MCP-387, MCP-392).
//!
//! [`crate::ui`] declares [`crate::ui::McpPanelCallbacks`] and [`crate::ui::SetupPanelCallbacks`] and derives no state of
//! its own; [`crate::commands`] owns the switch. This module is the layer between them: it reads
//! [`McpState`]'s connection map, the credential store and the config ladder, and it is the only
//! place in the crate that answers a panel's questions.
//!
//! # Why the callbacks hold a `Weak` and not the extension
//!
//! Both async members of [`crate::ui::McpPanelCallbacks`] return `BoxFuture<'static, _>`, so nothing here can
//! borrow. Each object therefore owns its inputs — an `Arc<McpState>`, a cloned
//! [`cyrup_ext::HostCtx`], the resolved [`crate::dirs::McpDirs`] — plus a
//! [`std::sync::Weak`] back to the extension, upgraded per call. Strong would be a cycle: the
//! command arm that built the callbacks is blocked on the panel that holds them, so a strong handle
//! would keep the extension alive for exactly as long as the object it owns. This is the same shape
//! `McpExtension::install_surface_sync` uses for the metadata listener.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use cyrup_ext::native::HostCtx;
use futures::future::BoxFuture;
use indexmap::IndexMap;

use crate::config::{
    ConfigWritePreview, DiscoveryKind, ImportKind, KnownServerPreset, McpStandardConfigSummary,
};
use crate::credentials::OAuthCredentialStatus;
use crate::dirs::{McpDirs, ServerCacheEntry};
use crate::extension::McpExtension;
use crate::onboarding::OnboardingState;
use crate::state::McpState;
use crate::ui::{
    AddServerOutcome, AdoptImportsOutcome, ConnectionStatus, McpAuthResult, McpPanelCallbacks,
    SetupPanelCallbacks,
};

/// The extension was dropped while a panel it opened was still running a job.
///
/// Unreachable in practice — the command arm that opened the panel is blocked inside the same
/// extension — but the `Weak` upgrade has to answer something, and a sentence beats a panic.
const EXTENSION_GONE: &str = "MCP: the extension was dropped while the panel was open.";

// =================================================================================================
// MCP-392 — `buildMcpPanelCallbacks(state, config, ctx)` (`commands.ts:483-537`)
// =================================================================================================

/// The `/mcp` browser panel's and `/mcp-auth` picker's callbacks.
///
/// Built once per panel open, exactly as upstream's factory runs once per open — which is what
/// makes [`Self::auth_status_failures`] safe to keep out of `McpState::failure_messages`.
pub(crate) struct PanelCallbacks {
    ext: Weak<McpExtension>,
    state: Arc<McpState>,
    ctx: HostCtx,
    dirs: McpDirs,
    /// `const authStatusFailures = new Map<string, string>()` (`commands.ts:491`).
    ///
    /// **Panel-only diagnostics, per open.** When the credential store cannot be read,
    /// [`Self::connection_status`] reports `failed` and records why *here* rather than in
    /// `McpState::failure_messages`, because a status inspection must not poison the 60-second
    /// connect backoff `crate::live::failure_age_seconds` drives. A panel opened twice starts with
    /// an empty map both times.
    auth_status_failures: Mutex<IndexMap<String, String>>,
}

impl PanelCallbacks {
    pub(crate) fn new(
        ext: Weak<McpExtension>,
        state: Arc<McpState>,
        ctx: HostCtx,
        dirs: McpDirs,
    ) -> Self {
        Self {
            ext,
            state,
            ctx,
            dirs,
            auth_status_failures: Mutex::new(IndexMap::new()),
        }
    }

    /// Record a per-open diagnostic and answer `failed`.
    fn record_auth_failure(&self, server: &str, message: String) -> ConnectionStatus {
        if let Ok(mut map) = self.auth_status_failures.lock() {
            map.insert(server.to_string(), message);
        }
        ConnectionStatus::Failed
    }
}

impl McpPanelCallbacks for PanelCallbacks {
    /// `getConnectionStatus(serverName)` (`commands.ts:499-529`) — eight rungs, first match wins.
    ///
    /// The order is load-bearing and is upstream's: disabled, then a URL that will not resolve,
    /// then the credential store, then the live connection, then the failure window, then idle. In
    /// particular the credential check runs **before** the connection status, so a server that is
    /// connected on a token that has since been deleted still reads `needs-auth`.
    fn connection_status(&self, server: &str) -> ConnectionStatus {
        // `authStatusFailures.delete(serverName)` — cleared first, so a store that recovered
        // between two repaints stops reporting yesterday's diagnostic.
        if let Ok(mut map) = self.auth_status_failures.lock() {
            map.shift_remove(server);
        }
        let Some(definition) = self.state.config.mcp_servers.get(server) else {
            // `isServerDisabled(undefined)` is falsy upstream, so an unknown name falls THROUGH
            // rather than reporting `disabled`. With no definition there is no URL to resolve and
            // no connection to look up, so the ladder bottoms out.
            return ConnectionStatus::Idle;
        };
        if definition.is_disabled() {
            return ConnectionStatus::Disabled;
        }

        // `resolveServerUrl` CAN THROW, and upstream maps the throw to `failed` — not to `idle`,
        // and not to a propagated error: the panel is a status view and must render something.
        let Ok(server_url) = crate::credentials::resolve_server_url(
            definition.url.as_deref(),
            &crate::credentials::process_env(),
        ) else {
            return ConnectionStatus::Failed;
        };

        // The four-condition OAuth guard: `auth === "oauth"`, a resolved URL, `oauth !== false`,
        // and a grant type that is not `client_credentials`. Three of the four are
        // `uses_oauth_authorization_code`; the URL is the fourth and is already in hand.
        if let Some(url) = server_url.as_deref()
            && definition.uses_oauth_authorization_code()
        {
            let store = self.state.manager.auth_store().unwrap_or_else(|| {
                crate::credentials::McpAuthStore::new(
                    self.dirs.clone(),
                    self.state.auth_storage_options.clone(),
                )
            });
            match store.inspect_auth_for_url(server, url) {
                Ok(OAuthCredentialStatus::Unavailable { message }) => {
                    return self.record_auth_failure(server, message);
                }
                Ok(OAuthCredentialStatus::Absent) => return ConnectionStatus::NeedsAuth,
                // `!authStatus.entry.tokens`. `AuthEntry` carries no `tokens` field — the tokens
                // are PROJECTED out of `credentials` by `crate::oauth::project_tokens`, the same
                // projection `crate::oauth::get_auth_status` uses. Testing `credentials.is_none()`
                // instead would call a half-written entry authenticated.
                Ok(OAuthCredentialStatus::Present(entry))
                    if entry
                        .credentials
                        .as_ref()
                        .and_then(crate::oauth::project_tokens)
                        .is_none() =>
                {
                    return ConnectionStatus::NeedsAuth;
                }
                Ok(OAuthCredentialStatus::Present(_)) => {}
                // A store error that is not "unavailable" has no upstream arm: `inspectAuthForUrl`
                // throws and nothing catches it. Recording it like the unavailable case keeps the
                // reason on screen instead of degrading to a silent `idle`.
                Err(error) => return self.record_auth_failure(server, error.to_string()),
            }
        }

        // `crate::lifecycle::ConnectionStatus`, the three-variant one `ServerConnection::status()`
        // returns — NOT this function's own six-variant return type, which shares its name.
        match self
            .state
            .manager
            .get_connection(server)
            .map(|c| c.status())
        {
            Some(crate::lifecycle::ConnectionStatus::NeedsAuth) => ConnectionStatus::NeedsAuth,
            Some(crate::lifecycle::ConnectionStatus::Connected) => ConnectionStatus::Connected,
            _ if crate::live::failure_age_seconds(&self.state, server).is_some() => {
                ConnectionStatus::Failed
            }
            _ => ConnectionStatus::Idle,
        }
    }

    /// `getFailureMessage(serverName)` — `authStatusFailures.get(name) ?? getFailureMessage(state, name)`.
    ///
    /// The panel-only diagnostic WINS over the connect-failure text, because it is the more
    /// specific of the two and the reason the status is `failed` at all.
    fn failure_message(&self, server: &str) -> Option<String> {
        self.auth_status_failures
            .lock()
            .ok()
            .and_then(|map| map.get(server).cloned())
            .or_else(|| crate::live::failure_message(&self.state, server))
    }

    /// `canAuthenticate(serverName)` — the BROAD predicate, deliberately.
    ///
    /// This is `supports_oauth`, not [`crate::config::ServerEntry::uses_oauth_authorization_code`]:
    /// the question is whether pressing `ctrl+a` on this row could start a flow, and a URL server
    /// with no `auth` key can. `connection_status` asks the narrower question because it is about a
    /// token that should already exist.
    fn can_authenticate(&self, server: &str) -> bool {
        self.state
            .config
            .mcp_servers
            .get(server)
            .is_some_and(|definition| {
                !definition.is_disabled() && crate::oauth::supports_oauth(definition)
            })
    }

    /// `refreshCacheAfterReconnect(serverName)` — re-reads the WHOLE cache file, every call.
    ///
    /// That is not an oversight to optimise away: it is how the panel observes what
    /// `update_metadata_cache` flushed during the reconnect that just finished.
    fn refresh_cache_after_reconnect(&self, server: &str) -> Option<ServerCacheEntry> {
        // `crate::dirs::load_metadata_cache`, NOT `crate::registration`'s: the crate carries two
        // `MetadataCache`/`ServerCacheEntry` pairs, and the panel is typed against `crate::dirs`'.
        // The registration one reads the same file and would not compile here — which is the only
        // reason the confusion is catchable.
        crate::dirs::load_metadata_cache(&self.dirs.metadata_cache())?
            .servers
            .get(server)
            .cloned()
    }

    /// `authenticate(serverName)` — delegated whole to `authenticateServer`.
    ///
    /// Every guard, message and level lives there and none is repeated here.
    fn authenticate(&self, server: String) -> BoxFuture<'static, Result<McpAuthResult, String>> {
        let (ext, state, ctx) = (self.ext.clone(), Arc::clone(&self.state), self.ctx.clone());
        Box::pin(async move {
            let Some(ext) = ext.upgrade() else {
                return Err(EXTENSION_GONE.to_string());
            };
            let outcome = ext.authenticate_server(&state, &server, &ctx).await;
            // An EMPTY message is the abort arm's "say nothing" and must become `None`, not a
            // blank line in the panel's notice row.
            Ok(McpAuthResult {
                ok: outcome.ok,
                message: (!outcome.message.is_empty()).then_some(outcome.message),
            })
        })
    }

    /// `reconnect(serverName)` — `reconnectServer`'s boolean, over the shared connect body.
    fn reconnect(&self, server: String) -> BoxFuture<'static, Result<bool, String>> {
        let ext = self.ext.clone();
        Box::pin(async move {
            let Some(ext) = ext.upgrade() else {
                return Err(EXTENSION_GONE.to_string());
            };
            // No proxy context means MCP never finished initializing. `false` is
            // `reconnectServer`'s own "did not reconnect", which the panel renders as a failed
            // row — the right answer, and not an error dialog.
            let Some(ctx) = ext.proxy_ctx() else {
                return Ok(false);
            };
            Ok(ext.reconnect_one(&ctx, &server).await.ok)
        })
    }
}

// =================================================================================================
// MCP-387 — `openMcpSetup`'s callback object (`commands.ts:440-478`)
// =================================================================================================

/// The `/mcp setup` panel's callbacks.
///
/// Every member is a call into [`crate::config::ConfigContext`] or [`crate::onboarding`]; nothing
/// here writes a config file itself.
pub(crate) struct SetupCallbacks {
    dirs: McpDirs,
    /// `$HOME` as the extension resolved it, so a test fixture's home reaches the ladder.
    home: Option<PathBuf>,
    /// `discovery.fingerprint`, captured at open — what [`Self::mark_setup_completed`] stamps.
    fingerprint: String,
    /// `options.includeHostConfigs`. `/mcp setup` leaves it on; the zero-servers delegation from
    /// `/mcp` turns it off, and the two re-reads below must use the same value the panel opened
    /// with or the RepoPrompt proposal could change under the cursor.
    include_host_configs: bool,
    /// Upstream's `let configChanged = false`, closed over by the callbacks and read by
    /// `openMcpSetup` after the panel resolves.
    ///
    /// An `Arc<AtomicBool>` rather than a field: the four write members return
    /// `BoxFuture<'static, _>` and so cannot borrow `self`.
    config_changed: Arc<AtomicBool>,
}

impl SetupCallbacks {
    pub(crate) fn new(
        dirs: McpDirs,
        home: Option<PathBuf>,
        fingerprint: String,
        include_host_configs: bool,
    ) -> Self {
        Self {
            dirs,
            home,
            fingerprint,
            include_host_configs,
            config_changed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The config ladder, rebuilt per call.
    ///
    /// Deliberately **not** a stored `ConfigContext` and deliberately not routed through
    /// `McpExtension::config_context()`: this is the same two lines that method runs, over inputs
    /// this object owns outright. Owning them is what lets every member below be infallible —
    /// there is no extension handle to upgrade, so the four synchronous `preview_*` members, which
    /// run from inside `render` on every frame and have nowhere to report a failure, cannot have
    /// one. Rebuilding rather than caching preserves the re-read `--mcp-config` semantics
    /// (upstream's `pi.getFlag("mcp-config")`, re-read per call).
    fn context(&self) -> crate::config::ConfigContext {
        let explicit = crate::config::config_path_from_argv(std::env::args()).map(PathBuf::from);
        let mut context = crate::config::ConfigContext::new(self.dirs.clone(), explicit.as_deref());
        if let Some(home) = self.home.clone() {
            context = context.with_home(home);
        }
        context
    }

    /// Whether any callback wrote something. Read after the panel closes.
    pub(crate) fn config_changed(&self) -> bool {
        self.config_changed.load(Ordering::Acquire)
    }

    /// `getMcpDiscoverySummary(configOverridePath, ctx.cwd, options).repoPrompt`, re-read.
    ///
    /// Upstream re-runs the whole discovery inside both RepoPrompt members rather than closing over
    /// the summary, so a `.mcp.json` created since the panel opened is honoured. Kept.
    fn repo_prompt(&self) -> Option<(PathBuf, String, crate::config::ServerEntry)> {
        let mut diagnostics = Vec::new();
        let discovery = self
            .context()
            .mcp_discovery_summary(self.include_host_configs, &mut diagnostics);
        let repo_prompt = discovery.repo_prompt;
        // All three or none: upstream's `!entry || !targetPath || !serverName` refuses as one.
        Some((
            repo_prompt.target_path?,
            repo_prompt.server_name?,
            repo_prompt.entry?,
        ))
    }
}

impl SetupPanelCallbacks for SetupCallbacks {
    fn preview_imports(&self, imports: &[ImportKind]) -> ConfigWritePreview {
        self.context().preview_compatibility_imports(imports)
    }

    fn preview_starter_project(&self) -> ConfigWritePreview {
        self.context().preview_starter_project_config()
    }

    fn preview_repo_prompt(&self) -> Option<ConfigWritePreview> {
        let (path, name, entry) = self.repo_prompt()?;
        Some(crate::config::preview_shared_server_entry(
            &path, &name, &entry,
        ))
    }

    /// `previewKnownServer(preset)` — always against the PROJECT file, never the discovered path a
    /// RepoPrompt add would use, and keyed by `preset.id` rather than its display name.
    fn preview_known_server(&self, preset: &KnownServerPreset) -> ConfigWritePreview {
        crate::config::preview_shared_server_entry(
            &self.context().project_path(),
            preset.id,
            &preset.entry,
        )
    }

    /// `adoptImports(imports)` — and `configChanged` is set only for a NON-EMPTY `added`.
    ///
    /// An adopt where every requested kind was already listed wrote nothing, so flagging it would
    /// trigger a `/reload` for a no-op.
    fn adopt_imports(
        &self,
        imports: Vec<ImportKind>,
    ) -> BoxFuture<'static, Result<AdoptImportsOutcome, String>> {
        let (context, changed) = (self.context(), Arc::clone(&self.config_changed));
        Box::pin(async move {
            let result = context
                .ensure_compatibility_imports(&imports)
                .map_err(|error| error.to_string())?;
            if !result.added.is_empty() {
                changed.store(true, Ordering::Release);
            }
            Ok(AdoptImportsOutcome {
                added: result.added,
                path: result.path,
            })
        })
    }

    fn scaffold_project_config(&self) -> BoxFuture<'static, Result<PathBuf, String>> {
        let (context, changed) = (self.context(), Arc::clone(&self.config_changed));
        Box::pin(async move {
            let path = context
                .write_starter_project_config()
                .map_err(|error| error.to_string())?;
            changed.store(true, Ordering::Release);
            Ok(path)
        })
    }

    fn add_repo_prompt(&self) -> BoxFuture<'static, Result<AddServerOutcome, String>> {
        let changed = Arc::clone(&self.config_changed);
        // Resolved BEFORE the future, so the discovery re-read happens at the keystroke rather
        // than whenever the panel's job queue gets to it.
        let resolved = self.repo_prompt();
        Box::pin(async move {
            let (path, server_name, entry) = resolved.ok_or_else(|| {
                "RepoPrompt is not available to add from this setup screen.".to_string()
            })?;
            let written = crate::config::write_shared_server_entry(&path, &server_name, &entry)
                .map_err(|error| error.to_string())?;
            changed.store(true, Ordering::Release);
            Ok(AddServerOutcome {
                path: written,
                server_name,
            })
        })
    }

    /// `addKnownServer(preset)` — writes under `preset.id` and reports `preset.name`.
    ///
    /// The asymmetry is upstream's and is user-visible: adding "Chrome DevTools" writes the key
    /// `chrome-devtools` and notices `Added Chrome DevTools to …` (MCP-379).
    fn add_known_server(
        &self,
        preset: KnownServerPreset,
    ) -> BoxFuture<'static, Result<AddServerOutcome, String>> {
        let (context, changed) = (self.context(), Arc::clone(&self.config_changed));
        Box::pin(async move {
            let path = crate::config::write_shared_server_entry(
                &context.project_path(),
                preset.id,
                &preset.entry,
            )
            .map_err(|error| error.to_string())?;
            changed.store(true, Ordering::Release);
            Ok(AddServerOutcome {
                path,
                server_name: preset.name.to_string(),
            })
        })
    }

    fn open_path(&self, path: PathBuf) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move { crate::ui::open_path(&path).await })
    }

    /// `markSetupCompleted()` — called after EVERY successful write, so the stamp is idempotent and
    /// a write failure never marks setup done.
    fn mark_setup_completed(&self) {
        if let Err(error) = crate::onboarding::mark_setup_completed(
            &self.dirs.onboarding_state(),
            Some(&self.fingerprint),
        ) {
            // Upstream's `persistSetupCompleted` swallows its own write failure; the state file is
            // a hint store, and losing the stamp costs one repeated hint, not correctness.
            tracing::debug!("MCP: could not record setup completion: {error}");
        }
    }
}

// =================================================================================================
// `buildSharedConfigNoticeLines` (`commands.ts:388-405`)
// =================================================================================================

/// The two-line shared-config notice, and the fingerprint that retires it.
///
/// Empty lines and a `None` fingerprint travel together — "say nothing, stamp nothing" — which is
/// why this returns a pair rather than two independently-computed values: stamping a fingerprint
/// for a notice that was never rendered would suppress the notice forever.
pub(crate) fn shared_config_notice(
    summary: &McpStandardConfigSummary,
    onboarding: &OnboardingState,
) -> (Vec<String>, Option<String>) {
    if !summary.has_shared_servers || onboarding.shared_config_hint_shown {
        return (Vec::new(), None);
    }
    let sources = summary
        .sources
        .iter()
        .filter(|source| source.kind == DiscoveryKind::Shared && source.server_count > 0)
        .map(|source| source.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    (
        vec![
            format!("Using standard MCP config from {sources}."),
            // CYRUP-DELTA: upstream says "Pi". The sentence names the product to the user, and the
            // crate rewords upstream's product references rather than shipping the host's name.
            "Cyrup only writes compatibility imports and adapter-specific overrides into \
             Cyrup-owned files when needed."
                .to_string(),
        ],
        Some(summary.fingerprint.clone()),
    )
}
