//! The discovery modes — `status`, `list`, `instructions`, `describe` and
//! `search`. MCP-154…MCP-160, MCP-177.
//!
//! See [`crate::proxy`] for the module overview.

use std::collections::BTreeMap;

use serde_json::{json, Map as JsonMap, Value};

use cyrup_core::{ToolResult};

use crate::config::ServerEntry;
use crate::proxy::constants::{INSTRUCTIONS_PREVIEW_LENGTH, MAX_REGEX_SEARCH_QUERY_LENGTH, REGEX_DFA_SIZE_LIMIT, REGEX_SIZE_LIMIT};
use crate::proxy::env::{ConnectionStatus, ProxyCtx};
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::ranking::{RankedToolMatch, paginate, rank_collate, rank_tool_matches, resolve_search_keywords};
use crate::proxy::results::{ambiguous_tool_result, details, details_err, disabled_result, get_enabled_tool_matches, not_found_result, text_result};
use crate::proxy::tool_metadata::{ToolMetadata, find_tool_by_name, truncate_at_word};

// ==================================================================================================
// 6 · Discovery modes — `status`, `list`, `instructions` (MCP-154, MCP-155, MCP-156)
// ==================================================================================================

/// `proxy-modes.ts:277` `executeStatus(state)`.
///
/// Per server key of `config.mcpServers`, **in insertion order**, the status is computed by this
/// six-rung ladder: `disabled` → `connected` → `needs-auth` → `failed` (when the failure age is
/// non-null) → `cached` (metadata present) → `not connected`. `metadata` and `connection` are forced
/// absent and `failedAgo` to `null` for a disabled server.
///
/// The header counts **enabled servers only**; the glyphs `⊘ ✓ ⚠ ○ ✗` are literal and must not be
/// substituted. `details.servers[i].disabled` is present **only when true**.
#[must_use]
pub fn execute_status(ctx: &ProxyCtx) -> ToolResult {
    #[derive(Clone)]
    struct Row {
        name: String,
        status: &'static str,
        tool_count: usize,
        failed_ago: Option<u64>,
        disabled: bool,
    }

    let mut servers: Vec<Row> = Vec::new();
    for name in ctx.config().mcp_servers.keys() {
        let disabled = ctx.is_disabled(name);
        let connection = if disabled { None } else { ctx.env.get_connection(name) };
        let tool_count = if disabled {
            0
        } else {
            ctx.with_metadata(|metadata| metadata.get(name).map(Vec::len)).unwrap_or(0)
        };
        let has_metadata = !disabled && ctx.with_metadata(|metadata| metadata.contains_key(name));
        let failed_ago = if disabled { None } else { ctx.env.failure_age_seconds(name) };

        let status = if disabled {
            "disabled"
        } else if connection == Some(ConnectionStatus::Connected) {
            "connected"
        } else if connection == Some(ConnectionStatus::NeedsAuth) {
            "needs-auth"
        } else if failed_ago.is_some() {
            "failed"
        } else if has_metadata {
            "cached"
        } else {
            "not connected"
        };

        servers.push(Row { name: name.clone(), status, tool_count, failed_ago, disabled });
    }

    let disabled_count = servers.iter().filter(|row| row.disabled).count();
    let enabled: Vec<&Row> = servers.iter().filter(|row| !row.disabled).collect();
    let total_tools: usize = enabled.iter().map(|row| row.tool_count).sum();
    let connected_count = enabled.iter().filter(|row| row.status == "connected").count();

    let mut text = format!("MCP: {connected_count}/{} servers, {total_tools} tools", enabled.len());
    if disabled_count > 0 {
        text.push_str(&format!(" ({disabled_count} disabled)"));
    }
    text.push_str("\n\n");
    for row in &servers {
        let name = &row.name;
        if row.disabled {
            text.push_str(&format!("⊘ {name} (disabled)\n"));
            continue;
        }
        match row.status {
            "connected" => text.push_str(&format!("✓ {name} ({} tools)\n", row.tool_count)),
            "needs-auth" => text.push_str(&format!("⚠ {name} (needs auth)\n")),
            "cached" => text.push_str(&format!("○ {name} ({} tools, cached)\n", row.tool_count)),
            "failed" => {
                text.push_str(&format!("✗ {name} (failed {}s ago)\n", row.failed_ago.unwrap_or(0)));
            }
            _ => text.push_str(&format!("○ {name} (not connected)\n")),
        }
    }
    if !servers.is_empty() {
        text.push_str("\nmcp({ server: \"name\" }) to list tools, mcp({ search: \"...\" }) to search");
    }

    let rows: Vec<Value> = servers
        .iter()
        .map(|row| {
            let mut entry = JsonMap::new();
            entry.insert("name".to_string(), Value::String(row.name.clone()));
            entry.insert("status".to_string(), Value::String(row.status.to_string()));
            entry.insert("toolCount".to_string(), json!(row.tool_count));
            entry.insert(
                "failedAgo".to_string(),
                row.failed_ago.map_or(Value::Null, |seconds| json!(seconds)),
            );
            if row.disabled {
                entry.insert("disabled".to_string(), Value::Bool(true));
            }
            Value::Object(entry)
        })
        .collect();

    let mut map = details("status");
    map.insert("servers".to_string(), Value::Array(rows));
    map.insert("totalTools".to_string(), json!(total_tools));
    map.insert("connectedCount".to_string(), json!(connected_count));
    map.insert("disabledCount".to_string(), json!(disabled_count));
    text_result(text.trim().to_string(), map)
}

