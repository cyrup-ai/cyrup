//! Integration test for SUBA-005's four newly-ported management actions —
//! `eject` / `disable` / `enable` / `reset` (pi `agents/agent-management.ts:909-1032` at v0.34.0).
//!
//! Every assertion here is on **observable effect**, never on "the verb was accepted":
//!
//! - `eject` — the bundled file's bytes are on disk in the writable scope AND the next discovery
//!   pass resolves the agent to that new file at `AgentSource::User`, i.e. the copy really shadows
//!   the builtin.
//! - `disable` — the agent DISAPPEARS from the delegation-time discovery view
//!   ([`discover_agents`]), which is the view `exec/` resolves a requested persona through. An
//!   agent that a subagent call can still spawn is not disabled, whatever the settings file says.
//! - `enable` — the agent REAPPEARS in that same view, and its unrelated `model` override survives.
//! - `reset` — the custom `.md` file is gone from disk, the settings override entry is gone, and
//!   discovery resolves the agent back to the bundled builtin.
//!
//! No filesystem or settings mocking anywhere (this crate's standing convention): every scope is a
//! real directory tree under a real `tempfile::tempdir()`, and both `settings.json` files are real
//! files that the handlers read-modify-write and that the *next* [`AgentDiscoveryConfig`] re-reads
//! from disk — exactly as `SubagentExecutor::discovery_config` does, once per tool call (R-SA-019).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use cyrup_ext_subagents::discovery::management::{
    ManagementRequest, handle_management_action,
};
use cyrup_ext_subagents::discovery::types::AgentSource;
use cyrup_ext_subagents::discovery::{
    AgentDiscoveryConfig, discover_agents, discover_agents_all, load_layered_override_settings,
};
use cyrup_resources::InstalledPackages;

// -------------------------------------------------------------------------------------------
// Fixture: a real four-scope tree with real, re-read-per-call settings.json files
// -------------------------------------------------------------------------------------------

fn user_agents_dir(base: &Path) -> PathBuf {
    base.join("home").join(".cyrup").join("agents")
}

fn project_agents_dir(base: &Path) -> PathBuf {
    base.join("project").join(".cyrup").join("agents")
}

fn user_settings_path(base: &Path) -> PathBuf {
    user_agents_dir(base).join("settings.json")
}

fn project_settings_path(base: &Path) -> PathBuf {
    project_agents_dir(base).join("settings.json")
}

fn builtin_dir(base: &Path) -> PathBuf {
    base.join("builtin")
}

/// Rebuild the discovery config from what is CURRENTLY on disk — the load-bearing part of this
/// fixture. Every handler under test writes to `settings.json`; a test that reused a config built
/// before the write would be asserting against a stale in-memory snapshot rather than against the
/// effect the write actually had. This mirrors `SubagentExecutor::discovery_config`, which likewise
/// re-reads both settings files on every single tool call.
fn cfg_from_disk(base: &Path) -> AgentDiscoveryConfig {
    let override_settings = load_layered_override_settings(
        &user_settings_path(base),
        Some(&project_settings_path(base)),
    )
    .expect("settings load");
    AgentDiscoveryConfig {
        builtin_agents_dir: Some(builtin_dir(base)),
        installed_packages: InstalledPackages { packages: Vec::new() },
        trusted_project: true,
        global_dir: base.join("global"),
        project_root: Some(base.join("project")),
        user_agent_dirs: vec![user_agents_dir(base)],
        user_chain_dirs: vec![base.join("home").join(".cyrup").join("chains")],
        project_agent_dirs: vec![project_agents_dir(base)],
        project_chain_dirs: vec![base.join("project").join(".cyrup").join("chains")],
        override_settings,
    }
}

/// A builtin persona file carrying a trailing comment line and a frontmatter key this crate's
/// parser ignores — both present specifically so `eject`'s byte-for-byte copy contract is
/// falsifiable (a re-serialization of the parsed `AgentDefinition` would drop them).
const SCOUT_BUILTIN_FILE: &str = "---\nname: scout\ndescription: bundled scout persona\nfutureFrontmatterKey: preserve-me\n---\n\nYou are scout.\n\n<!-- a comment only a byte copy preserves -->\n";

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path();
    std::fs::create_dir_all(builtin_dir(base)).unwrap();
    std::fs::create_dir_all(user_agents_dir(base)).unwrap();
    std::fs::create_dir_all(project_agents_dir(base)).unwrap();
    std::fs::write(builtin_dir(base).join("scout.md"), SCOUT_BUILTIN_FILE).unwrap();
    std::fs::write(
        builtin_dir(base).join("worker.md"),
        "---\nname: worker\ndescription: bundled worker persona\n---\n\nYou are worker.\n",
    )
    .unwrap();
    tmp
}

