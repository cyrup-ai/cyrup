//! Applying a `models.json` over the built-in registry: per-provider composition, the
//! `modelOverrides` patch layer, compat merging, and the configured-provider predicate both
//! the binary and the session ask (CFG-002, CFG-022).

use cyrup_core::ProviderId;
use cyrup_provider::Model;

use super::schema::{ModelDefinition, ModelFile, ModelOverride, ProviderConfig};

impl ModelFile {
    /// Compose `base` (the built-in / provider-supplied registry) with this `models.json`, returning
    /// the effective model list plus one message per rejected provider block.
    ///
    /// 1:1 with Pi's `composeModelProvider` restricted to the credential-blind layers
    /// (provider-composer.ts:411-437): for every provider id in the union of `base` and the file,
    /// `applyModelsJson` rewrites `baseUrl`/`compat` on the built-ins and upserts the declared
    /// `models` (:161-199), then `modelOverrides` patches the result last (:433-436).
    ///
    /// A provider block that Pi would `throw` on (no distinguishing key, a custom model with no
    /// resolvable `api`/`baseUrl`, a non-positive `contextWindow`/`maxTokens`) is REJECTED WHOLE —
    /// its built-in models are kept untouched — and its message is returned. Pi's own
    /// `compositionErrors` map does exactly this (model-runtime.ts:104), so a single bad block never
    /// costs the user the rest of the registry.
    ///
    /// Provider ORDER follows Pi's `rebuildProviders` (model-runtime.ts:225-231): it iterates
    /// `providerIds()` = `builtins ∪ … ∪ config.getProviderIds()`, a `Set` whose iteration order is
    /// insertion order, so the built-ins keep their registration order and a provider that exists
    /// only in `models.json` is appended after them. Composition REPLACES a provider's entries in
    /// place (`models.setProvider(...)`, :215) — it never appends a second, shadowed copy.
    pub fn compose(&self, base: &[Model]) -> (Vec<Model>, Vec<String>) {
        let mut errors: Vec<String> = Vec::new();
        let mut out: Vec<Model> = Vec::new();
        // Pi's `providerIds()` order: base providers first (first-seen), then the file's own.
        let mut order: Vec<&str> = Vec::new();
        for m in base {
            if !order.contains(&m.provider.as_str()) {
                order.push(m.provider.as_str());
            }
        }
        for provider_id in self.providers.keys() {
            if !order.contains(&provider_id.as_str()) {
                order.push(provider_id.as_str());
            }
        }
        for provider_id in order {
            let base_models: Vec<Model> = base
                .iter()
                .filter(|m| m.provider.as_str() == provider_id)
                .cloned()
                .collect();
            let Some(config) = self.providers.get(provider_id) else {
                // No overlay: the built-in stands untouched (Pi :210-214).
                out.extend(base_models);
                continue;
            };
            match apply_models_json(provider_id, &base_models, config) {
                Ok(models) => out.extend(models),
                Err(msg) => {
                    errors.push(msg);
                    // Keep the untouched built-ins for this provider (Pi records the error and
                    // re-registers `base`, model-runtime.ts:218-221).
                    out.extend(base_models);
                }
            }
        }
        (out, errors)
    }
}

/// Whether `provider` has configured auth, across BOTH credential channels Pi's `hasConfiguredAuth`
/// sees — the credential store AND `models.json` (CFG-022).
///
/// Pi's `hasConfiguredAuth` (model-runtime.ts:372-374) is a set-membership test against
/// `snapshot.configuredProviders`, and that set is filled by running `models.checkAuth` over EVERY
/// composed provider. A provider that exists only in `models.json` is composed like any other, and
/// its `check` closure is `composeApiKeyAuth`'s (provider-composer.ts:314-332). So a user-declared
/// provider carrying its own `apiKey` counts as configured with nothing in `auth.json` at all.
///
/// cyrup had two disagreeing predicates: [`crate::AuthStore::has_auth`] alone on the binary's default-launch
/// path (which knows only `--api-key`, an `auth.json` entry, and the `env_keys` table of KNOWN
/// provider ids, so a user-declared provider matched none of the three), and a second, models.json-
/// aware one inside the session. This is the single predicate both call.
///
/// `env` is the optional provider-scoped override map; it is consulted ahead of the process
/// environment by both tiers.
pub fn provider_is_configured(
    auth: &crate::auth::AuthStore,
    models_json: &ModelFile,
    provider: &ProviderId,
    env: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    auth.has_auth(provider, env)
        || models_json_provider_is_configured(models_json, provider.as_str(), env)
}

/// The `models.json` tier of [`provider_is_configured`]: a declared `apiKey` that is *configured*
/// in the config-value sense (Pi `composeApiKeyAuth`'s `check`, provider-composer.ts:320-329).
///
/// **NEVER RESOLVES THE VALUE.** Pi's own check is deliberately pure — a `!command` value returns
/// "configured API key" on the strength of *being* a command (`isCommandConfigValue`, :321) without
/// running it, and a `$VAR` template is configured exactly when every name it references is defined
/// (:322-328). Resolving here would execute a shell command out of `models.json` on a *status*
/// query, on a predicate that runs inside filter loops; resolution belongs on the request path.
///
/// The env-var arm is what distinguishes this from a bare `api_key.is_some()`: a template naming an
/// unset variable is NOT configured, which is the same judgement Pi makes.
pub fn models_json_provider_is_configured(
    models_json: &ModelFile,
    provider_id: &str,
    env: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    let Some(raw) = models_json
        .providers
        .get(provider_id)
        .and_then(|c| c.api_key.as_deref())
    else {
        // Credential *acquisition* (an `oauth` block) is deliberately out of scope: Pi's
        // `composeApiKeyAuth` returns `undefined` outright for an oauth-only provider (:302).
        return false;
    };
    crate::config_value::is_command_config_value(raw)
        || crate::config_value::is_config_value_configured(raw, env)
}

/// Pi `applyModelsJson` + `modelFromJson` + the `modelOverrides` map
/// (provider-composer.ts:161-199, 124-159, 433-436), as one fallible composition over ONE provider's
/// models. Returns the provider's effective model list, or Pi's own error string.
pub(crate) fn apply_models_json(
    provider_id: &str,
    base_models: &[Model],
    config: &ProviderConfig,
) -> Result<Vec<Model>, String> {
    // `oauth` names an auth gateway, and the gateway has to live somewhere: Pi checks this FIRST,
    // ahead of the empty-block guard, so `{"oauth":"radius"}` reports the missing `baseUrl` rather
    // than the generic "must specify …" (provider-composer.ts:167-169).
    if config.oauth.is_some() && config.base_url.is_none() {
        return Err(format!(
            "Provider {provider_id}: \"baseUrl\" is required when \"oauth\" is set."
        ));
    }
    let has_overrides = !config.model_overrides.is_empty();
    if config.models.is_empty()
        && config.base_url.is_none()
        && config.headers.is_none()
        && config.compat.is_none()
        && !has_overrides
        && config.api_key.is_none()
        // `!config.oauth` (:178) — an oauth mode is itself a distinguishing key.
        && config.oauth.is_none()
        && config.auth_header.is_none()
    {
        return Err(format!(
            "Provider {provider_id}: must specify \"baseUrl\", \"headers\", \"compat\", \
             \"modelOverrides\", or \"models\"."
        ));
    }

    // Step 1: rewrite every built-in with the provider-level baseUrl + compat (:186-190).
    let mut models: Vec<Model> = base_models
        .iter()
        .map(|m| {
            let mut m = m.clone();
            // `config.oauth === "radius" ? model.baseUrl : (config.baseUrl ?? model.baseUrl)`
            // (:188): under an oauth mode the block's `baseUrl` is the auth gateway, so the models
            // keep their own request endpoints. `oauth` is single-valued, so `is_none()` is the
            // exact negation of Pi's `=== "radius"`.
            if let Some(base_url) = &config.base_url
                && config.oauth.is_none()
            {
                m.base_url = base_url.clone();
            }
            m.compat = merge_compat(m.compat.as_ref(), config.compat.as_ref());
            m
        })
        .collect();

    // Step 2: upsert each declared model (:191-197).
    for definition in &config.models {
        let existing = models.iter().position(|m| m.id.as_str() == definition.id);
        // `findModelDefaults(models, definition.id, definition.api ?? config.api)`
        // (provider-composer.ts:239). The third argument is the DEFINITION's api first, then the
        // provider block's — passing only the definition's reproduces half of CFG-092 whenever the
        // wire api is declared at provider level.
        let defaults = find_model_defaults(
            &models,
            &definition.id,
            definition.api.as_deref().or(config.api.as_deref()),
        );
        let model = model_from_json(provider_id, definition, config, defaults)?;
        match existing {
            Some(i) => {
                if let Some(slot) = models.get_mut(i) {
                    *slot = model;
                }
            }
            None => models.push(model),
        }
    }

    // Step 3: modelOverrides are the topmost user-config layer (:433-436). Upstream validates an
    // override's `inputLimits` against the SAME `ModelInputLimitsSchema` as a definition's
    // (`model-config.ts:223` vs `:206`), so the bounds are checked on both layers — and on every
    // override in the block, not only the ones whose id matched a model, because the schema layer
    // upstream never looks at whether the id resolves.
    for (model_id, ov) in &config.model_overrides {
        validate_input_limits(provider_id, model_id, ov.input_limits.as_ref())?;
    }
    for m in &mut models {
        if let Some(ov) = config.model_overrides.get(m.id.as_str()) {
            apply_model_override(m, ov);
        }
    }
    Ok(models)
}

