//! `ToolMetadata` and the tool-name grammar.
//!
//! **This module is temporary by design.** See the integration note below: it is a
//! byte-faithful copy of what section 13e owns, kept here so 13d compiles and is testable
//! standalone. When 13e lands, this file is deleted and its `mod`/`pub use` lines in
//! [`crate::proxy`] are replaced by `pub use crate::renderers::{…}` — a delete, not a rewrite.
//!
//! See [`crate::proxy`] for the module overview.


use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;


use crate::config::{
    ServerEntry,
    ToolPrefix,
};
use crate::proxy::constants::{REGEX_DFA_SIZE_LIMIT, REGEX_SIZE_LIMIT};

// ==================================================================================================
// 2 · `ToolMetadata` and the tool-name grammar
//
// INTEGRATION NOTE (13e / MCP-200…MCP-207): `ToolMetadata` and the six naming functions below are
// `types.ts`' and are owned by section 13e, which lands them in `crate::renderers`. They are ported
// here byte-faithfully so 13d compiles and is testable standalone; when 13e lands, delete this
// block and replace it with `pub use crate::renderers::{…};` — the shapes are upstream's, so the
// swap is a delete, not a rewrite. `is_tool_allowed` here is the *simple* candidate-set form
// (`otherCurrentCandidates: Set<string>`); 13e owns the `ToolSelectorCandidateIndex` memoising form.
// ==================================================================================================

/// `types.ts:575` `ToolMetadata` — one model-visible MCP tool or resource tool.
///
/// `uiResourceUri` and `uiStreamMode` are **Cut 2** (MCP Apps); `uiVisibility` survives the cut
/// because `buildProxyDescription`'s counts use it to hide tools the server explicitly marked
/// app-only (13d §2, MCP-208).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetadata {
    /// Prefixed, model-visible name (e.g. `xcodebuild_list_sims`).
    pub name: String,
    /// The name the MCP server knows (e.g. `list_sims`) — what `tools/call` is sent.
    pub original_name: String,
    /// `tool.description ?? ""`.
    pub description: String,
    /// Resource tools only: the URI `resources/read` is issued against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    /// `_meta.ui.visibility`, when the server declared one. `None` == visible to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_visibility: Option<Vec<String>>,
    /// The raw JSON Schema, stored for `describe` and for the `Expected parameters:` error suffix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

impl ToolMetadata {
    /// A plain tool, for tests and for callers that only need the three required fields.
    #[must_use]
    pub fn new(name: impl Into<String>, original_name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            original_name: original_name.into(),
            description: description.into(),
            resource_uri: None,
            ui_visibility: None,
            input_schema: None,
        }
    }
}

/// `ui-tool-visibility.ts` `isUiToolVisibleToModel(v)` = `v === undefined || v.includes("model")`.
///
/// **Kept from the cut file** (13d §2): dropping it would expose to the model tools the server
/// explicitly marked app-only.
#[must_use]
pub fn is_ui_tool_visible_to_model(visibility: Option<&[String]>) -> bool {
    match visibility {
        None => true,
        Some(list) => list.iter().any(|entry| entry == "model"),
    }
}

/// `types.ts:651` `sanitizeServerPrefix(serverName, preserveProviderValid = true)`.
///
/// With the default `preserveProviderValid`, `[A-Za-z0-9_-]` is kept **verbatim** — so `-` survives —
/// and anything else is hex-escaped as `_<codepoint-hex>_`. This is the four-mode, hyphen-preserving
/// grammar; `cyrup_ext_subagents::exec::mcp_direct_tools::get_server_prefix` is a three-mode,
/// hyphen-*replacing* one ported from `pi-subagents`, and the two disagree for every hyphenated
/// server name. **MCP-178/MCP-205 open decision**, recommendation (b): upgrade `mcp_direct_tools` so
/// one tool-name grammar governs the process. This file implements the adapter's rule because
/// `rankSuggestions` and [`crate::proxy::execute_call`]'s phase 4 both test `toolName.starts_with(prefix + "_")`,
/// and under the other rule every hyphenated server silently stops matching.
#[must_use]
pub fn sanitize_server_prefix(server_name: &str) -> String {
    let mut out = String::with_capacity(server_name.len());
    for ch in server_name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
            out.push_str(&format!("{:x}", ch as u32));
            out.push('_');
        }
    }
    out
}

