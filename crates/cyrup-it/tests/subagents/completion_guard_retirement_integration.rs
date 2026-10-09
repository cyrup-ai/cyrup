//! SUBA-107 — proof that the completion-mutation guard is GONE from the production run path.
//!
//! Upstream reference: `pi-subagents` deleted `src/runs/shared/completion-guard.ts` and
//! `src/runs/shared/task-intent.ts` at v0.70.1 in `7c98a696` ("refactor: remove inferred no-edit
//! completion failures (#2356)"). At v0.71.0 `completionGuard`,
//! `evaluateCompletionMutationGuard`, `classifyTaskMutationIntent`, `taskMayMutate`,
//! `hasMutationToolCall`, `hasMutationToolCapability` and `LLM_INTENT_ARBITER` all have ZERO hits
//! under `src/` (`git grep` at the tag). CHANGELOG 0.70.1 Changed: "Stop guessing whether task
//! wording requires file edits… The `completionGuard` setting and
//! `PI_SUBAGENTS_LLM_INTENT_ARBITER` switch have been removed."
//!
//! What that means observably, and what this file pins: a write-capable agent handed an
//! implementation-shaped task that finishes cleanly WITHOUT calling any edit/write/bash tool is a
//! SUCCESS. cyrup used to fail it (exit 1 + a `completion_guard` needs-attention notice) purely on
//! the wording of the task string.
//!
//! No mocking: the run below spawns the REAL `cyrup-subagent-fixture` binary as a genuine OS
//! subprocess through `RunOptions::spawn_command` and asserts on the REAL observed `SingleResult`
//! and the REAL control events raised through `RunOptions::on_control_event`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::control::{
    ControlEvent, ControlEventSink, ResolvedControlConfig, control_event_reason_wire,
};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

fn write_script(dir: &Path, name: &str, script_json: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script_json.to_string()).expect("write fixture script");
    path
}

/// A WRITE-CAPABLE agent: `tools: None` is "inherit every tool", so
/// `has_mutation_tool_capability` was true for it and the retired guard's read-only exemption
/// never applied. The persona also does NOT opt out — the `completionGuard` key no longer exists.
fn write_capable_agent(model: &str) -> AgentConfig {
    AgentConfig {
        model_is_settings_default: false,
        machine: None,
        acceptance_role: None,
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
        // SUBA-111: this literal predates `allowedAgents`; it declares no delegation bound.
        allowed_agents: None,
        allow_nested_subagents: None,
        output: None,
        inherit_project_context: false,
        inherit_global_context: false,
        mutation_tools: None,
        inherit_skills: true,
        skills: Vec::new(),
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

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        launch_model: None,
        tool_timeout_ms: None,
        model_override_from_parent: false,
        model_response_aliases: None,
        parent_env_overrides: std::collections::BTreeMap::new(),
        machine: None,
        model_exclusions: None,
        fast: false,
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
        // SUBA-155 — a fixture launch has no parent session model, so the reserved
        // `modelScope` allow tokens stay unexpanded (upstream's fail-closed rule).
        parent_model: None,
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
        // SUBA-158 — a fixture launch carries no host trust information, which is pi's
        // `undefined` arm: the child decides for itself, exactly as before the field existed.
        parent_project_trusted: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        steer_ack_dir: None,
        external_log_dir: None,
        steer_capability_path: None,
        // Control raises are ENABLED for this run, so a surviving guard would really have emitted
        // its `completion_guard` needs-attention notice into the sink below rather than being
        // silently swallowed by a disabled monitor. Without this the notice half of the assertion
        // would pass vacuously.
        control_config: Some(ResolvedControlConfig::default()),
        on_control_event: None,
        artifacts_dir: None,
        transcript: None,
        model_scope: None,
    }
}

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 11, "output": 7, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 18,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// A write-capable agent handed an implementation-shaped task that makes NO mutating tool call and
/// whose child exits 0 must SUCCEED.
///
/// Red before SUBA-107: `exec/mod.rs`'s `apply_completion_guard` saw
/// `expects_implementation_mutation("worker", "Implement the fix in the parser") == true` and
/// `has_mutation_tool_call(&[]) == false`, set `exit_code = 1`, pushed
/// `COMPLETION_GUARD_ERROR_MESSAGE` and raised a `completion_guard` control notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_implementation_task_with_no_edits_finishes_clean() {
    let dir = tempfile::tempdir().expect("tempdir");

    // No `tool_execution_*` lines at all: the child answered in prose and never touched a file.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": message_end_line(
                "The parser already handles this case; no change was needed.",
            )},
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_end"}"#.to_string())}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let events: Arc<Mutex<Vec<ControlEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);

    let agent = write_capable_agent("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    });
    opts.on_control_event = Some(ControlEventSink::new(move |event: &ControlEvent| {
        if let Ok(mut guard) = sink_events.lock() {
            guard.push(event.clone());
        }
    }));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Implement the fix in the parser", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    assert_eq!(
        result.exit_code, 0,
        "an implementation-shaped task that needed no edit is a SUCCESS at pi-subagents \
         v0.71.0 — the completion-mutation guard that failed it was deleted upstream in \
         7c98a696: {result:?}"
    );
    let error_text = result.error.clone().unwrap_or_default();
    assert!(
        !error_text.contains("completion-mutation guard"),
        "no completion-mutation-guard error text may survive: {error_text:?}"
    );

    // Both the raise sink AND the result's own recorded events: before SUBA-107 the guard notice
    // landed in `result.control_events` (the run record) even when nothing was attached to the
    // sink, so asserting on the sink alone would have passed vacuously.
    let raised = events.lock().expect("sink mutex").clone();
    for (source, seen) in [("sink", &raised), ("result", &result.control_events)] {
        let guard_notices: Vec<&ControlEvent> = seen
            .iter()
            .filter(|event| {
                event
                    .reason
                    .is_some_and(|reason| control_event_reason_wire(reason) == "completion_guard")
            })
            .collect();
        assert!(
            guard_notices.is_empty(),
            "no `completion_guard` control notice may reach the {source} — upstream's reason \
             union at v0.71.0 (`shared/types.ts:384`) has no such member: {guard_notices:?}"
        );
    }
}

/// The retired symbols must not creep back in. A grep guard, because the row's whole point is that
/// this machinery is DELETED, not merely bypassed on one path.
#[test]
fn the_retired_completion_guard_symbols_have_zero_hits_in_the_workspace() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is cyrup-it's parent");

    let mut offenders: Vec<String> = Vec::new();
    let needles = [
        "COMPLETION_GUARD_ERROR_MESSAGE",
        "evaluate_completion_mutation_guard",
        "CompletionMutationGuardResult",
        "classify_task_mutation_intent",
    ];
    visit_rust_files(crates_dir, &mut |path, body| {
        // This file NAMES the retired symbols in prose on purpose.
        if path.ends_with("completion_guard_retirement_integration.rs") {
            return;
        }
        for needle in needles {
            if body.contains(needle) {
                offenders.push(format!("{}: {needle}", path.display()));
            }
        }
    });

    assert!(
        offenders.is_empty(),
        "retired completion-guard symbols are still referenced: {offenders:#?}"
    );
}

fn visit_rust_files(dir: &Path, sink: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            visit_rust_files(&path, sink);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && let Ok(body) = std::fs::read_to_string(&path)
        {
            sink(&path, &body);
        }
    }
}
