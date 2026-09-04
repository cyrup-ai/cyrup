//! Shared result helpers (13d §12) — the envelope every mode returns through.
//!
//! See [`crate::proxy`] for the module overview.

use indexmap::IndexMap;
use serde_json::{Map as JsonMap, Value};

use cyrup_core::{Content, ToolResult};

use crate::config::{McpConfig, McpSettings, ServerEntry};
use crate::proxy::env::format_auth_required_message;
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::tool_metadata::ToolMetadata;

// ==================================================================================================
// 5 · Shared result helpers (13d §12)
// ==================================================================================================

/// `{content: [{type:"text", text}], details}` — the envelope every mode returns.
pub(crate) fn text_result(
    text: impl Into<cyrup_core::SharedStr>,
    details: JsonMap<String, Value>,
) -> ToolResult {
    ToolResult {
        content: vec![Content::Text {
            text: text.into(),
            text_signature: None,
        }],
        details: Some(Value::Object(details)),
        ..Default::default()
    }
}

/// A `details` builder seeded with `{mode}`.
pub(crate) fn details(mode: &str) -> JsonMap<String, Value> {
    let mut map = JsonMap::new();
    map.insert("mode".to_string(), Value::String(mode.to_string()));
    map
}

/// A `details` builder seeded with `{mode, error}`.
pub(crate) fn details_err(mode: &str, code: McpErrorCode) -> JsonMap<String, Value> {
    let mut map = details(mode);
    map.insert(
        "error".to_string(),
        Value::String(code.as_str().to_string()),
    );
    map
}

/// `proxy-modes.ts:61` `ambiguousToolResult(mode, toolName)`.
///
/// The **fail-closed** answer: a bare name matching more than one enabled server is refused rather
/// than guessed. `getSingleToolMatch` returning the `"ambiguous"` sentinel instead of `matches[0]`
/// is what upstream's conformance suite calls "fails closed for duplicate unqualified proxy names",
/// and it is why MCP-163 is this section's only `critical`.
#[must_use]
pub fn ambiguous_tool_result(mode: &str, tool_name: &str) -> ToolResult {
    let message = format!("Tool \"{tool_name}\" matches multiple servers. Specify a server.");
    let mut map = details_err(mode, McpErrorCode::AmbiguousTool);
    map.insert(
        "requestedTool".to_string(),
        Value::String(tool_name.to_string()),
    );
    map.insert("message".to_string(), Value::String(message.clone()));
    text_result(message, map)
}

/// `proxy-modes.ts:69` `disabledResult(mode, serverName)` — shared by every mode.
#[must_use]
pub fn disabled_result(mode: &str, server_name: &str) -> ToolResult {
    let message = format!(
        "Server \"{server_name}\" is disabled. Run /mcp enable {server_name} and /reload to enable it."
    );
    let mut map = details_err(mode, McpErrorCode::ServerDisabled);
    map.insert("server".to_string(), Value::String(server_name.to_string()));
    map.insert("message".to_string(), Value::String(message.clone()));
    text_result(message, map)
}

/// `Server "<s>" not found. Use mcp({}) to see available servers.` — `auth-start`, `auth-complete`,
/// `list`, `instructions` and `connect` all share the text and the `not_found` code.
pub(crate) fn not_found_result(mode: &str, server_name: &str) -> ToolResult {
    let mut map = details_err(mode, McpErrorCode::NotFound);
    map.insert("server".to_string(), Value::String(server_name.to_string()));
    text_result(
        format!("Server \"{server_name}\" not found. Use mcp({{}}) to see available servers."),
        map,
    )
}

/// `proxy-modes.ts:77` `getAuthRequiredMessage(state, serverName, defaultMessage?)`.
///
/// The default names both escape hatches; a configured `settings.authRequiredMessage` still wins,
/// which is why the caller-supplied default in [`crate::proxy::attempt_auto_auth`] step 4 also routes through
/// here rather than being returned directly.
#[must_use]
pub fn get_auth_required_message(settings: &McpSettings, server_name: &str) -> String {
    format_auth_required_message(
        settings,
        server_name,
        &default_auth_required_message(server_name),
    )
}

