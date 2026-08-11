//! Integration tests for T0.1 (real named-persona resolution for chain/parallel/background steps,
//! C13) and T0.3 (child recursion-depth increment at the spawn boundary, C15) — both proven end to
//! end against the REAL scripted `cyrup-subagent-fixture` subprocess (arch-SA §11), never a mock,
//! matching this crate's standing testing convention (`tests/exec_run_sync_integration.rs`,
//! `tests/background_runner_main_integration.rs`).
//!
//! The single observation channel both tiers rely on is the per-attempt raw-stdout **tee**
//! `exec::run_sync` writes for every spawned child (`<cwd>/.cyrup-subagent-scratch/attempt-0.jsonl`,
//! R-SA-058): `consume_stdout` tees EVERY raw line — including the fixture's `echo_argv`/`echo_env`
//! NDJSON lines — so a test can read back exactly what argv and env the real child actually
//! received, which is what makes the persona's system prompt / tool allowlist (T0.1) and the child's
//! inherited `CYRUP_SUBAGENT_DEPTH`/`_MAX_DEPTH` (T0.3) directly observable.
//!
//! Separate compilation unit from `lib.rs`, so NOT bound by that crate's `#![forbid(unsafe_code)]`;
//! the `unsafe` env mutation (Rust 2024 requires it for `std::env::set_var`/`remove_var`) is scoped
//! and serialized under [`ENV_MUTATION_LOCK`], exactly like every other integration test here.
//!
//! Gated on `test-fixtures` (the `cyrup-subagent-fixture` `[[bin]]`'s own `required-features` gate):
//! without it the fixture is never built and this file compiles to an empty, passing test list.

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, run};
use cyrup_ext_subagents::background::{
    ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, run_artifact_roots,
};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode, ToolRef};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, ResolvedAgentPersona, RunOptions};
use cyrup_ext_subagents::extension::{BackgroundStepsSpec, SubagentExecutor, SubagentsExtension};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::registration::{DynamicFanoutConfig, ExtensionChainConfig, SubagentExtensionConfig};
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

fn write_script(dir: &Path, name: &str, script_json: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script_json.to_string()).expect("write fixture script");
    path
}

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// Read the per-attempt raw-stdout tee `exec::run_sync` wrote for the first (and here only) spawn
/// attempt of a step whose child ran in `cwd`. The tee holds every raw NDJSON line the real child
/// emitted, including the fixture's `echo_argv`/`echo_env` lines.
fn read_attempt_tee(cwd: &Path) -> String {
    let path = cwd.join(".cyrup-subagent-scratch").join("attempt-0.jsonl");
    std::fs::read_to_string(&path).unwrap_or_default()
}

fn single_step(agent: &str, task: &str) -> SingleStepSpec {
    SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: agent.to_string(),
        task: task.to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    }
}

