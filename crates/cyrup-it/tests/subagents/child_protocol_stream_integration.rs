//! FULLY-WIRED PROOFS (real OS subprocesses, no mocks) for the three child-NDJSON-stream gaps
//! ported from pi-subagents v0.43.0, each driving the REAL production path — `exec::run_sync` →
//! `build_attempt_spawn_plan` → `SpawnedChild::spawn` → the bounded stdout reader → `drive_attempt`
//! — against the scripted `cyrup-subagent-fixture` child:
//!
//! - **G75** the 16 MiB per-line stdout cap and its `protocol_output_limit` diagnostic
//!   (`runs/shared/child-protocol.ts:6,244-293`; failed at `runs/foreground/execution.ts:1026-1046`),
//!   plus the oversized-aggregate PROJECTION that keeps a legitimately huge `turn_end`/`agent_end`
//!   from failing the run (`child-protocol.ts:226-238`).
//! - **G76** the drain lifecycle (`projectChildLifecycle`, `child-protocol.ts:394-401`, applied at
//!   `execution.ts:844,947`): `agent_settled` STARTS the final-stop grace window and
//!   `agent_end{willRetry:true}` CANCELS it.
//! - **G74** the zero-activity startup retry ladder (`runs/shared/subagent-startup-retry.ts`,
//!   driven at `execution.ts:1518-1619`): the SAME model is relaunched up to three extra times when
//!   the child died before doing anything at all, and is NOT relaunched when it did.
//!
//! Gated on `test-fixtures` (matching the `cyrup-subagent-fixture` `[[bin]]` `required-features`) —
//! without that feature the fixture binary does not exist and none of these tests can run at all.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::child_protocol::MAX_CHILD_PENDING_LINE_BYTES;
use cyrup_ext_subagents::exec::fallback::{
    ModelOverride, SUBAGENT_STARTUP_RETRY_DELAYS_MS, format_subagent_startup_retry_exhausted_error,
};
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

/// The fixture binary plus its script, named for ONE run instead of moved into the process
/// environment every concurrently-running test in this binary shares. `base_args` reaches the
/// child's argv and, through `CYRUP_SUBAGENT_BINARY_ARGS`, any grandchild that re-execs.
fn fixture_spawn_command(script_path: &Path) -> SpawnCommand {
    SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    }
}

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "stop"
        }
    })
    .to_string()
}

fn base_agent_config(model: &str) -> AgentConfig {
    AgentConfig {
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
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
        runner: None, // SUBA-074: the native child, as before
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
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
// G75 — the per-line stdout cap
// =================================================================================================

/// A child that emits ONE line larger than the cap must fail the run with the
/// `protocol_output_limit` diagnostic.
///
/// Before the fix the parent read stdout through `tokio::io::Lines`, which grows a single `String`
/// until it sees a `\n`: this exact child (17 MiB with no newline until the very end) grew the
/// PARENT's heap by 17 MiB and then delivered the line as an ordinary event. A child that never
/// emits the newline at all — the real hazard — grew it without any bound at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_over_cap_child_line_fails_the_run_with_a_protocol_output_limit() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            // NOT one of the two redundant aggregate records, so there is nothing to project: the
            // only correct outcome is a diagnosed failure.
            {"kind": "emit_padded",
             "head": "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":\"",
             "pad_bytes": MAX_CHILD_PENDING_LINE_BYTES + 4096,
             "tail": "\"}}"},
            {"kind": "emit", "line": message_end_line("this never gets read")}
        ],
        "exit_code": 0
    });
    let result = run_fixture(dir.path(), &script, "over-cap.json").await;

    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.starts_with("protocol_output_limit:"),
        "an over-cap child line must surface pi's `formatProtocolOutputLimit` text as the run's \
         error; got {error:?} (exit {})",
        result.exit_code
    );
    assert!(
        error.contains(&format!("exceeded {MAX_CHILD_PENDING_LINE_BYTES} bytes")),
        "the diagnostic must name the cap that was exceeded: {error}"
    );
    assert_ne!(
        result.exit_code, 0,
        "a protocol violation is a FAILED run, never coerced to success: {result:?}"
    );
}

