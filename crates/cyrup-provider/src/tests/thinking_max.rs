//! `max` thinking-level parity (PROV-002 / DRIFT-008).
//!
//! Pi added the `max` rung in fbdd4638 (2026-07-09) — `ThinkingLevel = … | "xhigh" | "max"`
//! (`packages/ai/src/types.ts:79`) plus a 7-entry `EXTENDED_THINKING_LEVELS`
//! (`packages/ai/src/models.ts:661`). cyrup's catalogs were a faithful snapshot of pi @ 5c1a2977,
//! i.e. of the state *before* that commit, so the top rung was unreachable and
//! `claude-opus-4-6` shipped `{"xhigh":"max"}` — the label said `xhigh` while the wire effort was
//! `max`. These tests pin the observable end of the fix: the ladder, the clamp, and the bytes that
//! actually go out on each wire API.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::Model;
use crate::api::compat::thinking_level_key;
use crate::collection::{
    EXTENDED_THINKING_LEVELS, clamp_thinking_level, get_supported_thinking_levels,
};
use cyrup_core::{ModelThinkingLevel, ThinkingLevel};
fn model(provider: &str, id: &str) -> Model {
    crate::all_providers()
        .into_iter()
        .find(|p| p.id().as_str() == provider)
        .unwrap_or_else(|| panic!("no provider {provider}"))
        .models()
        .iter()
        .find(|m| m.id.as_str() == id)
        .unwrap_or_else(|| panic!("no model {provider}/{id}"))
        .clone()
}

// ---------------------------------------------------------------- the ladder + the key space ----

/// Pi `EXTENDED_THINKING_LEVELS` (models.ts:661) is 7 rungs ending in `max`. Order is load-bearing:
/// `clampThinkingLevel` walks this array UPWARD first, so `max` must sit after `xhigh`.
#[test]
fn extended_ladder_is_seven_rungs_ending_in_max() {
    assert_eq!(
        EXTENDED_THINKING_LEVELS,
        [
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Xhigh,
            ModelThinkingLevel::Max,
        ]
    );
}

#[test]
fn max_has_a_thinking_level_map_key() {
    assert_eq!(thinking_level_key(ModelThinkingLevel::Max), "max");
    // The key must round-trip through serde too — settings/session persistence go through it.
    assert_eq!(
        serde_json::to_value(ModelThinkingLevel::Max).unwrap(),
        serde_json::json!("max")
    );
    assert_eq!(
        serde_json::from_value::<ModelThinkingLevel>(serde_json::json!("max")).unwrap(),
        ModelThinkingLevel::Max
    );
    assert_eq!(
        serde_json::from_value::<ThinkingLevel>(serde_json::json!("max")).unwrap(),
        ThinkingLevel::Max
    );
}

#[test]
fn model_thinking_level_max_lowers_to_the_on_level_max() {
    assert_eq!(
        ModelThinkingLevel::Max.level(),
        Some(ThinkingLevel::Max),
        "`max` is an ON level, never silently `off`"
    );
    assert!(ModelThinkingLevel::Max.is_on());
    assert_eq!(
        ModelThinkingLevel::from(ThinkingLevel::Max),
        ModelThinkingLevel::Max
    );
}

// ------------------------------------------------------- supported-levels + clamp, per upstream --

/// Pi `models.ts:669` @v0.83.0 — `if (level === "xhigh" || level === "max") return mapped !== undefined;`.
/// `max` is opt-in per model exactly like `xhigh`, so a model with no map advertises neither.
#[test]
fn max_requires_an_explicit_map_entry_like_xhigh() {
    // A reasoning model with NO thinkingLevelMap: neither top rung is offered.
    let gemini = model("google", "gemini-2.5-pro");
    assert!(gemini.reasoning);
    assert!(gemini.thinking_level_map.is_none());
    let levels = get_supported_thinking_levels(&gemini);
    assert!(!levels.contains(&ModelThinkingLevel::Max), "{levels:?}");
    assert!(!levels.contains(&ModelThinkingLevel::Xhigh), "{levels:?}");
}

/// pi `anthropic.models.ts:136` @91585d9a — `claude-opus-4-6: thinkingLevelMap: {"max":"max"}`.
/// This is the model the bug report named: cyrup carried `{"xhigh":"max"}`, so the selector said
/// `xhigh` while Anthropic received effort `max`.
#[test]
fn opus_4_6_offers_max_and_not_xhigh() {
    let m = model("anthropic", "claude-opus-4-6");
    let map = m.thinking_level_map.as_ref().expect("opus-4-6 map");
    assert_eq!(map.get("max"), Some(&Some("max".to_string())));
    assert_eq!(map.get("xhigh"), None, "no native xhigh rung upstream");

    let levels = get_supported_thinking_levels(&m);
    assert!(levels.contains(&ModelThinkingLevel::Max), "{levels:?}");
    assert!(!levels.contains(&ModelThinkingLevel::Xhigh), "{levels:?}");
}

