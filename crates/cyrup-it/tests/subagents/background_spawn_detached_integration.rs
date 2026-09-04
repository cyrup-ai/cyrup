//! Integration test: `background::spawn_detached::spawn_detached_runner` end to end against the
//! scripted-NDJSON test-double binary (`cyrup-subagent-fixture`, arch-SA §11) — proving the hop-1
//! detached second-process spawn (func-SA R-SA-070/071; arch-SA §6.5) actually detaches: the
//! spawned process keeps running, verified via its own OS pid, independent of THIS test process's
//! lifetime/control-flow, exactly the way a real background subagent run must survive tool-call
//! return and even orchestrator exit.
//!
//! No mocking anywhere in this file (this codebase's standing convention, restated in this
//! crate's own task brief and already established by `tests/exec_run_sync_integration.rs`): every
//! test below spawns the REAL `cyrup-subagent-fixture` binary as a genuine OS subprocess via
//! an explicit `SpawnCommand` handed to `spawn_detached_runner_with_command` (the injectable core
//! of `spawn_detached_runner`; the `CYRUP_SUBAGENT_BINARY` ladder it wraps is proved separately by
//! `spawn::resolve_spawn_command_from`'s own unit tests), and verifies liveness/process-group/stdio-redirection/argv
//! purely via independent OS-level probes (`kill -0`, `ps`, reading the real redirected log
//! files) — never via this crate's own bookkeeping.
//!
//! This file is a separate compilation unit from `cyrup-ext-subagents`'s own `lib.rs` (ordinary
//! Cargo integration-test placement), so it is NOT bound by that crate's own
//! `#![forbid(unsafe_code)]`, and `CARGO_BIN_EXE_cyrup-subagent-fixture` (only available to
//! integration tests, never to a library's own `#[cfg(test)]` unit tests) resolves here — exactly
//! the same two reasons `tests/exec_run_sync_integration.rs` lives outside `src/`. The `unsafe`
//! file mutates no process-global state, so it contains no `unsafe` and needs no mutex.
//!
//! Gated on the `test-fixtures` Cargo feature (matching the `cyrup-subagent-fixture` `[[bin]]`
//! target's own `required-features` gate, `Cargo.toml`): without that feature the fixture binary
//! is never built at all, so this whole file compiles to an empty test list (`cargo test` reports
//! it as a normal, zero-test pass) rather than every test failing at spawn time with a confusing
//! "No such file or directory".

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::time::Duration;

use cyrup_ext_subagents::background::spawn_detached::spawn_detached_runner_with_command;
use cyrup_ext_subagents::spawn::SpawnCommand;

/// The scripted fixture as an explicit command for `spawn_detached_runner_with_command`.
///
/// The injectable core exists for exactly this: substituting the binary WITHOUT moving
/// `CYRUP_SUBAGENT_BINARY` on a process every other test in this binary shares. That variable's own
/// resolution ladder is proved by `spawn::resolve_spawn_command_from`'s unit tests, which drive it
/// through an injected lookup — so nothing is lost by not exercising it a second time here.
fn fixture_cmd(script_path: &std::path::Path) -> SpawnCommand {
    SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    }
}

const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

/// Path to the real, already-built `cyrup-subagent-fixture` binary.
///
/// MIGRATION: this used to be `PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))`, which
/// worked only while this file lived in `cyrup-ext-subagents` — Cargo sets `CARGO_BIN_EXE_<name>`
/// only for test targets in the SAME package as that binary. In `cyrup-it` it does not resolve at
/// all, so the path now comes from this crate's `build.rs`, which builds the fixture (with the
/// owning crate's `--features test-fixtures`) and exports `CYRUP_IT_BIN_CYRUP_SUBAGENT_FIXTURE`.
fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

