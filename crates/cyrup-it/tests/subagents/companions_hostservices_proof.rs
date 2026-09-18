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
//!   asserting the host's ACKNOWLEDGED injection seam (`inject_message_ack` — what
//!   `HostServicesCompletionSink` delivers through, because only a real `InjectOutcome::Accepted`
//!   licenses deleting the run's sole result payload) fired with `trigger_turn=true` and the
//!   `subagent-notify` custom type.
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
use cyrup_ext::host::{HostServices, InjectOutcome};
use cyrup_ext_subagents::background::{ResultFile, RunId, RunMode, RunState};
use cyrup_ext_subagents::extension::SubagentExecutor;
use cyrup_ext_subagents::fork_context::ContextMode;
use cyrup_ext_subagents::paths::Roots;
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
    Message::User {
        content: vec![Content::text(s)],
        timestamp: 0,
    }
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
fn create_persisted_session(
    cwd: &std::path::Path,
    layout: &SessionLayout,
    marker: &str,
) -> (PathBuf, String) {
    let mut mgr =
        SessionManager::create(cwd, layout, NewSessionOpts::default()).expect("create session");
    mgr.append_message(user(&format!("hello {marker}")))
        .expect("append user");
    mgr.append_message(assistant(&format!("hi from {marker}")))
        .expect("append assistant");
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
    executor.set_host_services(Arc::new(SessionFileServices {
        session_file: file_a.clone(),
    }));

    let fork = executor
        // SUBA-075: `force_thinking_off` — this fixture resolves no model ladder, which is the
        // case upstream's own `?? true` fallback covers.
        .resolve_context(cwd, ContextMode::Fork, true)
        .await
        .expect("fork resolves against the real session_file handle");

    let branch_path = fork
        .session_file_path
        .expect("a fork produces a concrete branch file");
    let reopened = SessionManager::open(&branch_path).expect("reopen the branched session");
    // `create_branched_session` records the parent's provenance on the branch header (the parent's
    // own session file, whose name embeds the parent session id) — pi lineage (R-SA-143).
    let parent = reopened
        .header()
        .parent_session
        .clone()
        .expect("branch records its parent");

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
// (e) LIVE COMPLETION TURN-INJECTION: inject_message_ack(trigger_turn=true), not the stderr sink.
// =================================================================================================

/// A host backend that records every injection, wired the way the ONE production backend
/// (`cyrup-session-svc`'s `LiveHostServices`, `host_services.rs:1578-1613`) is: both capability
/// methods funnel into the same recording queue, so the fire-and-forget and the ACKNOWLEDGED seam
/// cannot be two different delivery routes.
///
/// Implementing only `inject_message` would make this test vacuous rather than strict: the
/// completion sink under test is `HostServicesCompletionSink`, which delivers through
/// `inject_message_ack` (`background/watch/sink.rs:104-119`) precisely because it holds a
/// background run's SOLE terminal result file and may only destroy it against a real
/// `InjectOutcome::Accepted`. The trait's `inject_message_ack` default denies
/// (`cyrup-ext/src/host/services.rs:530-539`), so a recorder missing it records nothing and every
/// delivery defers forever.
#[derive(Default)]
struct RecordingInjectServices {
    /// (content, custom_type, display, trigger_turn) of each injection, from either seam.
    calls: Mutex<Vec<(String, Option<String>, bool, bool)>>,
}

impl RecordingInjectServices {
    /// The single delivery route both capability methods share — `LiveHostServices`'s
    /// `enqueue_injection` (`host_services.rs:1083-1106`) reduced to "the session took it".
    fn record(&self, content: &str, custom_type: Option<&str>, display: bool, trigger_turn: bool) {
        self.calls.lock().expect("inject lock").push((
            content.to_string(),
            custom_type.map(str::to_string),
            display,
            trigger_turn,
        ));
    }
}

impl HostServices for RecordingInjectServices {
    // The delivery watcher is session-scoped: `install_completion_watcher` builds its
    // `ResultDeliveryOwnership` from `services.session_id()` + this process's minted
    // `completion_owner_id`, and a session-less watcher owns nothing (`owns` refuses when either
    // identity is absent). The recorded session must match the one the ResultFile carries.
    fn session_id(&self) -> Option<String> {
        Some("it-proof-session".to_string())
    }

    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        _details: Option<&serde_json::Value>,
        trigger_turn: bool,
    ) -> Result<(), String> {
        self.record(content, custom_type, display, trigger_turn);
        Ok(())
    }

    fn inject_message_ack(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        _details: Option<&serde_json::Value>,
        trigger_turn: bool,
    ) -> Result<tokio::sync::oneshot::Receiver<InjectOutcome>, String> {
        self.record(content, custom_type, display, trigger_turn);
        // A live session resolves this once the message is really in the transcript; this recorder
        // IS the transcript, so the message is in it by the time the receiver is handed back.
        // `Accepted` is the only licence the sink has to consume the run's payload (R-SA-099).
        let (ack, rx) = tokio::sync::oneshot::channel();
        ack.send(InjectOutcome::Accepted)
            .map_err(|_| "ack receiver dropped".to_string())?;
        Ok(rx)
    }
}

