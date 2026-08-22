//! SEAM-005 (+ its EXT-005 consumer) driven through the REAL RPC adapter.
//!
//! Two things are proved here that a session-level test cannot:
//!
//! 1. `agent_settled` reaches the WIRE — an RPC client sees `{"type":"agent_settled"}` exactly once
//!    per run, after the last `agent_end` (Pi emits it to every subscriber, agent-session.ts:585,
//!    and `rpc-mode.ts:354` forwards every subscribed event to stdout).
//! 2. The event is LOAD-BEARING, not decorative: Pi's RPC host acts on a pending `ctx.shutdown()`
//!    in exactly this arm — `if (event.type === "agent_settled") void checkShutdownRequested()`
//!    (rpc-mode.ts:355-358) — and nowhere else. So the test holds the client's write half OPEN (no
//!    EOF, the loop's only other exit) and asserts `run_rpc` returns anyway. Without the
//!    `agent_settled` arm the adapter would run forever.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::host::{ControlOp, HostServices};
use cyrup_ext::{CommandDescriptor, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension};
use crate::run_rpc;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionRuntime, SessionFactory};
use tokio::io::{AsyncWriteExt, BufReader};

use super::support::{base_config_no_ext, create_runtime, fixture, parse_lines, type_of, Fixture};

/// A native built-in exposing `/quitnow`, which calls the base-context `ctx.shutdown()` (Pi
/// `ctx.shutdown()`, extensions/types.ts:344 → `runner.shutdown()`, runner.ts:656-662).
#[derive(Default)]
struct QuitExt {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    /// Set by `/armquit`: from then on the MID-RUN `message_end` handler is what requests the
    /// shutdown, so the only checkpoint that can honour it is `agent_settled` (a run is in flight
    /// at the moment of the request, and Pi never shuts down mid-run).
    armed: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl NativeExtension for QuitExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("quit-ext")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::MessageEnd]);
        for (name, description) in [
            ("quitnow", "request a graceful host shutdown"),
            ("armquit", "request the shutdown from the NEXT run's message_end handler"),
        ] {
            api.register_command(
                name,
                CommandDescriptor {
                    description: description.into(),
                    completions: Vec::new(),
                },
            );
        }
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::MessageEnd { .. })
            && self.armed.load(std::sync::atomic::Ordering::SeqCst)
            && let Some(svc) = self.svc()
        {
            let _ = svc.control(ControlOp::Shutdown);
        }
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let svc = self.svc().ok_or_else(|| ExtError::Component("no host services".into()))?;
        if name == "armquit" {
            self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
            return Ok(Some(String::new()));
        }
        svc.control(ControlOp::Shutdown).map_err(ExtError::Component)?;
        Ok(Some(String::new()))
    }
}

impl QuitExt {
    fn svc(&self) -> Option<Arc<dyn HostServices>> {
        self.services.lock().ok().and_then(|g| g.clone())
    }
}

fn faux_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("rpc answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("rpc answer 2")], StopReason::Stop),
    ]);
    faux
}

async fn runtime_with(
    fx: &Fixture,
    ext: Option<Arc<QuitExt>>,
) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux_ok();
    let cfg = base_config_no_ext(fx);
    let target = cfg.target.clone();
    let mut factory = SessionFactory::new(provider, cfg);
    if let Some(e) = ext {
        factory = factory.with_native_extension(e as Arc<dyn NativeExtension>);
    }
    create_runtime(factory, target).await
}

/// An RPC client sees `agent_settled` on the wire, once per run, after the run's last `agent_end`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_emits_agent_settled_after_the_run() {
    let fx = fixture();
    let runtime = runtime_with(&fx, None).await;

    let input = concat!(r#"{"type":"prompt","id":"1","message":"hello"}"#, "\n");
    let reader = std::io::Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let types: Vec<&str> = lines.iter().map(type_of).collect();
    let settled: Vec<usize> =
        types.iter().enumerate().filter(|(_, t)| **t == "agent_settled").map(|(i, _)| i).collect();
    assert_eq!(settled.len(), 1, "exactly one agent_settled on the wire: {types:?}");
    let last_end = types
        .iter()
        .enumerate()
        .filter(|(_, t)| **t == "agent_end")
        .map(|(i, _)| i)
        .next_back()
        .expect("an agent_end on the wire");
    assert!(settled[0] > last_end, "agent_settled follows the run's last agent_end: {types:?}");
}

