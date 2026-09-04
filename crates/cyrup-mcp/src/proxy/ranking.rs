//! `search-ranking.ts` — allocation-free integer scoring, no I/O
//! (MCP-172…MCP-178).
//!
//! See [`crate::proxy`] for the module overview.

use indexmap::{IndexMap, IndexSet};

use crate::config::{McpConfig, ServerEntry, ToolPrefix, locale_compare};
use crate::proxy::constants::{
    MIN_STEM_LENGTH, WEIGHT_DESCRIPTION, WEIGHT_KEYWORDS, WEIGHT_NAME, WEIGHT_ORIGINAL_NAME,
    WEIGHT_SERVER,
};
use crate::proxy::tool_metadata::{
    ToolMetadata, matches_tool_pattern, resolve_tool_prefix, server_prefix, tool_name_candidates,
};

// ==================================================================================================
// 3 · `search-ranking.ts` — 206 lines of allocation-free integer scoring, no I/O
//     (MCP-172, MCP-173, MCP-174, MCP-175, MCP-176, MCP-177, MCP-178)
// ==================================================================================================

/// `search-ranking.ts:54` `normalizeSearchText(value)` — three steps, **in this order**.
///
/// 1. `replace(/([a-z0-9])([A-Z])/g, "$1 $2")` — camelCase split, **before** lowercasing, so `ID`
///    does not split (the pattern needs a lowercase or digit *before* the uppercase). The JS global
///    replace is non-overlapping: it consumes both characters of a match, so `"aBcD"` becomes
///    `"a Bc D"`, and this scanner reproduces that by advancing two characters on a hit.
/// 2. `replace(/[_./:-]+/g, " ")` — the class is exactly `_ . / : -`, runs collapsed to one space.
/// 3. `toLowerCase()`.
///
/// Hand-written rather than `regex` (MCP-172): both patterns are trivial and a scanner keeps the
/// ranking path allocation-light.
#[must_use]
pub fn normalize_search_text(value: &str) -> String {
    // Step 1 — camelCase split.
    let chars: Vec<char> = value.chars().collect();
    let mut split = String::with_capacity(value.len() + 8);
    let mut index = 0usize;
    while index < chars.len() {
        let current = chars.get(index).copied().unwrap_or('\0');
        let next = chars.get(index + 1).copied();
        let boundary = matches!(next, Some(n) if n.is_ascii_uppercase())
            && (current.is_ascii_lowercase() || current.is_ascii_digit());
        if boundary {
            split.push(current);
            split.push(' ');
            if let Some(n) = next {
                split.push(n);
            }
            index += 2;
        } else {
            split.push(current);
            index += 1;
        }
    }

    // Step 2 — separator runs to a single space.
    let mut collapsed = String::with_capacity(split.len());
    let mut in_run = false;
    for ch in split.chars() {
        if matches!(ch, '_' | '.' | '/' | ':' | '-') {
            if !in_run {
                collapsed.push(' ');
            }
            in_run = true;
        } else {
            collapsed.push(ch);
            in_run = false;
        }
    }

    // Step 3.
    collapsed.to_lowercase()
}

/// `search-ranking.ts:62` `tokenize(value)` =
/// `normalizeSearchText(value).split(/[^a-z0-9]+/).filter(Boolean)`.
///
/// ASCII-only by construction: any non-`[a-z0-9]` byte is a separator, so a non-ASCII identifier
/// tokenizes to nothing. That is upstream's behaviour and is load-bearing for the coverage gate.
#[must_use]
pub fn tokenize(value: &str) -> Vec<String> {
    let normalized = normalize_search_text(value);
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in normalized.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// One scored `(server, tool)` pair — `search-ranking.ts:20` `RankedToolMatch`.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedToolMatch {
    /// The `mcpServers` key the tool came from.
    pub server: String,
    /// The tool's metadata record.
    pub tool: ToolMetadata,
    /// The integer score. **Always `0` on the regex path**, which is never sorted.
    pub score: i64,
}

