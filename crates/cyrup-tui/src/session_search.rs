//! The session-selector search query-DSL (spec/tui/05 §6; port of Pi's
//! `components/session-selector-search.ts` (194)). The `/resume` picker accepts a small search
//! language over the persisted-session list: whitespace-separated **fuzzy** tokens, `"quoted
//! phrases"` matched as normalized substrings, and a `re:<pattern>` **regex** mode. A `NameFilter`
//! restricts to named sessions, and a `SortMode` orders the survivors (incoming order for
//! `Recent`, ascending score for `Relevance`).
//!
//! This module is the **pure** matcher: it parses a query into a [`ParsedSearchQuery`] and scores an
//! already-assembled per-session search text (`{id} {name} {allMessagesText} {cwd}`,
//! `session-selector-search.ts:26`). The chrome (`app.rs`) assembles that text from
//! [`cyrup_session_svc::SessionInfo`] and applies the [`NameFilter`]/[`SortMode`].
//!
//! **Regex `re:` mode** (`session-selector-search.ts:44-56`) compiles a JS `RegExp`. cyrup has no
//! approved regex dependency, so the prefix is recognized and surfaced as a one-line *unsupported*
//! error rather than silently matching nothing; wiring a real engine is the single dep-gated residual
//! noted in the gap doc. Every other branch is 1:1.

use crate::fuzzy::fuzzy_match;

/// Whether to keep all sessions or only named ones (`NameFilter`, `session-selector-search.ts:9`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameFilter {
    /// Keep every session (`"all"`).
    All,
    /// Keep only sessions with a non-blank name (`"named"`).
    Named,
}

/// How matching sessions are ordered (`SortMode`, `session-selector-search.ts:5`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    /// Parent→child thread order (filter only, keep incoming order — same as `Recent` here since the
    /// caller supplies the threaded order).
    Threaded,
    /// Filter only, keep incoming order (`"recent"`).
    Recent,
    /// Sort by ascending score, tie-break newest-first (`"relevance"`).
    Relevance,
}

/// One parsed token: a loose `Fuzzy` subsequence match, or a normalized `Phrase` substring
/// (`{kind, value}`, `session-selector-search.ts:11`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// A fuzzy (subsequence) token.
    Fuzzy,
    /// A `"quoted phrase"` matched as a normalized substring.
    Phrase,
}

/// A parsed search token (`{kind, value}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchToken {
    /// Whether this token matches fuzzily or as a phrase.
    pub kind: TokenKind,
    /// The token text.
    pub value: String,
}

/// The query mode (`mode: "tokens" | "regex"`, `session-selector-search.ts:12`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryMode {
    /// Whitespace/quote tokenized fuzzy+phrase matching.
    Tokens,
    /// `re:<pattern>` regex matching (dep-gated; see module docs).
    Regex,
}

/// A parsed search query (`ParsedSearchQuery`, `session-selector-search.ts:11-18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSearchQuery {
    /// Tokens vs. regex.
    pub mode: QueryMode,
    /// The fuzzy/phrase tokens (empty in regex mode).
    pub tokens: Vec<SearchToken>,
    /// The raw `re:` pattern (regex mode only; matching is dep-gated).
    pub regex_pattern: Option<String>,
    /// When set, parsing failed and matching should treat every session as non-matching
    /// (`error?: string`, `:17`).
    pub error: Option<String>,
}

/// Lowercase + collapse runs of whitespace to a single space + trim
/// (`normalizeWhitespaceLower`, `session-selector-search.ts:20`).
pub fn normalize_whitespace_lower(text: &str) -> String {
    let lowered = text.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_ws = false;
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out.trim().to_string()
}

/// `true` when the session has a non-blank name (`hasSessionName`, `session-selector-search.ts:30`).
pub fn has_session_name(name: Option<&str>) -> bool {
    name.is_some_and(|n| !n.trim().is_empty())
}

