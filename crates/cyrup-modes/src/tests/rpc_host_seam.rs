//! RPC-host correctness: the two ways `run_rpc` can lose touch with reality.
//!
//! * **SEAM-021** — `run_rpc` must RETURN when stdin reaches EOF. Pi's RPC host wires
//!   `process.stdin.on("end", () => void shutdown())` unconditionally (`rpc-mode.ts:799-802`), and
//!   its `steer`/`follow_up` arms (`rpc-mode.ts:417-425`) hold no in-flight state at all. cyrup
//!   defers the EOF exit until a run settles, which is only sound if the "a run is in flight" latch
//!   is set exclusively when a run was actually STARTED — `AgentSession::steer`/`follow_up` only
//!   push onto the pending queues (`session.rs` `_queueSteer`/`_queueFollowUp` port), so a steer on
//!   an idle session produces no `agent_settled` and latching there wedges the loop forever.
//!
//! * **SEAM-022** — the host must re-acquire the active session whenever the runtime REPLACES it,
//!   from any path. Pi hands the runtime a `rebindSession` callback (`rpc-mode.ts:312-314`,
//!   defined `:316-360`) which `finishSessionReplacement` (`agent-session-runtime.ts:187-190`)
//!   invokes on every replacement — including one an extension triggered via
//!   `ctx.newSession()`. Deriving the rebind from the RPC command NAME instead misses exactly that
//!   case, and every later command is then serviced by the disposed session.
//!
//! Both tests assert OBSERVABLE behavior: that the future completes inside a timeout, and that a
//! `get_state` issued after the swap reports the NEW session's id.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    CommandDescriptor, ControlOp, ExtError, HostCtx, HostEvent, HookOutcome, HostServices, InitApi,
    NativeExtension,
};
use crate::run_rpc;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

// ---------------------------------------------------------------------------------------------
// Fixture
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

async fn build_runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let factory = Arc::new(SessionFactory::new(faux_ok(), base_config(fx)));
    AgentSessionRuntime::create(factory, SessionTarget::New).await.expect("build runtime")
}

async fn build_runtime_with(
    fx: &Fixture,
    ext: Arc<dyn NativeExtension>,
) -> Arc<AgentSessionRuntime> {
    let factory = Arc::new(SessionFactory::new(faux_ok(), base_config(fx)).with_native_extension(ext));
    AgentSessionRuntime::create(factory, SessionTarget::New).await.expect("build runtime")
}

fn parse_lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .expect("utf8 output")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("valid json line"))
        .collect()
}

/// Feed `lines` to `run_rpc` over a REAL async pipe whose write half is then DROPPED (a genuine
/// stdin EOF, not a pre-filled cursor), and require the whole loop to finish inside `budget`.
async fn drive_rpc_to_eof(
    runtime: &AgentSessionRuntime,
    lines: &str,
    budget: Duration,
) -> Vec<Value> {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let payload = lines.to_string();
    let feeder = tokio::spawn(async move {
        let mut client = client;
        client.write_all(payload.as_bytes()).await.expect("write commands");
        client.flush().await.expect("flush commands");
        // Dropping the write half closes the pipe — the reader observes EOF, exactly as a scripted
        // client closing its child's stdin does.
        drop(client);
    });

    let mut out: Vec<u8> = Vec::new();
    let ran = tokio::time::timeout(
        budget,
        run_rpc(runtime, tokio::io::BufReader::new(server), &mut out),
    )
    .await;
    feeder.await.expect("feeder task");
    ran.expect("run_rpc must RETURN at stdin EOF (SEAM-021 — it hung instead)")
        .expect("run_rpc completes without error");
    parse_lines(&out)
}

// ---------------------------------------------------------------------------------------------
// SEAM-021 — the EOF hang
// ---------------------------------------------------------------------------------------------

/// A `steer` on an IDLE session enqueues a message and starts NO run, so no `agent_settled` will
/// ever be observed for it. Latching the in-flight gate there makes the EOF exit condition
/// (`!reader_open && !in_flight && dispatches.is_empty()`) permanently false and `run_rpc` never
/// returns — which in turn defeats `run_rpc_dispatch`'s `runtime.dispose()`, so `session_shutdown`
/// is never emitted either. Pi's `case "steer"` is just `await session.steer(...)` with no gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_on_an_idle_session_does_not_wedge_the_eof_exit() {
    let fx = fixture();
    let runtime = build_runtime(&fx).await;

    let lines = concat!(r#"{"type":"steer","message":"hello","id":"s"}"#, "\n");
    let out = drive_rpc_to_eof(&runtime, lines, Duration::from_secs(10)).await;

    let steer = out
        .iter()
        .find(|l| l["command"] == "steer")
        .unwrap_or_else(|| panic!("no steer response in {out:?}"));
    assert_eq!(steer["success"], true, "the steer itself is accepted: {steer}");
}

