//! Message + tool conversion (Pi openai-responses-shared.ts): the `tools` array.

use crate::context::ToolDef;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use serde_json::{Map, Value, json};

/// Pi's `ConvertResponsesToolsOptions` (`openai-responses-shared.ts:344-347` @v0.83.0), carrying
/// only the members cyrup's function-tool branch consumes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConvertResponsesToolsOptions {
    /// Pi `options.deferLoading`: set only for definitions carried inside a `tool_search_output`,
    /// never for `body.tools`.
    pub defer_loading: bool,
    /// Pi `const supportsStrictMode = options?.supportsStrictMode ?? true`
    /// (`openai-responses-shared.ts:346`). Each caller resolves it from its OWN compat default —
    /// `?? false` on openai-responses (`openai-responses.ts:72`), `?? true` on azure
    /// (`azure-openai-responses.ts:302`) and codex (`openai-codex-responses.ts:539`).
    pub supports_strict_mode: bool,
    /// Pi `const defaultStrict = options?.strict === undefined ? false : options.strict`
    /// (`:345`). `Some(b)` is a JSON boolean; **`None` is JSON `null`**, which is what
    /// `openai-codex-responses.ts:576` passes (`strict: null`) — not an absent key.
    pub default_strict: Option<bool>,
}

/// 1:1 port of Pi `convertResponsesTools` (`openai-responses-shared.ts:359-395` @v0.84.2).
///
/// PROV-034: the `strict` key is emitted **only** when `supportsStrictMode` (`:376-377`). pi's
/// function-tool literal is built without it, so a model that does not opt in receives no `strict`
/// key at all — where cyrup used to hard-code `"strict": false` on every tool of every request.
///
/// PROV-011: `strict` is `resolveJsonSchemaStrictSampling(tool, supportsStrictMode) ?? defaultStrict`
/// (`:365`, `:377`), so a tool that opted into JSON-schema constrained sampling gets `true` and
/// every other tool keeps the caller's default. A `strict: "require"` tool on a route without
/// strict mode fails the request with pi's exact message.
pub(crate) fn convert_responses_tools(
    tools: &[ToolDef],
    options: ConvertResponsesToolsOptions,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    tools
        .iter()
        .map(|t| {
            let constrained_strict =
                resolve_json_schema_strict_sampling(t, options.supports_strict_mode)?;
            // `const strict = constrainedStrict ?? defaultStrict` (`:381` @v0.84.2) — resolved
            // BEFORE the schema is converted, so a caller-supplied `default_strict = Some(true)`
            // converts too, and reused verbatim for the `strict` key below.
            let strict = constrained_strict.or(options.default_strict);
            let mut o = Map::new();
            o.insert("type".to_string(), json!("function"));
            o.insert("name".to_string(), json!(t.name));
            o.insert("description".to_string(), json!(t.description));
            o.insert(
                "parameters".to_string(),
                json_schema_tool_parameters(t, strict == Some(true))?,
            );
            // Pi spreads `...(options?.deferLoading ? { defer_loading: true } : {})` — the key is
            // ABSENT, not `false`, when the tool is part of the request prefix.
            if options.defer_loading {
                o.insert("defer_loading".to_string(), json!(true));
            }
            if options.supports_strict_mode {
                o.insert(
                    "strict".to_string(),
                    match strict {
                        Some(b) => json!(b),
                        None => Value::Null,
                    },
                );
            }
            Ok(Value::Object(o))
        })
        .collect()
}