/// Pi `findModelDefaults` (provider-composer.ts:198-205 @v0.87.1): which existing model a declared
/// `models.json` entry inherits `api` / `baseUrl` from, as four rungs tried in order.
///
/// 1. the same `id` — an override of a built-in inherits from the built-in it replaces;
/// 2. the same wire `api`, when the caller knows one — so an `anthropic-messages` entry cannot take
///    an `openai-responses` model's endpoint;
/// 3. the first `openai-completions` model — upstream's de-facto default wire api;
/// 4. the first model at all.
///
/// New upstream at v0.85.0 (absent at v0.84.4): rungs 2 and 3 did not exist when the cyrup port was
/// written, which is why CFG-092 is drift rather than a porting mistake.
fn find_model_defaults<'a>(
    models: &'a [Model],
    model_id: &str,
    api: Option<&str>,
) -> Option<&'a Model> {
    models
        .iter()
        .find(|m| m.id.as_str() == model_id)
        .or_else(|| api.and_then(|api| models.iter().find(|m| m.api.as_str() == api)))
        .or_else(|| {
            models
                .iter()
                .find(|m| m.api.as_str() == "openai-completions")
        })
        .or_else(|| models.first())
}

/// Pi `modelFromJson` (provider-composer.ts:124-159): build one `Model` from a `models.json`
/// definition, inheriting `api`/`baseUrl` from the provider block and then from the same-id built-in.
fn model_from_json(
    provider_id: &str,
    definition: &ModelDefinition,
    provider_config: &ProviderConfig,
    defaults: Option<&Model>,
) -> Result<Model, String> {
    let api = definition
        .api
        .clone()
        .or_else(|| provider_config.api.clone())
        .or_else(|| defaults.map(|d| d.api.as_str().to_string()))
        .ok_or_else(|| {
            format!(
                "Provider {provider_id}, model {}: no \"api\" specified. Set at provider or model \
                 level.",
                definition.id
            )
        })?;
    let base_url = definition
        .base_url
        .clone()
        .or_else(|| provider_config.base_url.clone())
        .or_else(|| defaults.map(|d| d.base_url.clone()))
        .ok_or_else(|| {
            format!("Provider {provider_id}: \"baseUrl\" is required when defining custom models.")
        })?;
    // `definition.contextWindow !== undefined && definition.contextWindow <= 0`
    // (provider-composer.ts:138-143 @v0.83.0) — NOT `=== 0`. CFG-046.
    if definition.context_window.is_some_and(|v| v <= 0) {
        return Err(format!(
            "Provider {provider_id}, model {}: invalid contextWindow",
            definition.id
        ));
    }
    if definition.max_tokens.is_some_and(|v| v <= 0) {
        return Err(format!(
            "Provider {provider_id}, model {}: invalid maxTokens",
            definition.id
        ));
    }
    validate_input_limits(
        provider_id,
        &definition.id,
        definition.input_limits.as_ref(),
    )?;
    Ok(Model {
        id: definition.id.as_str().into(),
        name: definition
            .name
            .clone()
            .unwrap_or_else(|| definition.id.clone()),
        api: api.as_str().into(),
        provider: provider_id.into(),
        base_url,
        reasoning: definition.reasoning.unwrap_or(false),
        input: definition
            .input
            .clone()
            .unwrap_or_else(|| vec![cyrup_provider::Modality::Text]),
        // `inputLimits: definition.inputLimits` (provider-composer.ts:239 @v1.0.4) — verbatim,
        // with NO provider-block or same-id-builtin fallback, exactly like `prompt_cache` below.
        // The generator stamp (`apply_image_input_metadata`) deliberately does NOT run here: it is
        // gated to catalog parse sites, as upstream's is generator-only. A row that leaves this
        // unset therefore carries no profile and the resizer falls back to
        // `cyrup_provider::DEFAULT_IMAGE_RESIZE`, which is byte-for-byte the same 2000/2000/4.5
        // MiB/80 profile the generator would have stamped (`image-resize-core.ts:24-29` ==
        // `generate-models.ts:424-430`). CFG-085.
        input_limits: definition.input_limits.clone(),
        cost: definition.cost.clone().unwrap_or_default(),
        // `promptCache: definition.promptCache` (provider-composer.ts:241 @v1.0.4) — verbatim, no
        // provider-level or built-in fallback. `apply_prompt_cache_metadata` fills direct-Anthropic
        // rows that leave it unset; a row that sets it keeps what it set.
        prompt_cache: definition.prompt_cache.clone(),
        // Both are guaranteed `> 0` by the checks above, so the cast is total.
        context_window: definition.context_window.map_or(128_000, |v| v as u64),
        max_tokens: definition.max_tokens.map_or(16_384, |v| v as u64),
        // `samplingParams: definition.samplingParams` (provider-composer.ts:158 @v0.84.1) — copied
        // verbatim, with NO fallback to `providerConfig` or `defaults`: the provider block has no
        // `samplingParams` key in `ProviderConfigSchema`, and a same-id built-in's defaults are
        // deliberately not inherited here. CFG-039. `samplingParamsByThinkingLevel:
        // definition.samplingParamsByThinkingLevel` (provider-composer.ts:245 @f1b2e77f5) is copied
        // the same way. CFG-104.
        sampling_params: cyrup_provider::ModelSamplingParams::new(
            definition.sampling_params.clone(),
            definition.sampling_params_by_thinking_level.clone(),
        ),
        thinking_level_map: definition.thinking_level_map.clone(),
        // Pi sets `headers: undefined` on the composed model — `models.json` headers are REQUEST
        // config resolved separately through `resolveConfiguredModelHeaders` (:156, :501-511), so
        // they never leak into the credential-blind snapshot. cyrup's counterpart of that separate
        // resolution is [`crate::provider_compose::raw_model_headers`], applied per request in
        // `ConfiguredApiKeyAuth::resolve`; without it the declared header would be inert.
        headers: None,
        compat: merge_compat(provider_config.compat.as_ref(), definition.compat.as_ref()),
    })
}

