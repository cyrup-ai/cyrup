//! Tests for the `openai-responses` wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod convert;
mod decode;
mod errors;
mod headers;
mod params;
mod tools;

use super::decoder::*;
use super::headers::*;
use super::ids::*;
use super::options::*;
use super::params::*;
use super::tools::*;
use super::url::*;
use super::*;
use crate::api::compat::ModelCompat;
use crate::api::compat::{SessionAffinityFormat, get_responses_compat};
use crate::auth::AuthResult;
use crate::context::{Context, ToolDef};
use crate::model::{Modality, Model, ModelCost};
use crate::stream::ApiStreamOptions;
use crate::stream::collect_message;
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::hash::short_hash;
use cyrup_core::{
    ApiId, AssistantMessage, Content, Message, ModelThinkingLevel, StopReason, ToolCall,
    ToolCallId, Usage,
};
use serde_json::{Value, json};

fn model() -> Model {
    Model {
        id: "gpt-5".into(),
        name: "GPT-5".into(),
        api: API_ID.into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com/v1".to_string(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.5,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 400_000,
        max_tokens: 128_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
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

fn auth() -> AuthResult {
    AuthResult {
        auth: crate::auth::ModelAuth {
            api_key: Some("sk-test".to_string()),
            headers: None,
            base_url: None,
        },
        env: None,
        source: Some("test".to_string()),
    }
}
