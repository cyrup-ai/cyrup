//! Endpoint and header encoding.

use super::*;

#[test]
fn url_appends_streaming_endpoint() {
    assert_eq!(
        stream_url(
            "https://generativelanguage.googleapis.com/v1beta",
            "gemini-2.5-pro"
        ),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
    );
    assert_eq!(
        stream_url("https://host/v1beta/", "gemini-2.0-flash"),
        "https://host/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse"
    );
}

#[test]
fn headers_use_goog_api_key() {
    let m = model_with("gemini-2.0-flash", false);
    let headers = build_headers(&m, &StreamOptions::default(), "test-key");
    assert_eq!(
        headers
            .get("x-goog-api-key")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("test-key")
    );
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("application/json")
    );
}

/// PROV-095 — pi `{ "User-Agent": getPiUserAgent(), ...model.headers, ...optionsHeaders }`
/// (google-generative-ai.ts:357 @v0.87.1, #8305): a default client User-Agent under every overlay.
#[test]
fn default_user_agent_sits_under_the_overlays() {
    let m = model_with("gemini-2.0-flash", false);
    crate::utils::user_agent::assert_default_user_agent_under_overlays(|overlay| {
        let opts = StreamOptions {
            headers: overlay,
            ..Default::default()
        };
        build_headers(&m, &opts, "test-key")
    });
}

/// PROV-126 — pi `providerHeadersToRecord` (google-generative-ai.ts:358, utils/headers.ts:11-23
/// @ce950d78f) merges case-insensitively: a differently-cased overlay replaces the lowercase default
/// instead of riding beside it, keeps the overlay's spelling, and a differently-cased `None` removes
/// it. Red before PROV-126 (two `x-goog-api-key` headers).
#[test]
fn differently_cased_overlays_replace_the_default_header() {
    let mut m = model_with("gemini-2.0-flash", false);
    m.headers = Some(crate::HeaderMap::from([(
        "X-Goog-Api-Key".to_string(),
        Some("model-key".to_string()),
    )]));
    let opts = StreamOptions {
        headers: Some(crate::HeaderMap::from([("Content-Type".to_string(), None)])),
        ..Default::default()
    };
    let headers = build_headers(&m, &opts, "test-key");
    let named = |name: &str| -> Vec<(String, Option<String>)> {
        headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    assert_eq!(
        named("x-goog-api-key"),
        vec![("X-Goog-Api-Key".to_string(), Some("model-key".to_string()))]
    );
    assert_eq!(
        named("content-type"),
        vec![("Content-Type".to_string(), None)]
    );
}
