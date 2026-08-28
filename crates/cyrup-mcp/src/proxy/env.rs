//! The collaborator seam — [`ProxyEnv`], and the context every mode takes.
//!
//! See [`crate::proxy`] for the module overview.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::{Map as JsonMap, Value};

use cyrup_core::{CancelToken, Content};

use crate::abort::combine;
use crate::config::{
    McpConfig, McpSettings, ServerEntry,
};
use crate::errors::{McpError, McpResult};
use crate::owner::McpRuntimeOwner;
use crate::state::McpState;
use crate::proxy::approval::{ensure_tool_call_approved, is_tool_call_approval_required};
use crate::proxy::call::AuthRecovery;
use crate::proxy::ranking::rank_suggestions;
use crate::proxy::tool_metadata::ToolMetadata;

// ==================================================================================================
// 4 · The collaborator seam — `ProxyEnv`, and the context every mode takes
// ==================================================================================================

/// `types.ts:138` `McpConnection["status"]` — the three states a connection can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Handshake completed; `tools/call` will be accepted.
    Connected,
    /// The transport closed, or was never opened.
    Closed,
    /// The server answered `401`/`WWW-Authenticate`; an OAuth flow must run first.
    NeedsAuth,
}

/// What `manager.connect` / `manager.reconnect` hand back to [`crate::proxy::execute_connect`].
///
/// The `buildToolMetadata(connection.tools, connection.resources, …)` step is applied by the
/// implementor rather than here: that builder is `tool-metadata.ts`'s and is owned by 13e
/// (MCP-207). Everything downstream of it — where the metadata is stored, when instructions are
/// **deleted** rather than set, and the eight-step commit order — stays in this file, because that
/// order is the port.
#[derive(Debug, Clone, Default)]
pub struct ConnectOutcome {
    /// `connection.status`.
    pub status: Option<ConnectionStatus>,
    /// `buildToolMetadata(...).metadata` for this server.
    pub metadata: Vec<ToolMetadata>,
    /// `connection.instructions` — `None` means **delete** the cached entry, not "leave it".
    pub instructions: Option<String>,
    /// `connection.promptDiscoveryFailed` — when false, prompt metadata is reconstructed and the
    /// server joins `promptMetadataLive`.
    pub prompt_discovery_failed: bool,
}

impl ConnectOutcome {
    /// `connection.status === "needs-auth"`.
    #[must_use]
    pub fn needs_auth(&self) -> bool {
        self.status == Some(ConnectionStatus::NeedsAuth)
    }
}

/// The already-transformed payload of one `tools/call`.
///
/// `content` has been through `transformMcpContent` / `resolveMcpResultContent`
/// (`tool-registrar.ts`, owned by 13e) before it reaches this file; `raw` is the untouched
/// `CallToolResult` the output guard stores as `rawMcpResult`.
#[derive(Debug, Clone, Default)]
pub struct CallToolOutcome {
    /// The transformed content blocks.
    pub content: Vec<Content>,
    /// `result.isError` — the discriminator for the `tool_error` path.
    pub is_error: bool,
    /// The raw MCP result, for `guardedMcpDetails`' `mcpResult` key.
    pub raw: Option<Value>,
}

/// The three failures [`crate::proxy::execute_call`]'s catch block distinguishes (13d §10, MCP-165).
#[derive(Debug)]
pub enum ProxyCallError {
    /// `session-recovery.ts`'s `SessionRecoveryAuthRequiredError` — a mid-request `needs-auth` that
    /// [`crate::proxy::attempt_auto_auth`] could not rescue. `auth_message` is the error's own text when it
    /// carried one.
    SessionRecoveryAuthRequired {
        /// The server the recovery was attempted against.
        server: String,
        /// `error.authMessage`, when present.
        auth_message: Option<String>,
    },
    /// rmcp's `UrlElicitationRequiredError` — the server wants a URL interaction first.
    UrlElicitationRequired {
        /// Opaque detail handed straight back to `manager.handleUrlElicitationRequired`.
        detail: String,
    },
    /// Everything else, including aborts.
    Other(McpError),
}

/// `manager.handleUrlElicitationRequired`'s verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlElicitationAction {
    /// The user opened the URL; the tool did not run and must be retried.
    Accept,
    /// The user refused.
    Decline,
    /// The interaction was cancelled.
    Cancel,
}

