//! The Radius gateway provider — port of pi v0.84.4 `packages/ai/src/providers/radius.ts`
//! (`radiusProvider`, `:20-82`) and the whole of `packages/ai/src/providers/radius-config.ts`
//! (`:1-96`). PROV-014.
//!
//! Radius is pi's **only dynamic built-in**: it ships no static catalog (`all.ts:50-52` @v0.84.4 —
//! "`KnownProvider` additionally includes purely dynamic providers (e.g. `radius`) that have no
//! static catalog entry"), speaks the [`pi-messages`](crate::api::pi_messages) protocol, and learns
//! its models from the gateway's own `GET /v1/config` (`radius-config.ts:80-96`). Two things make
//! it a provider *kind* rather than a fleet row:
//!
//! 1. **Auth is api-key OR OAuth** — `envApiKeyAuth("Radius API key", ["RADIUS_API_KEY"])` plus
//!    `lazyOAuth({ name, load: () => loadRadiusOAuth({ name, gateway }) })` (`radius.ts:30-33`),
//!    and the OAuth flow is parameterised by the gateway. The flow itself is
//!    [`crate::auth::oauth::radius::RadiusOAuth`]; this module only wires it.
//! 2. **The catalog is refreshed, not embedded** — `refreshModels` (`radius.ts:35-78`) restores
//!    the persisted catalog, imports a legacy one cached on a pre-`ModelsStore` OAuth credential,
//!    then (network permitting) fetches `{gateway}/v1/config` and publishes the result.
//!
//! # Tag-to-tag: v0.83.0 → v0.84.4
//!
//! `radius-config.ts` is byte-identical at both tags. `radius.ts` changed shape once, at v0.84.x:
//! v0.83.0 (`:36-63`) dedups with a hand-rolled `inflightRefresh ??=` memo and reads/writes
//! `context.store` directly, while v0.84.4 (`:35-78`) receives the snapshot as `context.stored`
//! and writes through the generation-checked `context.publish({ persist, update })`. The
//! *observable* sequence — restore, legacy import, network gate, fetch, persist — is the same at
//! both tags, and that sequence is what [`RadiusProvider::refresh_models`] ports.
//!
//! # Where the catalog lives in cyrup (a deliberate shape divergence, shared with the pi.dev overlay)
//!
//! Upstream mutates a closure-captured `models` array in place and `getModels` returns it. cyrup's
//! [`Provider::models`] returns a BORROWED slice, so an in-place update is not expressible without
//! changing the trait (the same constraint [`crate::remote_catalog`] documents at its head). The
//! port therefore splits pi's one function in two, exactly as the pi.dev overlay does:
//!
//! * **publish** — [`RadiusProvider::refresh_models`] fetches the gateway config and writes it to
//!   the attached [`ModelsStore`] under the provider id (pi's `persist: { models, checkedAt }`).
//! * **restore** — the next registry build reads the store back through
//!   [`crate::remote_catalog::RemoteCatalog::load_overlay`] and merges it over this provider's
//!   (empty) embedded catalog via [`crate::remote_catalog::CatalogOverlay::apply`] (pi's
//!   `context.stored` restore, `radius.ts:36-48`).
//!
//! That is behaviourally equivalent because cyrup keeps no long-lived registry — every read
//! rebuilds `all_providers_with_overlay(..)` — and it is why the persisted entry carries a
//! `last_modified` stamp (see [`RadiusProvider::refresh_models`] for that one `[CYRUP-DELTA]`).
//!
//! **What the pi.dev overlay must NOT do to this provider.** Upstream wraps every built-in in
//! `withRemoteCatalog` EXCEPT radius (`coding-agent/src/core/model-runtime.ts:183-189` @v0.84.4:
//! `provider.id === "radius" ? provider : withRemoteCatalog(…)`), because radius's store entry is
//! owned by the gateway refresh, not by pi.dev. cyrup mirrors that at the trigger site
//! (`crates/cyrup/src/provider.rs`), which excludes `radius` from the ids it fetches from pi.dev —
//! otherwise pi.dev's `404` branch would rewrite the entry's `lastModified` to `0` and the overlay
//! loader's staleness guard would discard the gateway catalog on the next start.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::oauth::load::RadiusOptions;
use crate::auth::oauth::radius::{RadiusOAuth, normalize_radius_gateway_url};
use crate::auth::{
    AuthContext, AuthOverrides, Credential, CredentialStore, EnvAuthContext,
    InMemoryCredentialStore, ProviderAuth, env_key, resolve_provider_auth,
};
use crate::context::Context;
use crate::error::ProviderError;
use crate::known_api::PI_MESSAGES;
use crate::model::{Modality, Model, ModelCost, ThinkingLevelMap};
use crate::models_store::{ModelsStore, ModelsStoreEntry};
use crate::provider::{Provider, RefreshModelsContext};
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::refresh::RefreshDedup;
use crate::utils::simple_options::SimpleStreamOptions;
use crate::wire::WireProvider;
use cyrup_core::{CancelToken, EventStream, ProviderId};
use std::sync::Arc;
use std::time::Duration;

/// The built-in provider id (`radius.ts:21` — `options.id ?? "radius"`).
pub const RADIUS_PROVIDER_ID: &str = "radius";

/// The built-in display name (`radius.ts:22` — `options.name ?? "Radius"`).
pub const RADIUS_PROVIDER_NAME: &str = "Radius";

/// `DEFAULT_RADIUS_GATEWAY` (`radius-config.ts:4`).
pub const DEFAULT_RADIUS_GATEWAY: &str = "https://radius.pi.dev";

/// The env var carrying the Radius API key (`envApiKeyAuth("Radius API key",
/// ["RADIUS_API_KEY"])`, `radius.ts:31`; `env-api-keys.ts:93`).
pub const RADIUS_API_KEY_ENV: &str = "RADIUS_API_KEY";

/// Upstream's `envApiKeyAuth` label — what `/login` lists and `Enter {name}` interpolates.
pub const RADIUS_API_KEY_AUTH_NAME: &str = "Radius API key";

/// `truncateHttpBody`'s cap (`radius-config.ts:75-78`): 512 characters plus a `…`.
const MAX_CONFIG_ERROR_BODY_CHARS: usize = 512;

