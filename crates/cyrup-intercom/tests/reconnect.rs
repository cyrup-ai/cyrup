//! ICOM-003 — `ensureConnected` / `scheduleReconnect` proof, against a REAL broker process.
//!
//! Every test here drives a FAILURE first and asserts the recovery, because a happy-path connect
//! proves nothing about a reconnect policy:
//!   1. [`a_broker_drop_reconnects_and_the_session_can_send_again`] — connected, broker killed, the
//!      ladder heals the session on its own and a send works again. Pre-fix, the inbound loop just
//!      `break`ed on `Disconnected` and `SharedIntercomState::client()` kept handing out a dead
//!      client forever, so the send at the end failed with "not connected".
//!   2. [`a_tool_connect_that_is_refused_succeeds_on_the_next_call`] — a broker that refuses (its
//!      launch command exits immediately), then accepts. Pre-fix there was exactly one connect
//!      attempt per session, in the `SessionStart` task, so the tool stayed permanently broken.
//!   3. [`a_deliberate_shutdown_never_reconnects`] — the whole production `IntercomExtension`
//!      lifecycle: after `SessionShutdown`, killing the broker must NOT arm a backoff rung.
//!
//! Upstream reference: `pi-intercom` `git show v0.7.0:index.ts` — `getReconnectDelayMs` :564-567,
//! `scheduleReconnect` :794-809, `ensureConnected` :810-861, the disconnect handler :779-789, the
//! startup connect :952-965 and the teardown :1060-1064.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolUpdate, ToolUpdateSink};
use cyrup_ext::{HostCtx, HostEvent, NativeExtension};
use cyrup_intercom::config::load_config;
use cyrup_intercom::connect::{self, ConnectParams, ConnectReason};
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::{IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};
use cyrup_intercom::transport::spawn::wait_for_broker;

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn broker_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"))
}

fn registration(name: &str) -> SessionRegistration {
    SessionRegistration {
        name: Some(name.to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id(),
        started_at: now_ms(),
        last_activity: now_ms(),
        status: None,
    }
}

/// Point this agent dir's intercom config at `command` as the broker launch command. This is what
/// makes the reconnect ladder's own `ensure_broker` (pi `spawnBrokerIfNeeded` inside
/// `ensureConnected`, `index.ts:828`) launch the REAL broker binary instead of re-execing the test
/// harness, and it is also how test 2 flips a refusing broker into an accepting one.
fn write_broker_command(intercom_dir: &Path, command: &Path) {
    std::fs::create_dir_all(intercom_dir).expect("create intercom dir");
    let body = serde_json::json!({
        "brokerCommand": command.to_string_lossy(),
        "brokerArgs": [],
    });
    std::fs::write(
        cyrup_intercom::config::config_path(intercom_dir),
        serde_json::to_string(&body).expect("serialize config"),
    )
    .expect("write config.json");
}

fn spawn_broker(agent_dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(broker_bin())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess")
}

/// Poll `predicate` until it holds or `budget` elapses.
async fn within<F: FnMut() -> bool>(budget: Duration, mut predicate: F) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn state_for(agent_dir: &Path) -> Arc<SharedIntercomState> {
    let config = load_config(&intercom_dir_path(agent_dir));
    Arc::new(SharedIntercomState::new(config, 600_000, PathBuf::from("/tmp/work")))
}

fn params_for(agent_dir: &Path) -> ConnectParams {
    ConnectParams {
        agent_dir: agent_dir.to_path_buf(),
        metadata: None,
        model: Some("test-model".to_string()),
    }
}

/// THE ICOM-003 regression: a live session whose broker dies must heal itself.
///
/// Pre-fix behavior (the defect): `inbound.rs`'s `Ok(InboundEvent::Disconnected(_)) => break` left
/// `state.client()` holding the dead client and no retry anywhere, so every later send/list/tool
/// failed for the rest of the process. This test fails against that: the final `send` returns
/// `not connected`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broker_drop_reconnects_and_the_session_can_send_again() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir, &broker_bin());
    let socket = broker_socket_path(&intercom_dir);

    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(5)).await.expect("broker up");

    let state = state_for(agent_dir.path());
    connect::begin_runtime(&state, params_for(agent_dir.path()));
    let client = connect::ensure_connected(&state, ConnectReason::Startup)
        .await
        .expect("the startup connect registers");
    assert!(client.is_connected());
    let id_before = client.session_id().expect("broker assigned a session id");

    // An ask that is in flight when the socket drops. pi rejects the reply waiter on the disconnect
    // edge (`index.ts:783-784`); it must NOT hang across the reconnect waiting for an answer the
    // broker can never redeliver (there is no mailbox — delivery is at-most-once).
    let inflight = state
        .waiter
        .register("peer-session".to_string(), "q-inflight".to_string())
        .expect("registers the outbound ask");

    // --- FAILURE: the broker dies underneath the live session. ---
    broker.kill().await.expect("kill the broker");

    assert!(
        within(Duration::from_secs(5), || state.client().is_none()).await,
        "the disconnect edge must drop the dead client"
    );
    let inflight_result = tokio::time::timeout(Duration::from_secs(2), inflight)
        .await
        .expect("the in-flight ask resolves on the disconnect edge, it does not hang")
        .expect("the waiter slot was resolved");
    let reason = inflight_result.expect_err("the ask fails; it is not silently delivered");
    assert!(
        reason.starts_with("Disconnected while waiting for reply:"),
        "pi's disconnect reason must survive to the caller, got: {reason}"
    );
    assert!(state.connect.reconnect_armed(), "the disconnect edge armed a backoff rung");

    // --- RECOVERY: the ladder fires (rung 0 = 1000 ms), respawns the broker and re-registers. ---
    assert!(
        within(Duration::from_secs(20), || state
            .client()
            .is_some_and(|c| c.is_connected()))
        .await,
        "the reconnect ladder must restore a live client without any further caller action"
    );
    let recovered = state.client().expect("a live client");
    assert_eq!(
        recovered.session_id(),
        Some(id_before.clone()),
        "the reconnect re-registers under the SAME identity (broker takeover), not a second one"
    );
    assert_eq!(state.connect.attempt(), 0, "a successful connect resets the backoff ladder");

    // The recovered connection genuinely works end to end.
    let peer = IntercomClient::connect(&socket, registration("peer"), Some("peer-session".to_string()))
        .await
        .expect("a peer registers on the respawned broker");
    let sessions = recovered.list_sessions().await.expect("list over the recovered connection");
    assert!(sessions.iter().any(|s| s.id == id_before), "we are registered under our old id");
    let sent = recovered
        .send("peer-session", SendOptions { text: "after reconnect".to_string(), ..Default::default() })
        .await
        .expect("send over the recovered connection");
    assert!(sent.delivered, "the reconnected session can reach a peer again: {sent:?}");

    connect::shutdown(&state);
    if let Some(c) = state.client() {
        c.disconnect();
    }
    peer.disconnect();
}

