//! EXT-081 — a FAILED extension load leaves NOTHING behind.
//!
//! pi: `initializeExtension` (`core/extensions/loader.ts:541-559` @v0.87.1) is
//! `try { await factory(load.api); load.commit(); } catch (error) { load.discard(); throw error; }`.
//! `discard` (`:462-467`) flips the API to `"failed"`, runs every `loadingUnsubscribers` entry, and
//! `clearPending()`s the pending flag values and the deferred runtime changes — `registerProvider`
//! being one of those (`:421-433` routes it through `applyRuntimeChange`, `:244-247`). Everything
//! else an extension registered during its factory lives on the per-extension `Extension` object
//! (`createExtension`, `:525-538`: `tools`/`commands`/`flags`/`shortcuts`/`messageRenderers`/
//! `entryRenderers`), and `initializeExtension` throws WITHOUT ever handing that object to the
//! runner — so upstream needs no sweep, the whole thing is garbage.
//!
//! cyrup's registry is FLAT and SHARED and every registration writes through it immediately, so the
//! sweep has to be explicit: `ExtensionRegistry::purge_owner` + `SharedBus::unsubscribe_all` +
//! `release_id`, run from the failure tail of both load paths.
//!
//! Before the fix `load_native_inner` released only the ID (its own doc comment conceded
//! "Registrations already written to the registry before the failing step are left in place") and
//! `load_wasm_with_caps` released NOTHING — a `?` on `LiveExtension::load` returned with the id
//! still in `loaded`, so a retry of the same id failed with a spurious `DuplicateId`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use crate::registry::CommandDescriptor;
use crate::{
    EventKind, ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent,
    InitApi, NativeExtension,
};
use cyrup_core::{ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use serde_json::{Value, json};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

const TOOL: &str = "ext081_tool";
const COMMAND: &str = "ext081_command";
const SHORTCUT: &str = "ctrl+alt+ext081";
const FLAG: &str = "ext081_flag";
const PROVIDER: &str = "ext081_provider";
const TOPIC: &str = "ext081_topic";

struct Noop(Value);

#[async_trait::async_trait]
impl Tool for Noop {
    fn name(&self) -> &str {
        TOOL
    }
    fn parameters(&self) -> &Value {
        &self.0
    }
    async fn execute(
        &self,
        _id: ToolCallId,
        _args: Value,
        _cancel: cyrup_core::CancelToken,
        _sink: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text("ok")],
            ..Default::default()
        })
    }
}

fn noop_tool() -> Arc<dyn Tool> {
    Arc::new(Noop(
        json!({"type": "object", "properties": {}, "additionalProperties": true}),
    ))
}

/// A native that registers ONE of everything and then, optionally, fails.
///
/// The failure is a MALFORMED SECOND PROVIDER CONFIG rather than an `Err` from `init`, because
/// `load_native_body` drains the whole `InitApi` after `init` returns: an `init` that fails writes
/// nothing, so it could not observe the purge. A registration step that fails after earlier ones
/// succeeded is the real shape of this bug on the native path — and the provider loop runs LAST
/// (after tools, commands, renderers, shortcuts and flags), so every other registration has already
/// landed when it trips.
struct Registrant {
    id: ExtensionId,
    fail: bool,
}

