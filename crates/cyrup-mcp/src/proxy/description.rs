//! `buildProxyDescription` — the regenerated description (MCP-152, MCP-198).
//!
//! See [`crate::proxy`] for the module overview.


use indexmap::{IndexMap, IndexSet};


use crate::config::{
    McpConfig, ServerEntry,
    ToolPrefix,
};
use crate::proxy::constants::INSTRUCTIONS_SNIPPET_LENGTH;
use crate::proxy::tool_metadata::{CandidateIndex, ToolMetadata, is_tool_allowed, is_ui_tool_visible_to_model, resolve_tool_prefix, resource_name_to_tool_name, tool_name_candidates, truncate_at_word};

// ==================================================================================================
// 13 · `buildProxyDescription` — the regenerated description (MCP-152, MCP-198)
// ==================================================================================================

/// One `directSpecs` entry, reduced to the two fields the description reads.
///
/// The full `DirectToolSpec` is 13e's; this is the projection `buildProxyDescription` actually
/// consumes, so the two never have to agree on more than a name and a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectToolSummary {
    /// The `mcpServers` key that contributed the tool.
    pub server_name: String,
    /// The registered, model-visible name.
    pub prefixed_name: String,
}

/// The subset of `mcp-cache.json` [`build_proxy_description`] reads for one server.
///
/// **Only cache-valid entries reach here** — the caller applies `isServerCacheValid(entry,
/// definition)` and passes `None` for a stale one. A stale entry is *not* skipped by the caller's
/// loop, it just yields zero counts, and a zero total is what drops the server out of the summary.
#[derive(Debug, Clone, Default)]
pub struct CachedServerEntry {
    /// The cached tools, with their `uiVisibility` intact.
    pub tools: Vec<ToolMetadata>,
    /// `(name, uri)` per cached resource.
    pub resources: Vec<(String, String)>,
    /// The server's own `instructions` from the initialize handshake.
    pub instructions: Option<String>,
}

/// `direct-tools.ts:259` `hasToolFilters` (upstream `faf55f7`) — does this server declare a tool
/// selector at all?
///
/// Upstream tests `Array.isArray(x) && x.length > 0` on both fields, so a JSON `null`, a non-array
/// and `[]` all read as "no filter" — which is exactly `Option<Vec<String>>` plus the emptiness
/// check. The predicate has to be *cheap* and *total*: it is the guard that decides whether the
/// collision scan runs at all.
///
/// [`crate::registration`] carries a private twin for the cache-side copy of `buildProxyDescription`;
/// the two collapse into one when MCP-207 merges this file's simple candidate-set form into 13e's
/// memoised [`crate::registration::CandidateIndex`].
fn server_has_tool_filters(definition: &ServerEntry) -> bool {
    definition.include_tools.as_ref().is_some_and(|list| !list.is_empty())
        || definition.exclude_tools.as_ref().is_some_and(|list| !list.is_empty())
}

