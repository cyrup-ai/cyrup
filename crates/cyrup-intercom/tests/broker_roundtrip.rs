//! FULLY-WIRED PROOF (the port doc §5 Phase 2/§11.1): stand up the **real** broker as a genuine
//! child OS process (the `cyrup-intercom-broker` fixture binary) and drive a child→broker→supervisor
//! ask/answer round trip through **two real `IntercomClient`s over the real Unix socket** — not a
//! mock. This is the child↔supervisor path the `contact_supervisor` tool + the seam channels ride
//! on (the port doc §7.4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_ext_subagents::background::RunId;
use cyrup_ext_subagents::tui::intercom::{DeliveryChannel, IntercomPayload, SubagentResultStatus};
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::seams::IntercomDeliveryChannel;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::transport::client::{IntercomClient, InboundEvent, SendOptions};
use cyrup_intercom::transport::protocol::{now_ms, SessionRegistration};
use cyrup_intercom::transport::spawn::wait_for_broker;

fn registration(name: &str) -> SessionRegistration {
    SessionRegistration {
        name: Some(name.to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        extra: Default::default(),
    }
}

/// Wait for the next `Message` inbound event on `rx`, with a bound.
async fn next_message(
    rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>,
) -> (cyrup_intercom::transport::protocol::SessionInfo, cyrup_intercom::transport::protocol::Message) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { from, message })) => return (from, *message),
            Ok(Ok(_other)) => continue,
            Ok(Err(e)) => panic!("event channel error: {e}"),
            Err(_) => panic!("timed out waiting for an inbound message"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_to_broker_to_supervisor_round_trip_over_the_real_socket() {
    let broker_bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = agent_dir.path().join("intercom");
    let socket_path = intercom_dir.join("broker.sock");

    // Launch the REAL broker as a genuine child process, pointing it at our temp agent dir.
    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess");

    // Wait for the broker's health probe to answer over the real socket.
    wait_for_broker(&socket_path, Duration::from_secs(5))
        .await
        .expect("broker becomes health-connectable");

    // Supervisor connects with a STABLE session id (so the child can address it deterministically).
    let supervisor = IntercomClient::connect(
        &socket_path,
        registration("supervisor"),
        Some("supervisor-session".to_string()),
    )
    .await
    .expect("supervisor registers");
    assert_eq!(supervisor.session_id().as_deref(), Some("supervisor-session"));
    let mut supervisor_events = supervisor.subscribe();

    // Child connects.
    let child = IntercomClient::connect(&socket_path, registration("subagent-chat-1"), None)
        .await
        .expect("child registers");
    let child_id = child.session_id().expect("child has a session id");

    // 1) Child asks the supervisor (expects_reply → records an ask edge on the broker).
    let question_id = uuid::Uuid::new_v4().to_string();
    let send_result = child
        .send(
            "supervisor-session",
            SendOptions {
                text: "Which database should I use?".to_string(),
                expects_reply: Some(true),
                message_id: Some(question_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("child send routes through the broker");
    assert!(send_result.delivered, "the broker delivered the ask to the supervisor: {send_result:?}");

    // 2) Supervisor RECEIVES the ask, child→broker→supervisor.
    let (from, ask) = next_message(&mut supervisor_events).await;
    assert_eq!(from.id, child_id, "the ask came from the child session");
    assert_eq!(ask.id, question_id);
    assert_eq!(ask.expects_reply, Some(true));
    assert_eq!(ask.content.text, "Which database should I use?");

    // 3) Supervisor replies (reply_to = questionId), routed back over the SAME broker.
    let mut child_events = child.subscribe();
    let reply_result = supervisor
        .send(
            &child_id,
            SendOptions {
                text: "Use Postgres.".to_string(),
                reply_to: Some(question_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("supervisor reply routes through the broker");
    assert!(reply_result.delivered, "the reply was delivered to the still-connected child");

    // 4) The child RECEIVES the answer — the full round trip closes.
    let (reply_from, reply) = next_message(&mut child_events).await;
    assert_eq!(reply_from.id, "supervisor-session");
    assert_eq!(reply.reply_to.as_deref(), Some(question_id.as_str()));
    assert_eq!(reply.content.text, "Use Postgres.");

    // Also prove list discovery sees both sessions through the real broker.
    let sessions = supervisor.list_sessions().await.expect("list sessions");
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"supervisor-session"));
    assert!(ids.contains(&child_id.as_str()));

    // Clean teardown.
    child.disconnect();
    supervisor.disconnect();
    let _ = broker.kill().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_channel_relays_a_grouped_result_to_the_supervisor_over_the_broker() {
    // Exercises the REAL `IntercomDeliveryChannel` (the R-SA-123/124/125 seam impl) end-to-end over
    // the broker: an orchestrator that is itself a child relays its allowlisted grouped result to its
    // supervisor. Proves the delivery seam's broker-relay path (not just the degraded no-supervisor
    // path unit-tested in-crate).
    let broker_bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");

    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn broker");
    wait_for_broker(&socket_path, Duration::from_secs(5)).await.expect("broker up");

    // The supervisor (the delivery target).
    let supervisor =
        IntercomClient::connect(&socket_path, registration("supervisor"), Some("supervisor-session".to_string()))
            .await
            .expect("supervisor registers");
    let mut supervisor_events = supervisor.subscribe();

    // The orchestrator's own client + shared state, with its supervisor target set.
    let orchestrator = Arc::new(
        IntercomClient::connect(&socket_path, registration("orchestrator"), Some("orch-session".to_string()))
            .await
            .expect("orchestrator registers"),
    );
    let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
    state.set_client(Some(orchestrator.clone()));
    let delivery = IntercomDeliveryChannel::new(state, Some("supervisor-session".to_string()));

    let payload = IntercomPayload {
        run_id: RunId::from_token("run00000000000042"),
        agent: "researcher".to_string(),
        success: true,
        outputs: vec!["the grouped result output".to_string()],
        total_tokens: 4242,
        status: SubagentResultStatus::Completed,
        summary: "1 completed".to_string(),
        child_statuses: vec![SubagentResultStatus::Completed],
    };
    let delivered = delivery.send(payload).await.expect("delivery returns a verdict");
    assert!(delivered, "the delivery channel relayed the result to the supervisor over the broker");

    // The supervisor receives the allowlisted relay body.
    let (from, message) = next_message(&mut supervisor_events).await;
    assert_eq!(from.id, "orch-session");
    assert!(message.content.text.contains("run00000000000042"));
    assert!(message.content.text.contains("researcher"));
    assert!(message.content.text.contains("Total tokens: 4242"));
    assert!(message.content.text.contains("the grouped result output"));

    supervisor.disconnect();
    orchestrator.disconnect();
    let _ = broker.kill().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_refuses_a_mutual_ask() {
    let broker_bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");

    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn broker");
    wait_for_broker(&socket_path, Duration::from_secs(5)).await.expect("broker up");

    let a = IntercomClient::connect(&socket_path, registration("a"), Some("sess-a".to_string()))
        .await
        .expect("a registers");
    let b = IntercomClient::connect(&socket_path, registration("b"), Some("sess-b".to_string()))
        .await
        .expect("b registers");

    // a asks b (records edge a→b).
    let q1 = uuid::Uuid::new_v4().to_string();
    let r1 = a
        .send("sess-b", SendOptions { text: "q".to_string(), expects_reply: Some(true), message_id: Some(q1), ..Default::default() })
        .await
        .expect("a→b ask");
    assert!(r1.delivered);

    // b tries to ask a back while a's ask is open → mutual-ask refusal (broker.ts:460-469).
    let q2 = uuid::Uuid::new_v4().to_string();
    let r2 = b
        .send("sess-a", SendOptions { text: "q2".to_string(), expects_reply: Some(true), message_id: Some(q2), ..Default::default() })
        .await
        .expect("b→a ask returns a broker verdict");
    assert!(!r2.delivered, "a mutual ask must be refused");
    assert!(
        r2.reason.as_deref().unwrap_or_default().contains("Mutual ask refused"),
        "reason: {:?}",
        r2.reason
    );

    a.disconnect();
    b.disconnect();
    let _ = broker.kill().await;
}
