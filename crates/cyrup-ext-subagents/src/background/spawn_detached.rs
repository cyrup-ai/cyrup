//! Hop-1 detached second-process spawn (func-SA R-SA-070/071; arch-SA §6.5).
//!
//! This is the FIRST of the two OS-process hops func-SA §1.1 mandates for background execution:
//! the orchestrator (this process) spawns the `cyrup` binary again, this time selecting the
//! internal `__subagent-runner --config <path>` subcommand, as a genuinely **detached** second
//! process — new process group / session leader on Unix (`DETACHED_PROCESS |
//! CREATE_NEW_PROCESS_GROUP` on Windows), stdio fully redirected to files (never inherited), and
//! — the entire point of this module — the resulting [`tokio::process::Child`] handle is dropped
//! **without ever being awaited**. Hop 2 (the runner's own step-by-step execution loop, itself
//! re-execing further children through [`crate::spawn::SpawnedChild`]) is `background::
//! runner_main`, a later phase of this crate's build-out (not yet implemented, hence a plain
//! module-path reference here rather than an intra-doc link); this module's only job is getting
//! that second process successfully off the ground and confirmed alive via its pid, then getting
//! entirely out of its way.
//!
//! # Why "never awaited" is load-bearing, not an oversight
//!
//! R-SA-071 requires the detached runner to outlive the orchestrator: if the orchestrating
//! `cyrup` process crashes, is `Ctrl-C`'d, or exits normally while a background run is still in
//! flight, the runner MUST keep going to completion. `tokio::process::Child` has no destructor
//! that kills the underlying OS process on `Drop` (unlike, say, `std::process::Child` on some
//! other language runtimes) — dropping it here simply releases *this* process's in-memory handle
//! to the child's pid/pipes; the real OS process keeps running under its own, already-detached
//! process group, completely independent of whether this process's `tokio::process::Child` value
//! is still alive. Awaiting the child here (`child.wait().await`) would be actively wrong: it
//! would block the calling task until the ENTIRE background run finishes, defeating R-SA-074's
//! "return immediately after confirmed spawn" contract and this whole subsystem's reason for
//! existing.
//!
//! # Why stdio must be files, never inherited
//!
//! `Stdio::inherit()` would tie the child's stdout/stderr file descriptors to the orchestrator's
//! own terminal/pipe — if the orchestrator later closes those descriptors (process exit,
//! `Ctrl-C`-triggered pipe teardown), a still-running detached child writing to an inherited fd
//! could receive `SIGPIPE`/`EPIPE` and be killed or corrupted through no fault of its own, directly
//! undermining R-SA-071. Redirecting to real files owned by the run's own [`super::RunPaths`]
//! (`runner.stdout.log`/`runner.stderr.log`) makes the child's stdio lifetime independent of the
//! orchestrator's own descriptors, and incidentally gives an operator a durable place to inspect
//! what the detached runner printed before it had a chance to write its first `status.json`
//! (`background/runner_main.rs`'s R-SA-075 initial-status write is not instantaneous — these log
//! files are the fallback diagnostic surface for the sliver of time before that write lands).
//!
//! # The one thing it DOES put in the child's environment (PERM-001)
//!
//! R-SA-073 routes all runner *configuration* through the one-shot config file, never env blobs,
//! and that is still true. The single exception is the R-SA-P1 parent-session ANCHOR
//! ([`crate::exec::PARENT_SESSION_ENV_VAR`]), which is not runner configuration at all — it is
//! ambient process identity that pi propagates purely by environment INHERITANCE
//! (`pi-subagents/src/extension/index.ts:555` publishes it into the orchestrator's own
//! `process.env`, so every descendant at every hop simply inherits it). cyrup's orchestrator cannot
//! write its own `process.env` (`#![forbid(unsafe_code)]` + 2024-edition `unsafe std::env::set_var`),
//! so this module writes that ONE entry explicitly onto the hop-1 child instead, resolved by
//! [`super::parent_anchor::detached_runner_env_overlay`].
//!
//! Without it the whole background half of permission ask-forwarding was dead: the hop-2 runner had
//! no anchor in its environment, so `exec::build_attempt_spawn_plan`'s "explicit → inherited env →
//! empty" ladder resolved EMPTY for every hop-3 child, so a background subagent that hit an `ask`
//! addressed a null forwarding target and `cyrup-permission-system` fail-closed denied it with no
//! prompt ever reaching the operator.
//!
//! # What this module does NOT do
//!
//! - It does not write `runner-config.json` itself — the caller (a later phase's background-run
//!   entry point in `exec/`/`registration/`) is responsible for serializing the resolved
//!   `RunnerStep`s / cwd / session-file path into the one-shot config file per R-SA-073 and
//!   passing its path in; this module only accepts an already-written `cfg_path` and threads it
//!   through as the `--config` argv value.
//! - It does not create `status.json` or any provisional status ([`super::RunStatus::provisional`]
//!   exists for exactly that grace-window need) — the caller does that immediately after a
//!   successful [`spawn_detached_runner`] call, using the pid this function returns.
//! - It does not itself delete the config file after read — R-SA-073's "runner MUST delete this
//!   config file immediately after reading it" is the runner's own responsibility
//!   (`background/runner_main.rs`), executing inside the detached second process, not this
//!   (orchestrator-side) spawn call.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::SubagentError;
use crate::spawn::SpawnCommand;