// =============================================================================================
// T0.1 (C13): a chain step dispatches the REAL named persona — its system prompt AND its tool
// allowlist reach the spawned child — instead of the empty-system-prompt / `--model default` /
// guard-disabled placeholder the runner previously synthesized. Driven through the REAL hop-2
// `run()` loop so the actual `RunnerConfig::resolved_agents` -> `ExecSingleStepExecutor` ->
// `run_single` persona-lookup wiring is exercised, mirroring pi where the child resolves its agent
// config from the already-resolved `agents` list (`chain-execution.ts:1011`).
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_step_dispatches_the_real_named_persona_reaching_the_child_with_its_prompt_and_tools() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // The fixture echoes its own argv back as NDJSON so this test can read exactly what the child
    // received — the persona's `--tools` allowlist and its appended system-prompt body.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("review complete")},
        ],
        "echo_argv": true,
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    const REVIEWER_SYSTEM_PROMPT: &str = "You are the REVIEWER persona. Be rigorous.";
    // The plan-time resolved persona for `reviewer` — Append mode (so the system prompt is composed
    // into the task text handed to the child) with a real tool allowlist. This is exactly what the
    // orchestrator produces via `exec::resolve_step_agent_config` for a discovered `reviewer`
    // agent.
    let reviewer = ResolvedAgentPersona {
        name: "reviewer".to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Append,
        system_prompt_body: REVIEWER_SYSTEM_PROMPT.to_string(),
        tools: Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("grep".to_string()),
        ]),
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
    };
    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert("reviewer".to_string(), reviewer);

    let run_id = RunId::from_token("t01reviewr");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Chain,
        steps: vec![RunnerStep::SingleStep(single_step("reviewer", "Review the diff"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 5,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents,
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
};
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    let outcome = run(&cfg_path, &run_paths).await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    outcome.expect("run() itself never returns Err");

    // The load-bearing proof: the child was actually spawned with the REAL reviewer persona.
    let tee = read_attempt_tee(dir.path());
    assert!(
        !tee.is_empty(),
        "a tee must exist — proving the reviewer persona resolved and the child actually spawned, \
         not an `Unknown agent` short-circuit before any spawn"
    );

    // (1) The persona's TOOL allowlist reached the child as `--tools read,grep` argv.
    let saw_tools_flag = tee.lines().any(|l| l.contains("\"arg\":\"--tools\""));
    let saw_allowlist = tee.lines().any(|l| l.contains("read,grep"));
    assert!(
        saw_tools_flag && saw_allowlist,
        "the reviewer persona's tool allowlist (`--tools read,grep`) must reach the child — the \
         placeholder carried no tools. tee:\n{tee}"
    );

    // (2) The persona's SYSTEM PROMPT (Append mode) was composed into the task text the child
    // received — the placeholder carried an empty system prompt.
    assert!(
        tee.contains("You are the REVIEWER persona"),
        "the reviewer persona's system-prompt body must reach the child, proving `## reviewer` ran \
         the real reviewer, not an empty-system-prompt placeholder. tee:\n{tee}"
    );

    // The terminal records must exist and be internally consistent (the run reached a terminal
    // state; the persona-driven step actually ran).
    let status: RunStatus =
        serde_json::from_slice(&tokio::fs::read(&run_paths.status).await.expect("status.json"))
            .expect("parse status.json");
    let result_file: ResultFile =
        serde_json::from_slice(&tokio::fs::read(&run_paths.result).await.expect("ResultFile"))
            .expect("parse ResultFile");
    assert!(status.state.is_terminal(), "run must reach a terminal state: {status:?}");
    assert_eq!(result_file.results.len(), 1, "the one reviewer step must have produced one result");
    assert_eq!(
        result_file.results[0].agent, "reviewer",
        "the recorded result must attribute the run to the real reviewer agent"
    );
}

// =============================================================================================
// A (pi `originalTask`, `chain-execution.ts:632-652`): a chain step's `{task}` placeholder resolves
// to the run-wide `original_task` the orchestrator serialized into `RunnerConfig` — threaded through
// the runner's `ChainRunContext` into `resolve_step_task`'s `{task}` substitution, so the child is
// dispatched with the SUBSTITUTED task text, not the literal `{task}`. Proven end-to-end by reading
// the substituted task back out of the `echo_argv` raw-stdout tee.
// =============================================================================================

#[tokio::test]
async fn chain_step_task_placeholder_resolves_to_the_configs_original_task() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // The fixture echoes its argv (including the composed prompt/task) so the test can read exactly
    // what the child was dispatched with.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("done")},
        ],
        "echo_argv": true,
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    // A Replace-mode persona with an empty system prompt so the child's task text is the raw
    // (substituted) step task — no appended prompt to obscure the marker.
    let worker = ResolvedAgentPersona {
        name: "worker".to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
    };
    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert("worker".to_string(), worker);

    let run_id = RunId::from_token("taskresolv1");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Chain,
        // The step template references `{task}` — it must be substituted with `original_task`.
        steps: vec![RunnerStep::SingleStep(single_step("worker", "Handle {task} now"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 5,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents,
        original_task: "ORIGINAL_TASK_MARKER".to_string(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
};
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    let outcome = run(&cfg_path, &run_paths).await;
    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    outcome.expect("run() itself never returns Err");

    let tee = read_attempt_tee(dir.path());
    assert!(!tee.is_empty(), "a tee must exist — the child actually spawned");
    assert!(
        tee.contains("Handle ORIGINAL_TASK_MARKER now"),
        "the step's `{{task}}` must resolve to the config's original_task in the dispatched task \
         text. tee:\n{tee}"
    );
    assert!(
        !tee.contains("Handle {task} now"),
        "the literal `{{task}}` placeholder must NOT survive substitution. tee:\n{tee}"
    );
}

// =============================================================================================
// T0.3 (C15, SAFETY): a spawned child inherits `parent_depth + 1`, NOT the parent's own depth
// verbatim — so the recursion guard actually binds across the subprocess boundary. Proven by
// reading the child's echoed `CYRUP_SUBAGENT_DEPTH`/`_MAX_DEPTH` back out of the raw-stdout tee.
// Mirrors pi's `getSubagentDepthEnv` (`recursion-guard.test.ts:210-257`).
// =============================================================================================

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
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
        acceptance: Some(AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![])),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
        model_scope: None,
    }
}

