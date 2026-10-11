//! PROV-101: a grammar-constrained `codemode` call, end to end through a real agent turn.
//!
//! The model is a scripted loopback server that speaks the OpenAI Responses SSE wire. The agent,
//! the `openai-responses` adapter, the `codemode` tool and the V8 sandbox are all real. What this
//! pins is the whole path pi's grammar sampling takes (`openai-responses-shared.ts` @v1.0.4):
//!
//! 1. the request declares `codemode` as a `custom` tool with the Lark grammar, not a JSON schema;
//! 2. a streamed `custom_tool_call` whose `input` is the raw script becomes a tool call whose
//!    `code` argument is that exact text, and `codemode` executes it;
//! 3. the next request replays the call as a `custom_tool_call` with the raw text and answers it
//!    with a `custom_tool_call_output`;
//! 4. on a model that does not declare grammar support the same tool stays a JSON function tool.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_agent::{Agent, AgentMessage, ProviderStreamFn};
use cyrup_codemode::source::CODEMODE_SOURCE_GRAMMAR;
use cyrup_core::{Content, ModelRef, Tool};
use cyrup_provider::{
    Credential, InMemoryCredentialStore, Model, ModelCost, ProviderAuth, WireProvider,
    builtin_registry, env_key,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{CodemodeHostSlot, CodemodeTool, CodemodeToolOptions, EngineSandboxFactory};
use crate::testkit::{RecordingHost, StubTool};

/// The raw script the model writes. It has a quote, a newline, a backslash and non-ASCII text, which
/// is what a JSON string would have to escape.
const SCRIPT: &str =
    "// @options: {\"timeout_ms\": 20000}\nconst answer = 6 * 7;\nreturn `n=${answer} é\\n\"ok\"`;";

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

/// The Responses stream a grammar model answers with: one `custom_tool_call` item whose text
/// arrives in three deltas.
fn custom_call_response() -> String {
    let item = |input: &str| json!({ "type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "codemode", "input": input });
    let chars: Vec<char> = SCRIPT.chars().collect();
    let third = chars.len() / 3;
    let pieces: Vec<String> = chars
        .chunks(third + 1)
        .map(|c| c.iter().collect())
        .collect();
    let mut events = vec![
        json!({ "type": "response.created", "response": { "id": "resp_1" } }),
        json!({ "type": "response.output_item.added", "output_index": 0, "item": item("") }),
    ];
    for piece in &pieces {
        events.push(json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "item_id": "ctc_1", "delta": piece }));
    }
    events.push(json!({ "type": "response.custom_tool_call_input.done", "output_index": 0, "item_id": "ctc_1", "input": SCRIPT }));
    events.push(
        json!({ "type": "response.output_item.done", "output_index": 0, "item": item(SCRIPT) }),
    );
    events.push(json!({ "type": "response.completed", "response": { "id": "resp_1", "status": "completed", "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } } }));
    sse(&events)
}

/// The Responses stream a model without grammar support answers with: a JSON `function_call`.
fn function_call_response() -> String {
    let arguments = json!({ "code": SCRIPT }).to_string();
    sse(&[
        json!({ "type": "response.created", "response": { "id": "resp_1" } }),
        json!({ "type": "response.output_item.added", "output_index": 0, "item": { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "codemode", "arguments": "" } }),
        json!({ "type": "response.function_call_arguments.delta", "output_index": 0, "delta": arguments }),
        json!({ "type": "response.function_call_arguments.done", "output_index": 0, "arguments": arguments }),
        json!({ "type": "response.output_item.done", "output_index": 0, "item": { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "codemode", "arguments": arguments } }),
        json!({ "type": "response.completed", "response": { "id": "resp_1", "status": "completed", "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } } }),
    ])
}

fn final_text_response() -> String {
    sse(&[
        json!({ "type": "response.created", "response": { "id": "resp_2" } }),
        json!({ "type": "response.output_item.added", "output_index": 0, "item": { "type": "message", "id": "msg_1" } }),
        json!({ "type": "response.output_text.delta", "output_index": 0, "delta": "done" }),
        json!({ "type": "response.output_item.done", "output_index": 0, "item": { "type": "message", "id": "msg_1", "content": [{ "type": "output_text", "text": "done" }] } }),
        json!({ "type": "response.completed", "response": { "id": "resp_2", "status": "completed", "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } } }),
    ])
}

/// A loopback server answering each request with the next scripted body, recording every request
/// body it receives.
async fn serve(responses: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    tokio::spawn(async move {
        let mut responses = responses.into_iter();
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut chunk = vec![0u8; 16384];
            let body_start = loop {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(at + 4);
                }
            };
            let Some(start) = body_start else { continue };
            let head = String::from_utf8_lossy(&raw[..start]).to_lowercase();
            let length: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            while raw.len() < start + length {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            if let Ok(body) = serde_json::from_slice::<Value>(&raw[start..]) {
                sink.lock().unwrap().push(body);
            }
            let body = responses.next().unwrap_or_else(final_text_response);
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (format!("http://{addr}/v1"), seen)
}

fn model(base_url: &str, grammar: bool) -> Model {
    Model {
        id: "gpt-5".into(),
        name: "GPT-5".into(),
        api: "openai-responses".into(),
        provider: "p".into(),
        base_url: base_url.to_string(),
        reasoning: false,
        input: vec![cyrup_provider::Modality::Text],
        cost: ModelCost::default(),
        prompt_cache: None,
        input_limits: None,
        context_window: 100_000,
        max_tokens: 10_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: Some(cyrup_provider::api::compat::ModelCompat {
            supports_openai_grammar_tools: Some(grammar),
            ..Default::default()
        }),
        headers: None,
    }
}

/// One agent turn that makes a single `codemode` call and then finishes; returns the requests the
/// server saw and the transcript.
async fn turn(grammar: bool, first_response: String) -> (Vec<Value>, Vec<AgentMessage>) {
    let (base_url, seen) = serve(vec![first_response, final_text_response()]).await;
    let provider = WireProvider::new(
        "p",
        "P",
        vec![model(&base_url, grammar)],
        ProviderAuth::with_api_key(env_key("P API key", ["P_API_KEY"])),
        Arc::new(InMemoryCredentialStore::new().with_credential(
            cyrup_core::ProviderId::from("p"),
            Credential::api_key("sk-test"),
        )),
        Arc::new(builtin_registry()),
    );

    let slot = CodemodeHostSlot::new();
    slot.bind(Arc::new(RecordingHost::new(vec![
        StubTool::new("echo", "Echo text back.").arc(),
    ])));
    let tool: Arc<dyn Tool> = Arc::new(CodemodeTool::new(CodemodeToolOptions::new(
        slot,
        Arc::new(EngineSandboxFactory),
    )));
    let agent = Agent::builder(
        ModelRef {
            provider: "p".into(),
            api: Some("openai-responses".into()),
            model: "gpt-5".into(),
        },
        Arc::new(ProviderStreamFn::new(Arc::new(provider))),
    )
    .tools(vec![tool])
    .build();

    let handle = agent.prompt("compute").await.unwrap();
    let messages = handle.finished().await;
    agent.wait_for_idle().await;
    let requests = seen.lock().unwrap().clone();
    (requests, messages)
}

fn tool_result_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::ToolResult(r) => Some(
                r.content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool result in {messages:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grammar_model_sends_codemode_as_a_custom_tool_and_runs_the_raw_script() {
    let (requests, messages) = turn(true, custom_call_response()).await;
    assert_eq!(requests.len(), 2, "{requests:?}");

    // 1. The declaration: a `custom` tool carrying the Lark grammar, not a JSON schema.
    let tool = &requests[0]["tools"][0];
    assert_eq!(tool["type"], "custom", "{tool}");
    assert_eq!(tool["name"], "codemode");
    assert_eq!(
        tool["format"],
        json!({ "type": "grammar", "syntax": "lark", "definition": CODEMODE_SOURCE_GRAMMAR })
    );
    assert!(tool.get("parameters").is_none(), "{tool}");

    // 2. The script ran with the model's exact text: the result has what it computed.
    let result = tool_result_text(&messages);
    assert!(result.contains("n=42 é"), "{result}");
    assert!(result.contains("\"ok\""), "{result}");

    // 3. The next request replays the call as raw text and answers it as `custom_tool_call_output`.
    let input = requests[1]["input"].as_array().unwrap();
    let call = input
        .iter()
        .find(|i| i["type"] == "custom_tool_call")
        .unwrap_or_else(|| panic!("no custom_tool_call in {input:?}"));
    assert_eq!(call["input"], SCRIPT);
    assert_eq!(call["call_id"], "call_1");
    assert_eq!(call["id"], "ctc_1");
    let output = input
        .iter()
        .find(|i| i["type"] == "custom_tool_call_output")
        .unwrap_or_else(|| panic!("no custom_tool_call_output in {input:?}"));
    assert_eq!(output["call_id"], "call_1");
    assert!(output["output"].as_str().unwrap().contains("n=42 é"));
    assert!(input.iter().all(|i| i["type"] != "function_call"));
    assert!(input.iter().all(|i| i["type"] != "function_call_output"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_without_grammar_support_keeps_the_json_function_tool() {
    let (requests, messages) = turn(false, function_call_response()).await;
    assert_eq!(requests.len(), 2, "{requests:?}");

    let tool = &requests[0]["tools"][0];
    assert_eq!(tool["type"], "function", "{tool}");
    assert_eq!(tool["name"], "codemode");
    assert!(tool.get("format").is_none());
    assert_eq!(tool["parameters"]["required"], json!(["code"]));

    assert!(tool_result_text(&messages).contains("n=42 é"));

    let input = requests[1]["input"].as_array().unwrap();
    let call = input
        .iter()
        .find(|i| i["type"] == "function_call")
        .unwrap_or_else(|| panic!("no function_call in {input:?}"));
    // The arguments are the JSON object, with the script as an escaped string.
    let arguments: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({ "code": SCRIPT }));
    assert!(input.iter().any(|i| i["type"] == "function_call_output"));
    assert!(input.iter().all(|i| i["type"] != "custom_tool_call"));
}