fn write_agent(dir: &Path, local_name: &str, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(format!("{local_name}.md")),
        format!("---\nname: {local_name}\ndescription: {description}\n---\n\nYou are {local_name}.\n"),
    )
    .unwrap();
}

struct Outcome {
    text: String,
    is_error: bool,
}

/// Run one management action against the CURRENT on-disk state.
fn act(base: &Path, action: &str, agent: Option<&str>, scope: Option<&str>) -> Outcome {
    let cfg = cfg_from_disk(base);
    let req = ManagementRequest {
        agent,
        chain_name: None,
        agent_scope: scope,
        config: None,
        current_session_model: None,
    };
    let outcome = handle_management_action(&cfg, action, &req).expect("management action runs");
    Outcome { text: outcome.text, is_error: outcome.is_error }
}

/// The DELEGATION-time view (`exec/` resolves a requested persona through exactly this call) — the
/// only view in which "disabled" means anything observable.
fn delegatable_names(base: &Path) -> Vec<String> {
    discover_agents(&cfg_from_disk(base), None)
        .expect("discovery")
        .agents
        .iter()
        .map(|a| a.name.clone())
        .collect()
}

fn managed_agent(base: &Path, name: &str) -> Option<cyrup_ext_subagents::discovery::types::AgentDefinition> {
    discover_agents_all(&cfg_from_disk(base))
        .expect("discovery")
        .agents
        .into_iter()
        .find(|a| a.name == name)
}

fn settings_json(path: &Path) -> serde_json::Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).expect("settings parses"),
        Err(_) => serde_json::Value::Null,
    }
}

// -------------------------------------------------------------------------------------------
// eject
// -------------------------------------------------------------------------------------------

#[test]
fn eject_copies_the_bundled_file_verbatim_and_the_copy_shadows_the_builtin() {
    let tmp = fixture();
    let base = tmp.path();
    assert_eq!(
        managed_agent(base, "scout").expect("scout discovered").source,
        AgentSource::Builtin,
        "precondition: scout resolves to the bundled tier before the eject"
    );

    let outcome = act(base, "eject", Some("scout"), None);
    assert!(!outcome.is_error, "{}", outcome.text);

    // 1. The bytes are on disk, unaltered — including the frontmatter key and the comment a
    //    re-serialization round-trip would have dropped.
    let ejected = user_agents_dir(base).join("scout.md");
    assert_eq!(
        std::fs::read_to_string(&ejected).unwrap(),
        SCOUT_BUILTIN_FILE,
        "eject must copy the source file byte-for-byte"
    );

    // 2. The copy actually takes over: the next discovery pass resolves `scout` to the USER file.
    let after = managed_agent(base, "scout").expect("scout still discovered");
    assert_eq!(after.source, AgentSource::User, "the ejected copy must shadow the builtin");
    assert_eq!(after.file_path, ejected);
    assert!(outcome.text.contains("Ejected agent 'scout' from builtin to user scope"), "{}", outcome.text);
}

#[test]
fn eject_into_project_scope_writes_the_project_dir_and_wins_over_a_user_copy() {
    let tmp = fixture();
    let base = tmp.path();
    // A user-scope copy already exists; a project eject must still land, and must win the merge.
    write_agent(&user_agents_dir(base), "scout", "user scout");

    let outcome = act(base, "eject", Some("scout"), Some("project"));
    assert!(!outcome.is_error, "{}", outcome.text);
    let ejected = project_agents_dir(base).join("scout.md");
    assert_eq!(std::fs::read_to_string(&ejected).unwrap(), SCOUT_BUILTIN_FILE);

    let after = managed_agent(base, "scout").expect("scout discovered");
    assert_eq!(after.source, AgentSource::Project);
    assert_eq!(after.file_path, ejected);
}

