//! Which tool names a session's `--tools` and `--exclude-tools` allow, and how MCP tools are
//! treated by them (`createToolNameMatcher`, `isMcpToolName`, `core/mcp-servers.ts`, and
//! `AgentSession._isAllowedTool` / `_allowlistFiltersMcp`, `core/agent-session.ts` @v1.0.4).
//!
//! Everything here is a pure function of the flag entries and a name, so the registry, the
//! extension host and the launch code decide with one definition.

use std::collections::HashSet;

/// The three resource tools, which reach every MCP server that has resources
/// (`LIST_MCP_RESOURCES_TOOL`, `LIST_MCP_RESOURCE_TEMPLATES_TOOL`, `READ_MCP_RESOURCE_TOOL`).
const MCP_RESOURCE_TOOLS: [&str; 3] = [
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
];

/// `isMcpToolName` (`core/mcp-servers.ts` @v1.0.4): a server tool (`mcp__<server>__<tool>`) or a
/// resource tool.
#[must_use]
pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__") || MCP_RESOURCE_TOOLS.contains(&name)
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters and everything else
/// matches itself (`toolPatternRegExp`, `core/mcp-servers.ts` @v1.0.4: the pattern anchored at both
/// ends, `*` as `.*`).
///
/// [CYRUP-DELTA] `*` here also matches a line terminator, where the upstream `.` does not. A tool
/// name cannot contain one, so no name matches differently.
fn pattern_matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut parts = parts.peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            // The last piece is anchored to the end of the name, after whatever the star took.
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = rest.get(at + part.len()..).unwrap_or_default(),
            None => return false,
        }
    }
    // No `*` at all: the whole name had to be the first piece.
    rest.is_empty()
}

/// Whether `name` is one of `entries`, each an exact name or, when it contains `*`, a pattern
/// (`createToolNameMatcher(entries)(name)`, `core/mcp-servers.ts` @v1.0.4).
#[must_use]
pub fn tool_name_matches<'a>(entries: impl IntoIterator<Item = &'a String>, name: &str) -> bool {
    entries.into_iter().any(|entry| {
        if entry.contains('*') {
            pattern_matches(entry, name)
        } else {
            entry == name
        }
    })
}

/// `_allowlistFiltersMcp` (`AgentSession` constructor, `core/agent-session.ts` @v1.0.4): whether an
/// allowlist also decides which MCP tools stay registered. An empty one does (`--no-tools`), and so
/// does one with an entry for an MCP tool; any other allowlist leaves MCP tools registered, for
/// `codemode` and `tool_search` to reach.
#[must_use]
pub fn allowlist_filters_mcp(allowed: &HashSet<String>) -> bool {
    allowed.is_empty() || allowed.iter().any(|entry| entry.starts_with("mcp__"))
}