/// Per-request budget for the gateway config fetch. Upstream has none (it relies on the caller's
/// abort signal, `radius-config.ts:87`); cyrup applies the same 15s the pi.dev overlay applies
/// per request (`remote_catalog.rs`'s `DEFAULT_REQUEST_TIMEOUT`) so a hung gateway can never pin
/// a background refresh task. The abort token is honoured independently of this.
const DEFAULT_CONFIG_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// ------------------------------------------------------------------------------ radius-config --

/// `RadiusProviderOptions` (`radius.ts:13-17`): every member optional, defaults applied in
/// [`RadiusProvider::new`] exactly as `radiusProvider` applies them (`:21-23`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadiusProviderOptions {
    pub id: Option<String>,
    pub name: Option<String>,
    pub gateway: Option<String>,
}

/// One row of the gateway's `/v1/config` (`RadiusGatewayModel`, `radius-config.ts:6-15`).
///
/// Field for field upstream's type; `serde` is the type check that `isRadiusGatewayModel`
/// (`:26-40`) performs by hand — see [`sanitize_radius_gateway_config`] for the two places the
/// two differ.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiusGatewayModel {
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<Modality>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
}

/// `RadiusGatewayConfig` (`radius-config.ts:17-20`): the request endpoint every model shares plus
/// the rows.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiusGatewayConfig {
    pub base_url: String,
    pub models: Vec<RadiusGatewayModel>,
}

/// `isRadiusGatewayModel` (`radius-config.ts:26-40`), verbatim: an object (not an array) whose
/// `id`/`name` are strings, `reasoning` a boolean, `input` an array, `cost` a non-null non-array
/// object, and `contextWindow`/`maxTokens` numbers. `thinkingLevelMap` is not checked upstream and
/// is not checked here.
fn is_radius_gateway_model(value: &serde_json::Value) -> bool {
    let Some(model) = value.as_object() else {
        return false;
    };
    model.get("id").is_some_and(serde_json::Value::is_string)
        && model.get("name").is_some_and(serde_json::Value::is_string)
        && model
            .get("reasoning")
            .is_some_and(serde_json::Value::is_boolean)
        && model.get("input").is_some_and(serde_json::Value::is_array)
        && model.get("cost").is_some_and(serde_json::Value::is_object)
        && model
            .get("contextWindow")
            .is_some_and(serde_json::Value::is_number)
        && model
            .get("maxTokens")
            .is_some_and(serde_json::Value::is_number)
}

/// `sanitizeRadiusGatewayConfig` (`radius-config.ts:42-50`): `None` unless `config` is an object
/// carrying a string `baseUrl` and an array `models`; rows that fail
/// [`is_radius_gateway_model`] are FILTERED, never fatal (`:48`).
///
/// `[CYRUP-DELTA]` two narrowings, both floor-preserving. Upstream keeps a row after the shape
/// check even when its *contents* are off — an `input` entry that is neither `"text"` nor
/// `"image"`, a `cost` object missing a rate, a fractional `contextWindow` — because its `Model` is
/// structural. cyrup's [`Model`] is not, so a row that passes the shape check but does not
/// deserialize is DROPPED rather than fabricated with invented values (the same choice
/// [`crate::remote_catalog::parse_catalog`] records). The gateway's other rows survive.
pub fn sanitize_radius_gateway_config(config: &serde_json::Value) -> Option<RadiusGatewayConfig> {
    let object = config.as_object()?;
    let base_url = object.get("baseUrl")?.as_str()?.to_string();
    let models = object.get("models")?.as_array()?;
    Some(RadiusGatewayConfig {
        base_url,
        models: models
            .iter()
            .filter(|row| is_radius_gateway_model(row))
            .filter_map(|row| serde_json::from_value::<RadiusGatewayModel>(row.clone()).ok())
            .collect(),
    })
}

/// `getRadiusCredentialConfig` (`radius-config.ts:57-59`): the `gatewayConfig` a pre-`ModelsStore`
/// Radius OAuth login cached ON the credential (`RadiusOAuthCredential`, `:22-24`). cyrup's
/// [`Credential::Oauth`] keeps such extra members in its flattened `ext` map, which is where it is
/// read from here. `None` for an api-key credential, no credential, or no/invalid config.
pub fn radius_credential_config(credential: Option<&Credential>) -> Option<RadiusGatewayConfig> {
    match credential {
        Some(Credential::Oauth { ext, .. }) => ext
            .get("gatewayConfig")
            .and_then(sanitize_radius_gateway_config),
        _ => None,
    }
}

/// `getRadiusModelsFromConfig` (`radius-config.ts:61-68`): every row spread onto a `pi-messages`
/// [`Model`] stamped with this provider's id and the config's shared `baseUrl`.
pub fn radius_models_from_config(provider_id: &str, config: &RadiusGatewayConfig) -> Vec<Model> {
    config
        .models
        .iter()
        .map(|row| Model {
            id: row.id.as_str().into(),
            name: row.name.clone(),
            api: PI_MESSAGES.into(),
            provider: provider_id.into(),
            base_url: config.base_url.clone(),
            reasoning: row.reasoning,
            input: row.input.clone(),
            cost: row.cost.clone(),
            context_window: row.context_window,
            max_tokens: row.max_tokens,
            sampling_params: None,
            thinking_level_map: row.thinking_level_map.clone(),
            compat: None,
            headers: None,
        })
        .collect()
}

/// `getRadiusModels` (`radius-config.ts:70-73`): the legacy credential-cached catalog, or empty.
pub fn radius_models(provider_id: &str, credential: Option<&Credential>) -> Vec<Model> {
    radius_credential_config(credential)
        .map(|config| radius_models_from_config(provider_id, &config))
        .unwrap_or_default()
}

/// `truncateHttpBody` (`radius-config.ts:75-78`): trim, then cap at 512 chars with a `…`.
/// Character-based like upstream's `slice`, so a multi-byte body is never cut mid-scalar.
pub fn truncate_http_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > MAX_CONFIG_ERROR_BODY_CHARS {
        let head: String = trimmed.chars().take(MAX_CONFIG_ERROR_BODY_CHARS).collect();
        format!("{head}…")
    } else {
        trimmed.to_string()
    }
}

