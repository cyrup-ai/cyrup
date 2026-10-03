//! the llama.cpp `Provider` and its API-key login (`provider.ts`)
//!
//! Port of `packages/coding-agent/src/extensions/llama/provider.ts` @v0.99.2-17 (EXT-027), minus
//! the pure model mapping that lives in [`crate::model`]. [`LlamaProvider`] is pi's
//! `Provider<"openai-completions">` object (`createLlamaProvider`, `:137-266`): `llama.cpp`, one
//! provider with chat models that stream through the `openai-completions` api against
//! `<server>/v1` and a classifier twin of each that answers through `llama-cpp-classify`
//! against the server root.
//!
//! # Catalog state
//!
//! Pi's `createLlamaProvider` closes over two mutable arrays that `setCatalog` and the refresh's
//! `update` callback reassign (`:138-150`, `:219-222`, `:254-257`). `cyrup_provider::Provider::models`
//! returns a slice, so a [`LlamaProvider`]'s catalog is immutable and is REPLACED, never mutated:
//! [`LlamaController`] builds a fresh provider for the new catalog and hands it to the registration
//! callback it was given, which is how the host swaps a live provider (`register_provider_live`,
//! `cyrup-ext`'s `LateRegistrar`). The controller is pi's `LlamaProviderController`
//! (`:132-135`).
//!
//! # Refresh
//!
//! [`LlamaProvider::refresh`] is `refreshModels` (`:201-259`): it takes pi's
//! `RefreshModelsContext` as [`LlamaRefreshContext`] (credential, stored catalog, `publish`,
//! `allowNetwork`, abort signal). `cyrup_provider::RefreshModelsContext` does not carry the
//! credential, the stored catalog or `publish`, so [`Provider::refresh_models`] gets them from the
//! [`LlamaRefreshHost`] the controller was built with and answers `None` (a static provider, like
//! `RadiusProvider` without a models store) when there is none.
//!
//! Upstream reads the server URL differently in two places: `refreshModels` takes it only from the
//! credential's `env.LLAMA_BASE_URL` (`:228-230`, `credentialServerUrl`, `:24-27`), while `check`
//! and `resolve` also fall back to the `LLAMA_BASE_URL` environment variable
//! (`resolveServerUrl`, `:29-35`). Both behaviours are reproduced as they are: a refresh with no
//! stored URL does nothing even when the environment names a server.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

use cyrup_core::{CancelToken, EventStream, ProviderId};
use cyrup_provider::Provider;
use cyrup_provider::api::llama_cpp_classify::llama_cpp_classify_api;
use cyrup_provider::api::{ApiRegistry, builtin_registry};
use cyrup_provider::auth::oauth::{AuthInteraction, AuthPrompt, OAuthError};
use cyrup_provider::auth::{
    ApiKeyAuth, AuthContext, AuthResult, Credential, CredentialStore, EnvAuthContext, ModelAuth,
    ProviderAuth, ProviderEnv,
};
use cyrup_provider::collection::{AuthCheck, AuthType};
use cyrup_provider::known_api::OPENAI_COMPLETIONS;
use cyrup_provider::stream::{StreamEvent, StreamOptions};
use cyrup_provider::utils::simple_options::SimpleStreamOptions;
use cyrup_provider::{
    AnyModel, AuthError, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult,
    Context, KnownClassifierApi, Model, ProviderClassifier, ProviderError, RefreshModelsContext,
    WireProvider,
};
use futures::future::try_join_all;
use serde::{Deserialize, Serialize};

use crate::client::{
    LlamaClient, LlamaModelInfo, LlamaModelStatus, llama_inference_url, normalize_llama_server_url,
};
use crate::error::LlamaError;
use crate::model::{model_is_selectable, to_classifier_model, to_model};

pub use crate::LLAMA_PROVIDER_ID;

/// The server a fresh login offers (`DEFAULT_LLAMA_SERVER_URL`, `provider.ts:23`).
pub const DEFAULT_LLAMA_SERVER_URL: &str = "http://127.0.0.1:8080";

