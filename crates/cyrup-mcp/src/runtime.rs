//! `initializeMcp` — the runtime build (`init.ts`; 13a §8–§13, MCP-015…MCP-026).
//!
//! This is the function that turns a configuration into a live runtime: it builds the server
//! manager, wires the sampling and elicitation gates, registers the owner's cleanups in their exact
//! LIFO order, bootstraps the metadata cache, registers each server's lifecycle, rehydrates
//! metadata from a hash-valid cache entry, runs the bounded startup connect pass, and returns the
//! [`McpState`] the extension commits.
//!
//! # MCP-015 — snapshot before the first await, and why
//!
//! Upstream's own comment: *"Pi guards ExtensionContext getters after reload. Snapshot all values
//! that can be used by asynchronous work before the first await."* Seven reads
//! (`configPath`, `cwd`, `hasUI`, `mode`, `rawUi`, `modelRegistry`, `initialSignal`) plus two
//! derivations (`ui = createOwnedUi(rawUi, owner)` and
//! `runtimeSignal = combineAbortSignals(owner.signal, initialSignal)`). [`ContextSnapshot`] is that
//! snapshot, taken by the caller *before* this function is ever polled — which is stricter than
//! taking it here, and is why it is a parameter rather than something this function reads.
//!
//! # The two live closures that are deliberately not snapshotted
//!
//! The sampling gate's `getCurrentModel` and `getSignal` are **live closures over `ctx`**,
//! owner-guarded on each call: `owner.isActive() ? ctx.model : undefined` and
//! `owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal) : owner.signal`. They must stay
//! live — a sampling request arriving twenty minutes into a session has to see the model the user
//! selected five minutes ago, not the one at init.
//!
//! # This file has two halves, and they meet only at the seam
//!
//! Everything above `The wire` is the **runtime build**: one function, called once per generation,
//! that turns configuration into an [`McpState`]. Everything below it is the **connection**: how a
//! [`crate::config::ServerEntry`] becomes an `rmcp` transport, how that transport becomes an
//! initialised client, and what the client does when a server talks back unprompted
//! ([`McpClientHandler`]). The halves share no state — the second is a set of pure builders and one
//! handler type that `server_manager` composes — which is deliberate: the connection half must be
//! testable without an `McpState`, an owner, or a tokio reactor, and most of it is.
//!
//! # Cleanup LIFO order after Cut 2
//!
//! `lifecycle.graceful_shutdown()` → `shutdown_oauth` → (registered later, by `startInitialization`)
//! `cleanup_materialized_binary_resources`. Registered in that order, run in reverse. The
//! `uiServer.close(reason)` cleanup is **Cut 2** and has no replacement.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext::HostServices;

use crate::config::{McpConfig, ServerLifecycle};
use crate::dirs::McpDirs;
use crate::errors::McpResult;
use crate::lifecycle::{McpLifecycleManager, DEFAULT_IDLE_TIMEOUT_MINUTES};
use crate::owner::{McpRuntimeOwner, OwnedServices};
use crate::state::{
    AuthStorageOptions, McpServerManager, McpState, McpStateParts, OAuthRuntime, OpenBrowser,
    SendMessage,
};

/// Everything the extension context can supply, captured **before the first await** (MCP-015).
///
/// `rawUi` is not here: it is consumed immediately into the fenced [`OwnedServices`] handle, which
/// is what actually crosses the await.
#[derive(Clone)]
pub struct ContextSnapshot {
    /// `--mcp-config`'s value, if the user passed one. Read from argv, not the flag store — see
    /// [`crate::config::config_path_from_argv`].
    pub config_path: Option<PathBuf>,
    /// The session working directory. `resolveConfigPath`'s base.
    pub cwd: PathBuf,
    /// `ctx.hasUI`. Gates elicitation entirely and sampling unless auto-approve is on.
    pub has_ui: bool,
    /// `ctx.mode`. `allowUrl` for URL elicitation is `mode === "tui"` specifically, not `hasUI`.
    pub mode: String,
    /// `ctx.signal` — the caller's own cancellation, combined with the owner's into
    /// `runtimeSignal`. `None` where the host supplies none.
    pub initial_signal: Option<CancelToken>,
    /// The live capability backend, stashed by `set_host_services` before `init`. Fenced into
    /// [`OwnedServices`] before it is handed to anything asynchronous.
    pub services: Option<Arc<dyn HostServices>>,
}

impl ContextSnapshot {
    /// `isTuiMode(ctx)` — exported from `init.ts` and exactly `ctx.hasUI && ctx.mode === "tui"`.
    /// Not the same test as `has_ui`: URL elicitation requires a real TUI, not merely a UI.
    #[must_use]
    pub fn is_tui_mode(&self) -> bool {
        self.has_ui && self.mode == "tui"
    }
}

/// `createMcpAdapter(options)`'s options, as far as [`initialize_mcp`] consumes them.
#[derive(Clone, Default)]
pub struct InitializeOptions {
    /// `options.config` — a programmatic configuration that **replaces** discovery.
    /// Cloned at factory time and again per call upstream, so a caller cannot mutate a live
    /// runtime through the object it passed in.
    pub programmatic_config: Option<McpConfig>,
    /// `options.oauthRuntime`. When `None`, this generation **owns** the runtime it creates and
    /// registers `shutdownOAuth` as an owner cleanup; when `Some`, it does not.
    pub oauth_runtime: Option<Arc<OAuthRuntime>>,
}

/// `initializeMcp(pi, ctx, owner, options)` — build the live runtime for one generation.
///
/// **Minimal body (MCP-016…MCP-026 fill it).** What is landed now is the shape every later unit
/// hangs off and must not re-derive: the snapshot parameter, the owner-fenced UI handle, the
/// combined runtime signal, the manager/lifecycle construction order, the cleanup registration
/// order, and the [`McpState`] commit. The staged connect pass, the two-pass metadata build, the
/// cache bootstrap and the sampling/elicitation gates are the work.
///
/// Wiring order, reproduced from 13a §8 so a filler does not have to re-read it:
///
/// 1. `config` — the programmatic clone, else [`crate::config::load_mcp_config`]
/// 2. `authStorageOptions = getAuthStorageOptions(settings.oauthDir, cwd)`
/// 3. `ownsOAuthRuntime = options.oauthRuntime === undefined`
/// 4. `manager = new McpServerManager(cwd)` + its five setters
/// 5. the **sampling gate**: `settings.sampling !== false && (has_ui || samplingAutoApprove)`
/// 6. the **elicitation gate**: `settings.elicitation !== false && has_ui`, `allowUrl = is_tui_mode`
/// 7. `lifecycle = new McpLifecycleManager(manager, hasPendingAuth)`
/// 8. allocate the live maps and sets
/// 9. build the state
/// 10. if `ownsOAuthRuntime`: `owner.addCleanup(shutdownOAuth)`
/// 11. `manager.setMetadataListChangedListener(...)`
/// 12. `owner.addCleanup(lifecycle.gracefulShutdown)`
pub async fn initialize_mcp(
    owner: Arc<McpRuntimeOwner>,
    dirs: McpDirs,
    snapshot: ContextSnapshot,
    options: InitializeOptions,
) -> McpResult<Arc<McpState>> {
    // Step 1 — the config this generation runs. `loadMcpConfig` cannot fail (MCP-003).
    let config = options.programmatic_config.clone().unwrap_or_else(|| {
        crate::config::load_mcp_config(&dirs, snapshot.config_path.as_deref())
    });

    // MCP-015's two derivations. The fenced handle is built BEFORE anything asynchronous can hold
    // it, which is the whole point: what crosses the await is already inert-on-stop.
    // `const rawUi = hasUI ? ctx.ui : undefined` (`init.ts:104-106`). The `has_ui` gate is
    // load-bearing and not cosmetic: `McpState::dialog()` is `None` exactly when this is `None`, and
    // that is the ONLY thing that distinguishes "this session has no interactive UI" from "the user
    // was asked and declined". Deriving `ui` straight from `snapshot.services` — which the host
    // supplies whether or not a human can see it — made `NoInteractiveSession` unreachable in
    // production and collapsed MCP-232's two answers into one (the safe one, but the wrong one).
    let ui = snapshot
        .has_ui
        .then(|| snapshot.services.clone())
        .flatten()
        .map(|services| Arc::new(OwnedServices::new(services, Arc::clone(&owner))));
    let runtime_signal =
        crate::abort::combine(&owner.token(), snapshot.initial_signal.as_ref());

    // Steps 2-4. `getAuthStorageOptions(settings.oauthDir, cwd)` — and **only** `settings.oauthDir`:
    // `$MCP_OAUTH_DIR` and the `<agent_dir>/mcp-oauth` default are the store's own precedence ladder
    // ([`crate::credentials::McpAuthStore::auth_base_dir`]), so pre-resolving a base dir here would
    // pin the lowest rung and make the environment override unreachable (MCP-265).
    let auth_storage_options = AuthStorageOptions::from_settings(
        config
            .settings
            .as_ref()
            .and_then(|settings| settings.oauth_dir.as_deref()),
        &snapshot.cwd,
    );
    let owns_oauth_runtime = options.oauth_runtime.is_none();
    // `createOAuthRuntime(signal)` under the generation's own token, so a session replacement
    // aborts every in-flight login it started and disturbs no other's (MCP-301, MCP-344).
    let oauth_runtime = options
        .oauth_runtime
        .clone()
        .unwrap_or_else(|| crate::oauth::create_oauth_runtime(Some(&owner.token())));
    // `new McpServerManager(cwd)` — with `createConnection` supplied. [`ConnectionBuilder`] is that
    // seam's body (MCP-101/109/113/114/115/115a), and this line is the one place in the crate that
    // installs it; everywhere else the manager still carries `UnbuiltConnectionFactory` and every
    // connect fails loudly by design. `McpServerManager::new`/`default` keep the unbuilt factory
    // deliberately: they are `server_manager.rs`'s constructors and every test in that module
    // scripts its own factory, so flipping their default would be a change to a file this unit does
    // not own for no behaviour anyone depends on.
    //
    // **REACH, stated exactly, because the obvious reading of the line above is wrong.** Installing
    // the builder *here* does not put it on any shipping path: `initialize_mcp` itself has no
    // non-test caller. `grep -rn 'initialize_mcp('` over the repo returns its definition and one
    // call, `runtime.rs:403`, inside this file's `#[cfg(test)]` module. The production entry point
    // that would reach it — `McpExtension::on_session_start` (`extension.rs:279-300`) — is still
    // MCP-008/MCP-011's empty body and calls nothing. So the seam is FILLED and REACHABLE (every
    // arm below is exercised against real children and a real loopback server), but not yet reached
    // from a session. "A configured server connects in production" becomes true when
    // `on_session_start` calls `startInitialization`, not here.
    //
    // Two things the builder does NOT yet get here, both named rather than smuggled:
    // `with_handler_factory` (the manager's sampling/elicitation/list-changed hooks, MCP-118/120/122)
    // and `with_auth_provider` (section 05's OAuth provider, MCP-115). Without the second, an HTTP
    // server whose credential is already in the store still ends at `needs-auth` — the same outcome
    // upstream reaches on a first login, and the wrong one for a returning user.
    let manager = Arc::new(McpServerManager::with_factory(
        Some(snapshot.cwd.clone()),
        Arc::new(ConnectionBuilder::new(Some(snapshot.cwd.clone()))),
    ));
    // Step 4's setters, now that `McpServerManager` is real (MCP-100). Four of the eight are
    // resolvable here; the other four are not this step's:
    //
    // * `setMetadataListChangedListener` is **step 11** — installed after the state commits, so a
    //   hook fired mid-build cannot see a half-installed surface (MCP-011/MCP-030);
    // * `setSamplingConfig` / `setElicitationConfig` are steps 5-6's gates (MCP-118/MCP-121/MCP-122)
    //   and are wired with their handlers, not here;
    // * `setTraceConfig` has no counterpart at all — `mcp-trace.ts` is MCP-133, unported.
    //
    // `runtimeSignal` is combined **once per generation**, which is what makes
    // `crate::abort::combine`'s one-forwarder-task-per-pair cost affordable (13a §8).
    manager.set_runtime_signal(Some(runtime_signal));
    manager.set_default_request_timeout_ms(
        config
            .settings
            .as_ref()
            .and_then(|settings| settings.request_timeout_ms),
    );
    manager.set_auth_storage_options(auth_storage_options.clone());
    manager.set_oauth_runtime(Arc::clone(&oauth_runtime));

    // Step 7. `hasPendingAuth` is the OAuth runtime's, so an authenticating server is never reaped.
    let lifecycle = Arc::new(McpLifecycleManager::new(Arc::clone(&manager), Arc::new(|_| false)));
    lifecycle.set_global_idle_timeout(
        config
            .settings
            .as_ref()
            .and_then(|s| s.idle_timeout)
            .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES),
    );

    // Steps 8-9.
    let open_browser: OpenBrowser = {
        let owner = Arc::clone(&owner);
        Arc::new(move |_url: String| {
            let owner = Arc::clone(&owner);
            Box::pin(async move {
                // Guarded on BOTH sides of the await, as upstream is (13a §8 step 9).
                owner.throw_if_inactive()?;
                let result = Ok(());
                owner.throw_if_inactive()?;
                result
            })
        })
    };
    let send_message: SendMessage = {
        let owner = Arc::clone(&owner);
        Arc::new(move |message: String| {
            // `if (!owner.isActive()) return;` then `pi.sendMessage(...)`. The owner check IS the
            // guard — a stale generation must not inject into the session that replaced it.
            if owner.is_active() {
                tracing::debug!("MCP: send_message not yet wired — dropping {} bytes", message.len());
            }
        })
    };

    let state = Arc::new(McpState::new(McpStateParts {
        owner: Arc::clone(&owner),
        manager,
        lifecycle: Arc::clone(&lifecycle),
        config,
        programmatic_config: options.programmatic_config,
        oauth_runtime,
        auth_storage_options,
        ui,
        open_browser,
        send_message,
    }));

    // Steps 10 and 12 — registered in this order so the LIFO run order is
    // `gracefulShutdown` -> `shutdownOAuth`.
    if owns_oauth_runtime {
        let oauth_runtime = Arc::clone(&state.oauth_runtime);
        owner.add_cleanup(Box::new(move || {
            Box::pin(async move {
                crate::oauth::shutdown_oauth(&oauth_runtime).await;
                Ok(())
            })
        }));
    }
    owner.add_cleanup(Box::new(move || {
        Box::pin(async move {
            lifecycle.graceful_shutdown().await;
            Ok(())
        })
    }));

    // 13a §8's zero-enabled-servers early return (MCP-018): no cache work, no lifecycle, no health
    // checks — just the published snapshot and the state.
    if state.config.enabled_servers().next().is_none() {
        state.publish_status(crate::state::McpStatusSnapshot::default());
        return Ok(state);
    }

    Ok(state)
}

/// `startLoadTimeInitialization`'s gate (MCP-012): connect at load **only** when some enabled
/// server declares `lifecycle: "eager" | "keep-alive"`. Everything else waits for its first call.
///
/// Landed now because [`McpExtension`][crate::extension::McpExtension]'s `init` consults it to decide whether to
/// spawn the pre-warm task at all, and getting this wrong costs every session a subprocess
/// handshake it did not need.
#[must_use]
pub fn needs_load_time_initialization(config: &McpConfig) -> bool {
    config.enabled_servers().any(|(_, entry)| entry.lifecycle_mode().is_prewarmed())
}

/// `MCP_DIRECT_TOOLS`'s "no servers" sentinel (MCP-013). Not an empty string: an empty value is
/// indistinguishable from unset in most shells, so upstream picked a token.
pub const DIRECT_TOOLS_NONE_SENTINEL: &str = "__none__";

/// Normalise `MCP_DIRECT_TOOLS` — `split(',').map(trim).filter(non-empty)`, with the
/// [`DIRECT_TOOLS_NONE_SENTINEL`] collapsing to "explicitly none" (MCP-013).
///
/// `None` means the variable is unset (no pinning); `Some(empty)` means the sentinel was given.
#[must_use]
pub fn direct_tools_override(raw: Option<&str>) -> Option<Vec<String>> {
    let raw = raw?;
    if raw.trim() == DIRECT_TOOLS_NONE_SENTINEL {
        return Some(Vec::new());
    }
    Some(
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Whether `lifecycle` should be marked keep-alive at **registration** time. Only `keep-alive` is;
/// `lazy-keep-alive` is marked after its first successful connect (13a §10 step 5).
#[must_use]
pub fn marks_keep_alive_at_registration(lifecycle: ServerLifecycle) -> bool {
    matches!(lifecycle, ServerLifecycle::KeepAlive)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::ServerEntry;

    fn config_with(lifecycle: Option<ServerLifecycle>, disabled: bool) -> McpConfig {
        let mut config = McpConfig::default();
        config.mcp_servers.insert(
            "s".to_string(),
            ServerEntry {
                lifecycle,
                disabled: disabled.then_some(true),
                ..Default::default()
            },
        );
        config
    }

    #[test]
    fn prewarm_gate_matches_the_two_lifecycles_and_skips_disabled() {
        assert!(!needs_load_time_initialization(&config_with(None, false)), "lazy is the default");
        assert!(needs_load_time_initialization(&config_with(Some(ServerLifecycle::Eager), false)));
        assert!(needs_load_time_initialization(&config_with(Some(ServerLifecycle::KeepAlive), false)));
        assert!(
            !needs_load_time_initialization(&config_with(Some(ServerLifecycle::LazyKeepAlive), false)),
            "lazy-keep-alive connects on first call, not at load"
        );
        assert!(
            !needs_load_time_initialization(&config_with(Some(ServerLifecycle::Eager), true)),
            "a disabled server never pre-warms"
        );
    }

    #[test]
    fn direct_tools_override_normalisation() {
        assert_eq!(direct_tools_override(None), None);
        assert_eq!(direct_tools_override(Some("__none__")), Some(Vec::new()));
        assert_eq!(
            direct_tools_override(Some(" a , ,b ")),
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn only_keep_alive_is_marked_at_registration() {
        assert!(marks_keep_alive_at_registration(ServerLifecycle::KeepAlive));
        assert!(!marks_keep_alive_at_registration(ServerLifecycle::LazyKeepAlive));
        assert!(!marks_keep_alive_at_registration(ServerLifecycle::Eager));
    }

    #[test]
    fn tui_mode_is_stricter_than_has_ui() {
        let snap = ContextSnapshot {
            config_path: None,
            cwd: PathBuf::from("/w"),
            has_ui: true,
            mode: "print".to_string(),
            initial_signal: None,
            services: None,
        };
        assert!(!snap.is_tui_mode());
        let tui = ContextSnapshot { mode: "tui".to_string(), ..snap };
        assert!(tui.is_tui_mode());
    }

    #[tokio::test]
    async fn zero_enabled_servers_returns_a_state_without_connecting() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let dirs = McpDirs::new(PathBuf::from("/nonexistent/agent"), PathBuf::from("/w"));
        let snapshot = ContextSnapshot {
            config_path: None,
            cwd: PathBuf::from("/w"),
            has_ui: false,
            mode: "print".to_string(),
            initial_signal: None,
            services: None,
        };
        let state = initialize_mcp(owner, dirs, snapshot, InitializeOptions::default())
            .await
            .unwrap();
        assert!(state.config.mcp_servers.is_empty());
        assert!(state.owner.is_active());
    }
}

// =================================================================================================
// The wire — transports, the rmcp client and `ClientHandler` (13c §3.2–§3.10)
//
// Everything below this line is the *connection* half of the runtime: how a [`ServerEntry`] becomes
// a live `rmcp` transport, how that transport becomes an initialised client, and what the client
// does when the server talks back unprompted. It is deliberately free of connection *bookkeeping* —
// the registry, the single-flight maps, the generations and the retry ladders are
// `server_manager`'s (MCP-100, MCP-115, MCP-123…126) and consume the seams declared here.
//
// The four surfaces upstream has that do not exist here, so a reader does not go looking:
//
// * the legacy HTTP+SSE transport (**Cut 1** — rmcp 3.1.x ships no SSE *client* transport at all),
// * the raw framed unix socket (**Cut 3** — [`ServerEntry`] has no `socket` field),
// * the adapter-private UI stream-patch notification (**Cut 2** — see
//   [`STREAM_RESULT_PATCH_METHOD`]), and
// * `wrapTransportWithMcpTrace`, whose *composition* trick is what makes upstream's protocol probe
//   run on a disposable sibling process — see [`version_negotiation`] for the delta that costs.
// =================================================================================================

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
// `http` and `sse-stream` are declared dependencies for exactly this reason: implementing
// `StreamableHttpClient` (here for [`SessionIdProbe`], in `request_headers_command.rs` for the
// signing decorator) means naming the trait's argument and return types, and a trait impl leaves no
// option to reach them by inference. See the crate's dependency note.
use http::{HeaderName, HeaderValue};
use sse_stream::{Error as SseError, Sse};
// SEP-2577 deprecates `sampling/createMessage` protocol-wide and rmcp marks the types
// accordingly; `pi-mcp-adapter` ships a sampling handler and 1:1 parity is a hard rule, so the
// deprecation is acknowledged and suppressed rather than obeyed. rmcp's own `handler/client.rs`
// carries the identical `#![expect(deprecated)]` for the identical reason.
#[allow(deprecated)]
use rmcp::model::{
    ClientCapabilities, ClientInfo, CreateMessageRequestMethod, CreateMessageRequestParams,
    CreateMessageResult, CustomNotification, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationCapability, FormElicitationCapability, Implementation,
    ProtocolVersion, SamplingCapability, UrlElicitationCapability,
};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, MaybeSendFuture, NotificationContext, Peer,
    PeerRequestOptions, RequestContext, RoleClient, RunningService,
};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
    StreamableHttpPostResponse,
};
use rmcp::transport::{ConfigureCommandExt, IntoTransport, StreamableHttpClientTransport,
    TokioChildProcess};
use rmcp::{ClientHandler, ErrorData};
use tokio::process::ChildStderr;

use crate::config::{HttpTransport, ProtocolVersionSetting, ServerEntry};
use crate::errors::McpError;
use crate::lifecycle::ConnectionStatus;
use crate::server_manager::{
    ConnectionFactory, ConnectionResource, CreateConnection, NewConnection,
};

// -------------------------------------------------------------------------------------------------
// MCP-113 — transport selection and mutual exclusion (§3.2)
// -------------------------------------------------------------------------------------------------

/// The two transports that survive the cuts.
///
/// Upstream counts three (`command`, `url`, `socket`) and enforces "exactly one"; here the count is
/// two, because Cut 3 removed the raw unix socket outright — see [`socket_cut_diagnostic`] for the
/// string a configuration that still carries one has to be refused with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// `definition.command` — a child process speaking NDJSON over stdio (§3.3).
    Stdio,
    /// `definition.url` — streamable HTTP (§3.4). The only HTTP transport rmcp implements
    /// client-side, and post-Cut-1 the only one this port accepts.
    StreamableHttp,
}

/// `createConnection`'s mutual-exclusion check (`server-manager.ts:466-470`), minus `socket`.
///
/// Three properties are load-bearing and each has cost a porter a bug somewhere:
///
/// 1. **An empty string counts as unconfigured.** `{command: "", url: "http://x"}` is a *valid*
///    HTTP server, not an error — upstream's filter is
///    `typeof value === "string" && value.length > 0`.
/// 2. **`${name}` is unquoted in this one message**, unlike almost every other string in
///    `server-manager.ts`. Reproduced verbatim.
/// 3. **The check fires at connect time, not at parse time** (13b "Validation rules that fire at
///    connect time"). A two-transport entry loads fine and fails per-connection, which is why
///    `isServerCacheValid` has to wrap `computeServerHash` in a `try`/`catch`.
///
/// The returned error is [`McpError::Config`] rather than [`McpError::Server`] because `Config`'s
/// `Display` is `{0}` — the message already contains the server name and `Server`'s
/// `"{server}: {message}"` would print it twice.
///
/// # The two cut diagnostics
///
/// `httpTransport: "sse"` is refused here with the Cut-1 sentence. It *should* already have been
/// refused at config load (13b "Cut-value diagnostics"), and this is the second line of defence:
/// silence is the one unacceptable behaviour, because `agent-plugin-loader.ts` sets `httpTransport`
/// straight from a manifest's `type: "sse"`, so a plugin declaring SSE is a live, reachable case
/// that would otherwise appear configured and never connect.
///
/// # Errors
///
/// The exactly-one-transport error, or the Cut-1 legacy-SSE diagnostic.
pub fn select_transport(name: &str, entry: &ServerEntry) -> McpResult<TransportKind> {
    // `[definition.command, definition.url].filter(v => typeof v === "string" && v.length > 0)`.
    let command = configured(entry.command.as_deref());
    let url = configured(entry.url.as_deref());

    match (command, url) {
        (Some(_), None) => Ok(TransportKind::Stdio),
        (None, Some(_)) => {
            if entry.http_transport == Some(HttpTransport::Sse) {
                return Err(McpError::Config(sse_cut_diagnostic(name)));
            }
            Ok(TransportKind::StreamableHttp)
        }
        // Both or neither. Upstream's message named three transports; post-cut it names two, and
        // the `${name}` stays unquoted.
        _ => Err(McpError::Config(format!(
            "Server {name} must configure exactly one of command or url"
        ))),
    }
}