/// `proxy-modes.ts:633` `executeList(state, server)`.
///
/// Five outcomes, three of them for the zero-tool case, each with a distinct `details` shape. The
/// `Use mcp({ instructions: … }) for the full text.` pointer appears **only when the 300-character
/// preview actually truncated**.
#[must_use]
pub fn execute_list(ctx: &ProxyCtx, server: &str) -> ToolResult {
    if !ctx.config().mcp_servers.contains_key(server) {
        let mut map = details_err("list", McpErrorCode::NotFound);
        map.insert("server".to_string(), Value::String(server.to_string()));
        map.insert("tools".to_string(), Value::Array(Vec::new()));
        map.insert("count".to_string(), json!(0));
        return text_result(
            format!("Server \"{server}\" not found. Use mcp({{}}) to see available servers."),
            map,
        );
    }
    if ctx.is_disabled(server) {
        return disabled_result("list", server);
    }

    let metadata: Option<Vec<ToolMetadata>> =
        ctx.with_metadata(|map| map.get(server).cloned());
    let tool_names: Vec<String> =
        metadata.as_ref().map(|tools| tools.iter().map(|tool| tool.name.clone()).collect()).unwrap_or_default();
    let connection = ctx.env.get_connection(server);
    // `Boolean(instructions)` — an empty string is falsy upstream, so it neither renders the
    // preview block nor sets `hasInstructions`.
    let instructions = ctx.server_instructions(server).filter(|text| !text.is_empty());

    let mut instructions_text = String::new();
    if let Some(instructions) = instructions.as_ref() {
        let preview = truncate_at_word(instructions, INSTRUCTIONS_PREVIEW_LENGTH);
        instructions_text = format!("\n\nServer instructions:\n{preview}");
        if &preview != instructions {
            instructions_text.push_str(&format!("\nUse mcp({{ instructions: \"{server}\" }}) for the full text."));
        }
    }
    let has_instructions = instructions.is_some();

    if tool_names.is_empty() {
        if connection == Some(ConnectionStatus::Connected) {
            let mut map = details("list");
            map.insert("server".to_string(), Value::String(server.to_string()));
            map.insert("tools".to_string(), Value::Array(Vec::new()));
            map.insert("count".to_string(), json!(0));
            map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
            return text_result(format!("Server \"{server}\" has no tools.{instructions_text}"), map);
        }
        if metadata.is_some() {
            let mut map = details("list");
            map.insert("server".to_string(), Value::String(server.to_string()));
            map.insert("tools".to_string(), Value::Array(Vec::new()));
            map.insert("count".to_string(), json!(0));
            map.insert("cached".to_string(), Value::Bool(true));
            map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
            return text_result(
                format!("Server \"{server}\" has no cached tools (not connected).{instructions_text}"),
                map,
            );
        }
        let mut map = details_err("list", McpErrorCode::NotConnected);
        map.insert("server".to_string(), Value::String(server.to_string()));
        map.insert("tools".to_string(), Value::Array(Vec::new()));
        map.insert("count".to_string(), json!(0));
        map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
        return text_result(
            format!("Server \"{server}\" is configured but not connected. Use mcp({{ connect: \"{server}\" }}) or /mcp reconnect {server} to retry.{instructions_text}"),
            map,
        );
    }

    let cached_note = if connection == Some(ConnectionStatus::Connected) { "" } else { " (not connected, cached)" };
    let mut text = format!("{server} ({} tools{cached_note}):\n\n", tool_names.len());
    let descriptions: BTreeMap<String, String> = metadata
        .as_ref()
        .map(|tools| tools.iter().map(|tool| (tool.name.clone(), tool.description.clone())).collect())
        .unwrap_or_default();
    for tool in &tool_names {
        let description = descriptions.get(tool).map(String::as_str).unwrap_or_default();
        let truncated = truncate_at_word(description, 50);
        text.push_str(&format!("- {tool}"));
        if !truncated.is_empty() {
            text.push_str(&format!(" - {truncated}"));
        }
        text.push('\n');
    }
    text.push_str(&instructions_text);

    let mut map = details("list");
    map.insert("server".to_string(), Value::String(server.to_string()));
    map.insert(
        "tools".to_string(),
        Value::Array(tool_names.iter().map(|name| Value::String(name.clone())).collect()),
    );
    map.insert("count".to_string(), json!(tool_names.len()));
    map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
    text_result(text.trim().to_string(), map)
}

/// `proxy-modes.ts:700` `executeInstructions(state, server)`.
///
/// Five outcomes checked in this order: `not_found`, `server_disabled`, cached instructions,
/// `no_instructions` (connected and the server declared none), `not_connected`.
/// **Cached instructions win even for a disconnected server** — the connection is only consulted
/// once the cache has already missed.
#[must_use]
pub fn execute_instructions(ctx: &ProxyCtx, server: &str) -> ToolResult {
    if !ctx.config().mcp_servers.contains_key(server) {
        return not_found_result("instructions", server);
    }
    if ctx.is_disabled(server) {
        return disabled_result("instructions", server);
    }

    if let Some(instructions) = ctx.server_instructions(server).filter(|text| !text.is_empty()) {
        let mut map = details("instructions");
        map.insert("server".to_string(), Value::String(server.to_string()));
        // JS `.length` is UTF-16 code units; `chars().count()` is the closest honest analogue and
        // this value is diagnostic only.
        map.insert("length".to_string(), json!(instructions.chars().count()));
        return text_result(format!("{server} instructions:\n\n{instructions}"), map);
    }

    if ctx.env.get_connection(server) == Some(ConnectionStatus::Connected) {
        let mut map = details_err("instructions", McpErrorCode::NoInstructions);
        map.insert("server".to_string(), Value::String(server.to_string()));
        return text_result(format!("Server \"{server}\" does not provide instructions."), map);
    }

    let mut map = details_err("instructions", McpErrorCode::NotConnected);
    map.insert("server".to_string(), Value::String(server.to_string()));
    text_result(
        format!("No instructions cached for \"{server}\". Use mcp({{ connect: \"{server}\" }}) to connect and refresh."),
        map,
    )
}

