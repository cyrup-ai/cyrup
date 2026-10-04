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
    let partials = tool_call_partials(
        &[r#"{"path":"a.txt","#, r#""content":"hi"}"#],
        r#"{"path":"a.txt","content":"hi"}"#,
    )
    .await;
    let last = partials
        .last()
        .expect("at least one toolcall_delta partial");
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
    let last = partials
        .last()
        .expect("the done frame must emit a toolcall_delta");
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

/// Decode a scripted stream and return the first tool call of the final message plus the
/// `ToolCallEnd` event's tool call.
async fn decode_tool_call(raw: &str) -> (ToolCall, ToolCall) {
    let (sink, mut rx) = crate::api::channel(1024);
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let m = model();
    let api = ApiId::from(API_ID);
    decode_stream(frames, &m, &api, &sink).await;
    drop(sink);
    let mut ended = None;
    let mut last_partial = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            crate::stream::StreamEvent::ToolCallEnd { tool_call, .. } => ended = Some(tool_call),
            crate::stream::StreamEvent::Done { message, .. } => last_partial = Some(message),
            _ => {}
        }
    }
    let message = last_partial.expect("a terminal Done event");
    let final_call = message
        .content
        .iter()
        .find_map(|c| match c {
            Content::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        })
        .expect("a tool call in the final message");
    (final_call, ended.expect("a ToolCallEnd event"))
}

const COMPLETED: &str = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"status\":\"completed\"}}\n\n";

#[tokio::test]
async fn a_namespace_received_only_on_output_item_done_is_recorded() {
    // Pi `openai-responses-namespace.test.ts` "round-trips a function namespace received only on
    // output_item.done" (`openai-responses-shared.ts:715`).
    let raw = format!(
        "{}{}{COMPLETED}",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"{\\\"value\\\":\\\"hello\\\"}\",\"namespace\":\"dynamic_tools\"}}\n\n",
    );
    let (tc, ended) = decode_tool_call(&raw).await;
    assert_eq!(tc.id.as_str(), "call_test|fc_test");
    assert_eq!(tc.name, "lookup");
    assert_eq!(tc.arguments.get("value"), Some(&json!("hello")));
    assert_eq!(tc.namespace.as_deref(), Some("dynamic_tools"));
    assert_eq!(
        ended.namespace.as_deref(),
        Some("dynamic_tools"),
        "the streamed ToolCallEnd block carries it too"
    );
}

#[tokio::test]
async fn a_namespace_received_on_output_item_added_is_recorded_and_done_may_override_it() {
    // `openai-responses-shared.ts:491`: read when the block starts. `:715`: replaced when the
    // done item names one.
    let added = "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"\",\"namespace\":\"early\"}}\n\n";
    let done_without = "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"{}\"}}\n\n";
    let done_with = "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"{}\",\"namespace\":\"late\"}}\n\n";
    let (tc, _) = decode_tool_call(&format!("{added}{done_without}{COMPLETED}")).await;
    assert_eq!(tc.namespace.as_deref(), Some("early"));
    let (tc, _) = decode_tool_call(&format!("{added}{done_with}{COMPLETED}")).await;
    assert_eq!(tc.namespace.as_deref(), Some("late"));
}

#[tokio::test]
async fn an_ordinary_function_call_has_no_namespace() {
    let raw = format!(
        "{}{}{COMPLETED}",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"name\":\"lookup\",\"arguments\":\"{}\"}}\n\n",
    );
    let (tc, ended) = decode_tool_call(&raw).await;
    assert_eq!(tc.namespace, None);
    assert_eq!(ended.namespace, None);
}

/// PROV-116 / `1b2aa0ca0` ("reject unfinished Responses tool calls instead of running them",
/// #9974) — a stream that reaches `response.completed` with a tool call whose
/// `response.output_item.done` never arrived is an **error**, not a `toolUse` terminal the agent
/// would execute (`openai-responses-shared.ts:764-775`). The message is pi's verbatim.
#[tokio::test]
async fn prov116_a_completed_stream_with_an_unfinished_tool_call_is_an_error() {
    let raw = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"bash\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"rm -r\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(
        msg.error_message.as_deref(),
        Some("OpenAI Responses stream completed with an unfinished tool call: bash (call_1|fc_1)")
    );
}

/// PROV-116 — the concrete failure `1b2aa0ca0`'s commit message describes: a non-compliant server
/// that **omits `output_index`**, so two parallel calls collide on one slot key. `create_slot`
/// overwrites `slots[0]`, orphaning the first block, and the second call's `output_item.done`
/// then clears the slot — so at end-of-stream the slot table is empty while an unfinished call
/// sits in the content. That is why the marker has to be per-block (upstream's deleted
/// `partialJson` scratch field, ported here as `RBlock::Tool::finished`) and not per-slot.
///
/// Note that the orphaned call's argument buffer is also **empty** here: it never received a
/// delta. A "non-empty `partial_json`" marker would miss it entirely.
#[tokio::test]
async fn prov116_a_server_omitting_output_index_cannot_smuggle_an_orphaned_call_through() {
    let raw = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        // Two parallel calls, both with no `output_index` at all.
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_2\",\"name\":\"echo\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\\\"x\\\":\\\"b\\\"}\"}\n\n",
        // Only the SECOND call is finished.
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_2\",\"name\":\"echo\",\"arguments\":\"{\\\"x\\\":\\\"b\\\"}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    // The FIRST offender in content order is named, as upstream's `for (const block of
    // output.content)` throw does.
    assert_eq!(
        msg.error_message.as_deref(),
        Some("OpenAI Responses stream completed with an unfinished tool call: echo (call_1|fc_1)")
    );
}

/// PROV-116 ordering — upstream raises the unfinished-call error **after** the terminal-event
/// check (`openai-responses-shared.ts:760-775`), so a stream that was cut before
/// `response.completed` still reports the truncation. The unfinished call is present here, which
/// is what makes the ordering observable.
#[tokio::test]
async fn prov116_a_truncated_stream_reports_truncation_not_the_unfinished_call() {
    let raw = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"bash\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"ls\"}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    let err = msg.error_message.unwrap();
    assert!(err.contains("terminal response event"), "{err}");
    assert!(!err.contains("unfinished tool call"), "{err}");
}

/// PROV-116 negative control — a stream whose terminal stop reason is **not** `toolUse` is left
/// alone (`if (output.stopReason === "toolUse")`, `openai-responses-shared.ts:764`). A reasoning
/// item never carries the marker, and an `incomplete`/`length` terminal with a *finished* call is
/// not rewritten either.
#[tokio::test]
async fn prov116_the_guard_only_applies_to_a_tool_use_terminal() {
    // A finished call plus a `max_output_tokens` incomplete: `length`, not `toolUse`, and the
    // call is finished anyway — nothing to report.
    let raw = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{}\"}}\n\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Length);
    assert!(msg.error_message.is_none());

    // Plain text, no tool blocks at all: a clean `stop`.
    let raw = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"completed\"}}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Stop);
}