/// `llamaInferenceUrl(DEFAULT_LLAMA_SERVER_URL)` (`provider.ts:155`), the provider's own base URL.
/// Fixed, because its input is; a test pins that it is what [`llama_inference_url`] answers.
pub(crate) const DEFAULT_LLAMA_BASE_URL: &str = "http://127.0.0.1:8080/v1";

/// The environment variable naming the server (`LLAMA_BASE_URL`, `provider.ts:25`, `:33`, `:163`,
/// `:178`).
pub const LLAMA_BASE_URL_ENV: &str = "LLAMA_BASE_URL";

/// The environment variable holding the API key (`LLAMA_API_KEY`, `provider.ts:190`).
pub const LLAMA_API_KEY_ENV: &str = "LLAMA_API_KEY";

/// The key sent when none is configured (`?? "local"`, `provider.ts:190`).
pub const DEFAULT_LLAMA_API_KEY: &str = "local";

/// The provider's display name (`name: "llama.cpp"`, `provider.ts:154`).
const PROVIDER_NAME: &str = "llama.cpp";

/// The login strategy's display name (`name: "llama.cpp server"`, `provider.ts:158`).
const AUTH_NAME: &str = "llama.cpp server";

// ------------------------------------------------------------------------------------ server url --

/// The server URL a credential stores (`credentialServerUrl`, `provider.ts:24-27`): its
/// `env.LLAMA_BASE_URL` when that is non-blank, normalised.
fn credential_server_url(credential: Option<&Credential>) -> Result<Option<String>, LlamaError> {
    credential
        .and_then(Credential::env)
        .and_then(|env| env.get(LLAMA_BASE_URL_ENV))
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_llama_server_url(value))
        .transpose()
}

/// The server URL for auth (`resolveServerUrl`, `provider.ts:29-35`): the credential's, else the
/// trimmed `LLAMA_BASE_URL` of `ctx`, normalised; `None` when neither names a server.
async fn resolve_server_url(
    ctx: &dyn AuthContext,
    credential: Option<&Credential>,
) -> Result<Option<String>, LlamaError> {
    let configured = match credential_server_url(credential)? {
        Some(url) => Some(url),
        None => ctx
            .env(LLAMA_BASE_URL_ENV)
            .await
            .map(|value| value.trim().to_string()),
    };
    configured
        .filter(|value| !value.is_empty())
        .map(|value| normalize_llama_server_url(&value))
        .transpose()
}

/// Whether the router can load an `unloaded` preset on first use
/// (`routerAutoloadEnabled`, `provider.ts:45-56`).
///
/// Asks `GET /props` only when the catalog holds an unloaded preset (`:50`), requires
/// `models_autoload` to be `true` (`:52`) and reads any failure as "off" (`:53-55`).
async fn router_autoload_enabled(
    client: &LlamaClient,
    catalog: &[LlamaModelInfo],
    cancel: &CancelToken,
) -> bool {
    if !catalog.iter().any(|model| {
        model.status.value == LlamaModelStatus::Unloaded
            && model.source.as_deref() == Some("preset")
    }) {
        return false;
    }
    client
        .props(None, cancel)
        .await
        .is_ok_and(|props| props.models_autoload == Some(true))
}

// ------------------------------------------------------------------------------------------ auth --

/// A process environment variable by name (pi's `process.env`, `provider.ts:163`, `:166`).
pub type ProcessEnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The `llama.cpp server` API-key strategy (`auth.apiKey`, `provider.ts:156-198`): `login` stores
/// the server URL and an optional key, `check` and `resolve` stay dormant until a server is
/// configured.
pub struct LlamaApiKeyAuth {
    process_env: ProcessEnvFn,
}

impl Default for LlamaApiKeyAuth {
    fn default() -> Self {
        Self {
            process_env: Arc::new(|name| std::env::var(name).ok()),
        }
    }
}

impl LlamaApiKeyAuth {
    /// A strategy whose login reads `process_env` where pi reads `process.env` (`provider.ts:163`,
    /// `:166`), so a test can choose the environment the prompt placeholder is built from.
    #[must_use]
    pub fn with_process_env(process_env: ProcessEnvFn) -> Self {
        Self { process_env }
    }
}

/// Fold a [`LlamaError`] into the login failure the TUI prints (the thrown `Error` upstream).
fn login_failure(error: &LlamaError) -> OAuthError {
    OAuthError::Failed(error.to_string())
}

