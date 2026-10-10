//! Headers.

use super::*;

#[test]
fn sse_headers_match_upstream_and_auth_cannot_be_overridden() {
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
    // pi's default `originator: "pi"` (:1647 @f1b2e77f5) — the backend gates on this identity.
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
    // pi `headers.set("User-Agent", getPiUserAgent())` (`openai-codex-responses.ts:1626`
    // @v0.87.1): pi's own product token with Node's platform, `os.release()` and arch tokens — the
    // same builder the seven `cyrup (…)` adapters use (PROV-095).
    assert_eq!(
        h.get("User-Agent"),
        Some(&Some(crate::utils::user_agent::platform_user_agent("pi")))
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

/// PROV-143 — port of pi `0cf65d2bf` (#10429)'s "lets model and caller headers override
/// originator and User-Agent" (`test/openai-codex-stream.test.ts:800-847` @f1b2e77f5): a model
/// `originator` and a caller `user-agent` win over the seeded defaults, while a caller
/// `Authorization` / `chatgpt-account-id` is still replaced by the credential's values.
#[test]
fn prov143_model_and_caller_headers_override_originator_and_user_agent() {
    let mut model = codex_model("gpt-5.1-codex");
    model.headers = Some(
        [("originator".to_string(), Some("my-app".to_string()))]
            .into_iter()
            .collect(),
    );
    let auth = AuthResult::from_key("tok", "test");
    let opts = StreamOptions {
        headers: Some(
            [
                ("user-agent".to_string(), Some("my-app/1.0".to_string())),
                (
                    "Authorization".to_string(),
                    Some("Bearer ignored".to_string()),
                ),
                (
                    "chatgpt-account-id".to_string(),
                    Some("acct_ignored".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..StreamOptions::default()
    };
    let h = build_sse_headers(&model, &auth, &opts, "acct_1", "tok", None);

    // Case-insensitive lookup, as pi's `Headers.get` is: each name must resolve to ONE entry.
    let get = |name: &str| -> Vec<Option<String>> {
        h.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .collect()
    };
    assert_eq!(get("originator"), vec![Some("my-app".to_string())]);
    assert_eq!(get("User-Agent"), vec![Some("my-app/1.0".to_string())]);
    assert_eq!(get("Authorization"), vec![Some("Bearer tok".to_string())]);
    assert_eq!(get("chatgpt-account-id"), vec![Some("acct_1".to_string())]);
}

/// PROV-143 — the per-credential overlay sits in the same slot as `model.headers`, so it also
/// overrides the defaults, and a `None` overlay value deletes one (pi `headers.delete(key)`,
/// `:1652-1653` @f1b2e77f5).
#[test]
fn prov143_credential_overlay_overrides_and_a_none_overlay_deletes_the_default() {
    let model = codex_model("gpt-5.1-codex");
    let mut auth = AuthResult::from_key("tok", "test");
    auth.auth.headers = Some(
        [("originator".to_string(), Some("cred-app".to_string()))]
            .into_iter()
            .collect(),
    );
    let opts = StreamOptions {
        headers: Some([("User-Agent".to_string(), None)].into_iter().collect()),
        ..StreamOptions::default()
    };
    let h = build_sse_headers(&model, &auth, &opts, "acct_1", "tok", None);

    assert_eq!(h.get("originator"), Some(&Some("cred-app".to_string())));
    // `None` survives in the map as the deletion marker the sender honours; no default remains.
    assert_eq!(h.get("User-Agent"), Some(&None));
}
