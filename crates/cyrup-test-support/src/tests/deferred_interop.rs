//! R-00-013 interop: a **Pi-written** session JSONL must load, retain and re-export without loss.
//!
//! Two fields on `AssistantMessage` were absent from cyrup and therefore destroyed on every
//! re-export:
//!
//! * `rawStopReason` (`v0.84.1 ai/src/types.ts:426`, and already at `v0.83.0 ai/src/types.ts:411`)
//!   — Pi populates it on essentially every settled streaming turn: Anthropic
//!   `event.delta.stop_reason` (`v0.83.0 ai/src/api/anthropic-messages.ts:709`), Google
//!   `candidate.finishReason` (`:215`), Mistral `choice.finishReason` (`:356`), OpenAI-completions
//!   `choice.finish_reason` (`:459`), OpenAI-responses `response.status` (`:567,721`). It is a
//!   LIVE, every-file loss, not a v0.84 novelty.
//! * `deferred: DeferredHandle` (`v0.84.1 ai/src/types.ts:395-404,424`) — the payload of a
//!   `stopReason: "deferred"` turn. Without it the handle is the only record of an in-flight
//!   request, and losing it strands the request permanently.
//!
//! Workspace clippy DENIES `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` — hence the
//! file-level allow.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::assert_jsonl_roundtrip;

const HEADER: &str = r#"{"type":"session","version":3,"id":"01890000-0000-7000-8000-000000000001","timestamp":"2026-08-08T00:00:00.000Z","cwd":"/tmp/pi-session"}"#;

/// A user entry.
fn user(id: &str, parent: Option<&str>, text: &str) -> String {
    let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
    format!(
        r#"{{"type":"message","id":"{id}","parentId":{parent},"timestamp":"2026-08-08T00:00:00.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}],"timestamp":1754611200000}}}}"#
    )
}

/// An assistant entry whose `message` body is spliced in verbatim, so each test controls the exact
/// Pi bytes under test.
fn assistant(id: &str, parent: &str, body: &str) -> String {
    format!(
        r#"{{"type":"message","id":"{id}","parentId":"{parent}","timestamp":"2026-08-08T00:00:00.000Z","message":{{"role":"assistant","content":[],"api":"anthropic-messages","provider":"anthropic","model":"claude-sonnet-4-5","usage":{{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}}},{body},"timestamp":1754611200000}}}}"#
    )
}

fn jsonl(lines: &[String]) -> String {
    let mut s = String::from(HEADER);
    for l in lines {
        s.push('\n');
        s.push_str(l);
    }
    s.push('\n');
    s
}

