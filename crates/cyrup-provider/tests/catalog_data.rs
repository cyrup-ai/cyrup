//! Embedded model-catalog data correctness (PROV-004).
//!
//! # Provenance
//!
//! cyrup's 31 embedded catalogs under `src/providers/catalog/*.json` are a **byte-faithful
//! snapshot of pi @ `5c1a2977`** ("fix(ai): update generated model catalogue", 2026-06-30):
//! diffing every catalog field-by-field against that revision comes out identical for 30 of the 30
//! files that have a `packages/ai/src/providers/*.models.ts` counterpart, modulo three deltas that
//! upstream itself later adopted.
//!
//! pi then refreshed the catalogs seventeen times between `5c1a2977` and `91585d9a`
//! (2026-07-10 16:34) — the revision matching cyrup's own HEAD baseline date. cyrup never picked
//! that window up, so the embedded data was stale against cyrup's *own* declared baseline. This is
//! **owed debt, not post-baseline drift**: every commit cited below landed before 2026-07-10.
//!
//! The commits in that window that moved data these tests pin:
//!
//! | pi commit | date | effect |
//! |---|---|---|
//! | `cc2db980` | 2026-07-08 | switched generation to models.dev per-provider catalogs — retired 10 EOL Claude models, raised Sonnet 4.5 to 1M context, fixed `mistral-medium-latest`, zeroed the Xiaomi token-plan (prepaid) rates |
//! | `46145bef` | 2026-07-09 | OpenRouter context windows now come from the *top serving provider* rather than the theoretical model max |
//! | `fbdd4638` | 2026-07-09 | added the `max` thinking rung (covered by `thinking_max.rs`, PROV-002/DRIFT-008) |
//! | `7df2a94e` | 2026-07-09 | GPT-5.6 (`luna`/`sol`/`terra`) metadata |
//! | `5b4bda30`, `ee24a9ec`, `72d77b53`, `1da1cdb2`, `844d175e` | 2026-07-08..09 | generated-catalog refreshes |
//!
//! # Why these tests exist
//!
//! Every catalog is `include_str!`-ed and parsed with `serde_json::from_str(..).unwrap_or_default()`
//! (e.g. `providers/anthropic.rs:30`). A malformed edit therefore yields an **empty** provider
//! catalog with no diagnostic whatsoever — the failure is completely silent, and the only symptom
//! is a provider that mysteriously offers no models. `every_embedded_catalog_parses_non_empty`
//! closes that hole for the whole set, and the value assertions below stop a *well-formed but
//! wrong* number from going unnoticed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_provider::collection::{CreateModelsOptions, Models, create_models};
use cyrup_provider::{Model, all_providers};

// ------------------------------------------------------------------- the silent-emptiness guard --

/// Every embedded catalog blob, by file name. `include_str!` here mirrors the production loaders,
/// so a file that stops parsing fails *this* test loudly instead of silently degrading a provider
/// to zero models via `unwrap_or_default()`.
const CATALOGS: &[(&str, &str)] = &[
    ("ant-ling", include_str!("../src/providers/catalog/ant-ling.json")),
    ("anthropic", include_str!("../src/providers/catalog/anthropic.json")),
    ("azure-openai-responses", include_str!("../src/providers/catalog/azure-openai-responses.json")),
    ("cerebras", include_str!("../src/providers/catalog/cerebras.json")),
    ("cloudflare-ai-gateway", include_str!("../src/providers/catalog/cloudflare-ai-gateway.json")),
    ("cloudflare-workers-ai", include_str!("../src/providers/catalog/cloudflare-workers-ai.json")),
    ("deepseek", include_str!("../src/providers/catalog/deepseek.json")),
    ("fireworks", include_str!("../src/providers/catalog/fireworks.json")),
    ("google", include_str!("../src/providers/catalog/google.json")),
    ("groq", include_str!("../src/providers/catalog/groq.json")),
    ("huggingface", include_str!("../src/providers/catalog/huggingface.json")),
    ("kimi-coding", include_str!("../src/providers/catalog/kimi-coding.json")),
    ("minimax", include_str!("../src/providers/catalog/minimax.json")),
    ("minimax-cn", include_str!("../src/providers/catalog/minimax-cn.json")),
    ("mistral", include_str!("../src/providers/catalog/mistral.json")),
    ("moonshotai", include_str!("../src/providers/catalog/moonshotai.json")),
    ("moonshotai-cn", include_str!("../src/providers/catalog/moonshotai-cn.json")),
    ("nvidia", include_str!("../src/providers/catalog/nvidia.json")),
    ("openai", include_str!("../src/providers/catalog/openai.json")),
    ("opencode", include_str!("../src/providers/catalog/opencode.json")),
    ("opencode-go", include_str!("../src/providers/catalog/opencode-go.json")),
    ("openrouter", include_str!("../src/providers/catalog/openrouter.json")),
    ("vercel-ai-gateway", include_str!("../src/providers/catalog/vercel-ai-gateway.json")),
    ("xai", include_str!("../src/providers/catalog/xai.json")),
    ("xiaomi", include_str!("../src/providers/catalog/xiaomi.json")),
    ("xiaomi-token-plan-ams", include_str!("../src/providers/catalog/xiaomi-token-plan-ams.json")),
    ("xiaomi-token-plan-cn", include_str!("../src/providers/catalog/xiaomi-token-plan-cn.json")),
    ("xiaomi-token-plan-sgp", include_str!("../src/providers/catalog/xiaomi-token-plan-sgp.json")),
    ("zai", include_str!("../src/providers/catalog/zai.json")),
    ("zai-coding-cn", include_str!("../src/providers/catalog/zai-coding-cn.json")),
];

