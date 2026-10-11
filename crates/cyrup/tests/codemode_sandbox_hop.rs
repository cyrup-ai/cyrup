//! `__codemode-sandbox` — the hop the `codemode` tool re-execs `current_exe()` into, driven through
//! the shipped binary. `predispatch` classifies the argv and `main.rs` dispatches it; a
//! classification arm with no dispatch arm falls through to clap and fails SILENTLY, which is what
//! this exists to catch. The frames are the pipe protocol of
//! `cyrup_codemode_runtime::sandbox::wire`: one line per message, a JSON document behind the
//! `FRAME_MARKER`.
//!
//! The second test is the reason the hop exists: `new Array(2 ** 27).fill(0)` under the tool's
//! 256 MiB heap limit aborts a V8 process with SIGTRAP, which used to take the whole `cyrup`
//! process down. Here only the sandbox process dies, and it says why first.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

const MARKER: &str = "\u{1}cyrup-codemode-sandbox\u{1}";
const HEAP: u64 = 256 * 1024 * 1024;

fn start(code: &str) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cyrup"))
        .arg("__codemode-sandbox")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let frame = json!({ "Start": {
        "code": code,
        "tools_json": "[]",
        "globals_json": "[]",
        "store_json": "{}",
        "memory_limit": HEAP,
    }});
    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "{MARKER}{frame}").unwrap();
    stdin.flush().unwrap();
    child
}

/// The frames the process wrote, up to its end.
fn frames(child: &mut Child) -> Vec<Value> {
    let stdout = child.stdout.take().unwrap();
    let mut frames = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.unwrap();
        // Every byte on stdout is a frame: nothing else may reach the parent's pipe.
        let body = line
            .strip_prefix(MARKER)
            .unwrap_or_else(|| panic!("a line on the sandbox's stdout is not a frame: {line:?}"));
        frames.push(serde_json::from_str(body).unwrap());
    }
    frames
}

#[test]
fn the_hop_runs_a_script_and_answers_on_stdout_alone() {
    let mut child = start("text('shown'); return 6 * 7");
    let frames = frames(&mut child);
    assert_eq!(
        frames,
        [
            json!({ "Text": "shown" }),
            // [CYRUP-DELTA] The end of a script carries the errors it never looked at
            // (`UnobservedReport`); this one left none.
            json!({ "Done": { "Returned": {
                "value": "42",
                "writes": "[]",
                "report": { "unhandled": [], "total": 0, "unsettled": [], "waited": [] }
            } } }),
        ]
    );
    // It ends by itself once the script has settled.
    assert!(child.wait().unwrap().success());
}

#[test]
fn an_allocation_the_heap_cannot_satisfy_is_reported_before_the_process_dies() {
    let mut child = start("text('before'); const a = new Array(2 ** 27).fill(0); return a.length");
    let frames = frames(&mut child);
    let status = child.wait().unwrap();
    assert_eq!(frames[0], json!({ "Text": "before" }), "{frames:?}");
    let last = frames.last().unwrap();
    // Either the engine let the heap-limit callback finish the script, or it aborted and the
    // out-of-memory handler spoke first; the host reads both as `InternalError: out of memory`.
    let reported = last == &json!("OutOfMemory")
        || last["Done"]["Threw"]
            .as_str()
            .is_some_and(|error| error.contains("out of memory"));
    assert!(reported, "{frames:?} ({status:?})");
}
