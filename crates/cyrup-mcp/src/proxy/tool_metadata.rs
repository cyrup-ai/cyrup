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
    /// `tool.annotations`, validated by [`extract_tool_annotations`] — the server's behaviour
    /// hints (MCP-601). `None` when the server declared none or none survived validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpToolAnnotations>,
}

/// `types.ts:752` `McpToolAnnotations` — "Hints, not guarantees."
///
/// The four booleans are the spec's tool annotations and `title` is the server's own display name
/// for the tool. `2bc904f` (#728, `pi-mcp-adapter` v3.3.0) states the gap this closes: "Servers
/// declare read-only, destructive, idempotent, and open-world hints on their tools. The adapter
/// dropped them, so neither the model nor the user could tell a read-only tool from one that
/// deletes data."
///
/// Read in exactly two places, both of them decisions and neither of them a listing:
/// [`crate::proxy::is_tool_call_approval_required`]'s `approveTools: "destructive"` gate and the
/// approval dialog's warning line, plus `mcp({ describe })`'s `Hints:` row. Search results and
/// direct-tool descriptions are deliberately **unchanged** — porting the hints into those would be
/// a divergence, not a bonus.
///
/// Every member is lenient on the way in: see [`extract_tool_annotations`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    /// A human-readable title the server gave the tool.
    #[serde(
        default,
        deserialize_with = "lenient_annotation_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<String>,
    /// The tool does not modify its environment.
    #[serde(
        default,
        deserialize_with = "lenient_annotation_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub read_only_hint: Option<bool>,
    /// The tool may delete or overwrite data.
    #[serde(
        default,
        deserialize_with = "lenient_annotation_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub destructive_hint: Option<bool>,
    /// Repeating the call with the same arguments has no additional effect.
    #[serde(
        default,
        deserialize_with = "lenient_annotation_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub idempotent_hint: Option<bool>,
    /// The tool reaches an open world of external entities.
    #[serde(
        default,
        deserialize_with = "lenient_annotation_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub open_world_hint: Option<bool>,
}

impl McpToolAnnotations {
    /// `Object.keys(kept).length > 0` — whether anything survived.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.read_only_hint.is_none()
            && self.destructive_hint.is_none()
            && self.idempotent_hint.is_none()
            && self.open_world_hint.is_none()
    }
}

/// `if (typeof source.title === "string") kept.title = source.title;` — a non-string `title` is
/// **dropped**, not an error (`utils.ts:505`).
fn lenient_annotation_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?
        .and_then(|value| value.as_str().map(std::string::ToString::to_string)))
}

/// `if (typeof source[key] === "boolean") kept[key] = source[key];` — a non-boolean hint is
/// **dropped**, not an error (`utils.ts:507`). `"true"` is a string and does not survive.
fn lenient_annotation_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?.and_then(|value| value.as_bool()))
}

