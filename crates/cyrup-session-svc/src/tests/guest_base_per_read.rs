//! PROV-165, PROV-166, SEAM-166 — a guest registration over a built-in is composed over its base
//! AS THE BASE IS AT EACH READ, and the session starts on the row `/model` lists.
//!
//! pi composes the registration over its base inside the composed provider's `getAllModels`, which
//! reads the base on every call (`coding-agent/src/core/provider-composer.ts:555-575`
//! @f1b2e77f5); the base of a built-in id is the `withRemoteCatalog`-wrapped built-in
//! (`core/model-runtime.ts:228-233`), so a catalog reload reaches the composed rows at once. A
//! composition that throws leaves the BASE registered (`composeProvider`, `:312-315`). And pi
//! resolves the starting model only after every extension's provider is registered
//! (`createAgentSessionServices` before `createAgentSession`), so the session starts on the
//! composed row.
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

use cyrup_core::ExtensionId;
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{
    CatalogOverlay, InMemoryCredentialStore, Model, Provider, anthropic_provider_with,
    builtin_registry,
};
use tempfile::TempDir;

use crate::{AgentSession, GuestProviderRegistry, SessionBuilder, SessionConfig};

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

fn anthropic() -> Arc<dyn Provider> {
    Arc::new(anthropic_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    ))
}

fn faux() -> Arc<dyn Provider> {
    Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1")],
        ..FauxConfig::default()
    }))
}

fn config(fx: &Fx) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

async fn session_on(fx: &Fx, installed: Arc<dyn Provider>) -> AgentSession {
    SessionBuilder::new(installed, config(fx))
        .build()
        .await
        .expect("session builds")
}

/// The registry's rows of `provider`, in order — what `/model` lists before the auth filter.
fn rows_of(session: &AgentSession, provider: &str) -> Vec<Model> {
    session
        .full_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() == provider)
        .collect()
}

fn ids(rows: &[Model]) -> Vec<String> {
    rows.iter().map(|m| m.id.as_str().to_string()).collect()
}

fn register(session: &AgentSession, id: &str, config: serde_json::Value) {
    session
        .services()
        .ext_host
        .registry()
        .register_provider(ExtensionId::from("proxy-ext"), id, config)
        .unwrap();
}

/// One anthropic chat row, as the pi.dev catalog would publish it.
fn anthropic_row(id: &str) -> Model {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": id,
        "api": "anthropic-messages",
        "provider": "anthropic",
        "baseUrl": "https://api.anthropic.com",
        "reasoning": true,
        "input": ["text", "image"],
        "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": 200_000,
        "maxTokens": 8192
    }))
    .unwrap()
}

/// One groq chat row.
fn groq_row(id: &str) -> Model {
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

// ---- PROV-165: the composed rows follow the base as it is at each read ----------------------

/// Done-means (1). A registration over the built-in the session runs on, then a pi.dev catalog
/// reload that adds a row to that built-in: the id lists the RELOADED base's rows at the
/// registration's URL, and the provider the session streams through carries the new row too.
///
/// RED at `9be1ec46`: the registration was composed over the base as the overlay stood when it
/// landed and nothing re-composed it on a reload, so `claude-reloaded` was missing (and
/// `compose_model_registry` drops the overlay's own rows for a guest-owned id).
#[tokio::test]
async fn an_overlay_reload_reaches_a_registration_over_a_builtin() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await;
    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    assert!(
        !ids(&rows_of(&session, "anthropic")).contains(&"claude-reloaded".to_string()),
        "fixture: the row is not in the compiled-in catalog"
    );

    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "anthropic".to_string(),
            vec![anthropic_row("claude-reloaded")],
        )]));

    let rows = rows_of(&session, "anthropic");
    let reloaded = rows
        .iter()
        .find(|m| m.id.as_str() == "claude-reloaded")
        .unwrap_or_else(|| panic!("the reloaded row is listed: {:?}", ids(&rows)));
    assert_eq!(reloaded.base_url, "http://proxy.test");
    assert!(
        rows.iter().all(|m| m.base_url == "http://proxy.test"),
        "every row of the id is the registration's"
    );
    let installed = session.provider_swap().current();
    assert_eq!(
        installed
            .get_model("claude-reloaded")
            .map(|m| m.base_url.as_str()),
        Some("http://proxy.test"),
        "the provider the session streams through is the recomposed one"
    );
}

