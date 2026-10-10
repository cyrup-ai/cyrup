//! What survives a child's ABNORMAL end — SUBA-195 (streamed text on a timeout or a child error)
//! and SUBA-196 (a per-tool timeout's attribution).
//!
//! Every test drives [`crate::exec::run_sync`] against a real `/bin/sh` child that speaks the
//! json-mode wire and is then genuinely ended mid-stream: killed by the run deadline while it
//! sleeps, killed by a per-tool deadline while a `bash` call is open, or exiting non-zero on its
//! own. The lines are written before the child blocks, so what the parent has read when the end
//! comes is fixed by the script rather than by timing; the deadlines only decide WHEN the kill
//! lands, never what was streamed before it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::time::{Duration, Instant};

use crate::exec::testsupport::{base_opts, sample_agent_config};
use crate::exec::{RunOptions, SingleResult, format_timeout_message, run_sync};

/// One json-mode line per entry, `printf`-ed in order by the child.
fn script(dir: &Path, lines: &[serde_json::Value], tail: &str) -> crate::spawn::SpawnCommand {
    let mut body = String::from("#!/bin/sh\n");
    for line in lines {
        // Single-quoted for the shell; none of the fixtures below contain a `'`.
        body.push_str(&format!("printf '%s\\n' '{line}'\n"));
    }
    body.push_str(tail);
    body.push('\n');
    let path = dir.join("child.sh");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    crate::spawn::SpawnCommand {
        binary: path,
        base_args: Vec::new(),
    }
}

fn start() -> serde_json::Value {
    serde_json::json!({"type": "message_start", "message": {"role": "assistant", "content": []}})
}

fn delta(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "message_update",
        "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": text}})
}

fn opts_with(dir: &Path, child: crate::spawn::SpawnCommand) -> RunOptions {
    let mut opts = base_opts(dir, &["m1"]);
    opts.spawn_command = Some(child);
    opts
}

fn with_run_deadline(mut opts: RunOptions, ms: u64) -> RunOptions {
    opts.timeout_ms = Some(ms);
    opts.deadline_at = Some(Instant::now() + Duration::from_millis(ms));
    opts
}

async fn run(opts: &RunOptions) -> SingleResult {
    tokio::time::timeout(
        Duration::from_secs(30),
        run_sync(&sample_agent_config("m1", &[]), "answer the question", opts),
    )
    .await
    .expect("run_sync settles once the child is gone")
}

fn output(result: &SingleResult) -> &str {
    result.final_output.as_deref().unwrap_or_default()
}

/// SUBA-195 Verify, first half: a child killed by its run deadline while streaming "half an
/// answer" returns that text under the timeout preamble, flagged partial. RED at HEAD: the text
/// never reached a `message_end`, so the delivered output was the timeout sentence and recovery
/// summary alone, and `output_partial` did not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_killed_mid_stream_by_its_run_deadline_keeps_the_streamed_text() {
    let dir = tempfile::tempdir().unwrap();
    let child = script(
        dir.path(),
        &[start(), delta("half an "), delta("answer")],
        "exec sleep 30",
    );
    let opts = with_run_deadline(opts_with(dir.path(), child), 1_500);

    let result = run(&opts).await;

    assert!(result.timed_out, "{result:?}");
    assert_ne!(result.exit_code, 0, "the run still fails: {result:?}");
    assert!(result.output_partial, "{result:?}");
    let text = output(&result);
    assert!(
        text.starts_with(&format_timeout_message(1_500)),
        "the timeout leads: {text}"
    );
    assert!(
        text.ends_with("\n\nPartial output before timeout:\nhalf an answer"),
        "the streamed text follows under the timeout heading: {text}"
    );
}

/// SUBA-195 Verify, second half: a completed tool-only reply before the timeout clears the
/// tracker, so an earlier draft never comes back as partial output. This passes at HEAD too (HEAD
/// kept no streamed text at all), so it is the NON-REGRESSION GUARD for the tracker's clear arm,
/// not proof of the fix: making a clean `message_end` keep what it had (dropping both its
/// `streaming.clear()` and its `Latest::None`) turns it red.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completed_tool_only_reply_before_the_timeout_leaves_no_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let tool_only = serde_json::json!({"type": "message_end", "message": {
        "role": "assistant", "stopReason": "toolUse",
        "content": [{"type": "toolCall", "id": "c1", "name": "read", "arguments": {}}]}});
    let child = script(
        dir.path(),
        &[start(), delta("let me look first"), tool_only],
        "exec sleep 30",
    );
    let opts = with_run_deadline(opts_with(dir.path(), child), 1_500);

    let result = run(&opts).await;

    assert!(result.timed_out, "{result:?}");
    assert!(!result.output_partial, "{result:?}");
    let text = output(&result);
    assert!(
        !text.contains("let me look first") && !text.contains("Partial output before"),
        "a completed reply's draft is not partial output: {text}"
    );
}