/// Pi `mergeInputLimits` (provider-composer.ts:144-162 @v1.0.4), ported branch for branch:
///
/// ```ts
/// if (!override) return base;
/// return {
///   ...base, ...override,
///   images: override.images
///     ? { ...base?.images, ...override.images,
///         resize: override.images.resize
///           ? { ...base?.images?.resize, ...override.images.resize }
///           : base?.images?.resize }
///     : base?.images,
/// };
/// ```
///
/// A THREE-LEVEL conditional per-key merge, and NOT the flat one-level spread `promptCache` gets
/// next to it at `:196`. Copying that shape by analogy gets this wrong in two directions at once:
/// replacing the whole `images` would reset a generator-stamped `maxPerRequest`, and replacing the
/// whole `resize` would erase three of the four resize keys for an override naming one. A JS spread
/// copies only keys that are PRESENT, which is what each `Option::or` below reproduces.
fn merge_input_limits(
    model: &mut Option<cyrup_provider::ModelInputLimits>,
    ov: Option<&cyrup_provider::ModelInputLimits>,
) {
    // `if (!override) return base;` — an absent override leaves the composed model untouched.
    let Some(ov) = ov else { return };
    let base = model.take().unwrap_or_default();
    *model = Some(cyrup_provider::ModelInputLimits {
        // Level 1: `...base, ...override` over the scalar key.
        max_request_bytes: ov.max_request_bytes.or(base.max_request_bytes),
        images: match ov.images.as_ref() {
            // `: base?.images` — the override names no `images`, so the base block survives WHOLE,
            // including its `resize`.
            None => base.images,
            Some(ov_images) => {
                let base_images = base.images.unwrap_or_default();
                Some(cyrup_provider::ModelImageInputLimits {
                    // Level 2: `...base?.images, ...override.images` over the two scalars.
                    max_per_message: ov_images.max_per_message.or(base_images.max_per_message),
                    max_per_request: ov_images.max_per_request.or(base_images.max_per_request),
                    resize: match ov_images.resize.as_ref() {
                        // `: base?.images?.resize` — the base profile survives whole.
                        None => base_images.resize,
                        Some(ov_resize) => {
                            let base_resize = base_images.resize.unwrap_or_default();
                            // Level 3: `...base?.images?.resize, ...override.images.resize`.
                            Some(cyrup_provider::ModelImageResizeOptions {
                                max_width: ov_resize.max_width.or(base_resize.max_width),
                                max_height: ov_resize.max_height.or(base_resize.max_height),
                                max_bytes: ov_resize.max_bytes.or(base_resize.max_bytes),
                                jpeg_quality: ov_resize.jpeg_quality.or(base_resize.jpeg_quality),
                            })
                        }
                    },
                })
            }
        },
    });
}

/// Pi `applyModelOverride` (provider-composer.ts): patch a composed model with a `modelOverrides`
/// entry. Every field is individually optional; an absent field leaves the model unchanged.
/// Pi's `ModelInputLimitsSchema` bounds (`core/model-config.ts:154-169` @v1.0.4), enforced.
///
/// ```ts
/// const ImageResizeSchema = Type.Object({
///   maxWidth: Type.Optional(Type.Integer({ minimum: 1 })),
///   maxHeight: Type.Optional(Type.Integer({ minimum: 1 })),
///   maxBytes: Type.Optional(Type.Integer({ minimum: 1 })),
///   jpegQuality: Type.Optional(Type.Integer({ minimum: 1, maximum: 100 })),
/// });
/// const ModelInputLimitsSchema = Type.Object({
///   maxRequestBytes: Type.Optional(Type.Integer({ minimum: 1 })),
///   images: Type.Optional(Type.Object({
///     resize: Type.Optional(ImageResizeSchema),
///     maxPerMessage: Type.Optional(Type.Integer({ minimum: 1 })),
///     maxPerRequest: Type.Optional(Type.Integer({ minimum: 1 })),
///   })),
/// });
/// ```
///
/// Without this, `"maxWidth": 0` deserializes happily into `Option<u32>` and
/// `ModelImageResizeOptions::resolve`'s defensive `.max(1)` turns it into a ONE-PIXEL clamp, so
/// every image in the session silently becomes 1px wide instead of the user being told their
/// config is wrong. `"jpegQuality": 200` likewise reached `JpegEncoder::new_with_quality`.
/// Upstream has no `.max(1)` precisely because the schema makes a zero unreachable; porting the
/// clamp without the bound turned a loud config error into a silent behaviour change.
///
/// [CYRUP-DELTA] The GRANULARITY differs, deliberately and in the user's favour. Upstream's bound
/// is a TypeBox schema check that fails the whole `models.json`; this rejects the one provider
/// block, the same way the `contextWindow`/`maxTokens` checks above do (CFG-046), so one bad key
/// never costs the user the rest of their registry. The key is named either way.
fn validate_input_limits(
    provider_id: &str,
    model_id: &str,
    limits: Option<&cyrup_provider::ModelInputLimits>,
) -> Result<(), String> {
    let Some(limits) = limits else {
        return Ok(());
    };
    let at_least_one = |value: Option<u64>, key: &str| -> Result<(), String> {
        match value {
            Some(v) if v < 1 => Err(format!(
                "Provider {provider_id}, model {model_id}: invalid inputLimits.{key} ({v}); expected an integer >= 1"
            )),
            _ => Ok(()),
        }
    };
    at_least_one(limits.max_request_bytes, "maxRequestBytes")?;
    if let Some(images) = &limits.images {
        at_least_one(images.max_per_message, "images.maxPerMessage")?;
        at_least_one(images.max_per_request, "images.maxPerRequest")?;
        if let Some(resize) = &images.resize {
            at_least_one(resize.max_width.map(u64::from), "images.resize.maxWidth")?;
            at_least_one(resize.max_height.map(u64::from), "images.resize.maxHeight")?;
            at_least_one(resize.max_bytes, "images.resize.maxBytes")?;
            // The only key with an UPPER bound: `Type.Integer({ minimum: 1, maximum: 100 })`.
            if let Some(q) = resize.jpeg_quality
                && !(1..=100).contains(&q)
            {
                return Err(format!(
                    "Provider {provider_id}, model {model_id}: invalid inputLimits.images.resize.jpegQuality ({q}); expected an integer in 1..=100"
                ));
            }
        }
    }
    Ok(())
}

/// Pi `mergeSamplingParamsByThinkingLevel` (`core/provider-composer.ts:164-175` @f1b2e77f5):
/// no override keeps the composed map as is; otherwise every level the override names becomes
/// `{ ...base?.[level], ...override[level] }` and every other level is kept. CFG-104.
fn merge_sampling_params_by_thinking_level(
    base: Option<&cyrup_provider::SamplingParamsByThinkingLevel>,
    over: Option<&cyrup_provider::SamplingParamsByThinkingLevel>,
) -> Option<cyrup_provider::SamplingParamsByThinkingLevel> {
    let Some(over) = over else {
        return base.cloned();
    };
    let mut merged = base.cloned().unwrap_or_default();
    for level in cyrup_provider::SamplingParamsByThinkingLevel::LEVELS {
        if let Some(params) = over.get(level) {
            let slot = merged.slot_mut(level).get_or_insert_with(Default::default);
            for (key, value) in params {
                slot.insert(key.clone(), value.clone());
            }
        }
    }
    Some(merged)
}

