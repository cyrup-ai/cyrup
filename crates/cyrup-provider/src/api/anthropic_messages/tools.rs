//! Request encoding — the `tools` array (Pi `convertTools`).

use super::claude_code::to_claude_code_name;
use crate::context::ToolDef;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use serde_json::{Map, Value, json};

/// Map cyrup [`ToolDef`]s to Anthropic `tools` (Pi `convertTools`, anthropic-messages.ts:1188-1211).
/// `cache_control` is applied to the last tool only; `eager_input_streaming` when supported.
///
/// `defer_loading` marks a tool as transcript-anchored (DRIFT-001): it still ships in
/// `params.tools`, but the model only "sees" it at the `tool_reference` that names it. It is
/// inserted where Pi spreads it — after `input_schema`, before `cache_control` (Pi :1315-1321) —
/// though the workspace's `serde_json` has no `preserve_order` feature, so the serialized key
/// order is alphabetical either way and only the key SET is observable on the wire.
pub(crate) fn convert_tools(
    tools: &[ToolDef],
    is_oauth: bool,
    supports_eager: bool,
    supports_strict_tools: bool,
    cache_control: Option<&Value>,
    defer_loading: bool,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let last = tools.len().saturating_sub(1);
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            // PROV-011 — `anthropic-messages.ts:1337` @v0.84.2.
            let strict = resolve_json_schema_strict_sampling(tool, supports_strict_tools)?;
            // `const parameters = getJsonSchemaToolParameters(tool, strict)` (`:1338` @v0.84.2):
            // BOTH the legacy three-key subset and the strict spread are taken from the CONVERTED
            // schema (`:1339-1344`), not from the tool's raw parameters.
            let parameters = json_schema_tool_parameters(tool, strict == Some(true))?;
            let name = if is_oauth {
                to_claude_code_name(&tool.name)
            } else {
                tool.name.clone()
            };
            let properties = parameters
                .get("properties")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let required = parameters
                .get("required")
                .cloned()
                .unwrap_or_else(|| json!([]));
            // `legacyInputSchema` (`:1340-1344` @v0.84.2) — the three-key subset Anthropic has
            // always accepted. Under strict sampling pi sends the WHOLE (converted) schema with
            // that subset spread over it (`:1345-1351`), so `type`/`properties`/`required` still
            // win and any extra keyword (`additionalProperties`, …) survives for the constrainer.
            //
            // Built as a `Map` rather than via `json!` so the strict arm can spread it without
            // an `as_object().expect(..)` round-trip — the workspace denies `expect_used`, and
            // an infallible construction is stronger than a justified panic either way.
            let mut legacy = Map::new();
            legacy.insert("type".to_string(), json!("object"));
            legacy.insert("properties".to_string(), properties);
            legacy.insert("required".to_string(), required);
            let input_schema = if strict == Some(true) {
                let mut merged = parameters.as_object().cloned().unwrap_or_else(Map::new);
                for (k, v) in &legacy {
                    merged.insert(k.clone(), v.clone());
                }
                Value::Object(merged)
            } else {
                Value::Object(legacy)
            };
            let mut o = Map::new();
            o.insert("name".to_string(), json!(name));
            o.insert("description".to_string(), json!(tool.description));
            if supports_eager {
                o.insert("eager_input_streaming".to_string(), json!(true));
            }
            // `...(strict === true ? { strict: true } : {})` (`:1357`) — inserted where pi spreads
            // it, between `eager_input_streaming` and `input_schema`. As with `defer_loading`
            // above, that insertion order is for readability against pi only: `serde_json`'s `Map`
            // is a `BTreeMap` here, so the wire order is lexicographic regardless.
            if strict == Some(true) {
                o.insert("strict".to_string(), json!(true));
            }
            o.insert("input_schema".to_string(), input_schema);
            if defer_loading {
                o.insert("defer_loading".to_string(), json!(true));
            }
            if let Some(cc) = cache_control
                && index == last
            {
                o.insert("cache_control".to_string(), cc.clone());
            }
            Ok(Value::Object(o))
        })
        .collect()
}
