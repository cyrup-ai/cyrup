//! Integration test: the two live seams that decide WHICH directory a session's subagent
//! discovery is rooted at, and the one live seam that canonicalizes an agent alias before a
//! dispatch runs. Both were shipped without coverage.
//!
//! # G101 — `projectRootResolution` must be OBSERVABLE
//!
//! `SubagentExecutor::discovery_dirs_config` resolves the project root through
//! `find_configured_project_root` (pi `findConfiguredProjectRoot`, `agents.ts:657-672` @v0.43.0),
//! and `SubagentExecutor::discovery_config` then keys the PROJECT `settings.json` on that resolved
//! root rather than on the raw `cwd` (pi `getProjectAgentSettingsPath`, `agents.ts:678-681`, which
//! is `findConfiguredProjectRoot(cwd)` + `"settings.json"`).
//!
//! Both halves matter and neither is visible from a unit test of the helper alone. Keying the
//! settings file on `cwd` was in fact the bug: a session started in a subdirectory read a settings
//! file that does not exist while its agents came from the real project root — and, worse, it made
//! `subagents.projectRootResolution` structurally UNOBSERVABLE, because the very setting that MOVES
//! the root lives in the file the root selects. This file makes it observable, from the outside, on
//! the live path (`SubagentExecutor::resolve_agent`, whose only route to a directory list is
//! `discovery_config`), and asserts it:
//!
//! - **half A** (dirs): with `git-root` resolution the nested cwd discovers the REPO ROOT's agent
//!   and stops seeing its own nearest-candidate agent — the root really moved;
//! - **half B** (settings): the `subagents.defaultModel` that the repo-root settings file declares
//!   is applied to that agent — the settings file was read from the RESOLVED root, not from `cwd`.
//!
//! The `nearest` control flips BOTH halves back, which is what proves the assertions are measuring
//! the resolution rather than a constant.
//!
//! # G97 — `canonicalize_execution_params` is on the LIVE dispatch path
//!
//! `SubagentTool::call` invokes `canonicalize_execution_params` exactly once, immediately before the
//! mode arm (pi `subagent-executor.ts:4923-4925`). Deleting that one call left the whole crate suite
//! green: the only tests were unit tests calling the helper directly. The per-site location suffix
//! pi appends to an ambiguity (`(task 2)`, `(step 3, task 1)`) is produced NOWHERE else in this
//! crate, so asserting it through `Tool::execute` pins the call site itself.
//!
//! Everything here is real: real directory trees under real `tempfile::tempdir()`s, the real
//! discovery pipeline, and the real `Tool::execute` entry point. No child process is spawned — every
//! assertion lands on a refusal or a resolution that happens strictly before any subprocess would.
//!
//! Separate compilation unit from `lib.rs`, so NOT bound by that crate's `#![forbid(unsafe_code)]`;
//! the user-scope root is supplied as `SubagentExtensionConfig::roots` rather than exported, so
//! this file mutates no process-global state and needs no lock.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;


use cyrup_core::{CancelToken, Tool, ToolCallId};
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::discovery::types::AgentReadScope;
use cyrup_ext_subagents::error::SubagentError;
use cyrup_ext_subagents::extension::{SubagentExecutor, SubagentsExtension};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

const EXTRA_DIRS_ENV_VAR: &str = "CYRUP_SUBAGENT_EXTRA_AGENT_DIRS";

/// An empty user-scope root for one test, so the USER-scope `settings.json` / `~/.cyrup/agents` of
/// the developer or CI machine can never contribute. Passed to `resolve_agent` as `roots`
/// rather than exported, so nothing process-global moves and no lock is needed.
///
/// `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` is asserted-absent rather than cleared: it is a developer
/// escape hatch, and a test that silently deleted it could mask an ambient value instead of
/// reporting it. If it IS set, this test cannot be hermetic and says so.
struct HomeSandbox {
    home: tempfile::TempDir,
}

impl HomeSandbox {
    fn enter() -> Self {
        assert!(
            std::env::var_os(EXTRA_DIRS_ENV_VAR).is_none(),
            "{EXTRA_DIRS_ENV_VAR} is set in this environment; it would add an ambient dir to the \
             User tier and make this test's discovery result non-hermetic. Unset it to run this \
             suite."
        );
        Self { home: tempfile::tempdir().expect("home tempdir") }
    }

    fn path(&self) -> &Path {
        self.home.path()
    }
}

fn write_agent(dir: &Path, name: &str, frontmatter_extra: &str) {
    std::fs::create_dir_all(dir).expect("mkdir agent dir");
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: The {name} agent\n{frontmatter_extra}---\n\nYou are {name}.\n"),
    )
    .expect("write agent file");
}

