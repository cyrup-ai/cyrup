//! Tests for the `anthropic-messages` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod catalog;
mod convert;
mod decode;
mod deferred_tools;
mod headers;
mod params;
mod perf001;
mod tool_references;
mod tools;

use super::claude_code::*;
use super::compat::*;
use super::driver::*;
use super::headers::*;
use super::messages::*;
use super::options::*;
use super::params::*;
use super::*;
use crate::api::channel;
use crate::api::compat::ModelCompat;
use crate::auth::types::ModelAuth;
use crate::auth::{AuthResult, ProviderEnv};
use crate::context::{Context, ToolDef};
use crate::model::{Modality, Model, ModelCost};
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use cyrup_core::{
    ApiId, AssistantMessage, Content, Message, ModelThinkingLevel, ProviderId, StopReason,
    ToolCall, ToolCallId, Usage,
};
use serde_json::{Map, Value, json};

fn auth_with(api_key: Option<&str>) -> AuthResult {
    AuthResult {
        auth: ModelAuth {
            api_key: api_key.map(String::from),
            ..Default::default()
        },
        env: None,
        source: None,
    }
}

fn model() -> Model {
    Model {
        id: "claude-opus-4-5".into(),
        name: "Claude Opus 4.5".into(),
        api: API_ID.into(),
        provider: "anthropic".into(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: true,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 64_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn user_ctx(text: &str) -> Context {
    Context {
        system_prompt: Some("be brief".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text(text)],
            timestamp: 0,
        }],
        tools: Vec::new(),
    }
}

async fn collect(frames_bytes: Vec<u8>, m: &Model) -> Vec<StreamEvent> {
    let (sink, mut rx) = channel(64);
    let api = ApiId::from(API_ID);
    let frames = decode_sse_bytes(frames_bytes);
    let m2 = m.clone();
    let api2 = api.clone();
    let task = tokio::spawn(async move {
        decode_stream(frames, &m2, &api2, &sink, false, &[]).await;
    });
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    task.await.unwrap();
    events
}

fn tool_def(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: format!("The {name} tool"),
        parameters: json!({ "type": "object", "properties": {}, "required": [] }),
        constrained_sampling: None,
    }
}

fn tc_assistant(calls: &[(&str, &str)]) -> Message {
    Message::Assistant(AssistantMessage {
        content: calls
            .iter()
            .map(|(id, name)| {
                Content::ToolCall(ToolCall {
                    id: ToolCallId::from(*id),
                    name: (*name).to_string(),
                    arguments: Map::new().into(),
                    thought_signature: None,
                })
            })
            .collect(),
        provider: ProviderId::from("anthropic"),
        model: "claude-opus-4-6".into(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 2,
    })
}

fn tr(id: &str, content: Vec<Content>, added: &[&str]) -> Message {
    Message::ToolResult {
        tool_call_id: ToolCallId::from(id),
        tool_name: "base_tool".to_string(),
        content,
        is_error: false,
        details: None,
        usage: None,
        added_tool_names: added.iter().map(|s| (*s).to_string()).collect(),
        timestamp: 3,
    }
}

/// Pi `makeContext`: user → assistant(toolCall base_tool) → toolResult(added) → user.
fn deferred_ctx(tools: Vec<ToolDef>, added: &[&str]) -> Context {
    Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 1,
            },
            tc_assistant(&[("call_1", "base_tool")]),
            tr("call_1", vec![Content::text("done")], added),
            Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 4,
            },
        ],
        tools,
    }
}

fn opus_4_6() -> Model {
    Model {
        id: "claude-opus-4-6".into(),
        ..model()
    }
}

/// The `content` array of the first `user` message that carries a `tool_result` block.
fn tool_result_content(body: &Value) -> Vec<Value> {
    let msgs = body["messages"].as_array().expect("messages");
    for m in msgs {
        if let Some(arr) = m["content"].as_array()
            && arr.iter().any(|b| b["type"] == "tool_result")
        {
            return arr.clone();
        }
    }
    panic!("no tool_result in payload: {body:#}");
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}
