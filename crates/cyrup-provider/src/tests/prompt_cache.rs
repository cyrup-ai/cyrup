//! `SEAM-131` groundwork — the prompt-cache lifetime the warmer schedules against.
//!
//! Upstream: `packages/ai/scripts/generate-models.ts:979-992` (the `ANTHROPIC_PROMPT_CACHE`
//! stamp) and `packages/coding-agent/src/core/cache-warmer.ts:33-47`
//! (`getPromptCacheTtlMs`), read at **v1.0.4** through git objects.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::model::ModelPromptCache;
use crate::{CacheRetention, Model, ProviderEnv, prompt_cache_ttl_ms};

/// A catalog row as JSON, so the test exercises `load_catalog`'s real parse-and-stamp path rather
/// than a hand-built struct.
fn row(provider: &str, api: &str, extra: &str) -> String {
    format!(
        r#"[{{"id":"m","name":"M","api":"{api}","provider":"{provider}","baseUrl":"https://x",
             "reasoning":false,"input":["text"],
             "cost":{{"input":1.0,"output":1.0,"cacheRead":0.1,"cacheWrite":1.25}},
             "contextWindow":200000,"maxTokens":8192{extra}}}]"#
    )
}

fn one(json: &str) -> Model {
    crate::catalog::load_catalog(json)
        .expect("catalog parses")
        .pop()
        .expect("one row")
}

// ------------------------------------------------------- the generator-time stamp --------------

/// `applyPromptCacheMetadata`: direct Anthropic gets the two tiers, in seconds.
#[test]
fn direct_anthropic_rows_carry_the_five_minute_and_one_hour_tiers() {
    let m = one(&row("anthropic", "anthropic-messages", ""));
    assert_eq!(
        m.prompt_cache,
        Some(ModelPromptCache {
            short: Some(300),
            long: Some(3600)
        }),
        "ANTHROPIC_PROMPT_CACHE = {{ short: 300, long: 3600 }}"
    );
}

/// Upstream's explicit reason: *"Only direct Anthropic is annotated so cache warming does not
/// assume equivalent behavior through proxies."* A Claude model reached through a gateway is a
/// different provider id and must stay unannotated.
#[test]
fn a_claude_model_through_a_gateway_is_not_annotated() {
    let m = one(&row("openrouter", "openai-completions", ""));
    assert!(
        m.prompt_cache.is_none(),
        "a proxied model must not inherit Anthropic's lifetimes"
    );
}

/// The same api id under a different provider is still not direct Anthropic.
#[test]
fn the_anthropic_api_under_another_provider_is_not_annotated() {
    let m = one(&row("bedrock", "anthropic-messages", ""));
    assert!(m.prompt_cache.is_none());
}

/// Upstream declines to annotate OpenAI deliberately (`generate-models.ts:989-991`).
#[test]
fn openai_rows_are_not_annotated() {
    let m = one(&row("openai", "openai-completions", ""));
    assert!(m.prompt_cache.is_none());
}

/// Like its compat sibling, the stamp only fills a key the row left unset.
#[test]
fn a_row_that_declares_its_own_lifetimes_is_not_overwritten() {
    let m = one(&row(
        "anthropic",
        "anthropic-messages",
        r#","promptCache":{"short":60}"#,
    ));
    assert_eq!(
        m.prompt_cache,
        Some(ModelPromptCache {
            short: Some(60),
            long: None
        }),
        "an explicit row value wins and no tier is invented beside it"
    );
}