/// Parse a query string into a [`ParsedSearchQuery`] (`parseSearchQuery`,
/// `session-selector-search.ts:39-110`).
///
/// - Empty → an empty token list (matches everything).
/// - `re:<pattern>` → regex mode (empty pattern → an `"Empty regex"` error).
/// - Otherwise a quote-aware tokenizer: `"…"` opens/closes a phrase token, unquoted whitespace splits
///   fuzzy tokens. An **unbalanced** closing quote falls back to plain whitespace tokenization (`:96`).
pub fn parse_search_query(query: &str) -> ParsedSearchQuery {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return ParsedSearchQuery {
            mode: QueryMode::Tokens,
            tokens: Vec::new(),
            regex_pattern: None,
            error: None,
        };
    }

    // Regex mode: `re:<pattern>`.
    if let Some(rest) = trimmed.strip_prefix("re:") {
        let pattern = rest.trim();
        if pattern.is_empty() {
            return ParsedSearchQuery {
                mode: QueryMode::Regex,
                tokens: Vec::new(),
                regex_pattern: None,
                error: Some("Empty regex".to_string()),
            };
        }
        return ParsedSearchQuery {
            mode: QueryMode::Regex,
            tokens: Vec::new(),
            regex_pattern: Some(pattern.to_string()),
            error: None,
        };
    }

    // Token mode with quote support.
    let mut tokens: Vec<SearchToken> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let mut had_unclosed_quote = false;

    let flush = |buf: &mut String, kind: TokenKind, tokens: &mut Vec<SearchToken>| {
        let v = buf.trim().to_string();
        buf.clear();
        if v.is_empty() {
            return;
        }
        tokens.push(SearchToken { kind, value: v });
    };

    for ch in trimmed.chars() {
        if ch == '"' {
            if in_quote {
                flush(&mut buf, TokenKind::Phrase, &mut tokens);
                in_quote = false;
            } else {
                flush(&mut buf, TokenKind::Fuzzy, &mut tokens);
                in_quote = true;
            }
            continue;
        }
        if !in_quote && ch.is_whitespace() {
            flush(&mut buf, TokenKind::Fuzzy, &mut tokens);
            continue;
        }
        buf.push(ch);
    }

    if in_quote {
        had_unclosed_quote = true;
    }

    // Unbalanced quotes → plain whitespace tokenization (`:96-106`).
    if had_unclosed_quote {
        let tokens = trimmed
            .split_whitespace()
            .map(|t| SearchToken { kind: TokenKind::Fuzzy, value: t.to_string() })
            .collect();
        return ParsedSearchQuery {
            mode: QueryMode::Tokens,
            tokens,
            regex_pattern: None,
            error: None,
        };
    }

    flush(&mut buf, if in_quote { TokenKind::Phrase } else { TokenKind::Fuzzy }, &mut tokens);

    ParsedSearchQuery { mode: QueryMode::Tokens, tokens, regex_pattern: None, error: None }
}

/// Score `text` against a parsed query (`matchSession`, `session-selector-search.ts:113-152`):
/// `Some(score)` (lower = better) when it matches, `None` otherwise. Phrase tokens match a normalized
/// substring (score `idx*0.1`), fuzzy tokens via [`fuzzy_match`]; an empty token list matches with
/// score `0`. Regex mode is dep-gated → always `None`.
pub fn match_text(text: &str, parsed: &ParsedSearchQuery) -> Option<f64> {
    if parsed.error.is_some() {
        return None;
    }
    if parsed.mode == QueryMode::Regex {
        // No approved regex engine — recognized but unsupported (see module docs).
        return None;
    }
    if parsed.tokens.is_empty() {
        return Some(0.0);
    }

    let mut total = 0.0f64;
    let mut normalized: Option<String> = None;
    for token in &parsed.tokens {
        match token.kind {
            TokenKind::Phrase => {
                let norm = normalized.get_or_insert_with(|| normalize_whitespace_lower(text));
                let phrase = normalize_whitespace_lower(&token.value);
                if phrase.is_empty() {
                    continue;
                }
                // A phrase token that does not occur at all disqualifies the row outright — `?`
                // is that `return None`, not a silently skipped token.
                total += norm.find(&phrase)? as f64 * 0.1;
            }
            // Same rule for a fuzzy token: no match means the row does not match.
            TokenKind::Fuzzy => total += fuzzy_match(&token.value, text)?,
        }
    }
    Some(total)
}

