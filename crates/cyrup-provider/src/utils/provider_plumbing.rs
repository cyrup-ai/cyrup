//! cyrup's own cross-`api/` plumbing: the helpers every wire impl needs, collapsed to one copy.
//!
//! Unlike the rest of [`crate::utils`], these are NOT 1:1 ports of a `packages/ai/src/utils/*`
//! file. Each is a piece of glue that either has a **single** upstream counterpart that got
//! independently ported into N `api/<protocol>.rs` files, or has no upstream counterpart at all
//! because pi gets it from the JS runtime / a vendor SDK. Collapsing them *improves* the 1:1
//! pi→cyrup mapping — one upstream function now maps to one cyrup function again.
//!
//! - [`provider_env_value`] — pi's single `getProviderEnvValue` (`provider-env.ts:44-52`), which
//!   had been ported five times.
//! - [`resolve_cache_retention`] — pi declares `resolveCacheRetention` once per api file, but the
//!   `anthropic-messages` / `openai-completions` / `openai-responses` / `bedrock-converse-stream`
//!   bodies are the same ladder; [`resolve_cache_retention_with`] is the env-agnostic core the
//!   bedrock leg drives with its own `EnvSource` test seam. `pi-messages` is deliberately NOT one
//!   of them — see the note on its own copy.
//! - [`now_millis`] — no upstream counterpart: pi writes `Date.now()`.
//! - [`connect_sse`] — no upstream counterpart: pi's api files hand the request to a vendor SDK
//!   client, so "build a proxy-aware client, open the SSE stream, fire `onResponse`" is cyrup's
//!   direct-wire substitute (PROV-006 + gap-08 #3) and had been copy-pasted into six impls.

use crate::api::EventSink;
use crate::auth::{AuthResult, ProviderEnv};
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{SseFrame, SseRequest, build_client_for_target, open_sse};
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::CancelToken;
use futures::Stream;
use std::pin::Pin;

/// Resolve a provider env value (Pi `getProviderEnvValue`, provider-env.ts:44-52): the scoped
/// `env` overlay wins over the process environment, and an empty string counts as absent (JS `||`).
pub(crate) fn provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name).filter(|v| !v.is_empty())
    {
        return Some(v.clone());
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The `resolveCacheRetention` ladder over an arbitrary env lookup: an explicit caller value wins;
/// otherwise `PI_CACHE_RETENTION == "long"` promotes to `Long`; otherwise `Short`.
///
/// The `lookup` seam exists for `bedrock-converse-stream`, whose `EnvSource` distinguishes the
/// scoped overlay from the ambient process environment so its resolution tests never depend on the
/// AWS configuration of the machine running them.
pub(crate) fn resolve_cache_retention_with(
    cache_retention: Option<CacheRetention>,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> CacheRetention {
    if let Some(c) = cache_retention {
        return c;
    }
    if lookup("PI_CACHE_RETENTION").as_deref() == Some("long") {
        return CacheRetention::Long;
    }
    CacheRetention::Short
}

/// 1:1 port of Pi `resolveCacheRetention` — `anthropic-messages.ts:46-54`,
/// `openai-completions.ts:141-149` and `openai-responses.ts:47-55` declare the identical ladder:
/// an explicit caller value wins; otherwise `PI_CACHE_RETENTION == "long"` promotes to `Long`;
/// otherwise `Short`.
pub(crate) fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> CacheRetention {
    resolve_cache_retention_with(cache_retention, |name| provider_env_value(name, env))
}

/// Current unix time in milliseconds (0 on a clock error — never panics).
///
/// The unit is **milliseconds**, the unit `Credential::Oauth.expires` is stored in: Pi writes the
/// deadline as `Date.now() + expires_in * 1000` (ai/src/auth/oauth/anthropic.ts:225 and :338;
/// likewise openai-codex.ts:145, kimi-coding.ts:137, github-copilot.ts:274) and compares it against
/// `Date.now()` (ai/src/auth/resolve.ts:110). Comparing a millisecond deadline against a seconds
/// clock made every stored token look valid until roughly the year 57,760.
///
/// The conversion saturates rather than wrapping: `as i64` on the `u128` millisecond count would
/// turn a clock past `i64::MAX` ms into a *negative* instant, i.e. an already-expired credential
/// and a nonsensical timestamp, instead of a clamped one.
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Boxed frame stream — the shape [`open_sse`] returns and each api's `decode_stream` consumes.
pub(crate) type FrameStream = Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>;

/// Open `req` as an SSE stream, or push the terminal error event and return `None`.
///
/// The connect sequence shared by every api impl that has no protocol-specific error mapping to do
/// (anthropic-messages, azure-openai-responses, google-generative-ai, mistral-conversations,
/// openai-completions, openai-responses):
///
/// 1. Honor HTTP(S)_PROXY for the live client (Pi `resolveHttpProxyUrlForTarget`,
///    node-http-proxy.ts:92-112; applied per request as in bedrock-converse-stream.ts:187).
///    PROV-006: the request idle timeout. `StreamOptions.timeout_ms` overrides the process-global
///    `configure_http_idle_timeout` default, exactly as Pi layers the SDK client's `timeout` on top
///    of the global undici dispatcher (sdk.ts:304-309).
/// 2. gap-08 #3: capture `{status, headers}` at connect, then fire `after_provider_response`.
///
/// A client-build failure, and a transport / non-2xx / abort-during-connect failure from
/// [`open_sse`], both become a terminal `Error` (R-01-018/045) and yield `None`. `on_response`
/// fires only once the stream is open, matching the copies this replaces.
pub(crate) async fn connect_sse(
    req: SseRequest,
    model: &Model,
    auth: &AuthResult,
    opts: &StreamOptions,
    cancel: CancelToken,
    sink: &EventSink,
) -> Option<FrameStream> {
    let provider = model.provider.clone();
    let model_id = model.id.as_str();

    let client = match build_client_for_target(
        &req.url,
        &crate::auth::types::EnvAuthContext,
        auth.env.as_ref(),
        opts.timeout_ms,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            sink.send(e.into_error_event(provider, model_id, Some(model.api.clone())))
                .await;
            return None;
        }
    };

    let capture = crate::stream::ResponseCapture::default();
    let on_resp = capture.sse_hook(opts);
    let frames = match open_sse(
        &client,
        req,
        cancel,
        None,
        on_resp,
        ProviderRetry::from_options(opts),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            sink.send(e.into_error_event(provider, model_id, Some(model.api.clone())))
                .await;
            return None;
        }
    };
    capture.fire(opts, model).await;
    Some(frames)
}