#[async_trait::async_trait]
impl NativeExtension for Registrant {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::UserBash]);
        api.register_tool(noop_tool());
        api.register_command(
            COMMAND,
            CommandDescriptor {
                description: "ext081".into(),
                completions: Vec::new(),
            },
        );
        api.register_shortcut(SHORTCUT, Some("ext081".into()));
        api.register_flag(FLAG, json!({"type": "boolean", "default": false}));
        api.add_autocomplete(COMMAND);
        api.add_autocomplete_provider();
        api.register_tool_renderer(TOOL);
        api.register_message_renderer("ext081_msg");
        api.register_entry_renderer("ext081_entry");
        api.register_markdown_transformer();
        api.subscribe_terminal_input();
        api.subscribe_bus(TOPIC);
        api.register_provider(PROVIDER, json!({"name": PROVIDER, "models": []}));
        if self.fail {
            // `ProviderHub::register` parses the config into `ProviderConfig`; a non-string `name`
            // is a serde failure, surfaced as `ExtError::Component`.
            api.register_provider("ext081_broken", json!({"name": 7}));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A recording [`crate::provider::ModelRegistrySink`], so the test can see that purging a provider
/// actually reaches the model registry (`register_provider`'s "immediate upsert if the model
/// registry is bound" is what has to be undone).
#[derive(Default)]
struct SinkLog(Mutex<Vec<String>>);

impl crate::provider::ModelRegistrySink for SinkLog {
    fn upsert_provider(&self, reg: &crate::provider::ProviderRegistration) {
        if let Ok(mut g) = self.0.lock() {
            g.push(format!("upsert:{}", reg.id));
        }
    }
    fn upsert_live_provider(&self, id: &str, _provider: Arc<dyn cyrup_provider::Provider>) {
        if let Ok(mut g) = self.0.lock() {
            g.push(format!("upsert-live:{id}"));
        }
    }
    fn remove_provider(&self, id: &str) {
        if let Ok(mut g) = self.0.lock() {
            g.push(format!("remove:{id}"));
        }
    }
}

/// Every registration this owner could have made is GONE from every shared table.
fn assert_purged(host: &ExtensionHost, id: &ExtensionId) {
    let r = host.registry();

    assert!(
        !r.extension_tools()
            .unwrap()
            .iter()
            .any(|t| t.name() == TOOL),
        "the failed extension's TOOL must not stay in the active set"
    );
    assert!(
        r.resolved_command_owner(COMMAND).unwrap().is_none(),
        "the failed extension's COMMAND must not stay registered"
    );
    assert!(
        !r.resolved_commands()
            .unwrap()
            .iter()
            .any(|c| c.name == COMMAND),
        "nor survive as an orphan in the load-order Vec behind `name:N` disambiguation"
    );
    assert!(
        !r.shortcut_keys().unwrap().iter().any(|k| k == SHORTCUT),
        "the failed extension's SHORTCUT must not stay registered"
    );
    assert!(
        r.shortcut_owner(SHORTCUT).unwrap().is_none(),
        "nor keep an owner entry"
    );
    assert!(
        r.get_flag(FLAG).unwrap().is_none(),
        "the failed extension's FLAG must not stay registered"
    );
    assert!(
        !r.provider_ids().unwrap().iter().any(|p| p == PROVIDER),
        "the failed extension's PROVIDER must not stay registered"
    );
    assert!(
        !r.provider_pending_ids()
            .unwrap()
            .iter()
            .any(|p| p == PROVIDER),
        "nor stay queued for the next model-registry bind"
    );
    assert!(
        !r.command_autocomplete()
            .unwrap()
            .iter()
            .any(|(o, _)| o == id),
        "nor keep its command-autocomplete opt-in"
    );
    assert!(
        !r.autocomplete_providers().unwrap().iter().any(|o| o == id),
        "nor keep its stacked global autocomplete provider"
    );
    assert!(
        !r.conflicts().unwrap().iter().any(|c| c.path == *id),
        "nor leave a conflict record naming it"
    );

    assert!(
        !host.loaded_ids().contains(id),
        "a failed load must not keep the id reserved (the startup listing would report it loaded)"
    );
    assert!(
        !host.bus().subscribers_for(TOPIC).contains(id),
        "pi's `discard` runs every `loadingUnsubscribers` entry (loader.ts:465) — the failed \
         extension must not still be on the shared bus"
    );
}

// ---------------------------------------------------------------------------
// The NATIVE path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failed_native_load_leaves_no_registrations() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(SinkLog::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();

    let id: ExtensionId = "ext081-native".into();
    let err = host
        .load_native(Arc::new(Registrant {
            id: id.clone(),
            fail: true,
        }))
        .await
        .expect_err("the malformed provider config fails the load");
    assert!(
        matches!(err, ExtError::Component(_)),
        "the load failed for the reason the fixture intended, not some other way: {err:?}"
    );

    assert_purged(&host, &id);
    assert!(
        sink.0
            .lock()
            .unwrap()
            .contains(&format!("remove:{PROVIDER}")),
        "purging the owner must tell the MODEL REGISTRY to drop the models its provider upserted, \
         got {:?}",
        sink.0.lock().unwrap()
    );
}

