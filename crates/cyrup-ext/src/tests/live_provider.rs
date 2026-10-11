//! H1 — a NATIVE extension contributes a LIVE provider and can replace it after startup; a native
//! can be HIDDEN from the startup `[Extensions]` listing.
//!
//! pi: `registerProvider(name, config)` hands the `ModelRegistry` a full provider object, and a
//! later `registerProvider` of the same name replaces it ("replaces all models",
//! `core/model-registry.ts:919` @v0.84.1; `extensions/types.ts:1337,1363-1475`). pi marks every
//! `builtin:` extension `hidden` (`core/resource-loader.ts:729` @v0.99.2-17) and the startup panel
//! lists only `!extension.hidden` (`modes/interactive/interactive-mode.ts:1778`).
//!
//! cyrup's JSON `register_provider` could only describe a static `ProviderConfig`, so the hub always
//! rebuilt a `ConfigProvider`. These tests pin that the live door stores the extension's OWN `Arc`
//! (pointer-equal at the sink), that a post-`init` registration through the `LateRegistrar` replaces
//! it, that a failed load purges it, and that the JSON door behaves exactly as before.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::provider::{ModelRegistrySink, ProviderRegistration};
use crate::registry::ExtensionRegistry;
use crate::{
    ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    LateRegistrar, NativeExtension,
};
use cyrup_core::ExtensionId;
use cyrup_provider::{ConfigProvider, Provider};
use serde_json::json;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// A live provider whose identity is observable by pointer.
fn live(id: &str) -> Arc<dyn Provider> {
    ConfigProvider::new(id, id, None, Vec::new()).into_arc()
}

fn same(a: &Arc<dyn Provider>, b: &Arc<dyn Provider>) -> bool {
    Arc::ptr_eq(a, b)
}

/// A recording sink that keeps the exact `Arc`s it was handed, the way `GuestProviderRegistry` does.
#[derive(Default)]
struct Sink {
    events: Mutex<Vec<String>>,
    live: Mutex<BTreeMap<String, Arc<dyn Provider>>>,
}

impl Sink {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    fn live(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.live.lock().unwrap().get(id).cloned()
    }
}

impl ModelRegistrySink for Sink {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        self.live.lock().unwrap().remove(&reg.id);
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert:{}", reg.id));
    }
    fn upsert_live_provider(&self, id: &str, provider: Arc<dyn Provider>) {
        self.live.lock().unwrap().insert(id.to_string(), provider);
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert-live:{id}"));
    }
    fn remove_provider(&self, id: &str) {
        self.live.lock().unwrap().remove(id);
        self.events.lock().unwrap().push(format!("remove:{id}"));
    }
    fn upsert_virtual_model(
        &self,
        definition: &cyrup_provider::VirtualModelDefinition,
    ) -> Result<(), String> {
        self.events.lock().unwrap().push(format!(
            "upsert-virtual:{}/{}",
            definition.spec.provider.as_str(),
            definition.spec.id.as_str()
        ));
        Ok(())
    }
    fn remove_virtual_model(&self, provider: &str, id: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("remove-virtual:{provider}/{id}"));
    }
}

/// A native that registers live providers at `init` and stashes its [`LateRegistrar`] so a test can
/// drive the post-`init` verbs the way a command handler or background task would.
struct Native {
    id: ExtensionId,
    init_providers: Vec<(String, Arc<dyn Provider>)>,
    hidden: bool,
    registrar: Mutex<Option<Arc<dyn LateRegistrar>>>,
}

impl Native {
    fn new(id: &str, init_providers: Vec<(String, Arc<dyn Provider>)>) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            init_providers,
            hidden: false,
            registrar: Mutex::new(None),
        })
    }

    fn hidden(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            init_providers: Vec::new(),
            hidden: true,
            registrar: Mutex::new(None),
        })
    }

    fn registrar(&self) -> Arc<dyn LateRegistrar> {
        self.registrar
            .lock()
            .unwrap()
            .clone()
            .expect("the host binds the registrar before init")
    }
}

