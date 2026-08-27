//! Request encoding — the `POST` target, the prompt-caching probe and the request headers
//! (Pi `new Mistral({ serverURL })` + the header overlay, mistral-conversations.ts:65-68).

use crate::HeaderMap;
use crate::auth::AuthResult;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};

/// Resolve the `POST` target (Pi `new Mistral({ serverURL: model.baseUrl })`,
/// mistral-conversations.ts:65-68). An auth base-url override wins over `model.base_url`. The
/// endpoint is `{base}/v1/chat/completions`.
pub(super) fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(chat_url(base))
}

/// Normalize a base URL to the `/v1/chat/completions` endpoint.
pub(super) fn chat_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/v1/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/chat/completions")
    }
}

/// `cacheRetention !== "none" && !!sessionId` (Pi `shouldUsePromptCaching`,
/// mistral-conversations.ts:270-272). cyrup's `cache_retention` defaults to `None` (unset), which —
/// like Pi's `undefined` — is `!== "none"`.
pub(super) fn should_use_prompt_caching(opts: &StreamOptions) -> Option<&str> {
    let retention_ok = opts.cache_retention != Some(CacheRetention::None);
    if retention_ok && let Some(sid) = &opts.session_id {
        return Some(sid.as_str());
    }
    None
}

/// Build the request headers (Pi `buildRequestOptions`, mistral-conversations.ts:213-238). Mistral
/// authenticates with `Authorization: Bearer`. `x-affinity` carries the session id for KV-cache
/// reuse. The model/opts header overlays layer last (a `None` value suppresses a default).
pub(super) fn build_headers(model: &Model, opts: &StreamOptions, api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    headers.insert(
        "authorization".to_string(),
        Some(format!("Bearer {api_key}")),
    );

    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }

    // `x-affinity` for prefix caching, only when not already set by an overlay
    // (mistral-conversations.ts:229-231).
    if let Some(sid) = should_use_prompt_caching(opts)
        && !headers.contains_key("x-affinity")
    {
        headers.insert("x-affinity".to_string(), Some(sid.to_string()));
    }

    headers
}