/// GUARD (green at `9be1ec46`) — a reload that has NO entry for the registered id leaves its rows
/// exactly as they were, and a registration under an id no built-in owns is untouched by it.
#[tokio::test]
async fn an_overlay_reload_without_an_entry_for_the_id_changes_nothing_for_it() {
    let fx = fixture();
    let session = session_on(&fx, faux()).await;
    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    register(
        &session,
        "acme",
        serde_json::json!({
            "baseUrl": "https://acme.test/v1",
            "api": "openai-completions",
            "apiKey": "sk-acme",
            "models": [{ "id": "acme-fast" }],
        }),
    );
    let before = rows_of(&session, "anthropic");
    let acme_before = rows_of(&session, "acme");

    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "groq".to_string(),
            vec![groq_row("groq-reloaded")],
        )]));

    assert!(
        session
            .full_model_catalog()
            .iter()
            .any(|m| m.id.as_str() == "groq-reloaded"),
        "fixture: the reload landed"
    );
    assert_eq!(rows_of(&session, "anthropic"), before);
    assert_eq!(rows_of(&session, "acme"), acme_before);
}

// ---- PROV-166: a re-registration that cannot compose serves the base -----------------------

/// A `models.json` whose anthropic block carries an override `models.json` itself rejects
/// (`inputLimits.maxRequestBytes` must be >= 1): the block is rejected for the base (which keeps
/// its built-ins), and any registration that DECLARES models fails to compose, because the
/// overrides apply over the rows it declares (PROV-164). A registration without `models` composes.
fn write_rejected_override(fx: &Fx) {
    std::fs::write(
        fx.agent_dir.join("models.json"),
        r#"{ "providers": { "anthropic": { "modelOverrides": {
            "proxy-only": { "inputLimits": { "maxRequestBytes": 0 } }
        } } } }"#,
    )
    .unwrap();
}

/// Done-means (2). A good registration, then a re-registration that cannot compose: the id lists
/// the BASE's rows, and the provider the session streams through is the base, not the previous
/// composition (pi `composeProvider` records the error and returns `base`,
/// `model-runtime.ts:312-315` @f1b2e77f5).
///
/// RED at `9be1ec46`: the failed composition was logged and the map left as it was, so the id
/// kept listing the first registration's rows at `http://proxy.test`.
#[tokio::test]
async fn a_re_registration_that_cannot_compose_serves_the_base() {
    let fx = fixture();
    write_rejected_override(&fx);
    let session = session_on(&fx, anthropic()).await;
    let base = rows_of(&session, "anthropic");
    assert!(
        base.iter()
            .all(|m| m.base_url == "https://api.anthropic.com"),
        "fixture: the base rows"
    );

    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    assert!(
        rows_of(&session, "anthropic")
            .iter()
            .all(|m| m.base_url == "http://proxy.test"),
        "fixture: the first registration composed"
    );

    // Merged over the first (PROV-163): `{ baseUrl, apiKey, models }` — the declared row takes the
    // rejected override, so the composition fails.
    register(
        &session,
        "anthropic",
        serde_json::json!({ "models": [{ "id": "proxy-only" }] }),
    );
    assert_eq!(
        rows_of(&session, "anthropic"),
        base,
        "the id lists its base's rows"
    );
    let installed = session.provider_swap().current();
    assert_eq!(
        installed.models(),
        base.as_slice(),
        "the session streams the base"
    );
    assert!(
        !session.services().guest_providers.has_provider("anthropic"),
        "no composed provider is held for the id"
    );
}

