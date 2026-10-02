//! H1 — a native extension's LIVE provider reaches the session's model registry, and replacing it
//! after startup changes what `/model` lists.
//!
//! pi: `registerProvider` folds the registered provider's models into the ONE `ModelRegistry` that
//! `getAvailable()` / `find()` / `setModel` read, and a later `registerProvider` of the same name
//! "replaces all models" (`core/model-registry.ts:917-940` @v0.84.1). cyrup holds the realized
//! providers in [`GuestProviderRegistry`]; its `generation` is the cache key of the composed-registry
//! snapshot (`session/model.rs::full_model_registry`, `session/model_runtime.rs`), so a replacement
//! that did not bump it would leave `/model` showing the old catalog.
//!
//! The extension reaches the registry two ways, both exercised here: `InitApi::register_provider_live`
//! during `init` (flushed when the sink binds) and `LateRegistrar::register_provider_live` /
//! `unregister_provider` after startup (a command handler or background task).
//!
//! **No network.** Nothing here issues a request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::ExtensionId;
use cyrup_ext::provider::{ModelRegistrySink, ProviderRegistration};
use cyrup_ext::{
    ExtError, HookOutcome, HostCtx, HostEvent, InitApi, LateRegistrar, NativeExtension,
};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{ConfigProvider, Model, Provider};
use tempfile::TempDir;

use crate::{AgentSession, GuestProviderRegistry, SessionBuilder, SessionConfig};

const PROVIDER: &str = "llama.cpp";

/// A live provider over a static catalog of `model_ids`, built the way the JSON door would realize
/// them (so the models carry a real `api` / `baseUrl`), but handed over as a finished `Arc`.
fn live_provider(model_ids: &[&str]) -> Arc<dyn Provider> {
    let reg = ProviderRegistration {
        id: PROVIDER.to_string(),
        config: serde_json::from_value(serde_json::json!({
            "name": "llama.cpp",
            "baseUrl": "http://127.0.0.1:8080/v1",
            "api": "openai-completions",
            "models": model_ids
                .iter()
                .map(|id| serde_json::json!({ "id": id, "name": id }))
                .collect::<Vec<_>>(),
        }))
        .unwrap(),
        resolved_api_key: None,
    };
    ConfigProvider::new(PROVIDER, "llama.cpp", None, reg.build_models()).into_arc()
}

/// Registers `initial` at `init` (when `Some`) and keeps its [`LateRegistrar`].
struct LiveNative {
    id: &'static str,
    initial: Option<Arc<dyn Provider>>,
    registrar: Mutex<Option<Arc<dyn LateRegistrar>>>,
    fail_after_register: bool,
}

impl LiveNative {
    fn new(id: &'static str, initial: Option<Arc<dyn Provider>>) -> Arc<Self> {
        Arc::new(Self {
            id,
            initial,
            registrar: Mutex::new(None),
            fail_after_register: false,
        })
    }

    fn registrar(&self) -> Arc<dyn LateRegistrar> {
        self.registrar.lock().unwrap().clone().unwrap()
    }
}