/// MIRROR: the aggregate records that can legitimately exceed the cap are RECOVERED, not failed
/// (`PI_AGGREGATE_EVENT_PROJECTOR`, `child-protocol.ts:226-238`). cyrup's json mode emits `turn_end`
/// carrying the whole assistant message plus every tool result, so one parallel image read can push
/// it past 16 MiB with every granular event perfectly valid — capping without this recovery would
/// have failed runs upstream completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_over_cap_turn_end_aggregate_is_projected_and_the_run_still_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("the real answer")},
            {"kind": "emit_padded",
             "head": "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"content\":\"",
             "pad_bytes": MAX_CHILD_PENDING_LINE_BYTES + 4096,
             "tail": "\"}}"},
            {"kind": "emit", "line": "{\"type\":\"agent_settled\"}"}
        ],
        "exit_code": 0
    });
    let result = run_fixture(dir.path(), &script, "over-cap-aggregate.json").await;

    assert_eq!(
        result.exit_code, 0,
        "a syntactically valid, redundant aggregate must be reduced, not fail the run: {result:?}"
    );
    assert_eq!(result.error, None, "no diagnostic for a projected record");
    assert_eq!(
        result.final_output.as_deref(),
        Some("the real answer"),
        "the granular events around the oversized aggregate must still drive the result"
    );
}

// =================================================================================================
// G76 — the drain lifecycle
// =================================================================================================

/// `agent_settled` ARMS the final-stop grace window (`projectChildLifecycle`,
/// `child-protocol.ts:398`).
///
/// The child here settles and then never exits (it holds stdout open for 30s). Before the fix
/// `agent_settled` parsed to `SubagentEvent::Unknown`, nothing armed the window, and — with no
/// terminal assistant stop, no deadline and no cancel — the parent had no arm that could ever fire:
/// the delegating tool call simply blocked for the full 30s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_settled_arms_the_final_drain_so_a_settled_but_hanging_child_is_force_drained() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            // Deliberately NOT a terminal assistant stop: `stopReason` is `toolUse`, so the only
            // thing that can arm the window is `agent_settled` itself.
            {"kind": "emit", "line": serde_json::json!({
                "type": "message_end",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "settled output"}],
                    "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                        "totalTokens": 2, "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0,
                        "cacheWrite": 0.0, "total": 0.0}},
                    "stopReason": "toolUse"
                }
            }).to_string()},
            {"kind": "emit", "line": "{\"type\":\"agent_settled\"}"},
            {"kind": "sleep_ms", "ms": 30000}
        ],
        "exit_code": 0
    });

    let started = Instant::now();
    let result = run_fixture(dir.path(), &script, "settled-hang.json").await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(15),
        "the settled child must be force-drained on the 1s grace window, not waited out for its \
         full 30s sleep; took {elapsed:?}"
    );
    assert_eq!(
        result.exit_code, 0,
        "a child that ANNOUNCED it settled and was then force-drained is pi's \
         `forcedDrainAfterFinalSuccess` (`execution.ts:1080` reads \
         `cleanTerminalAssistantStopReceived || agentSettledReceived`): {result:?}"
    );
    assert_eq!(result.final_output.as_deref(), Some("settled output"));
}

/// `agent_end{willRetry:true}` CANCELS an armed drain (`child-protocol.ts:397`).
///
/// The child emits a clean terminal stop (arming the 1s window), then announces an auto-retry, then
/// works for 2.5s and produces its REAL answer. Before the fix `will_retry` was parsed at
/// `ndjson.rs:156` and thrown away: the window fired at 1s and the still-working child was killed
/// through the signal ladder, the run silently keeping the pre-retry text as its answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_end_with_will_retry_cancels_the_armed_drain_so_the_retry_survives() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("partial answer before the retry")},
            {"kind": "emit", "line": "{\"type\":\"agent_end\",\"willRetry\":true}"},
            {"kind": "sleep_ms", "ms": 2500},
            {"kind": "emit", "line": message_end_line("the answer after the retry")},
            {"kind": "emit", "line": "{\"type\":\"agent_settled\"}"}
        ],
        "exit_code": 0
    });
    let result = run_fixture(dir.path(), &script, "will-retry.json").await;

    assert_eq!(
        result.final_output.as_deref(),
        Some("the answer after the retry"),
        "the retrying child must survive the grace window it had already armed: {result:?}"
    );
    assert_eq!(result.exit_code, 0, "{result:?}");
}

