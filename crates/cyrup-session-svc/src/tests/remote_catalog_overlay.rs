//! The runtime model-catalog overlay reaches a LIVE session (DRIFT-007).
//!
//! `crates/cyrup-provider/src/tests/remote_catalog.rs` proves the fetch/merge/failure semantics.
//! This file proves the wiring: that `<agent_dir>/models-store.json` — the file a background
//! refresh writes — is picked up by [`AgentSession::full_model_catalog`], the registry `/model`,
//! model resolution and every enumeration path actually read.
//!
//! **No network.** Nothing here issues a request: the store is written directly, exactly as a
//! completed refresh would have left it, and the session only ever reads it from disk.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::models_store::{ModelsStore, ModelsStoreEntry};
use crate::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

fn faux() -> Arc<FauxProvider> {
    Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1")],
        ..FauxConfig::default()
    }))
}

fn groq_model_json(id: &str, context_window: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": id,
        "api": "openai-completions",
        "provider": "groq",
        "baseUrl": "https://api.groq.com/openai/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": context_window,
        "maxTokens": 8192
    })
}

/// Write `<agent_dir>/models-store.json` the way a completed refresh would, with a `lastModified`
/// strictly newer than the built-in catalog manifest so the staleness guard keeps it.
async fn seed_store(agent_dir: &Path, models: Vec<serde_json::Value>) {
    let store = cyrup_config::models_store::FileModelsStore::new(
        agent_dir.join(cyrup_config::models_store::MODELS_STORE_FILE_NAME),
    );
    let newer = cyrup_provider::builtin_model_data_generated_at().unwrap() + 1;
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: models
                    .into_iter()
                    .map(|v| serde_json::from_value(v).unwrap())
                    .collect(),
                last_modified: Some(newer),
                checked_at: Some(newer),
                etag: Some("\"v1\"".into()),
            },
            None,
        )
        .await
        .unwrap();
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
    Fx { _tmp: tmp, cwd, agent_dir }
}

async fn catalog_for(fx: &Fx) -> Vec<cyrup_provider::Model> {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    SessionBuilder::new(faux(), cfg)
        .build()
        .await
        .expect("session builds")
        .full_model_catalog()
}

#[tokio::test]
async fn a_persisted_overlay_reaches_the_live_session_registry_without_removing_anything() {
    let fx = fixture();
    let before = catalog_for(&fx).await;
    let groq_before: Vec<String> = before
        .iter()
        .filter(|m| m.provider.as_str() == "groq")
        .map(|m| m.id.as_str().to_string())
        .collect();
    assert!(!groq_before.is_empty(), "groq ships an embedded catalog");
    assert!(
        !groq_before.iter().any(|id| id == "overlay-only-model"),
        "sanity: the remote-only model is not embedded"
    );

    // A background refresh completes and writes the cache...
    seed_store(
        &fx.agent_dir,
        vec![
            groq_model_json("overlay-only-model", 777_777),
            // ...and also re-states an EMBEDDED model with new metadata.
            groq_model_json(&groq_before[0], 999_999),
        ],
    )
    .await;

    // ...and the next session to be built sees it.
    let after = catalog_for(&fx).await;
    let groq_after: Vec<&cyrup_provider::Model> =
        after.iter().filter(|m| m.provider.as_str() == "groq").collect();

    // ADD.
    assert!(
        groq_after.iter().any(|m| m.id.as_str() == "overlay-only-model"),
        "the persisted overlay did not reach the session registry"
    );
    // REPLACE, in place.
    let replaced = groq_after
        .iter()
        .find(|m| m.id.as_str() == groq_before[0])
        .expect("the embedded model is still there");
    assert_eq!(replaced.context_window, 999_999);
    // FLOOR: nothing embedded disappeared, for groq or for anyone else.
    for id in &groq_before {
        assert!(
            groq_after.iter().any(|m| m.id.as_str() == id.as_str()),
            "the overlay removed embedded model {id}"
        );
    }
    assert_eq!(after.len(), before.len() + 1);
}

#[tokio::test]
async fn a_corrupt_or_stale_cache_leaves_the_session_exactly_as_it_is_today() {
    let fx = fixture();
    let baseline = catalog_for(&fx).await;
    let path = fx
        .agent_dir
        .join(cyrup_config::models_store::MODELS_STORE_FILE_NAME);

    // Corrupt file.
    std::fs::write(&path, "{ not json at all").unwrap();
    assert_eq!(catalog_for(&fx).await, baseline, "a corrupt cache changed the registry");

    // Well-formed but STALE: `lastModified` older than the built-in manifest, i.e. an overlay
    // persisted before an upgrade that refreshed the embedded catalogs (pi #7016).
    let store = cyrup_config::models_store::FileModelsStore::new(&path);
    let older = cyrup_provider::builtin_model_data_generated_at().unwrap() - 1;
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: vec![serde_json::from_value(groq_model_json("stale-model", 1)).unwrap()],
                last_modified: Some(older),
                checked_at: Some(older),
                etag: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        catalog_for(&fx).await,
        baseline,
        "a pre-upgrade overlay must be discarded whole, not layered over newer embedded data"
    );

    // Well-formed and fresh but EMPTY — the case that would delete a provider if the overlay were a
    // replacement rather than a merge.
    let newer = cyrup_provider::builtin_model_data_generated_at().unwrap() + 1;
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: Vec::new(),
                last_modified: Some(newer),
                checked_at: Some(newer),
                etag: Some("\"empty\"".into()),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        catalog_for(&fx).await,
        baseline,
        "an empty overlay must be indistinguishable from no overlay"
    );
}
