//! Regression proof for "`/intercom <target> <message>` writes no `intercom_sent` transcript entry".
//!
//! Upstream (`pi-intercom` `git show v0.7.0:index.ts:1878-1885`), on the compose-overlay result path
//! the `/intercom` command drives:
//! ```text
//!   pi.appendEntry("intercom_sent", {
//!     to: selectedSession.name || selectedSession.id,
//!     message: { text: result.text },
//!     messageId: result.messageId,
//!     timestamp: Date.now(),
//!   });
//! ```
//! Pre-fix `extension.rs` ran `compose_send(...)` and returned the result text with no
//! `append_entry` anywhere, so the slash-command leg was the ONLY send in the crate that left no
//! trace in the session transcript — the `intercom` tool's `send`/`ask`/`reply` arms all append
//! (`tools/intercom.rs`). The port doc §4.3 carve-out degrades pi's interactive OVERLAY to text; it
//! says nothing about dropping the persistence half.
//!
//! Everything here is real: a real broker subprocess, a real Unix socket, a real peer session, the
//! real `NativeExtension::execute_command` command-tier dispatch, and a `HostServices` sink
//! late-bound exactly as the builder binds the live session backend.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::{ExtMode, HostCtx, HostEvent, HostServices, NativeExtension};
use cyrup_intercom::config::load_config;
use cyrup_intercom::extension::{INTERCOM_COMMAND, IntercomExtension};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::{IntercomClient, InboundEvent};
use cyrup_intercom::transport::spawn::wait_for_broker;
use serde_json::Value;
use crate::common::{registration, spawn_broker, within, write_broker_command};

const MY_SESSION_ID: &str = "session-aaaabbbbccccdddd";
const PEER_SESSION_ID: &str = "session-1111222233334444";
const PEER_NAME: &str = "reviewer";

/// Records every `append_entry` the extension makes, and reports a live session id so the
/// production `SessionStart` connect registers normally.
struct RecordingSink {
    appended: Mutex<Vec<(String, Value)>>,
}

impl RecordingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self { appended: Mutex::new(Vec::new()) })
    }

    fn entries(&self) -> Vec<(String, Value)> {
        self.appended.lock().unwrap().clone()
    }
}

impl HostServices for RecordingSink {
    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        let mut g = self.appended.lock().unwrap();
        g.push((custom_type.to_string(), data.clone()));
        Ok(format!("entry-{}", g.len()))
    }
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
}

/// Pull inbound events off `rx` until a `Message` shows up (DRAIN rather than assuming the first
/// event is ours — presence/registration traffic shares this stream), or the budget expires.
async fn next_message(
    rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>,
    budget: Duration,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { message, .. })) => return Some(message.content.text),
            Ok(Ok(_)) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