/// `types.ts:677` `getServerPrefix(serverName, mode)`.
///
/// `short` strips a trailing `-?mcp` (case-insensitively) and falls back to the literal `mcp` when
/// that empties the name; `mcp` mode produces `mcp__<sanitized>`; `none` yields the empty string,
/// which every caller treats as "no prefix".
#[must_use]
pub fn get_server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let stripped = strip_mcp_suffix(server_name);
            let short = sanitize_server_prefix(stripped);
            if short.is_empty() { "mcp".to_string() } else { short }
        }
        ToolPrefix::Mcp => format!("mcp__{}", sanitize_server_prefix(server_name)),
        ToolPrefix::Server => sanitize_server_prefix(server_name),
    }
}

/// `serverName.replace(/-?mcp$/i, "")` — the `short` mode's suffix strip, anchored at the end and
/// case-insensitive, with an optional single leading hyphen.
fn strip_mcp_suffix(server_name: &str) -> &str {
    let bytes = server_name.as_bytes();
    let len = bytes.len();
    if len < 3 {
        return server_name;
    }
    let tail = server_name.get(len - 3..).unwrap_or_default();
    if !tail.eq_ignore_ascii_case("mcp") {
        return server_name;
    }
    if len >= 4 && bytes.get(len - 4) == Some(&b'-') {
        return server_name.get(..len - 4).unwrap_or_default();
    }
    server_name.get(..len - 3).unwrap_or_default()
}

/// `types.ts:694` `formatToolName(toolName, serverName, prefix)` — note the `.` → `_` sanitisation
/// of the *tool* name, which `cyrup_ext_subagents`' copy omits (MCP-205 row 4).
#[must_use]
pub fn format_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = get_server_prefix(server_name, prefix);
    let sanitized = tool_name.replace('.', "_");
    if server_prefix.is_empty() { sanitized } else { format!("{server_prefix}_{sanitized}") }
}

/// `types.ts:770` `formatLegacyToolName` — the pre-2.x spelling, which sanitised `-` as well as `.`
/// and used the non-`preserveProviderValid` prefix.
fn format_legacy_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = match prefix {
        ToolPrefix::None => String::new(),
        _ => {
            let base = get_server_prefix(server_name, prefix);
            // `getLegacyServerPrefix` re-sanitises with `preserveProviderValid = false`, i.e. only
            // `[A-Za-z0-9]` survives verbatim.
            let mut out = String::with_capacity(base.len());
            for ch in base.chars() {
                if ch.is_ascii_alphanumeric() {
                    out.push(ch);
                } else {
                    out.push('_');
                    out.push_str(&format!("{:x}", ch as u32));
                    out.push('_');
                }
            }
            out
        }
    };
    let sanitized: String = tool_name.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();
    if server_prefix.is_empty() { sanitized } else { format!("{server_prefix}_{sanitized}") }
}

/// `types.ts:704` `resolveToolPrefix(definition, globalPrefix)` — the per-server override wins.
#[must_use]
pub fn resolve_tool_prefix(definition: Option<&ServerEntry>, global_prefix: ToolPrefix) -> ToolPrefix {
    definition.and_then(|entry| entry.tool_prefix).unwrap_or(global_prefix)
}

/// `types.ts:779` `getToolNameCandidates(toolName, serverName, prefix, includeLegacy = true)`.
///
/// The set every `includeTools` / `excludeTools` / `searchKeywords` pattern is matched against: the
/// bare name, the name under the effective prefix and under all three concrete modes, and — when
/// `include_legacy` — the same five under `-`→`_` normalisation plus the four legacy spellings.
/// An [`IndexSet`] rather than a `HashSet` so the insertion order upstream's `Set` preserves is
/// preserved here too; only membership is read today, but the order is free.
#[must_use]
pub fn get_tool_name_candidates(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_legacy: bool,
) -> IndexSet<String> {
    let mut candidates = IndexSet::new();
    candidates.insert(tool_name.to_string());
    candidates.insert(format_tool_name(tool_name, server_name, prefix));
    candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Server));
    candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Short));
    candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Mcp));
    if include_legacy {
        let legacy_tool_name = tool_name.replace('-', "_");
        candidates.insert(legacy_tool_name.clone());
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, prefix));
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, ToolPrefix::Server));
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, ToolPrefix::Short));
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, ToolPrefix::Mcp));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, prefix));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, ToolPrefix::Server));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, ToolPrefix::Short));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, ToolPrefix::Mcp));
        candidates.insert(format_tool_name(tool_name, server_name, prefix).replace('-', "_"));
        candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Server).replace('-', "_"));
        candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Short).replace('-', "_"));
        candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Mcp).replace('-', "_"));
    }
    candidates
}

