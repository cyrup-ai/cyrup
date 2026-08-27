//! Request encoding: the `chat/completions` endpoint URL and the header overlays.

use crate::HeaderMap;
use crate::api::compat::{ResolvedCompat, SessionAffinityFormat};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::model::Model;
use crate::stream::StreamOptions;

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/chat/completions` (appended unless `base` already names it).
pub(super) fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(chat_completions_url(base))
}

/// Normalize a base URL to the `chat/completions` endpoint.
pub(crate) fn chat_completions_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// Build the request headers: `Authorization: Bearer <key>` plus the auth / model / opts header
/// overlays (a `None` value suppresses a default). Precedence (lowest → highest): auth overlay <
/// `model.headers` < session-affinity < `opts.headers` — matching Pi `createClient`
/// (openai-completions.ts:505-524), which seeds `{ ...model.headers }`, then layers session
/// affinity, then merges `optionsHeaders` last so per-request headers win.
///
/// `cache_session_id` is the cache-gated session id (Pi `cacheSessionId`): when the resolved
/// `send_session_affinity_headers` compat flag is set and a session id is available, the
/// `session_id` / `x-client-request-id` / `x-session-affinity` headers are injected (Pi
/// `createClient`, openai-completions.ts:515-519). They are placed after `model.headers` but
/// before the opts overlay so per-request headers can still override them (matching Pi, which
/// merges `optionsHeaders` last).
pub(crate) fn build_headers(
    model: &Model,
    ctx: &Context,
    auth: &AuthResult,
    opts: &StreamOptions,
    compat: &ResolvedCompat,
    cache_session_id: Option<&str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(key) = &auth.auth.api_key {
        headers.insert("Authorization".to_string(), Some(format!("Bearer {key}")));
    }
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // Top-level per-provider headers (Pi `createClient` seeds `{ ...model.headers }`,
    // openai-completions.ts:505). Layered above the auth overlay and below session affinity / opts
    // so a `None` value suppresses a default header.
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // PROV-028: the per-request Copilot headers, layered on top of `model.headers` exactly where Pi
    // puts `Object.assign(headers, copilotHeaders)` (openai-completions.ts:638-645) — after the
    // `{ ...model.headers }` seed and before session affinity / the opts overlay.
    crate::api::github_copilot_headers::apply_copilot_dynamic_headers(
        &mut headers,
        model.provider.as_str(),
        &ctx.messages,
    );
    // Session-affinity headers (Pi `createClient`, openai-completions.ts:647-656 @v0.83.0). The
    // enabling flag is `false` for every provider in `detect_compat`, but the emission is ported
    // for 1:1 parity so an explicit `model.compat.sendSessionAffinityHeaders` override takes
    // effect. PROV-024: the header SET is chosen by `sessionAffinityFormat`, not fixed — an
    // OpenRouter model reads `x-session-id` and none of the other three.
    if compat.send_session_affinity_headers
        && let Some(sid) = cache_session_id
    {
        if compat.session_affinity_format == SessionAffinityFormat::Openrouter {
            headers.insert("x-session-id".to_string(), Some(sid.to_string()));
        } else {
            if compat.session_affinity_format == SessionAffinityFormat::Openai {
                headers.insert("session_id".to_string(), Some(sid.to_string()));
            }
            headers.insert("x-client-request-id".to_string(), Some(sid.to_string()));
            headers.insert("x-session-affinity".to_string(), Some(sid.to_string()));
        }
    }
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}
