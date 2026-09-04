//! The RPC failure envelopes: what a client sees for an unparseable line, a well-formed line with
//! an unknown `type`, and a known type with a malformed payload — pi's `command`/`id`/`error`
//! echoing rules, including that a numeric `id` still EXECUTES and round-trips as a number.

use std::io::Cursor;
use std::sync::Arc;

use super::support::{build_runtime, fixture, parse_lines, type_of};
use crate::run_rpc;
use cyrup_provider::faux::FauxProvider;

#[tokio::test]
async fn rpc_unknown_command_echoes_id_on_failure() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| type_of(l) == "response")
        .expect("a response");
    assert_eq!(resp["success"], false, "unknown command must fail: {resp}");
    assert_eq!(
        resp["id"], "9",
        "unknown command must echo the request id: {resp}"
    );
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
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| type_of(l) == "response")
        .expect("a response");
    assert_eq!(
        resp["success"], false,
        "malformed command must fail: {resp}"
    );
    assert_eq!(
        resp["id"], "13",
        "malformed command must echo the request id: {resp}"
    );
    assert!(
        resp["error"].is_string(),
        "malformed command must carry an error: {resp}"
    );
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
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| type_of(l) == "response")
        .expect("a response");
    assert_eq!(resp["success"], false, "parse error must fail: {resp}");
    assert_eq!(
        resp["command"], "parse",
        "parse error uses command:\"parse\": {resp}"
    );
    assert!(
        resp.get("id").is_none(),
        "parse error must NOT carry an id: {resp}"
    );
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
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| type_of(l) == "response")
        .expect("a response");
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
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| type_of(l) == "response")
        .expect("a response");
    assert_eq!(
        resp["command"], "abort",
        "numeric-id command must EXECUTE: {resp}"
    );
    assert_eq!(
        resp["success"], true,
        "abort with a numeric id must succeed: {resp}"
    );
    assert_eq!(
        resp["id"], 5,
        "numeric id must be echoed as a number, as-is: {resp}"
    );
    assert!(
        resp["id"].is_number(),
        "id must remain a JSON number, not a string: {resp}"
    );
}

#[tokio::test]
async fn rpc_unknown_command_is_a_failure_not_a_panic() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = "{\"type\":\"does_not_exist\",\"id\":\"9\"}\n";
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| type_of(l) == "response")
        .expect("a response");
    assert_eq!(resp["success"], false, "unknown command must fail: {resp}");
    assert!(
        resp["error"].is_string(),
        "unknown command must carry an error: {resp}"
    );
}
