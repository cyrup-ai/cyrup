//! Tests for the `google-generative-ai` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod capabilities;
mod convert;
mod decode;
mod endpoint;
mod params;
mod signatures;
mod stop_reason;
mod thinking;
mod tools;

use super::capabilities::*;
use super::convert::*;
use super::driver::*;
use super::endpoint::*;
use super::options::*;
use super::params::*;
use super::signatures::*;
use super::stop_reason::*;
use super::*;
use crate::api::channel;
use crate::context::{Context, ToolDef};
use crate::model::ModelCost;
use crate::model::{Modality, Model};
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{StreamEvent, StreamOptions, ToolChoice};
use cyrup_core::{
    ApiId, AssistantMessage, Content, Message, ModelThinkingLevel, StopReason, ToolCall,
    ToolCallId, Usage,
};
use serde_json::{Value, json};

fn model_with(id: &str, reasoning: bool) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        api: API_ID.into(),
        provider: "google".into(),
        base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
        reasoning,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost {
            input: 0.3,
            output: 2.5,
            cache_read: 0.03,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 1_048_576,
        max_tokens: 65_536,
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
        decode_stream(frames, &m2, &api2, &sink).await;
    });
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    task.await.unwrap();
    events
}

/// Build a two-message context whose assistant turn is attributed to `(provider, model)`.
fn signed_block_ctx(provider: &str, model: &str, content: Vec<Content>) -> Context {
    Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("Hi")],
                timestamp: 0,
            },
            Message::Assistant(AssistantMessage {
                content,
                provider: provider.into(),
                model: model.to_string(),
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
            }),
        ],
        tools: Vec::new(),
    }
}

fn a_tool_call() -> Content {
    Content::ToolCall(ToolCall {
        id: ToolCallId::from("call_1"),
        name: "bash".to_string(),
        arguments: serde_json::Map::new(),
        thought_signature: None,
    })
}

fn model_turn_parts(contents: &[Value]) -> Vec<Value> {
    contents
        .iter()
        .find(|c| c["role"] == "model")
        .and_then(|c| c["parts"].as_array().cloned())
        .expect("model turn")
}
