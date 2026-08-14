//! SEAM-026 (with SEAM-001's and SEAM-002's SDK residuals) — the documented one-call SDK path must
//! BIND its extensions and must offer a teardown.
//!
//! Pi ground truth: `AgentSession.bindExtensions()` ends with
//! `await this._extensionRunner.emit(this._sessionStartEvent)` (agent-session.ts:2250), the event
//! defaulting to `{type:"session_start", reason:"startup"}` (:389); every pi host calls it
//! (print-mode.ts:73, rpc-mode.ts:318, interactive-mode.ts:1698). The mirror image is
//! `AgentSessionRuntime.dispose()` — `await emitSessionShutdownEvent(…, {type:"session_shutdown",
//! reason:"quit"}); this.session.dispose();` (agent-session-runtime.ts:398-404).
//!
//! `CyrupBuilder::build_session` did neither: `SessionBuilder::new` → customizers →
//! `Session::new(builder.build().await?)`, with no `bind_extensions()` anywhere under
//! `crates/cyrup-sdk`, and no `close`/`dispose`/`Drop` at all. Extensions ARE reachable from this
//! path (`customize` hands out the `SessionBuilder`, which exposes `with_native_extension`), so an
//! embedder got extensions that never saw either end of the session's life: audit loggers never
//! initialised, intercom identities were never registered or deregistered, permission policy was
//! never loaded.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::ExtensionId;
use cyrup_ext::{EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_sdk::{Cyrup, SessionConfig};
use tempfile::TempDir;

/// Records the lifecycle events an embedder's extension is notified of, in arrival order — exactly
/// the surface pi extensions observe.
#[derive(Clone, Default)]
struct LifecycleRecorder {
    seen: Arc<Mutex<Vec<String>>>,
}

impl LifecycleRecorder {
    fn recorded(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl NativeExtension for LifecycleRecorder {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("sdk-lifecycle-recorder")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionStart, EventKind::SessionShutdown]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let entry = match ev {
            HostEvent::SessionStart { reason, .. } => Some(format!("session_start:{reason}")),
            HostEvent::SessionShutdown { reason, .. } => Some(format!("session_shutdown:{reason}")),
            _ => None,
        };
        if let Some(e) = entry
            && let Ok(mut g) = self.seen.lock()
        {
            g.push(e);
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

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.persist = false;
    cfg
}

/// Assemble a session through the DOCUMENTED one-call SDK path, with a recording extension
/// registered the way an embedder registers one (`customize` → `with_native_extension`).
async fn build(fx: &Fixture, rec: &LifecycleRecorder) -> cyrup_sdk::Session {
    let ext: Arc<dyn NativeExtension> = Arc::new(rec.clone());
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Cyrup::builder()
        .customize(move |b| b.with_native_extension(ext))
        .build_session(provider, config(fx))
        .await
        .expect("build_session")
}

// ================================================================================== SEAM-001 ====

/// THE SEAM-001 SDK residual: `build_session().await` must have announced the session to its
/// extensions before it returns. Pre-fix this recorded `[]`.
#[tokio::test]
async fn build_session_announces_session_start_to_extensions() {
    let fx = fixture();
    let rec = LifecycleRecorder::default();
    let _session = build(&fx, &rec).await;

    assert_eq!(
        rec.recorded(),
        vec!["session_start:startup".to_string()],
        "the SDK's one-call path must bind its extensions and emit \
         session_start{{reason:\"startup\"}} (Pi agent-session.ts:389 + :2250)"
    );
}

/// The same for the zero-config `build_session_auto` entry point, which delegates to
/// `build_session` — proving the announcement lives on the shared path, not one caller.
#[tokio::test]
async fn build_session_auto_announces_session_start_too() {
    let fx = fixture();
    let rec = LifecycleRecorder::default();
    let ext: Arc<dyn NativeExtension> = Arc::new(rec.clone());
    let mut cfg = config(&fx);
    cfg.model_pattern = Some("anthropic/claude-opus-4-8".into());

    let session = Cyrup::builder()
        .customize(move |b| b.with_native_extension(ext))
        .build_session_auto(cfg)
        .await
        .expect("build_session_auto");

    assert_eq!(rec.recorded(), vec!["session_start:startup".to_string()]);
    session.close().await;
}

// ================================================================================== SEAM-002 ====

/// THE SEAM-002 SDK residual: there must BE a teardown, and it must announce
/// `session_shutdown{reason:"quit"}` — pi's `AgentSessionRuntime.dispose()`
/// (agent-session-runtime.ts:398-404). Pre-fix no such method existed on `Session` at all.
#[tokio::test]
async fn close_announces_session_shutdown_to_extensions() {
    let fx = fixture();
    let rec = LifecycleRecorder::default();
    let session = build(&fx, &rec).await;

    session.close().await;

    assert_eq!(
        rec.recorded(),
        vec!["session_start:startup".to_string(), "session_shutdown:quit".to_string()],
        "an embedder's extensions must see BOTH ends of the session's life, start then shutdown"
    );
}

/// The announcement is exactly-once at each end even across a full drive: build, run a turn, close.
/// (`bind_extensions` is latched by `start_announced`, and `close` consumes the handle so it cannot
/// be called twice.)
#[tokio::test]
async fn a_full_embedder_drive_announces_each_end_exactly_once() {
    let fx = fixture();
    let rec = LifecycleRecorder::default();
    let session = build(&fx, &rec).await;

    let _ = session.run("hello").await;
    session.close().await;

    let seen = rec.recorded();
    assert_eq!(
        seen.iter().filter(|e| e.starts_with("session_start")).count(),
        1,
        "exactly one session_start across the whole embedder drive: {seen:?}"
    );
    assert_eq!(
        seen.iter().filter(|e| e.starts_with("session_shutdown")).count(),
        1,
        "exactly one session_shutdown across the whole embedder drive: {seen:?}"
    );
    assert_eq!(
        seen.first().map(String::as_str),
        Some("session_start:startup"),
        "start is first: {seen:?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some("session_shutdown:quit"),
        "shutdown is last: {seen:?}"
    );
}