impl UrlElicitationAction {
    /// The `details.action` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            UrlElicitationAction::Accept => "accept",
            UrlElicitationAction::Decline => "decline",
            UrlElicitationAction::Cancel => "cancel",
        }
    }
}

/// `types.ts:477 @v2.26.1` `McpToolApprovalOrigin` — which surface is asking.
///
/// Upstream has five: `"proxy" | "direct" | "script" | "resource" | "iframe"`. Two are cut and
/// neither leaves a hole: `"script"` is **Cut 4** (`mcpScript` / the JS worker) and `"iframe"` is
/// **Cut 2** (MCP Apps, `ui-server.ts:474`). The three that survive are the three surfaces that can
/// still reach a tool.
///
/// The value reaches only `requestBrokerApproval` upstream, which is MCP-233's cut — so it is
/// carried and not yet read. See [`ensure_tool_call_approved`] for why it stays in the signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOrigin {
    /// `proxy-modes.ts:1145` — a `mcp({tool})` call through the gateway.
    Proxy,
    /// `direct-tools.ts:440 @v2.26.1` — a registered per-tool direct tool.
    Direct,
    /// A resource tool, derived when `toolMeta.resourceUri` is set. Reached from **both** call
    /// sites, which is why the derivation is a constructor rather than a literal.
    Resource,
}

impl ApprovalOrigin {
    /// `proxy-modes.ts:1145` `origin ?? (toolMeta.resourceUri ? "resource" : "proxy")` — the
    /// gateway's derivation, applied only when the caller passed no explicit origin.
    #[must_use]
    pub const fn for_proxy_call(resource_uri: Option<&String>) -> Self {
        match resource_uri {
            Some(_) => ApprovalOrigin::Resource,
            None => ApprovalOrigin::Proxy,
        }
    }

    /// `direct-tools.ts:440 @v2.26.1` `spec.resourceUri ? "resource" : "direct"` — the direct-tool
    /// derivation.
    ///
    /// **The two derivations differ in their fallback and only there**, which is the whole reason
    /// both are written out: a resource tool reports `resource` whichever surface invoked it, while
    /// a plain tool reports the surface. Collapsing them into one helper would make every direct
    /// tool call claim it came through the gateway.
    #[must_use]
    pub const fn for_direct_tool(resource_uri: Option<&String>) -> Self {
        match resource_uri {
            Some(_) => ApprovalOrigin::Resource,
            None => ApprovalOrigin::Direct,
        }
    }

    /// The `details.origin` spelling — `types.ts:477 @v2.26.1`'s own strings.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ApprovalOrigin::Proxy => "proxy",
            ApprovalOrigin::Direct => "direct",
            ApprovalOrigin::Resource => "resource",
        }
    }
}

/// `tool-approval.ts`'s `ToolCallApprovalResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// `{ok: true}`.
    Approved,
    /// `{ok: false, reason: "denied"}` — the user said no.
    Denied,
    /// `{ok: false, reason: …}` — approval is required and there is no interactive session.
    NoInteractiveSession,
}

/// The per-call-site half of `guardMcpOutput`'s options.
///
/// The limits themselves come from `resolveMcpOutputGuardOptions(config.settings)` — already
/// available as [`crate::config::McpSettings::output_guard`] — and are read by the implementor;
/// only these four vary between the three call sites.
#[derive(Debug, Clone, Default)]
pub struct OutputGuardOptions {
    /// `"Error: "` on the tool-error path, `"Failed to call tool: "` in the catch, else empty.
    pub prefix: String,
    /// `"\n\nExpected parameters:\n<formatSchema>"` when an input schema exists.
    pub suffix: String,
    /// `"Tool execution failed"` on the tool-error path.
    pub empty_text_fallback: Option<String>,
    /// The untouched MCP result, stored as `details.mcpResult` when it survives the size cap.
    pub raw_mcp_result: Option<Value>,
}

/// `mcp-output-guard.ts`'s `GuardedMcpOutput`.
#[derive(Debug, Clone, Default)]
pub struct GuardedOutput {
    /// The bounded content actually returned to the model.
    pub content: Vec<Content>,
    /// `details.mcpResult`, when the raw result fit under `detailsMaxBytes`.
    pub mcp_result: Option<Value>,
    /// `details.outputGuard`, when text was truncated or spilled to a file.
    pub output_guard: Option<Value>,
}