/// The three-tier token ladder, shared by the field loop and the keyword loop.
///
/// Exactly one tier fires per `(field, query token)` pair, first match wins:
/// * the token is a field token ⇒ `weight * 4`;
/// * some field token prefixes the query token, **or** the query token prefixes a field token of at
///   least [`MIN_STEM_LENGTH`] characters ⇒ `weight * 2`;
/// * the raw (normalised) value contains the token ⇒ `weight * 1`.
///
/// **The stem rule is deliberately asymmetric.** `field.starts_with(token)` matches at any length,
/// but `token.starts_with(field)` only when the field token is ≥ 4 characters, because real
/// descriptions tokenize possessives into single letters.
fn token_bonus(
    weight: i64,
    field_tokens: &[String],
    raw_contains: bool,
    token: &str,
) -> Option<i64> {
    if field_tokens.iter().any(|field_token| field_token == token) {
        return Some(weight * 4);
    }
    let stemmed = field_tokens.iter().any(|field_token| {
        field_token.starts_with(token)
            || (field_token.chars().count() >= MIN_STEM_LENGTH
                && token.starts_with(field_token.as_str()))
    });
    if stemmed {
        return Some(weight * 2);
    }
    if raw_contains {
        return Some(weight);
    }
    None
}

/// The phrase ladder for one field: exact ⇒ `×14` (also sets `whole_field_exact`), prefix ⇒ `×9`,
/// substring ⇒ `×6`. First match wins; a miss contributes nothing and does not set `phrase_matched`.
fn phrase_bonus(weight: i64, value: &str, normalized_query: &str) -> Option<(i64, bool)> {
    if value == normalized_query {
        return Some((weight * 14, true));
    }
    if value.starts_with(normalized_query) {
        return Some((weight * 9, false));
    }
    if value.contains(normalized_query) {
        return Some((weight * 6, false));
    }
    None
}

