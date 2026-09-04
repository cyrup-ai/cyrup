//! `init.ts`'s live-state verbs (13a §13, §17, §18, §19; 13c §3.16) and the crate's one production
//! [`crate::proxy::ProxyEnv`].
//!
//! These are deliberately NOT in [`crate::runtime`]: that module's doc declares two halves — the
//! runtime BUILD and the CONNECTION — that "share no state", and the connection half is testable
//! without an [`McpState`], an owner or a reactor. Mutating a *committed* [`McpState`] is a third
//! thing.
//!
//! Named `live` rather than `env` because [`crate::proxy::env`] already exists and the workspace
//! denies `rustdoc::broken_intra_doc_links` under `--document-private-items`: an intra-doc link
//! spelled `env` in either module would resolve ambiguously and fail the build.
//!
//! # The one divergence from `init.ts`'s signatures, and why
//!
//! Upstream's `loadMetadataCache()` / `saveMetadataCache()` are **module-global**: they resolve
//! `<agent_dir>/mcp-cache.json` themselves and take no path. This port has no module-global agent
//! directory — [`crate::dirs::McpDirs`] is threaded explicitly, and [`McpState`] does not carry one
//! — so the three verbs that touch the cache ([`update_metadata_cache`], [`lazy_connect`] and
//! [`metadata_flush`]) take an [`McpDirs`] alongside the state. Every other verb takes the state
//! alone, exactly as upstream does.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, ReadResourceRequest,
    ReadResourceRequestParams, ReadResourceResult, ServerResult,
};
use rmcp::service::{Peer, PeerRequestOptions, RequestHandle, RoleClient, ServiceError};
use serde_json::{Map as JsonMap, Value};

use cyrup_core::{CancelToken, Content};

use crate::config::ServerEntry;
use crate::dirs::McpDirs;
use crate::errors::{McpError, McpResult};
use crate::lifecycle::ConnectionStatus as LinkStatus;
use crate::proxy::{
    ApprovalOrigin, ApprovalOutcome, AuthRecovery, CallToolOutcome, ConnectOutcome,
    ConnectionStatus, GuardedOutput, OutputGuardOptions, ProxyCallError, ProxyEnv, ToolMetadata,
    UrlElicitationAction,
};
use crate::state::{
    MCP_STATUS_SNAPSHOT_VERSION, McpServerRuntimeStatus, McpServerStatusSnapshot, McpState,
    McpStatusSnapshot, ServerFailure,
};

/// `init.ts:40` `FAILURE_BACKOFF_MS = 60 * 1000` (13a §13).
pub const FAILURE_BACKOFF_MS: u64 = 60_000;
/// `init.ts:41` `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024`.
pub const MAX_FAILURE_MESSAGE_CHARS: usize = 8 * 1024;
/// `init.ts:284` and `:383`'s two `parallelLimit(…, 10, …)` call sites (MCP-022, MCP-026, MCP-130).
pub const STARTUP_CONNECT_CONCURRENCY: usize = 10;

/// `types.ts:138` `McpConnection["status"]` is declared **twice** in this crate — as
/// [`crate::lifecycle::ConnectionStatus`], which is what
/// [`crate::server_manager::ServerConnection::status`] answers, and as
/// [`crate::proxy::ConnectionStatus`], which is what [`ProxyEnv`] speaks. Same three states, and
/// neither module names the other's: `lifecycle.rs` is below `proxy/` in the dependency order and
/// `proxy/env.rs` declares its own so the seam stays testable without a manager.
///
/// This is the one place they meet. It is an exhaustive `match` rather than a cast so that adding a
/// fourth state to either enum is a compile error here instead of a silent mistranslation.
const fn proxy_status(status: LinkStatus) -> ConnectionStatus {
    match status {
        LinkStatus::Connected => ConnectionStatus::Connected,
        LinkStatus::Closed => ConnectionStatus::Closed,
        LinkStatus::NeedsAuth => ConnectionStatus::NeedsAuth,
    }
}

// =================================================================================================
// `parallelLimit` (MCP-087 / MCP-130) — one function, two units
// =================================================================================================

/// `utils.ts` `parallelLimit(items, limit, f)` — at most `limit` in flight, results **by original
/// index**.
///
/// `buffered` is the whole port: it keeps `limit` futures in flight and yields in input order, which
/// is exactly `parallelLimit`'s two properties. `buffer_unordered` / `for_each_concurrent` is WRONG
/// here — `init.ts:305` and `:327` walk `results` twice and `init.ts:382` filters against it by
/// name, all of which assume every element is present and positionally meaningful.
pub async fn parallel_limit<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R>,
{
    use futures::StreamExt as _;
    // A `limit` of 0 would stall the stream; upstream's callers only ever pass 10.
    let limit = limit.max(1);
    futures::stream::iter(items.into_iter().map(f))
        .buffered(limit)
        .collect::<Vec<R>>()
        .await
}

// =================================================================================================
// Failure tracking (MCP-024) — `init.ts:53-81`, `mcp-status.ts:15-21`
// =================================================================================================

/// `init.ts:53-60` `clearFailure(state, serverName)` — idempotent, and the first thing
/// [`record_failure`] calls.
pub fn clear_failure(state: &McpState, server: &str) {
    if let Ok(mut tracker) = state.failure_tracker.lock() {
        tracker.shift_remove(server);
    }
    if let Ok(mut messages) = state.failure_messages.lock() {
        messages.shift_remove(server);
    }
}

/// `init.ts:62-81` `recordFailure(state, serverName, message)`.
///
/// **Two deliberate deviations from upstream's bookkeeping, both observably identical.** (1) There
/// is no timer map. Upstream's `clearTimeout` (`init.ts:58`) exists so a superseded timer cannot
/// clear a newer failure; the `last_failure == failed_at` check below already guarantees that
/// (`init.ts:72`), so a 23rd [`McpState`] field would buy nothing. (2) `timer.unref?.()`
/// (`init.ts:79`) needs no analog — a tokio task does not hold the process open — but the select on
/// the owner token is REQUIRED, not optional: without it a clean shutdown waits out the full 60 s.
pub fn record_failure(state: &Arc<McpState>, server: &str, message: &str) {
    // Read the streak BEFORE the clear wipes it. `ServerFailure::count` is cyrup-only — upstream's
    // `failureTracker` maps name -> timestamp (`init.ts:65`) and has no count — so nothing upstream
    // pins this ordering, but a count read after `clear_failure` is always 0.
    let previous = state
        .failure_tracker
        .lock()
        .ok()
        .and_then(|tracker| tracker.get(server).map(|failure| failure.count))
        .unwrap_or(0);
    clear_failure(state, server);

    let failed_at = Instant::now();
    if let Ok(mut tracker) = state.failure_tracker.lock() {
        tracker.insert(
            server.to_string(),
            ServerFailure {
                last_failure: failed_at,
                count: previous.saturating_add(1),
            },
        );
    }
    if let Ok(mut messages) = state.failure_messages.lock() {
        messages.insert(server.to_string(), truncate_failure_message(message));
    }

    // `WeakMap<McpExtensionState, …>` (`init.ts:42`): the expiry task must not keep the state alive.
    let weak = Arc::downgrade(state);
    let owner = state.owner.token();
    let name = server.to_string();
    // `setTimeout` always exists upstream; a reactor here does not. Outside one the record simply
    // never expires — a bounded degradation, and never a panic on a connect path.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        tokio::select! {
            biased;
            () = owner.cancelled() => {}
            () = tokio::time::sleep(Duration::from_millis(FAILURE_BACKOFF_MS)) => {
                let Some(state) = weak.upgrade() else { return };
                // `if (!state.owner.isActive()) { … return; }` (`init.ts:68`).
                if !state.owner.is_active() {
                    return;
                }
                // `failureTracker.get(serverName) === failedAt` (`init.ts:72`): a re-insert must NOT
                // be cleared by the older timer.
                let still_ours = state.failure_tracker.lock().is_ok_and(|tracker| {
                    tracker.get(&name).is_some_and(|failure| failure.last_failure == failed_at)
                });
                if still_ours {
                    clear_failure(&state, &name);
                    // `publishMcpStatusSnapshot(state)` (`init.ts:75`) — the SNAPSHOT only, not the
                    // footer: this fires on a timer with no user action behind it.
                    state.publish_status(create_mcp_status_snapshot(&state));
                }
            }
        }
    });
}

/// `message.slice(0, MAX_FAILURE_MESSAGE_CHARS)` (`init.ts:66`), on a char boundary.
///
/// Upstream slices UTF-16 code units and is safe only because the string is ASCII in practice; a
/// hostile server's stderr is not. The cap is bytes here and the cut walks back to the nearest
/// boundary — at most three bytes shorter than upstream's for the same input.
fn truncate_failure_message(message: &str) -> String {
    if message.len() <= MAX_FAILURE_MESSAGE_CHARS {
        return message.to_string();
    }
    let mut cut = MAX_FAILURE_MESSAGE_CHARS;
    while cut > 0 && !message.is_char_boundary(cut) {
        cut -= 1;
    }
    message.get(..cut).unwrap_or_default().to_string()
}

/// `mcp-status.ts:15-21` `getActiveFailureAgeSeconds(state, name)` — `None` outside the 60 s window.
///
/// Upstream's falsy-`failedAt` arm (an epoch-`0` timestamp counting as absent) has no analog: the
/// record holds an [`Instant`], which has no zero value, and absence is `None`.
///
/// `Math.round(ageMs / 1000)` is spelled in integer arithmetic rather than through `f64`, so no
/// rounding of the *duration itself* can move a boundary case across the window test above it.
#[must_use]
pub fn failure_age_seconds(state: &McpState, server: &str) -> Option<u64> {
    let tracker = state.failure_tracker.lock().ok()?;
    let age = tracker.get(server)?.last_failure.elapsed();
    // `if (ageMs > FAILURE_BACKOFF_MS) return undefined` — strictly greater, so 60.000 s is inside.
    if age > Duration::from_millis(FAILURE_BACKOFF_MS) {
        return None;
    }
    let rounded = age.as_millis().saturating_add(500) / 1000;
    Some(u64::try_from(rounded).unwrap_or(u64::MAX))
}

/// `init.ts:612-615` `getFailureMessage(state, serverName)` — the same 60 s window over
/// `failureMessages`, which is what `/mcp` and [`create_mcp_status_snapshot`]'s `failed` rung both
/// want to show beside the age.
#[must_use]
pub fn failure_message(state: &McpState, server: &str) -> Option<String> {
    failure_age_seconds(state, server)?;
    state
        .failure_messages
        .lock()
        .ok()
        .and_then(|messages| messages.get(server).cloned())
}

// =================================================================================================
// The snapshot builder (MCP-137) — `mcp-status.ts:24-77`
// =================================================================================================