// =================================================================================================
// G74 — zero-activity startup retry
// =================================================================================================

/// A child that dies before ANY model or tool activity is relaunched on the SAME model, up to
/// `SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1` launches total, then reported as a startup failure
/// (`execution.ts:1558-1619`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_activity_child_exit_relaunches_the_same_model_then_reports_exhaustion() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Nothing on stdout, nothing on stderr, a bare non-zero exit — the shape of a child that never
    // got as far as running.
    let script = serde_json::json!({ "steps": [], "exit_code": 3 });
    let result = run_fixture(dir.path(), &script, "no-start.json").await;

    let expected_launches = SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1;
    assert_eq!(
        result.model_attempts.len(),
        expected_launches,
        "every LAUNCH gets its own attempt row (pi pushes inside the startup loop): {:?}",
        result.model_attempts
    );
    assert_eq!(
        result.attempted_models.len(),
        1,
        "a startup relaunch is the SAME rung of the model ladder, recorded once \
         (`execution.ts:1536-1539`): {:?}",
        result.attempted_models
    );
    for row in result.model_attempts.iter().take(expected_launches - 1) {
        let note = row.error.clone().unwrap_or_default();
        assert!(
            note.starts_with("[startup-retry] fixture-model exited before model or tool activity"),
            "each relaunched row carries pi's retry note verbatim; got {note:?}"
        );
    }
    assert_eq!(
        result.error.as_deref(),
        Some(
            format_subagent_startup_retry_exhausted_error("fixture-model", expected_launches)
                .as_str()
        ),
        "the terminal error is pi's exhausted-startup text: {result:?}"
    );
    assert_ne!(result.exit_code, 0, "{result:?}");

    // Four LAUNCHES means four real child processes, each with its own NDJSON tee artifact.
    let scratch = cyrup_ext_subagents::background::attempt_scratch_dir(dir.path());
    for index in 0..expected_launches {
        assert!(
            scratch.join(format!("attempt-{index}.jsonl")).exists(),
            "attempt-{index}.jsonl must exist — each relaunch is a fresh OS subprocess, never an \
             in-process retry"
        );
    }
}

/// MIRROR: a child that DID something and then failed is not a startup failure — it is launched
/// exactly once. This is the boundary that matters: relaunching here would triple the cost of a
/// genuinely failing model and hide the failure behind a startup story.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_that_produced_output_before_failing_is_not_relaunched() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [{"kind": "emit", "line": message_end_line("I ran, then broke")}],
        "exit_code": 3
    });
    let result = run_fixture(dir.path(), &script, "ran-then-failed.json").await;

    assert_eq!(
        result.model_attempts.len(),
        1,
        "a child with real activity must be launched ONCE: {:?}",
        result.model_attempts
    );
    let error = result.error.clone().unwrap_or_default();
    assert!(
        !error.contains("failed to start"),
        "this is not a startup failure and must not be reported as one: {error}"
    );
    assert!(
        !cyrup_ext_subagents::background::attempt_scratch_dir(dir.path())
            .join("attempt-1.jsonl")
            .exists(),
        "no second child may be spawned"
    );
}

/// MIRROR: a child that dies with STDERR to show for itself has explained itself, and is likewise
/// launched once — pi's `!evidence.error?.trim()` leg (`subagent-startup-retry.ts:52`), reached in
/// cyrup through the stderr-into-error surfacing (`execution.ts:686`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_that_died_with_a_diagnostic_is_not_relaunched() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [{"kind": "emit_stderr", "line": "fatal: model 'fixture-model' is not configured"}],
        "exit_code": 3
    });
    let result = run_fixture(dir.path(), &script, "died-loudly.json").await;

    assert_eq!(
        result.model_attempts.len(),
        1,
        "a diagnosed failure is not a silent startup failure: {:?}",
        result.model_attempts
    );
    assert_eq!(
        result.error.as_deref(),
        Some("fatal: model 'fixture-model' is not configured"),
        "the child's own diagnostic must survive as the run's error: {result:?}"
    );
}
