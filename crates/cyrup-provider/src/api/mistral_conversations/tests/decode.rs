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

/// PROV-073 — post-port drift. Pi keys the streamed tool block on
/// `toolCall.index ?? callId` (`mistral-conversations.ts:695` @v0.87.1); cyrup keyed it on the
/// composite `{call_id}:{index}` (the v0.84.1 shape, changed upstream by #8387 in v0.84.4) after
/// collapsing an absent `index` to `0`. An id-less continuation chunk therefore derived a
/// synthetic `call_id`, computed a DIFFERENT key, and opened a SECOND tool block holding the tail
/// of the arguments.
#[tokio::test]
async fn prov073_an_id_less_continuation_chunk_appends_to_the_indexed_block() {
    let m = model_with("codestral-latest", false);

    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"toolCalls\":[{\"id\":\"abcdefghi\",\"index\":0,\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"a\\\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"toolCalls\":[{\"index\":0,\"function\":{\"arguments\":\":1}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    let tool_calls: Vec<_> = msg
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_calls.len(),
        1,
        "the continuation chunk must append, not open a second block: {tool_calls:?}"
    );
    assert_eq!(tool_calls[0].id.as_str(), "abcdefghi");
    assert_eq!(tool_calls[0].name, "read");
    assert_eq!(
        tool_calls[0].arguments.get("a").and_then(Value::as_i64),
        Some(1)
    );

    // NEGATIVE: with NO `index` on either chunk, pi falls back to `callId`, so two distinct ids
    // still open two blocks. This pins the `None => call_id` arm — it would fail if the fix keyed
    // unconditionally on the collapsed index.
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"toolCalls\":[{\"id\":\"aaaaaaaaa\",\"function\":{\"name\":\"read\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"toolCalls\":[{\"id\":\"bbbbbbbbb\",\"function\":{\"name\":\"write\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    let ids: Vec<String> = msg
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(tc.id.as_str().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["aaaaaaaaa".to_string(), "bbbbbbbbb".to_string()]);
}

/// PROV-084, Mistral half: `readMistralEvents` is a hand-rolled `body.getReader()` loop, not the
/// Mistral SDK's stream helper, and it flushes the residual buffer after the boundary drain and
/// after the `done` break — `if (done) break;` (`mistral-conversations.ts:468` @v0.87.1) then
/// `if (buffer.trim()) { const event = parseMistralEvent(buffer); if (event !== MISTRAL_STREAM_DONE
/// && event) yield event; }` (`:471-474`). So a transcript cut right after the chunk that carries
/// `finishReason`, with no terminating blank line and no `data: [DONE]`, still finishes upstream.
/// cyrup had `flush_at_eof: false` here on the false premise that pi frames Mistral with an SDK
/// helper, so that last chunk was dropped, no chunk carried a `finishReason`, and `driver.rs:87-91`
/// ended the turn on "Mistral stream ended without a finish reason".
#[tokio::test]
async fn prov084_a_transcript_cut_after_the_finish_reason_chunk_still_finishes() {
    let raw = concat!(
        "data: {\"id\":\"resp_9\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        // The stream ends HERE — no blank line after the last data line, and no `[DONE]`.
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}],\"usage\":{\"promptTokens\":3,\"completionTokens\":4,\"totalTokens\":7}}\n",
    );
    let m = model_with("codestral-latest", false);
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { .. })),
        "no error terminal expected, got {events:?}"
    );
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));
    assert_eq!(message.response_id.as_deref(), Some("resp_9"));
    assert_eq!(message.usage.total_tokens, 7);
}

/// PROV-084, Mistral half, over the LIVE `run()` path — the one place the production
/// `SseRequest.flush_at_eof` (`mistral_conversations/mod.rs:146`) is observable. `readMistralEvents`
/// is a hand-rolled `body.getReader()` loop, not the Mistral SDK's stream helper, and its
/// `if (done) break;` (`mistral-conversations.ts:468` @v0.87.1) is followed by
/// `if (buffer.trim()) { const event = parseMistralEvent(buffer); if (event !== MISTRAL_STREAM_DONE
/// && event) yield event; }` (`:471-474`), so a socket closed right after the chunk that carries
/// `finishReason` — no blank line, no `data: [DONE]` — still finishes the turn upstream.
///
/// The replay-path sibling (`prov084_a_transcript_cut_after_the_finish_reason_chunk_still_finishes`)
/// pins the decoder, but it reads through `decode_sse_bytes_flushing_at_eof`, whose flag is
/// hard-coded in the test harness: it stays green if the live gate regresses to `false`. This test is
/// the one that goes red — without the flag the last chunk is dropped, no chunk carries a
/// `finishReason`, and `driver.rs` ends the turn on "Mistral stream ended without a finish reason".
#[tokio::test]
async fn prov084_the_live_run_path_flushes_a_reply_cut_after_the_finish_reason_chunk() {
    let base = serve_once(concat!(
        "data: {\"id\":\"resp_eof\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        // The response body ends HERE: one `\n` after the last data line, no blank line.
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}],\"usage\":{\"promptTokens\":3,\"completionTokens\":4,\"totalTokens\":7}}\n",
    ))
    .await;
    let m = model_with("codestral-latest", false);
    let events = run_against(&base, &m).await;

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { .. })),
        "no error terminal expected, got {events:?}"
    );
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));
    assert_eq!(message.response_id.as_deref(), Some("resp_eof"));
    assert_eq!(message.usage.total_tokens, 7);
}

