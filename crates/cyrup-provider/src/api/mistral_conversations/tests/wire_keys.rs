//! PROV-152 — the keys on Mistral's wire.
//!
//! pi dropped the `@mistralai/mistralai` SDK in `9dd90a497` (v1.0.1). The SDK's TypeScript surface
//! is camelCase (`maxTokens`, `toolCalls`, `finishReason`, `promptTokens`) and its generated zod
//! schemas remapped those names to Mistral's snake_case on the way out and back in; once pi spoke
//! HTTP itself it had to remap them itself — `toMistralWirePayload` / `toMistralWireMessage` /
//! `toMistralWireContentChunk` on the request (`mistral-conversations.ts:386-451` @f1b2e77f5) and
//! `chunk.usage.prompt_tokens` / `choice.finish_reason` / `delta.tool_calls` on the stream
//! (`:616-633`, `:710`). Mistral's published OpenAPI (`docs.mistral.ai/openapi.yaml`) declares
//! `ChatCompletionRequest`, `AssistantMessage`, `ToolMessage` and `ImageURLChunk` as
//! `additionalProperties: false` with a documented `422` validation error, so a camelCase key on
//! the request is a rejected turn, not a no-op; on the response it is a field that is never there.
//!
//! The two directions fail differently, so they are tested differently: the request side asserts
//! the bytes that actually leave `ApiImpl::run` (captured off a loopback socket, AFTER the
//! `before_provider_request` hook), the response side decodes a recorded-shape snake_case stream.
//! Fixtures are ported from pi's `test/mistral-http-transport.test.ts` and
//! `test/mistral-raw-stop-reason.test.ts` @f1b2e77f5.

use super::*;
use crate::utils::retry::is_retryable_assistant_error;
use cyrup_core::{AssistantMessage, ToolCall, Usage};
use serde_json::Map;

/// Every camelCase name pi's `toMistralWire*` functions rename, plus the request-message and
/// content-chunk ones. None of them may survive onto Mistral's wire at the levels pi remaps.
const CAMEL_PAYLOAD_KEYS: [&str; 12] = [
    "topP",
    "maxTokens",
    "randomSeed",
    "responseFormat",
    "toolChoice",
    "presencePenalty",
    "frequencyPenalty",
    "parallelToolCalls",
    "reasoningEffort",
    "promptMode",
    "promptCacheKey",
    "safePrompt",
];
const CAMEL_MESSAGE_KEYS: [&str; 2] = ["toolCalls", "toolCallId"];
const CAMEL_CHUNK_KEYS: [&str; 6] = [
    "imageUrl",
    "documentUrl",
    "documentName",
    "fileId",
    "referenceIds",
    "inputAudio",
];

/// pi `createTerminalEvent()` (`mistral-http-transport.test.ts`), the response every request
/// test is answered with.
const TERMINAL_SSE: &str = "data: {\"id\":\"mistral-response-id\",\"model\":\"mistral-large-latest\",\"choices\":[{\"index\":0,\"finish_reason\":\"stop\",\"delta\":{}}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\r\n\r\ndata: [DONE]\r\n\r\n";

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Serve one canned SSE response off loopback and hand back the JSON body of the request it
/// received — the bytes `ApiImpl::run` actually put on the wire.
async fn serve_once_capturing(
    sse: &'static str,
) -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut req = Vec::new();
        let mut buf = [0u8; 8192];
        let body = loop {
            let n = sock.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break Vec::new();
            }
            req.extend_from_slice(&buf[..n]);
            let Some(pos) = find_subslice(&req, b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&req[..pos]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .expect("the request carries a content-length");
            if req.len() >= pos + 4 + len {
                break req[pos + 4..pos + 4 + len].to_vec();
            }
        };
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            sse.len()
        );
        let _ = sock.write_all(head.as_bytes()).await;
        let _ = sock.write_all(sse.as_bytes()).await;
        let _ = sock.flush().await;
        let _ = tx.send(body);
    });
    (format!("http://{addr}"), rx)
}

