//! Pi-fidelity message and payload shaping — the transcript messages and event JSON the loop
//! must produce byte-for-byte, including which optional keys are present (AGENT-009).

use super::util::now_millis;
use cyrup_core::{
    ApiId, AssistantMessage, Content, ModelRef, ProviderId, StopReason, ToolCall, ToolUpdate, Usage,
    UNRESOLVED_API,
};
use serde_json::Value;

/// An errored assistant transcript message matching Pi `handleRunFailure` (agent.ts:497-506): one
/// EMPTY text block (`[{type:"text", text:""}]`, NOT empty content) plus a `Date.now()` timestamp.
/// Both reach the wire payload via `convert_to_llm`, so they must mirror Pi byte-for-byte.
/// `cyrup_core::AssistantMessage::errored` yields `content: []`/`timestamp: 0`; this overlays Pi's
/// single empty text block and wall-clock stamp on top (the `errored` type lives in cyrup-core and is
/// shared, so the Pi-specific transcript shape is applied here at the agent boundary).
pub(super) fn errored_assistant(
    provider: ProviderId,
    model: &str,
    api: Option<ApiId>,
    stop_reason: StopReason,
    msg: impl Into<String>,
) -> AssistantMessage {
    let mut m = AssistantMessage::errored(provider, model, api, stop_reason, msg);
    m.content = vec![Content::text("")];
    m.timestamp = now_millis();
    m
}

pub(super) fn empty_assistant(model: &ModelRef) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.model.to_string(),
        // Pi: AssistantMessage.api is required (types.ts:386). A ModelRef built from a not-yet-
        // resolved user selection may carry no api; fall back to the sentinel so the field is set.
        api: model.api.clone().unwrap_or_else(|| ApiId::from(UNRESOLVED_API)),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        // This message is only ever a PARTIAL — it seeds `partial` before the first `start` event
        // and is replaced wholesale by `event.partial()` thereafter (agent-loop.ts:313-314). Pi's
        // corresponding seed is `stopReason: "pending"` in each stream function's `output`; a
        // `Stop` seed made a `message_start` emitted on a pre-first-event abort claim a completed
        // turn. It never reaches `message_end`: every return path stamps a settled reason.
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    }
}

pub(super) fn tool_calls(a: &AssistantMessage) -> Vec<ToolCall> {
    a.content
        .iter()
        .filter_map(|c| if let Content::ToolCall(tc) = c { Some(tc.clone()) } else { None })
        .collect()
}

/// An empty JSON object — pi's `createErrorToolResult` returns `details: {}` (an object LITERAL,
/// `packages/agent/src/agent-loop.ts:756-761` @v0.83.0), not `undefined`, so every loop-generated
/// error result carries the key with an empty map. AGENT-009.
pub(super) fn empty_details() -> Value {
    Value::Object(serde_json::Map::new())
}

/// The `tool_execution_end.result` payload — Pi emits `result: finalized.result` VERBATIM
/// (`emitToolExecutionEnd`, `agent-loop.ts:763-771` @v0.83.0), so `JSON.stringify` drops every
/// absent key. `details`, `usage`, `addedToolNames` and `terminate` are all optional on
/// `AgentToolResult` (`types.ts:354-368`), so an absent value must produce NO key at all rather than
/// a `null` (AGENT-009). Only `content` is unconditional.
pub(super) fn result_value_of(
    content: &[Content],
    details: &Option<Value>,
    usage: Option<&Usage>,
    added_tool_names: &[String],
    terminate: Option<bool>,
) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("content".to_string(), serde_json::to_value(content).unwrap_or(Value::Null));
    if let Some(d) = details {
        obj.insert("details".to_string(), d.clone());
    }
    if let Some(u) = usage {
        obj.insert("usage".to_string(), serde_json::to_value(u).unwrap_or(Value::Null));
    }
    if !added_tool_names.is_empty() {
        obj.insert(
            "addedToolNames".to_string(),
            serde_json::to_value(added_tool_names).unwrap_or(Value::Null),
        );
    }
    if let Some(t) = terminate {
        obj.insert("terminate".to_string(), Value::Bool(t));
    }
    Value::Object(obj)
}

/// The `tool_execution_update.partialResult` payload — Pi emits the tool's `AgentToolResult`
/// (`{content, details?, terminate?}`) verbatim (agent-loop.ts:681-691 @v0.83.0), so BOTH optional
/// keys are dropped by `JSON.stringify` when the tool left them `undefined` (types.ts:350-360).
/// Mirror that: include `details`/`terminate` only when `Some`, never as a `null`.
pub(super) fn update_value(u: &ToolUpdate) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("content".to_string(), serde_json::to_value(&u.content).unwrap_or(Value::Null));
    if let Some(d) = &u.details {
        obj.insert("details".to_string(), d.clone());
    }
    if let Some(t) = u.terminate {
        obj.insert("terminate".to_string(), Value::Bool(t));
    }
    Value::Object(obj)
}
