//! PROV-101: grammar-constrained (custom) tools on the Responses wire — the request side, the
//! replay of a call and its result, and the decoding of the streamed `custom_tool_call`.
//! Reference: pi `packages/ai/test/constrained-sampling.test.ts` @v1.0.4.

use super::*;
use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants};
use crate::stream::StreamEvent;
use std::collections::HashMap;

fn grammar_tool(name: &str, property: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: "a grammar tool".into(),
        parameters: json!({
            "type": "object",
            "properties": { property: { "type": "string" } },
            "required": [property],
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

// ---------------------------------------------------------------------------
// Request side
// ---------------------------------------------------------------------------

/// A model that declares `supportsOpenAIGrammarTools` gets the grammar tool as a `custom` tool
/// whose `format` carries the Lark definition; one that does not keeps the JSON function tool.
#[test]
fn a_grammar_tool_is_sent_as_a_custom_tool_only_when_the_model_supports_it() {
    let mut ctx = user_ctx("hi");
    ctx.tools = vec![grammar_tool("sample_tool", "payload")];

    let body = build_params(&grammar_model(true), &ctx, &StreamOptions::default(), None);
    assert_eq!(
        body["tools"][0],
        json!({
            "type": "custom",
            "name": "sample_tool",
            "description": "a grammar tool",
            "format": { "type": "grammar", "syntax": "lark", "definition": "start: /[a-z]+/" },
        })
    );

    for m in [grammar_model(false), model()] {
        let body = build_params(&m, &ctx, &StreamOptions::default(), None);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "sample_tool");
        assert!(body["tools"][0].get("format").is_none());
    }
}

/// A tool that opted into a grammar but supplied no usable variant cannot be sent as a custom
/// tool: the turn fails with pi's message instead of silently falling back.
#[test]
fn a_grammar_tool_without_a_variant_fails_the_request_on_a_supporting_model() {
    let mut tool = grammar_tool("sample_tool", "payload");
    tool.constrained_sampling = Some(ConstrainedSampling::Config(
        ConstrainedSamplingConfig::Grammar {
            variants: GrammarVariants::default(),
        },
    ));
    let mut ctx = user_ctx("hi");
    ctx.tools = vec![tool];
    let err = try_build_params(
        &grammar_model(true),
        &ctx,
        &StreamOptions::default(),
        None,
        ResponsesTokenKind::ApiKey,
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        "Tool \"sample_tool\" cannot use grammar constrained sampling: no supported grammar variant was provided."
    );
}

// ---------------------------------------------------------------------------
// Replay: the call and its result in the next request
// ---------------------------------------------------------------------------

fn call_and_result(provider: &str, api: &str, model_id: &str, args: Value) -> Vec<Message> {
    let Value::Object(args) = args else {
        panic!("test arguments are an object")
    };
    vec![
        Message::User {
            content: vec![Content::text("go")],
            timestamp: 0,
        },
        Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: ToolCallId::from("call_1|ctc_1"),
                name: "sample_tool".to_string(),
                arguments: args.into(),
                thought_signature: None,
                namespace: None,
            })],
            provider: provider.into(),
            model: model_id.to_string(),
            api: api.into(),
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
            tool_call_id: ToolCallId::from("call_1|ctc_1"),
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

fn replay_ctx(messages: Vec<Message>) -> Context {
    Context {
        system_prompt: None,
        messages,
        tools: vec![grammar_tool("sample_tool", "payload")],
    }
}

#[test]
fn a_grammar_call_replays_as_a_custom_tool_call_and_its_result_as_custom_tool_call_output() {
    let ctx = replay_ctx(call_and_result(
        "openai",
        API_ID,
        "gpt-5",
        json!({ "payload": "abc" }),
    ));
    let body = build_params(&grammar_model(true), &ctx, &StreamOptions::default(), None);
    let input = body["input"].as_array().unwrap();
    assert!(
        input.contains(&json!({
            "type": "custom_tool_call",
            "id": "ctc_1",
            "call_id": "call_1",
            "name": "sample_tool",
            "input": "abc",
        })),
        "{input:?}"
    );
    assert!(
        input.contains(&json!({
            "type": "custom_tool_call_output",
            "call_id": "call_1",
            "output": "done",
        })),
        "{input:?}"
    );
    assert!(input.iter().all(|i| i["type"] != "function_call"));
    assert!(input.iter().all(|i| i["type"] != "function_call_output"));
}

/// Pi: arguments that do not hold the grammar property as a string cannot be replayed as raw text.
#[test]
fn a_grammar_call_without_its_string_argument_cannot_be_replayed() {
    for args in [json!({}), json!({ "payload": 42 })] {
        let ctx = replay_ctx(call_and_result("openai", API_ID, "gpt-5", args));
        let err = try_build_params(
            &grammar_model(true),
            &ctx,
            &StreamOptions::default(),
            None,
            ResponsesTokenKind::ApiKey,
        )
        .unwrap_err();
        assert!(
            err.0.contains(
                "Grammar tool call \"sample_tool\" requires argument \"payload\" to be a string"
            ),
            "{err}"
        );
    }
}

/// earendil-works/radius#115: a gateway forwards another model's history as a foreign provider;
/// its item id is not one this endpoint issued, so it is dropped.
#[test]
fn a_foreign_item_id_is_dropped_when_a_grammar_call_is_replayed() {
    let ctx = replay_ctx(call_and_result(
        "radius",
        "pi-messages",
        "gpt-other",
        json!({ "payload": "abc" }),
    ));
    let body = build_params(&grammar_model(true), &ctx, &StreamOptions::default(), None);
    let call = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "custom_tool_call")
        .expect("custom_tool_call");
    assert_eq!(call["call_id"], "call_1");
    assert_eq!(call["input"], "abc");
    assert!(call.get("id").is_none(), "{call}");
}