/// The internal `cyrup` CLI subcommand a detached runner process is launched under (registered in
/// `crates/cyrup/src/subagent_runner_cmd.rs`, outside this crate — see arch-SA §2.2's crate/module
/// layout: `cyrup` is the one binary crate that owns CLI-subcommand dispatch, this crate is a pure
/// library the subcommand handler calls into).
const SUBAGENT_RUNNER_SUBCOMMAND: &str = "__subagent-runner";

/// The argv flag preceding the one-shot runner-config file path (R-SA-073).
const CONFIG_FLAG: &str = "--config";

/// Windows `CREATE_NO_WINDOW`-adjacent creation flag constants (`winbase.h`), inlined as literal
/// `u32`s rather than pulled from an extra Windows-only crate dependency — mirrors this crate's
/// existing convention of inlining well-known OS constants (`spawn::signal` inlines `nix` signal
/// numbers via the `nix` crate already in this crate's dependency closure; these two flags have no
/// `nix`-equivalent workspace dependency to borrow from on the Windows side, so they are named
/// constants here instead of magic numbers inline in [`spawn_detached_runner`]).
#[cfg(windows)]
mod windows_flags {
    /// The child process has no console of its own and is not attached to the parent's console —
    /// the load-bearing half of Windows detachment: without this, the child would remain part of
    /// the parent's console session and could be signaled/torn down alongside it.
    pub(super) const DETACHED_PROCESS: u32 = 0x0000_0008;
    /// The child becomes the root of a new process group, so a `CTRL_C_EVENT`/`CTRL_BREAK_EVENT`
    /// sent to the parent's console (if any) is not automatically propagated to the child — the
    /// Windows analog of Unix's `process_group(0)` isolating the child from the parent's own
    /// signal disposition (func-SA R-SA-070's "not signaled by the parent's process group").
    pub(super) const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
}