/// The guard. Production loaders swallow a parse error into `Vec::default()`, so without this a
/// typo'd catalog ships as an empty provider. Here the error is surfaced verbatim.
#[test]
fn every_embedded_catalog_parses_non_empty() {
    assert_eq!(CATALOGS.len(), 30, "catalog roster drifted from the file set");
    for (name, blob) in CATALOGS {
        let models: Vec<Model> = serde_json::from_str(blob)
            .unwrap_or_else(|e| panic!("catalog {name}.json failed to parse: {e}"));
        assert!(!models.is_empty(), "catalog {name}.json parsed to ZERO models");
        for m in &models {
            assert!(!m.id.as_str().is_empty(), "{name}: empty model id");
            assert!(m.context_window > 0, "{name}: {} has zero contextWindow", m.id);
            // Azure is the one provider whose endpoint is per-deployment and therefore supplied by
            // the user at runtime — pi's generated catalog ships `baseUrl: ""` for all 42 entries
            // (`azure-openai-responses.models.ts` @ `91585d9a`). Everywhere else an empty baseUrl
            // means the request would go nowhere.
            if *name != "azure-openai-responses" {
                assert!(!m.base_url.is_empty(), "{name}: {} has empty baseUrl", m.id);
            }
        }
    }
}

/// The same guard one level up: every *registered* provider must expose a non-empty catalog. Catches
/// a loader wired to the wrong file as well as a bad blob.
#[test]
fn every_registered_provider_has_a_non_empty_catalog() {
    for p in all_providers() {
        assert!(
            !p.models().is_empty(),
            "provider {} exposes zero models — catalog parse likely failed silently",
            p.id()
        );
    }
}

// -------------------------------------------------------------------------- the selection seam --

/// `Models::get_model` (collection.rs:108) is cyrup's port of pi `models.getModel` — the lookup
/// every consumer performs to turn a `provider/id` pair into the `Model` that shapes the request.
/// Asserting through it (rather than against the raw JSON) proves the corrected value actually
/// *reaches model selection*.
fn selection() -> Models {
    let mut models = create_models(CreateModelsOptions::default());
    for p in all_providers() {
        models.set_provider(p);
    }
    models
}

fn pick(models: &Models, provider: &str, id: &str) -> Model {
    models
        .get_model(provider, id)
        .unwrap_or_else(|| panic!("model selection could not find {provider}/{id}"))
}

// ------------------------------------------------------------------------------ the headline case --

/// **The headline PROV-004 case.** Anthropic shipped a 1M-token context for Sonnet 4.5; pi picked it
/// up in `cc2db980` (`anthropic.models.ts:185-201` and `:202-218` @ `91585d9a` both read
/// `contextWindow: 1000000`). cyrup's snapshot still capped it at 200k, so compaction triggered ~5x
/// too early and the user could not use four fifths of the window they were paying for.
/// `maxTokens` was already correct at 64000 and must not move.
#[test]
fn sonnet_4_5_offers_the_full_1m_context_window() {
    let models = selection();
    for id in ["claude-sonnet-4-5", "claude-sonnet-4-5-20250929"] {
        let m = pick(&models, "anthropic", id);
        assert_eq!(
            m.context_window, 1_000_000,
            "{id}: pi anthropic.models.ts @91585d9a says contextWindow 1000000 (cc2db980)"
        );
        assert_eq!(m.max_tokens, 64_000, "{id}: maxTokens is unchanged at 64000");
    }
}

