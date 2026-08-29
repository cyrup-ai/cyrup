//! `executeCall` — the resolution state machine (MCP-163, **critical**) and the
//! invocation half (MCP-164, MCP-165).
//!
//! See [`crate::proxy`] for the module overview.

use std::sync::Arc;

use serde_json::{Map as JsonMap, Value};

use cyrup_core::{CancelToken, Content, ToolResult};

use crate::abort::{is_abort_error, throw_if_aborted};
use crate::config::{
    ServerEntry,
    ToolPrefix,
};
use crate::errors::McpResult;
use crate::proxy::auth::{AutoAuthResult, attempt_auto_auth};
use crate::proxy::constants::MCP_TOOL_NAME;
use crate::proxy::env::{ApprovalOrigin, ApprovalOutcome, ConnectionStatus, OutputGuardOptions, ProxyCallError, ProxyCtx, UrlElicitationAction};
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::results::{SingleMatch, ambiguous_tool_result, details, details_err, get_auth_required_message, get_enabled_tool_matches, get_single_tool_match, get_tool_matches, text_result};
use crate::proxy::tool_metadata::{ToolMetadata, find_tool_by_name, server_prefix};

// ==================================================================================================
// 12 · `executeCall` — the resolution state machine (MCP-163, **critical**) and the invocation
//      half (MCP-164, MCP-165)
// ==================================================================================================

/// The function-scoped `autoAuthAttempted` boolean of `proxy-modes.ts:817`.
///
/// **At most one auto-auth per [`execute_call`] invocation, across all five call sites** — phase 3,
/// phase 4, phase 6's pre-connect and post-connect checks, and `recoverAuthConnection`, which fires
/// from inside `withSessionRecovery` *after* the request started. The latch is the defence against a
/// browser-flow loop on a misconfigured OAuth server: get it wrong and a single tool call opens one
/// browser tab per resolution attempt.
///
/// Shared (rather than a plain local `bool`) for exactly one reason: [`AuthRecovery`] must read and
/// set the *same* latch from inside the env's session-recovery loop.
#[derive(Debug, Default, Clone)]
pub struct AutoAuthLatch(Arc<std::sync::atomic::AtomicBool>);

impl AutoAuthLatch {
    /// A fresh, unlatched instance — one per `executeCall`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `if (!autoAuthAttempted) { autoAuthAttempted = true; … }` — `true` when *this* caller won the
    /// latch and may run the ladder.
    #[must_use]
    pub fn claim(&self) -> bool {
        !self.0.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// The value reported as `details.autoAuthAttempted` on the `auth_required` catch arm.
    #[must_use]
    pub fn attempted(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// `proxy-modes.ts:1152` `recoverAuthConnection` — the `onNeedsAuth` callback handed to
/// `withSessionRecovery`.
///
/// Lives here, not in the env, because it reuses [`AutoAuthLatch`] and because its ladder is the
/// port. The env calls [`AuthRecovery::recover`] from inside its recovery loop and translates the
/// answer into "retry the request" or "give up".
pub struct AuthRecovery<'a> {
    ctx: &'a ProxyCtx,
    server: String,
    latch: AutoAuthLatch,
    cancel: CancelToken,
}

impl AuthRecovery<'_> {
    /// The recovery ladder: an already-connected connection short-circuits; otherwise a single
    /// auto-auth attempt, then close-if-still-needs-auth, `clearFailure`, reconnect.
    ///
    /// A **failed** auto-auth raises [`ProxyCallError::SessionRecoveryAuthRequired`] carrying the
    /// message, which is upstream's `throw new SessionRecoveryAuthRequiredError(server, message)`.
    pub async fn recover(&self) -> Result<Option<ConnectionStatus>, ProxyCallError> {
        let current = self.ctx.env.get_connection(&self.server);
        if current == Some(ConnectionStatus::Connected) {
            return Ok(current);
        }
        if self.latch.claim() {
            let attempt = attempt_auto_auth(self.ctx, &self.server, &self.cancel)
                .await
                .map_err(ProxyCallError::Other)?;
            match attempt {
                AutoAuthResult::Failed(message) => {
                    return Err(ProxyCallError::SessionRecoveryAuthRequired {
                        server: self.server.clone(),
                        auth_message: Some(message),
                    });
                }
                AutoAuthResult::Success => {
                    if !self.ctx.config().mcp_servers.contains_key(&self.server) {
                        return Ok(None);
                    }
                    let after = self.ctx.env.get_connection(&self.server);
                    if after == Some(ConnectionStatus::Connected) {
                        return Ok(after);
                    }
                    if after == Some(ConnectionStatus::NeedsAuth) {
                        self.ctx.env.close(&self.server).await;
                    }
                    self.ctx.env.clear_failure(&self.server);
                    let outcome = self
                        .ctx
                        .env
                        .connect(&self.server, &self.cancel)
                        .await
                        .map_err(ProxyCallError::Other)?;
                    return Ok(outcome.status);
                }
                AutoAuthResult::Skipped => {}
            }
        }
        Ok(self.ctx.env.get_connection(&self.server))
    }
}

/// `{server, resourceUri}` for a resource tool, `{server, tool: originalName}` otherwise.
///
/// **Fixed once in phase 6 and reused by every subsequent result** — that is why a reconnect which
/// re-resolves the tool still reports the identity computed before it.
fn call_identity(server: &str, tool: &ToolMetadata) -> Vec<(String, Value)> {
    match tool.resource_uri.as_ref() {
        Some(uri) => vec![
            ("server".to_string(), Value::String(server.to_string())),
            ("resourceUri".to_string(), Value::String(uri.clone())),
        ],
        None => vec![
            ("server".to_string(), Value::String(server.to_string())),
            ("tool".to_string(), Value::String(tool.original_name.clone())),
        ],
    }
}

/// Splice a `callIdentity` into a `details` map at the point the JS spread would land.
fn spread(map: &mut JsonMap<String, Value>, identity: &[(String, Value)]) {
    for (key, value) in identity {
        map.insert(key.clone(), value.clone());
    }
}

/// `proxy-modes.ts:833` `disabledCallResult(disabledServer, metadata?)`.
///
/// The **disabled check happens after resolution** so the error can name the resolved tool: a
/// resource tool reports `{server, resourceUri}`, a normal tool `{server, tool: originalName}`, and
/// an unresolved name falls back to `{server, requestedTool}`.
fn disabled_call_result(disabled_server: &str, tool_name: &str, metadata: Option<&ToolMetadata>) -> ToolResult {
    let message = format!(
        "Server \"{disabled_server}\" is disabled. Run /mcp enable {disabled_server} and /reload to enable it."
    );
    let mut map = details_err("call", McpErrorCode::ServerDisabled);
    match metadata {
        None => {
            map.insert("server".to_string(), Value::String(disabled_server.to_string()));
            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
        }
        Some(tool) => spread(&mut map, &call_identity(disabled_server, tool)),
    }
    map.insert("message".to_string(), Value::String(message.clone()));
    text_result(message, map)
}

/// The `Did you mean: …` suffix, empty when there is nothing to suggest.
fn suggestion_text(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        String::new()
    } else {
        format!(" Did you mean: {}", suggestions.join(", "))
    }
}

