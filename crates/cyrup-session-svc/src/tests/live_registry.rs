//! SEAM-167, SEAM-168, SEAM-169, PROV-167 — the guest-provider/catalog seam, round 4: one provider
//! per id, read live.
//!
//! pi keeps ONE provider per id in its runtime, and the provider of a built-in id is the
//! `withRemoteCatalog`-wrapped built-in whose rows read the pi.dev catalog on every call
//! (`coding-agent/src/core/model-runtime.ts:228-233` @f1b2e77f5). Its registry checks a virtual
//! model against that live runtime (`:960`), and the session re-reads its current model from it
//! after each of the five provider actions (`core/agent-session.ts:3346-3357`, wired `:3470-3490`).
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
use cyrup_provider::{
    CatalogOverlay, InMemoryCredentialStore, Model, Provider, anthropic_provider_with,
    builtin_registry,
};
use tempfile::TempDir;

use crate::{AgentSession, ProviderResolver, SessionBuilder, SessionConfig};

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

/// The compiled-in anthropic, as the binary's launch path builds it with no overlay installed.
fn anthropic() -> Arc<dyn Provider> {
    Arc::new(anthropic_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    ))
}

fn config(fx: &Fx) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// The binary's resolver, in the shape that matters here: the providers it resolves are the
/// built-ins (and `models.json` providers) the session composes itself — `BuiltinProviderResolver`
/// over `select_provider` (`crates/cyrup/src/provider.rs`).
struct Builtins;

impl ProviderResolver for Builtins {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        let (models, _errors) = cyrup_config::compose_provider_registry(
            &cyrup_config::ModelFile::default(),
            cyrup_provider::CreateModelsOptions::default(),
        );
        models
            .get_provider(provider_id)
            .ok_or_else(|| format!("unknown provider {provider_id}"))
    }

    fn resolves_builtins(&self) -> bool {
        true
    }
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

/// A session built the way the binary builds one: the launch provider injected, the built-in
/// resolver wired.
async fn launched_on(fx: &Fx, installed: Arc<dyn Provider>) -> AgentSession {
    SessionBuilder::new(installed, config(fx))
        .provider_resolver(Arc::new(Builtins))
        .build()
        .await
        .expect("session builds")
}

fn register(session: &AgentSession, id: &str, config: serde_json::Value) {
    session
        .services()
        .ext_host
        .registry()
        .register_provider(ExtensionId::from("proxy-ext"), id, config)
        .unwrap();
}

/// One groq chat row, as the pi.dev catalog would publish it.
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

/// One anthropic chat row, as the pi.dev catalog would publish it.
fn anthropic_row(id: &str, name: &str) -> Model {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
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

// ---- SEAM-167: a provider action reaches the current model -----------------------------------

/// Done-means (a). A session on `anthropic/claude-fable-5`, then `registerProvider("anthropic", {
/// baseUrl })` mid-session: the session's CURRENT model — the row compaction, the limits, the
/// thinking clamp and the attribution headers read — carries the new URL, and so does the row an
/// extension's `ctx.model` reads (the host services' mirror), with no session read in between
/// (pi `_refreshCurrentModelFromRegistry` right after the action, `agent-session.ts:3470-3490`).
///
/// RED at `83daabf4`: `compaction_model` was written only at construction and by `set_model`, so
/// both stayed at `https://api.anthropic.com` while the next request already went to the proxy.
#[tokio::test]
async fn a_registration_over_the_current_models_provider_reaches_the_current_model() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await.into_shared();
    session.set_model("anthropic/claude-fable-5").await.unwrap();
    let before = session.limits_model().await.unwrap();
    assert_eq!(before.base_url, "https://api.anthropic.com", "fixture");

    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );

    // The extension's view first: nothing on the session has been read since the registration.
    let mirrored = cyrup_ext::host::HostServices::current_model_info(
        session.services().host_services.as_ref(),
    )
    .unwrap();
    assert_eq!(
        mirrored["baseUrl"], "http://proxy.test",
        "ctx.model is re-read right after the provider action"
    );
    let current = session.limits_model().await.unwrap();
    assert_eq!(current.id.as_str(), "claude-fable-5");
    assert_eq!(current.base_url, "http://proxy.test");
    assert_eq!(
        session
            .provider_swap()
            .current()
            .get_model("claude-fable-5")
            .map(|m| m.base_url.as_str()),
        Some("http://proxy.test"),
        "the row the request carries is the same row"
    );

    // An unregistration is a provider action too: the base's row comes back.
    session
        .services()
        .ext_host
        .registry()
        .unregister_provider("anthropic")
        .unwrap();
    assert_eq!(
        session.limits_model().await.unwrap().base_url,
        "https://api.anthropic.com"
    );
}