/// `claude-sonnet-4-6` doubled its output cap. pi `anthropic.models.ts:235` @ `91585d9a`:
/// `maxTokens: 128000`. cyrup capped generation at 64000, silently truncating long outputs.
#[test]
fn sonnet_4_6_max_tokens_is_128k() {
    let m = pick(&selection(), "anthropic", "claude-sonnet-4-6");
    assert_eq!(m.max_tokens, 128_000);
    assert_eq!(m.context_window, 1_000_000);
}

// ------------------------------------------------------------------- retired / added model sets --

/// `cc2db980` moved generation to models.dev's per-provider catalogs, which do not list EOL models;
/// ten Claude 3.x/4.0 entries were dropped from `anthropic.models.ts` in that commit. Offering a
/// retired model is worse than not offering it: selection succeeds and the API call then fails.
#[test]
fn retired_claude_models_are_gone() {
    let models = selection();
    for id in [
        "claude-3-5-sonnet-20240620",
        "claude-3-5-sonnet-20241022",
        "claude-3-7-sonnet-20250219",
        "claude-3-haiku-20240307",
        "claude-3-opus-20240229",
        "claude-3-sonnet-20240229",
        "claude-opus-4-0",
        "claude-opus-4-20250514",
        "claude-sonnet-4-0",
        "claude-sonnet-4-20250514",
    ] {
        assert!(
            models.get_model("anthropic", id).is_none(),
            "{id} was retired upstream in cc2db980 but is still selectable"
        );
    }
    // …and the surviving set is exactly pi's 14.
    assert_eq!(models.get_models(Some("anthropic")).len(), 14);
    // Every remaining Claude model is a reasoning model (pi @91585d9a: all 14 `reasoning: true`).
    assert!(models.get_models(Some("anthropic")).iter().all(|m| m.reasoning));
}

/// Models pi added in the missed window that cyrup users simply could not select. Sources:
/// `xai.models.ts` (grok-4.5), `openai.models.ts` + `azure-openai-responses.models.ts` (`7df2a94e`,
/// GPT-5.6 luna/sol/terra), `cerebras.models.ts`, `huggingface.models.ts` (`cc2db980`),
/// `nvidia.models.ts`, `opencode.models.ts`, `vercel-ai-gateway.models.ts` — all @ `91585d9a`.
#[test]
fn models_added_upstream_are_now_selectable() {
    let models = selection();
    let expect = [
        ("xai", "grok-4.5"),
        ("openai", "gpt-5.6-luna"),
        ("openai", "gpt-5.6-sol"),
        ("openai", "gpt-5.6-terra"),
        ("azure-openai-responses", "gpt-5.6-luna"),
        ("cerebras", "gemma-4-31b"),
        ("huggingface", "openai/gpt-oss-20b"),
        ("nvidia", "z-ai/glm-5.2"),
        ("opencode", "claude-fable-5"),
        ("opencode", "grok-4.5"),
        ("opencode", "hy3-free"),
        ("vercel-ai-gateway", "anthropic/claude-fable-5"),
        ("vercel-ai-gateway", "xai/grok-4.5"),
        ("openrouter", "x-ai/grok-4.5"),
        ("openrouter", "openai/gpt-5.6-luna"),
        ("openrouter", "tencent/hy3"),
    ];
    for (provider, id) in expect {
        assert!(
            models.get_model(provider, id).is_some(),
            "{provider}/{id} exists upstream @91585d9a but is not selectable"
        );
    }
    // nvidia replaced GLM 5.1 with 5.2; openrouter renamed the poolside ids.
    assert!(models.get_model("nvidia", "z-ai/glm-5.1").is_none());
    assert!(models.get_model("openrouter", "poolside/laguna-xs.2").is_none());
    assert!(models.get_model("openrouter", "poolside/laguna-xs-2.1").is_some());
}

// ------------------------------------------------------------------------------- cost / capability --

