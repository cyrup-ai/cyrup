//! Tests for the `bedrock-converse-stream` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod config;
mod convert;
mod decode;
mod driver;
mod errors;
mod framing;
mod headers;
mod params;
mod sigv4;

use super::blocks::*;
use super::config::*;
use super::convert::*;
use super::driver::*;
use super::env::*;
use super::events::*;
use super::errors::*;
use super::failure::*;
use super::framing::*;
use super::headers::*;
use super::params::*;
use super::sigv4::*;
use super::url::*;
use super::*;
use crate::HeaderMap;
use crate::auth::{AuthResult, ProviderEnv};
use crate::context::{Context, ToolDef};
use crate::model::{Modality, Model, ModelCost};
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, ModelThinkingLevel, StopReason,
    ToolCall, ToolCallId, Usage,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn model_with(id: &str, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.to_string(),
        api: API_ID.into(),
        provider: "amazon-bedrock".into(),
        base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
        reasoning: true,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
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

fn sonnet_45() -> Model {
    model_with(
        "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
        "Claude Sonnet 4.5",
    )
}

fn opus_48() -> Model {
    model_with("global.anthropic.claude-opus-4-8-v1", "Claude Opus 4.8 (Global)")
}

fn user_ctx(text: &str) -> Context {
    Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text(text)],
            timestamp: 0,
        }],
        tools: Vec::new(),
    }
}

fn env_map(pairs: &[(&str, &str)]) -> ProviderEnv {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// An `EnvSource` with an explicit (possibly empty) ambient map, so no test can be influenced
/// by the ambient AWS configuration of the machine running it.
fn env_source<'a>(overlay: Option<&'a ProviderEnv>, ambient: &'a ProviderEnv) -> EnvSource<'a> {
    EnvSource {
        overlay,
        ambient: Some(ambient),
    }
}

/// Keyless auth — `AuthResult` has no `Default`.
fn no_auth() -> AuthResult {
    AuthResult {
        auth: crate::auth::types::ModelAuth::default(),
        env: None,
        source: Some("keyless".to_string()),
    }
}

fn opts_with_reasoning(level: ModelThinkingLevel) -> StreamOptions {
    StreamOptions {
        reasoning: level,
        ..Default::default()
    }
}

fn payload(model: &Model, ctx: &Context, opts: &StreamOptions, bedrock: &BedrockOptions) -> Value {
    let ambient = ProviderEnv::new();
    build_params(
        model,
        ctx,
        opts,
        bedrock,
        CacheRetention::None,
        &env_source(None, &ambient),
    )
    .expect("payload builds")
}

fn messages_of(body: &Value) -> &Vec<Value> {
    body["messages"].as_array().expect("messages array")
}

/// Encode one AWS event-stream frame, so the decoder is tested against bytes built to the
/// published layout rather than against its own output.
fn frame(headers: &[(&str, &str)], payload: &str) -> Vec<u8> {
    let mut header_bytes = Vec::new();
    for (name, value) in headers {
        header_bytes.push(name.len() as u8);
        header_bytes.extend_from_slice(name.as_bytes());
        header_bytes.push(7); // string
        header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        header_bytes.extend_from_slice(value.as_bytes());
    }
    let payload = payload.as_bytes();
    let total = 16 + header_bytes.len() + payload.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    let prelude_crc = crc32(&out);
    out.extend_from_slice(&prelude_crc.to_be_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(payload);
    let message_crc = crc32(&out);
    out.extend_from_slice(&message_crc.to_be_bytes());
    out
}

fn event(event_type: &str, payload: &str) -> Vec<u8> {
    frame(
        &[
            (":message-type", "event"),
            (":event-type", event_type),
            (":content-type", "application/json"),
        ],
        payload,
    )
}

async fn collect(chunks: Vec<Vec<u8>>, model: &Model) -> Vec<StreamEvent> {
    let api = ApiId::from(API_ID);
    let (sink, mut rx) = crate::api::channel(64);
    let mut dec = Decoder::default();
    let mut frames = EventStreamDecoder::default();
    let m = model.clone();
    let a = api.clone();
    let task = tokio::spawn(async move {
        // No `start` is pushed here: `dispatch_frame` emits it from `messageStart`, exactly as
        // pi does (`:262`), which is what this helper is exercising.
        for chunk in chunks {
            frames.push(&chunk);
            while let Some(f) = frames.next_frame().expect("frame") {
                if let Err(message) = dispatch_frame(&f, &mut dec, &m, &a, &sink).await {
                    let mut msg = dec.snapshot(&m, &a);
                    msg.stop_reason = StopReason::Error;
                    msg.error_message = Some(message);
                    sink.send(StreamEvent::terminal(msg)).await;
                    return;
                }
            }
        }
        let mut msg = dec.snapshot(&m, &a);
        if dec.stop_reason == Some(StopReason::Error) && dec.error_message.is_none() {
            msg.error_message = Some("An unknown error occurred".to_string());
        }
        sink.send(StreamEvent::end_of_stream(
            msg,
            dec.stop_reason,
            "Bedrock stream ended without a stop reason",
        ))
        .await;
    });
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    let _ = task.await;
    out
}

fn kinds(events: &[StreamEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            StreamEvent::Start { .. } => "start",
            StreamEvent::TextStart { .. } => "text_start",
            StreamEvent::TextDelta { .. } => "text_delta",
            StreamEvent::TextEnd { .. } => "text_end",
            StreamEvent::ThinkingStart { .. } => "thinking_start",
            StreamEvent::ThinkingDelta { .. } => "thinking_delta",
            StreamEvent::ThinkingEnd { .. } => "thinking_end",
            StreamEvent::ToolCallStart { .. } => "toolcall_start",
            StreamEvent::ToolCallDelta { .. } => "toolcall_delta",
            StreamEvent::ToolCallEnd { .. } => "toolcall_end",
            StreamEvent::Done { .. } => "done",
            StreamEvent::Error { .. } => "error",
        })
        .collect()
}

#[test]
fn the_factory_serves_the_bedrock_api_id() {
    assert_eq!(factory().api().as_str(), "bedrock-converse-stream");
}
