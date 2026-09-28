//! Endpoint and header encoding.

use super::*;

#[test]
fn url_appends_chat_completions() {
    assert_eq!(
        chat_url("https://api.mistral.ai"),
        "https://api.mistral.ai/v1/chat/completions"
    );
    assert_eq!(
        chat_url("https://api.mistral.ai/"),
        "https://api.mistral.ai/v1/chat/completions"
    );
}

#[test]
fn headers_use_bearer_and_affinity() {
    let m = model_with("mistral-medium-2604", true);
    let opts = StreamOptions {
        session_id: Some(SessionId::from("sess-1")),
        ..Default::default()
    };
    let headers = build_headers(&m, &opts, "sk-mistral");
    assert_eq!(
        headers
            .get("authorization")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("Bearer sk-mistral")
    );
    // x-affinity set from the session id (default cacheRetention is not "none").
    assert_eq!(
        headers.get("x-affinity").and_then(|v| v.clone()).as_deref(),
        Some("sess-1")
    );
}

/// PROV-095 — pi `new Headers({ "User-Agent": getPiUserAgent(), … })` before both overrides
/// (mistral-conversations.ts:337-345 @v0.87.1, #8305): a default client User-Agent under every
/// overlay.
#[test]
fn default_user_agent_sits_under_the_overlays() {
    let m = model_with("mistral-medium-2604", true);
    crate::utils::user_agent::assert_default_user_agent_under_overlays(|overlay| {
        let opts = StreamOptions {
            headers: overlay,
            ..Default::default()
        };
        build_headers(&m, &opts, "sk-mistral")
    });
}
