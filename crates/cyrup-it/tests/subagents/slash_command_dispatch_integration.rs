//! Integration test: `/subagents-refine` is REACHABLE — the slash surface over the refinement
//! write half, driven through the REAL `SubagentsExtension::execute_command` (the
//! `cyrup_ext::native::NativeExtension` trait method a user typing the command reaches).
//!
//! # What used to be here, and why it is not any more
//!
//! This file was written for R-SA-130, which closed the stub "recognized, not yet executing" arm
//! that `dispatch_slash` fell into for 8 of the 13 registered commands. Five of its tests drove
//! `/chain`, `/parallel` and `/run-chain`. **Upstream DELETED those three commands (and
//! `/chain-prompts`) at v0.41.0** and VL-S12 removes them here, so there is no command left for
//! those tests to reach: they are not re-pointed, they are gone, because what they pinned was the
//! slash SURFACE and that surface no longer exists.
//!
//! Nothing they proved about the machinery UNDER that surface is lost — the chain-graph walker,
//! the bounded parallel fan-out and saved-chain discovery are all still reached, by the `subagent`
//! tool's own `chain[]`/`tasks[]` arms, and are pinned as such in
//! `tool_parallel_chain_integration.rs`. Only the four dead entry points went.
//!
//! No mocking: the dispatch below drives the REAL `execute_command` path.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Write a trivial agent persona `.md` file to `<cwd>/.cyrup/agents/<local_name>.md` — the exact
/// project-scope discovery root `SubagentExecutor::discovery_config` scans, so the fixture persona
/// is genuinely discovered through the real discovery pipeline.
fn write_fixture_persona(cwd: &Path, local_name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{local_name}.md")),
        format!(
            "---\nname: {local_name}\ndescription: a trivial fixture persona for slash-command \
             dispatch tests\nmodel: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

fn command_ctx(cwd: &Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, false, cwd.to_path_buf())
}

// =====================================================================================================
// /subagents-refine — VL-S13: the SECOND production surface for the refinement write half
// =====================================================================================================

/// `/subagents-refine` is a real production surface, not a convenience alias: upstream's own
/// missing-agent refusal (`agents/agent-refinements.ts:548`) names this command by name, so the
/// tool's error message is a lie until the command dispatches. It is also the surface with the
/// least coverage — the tool arm has six in-crate routing tests, while this one shares only its
/// BODY (`extension/tool/refinement::run_refinement_action`) with them. What is unique to this
/// arm, and therefore untested until now, is its own argument parsing and the dispatch that
/// reaches that body at all.
///
/// All three cases run through the REAL
/// `SubagentsExtension::execute_command` (the `cyrup_ext::native::NativeExtension` trait method a
/// user typing the command reaches) — the one surface this file still has to pin, now that the
/// four v0.41.0-deleted commands its other tests drove are gone.
///
/// Upstream refuses `parts.length !== 1` (`slash/slash-commands.ts:965`) — BOTH zero words and
/// two — with one usage sentence, so both directions are asserted; a parser that only guarded the
/// empty case would pass a one-sided test.
///
/// The one-word case deliberately asserts pi `:593`'s sentence rather than a usage error, because
/// that sentence is produced DEEP inside `handle_refinement_action` — past argument parsing, past
/// agent resolution, past the existing-file read, inside `refine` itself. Nothing short of the
/// real handler running produces it, so it proves dispatch rather than mere recognition. No child
/// process is spawned on this path (`:593` refuses above the launch), which is why no fixture
/// binary is configured here: if this arm somehow tried to spawn, it would fail loudly.
///
/// Gutted: delete the `[agent] = parts[..]` guard → the zero-word case dispatches with an empty
/// agent and gets the MISSING-AGENT sentence instead of the usage one, and the two-word case
/// refuses with `agent not found: two`, so both usage assertions fail. Drop the
/// `SlashCommandName::SubagentsRefine` dispatch arm → `execute_command` returns the
/// no-handler `ExtError` and all three `.expect`s fail. Point the arm at a stub instead of
/// `run_refinement_action` → the one-word assertion fails, because only the real handler renders
/// `:593`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagents_refine_command_refuses_bad_arity_and_otherwise_reaches_the_real_handler() {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "probe");

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    // Zero words — upstream `:965`'s `parts.length !== 1`.
    let zero = extension
        .execute_command("subagents-refine", "", &ctx)
        .await
        .expect("execute_command renders a SubagentError as text, not an ExtError")
        .expect("a rendered message");
    assert!(
        zero.contains("Usage: /subagents-refine <agent>"),
        "zero words must be refused with upstream's usage sentence: {zero}"
    );

    // Two words — the SAME refusal. `parts.length !== 1` is not `parts.length === 0`.
    let two = extension
        .execute_command("subagents-refine", "probe extra", &ctx)
        .await
        .expect("execute_command renders a SubagentError as text, not an ExtError")
        .expect("a rendered message");
    assert!(
        two.contains("Usage: /subagents-refine <agent>"),
        "two words must be refused with the same usage sentence: {two}"
    );

    // One word — reaches the real handler. `probe` resolves through real discovery, and with no
    // evidence on disk the handler lands on pi `:593`.
    let one = extension
        .execute_command("subagents-refine", "probe", &ctx)
        .await
        .expect("execute_command renders a SubagentError as text, not an ExtError")
        .expect("a rendered message");
    assert_eq!(
        one,
        "No bounded recent evidence was found for 'probe'. No proposal child was launched and no \
         overlay was written.",
        "one word must reach `handle_refinement_action` itself — this sentence exists nowhere else"
    );

    // And, as everywhere else in this feature, the proof that nothing was written.
    let overlay = cyrup_ext_subagents::exec::agent_refinements::get_agent_refinement_path(
        work_dir.path(),
        "probe",
    )
    .expect("a usable agent name");
    assert!(
        !overlay.exists(),
        "the slash surface must not write an overlay on the no-evidence path: {}",
        overlay.display()
    );
}
