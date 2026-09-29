//! The curated per-provider default model table, the scan order it is read in, and the
//! fallback-model synthesis built on top of it (R-07-021).

use cyrup_provider::Model;

/// Curated default model id per known provider (Pi `defaultModelPerProvider`,
/// `model-resolver.ts:20-59` @v0.87.1). Returns `None` for an unknown provider.
///
/// CFG-084 — every row is pi's v0.87.1 value except `meta` (`muse-spark-1.3`), a provider cyrup
/// does not ship, pinned by name in the test below.
pub fn default_model_per_provider(provider: &str) -> Option<&'static str> {
    let id = match provider {
        "amazon-bedrock" => "us.anthropic.claude-opus-4-6-v1",
        "ant-ling" => "Ring-2.6-1T",
        "anthropic" => "claude-opus-4-8",
        "openai" => "gpt-5.5",
        "azure-openai-responses" => "gpt-5.4",
        "openai-codex" => "gpt-5.5",
        // Resolved against the gateway's runtime catalog (`radius.rs`), so no embedded row gates it.
        "radius" => "balanced",
        "nvidia" => "nvidia/nemotron-3-super-120b-a12b",
        "deepseek" => "deepseek-v4-pro",
        "google" => "gemini-3.1-pro-preview",
        "google-vertex" => "gemini-3.1-pro-preview",
        "github-copilot" => "gpt-5.4",
        "openrouter" => "moonshotai/kimi-k2.6",
        "vercel-ai-gateway" => "zai/glm-5.1",
        "xai" => "grok-4.7",
        "groq" => "openai/gpt-oss-120b",
        // CHASED to v0.87.1 (`model-resolver.ts:37`). These three sat in the test's `DEFERRED`
        // array because the id upstream had moved to was not in the embedded catalog — the
        // catalog was frozen at `b0c2a90e` and `glm-5.3`/`gpt-oss-120b` post-date it. PROV-071's
        // live refresh carries all three, so the block that deferred them is gone and NOT chasing
        // them is now the defect: `zai-glm-4.7` and `glm-5.1` are retired upstream, so leaving
        // them here drops three providers out of `first_default_or_first`'s scan entirely and
        // launches the user on whatever sorts first.
        "cerebras" => "gpt-oss-120b",
        "zai" => "glm-5.3",
        "zai-coding-cn" => "glm-5.3",
        "mistral" => "devstral-medium-latest",
        "minimax" => "MiniMax-M2.7",
        "minimax-cn" => "MiniMax-M2.7",
        "moonshotai" => "kimi-k2.6",
        "moonshotai-cn" => "kimi-k2.6",
        "huggingface" => "moonshotai/Kimi-K2.6",
        "fireworks" => "accounts/fireworks/models/kimi-k2p6",
        "together" => "moonshotai/Kimi-K2.6",
        "baseten" => "zai-org/GLM-5.2",
        "opencode" => "kimi-k2.6",
        "opencode-go" => "kimi-k2.6",
        "kimi-coding" => "kimi-for-coding",
        "cloudflare-workers-ai" => "@cf/moonshotai/kimi-k2.6",
        "cloudflare-ai-gateway" => "workers-ai/@cf/moonshotai/kimi-k2.6",
        // Alibaba Cloud Model Studio "Token Plan" — two regions, identical catalogs, separate
        // endpoints and API keys (`ai/scripts/generate-models.ts:1993-2012`). Both name the same
        // curated default, which is pi's own value at `model-resolver.ts:47-48` and NOT an
        // extrapolation from the `-cn` sibling: upstream writes `qwen3.7-max` on both keys.
        "qwen-token-plan" => "qwen3.7-max",
        "qwen-token-plan-cn" => "qwen3.7-max",
        "qwen-token-plan-individual" => "qwen3.8-max",
        "xiaomi" => "mimo-v2.5-pro",
        "xiaomi-token-plan-cn" => "mimo-v2.5-pro",
        "xiaomi-token-plan-ams" => "mimo-v2.5-pro",
        "xiaomi-token-plan-sgp" => "mimo-v2.5-pro",
        _ => return None,
    };
    Some(id)
}

