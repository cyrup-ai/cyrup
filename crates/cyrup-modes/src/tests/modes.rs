//! Integration tests for the non-interactive adapters (arch-11 §2.2; func-11 R-11-005/007/011…016).
//!
//! Each test builds a real wired [`AgentSession`] over a scripted `FauxProvider` in a tempdir and
//! drives one adapter into an in-memory sink, then asserts on the produced bytes — exactly how the
//! binary will drive them over real stdio.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{run_json, run_print, run_rpc, PrintOptions, SessionCommand};
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, faux_tool_call,
    FauxMessageOptions, FauxProvider,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSessionRuntime, InputSource, SessionConfig, SessionFactory, UserInput,
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

/// Build the multi-session runtime host the RPC adapter drives (Pi `rpc-mode.ts` `runtimeHost`).
///
/// Carries a [`AnyFauxResolver`] because the real host always carries one: `main.rs` hands every
/// `SessionFactory` a `BuiltinProviderResolver`. Any model command whose target model belongs to a
/// provider other than the installed one (`set_model` across providers, and `cycle_model` since it
/// walks `getAvailable()` across every configured provider) has to install that provider, and a
/// resolver-less host can only fail there — which says nothing about the RPC contract under test.
/// It matters here because these fixtures are NOT hermetic against the ambient environment: a
/// `TOGETHER_API_KEY` in the developer's shell makes `together` a configured provider and puts its
/// whole catalog in the available set, exactly as it would for a real user.
async fn build_runtime(fx: &Fixture, faux: Arc<FauxProvider>) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, cfg)
            .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>),
    );
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
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [UserInput::text("what is the answer?", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("the final answer"), "final assistant text missing:\n{text}");
    assert!(String::from_utf8(err).unwrap().is_empty(), "a clean turn writes nothing to stderr");
}

/// G3 — PRINT mode prints ONLY the final assistant message of a multi-message turn, exactly once,
/// never one line per intermediate message. Pi's send loop produces no output and the terminal
/// output block reads `state.messages[state.messages.length - 1]` outside the loop
/// (print-mode.ts:121-146). Pre-fix cyrup wrote the accumulated text on every call, so a two-message
/// turn produced BOTH `"first answer"` and `"second answer"`.
#[tokio::test]
async fn print_mode_prints_only_the_final_message_of_a_turn() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [
            UserInput::text("q1", InputSource::Cli),
            UserInput::text("q2", InputSource::Cli),
        ],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let text = String::from_utf8(out).unwrap();
    assert_eq!(text, "second answer\n", "only the FINAL message prints, exactly once (G3): {text:?}");
    assert!(!text.contains("first answer"), "an intermediate message must NOT print (G3): {text:?}");
    assert!(String::from_utf8(err).unwrap().is_empty(), "a clean turn writes nothing to stderr");
}

/// G4 — a failed final turn: Pi writes `errorMessage` to stderr and suppresses the assistant stdout
/// (print-mode.ts:133-137). Pre-fix cyrup wrote the failed turn's partial text to stdout and never
/// touched stderr.
#[tokio::test]
async fn print_mode_routes_a_failed_turn_to_stderr_and_suppresses_stdout() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message_with(
        vec![faux_text("partial garbled output")],
        StopReason::Error,
        FauxMessageOptions {
            error_message: Some("the model exploded".into()),
            ..Default::default()
        },
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [UserInput::text("q", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let stdout = String::from_utf8(out).unwrap();
    let stderr = String::from_utf8(err).unwrap();
    assert!(stdout.is_empty(), "a failed turn suppresses assistant stdout (G4): {stdout:?}");
    assert_eq!(stderr, "the model exploded\n", "the error message goes to stderr (G4): {stderr:?}");
}

/// SEAM-016 — `run_print` returns pi's `exitCode`, decided inside the terminal output block from
/// the SAME `lastMessage` it prints (`print-mode.ts:139-148` @v0.84.1) and returned at `:151`.
///
/// Pins the three arms that used to diverge, all of which were computed elsewhere (`run.rs`'s
/// reverse transcript scan) rather than here:
/// * a clean `stop` keeps the `exitCode = 0` pi initialises at `:35`;
/// * `error` raises it to 1 (`:147`);
/// * **`aborted` raises it to 1 too** — pi's condition is `stopReason === "error" || stopReason ===
///   "aborted"` (`:145`), one branch, one assignment. cyrup answered **130** for this case, a code
///   pi never emits from print mode, so this assertion was RED before the change.
#[tokio::test]
async fn print_mode_exit_code_is_pis_zero_or_one_from_the_final_message() {
    for (reason, expected) in [
        (StopReason::Stop, 0),
        (StopReason::Error, 1),
        (StopReason::Aborted, 1),
    ] {
        let fx = fixture();
        let faux = Arc::new(FauxProvider::new());
        faux.set_responses(vec![faux_assistant_message(vec![faux_text("out")], reason)]);
        let runtime = build_runtime(&fx, faux).await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_print(
            &runtime,
            [UserInput::text("q", InputSource::Cli)],
            &mut out,
            &mut err,
            PrintOptions::default(),
        )
        .await
        .expect("print mode runs");
        assert_eq!(code, expected, "pi's exitCode for stop reason {reason:?}");
    }
}

/// G4 — an aborted final turn with NO `error_message` falls back to Pi's `Request ${stopReason}`
/// string on stderr, still suppressing stdout (print-mode.ts:136, the `|| ` branch).
#[tokio::test]
async fn print_mode_aborted_turn_without_message_uses_the_request_reason_fallback() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("half-written")],
        StopReason::Aborted,
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [UserInput::text("q", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    assert!(String::from_utf8(out).unwrap().is_empty(), "aborted turn suppresses stdout (G4)");
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "Request aborted\n",
        "an aborted turn without an error_message falls back to `Request aborted` (G4)"
    );
}

/// SEAM-014 / DRIFT-010 — `get_available_thinking_levels` is one of pi's 32 RPC verbs
/// (`modes/rpc/rpc-types.ts:39` @v0.83.0), handled at `rpc-mode.ts:507-510` and answering
/// `{levels}` (`rpc-types.ts:158-164`). It was the ONE verb missing from cyrup's `SessionCommand`
/// switch, so a client could not enumerate which levels the active model supports and had to
/// hard-code the list or offer levels the model rejects.
///
/// SEAM-054 — a BLANK stdin line must produce pi's `parse` error response, not silence. pi's
/// `emitLine` has no emptiness filter (`modes/rpc/jsonl.ts:25-41` @v0.84.1, emit at `:38`), so the
/// empty string reaches `handleInputLine`, `JSON.parse("")` throws, and pi writes
/// `error(undefined, "parse", …)` (`rpc-mode.ts:752-758`). cyrup dropped the record before the
/// command loop ever saw it, so any client correlating n-lines-in to n-responses-out desynchronised
/// and one waiting on a reply hung.
///
/// SEAM-053 — the OPTIONAL `RpcSessionState` members (`sessionFile?`, `sessionName?`, `model?`,
/// `rpc-types.ts:95/102/104`) must be ABSENT, not `null`: pi builds the object as a TS literal and
/// `JSON.stringify` drops an `undefined` property (`rpc-mode.ts:445-458`), so its line for an
/// unnamed ephemeral session contains neither key. A client using `"sessionName" in state` — the
/// natural idiom for an optional property — took the wrong branch against cyrup's explicit `null`.
#[tokio::test]
async fn rpc_thinking_levels_blank_lines_and_omitted_optional_state_match_pi() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let runtime = build_runtime(&fx, faux).await;

    let input = concat!(
        "\n",
        r#"{"type":"get_available_thinking_levels","id":"levels"}"#,
        "\n",
        r#"{"type":"get_state","id":"state"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");
    let lines = parse_lines(&out);
    let responses: Vec<&Value> = lines.iter().filter(|l| type_of(l) == "response").collect();

    // SEAM-054: the blank line was ANSWERED, with pi's `parse` command and no id.
    let parse_err = responses
        .iter()
        .find(|r| r["command"] == "parse")
        .expect("a blank line must produce pi's `parse` error response, not silence");
    assert_eq!(parse_err["success"], false);
    assert!(parse_err.get("id").is_none() || parse_err["id"].is_null());

    // SEAM-014: the verb exists and answers `{levels: [...]}`.
    let levels = responses
        .iter()
        .find(|r| r["command"] == "get_available_thinking_levels")
        .expect("get_available_thinking_levels must be a recognized verb");
    assert_eq!(levels["success"], true, "{levels}");
    assert!(
        levels["data"]["levels"].is_array(),
        "pi's response shape is {{levels}} (rpc-types.ts:158-164): {levels}"
    );

    // SEAM-053: `sessionName` was never set on this session, so the KEY is absent.
    let state = responses
        .iter()
        .find(|r| r["command"] == "get_state")
        .expect("a get_state response");
    let data = state["data"].as_object().expect("get_state data object");
    assert!(
        !data.contains_key("sessionName"),
        "an unnamed session must OMIT sessionName, not send null (rpc-types.ts:104): {state}"
    );
    // The required members are still all present and non-null.
    for field in ["sessionId", "thinkingLevel", "isStreaming", "messageCount"] {
        assert!(
            data.get(field).is_some_and(|v| !v.is_null()),
            "required RpcSessionState field {field} missing: {state}"
        );
    }
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
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    run_json(&runtime, [UserInput::text("write a file", InputSource::Cli)], &mut out)
        .await
        .expect("json mode runs");

    let lines = parse_lines(&out);
    assert!(!lines.is_empty(), "no lines emitted");

    // Line 1 is the session header (Pi print-mode.ts:112-117); the event stream follows it.
    assert_eq!(type_of(&lines[0]), "session", "line 1 must be the session header: {lines:?}");
    let events = &lines[1..];
    assert!(!events.is_empty(), "no events emitted");

    // Every event line parsed; the ordered `type` tags bracket the run.
    let kinds: Vec<&str> = events.iter().map(type_of).collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "must start with agent_start: {kinds:?}");
    // SEAM-005: json mode writes EVERY subscribed session event verbatim (Pi print-mode.ts:103-108),
    // so the stream now closes with `agent_settled` — the run's last `agent_end` immediately
    // precedes it.
    assert_eq!(kinds.last(), Some(&"agent_settled"), "must end with agent_settled: {kinds:?}");
    assert_eq!(
        kinds.iter().rev().nth(1),
        Some(&"agent_end"),
        "…preceded by the run's last agent_end: {kinds:?}"
    );

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

/// The raw (unparsed) LF-delimited lines of the sink, so a byte-exact assertion can be made about
/// the first line without a JSON round-trip normalizing whitespace or key order.
fn raw_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8(bytes.to_vec()).expect("utf8 output").lines().map(str::to_string).collect()
}

