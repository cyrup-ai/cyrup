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