/// pi `anthropic.models.ts:246` @91585d9a — `claude-sonnet-5: {"xhigh":"xhigh","max":"max"}`.
/// cyrup shipped NO map at all, so `xhigh` was unsupported and silently clamped down to `high`.
#[test]
fn sonnet_5_offers_both_top_rungs() {
    let m = model("anthropic", "claude-sonnet-5");
    let map = m.thinking_level_map.as_ref().expect("sonnet-5 map");
    assert_eq!(map.get("xhigh"), Some(&Some("xhigh".to_string())));
    assert_eq!(map.get("max"), Some(&Some("max".to_string())));

    let levels = get_supported_thinking_levels(&m);
    assert!(levels.contains(&ModelThinkingLevel::Xhigh), "{levels:?}");
    assert!(levels.contains(&ModelThinkingLevel::Max), "{levels:?}");
    assert_eq!(
        clamp_thinking_level(&m, ModelThinkingLevel::Xhigh),
        ModelThinkingLevel::Xhigh,
        "xhigh must no longer be clamped away on sonnet-5"
    );
}

/// Pi `clampThinkingLevel` (models.ts:681-687) walks the ladder UPWARD from the requested rung
/// first. On opus-4-6, whose only top rung is `max`, a request for `xhigh` must promote to `max`
/// rather than fall back down to `high`.
#[test]
fn xhigh_promotes_up_to_max_when_only_max_exists() {
    let m = model("anthropic", "claude-opus-4-6");
    assert_eq!(
        clamp_thinking_level(&m, ModelThinkingLevel::Xhigh),
        ModelThinkingLevel::Max
    );
    assert_eq!(
        clamp_thinking_level(&m, ModelThinkingLevel::Max),
        ModelThinkingLevel::Max
    );
}

/// The downward half: a model that supports neither top rung clamps `max` back to `high`
/// (Pi's second loop, models.ts:688-691).
#[test]
fn max_clamps_down_when_unsupported() {
    let gemini = model("google", "gemini-2.5-pro");
    assert_eq!(
        clamp_thinking_level(&gemini, ModelThinkingLevel::Max),
        ModelThinkingLevel::High
    );
}

// ------------------------------------------------------------------------------ the wire value --
//
// The per-API request-body builders are `pub(crate)`, so the byte-level wire assertions for `max`
// live inline next to them:
//   - `api/anthropic_messages.rs::tests::adaptive_thinking_encodes_max_effort`
//     and `::max_label_matches_the_wire_effort_on_opus_4_6`
//   - `api/openai_completions.rs::tests::reasoning_effort_encodes_max`
// What this file can pin publicly is the level→effort KEY those builders look up, plus the
// budget-provider clamp.

/// The `thinkingLevelMap` lookup key must equal the label the UI shows for the same rung — the
/// display/wire agreement PROV-002 reported broken (`claude-opus-4-6` displayed `xhigh` while
/// sending effort `max`). With the catalog corrected, the rung a user can select on opus-4-6 is
/// `max`, its key is `"max"`, and the map sends `"max"`.
#[test]
fn opus_4_6_label_and_mapped_effort_agree() {
    let m = model("anthropic", "claude-opus-4-6");
    let selected = clamp_thinking_level(&m, ModelThinkingLevel::Xhigh);
    let key = thinking_level_key(selected);
    let mapped = m
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(key))
        .cloned()
        .flatten()
        .expect("opus-4-6 maps its selectable top rung");
    assert_eq!(key, "max");
    assert_eq!(
        key, mapped,
        "the displayed level must equal the wire effort (was: label `xhigh`, wire `max`)"
    );
}

/// Sonnet-5 keeps `xhigh` and `max` as DISTINCT efforts — proof the rungs are not aliases.
#[test]
fn sonnet_5_maps_xhigh_and_max_distinctly() {
    let m = model("anthropic", "claude-sonnet-5");
    let map = m.thinking_level_map.as_ref().expect("map");
    assert_eq!(
        map.get(thinking_level_key(ModelThinkingLevel::Xhigh)),
        Some(&Some("xhigh".to_string()))
    );
    assert_eq!(
        map.get(thinking_level_key(ModelThinkingLevel::Max)),
        Some(&Some("max".to_string()))
    );
}

