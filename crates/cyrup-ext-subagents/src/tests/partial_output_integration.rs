//! SUBA-195 / SUBA-196 — an ABNORMAL end, driven for real: every test here launches a REAL child
//! process (a shell script speaking the child NDJSON wire, the crate's in-tree fake child, as
//! `abort_recovery_integration.rs` does) and lets the parent kill it — by the run deadline, by a
//! per-tool deadline — or lets it die mid-stream, then asserts what reached the parent's result.
//!
//! Upstream: `createPartialOutputTracker` (`src/runs/shared/partial-output.ts` @pi-subagents
//! ad11b7ab, `9bc8f2d1` / #2653), consumed in `runSingleAttempt` (`src/runs/foreground/execution.ts`)
//! and `runChildSession` (`src/runs/background/run-child-session.ts`); `timeoutCause` (`ba008223` /
//! #2694, same foreground file). The streamed text crosses cyrup's wire as the delta-only
//! `message_update` projection (`crates/cyrup-modes/src/json_event.rs`), which is exactly what the
//! scripts below emit.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::exec::testsupport::{base_opts, sample_agent_config};

/// One `printf` line of the script, shell-quoted.
fn emit(json: &serde_json::Value) -> String {
    let text = json.to_string();
    format!("printf '%s\\n' '{}'\n", text.replace('\'', r"'\''"))
}

fn message_start() -> serde_json::Value {
    json!({
        "type": "message_start",
        "message": { "role": "assistant", "content": [] },
    })
}

/// The delta-only projection cyrup's json mode actually writes (`{type, usage,
/// assistantMessageEvent}` — no `message`, no `partial`).
fn text_delta(text: &str) -> serde_json::Value {
    json!({
        "type": "message_update",
        "usage": { "input": 0, "output": 0 },
        "assistantMessageEvent": { "type": "text_delta", "contentIndex": 0, "delta": text },
    })
}

/// A completed assistant reply that ONLY calls a tool, followed by that tool starting — the child
/// is then inside the tool when anything kills it.
fn tool_only_reply_then_bash_starts() -> String {
    let mut body = emit(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": "call-1", "name": "bash", "arguments": {} }],
            "usage": { "output": 7 },
            "stopReason": "toolUse",
        },
    }));
    body.push_str(&emit(&json!({
        "type": "tool_execution_start",
        "toolCallId": "call-1",
        "toolName": "bash",
        "args": { "command": "sleep 100" },
    })));
    body
}

fn write_child(dir: &Path, name: &str, body: &str) -> PathBuf {
    let script = dir.join(name);
    std::fs::write(&script, format!("#!/bin/sh\n{body}")).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

/// Run `script` through the real `run_sync` with a bound inline output file, so a test can also
/// prove the recovered text never reached it.
async fn run(
    dir: &Path,
    script: PathBuf,
    timeout_ms: Option<u64>,
    tool_timeout_ms: Option<u64>,
) -> crate::exec::run_result::SingleResult {
    let agent = sample_agent_config("m1", &[]);
    let mut opts = base_opts(dir, &["m1"]);
    opts.timeout_ms = timeout_ms;
    opts.deadline_at =
        timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    opts.tool_timeout_ms = tool_timeout_ms;
    opts.output_path = Some(dir.join("report.md"));
    opts.spawn_command = Some(crate::spawn::SpawnCommand {
        binary: script,
        base_args: Vec::new(),
    });
    crate::exec::run_sync(&agent, "write the report", &opts).await
}

fn assert_nothing_saved_or_accepted(dir: &Path, result: &crate::exec::run_result::SingleResult) {
    assert!(
        !dir.join("report.md").exists(),
        "the recovered text must not be saved as the output file"
    );
    assert!(result.saved_output_path.is_none());
    assert!(
        result.acceptance.as_ref().is_none_or(|ledger| matches!(
            ledger.status,
            crate::exec::acceptance::AcceptanceStatus::NotRequired
                | crate::exec::acceptance::AcceptanceStatus::Rejected
        )),
        "the recovered text never satisfies acceptance: {:?}",
        result.acceptance
    );
}

/// SUBA-195 Verify, first half: "A child killed by `timeoutMs` while streaming 'half an answer'
/// returns that text under the timeout preamble with `outputPartial: true`."
///
/// The child streams two deltas of one reply, then sleeps far past the 800ms deadline, so the
/// parent's own deadline arm terminates it with the reply unfinished.
#[tokio::test]
async fn text_streaming_at_the_run_deadline_is_returned_under_the_timeout_preamble() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut body = emit(&json!({ "type": "agent_start" }));
    body.push_str(&emit(&message_start()));
    body.push_str(&emit(&text_delta("half ")));
    body.push_str(&emit(&text_delta("an answer")));
    body.push_str("sleep 30\n");
    let script = write_child(dir.path(), "streaming-child.sh", &body);

    let result = run(dir.path(), script, Some(800), None).await;

    assert!(result.timed_out, "{result:?}");
    assert_ne!(result.exit_code, 0);
    assert!(result.output_partial, "{result:?}");
    let output = result.final_output.as_deref().unwrap_or_default();
    assert!(
        output.starts_with("Subagent timed out after 800ms.\n\nRecovery summary:"),
        "{output}"
    );
    assert!(
        output.ends_with("\n\nPartial output before timeout:\nhalf an answer"),
        "{output}"
    );
    assert_eq!(
        result.output_state,
        crate::exec::output_state::SubagentOutputState::Present
    );
    assert_nothing_saved_or_accepted(dir.path(), &result);
}