#[async_trait::async_trait]
impl ApiKeyAuth for LlamaApiKeyAuth {
    fn name(&self) -> &str {
        AUTH_NAME
    }

    fn supports_login(&self) -> bool {
        true
    }

    /// `login` (`provider.ts:159-180`): prompt for the server URL (placeholder `LLAMA_BASE_URL` or
    /// the default, `:160-164`), then for an optional key (`:168-173`), check both against the
    /// server with `GET /models` (`:174`) and return
    /// `{ type: "api_key", key: apiKey || undefined, env: { LLAMA_BASE_URL } }` (`:175-179`).
    ///
    /// A blank URL answer falls back to `LLAMA_BASE_URL`, then to the default (`:165-167`).
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let env_url = (self.process_env)(LLAMA_BASE_URL_ENV);
        let mut url_prompt = AuthPrompt::text("llama.cpp server URL");
        url_prompt.placeholder = Some(
            env_url
                .clone()
                .unwrap_or_else(|| DEFAULT_LLAMA_SERVER_URL.to_string()),
        );
        let entered = interaction.prompt(url_prompt).await?;
        let chosen = [entered.trim(), env_url.as_deref().unwrap_or_default()]
            .into_iter()
            .find(|candidate| !candidate.is_empty())
            .unwrap_or(DEFAULT_LLAMA_SERVER_URL);
        let server_url =
            normalize_llama_server_url(chosen).map_err(|error| login_failure(&error))?;

        let entered_key = interaction
            .prompt(AuthPrompt::secret("API key (optional)"))
            .await?;
        let api_key = Some(entered_key.trim().to_string()).filter(|key| !key.is_empty());

        let cancel = interaction.cancel().cloned().unwrap_or_default();
        LlamaClient::new(&server_url, api_key.clone(), None)
            .await
            .map_err(|error| login_failure(&error))?
            .list(false, &cancel)
            .await
            .map_err(|error| login_failure(&error))?;

        let mut env = ProviderEnv::new();
        env.insert(LLAMA_BASE_URL_ENV.to_string(), server_url);
        Ok(Credential::ApiKey {
            key: api_key,
            env: Some(env),
        })
    }

    fn supports_check(&self) -> bool {
        true
    }

    /// `check` (`provider.ts:181-186`): active only when a server URL is known; the source is
    /// `stored credential` for a stored credential and `LLAMA_BASE_URL` for the environment.
    async fn check(
        &self,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        let server_url = resolve_server_url(ctx, cred)
            .await
            .map_err(|error| AuthError::api_key(ProviderId::from(LLAMA_PROVIDER_ID), error))?;
        Ok(server_url.map(|_| AuthCheck {
            auth_type: AuthType::ApiKey,
            source: Some(source_label(cred).to_string()),
        }))
    }

    /// `resolve` (`provider.ts:187-196`): `None` until a server URL is known; otherwise the key is
    /// the credential's, else `LLAMA_API_KEY`, else `local` (`:190`), the base URL is
    /// `<server>/v1` (`:192`) and the env overlay is the credential's with `LLAMA_BASE_URL` set
    /// to the server (`:193`).
    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let fail = |error| AuthError::api_key(ProviderId::from(LLAMA_PROVIDER_ID), error);
        let Some(server_url) = resolve_server_url(ctx, cred).await.map_err(fail)? else {
            return Ok(None);
        };
        let stored_key = match cred {
            Some(Credential::ApiKey { key, .. }) => key.clone(),
            _ => None,
        };
        let api_key = match stored_key {
            Some(key) => key,
            None => ctx
                .env(LLAMA_API_KEY_ENV)
                .await
                .unwrap_or_else(|| DEFAULT_LLAMA_API_KEY.to_string()),
        };
        let mut env: ProviderEnv = cred.and_then(Credential::env).cloned().unwrap_or_default();
        env.insert(LLAMA_BASE_URL_ENV.to_string(), server_url.clone());
        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: Some(api_key),
                headers: None,
                base_url: Some(llama_inference_url(&server_url).map_err(fail)?),
            },
            env: Some(env),
            source: Some(source_label(cred).to_string()),
        }))
    }
}

