//! Request encoding — `toolChoice`, the `functionDeclarations` shape and the tool-result text
//! flattener (Pi `mapToolChoice` / `toFunctionTools`, mistral-conversations.ts:600-647).

use crate::context::ToolDef;
use crate::stream::ToolChoice;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use serde_json::{Value, json};

/// Map a tool-choice to Mistral's `toolChoice` (Pi `mapToolChoice`,
/// mistral-conversations.ts:636-647). cyrup's [`ToolChoice`] maps onto `"auto"`/`"none"`/
/// `"required"` and the `{type:"function",function:{name}}` object form.
pub(super) fn map_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Function { name } => {
            json!({ "type": "function", "function": { "name": name } })
        }
    }
}

/// Convert tools to Mistral `FunctionTool`s (Pi `toFunctionTools`,
/// `mistral-conversations.ts:753-766` @**v0.84.2**).
///
/// PROV-011 — `strict` is `resolveJsonSchemaStrictSampling(tool, true) ?? false` (`:755`) and the
/// schema is `getJsonSchemaToolParameters(tool, strict)` (`:761`). Mistral is the one route that
/// passes `true` unconditionally: every Mistral model supports strict schemas, so the
/// "strict tools are unsupported" arm is unreachable here. The `Result` is still load-bearing —
/// a `strict: "require"` tool whose schema cannot be converted to the strict subset fails the
/// request with the conversion's own reason.
pub(super) fn to_function_tools(
    tools: &[ToolDef],
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    tools
        .iter()
        .map(|t| {
            let strict = resolve_json_schema_strict_sampling(t, true)?;
            Ok(json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": json_schema_tool_parameters(t, strict == Some(true))?,
                    "strict": strict.unwrap_or(false),
                },
            }))
        })
        .collect()
}

/// The text for a tool-result message (Pi `buildToolResultText`, mistral-conversations.ts:600-619).
pub(super) fn build_tool_result_text(
    text: &str,
    has_images: bool,
    supports_images: bool,
    is_error: bool,
) -> String {
    let trimmed = text.trim();
    let error_prefix = if is_error { "[tool error] " } else { "" };
    if !trimmed.is_empty() {
        let image_suffix = if has_images && !supports_images {
            "\n[tool image omitted: model does not support images]"
        } else {
            ""
        };
        return format!("{error_prefix}{trimmed}{image_suffix}");
    }
    if has_images {
        if supports_images {
            return if is_error {
                "[tool error] (see attached image)".to_string()
            } else {
                "(see attached image)".to_string()
            };
        }
        return if is_error {
            "[tool error] (image omitted: model does not support images)".to_string()
        } else {
            "(image omitted: model does not support images)".to_string()
        };
    }
    if is_error {
        "[tool error] (no tool output)".to_string()
    } else {
        "(no tool output)".to_string()
    }
}
