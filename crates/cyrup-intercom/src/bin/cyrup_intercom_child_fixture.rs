//! `cyrup-intercom-child-fixture` — a REAL subagent-child stand-in that ACTIVATES the intercom
//! child bridge from nothing but its process environment, gated behind the `test-fixtures` Cargo
//! feature and never built for / shipped inside the real `cyrup` binary.
//!
//! # Why this binary exists (the claim-6 production-activation proof)
//!
//! The scripted-NDJSON `cyrup-subagent-fixture` (in `cyrup-ext-subagents`) proves the production
//! spawn overlay DELIVERS the six `CYRUP_SUBAGENT_*` child-bridge env vars into a real child's
//! environment — but that fixture is a pure NDJSON emitter that never constructs an
//! `IntercomExtension`, so it can prove neither that a real child READS that env via the production
//! `read_child_orchestrator_metadata()` gate NOR that it registers on the broker and completes a
//! `contact_supervisor` round trip. `cyrup-ext-subagents` cannot depend on `cyrup-intercom` (the
//! dependency edge runs the other way), so a child that actually speaks intercom must live HERE.
//!
//! This binary does exactly what a real subagent child's `IntercomExtension` does on `SessionStart`,
//! minus the full session machinery:
//!
//! 1. Reads its child-orchestrator metadata via the SAME production gate the extension uses
//!    ([`cyrup_intercom::identity::read_child_orchestrator_metadata`]). If the production spawn path
//!    did NOT set the bridge env, this returns `None` and the process exits non-zero (marker
//!    `BRIDGE_INERT`) — so a test that drives the real spawn path fails loudly if the seam is inert.
//! 2. Connects to the real broker (discovered from the inherited `CYRUP_CODING_AGENT_DIR`) and
//!    registers under its OWN deterministic presence label — `metadata.session_name`, the string the
//!    production overlay wrote as `CYRUP_SUBAGENT_INTERCOM_SESSION_NAME` (=
//!    `resolve_subagent_intercom_target(run_id, agent, index)`) — exactly what the extension's
//!    `build_registration` registers a child under.
//! 3. Sends a `contact_supervisor`-shaped ask to its supervisor
//!    ([`cyrup_intercom::identity::preferred_supervisor_target`], the `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET`
//!    the overlay wrote), waits for the supervisor's reply over the broker, then emits a terminal
//!    `message_end` NDJSON line carrying that reply so the driving `run_sync` returns it as final
//!    output (closing the round trip observably at BOTH ends).
//!
//! Every non-zero exit path also emits a `message_end` marker line so the driving `run_sync` tees a
//! diagnosable reason into its attempt log rather than the test seeing a bare non-zero exit.

use std::io::Write;
use std::time::Duration;

use cyrup_intercom::identity::{preferred_supervisor_target, read_child_orchestrator_metadata};
use cyrup_intercom::paths::{agent_dir_path, broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};

/// The child-bridge env gate returned `None`: the production spawn overlay did not set the six
/// `CYRUP_SUBAGENT_*` vars, so the seam is inert.
const EXIT_BRIDGE_INERT: i32 = 7;
/// Could not connect to the broker at the inherited agent dir.
const EXIT_CONNECT_FAILED: i32 = 8;
/// The ask was not delivered to the supervisor (no registered receiver at the target).
const EXIT_ASK_NOT_DELIVERED: i32 = 9;
/// No reply arrived from the supervisor within the bound.
const EXIT_NO_REPLY: i32 = 10;

#[tokio::main]
async fn main() {
    let code = run().await;
    std::process::exit(code);
}

async fn run() -> i32 {
    // (1) The PRODUCTION child-bridge gate — reads only the process environment. `None` here means a
    // real spawned child would NOT register `contact_supervisor`: the seam is production-inert.
    let Some(meta) = read_child_orchestrator_metadata() else {
        emit_message_end(
            "BRIDGE_INERT: read_child_orchestrator_metadata() returned None — the production spawn \
             overlay did not set the child-bridge env vars",
        );
        return EXIT_BRIDGE_INERT;
    };

    // The child's OWN deterministic presence label (the parent steers it here) and its supervisor's
    // addressable target — both derived from the production-set env, never hand-injected by the test.
    let own_label = meta.session_name.clone().unwrap_or_default();
    let supervisor_target = preferred_supervisor_target(&meta);

    let agent_dir = agent_dir_path();
    let socket = broker_socket_path(&intercom_dir_path(&agent_dir));

    let registration = SessionRegistration {
        name: Some(own_label),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        model: "cyrup-intercom-child-fixture".to_string(),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        extra: Default::default(),
    };

    // (2) Register as a genuine broker participant under the deterministic child label.
    let client = match IntercomClient::connect(&socket, registration, None).await {
        Ok(client) => client,
        Err(err) => {
            emit_message_end(&format!("BRIDGE_CONNECT_FAILED: {err}"));
            return EXIT_CONNECT_FAILED;
        }
    };
    let mut events = client.subscribe();

    // (3) Send the contact_supervisor-shaped ask to the supervisor and await its reply.
    let question_id = uuid::Uuid::new_v4().to_string();
    let ask_text = format!(
        "CHILD_ASK::run={}::agent={}::index={}::which database?",
        meta.run_id, meta.agent, meta.index
    );
    match client
        .send(
            &supervisor_target,
            SendOptions {
                text: ask_text,
                expects_reply: Some(true),
                message_id: Some(question_id.clone()),
                ..Default::default()
            },
        )
        .await
    {
        Ok(result) if result.delivered => {}
        Ok(result) => {
            emit_message_end(&format!("BRIDGE_ASK_NOT_DELIVERED: {:?}", result.reason));
            return EXIT_ASK_NOT_DELIVERED;
        }
        Err(err) => {
            emit_message_end(&format!("BRIDGE_ASK_ERROR: {err}"));
            return EXIT_ASK_NOT_DELIVERED;
        }
    }

    let answer = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match events.recv().await {
                Ok(InboundEvent::Message { message, .. })
                    if message.reply_to.as_deref() == Some(question_id.as_str()) =>
                {
                    return Some(message.content.text);
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    })
    .await;

    client.disconnect();

    match answer {
        Ok(Some(text)) => {
            // The round trip closed: echo the supervisor's answer as this turn's final output so the
            // driving `run_sync` returns it (proving the reply reached the real child).
            emit_message_end(&format!("SUPERVISOR_REPLY::{text}"));
            0
        }
        _ => {
            emit_message_end("BRIDGE_NO_REPLY: the supervisor's reply never reached the child");
            EXIT_NO_REPLY
        }
    }
}

/// Emit one terminal `message_end` NDJSON line (the wire shape `run_sync` parses as this turn's final
/// assistant text), then flush. Mirrors the `message_end` shape the `cyrup-subagent-fixture`
/// integration tests use.
fn emit_message_end(text: &str) {
    let line = serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
