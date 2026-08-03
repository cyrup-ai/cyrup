//! The `Models` runtime collection (1:1 port of Pi `packages/ai/src/models.ts`).
//!
//! A registry of [`Provider`]s plus auth application and stream convenience. Providers own stream
//! behavior; `Models` resolves request auth against its own credential store + auth context and
//! delegates each request to the provider that owns the model (`createModels`, `setProvider`,
//! `getProvider(s)`, `getModel(s)`, `getAuth`, `applyAuth`, `stream`/`complete`, `refresh`). Also
//! ports the model-capability helpers (`getSupportedThinkingLevels`, `clampThinkingLevel`,
//! `modelsAreEqual`, `hasApi`).

use crate::api::channel;
use crate::auth::{
    AuthContext, AuthOverrides, AuthResult, CredentialStore, EnvAuthContext,
    InMemoryCredentialStore, resolve_provider_auth,
};
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions, collect_message};
use crate::utils::simple_options::SimpleStreamOptions;
use cyrup_core::{AssistantMessage, EventStream, ModelThinkingLevel};
use futures::StreamExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

/// Channel buffer for the collection's lazy auth-applying stream bridge.
const STREAM_BUFFER: usize = 64;

/// Options for [`create_models`] (Pi `CreateModelsOptions`). All default: an empty in-memory
/// credential store, the real-env auth context, and no remote catalog overlay.
#[derive(Clone, Default)]
pub struct CreateModelsOptions {
    pub credentials: Option<Arc<dyn CredentialStore>>,
    pub auth_context: Option<Arc<dyn AuthContext>>,
    /// The persisted pi.dev model-catalog overlay to merge over the compiled-in catalogs
    /// (DRIFT-007; Pi threads its equivalent through `ModelRuntime`'s `modelsStore` +
    /// `withRemoteCatalog`, `model-runtime.ts:139-151`).
    ///
    /// `None` — the default and what every pre-DRIFT-007 caller gets — means "embedded catalogs
    /// only", which is exactly today's behavior. The overlay can only ADD or REPLACE models by id
    /// (see [`crate::remote_catalog::merge_models`]); it can never remove one, so a failed, disabled
    /// or offline refresh is indistinguishable from `None`.
    pub catalog_overlay: Option<Arc<crate::remote_catalog::CatalogOverlay>>,
}

/// Runtime collection of providers plus auth application + stream convenience (Pi `MutableModels`).
/// Providers are held by id (ids are unique; `set_provider` upserts).
pub struct Models {
    providers: BTreeMap<String, Arc<dyn Provider>>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
}

/// Build an empty collection (Pi `createModels`).
pub fn create_models(options: CreateModelsOptions) -> Models {
    Models {
        providers: BTreeMap::new(),
        credentials: options
            .credentials
            .unwrap_or_else(|| Arc::new(InMemoryCredentialStore::new())),
        auth_context: options
            .auth_context
            .unwrap_or_else(|| Arc::new(EnvAuthContext)),
    }
}

impl Models {
    // ---- MutableModels (Pi models.ts:130) ----

    /// Upsert/replace by `provider.id` (Pi `setProvider`). Ids are unique.
    pub fn set_provider(&mut self, provider: Arc<dyn Provider>) {
        self.providers
            .insert(provider.id().as_str().to_string(), provider);
    }

    /// Remove a provider by id (Pi `deleteProvider`).
    pub fn delete_provider(&mut self, id: &str) {
        self.providers.remove(id);
    }

    /// Remove every provider (Pi `clearProviders`).
    pub fn clear_providers(&mut self) {
        self.providers.clear();
    }

    // ---- reads (Pi models.ts:164) ----

    /// All registered providers (Pi `getProviders`).
    pub fn get_providers(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.values().cloned().collect()
    }