/// `search-ranking.ts:65` `scoreToolMatch(tool, server, query, keywords?)`.
///
/// `None` is "this tool does not match at all" — the coverage gate's verdict, not a zero score.
///
/// Steps, in order (13d §7):
/// 1. normalise and tokenize the query; **empty tokens ⇒ `None`**;
/// 2. four fields — `name`, `originalName`, `server`, `description` — in that exact order, each
///    normalised but **not trimmed** (a leading space in a description defeats `starts_with`);
/// 3. one phrase bonus per field;
/// 4. one token bonus per (field, query token);
/// 5. keywords, only when `Some` and non-empty — the phrase bonus is a **max over phrases** added
///    **once**, deliberately, so a query spanning two unrelated keywords cannot collect it twice;
/// 6. the coverage gate;
/// 7. the final bonuses.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn score_tool_match(
    tool: &ToolMetadata,
    server: &str,
    query: &str,
    keywords: Option<&[String]>,
) -> Option<i64> {
    let normalized_query = normalize_search_text(query).trim().to_string();
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return None;
    }

    // Step 2 — the field order is the JS object literal's insertion order, and it matters because
    // the first phrase hit per field is the only one that scores.
    let fields: [(i64, String); 4] = [
        (WEIGHT_NAME, normalize_search_text(&tool.name)),
        (
            WEIGHT_ORIGINAL_NAME,
            normalize_search_text(&tool.original_name),
        ),
        (WEIGHT_SERVER, normalize_search_text(server)),
        (WEIGHT_DESCRIPTION, normalize_search_text(&tool.description)),
    ];

    let mut score: i64 = 0;
    let mut phrase_matched = false;
    let mut whole_field_exact = false;
    let mut matched_tokens: IndexSet<String> = IndexSet::new();

    for (weight, value) in &fields {
        let field_tokens = tokenize(value);
        if let Some((bonus, exact)) = phrase_bonus(*weight, value, &normalized_query) {
            score += bonus;
            phrase_matched = true;
            whole_field_exact |= exact;
        }
        for token in &query_tokens {
            if let Some(bonus) = token_bonus(
                *weight,
                &field_tokens,
                value.contains(token.as_str()),
                token,
            ) {
                score += bonus;
                matched_tokens.insert(token.clone());
            }
        }
    }

    // Step 5 — configured keywords are discrete phrases, so the phrase-level bonus is computed per
    // phrase (best match wins) rather than on a joined string.
    if let Some(keywords) = keywords
        && !keywords.is_empty()
    {
        let weight = WEIGHT_KEYWORDS;
        let phrases: Vec<String> = keywords
            .iter()
            .map(|keyword| normalize_search_text(keyword).trim().to_string())
            .filter(|phrase| !phrase.is_empty())
            .collect();
        let mut phrase_score: i64 = 0;
        for phrase in &phrases {
            if let Some((bonus, exact)) = phrase_bonus(weight, phrase, &normalized_query) {
                phrase_score = phrase_score.max(bonus);
                phrase_matched = true;
                whole_field_exact |= exact;
            }
        }
        score += phrase_score;

        let keyword_tokens: Vec<String> =
            phrases.iter().flat_map(|phrase| tokenize(phrase)).collect();
        for token in &query_tokens {
            let raw_contains = phrases.iter().any(|phrase| phrase.contains(token.as_str()));
            if let Some(bonus) = token_bonus(weight, &keyword_tokens, raw_contains, token) {
                score += bonus;
                matched_tokens.insert(token.clone());
            }
        }
    }

    // Step 6 — the coverage gate. Without a phrase match, a 1-2 token query must match **all** its
    // tokens and a longer query must reach 0.6.
    let matched = matched_tokens.len();
    let total = query_tokens.len();
    let full_coverage = matched == total; // integer comparison, never a float equality
    let coverage = matched as f64 / total as f64;
    if !phrase_matched
        && (if total <= 2 {
            !full_coverage
        } else {
            coverage < 0.6
        })
    {
        return None;
    }

    // Step 7 — the final bonuses. `Math.round` on a positive value is Rust's `f64::round`.
    score += if full_coverage {
        25
    } else {
        (coverage * 10.0).round() as i64
    };
    if let Some(first) = query_tokens.first()
        && tokenize(
            &fields
                .first()
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
        )
        .iter()
        .any(|token| token == first)
    {
        score += 8;
    }
    if whole_field_exact {
        score += 20;
    }
    Some(score)
}

/// `search-ranking.ts:30` `resolveSearchKeywords(definition, toolOriginalName, serverName,
/// globalPrefix)`.
///
/// Keys match by original name, prefixed name, and glob — the same candidate set
/// `includeTools`/`excludeTools` use — and **all** matching entries are unioned, deduped, in key
/// order. A missing / non-object / array map yields `[]`; non-string and blank values are dropped.
///
/// **Two divergences forced by `ServerEntry::search_keywords`'s current type**
/// (`Option<BTreeMap<String, Vec<String>>>`, `config.rs`), both reported for integration:
/// * a `BTreeMap` **sorts** its keys, where `Object.entries` yields insertion order — so the union
///   order can differ from upstream's when two glob keys both match. `IndexMap` is the fix.
/// * `lenient` rejects the whole field when *any* value is not a `string[]`, where upstream drops
///   only the offending key (and only the offending element).
///
/// Configured keywords are searchable by ranked query **and** by regex, but never appear in
/// schemas, `describe` output, or the metadata cache.
#[must_use]
pub fn resolve_search_keywords(
    definition: Option<&ServerEntry>,
    tool_original_name: &str,
    server_name: &str,
    global_prefix: ToolPrefix,
) -> Vec<String> {
    let Some(map) = definition.and_then(|entry| entry.search_keywords.as_ref()) else {
        return Vec::new();
    };
    let candidates = tool_name_candidates(
        tool_original_name,
        server_name,
        resolve_tool_prefix(definition, global_prefix),
        true,
    );
    let mut keywords: Vec<String> = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for (pattern, values) in map {
        if !matches_tool_pattern(&candidates, std::slice::from_ref(pattern)) {
            continue;
        }
        for value in values {
            let trimmed = value.trim();
            if trimmed.is_empty() || seen.contains(trimmed) {
                continue;
            }
            seen.insert(trimmed.to_string());
            keywords.push(trimmed.to_string());
        }
    }
    keywords
}