/// `extractToolAnnotations(annotations)` (`utils.ts:500`) — keep only the spec annotations with the
/// right types.
///
/// Upstream's doc comment is the specification: "Server and cache input is untrusted, so a
/// malformed field is dropped instead of failing the tool list." That is what the two lenient
/// deserialisers above give, member by member: `{readOnlyHint: "yes", destructiveHint: true}` keeps
/// `destructiveHint` and loses `readOnlyHint`. A non-object, and an object out of which nothing
/// survives, are both `None` — the case upstream's `...(annotations !== undefined ? … : {})` spread
/// omits, so an all-malformed `annotations` is indistinguishable from an absent one.
#[must_use]
pub fn extract_tool_annotations(annotations: Option<&Value>) -> Option<McpToolAnnotations> {
    let value = annotations?;
    if !value.is_object() {
        return None;
    }
    let kept: McpToolAnnotations = serde_json::from_value(value.clone()).unwrap_or_default();
    (!kept.is_empty()).then_some(kept)
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

/// `extractToolAnnotations(tool.annotations)` over a **live** `tools/list` entry.
///
/// rmcp has already decoded `annotations` into a typed `ToolAnnotations` whose members are exactly
/// upstream's five, so there is no untrusted JSON left to validate here — the strictness moved
/// into rmcp's own deserialiser, which is where a malformed member costs the whole `tools/list`
/// rather than just itself. That is rmcp's shape and predates this port; the leniency
/// [`extract_tool_annotations`] implements is still needed for the **cache**, which cyrup both
/// writes and reads and which a co-installed `pi-mcp-adapter` also writes.
///
/// The one rule reproduced here is the last line of upstream's extractor: an `annotations` object
/// out of which nothing survives is `undefined`, not `{}`.
#[must_use]
pub fn tool_annotations(tool: &rmcp::model::Tool) -> Option<McpToolAnnotations> {
    let source = tool.annotations.as_ref()?;
    let kept = McpToolAnnotations {
        title: source.title.clone(),
        read_only_hint: source.read_only_hint,
        destructive_hint: source.destructive_hint,
        idempotent_hint: source.idempotent_hint,
        open_world_hint: source.open_world_hint,
    };
    (!kept.is_empty()).then_some(kept)
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- MCP-601 · `extractToolAnnotations` ------------------------------------------------------

    /// Upstream's doc comment is the assertion: "Server and cache input is untrusted, so a
    /// malformed field is dropped instead of failing the tool list." Member by member — the good
    /// hints survive the bad ones.
    #[test]
    fn a_malformed_annotation_member_is_dropped_and_the_rest_kept() {
        let kept = extract_tool_annotations(Some(&json!({
            "title": 7,
            "readOnlyHint": "yes",
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": null,
            "somethingElse": { "nested": 1 }
        })))
        .expect("something survived");
        assert_eq!(kept.title, None, "a non-string title is dropped");
        assert_eq!(
            kept.read_only_hint, None,
            "`\"yes\"` is a string, not a boolean"
        );
        assert_eq!(kept.destructive_hint, Some(true));
        assert_eq!(
            kept.idempotent_hint,
            Some(false),
            "`false` is a declared hint, not an absent one"
        );
        assert_eq!(kept.open_world_hint, None, "`null` is not a boolean");
    }

    /// `return Object.keys(kept).length > 0 ? kept : undefined` — an object out of which nothing
    /// survives is indistinguishable from an absent one, because that is the case upstream's
    /// `...(annotations !== undefined ? … : {})` spread omits.
    #[test]
    fn an_annotations_object_with_nothing_usable_is_absent() {
        assert_eq!(extract_tool_annotations(None), None);
        assert_eq!(extract_tool_annotations(Some(&json!("destructive"))), None);
        assert_eq!(extract_tool_annotations(Some(&json!([true]))), None);
        assert_eq!(extract_tool_annotations(Some(&json!({}))), None);
        assert_eq!(
            extract_tool_annotations(Some(&json!({ "readOnlyHint": 1, "title": [] }))),
            None,
            "every member malformed is the same as no member"
        );
    }

    /// The hints must survive `mcp-cache.json`, or a cold-start direct-tool surface cannot tell a
    /// read-only tool from a destructive one — which is the whole point of carrying them on
    /// `CachedTool` as well as on `ToolMetadata`.
    #[test]
    fn a_hint_survives_a_json_round_trip() {
        let hints = extract_tool_annotations(Some(&json!({
            "title": "Delete everything",
            "destructiveHint": true,
            "readOnlyHint": false
        })))
        .expect("hints");
        let text = serde_json::to_string(&hints).expect("serializes");
        // camelCase on the wire, and only the declared members are written.
        assert_eq!(
            text,
            r#"{"title":"Delete everything","readOnlyHint":false,"destructiveHint":true}"#
        );
        let back = extract_tool_annotations(Some(
            &serde_json::from_str::<serde_json::Value>(&text).expect("parses"),
        ))
        .expect("hints survive");
        assert_eq!(back, hints);
    }

    /// The live path: rmcp has already typed `annotations`, so the only rule left to reproduce is
    /// the last line of upstream's extractor.
    #[test]
    fn a_live_tool_with_an_all_default_annotations_object_carries_no_hints() {
        let bare = rmcp::model::Tool::new(
            "t",
            "d",
            std::sync::Arc::new(serde_json::Map::new()),
        );
        assert_eq!(tool_annotations(&bare), None, "no `annotations` at all");

        let empty = bare
            .clone()
            .annotate(rmcp::model::ToolAnnotations::default());
        assert_eq!(
            tool_annotations(&empty), None,
            "an `annotations` object declaring nothing is `undefined`, not `{}`"
        );

        let marked = bare.annotate(rmcp::model::ToolAnnotations {
            destructive_hint: Some(true),
            ..rmcp::model::ToolAnnotations::default()
        });
        assert_eq!(
            tool_annotations(&marked).and_then(|hints| hints.destructive_hint),
            Some(true)
        );
    }
}
