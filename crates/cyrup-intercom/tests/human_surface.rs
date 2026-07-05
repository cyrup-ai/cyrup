//! FULLY-WIRED PROOF for the P4 HUMAN surfaces (the port doc §4.2/§4.1 Route B, §5 Phase 4). Stands
//! up the **real** broker as a genuine child OS process and drives the two human-facing legs over the
//! real Unix socket with a **scripted `HostServices` sink** late-bound exactly as the builder binds it
//! (P-1 Route B — `set_host_services` before use):
//!
//!   LEG A (inbound surface) — a child's ask arrives at the orchestrator over the broker; the REAL
//!     production inbound loop ([`cyrup_intercom::inbound::spawn_inbound_loop`]) records it and
//!     SURFACES it to the human via `HostServices::append_entry("intercom_message", …)`. The scripted
//!     sink records the surfaced entry; we assert its `content` is pi's `📨 From …` body.
//!
//!   LEG B (outbound ask ← human reply) — the child issues its ask through the REAL single-slot
//!     outbound waiter ([`SharedIntercomState::ask_and_wait`]) and BLOCKS. The orchestrator's REAL
//!     [`IntercomClarifyChannel::ask`] correlates it, surfaces the prompt to the human via
//!     `HostServices::input` (the scripted sink answers "Use Postgres."), and routes that answer back
//!     to the still-alive child over the broker. The child's blocking ask unblocks WITH the human's
//!     answer — the whole child→broker→supervisor→human→broker→child round trip.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::CancelToken;
use cyrup_ext::{DialogOptions, HostServices};
use cyrup_ext_subagents::background::RunId;
use cyrup_ext_subagents::tui::intercom::{ClarifyChannel, ClarifyRequest};
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::identity::{
    ChildMessageKind, ChildOrchestratorMetadata, format_child_orchestrator_message,
};
use cyrup_intercom::inbound::spawn_inbound_loop;
use cyrup_intercom::seams::IntercomClarifyChannel;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};
use cyrup_intercom::transport::spawn::wait_for_broker;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// A scripted `HostServices` sink: records every `append_entry` (and signals it on a channel) and
/// answers `input` with a canned reply. The in-crate analog of the live TUI/RPC backend the session
/// injects — proves the human surface reaches a real `HostServices`, not a stub.
struct ScriptedSink {
    appended: Mutex<Vec<(String, Value)>>,
    append_tx: UnboundedSender<(String, Value)>,
    input_answer: Option<String>,
    input_prompts: Mutex<Vec<String>>,
}

