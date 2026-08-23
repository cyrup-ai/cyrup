//! The two session-lifecycle announcements: `session_start` when a session BEGINS (SEAM-001) and
//! `session_info_changed` when a live one is RENAMED (A.6 / EXT-011). Both fan out to
//! `AgentSessionEvent` subscribers and to the extension host, and both were once emitted from only
//! one of the paths that should emit them.
//!
//! SEAM-001 — the INITIAL session must announce itself with `session_start{reason:"startup"}`.
//!
//! Pi ground truth: `AgentSession` stores `this._sessionStartEvent = config.sessionStartEvent ??
//! { type: "session_start", reason: "startup" }` (agent-session.ts:389) and every host's
//! `bindExtensions()` ends with `await this._extensionRunner.emit(this._sessionStartEvent)`
//! (agent-session.ts:2250) — print-mode.ts:73, rpc-mode.ts:318 and interactive-mode.ts:1698 all call
//! it at startup, BEFORE any prompt. Only the runtime's REPLACEMENT paths override the reason
//! (`"new"`/`"resume"`/`"fork"`, agent-session-runtime.ts:218/251/305).
//!
//! Pre-fix cyrup emitted `session_start` exclusively from the replacement tail
//! (`AgentSessionRuntime::install_inner`), so the first — and in a one-shot run, the only — session
//! of every process was never announced: the permission gate never cleared its approval store or
//! started the ask-forwarding watcher, subagents never reset background-run tracking, intercom never
//! saw its `SessionStart` arm.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::ExtensionId;
use cyrup_ext::{EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use super::common::{base_config, fixture, Fixture};
use crate::{
    AgentSessionEvent, AgentSessionRuntime, SessionBuilder, SessionConfig, SessionFactory,
    SessionTarget,
};
use futures::StreamExt;

/// A native extension that records the `reason` of every `session_start` it is notified of, in
/// arrival order — the exact surface pi's extensions observe.
#[derive(Clone, Default)]
struct StartRecorder {
    reasons: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for StartRecorder {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("start-recorder")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::SessionStart { reason, .. } = ev
            && let Ok(mut g) = self.reasons.lock()
        {
            g.push(reason.clone());
        }
        HookOutcome::Noop
    }
}

/// [`base_config`] with persistence off: the `session_start` tests assert over what an
/// UNSAVED session does, so nothing may be written to the session store.
fn unsaved_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = base_config(fx);
    cfg.persist = false;
    cfg
}

fn recorded(rec: &StartRecorder) -> Vec<String> {
    rec.reasons.lock().unwrap().clone()
}

/// THE headline proof: the host constructor every interactive/RPC run goes through
/// (`AgentSessionRuntime::create`) must announce the initial session to extensions with pi's
/// `"startup"` reason. Pre-fix this recorded `[]`.
#[tokio::test]
async fn runtime_create_announces_the_initial_session_to_extensions() {
    let fx = fixture();
    let rec = StartRecorder::default();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let cfg = unsaved_config(&fx);
    let target = cfg.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, cfg).with_native_extension(Arc::new(rec.clone())),
    );

    let _runtime = AgentSessionRuntime::create(factory, target).await.unwrap();

    assert_eq!(
        recorded(&rec),
        vec!["startup".to_string()],
        "the INITIAL session must emit session_start{{reason:\"startup\"}} exactly once \
         (Pi agent-session.ts:389 + :2250)"
    );
}

/// The replacement path must keep its own reason and must NOT be turned into a second `"startup"`
/// — `install_inner`'s emission stays intact and the two announcements are ordered
/// `startup` then `new` (Pi agent-session-runtime.ts:251).
#[tokio::test]
async fn replacement_appends_its_own_reason_without_double_announcing() {
    let fx = fixture();
    let rec = StartRecorder::default();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let cfg = unsaved_config(&fx);
    let target = cfg.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, cfg).with_native_extension(Arc::new(rec.clone())),
    );

    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    runtime.new_session().await.unwrap();

    assert_eq!(
        recorded(&rec),
        vec!["startup".to_string(), "new".to_string()],
        "a session REPLACEMENT announces with its own reason after the initial startup announcement"
    );
}

