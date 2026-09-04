//! SUBA-084 — runtime agent registration end to end (pi `test/unit/runtime-agent-registration.test.ts`
//! @v0.64.0, ported case for case where cyrup has the surface): an agent registered IN-PROCESS
//! through [`SubagentExecutor::register_agent`] / [`SubagentsExtension::register_agent`] reaches
//! the real discovery pipeline (`discover_agents` / `discover_agents_all` / the management `list`)
//! with `source == Runtime` and `file_path == runtime:<name>`, collides fail-CLOSED against builtin,
//! runtime and configured identities — including configured agents HIDDEN by scope narrowing —
//! disposes idempotently, and is cleared by `SessionShutdown`.
//!
//! No mocking (this crate's standing convention): every on-disk scope is a real directory under a
//! real `tempfile::tempdir()`, and the executor path builds its discovery config through the same
//! `SubagentExecutor::discovery_config` every tool call uses.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_ext::HostEvent;
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};

use crate::discovery::management::{ManagementRequest, handle_management_action};
use crate::discovery::runtime_registry::{
    MAX_RUNTIME_AGENTS_PER_OWNER, RuntimeAgentDefinition, RuntimeAgentRegistry,
};
use crate::discovery::types::{AgentReadScope, AgentSource, SystemPromptMode};
use crate::discovery::{
    AgentDiscoveryConfig, AgentNameResolution, discover_agents, discover_agents_all,
    resolve_agent_name,
};
use crate::extension::{SubagentExecutor, SubagentsExtension};

// -------------------------------------------------------------------------------------------
// Fixture: a real user + project tree, no settings, no builtins
// -------------------------------------------------------------------------------------------

fn user_agents_dir(base: &Path) -> PathBuf {
    base.join("home").join(".cyrup").join("agents")
}

fn project_agents_dir(base: &Path) -> PathBuf {
    base.join("project").join(".cyrup").join("agents")
}

/// `writeProjectAgent` / `writeUserAgent` (`runtime-agent-registration.test.ts:47-57`).
fn write_agent(dir: &Path, name: &str, aliases: &[&str]) {
    std::fs::create_dir_all(dir).expect("mkdir agent dir");
    let alias_line = if aliases.is_empty() {
        String::new()
    } else {
        format!("aliases: {}\n", aliases.join(", "))
    };
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: {name} agent\n{alias_line}---\n\n{name} prompt.\n"
        ),
    )
    .expect("write agent file");
}

/// The discovery config an embedder-less test drives directly: on-disk tiers from `base`, plus
/// whatever `registry` currently holds — the same copy `SubagentExecutor::discovery_config` makes.
fn cfg_with_registry(base: &Path, registry: &RuntimeAgentRegistry) -> AgentDiscoveryConfig {
    AgentDiscoveryConfig {
        user_agent_dirs: vec![user_agents_dir(base)],
        project_agent_dirs: vec![project_agents_dir(base)],
        project_root: Some(base.join("project")),
        global_dir: base.join("global"),
        runtime_agents: registry.list(),
        ..AgentDiscoveryConfig::default()
    }
}

fn registry() -> Arc<RuntimeAgentRegistry> {
    Arc::new(RuntimeAgentRegistry::new())
}

fn list_request<'a>(scope: Option<&'a str>) -> ManagementRequest<'a> {
    ManagementRequest {
        agent: None,
        chain_name: None,
        agent_scope: scope,
        config: None,
        current_session_model: None,
        proactive_skills: None,
    }
}

// -------------------------------------------------------------------------------------------
// 1. Reaches discovery without writing any config (`:81-104`)
// -------------------------------------------------------------------------------------------

