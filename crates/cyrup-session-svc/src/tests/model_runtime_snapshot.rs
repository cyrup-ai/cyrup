//! CFG-020 — the composed model registry is built ONCE per invalidation, and the availability
//! filter is evaluated per PROVIDER, not per model.
//!
//! pi composes its registry in `rebuildProviders()` and stores the result in `this.snapshot`
//! (`packages/coding-agent/src/core/model-runtime.ts:268-284` @v0.87.1); every later read —
//! `getAvailableSnapshot()` (`:423-425`), `hasConfiguredAuth()` (`:467-469`) — is a field or `Set`
//! lookup. cyrup recomposed on every read instead.
//!
//! Both tests below measure that through ONE seam: a [`CountingProvider`] that counts calls to
//! `Provider::models()`. That is not a proxy for the property, it IS the property at two different
//! call sites:
//!
//! * the composition reads the installed provider's catalog exactly once
//!   (`session/model.rs`, the BASE layer), so the count over a series of registry reads is the
//!   number of COMPOSITIONS; and
//! * the pre-CFG-020 availability filter read it once per MODEL, inside `has_configured_auth`'s
//!   third arm, so the count over ONE `available_model_catalog()` call is the number of PER-MODEL
//!   evaluations.
//!
//! **No network.** Nothing here issues a request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cyrup_agent::{Context, StreamOptions};
use cyrup_core::{EventStream, ExtensionId, ProviderId};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{CatalogOverlay, Model, Provider, StreamEvent};
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

/// A [`Provider`] that counts catalog reads and otherwise delegates to a real [`FauxProvider`].
struct CountingProvider {
    inner: FauxProvider,
    calls: Arc<AtomicUsize>,
}

impl CountingProvider {
    /// Build one, returning it beside its catalog-read counter. Not `new`, because it hands back
    /// the counter alongside the provider rather than `Self`.
    fn install(model_id: &str) -> (Arc<dyn Provider>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(Self {
            inner: FauxProvider::with_config(FauxConfig {
                models: vec![FauxModelDefinition::new(model_id)],
                ..FauxConfig::default()
            }),
            calls: Arc::clone(&calls),
        });
        (provider as Arc<dyn Provider>, calls)
    }
}

#[async_trait::async_trait]
impl Provider for CountingProvider {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    fn models(&self) -> &[Model] {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.models()
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }
}

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

async fn session_with(fx: &Fx, provider: Arc<dyn Provider>) -> AgentSession {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    SessionBuilder::new(provider, cfg)
        .build()
        .await
        .expect("session builds")
}

fn guest_config(model_id: &str) -> serde_json::Value {
    serde_json::json!({
        "name": "Acme",
        "baseUrl": "https://acme.test/v1",
        "api": "openai-completions",
        "apiKey": "sk-acme-123",
        "models": [{ "id": model_id, "name": model_id, "contextWindow": 64000, "maxTokens": 4096 }],
    })
}

fn overlay_model(id: &str) -> Model {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": id,
        "api": "openai-completions",
        "provider": "groq",
        "baseUrl": "https://api.groq.com/openai/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": 128_000,
        "maxTokens": 8192
    }))
    .unwrap()
}

/// The registry is composed ONCE and then re-read from the snapshot, and each of the THREE
/// invalidation sources advances it by exactly one — pi's `rebuildProviders()` →
/// `updateModelSnapshot()` → field reads (`model-runtime.ts:268-284`, `:423-425`).
///
/// FAILS WITHOUT CFG-020 at the very first assertion: `full_model_catalog()` recomposed on every
/// call, so five reads read the installed provider's catalog five times.
#[tokio::test]
async fn the_registry_is_composed_once_per_invalidation_not_once_per_read() {
    let fx = fixture();
    let (provider, calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, Arc::clone(&provider)).await;
    // Building a session reads the installed catalog itself (initial model resolution); the
    // contract under test is about READS of the registry, so start the count there.
    calls.store(0, Ordering::Relaxed);

    // --- 1. Five reads, one composition. ---
    let first = session.full_model_catalog();
    for _ in 0..4 {
        let again = session.full_model_catalog();
        assert_eq!(
            again.len(),
            first.len(),
            "a cached read must return the same registry"
        );
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "five registry reads must compose ONCE (pi reads `this.snapshot.all`, a field)"
    );

    // --- 2. A guest provider registration invalidates, exactly once. ---
    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("acme-ext"),
            "acme",
            guest_config("acme-fast"),
        )
        .unwrap();
    let after_guest = session.full_model_catalog();
    assert!(
        after_guest.iter().any(|m| m.id.as_str() == "acme-fast"),
        "the guest model must be in the registry"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "a guest registration invalidates the snapshot exactly once"
    );
    for _ in 0..3 {
        let _ = session.full_model_catalog();
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "reads after the invalidation are served from the new snapshot"
    );

    // --- 3. Installing a catalog overlay invalidates, exactly once. ---
    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "groq".to_string(),
            vec![overlay_model("overlay-only-model")],
        )]));
    let after_overlay = session.full_model_catalog();
    assert!(
        after_overlay
            .iter()
            .any(|m| m.id.as_str() == "overlay-only-model"),
        "the overlay model must reach the registry"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        3,
        "installing an overlay invalidates the snapshot exactly once"
    );
    for _ in 0..3 {
        let _ = session.full_model_catalog();
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        3,
        "and then stops recomposing"
    );

    // --- 4. Swapping the installed provider invalidates, exactly once. ---
    let (other, other_calls) = CountingProvider::install("other-1");
    session.provider_swap().store(Arc::clone(&other));
    let after_swap = session.full_model_catalog();
    assert!(
        after_swap.iter().any(|m| m.id.as_str() == "other-1"),
        "the newly installed provider's catalog must be in the registry"
    );
    assert_eq!(
        other_calls.load(Ordering::Relaxed),
        1,
        "a provider swap invalidates the snapshot exactly once"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        3,
        "and the provider that is no longer installed is not read again"
    );
    for _ in 0..3 {
        let _ = session.full_model_catalog();
    }
    assert_eq!(other_calls.load(Ordering::Relaxed), 1);

    // --- 5. Swapping BACK is a miss, not a stale hit: the key is the live provider. ---
    session.provider_swap().store(provider);
    let back = session.full_model_catalog();
    assert!(
        back.iter().any(|m| m.id.as_str() == "faux-1"),
        "swapping back must restore the original provider's catalog"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        4,
        "swapping back recomposes rather than serving the other provider's snapshot"
    );
}