/// The one-shot (print/json) shape: a session assembled straight off `SessionBuilder` announces
/// itself when the host binds extensions, and binding twice does not double-announce (pi emits
/// `_sessionStartEvent` exactly once per session).
#[tokio::test]
async fn bind_extensions_announces_once_per_session() {
    let fx = fixture();
    let rec = StartRecorder::default();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, unsaved_config(&fx))
        .with_native_extension(Arc::new(rec.clone()))
        .build()
        .await
        .unwrap();

    session.bind_extensions().await;
    session.bind_extensions().await;

    assert_eq!(
        recorded(&rec),
        vec!["startup".to_string()],
        "session_start is emitted exactly once per session, however many times a host binds"
    );
}

/// A session built for a `New` target through the factory is announced by whichever tier installs
/// it — never twice. Guards the seam against a future host that both creates a runtime and binds
/// the session it hands out.
#[tokio::test]
async fn runtime_session_is_not_reannounced_by_a_host_bind() {
    let fx = fixture();
    let rec = StartRecorder::default();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let cfg = unsaved_config(&fx);
    let factory = Arc::new(
        SessionFactory::new(provider, cfg).with_native_extension(Arc::new(rec.clone())),
    );
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();

    runtime.session().await.bind_extensions().await;

    assert_eq!(
        recorded(&rec),
        vec!["startup".to_string()],
        "a runtime-owned session already announced itself; a host bind must not repeat it"
    );
}

// ============================================== the OTHER lifecycle announcement: session_info ====

/// `set_session_name` emits `session_info_changed { name }` to live subscribers (Pi
/// agent-session.ts:2714-2715) — previously it persisted the entry and emitted NOTHING.
#[tokio::test]
async fn set_session_name_emits_session_info_changed() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, unsaved_config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();

    let mut stream = session.subscribe();
    session.set_session_name("my session").await.expect("set name");

    let mut found: Option<Option<String>> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
        if let AgentSessionEvent::SessionInfoChanged { name } = &ev {
            found = Some(name.clone());
            break;
        }
    }
    assert_eq!(found, Some(Some("my session".to_string())), "session_info_changed{{name}} must fire");
    assert_eq!(session.session_name().await.as_deref(), Some("my session"));
}

/// A native extension that records every `session_info_changed` payload it is handed.
struct InfoChangedRecorder(Arc<std::sync::Mutex<Vec<Option<String>>>>);

#[async_trait::async_trait]
impl NativeExtension for InfoChangedRecorder {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("info-changed-recorder")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionInfoChanged]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::SessionInfoChanged { name } = ev {
            crate::sync::lock(&self.0).push(name.clone());
        }
        HookOutcome::Noop
    }
}

/// EXT-011 — the rename is also an EXTENSION event: pi `SessionInfoChangedEvent`
/// (`extensions/types.ts:571-575` @v0.83.0), subscribed and dispatched like any other lifecycle
/// notify.
///
/// RED before this pass: `EventKind::SessionInfoChanged`, `HostEvent::SessionInfoChanged`, the WIT
/// export and the SDK's `on_session_info_changed` all existed, but NOTHING in the session emitted
/// it — `set_session_name` fanned the event out to `AgentSessionEvent` subscribers only. A guest
/// could subscribe and never be called, which is the worst failure shape: silent and untestable
/// from the guest side. This recorder would collect zero payloads.
#[tokio::test]
async fn set_session_name_also_dispatches_the_session_info_changed_extension_event() {
    let fx = fixture();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, unsaved_config(&fx))
        .with_native_extension(Arc::new(InfoChangedRecorder(Arc::clone(&seen))))
        .build()
        .await
        .expect("build")
        .into_shared();

    session.set_session_name("my session").await.expect("set name");

    assert_eq!(
        crate::sync::lock(&seen).clone(),
        vec![Some("my session".to_string())],
        "the extension must receive `session_info_changed` with the resolved name"
    );

    // An empty/whitespace name resolves to `None` through `getSessionName()`, and the extension
    // sees the SAME `None` the `AgentSessionEvent` subscribers do.
    session.set_session_name("   ").await.expect("clear name");
    assert_eq!(
        crate::sync::lock(&seen).last().cloned(),
        Some(None),
        "a blank rename dispatches `name: None`, not the previous name"
    );
}
