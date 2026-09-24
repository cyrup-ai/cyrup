//! UW-3 — the parent reads an ARMED child's watchdog status events, through the real production
//! path (`exec::run_sync` → `build_attempt_spawn_plan` → `SpawnedChild::spawn` → `drive_attempt`)
//! against the scripted `cyrup-subagent-fixture` child.
//!
//! Before the fold existed, a child's `subagent.watchdog.status` line parsed to
//! `SubagentEvent::Unknown` and was dropped. An armed child that finished its turn and then started
//! its watchdog review was force-drained 1 s after its final `message_end` — mid-review — and the
//! run was coerced to exit 0 with the pre-review answer (`.flux/todo/CHILD_STATUS_EVENTS.md`).
//!
//! The child here is armed exactly as a real one is: the run's own project settings turn the child
//! watchdog on, and the parent encodes the resolved config into the child's env. Each test first
//! ASSERTS that env var is present (via the same public plan builder the spawn uses) and reads the
//! identity out of it, so the scripted status lines carry the identity the parent actually
//! encoded — not upstream's literal `index ?? 0`, which cyrup does not write for an unindexed run.

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
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions, SingleResult};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;
use cyrup_ext_subagents::watchdog::child_status::{
    CHILD_WATCHDOG_CONFIG_ENV, ChildWatchdogConfig, ChildWatchdogPhase,
    decode_child_watchdog_config,
};

const TAIL_TIMEOUT_MS: u64 = 4000;

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

