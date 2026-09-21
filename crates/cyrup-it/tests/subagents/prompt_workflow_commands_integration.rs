//! Integration test: `/prompt-workflow` is REACHABLE (G93).
//!
//! The crate shipped seven `prompts/*.md` recipes under `resources/prompts/` and a discovery
//! function for them (`registration::resources::bundled_prompt_files`) whose only caller was that
//! module's own `#[cfg(test)]` block. Upstream reaches those recipes through
//! `registerPromptWorkflowCommands` (`pi-subagents/src/slash/prompt-workflows.ts:269` @v0.34.0),
//! itself called from `registerSlashCommands` (`slash/slash-commands.ts:795-800`) which the
//! extension entry point calls at `extension/index.ts:605`.
//!
//! # VL-S12: `/chain-prompts` is gone, its capability is not
//!
//! This file registered and drove `/chain-prompts` alongside `/prompt-workflow`. Upstream DELETED
//! that command at v0.41.0 and VL-S12 removes it here, so the three tests that named it are gone
//! with it. The capability under it survives on the OTHER command: a recipe whose own `chain:`
//! frontmatter names further recipes still expands to a multi-step chain through
//! `slash_prompt_workflow` (pi `prompt-workflows.ts:286-295`), reaching the SAME
//! `build_chain_steps`/`split_prompt_chain`/`run_prompt_workflow_chain` trio. That surviving
//! branch is pinned in-crate, at
//! `extension/host/mod.rs::prompt_workflows_chain_frontmatter_still_expands_to_multiple_steps`,
//! which asserts the step counts through a spawn cap and so needs no child process; duplicating it
//! here would buy nothing.
//!
//! Every test here drives the REAL user entry point, not the ported functions:
//! `ExtensionHost::execute_native_command` — the exact call
//! `cyrup_session_svc::session::try_execute_extension_command` makes when a user submits
//! `/prompt-workflow …` (`crates/cyrup-session-svc/src/session.rs:958`). A test that called
//! `discover_prompt_workflows` directly would prove nothing that was not already true.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;

// MIGRATION: the original `#[cfg(feature = "test-fixtures")]` here named
// cyrup-ext-subagents' own bin-gating feature. In cyrup-it that spelling names THIS crate's
// features, where no `test-fixtures` exists — so this item would have compiled OUT and the
// test would have passed vacuously. build.rs always builds the fixture binaries, so the gate
// is now a build-script postcondition. See this target's main.rs.
/// Load the subagents extension into a real [`ExtensionHost`] rooted at `cwd` — the same
/// `load_native` call `cyrup-session-svc`'s builder makes, which runs the extension's real `init`.
async fn host_at(cwd: &Path, fixture_script: Option<&Path>) -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: cwd.to_path_buf(),
    }));
    // The fixture, when this test needs one, is named for THIS extension rather than moved into
    // the process environment every concurrently-running test in this binary shares.
    let config = SubagentExtensionConfig {
        spawn_command: fixture_script.map(|script| SpawnCommand {
            binary: crate::support::bins::subagent_fixture(),
            base_args: vec!["--fixture-script".to_string(), script.display().to_string()],
        }),
        ..SubagentExtensionConfig::default()
    };
    host.load_native(Arc::new(SubagentsExtension::with_config_and_cwd(
        config,
        cwd.to_path_buf(),
    )))
    .await
    .expect("the subagents extension loads");
    host
}

/// Run a slash command exactly as a user submission does.
async fn slash(host: &ExtensionHost, name: &str, args: &str) -> String {
    host.execute_native_command(name, args, &CancelToken::new())
        .await
        .expect("routing succeeds")
        .expect("a NATIVE extension owns this command")
        .expect("the handler does not error")
        .expect("the handler returns transcript text")
}

/// Registration proof: `/prompt-workflow` is in the host's native command table after `init`, so
/// the session's `try_execute_extension_command` can route it at all — and the four commands
/// upstream deleted at v0.41.0 are NOT.
///
/// The negative half is VL-S12's palette assertion at the surface that actually matters. A
/// deleted `SlashCommandName` variant whose descriptor was left behind in `SLASH_COMMANDS` still
/// shows up here, advertised to the user and dispatching into nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_workflow_is_registered_and_the_four_deleted_commands_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path(), None).await;
    let names = host.native_command_names();
    assert!(
        names.iter().any(|n| n == "prompt-workflow"),
        "/prompt-workflow must be registered: {names:?}"
    );
    for gone in ["chain", "parallel", "run-chain", "chain-prompts"] {
        assert!(
            !names.iter().any(|n| n == gone),
            "/{gone} was deleted upstream at v0.41.0 and must not be advertised by the host: \
             {names:?}"
        );
    }
}

