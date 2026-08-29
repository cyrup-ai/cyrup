//! Fixtures shared by more than one `proxy` submodule's tests.
//!
//! [`FakeEnv`] is the [`crate::proxy::ProxyEnv`] double MCP-196 requires.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use indexmap::{IndexMap};
use serde_json::{Map as JsonMap, Value};

use cyrup_core::{CancelToken, Content, ToolResult};

use crate::config::{
    McpConfig, ServerEntry,
    
};
use crate::errors::{McpError, McpResult};
use crate::owner::McpRuntimeOwner;
use crate::state::McpState;

// ---- fixtures --------------------------------------------------------------------------------

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::lifecycle::McpLifecycleManager;
use crate::state::{McpServerManager, McpStateParts};
use crate::proxy::call::AuthRecovery;
use crate::proxy::env::{ApprovalOrigin, ApprovalOutcome, CallToolOutcome, ConnectOutcome, ConnectionStatus, GuardedOutput, OutputGuardOptions, ProxyCallError, ProxyCtx, ProxyEnv, UrlElicitationAction};
use crate::proxy::tool_metadata::ToolMetadata;

/// A scripted [`ProxyEnv`].
///
/// MCP-196 names this as a requirement, not a convenience: the auto-auth suite "needs a
/// controllable `needs-auth` connection state and an injectable `authenticate`". Everything the
/// fake owns is one of those two knobs plus the counters the single-shot latch is asserted
/// against.
#[derive(Default)]
pub(crate) struct FakeEnv {
    pub(crate) connections: Mutex<BTreeMap<String, ConnectionStatus>>,
    pub(crate) connecting: Mutex<BTreeSet<String>>,
    pub(crate) failures: Mutex<BTreeMap<String, u64>>,
    /// Servers `lazy_connect` succeeds for; everything else fails.
    pub(crate) lazy_ok: Mutex<BTreeSet<String>>,
    /// How many times `authenticate` was invoked — the latch assertion.
    pub(crate) authenticate_calls: AtomicUsize,
    /// `authenticate` fails when set, which drives the `failed` arm of the ladder.
    pub(crate) authenticate_fails: Mutex<Option<String>>,
    /// `supportsOAuth(definition)`.
    pub(crate) oauth_servers: Mutex<BTreeSet<String>>,
    pub(crate) approval: Mutex<Option<ApprovalOutcome>>,
    pub(crate) all_tools: Mutex<Option<Vec<String>>>,
    pub(crate) approval_required: Mutex<BTreeSet<String>>,
}

impl FakeEnv {
    pub(crate) fn with_connection(self, server: &str, status: ConnectionStatus) -> Self {
        self.connections.lock().unwrap().insert(server.to_string(), status);
        self
    }
    pub(crate) fn with_connecting(self, server: &str) -> Self {
        self.connecting.lock().unwrap().insert(server.to_string());
        self
    }
    pub(crate) fn with_failure(self, server: &str, age: u64) -> Self {
        self.failures.lock().unwrap().insert(server.to_string(), age);
        self
    }
    pub(crate) fn with_oauth(self, server: &str) -> Self {
        self.oauth_servers.lock().unwrap().insert(server.to_string());
        self
    }
    pub(crate) fn with_authenticate_failure(self, message: &str) -> Self {
        *self.authenticate_fails.lock().unwrap() = Some(message.to_string());
        self
    }
    pub(crate) fn with_all_tools(self, names: &[&str]) -> Self {
        *self.all_tools.lock().unwrap() =
            Some(names.iter().map(|name| (*name).to_string()).collect());
        self
    }
    pub(crate) fn with_approval_required(self, tool: &str) -> Self {
        self.approval_required.lock().unwrap().insert(tool.to_string());
        self
    }
}

