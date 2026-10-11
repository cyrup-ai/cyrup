//! SEAM-158 — a dialog opened by an extension's `before_agent_start` handler reaches the RPC client.
//!
//! pi's `case "prompt"` (`rpc-mode.ts:394-414` @v1.0.4) starts the prompt with `void session.prompt(
//! …, { preflightResult })` and returns, so its loop keeps reading stdin and writing stdout while
//! the prompt's preparation runs; the `prompt` response is written from `preflightResult`, which
//! `AgentSession.prompt` calls only after `emitBeforeAgentStart` (`agent-session.ts:2048`, `:2095`).
//! A dialog a `before_agent_start` handler opens therefore reaches the client — as an
//! `extension_ui_request`, BEFORE the `prompt` response — and the client's `extension_ui_response`
//! resumes the handler.
//!
//! cyrup dispatched `prompt` inline and awaited the whole preparation inside the command arm, so the
//! loop that forwards dialogs (and reads their answers) never ran until the dialog gave up: the host
//! wrote no line at all, not even the `prompt` response.

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::support::{
    build_runtime_with_ext, fixture, parse_lines, read_json_line, spawn_rpc_duplex, type_of,
};

/// A native extension whose `before_agent_start` handler asks the user to confirm the run, and
/// records the answer.
#[derive(Default)]
struct ConfirmBeforeStart {
    services: std::sync::Mutex<Option<Arc<dyn HostServices>>>,
    answer: Arc<std::sync::Mutex<Option<bool>>>,
}

#[async_trait::async_trait]
impl NativeExtension for ConfirmBeforeStart {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("confirm-before-start")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut slot) = self.services.lock() {
            *slot = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::BeforeAgentStart]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::BeforeAgentStart { .. })
            && let Some(services) = self.services.lock().ok().and_then(|slot| slot.clone())
        {
            let answer = services.confirm(
                "Start the run?",
                "asked from before_agent_start",
                &cyrup_ext::host::DialogOptions::default(),
            );
            if let Ok(mut slot) = self.answer.lock() {
                *slot = Some(answer);
            }
        }
        HookOutcome::Noop
    }
}

/// The SEAM-158 Verify clause: the handler's dialog reaches the client before the `prompt` response,
/// the client's answer resumes the handler, and the run then starts and settles.
///
/// RED at HEAD: no line arrives within the bound (the live run saw none for 20 s).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_before_agent_start_dialog_reaches_the_client_before_the_prompt_response() {
    let fx = fixture();
    let ext = Arc::new(ConfirmBeforeStart::default());
    let answer = ext.answer.clone();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ran")],
        StopReason::Stop,
    )]);
    let runtime = build_runtime_with_ext(&fx, faux, ext).await;
    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    client_tx
        .write_all(b"{\"type\":\"prompt\",\"id\":\"p\",\"message\":\"go\"}\n")
        .await
        .unwrap();

    // The first line the client sees must be the dialog: nothing else can be written before the
    // preparation it is part of has finished.
    let first = tokio::time::timeout(Duration::from_secs(5), read_json_line(&mut client_reader))
        .await
        .expect("the before_agent_start dialog reaches the client while the prompt is preparing");
    assert_eq!(type_of(&first), "extension_ui_request", "{first}");
    assert_eq!(first["method"], "confirm", "{first}");
    let id = first["id"]
        .as_str()
        .expect("the request carries an id")
        .to_string();

    client_tx
        .write_all(
            format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"confirmed\":true}}\n")
                .as_bytes(),
        )
        .await
        .unwrap();

    // Then the prompt is answered and the run starts and settles.
    let mut lines: Vec<Value> = Vec::new();
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let line = read_json_line(&mut client_reader).await;
            let done = type_of(&line) == "agent_settled";
            lines.push(line);
            if done {
                break;
            }
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "the run settled after the dialog was answered: {lines:?}"
    );
    let response = lines
        .iter()
        .position(|l| type_of(l) == "response" && l["id"] == "p")
        .expect("the prompt is answered");
    assert_eq!(lines[response]["success"], true, "{lines:?}");
    let started = lines
        .iter()
        .position(|l| type_of(l) == "agent_start")
        .expect("the run started");
    assert!(
        response < started,
        "the prompt response precedes the run's events, as pi's preflightResult does: {lines:?}"
    );
    assert_eq!(
        *answer.lock().unwrap(),
        Some(true),
        "the handler saw the client's answer"
    );

    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(20), rpc)
        .await
        .expect("run_rpc returns after stdin ends")
        .unwrap();
}

/// SEAM-161 (b) — the RPC `agent_start` re-abort at stdin EOF (`450b35f3c`, SEAM-154), driven in a
/// fixed order instead of by a `prompt` and the close in one write (which proved it only 5 of 5 live
/// runs, never deterministically).
///
/// The `before_agent_start` dialog pins the order: the prompt's preparation parks on it, the client
/// closes stdin, and the EOF handling (`abort_at_eof`) aborts the session — no run yet, so nothing —
/// and answers the dialog as cancelled. Only then does the handler return and the run start, AFTER
/// the EOF was seen. The model never answers, so only the re-abort on that run's `agent_start` ends
/// it: without it the loop waits out its 5 s bound and the run never settles as aborted.
///
/// Passes at HEAD+SEAM-158 (the re-abort exists; this is the pin SEAM-161 asked for) — a
/// NON-REGRESSION GUARD. Removing the re-abort turns it red.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_that_starts_after_stdin_ended_is_aborted_on_its_agent_start() {
    let fx = fixture();
    let ext = Arc::new(ConfirmBeforeStart::default());
    let answer = ext.answer.clone();
    let faux = Arc::new(FauxProvider::new());
    // A model that never answers (no test releases it).
    let release = Arc::new(tokio::sync::Notify::new());
    faux.set_response_steps(vec![FauxResponseStep::async_factory(
        move |_ctx, _opts, _state, _model| {
            let release = release.clone();
            async move {
                release.notified().await;
                faux_assistant_message(vec![faux_text("never reached")], StopReason::Stop)
            }
        },
    )]);
    let runtime = build_runtime_with_ext(&fx, faux, ext).await;
    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    client_tx
        .write_all(b"{\"type\":\"prompt\",\"id\":\"p\",\"message\":\"go\"}\n")
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), read_json_line(&mut client_reader))
        .await
        .expect("the preparation is parked on the dialog");
    assert_eq!(type_of(&first), "extension_ui_request", "{first}");

    let closed = std::time::Instant::now();
    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(20), rpc)
        .await
        .expect("run_rpc returns after stdin ends")
        .unwrap();
    let elapsed = closed.elapsed();

    let mut rest = String::new();
    {
        use tokio::io::AsyncReadExt as _;
        let _ = client_reader.read_to_string(&mut rest).await;
    }
    let lines = parse_lines(rest.as_bytes());
    assert_eq!(
        *answer.lock().unwrap(),
        Some(false),
        "the EOF answered the open dialog as cancelled, so the run started after it"
    );
    assert!(
        lines.iter().any(|l| type_of(l) == "agent_start"),
        "the run started after the EOF: {lines:?}"
    );
    let aborted = lines.iter().any(|l| {
        type_of(l) == "message_end"
            && l["message"]["role"] == "assistant"
            && l["message"]["stopReason"] == "aborted"
    });
    assert!(
        aborted,
        "its agent_start re-abort ended it as an aborted turn: {lines:?}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "the re-abort ended the run; waiting out the 5 s bound would not: {elapsed:?}"
    );
}