/// `proxy-modes.ts:806` `executeCall(state, toolName, args?, serverOverride?, getPiTools?, signal?,
/// origin?)`.
///
/// Eight phases; see the module docs for why this is the section's only `critical`. The short
/// version: [`get_single_tool_match`] and [`get_enabled_tool_matches`] exist to **refuse rather than
/// guess**, and a port that resolves ambiguity by first-match sends `create_issue` to whichever
/// server happens to be first in the map — a silently wrong tool call against a live external
/// system, on a normal path.
#[allow(clippy::too_many_lines)]
pub async fn execute_call(
    ctx: &ProxyCtx,
    tool_name: &str,
    args: Option<&Value>,
    server_override: Option<&str>,
    cancel: &CancelToken,
    origin: Option<ApprovalOrigin>,
) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(&owned, ctx.owner().stop_reason().as_deref().map(String::as_str))?;

    let mut server_name: Option<String> = server_override.map(str::to_string);
    let mut tool_meta: Option<ToolMetadata> = None;
    let latch = AutoAuthLatch::new();
    let prefix_mode = ctx.config().tool_prefix();

    // ---- Phase 1 — a server hint was given ------------------------------------------------------
    if let Some(hint) = server_name.clone() {
        if !ctx.config().mcp_servers.contains_key(&hint) {
            let mut map = details_err("call", McpErrorCode::ServerNotFound);
            map.insert("server".to_string(), Value::String(hint.clone()));
            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
            return Ok(text_result(
                format!("Server \"{hint}\" not found. Use mcp({{}}) to see available servers."),
                map,
            ));
        }
        let matched = ctx.with_metadata(|metadata| get_single_tool_match(metadata.get(&hint), tool_name));
        match matched {
            SingleMatch::Ambiguous => return Ok(ambiguous_tool_result("call", tool_name)),
            SingleMatch::One(found) => tool_meta = Some(found),
            SingleMatch::None => {}
        }
        // The disabled check runs AFTER resolution so the error can name the resolved tool.
        if ctx.is_disabled(&hint) {
            return Ok(disabled_call_result(&hint, tool_name, tool_meta.as_ref()));
        }
    } else {
        // ---- Phase 2 — no hint: the ambiguity gate, then two ordered scans -----------------------
        let gate = ctx.with_metadata(|metadata| {
            let exact = get_enabled_tool_matches(ctx.config(), metadata, tool_name, true);
            if exact.len() > 1 {
                return Err(());
            }
            if exact.is_empty()
                && get_enabled_tool_matches(ctx.config(), metadata, tool_name, false).len() > 1
            {
                return Err(());
            }
            Ok(())
        });
        if gate.is_err() {
            return Ok(ambiguous_tool_result("call", tool_name));
        }

        let scan = ctx.with_metadata(|metadata| {
            let mut disabled_match: Option<(String, ToolMetadata)> = None;
            let mut resolved: Option<(String, ToolMetadata)> = None;
            // Scan A — **exact name only**.
            for (server, tools) in metadata {
                let Some(found) = tools.iter().find(|tool| tool.name == tool_name) else { continue };
                if ctx.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
                    if disabled_match.is_none() {
                        disabled_match = Some((server.clone(), found.clone()));
                    }
                    continue;
                }
                resolved = Some((server.clone(), found.clone()));
                break;
            }
            // Scan B — the fuzzy pass, **guarded by `!toolMeta && !disabledMatch`**. An exact match
            // on a *disabled* server suppresses the fuzzy pass entirely, so a fuzzy-matching enabled
            // server is never reached. Faithful, and deliberately not "fixed".
            if resolved.is_none() && disabled_match.is_none() {
                for (server, tools) in metadata {
                    let Some(found) = find_tool_by_name(tools, tool_name) else { continue };
                    if ctx.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
                        if disabled_match.is_none() {
                            disabled_match = Some((server.clone(), found.clone()));
                        }
                        continue;
                    }
                    resolved = Some((server.clone(), found.clone()));
                    break;
                }
            }
            (resolved, disabled_match)
        });
        match scan {
            (Some((server, found)), _) => {
                server_name = Some(server);
                tool_meta = Some(found);
            }
            (None, Some((disabled, found))) => {
                return Ok(disabled_call_result(&disabled, tool_name, Some(&found)));
            }
            (None, None) => {}
        }
    }

    // ---- Phase 3 — hinted server, tool still unknown: lazy connect + the auto-auth ladder --------
    if let (Some(hint), None) = (server_name.clone(), tool_meta.clone()) {
        let connected = ctx.env.lazy_connect(&hint, &owned).await;
        if connected {
            match ctx.with_metadata(|metadata| get_single_tool_match(metadata.get(&hint), tool_name)) {
                SingleMatch::Ambiguous => return Ok(ambiguous_tool_result("call", tool_name)),
                SingleMatch::One(found) => tool_meta = Some(found),
                SingleMatch::None => {}
            }
        } else {
            if ctx.env.get_connection(&hint) == Some(ConnectionStatus::NeedsAuth) {
                if latch.claim() {
                    match attempt_auto_auth(ctx, &hint, &owned).await? {
                        AutoAuthResult::Failed(message) => {
                            let mut map = details_err("call", McpErrorCode::AuthRequired);
                            map.insert("server".to_string(), Value::String(hint.clone()));
                            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                            map.insert("message".to_string(), Value::String(message.clone()));
                            return Ok(text_result(message, map));
                        }
                        AutoAuthResult::Success => {
                            ctx.env.close(&hint).await;
                            ctx.env.clear_failure(&hint);
                            if ctx.env.lazy_connect(&hint, &owned).await {
                                match ctx.with_metadata(|metadata| {
                                    get_single_tool_match(metadata.get(&hint), tool_name)
                                }) {
                                    SingleMatch::Ambiguous => {
                                        return Ok(ambiguous_tool_result("call", tool_name));
                                    }
                                    SingleMatch::One(found) => tool_meta = Some(found),
                                    SingleMatch::None => {
                                        let suggestions = ctx.suggestions(tool_name, 5);
                                        let mut map = details_err(
                                            "call",
                                            McpErrorCode::ToolNotFoundAfterReconnect,
                                        );
                                        map.insert("server".to_string(), Value::String(hint.clone()));
                                        map.insert(
                                            "requestedTool".to_string(),
                                            Value::String(tool_name.to_string()),
                                        );
                                        map.insert(
                                            "suggestions".to_string(),
                                            Value::Array(
                                                suggestions
                                                    .iter()
                                                    .map(|name| Value::String(name.clone()))
                                                    .collect(),
                                            ),
                                        );
                                        return Ok(text_result(
                                            format!(
                                                "Tool \"{tool_name}\" not found on \"{hint}\" after reconnect.{}",
                                                suggestion_text(&suggestions)
                                            ),
                                            map,
                                        ));
                                    }
                                }
                            }
                        }
                        AutoAuthResult::Skipped => {}
                    }
                }

                if tool_meta.is_none()
                    && ctx.env.get_connection(&hint) == Some(ConnectionStatus::NeedsAuth)
                {
                    let message = get_auth_required_message(ctx.settings(), &hint);
                    let mut map = details_err("call", McpErrorCode::AuthRequired);
                    map.insert("server".to_string(), Value::String(hint.clone()));
                    map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                    map.insert("message".to_string(), Value::String(message.clone()));
                    return Ok(text_result(message, map));
                }
            }

            if tool_meta.is_none()
                && let Some(failed_ago) = ctx.env.failure_age_seconds(&hint)
            {
                let mut map = details_err("call", McpErrorCode::ServerBackoff);
                map.insert("server".to_string(), Value::String(hint.clone()));
                map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                return Ok(text_result(
                    format!("Server \"{hint}\" not available (last failed {failed_ago}s ago)"),
                    map,
                ));
            }
        }
    }

    // ---- Phase 4 — lazy prefix discovery ---------------------------------------------------------
    // Only when there is **no** server and **no** tool and the prefix mode is not `none`.
    let mut prefix_matched_server: Option<String> = None;
    if server_name.is_none() && tool_meta.is_none() && prefix_mode != ToolPrefix::None {
        let mut candidates: Vec<(String, String)> = ctx
            .config()
            .mcp_servers
            .keys()
            .filter(|name| !ctx.is_disabled(name))
            .map(|name| (name.clone(), server_prefix(name, prefix_mode)))
            .filter(|(_, prefix)| !prefix.is_empty() && tool_name.starts_with(&format!("{prefix}_")))
            .collect();
        // Descending prefix length: with servers `foo` and `foo-bar`, `foo-bar_x` must resolve
        // against `foo-bar`, not `foo`.
        candidates.sort_by_key(|(_, prefix)| std::cmp::Reverse(prefix.len()));

        let mut lazy_exact: Vec<(String, ToolMetadata)> = Vec::new();
        let mut lazy_fallback: Vec<(String, ToolMetadata)> = Vec::new();
        for (candidate, _) in candidates {
            let existing = ctx.env.get_connection(&candidate);
            // Skip a server in failure backoff **unless** it is `needs-auth` — an auth wall is not
            // a transport failure and must stay reachable.
            if ctx.env.failure_age_seconds(&candidate).is_some()
                && existing != Some(ConnectionStatus::NeedsAuth)
            {
                continue;
            }

            let mut connected = ctx.env.lazy_connect(&candidate, &owned).await;
            if !connected
                && ctx.env.get_connection(&candidate) == Some(ConnectionStatus::NeedsAuth)
                && latch.claim()
            {
                match attempt_auto_auth(ctx, &candidate, &owned).await? {
                    AutoAuthResult::Failed(message) => {
                        let mut map = details_err("call", McpErrorCode::AuthRequired);
                        map.insert("server".to_string(), Value::String(candidate.clone()));
                        map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                        map.insert("message".to_string(), Value::String(message.clone()));
                        return Ok(text_result(message, map));
                    }
                    AutoAuthResult::Success => {
                        ctx.env.close(&candidate).await;
                        ctx.env.clear_failure(&candidate);
                        connected = ctx.env.lazy_connect(&candidate, &owned).await;
                    }
                    AutoAuthResult::Skipped => {}
                }
            }
            if !connected {
                continue;
            }
            if prefix_matched_server.is_none() {
                prefix_matched_server = Some(candidate.clone());
            }

            let per_server = ctx.with_metadata(|metadata| {
                let tools = metadata.get(&candidate).cloned().unwrap_or_default();
                let exact: Vec<ToolMetadata> =
                    get_tool_matches(&tools, tool_name, true).into_iter().cloned().collect();
                if exact.len() > 1 {
                    return Err(());
                }
                if let Some(single) = exact.into_iter().next() {
                    return Ok(Some((true, single)));
                }
                let fallback: Vec<ToolMetadata> =
                    get_tool_matches(&tools, tool_name, false).into_iter().cloned().collect();
                if fallback.len() > 1 {
                    return Err(());
                }
                Ok(fallback.into_iter().next().map(|single| (false, single)))
            });
            match per_server {
                Err(()) => return Ok(ambiguous_tool_result("call", tool_name)),
                Ok(Some((true, found))) => lazy_exact.push((candidate.clone(), found)),
                Ok(Some((false, found))) => lazy_fallback.push((candidate.clone(), found)),
                Ok(None) => {}
            }
        }

        // Exacts win if any; `>1` in the winning set is ambiguous; exactly one is adopted.
        let lazy_matches = if lazy_exact.is_empty() { lazy_fallback } else { lazy_exact };
        if lazy_matches.len() > 1 {
            return Ok(ambiguous_tool_result("call", tool_name));
        }
        if let Some((server, found)) = lazy_matches.into_iter().next() {
            server_name = Some(server);
            tool_meta = Some(found);
        }
    }

    // ---- Phase 5 — unresolved --------------------------------------------------------------------
    let (Some(server_name), Some(mut tool_meta)) = (server_name.clone(), tool_meta.clone()) else {
        // `getPiTools` is an OPTIONAL callback upstream, invoked as `getPiTools?.()`.
        // `all_tool_names() == None` — "no live session backend attached" — is exactly that branch:
        // skip the native-tool check and fall through to `tool_not_found` (MCP-199).
        let native = if server_override.is_none() {
            ctx.env.all_tool_names().and_then(|names| {
                names.into_iter().find(|name| name == tool_name && name != MCP_TOOL_NAME)
            })
        } else {
            None
        };
        if let Some(native) = native {
            let mut map = details_err("call", McpErrorCode::NativeTool);
            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
            // MCP-163 naming decision: `Pi` → `cyrup`. The only text this port changes for a reason
            // other than the scope cuts.
            return Ok(text_result(
                format!("\"{native}\" is a native cyrup tool. Call {native} directly instead of using mcp({{ tool: \"{native}\" }})."),
                map,
            ));
        }

        let hint_server = server_name.clone().or_else(|| prefix_matched_server.clone());
        let available = hint_server.as_ref().map(|server| ctx.tool_names(server)).unwrap_or_default();
        let mut message = format!("Tool \"{tool_name}\" not found.");
        if available.is_empty() {
            message.push_str(" Use mcp({ search: \"...\" }) to search.");
        } else {
            message.push_str(&format!(
                " Server \"{}\" has: {}",
                hint_server.clone().unwrap_or_default(),
                available.join(", ")
            ));
        }
        let suggestions = ctx.suggestions(tool_name, 5);
        message.push_str(&suggestion_text(&suggestions));

        let mut map = details_err("call", McpErrorCode::ToolNotFound);
        map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
        map.insert(
            "hintServer".to_string(),
            hint_server.map_or(Value::Null, Value::String),
        );
        map.insert(
            "suggestions".to_string(),
            Value::Array(suggestions.iter().map(|name| Value::String(name.clone())).collect()),
        );
        return Ok(text_result(message, map));
    };

    // ---- Phase 6 — connection readiness -----------------------------------------------------------
    // `callIdentity` is fixed HERE and reused by every subsequent result — including after a
    // reconnect re-resolves `toolMeta` to a different record.
    let identity = call_identity(&server_name, &tool_meta);

    let mut connection = ctx.env.get_connection(&server_name);
    if connection == Some(ConnectionStatus::NeedsAuth) {
        if latch.claim() {
            match attempt_auto_auth(ctx, &server_name, &owned).await? {
                AutoAuthResult::Failed(message) => {
                    let mut map = details_err("call", McpErrorCode::AuthRequired);
                    spread(&mut map, &identity);
                    map.insert("message".to_string(), Value::String(message.clone()));
                    return Ok(text_result(message, map));
                }
                AutoAuthResult::Success => {
                    ctx.env.close(&server_name).await;
                    ctx.env.clear_failure(&server_name);
                    connection = ctx.env.get_connection(&server_name);
                }
                AutoAuthResult::Skipped => {}
            }
        }
        if connection == Some(ConnectionStatus::NeedsAuth) {
            let message = get_auth_required_message(ctx.settings(), &server_name);
            let mut map = details_err("call", McpErrorCode::AuthRequired);
            spread(&mut map, &identity);
            map.insert("message".to_string(), Value::String(message.clone()));
            return Ok(text_result(message, map));
        }
    }

    if connection != Some(ConnectionStatus::Connected) {
        if let Some(failed_ago) = ctx.env.failure_age_seconds(&server_name) {
            let mut map = details_err("call", McpErrorCode::ServerBackoff);
            spread(&mut map, &identity);
            return Ok(text_result(
                format!("Server \"{server_name}\" not available (last failed {failed_ago}s ago)"),
                map,
            ));
        }
        if !ctx.config().mcp_servers.contains_key(&server_name) {
            let mut map = details_err("call", McpErrorCode::ServerNotConnected);
            spread(&mut map, &identity);
            return Ok(text_result(format!("Server \"{server_name}\" not connected"), map));
        }

        let reconnected: McpResult<Option<ToolResult>> = async {
            ctx.set_status(&format!("connecting to {server_name}..."));
            let mut outcome = ctx.env.connect(&server_name, &owned).await?;
            if outcome.needs_auth() {
                if latch.claim() {
                    match attempt_auto_auth(ctx, &server_name, &owned).await? {
                        AutoAuthResult::Failed(message) => {
                            let mut map = details_err("call", McpErrorCode::AuthRequired);
                            spread(&mut map, &identity);
                            map.insert("message".to_string(), Value::String(message.clone()));
                            return Ok(Some(text_result(message, map)));
                        }
                        AutoAuthResult::Success => {
                            ctx.env.close(&server_name).await;
                            outcome = ctx.env.connect(&server_name, &owned).await?;
                        }
                        AutoAuthResult::Skipped => {}
                    }
                }
                if outcome.needs_auth() {
                    let message = get_auth_required_message(ctx.settings(), &server_name);
                    let mut map = details_err("call", McpErrorCode::AuthRequired);
                    spread(&mut map, &identity);
                    map.insert("message".to_string(), Value::String(message.clone()));
                    return Ok(Some(text_result(message, map)));
                }
            }
            ctx.env.clear_failure(&server_name);
            ctx.env.update_server_metadata(&server_name);
            ctx.env.update_metadata_cache(&server_name);
            ctx.state.notify_tool_metadata_updated(&server_name, "proxy-call-reconnect");
            ctx.env.mark_keep_alive_after_connect(&server_name);
            ctx.env.update_status_bar();
            Ok(None)
        }
        .await;

        match reconnected {
            Ok(Some(early)) => return Ok(early),
            Ok(None) => {}
            Err(error) => {
                // As in [`execute_connect`]: an abort is reported, not rethrown, and never recorded
                // as a connect failure.
                let message = error.to_string();
                let aborted = is_abort_error(&error, Some(&owned));
                if !aborted {
                    ctx.env.record_failure(&server_name, &message);
                }
                ctx.env.update_status_bar();
                let code = if aborted { McpErrorCode::Aborted } else { McpErrorCode::ConnectFailed };
                let mut map = details_err("call", code);
                spread(&mut map, &identity);
                map.insert("message".to_string(), Value::String(message.clone()));
                return Ok(text_result(format!("Failed to connect to \"{server_name}\": {message}"), map));
            }
        }

        // Re-resolve after the handshake.
        match ctx.with_metadata(|metadata| get_single_tool_match(metadata.get(&server_name), tool_name)) {
            SingleMatch::Ambiguous => return Ok(ambiguous_tool_result("call", tool_name)),
            SingleMatch::One(found) => tool_meta = found,
            SingleMatch::None => {
                let available = ctx.tool_names(&server_name);
                let hint = if available.is_empty() {
                    format!("Server \"{server_name}\" has no tools.")
                } else {
                    format!("Available tools on \"{server_name}\": {}", available.join(", "))
                };
                let suggestions = ctx.suggestions(tool_name, 5);
                let mut map = details_err("call", McpErrorCode::ToolNotFoundAfterReconnect);
                map.insert("server".to_string(), Value::String(server_name.clone()));
                map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                map.insert(
                    "suggestions".to_string(),
                    Value::Array(suggestions.iter().map(|name| Value::String(name.clone())).collect()),
                );
                return Ok(text_result(
                    format!(
                        "Tool \"{tool_name}\" not found on \"{server_name}\" after reconnect. {hint}{}",
                        suggestion_text(&suggestions)
                    ),
                    map,
                ));
            }
        }
    }

    // ---- Phase 7 — post-connect disabled recheck --------------------------------------------------
    // The definition may have been swapped under a live connection.
    if ctx.is_disabled(&server_name) {
        return Ok(disabled_call_result(&server_name, tool_name, Some(&tool_meta)));
    }

    // ---- Phase 8 — approval -----------------------------------------------------------------------
    // `toolMeta.resourceUri ? (args ?? {}) : normalizeToolArguments(args)`. Both arms are the same
    // value here: `normalizeToolArguments`' JSON round-trip exists to strip non-serializable
    // JavaScript values, and `parse_args` has already produced a plain JSON object.
    let normalized_args = args.cloned().unwrap_or_else(|| Value::Object(JsonMap::new()));
    let resolved_origin =
        origin.unwrap_or_else(|| ApprovalOrigin::for_proxy_call(tool_meta.resource_uri.as_ref()));
    match ctx
        .env
        .ensure_tool_call_approved(&server_name, &tool_meta, &normalized_args, resolved_origin, &owned)
        .await
    {
        ApprovalOutcome::Approved => {}
        outcome => {
            let denied = outcome == ApprovalOutcome::Denied;
            let message = if denied {
                format!(
                    "The user declined approval to run MCP tool \"{}\" on server \"{server_name}\".",
                    tool_meta.original_name
                )
            } else {
                format!(
                    "MCP tool \"{}\" on server \"{server_name}\" is approval-gated and requires an interactive session.",
                    tool_meta.original_name
                )
            };
            let code =
                if denied { McpErrorCode::ApprovalDenied } else { McpErrorCode::ApprovalRequired };
            let mut map = details_err("call", code);
            // NOT `callIdentity`: a resource tool reports `tool` here rather than `resourceUri`.
            map.insert("server".to_string(), Value::String(server_name.clone()));
            map.insert("tool".to_string(), Value::String(tool_meta.original_name.clone()));
            return Ok(text_result(message, map));
        }
    }

    // ---- Invocation ------------------------------------------------------------------------------
    let schema_suffix = tool_meta
        .input_schema
        .as_ref()
        .map(|schema| format!("\n\nExpected parameters:\n{}", ctx.env.format_schema(schema, "  ")))
        .unwrap_or_default();
    let recovery =
        AuthRecovery { ctx, server: server_name.clone(), latch: latch.clone(), cancel: owned.clone() };

    // try { touch; incrementInFlight; … } finally { decrementInFlight; touch }
    ctx.env.touch(&server_name);
    ctx.env.increment_in_flight(&server_name);
    let outcome = invoke(
        ctx,
        &server_name,
        &tool_meta,
        &identity,
        &normalized_args,
        &schema_suffix,
        &recovery,
        &latch,
        &owned,
    )
    .await;
    ctx.env.decrement_in_flight(&server_name);
    ctx.env.touch(&server_name);
    outcome
}