/// The availability filter evaluates its per-provider predicates ONCE PER DISTINCT PROVIDER — pi's
/// `snapshot.configuredProviders`, built from one `checkAuth` per provider and then consulted as
/// `configuredProviders.has(model.provider)` (`model-runtime.ts:283`, `:302-311`).
///
/// FAILS WITHOUT CFG-020: `available_model_catalog()` ran `has_configured_auth` per MODEL, whose
/// third arm reads the installed provider's catalog, so the count was one per model in the whole
/// registry — over a thousand — instead of the two reads (one composing, one building the filter)
/// it is now.
#[tokio::test]
async fn availability_is_evaluated_per_provider_not_per_model() {
    let fx = fixture();
    let (provider, calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;

    let registry_len = session.full_model_catalog().len();
    let distinct_providers = {
        let mut ids: Vec<String> = session
            .full_model_catalog()
            .iter()
            .map(|m| m.provider.as_str().to_string())
            .collect();
        ids.sort();
        ids.dedup();
        ids.len()
    };
    assert!(
        registry_len > 500 && distinct_providers < 100,
        "sanity: the built-in registry is ~1000 models across a few dozen providers, got \
         {registry_len} models / {distinct_providers} providers"
    );

    calls.store(0, Ordering::Relaxed);
    let available = session.available_model_catalog();
    let reads = calls.load(Ordering::Relaxed);
    assert!(
        !available.is_empty(),
        "the offline faux model is always available"
    );
    assert!(
        reads <= 2,
        "one availability pass must read the installed catalog a BOUNDED number of times \
         (composition + filter build), got {reads} reads over {registry_len} models"
    );
    assert!(
        reads < distinct_providers,
        "and certainly not once per provider ({distinct_providers}), let alone once per model"
    );
}

/// GUARD (green before and after): hoisting the predicate out of the per-model loop changes no
/// ANSWER. `available_model_catalog()` is still exactly the full registry filtered by
/// `has_configured_auth`, model for model, in order.
#[tokio::test]
async fn the_hoisted_filter_answers_exactly_what_has_configured_auth_answers() {
    let fx = fixture();
    let (provider, _calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;

    // A guest provider and an overlay, so all three arms of the predicate are live.
    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("acme-ext"),
            "acme",
            guest_config("acme-fast"),
        )
        .unwrap();
    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "groq".to_string(),
            vec![overlay_model("overlay-only-model")],
        )]));

    let expected: Vec<(String, String)> = session
        .full_model_catalog()
        .into_iter()
        .filter(|m| session.has_configured_auth(m))
        .map(|m| (m.provider.as_str().to_string(), m.id.as_str().to_string()))
        .collect();
    let actual: Vec<(String, String)> = session
        .available_model_catalog()
        .into_iter()
        .map(|m| (m.provider.as_str().to_string(), m.id.as_str().to_string()))
        .collect();
    assert_eq!(actual, expected);
    assert!(
        actual.iter().any(|(p, i)| p == "acme" && i == "acme-fast"),
        "the guest arm is exercised"
    );
    assert!(
        actual.iter().any(|(p, i)| p == "faux" && i == "faux-1"),
        "the offline-faux arm is exercised"
    );
}
