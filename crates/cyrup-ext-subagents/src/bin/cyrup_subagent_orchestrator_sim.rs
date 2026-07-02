//! `cyrup-subagent-orchestrator-sim` — a tiny standalone helper binary, gated behind the
//! `test-fixtures` Cargo feature, built for A-SA-12's acceptance scenario (func-SA §7: "killing
//! the orchestrator process mid-background-run leaves the detached runner process alive and it
//! completes, writing a valid terminal `status.json` + `ResultFile`", DI-SA-8/R-SA-070/071).
//!
//! # Why a separate binary is required to test this at all
//!
//! Every other integration test in this crate that exercises `spawn_detached_runner` does so from
//! the SAME OS process as the test harness itself (a `#[tokio::test]` async fn calling the
//! function directly). That is sufficient to prove hop-1 detachment (the spawned runner is not
//! `.wait()`-ed and keeps running after the calling function returns), but it CANNOT prove
//! survival across orchestrator death — the "orchestrator" in those tests never actually dies,
//! the test function just stops awaiting it. A-SA-12 specifically requires killing the real OS
//! process that called `spawn_detached_runner` and observing that its child (the detached runner)
//! is unaffected. That requires the "orchestrator" role to be a genuine, separate, killable OS
//! process — hence this binary.
//!
//! # Why this binary ALSO plays the "runner" (hop-2) role, dual-mode
//!
//! `spawn_detached_runner` re-execs whatever `resolve_spawn_command()` resolves to (normally the
//! current `cyrup` binary, overridable via `CYRUP_SUBAGENT_BINARY`) with `__subagent-runner
//! --config <path>` argv, expecting THAT process to understand the subcommand and dispatch into
//! [`cyrup_ext_subagents::background::runner_main::run`] — exactly what `crates/cyrup/src/
//! subagent_runner_cmd.rs` does in production. This crate (`cyrup-ext-subagents`) cannot depend on
//! `crates/cyrup` (that would be circular: `cyrup` already depends on this crate), so there is no
//! way to point `CYRUP_SUBAGENT_BINARY` at a real, full `cyrup` binary from this crate's own test
//! fixtures. Instead, THIS binary points `CYRUP_SUBAGENT_BINARY` at **itself** and recognizes the
//! `__subagent-runner --config <path>` argv shape on its own, replicating
//! `subagent_runner_cmd.rs`'s own minimal parse/dispatch logic verbatim (same flag name, same
//! `RunPaths` derivation-from-config-path convention) so the grandchild process really is the real
//! [`cyrup_ext_subagents::background::runner_main::run`] loop, not another scripted stand-in.
//!
//! # Contract
//!
//! **Orchestrator mode** (default): invoked as
//! ```text
//! cyrup-subagent-orchestrator-sim <runner-config-path> <stdout-log-path> <stderr-log-path>
//! ```
//! Sets `CYRUP_SUBAGENT_BINARY` to its own `current_exe()` path (so the detached child it spawns
//! re-execs itself, landing in runner mode below), calls
//! [`cyrup_ext_subagents::background::spawn_detached::spawn_detached_runner`] exactly once,
//! prints the resulting pid to stdout as a bare decimal integer followed by a newline, flushes,
//! and exits immediately with code 0 — deliberately NEVER awaiting the spawned child, mirroring
//! the real orchestrator's own `spawn_background` contract (R-SA-074: "return immediately after
//! confirmed spawn"). The calling integration test then kills THIS process (simulating
//! orchestrator death) and independently verifies the grandchild survives.
//!
//! On any spawn failure, prints `SPAWN_FAILED: <message>` to stdout and exits with code 1.
//!
//! **Runner mode**: invoked as `<binary> __subagent-runner --config <path>` (recognized by argv[1]
//! == `__subagent-runner`, exactly [`spawn_detached_runner`]'s own hardcoded subcommand literal).
//! Derives [`RunPaths`] from the config path per the fixed on-disk layout convention (config path's
//! parent = run dir = `<run_id>`; run dir's parent = `AsyncRoot`; `AsyncRoot`'s sibling `results/`
//! = `ResultsDir`) and calls [`run`] directly to completion.
//!
//! # The `CYRUP_SUBAGENT_STEP_BINARY` relay (avoiding a `CYRUP_SUBAGENT_BINARY` self-reference loop)
//!
//! Orchestrator mode overwrites `CYRUP_SUBAGENT_BINARY` (inherited by the detached child) to point
//! at THIS binary, so hop-1 lands back here in runner mode. But `run()`'s own per-step child spawn
//! (hop 2's real work: actually running the scripted fixture) ALSO reads `CYRUP_SUBAGENT_BINARY`
//! (env vars are inherited transitively) — if left unchanged, the runner would try to re-exec
//! ITSELF again for the step's own child, which does not understand the plain NDJSON wire protocol
//! a step's child is expected to speak. To avoid that self-reference loop, the calling integration
//! test sets a SEPARATE env var, `CYRUP_SUBAGENT_STEP_BINARY`, carrying the REAL scripted fixture's
//! path; runner mode reads it and reassigns `CYRUP_SUBAGENT_BINARY` to that value before calling
//! [`run`], so the step's own child spawn resolves to the real fixture, not back to this binary.

