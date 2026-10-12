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

// ---- PROV-162: a registration over a BUILT-IN id composes over it ---------------------------

/// The registry's rows of `provider`, in order.
fn rows_of(session: &AgentSession, provider: &str) -> Vec<Model> {
    session
        .full_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() == provider)
        .collect()
}

/// PROV-162 — `registerProvider("anthropic", { baseUrl })` lists Anthropic's models at the new URL.
///
/// What `compose_model_registry` (`session/model.rs`) did at `360dc2bb`, established by this
/// test there: it unions `current.models()`, the guest providers' models, `default_models` and then
/// composes `models.json` over the union, first row wins on `(provider, id)`. The guest
/// registration was a base-less `ConfigProvider` with no rows (no `models` key), so the union kept
/// the compiled-in anthropic rows unchanged: the run at `360dc2bb` fails the URL assertion with
/// `https://api.anthropic.com` (count unchanged). pi lists the composed provider's rows
/// (`applyExtension`, `provider-composer.ts:347-348` @f1b2e77f5): the same models, the new URL.
#[tokio::test]
async fn a_base_url_registration_over_a_builtin_lists_its_models_at_the_new_url() {
    let fx = fixture();
    let (provider, _calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;
    let before = rows_of(&session, "anthropic");
    assert!(!before.is_empty());
    assert!(
        before
            .iter()
            .all(|m| m.base_url == "https://api.anthropic.com"),
        "fixture: the built-in rows"
    );

    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("e"),
            "anthropic",
            serde_json::json!({ "baseUrl": "http://proxy.test" }),
        )
        .unwrap();

    let after = rows_of(&session, "anthropic");
    let urls: Vec<&str> = after.iter().map(|m| m.base_url.as_str()).collect();
    assert!(urls.iter().all(|u| *u == "http://proxy.test"), "{urls:?}");
    let ids = |rows: &[Model]| -> Vec<String> {
        rows.iter().map(|m| m.id.as_str().to_string()).collect()
    };
    assert_eq!(
        ids(&after),
        ids(&before),
        "the same models, in the same order"
    );
}

/// PROV-162 — a registration WITH `models` over a built-in lists only its own rows for that id
/// (pi `applyExtension`, `:350`: the definitions replace the catalog), each filled from the base
/// (`findExtensionModelDefaults`, `:261-273`): no id match, no api, no `openai-completions` row in
/// anthropic's catalog, so the first row's `anthropic-messages` and `baseUrl`.
///
/// RED at `360dc2bb`: the definition has no `api`/`baseUrl` of its own, so `build_models` skipped
/// it, and the anthropic rows were the 17 built-ins.
#[tokio::test]
async fn a_registration_with_models_over_a_builtin_lists_only_its_rows() {
    let fx = fixture();
    let (provider, _calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;
    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("e"),
            "anthropic",
            serde_json::json!({
                "models": [{ "id": "proxy-only", "contextWindow": 1000, "maxTokens": 100 }]
            }),
        )
        .unwrap();
    let rows = rows_of(&session, "anthropic");
    let ids: Vec<&str> = rows.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["proxy-only"]);
    assert_eq!(rows[0].api.as_str(), "anthropic-messages");
    assert_eq!(rows[0].base_url, "https://api.anthropic.com");
}

/// The env vars anthropic's own strategy reads (`providers/anthropic.rs` `anthropic_auth()`).
fn anthropic_env_is_set() -> bool {
    [
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_OAUTH_TOKEN",
        "ANTHROPIC_API_KEY",
    ]
    .iter()
    .any(|name| std::env::var(name).is_ok_and(|v| !v.is_empty()))
}