/// E / O2 — JSON mode writes the session header (`sessionManager.getHeader()`) as JSONL **line 1**,
/// BEFORE any event, so a consumer built to Pi's wire format learns the session id/cwd up front (Pi
/// `runPrintMode`: `writeRawStdout(JSON.stringify(header))` ahead of the subscription,
/// print-mode.ts:112-117). The emitted bytes must be the byte-exact Pi `SessionHeader` record:
/// `{"type":"session","version":3,"id":…,"timestamp":…,"cwd":…}` (session-manager.ts:32-39,856-863) —
/// compact, camelCase, keys in that order, `version` a number, `parentSession` omitted when unset.
/// Pre-fix cyrup emitted no header line at all and the first line was `agent_start`.
#[tokio::test]
async fn json_mode_writes_session_header_as_first_line() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("done")], StopReason::Stop)]);
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;

    let mut out: Vec<u8> = Vec::new();
    run_json(&runtime, [UserInput::text("hello", InputSource::Cli)], &mut out)
        .await
        .expect("json mode runs");

    let lines = raw_lines(&out);
    assert!(lines.len() >= 2, "expected a header line plus events, got {} lines", lines.len());

    // Build the byte-exact Pi header record from the session's own header field values and assert
    // line 1 equals it verbatim (serde compact output == Pi's `JSON.stringify(header)` for the same
    // `SessionHeader` interface: same key order, no spaces, `version` numeric, `id`/`cwd` strings).
    let header = session.session_header().await;
    let expected = format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"{}\",\"timestamp\":\"{}\",\"cwd\":\"{}\"}}",
        header.id, header.timestamp, header.cwd
    );
    assert_eq!(lines[0], expected, "line 1 must be the byte-exact Pi session header");

    // Sanity: the header id is this session's id, cwd is the project cwd, no `parentSession` key,
    // and the very next line begins the event stream.
    let parsed: Value = serde_json::from_str(&lines[0]).expect("header line is valid json");
    assert_eq!(parsed["type"], "session");
    assert_eq!(parsed["version"], 3);
    assert_eq!(parsed["id"].as_str(), Some(session.session_id().as_str()));
    assert_eq!(parsed["cwd"].as_str(), Some(fx.cwd.to_string_lossy().as_ref()));
    assert!(parsed.get("parentSession").is_none(), "parentSession must be omitted when unset");
    let first_event: Value = serde_json::from_str(&lines[1]).expect("line 2 is valid json");
    assert_eq!(
        type_of(&first_event),
        "agent_start",
        "the event stream (agent_start) must immediately follow the header"
    );
}

