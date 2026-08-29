//! Tests for the `openai-completions` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod cache;
mod decode;
mod finalize;
mod headers;
mod params;
mod tools;
mod transform;

use super::cache::*;
use super::decode::*;
use super::driver::*;
use super::headers::*;
use super::params::*;
use super::transform::*;
use crate::api::channel;
use crate::api::compat::{DeferredToolsMode, ModelCompat, get_compat};
use crate::auth::{AuthResult, ModelAuth};
use crate::context::{Context, ToolDef};
use crate::model::{Modality, Model, ModelCost};
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use crate::utils::hash::short_hash;
use crate::utils::provider_plumbing::resolve_cache_retention;
use cyrup_core::{
    ApiId, AssistantMessage, Content, Message, ModelThinkingLevel, StopReason, ToolCall, ToolCallId,
    Usage,
};
use serde_json::{Map, Value, json};

fn model() -> Model {
    Model {
        id: "openai/gpt-oss-120b".into(),
        name: "GPT OSS".into(),
        api: API_ID.into(),
        provider: "together".into(),
        base_url: "https://api.together.ai/v1".to_string(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.5,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 131072,
        max_tokens: 131072,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn auth_with_key() -> AuthResult {
    AuthResult {
        auth: ModelAuth {
            api_key: Some("sk-xyz".into()),
            ..Default::default()
        },
        env: None,
        source: Some("env".into()),
    }
}

fn ctx_with_tool_call_ids(ids: &[&str]) -> Context {
    let calls: Vec<Content> = ids
        .iter()
        .map(|id| {
            Content::ToolCall(ToolCall {
                id: ToolCallId::from(*id),
                name: "read".into(),
                arguments: Map::new().into(),
                thought_signature: None,
            })
        })
        .collect();
    let mut messages = vec![Message::Assistant(AssistantMessage {
        content: calls,
        // A DIFFERENT provider/api produced these — the cross-provider replay case.
        provider: "openai".into(),
        model: "gpt-5.4".into(),
        api: "openai-responses".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    })];
    for id in ids {
        messages.push(Message::ToolResult {
            tool_call_id: ToolCallId::from(*id),
            tool_name: "read".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            timestamp: 0,
            usage: None,
            added_tool_names: Vec::new(),
        });
    }
    Context {
        system_prompt: None,
        messages,
        tools: vec![],
    }
}

fn openai_model() -> Model {
    let mut m = model();
    m.id = "gpt-5".into();
    m.provider = "openai".into();
    m.base_url = "https://api.openai.com/v1".to_string();
    m
}

async fn collect_events(raw: &'static str) -> Vec<StreamEvent> {
    let (sink, mut rx) = channel(256);
    let m = model();
    let api = ApiId::from(API_ID);
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let handle = tokio::spawn(async move {
        decode_stream(frames, &m, &api, &sink).await;
    });
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    handle.await.unwrap();
    events
}

async fn collect_events_with(m: Model, raw: &'static str) -> Vec<StreamEvent> {
    let (sink, mut rx) = channel(256);
    let api = ApiId::from(API_ID);
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let handle = tokio::spawn(async move {
        decode_stream(frames, &m, &api, &sink).await;
    });
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    handle.await.unwrap();
    events
}