/// A loaded extension's `ctx.shutdown()` terminates `run_rpc` at the next SETTLE point, with the
/// client's input stream still OPEN.
///
/// The open reader is the point: EOF is the adapter's only other exit, so a `run_rpc` that returns
/// here can only have done so via the `agent_settled` → shutdown check. Before SEAM-005 there was no
/// `agent_settled` at all and `ControlOp::Shutdown` did not exist; the command's op was dropped by
/// the drain and this call would hang until the harness timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extension_shutdown_ends_rpc_at_the_next_settle_point() {
    let fx = fixture();
    let runtime = runtime_with(&fx, Some(Arc::new(QuitExt::default()))).await;

    // A real duplex pipe whose WRITE half this test keeps alive: the adapter never sees EOF.
    let (client, server) = tokio::io::duplex(4096);
    let (server_read, _server_write) = tokio::io::split(server);
    let (_client_read, mut client_write) = tokio::io::split(client);

    // `/armquit` only ARMS the extension (it requests nothing itself, so the command-tail check
    // below finds no pending shutdown); the request is then made from a MID-RUN `message_end`
    // handler during the following prompt. Pi is explicit that a shutdown asked for while a run is
    // in flight is honoured at the settle point, never mid-run.
    client_write
        .write_all(
            concat!(
                r#"{"type":"prompt","id":"1","message":"/armquit"}"#,
                "\n",
                r#"{"type":"prompt","id":"2","message":"hello"}"#,
                "\n",
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    client_write.flush().await.unwrap();

    let mut out: Vec<u8> = Vec::new();
    let finished = tokio::time::timeout(
        Duration::from_secs(20),
        run_rpc(&runtime, BufReader::new(server_read), &mut out),
    )
    .await;

    // Hold the write half until after the assertion so the reader genuinely never EOFs.
    drop(client_write);

    let finished = finished.expect(
        "run_rpc must return on the extension's shutdown request; the input stream was still open, \
         so a timeout here means the agent_settled -> shutdown check never fired",
    );
    finished.expect("rpc mode runs");

    let types: Vec<String> = parse_lines(&out).iter().map(|v| type_of(v).to_string()).collect();
    assert!(
        types.iter().any(|t| t == "agent_settled"),
        "the settle point the shutdown was honoured at is on the wire: {types:?}"
    );
    assert!(
        runtime.session().await.shutdown_requested(),
        "the extension's ctx.shutdown() actually latched on the live session"
    );
}

/// EXT-005: a shutdown requested by a COMMAND, with no agent run ever having happened, ends
/// `run_rpc` at the tail of that command.
///
/// Pi calls `checkShutdownRequested()` in TWO places: the `agent_settled` arm (rpc-mode.ts:355-358)
/// AND after every handled command — `await checkShutdownRequested();` at the end of
/// `handleInputLine`'s try block (rpc-mode.ts:786). The second is the one Pi's own canonical example
/// depends on: `coding-agent/examples/extensions/shutdown-command.ts` is a `/quit` COMMAND that
/// exits pi without any run. cyrup gated the exit on having observed an `agent_settled`, so that
/// exact extension did nothing — the client's stream is open here, so a `run_rpc` that hangs is the
/// pre-fix behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extension_shutdown_from_a_command_ends_rpc_without_any_run() {
    let fx = fixture();
    let runtime = runtime_with(&fx, Some(Arc::new(QuitExt::default()))).await;

    let (client, server) = tokio::io::duplex(4096);
    let (server_read, _server_write) = tokio::io::split(server);
    let (_client_read, mut client_write) = tokio::io::split(client);

    // ONE line, and it is not a run: the command handler calls `ctx.shutdown()` and returns.
    client_write
        .write_all(concat!(r#"{"type":"prompt","id":"1","message":"/quitnow"}"#, "\n").as_bytes())
        .await
        .unwrap();
    client_write.flush().await.unwrap();

    let mut out: Vec<u8> = Vec::new();
    let finished = tokio::time::timeout(
        Duration::from_secs(20),
        run_rpc(&runtime, BufReader::new(server_read), &mut out),
    )
    .await;
    drop(client_write);

    finished
        .expect(
            "run_rpc must return at the tail of the command that requested shutdown; the input \
             stream was still open and NO run ever started, so a timeout means the exit was still \
             gated on an agent_settled that can never arrive",
        )
        .expect("rpc mode runs");

    let types: Vec<String> = parse_lines(&out).iter().map(|v| type_of(v).to_string()).collect();
    assert!(
        !types.iter().any(|t| t == "agent_settled"),
        "no run happened, so there is no settle point — the exit came from the command tail: {types:?}"
    );
    assert!(
        runtime.session().await.shutdown_requested(),
        "the extension's ctx.shutdown() latched on the live session"
    );
}
