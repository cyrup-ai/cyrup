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