/// `types.ts:814` `globToRegExp(pattern)` — `*` → `.*`, `?` → `.`, everything in
/// `[.+^${}()|[\]\\]` escaped, anchored at both ends.
///
/// Compiled with the same explicit ceilings as the search path; a pattern that will not compile
/// simply matches nothing, which is upstream's behaviour for a pattern that matches nothing.
fn glob_to_regex(pattern: &str) -> Option<regex::Regex> {
    let mut out = String::with_capacity(pattern.len() * 2 + 2);
    out.push('^');
    for ch in pattern.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('$');
    regex::RegexBuilder::new(&out)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .ok()
}

/// `types.ts:830` `matchesToolPattern(candidates, patterns)`.
///
/// An empty or absent pattern list is `false`. A pattern containing neither `*` nor `?` is an exact
/// membership test; a glob is compiled and tested against every candidate.
#[must_use]
pub fn matches_tool_pattern(candidates: &IndexSet<String>, patterns: Option<&[String]>) -> bool {
    let Some(patterns) = patterns else { return false };
    if patterns.is_empty() {
        return false;
    }
    for pattern in patterns {
        let is_glob = pattern.contains('*') || pattern.contains('?');
        if !is_glob && candidates.contains(pattern) {
            return true;
        }
        if is_glob
            && let Some(matcher) = glob_to_regex(pattern)
            && candidates.iter().any(|candidate| matcher.is_match(candidate))
        {
            return true;
        }
    }
    false
}

/// `types.ts:857` `indexHasOtherCurrentMatch(index, toolName, currentCandidates, pattern)`.
///
/// "Does this selector reach a tool other than mine?" A non-glob pattern collides when it is some
/// *other* server's candidate and not one of mine. A glob collides when it matches **more**
/// candidates overall than it matches of mine — which is why the caller's set spans every server
/// including this one, and why a naive `any(matches)` would report a self-match as a collision.
///
/// **Not memoised, unlike upstream post-`14c0e6c`.** Upstream hoists `matcherByPattern` and
/// `matchingCountByPattern` onto a `ToolSelectorCandidateIndex` built once per filtered server
/// (`types.ts:811-829`); this free function takes a bare `&IndexSet` and so recompiles the glob and
/// re-walks the whole candidate set once per **(tool, pattern)** pair — not, as an earlier revision
/// of this comment claimed, once per pattern. Only reached when a pattern matches a legacy-only
/// candidate, so it is a cost, never a behaviour difference; the planned close is to delete this
/// function in favour of [`crate::registration::CandidateIndex`], which is the memoising port, when
/// MCP-207 collapses the two selector paths (see MCP-249). `additionalCurrentCandidatesByToolName`
/// has no caller on this path and is not modelled.
fn index_has_other_current_match(
    all_candidates: &IndexSet<String>,
    current_candidates: &IndexSet<String>,
    pattern: &str,
) -> bool {
    let is_glob = pattern.contains('*') || pattern.contains('?');
    if !is_glob {
        return all_candidates.contains(pattern) && !current_candidates.contains(pattern);
    }
    let Some(matcher) = glob_to_regex(pattern) else { return false };
    let total = all_candidates.iter().filter(|candidate| matcher.is_match(candidate)).count();
    if total == 0 {
        return false;
    }
    let mine = current_candidates
        .iter()
        .filter(|candidate| all_candidates.contains(*candidate) && matcher.is_match(candidate))
        .count();
    total > mine
}