/// The literal default `getAuthRequiredMessage` is declared with.
pub(crate) fn default_auth_required_message(server_name: &str) -> String {
    format!(
        "Server \"{server_name}\" requires OAuth authentication. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
    )
}

/// `proxy-modes.ts:85` `getAuthFailedMessage(state, serverName, message)`.
///
/// The two arms differ: with a configured template the guidance is *appended* via
/// [`get_auth_required_message`]; without one the default guidance is inlined literally. Both spell
/// the same sentence, but the template arm renders the user's text.
#[must_use]
pub fn get_auth_failed_message(settings: &McpSettings, server_name: &str, message: &str) -> String {
    if settings.auth_required_message().is_some() {
        format!(
            "OAuth authentication failed for \"{server_name}\": {message}. {}",
            get_auth_required_message(settings, server_name)
        )
    } else {
        format!(
            "OAuth authentication failed for \"{server_name}\": {message}. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
        )
    }
}

/// `proxy-modes.ts:39` `getToolMatches(metadata, toolName, exact)`.
///
/// `exact` compares `tool.name` verbatim; the fuzzy form compares with all `-` replaced by `_` on
/// **both** sides.
pub(crate) fn get_tool_matches<'a>(
    metadata: &'a [ToolMetadata],
    tool_name: &str,
    exact: bool,
) -> Vec<&'a ToolMetadata> {
    if exact {
        return metadata
            .iter()
            .filter(|tool| tool.name == tool_name)
            .collect();
    }
    let normalized = tool_name.replace('-', "_");
    metadata
        .iter()
        .filter(|tool| tool.name.replace('-', "_") == normalized)
        .collect()
}

/// `proxy-modes.ts:46` `getEnabledToolMatches(state, toolName, exact)` — flat-mapped over
/// non-disabled servers in `state.toolMetadata` **insertion order**.
pub(crate) fn get_enabled_tool_matches(
    config: &McpConfig,
    metadata: &IndexMap<String, Vec<ToolMetadata>>,
    tool_name: &str,
    exact: bool,
) -> Vec<(String, ToolMetadata)> {
    let mut matches = Vec::new();
    for (server, tools) in metadata {
        if config
            .mcp_servers
            .get(server)
            .is_some_and(ServerEntry::is_disabled)
        {
            continue;
        }
        for tool in get_tool_matches(tools, tool_name, exact) {
            matches.push((server.clone(), tool.clone()));
        }
    }
    matches
}

/// `proxy-modes.ts:55` `getSingleToolMatch(metadata, toolName)`'s three-valued return.
#[derive(Debug, Clone, PartialEq)]
pub enum SingleMatch {
    /// Exactly one match — exact if any exact matches existed, else the single fuzzy one.
    One(ToolMetadata),
    /// **More than one.** The sentinel that fails the call closed rather than routing it to
    /// whichever server happened to be first in the map.
    Ambiguous,
    /// Nothing matched.
    None,
}

