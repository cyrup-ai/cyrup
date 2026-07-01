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
use cyrup_session_svc::{
    AgentSessionRuntime, InputSource, SessionBuilder, SessionConfig, SessionFactory, UserInput,
};
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

/// Build the multi-session runtime host the RPC adapter drives (Pi `rpc-mode.ts` `runtimeHost`).
async fn build_runtime(fx: &Fixture, faux: Arc<FauxProvider>) -> AgentSessionRuntime {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, cfg));
    AgentSessionRuntime::create(factory, target).await.expect("build runtime")
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
    let runtime = build_runtime(&fx, faux).await;

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

    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

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

    // (3) get_state returns the full session snapshot (Pi RpcSessionState).
    let state = responses.iter().find(|r| r["command"] == "get_state").expect("a get_state response");
    assert_eq!(state["id"], "3");
    assert!(state["data"]["sessionId"].is_string(), "state missing sessionId: {state}");
    assert!(state["data"]["model"]["provider"].is_string(), "state missing model: {state}");
    // The widened shape (gap #25): every Pi RpcSessionState field is present.
    for field in [
        "thinkingLevel",
        "isStreaming",
        "isCompacting",
        "steeringMode",
        "followUpMode",
        "autoCompactionEnabled",
        "messageCount",
        "pendingMessageCount",
    ] {
        assert!(!state["data"][field].is_null(), "state missing {field}: {state}");
    }

    // (4) get_commands returns the invocable-commands envelope (Pi `{commands:[...]}`), NOT the RPC
    // verb names (gap #2). The faux fixture registers no extensions/prompts/skills → an empty list.
    let cmds = responses
        .iter()
        .find(|r| r["command"] == "get_commands")
        .expect("a get_commands response");
    assert_eq!(cmds["id"], "4");
    assert!(cmds["data"]["commands"].is_array(), "get_commands must carry a commands array: {cmds}");

    // (5) the agent event stream surfaced on the protocol (at least the run start).
    let kinds: Vec<&str> = events.iter().copied().map(type_of).collect();
    assert!(kinds.contains(&"agent_start"), "no agent_start event on the stream: {kinds:?}");
}

#[tokio::test]
async fn rpc_fork_at_entry_branches_and_rebinds() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("forked answer")],
        StopReason::Stop,
    )]);
    let runtime = build_runtime(&fx, faux).await;
    let session = runtime.session().await;

    // A fork targets an entry: run one prompt so the branch has a user-message anchor to fork at.
    session
        .prompt_accepted(UserInput::text("seed the session", InputSource::Cli))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let anchor = session
        .user_messages_for_forking()
        .await
        .into_iter()
        .next()
        .expect("a forkable user-message anchor");

    // Pi `fork({entryId})`: position "before" the user message; selected text is returned so a UI
    // can re-edit it; the runtime swaps the active session and the protocol rebinds.
    let input = format!("{{\"type\":\"fork\",\"id\":\"42\",\"entryId\":\"{}\"}}\n", anchor.entry_id.as_str());
    let reader = Cursor::new(input.into_bytes());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| l["command"] == "fork").expect("a fork response");
    assert_eq!(resp["id"], "42");
    assert_eq!(resp["success"], true, "fork must succeed: {resp}");
    assert_eq!(resp["data"]["cancelled"], false, "fork was not vetoed: {resp}");
    assert_eq!(resp["data"]["text"], "seed the session", "fork returns the anchor text: {resp}");
}

#[tokio::test]
async fn rpc_unknown_command_echoes_id_on_failure() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "unknown command must fail: {resp}");
    assert_eq!(resp["id"], "9", "unknown command must echo the request id: {resp}");
}

#[tokio::test]
async fn rpc_malformed_command_echoes_id_on_failure() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // A valid JSON object with a known `type` but a missing required field (`message`).
    let input = "{\"type\":\"prompt\",\"id\":\"13\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "malformed command must fail: {resp}");
    assert_eq!(resp["id"], "13", "malformed command must echo the request id: {resp}");
    assert!(resp["error"].is_string(), "malformed command must carry an error: {resp}");
    // A malformed payload of a KNOWN type echoes the REAL command name, NOT "unknown" (#8): Pi's
    // catch-block re-emits `error(id, command.type, …)` (rpc-mode.ts:755-772).
    assert_eq!(
        resp["command"], "prompt",
        "malformed payload must echo the real command name, not `unknown`: {resp}"
    );
}