fn depth_echo_agent(model: &str, depth: DepthEnvelope, max_subagent_depth: Option<u32>) -> AgentConfig {
    AgentConfig {
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_output: OutputCap::default(),
        max_subagent_depth,
        memory: None,
        tool_budget: None,
        depth,
    }
}

async fn run_depth_echo_child(dir: &Path, agent: &AgentConfig) -> String {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    // The fixture echoes the two depth env vars it actually inherited back as NDJSON lines.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("done")},
        ],
        "echo_env": ["CYRUP_SUBAGENT_DEPTH", "CYRUP_SUBAGENT_MAX_DEPTH"],
        "exit_code": 0
    });
    let script_path = write_script(dir, "depth-script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let opts = base_run_options(dir, "fixture-model");
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(agent, "do the work", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    assert_eq!(result.exit_code, 0, "the depth-echo child must exit cleanly: {result:?}");

    read_attempt_tee(dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_child_inherits_incremented_depth_not_the_parent_envelope_verbatim() {
    let dir = tempfile::tempdir().expect("real tempdir");
    // This process stands at depth 1 of a ceiling-5 run and declares no tighter agent-level max —
    // so the real child it spawns MUST inherit depth 2 under the same ceiling 5. Before the T0.3
    // fix the child inherited the parent's own depth (1) verbatim and the guard never tripped.
    let agent = depth_echo_agent(
        "fixture-model",
        DepthEnvelope {
            current_depth: 1,
            max_depth: 5,
        },
        None,
    );

    let tee = run_depth_echo_child(dir.path(), &agent).await;

    assert!(
        tee.contains("\"env\":\"CYRUP_SUBAGENT_DEPTH\",\"type\":\"unknown\",\"value\":\"2\""),
        "the child MUST see CYRUP_SUBAGENT_DEPTH=2 (parent 1 + 1), never the parent's own 1 \
         verbatim — this is the C15 recursion-increment fix. tee:\n{tee}"
    );
    assert!(
        tee.contains("\"env\":\"CYRUP_SUBAGENT_MAX_DEPTH\",\"type\":\"unknown\",\"value\":\"5\""),
        "with no agent-level tightening the inherited ceiling 5 passes through unchanged. tee:\n{tee}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_child_depth_env_applies_the_agents_tightening_only_max() {
    let dir = tempfile::tempdir().expect("real tempdir");
    // Inherited ceiling 5, but this agent declares its own tighter ceiling of 2 for its children:
    // the child must inherit depth 1 under ceiling min(5, 2) = 2 (R-SA-056 tightening-only).
    let agent = depth_echo_agent(
        "fixture-model",
        DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
        Some(2),
    );

    let tee = run_depth_echo_child(dir.path(), &agent).await;

    assert!(
        tee.contains("\"env\":\"CYRUP_SUBAGENT_DEPTH\",\"type\":\"unknown\",\"value\":\"1\""),
        "child depth must be parent 0 + 1 = 1. tee:\n{tee}"
    );
    assert!(
        tee.contains("\"env\":\"CYRUP_SUBAGENT_MAX_DEPTH\",\"type\":\"unknown\",\"value\":\"2\""),
        "the agent's tighter declared max (2) must win over the looser inherited ceiling (5) in \
         the child's spawn env. tee:\n{tee}"
    );
}

// =============================================================================================
// T0.3 completeness: a chain step whose inherited envelope is ALREADY at the ceiling trips the
// depth guard and spawns NO further child. Proven through the real `run()` loop: the runner's own
// R-SA-055 guard rejects the whole run up front, so no child is ever spawned and the fixture's
// marker output appears nowhere. (The increment above is what makes a genuinely deep chain reach
// this state across the subprocess boundary in production.)
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_chain_at_the_ceiling_trips_the_guard_and_spawns_no_further_child() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    const NEVER_SPAWNED_MARKER: &str = "THIS-CHILD-MUST-NEVER-RUN";
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line(NEVER_SPAWNED_MARKER)},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — a REAL working fixture, so a failure to spawn
    // is attributable only to the depth guard, not an unreachable binary.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert(
        "reviewer".to_string(),
        ResolvedAgentPersona {
            name: "reviewer".to_string(),
            model: Some(ModelId::from("fixture-model")),
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            output: None,
            inherit_project_context: false,
            inherit_skills: true,
            skills: Vec::new(),
            completion_guard: Some(false),
            max_subagent_depth: None,
            default_context: None,
            memory: None,
            tool_budget: None,
        },
    );

    let run_id = RunId::from_token("t03deep001");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    // A ceiling of 0 with the process's own (absent) depth env resolving to 0 means this run is
    // already blocked — the same terminal state a genuinely deep chain reaches once the T0.3
    // increment has walked the inherited depth up to the ceiling across successive spawns.
    let config = RunnerConfig {
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Chain,
        steps: vec![RunnerStep::SingleStep(single_step("reviewer", "review at the ceiling"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 0,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents,
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
};
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    let outcome = run(&cfg_path, &run_paths).await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    outcome.expect("run() itself never returns Err, even on a depth rejection");

    let status: RunStatus =
        serde_json::from_slice(&tokio::fs::read(&run_paths.status).await.expect("status.json"))
            .expect("parse status.json");
    let result_file: ResultFile =
        serde_json::from_slice(&tokio::fs::read(&run_paths.result).await.expect("ResultFile"))
            .expect("parse ResultFile");

    assert_eq!(
        status.state,
        RunState::Failed,
        "a depth-exhausted chain must reach terminal Failed, spawning nothing: {status:?}"
    );
    assert!(!result_file.success);
    assert!(
        result_file
            .results
            .iter()
            .all(|r| r.final_output.as_deref().unwrap_or_default() != NEVER_SPAWNED_MARKER),
        "the child fixture's marker must never appear — proving NO further child was spawned: \
         {result_file:?}"
    );
    assert!(
        !dir.path().join(".cyrup-subagent-scratch").exists(),
        "no spawn-scratch dir may exist: the guard must reject before any step reaches run_sync's \
         spawn setup"
    );
}

// =============================================================================================
// Tier-2 (b): a chain step with `output = report.md` writes the file AND its delivered output
// carries the saved-output reference message (pi `finalizeSingleOutput`, `single-output.ts`).
// Driven through the SAME foreground chain executor (`SubagentExecutor::run_chain_foreground` ->
// `ExecSingleStepExecutor::run_single` -> `exec::run_sync`) real dispatch a `/chain` step uses,
// against the REAL scripted fixture child (never a mock).
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_with_output_writes_the_file_and_returns_the_saved_output_reference() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // The fixture child emits one assistant message; it does NOT write the output file itself, so
    // the orchestrator (run_sync) persists the captured output to `report.md` and emits the
    // saved-output reference.
    const REPORT_BODY: &str = "the analyzed report body";
    let script = serde_json::json!({
        "steps": [ {"kind": "emit", "line": message_end_line(REPORT_BODY)} ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    // A report/summarize persona + task (NOT implementation-mutation shaped, so the acceptance
    // heuristic is NotRequired and the completion guard — also disabled here — never fires): the run
    // stays exit 0, which is what gates the saved-output reference.
    let reporter = ResolvedAgentPersona {
        name: "reporter".to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
    };
    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert("reporter".to_string(), reporter);

    // The step carries an `output` FILE path (relative — resolved against the run cwd).
    let step = SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: "reporter".to_string(),
        task: "Summarize the analysis.".to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: Some("report.md".to_string()),
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    };

    let executor = SubagentExecutor::new();
    let outcome = executor
        .run_chain_foreground(
            dir.path(),
            vec![RunnerStep::SingleStep(step)],
            resolved_agents,
            String::new(),
            None,
            CancelToken::new(),
            None,
        )
        .await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    let (results, _groups) = outcome.expect("the foreground chain walk completes");
    assert_eq!(results.len(), 1, "one step, one result");
    assert!(results[0].success, "the step must succeed: {:?}", results[0].error);

    // (1) The file was actually written on disk with the child's output.
    let expected_path = dir.path().join("report.md");
    let written = std::fs::read_to_string(&expected_path).expect("report.md must be written on disk");
    assert_eq!(
        written.trim(),
        REPORT_BODY,
        "the output file must contain the persisted step output"
    );

    // (2) The delivered output carries the saved-output reference message, naming the resolved
    // absolute path — and (inline mode) keeps the body before it.
    let delivered = results[0]
        .final_output
        .as_deref()
        .expect("the step must deliver final output");
    assert!(
        delivered.contains("Output saved to:"),
        "the delivered output must carry the saved-output reference: {delivered}"
    );
    assert!(
        delivered.contains(&expected_path.display().to_string()),
        "the saved-output reference must name the resolved output path {}: {delivered}",
        expected_path.display()
    );
    assert!(
        delivered.contains(REPORT_BODY),
        "inline mode keeps the body before the reference message: {delivered}"
    );
}

// =============================================================================================
// Regression: the chain-wide `timeoutMs` (pi `chain-execution.ts:606`: `deadlineAt =
// params.deadlineAt ?? Date.now() + timeoutMs`, threaded into EVERY step's `runSync` call) must
// actually reach the real spawned child's `RunOptions::deadline_at`/`timeout_ms` via
// `SubagentExecutor::run_chain_foreground` -> `ChainRunContext` -> `ExecSingleStepExecutor::
// run_single`. Pre-fix, `run_chain_foreground` hardcoded `ChainRunContext::deadline_at: None`
// (and `route_chain_mode` never even resolved a `timeout_ms` from the tool's `timeoutMs`/
// `maxRuntimeMs` params in the first place), so a chain step's real child ran to completion no
// matter how large its task — chain timeouts did nothing. This test's fixture child sleeps a real
// 30 real seconds; if the deadline never reaches it, the outer 5s `tokio::time::timeout` below
// fires first and this test fails, exactly reproducing the pre-fix hang.
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_wide_timeout_ms_reaches_the_real_child_and_terminates_it() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "sleep_ms", "ms": 30_000}
        ],
        "ignore_sigint": true,
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-chain-timeout.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let reporter = ResolvedAgentPersona {
        name: "reporter".to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
    };
    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert("reporter".to_string(), reporter);

    let step = SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: "reporter".to_string(),
        task: "Summarize the analysis.".to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    };

    let executor = SubagentExecutor::new();
    // The chain-wide `timeout_ms = Some(300)` here is the SAME value `route_chain_mode` resolves
    // from the tool's `timeoutMs`/`maxRuntimeMs` params and threads through
    // `run_or_background_graph` -> `run_chain_foreground` post-fix.
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        executor.run_chain_foreground(
            dir.path(),
            vec![RunnerStep::SingleStep(step)],
            resolved_agents,
            String::new(),
            None,
            CancelToken::new(),
            Some(300),
        ),
    )
    .await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    let (results, _groups) = outcome
        .expect(
            "the chain-wide timeout_ms must reach the real child's RunOptions::deadline_at and \
             terminate it well within 5s of wall-clock time — a hang here reproduces the pre-fix \
             bug (ChainRunContext::deadline_at always None)",
        )
        .expect("run_chain_foreground itself returns Ok even for a step that times out");

    assert_eq!(results.len(), 1, "one step, one result");
    assert!(
        !results[0].success,
        "a timed-out step must be reported as a chain-step failure: {:?}",
        results[0]
    );
}

