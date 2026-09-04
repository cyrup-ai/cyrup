//! FULLY-WIRED PROOFS (real OS subprocesses, no mocks) for the parts of the attempt lifecycle whose
//! behaviour is only visible when a REAL child is driven end-to-end through `exec::run_sync`.
//!
//! Each test here covers a behaviour that a mutation proved was unguarded — the implementation could
//! be deleted outright and the entire suite stayed green:
//!
//! - **The startup-retry backoff's Cancelled/Interrupted lifecycle** (pi
//!   `waitForSubagentStartupRetry`, `subagent-startup-retry.ts:86-104`, branched at
//!   `execution.ts:1583-1600`). The whole body of the production `wait_startup_retry` override —
//!   both already-aborted checks and both `select!` arms — could collapse to `sleep(delay).await;
//!   Proceed` with nothing to show for it, including the branch that produces pi's PAUSED-SUCCESS.
//! - **`apply_startup_outcome`'s `Cancelled`/`Exhausted` arms** (pi `execution.ts:1594-1616`), which
//!   are what put the diagnosis into `finalOutput` rather than leaving it only in `error`.
//! - **The retry note reaching the RELAUNCHED child's context** (pi `attemptNotes.push(retryNote)`
//!   at `execution.ts:1603`, read back as `recentOutput: [...shared.attemptNotes]` at `:432`).
//! - **The `protocol_error` promotion's PRIORITY** over `spawn_error`/`trailing_assistant_error`
//!   (pi `execution.ts:1099`: `if (!result.error && closeError) result.error = closeError` — the
//!   close handler only fills in what `failProtocol` has not already set).
//! - **The final-stop drain window arming on a terminal assistant stop** (pi
//!   `projectChildLifecycle`, `child-protocol.ts:400`, applied at `execution.ts:947`) — previously
//!   guarded ONLY by a pure-function projection table, with no subprocess-level coverage at all.
//!
//! Gated on `test-fixtures` (matching the `cyrup-subagent-fixture` `[[bin]]` `required-features`).

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
    format_subagent_startup_retry_note,
};
use cyrup_ext_subagents::exec::output::{INTERRUPTED_FINAL_OUTPUT, OutputCap};
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions, SingleResult};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
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