/// `credential ? "stored credential" : "LLAMA_BASE_URL"` (`provider.ts:184`, `:194`).
fn source_label(credential: Option<&Credential>) -> &'static str {
    if credential.is_some() {
        "stored credential"
    } else {
        LLAMA_BASE_URL_ENV
    }
}

// -------------------------------------------------------------------------------------- refresh --

/// The persisted catalog (pi `ModelsStoreEntry`, `models-store.ts:3-5` @v0.99.2-17, as
/// `refreshModels` writes it, `provider.ts:252-253`): every model of every type in one array, chat
/// models first, then their classifier twins, and the time of the check in milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// Chat models, then classifier models.
    pub models: Vec<AnyModel>,
    /// `Date.now()` when the catalog was read from the server.
    pub checked_at: i64,
}

/// One publication (`ModelsPublication`, `models.ts:63-68`): an optional entry to persist and an
/// optional synchronous update of the provider's in-memory catalog.
///
/// Pi's `persist` also takes `null` for "delete the entry"; `refreshModels` never uses it, so only
/// "leave storage alone" (`None`) and "write this" (`Some`) exist.
pub struct CatalogPublication {
    /// The entry to write; `None` leaves storage unchanged.
    pub persist: Option<CatalogEntry>,
    /// Runs only after the persistence step succeeded, and only if the publication was not
    /// superseded or aborted.
    pub update: Option<Box<dyn FnOnce() + Send>>,
}

/// Generation-checked publication (`RefreshModelsContext.publish`, `models.ts:81-83`).
#[async_trait::async_trait]
pub trait CatalogPublisher: Send + Sync {
    /// Persist `publication.persist`, then run `publication.update`. Answers `false`, with neither
    /// done, when the refresh was superseded or aborted: the caller must stop.
    ///
    /// A persistence failure is REPORTED, not folded into `false`: pi's `context.publish` rejects
    /// when the store write does and the refresh fails with it (`provider.ts:213-222`,
    /// `:251-258`; `ModelsImpl.publishProviderModels`, `models.ts:498-518`), which is how `/llama`
    /// learns the catalog was not saved (`index.ts:61-62`). A host that folded a failed write into
    /// `false` would make the refresh end quietly with nothing installed.
    ///
    /// # Errors
    ///
    /// The host's failure to persist, or an abort of the publication.
    async fn publish(&self, publication: CatalogPublication) -> Result<bool, LlamaError>;
}

/// What a refresh needs from its host: pi's `RefreshModelsContext.credential` and `.stored`
/// (`models.ts:36-39`) plus the publisher.
#[async_trait::async_trait]
pub trait LlamaRefreshHost: CatalogPublisher {
    /// The effective configured credential, if any (`context.credential`).
    async fn credential(&self) -> Option<Credential>;
    /// The persisted catalog (`context.stored`), if one was written.
    async fn stored(&self) -> Option<CatalogEntry>;
}

/// pi's `RefreshModelsContext` (`models.ts:34-53`), as `refreshModels` reads it.
pub struct LlamaRefreshContext<'a> {
    /// `context.credential`: anything but an API-key credential keeps a refresh offline.
    pub credential: Option<&'a Credential>,
    /// `context.stored`: restored before any network access.
    pub stored: Option<&'a CatalogEntry>,
    /// `context.publish`.
    pub publisher: &'a dyn CatalogPublisher,
    /// `context.allowNetwork`: `false` during offline, cache-only initialisation.
    pub allow_network: bool,
    /// `context.signal`.
    pub cancel: &'a CancelToken,
}

/// A refresh failure as the host's error taxonomy: an abort stays an abort.
fn provider_error(error: LlamaError) -> ProviderError {
    match error {
        LlamaError::Cancelled => ProviderError::Aborted,
        other => ProviderError::ModelSource(Box::new(other)),
    }
}

// ------------------------------------------------------------------------------------- provider --

/// What every provider a controller builds shares.
#[derive(Clone)]
struct Parts {
    auth: ProviderAuth,
    credentials: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
    auth_context: Arc<dyn AuthContext>,
    host: Option<Arc<dyn LlamaRefreshHost>>,
}

