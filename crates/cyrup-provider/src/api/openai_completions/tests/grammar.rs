//! PROV-101: grammar-constrained (`custom`) tools on the Chat Completions wire. The catalog enables
//! `supportsOpenAIGrammarTools` for no completions route, so this is reached through a model's own
//! `compat` override; the default stays the JSON function tool.
//! Reference: pi `openai-completions.ts` `convertTools`, `convertMessages` and the streaming
//! `customInput` handling @v1.0.4.

use super::*;
use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants};
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use std::collections::HashMap;

fn grammar_tool() -> ToolDef {
    ToolDef {
        name: "sample_tool".into(),
        description: "a grammar tool".into(),
        parameters: json!({
            "type": "object",
            "properties": { "payload": { "type": "string" } },
            "required": ["payload"],
        }),
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /[a-z]+/".to_string()),
                    openai_regex: None,
                },
            },
        )),
    }
}

fn grammar_model(enabled: bool) -> Model {
    let mut m = model();
    m.compat = Some(ModelCompat {
        supports_openai_grammar_tools: Some(enabled),
        ..Default::default()
    });
    m
}

fn history(tool_call_args: Value) -> Vec<Message> {
    let Value::Object(args) = tool_call_args else {
        panic!("test arguments are an object")
    };
    vec![
        Message::User {
            content: vec![Content::text("go")],
            timestamp: 0,
        },
        Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: ToolCallId::from("call_1"),
                name: "sample_tool".to_string(),
                arguments: args.into(),
                thought_signature: None,
                namespace: None,
            })],
            provider: "together".into(),
            model: "openai/gpt-oss-120b".to_string(),
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
            timestamp: 0,
            duration_ms: None,
        }),
        Message::ToolResult {
            duration_ms: None,
            tool_call_id: ToolCallId::from("call_1"),
            tool_name: "sample_tool".to_string(),
            content: vec![Content::text("done")],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 0,
            nested_calls: None,
        },
    ]
}

#[test]
fn a_grammar_tool_is_a_custom_tool_only_when_the_model_opts_in() {
    let ctx = Context {
        tools: vec![grammar_tool()],
        ..Default::default()
    };
    let body = build_body(&grammar_model(true), &ctx, &StreamOptions::default());
    assert_eq!(
        body["tools"][0],
        json!({
            "type": "custom",
            "custom": {
                "name": "sample_tool",
                "description": "a grammar tool",
                "format": {
                    "type": "grammar",
                    "grammar": { "syntax": "lark", "definition": "start: /[a-z]+/" },
                },
            },
        })
    );
    for m in [grammar_model(false), model()] {
        let body = build_body(&m, &ctx, &StreamOptions::default());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "sample_tool");
    }
}

#[test]
fn a_grammar_call_replays_as_a_custom_tool_call_carrying_the_raw_text() {
    let ctx = Context {
        messages: history(json!({ "payload": "abc" })),
        tools: vec![grammar_tool()],
        ..Default::default()
    };
    let body = build_body(&grammar_model(true), &ctx, &StreamOptions::default());
    let assistant = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message");
    assert_eq!(
        assistant["tool_calls"],
        json!([{ "id": "call_1", "type": "custom", "custom": { "name": "sample_tool", "input": "abc" } }])
    );

    // The result is the usual `tool` message.
    assert!(body["messages"].as_array().unwrap().iter().any(|m| {
        m["role"] == "tool" && m["tool_call_id"] == "call_1" && m["content"] == "done"
    }));

    // Support off: the same history replays as a `function` call with JSON arguments.
    let body = build_body(&grammar_model(false), &ctx, &StreamOptions::default());
    let assistant = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message");
    assert_eq!(
        assistant["tool_calls"][0],
        json!({
            "id": "call_1",
            "type": "function",
            "function": { "name": "sample_tool", "arguments": "{\"payload\":\"abc\"}" },
        })
    );
}

