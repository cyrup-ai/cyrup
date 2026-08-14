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
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use crate::{
    AgentSessionRuntime, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
use tempfile::TempDir;

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

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
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
    let cfg = base_config(&fx);
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
    let cfg = base_config(&fx);
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
    let session = SessionBuilder::new(provider, base_config(&fx))
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
    let cfg = base_config(&fx);
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
