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

/// PROV-084, end to end: a real transcript whose final `message_stop` frame is not followed by the
/// terminating blank line. pi still yields that frame (`anthropic-messages.ts:461-464` @v0.87.1);
/// cyrup dropped it, so `driver.rs:105` reported "Anthropic stream ended before message_stop" and
/// the turn failed.
#[tokio::test]
async fn prov084_a_transcript_cut_after_message_stop_still_finishes() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
        // The stream ends HERE — no blank line after the last data line.
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
    );
    let m = model();
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { .. })),
        "no error terminal expected, got {events:?}"
    );
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    assert_eq!(msg.stop_reason, StopReason::Stop);
}

/// PROV-090 — a `message_start` whose `message.model` differs from the requested id records
/// `responseModel` and, when the model's compat prices that fallback, costs the turn with the
/// fallback's rates (pi `anthropic-messages.ts:605-614` @v0.87.1). Translated from
/// `packages/ai/test/anthropic-sse-parsing.test.ts` @v0.87.1.
mod prov090_server_side_fallback {
    use super::*;
    use crate::api::compat::AnthropicAllowedFallbackModel;

    /// `message_start` naming `fallback-model`, 100 input tokens, then a clean `end_turn`.
    const RELABELLED: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"fallback-model\",\"usage\":{\"input_tokens\":100,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"hi\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":0}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    fn with_fallback(provider: &str, id: &str) -> Model {
        Model {
            compat: Some(ModelCompat {
                allowed_fallback_models: Some(vec![AnthropicAllowedFallbackModel {
                    provider: ProviderId::from(provider),
                    model: id.to_string(),
                    cost: ModelCost {
                        input: 3.0,
                        output: 5.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                        tiers: None,
                    },
                }]),
                ..Default::default()
            }),
            ..model()
        }
    }

    async fn terminal(raw: &str, m: &Model) -> AssistantMessage {
        let events = collect(raw.as_bytes().to_vec(), m).await;
        events
            .iter()
            .find_map(StreamEvent::terminal_message)
            .map(|m| (**m).clone())
            .expect("terminal")
    }

    /// (a) A matching entry: `response_model` is recorded AND the fallback's rates cost the turn.
    /// 100 input tokens at $3/1e6 is $0.0003; `model()`'s own input rate is $5/1e6.
    #[tokio::test]
    async fn a_matching_entry_sets_response_model_and_swaps_the_cost() {
        let msg = terminal(RELABELLED, &with_fallback("anthropic", "fallback-model")).await;
        assert_eq!(msg.response_model.as_deref(), Some("fallback-model"));
        assert_eq!(msg.usage.input, 100);
        assert!(
            (msg.usage.cost.input - 0.0003).abs() < 1e-12,
            "cost.input was {}",
            msg.usage.cost.input
        );
    }

    /// (b) No matching entry (right id, wrong provider): the relabelling is still recorded, but the
    /// cost stays on `model.cost` — 100 tokens at $5/1e6 = $0.0005. This half of pi's port needs no
    /// `fallbacks` request field at all; it is the relabelling-proxy fix on its own.
    #[tokio::test]
    async fn b_no_matching_entry_keeps_the_requested_models_cost() {
        let msg = terminal(RELABELLED, &with_fallback("openrouter", "fallback-model")).await;
        assert_eq!(msg.response_model.as_deref(), Some("fallback-model"));
        assert!(
            (msg.usage.cost.input - 0.0005).abs() < 1e-12,
            "cost.input was {}",
            msg.usage.cost.input
        );
    }

    /// A `message_start` echoing the REQUESTED id leaves `response_model` unset — pi's guard is
    /// `responseModel !== model.id`, so the overwhelmingly common case is untouched.
    #[tokio::test]
    async fn an_echoed_model_id_leaves_response_model_none() {
        let raw = RELABELLED.replace("fallback-model", "claude-opus-4-5");
        let msg = terminal(&raw, &with_fallback("anthropic", "fallback-model")).await;
        assert_eq!(msg.response_model, None);
        assert!((msg.usage.cost.input - 0.0005).abs() < 1e-12);
    }

    /// (c) A `fallback` content block AFTER content has been emitted is a terminal error (pi
    /// `:627-629` throws).
    #[tokio::test]
    async fn c_a_mid_output_fallback_block_is_a_terminal_error() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"partial\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"fallback\"}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let msg = terminal(raw, &model()).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(
            msg.error_message
                .as_deref()
                .unwrap_or_default()
                .contains("unsupported mid-output model fallback"),
            "error_message was {:?}",
            msg.error_message
        );
    }

    /// (d) A `fallback` block as the FIRST content block is skipped, not an error (pi's `continue`).
    #[tokio::test]
    async fn d_a_leading_fallback_block_is_ignored() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"fallback\"}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"ok\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let msg = terminal(raw, &model()).await;
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert_eq!(msg.error_message, None);
        assert_eq!(msg.content.len(), 1);
    }
}
