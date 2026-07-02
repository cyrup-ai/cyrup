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

    let printed = String::from_utf8(out).unwrap();
    let first = printed.find("first answer").expect("first answer printed");
    let second = printed
        .find("second answer")
        .expect("second answer printed");
    assert!(
        first < second,
        "follow-up replayed after the initial run (R-11-009)"
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

    assert_eq!(
        kinds.first().map(String::as_str),
        Some("agent_start"),
        "stream opens with agent_start"
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("agent_end"),
        "stream closes with agent_end"
    );
    assert_eq!(code, 0);
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
