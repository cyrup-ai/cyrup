//! `ConfigProvider` — a [`Provider`] built from a custom/guest provider registration (Pi
//! `ModelRegistry.applyProviderConfig`, model-registry.ts:892-960). A guest `pi.registerProvider()`
//! contributes a `baseUrl`/`api`/`apiKey` + a model catalog; Pi folds those models straight into the
//! shared `ModelRegistry.models` array (model-registry.ts:917-940) so the registered model becomes
//! selectable and streamable exactly like a built-in.
//!
//! cyrup streams through a concrete [`Provider`] per provider id, so a guest registration is realized
//! as a `ConfigProvider`: a static catalog + an api-key auth + an in-memory credential store seeded
//! with the registration's resolved api key (Pi `providerRequestConfigs.apiKey`,
//! model-registry.ts:705-741) + the built-in api registry — mirroring how the built-in
//! together/openai providers are constructed ([`crate::wire::WireProvider`]).

use crate::api::builtin_registry;
use crate::auth::{
    Credential, CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key,
};
use crate::context::Context;
use crate::model::Model;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions};
use crate::wire::WireProvider;
use cyrup_core::{EventStream, ProviderId};
use std::sync::Arc;

/// A [`Provider`] realized from a guest/custom provider registration (Pi `applyProviderConfig`). It
/// delegates the whole stream/auth machinery to an inner [`WireProvider`] — the same reusable
/// implementation the built-in providers ride — over the registration's static catalog.
pub struct ConfigProvider {
    inner: WireProvider,
}

impl ConfigProvider {
    /// Build a provider from a registration's resolved parts: `id`, display `name`, the already
    /// resolved api key (literal / `$ENV` / `!command` — resolution happens at registration time),
    /// and the parsed model catalog. A present, non-empty key is seeded into an in-memory credential
    /// store so request auth resolves to it (Pi stores it in `providerRequestConfigs` and injects it
    /// per request, model-registry.ts:705-741); absent a key the provider streams as "not configured"
    /// (Pi requires `apiKey` or `oauth` when a registration defines models, model-registry.ts:880-882).
    pub fn new(
        id: impl Into<ProviderId>,
        name: impl Into<String>,
        resolved_api_key: Option<String>,
        models: Vec<Model>,
    ) -> Self {
        let id = id.into();
        let store: Arc<dyn CredentialStore> = match resolved_api_key {
            Some(key) if !key.is_empty() => Arc::new(
                InMemoryCredentialStore::new()
                    .with_credential(id.clone(), Credential::api_key(key)),
            ),
            _ => Arc::new(InMemoryCredentialStore::new()),
        };
        // `env_key` with no vars: a stored credential owns the provider (used here); with none stored
        // it resolves to "not configured" rather than reading an unrelated env var.
        let auth = ProviderAuth::with_api_key(env_key("API key", Vec::<String>::new()));
        let inner =
            WireProvider::new(id, name, models, auth, store, Arc::new(builtin_registry()));
        Self { inner }
    }

    /// Erase to a shared `Arc<dyn Provider>` (the form the model registry / provider swap install).
    pub fn into_arc(self) -> Arc<dyn Provider> {
        Arc::new(self)
    }
}

/// # PROV-M01 — every surface method is delegated, including the ones with trait defaults
///
/// Upstream a registration-backed provider is not a wrapper at all: `applyProviderConfig` folds the
/// registration's models straight into the shared `ModelRegistry.models` array
/// (`model-registry.ts:917-940` @v0.83.0), so `name`/`baseUrl`/`headers`/`filterModels` are read off
/// the one `Provider` object that owns them. cyrup realizes the registration as a wrapper around a
/// [`WireProvider`], and a hand-written delegating impl forwards only what it names — while
/// `Provider`'s cyrup counterparts of those four members carry TRAIT DEFAULTS
/// (`provider.rs:23-51`), which return a plausible answer instead of failing.
///
/// The display `name` proved it: [`ConfigProvider::new`] takes one, stores it on the inner
/// `WireProvider` (which overrides `Provider::name`, `wire.rs:113-115`), and — with this delegation
/// missing — no caller could ever read it back. `ConfigProvider::name()` fell through to the trait
/// default `self.id().as_str()`, so a registration declaring `"Acme"` displayed as `acme` in every
/// provider picker and status line. `base_url`, `headers` and `filter_models` were dropped by the
/// same mechanism (`wire.rs:117-137`).
///
/// `get_model` is intentionally left on its default: it derives from `models()`, which IS delegated,
/// so it already resolves against the inner catalog.
#[async_trait::async_trait]
impl Provider for ConfigProvider {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    /// PROV-M01 — the registration's display name. Without this the trait default reported the id.
    fn name(&self) -> &str {
        Provider::name(&self.inner)
    }

