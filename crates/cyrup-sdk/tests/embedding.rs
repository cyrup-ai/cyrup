//! SDK embedding tests (arch-11 §11, A-11-6).
//!
//! Build a [`cyrup_sdk::Session`] over a scripted [`FauxProvider`] in a tempdir (no network/tokens)
//! and exercise the documented embedding surface: run a prompt to completion and collect the final
//! assistant text; stream typed events and assert their order; abort a running tool; and read state
//! (messages / last_assistant_text / session_id).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use cyrup_sdk::{AgentSessionEvent, Cyrup, InputSource, Session, SessionConfig, UserInput};
use futures::StreamExt;
use tempfile::TempDir;

// ----------------------------------------------------------------------------------------------
// Fixtures
// ----------------------------------------------------------------------------------------------

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project
    cfg
}

async fn build_session(fx: &Fixture, faux: Arc<FauxProvider>) -> Session {
    let provider: Arc<dyn Provider> = faux;
    Cyrup::builder().build_session(provider, config(fx)).await.expect("build session")
}

// ----------------------------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------------------------

/// `run` drives a prompt to completion and returns the final assistant text.
#[tokio::test]
async fn run_collects_final_assistant_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("the answer is 42")],
        StopReason::Stop,
    )]);

    let session = build_session(&fx, faux).await;
    let text = session.run("what is the answer?").await.expect("run completes");
    assert_eq!(text, "the answer is 42");
}

/// `run_collecting` yields the typed event stream in order, terminating at `agent_end`.
#[tokio::test]
async fn streamed_events_arrive_in_order() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("hello there")],
        StopReason::Stop,
    )]);

    let session = build_session(&fx, faux).await;
    let (events, text) = session
        .run_collecting(UserInput::text("greet me", InputSource::Sdk))
        .await
        .expect("run_collecting completes");

    let kinds: Vec<&str> = events.iter().map(AgentSessionEvent::kind).collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "must start with agent_start: {kinds:?}");
    assert_eq!(kinds.last(), Some(&"agent_end"), "must end with agent_end: {kinds:?}");

    // turn_start precedes message_start precedes message_end precedes agent_end.
    let pos = |k: &str| kinds.iter().position(|x| *x == k);
    assert!(pos("turn_start") < pos("message_end"), "turn_start before message_end: {kinds:?}");
    assert!(
        pos("message_start") < pos("message_end"),
        "message_start before message_end: {kinds:?}"
    );
    assert!(pos("message_end") < pos("agent_end"), "message_end before agent_end: {kinds:?}");

    assert_eq!(text.as_deref(), Some("hello there"));
}

/// Reading state via the SDK: messages, last_assistant_text, session_id, system_prompt.
#[tokio::test]
async fn reads_state_after_a_run() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("stateful reply")],
        StopReason::Stop,
    )]);

    let session = build_session(&fx, faux).await;
    let id_before = session.session_id().clone();

    let _ = session.run("remember this").await.expect("run completes");

    // last_assistant_text reflects the run.
    assert_eq!(session.last_assistant_text().await.as_deref(), Some("stateful reply"));

    // messages: user prompt then assistant reply persisted on the branch.
    let msgs = session.messages().await;
    assert_eq!(msgs.len(), 2, "expected user + assistant, got {}", msgs.len());
    assert!(
        matches!(msgs[0], cyrup_core::Message::User { .. }),
        "first message is the user prompt"
    );
    assert!(
        matches!(msgs[1], cyrup_core::Message::Assistant(_)),
        "second message is the assistant reply"
    );

    // session_id is stable; the system prompt is assembled (non-empty).
    assert_eq!(session.session_id(), &id_before);
    assert!(!session.system_prompt().is_empty(), "system prompt should be assembled");
    assert!(!session.is_streaming().await, "session must be idle after the run");
}

/// Aborting a running tool unblocks the run (no deadlock). The faux provider schedules a long
/// `bash` (`sleep`) call; the SDK's `prompt` stream surfaces `tool_execution_start`, then `abort`
/// cancels the run — `wait_for_idle` must return promptly and the session is idle.
#[tokio::test]
async fn abort_cancels_a_running_tool() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Turn 1: call the builtin bash tool with a long sleep. After abort, no further response needed.
    // (Build the JSON args via `FromStr` so the test needs no direct `serde_json` dependency; the
    // target type `serde_json::Value` is inferred from `faux_tool_call`'s parameter.)
    let bash_args = r#"{"command": "sleep 30"}"#.parse().expect("valid json");
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_tool_call("bash", bash_args)],
        StopReason::ToolUse,
    )]);

    let session = build_session(&fx, faux).await;
    let mut stream = session.prompt("run the sleeper").await.expect("prompt accepted");

    // Wait until the tool starts executing, then abort the run via the SDK handle.
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(ev)) if ev.kind() == "tool_execution_start" => {
                session.abort();
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => panic!("stream ended before the tool started"),
            Err(_) => panic!("timed out waiting for tool_execution_start"),
        }
    }

    // The run must settle quickly (proving abort killed the blocking bash), not after `sleep 30`.
    tokio::time::timeout(Duration::from_secs(5), session.wait_for_idle())
        .await
        .expect("wait_for_idle must complete after abort");
    assert!(!session.is_streaming().await, "session must be idle after abort");
}
