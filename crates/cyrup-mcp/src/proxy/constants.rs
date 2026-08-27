//! The module's literal constants — timeouts, length budgets, the approval
//! option strings, the ranking weights and the regex size limits.
//!
//! See [`crate::proxy`] for the module overview.





// ==================================================================================================
// 0 · Constants
// ==================================================================================================

/// `index.ts:39` `INIT_WAIT_TIMEOUT_MS = 30_000` — the bounded wait the dispatch preamble races the
/// live init future against.
pub const INIT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// `proxy-modes.ts:27` `MAX_REGEX_SEARCH_QUERY_LENGTH = 256`.
///
/// JS measures `query.length` in UTF-16 code units. This port measures `chars().count()` (Unicode
/// scalar values), which diverges only for astral-plane text in a regex query — a query that would
/// be rejected at 128 astral characters upstream and 256 here. Stated rather than silently changed.
pub const MAX_REGEX_SEARCH_QUERY_LENGTH: usize = 256;

/// `proxy-modes.ts:28` `INSTRUCTIONS_PREVIEW_LENGTH = 300` — [`crate::proxy::execute_list`]'s inline preview.
pub const INSTRUCTIONS_PREVIEW_LENGTH: usize = 300;

/// `direct-tools.ts:27` `INSTRUCTIONS_SNIPPET_LENGTH = 150` — [`crate::proxy::build_proxy_description`]'s snippet.
pub const INSTRUCTIONS_SNIPPET_LENGTH: usize = 150;

/// `tool-approval.ts:176 @v2.26.1` — the approval dialog's argument preview budget, in UTF-16 code units.
///
/// Not a named constant upstream (the literal `500` is inline); named here because
/// `approval_argument_preview` and its conformance test both have to agree on it.
pub const APPROVAL_PREVIEW_LENGTH: usize = 500;

/// `tool-approval.ts:182 @v2.26.1` — the approval dialog's first option. **A wire contract**: the answer is
/// compared against this exact string, so a reworded label silently becomes a denial.
pub const APPROVE_ONCE_OPTION: &str = "Allow once";

/// `tool-approval.ts:182 @v2.26.1` — the second option, the one that writes the session cache.
pub const APPROVE_FOR_SESSION_OPTION: &str = "Allow for session";

/// `tool-approval.ts:182 @v2.26.1` — the third option. Never compared against: it is the `_` arm's label,
/// and every answer that is not one of the two `Allow …` strings denies. Named so the option list
/// reads as three deliberate strings rather than two and a loose literal.
pub const DENY_OPTION: &str = "Deny";

/// `tool-approval.ts:182 @v2.26.1` `["Allow once", "Allow for session", "Deny"]`, in upstream's order — the
/// order the human sees, and therefore the one a muscle-memory `Enter` picks.
pub const APPROVAL_OPTIONS: [&str; 3] =
    [APPROVE_ONCE_OPTION, APPROVE_FOR_SESSION_OPTION, DENY_OPTION];

/// `search-ranking.ts:9` — shortest field token allowed to stem-match a longer query token.
///
/// Real descriptions tokenize possessives into single letters (`"project's"` → `["project", "s"]`),
/// which would otherwise make every query starting with that letter a match.
pub const MIN_STEM_LENGTH: usize = 4;

/// `search-ranking.ts:11` `FIELD_WEIGHTS.name`.
pub(crate) const WEIGHT_NAME: i64 = 12;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.originalName`.
pub(crate) const WEIGHT_ORIGINAL_NAME: i64 = 10;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.server`.
pub(crate) const WEIGHT_SERVER: i64 = 8;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.description`.
pub(crate) const WEIGHT_DESCRIPTION: i64 = 5;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.keywords`.
pub(crate) const WEIGHT_KEYWORDS: i64 = 5;

/// The regex engine's compiled-program ceiling, set **explicitly** rather than left to the crate
/// default: a supplied pattern must not be able to trade a short string for an unbounded DFA.
///
/// Two consumers, both untrusted-input sites. **MCP-159**, [`crate::proxy::execute_search`]'s
/// model-supplied `regex` query, where a size-limit failure surfaces as
/// [`crate::proxy::McpErrorCode::InvalidPattern`] exactly as a syntax error does. **MCP-076**,
/// [`crate::registration`]'s glob compiler, which compiles the config-supplied `includeTools` /
/// `excludeTools` / `approveTools` / `searchKeywords` patterns and where a failure means the
/// pattern matches nothing — the same outcome a pattern the parser rejects already had.
pub(crate) const REGEX_SIZE_LIMIT: usize = 1 << 20;
/// Companion ceiling for the lazy DFA cache; see [`REGEX_SIZE_LIMIT`].
pub(crate) const REGEX_DFA_SIZE_LIMIT: usize = 1 << 20;

/// The tool's registered name. **Fixed by cross-crate contract** (13d §13.1):
/// `cyrup_permission_system::manager` branches on `normalized == "mcp"` and its `BUILT_IN_TOOL_NAMES`
/// does not contain it, so it must arrive from the extension registry under exactly this name;
/// `gate.rs` addresses it by name in the no-UI denial text.
pub const MCP_TOOL_NAME: &str = "mcp";

/// `index.ts:824` `label: "MCP"`.
pub const MCP_TOOL_LABEL: &str = "MCP";

/// `index.ts:825` `promptSnippet`.
pub const MCP_TOOL_PROMPT_SNIPPET: &str =
    "MCP gateway — status, search, describe, auth, and single MCP tool calls";

/// The guideline bullet `cyrup_permission_system::sanitize::tools` keys its MCP rule on.
///
/// 13d §13.3: `normalize_guideline_text` is bullet-strip + whitespace-collapse + lowercase, so this
/// source string need not itself be lowercase — but it must normalise to
/// `use mcp for mcp discovery first: search by capability, describe one exact tool name, then call
/// it.` **The failure mode is inverted from the obvious guess**: `should_keep_guideline` is
/// `guideline_keep_rule(...).unwrap_or(true)`, so a bullet matching no rule is *always kept*. A
/// mismatched string therefore does not delete guidance — it silently disables the gating and leaves
/// "use mcp …" in the system prompt after the `mcp` tool has been taken away (MCP-192).
pub const MCP_TOOL_GUIDELINE: &str =
    "Use mcp for MCP discovery first: search by capability, describe one exact tool name, then call it.";

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // ---- MCP-192 · the permission system's guideline key ---------------------------------------------

    #[test]
    fn guideline_normalises_to_the_sanitizer_key() {
        // The sanitizer's normalisation is bullet-strip + whitespace-collapse + lowercase, so the
        // source string need not itself be lowercase.
        let normalised = MCP_TOOL_GUIDELINE
            .trim_start_matches(['-', '*', '•', ' '])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert_eq!(
            normalised,
            "use mcp for mcp discovery first: search by capability, describe one exact tool name, then call it."
        );
        assert_eq!(MCP_TOOL_NAME, "mcp");
    }

}
