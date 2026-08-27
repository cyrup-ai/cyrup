//! Request encoding — the `:streamGenerateContent` endpoint and the request headers
//! (Pi `createClient`, google-generative-ai.ts:321-340).

use crate::HeaderMap;
use crate::auth::AuthResult;
use crate::model::Model;
use crate::stream::StreamOptions;

/// Resolve the `POST` target (Pi `createClient` httpOptions.baseUrl, google-generative-ai.ts:326).
/// An auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/models/{model}:streamGenerateContent?alt=sse`.
pub(super) fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(stream_url(base, model.id.as_str()))
}

/// Normalize a base URL to the streaming-generate endpoint.
pub(super) fn stream_url(base: &str, model_id: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/models/{model_id}:streamGenerateContent?alt=sse")
}

/// Build the request headers (Pi `createClient`, google-generative-ai.ts:321-340). The Gemini REST
/// API authenticates with the `x-goog-api-key` header. The model/opts header overlays layer last (a
/// `None` value suppresses a default).
pub(super) fn build_headers(model: &Model, opts: &StreamOptions, api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    headers.insert("x-goog-api-key".to_string(), Some(api_key.to_string()));

    // model.headers < opts.headers (a `None` suppresses a default — Pi `providerHeadersToRecord`,
    // google-generative-ai.ts:331).
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
    headers
}
