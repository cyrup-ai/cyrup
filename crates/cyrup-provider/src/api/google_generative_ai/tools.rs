//! Request encoding — `functionDeclarations` and the `functionCallingConfig` mode (Pi
//! `convertTools` / `resolveGoogleFunctionCallingMode`, google-shared.ts:272-324).

use crate::context::ToolDef;
use crate::stream::ToolChoice;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use serde_json::{Value, json};
use super::capabilities::gemini_major_version;

/// Convert tools to Gemini `functionDeclarations` (Pi `convertTools`, `google-shared.ts:318-339`
/// @v0.84.2). Uses `parametersJsonSchema` (full JSON Schema). `None` when there are no tools.
///
/// PROV-011: `supports_strict_mode` is [`supports_google_strict_tool_sampling`] of the model id,
/// threaded in from the caller (`google-generative-ai.ts:383` passes it as the third argument) so a
/// tool that opted in is serialized with the STRICT-converted schema Gemini's `VALIDATED` mode
/// constrains against. Upstream's `useParameters` legacy branch has no cyrup caller and is not
/// ported.
pub(super) fn convert_tools(
    tools: &[ToolDef],
    supports_strict_mode: bool,
) -> Result<Option<Value>, ConstrainedSamplingError> {
    if tools.is_empty() {
        return Ok(None);
    }
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            let strict = resolve_json_schema_strict_sampling(t, supports_strict_mode)?;
            Ok(json!({
                "name": t.name,
                "description": t.description,
                "parametersJsonSchema": json_schema_tool_parameters(t, strict == Some(true))?,
            }))
        })
        .collect::<Result<Vec<Value>, ConstrainedSamplingError>>()?;
    Ok(Some(json!([{ "functionDeclarations": decls }])))
}

/// Map a tool-choice to a Gemini `functionCallingConfig.mode` (Pi `mapToolChoice`,
/// google-shared.ts:293-304). cyrup's [`ToolChoice`] maps `Auto/None/Required→Function?` onto
/// `AUTO/NONE/ANY`; a named-function choice constrains to `ANY` (Gemini has no per-name mode).
fn map_tool_choice(tc: &ToolChoice) -> &'static str {
    match tc {
        ToolChoice::None => "NONE",
        ToolChoice::Required | ToolChoice::Function { .. } => "ANY",
        ToolChoice::Auto => "AUTO",
    }
}

/// Pi `supportsGoogleStrictToolSampling` (`google-shared.ts:292-295` @**v0.83.0**): Gemini major
/// version >= 3. A non-Gemini id has no major version and is therefore **false** — note this is
/// the OPPOSITE default from
/// [`supports_multimodal_function_response`](super::capabilities::supports_multimodal_function_response),
/// which returns `true` for the same input.
pub(super) fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
    gemini_major_version(model_id).is_some_and(|v| v >= 3)
}

/// Pi `resolveGoogleFunctionCallingMode` (`google-shared.ts:311-324` @**v0.83.0**) — PROV-011.
///
/// The `VALIDATED` mode is the whole point of the Google leg: it is the one route where a strict
/// tool buys a server-side guarantee that the emitted `functionCall` matches the declared schema,
/// rather than a hint. It is returned only when no explicit `none`/`any` choice overrides it.
///
/// `Array.prototype.some` short-circuits on the first `true`, so a later tool whose
/// `strict: "require"` cannot be honoured is never evaluated. That is reproduced exactly: the
/// iteration stops at the first tool that resolves `true`.
pub(super) fn resolve_google_function_calling_mode(
    tools: &[ToolDef],
    tool_choice: Option<&ToolChoice>,
    supports_strict_mode: bool,
) -> Result<Option<&'static str>, ConstrainedSamplingError> {
    let mut use_strict_mode = false;
    for tool in tools {
        if resolve_json_schema_strict_sampling(tool, supports_strict_mode)? == Some(true) {
            use_strict_mode = true;
            break;
        }
    }
    // `toolChoice === "none" || toolChoice === "any"` — an explicit hard choice wins over
    // VALIDATED. cyrup's `ToolChoice::Required`/`Function` are the two spellings that map to
    // pi's `"any"`; `Auto` is pi's `"auto"` and does NOT take this branch.
    if let Some(tc @ (ToolChoice::None | ToolChoice::Required | ToolChoice::Function { .. })) =
        tool_choice
    {
        return Ok(Some(map_tool_choice(tc)));
    }
    if use_strict_mode {
        return Ok(Some("VALIDATED"));
    }
    Ok(tool_choice.map(map_tool_choice))
}