/// Spawn the `cyrup` binary as a genuinely detached second OS process running the internal
/// `__subagent-runner --config <cfg_path>` subcommand (R-SA-070/071).
///
/// # Parameters
///
/// - `cfg_path`: path to the already-written, one-shot runner-config file (R-SA-073) — passed
///   verbatim as the `--config` argv value. This function does not read, validate, or take
///   ownership of this file; the runner (hop 2) reads and deletes it.
/// - `stdout_log_path` / `stderr_log_path`: real files the child's stdout/stderr are redirected
///   to (never inherited — see module docs). Both are created (or truncated, if a stale file from
///   a prior run with the same path somehow still exists) synchronously, before `spawn()` is
///   called, so a failure to create either surfaces as a clean [`SubagentError::Spawn`] rather
///   than a partially-launched child.
///
/// The child's environment is INHERITED from this process (no `env_clear()`, matching
/// [`crate::spawn::SpawnedChild::spawn`]'s identical inherit-only-unless-overlaid convention) and
/// then overlaid with [`super::parent_anchor::detached_runner_env_overlay`] — in practice the one
/// R-SA-P1 parent-session anchor entry, and nothing else (PERM-001; see the module docs for why
/// that single entry is not an R-SA-073 violation). Everything that is genuinely runner
/// CONFIGURATION still travels through the one-shot config file (`cfg_path`), never env.
///
/// # Detachment mechanism
///
/// - **Unix**: [`tokio::process::Command::process_group`]`(0)` — the child becomes the leader of
///   its own new process group (pid == pgid), so it is never signaled as a side effect of a
///   signal sent to the orchestrator's own process group (e.g. a terminal-driven `Ctrl-C`
///   SIGINT-to-foreground-process-group, which by default targets every process sharing that
///   group). This is the identical mechanism [`crate::spawn::SpawnedChild::spawn`] and
///   `exec::acceptance::run_one_verify_command` already use for their own (non-detached, but
///   still signal-isolated) children — reused here rather than inventing a second convention.
/// - **Windows**: `creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)` — the nearest
///   platform equivalent per R-SA-070's own "or the nearest platform equivalent" clause.
///
/// # Return value and the "never awaited" contract
///
/// On success, returns the child's OS pid. The [`tokio::process::Child`] value itself is dropped
/// before this function returns — it is NEVER `.wait()`-ed, NEVER `.wait_with_output()`-ed, and no
/// task is spawned to await it later. This is the entire point of detachment (see module docs):
/// the real OS process's lifetime is already fully independent of this process's in-memory
/// handle, and this function's only remaining job once `spawn()` succeeds is confirming (and
/// returning) the pid.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if either log file cannot be created, if `spawn()` itself
/// fails (binary not found, permission denied, resource limits, …), or — in the practically
/// unreachable case where `spawn()` succeeds but the OS declines to report a pid at all — if no
/// pid is available to confirm the detached process actually started (this crate treats "spawned
/// but we cannot learn its pid" as equivalent to a spawn failure, since every other part of this
/// subsystem, from the R-SA-090 provisional-status grace window to R-SA-081's interrupt-signal
/// delivery, is keyed on having a real pid in hand).
pub fn spawn_detached_runner(
    cfg_path: &Path,
    stdout_log_path: &Path,
    stderr_log_path: &Path,
) -> Result<u32, SubagentError> {
    spawn_detached_runner_with_command(
        &crate::spawn::resolve_spawn_command(),
        cfg_path,
        stdout_log_path,
        stderr_log_path,
        // PERM-001: the R-SA-P1 anchor this orchestrator should hand downward — INHERITED (this
        // process is itself a subagent child launching a nested background run) → PUBLISHED (the
        // root orchestrator's live session id, put in the register by the permission companion's
        // PARENT-role `SessionStart`) → none. Resolved HERE, in the env-reading wrapper, for the
        // same reason `spawn::resolve_spawn_command` is: the injectable core below stays pure.
        &super::parent_anchor::detached_runner_env_overlay(),
    )
}