/// THE reachability proof for the seven bundled recipes: typing `/prompt-workflow list` names every
/// one of them. This is the only path from a keystroke to `bundled_prompt_files()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_workflow_list_names_every_bundled_recipe() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path(), None).await;

    let output = slash(&host, "prompt-workflow", "list").await;
    assert!(output.starts_with("Prompt workflows:"), "got: {output}");
    for recipe in [
        "gather-context-and-clarify",
        "parallel-cleanup",
        "parallel-research",
        "parallel-review",
        "review-loop",
    ] {
        assert!(
            output.contains(&format!("- {recipe}: ")),
            "the bundled recipe {recipe:?} must be listed: {output}"
        );
    }
    // `parallel-context-build`/`parallel-handoff-plan` were deleted upstream in `83b9872` together
    // with the `planner`/`context-builder` roles their every step named. They must NOT be listed —
    // a recipe that dispatches to an agent that no longer exists is a broken suggestion.
    for gone in ["parallel-context-build", "parallel-handoff-plan"] {
        assert!(
            !output.contains(&format!("- {gone}: ")),
            "the removed recipe {gone:?} must not be listed: {output}"
        );
    }

    // pi `:275` — a BARE `/prompt-workflow` lists too, it does not error.
    let bare = slash(&host, "prompt-workflow", "").await;
    assert_eq!(
        bare, output,
        "a bare invocation lists exactly as `list` does"
    );
}

/// A PROJECT recipe is discovered through the same command, and shadows a bundled one by name
/// (`workflows.set(workflow.name, …)`, `prompt-workflows.ts:123`, project dir read last).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_recipe_is_listed_and_shadows_the_bundled_one() {
    let dir = tempfile::tempdir().unwrap();
    let prompts = dir.path().join(".cyrup").join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(
        prompts.join("parallel-review.md"),
        "---\ndescription: PROJECT REVIEW RECIPE\n---\nReview $ARGUMENTS\n",
    )
    .unwrap();

    let host = host_at(dir.path(), None).await;
    let output = slash(&host, "prompt-workflow", "list").await;
    assert!(
        output.contains("- parallel-review: PROJECT REVIEW RECIPE"),
        "the project recipe must shadow the bundled one: {output}"
    );
    assert_eq!(
        output.matches("- parallel-review: ").count(),
        1,
        "shadowing replaces, never duplicates: {output}"
    );
}

/// An unknown name is refused with pi's exact wording (`:281`), never silently listed or run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_recipe_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path(), None).await;
    let output = slash(&host, "prompt-workflow", "no-such-recipe").await;
    assert!(
        output.contains("Unknown prompt workflow: no-such-recipe"),
        "got: {output}"
    );
}

/// A chain naming an unresolvable recipe fails the WHOLE expansion with pi's exact message
/// (`prompt-workflows.ts:290`), rather than silently running the steps that did resolve.
///
/// # VL-S12: re-pointed from `/chain-prompts`, not deleted
///
/// This assertion used to be made through `/chain-prompts a -> no-such -- args` (pi `:321`'s
/// owner-less wording). That command is gone, but the refusal is not: `build_chain_steps` is
/// still reached from `slash_prompt_workflow`'s `chain:` frontmatter branch (`:286-295`), which
/// names the OWNING recipe, so the surviving sentence is upstream's `:290` form —
/// `Unknown prompt workflow in chain '<owner>': <step>`. Same guard, same failure mode, the one
/// entry point that still exists.
///
/// Gutted: let `build_chain_steps` skip an unresolvable step instead of failing → the expansion
/// succeeds with one step and this assertion sees a run report instead of the refusal. Drop the
/// `chain:` branch entirely → `outer`'s own (empty) body runs as a single recipe and the
/// assertion sees that instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recipe_chain_naming_an_unknown_recipe_refuses_the_whole_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let prompts = dir.path().join(".cyrup").join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(
        prompts.join("outer.md"),
        "---\ndescription: an outer recipe whose chain names a missing step\n\
         chain: parallel-research -> no-such\n---\nnever runs\n",
    )
    .unwrap();

    // No fixture binary is configured: if the expansion wrongly SUCCEEDED and tried to run, the
    // spawn would fail loudly rather than quietly passing this assertion.
    let host = host_at(dir.path(), None).await;
    let output = slash(&host, "prompt-workflow", "outer do it").await;
    assert!(
        output.contains("Unknown prompt workflow in chain 'outer': no-such"),
        "pi `:290`'s sentence, naming the owning recipe; got: {output}"
    );
}

// =================================================================================================
// Real execution — a recipe actually spawns a child (gated on the fixture binary)
// =================================================================================================