/// `typeof value === "string" && value.length > 0` — an empty string is *unconfigured*, not a
/// misconfigured transport. Note this is `is_empty`, not `trim().is_empty()`: upstream tests the
/// raw length, so a single space is a configured (and doomed) command.
#[must_use]
fn configured(value: Option<&str>) -> Option<&str> {
    value.filter(|candidate| !candidate.is_empty())
}

/// The **Cut 1** diagnostic, verbatim from 13c §3.2. Exposed so the config loader (13b, MCP-050…065)
/// refuses `httpTransport: "sse"` with the *same sentence* the connect path would, rather than a
/// second paraphrase of it.
#[must_use]
pub fn sse_cut_diagnostic(name: &str) -> String {
    format!(
        "MCP server \"{name}\" requests the legacy HTTP+SSE transport, which cyrup does not \
         support; use streamable HTTP."
    )
}

/// The **Cut 3** diagnostic, verbatim from 13c §3.2.
///
/// [`ServerEntry`] has no `socket` field, so this string can never be produced from a parsed entry —
/// it belongs to the config loader, which sees the raw key before it is dropped. It lives here
/// beside its Cut-1 sibling so the two sentences stay together and neither drifts.
#[must_use]
pub fn socket_cut_diagnostic(name: &str) -> String {
    format!(
        "MCP server \"{name}\" configures `socket`; cyrup supports only stdio (`command`) and \
         streamable HTTP (`url`)."
    )
}

// -------------------------------------------------------------------------------------------------
// MCP-102 — bounded stderr capture (§3.3)
// -------------------------------------------------------------------------------------------------

/// `MAX_CAPTURED_STDERR_BYTES` (`server-manager.ts:64`). 8 KiB, and the bound is applied to every
/// chunk *before* it is appended, so a child that writes a megabyte in one burst never causes a
/// megabyte allocation.
pub const MAX_CAPTURED_STDERR_BYTES: usize = 8 * 1024;

/// `MAX_CAPTURED_STDERR_LINES` (`server-manager.ts:65`). Only the **last three** non-empty lines
/// reach the user.
pub const MAX_CAPTURED_STDERR_LINES: usize = 3;

/// `boundedStderrChunk` (`server-manager.ts:96-112`), byte arm only.
///
/// Upstream has two arms because a Node `"data"` listener can hand back either a `Buffer` or a
/// `string`; a `tokio::process::ChildStderr` is a byte stream and only the `Buffer` arm is
/// reachable, which is also the arm that does not have to reason about chars-vs-bytes. The
/// string arm's whole purpose — *"limit string conversion before encoding"* — is satisfied here by
/// never decoding at all until [`stderr_tail_detail`].
#[must_use]
pub fn bounded_stderr_chunk(chunk: &[u8]) -> &[u8] {
    let start = chunk.len().saturating_sub(MAX_CAPTURED_STDERR_BYTES);
    chunk.get(start..).unwrap_or(chunk)
}

/// `appendStderrTail` (`server-manager.ts:114-122`) as an in-place ring.
///
/// Upstream allocates a fresh `Buffer` per chunk (`Buffer.concat` then a possible `subarray`); a
/// `VecDeque` gives the same last-N-bytes semantics with amortised O(1) appends and no reallocation
/// per stderr event, which matters because a chatty server emits one of these per line.
pub fn append_stderr_tail(tail: &mut VecDeque<u8>, chunk: &[u8]) {
    let bytes = bounded_stderr_chunk(chunk);
    if bytes.is_empty() {
        return;
    }
    tail.extend(bytes.iter().copied());
    let excess = tail.len().saturating_sub(MAX_CAPTURED_STDERR_BYTES);
    if excess > 0 {
        drop(tail.drain(..excess));
    }
}

/// The failure-message suffix built from a captured tail (`server-manager.ts:625-633`).
///
/// `tail.toString("utf8").trim()`, split on `/\r?\n/`, each line trimmed, empties dropped, the last
/// [`MAX_CAPTURED_STDERR_LINES`] joined by `" — "` (space, U+2014 EM DASH, space). `None` when
/// nothing survives, which is the arm that makes `debug: true` produce no `(...)` suffix at all —
/// in debug mode stderr is *inherited* by the host terminal and there is no tail to build from.
///
/// A multi-byte sequence split at the 8 KiB boundary becomes U+FFFD, exactly as Node's
/// `Buffer.toString("utf8")` does.
#[must_use]
pub fn stderr_tail_detail(tail: &VecDeque<u8>) -> Option<String> {
    if tail.is_empty() {
        return None;
    }
    let bytes: Vec<u8> = tail.iter().copied().collect();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text
        .trim()
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(MAX_CAPTURED_STDERR_LINES);
    let detail = lines.get(start..).unwrap_or(&lines).join(" — ");
    Some(detail)
}

/// `` throw new Error(`${baseMessage} (${detail})`) `` — the enrichment applied to a stdio connect
/// failure when the child said something on the way down (`server-manager.ts:629-631`).
///
/// Returns `base` untouched when there is no tail, which is both the `debug: true` case and the
/// "child died silently" case.
#[must_use]
pub fn with_stderr_tail(base: &str, tail: &VecDeque<u8>) -> String {
    match stderr_tail_detail(tail) {
        Some(detail) => format!("{base} ({detail})"),
        None => base.to_string(),
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-101 — the stdio transport (§3.3)
// -------------------------------------------------------------------------------------------------

/// Everything the stdio spawn needs, **already resolved**.
///
/// The resolution itself is deliberately not here — it is [`crate::secrets`], and
/// [`Self::resolve`] is the seam. `resolveEnv`'s full-environment copy and the `!`/`!!`
/// command-secret grammar are [`crate::secrets::resolve_stdio_env`] (MCP-083); `interpolateEnvVars`
/// over `args` and the npx/npm rewrite of `command`/`args` (MCP-103) stay with the caller, which is
/// why [`Self::resolve`] takes them already-built. Keeping the split here is what lets
/// [`spawn_stdio_transport`] stay a pure function of its inputs and be unit-tested without a shell.
pub struct StdioTransportSpec {
    /// Post-npx-resolution executable: `resolved.isJs ? "node" : resolved.binPath`, else
    /// `definition.command` untouched. Never interpolated — upstream interpolates `args`, not
    /// `command`.
    pub command: String,
    /// Post-npx-resolution, post-`interpolateEnvVars` arguments. Never `!command`-resolved.
    pub args: Vec<String>,
    /// `resolveEnv(...)`'s output: the **full** child environment, not a set of overrides.
    /// `StdioClientTransport`'s `env` option *replaces* the child environment, and `resolveEnv`
    /// copies all of `process.env` before layering, so [`spawn_stdio_transport`] clears and sets
    /// rather than merging — a `Command` that merely `.envs()`d would silently keep host variables
    /// a caller had deliberately dropped.
    pub env: HashMap<String, String>,
    /// `resolveConfigPath(definition.cwd) ?? this.defaultCwd`. `None` reproduces the *"key omitted
    /// entirely"* arm — the child inherits the parent's cwd.
    pub cwd: Option<PathBuf>,
    /// `definition.pluginDataDir`, `mkdir -p`'d **before** the spawn. Set only by
    /// [`crate::agent_plugin`]; it is `${PLUGIN_DATA}`, and a plugin whose first write creates it
    /// would race its own server.
    pub plugin_data_dir: Option<PathBuf>,
    /// `definition.debug`. `true` ⇒ stderr is **inherited** by the host terminal and there is no
    /// tail; `false` ⇒ piped, and the returned handle feeds [`append_stderr_tail`].
    pub debug: bool,
}

impl StdioTransportSpec {
    /// §3.3 step 7's `env` and the two fields read straight off the definition, resolved — the
    /// MCP-083 half of the stdio pre-flight.
    ///
    /// `command`, `args` and `cwd` arrive already-built because each belongs to a different unit:
    /// the npx/npm rewrite is MCP-103's, `interpolateEnvVars` over `args` is MCP-082's, and
    /// `resolveConfigPath(definition.cwd) ?? this.defaultCwd` is MCP-084's plus the manager's
    /// default. `env` is the one field that can execute a user's shell command, and it is the one
    /// this constructor owns.
    ///
    /// `base` is upstream's `process.env` — [`crate::secrets::process_env_snapshot`] in production,
    /// an explicit map in a test.
    ///
    /// # Errors
    ///
    /// The first `env` value whose `!command` fails, carrying
    /// `` MCP server "{server}" stdio env "{key}" `` — see [`crate::secrets::resolve_env`].
    pub fn resolve(
        entry: &ServerEntry,
        server_name: &str,
        command: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        base: &HashMap<String, String>,
    ) -> McpResult<Self> {
        Ok(Self {
            command,
            args,
            env: crate::secrets::resolve_stdio_env(entry, server_name, base)?,
            cwd,
            // `if (definition.pluginDataDir) mkdirSync(...)` (`server-manager.ts:488`) — used raw,
            // NOT `resolveConfigPath`'d, because only `agent_plugin` writes it and it writes an
            // absolute path it built itself.
            plugin_data_dir: entry.plugin_data_dir.as_deref().map(PathBuf::from),
            debug: entry.debug == Some(true),
        })
    }
}

/// Spawn the child and hand back rmcp's stdio transport plus, in non-debug mode, the stderr handle.
///
/// The rmcp fit is exact and worth stating precisely, because it inverts the obvious reading of
/// upstream: `TokioChildProcessBuilder`'s **default** is `Stdio::inherit()` — upstream's
/// `debug: true` — so the port calls `.stderr(Stdio::piped())` on the `debug: false` arm rather than
/// the other way round. `spawn()` then returns `Option<ChildStderr>`, `Some` exactly when piped.
///
/// Ordering follows §3.3 step 5 → 7: the plugin data directory is created *before* the process
/// exists. Step 4's `throwIfAborted(signal)` sits one level up in the connection builder, between
/// npx resolution and this call.
///
/// # Errors
///
/// [`McpError::Io`] carrying the offending path: the plugin data directory when `mkdir -p` fails,
/// the command when the spawn does. A failed spawn is the common case — a mistyped `command` — and
/// the path is what makes it actionable.
pub fn spawn_stdio_transport(
    spec: &StdioTransportSpec,
) -> McpResult<(TokioChildProcess, Option<ChildStderr>)> {
    // §3.3 step 5. `mkdirSync(definition.pluginDataDir, { recursive: true })`.
    if let Some(dir) = spec.plugin_data_dir.as_ref() {
        std::fs::create_dir_all(dir).map_err(|source| McpError::Io {
            path: dir.clone(),
            source,
        })?;
    }

    let mut builder = TokioChildProcess::builder(tokio::process::Command::new(&spec.command)
        .configure(|command| {
            command.args(&spec.args);
            // The replace-not-merge semantics of `StdioClientTransport`'s `env` option. See
            // `StdioTransportSpec::env`.
            command.env_clear();
            command.envs(&spec.env);
            // §3.3 step 6: the key is *omitted entirely* when neither the definition nor the
            // manager supplies one, so `None` must not become `current_dir(".")`.
            if let Some(cwd) = spec.cwd.as_ref() {
                command.current_dir(cwd);
            }
        }));

    if !spec.debug {
        builder = builder.stderr(Stdio::piped());
    }

    builder.spawn().map_err(|source| McpError::Io {
        path: PathBuf::from(&spec.command),
        source,
    })
}

// -------------------------------------------------------------------------------------------------
// MCP-109 — the streamable HTTP transport (§3.4)
// -------------------------------------------------------------------------------------------------

/// Everything the HTTP transport needs, **already resolved** (§3.4 steps 1–7).
///
/// Steps 2–6 — `resolveCommandSecretsRecord` over the headers, the `bearerToken`/`bearerTokenEnv`
/// ladder and the `new Headers()` injection guard — are [`crate::secrets::resolve_http_secrets`]
/// (MCP-083), and [`Self::resolve`] is the seam onto it. Step 1, `resolveServerUrl`'s three throws,
/// is still MCP-084's and is why `url` arrives as a parameter. What reaches
/// [`build_http_transport_config`] is the finished pre-flight.
pub struct HttpTransportSpec {
    /// The `mcpServers` key, for the error strings.
    pub server: String,
    /// `resolveServerUrl(definition)` — interpolated and already proven to parse.
    pub url: String,
    /// The resolved header set, as an ordered `Vec` rather than a map so the transport cannot
    /// reorder it again. **Named delta:** upstream's order is `Object.entries(definition.headers)`,
    /// i.e. the order the keys were written in `mcp.json`, while [`ServerEntry::headers`] is a
    /// `BTreeMap` and hands them over alphabetically. Header *semantics* do not depend on order,
    /// so the only observable consequence is the order they appear on the wire.
    /// `Authorization` is *not* in here when a bearer token resolved — see [`Self::bearer_token`].
    pub headers: Vec<(String, String)>,
    /// The bearer token **without** the `Bearer ` prefix.
    ///
    /// Upstream writes `headers["Authorization"] = \`Bearer ${token}\``; rmcp owns that header
    /// through `auth_header` and applies it with `RequestBuilder::bearer_auth`, which produces the
    /// byte-identical header. Routing it through the typed field rather than the custom-header map
    /// is what keeps the SSE `GET` stream and the session `DELETE` authorized too — both are issued
    /// by the transport worker, not by the caller, and only `auth_header` reaches them.
    pub bearer_token: Option<String>,
}

impl HttpTransportSpec {
    /// §3.4 steps 2–6 applied to a definition — the MCP-083 half of the HTTP pre-flight.
    ///
    /// `url` arrives already-resolved: step 1 is `resolveServerUrl`, whose three throws are MCP-084's
    /// and which must run **before** this, since a URL with a missing environment variable has to
    /// fail before a secret command is ever spawned for it.
    ///
    /// # Errors
    ///
    /// A header or bearer `!command` that failed — carrying
    /// `` MCP server "{server}" HTTP header "{key}" `` or `` MCP server "{server}" HTTP bearer
    /// token `` — or the injection guard's invalid-header-value sentence. See
    /// [`crate::secrets::resolve_http_secrets`].
    pub fn resolve(
        entry: &ServerEntry,
        server_name: &str,
        url: String,
        env: &crate::credentials::EnvFn,
    ) -> McpResult<Self> {
        let resolved = crate::secrets::resolve_http_secrets(entry, server_name, env)?;
        Ok(Self {
            server: server_name.to_string(),
            url,
            headers: resolved.headers,
            bearer_token: resolved.bearer_token,
        })
    }
}

/// Build the streamable-HTTP transport's configuration
/// (`connectHttpClient`'s `new StreamableHTTPClientTransport(url, transportOptions)`).
///
/// # Two rmcp defaults, one of which is wrong for this port
///
/// * **`reinit_on_expired_session` defaults to `true`.** It performs one silent `initialize` replay
///   when the server 404s an expired session. The port turns it **off** and keeps it off: upstream
///   surfaces `isTerminatedSession` to `withSessionRecovery` (MCP-134/135), which re-runs discovery
///   and re-registers metadata. A transparent transport-level reinit would produce a live session
///   whose tool list the adapter never refreshed — the tools would keep resolving against a session
///   that no longer knows them.
/// * **`allow_stateless` defaults to `true`**, which *is* the upstream-equivalent value — named
///   only so the next reader does not "fix" it.
///
/// # Why this returns a config rather than a transport
///
/// `transportOptions.authProvider` has no field here: rmcp models OAuth by **wrapping the HTTP
/// client**, so the authorized transport is
/// `StreamableHttpClientTransport::with_client(AuthClient::new(client, manager), config)` — a
/// *different concrete type* from the unauthorized `StreamableHttpClientTransport<reqwest::Client>`.
/// Returning the config keeps both arms of MCP-115's ladder expressible from one builder, and keeps
/// this function free of a `reqwest` type name (`reqwest` is in the lock file transitively but is
/// not a declared dependency of this crate — see the crate's dependency note). Hand the result to
/// [`http_transport_with_client`], or to `StreamableHttpClientTransport::from_config` once `reqwest`
/// is declared: `from_config` applies rmcp's tuned client (idle pooling off to dodge the ~40 ms
/// Linux delayed-ACK stall, redirects off so custom headers cannot be replayed to a redirect
/// target), and hand-rolling `reqwest::Client::new()` silently loses both.
///
/// # Errors
///
/// [`McpError::Server`] when a header name or value is not representable on the wire.
/// [`crate::secrets::resolve_http_secrets`] has already validated the command-sourced ones with the
/// exact upstream sentence (MCP-083, §3.4 step 6); this arm therefore only fires for a *statically
/// configured* header that upstream would have let through to `fetch` and failed on later — a
/// **recorded divergence**: cyrup rejects it at transport construction, with a message that does not
/// falsely blame a command.
pub fn build_http_transport_config(
    spec: &HttpTransportSpec,
) -> McpResult<StreamableHttpClientTransportConfig> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(spec.url.clone());
    config.reinit_on_expired_session = false;

    for (name, value) in &spec.headers {
        // `custom_headers` is `HashMap<http::HeaderName, http::HeaderValue>`. `http` is not a
        // declared dependency of this crate, so both types are reached through inference from the
        // map's own type rather than named. `HeaderName`/`HeaderValue`'s `TryFrom<&str>` impls are
        // what reject CR/LF and control bytes — the same rejection `new Headers()` performs
        // upstream, which is the header-injection guard of §3.4 step 6.
        let header_name = match TryFrom::try_from(name.as_str()) {
            Ok(header_name) => header_name,
            Err(_) => return Err(invalid_header(&spec.server, name)),
        };
        let header_value = match TryFrom::try_from(value.as_str()) {
            Ok(header_value) => header_value,
            Err(_) => return Err(invalid_header(&spec.server, name)),
        };
        let _ = config.custom_headers.insert(header_name, header_value);
    }

    if let Some(token) = spec.bearer_token.as_ref() {
        config.auth_header = Some(token.clone());
    }

    Ok(config)
}

/// `new StreamableHTTPClientTransport(url, options)` for a caller-supplied HTTP client.
///
/// Generic over the client so the unauthorized arm (`reqwest::Client`) and MCP-115's authorized arm
/// (`AuthClient<reqwest::Client>`) share one construction path, and so this module needs no HTTP
/// client dependency of its own. [`connect_client`] accepts either without change.
#[must_use]
pub fn http_transport_with_client<C: StreamableHttpClient>(
    client: C,
    config: StreamableHttpClientTransportConfig,
) -> StreamableHttpClientTransport<C> {
    StreamableHttpClientTransport::with_client(client, config)
}

/// `(connection.transport as {sessionId?: string})?.sessionId != null` — `session-recovery.ts:59-65`,
/// as a [`StreamableHttpClient`] decorator.
///
/// # Why a decorator and not a field read
///
/// Upstream reads the transport: `get sessionId() { return this._sessionId; }` in the pinned SDK.
/// rmcp keeps the session id in a **local of the transport worker's `run` loop**
/// (`rmcp-3.1.4/src/transport/streamable_http_client.rs:893`) and `StreamableHttpClientTransport`
/// exposes no accessor at all, so there is no field to read. What *is* observable is the response
/// the session id arrives on: `post_message` returns
/// [`StreamableHttpPostResponse::Json`]/[`StreamableHttpPostResponse::Sse`] carrying the
/// `Mcp-Session-Id` the server sent, and the client is a seam this crate already occupies
/// ([`crate::request_headers_command::RequestHeadersCommandClient`] sits in exactly this position).
/// So the flag is set from the wire, once, by whichever response first carries a session id — which
/// for a stateful server is the `initialize` response, the same message upstream's `_sessionId` is
/// assigned from.
///
/// # What this buys, stated as the bug it fixes
///
/// The field used to be a hardcoded `true` under a comment quoting upstream's live read. A
/// streamable-HTTP server that returns **no** `Mcp-Session-Id` is legal — rmcp's `allow_stateless`
/// default is `true` and this port leaves it there — and upstream reads `undefined` for it, so
/// `shouldReconnectAfterRefresh`'s first line (`if (!hadSessionId) return false`) short-circuits.
/// With the constant, `lifecycle.rs:1090`'s `had_session_id` was always `true`, so a plain 404 from
/// a stateless server was classified as a terminated session on every health tick and drove a
/// reconnect-plus-rediscovery cycle upstream never performs.
#[derive(Clone)]
pub struct SessionIdProbe<C> {
    inner: C,
    seen: Arc<std::sync::atomic::AtomicBool>,
}

impl<C> SessionIdProbe<C> {
    /// Wrap `inner`, sharing one flag with every clone the transport makes of it.
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            seen: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// The flag this probe writes. Cloned out **before** the client is handed to the transport,
    /// because the transport takes the client by value.
    #[must_use]
    pub fn flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.seen)
    }

    fn record(&self, response: &StreamableHttpPostResponse) {
        let carried = match response {
            StreamableHttpPostResponse::Json(_, session_id)
            | StreamableHttpPostResponse::Sse(_, session_id) => session_id.is_some(),
            // `Accepted` is a 202/204 with no headers worth reading, and any future variant is
            // likewise not a session-id carrier until this match is revisited (the enum is
            // `#[non_exhaustive]`).
            _ => false,
        };
        if carried {
            // `Relaxed` is enough: the flag is read once, after the handshake future has been
            // awaited to completion, and that await is the happens-before edge.
            self.seen
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl<C> StreamableHttpClient for SessionIdProbe<C>
where
    C: StreamableHttpClient + Sync,
{
    type Error = C::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let response = self
            .inner
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await?;
        self.record(&response);
        Ok(response)
    }

    /// Overridden rather than left to the trait default: the default would delegate to
    /// [`Self::post_message`] and drop the transport-wide SSE event-size limit the inner client
    /// enforces. Same reason for the `get_stream` pair below.
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let response = self
            .inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await?;
        self.record(&response);
        Ok(response)
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.inner
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
    }
}

