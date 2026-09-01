//! Integration test: `/prompt-workflow` and `/chain-prompts` are REACHABLE (G93).
//!
//! The crate shipped seven `prompts/*.md` recipes under `resources/prompts/` and a discovery
//! function for them (`registration::resources::bundled_prompt_files`) whose only caller was that
//! module's own `#[cfg(test)]` block. Upstream reaches those recipes through two slash commands
//! registered by `registerPromptWorkflowCommands` (`pi-subagents/src/slash/prompt-workflows.ts:269,
//! 303` @v0.34.0), itself called from `registerSlashCommands` (`slash/slash-commands.ts:795-800`)
//! which the extension entry point calls at `extension/index.ts:605`.
//!
//! Every test here drives the REAL user entry point, not the ported functions:
//! `ExtensionHost::execute_native_command` — the exact call
//! `cyrup_session_svc::session::try_execute_extension_command` makes when a user submits
//! `/prompt-workflow …` (`crates/cyrup-session-svc/src/session.rs:958`). A test that called
//! `discover_prompt_workflows` directly would prove nothing that was not already true.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

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

/// Registration proof: both commands are in the host's native command table after `init`, so the
/// session's `try_execute_extension_command` can route `/prompt-workflow` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_prompt_commands_are_registered_with_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path(), None).await;
    let names = host.native_command_names();
    assert!(
        names.iter().any(|n| n == "prompt-workflow"),
        "/prompt-workflow must be registered: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "chain-prompts"),
        "/chain-prompts must be registered: {names:?}"
    );
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
    assert_eq!(bare, output, "a bare invocation lists exactly as `list` does");

    // pi `:308-310` — `/chain-prompts` with an empty declaration lists the same set.
    let chain_list = slash(&host, "chain-prompts", "").await;
    assert_eq!(chain_list, output);
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

/// A `/chain-prompts` declaration naming an unresolvable recipe fails the WHOLE expansion with
/// pi's exact message (`:321`), rather than silently running the steps that did resolve.
///
/// (Upstream's `names.length === 0` usage-line branch at `:314-316` is not exercised here because
/// it is unreachable through this entry point: `splitChainDeclaration` trims the declaration
/// (`:216`), the empty declaration is already handled by the `list` branch at `:308`, and
/// `splitPromptChain` splits on the literal `" -> "` — so any surviving declaration yields at
/// least one name. The branch is ported for fidelity; the reachable failure is this one.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_prompts_refuses_a_chain_containing_an_unknown_recipe() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path(), None).await;
    let output = slash(&host, "chain-prompts", "parallel-research -> no-such -- do it").await;
    assert!(
        output.contains("Unknown prompt workflow: no-such"),
        "got: {output}"
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

/// `/chain-prompts a -> b -- args` runs BOTH recipes as one native chain through the same
/// `run_or_background_chain` walker `/chain` uses (pi hands the lowered `chain` array to the one
/// executor, `prompt-workflows.ts:319-324` @v0.34.0).
// MIGRATION: the original `#[cfg(feature = "test-fixtures")]` here named
// cyrup-ext-subagents' own bin-gating feature. In cyrup-it that spelling names THIS crate's
// features, where no `test-fixtures` exists — so this item would have compiled OUT and the
// test would have passed vacuously. build.rs always builds the fixture binaries, so the gate
// is now a build-script postcondition. See this target's main.rs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_prompts_runs_every_recipe_as_one_native_chain() {

    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("recipe-worker.md"),
        "---\nname: recipe-worker\ndescription: fixture persona for chain-prompts dispatch\n\
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

    // SAFETY: as above.
    let host = host_at(dir.path(), Some(&script_path)).await;
    let output = slash(&host, "chain-prompts", "flow-a -> flow-b -- the backlog").await;


    assert_eq!(
        output.matches("CHAINED_RECIPE_RAN").count(),
        2,
        "both recipes must have run as chain steps: {output}"
    );
}