/// `mcp-status.ts:24-77` `createMcpStatusSnapshot(state)` (13c §3.16). Never connects, never
/// queries.
///
/// Ordering is load-bearing: [`crate::config::McpConfig::mcp_servers`] is an `IndexMap`, so
/// config-file order is preserved. A `BTreeMap` anywhere on this path would list servers
/// alphabetically in `/mcp` and in the footer.
#[must_use]
pub fn create_mcp_status_snapshot(state: &McpState) -> McpStatusSnapshot {
    let mut servers = Vec::with_capacity(state.config.mcp_servers.len());
    let (mut total_tools, mut total_resources) = (0usize, 0usize);
    let (mut connected_count, mut disabled_count) = (0usize, 0usize);

    for (name, definition) in &state.config.mcp_servers {
        // `definition?.disabled === true` — only the literal boolean.
        let disabled = definition.is_disabled();
        let connection = if disabled {
            None
        } else {
            state.manager.get_connection(name)
        };
        let status_of = connection.as_ref().map(|c| c.status());
        let metadata_len = if disabled {
            None
        } else {
            state
                .tool_metadata
                .lock()
                .ok()
                .and_then(|map| map.get(name).map(Vec::len))
        };

        // `metadata?.length ?? (connection?.status === "connected" ? connection.tools.length : 0)`
        let tool_count = metadata_len.unwrap_or_else(|| match (status_of, connection.as_ref()) {
            (Some(LinkStatus::Connected), Some(c)) => c.tools().len(),
            _ => 0,
        });
        // `resourceCounts?.get(name) ?? (connected ? connection.resources.length : undefined)`
        let resource_count = if disabled {
            None
        } else {
            state
                .resource_counts
                .lock()
                .ok()
                .and_then(|map| map.get(name).copied())
                .or_else(|| match (status_of, connection.as_ref()) {
                    (Some(LinkStatus::Connected), Some(c)) => Some(c.resources().len()),
                    _ => None,
                })
        };
        let failed_ago = if disabled {
            None
        } else {
            failure_age_seconds(state, name)
        };

        // `mcp-status.ts:42-55` — first match wins, and the two counters increment INSIDE the
        // ladder.
        let status = if disabled {
            disabled_count += 1;
            McpServerRuntimeStatus::Disabled
        } else if status_of == Some(LinkStatus::Connected) {
            connected_count += 1;
            McpServerRuntimeStatus::Connected
        } else if status_of == Some(LinkStatus::NeedsAuth) {
            McpServerRuntimeStatus::NeedsAuth
        } else if failed_ago.is_some() {
            McpServerRuntimeStatus::Failed
        } else if metadata_len.is_some() {
            McpServerRuntimeStatus::Cached
        } else {
            McpServerRuntimeStatus::NotConnected
        };

        // `totalTools += disabled ? 0 : toolCount` and
        // `if (!disabled && resourceCount !== undefined) totalResources += resourceCount`.
        if !disabled {
            total_tools = total_tools.saturating_add(tool_count);
            total_resources = total_resources.saturating_add(resource_count.unwrap_or(0));
        }
        servers.push(McpServerStatusSnapshot {
            name: name.clone(),
            status,
            tool_count,
            resource_count,
            // `...(status === "failed" && failedAgoSeconds !== undefined ? {failedAgoSeconds} : {})`
            failed_ago_seconds: (status == McpServerRuntimeStatus::Failed)
                .then_some(failed_ago)
                .flatten(),
            disabled,
        });
    }

    McpStatusSnapshot {
        version: MCP_STATUS_SNAPSHOT_VERSION,
        servers,
        total_tools,
        total_resources,
        connected_count,
        disabled_count,
    }
}

// =================================================================================================
// `updateStatusBar` (MCP-032) — `init.ts:568-602`
// =================================================================================================

/// `init.ts:568-602` `updateStatusBar(state)` (13a §18).
///
/// Step 1 publishes ALWAYS, before the `!ui` return: a headless run still feeds the watch, which is
/// what `/mcp` and the proxy tool's `status` mode read. Step 11's `ui.theme.fg("accent", …)` has no
/// analog — `HostServices` exposes a theme *name* and no `fg(role, text)` — and collapses to
/// upstream's own `ui.theme ? … : formattedStatus` no-theme arm.
pub fn update_status_bar(state: &McpState) {
    let snapshot = create_mcp_status_snapshot(state);
    // `connectedCount` (`init.ts:579-582`) is "connected AND the definition exists AND is not
    // disabled" — exactly what the snapshot's ladder just counted, over `config.mcpServers` instead
    // of over the connection map. Same set, one pass.
    let connected = snapshot.connected_count;
    state.publish_status(snapshot);

    let Some(ui) = state.ui.as_ref() else { return };
    let counts = crate::ui::FooterCounts::from_config(&state.config, connected);
    let text = crate::ui::footer_status_text(&state.config, counts);
    cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
}

// =================================================================================================
// `updateServerMetadata` (MCP-028) and `updateMetadataCache` — `init.ts:471-543`
// =================================================================================================

/// `init.ts:471-500` `updateServerMetadata(state, serverName)` (13a §17).
pub fn update_server_metadata(state: &McpState, server: &str) {
    let Some(connection) = state.manager.get_connection(server) else {
        return;
    };
    if connection.status() != LinkStatus::Connected {
        return;
    }
    let Some(definition) = state.config.mcp_servers.get(server) else {
        return;
    };

    // `init.ts:477-484` — a server disabled WHILE connected disappears from the surface on the next
    // refresh instead of lingering. All five maps, then return.
    if definition.is_disabled() {
        forget_server_metadata(state, server);
        return;
    }

    // The collision universe here is `state.toolMetadata` — every server's CURRENT names — not the
    // startup snapshot (`init.ts:488` passes `state.toolMetadata`; `init.ts:340` passes
    // `startupKnownMetadata`). Getting this wrong makes prefixed names order-dependent.
    let universe = state
        .tool_metadata
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let built = crate::registration::build_tool_metadata(
        &connection.tools(),
        &connection.resources(),
        definition,
        server,
        state.config.tool_prefix(),
        Some(&state.config.mcp_servers),
        Some(&universe),
        false,
    );

    if let Ok(mut map) = state.tool_metadata.lock() {
        map.insert(server.to_string(), built.metadata);
    }
    if let Ok(mut counts) = state.resource_counts.lock() {
        counts.insert(server.to_string(), connection.resources().len());
    }
    // `init.ts:491-494` — only from a LIVE list, and only when discovery did not fail.
    commit_prompt_metadata(state, server);
    // `if (connection.instructions) … else delete` (`init.ts:495-499`) — a TRUTHY test, so an EMPTY
    // string DELETES. `proxy/auth.rs`'s commit arm already spells this correctly.
    if let Ok(mut map) = state.server_instructions.lock() {
        match connection.instructions().filter(|text| !text.is_empty()) {
            Some(text) => {
                map.insert(server.to_string(), text.to_string());
            }
            None => {
                map.shift_remove(server);
            }
        }
    }
}

/// `init.ts:491-494` — [`update_server_metadata`]'s prompt half, extracted so `proxy/auth.rs`'s
/// eight-step commit can run it alone (it has already tested `promptDiscoveryFailed` itself, and
/// re-testing it here is what keeps the two call sites from drifting).
///
/// A cache-rehydrated prompt list is deliberately **not** routed through here — see
/// [`rehydrate_from_cache`] for why `promptMetadataLive` is the flag that separates the two.
pub fn commit_prompt_metadata(state: &McpState, server: &str) {
    let Some(connection) = state.manager.get_connection(server) else {
        return;
    };
    if connection.status() != LinkStatus::Connected || connection.prompt_discovery_failed() {
        return;
    }
    let Some(definition) = state.config.mcp_servers.get(server) else {
        return;
    };
    let prompts = crate::registration::reconstruct_prompt_metadata(
        server,
        &connection.prompts(),
        state.config.tool_prefix(),
        Some(definition),
    );
    if let Ok(mut map) = state.prompt_metadata.lock() {
        map.insert(server.to_string(), prompts);
    }
    if let Ok(mut live) = state.prompt_metadata_live.lock() {
        live.insert(server.to_string());
    }
}

/// The five-map delete `init.ts:478-482` performs, shared with `unregisterServer`.
fn forget_server_metadata(state: &McpState, server: &str) {
    if let Ok(mut map) = state.tool_metadata.lock() {
        map.shift_remove(server);
    }
    if let Ok(mut map) = state.resource_counts.lock() {
        map.shift_remove(server);
    }
    if let Ok(mut map) = state.prompt_metadata.lock() {
        map.shift_remove(server);
    }
    if let Ok(mut live) = state.prompt_metadata_live.lock() {
        live.remove(server);
    }
    if let Ok(mut map) = state.server_instructions.lock() {
        map.shift_remove(server);
    }
}

/// `init.ts:502-543` `updateMetadataCache(state, serverName, options)`'s options.
#[derive(Debug, Clone, Copy)]
pub struct MetadataCacheOptions {
    /// `options.preserveEmptyResources !== false` (`init.ts:528`). The default is "preserve"; the
    /// list-changed listener passes `false` because THAT empty `resources/list` is authoritative.
    pub preserve_empty_resources: bool,
}

impl MetadataCacheOptions {
    /// Upstream's `{}` — the absent key reads as `!== false`, i.e. preserve. Spelled as a named
    /// constructor rather than a `Default` impl so no call site can silently mean the other thing.
    #[must_use]
    pub fn preserving() -> Self {
        Self {
            preserve_empty_resources: true,
        }
    }
}

/// `Date.now()` in epoch milliseconds, for `ServerCacheEntry::cached_at`.
///
/// `0` is the crate's "invalid entry" sentinel (`is_server_cache_valid` rejects it with the falsy
/// `!entry.cachedAt` test), which is exactly the right answer for a clock that cannot be read.
fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// `init.ts:502-543` `updateMetadataCache(state, serverName, options)` — write this server's entry
/// into `<agent_dir>/mcp-cache.json`.
///
/// The read and the write are both [`crate::dirs`]': this is the **writer** side of the cache, and
/// [`crate::dirs::save_metadata_cache`] merges rather than replaces, which is what makes upstream's
/// one-entry `saveMetadataCache({version: 1, servers: {[name]: entry}})` (`init.ts:542`)
/// non-destructive there and here.
///
/// All three conditional keys are reproduced: `prompts` falls back to the existing entry's when
/// `promptDiscoveryFailed` **and** the hash matches (`init.ts:519-521`), `resources` is `[]` under
/// `exposeResources: false` (`:518`), and `instructions` is written whenever the connection carries
/// the field at all — a `!== undefined` test, so unlike [`update_server_metadata`]'s truthy one an
/// **empty string is cached**, not deleted (`:538`).
///
/// `dirs` is a parameter because upstream's `loadMetadataCache()` / `saveMetadataCache()` are
/// module-global and this port's are not; [`McpState`] carries no [`McpDirs`].
pub fn update_metadata_cache(
    state: &McpState,
    dirs: &McpDirs,
    server: &str,
    options: MetadataCacheOptions,
) {
    let Some(connection) = state.manager.get_connection(server) else {
        return;
    };
    if connection.status() != LinkStatus::Connected {
        return;
    }
    let Some(definition) = state.config.mcp_servers.get(server) else {
        return;
    };
    if definition.is_disabled() {
        return;
    }

    // `const configHash = computeServerHash(definition)` — which THROWS on a `url` naming an unset
    // variable. `None` is that throw, and a cache entry that cannot be keyed by identity must not
    // be written at all: the reader would reject it on the next run anyway.
    let Some(config_hash) = crate::registration::default_server_hasher(definition) else {
        return;
    };

    let path = dirs.metadata_cache();
    let existing = crate::dirs::load_metadata_cache(&path);
    let existing_entry = existing
        .as_ref()
        .and_then(|cache| cache.servers.get(server));
    let hash_matches =
        existing_entry.is_some_and(|entry| entry.config_hash.as_str() == config_hash.as_str());

    let tools = crate::dirs::serialize_tools(&connection.tools());
    // `definition.exposeResources === false ? [] : serializeResources(connection.resources)`.
    let mut resources = if definition.expose_resources() {
        crate::dirs::serialize_resources(&connection.resources())
    } else {
        Vec::new()
    };
    // `init.ts:519-521` — a failed `prompts/list` keeps the previous entry's prompts, but ONLY when
    // that entry describes the same server definition. Otherwise the key is omitted entirely, which
    // is not the same as writing `[]`: an absent `prompts` means "never discovered".
    let prompts = if connection.prompt_discovery_failed() {
        if hash_matches {
            existing_entry.and_then(|entry| entry.prompts.clone())
        } else {
            None
        }
    } else {
        Some(crate::dirs::serialize_prompts(&connection.prompts()))
    };

    // `init.ts:523-531` — five conjuncts, and the empty-list guard is the whole reason
    // `preserveEmptyResources` exists: a `resources/list` that came back empty because the server
    // is still warming up must not erase yesterday's list, while the list-changed listener's empty
    // one is authoritative and passes `false`.
    if definition.expose_resources()
        && resources.is_empty()
        && hash_matches
        && options.preserve_empty_resources
        && let Some(previous) = existing_entry.filter(|entry| !entry.resources.is_empty())
    {
        resources = previous.resources.clone();
    }

    let entry = crate::dirs::ServerCacheEntry {
        config_hash,
        tools,
        resources,
        prompts,
        // `...(connection.instructions !== undefined ? {instructions} : {})` — presence, not
        // truthiness. An empty string round-trips as an empty string.
        instructions: connection.instructions().map(str::to_string),
        cached_at: now_epoch_ms(),
    };

    let mut one = crate::dirs::MetadataCache::default();
    one.servers.insert(server.to_string(), entry);
    if let Err(error) = crate::dirs::save_metadata_cache(&path, &one) {
        // Upstream's `saveMetadataCache` swallows its own write failure (`metadata-cache.ts:57`'s
        // try/catch); a cache that cannot be written is a slower next start, never a failed connect.
        tracing::debug!("MCP: failed to write metadata cache for {server}: {error}");
    }
}

