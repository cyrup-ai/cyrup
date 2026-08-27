//! Tests for the `mistral-conversations` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod decode;
mod endpoint;
mod payload;
mod reasoning;
mod stop_reason;
mod tool_call_id;
mod tools;

use super::driver::*;
use super::endpoint::*;
use super::finish::*;
use super::messages::*;
use super::options::*;
use super::payload::*;
use super::tool_call_id::*;
use super::*;
use crate::api::channel;
use crate::context::{Context, ToolDef};
use crate::model::ModelCost;
use crate::model::{Modality, Model};
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{StreamEvent, StreamOptions, ToolChoice};
use cyrup_core::{ApiId, Content, Message, ModelThinkingLevel, StopReason};
use cyrup_core::{SessionId, ToolCallId as CoreToolCallId};
use serde_json::{Value, json};

fn model_with(id: &str, reasoning: bool) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        api: API_ID.into(),
        provider: "mistral".into(),
        base_url: "https://api.mistral.ai".to_string(),
        reasoning,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 0.4,
            output: 2.0,
            cache_read: 0.04,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 256_000,
        max_tokens: 4096,
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
