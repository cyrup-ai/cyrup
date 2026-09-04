//! The `intercom` TOOL's six actions, driven against a real broker child process.
//!
//! Drained from `crates/cyrup-intercom/src/tools/intercom.rs`'s `#[cfg(test)]` module, which is
//! where these nine were written and where they did not belong: every one of them spawns the real
//! `cyrup-intercom-broker` binary as a subprocess and talks to it over a Unix socket, which is the
//! definition of a seam test (docs/TEST-ARCHITECTURE.md §9.1). They passed in `src/` only because
//! some sibling integration target in the same package incidentally caused cargo to link the
//! broker bin into `target/<profile>/`; once those targets moved here, `cargo test -p
//! cyrup-intercom --lib` stopped producing the binary and all nine went red with
//! `spawn the real intercom broker subprocess: Os { code: 2, kind: NotFound }`. The binary is
//! resolved properly here, by `build.rs` → [`crate::support::bins::intercom_broker`].
//!
//! ONE mechanical rewrite, and it is the only difference from the originals: the `src/` copies
//! called the crate-private `IntercomTool::dispatch(IntercomParams { … })`, which an external crate
//! cannot name. They call the PUBLIC `Tool::execute(call_id, json, cancel, sink)` here instead —
//! the same code path one frame earlier (`execute` is nothing but
//! `serde_json::from_value::<IntercomParams>(params)` followed by `self.dispatch(parsed, &cancel)`,
//! `tools/intercom.rs:426-436`), so every assertion below is unchanged and each now additionally
//! proves its arguments survive the tool's own `camelCase` schema deserialization. No assertion was
//! relaxed, retimed or dropped in the move.
//!
//! The `spawn_broker`/`registration` helpers the originals carried are byte-identical to
//! [`super::common`]'s, so they are not duplicated here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate};
use cyrup_ext::{CannedResponses, RecordingServices};
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::identity::short_session_id;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{Message, MessageContent, SessionInfo, now_ms};

use super::common::{Broker, registration};

fn session(id: &str, cwd: &str) -> SessionInfo {
    SessionInfo {
        // ICOM-041: `runtimeFallbackAlias` (v0.10.1 types.ts:6-7) — these fixtures
        // register under a REAL name, not a synthesized unnamed-runtime alias.
        runtime_fallback_alias: None,
        id: id.to_string(),
        name: Some(id.to_string()),
        cwd: cwd.to_string(),
        model: "m".to_string(),
        pid: 1u32.into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        peer_uid: None,
        trusted_local: None,
        context_pct: None,
        context_tokens: None,
        context_window: None,
        tmux_pane: None,
        extra: Default::default(),
    }
}

