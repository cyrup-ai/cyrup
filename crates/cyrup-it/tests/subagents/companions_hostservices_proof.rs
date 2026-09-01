//! FULLY-WIRED PROOFS for the two P0-host-services-closeable blockers (reconciliation §4 step 5
//! items 5 + 6), driven through the REAL production seams — no mocks of the wired code:
//!
//! - **(d)** fork-context branches from the REAL `HostServices::session_file()` handle (blocker #4),
//!   NOT `SessionManager::continue_recent(cwd)`'s most-recent-mtime heuristic. Proven by creating
//!   TWO persisted sessions in one cwd (the heuristic would pick the newer), pointing
//!   `session_file()` at the OLDER one, and asserting the fork branched from the OLDER (its id shows
//!   up in the branch's `parentSession` provenance).
//! - **(e)** a completed background run's notification is INJECTED live into the orchestrator session
//!   via `HostServices::inject_message(..., trigger_turn=true)` (R-SA-101), on the REAL scripted
//!   backend — NOT the stderr `LoggingCompletionSink`. Proven by binding a recording `HostServices`
//!   into the executor, installing the real completion watcher, dropping a terminal `ResultFile`, and
//!   asserting `inject_message` fired with `trigger_turn=true` and the `subagent-notify` custom type.
//!
//! These exercise `SubagentExecutor::{set_host_services, resolve_context, install_completion_watcher}`
//! end to end — the seams `crates/cyrup-session-svc/src/builder.rs`'s P-1 `load_native_with_services`
//! late-binds in production and `extension.rs`'s `SessionStart` handler drives.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::type_complexity
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::{AssistantMessage, Content, Message, StopReason, Usage};
use cyrup_ext::host::HostServices;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::background::{ResultFile, RunId, RunMode, RunState};
use cyrup_ext_subagents::extension::SubagentExecutor;
use cyrup_ext_subagents::fork_context::ContextMode;
use cyrup_session::{NewSessionOpts, SessionLayout, SessionManager};

/// The sandbox root for one test, handed to the executor as `SubagentExtensionConfig::roots`.
fn sandboxed(home: &std::path::Path) -> cyrup_ext_subagents::registration::SubagentExtensionConfig {
    cyrup_ext_subagents::registration::SubagentExtensionConfig {
        roots: Roots::sandboxed(home),
        ..Default::default()
    }
}

fn assistant(s: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(s)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    })
}

fn user(s: &str) -> Message {
    Message::User { content: vec![Content::text(s)], timestamp: 0 }
}

/// A `HostServices` backend that returns a fixed `session_file()` — the seam blocker #4 consumes.
struct SessionFileServices {
    session_file: PathBuf,
}

impl HostServices for SessionFileServices {
    fn session_file(&self) -> Option<PathBuf> {
        Some(self.session_file.clone())
    }
}

/// Create one persisted parent session under `layout` (a user + assistant message → a real leaf +
/// on-disk JSONL), returning its persisted file path + its session id.
fn create_persisted_session(cwd: &std::path::Path, layout: &SessionLayout, marker: &str) -> (PathBuf, String) {
    let mut mgr = SessionManager::create(cwd, layout, NewSessionOpts::default()).expect("create session");
    mgr.append_message(user(&format!("hello {marker}"))).expect("append user");
    mgr.append_message(assistant(&format!("hi from {marker}"))).expect("append assistant");
    let file = mgr.session_file().expect("session persisted").to_path_buf();
    let id = mgr.session_id().to_string();
    (file, id)
}