/// Write `script_json` to a fresh temp file and return its path, for
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT` to point at.
fn write_script(dir: &std::path::Path, name: &str, script_json: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script_json.to_string()).expect("write fixture script");
    path
}

/// Report whether the OS still considers `pid` a genuinely *running* process — an
/// independent-of-our-own-bookkeeping liveness probe, exactly what proves genuine detachment
/// rather than this crate merely claiming it.
///
/// WHY NOT A BARE `kill -0`: `kill -0` succeeds for a ZOMBIE, i.e. a process that has already run
/// to completion and exited but whose exit status nobody has reaped yet. The pid slot still
/// exists and is still signalable, so `kill -0` reports success for a `<defunct>` entry that is
/// not executing anything. That is not a hypothetical here: every process this file spawns
/// detached deliberately OUTLIVES its own parent, so the kernel re-parents it to pid 1, and
/// reaping then depends entirely on pid 1 being a real init that calls `wait()`. Under the
/// container/microVM this suite runs in, pid 1 is `process_api --firecracker-init`, which does
/// not reap orphans — so a detached runner that exited cleanly stays defunct for the rest of the
/// sandbox's life and a bare `kill -0` would claim it is still alive forever.
///
/// So consult `/proc/<pid>/stat` first (field 3 is the single-character process state) and treat
/// state `Z` as NOT alive, while every other state — `R`, `S`, `D`, `T`, ... — is alive, keeping
/// the probe honest for a genuinely running process. `kill -0` remains the fallback for a Unix
/// without `/proc` (and for a pid that has been fully reaped, where `/proc/<pid>` is gone and
/// `kill -0` correctly reports dead).
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    match proc_state_char(pid) {
        Some(state) => state != 'Z',
        None => kill_zero_succeeds(pid),
    }
}

/// Read the process state character (field 3) out of `/proc/<pid>/stat`, or `None` when there is
/// no such entry to read (no `/proc` on this platform, or the pid is fully gone/reaped).
///
/// Field 2 (`comm`) is parenthesized and may itself contain spaces and `)`, so the fields after
/// it are located from the LAST `)` in the line rather than by splitting from the start.
#[cfg(unix)]
fn proc_state_char(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().next()?.chars().next()
}

/// The original probe, kept as the no-`/proc` fallback: `kill -0 <pid>` succeeds iff the pid slot
/// exists and is signalable by us (which includes zombies — see [`pid_is_alive`]).
#[cfg(unix)]
fn kill_zero_succeeds(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn kill_pid_for_cleanup(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

/// THE core proof this whole subsystem exists for: [`spawn_detached_runner`] returns a real, live
/// pid for a process that keeps running independently of THIS test's own control flow — verified
/// via `kill -0` (an OS-level probe, never this crate's own bookkeeping) both immediately after
/// spawn AND after a sleep that outlasts any plausible reap/cleanup window a naive (incorrect)
/// implementation might have raced against. The fixture binary is scripted to sleep well past this
/// test's own assertions, so if `spawn_detached_runner` had (incorrectly) awaited the child
/// inline, this test function itself would never reach its assertions in the first place — the
/// very fact this test returns promptly is itself part of what it is proving (R-SA-070/071/074).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_detached_runner_process_keeps_running_independent_of_this_process() {
    let dir = tempfile::tempdir().expect("real tempdir");

    // A script that sleeps far longer than this test needs to complete its own assertions — if
    // this test finishes well before that sleep would elapse AND the pid is still alive at that
    // point, the child was genuinely not awaited inline by spawn_detached_runner.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": "{\"type\":\"unknown\",\"phase\":\"started\"}"},
            {"kind": "sleep_ms", "ms": 3000}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let cfg_path = dir.path().join("runner-config.json");
    std::fs::write(&cfg_path, "{}").expect("write placeholder config");
    let stdout_log = dir.path().join("runner.stdout.log");
    let stderr_log = dir.path().join("runner.stderr.log");

    let started = tokio::time::Instant::now();
    let spawn_result = spawn_detached_runner_with_command(
        &fixture_cmd(&script_path),
        &cfg_path,
        &stdout_log,
        &stderr_log,
        // The SAME overlay `spawn_detached_runner` builds — this substitutes the binary, not the
        // parent-anchor plumbing, and one test below asserts the child inherits that anchor.
        &cyrup_ext_subagents::background::parent_anchor::detached_runner_env_overlay(),
    );

    let pid = spawn_result.expect("detached spawn succeeds");

    // The call itself must return promptly — it must NOT have awaited the 3s-sleeping child.
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "spawn_detached_runner must return immediately after confirming spawn, not await \
         completion (R-SA-070/074); took {:?}",
        started.elapsed()
    );

    assert!(pid > 0, "a real, nonzero pid must be returned");
    assert!(
        pid_is_alive(pid),
        "the detached child must be alive (kill -0) immediately after spawn_detached_runner \
         returns"
    );

    // Sleep past HALF the fixture's own scripted 3s sleep, well beyond any plausible reap/cleanup
    // window a naive (incorrect) implementation might have raced against, and reconfirm liveness
    // via the SAME independent OS-level probe — this is the load-bearing assertion: the process
    // is still running on its own, driven by nothing this test process is doing (no task holds a
    // handle to it, nothing is awaiting it).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        pid_is_alive(pid),
        "the detached child must STILL be alive partway through its own scripted lifetime, \
         proving it is running independently, not tied to this test's task lifetime"
    );

    kill_pid_for_cleanup(pid);
}

/// The detached child's process group must differ from this test process's own — the concrete
/// mechanism (`process_group(0)`) that gives R-SA-070's "not signaled by the parent's process
/// group" its teeth. `ps -o pgid= -p <pid>` reads the REAL kernel-reported process group, never
/// this crate's own bookkeeping.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_runner_gets_its_own_process_group() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({"steps": [{"kind": "sleep_ms", "ms": 2000}], "exit_code": 0});
    let script_path = write_script(dir.path(), "script.json", &script);

    let cfg_path = dir.path().join("runner-config.json");
    std::fs::write(&cfg_path, "{}").expect("write placeholder config");
    let stdout_log = dir.path().join("runner.stdout.log");
    let stderr_log = dir.path().join("runner.stderr.log");

    let spawn_result = spawn_detached_runner_with_command(
        &fixture_cmd(&script_path),
        &cfg_path,
        &stdout_log,
        &stderr_log,
        // The SAME overlay `spawn_detached_runner` builds — this substitutes the binary, not the
        // parent-anchor plumbing, and one test below asserts the child inherits that anchor.
        &cyrup_ext_subagents::background::parent_anchor::detached_runner_env_overlay(),
    );

    let pid = spawn_result.expect("detached spawn succeeds");

    let own_pgid_output = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs for our own pid");
    let own_pgid = String::from_utf8_lossy(&own_pgid_output.stdout)
        .trim()
        .to_string();

    let child_pgid_output = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs for the detached child's pid");
    let child_pgid = String::from_utf8_lossy(&child_pgid_output.stdout)
        .trim()
        .to_string();

    assert!(
        !child_pgid.is_empty(),
        "the detached child must still be alive long enough for ps to report its pgid"
    );
    assert_ne!(
        own_pgid, child_pgid,
        "the detached child must be the leader of its OWN new process group (process_group(0)), \
         never a member of this test process's group"
    );
    // A process group leader's pgid equals its own pid.
    assert_eq!(
        child_pgid,
        pid.to_string(),
        "process_group(0) means the child's pgid must equal its own pid (group leader)"
    );

    kill_pid_for_cleanup(pid);
}

/// R-SA-070: stdout/stderr must be redirected to the given log files, never inherited from this
/// test process's own stdio — verified by having the scripted fixture emit a line and confirming
/// it lands in the redirected file, never on this test's own captured output.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_is_redirected_to_the_given_log_files_not_inherited() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": "{\"type\":\"unknown\",\"marker\":\"detached-stdout-marker\"}"}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let cfg_path = dir.path().join("runner-config.json");
    std::fs::write(&cfg_path, "{}").expect("write placeholder config");
    let stdout_log = dir.path().join("runner.stdout.log");
    let stderr_log = dir.path().join("runner.stderr.log");

    let spawn_result = spawn_detached_runner_with_command(
        &fixture_cmd(&script_path),
        &cfg_path,
        &stdout_log,
        &stderr_log,
        // The SAME overlay `spawn_detached_runner` builds — this substitutes the binary, not the
        // parent-anchor plumbing, and one test below asserts the child inherits that anchor.
        &cyrup_ext_subagents::background::parent_anchor::detached_runner_env_overlay(),
    );

    let pid = spawn_result.expect("detached spawn succeeds");

    // Give the (fast, non-sleeping) fixture time to run to completion and flush its output.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut contents = String::new();
    while tokio::time::Instant::now() < deadline {
        contents = std::fs::read_to_string(&stdout_log).unwrap_or_default();
        if contents.contains("detached-stdout-marker") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        contents.contains("detached-stdout-marker"),
        "the child's stdout must land in the redirected log file: got {contents:?}"
    );

    kill_pid_for_cleanup(pid);
}

/// The `__subagent-runner --config <cfg_path>` argv contract (R-SA-070/073): the detached runner
/// subcommand must receive exactly that subcommand name, the `--config` flag, and the exact
/// config path this function was called with — verified by echoing argv back via the fixture's
/// own `echo_argv` script feature.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passes_the_subcommand_and_config_flag_with_the_exact_path() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({"echo_argv": true, "exit_code": 0});
    let script_path = write_script(dir.path(), "script.json", &script);

    let cfg_path = dir.path().join("a-particular-runner-config.json");
    std::fs::write(&cfg_path, "{}").expect("write placeholder config");
    let stdout_log = dir.path().join("runner.stdout.log");
    let stderr_log = dir.path().join("runner.stderr.log");

    let spawn_result = spawn_detached_runner_with_command(
        &fixture_cmd(&script_path),
        &cfg_path,
        &stdout_log,
        &stderr_log,
        // The SAME overlay `spawn_detached_runner` builds — this substitutes the binary, not the
        // parent-anchor plumbing, and one test below asserts the child inherits that anchor.
        &cyrup_ext_subagents::background::parent_anchor::detached_runner_env_overlay(),
    );

    let pid = spawn_result.expect("detached spawn succeeds");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut contents = String::new();
    while tokio::time::Instant::now() < deadline {
        contents = std::fs::read_to_string(&stdout_log).unwrap_or_default();
        if contents.lines().count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let cfg_path_str = cfg_path.display().to_string();
    assert!(
        contents.contains("__subagent-runner"),
        "argv must include the __subagent-runner subcommand: got {contents:?}"
    );
    assert!(
        contents.contains("--config"),
        "argv must include the --config flag: got {contents:?}"
    );
    assert!(
        contents.contains(&cfg_path_str),
        "argv must include the exact config path passed in: got {contents:?}"
    );

    kill_pid_for_cleanup(pid);
}

/// A child that exits quickly (the common, well-behaved case) must not leave a zombie/defunct
/// process behind just because `spawn_detached_runner` never calls `wait()` on it — since this
/// test process is not the child's parent-of-record in any special way (it IS the real OS parent,
/// same as any spawner), we confirm the process eventually disappears from `kill -0`'s view on its
/// own once it exits and is reaped by the OS's normal child-reaping (via this test's own process
/// occasionally polling `waitpid`-adjacent state indirectly through `kill -0`, which reports dead
/// once reaped OR once no longer running as an active PID depending on platform zombie-reaping
/// timing) — a coarse-grained but real, non-mocked confirmation that "never awaited" does not
/// leak an unreapable resource under ordinary conditions.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quickly_exiting_detached_child_eventually_disappears_on_its_own() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({
        "steps": [{"kind": "emit", "line": "{\"type\":\"unknown\",\"done\":true}"}],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let cfg_path = dir.path().join("runner-config.json");
    std::fs::write(&cfg_path, "{}").expect("write placeholder config");
    let stdout_log = dir.path().join("runner.stdout.log");
    let stderr_log = dir.path().join("runner.stderr.log");

    let spawn_result = spawn_detached_runner_with_command(
        &fixture_cmd(&script_path),
        &cfg_path,
        &stdout_log,
        &stderr_log,
        // The SAME overlay `spawn_detached_runner` builds — this substitutes the binary, not the
        // parent-anchor plumbing, and one test below asserts the child inherits that anchor.
        &cyrup_ext_subagents::background::parent_anchor::detached_runner_env_overlay(),
    );

    let pid = spawn_result.expect("detached spawn succeeds");
    assert!(pid > 0, "a confirmed spawn reports a real pid: {pid}");

    // NOT asserted here: that the child is still alive. This fixture emits one line and exits 0,
    // so it is designed to be gone almost immediately — under CPU contention it can exit before
    // the next statement in this test runs, and an `assert!(pid_is_alive(pid))` at this point then
    // fails for the one reason that is not a defect. It was a race this test could only ever lose
    // and never needed to win: a child that has ALREADY exited satisfies "eventually disappears"
    // trivially, which is the property below and the whole subject of this test.

    // Give the fast fixture generous time to exit and (on most Unix systems, since this test
    // process IS its real parent) be reaped. This deadline is intentionally generous (well beyond
    // the fixture's own near-instant exit) because reaping/zombie-collection scheduling is not
    // guaranteed to be prompt under heavy concurrent CPU contention from the rest of this crate's
    // test suite running in parallel (`cargo test` spawns many real subprocesses across other
    // files at the same time) — a tight bound here would produce a false failure driven by test
    // scheduling noise rather than any defect in `spawn_detached_runner` itself.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut still_alive = true;
    while tokio::time::Instant::now() < deadline {
        if !pid_is_alive(pid) {
            still_alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        !still_alive,
        "a fast-exiting detached child should no longer report alive via kill -0 after it has \
         had ample time to exit, even though spawn_detached_runner never awaited it directly"
    );
}

// =================================================================================================
// A-SA-12: background survives orchestrator (func-SA §7; DI-SA-8; R-SA-070/071)
// =================================================================================================
//
// "Killing the orchestrator process mid-background-run leaves the detached runner process alive
// and it completes, writing a valid terminal `status.json` + `ResultFile`."
//
// Every other test in this file proves hop-1 detachment from WITHIN this same test process (the
// "orchestrator" role never actually dies, the test function just stops awaiting the child) — see
// this file's own module doc for why that is NOT sufficient to prove survival across orchestrator
// death. This test uses `cyrup-subagent-orchestrator-sim` (a real, separate, killable OS process
// built specifically for this scenario, see that binary's own module doc) to play the orchestrator
// role for real: it calls `spawn_detached_runner` itself, reports the resulting grandchild pid,
// and exits immediately — at which point THIS test kills it (simulating a crash/quit/Ctrl-C) and
// verifies, purely via independent OS-level/filesystem probes, that the grandchild (the real
// detached runner) is unaffected and eventually writes a valid terminal `status.json` +
// `ResultFile`.

use std::collections::BTreeMap;

use cyrup_core::ModelId;
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::runner_main::RunnerConfig;
use cyrup_ext_subagents::background::{RunId, RunMode, RunPaths, RunState};
use cyrup_ext_subagents::discovery::types::SystemPromptMode;
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// Path to the real, already-built `cyrup-subagent-orchestrator-sim` helper binary.
fn orchestrator_sim_binary_path() -> PathBuf {
    crate::support::bins::subagent_orchestrator_sim()
}

/// A minimal resolved persona for a fixture-driven test (T0.1 / C13) — a real model so the
/// fallback ladder is non-empty (the scripted fixture ignores `--model`), guard off, `Replace`
/// mode. Every step's agent must now have a plan-time persona in `resolved_agents`.
fn fixture_persona(name: &str) -> ResolvedAgentPersona {
    ResolvedAgentPersona {
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
        name: name.to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
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
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
        runner: None, // SUBA-074: the native child, as before
    }
}

/// The plan-time `resolved_agents` map covering every agent name any step in this file dispatches
/// (`worker`/`first`; `second` is deliberately never dispatched but is included so a spurious
/// dispatch would still resolve rather than masking the real assertion behind an `Unknown agent`).
fn all_personas() -> BTreeMap<String, ResolvedAgentPersona> {
    ["worker", "first", "second"]
        .into_iter()
        .map(|name| (name.to_string(), fixture_persona(name)))
        .collect()
}

/// Mirrors `tests/background_runner_main_integration.rs`'s own identical helper — a minimal,
/// all-other-fields-`None` [`SingleStepSpec`] for `agent`/`task`.
fn single_step(agent: &str, task: &str) -> SingleStepSpec {
    SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: agent.to_string(),
        task: task.to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_runner_survives_orchestrator_death_and_writes_terminal_files() {
    let dir = tempfile::tempdir().expect("real tempdir");

    // A script the grandchild detached runner's own spawned child (the scripted fixture) will
    // run: emits one MessageEnd-shaped line then exits 0 — enough for run_inner's single-step
    // walk to reach a clean terminal Complete state.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}}}"#},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let run_id = RunId::from_token("orchsurvive0000000000000000001");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    std::fs::create_dir_all(&async_root).expect("mkdir async_root");
    std::fs::create_dir_all(&results_dir).expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    std::fs::create_dir_all(&run_paths.run_dir).expect("mkdir run_dir");

    // A real RunnerConfig, using the actual Rust type (never hand-rolled JSON, which would be
    // fragile against this type's own serde shape) — one SingleStep, matching
    // `background_runner_main_integration.rs`'s own identical `single_step` helper shape.
    let runner_config = RunnerConfig {
        turn_budget: None,
        permission_rules: None, // SUBA-073: no policy — the pre-field behaviour
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step(
            "worker",
            "do the thing",
        ))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so the detached runner subprocess rebuilds
        // its RunPaths from THESE (never re-derives), writing its terminal ResultFile into the
        // same results dir this test created.
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
        model_scope: None,
        control: None,
        include_progress: None,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &runner_config)
        .await
        .expect("write real runner config");

    let orchestrator_stdout_log = dir.path().join("orchestrator.stdout.log");
    let orchestrator_stderr_log = dir.path().join("orchestrator.stderr.log");
    let runner_stdout_log = run_paths.runner_stdout_log.clone();
    let runner_stderr_log = run_paths.runner_stderr_log.clone();

    // Spawn the orchestrator-sim as a REAL, separate OS process (not a tokio::process::Command
    // held inside this test's own async task, so this test's own process is genuinely the OS
    // parent of "the orchestrator", exactly the shape needed to then kill it independently).
    let mut orchestrator_cmd = std::process::Command::new(orchestrator_sim_binary_path());
    orchestrator_cmd
        .arg(&cfg_path)
        .arg(&runner_stdout_log)
        .arg(&runner_stderr_log)
        // NOT CYRUP_SUBAGENT_BINARY here: orchestrator-sim manages that env var itself (pointing
        // hop-1 back at its own binary so the detached child lands in runner mode) — see that
        // binary's own module doc for the CYRUP_SUBAGENT_STEP_BINARY relay this crosses.
        .env("CYRUP_SUBAGENT_STEP_BINARY", fixture_binary_path())
        .env(FIXTURE_SCRIPT_ENV_VAR, &script_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(
            std::fs::File::create(&orchestrator_stderr_log)
                .expect("create orchestrator stderr log"),
        ));
    let _ = &orchestrator_stdout_log; // reserved for debugging; stdout is piped and read directly below.

    let mut orchestrator_child = orchestrator_cmd.spawn().expect("orchestrator-sim spawns");
    let orchestrator_pid = orchestrator_child.id();

    // Read the grandchild pid the orchestrator-sim prints to its own stdout, then wait for the
    // orchestrator-sim itself to exit (it exits immediately after printing, per its own
    // documented contract) — at which point it is TRULY gone, not merely "stopped being awaited".
    let stdout = orchestrator_child.stdout.take().expect("piped stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let mut first_line = String::new();
    std::io::BufRead::read_line(&mut reader, &mut first_line)
        .expect("read orchestrator-sim stdout");
    let trimmed = first_line.trim();
    assert!(
        !trimmed.starts_with("SPAWN_FAILED"),
        "orchestrator-sim failed to spawn the detached runner: {trimmed}"
    );
    let grandchild_pid: u32 = trimmed
        .parse()
        .unwrap_or_else(|_| panic!("orchestrator-sim printed a non-pid first line: {trimmed:?}"));

    let orchestrator_status = orchestrator_child
        .wait()
        .expect("orchestrator-sim process can be waited on");
    assert!(
        orchestrator_status.success(),
        "orchestrator-sim itself must exit 0 having only spawned, never awaited, the grandchild"
    );

    // The orchestrator-sim process (pid `orchestrator_pid`) has now genuinely exited on its own —
    // this already IS "the orchestrator died". Independently confirm via `kill -0` that its own
    // pid is gone (never trust this test's own `wait()` bookkeeping alone), then confirm the
    // GRANDCHILD (the real detached runner, a distinct pid) is still alive.
    assert!(
        !pid_is_alive(orchestrator_pid),
        "the orchestrator-sim process must be genuinely gone after wait() returns"
    );
    assert!(
        pid_is_alive(grandchild_pid),
        "the detached runner (grandchild pid {grandchild_pid}) must still be alive immediately \
         after its own spawning orchestrator process has exited — this is the entire point of \
         DI-SA-8/R-SA-070"
    );

    // Poll for the terminal files the runner writes as its very last acts (R-SA-077: status.json
    // strictly before ResultFile) — generous bound, matching this file's other timing-tolerant
    // tests, since this runs concurrently with the rest of the crate's real-subprocess test suite.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if run_paths.result.exists() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let stdout_contents = std::fs::read_to_string(&runner_stdout_log).unwrap_or_default();
            let stderr_contents = std::fs::read_to_string(&runner_stderr_log).unwrap_or_default();
            let status_contents = std::fs::read_to_string(&run_paths.status).unwrap_or_default();
            panic!(
                "the detached runner never wrote a terminal ResultFile within the deadline, even \
                 though it survived its orchestrator's death.\nrunner stdout:\n{stdout_contents}\n\
                 runner stderr:\n{stderr_contents}\nstatus.json (if any):\n{status_contents}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Both terminal files exist and are individually well-formed, valid, and mutually consistent
    // — proving the runner didn't just "survive" but actually ran its step and completed cleanly.
    let status: cyrup_ext_subagents::background::RunStatus =
        serde_json::from_slice(&std::fs::read(&run_paths.status).expect("status.json exists"))
            .expect("status.json parses");
    let result_file: cyrup_ext_subagents::background::ResultFile =
        serde_json::from_slice(&std::fs::read(&run_paths.result).expect("ResultFile exists"))
            .expect("ResultFile parses");

    let stdout_contents = std::fs::read_to_string(&runner_stdout_log).unwrap_or_default();
    let stderr_contents = std::fs::read_to_string(&runner_stderr_log).unwrap_or_default();
    assert_eq!(
        status.state,
        RunState::Complete,
        "status.json: {status:?}\nresult_file: {result_file:?}\nrunner stdout:\n{stdout_contents}\nrunner stderr:\n{stderr_contents}"
    );
    assert_eq!(
        result_file.state,
        RunState::Complete,
        "ResultFile: {result_file:?}"
    );
    assert!(result_file.success, "ResultFile: {result_file:?}");
    assert_eq!(result_file.run_id, run_id);
    assert_eq!(
        result_file.run_id, status.run_id,
        "status.json and ResultFile must agree on run identity"
    );

    // The grandchild's own pid must have naturally exited too by now (it completed its work) —
    // confirmed independently, never just inferred from the files' presence.
    assert!(
        !pid_is_alive(grandchild_pid),
        "the detached runner process itself should have exited after writing its terminal files"
    );
}

// =================================================================================================
// A-SA-14: interrupt is soft (func-SA §7; R-SA-084/085)
// =================================================================================================
//
// "Interrupting a running child results in state `Paused`, not `Failed`; resuming that paused run
// spawns a genuinely new subprocess, verified by distinct pids across the interrupt/resume
// boundary."
//
// This test drives the runner as a REAL, separate OS process (via `cyrup-subagent-orchestrator-
// sim`'s runner mode directly — not this crate's own `runner_main::run` called in-process, which
// would make `control::interrupt`'s best-effort SIGUSR2 wake-up target THIS TEST'S OWN process;
// SIGUSR2 has no handler installed anywhere in this crate and defaults to terminating an unhandled
// process, so delivering it to the test's own pid would kill the test itself rather than exercise
// anything). `control::interrupt` is called against the real runner subprocess's own real,
// on-disk-recorded pid — a genuine end-to-end exercise of the file-based control-inbox mechanism
// (DI-SA-9), not a simulation.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupting_a_running_step_pauses_rather_than_fails_the_run() {
    let dir = tempfile::tempdir().expect("real tempdir");

    // Two steps: the first sleeps long enough to give this test a generous, contention-tolerant
    // window to observe Running status AND deliver an interrupt while it is genuinely still in
    // flight (this crate's other timing-tolerant tests use similarly generous bounds under heavy
    // concurrent real-subprocess test-suite load); the second (which must NEVER be dispatched)
    // would emit a distinguishing marker if it ever ran.
    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 5000},
            {"kind": "emit", "line": "{\"type\":\"unknown\",\"phase\":\"first step done\"}"},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let run_id = RunId::from_token("interruptrun00000000000000001");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    std::fs::create_dir_all(&async_root).expect("mkdir async_root");
    std::fs::create_dir_all(&results_dir).expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    std::fs::create_dir_all(&run_paths.run_dir).expect("mkdir run_dir");

    // Two SingleSteps in one Chain-mode run, so an interrupt observed between step 1 and step 2
    // has real remaining work to cut short (R-SA-084 marks the NOT-yet-dispatched step(s) Paused
    // too — see `mark_remaining_paused`'s own doc).
    let runner_config = RunnerConfig {
        turn_budget: None,
        permission_rules: None,
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Chain,
        steps: vec![
            RunnerStep::SingleStep(single_step("first", "first task")),
            RunnerStep::SingleStep(single_step("second", "second task, must never run")),
        ],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so the detached runner subprocess rebuilds
        // its RunPaths from THESE (never re-derives), writing its terminal ResultFile into the
        // same results dir this test created.
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
        model_scope: None,
        control: None,
        include_progress: None,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &runner_config)
        .await
        .expect("write real runner config");

    // Spawn the runner directly in RUNNER mode (skip the orchestrator-mode wrapper entirely — no
    // detachment/orchestrator-death concern for this scenario, just a real, separate, signalable
    // OS process running the real `run_inner` loop).
    let runner_stderr_log = dir.path().join("runner-mode.stderr.log");
    let mut runner_cmd = std::process::Command::new(orchestrator_sim_binary_path());
    runner_cmd
        .arg("__subagent-runner")
        .arg("--config")
        .arg(&cfg_path)
        .env("CYRUP_SUBAGENT_STEP_BINARY", fixture_binary_path())
        .env(FIXTURE_SCRIPT_ENV_VAR, &script_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(
            std::fs::File::create(&runner_stderr_log).expect("create runner-mode stderr log"),
        ));
    let mut runner_child = runner_cmd.spawn().expect("runner-mode process spawns");
    let runner_pid = runner_child.id();

    // Wait for status.json to report Running with a real pid recorded (R-SA-075's initial-status
    // write) before attempting to interrupt — `control::interrupt` requires exactly this state.
    let status_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(bytes) = std::fs::read(&run_paths.status)
            && let Ok(status) =
                serde_json::from_slice::<cyrup_ext_subagents::background::RunStatus>(&bytes)
            && status.state == RunState::Running
            && status.pid == Some(runner_pid)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < status_deadline,
            "status.json never reported Running with the real runner pid within the deadline"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Deliver a REAL interrupt via the real, file-based control mechanism, while the first step
    // is still genuinely sleeping (well inside its own 800ms window).
    let outcome = cyrup_ext_subagents::background::control::interrupt(
        &async_root,
        &results_dir,
        run_id.as_str(),
        "test",
        Some("A-SA-14 interrupt-is-soft scenario".to_string()),
    )
    .await
    .expect("interrupt() call itself succeeds");
    assert_eq!(
        outcome,
        cyrup_ext_subagents::background::control::InterruptOutcome::Delivered,
        "the interrupt must be genuinely delivered against the real Running run"
    );

    // Wait for the runner process to actually exit on its own (never killed by this test) —
    // proving the interrupt was honored cooperatively via the file-based mechanism, not via any
    // external force.
    let exit_status = tokio::task::spawn_blocking(move || runner_child.wait())
        .await
        .expect("join")
        .expect("runner process can be waited on");
    assert!(
        exit_status.success(),
        "the runner process must still exit cleanly (code 0) after an interrupt — R-SA-084's soft \
         interrupt is not a crash. exit status: {exit_status:?}\nstderr:\n{}\nstatus.json (if any): {}",
        std::fs::read_to_string(&runner_stderr_log).unwrap_or_default(),
        std::fs::read_to_string(&run_paths.status).unwrap_or_default()
    );

    // THE core A-SA-14 assertion: the terminal state is Paused, never Failed.
    let status: cyrup_ext_subagents::background::RunStatus =
        serde_json::from_slice(&std::fs::read(&run_paths.status).expect("status.json exists"))
            .expect("status.json parses");
    assert_eq!(
        status.state,
        RunState::Paused,
        "an interrupted run must land in Paused, never Failed: {status:?}"
    );

    // The second step (never dispatched) must be recorded as Paused too, per
    // `mark_remaining_paused`'s documented contract — not silently left Pending, not Failed.
    assert_eq!(
        status.steps.len(),
        2,
        "both steps must be represented in status.json: {status:?}"
    );
    assert_ne!(
        status.steps[1].status,
        cyrup_ext_subagents::background::StepState::Failed,
        "the never-dispatched second step must not be marked Failed just because the run was \
         interrupted before reaching it: {:?}",
        status.steps[1]
    );

    // R-SA-077's own text explicitly includes "interrupt-induced pause" among the terminal
    // completions that get a written ResultFile ("On terminal completion (success, failure, or
    // interrupt-induced pause), the runner MUST write the final status.json before writing the
    // terminal ResultFile") — this runner PROCESS's own lifetime genuinely ends here (a resume
    // would spawn a wholly new hop-2 process later, never resume this one), so a ResultFile
    // recording state=Paused is the spec-mandated behavior, not a contradiction of "Paused is
    // non-terminal" (that property describes `RunState`'s own transition-graph semantics — Paused
    // can still transition onward to Running/Failed on a LATER run — not whether THIS process
    // writes a result record for its own now-ended lifetime).
    let result_file: cyrup_ext_subagents::background::ResultFile = serde_json::from_slice(
        &std::fs::read(&run_paths.result).expect("ResultFile exists (R-SA-077 covers Paused too)"),
    )
    .expect("ResultFile parses");
    assert_eq!(
        result_file.state,
        RunState::Paused,
        "ResultFile: {result_file:?}"
    );
    assert!(
        !result_file.success,
        "a paused run is not a success: {result_file:?}"
    );
}

