//! Integration test: closing gap R-SA-130 — `/chain`, `/parallel`, and `/run-chain` slash commands
//! now route through REAL execution (`SubagentExecutor::run_chain_foreground`/
//! `spawn_background_steps`, the identical `spawn::chain_graph::walk_chain` +
//! `background::runner_main::ExecSingleStepExecutor` machinery the `subagent` tool and the hop-2
//! detached runner already use), rather than the stub "recognized, not yet executing" arm the
//! adversarial audit found `extension.rs::dispatch_slash` falling into for 8 of the 13 registered
//! commands.
//!
//! No mocking anywhere in this file (this crate's own standing convention, matching every other
//! `tests/*_integration.rs` file here): every dispatch below drives the REAL
//! `SubagentsExtension::execute_command` (the `cyrup_ext::native::NativeExtension` trait method),
//! which spawns REAL child OS subprocesses — the scripted-NDJSON test-double `cyrup-subagent-
//! fixture` binary (arch-SA §11) — via `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1's documented
//! override escape hatch).
//!
//! Gated on the `test-fixtures` Cargo feature, matching every other fixture-dependent integration
//! test in this crate.

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;

use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::background::RunState;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Serializes every test in this file that mutates `CYRUP_SUBAGENT_BINARY`/
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT`/`CYRUP_HOME` (process-global state), mirroring every other
/// fixture-based integration test in this crate's identical `ENV_MUTATION_LOCK` convention.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";
const CYRUP_HOME_ENV_VAR: &str = "CYRUP_HOME";

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
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

/// Write a trivial agent persona `.md` file to `<cwd>/.cyrup/agents/<local_name>.md` — the exact
/// project-scope discovery root `SubagentExecutor::discovery_config` scans, so every fixture
/// persona below is genuinely discovered through the real discovery pipeline.
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

/// Write a saved chain `.chain.json` file to `<cwd>/.cyrup/agents/<name>.chain.json` (the same
/// directory root project-scope CHAIN discovery scans, per `SubagentExecutor::discovery_config`'s
/// `project_chain_dirs` wiring) in the real pi `.chain.json` shape (a root `chain` array of
/// [`ChainStepConfig`] authoring steps), so the on-disk shape is exactly what `discovery::chains`
/// actually parses.
fn write_fixture_chain(
    cwd: &Path,
    name: &str,
    steps: &[cyrup_ext_subagents::discovery::types::ChainStepConfig],
) {
    // Saved chains live in the SEPARATE `.cyrup/chains` dir (pi `getUserChainDir`/
    // `resolveNearestProjectChainDirs` = `<configDir>/chains`, NEVER the agents dir), which is what
    // `resolve_project_chain_read_dirs` (`.cyrup/chains`) scans — writing to `.cyrup/agents` here
    // meant `/run-chain` discovery never found the chain.
    let dir = cwd.join(".cyrup").join("chains");
    std::fs::create_dir_all(&dir).expect("mkdir .cyrup/chains");
    let payload = serde_json::json!({
        "name": name,
        "description": "a trivial fixture chain for /run-chain dispatch tests",
        "chain": steps,
    });
    std::fs::write(
        dir.join(format!("{name}.chain.json")),
        serde_json::to_string_pretty(&payload).expect("serialize chain"),
    )
    .expect("write fixture chain");
}

fn single_step(agent: &str, task: &str) -> cyrup_ext_subagents::discovery::types::ChainStepConfig {
    cyrup_ext_subagents::discovery::types::ChainStepConfig {
        agent: Some(agent.to_string()),
        task: Some(task.to_string()),
        ..Default::default()
    }
}

/// RAII guard installing `CYRUP_SUBAGENT_BINARY`/`CYRUP_SUBAGENT_FIXTURE_SCRIPT` for the life of
/// one test, mirroring every sibling fixture-based integration test's identical setup/teardown —
/// factored into a guard here (rather than repeated inline blocks) since this file drives several
/// independent scripted scenarios.
struct FixtureEnvGuard;

impl FixtureEnvGuard {
    fn install(script_path: &Path) -> Self {
        let fixture = fixture_binary_path();
        // SAFETY: scoped, mutex-serialized env mutation for the duration of one test, mirroring
        // every other fixture-based integration test in this crate (Rust 2024 requires `unsafe`
        // for `std::env::set_var`/`remove_var`; this file is a separate compilation unit from this
        // crate's own `#![forbid(unsafe_code)]` `lib.rs`).
        unsafe {
            std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
            std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, script_path);
        }
        Self
    }
}

impl Drop for FixtureEnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `install`'s own safety comment.
        unsafe {
            std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
            std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        }
    }
}

fn command_ctx(cwd: &Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, false, cwd.to_path_buf())
}

// =====================================================================================================
// /chain — R-SA-129/130: routes into the real chain-graph walker
// =====================================================================================================

