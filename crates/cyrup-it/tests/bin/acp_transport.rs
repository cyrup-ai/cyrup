//! The ACP front-end's process seam: `ACP-003`, `ACP-021`, `ACP-001`, `ACP-026`, `ACP-002`.
//!
//! These four units name assertions that **cannot** be made in-process, so they live here per
//! `docs/TEST-ARCHITECTURE.md` §2: each spawns the real `cyrup` binary, writes hand-written
//! JSON-RPC frames to its stdin, and asserts on the raw bytes a supervising editor would see on
//! stdout plus the process's exit code.
//!
//! Fully offline and **deliberately credential-less** — that is not hygiene here, it is the
//! subject: `ACP-021` is exactly the claim that a `cyrup --acp` with no configured provider still
//! answers `initialize` on stdout and exits 0 on EOF, rather than printing
//! `No models available. Use /login …` to stderr and exiting 1 before the transport exists.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::TempDir;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    /// Every JSON-RPC frame on stdout, in wire order. A frame that does not parse is a failure, not
    /// a skip: an ACP client's parser would die on it too.
    fn frames(&self) -> Vec<serde_json::Value> {
        self.stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"))
            })
            .collect()
    }

    fn frame_with_id(&self, id: u64) -> serde_json::Value {
        self.frames()
            .into_iter()
            .find(|f| f.get("id").and_then(serde_json::Value::as_u64) == Some(id))
            .unwrap_or_else(|| panic!("no frame with id {id} in:\n{}", self.stdout))
    }
}

/// Spawn `cyrup` with `args`, feed `stdin_frames` (newline-delimited), close stdin, and collect.
///
/// The env is scrubbed of every provider credential **and** of the three built-in opt-ins, for the
/// reasons `unknown_flag_exit`'s own comment gives: an ambient `CYRUP_INTERCOM=1` alone satisfies
/// `is_installed()` and detaches an immortal broker per run.
fn run(args: &[&str], stdin_frames: &[&str]) -> (Run, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = Command::new(crate::support::bins::cyrup());
    cmd.current_dir(&work)
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("TOGETHER_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("CYRUP_INTERCOM")
        .env_remove("CYRUP_SUBAGENTS")
        .env_remove("CYRUP_PERMISSION_SYSTEM")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn cyrup --acp");
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        for frame in stdin_frames {
            writeln!(stdin, "{frame}").expect("write frame");
        }
        // Dropping `stdin` closes the pipe — this IS `ACP-005`'s stdin-EOF path.
    }
    let out = child.wait_with_output().expect("wait for cyrup");
    (
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        tmp,
    )
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true}}}"#;

/// **ACP-003** — the stdio transport bootstrap. Pipe a hand-written `initialize` frame into
/// `cyrup --acp`; assert a well-formed response frame and exit 0 on EOF.
///
/// **ACP-021** — and do it with **no credentials configured**, which is the whole unit. Both
/// non-interactive hosts take pi's modelless hard stop (`require_model: true`, main.ts:852-855),
/// which `session_launch::launch` executes — `diagnostics::no_models_available()` then exit 1 —
/// *before* `main` reaches `match mode`. If `AppMode::Acp` ever joins that leg, this test goes red
/// with exit 1, zero frames on stdout and `No models available` on stderr, and the entire
/// authentication surface (`ACP-010`, `ACP-012`, `ACP-016`, `ACP-017`) becomes unreachable.
#[test]
fn acp_answers_initialize_with_no_credentials_and_exits_zero_on_eof() {
    let (run, _tmp) = run(&["--acp"], &[INITIALIZE]);

    assert_eq!(
        run.code, 0,
        "ACP-003/ACP-021: clean stdin EOF is exit 0.\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert!(
        !run.stderr.contains("No models available"),
        "ACP-021: the modelless hard stop must not apply to the ACP host.\nstderr:\n{}",
        run.stderr
    );

    let response = run.frame_with_id(1);
    assert_eq!(response["jsonrpc"], "2.0");
    assert!(
        response.get("error").is_none(),
        "ACP-021: initialize must succeed without credentials: {response}"
    );
    let result = &response["result"];
    // ACP-050: every requested version is answered with 1.
    assert_eq!(result["protocolVersion"], 1);
    // ACP-051: `agentInfo` is compile-time identity — no package.json walk, no `??` fallback.
    assert_eq!(result["agentInfo"]["name"], "cyrup");
    assert!(
        result["agentInfo"]["version"].is_string(),
        "agentInfo.version must be present: {result}"
    );
    assert!(
        result.get("agentCapabilities").is_some(),
        "the response must carry an agentCapabilities block: {result}"
    );
}