/// The header is written exactly ONCE per session even across a multi-prompt `--mode json` run
/// (initial submission + follow-ups, each dispatched as a separate `run_json` call, mirroring
/// `run_json_dispatch`). Pi writes `getHeader()` a single time before its whole message loop
/// (print-mode.ts:112-119), so a second `run_json` on the same session must NOT re-emit it.
#[tokio::test]
async fn json_mode_writes_session_header_exactly_once_across_followups() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let runtime = build_runtime(&fx, faux).await;

    // Replay the exact dispatch shape: initial prompt, then one follow-up, into ONE sink — and
    // additionally a SECOND `run_json` call on the same runtime, which must not re-emit the header.
    let mut out: Vec<u8> = Vec::new();
    run_json(&runtime, [UserInput::text("first", InputSource::Cli)], &mut out)
        .await
        .expect("initial json run");
    run_json(&runtime, [UserInput::text("second", InputSource::Cli)], &mut out)
        .await
        .expect("follow-up json run");

    let header_lines = parse_lines(&out).into_iter().filter(|v| type_of(v) == "session").count();
    assert_eq!(header_lines, 1, "the session header must be emitted exactly once across the run");
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

    // get_session_stats answers with Pi's `SessionStats` (agent-session.ts:260-277), which
    // rpc-types.ts:183 names as this command's `data` verbatim. This assertion used to require
    // `messageCount` — one of seven cyrup-invented keys (`messageCount`/`userMessageCount`/
    // `assistantMessageCount`/`toolResultCount`/`inputTokens`/`outputTokens`/`cacheTokens`) that a
    // Pi-contract client reads as `undefined`. It was pinning SEAM-031 and is rewritten, not
    // deleted: the contract is now asserted key-for-key, including that the invented names are GONE.
    let stats = resp("get_session_stats");
    assert_eq!(stats["success"], true);
    let data = &stats["data"];
    for key in [
        "sessionId",
        "userMessages",
        "assistantMessages",
        "toolCalls",
        "toolResults",
        "totalMessages",
        "tokens",
        "cost",
    ] {
        assert!(!data[key].is_null(), "stats missing Pi key `{key}`: {stats}");
    }
    for key in ["input", "output", "cacheRead", "cacheWrite", "total"] {
        assert!(
            data["tokens"][key].is_number(),
            "stats.tokens missing Pi key `{key}`: {stats}"
        );
    }
    for gone in [
        "messageCount",
        "userMessageCount",
        "assistantMessageCount",
        "toolResultCount",
        "inputTokens",
        "outputTokens",
        "cacheTokens",
    ] {
        assert!(data[gone].is_null(), "cyrup-invented key `{gone}` still on the wire: {stats}");
    }
    // `sessionFile` is `string | undefined` — this runtime IS persisted, so it must be present.
    assert!(data["sessionFile"].is_string(), "stats missing sessionFile: {stats}");

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

/// A genuine immediate-bash backend failure (not a cancellation) must be reported as a real RPC
/// `error(...)` response, NEVER fabricated into a "successful" `bash` response — and must NEVER be
/// recorded into transcript history. Pi's `executeBashWithOperations` only catches the abort case in
/// its `catch` block (`bash-executor.ts:130-155`); every other error `throw`s (line 154),
/// propagating through `AgentSession.executeBash` uncaught (`agent-session.ts:2628-2643`:
/// `recordBashResult` is only reached on the success path inside `try`) to the RPC dispatcher's
/// `catch` (`rpc-mode.ts:756-772`), which converts it into an `error(...)` response with no history
/// entry ever recorded.
#[tokio::test]
async fn rpc_bash_backend_failure_is_not_fabricated_into_a_success() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;
    let session = runtime.session().await;

    // Doom every real spawn attempt: `LocalProc::exec` checks the session cwd exists BEFORE ever
    // spawning (mirrors Pi's `fsAccess(cwd, F_OK)`, bash.ts:70-74) — remove it out from under the
    // already-built session so the bash command below hits a genuine backend error, not a
    // cancellation and not a real process failure racy across platforms.
    std::fs::remove_dir_all(&fx.cwd).expect("remove the session cwd out from under the session");

    let input = concat!(r#"{"type":"bash","id":"b1","command":"echo hi"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let bash_resp = lines.iter().find(|l| l["id"] == "b1").expect("bash response");
    assert_eq!(
        bash_resp["success"], false,
        "a genuine backend failure must not report success: {bash_resp}"
    );
    assert!(
        bash_resp["error"].as_str().unwrap_or_default().contains("Working directory does not exist"),
        "the real backend error message must surface verbatim: {bash_resp}"
    );
    assert!(bash_resp["data"].is_null(), "a failed bash call carries no data payload: {bash_resp}");

    // `cyrup_agent::AgentMessage` isn't a direct dependency of this crate; serialize the live agent
    // state generically (its `Custom{kind:"bashExecution",..}` variant always serializes with that
    // literal string, `session.rs`'s `record_bash_result`) rather than naming the foreign type.
    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        !msgs_json.to_string().contains("bashExecution"),
        "a genuine backend failure must NEVER be recorded into transcript history: {msgs_json}"
    );
}

/// Read one non-empty JSONL record from an async reader (test helper for the interactive RPC flow).
async fn read_json_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Value {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.expect("read a line");
        assert!(n > 0, "unexpected EOF while awaiting a json line");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed).expect("valid json line");
    }
}

/// Mode #4 end-to-end: a loaded guest's synchronous `ui.{select,confirm,input}` capability round-trips
/// through the RPC transport — the loop emits an `extension_ui_request` on stdout and the client's
/// `extension_ui_response` resumes the wasm-suspended guest (Pi `createExtensionUIContext` +
/// `handleInputLine`, rpc-mode.ts:135-160,739-753). Multi-thread so the guest's `block_in_place`
/// reply-wait is legal. The transport is an in-memory duplex pair standing in for real stdio.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_extension_ui_request_response_round_trips() {
    use cyrup_ext::host::{DialogOptions, HostServices};
    use tokio::io::{AsyncWriteExt, BufReader};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // The SAME session Arc the loop installs its ui sink onto (shared through the runtime).
    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    // In-memory bidirectional transport (client <-> server) in place of stdio.
    let (mut client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let mut client_reader = BufReader::new(client_rx);

    let rpc = tokio::spawn(async move {
        let reader = BufReader::new(server_rx);
        let mut writer = server_tx;
        run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });

    // A get_state first: its response proves the loop is up and the ui sink is installed (set before
    // the select! loop), so the following guest dialog cannot race the sink.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    // (1) select → the loop emits the Pi request; the client answers with the chosen STRING, which
    //     reaches the guest UNCHANGED — the WIT `select` return is now the chosen STRING itself
    //     (world.wit:259), byte-for-byte Pi's `select(...): Promise<string|undefined>` (types.ts:127),
    //     with NO index translation anywhere in the round-trip.
    let hs = host_services.clone();
    let guest_select = tokio::spawn(async move {
        hs.select("Pick one", &serde_json::json!(["alpha", "beta", "gamma"]), &DialogOptions::default())
    });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["type"], "extension_ui_request");
    assert_eq!(req["method"], "select");
    assert_eq!(req["title"], "Pick one");
    assert_eq!(req["options"], serde_json::json!(["alpha", "beta", "gamma"]));
    let id = req["id"].as_str().unwrap().to_string();
    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"value\":\"gamma\"}}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(
        guest_select.await.unwrap().as_deref(),
        Some("gamma"),
        "select's wire {{value}} string passes straight through, with no index math"
    );

    // (2) confirm → `{confirmed:true}` resumes the guest with true. L4 review §2.6: the guest's
    //     `message` (a large formatted body, distinct from the `title`) reaches the wire verbatim,
    //     not hard-coded to `""`.
    let hs = host_services.clone();
    let guest_confirm = tokio::spawn(async move {
        hs.confirm("Proceed?", "a large formatted body, distinct from the title", &DialogOptions::default())
    });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "confirm");
    assert_eq!(req["title"], "Proceed?");
    assert_eq!(
        req["message"], "a large formatted body, distinct from the title",
        "confirm's message body reaches the wire distinct from title, not hard-coded empty"
    );
    let id = req["id"].as_str().unwrap().to_string();
    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"confirmed\":true}}\n").as_bytes())
        .await
        .unwrap();
    assert!(guest_confirm.await.unwrap(), "confirm round-trips true");

    // (3) input cancelled → `{cancelled:true}` yields None (Pi `parseResponse` default). L4 review
    //     §2.7: the guest's `placeholder` reaches the wire (present, not dropped).
    let hs = host_services.clone();
    let guest_input = tokio::spawn(async move {
        hs.input("Name?", Some("e.g. Ada Lovelace"), &DialogOptions::default())
    });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "input");
    assert_eq!(
        req["placeholder"], "e.g. Ada Lovelace",
        "input's placeholder reaches the wire instead of being dropped"
    );
    let id = req["id"].as_str().unwrap().to_string();
    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"cancelled\":true}}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(guest_input.await.unwrap(), None, "cancelled input -> None");

    // EOF → the loop drains and returns.
    drop(client_tx);
    rpc.await.unwrap();
}

