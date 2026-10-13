//! SEAM-163 / SEAM-164 / SEAM-165 — what an extension says at `session_start`, and what a prompt
//! sent while `agent_settled` handlers run is told.
//!
//! The runtime is built UNANNOUNCED, as `main.rs` builds it for `--mode rpc` (SEAM-033), so
//! `run_rpc` is what announces the session; a runtime built by `AgentSessionRuntime::create`
//! announces at construction and would measure a window production does not have.

use std::sync::Arc;
use std::sync::OnceLock;

use super::support::{Fixture, base_config, fixture, read_json_line, spawn_rpc_duplex};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::host::{HostServices, NotifyKind, services::WidgetPlacement};
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSessionRuntime, SessionFactory};
use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream};

async fn unannounced(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    ext: Arc<dyn NativeExtension>,
) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = base_config(fx);
    // `main.rs` builds an RPC session in `AppMode::Rpc`, a mode that attaches an effect drain.
    cfg.app_mode = cyrup_config::AppMode::Rpc;
    let target = cfg.target.clone();
    AgentSessionRuntime::create_unannounced(
        Arc::new(SessionFactory::new(provider, cfg).with_native_extension(ext)),
        target,
    )
    .await
    .expect("build runtime")
}

/// Write one line, then read until the response to `id`, returning every line read.
async fn ask(
    tx: &mut DuplexStream,
    rx: &mut BufReader<DuplexStream>,
    line: &str,
    id: &str,
) -> Vec<Value> {
    tx.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    tx.flush().await.unwrap();
    let mut seen = Vec::new();
    loop {
        let v = read_json_line(rx).await;
        let done = v["type"] == "response" && v["id"] == id;
        seen.push(v);
        if done {
            return seen;
        }
    }
}

fn ui_requests<'a>(lines: &'a [Value], method: &str) -> Vec<&'a Value> {
    lines
        .iter()
        .filter(|l| l["type"] == "extension_ui_request" && l["method"] == method)
        .collect()
}

// --- SEAM-164 ------------------------------------------------------------------------------------

/// Sets four pieces of state and says one notice in `session_start`, once.
struct StateAtStart {
    services: OnceLock<Arc<dyn HostServices>>,
}

#[async_trait::async_trait]
impl NativeExtension for StateAtStart {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("state-at-start")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let (HostEvent::SessionStart { .. }, Some(s)) = (ev, self.services.get()) {
            s.set_status("ext", Some("ready"));
            s.set_widget(
                "w",
                Some(&["widget line".to_string()]),
                WidgetPlacement::default(),
            );
            s.set_title("titled at start");
            s.set_editor_text("drafted at start", false);
            s.notify("said at start", NotifyKind::Info);
        }
        HookOutcome::Noop
    }
}

/// SEAM-164 — every state effect an extension sets in `session_start` reaches the RPC client, not
/// only its notice. RED at the base: only `notify` arrived (`run_rpc` announced before it attached
/// its effect drain, and the drain-less host keeps notices alone).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam164_session_start_state_reaches_the_rpc_client() {
    let fx = fixture();
    let runtime = unannounced(
        &fx,
        Arc::new(FauxProvider::new()),
        Arc::new(StateAtStart {
            services: OnceLock::new(),
        }),
    )
    .await;
    let (mut tx, mut rx, rpc) = spawn_rpc_duplex(runtime);
    let lines = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"get_state","id":"boot"}"#,
        "boot",
    )
    .await;

    let status = ui_requests(&lines, "setStatus");
    assert_eq!(status.len(), 1, "{lines:#?}");
    assert_eq!(status[0]["statusKey"], "ext");
    assert_eq!(status[0]["statusText"], "ready");
    let widget = ui_requests(&lines, "setWidget");
    assert_eq!(widget.len(), 1, "{lines:#?}");
    assert_eq!(widget[0]["widgetKey"], "w");
    assert_eq!(
        ui_requests(&lines, "setTitle")[0]["title"],
        "titled at start"
    );
    assert_eq!(
        ui_requests(&lines, "set_editor_text")[0]["text"],
        "drafted at start"
    );
    assert_eq!(ui_requests(&lines, "notify")[0]["message"], "said at start");

    drop(tx);
    rpc.await.expect("rpc loop exits at EOF");
}

// --- SEAM-163 ------------------------------------------------------------------------------------

/// SEAM-163 — forty notices before the drain: the client is handed the thirty-two kept AND one
/// notice saying eight more were not. RED at the base: thirty-two, and nothing about the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam163_notices_past_the_cap_are_counted_not_lost_in_silence() {
    let fx = fixture();
    let runtime = unannounced(
        &fx,
        Arc::new(FauxProvider::new()),
        Arc::new(StateAtStart {
            services: OnceLock::new(),
        }),
    )
    .await;
    // Said by a background extension (the MCP adapter connecting forty servers) before the mode
    // runs, i.e. before any drain exists.
    {
        let session = runtime.session().await;
        for n in 0..40 {
            session
                .services()
                .host_services
                .notify(&format!("early {n}"), NotifyKind::Error);
        }
    }
    let (mut tx, mut rx, rpc) = spawn_rpc_duplex(runtime);
    let lines = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"get_state","id":"boot"}"#,
        "boot",
    )
    .await;

    let notices: Vec<String> = ui_requests(&lines, "notify")
        .iter()
        .map(|n| n["message"].as_str().unwrap_or_default().to_string())
        .collect();
    let early: Vec<&String> = notices.iter().filter(|m| m.starts_with("early ")).collect();
    assert_eq!(early.len(), 32, "{notices:#?}");
    assert_eq!(early[0], "early 0", "the first ones are the ones kept");
    let marker: Vec<&String> = notices
        .iter()
        .filter(|m| m.contains("8 more") && m.contains("not kept"))
        .collect();
    assert_eq!(marker.len(), 1, "one notice counts the rest: {notices:#?}");

    drop(tx);
    rpc.await.expect("rpc loop exits at EOF");
}

