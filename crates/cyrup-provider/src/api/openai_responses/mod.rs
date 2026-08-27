//! The `openai-responses` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the OpenAI Responses streaming API (`POST {baseUrl}/responses`, SSE
//! events `response.created` / `response.output_item.{added,done}` /
//! `response.{output_text,reasoning_text,reasoning_summary_text,refusal,function_call_arguments}.delta`
//! / `response.completed` / …). Shared by the `openai` provider (and, with their own variants,
//! azure / cloudflare-ai-gateway / github-copilot). Pure JSON-over-SSE — no SDK, no new dependency.
//! 1:1 port of Pi's `api/openai-responses.ts` + `api/openai-responses-shared.ts` (reasoning items,
//! encrypted-content include, prompt-cache key/retention, structured text signatures, foreign
//! tool-call-id rewriting, and the full streaming decoder).
//!
//! Wire JSON uses OpenAI's own field names (snake_case), NOT the cyrup camelCase convention.

mod auth;
mod blocks;
mod convert;
mod decoder;
mod errors;
mod events;
mod finalize;
mod headers;
mod ids;
mod options;
mod params;
mod pricing;
mod slots;
mod tools;
mod url;

#[cfg(test)]
mod tests;

pub use options::{OpenAiResponsesOptions, ReasoningSummary};

pub(crate) use convert::convert_responses_messages;
pub(crate) use decoder::decode_stream;
pub(crate) use tools::{ConvertResponsesToolsOptions, convert_responses_tools};
#[cfg(test)]
pub(crate) use params::build_params;

use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::stream::sse::SseRequest;
use crate::utils::provider_plumbing::connect_sse;
use auth::resolve_api_key;
use cyrup_core::{ApiId, CancelToken};
use headers::build_headers;
use params::try_build_params;
use std::sync::Arc;
use url::resolve_url;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::OPENAI_RESPONSES;

/// The `ApiImpl` for `"openai-responses"`.
pub struct OpenAiResponsesApi {
    api: ApiId,
}

impl Default for OpenAiResponsesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl OpenAiResponsesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(OpenAiResponsesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for OpenAiResponsesApi {
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

        let url = match resolve_url(model, auth) {
            Some(url) => url,
            None => {
                let e = ProviderError::Transport("no base URL configured for model".into());
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // getClientApiKey (openai-responses.ts:37-41): an explicit key wins; otherwise an
        // authorization / cf-aig-authorization header lets the key be the literal "unused".
        let api_key = match resolve_api_key(auth, opts) {
            Some(k) => k,
            None => {
                let e = ProviderError::Transport(
                    format!("No API key for provider: {}", model.provider).into(),
                );
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message.
        let params = match try_build_params(model, ctx, opts, auth.env.as_ref()) {
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
        let headers = build_headers(model, ctx, auth, opts, &api_key);
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