/// #6 — a line that is not valid JSON is Pi's `"parse"` error (rpc-mode.ts:728-734): `command:"parse"`,
/// **no** `id` (the id cannot be recovered — `JSON.parse` itself failed), message prefixed
/// `Failed to parse command: `.
#[tokio::test]
async fn rpc_parse_error_emits_parse_command_without_id() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // Not valid JSON (trailing brace/garbage) — cannot be parsed at all.
    let input = "{not json at all\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "parse error must fail: {resp}");
    assert_eq!(resp["command"], "parse", "parse error uses command:\"parse\": {resp}");
    assert!(resp.get("id").is_none(), "parse error must NOT carry an id: {resp}");
    let err = resp["error"].as_str().expect("error string");
    assert!(
        err.starts_with("Failed to parse command: "),
        "parse error message prefix must match Pi: {resp}"
    );
}

/// #7 — a well-formed line with an unrecognized `type` echoes the REAL type as `command` and the
/// `Unknown command: <type>` message (rpc-mode.ts:686-689), echoing the id.
#[tokio::test]
async fn rpc_unknown_command_echoes_real_type_and_message() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false);
    assert_eq!(resp["id"], "9", "unknown command echoes the id: {resp}");
    assert_eq!(
        resp["command"], "does_not_exist",
        "unknown command echoes the REAL type, not `unknown`: {resp}"
    );
    assert_eq!(
        resp["error"], "Unknown command: does_not_exist",
        "unknown command message must be `Unknown command: <type>`: {resp}"
    );
}

/// #10 — a command with a numeric `id` must EXECUTE (not just recover the id on an error path) and
/// echo the id back as-is (an opaque number; Pi types `id?: string` but a number passes through,
/// rpc-mode.ts:383).
#[tokio::test]
async fn rpc_numeric_id_executes_and_echoes_number() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // `abort` is idempotent and always succeeds; a numeric id must not trip payload validation.
    let input = "{\"type\":\"abort\",\"id\":5}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["command"], "abort", "numeric-id command must EXECUTE: {resp}");
    assert_eq!(resp["success"], true, "abort with a numeric id must succeed: {resp}");
    assert_eq!(resp["id"], 5, "numeric id must be echoed as a number, as-is: {resp}");
    assert!(resp["id"].is_number(), "id must remain a JSON number, not a string: {resp}");
}

#[tokio::test]
async fn rpc_unknown_command_is_a_failure_not_a_panic() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| type_of(l) == "response").expect("a response");
    assert_eq!(resp["success"], false, "unknown command must fail: {resp}");
    assert!(resp["error"].is_string(), "unknown command must carry an error: {resp}");
}

/// `SessionCommand` deserializes the documented snake_case `type` tags + Pi's camelCase fields
/// (`streamingBehavior`, `images`), 1:1 with `rpc-types.ts:22`.
#[test]
fn session_command_parses_streaming_behavior() {
    // The `id` is NOT a variant field (recovered from the raw line in `dispatch`, mirroring Pi's
    // `const id = command.id`); it is ignored by the command payload deserialization.
    let cmd: SessionCommand = serde_json::from_str(
        r#"{"type":"prompt","id":"7","message":"hi","streamingBehavior":"followUp"}"#,
    )
    .expect("parse prompt");
    match cmd {
        SessionCommand::Prompt { message, images, streaming_behavior } => {
            assert_eq!(message, "hi");
            assert!(images.is_empty());
            assert!(streaming_behavior.is_some());
        }
        other => panic!("expected Prompt, got {other:?}"),
    }
}