fn invalid_header(server: &str, name: &str) -> McpError {
    McpError::Server {
        server: server.to_string(),
        message: format!("HTTP header \"{name}\" is not a valid header name/value pair"),
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-117 — protocol-revision negotiation (§3.6)
// -------------------------------------------------------------------------------------------------

/// `resolveVersionNegotiation(definition)` (`server-manager.ts:83-95`) as an rmcp lifecycle mode.
///
/// | `protocolVersion` | upstream | here |
/// |---|---|---|
/// | absent / `"legacy"` | omit `versionNegotiation` entirely | [`ClientLifecycleMode::Initialize`] |
/// | `"auto"` | `{ mode: "auto" }` | [`ClientLifecycleMode::Auto`] |
/// | `"2026-07-28"` | `{ mode: { pin: "2026-07-28" } }` | [`ClientLifecycleMode::Discover`] |
///
/// | anything else | `` throw `Invalid MCP protocolVersion: ${String(…)}` `` | [`McpError::Config`] with that sentence |
///
/// # The fourth arm is reachable, and it is reachable *here*
///
/// It was not, and that was a divergence: [`ProtocolVersionSetting`] was a closed three-variant
/// enum behind `deserialize_with = "lenient"`, so `"2025-06-18"` was discarded at parse time. The
/// value never reached the digest (see [`ProtocolVersionSetting`]) and never reached this `match`
/// either, so a server pinning a real revision this build does not implement negotiated as
/// `legacy` in silence instead of failing to connect.
///
/// [`ProtocolVersionSetting::Other`] restores upstream's shape exactly: the deserialiser validates
/// nothing, `computeServerHash` hashes the value verbatim, and the throw happens **at connect**,
/// which is the one place `resolveVersionNegotiation` (`server-manager.ts:82-95`) performs it. That
/// is why this function is fallible; [`invalid_protocol_version_message`] is the sentence it
/// raises, and `String(definition.protocolVersion)` is [`ProtocolVersionSetting::as_js_string`].
///
/// # Errors
///
/// The entry pinned a `protocolVersion` that is not `"legacy"`, `"auto"` or `"2026-07-28"`.
///
/// # The named delta, and it is the sharpest one in this section
///
/// Upstream runs `server/discover` on a **disposable sibling process**: `mcp-trace.ts`'s
/// `wrapTransportWithMcpTrace` composes callbacks in place rather than returning a wrapper object
/// *precisely* so the SDK can still detect the base stdio transport and clone it for the probe.
/// rmcp does not do this — `serve_client_with_lifecycle` runs `discover_startup` (and, on `Auto`,
/// the legacy fallback) on the **same** `&mut transport` — and rmcp returns `DiscoverOutcome::Legacy`
/// only when the probe produced a complete, correlated JSON-RPC error, "i.e. the transport is in a
/// known-good state". A legacy stdio server that *exits* on `server/discover` therefore fails under
/// `Auto` where pi burned a sibling and connected anyway. Upstream ships a fixture for exactly this
/// case (`__tests__/fixtures/legacy-exits-on-discover-server.mjs`). `Auto` is also bounded by rmcp's
/// `DEFAULT_AUTO_DISCOVER_TIMEOUT` of 10 s, after which it falls back to legacy on the same
/// transport. `"legacy"` — the default — is unaffected: it never sends `server/discover` at all.
pub fn version_negotiation(entry: &ServerEntry) -> McpResult<ClientLifecycleMode> {
    Ok(match entry.protocol_version {
        // Byte-identical arms: `undefined` and `"legacy"` both send a plain `initialize`.
        None | Some(ProtocolVersionSetting::Legacy) => ClientLifecycleMode::Initialize,
        Some(ProtocolVersionSetting::Auto) => ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            // The version the fallback `initialize` announces. `LATEST` is what
            // `ClientLifecycleMode::Initialize` would have sent, so the fallback is byte-identical
            // to the `"legacy"` arm — which is what `{ mode: "auto" }` promises.
            legacy_version: Some(ProtocolVersion::LATEST),
        },
        Some(ProtocolVersionSetting::V20260728) => ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
        // `default:` — upstream's throw, at the moment upstream throws it.
        Some(ProtocolVersionSetting::Other(_)) => {
            return Err(McpError::Config(invalid_protocol_version_message(
                &entry
                    .protocol_version
                    .as_ref()
                    .map(ProtocolVersionSetting::as_js_string)
                    .unwrap_or_default(),
            )));
        }
    })
}

/// `` `Invalid MCP protocolVersion: ${String(definition.protocolVersion)}` `` — upstream's fourth
/// arm, verbatim.
///
/// Raised by [`version_negotiation`], which is where `resolveVersionNegotiation` raises it: at
/// **connect**, never at parse. `config.ts` does not validate `protocolVersion` at all, and
/// `computeServerHash` has already hashed the value verbatim by the time anything looks at it.
#[must_use]
pub fn invalid_protocol_version_message(raw: &str) -> String {
    format!("Invalid MCP protocolVersion: {raw}")
}

// -------------------------------------------------------------------------------------------------
// MCP-118 — client identity and capability advertisement (§3.5)
// -------------------------------------------------------------------------------------------------

/// The client-name prefix the **server sees**. Upstream sends `pi-mcp-<server>`; this is a
/// **recorded rename** — any MCP server that allow-lists the pi client name will not recognise
/// cyrup, and that is a deliberate, visible consequence rather than an oversight.
pub const CLIENT_NAME_PREFIX: &str = "cyrup-mcp-";

/// `{ name, version: "1.0.0" }` — the version half of the client identity, unchanged from upstream.
/// It is protocol identity, not the crate version: bumping cyrup must not change what a server sees.
pub const CLIENT_VERSION: &str = "1.0.0";

/// `buildClientCapabilities()` (`server-manager.ts:678-690`).
///
/// * `sampling: {}` **iff** a sampling handler is wired.
/// * `elicitation: { form: {} }` iff an elicitation handler is wired, plus `url: {}` iff
///   `allowUrl` — which `init.ts` sets from `mode === "tui"`, *not* from `hasUI`.
///
/// Servers gate features on this, so the on/off combinations are behaviour, not decoration.
///
/// # Recorded divergence: the empty case
///
/// Upstream omits the `capabilities` **key entirely** when both are absent. rmcp's
/// `InitializeRequestParams::capabilities` is not an `Option`, so the port sends `"capabilities":{}`.
/// Every field of [`ClientCapabilities`] is `skip_serializing_if`, so the object is empty and
/// semantically identical for any conforming server (the MCP schema requires the key), but the
/// frames are not byte-identical and a fixture asserting "key absent" cannot pass. There is no way
/// to express the omission through rmcp's typed handshake.
#[must_use]
#[allow(deprecated)]
pub fn build_client_capabilities(sampling: bool, elicitation: Option<ElicitationMode>) -> ClientCapabilities {
    // `ClientCapabilities` is `#[non_exhaustive]` and its `builder()` is generated behind the
    // `server`/`macros` features this crate does not enable — `Default` + field assignment is the
    // only construction route. Same for the two capability structs.
    let mut capabilities = ClientCapabilities::default();
    if sampling {
        capabilities.sampling = Some(SamplingCapability::default());
    }
    if let Some(mode) = elicitation {
        // `form: {}` is unconditional; `url: {}` rides on `allowUrl`.
        let mut elicit = ElicitationCapability::new().with_form(FormElicitationCapability::new());
        if mode.allow_url {
            elicit = elicit.with_url(UrlElicitationCapability::new());
        }
        capabilities.elicitation = Some(elicit);
    }
    capabilities
}

/// Whether an elicitation handler is wired, and whether it may open URLs.
///
/// `allow_url` is [`ContextSnapshot::is_tui_mode`] — `hasUI && mode === "tui"` — and is *stricter*
/// than `has_ui`: a print-mode session with a UI still refuses URL elicitation.
#[derive(Debug, Clone, Copy)]
pub struct ElicitationMode {
    /// `elicitationConfig.allowUrl`.
    pub allow_url: bool,
}

/// `new Client({ name: \`pi-mcp-${serverName}\`, version: "1.0.0" }, { capabilities })`
/// (`server-manager.ts:692-720`) — the `initialize` frame's client half.
#[must_use]
pub fn client_info(server: &str, capabilities: ClientCapabilities) -> ClientInfo {
    ClientInfo::new(
        capabilities,
        Implementation::new(format!("{CLIENT_NAME_PREFIX}{server}"), CLIENT_VERSION),
    )
}

// -------------------------------------------------------------------------------------------------
// MCP-120 / MCP-121 / MCP-122 — `ClientHandler`: what the server says unprompted (§3.10)
// -------------------------------------------------------------------------------------------------

/// The adapter-private notification method of **Cut 2** (`ui-stream-types.ts:6`).
///
/// MCP Apps / the UI extension are cut wholesale, so nothing handles this. It is named here for one
/// reason: a server that still emits it must land in [`ClientHandler::on_custom_notification`] and be
/// *dropped* — not logged above `debug`, and above all not treated as a protocol error, because an
/// unhandled notification that closed the connection would take a working server down mid-session.
pub const STREAM_RESULT_PATCH_METHOD: &str = "notifications/pi-mcp-adapter/result-patch";

/// `notifications/elicitation/complete` — the server telling us a browser interaction finished
/// (`server-manager.ts:731-740`).
///
/// rmcp models no first-class variant for it, so it arrives at
/// [`ClientHandler::on_custom_notification`] alongside the Cut-2 method above.
pub const ELICITATION_COMPLETE_METHOD: &str = "notifications/elicitation/complete";

/// Which list a `list_changed` notification is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// `notifications/tools/list_changed`.
    Tools,
    /// `notifications/prompts/list_changed`. The only kind that additionally clears
    /// `promptDiscoveryFailed`.
    Prompts,
    /// `notifications/resources/list_changed`.
    Resources,
}

impl ListKind {
    /// The reason string handed to `metadataListChangedListener`. **Byte-exact** — the metadata
    /// layer switches on it.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Tools => "tools-list-changed",
            Self::Prompts => "prompts-list-changed",
            Self::Resources => "resources-list-changed",
        }
    }

    /// The `<kind>` in `` `MCP: <kind>/list_changed refresh failed for ${serverName}: ${message}` ``.
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Prompts => "prompts",
            Self::Resources => "resources",
        }
    }

    /// The debug line emitted when the refresh itself fails (`server-manager.ts:752`, and its two
    /// siblings). Upstream logs and returns — a failed refresh never disturbs the live list.
    #[must_use]
    pub fn refresh_failed_message(self, server: &str, message: &str) -> String {
        format!(
            "MCP: {}/list_changed refresh failed for {server}: {message}",
            self.wire_name()
        )
    }
}

/// Opaque per-client identity — upstream's `connection.client !== client` check.
///
/// Every `list_changed` handler upstream closes over the `Client` it was built for and refuses to
/// touch the connection map unless the map still holds *that* client. In TypeScript that is object
/// identity; here it is `Arc::ptr_eq` on a token minted once per handler and cloned into every
/// event. Without it, a notification from a client that lost a reconnect race would overwrite the
/// fresh connection's tool list with the stale server's.
#[derive(Clone)]
pub struct ClientIdentity(Arc<IdentityToken>);

/// The allocation whose address *is* the identity. Private, zero-sized, never read.
struct IdentityToken;

impl ClientIdentity {
    fn new() -> Self {
        Self(Arc::new(IdentityToken))
    }

    /// `connection.client === client`.
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClientIdentity({:p})", Arc::as_ptr(&self.0))
    }
}

/// A `list_changed` notification, with everything the manager needs to decide whether to act.
pub struct ListChangedEvent {
    /// The `mcpServers` key.
    pub server: String,
    /// Which list changed.
    pub kind: ListKind,
    /// The identity to `ptr_eq` against `connections[server]` before touching anything.
    pub identity: ClientIdentity,
    /// The live peer, so the hook can re-run `list_all_*` without a second lookup. rmcp's
    /// notification carries **no list** — unlike the TS SDK, which refreshes for you and hands back
    /// `(error, list)` — but `Peer` has already invalidated its own response cache by the time the
    /// handler runs, so the re-list is guaranteed fresh.
    pub peer: Peer<RoleClient>,
}

/// A `notifications/elicitation/complete`, already gated on the runtime signal and on `allowUrl`.
pub struct ElicitationCompleteEvent {
    /// The `mcpServers` key.
    pub server: String,
    /// `notification.params.elicitationId`.
    pub elicitation_id: String,
}

/// The identity-guarded refresh. Owned by the manager (MCP-120), which holds the connection map.
pub type ListChangedHook = Arc<dyn Fn(ListChangedEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// The URL-elicitation completion sink (MCP-122). The manager owns the accepted-id registry, so it
/// is the only place that can honour *"the notice fires only if `Set.delete` returned true"* — a
/// duplicate completion must be silent.
pub type ElicitationCompleteHook = Arc<dyn Fn(ElicitationCompleteEvent) + Send + Sync>;

/// `registerSamplingHandler(client, ...)` — `sampling/createMessage`. Section 05's body; this is the
/// seam it plugs into. See the import block for why the SEP-2577 deprecation is suppressed.
#[allow(deprecated)]
pub type SamplingHook = Arc<
    dyn Fn(String, CreateMessageRequestParams) -> BoxFuture<'static, Result<CreateMessageResult, ErrorData>>
        + Send
        + Sync,
>;

/// `registerElicitationHandler(client, ...)` — `elicitation/create`. Section 05's body.
pub type ElicitationHook = Arc<
    dyn Fn(String, ElicitRequestParams) -> BoxFuture<'static, Result<ElicitResult, ErrorData>>
        + Send
        + Sync,
>;

/// Construction parameters for [`McpClientHandler`]. A struct rather than a nine-argument `new`
/// because five of the fields are optional and three of those are closures.
pub struct McpClientHandlerParts {
    /// The `mcpServers` key. Every hook and every log line carries it.
    pub server: String,
    /// `runtimeSignal` — `combineAbortSignals(owner.signal, ctx.signal)`. The URL-elicitation
    /// completion handler is a **no-op once this is cancelled**, which is what stops a stale
    /// generation from notifying into the session that replaced it.
    pub runtime_signal: CancelToken,
    /// `elicitationConfig`, or `None` when no elicitation handler is wired.
    pub elicitation_mode: Option<ElicitationMode>,
    /// `samplingConfig ? registerSamplingHandler(...) : undefined`.
    pub sampling: Option<SamplingHook>,
    /// `elicitationConfig ? registerElicitationHandler(...) : undefined`.
    pub elicitation: Option<ElicitationHook>,
    /// The manager's identity-guarded refresh (MCP-120).
    pub list_changed: Option<ListChangedHook>,
    /// The manager's completion sink (MCP-122). Upstream registers the underlying notification
    /// handler **only when `allowUrl`**; the same gate is applied here at dispatch, so wiring the
    /// hook without `allow_url` is inert rather than wrong.
    pub elicitation_complete: Option<ElicitationCompleteHook>,
}

/// `createClient(serverName, definition)`'s client object, as an rmcp [`ClientHandler`].
///
/// # Why this is an `Arc` newtype
///
/// Upstream writes `let client: Client;` and then assigns it *after* the callbacks that close over
/// it — a self-reference that TypeScript permits because the closures do not run until the client
/// exists. Rust has no such window, so the shared state lives behind an `Arc` and the handler is a
/// cheap clone of the pointer: `serve_client_with_lifecycle_and_ct` takes the service **by value**,
/// the caller keeps a clone, and both observe the same [`ClientIdentity`]. That identity is the
/// whole point — it is what `Arc::ptr_eq` compares in the `list_changed` guard.
#[derive(Clone)]
pub struct McpClientHandler {
    shared: Arc<HandlerShared>,
}

struct HandlerShared {
    server: String,
    runtime_signal: CancelToken,
    allow_url: bool,
    info: ClientInfo,
    identity: ClientIdentity,
    sampling: Option<SamplingHook>,
    elicitation: Option<ElicitationHook>,
    list_changed: Option<ListChangedHook>,
    elicitation_complete: Option<ElicitationCompleteHook>,
}

impl McpClientHandler {
    /// Build the handler and, with it, the `initialize` frame's client half.
    ///
    /// The capability set is derived from the *wired hooks*, not from configuration: upstream's
    /// `buildClientCapabilities` tests `this.samplingConfig` / `this.elicitationConfig`, so a
    /// runtime with the config present but the handler absent would advertise a capability it
    /// cannot serve. Deriving from the hook makes that state unrepresentable.
    #[must_use]
    pub fn new(parts: McpClientHandlerParts) -> Self {
        let allow_url = parts
            .elicitation_mode
            .is_some_and(|mode| mode.allow_url);
        let capabilities = build_client_capabilities(
            parts.sampling.is_some(),
            parts.elicitation.is_some().then_some(ElicitationMode { allow_url }),
        );
        let info = client_info(&parts.server, capabilities);
        Self {
            shared: Arc::new(HandlerShared {
                server: parts.server,
                runtime_signal: parts.runtime_signal,
                allow_url,
                info,
                identity: ClientIdentity::new(),
                sampling: parts.sampling,
                elicitation: parts.elicitation,
                list_changed: parts.list_changed,
                elicitation_complete: parts.elicitation_complete,
            }),
        }
    }

    /// The `mcpServers` key this client speaks for.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.shared.server
    }

    /// The identity every clone of this handler shares, and that the manager stores alongside the
    /// connection so a stale notification can be recognised.
    #[must_use]
    pub fn identity(&self) -> ClientIdentity {
        self.shared.identity.clone()
    }

    /// The `initialize` frame's client half, as it will be sent.
    #[must_use]
    pub fn info(&self) -> &ClientInfo {
        &self.shared.info
    }

    /// Shared body of the three `on_*_list_changed` arms (§3.10 — all three are identical in shape).
    ///
    /// The two upstream early returns that have no analogue here are worth naming: `if (error)` and
    /// `if (!list)` both exist because the TS SDK does the refresh *for* you and reports its result.
    /// rmcp's notification is bare, so the refresh — and therefore the failure and the null — moves
    /// into the hook, which is where [`ListKind::refresh_failed_message`] is emitted.
    fn dispatch_list_changed(
        &self,
        kind: ListKind,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + 'static {
        let hook = self.shared.list_changed.clone();
        let event = ListChangedEvent {
            server: self.shared.server.clone(),
            kind,
            identity: self.identity(),
            peer: context.peer,
        };
        async move {
            let Some(hook) = hook else {
                tracing::debug!(
                    server = %event.server,
                    kind = kind.wire_name(),
                    "MCP: list_changed with no refresh hook wired; ignoring"
                );
                return;
            };
            hook(event).await;
        }
    }
}

#[allow(deprecated)]
impl ClientHandler for McpClientHandler {
    /// The `initialize` frame. See [`build_client_capabilities`] for the four on/off combinations
    /// and the one recorded divergence (`"capabilities":{}` where upstream omits the key).
    fn get_info(&self) -> ClientInfo {
        self.shared.info.clone()
    }

    /// `sampling/createMessage`. With no handler wired the capability was never advertised, so a
    /// conforming server never asks; an unconforming one gets rmcp's own `METHOD_NOT_FOUND`, which
    /// is exactly what the TS SDK returns for an unregistered request handler.
    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<CreateMessageResult, ErrorData>> + MaybeSendFuture + '_
    {
        let hook = self.shared.sampling.clone();
        let server = self.shared.server.clone();
        async move {
            let Some(hook) = hook else {
                return Err(ErrorData::method_not_found::<CreateMessageRequestMethod>());
            };
            hook(server, params).await
        }
    }

    /// `elicitation/create`. The `Decline` fallback is rmcp's own default and is the right one: a
    /// client that cannot ask the user must not silently accept on their behalf.
    fn create_elicitation(
        &self,
        params: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<ElicitResult, ErrorData>> + MaybeSendFuture + '_ {
        let hook = self.shared.elicitation.clone();
        let server = self.shared.server.clone();
        async move {
            let Some(hook) = hook else {
                return Ok(ElicitResult::new(ElicitationAction::Decline));
            };
            hook(server, params).await
        }
    }

    fn on_tool_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.dispatch_list_changed(ListKind::Tools, context)
    }

    fn on_prompt_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.dispatch_list_changed(ListKind::Prompts, context)
    }

    fn on_resource_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.dispatch_list_changed(ListKind::Resources, context)
    }

    /// Everything rmcp has no first-class variant for — which, for this port, is two methods and a
    /// firm policy for everything else.
    ///
    /// * [`ELICITATION_COMPLETE_METHOD`] (MCP-122) — gated on `allowUrl` **and** on the runtime
    ///   signal, exactly as upstream's `if (this.runtimeSignal?.aborted) return`. The set-delete and
    ///   the *"only if delete returned true"* notice belong to the hook, because the registry is the
    ///   manager's.
    /// * [`STREAM_RESULT_PATCH_METHOD`] (MCP-121, Cut 2) — dropped at `debug`, deliberately.
    /// * anything else — dropped at `debug`. An unhandled notification must **never** fault the
    ///   connection; that is the property the Cut-2 verification asserts.
    fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        let server = self.shared.server.clone();
        let allow_url = self.shared.allow_url;
        let aborted = self.shared.runtime_signal.is_cancelled();
        let hook = self.shared.elicitation_complete.clone();
        async move {
            match notification.method.as_str() {
                ELICITATION_COMPLETE_METHOD => {
                    // `if (this.runtimeSignal?.aborted) return;` plus the `allowUrl` registration
                    // gate, collapsed into one dispatch-time test.
                    if aborted || !allow_url {
                        return;
                    }
                    let Some(elicitation_id) = notification
                        .params
                        .as_ref()
                        .and_then(|params| params.get("elicitationId"))
                        .and_then(serde_json::Value::as_str)
                    else {
                        tracing::debug!(
                            %server,
                            "MCP: elicitation/complete without a string elicitationId; ignoring"
                        );
                        return;
                    };
                    let Some(hook) = hook else { return };
                    hook(ElicitationCompleteEvent {
                        server,
                        elicitation_id: elicitation_id.to_string(),
                    });
                }
                STREAM_RESULT_PATCH_METHOD => {
                    // Cut 2. The UI extension is gone; the notification is not an error.
                    tracing::debug!(
                        %server,
                        "MCP: UI stream-patch notification ignored (MCP Apps are Cut 2)"
                    );
                }
                other => {
                    tracing::debug!(%server, method = %other, "MCP: unhandled notification ignored");
                }
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-148 — the rmcp client construction itself
// -------------------------------------------------------------------------------------------------

/// `await client.connect(transport, requestOptions)` — initialise the connection or fail.
///
/// One call collapses three upstream mechanisms (§3.8):
///
/// * **`connectClientWithAbort`.** `serve_client_with_lifecycle_and_ct` owns the whole
///   initialise-or-fail path and races the token itself, returning
///   [`ClientInitializeError::Cancelled`].
/// * **The `abortCleanupPromises` `WeakMap`.** On `Err` the transport is dropped here, and
///   `TokioChildProcess`'s `ChildWithCleanup::drop` spawns a `kill()`, so no child survives an
///   aborted connect and there is no second `close()` to coordinate. What does *not* collapse is the
///   HTTP retry ladder's once-only teardown across attempts — that is adapter policy and stays in
///   `server_manager` (MCP-123).
/// * **`resolveVersionNegotiation`'s option.** Folded into the `lifecycle` argument; see
///   [`version_negotiation`].
///
/// `requestOptions` is split in two here. Its **signal** half is the `ct` argument. Its **timeout**
/// half is not expressible on this call — rmcp's connect takes no per-request timeout — so it is
/// applied one layer out, by [`connect_client_bounded`], which every connect arm calls instead of
/// this function. Do not call this one directly from a connect arm: an unbounded handshake is
/// exactly the defect `connect_client_bounded` exists to prevent.
///
/// # Errors
///
/// [`ClientInitializeError`], returned unwrapped rather than mapped into [`McpError`]. The auth
/// ladder (MCP-115) needs `ClientInitializeError::auth_challenge()` — which walks the `source()`
/// chain for `AuthRequiredError`/`InsufficientScopeError` and hands back the `WWW-Authenticate`
/// header — and flattening the error here would destroy the one thing that makes the 401 predicate
/// typed instead of hand-written.
pub async fn connect_client<T, E, A>(
    handler: McpClientHandler,
    transport: T,
    lifecycle: ClientLifecycleMode,
    ct: CancelToken,
) -> Result<RunningService<RoleClient, McpClientHandler>, ClientInitializeError>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    rmcp::service::serve_client_with_lifecycle_and_ct(handler, transport, lifecycle, ct).await
}

/// [`connect_client`] under `requestOptions.timeout` — the other half of
/// `abortable(client.connect(transport, requestOptions), signal)` (`server-manager.ts:660`).
///
/// # Why this exists at all
///
/// `CreateConnection::request_options` is computed by the manager (`server_manager.rs:1651`) once
/// per attempt and was, until this function, **read by nothing**. The consequence was not cosmetic:
/// a server that accepts the connection and then never answers `initialize` parked the connect
/// future — and with it the manager's per-name single-flight slot — until someone called `close`.
/// MEASURED through the real `McpServerManager` with entry
/// `{command:"sh", args:["-c","exec sleep 60"], requestTimeoutMs: 300}`: still hanging after 6 s.
/// Upstream fails that connect at 300 ms. `runtime.rs`'s own comment used to say only that
/// "`requestOptions` does not appear", which reads as "has no effect" rather than as "the handshake
/// is unbounded"; both statements are now written where they can be acted on.
///
/// # Two NAMED DELTAS in what the budget covers
///
/// 1. **Scope.** Upstream arms the timer around the `initialize` *request*
///    (`SdkError(RequestTimeout, "Request timed out", { timeout })`, the SDK's
///    `_setupTimeout(messageId, timeout, …)`); this arms it around rmcp's whole
///    `serve_client_with_lifecycle_and_ct`, which is `initialize` plus the
///    `notifications/initialized` send. The extra span is one buffered write on a transport that
///    has already answered, so the budget is very slightly stricter than upstream's, never looser.
/// 2. **`maxTotalTimeout` / `resetTimeoutOnProgress`** have no upstream analogue in
///    `buildRequestOptions` and are not applied.
///
/// `None` means no timeout, which is `normalizeRequestTimeoutMs`'s answer for an absent, zero,
/// negative, `NaN` or infinite value — see [`resolve_request_timeout`] for why that does **not**
/// fall back to the global.
///
/// # Errors
///
/// The inner `Result` is [`connect_client`]'s. The **outer** `Err` is the timeout, carrying the
/// budget that elapsed so the caller can render it; it is deliberately not a
/// [`ClientInitializeError`], because rmcp has no timeout variant and manufacturing one
/// (`ConnectionClosed`, say) would make a wedged server indistinguishable from a hung-up one.
pub async fn connect_client_bounded<T, E, A>(
    handler: McpClientHandler,
    transport: T,
    lifecycle: ClientLifecycleMode,
    ct: CancelToken,
    timeout: Option<Duration>,
) -> Result<Result<RunningService<RoleClient, McpClientHandler>, ClientInitializeError>, Duration>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let Some(budget) = timeout else {
        return Ok(connect_client(handler, transport, lifecycle, ct).await);
    };
    // Dropping the `connect_client` future is what tears the half-built connection down, and it is
    // the same drop the abort path relies on: `serve_client_with_ct_inner` holds the transport in a
    // local, so the transport (and, for stdio, `ChildWithCleanup::drop`'s `kill()`) goes with it.
    match tokio::time::timeout(budget, connect_client(handler, transport, lifecycle, ct)).await {
        Ok(outcome) => Ok(outcome),
        Err(_elapsed) => Err(budget),
    }
}

