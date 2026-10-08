//! The NATIVE-tier virtual-model registration seam: `InitApi::register_virtual_model`, the pre-bind
//! queue, the bind-time flush with per-item error containment, the post-`init` `LateRegistrar`
//! door, and purge-by-owner.
//!
//! pi: `pi.registerVirtualModel()` pushes onto `runtime.pendingVirtualModelRegistrations`
//! (`extensions/loader.ts:226-228` @v1.0.4) while the extension loads; `bindCore` flushes the queue
//! with a PER-ITEM `try`/`catch` and then replaces the runtime slot with a direct call
//! (`extensions/runner.ts:498-543`); `createAgentSessionServices` runs the same contained loop
//! (`core/agent-session-services.ts:182-194`). `unregisterVirtualModel(provider, id)` removes one
//! pair (`core/model-runtime.ts:976-982`), and `unregisterProvider` does NOT
//! (`docs/virtual-models.md`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use crate::provider::{ModelRegistrySink, ProviderRegistration};
use crate::{
    ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    LateRegistrar, NativeExtension,
};
use cyrup_core::{ExtensionId, ModelThinkingLevel, ProviderId};
use cyrup_provider::{
    ConfigProvider, ModelRoute, ModelRouteError, ModelRouteRequest, ModelRouter, Provider,
    VirtualModelDefinition, VirtualModelSpec,
};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// A router that routes nowhere. These tests drive the REGISTRATION lifecycle; routing itself is
/// `cyrup-provider`'s and is covered there.
struct NoRouter;

#[async_trait::async_trait]
impl ModelRouter for NoRouter {
    async fn route(&self, _request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        Err(ModelRouteError::new("not routed in this test"))
    }
}

/// `tag` rides on the spec's `name`, which is how a REPLACEMENT is told from an append: the two
/// registrations share a `(provider, id)` and differ only in what reached the sink.
fn definition(provider: &str, id: &str, tag: &str) -> VirtualModelDefinition {
    VirtualModelDefinition::new(
        VirtualModelSpec {
            provider: ProviderId::from(provider),
            id: id.into(),
            name: tag.to_string(),
            thinking_levels: Some(vec![ModelThinkingLevel::Low]),
            context_window: None,
            max_tokens: None,
            input: None,
        },
        Arc::new(NoRouter),
    )
}

/// A recording sink. `refuse` makes `upsert_virtual_model` answer an error for one `(provider, id)`,
/// standing in for the registry's physical-conflict refusal.
#[derive(Default)]
struct Sink {
    events: Mutex<Vec<String>>,
    /// The tag of the router the sink last saw for each `(provider, id)` — the way a replacement is
    /// told from an append.
    tags: Mutex<Vec<(String, String)>>,
    refuse: Mutex<Option<(String, String)>>,
}

impl Sink {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    /// `("{provider}/{id}", tag)` for each virtual model the sink holds — the tag being the spec's
    /// `name`, so a replacement is observable rather than merely counted.
    fn tags(&self) -> Vec<(String, String)> {
        self.tags.lock().unwrap().clone()
    }
    fn refuse(&self, provider: &str, id: &str) {
        *self.refuse.lock().unwrap() = Some((provider.to_string(), id.to_string()));
    }
}

impl ModelRegistrySink for Sink {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert:{}", reg.id));
    }
    fn upsert_live_provider(&self, id: &str, _provider: Arc<dyn Provider>) {
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert-live:{id}"));
    }
    fn remove_provider(&self, id: &str) {
        self.events.lock().unwrap().push(format!("remove:{id}"));
    }
    fn upsert_virtual_model(&self, definition: &VirtualModelDefinition) -> Result<(), String> {
        let key = (
            definition.spec.provider.as_str().to_string(),
            definition.spec.id.as_str().to_string(),
        );
        if self.refuse.lock().unwrap().as_ref() == Some(&key) {
            return Err(format!(
                "Virtual model {}/{} conflicts with a physical model.",
                key.0, key.1
            ));
        }
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert-virtual:{}/{}", key.0, key.1));
        let mut tags = self.tags.lock().unwrap();
        let tag = definition.spec.name.clone();
        match tags
            .iter_mut()
            .find(|(k, _)| *k == format!("{}/{}", key.0, key.1))
        {
            Some(slot) => slot.1 = tag,
            None => tags.push((format!("{}/{}", key.0, key.1), tag)),
        }
        Ok(())
    }
    fn remove_virtual_model(&self, provider: &str, id: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("remove-virtual:{provider}/{id}"));
        self.tags
            .lock()
            .unwrap()
            .retain(|(k, _)| *k != format!("{provider}/{id}"));
    }
}

/// A native that registers virtual models (and optionally a live provider) at `init`, and stashes
/// its [`LateRegistrar`] so a test can drive the post-`init` verbs.
struct Native {
    id: ExtensionId,
    init_models: Vec<VirtualModelDefinition>,
    init_provider: Option<(String, Arc<dyn Provider>)>,
    registrar: Mutex<Option<Arc<dyn LateRegistrar>>>,
}