/// SUBA-195 Verify, second half: "a tool-only completed reply before the timeout clears it."
///
/// The child streams text, then COMPLETES that turn as a tool-only reply and starts the tool; the
/// deadline lands inside the tool. The earlier streamed text belongs to a reply that completed, so
/// it must not come back as partial output.
#[tokio::test]
async fn a_tool_only_completed_reply_before_the_deadline_clears_the_streamed_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut body = emit(&json!({ "type": "agent_start" }));
    body.push_str(&emit(&message_start()));
    body.push_str(&emit(&text_delta("stale streamed text")));
    body.push_str(&tool_only_reply_then_bash_starts());
    body.push_str("sleep 30\n");
    let script = write_child(dir.path(), "tool-only-child.sh", &body);

    let result = run(dir.path(), script, Some(800), None).await;

    assert!(result.timed_out, "{result:?}");
    assert!(!result.output_partial, "{result:?}");
    let output = result.final_output.as_deref().unwrap_or_default();
    assert!(
        !output.contains("stale streamed text"),
        "a completed reply clears the tracked text: {output}"
    );
    assert!(
        output.starts_with("Subagent timed out after 800ms.\n\nRecovery summary:"),
        "{output}"
    );
    assert!(
        !output.contains("Partial output before timeout:"),
        "{output}"
    );
}

/// SUBA-195 — the `"child error"` cause: the child streams half a reply and then dies (non-zero
/// exit, a stderr diagnostic, no `agent_settled`) — cyrup's subprocess form of upstream's thrown
/// child session. The text comes back labelled `Partial output before child error:`.
#[tokio::test]
async fn text_streaming_when_the_child_dies_is_returned_as_partial_output_before_child_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut body = emit(&json!({ "type": "agent_start" }));
    body.push_str(&emit(&message_start()));
    body.push_str(&emit(&text_delta("half an ")));
    body.push_str(&emit(&text_delta("answer")));
    body.push_str("printf '%s\\n' 'provider stream crashed' >&2\nexit 3\n");
    let script = write_child(dir.path(), "crashing-child.sh", &body);

    let result = run(dir.path(), script, None, None).await;

    assert!(!result.timed_out);
    assert_eq!(result.exit_code, 3, "{result:?}");
    assert!(result.output_partial, "{result:?}");
    assert_eq!(
        result.final_output.as_deref(),
        Some("Partial output before child error:\nhalf an answer"),
        "{result:?}"
    );
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("provider stream crashed")),
        "the error stays the child's own: {:?}",
        result.error
    );
    assert_nothing_saved_or_accepted(dir.path(), &result);
}

/// Upstream's "ordinary non-zero exits are unchanged" (`9bc8f2d1`): a child whose run SETTLED and
/// then exited non-zero did not throw, so text it left streaming is not recovered.
#[tokio::test]
async fn a_child_that_settled_before_exiting_non_zero_is_not_a_child_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut body = emit(&json!({ "type": "agent_start" }));
    body.push_str(&emit(&message_start()));
    body.push_str(&emit(&text_delta("half an answer")));
    body.push_str(&emit(&json!({ "type": "agent_settled" })));
    body.push_str("printf '%s\\n' 'exited after settling' >&2\nexit 3\n");
    let script = write_child(dir.path(), "settled-child.sh", &body);

    let result = run(dir.path(), script, None, None).await;

    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(!result.output_partial, "{result:?}");
    assert!(
        !result
            .final_output
            .as_deref()
            .unwrap_or_default()
            .contains("half an answer"),
        "{result:?}"
    );
}

