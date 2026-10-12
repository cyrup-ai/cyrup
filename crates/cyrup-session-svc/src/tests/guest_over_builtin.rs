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
    let registry = Arc::new(GuestProviderRegistry::new());
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