/// `mistral-medium-latest` was mis-described on **two** axes at once: cyrup said it cannot reason
/// and billed it at roughly a quarter of its real rate. pi `mistral.models.ts` @ `91585d9a`
/// (changed by `cc2db980`): `reasoning: true`, `cost {input 1.5, output 7.5, cacheRead 0.15}`.
/// `reasoning: false` also suppresses the thinking UI for a model that supports it.
#[test]
fn mistral_medium_reasons_and_is_priced_correctly() {
    let m = pick(&selection(), "mistral", "mistral-medium-latest");
    assert!(m.reasoning, "pi mistral.models.ts @91585d9a: reasoning true");
    assert_eq!(m.cost.input, 1.5);
    assert_eq!(m.cost.output, 7.5);
    assert_eq!(m.cost.cache_read, 0.15);
}

/// Two cache-read rates were wrong by ~2x and 10x respectively. A wrong `cacheRead` does not break a
/// request — it silently misreports spend in the footer and in session cost accounting.
/// pi @ `91585d9a`: `fireworks.models.ts` glm-5p2 `cacheRead: 0.14` (`844d175e` aligned the pair);
/// `vercel-ai-gateway.models.ts` deepseek-v4-flash `cacheRead: 0.028`.
#[test]
fn cache_read_rates_match_upstream() {
    let models = selection();
    let fw = pick(&models, "fireworks", "accounts/fireworks/models/glm-5p2");
    assert_eq!(fw.cost.cache_read, 0.14, "cyrup over-reported this by 1.86x");

    let vercel = pick(&models, "vercel-ai-gateway", "deepseek/deepseek-v4-flash");
    assert_eq!(
        vercel.cost.cache_read, 0.028,
        "cyrup under-reported this by 10x (0.0028)"
    );
}

/// The Xiaomi *token-plan* providers are prepaid: usage is drawn from a plan, not billed per token,
/// so pi zeroes every rate. `cc2db980` also removed the API-billing-only models that had been cloned
/// into the token-plan catalogs (see that commit's own message naming `mimo-v2-omni`). cyrup billed
/// phantom dollars against a prepaid plan and offered two models the endpoint does not serve.
#[test]
fn xiaomi_token_plans_are_prepaid_and_carry_no_api_billing_models() {
    let models = selection();
    for provider in [
        "xiaomi-token-plan-ams",
        "xiaomi-token-plan-cn",
        "xiaomi-token-plan-sgp",
    ] {
        let list = models.get_models(Some(provider));
        assert!(!list.is_empty(), "{provider} has no models");
        for m in &list {
            assert_eq!(
                (m.cost.input, m.cost.output, m.cost.cache_read, m.cost.cache_write),
                (0.0, 0.0, 0.0, 0.0),
                "{provider}/{}: token-plan usage is prepaid, pi zeroes all rates",
                m.id
            );
        }
        for id in ["mimo-v2-omni", "mimo-v2.5-pro-ultraspeed"] {
            assert!(
                models.get_model(provider, id).is_none(),
                "{provider}/{id} is API-billing-only; cc2db980 removed it from the token plans"
            );
        }
    }
}

/// pi `46145bef` ("use context length from top provider") replaced OpenRouter's theoretical model
/// maxima with what the top serving provider actually accepts. cyrup's 10M for Llama 4 Scout was the
/// paper number; a request sized to it is rejected by the router. Same class of fix for the
/// `maxTokens` entries, which were the router's generic 4096/16384 defaults.
#[test]
fn openrouter_context_windows_come_from_the_serving_provider() {
    let models = selection();

    let scout = pick(&models, "openrouter", "meta-llama/llama-4-scout");
    assert_eq!(scout.context_window, 327_680, "not the 10M paper maximum");

    let flash = pick(&models, "openrouter", "deepseek/deepseek-v4-flash");
    assert_eq!(flash.max_tokens, 65_536, "cyrup had the router default 4096");

    let r1 = pick(&models, "openrouter", "deepseek/deepseek-r1");
    assert_eq!(r1.context_window, 64_000);

    // A transposed digit in the snapshot: 1048756 is not a power-of-two-ish window at all.
    let gem = pick(&models, "openrouter", "google/gemini-3.1-pro-preview-customtools");
    assert_eq!(gem.context_window, 1_048_576);
}

/// opencode-go's MiniMax M3 carried a marketing suffix cyrup's snapshot picked up and upstream
/// dropped (`opencode-go.models.ts` @ `91585d9a`: `name: "MiniMax-M3"`). Display-only, but the model
/// picker shows it.
#[test]
fn opencode_go_minimax_m3_display_name_matches_upstream() {
    let m = pick(&selection(), "opencode-go", "minimax-m3");
    assert_eq!(m.name, "MiniMax-M3");
}