/// SUBA-195, the child-error cause: a child that streams and then exits non-zero on its own keeps
/// that text under `Partial output before child error:`. RED at HEAD: the delivered output held
/// none of the streamed text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_that_exits_on_an_error_mid_stream_keeps_the_streamed_text() {
    let dir = tempfile::tempdir().unwrap();
    let child = script(
        dir.path(),
        &[start(), delta("half an "), delta("answer")],
        "echo 'provider exploded' >&2\nexit 3",
    );
    let opts = opts_with(dir.path(), child);

    let result = run(&opts).await;

    assert!(!result.timed_out, "{result:?}");
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(result.output_partial, "{result:?}");
    assert!(
        output(&result).contains("Partial output before child error:\nhalf an answer"),
        "{result:?}"
    );
}

/// A child ended by a SIGNAL is not a child error (`resolveSubagentResultStatus` reads an
/// unexplained signal as `stopped`, and upstream's gate excludes `stopped`). Passes at HEAD; the
/// NON-REGRESSION GUARD for the `code != 0` half of `child_error` — widening it to "any non-success
/// status" turns it red.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_killed_by_a_signal_is_not_a_child_error() {
    let dir = tempfile::tempdir().unwrap();
    let child = script(
        dir.path(),
        &[start(), delta("half an answer")],
        "kill -9 $$",
    );
    let opts = opts_with(dir.path(), child);

    let result = run(&opts).await;

    assert!(!result.output_partial, "{result:?}");
    assert!(!output(&result).contains("half an answer"), "{result:?}");
}

/// SUBA-196 Verify: a child whose `bash` exceeds its per-tool deadline, with NO run timeout,
/// reports the tool-timeout sentence first, never "timed out after 0ms", and does not present the
/// tool message as partial output. RED at HEAD: the output read
/// `Subagent timed out after 0ms.\n\n…\n\nPartial output before timeout:\nTool 'bash' exceeded …`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_per_tool_timeout_leads_with_the_tools_own_message() {
    let dir = tempfile::tempdir().unwrap();
    let child = script(
        dir.path(),
        &[
            serde_json::json!({"type": "tool_execution_start", "toolCallId": "c1",
            "toolName": "bash", "args": {"command": "sleep 600"}}),
        ],
        "exec sleep 30",
    );
    let mut opts = opts_with(dir.path(), child);
    opts.tool_timeout_ms = Some(400);

    let result = run(&opts).await;

    assert!(result.timed_out, "{result:?}");
    let text = output(&result);
    assert!(
        text.starts_with("Tool 'bash' exceeded its timeout of 400ms."),
        "the tool's own message leads: {text}"
    );
    assert!(!text.contains("timed out after 0ms"), "{text}");
    assert!(!text.contains("Partial output before timeout:"), "{text}");
}

/// SUBA-196 with SUBA-195: text streamed before a tool deadline fired is the partial output under
/// the TOOL's message. RED at HEAD on both halves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_streamed_before_a_tool_timeout_follows_the_tools_message() {
    let dir = tempfile::tempdir().unwrap();
    let child = script(
        dir.path(),
        &[
            start(),
            delta("checking the build"),
            serde_json::json!({"type": "tool_execution_start", "toolCallId": "c1",
                "toolName": "bash", "args": {"command": "make"}}),
        ],
        "exec sleep 30",
    );
    let mut opts = opts_with(dir.path(), child);
    opts.tool_timeout_ms = Some(400);

    let result = run(&opts).await;

    assert!(result.output_partial, "{result:?}");
    let text = output(&result);
    assert!(
        text.starts_with("Tool 'bash' exceeded its timeout of 400ms."),
        "{text}"
    );
    assert!(
        text.ends_with("\n\nPartial output before timeout:\nchecking the build"),
        "{text}"
    );
}