/// `init.ts:560-566` `flushMetadataCache(state)` — the [`crate::lifecycle::MetadataFlush`]
/// `shutdown_state` takes, replacing `no_metadata_flush` (MCP-031).
///
/// `dirs` is captured rather than taken per call because [`crate::lifecycle::MetadataFlush`]'s own
/// signature is `Fn(&Arc<McpState>)` and [`McpState`] carries no [`McpDirs`] — see the module doc.
#[must_use]
pub fn metadata_flush(dirs: McpDirs) -> crate::lifecycle::MetadataFlush {
    Arc::new(move |state: &Arc<McpState>| {
        for (name, connection) in state.manager.get_all_connections() {
            if connection.status() == LinkStatus::Connected {
                update_metadata_cache(state, &dirs, &name, MetadataCacheOptions::preserving());
            }
        }
        Ok(())
    })
}

// =================================================================================================
// `lazyConnect` (MCP-033) — `init.ts:617-662`
// =================================================================================================

/// `init.ts:617-662` `lazyConnect(state, serverName, signal)` (13a §19) — `true` iff the server
/// ended `connected`.
///
/// The seam **flattens upstream's throw into a `bool`**, which moves one load-bearing detail into
/// the error arm below: `init.ts:652-655` is
/// `if (isAbortError(error, ownedSignal)) { throwIfAborted(ownedSignal); }`, which **falls through**
/// to `recordFailure` when the rethrow does not fire. So the entire observable content of §19 step 8
/// is: *an abort on an actually-cancelled signal must not `record_failure`; a stray abort error on a
/// live signal must.* Collapsing the two lets a server-side cancellation poison the next sixty
/// seconds of that server's availability.
pub async fn lazy_connect(
    state: &Arc<McpState>,
    dirs: &McpDirs,
    server: &str,
    cancel: &CancelToken,
) -> bool {
    // 1 — `combineAbortSignals(state.owner?.signal, signal)` then `throwIfAborted`.
    let owned = crate::abort::combine(&state.owner.token(), Some(cancel));
    if owned.is_cancelled() {
        return false;
    }
    // 2-3 — needs-auth is checked FIRST (`init.ts:621`), connected second (`:624`).
    if let Some(connection) = state.manager.get_connection(server) {
        match connection.status() {
            LinkStatus::NeedsAuth => return false,
            LinkStatus::Connected => {
                update_server_metadata(state, server);
                state.lifecycle.mark_keep_alive_after_connect(server);
                return true;
            }
            LinkStatus::Closed => {}
        }
    }
    // 4 — inside the backoff, do not retry.
    if failure_age_seconds(state, server).is_some() {
        return false;
    }
    // 5
    let Some(definition) = state.config.mcp_servers.get(server) else {
        return false;
    };
    if definition.is_disabled() {
        return false;
    }
    // 6
    if let Some(ui) = state.ui.as_ref() {
        let text =
            crate::ui::format_mcp_status(&state.config, &format!("connecting to {server}..."));
        cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
    }
    // 7-8
    match state
        .manager
        .connect(server, definition, Some(&owned))
        .await
    {
        Ok(connection) if connection.status() == LinkStatus::NeedsAuth => false,
        Ok(_) => {
            clear_failure(state, server);
            update_server_metadata(state, server);
            update_metadata_cache(state, dirs, server, MetadataCacheOptions::preserving());
            state.notify_tool_metadata_updated(server, "lazy-connect");
            state.lifecycle.mark_keep_alive_after_connect(server);
            update_status_bar(state);
            true
        }
        Err(error) => {
            // `if (isAbortError(...)) throwIfAborted(ownedSignal)` — the rethrow arm. No failure is
            // recorded for a genuine cancellation; a stray abort on a LIVE signal falls through and
            // IS recorded, exactly as upstream's non-throwing `throwIfAborted` does.
            // `is_abort_error` answers `true` when the token is cancelled OR the error is
            // `McpError::Aborted`, so the `&& owned.is_cancelled()` conjunct is what separates the
            // two arms. Do not simplify it away.
            if crate::abort::is_abort_error(&error, Some(&owned)) && owned.is_cancelled() {
                return false;
            }
            let message = error.to_string();
            record_failure(state, server, &message);
            tracing::debug!(
                "MCP: lazy connect failed for {server}: {}",
                crate::ui::sanitize_terminal_text(&message)
            );
            update_status_bar(state);
            false
        }
    }
}

// =================================================================================================
// `rehydrateFromCache` (MCP-021) — `init.ts:256-269`
// =================================================================================================

/// `init.ts:256-269` — populate this generation's maps from a hash-valid cache entry.
///
/// Takes [`crate::registration`]'s LENIENT [`crate::registration::ServerCacheEntry`], not
/// [`crate::dirs`]'s: that is the reader half, and it is the one `resolve_direct_tools` and
/// `resolve_cached_prompts` already registered this session's surface from.
///
/// Four writes, three conditional, and one deliberate omission: `promptMetadataLive` is NOT touched.
/// That set is the "came from a live `prompts/list`" flag, and adding a cache-rehydrated server to
/// it would make [`update_metadata_cache`] treat stale prompts as authoritative.
///
/// The cache-validity guard is **not** re-derived here: [`crate::registration::valid_entry`] is
/// exactly `cachedEntry && isServerCacheValid(cachedEntry, definition)` and is already the guard
/// `resolve_direct_tools` opens with. Reaching for `crate::dirs::try_compute_server_hash` on this
/// path instead would be the reader/writer hash drift that seam exists to prevent.
pub fn rehydrate_from_cache(
    state: &McpState,
    server: &str,
    definition: &ServerEntry,
    entry: &crate::registration::ServerCacheEntry,
    cache: &crate::registration::MetadataCache,
) {
    let prefix = state.config.tool_prefix();
    let metadata = crate::registration::reconstruct_tool_metadata(
        server,
        entry,
        prefix,
        definition,
        Some(&state.config.mcp_servers),
        Some(cache),
    );
    if let Ok(mut map) = state.tool_metadata.lock() {
        map.insert(server.to_string(), metadata);
    }
    // `if (Array.isArray(cachedEntry.resources))` — an ABSENT list writes no count at all, which is
    // not the same as writing 0: 0 means "asked, got none". `entry.resources` is the raw `Option`,
    // NOT the `entry.resources()` accessor, which flattens absent into `&[]`.
    if let Some(resources) = entry.resources.as_ref()
        && let Ok(mut counts) = state.resource_counts.lock()
    {
        counts.insert(server.to_string(), resources.len());
    }
    // `if (cachedEntry.prompts?.length)` — NON-EMPTY, not merely present.
    if !entry.prompts().is_empty()
        && let Ok(mut map) = state.prompt_metadata.lock()
    {
        map.insert(
            server.to_string(),
            crate::registration::reconstruct_prompt_metadata(
                server,
                entry.prompts(),
                prefix,
                Some(definition),
            ),
        );
    }
    // `if (cachedEntry.instructions)` — truthy, so an empty string writes nothing.
    if let Some(text) = entry
        .instructions
        .as_deref()
        .filter(|text| !text.is_empty())
        && let Ok(mut map) = state.server_instructions.lock()
    {
        map.insert(server.to_string(), text.to_string());
    }
}

// =================================================================================================
// `RuntimeEnv` — the one production `ProxyEnv`
// =================================================================================================

/// The crate's ONE production [`crate::proxy::ProxyEnv`].
///
/// Every method is a delegation; the call order and branch structure live in [`crate::proxy`], which
/// is what makes `FakeEnv` and this type interchangeable under 13d's conformance suite (MCP-196).
pub struct RuntimeEnv {
    /// The committed generation this env speaks for.
    state: Arc<McpState>,
    /// Where [`update_metadata_cache`] writes. Cloned from `McpExtension::dirs`.
    dirs: McpDirs,
    /// `Weak`, for the same reason the surface-sync listener is: the state this env holds is owned
    /// by the extension, so a strong handle back would cycle.
    extension: std::sync::Weak<crate::extension::McpExtension>,
}

impl std::fmt::Debug for RuntimeEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeEnv")
            .field("servers", &self.state.config.mcp_servers.len())
            .field("agent_dir", &self.dirs.agent_dir())
            .finish_non_exhaustive()
    }
}

impl RuntimeEnv {
    /// Bind an env to a committed state, the directory layout its cache lives in, and the extension
    /// that owns both.
    #[must_use]
    pub fn new(
        state: Arc<McpState>,
        dirs: McpDirs,
        extension: std::sync::Weak<crate::extension::McpExtension>,
    ) -> Self {
        Self {
            state,
            dirs,
            extension,
        }
    }

    /// `state.config.mcpServers[serverName]`, or the byte-exact upstream "not configured" error
    /// (`init.ts:386`) — the one shape `connect` / `reconnect` / the auth verbs all need.
    ///
    /// # Errors
    ///
    /// [`McpError::Other`] naming the server, when no `mcpServers` entry exists.
    fn definition(&self, server: &str) -> McpResult<ServerEntry> {
        self.state
            .config
            .mcp_servers
            .get(server)
            .cloned()
            .ok_or_else(|| McpError::other(format!("MCP server \"{server}\" is not configured")))
    }

