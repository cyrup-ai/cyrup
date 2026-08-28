//! FULLY-WIRED regression proof for pi `dismissIncomingAsk` (`pi-intercom` v0.10.1
//! `index.ts:529-531`):
//!
//! ```text
//! function dismissIncomingAsk(messageId: string): void {
//!   replyTracker.dismissPendingAsk(messageId);
//! }
//! ```
//!
//! ICOM-035 — this file used to pin v0.7.0's TWO-part body, whose second half spliced the id out of
//! `pendingIdleMessages`. Upstream deleted that queue wholesale at v0.9.3 (`25ffb96`) and deleted
//! its tests with it; `flush_idle_messages` and `SharedIntercomState::pending_inbound_len` no
//! longer exist in cyrup either. The SURVIVING contract, which is what this file now pins:
//!
//! 1. a message arriving at a BUSY, interactive session is **steered onto the live run
//!    immediately** (`decide_inbound_policy` ⇒ `InboundPolicy::Steer`, pi `index.ts:876`'s
//!    `deliverAs: "steer"`) — it is not parked anywhere;
//! 2. answering it mid-run through the real `intercom{reply}` tool drops it from the reply
//!    tracker's PENDING ASKS (`replyTracker.dismissPendingAsk`), so a later `intercom{list}` does
//!    not re-surface an ask the agent already answered;
//! 3. and because nothing holds a second copy, the message is delivered **exactly once** — going
//!    idle afterwards re-injects nothing.
//!
//! Everything below the assertions is real: a genuine `cyrup-intercom-broker` child process, two
//! real `IntercomClient`s over the real Unix socket, the real `spawn_inbound_loop` and the real
//! `IntercomTool` dispatch, observed through a `HostServices` that records `inject_message`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolUpdateSink};
use cyrup_ext::HostServices;
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::inbound::spawn_inbound_loop;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::{IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::now_ms;
use cyrup_intercom::transport::spawn::wait_for_broker;
use crate::common::registration;

/// A `HostServices` with a SETTABLE `is_idle` (the live run-in-flight signal `decide_inbound_policy`
/// reads) that records every `inject_message` — so the delivery COUNT is directly observable rather
/// than inferred.
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
        _details: Option<&serde_json::Value>,
        _trigger_turn: bool,
    ) -> Result<(), String> {
        self.injected.lock().unwrap_or_else(|e| e.into_inner()).push(content.to_string());
        Ok(())
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
async fn replying_mid_run_dismisses_the_pending_ask_and_never_redelivers() {
    let broker_bin = crate::support::bins::intercom_broker();
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

    // THIS session — busy (a run is in flight) and interactive, so an inbound message is STEERED.
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
    // policy (busy + has_ui ⇒ `InboundPolicy::Steer`).
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
            ..Default::default()
        })
        .await
        .expect("peer's ask is accepted by the broker");

    // v0.9.3 onward: a busy INTERACTIVE session steers the message onto the live run at once
    // (`decide_inbound_policy` ⇒ `Steer`; pi `index.ts:876`'s `deliverAs: "steer"`). Exactly one
    // delivery, and it happens while the run is still in flight.
    assert!(
        wait_until(Duration::from_secs(10), || host.injected().len() == 1).await,
        "a busy interactive session must be STEERED with the inbound message, not parked; \
         injected: {:?}",
        host.injected()
    );
    // …and the ask is recorded as pending, so `intercom{reply}` can resolve it without a `replyTo`.
    assert_eq!(
        state.tracker.lock().unwrap().list_pending(now_ms()).len(),
        1,
        "the inbound ask must be tracked as pending until it is answered"
    );

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

    // THE CONTRACT: `dismissIncomingAsk` drops the ANSWERED ask from the tracker's pending set
    // (`v0.10.1 index.ts:529-531`, called from the delivered-`reply` arm at `:2226`). Without it
    // the agent's own `intercom{list}` keeps advertising a question it has already answered.
    assert!(
        state.tracker.lock().unwrap().list_pending(now_ms()).is_empty(),
        "the answered inbound ask must be dismissed from the reply tracker's pending asks"
    );

    // THE CONSEQUENCE: nothing holds a second copy, so going idle re-injects nothing. (Pre-v0.9.3
    // this was the `pendingIdleMessages` flush; the queue is gone, and the delivery count is the
    // observable that survives it.)
    host.set_idle(true);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        host.injected().len(),
        1,
        "the inbound message must be delivered EXACTLY once (steered), never re-injected on idle; \
         injected: {:?}",
        host.injected()
    );

    peer_client.disconnect();
    agent_client.disconnect();
    let _ = broker.kill().await;
}
