//! The `mistral-conversations` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking Mistral's `chat.stream` API (`POST {baseUrl}/v1/chat/completions`,
//! `stream: true`, SSE `data:`-framed `CompletionChunk` JSON ending with `data: [DONE]`). Shared by
//! the `mistral` provider. Pure JSON-over-SSE — no SDK, no new dependency.
//!
//! 1:1 port of Pi's `api/mistral-conversations.ts`: the chat-payload encoder (`buildChatPayload` /
//! `toChatMessages` / `toFunctionTools`), the deterministic 9-char tool-call-id normalizer
//! (`createMistralToolCallIdNormalizer` / `deriveMistralToolCallId` over `shortHash`), the
//! `promptMode`/`reasoningEffort` reasoning lowering, `x-affinity` + `promptCacheKey` prefix caching,
//! and the `CompletionChunk` streaming decoder (string / `text` / `thinking` content chunks +
//! incremental tool calls).
//!
//! Wire JSON uses Mistral's own field names (camelCase: `maxTokens`, `toolChoice`, `promptMode`,
//! `reasoningEffort`, `toolCalls`, `toolCallId`).

mod blocks;
mod content;
mod decoder;
mod driver;
mod endpoint;
mod finish;
mod messages;
mod options;
mod payload;
mod reasoning;
mod tool_call_id;
mod tools;

#[cfg(test)]
mod tests;

pub(crate) use driver::decode_stream;
pub use options::{MistralOptions, MistralPromptMode, MistralReasoningEffort};

use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::stream::sse::SseRequest;
use crate::utils::provider_plumbing::connect_sse;
use cyrup_core::{ApiId, CancelToken};
use endpoint::{build_headers, resolve_url};
use payload::build_chat_payload;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::MISTRAL_CONVERSATIONS;

/// Mistral tool-call ids are exactly 9 alphanumerics (Pi `MISTRAL_TOOL_CALL_ID_LENGTH`,
/// mistral-conversations.ts:31).
const MISTRAL_TOOL_CALL_ID_LENGTH: usize = 9;

/// The `ApiImpl` for `"mistral-conversations"`.
pub struct MistralConversationsApi {
    api: ApiId,
}

impl Default for MistralConversationsApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl MistralConversationsApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(MistralConversationsApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for MistralConversationsApi {
    fn api(&self) -> &ApiId {
        &self.api
    }

    async fn run(
        &self,
        model: &Model,
        ctx: &Context,
        auth: &AuthResult,
        opts: &StreamOptions,
        cancel: CancelToken,
        sink: EventSink,
    ) {
        let provider = model.provider.clone();
        let model_id = model.id.as_str().to_string();

        let api_key = match &auth.auth.api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => {
                let e =
                    ProviderError::Transport(format!("No API key for provider: {provider}").into());
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        let url = match resolve_url(model, auth) {
            Some(url) => url,
            None => {
                let e = ProviderError::Transport("no base URL configured for model".into());
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message.
        let params = match build_chat_payload(model, ctx, opts) {
            Ok(p) => p,
            Err(e) => {
                let e = ProviderError::from(e);
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(opts, model, params).await;
        let headers = build_headers(model, opts, &api_key);
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        let Some(frames) = connect_sse(req, model, auth, opts, cancel, &sink).await else {
            return;
        };

        decode_stream(frames, model, &self.api, &sink).await;
    }
}