/// The camelCase request fields + new command tags deserialize 1:1 with `rpc-types.ts` (the `type`
/// discriminants are snake_case; multi-word fields are camelCase on the wire).
#[test]
fn session_command_parses_new_command_shapes() {
    // `set_model` takes `provider` + `modelId`.
    match serde_json::from_str::<SessionCommand>(
        r#"{"type":"set_model","provider":"anthropic","modelId":"claude"}"#,
    )
    .expect("parse set_model")
    {
        SessionCommand::SetModel { provider, model_id, .. } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model_id, "claude");
        }
        other => panic!("expected SetModel, got {other:?}"),
    }
    // `fork` takes `entryId`.
    match serde_json::from_str::<SessionCommand>(r#"{"type":"fork","entryId":"e1"}"#)
        .expect("parse fork")
    {
        SessionCommand::Fork { entry_id, .. } => assert_eq!(entry_id, "e1"),
        other => panic!("expected Fork, got {other:?}"),
    }
    // `bash` takes `command` + `excludeFromContext`; `set_steering_mode` the `one-at-a-time` arg.
    serde_json::from_str::<SessionCommand>(
        r#"{"type":"bash","command":"ls","excludeFromContext":true}"#,
    )
    .expect("parse bash");
    serde_json::from_str::<SessionCommand>(r#"{"type":"set_steering_mode","mode":"one-at-a-time"}"#)
        .expect("parse set_steering_mode");
    serde_json::from_str::<SessionCommand>(r#"{"type":"new_session","parentSession":"p.jsonl"}"#)
        .expect("parse new_session");
}

/// The extended command surface drives the runtime/session: model listing, thinking, queue modes,
/// stats, a session-name round-trip, a new_session swap, and the corrected `get_commands` envelope.
#[tokio::test]
async fn rpc_extended_command_surface() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = concat!(
        r#"{"type":"get_available_models","id":"a"}"#,
        "\n",
        r#"{"type":"cycle_model","id":"b"}"#,
        "\n",
        r#"{"type":"set_thinking_level","id":"c","level":"medium"}"#,
        "\n",
        r#"{"type":"set_steering_mode","id":"d","mode":"one-at-a-time"}"#,
        "\n",
        r#"{"type":"get_session_stats","id":"e"}"#,
        "\n",
        r#"{"type":"set_session_name","id":"f","name":"  my session  "}"#,
        "\n",
        r#"{"type":"set_session_name","id":"g","name":"   "}"#,
        "\n",
        r#"{"type":"get_messages","id":"h"}"#,
        "\n",
        r#"{"type":"new_session","id":"i"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = |cmd: &str| lines.iter().find(|l| l["command"] == cmd).unwrap_or_else(|| panic!("no {cmd} response"));

    // get_available_models → {models:[...]}.
    assert_eq!(resp("get_available_models")["success"], true);
    assert!(resp("get_available_models")["data"]["models"].is_array());

    // cycle_model with a single faux model → success with null (≤1 candidate).
    assert_eq!(resp("cycle_model")["success"], true);

    // set_thinking_level clamps + succeeds; set_steering_mode succeeds.
    assert_eq!(resp("set_thinking_level")["success"], true);
    assert_eq!(resp("set_steering_mode")["success"], true);

    // get_session_stats carries the aggregate counters.
    let stats = resp("get_session_stats");
    assert_eq!(stats["success"], true);
    assert!(stats["data"]["messageCount"].is_number(), "stats missing messageCount: {stats}");

    // set_session_name trims + persists ("f"), and rejects an empty/whitespace name ("g").
    let named: Vec<&Value> = lines.iter().filter(|l| l["command"] == "set_session_name").collect();
    let ok = named.iter().find(|r| r["id"] == "f").expect("named ok response");
    assert_eq!(ok["success"], true, "trim+persist must succeed: {ok}");
    let empty = named.iter().find(|r| r["id"] == "g").expect("named empty response");
    assert_eq!(empty["success"], false, "empty name must be rejected: {empty}");

    // get_messages now returns the {messages:[...]} envelope (gap #45).
    assert!(resp("get_messages")["data"]["messages"].is_array(), "get_messages envelope");

    // new_session swaps the active session and is not vetoed.
    let new = resp("new_session");
    assert_eq!(new["success"], true);
    assert_eq!(new["data"]["cancelled"], false, "new_session not vetoed: {new}");
}