fn completed_result(run_id: &str) -> ResultFile {
    ResultFile {
        // The identity pair `Attribution::classify` reads: a result with no session is
        // `Unattributed` (nothing happens, pi `result-watcher.ts:408`), and one with a foreign
        // owner is `ObserveOnly`. Delivery requires BOTH to match the watcher's own — the session
        // `RecordingInjectServices::session_id` reports, and THIS process's minted owner id.
        completion_owner_id: Some(cyrup_ext_subagents::identity::current_completion_owner_id()),
        session_id: cyrup_ext_subagents::identity::SessionId::parse("it-proof-session"),
        id: RunId::from_token(run_id),
        run_id: RunId::from_token(run_id),
        agent: "researcher".to_string(),
        mode: RunMode::Single,
        state: RunState::Complete,
        success: true,
        cwd: PathBuf::from("/tmp"),
        session_file: None,
        // SUBA-016: this fixture is a plain tool-launched run, not a scheduled one, so it carries
        // no `ScheduleOrigin` (pi `AsyncStatus.scheduleOrigin` is set only by the scheduled-run
        // manager). Spelled out because `ResultFile` has no `Default`, so the field's addition
        // broke this literal.
        schedule_origin: None,
        results: Vec::new(),
        workflow_children: None,
        workflow_receipt: None,
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
    tokio::fs::create_dir_all(&results_dir)
        .await
        .expect("mkdir results_dir");
    // Written THROUGH the session-partitioned index (stage → index → promote), exactly as the
    // detached runner's `finish_run` does: the watcher's candidate enumeration is index-driven
    // (pi `indexedResultCandidates`, `result-watcher.ts:634-644`) and never a directory scan of
    // the results root, so a raw public payload with no index entry is invisible to the prime
    // scan by design.
    let session_id = cyrup_ext_subagents::identity::SessionId::parse("it-proof-session")
        .expect("non-empty session id");
    let written_at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis(),
    )
    .expect("epoch millis fit i64");
    cyrup_ext_subagents::background::result_index::write_async_result_file(
        &cyrup_ext_subagents::background::result_index::ResultWrite {
            results_dir: &results_dir,
            session_id: &session_id,
            run_id: &RunId::from_token("runproofe000000e"),
            written_at,
            async_dir: None,
            tool_call_id: None,
        },
        &completed_result("runproofe000000e"),
    )
    .await
    .expect("write result through the session-partitioned index");

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
            panic!(
                "inject_message_ack never fired: the completion did not reach the live host services"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let calls = services.calls.lock().expect("lock");
    assert_eq!(
        calls.len(),
        1,
        "exactly one live injection for one completion, got: {calls:?}"
    );
    let (content, custom_type, display, trigger_turn) = &calls[0];
    assert!(
        trigger_turn,
        "R-SA-101: the completion MUST re-enter the parent turn loop (trigger_turn=true)"
    );
    assert_eq!(
        custom_type.as_deref(),
        Some("subagent-notify"),
        "pi's fixed customType"
    );
    assert!(
        !display,
        "SUBA-090: a plain successful background completion is injected hidden \
         (`display` false, notify.ts:402 @v0.64.0) while still triggering the turn"
    );
    assert!(
        content.contains("Background task completed") && content.contains("researcher"),
        "the injected body is the real notify.ts content: {content}",
    );
}