/// The rank tie-break — `String.prototype.localeCompare` with no locale (ICU root collation).
///
/// **MCP-171** offered three options; this takes the exact one by reusing
/// [`crate::config::locale_compare`], the `feruca` UCA collator already proven against Node in
/// `cyrup-tools/src/tools/ls.rs` and `cyrup-config/src/model.rs`. A fourth hand-rolled ASCII
/// approximation is exactly the drift those two exist to prevent, and the collator is only ever
/// asked to order equal-score results and a hint list — never to decide which tools match.
#[must_use]
pub fn rank_collate(left: &str, right: &str) -> std::cmp::Ordering {
    locale_compare(left, right)
}

/// `search-ranking.ts:152` `rankToolMatches(state, query, server?, includeKeywords = true)`.
///
/// Walks `state.toolMetadata` in **insertion order** (MCP-170), skips disabled servers and — when
/// `server` is set — non-matching ones, and sorts by score descending then [`rank_collate`]
/// ascending.
///
/// `has_keywords` is `includeKeywords && definition?.searchKeywords !== undefined`: **an empty
/// object still counts as present**, which changes whether `keywords` is `Some(&[])` or `None`.
/// `Some(&[])` is a no-op by [`score_tool_match`]'s non-empty guard — reproduced, not simplified,
/// because the distinction is what the "does not change scoring when the keyword list is empty"
/// conformance case pins.
#[must_use]
pub fn rank_tool_matches(
    config: &McpConfig,
    tool_metadata: &IndexMap<String, Vec<ToolMetadata>>,
    query: &str,
    server: Option<&str>,
    include_keywords: bool,
) -> Vec<RankedToolMatch> {
    let global_prefix = config.tool_prefix();
    let mut matches: Vec<RankedToolMatch> = Vec::new();
    for (server_name, metadata) in tool_metadata {
        if let Some(filter) = server
            && server_name != filter
        {
            continue;
        }
        let definition = config.mcp_servers.get(server_name);
        if definition.is_some_and(ServerEntry::is_disabled) {
            continue;
        }
        let has_keywords =
            include_keywords && definition.is_some_and(|entry| entry.search_keywords.is_some());
        for tool in metadata {
            let keywords = if has_keywords {
                Some(resolve_search_keywords(
                    definition,
                    &tool.original_name,
                    server_name,
                    global_prefix,
                ))
            } else {
                None
            };
            if let Some(score) = score_tool_match(tool, server_name, query, keywords.as_deref()) {
                matches.push(RankedToolMatch {
                    server: server_name.clone(),
                    tool: tool.clone(),
                    score,
                });
            }
        }
    }
    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| rank_collate(&a.tool.name, &b.tool.name))
    });
    matches
}

/// The result of [`paginate`] — `search-ranking.ts:176`'s return object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The slice actually returned.
    pub items: Vec<T>,
    /// `items.length` of the **unpaginated** input.
    pub total: usize,
    /// Whether `next_offset` is `Some`.
    pub has_more: bool,
    /// `safeOffset + page.length`, but only when that is still inside the list.
    pub next_offset: Option<usize>,
}

