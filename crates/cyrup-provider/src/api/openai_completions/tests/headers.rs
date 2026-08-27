//! Request builder: the endpoint URL and the header overlays.

use super::*;

#[test]
fn url_appends_chat_completions() {
    assert_eq!(
        chat_completions_url("https://api.together.ai/v1"),
        "https://api.together.ai/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://api.together.ai/v1/"),
        "https://api.together.ai/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://x/v1/chat/completions"),
        "https://x/v1/chat/completions"
    );
}

#[test]
fn headers_set_bearer_auth() {
    let compat = get_compat(&model());
    let headers = build_headers(
        &model(),
        &Context::default(),
        &auth_with_key(),
        &StreamOptions::default(),
        &compat,
        None,
    );
    assert_eq!(
        headers.get("Authorization"),
        Some(&Some("Bearer sk-xyz".to_string()))
    );
    // No session-affinity headers: the flag is false for `together` and there is no session id.
    assert!(!headers.contains_key("session_id"));
}

/// `model.headers` merge precedence (Pi `createClient`, openai-completions.ts:505-524):
/// auth overlay < `model.headers` < `opts.headers`, and a `None` value suppresses a default.
#[test]
fn model_headers_merge_precedence_and_suppression() {
    let compat = get_compat(&model());
    let mut auth = auth_with_key();
    auth.auth.headers = Some(crate::HeaderMap::from([
        ("X-All".to_string(), Some("auth".to_string())),
        ("X-AM".to_string(), Some("auth".to_string())),
        ("X-Auth".to_string(), Some("a".to_string())),
        ("X-Drop".to_string(), Some("keep".to_string())),
    ]));
    let mut m = model();
    m.headers = Some(crate::HeaderMap::from([
        ("X-All".to_string(), Some("model".to_string())),
        ("X-AM".to_string(), Some("model".to_string())),
        ("X-Model".to_string(), Some("m".to_string())),
        ("X-Drop".to_string(), None), // suppress the auth default
    ]));
    let opts = StreamOptions {
        headers: Some(crate::HeaderMap::from([(
            "X-All".to_string(),
            Some("opts".to_string()),
        )])),
        ..Default::default()
    };
    let headers = build_headers(&m, &Context::default(), &auth, &opts, &compat, None);
    // opts wins the key present at all three layers.
    assert_eq!(headers.get("X-All"), Some(&Some("opts".to_string())));
    // model overrides auth on a key present at both (and not in opts).
    assert_eq!(headers.get("X-AM"), Some(&Some("model".to_string())));
    // Layer-exclusive keys survive.
    assert_eq!(headers.get("X-Auth"), Some(&Some("a".to_string())));
    assert_eq!(headers.get("X-Model"), Some(&Some("m".to_string())));
    // `None` from model.headers suppresses the auth default (carried as `Some(None)`).
    assert_eq!(headers.get("X-Drop"), Some(&None));
    // Bearer auth still present.
    assert_eq!(
        headers.get("Authorization"),
        Some(&Some("Bearer sk-xyz".to_string()))
    );
}

/// Build a `{assistant with N tool calls} + {N tool results}` context for the given raw ids.
/// PROV-028 — `buildCopilotDynamicHeaders` on the chat-completions route
/// (openai-completions.ts:638-645). Copilot's Fable/Kimi rows ride this api.
#[test]
fn copilot_dynamic_headers_on_the_completions_route() {
    let mut m = model();
    m.provider = "github-copilot".into();
    let compat = get_compat(&m);

    let ctx = Context {
        system_prompt: None,
        messages: vec![cyrup_core::Message::User {
            content: vec![cyrup_core::Content::Image {
                data: "aGk=".to_string(),
                mime_type: "image/png".to_string(),
            }],
            timestamp: 0,
        }],
        tools: vec![],
    };
    let headers = build_headers(
        &m,
        &ctx,
        &auth_with_key(),
        &StreamOptions::default(),
        &compat,
        None,
    );
    assert_eq!(headers.get("X-Initiator"), Some(&Some("user".to_string())));
    assert_eq!(
        headers.get("Openai-Intent"),
        Some(&Some("conversation-edits".to_string()))
    );
    assert_eq!(
        headers.get("Copilot-Vision-Request"),
        Some(&Some("true".to_string())),
        "an image turn requires the vision header or Copilot rejects the request"
    );

    // Non-Copilot providers get none of them.
    let plain = build_headers(
        &model(),
        &ctx,
        &auth_with_key(),
        &StreamOptions::default(),
        &get_compat(&model()),
        None,
    );
    assert!(!plain.contains_key("X-Initiator"));
    assert!(!plain.contains_key("Copilot-Vision-Request"));
}