/// The MCP-198 cross-server collision set, as the memoising index [`is_tool_allowed`] consumes:
/// every *current-form* name candidate of every enabled server that has a cache entry — including
/// the server being filtered, whose own candidates are subtracted by match *count* inside
/// [`CandidateIndex`] rather than pre-deleted.
///
/// **`None`, never an empty index, unless some server declares a selector.**
/// `direct-tools.ts:257-262`, upstream `faf55f7` ("avoid O(tools²) startup collision scan when no
/// tool filters are configured"): [`is_tool_allowed`] short-circuits on absent/empty `includeTools`
/// *and* `excludeTools` before it ever reads the set, so building one nothing consults is pure
/// startup cost — the report behind that commit had 14 servers / ~800 tools, where the equivalent
/// scan cost ~2.6s of synchronous startup and dominated `pi`'s 3.66s launch. This description is
/// regenerated on every metadata update, so the waste was per-reconnect, not once.
///
/// `None` rather than an empty index is safe *because* this gate and the per-server gate in
/// [`build_proxy_description`] test the identical predicate: no server can consult an index that
/// was never built. One build serves the whole call, where upstream rebuilds an identical index per
/// filtered server (it is not parameterised by the server being filtered).
fn collision_index(
    config: &McpConfig,
    cache: &IndexMap<String, CachedServerEntry>,
    prefix: ToolPrefix,
) -> Option<CandidateIndex> {
    if !config.mcp_servers.values().any(server_has_tool_filters) {
        return None;
    }
    let mut all_candidates: IndexSet<String> = IndexSet::new();
    for (other_server, other_definition) in &config.mcp_servers {
        let Some(other_entry) = cache.get(other_server) else { continue };
        if other_definition.is_disabled() {
            continue;
        }
        let other_prefix = resolve_tool_prefix(Some(other_definition), prefix);
        for tool in &other_entry.tools {
            // `isUiToolVisibleToModel` **survives the MCP Apps cut**: dropping it would expose to
            // the model tools the server explicitly marked app-only.
            if !is_ui_tool_visible_to_model(tool.ui_visibility.as_deref()) {
                continue;
            }
            all_candidates
                .extend(tool_name_candidates(&tool.name, other_server, other_prefix, false));
        }
        if other_definition.expose_resources() {
            for (name, _) in &other_entry.resources {
                let base = format!("read_{}", resource_name_to_tool_name(name));
                all_candidates.extend(tool_name_candidates(&base, other_server, other_prefix, false));
            }
        }
    }
    Some(CandidateIndex::new(all_candidates))
}

