//! Model + capability/cost metadata (arch-01 §4.2 / func-01 §4.2).

use crate::HeaderMap;
use crate::api::compat::OpenAiCompletionsCompat;
use cyrup_core::{ApiId, ModelId, ProviderId};

/// Maps pi thinking levels (`off`/`minimal`/`low`/`medium`/`high`/`xhigh`/`max`) to provider/model
/// specific reasoning values. Mirrors Pi's `ThinkingLevelMap = Partial<Record<ModelThinkingLevel,
/// string | null>>`: a missing key uses the provider default, a `null` value marks the level
/// unsupported, and a string overrides the wire value sent for that level.
pub type ThinkingLevelMap = std::collections::BTreeMap<String, Option<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
}

/// One request-wide pricing tier (Pi `ModelCostTier`, types.ts:750-753). Carries the same four
/// per-1e6-token rates as [`ModelCost`] plus the input-token threshold above which they apply.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    /// Use this tier for requests whose total input usage *exceeds* this token count (strict `>`).
    pub input_tokens_above: u64,
    /// USD per 1e6 tokens.
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Per-1e6-token rates for a model (Pi `ModelCost extends ModelCostRates`, types.ts:743-758).
///
/// `tiers` is Pi's long-context pricing ladder: the highest matching `inputTokensAbove` threshold
/// replaces *all four* base rates for the whole request (see [`crate::usage::compute_cost`], a port
/// of Pi `calculateCost`, models.ts:639-658). Additive and defaulted, so a catalog entry or a
/// provider-registered model that omits it keeps flat pricing.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// USD per 1e6 tokens.
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// Prompt-cache lifetimes in seconds, per retention tier (Pi
/// `ModelPromptCache = Partial<Record<Exclude<CacheRetention, "none">, number>>`,
/// `packages/ai/src/types.ts:118` @v1.0.4). `none` is excluded upstream by construction: asking for
/// no caching has no lifetime.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPromptCache {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long: Option<u64>,
}

/// Lifetime in MILLISECONDS of the prompt-cache entry a request writes, from the model's
/// [`ModelPromptCache`] tier for the retention that request used — Pi `getPromptCacheTtlMs`
/// (`core/cache-warmer.ts:38-47` @v1.0.4).
///
/// `None` when the model publishes no lifetime for that tier, or when the request asked for
/// [`CacheRetention::None`]. Prompt-cache warming treats `None` as "do not schedule anything".
///
/// **The retention ladder here defaults to `Short`, and that is deliberate.** It is
/// [`crate::utils::provider_plumbing::resolve_cache_retention`], the ladder every wire adapter
/// applies (`anthropic-messages.ts:46-54` and siblings): explicit caller value, else
/// `CYRUP_CACHE_RETENTION == "long"`, else `Short`. It is NOT
/// `api::pi_messages::resolve_cache_retention`, which deliberately leaves an unset retention unset
/// so a gateway's own policy wins — reusing that one here would make a cached request look
/// lifetime-less and silently disable warming for every pi-messages model. Upstream's warmer
/// inlines the short-defaulting form for the same reason (`cache-warmer.ts:39-41`).
///
/// pi reads `PI_CACHE_RETENTION`; the spelling is renamed with the rest of the public env surface.
#[must_use]
pub fn prompt_cache_ttl_ms(
    model: &Model,
    cache_retention: Option<crate::CacheRetention>,
    env: Option<&crate::ProviderEnv>,
) -> Option<u64> {
    use crate::CacheRetention;
    let retention = crate::utils::provider_plumbing::resolve_cache_retention(
        cache_retention,
        crate::utils::provider_plumbing::EnvSource::new(env),
    );
    let tiers = model.prompt_cache.as_ref()?;
    let seconds = match retention {
        CacheRetention::None => return None,
        CacheRetention::Short => tiers.short,
        CacheRetention::Long => tiers.long,
    }?;
    Some(seconds.saturating_mul(1_000))
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: ModelId,
    pub name: String,
    pub api: ApiId,
    pub provider: ProviderId,
    /// Required base URL for the provider endpoint (Pi `Model.baseUrl: string`, types.ts:671).
    pub base_url: String,
    pub reasoning: bool,
    pub input: Vec<Modality>,
    pub cost: ModelCost,
    /// Best-effort prompt-cache lifetime in SECONDS for each retention tier a request can ask
    /// for (Pi `Model.promptCache: ModelPromptCache`, `packages/ai/src/types.ts:1127` @v1.0.4,
    /// declared immediately before `contextWindow` — which is why it sits here).
    ///
    /// Absent means the model publishes no lifetime, which is not the same as caching being off:
    /// a request can still write a cache entry, we just do not know when it expires. The one
    /// consumer that needs the number is prompt-cache warming
    /// ([`prompt_cache_ttl_ms`]), which declines to schedule anything without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<ModelPromptCache>,
    pub context_window: u64,
    pub max_tokens: u64,
    /// Default sampling parameters for this model (Pi `Model.samplingParams`, types.ts:801-802
    /// @v0.84.1, introduced by `25a2c8dcf`), declared in pi immediately after `maxTokens` — which is
    /// why it sits here. Per-request [`crate::StreamOptions::sampling_params`] keys override these,
    /// merged per key by [`crate::utils::simple_options::build_base_options`]
    /// (`simple-options.ts:27-33`). Only the OpenAI-compatible adapters apply the result; every
    /// other api ignores it. Additive, defaulted to `None`. AGENT-026.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sampling_params: Option<serde_json::Map<String, serde_json::Value>>,
    /// Per-level reasoning value overrides (Pi `Model.thinkingLevelMap`). Additive, defaulted.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    /// OpenAI-completions compatibility overrides (Pi `Model.compat`). When unset, the wire impl
    /// auto-detects from `provider` + `base_url`. Additive, defaulted.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compat: Option<OpenAiCompletionsCompat>,
    /// Top-level per-provider request headers (Pi `Model.headers`, types.ts). Merged into the
    /// outgoing request below the per-request `StreamOptions.headers` overlay (auth overlay <
    /// `model.headers` < `opts.headers`); a `None` value suppresses a default header. Additive,
    /// defaulted to `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub headers: Option<HeaderMap>,
}

impl Model {
    pub fn supports_image_input(&self) -> bool {
        self.input.contains(&Modality::Image)
    }
}