/// SUBA-196 Verify: "A child whose `bash` exceeds its tool timeout with no `timeoutMs` reports the
/// tool-timeout sentence first and no 'timed out after 0ms'." — upstream's own test shape
/// (`ba008223`, `test/integration/error-handling.test.ts`: `^Tool 'bash' exceeded its timeout of
/// 1000ms\.\n\nRecovery summary:` and `doesNotMatch(/timed out after 0ms/)`).
///
/// The child answers in prose, calls `bash`, and the tool never returns; only the 300ms per-tool
/// deadline can end it. The completed prose is the only partial output — the tool's message is the
/// cause, never the partial.
#[tokio::test]
async fn a_per_tool_timeout_leads_with_the_tools_own_message_and_is_not_partial_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut body = emit(&json!({ "type": "agent_start" }));
    body.push_str(&emit(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "text", "text": "checked the tree" },
                { "type": "toolCall", "id": "call-1", "name": "bash", "arguments": {} },
            ],
            "usage": { "output": 9 },
            "stopReason": "toolUse",
        },
    })));
    body.push_str(&emit(&json!({
        "type": "tool_execution_start",
        "toolCallId": "call-1",
        "toolName": "bash",
        "args": { "command": "sleep 100" },
    })));
    body.push_str("sleep 30\n");
    let script = write_child(dir.path(), "tool-timeout-child.sh", &body);

    let result = run(dir.path(), script, None, Some(300)).await;

    assert!(result.timed_out, "{result:?}");
    assert_eq!(
        result.error.as_deref(),
        Some("Tool 'bash' exceeded its timeout of 300ms.")
    );
    let output = result.final_output.as_deref().unwrap_or_default();
    assert!(
        output.starts_with("Tool 'bash' exceeded its timeout of 300ms.\n\nRecovery summary:"),
        "{output}"
    );
    assert!(!output.contains("timed out after 0ms"), "{output}");
    assert!(
        output.ends_with("\n\nPartial output before timeout:\nchecked the tree"),
        "the completed prose, not the tool's message, is the partial output: {output}"
    );
    assert!(!result.output_partial, "nothing was still streaming");
    assert_nothing_saved_or_accepted(dir.path(), &result);
}

/// SUBA-195 — the BACKGROUND half (pi `runChildSession`, `src/runs/background/run-child-session.ts`,
/// whose `outputPartial` the runner carries onto the step and the terminal result,
/// `subagent-runner.ts`): the same crashing child, launched by the detached runner's own entry
/// (`run_with`), must reach the terminal `ResultFile` with its text and the flag. cyrup's runner
/// drives each step through `run_sync`, so this proves the waist `build_step_result` →
/// `step_result_to_single_result` rather than a second tracker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_background_runner_carries_the_recovered_text_to_the_terminal_result() {
    use crate::background::atomic::write_atomic_json;
    use crate::background::runner_main::{RunnerOverrides, run_with};
    use crate::background::{RunId, RunPaths};

    let dir = tempfile::tempdir().expect("tempdir");
    let mut body = emit(&json!({ "type": "agent_start" }));
    body.push_str(&emit(&message_start()));
    body.push_str(&emit(&text_delta("half an answer")));
    body.push_str("printf '%s\\n' 'provider stream crashed' >&2\nexit 3\n");
    let script = write_child(dir.path(), "crashing-child.sh", &body);

    let run_id = RunId::from_token("partialrun1");
    let run_paths = RunPaths::for_run(
        &dir.path().join("async"),
        &dir.path().join("results"),
        &run_id,
    );
    std::fs::create_dir_all(&run_paths.run_dir).expect("mkdir run dir");
    std::fs::create_dir_all(dir.path().join("results")).expect("mkdir results");
    let mut cfg =
        super::deadline_checkpoint_runner_integration::config(dir.path(), &run_id, u64::MAX);
    cfg.timeout_ms = None;
    cfg.deadline_at_ms = None;
    cfg.checkpoint_before_deadline_ms = None;
    cfg.session_id = Some("partial-session".to_string());
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &cfg)
        .await
        .expect("write config");

    run_with(
        &cfg_path,
        &run_paths,
        RunnerOverrides {
            spawn_command: Some(crate::spawn::SpawnCommand {
                binary: script,
                base_args: Vec::new(),
            }),
            roots: Some(crate::paths::Roots::sandboxed(dir.path())),
            ..RunnerOverrides::default()
        },
    )
    .await
    .expect("run_with never returns Err");

    let session = crate::identity::SessionId::parse("partial-session").expect("non-empty");
    let result_path = run_paths
        .resolve_result(&session, &run_id)
        .await
        .expect("terminal result");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(result_path).expect("read result"))
            .expect("parse result");
    let child = &raw["results"][0];
    assert_eq!(child["outputPartial"], json!(true), "{raw:#}");
    assert_eq!(
        child["finalOutput"],
        json!("Partial output before child error:\nhalf an answer"),
        "{raw:#}"
    );
    assert_eq!(child["outputState"], json!("present"), "{raw:#}");
    // …and a `wait` reader projects the flag (pi `toWaitCompletion`).
    let completion = crate::background::wait_completions::to_wait_completion(&raw, &run_id)
        .expect("a well-formed payload projects");
    assert!(
        completion
            .results
            .first()
            .is_some_and(|child| child.output_partial),
        "{completion:?}"
    );
}