/// The `follow_up` twin of the above — same latch, same wedge.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_up_on_an_idle_session_does_not_wedge_the_eof_exit() {
    let fx = fixture();
    let runtime = build_runtime(&fx).await;

    let lines = concat!(r#"{"type":"follow_up","message":"later","id":"f"}"#, "\n");
    let out = drive_rpc_to_eof(&runtime, lines, Duration::from_secs(10)).await;

    let fu = out
        .iter()
        .find(|l| l["command"] == "follow_up")
        .unwrap_or_else(|| panic!("no follow_up response in {out:?}"));
    assert_eq!(fu["success"], true, "the follow_up itself is accepted: {fu}");
}

/// A steer that FAILS preflight (`_throwIfExtensionCommand`, agent-session.ts:1312-1321) must not
/// latch either — the pre-fix code set the flag BEFORE the call, so even the rejection wedged EOF.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_steer_does_not_wedge_the_eof_exit() {
    let fx = fixture();
    let ext: Arc<dyn NativeExtension> = Arc::new(SwapExt::default());
    let runtime = build_runtime_with(&fx, ext).await;

    let lines = concat!(r#"{"type":"steer","message":"/swap","id":"s"}"#, "\n");
    let out = drive_rpc_to_eof(&runtime, lines, Duration::from_secs(10)).await;

    let steer = out
        .iter()
        .find(|l| l["command"] == "steer")
        .unwrap_or_else(|| panic!("no steer response in {out:?}"));
    assert_eq!(
        steer["success"], false,
        "an extension command cannot be queued via steer (Pi throws): {steer}"
    );
}

// ---------------------------------------------------------------------------------------------
// SEAM-022 — rebinding after an EXTENSION-triggered replacement
// ---------------------------------------------------------------------------------------------

/// A native built-in whose `/swap` command queues a runtime-tier `ControlOp::NewSession` through
/// the same `HostServices::control` seam a wasm guest's `control.*` import reaches. The RPC line
/// that triggers it is an ordinary `{"type":"prompt","message":"/swap"}` — the command NAME the
/// pre-fix rebind predicate inspects is `"prompt"`, never one of the four session-replacing verbs.
#[derive(Default)]
struct SwapExt {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for SwapExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("rpc-host-seam-swap")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "swap",
            CommandDescriptor {
                description: "replace the active session".to_string(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        _name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let svc = self
            .services
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| ExtError::Component("no host services".into()))?;
        svc.control(ControlOp::NewSession { opts: json!({}) }).map_err(ExtError::Component)?;
        Ok(Some(String::new()))
    }
}

/// After an extension control op replaces the active session, the NEXT command must be serviced by
/// the NEW session. Pi guarantees this by having the runtime call the host's `rebindSession`
/// (`agent-session-runtime.ts:187-190`); cyrup's runtime signals the same thing by bumping
/// `watch_generation()`. Deriving the rebind from the command name leaves the loop holding the
/// disposed session, so `get_state` reports the OLD session id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_after_an_extension_session_swap_reaches_the_new_session() {
    let fx = fixture();
    let ext: Arc<dyn NativeExtension> = Arc::new(SwapExt::default());
    let runtime = build_runtime_with(&fx, ext).await;
    let first_id = runtime.session().await.session_id().to_string();

    let lines = concat!(
        r#"{"type":"prompt","message":"/swap","id":"swap"}"#,
        "\n",
        r#"{"type":"get_state","id":"after"}"#,
        "\n",
    );
    let out = drive_rpc_to_eof(&runtime, lines, Duration::from_secs(20)).await;

    // The op really did replace the session at the runtime tier.
    let replaced_id = runtime.session().await.session_id().to_string();
    assert_ne!(replaced_id, first_id, "ctx.newSession() replaced the runtime's active session");

    let after = out
        .iter()
        .find(|l| l["command"] == "get_state" && l["id"] == "after")
        .unwrap_or_else(|| panic!("no get_state response in {out:?}"));
    assert_eq!(after["success"], true, "get_state succeeded: {after}");
    assert_eq!(
        after["data"]["sessionId"].as_str(),
        Some(replaced_id.as_str()),
        "a command issued AFTER an extension-triggered swap must be serviced by the NEW session \
         (SEAM-022); it was answered by {first_id}"
    );
}

