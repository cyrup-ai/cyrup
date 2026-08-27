//! URL / session.

use super::DEFAULT_CODEX_BASE_URL;
use crate::api::compat::clamp_openai_prompt_cache_key;
use crate::auth::AuthResult;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};

/// The base URL a request targets. Upstream reads `model.baseUrl` (`:315`, `:405`); cyrup splits
/// pi's single `model.baseUrl` into the catalog value plus a per-credential override, so the
/// override wins here exactly as it does in [`openai_responses`](crate::api::openai_responses).
pub(super) fn resolved_base_url<'a>(model: &'a Model, auth: &'a AuthResult) -> &'a str {
    auth.auth
        .base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(model.base_url.as_str())
}

/// 1:1 port of pi `resolveCodexUrl` (`openai-codex-responses.ts:637-643`): a blank base falls back
/// to [`DEFAULT_CODEX_BASE_URL`], trailing slashes are trimmed, and the path is completed to
/// `/codex/responses` without ever doubling a segment the caller already supplied.
pub(super) fn resolve_codex_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        return normalized.to_string();
    }
    if normalized.ends_with("/codex") {
        return format!("{normalized}/responses");
    }
    format!("{normalized}/codex/responses")
}

/// pi `:281-282`: the cache-scoped session id, clamped to the OpenAI prompt-cache key length.
/// `cacheRetention === "none"` drops it entirely.
pub(super) fn codex_session_id(opts: &StreamOptions) -> Option<String> {
    if opts.cache_retention == Some(CacheRetention::None) {
        return None;
    }
    opts.session_id
        .as_ref()
        .map(|s| clamp_openai_prompt_cache_key(s.as_str()))
}