/// The llama.cpp provider (`createLlamaProvider().provider`, `provider.ts:152-263`).
///
/// Streaming and auth resolution are a [`WireProvider`] over the `openai-completions` api;
/// `classify` is the `llama-cpp-classify` api (`classifier.classify`, `:262`). Every [`Provider`]
/// surface method is delegated by name rather than left to a trait default.
pub struct LlamaProvider {
    id: ProviderId,
    base_url: String,
    models: Vec<Model>,
    classifiers: Vec<ClassifierModel>,
    inner: WireProvider,
    classifier: Arc<dyn ProviderClassifier>,
    parts: Parts,
    controller: Weak<ControllerShared>,
}

impl LlamaProvider {
    fn build(
        parts: &Parts,
        models: Vec<Model>,
        classifiers: Vec<ClassifierModel>,
        controller: Weak<ControllerShared>,
    ) -> Self {
        let id = ProviderId::from(LLAMA_PROVIDER_ID);
        let inner = WireProvider::new(
            id.clone(),
            PROVIDER_NAME,
            Vec::new(),
            parts.auth.clone(),
            parts.credentials.clone(),
            parts.registry.clone(),
        )
        .with_auth_context(parts.auth_context.clone());
        Self {
            id,
            base_url: DEFAULT_LLAMA_BASE_URL.to_string(),
            models,
            classifiers,
            inner,
            classifier: llama_cpp_classify_api(),
            parts: parts.clone(),
            controller,
        }
    }

    /// The classifier twins (`classifiers`, `provider.ts:139`), after the chat models in
    /// [`Provider::get_all_models`].
    #[must_use]
    pub fn classifier_models(&self) -> &[ClassifierModel] {
        &self.classifiers
    }

    /// The `update` callback of a publication: replace the controller's catalog
    /// (`models = …; classifiers = …`, `provider.ts:219-222`, `:254-257`).
    fn catalog_update(
        &self,
        models: Vec<Model>,
        classifiers: Vec<ClassifierModel>,
    ) -> Box<dyn FnOnce() + Send> {
        let controller = self.controller.clone();
        Box::new(move || {
            if let Some(controller) = controller.upgrade() {
                controller.install(models, classifiers);
            }
        })
    }