#[async_trait::async_trait]
impl NativeExtension for Native {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for (id, provider) in &self.init_providers {
            api.register_provider_live(id.clone(), Arc::clone(provider));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn is_hidden(&self) -> bool {
        self.hidden
    }
    fn set_late_registrar(&self, registrar: Arc<dyn LateRegistrar>) {
        *self.registrar.lock().unwrap() = Some(registrar);
    }
}

fn bound_host() -> (ExtensionHost, Arc<Sink>) {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    (host, sink)
}

// ---------------------------------------------------------------------------
// init-time registration
// ---------------------------------------------------------------------------

/// `InitApi::register_provider_live` reaches the bound model registry as the extension's OWN `Arc`
/// — not a `ConfigProvider` rebuilt from a config — and is listed among the registered providers.
#[tokio::test]
async fn an_init_time_live_provider_reaches_the_sink_as_the_same_arc() {
    let (host, sink) = bound_host();
    let provider = live("llama.cpp");
    host.load_native(Native::new(
        "ext-live",
        vec![("llama.cpp".into(), Arc::clone(&provider))],
    ))
    .await
    .unwrap();

    let got = sink
        .live("llama.cpp")
        .expect("the sink was handed a live provider");
    assert!(
        same(&got, &provider),
        "the sink must hold the extension's own Arc, not a rebuilt provider"
    );
    assert_eq!(sink.events(), vec!["upsert-live:llama.cpp".to_string()]);
    assert!(
        host.registry()
            .provider_ids()
            .unwrap()
            .contains(&"llama.cpp".to_string()),
        "the live provider is a registered provider id"
    );
}

/// Registered BEFORE a sink is bound, the live provider is queued and flushed as the same `Arc` at
/// `bind` — the defer→bindCore lifecycle the JSON door already has (Pi `bindCore`).
#[tokio::test]
async fn a_live_provider_registered_before_the_sink_binds_is_flushed_at_bind() {
    let host = ExtensionHost::new(cfg());
    let provider = live("llama.cpp");
    host.load_native(Native::new(
        "ext-live",
        vec![("llama.cpp".into(), Arc::clone(&provider))],
    ))
    .await
    .unwrap();
    assert_eq!(
        host.registry().provider_pending_ids().unwrap(),
        vec!["llama.cpp".to_string()],
        "queued while no sink is bound"
    );

    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    assert!(same(&sink.live("llama.cpp").unwrap(), &provider));
}

/// A provider that identifies itself as something other than the id it is registered under would
/// list models no `model.provider` lookup could reach, so the registration is refused and fails the
/// extension's load.
#[tokio::test]
async fn a_live_provider_registered_under_a_different_id_fails_the_load() {
    let (host, sink) = bound_host();
    let err = host
        .load_native(Native::new(
            "ext-mismatch",
            vec![("llama.cpp".into(), live("not-llama"))],
        ))
        .await
        .expect_err("the id disagrees with the provider's own identity");
    assert!(matches!(err, ExtError::Component(_)), "{err:?}");
    assert!(sink.events().is_empty(), "nothing reached the sink");
    assert!(host.registry().provider_ids().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// post-init registration through the LateRegistrar
// ---------------------------------------------------------------------------

/// After `init`, the extension's `LateRegistrar` registers a live provider; registering the same id
/// again REPLACES it (the sink now holds the new `Arc`), without a second registry entry.
#[tokio::test]
async fn a_late_live_registration_replaces_the_previous_provider() {
    let (host, sink) = bound_host();
    let ext = Native::new("ext-live", Vec::new());
    host.load_native(ext.clone()).await.unwrap();
    let registrar = ext.registrar();

    let first = live("llama.cpp");
    registrar
        .register_provider_live("llama.cpp".into(), Arc::clone(&first))
        .unwrap();
    assert!(same(&sink.live("llama.cpp").unwrap(), &first));

    let second = live("llama.cpp");
    registrar
        .register_provider_live("llama.cpp".into(), Arc::clone(&second))
        .unwrap();
    let held = sink.live("llama.cpp").unwrap();
    assert!(
        same(&held, &second),
        "the replacement Arc is the one the sink holds"
    );
    assert!(!same(&held, &first));
    assert_eq!(
        host.registry().provider_ids().unwrap(),
        vec!["llama.cpp".to_string()],
        "a replace is not a second entry"
    );
    assert_eq!(
        sink.events(),
        vec!["upsert-live:llama.cpp".to_string(); 2],
        "each registration reaches the sink, so its generation moves"
    );
}

/// `LateRegistrar::unregister_provider` retracts what the extension registered and reaches the sink.
#[tokio::test]
async fn a_late_unregister_removes_the_provider_from_the_sink() {
    let (host, sink) = bound_host();
    let ext = Native::new("ext-live", vec![("llama.cpp".into(), live("llama.cpp"))]);
    host.load_native(ext.clone()).await.unwrap();

    assert!(ext.registrar().unregister_provider("llama.cpp").unwrap());
    assert!(sink.live("llama.cpp").is_none());
    assert_eq!(sink.events().last().unwrap(), "remove:llama.cpp");
    assert!(host.registry().provider_ids().unwrap().is_empty());
    assert!(
        !ext.registrar().unregister_provider("llama.cpp").unwrap(),
        "a second unregister finds nothing"
    );
}

/// The registrar is bound to ONE owner: it cannot retract a provider another extension owns.
#[tokio::test]
async fn a_registrar_cannot_unregister_another_extensions_provider() {
    let (host, sink) = bound_host();
    let owner = Native::new("ext-owner", vec![("llama.cpp".into(), live("llama.cpp"))]);
    let other = Native::new("ext-other", Vec::new());
    host.load_native(owner.clone()).await.unwrap();
    host.load_native(other.clone()).await.unwrap();

    assert!(
        !other.registrar().unregister_provider("llama.cpp").unwrap(),
        "not the owner: nothing removed"
    );
    assert!(sink.live("llama.cpp").is_some());
    assert!(owner.registrar().unregister_provider("llama.cpp").unwrap());
    assert!(sink.live("llama.cpp").is_none());
}

/// EXT-090 — the GUEST door applies the same owner rule as the native registrar above: pi's
/// `unregisterProvider(name)` removes by name for anyone (`model-runtime.ts:944-950` @f1b2e77f5),
/// cyrup narrows BOTH doors to the id's current owner (a recorded `[CYRUP-DELTA]`).
#[tokio::test]
async fn a_guest_cannot_unregister_another_extensions_provider() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;
    use crate::host::{GuestState, HostState, StoreLimits};

    let registry = Arc::new(ExtensionRegistry::new());
    let sink = Arc::new(Sink::default());
    registry.bind_model_registry(sink.clone()).unwrap();
    let guest_state = |id: &str| {
        HostState::with_guest(
            StoreLimits::default(),
            Arc::new(GuestState::new(ExtensionId::from(id), registry.clone())),
        )
    };
    let mut owner = guest_state("guest-owner");
    let mut intruder = guest_state("guest-intruder");

    owner
        .register_provider("acme".into(), json_config().to_string())
        .await;
    assert_eq!(registry.provider_ids().unwrap(), vec!["acme".to_string()]);

    intruder.unregister_provider("acme".into()).await;
    assert_eq!(
        registry.provider_ids().unwrap(),
        vec!["acme".to_string()],
        "a guest may not retract a provider another extension owns"
    );
    assert_eq!(
        sink.events(),
        vec!["upsert:acme".to_string()],
        "nothing reached the sink"
    );

    owner.unregister_provider("acme".into()).await;
    assert!(
        registry.provider_ids().unwrap().is_empty(),
        "its own provider still unregisters"
    );
    assert_eq!(sink.events().last().unwrap(), "remove:acme");
}

// ---------------------------------------------------------------------------
// failed load / ownership
// ---------------------------------------------------------------------------

/// A native that registered a live provider and then fails its load leaves NOTHING behind: the
/// provider is purged from the hub and the model registry is told to drop it (pi `discard`,
/// `core/extensions/loader.ts:462-467` @v0.87.1).
#[tokio::test]
async fn a_failed_load_purges_the_live_providers_it_registered() {
    let (host, sink) = bound_host();
    let err = host
        .load_native(Native::new(
            "ext-fails",
            vec![
                ("good".into(), live("good")),
                ("bad".into(), live("mismatch")),
            ],
        ))
        .await
        .expect_err("the second provider's id disagrees with its identity");
    assert!(matches!(err, ExtError::Component(_)), "{err:?}");

    assert!(
        sink.events().contains(&"upsert-live:good".to_string()),
        "the first provider DID land before the failure: {:?}",
        sink.events()
    );
    assert!(
        sink.live("good").is_none(),
        "the failed owner's live provider must be dropped from the model registry"
    );
    assert!(sink.events().contains(&"remove:good".to_string()));
    assert!(host.registry().provider_ids().unwrap().is_empty());
    assert!(!host.loaded_ids().contains(&"ext-fails".into()));
}

/// `purge_owner` (the removal path) drops a live provider; a provider re-registered by someone else
/// is NOT dropped with its previous owner — last registration wins ownership.
#[test]
fn purge_owner_drops_the_owners_live_providers_and_only_those() {
    let registry = ExtensionRegistry::new();
    let sink = Arc::new(Sink::default());
    registry.bind_model_registry(sink.clone()).unwrap();
    let a: ExtensionId = "ext-a".into();
    let b: ExtensionId = "ext-b".into();

    registry
        .register_provider_live(a.clone(), "pa", live("pa"))
        .unwrap();
    registry
        .register_provider_live(b.clone(), "pb", live("pb"))
        .unwrap();
    registry.purge_owner(&a).unwrap();
    assert!(sink.live("pa").is_none(), "a's provider is gone");
    assert!(sink.live("pb").is_some(), "b's provider is untouched");

    // Ownership transfer: b re-registers a's id; purging a must not take b's provider.
    registry
        .register_provider_live(a.clone(), "shared", live("shared"))
        .unwrap();
    registry
        .register_provider_live(b.clone(), "shared", live("shared"))
        .unwrap();
    registry.purge_owner(&a).unwrap();
    assert!(sink.live("shared").is_some(), "b owns `shared` now");
    registry.purge_owner(&b).unwrap();
    assert!(sink.live("shared").is_none());
}

// ---------------------------------------------------------------------------
// the JSON door is unchanged
// ---------------------------------------------------------------------------

fn json_config() -> serde_json::Value {
    json!({
        "name": "Acme",
        "baseUrl": "https://acme.test/v1",
        "api": "openai-completions",
        "apiKey": "sk-acme-123",
        "models": [{ "id": "acme-fast", "name": "Acme Fast", "contextWindow": 64000, "maxTokens": 4096 }],
    })
}

/// REGRESSION PIN: a JSON registration still reaches the sink through `upsert_provider` with the
/// parsed config and the resolved key (never `upsert_live_provider`), queues before bind, and is
/// removed by `unregister_provider` / `purge_owner` exactly as before.
#[test]
fn a_json_registration_still_takes_the_config_provider_path() {
    let registry = ExtensionRegistry::new();
    let a: ExtensionId = "ext-a".into();
    registry
        .register_provider(a.clone(), "acme", json_config())
        .unwrap();
    assert_eq!(
        registry.provider_pending_ids().unwrap(),
        vec!["acme".to_string()],
        "queued before a sink is bound"
    );

    let sink = Arc::new(Sink::default());
    registry.bind_model_registry(sink.clone()).unwrap();
    assert_eq!(sink.events(), vec!["upsert:acme".to_string()]);
    assert!(sink.live("acme").is_none(), "not the live door");
    let reg = registry.provider_registration("acme").unwrap().unwrap();
    assert_eq!(reg.resolved_api_key.as_deref(), Some("sk-acme-123"));
    let built = reg.build_provider();
    assert_eq!(built.models().len(), 1);
    assert_eq!(built.models()[0].id.as_str(), "acme-fast");

    assert!(registry.unregister_provider("acme").unwrap());
    assert_eq!(sink.events().last().unwrap(), "remove:acme");

    registry
        .register_provider(a.clone(), "acme", json_config())
        .unwrap();
    registry.purge_owner(&a).unwrap();
    assert_eq!(sink.events().last().unwrap(), "remove:acme");
    assert!(registry.provider_ids().unwrap().is_empty());
}

/// REGRESSION PIN: the JSON door's ownership rule, which the live door mirrors — last registration
/// wins, so purging the previous owner leaves the provider with the new one.
#[test]
fn json_ownership_is_last_registration_wins() {
    let registry = ExtensionRegistry::new();
    let sink = Arc::new(Sink::default());
    registry.bind_model_registry(sink.clone()).unwrap();
    let a: ExtensionId = "ext-a".into();
    let b: ExtensionId = "ext-b".into();
    registry
        .register_provider(a.clone(), "acme", json_config())
        .unwrap();
    registry
        .register_provider(b.clone(), "acme", json_config())
        .unwrap();
    registry.purge_owner(&a).unwrap();
    assert!(registry.provider_registration("acme").unwrap().is_some());
    registry.purge_owner(&b).unwrap();
    assert!(registry.provider_registration("acme").unwrap().is_none());
}

/// An id lives behind ONE door at a time: a live registration replaces a JSON one under the same id
/// and vice versa, so a stale registration of the other kind cannot be re-flushed or resurrected.
#[test]
fn the_two_doors_replace_each_other_under_one_id() {
    let registry = ExtensionRegistry::new();
    let sink = Arc::new(Sink::default());
    registry.bind_model_registry(sink.clone()).unwrap();
    let a: ExtensionId = "ext-a".into();

    registry
        .register_provider(a.clone(), "acme", json_config())
        .unwrap();
    let provider = live("acme");
    registry
        .register_provider_live(a.clone(), "acme", Arc::clone(&provider))
        .unwrap();
    assert!(
        registry.provider_registration("acme").unwrap().is_none(),
        "the JSON registration was replaced"
    );
    assert!(same(&sink.live("acme").unwrap(), &provider));
    assert_eq!(registry.provider_ids().unwrap(), vec!["acme".to_string()]);

    registry
        .register_provider(a.clone(), "acme", json_config())
        .unwrap();
    assert!(registry.provider_registration("acme").unwrap().is_some());
    assert!(
        sink.live("acme").is_none(),
        "the JSON upsert replaced the live provider at the sink"
    );
    assert_eq!(registry.provider_ids().unwrap(), vec!["acme".to_string()]);

    // A fresh sink bound later must not be handed the stale live provider.
    let late = ExtensionRegistry::new();
    late.register_provider_live(a.clone(), "acme", live("acme"))
        .unwrap();
    late.register_provider(a, "acme", json_config()).unwrap();
    let late_sink = Arc::new(Sink::default());
    late.bind_model_registry(late_sink.clone()).unwrap();
    assert_eq!(late_sink.events(), vec!["upsert:acme".to_string()]);
}

// ---------------------------------------------------------------------------
// hidden extensions
// ---------------------------------------------------------------------------

/// A hidden native is loaded and listed by `loaded_ids` but absent from the startup list; a visible
/// one is in both. Default `is_hidden` is `false`.
#[tokio::test]
async fn a_hidden_extension_is_loaded_but_not_in_the_startup_list() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Native::new("ext-visible", Vec::new()))
        .await
        .unwrap();
    host.load_native(Native::hidden("ext-hidden"))
        .await
        .unwrap();

    let loaded: Vec<String> = host.loaded_ids().iter().map(|i| i.to_string()).collect();
    assert_eq!(loaded, vec!["ext-visible", "ext-hidden"]);
    let visible: Vec<String> = host
        .loaded_visible_ids()
        .iter()
        .map(|i| i.to_string())
        .collect();
    assert_eq!(visible, vec!["ext-visible"]);
    assert!(host.is_extension_hidden(&"ext-hidden".into()));
    assert!(!host.is_extension_hidden(&"ext-visible".into()));
    assert!(
        !host.is_extension_hidden(&"never-loaded".into()),
        "an unknown id is not hidden"
    );
}