#[test]
fn a_grammar_call_without_its_string_argument_cannot_be_replayed() {
    for args in [json!({}), json!({ "payload": 42 })] {
        let ctx = Context {
            messages: history(args),
            tools: vec![grammar_tool()],
            ..Default::default()
        };
        let err = build_body_with_env(
            &grammar_model(true),
            &ctx,
            &StreamOptions::default(),
            EnvSource::default(),
        )
        .unwrap_err();
        let ConstrainedSamplingError(message) = err;
        assert!(
            message.contains(
                "Grammar tool call \"sample_tool\" requires argument \"payload\" to be a string"
            ),
            "{message}"
        );
    }
}

async fn decode_with_inputs(chunks: &[Value], inputs: &[(&str, &str)]) -> Vec<StreamEvent> {
    let raw: String = chunks
        .iter()
        .map(|c| format!("data: {c}\n\n"))
        .chain(["data: [DONE]\n\n".to_string()])
        .collect();
    let (sink, mut rx) = channel(256);
    let m = model();
    let api = ApiId::from(API_ID);
    let frames = decode_sse_bytes(raw.into_bytes());
    let inputs: HashMap<String, String> = inputs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    decode_stream_with_grammar_inputs(frames, &m, &api, &sink, inputs).await;
    drop(sink);
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

fn custom_chunk(input: &str, first: bool) -> Value {
    let mut call = json!({ "index": 0, "custom": { "input": input } });
    if first {
        call["id"] = json!("call_1");
        call["type"] = json!("custom");
        call["custom"]["name"] = json!("sample_tool");
    }
    json!({ "choices": [{ "delta": { "tool_calls": [call] } }] })
}

fn finish() -> Value {
    json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] })
}

fn deltas(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect()
}

fn terminal(events: &[StreamEvent]) -> &AssistantMessage {
    match events.last() {
        Some(StreamEvent::Done { message, .. }) => message,
        Some(StreamEvent::Error { error, .. }) => error,
        other => panic!("no terminal: {other:?}"),
    }
}

/// A streamed `custom` tool call accumulates its raw text under the grammar's property; the JSON
/// deltas add up to the object, and the finished call carries the exact text.
#[tokio::test]
async fn a_streamed_custom_tool_call_decodes_to_the_raw_text() {
    let text = "// @options: {\"timeout_ms\": 5}\nreturn \"é ✓\\n\";";
    let mut chunks = vec![custom_chunk("", true)];
    let chars: Vec<char> = text.chars().collect();
    for piece in chars.chunks(4) {
        chunks.push(custom_chunk(&piece.iter().collect::<String>(), false));
    }
    chunks.push(finish());

    let events = decode_with_inputs(&chunks, &[("sample_tool", "code")]).await;
    let message = terminal(&events);
    assert_eq!(message.stop_reason, StopReason::ToolUse, "{message:?}");
    assert_eq!(
        serde_json::to_value(&message.content).unwrap(),
        json!([{
            "type": "toolCall",
            "id": "call_1",
            "name": "sample_tool",
            "arguments": { "code": text },
        }])
    );
    let joined: Value = serde_json::from_str(&deltas(&events)).unwrap();
    assert_eq!(joined, json!({ "code": text }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallEnd { tool_call, .. }
        if tool_call.arguments.get("code") == Some(&json!(text))))
    );
}

/// A call with no text at all still closes into a valid `{property: ""}` object.
#[tokio::test]
async fn an_empty_custom_tool_call_closes_to_an_empty_string() {
    let events = decode_with_inputs(
        &[custom_chunk("", true), finish()],
        &[("sample_tool", "code")],
    )
    .await;
    let message = terminal(&events);
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(tc.arguments.get("code"), Some(&json!("")));
}

/// A `custom` call to a tool the request did not declare as a grammar tool is stored under pi's
/// `"input"` fallback property.
#[tokio::test]
async fn an_undeclared_custom_tool_call_is_stored_under_input() {
    let events = decode_with_inputs(&[custom_chunk("raw", true), finish()], &[]).await;
    let message = terminal(&events);
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(tc.arguments.get("input"), Some(&json!("raw")));
}