    /// `refreshModels` (`provider.ts:201-259`).
    ///
    /// 1. Restore: the stored entry's `llama.cpp` chat and classifier models become the catalog
    ///    and seed the per-model cached context windows (`:202-226`); a publication that was
    ///    superseded or aborted ends the refresh (`:216-225`).
    /// 2. Gate: no network, an aborted signal, a credential that is not an API key, or one with no
    ///    `env.LLAMA_BASE_URL` ends it (`:228-230`).
    /// 3. Fetch `GET /models` (`:232`), then check the router's autoload (`:234`) and keep the
    ///    selectable entries (`:236`).
    /// 4. Only `loaded` models are asked for `GET /props?model=<id>&autoload=false`
    ///    (`:237-247`): that is the one read that cannot load or wake anything. Unloaded autoload
    ///    presets would have to be loaded and querying a sleeping model may wake it, so those stay
    ///    unclassified (no thinking support) until a later refresh sees them loaded (`:240-243`).
    /// 5. Publish the chat models followed by the classifiers, persisted with the time of the check,
    ///    and replace the catalog (`:248-258`).
    ///
    /// An abort after each await returns without publishing (`:233`, `:235`, `:251`); a failing
    /// request or a publication whose persistence failed leaves the previous catalog in place and
    /// is returned (`await context.publish(..)` rejects, `:213-222`, `:251-258`).
    ///
    /// # Errors
    ///
    /// A failed `/models` or `/props` request, an unusable server URL, or an abort of one of them.
    pub async fn refresh(&self, ctx: &LlamaRefreshContext<'_>) -> Result<(), LlamaError> {
        let mut cached_context_windows: BTreeMap<String, u64> = BTreeMap::new();
        if let Some(stored) = ctx.stored {
            let mut restored: Vec<Model> = Vec::new();
            let mut restored_classifiers: Vec<ClassifierModel> = Vec::new();
            for model in &stored.models {
                if model.provider().as_str() != LLAMA_PROVIDER_ID {
                    continue;
                }
                match model {
                    AnyModel::Chat(chat) if chat.api.as_str() == OPENAI_COMPLETIONS => {
                        restored.push(chat.clone());
                    }
                    AnyModel::Classifier(classifier)
                        if classifier.api.as_str()
                            == KnownClassifierApi::LlamaCppClassify.as_str() =>
                    {
                        restored_classifiers.push(classifier.clone());
                    }
                    _ => {}
                }
            }
            let windows = restored
                .iter()
                .map(|model| (model.id.as_str(), model.context_window))
                .chain(
                    restored_classifiers
                        .iter()
                        .map(|model| (model.id.as_str(), model.context_window)),
                );
            for (id, context_window) in windows {
                cached_context_windows.insert(id.to_string(), context_window);
            }
            let published = ctx
                .publisher
                .publish(CatalogPublication {
                    persist: None,
                    update: Some(self.catalog_update(restored, restored_classifiers)),
                })
                .await?;
            if !published {
                return Ok(());
            }
        }

        if !ctx.allow_network || ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let Some(Credential::ApiKey { key, .. }) = ctx.credential else {
            return Ok(());
        };
        let Some(server_url) = credential_server_url(ctx.credential)? else {
            return Ok(());
        };
        let env = match ctx.credential {
            Some(Credential::ApiKey { env, .. }) => env.as_ref(),
            _ => None,
        };
        let client = LlamaClient::new(&server_url, key.clone(), env).await?;
        let catalog = client.list(false, ctx.cancel).await?;
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let router_autoload = router_autoload_enabled(&client, &catalog, ctx.cancel).await;
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let selectable: Vec<&LlamaModelInfo> = catalog
            .iter()
            .filter(|model| model_is_selectable(model, router_autoload))
            .collect();
        let refreshed = try_join_all(selectable.iter().map(|model| async {
            let cached = cached_context_windows.get(&model.id).copied();
            if model.status.value != LlamaModelStatus::Loaded {
                return to_model(model, &server_url, None, cached);
            }
            let props = client.props(Some(&model.id), ctx.cancel).await?;
            to_model(model, &server_url, Some(&props), cached)
        }))
        .await?;
        let refreshed_classifiers: Vec<ClassifierModel> = selectable
            .iter()
            .map(|model| {
                to_classifier_model(
                    model,
                    &server_url,
                    cached_context_windows.get(&model.id).copied(),
                )
            })
            .collect();
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let persisted: Vec<AnyModel> = refreshed
            .iter()
            .cloned()
            .map(AnyModel::Chat)
            .chain(
                refreshed_classifiers
                    .iter()
                    .cloned()
                    .map(AnyModel::Classifier),
            )
            .collect();
        ctx.publisher
            .publish(CatalogPublication {
                persist: Some(CatalogEntry {
                    models: persisted,
                    checked_at: cyrup_provider::auth::oauth::now_ms(),
                }),
                update: Some(self.catalog_update(refreshed, refreshed_classifiers)),
            })
            .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Provider for LlamaProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    /// `name: "llama.cpp"` (`provider.ts:154`).
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    /// `baseUrl: llamaInferenceUrl(DEFAULT_LLAMA_SERVER_URL)` (`provider.ts:155`).
    fn base_url(&self) -> Option<&str> {
        Some(&self.base_url)
    }

    /// `getModels` (`provider.ts:199`): the selectable chat models.
    fn models(&self) -> &[Model] {
        &self.models
    }

    /// `getAllModels` (`provider.ts:200`): the chat models, then their classifier twins.
    fn get_all_models(&self) -> Vec<AnyModel> {
        self.models
            .iter()
            .cloned()
            .map(AnyModel::Chat)
            .chain(self.classifiers.iter().cloned().map(AnyModel::Classifier))
            .collect()
    }

    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.parts.auth)
    }

    /// `refreshModels` (`provider.ts:201`): [`LlamaProvider::refresh`] with the credential, stored
    /// catalog and publisher of the [`LlamaRefreshHost`]; `None` (a static provider) without one.
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        let host = self.parts.host.clone()?;
        let credential = host.credential().await;
        let stored = host.stored().await;
        let outcome = self
            .refresh(&LlamaRefreshContext {
                credential: credential.as_ref(),
                stored: stored.as_ref(),
                publisher: host.as_ref(),
                allow_network: ctx.allow_network,
                cancel: &ctx.cancel,
            })
            .await;
        Some(outcome.map_err(provider_error))
    }

    /// `classify` is attached unconditionally (`provider.ts:262`), so the provider supports
    /// classification even while its catalog lists no classifier twin yet.
    fn supports_classification(&self) -> bool {
        true
    }

    /// `classify` (`provider.ts:262`): the `llama-cpp-classify` api.
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        self.classifier.classify(model, context, options).await
    }

    /// `stream` (`provider.ts:260`).
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }

    /// `streamSimple` (`provider.ts:261`).
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream_simple(model, context, options)
    }
}