#[async_trait::async_trait]
impl ProxyEnv for FakeEnv {
    fn get_connection(&self, server: &str) -> Option<ConnectionStatus> {
        self.connections.lock().unwrap().get(server).copied()
    }
    fn is_connecting(&self, server: &str) -> bool {
        self.connecting.lock().unwrap().contains(server)
    }
    async fn connect(&self, server: &str, _cancel: &CancelToken) -> McpResult<ConnectOutcome> {
        Ok(ConnectOutcome { status: self.get_connection(server), ..ConnectOutcome::default() })
    }
    async fn reconnect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome> {
        self.connect(server, cancel).await
    }
    async fn lazy_connect(&self, server: &str, _cancel: &CancelToken) -> bool {
        self.lazy_ok.lock().unwrap().contains(server)
    }
    async fn close(&self, server: &str) {
        self.connections.lock().unwrap().remove(server);
    }
    fn touch(&self, _server: &str) {}
    fn increment_in_flight(&self, _server: &str) {}
    fn decrement_in_flight(&self, _server: &str) {}
    async fn call_tool(
        &self,
        _server: &str,
        _tool: &str,
        _arguments: JsonMap<String, Value>,
        _recovery: &AuthRecovery<'_>,
        _cancel: &CancelToken,
    ) -> Result<CallToolOutcome, ProxyCallError> {
        Ok(CallToolOutcome::default())
    }
    async fn read_resource(
        &self,
        _server: &str,
        _uri: &str,
        _recovery: &AuthRecovery<'_>,
        _cancel: &CancelToken,
    ) -> Result<Vec<Content>, ProxyCallError> {
        Ok(Vec::new())
    }
    async fn handle_url_elicitation_required(
        &self,
        _server: &str,
        _error: &rmcp::model::ErrorData,
    ) -> UrlElicitationAction {
        UrlElicitationAction::Accept
    }
    fn failure_age_seconds(&self, server: &str) -> Option<u64> {
        self.failures.lock().unwrap().get(server).copied()
    }
    fn record_failure(&self, server: &str, _message: &str) {
        self.failures.lock().unwrap().insert(server.to_string(), 0);
    }
    fn clear_failure(&self, server: &str) {
        self.failures.lock().unwrap().remove(server);
    }
    fn update_status_bar(&self) {}
    fn update_server_metadata(&self, _server: &str) {}
    fn update_metadata_cache(&self, _server: &str) {}
    fn mark_keep_alive_after_connect(&self, _server: &str) {}
    fn commit_prompt_metadata(&self, _server: &str) {}
    fn sync_tool_surface(&self) {}
    fn supports_oauth(&self, definition: &ServerEntry) -> bool {
        definition
            .url
            .as_ref()
            .is_some_and(|url| self.oauth_servers.lock().unwrap().iter().any(|s| url.contains(s)))
    }
    fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>> {
        Ok(definition.url.clone())
    }
    async fn authenticate(
        &self,
        _server: &str,
        _server_url: &str,
        _definition: &ServerEntry,
        _cancel: &CancelToken,
    ) -> McpResult<()> {
        self.authenticate_calls.fetch_add(1, Ordering::SeqCst);
        match self.authenticate_fails.lock().unwrap().clone() {
            Some(message) => Err(McpError::other(message)),
            None => Ok(()),
        }
    }
    async fn start_auth(
        &self,
        _server: &str,
        _server_url: &str,
        _definition: &ServerEntry,
        _cancel: &CancelToken,
    ) -> McpResult<Option<String>> {
        Ok(Some("https://auth.example.com/authorize".to_string()))
    }
    async fn complete_auth_from_input(
        &self,
        _server: &str,
        _input: &str,
        _cancel: &CancelToken,
    ) -> McpResult<String> {
        Ok("authenticated".to_string())
    }
    fn format_schema(&self, _schema: &Value, indent: &str) -> String {
        format!("{indent}(schema)")
    }
    fn render_ts_shape(&self, _schema: &Value) -> Option<String> {
        Some("{ a: string }".to_string())
    }
    fn is_tool_call_approval_required(&self, _server: &str, tool: &ToolMetadata) -> bool {
        self.approval_required.lock().unwrap().contains(&tool.name)
    }
    async fn ensure_tool_call_approved(
        &self,
        _server: &str,
        _tool: &ToolMetadata,
        _arguments: &Value,
        _origin: ApprovalOrigin,
        _cancel: &CancelToken,
    ) -> ApprovalOutcome {
        self.approval.lock().unwrap().unwrap_or(ApprovalOutcome::Approved)
    }
    async fn guard_mcp_output(
        &self,
        content: Vec<Content>,
        _options: OutputGuardOptions,
    ) -> GuardedOutput {
        GuardedOutput { content, ..GuardedOutput::default() }
    }
    fn all_tool_names(&self) -> Option<Vec<String>> {
        self.all_tools.lock().unwrap().clone()
    }
}