/// The ordered list of known providers, used to scan for a curated default (Pi iterates
/// `Object.keys(defaultModelPerProvider)`, model-resolver.ts:593/655).
const KNOWN_PROVIDERS: &[&str] = &[
    "amazon-bedrock",
    "ant-ling",
    "anthropic",
    "openai",
    "azure-openai-responses",
    "openai-codex",
    "radius",
    "nvidia",
    "deepseek",
    "google",
    "google-vertex",
    "github-copilot",
    "openrouter",
    "vercel-ai-gateway",
    "xai",
    "groq",
    "cerebras",
    "zai",
    "zai-coding-cn",
    "mistral",
    "minimax",
    "minimax-cn",
    "moonshotai",
    "moonshotai-cn",
    "huggingface",
    "fireworks",
    "together",
    "baseten",
    "opencode",
    "opencode-go",
    "kimi-coding",
    "cloudflare-workers-ai",
    "cloudflare-ai-gateway",
    // Position is load-bearing: [`first_default_or_first`] returns the FIRST provider in this list
    // with an available curated-default match, so the order must be pi's `Object.keys` order —
    // insertion order of `defaultModelPerProvider` (`model-resolver.ts:14-53`), where the two
    // qwen keys sit between `cloudflare-ai-gateway` and `xiaomi`.
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "qwen-token-plan-individual",
    "xiaomi",
    "xiaomi-token-plan-cn",
    "xiaomi-token-plan-ams",
    "xiaomi-token-plan-sgp",
];

/// Find the first available model whose (provider, id) matches a curated default, else the first
/// available model (Pi's loop at model-resolver.ts:593-602 / 655-667).
pub(super) fn first_default_or_first(available: &[Model]) -> Option<Model> {
    for provider in KNOWN_PROVIDERS {
        if let Some(default_id) = default_model_per_provider(provider)
            && let Some(m) = available
                .iter()
                .find(|m| m.provider.as_str() == *provider && m.id.as_str() == default_id)
        {
            return Some(m.clone());
        }
    }
    available.first().cloned()
}