/// A project recipe run through `/prompt-workflow` reaches a REAL child subprocess, with the
/// recipe's body (after `$ARGUMENTS` substitution) as the task and its `subagent:` frontmatter as
/// the persona. Proves the command is wired to the same executor `/run` uses, not to a stub.
// MIGRATION: the original `#[cfg(feature = "test-fixtures")]` here named
// cyrup-ext-subagents' own bin-gating feature. In cyrup-it that spelling names THIS crate's
// features, where no `test-fixtures` exists — so this item would have compiled OUT and the
// test would have passed vacuously. build.rs always builds the fixture binaries, so the gate
// is now a build-script postcondition. See this target's main.rs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recipe_runs_through_a_real_child_process() {
    let dir = tempfile::tempdir().unwrap();

    // The persona the recipe names, discovered through the real project-scope pipeline.
    let agents = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("recipe-worker.md"),
        "---\nname: recipe-worker\ndescription: fixture persona for prompt-workflow dispatch\n\
         model: fixture/model\n---\n\nYou are a trivial test persona.\n",
    )
    .unwrap();

    let prompts = dir.path().join(".cyrup").join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(
        prompts.join("fixture-flow.md"),
        "---\ndescription: fixture flow\nsubagent: recipe-worker\n---\nHandle $ARGUMENTS now\n",
    )
    .unwrap();

    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "RECIPE_CHILD_RAN"}],
                "usage": {
                    "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "stopReason": "stop"
            }
        }).to_string() }],
        "exit_code": 0
    });
    let script_path = dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).unwrap();

    let host = host_at(dir.path(), Some(&script_path)).await;
    let output = slash(&host, "prompt-workflow", "fixture-flow the backlog").await;

    assert!(
        output.contains("RECIPE_CHILD_RAN"),
        "the recipe must have reached a real child process: {output}"
    );
}

/// A recipe whose own `chain:` frontmatter names two further recipes runs BOTH of them as one
/// native chain, through the same `run_prompt_workflow_chain` walker (pi hands the lowered
/// `chain` array to the one executor, `prompt-workflows.ts:286-295` @v0.34.0).
///
/// # VL-S12: re-pointed from `/chain-prompts`, not deleted
///
/// The declaration used to be typed at `/chain-prompts flow-a -> flow-b -- the backlog`. That
/// command is gone; the frontmatter form is upstream's other producer of the identical step list
/// and it reaches `split_prompt_chain` → `build_chain_steps` → `run_prompt_workflow_chain`
/// unchanged.
///
/// This is NOT a duplicate of the in-crate step-count proof at
/// `extension/host/mod.rs::prompt_workflows_chain_frontmatter_still_expands_to_multiple_steps`,
/// which counts steps through a one-spawn cap and deliberately spawns nothing. The half only an
/// IT can make is this one: TWO REAL CHILD PROCESSES ran, and both of their outputs are in the
/// rendered report.
// MIGRATION: the original `#[cfg(feature = "test-fixtures")]` here named
// cyrup-ext-subagents' own bin-gating feature. In cyrup-it that spelling names THIS crate's
// features, where no `test-fixtures` exists — so this item would have compiled OUT and the
// test would have passed vacuously. build.rs always builds the fixture binaries, so the gate
// is now a build-script postcondition. See this target's main.rs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recipe_chain_declared_in_frontmatter_runs_every_step_as_a_real_child() {
    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("recipe-worker.md"),
        "---\nname: recipe-worker\ndescription: fixture persona for recipe-chain dispatch\n\
         model: fixture/model\n---\n\nYou are a trivial test persona.\n",
    )
    .unwrap();

    let prompts = dir.path().join(".cyrup").join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    for name in ["flow-a", "flow-b"] {
        std::fs::write(
            prompts.join(format!("{name}.md")),
            format!("---\ndescription: {name}\nsubagent: recipe-worker\n---\n{name}: $ARGUMENTS\n"),
        )
        .unwrap();
    }

    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "CHAINED_RECIPE_RAN"}],
                "usage": {
                    "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "stopReason": "stop"
            }
        }).to_string() }],
        "exit_code": 0
    });
    let script_path = dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).unwrap();

    // The recipe whose `chain:` frontmatter names the two above — pi `prompt-workflows.ts:286`.
    std::fs::write(
        prompts.join("outer.md"),
        "---\ndescription: outer\nchain: flow-a -> flow-b\n---\nnever runs: $ARGUMENTS\n",
    )
    .unwrap();

    let host = host_at(dir.path(), Some(&script_path)).await;
    let output = slash(&host, "prompt-workflow", "outer the backlog").await;

    assert_eq!(
        output.matches("CHAINED_RECIPE_RAN").count(),
        2,
        "both chained recipes must have run as real children — one occurrence means the \
         `chain:` branch was skipped and `outer`'s own body ran instead: {output}"
    );
}