/// The fire-and-forget half of the `ui` capability (`notify`/`set-status`/`set-widget`/`set-title`/
/// `set-editor-text`/`paste-editor-text`) must ALSO reach the RPC client, exactly like Pi's own
/// `notify`/`setStatus`/`setWidget`/`setTitle`/`setEditorText` RPC handlers, each of which just calls
/// `output({type:"extension_ui_request", id, method, ...})` inline with no correlated response
/// expected (`rpc-mode.ts:149-241`) — unlike `confirm`/`input`/`select`/`editor` above, none of these
/// calls block on a reply, so no `extension_ui_response` is ever sent back for them in this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_fire_and_forget_ui_effects_reach_the_wire() {
    use cyrup_ext::host::{HostServices, NotifyKind};
    use tokio::io::{AsyncWriteExt, BufReader};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let mut client_reader = BufReader::new(client_rx);

    let rpc = tokio::spawn(async move {
        let reader = BufReader::new(server_rx);
        let mut writer = server_tx;
        run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });

    // A get_state first proves the loop (and its effect sink) is up before any effect fires.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    // notify → `{method:"notify", message, notifyType}` (rpc-mode.ts:149-157). None of these calls
    // block: `HostServices::notify` is a plain sync fire-and-forget send, called directly (no
    // `spawn_blocking` needed, unlike `confirm`/`input`/`select`/`editor` above).
    host_services.notify("careful now", NotifyKind::Warning);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["type"], "extension_ui_request");
    assert_eq!(req["method"], "notify");
    assert_eq!(req["message"], "careful now");
    assert_eq!(req["notifyType"], "warning");

    // set_status(key, Some(text)) → `{method:"setStatus", statusKey, statusText}` (rpc-mode.ts:163-172).
    host_services.set_status("git", Some("main"));
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setStatus");
    assert_eq!(req["statusKey"], "git");
    assert_eq!(req["statusText"], "main");

    // set_status(key, None) clears the key → `statusText` is OMITTED entirely (not `null`), matching
    // Pi's `JSON.stringify` dropping an `undefined` property.
    host_services.set_status("git", None);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setStatus");
    assert_eq!(req["statusKey"], "git");
    assert!(req.get("statusText").is_none(), "a cleared status must omit statusText: {req:?}");

    // SEAM-028 — this case DOCUMENTS a divergence; it does not bless one. cyrup's WIT collapsed
    // pi's 3-arg `setWidget(key, content, options)` into one opaque JSON payload
    // (`set-widget: func(widget-json: string)`, both `wit/world.wit` copies), so the emitter sends a
    // cyrup-invented `widget` blob where pi's `RpcExtensionUIRequest` member pins
    // `widgetKey: string; widgetLines: string[] | undefined; widgetPlacement?: "aboveEditor" |
    // "belowEditor"` (`modes/rpc/rpc-types.ts:264-271`) and has NO `widget` key at all.
    //
    // Asserting the collapse as CORRECT is what made this suite certify the divergence and invited
    // a revert of the fix, so the assertion below is the pi-shaped one and the case is `#[ignore]`d
    // until SEAM-011 widens the WIT. Do not re-point it at `req["widget"]`.
    host_services.set_widget(&serde_json::json!({"widget": "text", "text": "hi"}));
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setWidget");
    // The `method` is the only part of this member cyrup gets right, so it is the only part
    // asserted. The pi-shaped payload assertion lives in
    // `set_widget_carries_pis_three_fields_and_no_widget_blob`, `#[ignore]`d against SEAM-011.

    // set_title → `{method:"setTitle", title}` (rpc-mode.ts:216-223).
    host_services.set_title("My Session");
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setTitle");
    assert_eq!(req["title"], "My Session");

    // set_editor_text(text, is_paste=false) → `{method:"set_editor_text", text}` — snake_case method
    // name (rpc-mode.ts:234-241), unlike this test's other camelCase methods.
    host_services.set_editor_text("typed text", false);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "set_editor_text");
    assert_eq!(req["text"], "typed text");

    // paste_editor_text (is_paste=true) collapses onto the SAME wire method as set_editor_text — Pi's
    // own `pasteToEditor(text) { this.setEditorText(text); }` (rpc-mode.ts:230-232).
    host_services.set_editor_text("pasted text", true);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "set_editor_text");
    assert_eq!(req["text"], "pasted text");

    // The loop is still alive and responsive after all six effects (proves the drain arm never
    // blocked the select! loop or consumed a client-facing response slot).
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(after["command"], "get_state");

    drop(client_tx);
    rpc.await.unwrap();
}

