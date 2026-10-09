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

/// The cache-safe resize profile a model row declares, re-exported from its home in `cyrup-core`.
///
/// Upstream declares these four keys twice over — `ModelImageResizeOptions` beside the catalog
/// types (`packages/ai/src/types.ts:1075-1081` @v1.0.4) and `ImageResizeOptions` beside the resizer
/// (`packages/coding-agent/src/utils/image-resize-core.ts:4-9` @v1.0.4) — and passes one straight
/// into the other (`agent-session.ts:1933`: `resizeOptions: this._limitsModel()?.inputLimits?.images?.resize`).
/// cyrup therefore has ONE type, and it is homed in `cyrup-core` because that is the only crate
/// below both this one and `cyrup-tools`, where the resizer lives. The path
/// `cyrup_provider::ModelImageResizeOptions` is unchanged for every catalog-side caller.
pub use cyrup_core::ModelImageResizeOptions;

/// Per-image provider input limits (Pi `ModelImageInputLimits`, `types.ts:1083-1090` @v1.0.4).
///
/// Only [`Self::resize`] has a consumer. [`Self::max_per_message`] and [`Self::max_per_request`]
/// are modelled, parsed and round-tripped WITHOUT enforcement, which is parity and not a
/// shortfall: upstream declares, generates, schema-validates and tests them but has no runtime
/// reader, and says so — *"Pi does not yet rewrite or reject history based on them"*
/// (`packages/coding-agent/docs/models.md:89` @v1.0.4). Dropping them instead would lose catalog
/// information on every parse and every write back to the models store.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelImageInputLimits {
    /// Cache-safe resize profile applied before a new image enters conversation history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resize: Option<ModelImageResizeOptions>,
    /// Maximum images accepted in one provider MESSAGE. Descriptive only — see the type docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_message: Option<u64>,
    /// Maximum images accepted across one provider REQUEST. Descriptive only — see the type docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_request: Option<u64>,
}

/// Provider input limits and cache-safe preprocessing metadata (Pi `ModelInputLimits`,
/// `types.ts:1092-1096` @v1.0.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInputLimits {
    /// Maximum serialized provider request size in bytes. Descriptive only — see
    /// [`ModelImageInputLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<ModelImageInputLimits>,
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
    /// Provider input limits and cache-safe image preprocessing metadata (Pi
    /// `BaseModel.inputLimits: ModelInputLimits`, `packages/ai/src/types.ts:1105` @v1.0.4,
    /// declared immediately after `input` and immediately before `cost` — which is why it sits
    /// here).
    ///
    /// It is on `BaseModel`, so chat, image AND classifier rows all carry it
    /// ([`crate::ImageModel::input_limits`], [`crate::ClassifierModel::input_limits`]).
    ///
    /// The one consumer is `inputLimits.images.resize`: the cache-safe profile a new image is
    /// resized to before it enters conversation history, read off the REQUEST model
    /// (`agent-session.ts:1933` and `:694`, `tools/read.ts:138` @v1.0.4). The other three keys are
    /// descriptive — see [`ModelImageInputLimits`].
    ///
    /// Absent means the row publishes no limits; the resizer then falls back to the shared default
    /// profile ([`crate::catalog::DEFAULT_IMAGE_RESIZE`]), which is the same 2000px / 4.5 MiB /
    /// quality-80 profile the generator stamps, so a stamped and an unstamped row resolve alike.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<ModelInputLimits>,
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

    /// This model's cache-safe image resize profile, i.e. pi's
    /// `model.inputLimits?.images?.resize` (`core/agent-session.ts:1932`,
    /// `core/tools/read.ts:138` @v1.0.4).
    ///
    /// One accessor rather than the three-`and_then` chain repeated at each consumer: the prompt
    /// normalizer, the `read` tool's live handle and every site that re-pushes it on a model switch
    /// all want exactly this projection, and `None` is the answer at any of the three levels —
    /// which the resizer resolves per key against [`crate::DEFAULT_IMAGE_RESIZE`].
    #[must_use]
    pub fn image_resize_profile(&self) -> Option<cyrup_core::ModelImageResizeOptions> {
        self.input_limits.as_ref()?.images.as_ref()?.resize.clone()
    }
}