    /// `buildToolMetadata(connection.tools, connection.resources, …, state.toolMetadata)` plus the
    /// three other fields [`ConnectOutcome`] carries — `proxy-modes.ts:778`'s exact call, shared by
    /// [`ProxyEnv::connect`] and [`ProxyEnv::reconnect`].
    fn outcome_of(
        &self,
        server: &str,
        connection: &crate::server_manager::ServerConnection,
    ) -> ConnectOutcome {
        let known = self
            .state
            .tool_metadata
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let definition = self.state.config.mcp_servers.get(server);
        let metadata = match definition {
            Some(definition) => {
                crate::registration::build_tool_metadata(
                    &connection.tools(),
                    &connection.resources(),
                    definition,
                    server,
                    self.state.config.tool_prefix(),
                    Some(&self.state.config.mcp_servers),
                    Some(&known),
                    false,
                )
                .metadata
            }
            None => Vec::new(),
        };
        ConnectOutcome {
            status: Some(proxy_status(connection.status())),
            metadata,
            instructions: connection.instructions().map(str::to_string),
            prompt_discovery_failed: connection.prompt_discovery_failed(),
        }
    }

    /// `{authStorageOptions: state.authStorageOptions, signal, runtime: state.oauthRuntime}` —
    /// the three options every `mcp-auth-flow.ts` call site in `proxy-modes.ts` passes
    /// (`proxy-modes.ts:165-177`, `:368-372`, `:408-411`).
    fn auth_options(&self, cancel: &CancelToken) -> crate::oauth::AuthenticateOptions {
        // The GENERATION's vault, not a fresh one. `initialize_mcp` publishes the same handle it
        // built [`crate::runtime::StoredCredentialAuth`] over, so a token this flow writes lands in
        // the cache the connect ladder reads and `invalidateAuthEntryCache` evicts an entry someone
        // else will see. The fallback is for a manager nothing published to — a `McpServerManager`
        // built by `new`/`default` — and reconstructs from the same `(dirs, authStorageOptions)`
        // pair, so it differs only in which in-process cache it holds.
        let store = self.state.manager.auth_store().unwrap_or_else(|| {
            crate::credentials::McpAuthStore::new(
                self.dirs.clone(),
                self.state.auth_storage_options.clone(),
            )
        });
        let storage: Arc<dyn crate::oauth::McpOAuthStorage> = Arc::new(store);
        let mut options = crate::oauth::AuthenticateOptions::new(storage);
        options.runtime = Some(Arc::clone(&self.state.oauth_runtime));
        options.signal = Some(cancel.clone());
        options
    }
}

// =================================================================================================
// MCP-164 — the two live requests: `tools/call` and `resources/read`
//
// `proxy-modes.ts:1198-1240` is two `withSessionRecovery(...)` call sites and nothing else, so this
// section is those two, the recovery wrapper they ride (`session-recovery.ts:88-152`) and the
// cancellation the `ProxyEnv` trait docs prescribe. What it deliberately does NOT re-derive:
// `isTerminatedSession` is already `server_manager::is_terminated_session` (MCP-134), the auto-auth
// ladder is already behind `AuthRecovery::recover` (MCP-162 — calling it rather than re-deriving it
// is what keeps that unit's single-shot latch honest), and `transformMcpContent` /
// `transformMcpResourceContents` / `resolveMcpResultContent` are already `renderers.rs`
// (MCP-220..MCP-222).
//
// # Why the request is sent by hand rather than through `Peer::call_tool_once`
//
// Upstream passes `requestOptions` — which carries `ownedSignal` — into every `client.callTool` /
// `client.readResource`, and the SDK answers an aborted signal by sending `notifications/cancelled`
// and rejecting. rmcp's convenience wrappers take no signal: they build the request, await it, and
// a caller who drops the future leaves the server *still running the tool* with no cancellation on
// the wire. `Peer::send_request_with_option` hands back a `RequestHandle` instead, and
// `RequestHandle::cancel(reason)` is exactly the notification the SDK sends — so the two verbs go
// through the handle. `request_on_peer` is that, once, for both.
//
// The cost of not calling `call_tool_once`/`read_resource` is rmcp's own response cache
// (SEP-2549 `resources/read` freshness), which those two consult and this does not. Upstream has no
// such cache and `guardMcpOutput` writes a fresh spill file per call, so skipping it is the
// upstream-faithful direction rather than a regression.
// =================================================================================================

/// rmcp's own rendering of `StreamableHttpError::SessionExpired`, read off the type rather than
/// copied into a literal, so a change to that text in a future rmcp cannot silently stop matching.
///
/// The type parameter is only the HTTP client's error type and the variant is a unit — the `Display`
/// impl ignores it — so `std::io::Error` stands in for the decorator stack
/// [`crate::runtime::http_transport_with_client`] actually builds, which this module cannot spell.
static SESSION_EXPIRED_DISPLAY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    rmcp::transport::streamable_http_client::StreamableHttpError::<std::io::Error>::SessionExpired
        .to_string()
});

/// One failed MCP request, carrying both the error the caller will surface **and** the two facts
/// [`crate::server_manager::is_terminated_session`] classifies on.
///
/// The pair has to travel together: the predicate needs the JSON-RPC code or the HTTP status, and
/// [`McpError`] has nowhere to put either. Mapping first and classifying afterwards would mean
/// re-deriving the code from a rendered string, which is exactly the fragility MCP-134's evidence
/// seam exists to avoid.
#[derive(Debug)]
struct RequestFailure {
    /// What the caller surfaces — [`ProxyCallError::Other`]'s payload.
    error: McpError,
    /// `err instanceof SdkHttpError ? err.status : undefined`.
    http_status: Option<u16>,
    /// `err instanceof ProtocolError ? err.code : undefined`.
    protocol_code: Option<i32>,
    /// `err.message` — the serialised body the predicate's two 400 regexes scan on the HTTP arm,
    /// and the **bare** protocol message its two-string set is tested against on the other.
    message: String,
}

impl RequestFailure {
    /// A failure with no transport evidence — nothing a session recovery could act on.
    fn plain(message: String) -> Self {
        Self {
            error: McpError::other(message.clone()),
            http_status: None,
            protocol_code: None,
            message,
        }
    }

    /// The cancel arm. [`McpError::Aborted`] rather than [`McpError::Other`] is load-bearing:
    /// `crate::abort::is_abort_error` is what turns this into `details.error == "aborted"` instead
    /// of `"call_failed"` in `executeCall`'s catch.
    fn aborted() -> Self {
        Self {
            error: McpError::Aborted(crate::abort::ABORTED_FALLBACK_REASON.to_string()),
            http_status: None,
            protocol_code: None,
            message: crate::abort::ABORTED_FALLBACK_REASON.to_string(),
        }
    }

    /// rmcp's [`ServiceError`] → this crate's error class plus the evidence.
    ///
    /// The `McpError` arm keeps the server's message **bare**, code and all left out of the text.
    /// That is not a simplification: `ProtocolError`'s constructor is `super(message)`
    /// (`@modelcontextprotocol/client` `src-D_zzAWoS.mjs:3446`), so `error.message` upstream is the
    /// server's own sentence, and `executeCall`'s catch surfaces it verbatim under the
    /// `Failed to call tool: ` prefix. Prefixing it with `MCP error {code}:` here would diverge from
    /// what the user reads — **and** would break the evidence: `isTerminatedSession`'s protocol arm
    /// tests `err.message` against two exact strings, and a prefixed message matches neither.
    /// The code is carried out in [`Self::protocol_code`], where the predicate wants it.
    fn from_service(error: ServiceError) -> Self {
        match error {
            ServiceError::McpError(data) => Self {
                error: McpError::other(data.message.to_string()),
                http_status: None,
                protocol_code: Some(data.code.0),
                message: data.message.into_owned(),
            },
            // `ServiceError::Cancelled` is the peer telling us the request was cancelled; upstream's
            // `isAbortError` arm would classify the SDK's equivalent the same way.
            ServiceError::Cancelled { reason } => {
                let reason =
                    reason.unwrap_or_else(|| crate::abort::ABORTED_FALLBACK_REASON.to_string());
                Self {
                    error: McpError::Aborted(reason.clone()),
                    http_status: None,
                    protocol_code: None,
                    message: reason,
                }
            }
            other => {
                let http_status = session_expired_status(&other);
                let message = other.to_string();
                Self {
                    error: McpError::other(message.clone()),
                    http_status,
                    protocol_code: None,
                    message,
                }
            }
        }
    }

    /// The borrowed view MCP-134's predicate takes.
    fn evidence(&self) -> crate::server_manager::TerminatedSessionEvidence<'_> {
        crate::server_manager::TerminatedSessionEvidence {
            http_status: self.http_status,
            protocol_code: self.protocol_code,
            message: &self.message,
        }
    }
}

/// `err instanceof SdkHttpError && err.status === 404`, as far as rmcp lets this module see it.
///
/// rmcp folds the spec's "404 for a request that carried an `Mcp-Session-Id`" into one typed
/// variant, `StreamableHttpError::SessionExpired`, which is the same fact
/// [`crate::server_manager::TerminatedSessionEvidence`] documents may be reported as `Some(404)`
/// directly. It arrives here inside a `DynamicTransportError`'s boxed error, and
/// `StreamableHttpError` is generic over the HTTP client's error type — the concrete parameter being
/// the `SessionIdProbe`/`AuthClient` stack `runtime.rs` assembles — so a `downcast_ref` would have to
/// name a type this module cannot spell. Comparing against rmcp's own `Display` for the variant can,
/// and [`SESSION_EXPIRED_DISPLAY`] takes that string from rmcp instead of restating it.
///
/// The 400-with-both-markers arm of `isTerminatedSession` has no counterpart to extract: rmcp raises
/// no typed 400 and does not carry the status alongside the body. That arm is therefore unreachable
/// today, which fails **closed** — an unproven session is not retried, and a retry that should not
/// have happened can double-execute a tool call.
fn session_expired_status(error: &ServiceError) -> Option<u16> {
    let ServiceError::TransportSend(transport) = error else {
        return None;
    };
    let expired: &str = SESSION_EXPIRED_DISPLAY.as_str();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(transport.error.as_ref());
    while let Some(error) = current {
        if error.to_string() == expired {
            return Some(404);
        }
        current = error.source();
    }
    None
}

