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
use crate::stream::sse::decode_sse_bytes_flushing_at_eof;
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
        prompt_cache: None,
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
    // PROV-084: the live Mistral adapter sets `flush_at_eof`, so the replay path must too.
    let frames = decode_sse_bytes_flushing_at_eof(frames_bytes);
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

/// An [`AuthResult`] carrying just a bearer key, with an explicit (empty) env overlay so the
/// loopback tests below cannot be steered by the developer's shell (only proxy-target resolution in
/// `build_client_for_target` reads `auth.env`).
fn auth_with(api_key: &str) -> AuthResult {
    AuthResult {
        auth: crate::auth::types::ModelAuth {
            api_key: Some(api_key.to_string()),
            ..Default::default()
        },
        env: Some(crate::auth::ProviderEnv::new()),
        source: None,
    }
}

/// Serve one canned HTTP/1.1 response off `127.0.0.1:0` and return its origin. NOTHING in this
/// module's tests may reach a real host (rule: no network in tests). `body` is written verbatim, so
/// a transcript that ends without a terminating blank line reaches the framer exactly as a relay
/// that closed the connection early would deliver it.
async fn serve_once(body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let _ = sock.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}")
}

/// Drive the whole `ApiImpl::run` path against a loopback origin and collect the events.
async fn run_against(base: &str, m: &Model) -> Vec<StreamEvent> {
    let mut m = m.clone();
    m.base_url = base.to_string();
    let (sink, mut rx) = channel(64);
    MistralConversationsApi::new()
        .run(
            &m,
            &user_ctx("hi"),
            &auth_with("sk-live"),
            &StreamOptions::default(),
            CancelToken::new(),
            sink,
        )
        .await;
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}