use std::io::Write;
use std::path::{Path, PathBuf};

use cyrup_ext_subagents::background::RunPaths;
use cyrup_ext_subagents::background::runner_main::run;

const SUBAGENT_RUNNER_SUBCOMMAND: &str = "__subagent-runner";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some(SUBAGENT_RUNNER_SUBCOMMAND) {
        run_runner_mode(&args).await;
    } else {
        run_orchestrator_mode(&args);
    }
}

/// Runner mode: replicates `crates/cyrup/src/subagent_runner_cmd.rs`'s own minimal
/// `--config <path>` parse and [`RunPaths`] derivation, then calls [`run`] directly.
async fn run_runner_mode(args: &[String]) {
    // Relay CYRUP_SUBAGENT_STEP_BINARY (the real scripted-fixture path, set by the calling test)
    // into CYRUP_SUBAGENT_BINARY (what `resolve_spawn_command` inside `run()`'s own per-step
    // child spawn actually reads) — see this file's module doc for why this avoids a
    // self-reference loop. SAFETY: single-threaded at this point (before any spawn/tokio work in
    // this process begins), no concurrent readers of this env var exist yet.
    if let Ok(step_binary) = std::env::var("CYRUP_SUBAGENT_STEP_BINARY") {
        unsafe {
            std::env::set_var("CYRUP_SUBAGENT_BINARY", step_binary);
        }
    }

    let Some(cfg_path) = args.get(2..).and_then(parse_config_flag) else {
        eprintln!("{SUBAGENT_RUNNER_SUBCOMMAND}: missing required --config <path> argument");
        std::process::exit(2);
    };

    let Some(run_paths) = derive_run_paths(&cfg_path) else {
        eprintln!(
            "{SUBAGENT_RUNNER_SUBCOMMAND}: --config path {} is too shallow to derive RunPaths",
            cfg_path.display()
        );
        std::process::exit(2);
    };

    match run(&cfg_path, &run_paths).await {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("{SUBAGENT_RUNNER_SUBCOMMAND}: run() returned an error: {err}");
            std::process::exit(1);
        }
    }
}

fn parse_config_flag(rest: &[String]) -> Option<PathBuf> {
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn derive_run_paths(cfg_path: &Path) -> Option<RunPaths> {
    let run_dir = cfg_path.parent()?;
    let run_id_token = run_dir.file_name()?.to_string_lossy().into_owned();
    let async_root = run_dir.parent()?;
    let results_dir = async_root.parent().unwrap_or(async_root).join("results");
    let run_id = cyrup_ext_subagents::background::RunId::from_token(run_id_token);
    Some(RunPaths::for_run(async_root, &results_dir, &run_id))
}

/// Orchestrator mode: spawns the real hop-1 detached process (pointed at THIS SAME binary via
/// `CYRUP_SUBAGENT_BINARY` so the detached child lands back in runner mode above), prints its pid,
/// and exits immediately without awaiting it.
fn run_orchestrator_mode(args: &[String]) {
    let (Some(cfg_path), Some(stdout_log), Some(stderr_log)) =
        (args.get(1), args.get(2), args.get(3))
    else {
        eprintln!(
            "usage: cyrup-subagent-orchestrator-sim <runner-config-path> <stdout-log-path> \
             <stderr-log-path>"
        );
        std::process::exit(2);
    };

    let cfg_path = PathBuf::from(cfg_path);
    let stdout_log = PathBuf::from(stdout_log);
    let stderr_log = PathBuf::from(stderr_log);

    // Point CYRUP_SUBAGENT_BINARY at THIS binary's own resolved path so the detached child this
    // process spawns re-execs itself and lands in runner mode above — see this file's module doc
    // for why a real `cyrup` binary cannot be used here. SAFETY: single-threaded at this point
    // (before any spawn/tokio work begins), no concurrent readers of this env var exist yet.
    if let Ok(self_path) = std::env::current_exe() {
        unsafe {
            std::env::set_var("CYRUP_SUBAGENT_BINARY", self_path);
        }
    }

    match cyrup_ext_subagents::background::spawn_detached::spawn_detached_runner(
        &cfg_path,
        &stdout_log,
        &stderr_log,
    ) {
        Ok(pid) => {
            println!("{pid}");
            let _ = std::io::stdout().flush();
        }
        Err(err) => {
            println!("SPAWN_FAILED: {err}");
            let _ = std::io::stdout().flush();
            std::process::exit(1);
        }
    }
    // Deliberately exit immediately without awaiting the spawned child — this process's own,
    // real, prompt exit is what the calling test then races against to simulate "the orchestrator
    // died mid-background-run".
}