// =============================================================================================
// Regression: `chain.dynamicFanout.maxItems` (pi `config.chain.dynamicFanout.maxItems`) must reach
// the detached background runner's own `ChainRunContext::dynamic_fanout_max_items` via the
// one-shot `RunnerConfig` handoff file — `SubagentExecutor::spawn_background_steps` resolves the
// live config ONCE at plan time and bakes it into `RunnerConfig`. Pre-fix, this field was always
// hardcoded to `None` regardless of the live config, so a background dynamic-fanout step relying
// on the config-level cap (rather than its own `expand.maxItems`) would always fail
// materialization. Proven by reading back the REAL `runner-config.json` this call writes to disk
// (the fixture binary substituted for the detached hop-2 runner just exits immediately, per its
// default empty script, never itself touching the file) rather than by running a dynamic step to
// completion end to end (the scripted fixture has no structured-output capability).
// =============================================================================================

#[tokio::test]
async fn spawn_background_steps_bakes_the_configured_dynamic_fanout_max_items_into_runner_config() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // No script file set: the fixture's own env-fallback default (no steps, immediate exit 0) is
    // enough — this test only needs the detached "runner" process to start and exit quickly
    // WITHOUT ever reading `runner-config.json` itself, so the file survives for read-back.
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
    }

    let cfg = SubagentExtensionConfig {
        chain: Some(ExtensionChainConfig {
            dynamic_fanout: Some(DynamicFanoutConfig { max_items: Some(7) }),
        }),
        ..SubagentExtensionConfig::default()
    };
    let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());
    let executor = ext.executor();

    let step = SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: "worker".to_string(),
        task: "do something".to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    };

    let run_id = executor
        .spawn_background_steps(
            dir.path(),
            BackgroundStepsSpec {
                // SUBA-N03: this fixture drives the generic step-graph entry point, which
                // carries none of the SINGLE-mode overrides.
                run_id: cyrup_ext_subagents::background::RunId::new(),
                timeout_ms: None,
                share: None,
                artifacts_dir: None,
                artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
                steps: vec![RunnerStep::SingleStep(step)],
                mode: RunMode::Chain,
                session_file: None,
                resolved_agents: BTreeMap::new(),
                original_task: String::new(),
                chain_dir: None,
                control: None,
                include_progress: None,
            },
        )
        .await
        .expect("spawn_background_steps confirms the detached hop-1 spawn");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
    }

    // Reconstruct the SAME run-dir path `spawn_background_steps` wrote `runner-config.json` under
    // (C7 shared roots — no inherited nested-route env is set in this test process, so this is the
    // plain per-cwd derivation `resolve_background_storage_roots` itself falls back to).
    let roots = run_artifact_roots(dir.path());
    let run_paths = RunPaths::for_run(&roots.async_root, &roots.results_dir, &run_id);
    let cfg_path = run_paths.run_dir.join("runner-config.json");

    // Give the substituted "detached runner" (the fixture, which never touches this file) a brief
    // moment to finish starting; the file itself is written synchronously by `spawn_background_steps`
    // BEFORE the detached spawn even happens, so this is purely to let the fixture's own process
    // exit cleanly before the test process tears down its tempdir.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let raw = tokio::fs::read(&cfg_path)
        .await
        .expect("runner-config.json must still exist — the fixture substitute never reads it");
    let written: RunnerConfig =
        serde_json::from_slice(&raw).expect("parse the real written RunnerConfig");

    assert_eq!(
        written.dynamic_fanout_max_items,
        Some(7),
        "the live config's chain.dynamicFanout.maxItems (7) must be baked into RunnerConfig \
         verbatim — pre-fix this was always None regardless of the live config"
    );
}

