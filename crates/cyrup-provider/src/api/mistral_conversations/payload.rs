//! Request encoding — the `/v1/chat/completions` request body (Pi `buildChatPayload`,
//! mistral-conversations.ts:230-270).

use crate::api::compat::sanitize_surrogates;
use crate::api::openai_completions::transform_messages_with;
use crate::context::Context;
use crate::model::{Modality, Model};
use crate::stream::StreamOptions;
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use serde_json::{Map, Value, json};
use super::endpoint::should_use_prompt_caching;
use super::messages::to_chat_messages;
use super::reasoning::lower_reasoning;
use super::tool_call_id::MistralToolCallIdNormalizer;
use super::tools::{map_tool_choice, to_function_tools};

/// Test-only convenience wrapper for [`build_chat_payload`].
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(super) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_chat_payload(model, ctx, opts)
        .expect("fixture declares no unsatisfiable constrained sampling")
}

/// Build the `chat/completions` request body (1:1 port of Pi `buildChatPayload` + the `streamSimple`
/// reasoning lowering, mistral-conversations.ts:110-131,240-268).
pub(super) fn build_chat_payload(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
) -> Result<Value, ConstrainedSamplingError> {
    let supports_images = model.input.contains(&Modality::Image);

    // Stateful 9-char tool-call-id normalizer (Pi createMistralToolCallIdNormalizer).
    let normalizer = MistralToolCallIdNormalizer::default();
    let transformed = transform_messages_with(&ctx.messages, model, |id| normalizer.normalize(id));

    let mut messages = to_chat_messages(&transformed, supports_images);

    // System prompt is prepended (Pi mistral-conversations.ts:260-265).
    if let Some(sp) = &ctx.system_prompt {
        messages.insert(
            0,
            json!({ "role": "system", "content": sanitize_surrogates(sp) }),
        );
    }

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("stream".to_string(), json!(true));
    obj.insert("messages".to_string(), Value::Array(messages));

    if !ctx.tools.is_empty() {
        obj.insert(
            "tools".to_string(),
            Value::Array(to_function_tools(&ctx.tools)?),
        );
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }
    if let Some(max) = opts.max_tokens {
        obj.insert("maxTokens".to_string(), json!(max));
    }
    if let Some(tc) = &opts.tool_choice {
        obj.insert("toolChoice".to_string(), map_tool_choice(tc));
    }

    // Reasoning lowering (Pi `streamSimple`, mistral-conversations.ts:120-130). Direct
    // `MistralOptions.promptMode`/`reasoningEffort` per-request overrides (Pi `buildChatPayload`
    // reads `options.promptMode`/`options.reasoningEffort` verbatim, mistral-conversations.ts:256-257)
    // each win over the computed value, independently of one another.
    let (mut prompt_mode, mut reasoning_effort) = lower_reasoning(model, opts.reasoning);
    if let Some(pm) = opts.mistral_options().and_then(|m| m.prompt_mode) {
        prompt_mode = Some(pm.as_wire());
    }
    if let Some(re) = opts.mistral_options().and_then(|m| m.reasoning_effort) {
        reasoning_effort = Some(re.as_wire().to_string());
    }
    if let Some(pm) = prompt_mode {
        obj.insert("promptMode".to_string(), json!(pm));
    }
    if let Some(re) = reasoning_effort {
        obj.insert("reasoningEffort".to_string(), json!(re));
    }

    if let Some(sid) = should_use_prompt_caching(opts) {
        obj.insert("promptCacheKey".to_string(), json!(sid));
    }

    Ok(Value::Object(obj))
}