/// A broker that REFUSES (its launch command exits immediately) and then ACCEPTS, observed at the
/// real `intercom` TOOL boundary — pi routes every tool call through `ensureConnected("tool")`
/// (`index.ts:1231,1477`).
///
/// Pre-fix, `IntercomTool::dispatch` read `state.client()` directly: after the one `SessionStart`
/// connect had failed, `client()` was `None` forever and this tool returned
/// "intercom is not connected to the broker" for the rest of the session. This test fails against
/// that — the SECOND call still errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_after_a_refused_broker_retries_and_succeeds() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    // A "broker" that exits 1 the moment it is launched: `ensure_broker` fails fast.
    write_broker_command(&intercom_dir, Path::new("/bin/false"));

    let state = state_for(agent_dir.path());
    connect::begin_runtime(&state, params_for(agent_dir.path()));
    let tool = IntercomTool::new(state.clone());

    // --- FAILURE: the broker refuses, so the tool call fails. ---
    let err = tool
        .execute(
            ToolCallId::from("tc-1"),
            serde_json::json!({ "action": "list" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .err()
        .map(|e| e.to_string())
        .expect("a broker that exits immediately must fail the tool call");
    assert!(err.contains("not connected to the broker"), "{err}");
    assert!(state.client().is_none());
    assert!(
        !state.connect.reconnect_armed(),
        "a tool-reason failure surfaces to its caller without arming a retry storm (index.ts:847-849)"
    );

    // --- The broker starts accepting. The very next tool call must connect and work. ---
    write_broker_command(&intercom_dir, &broker_bin());
    let result = tool
        .execute(
            ToolCallId::from("tc-2"),
            serde_json::json!({ "action": "list" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("the retry spawns a working broker, registers, and lists");
    let text = result
        .content
        .iter()
        .map(|c| match c {
            cyrup_core::Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect::<String>();
    assert!(text.contains("Current session"), "the tool now sees a live broker: {text}");
    assert!(state.client().is_some_and(|c| c.is_connected()));

    connect::shutdown(&state);
    if let Some(c) = state.client() {
        c.disconnect();
    }
}

/// Hazard 3: a deliberate shutdown must never reconnect. Driven through the REAL extension
/// lifecycle (`SessionStart` → `SessionShutdown`), so this covers the production wiring and not just
/// the supervisor's internal guards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deliberate_shutdown_never_reconnects() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir, &broker_bin());
    let socket = broker_socket_path(&intercom_dir);

    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(5)).await.expect("broker up");

    let ext = IntercomExtension::new(
        agent_dir.path().to_path_buf(),
        PathBuf::from("/tmp/work"),
        load_config(&intercom_dir),
        None,
    )
    .expect("build the extension");
    let ctx = HostCtx::event(cyrup_ext::ExtMode::Print, false, agent_dir.path().to_path_buf());

    let _ = ext.on_event(&HostEvent::SessionStart { reason: "test".to_string() }, &ctx).await;
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(20), || state.client().is_some_and(|c| c.is_connected())).await,
        "the session connects on SessionStart"
    );

    let _ = ext.on_event(&HostEvent::SessionShutdown { reason: "test".to_string() }, &ctx).await;
    assert!(state.connect.is_shutting_down());
    assert!(state.client().is_none());
    assert!(!state.connect.reconnect_armed(), "shutdown leaves no armed backoff rung");

    // --- FAILURE AFTER SHUTDOWN: the broker dies. Nothing may reconnect. ---
    broker.kill().await.expect("kill the broker");
    // Well past rung 0's 1000 ms backoff.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!state.connect.reconnect_armed(), "a post-shutdown disconnect must not arm the ladder");
    assert!(state.client().is_none(), "and must not resurrect a client");

    let err = connect::ensure_connected(&state, ConnectReason::Background)
        .await
        .err()
        .map(|e| e.to_string())
        .expect("an explicit connect after shutdown is refused");
    assert!(err.contains("shutting down"), "{err}");
    assert!(!state.connect.reconnect_armed(), "the refusal armed nothing either");
}
