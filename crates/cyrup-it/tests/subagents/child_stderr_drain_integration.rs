//! FULLY-WIRED PROOFS (real OS subprocesses, no mocks) that a child's STDERR is drained
//! CONCURRENTLY with the run, and that what survives the bound is its TAIL.
//!
//! Both properties come straight from pi-subagents v0.43.0's own stderr wiring, which is a single
//! chunk-level handler installed at spawn time:
//!
//! ```text
//! // runs/foreground/execution.ts:1025
//! const stderrTail = createBoundedByteTail();
//! // runs/foreground/execution.ts:1056-1059
//! proc.stderr.on("data", (chunk: Buffer) => {
//!     stderrTail.push(chunk);
//!     stderrReader.push(chunk);
//! });
//! // runs/foreground/execution.ts:1077 (in the `close` handler)
//! const stderr = stderrTail.text();
//! ```
//!
//! Two things in those four lines are load-bearing, and each has a test here:
//!
//! - **It runs DURING the run, not after it.** A `data` handler fires as the bytes arrive, so the
//!   OS pipe never fills and the child never blocks in `write(2)`.
//!   [`a_child_writing_more_stderr_than_the_pipe_buffer_does_not_deadlock_the_parent`] scripts a
//!   child that writes 200 KiB — comfortably past Linux's ~64 KiB pipe buffer — and asserts the
//!   run RETURNS.
//! - **The tail is fed RAW CHUNKS, independent of any line bounding.** `stderrTail.push(chunk)`
//!   never sees a line at all, so an over-long stderr line cannot truncate the capture; the
//!   separate `stderrReader` (`execution.ts:1047-1052`) is the only thing bounded per line, and it
//!   feeds the transcript, not the error.
//!   [`an_over_limit_stderr_line_surfaces_its_tail_not_a_truncation`] scripts one stderr line
//!   larger than `MAX_CHILD_STDERR_BYTES` and asserts the END of it — where a fatal error actually
//!   is — is what reaches the run's error.
//!
//! Gated on `test-fixtures` (matching the `cyrup-subagent-fixture` `[[bin]]` `required-features`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::child_protocol::MAX_CHILD_STDERR_BYTES;
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions, SingleResult};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

fn write_script(dir: &Path, name: &str, script: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script.to_string()).expect("write fixture script");
    path
}

fn base_agent_config(model: &str) -> AgentConfig {
    AgentConfig {
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        model_provider: None,
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        exclude_tools: Vec::new(),
        allow_nested_subagents: None,
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        memory: None,
        tool_budget: None,
        runner: None,          // SUBA-074: the native child, as before
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

/// The fixture binary plus its script, named for ONE run rather than moved into the process
/// environment that every concurrently-running test in this binary shares. `base_args` reaches
/// the child's argv and, via `CYRUP_SUBAGENT_BINARY_ARGS`, any grandchild that re-execs.
fn fixture_spawn_command(script_path: &Path) -> SpawnCommand {
    SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    }
}

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        turn_budget: None,
        permission_rules: None, // SUBA-073: no policy — the pre-field behaviour
        // SUBA-078: this fixture exercises no reasoning ceiling — `None` is "no ceiling
        // configured, so the bound is off", matching `runner_main.rs`'s own hop-2 default.
        thinking_ceiling: None,
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        enforce_hard_turn_limit: false,
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        reads: None,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from(model)],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        skills: None,
        runtime_cwd: None,
        include_progress: None,
        agent_scope: None,
        acceptance: Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            vec![],
        )),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        // SUBA-049: the RETURN half of G90's steer channel. Both paths exist only under a background
        // run directory; a foreground fixture like this one has none. Load-bearing:
        // `build_attempt_spawn_plan` gates both env keys on presence (exec/mod.rs:2227-2250), so
        // `None` keeps the child's env overlay byte-identical to a real foreground child's.
        steer_ack_dir: None,
        steer_capability_path: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        model_scope: None,
    }
}

/// Drive the REAL user action: an orchestrator delegating one task to a subagent.
///
/// The 60 s ceiling is the deadlock detector: nothing scripted here takes anywhere near that long,
/// so the only way to reach it is a parent waiting on an exit that can never come.
async fn run_fixture(dir: &Path, script: &serde_json::Value, name: &str) -> SingleResult {
    let script_path = write_script(dir, name, script);
    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir, "fixture-model");
    opts.spawn_command = Some(fixture_spawn_command(&script_path));
    tokio::time::timeout(
        Duration::from_secs(60),
        cyrup_ext_subagents::exec::run_sync(&agent, "do the thing", &opts),
    )
    .await
    .expect("run_sync must not hang against a scripted fixture child")
}