/// The session's own readers re-read on their own (no listener: a by-value session nobody shares):
/// a registration, then the limits model, the thinking clamp's model and the supported levels
/// read the registration's row.
///
/// RED with the re-read disabled (`current_model` returning the stored row): the limits model
/// stayed at `https://api.anthropic.com`.
#[tokio::test]
async fn a_registration_reaches_the_current_model_of_a_session_nobody_shares() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await;
    session.set_model("anthropic/claude-fable-5").await.unwrap();
    register(
        &session,
        "anthropic",
        serde_json::json!({
            "baseUrl": "http://proxy.test",
            "apiKey": "sk-proxy",
            "models": [{ "id": "claude-fable-5", "reasoning": false }]
        }),
    );
    let current = session.limits_model().await.unwrap();
    assert_eq!(current.base_url, "http://proxy.test");
    assert!(!current.reasoning);
    assert!(
        !session.supports_thinking(),
        "the thinking clamp reads the re-read row"
    );
}

/// `[CYRUP-DELTA]` beyond pi (which re-reads only after a provider action): a pi.dev reload that
/// changes the current model's row reaches the current model too, because the REQUEST follows the
/// reload here (the installed built-in is rebuilt over it) and the limits must be the row the
/// request carries.
///
/// RED with the installed built-in not followed, and with the re-read disabled.
#[tokio::test]
async fn an_overlay_reload_reaches_the_current_model() {
    let fx = fixture();
    let session = launched_on(&fx, anthropic()).await;
    session.set_model("anthropic/claude-fable-5").await.unwrap();
    let mut reloaded = anthropic_row("claude-fable-5", "Claude Fable 5 (reloaded)");
    reloaded.context_window = 1_234_567;
    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "anthropic".to_string(),
            vec![reloaded],
        )]));
    let current = session.limits_model().await.unwrap();
    assert_eq!(current.context_window, 1_234_567);
    assert_eq!(
        session
            .provider_swap()
            .current()
            .get_model("claude-fable-5")
            .map(|m| m.context_window),
        Some(1_234_567)
    );
}

// ---- SEAM-168: the installed built-in reads the overlay as it is now -------------------------

/// Done-means (b) — SEAM-168's probe, committed. A session launched the binary's way on the
/// compiled-in anthropic; then a pi.dev catalog reload that RENAMES `claude-fable-5` and ADDS
/// `claude-new-only`. `/model` lists the reloaded rows, and the provider the session streams
/// through resolves every anthropic id `/model` lists to the row listed (`ProviderStreamFn`
/// resolves a request's row with `get_model`, and falls back to the FIRST row for an id it does not
/// have).
///
/// RED with the installed built-in not followed (`follow_installed_builtin` a no-op): `/model`
/// listed `"Claude Fable 5"` (the installed provider's row shadowed the reloaded one, first-wins)
/// and listed `claude-new-only`, which the installed provider did not have.
#[tokio::test]
async fn an_overlay_reload_reaches_the_installed_builtin() {
    let fx = fixture();
    let session = launched_on(&fx, anthropic()).await;
    assert_eq!(
        rows_of(&session, "anthropic")
            .iter()
            .find(|m| m.id.as_str() == "claude-fable-5")
            .map(|m| m.name.as_str()),
        Some("Claude Fable 5"),
        "fixture"
    );

    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "anthropic".to_string(),
            vec![
                anthropic_row("claude-fable-5", "Claude Fable 5 (reloaded)"),
                anthropic_row("claude-new-only", "Claude New Only"),
            ],
        )]));

    let listed: Vec<Model> = session
        .available_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() == "anthropic")
        .collect();
    let named = |id: &str| {
        listed
            .iter()
            .find(|m| m.id.as_str() == id)
            .map(|m| m.name.clone())
    };
    assert_eq!(
        named("claude-fable-5").as_deref(),
        Some("Claude Fable 5 (reloaded)"),
        "/model lists the reloaded row"
    );
    assert_eq!(named("claude-new-only").as_deref(), Some("Claude New Only"));
    let installed = session.provider_swap().current();
    for row in &listed {
        assert_eq!(
            installed.get_model(row.id.as_str()),
            Some(row),
            "the installed provider streams {} as listed",
            row.id.as_str()
        );
    }
}