/// The pure(r) core of [`spawn_detached_runner`], parameterized over which resolved
/// [`SpawnCommand`] to re-exec, so tests can substitute the scripted `cyrup-subagent-fixture`
/// binary (arch-SA §11) WITHOUT mutating real process environment state — this crate is
/// `#![forbid(unsafe_code)]`, and `std::env::set_var` is `unsafe` as of the 2024 edition, so
/// (mirroring `spawn::mod::resolve_spawn_command_from` and `spawn::depth::
/// resolve_effective_depth_from`'s identical injectable-core convention) the real env-reading
/// entry point ([`spawn_detached_runner`]) is a thin wrapper around this fully-parameterized,
/// directly-testable function.
///
/// See [`spawn_detached_runner`] for the full parameter/return/detachment/error contract — this
/// function's behavior is identical, it merely accepts `command` and `env_overlay` explicitly
/// instead of resolving them from this process's own environment internally.
///
/// `env_overlay` is applied on TOP of the inherited environment (never `env_clear`), exactly as
/// [`crate::spawn::SpawnedChild::spawn`] applies its own overlay. Production passes
/// [`super::parent_anchor::detached_runner_env_overlay`]; an empty map reproduces the historical
/// inherit-only behavior verbatim.
///
/// # Errors
///
/// See [`spawn_detached_runner`].
pub fn spawn_detached_runner_with_command(
    spawn_command: &SpawnCommand,
    cfg_path: &Path,
    stdout_log_path: &Path,
    stderr_log_path: &Path,
    env_overlay: &BTreeMap<String, String>,
) -> Result<u32, SubagentError> {
    let stdout_file = std::fs::File::create(stdout_log_path).map_err(SubagentError::Spawn)?;
    let stderr_file = std::fs::File::create(stderr_log_path).map_err(SubagentError::Spawn)?;

    let mut command = tokio::process::Command::new(&spawn_command.binary);
    command
        .args(&spawn_command.base_args)
        .arg(SUBAGENT_RUNNER_SUBCOMMAND)
        .arg(CONFIG_FLAG)
        .arg(cfg_path)
        // R-SA-070: stdin/stdout/stderr redirected to real files, never inherited from the
        // orchestrator (see module docs for why inheriting would undermine R-SA-071).
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(stdout_file))
        .stderr(std::process::Stdio::from(stderr_file))
        // PERM-001: overlay (never `env_clear`) — the R-SA-P1 anchor pi would have propagated by
        // plain `process.env` inheritance, written explicitly here because cyrup's orchestrator
        // cannot mutate its own environment. Empty in every no-anchor case.
        .envs(env_overlay);

    #[cfg(unix)]
    {
        // New process group (pid == pgid): isolates the detached child from any signal sent to
        // the orchestrator's own process group (R-SA-070's "not signaled by the parent's process
        // group"). Inherent method on `tokio::process::Command` — no extension-trait import
        // needed, mirroring `spawn::SpawnedChild::spawn`'s identical usage.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_flags::DETACHED_PROCESS | windows_flags::CREATE_NEW_PROCESS_GROUP);
    }

    let child = command.spawn().map_err(SubagentError::Spawn)?;

    let pid = child.id().ok_or_else(|| {
        SubagentError::Spawn(std::io::Error::other(
            "detached runner spawned but reported no pid",
        ))
    })?;

    // THE POINT OF THIS FUNCTION: drop the child handle without ever awaiting it. The real OS
    // process keeps running under its own detached process group, entirely independent of this
    // in-process `tokio::process::Child` value's lifetime (module docs explain why `Drop` here is
    // safe and correct, not a leak). No `.wait()`, no `.wait_with_output()`, no `tokio::spawn` to
    // await it on a background task later — any of those would defeat R-SA-071/R-SA-074.
    drop(child);

    Ok(pid)
}