/// `new URL("/v1/config", gateway)` (`radius-config.ts:87`) for an already-normalized gateway: an
/// absolute path resolves against the gateway's ORIGIN, so any path the configured gateway carries
/// is discarded — the WHATWG behaviour upstream relies on, and the same rule
/// [`crate::auth::oauth::radius`] applies to the OAuth endpoints.
fn config_url(gateway: &str) -> String {
    match gateway.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{host}/v1/config")
        }
        // Unreachable after `normalize_radius_gateway_url`, which always supplies a scheme.
        None => format!("{gateway}/v1/config"),
    }
}

/// `loadRadiusGatewayConfig` (`radius-config.ts:80-96`): `GET {gateway}/v1/config` with
/// `accept: application/json` and — only when a key is available — `authorization: Bearer {key}`
/// (`:85-86`). A non-OK status is `Could not load Radius config from {gateway}: {status}:
/// {truncated body}` (`:88-92`); a body that does not sanitize is `Invalid Radius config from
/// {gateway}` (`:93-94`). Both messages are upstream's, byte for byte, so the `/model` picker and
/// `cyrup update --models` report what pi reports.
///
/// `cancel` is upstream's `signal`: the request is raced against it and an abort yields
/// [`ProviderError::Aborted`]. The HTTP client is built per target so `HTTP(S)_PROXY`/`NO_PROXY`
/// resolve exactly as they do for provider traffic (PROV-047).
pub async fn load_radius_gateway_config(
    gateway: &str,
    api_key: Option<&str>,
    ctx: &dyn AuthContext,
    cancel: &CancelToken,
) -> Result<RadiusGatewayConfig, ProviderError> {
    load_radius_gateway_config_with_timeout(
        gateway,
        api_key,
        ctx,
        cancel,
        DEFAULT_CONFIG_REQUEST_TIMEOUT,
    )
    .await
}

async fn load_radius_gateway_config_with_timeout(
    gateway: &str,
    api_key: Option<&str>,
    ctx: &dyn AuthContext,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<RadiusGatewayConfig, ProviderError> {
    let url = config_url(gateway);
    let client = crate::stream::sse::build_client_for_target(&url, ctx, None, None).await?;
    let mut request = client
        .get(&url)
        .timeout(timeout)
        .header("accept", "application/json");
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.header("authorization", format!("Bearer {key}"));
    }

    let response = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ProviderError::Aborted),
        sent = request.send() => sent.map_err(|e| ProviderError::Transport(Box::new(e)))?,
    };
    let status = response.status().as_u16();
    let body = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ProviderError::Aborted),
        text = response.text() => text.map_err(|e| ProviderError::Transport(Box::new(e)))?,
    };

    if !(200..300).contains(&status) {
        return Err(ProviderError::Http {
            status,
            message: format!(
                "Could not load Radius config from {gateway}: {status}: {}",
                truncate_http_body(&body)
            ),
        });
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| ProviderError::Decode(format!("Invalid Radius config from {gateway}")))?;
    sanitize_radius_gateway_config(&value)
        .ok_or_else(|| ProviderError::Decode(format!("Invalid Radius config from {gateway}")))
}

// -------------------------------------------------------------------------------- the provider --

/// The `auth: { apiKey, oauth }` clause of `radiusProvider` (`radius.ts:30-33`) for a gateway.
///
/// `lazyOAuth({ name, load: () => loadRadiusOAuth({ name, gateway }) })` is a
/// [`RadiusOAuth`] built directly: Rust links statically, so there is nothing to defer (the same
/// reasoning as [`super::builtin_oauth`]). The flow's `name` is the PROVIDER's display name, not a
/// constant — a `models.json` provider declaring `"oauth": "radius"` signs in under its own name.
pub fn radius_auth(name: &str, gateway: &str) -> ProviderAuth {
    ProviderAuth {
        api_key: Some(env_key(RADIUS_API_KEY_AUTH_NAME, [RADIUS_API_KEY_ENV])),
        oauth: Some(Arc::new(RadiusOAuth::new(RadiusOptions {
            name: name.to_string(),
            gateway: gateway.to_string(),
        }))),
    }
}

/// The Radius gateway provider (`radiusProvider`, `radius.ts:20-82`).
///
/// Streaming, auth resolution and api dispatch are a [`WireProvider`] over the shared
/// [`ApiRegistry`]'s `pi-messages` impl (`streams.stream(model, context, streamOptions)`,
/// `:79-80`); this type adds the gateway, the catalog publication seam and
/// [`Provider::refresh_models`]. Every `Provider` surface method is delegated by name (PROV-M01):
/// a trait default answering for the inner would silently rename the provider or drop its auth.
pub struct RadiusProvider {
    id: ProviderId,
    name: String,
    /// `normalizeRadiusGatewayUrl(options.gateway ?? DEFAULT_RADIUS_GATEWAY)` (`:23`).
    gateway: String,
    auth: ProviderAuth,
    credentials: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
    auth_ctx: Arc<dyn AuthContext>,
    inner: WireProvider,
    /// Where [`Self::refresh_models`] publishes (pi `context.publish({ persist })`). `None` makes
    /// this a STATIC provider — there is nowhere to publish, so `refresh_models` answers `None`
    /// like every other static built-in rather than fetching a catalog it must then discard.
    models_store: Option<Arc<dyn ModelsStore>>,
    /// Concurrent refreshes share one in-flight fetch (`inflightRefresh ??=` at v0.83.0
    /// `radius.ts:37`; the trait contract at [`Provider::refresh_models`]).
    dedup: RefreshDedup,
    request_timeout: Duration,
}

