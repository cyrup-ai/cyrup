//! Fixtures shared by more than one `exec` submodule's tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::discovery::types::{OutputMode, SystemPromptMode};
use crate::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use crate::exec::agent_config::{AgentConfig, RunOptions};
use crate::exec::fallback::ModelOverride;
use crate::exec::mcp_direct_tools::{McpDirs, ServerEntry, compute_mcp_server_hash};
use crate::exec::output::OutputCap;
use crate::exec::spawn_plan::{APPEND_SYSTEM_PROMPT_FLAG, AttemptSpawnPlan, SYSTEM_PROMPT_FLAG};
use crate::fork_context::ForkContext;
use crate::spawn::depth::DepthEnvelope;
use cyrup_core::{CancelToken, ModelId};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn sample_agent_config(model: &str, fallback: &[&str]) -> AgentConfig {
    AgentConfig {
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        model_provider: None,
        fallback_models: fallback.iter().map(|m| ModelId::from(*m)).collect(),
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
        runner: None,
        acceptance_role: None,
        default_acceptance: None,
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

pub(crate) fn base_opts(cwd: &std::path::Path, available: &[&str]) -> RunOptions {
    RunOptions {
        // SCOPE_3j: no cached-exclusion registry for a fixture run — nothing is filtered and
        // nothing is recorded, which is this field's documented `None` behaviour.
        model_exclusions: None,
        structured_output_dir: None,
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        // No host observation on this fixture: `None` is UNKNOWN, which skips the intersection and
        // is the pre-mechanism behaviour every existing test was written against.
        host_available_builtins: None,
        // SUBA-021: no usage budget on this path (see the field doc).
        usage_budget: None,
        turn_budget: None,
        permission_rules: None,
        enforce_hard_turn_limit: false,
        model_scope: None,
        thinking_ceiling: None,
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
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        transcript: None,
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
        !argv
            .iter()
            .any(|a| a.starts_with("--system-prompt=") || a.starts_with("--append-system-prompt=")),
        "SUBA-030: the persona must NEVER ride on argv as `--flag=<body>`; argv was {argv:?}"
    );
    let path = argv
        .get(idx + 1)
        .unwrap_or_else(|| panic!("the flag must be followed by a spill path; argv {argv:?}"));
    Some(
        std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("the spill file named on argv must be readable ({path}): {e}")
        }),
    )
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

// ================================================================================================
// Hermetic direct-MCP layouts
// ================================================================================================

/// A [`cyrup_ext::host::HostServices`] double answering only the ROWS seam — the
/// `cyrup-session-svc/src/host_services.rs:2075` shape. Every other capability keeps the trait's
/// default, exactly as `prompt_runtime.rs:3010`'s `RegistryHost` does.
///
/// Lives here rather than in `tool_surface.rs`'s own `mod tests` because two modules now need it:
/// that module's [`crate::exec::tool_surface::host_builtin_tool_names`] tests and
/// `extension::executor`'s test that the observation seam reads the live host.
pub(crate) struct RowsHost(pub(crate) Option<Vec<serde_json::Value>>);

impl cyrup_ext::host::HostServices for RowsHost {
    fn all_tools(&self) -> Option<Vec<serde_json::Value>> {
        self.0.clone()
    }
}

// ================================================================================================

/// A throwaway `$HOME` + project pair with an [`McpDirs`] pointing into it, so a test can drive the
/// file-backed direct-MCP resolver without touching the real `~/.cyrup/agent`.
///
/// Lives here rather than in `mcp_direct_tools.rs`'s own `mod tests` because two `exec` submodules
/// now need it: that module's resolver tests and `tool_surface.rs`'s fold tests.
pub(crate) struct McpFixture {
    pub(crate) _root: tempfile::TempDir,
    pub(crate) agent_dir: PathBuf,
    pub(crate) project_dir: PathBuf,
    pub(crate) dirs: McpDirs,
}

pub(crate) fn make_fixture() -> McpFixture {
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home");
    let agent_dir = home.join(".cyrup").join("agent");
    let project_dir = root.path().join("project");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let dirs = McpDirs {
        agent_dir: agent_dir.clone(),
        generic_global_config_path: home.join(".config").join("mcp").join("mcp.json"),
        home,
    };
    McpFixture {
        _root: root,
        agent_dir,
        project_dir,
        dirs,
    }
}

pub(crate) fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, serde_json::to_string_pretty(value).expect("json")).expect("write");
}

/// Mirror of pi's `writeMcpFixture`: write an `mcp.json` (server config) and a matching
/// `mcp-cache.json` (tools/resources + the identity hash) under the given config path + agent
/// dir.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_mcp_fixture(
    fixture: &McpFixture,
    server_name: &str,
    definition_extra: Value,
    settings: Option<Value>,
    tools: Vec<&str>,
    resources: Vec<(&str, &str)>,
    config_path: Option<PathBuf>,
    cached_at: Option<i64>,
) {
    let mut definition = serde_json::json!({ "command": "npx", "args": ["chrome-devtools-mcp"] });
    if let Value::Object(extra) = definition_extra
        && let Value::Object(base) = &mut definition
    {
        for (k, v) in extra {
            base.insert(k, v);
        }
    }
    let mut config = serde_json::Map::new();
    if let Some(settings) = settings {
        config.insert("settings".to_string(), settings);
    }
    config.insert(
        "mcpServers".to_string(),
        serde_json::json!({ server_name: definition.clone() }),
    );
    let config_path = config_path.unwrap_or_else(|| fixture.agent_dir.join("mcp.json"));
    write_json(&config_path, &Value::Object(config));

    let entry: ServerEntry = serde_json::from_value(definition.clone()).expect("server entry");
    let tools_json: Vec<Value> = tools
        .iter()
        .map(|name| serde_json::json!({ "name": name }))
        .collect();
    let resources_json: Vec<Value> = resources
        .iter()
        .map(|(name, uri)| serde_json::json!({ "name": name, "uri": uri }))
        .collect();
    write_json(
        &fixture.agent_dir.join("mcp-cache.json"),
        &serde_json::json!({
            "version": 1,
            "servers": {
                server_name: {
                    "configHash": compute_mcp_server_hash(&entry).expect("fixture definitions are hashable"),
                    "cachedAt": cached_at.unwrap_or_else(crate::time::now_epoch_millis),
                    "tools": tools_json,
                    "resources": resources_json,
                }
            }
        }),
    );
}