/// `new SdkError(SdkErrorCode.RequestTimeout, "Request timed out", { timeout })` — the SDK's
/// per-request timer (`@modelcontextprotocol/client` `src-D_zzAWoS.mjs:6149`). `SdkError`'s
/// constructor is `super(message)`, so `error.message` is **exactly** `Request timed out`; the
/// budget rides in `data`, not in the text, and is therefore not appended here either.
///
/// It reaches the user through the same `McpError::Server` envelope and the same §3.3 step-8 stderr
/// tail as any other handshake failure, which is what a caller of `connect` sees upstream too.
pub const HANDSHAKE_TIMED_OUT: &str = "Request timed out";

fn handshake_timeout_error(server: &str, stderr: Option<&StderrPump>) -> McpError {
    let message = match stderr {
        Some(pump) => with_stderr_tail(
            HANDSHAKE_TIMED_OUT,
            &pump
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ),
        None => HANDSHAKE_TIMED_OUT.to_string(),
    };
    McpError::Server {
        server: server.to_string(),
        message,
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-128 — request options (§3.13)
// -------------------------------------------------------------------------------------------------

/// `normalizeRequestTimeoutMs` (`server-manager.ts:1249-1253`): finite **and** strictly positive,
/// else no timeout.
///
/// The subtle half is in [`resolve_request_timeout`], not here.
#[must_use]
pub fn normalize_request_timeout_ms(timeout_ms: Option<f64>) -> Option<Duration> {
    let ms = timeout_ms?;
    if !ms.is_finite() || ms <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(ms / 1000.0))
}

/// `getResolvedRequestTimeoutMs(definition)` (`server-manager.ts:234-239`).
///
/// **The trap**: an invalid *per-server* `requestTimeoutMs` — `0`, negative, `NaN`, `Infinity` —
/// resolves to **no timeout at all**. It does *not* fall back to the global. Upstream's shape is
/// `if (definition?.requestTimeoutMs !== undefined) return normalize(...)`, so the global is only
/// consulted when the per-server key is **absent**; once present, whatever it normalises to wins.
/// A porter who writes `definition.or(global)` after normalising gets a plausible-looking
/// implementation that silently reinstates a 30-second cap the user disabled on purpose.
#[must_use]
pub fn resolve_request_timeout(entry: Option<&ServerEntry>, global_ms: Option<f64>) -> Option<Duration> {
    match entry.and_then(|entry| entry.request_timeout_ms) {
        Some(per_server) => normalize_request_timeout_ms(Some(per_server)),
        None => normalize_request_timeout_ms(global_ms),
    }
}

/// `buildRequestOptions(definition, signal)` (`server-manager.ts:241-256`).
///
/// Upstream returns `undefined` when *neither* a signal nor a timeout exists, and the object is
/// shared by the connect and all three discovery list calls. Here only the timeout half is
/// representable: [`PeerRequestOptions`] has no signal field, because rmcp cancels a request by
/// dropping its future — the `ownedSignal` half therefore lives in the `abortable(..)` wrapper
/// around each call rather than inside the options object.
///
/// `reset_timeout_on_progress` and `max_total_timeout` have no upstream analogue and stay at their
/// defaults.
#[must_use]
pub fn build_request_options(
    entry: Option<&ServerEntry>,
    global_ms: Option<f64>,
) -> Option<PeerRequestOptions> {
    resolve_request_timeout(entry, global_ms).map(PeerRequestOptions::with_timeout)
}

// =================================================================================================
// The `createConnection` body — MCP-101 (stdio), MCP-109/113/114/115/115a (HTTP)
//
// `server_manager.rs` owns the state machine around `createConnection` and declares it as the
// [`ConnectionFactory`] seam; this is that seam's body. Upstream it is one 180-line method
// (`server-manager.ts:442-624`) whose two arms share a catch; here the two arms are
// [`ConnectionBuilder::connect_stdio`] and [`ConnectionBuilder::connect_http_client`] and the shared
// catch is [`ConnectionBuilder::create_connection`], which is the one place the §3.3 step-8 stderr
// enrichment and the `MCP connection setup failed` wrapper live.
//
// What is deliberately NOT here, so a reader does not go looking:
//
// * **npx/npm pre-resolution (MCP-103).** `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary`
//   is `pub(super)` and its promotion is MCP-103's, in a crate this unit does not own. The call site
//   is marked below so landing MCP-103 is three lines.
// * **Discovery (MCP-119).** `NewConnection` has nowhere to put tools/resources/prompts —
//   `ServerConnection::new` hardcodes them empty — so a factory *cannot* deliver them through this
//   seam as it stands. Widening `NewConnection` is `server_manager.rs`'s change, not this one.
// * **`wrapTransportWithMcpTrace` (T-10/MCP-477).** No trace observer is threaded yet.
// =================================================================================================

/// `new McpOAuthProvider(serverName, serverUrl, extractOAuthConfig(definition), …)` — the provider
/// half of §3.4's ladder, as a seam.
///
/// **MCP-115 owns the state machine and the ordering; the provider itself is section 05's
/// `rmcp::transport::auth` work** (13c:1330). This trait is the line between them: the ladder
/// decides *when* a provider must exist and what a 401 means, and an implementation decides what
/// `Authorization` value — if any — that provider can produce right now.
///
/// # The default is not a stub, it is one of upstream's two real outcomes
///
/// Upstream constructs the provider with `{ onRedirect: async () => {} }`, so when the SDK's
/// `auth()` decides a browser round trip is required it completes with `"REDIRECT"`, nothing is
/// opened, and the transport raises `UnauthorizedError` again. The connect therefore ends at
/// `needs-auth` for every server with no usable stored credential — which is exactly what
/// [`NoStoredCredentials`] produces. What the default cannot reproduce is the *other* outcome: a
/// server whose credential IS in the store connects on the retry. Binding that arm is a matter of
/// calling [`crate::oauth::get_valid_token`], which needs an
/// [`crate::oauth::AuthenticateOptions`] — the storage handle and the OAuth runtime — and those are
/// held by the manager, not by this crate's transport layer.
pub trait HttpAuthProvider: Send + Sync + std::fmt::Debug + 'static {
    /// The `Authorization` value this attempt should carry, **without** the `Bearer ` prefix (rmcp
    /// applies the scheme through `auth_header`), or `None` when the provider has nothing to offer.
    ///
    /// `challenge` is the `WWW-Authenticate` header of the 401 that promoted an implicit provider,
    /// and is `None` on the `explicit` arm — upstream's provider ignores it entirely, while rmcp's
    /// `AuthorizationRequest::with_challenge` consumes it, so it is carried rather than dropped.
    fn authorize<'a>(
        &'a self,
        server: &'a str,
        url: &'a str,
        challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>>;

    /// `invalidateAuthEntryCache(name)` (`mcp-auth.ts`) — forget the cached entry so the next read
    /// reloads secure storage. The **once-per-episode policy** is the ladder's, not this
    /// implementation's: see [`ConnectionBuilder::connect_http_client`]'s `invalidated` flag
    /// (MCP-116).
    fn invalidate_auth_entry_cache(&self, server: &str);
}

/// The default [`HttpAuthProvider`]: no credential is available, ever.
///
/// See [`HttpAuthProvider`] for why this is upstream-faithful rather than inert — it reproduces the
/// `onRedirect: async () => {}` outcome, which is the one a first-time login takes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoStoredCredentials;

impl HttpAuthProvider for NoStoredCredentials {
    fn authorize<'a>(
        &'a self,
        _server: &'a str,
        _url: &'a str,
        _challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>> {
        Box::pin(async { Ok(None) })
    }

    fn invalidate_auth_entry_cache(&self, _server: &str) {}
}

/// `createClient(serverName, definition)` — how the builder obtains its [`McpClientHandler`].
///
/// A seam because every hook the handler carries (`registerSamplingHandler`,
/// `registerElicitationHandler`, the three `listChanged.onChanged` callbacks, the URL-elicitation
/// completion sink) is owned by the **manager**, which holds the connection map the identity guards
/// compare against. The builder knows the server name and the runtime signal and nothing else.
pub type HandlerFactory = Arc<dyn Fn(&str, &CancelToken) -> McpClientHandler + Send + Sync>;

/// The hookless handler — sampling and elicitation are not advertised, and `list_changed`
/// notifications are logged and dropped. What [`ConnectionBuilder::new`] installs until a manager
/// supplies its own.
#[must_use]
pub fn bare_handler_factory() -> HandlerFactory {
    Arc::new(|server: &str, runtime_signal: &CancelToken| {
        McpClientHandler::new(McpClientHandlerParts {
            server: server.to_string(),
            runtime_signal: runtime_signal.clone(),
            elicitation_mode: None,
            sampling: None,
            elicitation: None,
            list_changed: None,
            elicitation_complete: None,
        })
    })
}

/// `isUnauthorizedHttpError(error)` (`server-manager.ts:73-75`) **and** the challenge it carries.
///
/// Upstream is `error instanceof UnauthorizedError || (error instanceof SdkHttpError && error.status
/// === 401)` — **401 only**. rmcp offers `ClientInitializeError::auth_challenge()`, which is nearly
/// this but not quite: it matches `AuthRequiredError` (401) *and* `InsufficientScopeError` (403),
/// because rmcp's reactive-OAuth flow treats an insufficient-scope 403 as actionable too. Upstream
/// does not: a 403 falls past both 401 arms, past `shouldFallbackToSse` (403 is not in its
/// `[404, 405, 406, 415]`), and out of `connectHttpClient` as a hard error.
///
/// So the predicate here walks the same `source()` chain rmcp's does and downcasts **only**
/// `AuthRequiredError`. That is one hand-written walk against the plan's "the 401 predicate is not
/// hand-written", and the reason is stated so it can be overruled: using `auth_challenge()` as-is
/// would turn every scope-denied 403 into an OAuth retry and then into `needs-auth`, which is a
/// different user-visible outcome from upstream's hard failure on a path that touches credentials.
///
/// `Some("")` is a real value, and it is [`bare_unauthorized`]'s answer: a 401 with **no**
/// `WWW-Authenticate` header still authorizes the retry, and the challenge is genuinely absent
/// rather than empty-by-accident. [`crate::oauth::on_unauthorized`] takes `Option<&str>` precisely
/// so that case is expressible.
///
/// # The bare 401 is why this function is not just the `AuthRequiredError` downcast
///
/// rmcp builds `AuthRequiredError` **only** inside
/// `if response.status() == UNAUTHORIZED && let Some(header) = response.headers().get(WWW_AUTHENTICATE)`
/// (`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:210-222` for POST, `:97-110`
/// for GET). A 401 with no challenge header therefore never becomes that type, and a downcast-only
/// predicate answers `None` for it — which sends a permanently-401 server down arm 7 as a hard
/// connect failure and never offers the user `/mcp-auth`.
///
/// Upstream has no such gate: `isUnauthorizedHttpError` is
/// `error instanceof UnauthorizedError || (error instanceof SdkHttpError && error.status === 401)`
/// (`server-manager.ts:73-75`) — the header plays no part. Confirmed in the pinned SDK: with no
/// `authProvider` a 401 falls past the `response.status === 401 && this._authProvider` guard to
/// `throw new SdkHttpError(..., { status: response.status })`, and with a provider a 401 lacking
/// `www-authenticate` still reaches `throw markAuthSeamEscape(new UnauthorizedError())`.
///
/// MEASURED before this widening, against a loopback fixture answering every `initialize` with
/// `HTTP/1.1 401 Unauthorized` and no `WWW-Authenticate`, an implicit-OAuth entry and a counting
/// provider: one attempt, no retry, no `needs-auth`, no `invalidateAuthEntryCache` — the connect
/// failed with `unexpected server response: HTTP 401 Unauthorized`.
///
/// The `InsufficientScopeError` exclusion survives the widening untouched: a 403 becomes
/// [`StreamableHttpError::InsufficientScope`] when it carries a challenge and an
/// `UnexpectedServerResponse` reading `HTTP 403 …` when it does not, and [`bare_unauthorized`]
/// matches neither.
#[must_use]
pub fn unauthorized_challenge(error: &ClientInitializeError) -> Option<&str> {
    // The walk has to START at `DynamicTransportError::error`, not at `error.source()`.
    // MEASURED: `ClientInitializeError::TransportError`'s `#[error("Send message error {error},
    // when {context}")]` carries **no** `#[source]` and its field is not named `source`, so
    // thiserror generates no `source()` edge and a chain walk rooted at the `ClientInitializeError`
    // finds nothing. The first draft of this function did exactly that and every 401 fell through
    // to the hard-error arm — a permanent-401 server produced a connect failure instead of
    // `needs-auth`, which is the outcome MCP-115's verify line is about. rmcp's own
    // `auth_challenge` (`service/client.rs:110-131`) starts at `error.error.as_ref()` for this
    // reason, and so does this.
    let transport = match error {
        ClientInitializeError::TransportError { error, .. } => error,
        // A 401 in the fallback leg is still actionable. Reachable only under
        // `ClientLifecycleMode::Auto` (`protocolVersion: "auto"`).
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            return unauthorized_challenge(fallback);
        }
        _ => return None,
    };
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(transport.error.as_ref());
    while let Some(current) = source {
        if let Some(required) =
            current.downcast_ref::<rmcp::transport::streamable_http_client::AuthRequiredError>()
        {
            return Some(&required.www_authenticate_header);
        }
        if bare_unauthorized(current) {
            return Some("");
        }
        source = std::error::Error::source(current);
    }
    None
}

/// rmcp's rendering of `HTTP {status}: {body}` for a status it has no variant for
/// (`streamable_http_client.rs:296`, `format!("HTTP {status}: {body}")` with `status:
/// reqwest::StatusCode`, whose `Display` is `401 Unauthorized`).
const UNEXPECTED_UNAUTHORIZED_PREFIX: &str = "HTTP 401 ";

/// Is this link in the chain a 401 that rmcp did **not** turn into an `AuthRequiredError`?
///
/// # Why one arm is a string prefix, stated rather than hidden
///
/// rmcp keeps the status on the error in exactly one of its two 401 shapes:
///
/// * [`StreamableHttpError::Client`] wraps the `reqwest::Error` from `error_for_status()`, which
///   still carries `.status()`. This is the GET/SSE leg. TYPED — no parsing.
/// * [`StreamableHttpError::UnexpectedServerResponse`] carries a `Cow<str>` and **nothing else**.
///   This is the POST leg, i.e. the one `initialize` uses, so it is the arm that actually matters.
///   The status is only in the text, so matching it is a prefix test against rmcp's own format
///   string. There is no typed channel to prefer: the variant is `(Cow<'static, str>)`.
///
/// The fragility is real and bounded — if rmcp changes that format the predicate silently narrows
/// back to the header-carrying 401, which is the behaviour this function was written to fix. It is
/// pinned by `a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder`, which drives a real
/// loopback server through the real transport, so an rmcp upgrade that changes the wording fails a
/// test rather than a user.
#[must_use]
pub fn bare_unauthorized(error: &(dyn std::error::Error + 'static)) -> bool {
    let Some(http) = error.downcast_ref::<
        rmcp::transport::streamable_http_client::StreamableHttpError<reqwest::Error>,
    >() else {
        return false;
    };
    match http {
        rmcp::transport::streamable_http_client::StreamableHttpError::Client(client) => {
            client.status() == Some(reqwest::StatusCode::UNAUTHORIZED)
        }
        rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
            message,
        ) => message.starts_with(UNEXPECTED_UNAUTHORIZED_PREFIX),
        _ => false,
    }
}

/// The tuned `reqwest::Client` `StreamableHttpClientTransport::from_config` would have built.
///
/// rmcp's `default_http_client` is **private**, so a port that needs to wrap the client — this one
/// does, twice: [`crate::request_headers_command::RequestHeadersCommandClient`] and, when section 05
/// lands it, `AuthClient` — has to rebuild it. Both settings are load-bearing and are copied from
/// `rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:381-387` with its reasons:
///
/// * `pool_max_idle_per_host(0)` — idle pooling causes ~40 ms stalls from TCP delayed ACK on Linux
///   when a previous response body was not fully consumed before reuse.
/// * `redirect(Policy::none())` — so caller-supplied custom headers (which here include resolved
///   **secrets**) cannot be replayed to a redirect target. This one is a security property, not a
///   latency one.
///
/// # Errors
///
/// [`McpError::Other`] when the TLS backend fails to initialise. rmcp `expect()`s here; a
/// connect-time failure is a better answer than a panic inside a session.
pub fn build_http_client() -> McpResult<reqwest::Client> {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| McpError::other(format!("Failed to build the MCP HTTP client: {error}")))
}

/// `{ client, transport }` — the live half of a connection, as `server_manager`'s
/// [`ConnectionResource`].
///
/// # Why this is not `StdioChildConnection`
///
/// `server_manager::StdioChildConnection` adopts a raw `TokioChildProcess` and calls
/// `graceful_shutdown` on it. That shape only exists for a caller that never handed the transport to
/// rmcp. This builder *does* — `serve_client_with_lifecycle_and_ct` takes the transport **by value**
/// — so the child is owned by the `RunningService` from the handshake onward and `close()` has to go
/// through it. Closing the service cancels its task, which closes the transport, which is
/// `TokioChildProcess::close` → the same `graceful_shutdown`. One owner, one close.
pub struct McpConnection {
    /// `None` once shutdown has completed, which is what makes [`ConnectionResource::close`]
    /// idempotent. A `tokio::sync::Mutex` because the guard is held **across** the close await, for
    /// the reason `StdioChildConnection` states at length: a cancelled close must leave the service
    /// where the next caller finds it, not an empty slot that answers `Ok(())` having killed
    /// nothing.
    service: tokio::sync::Mutex<Option<RunningService<RoleClient, McpClientHandler>>>,
    identity: ClientIdentity,
    peer: Peer<RoleClient>,
    has_session_id: bool,
    pid: Option<u32>,
    /// `stderrTail` — `None` for HTTP and for `debug: true` stdio, both of which have no pipe.
    stderr: Option<StderrPump>,
}

/// The `"data"` listener of §3.3 step 8 and the task that drains it.
struct StderrPump {
    tail: Arc<std::sync::Mutex<VecDeque<u8>>>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// How long [`StderrPump::settle`] waits for the drain task to reach EOF before giving up on it.
///
/// Not an upstream constant — upstream has no wait at all; see [`StderrPump::settle`] for why this
/// port needs one and why the number is not load-bearing.
const STDERR_SETTLE: Duration = Duration::from_millis(250);

impl StderrPump {
    /// Let the drain task finish before the tail is read.
    ///
    /// **This is a named delta, and it exists because the port measured a race upstream cannot
    /// have.** Node delivers `"data"` on the same event loop that drives the transport, so by the
    /// time `createConnection`'s catch runs, everything the child already wrote has been appended
    /// and `stderrTail` is complete. Here the drain is a separate tokio task, and
    /// `connect_client`'s error can win the race — MEASURED: with the suite under load, the
    /// assertion that a failed connect carries `(…)` failed intermittently because the tail was
    /// still empty when the message was built.
    ///
    /// Waiting is correct rather than merely convenient: `serve_client_with_lifecycle_and_ct`
    /// closes the transport on every failure path, which kills the child, which EOFs the pipe, so
    /// the task this awaits is already finishing. [`STDERR_SETTLE`] bounds the one case where it is
    /// not — a child that survived the transport close — and a quarter of a second on a connect
    /// that has already failed is not a latency anyone can observe.
    async fn settle(&self) {
        let handle = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(handle) = handle else { return };
        let aborter = handle.abort_handle();
        // `timeout` DROPS the `JoinHandle` on expiry, and dropping a `JoinHandle` detaches rather
        // than cancels — hence the explicit abort. Without it a child that never EOFs leaves one
        // task parked on a pipe for the life of the process.
        if tokio::time::timeout(STDERR_SETTLE, handle).await.is_err() {
            aborter.abort();
        }
    }
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConnection")
            .field("pid", &self.pid)
            .field("has_session_id", &self.has_session_id)
            .field(
                "live",
                &self.service.try_lock().map_or(true, |slot| slot.is_some()),
            )
            .finish()
    }
}

impl McpConnection {
    /// The `Peer` every request goes through. **Not on the [`ConnectionResource`] trait**, which
    /// exposes only `close`/`has_session_id`/`child_pid`/`stderr_detail` — so a manager holding an
    /// `Arc<dyn ConnectionResource>` cannot reach it. Discovery (MCP-119) and `tools/call` need it,
    /// and giving them a way to get it is a `server_manager.rs` change (a `peer()` method on the
    /// trait, or a typed field on `NewConnection`), named here rather than smuggled in.
    #[must_use]
    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
    }

    /// The handler identity `list_changed` guards `ptr_eq` against (MCP-120).
    #[must_use]
    pub fn identity(&self) -> ClientIdentity {
        self.identity.clone()
    }
}