/// Drive `ApiImpl::run` and return (the request body it sent, the events it emitted).
async fn run_capturing(
    m: &Model,
    ctx: &Context,
    opts: &StreamOptions,
) -> (Value, Vec<StreamEvent>) {
    let (base, rx) = serve_once_capturing(TERMINAL_SSE).await;
    let mut m = m.clone();
    m.base_url = base;
    let (sink, mut ev_rx) = channel(64);
    MistralConversationsApi::new()
        .run(
            &m,
            ctx,
            &auth_with("secret"),
            opts,
            CancelToken::new(),
            sink,
        )
        .await;
    let mut events = Vec::new();
    while let Some(ev) = ev_rx.recv().await {
        events.push(ev);
    }
    let body = rx.await.expect("the loopback server saw a request");
    (
        serde_json::from_slice(&body).expect("the request body is JSON"),
        events,
    )
}

/// No camelCase key at any level pi's `toMistralWire*` functions rename.
fn assert_no_camel_keys(body: &Value) {
    let top = body.as_object().expect("an object body");
    for k in CAMEL_PAYLOAD_KEYS {
        assert!(!top.contains_key(k), "camelCase `{k}` on the wire: {body}");
    }
    if let Some(rf) = top.get("response_format").and_then(Value::as_object) {
        assert!(!rf.contains_key("jsonSchema"), "{body}");
        if let Some(js) = rf.get("json_schema").and_then(Value::as_object) {
            assert!(!js.contains_key("schemaDefinition"), "{body}");
        }
    }
    for msg in top["messages"].as_array().expect("messages") {
        let msg = msg.as_object().expect("an object message");
        for k in CAMEL_MESSAGE_KEYS {
            assert!(!msg.contains_key(k), "camelCase `{k}` on a message: {body}");
        }
        for chunk in msg
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for k in CAMEL_CHUNK_KEYS {
                assert!(
                    chunk.get(k).is_none(),
                    "camelCase `{k}` on a content chunk: {body}"
                );
            }
        }
    }
}

fn lookup_tool() -> ToolDef {
    ToolDef {
        name: "lookup".to_string(),
        description: "Look something up".to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"],
        }),
        constrained_sampling: None,
    }
}

/// pi `"serializes SDK-style payloads to the Mistral wire format"`: every top-level option and a
/// user image chunk, including the keys an `onPayload` hook adds — the hook sees pi's SDK-style
/// (camelCase) payload, and the conversion runs after it.
#[tokio::test]
async fn prov152_request_body_is_mistral_snake_case_after_the_payload_hook() {
    let mut m = model_with("mistral-large-latest", false);
    m.input = vec![Modality::Text, Modality::Image];
    let ctx = Context {
        system_prompt: Some("Be precise".to_string()),
        messages: vec![Message::User {
            content: vec![
                Content::text("describe"),
                Content::Image {
                    data: "aGVsbG8=".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ],
            timestamp: 1,
        }],
        tools: vec![lookup_tool()],
    };
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let seen2 = seen.clone();
    let hook: crate::stream::OnPayload = std::sync::Arc::new(move |payload: Value, _m| {
        *seen2.lock().unwrap() = Some(payload.clone());
        let mut obj = payload.as_object().cloned().unwrap();
        for (k, v) in [
            ("topP", json!(0.9)),
            ("randomSeed", json!(42)),
            (
                "responseFormat",
                json!({
                    "type": "json_schema",
                    "jsonSchema": {
                        "name": "result",
                        "schemaDefinition": {
                            "type": "object",
                            "properties": { "maxTokens": { "type": "number" } },
                        },
                    },
                }),
            ),
            ("presencePenalty", json!(0.1)),
            ("frequencyPenalty", json!(0.2)),
            ("parallelToolCalls", json!(true)),
            ("safePrompt", json!(true)),
        ] {
            obj.insert(k.to_string(), v);
        }
        futures::FutureExt::boxed(async move { Some(Value::Object(obj)) })
    });
    let opts = StreamOptions {
        max_tokens: Some(123),
        tool_choice: Some(ToolChoice::Function {
            name: "lookup".to_string(),
        }),
        session_id: Some(SessionId::from("session-1")),
        api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
            prompt_mode: Some(MistralPromptMode::Reasoning),
            reasoning_effort: Some(MistralReasoningEffort::High),
        })),
        on_payload: Some(hook),
        ..Default::default()
    };

    let (wire, events) = run_capturing(&m, &ctx, &opts).await;

    // The hook saw pi's SDK-style payload (pi asserts `callbackPayload.maxTokens` etc.).
    let hooked = seen.lock().unwrap().clone().expect("the hook ran");
    assert_eq!(hooked["maxTokens"], 123);
    assert_eq!(hooked["promptMode"], "reasoning");
    assert_eq!(hooked["promptCacheKey"], "session-1");

    assert_eq!(wire["max_tokens"], 123);
    assert_eq!(wire["prompt_mode"], "reasoning");
    assert_eq!(wire["reasoning_effort"], "high");
    assert_eq!(
        wire["tool_choice"],
        json!({ "type": "function", "function": { "name": "lookup" } })
    );
    assert_eq!(wire["prompt_cache_key"], "session-1");
    assert_eq!(wire["top_p"], 0.9);
    assert_eq!(wire["random_seed"], 42);
    assert_eq!(wire["presence_penalty"], 0.1);
    assert_eq!(wire["frequency_penalty"], 0.2);
    assert_eq!(wire["parallel_tool_calls"], true);
    assert_eq!(wire["safe_prompt"], true);
    // A user schema's own `maxTokens` property is data, not a request key: pi does not touch it.
    assert_eq!(
        wire["response_format"],
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": "result",
                "schema": {
                    "type": "object",
                    "properties": { "maxTokens": { "type": "number" } },
                },
            },
        })
    );
    assert_no_camel_keys(&wire);
    assert_eq!(
        wire["messages"],
        json!([
            { "role": "system", "content": "Be precise" },
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe" },
                    { "type": "image_url", "image_url": "data:image/png;base64,aGVsbG8=" },
                ],
            },
        ])
    );
    // And the snake_case terminal pi answers with settles the turn cleanly.
    assert!(
        matches!(events.last(), Some(StreamEvent::Done { message, .. }) if message.stop_reason == StopReason::Stop),
        "{:?}",
        events.last()
    );
}