#[test]
fn eject_refuses_to_clobber_an_existing_custom_agent_in_the_target_scope() {
    let tmp = fixture();
    let base = tmp.path();
    write_agent(&user_agents_dir(base), "scout", "hand-written user scout");
    let before = std::fs::read_to_string(user_agents_dir(base).join("scout.md")).unwrap();

    let outcome = act(base, "eject", Some("scout"), Some("user"));
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(
        outcome.text.contains("is already a custom user agent at"),
        "must give pi's distinct already-custom refusal, not the generic not-found one: {}",
        outcome.text
    );
    assert_eq!(
        std::fs::read_to_string(user_agents_dir(base).join("scout.md")).unwrap(),
        before,
        "a refused eject must leave the existing file untouched"
    );
}

#[test]
fn eject_refuses_an_agent_that_has_no_bundled_source() {
    let tmp = fixture();
    let base = tmp.path();
    write_agent(&user_agents_dir(base), "custom-only", "no bundled default");

    let outcome = act(base, "eject", Some("custom-only"), None);
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(
        outcome.text.contains("not found or is not a bundled/package agent"),
        "{}",
        outcome.text
    );
    assert!(
        !user_agents_dir(base).join("custom-only.md").metadata().unwrap().is_dir(),
        "sanity: the pre-existing custom file is still a file"
    );
}

// -------------------------------------------------------------------------------------------
// disable / enable
// -------------------------------------------------------------------------------------------

#[test]
fn disable_actually_removes_the_agent_from_the_delegation_view() {
    let tmp = fixture();
    let base = tmp.path();
    assert!(delegatable_names(base).contains(&"scout".to_string()), "precondition");

    let outcome = act(base, "disable", Some("scout"), None);
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(outcome.text.contains("Disabled agent 'scout' via user settings override"), "{}", outcome.text);

    // THE assertion: a subagent call resolving `scout` through the delegation view no longer can.
    let names = delegatable_names(base);
    assert!(!names.contains(&"scout".to_string()), "scout must be undelegatable after disable: {names:?}");
    assert!(names.contains(&"worker".to_string()), "only the named agent is affected: {names:?}");

    // And it is a real settings-file write, not in-memory state.
    let settings = settings_json(&user_settings_path(base));
    assert_eq!(settings["subagents"]["agentOverrides"]["scout"]["disabled"], serde_json::json!(true));
}

#[test]
fn enable_restores_the_agent_and_preserves_its_unrelated_overrides() {
    let tmp = fixture();
    let base = tmp.path();
    // A pre-existing user override that both disables scout AND pins its model.
    std::fs::write(
        user_settings_path(base),
        r#"{"theme":"dark","subagents":{"agentOverrides":{"scout":{"disabled":true,"model":"anthropic/pinned"}}}}"#,
    )
    .unwrap();
    assert!(!delegatable_names(base).contains(&"scout".to_string()), "precondition: scout starts disabled");

    let outcome = act(base, "enable", Some("scout"), None);
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(outcome.text.contains("Enabled agent 'scout' (removed disabled override at"), "{}", outcome.text);

    // Restored to the delegation view...
    assert!(delegatable_names(base).contains(&"scout".to_string()), "scout must be delegatable again");
    // ...with its model override still applied (enable removes ONLY `disabled`)...
    let scout = managed_agent(base, "scout").expect("scout discovered");
    assert_eq!(scout.model.as_ref().map(cyrup_core::ModelId::as_str), Some("anthropic/pinned"));
    // ...and every unrelated key in the settings document intact.
    let settings = settings_json(&user_settings_path(base));
    assert_eq!(settings["theme"], serde_json::json!("dark"));
    assert_eq!(settings["subagents"]["agentOverrides"]["scout"]["model"], serde_json::json!("anthropic/pinned"));
    assert!(settings["subagents"]["agentOverrides"]["scout"].get("disabled").is_none());
}

#[test]
fn enable_on_an_already_enabled_agent_is_a_success_and_writes_nothing() {
    let tmp = fixture();
    let base = tmp.path();
    let outcome = act(base, "enable", Some("scout"), None);
    assert!(!outcome.is_error, "{}", outcome.text);
    assert_eq!(outcome.text, "Agent 'scout' is already enabled.");
    assert!(!user_settings_path(base).exists(), "a no-op enable must not create a settings file");
}

