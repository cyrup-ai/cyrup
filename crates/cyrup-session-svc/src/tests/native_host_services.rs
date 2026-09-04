//! FULLY-WIRED PROOF for the shared native-reach-to-live-session host prerequisites (reconciliation
//! §2, P-1 + P-2). A NATIVE built-in extension captures the live `Arc<dyn HostServices>` the builder
//! threads into `load_native_with_services` (P-1) and, from a background tokio task OUTSIDE any
//! `HostCtx`, calls the P-2 accessors + a dialog + message-injection:
//!
//! - `session_id()` — the REAL live session id (not the deny-default `None`).
//! - `session_file()` — the REAL persisted session-file path (what cyrup-ext-subagents fork-context
//!   branches from instead of a most-recent-mtime heuristic).
//! - `confirm()` — reaches the scripted ui sink and returns its reply (not the deny `false`).
//! - `inject_message()` — reaches the live inject sink bound by `into_shared` and triggers a REAL
//!   agent turn OVER the injected custom message (R-SA-101; not the deny `Err`).
//!
//! This proves native code genuinely reaches id/file/dialogs/message-injection through the ONE shared
//! seam every remaining cyrup-ext-subagents blocker + both companions close on — not stubs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig, UiKind, UiReply, UiRequest};
use cyrup_agent::AgentMessage;
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    DialogOptions, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use futures::StreamExt;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;

/// What the background probe task observed by calling the captured `Arc<dyn HostServices>` — the raw
/// return of each seam, so the test can assert each reached the REAL `LiveHostServices`, not a stub.
#[derive(Debug)]
struct ProbeResults {
    session_id: Option<String>,
    session_file: Option<PathBuf>,
    confirm: bool,
    inject: Result<(), String>,
}

impl Default for ProbeResults {
    fn default() -> Self {
        Self {
            session_id: None,
            session_file: None,
            confirm: false,
            inject: Err("probe never captured host services".to_string()),
        }
    }
}

/// A native built-in that stashes the P-1 host-services `Arc` and, from a background tokio task,
/// exercises the P-2 seams. It does NOT touch `HostCtx` — the whole point is native reach OUTSIDE any
/// live dispatch context (the intercom socket-delivery / permission forwarding-watcher pattern).
struct ProbeExt {
    /// The `Arc` handed in via P-1 (`set_host_services`), captured before `init`.
    services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// The test releases the probe once the session is fully wired (into_shared + ui sink attached).
    go: Arc<Notify>,
    /// Where the background task reports what each seam returned.
    results_tx: UnboundedSender<ProbeResults>,
}

#[async_trait::async_trait]
impl NativeExtension for ProbeExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("native-host-services-probe")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        // P-1: capture the SAME live backend the WASM path gets. Bound BEFORE `init`.
        let _ = self.services.set(services);
    }

    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        // The Arc is already captured (P-1 binds it before `init`). Spawn a detached background task
        // that reaches the live backend once the test releases it — genuinely outside any `HostCtx`.
        let services = self.services.get().cloned();
        let go = self.go.clone();
        let tx = self.results_tx.clone();
        tokio::spawn(async move {
            go.notified().await; // wait until the session is fully assembled + the ui sink is attached
            let Some(svc) = services else {
                let _ = tx.send(ProbeResults::default());
                return;
            };
            let results = ProbeResults {
                session_id: svc.session_id(),
                session_file: svc.session_file(),
                // `confirm` blocks on the scripted ui sink's reply (block_in_place + a oneshot).
                confirm: svc.confirm(
                    "proceed?",
                    "from a native background task",
                    &DialogOptions::default(),
                ),
                // `inject_message` forwards to the live inject sink, which triggers a REAL turn.
                inject: svc.inject_message(
                    "background result",
                    Some("probe-notify"),
                    true,
                    None,
                    true,
                ),
            };
            let _ = tx.send(results);
        });
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    // One response feeds the single turn the injected message triggers.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    faux
}

