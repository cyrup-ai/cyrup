//! Streaming decode of a framed response.

use super::*;

#[tokio::test]
async fn decodes_text_thinking_and_tool_use_in_upstream_order() {
    let model = sonnet_45();
    let chunks = vec![
        event("messageStart", "{\"role\":\"assistant\"}"),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":0,\"delta\":{\"reasoningContent\":{\"text\":\"think\"}}}",
        ),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":0,\"delta\":{\"reasoningContent\":{\"signature\":\"sig\"}}}",
        ),
        event("contentBlockStop", "{\"contentBlockIndex\":0}"),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":1,\"delta\":{\"text\":\"Hel\"}}",
        ),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":1,\"delta\":{\"text\":\"lo\"}}",
        ),
        event("contentBlockStop", "{\"contentBlockIndex\":1}"),
        event(
            "contentBlockStart",
            "{\"contentBlockIndex\":2,\"start\":{\"toolUse\":{\"toolUseId\":\"t1\",\"name\":\"lookup\"}}}",
        ),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":2,\"delta\":{\"toolUse\":{\"input\":\"{\\\"q\\\":\"}}}",
        ),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":2,\"delta\":{\"toolUse\":{\"input\":\"1}\"}}}",
        ),
        event("contentBlockStop", "{\"contentBlockIndex\":2}"),
        event(
            "metadata",
            "{\"usage\":{\"inputTokens\":10,\"outputTokens\":5,\"cacheReadInputTokens\":2,\"cacheWriteInputTokens\":1,\"totalTokens\":18}}",
        ),
        event("messageStop", "{\"stopReason\":\"tool_use\"}"),
    ];

    let events = collect(chunks, &model).await;
    assert_eq!(
        kinds(&events),
        vec![
            "start",
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_delta",
            "toolcall_end",
            "done",
        ]
    );

    let StreamEvent::Done { message, .. } = events.last().unwrap() else {
        panic!("expected a done terminal");
    };
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(message.usage.input, 10);
    assert_eq!(message.usage.output, 5);
    assert_eq!(message.usage.cache_read, 2);
    assert_eq!(message.usage.cache_write, 1);
    // The provider's own `totalTokens` is preserved, not recomputed (pi `:542`).
    assert_eq!(message.usage.total_tokens, 18);
    // 10 in @ $3/1e6 + 5 out @ $15/1e6 + 2 cacheRead @ $0.3/1e6 + 1 cacheWrite @ $3.75/1e6.
    let expected = 10.0 * 3.0 / 1e6 + 5.0 * 15.0 / 1e6 + 2.0 * 0.3 / 1e6 + 3.75 / 1e6;
    assert!(message.usage.cost.total > 0.0);
    assert!((message.usage.cost.total - expected).abs() < 1e-12);

    assert_eq!(message.content.len(), 3);
    match &message.content[0] {
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => {
            assert_eq!(thinking, "think");
            assert_eq!(thinking_signature.as_deref(), Some("sig"));
        }
        other => panic!("expected thinking, got {other:?}"),
    }
    match &message.content[2] {
        Content::ToolCall(tc) => {
            assert_eq!(tc.name, "lookup");
            assert_eq!(tc.id.as_str(), "t1");
            assert_eq!(tc.arguments.get("q"), Some(&json!(1)));
        }
        other => panic!("expected a tool call, got {other:?}"),
    }
}
