//! The ONE provider composer — pi `composeModelProvider` (`coding-agent/src/core/
//! provider-composer.ts:542-683` @f1b2e77f5), shared by every layer cyrup composes over a base.
//!
//! pi composes a provider id from three layers: the `base` (`nativeExtensionProviders.get(id) ??
//! builtins.get(id)`, `model-runtime.ts:301-310`), the `models.json` block (`config`) and an
//! extension's `registerProvider` (`extension`). cyrup reaches that composition from two places:
//! the `models.json` composer (`cyrup-config`'s `ModelFile::compose_providers`) and a guest
//! registration over a built-in id (`cyrup-ext`'s `ProviderRegistration::compose_over`, driven by
//! the session's guest-provider registry). Both hand [`compose_over_base`] the layer's own catalog
//! (each layer's catalog step is its own: `applyModelsJson` / `applyExtension`) and the layer's
//! auth inputs, and get back the same [`WireProvider`] shape, so there is one place where:
//!
//! * the composed auth keeps the base's layers ([`compose_provider_auth`], pi
//!   `composeApiKeyAuth`/`composeOAuthAuth`, `:417-506`): `inherited = base.auth.apiKey`, a
//!   stored credential first, then the layer's configured key, then the inherited ambient
//!   resolution; `oauth = extension.oauth ?? base.auth.oauth`;
//! * `name`/`baseUrl` fall back to the base's (`:607-608`) and `headers` are the base's (`:609`);
//! * a model whose api the base lists streams THROUGH the base (`supportsBaseApi`, `:582`,
//!   `:593-596`) — [`WireProvider::with_operations_from`] + [`ApiDispatch::Global`];
//! * images, classification and availability filters are the base's (`:636-680`).
//!
//! The configured key and header VALUES are resolved through a [`ConfigValues`] the layer
//! supplies: `models.json` values are pi's config-value language (`$VAR`, `!command`), which
//! `cyrup-config` implements; a guest registration's key is already resolved at registration
//! time (`cyrup-ext`'s `resolve_api_key`), so it rides through [`LiteralValues`].

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_core::ProviderId;

use crate::api::ApiRegistry;
use crate::auth::oauth::{AuthInteraction, AuthPrompt, OAuthError};
use crate::auth::{
    ApiKeyAuth, AuthContext, AuthResult, Credential, CredentialStore, ModelAuth, ProviderAuth,
};
use crate::classifier::{ClassifierModel, ImageModel};
use crate::error::AuthError;
use crate::model::Model;
use crate::provider::Provider;
use crate::wire::{ApiDispatch, WireProvider};

/// A plain string-to-string header map, as a layer declares it (before resolution).
pub type RawHeaders = HashMap<String, String>;

/// How a layer's configured VALUES resolve — the key and the header templates (pi
/// `resolveConfigValueOrThrow` / `resolveHeadersOrThrow` / `getConfigValueEnvVarNames`,
/// `resolve-config-value.ts`).
#[async_trait::async_trait]
pub trait ConfigValues: Send + Sync {
    /// The environment variables a raw value references (pi `getConfigValueEnvVarNames`), read
    /// through the auth context before [`ConfigValues::resolve`] runs.
    fn env_var_names(&self, raw: &str) -> Vec<String>;

    /// Resolve one raw value (pi `resolveConfigValueOrThrow`). `description` names the value in
    /// the error text.
    async fn resolve(
        &self,
        raw: &str,
        description: &str,
        env: Option<&HashMap<String, String>>,
    ) -> Result<String, String>;

    /// Resolve a header map (pi `resolveHeadersOrThrow`); `None` when no header survives.
    fn resolve_headers(
        &self,
        headers: Option<&RawHeaders>,
        description: &str,
        env: Option<&HashMap<String, String>>,
    ) -> Result<Option<RawHeaders>, String>;
}

/// [`ConfigValues`] for values that are already resolved: each value is itself. A guest
/// registration's `apiKey` is resolved once, at registration (`cyrup-ext`'s `resolve_api_key`),
/// so the composed provider must not resolve it a second time.
pub struct LiteralValues;

#[async_trait::async_trait]
impl ConfigValues for LiteralValues {
    fn env_var_names(&self, _raw: &str) -> Vec<String> {
        Vec::new()
    }

    async fn resolve(
        &self,
        raw: &str,
        _description: &str,
        _env: Option<&HashMap<String, String>>,
    ) -> Result<String, String> {
        Ok(raw.to_string())
    }

