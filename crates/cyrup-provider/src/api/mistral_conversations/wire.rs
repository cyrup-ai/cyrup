//! Request encoding — the SDK-style payload → Mistral wire conversion (PROV-152; Pi
//! `toMistralWirePayload` / `toMistralWireMessage` / `toMistralWireContentChunk` /
//! `remapMistralProperty`, `mistral-conversations.ts:386-451` @f1b2e77f5).
//!
//! pi builds its chat payload in the `@mistralai/mistralai` SDK's camelCase (`maxTokens`,
//! `promptMode`, `toolCalls`, `imageUrl`, …), hands THAT to `onPayload`, and only then converts it
//! to Mistral's own snake_case with these functions (`requestMistralStream`,
//! `body: JSON.stringify(toMistralWirePayload(payload))`, `:318`). The SDK used to do the
//! conversion inside its generated zod schemas; when pi replaced the SDK with a native transport
//! (`9dd90a497`, v1.0.1) the conversion moved here. cyrup keeps the same split: `payload.rs` /
//! `messages.rs` build the SDK-style payload, the `before_provider_request` hook sees it, and
//! [`to_mistral_wire_payload`] runs last, so a hook that adds an SDK-style key (`topP`,
//! `parallelToolCalls`, …) still reaches the wire in Mistral's spelling.
//!
//! Why it matters: Mistral's OpenAPI declares `ChatCompletionRequest`, `AssistantMessage`,
//! `ToolMessage` and `ImageURLChunk` as `additionalProperties: false` with a documented `422`
//! validation error, so an unconverted camelCase key is a rejected request, not an ignored one.

use serde_json::{Map, Value};

/// Pi `toMistralWirePayload`'s top-level rename table (`mistral-conversations.ts:388-401`), in
/// upstream's order.
const PAYLOAD_KEYS: [(&str, &str); 12] = [
    ("topP", "top_p"),
    ("maxTokens", "max_tokens"),
    ("randomSeed", "random_seed"),
    ("responseFormat", "response_format"),
    ("toolChoice", "tool_choice"),
    ("presencePenalty", "presence_penalty"),
    ("frequencyPenalty", "frequency_penalty"),
    ("parallelToolCalls", "parallel_tool_calls"),
    ("reasoningEffort", "reasoning_effort"),
    ("promptMode", "prompt_mode"),
    ("promptCacheKey", "prompt_cache_key"),
    ("safePrompt", "safe_prompt"),
];

/// Pi `toMistralWireContentChunk`'s rename table (`:434-441`).
const CHUNK_KEYS: [(&str, &str); 6] = [
    ("imageUrl", "image_url"),
    ("documentUrl", "document_url"),
    ("documentName", "document_name"),
    ("fileId", "file_id"),
    ("referenceIds", "reference_ids"),
    ("inputAudio", "input_audio"),
];

/// Pi `remapMistralProperty` (`:447-451`): move `source` to `target` when present, overwriting any
/// existing `target` exactly as upstream's `record[target] = record[source]` does.
fn remap(record: &mut Map<String, Value>, source: &str, target: &str) {
    if let Some(v) = record.remove(source) {
        record.insert(target.to_string(), v);
    }
}

/// Convert an SDK-style chat payload to Mistral's wire shape (Pi `toMistralWirePayload`,
/// `mistral-conversations.ts:386-420` @f1b2e77f5). A non-object payload (only reachable through a
/// hook that replaced the body with one) is passed through untouched.
pub(super) fn to_mistral_wire_payload(payload: Value) -> Value {
    let Value::Object(mut wire) = payload else {
        return payload;
    };
    for (source, target) in PAYLOAD_KEYS {
        remap(&mut wire, source, target);
    }
    if let Some(Value::Array(messages)) = wire.get_mut("messages") {
        for message in messages.iter_mut() {
            to_mistral_wire_message(message);
        }
    }
    // `response_format.jsonSchema` → `json_schema`, and inside it `schemaDefinition` → `schema`
    // (`:406-417`). The user's schema itself is data and is not walked.
    if let Some(Value::Object(response_format)) = wire.get_mut("response_format") {
        remap(response_format, "jsonSchema", "json_schema");
        if let Some(Value::Object(json_schema)) = response_format.get_mut("json_schema") {
            remap(json_schema, "schemaDefinition", "schema");
        }
    }
    Value::Object(wire)
}

/// Pi `toMistralWireMessage` (`:422-430`): `toolCalls` → `tool_calls`, `toolCallId` →
/// `tool_call_id`, and each array content chunk through [`to_mistral_wire_content_chunk`].
fn to_mistral_wire_message(message: &mut Value) {
    let Value::Object(wire) = message else {
        return;
    };
    remap(wire, "toolCalls", "tool_calls");
    remap(wire, "toolCallId", "tool_call_id");
    if let Some(Value::Array(chunks)) = wire.get_mut("content") {
        for chunk in chunks.iter_mut() {
            to_mistral_wire_content_chunk(chunk);
        }
    }
}

/// Pi `toMistralWireContentChunk` (`:432-445`).
fn to_mistral_wire_content_chunk(chunk: &mut Value) {
    if let Value::Object(wire) = chunk {
        for (source, target) in CHUNK_KEYS {
            remap(wire, source, target);
        }
    }
}