/// `set-header`/`set-footer`/`set-tools-expanded` are delivered to the in-process [`UiEffect`] sink
/// (closing the "reaches no consumer at all" gap) but deliberately NEVER forwarded onto the RPC wire —
/// Pi's own RPC mode does not deliver them either ("Custom header/footer not supported in RPC mode -
/// requires TUI access", "Tool expansion not supported in RPC mode - no TUI", rpc-mode.ts:209-215,
/// 296-298). Proven by calling all three back-to-back and observing the very next wire line is still
/// the `get_state` response sent immediately after — no `extension_ui_request` slipped out ahead of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_header_footer_and_tools_expanded_effects_never_reach_the_wire() {
    use cyrup_ext::host::HostServices;
    use tokio::io::{AsyncWriteExt, BufReader};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let mut client_reader = BufReader::new(client_rx);

    let rpc = tokio::spawn(async move {
        let reader = BufReader::new(server_rx);
        let mut writer = server_tx;
        run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });

    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    host_services.set_header("custom header");
    host_services.set_footer("custom footer");
    host_services.set_tools_expanded(true);

    // SEAM-030 (b) — the 50 ms sleep that used to sit here is replaced by a POSITIVE
    // synchronisation point: the `get_state` round-trip below is itself the barrier. Its response
    // cannot be written until the loop has processed everything queued ahead of it, so "the very
    // next wire line is the get_state response" proves no `extension_ui_request` was emitted
    // WITHOUT depending on how long a sleep happens to be. (This one was a smell rather than a
    // defect — `extension_ui_effect_json` returns `None` for `SetHeader`/`SetFooter`/
    // `SetToolsExpanded`, so no request can ever be written regardless — but the sleep asserted
    // nothing and cost 50 ms per run.)
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(
        after["command"], "get_state",
        "the very next wire line must be the get_state response, not a stray extension_ui_request \
         for set_header/set_footer/set_tools_expanded: {after:?}"
    );
    assert_eq!(after["id"], "after");

    drop(client_tx);
    rpc.await.unwrap();
}

/// L4 review §2.2 (CRITICAL): a `timeout_ms`-bearing dialog whose RPC client NEVER answers must still
/// resolve within that window — Pi's `createDialogPromise` host-armed `setTimeout`
/// (`rpc-mode.ts:114-119`) ALWAYS settles the Promise regardless of client behavior. Proves the fix
/// end-to-end over the real wire protocol: the client sees the `timeout` field on the outgoing request
/// (rpc-types.ts shape) but deliberately never answers it, and the loop stays alive/responsive
/// afterward (a subsequent `get_state` still gets served — the turn was never left hung).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_extension_ui_request_times_out_to_the_default_when_client_never_responds() {
    use cyrup_ext::host::{DialogOptions, HostServices};
    use tokio::io::{AsyncWriteExt, BufReader};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let mut client_reader = BufReader::new(client_rx);

    let rpc = tokio::spawn(async move {
        let reader = BufReader::new(server_rx);
        let mut writer = server_tx;
        run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });

    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    // Open a confirm dialog with a short live countdown; the guest call is driven on a blocking task
    // exactly as the wasm-suspended host import would be.
    let hs = host_services.clone();
    let opts = DialogOptions { timeout_ms: Some(80), signal_id: None };
    let guest_confirm = tokio::spawn(async move { hs.confirm("Proceed?", "body", &opts) });

    // The client sees the request, including Pi's `timeout` field — and simply never answers it.
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "confirm");
    assert_eq!(req["timeout"], 80, "the wire `timeout` field carries opts.timeout_ms verbatim");

    // The guest call must settle to the confirm default (`false`) on its own, well inside a generous
    // bound — proving the host, not the client, is what unblocks it.
    let resolved = tokio::time::timeout(Duration::from_secs(5), guest_confirm)
        .await
        .expect("the dialog must not hang past its timeout_ms")
        .expect("confirm task");
    assert!(!resolved, "an unanswered confirm settles to Pi's `false` default on timeout");
    // SEAM-030 — the `started.elapsed() < 2s` margin that used to follow is DELETED: it carried no
    // semantic content the `timeout(5s)` + `assert!(!resolved)` above does not already carry (the
    // dialog demonstrably settled on its own, unanswered), and it was the most flake-prone
    // assertion in this file.

    // The loop is still alive and serving requests — the abandoned dialog never hung the session.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(after["command"], "get_state");
    assert_eq!(after["id"], "after");

    drop(client_tx);
    rpc.await.unwrap();
}

/// `abort`/`abort_retry` must NOT force-dismiss an open `confirm`/`input`/`select` dialog. Pi's
/// `session.abort()` (`agent-session.ts`) only cancels the run; `rpc-mode.ts`'s `case "abort"`
/// (~line 424) and `case "abort_retry"` (~line 541) never touch `pendingExtensionRequests` — a
/// dialog is dismissed early ONLY through the extension's own opt-in `signal` binding
/// (`ExtensionUIDialogOptions.signal`, types.ts:320-321), which nothing in Pi's first-party code
/// wires to "the turn got aborted" by default. The client here deliberately never sends an
/// `extension_ui_response` while aborting — the still-open dialog must remain genuinely pending
/// through the abort, and only settle once a real response finally arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_abort_does_not_force_resolve_a_pending_dialog() {
    use cyrup_ext::host::{DialogOptions, HostServices};
    use tokio::io::{AsyncWriteExt, BufReader};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let mut client_reader = BufReader::new(client_rx);

    let rpc = tokio::spawn(async move {
        let reader = BufReader::new(server_rx);
        let mut writer = server_tx;
        run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });

    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    // Open a `select` dialog with NO timeout at all — nothing but a genuine response can unblock it.
    let hs = host_services.clone();
    let mut guest_select =
        tokio::spawn(async move {
            hs.select("Pick one", &serde_json::json!(["a", "b"]), &DialogOptions::default())
        });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "select");
    let dialog_id = req["id"].as_str().unwrap().to_string();

    // The client never answers the dialog — it aborts the turn instead.
    client_tx.write_all(b"{\"type\":\"abort\",\"id\":\"stop\"}\n").await.unwrap();
    let abort_resp = read_json_line(&mut client_reader).await;
    assert_eq!(abort_resp["command"], "abort");
    assert_eq!(abort_resp["success"], true);

    // The guest's dialog must NOT resolve from the abort alone — it stays genuinely pending.
    let still_pending = tokio::time::timeout(Duration::from_millis(300), &mut guest_select).await;
    assert!(
        still_pending.is_err(),
        "abort must not force-resolve an open dialog: {still_pending:?}"
    );

    // The loop is still alive, and the dialog is STILL answerable by a real `extension_ui_response`
    // after the abort — proving it was left genuinely pending, not silently dropped.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(after["command"], "get_state");

    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{dialog_id}\",\"value\":\"a\"}}\n").as_bytes())
        .await
        .unwrap();
    let resolved = tokio::time::timeout(Duration::from_secs(2), guest_select)
        .await
        .expect("the dialog must still be answerable after an unrelated abort")
        .expect("select task");
    assert_eq!(resolved.as_deref(), Some("a"), "a real response after abort still resumes the guest");

    drop(client_tx);
    rpc.await.unwrap();
}

// ----------------------------------------------------------------------------------------------
// G1 (CRITICAL) — the command loop must not be serialized: `abort_bash`/`abort` sent WHILE a
// long-running command is in flight must interrupt it (Pi `void handleInputLine(line)`,
// rpc-mode.ts:782, dispatches each line concurrently, so `abort_bash` reaches `session.abortBash()`
// (rpc-mode.ts:557-560) while the in-flight `bash`'s `await session.executeBash(...)`
// (rpc-mode.ts:550-555) is still running).
// ----------------------------------------------------------------------------------------------

