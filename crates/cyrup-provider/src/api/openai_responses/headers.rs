//! Request encoding: the request headers (Pi `createClient`, openai-responses.ts:193-229).

use crate::HeaderMap;
use crate::api::compat::{SessionAffinityFormat, get_responses_compat};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::provider_plumbing::resolve_cache_retention;

/// Pi `hasHeader` (openai-responses.ts:28-35): a header is "present" only when set to a non-empty,
/// non-`None` value (case-insensitive name match).
pub(super) fn header_present(headers: Option<&HeaderMap>, name: &str) -> bool {
    let Some(map) = headers else { return false };
    let want = name.to_ascii_lowercase();
    map.iter().any(|(k, v)| {
        k.to_ascii_lowercase() == want
            && v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
    })
}

/// Build the request headers: `Authorization: Bearer <key>` plus the model / session / opts header
/// overlays (Pi `createClient`, openai-responses.ts:193-229). Precedence (low → high): auth Bearer
/// < `model.headers` < session affinity < `opts.headers`.
pub(super) fn build_headers(
    model: &Model,
    ctx: &Context,
    auth: &AuthResult,
    opts: &StreamOptions,
    api_key: &str,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization".to_string(),
        Some(format!("Bearer {api_key}")),
    );
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // `{ ...model.headers }` (openai-responses.ts:201).
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }

    // PROV-028: the per-request Copilot headers (openai-responses.ts:223-230), applied where Pi's
    // `Object.assign(headers, copilotHeaders)` sits — after `{ ...model.headers }`, before the
    // session headers and the opts overlay.
    crate::api::github_copilot_headers::apply_copilot_dynamic_headers(
        &mut headers,
        model.provider.as_str(),
        &ctx.messages,
    );

    // Session headers (openai-responses.ts:211-216). Gated on cache retention != none.
    let cache = resolve_cache_retention(opts.cache_retention, auth.env.as_ref());
    let compat = get_responses_compat(model);
    // PROV-033: the three-way branch (openai-responses.ts:233-241 @v0.83.0). The former
    // `send_session_id_header` gate was a flag pi DELETED (#6496, `packages/ai/CHANGELOG.md:168`),
    // and it made `x-session-id` — the only header OpenRouter reads — unreachable.
    if cache != CacheRetention::None
        && let Some(sid) = &opts.session_id
    {
        if compat.session_affinity_format == SessionAffinityFormat::Openrouter {
            headers.insert("x-session-id".to_string(), Some(sid.as_str().to_string()));
        } else {
            if compat.session_affinity_format == SessionAffinityFormat::Openai {
                headers.insert("session_id".to_string(), Some(sid.as_str().to_string()));
            }
            headers.insert(
                "x-client-request-id".to_string(),
                Some(sid.as_str().to_string()),
            );
        }
    }

    // Merge opts headers last so they override defaults (openai-responses.ts:219-221).
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}