/// The id-reservation half on the NATIVE path: a retry of the SAME id after a failed load must
/// succeed rather than trip `DuplicateId`. pi's `LoadExtensionsResult.extensions` only ever holds
/// extensions that loaded; a failure lives in the sibling `errors` array and claims nothing.
///
/// This one is GREEN both before and after EXT-081 and is kept as the ANTI-REGRESSION half: the
/// native path already called `release_id` (EXT-S01), and every registry table is keyed by owner, so
/// a retry under the same id overwrites rather than duplicating. Its job is to prove the new purge
/// cannot poison an id it just swept — the purge half of the row is proved by
/// `a_failed_native_load_leaves_no_registrations` above. The WASM sibling
/// (`a_failed_wasm_load_releases_the_id`) is the one that was RED, because that path released
/// nothing at all.
#[tokio::test]
async fn the_same_id_loads_cleanly_after_a_failed_native_load() {
    let host = ExtensionHost::new(cfg());
    let id: ExtensionId = "ext081-retry".into();

    host.load_native(Arc::new(Registrant {
        id: id.clone(),
        fail: true,
    }))
    .await
    .expect_err("first load fails");

    host.load_native(Arc::new(Registrant {
        id: id.clone(),
        fail: false,
    }))
    .await
    .expect("a FIXED build of the same extension loads under the same id");

    // And the retry's own registrations are all live — the purge must not have poisoned the id.
    assert!(
        host.registry()
            .extension_tools()
            .unwrap()
            .iter()
            .any(|t| t.name() == TOOL),
        "the successful retry's tool is registered"
    );
    assert!(host.loaded_ids().contains(&id));

    // One command, invocable under its BARE name: the purge must not have left the first attempt's
    // entry behind to be disambiguated into `…:1` / `…:2`.
    let resolved = host.registry().resolved_commands().unwrap();
    let mine: Vec<_> = resolved.iter().filter(|c| c.name == COMMAND).collect();
    assert_eq!(mine.len(), 1, "one registered command");
    assert_eq!(mine[0].invocation_name, COMMAND);
}

// ---------------------------------------------------------------------------
// Anti-regression: the purge must NEVER fire on the happy path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_successful_native_load_keeps_every_registration() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(SinkLog::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();

    let id: ExtensionId = "ext081-ok".into();
    host.load_native(Arc::new(Registrant {
        id: id.clone(),
        fail: false,
    }))
    .await
    .expect("the happy path loads");

    let r = host.registry();
    assert!(
        r.extension_tools()
            .unwrap()
            .iter()
            .any(|t| t.name() == TOOL),
        "tool"
    );
    assert!(
        r.resolved_command_owner(COMMAND).unwrap().is_some(),
        "command"
    );
    assert!(
        r.shortcut_owner(SHORTCUT).unwrap().as_ref() == Some(&id),
        "shortcut"
    );
    assert!(r.get_flag(FLAG).unwrap().is_some(), "flag");
    assert!(
        r.provider_ids().unwrap().iter().any(|p| p == PROVIDER),
        "provider"
    );
    assert!(host.loaded_ids().contains(&id), "id");
    assert!(
        host.bus().subscribers_for(TOPIC).contains(&id),
        "bus subscription"
    );
    assert!(
        !sink
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("remove:")),
        "the model registry was never told to drop anything, got {:?}",
        sink.0.lock().unwrap()
    );
}