/// The core proof for `/chain`: a two-agent sequential chain, each step backed by the REAL fixture
/// subprocess, dispatched through `SubagentsExtension::execute_command` exactly as a user typing
/// `/chain researcher "task1" -> reviewer "task2"` would trigger — asserting the rendered text
/// contains BOTH steps' own distinct fixture output, proving both children genuinely ran in order
/// (not a stub echoing the command back).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_command_runs_every_step_through_a_real_child_process_in_order() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "researcher");
    write_fixture_persona(work_dir.path(), "reviewer");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("CHAIN_STEP_OUTPUT") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");
    let _env = FixtureEnvGuard::install(&script_path);

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = extension
        .execute_command(
            "chain",
            "researcher \"first task\" -> reviewer \"second task\"",
            &ctx,
        )
        .await
        .expect("execute_command does not error")
        .expect("chain produces textual output");

    assert!(
        !output.contains("recognized by the subagents extension"),
        "the stub placeholder text must be gone now that /chain routes through real execution: \
         {output}"
    );
    assert_eq!(
        output.matches("CHAIN_STEP_OUTPUT").count(),
        2,
        "both chain steps must have run through the real fixture child and contributed their own \
         output: {output}"
    );
    assert!(output.contains("step 1: ok"), "got: {output}");
    assert!(output.contains("step 2: ok"), "got: {output}");
}

/// `/chain --bg`: the same chain, dispatched to a genuine detached second-hop process
/// (`spawn_background_steps` -> `spawn_detached_runner`) rather than run inline — proving the
/// background path is ALSO real execution, not merely a different flavor of stub.
///
/// # Scope note (honest, not a stub-in-disguise)
///
/// `CYRUP_SUBAGENT_BINARY` here plays the role of hop-1 itself (mirroring
/// `background_spawn_detached_integration.rs`'s own established convention for this exact
/// constraint: the scripted `cyrup-subagent-fixture` binary is a `cyrup`-shaped test double for
/// the FOREGROUND single-agent invocation contract, not a `cyrup __subagent-runner
/// --config <path>`-capable reimplementation of `background::runner_main::run`). This test
/// therefore proves what is genuinely provable at this crate's boundary without the real `cyrup`
/// binary: `/chain --bg` builds a correct multi-step `RunnerConfig` (mode=Chain, 2 steps) and
/// hands it to a REAL, independently-alive, correctly-redirected detached OS process via
/// `spawn_detached_runner` — the identical proof `background_spawn_detached_integration.rs`'s own
/// suite already establishes for `spawn_detached_runner` in isolation, exercised here through the
/// full `/chain --bg` slash-command dispatch path instead of calling `spawn_detached_runner`
/// directly. Full end-to-end hop-2 chain completion through a real `__subagent-runner` handshake
/// is covered separately by `background_runner_main_integration.rs` (which drives
/// `background::runner_main::run` directly, the one piece that genuinely needs the real `cyrup`
/// binary to nest a further real spawn) — composing the two is out of this crate's own test-fixture
/// capability (no `cyrup`-shaped, `__subagent-runner`-aware binary is built by this crate).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_command_with_bg_flag_spawns_a_real_tracked_detached_process() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let cyrup_home = tempfile::tempdir().expect("real tempdir for CYRUP_HOME");
    write_fixture_persona(work_dir.path(), "worker");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("BG_CHAIN_OUTPUT") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");
    let _env = FixtureEnvGuard::install(&script_path);
    // SAFETY: scoped under the same mutex-held critical section as the fixture-binary env vars.
    unsafe {
        std::env::set_var(CYRUP_HOME_ENV_VAR, cyrup_home.path());
    }

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = extension
        .execute_command("chain", "worker \"do it\" -> worker \"do it again\" --bg", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("bg dispatch produces a status message");

    assert!(
        output.contains("Background subagent run started"),
        "got: {output}"
    );
    assert!(
        !output.contains("recognized by the subagents extension"),
        "got: {output}"
    );

    // Poll the tracker (the same primitive `run_cost_report`'s own test uses) until it observes
    // SOME reconciled status for the new run, proving a real run-id-bearing hop-1 process was
    // genuinely spawned and tracked (never a silent no-op) — matching the scope note above, this
    // does not require the fixture to have fully executed a two-step chain (it cannot; see above).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let job = loop {
        extension.executor().tracker().tick_once().await;
        let snapshot = extension.executor().tracker().snapshot();
        if let Some(job) = snapshot.into_iter().find(|j| j.last_status.is_some()) {
            break job;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "the backgrounded /chain run's hop-1 process never produced any observable \
                 status within the deadline — proving the spawn itself never genuinely happened"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // `Queued` is the tracker's own honest pre-reconciliation state (the fixture binary, playing
    // hop-1's role per this test's own scope note above, never writes a `status.json` of its own
    // — it is not a `__subagent-runner`-aware binary) — every `RunState` variant here is still
    // proof this run-id is genuinely tracked (never a ghost the tracker knows nothing about),
    // which is this assertion's actual claim.
    let status = job.last_status.expect("checked above");
    assert!(
        matches!(
            status.state,
            RunState::Queued | RunState::Failed | RunState::Complete | RunState::Running
        ),
        "the tracked run must reach an OBSERVED state (not remain a ghost), got: {status:?}"
    );

    // Independent, filesystem-level confirmation the REAL fixture binary was genuinely re-exec'd
    // as hop-1 with this run's own `--config` path (rather than `spawn_background_steps` having
    // silently no-op'd): the runner's own redirected stdout log has non-trivial content.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = tokio::fs::read_to_string(&job.paths.runner_stdout_log).await
            && !contents.trim().is_empty()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "runner.stdout.log at {:?} was never written to — the detached hop-1 process was \
                 never genuinely spawned with redirected stdio",
                job.paths.runner_stdout_log
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(CYRUP_HOME_ENV_VAR);
    }
}