impl ConnectionResource for McpConnection {
    fn close(&self) -> BoxFuture<'_, McpResult<()>> {
        Box::pin(async move {
            if let Some(pump) = self.stderr.as_ref()
                && let Some(task) = pump
                    .task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
            {
                task.abort();
            }
            let mut slot = self.service.lock().await;
            let Some(service) = slot.as_mut() else {
                return Ok(());
            };
            // `RunningService::close(&mut self)` cancels the service task and awaits it, and that
            // task is what runs `transport.close()`. `cancel(self)` would consume, which a
            // `&self` trait method cannot do — this is why the slot is an `Option` behind a mutex
            // rather than the service itself.
            let outcome = service.close().await;
            drop(slot.take());
            outcome.map(|_| ()).map_err(|join| {
                McpError::other(format!("MCP connection close failed: {join}"))
            })
        })
    }

    fn has_session_id(&self) -> bool {
        self.has_session_id
    }

    fn child_pid(&self) -> Option<u32> {
        self.pid
    }

    fn stderr_detail(&self) -> Option<String> {
        let pump = self.stderr.as_ref()?;
        stderr_tail_detail(
            &pump
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

/// `createConnection`'s two arms and the catch they share — the [`ConnectionFactory`] this port
/// installs in place of `UnbuiltConnectionFactory`.
///
/// Everything the manager reads off `this` inside `createConnection` is a field here:
/// `this.defaultCwd`, the client construction, and (through [`HttpAuthProvider`]) the
/// `authStorageOptions` / `oauthRuntime` the OAuth provider would be built from.
pub struct ConnectionBuilder {
    default_cwd: Option<PathBuf>,
    env: crate::credentials::EnvFn,
    /// `{ ...process.env }` — the map `resolveEnv` copies before layering the per-server overrides.
    /// A snapshot rather than a lookup because `StdioClientTransport`'s `env` option **replaces**
    /// the child environment, so the whole set has to be enumerable.
    base_env: HashMap<String, String>,
    home: PathBuf,
    handler: HandlerFactory,
    auth: Arc<dyn HttpAuthProvider>,
}

impl std::fmt::Debug for ConnectionBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionBuilder")
            .field("default_cwd", &self.default_cwd)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl ConnectionBuilder {
    /// `new McpServerManager(defaultCwd)`'s half of the connection builder.
    ///
    /// `default_cwd` is upstream's `this.defaultCwd` — the session working directory, used only when
    /// `definition.cwd` is absent, and `None` reproduces the *"omit the key entirely"* arm of §3.3
    /// step 6.
    #[must_use]
    pub fn new(default_cwd: Option<PathBuf>) -> Self {
        Self {
            default_cwd,
            env: crate::credentials::process_env(),
            base_env: crate::secrets::process_env_snapshot(),
            home: crate::dirs::home_dir(),
            handler: bare_handler_factory(),
            auth: Arc::new(NoStoredCredentials),
        }
    }

    /// Install the manager's `createClient` — the hooks of §3.5, §3.10 and §3.16.
    #[must_use]
    pub fn with_handler_factory(mut self, handler: HandlerFactory) -> Self {
        self.handler = handler;
        self
    }

    /// Install the OAuth provider seam (MCP-115). See [`HttpAuthProvider`].
    #[must_use]
    pub fn with_auth_provider(mut self, auth: Arc<dyn HttpAuthProvider>) -> Self {
        self.auth = auth;
        self
    }

    /// Override `process.env` and `os.homedir()`. Tests only — production reads the real ones, and
    /// edition 2024 makes `std::env::set_var` `unsafe`, which is why this is a seam and not a
    /// global.
    ///
    /// `env` is the *lookup* (`interpolateEnvVars`, `resolveServerUrl`, `resolveBearerToken`) and
    /// `base_env` is the *snapshot* `resolveEnv` copies; upstream both are `process.env` and here
    /// they are separate because an `EnvFn` cannot be enumerated.
    #[must_use]
    pub fn with_environment(
        mut self,
        env: crate::credentials::EnvFn,
        base_env: HashMap<String, String>,
        home: PathBuf,
    ) -> Self {
        self.env = env;
        self.base_env = base_env;
        self.home = home;
        self
    }
}

/// What one turn of §3.4's ladder produced.
enum HttpAttempt {
    /// `{ status: "connected", client, transport }`.
    Connected(Arc<McpConnection>),
    /// `{ status: "failed", client, transport, error }`. The client and transport are **not**
    /// carried: rmcp's `serve_client_with_lifecycle_and_ct` has already closed the transport on
    /// every failure path, so there is nothing left for the ladder to hand back or close. That is
    /// the whole of the difference between this and upstream's tuple — see
    /// [`ConnectionBuilder::connect_http_client`]'s "what rmcp owns" note.
    /// Boxed because `ClientInitializeError` is ~376 bytes and the success arm is a pointer;
    /// `clippy::large_enum_variant` is right that an unboxed union would cost every attempt the
    /// size of its own worst case.
    Failed(Box<ClientInitializeError>),
    /// `requestOptions.timeout` elapsed before the handshake settled — a rejection upstream too
    /// (`SdkError(RequestTimeout, "Request timed out")`), so it takes the same ladder path a
    /// non-401 failure takes: past arm 3's abort check, past the 401 arms (it is not one), out at
    /// arm 7. It is a separate variant only because rmcp has no timeout `ClientInitializeError`.
    TimedOut,
}

/// `connectHttpClient`'s return — `{ client, transport, status, credentialsInvalidated }`.
#[derive(Debug)]
pub struct HttpConnection {
    /// The live half, or — on the `needs-auth` arm — a resource with nothing to close.
    pub resource: Arc<dyn ConnectionResource>,
    /// `"connected"` or `"needs-auth"`.
    pub status: ConnectionStatus,
    /// `credentialsInvalidated`, possibly set by this connect's own 401 handling (MCP-116).
    pub credentials_invalidated: bool,
}

impl ConnectionBuilder {
    /// The stdio arm of `createConnection` (`server-manager.ts:472-503`), §3.3.
    ///
    /// Upstream's order is load-bearing and is reproduced exactly:
    ///
    /// 1. `client = this.createClient(name, definition)` — which runs `resolveVersionNegotiation`,
    ///    so an invalid `protocolVersion` throws **before a child is spawned**. A port that built
    ///    the transport first would leave a process behind on that throw.
    /// 2. `args = (definition.args ?? []).map(interpolateEnvVars)` — `args` are interpolated and
    ///    **never** `!command`-resolved; `command` is neither.
    /// 3. the npx/npm rewrite (MCP-103 — see the marker below).
    /// 4. `throwIfAborted(signal)`.
    /// 5. `mkdirSync(definition.pluginDataDir, { recursive: true })`.
    /// 6. `cwd = resolveConfigPath(definition.cwd) ?? this.defaultCwd`.
    /// 7. `new StdioClientTransport({...})` — which is where `resolveEnv` runs, and therefore where
    ///    a `!command` env value executes.
    /// 8. the `"data"` listener that feeds the bounded stderr tail.
    ///
    /// Steps 5–7 are [`spawn_stdio_transport`] and [`StdioTransportSpec::resolve`]; this method is
    /// the ordering and the two rewrites.
    ///
    /// # Errors
    ///
    /// `Invalid MCP protocolVersion: …`; the abort reason; [`McpError::Io`] for the plugin data
    /// directory or the spawn; one of `resolveCommandSecret`'s five sentences for an `env` value;
    /// or the handshake failure itself.
    async fn connect_stdio(&self, request: &CreateConnection) -> McpResult<Arc<McpConnection>> {
        let entry = request.definition.as_ref();
        let name = request.name.as_str();

        // Step 1. `createClient` — the throw has to precede the spawn.
        let lifecycle = version_negotiation(entry)?;
        let handler = (self.handler)(name, &request.request);
        let identity = handler.identity();

        // Step 2. Note `command` is used raw: upstream maps `interpolateEnvVars` over `args` only.
        let command = entry.command.clone().unwrap_or_default();
        let args: Vec<String> = entry
            .args
            .iter()
            .flatten()
            .map(|arg| crate::credentials::interpolate_env_vars(arg, &self.env))
            .collect();

        // Step 3 — MCP-103, NOT PORTED. Upstream:
        //   if (command === "npx" || command === "npm") {
        //     const resolved = await resolveNpxBinary(command, args, signal);
        //     if (resolved) { command = resolved.isJs ? "node" : resolved.binPath;
        //                     args = resolved.isJs ? [resolved.binPath, ...resolved.extraArgs]
        //                                          : resolved.extraArgs;
        //                     logger.debug(`${name} resolved to ${resolved.binPath} (skipping npm parent)`); } }
        // `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary` is `pub(super)` in a crate this
        // unit does not own; MCP-103 is its `pub` promotion plus these five lines. Until then the
        // tracked child of an `npx` server is the npm launcher, and §3.12's single-pid kill leaves
        // the real server orphaned — the residual `StdioChildConnection`'s doc already names.

        // Step 4.
        crate::abort::throw_if_aborted(&request.attempt, None)?;

        // Step 6. `resolveConfigPath(definition.cwd) ?? this.defaultCwd` — and `None` is the
        // *"key omitted entirely"* arm, not `current_dir(".")`.
        let cwd = entry
            .cwd
            .as_deref()
            .map(|raw| PathBuf::from(crate::dirs::resolve_config_path(raw, &self.env, &self.home)))
            .or_else(|| self.default_cwd.clone());

        // Steps 5 + 7.
        //
        // `resolve` is moved off the async worker deliberately, and this is the one place in the
        // builder where that matters. `StdioTransportSpec::resolve` → `secrets::resolve_stdio_env`
        // → `resolve_command_secret` is a `std::process::Command` spawn polled with
        // `std::thread::sleep`, bounded by `COMMAND_SECRET_TIMEOUT` (10 s) and cancellable by
        // nothing. Run inline it would hold a tokio worker for up to ten seconds **inside the
        // manager's single-flight connect future** — a `futures::Shared` first polled on the
        // detached tail task (`server_manager.rs`'s `handle.spawn(tail)`) — during which
        // `close`/`close_all`'s abort could not preempt the attempt, which is precisely the
        // guarantee wave 4's concurrency tests were measured on (they were measured against
        // `UnbuiltConnectionFactory`, which returned instantly, so nothing there would have caught
        // it). Upstream's `spawnSync` blocks node's whole event loop, so leaving it inline would be
        // arguable *parity*; it is still the one way this `createConnection` body can weaken a
        // guarantee the rest of the crate relies on, so it does not stay inline.
        //
        // What `spawn_blocking` does NOT buy: the command itself is still uncancellable and still
        // runs to its own timeout after an abort. That is parity — a `spawnSync` cannot be
        // interrupted either — and the point of the wrapper is the worker thread, not the child.
        let spec = {
            let definition = Arc::clone(&request.definition);
            let server_name = name.to_string();
            let base_env = self.base_env.clone();
            match tokio::task::spawn_blocking(move || {
                StdioTransportSpec::resolve(
                    &definition,
                    &server_name,
                    command,
                    args,
                    cwd,
                    &base_env,
                )
            })
            .await
            {
                Ok(spec) => spec?,
                // The closure cannot panic under this crate's lint policy and the runtime is alive
                // (we are running on it), so this arm is defensive. Reported rather than unwrapped:
                // a `JoinError` here is a local failure, and it names the step it happened in.
                Err(_join) => {
                    return Err(McpError::Server {
                        server: name.to_string(),
                        message: "MCP stdio environment resolution failed to run".to_string(),
                    });
                }
            }
        };
        let (process, stderr) = spawn_stdio_transport(&spec)?;
        let pid = process.id();

        // Step 8. Started BEFORE the handshake: between the spawn and the first read the child can
        // already have filled the 64 KiB pipe, and a child blocked in `write` never answers
        // `initialize`. The task is aborted by `McpConnection::close`.
        let stderr = stderr.map(start_stderr_pump);

        // `await this.connectClientWithAbort(client, transport, requestOptions, signal)`.
        // `requestOptions` is split: its signal half is the `ct` argument, its timeout half is
        // [`connect_client_bounded`]'s budget. A wedged child — one that accepts the pipe and never
        // answers `initialize` — fails here at `requestTimeoutMs` instead of parking this future
        // and the manager's single-flight slot for that name forever.
        let handshake = connect_client_bounded(
            handler,
            process,
            lifecycle,
            request.attempt.clone(),
            request.request_options.as_ref().and_then(|options| options.timeout),
        )
        .await;
        let handshake = match handshake {
            Ok(handshake) => handshake,
            Err(_budget) => {
                // Same order as the failure arm below: let the drain settle so the tail is whole,
                // then report. The child is already dying — dropping the `connect_client` future
                // dropped the transport, and `ChildWithCleanup::drop` spawns `kill()`.
                if let Some(pump) = stderr.as_ref() {
                    pump.settle().await;
                }
                return Err(handshake_timeout_error(name, stderr.as_ref()));
            }
        };
        match handshake {
            Ok(service) => Ok(Arc::new(McpConnection {
                peer: service.peer().clone(),
                service: tokio::sync::Mutex::new(Some(service)),
                identity,
                has_session_id: false,
                pid,
                stderr,
            })),
            Err(error) => {
                // What happens to the child here, stated exactly, because "rmcp cleans up" is not
                // precise enough to rely on: `serve_client_with_ct_inner` holds the transport in a
                // local, so every `?` and the `tokio::select!` cancellation arm **drop** it — and
                // `TokioChildProcess`'s `ChildWithCleanup::drop`
                // (`rmcp-3.1.4/src/transport/child_process.rs:45-57`) spawns a fire-and-forget
                // `kill()`. NAMED DELTA: that is a SIGKILL with no graceful window, where upstream's
                // catch runs `client.close()` and the TS SDK escalates close-stdin → 2 s → SIGTERM →
                // 2 s → SIGKILL. A stdio server that would have flushed state on SIGTERM does not
                // get the chance when its *handshake* fails. It is bounded to the failed-connect
                // path — a successful connection is torn down through `McpConnection::close`, which
                // is `graceful_shutdown` — and `a_failed_handshake_leaves_no_child_behind` pins that
                // the child does die.
                //
                // What is NOT rmcp's at all is the drain task: it has to be allowed to finish before
                // the tail is read, and then stopped.
                if let Some(pump) = stderr.as_ref() {
                    pump.settle().await;
                }
                Err(initialize_error(name, &error, stderr.as_ref()))
            }
        }
    }

    /// `connectHttpClient(definition, serverName, requestOptions, signal, traceObserver,
    /// credentialsInvalidated)` (`server-manager.ts:826-970`) — §3.4 steps 1–7 and the attempt
    /// ladder, MCP-109/113/114/115/115a.
    ///
    /// # The ladder's arm order IS the specification
    ///
    /// 1. connected ⇒ return;
    /// 2. the abort-cleanup aggregate ⇒ **rethrow**, never retried;
    /// 3. `if (signal?.aborted) throwIfAborted(signal)`;
    /// 4. implicit-deferred + 401 ⇒ construct the provider and retry the **same** kind;
    /// 5. any other 401 ⇒ `needs-auth` when OAuth is supported (invalidating the credential cache
    ///    at most once per episode), else rethrow;
    /// 6. ~~SSE fallback~~ — Cut 1;
    /// 7. `throw result.error`.
    ///
    /// [`crate::oauth::on_unauthorized`] is arms 4 and 5 as one function; the order of everything
    /// around it is here.
    ///
    /// # What rmcp owns, and what that costs
    ///
    /// Upstream's `attempt` has its own catch that closes the failed attempt's client and, when
    /// *that* close fails, raises `MCP HTTP connection cleanup failed`
    /// ([`McpError::HttpCleanupFailed`]). In this port `serve_client_with_ct_inner` holds the
    /// transport in a local and **drops** it on every failure path — a drop cannot report a
    /// failure, so there is no separate cleanup outcome to observe: **neither [`McpError::HttpCleanupFailed`] nor
    /// [`McpError::AbortCleanupFailed`] has a producer on this path**, and arm 2 is therefore
    /// written-but-unreachable. That is MCP-123's residual verbatim ("the once-only cleanup handle
    /// for the HTTP retry ladder … and the cleanup-failure-versus-connect-failure distinction"),
    /// not something this unit closed and not something it can close from here.
    ///
    /// # Errors
    ///
    /// `resolveServerUrl`'s two sentences; any of `resolveCommandSecret`'s five; the
    /// invalid-header-value sentence; `Invalid MCP protocolVersion: …`; the request-headers-command
    /// validation sentences; the abort reason; or the handshake failure.
    pub async fn connect_http_client(
        &self,
        request: &CreateConnection,
    ) -> McpResult<HttpConnection> {
        let entry = request.definition.as_ref();
        let name = request.name.as_str();

        crate::abort::throw_if_aborted(&request.attempt, None)?;

        // Step 1. `const serverUrl = resolveServerUrl(definition)!` — throws before any secret
        // command is spawned for a URL that cannot resolve.
        let Some(server_url) =
            crate::credentials::resolve_server_url(entry.url.as_deref(), &self.env)?
        else {
            // `resolveServerUrl` returns `undefined` only for an absent `url`, and
            // `select_transport` has already established there is one. `!` upstream.
            return Err(McpError::Config(format!(
                "Server {name} must configure exactly one of command or url"
            )));
        };

        // Steps 2–6: `hasCommandHeader`, `resolveCommandSecretsRecord`, `commandBearer`, the bearer
        // ladder and the `new Headers()` injection guard — all of `crate::secrets`.
        let spec = HttpTransportSpec::resolve(entry, name, server_url.clone(), &self.env)?;

        // `server-manager.ts:868-870`: built **once per connect**, above `attempt`, and spread into
        // every attempt's transport options. The plan's cyrup note says "inside the retry closure";
        // the .ts says outside, and outside is what makes the eager `resolvedCommand(config)`
        // validation (`request-headers-command.ts:309`) fail the CONNECT exactly once rather than
        // once per attempt. The decorator is reused across attempts because the thing that must be
        // fresh per attempt is the MCP client, not the HTTP one.
        let http_client = build_http_client()?;
        let signing_client = match entry.request_headers_command.clone() {
            Some(config) => Some(crate::request_headers_command::RequestHeadersCommandClient::new(
                http_client.clone(),
                config,
                Some(request.attempt.clone()),
            )?),
            None => None,
        };

        // `let authState: HttpAuthProviderState = supportsOAuth(definition) ? … : { disabled }`.
        let mut auth_state = crate::oauth::initial_http_auth_state(entry);
        let mut invalidated = request.credentials_invalidated;

        loop {
            // `const authProvider = "provider" in authState ? authState.provider : undefined;`
            // and, with it, the token that provider would attach. Explicit OAuth reads the store on
            // the FIRST attempt; implicit OAuth reaches here only after a 401 has proven it needed
            // to — which is the whole point of the two-state split (MCP-115).
            let oauth_token = match &auth_state {
                crate::oauth::HttpAuthProviderState::Explicit => {
                    self.auth.authorize(name, &server_url, None).await?
                }
                crate::oauth::HttpAuthProviderState::ImplicitChallenged { challenge } => {
                    self.auth
                        .authorize(name, &server_url, challenge.as_deref())
                        .await?
                }
                crate::oauth::HttpAuthProviderState::Disabled
                | crate::oauth::HttpAuthProviderState::ImplicitDeferred => None,
            };

            let attempt = self
                .http_attempt(request, &spec, entry, &http_client, signing_client.as_ref(), oauth_token)
                .await?;

            let failure = match attempt {
                // Arm 1.
                HttpAttempt::Connected(connection) => {
                    return Ok(HttpConnection {
                        resource: connection,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: invalidated,
                    });
                }
                HttpAttempt::Failed(error) => Some(error),
                // The handshake budget elapsed. Carried to the same place a non-401 failure goes,
                // rather than returned from `http_attempt`, so arm 3 still runs first: a `close`
                // racing a timeout is an abort upstream, because `attempt`'s rejection reaches the
                // ladder and the aborted check precedes everything that inspects the error.
                HttpAttempt::TimedOut => None,
            };

            // Arm 2 — `if (result.error instanceof AggregateError && message === "MCP connection
            // abort cleanup failed") throw result.error;`. Unreachable here; see the method doc.

            // Arm 3 — `if (signal?.aborted) throwIfAborted(signal);`. Before the 401 arms, so a
            // close racing a 401 is an abort and not a `needs-auth`.
            crate::abort::throw_if_aborted(&request.attempt, None)?;

            let Some(error) = failure else {
                // Arm 7 for the timeout: it is not a 401, so no OAuth arm can claim it.
                return Err(handshake_timeout_error(name, None));
            };

            // Arms 4 and 5.
            let Some(challenge) = unauthorized_challenge(&error) else {
                // Arm 6 is Cut 1; arm 7 — `throw result.error`.
                return Err(initialize_error(name, &error, None));
            };
            match crate::oauth::on_unauthorized(&auth_state, Some(challenge)) {
                crate::oauth::UnauthorizedAction::RetryOnce(next) => {
                    auth_state = next;
                    continue;
                }
                crate::oauth::UnauthorizedAction::NeedsAuth => {
                    // MCP-116's once-per-episode guard. The flag rides back out on the connection
                    // record and returns on the next `connect`, so a retry loop cannot repeatedly
                    // discard a good cached credential.
                    if !invalidated {
                        self.auth.invalidate_auth_entry_cache(name);
                        invalidated = true;
                    }
                    return Ok(HttpConnection {
                        // Upstream returns the failed attempt's `{client, transport}` here so the
                        // manager can close them later; rmcp has already dropped both by the time
                        // the error surfaces, so the honest resource is one with nothing to close.
                        // The consequence is bounded: an HTTP transport's drop closes its worker
                        // and its reqwest connections, and there is no child process on this arm.
                        resource: crate::server_manager::InertResource::new(),
                        status: ConnectionStatus::NeedsAuth,
                        credentials_invalidated: invalidated,
                    });
                }
                crate::oauth::UnauthorizedAction::HardError => {
                    return Err(initialize_error(name, &error, None));
                }
            }
        }
    }

    /// One turn of the ladder — upstream's `attempt(kind)` closure, minus the `kind` (Cut 1 leaves
    /// one transport).
    ///
    /// A fresh MCP client per attempt, exactly as upstream: `const client =
    /// this.createClient(serverName, definition)` is *inside* `attempt`, so the retry after an
    /// implicit-OAuth challenge does not reuse a client that has already seen a failed
    /// `initialize`.
    async fn http_attempt(
        &self,
        request: &CreateConnection,
        spec: &HttpTransportSpec,
        entry: &ServerEntry,
        http_client: &reqwest::Client,
        signing_client: Option<
            &crate::request_headers_command::RequestHeadersCommandClient<reqwest::Client>,
        >,
        oauth_token: Option<String>,
    ) -> McpResult<HttpAttempt> {
        let name = request.name.as_str();
        let mut config = build_http_transport_config(spec)?;

        // `transportOptions.authProvider`'s effect on the wire. PRECEDENCE, measured from the
        // pinned SDK rather than assumed: `StreamableHTTPClientTransport._commonHeaders`
        // (`node_modules/@modelcontextprotocol/client/dist/index.mjs:5034-5049`) writes the
        // provider's `Authorization` into a plain object and then spreads `requestInit.headers`
        // OVER it inside a single `new Headers({...headers, ...extraHeaders})`. One `Headers`
        // object, therefore exactly one `Authorization` value, and the CONFIGURED one wins.
        //
        // # Why this needs two clauses and not one
        //
        // **rmcp carries the bearer in a separate `auth_header` channel from the custom-header
        // map, so every path that can produce an `Authorization` has to clear the other channel
        // explicitly.** `auth_header` is applied with `RequestBuilder::bearer_auth` and the map with
        // `builder.header(name, value)` — and both of those APPEND
        // (`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:196-203`, `:29-37`).
        // Upstream has one channel and `set` semantics; this port has two channels and `append`
        // semantics, so parity is not the default and has to be restored at every producer. This is
        // the THIRD instance of the same defect class: wave 2 fixed it one layer down in
        // `secrets::resolve_http_secrets` (a resolved `bearerToken` strips a configured
        // `Authorization` out of the map), `request_headers_command::apply_derived` fixed it for a
        // signing command's derived header, and this is the OAuth token's.
        //
        // MEASURED before this clause existed, entry `{url, auth:"oauth", headers:{"Authorization":
        // "Static abc123"}}` with a provider returning `from-store`, against a loopback fixture
        // recording raw request bytes:
        //   authorization values = ["Bearer from-store", "Static abc123"]
        // — two values where upstream sends one, i.e. a credential the config did not name was
        // additionally disclosed to the server.
        //
        // The combination is reachable by config, not hypothetical: `supports_oauth` answers true
        // for `auth: "oauth"` **before** any custom-header gate (`crate::oauth::supports_oauth`,
        // mirroring `mcp-auth-flow.ts:940-953`, whose own doc says "even with custom headers"), and
        // `auth: "oauth"` means no `spec.bearer_token`, so nothing else strips the map's entry. A
        // previous revision of this comment argued the collision was impossible because
        // `supportsOAuth` is false whenever `auth === "bearer"`; that reasoning covers only the
        // `auth_header` channel and misses the `custom_headers` one entirely.
        if config
            .custom_headers
            .contains_key(&http::header::AUTHORIZATION)
        {
            // The configured header wins, matching the spread order above. `oauth_token` is dropped
            // rather than installed — dropping it is what makes the count on the wire one.
        } else if config.auth_header.is_none() {
            config.auth_header = oauth_token;
        }

        // `skipIssuerMetadataValidation` is gated on a provider EXISTING upstream
        // (`server-manager.ts:898-903`)
        // (`authProvider !== undefined && definition.oauth !== false &&
        //   definition.oauth?.skipIssuerMetadataValidation === true`). rmcp's streamable-HTTP
        // transport config has no such field — issuer-metadata validation belongs to
        // `rmcp::transport::auth`'s discovery, which is section 05's — so the flag is read here and
        // handed to nothing. RECORDED: a server that needs `skipIssuerMetadataValidation` will fail
        // discovery once section 05 wires the provider, and this is the line that has to grow an
        // argument then.
        let _skip_issuer_metadata_validation = skip_issuer_metadata_validation(entry);

        // `this.createClient(serverName, definition)` — fresh per attempt.
        let lifecycle = version_negotiation(entry)?;
        let handler = (self.handler)(name, &request.request);
        let identity = handler.identity();

        // `requestOptions.timeout`, per attempt — upstream's object is built once and passed to
        // every `client.connect`, so each turn of the ladder gets the full budget rather than a
        // share of one. See [`connect_client_bounded`].
        let budget = request.request_options.as_ref().and_then(|options| options.timeout);
        // [`SessionIdProbe`] wraps whichever client this attempt uses, so `has_session_id` below is
        // a read of what the server actually sent rather than a constant. The flag is cloned out
        // BEFORE the probe is handed to the transport, which takes its client by value.
        let (outcome, session_id) = match signing_client {
            Some(signing) => {
                let probe = SessionIdProbe::new(signing.clone());
                let session_id = probe.flag();
                (
                    connect_client_bounded(
                        handler,
                        http_transport_with_client(probe, config),
                        lifecycle,
                        request.attempt.clone(),
                        budget,
                    )
                    .await,
                    session_id,
                )
            }
            None => {
                let probe = SessionIdProbe::new(http_client.clone());
                let session_id = probe.flag();
                (
                    connect_client_bounded(
                        handler,
                        http_transport_with_client(probe, config),
                        lifecycle,
                        request.attempt.clone(),
                        budget,
                    )
                    .await,
                    session_id,
                )
            }
        };

        let Ok(outcome) = outcome else {
            return Ok(HttpAttempt::TimedOut);
        };

        Ok(match outcome {
            Ok(service) => HttpAttempt::Connected(Arc::new(McpConnection {
                peer: service.peer().clone(),
                service: tokio::sync::Mutex::new(Some(service)),
                identity,
                // `(transport as {sessionId?: string})?.sessionId != null` (`session-recovery.ts`)
                // — a LIVE read, off the `Mcp-Session-Id` the handshake response carried. See
                // [`SessionIdProbe`] for why it is observed at the client rather than the
                // transport, and for the session-recovery bug the previous hardcoded `true` caused
                // for stateless servers.
                has_session_id: session_id.load(std::sync::atomic::Ordering::Relaxed),
                pid: None,
                stderr: None,
            })),
            Err(error) => HttpAttempt::Failed(Box::new(error)),
        })
    }
}

/// `definition.oauth !== false && definition.oauth?.skipIssuerMetadataValidation === true` —
/// `server-manager.ts:898-903`. See `ConnectionBuilder::http_attempt` for why it currently has no
/// consumer.
#[must_use]
pub fn skip_issuer_metadata_validation(entry: &ServerEntry) -> bool {
    matches!(
        entry.oauth.as_ref(),
        Some(crate::config::OAuthSetting::Config(config))
            if config.skip_issuer_metadata_validation == Some(true)
    )
}

/// Start the §3.3 step-8 drain task over a piped `stderr`.
fn start_stderr_pump(mut stderr: ChildStderr) -> StderrPump {
    let tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let sink = Arc::clone(&tail);
    // `tokio::spawn` panics off-runtime and this crate denies `clippy::panic`; no runtime means no
    // drain, which degrades the tail rather than the connection.
    let task = tokio::runtime::Handle::try_current().ok().map(|handle| {
        handle.spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut buffer = [0_u8; 8192];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => {
                        let Some(chunk) = buffer.get(..read) else { return };
                        let mut tail =
                            sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        append_stderr_tail(&mut tail, chunk);
                    }
                }
            }
        })
    });
    StderrPump {
        tail,
        task: std::sync::Mutex::new(task),
    }
}