/// The whole shared foundation, proven end-to-end: a native built-in's background task reaches the
/// REAL live session's id/file/dialogs/message-injection through the P-1 captured Arc.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_background_task_reaches_live_host_services() {
    let fx = fixture();
    let services_slot: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
    let go = Arc::new(Notify::new());
    let (results_tx, mut results_rx) = tokio::sync::mpsc::unbounded_channel::<ProbeResults>();
    let ext = Arc::new(ProbeExt {
        services: services_slot.clone(),
        go: go.clone(),
        results_tx,
    });

    // Assemble a REAL session with the native probe registered, then bind it (into_shared installs the
    // inject sink — the production path the runtime / SDK / main use).
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(ext)
        .build()
        .await
        .unwrap()
        .into_shared();

    // The probe's captured Arc MUST be the very backend the assembled session exposes (P-1 wired the
    // SAME instance the builder stores on the session).
    assert!(
        services_slot.get().is_some(),
        "P-1: the native extension captured a host-services Arc"
    );

    // Scripted ui sink: answer every dialog (the probe issues a `confirm`).
    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    session.services().host_services.set_ui_sink(ui_tx);
    tokio::spawn(async move {
        while let Some(req) = ui_rx.recv().await {
            let reply = match req.kind {
                UiKind::Confirm => UiReply::Confirm(true),
                _ => UiReply::Text(Some("scripted".to_string())),
            };
            let _ = req.reply.send(reply);
        }
    });

    // Observe the injected turn: subscribe BEFORE releasing the probe so no event is missed.
    let mut events = session.subscribe();

    // Everything is wired — release the background probe.
    go.notify_one();

    // Collect what the background task saw.
    let results = tokio::time::timeout(Duration::from_secs(15), results_rx.recv())
        .await
        .expect("the native background probe reported within the timeout")
        .expect("the probe channel delivered results");

    // ---- P-2 (a): the REAL session id, not the deny-default `None`. ----
    assert_eq!(
        results.session_id.as_deref(),
        Some(session.session_id().as_str()),
        "session_id() reached the live session id (not a stub)"
    );

    // ---- P-2 (b): the REAL persisted session-file path, matching the session's own read. ----
    assert!(
        results.session_file.is_some(),
        "session_file() resolved a real persisted path"
    );
    assert_eq!(
        results.session_file,
        session.session_file().await,
        "session_file() from the background task matches the live session's own file (not a stub)"
    );

    // ---- confirm: routed to the scripted ui sink and returned its reply (deny default is `false`). ----
    assert!(
        results.confirm,
        "confirm() reached the live ui sink and returned its scripted reply"
    );

    // ---- inject_message: reached the live inject sink (deny default is `Err`). ----
    assert!(
        results.inject.is_ok(),
        "inject_message() reached the live inject sink: {:?}",
        results.inject
    );

    // ---- inject_message ACTUALLY injected the custom message + triggered a real turn (R-SA-101). ----
    // Drain the live event stream for the injected `probe-notify` custom message and the turn it drove.
    let mut saw_injected_custom = false;
    let mut saw_turn = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(10), events.next()).await {
        match &ev {
            AgentSessionEvent::MessageStart { message }
            | AgentSessionEvent::MessageEnd { message } => {
                if let AgentMessage::Custom { kind, .. } = message
                    && kind == "probe-notify"
                {
                    saw_injected_custom = true;
                }
            }
            AgentSessionEvent::AgentEnd { .. } => {
                saw_turn = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_injected_custom,
        "the injected `probe-notify` custom message flowed through the live turn loop"
    );
    assert!(
        saw_turn,
        "inject_message(trigger_turn=true) drove a REAL agent turn to completion"
    );

    session.wait_for_idle().await;
    // The triggered turn produced the faux assistant response — with NO prior prompt, the ONLY turn is
    // the one inject_message drove, so a present assistant reply is unambiguous proof it ran.
    assert_eq!(
        session.last_assistant_text().await.as_deref(),
        Some("ok"),
        "the injected turn ran end-to-end and produced the assistant response"
    );
}