impl GuardedOutput {
    /// `mcp-output-guard.ts:78` `guardedMcpDetails(guarded)` — each key present **only** when set.
    pub(crate) fn write_details(&self, details: &mut JsonMap<String, Value>) {
        if let Some(result) = &self.mcp_result {
            details.insert("mcpResult".to_string(), result.clone());
        }
        if let Some(guard) = &self.output_guard {
            details.insert("outputGuard".to_string(), guard.clone());
        }
    }
}

/// The subsystems the proxy modes call into, late-bound.
///
/// Each method names the upstream function it stands for. Implementing this trait is the whole of
/// integrating 13d with 13a/13c/13e/13g; the call order and branch structure live in this file.
#[async_trait::async_trait]
pub trait ProxyEnv: Send + Sync {
    // --- server-manager.ts -----------------------------------------------------------------------
    /// `state.manager.getConnection(server)?.status`.
    fn get_connection(&self, server: &str) -> Option<ConnectionStatus>;
    /// `state.manager.isConnecting(server)` — drives [`crate::proxy::execute_search`]'s zero-result hint.
    fn is_connecting(&self, server: &str) -> bool;
    /// `state.manager.connect(server, definition, signal)`.
    async fn connect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome>;
    /// `state.manager.reconnect(server, definition, currentConnection, signal)`.
    async fn reconnect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome>;
    /// `init.ts`'s `lazyConnect(state, server, signal)` — `true` iff the server ended `connected`.
    async fn lazy_connect(&self, server: &str, cancel: &CancelToken) -> bool;
    /// `state.manager.close(server)`.
    async fn close(&self, server: &str);
    /// `state.manager.touch(server)`.
    fn touch(&self, server: &str);
    /// `state.manager.incrementInFlight(server)`.
    fn increment_in_flight(&self, server: &str);
    /// `state.manager.decrementInFlight(server)`.
    fn decrement_in_flight(&self, server: &str);
    /// `withSessionRecovery(..., conn => abortable(conn.client.callTool({name, arguments}), signal))`.
    ///
    /// The cancellation wrapper belongs on **this** side: rmcp's shape is
    /// `Peer::send_request_with_option(...)` → `RequestHandle`, with a task calling
    /// `RequestHandle::cancel(reason)` when `cancel` fires.
    ///
    /// `recovery` is the `onNeedsAuth` callback — call [`AuthRecovery::recover`] from inside the
    /// recovery loop rather than re-deriving the ladder, so the single-shot latch is honoured
    /// (MCP-162).
    async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: JsonMap<String, Value>,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
    ) -> Result<CallToolOutcome, ProxyCallError>;
    /// `withSessionRecovery(..., conn => conn.client.readResource({uri}, requestOptions))`.
    ///
    /// **Deliberately not wrapped in `abortable`** — upstream's asymmetry, reproduced rather than
    /// "fixed": a resource read is cancellable only through the request options' own signal
    /// (13d §10). `recovery` is the same `onNeedsAuth` callback [`ProxyEnv::call_tool`] takes.
    async fn read_resource(
        &self,
        server: &str,
        uri: &str,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
    ) -> Result<Vec<Content>, ProxyCallError>;
    /// `state.manager.handleUrlElicitationRequired(server, error)`.
    async fn handle_url_elicitation_required(&self, server: &str, detail: &str) -> UrlElicitationAction;

    // --- init.ts ---------------------------------------------------------------------------------
    /// `getFailureAgeSeconds(state, server)` — `None` outside the 60-second backoff window.
    fn failure_age_seconds(&self, server: &str) -> Option<u64>;
    /// `recordFailure(state, server, message)`. Never called for an abort.
    fn record_failure(&self, server: &str, message: &str);
    /// `clearFailure(state, server)`.
    fn clear_failure(&self, server: &str);
    /// `updateStatusBar(state)`.
    fn update_status_bar(&self);
    /// `updateServerMetadata(state, server)` — refresh `state.toolMetadata` from a live connection.
    fn update_server_metadata(&self, server: &str);
    /// `updateMetadataCache(state, server)` — write `<agent_dir>/mcp-cache.json`.
    fn update_metadata_cache(&self, server: &str);
    /// `markKeepAliveAfterConnect(state, server)`.
    fn mark_keep_alive_after_connect(&self, server: &str);
    /// `state.promptMetadata.set(...)` + `state.promptMetadataLive.add(...)` via
    /// `reconstructPromptMetadata` — run only when `!connection.promptDiscoveryFailed`.
    fn commit_prompt_metadata(&self, server: &str);
    /// `syncToolSurface(ctx)` — re-derive direct tools and the `mcp` description after a connect
    /// (dispatch arm 4). A no-op until `HA-1` lands (MCP-193).
    fn sync_tool_surface(&self);

    // --- mcp-auth-flow.ts ------------------------------------------------------------------------
    /// `supportsOAuth(definition)`.
    fn supports_oauth(&self, definition: &ServerEntry) -> bool;
    /// `utils.ts:167` `resolveServerUrl(definition)`.
    ///
    /// `Ok(None)` is a falsy URL (a stdio server); `Err` is the **throw** — a missing `${VAR}` or a
    /// URL that will not parse after interpolation. [`crate::proxy::attempt_auto_auth`] treats those differently
    /// and the distinction is load-bearing.
    ///
    /// **The implementation exists: [`crate::credentials::resolve_server_url`]** (MCP-084), which
    /// carries upstream's three byte-exact messages and is the same function
    /// [`crate::dirs::ResolvedIdentity::resolve`] hashes through. A production implementor must
    /// delegate to it rather than mint a second copy — the config digest and the connect path have
    /// to agree about what a server's URL *is*, or a server connects to one host and caches under
    /// another.
    fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>>;
    /// `authenticate(server, url, definition, {authStorageOptions?, signal?, runtime})`.
    async fn authenticate(
        &self,
        server: &str,
        server_url: &str,
        definition: &ServerEntry,
        cancel: &CancelToken,
    ) -> McpResult<()>;
    /// `startAuth(...)` → `{authorizationUrl}`. `Ok(None)` means the flow completed synchronously
    /// (client-credentials).
    async fn start_auth(
        &self,
        server: &str,
        server_url: &str,
        definition: &ServerEntry,
        cancel: &CancelToken,
    ) -> McpResult<Option<String>>;
    /// `completeAuthFromInput(server, input, opts)` → the resulting status string.
    async fn complete_auth_from_input(
        &self,
        server: &str,
        input: &str,
        cancel: &CancelToken,
    ) -> McpResult<String>;

    // --- tool-metadata.ts / ts-shape.ts -----------------------------------------------------------
    /// `formatSchema(schema, indent)`. Note [`crate::proxy::execute_describe`] passes the **default** `"  "` while
    /// [`crate::proxy::execute_search`] passes `"    "`.
    fn format_schema(&self, schema: &Value, indent: &str) -> String;
    /// `renderTsShape(schema)` — `None` is upstream's `null`, which forks to `Parameters:`.
    fn render_ts_shape(&self, schema: &Value) -> Option<String>;

    // --- tool-approval.ts ------------------------------------------------------------------------
    /// `isToolCallApprovalRequired(config, server, toolMeta, state.toolMetadata)` — the
    /// `" (requires approval)"` marker in `describe` and `search`.
    fn is_tool_call_approval_required(&self, server: &str, tool: &ToolMetadata) -> bool;
    /// `ensureToolCallApproved(state, server, toolMeta, args, signal, origin)`.
    async fn ensure_tool_call_approved(
        &self,
        server: &str,
        tool: &ToolMetadata,
        arguments: &Value,
        origin: ApprovalOrigin,
        cancel: &CancelToken,
    ) -> ApprovalOutcome;

    // --- mcp-output-guard.ts ---------------------------------------------------------------------
    /// `guardMcpOutput(content, {...resolveMcpOutputGuardOptions(settings), ...options})`.
    async fn guard_mcp_output(&self, content: Vec<Content>, options: OutputGuardOptions) -> GuardedOutput;

    // --- pi.getAllTools() ------------------------------------------------------------------------
    /// `getPiTools?.()` — `HostServices::all_tool_names()`.
    ///
    /// **`None` is not a defect**: upstream's `getPiTools` is an *optional* parameter invoked as
    /// `getPiTools?.()`, and `None` reproduces that branch exactly — skip the native-tool check and
    /// fall through to `tool_not_found` (MCP-199). Do **not** synthesise a built-in name list as a
    /// floor: that would answer `native_tool` for a built-in the session actually disabled, which pi
    /// never does.
    fn all_tool_names(&self) -> Option<Vec<String>>;
}

