//! Dispatch integration tests (arch-11 §11 A-11-2/A-11-3): build a real `AgentSession` over a
//! scripted faux provider in a tempdir and exercise the PRINT and JSON dispatchers into `Vec<u8>`
//! buffers — no TTY, no network. Asserts the final assistant text (PRINT) and the ordered JSONL
//! event sequence (JSON), plus follow-up replay (R-11-009).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup::input::Inputs;
use cyrup::run::{run_json_dispatch, run_print_dispatch};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_sdk::core::{AssistantMessage, StopReason};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, SessionTarget};
use tempfile::TempDir;

/// Build an ephemeral in-memory session over a faux provider scripted with `responses`.
/// The tempdirs are returned so they outlive the session.
async fn session_with(responses: Vec<AssistantMessage>) -> (AgentSession, TempDir, TempDir) {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;

    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false; // ephemeral, like one-shot PRINT/JSON (R-11-008)

    let session = SessionBuilder::new(provider, config).build().await.unwrap();
    (session, cwd, agent_dir)
}

fn text(initial: &str, follow_ups: &[&str]) -> Inputs {
    Inputs {
        initial: initial.to_string(),
        images: Vec::new(),
        follow_ups: follow_ups.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn print_dispatch_writes_final_assistant_text() {
    let (session, _cwd, _agent) = session_with(vec![faux_assistant_message(
        vec![faux_text("hello world")],
        StopReason::Stop,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    let code = run_print_dispatch(&session, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    let printed = String::from_utf8(out).unwrap();
    assert_eq!(printed.trim(), "hello world");
    assert_eq!(code, 0, "a clean Stop terminal reason maps to exit 0");
}

#[tokio::test]
async fn print_dispatch_replays_follow_ups_in_order() {
    let (session, _cwd, _agent) = session_with(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ])
    .await;

    let mut out: Vec<u8> = Vec::new();
    run_print_dispatch(&session, &text("q1", &["q2"]), &mut out)
        .await
        .unwrap();

    // PRINT mode emits ONLY the final transcript message (Pi print-mode.ts:129-146; cyrup commit
    // a2c1bf5). The faux provider answers turns in script order, so the printed final is the
    // follow-up's response ("second answer") — proving the follow-up (q2) was replayed AFTER the
    // initial (q1) consumed "first answer" (R-11-009). The initial turn's text is NOT printed.
    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("second answer"),
        "the follow-up's response is the final printed message, got: {printed:?}"
    );
    assert!(
        !printed.contains("first answer"),
        "only the final message is printed; the initial turn's text is suppressed, got: {printed:?}"
    );
}

#[tokio::test]
async fn json_dispatch_emits_ordered_event_stream() {
    let (session, _cwd, _agent) = session_with(vec![faux_assistant_message(
        vec![faux_text("hi there")],
        StopReason::Stop,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    let code = run_json_dispatch(&session, &text("hello", &[]), &mut out)
        .await
        .unwrap();

    let body = String::from_utf8(out).unwrap();
    let kinds: Vec<String> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("each JSONL line is a JSON object");
            value
                .get("type")
                .and_then(|t| t.as_str())
                .expect("every event carries a snake_case `type` tag")
                .to_string()
        })
        .collect();

    // JSON mode writes `sessionManager.getHeader()` as JSONL line 1 before the event stream (Pi
    // print-mode.ts:112-117; cyrup commit cbbde87), so the stream opens with the `session` header,
    // then `agent_start`, and closes with `agent_end`.
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("session"),
        "stream opens with the session header line"
    );
    assert_eq!(
        kinds.get(1).map(String::as_str),
        Some("agent_start"),
        "the first event after the header is agent_start"
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("agent_end"),
        "stream closes with agent_end"
    );
    assert_eq!(code, 0);
}

/// JSON mode ALWAYS returns exit 0, even when the final turn errored/aborted (Pi print-mode.ts:34,
/// 129-148: `exitCode` inits to 0 and is mutated only inside `if (mode === "text")`, so JSON never
/// leaves 0). The contrast below shows the SAME failed turn in PRINT/text mode DOES surface exit 1 —
/// so the divergence is JSON-mode-specific, not a session-wide change.
#[tokio::test]
async fn json_dispatch_always_exits_zero_even_on_failed_turn() {
    let (session, _cwd, _agent) = session_with(vec![faux_assistant_message(
        vec![faux_text("boom")],
        StopReason::Error,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    let code = run_json_dispatch(&session, &text("hi", &[]), &mut out)
        .await
        .unwrap();
    assert_eq!(
        code, 0,
        "JSON mode always returns 0 regardless of the terminal stop reason (Pi convention)"
    );

    // Contrast: the identical failed turn in PRINT/text mode surfaces exit 1 (print-mode.ts:135-137).
    let (session2, _cwd2, _agent2) = session_with(vec![faux_assistant_message(
        vec![faux_text("boom")],
        StopReason::Error,
    )])
    .await;
    let mut sink: Vec<u8> = Vec::new();
    let text_code = run_print_dispatch(&session2, &text("hi", &[]), &mut sink)
        .await
        .unwrap();
    assert_eq!(
        text_code, 1,
        "PRINT/text mode DOES surface a failed final turn as exit 1"
    );
}

/// `--session-id <fresh>` builds via the new `SessionTarget::CreateWithId` arm (Pi
/// `SessionManager.create(cwd, dir, { id })`, main.ts:349) — the session adopts the exact id and
/// persists. Previously a fresh id hit a literal `open` and failed; this exercises the create arm.
#[tokio::test]
async fn create_with_id_target_adopts_the_exact_id() {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = true;
    config.target = SessionTarget::CreateWithId("my-custom-id".to_string());

    let session = SessionBuilder::new(provider, config).build().await.unwrap();
    assert_eq!(session.session_id().as_str(), "my-custom-id");
}

/// `--fork <ref>` builds via the new `SessionTarget::Fork` arm (Pi `SessionManager.forkFrom`,
/// main.ts:251): the source session's history is copied into a fresh session that adopts the supplied
/// `--session-id`. Exercises the create-source → flush → fork-with-id path end-to-end.
#[tokio::test]
async fn fork_target_copies_history_into_a_new_id() {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    // 1. Build + run a persisted SOURCE session so a file with entries exists on disk.
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("source answer")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut src_cfg = SessionConfig::new(cwd.path(), agent_dir.path());
    src_cfg.persist = true;
    let source = SessionBuilder::new(provider, src_cfg)
        .build()
        .await
        .unwrap();
    let mut sink: Vec<u8> = Vec::new();
    run_print_dispatch(&source, &text("seed", &[]), &mut sink)
        .await
        .unwrap();
    let source_file = source
        .session_file()
        .await
        .expect("source session flushed to disk");

    // 2. Fork that file into a fresh session that adopts the explicit id.
    let provider2: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut fork_cfg = SessionConfig::new(cwd.path(), agent_dir.path());
    fork_cfg.persist = true;
    fork_cfg.target = SessionTarget::Fork {
        source: source_file,
        id: Some("forked-id".to_string()),
    };
    let forked = SessionBuilder::new(provider2, fork_cfg)
        .build()
        .await
        .unwrap();

    assert_eq!(
        forked.session_id().as_str(),
        "forked-id",
        "fork adopts --session-id"
    );
    assert!(
        !forked.entries_json().await.is_empty(),
        "fork copies the source history"
    );
}

// ----------------------------------------------------------------------------------------------
// SEAM-002 — every non-interactive host must tear the session down on a normal exit, emitting
// `session_shutdown{reason:"quit"}`. Pi reaches `AgentSessionRuntime.dispose()`
// (agent-session-runtime.ts:397-404) on EVERY exit: print-mode.ts's `finally { await
// disposeRuntime() }` (:152-157) and rpc-mode.ts's `shutdown()` (:723-739), triggered by stdin EOF
// (:801-803). Pre-fix cyrup's `dispose()` had zero production callers, so no extension and no
// subscriber ever observed a shutdown on a normal `cyrup -p …` / `--mode rpc` run.
//
// Note the ORDER pi uses — `unsubscribe()` then `dispose()` (rpc-mode.ts:731-733) — so the shutdown
// is NOT written to the mode's own output sink; it is observed by an independent subscriber (and by
// extensions), which is exactly what these tests assert.
// ----------------------------------------------------------------------------------------------

/// Drain `sub` for up to `budget` events and report whether a `session_shutdown` with
/// `reason == "quit"` came through.
async fn saw_quit_shutdown(
    sub: &mut cyrup_sdk::core::EventStream<cyrup_session_svc::AgentSessionEvent>,
) -> bool {
    use futures::StreamExt;
    for _ in 0..200 {
        match tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await {
            Ok(Some(cyrup_session_svc::AgentSessionEvent::SessionShutdown { reason })) => {
                assert_eq!(reason, "quit", "Pi disposes with reason `quit`");
                return true;
            }
            Ok(Some(_)) => {}
            _ => return false,
        }
    }
    false
}

#[tokio::test]
async fn print_dispatch_disposes_the_session_on_exit() {
    let (session, _cwd, _agent) = session_with(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )])
    .await;
    let mut sub = session.subscribe();

    let mut out: Vec<u8> = Vec::new();
    run_print_dispatch(&session, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    assert!(
        saw_quit_shutdown(&mut sub).await,
        "PRINT dispatch must emit session_shutdown{{reason:\"quit\"}} on exit (Pi print-mode.ts:152-157)"
    );
}

#[tokio::test]
async fn json_dispatch_disposes_the_session_on_exit() {
    let (session, _cwd, _agent) = session_with(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )])
    .await;
    let mut sub = session.subscribe();

    let mut out: Vec<u8> = Vec::new();
    run_json_dispatch(&session, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    assert!(
        saw_quit_shutdown(&mut sub).await,
        "JSON dispatch must emit session_shutdown{{reason:\"quit\"}} on exit (Pi print-mode.ts:152-157)"
    );
}

/// RPC: reader EOF is Pi's `process.stdin.on("end") → shutdown() → runtimeHost.dispose()`
/// (rpc-mode.ts:801-803 / :723-739).
#[tokio::test]
async fn rpc_dispatch_disposes_the_runtime_at_reader_eof() {
    use cyrup_session_svc::{AgentSessionRuntime, SessionFactory};

    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, config));
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();

    let session = runtime.session().await;
    let mut sub = session.subscribe();

    // A single command, then EOF.
    let reader =
        tokio::io::BufReader::new(std::io::Cursor::new(b"{\"type\":\"get_state\",\"id\":\"s\"}\n".to_vec()));
    let mut writer: Vec<u8> = Vec::new();
    cyrup::run::run_rpc_dispatch(&runtime, reader, &mut writer)
        .await
        .unwrap();

    assert!(
        saw_quit_shutdown(&mut sub).await,
        "RPC dispatch must dispose the runtime at reader EOF (Pi rpc-mode.ts:723-739/:801-803)"
    );
}
