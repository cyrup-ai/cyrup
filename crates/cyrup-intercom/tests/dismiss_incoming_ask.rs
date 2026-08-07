//! FULLY-WIRED regression proof for pi `dismissIncomingAsk`'s pending-idle SPLICE
//! (`pi-intercom` v0.7.0 `index.ts:455-459`):
//!
//! ```text
//! function dismissIncomingAsk(messageId: string): void {
//!   replyTracker.dismissPendingAsk(messageId);
//!   const queuedIndex = pendingIdleMessages.findIndex((entry) => entry.message.id === messageId);
//!   if (queuedIndex >= 0) pendingIdleMessages.splice(queuedIndex, 1);
//! }
//! ```
//!
//! The scenario the splice exists for: a peer messages this session WHILE a run is in flight. The
//! production inbound loop records the ask in the `ReplyTracker`, surfaces it to the human, and —
//! because the session is busy and interactive — parks it in the pending-idle queue. The running
//! agent can see the surfaced ask and answer it mid-run with `intercom{reply}`. Cyrup's `reply`
//! only reached the tracker, so the answered message stayed in the queue and
//! `flush_idle_messages` re-injected it as a fresh turn-driving delivery once the run ended.
//!
//! Everything below the assertions is real: a genuine `cyrup-intercom-broker` child process, two
//! real `IntercomClient`s over the real Unix socket, the real `spawn_inbound_loop`, the real
//! `IntercomTool` dispatch, and the real `flush_idle_messages` drain observed through a
//! `HostServices` that records `inject_message`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolUpdateSink};
use cyrup_ext::HostServices;
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::inbound::{flush_idle_messages, spawn_inbound_loop};
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::{IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{now_ms, SessionRegistration};
use cyrup_intercom::transport::spawn::wait_for_broker;

/// A `HostServices` with a SETTABLE `is_idle` (the live run-in-flight signal the inbound policy and
/// the flush both read) that records every `inject_message` — so a re-injected message is directly
/// observable rather than inferred.
struct IdleControlledHost {
    idle: AtomicBool,
    injected: Mutex<Vec<String>>,
}

impl IdleControlledHost {
    fn new(idle: bool) -> Self {
        Self { idle: AtomicBool::new(idle), injected: Mutex::new(Vec::new()) }
    }
    fn set_idle(&self, idle: bool) {
        self.idle.store(idle, Ordering::SeqCst);
    }
    fn injected(&self) -> Vec<String> {
        self.injected.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl HostServices for IdleControlledHost {
    fn is_idle(&self) -> bool {
        self.idle.load(Ordering::SeqCst)
    }
    fn append_entry(&self, _custom_type: &str, _data: &serde_json::Value) -> Result<String, String> {
        Ok("entry-1".to_string())
    }
    fn inject_message(
        &self,
        content: &str,
        _custom_type: Option<&str>,
        _display: bool,
        _trigger_turn: bool,
    ) -> Result<(), String> {
        self.injected.lock().unwrap_or_else(|e| e.into_inner()).push(content.to_string());
        Ok(())
    }
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

/// Poll `predicate` until it holds or `bound` elapses. Used instead of a fixed sleep so the test
/// stays honest under heavy CPU contention (a slow broker handshake must not read as a failure).
async fn wait_until(bound: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replying_mid_run_removes_the_message_from_the_pending_idle_queue() {
    let broker_bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");

    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess");
    wait_for_broker(&socket_path, Duration::from_secs(10))
        .await
        .expect("broker becomes health-connectable");

    // THIS session — busy (a run is in flight) and interactive, so an inbound message is parked.
    let agent_client = Arc::new(
        IntercomClient::connect(&socket_path, registration("agent"), Some("agent-session".into()))
            .await
            .expect("agent session connects"),
    );
    let host = Arc::new(IdleControlledHost::new(false));
    let state = Arc::new(SharedIntercomState::new(
        IntercomConfig::default(),
        600_000,
        PathBuf::from("/tmp/work"),
    ));
    state.set_client(Some(agent_client.clone()));
    state.set_host_services(host.clone());
    state.set_has_ui(true);
    // The REAL production inbound loop: it records the ask, surfaces it, and applies the delivery
    // policy (busy + has_ui ⇒ park in the pending-idle queue).
    spawn_inbound_loop(state.clone(), agent_client.clone());

    // The PEER stays connected for the whole test — the `reply` below only dismisses on a CONFIRMED
    // delivery, so dropping the peer early would make this pass for the wrong reason.
    let peer_client = Arc::new(
        IntercomClient::connect(&socket_path, registration("peer"), Some("peer-session".into()))
            .await
            .expect("peer session connects"),
    );
    let agent_id = agent_client.session_id().expect("agent has a broker-assigned id");
    peer_client
        .send(&agent_id, SendOptions {
            text: "should we ship it?".to_string(),
            attachments: None,
            reply_to: None,
            expects_reply: Some(true),
            message_id: None,
        })
        .await
        .expect("peer's ask is accepted by the broker");

    // The busy interactive session parked it rather than steering the live run.
    assert!(
        wait_until(Duration::from_secs(10), || state.pending_inbound_len() == 1).await,
        "the inbound ask must be parked in the pending-idle queue while the run is in flight"
    );
    assert!(host.injected().is_empty(), "a busy session must not be steered mid-run");

    // The running agent answers the ask MID-RUN through the REAL `Tool::execute` entry point — the
    // same call the agent loop makes. `to`/`replyTo` are omitted, so `resolveReplyTarget` resolves
    // the single pending ask (pi `index.ts:1696`).
    let tool = IntercomTool::new(state.clone());
    let out = tool
        .execute(
            ToolCallId::from("call-1"),
            serde_json::json!({ "action": "reply", "message": "yes, ship it" }),
            CancelToken::new(),
            Box::new(|_| {}) as ToolUpdateSink,
        )
        .await
        .expect("the mid-run reply is delivered to the still-connected peer");
    let out_text: String = out
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(out_text.contains("Reply sent"), "the reply reports delivery: {out_text}");

    // THE DEFECT: pi splices the answered entry out of `pendingIdleMessages`. Without it the entry
    // survives here and the flush below replays it.
    assert_eq!(
        state.pending_inbound_len(),
        0,
        "the answered inbound ask must be removed from the pending-idle queue \
         (pi `dismissIncomingAsk`'s `pendingIdleMessages.splice(queuedIndex, 1)`)"
    );

    // THE CONSEQUENCE, at the real drain: the run ends, the flush runs, and nothing is re-injected.
    host.set_idle(true);
    flush_idle_messages(&state);
    // Give any surviving entry every chance to be delivered (the flush hops through the scheduler,
    // and `queue_idle_message` also armed the debounce) before asserting the negative.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(
        host.injected().is_empty(),
        "an already-answered inbound ask must NOT be re-injected once the run ends; injected: {:?}",
        host.injected()
    );

    peer_client.disconnect();
    agent_client.disconnect();
    let _ = broker.kill().await;
}
