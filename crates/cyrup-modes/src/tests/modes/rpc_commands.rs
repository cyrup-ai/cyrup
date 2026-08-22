//! The RPC verb surface (R-11-011…016): each case scripts a request stream into `run_rpc` over a
//! `Cursor` and asserts on the response envelopes and event lines it writes back — the prompt /
//! abort / state / commands core, `fork`, the extended model-thinking-stats-session surface,
//! `compact`'s refusal path, and the shapes pi's `RpcSessionState` and `SessionStats` pin
//! key-for-key.

use std::io::Cursor;
use std::sync::Arc;

use super::support::{build_runtime, fixture, parse_lines, type_of};
use crate::run_rpc;
use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_session_svc::{InputSource, UserInput};
use serde_json::Value;

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
