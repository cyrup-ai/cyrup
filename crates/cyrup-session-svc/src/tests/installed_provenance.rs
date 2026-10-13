//! SEAM-171 (and the filed SEAM-173) — what the session lists for the id of the provider it has
//! installed depends on where that provider came from.
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
//! | an embedder's | injected at build, or returned by a resolver that does not resolve the built-ins | the provider's own rows alone (this file) |
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
fn streamed_as(session: &AgentSession, requested: &Mutex<Vec<String>>, row: &Model) -> String {
    requested.lock().unwrap().clear();
    let model = ModelRef {
        provider: row.provider.clone(),
        api: Some(row.api.clone()),
        model: row.id.clone(),
    };
    // The faux records the model in `stream` itself; the events are not needed.
    drop(
        session
            .provider_swap()
            .stream(&model, &Context::default(), &StreamOptions::default()),
    );
    requested
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("nothing was streamed for {}", row.id.as_str()))
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
    let misrouted: Vec<String> = listed
        .iter()
        .filter(|m| m.provider.as_str() == "anthropic")
        .filter_map(|row| {
            let streamed = streamed_as(&session, &requested, row);
            (streamed != row.id.as_str()).then(|| format!("{} -> {streamed}", row.id.as_str()))
        })
        .collect();
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

/// SEAM-173 (filed by this round, OPEN) — pi composes `models.json` over a native provider
/// (`composeProvider(id)` with `config`, `model-runtime.ts:301-310`), so a block that adds a model to
/// the embedder's id adds it to the provider that streams it. cyrup composes `models.json` over the
/// LISTING only (`compose_physical_registry`'s top layer); the injected provider itself is never
/// composed (`with_models_json_over_native` runs for live registrations, PROV-167), so the added
/// row is listed and streams the provider's first row.
///
/// Pinned, not fixed: `#[ignore]`d so the suite stays green; run it with `--run-ignored only`.
/// RED at this round's HEAD (`embedder-only` streamed for `json-added`).
#[tokio::test]
#[ignore = "SEAM-173: open — an embedder's provider is not composed with models.json"]
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
    assert_eq!(streamed_as(&session, &requested, added), "json-added");
}
