//! SSE decoding into ordered stream events.

use super::*;

#[tokio::test]
async fn decodes_text_and_tool_stream() {
    // A realistic Anthropic SSE transcript: message_start, a text block, a tool_use block, and
    // message_delta(tool_use) + message_stop.
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"read\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"a\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
    // text delta carried "Hello".
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta { delta, .. } if delta == "Hello"))
    );
    // tool call end with parsed args.
    let tool_end = events.iter().find_map(|e| match e {
        StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
        _ => None,
    });
    let tool = tool_end.expect("toolcall_end");
    assert_eq!(tool.id.as_str(), "toolu_9");
    assert_eq!(tool.name, "read");
    assert_eq!(
        tool.arguments.get("path").and_then(Value::as_str),
        Some("a")
    );
    // terminal done with ToolUse + usage/cost computed.
    let done = events.iter().find_map(|e| match e {
        StreamEvent::Done { message, .. } => Some(message.clone()),
        _ => None,
    });
    let msg = done.expect("done terminal");
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.response_id.as_deref(), Some("msg_1"));
    assert_eq!(msg.usage.input, 10);
    assert_eq!(msg.usage.output, 7);
    assert!(msg.usage.cost.total > 0.0);
}

#[tokio::test]
async fn decodes_thinking_with_signature() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reason\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SIG\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let done = events.iter().find_map(|e| match e {
        StreamEvent::Done { message, .. } => Some(message.clone()),
        _ => None,
    });
    let msg = done.expect("done");
    assert_eq!(msg.stop_reason, StopReason::Stop);
    let thinking = msg.content.iter().find_map(|c| match c {
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => Some((thinking.clone(), thinking_signature.clone())),
        _ => None,
    });
    let (thinking, sig) = thinking.expect("thinking block");
    assert_eq!(thinking, "reason");
    assert_eq!(sig.as_deref(), Some("SIG"));
}

/// DRIFT-003: `content_block_start` may already carry the head of a text block. Pi seeds the
/// block with `event.content_block.text ?? ""`; dropping it silently truncates the reply.
#[tokio::test]
async fn content_block_start_text_payload_is_kept() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hel\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;

    // The seeded head is visible on the very first partial snapshot, not only at the end.
    let start_partial = events.iter().find_map(|e| match e {
        StreamEvent::TextStart { partial, .. } => Some(partial.clone()),
        _ => None,
    });
    let start_text = start_partial
        .expect("text_start")
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .expect("text block on the start partial");
    assert_eq!(start_text, "Hel");

    let done = events.iter().find_map(|e| match e {
        StreamEvent::Done { message, .. } => Some(message.clone()),
        _ => None,
    });
    let msg = done.expect("done");
    let text = msg
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .expect("text block");
    assert_eq!(text, "Hello", "the content_block_start head was dropped");
}

/// PORT BUG (present at v0.83.0, never ported): pi writes
/// `output.rawStopReason = event.delta.stop_reason` at
/// `v0.84.1 ai/src/api/anthropic-messages.ts:709`, and cyrup filled `raw_stop_reason: None` at
/// every construction site. The narrowing map is lossy — `refusal`, `sensitive` and every
/// unknown reason all become [`StopReason::Error`] — so without the raw string the turn no
/// longer records WHICH one the provider sent.
#[tokio::test]
async fn message_delta_records_the_providers_own_stop_reason() {
    let head = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
    );
    let stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    // A refusal maps to `error`; only the raw string says it was a refusal and not a transport
    // failure. `emit_error` builds its terminal from the same snapshot, so it must survive there.
    let refusal = format!(
        "{head}event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"refusal\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n{stop}"
    );
    let m = model();
    let events = collect(refusal.into_bytes(), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(error.raw_stop_reason.as_deref(), Some("refusal"));

    // MIRROR 1: a clean `end_turn` keeps its raw word too, on the `done` terminal AND on every
    // in-flight partial emitted after the `message_delta`.
    let clean = format!(
        "{head}event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n{stop}"
    );
    let events = collect(clean.into_bytes(), &m).await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));

    // MIRROR 2: no `message_delta` at all → nothing to record. pi never assigns, so the field
    // stays absent rather than being invented from the truncation diagnostic.
    let truncated = format!("{head}{stop}");
    let events = collect(truncated.into_bytes(), &m).await;
    let last = events.last().expect("a terminal");
    assert_eq!(
        last.terminal_message()
            .and_then(|t| t.raw_stop_reason.clone()),
        None
    );
}

/// pi's guard is `if (event.delta.stop_reason)` (`v0.84.1
/// ai/src/api/anthropic-messages.ts:708`) — JS truthiness, so `""` is not a stop reason. cyrup
/// tested only for presence, so an empty string reached `map_stop_reason` and settled the turn
/// on `Unhandled stop reason: ` instead of leaving the `"pending"` seed to be reported as the
/// truncation it is.
#[tokio::test]
async fn an_empty_stop_reason_is_not_a_stop_reason() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Anthropic stream ended without a stop reason")
    );
    assert_eq!(error.raw_stop_reason, None);
}

/// DRIFT-003: the same for thinking blocks. The signature matters most — a thinking block
/// replayed to Anthropic without its signature is rejected, so a signature delivered only on
/// the open event must survive.
#[tokio::test]
async fn content_block_start_thinking_and_signature_payload_is_kept() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"rea\",\"signature\":\"SIG-FROM-START\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"son\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let done = events.iter().find_map(|e| match e {
        StreamEvent::Done { message, .. } => Some(message.clone()),
        _ => None,
    });
    let msg = done.expect("done");
    let (thinking, sig) = msg
        .content
        .iter()
        .find_map(|c| match c {
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } => Some((thinking.clone(), thinking_signature.clone())),
            _ => None,
        })
        .expect("thinking block");
    assert_eq!(thinking, "reason", "the thinking head was dropped");
    assert_eq!(
        sig.as_deref(),
        Some("SIG-FROM-START"),
        "the signature from content_block_start was dropped — the block is unreplayable"
    );
}

#[tokio::test]
async fn missing_message_stop_is_error() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let err = events.iter().find_map(|e| match e {
        StreamEvent::Error { error, .. } => Some((**error).clone()),
        _ => None,
    });
    let msg = err.expect("error terminal");
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert!(msg.error_message.unwrap().contains("message_stop"));
}

#[tokio::test]
async fn sse_error_event_is_error_terminal() {
    let raw = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let err = events.iter().find_map(|e| match e {
        StreamEvent::Error { error, .. } => Some((**error).clone()),
        _ => None,
    });
    assert!(err.is_some());
}