/// A session row the matcher orders: the assembled search `text`, its `name`, and a `modified`
/// recency key (newer = larger) for the `Relevance` tie-break.
#[derive(Clone, Debug)]
pub struct SearchRow<T> {
    /// The assembled search text (`{id} {name} {allMessagesText} {cwd}`).
    pub text: String,
    /// The session name (drives [`NameFilter::Named`]).
    pub name: Option<String>,
    /// A recency key (e.g. `modified` as nanos); larger = newer.
    pub recency: u128,
    /// The opaque payload returned to the caller (e.g. the session path/index).
    pub item: T,
}

/// Filter + sort rows by `query` (`filterAndSortSessions`, `session-selector-search.ts:154-194`).
/// Applies the [`NameFilter`] first, then — for a non-empty query — matches via [`match_text`] and
/// orders by [`SortMode`]. A query with a parse `error` yields an empty result.
pub fn filter_and_sort<T: Clone>(
    rows: &[SearchRow<T>],
    query: &str,
    sort: SortMode,
    name_filter: NameFilter,
) -> Vec<T> {
    let name_filtered: Vec<&SearchRow<T>> = rows
        .iter()
        .filter(|r| match name_filter {
            NameFilter::All => true,
            NameFilter::Named => has_session_name(r.name.as_deref()),
        })
        .collect();

    if query.trim().is_empty() {
        return name_filtered.into_iter().map(|r| r.item.clone()).collect();
    }

    let parsed = parse_search_query(query);
    if parsed.error.is_some() {
        return Vec::new();
    }

    match sort {
        SortMode::Recent | SortMode::Threaded => name_filtered
            .into_iter()
            .filter(|r| match_text(&r.text, &parsed).is_some())
            .map(|r| r.item.clone())
            .collect(),
        SortMode::Relevance => {
            let mut scored: Vec<(&SearchRow<T>, f64)> = name_filtered
                .into_iter()
                .filter_map(|r| match_text(&r.text, &parsed).map(|s| (r, s)))
                .collect();
            scored.sort_by(|a, b| {
                a.1.total_cmp(&b.1).then_with(|| b.0.recency.cmp(&a.0.recency))
            });
            scored.into_iter().map(|(r, _)| r.item.clone()).collect()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        let parsed = parse_search_query("   ");
        assert!(parsed.tokens.is_empty());
        assert_eq!(parsed.mode, QueryMode::Tokens);
        assert_eq!(match_text("anything at all", &parsed), Some(0.0));
    }

    #[test]
    fn fuzzy_tokens_split_on_whitespace() {
        let parsed = parse_search_query("foo bar");
        assert_eq!(parsed.tokens.len(), 2);
        assert_eq!(parsed.tokens[0], SearchToken { kind: TokenKind::Fuzzy, value: "foo".into() });
        assert_eq!(parsed.tokens[1], SearchToken { kind: TokenKind::Fuzzy, value: "bar".into() });
    }

    #[test]
    fn quoted_phrase_becomes_a_phrase_token() {
        // `foo "node cve" bar` → fuzzy(foo), phrase(node cve), fuzzy(bar).
        let parsed = parse_search_query("foo \"node cve\" bar");
        assert_eq!(parsed.tokens.len(), 3);
        assert_eq!(parsed.tokens[1], SearchToken { kind: TokenKind::Phrase, value: "node cve".into() });
    }

    #[test]
    fn phrase_matches_normalized_substring_only() {
        let parsed = parse_search_query("\"node cve\"");
        // Collapsed whitespace + case-insensitive substring match.
        assert!(match_text("Fixing a   NODE   CVE today", &parsed).is_some());
        assert!(match_text("node and a cve apart", &parsed).is_none());
    }

    #[test]
    fn all_tokens_must_match() {
        let parsed = parse_search_query("alpha zzzznope");
        assert!(match_text("alpha beta gamma", &parsed).is_none());
        let parsed2 = parse_search_query("alpha gamma");
        assert!(match_text("alpha beta gamma", &parsed2).is_some());
    }

    #[test]
    fn unbalanced_quote_falls_back_to_whitespace_tokens() {
        let parsed = parse_search_query("foo \"bar baz");
        // All fuzzy, quote stripped into the tokens via whitespace split.
        assert!(parsed.tokens.iter().all(|t| t.kind == TokenKind::Fuzzy));
        assert!(parsed.tokens.iter().any(|t| t.value.contains("bar")));
    }

    #[test]
    fn regex_mode_is_recognized_but_unsupported() {
        let parsed = parse_search_query("re:foo.*bar");
        assert_eq!(parsed.mode, QueryMode::Regex);
        assert_eq!(parsed.regex_pattern.as_deref(), Some("foo.*bar"));
        // Dep-gated: recognized, never matches.
        assert_eq!(match_text("foozzzbar", &parsed), None);
    }

    #[test]
    fn empty_regex_is_an_error() {
        let parsed = parse_search_query("re:   ");
        assert_eq!(parsed.error.as_deref(), Some("Empty regex"));
        assert!(match_text("anything", &parsed).is_none());
    }

    #[test]
    fn name_filter_keeps_only_named_sessions() {
        let rows = vec![
            SearchRow { text: "a one".into(), name: Some("Build".into()), recency: 2, item: 1 },
            SearchRow { text: "b two".into(), name: None, recency: 1, item: 2 },
            SearchRow { text: "c three".into(), name: Some("  ".into()), recency: 3, item: 3 },
        ];
        let all = filter_and_sort(&rows, "", SortMode::Recent, NameFilter::All);
        assert_eq!(all, vec![1, 2, 3]);
        let named = filter_and_sort(&rows, "", SortMode::Recent, NameFilter::Named);
        assert_eq!(named, vec![1]); // blank name is not "named"
    }

    #[test]
    fn recent_mode_keeps_incoming_order() {
        let rows = vec![
            SearchRow { text: "alpha task".into(), name: None, recency: 1, item: "a" },
            SearchRow { text: "alpha other".into(), name: None, recency: 2, item: "b" },
        ];
        let out = filter_and_sort(&rows, "alpha", SortMode::Recent, NameFilter::All);
        assert_eq!(out, vec!["a", "b"]);
    }

    #[test]
    fn relevance_mode_sorts_by_score_then_recency() {
        // Both match "cve"; the one where it appears earlier scores lower (better) for phrase, but for
        // fuzzy the earlier/consecutive match wins. Tie-break on recency (newer first).
        let rows = vec![
            SearchRow { text: "zzzz cve".into(), name: None, recency: 10, item: "late" },
            SearchRow { text: "cve now".into(), name: None, recency: 5, item: "early" },
        ];
        let out = filter_and_sort(&rows, "\"cve\"", SortMode::Relevance, NameFilter::All);
        // "early" has the phrase at index 0 (score 0), "late" at a larger index → "early" first.
        assert_eq!(out[0], "early");
    }

    #[test]
    fn parse_error_yields_empty_result() {
        let rows = vec![SearchRow { text: "x".into(), name: None, recency: 1, item: 1 }];
        let out = filter_and_sort(&rows, "re:", SortMode::Recent, NameFilter::All);
        assert!(out.is_empty());
    }
}
