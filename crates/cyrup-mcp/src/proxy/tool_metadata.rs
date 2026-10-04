//! `ToolMetadata`, the two readers that are shaped around it, and the proxy's view of the
//! tool-name grammar.
//!
//! **The move this module's header used to predict has happened.** The naming grammar it carried
//! was a second implementation of what [`crate::registration`] already owned, and the two had
//! drifted; it is now a `pub use` block (MCP-073/MCP-075/MCP-076, see the note below it), so
//! `crate::proxy::format_tool_name` and its siblings resolve to the one definition in
//! `registration.rs`. What remains here is genuinely 13e's and genuinely local: the
//! [`ToolMetadata`] shape and the two functions that read it.
//!
//! **The instruction for what is left still stands.** [`ToolMetadata`],
//! [`is_ui_tool_visible_to_model`] and [`find_tool_by_name`] are `types.ts`' and are owned by
//! section 13e (MCP-200…MCP-207), which lands them in [`crate::renderers`]. They are ported here
//! byte-faithfully so 13d compiles and is testable standalone; when 13e lands, delete them and
//! replace them with `pub use crate::renderers::{…};` alongside the block below. The shapes are
//! upstream's, so that swap is a delete, not a rewrite — the same move the naming grammar has
//! already made.
//!
//! See [`crate::proxy`] for the module overview.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ==================================================================================================
// 2 · `ToolMetadata` and the tool-name grammar
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
    /// `types.ts:756` — the behaviour hints the server declared on this tool (MCP-601).
    ///
    /// Read **only where a decision is made**: `mcp({ describe })`'s `Hints:` line and the approval
    /// prompt's destructive / read-only sentence. Search results and direct-tool descriptions are
    /// deliberately unchanged upstream, and porting the hints into those would be a divergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpToolAnnotations>,
}

impl ToolMetadata {
    /// A plain tool, for tests and for callers that only need the three required fields.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        original_name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            original_name: original_name.into(),
            description: description.into(),
            resource_uri: None,
            ui_visibility: None,
            input_schema: None,
            annotations: None,
        }
    }
}

/// `types.ts:752` `McpToolAnnotations` — "Hints, not guarantees."
///
/// The four booleans are the spec's `readOnlyHint` / `destructiveHint` / `idempotentHint` /
/// `openWorldHint` and `title` is the server's display name. Every member is optional and
/// **absent is not false**: upstream's renderer prints a hint only for a member that is present,
/// and the approval prompt's warning fires on `=== true`, never on "not false".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl McpToolAnnotations {
    /// `Object.keys(kept).length > 0` — upstream returns `undefined` rather than an empty object,
    /// and the difference is observable: `if (!annotations) return ""` in the describe renderer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// `Object.keys(kept).length > 0 ? kept : undefined`.
    #[must_use]
    pub fn non_empty(self) -> Option<Self> {
        if self.is_empty() { None } else { Some(self) }
    }

    /// The hints a **live** `tools/list` answer carried, out of rmcp's typed
    /// [`rmcp::model::ToolAnnotations`].
    ///
    /// No type filtering is needed on this path and none is done: rmcp has already rejected a
    /// non-boolean hint at the wire. [`extract_tool_annotations`] is the untrusted-input half, for
    /// the cache file.
    #[must_use]
    pub fn from_wire(annotations: &rmcp::model::ToolAnnotations) -> Option<Self> {
        Self {
            title: annotations.title.clone(),
            read_only_hint: annotations.read_only_hint,
            destructive_hint: annotations.destructive_hint,
            idempotent_hint: annotations.idempotent_hint,
            open_world_hint: annotations.open_world_hint,
        }
        .non_empty()
    }
}

