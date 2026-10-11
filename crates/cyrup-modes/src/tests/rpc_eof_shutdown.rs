//! SEAM-154 — closing the input is how an RPC client asks for the shutdown, as in pi.
//!
//! Pi's `process.stdin.on("end", () => void shutdown())` (`rpc-mode.ts:802-805` @v1.0.4) disposes the
//! runtime and exits at once; `AgentSession.dispose()` aborts the retry backoff, the compaction, the
//! branch summary, the bash command and the agent run on the way (`docs/rpc.md` @v1.0.4: "Close the
//! child's stdin to request an orderly shutdown"). cyrup's loop used to wait for the run and for
//! every running command to finish by themselves, so a client that disconnected mid-run left the
//! run going to completion, tools included, and a run parked on a dialog nobody could answer any
//! more kept the process alive for as long as it liked.
//!
//! Each test holds the input open until the work is demonstrably in flight, closes it, and requires
//! the loop to come back by itself. The bound in each is far inside what a wait for the work would
//! take, so a regression is a timeout, not a slow pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use super::support::{
    build_runtime, build_runtime_with_ext, fixture, parse_lines, read_json_line, spawn_rpc_duplex,
    type_of,
};

/// Everything the host wrote after the point the caller stopped reading, up to its return.
async fn rest_of(mut reader: tokio::io::BufReader<tokio::io::DuplexStream>) -> Vec<Value> {
    let mut text = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap() == 0 {
            break;
        }
        text.push_str(&line);
    }
    parse_lines(text.as_bytes())
}

/// A provider whose first response never comes (until `release` is notified, which no test does):
/// `started` is notified when the request reaches it, so a test knows the run is in flight.
fn hanging_provider(started: Arc<Notify>, release: Arc<Notify>) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::async_factory(
        move |_ctx, _opts, _state, _model| {
            let started = started.clone();
            let release = release.clone();
            async move {
                started.notify_one();
                release.notified().await;
                faux_assistant_message(vec![faux_text("never reached")], StopReason::Stop)
            }
        },
    )]);
    faux
}

/// A run that is waiting on the model when stdin ends is aborted, and the loop returns after the
/// abort has settled — it does not wait for the model. The closing events of the aborted run are
/// still written (`agent_settled` is the proof the abort settled the run, rather than the loop
/// giving up on it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eof_aborts_a_run_in_flight_instead_of_waiting_for_it() {
    let fx = fixture();
    let started = Arc::new(Notify::new());
    let faux = hanging_provider(started.clone(), Arc::new(Notify::new()));
    let runtime = build_runtime(&fx, faux).await;
    let (mut client_tx, client_reader, rpc) = spawn_rpc_duplex(runtime);

    client_tx
        .write_all(b"{\"type\":\"prompt\",\"id\":\"p\",\"message\":\"go\"}\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), started.notified())
        .await
        .expect("the request reaches the model");

    let closed = Instant::now();
    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(20), rpc)
        .await
        .expect("run_rpc must return after stdin ends; the run is parked on the model, so a wait for it never ends")
        .unwrap();
    let elapsed = closed.elapsed();

    let lines = rest_of(client_reader).await;
    assert!(
        super::support::has_settled(&lines),
        "the abort settled the run before the loop returned: {lines:?}"
    );
    let aborted = lines.iter().any(|l| {
        type_of(l) == "message_end"
            && l["message"]["role"] == "assistant"
            && l["message"]["stopReason"] == "aborted"
    });
    assert!(aborted, "the run ended as an aborted turn: {lines:?}");
    assert!(
        elapsed < Duration::from_secs(4),
        "an aborted run settles in moments; waiting out the bound would take 5 s: {elapsed:?}"
    );
}

