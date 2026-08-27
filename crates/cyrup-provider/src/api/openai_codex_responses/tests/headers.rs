//! Headers.

use super::*;

#[test]
fn sse_headers_match_upstream_and_cannot_be_overridden() {
    let mut model = codex_model("gpt-5.5-codex");
    // A differently-cased override of a Codex identity header must be REPLACED, not duplicated
    // (pi uses `Headers`, which is case-insensitive).
    model.headers = Some(
        [
            (
                "authorization".to_string(),
                Some("Bearer stale".to_string()),
            ),
            ("x-extra".to_string(), Some("kept".to_string())),
        ]
        .into_iter()
        .collect(),
    );
    let auth = AuthResult::from_key("tok", "test");
    let h = build_sse_headers(
        &model,
        &auth,
        &StreamOptions::default(),
        "acct_1",
        "tok",
        Some("sess-1"),
    );

    assert_eq!(
        h.get("Authorization"),
        Some(&Some("Bearer tok".to_string()))
    );
    assert!(!h.contains_key("authorization"), "duplicate cased header");
    assert_eq!(
        h.get("chatgpt-account-id"),
        Some(&Some("acct_1".to_string()))
    );
    // pi `headers.set("originator", "pi")` (:1593) — the backend gates on this identity.
    assert_eq!(h.get("originator"), Some(&Some("pi".to_string())));
    assert_eq!(
        h.get("OpenAI-Beta"),
        Some(&Some("responses=experimental".to_string()))
    );
    assert_eq!(
        h.get("accept"),
        Some(&Some("text/event-stream".to_string()))
    );
    assert_eq!(
        h.get("content-type"),
        Some(&Some("application/json".to_string()))
    );
    assert_eq!(h.get("session-id"), Some(&Some("sess-1".to_string())));
    assert_eq!(
        h.get("x-client-request-id"),
        Some(&Some("sess-1".to_string()))
    );
    // A non-conflicting model header survives.
    assert_eq!(h.get("x-extra"), Some(&Some("kept".to_string())));
    assert!(
        h.get("User-Agent")
            .and_then(|v| v.clone())
            .is_some_and(|v| v.starts_with("pi (")),
        "user agent: {:?}",
        h.get("User-Agent")
    );
}

#[test]
fn session_headers_are_omitted_without_a_session() {
    let model = codex_model("gpt-5.5-codex");
    let auth = AuthResult::from_key("tok", "test");
    let h = build_sse_headers(
        &model,
        &auth,
        &StreamOptions::default(),
        "acct",
        "tok",
        None,
    );
    assert!(!h.contains_key("session-id"));
    assert!(!h.contains_key("x-client-request-id"));
}