fn ask_message(id: &str) -> Message {
    Message {
        id: id.to_string(),
        timestamp: now_ms().into(),
        reply_to: None,
        expects_reply: Some(true),
        content: MessageContent {
            text: "hi".to_string(),
            attachments: None,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn result_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            Content::Text { text, .. } => text.to_string(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("")
}

/// A discarding [`cyrup_core::ToolUpdateSink`]. The `intercom` tool emits no progress updates, so
/// this is the whole of what the `dispatch`-vs-`execute` rewrite costs.
fn sink() -> Box<dyn FnMut(ToolUpdate) + Send + 'static> {
    Box::new(|_| {})
}

/// `IntercomTool::dispatch(IntercomParams { … })` → the public `Tool::execute` with the same
/// arguments as the tool's own JSON schema spells them (`replyTo`, camelCase).
async fn run(
    tool: &IntercomTool,
    cancel: &CancelToken,
    params: serde_json::Value,
) -> Result<ToolResult, cyrup_core::ToolError> {
    tool.execute(ToolCallId::from("call-1"), params, cancel.clone(), sink())
        .await
}

fn fresh_state() -> Arc<SharedIntercomState> {
    Arc::new(SharedIntercomState::new(
        IntercomConfig::default(),
        600_000,
        PathBuf::from("/w"),
    ))
}

// Regression proof for the dossier item "`reply` tool action is missing pi's self-target guard"
// (`pi-intercom/index.ts:1685-1691`): against the PRE-FIX cyrup behavior this would resolve the
// self-addressed pending ask and forward it straight to `client.send`, and the assertion on the
// still-pending ask below would fail (the pre-fix code unconditionally left the ask untouched only
// because it never even tried to dismiss it — but the delivered send itself would succeed against
// the live broker, which is the actual bug this proves is now refused before ever reaching send).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_refuses_when_the_resolved_target_is_the_current_session() {
    let broker = Broker::start().await;
    let client = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("self"),
            Some("self-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let state = fresh_state();
    state.set_client(Some(client.clone()));

    // A (misrouted) pending inbound ask whose sender id is THIS session's own id.
    state.tracker.lock().unwrap().record_incoming_message(
        session("self-session", "/w"),
        ask_message("q1"),
        now_ms(),
    );

    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let err = run(
        &tool,
        &cancel,
        serde_json::json!({ "action": "reply", "message": "hello back" }),
    )
    .await
    .expect_err("must refuse a self-target reply");
    assert!(
        err.message.contains("Cannot message the current session"),
        "got: {}",
        err.message
    );

    // The ask must still be pending — the guard fires before `markReplied`/`dismissPendingAsk`.
    let pending = state.tracker.lock().unwrap().list_pending(now_ms());
    assert_eq!(
        pending.len(),
        1,
        "the self-targeted ask must remain pending, not sent or dismissed"
    );

    client.disconnect();
}

// Regression proof for "`send` marks the ask replied before/regardless of delivery success"
// (`pi-intercom/index.ts:1537-1557`): against the PRE-FIX behavior, `mark_replied` ran
// unconditionally BEFORE the send even reached the broker, so the pending ask being replied-to
// would already be gone (list_pending empty) even though the send itself failed. This asserts the
// ask survives an undelivered send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_does_not_mark_the_ask_replied_when_delivery_fails() {
    let broker = Broker::start().await;
    let client = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("self"),
            Some("self-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let state = fresh_state();
    state.set_client(Some(client.clone()));

    // A real pending inbound ask this "send" call claims (via `replyTo`) to be answering.
    state.tracker.lock().unwrap().record_incoming_message(
        session("original-asker", "/w"),
        ask_message("q1"),
        now_ms(),
    );

    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let err = run(
        &tool,
        &cancel,
        serde_json::json!({
            "action": "send",
            "to": "no-such-session",
            "message": "this will not deliver",
            "replyTo": "q1",
        }),
    )
    .await
    .expect_err("delivery to an unknown session fails");
    assert!(
        err.message.contains("Session not found"),
        "got: {}",
        err.message
    );

    // The original inbound ask must still be pending — a failed send must not have marked it
    // replied, so the agent can still retry answering it.
    let pending = state.tracker.lock().unwrap().list_pending(now_ms());
    assert_eq!(
        pending.len(),
        1,
        "a failed send must leave the original ask pending for a retry"
    );

    client.disconnect();
}

// Regression proof for "`intercom{list}` drops the self-missing guard and the Current/Other
// section split" (`pi-intercom/index.ts:1478-1507`): against the PRE-FIX behavior this rendered a
// single flat, unheaded list of every session (including self) with no section split — the
// asserted headers below would be entirely absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn list_splits_current_and_other_sessions_with_headed_sections() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let other = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("other"),
            Some("other-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let state = fresh_state();
    state.set_client(Some(me.clone()));
    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = run(&tool, &cancel, serde_json::json!({ "action": "list" }))
        .await
        .expect("list succeeds");
    let text = result_text(&result);

    assert!(
        text.contains("**Current session:**"),
        "missing current-session header: {text}"
    );
    assert!(
        text.contains("**Other sessions:**"),
        "missing other-sessions header: {text}"
    );
    let current_idx = text.find("**Current session:**").unwrap();
    let other_idx = text.find("**Other sessions:**").unwrap();
    assert!(
        current_idx < other_idx,
        "current section must come first: {text}"
    );
    // Self must be tagged `[self]` and NOT appear again under "Other sessions" (rows render the
    // `shortSessionId` — `identity::short_session_id` — not the raw id, so match on that).
    let current_section = &text[current_idx..other_idx];
    let other_section = &text[other_idx..];
    let self_short_id = short_session_id("me-session");
    let other_short_id = short_session_id("other-session");
    // `[self, idle]`, not a bare `[self]`. `format_session_list_row` pushes `self` FIRST and then
    // appends the session's status; the CURRENT session's status comes from `current_status()`
    // (pi `v0.10.1 index.ts:676-680`), which is never empty — it floors at `idle` when no tool is
    // active and the agent is not running, which is exactly this fixture's state. A bare `[self]`
    // was only ever reachable before that status reached the row, so this assertion could not pass
    // against current behaviour. (The OTHER row still shows a bare `[same cwd]`: its status comes
    // from its own registration, which `common::registration` leaves `None`.)
    assert!(
        current_section.contains("[self, idle]"),
        "self row must be tagged `self` first and carry its lifecycle status: {text}"
    );
    assert!(
        current_section.contains(&self_short_id),
        "self row missing own id: {text}"
    );
    assert!(
        !other_section.contains(&self_short_id) && !other_section.contains("[self"),
        "self leaked into other sessions: {text}"
    );
    assert!(
        other_section.contains(&other_short_id),
        "the other session must be listed: {text}"
    );

    me.disconnect();
    other.disconnect();
}

// Regression proof for "confirmSend config is parsed but never enforced" (`index.ts:1524-1536`):
// against the PRE-FIX behavior `confirm_send`/`has_ui` were never read at all, so a declined
// confirmation would still deliver the message and the assertions below (cancellation text, no
// delivery, no audit entry) would fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_honors_a_declined_confirm_send_prompt() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let target = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("target"),
            Some("target-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let mut target_events = target.subscribe();

    let config = IntercomConfig {
        confirm_send: true,
        ..IntercomConfig::default()
    };
    let state = Arc::new(SharedIntercomState::new(
        config,
        600_000,
        PathBuf::from("/w"),
    ));
    state.set_client(Some(me.clone()));
    state.set_has_ui(true);
    let services = Arc::new(RecordingServices::new(CannedResponses {
        confirm: false,
        ..Default::default()
    }));
    state.set_host_services(services.clone());

    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = run(
        &tool,
        &cancel,
        serde_json::json!({
            "action": "send",
            "to": "target-session",
            "message": "please don't actually send",
        }),
    )
    .await
    .expect("a declined confirm is not an error");
    assert_eq!(result_text(&result), "Message cancelled by user");
    assert!(
        services.entries_persisted().is_empty(),
        "a cancelled send must not append an audit entry"
    );

    // The target never actually received the MESSAGE.
    //
    // Not "received nothing": the broker broadcasts a `PresenceUpdate` to every peer when a session
    // joins the roster, and `me` connects after `target` subscribes, so one reliably lands inside
    // any wait window here. This assertion used to be `recv()` timing out, which conflated that
    // routine presence traffic with delivery and went red the moment presence arrived promptly
    // enough. Drain the window and assert on the event KIND — a declined confirm must produce no
    // `MessageReceived`, while presence is expected and says nothing about delivery.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let mut delivered: Vec<InboundEvent> = Vec::new();
    let mut saw_presence = false;
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, target_events.recv()).await {
        match event {
            InboundEvent::PresenceUpdate(_) => saw_presence = true,
            other => delivered.push(other),
        }
    }
    assert!(
        delivered.is_empty(),
        "the declined send must never reach the broker/target; got: {delivered:?}"
    );
    // Presence-before-absence: if NOTHING at all arrived, the drain above proves nothing about
    // delivery — the channel could simply be dead. The presence broadcast is the liveness witness.
    assert!(
        saw_presence,
        "the target's event channel must be live (a peer joining broadcasts presence), otherwise \
         the empty-delivery assertion above is vacuous"
    );

    me.disconnect();
    target.disconnect();
}

// Regression proof for "intercom_sent / intercom_received audit-log entries are never recorded"
// (`index.ts:1549-1554`): against the PRE-FIX behavior `append_entry` was never called anywhere in
// this file, so `entries_persisted()` below would be empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_send_appends_an_intercom_sent_audit_entry() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let target = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("target"),
            Some("target-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let state = fresh_state();
    state.set_client(Some(me.clone()));
    let services = Arc::new(RecordingServices::new(CannedResponses::default()));
    state.set_host_services(services.clone());

    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = run(
        &tool,
        &cancel,
        serde_json::json!({ "action": "send", "to": "target-session", "message": "hello target" }),
    )
    .await
    .expect("send delivers");
    assert_eq!(result_text(&result), "Message sent to target-session");

    let entries = services.entries_persisted();
    assert_eq!(
        entries.len(),
        1,
        "exactly one intercom_sent entry: {entries:?}"
    );
    assert_eq!(entries[0].0, "intercom_sent");
    assert_eq!(
        entries[0].1.get("to").and_then(|v| v.as_str()),
        Some("target-session")
    );
    assert_eq!(
        entries[0]
            .1
            .get("message")
            .and_then(|m| m.get("text"))
            .and_then(|v| v.as_str()),
        Some("hello target")
    );

    me.disconnect();
    target.disconnect();
}

// ---------------------------------------------------------------------------------------
// Regression proofs for "three `intercom` tool result texts diverge from upstream".
// ---------------------------------------------------------------------------------------

/// `index.ts:1529,1571`: pi resolves the target for DELIVERY (`sendTo`) but reports the
/// CALLER-SUPPLIED `to` back to the model (`Message sent to ${to}`). Against the PRE-FIX cyrup
/// behavior — `format!("Message sent to {target}.")` with `target` from `resolve_or_err` — a
/// send addressed to the peer's NAME echoed back the raw session id it resolved to, so the
/// agent lost the human-readable handle it had just used.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_reports_the_caller_supplied_target_not_the_resolved_session_id() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    // Registered under the NAME "reviewer" but the SESSION ID "peer-session": the two differ,
    // so the reported target proves which one the tool echoes.
    let peer = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("reviewer"),
            Some("peer-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let state = fresh_state();
    state.set_client(Some(me.clone()));
    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = run(
        &tool,
        &cancel,
        serde_json::json!({ "action": "send", "to": "reviewer", "message": "please review" }),
    )
    .await
    .expect("send delivers");
    assert_eq!(
        result_text(&result),
        "Message sent to reviewer",
        "pi reports the caller-supplied `to`, not the resolved id, and with NO trailing period"
    );

    me.disconnect();
    peer.disconnect();
}

/// `index.ts:1669`: `**Reply from ${to}:**\n${replyText}`. Against the PRE-FIX cyrup behavior
/// (`Ok(text_result(reply))`) the tool returned the bare reply body, so a transcript that had
/// asked more than one peer carried no indication of which peer answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_prefixes_the_reply_with_the_reply_from_header() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let peer = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("reviewer"),
            Some("peer-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let state = fresh_state();
    state.set_client(Some(me.clone()));
    // The REAL inbound loop is what resolves the outbound single-slot waiter (`inbound.rs:327`).
    cyrup_intercom::inbound::spawn_inbound_loop(state.clone(), me.clone());

    // A scripted peer that answers the first ask it receives.
    let mut peer_events = peer.subscribe();
    let peer_writer = peer.clone();
    tokio::spawn(async move {
        while let Ok(event) = peer_events.recv().await {
            if let InboundEvent::Message { message, from } = event
                && message.expects_reply == Some(true)
            {
                let _ = peer_writer
                    .send(
                        &from.id,
                        SendOptions {
                            text: "ship it".to_string(),
                            attachments: None,
                            reply_to: Some(message.id.clone()),
                            expects_reply: None,
                            message_id: None,
                            ..Default::default()
                        },
                    )
                    .await;
                return;
            }
        }
    });

    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run(
            &tool,
            &cancel,
            serde_json::json!({ "action": "ask", "to": "reviewer", "message": "ok to ship?" }),
        ),
    )
    .await
    .expect("the ask resolves within the timeout")
    .expect("ask succeeds");
    assert_eq!(
        result_text(&result),
        "**Reply from reviewer:**\nship it",
        "pi headers the reply with the peer it came from"
    );

    me.disconnect();
    peer.disconnect();
}