/// A `bash` command still running when stdin ends is cancelled (pi's `dispose()` calls
/// `abortBash()`), and its response is still written, as a cancelled one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eof_cancels_a_running_bash_command_and_still_answers_it() {
    let fx = fixture();
    let runtime = build_runtime(&fx, Arc::new(FauxProvider::new())).await;
    let session = runtime.session().await;
    let (mut client_tx, client_reader, rpc) = spawn_rpc_duplex(runtime);

    // Longer than the wait below, so a wait for the command is a timeout.
    client_tx
        .write_all(b"{\"type\":\"bash\",\"id\":\"b\",\"command\":\"sleep 25\"}\n")
        .await
        .unwrap();
    for _ in 0..2000 {
        if session.is_bash_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(session.is_bash_running(), "fixture: the command is running");

    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(20), rpc)
        .await
        .expect("run_rpc must return after stdin ends, not after the command does")
        .unwrap();

    let lines = rest_of(client_reader).await;
    let answer = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b")
        .unwrap_or_else(|| panic!("no bash response in {lines:?}"));
    assert_eq!(
        answer["data"]["cancelled"], true,
        "the command was cancelled, not run to the end: {answer}"
    );
}

/// A native extension whose `tool_call` handler asks the user to confirm something — the shape of a
/// permission prompt — and records what the dialog came back with.
#[derive(Default)]
struct AskingExt {
    services: std::sync::Mutex<Option<Arc<dyn HostServices>>>,
    answer: Arc<std::sync::Mutex<Option<bool>>>,
}

#[async_trait::async_trait]
impl NativeExtension for AskingExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("asking-ext")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut slot) = self.services.lock() {
            *slot = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::ToolCall]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::ToolCall { .. })
            && let Some(services) = self.services.lock().ok().and_then(|slot| slot.clone())
        {
            // A synchronous host call, as a guest's `ui.confirm` is: it parks this worker until the
            // dialog is answered.
            let answer = services.confirm(
                "Proceed?",
                "a run is waiting for this",
                &cyrup_ext::host::DialogOptions::default(),
            );
            if let Ok(mut slot) = self.answer.lock() {
                *slot = Some(answer);
            }
        }
        HookOutcome::Noop
    }
}

/// A run parked on a dialog when stdin ends is released: with the input closed nobody can answer
/// the dialog, so it is answered as cancelled and the run is aborted behind it. The dialog's
/// handler is a synchronous host call that an abort cannot interrupt, so without the cancellation
/// the loop would only come back when its bound ran out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eof_answers_the_open_dialogs_as_cancelled_and_releases_the_run_parked_on_one() {
    let fx = fixture();
    let ext = Arc::new(AskingExt::default());
    let answer = ext.answer.clone();
    let faux = Arc::new(FauxProvider::new());
    // The model asks for a tool call, which the extension's `tool_call` handler then holds on the
    // dialog.
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call(
                "read",
                serde_json::json!({ "path": "a.txt" }),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("unreachable")], StopReason::Stop),
    ]);
    let runtime = build_runtime_with_ext(&fx, faux, ext).await;
    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    client_tx
        .write_all(b"{\"type\":\"prompt\",\"id\":\"p\",\"message\":\"go\"}\n")
        .await
        .unwrap();
    // The run is parked on the dialog once the host has forwarded it to the client.
    let request = loop {
        let line =
            tokio::time::timeout(Duration::from_secs(20), read_json_line(&mut client_reader))
                .await
                .expect("the handler's dialog reaches the client");
        if type_of(&line) == "extension_ui_request" {
            break line;
        }
    };
    assert_eq!(request["method"], "confirm");

    let closed = Instant::now();
    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(20), rpc)
        .await
        .expect("run_rpc must return after stdin ends")
        .unwrap();
    let elapsed = closed.elapsed();

    assert_eq!(
        *answer.lock().unwrap(),
        Some(false),
        "the dialog nobody can answer any more was answered as cancelled"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the run was released by the cancelled dialog, not by the loop giving up after its 5 s \
         bound: {elapsed:?}"
    );
}