// ---------------------------------------------------------------------------
// The WASM path.
//
// The full component E2E lives in `cyrup-it` (loading a real `cyrup-ext-sdk` guest needs the
// `wasm32-wasip2` toolchain, which is why `tests/wasm_host.rs` drives bare core-wasm modules
// instead). What is under test HERE is the failure TAIL of `load_wasm_with_caps`: a guest's `init`
// runs INSIDE `LiveExtension::load` and its `registration.*` imports write straight through to this
// same shared `ExtensionRegistry`, so the registrations are seeded through that registry under the
// loading id and the load is then failed at `LiveExtension::load` with bytes that are not a
// component. Before the fix this path released NOTHING at all — not even the id.
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-host")]
fn services() -> Arc<dyn crate::host::HostServices> {
    Arc::new(crate::DenyServices)
}

#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_failed_wasm_load_leaves_no_registrations() {
    use crate::manifest::Capabilities;

    let host = ExtensionHost::with_wasm(cfg()).expect("a wasm-enabled host");
    let sink = Arc::new(SinkLog::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();

    let id: ExtensionId = "ext081-wasm".into();
    let r = host.registry();
    r.register_tool(id.clone(), noop_tool()).unwrap();
    r.register_command(
        id.clone(),
        COMMAND,
        CommandDescriptor {
            description: "ext081".into(),
            completions: Vec::new(),
        },
    )
    .unwrap();
    r.register_shortcut(id.clone(), SHORTCUT, Some("ext081".into()))
        .unwrap();
    r.register_flag(
        id.clone(),
        FLAG,
        json!({"type": "boolean", "default": false}),
    )
    .unwrap();
    r.register_provider(
        id.clone(),
        PROVIDER,
        json!({"name": PROVIDER, "models": []}),
    )
    .unwrap();
    r.add_command_autocomplete(id.clone(), COMMAND).unwrap();
    r.add_autocomplete_provider(id.clone()).unwrap();
    host.bus().subscribe(id.clone(), TOPIC.to_string());

    let Err(_) = host
        .load_wasm_with_caps(
            id.clone(),
            b"\0asm not a component",
            services(),
            &Capabilities::default(),
        )
        .await
    else {
        panic!("bytes that are not a component must fail the load")
    };

    assert_purged(&host, &id);
    assert!(
        sink.0
            .lock()
            .unwrap()
            .contains(&format!("remove:{PROVIDER}")),
        "got {:?}",
        sink.0.lock().unwrap()
    );

    // And the id is free again: a second load of the same id gets past `reserve_id` and fails for
    // its OWN reason, never `DuplicateId`.
    let Err(err) = host
        .load_wasm_with_caps(
            id.clone(),
            b"\0asm not a component",
            services(),
            &Capabilities::default(),
        )
        .await
    else {
        panic!("still not a component")
    };
    assert!(
        !matches!(err, ExtError::DuplicateId(_)),
        "a retry of the same id must not trip a spurious DuplicateId: {err:?}"
    );
}

/// The id-reservation half on the WASM path, which had NO release at all: a `?` on
/// `LiveExtension::load` returned with the id still in `loaded`, so a retry of the same id failed
/// with a spurious `ExtError::DuplicateId` instead of its real reason. Kept separate from the purge
/// test above so the two halves of the row fail independently.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_failed_wasm_load_releases_the_id() {
    use crate::manifest::Capabilities;

    let host = ExtensionHost::with_wasm(cfg()).expect("a wasm-enabled host");
    let id: ExtensionId = "ext081-wasm-id".into();

    for attempt in 1..=2 {
        let Err(err) = host
            .load_wasm_with_caps(
                id.clone(),
                b"\0asm not a component",
                services(),
                &Capabilities::default(),
            )
            .await
        else {
            panic!("bytes that are not a component must fail the load")
        };
        assert!(
            !matches!(err, ExtError::DuplicateId(_)),
            "attempt {attempt}: a failed wasm load must release its id reservation, got {err:?}"
        );
        assert!(
            !host.loaded_ids().contains(&id),
            "attempt {attempt}: and must not report itself as loaded"
        );
    }
}