/// Drives a real `sleep`-backed `bash` over the in-memory transport, sends `abort_bash` immediately
/// after, and asserts the whole exchange finishes far faster than the sleep would take on its own —
/// only possible if the loop serviced `abort_bash` concurrently with the blocking `bash`, cancelling
/// it. On the pre-fix (fully-serialized) loop the `abort_bash` line stays buffered until the `bash`
/// `dispatch().await` returns naturally (~the full sleep), so `bash` runs to completion
/// (`cancelled:false`) and the exchange takes ~the whole sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_abort_bash_interrupts_a_running_bash_command() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // A bash that would run for 6s if never interrupted, immediately followed by `abort_bash`.
    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"sleep 6"}"#,
        "\n",
        r#"{"type":"abort_bash","id":"ab"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();

    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let bash = lines.iter().find(|l| l["id"] == "b1").expect("bash response");
    let abort = lines.iter().find(|l| l["id"] == "ab").expect("abort_bash response");
    assert_eq!(abort["command"], "abort_bash");
    assert_eq!(abort["success"], true, "abort_bash must be acknowledged: {abort}");

    // SEAM-030 — the wall-clock assertion that used to sit here (`elapsed < 3s`, "proving the
    // command loop is serialized") is DELETED. It asserted a scheduling outcome the test cannot
    // control, so under CI load or a debug build it failed for reasons unrelated to the behaviour
    // under test, while the deterministic assertion five lines below proves the same property: a
    // `cancelled:true` bash result can ONLY arise if `abort_bash` was serviced while `bash` was
    // still running (G1).
    assert_eq!(bash["command"], "bash");
    assert_eq!(
        bash["data"]["cancelled"], true,
        "the interrupted bash must report cancelled:true, not a full completion: {bash}"
    );
}

// ----------------------------------------------------------------------------------------------
// G2 (CRITICAL) — a contained extension fault must surface to the RPC client as an `extension_error`
// event on stdout (Pi `onError: (err) => output({type:"extension_error", extensionPath, event,
// error})`, rpc-mode.ts:347-349). Pre-fix, `Dispatcher::add_error_listener` is never called by any
// mode, so `report()` fans out to zero listeners and the fault is swallowed into a `tracing::warn!`.
// ----------------------------------------------------------------------------------------------

/// Loads a native extension that panics on the `input` event, drives a prompt through the RPC loop,
/// and asserts the client sees an `extension_error` line carrying `{event:"input", extensionPath,
/// error}`. The panic is contained (the run still proceeds), so the ONLY observable of the fault is
/// the surfaced event — which pre-fix never reaches the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_contained_extension_fault_surfaces_as_extension_error_event() {
    use cyrup_core::ExtensionId;
    use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};

    struct FaultyInputExt {
        id: ExtensionId,
    }
    #[async_trait::async_trait]
    impl NativeExtension for FaultyInputExt {
        fn id(&self) -> ExtensionId {
            self.id.clone()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe(&[EventKind::Input]);
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            panic!("boom in the input handler");
        }
    }

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let runtime = build_runtime(&fx, faux).await;

    // Load the faulting extension into the very session the RPC loop drives.
    let session = runtime.session().await;
    session
        .services()
        .ext_host
        .load_native(Arc::new(FaultyInputExt { id: "faulty".into() }))
        .await
        .expect("load faulty ext");

    let input = concat!(r#"{"type":"prompt","id":"p1","message":"hi"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let err_ev = lines.iter().find(|l| type_of(l) == "extension_error").unwrap_or_else(|| {
        panic!(
            "a contained extension fault must surface as an `extension_error` event (Pi \
             rpc-mode.ts:347-349); none found in:\n{lines:#?}"
        )
    });
    assert_eq!(err_ev["event"], "input", "carries the Pi ExtensionError.event name: {err_ev}");
    assert!(err_ev["extensionPath"].is_string(), "carries extensionPath: {err_ev}");
    assert!(err_ev["error"].is_string(), "carries the error message: {err_ev}");
}

// ----------------------------------------------------------------------------------------------
// SEAM-004 — `set_model` / `get_available_models` / `get_state` must resolve against the FULL
// auth-filtered model registry, not just the currently-installed provider's own catalog. Pi reads
// `session.modelRuntime.getAvailable()` (rpc-mode.ts:468 for set_model, :486 for
// get_available_models), which is `ModelRegistry.getAll().filter(hasConfiguredAuth)` — every
// configured provider, not one. Pre-fix cyrup called `session.model_catalog()` (the active provider
// only), so an RPC embedder could neither see nor select a model owned by another configured
// provider.
// ----------------------------------------------------------------------------------------------

/// A [`cyrup_session_svc::ProviderResolver`] that hands back an offline faux provider for any id —
/// stands in for the binary's `select_provider` seam so a cross-provider `set_model` can complete.
struct AnyFauxResolver;

impl cyrup_session_svc::ProviderResolver for AnyFauxResolver {
    fn resolve(&self, _provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        Ok(Arc::new(FauxProvider::new()))
    }
}

#[tokio::test]
async fn rpc_model_commands_span_the_full_auth_filtered_registry() {
    let fx = fixture();
    // Give `anthropic` a stored credential so `has_configured_auth` is true for its catalog — the
    // "second configured provider" the active (faux) provider knows nothing about.
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
    )
    .expect("write auth.json");

    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(&fx);
    let target = cfg.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, cfg)
            .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>),
    );
    let runtime = AgentSessionRuntime::create(factory, target).await.expect("build runtime");

    // Phase 1 — `get_available_models` must list the OTHER configured provider's models.
    let reader = Cursor::new(
        concat!(r#"{"type":"get_available_models","id":"a"}"#, "\n").as_bytes().to_vec(),
    );
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");
    let lines = parse_lines(&out);
    let listed = lines
        .iter()
        .find(|l| l["command"] == "get_available_models")
        .expect("get_available_models response");
    let models = listed["data"]["models"].as_array().expect("models array").clone();
    let anthropic = models
        .iter()
        .find(|m| m["provider"] == "anthropic")
        .unwrap_or_else(|| {
            panic!(
                "get_available_models must span every CONFIGURED provider (Pi \
                 modelRuntime.getAvailable(), rpc-mode.ts:486), not just the active one; got:\n{models:#?}"
            )
        })
        .clone();
    let anthropic_id =
        anthropic["id"].as_str().expect("catalog model carries an id").to_string();

    // Phase 2 — `set_model` onto that non-active provider must succeed, and `get_state` must then
    // report the FULL model record for it (not the two-field degraded stub).
    let script = format!(
        "{{\"type\":\"set_model\",\"id\":\"b\",\"provider\":\"anthropic\",\"modelId\":\"{anthropic_id}\"}}\n\
         {{\"type\":\"get_state\",\"id\":\"c\"}}\n"
    );
    let reader = Cursor::new(script.into_bytes());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");
    let lines = parse_lines(&out);
    let set = lines.iter().find(|l| l["command"] == "set_model").expect("set_model response");
    assert_eq!(
        set["success"], true,
        "set_model onto a different CONFIGURED provider must succeed (Pi rpc-mode.ts:468-475): {set}"
    );
    let state = lines.iter().find(|l| l["command"] == "get_state").expect("get_state response");
    assert_eq!(state["data"]["model"]["provider"], "anthropic", "get_state model: {state}");
    assert_eq!(state["data"]["model"]["id"].as_str(), Some(anthropic_id.as_str()));
    assert!(
        state["data"]["model"].get("contextWindow").is_some()
            || state["data"]["model"].as_object().map(|o| o.len()).unwrap_or(0) > 2,
        "get_state.model must be the FULL catalog record, not the degraded {{provider,id}} stub: {state}"
    );
}

