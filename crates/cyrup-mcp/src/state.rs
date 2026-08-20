//! `McpState` — the runtime record one generation owns (`state.ts`; 13a §8 step 9, MCP-001).
//!
//! Upstream's `McpExtensionState` carries **25** fields. Five are cut, leaving the **twenty** on
//! [`McpState`]:
//!
//! * `approvalEvents` — the pi-bus approval broker, subsumed by `ExtHooks::before_tool_call` plus
//!   `cyrup-permission-system`'s existing MCP target derivation, which is the same gate and is
//!   already wired and fail-closed;
//! * `uiResourceHandler`, `consentManager`, `uiServer`, `completedUiSessions` — all **Cut 2**
//!   (MCP Apps / the UI extension, entirely).
//!
//! # Shape: one `Arc<McpState>`, interior mutability per field
//!
//! Upstream is a mutable JavaScript record captured by a dozen closures, mutated freely because the
//! runtime is single-threaded. Here the record is built once, wrapped in an `Arc`, and shared; the
//! fields that genuinely mutate after construction carry their own lock. The maps are
//! [`indexmap::IndexMap`] rather than `HashMap` for the same reason
//! [`crate::config::McpConfig::mcp_servers`] is: `new Map()` preserves insertion order, and that
//! order is the order servers connect in and the order they list in `/mcp`.
//!
//! # Forward declarations, and how they get replaced
//!
//! Five collaborator types below ([`McpServerManager`], [`OAuthRuntime`],
//! [`AuthStorageOptions`], [`ServerToolMetadata`], [`PromptMetadata`]) are declared **here** because
//! `state.ts` is likewise a pure type file that names them as imports, and because
//! [`McpState`] cannot be landed without them. The unit that builds each subsystem replaces the
//! declaration with a one-line `pub use crate::<module>::<Type>;` at that point, which keeps
//! `crate::state::<Type>` a valid path for everything already written against it. Each one names
//! its owning unit.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::McpConfig;
use crate::errors::McpResult;
use crate::lifecycle::McpLifecycleManager;
use crate::owner::{McpRuntimeOwner, OwnedServices};

/// `state.openBrowser` — `owner.throwIfInactive(); await openUrl(...); owner.throwIfInactive()`,
/// guarded on **both** sides of the await (13a §8 step 9). Boxed rather than a bare function
/// because it closes over the owner, the host `exec` handle and `$BROWSER`.
pub type OpenBrowser = Arc<dyn Fn(String) -> BoxFuture<'static, McpResult<()>> + Send + Sync>;

/// `state.sendMessage` — `if (!owner.isActive()) return;` then `pi.sendMessage(...)`. Returns
/// nothing: upstream's send is fire-and-forget, and the owner check is the whole guard.
pub type SendMessage = Arc<dyn Fn(String) + Send + Sync>;