    /// PROV-M01 — trait default is `None`.
    fn base_url(&self) -> Option<&str> {
        self.inner.base_url()
    }

    /// PROV-M01 — trait default is `None`.
    fn headers(&self) -> Option<&crate::HeaderMap> {
        self.inner.headers()
    }

    fn models(&self) -> &[Model] {
        self.inner.models()
    }

    /// PROV-M01 — trait default returns the catalog unchanged.
    fn filter_models(
        &self,
        models: &[Model],
        credential: Option<&Credential>,
    ) -> Vec<Model> {
        self.inner.filter_models(models, credential)
    }

    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.inner.provider_auth()
    }

    /// PROV-M01 — trait default is `None` ("static provider"). Delegated so the answer tracks the
    /// inner rather than this wrapper's knowledge of what the inner currently implements.
    async fn refresh_models(&self) -> Option<Result<(), crate::error::ProviderError>> {
        self.inner.refresh_models().await
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }

    /// PROV-M01 — the trait default lowers and re-enters `self.stream`, which reaches the same
    /// inner today. Delegated anyway: an inner that overrides `stream_simple` (the token-budget
    /// providers do, `provider.rs:103-105`) must not have its override skipped by this wrapper.
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &crate::utils::simple_options::SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream_simple(model, context, options)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::known_api::OPENAI_COMPLETIONS;
    use crate::model::{Modality, ModelCost};
    use crate::stream::collect_message;
    use cyrup_core::StopReason;

    fn model(id: &str, base_url: &str) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: OPENAI_COMPLETIONS.into(),
            provider: "acme".into(),
            base_url: base_url.to_string(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 128_000,
            max_tokens: 16_384,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn exposes_registration_catalog() {
        let p = ConfigProvider::new("acme", "Acme", Some("sk-acme".into()), vec![model("acme-1", "https://acme.test/v1")]);
        assert_eq!(p.id().as_str(), "acme");
        assert!(p.get_model("acme-1").is_some());
    }

    /// PROV-M01 — the registration's DISPLAY NAME must survive the wrapper.
    ///
    /// The fixture's name is deliberately distinct from its id (`"Acme Machines, Inc."` vs `acme`),
    /// because `Provider::name`'s trait default is `self.id().as_str()` (`provider.rs:23-25`): a
    /// fixture that named the provider `"acme"` would compare the default against the default and
    /// stay green with the delegation deleted. Upstream there is no wrapper at all — the
    /// registration's fields land on the one `Provider` object (`model-registry.ts:917-940`
    /// @v0.83.0) — so nothing upstream can drop this.
    #[test]
    fn the_registrations_display_name_survives_the_wrapper() {
        let p = ConfigProvider::new(
            "acme",
            "Acme Machines, Inc.",
            Some("sk-acme".into()),
            vec![model("acme-1", "https://acme.test/v1")],
        );
        assert_ne!("Acme Machines, Inc.", p.id().as_str(), "fixture must not agree with the default");
        assert_eq!(Provider::name(&p), "Acme Machines, Inc.");
    }

    /// A seeded key resolves (auth does not short-circuit); the failure is a transport error against
    /// the unroutable base URL, proving the key flowed through to the request path.
    #[tokio::test]
    async fn seeded_key_resolves_then_fails_at_transport() {
        let mut m = model("acme-1", "http://127.0.0.1:1/v1");
        m.base_url = "http://127.0.0.1:1/v1".to_string();
        let p = ConfigProvider::new("acme", "Acme", Some("sk-acme".into()), vec![m.clone()]);
        let msg = collect_message(p.stream(&m, &Context::default(), &StreamOptions::default())).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(!err.contains("not configured"), "seeded key should resolve, got: {err}");
        assert!(err.contains("transport"), "expected transport error, got: {err}");
    }

    /// No key → the provider streams as "not configured" (Pi requires apiKey/oauth for models).
    #[tokio::test]
    async fn missing_key_is_not_configured() {
        let m = model("acme-1", "http://127.0.0.1:1/v1");
        let p = ConfigProvider::new("acme", "Acme", None, vec![m.clone()]);
        let msg = collect_message(p.stream(&m, &Context::default(), &StreamOptions::default())).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }
}