/// `_isAllowedTool` (`core/agent-session.ts` @v1.0.4): whether the session's flags keep a tool
/// registered. It is outside the denylist, and inside the allowlist when there is one; an MCP tool
/// the allowlist does not name stays unless [`allowlist_filters_mcp`].
///
/// `is_mcp` is whether the tool is an MCP tool: its name says so ([`is_mcp_tool_name`]), or the
/// tool does ([`crate::Tool::is_mcp_tool`]).
#[must_use]
pub fn is_allowed_tool(
    allowed: Option<&HashSet<String>>,
    excluded: &HashSet<String>,
    name: &str,
    is_mcp: bool,
) -> bool {
    if tool_name_matches(excluded, name) {
        return false;
    }
    let Some(allowed) = allowed else {
        return true;
    };
    tool_name_matches(allowed, name) || (!allowlist_filters_mcp(allowed) && is_mcp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(entries: &[&str]) -> HashSet<String> {
        entries.iter().map(|entry| (*entry).to_string()).collect()
    }

    #[test]
    fn a_star_matches_any_run_and_the_rest_matches_itself() {
        let matches = |pattern: &str, name: &str| pattern_matches(pattern, name);
        assert!(matches("mcp__*", "mcp__docs__find"));
        assert!(matches("mcp__*", "mcp__"));
        assert!(!matches("mcp__*", "xmcp__docs"));
        assert!(matches("*__find", "mcp__docs__find"));
        assert!(!matches("*__find", "mcp__docs__finder"));
        assert!(matches("mcp__*__find", "mcp__docs__find"));
        assert!(matches("mcp__*__find", "mcp____find"));
        assert!(!matches("mcp__*__find", "mcp__find"));
        assert!(matches("a*b*c", "aXbYc"));
        assert!(matches("a*b*c", "abc"));
        assert!(!matches("a*b*c", "acb"));
        assert!(matches("*", ""));
        assert!(matches("**", "anything"));
        // Regex metacharacters are literal.
        assert!(matches("a.b*", "a.bc"));
        assert!(!matches("a.b*", "axbc"));
        assert!(matches("f(x)*", "f(x)y"));
    }

    #[test]
    fn an_entry_without_a_star_is_an_exact_name() {
        let entries = set(&["read", "mcp__*"]);
        assert!(tool_name_matches(&entries, "read"));
        assert!(!tool_name_matches(&entries, "reader"));
        assert!(tool_name_matches(&entries, "mcp__a__b"));
        assert!(!tool_name_matches(&set(&[]), "read"));
    }

    #[test]
    fn mcp_names_are_server_tools_and_the_three_resource_tools() {
        assert!(is_mcp_tool_name("mcp__docs__find"));
        assert!(is_mcp_tool_name("read_mcp_resource"));
        assert!(is_mcp_tool_name("list_mcp_resources"));
        assert!(is_mcp_tool_name("list_mcp_resource_templates"));
        assert!(!is_mcp_tool_name("mcp"));
        assert!(!is_mcp_tool_name("docs_find"));
    }

    #[test]
    fn only_an_empty_allowlist_or_an_mcp_entry_filters_mcp_tools() {
        assert!(allowlist_filters_mcp(&set(&[])));
        assert!(allowlist_filters_mcp(&set(&["read", "mcp__docs__find"])));
        assert!(allowlist_filters_mcp(&set(&["mcp__*"])));
        assert!(!allowlist_filters_mcp(&set(&["read", "codemode"])));
    }

    #[test]
    fn the_flags_decide_which_tools_stay_registered() {
        let none = set(&[]);
        // No lists: everything.
        assert!(is_allowed_tool(None, &none, "x", false));
        // An allowlist keeps what it names, drops the rest, and keeps MCP tools it does not name.
        let allow = set(&["read", "co*"]);
        assert!(is_allowed_tool(Some(&allow), &none, "read", false));
        assert!(is_allowed_tool(Some(&allow), &none, "codemode", false));
        assert!(!is_allowed_tool(Some(&allow), &none, "bash", false));
        assert!(is_allowed_tool(Some(&allow), &none, "mcp__a__b", true));
        assert!(is_allowed_tool(Some(&allow), &none, "docs_find", true));
        // An MCP entry makes the allowlist decide for MCP tools too.
        let allow = set(&["read", "mcp__a__*"]);
        assert!(is_allowed_tool(Some(&allow), &none, "mcp__a__b", true));
        assert!(!is_allowed_tool(Some(&allow), &none, "mcp__c__d", true));
        // `--no-tools`: an empty allowlist drops MCP tools as well.
        let empty = set(&[]);
        assert!(!is_allowed_tool(Some(&empty), &none, "mcp__a__b", true));
        // The denylist wins, patterns included, MCP tools included.
        let deny = set(&["mcp__a__*", "bash"]);
        assert!(!is_allowed_tool(None, &deny, "mcp__a__b", true));
        assert!(!is_allowed_tool(
            Some(&set(&["read"])),
            &deny,
            "mcp__a__b",
            true
        ));
        assert!(!is_allowed_tool(None, &deny, "bash", false));
        assert!(is_allowed_tool(None, &deny, "mcp__c__d", true));
    }
}
