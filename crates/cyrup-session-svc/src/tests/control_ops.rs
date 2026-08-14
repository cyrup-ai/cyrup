//! SEAM-003 — extension runtime-tier control ops must PERFORM the operation, not be queued and
//! discarded.
//!
//! Pi binds every one of `ExtensionCommandContextActions` (`waitForIdle`/`newSession`/`fork`/
//! `navigateTree`/`switchSession`/`reload`, extensions/types.ts:1652-1672) to a REAL implementation
//! in every host (`modes/rpc/rpc-mode.ts:321-346`, `modes/print-mode.ts:75-95`), installs them via
//! `runner.bindCommandContext(...)` from `_applyExtensionBindings` (agent-session.ts:2308-2310), and
//! runs them INLINE from the command handler. cyrup queued them onto `LiveHostServices`'s control
//! channel and then dropped the drained vector on the floor (`session.rs` `try_execute_wasm_command`
//! did `let _deferred = …`), while the NATIVE command route did not drain at all.
//!
//! Every assertion here is on the OBSERVABLE EFFECT of the op — a bumped runtime generation, a
//! changed session id, a moved tree leaf, a persisted message, an aborted run — never on the fact
//! that `HostServices::control(...)` returned `Ok`. That return already succeeded before this fix;
//! asserting it would re-create exactly the bug being closed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{ExtensionId, Message, StopReason};
use cyrup_ext::{
    CommandDescriptor, ControlOp, ExtError, HostCtx, HostEvent, HookOutcome, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use crate::{
    AgentSession, AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget,
};
use serde_json::json;
use tempfile::TempDir;

// ---------------------------------------------------------------------------------------------
// A native built-in whose slash commands queue exactly one runtime-tier control op each, through
// the SAME `HostServices::control` seam a wasm guest's `control.*` import reaches (host/live.rs).
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct ControlExt {
    /// The LIVE capability backend, re-captured on every `set_host_services` call. A `OnceLock`
    /// would be wrong here: the factory re-`init`s this same extension into each REPLACEMENT
    /// session, each of which owns a fresh `LiveHostServices` with its own control queue — keeping
    /// the first would push later ops onto a dead session's queue.
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    /// Arguments the test parameterizes commands with (entry ids / session paths are only known
    /// after the session exists, so they are injected rather than hard-coded).
    arg: Arc<Mutex<String>>,
}

#[async_trait::async_trait]
impl NativeExtension for ControlExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("control-ops-probe")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::ToolCall]);
        for name in [
            "ctlnew",
            "ctlreload",
            "ctlnavigate",
            "ctlsend",
            "ctlwait",
            "ctlswitch",
            "ctlfork",
            "ctlabort",
        ] {
            api.register_command(name, CommandDescriptor { description: format!("control op {name}"), completions: Vec::new() });
        }
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let svc = self
            .services
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| ExtError::Component("no host services".into()))?;
        let arg = self.arg.lock().map(|g| g.clone()).unwrap_or_default();
        // `/ctlwait` queues TWO ops: a `WaitIdle` followed by a `SendMessage`. The second is what
        // makes the first OBSERVABLE — if `WaitIdle` were discarded (pre-SEAM-003) neither op would
        // land, and if it stalled the drain the second would never be applied either.
        if name == "ctlwait" {
            svc.control(ControlOp::WaitIdle).map_err(ExtError::Component)?;
            svc.control(ControlOp::SendMessage {
                message: json!({ "customType": "ctlAfterWait", "content": { "note": "after wait" } }),
                opts: json!({}),
            })
            .map_err(ExtError::Component)?;
            return Ok(Some(String::new()));
        }
        let op = match name {
            "ctlnew" => ControlOp::NewSession { opts: json!({}) },
            "ctlreload" => ControlOp::Reload,
            "ctlnavigate" => ControlOp::Navigate { entry_id: arg, opts: json!({}) },
            "ctlsend" => ControlOp::SendMessage {
                message: json!({ "customType": "ctlNote", "content": { "note": "from control op" } }),
                opts: json!({}),
            },
            "ctlswitch" => ControlOp::Switch { session_id: arg, opts: json!({}) },
            "ctlfork" => ControlOp::Fork { entry_id: arg, opts: json!({}) },
            "ctlabort" => ControlOp::Abort,
            other => return Err(ExtError::Component(format!("no such command {other}"))),
        };
        svc.control(op).map_err(ExtError::Component)?;
        Ok(Some(String::new()))
    }
}