/// The field round-trips in pi's camelCase and is elided when absent.
#[test]
fn prompt_cache_round_trips_in_camel_case_and_is_elided_when_absent() {
    let m = one(&row("anthropic", "anthropic-messages", ""));
    let json = serde_json::to_string(&m).unwrap();
    assert!(
        json.contains(r#""promptCache":{"short":300,"long":3600}"#),
        "{json}"
    );

    let bare = one(&row("openai", "openai-completions", ""));
    let json = serde_json::to_string(&bare).unwrap();
    assert!(!json.contains("promptCache"), "absent must not serialize");
}

// ------------------------------------------------------------ getPromptCacheTtlMs --------------

/// The ladder defaults to `short`, so a request that names no retention gets the five-minute tier
/// in milliseconds.
#[test]
fn an_unset_retention_resolves_to_the_short_tier_in_milliseconds() {
    let m = one(&row("anthropic", "anthropic-messages", ""));
    assert_eq!(prompt_cache_ttl_ms(&m, None, None), Some(300_000));
}

/// `CYRUP_CACHE_RETENTION=long` promotes to the long tier — pi's `PI_CACHE_RETENTION`, renamed
/// with the rest of the public env surface.
#[test]
fn the_env_opt_in_promotes_to_the_long_tier() {
    let m = one(&row("anthropic", "anthropic-messages", ""));
    let mut env = ProviderEnv::new();
    env.insert("CYRUP_CACHE_RETENTION".to_string(), "long".to_string());
    assert_eq!(prompt_cache_ttl_ms(&m, None, Some(&env)), Some(3_600_000));
}

/// An explicit caller value wins over the env.
#[test]
fn an_explicit_retention_wins_over_the_env() {
    let m = one(&row("anthropic", "anthropic-messages", ""));
    let mut env = ProviderEnv::new();
    env.insert("CYRUP_CACHE_RETENTION".to_string(), "long".to_string());
    assert_eq!(
        prompt_cache_ttl_ms(&m, Some(CacheRetention::Short), Some(&env)),
        Some(300_000)
    );
}

/// A request that turned caching off has no lifetime to keep warm.
#[test]
fn retention_none_has_no_lifetime() {
    let m = one(&row("anthropic", "anthropic-messages", ""));
    assert_eq!(
        prompt_cache_ttl_ms(&m, Some(CacheRetention::None), None),
        None
    );
}

/// A model that publishes no lifetime yields none, which is what makes the warmer decline to
/// schedule for every unannotated model.
#[test]
fn a_model_without_lifetimes_yields_none() {
    let m = one(&row("openai", "openai-completions", ""));
    assert_eq!(prompt_cache_ttl_ms(&m, None, None), None);
    assert_eq!(
        prompt_cache_ttl_ms(&m, Some(CacheRetention::Long), None),
        None
    );
}

/// A tier the model leaves unset yields none even when the other tier is present.
#[test]
fn a_missing_tier_yields_none() {
    let m = one(&row(
        "anthropic",
        "anthropic-messages",
        r#","promptCache":{"short":60}"#,
    ));
    assert_eq!(
        prompt_cache_ttl_ms(&m, Some(CacheRetention::Short), None),
        Some(60_000)
    );
    assert_eq!(
        prompt_cache_ttl_ms(&m, Some(CacheRetention::Long), None),
        None
    );
}

// --------------------------------------- the remote-catalog overlay ----------------------------

/// An overlay row for a direct-Anthropic model that omits `promptCache` still gets the lifetime.
///
/// `merge_models` replaces a baseline entry WHOLESALE by id
/// (`remote_catalog::merge_models`, pi `mergeModels`, `remote-catalog-provider.ts:8-16`), and an
/// overlay row is deserialized straight through serde — it never passes through
/// [`crate::catalog::load_catalog`], which is the only place the embedded catalogs get stamped. So
/// a refreshed `claude-*` row served without `promptCache` would arrive with `prompt_cache: None`,
/// `prompt_cache_ttl_ms` would answer `None`, and prompt-cache warming would be silently OFF for
/// every session on that model, reported only as `Inactive (cache lifetime unavailable)`.
///
/// **Red-proved** by removing the `apply_prompt_cache_metadata` call from `parse_catalog`: the
/// first assertion fails with `None`, and the TTL assertion with it.
#[test]
fn an_overlay_row_for_direct_anthropic_still_carries_the_lifetime() {
    let body = serde_json::json!([{
        "id": "claude-refreshed",
        "name": "Claude Refreshed",
        "api": "anthropic-messages",
        "baseUrl": "https://api.anthropic.com",
        "reasoning": true,
        "input": ["text"],
        "cost": {"input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75},
        "contextWindow": 200_000,
        "maxTokens": 64_000,
    }]);
    let parsed = crate::remote_catalog::parse_catalog("anthropic", &body).expect("parses");
    let model = match parsed.into_iter().next().expect("one row") {
        crate::classifier::AnyModel::Chat(m) => m,
        other => panic!("a chat row must classify as chat: {:?}", other.model_type()),
    };
    assert_eq!(
        model.prompt_cache,
        Some(ModelPromptCache {
            short: Some(300),
            long: Some(3600)
        }),
        "an overlay row must be stamped exactly as an embedded one is"
    );
    assert_eq!(
        prompt_cache_ttl_ms(&model, Some(CacheRetention::Short), None),
        Some(300_000),
        "which is what makes the warmer schedule at all"
    );
}

/// The two halves of the gate the overlay pass must keep: it only FILLS an unset key, and it is
/// direct-Anthropic only — a gateway row must not gain an invented lifetime, because warming it
/// would assume a proxy's cache behaves like Anthropic's.
///
/// **Red-proved** by dropping the provider/api guard from `apply_prompt_cache_metadata`: the
/// openrouter case gained `{short: 300, long: 3600}`.
#[test]
fn the_overlay_pass_keeps_the_rows_own_value_and_skips_gateways() {
    let row = |provider: &str, api: &str, extra: serde_json::Value| {
        let mut obj = serde_json::json!({
            "id": "m",
            "name": "M",
            "api": api,
            "baseUrl": "https://x",
            "reasoning": false,
            "input": ["text"],
            "cost": {"input": 1.0, "output": 1.0, "cacheRead": 0.1, "cacheWrite": 1.25},
            "contextWindow": 200_000,
            "maxTokens": 8192,
        });
        if let (Some(map), Some(extra)) = (obj.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                map.insert(k.clone(), v.clone());
            }
        }
        let parsed = crate::remote_catalog::parse_catalog(provider, &serde_json::json!([obj]))
            .expect("parses");
        match parsed.into_iter().next().expect("one row") {
            crate::classifier::AnyModel::Chat(m) => m.prompt_cache,
            _ => panic!("chat row"),
        }
    };

    // An explicit value WINS — the pass fills, it does not overwrite.
    assert_eq!(
        row(
            "anthropic",
            "anthropic-messages",
            serde_json::json!({"promptCache": {"short": 60}})
        ),
        Some(ModelPromptCache {
            short: Some(60),
            long: None
        })
    );
    // A gateway serving an `anthropic-messages` row is NOT direct Anthropic.
    assert_eq!(
        row("openrouter", "anthropic-messages", serde_json::json!({})),
        None
    );
    // Neither is an Anthropic-provider row on some other api.
    assert_eq!(
        row("anthropic", "openai-completions", serde_json::json!({})),
        None
    );
}