/// Synthesize a custom model for `(provider, model_id)` by cloning the provider's curated-default
/// (or first) model and overriding id/name (Pi `buildFallbackModel`, model-resolver.ts:163-177).
pub fn build_fallback_model(provider: &str, model_id: &str, available: &[Model]) -> Option<Model> {
    let provider_models: Vec<&Model> = available
        .iter()
        .filter(|m| m.provider.as_str() == provider)
        .collect();
    let base = provider_models.first().copied()?;
    let default_id = default_model_per_provider(provider);
    let base = match default_id {
        Some(did) => provider_models
            .iter()
            .find(|m| m.id.as_str() == did)
            .copied()
            .unwrap_or(base),
        None => base,
    };
    let mut model = base.clone();
    model.id = model_id.into();
    model.name = model_id.to_string();
    Some(model)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::fixtures::model;

    #[test]
    fn default_model_table_matches_pi() {
        // model-resolver.ts:14-50
        assert_eq!(
            default_model_per_provider("anthropic"),
            Some("claude-opus-4-8")
        );
        assert_eq!(default_model_per_provider("openai"), Some("gpt-5.5"));
        assert_eq!(
            default_model_per_provider("amazon-bedrock"),
            Some("us.anthropic.claude-opus-4-6-v1")
        );
        assert_eq!(default_model_per_provider("totally-unknown"), None);
    }

    /// **G16/G42.** The two Qwen Token Plan parents are `KnownProvider`s at v0.83.0
    /// (`ai/src/types.ts:67-68`) and carry a curated default (`model-resolver.ts:47-48`); cyrup's
    /// table had neither, so `--provider qwen-token-plan` fell through `default_model_per_provider`
    /// to `None`.
    ///
    /// The user action: a `models.json` that declares a `qwen-token-plan` provider block (R-07-023 —
    /// the only way to reach these two today, since the built-in registration is still blocked on
    /// catalog data that has never existed in pi's git history), then
    /// `cyrup --provider qwen-token-plan --model <an id the block does not list>`. Pi's
    /// `buildFallbackModel` clones the provider's CURATED default to carry its api/compat/window
    /// onto the custom id; with no table entry it cloned whichever model happened to be first.
    #[test]
    fn qwen_token_plan_custom_id_clones_the_curated_default_not_the_first_model() {
        // A models.json block listing the plan's models in catalog order — `MiniMax-M2.5` sorts
        // first and is exactly the wrong base: it is the ONE model of the fifteen that pi's own
        // `qwen-token-plan-models.test.ts` excludes from the Qwen thinking set.
        let mut minimax = model("qwen-token-plan", "MiniMax-M2.5", "MiniMax M2.5");
        minimax.reasoning = false;
        minimax.context_window = 200_000;
        let mut curated = model("qwen-token-plan", "qwen3.7-max", "Qwen3.7 Max");
        curated.context_window = 1_000_000;
        let available = vec![minimax, curated];

        let built = build_fallback_model("qwen-token-plan", "qwen3.9-max", &available)
            .expect("a provider with models must yield a fallback");
        assert_eq!(built.id.as_str(), "qwen3.9-max");
        assert_eq!(
            built.context_window, 1_000_000,
            "the clone base must be the curated qwen3.7-max, not the first-listed MiniMax-M2.5"
        );
        assert!(
            built.reasoning,
            "…and must therefore inherit the curated model's reasoning flag"
        );

        // Both regions name the SAME default (`model-resolver.ts:47-48`), and both must be known.
        assert_eq!(
            default_model_per_provider("qwen-token-plan"),
            Some("qwen3.7-max")
        );
        assert_eq!(
            default_model_per_provider("qwen-token-plan-cn"),
            Some("qwen3.7-max")
        );
        assert!(
            KNOWN_PROVIDERS.contains(&"qwen-token-plan")
                && KNOWN_PROVIDERS.contains(&"qwen-token-plan-cn"),
            "an entry absent from KNOWN_PROVIDERS is never scanned by first_default_or_first"
        );
    }

    /// MIRROR — the scan ORDER. `first_default_or_first` returns the first KNOWN_PROVIDERS entry
    /// with an available match, so inserting the qwen keys anywhere but pi's `Object.keys` position
    /// would silently re-rank every other provider's claim on the initial model. Pi's
    /// `defaultModelPerProvider` puts all THREE qwen keys — `qwen-token-plan`,
    /// `qwen-token-plan-cn`, then `qwen-token-plan-individual` — between `cloudflare-ai-gateway`
    /// and `xiaomi` (`model-resolver.ts:53-57`).
    #[test]
    fn mirror_qwen_keys_sit_where_pi_puts_them_in_the_scan_order() {
        let pos = |id: &str| KNOWN_PROVIDERS.iter().position(|p| *p == id);
        let (gateway, qwen, qwen_cn, qwen_individual, xiaomi) = (
            pos("cloudflare-ai-gateway").unwrap(),
            pos("qwen-token-plan").unwrap(),
            pos("qwen-token-plan-cn").unwrap(),
            pos("qwen-token-plan-individual").unwrap(),
            pos("xiaomi").unwrap(),
        );
        assert_eq!(qwen, gateway + 1);
        assert_eq!(qwen_cn, qwen + 1);
        assert_eq!(qwen_individual, qwen_cn + 1);
        assert_eq!(xiaomi, qwen_individual + 1);

        // And the consequence: with BOTH an xiaomi and a qwen default available, qwen wins.
        let available = vec![
            model("xiaomi", "mimo-v2.5-pro", "MiMo"),
            model("qwen-token-plan", "qwen3.7-max", "Qwen3.7 Max"),
        ];
        let chosen = first_default_or_first(&available).unwrap();
        assert_eq!(chosen.provider.as_str(), "qwen-token-plan");
    }

    /// CFG-019 + CFG-041: `defaultModelPerProvider` must equal pi's entries key for key AND in
    /// order — `Object.keys(defaultModelPerProvider)` IS the launch scan order at step 4
    /// (`model-resolver.ts:683-692` @v0.84.1), so a missing or misplaced key changes which model a
    /// user launches on.
    ///
    /// **Pinned at v0.87.1 (CFG-084) with NAMED exceptions, not silently mixed.** `DEFERRED` lists
    /// every row cyrup does not carry at pi's value, what it carries instead, and why. The last loop
    /// is the one that matters now that catalogs are live: a curated default naming an id the
    /// catalog no longer carries is NOT inert — `first_default_or_first` finds no match, skips the
    /// provider entirely, and the user silently lands on `available.first()` instead. That must be
    /// loud.
    #[test]
    fn default_model_per_provider_matches_pi_and_every_default_resolves() {
        // `git show v0.87.1:packages/coding-agent/src/core/model-resolver.ts`, `:20-59`.
        const PI: &[(&str, &str)] = &[
            ("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1"),
            ("ant-ling", "Ring-2.6-1T"),
            ("anthropic", "claude-opus-4-8"),
            ("openai", "gpt-5.5"),
            ("azure-openai-responses", "gpt-5.4"),
            ("openai-codex", "gpt-5.5"),
            ("radius", "balanced"),
            ("nvidia", "nvidia/nemotron-3-super-120b-a12b"),
            ("deepseek", "deepseek-v4-pro"),
            ("google", "gemini-3.1-pro-preview"),
            ("google-vertex", "gemini-3.1-pro-preview"),
            ("github-copilot", "gpt-5.4"),
            ("openrouter", "moonshotai/kimi-k2.6"),
            ("vercel-ai-gateway", "zai/glm-5.1"),
            ("xai", "grok-4.7"),
            ("groq", "openai/gpt-oss-120b"),
            ("cerebras", "gpt-oss-120b"),
            ("zai", "glm-5.3"),
            ("zai-coding-cn", "glm-5.3"),
            ("mistral", "devstral-medium-latest"),
            ("minimax", "MiniMax-M2.7"),
            ("minimax-cn", "MiniMax-M2.7"),
            ("moonshotai", "kimi-k2.6"),
            ("moonshotai-cn", "kimi-k2.6"),
            ("huggingface", "moonshotai/Kimi-K2.6"),
            ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
            ("together", "moonshotai/Kimi-K2.6"),
            ("baseten", "zai-org/GLM-5.2"),
            ("opencode", "kimi-k2.6"),
            ("opencode-go", "kimi-k2.6"),
            ("kimi-coding", "kimi-for-coding"),
            ("meta", "muse-spark-1.3"),
            ("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
            (
                "cloudflare-ai-gateway",
                "workers-ai/@cf/moonshotai/kimi-k2.6",
            ),
            ("qwen-token-plan", "qwen3.7-max"),
            ("qwen-token-plan-cn", "qwen3.7-max"),
            ("qwen-token-plan-individual", "qwen3.8-max"),
            ("xiaomi", "mimo-v2.5-pro"),
            ("xiaomi-token-plan-cn", "mimo-v2.5-pro"),
            ("xiaomi-token-plan-ams", "mimo-v2.5-pro"),
            ("xiaomi-token-plan-sgp", "mimo-v2.5-pro"),
        ];

        /// Rows where PI'S OWN curated default names an id pi's own catalog no longer serves.
        ///
        /// This is not a cyrup gap and must not be "fixed" into one. `defaultModelPerProvider`
        /// @v0.87.1 (`model-resolver.ts:46,50`) still reads
        /// `fireworks: "accounts/fireworks/models/kimi-k2p6"` and `"opencode-go": "kimi-k2.6"`,
        /// while the catalogs pi generates and publishes for those two providers carry
        /// `accounts/fireworks/models/kimi-k3` and no `kimi-k2.6` at all. pi therefore falls through
        /// its own curated default for these providers exactly as cyrup does, and picking a
        /// "better" successor here would be cyrup inventing upstream behaviour — the one thing the
        /// table is not allowed to do. The rows are listed so the guard below can skip them BY
        /// NAME, with this reason attached, rather than being weakened for everybody.
        const STALE_UPSTREAM: &[(&str, &str)] = &[
            ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
            ("opencode-go", "kimi-k2.6"),
        ];
        /// v0.87.1 rows cyrup does NOT carry at pi's value: `(provider, what cyrup carries)`, where
        /// `None` means no row at all. `meta`: the Meta Muse provider (pi v0.86.1) is not shipped —
        /// area 01 (PROV-080). Every other row, `radius`/`xai`/`cerebras`/`zai`/`zai-coding-cn`
        /// included, is pi's own value now that PROV-071's live catalogs carry the ids.
        const DEFERRED: &[(&str, Option<&str>)] = &[("meta", None)];

        let expected: Vec<(&str, &str)> = PI
            .iter()
            .filter_map(|(k, v)| match DEFERRED.iter().find(|(dk, _)| dk == k) {
                Some((_, carried)) => carried.map(|c| (*k, c)),
                None => Some((*k, *v)),
            })
            .collect();
        let ours: Vec<(&str, &str)> = KNOWN_PROVIDERS
            .iter()
            .map(|p| (*p, default_model_per_provider(p).unwrap_or("<missing>")))
            .collect();
        assert_eq!(ours, expected);
        assert_eq!(KNOWN_PROVIDERS.len(), 40);

        // A deferred row must still be deferred: catching up with pi without removing it from
        // `DEFERRED` fails here.
        for (key, _) in DEFERRED {
            let pi = PI.iter().find(|(k, _)| k == key).map(|(_, v)| *v);
            assert_ne!(default_model_per_provider(key), pi, "{key} now matches pi");
        }

        // THE GUARD. Every curated default must name a model the shipped catalog actually carries.
        // A provider with no embedded rows is skipped because it gets its catalog at runtime, and
        // `cyrup-provider`'s own `every_registered_provider_has_a_non_empty_catalog` asserts exactly
        // which providers those are, so a silently-empty catalog cannot hide behind the skip.
        //
        // DRIFT-009 shrank that skip from five providers to one. `baseten` and the three
        // `qwen-token-plan*` providers used to have zero embedded rows, so four of the entries in
        // `PI` above were checked for spelling and nothing else — `zai-org/GLM-5.2`, `qwen3.7-max`
        // twice and `qwen3.8-max` could have named anything. Now that all four ship catalogs those
        // four defaults are really resolved, and `radius` is the only provider left where this guard
        // is vacuous. `MEANINGFUL_NOW` pins that gain: if one of the four regressed to an empty
        // catalog the loop below would go quiet again and this assertion is what would notice.
        const MEANINGFUL_NOW: &[&str] = &[
            "baseten",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
        ];
        let mut checked: Vec<&str> = Vec::new();
        for provider in cyrup_provider::all_providers() {
            let id = provider.id().as_str();
            let Some(default_id) = default_model_per_provider(id) else {
                continue;
            };
            if provider.models().is_empty() {
                assert!(
                    !MEANINGFUL_NOW.contains(&id),
                    "{id} ships no embedded rows, so its curated default `{default_id}` is \
                     unchecked again — DRIFT-009 embedded this catalog precisely so it would be"
                );
                continue;
            }
            if let Some((_, stale)) = STALE_UPSTREAM.iter().find(|(k, _)| *k == id) {
                // The skip is itself asserted: it applies only while cyrup's value is still pi's.
                // If somebody edits the table to a successor id, this fires instead of quietly
                // letting the edit through as an upstream-faithful value.
                assert_eq!(
                    default_model_per_provider(id),
                    Some(*stale),
                    "{id} is listed as stale-upstream, so cyrup must carry pi's own value \
                     verbatim; if you changed it, you changed behaviour pi does not have"
                );
                continue;
            }
            if MEANINGFUL_NOW.contains(&id) {
                checked.push(
                    MEANINGFUL_NOW
                        .iter()
                        .find(|m| **m == id)
                        .copied()
                        .expect("id"),
                );
            }
            assert!(
                provider
                    .models()
                    .iter()
                    .any(|m| m.id.as_str() == default_id),
                "{id}'s curated default `{default_id}` is not in its catalog — \
                 `first_default_or_first` will skip {id} entirely and launch the user on whatever \
                 sorts first. Either the upstream table moved (chase it) or the model was retired \
                 (pick its successor)."
            );
        }
        checked.sort_unstable();
        assert_eq!(
            checked, MEANINGFUL_NOW,
            "every DRIFT-009 provider must reach the guard above"
        );
    }
}