// =================================================================================================
// (d) FORK-CONTEXT CORRECTNESS: branch from the REAL session_file(), not continue_recent's guess.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_branches_from_the_real_session_file_handle_not_the_mtime_heuristic() {

    let home = tempfile::tempdir().expect("home tempdir");
    let cwd_dir = tempfile::tempdir().expect("cwd tempdir");
    let cwd = cwd_dir.path();

    // `fork_resolver` derives its layout root from the executor's own `roots`; point that at
    // a hermetic tempdir so the branch (and continue_recent's discovery) stay isolated.
    let sessions_root = home.path().join(".cyrup").join("sessions");
    let layout = SessionLayout::new(sessions_root, cwd.to_path_buf());

    // Session A (OLDER) then session B (NEWER) in the SAME cwd. `continue_recent(cwd)` — the OLD
    // heuristic — would pick B (newest mtime). We point `session_file()` at A instead.
    let (file_a, id_a) = create_persisted_session(cwd, &layout, "session-A-old");
    std::thread::sleep(Duration::from_millis(20));
    let (_file_b, id_b) = create_persisted_session(cwd, &layout, "session-B-new");
    assert_ne!(id_a, id_b, "the two sessions must be distinct");

    // Bind a HostServices whose session_file() is the OLDER session A, then resolve a fork.
    let executor = SubagentExecutor::with_config(sandboxed(home.path()));
    executor.set_host_services(Arc::new(SessionFileServices { session_file: file_a.clone() }));

    let fork = executor
        // SUBA-075: `force_thinking_off` — this fixture resolves no model ladder, which is the
        // case upstream's own `?? true` fallback covers.
        .resolve_context(cwd, ContextMode::Fork, true)
        .await
        .expect("fork resolves against the real session_file handle");

    let branch_path = fork.session_file_path.expect("a fork produces a concrete branch file");
    let reopened = SessionManager::open(&branch_path).expect("reopen the branched session");
    // `create_branched_session` records the parent's provenance on the branch header (the parent's
    // own session file, whose name embeds the parent session id) — pi lineage (R-SA-143).
    let parent = reopened.header().parent_session.clone().expect("branch records its parent");

    assert!(
        parent.contains(&id_a),
        "the fork MUST branch from the real session_file() handle (session A, id={id_a}); \
         branch parent provenance was {parent}",
    );
    assert!(
        !parent.contains(&id_b),
        "branching from session B (id={id_b}) would mean the discarded continue_recent \
         newest-for-cwd heuristic was used; branch parent provenance was {parent}",
    );
}

// =================================================================================================
// (e) LIVE COMPLETION TURN-INJECTION: inject_message(trigger_turn=true), not the stderr sink.
// =================================================================================================

#[derive(Default)]
struct RecordingInjectServices {
    /// (content, custom_type, display, trigger_turn) of each `inject_message` call.
    calls: Mutex<Vec<(String, Option<String>, bool, bool)>>,
}

impl HostServices for RecordingInjectServices {
    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        _details: Option<&serde_json::Value>,
        trigger_turn: bool,
    ) -> Result<(), String> {
        self.calls
            .lock()
            .expect("inject lock")
            .push((content.to_string(), custom_type.map(str::to_string), display, trigger_turn));
        Ok(())
    }
}

fn completed_result(run_id: &str) -> ResultFile {
    ResultFile {
        id: RunId::from_token(run_id),
        run_id: RunId::from_token(run_id),
        agent: "researcher".to_string(),
        mode: RunMode::Single,
        state: RunState::Complete,
        success: true,
        cwd: PathBuf::from("/tmp"),
        session_file: None,
        results: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_completion_injects_a_turn_triggering_message_on_the_real_host_services() {

    let home = tempfile::tempdir().expect("home tempdir");
    let cwd_dir = tempfile::tempdir().expect("cwd tempdir");
    let cwd = cwd_dir.path();
    // Hermetic results-dir root, named directly rather than moved through process env.

    // A completing background run writes its terminal ResultFile into ResultsDir (the runner's last
    // act, R-SA-077) — dropped BEFORE the watcher installs so the watcher's prime scan delivers it.
    let results_dir =
        cyrup_ext_subagents::background::run_artifact_roots_in(&Roots::sandboxed(home.path()), cwd)
            .results_dir;
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let result_path = results_dir.join("run-notify-e.json");
    cyrup_ext_subagents::background::atomic::write_atomic_json(&result_path, &completed_result("runproofe000000e"))
        .await
        .expect("write result");

    // Bind the recording HostServices (P-1) into the executor, exactly as the builder's
    // `load_native_with_services` does in production, then install the REAL completion watcher —
    // which derives the live `HostServicesCompletionSink` (NOT the stderr LoggingCompletionSink)
    // BECAUSE a host handle is present, and primes-scans the already-on-disk result.
    let services = Arc::new(RecordingInjectServices::default());
    let executor = SubagentExecutor::with_config(sandboxed(home.path()));
    executor.set_host_services(services.clone());
    executor.install_completion_watcher(cwd).await;

    // Wait (bounded) for the watcher to drain the result into the live inject sink.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !services.calls.lock().expect("lock").is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            panic!("inject_message never fired: the completion did not reach the live host services");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let calls = services.calls.lock().expect("lock");
    assert_eq!(calls.len(), 1, "exactly one live injection for one completion, got: {calls:?}");
    let (content, custom_type, display, trigger_turn) = &calls[0];
    assert!(trigger_turn, "R-SA-101: the completion MUST re-enter the parent turn loop (trigger_turn=true)");
    assert_eq!(custom_type.as_deref(), Some("subagent-notify"), "pi's fixed customType");
    assert!(display, "pi's fixed display=true");
    assert!(
        content.contains("Background task completed") && content.contains("researcher"),
        "the injected body is the real notify.ts content: {content}",
    );
}