/// `types.ts:891` `matchesToolSelector(toolName, serverName, prefix, patterns,
/// otherCurrentCandidates)`.
///
/// **Two tiers, and the second is the whole point.** A pattern that matches one of the tool's
/// *current* names selects it outright. Only when that misses does the **legacy** residue — the
/// pre-2.x spellings, minus everything already in the current set — get a look, and then only for a
/// pattern that does not also reach some other configured tool. That is what stops a legacy
/// selector written for one server from silently capturing another server's tool after a rename.
fn matches_tool_selector(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: Option<&[String]>,
    all_current_candidates: Option<&IndexSet<String>>,
) -> bool {
    let Some(patterns) = patterns else { return false };
    if patterns.is_empty() {
        return false;
    }
    let current = get_tool_name_candidates(tool_name, server_name, prefix, false);
    if matches_tool_pattern(&current, Some(patterns)) {
        return true;
    }
    let Some(all_candidates) = all_current_candidates else {
        // No collision context: fall back to the full legacy-inclusive set.
        let legacy = get_tool_name_candidates(tool_name, server_name, prefix, true);
        return matches_tool_pattern(&legacy, Some(patterns));
    };
    let mut legacy = get_tool_name_candidates(tool_name, server_name, prefix, true);
    for candidate in &current {
        legacy.shift_remove(candidate);
    }
    patterns.iter().any(|pattern| {
        matches_tool_pattern(&legacy, Some(std::slice::from_ref(pattern)))
            && !index_has_other_current_match(all_candidates, &current, pattern)
    })
}

/// `types.ts:912` `isToolIncluded` — an absent or empty `includeTools` includes everything.
fn is_tool_included(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    all_current_candidates: Option<&IndexSet<String>>,
) -> bool {
    match include_tools {
        // `!Array.isArray(includeTools) || includeTools.length === 0` — both spellings of "no
        // allowlist" include everything.
        None | Some([]) => true,
        Some(patterns) => {
            matches_tool_selector(tool_name, server_name, prefix, Some(patterns), all_current_candidates)
        }
    }
}

/// `types.ts:940` `isToolAllowed(toolName, serverName, prefix, includeTools, excludeTools,
/// otherCurrentCandidates)` — the allowlist first, then the denylist.
///
/// `all_current_candidates` is the cross-server collision set (MCP-198), spanning **every**
/// cache-valid enabled server including this one.
#[must_use]
pub fn is_tool_allowed(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
    all_current_candidates: Option<&IndexSet<String>>,
) -> bool {
    is_tool_included(tool_name, server_name, prefix, include_tools, all_current_candidates)
        && !matches_tool_selector(
            tool_name,
            server_name,
            prefix,
            exclude_tools,
            all_current_candidates,
        )
}

// **De-duplicated at integration (MCP-203).** `resourceNameToToolName` lives in
// [`crate::registration`] with the rest of the naming grammar, and the copy that stood here had
// drifted: upstream's `result = "resource" + (result ? "_" + result : "")`
// (`resource-tools.ts:13`) yields `"resource"` for an all-punctuation name, where the copy yielded
// `"resource_"`. Every `read_<name>` resource tool that a server names `"///"` would have been
// registered under a name the cache could never match.
pub use crate::registration::resource_name_to_tool_name;

/// `tool-metadata.ts:154` `findToolByName(metadata, toolName)` — exact `name` match first,
/// otherwise compare with `-` globally replaced by `_` on **both** sides.
#[must_use]
pub fn find_tool_by_name<'a>(metadata: &'a [ToolMetadata], tool_name: &str) -> Option<&'a ToolMetadata> {
    if let Some(exact) = metadata.iter().find(|tool| tool.name == tool_name) {
        return Some(exact);
    }
    let normalized = tool_name.replace('-', "_");
    metadata.iter().find(|tool| tool.name.replace('-', "_") == normalized)
}

// **De-duplicated at integration (MCP-206).** `truncateAtWord` lives in
// [`crate::registration`], and the copy that stood here had drifted on units: JS `.length` and
// `.slice` count **UTF-16 code units** (`utils.ts:265-267`), which is what `registration.rs`
// measures; the copy counted Unicode scalar values, so an emoji-bearing description was cut at a
// different point than the description the cache and the subagent bridge compute.
pub use crate::registration::truncate_at_word;