/// The body of [`execute_call`]'s `try` — the three result paths and the three catch arms.
///
/// **Three result paths** after the MCP Apps cut (upstream had four; the UI-enabled-tool path is
/// gone): resource read, tool error, tool success. The spread order differs between them and is
/// reproduced: paths 1 and 2 put `callIdentity` before the guard keys, path 3 after.
#[allow(clippy::too_many_arguments)]
async fn invoke(
    ctx: &ProxyCtx,
    server_name: &str,
    tool_meta: &ToolMetadata,
    identity: &[(String, Value)],
    normalized_args: &Value,
    schema_suffix: &str,
    recovery: &AuthRecovery<'_>,
    latch: &AutoAuthLatch,
    owned: &CancelToken,
) -> McpResult<ToolResult> {
    // Path 1 — a resource tool. Note the read is NOT wrapped in `abortable`: upstream's asymmetry,
    // reproduced rather than "fixed" (13d §10). Cancellation reaches it only through the request
    // options carried by the env.
    if let Some(uri) = tool_meta.resource_uri.as_ref() {
        return match ctx.env.read_resource(server_name, uri, recovery, owned).await {
            Ok(content) => {
                let content = if content.is_empty() {
                    vec![Content::Text { text: "(empty resource)".into(), text_signature: None }]
                } else {
                    content
                };
                let guarded = ctx.env.guard_mcp_output(content, OutputGuardOptions::default()).await;
                let mut map = details("call");
                spread(&mut map, identity);
                guarded.write_details(&mut map);
                Ok(ToolResult {
                    content: guarded.content,
                    details: Some(Value::Object(map)),
                    ..Default::default()
                })
            }
            Err(error) => {
                catch_arm(ctx, server_name, identity, schema_suffix, latch, owned, error).await
            }
        };
    }

    let arguments = normalized_args.as_object().cloned().unwrap_or_default();
    match ctx.env.call_tool(server_name, &tool_meta.original_name, arguments, recovery, owned).await {
        Err(error) => catch_arm(ctx, server_name, identity, schema_suffix, latch, owned, error).await,
        // Path 2 — the server returned `isError: true`.
        Ok(result) if result.is_error => {
            let content = if result.content.is_empty() {
                vec![Content::Text { text: "(empty result)".into(), text_signature: None }]
            } else {
                result.content
            };
            let guarded = ctx
                .env
                .guard_mcp_output(
                    content,
                    OutputGuardOptions {
                        prefix: "Error: ".to_string(),
                        suffix: schema_suffix.to_string(),
                        empty_text_fallback: Some("Tool execution failed".to_string()),
                        raw_mcp_result: result.raw,
                    },
                )
                .await;
            let mut map = details_err("call", McpErrorCode::ToolError);
            spread(&mut map, identity);
            guarded.write_details(&mut map);
            Ok(ToolResult {
                content: guarded.content,
                details: Some(Value::Object(map)),
                ..Default::default()
            })
        }
        // Path 3 — success. `callIdentity` is spread AFTER the guard keys here.
        Ok(result) => {
            let content = if result.content.is_empty() {
                vec![Content::Text { text: "(empty result)".into(), text_signature: None }]
            } else {
                result.content
            };
            let guarded = ctx
                .env
                .guard_mcp_output(
                    content,
                    OutputGuardOptions { raw_mcp_result: result.raw, ..Default::default() },
                )
                .await;
            let mut map = details("call");
            guarded.write_details(&mut map);
            spread(&mut map, identity);
            Ok(ToolResult {
                content: guarded.content,
                details: Some(Value::Object(map)),
                ..Default::default()
            })
        }
    }
}