/// A call made while grammar tools were on replays as a plain `function_call` once the model no
/// longer supports them: a `ctc_*` id is not valid on a `function_call`, so it is dropped, and the
/// result answers with `function_call_output`.
#[test]
fn a_grammar_call_falls_back_to_a_function_call_when_support_is_off() {
    let ctx = replay_ctx(call_and_result(
        "openai",
        API_ID,
        "gpt-5",
        json!({ "payload": "abc" }),
    ));
    let body = build_params(&grammar_model(false), &ctx, &StreamOptions::default(), None);
    let input = body["input"].as_array().unwrap();
    let call = input
        .iter()
        .find(|i| i["type"] == "function_call")
        .expect("function_call");
    assert_eq!(call["call_id"], "call_1");
    assert_eq!(call["arguments"], "{\"payload\":\"abc\"}");
    assert!(call.get("id").is_none(), "{call}");
    assert!(input.iter().any(|i| i["type"] == "function_call_output"));
    assert!(input.iter().all(|i| i["type"] != "custom_tool_call"));
    assert!(input.iter().all(|i| i["type"] != "custom_tool_call_output"));
}

// ---------------------------------------------------------------------------
// Decoding the streamed custom_tool_call
// ---------------------------------------------------------------------------

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

async fn drain(events: &[Value], inputs: &[(&str, &str)]) -> Vec<StreamEvent> {
    let frames = decode_sse_bytes(sse(events).into_bytes());
    let (sink, mut rx) = crate::api::channel(256);
    let m = model();
    let api = ApiId::from(API_ID);
    let options = DecodeOptions {
        end_turn: None,
        grammar_inputs: inputs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<HashMap<_, _>>(),
    };
    decode_stream_with_options(frames, &m, &api, &sink, options).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
}

fn terminal(events: &[StreamEvent]) -> &AssistantMessage {
    match events.last() {
        Some(StreamEvent::Done { message, .. }) => message,
        Some(StreamEvent::Error { error, .. }) => error,
        other => panic!("no terminal event: {other:?}"),
    }
}

fn completed() -> Value {
    json!({
        "type": "response.completed",
        "response": { "status": "completed", "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } },
    })
}

fn custom_item(input: &str) -> Value {
    json!({ "type": "custom_tool_call", "call_id": "call_1", "id": "ctc_1", "name": "sample_tool", "input": input })
}

fn tool_deltas(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect()
}

/// Pi "starts custom Responses tool calls with their initial input": the start snapshot already
/// holds `{payload: "a"}`, the deltas add up to the JSON object of the final text, and the finished
/// call stores the raw text under the grammar's property.
#[tokio::test]
async fn a_custom_tool_call_decodes_to_a_tool_call_carrying_the_raw_text() {
    let events = drain(
        &[
            json!({ "type": "response.output_item.added", "output_index": 0, "item": custom_item("a") }),
            json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "item_id": "ctc_1", "delta": "b" }),
            json!({ "type": "response.custom_tool_call_input.done", "output_index": 0, "item_id": "ctc_1", "input": "abc" }),
            json!({ "type": "response.output_item.done", "output_index": 0, "item": custom_item("abc") }),
            completed(),
        ],
        &[("sample_tool", "payload")],
    )
    .await;

    let start = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolCallStart { partial, .. } => Some(partial.clone()),
            _ => None,
        })
        .expect("toolcall_start");
    let Some(Content::ToolCall(started)) = start.content.first() else {
        panic!("start partial holds no tool call: {:?}", start.content)
    };
    assert_eq!(
        Value::Object(started.arguments.clone().into()),
        json!({ "payload": "a" })
    );

    let message = terminal(&events);
    assert_eq!(message.stop_reason, StopReason::ToolUse, "{message:?}");
    assert_eq!(
        serde_json::to_value(&message.content).unwrap(),
        json!([{
            "type": "toolCall",
            "id": "call_1|ctc_1",
            "name": "sample_tool",
            "arguments": { "payload": "abc" },
        }])
    );
    let joined: Value = serde_json::from_str(&tool_deltas(&events)).unwrap();
    assert_eq!(joined, json!({ "payload": "abc" }));
    // The text streams as it arrives: the first delta restates the opening and the text so far
    // (`a` from the start item plus `b`), the `done` event adds the last character and closes the
    // object, and `output_item.done` has nothing left to add.
    let pieces: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(pieces, ["{\"payload\":\"ab", "c\"}"]);
}