/// A `message_end` carrying an assistant-side `errorMessage` — pi's `assistantError`, the thing the
/// close handler would otherwise promote into `result.error` (`execution.ts:476`).
fn message_end_with_error(text: &str, error: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "errorMessage": error,
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
async fn run_fixture_with(
    dir: &Path,
    script: &serde_json::Value,
    name: &str,
    mut opts: RunOptions,
) -> SingleResult {
    let script_path = write_script(dir, name, script);
    // This run names its own binary and script rather than moving process-global state every
    // concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    });
    let agent = base_agent_config("fixture-model");
    tokio::time::timeout(
        Duration::from_secs(60),
        cyrup_ext_subagents::exec::run_sync(&agent, "do the thing", &opts),
    )
    .await
    .expect("run_sync must not hang against a scripted fixture child")
}

async fn run_fixture(dir: &Path, script: &serde_json::Value, name: &str) -> SingleResult {
    let opts = base_run_options(dir, "fixture-model");
    run_fixture_with(dir, script, name, opts).await
}

/// When the two lifecycle tests below fire their signal.
///
/// The zero-activity child exits within milliseconds, so almost all of the ladder's wall time is
/// spent in its three backoffs (250 + 750 + 1500 ms). The LAST one is by far the widest, and on the
/// nominal timeline it spans roughly 1.1 s -> 2.6 s from the start of the run — so firing at 1.8 s
/// lands mid-window with several hundred milliseconds of margin on both sides, making this
/// insensitive to spawn-time jitter in a way a signal aimed at the 250 ms first backoff is not.
///
/// It also makes both tests discriminate sharply: a `wait_startup_retry` that ignored its signals
/// would sleep every backoff out and still be on its FOURTH launch by the time this fires, which
/// the launch-count assertions below reject.
const SIGNAL_AT: Duration = Duration::from_millis(1800);

/// The shape of a child that never got as far as running: nothing on stdout, nothing on stderr, a
/// bare non-zero exit. This is what arms the startup-retry ladder.
fn zero_activity_script() -> serde_json::Value {
    serde_json::json!({ "steps": [], "exit_code": 3 })
}

// =================================================================================================
// The startup-retry backoff lifecycle (item 3d / 3e)
// =================================================================================================

/// A HARD CANCEL landing during a startup-retry backoff abandons the run with pi's cancellation
/// text — it does not sleep the backoff out and relaunch a child into a cancelled run.
///
/// pi's `waitForSubagentStartupRetry` races the delay against both lifecycle signals
/// (`subagent-startup-retry.ts:86-104`) and its caller then branches on WHICH one fired
/// (`execution.ts:1583-1600`); the `else` leg — a cancel, not an interrupt — is this one:
///
/// ```text
/// const cancellationError = "Subagent startup retry cancelled before relaunch.";
/// result.error = cancellationError;
/// result.finalOutput = cancellationError;
/// ```
///
/// The child exits instantly with zero activity, so the ladder enters its first 250 ms backoff
/// almost immediately; cancelling at 120 ms lands inside that window. The assertions are chosen so
/// a `wait_startup_retry` that merely slept would fail all three: it would run the full launch
/// budget and report EXHAUSTION, not cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_during_the_startup_backoff_abandons_the_run_before_relaunching() {
    let dir = tempfile::tempdir().expect("tempdir");

    let opts = base_run_options(dir.path(), "fixture-model");
    let cancel = opts.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(SIGNAL_AT).await;
        cancel.cancel();
    });

    let result = run_fixture_with(
        dir.path(),
        &zero_activity_script(),
        "cancel-in-backoff.json",
        opts,
    )
    .await;

    const CANCELLED: &str = "Subagent startup retry cancelled before relaunch.";
    assert_eq!(
        result.error.as_deref(),
        Some(CANCELLED),
        "a cancel inside the backoff is pi's cancellation branch, not an exhausted ladder: {result:?}"
    );
    // `apply_startup_outcome`'s `Cancelled` arm — the half that puts the diagnosis in the OUTPUT,
    // not merely the error field.
    assert_eq!(
        result.final_output.as_deref(),
        Some(CANCELLED),
        "pi sets `result.finalOutput = cancellationError` alongside `result.error` \
         (`execution.ts:1596-1597`): {result:?}"
    );
    assert!(
        result.model_attempts.len() < SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1,
        "the ladder must STOP at the cancel — spending the whole launch budget means the backoff \
         ignored the signal: {:?}",
        result.model_attempts
    );
    assert_ne!(
        result.exit_code, 0,
        "a cancelled run is not a success: {result:?}"
    );
}

/// A SOFT INTERRUPT landing during a startup-retry backoff is pi's PAUSED SUCCESS: exit 0, a
/// CLEARED error, and the paused sentinel as the output (`execution.ts:1584-1592`):
///
/// ```text
/// result.exitCode = 0;
/// result.interrupted = true;
/// result.error = undefined;
/// result.finalOutput = "Interrupted. Waiting for explicit next action.";
/// ```
///
/// This is the branch most worth guarding, because getting it wrong is silent: an interrupt
/// reported as a failure looks exactly like a subagent that broke, and the run it paused is the one
/// the user asked to pause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupt_during_the_startup_backoff_is_a_paused_success_not_a_failure() {
    let dir = tempfile::tempdir().expect("tempdir");

    let opts = base_run_options(dir.path(), "fixture-model");
    let interrupt = opts.interrupt.clone();
    tokio::spawn(async move {
        tokio::time::sleep(SIGNAL_AT).await;
        interrupt.cancel();
    });

    let result = run_fixture_with(
        dir.path(),
        &zero_activity_script(),
        "interrupt-in-backoff.json",
        opts,
    )
    .await;

    assert_eq!(
        result.exit_code, 0,
        "a soft interrupt is a PAUSE, coerced to exit 0: {result:?}"
    );
    assert_eq!(
        result.error, None,
        "and its error is explicitly CLEARED — reporting one turns a pause into a failure: {result:?}"
    );
    // `apply_startup_outcome`'s `Interrupted` arm.
    assert_eq!(
        result.final_output.as_deref(),
        Some(INTERRUPTED_FINAL_OUTPUT),
        "the paused sentinel is what the caller renders: {result:?}"
    );
    assert!(
        result.model_attempts.len() < SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1,
        "the ladder must stop at the interrupt rather than spending the whole budget: {:?}",
        result.model_attempts
    );
}