/// One MCP request on a live peer, with upstream's `requestOptions.signal` cancellation.
///
/// `abortable(..)`'s `biased` cancel arm is the `is_cancelled` check ahead of the send: an
/// already-stopped generation must not start a request it would immediately cancel. After the send
/// the race is against `handle.rx` directly rather than `RequestHandle::await_response`, which
/// consumes the handle and would leave nothing to call `cancel` on — the borrow-then-take shape rmcp
/// itself uses for a pending subscription (`service/client.rs:391-432`). The timeout arm is
/// `await_response`'s own `(Some(timeout), None, false)` branch, reproduced because
/// [`crate::runtime::build_request_options`] only ever sets `timeout`: `reset_timeout_on_progress`
/// and `max_total_timeout` have no upstream analogue and stay at their defaults.
async fn request_on_peer(
    peer: &Peer<RoleClient>,
    request: ClientRequest,
    options: PeerRequestOptions,
    cancel: &CancelToken,
) -> Result<ServerResult, Box<RequestFailure>> {
    if cancel.is_cancelled() {
        return Err(Box::new(RequestFailure::aborted()));
    }
    let timeout = options.timeout;
    let mut handle = peer
        .send_request_with_option(request, options)
        .await
        .map_err(|error| Box::new(RequestFailure::from_service(error)))?;

    /// Which of the four ways the race can end happened, computed while `handle.rx` is borrowed so
    /// the arms below can move the handle into `cancel`.
    enum Settled {
        /// The peer answered.
        /// Boxed: this payload is ~360 bytes against a 16-byte second-largest variant, so an
        /// unboxed enum would make every `Settled` that wide (`clippy::large_enum_variant`).
        Response(Box<Result<ServerResult, ServiceError>>),
        /// The service dropped the responder — rmcp's `ServiceError::TransportClosed`.
        Closed,
        /// `options.timeout` elapsed.
        TimedOut(Duration),
        /// `ownedSignal` fired.
        Cancelled,
    }

    let settled = match timeout {
        Some(limit) => tokio::select! {
            biased;
            () = cancel.cancelled() => Settled::Cancelled,
            settled = tokio::time::timeout(limit, &mut handle.rx) => match settled {
                Ok(Ok(response)) => Settled::Response(Box::new(response)),
                Ok(Err(_closed)) => Settled::Closed,
                Err(_elapsed) => Settled::TimedOut(limit),
            },
        },
        None => tokio::select! {
            biased;
            () = cancel.cancelled() => Settled::Cancelled,
            settled = &mut handle.rx => match settled {
                Ok(response) => Settled::Response(Box::new(response)),
                Err(_closed) => Settled::Closed,
            },
        },
    };

    match settled {
        Settled::Response(response) => {
            (*response).map_err(|error| Box::new(RequestFailure::from_service(error)))
        }
        Settled::Closed => Err(Box::new(RequestFailure::plain(
            "MCP transport closed before the request was answered".to_string(),
        ))),
        Settled::TimedOut(limit) => {
            // `await_response` sends the same notification with the same reason before returning
            // its `Timeout`; the request is cancelled on the wire either way.
            let reason = RequestHandle::<RoleClient>::REQUEST_TIMEOUT_REASON.to_string();
            if let Err(error) = handle.cancel(Some(reason)).await {
                tracing::debug!("MCP: cancelling a timed-out request failed: {error}");
            }
            Err(Box::new(RequestFailure::plain(format!(
                "MCP request timed out after {} ms",
                limit.as_millis()
            ))))
        }
        Settled::Cancelled => {
            if let Err(error) = handle
                .cancel(Some(crate::abort::ABORTED_FALLBACK_REASON.to_string()))
                .await
            {
                tracing::debug!("MCP: cancelling an aborted request failed: {error}");
            }
            Err(Box::new(RequestFailure::aborted()))
        }
    }
}

/// `connection.client` — the peer every MCP request rides, or the honest refusal.
///
/// [`crate::server_manager::ConnectionResource::peer`] answers `None` for the two resources that
/// have no client behind them (the `needs-auth` early return, and a stdio child rmcp was never
/// given). Neither can serve a request, and inventing a result for one would be a tool result the
/// model believes came from the server.
fn peer_of(
    connection: &crate::server_manager::ServerConnection,
    server: &str,
) -> Result<Peer<RoleClient>, ProxyCallError> {
    connection.resource().peer().cloned().ok_or_else(|| {
        ProxyCallError::Other(McpError::Server {
            server: server.to_string(),
            message: "the connection has no live MCP client to send the request on".to_string(),
        })
    })
}

/// `result.isError ? transformMcpContent(result.content) : resolveMcpResultContent(result)`, plus
/// `rawMcpResult: result` — the whole of what `executeCall` needs from one `tools/call` answer.
///
/// Free rather than inline in [`ProxyEnv::call_tool`] so it can be driven by a **real**
/// [`CallToolResult`] off a live server in the tests below, with no manager and no state in the way:
/// the branch is the single place a correct wire result can still be turned into the wrong content.
///
/// The transform is chosen HERE, not in [`crate::proxy::execute_call`], because [`CallToolOutcome`]
/// carries one content list while upstream uses a different function on each side of
/// `result.isError`: `transformMcpContent(result.content)` on the error path, which never consults
/// `structuredContent`, and `resolveMcpResultContent(result)` on the success path, which falls back
/// to it when the block list comes out empty.
///
/// The `MaterializedResources` scope is `None`, which is `MaterializedResources::global()` — upstream
/// passes `state.owner?.signal` and this port has exactly one materialisation scope, the global one
/// `McpExtension`'s shutdown already cleans up.
fn call_tool_outcome(
    server: &str,
    result: &CallToolResult,
) -> Result<CallToolOutcome, ProxyCallError> {
    // The untouched result the output guard stores, and the same value both transforms read, so the
    // blocks and `details.mcpResult` cannot disagree.
    let raw = serde_json::to_value(result).map_err(|error| {
        ProxyCallError::Other(McpError::Server {
            server: server.to_string(),
            message: format!("the `tools/call` result could not be read: {error}"),
        })
    })?;
    // `result.isError` is optional on the wire and absent means "not an error".
    let is_error = result.is_error.unwrap_or(false);
    let blocks = if is_error {
        let content = raw
            .get("content")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        crate::renderers::transform_mcp_content(content, None)
    } else {
        crate::renderers::resolve_mcp_result_content(&raw, None)
    };
    Ok(CallToolOutcome {
        content: blocks
            .into_iter()
            .map(crate::renderers::McpContentBlock::into_core)
            .collect(),
        is_error,
        raw: Some(raw),
    })
}

/// `transformMcpResourceContents(result.contents ?? [])`.
///
/// The `?? []` is why an absent list is an **empty read** rather than a failure; `executeCall` turns
/// that into the `(empty resource)` placeholder. Free for the same reason
/// [`call_tool_outcome`] is.
fn resource_contents(
    server: &str,
    result: &ReadResourceResult,
) -> Result<Vec<Content>, ProxyCallError> {
    let raw = serde_json::to_value(result).map_err(|error| {
        ProxyCallError::Other(McpError::Server {
            server: server.to_string(),
            message: format!("the `resources/read` result could not be read: {error}"),
        })
    })?;
    let contents = raw
        .get("contents")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    Ok(
        crate::renderers::transform_mcp_resource_contents(contents, None)
            .into_iter()
            .map(crate::renderers::McpContentBlock::into_core)
            .collect(),
    )
}

impl RuntimeEnv {
    /// `withSessionRecovery(deps, serverName, fn)` (`session-recovery.ts:88-152`) — MCP-135.
    ///
    /// Runs `run` against the current connection's peer. A failure that is **proven** to be a
    /// terminated Streamable HTTP session reconnects exactly once and retries exactly once;
    /// everything else propagates unchanged, which is the module's whole point (a retry that should
    /// not have happened can double-execute the original request).
    ///
    /// # The one upstream step that is not here
    ///
    /// `if (definition && supportsOAuth(definition) && (err instanceof UnauthorizedError || 401))
    /// invalidateAuthEntryCache(serverName)`. The eviction primitive exists
    /// ([`crate::credentials::McpAuthStore::invalidate_cache`]) and, since
    /// [`crate::server_manager::McpServerManager::auth_store`] publishes the generation's vault,
    /// there is now a store to evict from that someone else reads — so the line is *reachable*. It
    /// is still not here, and the reason is scope rather than plumbing: this wrapper's contract is
    /// "reconnect once on a proven terminated session, propagate everything else unchanged", and a
    /// 401 arriving mid-call is the auto-auth ladder's ([`crate::proxy::call::AuthRecovery`])
    /// business, not this one's. Land it with the unit that owns that classification.
    async fn with_session_recovery<T, F, Fut>(
        &self,
        server: &str,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
        run: F,
    ) -> Result<T, ProxyCallError>
    where
        F: Fn(Peer<RoleClient>) -> Fut + Send,
        Fut: std::future::Future<Output = Result<T, RequestFailure>> + Send,
        T: Send,
    {
        // `if (isServerDisabled(deps.config.mcpServers[serverName])) throw …`. An absent entry is
        // NOT disabled — upstream's `isServerDisabled(undefined)` is false, and the "not connected"
        // error below is the one such a server gets.
        if self
            .state
            .config
            .mcp_servers
            .get(server)
            .is_some_and(ServerEntry::is_disabled)
        {
            return Err(ProxyCallError::Other(McpError::other(format!(
                "MCP server \"{server}\" is disabled"
            ))));
        }
        let Some(connection) = self.state.manager.get_connection(server) else {
            return Err(ProxyCallError::Other(McpError::other(format!(
                "Server \"{server}\" is not connected"
            ))));
        };
        // Captured BEFORE the call, which `isTerminatedSession`'s own doc requires in as many words:
        // catch-time transport state has already been replaced, so reading it there would classify
        // the wrong connection.
        let had_session_id = connection.has_session_id();

        let failure = match run(peer_of(&connection, server)?).await {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };
        if !crate::server_manager::is_terminated_session(&failure.evidence(), had_session_id) {
            return Err(ProxyCallError::Other(failure.error));
        }
        // "Re-read the live definition rather than reusing the stale connection's definition, in
        // case config changed since connect. If the server was removed from config in the meantime
        // there is nothing to reconnect to, so surface the original error."
        let Some(definition) = self.state.config.mcp_servers.get(server).cloned() else {
            return Err(ProxyCallError::Other(failure.error));
        };
        crate::abort::throw_if_aborted(cancel, None).map_err(ProxyCallError::Other)?;
        tracing::debug!("MCP session for \"{server}\" expired; reconnecting");

        let stale: crate::lifecycle::ConnectionHandle = connection;
        let mut fresh = self
            .state
            .manager
            .reconnect(server, &definition, &stale, Some(cancel))
            .await
            .map_err(ProxyCallError::Other)?;
        crate::abort::throw_if_aborted(cancel, None).map_err(ProxyCallError::Other)?;

        // `freshConnection = await onNeedsAuth?.(serverName) ?? freshConnection`. The callback is
        // MCP-162's ladder and it mutates the manager's table rather than returning a record, so the
        // re-read below is how its answer arrives.
        if fresh.status() == LinkStatus::NeedsAuth {
            recovery.recover().await?;
            crate::abort::throw_if_aborted(cancel, None).map_err(ProxyCallError::Other)?;
            if let Some(current) = self.state.manager.get_connection(server) {
                fresh = current;
            }
        }
        if fresh.status() == LinkStatus::NeedsAuth {
            return Err(ProxyCallError::SessionRecoveryAuthRequired {
                server: server.to_string(),
                auth_message: None,
            });
        }
        if fresh.status() != LinkStatus::Connected {
            return Err(ProxyCallError::Other(failure.error));
        }
        // Upstream wraps this in its own try/catch and logs, because a throwing listener must not
        // fail the call. Here it cannot throw and the `false` return means "a newer connection
        // replaced this one", which is not this caller's business either.
        let _published =
            self.state
                .manager
                .publish_metadata_changed(server, &fresh, "session-reconnect");
        crate::abort::throw_if_aborted(cancel, None).map_err(ProxyCallError::Other)?;
        run(peer_of(&fresh, server)?)
            .await
            .map_err(|failure| ProxyCallError::Other(failure.error))
    }

    /// `getRequestOptions(serverName, ownedSignal)`, with the signal half living in
    /// [`request_on_peer`] — see [`crate::server_manager::McpServerManager::get_request_options`]
    /// for why rmcp has nowhere to put it.
    ///
    /// Called once per *request* rather than once per call, because [`PeerRequestOptions`] is not
    /// `Clone` and the session-recovery retry needs its own. The value cannot drift between the two:
    /// `state.config` is this generation's committed snapshot and the manager's global default is
    /// set once at build.
    fn request_options(&self, server: &str) -> PeerRequestOptions {
        self.state
            .manager
            .get_request_options(server)
            .unwrap_or_else(PeerRequestOptions::no_options)
    }
}