/// `executeCall`'s catch block, in order (MCP-165).
///
/// 1. `SessionRecoveryAuthRequiredError` ⇒ `auth_required`, with `details.autoAuthAttempted`;
/// 2. `UrlElicitationRequiredError` ⇒ `url_elicitation_required` with the three action-specific
///    messages;
/// 3. anything else ⇒ the guard is applied to the *message* with `prefix: "Failed to call tool: "`,
///    and `details.message` becomes the literal `output truncated; see outputGuard.fullOutputPath`
///    when the guard spilled.
///
/// Upstream additionally fired `uiSession?.sendToolCancelled(message)` on all three arms; that goes
/// with Cut 2. The `finally` still does `decrementInFlight` + `touch` — see [`execute_call`].
async fn catch_arm(
    ctx: &ProxyCtx,
    server_name: &str,
    identity: &[(String, Value)],
    schema_suffix: &str,
    latch: &AutoAuthLatch,
    owned: &CancelToken,
    error: ProxyCallError,
) -> McpResult<ToolResult> {
    match error {
        ProxyCallError::SessionRecoveryAuthRequired { auth_message, .. } => {
            let message = auth_message
                .unwrap_or_else(|| get_auth_required_message(ctx.settings(), server_name));
            let mut map = details_err("call", McpErrorCode::AuthRequired);
            spread(&mut map, identity);
            map.insert("message".to_string(), Value::String(message.clone()));
            map.insert("autoAuthAttempted".to_string(), Value::Bool(latch.attempted()));
            Ok(text_result(message, map))
        }
        ProxyCallError::UrlElicitationRequired { error } => {
            let action = ctx.env.handle_url_elicitation_required(server_name, &error).await;
            let message = match action {
                UrlElicitationAction::Accept =>
                    "The original MCP tool did not run. Complete the opened browser interaction, then retry the tool.".to_string(),
                UrlElicitationAction::Decline => "The URL interaction was declined.".to_string(),
                UrlElicitationAction::Cancel => "The URL interaction was cancelled.".to_string(),
            };
            let mut map = details_err("call", McpErrorCode::UrlElicitationRequired);
            spread(&mut map, identity);
            map.insert("action".to_string(), Value::String(action.as_str().to_string()));
            Ok(text_result(message, map))
        }
        ProxyCallError::Other(error) => {
            let message = error.to_string();
            let guarded = ctx
                .env
                .guard_mcp_output(
                    vec![Content::Text { text: message.clone().into(), text_signature: None }],
                    OutputGuardOptions {
                        prefix: "Failed to call tool: ".to_string(),
                        suffix: schema_suffix.to_string(),
                        ..Default::default()
                    },
                )
                .await;
            let code = if is_abort_error(&error, Some(owned)) {
                McpErrorCode::Aborted
            } else {
                McpErrorCode::CallFailed
            };
            let mut map = details_err("call", code);
            spread(&mut map, identity);
            map.insert(
                "message".to_string(),
                Value::String(if guarded.output_guard.is_some() {
                    "output truncated; see outputGuard.fullOutputPath".to_string()
                } else {
                    message
                }),
            );
            guarded.write_details(&mut map);
            Ok(ToolResult {
                content: guarded.content,
                details: Some(Value::Object(map)),
                ..Default::default()
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::proxy::testsupport::{FakeEnv, config_with, ctx_with, stdio, text_of};
    use serde_json::json;

    // ---- MCP-163 · `executeCall`'s resolution state machine ---------------------------------------------

    #[tokio::test]
    async fn call_fails_closed_for_duplicate_unqualified_names() {
        let config = config_with(&[("zeta", stdio("a")), ("alpha", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("zeta", vec![ToolMetadata::new("create_issue", "create_issue", "")]),
                ("alpha", vec![ToolMetadata::new("create_issue", "create_issue", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        let result = execute_call(&ctx, "create_issue", None, None, &CancelToken::new(), None)
            .await
            .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("ambiguous_tool"));
        assert_eq!(details["mode"], json!("call"));
    }

    #[tokio::test]
    async fn an_exact_disabled_match_suppresses_the_fuzzy_scan_entirely() {
        // `off` matches EXACTLY and is disabled; `live` would match fuzzily. Upstream guards the
        // fuzzy pass with `!toolMeta && !disabledMatch`, so `live` is never reached.
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("off", disabled), ("live", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("off", vec![ToolMetadata::new("do-it", "do-it", "")]),
                ("live", vec![ToolMetadata::new("do_it", "do_it", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        let result =
            execute_call(&ctx, "do-it", None, None, &CancelToken::new(), None).await.unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("server_disabled"));
        assert_eq!(details["server"], json!("off"));
        // The identity names the resolved tool, not just the requested one.
        assert_eq!(details["tool"], json!("do-it"));
    }

    #[tokio::test]
    async fn an_unknown_server_hint_is_server_not_found() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(config, &[], &[], FakeEnv::default());
        let details = execute_call(&ctx, "t", None, Some("nope"), &CancelToken::new(), None)
            .await
            .unwrap()
            .details
            .unwrap();
        assert_eq!(details["error"], json!("server_not_found"));
        assert_eq!(details["requestedTool"], json!("t"));
    }

    // ---- MCP-199 · native-tool detection ------------------------------------------------------------------

    #[tokio::test]
    async fn a_same_named_host_tool_is_reported_as_native_and_none_falls_through() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) =
            ctx_with(config.clone(), &[], &[], FakeEnv::default().with_all_tools(&["read", "mcp"]));
        let result = execute_call(&ctx, "read", None, None, &CancelToken::new(), None).await.unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("native_tool"));
        assert_eq!(
            text_of(&result),
            "\"read\" is a native cyrup tool. Call read directly instead of using mcp({ tool: \"read\" })."
        );

        // `all_tool_names() == None` is upstream's `getPiTools === undefined` branch.
        let (ctx, _) = ctx_with(config, &[], &[], FakeEnv::default());
        let details = execute_call(&ctx, "read", None, None, &CancelToken::new(), None)
            .await
            .unwrap()
            .details
            .unwrap();
        assert_eq!(details["error"], json!("tool_not_found"));
        assert_eq!(details["hintServer"], Value::Null);
    }

    // ---- backoff and approval -------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_recorded_failure_short_circuits_a_hinted_call() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_failure("srv", 7);
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result =
            execute_call(&ctx, "thing", None, Some("srv"), &CancelToken::new(), None).await.unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("server_backoff"));
        assert_eq!(text_of(&result), "Server \"srv\" not available (last failed 7s ago)");
    }

    #[tokio::test]
    async fn approval_failures_report_tool_not_resource_uri() {
        let config = config_with(&[("files", stdio("a"))]);
        let mut resource = ToolMetadata::new("files_read_notes", "read_notes", "");
        resource.resource_uri = Some("file:///notes.md".to_string());
        let env = FakeEnv::default().with_connection("files", ConnectionStatus::Connected);
        *env.approval.lock().unwrap() = Some(ApprovalOutcome::Denied);
        let (ctx, _) = ctx_with(config, &[("files", vec![resource])], &[], env);

        let result =
            execute_call(&ctx, "files_read_notes", None, None, &CancelToken::new(), None).await.unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("approval_denied"));
        // NOT `callIdentity`: a resource tool reports `tool` here rather than `resourceUri`.
        assert_eq!(details["tool"], json!("read_notes"));
        assert!(details.get("resourceUri").is_none());
        assert_eq!(
            text_of(&result),
            "The user declined approval to run MCP tool \"read_notes\" on server \"files\"."
        );
    }

    #[tokio::test]
    async fn an_approval_gated_headless_session_reports_approval_required() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        *env.approval.lock().unwrap() = Some(ApprovalOutcome::NoInteractiveSession);
        let (ctx, _) =
            ctx_with(config, &[("srv", vec![ToolMetadata::new("srv_run", "run", "")])], &[], env);
        let result = execute_call(&ctx, "srv_run", None, None, &CancelToken::new(), None).await.unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("approval_required"));
        assert_eq!(
            text_of(&result),
            "MCP tool \"run\" on server \"srv\" is approval-gated and requires an interactive session."
        );
    }

    // ---- MCP-164 · result shaping --------------------------------------------------------------------------

    #[tokio::test]
    async fn an_empty_success_falls_back_to_the_placeholder_and_keeps_the_identity() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        let (ctx, _) =
            ctx_with(config, &[("srv", vec![ToolMetadata::new("srv_run", "run", "")])], &[], env);
        let result = execute_call(&ctx, "srv_run", None, None, &CancelToken::new(), None).await.unwrap();
        assert_eq!(text_of(&result), "(empty result)");
        let details = result.details.clone().unwrap();
        assert_eq!(details["mode"], json!("call"));
        assert_eq!(details["server"], json!("srv"));
        assert_eq!(details["tool"], json!("run"));
        assert!(details.get("error").is_none(), "a success carries no error code");
    }

}
