//! The `anthropic-messages` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the Anthropic Messages streaming API (`POST {baseUrl}/v1/messages`,
//! SSE events `message_start` / `content_block_{start,delta,stop}` / `message_delta` /
//! `message_stop`). Shared by every Anthropic-compatible provider (anthropic, kimi-coding, minimax,
//! minimax-cn, vercel-ai-gateway). Pure JSON-over-SSE — no SDK, no new dependency. 1:1 port of Pi's
//! `api/anthropic-messages.ts` encoder/decoder (extended thinking, `cache_control` + 1h ttl,
//! thinking signatures, `redacted_thinking`, beta headers, eager tool input streaming, and the
//! 64-char `^[a-zA-Z0-9_-]+$` tool-call-id rule).
//!
//! Wire JSON uses Anthropic's own field names (snake_case), NOT the cyrup camelCase convention.

mod blocks;
mod cache;
mod claude_code;
mod compat;
mod convert;
mod driver;
mod events;
mod federation;
mod headers;
mod messages;
mod options;
mod params;
mod stop_reason;
pub(crate) mod tools;
mod usage;

#[cfg(test)]
mod tests;

/// PROV-083a — the mid-conversation compat resolvers. Public because the flags decide whether a
/// transcript's later system messages reach the model in place or are folded into the leading one,
/// which a caller inspecting a model's capabilities needs to be able to ask.
pub use compat::{
    default_supports_mid_convo_system_messages, default_supports_mid_convo_tool_changes,
    supports_mid_convo_system_messages, supports_mid_convo_tool_changes,
};
pub(crate) use driver::decode_stream;
pub use options::{AnthropicOptions, AnthropicThinkingDisplay};
#[cfg(test)]
pub(crate) use params::build_body;

use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::stream::sse::SseRequest;
use crate::utils::provider_plumbing::{EnvSource, connect_sse};
use claude_code::resolve_is_oauth;
use cyrup_core::{ApiId, CancelToken};
use headers::{build_headers, resolve_url};
use params::{build_params, managed_active_effort};
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::ANTHROPIC_MESSAGES;

/// The `ApiImpl` for `"anthropic-messages"`.
pub struct AnthropicMessagesApi {
    api: ApiId,
}

impl Default for AnthropicMessagesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl AnthropicMessagesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(AnthropicMessagesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for AnthropicMessagesApi {
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

        let is_oauth = resolve_is_oauth(model, auth);
        // PROV-119 — workload identity federation. The exchange happens BEFORE the headers are
        // built, because the minted token is carried as an `Authorization: Bearer` overlay on a
        // local copy of the auth (pi keeps it out of `apiKey` too, `anthropic-messages.ts:1066`).
        // `is_oauth` is deliberately computed from the ORIGINAL auth above: a federated token is
        // `sk-ant-oat01-…`, so deriving it after the overlay would rewrite every tool name to its
        // Claude Code alias. There is no key to fall back to, so an exchange failure fails the
        // turn, with the Console's authentication history named as where the real reason lives.
        // The API BASE, as `resolve_url` derives it — `url` above is already the `/v1/messages`
        // endpoint, and the exchange lives at `/v1/oauth/token` off the same base.
        let api_base = auth
            .auth
            .base_url
            .as_deref()
            .unwrap_or(model.base_url.as_str())
            .to_string();
        let federated_auth = match federation::FederationConfig::from_env(
            model,
            auth,
            EnvSource::new(auth.env.as_ref()),
        ) {
            Some(config) => match federation::access_token(&api_base, &config).await {
                Ok(token) => Some(federation::apply_to_auth(auth, &token)),
                Err(e) => {
                    let e = ProviderError::Transport(e.to_string().into());
                    sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            },
            None => None,
        };
        let auth = federated_auth.as_ref().unwrap_or(auth);
        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message.
        let params = match build_params(
            model,
            ctx,
            opts,
            EnvSource::new(auth.env.as_ref()),
            is_oauth,
        ) {
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
        // PROV-042: `transformHeaders` runs LAST over the fully-assembled set (pi
        // `models.ts:657` @v0.84.4); its return value is what goes on the wire.
        let headers = crate::stream::apply_transform_headers(
            opts,
            build_headers(model, ctx, auth, opts, is_oauth),
        )
        .await;
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
            // pi's `iterateSseMessages` ends with `flushSseEvent(state)`
            // (`anthropic-messages.ts:461` @v0.87.1), so a stream cut after the final
            // `message_stop` frame still delivers it.
            flush_at_eof: true,
        };

        let Some(frames) = connect_sse(req, model, auth, opts, cancel, &sink).await else {
            return;
        };

        decode_stream(
            frames,
            model,
            &self.api,
            &sink,
            is_oauth,
            &ctx.tools,
            // pi seeds `output.providerThinkingLevel` from the SAME `options?.effort ?? "high"` the
            // request body's trailing marker carries (`anthropic-messages.ts:521`, `:1064`), so both
            // read one `managed_active_effort` (PROV-091).
            managed_active_effort(model, opts),
        )
        .await;
    }
}
