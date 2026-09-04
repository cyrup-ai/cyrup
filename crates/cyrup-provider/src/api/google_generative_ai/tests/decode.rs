//! The SSE decoder.

use super::*;

/// PORT BUG (present at v0.83.0, never ported): pi writes
/// `output.rawStopReason = candidate.finishReason`
/// (`v0.84.1 ai/src/api/google-generative-ai.ts:216`) and READS it back to compose the terminal
/// error (`:271-273`). cyrup reaches the same message text from the other end — [`map_stop_reason`]
/// bakes `Provider stopped with: …` in at map time — so the user-visible message was NOT
/// degraded; the RECORDED field was simply never written. Both halves are asserted here.
#[tokio::test]
async fn a_finish_reason_is_recorded_raw_and_names_itself_in_the_error() {
    let m = model_with("gemini-2.5-pro", true);

    let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"SAFETY\"}]}\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    // The READ half (pi `:271-273`): the reason names itself, never the generic fallback.
    assert_eq!(
        error.error_message.as_deref(),
        Some("Provider stopped with: SAFETY")
    );
    // The WRITE half (pi `:216`): the raw string is on the turn.
    assert_eq!(error.raw_stop_reason.as_deref(), Some("SAFETY"));

    // MIRROR 1: a clean STOP records its raw word on the `done` terminal.
    let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}]}\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("STOP"));

    // MIRROR 2: pi does NOT unset `rawStopReason` when the tool-call override rewrites
    // `stopReason` to `"toolUse"` (`:218-220`) — the raw word outlives the override.
    let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read\",\"args\":{}}}]},\"finishReason\":\"MALFORMED_FUNCTION_CALL\"}]}\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(message.error_message, None);
    assert_eq!(
        message.raw_stop_reason.as_deref(),
        Some("MALFORMED_FUNCTION_CALL")
    );

    // MIRROR 3: a truncated stream never delivered a finishReason, so there is nothing to record.
    let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let last = events.last().expect("a terminal");
    assert_eq!(
        last.terminal_message()
            .and_then(|t| t.raw_stop_reason.clone()),
        None
    );
}

#[tokio::test]
async fn decodes_text_thinking_and_tool_stream() {
    // A Gemini SSE transcript: a thinking part, a text part, then a functionCall + finishReason.
    let raw = concat!(
        "data: {\"responseId\":\"resp_1\",\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"reasoning\"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3,\"totalTokenCount\":15}}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read\",\"args\":{\"path\":\"a\"}}}]},\"finishReason\":\"STOP\"}]}\n\n",
    );
    let m = model_with("gemini-2.5-pro", true);
    let events = collect(raw.as_bytes().to_vec(), &m).await;

    assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ThinkingDelta { delta, .. } if delta == "reasoning"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta { delta, .. } if delta == "Hello"))
    );
    let tool = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        })
        .expect("toolcall_end");
    assert_eq!(tool.name, "read");
    assert_eq!(
        tool.arguments.get("path").and_then(Value::as_str),
        Some("a")
    );

    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    // A tool call is present → toolUse overrides the STOP finishReason.
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.response_id.as_deref(), Some("resp_1"));
    // input = prompt - cached = 10; output = candidates + thoughts = 5; total preserved from API.
    assert_eq!(msg.usage.input, 10);
    assert_eq!(msg.usage.output, 5);
    assert_eq!(msg.usage.reasoning, Some(3));
    assert_eq!(msg.usage.total_tokens, 15);
    assert!(msg.usage.cost.total > 0.0);
    // Content order: thinking, text, toolCall.
    assert!(matches!(msg.content[0], Content::Thinking { .. }));
    assert!(matches!(msg.content[1], Content::Text { .. }));
    assert!(matches!(msg.content[2], Content::ToolCall(_)));
}

#[tokio::test]
async fn synthesizes_tool_call_id_when_absent() {
    let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"ping\",\"args\":{}}}]},\"finishReason\":\"STOP\"}]}\n\n";
    let m = model_with("gemini-2.5-pro", true);
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let tool = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        })
        .expect("toolcall_end");
    // Synthesized id is `{name}_{millis}_{counter}`.
    assert!(
        tool.id.as_str().starts_with("ping_"),
        "got: {}",
        tool.id.as_str()
    );
}
