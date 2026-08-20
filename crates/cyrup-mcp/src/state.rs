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
//! [`AuthStorageOptions`], [`ServerToolMetadata`], [`PromptMetadata`]) were declared **here** because
//! `state.ts` is likewise a pure type file that names them as imports, and because
//! [`McpState`] cannot be landed without them. The unit that builds each subsystem replaces the
//! declaration with a one-line `pub use crate::<module>::<Type>;` at that point, which keeps
//! `crate::state::<Type>` a valid path for everything already written against it. Each one names
//! its owning unit. Cut 2 discharged two of the five: [`OAuthRuntime`] is now
//! [`crate::oauth::McpOAuthRuntime`] and [`AuthStorageOptions`] is now
//! [`crate::credentials::AuthStorageOptions`].

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::config::McpConfig;
use crate::dirs::{hex_sha256, stable_stringify, HashValue};
use crate::errors::McpResult;
use crate::lifecycle::McpLifecycleManager;
use crate::owner::{McpRuntimeOwner, OwnedServices};

/// `state.openBrowser` — `owner.throwIfInactive(); await openUrl(...); owner.throwIfInactive()`,
/// guarded on **both** sides of the await (13a §8 step 9). Boxed rather than a bare function
/// because it closes over the owner, the host `exec` handle and `$BROWSER`.
pub type OpenBrowser = Arc<dyn Fn(String) -> BoxFuture<'static, McpResult<()>> + Send + Sync>;

/// `state.sendMessage` — `if (!owner.isActive()) return;` then `pi.sendMessage(...)`. Returns
/// nothing: upstream's send is fire-and-forget, and the owner check is the whole guard.
///
/// **This is the v2.25.0 shape and it is incomplete at v2.26.1 — see plan unit MCP-027a.**
/// `48799fa` rewrote `init.ts:181-195`: the owner-guarded send became a `deliver` closure, and a
/// caller passing `options.triggerTurn` now has its message deferred behind
/// `lifecycle.ensureConverged(owner.signal)` so a turn cannot start against a stale keep-alive tool
/// catalog (delivering anyway, with one debug line, if convergence fails). This alias takes **no
/// options at all**, so that branch is not merely unported — it is inexpressible. Closing it changes
/// the alias, both structs that hold it, the builder at `runtime.rs:189` and every call site, which
/// is why it is a unit rather than an edit here.
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