/// pi `"serializes assistant thinking, tool calls, and tool results for replay"`.
#[tokio::test]
async fn prov152_replayed_tool_calls_and_tool_results_use_mistral_wire_keys() {
    let mut m = model_with("mistral-large-latest", false);
    m.input = vec![Modality::Text, Modality::Image];
    let mut args = Map::new();
    args.insert("query".to_string(), json!("pi"));
    let ctx = Context {
        system_prompt: None,
        messages: vec![
            Message::Assistant(AssistantMessage {
                content: vec![
                    Content::thinking("reason"),
                    Content::text("answer"),
                    Content::ToolCall(ToolCall {
                        id: CoreToolCallId::from("abc123456"),
                        name: "lookup".to_string(),
                        arguments: args.into(),
                        thought_signature: None,
                        namespace: None,
                    }),
                ],
                provider: "mistral".into(),
                model: "mistral-large-latest".into(),
                api: API_ID.into(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                thinking_level: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 1,
                duration_ms: None,
            }),
            Message::ToolResult {
                duration_ms: None,
                tool_call_id: CoreToolCallId::from("abc123456"),
                tool_name: "lookup".to_string(),
                content: vec![
                    Content::text("found"),
                    Content::Image {
                        data: "aGVsbG8=".to_string(),
                        mime_type: "image/png".to_string(),
                    },
                ],
                is_error: false,
                details: None,
                timestamp: 2,
                usage: None,
                added_tool_names: Vec::new(),
                nested_calls: None,
            },
        ],
        tools: Vec::new(),
    };

    let (wire, _) = run_capturing(&m, &ctx, &StreamOptions::default()).await;

    assert_no_camel_keys(&wire);
    assert_eq!(
        wire["messages"],
        json!([
            {
                "role": "assistant",
                "prefix": false,
                "content": [
                    { "type": "thinking", "thinking": [{ "type": "text", "text": "reason" }] },
                    { "type": "text", "text": "answer" },
                ],
                "tool_calls": [
                    {
                        "id": "abc123456",
                        "type": "function",
                        "function": { "name": "lookup", "arguments": "{\"query\":\"pi\"}" },
                        "index": 0,
                    },
                ],
            },
            {
                "role": "tool",
                "tool_call_id": "abc123456",
                "name": "lookup",
                "content": [
                    { "type": "text", "text": "found" },
                    { "type": "image_url", "image_url": "data:image/png;base64,aGVsbG8=" },
                ],
            },
        ])
    );
}

/// pi `"parses native thinking, text, fragmented tool calls, and cached-token usage"`: the stream
/// in Mistral's own keys — `finish_reason`, `delta.tool_calls`, `usage.prompt_tokens` /
/// `completion_tokens` / `total_tokens` / `prompt_tokens_details.cached_tokens`.
#[tokio::test]
async fn prov152_a_snake_case_stream_yields_tool_calls_stop_reason_and_usage() {
    let raw = concat!(
        "data: {\"id\":\"response-1\",\"model\":\"mistral-large-latest\",\"choices\":[{\"index\":0,\"finish_reason\":null,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"reason\"}]}]}}]}\r\n\r\n",
        "data: {\"id\":\"response-1\",\"model\":\"mistral-large-latest\",\"choices\":[{\"index\":0,\"finish_reason\":null,\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"answer\"}]}}]}\r\n\r\n",
        "data: {\"id\":\"response-1\",\"model\":\"mistral-large-latest\",\"choices\":[{\"index\":0,\"finish_reason\":null,\"delta\":{\"tool_calls\":[{\"id\":\"abc123456\",\"index\":0,\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"query\\\":\"}}]}}]}\r\n\r\n",
        "data: {\"id\":\"response-1\",\"model\":\"mistral-large-latest\",\"choices\":[{\"index\":0,\"finish_reason\":\"tool_calls\",\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"\",\"arguments\":\"\\\"pi\\\"}\"}}]}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4,\"total_tokens\":14,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\r\n\r\n",
        "data: [DONE]\r\n\r\n",
    );
    let m = model_with("mistral-large-latest", false);
    let events = collect(raw.as_bytes().to_vec(), &m).await;

    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("tool_calls"));
    assert_eq!(message.response_id.as_deref(), Some("response-1"));
    assert_eq!(message.content.len(), 3, "{:?}", message.content);
    assert!(
        matches!(&message.content[0], Content::Thinking { thinking, .. } if thinking == "reason")
    );
    assert!(matches!(&message.content[1], Content::Text { text, .. } if text == "answer"));
    let Content::ToolCall(tc) = &message.content[2] else {
        panic!("expected a tool call, got {:?}", message.content[2]);
    };
    assert_eq!(tc.id.as_str(), "abc123456");
    assert_eq!(tc.name, "lookup");
    assert_eq!(
        serde_json::to_value(&tc.arguments).unwrap(),
        json!({ "query": "pi" })
    );
    // pi: `toMatchObject({ input: 7, output: 4, cacheRead: 3, cacheWrite: 0, totalTokens: 14 })`.
    assert_eq!(message.usage.input, 7);
    assert_eq!(message.usage.output, 4);
    assert_eq!(message.usage.cache_read, 3);
    assert_eq!(message.usage.cache_write, 0);
    assert_eq!(message.usage.total_tokens, 14);
}