/// GUARD (green at `83daabf4`) — the same reload on a session whose provider is an EMBEDDER's (no
/// resolver declares it a built-in): the provider is never replaced, it is the id's provider on its
/// own terms (pi's native-provider precedence).
#[tokio::test]
async fn an_overlay_reload_does_not_replace_an_embedders_provider() {
    let fx = fixture();
    let injected = anthropic();
    let session = session_on(&fx, Arc::clone(&injected)).await;
    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "anthropic".to_string(),
            vec![anthropic_row("claude-new-only", "Claude New Only")],
        )]));
    let _ = session.available_model_catalog();
    assert!(Arc::ptr_eq(&session.provider_swap().current(), &injected));
}

/// A cross-provider `/model` select of a built-in installs the session's own built-in, which then
/// follows the next reload too (the in-session half of SEAM-168: the swap's resolver used to hand
/// back a provider made over the overlay of the moment, which nothing rebuilt).
///
/// RED with the installed built-in not followed: `claude-new-only` is listed and not installed.
#[tokio::test]
async fn a_builtin_selected_mid_session_follows_the_next_reload() {
    let fx = fixture();
    // `--api-key` for anthropic: the runtime key the binary installs on the session's store.
    let auth = Arc::new(cyrup_config::AuthStore::at(fx.agent_dir.join("auth.json")));
    auth.set_runtime_api_key(
        cyrup_core::ProviderId::from("anthropic"),
        "sk-cli".to_string(),
    );
    let session = SessionBuilder::new(
        Arc::new(cyrup_provider::faux::FauxProvider::new()),
        config(&fx),
    )
    .auth(auth)
    .provider_resolver(Arc::new(Builtins))
    .build()
    .await
    .expect("session builds");
    session.set_model("anthropic/claude-fable-5").await.unwrap();
    assert_eq!(session.provider_swap().current().id().as_str(), "anthropic");

    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "anthropic".to_string(),
            vec![anthropic_row("claude-new-only", "Claude New Only")],
        )]));
    assert!(ids(&rows_of(&session, "anthropic")).contains(&"claude-new-only".to_string()));
    assert!(
        session
            .provider_swap()
            .current()
            .get_model("claude-new-only")
            .is_some()
    );
}

// ---- SEAM-169: an extension's virtual model is checked against the live catalog --------------

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

fn virtual_model(provider: &str, id: &str) -> cyrup_provider::VirtualModelDefinition {
    cyrup_provider::VirtualModelDefinition::new(
        cyrup_provider::VirtualModelSpec {
            provider: provider.into(),
            id: id.into(),
            name: "Routed".to_string(),
            thinking_levels: None,
            context_window: None,
            max_tokens: None,
            input: None,
        },
        Arc::new(NoRoute),
    )
}