/// The MIRROR that keeps the two tests above honest: with NO signal firing, the same child runs the
/// backoff ladder to completion and reports EXHAUSTION. Without this, "stopped early" could be the
/// only behaviour the suite ever exercises, and a `wait_startup_retry` that always returned
/// `Cancelled` would pass both tests above.
///
/// This also pins `apply_startup_outcome`'s `Exhausted` arm: pi sets `result.finalOutput =
/// startupError` as well as `result.error` (`execution.ts:1610-1611`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_signal_the_backoff_ladder_runs_to_exhaustion_and_reports_it_as_the_output() {
    let dir = tempfile::tempdir().expect("tempdir");

    let result = run_fixture(dir.path(), &zero_activity_script(), "exhausted.json").await;

    let expected_launches = SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1;
    let exhausted =
        format_subagent_startup_retry_exhausted_error("fixture-model", expected_launches);
    assert_eq!(
        result.model_attempts.len(),
        expected_launches,
        "an undisturbed backoff spends the whole launch budget: {:?}",
        result.model_attempts
    );
    assert_eq!(
        result.error.as_deref(),
        Some(exhausted.as_str()),
        "{result:?}"
    );
    assert_eq!(
        result.final_output.as_deref(),
        Some(exhausted.as_str()),
        "`apply_startup_outcome`'s Exhausted arm puts the diagnosis in the OUTPUT too, not only \
         in `error` (pi `execution.ts:1610-1611`): {result:?}"
    );
}

// =================================================================================================
// The retry note reaching the relaunched child (item 3f)
// =================================================================================================