#[test]
fn runtime_agent_reaches_discovery_without_writing_config() {
    let base = tempfile::tempdir().expect("tempdir");
    let registry = registry();
    let mut def = RuntimeAgentDefinition::new("Runtime scout", "Scout at runtime.");
    def.aliases = Some(vec!["rscout".to_string()]);
    def.model = Some("openai/gpt-5-mini".to_string());
    let registration = registry.register("runtime-scout", &def).expect("registers");

    let result =
        discover_agents(&cfg_with_registry(base.path(), &registry), None).expect("discovers");
    let agent = result
        .agents
        .iter()
        .find(|a| a.name == "runtime-scout")
        .expect("runtime agent is discovered");
    assert_eq!(agent.source, AgentSource::Runtime);
    assert_eq!(agent.file_path, PathBuf::from("runtime:runtime-scout"));
    assert_eq!(agent.aliases, vec!["rscout".to_string()]);
    assert_eq!(agent.system_prompt_body, "Scout at runtime.");
    assert_eq!(
        agent.system_prompt_mode,
        SystemPromptMode::Replace,
        "name-sensitive default"
    );
    assert!(!agent.inherit_project_context);
    assert_eq!(
        agent.model.as_ref().map(|m| m.as_str()),
        Some("openai/gpt-5-mini")
    );
    assert!(
        !user_agents_dir(base.path()).exists(),
        "no user file was written"
    );
    assert!(
        !project_agents_dir(base.path()).exists(),
        "no project file was written"
    );
    assert!(
        !base
            .path()
            .join("home")
            .join(".cyrup")
            .join("agents")
            .join("settings.json")
            .exists()
    );

    // The management view sees it too, and resolution by alias works.
    let all =
        discover_agents_all(&cfg_with_registry(base.path(), &registry)).expect("discovers all");
    assert!(all.agents.iter().any(|a| a.name == "runtime-scout"));
    match resolve_agent_name("rscout", &all.agents) {
        AgentNameResolution::Found(found) => assert_eq!(found.name, "runtime-scout"),
        other => panic!("alias should resolve: {other:?}"),
    }

    registration.dispose();
    let after =
        discover_agents(&cfg_with_registry(base.path(), &registry), None).expect("discovers");
    assert!(!after.agents.iter().any(|a| a.name == "runtime-scout"));
}

// -------------------------------------------------------------------------------------------
// 2. Listed by the management `list` action under every scope (`:220-230`)
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn runtime_agent_is_listed_by_management_list() {
    let base = tempfile::tempdir().expect("tempdir");
    let registry = registry();
    let mut def = RuntimeAgentDefinition::new("Runtime helper", "Help at runtime.");
    def.aliases = Some(vec!["helper".to_string()]);
    registry
        .register("runtime-helper", &def)
        .expect("registers");
    let cfg = cfg_with_registry(base.path(), &registry);

    for scope in [None, Some("user"), Some("project")] {
        let out = handle_management_action(&cfg, "list", &list_request(scope))
            .await
            .expect("list ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("- runtime-helper (runtime, aliases: helper): Runtime helper"),
            "scope {scope:?}: {}",
            out.text
        );
    }
}

// -------------------------------------------------------------------------------------------
// 3./4. Fails closed for builtin and duplicate runtime identities (`:274-303`)
// -------------------------------------------------------------------------------------------

#[test]
fn fails_closed_for_builtin_identity() {
    let registry = registry();
    let err = registry
        .register("scout", &RuntimeAgentDefinition::new("Bad", "Bad."))
        .expect_err("builtin name");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'scout' collides with builtin agent 'scout'."
    );
    let mut aliased = RuntimeAgentDefinition::new("Bad", "Bad.");
    aliased.aliases = Some(vec!["worker".to_string()]);
    let err = registry
        .register("runtime-x", &aliased)
        .expect_err("builtin alias");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'runtime-x' collides with builtin agent 'worker'."
    );
    assert!(registry.is_empty());
}