/// `index.ts:1726`: `Reply sent to ${target.from.name || target.from.id}` — the sender's NAME is
/// preferred over its id. Against the PRE-FIX cyrup behavior (`target.from.id`) a reply to a
/// named peer reported the raw session id back instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_reports_the_sender_name_rather_than_the_raw_session_id() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let peer = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("reviewer"),
            Some("peer-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let state = fresh_state();
    state.set_client(Some(me.clone()));

    // A REAL inbound ask from the peer, so the broker holds the ask edge a reply must match
    // (`broker.ts:434-441`). Record it exactly as `spawn_inbound_loop` step (2) does
    // (`inbound.rs:332-336`); the sender's NAME ("reviewer") and SESSION ID ("peer-session")
    // differ, which is what makes the reported target diagnostic.
    let mut my_events = me.subscribe();
    peer.send(
        "me-session",
        SendOptions {
            text: "ok to ship?".to_string(),
            attachments: None,
            reply_to: None,
            expects_reply: Some(true),
            message_id: Some("q1".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("the ask is delivered");
    // DRAIN to the ask rather than assuming it is the next frame. The broker legitimately
    // interleaves presence events — under CPU contention this saw
    // `SessionJoined(SessionInfo { id: "peer-session", … })` first and failed 1 run in 6, while
    // passing 9 in 9 idle. Frame ORDER between presence and messages was never promised, so
    // asserting on "the next frame" tested the scheduler, not the code.
    let (from, message) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = my_events.recv().await.expect("the event channel delivers");
            if let InboundEvent::Message { from, message } = event {
                return (from, *message);
            }
        }
    })
    .await
    .expect("the inbound ask arrives");
    assert_eq!(from.name.as_deref(), Some("reviewer"));
    state
        .tracker
        .lock()
        .unwrap()
        .record_incoming_message(from, message, now_ms());

    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = run(
        &tool,
        &cancel,
        serde_json::json!({ "action": "reply", "message": "looks good" }),
    )
    .await
    .expect("reply delivers");
    assert_eq!(
        result_text(&result),
        "Reply sent to reviewer",
        "pi prefers the sender's name over its session id, and appends no period"
    );

    me.disconnect();
    peer.disconnect();
}

/// `index.ts:1765`: a four-line `**Intercom Status:**` markdown block. Against the PRE-FIX
/// cyrup behavior the tool emitted a single pipe-delimited line
/// (`intercom: connected | session id: … | active sessions: …`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_renders_pi_four_line_intercom_status_block() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let peer = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("reviewer"),
            Some("peer-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let state = fresh_state();
    state.set_client(Some(me.clone()));
    let tool = IntercomTool::new(state.clone());
    let cancel = CancelToken::new();
    let result = run(&tool, &cancel, serde_json::json!({ "action": "status" }))
        .await
        .expect("status succeeds");
    assert_eq!(
        result_text(&result),
        "**Intercom Status:**\nConnected: Yes\nSession ID: me-session\nActive sessions: 2"
    );

    me.disconnect();
    peer.disconnect();
}