/// `search-ranking.ts:176` `paginate(items, offset, limit)`.
///
/// `offset` and `limit` arrive from JSON as numbers, so both are `f64` here:
/// `safeOffset = Number.isFinite(offset) ? Math.max(0, Math.trunc(offset)) : 0`;
/// `safeLimit = Number.isFinite(limit) ? Math.max(1, Math.trunc(limit)) : 1`.
/// JS `slice` clamps both ends and never throws — Rust must clamp explicitly.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn paginate<T: Clone>(items: &[T], offset: f64, limit: f64) -> Page<T> {
    let safe_offset: usize = if offset.is_finite() {
        offset.trunc().max(0.0) as usize
    } else {
        0
    };
    let safe_limit: usize = if limit.is_finite() {
        limit.trunc().max(1.0) as usize
    } else {
        1
    };
    let total = items.len();
    let start = safe_offset.min(total);
    let end = start.saturating_add(safe_limit).min(total);
    let page: Vec<T> = items.get(start..end).unwrap_or_default().to_vec();
    // Upstream computes `safeOffset + page.length`, NOT `start + page.length`; for an offset past
    // the end the page is empty, so the two agree.
    let next_offset_raw = safe_offset.saturating_add(page.len());
    let has_more = next_offset_raw < total;
    Page {
        items: page,
        total,
        has_more,
        next_offset: if has_more {
            Some(next_offset_raw)
        } else {
            None
        },
    }
}

