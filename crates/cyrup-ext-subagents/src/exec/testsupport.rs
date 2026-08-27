//! Fixtures shared by more than one `exec` submodule's tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use crate::discovery::types::{OutputMode, SystemPromptMode};
use crate::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use crate::exec::agent_config::{AgentConfig, RunOptions};
use crate::exec::fallback::ModelOverride;
use crate::exec::output::OutputCap;
use crate::exec::spawn_plan::{APPEND_SYSTEM_PROMPT_FLAG, AttemptSpawnPlan, SYSTEM_PROMPT_FLAG};
use crate::fork_context::ForkContext;
use crate::spawn::depth::DepthEnvelope;
use cyrup_core::{CancelToken, ModelId};

pub(crate) fn sample_agent_config(model: &str, fallback: &[&str]) -> AgentConfig {
    AgentConfig {
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        fallback_models: fallback.iter().map(|m| ModelId::from(*m)).collect(),
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

pub(crate) fn base_opts(cwd: &std::path::Path, available: &[&str]) -> RunOptions {
    RunOptions {
        // SUBA-021: no usage budget on this path (see the field doc).
        usage_budget: None,
        turn_budget: None,
        permission_rules: None,
        enforce_hard_turn_limit: false,
        model_scope: None,
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        reads: None,
        steer_ack_dir: None,
        steer_capability_path: None,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: available.iter().map(|m| ModelId::from(*m)).collect(),
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
    }
}

// ---- SUBA-001: persona system-prompt delivery (pi `runs/shared/pi-args.ts:159-165` @ v0.34.0) ----

/// The delivered persona: locate the `--system-prompt`/`--append-system-prompt` FLAG element,
/// take the element after it as the spill path, and return the file's contents (SUBA-030 — pi
/// `runs/shared/pi-args.ts:580-585` pushes flag and path as two argv elements).
///
/// Deliberately asserts the two-element shape on the way through: a regression back to the old
/// `--flag=<body>` single-element form makes `starts_with("--system-prompt")` still match while
/// the path lookup fails, so it fails loudly rather than reading as "no persona".
pub(crate) fn delivered_system_prompt(argv: &[String]) -> Option<String> {
    let idx = argv
        .iter()
        .position(|a| a == "--system-prompt" || a == "--append-system-prompt")?;
    assert!(
        !argv.iter().any(|a| a.starts_with("--system-prompt=")
            || a.starts_with("--append-system-prompt=")),
        "SUBA-030: the persona must NEVER ride on argv as `--flag=<body>`; argv was {argv:?}"
    );
    let path = argv
        .get(idx + 1)
        .unwrap_or_else(|| panic!("the flag must be followed by a spill path; argv {argv:?}"));
    Some(std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("the spill file named on argv must be readable ({path}): {e}")
    }))
}

/// Read back the file `--system-prompt`/`--append-system-prompt` points at in a built plan.
pub(crate) fn read_system_prompt_arg(plan: &AttemptSpawnPlan) -> String {
    let argv = plan.spec.build_argv();
    let idx = argv
        .iter()
        .position(|a| a == SYSTEM_PROMPT_FLAG || a == APPEND_SYSTEM_PROMPT_FLAG)
        .expect("a non-empty persona must push a system-prompt flag");
    std::fs::read_to_string(&argv[idx + 1]).expect("the spilled prompt file must exist")
}