/// PROV-162 — a key-less registration over a built-in does not, by itself, configure the id: pi's
/// `configuredProviders` comes from `checkAuth` over the COMPOSED provider (`model-runtime.ts:343`),
/// whose `check` with no stored credential and no registration key is the inherited strategy's
/// (`provider-composer.ts:453-455`). A registration that carries a key is configured
/// (`:445-451`).
///
/// RED at `360dc2bb`: any guest registration marked its id configured
/// (`physical_provider_is_configured`'s guest arm), key or no key.
#[tokio::test]
async fn a_keyless_registration_over_a_builtin_is_configured_only_through_its_auth() {
    if anthropic_env_is_set() {
        eprintln!("skipped: an ANTHROPIC_* credential is set in this environment");
        return;
    }
    let fx = fixture();
    let (provider, _calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;
    let register = |config: serde_json::Value| {
        session
            .services()
            .ext_host
            .registry()
            .register_provider(ExtensionId::from("e"), "anthropic", config)
            .unwrap();
    };

    register(serde_json::json!({ "baseUrl": "http://proxy.test" }));
    let row = rows_of(&session, "anthropic").remove(0);
    assert!(
        !session.has_configured_auth(&row),
        "a key-less registration over anthropic, with no anthropic credential, is not configured"
    );
    assert!(
        !session
            .available_model_catalog()
            .iter()
            .any(|m| m.provider.as_str() == "anthropic")
    );

    register(serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-registration" }));
    let row = rows_of(&session, "anthropic").remove(0);
    assert!(session.has_configured_auth(&row));
}

/// PROV-162 — the registration composes over the base AS THE SESSION HAS IT, `models.json` layer
/// included, and its own layer sits on top (pi's order: built-in → `models.json` → extension,
/// `provider-composer.ts:555-560` @f1b2e77f5). A `models.json` `baseUrl` for anthropic therefore
/// reaches a definition's gaps (its `defaults` row is the composed base's), while the registration's
/// own `baseUrl` wins over it for a registration without `models` (`extension?.baseUrl ??
/// config?.baseUrl`, `:608`, and `applyExtension` over `applyModelsJson`'s rows, `:556-560`).
///
/// RED at `360dc2bb`: no base reached the registration, so the definition (no `api`/`baseUrl` of
/// its own) was skipped and the 17 built-in rows were listed — and there `models.json` was composed
/// over the whole union LAST, which would put its `baseUrl` back over a registration's. Named
/// mutations: the builder's base resolver detached (the base is the bare built-in) turns the first
/// assertion red; re-applying `models.json` over the guest-owned rows turns the second red.
#[tokio::test]
async fn a_registration_composes_over_the_models_json_layer_and_sits_above_it() {
    let fx = fixture();
    std::fs::write(
        fx.agent_dir.join("models.json"),
        r#"{ "providers": { "anthropic": { "baseUrl": "https://models-json.test" } } }"#,
    )
    .unwrap();
    let (provider, _calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;
    let register = |config: serde_json::Value| {
        session
            .services()
            .ext_host
            .registry()
            .register_provider(ExtensionId::from("e"), "anthropic", config)
            .unwrap();
    };

    register(serde_json::json!({ "models": [{ "id": "proxy-only" }] }));
    let rows = rows_of(&session, "anthropic");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].base_url, "https://models-json.test");

    // A re-registration MERGES over the previous one (PROV-163, pi `registerProvider`,
    // `model-runtime.ts:926-933`), so the definitions above would survive a `{ baseUrl }`; an
    // unregistration drops the stored registration (pi `unregisterProvider`, `:944-950`) and the
    // next one starts from nothing, which is the registration-without-`models` this half is about.
    session
        .services()
        .ext_host
        .registry()
        .unregister_provider("anthropic")
        .unwrap();
    register(serde_json::json!({ "baseUrl": "http://proxy.test" }));
    let rows = rows_of(&session, "anthropic");
    assert!(rows.len() > 1);
    assert!(rows.iter().all(|m| m.base_url == "http://proxy.test"));
}

/// PROV-164 — a `models.json` `modelOverrides` entry reaches a model an extension RE-DECLARES: pi
/// applies the overrides last, over the extension's rows ("models.json modelOverrides are the
/// topmost user-config layer: they apply once, after custom-model upserts, extension model
/// replacement…", `provider-composer.ts:555-575` @f1b2e77f5). Asserted on the session's list AND on
/// the provider the session streams through, which reads its OWN row (`ProviderStreamFn`
/// re-resolves `provider.get_model`), so the two cannot disagree.
///
/// RED at `4f522804b`: the registration's definitions replaced the base's (overridden) rows and no
/// override reached them — the row kept the extension's name and the default `contextWindow`.
#[tokio::test]
async fn a_models_json_override_reaches_a_model_the_extension_re_declares() {
    let fx = fixture();
    let id = cyrup_provider::anthropic_provider()
        .models()
        .first()
        .expect("anthropic lists models")
        .id
        .as_str()
        .to_string();
    std::fs::write(
        fx.agent_dir.join("models.json"),
        serde_json::json!({ "providers": { "anthropic": { "modelOverrides": {
            id.as_str(): { "name": "From models.json", "contextWindow": 4242 }
        } } } })
        .to_string(),
    )
    .unwrap();
    let (provider, _calls) = CountingProvider::install("faux-1");
    let session = session_with(&fx, provider).await;
    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("e"),
            "anthropic",
            serde_json::json!({ "models": [{ "id": id.as_str(), "name": "From the extension" }] }),
        )
        .unwrap();

    let rows = rows_of(&session, "anthropic");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id.as_str(), id);
    assert_eq!(rows[0].name, "From models.json");
    assert_eq!(rows[0].context_window, 4242);

    let streamed = session
        .services()
        .guest_providers
        .provider("anthropic")
        .expect("the registration landed");
    let row = streamed
        .get_model(&id)
        .expect("the streamed provider lists it");
    assert_eq!(row.name, "From models.json");
    assert_eq!(row.context_window, 4242);
}
