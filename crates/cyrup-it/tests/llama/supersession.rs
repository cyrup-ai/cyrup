//! SEAM-151 — a llama.cpp catalog publication superseded mid-flight changes NO provider state:
//! not the session's registry, not the extension registry's ownership of the id, not the
//! controller's own `current` provider.
//!
//! pi runs a publication's `update` only after re-checking the abort signal and the provider's
//! refresh generation in the same synchronous turn (`packages/ai/src/models.ts:521-522`
//! @f1b2e77f5), so a stale publication runs no side effect at all. cyrup's one re-check under the
//! provider-map lock is the session registry's (`GuestProviderRegistry::stale_publication`); this
//! target assembles the real chain behind it — a real [`LlamaController`] refreshed by the real
//! refresh engine, its `update` handing the new provider to the real [`ExtensionRegistry`], whose
//! hub hands it to the registry — and checks every holder after a supersession.
//!
//! **What the test forces.** The supersession is injected, not timed: the controller's register
//! callback, which runs INSIDE the publication's `update` (so after `publish_queued`'s last check),
//! first lets a competing extension re-register `llama.cpp` to completion on a scoped thread (a
//! genuine outside writer: the engine marks the publishing thread, not the process), then forwards
//! the controller's provider. No sleep, no test hook in production code.

use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, ExtensionId};
use cyrup_ext::host::services::{ModelsPublication, ProviderRefreshRequest};
use cyrup_ext::registry::ExtensionRegistry;
use cyrup_llama::LLAMA_PROVIDER_ID;
use cyrup_llama::LlamaError;
use cyrup_llama::provider::{
    CatalogEntry, CatalogPublication, LlamaController, LlamaControllerOptions, LlamaRefreshHost,
    RegisterProviderFn,
};
use cyrup_provider::{
    AnyModel, ConfigProvider, Credential, InMemoryCredentialStore, Modality, Model, ModelCost,
    Provider, RefreshModelsContext,
};
use cyrup_session_svc::GuestProviderRegistry;

/// A llama.cpp chat row as the controller restores one (`openai-completions`, its own provider id).
fn row(id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: LLAMA_PROVIDER_ID.into(),
        base_url: "http://127.0.0.1:9/v1".to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        input_limits: None,
        prompt_cache: None,
        context_window: 8192,
        max_tokens: 1024,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// The refresh host of a session with a stored catalog and no credential: the controller's
/// cache-only restore publishes `stored` through the engine's own context.
struct StoredCatalog(Vec<Model>);

#[async_trait::async_trait]
impl LlamaRefreshHost for StoredCatalog {
    async fn credential(&self, _ctx: &RefreshModelsContext) -> Option<Credential> {
        None
    }
    async fn stored(&self, _ctx: &RefreshModelsContext) -> Option<CatalogEntry> {
        Some(CatalogEntry {
            models: self.0.iter().cloned().map(AnyModel::Chat).collect(),
            checked_at: 1,
        })
    }
    async fn publish(
        &self,
        ctx: &RefreshModelsContext,
        publication: CatalogPublication,
    ) -> Result<bool, LlamaError> {
        ctx.publish(ModelsPublication {
            persist: None,
            update: publication.update,
        })
        .await
        .map_err(|error| LlamaError::Message(error.to_string()))
    }
}

/// SEAM-151's Verify, the llama half: after a stale `update`, the controller's `current` is the
/// provider it had before the publication, the id's owner is the competitor that superseded it,
/// and the session registry holds the competitor's provider.
///
/// RED at `4f522804b`: `try_install` stored the stale provider in `current` before handing it over,
/// and `ExtensionRegistry::register_provider_live` recorded the llama extension as the id's owner
/// after the hub's write, both whatever the registry decided — so `current` was the stale catalog
/// and the competitor could no longer retract the provider it owned.
#[tokio::test]
async fn a_superseded_llama_publication_changes_no_provider_state() {
    let llama = ExtensionId::from("builtin:llama.cpp");
    let competitor = ExtensionId::from("competitor");
    let ext = Arc::new(ExtensionRegistry::new());
    let registry = GuestProviderRegistry::new();
    ext.bind_model_registry(registry.clone()).unwrap();

    let newer: Arc<dyn Provider> =
        ConfigProvider::new(LLAMA_PROVIDER_ID, "newer", None, vec![row("newer")]).into_arc();
    // The competitor runs once, from the first hand-over made while the publication is in flight.
    let pending: Arc<Mutex<Option<Arc<dyn Provider>>>> =
        Arc::new(Mutex::new(Some(Arc::clone(&newer))));
    let register: RegisterProviderFn = {
        let ext = Arc::clone(&ext);
        let llama = llama.clone();
        let competitor = competitor.clone();
        let pending = Arc::clone(&pending);
        Arc::new(move |provider: Arc<dyn Provider>| {
            if let Some(newer) = pending.lock().unwrap().take() {
                std::thread::scope(|s| {
                    s.spawn(|| {
                        let _ = ext
                            .register_provider_live(competitor.clone(), LLAMA_PROVIDER_ID, newer)
                            .unwrap();
                    })
                    .join()
                    .unwrap();
                });
            }
            ext.register_provider_live(llama.clone(), LLAMA_PROVIDER_ID, provider)
                .map_err(|error| error.to_string())
        })
    };
    let controller = LlamaController::new(
        LlamaControllerOptions::new(Arc::new(InMemoryCredentialStore::new()), register)
            .with_refresh_host(Arc::new(StoredCatalog(vec![row("stale")]))),
    );
    // What the extension's `init` does: register the controller's (empty) provider.
    let before: Arc<dyn Provider> = controller.provider();
    let _ = ext
        .register_provider_live(llama.clone(), LLAMA_PROVIDER_ID, Arc::clone(&before))
        .unwrap();

    let result = registry
        .refresh(ProviderRefreshRequest {
            providers: Some(vec![LLAMA_PROVIDER_ID.to_string()]),
            allow_network: Some(false),
            force: false,
            cancel: CancelToken::new(),
        })
        .await;
    while registry.refresh_tasks_in_flight() > 0 {
        tokio::task::yield_now().await;
    }
    assert!(
        pending.lock().unwrap().is_none(),
        "the publication's update ran (the competitor was injected): {result:?}"
    );

    assert!(
        Arc::ptr_eq(&registry.provider(LLAMA_PROVIDER_ID).unwrap(), &newer),
        "the registry holds the competitor's provider"
    );
    let current: Arc<dyn Provider> = controller.provider();
    assert!(
        Arc::ptr_eq(&current, &before),
        "the stale publication replaced the controller's provider: it lists {:?}",
        current
            .models()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(controller.register_error(), None);
    assert!(
        ext.unregister_provider_owned(&competitor, LLAMA_PROVIDER_ID)
            .unwrap(),
        "the stale publication took the id's ownership from the competitor"
    );
    assert!(!registry.has_provider(LLAMA_PROVIDER_ID));
}
