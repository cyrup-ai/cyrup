//! Message conversion and tool config.

use super::*;

#[test]
fn blank_user_content_becomes_the_empty_placeholder() {
    let ctx = user_ctx("   ");
    let body = payload(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(
        messages_of(&body)[0]["content"],
        json!([{ "text": EMPTY_TEXT_PLACEHOLDER }])
    );
}

#[test]
fn blank_user_text_blocks_are_filtered_when_other_content_remains() {
    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text(""), Content::text("hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let body = payload(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(
        messages_of(&body)[0]["content"],
        json!([{ "text": "hello" }])
    );
}

#[test]
fn an_assistant_turn_whose_blocks_all_filter_out_is_dropped() {
    let assistant = AssistantMessage {
        content: vec![Content::text("   ")],
        provider: "amazon-bedrock".into(),
        model: "m".to_string(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    };
    let ctx = Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::Assistant(assistant),
        ],
        tools: Vec::new(),
    };
    let body = payload(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(messages_of(&body).len(), 1);
    assert_eq!(messages_of(&body)[0]["role"], json!("user"));
}

#[test]
fn blank_tool_result_content_becomes_the_empty_placeholder() {
    let ctx = Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: ToolCallId::from("tool-1"),
                    name: "tool".to_string(),
                    arguments: Map::new().into(),
                    thought_signature: None,
                })],
                provider: "amazon-bedrock".into(),
                model: "m".to_string(),
                api: API_ID.into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                timestamp: 0,
            }),
            Message::ToolResult {
                tool_call_id: ToolCallId::from("tool-1"),
                tool_name: "tool".to_string(),
                content: vec![Content::text("")],
                is_error: false,
                details: None,
                usage: None,
                added_tool_names: Vec::new(),
                timestamp: 0,
            },
        ],
        tools: Vec::new(),
    };
    let body = payload(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    let last = messages_of(&body).last().unwrap();
    assert_eq!(
        last["content"][0]["toolResult"]["content"],
        json!([{ "text": EMPTY_TEXT_PLACEHOLDER }])
    );
    assert_eq!(last["content"][0]["toolResult"]["status"], json!("success"));
}

/// pi `:867-903`: a RUN of consecutive tool results collapses into ONE user message.
#[test]
fn consecutive_tool_results_collapse_into_one_user_message() {
    let calls = ["a", "b", "c"];
    let mut messages = vec![
        Message::User {
            content: vec![Content::text("hi")],
            timestamp: 0,
        },
        Message::Assistant(AssistantMessage {
            content: calls
                .iter()
                .map(|id| {
                    Content::ToolCall(ToolCall {
                        id: ToolCallId::from(*id),
                        name: "tool".to_string(),
                        arguments: Map::new().into(),
                        thought_signature: None,
                    })
                })
                .collect(),
            provider: "amazon-bedrock".into(),
            model: "m".to_string(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        }),
    ];
    for id in calls {
        messages.push(Message::ToolResult {
            tool_call_id: ToolCallId::from(id),
            tool_name: "tool".to_string(),
            content: vec![Content::text(format!("result {id}"))],
            is_error: id == "b",
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 0,
        });
    }
    let ctx = Context {
        system_prompt: None,
        messages,
        tools: Vec::new(),
    };
    let body = payload(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    // user, assistant, ONE user carrying all three results.
    assert_eq!(messages_of(&body).len(), 3);
    let last = messages_of(&body).last().unwrap();
    assert_eq!(last["role"], json!("user"));
    let results = last["content"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[1]["toolResult"]["status"], json!("error"));
    assert_eq!(results[0]["toolResult"]["status"], json!("success"));
}

/// pi `:830-843`: a thinking block replayed without a signature degrades to plain text on
/// Claude models — Bedrock rejects a signature-less reasoning block.
#[test]
fn signatureless_thinking_replays_as_text_on_claude_and_as_reasoning_elsewhere() {
    let thinking = |sig: Option<&str>| Content::Thinking {
        thinking: "ponder".into(),
        thinking_signature: sig.map(str::to_string),
        redacted: false,
    };
    // The assistant turn must claim the SAME model the request targets: `transformMessages`
    // (`transform-messages.ts`) drops cross-model thinking before the converter ever sees it,
    // which would make this test about that transform rather than about `convertMessages`.
    let ctx = |model: &Model, content: Vec<Content>| Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::Assistant(AssistantMessage {
                content,
                provider: model.provider.clone(),
                model: model.id.as_str().to_string(),
                api: API_ID.into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                timestamp: 0,
            }),
        ],
        tools: Vec::new(),
    };

    let claude = sonnet_45();
    let body = payload(
        &claude,
        &ctx(&claude, vec![thinking(None)]),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(
        messages_of(&body)[1]["content"],
        json!([{ "text": "ponder" }])
    );

    let body = payload(
        &claude,
        &ctx(&claude, vec![thinking(Some("sig"))]),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(
        messages_of(&body)[1]["content"],
        json!([{ "reasoningContent": { "reasoningText": { "text": "ponder", "signature": "sig" } } }])
    );

    // MIRROR: a non-Claude model never sends the signature field at all (`:844-850`).
    let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
    let body = payload(
        &nova,
        &ctx(&nova, vec![thinking(Some("sig"))]),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(
        messages_of(&body)[1]["content"],
        json!([{ "reasoningContent": { "reasoningText": { "text": "ponder" } } }])
    );
}

#[test]
fn an_unknown_image_type_is_the_ported_error_string() {
    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::Image {
                data: "AAAA".to_string(),
                mime_type: "image/tiff".to_string(),
            }],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let ambient = ProviderEnv::new();
    let err = build_params(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::None,
        &env_source(None, &ambient),
    )
    .unwrap_err();
    assert_eq!(err, "Unknown image type: image/tiff");
}

#[test]
fn tool_call_ids_are_sanitized_and_capped_at_64_chars() {
    assert_eq!(normalize_tool_call_id("call:abc/def"), "call_abc_def");
    assert_eq!(normalize_tool_call_id(&"x".repeat(80)).len(), 64);
    assert_eq!(normalize_tool_call_id("keep-_09AZ"), "keep-_09AZ");
}

fn tool_ctx() -> Context {
    Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text("Use it")],
            timestamp: 0,
        }],
        tools: vec![ToolDef {
            name: "lookup".to_string(),
            description: "Look up a value".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            constrained_sampling: None,
        }],
    }
}