#[async_trait::async_trait]
impl NativeExtension for LiveNative {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        if let Some(p) = &self.initial {
            api.register_provider_live(PROVIDER, Arc::clone(p));
        }
        if self.fail_after_register {
            // A second registration whose id disagrees with the provider's own identity: refused
            // AFTER the first one landed, which fails the load.
            api.register_provider_live("someone-else", live_provider(&["x"]));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn set_late_registrar(&self, registrar: Arc<dyn LateRegistrar>) {
        *self.registrar.lock().unwrap() = Some(registrar);
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

async fn session_with(fx: &Fx, ext: Arc<LiveNative>) -> AgentSession {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(ext as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("session builds")
}

/// Store an api-key credential for [`PROVIDER`] so its auth strategy reports it configured.
///
/// H3: a live provider that carries an auth strategy is available exactly when that strategy's
/// check says so (`session/model.rs::provider_is_available`, pi `checkAuth`,
/// `core/model-runtime.ts:334-362`). [`live_provider`] is a [`ConfigProvider`], whose strategy is
/// the generic env-key one, so a stored credential is what makes it configured. The availability
/// rule itself is pinned in `live_provider_auth.rs`; these tests are about registry wiring.
async fn configure(session: &AgentSession) {
    session
        .services()
        .auth
        .modify(&cyrup_core::ProviderId::from(PROVIDER), |_| async {
            Ok(Some(cyrup_config::Credential::ApiKey {
                key: Some("test-key".to_string()),
                env: None,
            }))
        })
        .await
        .unwrap();
}

fn llama_ids(models: &[Model]) -> Vec<String> {
    let mut ids: Vec<String> = models
        .iter()
        .filter(|m| m.provider.as_str() == PROVIDER)
        .map(|m| m.id.as_str().to_string())
        .collect();
    ids.sort();
    ids
}

/// The registry half alone: `upsert_live_provider` stores the `Arc` as given and bumps the
/// generation on every call, including a replacement whose catalog is identical — the `Arc` is what
/// the session streams through.
#[test]
fn upsert_live_provider_stores_the_arc_and_bumps_the_generation() {
    let registry = GuestProviderRegistry::new();
    let first = live_provider(&["a"]);
    let g0 = registry.generation();
    registry.upsert_live_provider(PROVIDER, Arc::clone(&first));
    let g1 = registry.generation();
    assert_ne!(g0, g1, "a first live registration is a mutation");
    assert!(Arc::ptr_eq(&registry.provider(PROVIDER).unwrap(), &first));
    assert!(registry.has_provider(PROVIDER));

    let second = live_provider(&["a"]);
    registry.upsert_live_provider(PROVIDER, Arc::clone(&second));
    assert_ne!(
        registry.generation(),
        g1,
        "replacing with an equal catalog still moves the cache key"
    );
    assert!(Arc::ptr_eq(&registry.provider(PROVIDER).unwrap(), &second));
    assert_eq!(registry.ids(), vec![PROVIDER.to_string()]);
}

/// A provider registered at `init` is in the composed registry and counts as available, and the
/// session streams through the extension's own `Arc`.
#[tokio::test]
async fn an_init_time_live_provider_is_in_the_models_the_session_lists() {
    let fx = fixture();
    let provider = live_provider(&["tiny", "big"]);
    let session = session_with(
        &fx,
        LiveNative::new("llama-ext", Some(Arc::clone(&provider))),
    )
    .await;
    configure(&session).await;

    assert_eq!(
        llama_ids(&session.full_model_catalog()),
        vec!["big", "tiny"]
    );
    assert_eq!(
        llama_ids(&session.available_model_catalog()),
        vec!["big", "tiny"],
        "a live provider whose auth strategy reports it configured has its models selectable"
    );
    assert!(Arc::ptr_eq(
        &session
            .services()
            .guest_providers
            .provider(PROVIDER)
            .unwrap(),
        &provider
    ));
}

/// THE headline: replacing the provider after startup (a command handler re-listing the server's
/// models) bumps the generation and the NEXT catalog read is the new catalog — models that vanished
/// are gone, new ones are there — while an unchanged read stays served from the snapshot.
#[tokio::test]
async fn replacing_a_live_provider_after_startup_changes_what_model_lists_see() {
    let fx = fixture();
    let ext = LiveNative::new("llama-ext", Some(live_provider(&["old-a", "old-b"])));
    let session = session_with(&fx, Arc::clone(&ext)).await;
    configure(&session).await;
    let services = session.services();

    assert_eq!(
        llama_ids(&session.full_model_catalog()),
        vec!["old-a", "old-b"]
    );
    let gen_before = services.guest_providers.generation();
    let snapshot_before = session.full_model_catalog();
    assert_eq!(
        snapshot_before.len(),
        session.full_model_catalog().len(),
        "stable between mutations"
    );

    let replacement = live_provider(&["new-a"]);
    ext.registrar()
        .register_provider_live(PROVIDER.to_string(), Arc::clone(&replacement))
        .unwrap();

    assert_ne!(
        services.guest_providers.generation(),
        gen_before,
        "the replacement bumped the snapshot cache key"
    );
    assert_eq!(
        llama_ids(&session.full_model_catalog()),
        vec!["new-a"],
        "/model's registry shows the NEW catalog, not the old one"
    );
    assert_eq!(llama_ids(&session.available_model_catalog()), vec!["new-a"]);
    assert!(Arc::ptr_eq(
        &services.guest_providers.provider(PROVIDER).unwrap(),
        &replacement
    ));
}

/// `LateRegistrar::unregister_provider` removes the provider's models from the lists.
#[tokio::test]
async fn unregistering_a_live_provider_removes_its_models_from_the_lists() {
    let fx = fixture();
    let ext = LiveNative::new("llama-ext", Some(live_provider(&["tiny"])));
    let session = session_with(&fx, Arc::clone(&ext)).await;
    assert_eq!(llama_ids(&session.full_model_catalog()), vec!["tiny"]);
    let gen_before = session.services().guest_providers.generation();

    assert!(ext.registrar().unregister_provider(PROVIDER).unwrap());

    assert_ne!(session.services().guest_providers.generation(), gen_before);
    assert!(llama_ids(&session.full_model_catalog()).is_empty());
    assert!(!session.services().guest_providers.has_provider(PROVIDER));
}

/// A provider registered for the FIRST time after startup (no `init`-time registration) lands too:
/// the `LateRegistrar` is a registration door, not only a replacement one.
#[tokio::test]
async fn a_live_provider_first_registered_after_startup_is_listed() {
    let fx = fixture();
    let ext = LiveNative::new("llama-ext", None);
    let session = session_with(&fx, Arc::clone(&ext)).await;
    assert!(llama_ids(&session.full_model_catalog()).is_empty());

    ext.registrar()
        .register_provider_live(PROVIDER.to_string(), live_provider(&["late"]))
        .unwrap();
    assert_eq!(llama_ids(&session.full_model_catalog()), vec!["late"]);
}

/// A native whose load FAILS after it registered a live provider leaves no trace in the session's
/// model registry (pi `discard`, `loader.ts:462-467`): the build is contained, and the failed owner's
/// provider is not in the catalogs.
#[tokio::test]
async fn a_native_that_fails_to_load_leaves_no_live_provider_behind() {
    let fx = fixture();
    let ext = Arc::new(LiveNative {
        id: "llama-ext",
        initial: Some(live_provider(&["ghost"])),
        registrar: Mutex::new(None),
        fail_after_register: true,
    });
    let session = session_with(&fx, ext).await;

    assert!(
        !session.services().guest_providers.has_provider(PROVIDER),
        "the failed owner's provider must not survive in the registry"
    );
    assert!(llama_ids(&session.full_model_catalog()).is_empty());
    assert!(
        !session
            .services()
            .ext_host
            .loaded_ids()
            .contains(&ExtensionId::from("llama-ext"))
    );
}