/// A plain `function` call is unaffected: it still streams JSON argument fragments.
#[tokio::test]
async fn a_function_tool_call_still_streams_json_arguments() {
    let chunk = |args: &str, first: bool| {
        let mut call = json!({ "index": 0, "function": { "arguments": args } });
        if first {
            call["id"] = json!("call_1");
            call["function"]["name"] = json!("echo");
        }
        json!({ "choices": [{ "delta": { "tool_calls": [call] } }] })
    };
    let events = decode_with_inputs(
        &[chunk("{\"x\":", true), chunk("1}", false), finish()],
        &[("sample_tool", "code")],
    )
    .await;
    let message = terminal(&events);
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(tc.arguments.get("x"), Some(&json!(1)));
    assert_eq!(deltas(&events), "{\"x\":1}");
}

/// Driver level: the request `run` posts declares the grammar tool as `custom`, and the `custom`
/// tool call the server streams back decodes to a tool call holding the raw text. This is what
/// proves the Chat Completions driver hands the grammar map to its decoder.
#[tokio::test]
async fn run_posts_a_custom_tool_and_decodes_the_custom_call() {
    use crate::api::ApiImpl;
    use crate::api::test_server::{grammar_tool_def, serve_sse};

    let chunks = [custom_chunk("", true), custom_chunk("abc", false), finish()];
    let body: String = chunks
        .iter()
        .map(|c| format!("data: {c}\n\n"))
        .chain(["data: [DONE]\n\n".to_string()])
        .collect();
    let (origin, seen) = serve_sse(body).await;
    let mut m = grammar_model(true);
    m.base_url = format!("{origin}/v1");
    let ctx = Context {
        messages: vec![Message::User {
            content: vec![Content::text("go")],
            timestamp: 0,
        }],
        tools: vec![grammar_tool_def()],
        ..Default::default()
    };
    let (sink, mut rx) = channel(64);
    OpenAiCompletionsApi::new()
        .run(
            &m,
            &ctx,
            &AuthResult::from_key("sk-test", "test"),
            &StreamOptions::default(),
            cyrup_core::CancelToken::new(),
            sink,
        )
        .await;
    let mut last = None;
    while let Some(ev) = rx.recv().await {
        last = Some(ev);
    }
    let Some(StreamEvent::Done { message, .. }) = last else {
        panic!("no done terminal: {last:?}")
    };
    assert_eq!(
        serde_json::to_value(&message.content).unwrap(),
        json!([{
            "type": "toolCall",
            "id": "call_1",
            "name": "sample_tool",
            "arguments": { "payload": "abc" },
        }])
    );
    let request: Value = {
        let seen = seen.lock().unwrap();
        serde_json::from_str(&seen.first().expect("a request").1).unwrap()
    };
    assert_eq!(request["tools"][0]["type"], "custom");
    assert_eq!(request["tools"][0]["custom"]["name"], "sample_tool");
}

/// A block that opens from a fragment with only a name (no `custom` member yet) becomes a custom
/// call when the text fragment arrives (pi `ensureToolCallBlock`'s upgrade, `:539-548`).
#[tokio::test]
async fn a_block_opened_by_a_bare_name_becomes_a_custom_call_when_its_text_arrives() {
    let opener = json!({ "choices": [{ "delta": { "tool_calls": [
        { "index": 0, "id": "call_1", "function": { "name": "sample_tool" } }
    ] } }] });
    let text = json!({ "choices": [{ "delta": { "tool_calls": [
        { "index": 0, "custom": { "input": "abc" } }
    ] } }] });
    let events = decode_with_inputs(&[opener, text, finish()], &[("sample_tool", "payload")]).await;
    let message = terminal(&events);
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(tc.arguments.get("payload"), Some(&json!("abc")));
}