/// `utils.ts:500` `extractToolAnnotations(annotations: unknown)` (MCP-601).
///
/// Upstream's doc comment gives the reason this is type-filtered rather than deserialised:
/// "Server and cache input is untrusted, so a malformed field is dropped instead of failing the
/// tool list." `mcp-cache.json` is a cross-product file a co-installed `pi-mcp-adapter` writes
/// too, so `{"readOnlyHint": "yes"}` must cost that one hint — not the entry, and not the file.
#[must_use]
pub fn extract_tool_annotations(annotations: Option<&Value>) -> Option<McpToolAnnotations> {
    // `!annotations || typeof annotations !== "object"` — a JSON array is `typeof "object"` in JS,
    // but every member read below is a key lookup, which yields `undefined` on an array, so an
    // array lands on the same empty result as `{}`.
    let source = annotations?.as_object()?;
    let boolean = |key: &str| source.get(key).and_then(Value::as_bool);
    McpToolAnnotations {
        title: source
            .get("title")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        read_only_hint: boolean("readOnlyHint"),
        destructive_hint: boolean("destructiveHint"),
        idempotent_hint: boolean("idempotentHint"),
        open_world_hint: boolean("openWorldHint"),
    }
    .non_empty()
}

/// `proxy-modes.ts:709` `formatToolHints(toolMeta)` (MCP-601) — the `describe` `Hints:` line.
///
/// Each of the four booleans renders its positive or its negative word when **present**, and a
/// `title` renders only when it differs from both names the tool is known by. The title is
/// `JSON.stringify`-escaped, which is why it goes through [`serde_json::to_string`] rather than
/// `format!("{title:?}")`: Rust's `Debug` for `str` is not JSON (`\u{7f}` against `\u007f`).
#[must_use]
pub fn format_tool_hints(tool: &ToolMetadata) -> String {
    let Some(annotations) = tool.annotations.as_ref() else {
        return String::new();
    };
    let mut hints: Vec<String> = Vec::new();
    let mut flag = |value: Option<bool>, yes: &str, no: &str| {
        if let Some(value) = value {
            hints.push(if value { yes } else { no }.to_string());
        }
    };
    flag(annotations.read_only_hint, "read-only", "not read-only");
    flag(
        annotations.destructive_hint,
        "destructive",
        "non-destructive",
    );
    flag(annotations.idempotent_hint, "idempotent", "not idempotent");
    flag(annotations.open_world_hint, "open-world", "closed-world");
    if let Some(title) = annotations.title.as_deref().map(str::trim).filter(|title| {
        !title.is_empty() && *title != tool.original_name.as_str() && *title != tool.name.as_str()
    }) {
        let quoted = serde_json::to_string(title).unwrap_or_else(|_| format!("\"{title}\""));
        hints.push(format!("title {quoted}"));
    }
    hints.join(", ")
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

// **De-duplicated (MCP-073/MCP-075/MCP-076).** The tool-name grammar had two implementations in
// this crate and they had drifted: the copy that stood here re-escaped an already-escaped prefix in
// `formatLegacyToolName` (`mcp__github` → `mcp_5f__5f_github`), so every legacy `excludeTools` /
// `approveTools` / `searchKeywords` selector under `ToolPrefix::Mcp` — and every selector at all for
// a server whose name carries a character outside `[A-Za-z0-9_-]` — silently failed to match, and
// its `globToRegExp` compiled config-supplied patterns without a size ceiling.
// [`crate::registration`] is the surviving grammar; it is `types.ts` verbatim, its
// `ToolSelectorCandidateIndex` port memoises what the copy recomputed per (tool, pattern) pair, and
// it carries the inverse function `resolveServerFromToolName` the copy never had.
//
// Two earlier de-duplications are folded into the same list: `resourceNameToToolName` (MCP-203),
// whose copy yielded `"resource_"` where upstream yields `"resource"` for an all-punctuation name,
// and `truncateAtWord` (MCP-206), whose copy counted Unicode scalar values where JS `.length`
// counts UTF-16 code units.
pub use crate::registration::{
    CandidateIndex, format_tool_name, is_tool_allowed, matches_tool_pattern,
    resolve_server_from_tool_name, resolve_tool_prefix, resource_name_to_tool_name,
    sanitize_server_prefix, server_prefix, tool_name_candidates, truncate_at_word,
};

/// `tool-metadata.ts:154` `findToolByName(metadata, toolName)` — exact `name` match first,
/// otherwise compare with `-` globally replaced by `_` on **both** sides.
#[must_use]
pub fn find_tool_by_name<'a>(
    metadata: &'a [ToolMetadata],
    tool_name: &str,
) -> Option<&'a ToolMetadata> {
    if let Some(exact) = metadata.iter().find(|tool| tool.name == tool_name) {
        return Some(exact);
    }
    let normalized = tool_name.replace('-', "_");
    metadata
        .iter()
        .find(|tool| tool.name.replace('-', "_") == normalized)
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
    use serde_json::json;

    // ---- MCP-601 · `extractToolAnnotations` ---------------------------------------------------

    /// `utils.ts:500` — "Server and cache input is untrusted, so a malformed field is dropped
    /// instead of failing the tool list." One malformed member must cost that member only.
    #[test]
    fn a_malformed_annotation_member_is_dropped_and_the_rest_kept() {
        let kept = extract_tool_annotations(Some(&json!({
            "title": "List simulators",
            "readOnlyHint": true,
            "destructiveHint": "yes",
            "idempotentHint": 1,
            "openWorldHint": false,
            "unknownHint": true,
        })))
        .expect("the well-typed members survive");
        assert_eq!(kept.title.as_deref(), Some("List simulators"));
        assert_eq!(kept.read_only_hint, Some(true));
        assert_eq!(
            kept.destructive_hint, None,
            "a string is not a boolean hint"
        );
        assert_eq!(kept.idempotent_hint, None, "a number is not a boolean hint");
        assert_eq!(kept.open_world_hint, Some(false));
    }

    /// `Object.keys(kept).length > 0 ? kept : undefined` — the difference is observable, because
    /// the describe renderer's first line is `if (!annotations) return ""`.
    #[test]
    fn nothing_survivable_yields_none_rather_than_an_empty_object() {
        for input in [
            json!({}),
            json!({ "title": 7, "readOnlyHint": "true" }),
            json!("destructive"),
            json!(null),
        ] {
            assert_eq!(
                extract_tool_annotations(Some(&input)),
                None,
                "{input} must yield `undefined`, not an empty object"
            );
        }
        assert_eq!(extract_tool_annotations(None), None);
    }

    // ---- MCP-601 · `formatToolHints` ----------------------------------------------------------

    /// `proxy-modes.ts:709-723`. Each boolean renders its positive or its negative word when
    /// **present** — absent is not false — and the order is read-only, destructive, idempotent,
    /// open-world, title.
    #[test]
    fn the_describe_hints_render_present_members_in_upstream_order() {
        let mut tool = ToolMetadata::new("srv_wipe", "wipe", "");
        assert_eq!(
            format_tool_hints(&tool),
            "",
            "no annotations means no hints line at all"
        );

        tool.annotations = Some(McpToolAnnotations {
            read_only_hint: Some(false),
            destructive_hint: Some(true),
            open_world_hint: Some(false),
            ..McpToolAnnotations::default()
        });
        assert_eq!(
            format_tool_hints(&tool),
            "not read-only, destructive, closed-world",
            "an absent `idempotentHint` contributes nothing"
        );

        // A title renders only when it differs from both names, and is JSON-escaped.
        tool.annotations = Some(McpToolAnnotations {
            title: Some("wipe".to_string()),
            ..McpToolAnnotations::default()
        });
        assert_eq!(format_tool_hints(&tool), "");
        tool.annotations = Some(McpToolAnnotations {
            title: Some("  Wipe \"everything\"  ".to_string()),
            ..McpToolAnnotations::default()
        });
        assert_eq!(
            format_tool_hints(&tool),
            "title \"Wipe \\\"everything\\\"\"",
            "the server's title is escaped before it reaches the model"
        );
    }

    /// The live half: rmcp's typed hints, with upstream's "empty means absent" rule.
    #[test]
    fn wire_annotations_keep_their_values_and_collapse_when_empty() {
        let wire = rmcp::model::ToolAnnotations::new();
        assert_eq!(
            McpToolAnnotations::from_wire(&wire),
            None,
            "an all-absent `annotations` object is `undefined`"
        );
        let wire = wire.destructive(true);
        assert_eq!(
            McpToolAnnotations::from_wire(&wire),
            Some(McpToolAnnotations {
                destructive_hint: Some(true),
                ..McpToolAnnotations::default()
            })
        );
    }
}
