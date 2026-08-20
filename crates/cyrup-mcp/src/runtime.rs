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
    let ui = snapshot
        .services
        .clone()
        .map(|services| Arc::new(OwnedServices::new(services, Arc::clone(&owner))));
    let _runtime_signal =
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
    let manager = Arc::new(McpServerManager { cwd: snapshot.cwd.clone() });

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
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransportConfig,
};
use rmcp::transport::{ConfigureCommandExt, IntoTransport, StreamableHttpClientTransport,
    TokioChildProcess};
use rmcp::{ClientHandler, ErrorData};
use tokio::process::ChildStderr;

use crate::config::{HttpTransport, ProtocolVersionSetting, ServerEntry};
use crate::errors::McpError;

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
/// The resolution itself is deliberately not here. `interpolateEnvVars` (MCP-143), the `!`/`!!`
/// command-secret grammar (MCP-144), `resolveEnv`'s full-environment copy and the npx/npm rewrite
/// (MCP-103) all belong to `secrets` / the connection builder; what this module owns is the point
/// where resolved values become a running child. Keeping the split here is what lets the spawn stay
/// a pure function of its inputs and be unit-tested without a shell.
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
/// `resolveServerUrl`'s three throws, `resolveCommandSecretsRecord` over the headers, the
/// `bearerToken`/`bearerTokenEnv` ladder and the `new Headers()` injection guard are MCP-114's, in
/// `secrets`. What arrives here is the finished pre-flight.
pub struct HttpTransportSpec {
    /// The `mcpServers` key, for the error strings.
    pub server: String,
    /// `resolveServerUrl(definition)` — interpolated and already proven to parse.
    pub url: String,
    /// The resolved header set **in file order** (`IndexMap`/`Vec` rather than a sorted map: a
    /// server that reads two same-named headers positionally would otherwise see them reordered).
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
/// [`McpError::Server`] when a header name or value is not representable on the wire. MCP-114 has
/// already validated the command-sourced ones with the exact upstream sentence; this arm therefore
/// only fires for a *statically configured* header that upstream would have let through to `fetch`
/// and failed on later — a **recorded divergence**: cyrup rejects it at transport construction, with
/// a message that does not falsely blame a command.
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
/// Upstream's fourth arm — `` throw new Error(`Invalid MCP protocolVersion: ${String(...)}`) `` —
/// is **unreachable from a parsed entry**: [`ProtocolVersionSetting`] is a closed enum, so an
/// illegal value is refused by `serde` at config load. The string still has to exist, and it does,
/// as [`invalid_protocol_version_message`], for the loader to raise.
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
#[must_use]
pub fn version_negotiation(entry: &ServerEntry) -> ClientLifecycleMode {
    match entry.protocol_version {
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
    }
}

/// `` `Invalid MCP protocolVersion: ${String(definition.protocolVersion)}` `` — upstream's fourth
/// arm, verbatim.
///
/// Raised by the config loader (13b validation rule 2) rather than by [`version_negotiation`],
/// because the typed enum means an illegal value never survives parsing. Exposed here so the string
/// lives next to the table it is the `default:` case of.
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
/// `requestOptions` does **not** appear: rmcp's connect has no per-request timeout knob, and
/// upstream's `requestOptions.signal` is the `ct` argument here. The timeout half lives on the
/// per-request path — [`build_request_options`].
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
        let absent = version_negotiation(&entry());
        let mut legacy = entry();
        legacy.protocol_version = Some(ProtocolVersionSetting::Legacy);
        assert!(matches!(absent, ClientLifecycleMode::Initialize));
        assert!(matches!(version_negotiation(&legacy), ClientLifecycleMode::Initialize));
    }

    #[test]
    fn auto_and_pin_map_to_their_rmcp_modes() {
        let mut auto = entry();
        auto.protocol_version = Some(ProtocolVersionSetting::Auto);
        match version_negotiation(&auto) {
            ClientLifecycleMode::Auto { preferred_versions, legacy_version } => {
                assert_eq!(preferred_versions, vec![ProtocolVersion::V_2026_07_28]);
                assert_eq!(legacy_version, Some(ProtocolVersion::LATEST));
            }
            other => panic!("expected Auto, got {other:?}"),
        }

        let mut pinned = entry();
        pinned.protocol_version = Some(ProtocolVersionSetting::V20260728);
        match version_negotiation(&pinned) {
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