/// Token-budget providers collapse BOTH top rungs to `high` (Pi `clampReasoning`,
/// simple-options.ts:48-49) — `max` must not fall through unclamped and blow the budget table.
#[test]
fn token_budget_providers_clamp_max_to_high() {
    use crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_reasoning};

    assert_eq!(clamp_reasoning(ThinkingLevel::Max), ThinkingLevel::High);
    assert_eq!(clamp_reasoning(ThinkingLevel::Xhigh), ThinkingLevel::High);
    assert_eq!(
        adjust_max_tokens_for_thinking(Some(1000), 200_000, ThinkingLevel::Max, None),
        adjust_max_tokens_for_thinking(Some(1000), 200_000, ThinkingLevel::High, None),
        "max borrows high's 16384-token budget"
    );
}

// -------------------------------------------------------------- catalog silent-emptiness guard --

/// Every embedded catalog is `include_str!` + `serde_json::from_str(...).unwrap_or_default()`, so a
/// malformed edit yields an EMPTY provider instead of an error. Nothing asserted that before; this
/// makes a bad catalog edit fail loudly. The providers that ship no embedded catalog BY DESIGN
/// (`catalog_data::DYNAMIC_ONLY_PROVIDERS`, PROV-014) are skipped here and pinned in both
/// directions by `catalog_data::every_registered_provider_has_a_non_empty_catalog`.
#[test]
fn every_embedded_catalog_parses_non_empty() {
    let providers = crate::all_providers();
    assert!(providers.len() >= 31, "got {} providers", providers.len());
    for p in providers {
        if super::catalog_data::DYNAMIC_ONLY_PROVIDERS.contains(&p.id().as_str()) {
            continue;
        }
        assert!(
            !p.models().is_empty(),
            "provider `{}` has an EMPTY catalog — its embedded JSON almost certainly failed to \
             parse (the loaders swallow errors with unwrap_or_default)",
            p.id()
        );
    }
}

/// Corrected catalog VALUES have to reach model selection, not merely parse. Pins one representative
/// per shape of the DRIFT-008 correction.
#[test]
fn corrected_maps_reach_model_selection() {
    // shape 1: `{"xhigh":"max"}` -> `{"max":"max"}` (the mislabelled rung)
    for (prov, id) in [
        ("anthropic", "claude-opus-4-6"),
        ("openrouter", "anthropic/claude-opus-4.6"),
        ("vercel-ai-gateway", "anthropic/claude-opus-4.6"),
    ] {
        let m = model(prov, id);
        let levels = get_supported_thinking_levels(&m);
        assert!(
            levels.contains(&ModelThinkingLevel::Max)
                && !levels.contains(&ModelThinkingLevel::Xhigh),
            "{prov}/{id}: {levels:?}"
        );
    }

    // shape 2: `max` appended alongside an existing `xhigh`
    for (prov, id) in [
        ("anthropic", "claude-opus-4-7"),
        ("anthropic", "claude-opus-4-8"),
        ("anthropic", "claude-fable-5"),
    ] {
        let m = model(prov, id);
        let levels = get_supported_thinking_levels(&m);
        assert!(
            levels.contains(&ModelThinkingLevel::Max)
                && levels.contains(&ModelThinkingLevel::Xhigh),
            "{prov}/{id}: {levels:?}"
        );
    }

    // shape 3: a map added where cyrup had none at all
    for (prov, id) in [
        ("anthropic", "claude-sonnet-4-6"),
        ("anthropic", "claude-sonnet-5"),
    ] {
        let m = model(prov, id);
        assert!(
            m.thinking_level_map.is_some(),
            "{prov}/{id} must now carry a map"
        );
        assert!(get_supported_thinking_levels(&m).contains(&ModelThinkingLevel::Max));
    }

    // shape 4: an explicit `"max": null` — present but UNSUPPORTED (openrouter's deepseek entries,
    // pi openrouter.models.ts @91585d9a). Proves `max` is opt-in per model, not blanket-enabled.
    let ds = model("openrouter", "deepseek/deepseek-v4-pro");
    let map = ds.thinking_level_map.as_ref().expect("map");
    assert_eq!(map.get("max"), Some(&None));
    let levels = get_supported_thinking_levels(&ds);
    assert!(!levels.contains(&ModelThinkingLevel::Max), "{levels:?}");
    assert!(levels.contains(&ModelThinkingLevel::Xhigh), "{levels:?}");
}

/// No catalog may still carry the pre-fbdd4638 `"xhigh": "max"` remap — that exact shape IS the
/// defect (a rung labelled `xhigh` that sends effort `max`).
#[test]
fn no_catalog_still_remaps_xhigh_onto_max() {
    let mut offenders = Vec::new();
    for p in crate::all_providers() {
        for m in p.models().iter() {
            if let Some(map) = &m.thinking_level_map
                && map.get("xhigh") == Some(&Some("max".to_string()))
            {
                offenders.push(format!("{}/{}", p.id(), m.id));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these still label the `max` effort as `xhigh`: {offenders:?}"
    );
}