// ==================================================================================================
// 7 · `executeDescribe` (MCP-157)
// ==================================================================================================

/// `proxy-modes.ts:434` `executeDescribe(state, toolName)`.
///
/// 1. **Ambiguity first**, before any resolution: `>1` exact enabled match is ambiguous; otherwise
///    `0` exact and `>1` fuzzy is ambiguous.
/// 2. The single exact match; if none, walk `state.toolMetadata` in insertion order with
///    `findToolByName`, remembering the **first** disabled hit (`??=`) and breaking on the first
///    enabled hit.
/// 3. No hit: a disabled server that matched is reported as `server_disabled` rather than
///    `tool_not_found`; otherwise ranked suggestions.
/// 4. Render. Note `formatSchema` is called here with the **default** indent, unlike
///    [`execute_search`]'s `"    "`.
#[must_use]
pub fn execute_describe(ctx: &ProxyCtx, tool_name: &str) -> ToolResult {
    let resolved = ctx.with_metadata(|metadata| {
        let exact = get_enabled_tool_matches(ctx.config(), metadata, tool_name, true);
        if exact.len() > 1 {
            return Err(true);
        }
        if exact.is_empty()
            && get_enabled_tool_matches(ctx.config(), metadata, tool_name, false).len() > 1
        {
            return Err(true);
        }

        let mut server_name = exact.first().map(|(server, _)| server.clone());
        let mut tool_meta = exact.first().map(|(_, tool)| tool.clone());
        let mut disabled_match: Option<String> = None;

        if tool_meta.is_none() {
            for (server, tools) in metadata {
                let Some(found) = find_tool_by_name(tools, tool_name) else { continue };
                if ctx.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
                    // `??=` — the FIRST disabled hit is remembered and the scan continues.
                    disabled_match.get_or_insert_with(|| server.clone());
                    continue;
                }
                server_name = Some(server.clone());
                tool_meta = Some(found.clone());
                break;
            }
        }
        Ok((server_name, tool_meta, disabled_match))
    });

    let (server_name, tool_meta, disabled_match) = match resolved {
        Err(_) => return ambiguous_tool_result("describe", tool_name),
        Ok(triple) => triple,
    };

    let (Some(server_name), Some(tool_meta)) = (server_name, tool_meta) else {
        if let Some(disabled) = disabled_match {
            return disabled_result("describe", &disabled);
        }
        let suggestions = ctx.suggestions(tool_name, 5);
        let suggestion_text = if suggestions.is_empty() {
            String::new()
        } else {
            format!(" Did you mean: {}", suggestions.join(", "))
        };
        let mut map = details_err("describe", McpErrorCode::ToolNotFound);
        map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
        map.insert(
            "suggestions".to_string(),
            Value::Array(suggestions.iter().map(|name| Value::String(name.clone())).collect()),
        );
        return text_result(
            format!("Tool \"{tool_name}\" not found. Use mcp({{ search: \"...\" }}) to search.{suggestion_text}"),
            map,
        );
    };

    let approval_marker = if ctx.env.is_tool_call_approval_required(&server_name, &tool_meta) {
        " (requires approval)"
    } else {
        ""
    };
    let mut text = format!("{}{approval_marker}\n", tool_meta.name);
    text.push_str(&format!("Server: {server_name}\n"));
    if let Some(uri) = tool_meta.resource_uri.as_ref() {
        text.push_str(&format!("Type: Resource (reads from {uri})\n"));
    }
    let description =
        if tool_meta.description.is_empty() { "(no description)" } else { tool_meta.description.as_str() };
    text.push_str(&format!("\n{description}\n"));

    match (tool_meta.input_schema.as_ref(), tool_meta.resource_uri.as_ref()) {
        (Some(schema), None) => match ctx.env.render_ts_shape(schema) {
            // `renderTsShape` returning null is the fork to the long-form printer.
            None => text.push_str(&format!("\nParameters:\n{}", ctx.env.format_schema(schema, "  "))),
            Some(shape) => text.push_str(&format!("\nShape:\n{shape}")),
        },
        (_, Some(_)) => text.push_str("\nNo parameters required (resource tool)."),
        (None, None) => text.push_str("\nNo parameters defined."),
    }

    let mut map = details("describe");
    map.insert(
        "tool".to_string(),
        serde_json::to_value(&tool_meta).unwrap_or(Value::Null),
    );
    map.insert("server".to_string(), Value::String(server_name));
    text_result(text.trim().to_string(), map)
}

// ==================================================================================================
// 8 · `executeSearch` (MCP-158, MCP-159, MCP-160, MCP-177)
// ==================================================================================================

