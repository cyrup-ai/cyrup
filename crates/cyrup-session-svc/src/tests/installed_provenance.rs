//! SEAM-171 and SEAM-173 (and the filed SEAM-174) — what the session lists for the id of the
//! provider it has installed, and what that provider streams, depend on where it came from.
//!
//! pi composes ONE provider per id: `base = nativeExtensionProviders.get(id) ?? builtins.get(id)`
//! (`coding-agent/src/core/model-runtime.ts:302` @f1b2e77f5), then `models.json` and an extension
//! registration over it (`:303-315`). So a provider an embedder supplies under a built-in id IS that
//! id's provider: its rows are the id's rows, and the compiled-in rows of the id are not there at
//! all. The three provenances an installed provider can have, and what the session's physical
//! registry ([`crate::session::compose_physical_registry`]) lists for its id:
//!
//! | provenance | how it got into the slot | the id's rows |
//! |---|---|---|
//! | guest | `registerProvider` / a live registration | the registration's (PROV-162) |
//! | the session's own built-in | the binary's launch provider, a `/model` select, a base handed back (SEAM-168) | the compiled-in rows over the overlay, `models.json` over them |
//! | an embedder's | injected at build, or returned by a resolver that does not resolve the built-ins | the provider's own rows, with the session's `models.json` block composed over the provider itself (SEAM-173) |
//!
//! **No network.** Nothing here issues a request: the "stream" is the session's own
//! `ProviderSwap`, and the provider is a faux that records the model each request carries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_agent::{Context, StreamFn, StreamOptions};
use cyrup_core::ModelRef;
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider, FauxResponseMeta};
use cyrup_provider::{Model, Provider};
use futures::StreamExt;
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

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

fn config(fx: &Fx) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// An embedder's provider under the BUILT-IN id `anthropic`, with one model of its own, recording
/// the id of the model every request it is asked to stream carries.
fn embedders_anthropic(requested: Arc<Mutex<Vec<String>>>) -> Arc<dyn Provider> {
    Arc::new(FauxProvider::with_config(FauxConfig {
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        models: vec![FauxModelDefinition::new("embedder-only")],
        on_response: Some(Arc::new(move |_meta: &FauxResponseMeta, model: &Model| {
            requested
                .lock()
                .unwrap()
                .push(model.id.as_str().to_string());
        })),
        ..FauxConfig::default()
    }))
}

/// A session over an injected provider, with no resolver: the embedder's shape.
async fn session_on(fx: &Fx, installed: Arc<dyn Provider>) -> AgentSession {
    SessionBuilder::new(installed, config(fx))
        .build()
        .await
        .expect("session builds")
}

fn ids_of(rows: &[Model], provider: &str) -> Vec<String> {
    rows.iter()
        .filter(|m| m.provider.as_str() == provider)
        .map(|m| m.id.as_str().to_string())
        .collect()
}

/// Ask the session's stream source for `row`, as the agent loop does for the selected model, and
/// answer which model the provider was asked to stream.
///
/// The stream is DRIVEN to its end: a composed provider (SEAM-173, `models.json` over the
/// embedder's) resolves its auth before it hands the request to the provider it was composed over,
/// so nothing reaches the faux until the stream is polled.
async fn streamed_as(
    session: &AgentSession,
    requested: &Mutex<Vec<String>>,
    row: &Model,
) -> String {
    requested.lock().unwrap().clear();
    let model = ModelRef {
        provider: row.provider.clone(),
        api: Some(row.api.clone()),
        model: row.id.clone(),
    };
    let events: Vec<_> = session
        .provider_swap()
        .stream(&model, &Context::default(), &StreamOptions::default())
        .collect()
        .await;
    requested
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("nothing was streamed for {}: {events:?}", row.id.as_str()))
}