/// Done-means (c). An overlay that adds `groq/groq-new`, then an extension registering a virtual
/// `groq/groq-new`: refused as a physical conflict (pi `registerVirtualModel` checks
/// `this.models.getModel(providerId, id)` on its live runtime, `model-runtime.ts:957-961`).
///
/// RED at `83daabf4`: the extension path checked against the builder's startup catalog, composed
/// once with no overlay, so the registration was accepted and hid the physical row.
#[tokio::test]
async fn an_extensions_virtual_model_over_a_reloaded_physical_id_is_refused() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await;
    session
        .services()
        .catalog_overlay
        .install(CatalogOverlay::from_entries([(
            "groq".to_string(),
            vec![groq_row("groq-new")],
        )]));
    let registry = session.services().ext_host.registry();
    let refused = registry.register_virtual_model(
        ExtensionId::from("router-ext"),
        virtual_model("groq", "groq-new"),
        "router-ext",
    );
    let message = format!("{refused:?}");
    assert!(
        refused.is_err() && message.contains("conflicts with a physical model"),
        "refused as a physical conflict: {message}"
    );
    // CONTROL: an id nothing physical lists is accepted.
    registry
        .register_virtual_model(
            ExtensionId::from("router-ext"),
            virtual_model("groq", "groq-routed"),
            "router-ext",
        )
        .unwrap();
}

// ---- PROV-167: a JSON registration under a live provider's id, as pi does it ----------------

/// Done-means (d), read at the pin, and the row's premise does NOT hold there: pi's
/// `registerProvider` DELETES the native provider under the id before it composes
/// (`this.nativeExtensionProviders.delete(providerId)`, `core/model-runtime.ts:925` @f1b2e77f5), so
/// `composeProvider`'s base for the registration is `builtins.get(id)` — never the live provider.
/// `nativeExtensionProviders.get(id)` is the base of the `models.json` layer only. GUARD (green at
/// `83daabf4`): under an id no built-in owns the registration replaces the live provider and lists
/// what it declares (nothing, for `{ baseUrl }`), and the session streams the registration.
#[tokio::test]
async fn a_json_registration_under_a_live_id_replaces_the_live_provider() {
    let fx = fixture();
    let session = session_on(&fx, anthropic()).await;
    let live: Arc<dyn Provider> = Arc::new(cyrup_provider::faux::FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            provider: "acme-live".into(),
            models: vec![cyrup_provider::faux::FauxModelDefinition::new("acme-1")],
            ..cyrup_provider::faux::FauxConfig::default()
        },
    ));
    let registry = session.services().ext_host.registry();
    registry
        .register_provider_live(ExtensionId::from("native"), "acme-live", Arc::clone(&live))
        .unwrap();
    assert_eq!(ids(&rows_of(&session, "acme-live")), ["acme-1"], "fixture");

    register(
        &session,
        "acme-live",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    let guest = session
        .services()
        .guest_providers
        .provider("acme-live")
        .unwrap();
    assert!(!Arc::ptr_eq(&guest, &live), "the live provider is replaced");
    assert!(rows_of(&session, "acme-live").is_empty());
}

/// The same under a BUILT-IN id (a native that registered `anthropic` live): the JSON registration
/// composes over the built-in, as pi's `builtins.get(id)` base — the live rows are gone, the
/// built-in's are listed at the registration's URL, and the session streams that composition.
/// GUARD (green at `83daabf4`).
#[tokio::test]
async fn a_json_registration_under_a_live_builtin_id_composes_over_the_builtin() {
    let fx = fixture();
    let session = session_on(&fx, Arc::new(cyrup_provider::faux::FauxProvider::new())).await;
    let live: Arc<dyn Provider> = Arc::new(cyrup_provider::faux::FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            provider: "anthropic".into(),
            models: vec![cyrup_provider::faux::FauxModelDefinition::new("live-only")],
            ..cyrup_provider::faux::FauxConfig::default()
        },
    ));
    session
        .services()
        .ext_host
        .registry()
        .register_provider_live(ExtensionId::from("native"), "anthropic", live)
        .unwrap();
    assert_eq!(
        ids(&rows_of(&session, "anthropic")),
        ["live-only"],
        "fixture"
    );

    register(
        &session,
        "anthropic",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    let rows = rows_of(&session, "anthropic");
    assert!(ids(&rows).contains(&"claude-fable-5".to_string()));
    assert!(!ids(&rows).contains(&"live-only".to_string()));
    assert!(rows.iter().all(|m| m.base_url == "http://proxy.test"));
}

