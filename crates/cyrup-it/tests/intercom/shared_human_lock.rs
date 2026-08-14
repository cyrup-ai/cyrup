//! FULLY-WIRED PROOF for C3 (reconciliation §1 / §4 step 6): the ONE host-owned, session-scoped
//! `HumanInteractionLock` genuinely serializes the two companions' human prompts, so a permission
//! approval and an intercom clarify can NEVER prompt the same human at once.
//!
//! This is NOT a unit test of the lock in isolation. It drives BOTH real companion human-contact code
//! paths against ONE shared `HostServices` backend (the same shape the builder late-binds one
//! `LiveHostServices` Arc into every native via `set_host_services`):
//!
//!   - the REAL permission gate — `PermissionSystemExtension::on_event(ToolCall{bash})` → `resolve_ask`
//!     → the `LocalAskChannel` dialog — which now acquires the shared lock and HOLDS it across its
//!     blocking `select` dialog (the sink blocks `select` until the test releases it, modeling a human
//!     deliberating);
//!   - the REAL intercom clarify — `IntercomClarifyChannel::ask` over a genuine broker child process —
//!     which now acquires the SAME shared lock before surfacing its `input` prompt.
//!
//! With the permission ask holding the lock, the concurrent intercom clarify WAITS (its `input` never
//! fires — no double-prompt). Only after the permission dialog returns and the gate drops its guard
//! does the clarify acquire the lock, prompt the human, and route the answer back to the still-alive
//! child over the broker. The two human prompts are asserted to occupy DISJOINT time intervals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, ToolCallId};
use cyrup_ext::{
    DialogOptions, ExtMode, HostCtx, HostEvent, HookOutcome, HostServices, HumanInteractionLock,
    NativeExtension,
};
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
use cyrup_intercom::transport::spawn::wait_for_broker;
use cyrup_permission_system::PermissionSystemExtension;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use crate::common::registration;

/// The ONE `HostServices` sink shared by BOTH companions — exactly as the live session hands its ONE
/// `LiveHostServices` Arc to every native. It returns the SAME `Arc<HumanInteractionLock>` from
/// `human_interaction_lock` (the C3 shared lock); answers the PERMISSION dialog via `select` (which
/// BLOCKS, holding the shared lock, until the test releases it); answers the INTERCOM clarify via
/// `input`; and surfaces the inbound child ask via `append_entry`. Records the start/end instant of
/// each human prompt so the test can assert the two NEVER overlap.
struct SharedSink {
    human_lock: Arc<HumanInteractionLock>,
    /// Fires once when the permission `select` dialog has opened (⇒ the shared lock is now held).
    select_started_tx: UnboundedSender<()>,
    /// The permission `select` blocks on this receiver until the test sends `()` to release it.
    select_release: Mutex<std::sync::mpsc::Receiver<()>>,
    /// Flips true the instant the intercom clarify's `input` prompt actually begins — the observable
    /// signal of whether it double-prompted while the permission dialog held the lock.
    input_started: Arc<AtomicBool>,
    input_answer: String,
    /// Fires on each inbound-surface `append_entry` (⇒ the orchestrator recorded the child ask).
    append_tx: UnboundedSender<()>,
    /// `(label, start, end)` of every human prompt, in occurrence order — asserted disjoint.
    prompts: Mutex<Vec<(&'static str, Instant, Instant)>>,
}

impl HostServices for SharedSink {
    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        Some(Arc::clone(&self.human_lock))
    }