/// `createConnection`'s catch, §3.3 step 8: the failure message with the child's last three
/// non-empty stderr lines appended as `(a — b — c)`.
///
/// In `debug: true` mode stderr is inherited by the host terminal, so there is no tail and no
/// suffix — which is why `stderr` is an `Option` rather than an always-present buffer.
fn initialize_error(
    server: &str,
    error: &ClientInitializeError,
    stderr: Option<&StderrPump>,
) -> McpError {
    let base = error.to_string();
    let message = match stderr {
        Some(pump) => with_stderr_tail(
            &base,
            &pump
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ),
        None => base,
    };
    McpError::Server {
        server: server.to_string(),
        message,
    }
}

impl ConnectionFactory for ConnectionBuilder {
    fn create(&self, request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
        // The builder is cheap to clone by `Arc` and the future must be `'static`; every field is
        // either an `Arc` or a small owned value.
        let builder = Self {
            default_cwd: self.default_cwd.clone(),
            env: Arc::clone(&self.env),
            base_env: self.base_env.clone(),
            home: self.home.clone(),
            handler: Arc::clone(&self.handler),
            auth: Arc::clone(&self.auth),
        };
        Box::pin(async move { builder.create_connection(request).await })
    }
}

impl ConnectionBuilder {
    /// `createConnection` end to end — transport selection, the two arms, and the shared catch.
    ///
    /// # The shared catch, and the one thing it cannot do yet
    ///
    /// Upstream's catch closes the half-built connection and, when *that* close fails, wraps
    /// everything in `MCP connection setup failed` ([`McpError::SetupFailed`]). Reaching that arm
    /// needs a post-handshake step that can fail — upstream's is **discovery**
    /// (`Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])`, MCP-119) — and this
    /// builder does not have that one, because `NewConnection` has nowhere to put the results.
    ///
    /// **What it does have, stated precisely, because "no producer" is too strong and was written
    /// here before:** [`Self::post_handshake`]'s own abort check is a genuine post-handshake step
    /// that can fail, and when a `close` racing a successful handshake trips it *and* the
    /// `resource.close()` after it also fails (`McpConnection::close` returns `Err` on a `JoinError`
    /// from `service.close()`), the wrapper below fires and `SetupFailed` is raised against a real
    /// server. That window is narrow — an abort that arrives in the microsecond after the handshake
    /// settled, whose teardown then panics or is cancelled — but it is not empty. What is genuinely
    /// missing is upstream's *own* producer, discovery: landing MCP-119 into the region marked in
    /// `post_handshake` is what makes this arm ordinary rather than exotic.
    ///
    /// `McpServerManager::close_inner`'s pending-connect rethrow arm consumes it and is live:
    /// MCP-124 landed the variant, [`McpError::is_cleanup_failure`] matches it, and
    /// `From<&ManagerError>` keeps the class across the public boundary.
    async fn create_connection(&self, request: CreateConnection) -> McpResult<NewConnection> {
        crate::abort::throw_if_aborted(&request.attempt, None)?;

        // `configuredTransports.length !== 1` — MCP-113, already ported.
        match select_transport(&request.name, request.definition.as_ref())? {
            TransportKind::Stdio => {
                let connection = self.connect_stdio(&request).await?;
                self.post_handshake(&request, connection as Arc<dyn ConnectionResource>, false)
                    .await
            }
            TransportKind::StreamableHttp => {
                let http = self.connect_http_client(&request).await?;
                if http.status == ConnectionStatus::NeedsAuth {
                    // `if (httpConnection.status === "needs-auth") return { … }` — the early return
                    // skips the try block entirely, so no discovery and no notification handlers.
                    return Ok(NewConnection {
                        resource: http.resource,
                        status: ConnectionStatus::NeedsAuth,
                        credentials_invalidated: http.credentials_invalidated,
                    });
                }
                self.post_handshake(&request, http.resource, http.credentials_invalidated)
                    .await
            }
        }
    }

    /// The body of `createConnection`'s `try` after the handshake, and its catch.
    async fn post_handshake(
        &self,
        request: &CreateConnection,
        resource: Arc<dyn ConnectionResource>,
        credentials_invalidated: bool,
    ) -> McpResult<NewConnection> {
        // `throwIfAborted(signal)` — upstream's is at the TOP of the try, before the connect; the
        // one that matters after a successful handshake is `connect`'s own step-9 check, which
        // `McpServerManager` already performs. This one is the port's: a close that raced the
        // handshake must not leave a live child behind just because it arrived a microsecond late.
        //
        // ┌─ MCP-119 lands HERE: `Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])`
        // │  plus `attachAdapterNotificationHandlers` and `client.getInstructions()`. Everything it
        // │  raises flows into the catch below — which is what gives `SetupFailed` its *upstream*
        // │  producer. It already has one here: the abort check on the next line, when the
        // │  `resource.close()` that follows it also fails.
        // └─
        let outcome = crate::abort::throw_if_aborted(&request.attempt, None);

        let Err(error) = outcome else {
            return Ok(NewConnection {
                resource,
                status: ConnectionStatus::Connected,
                credentials_invalidated,
            });
        };

        // `const cleanupResults = await Promise.allSettled([abortCleanup ?? client.close()]);`
        let mut cleanup_failures: Vec<McpError> = Vec::new();
        if let Err(cleanup) = resource.close().await {
            cleanup_failures.push(cleanup);
        }
        if cleanup_failures.is_empty() {
            return Err(error);
        }
        // `reportedError = new AggregateError([error, ...cleanupFailures], "MCP connection setup failed")`
        let mut children = Vec::with_capacity(cleanup_failures.len() + 1);
        children.push(error);
        children.append(&mut cleanup_failures);
        Err(McpError::SetupFailed(crate::errors::CleanupErrors::from(
            children,
        )))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod wire_tests {
    use super::*;

    fn entry() -> ServerEntry {
        ServerEntry::default()
    }

    // --- MCP-113 --------------------------------------------------------------------------------

    #[test]
    fn empty_string_counts_as_unconfigured() {
        let mut e = entry();
        e.command = Some(String::new());
        e.url = Some("http://x".to_string());
        assert_eq!(select_transport("s", &e).unwrap(), TransportKind::StreamableHttp);
    }

    #[test]
    fn two_transports_and_none_produce_the_exact_message() {
        let expected = "Server s must configure exactly one of command or url";

        let mut both = entry();
        both.command = Some("a".to_string());
        both.url = Some("b".to_string());
        assert_eq!(select_transport("s", &both).unwrap_err().to_string(), expected);

        assert_eq!(select_transport("s", &entry()).unwrap_err().to_string(), expected);
    }

    #[test]
    fn legacy_sse_is_refused_with_the_cut_one_diagnostic_and_opens_nothing() {
        let mut e = entry();
        e.url = Some("http://x".to_string());
        e.http_transport = Some(HttpTransport::Sse);
        let err = select_transport("s", &e).unwrap_err();
        assert_eq!(err.to_string(), sse_cut_diagnostic("s"));
        assert!(err.to_string().contains("legacy HTTP+SSE transport"));
    }

    #[test]
    fn streamable_http_is_the_accepted_value() {
        let mut e = entry();
        e.url = Some("http://x".to_string());
        e.http_transport = Some(HttpTransport::StreamableHttp);
        assert_eq!(select_transport("s", &e).unwrap(), TransportKind::StreamableHttp);
    }

    #[test]
    fn the_two_cut_diagnostics_name_their_cut_subject() {
        assert!(socket_cut_diagnostic("s").contains("configures `socket`"));
        assert!(socket_cut_diagnostic("s").contains("streamable HTTP (`url`)"));
    }

    // --- MCP-102 --------------------------------------------------------------------------------

    #[test]
    fn a_one_mib_burst_is_bounded_before_it_is_appended() {
        let burst = vec![b'x'; 1024 * 1024];
        assert_eq!(bounded_stderr_chunk(&burst).len(), MAX_CAPTURED_STDERR_BYTES);

        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, &burst);
        append_stderr_tail(&mut tail, &burst);
        assert_eq!(tail.len(), MAX_CAPTURED_STDERR_BYTES);
    }

    #[test]
    fn a_split_multibyte_sequence_becomes_a_replacement_char_not_a_panic() {
        // 8 KiB + 1 of a 3-byte sequence: the boundary lands mid-character.
        let mut burst = vec![b'a'; MAX_CAPTURED_STDERR_BYTES - 1];
        burst.extend_from_slice("€".as_bytes());
        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, &burst);
        assert_eq!(tail.len(), MAX_CAPTURED_STDERR_BYTES);
        // Whatever survives must still decode; `from_utf8_lossy` is what Node's toString does.
        let detail = stderr_tail_detail(&tail).unwrap();
        assert!(!detail.is_empty());
    }

