//! Integration tests for the non-interactive adapters (arch-11 §2.2; func-11 R-11-005/007/011…016).
//!
//! Each test builds a real wired [`AgentSession`] over a scripted `FauxProvider` in a tempdir and
//! drives one adapter into an in-memory sink, then asserts on the produced bytes — exactly how the
//! binary will drive them over real stdio.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use cyrup_modes::{run_json, run_print, run_rpc, PrintOptions, SessionCommand};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{InputSource, SessionBuilder, SessionConfig, UserInput};
use cyrup_core::StopReason;
use serde_json::Value;
use tempfile::TempDir;

// ----------------------------------------------------------------------------------------------
// Fixture
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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project, no prompt
    cfg
}

async fn build_session(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
) -> cyrup_session_svc::AgentSession {
    let provider: Arc<dyn Provider> = faux;
    SessionBuilder::new(provider, base_config(fx)).build().await.expect("build session")
}

/// Parse the produced sink bytes into one `serde_json::Value` per non-empty LF-delimited line.
fn parse_lines(bytes: &[u8]) -> Vec<Value> {
    let text = String::from_utf8(bytes.to_vec()).expect("utf8 output");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("each line is valid json"))
        .collect()
}

fn type_of(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

// ----------------------------------------------------------------------------------------------
// PRINT mode
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn print_mode_emits_final_assistant_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("the final answer")],
        StopReason::Stop,
    )]);
    let session = build_session(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    run_print(
        &session,
        UserInput::text("what is the answer?", InputSource::Cli),
        &mut out,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("the final answer"), "final assistant text missing:\n{text}");
}

// ----------------------------------------------------------------------------------------------
// JSON mode
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn json_mode_emits_ordered_event_stream() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // turn 1: assistant w/ a `write` tool call; turn 2: final assistant text.
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("write", serde_json::json!({"path": "hi.txt", "content": "x"}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let session = build_session(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    run_json(&session, UserInput::text("write a file", InputSource::Cli), &mut out)
        .await
        .expect("json mode runs");

    let events = parse_lines(&out);
    assert!(!events.is_empty(), "no events emitted");

    // Every line parsed; the ordered `type` tags bracket the run.
    let kinds: Vec<&str> = events.iter().map(type_of).collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "must start with agent_start: {kinds:?}");
    assert_eq!(kinds.last(), Some(&"agent_end"), "must end with agent_end: {kinds:?}");

    let tes = kinds.iter().position(|k| *k == "tool_execution_start").expect("tool_execution_start");
    let tee = kinds.iter().position(|k| *k == "tool_execution_end").expect("tool_execution_end");
    assert!(tes < tee, "tool exec start must precede end: {kinds:?}");

    // message_end role order = user -> assistant(toolCall) -> toolResult -> assistant.
    let roles: Vec<String> = events
        .iter()
        .filter(|e| type_of(e) == "message_end")
        .map(message_role)
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "toolResult", "assistant"],
        "message_end roles out of order: {roles:?}"
    );
}

/// The `role` of a `message_end` event payload, distinguishing a tool-call assistant by content.
fn message_role(ev: &Value) -> String {
    ev.get("message")
        .and_then(|m| m.get("role"))
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string()
}