/// `direct-tools.ts:234` `buildProxyDescription(config, cache, directSpecs)`.
///
/// Six blocks in this exact order, each appended only when non-empty:
/// 1. the header, always, ending in a newline;
/// 2. direct-tool counts per server, in `directSpecs` iteration order;
/// 3. per-server proxy counts (`totalItems − directCount`, emitted only when `> 0`);
/// 4. disabled servers;
/// 5. 150-character instruction snippets;
/// 6. the usage block, always, byte-exact including the two-space indent, the arrow glyph `→` and
///    the **absence** of a trailing newline on the final `Mode:` line.
///
/// **MCP-198 — the counts are an O(servers × tools) cross-server computation, not a per-server
/// filter.** [`collision_index`] builds the set of name candidates produced by *every other*
/// cache-valid, enabled server (including `read_<resource>` names when `exposeResources !== false`)
/// and hands it to `isToolAllowed` as its collision set, so adding an unrelated server can change a
/// third server's advertised count. Built **once per call**, and **not at all** unless some server
/// declares a selector (`direct-tools.ts:257`, upstream `faf55f7`; upstream builds one index per
/// *filtered* server and, before that commit, rebuilt it per *tool* — the O(tools²) scan). Simplifying
/// it to a per-server `includeTools`/`excludeTools` filter would silently differ from pi's for any
/// workspace with overlapping tool names.
///
/// **Post-cut edits, both deliberate:** the header's `use mcpScript.` sentence is removed (Cut 4)
/// and `Pi` becomes `cyrup` (MCP-163's naming decision); the
/// `mcp({ action: "ui-messages" })` usage line is removed (Cut 2). Every other line, including the
/// `Mode:` precedence line, is unchanged.
#[must_use]
pub fn build_proxy_description(
    config: &McpConfig,
    cache: &IndexMap<String, CachedServerEntry>,
    direct_specs: &[DirectToolSummary],
) -> String {
    let prefix = config.tool_prefix();
    let mut desc = String::from(
        "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n",
    );

    // 2 · Direct tools, counted in `directSpecs` iteration order.
    let mut direct_by_server: IndexMap<String, usize> = IndexMap::new();
    for spec in direct_specs {
        *direct_by_server.entry(spec.server_name.clone()).or_insert(0) += 1;
    }
    if !direct_by_server.is_empty() {
        let parts: Vec<String> =
            direct_by_server.iter().map(|(server, count)| format!("{server} ({count})")).collect();
        desc.push_str(&format!(
            "\nDirect tools available (call as normal tools): {}\n",
            parts.join(", ")
        ));
    }

    // MCP-198 · the cross-server candidate-collision index — built once, and only when a selector
    // exists to read it. See [`collision_index`].
    let mut collision = collision_index(config, cache, prefix);

    // 3 · Per-server proxy counts.
    let mut server_summaries: Vec<String> = Vec::new();
    for (server_name, definition) in &config.mcp_servers {
        if definition.is_disabled() {
            continue;
        }
        let entry = cache.get(server_name);
        let effective_prefix = resolve_tool_prefix(Some(definition), prefix);
        // `direct-tools.ts:284` — the index is consulted only when *this* server declares a
        // selector. [`collision_index`] tests the same predicate across every server before it
        // builds anything, so this can never name an index that was skipped.
        //
        // Explicit loops, not `.filter(…).count()`: [`CandidateIndex`] memoises as it answers, so
        // the borrow is `&mut` and cannot cross a closure that the surrounding iterator also holds.
        let mut index =
            if server_has_tool_filters(definition) { collision.as_mut() } else { None };

        let mut tool_count = 0_usize;
        if let Some(entry) = entry {
            for tool in &entry.tools {
                if !is_ui_tool_visible_to_model(tool.ui_visibility.as_deref()) {
                    continue;
                }
                if is_tool_allowed(
                    &tool.name,
                    server_name,
                    effective_prefix,
                    definition.include_tools.as_deref(),
                    definition.exclude_tools.as_deref(),
                    index.as_deref_mut(),
                ) {
                    tool_count += 1;
                }
            }
        }

        let mut resource_count = 0_usize;
        if definition.expose_resources()
            && let Some(entry) = entry
        {
            for (name, _) in &entry.resources {
                let base = format!("read_{}", resource_name_to_tool_name(name));
                if is_tool_allowed(
                    &base,
                    server_name,
                    effective_prefix,
                    definition.include_tools.as_deref(),
                    definition.exclude_tools.as_deref(),
                    index.as_deref_mut(),
                ) {
                    resource_count += 1;
                }
            }
        }

        let total_items = tool_count + resource_count;
        if total_items == 0 {
            // This is how a stale or missing cache entry drops out of the summary.
            continue;
        }
        let direct_count = direct_by_server.get(server_name).copied().unwrap_or(0);
        let proxy_count = total_items.saturating_sub(direct_count);
        if proxy_count > 0 {
            server_summaries.push(format!("{server_name} ({proxy_count} tools)"));
        }
    }
    if !server_summaries.is_empty() {
        desc.push_str(&format!("\nServers: {}\n", server_summaries.join(", ")));
    }

    // 4 · Disabled servers.
    let disabled: Vec<&String> = config
        .mcp_servers
        .iter()
        .filter(|(_, definition)| definition.is_disabled())
        .map(|(name, _)| name)
        .collect();
    if !disabled.is_empty() {
        let names: Vec<&str> = disabled.iter().map(|name| name.as_str()).collect();
        desc.push_str(&format!(
            "\nDisabled servers (enable with /mcp enable <server> and /reload): {}\n",
            names.join(", ")
        ));
    }

    // 5 · Instruction snippets.
    let mut instruction_summaries: Vec<String> = Vec::new();
    for (server_name, definition) in &config.mcp_servers {
        if definition.is_disabled() {
            continue;
        }
        let Some(instructions) =
            cache.get(server_name).and_then(|entry| entry.instructions.as_ref()).filter(|text| !text.is_empty())
        else {
            continue;
        };
        // `instructions.replace(/\s+/g, " ").trim()` before truncating.
        let flattened = instructions.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet = truncate_at_word(&flattened, INSTRUCTIONS_SNIPPET_LENGTH);
        // The two-space indent is part of each summary line, not of the joiner.
        instruction_summaries.push(format!("  {server_name}: {snippet}"));
    }
    if !instruction_summaries.is_empty() {
        desc.push_str(&format!(
            "\nServer instructions (truncated - full text via mcp({{ instructions: \"name\" }})):\n{}\n",
            instruction_summaries.join("\n")
        ));
    }

    // 6 · The usage block. Byte-exact; the final `Mode:` line carries NO trailing newline.
    desc.push_str("\nUsage:\n");
    desc.push_str("  mcp({ })                              → Show server status\n");
    desc.push_str("  mcp({ server: \"name\" })               → List tools from server\n");
    desc.push_str("  mcp({ search: \"query\" })              → Search MCP tools by name/description\n");
    desc.push_str("  mcp({ describe: \"tool_name\" })        → Show tool details and parameters\n");
    desc.push_str("  mcp({ instructions: \"name\" })         → Show full server usage instructions\n");
    desc.push_str("  mcp({ connect: \"server-name\" })       → Connect to a server and refresh metadata\n");
    desc.push_str("  mcp({ tool: \"name\", args: { key: \"value\" } })         → Call a tool (object args; JSON string also accepted)\n");
    desc.push_str("  mcp({ action: \"auth-start\", server: \"name\" })      → Start manual OAuth and get a browser URL\n");
    desc.push_str("  mcp({ action: \"auth-complete\", server: \"name\", args: { redirectUrl: \"...\" } }) → Complete manual OAuth\n");
    desc.push_str("\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)");
    desc
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::proxy::testsupport::{config_with, stdio};

    // ---- `truncateAtWord` ----------------------------------------------------------------------------

    #[test]
    fn truncate_at_word_cuts_at_the_last_space_past_sixty_percent() {
        assert_eq!(truncate_at_word("short", 50), "short");
        assert_eq!(truncate_at_word("", 50), "");
        // Last space at index 8 of a 10-char budget: 8 > 6, so cut there.
        assert_eq!(truncate_at_word("abcdefgh ijklmnop", 10), "abcdefgh...");
        // Last space at index 2 of a 10-char budget: 2 <= 6, so cut at the budget.
        assert_eq!(truncate_at_word("ab cdefghijklmnop", 10), "ab cdefghi...");
        // No space at all: cut at the budget.
        assert_eq!(truncate_at_word("abcdefghijklmnop", 10), "abcdefghij...");
    }

    // ---- MCP-152 / MCP-198 · the regenerated description ----------------------------------------------

    #[test]
    fn proxy_description_renders_every_block_in_order() {
        let github = ServerEntry { command: Some("npx".to_string()), ..ServerEntry::default() };
        let docs = ServerEntry { command: Some("npx".to_string()), ..ServerEntry::default() };
        let off = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("github", github), ("docs", docs), ("legacy", off)]);

        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        cache.insert(
            "github".to_string(),
            CachedServerEntry {
                tools: vec![
                    ToolMetadata::new("github_create_issue", "create_issue", "Open an issue"),
                    ToolMetadata::new("github_list_prs", "list_prs", "List PRs"),
                ],
                resources: Vec::new(),
                instructions: None,
            },
        );
        cache.insert(
            "docs".to_string(),
            CachedServerEntry {
                tools: vec![ToolMetadata::new("docs_search", "search", "Search docs")],
                resources: Vec::new(),
                instructions: Some("  Always   cite the   page number.  ".to_string()),
            },
        );

        let direct = [DirectToolSummary {
            server_name: "github".to_string(),
            prefixed_name: "github_create_issue".to_string(),
        }];
        let description = build_proxy_description(&config, &cache, &direct);

        // 1 · the post-cut header, with `mcpScript` removed and the host renamed.
        assert!(description.starts_with(
            "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n"
        ));
        assert!(!description.contains("mcpScript"));
        assert!(!description.contains("Pi tools"));
        // 2 · direct-tool counts.
        assert!(description.contains("\nDirect tools available (call as normal tools): github (1)\n"));
        // 3 · proxy counts: github has 2 cached tools minus 1 direct = 1.
        assert!(description.contains("\nServers: github (1 tools), docs (1 tools)\n"));
        // 4 · disabled servers.
        assert!(description.contains(
            "\nDisabled servers (enable with /mcp enable <server> and /reload): legacy\n"
        ));
        // 5 · instruction snippets — whitespace collapsed, two-space indent part of the line.
        assert!(description.contains(
            "\nServer instructions (truncated - full text via mcp({ instructions: \"name\" })):\n  docs: Always cite the page number.\n"
        ));
        // 6 · the usage block, with the ui-messages line gone and no trailing newline.
        assert!(!description.contains("ui-messages"));
        assert!(description.ends_with(
            "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)"
        ));
        assert_eq!(description.matches('→').count(), 9, "nine usage arrows survive the cut");
    }

    /// MCP-198 · a tool hidden by `uiVisibility` is not counted, and does not reserve its name in
    /// the cross-server collision set.
    #[test]
    fn hidden_tools_are_excluded_from_the_advertised_counts() {
        let server = ServerEntry { command: Some("npx".to_string()), ..ServerEntry::default() };
        let config = config_with(&[("app", server)]);
        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        let mut hidden = ToolMetadata::new("app_widget", "widget", "App-only");
        hidden.ui_visibility = Some(vec!["app".to_string()]);
        cache.insert(
            "app".to_string(),
            CachedServerEntry {
                tools: vec![hidden, ToolMetadata::new("app_open", "open", "Open")],
                resources: Vec::new(),
                instructions: None,
            },
        );
        let description = build_proxy_description(&config, &cache, &[]);
        assert!(description.contains("\nServers: app (1 tools)\n"), "{description}");
        assert!(is_ui_tool_visible_to_model(None));
        assert!(is_ui_tool_visible_to_model(Some(&["model".to_string()])));
        assert!(!is_ui_tool_visible_to_model(Some(&[])));
        assert!(!is_ui_tool_visible_to_model(Some(&["app".to_string()])));
    }

    /// MCP-198 · the two-tier selector and its collision guard.
    ///
    /// A pattern that only reaches a tool's **legacy** spelling is disarmed when that same spelling
    /// is some other configured tool's *current* name — which is the whole reason
    /// `buildProxyDescription` computes a cross-server candidate set at all.
    #[test]
    fn tool_selectors_are_two_tier_and_collision_guarded() {
        let none: Option<&[String]> = None;
        // No filters at all ⇒ allowed.
        assert!(is_tool_allowed("do-it", "srv", ToolPrefix::Server, none, none, None));

        // A current-candidate include selects; a miss does not.
        let include_current = ["srv_do-it".to_string()];
        assert!(is_tool_allowed("do-it", "srv", ToolPrefix::Server, Some(&include_current), none, None));
        let include_other = ["something_else".to_string()];
        assert!(!is_tool_allowed("do-it", "srv", ToolPrefix::Server, Some(&include_other), none, None));

        // A current-candidate exclude excludes.
        let exclude_current = ["srv_do-it".to_string()];
        assert!(!is_tool_allowed("do-it", "srv", ToolPrefix::Server, none, Some(&exclude_current), None));

        // `do_it` is a LEGACY-only candidate of `do-it`.
        let current = tool_name_candidates("do-it", "srv", ToolPrefix::Server, false);
        let legacy = tool_name_candidates("do-it", "srv", ToolPrefix::Server, true);
        assert!(!current.contains("do_it"));
        assert!(legacy.contains("do_it"));

        let exclude_legacy = ["do_it".to_string()];
        // …with no collision context it still excludes…
        assert!(!is_tool_allowed("do-it", "srv", ToolPrefix::Server, none, Some(&exclude_legacy), None));
        // …and with a collision index that does not contain it, likewise.
        let mut quiet = CandidateIndex::new(current.clone());
        assert!(!is_tool_allowed(
            "do-it",
            "srv",
            ToolPrefix::Server,
            none,
            Some(&exclude_legacy),
            Some(&mut quiet)
        ));
        // But when `do_it` is another server's CURRENT name, the selector is disarmed.
        let mut collides_set: IndexSet<String> = current.clone();
        collides_set.insert("do_it".to_string());
        let mut collides = CandidateIndex::new(collides_set);
        assert!(is_tool_allowed(
            "do-it",
            "srv",
            ToolPrefix::Server,
            none,
            Some(&exclude_legacy),
            Some(&mut collides)
        ));
    }

    /// A configured `excludeTools` really does lower the count the model reads.
    #[test]
    fn excluded_tools_drop_out_of_the_advertised_count() {
        let filtered = ServerEntry {
            command: Some("npx".to_string()),
            exclude_tools: Some(vec!["srv_secret".to_string()]),
            ..ServerEntry::default()
        };
        let config = config_with(&[("srv", filtered)]);
        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        cache.insert(
            "srv".to_string(),
            CachedServerEntry {
                tools: vec![
                    ToolMetadata::new("srv_secret", "secret", ""),
                    ToolMetadata::new("srv_public", "public", ""),
                ],
                resources: vec![("notes.md".to_string(), "file:///notes.md".to_string())],
                instructions: None,
            },
        );
        let description = build_proxy_description(&config, &cache, &[]);
        // 2 tools − 1 excluded + 1 resource (`read_notes_md`) = 2.
        assert!(description.contains("\nServers: srv (2 tools)\n"), "{description}");
        assert_eq!(resource_name_to_tool_name("notes.md"), "notes_md");
        assert_eq!(resource_name_to_tool_name("9lives"), "resource_9lives");
        assert_eq!(resource_name_to_tool_name("__A B__"), "a_b");
    }

    /// `faf55f7` — the cross-server collision scan does not run *at all* when no server declares a
    /// selector.
    ///
    /// Upstream proves this by mocking `getToolNameCandidates` and asserting zero calls
    /// (`__tests__/collision-scan-lazy.test.ts`). The Rust equivalent is to assert the scan's
    /// product: [`collision_index`] is the only thing on this path that expands candidates, so
    /// an empty set is proof the scan was *skipped*, not merely fast — two servers whose tool names
    /// collide would otherwise both be indexed.
    #[test]
    fn collision_scan_is_skipped_when_no_server_declares_a_selector() {
        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        for server in ["a", "b"] {
            cache.insert(
                server.to_string(),
                CachedServerEntry {
                    tools: vec![ToolMetadata::new(format!("{server}_search"), "search", "Search")],
                    resources: Vec::new(),
                    instructions: None,
                },
            );
        }

        let unfiltered = config_with(&[("a", stdio("npx")), ("b", stdio("npx"))]);
        assert!(
            collision_index(&unfiltered, &cache, unfiltered.tool_prefix()).is_none(),
            "no includeTools/excludeTools anywhere — the O(tools²) scan must not run",
        );

        // Positive control: one selector on one server re-arms the scan for the whole call, and the
        // set spans the filtered server too — a tool's own candidates are subtracted by match count
        // inside `CandidateIndex::has_other_current_match`, never by omitting them here.
        let filtered =
            ServerEntry { exclude_tools: Some(vec!["a_search".to_string()]), ..stdio("npx") };
        let armed = config_with(&[("a", filtered), ("b", stdio("npx"))]);
        let index = collision_index(&armed, &cache, armed.tool_prefix());
        let candidates = index.as_ref().map(CandidateIndex::all_current);
        assert!(candidates.is_some_and(|set| set.contains("b_search")), "{candidates:?}");
        assert!(candidates.is_some_and(|set| set.contains("a_search")), "{candidates:?}");

        // …and skipping it changes nothing the model reads: the counts are identical either way,
        // which is the whole claim — a pure startup-cost fix, not a behaviour change.
        let described = build_proxy_description(&unfiltered, &cache, &[]);
        assert!(described.contains("\nServers: a (1 tools), b (1 tools)\n"), "{described}");
        let described = build_proxy_description(&armed, &cache, &[]);
        assert!(described.contains("\nServers: b (1 tools)\n"), "{described}");
    }

}