// Gap 3: Pi `createClient` (openai-completions.ts:515-519) — session-affinity headers are
// injected when the compat flag is set and a (cache-gated) session id is available.
#[test]
fn session_affinity_headers_emitted_when_flag_and_session_set() {
    let mut m = model();
    m.compat = Some(crate::api::compat::OpenAiCompletionsCompat {
        send_session_affinity_headers: Some(true),
        ..Default::default()
    });
    let compat = get_compat(&m);
    let headers = build_headers(
        &m,
        &Context::default(),
        &auth_with_key(),
        &StreamOptions::default(),
        &compat,
        Some("sess-7"),
    );
    assert_eq!(headers.get("session_id"), Some(&Some("sess-7".to_string())));
    assert_eq!(
        headers.get("x-client-request-id"),
        Some(&Some("sess-7".to_string()))
    );
    assert_eq!(
        headers.get("x-session-affinity"),
        Some(&Some("sess-7".to_string()))
    );
    assert!(
        !headers.contains_key("x-session-id"),
        "the openai form never sends OpenRouter's header"
    );

    // Flag off (default for every provider) => not emitted even with a session id present.
    let compat_off = get_compat(&model());
    let headers = build_headers(
        &model(),
        &Context::default(),
        &auth_with_key(),
        &StreamOptions::default(),
        &compat_off,
        Some("sess-7"),
    );
    assert!(!headers.contains_key("session_id"));

    // Flag on but no session id => not emitted.
    let headers = build_headers(
        &m,
        &Context::default(),
        &auth_with_key(),
        &StreamOptions::default(),
        &compat,
        None,
    );
    assert!(!headers.contains_key("session_id"));
}

/// PROV-024. `sessionAffinityFormat` selects the header SET
/// (openai-completions.ts:647-656 @v0.83.0); the detector is `isOpenRouter ? "openrouter" :
/// "openai"` (`:1473`) and the catalog override resolves at `:1515`. Red before the fix:
/// cyrup emitted the fixed OpenAI triple with no provider branch, so an OpenRouter completions
/// model got three headers it does not read and never got the one it does.
#[test]
fn session_affinity_format_selects_the_completions_header_set() {
    let headers_for = |m: &Model| {
        let compat = get_compat(m);
        build_headers(
            m,
            &Context::default(),
            &auth_with_key(),
            &StreamOptions::default(),
            &compat,
            Some("sess-7"),
        )
    };

    // OpenRouter, detected from the provider id: `x-session-id` ONLY.
    let mut router = model();
    router.provider = "openrouter".into();
    router.compat = Some(crate::api::compat::OpenAiCompletionsCompat {
        send_session_affinity_headers: Some(true),
        ..Default::default()
    });
    let h = headers_for(&router);
    assert_eq!(h.get("x-session-id"), Some(&Some("sess-7".to_string())));
    assert!(!h.contains_key("session_id"));
    assert!(!h.contains_key("x-client-request-id"));
    assert!(!h.contains_key("x-session-affinity"));

    // "openai-nosession": the pair WITHOUT `session_id` — pi's documented migration target for
    // the `sendSessionIdHeader: false` flag it deleted (packages/ai/CHANGELOG.md:168).
    let mut nos = model();
    nos.compat = Some(crate::api::compat::OpenAiCompletionsCompat {
        send_session_affinity_headers: Some(true),
        session_affinity_format: Some(crate::api::compat::SessionAffinityFormat::OpenaiNosession),
        ..Default::default()
    });
    let h = headers_for(&nos);
    assert!(!h.contains_key("session_id"));
    assert_eq!(
        h.get("x-client-request-id"),
        Some(&Some("sess-7".to_string()))
    );
    assert_eq!(
        h.get("x-session-affinity"),
        Some(&Some("sess-7".to_string()))
    );
    assert!(!h.contains_key("x-session-id"));
}