#[test]
fn fails_closed_for_duplicate_runtime_identity() {
    let registry = registry();
    let mut a = RuntimeAgentDefinition::new("A", "A.");
    a.aliases = Some(vec!["shared".to_string()]);
    registry.register("runtime-a", &a).expect("registers");
    let mut b = RuntimeAgentDefinition::new("B", "B.");
    b.aliases = Some(vec!["shared".to_string()]);
    let err = registry
        .register("runtime-b", &b)
        .expect_err("shared alias");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'runtime-b' collides with runtime agent 'runtime-a' on name or alias 'shared'."
    );
    let err = registry
        .register("runtime-a", &RuntimeAgentDefinition::new("Dup", "Dup."))
        .expect_err("same name");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'runtime-a' collides with runtime agent 'runtime-a' on name or alias 'runtime-a'."
    );
    assert_eq!(registry.len(), 1);
}

// -------------------------------------------------------------------------------------------
// 5. Malformed nested fields are refused at registration with upstream's text (`:305-319`)
// -------------------------------------------------------------------------------------------

#[test]
fn rejects_malformed_nested_definition_fields() {
    let registry = registry();
    let base = serde_json::json!({"description": "Bad", "systemPrompt": "Bad."});
    let with = |key: &str, value: serde_json::Value| {
        let mut object = base.clone();
        object
            .as_object_mut()
            .unwrap()
            .insert(key.to_string(), value);
        object
    };
    let runner = registry
        .register_value(
            "runtime-runner",
            &with("runner", serde_json::json!({"type": "bogus"})),
        )
        .expect_err("bogus runner");
    assert_eq!(
        runner.to_string(),
        "Runtime agent definition runner.type must be 'pi', 'external-cli', or 'external-job'."
    );
    let timeout = registry
        .register_value(
            "runtime-timeout",
            &with("defaultTimeoutMs", serde_json::json!(0)),
        )
        .expect_err("zero timeout");
    assert_eq!(
        timeout.to_string(),
        "Runtime agent definition defaultTimeoutMs must be a positive integer when provided."
    );
    let unknown = registry
        .register_value("runtime-unknown", &with("foo", serde_json::json!(1)))
        .expect_err("unknown field");
    assert_eq!(
        unknown.to_string(),
        "Runtime agent definition has unknown fields: foo."
    );
    assert!(
        registry.is_empty(),
        "nothing was registered by a refused call"
    );
}

// -------------------------------------------------------------------------------------------
// 6. cwd discovery introduces a configured collision (`:321-329`)
// -------------------------------------------------------------------------------------------

#[test]
fn fails_closed_when_cwd_discovery_introduces_configured_collision() {
    let base = tempfile::tempdir().expect("tempdir");
    let registry = registry();
    let mut def = RuntimeAgentDefinition::new("Runtime helper", "Help.");
    def.aliases = Some(vec!["helper".to_string()]);
    registry
        .register("runtime-helper", &def)
        .expect("registers");
    // Registration succeeds — the project file appears AFTERWARDS, and it is the next discovery
    // that fails closed.
    write_agent(
        &project_agents_dir(base.path()),
        "project-helper",
        &["helper"],
    );

    let err = discover_agents(&cfg_with_registry(base.path(), &registry), None)
        .expect_err("collision fails discovery closed");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'runtime-helper' collides with configured agent 'project-helper' on name or alias 'helper'."
    );

    // Plain same-name collision, same message shape.
    let registry2 = registry_with("dup");
    write_agent(&project_agents_dir(base.path()), "dup", &[]);
    let err = discover_agents(&cfg_with_registry(base.path(), &registry2), None).expect_err("dup");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'dup' collides with configured agent 'dup' on name or alias 'dup'."
    );
}

fn registry_with(name: &str) -> Arc<RuntimeAgentRegistry> {
    let registry = registry();
    registry
        .register(
            name,
            &RuntimeAgentDefinition::new("Runtime hidden", "Help."),
        )
        .expect("registers");
    registry
}

// -------------------------------------------------------------------------------------------
// 7. Configured agents hidden by scope precedence / explicit scope still block (`:331-356`)
// -------------------------------------------------------------------------------------------

