//! Request headers: bearer, session affinity and the Copilot overlay.

use super::*;

/// PROV-055 — the wire half, driven by the SHIPPED catalog rather than a synthetic model.
///
/// The logic below was already a faithful port; what leaked the header was the DATA. pi builds
/// every OpenCode Zen `@ai-sdk/openai` variant with
/// `compat = { sessionAffinityFormat: "openai-nosession" }`
/// (`ai/scripts/generate-models.ts:1666` @v0.83.0) and this route's gate is `if (sessionId)`
/// with no `sendSessionAffinityHeaders` guard (`api/openai-responses.ts:232-241`), so the
/// format alone decides the headers. cyrup's rows carried `compat: null`, the detector
/// (`api/compat.rs:84-92`) answered `Openai` for provider `opencode`, and cyrup sent a
/// `session_id` — a session identifier leaked to OpenCode Zen on every request that carries
/// one. Pre-fix this is RED on all 19 rows.
#[test]
fn opencode_responses_rows_never_emit_a_session_id_header() {
    let opts = StreamOptions {
        session_id: Some("sess-7".into()),
        ..Default::default()
    };
    let rows: Vec<Model> = crate::providers::opencode::opencode_models()
        .into_iter()
        .filter(|m| m.api.as_str() == "openai-responses")
        .collect();
    assert_eq!(rows.len(), 19, "scope must be the whole api, not a fixed id list");

    for m in &rows {
        let h = build_headers(m, &Context::default(), &auth(), &opts, "sk");
        assert!(
            !h.contains_key("session_id"),
            "opencode/{} leaked session_id",
            m.id.as_str()
        );
        // MIRROR: the OTHER affinity header is still sent, so this cannot be satisfied by
        // dropping session affinity for the provider altogether.
        assert_eq!(
            h.get("x-client-request-id"),
            Some(&Some("sess-7".to_string())),
            "opencode/{} lost x-client-request-id",
            m.id.as_str()
        );
    }
}

/// PROV-033. The three-way session-affinity branch (openai-responses.ts:233-241 @v0.83.0).
/// Red before the fix: `x-session-id` was unreachable on this route and the deleted
/// `sendSessionIdHeader` flag was the only knob.
#[test]
fn session_affinity_format_selects_the_header_set() {
    let opts = StreamOptions {
        session_id: Some("sess-7".into()),
        ..Default::default()
    };
    // openrouter detected from the base url ⇒ x-session-id ONLY.
    let mut router = model();
    router.base_url = "https://openrouter.ai/api/v1".to_string();
    let h = build_headers(&router, &Context::default(), &auth(), &opts, "sk");
    assert_eq!(h.get("x-session-id"), Some(&Some("sess-7".to_string())));
    assert!(!h.contains_key("session_id"));
    assert!(!h.contains_key("x-client-request-id"));

    // "openai-nosession" ⇒ x-client-request-id only.
    let mut nos = model();
    nos.compat = Some(ModelCompat {
        session_affinity_format: Some(SessionAffinityFormat::OpenaiNosession),
        ..Default::default()
    });
    let h = build_headers(&nos, &Context::default(), &auth(), &opts, "sk");
    assert!(!h.contains_key("session_id"));
    assert_eq!(
        h.get("x-client-request-id"),
        Some(&Some("sess-7".to_string()))
    );
    assert!(!h.contains_key("x-session-id"));
}

#[test]
fn headers_carry_bearer_and_session() {
    let m = model();
    let opts = StreamOptions {
        session_id: Some("sess-7".into()),
        ..Default::default()
    };
    let h = build_headers(&m, &Context::default(), &auth(), &opts, "sk-test");
    assert_eq!(
        h.get("Authorization"),
        Some(&Some("Bearer sk-test".to_string()))
    );
    assert_eq!(h.get("session_id"), Some(&Some("sess-7".to_string())));
    assert_eq!(
        h.get("x-client-request-id"),
        Some(&Some("sess-7".to_string()))
    );
}

/// PROV-028 — `buildCopilotDynamicHeaders` on the `/responses` route
/// (openai-responses.ts:223-230). Copilot's GPT/Gemini/MAI families ride this api.
#[test]
fn copilot_dynamic_headers_on_the_responses_route() {
    let mut m = model();
    m.provider = "github-copilot".into();

    let ctx = user_ctx("hi");
    let h = build_headers(&m, &ctx, &auth(), &StreamOptions::default(), "tid=abc");
    assert_eq!(h.get("X-Initiator"), Some(&Some("user".to_string())));
    assert_eq!(
        h.get("Openai-Intent"),
        Some(&Some("conversation-edits".to_string()))
    );
    assert!(!h.contains_key("Copilot-Vision-Request"));

    // Non-Copilot providers get none of them.
    let plain = build_headers(
        &model(),
        &ctx,
        &auth(),
        &StreamOptions::default(),
        "sk-test",
    );
    assert!(!plain.contains_key("X-Initiator"));
    assert!(!plain.contains_key("Openai-Intent"));
}
