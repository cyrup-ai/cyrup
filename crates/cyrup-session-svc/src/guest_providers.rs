//! The shared model-registry sink for guest-registered providers (arch-08 §5.6; Pi `bindCore` +
//! `ModelRegistry.registerProvider`, model-registry.ts:828-960 / runner.ts:308-380).
//!
//! ROOT CAUSE this closes (L4 gap #4): a guest `pi.registerProvider()` routed through the extension
//! host's [`cyrup_ext::ProviderHub`] but nothing consumed the registrations, so the model never
//! became selectable. Pi's `runner.bindCore` flushes each queued registration into the ONE
//! `ModelRegistry` that `getAvailable()` / `find()` / `setModel` all read (model-registry.ts:917-940
//! folds the registered models straight into `this.models`).
//!
//! cyrup's session streams through a concrete [`Provider`] per provider id, so this registry realizes
//! each guest registration as a `ConfigProvider` (via [`cyrup_ext::ProviderRegistration::build_provider`])
//! and holds it behind an `Arc`. The [`AgentSession`](crate::AgentSession) then UNIONs these providers'
//! catalogs into `full_model_registry()` / `available_model_catalog()` and installs the owning provider
//! into the [`crate::provider_swap::ProviderSwap`] on a matching `set_model`, so the registered model is both
//! SELECTABLE and STREAMABLE in the assembled run.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use cyrup_ext::provider::{ModelRegistrySink, ProviderRegistration};
use cyrup_provider::{Model, Provider};

/// The bound sink + shared lookup for guest-registered providers. Cheaply shareable via `Arc`
/// (interior `Mutex`); the SAME `Arc` is handed to [`cyrup_ext::ExtensionRegistry::bind_model_registry`]
/// (as the sink) and to the session (as the read view).
#[derive(Default)]
pub struct GuestProviderRegistry {
    /// Realized providers keyed by provider id, in insertion order (BTreeMap for a stable catalog).
    providers: Mutex<BTreeMap<String, Arc<dyn Provider>>>,
}

impl GuestProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The provider realized for `id`, if a guest registered one (cheap `Arc` clone). Poison-safe.
    pub fn provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.lock().get(id).cloned()
    }

    /// Whether a guest provider with this id is registered.
    pub fn has_provider(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }

    /// Every guest-registered provider's model catalog, unioned (Pi folds registered models into the
    /// shared `ModelRegistry.models`, model-registry.ts:917-940). Stable order by provider id.
    pub fn models(&self) -> Vec<Model> {
        self.lock().values().flat_map(|p| p.models().to_vec()).collect()
    }

    /// The registered provider ids (diagnostics).
    pub fn ids(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<dyn Provider>>> {
        // Poison-safe: a panic elsewhere must not wedge model selection (R-00-009).
        self.providers.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ModelRegistrySink for GuestProviderRegistry {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        // Full replacement for this provider id (Pi "replaces all models", model-registry.ts:919).
        let provider = reg.build_provider();
        self.lock().insert(reg.id.clone(), provider);
    }

    fn remove_provider(&self, id: &str) {
        self.lock().remove(id);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cyrup_ext::registry::ExtensionRegistry;
    use cyrup_core::ExtensionId;
    use serde_json::json;

    fn config() -> serde_json::Value {
        json!({
            "name": "Acme",
            "baseUrl": "https://acme.test/v1",
            "api": "openai-completions",
            "apiKey": "sk-acme-123",
            "models": [{ "id": "acme-fast", "name": "Acme Fast", "contextWindow": 64000, "maxTokens": 4096 }],
        })
    }

    /// Binding the registry flushes a queued guest registration into it (Pi `bindCore` pending flush).
    #[test]
    fn bind_flushes_pending_registration() {
        let ext = ExtensionRegistry::new();
        ext.register_provider(ExtensionId::from("acme-ext"), "acme", config()).unwrap();
        let sink: Arc<GuestProviderRegistry> = Arc::new(GuestProviderRegistry::new());
        ext.bind_model_registry(sink.clone()).unwrap();

        assert!(sink.has_provider("acme"));
        let models = sink.models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id.as_str(), "acme-fast");
        assert_eq!(models[0].provider.as_str(), "acme");
        assert_eq!(models[0].context_window, 64000);
    }

    /// A registration made AFTER bind upserts immediately (Pi post-`bindCore` live registration).
    #[test]
    fn live_registration_after_bind_upserts_immediately() {
        let ext = ExtensionRegistry::new();
        let sink: Arc<GuestProviderRegistry> = Arc::new(GuestProviderRegistry::new());
        ext.bind_model_registry(sink.clone()).unwrap();
        assert!(!sink.has_provider("acme"));

        ext.register_provider(ExtensionId::from("acme-ext"), "acme", config()).unwrap();
        assert!(sink.has_provider("acme"));
        assert!(sink.provider("acme").unwrap().get_model("acme-fast").is_some());

        ext.unregister_provider("acme").unwrap();
        assert!(!sink.has_provider("acme"));
    }
}