// ----------------------------------------------------------------------------------------------
// G39 — the SEAM-004 hole `cycle_model` was left in. Pi's `_cycleAvailableModel` opens with
// `const availableModels = await this._modelRuntime.getAvailable()` (agent-session.ts:1644 at
// v0.83.0) — `getAll().filter(hasConfiguredAuth)` across EVERY provider (model-runtime.ts:315-329).
// cyrup cycled `provider.current().models()`, the ONE installed provider's own catalog, so the
// `cycle_model` RPC verb (rpc.rs:1116, the `{"type":"cycle_model"}` request a `cyrup --mode rpc`
// client sends) could never leave the provider the session launched on even when a second provider
// was fully credentialed — while `get_available_models` and `set_model`, fixed by SEAM-004, could.
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn rpc_cycle_model_spans_the_full_auth_filtered_registry() {
    let fx = fixture();
    // The same second configured provider SEAM-004 uses: a stored `auth.json` credential makes
    // `has_configured_auth` true for anthropic's whole catalog. The active provider stays faux.
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
    )
    .expect("write auth.json");

    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let cfg = base_config(&fx);
    let target = cfg.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, cfg)
            .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>),
    );
    let runtime = AgentSessionRuntime::create(factory, target).await.expect("build runtime");

    let reader = Cursor::new(
        concat!(r#"{"type":"cycle_model","id":"c"}"#, "\n", r#"{"type":"get_state","id":"s"}"#, "\n")
            .as_bytes()
            .to_vec(),
    );
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");
    let lines = parse_lines(&out);

    let cycled = lines.iter().find(|l| l["command"] == "cycle_model").expect("cycle_model response");
    assert_eq!(cycled["success"], true, "cycle_model response: {cycled}");
    assert!(
        !cycled["data"].is_null(),
        "one faux model + a CREDENTIALED anthropic catalog is >1 candidate, so Pi's \
         `availableModels.length <= 1` early return must NOT fire (agent-session.ts:1645); \
         cycling the active provider's own catalog alone is what makes this null: {cycled}"
    );
    assert_eq!(
        cycled["data"]["model"]["provider"], "anthropic",
        "cycle_model must step onto the OTHER configured provider (Pi \
         `_modelRuntime.getAvailable()`, agent-session.ts:1644): {cycled}"
    );
    assert_eq!(cycled["data"]["isScoped"], false, "no scoped set → the available arm: {cycled}");

    // ...and the switch is real: the session's live model — what the next turn streams with —
    // reports the new provider, which also proves the owning provider was installed.
    let state = lines.iter().find(|l| l["command"] == "get_state").expect("get_state response");
    assert_eq!(state["data"]["model"]["provider"], "anthropic", "get_state after cycle: {state}");
}

// ----------------------------------------------------------------------------------------------
// SEAM-007 — a compaction refusal must reach the RPC client as `{success:false, error:"…"}`, not
// `{success:true, data:null}`. Pi's `compact` throws (agent-session.ts:1801-1808/1823-1825) and the
// throw propagates through the dispatcher's catch into `error(id, "compact", message)`.
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn rpc_compact_refusal_is_an_error_response_with_pi_s_reason() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let reader =
        Cursor::new(concat!(r#"{"type":"compact","id":"c1"}"#, "\n").as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines.iter().find(|l| l["command"] == "compact").expect("compact response");
    assert_eq!(
        resp["success"], false,
        "nothing-to-compact must be a FAILURE response, not success-with-null: {resp}"
    );
    assert_eq!(
        resp["error"], "Nothing to compact (session too small)",
        "carries Pi's verbatim reason: {resp}"
    );
}

// ----------------------------------------------------------------------------------------------
// DRIFT-004 — the JSON-RPC `bash` command must fire the `user_bash` extension event.
//
// Pi `rpc-mode.ts:558-579`'s `case "bash"` (added by pi 5d548ae9, 2026-07-28, "fix: rpc bash no
// longer bypass user_bash", #7214) emits `emitUserBash({type:"user_bash", command,
// excludeFromContext: command.excludeFromContext ?? false, cwd: sessionManager.getCwd()})` BEFORE
// touching the executor; a handler returning a full `UserBashEventResult.result`
// (`extensions/types.ts:1078-1083`) short-circuits execution entirely — Pi records that override
// via `recordBashResult` and answers `success(id, "bash", eventResult.result)`.
//
// The emission lives at the CALLER, not inside `AgentSession.executeBash` — Pi's `executeBash` still
// has no `emitUserBash` at HEAD, and both front-ends (`rpc-mode.ts:559`, `interactive-mode.ts:6014`)
// emit for themselves. cyrup mirrors that with the shared
// `AgentSession::execute_bash_with_user_event` wrapper over the bare `execute_bash`.
// ----------------------------------------------------------------------------------------------

/// A native extension that records every `user_bash` event payload it is delivered, and optionally
/// answers with a full `UserBashEventResult.result` override that must short-circuit execution.
struct RpcUserBashProbe {
    /// `(command, exclude_from_context, cwd)` for each delivered `user_bash` event.
    seen: Arc<std::sync::Mutex<Vec<(String, bool, String)>>>,
    /// When set, returned as the `result` override (Pi `UserBashEventResult.result`).
    override_result: Option<Value>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for RpcUserBashProbe {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("rpc-user-bash-probe")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::UserBash]);
        Ok(())
    }
    async fn on_event(
        &self,
        ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        if let cyrup_ext::HostEvent::UserBash { command, exclude_from_context, cwd } = ev {
            self.seen.lock().unwrap().push((
                command.clone(),
                *exclude_from_context,
                cwd.clone(),
            ));
            if let Some(result) = &self.override_result {
                return cyrup_ext::HookOutcome::Handled(cyrup_ext::HandledValue(
                    serde_json::json!({ "result": result }),
                ));
            }
        }
        cyrup_ext::HookOutcome::Noop
    }
}

/// Build the RPC runtime with a native extension registered into every session it builds.
async fn build_runtime_with_ext(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    ext: Arc<dyn cyrup_ext::NativeExtension>,
) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, cfg).with_native_extension(ext));
    AgentSessionRuntime::create(factory, target).await.expect("build runtime")
}