    #[test]
    fn the_suffix_is_the_last_three_non_empty_lines_joined_by_an_em_dash() {
        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, b"zero\r\n\n  one  \r\ntwo\nthree\n\n");
        assert_eq!(stderr_tail_detail(&tail).as_deref(), Some("one \u{2014} two \u{2014} three"));
        assert_eq!(with_stderr_tail("boom", &tail), "boom (one \u{2014} two \u{2014} three)");
    }

    #[test]
    fn an_empty_or_whitespace_tail_adds_no_suffix() {
        let empty = VecDeque::new();
        assert_eq!(stderr_tail_detail(&empty), None);
        assert_eq!(with_stderr_tail("boom", &empty), "boom");

        let mut blank = VecDeque::new();
        append_stderr_tail(&mut blank, b"   \n\r\n  ");
        assert_eq!(stderr_tail_detail(&blank), None, "debug-mode parity: no tail, no `(...)`");
    }

    // --- MCP-117 --------------------------------------------------------------------------------

    #[test]
    fn absent_and_legacy_are_the_same_lifecycle() {
        let absent = version_negotiation(&entry()).expect("no revision pinned");
        let mut legacy = entry();
        legacy.protocol_version = Some(ProtocolVersionSetting::Legacy);
        assert!(matches!(absent, ClientLifecycleMode::Initialize));
        assert!(matches!(
            version_negotiation(&legacy).expect("legacy"),
            ClientLifecycleMode::Initialize
        ));
    }

    #[test]
    fn auto_and_pin_map_to_their_rmcp_modes() {
        let mut auto = entry();
        auto.protocol_version = Some(ProtocolVersionSetting::Auto);
        match version_negotiation(&auto).expect("auto") {
            ClientLifecycleMode::Auto { preferred_versions, legacy_version } => {
                assert_eq!(preferred_versions, vec![ProtocolVersion::V_2026_07_28]);
                assert_eq!(legacy_version, Some(ProtocolVersion::LATEST));
            }
            other => panic!("expected Auto, got {other:?}"),
        }

        let mut pinned = entry();
        pinned.protocol_version = Some(ProtocolVersionSetting::V20260728);
        match version_negotiation(&pinned).expect("pinned") {
            ClientLifecycleMode::Discover { preferred_versions } => {
                assert_eq!(preferred_versions, vec![ProtocolVersion::V_2026_07_28]);
            }
            other => panic!("expected Discover, got {other:?}"),
        }
    }

    #[test]
    fn the_invalid_protocol_version_string_is_byte_exact() {
        assert_eq!(
            invalid_protocol_version_message("2019-01-01"),
            "Invalid MCP protocolVersion: 2019-01-01"
        );
    }

    /// `resolveVersionNegotiation`'s `default:` arm, at the moment upstream reaches it.
    ///
    /// This could not be written before [`ProtocolVersionSetting::Other`] existed: the value was
    /// discarded by `deserialize_with = "lenient"` at parse time, so a server pinning a revision
    /// this build does not implement negotiated as `legacy` in silence — and, worse, hashed as if it
    /// had pinned nothing, which is the digest divergence
    /// `cyrup_ext_subagents::exec::mcp_direct_tools::tests::a_protocol_revision_the_writer_used_to_reject_now_agrees_on_the_digest`
    /// closes. The constraint the fix had to respect is exactly this test: the value must survive
    /// the deserialiser **and** still be refused at connect.
    ///
    /// Every expected sentence is `` `Invalid MCP protocolVersion: ${String(v)}` `` evaluated on
    /// node 22, not transcribed — including `[object Object]` for an object and `1,2` for an array,
    /// which are `String()`'s answers and not JSON.
    #[test]
    fn an_unknown_revision_throws_upstreams_sentence_at_connect() {
        for (json, message) in [
            (r#"{"command":"x","protocolVersion":"2025-06-18"}"#, "Invalid MCP protocolVersion: 2025-06-18"),
            (r#"{"command":"x","protocolVersion":5}"#, "Invalid MCP protocolVersion: 5"),
            (r#"{"command":"x","protocolVersion":1.5}"#, "Invalid MCP protocolVersion: 1.5"),
            (r#"{"command":"x","protocolVersion":true}"#, "Invalid MCP protocolVersion: true"),
            (r#"{"command":"x","protocolVersion":null}"#, "Invalid MCP protocolVersion: null"),
            (r#"{"command":"x","protocolVersion":[1,2]}"#, "Invalid MCP protocolVersion: 1,2"),
            (r#"{"command":"x","protocolVersion":{"a":1}}"#, "Invalid MCP protocolVersion: [object Object]"),
            (r#"{"command":"x","protocolVersion":""}"#, "Invalid MCP protocolVersion: "),
        ] {
            let entry: ServerEntry = serde_json::from_str(json).expect("parses — the loader is not the validator");
            assert!(
                matches!(entry.protocol_version, Some(ProtocolVersionSetting::Other(_))),
                "the deserialiser must not have dropped it: {json}"
            );
            assert_eq!(
                version_negotiation(&entry).expect_err(json).to_string(),
                message,
                "{json}"
            );
        }

        // The three known revisions are untouched by the passthrough arm.
        for (json, expected) in [
            (r#"{"command":"x","protocolVersion":"legacy"}"#, ProtocolVersionSetting::Legacy),
            (r#"{"command":"x","protocolVersion":"auto"}"#, ProtocolVersionSetting::Auto),
            (r#"{"command":"x","protocolVersion":"2026-07-28"}"#, ProtocolVersionSetting::V20260728),
        ] {
            let entry: ServerEntry = serde_json::from_str(json).expect("entry");
            assert_eq!(entry.protocol_version, Some(expected), "{json}");
            assert!(version_negotiation(&entry).is_ok(), "{json}");
        }
    }

    // --- MCP-118 --------------------------------------------------------------------------------

    #[test]
    fn capabilities_cover_all_four_on_off_combinations() {
        let none = build_client_capabilities(false, None);
        assert!(none.sampling.is_none() && none.elicitation.is_none());
        assert_eq!(serde_json::to_value(&none).unwrap(), serde_json::json!({}));

        let sampling_only = build_client_capabilities(true, None);
        assert_eq!(
            serde_json::to_value(&sampling_only).unwrap(),
            serde_json::json!({ "sampling": {} })
        );

        let form_only = build_client_capabilities(false, Some(ElicitationMode { allow_url: false }));
        assert_eq!(
            serde_json::to_value(&form_only).unwrap(),
            serde_json::json!({ "elicitation": { "form": {} } })
        );

        let both = build_client_capabilities(true, Some(ElicitationMode { allow_url: true }));
        assert_eq!(
            serde_json::to_value(&both).unwrap(),
            serde_json::json!({ "sampling": {}, "elicitation": { "form": {}, "url": {} } })
        );
    }

    #[test]
    fn the_client_name_is_the_recorded_rename() {
        let info = client_info("github", ClientCapabilities::default());
        assert_eq!(info.client_info.name, "cyrup-mcp-github");
        assert_eq!(info.client_info.version, "1.0.0");
    }

    // --- MCP-120 / MCP-122 ----------------------------------------------------------------------

    #[test]
    fn the_three_reason_strings_are_byte_exact() {
        assert_eq!(ListKind::Tools.reason(), "tools-list-changed");
        assert_eq!(ListKind::Prompts.reason(), "prompts-list-changed");
        assert_eq!(ListKind::Resources.reason(), "resources-list-changed");
        assert_eq!(
            ListKind::Prompts.refresh_failed_message("s", "boom"),
            "MCP: prompts/list_changed refresh failed for s: boom"
        );
    }

    fn handler(allow_url: bool, hook: Option<ElicitationCompleteHook>) -> McpClientHandler {
        McpClientHandler::new(McpClientHandlerParts {
            server: "s".to_string(),
            runtime_signal: CancelToken::new(),
            elicitation_mode: Some(ElicitationMode { allow_url }),
            sampling: None,
            elicitation: None,
            list_changed: None,
            elicitation_complete: hook,
        })
    }

    #[test]
    fn two_handlers_never_share_an_identity_and_a_clone_always_does() {
        let a = handler(false, None);
        let b = handler(false, None);
        assert!(a.identity().ptr_eq(&a.clone().identity()));
        assert!(!a.identity().ptr_eq(&b.identity()));
    }

    #[test]
    fn capabilities_follow_the_wired_hooks_not_the_configuration() {
        // No elicitation hook wired => the capability is not advertised even though a mode was set.
        let h = handler(true, None);
        assert!(h.info().capabilities.elicitation.is_none());
        assert!(h.info().capabilities.sampling.is_none());
    }

    // --- MCP-101 --------------------------------------------------------------------------------

    #[tokio::test]
    async fn stdio_spawn_creates_the_plugin_dir_pipes_stderr_and_replaces_the_environment() {
        use tokio::io::AsyncReadExt;

        let dir = tempfile::tempdir().unwrap();
        let plugin_data = dir.path().join("nested/plugin-data");
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let spec = StdioTransportSpec {
            command: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                // Two lines on stderr, then exit: the value the override supplied, and whether a
                // host variable leaked through. `resolveEnv` builds the child's WHOLE environment,
                // so `HOME` — which this test process has and which a POSIX shell does not invent
                // for itself the way it invents `PATH` — must not be visible unless the spawn
                // merged instead of replacing.
                "printf '%s\\n%s\\n' \"$FOO\" \"${HOME:-unset}\" 1>&2".to_string(),
            ],
            env,
            cwd: Some(dir.path().to_path_buf()),
            plugin_data_dir: Some(plugin_data.clone()),
            debug: false,
        };

        let (transport, stderr) = spawn_stdio_transport(&spec).unwrap();
        assert!(plugin_data.is_dir(), "pluginDataDir is mkdir -p'd before the spawn");

        let mut stderr = stderr.expect("debug: false pipes stderr and hands back the handle");
        let mut captured = Vec::new();
        let _ = stderr.read_to_end(&mut captured).await.unwrap();

        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, &captured);
        assert_eq!(stderr_tail_detail(&tail).as_deref(), Some("bar \u{2014} unset"));
        assert_eq!(
            with_stderr_tail("MCP connection setup failed", &tail),
            "MCP connection setup failed (bar \u{2014} unset)"
        );

        drop(transport);
    }

    #[tokio::test]
    async fn debug_mode_inherits_stderr_so_there_is_no_tail_to_build_from() {
        let spec = StdioTransportSpec {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            env: HashMap::new(),
            cwd: None,
            plugin_data_dir: None,
            debug: true,
        };
        let (transport, stderr) = spawn_stdio_transport(&spec).unwrap();
        assert!(stderr.is_none(), "debug: true is rmcp's default Stdio::inherit()");
        drop(transport);
    }

    #[tokio::test]
    async fn a_missing_command_reports_the_path_it_could_not_spawn() {
        let spec = StdioTransportSpec {
            command: "/nonexistent/cyrup-mcp-fixture".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            plugin_data_dir: None,
            debug: false,
        };
        // `TokioChildProcess` is not `Debug`, so `unwrap_err` is unavailable here.
        let Err(err) = spawn_stdio_transport(&spec) else {
            panic!("spawning a nonexistent command must fail");
        };
        assert!(
            err.to_string().contains("/nonexistent/cyrup-mcp-fixture"),
            "the failing path is what makes a mistyped `command` actionable: {err}"
        );
    }

    // --- MCP-109 --------------------------------------------------------------------------------

    #[test]
    fn http_config_pins_the_uri_disables_reinit_and_routes_the_bearer_through_auth_header() {
        let spec = HttpTransportSpec {
            server: "s".to_string(),
            url: "https://example.test/mcp".to_string(),
            headers: vec![("X-A".to_string(), "1".to_string())],
            bearer_token: Some("hunter2".to_string()),
        };
        let config = build_http_transport_config(&spec).unwrap();
        assert_eq!(&*config.uri, "https://example.test/mcp");
        assert!(
            !config.reinit_on_expired_session,
            "rmcp defaults this ON; MCP-135 owns session recovery, not the transport"
        );
        assert!(config.allow_stateless, "rmcp's default, and the upstream-equivalent value");
        assert_eq!(
            config.auth_header.as_deref(),
            Some("hunter2"),
            "the token goes in WITHOUT the `Bearer ` prefix"
        );
        assert_eq!(config.custom_headers.len(), 1);
    }

    #[test]
    fn a_header_value_carrying_a_newline_cannot_inject_a_second_header() {
        let spec = HttpTransportSpec {
            server: "s".to_string(),
            url: "https://example.test/mcp".to_string(),
            headers: vec![("X-A".to_string(), "a\r\nX-Evil: 1".to_string())],
            bearer_token: None,
        };
        let err = build_http_transport_config(&spec).unwrap_err();
        assert!(err.to_string().contains("X-A"), "the offending header is named: {err}");
    }

    // --- MCP-083, the two deferred call sites -----------------------------------------------------

    #[test]
    fn the_stdio_spec_resolves_its_env_through_secrets_and_reports_the_failing_key() {
        let base: HashMap<String, String> =
            [("HOST_ONLY".to_string(), "kept".to_string())].into_iter().collect();

        let mut e = entry();
        e.command = Some("/bin/true".to_string());
        e.env = Some(
            [
                ("TOKEN".to_string(), "!printf hunter2".to_string()),
                ("PLAIN".to_string(), "x".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        e.plugin_data_dir = Some("/tmp/cyrup-mcp-plugin-data".to_string());

        let spec = StdioTransportSpec::resolve(
            &e,
            "srv",
            "/bin/true".to_string(),
            vec!["--x".to_string()],
            None,
            &base,
        )
        .unwrap();
        assert_eq!(spec.env.get("TOKEN").map(String::as_str), Some("hunter2"));
        assert_eq!(spec.env.get("PLAIN").map(String::as_str), Some("x"));
        assert_eq!(
            spec.env.get("HOST_ONLY").map(String::as_str),
            Some("kept"),
            "`resolveEnv` copies the WHOLE base environment before layering"
        );
        assert_eq!(spec.plugin_data_dir.as_deref(), Some(std::path::Path::new("/tmp/cyrup-mcp-plugin-data")));
        assert!(!spec.debug, "`debug` absent is `false`, i.e. stderr piped");

        // A failing secret is an error carrying the `stdio env` context string — never an empty
        // environment variable the child would silently authenticate with.
        e.env = Some([("TOKEN".to_string(), "!exit 9".to_string())].into_iter().collect());
        // `StdioTransportSpec` is not `Debug` (it carries a resolved environment), so the failure is
        // destructured rather than `unwrap_err`'d — the same shape the spawn tests above use.
        let Err(err) = StdioTransportSpec::resolve(
            &e,
            "srv",
            "/bin/true".to_string(),
            Vec::new(),
            None,
            &base,
        ) else {
            panic!("an unresolvable stdio env secret must never become a value");
        };
        assert_eq!(
            err.to_string(),
            "Failed to resolve MCP server \"srv\" stdio env \"TOKEN\": command exited with code 9"
        );
    }

    #[test]
    fn the_http_spec_resolves_headers_and_the_bearer_through_secrets() {
        use crate::config::{AuthKind, AuthMode};

        let env = crate::credentials::process_env();
        let mut e = entry();
        e.url = Some("https://example.test/mcp".to_string());
        e.headers = Some(
            [("X-Token".to_string(), "!printf abc".to_string())]
                .into_iter()
                .collect(),
        );
        e.auth = Some(AuthMode::Named(AuthKind::Bearer));
        e.bearer_token = Some("!printf hunter2".to_string());

        let spec = HttpTransportSpec::resolve(
            &e,
            "srv",
            "https://example.test/mcp".to_string(),
            &env,
        )
        .unwrap();
        assert_eq!(spec.server, "srv");
        assert_eq!(
            spec.headers,
            vec![("X-Token".to_string(), "abc".to_string())],
            "the header command ran and its stdout is the value"
        );
        assert_eq!(
            spec.bearer_token.as_deref(),
            Some("hunter2"),
            "WITHOUT the `Bearer ` prefix, which `auth_header` adds"
        );

        // And the resolved spec still passes the transport builder unchanged.
        let config = build_http_transport_config(&spec).unwrap();
        assert_eq!(config.auth_header.as_deref(), Some("hunter2"));
        assert_eq!(config.custom_headers.len(), 1);
    }

    // --- MCP-128 --------------------------------------------------------------------------------

    #[test]
    fn an_invalid_per_server_timeout_means_no_timeout_even_when_a_global_is_set() {
        let with = |ms: Option<f64>| {
            let mut e = entry();
            e.request_timeout_ms = ms;
            e
        };

        for bad in [0.0_f64, -1.0, f64::NAN, f64::INFINITY] {
            let e = with(Some(bad));
            assert_eq!(resolve_request_timeout(Some(&e), None), None);
            assert_eq!(
                resolve_request_timeout(Some(&e), Some(30_000.0)),
                None,
                "an invalid per-server value must NOT reinstate the global ({bad})"
            );
        }

        let pinned = with(Some(5_000.0));
        assert_eq!(
            resolve_request_timeout(Some(&pinned), Some(30_000.0)),
            Some(Duration::from_millis(5_000))
        );

        let absent = with(None);
        assert_eq!(
            resolve_request_timeout(Some(&absent), Some(30_000.0)),
            Some(Duration::from_millis(30_000)),
            "the global is consulted only when the per-server key is absent"
        );
        assert_eq!(resolve_request_timeout(Some(&absent), None), None);
        assert!(build_request_options(Some(&absent), None).is_none());
        assert_eq!(
            build_request_options(Some(&pinned), None).and_then(|o| o.timeout),
            Some(Duration::from_millis(5_000))
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod connection_builder_tests {
    use super::*;
    use std::collections::HashMap;

    // ── fixtures ──────────────────────────────────────────────────────────────────────────────
    //
    // `sh -c` everywhere, which is the same fixture runtime `server_manager::tests::child_process`
    // already uses and the same one `secrets::resolve_command_secret` spawns in production — so
    // nothing here is a new host dependency.

    /// A stdio "server" that writes one environment variable to **stderr** and exits non-zero.
    ///
    /// It fails the handshake on purpose: that is the path §3.3 step 8 decorates, so one fixture
    /// asserts both what the child's environment actually was and that the tail reaches the user.
    const ENV_TO_STDERR: &str = "printf '%s' \"$K\" >&2; exit 3";

    /// A minimal MCP server: answers `initialize` and echoes its cwd and its arguments back in
    /// `instructions`, which is where the test reads them from. Deliberately free of `${`, `$env:`
    /// and `{env:` so that passing it through `interpolateEnvVars` (which `args` go through) cannot
    /// rewrite the script itself.
    const TINY_MCP: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{},"serverInfo":{"name":"fixture","version":"1"},"instructions":"%s|%s"}}\n' "$id" "$PV" "$(pwd)" "$*"
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

    fn env_fn(pairs: &[(&str, &str)]) -> crate::credentials::EnvFn {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        Arc::new(move |key: &str| map.get(key).cloned())
    }

    /// A `base_env` with only what `sh` needs, so the "replace, don't merge" semantics of
    /// `StdioClientTransport`'s `env` option are observable: anything the child sees beyond `PATH`
    /// came from `definition.env`.
    fn base_env() -> HashMap<String, String> {
        [("PATH".to_string(), std::env::var("PATH").unwrap_or_default())]
            .into_iter()
            .collect()
    }

    fn builder() -> ConnectionBuilder {
        ConnectionBuilder::new(None).with_environment(
            env_fn(&[("HOME", "/home/fixture"), ("TOKEN", "s3cret")]),
            base_env(),
            PathBuf::from("/home/fixture"),
        )
    }

    fn request(name: &str, entry: ServerEntry) -> CreateConnection {
        CreateConnection {
            name: name.to_string(),
            definition: Arc::new(entry),
            attempt: CancelToken::new(),
            request: CancelToken::new(),
            credentials_invalidated: false,
            request_options: None,
        }
    }

    /// [`request`] with the manager's own `buildRequestOptions(definition, requestSignal)` applied,
    /// so a test exercises the same `request_options` a real `connect` would hand the factory.
    fn timed_request(name: &str, entry: ServerEntry) -> CreateConnection {
        let request_options = build_request_options(Some(&entry), None);
        CreateConnection {
            request_options,
            ..request(name, entry)
        }
    }

    fn stdio_entry(script: &str, args: &[&str]) -> ServerEntry {
        let mut entry = ServerEntry {
            command: Some("sh".to_string()),
            ..ServerEntry::default()
        };
        let mut argv = vec!["-c".to_string(), script.to_string(), "sh".to_string()];
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        entry.args = Some(argv);
        entry
    }

    fn record(pairs: &[(&str, &str)]) -> crate::config::StringRecord {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<std::collections::BTreeMap<String, String>>()
            .into()
    }

    // ── MCP-101 · §3.3 ────────────────────────────────────────────────────────────────────────

    /// `resolveEnv(definition.env, name, definition.literalEnv === true)`'s three arms, observed
    /// from **inside the child** rather than from the resolver's return value: the environment the
    /// process actually got is the thing that matters, and it is what a `.envs()`-instead-of-
    /// `.env_clear()` port would get wrong.
    ///
    /// Each case also exercises §3.3 step 8, since the child writes to stderr and dies: the
    /// connect failure carries `(<value>)`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_child_environment_is_resolved_exactly_as_resolve_env_specifies() {
        // `literalEnv: true` — verbatim, no interpolation, no command execution.
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "${HOME}")]));
        entry.literal_env = Some(true);
        let error = builder()
            .connect_stdio(&request("literal", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with("(${HOME})"),
            "literalEnv must not interpolate: {error}"
        );

        // Without `literalEnv`, `${HOME}` interpolates.
        //
        // NAMED RESIDUAL, and the reason this arm reads the *real* `HOME` while the `args` test
        // below reads the seam's: `secrets::resolve_command_secret` — which
        // `resolveEnv`'s non-literal arm goes through — takes no `EnvFn`; it resolves against
        // `secrets::PROCESS_ENV`. `ConnectionBuilder::with_environment`'s `env` therefore reaches
        // `args`, `cwd`, the URL and the bearer ladder but NOT `env` values. In production both are
        // `process.env` and there is no divergence; in a test the two seams are simply different,
        // and giving `resolve_command_secret` an `EnvFn` is a `secrets.rs` change this unit does
        // not own.
        let real_home = std::env::var("HOME").unwrap_or_default();
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "${HOME}")]));
        let error = builder()
            .connect_stdio(&request("interp", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with(&format!("({real_home})")),
            "expected the interpolated home {real_home}: {error}"
        );

        // `!cmd` runs a shell command and the child sees its trimmed stdout.
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "!echo hunter2")]));
        let error = builder()
            .connect_stdio(&request("command", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with("(hunter2)"),
            "expected the command's stdout: {error}"
        );

        // `!!X` consumes exactly ONE `!` and interpolates the rest — so the child sees `!` followed
        // by the expanded home, and nothing is executed.
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "!!${HOME}")]));
        let error = builder()
            .connect_stdio(&request("escaped", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with(&format!("(!{real_home})")),
            "expected one `!` consumed and the rest interpolated: {error}"
        );
    }

    /// The five `!command` failure strings are user-visible verbatim, and the failure happens
    /// **before** a child exists — `resolveEnv` runs inside `new StdioClientTransport({...})`, which
    /// is upstream's step 7.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_env_command_surfaces_upstreams_sentence_and_spawns_nothing() {
        let mut entry = stdio_entry("exit 0", &[]);
        entry.env = Some(record(&[("K", "!exit 1")]));
        let error = builder()
            .connect_stdio(&request("x", entry))
            .await
            .expect_err("a non-zero secret command fails the connect");
        assert_eq!(
            error.to_string(),
            "Failed to resolve MCP server \"x\" stdio env \"K\": command exited with code 1"
        );
    }

    /// `args = (definition.args ?? []).map(interpolateEnvVars)` and
    /// `cwd = resolveConfigPath(definition.cwd) ?? this.defaultCwd`, read back out of the live
    /// server's `instructions`. This is the only test in the file that completes a real MCP
    /// handshake, so it is also the proof that [`ConnectionBuilder::connect_stdio`] produces a
    /// usable connection rather than merely a spawned process.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn args_are_interpolated_and_cwd_follows_resolve_config_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut entry = stdio_entry(TINY_MCP, &["${TOKEN}", "$env:TOKEN", "{env:TOKEN}", "${NOPE}"]);
        entry.env = Some(record(&[(
            "PV",
            rmcp::model::ProtocolVersion::LATEST.as_str(),
        )]));
        entry.cwd = Some(temp.path().to_string_lossy().into_owned());

        let connection = builder()
            .connect_stdio(&request("live", entry))
            .await
            .expect("the fixture speaks MCP");
        let info = connection
            .peer()
            .peer_info()
            .expect("initialize completed, so peer info exists");
        let instructions = info
            .instructions
            .clone()
            .expect("the fixture puts cwd|args in instructions");
        let (cwd, args) = instructions.split_once('|').expect("cwd|args");

        // `temp.path()` can be a symlink on macOS (`/var` → `/private/var`); compare canonically.
        assert_eq!(
            std::fs::canonicalize(cwd).expect("cwd exists"),
            std::fs::canonicalize(temp.path()).expect("tempdir exists"),
        );
        // All THREE placeholder forms expand, and a missing variable becomes the empty string.
        assert_eq!(args, "s3cret s3cret s3cret ");

        connection.close().await.expect("close");
    }

    /// `debug: true` ⇒ stderr is **inherited**, so there is no tail and therefore no `(...)` suffix.
    /// The pair with the `debug: false` cases above is what pins the polarity — rmcp's default is
    /// `inherit`, the opposite of what a reader of `stderr: definition.debug ? "inherit" : "pipe"`
    /// expects to have to write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn debug_mode_inherits_stderr_and_produces_no_suffix() {
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "diagnostic")]));
        entry.debug = Some(true);
        let error = builder()
            .connect_stdio(&request("noisy", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            !error.to_string().contains("(diagnostic)"),
            "debug mode has no tail to append: {error}"
        );
    }

    /// `if (definition.pluginDataDir) mkdirSync(definition.pluginDataDir, { recursive: true })` —
    /// §3.3 step 5, and it happens **before** the spawn so a plugin's first write cannot race its
    /// own server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_plugin_data_directory_exists_before_the_child_does() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("plugin/data/dir");
        let mut entry = stdio_entry("test -d \"$D\" && printf ready >&2; exit 3", &[]);
        entry.plugin_data_dir = Some(nested.to_string_lossy().into_owned());
        entry.env = Some(record(&[("D", nested.to_string_lossy().into_owned().as_str())]));

        let error = builder()
            .connect_stdio(&request("plugin", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(nested.is_dir(), "the directory must exist after the connect");
        assert!(
            error.to_string().ends_with("(ready)"),
            "the CHILD must have seen it, not just the test: {error}"
        );
    }

    /// `createClient` runs `resolveVersionNegotiation` **first**, so an invalid `protocolVersion`
    /// throws before a process exists. A port that spawned first would leak one child per
    /// misconfigured server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_invalid_protocol_version_throws_before_anything_is_spawned() {
        let mut entry = stdio_entry("printf spawned >&2; exit 3", &[]);
        entry.protocol_version = Some(crate::config::ProtocolVersionSetting::Other(
            serde_json::from_str::<crate::config::RawJson>("\"2025-06-18\"").expect("raw"),
        ));
        let error = builder()
            .connect_stdio(&request("pinned", entry))
            .await
            .expect_err("an unknown revision fails the connect");
        assert_eq!(error.to_string(), "Invalid MCP protocolVersion: 2025-06-18");
        // No child ran, so no stderr tail was appended.
        assert!(!error.to_string().contains("spawned"));
    }

    // ── an HTTP fixture ───────────────────────────────────────────────────────────────────────
    //
    // A hand-rolled HTTP/1.1 responder rather than a framework: the assertions are about exact
    // bytes on the wire (which `Authorization` header arrived, how many attempts were made), and a
    // framework would normalise precisely the things under test. Every response carries
    // `Connection: close`, so one request is one accepted socket and the request log is ordered.

    /// One request as the server saw it.
    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Recorded {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        fn all(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
                .collect()
        }
    }

    /// How many `initialize` POSTs to answer with 401 before answering with a real handshake.
    /// `usize::MAX` is the permanent-401 server.
    struct HttpFixture {
        url: String,
        log: Arc<std::sync::Mutex<Vec<Recorded>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    /// What the fixture does, beyond the 401 count. Every field defaults to the shape the original
    /// fixture hardcoded, so an existing call site reads the same.
    #[derive(Clone)]
    struct FixtureOptions {
        /// How many `initialize` POSTs to answer with 401 before answering with a real handshake.
        unauthorized_initializes: usize,
        /// Emit `WWW-Authenticate` on those 401s. **`false` is the bare 401** — the shape rmcp
        /// never turns into an `AuthRequiredError`, and the one no test covered before.
        challenge: bool,
        /// Emit `mcp-session-id` on the handshake response. `false` is a legal stateless
        /// streamable-HTTP server, and the case `has_session_id` used to answer `true` for.
        session_id: bool,
        /// Read the `initialize` POST and then never answer it, holding the socket open. The
        /// wedged-server case `requestTimeoutMs` exists for.
        stall_initialize: bool,
        cancel_before: Option<(usize, CancelToken)>,
    }

    impl Default for FixtureOptions {
        fn default() -> Self {
            Self {
                unauthorized_initializes: 0,
                challenge: true,
                session_id: true,
                stall_initialize: false,
                cancel_before: None,
            }
        }
    }

    impl HttpFixture {
        async fn start(unauthorized_initializes: usize) -> Self {
            Self::start_with(FixtureOptions {
                unauthorized_initializes,
                ..FixtureOptions::default()
            })
            .await
        }

        /// As [`Self::start`], but the fixture cancels `token` **before writing** the response to
        /// the `nth` request (1-based). Writing after the cancel would be a race — the client could
        /// finish reading the response first — and the whole point of the arm-3 test is that the
        /// abort is already visible when the attempt settles.
        async fn start_cancelling_before_response(
            unauthorized_initializes: usize,
            nth: usize,
            token: CancelToken,
        ) -> Self {
            Self::start_with(FixtureOptions {
                unauthorized_initializes,
                cancel_before: Some((nth, token)),
                ..FixtureOptions::default()
            })
            .await
        }

        async fn start_with(options: FixtureOptions) -> Self {
            let FixtureOptions {
                unauthorized_initializes,
                challenge,
                session_id,
                stall_initialize,
                cancel_before,
            } = options;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback");
            let addr = listener.local_addr().expect("addr");
            let log: Arc<std::sync::Mutex<Vec<Recorded>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink = Arc::clone(&log);
            let task = tokio::spawn(async move {
                let mut initializes = 0_usize;
                let mut seen = 0_usize;
                // Sockets the fixture deliberately never answers. Held rather than dropped: a drop
                // would close the connection and the client would see EOF, which is a *different*
                // failure from a server that stays connected and stays silent.
                let mut held: Vec<tokio::net::TcpStream> = Vec::new();
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    let Some(recorded) = read_request(&mut socket).await else {
                        continue;
                    };
                    let is_initialize = recorded.body.contains("\"method\":\"initialize\"");
                    if is_initialize {
                        initializes += 1;
                    }
                    seen += 1;
                    if let Some((nth, token)) = cancel_before.as_ref()
                        && seen == *nth
                    {
                        token.cancel();
                    }
                    sink.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(recorded.clone());

                    let response = if recorded.method == "GET" || recorded.method == "DELETE" {
                        // No server-initiated stream and no session teardown: both are optional.
                        "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    } else if is_initialize && stall_initialize {
                        // Read, recorded, never answered — and the socket is held open by keeping
                        // it in scope. This is the wedged server: the TCP connect succeeded, so
                        // nothing below the MCP layer fails, and only `requestTimeoutMs` can end it.
                        held.push(socket);
                        continue;
                    } else if is_initialize && initializes <= unauthorized_initializes {
                        let challenge_header = if challenge {
                            "WWW-Authenticate: Bearer realm=\"mcp\", resource_metadata=\"https://example.invalid/.well-known\"\r\n"
                        } else {
                            // The BARE 401. rmcp builds `AuthRequiredError` only when the header is
                            // present, so this is the shape that used to fall out as a hard connect
                            // error instead of reaching the OAuth ladder.
                            ""
                        };
                        format!("HTTP/1.1 401 Unauthorized\r\n{challenge_header}Content-Length: 0\r\nConnection: close\r\n\r\n")
                    } else if is_initialize {
                        let id = json_id(&recorded.body);
                        let payload = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"{}\",\"capabilities\":{{}},\"serverInfo\":{{\"name\":\"fixture\",\"version\":\"1\"}}}}}}",
                            rmcp::model::ProtocolVersion::LATEST.as_str()
                        );
                        let session_header = if session_id {
                            "mcp-session-id: fixture-session\r\n"
                        } else {
                            // A stateless streamable-HTTP server. Legal, and rmcp's
                            // `allow_stateless` default accepts it.
                            ""
                        };
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{session_header}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        )
                    } else if recorded.body.contains("\"id\":") {
                        let id = json_id(&recorded.body);
                        let payload = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{}}}}");
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        )
                    } else {
                        // A notification. 202 with no body is what the spec asks for.
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    };
                    use tokio::io::AsyncWriteExt as _;
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            });
            Self {
                url: format!("http://{addr}/mcp"),
                log,
                _task: task,
            }
        }

        fn requests(&self) -> Vec<Recorded> {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn initializes(&self) -> usize {
            self.requests()
                .iter()
                .filter(|request| request.body.contains("\"method\":\"initialize\""))
                .count()
        }
    }

    fn json_id(body: &str) -> String {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("id").cloned())
            .map_or_else(|| "0".to_string(), |id| id.to_string())
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
        use tokio::io::AsyncReadExt as _;
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 4096];
        let head_end = loop {
            if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let read = socket.read(&mut buffer).await.ok()?;
            if read == 0 {
                return None;
            }
            raw.extend_from_slice(buffer.get(..read)?);
        };
        let head = String::from_utf8_lossy(raw.get(..head_end)?).into_owned();
        let mut lines = head.lines();
        let method = lines
            .next()?
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        let length: usize = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse().ok())
            .unwrap_or(0);
        while raw.len() < head_end + length {
            let read = socket.read(&mut buffer).await.ok()?;
            if read == 0 {
                break;
            }
            raw.extend_from_slice(buffer.get(..read)?);
        }
        let body = String::from_utf8_lossy(raw.get(head_end..)?).into_owned();
        Some(Recorded {
            method,
            headers,
            body,
        })
    }

    /// A [`HttpAuthProvider`] that counts what the ladder asked it for. The count IS the assertion
    /// for "no keychain read happened before the first 401".
    #[derive(Debug, Default)]
    struct CountingAuth {
        token: Option<String>,
        calls: std::sync::Mutex<Vec<Option<String>>>,
        invalidations: std::sync::Mutex<Vec<String>>,
    }

    impl CountingAuth {
        fn with_token(token: &str) -> Arc<Self> {
            Arc::new(Self {
                token: Some(token.to_string()),
                ..Self::default()
            })
        }

        fn empty() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn calls(&self) -> Vec<Option<String>> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn invalidations(&self) -> Vec<String> {
            self.invalidations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl HttpAuthProvider for CountingAuth {
        fn authorize<'a>(
            &'a self,
            _server: &'a str,
            _url: &'a str,
            challenge: Option<&'a str>,
        ) -> BoxFuture<'a, McpResult<Option<String>>> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(challenge.map(str::to_string));
            let token = self.token.clone();
            Box::pin(async move { Ok(token) })
        }

        fn invalidate_auth_entry_cache(&self, server: &str) {
            self.invalidations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(server.to_string());
        }
    }

    fn http_entry(url: &str) -> ServerEntry {
        ServerEntry {
            url: Some(url.to_string()),
            ..ServerEntry::default()
        }
    }

    // ── MCP-109 · the transport is really constructed ─────────────────────────────────────────

    /// The streamable-HTTP transport connects, carries `Mcp-Session-Id` forward, and the connection
    /// reports having one — which is `session-recovery.ts`'s `hadSessionId` gate (MCP-134).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_streamable_http_server_connects_and_the_session_id_is_visible() {
        let fixture = HttpFixture::start(0).await;
        let connection = builder()
            .connect_http_client(&request("http", http_entry(&fixture.url)))
            .await
            .expect("the fixture completes the handshake");
        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert!(
            connection.resource.has_session_id(),
            "streamable HTTP is the transport that has a session id"
        );
        assert_eq!(fixture.initializes(), 1, "one attempt, one initialize");
        connection.resource.close().await.expect("close");
    }

    /// MCP-134's other answer: a **stateless** streamable-HTTP server sends no `Mcp-Session-Id`, and
    /// `has_session_id` must be `false` for it.
    ///
    /// This is the ablation that the previous test could not perform. `has_session_id` was a
    /// hardcoded `true`, and the fixture always emitted `mcp-session-id`, so the test above passed
    /// identically with the field read from the wire and with it read from nothing. Consumed in
    /// production at `lifecycle.rs:1090` → `should_reconnect_after_refresh`, whose upstream first
    /// line is `if (!hadSessionId) return false` — so with the constant, a plain 404 from a
    /// stateless server was classified as a terminated session on every health tick and drove a
    /// reconnect + rediscovery cycle upstream never performs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stateless_http_server_reports_no_session_id() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            session_id: false,
            ..FixtureOptions::default()
        })
        .await;
        let connection = builder()
            .connect_http_client(&request("stateless", http_entry(&fixture.url)))
            .await
            .expect("a stateless streamable-HTTP server is a legal server and connects");
        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert!(
            !connection.resource.has_session_id(),
            "no `Mcp-Session-Id` on the wire means no session id, and `hadSessionId` closes the \
             session-recovery gate"
        );
        connection.resource.close().await.expect("close");
    }

    /// A **bare** 401 — 401 with no `WWW-Authenticate` — reaches the OAuth ladder.
    ///
    /// rmcp only builds `AuthRequiredError` when the challenge header is present, so this shape used
    /// to leave `unauthorized_challenge` answering `None` and the connect failing at arm 7 with
    /// `unexpected server response: HTTP 401 Unauthorized` — the user never offered `/mcp-auth`.
    /// Upstream's `isUnauthorizedHttpError` is status-only and reaches `needs-auth` here.
    /// `HttpFixture` always emitted the header before this test existed, which is why nothing caught
    /// it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            unauthorized_initializes: usize::MAX,
            challenge: false,
            ..FixtureOptions::default()
        })
        .await;
        let auth = CountingAuth::empty();
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("bare", http_entry(&fixture.url)))
            .await
            .expect("a bare 401 is `needs-auth`, not a connect failure");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert_eq!(fixture.initializes(), 2, "implicit-deferred promotes and retries once");
        assert_eq!(auth.invalidations(), vec!["bare".to_string()]);
        // The retry carried an EMPTY challenge, because there genuinely was none — that is what
        // `Some("")` means, and the arm the old doc claimed existed but nothing could produce.
        assert_eq!(
            auth.calls(),
            vec![Some(String::new())],
            "the promoted attempt asks the provider with an empty challenge"
        );
    }

    /// A 403 must NOT be widened along with the bare 401. `InsufficientScopeError` is rmcp's
    /// 403-with-challenge and upstream's predicate is 401-only, so a scope denial stays a hard
    /// error. Pinned here because the bare-401 widening is the one change that could have taken
    /// 403 with it.
    #[test]
    fn the_401_predicate_still_refuses_every_other_status() {
        for status in ["HTTP 403 Forbidden: nope", "HTTP 500 Internal Server Error: ", "HTTP 404 Not Found: "] {
            let error: rmcp::transport::streamable_http_client::StreamableHttpError<
                reqwest::Error,
            > = rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
                std::borrow::Cow::Owned(status.to_string()),
            );
            assert!(!bare_unauthorized(&error), "{status} is not a 401");
        }
        let unauthorized: rmcp::transport::streamable_http_client::StreamableHttpError<
            reqwest::Error,
        > = rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
            std::borrow::Cow::Borrowed("HTTP 401 Unauthorized: "),
        );
        assert!(bare_unauthorized(&unauthorized));
    }

    /// An OAuth token and a **configured** `Authorization` header must not both go on the wire.
    ///
    /// rmcp carries the bearer in a separate `auth_header` channel from the custom-header map and
    /// both channels APPEND, so parity with upstream's single `Headers` object has to be restored at
    /// every producer. MEASURED before the fix as `["Bearer from-store", "Static abc123"]` — the
    /// credential the config did not name was additionally disclosed to the server. Upstream's
    /// `_commonHeaders` spreads `requestInit.headers` over the provider's header, so the configured
    /// one wins.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_oauth_token_never_joins_a_configured_authorization_header() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth));
        entry.headers = Some(record(&[("Authorization", "Static abc123")]));
        let auth = CountingAuth::with_token("from-store");

        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("both", entry))
            .await
            .expect("connects");
        connection.resource.close().await.expect("close");

        let first = fixture.requests();
        let first = first.first().expect("one request");
        assert_eq!(
            first.all("authorization"),
            vec!["Static abc123"],
            "exactly one value, and the configured header wins"
        );
        assert!(
            !auth.calls().is_empty(),
            "the OAuth store is still consulted — `supportsOAuth` is true even with custom headers"
        );
    }

    /// The stdio pre-flight's `!command` env resolution must not hold the async worker that carries
    /// the connect future.
    ///
    /// `resolve_command_secret` is a `std::process::Command` spawn polled with
    /// `std::thread::sleep`, bounded by a 10-second timeout and cancellable by nothing. Run inline
    /// it parks a tokio worker for its whole duration **inside the manager's single-flight connect
    /// future**, during which `close`/`close_all`'s abort cannot preempt the attempt.
    ///
    /// Measured on a ONE-worker runtime, which is what makes the assertion mean something: the
    /// 200 ms timer can only fire on time if the worker is free while the 1-second command runs.
    /// Inline, the timer fires late — at roughly the command's own duration.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_slow_env_command_does_not_hold_the_worker_carrying_the_connect() {
        let mut entry = stdio_entry("exec cat", &[]);
        entry.env = Some(record(&[("SLOW", "!sleep 1; echo value")]));
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_millis(200),
            builder().connect_stdio(&request("slow-env", entry)),
        )
        .await;
        assert!(outcome.is_err(), "the connect is still in flight at 200 ms");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(700),
            "the timer fired {elapsed:?} in, so the worker was blocked by the env command rather \
             than free to run the timer"
        );
    }

    /// `requestTimeoutMs` bounds the **handshake**, on the stdio arm.
    ///
    /// The child accepts its pipes and never answers `initialize`. Before
    /// [`connect_client_bounded`], `CreateConnection::request_options` was built by the manager and
    /// read by nobody, so this connect — and with it the manager's per-name single-flight slot —
    /// parked until someone called `close`. MEASURED as still hanging after 6 s with
    /// `requestTimeoutMs: 300`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wedged_stdio_server_fails_its_handshake_at_request_timeout_ms() {
        let mut entry = stdio_entry("exec sleep 60", &[]);
        entry.request_timeout_ms = Some(300.0);
        let started = std::time::Instant::now();
        let error = builder()
            .connect_stdio(&timed_request("wedged", entry))
            .await
            .expect_err("a server that never answers `initialize` must not hang the connect");
        assert!(
            error.to_string().contains(HANDSHAKE_TIMED_OUT),
            "expected `{HANDSHAKE_TIMED_OUT}`, got {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the budget, not the test harness, is what ended this"
        );
    }

    /// The same bound on the HTTP arm: the fixture reads `initialize` and holds the socket open
    /// without answering, so nothing below the MCP layer fails and only the budget can end it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wedged_http_server_fails_its_handshake_at_request_timeout_ms() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            stall_initialize: true,
            ..FixtureOptions::default()
        })
        .await;
        let mut entry = http_entry(&fixture.url);
        entry.request_timeout_ms = Some(300.0);
        let started = std::time::Instant::now();
        let error = builder()
            .connect_http_client(&timed_request("wedged-http", entry))
            .await
            .expect_err("a silent HTTP server must not hang the connect");
        assert!(
            error.to_string().contains(HANDSHAKE_TIMED_OUT),
            "expected `{HANDSHAKE_TIMED_OUT}`, got {error}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    // ── MCP-114 · §3.4 steps 1–6 ──────────────────────────────────────────────────────────────

    /// `resolveServerUrl`'s missing-variable throw, with its singular/plural rule, raised **before**
    /// any socket is opened.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_url_with_a_missing_variable_fails_before_any_request() {
        let error = builder()
            .connect_http_client(&request("u", http_entry("https://x.example/${NOPE}")))
            .await
            .expect_err("a missing variable is a hard failure");
        assert_eq!(
            error.to_string(),
            "Missing environment variable in MCP server URL: NOPE"
        );

        let error = builder()
            .connect_http_client(&request(
                "u",
                http_entry("https://x.example/${NOPE}/${ALSONOPE}"),
            ))
            .await
            .expect_err("two missing variables pluralise");
        assert_eq!(
            error.to_string(),
            "Missing environment variables in MCP server URL: NOPE, ALSONOPE"
        );
    }

    /// Every header shape of §3.4 steps 2–6, read off the wire: a `!command` header runs, a `!!`
    /// header is an escaped literal, a plain header interpolates, and `auth: "bearer"` becomes
    /// exactly one `Authorization: Bearer <token>`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolved_headers_and_the_bearer_token_reach_the_wire() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        // `x-plain` interpolates against the REAL process environment, not the builder's seam —
        // see the residual named in `the_child_environment_is_resolved_exactly_as_resolve_env_
        // specifies`: `resolve_command_secrets_record` takes no `EnvFn`. The bearer ladder DOES
        // take the seam (`resolve_bearer_token`), which is why the two lines below read different
        // environments for the same syntax.
        let real_home = std::env::var("HOME").unwrap_or_default();
        entry.headers = Some(record(&[
            ("x-command", "!echo hunter2"),
            ("x-escaped", "!!literal"),
            ("x-plain", "${HOME}"),
        ]));
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Bearer));
        entry.bearer_token = Some("${TOKEN}".to_string());

        let connection = builder()
            .connect_http_client(&request("hdr", entry))
            .await
            .expect("connects");
        connection.resource.close().await.expect("close");

        let first = fixture.requests().first().cloned().expect("one request");
        assert_eq!(first.header("x-command"), Some("hunter2"));
        assert_eq!(first.header("x-escaped"), Some("!literal"));
        assert_eq!(first.header("x-plain"), Some(real_home.as_str()));
        assert_eq!(
            first.all("authorization"),
            vec!["Bearer s3cret"],
            "exactly one Authorization header, and it carries the bearer token"
        );
    }

    /// The header-injection guard: a command whose output contains a newline is refused with
    /// upstream's exact sentence, and nothing is sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_newline_bearing_command_secret_is_refused_with_upstreams_sentence() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.headers = Some(record(&[("x-bad", "!printf 'a\\nb'")]));

        let error = builder()
            .connect_http_client(&request("inject", entry))
            .await
            .expect_err("a newline cannot go on the wire");
        assert_eq!(
            error.to_string(),
            "Failed to resolve MCP server \"inject\" HTTP command secret: command returned an invalid header value"
        );
        assert!(fixture.requests().is_empty(), "nothing was sent");
    }

    // ── MCP-115 · the implicit/explicit ladder ────────────────────────────────────────────────

    /// The headline property of §3.4: with `auth` unset, **the credential store is not touched
    /// until the server answers 401 once**, and the retry after that challenge is on the same
    /// transport kind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn implicit_oauth_defers_the_provider_until_the_first_401() {
        let fixture = HttpFixture::start(1).await;
        let auth = CountingAuth::with_token("from-store");
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("implicit", http_entry(&fixture.url)))
            .await
            .expect("the second attempt succeeds");

        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert_eq!(fixture.initializes(), 2, "exactly two attempts");
        // ONE provider construction, and it happened only after the challenge arrived — carrying
        // that challenge, which is the `WWW-Authenticate` value the 401 sent.
        let calls = auth.calls();
        assert_eq!(calls.len(), 1, "one provider construction");
        assert_eq!(
            calls.first().and_then(Clone::clone).as_deref(),
            Some(
                "Bearer realm=\"mcp\", resource_metadata=\"https://example.invalid/.well-known\""
            ),
            "the challenge is carried into the provider"
        );
        // The retried attempt carried the token; the first did not.
        let initializes: Vec<Recorded> = fixture
            .requests()
            .into_iter()
            .filter(|request| request.body.contains("\"method\":\"initialize\""))
            .collect();
        assert_eq!(initializes.first().and_then(|r| r.header("authorization")), None);
        assert_eq!(
            initializes.get(1).and_then(|r| r.header("authorization")),
            Some("Bearer from-store")
        );
        assert!(auth.invalidations().is_empty(), "a success invalidates nothing");
        connection.resource.close().await.expect("close");
    }

    /// With `auth` set explicitly the provider exists from the first attempt — the other half of
    /// the implicit/explicit split.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_oauth_reads_the_store_before_the_first_attempt() {
        let fixture = HttpFixture::start(0).await;
        let auth = CountingAuth::with_token("eager");
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth));

        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("explicit", entry))
            .await
            .expect("connects on the first attempt");
        assert_eq!(fixture.initializes(), 1);
        assert_eq!(auth.calls(), vec![None], "constructed before any 401");
        assert_eq!(
            fixture
                .requests()
                .first()
                .and_then(|r| r.header("authorization")),
            Some("Bearer eager")
        );
        connection.resource.close().await.expect("close");
    }

    /// A permanent 401 yields `needs-auth` rather than an error, and
    /// `invalidateAuthEntryCache(name)` runs **at most once per episode** (MCP-116).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_permanent_401_becomes_needs_auth_and_invalidates_exactly_once() {
        let fixture = HttpFixture::start(usize::MAX).await;
        let auth = CountingAuth::empty();
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("locked", http_entry(&fixture.url)))
            .await
            .expect("needs-auth is not an error");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert!(connection.credentials_invalidated);
        assert_eq!(fixture.initializes(), 2, "one retry, then needs-auth");
        assert_eq!(auth.invalidations(), vec!["locked".to_string()]);

        // The carry-forward: a second connect that already knows the credential was discarded must
        // NOT discard it again.
        let mut second = request("locked", http_entry(&fixture.url));
        second.credentials_invalidated = true;
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&second)
            .await
            .expect("needs-auth again");
        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert_eq!(
            auth.invalidations(),
            vec!["locked".to_string()],
            "still exactly one invalidation across both episodes"
        );
    }

    /// `supportsOAuth` false ⇒ a 401 is a **hard error**, not `needs-auth`. `auth: false` is the
    /// shortest way to say that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_401_against_a_non_oauth_server_is_a_hard_error() {
        let fixture = HttpFixture::start(usize::MAX).await;
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Disabled(false));
        let auth = CountingAuth::empty();

        let error = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("noauth", entry))
            .await
            .expect_err("no OAuth means a 401 is fatal");
        assert!(error.to_string().contains("noauth"), "{error}");
        assert_eq!(fixture.initializes(), 1, "no retry without OAuth");
        assert!(auth.calls().is_empty(), "the store was never consulted");
        assert!(auth.invalidations().is_empty());
    }

    /// Arm 3 of the ladder: a close racing a 401 is an **abort**, not a `needs-auth`. The abort
    /// check sits between the aggregate rethrow and the 401 arms and the order is the specification.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_abort_racing_a_401_wins_over_the_needs_auth_arm() {
        // Deterministic by construction: the fixture cancels the attempt token BEFORE it writes
        // the first 401, so by the time the attempt settles the abort is already visible. A
        // sleep-then-cancel here would be a race against two loopback round trips, and the wrong
        // side of it is a `needs-auth` that silently passes for the wrong reason.
        let token = CancelToken::new();
        let fixture =
            HttpFixture::start_cancelling_before_response(usize::MAX, 1, token.clone()).await;
        let mut connect = request("raced", http_entry(&fixture.url));
        connect.attempt = token;
        let auth = CountingAuth::empty();
        let error = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&connect)
            .await
            .expect_err("an aborted connect is an error, never needs-auth");
        assert!(
            matches!(error, McpError::Aborted(_)),
            "expected an abort, got {error}"
        );
        assert!(auth.invalidations().is_empty(), "an abort invalidates nothing");
    }

    // ── MCP-115a · the per-request header command ─────────────────────────────────────────────

    /// The signing command runs on **every** outbound request, not once per connection, and its
    /// derived `Authorization` **replaces** the bearer one rather than joining it — upstream's
    /// `headers.set` semantics (`request-headers-command.ts:320`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_request_headers_command_signs_every_request_and_owns_authorization() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Bearer));
        entry.bearer_token = Some("static-bearer".to_string());
        entry.request_headers_command = Some(crate::config::HttpRequestHeadersCommand {
            command: Some("sh".to_string()),
            args: Some(vec![
                "-c".to_string(),
                "printf '{\"x-signature\":\"sig\",\"authorization\":\"Signature derived\"}'"
                    .to_string(),
            ]),
            ..crate::config::HttpRequestHeadersCommand::default()
        });

        let connection = builder()
            .connect_http_client(&request("signed", entry))
            .await
            .expect("connects");
        connection.resource.close().await.expect("close");

        let requests = fixture.requests();
        let posts: Vec<&Recorded> = requests
            .iter()
            .filter(|request| request.method == "POST")
            .collect();
        assert!(
            posts.len() >= 2,
            "initialize plus notifications/initialized at least: {}",
            posts.len()
        );
        for post in &posts {
            assert_eq!(post.header("x-signature"), Some("sig"));
            assert_eq!(
                post.all("authorization"),
                vec!["Signature derived"],
                "exactly one Authorization, and it is the DERIVED one"
            );
        }
    }

    /// A signing helper that fails aborts the request with upstream's sentence rather than sending
    /// an unsigned one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_signing_helper_aborts_the_request_instead_of_sending_it_unsigned() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.request_headers_command = Some(crate::config::HttpRequestHeadersCommand {
            command: Some("sh".to_string()),
            args: Some(vec!["-c".to_string(), "exit 7".to_string()]),
            ..crate::config::HttpRequestHeadersCommand::default()
        });

        let error = builder()
            .connect_http_client(&request("broken", entry))
            .await
            .expect_err("a failing helper fails the connect");
        assert!(
            error.to_string().contains("exited with code 7"),
            "expected the helper's own sentence: {error}"
        );
        assert!(
            fixture.requests().is_empty(),
            "nothing may go on the wire unsigned"
        );
    }


    // ── MCP-124 · does the taxonomy actually unblock `close`'s pending-connect rethrow? ────────

    /// A factory that parks until released, then fails with whatever it was given.
    #[derive(Debug)]
    struct ParkedFactory {
        gate: Arc<tokio::sync::Notify>,
        error: std::sync::Mutex<Option<McpError>>,
    }

    impl ConnectionFactory for ParkedFactory {
        fn create(&self, _request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
            let gate = Arc::clone(&self.gate);
            let error = self
                .error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            Box::pin(async move {
                gate.notified().await;
                Err(error.unwrap_or_else(|| McpError::other("no error was armed")))
            })
        }
    }

    /// `close`'s no-connection arm:
    /// ```text
    /// const pendingConnect = this.connectPromises.get(name);
    /// if (pendingConnect) { try { await pendingConnect } catch (error) {
    ///   if (this.containsCleanupFailure(error)) throw error; } }
    /// ```
    ///
    /// **This is the test the MCP-124 hand-off was about.** Before MCP-124 landed the aggregate
    /// variants, `McpError::is_cleanup_failure` matched only `RuntimeCleanupFailed | OAuthAggregate`
    /// — so a `createConnection` that failed with `MCP connection setup failed` was swallowed here
    /// and `close` reported success. The arm now fires, and it fires *structurally*: the
    /// discriminator is the variant, never the text.
    ///
    /// The pairing matters as much as the positive case: an ordinary connect failure — including one
    /// whose message literally reads `"MCP connection setup failed"` — must still be swallowed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn close_rethrows_a_pending_connects_setup_failure_and_swallows_everything_else() {
        async fn race_close_against(error: McpError) -> McpResult<()> {
            let gate = Arc::new(tokio::sync::Notify::new());
            let factory = Arc::new(ParkedFactory {
                gate: Arc::clone(&gate),
                error: std::sync::Mutex::new(Some(error)),
            });
            let manager = Arc::new(crate::server_manager::McpServerManager::with_factory(
                None, factory,
            ));
            let entry = ServerEntry {
                command: Some("true".to_string()),
                ..ServerEntry::default()
            };

            let connecting = {
                let manager = Arc::clone(&manager);
                let entry = entry.clone();
                tokio::spawn(async move { manager.connect("parked", &entry, None).await })
            };
            // Wait until the attempt is actually registered, so `close` takes the
            // no-connection-but-pending-connect branch rather than the do-nothing one.
            for _ in 0..200_u32 {
                if manager.is_connecting("parked") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(manager.is_connecting("parked"), "the connect must be in flight");

            let closing = {
                let manager = Arc::clone(&manager);
                tokio::spawn(async move { manager.close("parked").await })
            };
            tokio::time::sleep(Duration::from_millis(20)).await;
            gate.notify_waiters();

            let _ = connecting.await.expect("join connect");
            closing.await.expect("join close")
        }

        // The teardown failure: re-thrown.
        let outcome = race_close_against(McpError::SetupFailed(crate::errors::CleanupErrors::from(
            vec![
                McpError::other("connect ECONNREFUSED"),
                McpError::other("transport close failed"),
            ],
        )))
        .await;
        let error = outcome.expect_err("a setup failure must reach the closer");
        assert_eq!(
            error.to_string(),
            "connect ECONNREFUSED: transport close failed",
            "and it renders the way formatTerminalError renders it"
        );
        // The class survives the `ManagerError` → `McpError` boundary too, which is the half of
        // MCP-124 that `From<&ManagerError>` had to land: before it, this arrived as
        // `McpError::Other` carrying the same text and `is_cleanup_failure()` was `false`.
        assert!(
            matches!(error, McpError::SetupFailed(_)),
            "expected `SetupFailed` at the public boundary, got {error:?}"
        );
        assert!(error.is_cleanup_failure());

        // The ordinary connect failure: swallowed.
        race_close_against(McpError::other("connect ECONNREFUSED"))
            .await
            .expect("an ordinary connect failure during a close is expected and swallowed");

        // THE DOCUMENTED DIVERGENCE, at the one call site it changes: a server that puts the
        // aggregate's own text in an error message satisfies upstream's
        // `/cleanup failed|setup failed/` and is re-thrown there; here it is swallowed.
        race_close_against(McpError::Server {
            server: "parked".to_string(),
            message: "MCP connection setup failed".to_string(),
        })
        .await
        .expect("a text-only impostor is not a cleanup failure");
    }

    /// `SetupFailed`'s one producer that exists **today**, exercised.
    ///
    /// The doc on `create_connection` used to say the variant had "no producer on this path". It
    /// does have one — narrow, but real: `post_handshake`'s abort check is a genuine post-handshake
    /// step, and when a `close` racing a settled handshake trips it *and* the `resource.close()`
    /// after it also fails, the catch wraps both into `MCP connection setup failed`. Discovery
    /// (MCP-119) is upstream's producer and has not landed; that is a different sentence from
    /// "none exists", and the difference is what this test holds in place.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_abort_whose_own_cleanup_fails_is_a_setup_failure() {
        #[derive(Debug)]
        struct FailingClose;
        impl ConnectionResource for FailingClose {
            fn close(&self) -> futures::future::BoxFuture<'_, McpResult<()>> {
                Box::pin(async { Err(McpError::other("transport close failed")) })
            }
        }

        let mut connect = request("raced", stdio_entry("exec cat", &[]));
        // The close that arrived a microsecond after the handshake settled.
        connect.attempt = CancelToken::new();
        connect.attempt.cancel();

        // `NewConnection` is not `Debug`, so the Ok arm is destructured rather than `expect_err`'d.
        let Err(error) = builder()
            .post_handshake(&connect, Arc::new(FailingClose) as Arc<dyn ConnectionResource>, false)
            .await
        else {
            panic!("an aborted post-handshake step is a failure");
        };
        assert!(
            matches!(error, McpError::SetupFailed(_)),
            "expected `SetupFailed`, got {error:?}"
        );
        assert!(error.is_cleanup_failure(), "and `close_inner` must be able to see it");

        // The pairing: the SAME abort with a cleanup that SUCCEEDS is the abort, not a setup
        // failure. That is what makes the aggregate mean "the teardown failed too".
        #[derive(Debug)]
        struct CleanClose;
        impl ConnectionResource for CleanClose {
            fn close(&self) -> futures::future::BoxFuture<'_, McpResult<()>> {
                Box::pin(async { Ok(()) })
            }
        }
        let Err(error) = builder()
            .post_handshake(&connect, Arc::new(CleanClose) as Arc<dyn ConnectionResource>, false)
            .await
        else {
            panic!("still an abort");
        };
        assert!(matches!(error, McpError::Aborted(_)), "got {error:?}");
    }

    /// The factory is installable: `McpServerManager::with_factory(cwd, Arc::new(builder))` replaces
    /// `UnbuiltConnectionFactory` and a real server connects through the manager's own state
    /// machine. Without this the ladder above is only reachable from its own tests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_builder_is_a_usable_connection_factory_for_the_manager() {
        let fixture = HttpFixture::start(0).await;
        let manager = Arc::new(crate::server_manager::McpServerManager::with_factory(
            None,
            Arc::new(builder()),
        ));
        let connection = manager
            .connect("live", &http_entry(&fixture.url), None)
            .await
            .expect("the manager drives the builder");
        assert_eq!(connection.status(), ConnectionStatus::Connected);
        assert!(connection.resource().has_session_id());
        manager.close("live").await.expect("close");
    }


    /// A failed handshake must not leak the child, and the *mechanism* is worth pinning because it
    /// is not the obvious one: nothing in this port calls `close()` on that path.
    /// `serve_client_with_ct_inner` drops the transport, and `ChildWithCleanup::drop`
    /// (`rmcp-3.1.4/src/transport/child_process.rs:45-57`) spawns a `kill()`. If a future rmcp
    /// version moves the child out of that guard before its first await — which is exactly what
    /// `graceful_shutdown` already does — this test is what notices.
    ///
    /// The fixture ignores stdin closure and never answers `initialize`, so only the kill can end
    /// it. Linux-only because it reads `/proc`, the same gate `server_manager`'s child-process
    /// tests use.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_handshake_leaves_no_child_behind() {
        fn alive(pid: u32) -> bool {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            // `pid (comm) state …`; `comm` may contain spaces and parentheses, so the state is the
            // first field after the LAST `)`. A killed-but-unreaped child is `Z` and is NOT alive.
            stat.rsplit_once(')').is_some_and(|(_, rest)| {
                rest.trim_start().chars().next().is_some_and(|state| state != 'Z')
            })
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("pid");
        // Writes its pid, then blocks forever on a fifo-less read that stdin closure cannot end —
        // `sleep` ignores stdin entirely, which is the whole point.
        let script = "printf '%s' \"$$\" > \"$P\"; exec sleep 60";
        let mut entry = stdio_entry(script, &[]);
        entry.env = Some(record(&[("P", pid_file.to_string_lossy().into_owned().as_str())]));

        let connect = request("hung", entry);
        let token = connect.attempt.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            token.cancel();
        });
        let error = builder()
            .connect_stdio(&connect)
            .await
            .expect_err("a server that never answers `initialize` cannot connect");
        handle.await.expect("join");
        assert!(
            matches!(error, McpError::Server { .. }),
            "expected a connect failure, got {error}"
        );

        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("the child wrote its pid")
            .trim()
            .parse()
            .expect("a pid");
        // The kill is spawned as a detached task, so it is not synchronous with the error.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && alive(pid) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!alive(pid), "the child survived a failed handshake (pid {pid})");
    }

}