#[test]
fn fails_closed_against_configured_agent_hidden_by_scope() {
    let base = tempfile::tempdir().expect("tempdir");
    // Hidden by explicit scope: the user agent is not in a Project-scoped view at all …
    write_agent(&user_agents_dir(base.path()), "hidden-user", &[]);
    let empty = registry();
    let project_scoped = discover_agents(
        &cfg_with_registry(base.path(), &empty),
        Some(AgentReadScope::Project),
    )
    .expect("discovers");
    assert!(
        !project_scoped
            .agents
            .iter()
            .any(|a| a.name == "hidden-user")
    );
    // … yet a runtime agent of that name still fails the Project-scoped discovery closed.
    let registry = registry_with("hidden-user");
    let err = discover_agents(
        &cfg_with_registry(base.path(), &registry),
        Some(AgentReadScope::Project),
    )
    .expect_err("hidden by scope still collides");
    assert_eq!(
        err.to_string(),
        "Runtime agent 'hidden-user' collides with configured agent 'hidden-user' on name or alias 'hidden-user'."
    );

    // Hidden by precedence: a project agent shadows the user one in the merged view; the
    // runtime agent collides with the configured name regardless of which tier won.
    write_agent(&project_agents_dir(base.path()), "hidden-user", &[]);
    let merged = discover_agents(&cfg_with_registry(base.path(), &empty), None).expect("discovers");
    assert_eq!(
        merged
            .agents
            .iter()
            .find(|a| a.name == "hidden-user")
            .map(|a| a.source),
        Some(AgentSource::Project)
    );
    let err =
        discover_agents(&cfg_with_registry(base.path(), &registry), None).expect_err("collides");
    assert!(
        err.to_string().contains(
            "collides with configured agent 'hidden-user' on name or alias 'hidden-user'"
        ),
        "{err}"
    );
}

// -------------------------------------------------------------------------------------------
// 8. The management `list` fails closed on a scoped configured collision (`:358-366`)
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn management_list_fails_closed_on_scoped_configured_collision() {
    let base = tempfile::tempdir().expect("tempdir");
    write_agent(&user_agents_dir(base.path()), "hidden-user", &[]);
    let registry = registry_with("hidden-user");
    let cfg = cfg_with_registry(base.path(), &registry);
    let err = match handle_management_action(&cfg, "list", &list_request(Some("project"))).await {
        Ok(outcome) => panic!("list should fail closed, got: {}", outcome.text),
        Err(err) => err,
    };
    assert_eq!(
        err.to_string(),
        "Runtime agent 'hidden-user' collides with configured agent 'hidden-user' on name or alias 'hidden-user'."
    );
}

// -------------------------------------------------------------------------------------------
// 9. dispose is idempotent and removes exactly its own record (`:386-397`)
// -------------------------------------------------------------------------------------------

#[test]
fn dispose_is_idempotent_and_removes_agent() {
    let base = tempfile::tempdir().expect("tempdir");
    let registry = registry();
    let first = registry
        .register("runtime-a", &RuntimeAgentDefinition::new("A", "A."))
        .expect("registers");
    let second = registry
        .register("runtime-b", &RuntimeAgentDefinition::new("B", "B."))
        .expect("registers");
    assert_eq!(registry.len(), 2);

    first.dispose();
    assert!(first.is_disposed());
    first.dispose();
    assert_eq!(registry.len(), 1);
    let names: Vec<String> = discover_agents(&cfg_with_registry(base.path(), &registry), None)
        .expect("discovers")
        .agents
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec!["runtime-b".to_string()]);

    // A disposed handle never removes a LATER registration that re-uses the name (record
    // identity, not name equality).
    let replacement = registry
        .register("runtime-a", &RuntimeAgentDefinition::new("A2", "A2."))
        .expect("name is free again");
    first.dispose();
    assert_eq!(registry.len(), 2);
    assert_eq!(replacement.agent_name(), "runtime-a");
    second.dispose();
    replacement.dispose();
    assert!(registry.is_empty());
}

// -------------------------------------------------------------------------------------------
// 10. 200-per-owner cap (`:381`)
// -------------------------------------------------------------------------------------------