// =================================================================================================
// The deadlock
// =================================================================================================

/// A verbose child must not be able to freeze the session.
///
/// Linux gives a pipe a ~64 KiB buffer. A child that writes past that blocks in `write(2)` until
/// someone reads. The parent's stdout read loop is the only thing running during an attempt, so if
/// stderr is merely HELD (drained after the wait) rather than drained alongside, the child is
/// blocked writing stderr, the parent is blocked waiting for the child, and neither ever moves:
/// `run_sync` never returns. pi has no such state because `proc.stderr.on("data", …)`
/// (`execution.ts:1056`) consumes every chunk the moment it arrives.
///
/// 200 KiB is ~3x the buffer, so the child blocks well before it is done writing. This test asserts
/// only the thing that was broken — that the call RETURNS — plus that the child really did get to
/// finish writing, which is what distinguishes "drained concurrently" from "child killed to break
/// the jam".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_writing_more_stderr_than_the_pipe_buffer_does_not_deadlock_the_parent() {
    let dir = tempfile::tempdir().expect("tempdir");

    const PAD_BYTES: usize = 200 * 1024;

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit_stderr_padded",
             "head": "verbose-child-log: ",
             "pad_bytes": PAD_BYTES,
             "tail": " ::fatal: the thing that actually went wrong"},
        ],
        "exit_code": 3
    });

    // The assertion is `run_fixture`'s own 60 s timeout: before the fix this call never returned.
    let result = run_fixture(dir.path(), &script, "verbose-stderr.json").await;

    assert_ne!(
        result.exit_code, 0,
        "the child exited 3; a drained-but-failed run is still a failed run: {result:?}"
    );
    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.contains("::fatal: the thing that actually went wrong"),
        "the child ran to completion and its LAST stderr bytes must reach the run's error — \
         anything else means the write was cut short rather than drained; got {} bytes: {:?}",
        error.len(),
        error.chars().take(120).collect::<String>()
    );
}

// =================================================================================================
// The bounded tail
// =================================================================================================

/// An over-limit stderr line must surface its TAIL, not be truncated away.
///
/// pi feeds `stderrTail` RAW CHUNKS (`execution.ts:1057`), never lines, so its per-line stderr
/// bound (`stderrReader`, `execution.ts:1047-1052`) has no say over what the error says: the tail
/// keeps the last `MAX_CHILD_STDERR_BYTES` of everything written, full stop
/// (`createBoundedByteTail`, `child-protocol.ts:377-392`).
///
/// This matters because a child's fatal error is the LAST thing it writes. A capture that stops at
/// the first over-long line reports the child's warm-up chatter and drops the cause of death.
///
/// One line of `MAX_CHILD_STDERR_BYTES + 64 KiB` is used, so the line is over the per-line bound AND
/// the total is over the tail bound — both bounds are engaged at once, and only the tail may win.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_over_limit_stderr_line_surfaces_its_tail_not_a_truncation() {
    let dir = tempfile::tempdir().expect("tempdir");

    const TAIL_MARKER: &str = " ::panicked at 'index out of bounds'";

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit_stderr_padded",
             "head": "HEAD-MARKER-should-be-dropped ",
             "pad_bytes": MAX_CHILD_STDERR_BYTES + 64 * 1024,
             "tail": TAIL_MARKER},
        ],
        "exit_code": 1
    });
    let result = run_fixture(dir.path(), &script, "over-limit-stderr-line.json").await;

    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.ends_with(TAIL_MARKER),
        "the LAST bytes of an over-limit stderr line are what a failed run must report; got {} \
         bytes ending {:?}",
        error.len(),
        error.chars().rev().take(60).collect::<String>()
    );
    assert!(
        !error.contains("HEAD-MARKER-should-be-dropped"),
        "the head is what falls off a TAIL bound — retaining it means the capture is not bounded \
         from the end at all"
    );
    assert!(
        error.len() <= MAX_CHILD_STDERR_BYTES,
        "the capture must stay inside MAX_CHILD_STDERR_BYTES ({MAX_CHILD_STDERR_BYTES}); got {}",
        error.len()
    );
}