/// Done-means (2) of SEAM-171. An injected provider `anthropic` with one model: `/model` lists that
/// one under `anthropic`, and every listed `anthropic` id streams as itself.
///
/// RED at `01aba2bb` (and with the fix's filter removed): `/model` listed every compiled-in
/// `anthropic` row next to `embedder-only` (`compose_physical_registry` filled the id in from
/// `default_models` first-wins), and each of them streamed as `embedder-only` —
/// `ProviderStreamFn` falls back to the provider's first row for an id it does not have.
#[tokio::test]
async fn an_embedders_provider_under_a_builtin_id_lists_only_its_own_rows() {
    let fx = fixture();
    // `anthropic` is configured, so `/model`'s auth filter keeps every row of the id: what is
    // listed is decided by the registry alone.
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        serde_json::json!({ "anthropic": { "type": "api_key", "key": "sk-ant-fixture" } })
            .to_string(),
    )
    .unwrap();
    let requested = Arc::new(Mutex::new(Vec::new()));
    let session = session_on(&fx, embedders_anthropic(Arc::clone(&requested))).await;

    // What a user sees first: a listed id that streams as another model.
    let listed = session.available_model_catalog();
    let mut misrouted: Vec<String> = Vec::new();
    for row in listed.iter().filter(|m| m.provider.as_str() == "anthropic") {
        let streamed = streamed_as(&session, &requested, row).await;
        if streamed != row.id.as_str() {
            misrouted.push(format!("{} -> {streamed}", row.id.as_str()));
        }
    }
    assert!(
        misrouted.is_empty(),
        "every listed anthropic id streams as itself; these stream as another model: {misrouted:?}"
    );
    assert_eq!(
        ids_of(&listed, "anthropic"),
        vec!["embedder-only".to_string()],
        "/model lists that one"
    );
    assert_eq!(
        ids_of(&session.full_model_catalog(), "anthropic"),
        vec!["embedder-only".to_string()],
        "the registry lists the embedder's rows as the id's rows, and no compiled-in row"
    );

    // Only the installed id is the embedder's: every other built-in id keeps its compiled-in rows.
    assert!(
        !ids_of(&session.full_model_catalog(), "groq").is_empty(),
        "the other built-in ids are still listed"
    );
}

/// GUARD (green at `01aba2bb`) — an embedder's provider under an id NO built-in has lists its own
/// rows; there is nothing compiled-in to drop.
#[tokio::test]
async fn an_embedders_provider_under_a_fresh_id_lists_its_rows() {
    let fx = fixture();
    let installed: Arc<dyn Provider> = Arc::new(FauxProvider::with_config(FauxConfig {
        provider: "acme".into(),
        models: vec![
            FauxModelDefinition::new("acme-1"),
            FauxModelDefinition::new("acme-2"),
        ],
        ..FauxConfig::default()
    }));
    let session = session_on(&fx, installed).await;
    assert_eq!(
        ids_of(&session.full_model_catalog(), "acme"),
        vec!["acme-1".to_string(), "acme-2".to_string()]
    );
}

/// SEAM-173, done-means (1) — pi composes `models.json` over a native provider (`composeProvider(id)`
/// with `config`, `model-runtime.ts:301-315`), so a block that adds a model to the embedder's id
/// adds it to the provider that streams it. Filed by the SEAM-171 closure and committed
/// `#[ignore]`d; un-ignored as round 6 opened, RED at `a07a0e34` (`embedder-only` streamed for
/// `json-added`: the injected provider was stored as given, and `models.json` composed over the
/// LISTING only).
#[tokio::test]
async fn a_models_json_model_under_an_embedders_id_streams_as_itself() {
    let fx = fixture();
    std::fs::write(
        fx.agent_dir.join("models.json"),
        serde_json::json!({ "providers": { "anthropic": {
            "models": [{ "id": "json-added", "name": "Json Added" }]
        } } })
        .to_string(),
    )
    .unwrap();
    let requested = Arc::new(Mutex::new(Vec::new()));
    let session = session_on(&fx, embedders_anthropic(Arc::clone(&requested))).await;
    let listed = session.full_model_catalog();
    let added = listed
        .iter()
        .find(|m| m.provider.as_str() == "anthropic" && m.id.as_str() == "json-added")
        .unwrap_or_else(|| {
            panic!(
                "models.json's model is listed under the embedder's id: {:?}",
                ids_of(&listed, "anthropic")
            )
        });
    assert_eq!(streamed_as(&session, &requested, added).await, "json-added");
}

/// `models.json` as a file in the fixture's agent dir.
fn write_models_json(fx: &Fx, providers: serde_json::Value) {
    std::fs::write(
        fx.agent_dir.join("models.json"),
        serde_json::json!({ "providers": providers }).to_string(),
    )
    .unwrap();
}

/// `anthropic` configured in `auth.json`, so `/model`'s auth filter and a cross-provider select
/// decide nothing here.
fn configure_anthropic(fx: &Fx) {
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        serde_json::json!({ "anthropic": { "type": "api_key", "key": "sk-ant-fixture" } })
            .to_string(),
    )
    .unwrap();
}