// ---------------------------------------------------------------------------------------------

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
    cfg.no_extensions = true;
    cfg
}

fn faux_ok() -> Arc<dyn Provider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// Build a runtime over a factory carrying the control-op probe extension.
async fn runtime_with(fx: &Fixture, ext: Arc<ControlExt>) -> Arc<AgentSessionRuntime> {
    let factory = Arc::new(
        SessionFactory::new(faux_ok(), base_config(fx))
            .with_native_extension(ext as Arc<dyn NativeExtension>),
    );
    AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap()
}

async fn session_id_of(session: &AgentSession) -> String {
    session.session_id().to_string()
}

// =============================================================================================
// new_session / switch / fork / reload — the RUNTIME-tier ops
// =============================================================================================

/// `ControlOp::NewSession` from a command handler REPLACES the active session: the runtime's
/// generation bumps and the active session id changes (Pi `ctx.newSession()` → `runtimeHost
/// .newSession(options)`, rpc-mode.ts:322). Before SEAM-003 the op was queued and the drained
/// vector discarded, so the generation stayed 0 forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_new_session_actually_replaces_the_active_session() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext).await;

    let before_gen = runtime.generation().await;
    let before_id = session_id_of(&*runtime.session().await).await;

    let session = runtime.session().await;
    let _ = session.prompt("/ctlnew").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(before_gen, 0, "a freshly created runtime starts at generation 0");
    assert_eq!(
        runtime.generation().await,
        1,
        "ctx.newSession() from a command handler bumped the runtime generation"
    );
    assert_ne!(
        session_id_of(&*runtime.session().await).await,
        before_id,
        "the runtime is serving a DIFFERENT session after the control op"
    );
}

/// `ControlOp::Reload` rebuilds the active session in place (Pi `ctx.reload()` → `session.reload()`,
/// rpc-mode.ts:343-345): the generation bumps while the session FILE is preserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_reload_actually_rebuilds_the_session() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext).await;

    // Persist something so the session has a file to be reloaded from.
    let session = runtime.session().await;
    let _ = session.prompt("hello").await.unwrap();
    session.wait_for_idle().await;
    let file_before = session.session_file().await;
    assert!(file_before.is_some(), "the session persisted a file to reload from");

    let _ = session.prompt("/ctlreload").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        runtime.generation().await,
        1,
        "ctx.reload() from a command handler rebuilt the session (generation bumped)"
    );
    assert_eq!(
        runtime.session().await.session_file().await,
        file_before,
        "reload re-opens the SAME session file (it is a rebuild, not a new session)"
    );
}

/// `ControlOp::Switch` resumes a different session file (Pi `ctx.switchSession(path)` →
/// `runtimeHost.switchSession`, rpc-mode.ts:339-341).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_switch_actually_resumes_the_named_session() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext.clone()).await;

    // Session A gets a message + a file.
    let a = runtime.session().await;
    let _ = a.prompt("first session").await.unwrap();
    a.wait_for_idle().await;
    let a_file = a.session_file().await.expect("session A persisted a file");
    let a_id = session_id_of(&a).await;

    // Move to a brand-new session B via the runtime's own API (not the op under test).
    runtime.new_session().await.unwrap();
    let b = runtime.session().await;
    assert_ne!(session_id_of(&b).await, a_id);

    // Now ask an extension command to switch BACK to A.
    *ext.arg.lock().unwrap() = a_file.display().to_string();
    let _ = b.prompt("/ctlswitch").await.unwrap();
    b.wait_for_idle().await;

    assert_eq!(
        session_id_of(&*runtime.session().await).await,
        a_id,
        "ctx.switchSession(path) resumed the named session file"
    );
    assert!(
        runtime
            .session()
            .await
            .messages()
            .await
            .iter()
            .any(|m| matches!(m, Message::User { content, .. }
                if content.iter().any(|c| matches!(c, cyrup_core::Content::Text { text, .. }
                    if text.contains("first session"))))),
        "the resumed session carries session A's transcript"
    );
}

