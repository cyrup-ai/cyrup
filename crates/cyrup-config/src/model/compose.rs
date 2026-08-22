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
/// cyrup had two disagreeing predicates: [`AuthStore::has_auth`] alone on the binary's default-launch
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
        let defaults = existing.map_or_else(|| models.first(), |i| models.get(i));
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

    // Step 3: modelOverrides are the topmost user-config layer (:433-436).
    for m in &mut models {
        if let Some(ov) = config.model_overrides.get(m.id.as_str()) {
            apply_model_override(m, ov);
        }
    }
    Ok(models)
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
        cost: definition.cost.clone().unwrap_or_default(),
        // Both are guaranteed `> 0` by the checks above, so the cast is total.
        context_window: definition.context_window.map_or(128_000, |v| v as u64),
        max_tokens: definition.max_tokens.map_or(16_384, |v| v as u64),
        // `samplingParams: definition.samplingParams` (provider-composer.ts:158 @v0.84.1) — copied
        // verbatim, with NO fallback to `providerConfig` or `defaults`: the provider block has no
        // `samplingParams` key in `ProviderConfigSchema`, and a same-id built-in's defaults are
        // deliberately not inherited here. CFG-039.
        sampling_params: definition.sampling_params.clone(),
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

/// Pi `applyModelOverride` (provider-composer.ts): patch a composed model with a `modelOverrides`
/// entry. Every field is individually optional; an absent field leaves the model unchanged.
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
    // `contextWindow: override.contextWindow ?? model.contextWindow` (provider-composer.ts:118-119
    // @v0.83.0) — the override path has NO positivity check, unlike `modelFromJson`'s.
    //
    // [CYRUP-DELTA] pi stores a negative override verbatim (JS `number`); `Model::context_window`
    // is `u64`, so a negative value saturates to 0 here rather than wrapping. Upstream's own
    // behaviour on a negative override is an unguarded hole (a negative window reaches the request
    // builder), and reproducing the wrap would be strictly worse than reproducing the intent.
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
    if let Some(params) = &ov.sampling_params {
        let mut merged = model.sampling_params.clone().unwrap_or_default();
        for (key, value) in params {
            merged.insert(key.clone(), value.clone());
        }
        model.sampling_params = Some(merged);
    }
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
    /// (provider-composer.ts:138-143 @v0.83.0), rejecting ONLY that provider block. A custom model
    /// with an empty inherited `baseUrl` must still hit pi's
    /// `"baseUrl" is required when defining custom models.`
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
