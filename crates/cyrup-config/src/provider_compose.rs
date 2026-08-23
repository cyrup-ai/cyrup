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
//!    streamable provider. Its `streamWith` falls through to `getApiProvider(model.api)` (:459-465).
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
//! `WireProvider` (they differ only in id/name/catalog/auth), so building the composed provider the
//! same way reproduces the built-in's behavior exactly when there is nothing to compose, and adds
//! the overlay when there is.

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_core::ProviderId;
use cyrup_provider::auth::oauth::{AuthInteraction, AuthPrompt, OAuthError};
use cyrup_provider::wire::WireProvider;
use cyrup_provider::{
    ApiKeyAuth, ApiRegistry, AuthContext, AuthError, AuthResult, CreateModelsOptions, Credential,
    CredentialStore, InMemoryCredentialStore, Model, ModelAuth, Models, ProviderAuth,
    all_providers_with_overlay, builtin_registry, create_models,
};

use crate::config_value::{
    config_value_env_var_names, resolve_config_value_or_throw_async, resolve_headers_or_throw,
};
use crate::model::{ModelFile, ProviderConfig, apply_models_json};

/// Pi `configContextEnv` (provider-composer.ts:279-291): resolve, **through the injected auth
/// context** rather than the ambient process env, every environment variable named by a
/// config-value template, layered over `explicit`.
async fn config_context_env(
    values: &[String],
    ctx: &dyn AuthContext,
    explicit: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let mut env: HashMap<String, String> = explicit.cloned().unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for value in values {
        for name in config_value_env_var_names(value) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for name in names {
        if env.contains_key(&name) {
            continue;
        }
        if let Some(value) = ctx.env(&name).await {
            env.insert(name, value);
        }
    }
    if env.is_empty() { None } else { Some(env) }
}

/// Pi `withConfiguredAuth` (provider-composer.ts:250-262): merge the provider's configured headers
/// over the resolved ones, then add `Authorization: Bearer <key>` when `authHeader` is set.
///
/// `authHeader` with no resolved key is Pi's `throw new Error("authHeader requires a resolved API
/// key")`; here it is an `Err` that the caller turns into a terminal stream error, never a panic.
fn with_configured_auth(
    mut auth: ModelAuth,
    headers: Option<HashMap<String, String>>,
    auth_header: bool,
) -> Result<ModelAuth, String> {
    if auth.headers.is_some() || headers.is_some() {
        let mut merged = auth.headers.take().unwrap_or_default();
        if let Some(configured) = headers {
            for (key, value) in configured {
                merged.insert(key, Some(value));
            }
        }
        auth.headers = Some(merged);
    }
    if auth_header {
        let Some(key) = auth.api_key.clone() else {
            return Err("authHeader requires a resolved API key".to_string());
        };
        auth.headers
            .get_or_insert_with(Default::default)
            .insert("Authorization".to_string(), Some(format!("Bearer {key}")));
    }
    Ok(auth)
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

fn auth_err(provider: &ProviderId, message: String) -> AuthError {
    AuthError::ApiKey {
        provider: provider.clone(),
        cause: message.into(),
    }
}

/// The API-key strategy of a **composed** provider — Pi `composeApiKeyAuth`
/// (provider-composer.ts:293-357).
///
/// Precedence, exactly as Pi's `resolve` (:333-355):
///
/// 1. an explicit/stored [`Credential`] wins and is handed to the inherited built-in strategy when
///    there is one (so e.g. anthropic's own credential handling still applies);
/// 2. else the `models.json` `apiKey` resolved through the config-value language
///    (`${VAR}` / `!command` — [`crate::config_value::resolve_config_value_or_throw`]), fed to the inherited strategy
///    as a credential when there is one;
/// 3. else the inherited strategy's own ambient resolution (its env vars).
///
/// Whatever resolves is then decorated with the provider's configured `headers` + `authHeader`.
/// A base-less provider that declares no `apiKey` therefore resolves ONLY from the credential
/// store — matching Pi, and matching this pass's scope: credentials are *read* from the existing
/// auth-store/env path, never acquired.
pub struct ConfiguredApiKeyAuth {
    provider_id: String,
    /// Pi `name: inherited?.name ?? "API key"` (provider-composer.ts:315).
    ///
    /// **[CYRUP-DELTA]** `composeApiKeyAuth` evaluates that coalesce once, at compose time, into a
    /// plain string field. [`ApiKeyAuth::name`] returns `&str`, so the resolved value is stored
    /// here rather than recomputed per call — a borrow of a `String` owned by `inherited` cannot
    /// outlive the `&self` frame. Same observable value, same evaluation point as upstream.
    name: String,
    api_key: Option<String>,
    headers: Option<HashMap<String, String>>,
    /// Per-model request headers, keyed by model id ([`raw_model_headers`]). Applied LAST, over the
    /// provider-level `headers` and over `authHeader`'s `Authorization` — Pi's ordering, where
    /// `composeApiKeyAuth` produces the provider layer and `ModelRuntime.getAuth` merges the model
    /// layer on top of the finished result (model-runtime.ts:392-396).
    model_headers: HashMap<String, HashMap<String, String>>,
    auth_header: bool,
    inherited: Option<Arc<dyn ApiKeyAuth>>,
}

#[async_trait::async_trait]
impl ApiKeyAuth for ConfiguredApiKeyAuth {
    /// Pi `name: inherited?.name ?? "API key"` (provider-composer.ts:315) — resolved at compose
    /// time into [`ConfiguredApiKeyAuth::name`]. `/login`'s provider picker labels the api-key
    /// option with this (`login.rs`'s `login_provider_options`, `method_name:
    /// api_key.name()`), so hardcoding `"API key"` here erased the inherited strategy's display
    /// name — `"Gemini API key"`, `"GitHub Copilot token"`, `"Hugging Face token"` — from every
    /// provider a `models.json` block touches.
    fn name(&self) -> &str {
        &self.name
    }

    /// Pi's `login` member is **unconditionally populated** — `inherited?.login ?? (async
    /// (interaction) => …)` (provider-composer.ts:316-321) — so the object this composes always
    /// satisfies `interactive-mode.ts:4942`'s `method.login !== undefined`, and
    /// `ai/src/models.ts:433-435`'s `if (!method?.login) throw` never fires for a composed
    /// provider.
    ///
    /// [`ApiKeyAuth::supports_login`] is cyrup's stand-in for that presence check
    /// (`cyrup-provider/src/auth/mod.rs:64-78`), and its `false` default is what silently dropped
    /// the inherited strategy's login one frame after `compose_provider_auth` captured it: every
    /// `models.json`-declared provider answered `LoginError::Unsupported` at `login.rs`'s
    /// `.filter(|s| s.supports_login())`.
    fn supports_login(&self) -> bool {
        true
    }

    /// Pi `login: inherited?.login ?? (async (interaction) => ({ type: "api_key", key: await
    /// interaction.prompt({ type: "secret", message: "Enter API key" }) }))`
    /// (provider-composer.ts:316-321).
    ///
    /// The `??` coalesce is on the *member*, not on the strategy: an `inherited` whose `login` is
    /// absent (upstream's *"Absent = ambient-only"*, `ai/src/auth/types.ts:166`) falls through to
    /// the generic flow exactly as an absent `inherited` does. `supports_login()` is cyrup's
    /// spelling of `login !== undefined`, so it is the correct discriminator here.
    ///
    /// Delegating matters because the inherited flow is frequently multi-prompt — Cloudflare
    /// (account id), Google Vertex (project + location), Amazon Bedrock (profile) — and the
    /// generic one-secret flow would persist a partial credential and report success (ADR-0010
    /// §"(3)"). Note the generic message is the literal `"Enter API key"`, NOT `env_key`'s
    /// `format!("Enter {name}")` (`auth/helpers.ts:12`): upstream inlines the fixed string here.
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        if let Some(inner) = self.inherited.as_ref()
            && inner.supports_login()
        {
            return inner.login(interaction).await;
        }
        let key = interaction
            .prompt(AuthPrompt::secret("Enter API key"))
            .await?;
        Ok(Credential::api_key(key))
    }

    async fn resolve(
        &self,
        model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let resolved: Option<AuthResult> = if let Some(cred) = cred {
            match &self.inherited {
                Some(inner) => inner.resolve(model, ctx, Some(cred)).await?,
                None => match cred {
                    Credential::ApiKey {
                        key: Some(key),
                        env,
                    } if !key.is_empty() => Some(AuthResult {
                        auth: ModelAuth {
                            api_key: Some(key.clone()),
                            ..Default::default()
                        },
                        env: env.clone(),
                        source: Some("stored credential".to_string()),
                    }),
                    _ => None,
                },
            }
        } else if let Some(raw) = &self.api_key {
            let env = config_context_env(std::slice::from_ref(raw), ctx, None).await;
            // CFG-028: `!command` resolution runs on the blocking pool (same 10 s ceiling).
            let key = resolve_config_value_or_throw_async(
                raw,
                &format!("API key for provider \"{}\"", self.provider_id),
                env.as_ref(),
            )
            .await
            .map_err(|m| auth_err(&model.provider, m))?;
            match &self.inherited {
                Some(inner) => {
                    inner
                        .resolve(model, ctx, Some(&Credential::api_key(key)))
                        .await?
                }
                None => Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        ..Default::default()
                    },
                    env: None,
                    source: Some("configured API key".to_string()),
                }),
            }
        } else {
            match &self.inherited {
                Some(inner) => inner.resolve(model, ctx, None).await?,
                None => None,
            }
        };

        let Some(mut result) = resolved else {
            return Ok(None);
        };

        // Pi :351-353 — the header templates resolve against the credential's env overlay merged
        // under the resolution's own env, then through the auth context.
        let mut explicit: HashMap<String, String> = HashMap::new();
        if let Some(env) = cred.and_then(Credential::env) {
            for (k, v) in env {
                explicit.insert(k.clone(), v.clone());
            }
        }
        if let Some(env) = &result.env {
            for (k, v) in env {
                explicit.insert(k.clone(), v.clone());
            }
        }
        let values: Vec<String> = self
            .headers
            .as_ref()
            .map(|h| h.values().cloned().collect())
            .unwrap_or_default();
        let header_env = config_context_env(&values, ctx, Some(&explicit)).await;
        let headers = resolve_headers_or_throw(
            self.headers.as_ref(),
            &format!("provider \"{}\"", self.provider_id),
            header_env.as_ref(),
        )
        .map_err(|m| auth_err(&model.provider, m))?;
        result.auth = with_configured_auth(result.auth, headers, self.auth_header)
            .map_err(|m| auth_err(&model.provider, m))?;

        // Pi `ModelRuntime.getAuth`'s model branch (model-runtime.ts:383-397): resolve THIS model's
        // configured headers (`resolveConfiguredModelHeaders`, provider-composer.ts:501-511) against
        // the same env overlay and merge them over the resolved auth headers, on every call. This is
        // the only place a `models.json` per-model header ever reaches a request.
        if let Some(raw) = self.model_headers.get(model.id.as_str()) {
            let values: Vec<String> = raw.values().cloned().collect();
            let model_env = config_context_env(&values, ctx, Some(&explicit)).await;
            let description = format!(
                "model \"{}/{}\"",
                model.provider.as_str(),
                model.id.as_str()
            );
            let resolved = resolve_headers_or_throw(Some(raw), &description, model_env.as_ref())
                .map_err(|m| auth_err(&model.provider, m))?;
            if let Some(resolved) = resolved {
                let merged = result.auth.headers.get_or_insert_with(Default::default);
                for (key, value) in resolved {
                    merged.insert(key, Some(value));
                }
            }
        }
        Ok(Some(result))
    }
}