/// THE REGRESSION. `/intercom reviewer <message>` must both deliver AND persist an `intercom_sent`
/// entry (pi `index.ts:1878-1884`).
///
/// Against the pre-fix `extension.rs` the two MIRROR assertions (the command's reply text and the
/// peer's actual receipt) still pass — the send leg always worked — while the `intercom_sent`
/// assertions fail because `sink.entries()` is empty. That is what makes the failing assertion
/// non-vacuous: it isolates the persistence half from the delivery half.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_intercom_send_appends_an_intercom_sent_entry() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);

    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(20)).await.expect("broker up");

    // The peer this session will address by NAME. Its receiver is subscribed BEFORE the send and
    // held open for the whole test, so the delivery cannot be missed by a subscribe/send race.
    let peer = Arc::new(
        IntercomClient::connect(&socket, registration(PEER_NAME), Some(PEER_SESSION_ID.to_string()))
            .await
            .expect("the peer registers"),
    );
    let mut peer_events = peer.subscribe();

    // This session, brought up through the REAL production `SessionStart` path.
    let sink = RecordingSink::new();
    let ext = IntercomExtension::new(
        agent_dir.path().to_path_buf(),
        PathBuf::from("/tmp/work"),
        load_config(&intercom_dir),
        None,
    )
    .expect("build the extension");
    ext.set_host_services(sink.clone());
    let ctx = HostCtx::command(ExtMode::Print, false, agent_dir.path().to_path_buf());
    let _ = ext.on_event(&HostEvent::SessionStart { reason: "test".to_string() }, &ctx).await;
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state.client().is_some_and(|c| c.is_connected())).await,
        "the session connects on SessionStart"
    );

    // --- The user types `/intercom reviewer please review the diff` ---
    let reply = ext
        .execute_command(INTERCOM_COMMAND, "reviewer please review the diff", &ctx)
        .await
        .expect("the command dispatches")
        .expect("the command produces output");

    // MIRROR 1 (green pre- AND post-fix): the command reports the send to the model/user.
    assert_eq!(reply, "Message sent to reviewer.", "the reply text is unchanged by this fix");

    // MIRROR 2 (green pre- AND post-fix): the peer really received it over the broker.
    let delivered = next_message(&mut peer_events, Duration::from_secs(20)).await;
    assert_eq!(
        delivered.as_deref(),
        Some("please review the diff"),
        "the send leg itself always worked; only the transcript entry was missing"
    );

    // THE FIX: the transcript now carries pi's `intercom_sent` entry.
    let entries = sink.entries();
    let sent: Vec<&(String, Value)> = entries.iter().filter(|(t, _)| t == "intercom_sent").collect();
    assert_eq!(sent.len(), 1, "exactly one intercom_sent entry, got: {entries:?}");
    let data = &sent[0].1;
    assert_eq!(
        data.get("to").and_then(Value::as_str),
        Some(PEER_NAME),
        "pi `to: selectedSession.name || selectedSession.id` — the RESOLVED peer's label: {data}"
    );
    assert_eq!(
        data.get("message").and_then(|m| m.get("text")).and_then(Value::as_str),
        Some("please review the diff"),
        "pi `message: {{ text: result.text }}`: {data}"
    );
    assert!(
        data.get("messageId").and_then(Value::as_str).is_some_and(|id| !id.is_empty()),
        "pi `messageId: result.messageId` — the broker-assigned id, so the entry is correlatable: {data}"
    );
    assert!(
        data.get("timestamp").and_then(Value::as_u64).is_some_and(|t| t > 0),
        "pi `timestamp: Date.now()`: {data}"
    );

    peer.disconnect();
    if let Some(c) = state.client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// CONTROL (green pre- AND post-fix): the argument-less `/intercom` renders the session picker and
/// sends NOTHING, so it must persist nothing either. Upstream only appends on
/// `result?.sent && result.messageId && result.text` (`index.ts:1878`). This is what proves the
/// assertion above is about a real send and not an unconditional append.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_intercom_picker_appends_nothing() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);

    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(20)).await.expect("broker up");

    let peer = Arc::new(
        IntercomClient::connect(&socket, registration(PEER_NAME), Some(PEER_SESSION_ID.to_string()))
            .await
            .expect("the peer registers"),
    );

    let sink = RecordingSink::new();
    let ext = IntercomExtension::new(
        agent_dir.path().to_path_buf(),
        PathBuf::from("/tmp/work"),
        load_config(&intercom_dir),
        None,
    )
    .expect("build the extension");
    ext.set_host_services(sink.clone());
    let ctx = HostCtx::command(ExtMode::Print, false, agent_dir.path().to_path_buf());
    let _ = ext.on_event(&HostEvent::SessionStart { reason: "test".to_string() }, &ctx).await;
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state.client().is_some_and(|c| c.is_connected())).await,
        "the session connects on SessionStart"
    );

    let rendered = ext
        .execute_command(INTERCOM_COMMAND, "", &ctx)
        .await
        .expect("the command dispatches")
        .expect("the command produces output");
    assert!(rendered.contains(PEER_NAME), "the picker lists the peer: {rendered}");
    assert!(
        sink.entries().is_empty(),
        "opening the picker sends nothing, so it appends nothing: {:?}",
        sink.entries()
    );

    peer.disconnect();
    if let Some(c) = state.client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}