#[async_trait::async_trait]
impl ProxyEnv for RuntimeEnv {
    // --- server-manager.ts -----------------------------------------------------------------------

    fn get_connection(&self, server: &str) -> Option<ConnectionStatus> {
        self.state
            .manager
            .get_connection(server)
            .map(|connection| proxy_status(connection.status()))
    }

    fn is_connecting(&self, server: &str) -> bool {
        self.state.manager.is_connecting(server)
    }

    async fn connect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome> {
        let definition = self.definition(server)?;
        let connection = self
            .state
            .manager
            .connect(server, &definition, Some(cancel))
            .await?;
        Ok(self.outcome_of(server, &connection))
    }

    async fn reconnect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome> {
        let definition = self.definition(server)?;
        // `state.manager.reconnect(server, definition, currentConnection, signal)` — upstream reaches
        // this only with a live record in hand (`proxy-modes.ts:775`'s `current?.status ===
        // "connected"` fork). With none there is nothing stale to tear down, and a plain connect is
        // what `reconnect` would degrade to anyway.
        let Some(current) = self.state.manager.get_connection(server) else {
            let connection = self
                .state
                .manager
                .connect(server, &definition, Some(cancel))
                .await?;
            return Ok(self.outcome_of(server, &connection));
        };
        let stale: crate::lifecycle::ConnectionHandle = current;
        let connection = self
            .state
            .manager
            .reconnect(server, &definition, &stale, Some(cancel))
            .await?;
        Ok(self.outcome_of(server, &connection))
    }

    async fn lazy_connect(&self, server: &str, cancel: &CancelToken) -> bool {
        lazy_connect(&self.state, &self.dirs, server, cancel).await
    }

    async fn close(&self, server: &str) {
        // `close(name)` reports a teardown failure; upstream's callers are all `await
        // state.manager.close(serverName)` with no catch, and the trait returns nothing, so the
        // failure is logged rather than lost.
        if let Err(error) = self.state.manager.close(server).await {
            tracing::debug!("MCP: close failed for {server}: {error}");
        }
    }

    fn touch(&self, server: &str) {
        self.state.manager.touch(server);
    }

    fn increment_in_flight(&self, server: &str) {
        self.state.manager.increment_in_flight(server);
    }

    fn decrement_in_flight(&self, server: &str) {
        self.state.manager.decrement_in_flight(server);
    }