// ------------------------------------------------------------------------------------ controller --

/// What a controller's `set_catalog` takes beyond the catalog (`{ routerAutoload?: boolean }`,
/// `provider.ts:134`, `:146`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetCatalogOptions {
    /// Whether the router can autoload `unloaded` presets, which makes them selectable.
    pub router_autoload: bool,
}

/// Hands a freshly built provider to the host, replacing the one registered under
/// [`LLAMA_PROVIDER_ID`] (`LateRegistrar::register_provider_live` in `cyrup-ext`). An `Err` is the
/// host's refusal text.
pub type RegisterProviderFn = Arc<dyn Fn(Arc<dyn Provider>) -> Result<(), String> + Send + Sync>;

/// How a [`LlamaController`] is built.
pub struct LlamaControllerOptions {
    credentials: Arc<dyn CredentialStore>,
    register: RegisterProviderFn,
    registry: Option<Arc<ApiRegistry>>,
    auth_context: Option<Arc<dyn AuthContext>>,
    auth: Option<LlamaApiKeyAuth>,
    host: Option<Arc<dyn LlamaRefreshHost>>,
}

impl LlamaControllerOptions {
    /// `credentials` is the store the stream path reads the stored `llama.cpp` credential from
    /// (the agent streams through [`Provider::stream`] directly, so the provider resolves its own
    /// auth): pass the session's credential store. `register` receives every replacement provider.
    #[must_use]
    pub fn new(credentials: Arc<dyn CredentialStore>, register: RegisterProviderFn) -> Self {
        Self {
            credentials,
            register,
            registry: None,
            auth_context: None,
            auth: None,
            host: None,
        }
    }