/// The session approval cache key — `tool-approval.ts:151-152`.
///
/// Upstream builds `` `${serverName}\u0000${toolMeta.originalName}\u0000${argsHash}` `` where
/// `argsHash` is `createHash("sha256").update(stableStringify(args ?? {})).digest("hex")`. The set
/// it keys is [`McpState::approved_tool_calls`].
///
/// # The argument hash is the whole point
///
/// Upstream `5bcd6c5` (v2.26.1, issue #367) added it. Before it the key was just
/// `` `${serverName}\u0000${toolMeta.originalName}` ``, so one **Allow for session** on a harmless
/// payload silently approved every later call to that tool for the rest of the session — approve
/// `read_file {path: "README.md"}` once and `read_file {path: "~/.ssh/id_rsa"}` never prompts again.
/// That is a privilege escalation the user believes they granted a *narrow* permission to prevent,
/// which is why the approval is scoped to the payload the dialog actually displayed.
///
/// # Why the pre-image is `dirs`' `stable_stringify`, not a JSON serialiser
///
/// `tool-approval.ts:23-32`'s `stableStringify` is a verbatim copy of `metadata-cache.ts:344`, which
/// this crate already ports as [`crate::dirs::stable_stringify`] — sorted object keys, preserved
/// array order, the literal word `undefined` for an absent value. Serialising with `serde_json`
/// instead would make the digest depend on whether the `preserve_order` feature happens to be
/// unified into the build graph, and a second hand-rolled copy of the same walk is the silent drift
/// MCP-070 exists to prevent.
///
/// The key sort is what makes `{id, type}` and `{type, id}` **one** approval: a model re-emitting
/// its own arguments in a different order is asking to do the identical thing and must not
/// re-prompt. Array order stays significant — `["a", "b"]` and `["b", "a"]` are not always the same
/// request.
///
/// `args ?? {}` — an absent `args` and `args: {}` are one approval, so [`Value::Null`] (this port's
/// spelling of `undefined`) hashes as the empty object rather than as `null`.
///
/// # Caller
///
/// MCP-232's `ensureToolCallApproved`, which is **not yet ported**: it looks this key up before
/// prompting and inserts it on `Allow for session` (`tool-approval.ts:154`, `:161`, `:191`).
/// `13e-mcp-tools.md`'s MCP-232 text predates v2.26.1 and prescribes a `HashSet<(String, String)>`
/// keyed on `(server, original_tool)`, calling the loss of the NUL-joined form "not observable" — at
/// v2.26.1 that pair **is** the defect `5bcd6c5` fixed. The key is a triple.
#[must_use]
pub fn approval_cache_key(server: &str, original_tool: &str, args: &Value) -> String {
    let pre_image = if args.is_null() {
        // `args ?? {}`. Deliberately not `HashValue::from_json(Value::Null)`, which renders `null`
        // and would give "no arguments" a different approval from "empty arguments".
        stable_stringify(&HashValue::Object(Vec::new()))
    } else {
        stable_stringify(&HashValue::from_json(args.clone()))
    };
    let args_hash = hex_sha256(pre_image.as_bytes());
    format!("{server}\u{0}{original_tool}\u{0}{args_hash}")
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
/// **Landed by 13g (MCP-301).** The forward declaration is gone; `crate::state::OAuthRuntime` stays
/// a valid path for everything already written against it, and now names the real runtime.
pub use crate::oauth::McpOAuthRuntime as OAuthRuntime;

/// `mcp-auth.ts`'s `getAuthStorageOptions(settings.oauthDir, cwd)` — where credentials live.
/// `$MCP_OAUTH_DIR` (trimmed) outranks the configured dir, which outranks
/// `<agent_dir>/mcp-oauth`.
///
/// **Landed by 13f (MCP-265).** Note the shape change the real type brings: `base_dir` is
/// `Option<PathBuf>`, and **absent is not the same as `<agent_dir>/mcp-oauth`** — the precedence
/// ladder in [`crate::credentials::McpAuthStore::auth_base_dir`] is what turns absent into that, so
/// pre-resolving it here would defeat `$MCP_OAUTH_DIR`.
pub use crate::credentials::AuthStorageOptions;

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

#[cfg(test)]
mod tests {
    use super::approval_cache_key;
    use serde_json::{json, Value};

    /// `__tests__/tool-approval.test.ts` "caches only Allow for session decisions", as rewritten by
    /// `5bcd6c5`: three calls, **two** approvals. The reordered payload is the same request; the
    /// changed `id` is a new one and must prompt again.
    ///
    /// The reordering half holds whichever map backing `serde_json` was built with, because
    /// [`crate::dirs::stable_stringify`] sorts the keys itself.
    #[test]
    fn a_session_approval_is_scoped_to_its_arguments() {
        let approved = approval_cache_key(
            "demo",
            "search-records",
            &json!({"record": {"id": "safe", "type": "demo"}}),
        );
        let reordered = approval_cache_key(
            "demo",
            "search-records",
            &json!({"record": {"type": "demo", "id": "safe"}}),
        );
        let other = approval_cache_key(
            "demo",
            "search-records",
            &json!({"record": {"id": "other", "type": "demo"}}),
        );

        assert_eq!(approved, reordered, "the same payload in a different key order is one request");
        assert_ne!(approved, other, "a changed argument must not inherit an earlier approval (#367)");
    }

    /// `stableStringify(args ?? {})` — `mcp({tool})` and `mcp({tool, args: {}})` are one approval,
    /// and neither is an approval of a payload that actually carries a key.
    #[test]
    fn absent_arguments_hash_as_the_empty_object() {
        assert_eq!(
            approval_cache_key("demo", "search-records", &Value::Null),
            approval_cache_key("demo", "search-records", &json!({}))
        );
        assert_ne!(
            approval_cache_key("demo", "search-records", &Value::Null),
            approval_cache_key("demo", "search-records", &json!({"query": ""}))
        );
    }

    /// The two fields the key had before `5bcd6c5` still separate approvals: approving a tool on one
    /// server has never approved the same tool name on another.
    #[test]
    fn the_server_and_the_tool_still_separate_approvals() {
        let args = json!({"query": "x"});
        let base = approval_cache_key("demo", "search-records", &args);
        assert_ne!(base, approval_cache_key("other", "search-records", &args));
        assert_ne!(base, approval_cache_key("demo", "delete-records", &args));
    }

    /// Array order is not sorted away — `stableStringify` maps arrays elementwise. Deleting
    /// `["a", "b"]` is not always deleting `["b", "a"]`, and neither approval covers the other.
    #[test]
    fn array_order_stays_significant() {
        assert_ne!(
            approval_cache_key("demo", "delete-records", &json!({"ids": ["a", "b"]})),
            approval_cache_key("demo", "delete-records", &json!({"ids": ["b", "a"]}))
        );
    }

    /// The shape: `server`, `NUL`, `tool`, `NUL`, 64 lower-case hex. Upstream's separator is `NUL`
    /// precisely because no server or tool name can contain one, so no two `(server, tool)` pairs
    /// can join into the same string.
    #[test]
    fn the_key_is_two_nul_separated_names_and_a_sha256() {
        let key = approval_cache_key("demo", "search-records", &json!({"query": "x"}));
        let mut fields = key.split('\u{0}');
        assert_eq!(fields.next(), Some("demo"));
        assert_eq!(fields.next(), Some("search-records"));
        let digest = fields.next().unwrap_or_default();
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_eq!(fields.next(), None);
    }
}