/// One generation's proxy-mode context — upstream's `state: McpExtensionState` parameter.
///
/// Everything the modes read is read through `state` — including the tool metadata.
///
/// **The duplicate map is gone (D0).** This context used to carry its own
/// `tool_metadata`, because `crate::state::ServerToolMetadata` was a forward declaration that could
/// not hold a `Vec<ToolMetadata>`. It can now, so the two maps are one: [`Self::with_metadata`] and
/// [`Self::with_metadata_mut`] project `McpState::tool_metadata` directly. That collapse is what
/// makes a production [`ProxyEnv`] observable — a writer reaching `McpState::tool_metadata` while
/// the modes read a private copy would leave `mcp({action:"status"})` reporting every connected
/// server as not connected.
pub struct ProxyCtx {
    /// The generation's runtime record: config, owner, UI handle, `serverInstructions`, and
    /// `state.toolMetadata: Map<string, ToolMetadata[]>` — **insertion-ordered** (MCP-170), because
    /// that order decides which server wins a fuzzy tool-name match, which disabled server is named
    /// in an error, and the output order of the unsorted regex search path.
    pub state: Arc<McpState>,
    /// The late-bound collaborators.
    pub env: Arc<dyn ProxyEnv>,
}

impl ProxyCtx {
    /// Build a context over a live state and a collaborator implementation.
    #[must_use]
    pub fn new(state: Arc<McpState>, env: Arc<dyn ProxyEnv>) -> Self {
        Self { state, env }
    }