#[test]
fn tool_config_shape_and_choice_mapping() {
    let base = payload(
        &sonnet_45(),
        &tool_ctx(),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(
        base["toolConfig"]["tools"][0]["toolSpec"]["name"],
        json!("lookup")
    );
    assert_eq!(
        base["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["type"],
        json!("object")
    );
    // No choice configured ⇒ no `toolChoice` key at all.
    assert!(base["toolConfig"].get("toolChoice").is_none());

    for (choice, wire) in [
        (BedrockToolChoice::Auto, json!({ "auto": {} })),
        (BedrockToolChoice::Any, json!({ "any": {} })),
        (
            BedrockToolChoice::Tool {
                name: "lookup".to_string(),
            },
            json!({ "tool": { "name": "lookup" } }),
        ),
    ] {
        let bedrock = BedrockOptions {
            tool_choice: Some(choice),
            ..Default::default()
        };
        let body = payload(
            &sonnet_45(),
            &tool_ctx(),
            &StreamOptions::default(),
            &bedrock,
        );
        assert_eq!(body["toolConfig"]["toolChoice"], wire);
    }

    // `"none"` drops the whole toolConfig (pi `:931`).
    let bedrock = BedrockOptions {
        tool_choice: Some(BedrockToolChoice::None),
        ..Default::default()
    };
    let body = payload(
        &sonnet_45(),
        &tool_ctx(),
        &StreamOptions::default(),
        &bedrock,
    );
    assert!(body.get("toolConfig").is_none());

    // No tools ⇒ no toolConfig.
    let body = payload(
        &sonnet_45(),
        &user_ctx("hi"),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert!(body.get("toolConfig").is_none());
}

/// PROV-097 — `sanitizeBedrockDocument` over `toolUse.input` (pi `:921-934`, called at `:1008`
/// @v0.87.1) and the `redactedContent` replay of an encrypted thinking block (pi `:1010-1019`).
mod prov097_replay {
    use super::*;
    use base64::Engine as _;

    /// The assistant's `model` must match the request's, or `transform_messages_with`
    /// (`openai_completions::transform`, pi `transformMessages`) drops a redacted thinking block as
    /// foreign history before the Bedrock converter ever sees it.
    fn assistant(content: Vec<Content>) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            provider: "amazon-bedrock".into(),
            model: sonnet_45().id.as_str().to_string(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 1,
        })
    }

    fn assistant_blocks(content: Vec<Content>) -> Vec<Value> {
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("hi")],
                    timestamp: 0,
                },
                assistant(content),
            ],
            tools: Vec::new(),
        };
        let body = payload(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        messages_of(&body)
            .iter()
            .find(|m| m["role"] == "assistant")
            .and_then(|m| m["content"].as_array())
            .cloned()
            .unwrap_or_default()
    }

    /// (i) Zero-length keys are dropped at EVERY depth, including inside arrays; every other key
    /// survives untouched. Bedrock's `DocumentType` rejects an empty key, so replaying the
    /// arguments verbatim failed the whole request.
    #[test]
    fn empty_object_keys_are_stripped_from_tool_use_input_at_every_depth() {
        let args: Map<String, Value> = json!({
            "": 1,
            "ok": { "": 2, "keep": [ { "": 3, "deep": 4 } ] }
        })
        .as_object()
        .cloned()
        .unwrap();
        let blocks = assistant_blocks(vec![Content::ToolCall(ToolCall {
            id: ToolCallId::from("t1"),
            name: "tool".to_string(),
            arguments: args.into(),
            thought_signature: None,
        })]);
        assert_eq!(
            blocks[0]["toolUse"]["input"],
            json!({ "ok": { "keep": [ { "deep": 4 } ] } })
        );
    }

    /// (ii) A redacted thinking block is replayed as `reasoningContent.redactedContent`, never as
    /// `reasoningText` and never degraded to plain `text`.
    #[test]
    fn a_redacted_thinking_block_replays_as_redacted_content() {
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(b"opaque");
        let blocks = assistant_blocks(vec![Content::Thinking {
            thinking: "[Reasoning redacted]".into(),
            thinking_signature: Some(payload_b64.clone()),
            redacted: true,
        }]);
        assert_eq!(
            blocks,
            vec![json!({ "reasoningContent": { "redactedContent": payload_b64 } })]
        );
    }

    /// The same block with no stored payload is DROPPED (pi's `if (redactedContent?.length)`), and
    /// the assistant message therefore contributes nothing — not a `text` fallback.
    #[test]
    fn a_redacted_thinking_block_without_a_payload_is_dropped() {
        for signature in [None, Some("not base64 at all !!".to_string())] {
            let blocks = assistant_blocks(vec![Content::Thinking {
                thinking: "[Reasoning redacted]".into(),
                thinking_signature: signature.clone(),
                redacted: true,
            }]);
            assert!(blocks.is_empty(), "signature {signature:?} gave {blocks:?}");
        }
    }
}