/// A settled Pi turn carrying `rawStopReason` round-trips it. This is the every-file case: Pi
/// stamps `rawStopReason` on the vast majority of persisted assistant turns.
#[test]
fn settled_turn_round_trips_raw_stop_reason() {
    let input = jsonl(&[
        user("aaaaaaa1", None, "hello"),
        assistant(
            "aaaaaaa2",
            "aaaaaaa1",
            r#""stopReason":"stop","rawStopReason":"end_turn""#,
        ),
    ]);

    let exported = assert_jsonl_roundtrip(&input).expect("pi turn must round-trip");
    assert!(
        exported.contains(r#""rawStopReason":"end_turn""#),
        "rawStopReason must survive re-export, got:\n{exported}"
    );
}

/// The same for an ERROR turn, where `rawStopReason` is the only record of what the provider
/// actually said (Pi's `errorMessage` is cyrup-formatted prose; `rawStopReason` is the wire token).
#[test]
fn error_turn_round_trips_raw_stop_reason_and_error_message() {
    let input = jsonl(&[
        user("bbbbbbb1", None, "hello"),
        assistant(
            "bbbbbbb2",
            "bbbbbbb1",
            r#""stopReason":"error","errorMessage":"Provider stopped with: SAFETY","rawStopReason":"SAFETY""#,
        ),
    ]);

    let exported = assert_jsonl_roundtrip(&input).expect("pi error turn must round-trip");
    assert!(
        exported.contains(r#""rawStopReason":"SAFETY""#),
        "rawStopReason must survive on the error path too, got:\n{exported}"
    );
}

/// A `stopReason: "deferred"` turn keeps its whole [`DeferredHandle`], including the optional
/// `expiresAt`/`pollAfterMs`/`data` (`v0.84.1 ai/src/types.ts:395-404`).
///
/// This is the guard that makes `StopReason::Deferred` SAFE to have. Once the variant exists the
/// entry deserializes as a `KnownEntry::Message` and is re-serialized field by field — so without
/// `AssistantMessage::deferred` the handle is silently destroyed on the next save. (Before the
/// variant existed the same line fell to `Entry::Unknown`'s verbatim-preservation path,
/// `cyrup-session/src/entry.rs:275-281`, and survived by accident while being uninterpretable.)
#[test]
fn deferred_turn_round_trips_its_handle() {
    let handle = r#"{"provider":"openai","modelId":"gpt-5.5","api":"openai-responses","id":"resp_abc123","expiresAt":1754697600000,"pollAfterMs":2000,"data":{"conversion":{"reasoningIndex":2}}}"#;
    let input = jsonl(&[
        user("ccccccc1", None, "long job"),
        assistant(
            "ccccccc2",
            "ccccccc1",
            &format!(r#""stopReason":"deferred","deferred":{handle},"rawStopReason":"queued""#),
        ),
    ]);

    let exported = assert_jsonl_roundtrip(&input).expect("pi deferred turn must round-trip");
    for needle in [
        r#""stopReason":"deferred""#,
        r#""id":"resp_abc123""#,
        r#""expiresAt":1754697600000"#,
        r#""pollAfterMs":2000"#,
        r#""reasoningIndex":2"#,
        r#""rawStopReason":"queued""#,
    ] {
        assert!(
            exported.contains(needle),
            "lost {needle} on re-export:\n{exported}"
        );
    }
}

/// A deferred turn NOT at the leaf: everything after it must survive too. Guards the
/// "session truncates at the first entry it cannot parse" failure mode directly.
#[test]
fn entries_after_a_deferred_turn_survive() {
    let handle =
        r#"{"provider":"openai","modelId":"gpt-5.5","api":"openai-responses","id":"resp_mid"}"#;
    let input = jsonl(&[
        user("ddddddd1", None, "first"),
        assistant(
            "ddddddd2",
            "ddddddd1",
            &format!(r#""stopReason":"deferred","deferred":{handle}"#),
        ),
        user("ddddddd3", Some("ddddddd2"), "second"),
        assistant(
            "ddddddd4",
            "ddddddd3",
            r#""stopReason":"stop","rawStopReason":"end_turn""#,
        ),
    ]);

    let exported = assert_jsonl_roundtrip(&input).expect("tail after a deferred turn must survive");
    assert_eq!(
        exported.lines().count(),
        5,
        "header + 4 entries must all be re-exported:\n{exported}"
    );
    assert!(
        exported.contains(r#""text":"second""#),
        "tail entry lost:\n{exported}"
    );
}

/// MIRROR — a v0.83.0-shaped turn that carries NEITHER new field must re-export with neither key
/// present. `skip_serializing_if = "Option::is_none"` is what keeps every existing golden snapshot
/// and every captured-Pi byte comparison valid; a `null`-emitting spelling would have broken them
/// all.
#[test]
fn mirror_turn_without_the_new_fields_gains_no_new_keys() {
    let input = jsonl(&[
        user("eeeeeee1", None, "hello"),
        assistant("eeeeeee2", "eeeeeee1", r#""stopReason":"stop""#),
    ]);

    let exported = assert_jsonl_roundtrip(&input).expect("plain turn must round-trip");
    assert!(
        !exported.contains("rawStopReason"),
        "invented a rawStopReason key:\n{exported}"
    );
    assert!(
        !exported.contains("deferred"),
        "invented a deferred key:\n{exported}"
    );
}