/// `ControlOp::Fork` branches at an entry and switches the runtime to the branch (Pi `ctx.fork`,
/// rpc-mode.ts:330-336).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_fork_actually_branches_and_switches() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext.clone()).await;

    let session = runtime.session().await;
    let _ = session.prompt("root message").await.unwrap();
    session.wait_for_idle().await;

    let entries = session.entries_json().await;
    let anchor = entries
        .iter()
        .find(|e| e.get("type").and_then(|v| v.as_str()) == Some("message"))
        .and_then(|e| e.get("id"))
        .and_then(|v| v.as_str())
        .expect("a message entry to fork at")
        .to_string();
    let before_id = session_id_of(&session).await;

    *ext.arg.lock().unwrap() = anchor;
    let _ = session.prompt("/ctlfork").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(runtime.generation().await, 1, "ctx.fork() replaced the active session");
    assert_ne!(
        session_id_of(&*runtime.session().await).await,
        before_id,
        "the runtime is serving the FORKED session"
    );
}

// =============================================================================================
// navigate / send-message / wait-idle — the SESSION-local ops
// =============================================================================================

/// `ControlOp::Navigate` re-roots the session tree leaf (Pi `ctx.navigateTree(targetId)` →
/// `session.navigateTree`, rpc-mode.ts:325-337).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_navigate_actually_moves_the_tree_leaf() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext.clone()).await;
    let session = runtime.session().await;

    let _ = session.prompt("one").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("two").await.unwrap();
    session.wait_for_idle().await;

    let entries = session.entries_json().await;
    // Navigate to the FIRST message entry — a user message re-roots at its parent and drops the
    // later branch from the active path.
    let target = entries
        .iter()
        .find(|e| e.get("type").and_then(|v| v.as_str()) == Some("message"))
        .and_then(|e| e.get("id"))
        .and_then(|v| v.as_str())
        .expect("a message entry to navigate to")
        .to_string();
    let messages_before = session.messages().await.len();

    *ext.arg.lock().unwrap() = target;
    let _ = session.prompt("/ctlnavigate").await.unwrap();
    session.wait_for_idle().await;

    let messages_after = runtime.session().await.messages().await.len();
    assert!(
        messages_after < messages_before,
        "ctx.navigateTree() re-rooted the active branch: {messages_before} -> {messages_after}"
    );
}

/// `ControlOp::SendMessage` injects a custom message into the LIVE session (Pi `ctx.sendMessage`,
/// extensions/types.ts:1223 — `{triggerTurn, deliverAs}`); it must land in the transcript.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_send_message_actually_reaches_the_session() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext).await;
    let session = runtime.session().await;

    let _ = session.prompt("/ctlsend").await.unwrap();
    session.wait_for_idle().await;

    let entries = runtime.session().await.entries_json().await;
    assert!(
        entries.iter().any(|e| e.get("customType").and_then(|v| v.as_str()) == Some("ctlNote")),
        "ctx.sendMessage() persisted the custom message into the live session: {entries:?}"
    );
}

/// `ControlOp::WaitIdle` must be APPLIED and must not stall the drain (Pi `ctx.waitForIdle()` →
/// `session.waitForIdle()`, rpc-mode.ts:322). Observable via the op queued immediately AFTER it: the
/// custom message only lands if the drain got past the wait. cyrup bounds the wait (Pi's promise
/// cannot wedge the command path; cyrup's watch-based one could).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_wait_idle_is_applied_without_stalling_the_drain() {
    let fx = fixture();
    let ext = Arc::new(ControlExt::default());
    let runtime = runtime_with(&fx, ext).await;
    let session = runtime.session().await;

    let done = tokio::time::timeout(Duration::from_secs(10), async {
        let _ = session.prompt("/ctlwait").await.unwrap();
        session.wait_for_idle().await;
    })
    .await;
    assert!(done.is_ok(), "a queued wait-idle control op must not hang the command path");

    let entries = runtime.session().await.entries_json().await;
    assert!(
        entries.iter().any(|e| e.get("customType").and_then(|v| v.as_str()) == Some("ctlAfterWait")),
        "the op queued AFTER wait_idle was applied — the wait ran and the drain continued: {entries:?}"
    );

    // The session is still live afterwards.
    let _ = session.prompt("still alive").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        !session.messages().await.is_empty(),
        "the session continues to accept prompts after a wait-idle control op"
    );
}