#[test]
fn registry_caps_at_200_per_owner() {
    let registry = registry();
    for index in 0..MAX_RUNTIME_AGENTS_PER_OWNER {
        registry
            .register(
                &format!("runtime-{index}"),
                &RuntimeAgentDefinition::new("N", "N."),
            )
            .expect("under the cap");
    }
    let err = registry
        .register("runtime-overflow", &RuntimeAgentDefinition::new("N", "N."))
        .expect_err("201st");
    assert_eq!(
        err.to_string(),
        "Runtime agent registry supports at most 200 agents per Pi runtime."
    );
    assert_eq!(registry.len(), MAX_RUNTIME_AGENTS_PER_OWNER);
}

// -------------------------------------------------------------------------------------------
// 11. The executor seam + SessionShutdown clears (`extension/index.ts:528-546,971`)
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn executor_discovery_sees_registered_agent_and_session_shutdown_clears_it() {
    let base = tempfile::tempdir().expect("tempdir");
    let cwd = base.path().join("project");
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");
    let roots = crate::paths::Roots::sandboxed(&base.path().join("home"));
    let extension = SubagentsExtension::with_config_and_cwd(Default::default(), cwd.clone());
    let registration = extension
        .register_agent(
            "runtime-scout",
            &RuntimeAgentDefinition::new("Runtime scout", "Scout."),
        )
        .expect("registers");
    assert_eq!(registration.agent_name(), "runtime-scout");

    // The REAL discovery config every tool call builds carries the registry snapshot …
    let cfg = extension
        .executor()
        .discovery_config(&cwd, &roots)
        .expect("config");
    assert_eq!(cfg.runtime_agents.len(), 1);
    // … and the executor's own resolution path (the `subagent` tool's seam) finds the agent.
    let resolved = extension
        .executor()
        .resolve_agent(&cwd, "runtime-scout", AgentReadScope::Both, &roots)
        .expect("resolves");
    assert_eq!(resolved.source, AgentSource::Runtime);

    extension
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "test".to_string(),
                target_session_file: None,
            },
            &HostCtx::event(ExtMode::Json, false, cwd.clone()),
        )
        .await;
    assert!(
        extension.executor().runtime_agents().is_empty(),
        "session shutdown clears the registry"
    );
    assert!(
        extension
            .executor()
            .resolve_agent(&cwd, "runtime-scout", AgentReadScope::Both, &roots)
            .is_err()
    );
    // A handle outliving the clear is a harmless no-op.
    registration.dispose();

    // A bare executor exposes the same entry point (pi's `registerAgent` needs only the owner).
    let executor = SubagentExecutor::new();
    executor
        .register_agent("runtime-b", &RuntimeAgentDefinition::new("B", "B."))
        .expect("registers");
    assert_eq!(
        executor.runtime_agents().list()[0].file_path,
        PathBuf::from("runtime:runtime-b")
    );
}

// -------------------------------------------------------------------------------------------
// 12. `sourceRank` runtime = 4 outranks project in same-name resolution (`agents.ts:687`)
// -------------------------------------------------------------------------------------------

#[test]
fn runtime_source_outranks_project_in_name_resolution() {
    let registry = registry();
    registry
        .register("x", &RuntimeAgentDefinition::new("Runtime x", "x."))
        .expect("registers");
    let runtime_x = registry.list().remove(0);
    let project_x = crate::discovery::frontmatter::parse_agent_file(
        "---\nname: x\ndescription: Project x\n---\n\nx.\n",
        AgentSource::Project,
        Path::new("/project/.cyrup/agents/x.md"),
    )
    .expect("parses");
    // Same canonical name at two tiers is NOT ambiguous (`effectiveAgentMatch`); the highest
    // source rank wins, and runtime sits above project.
    for candidates in [
        vec![project_x.clone(), runtime_x.clone()],
        vec![runtime_x.clone(), project_x.clone()],
    ] {
        match resolve_agent_name("x", &candidates) {
            AgentNameResolution::Found(found) => assert_eq!(found.source, AgentSource::Runtime),
            other => panic!("expected the runtime agent: {other:?}"),
        }
    }
}
