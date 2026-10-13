//! PROV-162 — a guest `registerProvider` under a BUILT-IN id composes over the built-in, in the
//! assembled session: a registration made at extension `init` (flushed when the model-registry
//! sink binds, BEFORE the session builds its installed-provider slot) over the id the session was
//! started on ends up as the provider the session streams through.
//!
//! pi has one registry: `registerProvider` recomposes the id over its base
//! (`core/model-runtime.ts:921-934` → `composeProvider`, `:301-310` @f1b2e77f5), and the session's
//! model resolves its provider from that registry, so there is no second, uncomposed copy.
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
use cyrup_provider::{
    InMemoryCredentialStore, Provider, anthropic_provider_with, builtin_registry,
};
use tempfile::TempDir;

use crate::{AgentSession, GuestProviderRegistry, ProviderSwap, SessionBuilder, SessionConfig};

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

async fn anthropic_session(fx: &Fx) -> AgentSession {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let installed: Arc<dyn Provider> = Arc::new(anthropic_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    ));
    SessionBuilder::new(installed, cfg)
        .with_native_extension(Arc::new(ProxyAnthropic) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("session builds")
}

/// A startup registration over the INSTALLED provider's id is what the session streams through:
/// the installed slot holds the composed provider (Anthropic's models at the proxy URL, Anthropic's
/// name), not the built-in it was started on.
///
/// RED at `360dc2bb`: the bind flush ran before `follow_installed`, so the slot kept the built-in
/// (`https://api.anthropic.com` rows); and the registration itself had no rows (no `models`).
#[tokio::test]
async fn a_startup_registration_over_the_installed_id_is_the_provider_the_session_streams() {
    let fx = fixture();
    let session = anthropic_session(&fx).await;
    let installed = session.provider_swap().current();
    assert_eq!(installed.id().as_str(), "anthropic");
    assert_eq!(installed.name(), "Anthropic");
    assert!(!installed.models().is_empty());
    assert!(
        installed
            .models()
            .iter()
            .all(|m| m.base_url == "http://proxy.test"),
        "{:?}",
        installed
            .models()
            .iter()
            .map(|m| m.base_url.as_str())
            .collect::<Vec<_>>()
    );
}

/// The registry half alone: [`GuestProviderRegistry::follow_installed`] adopts a provider that was
/// registered under the slot's id before the slot was bound.
///
/// RED at `360dc2bb`: `follow_installed` only recorded the slot.
#[test]
fn follow_installed_adopts_a_provider_registered_before_the_slot_was_bound() {
    let ext = cyrup_ext::registry::ExtensionRegistry::new();
    ext.register_provider(
        ExtensionId::from("acme-ext"),
        "acme",
        serde_json::json!({
            "baseUrl": "https://acme.test/v1",
            "api": "openai-completions",
            "apiKey": "sk-acme",
            "models": [{ "id": "acme-fast" }],
        }),
    )
    .unwrap();
    let registry = GuestProviderRegistry::new();
    ext.bind_model_registry(registry.clone()).unwrap();

    let started_on =
        cyrup_provider::ConfigProvider::new("acme", "Acme", None, Vec::new()).into_arc();
    let swap = Arc::new(ProviderSwap::new(started_on, None));
    registry.follow_installed(&swap);
    assert!(Arc::ptr_eq(
        &swap.current(),
        &registry.provider("acme").unwrap()
    ));
}

/// GUARD (green at `360dc2bb`) — a registration under an id NO built-in owns lands exactly as it
/// did: the base-less provider built from the registration alone, its rows listed, configured by
/// being registered.
#[tokio::test]
async fn a_registration_under_an_unknown_id_lands_as_before() {
    let fx = fixture();
    let session = anthropic_session(&fx).await;
    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("acme-ext"),
            "acme",
            serde_json::json!({
                "name": "Acme",
                "baseUrl": "https://acme.test/v1",
                "api": "openai-completions",
                "models": [{ "id": "acme-fast", "contextWindow": 64000, "maxTokens": 4096 }],
            }),
        )
        .unwrap();
    let rows: Vec<_> = session
        .full_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() == "acme")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id.as_str(), "acme-fast");
    assert_eq!(rows[0].base_url, "https://acme.test/v1");
    assert_eq!(rows[0].context_window, 64000);
    assert!(session.has_configured_auth(&rows[0]));
    let provider = session.services().guest_providers.provider("acme").unwrap();
    assert_eq!(provider.name(), "Acme");
}

// ---- PROV-164: a registration over a refreshable base still refreshes ----------------------

/// A one-route loopback gateway answering every request with `body`; returns its URL and the raw
/// requests it saw.
async fn serve_gateway(body: &'static str) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            sink.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).to_string());
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (format!("http://{addr}"), seen)
}

/// An auth context holding exactly `vars` and no files.
struct ScriptedEnv(Vec<(&'static str, &'static str)>);