impl RadiusProvider {
    /// `radiusProvider(options)` (`radius.ts:20-27`) over an explicit credential store + api
    /// registry. The catalog starts EMPTY — `getRadiusModels(id, undefined)` (`:24`) is `[]`
    /// because there is no credential to read a legacy config from — and is filled by
    /// [`RadiusProvider::with_models`] (the persisted overlay) or by a refresh.
    pub fn new(
        options: RadiusProviderOptions,
        credentials: Arc<dyn CredentialStore>,
        registry: Arc<ApiRegistry>,
    ) -> Self {
        let id: ProviderId = options.id.as_deref().unwrap_or(RADIUS_PROVIDER_ID).into();
        let name = options
            .name
            .unwrap_or_else(|| RADIUS_PROVIDER_NAME.to_string());
        let gateway = normalize_radius_gateway_url(
            options.gateway.as_deref().unwrap_or(DEFAULT_RADIUS_GATEWAY),
        );
        let auth = radius_auth(&name, &gateway);
        let auth_ctx: Arc<dyn AuthContext> = Arc::new(EnvAuthContext);
        let inner = WireProvider::new(
            id.clone(),
            name.clone(),
            Vec::new(),
            auth.clone(),
            credentials.clone(),
            registry.clone(),
        )
        .with_auth_context(auth_ctx.clone());
        Self {
            id,
            name,
            gateway,
            auth,
            credentials,
            registry,
            auth_ctx,
            inner,
            models_store: None,
            dedup: RefreshDedup::new(),
            request_timeout: DEFAULT_CONFIG_REQUEST_TIMEOUT,
        }
    }

    fn rebuild_inner(&mut self, models: Vec<Model>) {
        self.inner = WireProvider::new(
            self.id.clone(),
            self.name.clone(),
            models,
            self.auth.clone(),
            self.credentials.clone(),
            self.registry.clone(),
        )
        .with_auth_context(self.auth_ctx.clone());
    }

    /// Install the last-known catalog (pi's restored `models`, `radius.ts:36-48`). In production
    /// this is what [`crate::remote_catalog::CatalogOverlay::apply`] does by wrapping; the builder
    /// exists so a caller holding a store snapshot can construct the provider already populated.
    #[must_use]
    pub fn with_models(mut self, models: Vec<Model>) -> Self {
        self.rebuild_inner(models);
        self
    }

    /// Attach the [`ModelsStore`] a refresh publishes to. Without one the provider is static.
    #[must_use]
    pub fn with_models_store(mut self, store: Arc<dyn ModelsStore>) -> Self {
        self.models_store = Some(store);
        self
    }

    /// Override the ambient auth context (tests / custom env sources) for BOTH the stream path and
    /// the refresh's key resolution + proxy lookup.
    #[must_use]
    pub fn with_auth_context(mut self, ctx: Arc<dyn AuthContext>) -> Self {
        self.auth_ctx = ctx;
        let models = self.inner.models().to_vec();
        self.rebuild_inner(models);
        self
    }

    /// Per-request budget for the gateway config fetch (default 15s; see the module doc).
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// The normalized gateway every config fetch and OAuth call is made against.
    pub fn gateway(&self) -> &str {
        &self.gateway
    }

    /// `radius.ts:35-78` @v0.84.4, one refresh, minus the in-place `update` (see the module doc):
    ///
    /// 1. `stored` — the persisted entry. Its restore is the overlay loader's job in cyrup, so here
    ///    it only gates the legacy import (`if (!stored && …)`, `:51`).
    /// 2. Legacy import (`:50-65`) — no stored entry AND an OAuth credential carrying a
    ///    pre-`ModelsStore` `gatewayConfig` ⇒ persist that config as the catalog.
    /// 3. `if (!context.allowNetwork || context.signal.aborted) return;` (`:67`).
    /// 4. The key: `credential.type === "oauth" ? credential.access : credential.key` (`:68`),
    ///    where upstream's `credential` is `resolveRefreshCredential`'s output — a stored OAuth
    ///    credential refreshed if expired, else the api-key strategy's env resolution
    ///    (`models.ts:448-474` @v0.84.4). cyrup's [`resolve_provider_auth`] is that same
    ///    resolution, and an OAuth credential's `to_auth` yields its access token as the key.
    /// 5. `loadRadiusGatewayConfig` (`:69`), the post-fetch abort check (`:70`), and the publish
    ///    (`:71-77`).
    async fn refresh_once(job: RefreshJob) -> Result<(), ProviderError> {
        let RefreshJob {
            id,
            gateway,
            auth,
            credentials,
            auth_ctx,
            models_store,
            ctx,
            timeout,
        } = job;
        let stored = models_store.read(id.as_str(), None).await.ok().flatten();

        // 2. `// Import catalogs cached by the pre-ModelsStore Radius implementation.` (`:50`)
        if stored.is_none() {
            let credential = credentials.read(&id).await?;
            if matches!(credential, Some(Credential::Oauth { .. })) {
                let legacy = radius_models(id.as_str(), credential.as_ref());
                if !legacy.is_empty() {
                    let now = now_ms();
                    models_store
                        .write(
                            id.as_str(),
                            ModelsStoreEntry {
                                models: legacy,
                                checked_at: Some(now),
                                last_modified: Some(now),
                                etag: None,
                            },
                            None,
                        )
                        .await?;
                }
            }
        }

        // 3.
        if !ctx.allow_network || ctx.is_aborted() {
            return Ok(());
        }

        // 4. The probe model only carries the identity `ApiKeyAuth::resolve` may read; the env-key
        // strategy radius uses ignores it.
        let probe = Model {
            id: id.as_str().into(),
            name: id.as_str().to_string(),
            api: PI_MESSAGES.into(),
            provider: id.clone(),
            base_url: gateway.clone(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        };
        let api_key = resolve_provider_auth(
            &id,
            &auth,
            &probe,
            credentials.as_ref(),
            auth_ctx.as_ref(),
            AuthOverrides::default(),
        )
        .await?
        .and_then(|result| result.auth.api_key);

        // 5.
        let config = load_radius_gateway_config_with_timeout(
            &gateway,
            api_key.as_deref(),
            auth_ctx.as_ref(),
            &ctx.cancel,
            timeout,
        )
        .await?;
        if ctx.is_aborted() {
            return Ok(());
        }
        let refreshed = radius_models_from_config(id.as_str(), &config);
        let now = now_ms();
        // `[CYRUP-DELTA]` upstream persists `{ models, checkedAt }` only (`:73`) — radius bypasses
        // `withRemoteCatalog`, so nothing upstream ever compares its entry against the built-in
        // catalogs' `generatedAt`. cyrup restores it through the SAME overlay loader as pi.dev,
        // whose staleness guard (`remote_models`, pi #7016) discards an entry with no
        // `lastModified`. The fetch time is therefore recorded as the modification stamp; radius
        // has no embedded rows for the guard to protect, so the guard's intent is preserved and
        // the entry survives it.
        models_store
            .write(
                id.as_str(),
                ModelsStoreEntry {
                    models: refreshed,
                    checked_at: Some(now),
                    last_modified: Some(now),
                    etag: None,
                },
                None,
            )
            .await
    }
}

/// Everything one [`RadiusProvider::refresh_once`] needs, snapshotted so the deduplicated future
/// owns it (`RefreshDedup::run` requires a `'static` future).
struct RefreshJob {
    id: ProviderId,
    gateway: String,
    auth: ProviderAuth,
    credentials: Arc<dyn CredentialStore>,
    auth_ctx: Arc<dyn AuthContext>,
    models_store: Arc<dyn ModelsStore>,
    ctx: RefreshModelsContext,
    timeout: Duration,
}

/// Milliseconds since the Unix epoch (pi `Date.now()`); a pre-epoch clock degrades to `0`.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[async_trait::async_trait]
impl Provider for RadiusProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn base_url(&self) -> Option<&str> {
        self.inner.base_url()
    }