/// PROV-115 — upstream-drift. `8930b9ec0` ("ignore empty Mistral content deltas") guards both of
/// pi's text branches (`mistral-conversations.ts:636`, `:678` @v1.0.0) with `if (!textDelta)
/// continue;` and says why: *"GLM models on Mistral send empty content deltas around thinking and
/// tool calls. Opening a block for them splits thinking into multiple blocks, which Mistral rejects
/// on replay."* cyrup's `push_text` opened a block for ANY delta, empty included — `close_current`
/// first emitted `ThinkingEnd`, then a fresh `text:""` block started — so a turn on `zai-glm-5-3`
/// (shipped in `providers/catalog/mistral.json`) landed `thinking / text:"" / thinking` instead of
/// one thinking block. The thinking branch already skipped empties (`content.rs:117`), so the split
/// came entirely from the text path.
#[tokio::test]
async fn prov115_an_empty_text_delta_does_not_split_thinking_into_two_blocks() {
    let m = model_with("zai-glm-5-3", true);

    // The shape GLM-on-Mistral sends: a thinking delta, an EMPTY content delta, a thinking delta.
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"a\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"b\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    assert_eq!(
        msg.content.len(),
        1,
        "the empty delta must not open a text block: {:?}",
        msg.content
    );
    let Content::Thinking { thinking, .. } = &msg.content[0] else {
        panic!("expected one thinking block, got {:?}", msg.content);
    };
    assert_eq!(thinking, "ab", "both thinking deltas land in ONE block");
    // No block was ever opened for the empty delta, so no text event was emitted either.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            StreamEvent::TextStart { .. } | StreamEvent::TextDelta { .. }
        )),
        "no text events expected, got {events:?}"
    );

    // MIRROR: the `{type:"text"}` item form of the same empty delta — pi's second guarded site
    // (`:678`). An array item `{"type":"text","text":""}` between two thinking deltas splits the
    // same way through the same `push_text`.
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"a\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"\"}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"b\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    assert_eq!(
        msg.content.len(),
        1,
        "an empty `text` item must not open a block either: {:?}",
        msg.content
    );

    // MIRROR: the bare-string array item — cyrup's third site, which pi reaches through the same
    // guarded branch because it normalizes string content to `[delta.content]` (`:630`).
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"a\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[\"\"]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"b\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    assert_eq!(
        msg.content.len(),
        1,
        "an empty string array item must not open a block either: {:?}",
        msg.content
    );

    // NEGATIVE: a NON-empty text delta between two thinking deltas still splits — that is upstream's
    // behaviour and the guard must not widen into dropping real text. This arm fails if the fix
    // suppressed the block unconditionally instead of only for an empty delta.
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"a\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"mid\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"b\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");
    assert_eq!(
        msg.content.len(),
        3,
        "real text still splits: {:?}",
        msg.content
    );
    assert!(matches!(&msg.content[1], Content::Text { text, .. } if text == "mid"));
}

/// PROV-115, the consequence half. The split only matters because it survives into the REQUEST:
/// `to_chat_messages` emits one `{"type":"thinking","thinking":[…]}` entry per thinking block
/// (`messages.rs:61-68`), and the empty text block between them is dropped by its own
/// `!text.trim().is_empty()` test (`:55`) — so the replayed assistant message carries TWO thinking
/// entries, which is the body Mistral rejects. This test replays the message a GLM turn produces and
/// asserts a single thinking entry; at HEAD the decoder handed it two blocks and it was two.
#[tokio::test]
async fn prov115_a_glm_turn_replays_as_one_assistant_thinking_entry() {
    let m = model_with("zai-glm-5-3", true);
    let raw = concat!(
        "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"step one \"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"step two\"}]}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]},\"finishReason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let msg = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("done terminal");

    // Second turn: that assistant message goes back on the wire.
    let replayed = to_chat_messages(
        &[
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::Assistant((*msg).clone()),
            Message::User {
                content: vec![Content::text("and now?")],
                timestamp: 0,
            },
        ],
        false,
    );
    let assistant = replayed
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
        .expect("an assistant message");
    let parts = assistant
        .get("content")
        .and_then(Value::as_array)
        .expect("content parts");
    let thinking: Vec<&Value> = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("thinking"))
        .collect();
    assert_eq!(
        thinking.len(),
        1,
        "Mistral rejects a replay with two thinking entries: {parts:?}"
    );
    assert_eq!(
        thinking[0]
            .get("thinking")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|p| p.get("text"))
            .and_then(Value::as_str),
        Some("step one step two")
    );
    // The real text still rides along as its own entry.
    assert!(
        parts
            .iter()
            .any(|p| p.get("type").and_then(Value::as_str) == Some("text")
                && p.get("text").and_then(Value::as_str) == Some("done")),
        "the real text must survive: {parts:?}"
    );
}
