//! The `google-generative-ai` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the Gemini Generative Language streaming API
//! (`POST {baseUrl}/models/{model}:streamGenerateContent?alt=sse`, newline-delimited
//! `GenerateContentResponse` JSON over SSE). Shared by the `google` provider and the
//! `opencode` provider's google-tagged models. Pure JSON-over-SSE — no SDK, no new dependency.
//!
//! 1:1 port of Pi's `api/google-generative-ai.ts` + `api/google-shared.ts`: the `Content[]`
//! encoder (`convertMessages`), `convertTools` (`parametersJsonSchema`), the Gemini-3 / Gemma-4
//! thinking-level vs token-budget split, `thoughtSignature` retention (`isThinkingPart` /
//! `retainThoughtSignature` / base64 validation), unique tool-call-id synthesis, and the
//! `candidate.content.parts` streaming decoder.
//!
//! Wire JSON uses Google's own field names (camelCase: `functionCall`, `thoughtSignature`,
//! `maxOutputTokens`, `thinkingConfig`), NOT cyrup's serde camelCase convention.

mod capabilities;
mod convert;
mod decoder;
mod driver;
mod endpoint;
mod finish;
mod options;
mod params;
mod parts;
mod signatures;
mod stop_reason;
mod thinking;
mod tools;

#[cfg(test)]
mod tests;

pub(crate) use driver::decode_stream;
pub use options::{GoogleOptions, GoogleThinking, GoogleThinkingLevel};
pub(crate) use params::build_params;

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
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::GOOGLE_GENERATIVE_AI;

/// Monotonic counter for synthesizing unique tool-call ids (Pi `toolCallCounter`,
/// google-generative-ai.ts:47).
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The `ApiImpl` for `"google-generative-ai"`.
pub struct GoogleGenerativeAiApi {
    api: ApiId,
}

impl Default for GoogleGenerativeAiApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl GoogleGenerativeAiApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(GoogleGenerativeAiApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for GoogleGenerativeAiApi {
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

        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body.
        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP.
        let params = match build_params(model, ctx, opts) {
            Ok(p) => p,
            Err(e) => {
                let e = ProviderError::from(e);
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
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