#[async_trait::async_trait]
impl cyrup_provider::AuthContext for ScriptedEnv {
    async fn env(&self, name: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_string())
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

/// The gateway's `GET /v1/config` body: one model (radius-config.ts's shape).
const RADIUS_CONFIG: &str = r#"{"baseUrl":"https://api.example.test/v1","models":[{"id":"radius-1","name":"Radius One","reasoning":true,"input":["text"],"cost":{"input":1,"output":2,"cacheRead":0.1,"cacheWrite":0.2},"contextWindow":128000,"maxTokens":8192}]}"#;

/// PROV-164's Verify — a registration over radius still refreshes its catalog. pi's composed
/// provider has `refreshModels` whenever its base does and runs `await base.refreshModels(context)`
/// before publishing its own `update` (`provider-composer.ts:613-635` @f1b2e77f5); its `getAllModels`
/// reads the base live, so the refreshed rows are the composed provider's rows.
///
/// The base is what the session's resolver hands a registration: radius with the overlay restored
/// over it (`builder.rs`, `attach_base_resolver`). Here the overlay is read from radius's own store
/// at resolve time, which is radius's restore half (`providers/radius.rs`, "Where the catalog lives
/// in cyrup"); the production radius is built with no store and is static, see `PROV-014`'s
/// residual (2).
///
/// RED at `4f522804b`: the composed `WireProvider` reported `has_refresh_models() == false`, so the
/// refresh engine skipped the id (no request reached the gateway) and the composed rows stayed the
/// empty catalog the registration was made over.
#[tokio::test]
async fn a_registration_over_radius_still_refreshes_its_catalog() {
    let (gateway, seen) = serve_gateway(RADIUS_CONFIG).await;
    let store: Arc<dyn cyrup_provider::ModelsStore> =
        Arc::new(cyrup_provider::InMemoryModelsStore::new());
    let registry = GuestProviderRegistry::new();
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(ScriptedEnv(Vec::new())),
    );
    {
        let store = Arc::clone(&store);
        // The overlay handed in is ignored: this base restores from radius's own store instead.
        registry.attach_base_resolver(
            Arc::new(
                move |id: &str, _overlay: Option<&Arc<cyrup_provider::CatalogOverlay>>| {
                    (id == "radius").then(|| {
                        let radius: Arc<dyn Provider> = Arc::new(
                            cyrup_provider::RadiusProvider::new(
                                cyrup_provider::RadiusProviderOptions {
                                    gateway: Some(gateway.clone()),
                                    ..Default::default()
                                },
                                Arc::new(InMemoryCredentialStore::new()),
                                Arc::new(builtin_registry()),
                            )
                            .with_auth_context(Arc::new(ScriptedEnv(vec![(
                                "RADIUS_API_KEY",
                                "sk-gateway",
                            )])))
                            .with_models_store(Arc::clone(&store)),
                        );
                        futures::executor::block_on(
                            cyrup_provider::RemoteCatalog::new(Arc::clone(&store))
                                .load_overlay(&["radius"]),
                        )
                        .apply(radius)
                    })
                },
            ),
            Arc::new(cyrup_provider::CatalogOverlaySlot::new()),
        );
    }
    let ext = cyrup_ext::registry::ExtensionRegistry::new();
    ext.bind_model_registry(registry.clone()).unwrap();
    ext.register_provider(
        ExtensionId::from("proxy-ext"),
        "radius",
        serde_json::json!({ "baseUrl": "http://proxy.test", "apiKey": "sk-proxy" }),
    )
    .unwrap();

    let composed = registry.provider("radius").unwrap();
    assert!(
        composed.models().is_empty(),
        "radius ships no rows of its own"
    );
    assert!(
        composed.has_refresh_models(),
        "a provider composed over a refreshable base is refreshable"
    );

    let result = registry
        .refresh(cyrup_ext::host::services::ProviderRefreshRequest {
            providers: Some(vec!["radius".to_string()]),
            allow_network: Some(true),
            force: false,
            cancel: cyrup_core::CancelToken::new(),
        })
        .await;
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    while registry.refresh_tasks_in_flight() > 0 {
        tokio::task::yield_now().await;
    }

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with("GET /v1/config HTTP/1.1"),
        "{}",
        requests[0]
    );
    let persisted = store
        .read("radius", None)
        .await
        .unwrap()
        .expect("persisted");
    let persisted: Vec<&str> = persisted.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(persisted, ["radius-1"], "radius's own refresh ran");

    let rows = registry.provider("radius").unwrap().models().to_vec();
    let listed: Vec<(&str, &str)> = rows
        .iter()
        .map(|m| (m.id.as_str(), m.base_url.as_str()))
        .collect();
    assert_eq!(
        listed,
        [("radius-1", "http://proxy.test")],
        "the refreshed catalog is the composed provider's, at the registration's URL"
    );
}
