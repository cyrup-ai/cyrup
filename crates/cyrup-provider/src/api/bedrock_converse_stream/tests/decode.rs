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

/// PROV-081 — pi #9457 (`bedrock-converse-stream.ts:712-715` @v0.87.1): the one-hour share of the
/// cache writes is summed from `usage.cacheDetails[]` into `cache_write_1h`, so it is priced at 2x
/// input instead of the five-minute `cacheWrite` rate. Without `cacheDetails` it stays unset.
#[tokio::test]
async fn one_hour_cache_writes_are_read_from_cache_details() {
    let model = sonnet_45();
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event(
                "metadata",
                "{\"usage\":{\"inputTokens\":10,\"outputTokens\":5,\"cacheWriteInputTokens\":1500,\"totalTokens\":1515,\"cacheDetails\":[{\"ttl\":\"1h\",\"inputTokens\":1000},{\"ttl\":\"5m\",\"inputTokens\":500}]}}",
            ),
            event("messageStop", "{\"stopReason\":\"end_turn\"}"),
        ],
        &model,
    )
    .await;
    let StreamEvent::Done { message, .. } = events.last().unwrap() else {
        panic!("expected a done terminal");
    };
    assert_eq!(message.usage.cache_write, 1500);
    assert_eq!(message.usage.cache_write_1h, Some(1000));
    // 500 short @ $3.75/1e6 + 1000 long @ 2 x $3/1e6.
    let expected = (500.0 * 3.75 + 1000.0 * 3.0 * 2.0) / 1e6;
    assert!((message.usage.cost.cache_write - expected).abs() < 1e-12);

    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event(
                "metadata",
                "{\"usage\":{\"inputTokens\":10,\"outputTokens\":5,\"cacheWriteInputTokens\":1500}}",
            ),
            event("messageStop", "{\"stopReason\":\"end_turn\"}"),
        ],
        &model,
    )
    .await;
    let StreamEvent::Done { message, .. } = events.last().unwrap() else {
        panic!("expected a done terminal");
    };
    assert_eq!(message.usage.cache_write_1h, None);
}

/// PROV-097 — encrypted reasoning (`reasoningContent.redactedContent`) from a non-Anthropic model on
/// Bedrock. pi `bedrock-converse-stream.ts:652-675` + `:678-690` @v0.87.1. Translated from
/// `packages/ai/test/bedrock-redacted-reasoning.test.ts` @v0.87.1.
mod prov097_redacted_reasoning {
    use super::*;
    use base64::Engine as _;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn thinking_of(events: &[StreamEvent]) -> Content {
        let msg = events
            .iter()
            .find_map(StreamEvent::terminal_message)
            .expect("terminal");
        msg.content
            .iter()
            .find(|c| matches!(c, Content::Thinking { .. }))
            .cloned()
            .expect("a thinking block")
    }

    /// Two `redactedContent` chunks, no `text`/`signature`: the block exists, carries the
    /// placeholder, is flagged `redacted`, and its signature is the base64 of the JOINED bytes.
    /// The two chunks are 4 and 2 bytes, so neither is a multiple of three — concatenating their
    /// own base64 strings would give the wrong answer, which is why the bytes are buffered.
    #[tokio::test]
    async fn redacted_content_becomes_one_placeholder_thinking_block() {
        let first: &[u8] = b"abcd";
        let second: &[u8] = b"ef";
        let chunks = vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event(
                "contentBlockDelta",
                &format!(
                    "{{\"contentBlockIndex\":0,\"delta\":{{\"reasoningContent\":{{\"redactedContent\":\"{}\"}}}}}}",
                    b64(first)
                ),
            ),
            event(
                "contentBlockDelta",
                &format!(
                    "{{\"contentBlockIndex\":0,\"delta\":{{\"reasoningContent\":{{\"redactedContent\":\"{}\"}}}}}}",
                    b64(second)
                ),
            ),
            event("contentBlockStop", "{\"contentBlockIndex\":0}"),
            event("messageStop", "{\"stopReason\":\"end_turn\"}"),
        ];
        let events = collect(chunks, &sonnet_45()).await;

        assert_eq!(
            thinking_of(&events),
            Content::Thinking {
                thinking: "[Reasoning redacted]".into(),
                thinking_signature: Some(b64(b"abcdef")),
                redacted: true,
            }
        );
        // Exactly ONE placeholder delta, from the FIRST chunk only (pi's `if (!redacted)` guard).
        let placeholders: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(placeholders, vec!["[Reasoning redacted]"]);
    }

    /// A `signature` arriving AFTER `redactedContent` is NOT appended: the field holds an Anthropic
    /// signature or an opaque redacted payload, never both (pi `:655-657`).
    #[tokio::test]
    async fn a_signature_after_redacted_content_is_not_appended() {
        let chunks = vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event(
                "contentBlockDelta",
                &format!(
                    "{{\"contentBlockIndex\":0,\"delta\":{{\"reasoningContent\":{{\"redactedContent\":\"{}\"}}}}}}",
                    b64(b"opaque")
                ),
            ),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":0,\"delta\":{\"reasoningContent\":{\"signature\":\"sig\"}}}",
            ),
            event("contentBlockStop", "{\"contentBlockIndex\":0}"),
            event("messageStop", "{\"stopReason\":\"end_turn\"}"),
        ];
        let events = collect(chunks, &sonnet_45()).await;
        let Content::Thinking {
            thinking_signature, ..
        } = thinking_of(&events)
        else {
            panic!("thinking");
        };
        assert_eq!(thinking_signature, Some(b64(b"opaque")));
    }

    /// Regression guard: the ordinary Anthropic-on-Bedrock path (`text` + `signature`) must stay
    /// exactly as it was — `redacted: false` and the signature verbatim, not base64 of anything.
    #[tokio::test]
    async fn the_plain_text_and_signature_path_is_unchanged() {
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
            event("messageStop", "{\"stopReason\":\"end_turn\"}"),
        ];
        let events = collect(chunks, &sonnet_45()).await;
        assert_eq!(
            thinking_of(&events),
            Content::Thinking {
                thinking: "think".into(),
                thinking_signature: Some("sig".to_string()),
                redacted: false,
            }
        );
    }
}