/// `state.onToolMetadataUpdated(serverName, reason)` — installed by `startInitialization` *after*
/// the state commits, so a hook fired mid-build cannot see a half-installed surface (MCP-011).
pub type ToolMetadataListener = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The live runtime for one generation of the MCP extension.
///
/// Built by [`crate::runtime::initialize_mcp`], committed by `startInitialization`, and torn down
/// by `shutdownState`. Everything reachable from here belongs to exactly one
/// [`McpRuntimeOwner`]; when that owner stops, every field is inert.
pub struct McpState {
    /// 1 · The generation's ownership token, cleanup stack and memoised stop.
    pub owner: Arc<McpRuntimeOwner>,
    /// 2 · Connections, the five race guards and the generation fencing.
    pub manager: Arc<McpServerManager>,
    /// 3 · The reconnect / idle-shutdown health-check state machine.
    pub lifecycle: Arc<McpLifecycleManager>,
    /// 4 · Per-server tool metadata — the source of every direct tool and of the proxy tool's
    /// description. Populated from `mcp-cache.json` at load and refreshed on connect.
    pub tool_metadata: Mutex<IndexMap<String, ServerToolMetadata>>,
    /// 5 · Per-server resource counts. A count, not the resources: the panel shows it and
    /// `updateMetadataCache` preserves it.
    pub resource_counts: Mutex<IndexMap<String, usize>>,
    /// 6 · Per-server prompt metadata, from either the cache or a live `prompts/list`.
    pub prompt_metadata: Mutex<IndexMap<String, Vec<PromptMetadata>>>,
    /// 7 · Which servers' prompt metadata came from a **live** connection. A cache-rehydrated
    /// prompt list is deliberately *not* added here, which is what flags it as non-live.
    pub prompt_metadata_live: Mutex<HashSet<String>>,
    /// 8 · Per-server `instructions` from the initialize handshake, surfaced in the system prompt.
    pub server_instructions: Mutex<IndexMap<String, String>>,
    /// 9 · The resolved configuration this generation is running.
    pub config: McpConfig,
    /// 10 · The caller-supplied programmatic config, if the adapter was constructed with one —
    /// cloned at factory time **and again per call**, upstream, so a caller cannot mutate a live
    /// runtime's configuration through the object it passed in.
    pub programmatic_config: Option<McpConfig>,
    /// 11 · The OAuth flow runtime. Owned by this generation only when the caller did not supply
    /// one; when owned, its shutdown is registered as an owner cleanup.
    pub oauth_runtime: Arc<OAuthRuntime>,
    /// 12 · Where credentials live — `$MCP_OAUTH_DIR`, then `settings.oauthDir`, then
    /// `<agent_dir>/mcp-oauth`.
    pub auth_storage_options: AuthStorageOptions,
    /// 13 · Per-server connect-failure state driving the **60-second backoff**. An abort must never
    /// land here: misclassifying a user cancellation as a connection failure poisons the next
    /// minute of that server's availability (see [`crate::abort::is_abort_error`]).
    pub failure_tracker: Mutex<IndexMap<String, ServerFailure>>,
    /// 14 · The last failure message per server, shown by `/mcp` and the panel.
    pub failure_messages: Mutex<IndexMap<String, String>>,
    /// 15 · The session-scoped approval cache — tool calls the user already approved.
    pub approved_tool_calls: Mutex<HashSet<String>>,
    /// 16 · Opens a URL, owner-guarded on both sides of the await.
    pub open_browser: OpenBrowser,
    /// 17 · The **fenced** host services handle: `createOwnedUi(rawUi, owner)`. `None` in a headless
    /// build. Every `ui?.notify(...)` in the runtime relies on this being inert after a stop rather
    /// than needing its own owner check.
    pub ui: Option<Arc<OwnedServices>>,
    /// 18 · Injects a message into the session, owner-guarded.
    pub send_message: SendMessage,
    /// 19 · The metadata-update hook, installed after the state commits.
    pub on_tool_metadata_updated: Mutex<Option<ToolMetadataListener>>,
    /// 20 · The status snapshot channel. Upstream publishes `MCP_STATUS_EVENT` v1 on `pi.events`;
    /// **no consumer for that exists in cyrup**, so the snapshot stays an in-crate
    /// [`tokio::sync::watch`] rather than inventing a bus topic nothing reads.
    pub status_events: watch::Sender<McpStatusSnapshot>,
}

/// The collaborators [`McpState::new`] cannot default — everything else it allocates itself, which
/// is `initializeMcp` step 8's "allocate the live maps/sets".
pub struct McpStateParts {
    /// See [`McpState::owner`].
    pub owner: Arc<McpRuntimeOwner>,
    /// See [`McpState::manager`].
    pub manager: Arc<McpServerManager>,
    /// See [`McpState::lifecycle`].
    pub lifecycle: Arc<McpLifecycleManager>,
    /// See [`McpState::config`].
    pub config: McpConfig,
    /// See [`McpState::programmatic_config`].
    pub programmatic_config: Option<McpConfig>,
    /// See [`McpState::oauth_runtime`].
    pub oauth_runtime: Arc<OAuthRuntime>,
    /// See [`McpState::auth_storage_options`].
    pub auth_storage_options: AuthStorageOptions,
    /// See [`McpState::ui`].
    pub ui: Option<Arc<OwnedServices>>,
    /// See [`McpState::open_browser`].
    pub open_browser: OpenBrowser,
    /// See [`McpState::send_message`].
    pub send_message: SendMessage,
}

impl McpState {
    /// `initializeMcp` steps 8–9: allocate the live maps and sets, then build the record.
    #[must_use]
    pub fn new(parts: McpStateParts) -> Self {
        let (status_events, _) = watch::channel(McpStatusSnapshot::default());
        Self {
            owner: parts.owner,
            manager: parts.manager,
            lifecycle: parts.lifecycle,
            tool_metadata: Mutex::new(IndexMap::new()),
            resource_counts: Mutex::new(IndexMap::new()),
            prompt_metadata: Mutex::new(IndexMap::new()),
            prompt_metadata_live: Mutex::new(HashSet::new()),
            server_instructions: Mutex::new(IndexMap::new()),
            config: parts.config,
            programmatic_config: parts.programmatic_config,
            oauth_runtime: parts.oauth_runtime,
            auth_storage_options: parts.auth_storage_options,
            failure_tracker: Mutex::new(IndexMap::new()),
            failure_messages: Mutex::new(IndexMap::new()),
            approved_tool_calls: Mutex::new(HashSet::new()),
            open_browser: parts.open_browser,
            ui: parts.ui,
            send_message: parts.send_message,
            on_tool_metadata_updated: Mutex::new(None),
            status_events,
        }
    }

