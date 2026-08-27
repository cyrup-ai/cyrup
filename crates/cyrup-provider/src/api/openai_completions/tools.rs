//! Request encoding: tool schemas, `tool_choice` and deferred-tool selection.

use crate::api::compat::ResolvedCompat;
use crate::context::ToolDef;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use cyrup_core::{Content, Message};
use serde_json::{Map, Value, json};

/// `true` if a message carries a tool call (assistant) or is a tool result.
pub(super) fn message_has_tool_use(msg: &Message) -> bool {
    match msg {
        Message::ToolResult { .. } => true,
        Message::Assistant(am) => am.content.iter().any(|c| matches!(c, Content::ToolCall(_))),
        Message::User { .. } => false,
    }
}

/// Map cyrup [`ToolDef`]s to OpenAI `tools` entries — Pi `convertTools`,
/// `openai-completions.ts:1286-1320` @**v0.83.0**.
///
/// `strict` is emitted only when the provider supports it (some reject unknown fields), and its
/// value is `resolveJsonSchemaStrictSampling(tool, …) ?? false` — so a tool that opted into
/// JSON-schema constrained sampling gets `strict: true` and every other tool keeps `false`
/// (PROV-011). A `strict: "require"` tool on a provider without strict mode fails the request with
/// pi's exact message.
/// Every tool name introduced mid-transcript by a `toolResult`'s `addedToolNames` — Pi
/// `getDeferredToolNames`, `openai-completions.ts:91-101` @v0.83.0.
///
/// PROV-025. Insertion-ordered for the same reason [`tools_by_name`] is: upstream's `Set` walks in
/// insertion order and that order reaches the wire. This is a DIFFERENT accessor from
/// [`crate::utils::deferred_tools`]'s placement map — it works off message names only, with no
/// notion of WHERE the tool became available.
pub(super) fn deferred_tool_names(messages: &[cyrup_core::Message]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for message in messages {
        if let cyrup_core::Message::ToolResult { added_tool_names, .. } = message {
            for name in added_tool_names {
                if !names.iter().any(|n| n == name) {
                    names.push(name.clone());
                }
            }
        }
    }
    names
}

/// `names.map(n => toolsByName.get(n)).filter(Boolean)` — Pi `getToolsByName`,
/// `openai-completions.ts:103-110` @v0.83.0. Walks `names` (not `tools`), so the emitted order is
/// the order the tools were introduced, and a name with no matching tool is dropped.
pub(super) fn tools_by_name(tools: &[ToolDef], names: &[String]) -> Vec<ToolDef> {
    names
        .iter()
        .filter_map(|name| tools.iter().find(|t| &t.name == name).cloned())
        .collect()
}

pub(crate) fn convert_tools(
    tools: &[ToolDef],
    compat: &ResolvedCompat,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    tools
        .iter()
        .map(|t| {
            let strict = resolve_json_schema_strict_sampling(t, compat.supports_strict_mode)?;
            let mut function = Map::new();
            function.insert("name".to_string(), json!(t.name));
            function.insert("description".to_string(), json!(t.description));
            // `getJsonSchemaToolParameters(tool, strict)` (`openai-completions.ts:1490` @v0.84.2)
            // — a route told `strict: true` rejects anything outside the strict subset.
            function.insert(
                "parameters".to_string(),
                json_schema_tool_parameters(t, strict == Some(true))?,
            );
            if compat.supports_strict_mode {
                function.insert("strict".to_string(), json!(strict.unwrap_or(false)));
            }
            Ok(json!({ "type": "function", "function": Value::Object(function) }))
        })
        .collect()
}