// ---------------------------------------------------------------------------------------------
// SEAM-023 / SEAM-024 — the `abort` verb over the wire
// ---------------------------------------------------------------------------------------------

/// A 10-minute first retry backoff — long enough that "the abort did not cancel it" is a hang, not
/// a slow pass.
fn slow_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "retry",
        serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 600_000}),
    )
    .unwrap();
    cli
}

/// A runtime whose first turn is a RETRYABLE transient error and whose second (the auto-retry
/// continuation, which must never be reached here) is a clean success.
async fn build_retrying_runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        cyrup_provider::faux::faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            cyrup_provider::faux::FauxMessageOptions {
                error_message: Some("overloaded".into()),
                ..Default::default()
            },
        ),
        faux_assistant_message(vec![faux_text("must not appear")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(
        SessionFactory::new(provider, base_config(fx)).cli_settings(slow_retry_settings()),
    );
    AgentSessionRuntime::create(factory, SessionTarget::New).await.expect("build runtime")
}

/// The RPC `abort` verb, end to end, against a session sitting in provider-retry backoff.
///
/// Pi's handler is `case "abort": { await session.abort(); return success(id, "abort"); }`
/// (rpc-mode.ts:427-430) over an `abort()` that is `abortRetry(); agent.abort(); await
/// waitForIdle()` (agent-session.ts:1542-1546). Both halves are load-bearing HERE:
///
///  * SEAM-023 — without `abortRetry()` the backoff keeps sleeping, so the run never reaches
///    `agent_settled`, so the loop's `in_flight` latch never clears, so the EOF exit condition
///    (`!reader_open && !in_flight && dispatches.is_empty()`) is permanently false and `run_rpc`
///    never returns. A client that aborts and disconnects leaks the process.
///  * SEAM-024 — the success reply must mean "the run has stopped", so a client that immediately
///    re-prompts is not racing a dying run.
///
/// The abort line is written only once the session has ACTUALLY entered the backoff, so this tests
/// the retry path rather than a mid-stream cancel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_over_rpc_stops_a_retrying_run_and_lets_the_loop_exit() {
    let fx = fixture();
    let runtime = build_retrying_runtime(&fx).await;

    let (client, server) = tokio::io::duplex(64 * 1024);
    let rt = Arc::clone(&runtime);
    let feeder = tokio::spawn(async move {
        let mut client = client;
        client
            .write_all(concat!(r#"{"type":"prompt","message":"go","id":"p"}"#, "\n").as_bytes())
            .await
            .expect("write prompt");
        client.flush().await.expect("flush");
        // Wait for the run to reach its retry backoff (Pi `isRetrying`, agent-session.ts:2553).
        for _ in 0..1000 {
            if rt.session().await.is_retrying() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(rt.session().await.is_retrying(), "fixture: the run must reach retry backoff");
        client
            .write_all(concat!(r#"{"type":"abort","id":"a"}"#, "\n").as_bytes())
            .await
            .expect("write abort");
        client.flush().await.expect("flush");
        drop(client); // genuine stdin EOF
    });

    let mut out: Vec<u8> = Vec::new();
    let ran = tokio::time::timeout(
        Duration::from_secs(20),
        run_rpc(&runtime, tokio::io::BufReader::new(server), &mut out),
    )
    .await;
    feeder.await.expect("feeder task");
    ran.expect(
        "run_rpc must return after an `abort` + EOF — the aborted retry backoff kept the \
         in-flight latch set (SEAM-023/024)",
    )
    .expect("run_rpc completes without error");

    let out = parse_lines(&out);
    let abort = out
        .iter()
        .find(|l| l["command"] == "abort")
        .unwrap_or_else(|| panic!("no abort response in {out:?}"));
    assert_eq!(abort["success"], true, "the abort itself succeeded: {abort}");

    // The cancelled backoff reported itself, and the retry never produced a second turn.
    let kinds: Vec<&str> =
        out.iter().filter_map(|l| l["type"].as_str().or_else(|| l["command"].as_str())).collect();
    assert!(
        out.iter().any(|l| l["type"] == "auto_retry_end" && l["success"] == false),
        "the aborted backoff must emit auto_retry_end{{success:false}}: {kinds:?}"
    );
    assert!(
        !out.iter().any(|l| l["type"] == "message_end"
            && l["message"]["content"].to_string().contains("must not appear")),
        "the aborted retry must not have produced a second assistant turn: {kinds:?}"
    );
}