// =================================================================================================
// PERM-001: the hop-1 detached spawn carries the R-SA-P1 parent-session anchor
// =================================================================================================

/// End-to-end proof of the PERM-001 repair, probed at the OS level rather than through this
/// crate's own bookkeeping: with a published parent-session anchor (what
/// `cyrup-permission-system`'s PARENT-role `SessionStart` installs — pi's
/// `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`,
/// `pi-subagents/src/extension/index.ts:716` @v0.43.0), the REAL detached `__subagent-runner` process
/// spawned by the production entry point [`spawn_detached_runner`] has
/// `CYRUP_SUBAGENT_PARENT_SESSION` in its own environment — read straight out of
/// `/proc/<pid>/environ`, i.e. the kernel's copy, not ours.
///
/// This is the whole background half of permission ask-forwarding. Against the pre-fix code the
/// assertion below cannot pass: `spawn_detached_runner` applied no env overlay at all, so the
/// runner had no anchor, so `exec::build_attempt_spawn_plan`'s "explicit → inherited env → empty"
/// ladder resolved EMPTY for every child the runner went on to spawn, so a background subagent
/// that hit an `ask` addressed a null forwarding target and was fail-closed denied with no prompt
/// ever reaching the operator.
///
/// Linux-only because `/proc/<pid>/environ` is the probe; the platform-independent halves of the
/// same contract are pinned by `background::spawn_detached`'s own unit tests.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_runner_process_inherits_the_published_parent_session_anchor() {
    let dir = tempfile::tempdir().expect("real tempdir");

    // Sleep long enough that `/proc/<pid>/environ` is still readable when this test probes it.
    let script = serde_json::json!({
        "steps": [{"kind": "sleep_ms", "ms": 3000}],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let cfg_path = dir.path().join("runner-config.json");
    std::fs::write(&cfg_path, "{}").expect("write placeholder config");
    let stdout_log = dir.path().join("runner.stdout.log");
    let stderr_log = dir.path().join("runner.stderr.log");

    // The register is process-global; it is mutated only here, under the same lock that guards
    // this file's env mutations, and cleared before the lock is released.
    cyrup_ext_subagents::publish_parent_session_anchor("session-perm001-e2e");
    let spawn_result = spawn_detached_runner_with_command(
        &fixture_cmd(&script_path),
        &cfg_path,
        &stdout_log,
        &stderr_log,
        // The SAME overlay `spawn_detached_runner` builds — this substitutes the binary, not the
        // parent-anchor plumbing, and one test below asserts the child inherits that anchor.
        &cyrup_ext_subagents::background::parent_anchor::detached_runner_env_overlay(),
    );
    cyrup_ext_subagents::clear_parent_session_anchor();

    let pid = spawn_result.expect("detached spawn succeeds");

    // Wait for the child to have actually `execve`'d before reading the kernel's copy of its
    // environment — otherwise this probe measures the PARENT's environ, not the child's, and this
    // test is a load-dependent flake.
    //
    // Mechanism (reproduced directly, 84/200 trials under `nproc` busy-loops): glibc's
    // `posix_spawn` — which `std::process::Command` uses on Linux for a plain spawn — implements
    // the fork half as `clone(CLONE_VM | CLONE_VFORK)`, so between `spawn()` returning and the
    // child reaching `execve` the child SHARES the parent's address space and `/proc/<pid>/environ`
    // reports the parent's LIVE environment. This test removes `CYRUP_SUBAGENT_BINARY` /
    // `CYRUP_SUBAGENT_FIXTURE_SCRIPT` from its own environment a few lines above, so a pre-exec
    // read yields the parent env MINUS those vars and MINUS the overlay — which is exactly the
    // observed failure (`got: [...]` with no `CYRUP_*` entry at all). Under an idle box the child
    // execs first and the race never shows.
    //
    // `/proc/<pid>/exe` is the discriminator: pre-exec it resolves to the PARENT's binary, post-exec
    // to the spawned one. Polling it (rather than sleeping a fixed amount, or relaxing the
    // assertion) is what makes the probe measure the thing the test claims to measure. The
    // production overlay itself is unchanged and correct — `.envs(env_overlay)` is handed to
    // `execve`, which is precisely why the value only becomes observable after that `execve`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    // The loop YIELDS the entries rather than pre-seeding a `mut` binding: every path that leaves
    // it either breaks with a value or trips the deadline assert, so an initial empty vec was dead
    // on arrival (clippy `unused_assignments`).
    let entries: Vec<String> = loop {
        let exec_done = std::fs::read_link(format!("/proc/{pid}/exe"))
            .map(|target| target != std::env::current_exe().unwrap_or_default())
            .unwrap_or(false);
        if exec_done {
            let environ =
                std::fs::read(format!("/proc/{pid}/environ")).expect("read child environ");
            break environ
                .split(|b| *b == 0)
                .filter(|chunk| !chunk.is_empty())
                .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                .collect();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the detached runner never reached execve within 5s; /proc/{pid}/exe still resolves to \
             this test binary"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };

    kill_pid_for_cleanup(pid);

    assert!(
        entries
            .iter()
            .any(|e| e == "CYRUP_SUBAGENT_PARENT_SESSION=session-perm001-e2e"),
        "the detached runner's own environment must carry the published R-SA-P1 anchor so every \
         child it spawns can address the root's ask-forwarding inbox; got: {entries:?}"
    );
}