impl Native {
    fn new(id: &str, init_models: Vec<VirtualModelDefinition>) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            init_models,
            init_provider: None,
            registrar: Mutex::new(None),
        })
    }
    fn with_provider(id: &str, provider_id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            init_models: vec![definition(provider_id, "auto", "v1")],
            init_provider: Some((
                provider_id.to_string(),
                ConfigProvider::new(provider_id, provider_id, None, Vec::new()).into_arc(),
            )),
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
        if let Some((id, provider)) = &self.init_provider {
            api.register_provider_live(id.clone(), Arc::clone(provider));
        }
        for def in &self.init_models {
            api.register_virtual_model(def.clone());
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

// ---------------------------------------------------------------------------

/// A virtual model registered while the extension loads is QUEUED and flushed when the model
/// registry binds — pi's `pendingVirtualModelRegistrations` (`loader.ts:190`, `:226-228`) drained
/// at `runner.ts:502-514` / `agent-session-services.ts:182-194`.
///
/// RED-PROVE: delete the `virtual_model_hub.bind(sink)` flush from
/// `ExtensionRegistry::bind_model_registry` (return `Vec::new()` instead) — the sink records
/// nothing and both the pending-before and the flushed-after assertions fail. Deleting only
/// `std::mem::take(&mut self.pending)`'s loop body fails the same way.
#[tokio::test]
async fn a_virtual_model_registered_before_bind_is_flushed_at_bind() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Native::new(
        "ext-a",
        vec![definition("router", "auto", "v1")],
    ))
    .await
    .unwrap();

    // Pre-bind: queued, not yet in any registry.
    assert_eq!(
        host.registry().virtual_model_pending_pairs().unwrap(),
        vec![("router".to_string(), "auto".to_string())]
    );

    let sink = Arc::new(Sink::default());
    let failures = host.registry().bind_model_registry(sink.clone()).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(
        sink.events(),
        vec!["upsert-virtual:router/auto".to_string()]
    );
    assert!(
        host.registry()
            .virtual_model_pending_pairs()
            .unwrap()
            .is_empty(),
        "the queue is cleared by the flush, as upstream clears it"
    );
}

/// Re-registering the same `(provider, id)` REPLACES the virtual model, in place —
/// `docs/virtual-models.md`: "Registering the same provider and ID again replaces the virtual
/// model", and pi's `Map.set` keeps the key's position.
///
/// RED-PROVE: make `VirtualModelHub::register` push unconditionally instead of replacing in place —
/// two entries flush, the sink sees `v1` then `v2` as two upserts (the `events` length assertion
/// fails) and `keys()` carries `router/auto` twice.
#[tokio::test]
async fn re_registering_a_provider_and_id_replaces_the_definition() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Native::new(
        "ext-a",
        vec![
            definition("router", "auto", "v1"),
            definition("router", "auto", "v2"),
        ],
    ))
    .await
    .unwrap();

    assert_eq!(
        host.registry().virtual_model_keys().unwrap(),
        vec![("router".to_string(), "auto".to_string())],
        "one key, not two"
    );
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    assert_eq!(
        sink.events(),
        vec!["upsert-virtual:router/auto".to_string()]
    );
    assert_eq!(
        sink.tags(),
        vec![("router/auto".to_string(), "v2".to_string())],
        "the SECOND router is the one that reached the sink"
    );
}

/// `unregister_virtual_model` removes ONE pair; a sibling under the same provider survives — pi's
/// pair-keyed filter (`loader.ts:229-232`, `model-runtime.ts:976-979`).
///
/// RED-PROVE: key `VirtualModelHub` by provider alone (drop the `.1` comparison from `unregister`) —
/// `router/second` disappears with `router/auto` and the survivor assertion fails.
#[tokio::test]
async fn unregister_removes_only_that_provider_and_id() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    let native = Native::new(
        "ext-a",
        vec![
            definition("router", "auto", "v1"),
            definition("router", "second", "v1"),
        ],
    );
    host.load_native(native.clone()).await.unwrap();

    assert!(
        native
            .registrar()
            .unregister_virtual_model("router", "auto")
            .unwrap()
    );
    assert_eq!(
        host.registry().virtual_model_keys().unwrap(),
        vec![("router".to_string(), "second".to_string())]
    );
    // An absent pair is pi's no-op: `false`, and no sink call.
    assert!(
        !native
            .registrar()
            .unregister_virtual_model("router", "auto")
            .unwrap()
    );
    assert_eq!(
        sink.events(),
        vec![
            "upsert-virtual:router/auto".to_string(),
            "upsert-virtual:router/second".to_string(),
            "remove-virtual:router/auto".to_string(),
        ]
    );
}

