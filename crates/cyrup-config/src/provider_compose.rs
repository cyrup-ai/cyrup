//! `models.json` → **provider** composition (arch-07 §3.6; Pi `provider-composer.ts` +
//! `ModelRuntime`).
//!
//! [`crate::model::ModelFile::compose`] answers "what does the catalog look like?". This module
//! answers the question the binary actually asks: "which [`cyrup_provider::Provider`] owns this model, and can it
//! stream?".
//!
//! Pi has exactly ONE model registry and it is the **composed** one.
//! `ModelRuntime.rebuildProviders` (model-runtime.ts:225-231) clears the collection and re-registers
//! every id in `providerIds()` — `builtins ∪ nativeExtensionProviders ∪ config.getProviderIds() ∪
//! extensionProviders` (:193-199) — through `recomposeProvider` (:200-223), which calls
//! `this.models.setProvider(composeModelProvider(providerId, base, this.config, extension))` (:215),
//! **replacing** the built-in. Three consequences cyrup must match:
//!
//! 1. `composeModelProvider` synthesizes a `Provider` even when `base` is `undefined`
//!    (provider-composer.ts:411-437), so a provider that exists ONLY in `models.json` is a real,
//!    streamable provider. Its `streamWith` falls through to `getApiProvider(model.api)`
//!    (`:583-603` @f1b2e77f5).
//! 2. Every consumer reads that one collection: `--list-models`
//!    (`modelRegistry.getAvailable()`, list-models.ts:35), `find`, `setModel`, `stream`. A caller
//!    that reaches for the raw built-in registry is reading a registry Pi does not have.
//! 3. A provider block Pi would `throw` on is recorded in `compositionErrors` and the **built-in is
//!    kept untouched** (`if (base) this.models.setProvider(base)`, model-runtime.ts:218-221) — one
//!    bad block never costs the user the rest of the registry.
//!
//! cyrup's analog of `getApiProvider(model.api)` is [`WireProvider`], which holds a catalog + a
//! [`ProviderAuth`] and dispatches each request through the shared [`ApiRegistry`] keyed on
//! `model.api`. Every built-in text provider in `cyrup-provider`'s `providers/all.rs` **is** a
//! `WireProvider` (they differ only in id/name/baseUrl/catalog/auth), so the composed provider is
//! built the same way, carrying the base's name and baseUrl unless the block sets its own
//! (provider-composer.ts:607-608, CFG-114). It declares [`ApiDispatch::Global`]: its registry is
//! `streamWith`'s process-wide one, and a model on an api its base's chat catalog lists is the base's
//! to answer (`supportsBaseApi`, `:582`, `:593-596`), which [`WireProvider`] resolves per request
//! through [`WireProvider::with_operations_from`]. So a miss reports pi's text on both branches
//! (PROV-157). Where the base is one of pi's single-api built-ins, pi's base streams any model
//! through its one api (`apiFor = single ?? byApi[model.api]`, `models.ts:1070`) where cyrup
//! dispatches on `model.api`: that is PROV-160, not a composition question.
//!
//! The assembly itself — auth layers, name/`baseUrl` fallbacks, `headers`, operations and
//! `supportsBaseApi` routing — is the ONE composer, `cyrup_provider::compose::compose_over_base`,
//! which a guest `registerProvider` over a built-in id goes through as well (PROV-162). This module
//! supplies the `models.json` layer: its catalog step (`apply_models_json`) and its config-value
//! language for the key and header templates.

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_provider::compose::{
    AuthLayer, ConfigValues, ProviderLayer, RawHeaders, compose_over_base,
};
use cyrup_provider::{
    AnyModel, ApiRegistry, AuthContext, ClassifierModel, CreateModelsOptions, CredentialStore,
    ImageModel, InMemoryCredentialStore, Model, Models, all_providers_with_overlay,
    builtin_registry, create_models,
};

/// The composed provider's API-key strategy — pi `composeApiKeyAuth`. It lives with the ONE
/// composer in `cyrup-provider` (PROV-162); re-exported here, where it was first ported.
pub use cyrup_provider::compose::ConfiguredApiKeyAuth;

use crate::config_value::{
    config_value_env_var_names, resolve_config_value_or_throw_async, resolve_headers_or_throw,
};
use crate::model::{ModelFile, ProviderConfig, apply_models_json, model_base_url_override};