/// Where pi DOES compose over a native's provider: the `models.json` layer (`composeProvider`'s
/// `base = nativeExtensionProviders.get(id) ?? builtins.get(id)` with `config` and no extension,
/// `model-runtime.ts:301-310`). A `models.json` block for a live provider's id applies over its
/// rows, in the listing and in the provider the session would stream through.
///
/// RED at `83daabf4`: the live provider was stored as given and the listing strips a guest id's
/// block (SEAM-166), so the block reached nothing (`"acme-1"`, not `"Renamed by models.json"`).
#[tokio::test]
async fn a_models_json_block_composes_over_a_live_provider() {
    let fx = fixture();
    std::fs::write(
        fx.agent_dir.join("models.json"),
        r#"{ "providers": { "acme-live": {
            "baseUrl": "http://acme-proxy.test",
            "apiKey": "sk-acme",
            "modelOverrides": { "acme-1": { "name": "Renamed by models.json" } }
        } } }"#,
    )
    .unwrap();
    let session = session_on(&fx, anthropic()).await;
    let live: Arc<dyn Provider> = Arc::new(cyrup_provider::faux::FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            provider: "acme-live".into(),
            models: vec![cyrup_provider::faux::FauxModelDefinition::new("acme-1")],
            ..cyrup_provider::faux::FauxConfig::default()
        },
    ));
    session
        .services()
        .ext_host
        .registry()
        .register_provider_live(ExtensionId::from("native"), "acme-live", live)
        .unwrap();
    let rows = rows_of(&session, "acme-live");
    assert_eq!(ids(&rows), ["acme-1"]);
    assert_eq!(rows[0].name, "Renamed by models.json");
    assert_eq!(rows[0].base_url, "http://acme-proxy.test");
    let provider = session
        .services()
        .guest_providers
        .provider("acme-live")
        .unwrap();
    assert_eq!(provider.get_model("acme-1"), Some(&rows[0]));
}

// ---- the cost of reading live, measured --------------------------------------------------------

/// MEASUREMENT, not a check (ignored; run with `--run-ignored only`). The hot reads this round
/// puts a revalidation in front of, on a session launched the binary's way on anthropic with one
/// registration composed over openai:
///
/// * a warm `/model` listing (`available_model_catalog`);
/// * one per-request model resolution — what `ProviderSwap::stream` does before every request:
///   the installed provider, then `get_model` of the request's id;
/// * one read of the current model's row (`supports_thinking`, a `compaction_model` reader);
/// * one guest-registry read (round 3's figure, for comparison);
///
/// and the price paid when the overlay DOES move: a fresh overlay installed before each listing.
#[tokio::test]
#[ignore = "measurement; prints timings"]
async fn measure_the_hot_reads() {
    use std::time::Instant;
    let fx = fixture();
    let session = launched_on(&fx, anthropic()).await;
    register(
        &session,
        "openai",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    );
    let _ = session.available_model_catalog();

    let n = 200u32;
    let started = Instant::now();
    for _ in 0..n {
        let _ = session.available_model_catalog();
    }
    let listing = started.elapsed() / n;

    let n = 20_000u32;
    let swap = session.provider_swap();
    let started = Instant::now();
    for _ in 0..n {
        let provider = swap.current();
        assert!(provider.get_model("claude-fable-5").is_some());
    }
    let per_request = started.elapsed() / n;

    let started = Instant::now();
    for _ in 0..n {
        let _ = session.supports_thinking();
    }
    let current_model = started.elapsed() / n;

    let guest = &session.services().guest_providers;
    let started = Instant::now();
    for _ in 0..n {
        let _ = guest.models();
    }
    let guest_read = started.elapsed() / n;

    let n = 30u32;
    let started = Instant::now();
    for i in 0..n {
        session
            .services()
            .catalog_overlay
            .install(CatalogOverlay::from_entries([(
                "anthropic".to_string(),
                vec![anthropic_row(
                    "claude-fable-5",
                    &format!("Claude Fable 5 #{i}"),
                )],
            )]));
        let _ = session.available_model_catalog();
        let _ = session.supports_thinking();
    }
    let moved = started.elapsed() / n;

    eprintln!(
        "warm /model listing {listing:?}; per-request resolution {per_request:?}; \
         current-model read {current_model:?}; guest-registry read {guest_read:?}; \
         overlay moved, then listing + current-model read {moved:?}"
    );
}