/// The startup-retry note must reach the LIVE progress surface of the relaunched attempt, not
/// merely be stamped on the previous attempt's error row.
///
/// pi writes it twice, to two different places, at `execution.ts:1602-1603`:
///
/// ```text
/// attempt.error = retryNote;      // the per-attempt history row
/// attemptNotes.push(retryNote);   // the context every LATER attempt is constructed with
/// ```
///
/// The second read-back is `recentOutput: [...shared.attemptNotes]` (`:432`), and pi streams that
/// same live `progress` object through `fireUpdate()` for the whole attempt. So the note is the
/// user's only explanation of why their run was silently relaunched, and it must arrive WHILE the
/// relaunch is happening: a settled snapshot cannot carry it, because `compactCompletedProgress`
/// (`shared/utils.ts:330-347`) empties `recentOutput` as one of its two growth terms.
///
/// The existing tests read only `ModelAttempt::error` — the FIRST write — so the second was
/// unguarded. It was in fact worse than untested: cyrup's live surface folds the child's raw NDJSON,
/// on which a parent-side note never appears, so the note went into an `AgentProgress` ring that
/// `compact_completed` then emptied, and it reached no surface at all. The
/// `LiveEventSink::emit_note` channel is what closes that; this test drives the real streaming path
/// (`run_foreground_streaming`) and asserts the note arrives on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_startup_retry_note_reaches_the_live_progress_surface_of_the_relaunch() {
    use std::sync::{Arc, Mutex};

    use cyrup_core::{ToolUpdate, ToolUpdateSink};
    use cyrup_ext_subagents::discovery::types::AgentReadScope;
    use cyrup_ext_subagents::extension::{
        ForegroundRunRequest, SingleRunOverrides, SubagentExecutor,
    };
    use cyrup_ext_subagents::tui::events::SubagentUpdatePayload;

    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home tempdir");

    // A discoverable PROJECT persona, so the streaming entry point can resolve an agent to run.
    let agents_dir = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
    std::fs::write(
        agents_dir.join("streamtest.md"),
        "---\nname: streamtest\ndescription: fixture streaming persona\nmodel: fixture-model\n\
         systemPromptMode: replace\ntools: read\n---\nYou are a fixture agent.\n",
    )
    .expect("write persona");

    let script_path = write_script(dir.path(), "note-live.json", &zero_activity_script());
    let fixture = fixture_binary_path();

    let updates: Arc<Mutex<Vec<ToolUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_updates = Arc::clone(&updates);
    let on_update: ToolUpdateSink = Box::new(move |u: ToolUpdate| {
        if let Ok(mut guard) = sink_updates.lock() {
            guard.push(u);
        }
    });

    let executor = SubagentExecutor::with_config(SubagentExtensionConfig {
        spawn_command: Some(SpawnCommand {
            binary: fixture,
            base_args: vec![
                "--fixture-script".to_string(),
                script_path.display().to_string(),
            ],
        }),
        roots: Roots::sandboxed(home.path()),
        ..SubagentExtensionConfig::default()
    });
    let streamed = tokio::time::timeout(
        Duration::from_secs(60),
        executor.run_foreground_streaming(
            ForegroundRunRequest {
                overrides: SingleRunOverrides::default(),
                cwd: dir.path(),
                agent_name: "streamtest",
                task: "Research the topic",
                agent_scope: AgentReadScope::Both,
                context: None,
                model_override: None,
                timeout_ms: None,
                cancel: CancelToken::new(),
            },
            on_update,
        ),
    )
    .await;
    streamed
        .expect("the streaming run must not hang")
        .expect("run_foreground_streaming resolves the persona and completes");

    let captured = updates.lock().expect("updates lock").clone();
    let payloads: Vec<SubagentUpdatePayload> = captured
        .iter()
        .filter_map(|u| {
            u.details
                .as_ref()
                .and_then(|d| serde_json::from_value::<SubagentUpdatePayload>(d.clone()).ok())
        })
        .collect();

    // Every relaunch's note, in the exact text pi formats. All of them must appear: each explains
    // one relaunch, and a surface that shows only the last one hides the earlier failures.
    for launch in 1..=SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() {
        let expected = format_subagent_startup_retry_note(
            "fixture-model",
            launch,
            SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1,
            SUBAGENT_STARTUP_RETRY_DELAYS_MS[launch - 1],
        );
        assert!(
            payloads.iter().any(|p| p
                .progress
                .iter()
                .any(|pr| pr.recent_output.iter().any(|line| line == &expected))),
            "the live progress surface must carry the retry note for launch {launch} — it is the \
             only explanation the user gets for a silent relaunch (pi `attemptNotes.push`, \
             `execution.ts:1603`, read back at `:432`). Saw recent_output {:?}",
            payloads
                .iter()
                .flat_map(|p| p.progress.iter())
                .map(|pr| pr.recent_output.clone())
                .collect::<Vec<_>>()
        );
    }
}

// =================================================================================================
// The protocol_error promotion's PRIORITY (item 3g)
// =================================================================================================

