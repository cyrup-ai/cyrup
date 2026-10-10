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

/// One free-form sampling map (Pi `SamplingParams = Record<string, unknown>`,
/// `packages/ai/src/types.ts` @f1b2e77f5; schema `SamplingParamsSchema`,
/// `coding-agent/src/core/model-config.ts:19`).
pub type SamplingParams = serde_json::Map<String, serde_json::Value>;

/// Per-thinking-level sampling overrides (Pi `SamplingParamsByThinkingLevel =
/// Partial<Record<ModelThinkingLevel, SamplingParams>>`, `packages/ai/src/types.ts:134`
/// @f1b2e77f5; schema `SamplingParamsByThinkingLevelSchema`, `core/model-config.ts:20-28`). CFG-104.
///
/// A struct with the seven pi keys rather than a map, because [`cyrup_core::ModelThinkingLevel`] is
/// not `Ord` and pi's schema is a closed `Type.Object` of exactly these optional keys. Like that
/// `Type.Object`, an unknown key is tolerated (serde ignores it) rather than rejected.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplingParamsByThinkingLevel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xhigh: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<SamplingParams>,
}

impl SamplingParamsByThinkingLevel {
    /// Pi `samplingParamsByThinkingLevel?.[level]`. Matched exhaustively so a new rung cannot be
    /// silently unreachable.
    #[must_use]
    pub fn get(&self, level: cyrup_core::ModelThinkingLevel) -> Option<&SamplingParams> {
        self.slot(level).as_ref()
    }

    /// The level's slot, for the per-level override merge.
    pub fn slot_mut(
        &mut self,
        level: cyrup_core::ModelThinkingLevel,
    ) -> &mut Option<SamplingParams> {
        use cyrup_core::ModelThinkingLevel as L;
        match level {
            L::Off => &mut self.off,
            L::Minimal => &mut self.minimal,
            L::Low => &mut self.low,
            L::Medium => &mut self.medium,
            L::High => &mut self.high,
            L::Xhigh => &mut self.xhigh,
            L::Max => &mut self.max,
        }
    }

    fn slot(&self, level: cyrup_core::ModelThinkingLevel) -> &Option<SamplingParams> {
        use cyrup_core::ModelThinkingLevel as L;
        match level {
            L::Off => &self.off,
            L::Minimal => &self.minimal,
            L::Low => &self.low,
            L::Medium => &self.medium,
            L::High => &self.high,
            L::Xhigh => &self.xhigh,
            L::Max => &self.max,
        }
    }

    /// Pi's iteration order in `mergeSamplingParamsByThinkingLevel`
    /// (`core/provider-composer.ts:168` @f1b2e77f5).
    pub const LEVELS: [cyrup_core::ModelThinkingLevel; 7] = [
        cyrup_core::ModelThinkingLevel::Off,
        cyrup_core::ModelThinkingLevel::Minimal,
        cyrup_core::ModelThinkingLevel::Low,
        cyrup_core::ModelThinkingLevel::Medium,
        cyrup_core::ModelThinkingLevel::High,
        cyrup_core::ModelThinkingLevel::Xhigh,
        cyrup_core::ModelThinkingLevel::Max,
    ];
}

/// A model's default sampling parameters: pi's two SIBLING fields `Model.samplingParams` and
/// `Model.samplingParamsByThinkingLevel` (`packages/ai/src/types.ts:851-854` @f1b2e77f5), held in
/// one Rust field. CFG-104.
///
/// [CYRUP-DELTA] Rust shape only — the JSON shape is pi's: [`Model`] (de)serializes the two keys
/// separately through `ModelRepr`. They share one `Option` so that every exhaustive `Model { ..,
/// sampling_params: None, .. }` literal in the tree keeps compiling when the per-level half was
/// added (one such literal sits in a module that could not be edited in the same change). Both
/// halves absent is represented as the outer `None`, never `Some` of two `None`s —
/// [`ModelSamplingParams::new`] enforces that, so `PartialEq` on [`Model`] is not split by it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelSamplingParams {
    /// Pi `Model.samplingParams`: applied at every thinking level.
    pub params: Option<SamplingParams>,
    /// Pi `Model.samplingParamsByThinkingLevel`: spread over [`Self::params`] for the effective
    /// (clamped) level by `resolve_sampling_params`.
    pub by_thinking_level: Option<SamplingParamsByThinkingLevel>,
}