    /// One provider by id (Pi `getProvider`).
    pub fn get_provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(id).cloned()
    }

    /// Last-known models from one provider, or all providers (Pi `getModels`). Best-effort: our
    /// `Provider::models()` is infallible (it cannot throw), matching Pi's catch-and-skip contract.
    pub fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
        match provider {
            Some(id) => self
                .providers
                .get(id)
                .map(|p| p.models().to_vec())
                .unwrap_or_default(),
            None => self
                .providers
                .values()
                .flat_map(|p| p.models().to_vec())
                .collect(),
        }
    }

    /// Runtime model lookup against last-known lists (Pi `getModel`).
    pub fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.get_models(Some(provider))
            .into_iter()
            .find(|m| m.id.as_str() == id)
    }

    // ---- auth (Pi models.ts:216) ----

    /// Resolve request auth for a model (Pi `getAuth`). `Ok(None)` when the provider is unknown or
    /// unconfigured; `Err` carries the R-01-017 taxonomy (e.g. `oauth` on a failed token refresh —
    /// the stored credential is preserved for re-login).
    pub async fn get_auth(&self, model: &Model) -> Result<Option<AuthResult>, ProviderError> {
        let Some(provider) = self.providers.get(model.provider.as_str()) else {
            return Ok(None);
        };
        let Some(auth) = provider.provider_auth() else {
            return Ok(None);
        };
        Ok(resolve_provider_auth(
            &model.provider,
            auth,
            model,
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            AuthOverrides::default(),
        )
        .await?)
    }

    // ---- stream / complete (Pi models.ts:258). applyAuth lives on [`AuthHelper`] so the spawned
    // stream task can own its inputs (Pi `applyAuth`, models.ts:230). ----

    /// Stream a model through the owning provider, applying auth first (Pi `stream`). Returns
    /// immediately; auth application happens behind the stream and any failure arrives as a terminal
    /// [`StreamEvent::Error`] (never thrown).
    pub fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (sink, rx) = channel(STREAM_BUFFER);

        let provider = self.providers.get(model.provider.as_str()).cloned();
        let credentials = self.credentials.clone();
        let auth_context = self.auth_context.clone();
        let provider_id = model.provider.clone();
        let model_owned = model.clone();
        let context = context.clone();
        let options = options.clone();

        tokio::spawn(async move {
            let model_id = model_owned.id.as_str().to_string();
            let api = Some(model_owned.api.clone());

            let Some(provider) = provider else {
                let err = ProviderError::UnknownProvider(provider_id.clone());
                sink.send(err.into_error_event(provider_id, &model_id, api))
                    .await;
                return;
            };

            // applyAuth (re-resolves against the collection store/ctx).
            let helper = AuthHelper {
                provider: provider.clone(),
                credentials,
                auth_context,
            };
            let (request_model, request_options) =
                match helper.apply_auth(&model_owned, &options).await {
                    Ok(v) => v,
                    Err(e) => {
                        sink.send(e.into_error_event(provider_id, &model_id, api))
                            .await;
                        return;
                    }
                };

            // Delegate to the provider and forward every event.
            let mut stream = provider.stream(&request_model, &context, &request_options);
            while let Some(event) = stream.next().await {
                if !sink.send(event).await {
                    break;
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
    }

    /// Stream to completion (Pi `complete`): drive the stream and fold into the terminal message.
    pub async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> AssistantMessage {
        collect_message(self.stream(model, context, options)).await
    }

    /// Stream a model through the owning provider with the unified "simple" option surface,
    /// applying auth first (Pi `streamSimple`, models.ts:278). Like [`Models::stream`], returns
    /// immediately and surfaces every failure (unknown provider, auth) as a terminal
    /// [`StreamEvent::Error`]. Auth is applied to the wrapped [`StreamOptions`] (`base`) exactly as
    /// for `stream`; the unified `reasoning`/`thinking_budgets` ride through untouched to the
    /// provider's [`Provider::stream_simple`].
    pub fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        let (sink, rx) = channel(STREAM_BUFFER);

        let provider = self.providers.get(model.provider.as_str()).cloned();
        let credentials = self.credentials.clone();
        let auth_context = self.auth_context.clone();
        let provider_id = model.provider.clone();
        let model_owned = model.clone();
        let context = context.clone();
        let options = options.clone();

        tokio::spawn(async move {
            let model_id = model_owned.id.as_str().to_string();
            let api = Some(model_owned.api.clone());

            let Some(provider) = provider else {
                let err = ProviderError::UnknownProvider(provider_id.clone());
                sink.send(err.into_error_event(provider_id, &model_id, api))
                    .await;
                return;
            };

            // applyAuth on the wrapped StreamOptions (Pi `applyAuth<SimpleStreamOptions>`).
            let helper = AuthHelper {
                provider: provider.clone(),
                credentials,
                auth_context,
            };
            let (request_model, request_base) =
                match helper.apply_auth(&model_owned, &options.base).await {
                    Ok(v) => v,
                    Err(e) => {
                        sink.send(e.into_error_event(provider_id, &model_id, api))
                            .await;
                        return;
                    }
                };
            let request_options = SimpleStreamOptions {
                base: request_base,
                reasoning: options.reasoning,
                thinking_budgets: options.thinking_budgets,
            };

            let mut stream = provider.stream_simple(&request_model, &context, &request_options);
            while let Some(event) = stream.next().await {
                if !sink.send(event).await {
                    break;
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
    }

    /// Stream to completion with the unified "simple" options (Pi `completeSimple`, models.ts:286):
    /// drive [`Models::stream_simple`] and fold into the terminal message.
    pub async fn complete_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> AssistantMessage {
        collect_message(self.stream_simple(model, context, options)).await
    }

    /// Ask dynamic providers to re-fetch their model lists (1:1 port of Pi `refresh`,
    /// models.ts:198-214).
    ///
    /// With a provider id: a static provider (no [`Provider::refresh_models`] source → `None`) is a
    /// no-op (`Ok(())`); a dynamic provider's fetch failure is surfaced as a `model_source`
    /// [`ProviderError`] (Pi wraps the cause in `ModelsError("model_source", …)`; an error that is
    /// already a `model_source` error is re-raised unchanged, mirroring Pi's
    /// `if (error instanceof ModelsError) throw error`).
    ///
    /// Without a provider id: every provider is refreshed concurrently, best-effort — failures are
    /// swallowed (Pi `Promise.allSettled`). Static providers are no-ops.
    pub async fn refresh(&self, provider: Option<&str>) -> Result<(), ProviderError> {
        if let Some(id) = provider {
            let Some(entry) = self.providers.get(id) else {
                // Unknown provider: no refresh source → no-op (Pi `if (!entry?.refreshModels) return`).
                return Ok(());
            };
            return match entry.refresh_models().await {
                None => Ok(()),
                Some(Ok(())) => Ok(()),
                Some(Err(e @ ProviderError::ModelSource(_))) => Err(e),
                Some(Err(e)) => Err(ProviderError::ModelSource(
                    format!("Model refresh failed for {id}: {e}").into(),
                )),
            };
        }

        // Best-effort: refresh every provider concurrently, ignoring failures (Pi `allSettled`).
        let refreshes = self.providers.values().map(|p| p.refresh_models());
        futures::future::join_all(refreshes).await;
        Ok(())
    }
}

/// A snapshot of the data `apply_auth` needs, captured for the spawned stream task.
struct AuthHelper {
    provider: Arc<dyn Provider>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
}

impl AuthHelper {
    async fn apply_auth(
        &self,
        model: &Model,
        options: &StreamOptions,
    ) -> Result<(Model, StreamOptions), ProviderError> {
        let Some(auth_strategy) = self.provider.provider_auth() else {
            return Ok((model.clone(), options.clone()));
        };
        let resolution = resolve_provider_auth(
            &model.provider,
            auth_strategy,
            model,
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            AuthOverrides {
                api_key: options.api_key.as_deref(),
                env: None,
            },
        )
        .await?;
        let Some(resolution) = resolution else {
            return Ok((model.clone(), options.clone()));
        };
        let auth = &resolution.auth;
        let mut request_model = model.clone();
        if let Some(base_url) = &auth.base_url {
            request_model.base_url = base_url.clone();
        }
        let mut request_options = options.clone();
        request_options.api_key = options.api_key.clone().or_else(|| auth.api_key.clone());
        request_options.headers = merge_headers(auth.headers.as_ref(), options.headers.as_ref());
        Ok((request_model, request_options))
    }
}

/// Merge auth headers with request headers; request headers win per key (Pi `applyAuth` header
/// spread `{ ...auth.headers, ...options.headers }`). `None` when neither side has any.
fn merge_headers(
    auth_headers: Option<&crate::HeaderMap>,
    option_headers: Option<&crate::HeaderMap>,
) -> Option<crate::HeaderMap> {
    if auth_headers.is_none() && option_headers.is_none() {
        return None;
    }
    let mut merged = crate::HeaderMap::new();
    if let Some(h) = auth_headers {
        for (k, v) in h {
            merged.insert(k.clone(), v.clone());
        }
    }
    if let Some(h) = option_headers {
        for (k, v) in h {
            merged.insert(k.clone(), v.clone());
        }
    }
    Some(merged)
}

// ---- model capability helpers (Pi models.ts:397) ----

/// The full ordered extended-thinking ladder (Pi `EXTENDED_THINKING_LEVELS`).
pub const EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
    ModelThinkingLevel::Off,
    ModelThinkingLevel::Minimal,
    ModelThinkingLevel::Low,
    ModelThinkingLevel::Medium,
    ModelThinkingLevel::High,
    ModelThinkingLevel::Xhigh,
    ModelThinkingLevel::Max,
];

fn level_key(level: ModelThinkingLevel) -> &'static str {
    crate::api::compat::thinking_level_key(level)
}

/// The thinking levels a model supports (Pi `getSupportedThinkingLevels`). A non-reasoning model
/// supports only `off`. A `thinkingLevelMap` value of `null` marks a level unsupported; `xhigh` and
/// `max` additionally require an explicit (non-`undefined`) map entry (Pi models.ts:670).
pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
    if !model.reasoning {
        return vec![ModelThinkingLevel::Off];
    }
    EXTENDED_THINKING_LEVELS
        .iter()
        .copied()
        .filter(|level| {
            let mapped = model
                .thinking_level_map
                .as_ref()
                .and_then(|m| m.get(level_key(*level)));
            match mapped {
                Some(None) => false, // explicit null → unsupported
                Some(Some(_)) => true,
                // undefined: `xhigh`/`max` require an entry, every other rung is implicit.
                None => !matches!(
                    *level,
                    ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max
                ),
            }
        })
        .collect()
}

/// Clamp a requested thinking level to one the model supports (Pi `clampThinkingLevel`): prefer the
/// requested level, else the nearest higher supported level, else the nearest lower, else the first
/// supported (or `off`).
pub fn clamp_thinking_level(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
    let available = get_supported_thinking_levels(model);
    if available.contains(&level) {
        return level;
    }
    let Some(requested_index) = EXTENDED_THINKING_LEVELS.iter().position(|l| *l == level) else {
        return available
            .first()
            .copied()
            .unwrap_or(ModelThinkingLevel::Off);
    };
    for candidate in EXTENDED_THINKING_LEVELS.iter().skip(requested_index) {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    for candidate in EXTENDED_THINKING_LEVELS.iter().take(requested_index).rev() {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    available
        .first()
        .copied()
        .unwrap_or(ModelThinkingLevel::Off)
}

/// Two models are equal iff their id AND provider match (Pi `modelsAreEqual`).
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

/// Runtime-checked narrowing for a dynamically looked-up model's wire api (Pi `hasApi`).
pub fn has_api(model: &Model, api: &str) -> bool {
    model.api.as_str() == api
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
    use crate::auth::types::Credential;
    use crate::known_api::OPENAI_COMPLETIONS;
    use crate::model::{Modality, ModelCost, ThinkingLevelMap};
    use crate::providers::fleet;
    use cyrup_core::{ProviderId, StopReason};
    use std::collections::BTreeMap;

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

    fn model(provider: &str, id: &str, reasoning: bool, map: Option<ThinkingLevelMap>) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: OPENAI_COMPLETIONS.into(),
            provider: provider.into(),
            base_url: "https://example.test/v1".into(),
            reasoning,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 1000,
            thinking_level_map: map,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn set_get_delete_clear_providers() {
        let mut models = create_models(CreateModelsOptions::default());
        assert!(models.get_providers().is_empty());

        models.set_provider(Arc::new(fleet::GROQ.provider()));
        models.set_provider(Arc::new(fleet::XAI.provider()));
        assert_eq!(models.get_providers().len(), 2);
        assert!(models.get_provider("groq").is_some());

        // Upsert by id is idempotent (replace, not append).
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        assert_eq!(models.get_providers().len(), 2);

        // Cross-provider getModels aggregates; provider-scoped narrows.
        let all = models.get_models(None);
        let groq_only = models.get_models(Some("groq"));
        assert!(all.len() > groq_only.len());
        assert!(groq_only.iter().all(|m| m.provider.as_str() == "groq"));
        assert!(
            models
                .get_model("xai", &groq_only[0].id.clone().to_string())
                .is_none()
        );

        models.delete_provider("groq");
        assert!(models.get_provider("groq").is_none());
        assert_eq!(models.get_providers().len(), 1);

        models.clear_providers();
        assert!(models.get_providers().is_empty());
    }

    #[tokio::test]
    async fn get_auth_resolves_env_key_via_collection_store() {
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "sk-groq".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("a groq model");
        let auth = models.get_auth(&m).await.expect("ok").expect("configured");
        assert_eq!(auth.auth.api_key.as_deref(), Some("sk-groq"));
        assert_eq!(auth.source.as_deref(), Some("env"));

        // Unknown provider → Ok(None).
        let unknown = model("nope", "x", false, None);
        assert!(models.get_auth(&unknown).await.expect("ok").is_none());
    }

    #[tokio::test]
    async fn get_auth_prefers_stored_credential() {
        let store = crate::auth::InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("groq"), Credential::api_key("stored-key"));
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::new(store)),
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "env-key".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");
        let auth = models.get_auth(&m).await.expect("ok").expect("configured");
        // Stored credential owns the provider (env not consulted, R-01-012).
        assert_eq!(auth.auth.api_key.as_deref(), Some("stored-key"));
        assert_eq!(auth.source.as_deref(), Some("stored"));
    }

    #[tokio::test]
    async fn stream_unknown_provider_yields_error_terminal() {
        let models = create_models(CreateModelsOptions::default());
        let m = model("ghost", "m1", false, None);
        let msg =
            collect_message(models.stream(&m, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("unknown provider"));
    }

    #[tokio::test]
    async fn stream_applies_auth_then_delegates() {
        // Configured env key → auth applies; an unroutable base URL proves we reached transport
        // (an unconfigured provider would short-circuit before the request).
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "sk-groq".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let mut m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");
        m.base_url = "http://127.0.0.1:1/v1".to_string();
        let msg =
            collect_message(models.stream(&m, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved: {err}"
        );
        assert!(err.contains("transport"), "expected transport error: {err}");
    }

    #[tokio::test]
    async fn stream_simple_unknown_provider_yields_error_terminal() {
        let models = create_models(CreateModelsOptions::default());
        let m = model("ghost", "m1", false, None);
        let opts = SimpleStreamOptions::default();
        let msg = collect_message(models.stream_simple(&m, &Context::default(), &opts)).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("unknown provider"));
    }

    #[tokio::test]
    async fn stream_simple_applies_auth_then_delegates() {
        // Mirrors `stream_applies_auth_then_delegates` for the simple path: a configured env key
        // resolves, and the unroutable base URL proves we reached transport (Pi `streamSimple`
        // applies auth identically to `stream`).
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "sk-groq".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let mut m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");
        m.base_url = "http://127.0.0.1:1/v1".to_string();
        let opts = SimpleStreamOptions {
            reasoning: Some(cyrup_core::ThinkingLevel::Low),
            ..Default::default()
        };
        let msg = collect_message(models.stream_simple(&m, &Context::default(), &opts)).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved: {err}"
        );
        assert!(err.contains("transport"), "expected transport error: {err}");
    }

    #[tokio::test]
    async fn complete_simple_folds_to_terminal_message() {
        let models = create_models(CreateModelsOptions::default());
        let m = model("ghost", "m1", false, None);
        let opts = SimpleStreamOptions::default();
        let msg = models.complete_simple(&m, &Context::default(), &opts).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("unknown provider"));
    }

    #[test]
    fn supported_thinking_levels_match_pi() {
        // Non-reasoning → only off.
        let plain = model("p", "a", false, None);
        assert_eq!(
            get_supported_thinking_levels(&plain),
            vec![ModelThinkingLevel::Off]
        );

        // Reasoning, no map → off,minimal,low,medium,high (xhigh AND max each require an explicit
        // entry — Pi models.ts:670 `if (level === "xhigh" || level === "max")`).
        let r = model("p", "b", true, None);
        assert_eq!(
            get_supported_thinking_levels(&r),
            vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Minimal,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
            ]
        );

        // null entry removes a level; explicit xhigh/max entries add them.
        let mut map = ThinkingLevelMap::new();
        map.insert("low".to_string(), None); // unsupported
        map.insert("xhigh".to_string(), Some("xhigh".to_string())); // enabled
        map.insert("max".to_string(), Some("max".to_string())); // enabled
        let r2 = model("p", "c", true, Some(map));
        let levels = get_supported_thinking_levels(&r2);
        assert!(!levels.contains(&ModelThinkingLevel::Low));
        assert!(levels.contains(&ModelThinkingLevel::Xhigh));
        assert!(levels.contains(&ModelThinkingLevel::Max));
        // `max` is opt-in exactly like `xhigh`: an explicit null keeps it off the ladder even
        // though every lower rung is implicitly on.
        let mut null_max = ThinkingLevelMap::new();
        null_max.insert("max".to_string(), None);
        let r3 = model("p", "d", true, Some(null_max));
        assert!(!get_supported_thinking_levels(&r3).contains(&ModelThinkingLevel::Max));
    }

    #[test]
    fn clamp_thinking_level_walks_up_then_down() {
        // Map removes medium+high; request medium → clamps up past the gap.
        let mut map = ThinkingLevelMap::new();
        map.insert("medium".to_string(), None);
        map.insert("high".to_string(), None);
        map.insert("xhigh".to_string(), Some("xhigh".to_string()));
        let m = model("p", "c", true, Some(map));
        // medium unsupported → nearest higher supported is xhigh.
        assert_eq!(
            clamp_thinking_level(&m, ModelThinkingLevel::Medium),
            ModelThinkingLevel::Xhigh
        );
        // `max` is above every supported rung → the upward walk finds nothing and the downward
        // walk lands on xhigh (Pi models.ts:688-691).
        assert_eq!(
            clamp_thinking_level(&m, ModelThinkingLevel::Max),
            ModelThinkingLevel::Xhigh
        );

        // The mirror case, and the one the corrected `claude-opus-4-6` catalog produces: `max` is
        // the ONLY top rung, so a request for `xhigh` must promote UP to it rather than fall to
        // `high`. This only works because `Max` is declared after `Xhigh` in the ladder.
        let mut only_max = ThinkingLevelMap::new();
        only_max.insert("max".to_string(), Some("max".to_string()));
        let m_max = model("p", "e", true, Some(only_max));
        assert_eq!(
            clamp_thinking_level(&m_max, ModelThinkingLevel::Xhigh),
            ModelThinkingLevel::Max
        );

        // Non-reasoning model clamps everything to off.
        let plain = model("p", "a", false, None);
        assert_eq!(
            clamp_thinking_level(&plain, ModelThinkingLevel::High),
            ModelThinkingLevel::Off
        );
    }

    #[test]
    fn models_equal_and_has_api() {
        let a = model("p", "m", false, None);
        let b = model("p", "m", true, None); // same id+provider, differing fields
        let c = model("q", "m", false, None);
        assert!(models_are_equal(Some(&a), Some(&b)));
        assert!(!models_are_equal(Some(&a), Some(&c)));
        assert!(!models_are_equal(Some(&a), None));
        assert!(has_api(&a, OPENAI_COMPLETIONS));
        assert!(!has_api(&a, "anthropic-messages"));
    }

    // ---- dynamic-refresh dispatch (Pi `Models.refresh`, models.ts:198-214) ----

    use crate::utils::refresh::RefreshDedup;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A dynamic provider whose `refresh_models` counts fetches (deduplicated via [`RefreshDedup`])
    /// and optionally fails. `stream()` is unused here and yields a terminal error immediately.
    struct DynProvider {
        id: cyrup_core::ProviderId,
        models: Vec<Model>,
        fetches: Arc<AtomicUsize>,
        fail: bool,
        dedup: RefreshDedup,
    }

    #[async_trait::async_trait]
    impl Provider for DynProvider {
        fn id(&self) -> &cyrup_core::ProviderId {
            &self.id
        }
        fn models(&self) -> &[Model] {
            &self.models
        }
        fn stream(
            &self,
            model: &Model,
            _context: &Context,
            _options: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            let msg = cyrup_core::AssistantMessage::errored(
                model.provider.clone(),
                model.id.as_str(),
                Some(model.api.clone()),
                cyrup_core::StopReason::Error,
                "unused",
            );
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::terminal(msg)).await;
            });
            Box::pin(ReceiverStream::new(rx))
        }
        async fn refresh_models(&self) -> Option<Result<(), ProviderError>> {
            let fetches = self.fetches.clone();
            let fail = self.fail;
            Some(
                self.dedup
                    .run(move || async move {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        if fail {
                            Err(ProviderError::Transport("network down".into()))
                        } else {
                            Ok(())
                        }
                    })
                    .await,
            )
        }
    }

    fn dyn_provider(id: &str, fail: bool) -> (Arc<DynProvider>, Arc<AtomicUsize>) {
        let fetches = Arc::new(AtomicUsize::new(0));
        let p = Arc::new(DynProvider {
            id: cyrup_core::ProviderId::from(id),
            models: vec![model(id, "m1", false, None)],
            fetches: fetches.clone(),
            fail,
            dedup: RefreshDedup::new(),
        });
        (p, fetches)
    }

    #[tokio::test]
    async fn refresh_static_provider_is_noop() {
        // A fleet provider has no dynamic source (`refresh_models` → None): refresh is a clean Ok.
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        models
            .refresh(Some("groq"))
            .await
            .expect("static refresh is a no-op");
        // Unknown provider id is also a no-op (Pi `if (!entry?.refreshModels) return`).
        models
            .refresh(Some("does-not-exist"))
            .await
            .expect("unknown is a no-op");
    }

    #[tokio::test]
    async fn refresh_dynamic_provider_calls_fetch() {
        let mut models = create_models(CreateModelsOptions::default());
        let (p, fetches) = dyn_provider("dyn-ok", false);
        models.set_provider(p);
        models.refresh(Some("dyn-ok")).await.expect("ok");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_dynamic_failure_is_model_source() {
        let mut models = create_models(CreateModelsOptions::default());
        let (p, fetches) = dyn_provider("dyn-bad", true);
        models.set_provider(p);
        let err = models
            .refresh(Some("dyn-bad"))
            .await
            .expect_err("should fail");
        assert_eq!(err.code(), "model_source");
        assert!(err.to_string().contains("Model refresh failed for dyn-bad"));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_all_is_best_effort_and_swallows_failures() {
        // A mix of a static provider, a failing dynamic provider, and a healthy dynamic provider:
        // refresh(None) must call every dynamic source and never error (Pi `allSettled`).
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(fleet::GROQ.provider())); // static
        let (bad, bad_fetches) = dyn_provider("dyn-bad", true);
        let (ok, ok_fetches) = dyn_provider("dyn-ok", false);
        models.set_provider(bad);
        models.set_provider(ok);
        models
            .refresh(None)
            .await
            .expect("best-effort never errors");
        assert_eq!(
            bad_fetches.load(Ordering::SeqCst),
            1,
            "failing source still fetched"
        );
        assert_eq!(
            ok_fetches.load(Ordering::SeqCst),
            1,
            "healthy source fetched"
        );
    }
}
