//! Tests for the `openai-codex-responses` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod account_id;
mod decode;
mod errors;
mod events;
mod headers;
mod request;
mod retry;
mod url;

use super::events::*;
use super::headers::*;
use super::options::*;
use super::request::*;
use super::retry::*;
use super::url::*;
use super::*;
use crate::api::channel;
use crate::api::openai_responses::decode_stream;
use crate::auth::AuthResult;
use crate::context::Context;
use crate::model::{Modality, Model, ModelCost};
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{CacheRetention, StreamEvent, StreamOptions, ToolChoice};
use base64::Engine as _;
use cyrup_core::{AssistantMessage, CancelToken, ModelThinkingLevel, SessionId, StopReason};
use serde_json::{Value, json};

fn codex_model(id: &str) -> Model {
    Model {
        id: id.into(),
        name: "M".into(),
        api: API_ID.into(),
        provider: "openai-codex".into(),
        base_url: String::new(),
        reasoning: true,
        input: vec![Modality::Text],
        // NON-ZERO rates. With `ModelCost::default()` every rate is 0.0, so any assertion of
        // the form `|priority_cost - baseline_cost * N| < eps` reduces to `|0.0 - 0.0| < eps`
        // and holds no matter what the code does — which is exactly how the service-tier
        // pricing test below shipped vacuous. A reviewer proved it by deleting the whole
        // feature under test and watching every test stay green.
        cost: ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 1000,
        max_tokens: 1000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn opts() -> OpenAiCodexResponsesOptions {
    OpenAiCodexResponsesOptions::default()
}

fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in pairs {
        map.insert(
            reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            reqwest::header::HeaderValue::from_str(v).unwrap(),
        );
    }
    map
}

/// A locally-synthesized JWT — three dot-separated base64 segments, no signature verification
/// anywhere in this code path, and no real credential.
fn fake_jwt(payload: &Value) -> String {
    let body = ATOB.encode(serde_json::to_vec(payload).unwrap());
    format!("eyJhbGciOiJub25lIn0.{body}.sig")
}

