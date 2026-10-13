//! SEAM-170 — a session the binary builds reads THE process's catalog overlay slot.
//!
//! pi has one `ModelRuntime` whose remote catalog every reader shares: the built-ins are wrapped in
//! `withRemoteCatalog` (`coding-agent/src/core/model-runtime.ts:228-233` @f1b2e77f5), and the
//! detached startup `void modelRuntime.refresh()` (`main.ts:942`) refreshes that same runtime, so a
//! model the refresh finds is listed — and streamed — by the session at once.
//!
//! cyrup's background refresh ([`crate::provider::spawn_model_catalog_refresh_with`]) installs into
//! the process slot (`provider.rs` `CATALOG_OVERLAY`). Before SEAM-170, `build_factory` never wired
//! `provider::model_catalog_service`, so every session it built took the builder's no-service arm:
//! a PRIVATE slot loaded from disk at build that nothing reloaded.
//!
//! These tests drive the real `build_factory` (every mode arm's factory) and the real background
//! refresh function, pointed at a loopback origin through the one transport seam the subsystem has
//! (`RemoteCatalog::with_base_url`, pi's `catalogBaseUrl`). The end-to-end proof — a real binary
//! whose own background refresh installs the row, which `/model` then lists and a request then
//! streams — is `cyrup-it` `bin::catalog_refresh_reaches_the_session`.
//!
//! **No network.** Every request goes to `127.0.0.1:0`, with an empty `AuthContext` so an ambient
//! proxy cannot reroute it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_config::policy::NetworkPolicy;
use cyrup_config::{AppMode, AuthStore, CliConfigOverrides, ConfigDirs, EnvVars, ModelFile};
use cyrup_provider::auth::AuthContext;
use cyrup_provider::models_store::{InMemoryModelsStore, ModelsStore};
use cyrup_provider::remote_catalog::RemoteCatalog;
use cyrup_session_svc::{AgentSession, SessionConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The row only the refresh publishes.
const REFRESHED: &str = "groq-refreshed-only";

/// Answers every request with one groq catalog holding [`REFRESHED`], dated far after the
/// compiled-in catalogs so the staleness guard keeps it.
async fn origin() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = serde_json::json!([{
        "id": REFRESHED,
        "name": "Groq Refreshed Only",
        "api": "openai-completions",
        "baseUrl": "https://api.groq.com/openai/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": 131_072,
        "maxTokens": 8192
    }])
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\netag: \"r1\"\r\nlast-modified: Fri, 01 Jan 2100 00:00:00 GMT\r\n\
         content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}")
}

struct EmptyEnv;

#[async_trait::async_trait]
impl AuthContext for EmptyEnv {
    async fn env(&self, _name: &str) -> Option<String> {
        None
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

fn catalog(base_url: &str) -> Arc<RemoteCatalog> {
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::default());
    Arc::new(
        RemoteCatalog::new(store)
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(std::time::Duration::from_secs(5)),
    )
}

fn online() -> NetworkPolicy {
    NetworkPolicy {
        offline: false,
        update_check: false,
        install_telemetry: false,
        analytics: false,
    }
}

/// A session built exactly as the rpc arm builds one: launched on groq (a stored key) by
/// `select_provider`, through `build_factory`.
async fn rpc_session_on_groq() -> (AgentSession, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        serde_json::json!({ "groq": { "type": "api_key", "key": "sk-groq-fixture" } }).to_string(),
    )
    .unwrap();
    let env = EnvVars {
        home: Some(agent_dir.clone()),
        ..EnvVars::default()
    };
    let overrides = CliConfigOverrides {
        agent_dir: Some(agent_dir.clone()),
        cwd: Some(cwd.clone()),
        ..Default::default()
    };
    let dirs = ConfigDirs::resolve(&overrides, &env).unwrap();
    let auth = Arc::new(AuthStore::at(agent_dir.join("auth.json")));
    let provider = crate::provider::select_provider(
        Some("groq"),
        None,
        None,
        &ModelFile::default(),
        Some(cyrup_config::login::runtime_credentials(auth.clone())),
    )
    .unwrap();

    let mut config = SessionConfig::new(cwd, agent_dir);
    config.persist = false;
    config.trust_override = Some(true);
    config.no_extensions = true;
    config.app_mode = AppMode::Rpc;
    config.model_network_enabled = true;
    let target = config.target.clone();
    let factory = crate::session_launch::build_factory(
        provider,
        config,
        crate::file_settings_store(&dirs),
        auth,
        &dirs,
        Arc::new(ModelFile::default()),
        None,
    )
    .unwrap();
    (factory.build(target, None).await.unwrap(), tmp)
}

fn lists(session: &AgentSession, id: &str) -> bool {
    session
        .available_model_catalog()
        .iter()
        .any(|m| m.provider.as_str() == "groq" && m.id.as_str() == id)
}

/// Done-means (1) at the factory: a background refresh that installs a new row for the session's
/// provider AFTER the session exists — `/model` lists it, and selecting it resolves it (the
/// session's current model is the refreshed row).
///
/// RED at `01aba2bb`: the session's slot was its own copy, so the refresh landed in a slot no
/// session read and `groq-refreshed-only` was never listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_refresh_reaches_a_session_the_binary_built() {
    let (session, _tmp) = rpc_session_on_groq().await;
    assert!(
        !lists(&session, REFRESHED),
        "fixture: the row is not compiled in"
    );

    let base_url = origin().await;
    crate::provider::spawn_model_catalog_refresh_with(
        catalog(&base_url),
        online(),
        AppMode::Rpc,
        vec!["groq".to_string()],
    )
    .expect("an online rpc run refreshes")
    .await
    .expect("the refresh task does not panic");

    assert!(
        lists(&session, REFRESHED),
        "/model lists the row the background refresh installed"
    );
    session
        .set_model(&format!("groq/{REFRESHED}"))
        .await
        .expect("the refreshed row is selectable");
    let current = session.model().expect("a current model");
    assert_eq!(current.model.as_str(), REFRESHED);
}

/// The wiring itself: the slot a factory-built session reads IS the process slot, and the
/// session's own `/model` refresh has a pi.dev half (a catalog service) to run.
///
/// RED at `01aba2bb`: a private slot, and no catalog service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_the_binary_built_shares_the_process_catalog_slot() {
    let (session, _tmp) = rpc_session_on_groq().await;
    assert!(
        Arc::ptr_eq(
            &session.services().catalog_overlay,
            &crate::provider::process_catalog_overlay()
        ),
        "the session's overlay slot is the process slot"
    );
    let svc = session
        .services()
        .model_catalog
        .as_ref()
        .expect("the session has the binary's catalog service");
    assert!(Arc::ptr_eq(
        &svc.overlay(),
        &crate::provider::process_catalog_overlay()
    ));
}