/// SEAM-173, done-means (2) — a `models.json` OVERRIDE of a model the embedder DOES declare reaches
/// the provider that streams it, not only the listing. RED at `a07a0e34`: `/model` listed the
/// renamed row (the listing's top layer), but the request the provider was handed carried the
/// embedder's own row (`embedder-only`, context window 128000).
#[tokio::test]
async fn a_models_json_override_of_an_embedders_model_reaches_the_streaming_provider() {
    let fx = fixture();
    write_models_json(
        &fx,
        serde_json::json!({ "anthropic": { "modelOverrides": {
            "embedder-only": { "name": "Renamed by models.json", "contextWindow": 4242 }
        } } }),
    );
    let handed = Arc::new(Mutex::new(Vec::<(String, u64)>::new()));
    let recorder = Arc::clone(&handed);
    let installed: Arc<dyn Provider> = Arc::new(FauxProvider::with_config(FauxConfig {
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        models: vec![FauxModelDefinition::new("embedder-only")],
        on_response: Some(Arc::new(move |_meta: &FauxResponseMeta, model: &Model| {
            recorder
                .lock()
                .unwrap()
                .push((model.name.clone(), model.context_window));
        })),
        ..FauxConfig::default()
    }));
    let session = session_on(&fx, installed).await;

    let listed = session.full_model_catalog();
    let row = listed
        .iter()
        .find(|m| m.provider.as_str() == "anthropic" && m.id.as_str() == "embedder-only")
        .unwrap();
    assert_eq!(
        (row.name.as_str(), row.context_window),
        ("Renamed by models.json", 4242),
        "/model lists the override"
    );
    let model = ModelRef {
        provider: row.provider.clone(),
        api: Some(row.api.clone()),
        model: row.id.clone(),
    };
    let _: Vec<_> = session
        .provider_swap()
        .stream(&model, &Context::default(), &StreamOptions::default())
        .collect()
        .await;
    assert_eq!(
        handed.lock().unwrap().as_slice(),
        [("Renamed by models.json".to_string(), 4242)],
        "the provider that streams is handed the overridden row"
    );
}

/// SEAM-173, done-means (3) — the third view. Codemode's registry (`compose_codemode_models`,
/// the installed provider set LAST over the composed built-ins) lists for the embedder's id what
/// `/model` lists, and the provider it holds for the id streams the added model as itself. RED at
/// `a07a0e34`: codemode listed `["embedder-only"]` (the raw provider replaced the composed
/// `anthropic`) while `/model` listed `["embedder-only", "json-added"]`.
#[tokio::test]
async fn codemode_lists_what_model_lists_for_an_embedders_id() {
    let fx = fixture();
    write_models_json(
        &fx,
        serde_json::json!({ "anthropic": {
            "models": [{ "id": "json-added", "name": "Json Added" }]
        } }),
    );
    let requested = Arc::new(Mutex::new(Vec::new()));
    let session = session_on(&fx, embedders_anthropic(Arc::clone(&requested))).await;

    let model_view = ids_of(&session.full_model_catalog(), "anthropic");
    let codemode = session.codemode_models_now();
    let codemode_view = ids_of(&codemode.get_models(Some("anthropic")), "anthropic");
    assert_eq!(
        model_view,
        vec!["embedder-only".to_string(), "json-added".to_string()],
        "/model"
    );
    assert_eq!(codemode_view, model_view, "codemode agrees with /model");

    let added = codemode.get_model("anthropic", "json-added").unwrap();
    requested.lock().unwrap().clear();
    let _: Vec<_> = codemode
        .get_provider("anthropic")
        .unwrap()
        .stream(&added, &Context::default(), &StreamOptions::default())
        .collect()
        .await;
    assert_eq!(
        requested.lock().unwrap().as_slice(),
        ["json-added".to_string()],
        "codemode's provider streams the added model as itself"
    );
}

/// GUARD (control, green at `a07a0e34` and after) — an embedder's provider under an id
/// `models.json` does NOT mention is stored as given: the composition is a layer over the
/// provider, not a replacement of it, so with no block there is nothing to compose.
#[tokio::test]
async fn an_embedders_provider_under_an_id_models_json_does_not_mention_is_stored_as_given() {
    let fx = fixture();
    write_models_json(
        &fx,
        serde_json::json!({ "groq": { "modelOverrides": {
            "llama-3.3-70b-versatile": { "name": "Renamed groq row" }
        } } }),
    );
    let requested = Arc::new(Mutex::new(Vec::new()));
    let injected = embedders_anthropic(Arc::clone(&requested));
    let session = session_on(&fx, Arc::clone(&injected)).await;
    assert!(
        Arc::ptr_eq(&session.provider_swap().current(), &injected),
        "the slot holds the injected provider itself"
    );
    assert_eq!(
        ids_of(&session.full_model_catalog(), "anthropic"),
        vec!["embedder-only".to_string()]
    );
}

/// A resolver of the embedder's own: it does not resolve the built-ins, and hands back
/// `provider` for its id.
struct EmbeddersResolver {
    provider: Arc<dyn Provider>,
}