/// `proxy-modes.ts:492` `executeSearch(state, query, regex?, server?, includeSchemas?, limit = 12,
/// offset = 0)`.
///
/// Three **mutually exclusive** selection paths, then one rendering path.
///
/// * `regex` truthy — length cap, compile, scan. **Every match gets `score: 0` and the list is never
///   sorted**, so the output order is server-insertion order then per-server metadata order, and it
///   is observable in `details.matches`.
/// * blank query — with no `server` that is `empty_query`; with one, all of that server's metadata
///   at `score: 0`, sorted by [`rank_collate`].
/// * otherwise — [`rank_tool_matches`].
///
/// A `server` filter naming a disabled server short-circuits to [`disabled_result`] before any of
/// them.
#[must_use]
pub fn execute_search(
    ctx: &ProxyCtx,
    query: &str,
    regex: Option<bool>,
    server: Option<&str>,
    include_schemas: Option<bool>,
    limit: Option<f64>,
    offset: Option<f64>,
) -> ToolResult {
    // `includeSchemas !== false`, so `undefined` ⇒ true.
    let show_schemas = include_schemas != Some(false);
    let limit = limit.unwrap_or(12.0);
    let offset = offset.unwrap_or(0.0);

    if let Some(server) = server
        && ctx.is_disabled(server)
    {
        return disabled_result("search", server);
    }

    let global_prefix = ctx.config().tool_prefix();
    let matches: Vec<RankedToolMatch> = if regex == Some(true) {
        // (a) The regex path, in this exact order.
        if query.chars().count() > MAX_REGEX_SEARCH_QUERY_LENGTH {
            let mut map = details_err("search", McpErrorCode::QueryTooLong);
            map.insert("query".to_string(), Value::String(query.to_string()));
            map.insert("maxLength".to_string(), json!(MAX_REGEX_SEARCH_QUERY_LENGTH));
            return text_result(
                format!("Regex query is too long; maximum length is {MAX_REGEX_SEARCH_QUERY_LENGTH} characters."),
                map,
            );
        }
        // Compiled case-insensitively with EXPLICIT ceilings (MCP-159). Upstream's `recheck` ReDoS
        // gate has no port: `regex` is a finite automaton with a linear-time matching guarantee, so
        // catastrophic backtracking is structurally impossible. The named residual is that JS-only
        // syntax — backreferences, lookaround — becomes `invalid_pattern` here where upstream
        // compiled it.
        let pattern = match regex::RegexBuilder::new(query)
            .case_insensitive(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
        {
            Ok(pattern) => pattern,
            Err(_) => {
                let mut map = details_err("search", McpErrorCode::InvalidPattern);
                map.insert("query".to_string(), Value::String(query.to_string()));
                return text_result(format!("Invalid regex: {query}"), map);
            }
        };

        ctx.with_metadata(|metadata| {
            let mut matches = Vec::new();
            for (server_name, tools) in metadata {
                let definition = ctx.config().mcp_servers.get(server_name);
                if definition.is_some_and(ServerEntry::is_disabled) {
                    continue;
                }
                if let Some(filter) = server
                    && server_name != filter
                {
                    continue;
                }
                for tool in tools {
                    // MCP-177: configured keywords are searchable by regex too, resolved against
                    // the GLOBAL prefix — the per-server override is applied inside
                    // `resolveSearchKeywords` via `resolveToolPrefix`.
                    let matched = pattern.is_match(&tool.name)
                        || pattern.is_match(&tool.description)
                        || resolve_search_keywords(definition, &tool.original_name, server_name, global_prefix)
                            .iter()
                            .any(|keyword| pattern.is_match(keyword));
                    if matched {
                        matches.push(RankedToolMatch {
                            server: server_name.clone(),
                            tool: tool.clone(),
                            score: 0,
                        });
                    }
                }
            }
            matches
        })
    } else if query.trim().is_empty() {
        // (b) The blank-query path.
        let Some(server) = server else {
            let map = details_err("search", McpErrorCode::EmptyQuery);
            return text_result("Search query cannot be empty", map);
        };
        ctx.with_metadata(|metadata| {
            let mut matches: Vec<RankedToolMatch> = metadata
                .get(server)
                .map(|tools| {
                    tools
                        .iter()
                        .map(|tool| RankedToolMatch {
                            server: server.to_string(),
                            tool: tool.clone(),
                            score: 0,
                        })
                        .collect()
                })
                .unwrap_or_default();
            matches.sort_by(|a, b| rank_collate(&a.tool.name, &b.tool.name));
            matches
        })
    } else {
        // (c) The ranked path.
        ctx.with_metadata(|metadata| rank_tool_matches(ctx.config(), metadata, query, server, true))
    };

    let page = paginate(&matches, offset, limit);

    if page.total == 0 {
        // The "still connecting" hint: with a `server` filter, that server iff it is configured AND
        // connecting; otherwise every configured, non-disabled, connecting server, sorted.
        let connecting: Vec<String> = match server {
            Some(server) => {
                if ctx.config().mcp_servers.contains_key(server) && ctx.env.is_connecting(server) {
                    vec![server.to_string()]
                } else {
                    Vec::new()
                }
            }
            None => {
                let mut names: Vec<String> = ctx
                    .config()
                    .mcp_servers
                    .keys()
                    .filter(|name| !ctx.is_disabled(name) && ctx.env.is_connecting(name))
                    .cloned()
                    .collect();
                names.sort_by(|a, b| rank_collate(a, b));
                names
            }
        };
        let base = match server {
            Some(server) => format!("No tools matching \"{query}\" in \"{server}\""),
            None => format!("No tools matching \"{query}\""),
        };
        let hint = match connecting.len() {
            0 => String::new(),
            1 => format!(
                " Server \"{}\" is still connecting; retry in a moment.",
                connecting.first().map(String::as_str).unwrap_or_default()
            ),
            _ => format!(
                " Servers {} are still connecting; retry in a moment.",
                connecting.iter().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(", ")
            ),
        };
        let mut map = details("search");
        map.insert("matches".to_string(), Value::Array(Vec::new()));
        map.insert("count".to_string(), json!(0));
        map.insert("hasMore".to_string(), Value::Bool(false));
        map.insert("nextOffset".to_string(), Value::Null);
        map.insert("query".to_string(), Value::String(query.to_string()));
        if !connecting.is_empty() {
            map.insert(
                "connectingServers".to_string(),
                Value::Array(connecting.iter().map(|name| Value::String(name.clone())).collect()),
            );
        }
        return text_result(format!("{base}{hint}"), map);
    }

    let plural = if page.total == 1 { "" } else { "s" };
    let mut text = format!("Found {} tool{plural} matching \"{query}\":\n\n", page.total);
    for entry in &page.items {
        let approval_marker = if ctx.env.is_tool_call_approval_required(&entry.server, &entry.tool) {
            " (requires approval)"
        } else {
            ""
        };
        if show_schemas {
            text.push_str(&format!("{}{approval_marker}\n", entry.tool.name));
            let description = if entry.tool.description.is_empty() {
                "(no description)"
            } else {
                entry.tool.description.as_str()
            };
            text.push_str(&format!("  {description}\n"));
            match (entry.tool.input_schema.as_ref(), entry.tool.resource_uri.as_ref()) {
                (Some(schema), None) => match ctx.env.render_ts_shape(schema) {
                    None => text.push_str(&format!(
                        "\n  Parameters:\n{}\n",
                        ctx.env.format_schema(schema, "    ")
                    )),
                    Some(shape) => {
                        let indented = shape
                            .split('\n')
                            .map(|line| format!("    {line}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        text.push_str(&format!("\n  Shape:\n{indented}\n"));
                    }
                },
                (_, Some(_)) => text.push_str("  No parameters (resource tool).\n"),
                (None, None) => {}
            }
            text.push('\n');
        } else {
            text.push_str(&format!("- {}{approval_marker}", entry.tool.name));
            if !entry.tool.description.is_empty() {
                text.push_str(&format!(" - {}", truncate_at_word(&entry.tool.description, 50)));
            }
            text.push('\n');
        }
    }
    if page.has_more {
        // Em-dash, not a hyphen — this string is model-visible.
        text.push_str(&format!(
            "\n{} of {} — offset: {} for more\n",
            page.items.len(),
            page.total,
            page.next_offset.unwrap_or(0)
        ));
    }

    let rows: Vec<Value> = page
        .items
        .iter()
        .map(|entry| {
            let mut row = JsonMap::new();
            row.insert("server".to_string(), Value::String(entry.server.clone()));
            row.insert("tool".to_string(), Value::String(entry.tool.name.clone()));
            row.insert("score".to_string(), json!(entry.score));
            Value::Object(row)
        })
        .collect();
    let mut map = details("search");
    map.insert("matches".to_string(), Value::Array(rows));
    map.insert("count".to_string(), json!(page.total));
    map.insert("hasMore".to_string(), Value::Bool(page.has_more));
    map.insert(
        "nextOffset".to_string(),
        page.next_offset.map_or(Value::Null, |offset| json!(offset)),
    );
    map.insert("query".to_string(), Value::String(query.to_string()));
    text_result(text.trim().to_string(), map)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::proxy::testsupport::{FakeEnv, config_with, ctx_with, http, stdio, text_of};

    // ---- MCP-159 · the regex path's rejection codes ---------------------------------------------------

    #[test]
    fn regex_gate_rejects_over_long_queries_and_uncompilable_patterns() {
        let long_query = "a".repeat(MAX_REGEX_SEARCH_QUERY_LENGTH + 1);
        assert!(long_query.chars().count() > MAX_REGEX_SEARCH_QUERY_LENGTH);
        // A backreference is JS-legal and `regex`-illegal: the named residual of dropping `recheck`.
        // Assembled at runtime because clippy's `invalid_regex` lint rejects the literal — which is
        // precisely the property under test.
        let backreference = format!("(a){}1", '\\');
        assert!(
            regex::RegexBuilder::new(&backreference)
                .case_insensitive(true)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
                .build()
                .is_err()
        );
        // RE-SPECIFIED, not ported (MCP-159): upstream's "rejects catastrophic-backtracking regex
        // queries" asserted `unsafe_pattern`. Rust's `regex` is a finite automaton with a
        // linear-time matching guarantee, so the pattern compiles, runs, and finishes.
        let nested = regex::RegexBuilder::new("(a+)+$")
            .case_insensitive(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
            .expect("a nested quantifier compiles under a linear-time engine");
        let start = std::time::Instant::now();
        assert!(!nested.is_match(&format!("{}b", "a".repeat(64))));
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }

    // ---- MCP-154 · `executeStatus` ------------------------------------------------------------------

    #[test]
    fn status_renders_all_six_rungs_with_their_glyphs() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[
            ("live", stdio("a")),
            ("waiting", http("https://a.example")),
            ("broken", stdio("b")),
            ("warm", stdio("c")),
            ("cold", stdio("d")),
            ("off", disabled),
        ]);
        let env = FakeEnv::default()
            .with_connection("live", ConnectionStatus::Connected)
            .with_connection("waiting", ConnectionStatus::NeedsAuth)
            .with_failure("broken", 12);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("live", vec![ToolMetadata::new("live_a", "a", ""), ToolMetadata::new("live_b", "b", "")]),
                ("warm", vec![ToolMetadata::new("warm_a", "a", "")]),
            ],
            &[],
            env,
        );

        let result = execute_status(&ctx);
        let text = text_of(&result);
        // The header counts ENABLED servers only, and totals only their tools.
        assert!(text.starts_with("MCP: 1/5 servers, 3 tools (1 disabled)\n\n"), "{text}");
        assert!(text.contains("✓ live (2 tools)\n"), "{text}");
        assert!(text.contains("⚠ waiting (needs auth)\n"), "{text}");
        assert!(text.contains("✗ broken (failed 12s ago)\n"), "{text}");
        assert!(text.contains("○ warm (1 tools, cached)\n"), "{text}");
        assert!(text.contains("○ cold (not connected)\n"), "{text}");
        assert!(text.contains("⊘ off (disabled)\n"), "{text}");
        assert!(text.ends_with(
            "mcp({ server: \"name\" }) to list tools, mcp({ search: \"...\" }) to search"
        ));

        let details = result.details.expect("details");
        assert_eq!(details["mode"], json!("status"));
        assert_eq!(details["totalTools"], json!(3));
        assert_eq!(details["connectedCount"], json!(1));
        assert_eq!(details["disabledCount"], json!(1));
        let rows = details["servers"].as_array().expect("servers");
        // Insertion order, not alphabetical.
        let names: Vec<&str> = rows.iter().map(|row| row["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["live", "waiting", "broken", "warm", "cold", "off"]);
        // `disabled` is present ONLY when true.
        assert!(rows[0].get("disabled").is_none());
        assert_eq!(rows[5]["disabled"], json!(true));
        assert_eq!(rows[2]["failedAgo"], json!(12));
        assert_eq!(rows[0]["failedAgo"], Value::Null);
    }

    // ---- MCP-155 · `executeList` ---------------------------------------------------------------------

    #[test]
    fn list_covers_its_five_outcomes() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[
            ("empty_live", stdio("a")),
            ("empty_cached", stdio("b")),
            ("never", stdio("c")),
            ("full", stdio("d")),
            ("off", disabled),
        ]);
        let long = "word ".repeat(120);
        let env = FakeEnv::default()
            .with_connection("empty_live", ConnectionStatus::Connected)
            .with_connection("full", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("empty_cached", Vec::new()),
                (
                    "full",
                    vec![
                        ToolMetadata::new("full_a", "a", "Does a thing"),
                        ToolMetadata::new("full_b", "b", ""),
                    ],
                ),
            ],
            &[("full", long.as_str()), ("empty_live", "Short note.")],
            env,
        );

        // 1 · unknown server.
        let unknown = execute_list(&ctx, "nope");
        assert_eq!(unknown.details.clone().unwrap()["error"], json!("not_found"));
        // 2 · disabled.
        assert_eq!(execute_list(&ctx, "off").details.clone().unwrap()["error"], json!("server_disabled"));
        // 4a · connected with zero tools, plus a short instructions preview and NO pointer.
        let live = execute_list(&ctx, "empty_live");
        assert_eq!(text_of(&live), "Server \"empty_live\" has no tools.\n\nServer instructions:\nShort note.");
        let details = live.details.clone().unwrap();
        assert_eq!(details["count"], json!(0));
        assert_eq!(details["hasInstructions"], json!(true));
        assert!(details.get("error").is_none());
        // 4b · metadata present but not connected.
        let cached = execute_list(&ctx, "empty_cached");
        assert_eq!(text_of(&cached), "Server \"empty_cached\" has no cached tools (not connected).");
        assert_eq!(cached.details.clone().unwrap()["cached"], json!(true));
        // 4c · no metadata at all.
        let never = execute_list(&ctx, "never");
        assert_eq!(never.details.clone().unwrap()["error"], json!("not_connected"));
        // 5 · the listing, with the pointer BECAUSE the 300-char preview truncated.
        let full = execute_list(&ctx, "full");
        let text = text_of(&full);
        assert!(text.starts_with("full (2 tools):\n\n- full_a - Does a thing\n- full_b\n"), "{text}");
        assert!(text.contains("\nUse mcp({ instructions: \"full\" }) for the full text."), "{text}");
        let details = full.details.clone().unwrap();
        assert_eq!(details["tools"], json!(["full_a", "full_b"]));
        assert_eq!(details["count"], json!(2));
    }

    #[test]
    fn list_marks_a_cached_listing_when_not_connected() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(
            config,
            &[("srv", vec![ToolMetadata::new("srv_a", "a", "")])],
            &[],
            FakeEnv::default(),
        );
        assert!(text_of(&execute_list(&ctx, "srv")).starts_with("srv (1 tools (not connected, cached)):"));
    }

    // ---- MCP-156 · `executeInstructions` --------------------------------------------------------------

    #[test]
    fn cached_instructions_win_even_for_a_disconnected_server() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[
            ("cached", stdio("a")),
            ("live", stdio("b")),
            ("cold", stdio("c")),
            ("off", disabled),
        ]);
        let env = FakeEnv::default().with_connection("live", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(config, &[], &[("cached", "Use the API key.")], env);

        assert_eq!(
            execute_instructions(&ctx, "missing").details.clone().unwrap()["error"],
            json!("not_found")
        );
        assert_eq!(
            execute_instructions(&ctx, "off").details.clone().unwrap()["error"],
            json!("server_disabled")
        );
        // Cached, and NOT connected — the cache is consulted before the connection.
        let cached = execute_instructions(&ctx, "cached");
        assert_eq!(text_of(&cached), "cached instructions:\n\nUse the API key.");
        assert_eq!(cached.details.clone().unwrap()["length"], json!(16));
        assert_eq!(
            execute_instructions(&ctx, "live").details.clone().unwrap()["error"],
            json!("no_instructions")
        );
        assert_eq!(
            execute_instructions(&ctx, "cold").details.clone().unwrap()["error"],
            json!("not_connected")
        );
    }

    // ---- MCP-157 · `executeDescribe` -------------------------------------------------------------------

    #[test]
    fn describe_renders_a_resource_tool_and_the_approval_marker() {
        let config = config_with(&[("files", stdio("a"))]);
        let mut resource = ToolMetadata::new("files_read_notes", "read_notes", "Read the notes");
        resource.resource_uri = Some("file:///notes.md".to_string());
        let env = FakeEnv::default().with_approval_required("files_read_notes");
        let (ctx, _) = ctx_with(config, &[("files", vec![resource])], &[], env);

        let result = execute_describe(&ctx, "files_read_notes");
        assert_eq!(
            text_of(&result),
            "files_read_notes (requires approval)\nServer: files\nType: Resource (reads from file:///notes.md)\n\nRead the notes\n\nNo parameters required (resource tool)."
        );
        let details = result.details.clone().unwrap();
        assert_eq!(details["mode"], json!("describe"));
        assert_eq!(details["server"], json!("files"));
        assert_eq!(details["tool"]["originalName"], json!("read_notes"));
    }

    #[test]
    fn describe_reports_a_disabled_only_match_as_server_disabled() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("off", disabled)]);
        let (ctx, _) = ctx_with(
            config,
            &[("off", vec![ToolMetadata::new("off_thing", "thing", "")])],
            &[],
            FakeEnv::default(),
        );
        let result = execute_describe(&ctx, "off_thing");
        assert_eq!(result.details.clone().unwrap()["error"], json!("server_disabled"));
    }

    #[test]
    fn describe_fails_closed_and_suggests_on_a_miss() {
        let config = config_with(&[("a", stdio("a")), ("b", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("a", vec![ToolMetadata::new("shared", "shared", "")]),
                ("b", vec![ToolMetadata::new("shared", "shared", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        assert_eq!(
            execute_describe(&ctx, "shared").details.clone().unwrap()["error"],
            json!("ambiguous_tool")
        );
        let miss = execute_describe(&ctx, "totally_absent");
        let details = miss.details.clone().unwrap();
        assert_eq!(details["error"], json!("tool_not_found"));
        assert_eq!(details["requestedTool"], json!("totally_absent"));
        assert!(text_of(&miss).starts_with("Tool \"totally_absent\" not found. Use mcp({ search: \"...\" }) to search."));
    }

    #[test]
    fn describe_forks_between_shape_and_parameters() {
        let config = config_with(&[("srv", stdio("a"))]);
        let mut with_schema = ToolMetadata::new("srv_run", "run", "Run it");
        with_schema.input_schema = Some(json!({"type": "object"}));
        let plain = ToolMetadata::new("srv_ping", "ping", "");
        let (ctx, _) = ctx_with(config, &[("srv", vec![with_schema, plain])], &[], FakeEnv::default());

        // `renderTsShape` returned a shape, so the `Shape:` arm is taken.
        assert!(text_of(&execute_describe(&ctx, "srv_run")).ends_with("\nShape:\n{ a: string }"));
        // No schema, no resource ⇒ the third arm, and an empty description renders the placeholder.
        assert_eq!(
            text_of(&execute_describe(&ctx, "srv_ping")),
            "srv_ping\nServer: srv\n\n(no description)\n\nNo parameters defined."
        );
    }

    // ---- MCP-158 / MCP-160 · `executeSearch` -----------------------------------------------------------

    #[test]
    fn regex_search_preserves_insertion_order_and_never_sorts() {
        // `zeta` is configured first, so its tools come first even though `alpha` sorts before it.
        let config = config_with(&[("zeta", stdio("a")), ("alpha", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("zeta", vec![ToolMetadata::new("z_two", "two", ""), ToolMetadata::new("z_one", "one", "")]),
                ("alpha", vec![ToolMetadata::new("a_one", "one", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        let result = execute_search(&ctx, "_", Some(true), None, Some(false), None, None);
        let details = result.details.clone().unwrap();
        let names: Vec<&str> =
            details["matches"].as_array().unwrap().iter().map(|m| m["tool"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["z_two", "z_one", "a_one"]);
        // Every regex match scores 0.
        assert!(details["matches"].as_array().unwrap().iter().all(|m| m["score"] == json!(0)));
    }

    #[test]
    fn regex_search_rejects_over_long_and_malformed_queries() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(config, &[("srv", Vec::new())], &[], FakeEnv::default());

        let long = "a".repeat(MAX_REGEX_SEARCH_QUERY_LENGTH + 1);
        let rejected = execute_search(&ctx, &long, Some(true), None, None, None, None);
        let details = rejected.details.clone().unwrap();
        assert_eq!(details["error"], json!("query_too_long"));
        assert_eq!(details["maxLength"], json!(256));
        assert_eq!(
            text_of(&rejected),
            "Regex query is too long; maximum length is 256 characters."
        );

        let malformed = execute_search(&ctx, "(a", Some(true), None, None, None, None);
        assert_eq!(malformed.details.clone().unwrap()["error"], json!("invalid_pattern"));
        // A non-regex search is unaffected by the cap.
        let plain = execute_search(&ctx, &long, None, None, None, None, None);
        assert_ne!(plain.details.clone().unwrap()["error"], json!("query_too_long"));
    }

    #[test]
    fn blank_search_needs_a_server_and_then_sorts_by_collation() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(
            config,
            &[(
                "srv",
                vec![ToolMetadata::new("Zeta", "Zeta", ""), ToolMetadata::new("alpha", "alpha", "")],
            )],
            &[],
            FakeEnv::default(),
        );
        // `search: ""` REACHES the mode (dispatch tests `!== undefined`).
        let empty = execute_search(&ctx, "", None, None, None, None, None);
        assert_eq!(empty.details.clone().unwrap()["error"], json!("empty_query"));
        assert_eq!(text_of(&empty), "Search query cannot be empty");

        let scoped = execute_search(&ctx, "  ", None, Some("srv"), Some(false), None, None);
        let details = scoped.details.clone().unwrap();
        let names: Vec<&str> =
            details["matches"].as_array().unwrap().iter().map(|m| m["tool"].as_str().unwrap()).collect();
        // ICU root collation, not byte order: `alpha` before `Zeta`.
        assert_eq!(names, vec!["alpha", "Zeta"]);
    }

    #[test]
    fn zero_results_report_connecting_servers_singular_and_plural() {
        let config = config_with(&[("one", stdio("a")), ("two", stdio("b"))]);
        let (ctx, env) =
            ctx_with(config.clone(), &[], &[], FakeEnv::default().with_connecting("one"));
        let single = execute_search(&ctx, "nothing", None, None, None, None, None);
        assert_eq!(
            text_of(&single),
            "No tools matching \"nothing\" Server \"one\" is still connecting; retry in a moment."
        );
        assert_eq!(single.details.clone().unwrap()["connectingServers"], json!(["one"]));
        drop(env);

        let (ctx, _) = ctx_with(
            config,
            &[],
            &[],
            FakeEnv::default().with_connecting("one").with_connecting("two"),
        );
        let many = execute_search(&ctx, "nothing", None, None, None, None, None);
        assert_eq!(
            text_of(&many),
            "No tools matching \"nothing\" Servers \"one\", \"two\" are still connecting; retry in a moment."
        );
        // A filtered search names only the filtered server, and the key is absent when empty.
        let filtered = execute_search(&ctx, "nothing", None, Some("one"), None, None, None);
        assert!(text_of(&filtered).starts_with("No tools matching \"nothing\" in \"one\""));
        let (ctx, _) = ctx_with(config_with(&[("one", stdio("a"))]), &[], &[], FakeEnv::default());
        let quiet = execute_search(&ctx, "nothing", None, None, None, None, None);
        assert!(quiet.details.clone().unwrap().get("connectingServers").is_none());
    }

    #[test]
    fn search_paginates_with_an_em_dash_footer() {
        let config = config_with(&[("srv", stdio("a"))]);
        let tools: Vec<ToolMetadata> = (0..5)
            .map(|index| ToolMetadata::new(format!("srv_report_{index}"), format!("report_{index}"), "Reporting"))
            .collect();
        let (ctx, _) = ctx_with(config, &[("srv", tools)], &[], FakeEnv::default());
        let page = execute_search(&ctx, "report", None, None, Some(false), Some(2.0), Some(0.0));
        let text = text_of(&page);
        assert!(text.starts_with("Found 5 tools matching \"report\":\n\n"), "{text}");
        assert!(text.ends_with("2 of 5 — offset: 2 for more"), "{text}");
        let details = page.details.clone().unwrap();
        assert_eq!(details["hasMore"], json!(true));
        assert_eq!(details["nextOffset"], json!(2));
        assert_eq!(details["count"], json!(5));

        // Singular header, and no footer, on the last page.
        let last = execute_search(&ctx, "report_4", None, None, Some(false), Some(12.0), Some(0.0));
        assert!(text_of(&last).starts_with("Found 1 tool matching"), "{}", text_of(&last));
        assert_eq!(last.details.clone().unwrap()["nextOffset"], Value::Null);
    }

    #[test]
    fn search_with_schemas_indents_the_shape_block_by_four() {
        let config = config_with(&[("srv", stdio("a"))]);
        let mut tool = ToolMetadata::new("srv_run", "run", "Run it");
        tool.input_schema = Some(json!({"type": "object"}));
        let (ctx, _) = ctx_with(config, &[("srv", vec![tool])], &[], FakeEnv::default());
        let text = text_of(&execute_search(&ctx, "run", None, None, None, None, None));
        assert!(text.contains("srv_run\n  Run it\n\n  Shape:\n    { a: string }"), "{text}");
    }

    #[test]
    fn a_disabled_server_filter_short_circuits_search() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("off", disabled)]);
        let (ctx, _) = ctx_with(config, &[], &[], FakeEnv::default());
        assert_eq!(
            execute_search(&ctx, "anything", None, Some("off"), None, None, None)
                .details
                .unwrap()["error"],
            json!("server_disabled")
        );
    }

}