    /// `withSessionRecovery(…, conn => abortable(conn.client.callTool({name, arguments}), signal))`
    /// (`proxy-modes.ts:1228-1240`) — **MCP-164**.
    ///
    /// `arguments` is always sent, an empty map included: upstream's `arguments: args ?? {}` puts the
    /// key on the wire unconditionally, and a server whose schema has no required properties still
    /// expects the member.
    ///
    /// The answer is turned into a [`CallToolOutcome`] by [`call_tool_outcome`], which owns the
    /// `result.isError` fork between the two content transforms.
    async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: JsonMap<String, Value>,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
    ) -> Result<CallToolOutcome, ProxyCallError> {
        let result = self
            .with_session_recovery(server, recovery, cancel, |peer| {
                let params =
                    CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.clone());
                let options = self.request_options(server);
                async move {
                    let response = request_on_peer(
                        &peer,
                        CallToolRequest::new(params).into(),
                        options,
                        cancel,
                    )
                    .await
                    .map_err(|failure| *failure)?;
                    match response {
                        ServerResult::CallToolResult(result) => Ok(result),
                        // SEP-2663 and MRTR are rmcp-era protocol extensions with no v2.26.1
                        // counterpart, and answering either needs a round this client never drives:
                        // `input_responses` for the first, `tasks/get` polling for the second. Both
                        // are reported rather than collapsed into an empty success, because an empty
                        // success is a tool result the model believes came from the server.
                        ServerResult::InputRequiredResult(_) => Err(RequestFailure::plain(format!(
                            "MCP tool \"{tool}\" on server \"{server}\" answered `input_required`; \
                             this client sends no in-call input responses"
                        ))),
                        ServerResult::CreateTaskResult(_) => Err(RequestFailure::plain(format!(
                            "MCP tool \"{tool}\" on server \"{server}\" deferred the call to a task; \
                             this client does not poll `tasks/get`"
                        ))),
                        _ => Err(RequestFailure::plain(format!(
                            "MCP server \"{server}\" answered `tools/call` for \"{tool}\" with a \
                             result that is not a CallToolResult"
                        ))),
                    }
                }
            })
            .await?;

        call_tool_outcome(server, &result)
    }

    /// `withSessionRecovery(…, conn => conn.client.readResource({uri}, requestOptions))`
    /// (`proxy-modes.ts:1198-1207`) — **MCP-164**.
    ///
    /// No `abortable(..)` wrapper, which is upstream's asymmetry reproduced rather than "fixed": the
    /// read is cancellable only through the request options' own signal, and in this port that is
    /// [`request_on_peer`]'s handle — the same place `call_tool`'s options-borne cancellation lives.
    /// The observable difference upstream (the outer promise settling ahead of the SDK's rejection)
    /// has no counterpart here, because the handle's cancel arm returns immediately either way.
    async fn read_resource(
        &self,
        server: &str,
        uri: &str,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
    ) -> Result<Vec<Content>, ProxyCallError> {
        let result = self
            .with_session_recovery(server, recovery, cancel, |peer| {
                let params = ReadResourceRequestParams::new(uri);
                let options = self.request_options(server);
                async move {
                    let response = request_on_peer(
                        &peer,
                        ReadResourceRequest::new(params).into(),
                        options,
                        cancel,
                    )
                    .await
                    .map_err(|failure| *failure)?;
                    match response {
                        ServerResult::ReadResourceResult(result) => Ok(result),
                        ServerResult::InputRequiredResult(_) => Err(RequestFailure::plain(format!(
                            "MCP server \"{server}\" answered `resources/read` for \"{uri}\" with \
                             `input_required`; this client sends no in-call input responses"
                        ))),
                        _ => Err(RequestFailure::plain(format!(
                            "MCP server \"{server}\" answered `resources/read` for \"{uri}\" with a \
                             result that is not a ReadResourceResult"
                        ))),
                    }
                }
            })
            .await?;

        resource_contents(server, &result)
    }

    async fn handle_url_elicitation_required(
        &self,
        server: &str,
        error: &rmcp::model::ErrorData,
    ) -> UrlElicitationAction {
        // `if (this.runtimeSignal?.aborted || !this.elicitationConfig?.allowUrl) return "cancel";`
        // (`server-manager.ts:801`). The owner check is this crate's spelling of the first half; the
        // manager's own method applies the runtime-signal and `allowUrl` halves and then runs the
        // loop, so a generation that stopped between the throw and here cancels at either gate.
        if !self.state.owner.is_active() {
            return UrlElicitationAction::Cancel;
        }
        self.state
            .manager
            .handle_url_elicitation_required(server, error)
            .await
    }

    // --- init.ts ---------------------------------------------------------------------------------

    fn failure_age_seconds(&self, server: &str) -> Option<u64> {
        failure_age_seconds(&self.state, server)
    }

    fn record_failure(&self, server: &str, message: &str) {
        record_failure(&self.state, server, message);
    }

    fn clear_failure(&self, server: &str) {
        clear_failure(&self.state, server);
    }

    fn update_status_bar(&self) {
        update_status_bar(&self.state);
    }

    fn update_server_metadata(&self, server: &str) {
        update_server_metadata(&self.state, server);
    }

    fn update_metadata_cache(&self, server: &str) {
        update_metadata_cache(
            &self.state,
            &self.dirs,
            server,
            MetadataCacheOptions::preserving(),
        );
    }

    fn mark_keep_alive_after_connect(&self, server: &str) {
        self.state.lifecycle.mark_keep_alive_after_connect(server);
    }

    fn commit_prompt_metadata(&self, server: &str) {
        commit_prompt_metadata(&self.state, server);
    }

    fn sync_tool_surface(&self) {
        if let Some(extension) = self.extension.upgrade() {
            let _ = extension.sync_tool_surface();
        }
    }

    // --- mcp-auth-flow.ts ------------------------------------------------------------------------

    fn supports_oauth(&self, definition: &ServerEntry) -> bool {
        crate::oauth::supports_oauth(definition)
    }

    /// MCP-084 — delegate, never mint a second copy: the config digest and the connect path have to
    /// agree about what a server's URL IS.
    fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>> {
        crate::credentials::resolve_server_url(
            definition.url.as_deref(),
            &crate::credentials::process_env(),
        )
    }

    async fn authenticate(
        &self,
        server: &str,
        server_url: &str,
        definition: &ServerEntry,
        cancel: &CancelToken,
    ) -> McpResult<()> {
        // Upstream never inspects the resolved status — `await authenticate(...)` and a throw is the
        // only failure channel (`proxy-modes.ts:165-177`).
        crate::oauth::authenticate(
            server,
            server_url,
            Some(definition),
            &self.auth_options(cancel),
        )
        .await
        .map(|_status| ())
    }

    async fn start_auth(
        &self,
        server: &str,
        server_url: &str,
        definition: &ServerEntry,
        cancel: &CancelToken,
    ) -> McpResult<Option<String>> {
        // `Ok(None)` is the client-credentials short-circuit, which completes synchronously and has
        // no authorization URL to hand back — `start_auth` spells that as an empty string.
        crate::oauth::start_auth(
            server,
            server_url,
            Some(definition),
            &self.auth_options(cancel),
        )
        .await
        .map(|url| (!url.is_empty()).then_some(url))
    }

    async fn complete_auth_from_input(
        &self,
        server: &str,
        input: &str,
        cancel: &CancelToken,
    ) -> McpResult<String> {
        crate::oauth::complete_auth_from_input(server, input, &self.auth_options(cancel))
            .await
            .map(|status| status.as_str().to_string())
    }

    // --- tool-metadata.ts / ts-shape.ts -----------------------------------------------------------

    /// **MCP-211, unscheduled and out of this group.** `formatSchema` is model-facing text and must
    /// not be improvised, so this answers a marker naming the unit rather than a plausible-looking
    /// rendering of a schema nobody ported a renderer for. The single place to fill is this body.
    fn format_schema(&self, _schema: &Value, _indent: &str) -> String {
        "(schema rendering is not wired — MCP-211)".to_string()
    }

    /// `None` is upstream's own real branch — the caller forks to `Parameters:` plus
    /// [`ProxyEnv::format_schema`] — so this is honest rather than a stub, and stays `None` until
    /// MCP-091 lands.
    fn render_ts_shape(&self, _schema: &Value) -> Option<String> {
        None
    }

    // --- tool-approval.ts ------------------------------------------------------------------------

    /// MCP-231 — the FREE function in `proxy/approval.rs`, not the [`crate::proxy::ProxyCtx`]
    /// wrapper: the ctx holds this env, so reaching back would cycle.
    fn is_tool_call_approval_required(&self, server: &str, tool: &ToolMetadata) -> bool {
        match self.state.tool_metadata.lock() {
            Ok(metadata) => crate::proxy::is_tool_call_approval_required(
                &self.state.config,
                server,
                tool,
                Some(&metadata),
            ),
            // A poisoned lock reaches the `tool_metadata == None` asymmetry `proxy/approval.rs`
            // documents honestly, rather than by guessing a map.
            Err(_) => {
                crate::proxy::is_tool_call_approval_required(&self.state.config, server, tool, None)
            }
        }
    }

    /// MCP-232 — the free function again, with the map **cloned**, unlike the sync arm: the gate
    /// awaits a human and a `std::sync::MutexGuard` cannot be held across an await.
    async fn ensure_tool_call_approved(
        &self,
        server: &str,
        tool: &ToolMetadata,
        arguments: &Value,
        origin: ApprovalOrigin,
        cancel: &CancelToken,
    ) -> ApprovalOutcome {
        let metadata = self
            .state
            .tool_metadata
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        crate::proxy::ensure_tool_call_approved(
            &self.state,
            server,
            tool,
            arguments,
            origin,
            cancel,
            &metadata,
        )
        .await
    }

    // --- mcp-output-guard.ts ---------------------------------------------------------------------

    /// MCP-225 — the whole wiring, through the bridge
    /// [`crate::renderers::McpOutputGuardOptions::from_resolved`] was built for. This is the crate's
    /// one production read of `$MCP_OUTPUT_GUARD`.
    async fn guard_mcp_output(
        &self,
        content: Vec<Content>,
        options: OutputGuardOptions,
    ) -> GuardedOutput {
        let resolved = self
            .state
            .config
            .settings_or_default()
            .output_guard(std::env::var("MCP_OUTPUT_GUARD").ok().as_deref());
        let mut guard_options = crate::renderers::McpOutputGuardOptions::from_resolved(resolved);
        guard_options.prefix = &options.prefix;
        guard_options.suffix = &options.suffix;
        guard_options.empty_text_fallback = options.empty_text_fallback.as_deref();
        guard_options.raw_mcp_result = options.raw_mcp_result.as_ref();

        let blocks = crate::renderers::McpContentBlock::vec_from_core(&content);
        let guarded = crate::renderers::guard_mcp_output(&blocks, &guard_options);
        GuardedOutput {
            mcp_result: guarded.mcp_result.clone(),
            output_guard: guarded.output_guard.clone(),
            content: guarded.into_core_content(),
        }
    }

    // --- pi.getAllTools() ------------------------------------------------------------------------

    /// `getPiTools?.()`. `None` is upstream's optional-parameter branch, NOT a defect: never
    /// synthesise a built-in name list as a floor.
    fn all_tool_names(&self) -> Option<Vec<String>> {
        self.state
            .ui
            .as_ref()
            .and_then(|ui| cyrup_ext::HostServices::all_tool_names(ui.as_ref()))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The two enums are the same three states declared twice, and [`proxy_status`] is the only
    /// bridge. A fourth state on either side has to fail here rather than mistranslate.
    #[test]
    fn the_two_connection_status_enums_map_across() {
        assert_eq!(
            proxy_status(LinkStatus::Connected),
            ConnectionStatus::Connected
        );
        assert_eq!(proxy_status(LinkStatus::Closed), ConnectionStatus::Closed);
        assert_eq!(
            proxy_status(LinkStatus::NeedsAuth),
            ConnectionStatus::NeedsAuth
        );
    }

    /// `message.slice(0, MAX_FAILURE_MESSAGE_CHARS)` (`init.ts:66`) — upstream slices UTF-16 code
    /// units and is safe only because the string is ASCII in practice. A hostile server's stderr is
    /// not, so the cut walks back to the nearest boundary instead of splitting a character.
    #[test]
    fn a_failure_message_is_cut_on_a_char_boundary() {
        assert_eq!(truncate_failure_message("already short"), "already short");

        // A four-byte character straddling the cap.
        let mut message = "a".repeat(MAX_FAILURE_MESSAGE_CHARS - 1);
        message.push('\u{1f600}');
        let cut = truncate_failure_message(&message);
        assert_eq!(cut.len(), MAX_FAILURE_MESSAGE_CHARS - 1);
        assert!(
            cut.chars().all(|c| c == 'a'),
            "the split character is dropped whole"
        );
    }

    /// `parallelLimit`'s first property: results come back **by original index**, whatever order
    /// the futures actually complete in. `init.ts:305`/`:327` walk `results` twice and `:382`
    /// filters against it by name, so an unordered port would silently misattribute every connect
    /// result.
    #[tokio::test]
    async fn parallel_limit_yields_results_in_input_order() {
        // Item `i` yields `8 - i` times, so the LAST item finishes first.
        let out = parallel_limit((0usize..8).collect(), 4, |i| async move {
            for _ in 0..(8 - i) {
                tokio::task::yield_now().await;
            }
            i
        })
        .await;
        assert_eq!(out, (0usize..8).collect::<Vec<_>>());
    }

    /// `parallelLimit`'s second property: at most `limit` in flight.
    #[tokio::test]
    async fn parallel_limit_never_exceeds_its_limit() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let out = parallel_limit((0usize..20).collect(), 3, |i| {
            let (in_flight, peak) = (Arc::clone(&in_flight), Arc::clone(&peak));
            async move {
                let now = in_flight.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(now, Ordering::AcqRel);
                tokio::task::yield_now().await;
                in_flight.fetch_sub(1, Ordering::AcqRel);
                i
            }
        })
        .await;
        assert_eq!(out.len(), 20);
        assert!(
            peak.load(Ordering::Acquire) <= 3,
            "at most `limit` futures may be in flight"
        );
    }

    /// A `limit` of 0 would stall `buffered` outright; upstream's callers only ever pass 10, but
    /// the floor is what keeps a future caller's computed limit from hanging a startup.
    #[tokio::test]
    async fn parallel_limit_of_zero_still_makes_progress() {
        let out = parallel_limit(vec![1usize, 2, 3], 0, |i| async move { i * 2 }).await;
        assert_eq!(out, vec![2, 4, 6]);
    }

    /// Upstream's `{}` reads `preserveEmptyResources !== false`, i.e. **preserve**. The named
    /// constructor exists so no call site can mean the other thing by omission.
    #[test]
    fn the_default_metadata_cache_options_preserve() {
        assert!(MetadataCacheOptions::preserving().preserve_empty_resources);
    }

    // ---- MCP-164 · the two live requests, against a real child process -------------------------

    /// A real stdio MCP server as an `sh` script, in the shape `runtime.rs`'s `TINY_MCP` and
    /// `server_manager.rs`'s child-process tests already use — no host dependency this suite does
    /// not already have.
    ///
    /// It answers **four** things that matter here and nothing else does: `tools/call` for `echo`
    /// with a real payload (`echoed:<text>`), `tools/call` for `boom` with `isError: true`,
    /// `tools/call` for `structured` with an EMPTY `content` and a `structuredContent` object — the
    /// one case that separates `resolveMcpResultContent` from `transformMcpContent` — and
    /// `resources/read` with a `contents` entry. The three `*/list` arms are MCP-119's, which runs
    /// during connect.
    ///
    /// It **echoes back the protocol version the client asked for** rather than naming one, so it
    /// negotiates like a real server instead of pinning this test to a version constant.
    const LIVE_ECHO: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$id" "$pv"
      ;;
    *'"method":"notifications/'*) : ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
    *'"method":"resources/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resources":[]}}\n' "$id"
      ;;
    *'"method":"prompts/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"prompts":[]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      case "$line" in
        *'"name":"boom"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"it went wrong"}],"isError":true}}\n' "$id"
          ;;
        *'"name":"structured"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[],"structuredContent":{"ok":true}}}\n' "$id"
          ;;
        *)
          text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed:%s"}],"isError":false}}\n' "$id" "$text"
          ;;
      esac
      ;;
    *'"method":"resources/read"'*)
      uri=$(printf '%s' "$line" | sed -n 's/.*"uri":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"contents":[{"uri":"%s","mimeType":"text/plain","text":"the fixture resource"}]}}\n' "$id" "$uri"
      ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

    fn live_entry() -> ServerEntry {
        ServerEntry {
            command: Some("sh".to_string()),
            args: Some(vec![
                "-c".to_string(),
                LIVE_ECHO.to_string(),
                "sh".to_string(),
            ]),
            ..ServerEntry::default()
        }
    }

    /// A live [`Peer`] on a real child process running [`LIVE_ECHO`], built by the **production**
    /// connection factory.
    ///
    /// The factory rather than [`crate::server_manager::McpServerManager::connect`], because these
    /// tests are about the REQUEST and nothing else: no state, no config, no metadata map, no
    /// resolution. [`a_model_issued_tool_call_returns_the_servers_own_result`] is the one that goes
    /// through the manager, and it is where the whole chain is asserted.
    ///
    /// The attempt token and the resource are both returned rather than dropped: the token is
    /// rmcp's service-loop cancellation token, and the resource owns the `RunningService` — dropping
    /// either takes the child with it.
    async fn live_peer() -> (
        CancelToken,
        Arc<dyn crate::server_manager::ConnectionResource>,
        Peer<RoleClient>,
    ) {
        use crate::server_manager::ConnectionFactory as _;
        let attempt = CancelToken::new();
        let made = crate::runtime::ConnectionBuilder::new(None)
            .create(crate::server_manager::CreateConnection {
                trace: None,
                name: "fixture".to_string(),
                definition: Arc::new(live_entry()),
                attempt: attempt.clone(),
                request: CancelToken::new(),
                credentials_invalidated: false,
                request_options: None,
            })
            .await
            .expect("the fixture connects");
        let peer = made
            .resource
            .peer()
            .cloned()
            .expect("the keystone: a connected resource hands its caller the live peer");
        (attempt, made.resource, peer)
    }

    async fn call(peer: &Peer<RoleClient>, tool: &str, arguments: Value) -> CallToolResult {
        let params = CallToolRequestParams::new(tool.to_string())
            .with_arguments(arguments.as_object().cloned().unwrap_or_default());
        let response = request_on_peer(
            peer,
            CallToolRequest::new(params).into(),
            PeerRequestOptions::no_options(),
            &CancelToken::new(),
        )
        .await
        .expect("the server answered");
        match response {
            ServerResult::CallToolResult(result) => result,
            other => panic!("expected a CallToolResult, got {other:?}"),
        }
    }

    fn text_of(content: &[Content]) -> String {
        match content.first() {
            Some(Content::Text { text, .. }) => text.to_string(),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    /// **MCP-164, the deliverable.** A `tools/call` reaches a real MCP server over a real pipe and
    /// that server's own answer comes back as the content `executeCall` will hand the model.
    ///
    /// `echoed:pong` exists nowhere in this crate: it can only have come off the child's stdout.
    #[tokio::test]
    async fn a_tool_call_reaches_a_real_server_and_its_own_answer_comes_back() {
        let (_attempt, _resource, peer) = live_peer().await;

        let result = call(&peer, "echo", serde_json::json!({ "text": "pong" })).await;
        let outcome = call_tool_outcome("fixture", &result).expect("the result reads");

        assert!(!outcome.is_error, "the server said isError:false");
        assert_eq!(text_of(&outcome.content), "echoed:pong");
        // `rawMcpResult` — what the output guard stores as `details.mcpResult`.
        let raw = outcome.raw.expect("the raw result rides along");
        assert_eq!(raw["content"][0]["text"], serde_json::json!("echoed:pong"));
    }

    /// `arguments` is always on the wire, an empty map included: upstream's `arguments: args ?? {}`
    /// writes the key unconditionally. The fixture's `sed` finds no `"text"` and echoes the empty
    /// string, which is only reachable if the request was well-formed and the tool really ran.
    #[tokio::test]
    async fn a_call_with_no_arguments_still_sends_an_arguments_object() {
        let (_attempt, _resource, peer) = live_peer().await;

        let result = call(&peer, "echo", Value::Null).await;
        let outcome = call_tool_outcome("fixture", &result).expect("the result reads");

        assert_eq!(text_of(&outcome.content), "echoed:");
    }

    /// `result.isError` is carried as a discriminator, not folded into a transport failure: a tool
    /// that legitimately fails is a *result*, and `executeCall`'s path 2 renders it with the guard's
    /// `Error: ` prefix and `details.error == "tool_error"`.
    #[tokio::test]
    async fn a_tool_that_answers_is_error_is_reported_as_an_error_result() {
        let (_attempt, _resource, peer) = live_peer().await;

        let result = call(&peer, "boom", Value::Null).await;
        let outcome = call_tool_outcome("fixture", &result).expect("the result reads");

        assert!(outcome.is_error, "the discriminator survives");
        assert_eq!(
            text_of(&outcome.content),
            "it went wrong",
            "the server's own text"
        );
    }

    /// An empty `content` with a `structuredContent` object is `resolveMcpResultContent`'s fallback,
    /// and it is the one case that proves the SUCCESS path uses that function rather than
    /// `transformMcpContent` — which would answer with nothing and let `executeCall` substitute
    /// `(empty result)`.
    #[tokio::test]
    async fn an_empty_content_falls_back_to_structured_content() {
        let (_attempt, _resource, peer) = live_peer().await;

        let result = call(&peer, "structured", Value::Null).await;
        let outcome = call_tool_outcome("fixture", &result).expect("the result reads");

        let text = text_of(&outcome.content);
        assert!(
            text.contains("\"ok\""),
            "the structured payload, got {text:?}"
        );
        assert!(
            text.contains("true"),
            "the structured payload, got {text:?}"
        );
    }

    /// The error path takes the OTHER transform: `transformMcpContent` never consults
    /// `structuredContent`, so an errored result with empty content stays empty and `executeCall`
    /// substitutes `(empty result)` under the `Error: ` prefix.
    ///
    /// Pinned against a hand-built result rather than the fixture because it is the *negative* of
    /// the test above, and the two together are what make the fork observable at all.
    #[test]
    fn the_error_path_does_not_consult_structured_content() {
        let mut result = CallToolResult::success(Vec::new());
        result.is_error = Some(true);
        result.structured_content = Some(serde_json::json!({ "ok": true }));

        let outcome = call_tool_outcome("fixture", &result).expect("the result reads");

        assert!(outcome.is_error);
        assert!(
            outcome.content.is_empty(),
            "no fallback on the error path: {:?}",
            outcome.content
        );
    }

    /// `resources/read` on the live peer, through the same handle-and-cancel path `tools/call` uses,
    /// and `transformMcpResourceContents` on the answer.
    #[tokio::test]
    async fn a_resource_read_reaches_the_live_server() {
        let (_attempt, _resource, peer) = live_peer().await;

        let response = request_on_peer(
            &peer,
            ReadResourceRequest::new(ReadResourceRequestParams::new("file:///fixture.txt")).into(),
            PeerRequestOptions::no_options(),
            &CancelToken::new(),
        )
        .await
        .expect("the server answered");
        let ServerResult::ReadResourceResult(result) = response else {
            panic!("expected a ReadResourceResult, got {response:?}");
        };
        let content = resource_contents("fixture", &result).expect("the result reads");

        assert_eq!(text_of(&content), "the fixture resource");
    }

    /// An already-cancelled token never lets the request take a poll — `abortable`'s `biased` arm,
    /// hoisted ahead of the send — and the failure it produces is an **abort**.
    ///
    /// [`McpError::Aborted`] rather than [`McpError::Other`] is load-bearing:
    /// `crate::abort::is_abort_error` is what turns this into `details.error == "aborted"` instead of
    /// `"call_failed"`, and a user who stopped a generation must not see a tool reported as broken.
    #[tokio::test]
    async fn an_already_cancelled_token_aborts_before_the_request_is_sent() {
        let (_attempt, _resource, peer) = live_peer().await;
        let cancel = CancelToken::new();
        cancel.cancel();

        let params = CallToolRequestParams::new("echo").with_arguments(JsonMap::new());
        let failure = request_on_peer(
            &peer,
            CallToolRequest::new(params).into(),
            PeerRequestOptions::no_options(),
            &cancel,
        )
        .await
        .expect_err("a cancelled token never returns a result");

        assert!(
            matches!(failure.error, McpError::Aborted(_)),
            "got {:?}",
            failure.error
        );
        // Evidence-free: an abort is never a terminated session, so it can never trigger a retry —
        // which would double-execute a tool the user just stopped.
        assert!(!crate::server_manager::is_terminated_session(
            &failure.evidence(),
            true
        ));
    }

    /// A JSON-RPC error keeps the server's message **bare** and carries the code out beside it.
    ///
    /// `ProtocolError`'s constructor is `super(message)`, so `executeCall`'s catch renders
    /// `Failed to call tool: Unknown tool` upstream — not `Failed to call tool: MCP error -32601:
    /// Unknown tool`. The code still has to survive, because `isTerminatedSession` classifies on it.
    #[test]
    fn a_json_rpc_error_keeps_its_bare_message_and_carries_the_code_beside_it() {
        let failure = RequestFailure::from_service(ServiceError::McpError(rmcp::ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            "Unknown tool",
            None,
        )));

        assert_eq!(failure.error.to_string(), "Unknown tool");
        assert_eq!(failure.evidence().message, "Unknown tool");
        assert_eq!(failure.evidence().protocol_code, Some(-32601));
        assert_eq!(failure.evidence().http_status, None);
    }

    /// `isTerminatedSession`'s protocol arm, reached through the mapping this module owns: a
    /// `-32000 Server not initialized` on a connection that HAD a session id is the spec's
    /// "your session is gone", and it is what makes [`RuntimeEnv::with_session_recovery`] reconnect
    /// once instead of surfacing the failure.
    ///
    /// The `had_session_id == false` row is the gate that keeps stdio — which has no session — out of
    /// the retry path entirely.
    #[test]
    fn a_terminated_session_is_recognised_through_the_mapping() {
        let failure = RequestFailure::from_service(ServiceError::McpError(rmcp::ErrorData::new(
            rmcp::model::ErrorCode(-32000),
            "Server not initialized",
            None,
        )));

        assert!(crate::server_manager::is_terminated_session(
            &failure.evidence(),
            true
        ));
        assert!(!crate::server_manager::is_terminated_session(
            &failure.evidence(),
            false
        ));
    }

    /// rmcp's `Display` for `StreamableHttpError::SessionExpired` is read off the type, not copied,
    /// so a change to that text cannot silently stop [`session_expired_status`] from matching.
    #[test]
    fn the_session_expired_marker_is_taken_from_rmcp() {
        assert_eq!(
            SESSION_EXPIRED_DISPLAY.as_str(),
            "Session expired (HTTP 404)"
        );
    }

    /// **The whole chain, in one test: a model-issued tool call gets a real server's real answer.**
    ///
    /// `executeCall` over the one production [`ProxyEnv`], against the same fixture the tests above
    /// drive directly — but through [`crate::server_manager::McpServerManager`] this time, which is
    /// the path a model's `mcp({tool: …})` actually takes: name resolution, the approval gate,
    /// `lazyConnect`, [`RuntimeEnv::call_tool`], the live peer, the content transform and the output
    /// guard. `echoed:pong` exists nowhere in this crate; it can only have come off the child's
    /// stdout.
    ///
    /// # Why the catalog is seeded by hand
    ///
    /// Filling `state.toolMetadata` from a live `tools/list` is MCP-119's, and `initializeMcp`'s
    /// startup pass is what does it in production. Seeding it here keeps MCP-164 provable on its
    /// own: the metadata is the *input* to `executeCall`'s resolution, and where it came from
    /// changes nothing about whether the request reaches the server.
    ///
    /// # The one thing this test catches that none of the others can
    ///
    /// It went red first, with `details.error == "call_failed"` and
    /// `MCP transport closed before the request was answered`: `AttemptSlot::drop` reaps the attempt
    /// token, and `runtime.rs` was handing that same token to rmcp as
    /// `serve_client_with_lifecycle_and_ct`'s `ct` — which rmcp keeps as the **service loop's**
    /// lifetime (`rmcp-3.1.4/src/service.rs:1431`). Every connection the manager returned had a dead
    /// service behind a live-looking `Peer`, and nothing could see it until something issued a
    /// request. [`crate::runtime::connect_client_bounded`]'s `detachable_from` is what scopes the
    /// connect signal to the connect now.
    ///
    /// So: keep this one through the manager rather than through the factory. Every other test in
    /// this section holds its own attempt token and would keep passing if that scoping were lost.
    #[tokio::test]
    async fn a_model_issued_tool_call_returns_the_servers_own_result() {
        let temp = tempfile::TempDir::new().unwrap();
        let entry = live_entry();
        let manager = Arc::new(crate::server_manager::McpServerManager::with_factory(
            None,
            Arc::new(crate::runtime::ConnectionBuilder::new(None)),
        ));
        let lifecycle = Arc::new(crate::lifecycle::McpLifecycleManager::new(
            Arc::clone(&manager),
            Arc::new(|_: &str| false),
        ));
        let state = Arc::new(McpState::new(crate::state::McpStateParts {
            owner: Arc::new(crate::owner::McpRuntimeOwner::new()),
            manager,
            lifecycle,
            config: crate::proxy::testsupport::config_with(&[("fixture", entry.clone())]),
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::state::AuthStorageOptions::default(),
            ui: None,
            open_browser: Arc::new(|_| Box::pin(async { Ok(()) })),
            send_message: Arc::new(|_| {}),
        }));
        let connection = state
            .manager
            .connect("fixture", &entry, None)
            .await
            .expect("the fixture connects");
        assert_eq!(
            connection.status(),
            LinkStatus::Connected,
            "a live child, past the handshake"
        );
        assert!(
            connection.resource().peer().is_some(),
            "and a peer to talk to it with"
        );

        state.tool_metadata.lock().unwrap().insert(
            "fixture".to_string(),
            vec![ToolMetadata::new("fixture_echo", "echo", "echo")],
        );
        let dirs = McpDirs::new(temp.path().to_path_buf(), temp.path().to_path_buf());
        let ctx = crate::proxy::ProxyCtx::new(
            Arc::clone(&state),
            Arc::new(RuntimeEnv::new(
                Arc::clone(&state),
                dirs,
                std::sync::Weak::new(),
            )),
        );

        let result = crate::proxy::execute_call(
            &ctx,
            "fixture_echo",
            Some(&serde_json::json!({ "text": "pong" })),
            None,
            &CancelToken::new(),
            None,
        )
        .await
        .expect("the call completes");

        let details = result.details.clone().expect("details");
        assert_eq!(
            details.get("error"),
            None,
            "a real result, not a failure: {details}"
        );
        assert_eq!(details["server"], serde_json::json!("fixture"));
        assert_eq!(
            details["tool"],
            serde_json::json!("echo"),
            "the ORIGINAL name went on the wire"
        );
        assert_eq!(
            text_of(&result.content),
            "echoed:pong",
            "the server's own answer"
        );
        assert_eq!(
            details["mcpResult"]["content"][0]["text"],
            serde_json::json!("echoed:pong"),
            "`rawMcpResult` rode out with it: {details}"
        );
    }
}