// --- SEAM-165 ------------------------------------------------------------------------------------

/// Services an `input` whose text starts with `handle` (pi `{ action: "handled" }`); in
/// `agent_settled` it tells the client `settling` and holds the settle until the test releases it,
/// so a `prompt` sent after that notice arrives while the handler runs.
struct SettlesSlowly {
    services: OnceLock<Arc<dyn HostServices>>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl NativeExtension for SettlesSlowly {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("settles-slowly")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input, EventKind::AgentSettled]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { text, .. } if text.starts_with("handle") => {
                HookOutcome::Handled(HandledValue(serde_json::json!({ "action": "handled" })))
            }
            HostEvent::AgentSettled { .. } => {
                if let Some(s) = self.services.get() {
                    s.notify("settling", NotifyKind::Info);
                }
                self.release.notified().await;
                HookOutcome::Noop
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// Run a first prompt to its `agent_settled`, send `second` while the handler holds it, and return
/// the response to `second`.
///
/// No public seam says the second prompt has reached the deferral check, so the handler is
/// released only after a `get_state` sent behind it has been answered (the loop has read and
/// dispatched the prompt) and the scheduler has had further turns. The VERDICT does not depend on
/// that: the base answers `started` only on the deferral branch, and the fix answers the prompt's
/// real disposition on either branch.
async fn prompt_during_settle(
    fx: &Fixture,
    settings: Option<cyrup_config::Settings>,
    second: &str,
) -> Value {
    let release = Arc::new(tokio::sync::Notify::new());
    let ext = Arc::new(SettlesSlowly {
        services: OnceLock::new(),
        release: Arc::clone(&release),
    });
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = base_config(fx);
    cfg.app_mode = cyrup_config::AppMode::Rpc;
    let target = cfg.target.clone();
    let mut factory = SessionFactory::new(provider, cfg).with_native_extension(ext);
    if let Some(settings) = settings {
        factory = factory.cli_settings(settings);
    }
    let runtime = AgentSessionRuntime::create_unannounced(Arc::new(factory), target)
        .await
        .expect("build runtime");
    let (mut tx, mut rx, rpc) = spawn_rpc_duplex(runtime);

    tx.write_all(b"{\"type\":\"prompt\",\"id\":\"first\",\"message\":\"go\"}\n")
        .await
        .unwrap();
    loop {
        let v = read_json_line(&mut rx).await;
        if v["type"] == "extension_ui_request" && v["message"] == "settling" {
            break;
        }
    }
    tx.write_all(format!("{second}\n").as_bytes())
        .await
        .unwrap();
    let mut answer = None;
    // The loop is not held behind the deferred prompt: a later command is answered while the
    // settle's handler is still waiting on `release`.
    let probed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ask(
            &mut tx,
            &mut rx,
            r#"{"type":"get_state","id":"probe"}"#,
            "probe",
        ),
    )
    .await
    .expect("a command after the deferred prompt is answered before the settle ends");
    for line in probed {
        if line["type"] == "response" && line["id"] == "second" {
            answer = Some(line);
        }
    }
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    release.notify_one();
    let answer = match answer {
        Some(answer) => answer,
        None => loop {
            let v = read_json_line(&mut rx).await;
            if v["type"] == "response" && v["id"] == "second" {
                break v;
            }
        },
    };
    // A run the deferred prompt started settles too; let its handler through, then close.
    release.notify_one();
    drop(tx);
    rpc.await.expect("rpc loop exits at EOF");
    answer
}

/// SEAM-165 — a `prompt` sent while `agent_settled` handlers run, and serviced by an `input`
/// handler, answers `handled`: the disposition SEAM-162 put on the wire is the prompt's real one.
/// RED at the base: `started`, written before the deferred prompt had done anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam165_a_prompt_deferred_past_agent_settled_answers_its_real_disposition() {
    let fx = fixture();
    let answer = prompt_during_settle(
        &fx,
        None,
        r#"{"type":"prompt","id":"second","message":"handle me"}"#,
    )
    .await;
    assert_eq!(answer["success"], true, "{answer}");
    assert_eq!(
        answer["data"],
        serde_json::json!({ "disposition": "handled" }),
        "{answer}"
    );
}

/// SEAM-165 — a deferred prompt that fails answers an `error`, not a `started` and a log line. The
/// failure is the pre-prompt compaction check on an invalid `compaction.keepRecentTokens`, which
/// runs only once a previous assistant message exists, so the first prompt is unaffected. RED at
/// the base: `started`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam165_a_deferred_prompt_that_fails_answers_an_error() {
    let fx = fixture();
    let mut settings = cyrup_config::Settings::new();
    settings
        .set_field(
            "compaction",
            serde_json::json!({ "enabled": false, "keepRecentTokens": 2.5 }),
        )
        .unwrap();
    let answer = prompt_during_settle(
        &fx,
        Some(settings),
        r#"{"type":"prompt","id":"second","message":"plain text"}"#,
    )
    .await;
    assert_eq!(answer["success"], false, "{answer}");
    assert_eq!(answer["command"], "prompt", "{answer}");
    assert!(
        answer["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{answer}"
    );
}
