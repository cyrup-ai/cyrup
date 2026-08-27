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
        Content::Text { text, .. } => Some(text.clone()),
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