fn apply_model_override(model: &mut Model, ov: &ModelOverride) {
    if let Some(name) = &ov.name {
        model.name = name.clone();
    }
    if let Some(r) = ov.reasoning {
        model.reasoning = r;
    }
    // Pi `:104-106`: `override.thinkingLevelMap ? { ...model.thinkingLevelMap,
    // ...override.thinkingLevelMap } : model.thinkingLevelMap` — a PARTIAL override patches the
    // named levels and keeps the model's other entries. Replacing the map wholesale would silently
    // change what every unmentioned thinking level sends on the wire. (The `modelFromJson` path is
    // different and correct as written: a model DEFINITION's map is used verbatim, `:141`.)
    if let Some(map) = &ov.thinking_level_map {
        let mut merged = model.thinking_level_map.clone().unwrap_or_default();
        for (level, value) in map {
            merged.insert(level.clone(), value.clone());
        }
        model.thinking_level_map = Some(merged);
    }
    if let Some(input) = &ov.input {
        model.input = input.clone();
    }
    // Pi `inputLimits: mergeInputLimits(model.inputLimits, override.inputLimits)` (`:186`).
    merge_input_limits(&mut model.input_limits, ov.input_limits.as_ref());
    // `contextWindow: override.contextWindow ?? model.contextWindow`
    // (`core/provider-composer.ts:197-198` @f1b2e77f5) — the composer itself has no positivity
    // check on the override path. A `models.json` file never reaches here with `<= 0`: the schema
    // types both fields as `PositiveTokenCountSchema` on the override too (`core/model-config.ts:30`,
    // `:59-60`), so `validate_models_config` rejects the whole file first (CFG-105). The saturation
    // below only guards a programmatically built `ModelFile` (`Model::context_window` is `u64`, so
    // a negative `i64` must not wrap).
    if let Some(cw) = ov.context_window {
        model.context_window = cw.max(0) as u64;
    }
    if let Some(mt) = ov.max_tokens {
        model.max_tokens = mt.max(0) as u64;
    }
    // Pi `:123-125` @v0.84.1: `override.samplingParams ? { ...model.samplingParams,
    // ...override.samplingParams } : model.samplingParams`. This is a per-key MERGE, not a
    // replacement — the same shape as `thinkingLevelMap` above and unlike every other field here —
    // so an override naming only `top_p` must leave a model-level `top_k` in place. CFG-039.
    // Pi `:196` @v1.0.4: `override.promptCache ? { ...model.promptCache, ...override.promptCache }
    // : model.promptCache` — a per-TIER merge, like `samplingParams` and `thinkingLevelMap` and
    // unlike `cost`/`input`. An override naming only one tier must leave the other in place, so a
    // `{ "long": 7200 }` override cannot silently disable short-tier cache warming.
    if let Some(pc) = &ov.prompt_cache {
        let mut merged = model.prompt_cache.clone().unwrap_or_default();
        if pc.short.is_some() {
            merged.short = pc.short;
        }
        if pc.long.is_some() {
            merged.long = pc.long;
        }
        model.prompt_cache = Some(merged);
    }
    let mut flat = model.flat_sampling_params().cloned();
    if let Some(params) = &ov.sampling_params {
        let mut merged = flat.unwrap_or_default();
        for (key, value) in params {
            merged.insert(key.clone(), value.clone());
        }
        flat = Some(merged);
    }
    let by_level = merge_sampling_params_by_thinking_level(
        model.sampling_params_by_thinking_level(),
        ov.sampling_params_by_thinking_level.as_ref(),
    );
    model.sampling_params = cyrup_provider::ModelSamplingParams::new(flat, by_level);
    if let Some(cost) = &ov.cost {
        if let Some(v) = cost.input {
            model.cost.input = v;
        }
        if let Some(v) = cost.output {
            model.cost.output = v;
        }
        if let Some(v) = cost.cache_read {
            model.cost.cache_read = v;
        }
        if let Some(v) = cost.cache_write {
            model.cost.cache_write = v;
        }
        if let Some(t) = &cost.tiers {
            model.cost.tiers = Some(t.clone());
        }
    }
    if let Some(compat) = &ov.compat {
        model.compat = merge_compat(model.compat.as_ref(), Some(compat));
    }
}

/// The three `compat` members Pi deep-merges instead of replacing (`mergeCompat`,
/// provider-composer.ts:87). Spelled in Pi's own wire (camelCase) form, because [`merge_compat`]
/// merges over the serialized JSON — the same names the file on disk uses.
const NESTED_COMPAT_KEYS: [&str; 3] = [
    "openRouterRouting",
    "vercelGatewayRouting",
    "chatTemplateKwargs",
];