/// `proxy-modes.ts:55` `getSingleToolMatch(metadata, toolName)`.
///
/// Exact matches win outright when there are any; only when there are none does the fuzzy set get a
/// look. `>1` in whichever set was consulted is [`SingleMatch::Ambiguous`].
#[must_use]
pub fn get_single_tool_match(metadata: Option<&Vec<ToolMetadata>>, tool_name: &str) -> SingleMatch {
    let Some(metadata) = metadata else {
        return SingleMatch::None;
    };
    let exact = get_tool_matches(metadata, tool_name, true);
    let matches = if exact.is_empty() {
        get_tool_matches(metadata, tool_name, false)
    } else {
        exact
    };
    if matches.len() > 1 {
        return SingleMatch::Ambiguous;
    }
    matches
        .first()
        .map_or(SingleMatch::None, |tool| SingleMatch::One((*tool).clone()))
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
    use crate::proxy::testsupport::{config_with, metadata_with};
    use serde_json::json;

    // ---- MCP-170 · insertion order decides which server is named ----------------------------------

    #[test]
    fn insertion_order_decides_the_disabled_server_named_first() {
        let disabled = ServerEntry {
            disabled: Some(true),
            ..ServerEntry::default()
        };
        let config = config_with(&[("zeta", disabled.clone()), ("alpha", disabled)]);
        let metadata = metadata_with(&[
            ("zeta", vec![ToolMetadata::new("t", "t", "")]),
            ("alpha", vec![ToolMetadata::new("t", "t", "")]),
        ]);
        // Both are disabled, so `getEnabledToolMatches` is empty and the fallback scan names the
        // FIRST disabled hit in insertion order.
        assert!(get_enabled_tool_matches(&config, &metadata, "t", true).is_empty());
        let first_disabled = metadata
            .keys()
            .find(|server| {
                config
                    .mcp_servers
                    .get(*server)
                    .is_some_and(ServerEntry::is_disabled)
            })
            .cloned();
        assert_eq!(first_disabled, Some("zeta".to_string()));
    }

    // ---- MCP-163 · the ambiguity gate fails closed -------------------------------------------------

    #[test]
    fn get_single_tool_match_fails_closed_for_duplicates() {
        let duplicates = vec![
            ToolMetadata::new("create_issue", "create_issue", "a"),
            ToolMetadata::new("create_issue", "create_issue", "b"),
        ];
        assert_eq!(
            get_single_tool_match(Some(&duplicates), "create_issue"),
            SingleMatch::Ambiguous
        );

        // A single exact match beats an earlier normalized fallback.
        let mixed = vec![
            ToolMetadata::new("create-issue", "create-issue", "fuzzy"),
            ToolMetadata::new("create_issue", "create_issue", "exact"),
        ];
        match get_single_tool_match(Some(&mixed), "create_issue") {
            SingleMatch::One(found) => assert_eq!(found.description, "exact"),
            other => panic!("expected the exact match, got {other:?}"),
        }

        // Two tools that collide ONLY after `-`→`_` normalization also fail closed — upstream's
        // "fails closed for same-server normalized fallback collisions". The query must have no
        // exact match for the fuzzy set to be consulted at all; when it does have one, the exact
        // match wins outright and there is nothing ambiguous about it.
        let normalized = vec![
            ToolMetadata::new("cre-ate_issue", "cre-ate_issue", "a"),
            ToolMetadata::new("cre_ate_issue", "cre_ate_issue", "b"),
        ];
        assert_eq!(
            get_single_tool_match(Some(&normalized), "cre-ate-issue"),
            SingleMatch::Ambiguous
        );
        // …and an exact hit against one of the two is NOT ambiguous.
        match get_single_tool_match(Some(&normalized), "cre_ate_issue") {
            SingleMatch::One(found) => assert_eq!(found.description, "b"),
            other => panic!("an exact match wins outright, got {other:?}"),
        }
        assert_eq!(get_single_tool_match(None, "anything"), SingleMatch::None);
    }

    #[test]
    fn ambiguous_and_disabled_results_carry_their_codes() {
        let ambiguous = ambiguous_tool_result("call", "create_issue");
        let details = ambiguous.details.expect("details");
        assert_eq!(details["error"], json!("ambiguous_tool"));
        assert_eq!(details["mode"], json!("call"));
        assert_eq!(
            details["message"],
            json!("Tool \"create_issue\" matches multiple servers. Specify a server.")
        );

        let disabled = disabled_result("list", "gh");
        let details = disabled.details.expect("details");
        assert_eq!(details["error"], json!("server_disabled"));
        assert_eq!(
            details["message"],
            json!("Server \"gh\" is disabled. Run /mcp enable gh and /reload to enable it.")
        );
    }
}