/// `search-ranking.ts:194` `rankSuggestions(state, name, limit)` — the "Did you mean:" list.
///
/// Strips the **longest** matching server prefix — probing modes `server`, `short` and `mcp`
/// regardless of the configured mode, with `none` deliberately excluded because it yields an empty
/// prefix — and re-ranks the remainder with **keywords disabled**, so a suggestion never comes from
/// a configured alias.
#[must_use]
pub fn rank_suggestions(
    config: &McpConfig,
    tool_metadata: &IndexMap<String, Vec<ToolMetadata>>,
    name: &str,
    limit: usize,
) -> Vec<String> {
    let mut stripped: Vec<String> = Vec::new();
    for server in config.mcp_servers.keys() {
        for mode in [ToolPrefix::Server, ToolPrefix::Short, ToolPrefix::Mcp] {
            let candidate = server_prefix(server, mode);
            if candidate.is_empty() || !name.starts_with(&format!("{candidate}_")) {
                continue;
            }
            stripped.push(candidate);
        }
    }
    // `sort((a, b) => b.length - a.length)` — descending prefix length, stably, so ties keep
    // configuration order.
    stripped.sort_by_key(|candidate| std::cmp::Reverse(candidate.len()));
    let query = stripped
        .first()
        .and_then(|candidate| name.get(candidate.len() + 1..).map(str::to_string))
        .unwrap_or_else(|| name.to_string());
    rank_tool_matches(config, tool_metadata, &query, None, false)
        .into_iter()
        .take(limit)
        .map(|entry| entry.tool.name)
        .collect()
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
    use crate::proxy::testsupport::{config_with, definition_with_keywords, metadata_with, tool};

    // ---- MCP-172 · `normalizeSearchText` / `tokenize` ---------------------------------------------

    #[test]
    fn normalize_splits_camel_case_before_lowercasing() {
        // `ID` does not split: the pattern needs a lowercase or digit BEFORE the uppercase.
        assert_eq!(
            normalize_search_text("getUserID_v2/foo"),
            "get user id v2 foo"
        );
        // The separator class is exactly `_ . / : -`, runs collapsed to one space.
        assert_eq!(normalize_search_text("a__b..c//d::e--f"), "a b c d e f");
        // The global replace is non-overlapping: it consumes both characters of a match.
        assert_eq!(normalize_search_text("aBcD"), "a bc d");
    }

    #[test]
    fn tokenize_drops_empties_and_non_ascii() {
        assert!(tokenize("").is_empty());
        assert_eq!(tokenize("get_user_id"), vec!["get", "user", "id"]);
        // Non-ASCII identifiers tokenize to nothing — upstream's ASCII-only split.
        assert!(tokenize("日本語").is_empty());
    }

    // ---- MCP-195 · the eleven upstream ranking cases ----------------------------------------------

    /// `search ranking` › "ranks an exact name above a description match".
    #[test]
    fn ranks_an_exact_name_above_a_description_match() {
        let exact = score_tool_match(
            &tool("search_records", "Find records"),
            "demo",
            "search",
            None,
        )
        .expect("exact name matches");
        let description = score_tool_match(
            &tool("find_records", "Search records"),
            "demo",
            "search",
            None,
        )
        .expect("description matches");
        assert!(
            exact > description,
            "exact {exact} should beat description {description}"
        );
    }

    /// `search ranking` › "drops partial two-token matches".
    #[test]
    fn drops_partial_two_token_matches() {
        assert_eq!(
            score_tool_match(
                &tool("search_records", "Find records"),
                "demo",
                "search missing",
                None
            ),
            None
        );
    }

    /// `search ranking` › "ignores single-letter possessive tokens instead of stem-matching them".
    #[test]
    fn ignores_single_letter_possessive_tokens() {
        // "project's" tokenizes to ["project", "s"]; a bare "s" must not match "simulator".
        assert_eq!(
            score_tool_match(
                &tool("sync_icon", "Add an icon to your project's icons file."),
                "better-icons",
                "simulator",
                None
            ),
            None
        );
        // Real stems still match: "sync" (4+ chars) may prefix-match "synchronize".
        assert!(
            score_tool_match(
                &tool("sync_icon", "Sync an icon."),
                "better-icons",
                "synchronize",
                None
            )
            .is_some()
        );
    }

    /// `search ranking` › "matches through configured keywords where the query would otherwise miss".
    #[test]
    fn matches_through_configured_keywords() {
        let advanced = tool(
            "search_records_advanced",
            "Advanced record search with filters",
        );
        let both = ["fuzzy lookup".to_string(), "legacy".to_string()];
        let one = ["fuzzy lookup".to_string()];

        assert_eq!(
            score_tool_match(&advanced, "demo", "fuzzy lookup", None),
            None
        );
        assert!(score_tool_match(&advanced, "demo", "fuzzy lookup", Some(&both)).is_some());
        // Single-token queries pass the coverage gate through keyword tokens too.
        assert_eq!(score_tool_match(&advanced, "demo", "fuzzy", None), None);
        assert!(score_tool_match(&advanced, "demo", "fuzzy", Some(&one)).is_some());
    }

    /// `search ranking` › "ranks an exact keyword alias above a description phrase match".
    #[test]
    fn ranks_an_exact_keyword_alias_above_a_description_phrase_match() {
        let keywords = ["fuzzy lookup".to_string()];
        let aliased = score_tool_match(
            &tool(
                "search_records_advanced",
                "Advanced record search with filters",
            ),
            "demo",
            "fuzzy lookup",
            Some(&keywords),
        )
        .expect("alias matches");
        let description = score_tool_match(
            &tool("record_search", "Fuzzy lookup across records"),
            "demo",
            "fuzzy lookup",
            None,
        )
        .expect("description matches");
        assert!(
            aliased > description,
            "alias {aliased} should beat description {description}"
        );
    }

    /// `search ranking` › "scores an exact alias above incidental cross-phrase token matches".
    ///
    /// The phrase bonus is a **max over phrases** added **once**, so "lookup legacy" — which spans
    /// two unrelated keywords — may token-match but must not collect a phrase bonus.
    #[test]
    fn scores_an_exact_alias_above_incidental_cross_phrase_token_matches() {
        let advanced = tool(
            "search_records_advanced",
            "Advanced record search with filters",
        );
        let keywords = ["fuzzy lookup".to_string(), "legacy".to_string()];
        let exact = score_tool_match(&advanced, "demo", "fuzzy lookup", Some(&keywords))
            .expect("exact alias matches");
        let cross = score_tool_match(&advanced, "demo", "lookup legacy", Some(&keywords))
            .expect("cross-phrase matches");
        assert!(
            exact > cross,
            "exact {exact} should beat cross-phrase {cross}"
        );
    }

    /// `search ranking` › "does not change scoring when the keyword list is empty".
    ///
    /// `Some(&[])` is a no-op by [`score_tool_match`]'s non-empty guard — which is exactly why the
    /// `Some([])` / `None` distinction in [`rank_tool_matches`] is reproduced rather than collapsed.
    #[test]
    fn empty_keyword_list_does_not_change_scoring() {
        let advanced = tool("search_records_advanced", "Advanced record search");
        assert_eq!(
            score_tool_match(&advanced, "demo", "advanced", Some(&[])),
            score_tool_match(&advanced, "demo", "advanced", None)
        );
    }

    /// `search ranking` › "paginates including offsets beyond the result set".
    #[test]
    fn paginates_including_offsets_beyond_the_result_set() {
        let items = vec!["a", "b", "c"];
        assert_eq!(
            paginate(&items, 1.0, 1.0),
            Page {
                items: vec!["b"],
                total: 3,
                has_more: true,
                next_offset: Some(2)
            }
        );
        assert_eq!(
            paginate(&items, 5.0, 1.0),
            Page {
                items: Vec::new(),
                total: 3,
                has_more: false,
                next_offset: None
            }
        );
    }

    /// `resolveSearchKeywords` › "matches keys by original name, prefixed name, and glob".
    #[test]
    fn resolve_search_keywords_matches_by_original_prefixed_and_glob() {
        let cases: [(&str, &[&str]); 4] = [
            ("search_records_advanced", &["fuzzy lookup"]),
            ("demo_search_records_advanced", &["fuzzy lookup"]),
            ("search_*", &["records"]),
            ("*", &["records"]),
        ];
        let expected = ["fuzzy lookup", "fuzzy lookup", "records", "records"];
        let names = [
            "search_records_advanced",
            "search_records_advanced",
            "search_records_advanced",
            "anything",
        ];
        for (index, (key, values)) in cases.iter().enumerate() {
            let entry = definition_with_keywords(&[(key, values)]);
            assert_eq!(
                resolve_search_keywords(Some(&entry), names[index], "demo", ToolPrefix::Server),
                vec![expected[index].to_string()],
                "case {index} ({key})"
            );
        }
    }

    /// `resolveSearchKeywords` › "unions and dedupes values from all matching keys".
    #[test]
    fn resolve_search_keywords_unions_and_dedupes() {
        let entry = definition_with_keywords(&[
            ("search_*", &["records", "fuzzy lookup"]),
            ("search_records_advanced", &["fuzzy lookup", "legacy"]),
        ]);
        assert_eq!(
            resolve_search_keywords(
                Some(&entry),
                "search_records_advanced",
                "demo",
                ToolPrefix::Server
            ),
            vec![
                "records".to_string(),
                "fuzzy lookup".to_string(),
                "legacy".to_string()
            ]
        );
    }

    /// `resolveSearchKeywords` › "returns nothing for non-matching keys or malformed config".
    ///
    /// The malformed-value arms upstream asserts (`"not-an-array"`, `["ok", 42, "  "]`) cannot be
    /// expressed while `ServerEntry::search_keywords` is `Option<BTreeMap<String, Vec<String>>>`:
    /// `lenient` has already dropped the whole field. The blank-value drop is still asserted, and
    /// the type-level divergence is recorded on [`resolve_search_keywords`].
    #[test]
    fn resolve_search_keywords_returns_nothing_for_non_matching_or_malformed() {
        let other = definition_with_keywords(&[("other_tool", &["nope"])]);
        assert!(
            resolve_search_keywords(
                Some(&other),
                "search_records_advanced",
                "demo",
                ToolPrefix::Server
            )
            .is_empty()
        );
        let blanks = definition_with_keywords(&[("search_records_advanced", &["ok", "  "])]);
        assert_eq!(
            resolve_search_keywords(
                Some(&blanks),
                "search_records_advanced",
                "demo",
                ToolPrefix::Server
            ),
            vec!["ok".to_string()]
        );
        assert!(
            resolve_search_keywords(None, "search_records_advanced", "demo", ToolPrefix::Server)
                .is_empty()
        );
    }

    // ---- MCP-175 · the coverage gate --------------------------------------------------------------

    #[test]
    fn coverage_gate_admits_two_of_three_and_refuses_one_of_three() {
        // No phrase match anywhere, so only the coverage ratio decides.
        let target = tool("alpha_bravo", "charlie delta");
        // 3 tokens, 2 matched = 0.667 ≥ 0.6 — survives.
        assert!(score_tool_match(&target, "srv", "alpha bravo zulu", None).is_some());
        // 3 tokens, 1 matched = 0.333 — dropped.
        assert_eq!(
            score_tool_match(&target, "srv", "alpha yankee zulu", None),
            None
        );
        // 2 tokens, 1 matched — a short query must match ALL its tokens.
        assert_eq!(score_tool_match(&target, "srv", "alpha zulu", None), None);
    }

    // ---- MCP-178 · `rankSuggestions` over a hyphenated server -------------------------------------

    /// The whole point of the four-mode, hyphen-**preserving** `sanitizeServerPrefix`: under
    /// `cyrup-ext-subagents`' hyphen-replacing rule the prefix would be `linear_server`, the
    /// `starts_with(prefix + "_")` test would fail, and the remainder would never be stripped.
    ///
    /// **Correction to 13d's verify line for MCP-178**, which names `linear-server_isues` →
    /// `linear-server_issues`: this ranker has no edit distance. `"isues"` tokenizes to `["isues"]`,
    /// `"issues".starts_with("isues")` is false, and a one-token query that matches none of its
    /// tokens is dropped by the coverage gate — upstream returns `[]` for that input too. The
    /// assertion below uses a near-miss the algorithm can actually resolve (a singular/plural slip),
    /// which is the behaviour the unit exists to pin.
    #[test]
    fn rank_suggestions_strips_a_hyphenated_server_prefix() {
        let config = config_with(&[("linear-server", ServerEntry::default())]);
        let metadata = metadata_with(&[(
            "linear-server",
            vec![ToolMetadata::new(
                "linear-server_issues",
                "issues",
                "List issues",
            )],
        )]);
        assert_eq!(
            rank_suggestions(&config, &metadata, "linear-server_issue", 5),
            vec!["linear-server_issues".to_string()]
        );
        // No edit distance: a transposed/dropped letter falls off the coverage gate, upstream and here.
        assert!(rank_suggestions(&config, &metadata, "linear-server_isues", 5).is_empty());
        assert_eq!(
            server_prefix("linear-server", ToolPrefix::Server),
            "linear-server"
        );
        assert_eq!(server_prefix("gh-mcp", ToolPrefix::Short), "gh");
        assert_eq!(server_prefix("gh-mcp", ToolPrefix::Mcp), "mcp__gh-mcp");
        assert_eq!(server_prefix("gh-mcp", ToolPrefix::None), "");
    }

    #[test]
    fn longest_prefix_wins_for_lazy_discovery() {
        // Two servers whose prefixes nest: `foo-bar_x` must resolve against `foo-bar`, not `foo`.
        let mut candidates = vec![
            ("foo".to_string(), server_prefix("foo", ToolPrefix::Server)),
            (
                "foo-bar".to_string(),
                server_prefix("foo-bar", ToolPrefix::Server),
            ),
        ];
        candidates.retain(|(_, prefix)| "foo-bar_x".starts_with(&format!("{prefix}_")));
        candidates.sort_by_key(|(_, prefix)| std::cmp::Reverse(prefix.len()));
        assert_eq!(
            candidates.first().map(|(name, _)| name.as_str()),
            Some("foo-bar")
        );
    }
}