/// The CONTROL for PROV-166 — GUARD (green at `9be1ec46`): a re-registration whose composition
/// SUCCEEDS still replaces the previous composition.
#[tokio::test]
async fn a_re_registration_that_composes_replaces_the_previous_composition() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await;
    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    register(
        &session,
        "anthropic",
        serde_json::json!({ "models": [{ "id": "proxy-only" }] }),
    );
    let rows = rows_of(&session, "anthropic");
    assert_eq!(ids(&rows), ["proxy-only"]);
    assert_eq!(rows[0].base_url, "http://proxy.test");
    assert_eq!(
        ids(session.provider_swap().current().models()),
        ["proxy-only"]
    );
}

/// Found while enumerating the writes (the installed slot is a copy of the map's entry): an
/// UNREGISTRATION of the id the session runs on leaves the base serving it (pi
/// `unregisterProvider` → `recomposeProvider` → `base`, `model-runtime.ts:944-950`).
///
/// RED at `9be1ec46`: `remove_provider` dropped the map's entry but left the composed provider in
/// the installed slot, whose rows `compose_model_registry` then listed (the id was no longer
/// guest-owned), still at `http://proxy.test`.
#[tokio::test]
async fn an_unregistration_of_the_installed_id_serves_the_base() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await;
    let base = rows_of(&session, "anthropic");
    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    session
        .services()
        .ext_host
        .registry()
        .unregister_provider("anthropic")
        .unwrap();
    assert_eq!(rows_of(&session, "anthropic"), base);
    assert_eq!(session.provider_swap().current().models(), base.as_slice());
}

// ---- SEAM-166: the starting model is the row `/model` lists --------------------------------

/// Registers `anthropic` with a proxy `baseUrl` at `init`.
struct ProxyAnthropic;