// =============================================================================================
// SUBA-N04 regression: a chain step's declared `acceptance` contract is HONOURED — its `verify[]`
// commands really execute, and a FAILING one makes the step fail instead of silently reporting
// success.
//
// Pre-fix, `ExecSingleStepExecutor::run_single` built `RunOptions { acceptance: None, .. }`
// unconditionally, so the field was parsed off the chain/tool surface, carried all the way into the
// runner, and then discarded with no warning. Every chain/parallel/background step that declared an
// acceptance contract ran completely UNVERIFIED and reported success on the exact same code path an
// accepted run reports it — silent, unlike a refusal. Upstream passes the step's own acceptance into
// the very same `runSync` call the single path uses (`pi-subagents/src/runs/foreground/
// chain-execution.ts:400` for a parallel task, `:1335` for a sequential step, @v0.34.0).
//
// Both directions are asserted against the REAL scripted fixture child through the REAL foreground
// chain walker (`SubagentExecutor::run_chain_foreground` -> `ExecSingleStepExecutor::run_single` ->
// `exec::run_sync` -> `exec::acceptance::evaluate_acceptance` -> a REAL `verify[]` subprocess), so a
// gate that always failed would be caught by the passing-command case just as a dropped contract is
// caught by the failing one.
// =============================================================================================