/// A protocol-output-limit diagnostic OUTRANKS a trailing assistant error.
///
/// pi's `failProtocol` assigns `result.error` at the instant the cap trips (`execution.ts:1026-1041`),
/// and the close handler only ever fills in what is still unset (`execution.ts:1026-1041`:
/// `if (!result.error && closeError) result.error = closeError`) — where `closeError` is
/// `result.error ?? toolDiagnosticError ?? assistantError` (`:1080`). So when a child both blows the
/// cap AND leaves a trailing assistant `errorMessage`, the protocol diagnostic is what survives.
///
/// The ordering is the whole content of this test: a child emitting only ONE of the two proves
/// nothing about which wins, and moving the `protocol_error` promotion below the assistant-error
/// promotion leaves every such single-cause test green. This matters because the protocol
/// diagnostic names the actual, actionable fault (a child whose output cannot be read), while the
/// assistant error is downstream noise from the same event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_protocol_limit_outranks_a_trailing_assistant_error() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            // A trailing assistant error lands FIRST, so it is already recorded when the cap trips.
            {"kind": "emit", "line": message_end_with_error(
                "partial", "the assistant's own trailing error message")},
            // ...then the child blows the per-line cap with a non-projectable record.
            {"kind": "emit_padded",
             "head": "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":\"",
             "pad_bytes": MAX_CHILD_PENDING_LINE_BYTES + 4096,
             "tail": "\"}}"}
        ],
        "exit_code": 0
    });
    let result = run_fixture(dir.path(), &script, "protocol-vs-assistant.json").await;

    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.starts_with("protocol_output_limit:"),
        "the protocol diagnostic is set at the moment the cap trips and must not be overwritten by \
         the close handler's assistant error; got {error:?}"
    );
    assert!(
        !error.contains("the assistant's own trailing error message"),
        "the assistant error must not displace it: {error}"
    );
    assert_ne!(result.exit_code, 0, "{result:?}");
}

/// MIRROR: with no protocol violation, the SAME trailing assistant error is exactly what gets
/// reported. Without this, "the error starts with `protocol_output_limit:`" could be satisfied by an
/// implementation that ignores assistant errors altogether.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trailing_assistant_error_is_reported_when_nothing_outranks_it() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_with_error(
                "partial", "the assistant's own trailing error message")}
        ],
        "exit_code": 0
    });
    let result = run_fixture(dir.path(), &script, "assistant-error-alone.json").await;

    assert_eq!(
        result.error.as_deref(),
        Some("the assistant's own trailing error message"),
        "an uncontested trailing assistant error IS the run's error: {result:?}"
    );
}

// =================================================================================================
// G76: the final-stop drain window, driven by a REAL child (item 4)
// =================================================================================================

/// A CLEAN TERMINAL ASSISTANT STOP arms the final-stop grace window, so a child that says its final
/// word and then refuses to exit is force-drained rather than waited out.
///
/// pi `projectChildLifecycle` (`child-protocol.ts:400`) returns `"start-drain"` for
/// `terminalAssistantStop`, applied at `execution.ts:920`. That arm is a PRE-EXISTING, load-bearing
/// behaviour — it is the only thing standing between "the assistant finished" and a session that
/// hangs on a child holding stdout open — and re-plumbing it through the projection table left it
/// with no subprocess-level coverage: deleting the arm reddened exactly one pure-function table
/// test while every integration test, including the `will_retry` cancel test, stayed green.
///
/// The distinction from the existing `agent_settled` test is the trigger: this child emits NO
/// `agent_settled` at all, so a terminal assistant stop is the only thing that can arm the window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_assistant_stop_arms_the_drain_so_a_hanging_child_is_force_drained() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            // `stopReason: "stop"` with no `errorMessage` — a CLEAN terminal assistant stop, and
            // deliberately the ONLY lifecycle signal this child ever emits.
            {"kind": "emit", "line": message_end_line("the final answer")},
            {"kind": "sleep_ms", "ms": 30000}
        ],
        "exit_code": 0
    });

    let started = Instant::now();
    let result = run_fixture(dir.path(), &script, "terminal-stop-hang.json").await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(15),
        "the child must be force-drained on the ~1s grace window the terminal stop armed, not \
         waited out for its full 30s sleep; took {elapsed:?}"
    );
    assert_eq!(
        result.exit_code, 0,
        "a child force-drained AFTER a clean terminal stop is pi's `forcedDrainAfterFinalSuccess` \
         (`execution.ts:1080`), coerced to exit 0: {result:?}"
    );
    assert_eq!(
        result.final_output.as_deref(),
        Some("the final answer"),
        "and its final word is still the run's answer: {result:?}"
    );
    assert_eq!(result.error, None, "{result:?}");
}