/// DRIFT-004 (a): an extension subscribed to `user_bash` actually RECEIVES the event — with the live
/// `{command, excludeFromContext, cwd}` — when the command arrives over JSON-RPC, and the command
/// still really executes when no handler overrides it (Pi `rpc-mode.ts:559-564,573-578`). Pre-fix,
/// `SessionCommand::Bash` called the bare `execute_bash`, so the probe stayed empty and every
/// RPC-issued command was invisible to extensions.
#[tokio::test]
async fn rpc_bash_delivers_user_bash_to_an_extension() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe { seen: seen.clone(), override_result: None });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo rpc-hello","excludeFromContext":true}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let delivered = seen.lock().unwrap().clone();
    assert_eq!(
        delivered.len(),
        1,
        "the extension must RECEIVE exactly one user_bash event from the RPC bash command: \
         {delivered:?}"
    );
    assert_eq!(delivered[0].0, "echo rpc-hello", "the live command crosses the seam");
    assert!(delivered[0].1, "the RPC excludeFromContext flag crosses the seam");
    assert_eq!(delivered[0].2, fx.cwd.display().to_string(), "the session cwd crosses the seam");

    // With no override, the command still really ran. Select the RESPONSE, not the
    // `bash_execution_update` event that shares the request id (DRIFT-006).
    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["success"], true, "an un-overridden RPC bash still executes: {resp}");
    assert!(
        resp["data"]["output"].as_str().unwrap_or_default().contains("rpc-hello"),
        "the real command output is returned: {resp}"
    );
}

/// DRIFT-004 (b): a `user_bash` handler that returns a full `result` override short-circuits local
/// execution — the RPC response carries the extension's `BashResult` verbatim and the transcript
/// records that override (Pi `rpc-mode.ts:566-571`: `recordBashResult(...)` then
/// `success(id, "bash", eventResult.result)`). The command chosen here would produce completely
/// different output if it were actually executed, so the assertion cannot pass by accident.
#[tokio::test]
async fn rpc_bash_honors_a_user_bash_result_override() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe {
        seen: seen.clone(),
        override_result: Some(serde_json::json!({
            "output": "handled-by-extension",
            "exitCode": 0,
            "cancelled": false,
            "truncated": false,
        })),
    });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;
    let session = runtime.session().await;

    let input =
        concat!(r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    assert_eq!(seen.lock().unwrap().len(), 1, "the handler was consulted");

    let lines = parse_lines(&out);
    assert!(
        !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
        "a short-circuited bash streams no execution deltas — nothing ran: {lines:?}"
    );
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["success"], true, "an overridden bash is a success response: {resp}");
    assert_eq!(
        resp["data"]["output"], "handled-by-extension",
        "the extension's result override is returned verbatim: {resp}"
    );
    assert!(
        !resp["data"]["output"].as_str().unwrap_or_default().contains("locally-executed"),
        "a result override must short-circuit local execution entirely: {resp}"
    );

    // Pi records the override through the same `recordBashResult` path a real execution uses.
    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        msgs_json.to_string().contains("handled-by-extension"),
        "the overridden result is recorded into the transcript: {msgs_json}"
    );
}

/// DRIFT-004 (c): a PARTIAL `result` override must short-circuit too. Pi is TypeScript with no
/// runtime type enforcement, so `emitUserBash` short-circuits on ANY truthy `result`
/// (`runner.ts:955-981`) and `rpc-mode.ts:566-571` takes it unconditionally — an extension may
/// legally return just `{output, exitCode}`.
///
/// A strict deserializer here would be a FAIL-OPEN, not a nicety: a sandbox or remote-exec
/// extension returning that shape would yield `None`, the override would be dropped, and the
/// command would fall through and run raw on the local shell — exactly what the extension existed
/// to prevent. `BashResult` is therefore `#[serde(default)]` on every field.
///
/// The sibling test above supplies all four fields and so cannot catch this.
#[tokio::test]
async fn rpc_bash_honors_a_partial_user_bash_result_override() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe {
        seen: seen.clone(),
        // Only the two fields a sandbox extension would realistically know.
        override_result: Some(serde_json::json!({
            "output": "sandboxed-elsewhere",
            "exitCode": 0,
        })),
    });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;
    let session = runtime.session().await;

    let input = concat!(r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    assert_eq!(seen.lock().unwrap().len(), 1, "the handler was consulted");

    let lines = parse_lines(&out);
    assert!(
        !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
        "a partial override must short-circuit local execution just like a full one: {lines:?}"
    );
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["data"]["output"], "sandboxed-elsewhere", "partial override honored: {resp}");
    assert!(
        !resp["data"]["output"].as_str().unwrap_or_default().contains("locally-executed"),
        "the command must NOT have reached the local shell: {resp}"
    );
    // Omitted fields fall back to their defaults rather than voiding the whole override.
    assert_eq!(resp["data"]["cancelled"], false, "omitted `cancelled` defaults: {resp}");
    assert_eq!(resp["data"]["truncated"], false, "omitted `truncated` defaults: {resp}");

    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        msgs_json.to_string().contains("sandboxed-elsewhere"),
        "the partial override is recorded into the transcript: {msgs_json}"
    );
}

/// SEAM-028/SEAM-011 — pi's `setWidget` union member is
/// `{ method: "setWidget"; widgetKey: string; widgetLines: string[] | undefined; widgetPlacement?:
/// "aboveEditor" | "belowEditor" }` (`modes/rpc/rpc-types.ts:264-271` @v0.83.0). The whole
/// `RpcExtensionUIRequest` union carries **no** `widget` key on any member.
///
/// cyrup emits a cyrup-invented `{"widget": <blob>}` instead, because both `wit/world.wit` copies
/// declare `set-widget: func(widget-json: string)` — one opaque payload where pi has three typed
/// fields. An RPC client written to pi's contract therefore cannot render extension widgets at all:
/// no key to key on, no lines to draw, no placement.
///
/// Ignored, not deleted, and deliberately written in pi's shape: this is the assertion that must go
/// green when SEAM-011 widens the WIT, and its presence is what stops the divergence being
/// re-certified as correct by the sibling case above.
#[tokio::test]
#[ignore = "SEAM-011: cyrup's WIT collapses pi's 3-arg setWidget into one opaque blob"]
async fn set_widget_carries_pis_three_fields_and_no_widget_blob() {
    let effect = cyrup_session_svc::UiEffect::SetWidget {
        widget: serde_json::json!({"widget": "text", "text": "hi"}),
    };
    let req = crate::rpc::extension_ui_effect_json(&effect).expect("setWidget reaches the wire");
    assert_eq!(req["method"], "setWidget");
    assert_eq!(req["widgetKey"], "text");
    assert_eq!(req["widgetLines"], serde_json::json!(["hi"]));
    assert!(
        req.get("widget").is_none(),
        "pi's setWidget union member carries no `widget` key: {req:?}"
    );
}