    /// The one read path onto `state.toolMetadata`. A poisoned lock degrades to "no metadata",
    /// never to a panic (the crate denies `clippy::panic` and `init` must not fail).
    pub(crate) fn with_metadata<R>(&self, f: impl FnOnce(&IndexMap<String, Vec<ToolMetadata>>) -> R) -> R {
        match self.state.tool_metadata.lock() {
            Ok(guard) => f(&guard),
            Err(_) => f(&IndexMap::new()),
        }
    }

    /// `isToolCallApprovalRequired(state.config, server, toolMeta, state.toolMetadata)` over this
    /// context (MCP-231) — the body a production [`ProxyEnv::is_tool_call_approval_required`] has.
    ///
    /// The metadata is read **under the lock, without cloning**, because this runs once per row in
    /// `describe` and `search`.
    ///
    /// **No production caller, and that is correct.** `describe`/`search` reach approval state
    /// through the trait ([`ProxyEnv::is_tool_call_approval_required`], called at
    /// `discovery.rs:342` and `:563`) precisely so a mode test can script it; the shipped
    /// implementor is [`crate::live::RuntimeEnv`]'s `is_tool_call_approval_required`
    /// (`live.rs:1609`), which takes its own metadata lock and lands on the same
    /// [`crate::proxy::is_tool_call_approval_required`]. This method is that body written against a
    /// [`ProxyCtx`] instead — the form the crate's approval tests drive directly
    /// (`approval.rs:995`, `:1015`) — and the reference an implementor is meant to mirror.
    #[must_use]
    pub fn approval_required(&self, server: &str, tool: &ToolMetadata) -> bool {
        self.with_metadata(|metadata| {
            is_tool_call_approval_required(&self.state.config, server, tool, Some(metadata))
        })
    }

    /// `ensureToolCallApproved(state, server, toolMeta, args, signal, origin)` over this context
    /// (MCP-232) — the body a production [`ProxyEnv::ensure_tool_call_approved`] has, and the one
    /// place `state` and `state.toolMetadata` are joined for it.
    ///
    /// The metadata **is** cloned here, unlike in [`Self::approval_required`]: the gate awaits a
    /// human, and a `std::sync::MutexGuard` cannot be held across an await. The cost is one map
    /// clone per MCP tool invocation, against a dialog that may sit on screen for minutes.
    ///
    /// **Integration note:** [`crate::proxy::execute_call`] deliberately keeps calling through
    /// [`ProxyCtx::env`] rather than this method — the trait is the seam MCP-196's conformance
    /// suite scripts a denial through. This is what the production implementor forwards to.
    pub async fn ensure_tool_call_approved(
        &self,
        server: &str,
        tool: &ToolMetadata,
        args: &Value,
        origin: ApprovalOrigin,
        cancel: &CancelToken,
    ) -> ApprovalOutcome {
        let metadata = self.with_metadata(Clone::clone);
        ensure_tool_call_approved(&self.state, server, tool, args, origin, cancel, &metadata).await
    }