/// Write `<root>/.cyrup/agents/settings.json` with the given `subagents` block. Creating the
/// directory is also what makes `root` a project-root CANDIDATE.
fn write_project_settings(root: &Path, subagents_json: &str) {
    let dir = root.join(".cyrup").join("agents");
    std::fs::create_dir_all(&dir).expect("mkdir .cyrup/agents");
    std::fs::write(dir.join("settings.json"), format!("{{\"subagents\":{subagents_json}}}"))
        .expect("write settings.json");
}

/// The two-candidate repository this file's G101 tests run against:
///
/// ```text
/// <tmp>/repo/.git/                              <- the git root
/// <tmp>/repo/.cyrup/agents/settings.json        <- candidate #2 (the git root), carries the policy
/// <tmp>/repo/.cyrup/agents/repo-agent.md
/// <tmp>/repo/packages/app/.cyrup/agents/        <- candidate #1 (the NEAREST candidate)
/// <tmp>/repo/packages/app/.cyrup/agents/app-agent.md
/// ```
///
/// `cwd` is `<tmp>/repo/packages/app`. Which of the two agents is discoverable is therefore a direct
/// readout of which candidate `find_configured_project_root` picked.
fn build_two_candidate_repo(tmp: &Path, repo_root_settings: &str) -> std::path::PathBuf {
    let repo = tmp.join("repo");
    let app = repo.join("packages").join("app");
    write_project_settings(&repo, repo_root_settings);
    write_agent(&repo.join(".cyrup").join("agents"), "repo-agent", "");
    std::fs::create_dir_all(app.join(".cyrup").join("agents")).expect("mkdir app agents dir");
    write_agent(&app.join(".cyrup").join("agents"), "app-agent", "");
    std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");
    app
}

// ================================================================================================
// G101 — project-root wiring, both halves
// ================================================================================================

/// `projectRootResolution: "git-root"`, declared at the repository root, moves the ENTIRE project
/// scope out to that root for a cwd nested below it — the directory list (half A) AND the
/// `settings.json` the `subagents.*` layer is read from (half B).
///
/// pi's rule (`agents.ts:657-672`) reaches the repo root here through its `||` at `:667`: the
/// nearest candidate (`packages/app`) declares nothing, the git root IS a candidate, and the git
/// root itself declares `"git-root"`. The nested sub-project needs no configuration of its own —
/// which is exactly the shape a monorepo has, and exactly the shape that is unobservable if the
/// settings file is keyed on `cwd`.
#[tokio::test]
async fn git_root_resolution_moves_both_the_agent_dirs_and_the_settings_file() {
    let sandbox = HomeSandbox::enter();
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = build_two_candidate_repo(
        tmp.path(),
        r#"{"projectRootResolution":"git-root","defaultModel":"prov/repo-root-model"}"#,
    );
    let executor = SubagentExecutor::new();

    // Half A — the READ DIRS came from the git root: its agent is visible...
    let repo_agent = executor
        .resolve_agent(&app, "repo-agent", AgentReadScope::Both, &Roots::sandboxed(sandbox.path()))
        .expect("the repo-root project agent must be discoverable from the nested cwd");
    assert_eq!(repo_agent.name, "repo-agent");

    // ...and the nearest candidate's own agent dir is NOT scanned at all, which is what proves the
    // root MOVED rather than merely widened.
    let missed = executor
        .resolve_agent(&app, "app-agent", AgentReadScope::Both, &Roots::sandboxed(sandbox.path()))
        .expect_err("git-root resolution must stop scanning the nearest candidate's agent dir");
    assert!(
        matches!(missed, SubagentError::AgentNotFound(_)),
        "expected a not-found, got: {missed}"
    );

    // Half B — the SETTINGS file was read from the same resolved root: its `defaultModel` reached
    // the merge. Keyed on `cwd` this would have read `<app>/.cyrup/agents/settings.json`, which does
    // not exist, and the agent would carry no model at all.
    assert_eq!(
        repo_agent.model.as_ref().map(cyrup_core::ModelId::as_str),
        Some("prov/repo-root-model"),
        "the project `settings.json` must be keyed on the RESOLVED project root, not on cwd"
    );
}

/// The control for the test above, and the half that makes it an assertion rather than a constant:
/// with the SAME tree and the SAME nested cwd, `projectRootResolution: "nearest"` pins the root at
/// `packages/app` and flips every observation.
///
/// Note that the repo root still declares `defaultModel` here. It has no effect, because the
/// settings file is keyed on the root that resolution PICKED — so this also pins half B in the
/// negative direction: a `settings.json` at a non-selected candidate must not leak in.
#[tokio::test]
async fn nearest_resolution_pins_both_the_agent_dirs_and_the_settings_file_at_the_nested_root() {
    let sandbox = HomeSandbox::enter();
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = build_two_candidate_repo(
        tmp.path(),
        r#"{"projectRootResolution":"nearest","defaultModel":"prov/repo-root-model"}"#,
    );
    let executor = SubagentExecutor::new();

    let app_agent = executor
        .resolve_agent(&app, "app-agent", AgentReadScope::Both, &Roots::sandboxed(sandbox.path()))
        .expect("the nearest candidate's own agent must be discoverable");
    assert_eq!(app_agent.name, "app-agent");

    let missed = executor
        .resolve_agent(&app, "repo-agent", AgentReadScope::Both, &Roots::sandboxed(sandbox.path()))
        .expect_err("nearest resolution must not reach out to the repository root's agents");
    assert!(
        matches!(missed, SubagentError::AgentNotFound(_)),
        "expected a not-found, got: {missed}"
    );

    assert_eq!(
        app_agent.model, None,
        "the repo root's `defaultModel` must NOT apply once resolution pinned the nested root"
    );
}