/// pi `test/mistral-raw-stop-reason.test.ts`, all three cases, on its own fixture: a
/// `finish_reason: "error"` chunk reaches `PROV-141`'s retryable
/// `"Provider stopped with: error (server error)"` through the decoder, an unknown reason stays
/// non-retryable, and a clean `stop` keeps its raw word.
#[tokio::test]
async fn prov152_a_snake_case_error_finish_reason_reaches_the_retryable_message() {
    fn fixture(reason: &str) -> Vec<u8> {
        format!(
            "data: {{\"id\":\"mistral-response-id\",\"model\":\"devstral-medium-latest\",\"choices\":[{{\"index\":0,\"finish_reason\":\"{reason}\",\"delta\":{{}}}}],\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":0,\"total_tokens\":1}}}}\n\ndata: [DONE]\n\n"
        )
        .into_bytes()
    }
    let m = model_with("devstral-medium-latest", false);

    let events = collect(fixture("stop"), &m).await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));
    assert_eq!(message.error_message, None);
    assert_eq!(message.usage.input, 1);
    assert_eq!(message.usage.total_tokens, 1);

    let events = collect(fixture("error"), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(error.raw_stop_reason.as_deref(), Some("error"));
    assert_eq!(
        error.error_message.as_deref(),
        Some("Provider stopped with: error (server error)")
    );
    assert!(
        is_retryable_assistant_error(error),
        "{:?}",
        error.error_message
    );

    let events = collect(fixture("unmapped_error"), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(error.raw_stop_reason.as_deref(), Some("unmapped_error"));
    assert_eq!(
        error.error_message.as_deref(),
        Some("Provider stopped with: unmapped_error")
    );
    assert!(!is_retryable_assistant_error(error));
}