fn agent_config() -> AgentConfig {
    AgentConfig {
        machine: None,
        acceptance_role: None,
        default_acceptance: None,
        name: "worker".to_string(),
        model: Some(ModelId::from("fixture-model")),
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
        inherit_global_context: false, // SUBA-101: parser default
        mutation_tools: None,          // SUBA-102: built-in set only
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        memory: None,
        tool_budget: None,
        runner: None,
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

fn run_options(cwd: &Path) -> RunOptions {
    RunOptions {
        parent_env_overrides: std::collections::BTreeMap::new(),
        machine: None,
        model_exclusions: None,
        fast: false,
        host_available_builtins: None,
        structured_output_dir: None,
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        turn_budget: None,
        permission_rules: None,
        thinking_ceiling: None,
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
        available_models: vec![ModelId::from("fixture-model")],
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
        // A run id but NO child index: exactly the shape whose `childIndex` cyrup omits from the
        // encoded config, which is the trap upstream's `options.index ?? 0` would fall into.
        run_id: Some(cyrup_ext_subagents::background::RunId::from_token(
            "uw3-run",
        )),
        child_index: None,
        steer_inbox_dir: None,
        steer_ack_dir: None,
        steer_capability_path: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        transcript: None,
        model_scope: None,
    }
}

/// Arm the child watchdog in `dir`'s own project settings, as an operator does.
fn arm(dir: &Path) {
    let settings = dir.join(".cyrup").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        serde_json::json!({
            "subagents": { "watchdog": {
                "enabled": true,
                "children": { "enabled": true, "watchdogTailTimeoutMs": TAIL_TIMEOUT_MS },
            }}
        })
        .to_string(),
    )
    .unwrap();
}

/// The config the parent will encode for this run — read through the SAME public plan builder the
/// spawn uses. `None` when the run is unarmed. Asserting on this before relying on it is what keeps
/// an unarmed-by-accident fixture from passing a test vacuously.
fn encoded_config(dir: &Path) -> Option<ChildWatchdogConfig> {
    let plan = cyrup_ext_subagents::exec::build_attempt_spawn_plan(
        &agent_config(),
        &ModelId::from("fixture-model"),
        "do the thing",
        &run_options(dir),
        DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
        dir,
        None,
    )
    .expect("plan builds");
    let raw = plan.spec.env_overlay.get(CHILD_WATCHDOG_CONFIG_ENV)?;
    let decoded = decode_child_watchdog_config(Some(raw.as_str()))
        .expect("the parent writes a decodable config")
        .expect("enabled");
    assert_eq!(
        Some(&decoded),
        plan.child_watchdog.as_ref(),
        "the plan carries out EXACTLY the config it encoded"
    );
    Some(decoded)
}

fn armed_config(dir: &Path) -> ChildWatchdogConfig {
    arm(dir);
    let config = encoded_config(dir).expect("the child watchdog env var is present");
    assert_eq!(
        config.child_index, None,
        "an unindexed run encodes no childIndex"
    );
    assert_eq!(config.run_id.as_deref(), Some("uw3-run"));
    assert_eq!(config.watchdog_tail_timeout_ms, TAIL_TIMEOUT_MS);
    config
}

/// One status line with the identity `config` names, exactly as `register_child.rs` emits it.
fn status_line(config: &ChildWatchdogConfig, seq: u64, phase: &str) -> String {
    let mut event = serde_json::json!({
        "type": "subagent.watchdog.status",
        "seq": seq,
        "phase": phase,
        "ts": 1_700_000_000_000_i64 + seq as i64,
        "followUpPending": false,
    });
    if let Some(run_id) = &config.run_id {
        event["runId"] = serde_json::json!(run_id);
    }
    if let Some(agent) = &config.agent {
        event["agent"] = serde_json::json!(agent);
    }
    if let Some(index) = config.child_index {
        event["childIndex"] = serde_json::json!(index);
        event["stepIndex"] = serde_json::json!(index);
    }
    event.to_string()
}

fn emit(line: String) -> serde_json::Value {
    serde_json::json!({"kind": "emit", "line": line})
}

fn sleep(ms: u64) -> serde_json::Value {
    serde_json::json!({"kind": "sleep_ms", "ms": ms})
}

async fn run(dir: &Path, steps: Vec<serde_json::Value>, name: &str) -> (SingleResult, Duration) {
    let script = serde_json::json!({ "steps": steps, "exit_code": 0 });
    let script_path = write_script(dir, name, &script);
    let mut opts = run_options(dir);
    opts.spawn_command = Some(SpawnCommand {
        binary: crate::support::bins::subagent_fixture(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    });
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        cyrup_ext_subagents::exec::run_sync(&agent_config(), "do the thing", &opts),
    )
    .await
    .expect("run_sync must not hang against a scripted child");
    (result, started.elapsed())
}

/// **T1 — the row's observable.** An armed child finishes its turn, reports `reviewing`, reviews
/// for 2.5 s, reports `idle`, settles and exits on its own. The parent must hold the run open for
/// the whole review and record the child's final phase.
///
/// Over the pre-fix parent the `reviewing` line was dropped, the 1 s final-drain window fired
/// mid-review and the child was SIGINT/SIGTERM'd: elapsed ≈ 1 s, a process signal, and no
/// `watchdog` on the result. Killing mutations: deleting the fold; `child_watchdog_is_active` →
/// `false`; not clearing `final_drain_at` on an active event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_armed_child_is_not_drained_while_its_watchdog_reviews() {
    let dir = tempfile::tempdir().unwrap();
    let config = armed_config(dir.path());
    let (result, elapsed) = run(
        dir.path(),
        vec![
            emit(message_end_line("the reviewed answer")),
            emit(status_line(&config, 1, "reviewing")),
            sleep(2500),
            emit(status_line(&config, 2, "idle")),
            emit("{\"type\":\"agent_settled\"}".to_string()),
        ],
        "t1.json",
    )
    .await;
    assert!(
        elapsed >= Duration::from_millis(2500),
        "the parent must wait out the review, not drain at 1 s: {elapsed:?}"
    );
    assert_eq!(
        result.process_signal, None,
        "the child exited on its own: {result:?}"
    );
    assert_eq!(result.exit_code, 0, "{result:?}");
    let watchdog = result
        .watchdog
        .as_ref()
        .expect("the folded child watchdog is on the result");
    assert_eq!(watchdog.phase, ChildWatchdogPhase::Idle);
    assert_eq!(watchdog.seq, 2);
    assert_eq!(watchdog.timed_out, None);
}

/// **T2 — the tail.** A child that reports `reviewing` and then never finishes is declared stale
/// by the PARENT after `watchdogTailTimeoutMs` and drained — not waited out for its 30 s. Killing
/// mutations: removing the tail arm (the run hangs for the child's full sleep); a tail that never
/// starts the drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_review_that_never_finishes_is_marked_stale_by_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    let config = armed_config(dir.path());
    let (result, elapsed) = run(
        dir.path(),
        vec![
            emit(message_end_line("answer")),
            emit(status_line(&config, 1, "reviewing")),
            sleep(30_000),
        ],
        "t2.json",
    )
    .await;
    assert!(
        elapsed < Duration::from_secs(10),
        "drained after the tail: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(TAIL_TIMEOUT_MS),
        "held for the whole tail allowance: {elapsed:?}"
    );
    let watchdog = result.watchdog.as_ref().expect("snapshot recorded");
    assert_eq!(watchdog.phase, ChildWatchdogPhase::Stale);
    assert_eq!(watchdog.timed_out, Some(true));
    assert_eq!(
        watchdog.reason.as_deref(),
        Some("child watchdog tail timeout")
    );
    assert_eq!(watchdog.seq, 2, "the parent's stale mark is the next seq");
}

