//! DRIFT-001: OAuth canonicalization and the `supportsToolReferences` gate.

use super::*;

#[test]
fn oauth_canonicalized_markers_match_active_tools() {
    // Pi "matches OAuth-canonicalized markers to active tools": marker "Read", tool "read".
    let ctx = deferred_ctx(vec![tool_def("base_tool"), tool_def("read")], &["Read"]);
    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::None),
        ..Default::default()
    };
    let body = build_params(&opus_4_6(), &ctx, &opts, None, true).unwrap();

    assert_eq!(tool_names(&body), ["base_tool", "Read"]);
    assert_eq!(body["tools"][1]["defer_loading"], json!(true));
    assert_eq!(
        tool_result_content(&body)[0]["content"],
        json!([{ "type": "tool_reference", "tool_name": "Read" }])
    );
}

#[test]
fn oauth_names_are_normalized_before_the_prior_usage_check() {
    // Pi "normalizes OAuth names before checking prior tool usage": the call is `Read`, the
    // marker is `read` — same tool after canonicalization, so nothing defers.
    let mut ctx = deferred_ctx(vec![tool_def("base_tool"), tool_def("read")], &["read"]);
    ctx.messages[1] = tc_assistant(&[("call_1", "Read")]);
    let body = build_params(&opus_4_6(), &ctx, &StreamOptions::default(), None, true).unwrap();

    assert_eq!(tool_names(&body), ["base_tool", "Read"]);
    assert!(
        body["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|t| t.get("defer_loading").is_none())
    );
    assert!(
        !serde_json::to_string(&body)
            .expect("json")
            .contains("tool_reference")
    );
}

#[test]
fn oauth_dedupe_collapses_case_variants_even_with_the_flag_off() {
    // Pi "deduplicates active tools after OAuth canonicalization". The unique-map collapse in
    // `splitDeferredTools` runs BEFORE the `!enabled` early return, so this lands on a model
    // with tool references OFF too. It is the ONE behavior change that is not gated by the
    // flag — and it is reachable only under OAuth, where the normalizer is not the identity.
    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text("Hello")],
            timestamp: 1,
        }],
        tools: vec![
            tool_def("read"),
            ToolDef {
                name: "Read".to_string(),
                description: "Canonical definition".to_string(),
                parameters: json!({ "type": "object", "properties": {}, "required": [] }),
                constrained_sampling: None,
            },
        ],
    };
    // Tool references ON (opus 4.6).
    let body = build_params(&opus_4_6(), &ctx, &StreamOptions::default(), None, true).unwrap();
    assert_eq!(tool_names(&body), ["Read"]);
    assert_eq!(body["tools"][0]["description"], "Canonical definition");

    // ...and OFF (haiku): still deduped.
    let haiku = Model {
        id: "claude-haiku-4-5".into(),
        ..model()
    };
    let body = build_params(&haiku, &ctx, &StreamOptions::default(), None, true).unwrap();
    assert_eq!(tool_names(&body), ["Read"]);

    // Non-OAuth normalizer is the identity, so both survive — no silent collapse.
    let body = build_params(&opus_4_6(), &ctx, &StreamOptions::default(), None, false).unwrap();
    assert_eq!(tool_names(&body), ["read", "Read"]);
}

#[test]
fn unsupported_models_emit_the_plain_tool_list() {
    // Pi "uses the normal tool list when Anthropic tool references are unsupported". The
    // second id is the date-suffix trap: `claude-sonnet-4-20250514` captures "20250514"
    // (8 chars) as the minor group, which the `< 8` guard folds to 0.
    let ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    for id in ["claude-haiku-4-5", "claude-sonnet-4-20250514"] {
        let m = Model {
            id: id.into(),
            ..model()
        };
        let body = build_body(&m, &ctx, &StreamOptions::default());
        assert_eq!(tool_names(&body), ["base_tool", "late_tool"], "{id}");
        assert!(
            body["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .all(|t| t.get("defer_loading").is_none()),
            "{id}"
        );
        assert!(
            !serde_json::to_string(&body)
                .expect("json")
                .contains("tool_reference"),
            "{id}"
        );
        // ...and the tool result keeps its content inline, exactly as before DRIFT-001.
        assert_eq!(tool_result_content(&body)[0]["content"], json!("done"));
    }
}

#[test]
fn an_explicit_compat_override_enables_a_non_anthropic_provider() {
    // Pi "supports explicit Anthropic compatibility overrides": the override wins over the
    // provider gate, so a proxy fronting Claude can opt in.
    let m = Model {
        id: "claude-opus-4-6".into(),
        provider: ProviderId::from("anthropic-proxy"),
        compat: Some(ModelCompat {
            supports_tool_references: Some(true),
            ..Default::default()
        }),
        ..model()
    };
    assert!(
        !default_supports_tool_references(&m),
        "the default gate says no"
    );
    let ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    let body = build_body(&m, &ctx, &StreamOptions::default());
    assert_eq!(body["tools"][1]["defer_loading"], json!(true));
    assert_eq!(
        tool_result_content(&body)[0]["content"],
        json!([{ "type": "tool_reference", "tool_name": "late_tool" }])
    );

    // ...and the override can force it OFF on a model the default enables.
    let off = Model {
        compat: Some(ModelCompat {
            supports_tool_references: Some(false),
            ..Default::default()
        }),
        ..opus_4_6()
    };
    let body = build_body(&off, &ctx, &StreamOptions::default());
    assert!(body["tools"][1].get("defer_loading").is_none());
}