// ================================================================================================
// G97 — `canonicalize_execution_params` on the live `Tool::execute` path
// ================================================================================================

/// Two agents that both claim the alias `prophet`, so any resolution of that alias is ambiguous.
fn write_ambiguous_alias_pair(cwd: &Path) {
    let agents = cwd.join(".cyrup").join("agents");
    write_agent(&agents, "seer", "aliases: prophet\n");
    write_agent(&agents, "augur", "aliases: prophet\n");
}

async fn dispatch(cwd: &Path, home: &Path, params: serde_json::Value) -> Result<(), String> {
    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home),
            ..SubagentExtensionConfig::default()
        },
        cwd.to_path_buf(),
    );
    extension
        .subagent_tool()
        .execute(
            ToolCallId::from("suba-g97"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The LIVE seam: `SubagentTool::call` -> `canonicalize_execution_params`. An ambiguous alias at a
/// `tasks[]` site aborts the WHOLE dispatch with pi's message plus the per-site location suffix
/// (`subagent-executor.ts:1682-1734`, driven from `:4923-4925`).
///
/// The `(task 2)` suffix is the load-bearing half: it is produced by exactly one expression in this
/// crate (`format!("task {}", index + 1)` inside `canonicalize_execution_params`), so an error text
/// carrying it can only have come from that call site. Deleting the call from `SubagentTool::call`
/// previously left every test in the crate green.
#[tokio::test]
async fn an_ambiguous_alias_in_a_task_list_aborts_the_live_tool_dispatch_with_its_location() {
    let sandbox = HomeSandbox::enter();
    let cwd = tempfile::tempdir().expect("tempdir");
    write_ambiguous_alias_pair(cwd.path());

    let err = dispatch(cwd.path(), sandbox.path(), serde_json::json!({
            "tasks": [
                { "agent": "seer", "task": "a" },
                { "agent": "prophet", "task": "b" }
            ]
        }),
    )
    .await
    .expect_err("an ambiguous alias must abort the dispatch, not fan out");

    assert_eq!(
        err, "Ambiguous agent alias 'prophet': augur, seer (task 2)",
        "the tool's own dispatch must canonicalize, and name the offending fan-out position"
    );
}

/// The same seam at a chain's static-parallel site, whose suffix is `(step N, task M)` — the other
/// expression only `canonicalize_execution_params` produces.
#[tokio::test]
async fn an_ambiguous_alias_in_a_chain_parallel_step_aborts_the_live_tool_dispatch() {
    let sandbox = HomeSandbox::enter();
    let cwd = tempfile::tempdir().expect("tempdir");
    write_ambiguous_alias_pair(cwd.path());

    let err = dispatch(cwd.path(), sandbox.path(), serde_json::json!({
            "chain": [
                { "agent": "seer", "task": "first" },
                { "parallel": [
                    { "agent": "augur", "task": "x" },
                    { "agent": "prophet", "task": "y" }
                ] }
            ]
        }),
    )
    .await
    .expect_err("an ambiguous alias must abort the dispatch, not run the chain");

    assert_eq!(
        err, "Ambiguous agent alias 'prophet': augur, seer (step 2, task 2)",
        "the chain site's location suffix names both the step and the task"
    );
}

/// The top-level SINGLE `agent` carries NO location suffix upstream (`subagent-executor.ts:1688`
/// passes `undefined`), which is the third distinct shape the call site produces. Asserted so the
/// suffix cannot be made unconditional.
#[tokio::test]
async fn an_ambiguous_top_level_alias_aborts_the_live_tool_dispatch_without_a_location() {
    let sandbox = HomeSandbox::enter();
    let cwd = tempfile::tempdir().expect("tempdir");
    write_ambiguous_alias_pair(cwd.path());

    let err = dispatch(cwd.path(), sandbox.path(), serde_json::json!({ "agent": "prophet", "task": "decide" }),
    )
    .await
    .expect_err("an ambiguous alias must abort the dispatch");

    assert_eq!(err, "Ambiguous agent alias 'prophet': augur, seer");
}