    /// The permission gate's registry / unknown-tool check (pi `index.ts:2218-2228`,
    /// `checkRequestedToolRegistration(toolName, pi.getAllTools())`) runs BEFORE any permission
    /// check and fails CLOSED — blocking every tool, never reaching `resolve_ask`/`select` at
    /// all — when [`HostServices::all_tool_names`] returns `None` (mirrors `tests/layers_wired.rs`'s
    /// own `RegistryServices` test double, this crate-pair's established convention for exercising
    /// the gate at all): a REAL host always reports its live tool registry, so this sink must too,
    /// or the `bash` `ToolCall` this test drives never reaches the `select` dialog it asserts on.
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["bash".to_string()])
    }

    fn select(&self, _prompt: &str, _options: &Value, _opts: &DialogOptions) -> Option<String> {
        let start = Instant::now();
        // The permission gate is now inside its `select` dialog HOLDING the shared lock — tell the test.
        let _ = self.select_started_tx.send(());
        // Block (holding the shared human-interaction lock via the gate's guard) until released — a
        // human deliberating over the approval. Nothing else locks this receiver, so no contention.
        let _ = self.select_release.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let end = Instant::now();
        self.prompts.lock().unwrap_or_else(|e| e.into_inner()).push(("permission-select", start, end));
        // "Allow Once" → the permission gate approves and the tool proceeds (HookOutcome::Noop).
        Some("Allow Once".to_string())
    }

    fn input(&self, _prompt: &str, _placeholder: Option<&str>, _opts: &DialogOptions) -> Option<String> {
        let start = Instant::now();
        self.input_started.store(true, Ordering::SeqCst);
        let end = Instant::now();
        self.prompts.lock().unwrap_or_else(|e| e.into_inner()).push(("intercom-input", start, end));
        Some(self.input_answer.clone())
    }

    fn append_entry(&self, _custom_type: &str, _data: &Value) -> Result<String, String> {
        let _ = self.append_tx.send(());
        Ok("entry-1".to_string())
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

async fn recv_signal(rx: &mut UnboundedReceiver<()>, what: &str) {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for: {what}"))
        .unwrap_or_else(|| panic!("signal channel closed before: {what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn permission_ask_holding_the_shared_lock_blocks_the_intercom_clarify_prompt() {
    let broker_bin = crate::support::bins::intercom_broker();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");
    // Empty policy ⇒ default ASK for bash (pi permission-manager.ts:44-50), so the permission gate
    // reaches the live human dialog.
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), "{}").unwrap();

    // The REAL broker as a genuine child OS process.
    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess");
    wait_for_broker(&socket_path, Duration::from_secs(5)).await.expect("broker becomes health-connectable");

    // ---- The orchestrator (supervisor) session: client + state + inbound loop.
    let orchestrator = Arc::new(
        IntercomClient::connect(&socket_path, registration("orchestrator"), Some("orch-session".to_string()))
            .await
            .expect("orchestrator registers"),
    );
    let orch_state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
    orch_state.set_client(Some(orchestrator.clone()));
    // The supervisor is the interactive human-facing session (pi `hasUI` = true): the inbound
    // delivery policy drives/steers a turn over the child ask rather than sending it a busy
    // auto-reply, so the child's outbound ask stays parked for the REAL human answer.
    orch_state.set_has_ui(true);

    // ---- The ONE shared HostServices sink (== the ONE shared HumanInteractionLock).
    let human_lock = Arc::new(HumanInteractionLock::new());
    let (select_started_tx, mut select_started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (append_tx, mut append_rx) = tokio::sync::mpsc::unbounded_channel();
    let input_started = Arc::new(AtomicBool::new(false));
    let sink = Arc::new(SharedSink {
        human_lock: human_lock.clone(),
        select_started_tx,
        select_release: Mutex::new(release_rx),
        input_started: input_started.clone(),
        input_answer: "Use Postgres.".to_string(),
        append_tx,
        prompts: Mutex::new(Vec::new()),
    });

    // BOTH companions get the SAME sink — the identical `set_host_services` late-bind the builder does
    // with one `LiveHostServices` Arc — so both resolve the SAME `HumanInteractionLock` instance.
    orch_state.set_host_services(sink.clone());
    let permission_ext =
        Arc::new(PermissionSystemExtension::new(agent_dir.path().to_path_buf(), agent_dir.path().to_path_buf()));
    permission_ext.set_host_services(sink.clone());
    assert!(
        Arc::ptr_eq(&sink.human_interaction_lock().expect("sink lock"), &human_lock),
        "both companions read the SAME session lock instance through HostServices::human_interaction_lock",
    );

    // The REAL production inbound loop records + surfaces every inbound message.
    spawn_inbound_loop(orch_state.clone(), orchestrator.clone());

    // ---- The child (subagent) session: client + its OWN state + inbound loop.
    let child = Arc::new(
        IntercomClient::connect(&socket_path, registration("subagent-chat-1"), None)
            .await
            .expect("child registers"),
    );
    let child_state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
    child_state.set_client(Some(child.clone()));
    spawn_inbound_loop(child_state.clone(), child.clone());

    // The child issues its blocking ask (body carries `Run: run-xyz` so the orchestrator's clarify can
    // correlate it). It parks until the human's answer routes back over the broker.
    let question_id = "question-abc".to_string();
    let ask_body =
        format_child_orchestrator_message(ChildMessageKind::Ask, &child_meta(), "Which database should I use?");
    let child_task = {
        let child_state = child_state.clone();
        let child = child.clone();
        let question_id = question_id.clone();
        tokio::spawn(async move {
            let cancel = CancelToken::new();
            child_state.ask_and_wait(&child, "orch-session", question_id, ask_body, None, &cancel).await
        })
    };

    // The orchestrator's inbound loop surfaced the child ask via `append_entry` ⇒ it is now RECORDED,
    // so the clarify below will correlate.
    recv_signal(&mut append_rx, "orchestrator inbound surface of the child ask").await;

    // ---- Start the PERMISSION ask: `resolve_ask` acquires the shared lock, then blocks in `select`
    //      HOLDING it (a human deliberating over the approval).
    let perm_task = {
        let ext = permission_ext.clone();
        tokio::spawn(async move {
            let ctx = HostCtx::event(ExtMode::Tui, true, PathBuf::from("/w"));
            let ev = HostEvent::ToolCall {
                call_id: ToolCallId::from("call-1"),
                name: "bash".to_string(),
                input: serde_json::json!({ "command": "ls -la" }),
            };
            ext.on_event(&ev, &ctx).await
        })
    };
    recv_signal(&mut select_started_rx, "permission select dialog to open").await;
    // The permission ask is now inside its `select` dialog HOLDING the shared lock (the behavioral
    // proof of "held" is that the intercom clarify below cannot prompt while this is true).

    // ---- Start the INTERCOM clarify concurrently: it must WAIT on the shared lock, never prompt.
    let clarify = IntercomClarifyChannel::new(orch_state.clone());
    let request = ClarifyRequest {
        run_id: RunId::from_token("run-xyz"),
        step_index: Some(0),
        prompt: "The subagent needs a database decision.".to_string(),
    };
    let clarify_task = tokio::spawn(async move { clarify.ask(request).await });

    // Give the clarify ample time to reach the lock. While the permission ask holds it, the intercom
    // `input` must NOT fire (the whole point of C3 — no double-prompt).
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !input_started.load(Ordering::SeqCst),
        "the intercom clarify must NOT prompt the human while the permission ask holds the shared lock",
    );
    assert!(!clarify_task.is_finished(), "the clarify is still WAITING on the shared lock, not resolved");

    // ---- Release the permission dialog: it returns, the gate drops its guard, the lock frees.
    release_tx.send(()).expect("release the permission dialog");
    let perm_outcome = tokio::time::timeout(Duration::from_secs(5), perm_task)
        .await
        .expect("permission resolved")
        .expect("permission task joined");
    assert!(
        matches!(perm_outcome, HookOutcome::Noop),
        "the human approved (Allow Once) → the tool proceeds: {perm_outcome:?}",
    );

    // ---- NOW the intercom clarify acquires the freed lock, prompts the human, routes the answer back.
    let clarify_answer = tokio::time::timeout(Duration::from_secs(5), clarify_task)
        .await
        .expect("clarify resolved within the timeout")
        .expect("clarify task joined")
        .expect("clarify returns the human answer");
    assert_eq!(clarify_answer, "Use Postgres.", "the clarify surfaced only AFTER the permission lock freed");
    assert!(input_started.load(Ordering::SeqCst), "the intercom input prompt fired once the lock freed");

    // ---- The two human prompts NEVER overlapped (disjoint intervals — the guarantee C3 exists for).
    let prompts = sink.prompts.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(prompts.len(), 2, "exactly two human prompts occurred: {prompts:?}");
    let (sel_label, _sel_start, sel_end) = prompts[0];
    let (inp_label, inp_start, _inp_end) = prompts[1];
    assert_eq!(sel_label, "permission-select", "the permission dialog prompted first");
    assert_eq!(inp_label, "intercom-input", "the intercom clarify prompted second");
    assert!(
        inp_start >= sel_end,
        "the intercom prompt began only AFTER the permission prompt ended — never simultaneously ({inp_start:?} vs {sel_end:?})",
    );

    // The child's blocking ask unblocked WITH the human's answer, over the real broker path.
    let child_reply = tokio::time::timeout(Duration::from_secs(5), child_task)
        .await
        .expect("the child ask resolved within the timeout")
        .expect("the child task joined")
        .expect("the child ask returned the reply");
    assert_eq!(child_reply, "Use Postgres.", "the human's answer reached the child through the broker");

    child.disconnect();
    orchestrator.disconnect();
    let _ = broker.kill().await;
}