#[async_trait::async_trait]
impl NativeExtension for ProxyAnthropic {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("proxy-anthropic")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_provider(
            "anthropic",
            serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A session started with `--model anthropic/<a built-in id>`, `models.json` giving anthropic its
/// own `baseUrl`, and an extension registering anthropic with another. `installed` is the provider
/// the launch path hands the builder: anthropic itself (the `--model` resolves at step 3), or a
/// provider that does not list it (the `--model` is deferred to step 3b).
async fn started_with_model(fx: &Fx, installed: Arc<dyn Provider>) -> (AgentSession, String) {
    std::fs::write(
        fx.agent_dir.join("models.json"),
        r#"{ "providers": { "anthropic": { "baseUrl": "https://models-json.test" } } }"#,
    )
    .unwrap();
    let model_id = anthropic().models()[0].id.as_str().to_string();
    let mut cfg = config(fx);
    cfg.model_pattern = Some(format!("anthropic/{model_id}"));
    let session = SessionBuilder::new(installed, cfg)
        .with_native_extension(Arc::new(ProxyAnthropic) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("session builds");
    (session, model_id)
}

/// Done-means (3): the session's starting model carries the REGISTRATION's `baseUrl`, not
/// `models.json`'s. pi's layer order is built-in → `models.json` → extension
/// (`provider-composer.ts:555-560` @f1b2e77f5), and pi resolves `--model` after the extension's
/// provider is registered.
async fn assert_starts_on_the_registrations_url(installed: Arc<dyn Provider>) {
    let fx = fixture();
    let (session, model_id) = started_with_model(&fx, installed).await;
    let model = session
        .services()
        .model
        .clone()
        .expect("the session has a model");
    assert_eq!(
        (model.provider.as_str(), model.id.as_str()),
        ("anthropic", model_id.as_str())
    );
    assert_eq!(model.base_url, "http://proxy.test");
}

/// Done-means (4) — the invariant the three rows circle: `/model`'s row for the id, the session's
/// starting model and the row the provider it streams through resolves are the same row.
async fn assert_starts_on_the_row_model_lists(installed: Arc<dyn Provider>) {
    let fx = fixture();
    let (session, model_id) = started_with_model(&fx, installed).await;
    let listed = rows_of(&session, "anthropic")
        .into_iter()
        .find(|m| m.id.as_str() == model_id)
        .expect("/model lists the id");
    assert_eq!(session.services().model.as_ref(), Some(&listed));
    assert_eq!(
        session
            .provider_swap()
            .current()
            .get_model(&model_id)
            .cloned()
            .as_ref(),
        Some(&listed),
        "the provider the session streams through resolves the same row"
    );
}

/// Done-means (3), the `--model` resolved at step 3 (the launch path installed anthropic).
///
/// RED at `9be1ec46` (`https://models-json.test`): step 3 resolved against the built-in before the
/// registration existed, and nothing revisited it once the registration landed.
#[tokio::test]
async fn a_model_resolved_at_step_3_carries_the_registrations_url() {
    assert_starts_on_the_registrations_url(anthropic()).await;
}

/// Done-means (3), the `--model` deferred to step 3b (the launch path installed a provider that
/// does not list it).
///
/// RED at `9be1ec46` (`https://models-json.test`): step 3b re-applied the whole `models.json` block
/// over the guest-composed rows (`resolve_model(&*owner, …, &model_file)`).
#[tokio::test]
async fn a_model_deferred_to_step_3b_carries_the_registrations_url() {
    assert_starts_on_the_registrations_url(faux()).await;
}

/// Done-means (4), step-3 arm. RED at `9be1ec46`: the starting model carried `models.json`'s
/// `baseUrl` while `/model` listed the registration's.
#[tokio::test]
async fn a_model_resolved_at_step_3_is_the_row_model_lists() {
    assert_starts_on_the_row_model_lists(anthropic()).await;
}

/// Done-means (4), step-3b arm. RED at `9be1ec46`, as the step-3 arm.
#[tokio::test]
async fn a_model_deferred_to_step_3b_is_the_row_model_lists() {
    assert_starts_on_the_row_model_lists(faux()).await;
}

// ---- the extension's virtual-model conflict check reads the composed id --------------------

/// A router that is never asked to route here.
struct NoRoute;

#[async_trait::async_trait]
impl cyrup_provider::ModelRouter for NoRoute {
    async fn route(
        &self,
        _request: cyrup_provider::ModelRouteRequest<'_>,
    ) -> Result<cyrup_provider::ModelRoute, cyrup_provider::ModelRouteError> {
        Err(cyrup_provider::ModelRouteError::new(
            "not routed in this test",
        ))
    }
}

/// Found while enumerating the recorded compositions: the catalog an extension's
/// `registerVirtualModel` is validated against is the builder's startup snapshot (taken before
/// any registration) with the live guest rows layered UNDER it, first-wins. pi validates against
/// `this.models.getModel(providerId, id)` (`model-runtime.ts:960` @f1b2e77f5) — the COMPOSED id,
/// whose rows a registration with `models` replaces. So a virtual model may take an id the
/// registration no longer lists.
///
/// RED at `9be1ec46`: the snapshot's built-in row answered first, so the registration was refused
/// as a physical conflict.
#[test]
fn a_virtual_model_may_take_an_id_a_registration_over_a_builtin_no_longer_lists() {
    use cyrup_ext::provider::ModelRegistrySink;

    let builtin = anthropic();
    let taken = builtin.models()[0].id.as_str().to_string();
    let registry = GuestProviderRegistry::new();
    registry.attach_virtual_models(
        Arc::new(cyrup_provider::VirtualModelRegistry::new()),
        Arc::new(crate::virtual_models::StartupVirtualCatalog::new(
            builtin.models().to_vec(),
        )),
    );
    let ext = cyrup_ext::registry::ExtensionRegistry::new();
    ext.bind_model_registry(registry.clone()).unwrap();
    ext.register_provider(
        ExtensionId::from("proxy-ext"),
        "anthropic",
        serde_json::json!({ "apiKey": "sk-proxy", "models": [{ "id": "proxy-only" }] }),
    )
    .unwrap();
    assert_eq!(
        ids(registry.provider("anthropic").unwrap().models()),
        ["proxy-only"]
    );

    let definition = cyrup_provider::VirtualModelDefinition::new(
        cyrup_provider::VirtualModelSpec {
            provider: "anthropic".into(),
            id: taken.as_str().into(),
            name: "Routed".to_string(),
            thinking_levels: None,
            context_window: None,
            max_tokens: None,
            input: None,
        },
        Arc::new(NoRoute),
    );
    assert_eq!(registry.upsert_virtual_model(&definition), Ok(()));
    // The id the registration DOES list is still a conflict.
    let mut conflicting = definition.clone();
    conflicting.spec.id = "proxy-only".into();
    assert!(registry.upsert_virtual_model(&conflicting).is_err());
}

// ---- PROV-165's cost question, measured -------------------------------------------------------

/// MEASUREMENT, not a check (ignored; run with `--run-ignored only`, release numbers are lower).
/// What one read of the guest registry costs under the two designs PROV-165 weighed, with `n`
/// registrations composed over built-ins:
///
/// * **revalidated read** (what ships): the read compares the overlay slot with the overlay each
///   composition was resolved with, and nothing moved;
/// * **resolve per read** (pi's literal shape): every read resolves each id's base and composes
///   the registration over it — forced here by installing a fresh overlay `Arc` before each read,
///   which makes the shipped code do exactly that work;
///
/// and, for scale, what one warm `/model` listing (`available_model_catalog`) costs today. That
/// listing reads the guest registry 41 times with one composed id (counted at the time of
/// writing: the generation key, then one `carries_own_credentials` per provider the availability
/// filter evaluates), so resolve-per-read would pay its per-read cost that many times per listing,
/// and could no longer key the composed-registry snapshot on the generation (CFG-020).
#[tokio::test]
#[ignore = "measurement; prints timings"]
async fn measure_a_guest_registry_read_revalidated_vs_resolved_per_read() {
    use std::time::Instant;
    const IDS: [&str; 5] = ["anthropic", "openai", "groq", "google", "mistral"];
    for n in [1usize, 5] {
        let fx = fixture();
        let session = session_on(&fx, faux()).await;
        for id in IDS.iter().take(n) {
            register(
                &session,
                id,
                serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
            );
        }
        let guest = &session.services().guest_providers;
        let _ = guest.models();
        let reads = 2_000u32;
        let started = Instant::now();
        for _ in 0..reads {
            let _ = guest.models();
        }
        let revalidated = started.elapsed() / reads;
        let reads = 50u32;
        let started = Instant::now();
        for _ in 0..reads {
            session
                .services()
                .catalog_overlay
                .install(CatalogOverlay::from_entries([(
                    "groq".to_string(),
                    vec![groq_row("groq-moved")],
                )]));
            let _ = guest.models();
        }
        let per_read = started.elapsed() / reads;
        eprintln!(
            "n={n}: revalidated read {revalidated:?}; resolve-per-read read {per_read:?} ({:.0}x)",
            per_read.as_secs_f64() / revalidated.as_secs_f64().max(f64::MIN_POSITIVE)
        );
    }
    let fx = fixture();
    let session = session_on(&fx, faux()).await;
    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    let _ = session.available_model_catalog();
    let started = Instant::now();
    for _ in 0..200 {
        let _ = session.available_model_catalog();
    }
    eprintln!(
        "warm /model listing (available_model_catalog), 1 composed id: {:?}",
        started.elapsed() / 200
    );
}