    /// The one write path onto `state.toolMetadata`.
    pub(crate) fn with_metadata_mut<R>(&self, f: impl FnOnce(&mut IndexMap<String, Vec<ToolMetadata>>) -> R) -> Option<R> {
        self.state.tool_metadata.lock().ok().map(|mut guard| f(&mut guard))
    }

    /// The resolved configuration this generation is running.
    pub(crate) fn config(&self) -> &McpConfig {
        &self.state.config
    }

    /// `state.config.settings`, or an all-defaults block.
    pub(crate) fn settings(&self) -> &McpSettings {
        self.state.config.settings_or_default()
    }

    /// `state.owner` — the generation's ownership token.
    pub(crate) fn owner(&self) -> &Arc<McpRuntimeOwner> {
        &self.state.owner
    }

    /// `combineAbortSignals(state.owner?.signal, signal)`.
    pub(crate) fn owned_signal(&self, cancel: &CancelToken) -> CancelToken {
        combine(&self.owner().token(), Some(cancel))
    }

    /// `state.ui` — `None` in a headless build, which is upstream's `if (state.ui)` guard.
    pub(crate) fn has_ui(&self) -> bool {
        self.state.ui.is_some()
    }

    /// `state.ui.setStatus("mcp", formatMcpStatus(config, message))`.
    ///
    /// `HostServices::set_status(key, Option<&str>)` is a keyed footer segment cleared with `None`;
    /// its default impl is a no-op, which degrades exactly the way upstream's `if (state.ui)` guard
    /// does — no gap.
    pub(crate) fn set_status(&self, message: &str) {
        let Some(ui) = self.state.ui.as_ref() else { return };
        let text = format_mcp_status(self.config(), message);
        cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
    }

    /// `getToolNames(state, serverName)` (`tool-metadata.ts:142`).
    pub(crate) fn tool_names(&self, server: &str) -> Vec<String> {
        self.with_metadata(|metadata| {
            metadata
                .get(server)
                .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    /// `state.serverInstructions.get(server)`.
    pub(crate) fn server_instructions(&self, server: &str) -> Option<String> {
        self.state.server_instructions.lock().ok().and_then(|map| map.get(server).cloned())
    }

    /// `isServerDisabled(state.config.mcpServers[server])` — **only** the literal boolean `true`
    /// disables a server, and an *unknown* server is not disabled.
    pub(crate) fn is_disabled(&self, server: &str) -> bool {
        self.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled)
    }

    /// `rankSuggestions(state, name, limit)` against this context.
    pub(crate) fn suggestions(&self, name: &str, limit: usize) -> Vec<String> {
        self.with_metadata(|metadata| rank_suggestions(self.config(), metadata, name, limit))
    }
}

// **De-duplicated at integration.** `utils.ts:339` `formatMcpStatus(config, message)` had landed
// twice: here against `&McpSettings`, and in `ui.rs` against `&McpConfig`. 13h owns the footer
// (`init.ts` `updateStatusBar` is `footer_status_text`'s only other caller), and upstream's
// parameter is `Pick<McpConfig, "settings">` — the config, not the settings — so the `ui.rs` one is
// both the owner's and the literal signature. This is its re-export.
pub use crate::ui::format_mcp_status;

/// `utils.ts:330` `formatAuthRequiredMessage(config, serverName, defaultMessage)` — a configured
/// `settings.authRequiredMessage` template wins, with `${server}` replaced everywhere.
#[must_use]
pub fn format_auth_required_message(
    settings: &McpSettings,
    server_name: &str,
    default_message: &str,
) -> String {
    match settings.auth_required_message() {
        Some(template) => template.replace("${server}", server_name),
        None => default_message.to_string(),
    }
}