/// pi's config-value language (`resolve-config-value.ts`): `$VAR` / `${VAR}` templates and
/// `!command` values — what a `models.json` block's `apiKey` and `headers` are written in. The
/// shared composer resolves a layer's values through this.
struct ConfigValueLanguage;

#[async_trait::async_trait]
impl ConfigValues for ConfigValueLanguage {
    fn env_var_names(&self, raw: &str) -> Vec<String> {
        config_value_env_var_names(raw)
    }

    async fn resolve(
        &self,
        raw: &str,
        description: &str,
        env: Option<&HashMap<String, String>>,
    ) -> Result<String, String> {
        // CFG-028: `!command` resolution runs on the blocking pool (same 10 s ceiling).
        resolve_config_value_or_throw_async(raw, description, env).await
    }

    fn resolve_headers(
        &self,
        headers: Option<&RawHeaders>,
        description: &str,
        env: Option<&HashMap<String, String>>,
    ) -> Result<Option<RawHeaders>, String> {
        resolve_headers_or_throw(headers, description, env)
    }
}

/// Pi `rawModelHeaders` (provider-composer.ts:384-396): the PER-MODEL request headers a
/// `models.json` block declares, keyed by model id — `{ ...modelOverrides[id].headers,
/// ...models[].headers }`, so a `models[]` definition's header wins over the same-named
/// `modelOverrides` one.
///
/// These are deliberately NOT on the composed [`Model`] (Pi sets `headers: undefined` there,
/// `modelFromJson` :156, so the credential-blind catalog snapshot stays credential-blind). Pi
/// resolves them per request instead, in `ModelRuntime.getAuth` (model-runtime.ts:383-397), which is
/// the [`ConfiguredApiKeyAuth::resolve`] seam here. Without that second half the declaration parses
/// and then does nothing — the same "declared but inert" defect CFG-002 was filed for.
///
/// The `extension?.models` third layer of Pi's spread has no counterpart here: cyrup composes from
/// `models.json` alone (no `ProviderConfigInput` extension layer reaches this function).
fn raw_model_headers(config: &ProviderConfig) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (model_id, over) in &config.model_overrides {
        if let Some(headers) = &over.headers {
            out.entry(model_id.clone())
                .or_default()
                .extend(headers.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
    for definition in &config.models {
        if let Some(headers) = &definition.headers {
            out.entry(definition.id.clone())
                .or_default()
                .extend(headers.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
    // Pi returns `undefined` for an empty header set (:395), so an entry that declared only an empty
    // object contributes nothing.
    out.retain(|_, headers| !headers.is_empty());
    out
}

/// A `models.json` block's auth inputs as the shared composer's [`AuthLayer`] (pi
/// `configuredApiKey` / `configuredHeaders` / `authHeader` / `rawModelHeaders`,
/// provider-composer.ts:388-401, :429, :508-527), resolving through pi's config-value language.
fn auth_layer(config: &ProviderConfig) -> AuthLayer {
    let mut layer = AuthLayer::new(Arc::new(ConfigValueLanguage));
    layer.api_key = config.api_key.clone();
    layer.headers = config
        .headers
        .as_ref()
        .map(|h| h.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    layer.model_headers = raw_model_headers(config);
    layer.auth_header = config.auth_header.unwrap_or(false);
    layer
}

/// Pi `composeApiKeyAuth` + `composeOAuthAuth` + the `!apiKey && !oauth` guard for a `models.json`
/// block — the shared composer's [`cyrup_provider::compose::compose_provider_auth`] over this
/// block's [`auth_layer`].
#[cfg(test)]
fn compose_provider_auth(
    provider_id: &str,
    base: Option<&cyrup_provider::ProviderAuth>,
    config: &ProviderConfig,
) -> Result<cyrup_provider::ProviderAuth, String> {
    cyrup_provider::compose::compose_provider_auth(provider_id, base, &auth_layer(config))
}

impl ModelFile {
    /// Pi `ModelRuntime.rebuildProviders`/`recomposeProvider` (model-runtime.ts:200-231): replace
    /// every provider this `models.json` mentions with its composed form, and register a brand-new
    /// [`WireProvider`] for each declared provider that has no built-in base.
    ///
    /// Returns one message per **rejected** block. A rejected block leaves its built-in registered
    /// untouched (Pi :218-221); a rejected *base-less* block registers nothing, so the id stays
    /// unknown and the caller's "not a built-in provider" error still names it. Providers absent
    /// from the file are left strictly alone — Pi's "no overlays: use the builtin untouched so its
    /// auth/login/stream behavior is exact" (:210-214).
    ///
    /// `auth_context` overrides the composed provider's ambient env source; pass `None` for the real
    /// process environment (what every built-in uses).
    pub fn compose_providers(
        &self,
        models: &mut Models,
        store: Arc<dyn CredentialStore>,
        registry: Arc<ApiRegistry>,
        auth_context: Option<Arc<dyn AuthContext>>,
    ) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        for (provider_id, config) in &self.providers {
            let base = models.get_provider(provider_id);
            // Pi `getAllProviderModels(base)` (`provider.getAllModels?.() ?? provider.getModels()`,
            // provider-composer.ts:538, :558 @v1.0.4): the base's catalog of EVERY type. The chat rows
            // go through the `models.json` block; the image and classifier rows are not what a
            // block's `models`/`modelOverrides` are about (chat-only upstream) but they are still
            // the provider's models, and the block's `baseUrl` still reaches them (:326).
            let mut base_models: Vec<Model> = Vec::new();
            let mut base_images: Vec<ImageModel> = Vec::new();
            let mut base_classifiers: Vec<ClassifierModel> = Vec::new();
            for row in base.iter().flat_map(|p| p.get_all_models()) {
                match row {
                    AnyModel::Chat(model) => base_models.push(model),
                    AnyModel::Image(model) => base_images.push(model),
                    AnyModel::Classifier(model) => base_classifiers.push(model),
                }
            }
            let composed = match apply_models_json(provider_id, &base_models, config) {
                Ok(models) => models,
                Err(message) => {
                    errors.push(message);
                    continue;
                }
            };
            if let Some(base_url) = model_base_url_override(config) {
                for model in &mut base_images {
                    model.base_url = base_url.to_string();
                }
                for model in &mut base_classifiers {
                    model.base_url = base_url.to_string();
                }
            }
            // Pi `name: extension?.name ?? config?.name ?? base?.name ?? extension?.oauth?.name ??
            // providerId` (provider-composer.ts:607 @f1b2e77f5); the base's name and `baseUrl`
            // (`:608`) are the shared composer's fallback. The provider's URL is the block's
            // `baseUrl` even under `oauth` — deliberately NOT `model_base_url_override`, which drops
            // it for the ROWS (:326, the block's URL is then the auth gateway). CFG-114.
            //
            // An `oauth` block's base is pi's radius provider, named `config.name ?? providerId`
            // (`model-runtime.ts:263-276`), not the built-in cyrup composes over (`[CYRUP-DELTA]`
            // in `model/schema.rs`), so its name never falls back to the built-in's.
            let name = match (&config.name, &config.oauth) {
                (Some(name), _) => Some(name.clone()),
                (None, Some(_)) => Some(provider_id.clone()),
                (None, None) => None,
            };
            let layer = ProviderLayer {
                name,
                base_url: config.base_url.clone(),
                models: composed,
                image_models: base_images,
                classifier_models: base_classifiers,
                auth: auth_layer(config),
            };
            // The ONE composer (PROV-162): the base's auth layers, `headers`, operations and
            // `supportsBaseApi` routing, `ApiDispatch::Global` for the rest (PROV-157). A block it
            // rejects (`no authentication method configured`) leaves the built-in registered.
            match compose_over_base(
                provider_id,
                base,
                layer,
                store.clone(),
                registry.clone(),
                auth_context.clone(),
            ) {
                // Pi `models.setProvider(...)`, model-runtime.ts:215.
                Ok(provider) => models.set_provider(Arc::new(provider)),
                Err(message) => errors.push(message),
            }
        }
        errors
    }
}

/// Pi's ONE registry: every built-in provider, composed with `<agent_dir>/models.json`
/// (`ModelRuntime.create` → `rebuildProviders`, model-runtime.ts:103-112).
///
/// This is what every model *resolution*, *enumeration* and *stream* path must read. Returns the
/// collection plus one message per rejected provider block (Pi's `compositionErrors` map) — a
/// malformed block is loud but never fatal, and never a panic.
///
/// The credential store in `options` is threaded to the providers themselves (not just to the
/// collection) so a runtime `--api-key` reaches the provider the binary hands to the session.
pub fn compose_provider_registry(
    file: &ModelFile,
    options: CreateModelsOptions,
) -> (Models, Vec<String>) {
    let store: Arc<dyn CredentialStore> = options
        .credentials
        .clone()
        .unwrap_or_else(|| Arc::new(InMemoryCredentialStore::new()));
    let auth_context = options.auth_context.clone();
    // The remote model-catalog overlay (DRIFT-007) is applied to the BUILT-INS, below
    // `models.json` — a user's explicit config still wins over anything pi.dev serves, and the
    // embedded catalogs still floor the result (the overlay only adds/replaces by model id).
    let overlay = options.catalog_overlay.clone();
    let registry = Arc::new(builtin_registry());
    let mut models = create_models(options);
    for provider in all_providers_with_overlay(store.clone(), registry.clone(), overlay.as_deref())
    {
        models.set_provider(provider);
    }
    let errors = file.compose_providers(&mut models, store, registry, auth_context);
    (models, errors)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use cyrup_provider::auth::oauth::{
        AuthInteraction, AuthPrompt, AuthPromptKind, ScriptedInteraction,
    };
    use cyrup_provider::{ApiKeyAuth, AuthError, AuthResult, Credential, ProviderAuth};

    /// A stand-in for a built-in strategy with a real, MULTI-prompt `login` — the shape Cloudflare
    /// (key + account id), Vertex (key + project + location) and Bedrock actually have. Its
    /// credential carries an `env` overlay so a test can prove the inherited flow ran and the
    /// generic one-secret flow did not.
    struct MultiPromptInherited;

    #[async_trait::async_trait]
    impl ApiKeyAuth for MultiPromptInherited {
        fn name(&self) -> &str {
            "Cloudflare API key"
        }
        fn supports_login(&self) -> bool {
            true
        }
        async fn login(
            &self,
            interaction: &dyn AuthInteraction,
        ) -> Result<Credential, cyrup_provider::auth::oauth::OAuthError> {
            let key = interaction
                .prompt(AuthPrompt::secret("Enter Cloudflare API key"))
                .await?;
            let account = interaction
                .prompt(AuthPrompt::text("Enter Cloudflare account ID"))
                .await?;
            Ok(Credential::ApiKey {
                key: Some(key),
                env: Some(
                    [("CLOUDFLARE_ACCOUNT_ID".to_string(), account)]
                        .into_iter()
                        .collect(),
                ),
            })
        }
        async fn resolve(
            &self,
            _model: &Model,
            _ctx: &dyn AuthContext,
            _cred: Option<&Credential>,
        ) -> Result<Option<AuthResult>, AuthError> {
            Ok(None)
        }
    }

    /// Upstream's *"Absent = ambient-only"* strategy (`ai/src/auth/types.ts:166`): a `name`, a
    /// `resolve`, and no `login` member at all.
    struct AmbientOnlyInherited;

    #[async_trait::async_trait]
    impl ApiKeyAuth for AmbientOnlyInherited {
        fn name(&self) -> &str {
            "Ambient token"
        }
        async fn resolve(
            &self,
            _model: &Model,
            _ctx: &dyn AuthContext,
            _cred: Option<&Credential>,
        ) -> Result<Option<AuthResult>, AuthError> {
            Ok(None)
        }
    }

    fn compose(base: Option<ProviderAuth>, config: ProviderConfig) -> Arc<dyn ApiKeyAuth> {
        compose_provider_auth("acme", base.as_ref(), &config)
            .expect("auth composes")
            .api_key
            .expect("composed provider has an api-key strategy")
    }

    fn key_of(cred: &Credential) -> Option<&str> {
        match cred {
            Credential::ApiKey { key, .. } => key.as_deref(),
            Credential::Oauth { .. } => None,
        }
    }

    /// Pi `name: inherited?.name ?? "API key"` (provider-composer.ts:315) — the INHERITED half.
    /// Before the fix `name()` was the hardcoded literal, so `/login`'s picker labelled every
    /// composed provider "API key".
    #[test]
    fn name_delegates_to_the_inherited_strategy() {
        let base = ProviderAuth::with_api_key(Arc::new(MultiPromptInherited));
        let composed = compose(Some(base), ProviderConfig::default());
        assert_eq!(composed.name(), "Cloudflare API key");
    }

    /// The `?? "API key"` half: a base-less `models.json` provider keeps upstream's literal.
    #[test]
    fn name_falls_back_to_api_key_without_a_base() {
        let config = ProviderConfig {
            api_key: Some("sk-configured".to_string()),
            ..Default::default()
        };
        let composed = compose(None, config);
        assert_eq!(composed.name(), "API key");
    }

    /// Pi always populates `login` (:316-321), so `method.login !== undefined`
    /// (`interactive-mode.ts:4942`) is true for EVERY composed provider — with a base, without a
    /// base, and with an ambient-only base. Before the fix the trait default returned `false` in
    /// all three cases and `login.rs`'s `.filter(|s| s.supports_login())` rejected the provider.
    #[test]
    fn supports_login_is_always_true() {
        assert!(compose(None, ProviderConfig::default()).supports_login());
        assert!(
            compose(
                Some(ProviderAuth::with_api_key(Arc::new(MultiPromptInherited))),
                ProviderConfig::default()
            )
            .supports_login()
        );
        assert!(
            compose(
                Some(ProviderAuth::with_api_key(Arc::new(AmbientOnlyInherited))),
                ProviderConfig::default()
            )
            .supports_login()
        );
    }

    /// `inherited?.login ?? …` (:316-318) resolves to the INHERITED flow when it has one: both
    /// prompts run and the multi-value credential survives whole. The generic flow would have
    /// asked exactly one question and dropped the account id.
    #[tokio::test]
    async fn login_delegates_to_the_inherited_flow() {
        let composed = compose(
            Some(ProviderAuth::with_api_key(Arc::new(MultiPromptInherited))),
            ProviderConfig::default(),
        );
        let interaction =
            ScriptedInteraction::new(vec![Ok("cf-key".to_string()), Ok("acct-123".to_string())]);
        let cred = composed.login(&interaction).await.expect("login succeeds");

        assert_eq!(key_of(&cred), Some("cf-key"));
        assert_eq!(
            cred.env().and_then(|e| e.get("CLOUDFLARE_ACCOUNT_ID")),
            Some(&"acct-123".to_string()),
            "the inherited flow's second value must survive; the generic one-secret flow drops it"
        );
        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 2, "inherited flow is multi-prompt");
        assert_eq!(prompts[0].message, "Enter Cloudflare API key");
        assert_eq!(prompts[1].message, "Enter Cloudflare account ID");
    }

    /// The `?? (async (interaction) => …)` half (:318-321): no base at all, so the generic
    /// one-secret flow runs with upstream's literal `"Enter API key"` message — NOT `env_key`'s
    /// `format!("Enter {name}")`.
    #[tokio::test]
    async fn login_runs_the_generic_secret_prompt_without_a_base() {
        let composed = compose(
            None,
            ProviderConfig {
                api_key: Some("sk-configured".to_string()),
                ..Default::default()
            },
        );
        let interaction = ScriptedInteraction::new(vec![Ok("sk-typed".to_string())]);
        let cred = composed.login(&interaction).await.expect("login succeeds");

        assert_eq!(key_of(&cred), Some("sk-typed"));
        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].kind, Some(AuthPromptKind::Secret));
        assert_eq!(prompts[0].message, "Enter API key");
    }

    /// The coalesce is on the `login` MEMBER, not on the strategy: an inherited strategy that is
    /// ambient-only (`login` absent) still yields the generic flow, exactly as `undefined ?? fn`
    /// does upstream. Delegating blindly here would surface `LoginUnsupported` instead.
    #[tokio::test]
    async fn login_falls_back_when_the_inherited_strategy_has_no_login() {
        let composed = compose(
            Some(ProviderAuth::with_api_key(Arc::new(AmbientOnlyInherited))),
            ProviderConfig::default(),
        );
        let interaction = ScriptedInteraction::new(vec![Ok("sk-typed".to_string())]);
        let cred = composed.login(&interaction).await.expect("login succeeds");

        assert_eq!(key_of(&cred), Some("sk-typed"));
        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].message, "Enter API key");
        // `name` still delegates even though `login` does not.
        assert_eq!(composed.name(), "Ambient token");
    }
}