/// **ACP-014** — a forgotten handler is a HANG, not a `method_not_found`: an unregistered
/// session-scoped method falls through to `default_handle_dispatch_from`, which returns
/// `Handled::No { retry: message.has_session_id() }` and the request is retained and retried.
///
/// So this asserts the two things that distinguish a live handler table from a hung one at this
/// seam: `authenticate` is answered even though it was sent *behind* a `session/new` that is still
/// building, and it answers with a `result` rather than an `error` (Zed calls it after the terminal
/// flow, and an error reads as a failed login).
///
/// That "answered behind a build" is also **`ACP-057`'s dispatch-loop property** in its cheapest
/// observable form, and this shape is why it is observable at all: a `session/new` that awaited its
/// build inline would hold the loop, and nothing after it could be answered before stdin closed.
///
/// # Why `session/new`'s own response is NOT asserted here
///
/// This helper is a **one-shot** pipe: it writes every frame and closes stdin immediately, which is
/// `ACP-005`'s EOF path. `session/new` now performs a real session build — settings, resources,
/// extension host — and the client has hung up long before it finishes, so there is nobody left to
/// answer. That is correct behaviour, not a hang, and the process still exits 0. The full
/// request/response lifecycle over a connection whose stdin stays open is
/// [`super::acp_session`]'s; this file keeps the assertions that need a *closed* pipe.
#[test]
fn a_later_request_is_answered_while_session_new_is_still_building() {
    let new_session = r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#;
    let authenticate = r#"{"jsonrpc":"2.0","id":3,"method":"authenticate","params":{"methodId":"cyrup_terminal_login"}}"#;
    let (run, _tmp) = run(&["--acp"], &[INITIALIZE, new_session, authenticate]);

    assert_eq!(run.code, 0, "stderr:\n{}", run.stderr);

    for id in [1u64, 3] {
        let frame = run.frame_with_id(id);
        assert!(
            frame.get("result").is_some() || frame.get("error").is_some(),
            "ACP-014: id {id} was never answered — a forgotten handler is a hang: {frame}"
        );
    }

    // ACP-014: `authenticate` is a successful no-op and MUST NOT error.
    let auth = run.frame_with_id(3);
    assert!(
        auth.get("error").is_none(),
        "ACP-014: authenticate must succeed; an error reads as a failed login: {auth}"
    );

    // ACP-057: id 3 was sent AFTER id 2 and answered while id 2 was still building.
    let ids: Vec<u64> = run
        .frames()
        .iter()
        .filter_map(|f| f.get("id").and_then(serde_json::Value::as_u64))
        .collect();
    assert!(
        ids.contains(&3),
        "ACP-057: `session/new` must be built OFF the dispatch loop, so a later request is still \
         answered. Frame id order was {ids:?}"
    );
}

/// **ACP-001** and **ACP-026** — the terminal-login gate.
///
/// `ACP-001`: `cyrup --acp --terminal-login` writes **zero** JSON-RPC frames to stdout. The token
/// arrives LAST because an ACP client appends `AuthMethod.args` to the agent command it already
/// holds, so the gate's predicate is membership anywhere in argv, and it is classified before clap.
///
/// `ACP-026`: with both ends piped there is no terminal to land in, so the run produces a
/// diagnostic and a non-TUI exit rather than a TUI painted into a pipe. Upstream's
/// `spawnSync(cmd, [], {stdio:"inherit"})` does exactly that, silently.
#[test]
fn terminal_login_writes_no_frames_and_refuses_without_a_tty() {
    let (run, _tmp) = run(&["--acp", "--terminal-login"], &[INITIALIZE]);

    assert!(
        run.stdout.trim().is_empty(),
        "ACP-001: the login gate must write zero JSON-RPC frames.\nstdout:\n{}",
        run.stdout
    );
    assert_ne!(
        run.code, 0,
        "ACP-026: with both ends piped this must refuse rather than paint a TUI into a pipe"
    );
    assert!(
        run.stderr.contains("--terminal-login"),
        "ACP-026: the refusal must be diagnosable.\nstderr:\n{}",
        run.stderr
    );
    // The diagnostic must not name another product, and must not claim a TUI was started.
    assert!(
        !run.stderr.to_lowercase().contains(" pi "),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("npm"), "{}", run.stderr);
}

/// **ACP-002** — `--mode acp` is the same host as `--acp`, and it resolves with pipes on both ends.
///
/// The unit's own verify is a `resolve_app_mode` table test (which lives in
/// `crates/cyrup/src/cli/tests/runtime_mode.rs`); this is the end-to-end half, and it is the one
/// that would catch the failure the unit is actually about — the ACP branch falling through to
/// `Print`, which would answer the client's first JSON-RPC frame as a chat prompt on the stream the
/// client is parsing as JSON-RPC.
#[test]
fn mode_acp_is_the_same_host_as_the_alias() {
    let (run, _tmp) = run(&["--mode", "acp"], &[INITIALIZE]);
    assert_eq!(run.code, 0, "stderr:\n{}", run.stderr);
    let response = run.frame_with_id(1);
    assert_eq!(
        response["result"]["agentInfo"]["name"], "cyrup",
        "ACP-002: `--mode acp` must reach the ACP host, not Print: {response}"
    );
    // The strongest form of "it did not become a one-shot printer": every stdout line parses as a
    // JSON-RPC frame, so no assistant text was ever written to the protocol stream.
    for frame in run.frames() {
        assert_eq!(
            frame["jsonrpc"], "2.0",
            "non-JSON-RPC line on stdout: {frame}"
        );
    }
}