    /// `publishMcpStatusSnapshot(state)` — into the in-crate watch channel. A `watch` send with no
    /// receivers is not an error, which matches a bus publish nobody subscribed to.
    pub fn publish_status(&self, snapshot: McpStatusSnapshot) {
        let _ = self.status_events.send(snapshot);
    }

    /// Observe the status snapshot — the seam `/mcp`, the footer and the panel read.
    #[must_use]
    pub fn subscribe_status(&self) -> watch::Receiver<McpStatusSnapshot> {
        self.status_events.subscribe()
    }

    /// `notifyToolMetadataUpdated(state, serverName, reason)`.
    ///
    /// A hook must **never break a connect** (MCP-030), so the listener is cloned out from under
    /// the lock and invoked outside it: a listener that re-enters this state cannot deadlock, and
    /// a poisoned lock degrades to "no listener" rather than to a failed connect.
    pub fn notify_tool_metadata_updated(&self, server: &str, reason: &str) {
        let listener = match self.on_tool_metadata_updated.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => None,
        };
        if let Some(listener) = listener {
            listener(server, reason);
        }
    }

    /// Install the metadata-update hook. Called by `startInitialization` **after** the state
    /// commits.
    pub fn set_tool_metadata_listener(&self, listener: Option<ToolMetadataListener>) {
        if let Ok(mut slot) = self.on_tool_metadata_updated.lock() {
            *slot = listener;
        }
    }
}

impl std::fmt::Debug for McpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState")
            .field("active", &self.owner.is_active())
            .field("servers", &self.config.mcp_servers.len())
            .field("has_ui", &self.ui.is_some())
            .finish_non_exhaustive()
    }
}

// =================================================================================================
// Forward declarations — see the module docs. Each names the unit that replaces it.
// =================================================================================================

/// `server-manager.ts`'s `McpServerManager`: the connection table, the five race guards, the
/// generation fencing, transport construction and `withSessionRecovery`.
///
/// **Forward declaration (13c / MCP-091…MCP-140 replace it with `pub use crate::manager::…`).**
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct McpServerManager {
    /// The session working directory `new McpServerManager(cwd)` is constructed with — the base
    /// every `resolveConfigPath` resolves against.
    pub cwd: std::path::PathBuf,
}

/// `oauth.ts`'s `createOAuthRuntime(signal)`: the flow registry, its own generation counter and the
/// four in-flight maps.
///
/// **Forward declaration (13g / MCP-280…MCP-330 replace it).**
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct OAuthRuntime {}

/// `mcp-auth.ts`'s `getAuthStorageOptions(settings.oauthDir, cwd)` — where credentials live.
/// `$MCP_OAUTH_DIR` (trimmed) outranks the configured dir, which outranks
/// `<agent_dir>/mcp-oauth`.
///
/// **Forward declaration (13f / MCP-260…MCP-279 replace it).**
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct AuthStorageOptions {
    /// The resolved storage root.
    pub base_dir: std::path::PathBuf,
}

/// `tool-metadata.ts`'s per-server metadata: the tools, their schemas, their resolved names, and
/// the prefix they were named under.
///
/// **Forward declaration (13e / MCP-200…MCP-259 replace it).**
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ServerToolMetadata {
    /// The resolved, model-visible tool names this server contributes, in server order.
    pub tool_names: Vec<String>,
}

/// `prompts.ts`'s per-prompt metadata: the prompt's name, its arguments and the slash command it
/// becomes.
///
/// **Forward declaration (MCP-039 / MCP-39x replace it).**
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PromptMetadata {
    /// The MCP prompt name, as the server reported it.
    pub name: String,
}

/// One server's connect-failure state — the input to the 60-second backoff (MCP-024).
#[derive(Debug, Clone)]
pub struct ServerFailure {
    /// When the last failure was recorded. `Instant`, not wall-clock: the backoff is a duration and
    /// must not be movable by a clock adjustment.
    pub last_failure: std::time::Instant,
    /// How many consecutive failures have been seen.
    pub count: u32,
}

impl Default for ServerFailure {
    fn default() -> Self {
        Self { last_failure: std::time::Instant::now(), count: 0 }
    }
}

/// The status snapshot upstream publishes as `MCP_STATUS_EVENT` v1. Kept in-crate on a
/// [`tokio::sync::watch`]: cyrup has no consumer for a bus topic here, and inventing one nothing
/// reads would be new functionality rather than a port.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusSnapshot {
    /// Servers currently connected.
    pub connected: Vec<String>,
    /// Servers configured but not connected.
    pub idle: Vec<String>,
    /// Servers whose last connect failed, with the message `/mcp` shows.
    pub failed: Vec<String>,
    /// Servers waiting on an OAuth flow.
    pub pending_auth: Vec<String>,
}