#[test]
fn disable_reports_the_winning_scope_when_a_project_override_overrules_the_write() {
    let tmp = fixture();
    let base = tmp.path();
    // A project-scope override explicitly keeps scout enabled; project beats user (R-SA-012), so a
    // user-scope disable cannot take effect. pi's distinguishing behavior is that it NOTICES.
    std::fs::write(
        project_settings_path(base),
        r#"{"subagents":{"agentOverrides":{"scout":{"disabled":false}}}}"#,
    )
    .unwrap();

    let outcome = act(base, "disable", Some("scout"), Some("user"));
    assert!(outcome.is_error, "a disable that did not take effect must be reported as an error");
    assert!(
        outcome.text.contains("but the agent is still enabled")
            && outcome.text.contains("A higher-precedence project override is likely winning"),
        "{}",
        outcome.text
    );
    // The claim is true: scout really is still delegatable.
    assert!(delegatable_names(base).contains(&"scout".to_string()));
}

#[test]
fn enable_names_the_cross_scope_override_that_still_disables_the_agent() {
    let tmp = fixture();
    let base = tmp.path();
    std::fs::write(
        project_settings_path(base),
        r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#,
    )
    .unwrap();
    std::fs::write(
        user_settings_path(base),
        r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#,
    )
    .unwrap();

    let outcome = act(base, "enable", Some("scout"), Some("user"));
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(
        outcome.text.contains("is still disabled via a project scope override at")
            && outcome.text.contains("Specify agentScope: 'project' to enable it."),
        "{}",
        outcome.text
    );
    assert!(!delegatable_names(base).contains(&"scout".to_string()), "the claim is true");

    // Following the instruction actually works — the whole point of naming the scope.
    let followed = act(base, "enable", Some("scout"), Some("project"));
    assert!(!followed.is_error, "{}", followed.text);
    assert!(delegatable_names(base).contains(&"scout".to_string()));
}

#[test]
fn disable_rejects_an_unknown_agent_and_an_invalid_scope_without_writing() {
    let tmp = fixture();
    let base = tmp.path();

    let unknown = act(base, "disable", Some("nope"), None);
    assert!(unknown.is_error);
    assert!(unknown.text.starts_with("Agent 'nope' not found. Available: scout, worker."), "{}", unknown.text);

    let bad_scope = act(base, "disable", Some("scout"), Some("both"));
    assert!(bad_scope.is_error);
    assert_eq!(bad_scope.text, "agentScope must be 'user' or 'project' for disable.");

    assert!(!user_settings_path(base).exists(), "a rejected disable must not write settings");
    assert!(delegatable_names(base).contains(&"scout".to_string()));
}

// -------------------------------------------------------------------------------------------
// reset
// -------------------------------------------------------------------------------------------

#[test]
fn reset_removes_both_the_custom_file_and_the_settings_override() {
    let tmp = fixture();
    let base = tmp.path();
    // Customize scout both ways: a shadowing user file AND a settings override pinning its model.
    write_agent(&user_agents_dir(base), "scout", "customized scout");
    std::fs::write(
        user_settings_path(base),
        r#"{"theme":"dark","subagents":{"agentOverrides":{"scout":{"model":"anthropic/pinned"}}}}"#,
    )
    .unwrap();
    let before = managed_agent(base, "scout").expect("scout discovered");
    assert_eq!(before.source, AgentSource::User, "precondition: the custom file wins");
    assert_eq!(before.model.as_ref().map(cyrup_core::ModelId::as_str), Some("anthropic/pinned"));

    let outcome = act(base, "reset", Some("scout"), None);
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(outcome.text.contains("Deleted custom user agent file at"), "{}", outcome.text);
    assert!(outcome.text.contains("Removed user settings override at"), "{}", outcome.text);
    assert!(outcome.text.contains("Reset agent 'scout' to its bundled builtin default."), "{}", outcome.text);

    // Both halves of the customization are really gone...
    assert!(!user_agents_dir(base).join("scout.md").exists(), "the custom file must be deleted");
    let settings = settings_json(&user_settings_path(base));
    assert_eq!(settings["theme"], serde_json::json!("dark"), "unrelated settings survive");
    assert!(settings.get("subagents").is_none(), "an emptied subagents block is pruned: {settings}");

    // ...and discovery resolves scout back to the bundled builtin, with no pinned model.
    let after = managed_agent(base, "scout").expect("scout still discovered");
    assert_eq!(after.source, AgentSource::Builtin);
    assert_eq!(after.model, None);
    assert_eq!(after.description, "bundled scout persona");
}