/// Unregistering the PROVIDER leaves its virtual model registered — `docs/virtual-models.md`:
/// "`pi.unregisterVirtualModel(provider, id)` removes it; `pi.unregisterProvider()` does not."
///
/// RED-PROVE: have `ProviderHub::unregister` (or `ExtensionRegistry::unregister_provider`) also
/// clear the virtual hub — the surviving-key assertion fails and the sink records a
/// `remove-virtual`.
#[tokio::test]
async fn unregistering_a_provider_leaves_its_virtual_model_registered() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    let native = Native::with_provider("ext-a", "faux");
    host.load_native(native.clone()).await.unwrap();

    assert!(native.registrar().unregister_provider("faux").unwrap());
    assert_eq!(
        host.registry().virtual_model_keys().unwrap(),
        vec![("faux".to_string(), "auto".to_string())],
        "unregisterProvider must not remove virtual models"
    );
    assert!(
        !sink
            .events()
            .iter()
            .any(|e| e.starts_with("remove-virtual")),
        "{:?}",
        sink.events()
    );
}

/// Purging an OWNER drops its virtual models.
///
/// **[CYRUP-DELTA]**, and a deliberate improvement: pi attributes virtual models to nobody
/// (`model-runtime.ts:179`), so after a `/reload` a dead extension's router stays registered and
/// its captured `runtime.createContext()` throws against the invalidated runtime
/// (`loader.ts:504-508`, `:191-200`).
///
/// RED-PROVE: delete the virtual block from `ExtensionRegistry::purge_in` — the key survives the
/// owner and the sink records no `remove-virtual`.
#[tokio::test]
async fn purging_an_owner_drops_its_virtual_models() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    host.load_native(Native::new(
        "ext-a",
        vec![definition("router", "auto", "v1")],
    ))
    .await
    .unwrap();

    let dropped = host
        .registry()
        .purge_owner(&ExtensionId::from("ext-a"))
        .unwrap();
    assert!(dropped >= 1);
    assert!(host.registry().virtual_model_keys().unwrap().is_empty());
    assert!(
        sink.events()
            .contains(&"remove-virtual:router/auto".to_string()),
        "{:?}",
        sink.events()
    );
}

/// A registration made AFTER the bind reaches the sink immediately — pi replaces the runtime slot
/// with a direct call at `runner.ts:539-543`.
///
/// RED-PROVE: make `VirtualModelHub::register` always queue (drop the `Some(sink)` arm) — the sink
/// stays empty and the registration is invisible until a second bind that never comes.
#[tokio::test]
async fn a_registration_after_bind_reaches_the_sink_immediately() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    let native = Native::new("ext-a", Vec::new());
    host.load_native(native.clone()).await.unwrap();
    assert!(sink.events().is_empty());

    native
        .registrar()
        .register_virtual_model(definition("router", "late", "v1"))
        .unwrap();
    assert_eq!(
        sink.events(),
        vec!["upsert-virtual:router/late".to_string()]
    );
    assert!(
        host.registry()
            .virtual_model_pending_pairs()
            .unwrap()
            .is_empty()
    );
}

/// A registration the sink REFUSES during the flush is contained as a diagnostic: the others still
/// land and the load does not fail — pi's per-item `try`/`catch` (`runner.ts:502-514`) and the
/// `Extension "{path}" error: {message}` push at `agent-session-services.ts:185-191`.
///
/// RED-PROVE: propagate the `Err` out of `VirtualModelHub::bind` (e.g. `?` inside the loop) — the
/// third registration never reaches the sink, so the `upsert-virtual:router/third` assertion fails;
/// swallowing the error silently instead fails the diagnostic assertion, which is the half an
/// extension author needs in order to see why their router never appeared.
#[tokio::test]
async fn a_failing_registration_is_contained_as_a_diagnostic() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Native::new(
        "ext-a",
        vec![
            definition("router", "first", "v1"),
            definition("router", "bad", "v1"),
            definition("router", "third", "v1"),
        ],
    ))
    .await
    .unwrap();

    let sink = Arc::new(Sink::default());
    sink.refuse("router", "bad");
    let failures = host.registry().bind_model_registry(sink.clone()).unwrap();

    assert_eq!(
        sink.events(),
        vec![
            "upsert-virtual:router/first".to_string(),
            "upsert-virtual:router/third".to_string(),
        ],
        "one refusal must not cost the others their registration"
    );
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].provider, "router");
    assert_eq!(failures[0].id, "bad");
    assert_eq!(failures[0].extension_path, "ext-a");
    assert_eq!(
        failures[0].diagnostic(),
        "Extension \"ext-a\" error: Virtual model router/bad conflicts with a physical model."
    );
}

/// REGRESSION GUARD — not a red proof: it passes with and without the virtual-model seam. A native
/// that registers nothing virtual leaves every virtual table empty, so the provider half of the
/// bind is unchanged by this work.
#[tokio::test]
async fn an_extension_that_registers_no_virtual_model_changes_nothing() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    let failures = host.registry().bind_model_registry(sink.clone()).unwrap();
    assert!(failures.is_empty());
    host.load_native(Native::new("ext-a", Vec::new()))
        .await
        .unwrap();
    assert!(host.registry().virtual_model_keys().unwrap().is_empty());
    assert!(sink.events().is_empty());
}