    fn headers(&self) -> Option<&crate::HeaderMap> {
        self.inner.headers()
    }

    fn models(&self) -> &[Model] {
        self.inner.models()
    }

    fn filter_models(&self, models: &[Model], credential: Option<&Credential>) -> Vec<Model> {
        self.inner.filter_models(models, credential)
    }

    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }

    /// `refreshModels` (`radius.ts:35-78`). `None` when no [`ModelsStore`] is attached (static);
    /// otherwise one deduplicated [`RadiusProvider::refresh_once`].
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        let job = RefreshJob {
            id: self.id.clone(),
            gateway: self.gateway.clone(),
            auth: self.auth.clone(),
            credentials: self.credentials.clone(),
            auth_ctx: self.auth_ctx.clone(),
            models_store: self.models_store.clone()?,
            ctx: ctx.clone(),
            timeout: self.request_timeout,
        };
        Some(self.dedup.run(move || Self::refresh_once(job)).await)
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream_simple(model, context, options)
    }
}

/// The built-in `radius` provider (`radiusProvider()` with no options, `all.ts:121` @v0.84.4)
/// over an explicit credential store + shared api registry. The registry MUST provide the
/// `pi-messages` impl (use [`builtin_registry`]).
pub fn radius_provider_with(
    credentials: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> RadiusProvider {
    RadiusProvider::new(RadiusProviderOptions::default(), credentials, registry)
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn radius_provider() -> RadiusProvider {
    radius_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::auth::InMemoryCredentialStore;
    use crate::models_store::InMemoryModelsStore;
    use crate::stream::collect_message;
    use cyrup_core::Content;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// An `AuthContext` over a fixed map: no ambient `RADIUS_API_KEY`, no `HTTP_PROXY`.
    struct MapCtx(BTreeMap<String, String>);

    #[async_trait::async_trait]
    impl AuthContext for MapCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn ctx(pairs: &[(&str, &str)]) -> Arc<dyn AuthContext> {
        Arc::new(MapCtx(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        ))
    }

    /// Serve canned HTTP/1.1 responses off `127.0.0.1:0`, recording every request. Nothing here
    /// may reach a real host.
    async fn serve(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let head = format!(
                    "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn row(id: &str) -> serde_json::Value {
        json!({
            "id": id,
            "name": format!("Model {id}"),
            "reasoning": true,
            "thinkingLevelMap": {"low": "low", "medium": null},
            "input": ["text", "image"],
            "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.1, "cacheWrite": 0.2},
            "contextWindow": 200000,
            "maxTokens": 32000
        })
    }

    const CONFIG_BODY: &str = r#"{"baseUrl":"https://api.example.test/v1","models":[{"id":"radius-1","name":"Radius One","reasoning":true,"input":["text"],"cost":{"input":1,"output":2,"cacheRead":0.1,"cacheWrite":0.2},"contextWindow":128000,"maxTokens":8192},{"id":"broken","name":"no cost"}]}"#;

    // ------------------------------------------------------------------- radius-config.ts --

    /// `isRadiusGatewayModel` (`radius-config.ts:26-40`): each required member's type is checked
    /// and a failing row is FILTERED (`:48`), never fatal.
    #[test]
    fn sanitize_filters_rows_that_fail_the_shape_check_and_keeps_the_rest() {
        let mut bad_reasoning = row("bad-reasoning");
        bad_reasoning["reasoning"] = json!("yes");
        let mut bad_cost = row("bad-cost");
        bad_cost["cost"] = json!([1, 2]);
        let mut bad_input = row("bad-input");
        bad_input["input"] = json!("text");
        let mut missing_max = row("missing-max");
        missing_max.as_object_mut().unwrap().remove("maxTokens");
        let config = json!({
            "baseUrl": "https://gw.example.test/v1",
            "models": [row("ok-1"), bad_reasoning, "not-an-object", bad_cost, bad_input, missing_max, row("ok-2"), []]
        });
        let sanitized = sanitize_radius_gateway_config(&config).expect("valid config");
        assert_eq!(sanitized.base_url, "https://gw.example.test/v1");
        let ids: Vec<&str> = sanitized.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["ok-1", "ok-2"]);
        let ok = &sanitized.models[0];
        assert_eq!(ok.name, "Model ok-1");
        assert!(ok.reasoning);
        assert_eq!(ok.input, vec![Modality::Text, Modality::Image]);
        assert_eq!(ok.context_window, 200_000);
        assert_eq!(ok.max_tokens, 32_000);
        assert_eq!(
            ok.thinking_level_map.as_ref().unwrap().get("medium"),
            Some(&None)
        );
    }

    /// `sanitizeRadiusGatewayConfig` (`:42-50`) returns `undefined` for a non-object, an array, a
    /// missing/non-string `baseUrl` or a non-array `models`.
    #[test]
    fn sanitize_rejects_configs_without_the_two_required_members() {
        for bad in [
            json!(null),
            json!("config"),
            json!([]),
            json!({"models": []}),
            json!({"baseUrl": 42, "models": []}),
            json!({"baseUrl": "https://gw"}),
            json!({"baseUrl": "https://gw", "models": {}}),
        ] {
            assert!(
                sanitize_radius_gateway_config(&bad).is_none(),
                "{bad} must not sanitize"
            );
        }
        // An empty `models` array IS a valid config (`:45` only checks `Array.isArray`).
        let empty = sanitize_radius_gateway_config(&json!({"baseUrl": "https://gw", "models": []}))
            .expect("empty models is valid");
        assert!(empty.models.is_empty());
    }

    /// `getRadiusModelsFromConfig` (`:61-68`): `api: "pi-messages"`, `provider: providerId`,
    /// `baseUrl: config.baseUrl` stamped onto every row, every other member carried through.
    #[test]
    fn models_from_config_stamp_api_provider_and_base_url() {
        let config = sanitize_radius_gateway_config(&json!({
            "baseUrl": "https://gw.example.test/v1",
            "models": [row("a"), row("b")]
        }))
        .unwrap();
        let models = radius_models_from_config("acme-radius", &config);
        assert_eq!(models.len(), 2);
        for (m, id) in models.iter().zip(["a", "b"]) {
            assert_eq!(m.id.as_str(), id);
            assert_eq!(m.api.as_str(), PI_MESSAGES);
            assert_eq!(m.provider.as_str(), "acme-radius");
            assert_eq!(m.base_url, "https://gw.example.test/v1");
            assert_eq!(m.name, format!("Model {id}"));
            assert_eq!(m.cost.input, 1.0);
            assert_eq!(m.cost.cache_write, 0.2);
            assert_eq!(
                m.thinking_level_map.as_ref().unwrap().get("low"),
                Some(&Some("low".to_string()))
            );
            assert!(m.compat.is_none());
        }
    }

    /// `getRadiusModels` (`:70-73`) + `getRadiusCredentialConfig` (`:57-59`): only an OAuth
    /// credential carrying a valid `gatewayConfig` yields models.
    #[test]
    fn legacy_models_come_only_from_an_oauth_credential_gateway_config() {
        let mut ext = serde_json::Map::new();
        ext.insert(
            "gatewayConfig".into(),
            json!({"baseUrl": "https://legacy.example.test", "models": [row("legacy-1")]}),
        );
        let oauth = Credential::Oauth {
            refresh: "r".into(),
            access: "a".into(),
            expires: i64::MAX,
            ext,
        };
        let legacy = radius_models("radius", Some(&oauth));
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].id.as_str(), "legacy-1");
        assert_eq!(legacy[0].base_url, "https://legacy.example.test");
        assert_eq!(legacy[0].provider.as_str(), "radius");

        assert!(radius_models("radius", None).is_empty());
        assert!(radius_models("radius", Some(&Credential::api_key("k"))).is_empty());
        let bare = Credential::Oauth {
            refresh: "r".into(),
            access: "a".into(),
            expires: i64::MAX,
            ext: serde_json::Map::new(),
        };
        assert!(radius_models("radius", Some(&bare)).is_empty());
        let mut invalid = serde_json::Map::new();
        invalid.insert("gatewayConfig".into(), json!({"models": []}));
        let invalid = Credential::Oauth {
            refresh: "r".into(),
            access: "a".into(),
            expires: i64::MAX,
            ext: invalid,
        };
        assert!(radius_models("radius", Some(&invalid)).is_empty());
    }

    /// `truncateHttpBody` (`:75-78`): trim, then 512 chars + `…` — counted in characters.
    #[test]
    fn truncate_http_body_trims_and_caps_at_512_chars() {
        assert_eq!(truncate_http_body("  short  "), "short");
        let exact: String = "é".repeat(512);
        assert_eq!(truncate_http_body(&exact), exact);
        let long: String = "é".repeat(513);
        let truncated = truncate_http_body(&long);
        assert_eq!(truncated.chars().count(), 513);
        assert!(truncated.ends_with('…'));
        assert!(truncated.starts_with(&"é".repeat(512)));
    }

    /// `new URL("/v1/config", gateway)` resolves against the ORIGIN, discarding any path.
    #[test]
    fn config_url_resolves_against_the_gateway_origin() {
        assert_eq!(
            config_url("https://radius.pi.dev"),
            "https://radius.pi.dev/v1/config"
        );
        assert_eq!(
            config_url("https://gw.example.test/some/path"),
            "https://gw.example.test/v1/config"
        );
        assert_eq!(
            config_url("http://127.0.0.1:4567"),
            "http://127.0.0.1:4567/v1/config"
        );
    }

    // ---------------------------------------------------------------- loadRadiusGatewayConfig --

    /// `:85-87` — `accept: application/json`, `authorization: Bearer` only with a key, `GET
    /// /v1/config`; the sanitized config comes back.
    #[tokio::test]
    async fn load_config_sends_pi_shaped_request_and_sanitizes_the_reply() {
        let (base, seen) = serve("HTTP/1.1 200 OK", CONFIG_BODY).await;
        let config = load_radius_gateway_config(
            &base,
            Some("sk-radius"),
            ctx(&[]).as_ref(),
            &CancelToken::new(),
        )
        .await
        .expect("config");
        assert_eq!(config.base_url, "https://api.example.test/v1");
        let ids: Vec<&str> = config.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["radius-1"], "the row without a cost is filtered");

        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        assert!(req.starts_with("GET /v1/config HTTP/1.1"), "{req}");
        let lower = req.to_lowercase();
        assert!(lower.contains("accept: application/json"), "{req}");
        assert!(lower.contains("authorization: bearer sk-radius"), "{req}");
    }

    /// `if (apiKey) headers.authorization = …` (`:86`) — no key, no header.
    #[tokio::test]
    async fn load_config_omits_authorization_without_a_key() {
        let (base, seen) = serve("HTTP/1.1 200 OK", CONFIG_BODY).await;
        load_radius_gateway_config(&base, None, ctx(&[]).as_ref(), &CancelToken::new())
            .await
            .expect("config");
        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        assert!(!req.to_lowercase().contains("authorization:"), "{req}");
    }

    /// `:88-92` — non-OK is `Could not load Radius config from {gateway}: {status}: {body}` with
    /// the body trimmed and capped.
    #[tokio::test]
    async fn load_config_reports_non_ok_status_with_upstreams_message() {
        let (base, _seen) = serve("HTTP/1.1 503 Service Unavailable", "  gateway down  ").await;
        let err = load_radius_gateway_config(&base, None, ctx(&[]).as_ref(), &CancelToken::new())
            .await
            .expect_err("503 is an error");
        match err {
            ProviderError::Http { status, message } => {
                assert_eq!(status, 503);
                assert_eq!(
                    message,
                    format!("Could not load Radius config from {base}: 503: gateway down")
                );
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    /// `:93-94` — a 200 whose body does not sanitize is `Invalid Radius config from {gateway}`.
    #[tokio::test]
    async fn load_config_rejects_a_body_that_does_not_sanitize() {
        for body in [r#"{"models":[]}"#, "not json", "[]"] {
            let (base, _seen) = serve("HTTP/1.1 200 OK", body).await;
            let err =
                load_radius_gateway_config(&base, None, ctx(&[]).as_ref(), &CancelToken::new())
                    .await
                    .expect_err("invalid body");
            assert_eq!(
                err.to_string(),
                format!("decode error: Invalid Radius config from {base}"),
                "body {body:?}"
            );
        }
    }

    /// The abort token is honoured: an already-cancelled refresh never opens a socket.
    #[tokio::test]
    async fn load_config_honours_an_already_cancelled_token() {
        let (base, seen) = serve("HTTP/1.1 200 OK", CONFIG_BODY).await;
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = load_radius_gateway_config(&base, None, ctx(&[]).as_ref(), &cancel)
            .await
            .expect_err("aborted");
        assert!(matches!(err, ProviderError::Aborted), "{err:?}");
        assert!(seen.lock().unwrap().is_empty(), "no request may be sent");
    }

    // --------------------------------------------------------------------------- the provider --

    /// `radius.ts:21-23` defaults and `:30-33` auth: id `radius`, name `Radius`, the default
    /// gateway, an env-key strategy on `RADIUS_API_KEY` and a gateway-bound OAuth flow that is NOT
    /// a subscription (`oauth/radius.ts:357-361` sets no `isSubscription`).
    #[test]
    fn built_in_identity_and_auth_match_upstream() {
        let p = radius_provider();
        assert_eq!(p.id().as_str(), RADIUS_PROVIDER_ID);
        assert_eq!(Provider::name(&p), RADIUS_PROVIDER_NAME);
        assert_eq!(p.gateway(), DEFAULT_RADIUS_GATEWAY);
        assert!(
            p.models().is_empty(),
            "`getRadiusModels(id, undefined)` is `[]`"
        );
        let auth = p.provider_auth().expect("auth clause");
        assert_eq!(
            auth.api_key.as_ref().expect("apiKey").name(),
            RADIUS_API_KEY_AUTH_NAME
        );
        let oauth = auth.oauth.as_ref().expect("oauth");
        assert_eq!(oauth.name(), RADIUS_PROVIDER_NAME);
        assert!(!oauth.is_subscription());
        let vars = crate::env_api_keys::api_key_env_vars(RADIUS_PROVIDER_ID).expect("env map");
        assert_eq!(vars, &[RADIUS_API_KEY_ENV]);
    }

    /// `radiusProvider({ id, name, gateway })` (`:21-23`) — the shape `configureRadiusProviders`
    /// builds for a `models.json` block with `"oauth": "radius"` (`model-runtime.ts:219-233`
    /// @v0.84.4). The gateway is normalized and the OAuth flow signs in under the provider's name.
    #[test]
    fn options_override_id_name_and_gateway() {
        let p = RadiusProvider::new(
            RadiusProviderOptions {
                id: Some("acme".into()),
                name: Some("Acme Gateway".into()),
                gateway: Some("gateway.acme.test/".into()),
            },
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        );
        assert_eq!(p.id().as_str(), "acme");
        assert_eq!(Provider::name(&p), "Acme Gateway");
        assert_eq!(p.gateway(), "https://gateway.acme.test");
        let oauth = p.provider_auth().unwrap().oauth.as_ref().unwrap();
        assert_eq!(oauth.name(), "Acme Gateway");
    }

    /// Without a [`ModelsStore`] the provider is static: `refresh_models` answers `None`, exactly
    /// like a built-in whose upstream definition has no `refreshModels`, and opens no socket.
    #[tokio::test]
    async fn refresh_without_a_store_is_static() {
        let p = radius_provider();
        assert!(
            p.refresh_models(&RefreshModelsContext::default())
                .await
                .is_none()
        );
    }

    /// `radius.ts:67-77` end to end over a loopback gateway: the env key resolves (`:68`), the
    /// config is fetched with it, and the persisted entry carries the stamped models
    /// (`persist: { models: refreshed, checkedAt }`, `:73`) plus the `lastModified` cyrup's
    /// overlay loader requires to restore it.
    #[tokio::test]
    async fn refresh_fetches_the_gateway_config_and_publishes_it_to_the_store() {
        let (base, seen) = serve("HTTP/1.1 200 OK", CONFIG_BODY).await;
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let p = RadiusProvider::new(
            RadiusProviderOptions {
                gateway: Some(base.clone()),
                ..Default::default()
            },
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(ctx(&[("RADIUS_API_KEY", "sk-env")]))
        .with_models_store(store.clone());

        let result = p
            .refresh_models(&RefreshModelsContext::default())
            .await
            .expect("dynamic provider");
        result.expect("refresh ok");

        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        assert!(req.starts_with("GET /v1/config HTTP/1.1"), "{req}");
        assert!(
            req.to_lowercase().contains("authorization: bearer sk-env"),
            "{req}"
        );

        let entry = store
            .read("radius", None)
            .await
            .unwrap()
            .expect("persisted");
        assert_eq!(entry.models.len(), 1);
        assert_eq!(entry.models[0].id.as_str(), "radius-1");
        assert_eq!(entry.models[0].provider.as_str(), "radius");
        assert_eq!(entry.models[0].api.as_str(), PI_MESSAGES);
        assert_eq!(entry.models[0].base_url, "https://api.example.test/v1");
        assert!(entry.checked_at.is_some());
        assert_eq!(entry.last_modified, entry.checked_at);
        assert!(entry.etag.is_none());

        // The published entry restores through the SAME overlay loader pi.dev uses — with the
        // built-in floor stamp installed, so the staleness guard is exercised, not bypassed.
        let overlay = crate::remote_catalog::RemoteCatalog::new(store)
            .with_local_generated_at(crate::providers::builtin_model_data_generated_at())
            .load_overlay(&["radius"])
            .await;
        let restored = overlay.apply(Arc::new(radius_provider()));
        assert_eq!(
            restored.models().len(),
            1,
            "the overlay restores the catalog"
        );
        assert_eq!(
            restored.get_model("radius-1").unwrap().api.as_str(),
            PI_MESSAGES
        );
        assert_eq!(Provider::name(restored.as_ref()), RADIUS_PROVIDER_NAME);
        assert!(restored.provider_auth().is_some_and(|a| a.oauth.is_some()));
    }

    /// `if (!context.allowNetwork || context.signal.aborted) return;` (`:67`) — the cache-only
    /// posture touches no socket and reports success.
    #[tokio::test]
    async fn refresh_without_network_issues_no_request() {
        let (base, seen) = serve("HTTP/1.1 200 OK", CONFIG_BODY).await;
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let p = RadiusProvider::new(
            RadiusProviderOptions {
                gateway: Some(base),
                ..Default::default()
            },
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(ctx(&[]))
        .with_models_store(store.clone());
        p.refresh_models(&RefreshModelsContext::cache_only())
            .await
            .expect("dynamic")
            .expect("ok");
        let cancelled = CancelToken::new();
        cancelled.cancel();
        p.refresh_models(&RefreshModelsContext {
            cancel: cancelled,
            ..Default::default()
        })
        .await
        .expect("dynamic")
        .expect("ok");
        assert!(seen.lock().unwrap().is_empty());
        assert!(store.read("radius", None).await.unwrap().is_none());
    }

    /// `:50-65` — no stored entry + an OAuth credential carrying a legacy `gatewayConfig` ⇒ the
    /// legacy catalog is persisted before (and independently of) the network fetch.
    #[tokio::test]
    async fn refresh_imports_a_legacy_credential_catalog_when_nothing_is_stored() {
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let mut ext = serde_json::Map::new();
        ext.insert(
            "gatewayConfig".into(),
            json!({"baseUrl": "https://legacy.example.test", "models": [row("legacy-1")]}),
        );
        credentials
            .modify(
                &"radius".into(),
                Box::new(move |_| {
                    Box::pin(async move {
                        Ok(Some(Credential::Oauth {
                            refresh: "r".into(),
                            access: "a".into(),
                            expires: i64::MAX,
                            ext,
                        }))
                    })
                }),
            )
            .await
            .unwrap();
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let p = radius_provider_with(credentials, Arc::new(builtin_registry()))
            .with_auth_context(ctx(&[]))
            .with_models_store(store.clone());
        p.refresh_models(&RefreshModelsContext::cache_only())
            .await
            .expect("dynamic")
            .expect("ok");
        let entry = store
            .read("radius", None)
            .await
            .unwrap()
            .expect("legacy persisted");
        assert_eq!(entry.models.len(), 1);
        assert_eq!(entry.models[0].id.as_str(), "legacy-1");
        assert_eq!(entry.models[0].base_url, "https://legacy.example.test");
    }

    /// A gateway failure is the refresh's error, verbatim, and leaves the store untouched.
    #[tokio::test]
    async fn refresh_surfaces_the_gateway_error_and_persists_nothing() {
        let (base, _seen) = serve("HTTP/1.1 500 Internal Server Error", "boom").await;
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let p = RadiusProvider::new(
            RadiusProviderOptions {
                gateway: Some(base.clone()),
                ..Default::default()
            },
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(ctx(&[]))
        .with_models_store(store.clone());
        let err = p
            .refresh_models(&RefreshModelsContext::default())
            .await
            .expect("dynamic")
            .expect_err("500");
        assert_eq!(
            err.to_string(),
            format!("http 500: Could not load Radius config from {base}: 500: boom")
        );
        assert!(store.read("radius", None).await.unwrap().is_none());
    }

    /// The registered provider STREAMS: a gateway model resolved through the built-in registry
    /// goes out over `pi-messages` (`POST {baseUrl}/messages`, bearer from `RADIUS_API_KEY`) and
    /// the reply comes back as the assistant message — PROV-014's verify clause.
    #[tokio::test]
    async fn a_gateway_model_streams_over_pi_messages_with_the_env_key() {
        let (base, seen) = serve(
            "HTTP/1.1 200 OK",
            concat!(
                "data: {\"type\":\"start\"}\n\n",
                "data: {\"type\":\"text_start\",\"contentIndex\":0}\n\n",
                "data: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"text_end\",\"contentIndex\":0,\"content\":\"ok\"}\n\n",
                "data: {\"type\":\"done\",\"reason\":\"stop\",\"usage\":{\"input\":3,\"output\":1,",
                "\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":4,",
                "\"cost\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0,\"total\":0}}}\n\n",
            ),
        )
        .await;
        let config = sanitize_radius_gateway_config(&json!({
            "baseUrl": format!("{base}/v1"),
            "models": [row("radius-1")]
        }))
        .unwrap();
        let models = radius_models_from_config("radius", &config);
        let p = radius_provider()
            .with_auth_context(ctx(&[("RADIUS_API_KEY", "sk-live")]))
            .with_models(models);
        let model = p.get_model("radius-1").expect("catalog row").clone();

        let context = Context {
            system_prompt: None,
            messages: vec![cyrup_core::Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let message = collect_message(p.stream(&model, &context, &StreamOptions::default())).await;
        assert_eq!(message.content, vec![Content::text("ok")]);

        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        assert!(req.starts_with("POST /v1/messages HTTP/1.1"), "{req}");
        assert!(
            req.to_lowercase().contains("authorization: bearer sk-live"),
            "{req}"
        );
    }
}
