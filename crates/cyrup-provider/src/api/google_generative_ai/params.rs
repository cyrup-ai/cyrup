//! Request encoding — the `:streamGenerateContent` request body (Pi `buildParams` + the
//! `streamSimple` thinking lowering, google-generative-ai.ts:283-400).

use super::convert::convert_messages;
use super::thinking::{UnsupportedThinkingLevel, thinking_config, thinking_config_override};
use super::tools::{
    convert_tools, resolve_google_function_calling_mode, supports_google_strict_tool_sampling,
};
use crate::api::compat::sanitize_surrogates;
use crate::context::Context;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use serde_json::{Map, Value, json};
use std::fmt;

/// Everything that can make the `:streamGenerateContent` body unbuildable.
///
/// `[CYRUP-DELTA]` — google-shared.ts:61-63: pi throws for an unmappable thinking level, so its
/// `buildParams` has no error type at all. cyrup surfaces both failure modes as one `Err` the driver
/// turns into a failed turn before any HTTP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GoogleParamsError {
    /// A `constrainedSampling` configuration that cannot be honoured.
    ConstrainedSampling(ConstrainedSamplingError),
    /// A thinking rung whose `thinkingLevelMap` entry does not name a Google level.
    UnsupportedThinkingLevel(String),
}

impl fmt::Display for GoogleParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GoogleParamsError::ConstrainedSampling(e) => e.fmt(f),
            GoogleParamsError::UnsupportedThinkingLevel(m) => f.write_str(m),
        }
    }
}

impl From<ConstrainedSamplingError> for GoogleParamsError {
    fn from(e: ConstrainedSamplingError) -> Self {
        GoogleParamsError::ConstrainedSampling(e)
    }
}

impl From<UnsupportedThinkingLevel> for GoogleParamsError {
    fn from(e: UnsupportedThinkingLevel) -> Self {
        GoogleParamsError::UnsupportedThinkingLevel(e.0)
    }
}

impl From<GoogleParamsError> for crate::error::ProviderError {
    fn from(e: GoogleParamsError) -> Self {
        match e {
            GoogleParamsError::ConstrainedSampling(e) => e.into(),
            GoogleParamsError::UnsupportedThinkingLevel(m) => {
                crate::error::ProviderError::Decode(m)
            }
        }
    }
}

/// Test-only convenience wrapper for [`build_params`].
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(super) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_params(model, ctx, opts)
        .expect("fixture declares no unsatisfiable constrained sampling or thinking level")
}

/// Build the `:streamGenerateContent` request body (1:1 port of Pi `buildParams` + the `streamSimple`
/// thinking lowering, google-generative-ai.ts:283-400). The unified `opts.reasoning` level drives the
/// `thinkingConfig` (level-based for Gemini 3 / Gemma 4, token-budget-based otherwise).
pub(crate) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
) -> Result<Value, GoogleParamsError> {
    let contents = convert_messages(model, ctx);

    let mut generation_config = Map::new();
    if let Some(temp) = opts.temperature {
        generation_config.insert("temperature".to_string(), json!(temp));
    }
    if let Some(max) = opts.max_tokens {
        generation_config.insert("maxOutputTokens".to_string(), json!(max));
    }

    // Thinking lowering (Pi `streamSimple`, google-generative-ai.ts:283-319). cyrup carries the
    // unified `reasoning` level directly, so the lowering happens inline (as in `anthropic_messages`).
    // A direct `GoogleOptions.thinking` per-request override (Pi `buildParams` reading
    // `options.thinking`, google-generative-ai.ts:373-384) bypasses that lowering and is read verbatim.
    if model.reasoning {
        let cfg = match opts.google_options().and_then(|g| g.thinking) {
            Some(thinking) => thinking_config_override(model, &thinking)?,
            None => thinking_config(model, opts.reasoning)?,
        };
        if let Some(cfg) = cfg {
            generation_config.insert("thinkingConfig".to_string(), cfg);
        }
    }

    let mut obj = Map::new();
    obj.insert("contents".to_string(), Value::Array(contents));

    // systemInstruction (Pi google-generative-ai.ts:359).
    if let Some(sp) = &ctx.system_prompt {
        obj.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": sanitize_surrogates(sp) }] }),
        );
    }

    // tools + toolConfig (Pi google-generative-ai.ts:369-378 @v0.83.0). PROV-011: the mode comes
    // from `resolveGoogleFunctionCallingMode`, which can return `VALIDATED`; the old code mapped
    // `tool_choice` alone and so could never emit it.
    if !ctx.tools.is_empty() {
        // Hoisted so `convertTools` and `resolveGoogleFunctionCallingMode` share ONE capability
        // read, exactly as `google-generative-ai.ts:374` binds `supportsStrictMode` once.
        let supports_strict_mode = supports_google_strict_tool_sampling(model.id.as_str());
        if let Some(tools) = convert_tools(&ctx.tools, supports_strict_mode)? {
            obj.insert("tools".to_string(), tools);
        }
        let mode = resolve_google_function_calling_mode(
            &ctx.tools,
            opts.tool_choice.as_ref(),
            supports_strict_mode,
        )?;
        if let Some(mode) = mode {
            obj.insert(
                "toolConfig".to_string(),
                json!({ "functionCallingConfig": { "mode": mode } }),
            );
        }
    }

    if !generation_config.is_empty() {
        obj.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }

    Ok(Value::Object(obj))
}