/// Pi `composeApiKeyAuth` + `composeOAuthAuth` + the `!apiKey && !oauth` guard
/// (provider-composer.ts:293-382, :441-443).
///
/// OAuth is inherited from the base provider only — cyrup implements no OAuth *acquisition*, so a
/// `models.json` block that names an `oauth` flow cannot mint one (out of scope for this pass).
/// One consequence: a provider whose ONLY auth is that inherited OAuth strategy gets no
/// [`ConfiguredApiKeyAuth`], so neither the provider-level nor the per-model configured headers ride
/// along on its requests — Pi applies both there too (`composeOAuthAuth`'s `toAuth`,
/// provider-composer.ts:371-379, plus `getAuth`). That gap belongs to the OAuth cluster.
/// The `!inherited && !rawKey && oauth` early-out (:303) is Pi's "OAuth-only providers get no
/// fabricated API-key login method".
fn compose_provider_auth(
    provider_id: &str,
    base: Option<&ProviderAuth>,
    config: &ProviderConfig,
) -> Result<ProviderAuth, String> {
    let inherited = base.and_then(|a| a.api_key.clone());
    let oauth = base.and_then(|a| a.oauth.clone());
    let api_key: Option<Arc<dyn ApiKeyAuth>> =
        if inherited.is_none() && config.api_key.is_none() && oauth.is_some() {
            None
        } else {
            Some(Arc::new(ConfiguredApiKeyAuth {
                provider_id: provider_id.to_string(),
                // Pi `name: inherited?.name ?? "API key"` (:315).
                name: inherited
                    .as_ref()
                    .map(|i| i.name().to_string())
                    .unwrap_or_else(|| "API key".to_string()),
                api_key: config.api_key.clone(),
                headers: config
                    .headers
                    .as_ref()
                    .map(|h| h.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                model_headers: raw_model_headers(config),
                auth_header: config.auth_header.unwrap_or(false),
                inherited,
            }))
        };
    if api_key.is_none() && oauth.is_none() {
        return Err(format!(
            "Provider {provider_id}: no authentication method configured."
        ));
    }
    Ok(ProviderAuth { api_key, oauth })
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
            let base_models: Vec<Model> = base
                .as_ref()
                .map(|p| p.models().to_vec())
                .unwrap_or_default();
            let base_auth = base.as_ref().and_then(|p| p.provider_auth());
            let composed = match apply_models_json(provider_id, &base_models, config) {
                Ok(models) => models,
                Err(message) => {
                    errors.push(message);
                    continue;
                }
            };
            let auth = match compose_provider_auth(provider_id, base_auth, config) {
                Ok(auth) => auth,
                Err(message) => {
                    errors.push(message);
                    continue;
                }
            };
            let name = config.name.clone().unwrap_or_else(|| provider_id.clone());
            let mut provider = WireProvider::new(
                provider_id.as_str(),
                name,
                composed,
                auth,
                store.clone(),
                registry.clone(),
            );
            if let Some(ctx) = &auth_context {
                provider = provider.with_auth_context(ctx.clone());
            }
            // Release the base handle (and `base_auth`, which borrows it) before the upsert that
            // replaces it — Pi `models.setProvider(...)`, model-runtime.ts:215.
            drop(base);
            models.set_provider(Arc::new(provider));
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
    use cyrup_provider::auth::oauth::{AuthPromptKind, ScriptedInteraction};

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