/// Pi `mergeCompat` (provider-composer.ts:78-96): the more specific layer wins per field, EXCEPT
/// that the three object-valued members in [`NESTED_COMPAT_KEYS`] are themselves merged one level
/// deep. Either side may be absent.
///
/// Both halves matter and Pi writes them as two passes:
/// 1. `{ ...base, ...override }` — implemented over the serialized form so every present key of
///    `over`, and only the present keys, lands on `base`;
/// 2. the nested pass (`:87-95`) — for each of the three keys, `{ ...baseValue, ...overrideValue }`,
///    so declaring e.g. `"openRouterRouting": { "zdr": true }` in `models.json` KEEPS the built-in's
///    other routing fields instead of replacing the object wholesale (which would silently change
///    the wire payload).
///
/// Pi's guard is `typeof value === "object" && value !== null` on EITHER side. When only one side is
/// an object the spread reduces to that side, which pass 1 has already produced, so pass 2 below
/// only has to act when both sides are objects. The one input where this differs from Pi is a
/// non-object scalar overriding an object (Pi's spread would index the scalar's characters into the
/// merged object, producing `{0:"x",…}` garbage that cannot deserialize); cyrup keeps the override
/// scalar, which is pass 1's result.
fn merge_compat(
    base: Option<&cyrup_provider::api::compat::OpenAiCompletionsCompat>,
    over: Option<&cyrup_provider::api::compat::OpenAiCompletionsCompat>,
) -> Option<cyrup_provider::api::compat::OpenAiCompletionsCompat> {
    match (base, over) {
        (b, None) => b.cloned(),
        (None, Some(o)) => Some(o.clone()),
        (Some(b), Some(o)) => {
            let (Ok(serde_json::Value::Object(mut bm)), Ok(serde_json::Value::Object(om))) =
                (serde_json::to_value(b), serde_json::to_value(o))
            else {
                return Some(o.clone());
            };
            // Capture both sides of the nested keys BEFORE the shallow spread overwrites them.
            let nested: Vec<(&str, Option<serde_json::Value>, Option<serde_json::Value>)> =
                NESTED_COMPAT_KEYS
                    .iter()
                    .map(|k| (*k, bm.get(*k).cloned(), om.get(*k).cloned()))
                    .collect();
            for (k, v) in om {
                bm.insert(k, v);
            }
            for (key, base_value, over_value) in nested {
                let (Some(base_obj), Some(over_obj)) = (
                    base_value.as_ref().and_then(serde_json::Value::as_object),
                    over_value.as_ref().and_then(serde_json::Value::as_object),
                ) else {
                    continue;
                };
                let mut merged = base_obj.clone();
                for (k, v) in over_obj {
                    merged.insert(k.clone(), v.clone());
                }
                bm.insert(key.to_string(), serde_json::Value::Object(merged));
            }
            serde_json::from_value(serde_json::Value::Object(bm))
                .map_or_else(|_| Some(o.clone()), Some)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::fixtures::{model, oai};

    // ---- findModelDefaults rungs (CFG-092) ---------------------------------------------------

    /// A built-in with an explicit wire api and endpoint.
    fn wired(provider: &str, id: &str, api: &str, base_url: &str) -> Model {
        let mut m = model(provider, id, id);
        m.api = cyrup_provider::ApiId::from(api);
        m.base_url = base_url.to_string();
        m
    }

    fn composed(base: &[Model], json: &str) -> Vec<Model> {
        let file: ModelFile = serde_json::from_str(json).unwrap();
        let (out, errors) = file.compose(base);
        assert!(errors.is_empty(), "{errors:?}");
        out
    }

    fn by_id<'a>(models: &'a [Model], id: &str) -> &'a Model {
        models
            .iter()
            .find(|m| m.id.as_str() == id)
            .expect("composed model present")
    }

    #[test]
    fn rung_2_a_declared_model_inherits_from_the_model_with_the_same_wire_api() {
        // provider-composer.ts:201 @v0.87.1. Note the provider block declares NO baseUrl, so step 1
        // leaves the built-ins' own endpoints in place and the two are still distinguishable.
        let base = vec![
            wired("acme", "a", "openai-responses", "https://x.example/v1"),
            wired("acme", "b", "anthropic-messages", "https://y.example/v1"),
        ];
        let out = composed(
            &base,
            r#"{"providers":{"acme":{"models":[{"id":"c","api":"anthropic-messages"}]}}}"#,
        );
        assert_eq!(
            by_id(&out, "c").base_url,
            "https://y.example/v1",
            "an anthropic-messages entry must not take an openai-responses model's endpoint"
        );
    }

    #[test]
    fn rung_2_reads_the_provider_blocks_api_when_the_definition_has_none() {
        // pi passes `definition.api ?? config.api`, so a provider-level `api` selects the rung too.
        let base = vec![
            wired("acme", "a", "openai-responses", "https://x.example/v1"),
            wired("acme", "b", "anthropic-messages", "https://y.example/v1"),
        ];
        let out = composed(
            &base,
            r#"{"providers":{"acme":{"api":"anthropic-messages","models":[{"id":"c"}]}}}"#,
        );
        assert_eq!(by_id(&out, "c").base_url, "https://y.example/v1");
    }

    #[test]
    fn rung_3_falls_back_to_the_first_openai_completions_model_not_to_models_0() {
        // provider-composer.ts:202. The openai-completions model sits at a NON-first index, so this
        // cannot pass by accident through rung 4.
        let base = vec![
            wired("acme", "a", "openai-responses", "https://x.example/v1"),
            wired("acme", "b", "openai-completions", "https://z.example/v1"),
        ];
        let out = composed(&base, r#"{"providers":{"acme":{"models":[{"id":"d"}]}}}"#);
        let d = by_id(&out, "d");
        assert_eq!(d.api.as_str(), "openai-completions");
        assert_eq!(d.base_url, "https://z.example/v1");
    }

    #[test]
    fn rung_1_same_id_still_beats_the_api_match() {
        // The ordering guard: reordering the rungs to try `api` before `id` breaks exactly this.
        let base = vec![
            wired("acme", "a", "openai-responses", "https://x.example/v1"),
            wired("acme", "b", "anthropic-messages", "https://y.example/v1"),
        ];
        let out = composed(
            &base,
            r#"{"providers":{"acme":{"models":[{"id":"b","api":"openai-responses"}]}}}"#,
        );
        assert_eq!(
            by_id(&out, "b").base_url,
            "https://y.example/v1",
            "the same-id built-in wins over the same-api one"
        );
    }

    // ---- samplingParamsByThinkingLevel (CFG-104) ---------------------------------------------

    /// Port of pi's "custom models and model overrides carry sampling params"
    /// (`coding-agent/test/model-registry.test.ts:757-806` @f1b2e77f5): a definition's per-level
    /// map is copied (`provider-composer.ts:245`), an override merges per level AND per key
    /// (`mergeSamplingParamsByThinkingLevel`, `:164-175`), an override can add a level the
    /// definition lacks, an override-only built-in gets both maps, and an untouched model keeps
    /// neither. This asserts on the composed `Model` only; the wire half of the same clauses (the
    /// per-key merge into `high` and `low`, an override-only `max` on a custom definition, an
    /// override-only level on a built-in) is
    /// `tests::models_json_provider::cfg104_sampling_params_by_thinking_level_reach_the_openai_completions_request`.
    #[test]
    fn cfg104_definitions_and_overrides_carry_sampling_params_by_thinking_level() {
        let base = vec![
            oai("openrouter", "anthropic/claude-sonnet-4"),
            oai("openrouter", "anthropic/claude-opus-4.1"),
        ];
        let out = composed(
            &base,
            r#"{"providers":{"openrouter":{
                 "baseUrl":"https://my-proxy.example.com/v1","api":"openai-completions",
                 "models":[{"id":"custom/sampling-model",
                   "samplingParams":{"temperature":1,"top_p":0.95,"top_k":0},
                   "samplingParamsByThinkingLevel":{
                     "low":{"temperature":0.6,"top_p":0.95},"high":{"temperature":0.8}}}],
                 "modelOverrides":{
                   "custom/sampling-model":{"samplingParamsByThinkingLevel":{
                     "low":{"temperature":0.5,"top_k":20},"max":{"temperature":1}}},
                   "anthropic/claude-sonnet-4":{"samplingParams":{"top_p":0.9},
                     "samplingParamsByThinkingLevel":{"high":{"temperature":0.8}}}}
               }}}"#,
        );
        let to_json = |m: &Model| serde_json::to_value(m).unwrap();

        let custom = to_json(by_id(&out, "custom/sampling-model"));
        assert_eq!(
            custom["samplingParams"],
            serde_json::json!({"temperature": 1, "top_p": 0.95, "top_k": 0})
        );
        assert_eq!(
            custom["samplingParamsByThinkingLevel"],
            serde_json::json!({
                "low": {"temperature": 0.5, "top_p": 0.95, "top_k": 20},
                "high": {"temperature": 0.8},
                "max": {"temperature": 1},
            })
        );

        let sonnet = to_json(by_id(&out, "anthropic/claude-sonnet-4"));
        assert_eq!(sonnet["samplingParams"], serde_json::json!({"top_p": 0.9}));
        assert_eq!(
            sonnet["samplingParamsByThinkingLevel"],
            serde_json::json!({"high": {"temperature": 0.8}})
        );

        let opus = by_id(&out, "anthropic/claude-opus-4.1");
        assert_eq!(
            opus.sampling_params, None,
            "models without sampling config keep it unset"
        );
    }

    // ---- models.json composition (CFG-002) --------------------------------------------------

    #[test]
    fn models_json_upserts_a_custom_model_and_rewrites_the_builtin_base_url() {
        let base = vec![oai("acme", "old")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"baseUrl":"https://proxy.test/v1","models":[{"id":"new","name":"New"}]}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(errors.is_empty(), "{errors:?}");
        let old = out
            .iter()
            .find(|m| m.id.as_str() == "old")
            .expect("built-in kept");
        assert_eq!(
            old.base_url, "https://proxy.test/v1",
            "baseUrl rewrites the built-in"
        );
        let new = out
            .iter()
            .find(|m| m.id.as_str() == "new")
            .expect("custom model added");
        assert_eq!(new.name, "New");
        assert_eq!(
            new.api.as_str(),
            "openai-completions",
            "api inherits from the built-in defaults"
        );
        assert_eq!(new.base_url, "https://proxy.test/v1");
    }

    #[test]
    fn models_json_model_overrides_patch_a_builtin_last() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"modelOverrides":{"m1":{"name":"Renamed","contextWindow":42,"cost":{"input":1.5}}}}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(errors.is_empty(), "{errors:?}");
        let m = out.iter().find(|m| m.id.as_str() == "m1").unwrap();
        assert_eq!(m.name, "Renamed");
        assert_eq!(m.context_window, 42);
        assert!((m.cost.input - 1.5).abs() < f64::EPSILON);
        // Untouched fields survive the patch.
        assert_eq!(m.max_tokens, 16_384);
    }

    #[test]
    fn a_rejected_provider_block_keeps_its_builtins_and_reports() {
        // No distinguishing key at all — Pi throws (provider-composer.ts:181-184).
        let base = vec![oai("acme", "m1")];
        let file: ModelFile =
            serde_json::from_str(r#"{"providers":{"acme":{"name":"Acme"}}}"#).unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "the bad block is reported");
        assert!(errors[0].contains("must specify"), "{errors:?}");
        assert!(
            out.iter().any(|m| m.id.as_str() == "m1"),
            "the built-ins survive a rejected block"
        );
    }

    #[test]
    fn a_custom_model_with_no_resolvable_base_url_is_rejected_loudly() {
        // No built-ins to inherit from, no provider baseUrl → Pi throws (provider-composer.ts:137).
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"ghost":{"api":"openai-completions","models":[{"id":"x"}]}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&[]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("baseUrl"), "{errors:?}");
        assert!(out.is_empty());
    }

    /// CFG-046, composition half: `definition.contextWindow <= 0` — not `=== 0` —
    /// (`core/provider-composer.ts:224-229` @f1b2e77f5), rejecting ONLY that provider block.
    ///
    /// This drives `compose` directly on a deserialized `ModelFile`, NOT `load`: since CFG-105 a
    /// FILE with `contextWindow <= 0` is a whole-file schema failure (`core/model-config.ts:30`,
    /// `:148-155`; see `load.rs`'s `cfg105_*` test), so this per-provider check is reachable only
    /// for programmatically built configs — which is exactly what pi keeps it for.
    #[test]
    fn a_non_positive_context_window_rejects_only_its_own_provider_block() {
        let base = vec![model("anthropic", "claude-opus-4-8", "Claude Opus")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{
                 "mycorp":{"baseUrl":"https://x","api":"openai-completions",
                           "models":[{"id":"m","contextWindow":-1}]},
                 "anthropic":{"baseUrl":"https://ok"}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("invalid contextWindow"), "{errors:?}");
        // The good block still composed.
        assert!(out.iter().any(|m| m.base_url == "https://ok"));
    }

    /// CFG-022 — the ONE `hasConfiguredAuth` predicate, over an `auth.json` that does not exist.
    ///
    /// Pi fills `configuredProviders` by running `checkAuth` over every COMPOSED provider
    /// (model-runtime.ts:372-374), so a provider declared only in `models.json` is configured on the
    /// strength of its own `apiKey`. cyrup's launch path consulted the credential store alone, which
    /// knows only `--api-key`, an `auth.json` entry and the `env_keys` table of KNOWN provider ids —
    /// none of which a user-declared provider can match.
    #[test]
    fn models_json_api_key_configures_a_provider_with_no_stored_credential() {
        let dir = crate::test_util::temp_dir();
        let auth = crate::auth::AuthStore::at(dir.join("auth.json"));
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{
                 "mycorp":  {"baseUrl":"https://g.test/v1","apiKey":"sk-literal","models":[{"id":"m"}]},
                 "keyless": {"baseUrl":"https://k.test/v1","models":[{"id":"m"}]}
               }}"#,
        )
        .unwrap();

        assert!(provider_is_configured(
            &auth,
            &file,
            &ProviderId::from("mycorp"),
            None
        ));
        assert!(
            !provider_is_configured(&auth, &file, &ProviderId::from("keyless"), None),
            "a baseUrl-only overlay carries no credential of its own"
        );
        assert!(
            !provider_is_configured(&auth, &file, &ProviderId::from("absent"), None),
            "a provider the file does not mention is not configured"
        );
    }

    /// The env-var arm of Pi's check (provider-composer.ts:322-328): a `$VAR` template is configured
    /// exactly when every name it references is defined. A bare `api_key.is_some()` would call the
    /// unset case configured.
    #[test]
    fn a_models_json_api_key_template_needs_its_env_vars_defined() {
        let dir = crate::test_util::temp_dir();
        let auth = crate::auth::AuthStore::at(dir.join("auth.json"));
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"mycorp":{"baseUrl":"https://g.test/v1","apiKey":"${MYCORP_TOKEN}",
                 "models":[{"id":"m"}]}}}"#,
        )
        .unwrap();
        let provider = ProviderId::from("mycorp");

        let empty = std::collections::HashMap::new();
        assert!(
            !provider_is_configured(&auth, &file, &provider, Some(&empty)),
            "MYCORP_TOKEN is not defined, so the key is not configured"
        );

        let mut env = std::collections::HashMap::new();
        env.insert("MYCORP_TOKEN".to_string(), "sk-from-env".to_string());
        assert!(provider_is_configured(&auth, &file, &provider, Some(&env)));
    }

    /// A `!command` `apiKey` counts as configured on the strength of BEING a command
    /// (`isCommandConfigValue`, provider-composer.ts:321) — the command must NOT run. This predicate
    /// is a status query called inside filter loops; resolving here would execute a shell command
    /// written in `models.json`.
    #[test]
    fn a_command_api_key_is_configured_without_ever_being_executed() {
        let dir = crate::test_util::temp_dir();
        let auth = crate::auth::AuthStore::at(dir.join("auth.json"));
        let marker = dir.join("executed-marker");
        let file: ModelFile = serde_json::from_str(&format!(
            r#"{{"providers":{{"mycorp":{{"baseUrl":"https://g.test/v1",
                 "apiKey":"!touch {}","models":[{{"id":"m"}}]}}}}}}"#,
            marker.display()
        ))
        .unwrap();

        assert!(provider_is_configured(
            &auth,
            &file,
            &ProviderId::from("mycorp"),
            None
        ));
        assert!(
            !marker.exists(),
            "the status predicate executed the `apiKey` command — it must never resolve the value"
        );
    }

    /// CFG-002 — the `oauth` half of `applyModelsJson` (provider-composer.ts:167-169, :178, :188).
    ///
    /// `oauth` names an auth GATEWAY, so Pi rejects a block that sets it without the `baseUrl` that
    /// gateway lives at, counts it as a distinguishing key in the empty-block guard, and — because
    /// the gateway URL is an auth endpoint rather than a request endpoint — does NOT let it rewrite
    /// the built-in models' `baseUrl`. cyrup modelled none of that: the key was not even a field, so
    /// serde dropped it silently.
    #[test]
    fn models_json_oauth_requires_a_base_url() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile =
            serde_json::from_str(r#"{"providers":{"acme":{"oauth":"radius"}}}"#).unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0], r#"Provider acme: "baseUrl" is required when "oauth" is set."#,
            "Pi's exact text, provider-composer.ts:168"
        );
        assert!(
            out.iter().any(|m| m.id.as_str() == "m1"),
            "a rejected block still keeps the provider's built-ins"
        );
    }

    /// `oauth` alone (with its required `baseUrl`) is a COMPLETE block — Pi's empty-block guard
    /// carries a `!config.oauth` term (provider-composer.ts:178) that cyrup omitted, so cyrup
    /// rejected it with the misleading `must specify "baseUrl", "headers", …` message.
    #[test]
    fn models_json_oauth_satisfies_the_empty_block_guard_without_rewriting_base_urls() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"oauth":"radius","baseUrl":"https://gateway.acme.test/v1"}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(
            errors.is_empty(),
            "an oauth block is a distinguishing key: {errors:?}"
        );
        let m = out
            .iter()
            .find(|m| m.id.as_str() == "m1")
            .expect("built-in kept");
        assert_eq!(
            m.base_url, "https://builtin.example/v1",
            "with `oauth` set the provider baseUrl is the AUTH gateway and must not become the \
             request endpoint (provider-composer.ts:188)"
        );
    }

    /// Without `oauth`, the very same `baseUrl` DOES rewrite the built-ins — the guard above must
    /// not weaken the ordinary proxy-override path.
    #[test]
    fn models_json_base_url_still_rewrites_builtins_without_oauth() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"baseUrl":"https://gateway.acme.test/v1"}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(out[0].base_url, "https://gateway.acme.test/v1");
    }
}

