//! The `openai-completions` [`ApiImpl`]: request assembly and the hand-off to the SSE decoder.

use super::decode::decode_stream;
use super::headers::{build_headers, resolve_url};
use super::params::build_body_with_env;
use crate::api::compat::get_compat;
use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::SseRequest;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::provider_plumbing::{connect_sse, resolve_cache_retention};
use cyrup_core::{ApiId, CancelToken};
use std::sync::Arc;

/// The wire-protocol id this impl serves.
pub(super) const API_ID: &str = crate::known_api::OPENAI_COMPLETIONS;

/// The `ApiImpl` for `"openai-completions"`.
pub struct OpenAiCompletionsApi {
    api: ApiId,
}

impl Default for OpenAiCompletionsApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl OpenAiCompletionsApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(OpenAiCompletionsApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for OpenAiCompletionsApi {
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

        // Resolve compat + the effective cache retention once (Pi `stream` L179-181), so both the
        // header build (session affinity) and the body build see the same view.
        let compat = get_compat(model);
        let cache = resolve_cache_retention(opts.cache_retention, auth.env.as_ref());
        // Pi: `cacheSessionId = cacheRetention === "none" ? undefined : options?.sessionId` (L181).
        let cache_session_id = match cache {
            CacheRetention::None => None,
            _ => opts.session_id.as_ref().map(|s| s.as_str()),
        };

        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message — upstream `buildParams` throws into `stream`'s catch.
        let params = match build_body_with_env(model, ctx, opts, auth.env.as_ref()) {
            Ok(p) => p,
            Err(e) => {
                let e = ProviderError::from(e);
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        // gap-08 #2: let a `before_provider_request` extension inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(opts, model, params).await;
        // PROV-042: `transformHeaders` runs LAST over the fully-assembled set (pi
        // `models.ts:657` @v0.84.4); its return value is what goes on the wire.
        let headers = crate::stream::apply_transform_headers(
            opts,
            build_headers(model, ctx, auth, opts, &compat, cache_session_id),
        )
        .await;
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
