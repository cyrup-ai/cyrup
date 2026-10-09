//! The `models.json` document types — provider blocks, model definitions, overrides — and the
//! request-auth resolution a provider block carries.

/// The OAuth auth mode a `models.json` provider block may declare (Pi
/// `ProviderConfigSchema.oauth`, model-config.ts:194).
///
/// Pi types this as `Type.Literal("radius")` — `radius` is the ONLY accepted spelling, and any
/// other value is a whole-file schema rejection (model-config.ts:265-272), not a silently ignored
/// key. Modelling it as a single-variant enum reproduces that: serde fails the load, and
/// [`load_models_file_reporting`](crate::model::load_models_file_reporting) turns the failure
/// into Pi's empty-snapshot-plus-one-message contract.
///
/// **[CYRUP-DELTA]** cyrup does not port the `radius` provider itself (`configureRadiusProviders`,
/// model-runtime.ts:175-191, synthesizes a built-in from the block's `baseUrl`), so a `radius`
/// block currently composes against no base models and contributes none. The composition-layer
/// semantics below are ported regardless, so the block is ACCEPTED rather than rejected with a
/// misleading "must specify baseUrl, headers, compat, modelOverrides, or models".
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelsJsonOauth {
    Radius,
}

/// A `models.json` provider request config (Pi `ProviderConfigSchema`, model-registry.ts:204-214):
/// the request-auth-relevant fields. `apiKey`/`headers` carry unresolved config-value templates;
/// resolve them with [`ProviderConfig::resolve_request_auth`].
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub auth_header: Option<bool>,
    /// OAuth auth mode (Pi `ProviderConfigSchema.oauth`, model-config.ts:194). Setting it makes
    /// `baseUrl` mandatory (provider-composer.ts:167-169) because that URL is the auth GATEWAY, and
    /// for the same reason suppresses the provider-level rewrite of the built-ins' request
    /// `baseUrl` (:188). It also counts as a distinguishing key in the empty-block guard (:178).
    #[serde(default)]
    pub oauth: Option<ModelsJsonOauth>,
    /// Provider-level compatibility overrides applied to every model of this provider (Pi
    /// `ProviderConfigSchema.compat`, model-config.ts:196).
    #[serde(default)]
    pub compat: Option<cyrup_provider::api::compat::OpenAiCompletionsCompat>,
    /// Inline model definitions (Pi `ProviderConfigSchema.models`, model-config.ts:197).
    #[serde(default)]
    pub models: Vec<ModelDefinition>,
    /// Per-model patches applied LAST, over built-ins and custom models alike (Pi
    /// `ProviderConfigSchema.modelOverrides`, model-config.ts:198; applied at
    /// provider-composer.ts:433-436).
    #[serde(default)]
    pub model_overrides: std::collections::BTreeMap<String, ModelOverride>,
}

/// Resolved request auth for a provider (Pi `ResolvedRequestAuth` ok-branch,
/// model-registry.ts:249-259).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedRequestAuth {
    pub api_key: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub auth_header: Option<bool>,
}

impl ProviderConfig {
    /// Resolve `apiKey` + `headers` through the config-value language (Pi
    /// `getApiKeyAndHeaders`/`resolveHeadersOrThrow`, model-registry.ts:659-736). `env` is an
    /// optional provider-scoped override map. Returns an error string on an unresolvable template.
    pub fn resolve_request_auth(
        &self,
        env: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<ResolvedRequestAuth, String> {
        let api_key = match &self.api_key {
            Some(raw) => Some(crate::config_value::resolve_config_value_or_throw(
                raw, "API key", env,
            )?),
            None => None,
        };
        let headers = match &self.headers {
            Some(map) => {
                let owned: std::collections::HashMap<String, String> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                crate::config_value::resolve_headers_or_throw(Some(&owned), "provider", env)?
            }
            None => None,
        };
        Ok(ResolvedRequestAuth {
            api_key,
            headers,
            auth_header: self.auth_header,
        })
    }
}

/// A single `models` entry inside a `models.json` provider block (Pi `ModelDefinitionSchema`,
/// model-config.ts:152-166). Every field but `id` is optional and inherits from the provider block
/// or from the same-id built-in model (Pi `modelFromJson`, provider-composer.ts:124-159).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefinition {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_level_map: Option<cyrup_provider::model::ThinkingLevelMap>,
    #[serde(default)]
    pub input: Option<Vec<cyrup_provider::Modality>>,
    /// Per-model image/request input limits — Pi
    /// `inputLimits: Type.Optional(ModelInputLimitsSchema)` (`model-config.ts:207` for a
    /// definition, `:223` for an override, @v1.0.4). Declared immediately after `input` and before
    /// `cost` in both schemas, the same slot upstream gives it on `BaseModel` (`types.ts:1105`) —
    /// which is why it sits here.
    ///
    /// ONE shared type serves both a definition and an override: upstream reuses the SAME
    /// `ModelInputLimitsSchema` object at both sites, unlike `cost`, which does get a separate
    /// partial override shape. Copied verbatim onto the composed model by `model_from_json`
    /// (`provider-composer.ts:239`) with no provider-block or same-id-builtin fallback, exactly as
    /// `prompt_cache` is.
    ///
    /// Only `images.resize` has a consumer. `max_request_bytes`, `images.max_per_message` and
    /// `images.max_per_request` are modelled and round-tripped but enforced nowhere — knowingly
    /// inert, matching upstream (`packages/coding-agent/docs/models.md:89`: "Pi does not yet
    /// rewrite or reject history based on them"). CFG-085.
    #[serde(default)]
    pub input_limits: Option<cyrup_provider::ModelInputLimits>,
    #[serde(default)]
    pub cost: Option<cyrup_provider::ModelCost>,
    /// `Type.Optional(Type.Number())` (model-config.ts:163 @v0.83.0) — SIGNED, because pi accepts a
    /// negative value at the schema layer and rejects it in `modelFromJson` with
    /// `invalid contextWindow`, per PROVIDER, keeping the rest of the file. A `u64` would turn the
    /// same document into a whole-file parse failure (CFG-046).
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// `Type.Optional(Type.Record(Type.String(), Type.Unknown()))` (model-config.ts:167 @v0.84.1) —
    /// arbitrary OpenAI-compatible sampling keys (`top_p`, `top_k`, `min_p`,
    /// `repetition_penalty`, …) that become the composed model's defaults. `modelFromJson` copies
    /// it straight across (`provider-composer.ts:158`); it is NOT inherited from the provider block
    /// or from the same-id built-in, because pi's `ModelDefinitionSchema` has no provider-level
    /// twin. CFG-039.
    #[serde(default)]
    pub sampling_params: Option<serde_json::Map<String, serde_json::Value>>,
    /// Prompt-cache lifetimes, in SECONDS, per retention tier — Pi
    /// `promptCache: Type.Optional(ModelPromptCacheSchema)` (`model-config.ts:209` for a
    /// definition, `:233` for an override). Copied verbatim onto the composed model
    /// (`provider-composer.ts:241`); it is what `cyrup_provider::prompt_cache_ttl_ms` schedules
    /// cache warming against, so a row that sets it opts a non-Anthropic provider in.
    #[serde(default)]
    pub prompt_cache: Option<cyrup_provider::ModelPromptCache>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<cyrup_provider::api::compat::OpenAiCompletionsCompat>,
}