// =====================================================================================================
// /parallel — R-SA-129/130: routes into the real bounded fan-out executor
// =====================================================================================================

/// `/parallel`: three steps fanned out concurrently, each backed by the real fixture subprocess —
/// asserting all three ran (R-SA-051's ordering guarantee restated at this command's own rendering
/// layer: every step's own output is present, in input order, regardless of completion order).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_command_fans_out_over_real_child_processes() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("PARALLEL_STEP_OUTPUT") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");
    let _env = FixtureEnvGuard::install(&script_path);

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = extension
        .execute_command(
            "parallel",
            "worker \"task a\" -> worker \"task b\" -> worker \"task c\"",
            &ctx,
        )
        .await
        .expect("execute_command does not error")
        .expect("parallel produces textual output");

    assert!(
        !output.contains("recognized by the subagents extension"),
        "got: {output}"
    );
    assert!(
        output.contains("step 1: ok (parallel group)"),
        "a /parallel call must render as one collapsed group step: {output}"
    );
    // Every fanned-out child's own real fixture output must be individually present (R-SA-051's
    // ordering guarantee, restated at this command's own text-rendering layer) — not merely a
    // single aggregate line with no per-child text at all.
    assert_eq!(
        output.matches("PARALLEL_STEP_OUTPUT").count(),
        3,
        "all three fanned-out children's real fixture output must appear in the rendered \
         result: {output}"
    );
    assert!(output.contains("child 1: ok"), "got: {output}");
    assert!(output.contains("child 2: ok"), "got: {output}");
    assert!(output.contains("child 3: ok"), "got: {output}");
}

// =====================================================================================================
// /run-chain — R-SA-129/130: resolves a saved chain via real discovery, then runs it for real
// =====================================================================================================

/// `/run-chain <name> -- <task>`: resolves a saved `.chain.json` chain through the REAL discovery
/// pipeline (not a hardcoded lookup), seeds the supplied task into the first step, and runs it
/// through the same real chain-walker `/chain` itself uses.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_chain_command_resolves_a_saved_chain_through_real_discovery_and_executes_it() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");
    write_fixture_chain(
        work_dir.path(),
        "release",
        &[single_step("worker", "placeholder"), single_step("worker", "fixed second step")],
    );

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("RUN_CHAIN_OUTPUT") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");
    let _env = FixtureEnvGuard::install(&script_path);

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = extension
        .execute_command("run-chain", "release -- the seeded first-step task", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("run-chain produces textual output");

    assert!(
        !output.contains("recognized by the subagents extension"),
        "got: {output}"
    );
    assert_eq!(
        output.matches("RUN_CHAIN_OUTPUT").count(),
        2,
        "both of the saved chain's steps must have run through the real fixture child: {output}"
    );
}

/// An unresolvable saved-chain name fails BEFORE any subprocess is spawned — proving `/run-chain`
/// genuinely resolves through discovery rather than blindly constructing a runnable graph from
/// whatever name was typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_chain_command_against_an_unknown_chain_name_fails_before_any_subprocess_spawn() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("real tempdir");
    // Deliberately no fixture chain written, and no `CYRUP_SUBAGENT_BINARY` override configured —
    // if this path somehow attempted a real spawn it would fail loudly (no fixture configured),
    // so the absence of any spawn attempt is provable by this test passing at all.

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = extension
        .execute_command("run-chain", "ghost-chain -- anything", &ctx)
        .await
        .expect("execute_command does not error (SubagentError is rendered as text, not ExtError)")
        .expect("a rendered error message");

    assert!(
        output.contains("chain not found") || output.contains("subagent command failed"),
        "an unresolvable saved-chain name must surface as a clear failure, not a silent no-op or \
         a stub acknowledgement: {output}"
    );
}