    fn resolve_headers(
        &self,
        headers: Option<&RawHeaders>,
        _description: &str,
        _env: Option<&HashMap<String, String>>,
    ) -> Result<Option<RawHeaders>, String> {
        Ok(headers.filter(|h| !h.is_empty()).cloned())
    }
}

/// The auth inputs one layer contributes (pi `configuredApiKey`, `configuredHeaders`,
/// `authHeader`, `rawModelHeaders`, `provider-composer.ts:388-401`, `:429`, `:508-527`).
#[derive(Clone)]
pub struct AuthLayer {
    /// The layer's configured key, unresolved (pi `extension?.apiKey ?? config?.apiKey`).
    pub api_key: Option<String>,
    /// The layer's provider-level headers, unresolved.
    pub headers: Option<RawHeaders>,
    /// Per-model request headers keyed by model id, unresolved — applied LAST, over the
    /// provider-level headers and over `authHeader`'s `Authorization` (pi `ModelRuntime.getAuth`,
    /// `model-runtime.ts:383-397`).
    pub model_headers: HashMap<String, RawHeaders>,
    /// pi `authHeader`: add `Authorization: Bearer <resolved key>`.
    pub auth_header: bool,
    /// How the values above resolve.
    pub values: Arc<dyn ConfigValues>,
}

impl AuthLayer {
    /// A layer that configures nothing, resolving through `values`.
    pub fn new(values: Arc<dyn ConfigValues>) -> Self {
        Self {
            api_key: None,
            headers: None,
            model_headers: HashMap::new(),
            auth_header: false,
            values,
        }
    }
}