// ---- CFG-085: `inputLimits` on a definition and an override -------------------------------------
//
// Pi `ModelInputLimitsSchema` is reused for BOTH (`model-config.ts:207` and `:223`), a definition's
// value is copied verbatim (`provider-composer.ts:239`) and an override goes through
// `mergeInputLimits` (`:144-162`), a three-level conditional per-key merge. Every level gets its own
// test, because the mistake this code invites is replacing a level wholesale.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod input_limits_tests {
    use cyrup_provider::{
        Modality, Model, ModelImageInputLimits, ModelImageResizeOptions, ModelInputLimits,
    };

    use super::ModelFile;
    use crate::model::fixtures::model;

    fn composed(base: &[Model], json: &str) -> Vec<Model> {
        let file: ModelFile = serde_json::from_str(json).unwrap();
        let (out, errors) = file.compose(base);
        assert!(errors.is_empty(), "{errors:?}");
        out
    }

    fn by_id<'a>(models: &'a [Model], id: &str) -> &'a Model {
        models
            .iter()
            .find(|m| m.id.as_str() == id)
            .expect("composed model present")
    }

    /// The shape a generator-stamped anthropic image-capable row carries
    /// (`generate-models.ts:994-1020`): every level of the nesting populated, so an override that
    /// clobbers a level is visible as a loss rather than as a change.
    fn stamped() -> ModelInputLimits {
        ModelInputLimits {
            max_request_bytes: Some(33_554_432),
            images: Some(ModelImageInputLimits {
                resize: Some(ModelImageResizeOptions {
                    max_width: Some(2000),
                    max_height: Some(2000),
                    max_bytes: Some(4_718_592),
                    jpeg_quality: Some(80),
                }),
                max_per_message: Some(20),
                max_per_request: Some(600),
            }),
        }
    }

    fn base_with_limits() -> Vec<Model> {
        let mut m = model("acme", "img", "img");
        m.base_url = "https://acme.example/v1".into();
        m.input = vec![Modality::Text, Modality::Image];
        m.input_limits = Some(stamped());
        vec![m]
    }

    fn overridden(json: &str) -> ModelInputLimits {
        let out = composed(&base_with_limits(), json);
        by_id(&out, "img")
            .input_limits
            .clone()
            .expect("the composed model keeps an inputLimits block")
    }

    #[test]
    fn a_definition_declares_input_limits_and_the_composer_copies_them_verbatim() {
        // CFG-085's declare half. camelCase keys at all three levels, so this also pins the serde
        // renaming — a snake_case leak would deserialize to `None` and fail here.
        let out = composed(
            &[],
            r#"{"providers":{"acme":{"baseUrl":"https://acme.example/v1","api":"openai-completions",
                "models":[{"id":"m","input":["text","image"],"inputLimits":{
                  "maxRequestBytes":32768,
                  "images":{"maxPerMessage":4,"maxPerRequest":9,
                            "resize":{"maxWidth":512,"maxHeight":384,"maxBytes":1000,"jpegQuality":55}}}}]}}}"#,
        );
        assert_eq!(
            by_id(&out, "m").input_limits,
            Some(ModelInputLimits {
                max_request_bytes: Some(32_768),
                images: Some(ModelImageInputLimits {
                    resize: Some(ModelImageResizeOptions {
                        max_width: Some(512),
                        max_height: Some(384),
                        max_bytes: Some(1000),
                        jpeg_quality: Some(55),
                    }),
                    max_per_message: Some(4),
                    max_per_request: Some(9),
                }),
            }),
            "`inputLimits: definition.inputLimits` (provider-composer.ts:239) — verbatim"
        );
    }

    #[test]
    fn a_definition_without_input_limits_still_composes_and_carries_no_profile() {
        // PROV-134's "a row without the field still parses" half. Weak on its own — it would pass
        // with the feature deleted — so it is a guard against a non-`Option` field or a missing
        // `#[serde(default)]`, not evidence of the merge.
        let out = composed(
            &[],
            r#"{"providers":{"acme":{"baseUrl":"https://acme.example/v1","api":"openai-completions",
                "models":[{"id":"m"}]}}}"#,
        );
        assert_eq!(by_id(&out, "m").input_limits, None);
    }

    #[test]
    fn the_catalog_stamp_does_not_run_over_a_user_models_json() {
        // Upstream's `applyImageInputMetadata` is GENERATOR-only; `modelFromJson` copies verbatim.
        // An image-capable declared row must therefore NOT acquire DEFAULT_IMAGE_RESIZE here.
        let out = composed(
            &[],
            r#"{"providers":{"acme":{"baseUrl":"https://acme.example/v1","api":"openai-completions",
                "models":[{"id":"m","input":["text","image"]}]}}}"#,
        );
        assert_eq!(
            by_id(&out, "m").input_limits,
            None,
            "a user models.json row must not be stamped — compose.rs is not a catalog parse site"
        );
    }

    #[test]
    fn an_override_of_one_resize_leaf_keeps_its_three_siblings_and_both_outer_levels() {
        // THE test. `{"inputLimits":{"images":{"resize":{"maxWidth":800}}}}` touches exactly one of
        // seven populated keys. A flat `model.input_limits = ov.input_limits.clone()` loses six.
        let merged = overridden(
            r#"{"providers":{"acme":{"modelOverrides":{"img":{"inputLimits":{"images":{"resize":{"maxWidth":800}}}}}}}}"#,
        );
        let images = merged.images.as_ref().unwrap();
        let resize = images.resize.as_ref().unwrap();
        assert_eq!(resize.max_width, Some(800), "the named leaf wins");
        assert_eq!(resize.max_height, Some(2000), "level 3 sibling survives");
        assert_eq!(
            resize.max_bytes,
            Some(4_718_592),
            "level 3 sibling survives"
        );
        assert_eq!(resize.jpeg_quality, Some(80), "level 3 sibling survives");
        assert_eq!(images.max_per_message, Some(20), "level 2 sibling survives");
        assert_eq!(
            images.max_per_request,
            Some(600),
            "level 2 sibling survives"
        );
        assert_eq!(
            merged.max_request_bytes,
            Some(33_554_432),
            "level 1 sibling survives"
        );
    }

    #[test]
    fn an_override_naming_no_images_keeps_the_whole_images_block() {
        // Pi's `: base?.images` arm.
        let merged = overridden(
            r#"{"providers":{"acme":{"modelOverrides":{"img":{"inputLimits":{"maxRequestBytes":99}}}}}}"#,
        );
        assert_eq!(merged.max_request_bytes, Some(99));
        assert_eq!(
            merged.images,
            stamped().images,
            "an override naming only maxRequestBytes must leave `images` entirely alone"
        );
    }

    #[test]
    fn an_override_naming_images_but_no_resize_keeps_the_whole_resize_profile() {
        // Pi's `: base?.images?.resize` arm — the level-2/level-3 boundary.
        let merged = overridden(
            r#"{"providers":{"acme":{"modelOverrides":{"img":{"inputLimits":{"images":{"maxPerMessage":3}}}}}}}"#,
        );
        let images = merged.images.as_ref().unwrap();
        assert_eq!(images.max_per_message, Some(3));
        assert_eq!(
            images.max_per_request,
            Some(600),
            "level 2 sibling survives"
        );
        assert_eq!(
            images.resize,
            stamped().images.unwrap().resize,
            "an override naming only images.maxPerMessage must leave `resize` intact"
        );
        assert_eq!(merged.max_request_bytes, Some(33_554_432));
    }

    #[test]
    fn an_override_without_input_limits_leaves_the_composed_profile_untouched() {
        // Pi's `if (!override) return base;`. Guards against the merge running unconditionally and
        // installing an empty block (which would serialize as `{}` where pi emits nothing).
        let merged =
            overridden(r#"{"providers":{"acme":{"modelOverrides":{"img":{"maxTokens":4096}}}}}"#);
        assert_eq!(merged, stamped());
    }

    #[test]
    fn an_override_without_input_limits_cannot_install_an_empty_block() {
        // The other half of `if (!override) return base;`. Dropping the guard and treating an
        // absent override as an empty one is indistinguishable on a model that HAS a profile, but
        // on one that does not it turns `undefined` into `{}` — which `skip_serializing_if` would
        // then emit as `"inputLimits":{}` where pi emits no key at all.
        let mut m = model("acme", "plain", "plain");
        m.base_url = "https://acme.example/v1".into();
        let out = composed(
            &[m],
            r#"{"providers":{"acme":{"modelOverrides":{"plain":{"maxTokens":4096}}}}}"#,
        );
        assert_eq!(by_id(&out, "plain").input_limits, None);
    }

    #[test]
    fn an_override_onto_a_model_with_no_profile_installs_the_override_alone() {
        // `{...base, ...override}` with `base` undefined: the result is the override's keys and
        // nothing invented around them.
        let mut m = model("acme", "plain", "plain");
        m.base_url = "https://acme.example/v1".into();
        let out = composed(
            &[m],
            r#"{"providers":{"acme":{"modelOverrides":{"plain":{"inputLimits":{"images":{"resize":{"jpegQuality":40}}}}}}}}"#,
        );
        assert_eq!(
            by_id(&out, "plain").input_limits,
            Some(ModelInputLimits {
                max_request_bytes: None,
                images: Some(ModelImageInputLimits {
                    resize: Some(ModelImageResizeOptions {
                        max_width: None,
                        max_height: None,
                        max_bytes: None,
                        jpeg_quality: Some(40),
                    }),
                    max_per_message: None,
                    max_per_request: None,
                }),
            }),
            "no key may be invented; the resizer fills the gaps from DEFAULT_IMAGE_RESIZE"
        );
    }

    // ---- The schema BOUNDS (CFG-085, validation half) -------------------------------------------
    //
    // Pi's `ImageResizeSchema` is `{ maxWidth: Integer({minimum:1}), maxHeight: Integer({minimum:1}),
    // maxBytes: Integer({minimum:1}), jpegQuality: Integer({minimum:1, maximum:100}) }`
    // (`model-config.ts:154-159` @v1.0.4) and `ModelInputLimitsSchema` bounds `maxRequestBytes`,
    // `images.maxPerMessage` and `images.maxPerRequest` the same way (`:160-169`). A user
    // `models.json` is validated against it, so an out-of-range key is a config ERROR naming the
    // key — not a value that reaches the resizer.
    //
    // Without these, `maxWidth: 0` deserialized fine and `resolve`'s defensive `.max(1)` turned it
    // into a ONE-PIXEL clamp on every image in the session. Upstream has no such clamp precisely
    // because the schema makes a zero unreachable.

    /// `compose` keeps the untouched built-ins for a rejected block and returns its message, so
    /// this returns the errors rather than asserting them empty.
    fn errors_of(base: &[Model], json: &str) -> Vec<String> {
        let file: ModelFile = serde_json::from_str(json).unwrap();
        let (_, errors) = file.compose(base);
        errors
    }

    #[test]
    fn a_zero_resize_dimension_on_a_definition_is_rejected_by_name() {
        let errors = errors_of(
            &[model("acme", "m1", "M1")],
            r#"{"providers":{"acme":{"models":[{"id":"custom","inputLimits":{"images":{"resize":{"maxWidth":0}}}}]}}}"#,
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("invalid inputLimits.images.resize.maxWidth"),
            "the message must name the offending key: {errors:?}"
        );
    }

    #[test]
    fn a_zero_resize_dimension_on_an_override_is_rejected_by_name() {
        let errors = errors_of(
            &[model("acme", "m1", "M1")],
            r#"{"providers":{"acme":{"modelOverrides":{"m1":{"inputLimits":{"images":{"resize":{"maxHeight":0}}}}}}}}"#,
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("invalid inputLimits.images.resize.maxHeight"),
            "an override is validated against the SAME schema as a definition: {errors:?}"
        );
    }

    /// `jpegQuality` is the one key with an UPPER bound. Both ends are rejected; `1` and `100` are
    /// the inclusive extremes and must still compose.
    #[test]
    fn jpeg_quality_is_bounded_to_1_through_100_inclusive() {
        let base = vec![model("acme", "m1", "M1")];
        let quality_json = |q: &str| {
            format!(
                "{}{q}{}",
                r#"{"providers":{"acme":{"modelOverrides":{"m1":{"inputLimits":{"images":{"resize":{"jpegQuality":"#,
                r#"}}}}}}}}"#
            )
        };
        for bad in ["0", "101", "200"] {
            let errors = errors_of(&base, &quality_json(bad));
            assert_eq!(errors.len(), 1, "jpegQuality {bad} must reject: {errors:?}");
            assert!(
                errors[0].contains("invalid inputLimits.images.resize.jpegQuality"),
                "{errors:?}"
            );
        }
        for ok in ["1", "100"] {
            let out = composed(&base, &quality_json(ok));
            assert_eq!(
                by_id(&out, "m1")
                    .input_limits
                    .as_ref()
                    .and_then(|l| l.images.as_ref())
                    .and_then(|i| i.resize.as_ref())
                    .and_then(|r| r.jpeg_quality),
                Some(ok.parse::<u8>().unwrap()),
                "the inclusive extremes must still compose"
            );
        }
    }

    /// The three inert caps are bounded too — upstream's schema does not exempt them for having no
    /// runtime reader.
    #[test]
    fn the_inert_caps_are_bounded_as_well() {
        let base = vec![model("acme", "m1", "M1")];
        for (json_key, message_key) in [
            (
                "\"maxRequestBytes\":0",
                "invalid inputLimits.maxRequestBytes",
            ),
            (
                "\"images\":{\"maxPerMessage\":0}",
                "invalid inputLimits.images.maxPerMessage",
            ),
            (
                "\"images\":{\"maxPerRequest\":0}",
                "invalid inputLimits.images.maxPerRequest",
            ),
        ] {
            let json = format!(
                "{}{json_key}{}",
                r#"{"providers":{"acme":{"modelOverrides":{"m1":{"inputLimits":{"#, r#"}}}}}}"#
            );
            let errors = errors_of(&base, &json);
            assert_eq!(errors.len(), 1, "{json_key} must reject: {errors:?}");
            assert!(errors[0].contains(message_key), "{errors:?}");
        }
    }

    /// A rejected block costs the user that block and nothing else — the built-ins for the provider
    /// stay, and so does every other provider (CFG-046's rule, applied here).
    #[test]
    fn a_rejected_input_limits_block_keeps_the_builtins_and_the_other_providers() {
        let base = vec![model("acme", "m1", "M1"), model("other", "o1", "O1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"modelOverrides":{"m1":{"name":"Renamed","inputLimits":{"images":{"resize":{"maxBytes":0}}}}}}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("invalid inputLimits.images.resize.maxBytes"));
        assert_eq!(
            by_id(&out, "m1").name,
            "M1",
            "the built-in stands untouched, including the `name` the same override tried to set"
        );
        assert!(
            out.iter().any(|m| m.id.as_str() == "o1"),
            "another provider must be unaffected"
        );
    }

    /// The mirror that keeps the above non-vacuous: a legal profile still composes.
    #[test]
    fn a_legal_profile_is_not_rejected() {
        let out = composed(
            &[model("acme", "m1", "M1")],
            r#"{"providers":{"acme":{"modelOverrides":{"m1":{"inputLimits":{"maxRequestBytes":1,"images":{"maxPerMessage":1,"maxPerRequest":1,"resize":{"maxWidth":1,"maxHeight":1,"maxBytes":1,"jpegQuality":50}}}}}}}}"#,
        );
        assert_eq!(
            by_id(&out, "m1").input_limits,
            Some(ModelInputLimits {
                max_request_bytes: Some(1),
                images: Some(ModelImageInputLimits {
                    resize: Some(ModelImageResizeOptions {
                        max_width: Some(1),
                        max_height: Some(1),
                        max_bytes: Some(1),
                        jpeg_quality: Some(50),
                    }),
                    max_per_message: Some(1),
                    max_per_request: Some(1),
                }),
            }),
            "every key at its inclusive minimum is legal"
        );
    }
}