    /// The api registry streaming dispatches through; it must provide `openai-completions`.
    /// Default: [`builtin_registry`].
    #[must_use]
    pub fn with_registry(mut self, registry: Arc<ApiRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// The ambient auth context. Default: the process environment ([`EnvAuthContext`]).
    #[must_use]
    pub fn with_auth_context(mut self, auth_context: Arc<dyn AuthContext>) -> Self {
        self.auth_context = Some(auth_context);
        self
    }

    /// The login strategy. Default: [`LlamaApiKeyAuth::default`].
    #[must_use]
    pub fn with_auth(mut self, auth: LlamaApiKeyAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Where a refresh gets its credential and stored catalog and publishes to. Without one
    /// [`Provider::refresh_models`] answers `None`.
    #[must_use]
    pub fn with_refresh_host(mut self, host: Arc<dyn LlamaRefreshHost>) -> Self {
        self.host = Some(host);
        self
    }
}

struct ControllerShared {
    parts: Parts,
    register: RegisterProviderFn,
    /// Serialises [`Self::try_install`]: `current` and the provider the host registered are two
    /// copies of one fact, so the write of one and the hand-off of the other must not interleave
    /// with another install's. Upstream has a single pair of closure arrays (`provider.ts:147-149`,
    /// `:254-257`), so last-write-wins is always consistent there.
    install: Mutex<()>,
    current: RwLock<Arc<LlamaProvider>>,
    register_error: RwLock<Option<String>>,
    this: Weak<ControllerShared>,
}

impl ControllerShared {
    fn read_current(&self) -> Arc<LlamaProvider> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_register_error(&self, error: Option<String>) {
        *self
            .register_error
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// Replace the catalog: build the provider for it, make it current and hand it to the host.
    fn try_install(
        &self,
        models: Vec<Model>,
        classifiers: Vec<ClassifierModel>,
    ) -> Result<(), LlamaError> {
        // Held until the host has the provider, so two installs (a `/llama` `set_catalog` and a
        // refresh's `update`) land in the same order in `current` and in the host's registry.
        let _installing = self
            .install
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let provider = Arc::new(LlamaProvider::build(
            &self.parts,
            models,
            classifiers,
            self.this.clone(),
        ));
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = provider.clone();
        let outcome = (self.register)(provider).map_err(LlamaError::Message);
        self.set_register_error(outcome.as_ref().err().map(ToString::to_string));
        outcome
    }

    /// [`Self::try_install`] for a publication's `update`, which cannot fail upstream: a host
    /// refusal is kept for [`LlamaController::register_error`].
    fn install(&self, models: Vec<Model>, classifiers: Vec<ClassifierModel>) {
        let _ = self.try_install(models, classifiers);
    }
}

/// `LlamaProviderController` (`provider.ts:132-135`): the provider and the way to replace its
/// catalog.
///
/// Cheap to clone; every clone drives the same provider.
#[derive(Clone)]
pub struct LlamaController {
    shared: Arc<ControllerShared>,
}

impl LlamaController {
    /// `createLlamaProvider()` (`provider.ts:137-266`): a provider with no models. It is not
    /// registered; register [`LlamaController::provider`] at extension init, and every later
    /// catalog change reaches the host through `options`' register callback.
    #[must_use]
    pub fn new(options: LlamaControllerOptions) -> Self {
        let parts = Parts {
            auth: ProviderAuth::with_api_key(Arc::new(options.auth.unwrap_or_default())),
            credentials: options.credentials,
            registry: options
                .registry
                .unwrap_or_else(|| Arc::new(builtin_registry())),
            auth_context: options
                .auth_context
                .unwrap_or_else(|| Arc::new(EnvAuthContext)),
            host: options.host,
        };
        let register = options.register;
        let shared = Arc::new_cyclic(|this: &Weak<ControllerShared>| ControllerShared {
            current: RwLock::new(Arc::new(LlamaProvider::build(
                &parts,
                Vec::new(),
                Vec::new(),
                this.clone(),
            ))),
            parts,
            register,
            install: Mutex::new(()),
            register_error: RwLock::new(None),
            this: this.clone(),
        });
        Self { shared }
    }

    /// The provider (`controller.provider`, `provider.ts:133`) for the catalog as it stands. A
    /// catalog change makes a NEW provider, so read this again rather than keeping the handle.
    #[must_use]
    pub fn provider(&self) -> Arc<LlamaProvider> {
        self.shared.read_current()
    }

    /// `setCatalog` (`provider.ts:142-150`): offer the selectable entries of a router catalog as
    /// chat models and classifier twins (no props are read, so none claims reasoning support), and
    /// register the resulting provider with the host.
    ///
    /// # Errors
    ///
    /// The URL errors of [`llama_inference_url`] for a bad `server_url` (nothing changes), or the
    /// host's refusal of the new provider (the catalog has changed, the host still holds the old
    /// provider).
    pub fn set_catalog(
        &self,
        catalog: &[LlamaModelInfo],
        server_url: &str,
        options: SetCatalogOptions,
    ) -> Result<(), LlamaError> {
        let selectable: Vec<&LlamaModelInfo> = catalog
            .iter()
            .filter(|model| model_is_selectable(model, options.router_autoload))
            .collect();
        let models = selectable
            .iter()
            .map(|model| to_model(model, server_url, None, None))
            .collect::<Result<Vec<_>, _>>()?;
        let classifiers = selectable
            .iter()
            .map(|model| to_classifier_model(model, server_url, None))
            .collect();
        self.shared.try_install(models, classifiers)
    }

    /// The host's last refusal of a provider this controller built, if the latest registration
    /// failed. A refresh's `update` cannot return an error upstream, so a refusal there is kept
    /// here instead of being dropped.
    #[must_use]
    pub fn register_error(&self) -> Option<String> {
        self.shared
            .register_error
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
