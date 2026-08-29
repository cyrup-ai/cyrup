//! The streaming decoder.

use super::*;

#[tokio::test]
async fn decodes_full_text_and_toolcall_stream() {
    // A scripted Responses SSE stream: text item + function_call item, then completed.
    let raw = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hello\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\" world\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello world\"}]}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{\\\"x\\\":1}\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.done\",\"output_index\":1,\"arguments\":\"{\\\"x\\\":1}\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{\\\"x\\\":1}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":3}}}}\n\n",
    );
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let (sink, rx) = crate::api::channel(64);
    let m = model();
    let api = ApiId::from(API_ID);
    decode_stream(frames, &m, &api, &sink).await;
    drop(sink);
    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
    let msg = collect_message(stream).await;

    // Tool call present => toolUse terminal.
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.response_id.as_deref(), Some("resp_1"));
    // Usage: cached subtracted from input; reasoning carried.
    assert_eq!(msg.usage.input, 8);
    assert_eq!(msg.usage.output, 5);
    assert_eq!(msg.usage.cache_read, 2);
    assert_eq!(msg.usage.reasoning, Some(3));
    // Cost applied (input 8/1e6 * 1.0 + output 5/1e6*2.0 + cacheRead 2/1e6*0.5).
    assert!(msg.usage.cost.total > 0.0);
    // Content: text "Hello world" + tool call echo({"x":1}).
    let text = msg.content.iter().find_map(|c| match c {
        Content::Text { text, .. } => Some(text.to_string()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("Hello world"));
    let tc = msg.content.iter().find_map(|c| match c {
        Content::ToolCall(tc) => Some(tc.clone()),
        _ => None,
    });
    let tc = tc.expect("tool call");
    assert_eq!(tc.name, "echo");
    assert_eq!(tc.id.as_str(), "call_1|fc_1");
    assert_eq!(tc.arguments.get("x"), Some(&json!(1)));
}

#[tokio::test]
async fn missing_terminal_event_is_an_error() {
    let raw = "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\"}}\n\n";
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let (sink, rx) = crate::api::channel(64);
    let m = model();
    let api = ApiId::from(API_ID);
    decode_stream(frames, &m, &api, &sink).await;
    drop(sink);
    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
    let msg = collect_message(stream).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert!(
        msg.error_message
            .unwrap()
            .contains("terminal response event")
    );
}

#[tokio::test]
async fn response_failed_emits_error() {
    let raw = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\"}}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit\",\"message\":\"slow down\"}}}\n\n",
    );
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let (sink, rx) = crate::api::channel(64);
    let m = model();
    let api = ApiId::from(API_ID);
    decode_stream(frames, &m, &api, &sink).await;
    drop(sink);
    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
    let msg = collect_message(stream).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert!(msg.error_message.unwrap().contains("rate_limit: slow down"));
    // `output.rawStopReason = event.response?.status` (v0.84.1
    // openai-responses-shared.ts:726) — absent status stays absent.
    assert_eq!(msg.raw_stop_reason, None);
}

/// PERF-001 regression. `response.function_call_arguments.done` REPLACES the tool block's
/// accumulated buffer with the provider's authoritative `arguments` string, and the `partial`
/// emitted alongside that frame must project the POST-`done` arguments.
///
/// The defect this guards against was a second copy of the argument state — an incremental parser
/// fed only the deltas — that the wholesale replacement left describing a buffer that no longer
/// existed, so the snapshot projected the PRE-`done` arguments: a silent wrong answer on three
/// wires (`openai_responses`, `azure_openai_responses` and `openai_codex_responses` all share this
/// `decode_stream`). The block now holds the buffer and nothing derived from it, so the states
/// cannot diverge; this test keeps that property nailed down.
///
/// Both shapes the frame takes are exercised. The first — `arguments` exactly equal to the
/// concatenated deltas — is what the pre-existing fixture sends, and it cannot catch the defect:
/// `strip_prefix` yields `""`, the `filter` drops it, and no event is emitted through the stale
/// state at all. The second — `arguments` carrying a member the deltas never sent — is the one
/// that fails when the two states can drift apart.
#[tokio::test]
async fn toolcall_arguments_done_reprojects_the_replaced_buffer() {
    async fn tool_call_partials(deltas: &[&str], done_arguments: &str) -> Vec<AssistantMessage> {
        let mut raw = String::from(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n\
             data: {\"type\":\"response.output_item.added\",\"output_index\":0,\
             \"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\
             \"name\":\"write\",\"arguments\":\"\"}}\n\n",
        );
        for d in deltas {
            let esc = serde_json::to_string(d).unwrap();
            raw.push_str(&format!(
                "data: {{\"type\":\"response.function_call_arguments.delta\",\
                 \"output_index\":0,\"delta\":{esc}}}\n\n"
            ));
        }
        let done_esc = serde_json::to_string(done_arguments).unwrap();
        raw.push_str(&format!(
            "data: {{\"type\":\"response.function_call_arguments.done\",\
             \"output_index\":0,\"arguments\":{done_esc}}}\n\n"
        ));
        raw.push_str(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\
             \"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\
             \"total_tokens\":2}}}\n\n",
        );

        // Sized above the event count and drained AFTER the decode: this harness's usual
        // `channel(64)` with an inline `decode_stream` await deadlocks once the channel fills.
        let (sink, mut rx) = crate::api::channel(1024);
        let frames = decode_sse_bytes(raw.into_bytes());
        let m = model();
        let api = ApiId::from(API_ID);
        decode_stream(frames, &m, &api, &sink).await;
        drop(sink);
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let crate::stream::StreamEvent::ToolCallDelta { partial, .. } = ev {
                out.push((*partial).clone());
            }
        }
        out
    }

    // Shape 1: `arguments` equals the concatenated deltas. The `done` frame emits nothing (the
    // remainder is empty), so only the delta frames' partials exist — all of them correct.
    let partials = tool_call_partials(&[r#"{"path":"a.txt","#, r#""content":"hi"}"#], r#"{"path":"a.txt","content":"hi"}"#).await;
    let last = partials.last().expect("at least one toolcall_delta partial");
    let args = match last.content.first() {
        Some(Content::ToolCall(tc)) => tc.arguments.clone(),
        other => panic!("expected a tool call, got {other:?}"),
    };
    assert_eq!(
        Value::Object(args.into()),
        serde_json::json!({"path": "a.txt", "content": "hi"}),
        "identical `arguments` must round-trip"
    );

    // Shape 2: `arguments` carries a member the deltas never sent, so the `done` frame DOES emit a
    // `ToolCallDelta`. Its partial must reflect the authoritative buffer, not the accumulated one.
    let full = r#"{"path":"a.txt","content":"hi","mode":"append"}"#;
    let partials = tool_call_partials(&[r#"{"path":"a.txt","#, r#""content":"hi""#], full).await;
    let last = partials.last().expect("the done frame must emit a toolcall_delta");
    let args = match last.content.first() {
        Some(Content::ToolCall(tc)) => tc.arguments.clone(),
        other => panic!("expected a tool call, got {other:?}"),
    };
    assert_eq!(
        *args,
        crate::parse_streaming_json_object(Some(full)),
        "the partial emitted with `arguments.done` must project the REPLACED buffer, not the \
         deltas that preceded it"
    );
}
