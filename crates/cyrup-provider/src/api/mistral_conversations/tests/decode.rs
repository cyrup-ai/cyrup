//! The SSE decoder.

use super::*;

/// PORT BUG (present at v0.83.0, never ported): pi writes
/// `output.rawStopReason = choice.finishReason`
/// (`v0.84.1 ai/src/api/mistral-conversations.ts:356`, same line at v0.83.0). cyrup filled
/// `raw_stop_reason: None`, so a `content_filter` stop and an unrecognized future reason were
/// indistinguishable once both collapsed into [`StopReason::Error`].
#[tokio::test]
async fn a_finish_reason_is_recorded_raw_beside_the_narrowed_one() {
    let m = model_with("codestral-latest", false);

    let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"content_filter\"}]}\n\ndata: [DONE]\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(error.raw_stop_reason.as_deref(), Some("content_filter"));

    // MIRROR 1: a clean `stop` keeps its raw word on the `done` terminal.
    let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finishReason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));

    // MIRROR 2: pi's guard is `if (choice.finishReason)` (`:355`), so a null one assigns
    // nothing — the field stays absent on the truncation terminal.
    let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finishReason\":null}]}\n\ndata: [DONE]\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let last = events.last().expect("a terminal");
    assert_eq!(
        last.terminal_message()
            .and_then(|t| t.raw_stop_reason.clone()),
        None
    );
}

#[tokio::test]
async fn decodes_text_and_tool_stream() {
    let raw = concat!(
        "data: {\"id\":\"resp_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"toolCalls\":[{\"id\":\"abcdefghi\",\"index\":0,\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"tool_calls\"}],\"usage\":{\"promptTokens\":10,\"completionTokens\":4,\"totalTokens\":14}}\n\n",
        "data: [DONE]\n\n",
    );
    let m = model_with("codestral-latest", false);
    let events = collect(raw.as_bytes().to_vec(), &m).await;

    assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
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
    assert_eq!(tool.id.as_str(), "abcdefghi");
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
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.response_id.as_deref(), Some("resp_1"));
    assert_eq!(msg.usage.input, 10);
    assert_eq!(msg.usage.output, 4);
    assert_eq!(msg.usage.total_tokens, 14);
}

#[tokio::test]
async fn decodes_thinking_chunks() {
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"ponder\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"answer\"}]},\"finishReason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let m = model_with("magistral-small", true);
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ThinkingDelta { delta, .. } if delta == "ponder"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta { delta, .. } if delta == "answer"))
    );
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done");
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert!(matches!(msg.content[0], Content::Thinking { .. }));
    assert!(matches!(msg.content[1], Content::Text { .. }));
}