impl ScriptedSink {
    fn new(input_answer: &str) -> (Arc<Self>, UnboundedReceiver<(String, Value)>) {
        let (append_tx, append_rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = Arc::new(Self {
            appended: Mutex::new(Vec::new()),
            append_tx,
            input_answer: Some(input_answer.to_string()),
            input_prompts: Mutex::new(Vec::new()),
        });
        (sink, append_rx)
    }
}

impl HostServices for ScriptedSink {
    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        let mut g = self.appended.lock().unwrap();
        let id = format!("entry-{}", g.len() + 1);
        g.push((custom_type.to_string(), data.clone()));
        let _ = self.append_tx.send((custom_type.to_string(), data.clone()));
        Ok(id)
    }
    fn input(&self, prompt: &str, _placeholder: Option<&str>, _opts: &DialogOptions) -> Option<String> {
        self.input_prompts.lock().unwrap().push(prompt.to_string());
        self.input_answer.clone()
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

fn child_meta() -> ChildOrchestratorMetadata {
    ChildOrchestratorMetadata {
        orchestrator_target: "orch-session".to_string(),
        orchestrator_session_id: Some("orch-session".to_string()),
        run_id: "run-xyz".to_string(),
        agent: "researcher".to_string(),
        index: "0".to_string(),
        session_name: Some("subagent-chat-1".to_string()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_message_surfaces_and_outbound_ask_receives_the_human_reply() {
    let broker_bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");

    // The REAL broker as a genuine child process.
    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess");
    wait_for_broker(&socket_path, Duration::from_secs(5)).await.expect("broker becomes health-connectable");

    // ---- The orchestrator (supervisor) session: client + state + scripted human sink + inbound loop.
    let orchestrator = Arc::new(
        IntercomClient::connect(&socket_path, registration("orchestrator"), Some("orch-session".to_string()))
            .await
            .expect("orchestrator registers"),
    );
    let orch_state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
    orch_state.set_client(Some(orchestrator.clone()));
    // The supervisor is the interactive human-facing session (pi `hasUI` = true): the inbound
    // delivery policy drives/steers a turn over the child ask rather than sending it a busy
    // auto-reply, so the child's outbound ask stays parked for the REAL human answer (LEG B).
    orch_state.set_has_ui(true);

    // P-1 Route B: late-bind the scripted HostServices exactly as the builder's
    // `load_native_with_services` → `set_host_services` does.
    let (sink, mut append_rx) = ScriptedSink::new("Use Postgres.");
    orch_state.set_host_services(sink.clone());

    // The REAL production inbound loop (records + surfaces every inbound message).
    spawn_inbound_loop(orch_state.clone(), orchestrator.clone());

    // ---- The child (subagent) session: client + its OWN state + inbound loop (drives try_deliver).
    let child = Arc::new(
        IntercomClient::connect(&socket_path, registration("subagent-chat-1"), None)
            .await
            .expect("child registers"),
    );
    let child_state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
    child_state.set_client(Some(child.clone()));
    spawn_inbound_loop(child_state.clone(), child.clone());

    // LEG B setup: the child issues its blocking ask through the REAL single-slot outbound waiter.
    // The body carries `Run: run-xyz` (formatChildOrchestratorMessage) so the orchestrator's
    // ClarifyChannel can correlate it.
    let question_id = "question-abc".to_string();
    let ask_body = format_child_orchestrator_message(ChildMessageKind::Ask, &child_meta(), "Which database should I use?");
    let child_task = {
        let child_state = child_state.clone();
        let child = child.clone();
        let question_id = question_id.clone();
        tokio::spawn(async move {
            let cancel = CancelToken::new();
            child_state
                .ask_and_wait(&child, "orch-session", question_id, ask_body, None, &cancel)
                .await
        })
    };

    // LEG A: the orchestrator's inbound loop surfaces the child's ask to the human via append_entry.
    let (custom_type, data) = tokio::time::timeout(Duration::from_secs(5), append_rx.recv())
        .await
        .expect("the inbound surface fired within the timeout")
        .expect("the append channel delivered the surfaced entry");
    assert_eq!(custom_type, "intercom_message", "surfaced as the intercom_message custom entry");
    let content = data["content"].as_str().unwrap_or_default();
    assert!(content.contains("📨 From"), "surfaced content is pi's inbound-message body: {content:?}");
    assert!(content.contains("Which database should I use?"), "content carries the child's ask: {content:?}");
    // The pre-rendered card (the §4.3 degrade of the inline renderer) is present too.
    assert!(data["card"].as_array().map(|a| !a.is_empty()).unwrap_or(false), "the rendered card is embedded");

    // LEG B: the orchestrator's REAL ClarifyChannel surfaces the prompt to the human (scripted
    // sink → "Use Postgres.") and routes that answer back to the still-alive child over the broker.
    let clarify = IntercomClarifyChannel::new(orch_state.clone());
    let request = ClarifyRequest {
        run_id: RunId::from_token("run-xyz"),
        step_index: Some(0),
        prompt: "The subagent needs a database decision.".to_string(),
    };
    let clarify_answer = tokio::time::timeout(Duration::from_secs(5), clarify.ask(request))
        .await
        .expect("clarify resolved within the timeout")
        .expect("clarify returns the human answer");
    assert_eq!(clarify_answer, "Use Postgres.", "the ClarifyChannel returned the human's answer");

    // The human prompt actually reached the scripted input sink.
    assert!(
        sink.input_prompts.lock().unwrap().iter().any(|p| p.contains("database decision")),
        "the clarify prompt reached HostServices::input",
    );

    // The child's SINGLE OUTBOUND ASK unblocked WITH the human's answer, over the real broker path.
    let child_reply = tokio::time::timeout(Duration::from_secs(5), child_task)
        .await
        .expect("the child ask resolved within the timeout")
        .expect("the child task joined")
        .expect("the child ask returned the reply");
    assert_eq!(child_reply, "Use Postgres.", "the human's answer reached the child through the broker");

    // Clean teardown.
    child.disconnect();
    orchestrator.disconnect();
    let _ = broker.kill().await;
}
