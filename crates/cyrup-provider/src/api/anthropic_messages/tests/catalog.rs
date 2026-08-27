//! DRIFT-001: the `supportsToolReferences` gate across the embedded catalogs.

use super::*;

#[test]
fn default_supports_tool_references_parses_versions_like_pis_regex() {
    let probe = |id: &str, provider: &str| {
        default_supports_tool_references(&Model {
            id: id.into(),
            provider: ProviderId::from(provider),
            ..model()
        })
    };
    // major > 4, or major == 4 && minor >= 5.
    for id in [
        "claude-sonnet-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-6",
        "claude-sonnet-5",
        "claude-opus-4-5",
        "claude-opus-4-5-20251101",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-fable-5",
    ] {
        assert!(probe(id, "anthropic"), "expected ON: {id}");
    }
    for id in [
        "claude-opus-4-1",            // minor 1 < 5
        "claude-opus-4-1-20250805",   // minor 1 < 5
        "claude-opus-4",              // minor absent → 0
        "claude-sonnet-4-20250514",   // 8-char date captured as minor → folded to 0
        "claude-haiku-4-5",           // haiku gate
        "claude-haiku-5",             // haiku gate
        "claude-3-5-sonnet-20241022", // family not at the anchored position
        "claude-mythos-5",            // unknown family
        "claude-opus-x-5",            // no major digits
        "claude-opus-45x",            // major run not followed by `-` or end
        "opus-5",                     // missing `claude-` prefix
    ] {
        assert!(!probe(id, "anthropic"), "expected OFF: {id}");
    }
    // The provider gate is exact-match: every reseller stays off on a byte-identical id.
    for p in [
        "vercel-ai-gateway",
        "cloudflare-ai-gateway",
        "fireworks",
        "opencode",
        "opencode-go",
        "kimi-coding",
        "minimax",
        "minimax-cn",
        "anthropic-proxy",
    ] {
        assert!(
            !probe("claude-opus-4-6", p),
            "expected OFF for provider {p}"
        );
    }
}

/// Constraint 3, proven against the REAL embedded catalogs rather than hand-built models.
#[test]
fn tool_references_default_off_across_every_embedded_catalog() {
    use crate::providers::all::all_providers;

    const EXPECTED_ON: [&str; 10] = [
        "claude-fable-5",
        "claude-opus-4-5",
        "claude-opus-4-5-20251101",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-6",
        "claude-sonnet-5",
    ];

    let mut on: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut providers_with_on: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for provider in all_providers() {
        for m in provider.models() {
            if m.api.as_str() != API_ID {
                continue;
            }
            total += 1;
            if get_anthropic_compat(m).supports_tool_references {
                on.push(m.id.as_str().to_string());
                providers_with_on.insert(m.provider.as_str().to_string());
            }
        }
    }
    on.sort();
    on.dedup();

    assert!(
        total > 200,
        "expected the real catalogs, saw {total} models"
    );
    assert_eq!(
        on,
        EXPECTED_ON,
        "the wire-payload blast radius of DRIFT-001 changed; \
         {} of {total} anthropic-messages models are ON",
        on.len()
    );
    assert_eq!(
        providers_with_on.into_iter().collect::<Vec<_>>(),
        ["anthropic"],
        "only the first-party Anthropic provider may enable tool references"
    );
}

/// The Responses half of the same flag: catalog-driven, `?? false`, and enabled ONLY on the
/// seven first-party OpenAI ids Pi's generator marks (`generate-models.ts:324-332`). Asserted
/// here from the Anthropic side so that a catalog edit which leaked the flag onto an
/// `anthropic-messages` model — where nothing reads it and it would be pure confusion — fails
/// loudly. The exhaustive on/off partition lives with the rendering, in
/// `openai_responses::tests::tool_search_is_off_for_every_openai_responses_model_but_the_seven`.
#[test]
fn tool_search_is_confined_to_the_openai_responses_catalog() {
    use crate::api::compat::get_responses_compat;
    use crate::providers::all::all_providers;

    let mut total = 0usize;
    let mut on: Vec<String> = Vec::new();
    for provider in all_providers() {
        for m in provider.models() {
            total += 1;
            if !get_responses_compat(m).supports_tool_search {
                continue;
            }
            assert_ne!(
                m.api.as_str(),
                API_ID,
                "{}/{} sets supportsToolSearch on an anthropic-messages model, where it is \
                 never read",
                m.provider.as_str(),
                m.id.as_str()
            );
            on.push(format!("{}/{}", m.provider.as_str(), m.id.as_str()));
        }
    }
    on.sort();
    assert_eq!(
        on,
        [
            // openai-codex, ported in the unported-work sweep. Its catalog carries the same
            // `supportsToolSearch` rows as `openai`, on the same `openai-responses` wire API —
            // the assertion is that tool-search stays confined to that API, not to one
            // provider, so a second responses-based provider legitimately widens this list.
            "openai-codex/gpt-5.4",
            "openai-codex/gpt-5.4-mini",
            "openai-codex/gpt-5.5",
            "openai-codex/gpt-5.6-luna",
            "openai-codex/gpt-5.6-sol",
            "openai-codex/gpt-5.6-terra",
            "openai/gpt-5.4",
            "openai/gpt-5.4-mini",
            "openai/gpt-5.4-pro",
            "openai/gpt-5.5",
            "openai/gpt-5.6-luna",
            "openai/gpt-5.6-sol",
            "openai/gpt-5.6-terra",
        ],
        "the tool-search blast radius changed"
    );
    assert!(
        total > 600,
        "expected the real catalogs, saw {total} models"
    );
    // ...and the flag is honored when a catalog/override does set it.
    let m = Model {
        compat: Some(ModelCompat {
            supports_tool_search: Some(true),
            ..Default::default()
        }),
        ..model()
    };
    assert!(get_responses_compat(&m).supports_tool_search);
}

/// Regression guard: with no `addedToolNames` anywhere, the payload must be byte-identical to
/// the pre-DRIFT-001 shape even on a model where the flag is ON.
#[test]
fn an_unmarked_transcript_is_byte_identical_on_a_flag_on_model() {
    let ctx = deferred_ctx(vec![tool_def("base_tool"), tool_def("late_tool")], &[]);
    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::None),
        ..Default::default()
    };
    let on = build_body(&opus_4_6(), &ctx, &opts);
    let off = build_body(
        &Model {
            id: "claude-haiku-4-5".into(),
            ..model()
        },
        &ctx,
        &opts,
    );
    assert_eq!(on["tools"], off["tools"]);
    assert_eq!(on["messages"], off["messages"]);
    let s = serde_json::to_string(&on).expect("json");
    assert!(!s.contains("defer_loading"));
    assert!(!s.contains("tool_reference"));
}