/// **T3 — identity.** Status lines from ANOTHER run are ignored: the child is drained at ~1 s as
/// before and no snapshot is recorded. Killing mutation: dropping the identity filter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_event_for_another_run_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let mut foreign = armed_config(dir.path());
    foreign.run_id = Some("someone-else".to_string());
    let (result, elapsed) = run(
        dir.path(),
        vec![
            emit(message_end_line("answer")),
            emit(status_line(&foreign, 1, "reviewing")),
            sleep(2500),
            emit(status_line(&foreign, 2, "idle")),
        ],
        "t3.json",
    )
    .await;
    assert!(
        elapsed < Duration::from_millis(2400),
        "drained on the 1 s window: {elapsed:?}"
    );
    assert!(result.watchdog.is_none(), "{:?}", result.watchdog);
}

/// **T4 — unarmed.** With no child watchdog configured the parent does not hold the run for a
/// status line (upstream `if (!childWatchdog) return`). Killing mutation: dropping that gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unarmed_parent_does_not_hold_for_status_events() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        encoded_config(dir.path()).is_none(),
        "no settings, no env var"
    );
    // The identity an ARMED run of the same shape would encode, taken from a second, armed dir —
    // so the lines below are exactly what an armed child would print.
    let armed_dir = tempfile::tempdir().unwrap();
    let identity = armed_config(armed_dir.path());
    let (result, elapsed) = run(
        dir.path(),
        vec![
            emit(message_end_line("answer")),
            emit(status_line(&identity, 1, "reviewing")),
            sleep(2500),
            emit(status_line(&identity, 2, "idle")),
        ],
        "t4.json",
    )
    .await;
    assert!(
        elapsed < Duration::from_millis(2400),
        "drained on the 1 s window: {elapsed:?}"
    );
    assert!(result.watchdog.is_none());
}

/// **T5 — retry.** `agent_end{willRetry:true}` clears the watchdog tail as well as the drain: a
/// stale tail must not fire mid-retry. The retry takes longer than the tail, so a tail left armed
/// would drain the child before its post-retry answer. Killing mutation: `CancelDrain` not clearing
/// `watchdog_tail_at`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_clears_the_watchdog_tail() {
    let dir = tempfile::tempdir().unwrap();
    let config = armed_config(dir.path());
    let (result, _) = run(
        dir.path(),
        vec![
            emit(message_end_line("before the retry")),
            emit(status_line(&config, 1, "reviewing")),
            emit("{\"type\":\"agent_end\",\"willRetry\":true}".to_string()),
            // Long enough that a tail left armed by the retry (fires at TAIL) PLUS the drain it
            // starts (1 s later) both expire well before the post-retry answer.
            sleep(TAIL_TIMEOUT_MS + 2500),
            emit(message_end_line("after the retry")),
            emit(status_line(&config, 2, "idle")),
            emit("{\"type\":\"agent_settled\"}".to_string()),
        ],
        "t5.json",
    )
    .await;
    assert_eq!(
        result.final_output.as_deref(),
        Some("after the retry"),
        "{result:?}"
    );
    assert_eq!(result.process_signal, None, "not force-drained: {result:?}");
}

/// **T6 — sequence.** A replayed older event cannot walk the phase back: `reviewing` at seq 2, then
/// a stale `idle` at seq 1, and the run is still held. Killing mutation: removing the
/// `seq <= current.seq` check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_older_status_cannot_release_the_hold() {
    let dir = tempfile::tempdir().unwrap();
    let config = armed_config(dir.path());
    let (result, elapsed) = run(
        dir.path(),
        vec![
            emit(message_end_line("answer")),
            emit(status_line(&config, 2, "reviewing")),
            emit(status_line(&config, 1, "idle")),
            sleep(1200),
            emit(status_line(&config, 3, "idle")),
            emit("{\"type\":\"agent_settled\"}".to_string()),
        ],
        "t6.json",
    )
    .await;
    assert!(
        elapsed >= Duration::from_millis(1200),
        "still held: {elapsed:?}"
    );
    assert_eq!(result.process_signal, None, "{result:?}");
    assert_eq!(result.watchdog.as_ref().map(|w| w.seq), Some(3));
}