impl ModelSamplingParams {
    /// `None` when both halves are absent, so "no sampling defaults" has one representation.
    #[must_use]
    pub fn new(
        params: Option<SamplingParams>,
        by_thinking_level: Option<SamplingParamsByThinkingLevel>,
    ) -> Option<Self> {
        (params.is_some() || by_thinking_level.is_some()).then_some(Self {
            params,
            by_thinking_level,
        })
    }

    /// Flat-map-only defaults (pi `samplingParams` with no per-level map).
    #[must_use]
    pub fn flat(params: SamplingParams) -> Self {
        Self {
            params: Some(params),
            by_thinking_level: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "ModelRepr", into = "ModelRepr")]
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
    /// Default sampling parameters for this model: Pi `Model.samplingParams` (types.ts:801-802
    /// @v0.84.1, introduced by `25a2c8dcf`; AGENT-026) AND `Model.samplingParamsByThinkingLevel`
    /// (`types.ts:851-854` @f1b2e77f5, added `76dfb88f6`; CFG-104), declared in pi immediately
    /// after `maxTokens` — which is why it sits here. See [`ModelSamplingParams`] for why the two
    /// pi fields share one Rust field.
    ///
    /// Resolved per request by [`crate::utils::simple_options::resolve_sampling_params`] (pi
    /// `resolveSamplingParams`, `api/simple-options.ts:24-34` @f1b2e77f5): flat map, then the
    /// effective level's map, then the per-request [`crate::StreamOptions::sampling_params`], per
    /// key. Only the OpenAI-compatible adapters apply the result; every other api ignores it.
    pub sampling_params: Option<ModelSamplingParams>,
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

    /// Pi `model.samplingParams`: the flat, every-level defaults.
    #[must_use]
    pub fn flat_sampling_params(&self) -> Option<&SamplingParams> {
        self.sampling_params.as_ref()?.params.as_ref()
    }

    /// Pi `model.samplingParamsByThinkingLevel`.
    #[must_use]
    pub fn sampling_params_by_thinking_level(&self) -> Option<&SamplingParamsByThinkingLevel> {
        self.sampling_params.as_ref()?.by_thinking_level.as_ref()
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

/// The serde shape of [`Model`]: pi's JSON keys, with `samplingParams` and
/// `samplingParamsByThinkingLevel` as the separate siblings they are upstream
/// (`packages/ai/src/types.ts:851-854` @f1b2e77f5). Field-for-field the same as [`Model`]
/// otherwise; see [`ModelSamplingParams`] for why the Rust struct differs.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelRepr {
    id: ModelId,
    name: String,
    api: ApiId,
    provider: ProviderId,
    base_url: String,
    reasoning: bool,
    input: Vec<Modality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_limits: Option<ModelInputLimits>,
    cost: ModelCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_cache: Option<ModelPromptCache>,
    context_window: u64,
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    sampling_params: Option<SamplingParams>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    sampling_params_by_thinking_level: Option<SamplingParamsByThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    thinking_level_map: Option<ThinkingLevelMap>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    compat: Option<OpenAiCompletionsCompat>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    headers: Option<HeaderMap>,
}

impl From<ModelRepr> for Model {
    fn from(r: ModelRepr) -> Self {
        Model {
            id: r.id,
            name: r.name,
            api: r.api,
            provider: r.provider,
            base_url: r.base_url,
            reasoning: r.reasoning,
            input: r.input,
            input_limits: r.input_limits,
            cost: r.cost,
            prompt_cache: r.prompt_cache,
            context_window: r.context_window,
            max_tokens: r.max_tokens,
            sampling_params: ModelSamplingParams::new(
                r.sampling_params,
                r.sampling_params_by_thinking_level,
            ),
            thinking_level_map: r.thinking_level_map,
            compat: r.compat,
            headers: r.headers,
        }
    }
}

impl From<Model> for ModelRepr {
    fn from(m: Model) -> Self {
        let (sampling_params, sampling_params_by_thinking_level) = match m.sampling_params {
            Some(s) => (s.params, s.by_thinking_level),
            None => (None, None),
        };
        ModelRepr {
            id: m.id,
            name: m.name,
            api: m.api,
            provider: m.provider,
            base_url: m.base_url,
            reasoning: m.reasoning,
            input: m.input,
            input_limits: m.input_limits,
            cost: m.cost,
            prompt_cache: m.prompt_cache,
            context_window: m.context_window,
            max_tokens: m.max_tokens,
            sampling_params,
            sampling_params_by_thinking_level,
            thinking_level_map: m.thinking_level_map,
            compat: m.compat,
            headers: m.headers,
        }
    }
}