/// Pi `configContextEnv` (provider-composer.ts:403-415): resolve, **through the injected auth
/// context** rather than the ambient process env, every environment variable named by a
/// config-value template, layered over `explicit`.
async fn config_context_env(
    values_of: &dyn ConfigValues,
    values: &[String],
    ctx: &dyn AuthContext,
    explicit: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let mut env: HashMap<String, String> = explicit.cloned().unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for value in values {
        for name in values_of.env_var_names(value) {
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

/// Pi `withConfiguredAuth` (provider-composer.ts:374-386): merge the provider's configured headers
/// over the resolved ones, then add `Authorization: Bearer <key>` when `authHeader` is set.
///
/// `authHeader` with no resolved key is Pi's `throw new Error("authHeader requires a resolved API
/// key")`; here it is an `Err` that the caller turns into a terminal stream error, never a panic.
fn with_configured_auth(
    mut auth: ModelAuth,
    headers: Option<RawHeaders>,
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

fn auth_err(provider: &ProviderId, message: String) -> AuthError {
    AuthError::ApiKey {
        provider: provider.clone(),
        cause: message.into(),
    }
}

/// The API-key strategy of a **composed** provider — Pi `composeApiKeyAuth`
/// (provider-composer.ts:417-481).
///
/// Precedence, exactly as Pi's `resolve` (:457-479):
///
/// 1. an explicit/stored [`Credential`] wins and is handed to the inherited built-in strategy when
///    there is one (so e.g. anthropic's own credential handling still applies);
/// 2. else the layer's configured `apiKey`, resolved through its [`ConfigValues`], fed to the
///    inherited strategy as a credential when there is one;
/// 3. else the inherited strategy's own ambient resolution (its env vars).
///
/// Whatever resolves is then decorated with the layer's configured `headers` + `authHeader`, and
/// the resolved model's own configured headers on top.
pub struct ConfiguredApiKeyAuth {
    provider_id: String,
    /// Pi `name: inherited?.name ?? "API key"` (provider-composer.ts:431).
    ///
    /// **[CYRUP-DELTA]** `composeApiKeyAuth` evaluates that coalesce once, at compose time, into a
    /// plain string field. [`ApiKeyAuth::name`] returns `&str`, so the resolved value is stored
    /// here rather than recomputed per call — a borrow of a `String` owned by `inherited` cannot
    /// outlive the `&self` frame. Same observable value, same evaluation point as upstream.
    name: String,
    layer: AuthLayer,
    inherited: Option<Arc<dyn ApiKeyAuth>>,
}

#[async_trait::async_trait]
impl ApiKeyAuth for ConfiguredApiKeyAuth {
    /// Pi `name: inherited?.name ?? "API key"` (provider-composer.ts:431) — resolved at compose
    /// time. `/login`'s provider picker labels the api-key option with this.
    fn name(&self) -> &str {
        &self.name
    }

    /// Pi's `login` member is **unconditionally populated** — `inherited?.login ?? (async
    /// (interaction) => …)` (provider-composer.ts:432-437) — so the object this composes always
    /// satisfies `method.login !== undefined`.
    fn supports_login(&self) -> bool {
        true
    }

    /// Pi `login: inherited?.login ?? (async (interaction) => ({ type: "api_key", key: await
    /// interaction.prompt({ type: "secret", message: "Enter API key" }) }))`
    /// (provider-composer.ts:432-437). The `??` coalesce is on the *member*: an `inherited` whose
    /// `login` is absent falls through to the generic flow exactly as an absent `inherited` does.
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
        let values = self.layer.values.as_ref();
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
        } else if let Some(raw) = &self.layer.api_key {
            let env = config_context_env(values, std::slice::from_ref(raw), ctx, None).await;
            let key = values
                .resolve(
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

        // Pi :475-477 — the header templates resolve against the credential's env overlay merged
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
        let header_values: Vec<String> = self
            .layer
            .headers
            .as_ref()
            .map(|h| h.values().cloned().collect())
            .unwrap_or_default();
        let header_env = config_context_env(values, &header_values, ctx, Some(&explicit)).await;
        let headers = values
            .resolve_headers(
                self.layer.headers.as_ref(),
                &format!("provider \"{}\"", self.provider_id),
                header_env.as_ref(),
            )
            .map_err(|m| auth_err(&model.provider, m))?;
        result.auth = with_configured_auth(result.auth, headers, self.layer.auth_header)
            .map_err(|m| auth_err(&model.provider, m))?;

        // Pi `ModelRuntime.getAuth`'s model branch (model-runtime.ts:383-397): resolve THIS model's
        // configured headers (`resolveConfiguredModelHeaders`, provider-composer.ts:685-696) against
        // the same env overlay and merge them over the resolved auth headers, on every call.
        if let Some(raw) = self.layer.model_headers.get(model.id.as_str()) {
            let raw_values: Vec<String> = raw.values().cloned().collect();
            let model_env = config_context_env(values, &raw_values, ctx, Some(&explicit)).await;
            let description = format!(
                "model \"{}/{}\"",
                model.provider.as_str(),
                model.id.as_str()
            );
            let resolved = values
                .resolve_headers(Some(raw), &description, model_env.as_ref())
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
/// (provider-composer.ts:417-506, :578-580).
///
/// `inherited = base.auth.apiKey` and `oauth = extension.oauth ?? base.auth.oauth`. Neither layer
/// cyrup composes can mint an OAuth flow of its own (a `models.json` `oauth` block names pi's
/// radius gateway; a guest registration's `oauth` block is closures the host does not drive,
/// EXT-M07), so the OAuth strategy is the base's. One consequence: a provider whose ONLY auth is
/// that inherited OAuth strategy gets no [`ConfiguredApiKeyAuth`], so the layer's configured headers
/// do not ride along on its requests — pi applies them there too (`composeOAuthAuth`'s `toAuth`,
/// `:495-504`). That gap belongs to the OAuth cluster. The `!inherited && rawKey === undefined &&
/// oauth` early-out (:427) is Pi's "OAuth-only providers get no fabricated API-key login method".
///
/// # Errors
///
/// Pi's `Provider <id>: no authentication method configured.` when neither strategy exists.
pub fn compose_provider_auth(
    provider_id: &str,
    base: Option<&ProviderAuth>,
    layer: &AuthLayer,
) -> Result<ProviderAuth, String> {
    let inherited = base.and_then(|a| a.api_key.clone());
    let oauth = base.and_then(|a| a.oauth.clone());
    let api_key: Option<Arc<dyn ApiKeyAuth>> =
        if inherited.is_none() && layer.api_key.is_none() && oauth.is_some() {
            None
        } else {
            Some(Arc::new(ConfiguredApiKeyAuth {
                provider_id: provider_id.to_string(),
                name: inherited
                    .as_ref()
                    .map(|i| i.name().to_string())
                    .unwrap_or_else(|| "API key".to_string()),
                layer: layer.clone(),
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

/// One layer composed over a base: its catalog (already through the layer's own catalog step —
/// `applyModelsJson` or `applyExtension`), its display name and base URL, and its auth inputs.
pub struct ProviderLayer {
    /// The layer's display name, if it sets one (pi `extension?.name ?? config?.name`); `None`
    /// falls back to the base's name, then the id (`provider-composer.ts:607`).
    pub name: Option<String>,
    /// The layer's provider-level base URL (pi `extension?.baseUrl ?? config?.baseUrl`); `None`
    /// falls back to the base's (`:608`).
    pub base_url: Option<String>,
    /// The composed chat catalog.
    pub models: Vec<Model>,
    /// The composed image catalog.
    pub image_models: Vec<ImageModel>,
    /// The composed classifier catalog.
    pub classifier_models: Vec<ClassifierModel>,
    /// The layer's auth inputs.
    pub auth: AuthLayer,
}

/// Compose `layer` over `base` into the provider pi's `composeModelProvider` builds
/// (`provider-composer.ts:542-683` @f1b2e77f5) — see the module docs for what it keeps of the
/// base. `base = None` is pi's base-less composition: it streams every model through the
/// process-wide `registry` (`streamWith`'s fall-through, `:598-599`).
///
/// `auth_context` overrides the composed provider's ambient env source; `None` keeps the real
/// process environment.
///
/// # Errors
///
/// Pi's `Provider <id>: no authentication method configured.` (`:580`).
pub fn compose_over_base(
    provider_id: &str,
    base: Option<Arc<dyn Provider>>,
    layer: ProviderLayer,
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
    auth_context: Option<Arc<dyn AuthContext>>,
) -> Result<WireProvider, String> {
    let auth = compose_provider_auth(
        provider_id,
        base.as_ref().and_then(|b| b.provider_auth()),
        &layer.auth,
    )?;
    // Pi `name: extension?.name ?? config?.name ?? base?.name ?? … ?? providerId` and `baseUrl:
    // extension?.baseUrl ?? config?.baseUrl ?? base?.baseUrl` (provider-composer.ts:607-608).
    let name = layer
        .name
        .or_else(|| base.as_ref().map(|b| b.name().to_string()))
        .unwrap_or_else(|| provider_id.to_string());
    let base_url = layer
        .base_url
        .or_else(|| base.as_ref().and_then(|b| b.base_url().map(str::to_string)));
    let mut provider = WireProvider::new(provider_id, name, layer.models, auth, store, registry)
        .with_image_models(layer.image_models)
        .with_classifier_models(layer.classifier_models)
        // `streamWith` falls through to the process-wide registry (provider-composer.ts:598-599);
        // a model its base lists is the base's (`:593-596`), handed over per request through
        // `with_operations_from` below. PROV-157, PROV-162.
        .with_api_dispatch(ApiDispatch::Global);
    if let Some(base_url) = base_url {
        provider = provider.with_base_url(base_url);
    }
    if let Some(ctx) = auth_context {
        provider = provider.with_auth_context(ctx);
    }
    if let Some(base) = base {
        // SEAM-173 — a base that encapsulates its own auth, under a layer that configures none,
        // keeps doing so (see `WireProvider::with_auth_from_base`).
        if base.provider_auth().is_none() && layer_configures_no_auth(&layer.auth) {
            provider = provider.with_auth_from_base();
        }
        // Pi `headers: base?.headers` (provider-composer.ts:609).
        if let Some(headers) = base.headers() {
            provider = provider.with_headers(headers.clone());
        }
        // Pi's composed provider streams a base-api model through the base, and calls
        // `base.generateImages` / `base.classify` / `base.filterModels` (:593-596, :636-680): the
        // layer changes the catalog and the auth, never what the provider can do.
        provider = provider.with_operations_from(base);
    }
    Ok(provider)
}

/// Whether `layer` configures no auth input at all: no key, no header of either scope, no
/// `authHeader`.
fn layer_configures_no_auth(layer: &AuthLayer) -> bool {
    layer.api_key.is_none()
        && layer.headers.is_none()
        && layer.model_headers.is_empty()
        && !layer.auth_header
}

/// Pi `findExtensionModelDefaults` (provider-composer.ts:261-273 @f1b2e77f5) for a chat
/// definition: the base row an extension's model definition fills its gaps (`api`, `baseUrl`)
/// from — the same `id`, else the first row on the definition's own `api`, else the first
/// `openai-completions` row (chat only, which every definition cyrup carries is), else the first
/// row at all.
pub fn find_extension_model_defaults<'a>(
    models: &'a [Model],
    id: &str,
    api: Option<&str>,
) -> Option<&'a Model> {
    models
        .iter()
        .find(|m| m.id.as_str() == id)
        .or_else(|| api.and_then(|api| models.iter().find(|m| m.api.as_str() == api)))
        .or_else(|| {
            models
                .iter()
                .find(|m| m.api.as_str() == crate::known_api::OPENAI_COMPLETIONS)
        })
        .or_else(|| models.first())
}