#[test]
fn reset_re_enables_an_agent_that_a_settings_override_had_disabled() {
    let tmp = fixture();
    let base = tmp.path();
    std::fs::write(
        user_settings_path(base),
        r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#,
    )
    .unwrap();
    assert!(!delegatable_names(base).contains(&"scout".to_string()), "precondition");

    let outcome = act(base, "reset", Some("scout"), None);
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(delegatable_names(base).contains(&"scout".to_string()), "reset must restore delegatability");
}

#[test]
fn reset_with_nothing_to_reset_is_a_success_and_hints_at_the_other_scope() {
    let tmp = fixture();
    let base = tmp.path();
    // Nothing at all customized: plain success, no hint.
    let plain = act(base, "reset", Some("worker"), None);
    assert!(!plain.is_error, "{}", plain.text);
    assert_eq!(
        plain.text,
        "Agent 'worker' has no user customization to reset. It is at its bundled builtin default."
    );

    // Customization exists in the OTHER scope: still a success, but it says where to look.
    write_agent(&project_agents_dir(base), "worker", "project worker");
    let hinted = act(base, "reset", Some("worker"), Some("user"));
    assert!(!hinted.is_error, "{}", hinted.text);
    assert!(
        hinted.text.contains("Customization exists in project scope; specify agentScope: 'project' to reset it."),
        "{}",
        hinted.text
    );
    assert!(
        project_agents_dir(base).join("worker.md").exists(),
        "a no-op user reset must not touch the project scope's file"
    );

    // And following the hint really removes it.
    let followed = act(base, "reset", Some("worker"), Some("project"));
    assert!(!followed.is_error, "{}", followed.text);
    assert!(!project_agents_dir(base).join("worker.md").exists());
    assert_eq!(managed_agent(base, "worker").expect("worker discovered").source, AgentSource::Builtin);
}

#[test]
fn reset_refuses_an_agent_with_no_bundled_default_and_points_at_delete() {
    let tmp = fixture();
    let base = tmp.path();
    write_agent(&user_agents_dir(base), "custom-only", "no bundled default");

    let outcome = act(base, "reset", Some("custom-only"), None);
    assert!(outcome.is_error, "{}", outcome.text);
    assert_eq!(
        outcome.text,
        "Agent 'custom-only' has no bundled default to reset to. Use { action: \"delete\", agent: \"custom-only\" } to remove the custom user agent."
    );
    assert!(
        user_agents_dir(base).join("custom-only.md").exists(),
        "a refused reset must not delete the file it declined to reset"
    );
}

// -------------------------------------------------------------------------------------------
// Cross-action: the four compose the way a real customize/undo session uses them
// -------------------------------------------------------------------------------------------

#[test]
fn eject_customize_reset_round_trips_back_to_the_bundled_default() {
    let tmp = fixture();
    let base = tmp.path();

    // eject -> edit the copy -> the edit is live
    assert!(!act(base, "eject", Some("scout"), None).is_error);
    write_agent(&user_agents_dir(base), "scout", "edited after eject");
    assert_eq!(
        managed_agent(base, "scout").expect("scout").description,
        "edited after eject"
    );

    // disable -> gone from delegation; enable -> back
    assert!(!act(base, "disable", Some("scout"), None).is_error);
    assert!(!delegatable_names(base).contains(&"scout".to_string()));
    assert!(!act(base, "enable", Some("scout"), None).is_error);
    assert!(delegatable_names(base).contains(&"scout".to_string()));

    // reset -> the bundled persona is back, verbatim
    assert!(!act(base, "reset", Some("scout"), None).is_error);
    let final_state = managed_agent(base, "scout").expect("scout");
    assert_eq!(final_state.source, AgentSource::Builtin);
    assert_eq!(final_state.description, "bundled scout persona");
    assert_eq!(final_state.file_path, builtin_dir(base).join("scout.md"));
}