// ----------------------------------------------------------------------------------------------
// RPC mode
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn rpc_mode_drives_prompt_and_answers_queries() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("rpc answer")],
        StopReason::Stop,
    )]);
    let session = build_session(&fx, faux).await;

    // A scripted request stream: prompt, then an abort, then two state queries.
    let input = concat!(
        r#"{"type":"prompt","id":"1","message":"hello"}"#,
        "\n",
        r#"{"type":"abort","id":"2"}"#,
        "\n",
        r#"{"type":"get_state","id":"3"}"#,
        "\n",
        r#"{"type":"get_commands","id":"4"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();

    run_rpc(&session, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    assert!(!lines.is_empty(), "no output produced");

    let responses: Vec<&Value> = lines.iter().filter(|l| type_of(l) == "response").collect();
    let events: Vec<&Value> = lines.iter().filter(|l| type_of(l) != "response").collect();

    // (1) the prompt was accepted, correlated by id.
    let prompt_resp = responses
        .iter()
        .find(|r| r["command"] == "prompt")
        .expect("a prompt response");
    assert_eq!(prompt_resp["id"], "1");
    assert_eq!(prompt_resp["success"], true);

    // (2) abort acknowledged.
    let abort_resp = responses.iter().find(|r| r["command"] == "abort").expect("an abort response");
    assert_eq!(abort_resp["id"], "2");
    assert_eq!(abort_resp["success"], true);

    // (3) get_state returns a session snapshot.
    let state = responses.iter().find(|r| r["command"] == "get_state").expect("a get_state response");
    assert_eq!(state["id"], "3");
    assert!(state["data"]["sessionId"].is_string(), "state missing sessionId: {state}");
    assert!(state["data"]["model"]["provider"].is_string(), "state missing model: {state}");

    // (4) get_commands lists the supported verbs.
    let cmds = responses
        .iter()
        .find(|r| r["command"] == "get_commands")
        .expect("a get_commands response");
    assert_eq!(cmds["id"], "4");
    let names: Vec<&str> = cmds["data"]
        .as_array()
        .expect("commands array")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"prompt"), "command catalog missing prompt: {names:?}");
    assert!(names.contains(&"get_state"), "command catalog missing get_state: {names:?}");

    // (5) the agent event stream surfaced on the protocol (at least the run start).
    let kinds: Vec<&str> = events.iter().copied().map(type_of).collect();
    assert!(kinds.contains(&"agent_start"), "no agent_start event on the stream: {kinds:?}");
}

#[tokio::test]
async fn rpc_fork_returns_a_new_session_id() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("forked answer")],
        StopReason::Stop,
    )]);
    let session = build_session(&fx, faux).await;
    let original = session.session_id().as_str().to_string();

    // A fork needs a non-empty source: run one prompt to completion so the session has entries.
    session
        .prompt_accepted(UserInput::text("seed the session", InputSource::Cli))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;

    let input = "{\"type\":\"fork\",\"id\":\"42\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&session, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| l["command"] == "fork").expect("a fork response");
    assert_eq!(resp["id"], "42");
    assert_eq!(resp["success"], true, "fork must succeed: {resp}");
    let new_id = resp["data"]["sessionId"].as_str().expect("fork returns a sessionId");
    assert!(!new_id.is_empty(), "fork sessionId must be non-empty: {resp}");
    assert_ne!(new_id, original, "fork must mint a new session id: {resp}");
}

#[tokio::test]
async fn rpc_unknown_command_echoes_id_on_failure() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = build_session(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&session, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "unknown command must fail: {resp}");
    assert_eq!(resp["id"], "9", "unknown command must echo the request id: {resp}");
}

#[tokio::test]
async fn rpc_malformed_command_echoes_id_on_failure() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = build_session(&fx, faux).await;

    // A valid JSON object with a known `type` but a missing required field (`message`).
    let input = "{\"type\":\"prompt\",\"id\":\"13\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&session, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "malformed command must fail: {resp}");
    assert_eq!(resp["id"], "13", "malformed command must echo the request id: {resp}");
    assert!(resp["error"].is_string(), "malformed command must carry an error: {resp}");
}

#[tokio::test]
async fn rpc_unknown_command_is_a_failure_not_a_panic() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = build_session(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&session, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "unknown command must fail: {resp}");
    assert!(resp["error"].is_string(), "unknown command must carry an error: {resp}");
}

/// `SessionCommand` deserializes the documented snake_case `type` tags + camelCase fields.
#[test]
fn session_command_parses_streaming_behavior() {
    let cmd: SessionCommand = serde_json::from_str(
        r#"{"type":"prompt","id":"7","message":"hi","streaming_behavior":"followUp"}"#,
    )
    .expect("parse prompt");
    match cmd {
        SessionCommand::Prompt { id, message, streaming_behavior } => {
            assert_eq!(id.as_deref(), Some("7"));
            assert_eq!(message, "hi");
            assert!(streaming_behavior.is_some());
        }
        other => panic!("expected Prompt, got {other:?}"),
    }
}