#[cfg(test)]
mod tests {
    //! Fast, no-real-subprocess-needed unit tests only. This crate's `[[bin]]`
    //! `cyrup-subagent-fixture` target only exposes `CARGO_BIN_EXE_cyrup-subagent-fixture` to
    //! ordinary Cargo **integration** tests (files under `tests/`), not to a library's own
    //! `#[cfg(test)]` unit-test module — `env!("CARGO_BIN_EXE_...")` is unavailable here at
    //! compile time. The full real-subprocess proof this module exists for (a genuinely detached
    //! process that keeps running independent of the spawning test's own lifetime, process-group
    //! isolation, stdio redirection, and the `--config` argv contract) therefore lives in
    //! `tests/background_spawn_detached_integration.rs`, mirroring this crate's own established
    //! convention for the identical constraint (`tests/exec_run_sync_integration.rs`'s module
    //! docs explain the same env-var availability boundary). The tests kept here cover the parts
    //! of this module's contract that do NOT require a real fixture binary: constructing a
    //! [`SpawnCommand`] by hand and asserting on the clean-failure path when stdio redirection
    //! itself cannot be established.

    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// A missing parent directory for the stdout log file fails cleanly (surfaced as
    /// [`SubagentError::Spawn`]) rather than spawning a half-configured child — no process should
    /// ever be launched if its own stdio redirection cannot be established first. Uses a
    /// hand-built [`SpawnCommand`] (never actually reached, since stdio setup fails first) so this
    /// test needs no real fixture binary at all.
    #[tokio::test]
    async fn missing_log_directory_fails_cleanly_without_spawning() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        std::fs::write(&cfg_path, "{}").expect("write placeholder config");

        let bogus_dir = dir.path().join("does-not-exist");
        let stdout_log = bogus_dir.join("runner.stdout.log");
        let stderr_log = bogus_dir.join("runner.stderr.log");