impl crate::ProviderResolver for EmbeddersResolver {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        (self.provider.id().as_str() == provider_id)
            .then(|| Arc::clone(&self.provider))
            .ok_or_else(|| format!("no provider '{provider_id}'"))
    }
}

/// SEAM-173 — the OTHER arrival of an embedder's provider: its resolver's return, on a
/// cross-provider `/model` select (`ProviderSwap::resolve_and_store`) and on an extension's model
/// call to a provider the session is not on (`ProviderSwap::resolve`, EXT-086). Both are composed
/// with `models.json` by the same composer. RED at `a07a0e34`: `resolve` handed back the raw
/// provider (no `json-added` row), and after the select `json-added` streamed as `embedder-only`.
#[tokio::test]
async fn a_resolver_returned_embedders_provider_is_composed_with_models_json() {
    let fx = fixture();
    configure_anthropic(&fx);
    write_models_json(
        &fx,
        serde_json::json!({ "anthropic": {
            "models": [{ "id": "json-added", "name": "Json Added" }]
        } }),
    );
    let requested = Arc::new(Mutex::new(Vec::new()));
    let resolver = Arc::new(EmbeddersResolver {
        provider: embedders_anthropic(Arc::clone(&requested)),
    });
    let starts_on: Arc<dyn Provider> = Arc::new(FauxProvider::with_config(FauxConfig {
        provider: "acme".into(),
        models: vec![FauxModelDefinition::new("acme-1")],
        ..FauxConfig::default()
    }));
    let session = SessionBuilder::new(starts_on, config(&fx))
        .provider_resolver(resolver)
        .build()
        .await
        .expect("session builds");

    let reached = session.provider_swap().resolve("anthropic").unwrap();
    assert!(
        reached.get_model("json-added").is_some(),
        "an extension's model call reaches the composed provider: {:?}",
        reached
            .models()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
    );

    session.set_model("anthropic/json-added").await.unwrap();
    let added = session
        .full_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == "anthropic" && m.id.as_str() == "json-added")
        .unwrap();
    assert_eq!(
        streamed_as(&session, &requested, &added).await,
        "json-added"
    );
}

/// An embedder's provider that refreshes its own catalog (pi `Provider.refreshModels`), counting
/// the refreshes it is asked for. Everything else is the faux it wraps.
struct RefreshingEmbedder {
    inner: FauxProvider,
    refreshed: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for RefreshingEmbedder {
    fn id(&self) -> &cyrup_core::ProviderId {
        self.inner.id()
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> cyrup_core::EventStream<cyrup_provider::StreamEvent> {
        self.inner.stream(model, context, options)
    }
    fn has_refresh_models(&self) -> bool {
        true
    }
    async fn refresh_models(
        &self,
        _ctx: &cyrup_provider::RefreshModelsContext,
    ) -> Option<Result<(), cyrup_provider::ProviderError>> {
        self.refreshed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(Ok(()))
    }
}

/// SEAM-174 (filed by this round, OPEN) — pi's `/model` refresh is ONE `modelRuntime.refresh()`
/// over the composed collection (`modes/interactive/model-catalog-refresh.ts:46-51`), whose
/// `Models.refresh` runs `refreshModels` on EVERY provider that has one (`models.ts:546-606`),
/// a native extension's included (`nativeExtensionProviders`, `core/model-runtime.ts:298`). cyrup's
/// `refresh_model_catalogs` runs the pi.dev half and the guest registry's `refresh_all`; the
/// provider in the installed slot, when it is an embedder's, is in neither, so it is never asked.
///
/// Pinned, not fixed: `#[ignore]`d so the suite stays green; run it with `--run-ignored only`.
/// RED at this round's HEAD (`0` refreshes).
#[tokio::test]
#[ignore = "SEAM-174: open — an embedder's provider is never refreshed by the session's refresh"]
async fn the_sessions_refresh_reaches_an_embedders_refreshing_provider() {
    let fx = fixture();
    let refreshed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let installed: Arc<dyn Provider> = Arc::new(RefreshingEmbedder {
        inner: FauxProvider::with_config(FauxConfig {
            provider: "acme".into(),
            models: vec![FauxModelDefinition::new("acme-1")],
            ..FauxConfig::default()
        }),
        refreshed: Arc::clone(&refreshed),
    });
    let session = session_on(&fx, installed).await;
    let _ = session
        .refresh_model_catalogs(cyrup_core::CancelToken::new())
        .await;
    assert_eq!(
        refreshed.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the session's `/model` refresh asks the embedder's provider to refresh"
    );
}
