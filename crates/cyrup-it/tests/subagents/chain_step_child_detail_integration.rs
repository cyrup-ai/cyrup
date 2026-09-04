//! The `exec::run_sync` -> `StepResult` half of the collect-record fidelity wiring, proven against
//! a REAL scripted subprocess child rather than a mock.
//!
//! Upstream a `SingleResult` carries the child's own `exitCode`
//! (`pi-subagents:v0.34.0:src/runs/foreground/execution.ts:847` — `result.exitCode = exitCode`) and
//! its `savedOutputPath` (`:963` — `result.savedOutputPath = resolvedOutput.savedPath`), and
//! `collectDynamicResults` copies both onto every dynamic fan-out collect record
//! (`src/runs/shared/dynamic-fanout.ts:278,283` @v0.34.0). This port's chain walker sees a step only through
//! the narrow `StepResult` seam, so both values have to survive
//! `ExecSingleStepExecutor::run_single`'s collapse of a `SingleResult` into a `StepResult` — which
//! before this change they did not: the exit code was re-derived as
//! `i32::from(!result.success)` one layer further out and the saved path had no field on
//! `SingleResult` at all.
//!
//! `tests/dynamic_collect_record_fidelity.rs` covers the other half (the walker's fold into the
//! collect-record array) with a scripted executor; this file covers the real-child half. Together
//! they cover the whole path, and neither leans on the other's layer.
//!
//! Separate compilation unit from `lib.rs`, so NOT bound by that crate's `#![forbid(unsafe_code)]`;
//! Mutates no process environment: the fixture is named per run through
//! `SubagentExtensionConfig::spawn_command`, so this file needs no `unsafe` and no lock.
//!
//! Gated on `test-fixtures` (the `cyrup-subagent-fixture` `[[bin]]`'s own `required-features`
//! gate): without it the fixture is never built and this file compiles to an empty, passing test
//! list.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::SystemPromptMode;
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::extension::SubagentExecutor;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec, StepResult};

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
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

/// A report/summarize persona whose completion guard is explicitly disabled, so the run's outcome
/// is decided by the child's own exit code and nothing else.
fn reporter_persona() -> ResolvedAgentPersona {
    ResolvedAgentPersona {
        name: "reporter".to_string(),
        model: Some(ModelId::from("fixture-model")),
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
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
        runner: None,          // SUBA-074: the native child, as before
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
    }
}

fn step(output_path: Option<&str>) -> SingleStepSpec {
    SingleStepSpec {
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
        output_path: output_path.map(str::to_string),
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    }
}

/// Run one chain step against the real fixture child and return its `StepResult`.
async fn run_one_step(
    dir: &Path,
    script: &serde_json::Value,
    output_path: Option<&str>,
) -> StepResult {
    let script_path = write_script(dir, "script.json", script);
    // The fixture named for THIS executor rather than moved into the process environment every
    // concurrently-running test in this binary shares.
    let config = SubagentExtensionConfig {
        spawn_command: Some(SpawnCommand {
            binary: fixture_binary_path(),
            base_args: vec![
                "--fixture-script".to_string(),
                script_path.display().to_string(),
            ],
        }),
        ..SubagentExtensionConfig::default()
    };

    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert("reporter".to_string(), reporter_persona());

    let outcome = SubagentExecutor::with_config(config)
        .run_chain_foreground(
            dir,
            vec![RunnerStep::SingleStep(step(output_path))],
            resolved_agents,
            String::new(),
            None,
            CancelToken::new(),
            None,
        )
        .await;

    let (results, _groups) = outcome.expect("the foreground chain walk completes");
    assert_eq!(results.len(), 1, "one step, one result");
    results.into_iter().next().expect("the single step result")
}

// =================================================================================================
// The child's real exit code survives the SingleResult -> StepResult collapse.
// =================================================================================================

/// pi keeps `result.exitCode` verbatim (`execution.ts:847`), so a child exiting `3` is reported as
/// `3`. Re-deriving the code from the step's success/failure — which is what a consumer got before
/// this change — flattens every non-zero code to exactly `1`, making a domain-specific exit
/// indistinguishable from an ordinary failure in a `{outputs.<collect.as>}` record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_child_exit_code_survives_onto_the_step_result() {
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [ {"kind": "emit", "line": message_end_line("partial work")} ],
        "exit_code": 3
    });
    let result = run_one_step(dir.path(), &script, None).await;

    assert!(!result.success, "exit 3 is a step failure: {result:?}");
    assert_eq!(
        result.exit_code,
        Some(3),
        "the child's OWN exit code must reach the walker, not a re-derived 1: {result:?}"
    );
    assert!(
        !result.timed_out,
        "a child that exited on its own was not killed by the deadline: {result:?}"
    );
}

/// The saved-output handoff's resolved path (pi `result.savedOutputPath`, `execution.ts:963`)
/// reaches the walker as its own value — the field a dynamic fan-out publishes as each record's
/// `outputPath` so a later chain step can locate the file its siblings wrote. Before this change
/// the path existed only folded into the prose of the delivered `final_output`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_saved_output_path_reaches_the_step_result_as_a_bare_path() {
    let dir = tempfile::tempdir().expect("real tempdir");

    const REPORT_BODY: &str = "the analyzed report body";
    let script = serde_json::json!({
        "steps": [ {"kind": "emit", "line": message_end_line(REPORT_BODY)} ],
        "exit_code": 0
    });
    let result = run_one_step(dir.path(), &script, Some("report.md")).await;

    assert!(result.success, "the step must succeed: {:?}", result.error);
    assert_eq!(
        result.exit_code,
        Some(0),
        "a clean child reports 0: {result:?}"
    );

    let expected = dir.path().join("report.md");
    // The handoff really happened: the file is on disk with the child's output in it.
    let written = std::fs::read_to_string(&expected).expect("report.md must be written on disk");
    assert_eq!(written.trim(), REPORT_BODY);
    assert_eq!(
        result.saved_output_path.as_deref(),
        Some(expected.display().to_string().as_str()),
        "the resolved saved-output path must reach the walker as a bare path: {result:?}"
    );
}