/// A context over a real [`McpState`] and a scripted [`FakeEnv`].
pub(crate) fn ctx_with(
    config: McpConfig,
    metadata: &[(&str, Vec<ToolMetadata>)],
    instructions: &[(&str, &str)],
    env: FakeEnv,
) -> (Arc<ProxyCtx>, Arc<FakeEnv>) {
    let manager = Arc::new(McpServerManager::default());
    let lifecycle =
        Arc::new(McpLifecycleManager::new(Arc::clone(&manager), Arc::new(|_: &str| false)));
    let state = Arc::new(McpState::new(McpStateParts {
        owner: Arc::new(McpRuntimeOwner::new()),
        manager,
        lifecycle,
        config,
        programmatic_config: None,
        oauth_runtime: crate::oauth::create_oauth_runtime(None),
        auth_storage_options: crate::state::AuthStorageOptions::default(),
        ui: None,
        open_browser: Arc::new(|_| Box::pin(async { Ok(()) })),
        send_message: Arc::new(|_| {}),
    }));
    {
        let mut slot = state.server_instructions.lock().unwrap();
        for (server, text) in instructions {
            slot.insert((*server).to_string(), (*text).to_string());
        }
    }
    let env = Arc::new(env);
    let ctx = Arc::new(ProxyCtx::new(state, Arc::clone(&env) as Arc<dyn ProxyEnv>));
    {
        let mut slot = ctx.state.tool_metadata.lock().unwrap();
        for (server, tools) in metadata {
            slot.insert((*server).to_string(), tools.clone());
        }
    }
    (ctx, env)
}

pub(crate) fn text_of(result: &ToolResult) -> String {
    match result.content.first() {
        Some(Content::Text { text, .. }) => text.to_string(),
        other => panic!("expected text content, got {other:?}"),
    }
}

pub(crate) fn stdio(command: &str) -> ServerEntry {
    ServerEntry { command: Some(command.to_string()), ..ServerEntry::default() }
}

pub(crate) fn http(url: &str) -> ServerEntry {
    ServerEntry { url: Some(url.to_string()), ..ServerEntry::default() }
}


/// `__tests__/search-ranking.test.ts`'s `tool(name, description)` helper.
pub(crate) fn tool(name: &str, description: &str) -> ToolMetadata {
    ToolMetadata::new(name, name, description)
}

/// `definition(searchKeywords)` — a `command` server carrying only a keyword map.
pub(crate) fn definition_with_keywords(pairs: &[(&str, &[&str])]) -> ServerEntry {
    // `IndexMap`, matching the field: insertion order is what upstream's `Object.entries`
    // walk preserves, and this helper feeds the glob-union ordering assertions below.
    let mut map: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
    for (key, values) in pairs {
        map.insert((*key).to_string(), values.iter().map(|v| (*v).to_string()).collect());
    }
    ServerEntry {
        command: Some("npx".to_string()),
        search_keywords: Some(map),
        ..ServerEntry::default()
    }
}

pub(crate) fn config_with(servers: &[(&str, ServerEntry)]) -> McpConfig {
    let mut mcp_servers = IndexMap::new();
    for (name, entry) in servers {
        mcp_servers.insert((*name).to_string(), entry.clone());
    }
    McpConfig { mcp_servers, settings: None, imports: Vec::new() }
}

pub(crate) fn metadata_with(servers: &[(&str, Vec<ToolMetadata>)]) -> IndexMap<String, Vec<ToolMetadata>> {
    let mut map = IndexMap::new();
    for (name, tools) in servers {
        map.insert((*name).to_string(), tools.clone());
    }
    map
}