/// A persona whose completion guard is OFF, so the only thing that can reject the acceptance gate in
/// these two tests is the `verify[]` command's own real exit code.
fn acceptance_persona(name: &str) -> ResolvedAgentPersona {
    ResolvedAgentPersona {
        name: name.to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
    }
}

/// Run ONE chain step carrying `acceptance` through the real foreground walker against the real
/// fixture child, and return its [`StepResult`]-shaped outcome fields (`success`, `error`).
///
/// `verify_command` is a real shell command run by `exec::acceptance::run_verify_commands` in the
/// step's own cwd.
async fn run_chain_step_with_acceptance(
    dir: &Path,
    acceptance: serde_json::Value,
) -> (bool, Option<String>) {
    let script = serde_json::json!({
        "steps": [ {"kind": "emit", "line": message_end_line("implemented the fix")} ],
        "exit_code": 0
    });
    let script_path = write_script(dir, "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert("builder".to_string(), acceptance_persona("builder"));

    let mut step = single_step("builder", "Fix the failing parser.");
    step.acceptance = Some(acceptance);

    let outcome = SubagentExecutor::new()
        .run_chain_foreground(
            dir,
            vec![RunnerStep::SingleStep(step)],
            resolved_agents,
            String::new(),
            None,
            CancelToken::new(),
            None,
        )
        .await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    let (results, _groups) = outcome.expect("the foreground chain walk completes");
    assert_eq!(results.len(), 1, "one step, one result");
    (results[0].success, results[0].error.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chain_step_acceptance_contract_with_a_failing_verify_command_fails_the_step() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // `level: "verified"` REQUIRES a real, executed, exit-0 `verify[]` command (DI-SA-5) — this one
    // exits 1. The child itself exits 0 and claims success in prose, which is precisely the claim
    // the acceptance gate exists to refuse.
    let (success, error) = run_chain_step_with_acceptance(
        dir.path(),
        serde_json::json!({
            "level": "verified",
            "verify": [{ "id": "unit-tests", "command": "exit 1" }]
        }),
    )
    .await;

    assert!(
        !success,
        "a declared `verified` contract whose verify[] command FAILED must not report success — \
         pre-fix the contract was dropped to None and this step reported success"
    );
    let error = error.expect("a rejected acceptance gate must carry the rejection reason");
    assert!(
        error.contains("acceptance rejected"),
        "the failure must name the acceptance gate (R-SA-033's post-hoc correction), not some \
         unrelated error: {error}"
    );
    assert!(
        error.contains("exit 1"),
        "the rejection must name the verify[] command that actually failed: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chain_step_acceptance_contract_with_a_passing_verify_command_still_succeeds() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // The positive control for the test above: the SAME contract shape, with a command that really
    // exits 0. If the gate were simply always-rejecting (or if `verified` could never be reached),
    // this would fail — which is what makes the failing case above meaningful.
    let (success, error) = run_chain_step_with_acceptance(
        dir.path(),
        serde_json::json!({
            "level": "verified",
            "verify": [{ "id": "unit-tests", "command": "exit 0" }]
        }),
    )
    .await;

    assert!(
        success,
        "a declared `verified` contract whose verify[] command PASSED must still succeed: {error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chain_step_with_an_invalid_acceptance_policy_fails_the_step_rather_than_running_ungated() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    // A policy that reaches the runner already malformed (the tool boundary refuses these up front,
    // pi `subagent-executor.ts:1757`; a hand-edited `runner-config.json` does not go through it).
    // Fail-closed: the step FAILS with pi's own `validateAcceptanceInput` message rather than
    // silently degrading to "no contract", which would be the very defect SUBA-N04 names.
    let (success, error) =
        run_chain_step_with_acceptance(dir.path(), serde_json::json!("nonsense")).await;

    assert!(!success, "an invalid acceptance policy must not run ungated and report success");
    let error = error.expect("an invalid acceptance policy must carry a reason");
    assert!(
        error.contains("acceptance has invalid level 'nonsense'."),
        "pi's verbatim validateAcceptanceInput message: {error}"
    );
}