/// Raw script text keeps its quotes, backslashes, newlines and non-ASCII characters, however the
/// provider splits it into deltas: the arguments hold exactly the text, and the JSON deltas decode
/// to the same text.
#[tokio::test]
async fn a_custom_tool_call_keeps_special_characters_across_delta_boundaries() {
    let text = "// @options: {\"timeout_ms\": 5}\nconst s = \"a\\nb\";\nreturn `é ✓ ${s}`;";
    let chars: Vec<char> = text.chars().collect();
    let mut evs = vec![
        json!({ "type": "response.output_item.added", "output_index": 0, "item": custom_item("") }),
    ];
    for piece in chars.chunks(3) {
        let delta: String = piece.iter().collect();
        evs.push(json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "delta": delta }));
    }
    evs.push(
        json!({ "type": "response.custom_tool_call_input.done", "output_index": 0, "input": text }),
    );
    evs.push(json!({ "type": "response.output_item.done", "output_index": 0, "item": custom_item(text) }));
    evs.push(completed());

    let events = drain(&evs, &[("sample_tool", "code")]).await;
    let message = terminal(&events);
    assert_eq!(message.stop_reason, StopReason::ToolUse, "{message:?}");
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(tc.arguments.get("code"), Some(&json!(text)));
    let joined: Value = serde_json::from_str(&tool_deltas(&events)).unwrap();
    assert_eq!(joined, json!({ "code": text }));
}

/// A `custom_tool_call` for a tool the request never declared as a grammar tool is stashed under
/// pi's `"input"` property rather than dropped.
#[tokio::test]
async fn an_undeclared_custom_tool_call_is_stored_under_input() {
    let events = drain(
        &[
            json!({ "type": "response.output_item.added", "output_index": 0, "item": custom_item("") }),
            json!({ "type": "response.output_item.done", "output_index": 0, "item": custom_item("raw") }),
            completed(),
        ],
        &[],
    )
    .await;
    let message = terminal(&events);
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(
        Value::Object(tc.arguments.clone().into()),
        json!({ "input": "raw" })
    );
}

/// A call whose `output_item.done` never arrived must not be handed to the agent to run.
#[tokio::test]
async fn an_unfinished_custom_tool_call_is_rejected_like_an_unfinished_function_call() {
    let events = drain(
        &[
            json!({ "type": "response.output_item.added", "output_index": 0, "item": custom_item("") }),
            json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "delta": "abc" }),
            completed(),
        ],
        &[("sample_tool", "payload")],
    )
    .await;
    let message = terminal(&events);
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message.error_message.as_deref(),
        Some(
            "OpenAI Responses stream completed with an unfinished tool call: sample_tool (call_1|ctc_1)"
        )
    );
}

/// The final text must extend what was streamed: a `done` that rewrites it is a protocol error.
#[tokio::test]
async fn a_custom_input_that_changes_non_monotonically_fails_the_turn() {
    let events = drain(
        &[
            json!({ "type": "response.output_item.added", "output_index": 0, "item": custom_item("") }),
            json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "delta": "abc" }),
            json!({ "type": "response.custom_tool_call_input.done", "output_index": 0, "input": "xyz" }),
        ],
        &[("sample_tool", "payload")],
    )
    .await;
    let message = terminal(&events);
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message.error_message.as_deref(),
        Some("grammar tool input for property \"payload\" changed non-monotonically")
    );
}

/// The `function_call_arguments` events belong to function calls only: they must not write into a
/// custom call's text.
#[tokio::test]
async fn function_call_argument_events_do_not_touch_a_custom_tool_call() {
    let events = drain(
        &[
            json!({ "type": "response.output_item.added", "output_index": 0, "item": custom_item("") }),
            json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "delta": "ab" }),
            json!({ "type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"x\":1}" }),
            json!({ "type": "response.output_item.done", "output_index": 0, "item": custom_item("abc") }),
            completed(),
        ],
        &[("sample_tool", "payload")],
    )
    .await;
    let message = terminal(&events);
    let Some(Content::ToolCall(tc)) = message.content.first() else {
        panic!("no tool call: {:?}", message.content)
    };
    assert_eq!(
        Value::Object(tc.arguments.clone().into()),
        json!({ "payload": "abc" })
    );
}