        let command = SpawnCommand {
            binary: std::path::PathBuf::from("does-not-matter-stdio-setup-fails-first"),
            base_args: Vec::new(),
        };
        let result = spawn_detached_runner_with_command(
            &command,
            &cfg_path,
            &stdout_log,
            &stderr_log,
            &BTreeMap::new(),
        );
        assert!(
            matches!(result, Err(SubagentError::Spawn(_))),
            "a missing log directory must fail cleanly as SubagentError::Spawn, never panic: \
             {result:?}"
        );
    }

    /// [`spawn_detached_runner_with_command`] builds argv in the exact `SUBAGENT_RUNNER_SUBCOMMAND
    /// CONFIG_FLAG cfg_path` order this module's docs promise, includes `command.base_args` ahead
    /// of both, AND applies `env_overlay` to the spawned process (PERM-001) — verified by spawning
    /// a real (trivial, always-available) `sh` wrapper that dumps its own argv and the anchor it
    /// received, without depending on the scripted `cyrup-subagent-fixture` binary at all. `sh` is
    /// resolved to its absolute path via this test's own real `PATH`, exactly mirroring
    /// `spawn::mod::tests::sh_command`'s established convention for a real-but-always-present
    /// stand-in binary.
    ///
    /// REWRITTEN for PERM-001. The previous version never called the function under test: it
    /// hand-built an equivalent `tokio::process::Command` and asserted on THAT, because (its own
    /// comment said) "`spawn_detached_runner_with_command` itself does not expose an env-overlay
    /// parameter (by design)" and the argv dump needed one env var. That premise is now false —
    /// the function takes the overlay — so the test calls the real thing, which additionally makes
    /// it a genuine regression test for the anchor entry: against the pre-fix code the
    /// `CYRUP_SUBAGENT_PARENT_SESSION` assertion below cannot pass, because nothing the hop-1
    /// spawn did could put it in the child's environment.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn argv_order_and_env_overlay_reach_the_detached_process() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        std::fs::write(&cfg_path, "{}").expect("write placeholder config");
        let stdout_log = dir.path().join("runner.stdout.log");
        let stderr_log = dir.path().join("runner.stderr.log");

        let sh_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("sh"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
        let command = SpawnCommand {
            binary: sh_path,
            base_args: vec![
                "-c".to_string(),
                // Dump argv, then the anchor the overlay was supposed to deliver, then a DONE
                // sentinel so the poll below never reads a half-written file.
                concat!(
                    r#"for a in "$@"; do printf '%s\n' "$a" >> "$CYRUP_TEST_ARGV_OUT"; done; "#,
                    r#"printf 'ANCHOR=%s\n' "$CYRUP_SUBAGENT_PARENT_SESSION" >> "$CYRUP_TEST_ARGV_OUT"; "#,
                    r#"printf 'DONE\n' >> "$CYRUP_TEST_ARGV_OUT""#
                )
                .to_string(),
                "--".to_string(),
            ],
        };

        let argv_out = dir.path().join("argv.txt");
        let mut overlay = BTreeMap::new();
        overlay.insert("CYRUP_TEST_ARGV_OUT".to_string(), argv_out.display().to_string());
        overlay.insert(
            crate::exec::PARENT_SESSION_ENV_VAR.to_string(),
            "session-detached-anchor".to_string(),
        );

        let pid = spawn_detached_runner_with_command(
            &command,
            &cfg_path,
            &stdout_log,
            &stderr_log,
            &overlay,
        )
        .expect("detached sh spawns");
        assert!(pid > 0, "a confirmed spawn must report a real pid");

        // The child is deliberately never awaited (that is this module's whole contract), so poll
        // for its own DONE sentinel instead of `wait()`ing on it.
        let mut contents = String::new();
        for _ in 0..200 {
            contents = std::fs::read_to_string(&argv_out).unwrap_or_default();
            if contents.contains("DONE") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines,
            vec![
                SUBAGENT_RUNNER_SUBCOMMAND,
                CONFIG_FLAG,
                cfg_path.display().to_string().as_str(),
                "ANCHOR=session-detached-anchor",
                "DONE",
            ],
            "argv must be [subcommand, config flag, config path] after base_args, and the \
             env overlay must reach the detached process (PERM-001)"
        );
    }

    /// PERM-001 regression, the negative direction: an EMPTY overlay must leave the child's
    /// environment exactly as inherited — no `CYRUP_SUBAGENT_PARENT_SESSION=""` entry that would
    /// MASK an anchor the process legitimately inherited (a spawn env is an overlay, never a
    /// replacement).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_overlay_writes_no_masking_anchor_entry() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        std::fs::write(&cfg_path, "{}").expect("write placeholder config");
        let stdout_log = dir.path().join("runner.stdout.log");
        let stderr_log = dir.path().join("runner.stderr.log");
        let probe_out = dir.path().join("env.txt");

        let sh_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("sh"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
        // The probe path is baked into the script text (not the overlay) precisely because this
        // test must pass an EMPTY overlay to the function under test.
        let script = format!(
            "printf 'SET=%s\\n' \"${{{var}+yes}}\" >> {out}; printf 'DONE\\n' >> {out}",
            var = crate::exec::PARENT_SESSION_ENV_VAR,
            out = probe_out.display(),
        );
        let command = SpawnCommand {
            binary: sh_path,
            base_args: vec!["-c".to_string(), script, "--".to_string()],
        };

        spawn_detached_runner_with_command(
            &command,
            &cfg_path,
            &stdout_log,
            &stderr_log,
            &BTreeMap::new(),
        )
        .expect("detached sh spawns");

        let mut contents = String::new();
        for _ in 0..200 {
            contents = std::fs::read_to_string(&probe_out).unwrap_or_default();
            if contents.contains("DONE") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        // `${VAR+yes}` expands to `yes` only when VAR is SET (even to ""), so this distinguishes
        // "absent" from "present but empty" — exactly the masking case being ruled out. The
        // ambient test environment normally has no anchor; if it somehow does, inheritance (not
        // the overlay) is what set it, and the assertion is skipped.
        if std::env::var(crate::exec::PARENT_SESSION_ENV_VAR).is_ok() {
            return;
        }
        assert_eq!(
            contents, "SET=\nDONE\n",
            "an empty overlay must add no anchor entry at all"
        );
    }
}