/// A `modelOverrides` entry: a partial patch applied to an already-composed model (Pi
/// `ModelOverrideSchema`, model-config.ts:168-186; applied last, provider-composer.ts:433-436).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOverride {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_level_map: Option<cyrup_provider::model::ThinkingLevelMap>,
    #[serde(default)]
    pub input: Option<Vec<cyrup_provider::Modality>>,
    /// Pi `:223`. The SAME `ModelInputLimitsSchema` as a definition's (see
    /// [`ModelDefinition::input_limits`]), but applied through `mergeInputLimits`
    /// (`provider-composer.ts:144-162`), which is a THREE-LEVEL per-key merge and strictly deeper
    /// than `prompt_cache`'s flat per-tier merge below:
    ///
    /// - `maxRequestBytes` merges per key;
    /// - `images` ABSENT keeps the composed model's whole `images` block;
    /// - `images` PRESENT merges `maxPerMessage`/`maxPerRequest` per key, and keeps the model's
    ///   whole `resize` unless `resize` is also present, in which case all four resize keys merge
    ///   per key.
    ///
    /// So an override naming only `images.resize.maxWidth` must leave `maxHeight`, `maxBytes`,
    /// `jpegQuality`, `images.maxPerRequest` and `maxRequestBytes` all in place. Replacing a level
    /// wholesale would silently reset a generator-stamped `maxPerRequest` or zero three resize
    /// keys. CFG-085.
    #[serde(default)]
    pub input_limits: Option<cyrup_provider::ModelInputLimits>,
    #[serde(default)]
    pub cost: Option<ModelCostOverride>,
    /// Signed for the same reason as [`ModelDefinition::context_window`] (model-config.ts:184).
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// `Type.Optional(Type.Record(Type.String(), Type.Unknown()))` (model-config.ts:188 @v0.84.1).
    /// Unlike every other override field this one MERGES per key rather than replacing:
    /// `override.samplingParams ? { ...model.samplingParams, ...override.samplingParams } :
    /// model.samplingParams` (`provider-composer.ts:123-125`). CFG-039.
    #[serde(default)]
    pub sampling_params: Option<serde_json::Map<String, serde_json::Value>>,
    /// Pi `:233`. Unlike most override fields this MERGES per tier:
    /// `override.promptCache ? { ...model.promptCache, ...override.promptCache } :
    /// model.promptCache` (`provider-composer.ts:196`), the same shape as `samplingParams` above —
    /// so an override naming only `long` leaves a composed `short` in place.
    #[serde(default)]
    pub prompt_cache: Option<cyrup_provider::ModelPromptCache>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<cyrup_provider::api::compat::OpenAiCompletionsCompat>,
}

/// The partial `cost` shape a `modelOverrides` entry may carry (model-config.ts:174-182): every rate
/// is individually optional and patches the composed model's cost.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostOverride {
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub tiers: Option<Vec<cyrup_provider::ModelCostTier>>,
}

/// A parsed `models.json` in Pi's `{ providers: { <name>: ProviderConfig } }` shape
/// (model-registry.ts:216-218 / model-config.ts:188-190).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ModelFile {
    #[serde(default)]
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use crate::model::load_models_file;

    #[test]
    fn models_file_provider_config_resolves_auth() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{ "providers": { "acme": { "baseUrl": "https://api.acme.test", "apiKey": "literal-key", "authHeader": true } } }"#,
        )
        .unwrap();
        let file = load_models_file(&path).unwrap();
        let cfg = file.providers.get("acme").unwrap();
        assert_eq!(cfg.base_url.as_deref(), Some("https://api.acme.test"));
        let resolved = cfg.resolve_request_auth(None).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("literal-key"));
        assert_eq!(resolved.auth_header, Some(true));
        // missing file → empty
        assert!(
            load_models_file(&dir.join("nope.json"))
                .unwrap()
                .providers
                .is_empty()
        );
    }
}
