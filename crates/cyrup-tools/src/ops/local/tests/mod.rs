//! Real-process behaviour tests for the local process backend, and the fixtures they share.
//!
//! Every case here spawns a real child and asserts on what the OS did with it, so the fixtures
//! (`exec_spec`, `argv`, `SleeperMarker`, `pid_exists`, `is_tracked`, `wait_gone`) are shared from
//! this module rather than duplicated per file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod exec;
mod exec_argv;
mod signal;
mod tracking;

use super::tracking::tracked_detached_child_pids;
use super::*;
use crate::ops::{ArgvSpec, ExecSpec, ExitStatus, ProcOps, ShellConfig};
use std::time::Duration;

// Every caller builds an `sh`-based spec and so lives under `#[cfg(unix)]`; the helper itself
// is portable. Silenced rather than gated so it stays available to any future Windows test.
#[cfg_attr(not(unix), allow(dead_code))]
fn argv(program: &str, args: &[&str]) -> ArgvSpec {
    ArgvSpec {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: std::env::temp_dir(),
        env: Vec::new(),
    }
}

fn exec_spec(command: &str) -> ExecSpec {
    ExecSpec {
        command: command.to_string(),
        cwd: std::env::temp_dir(),
        env: Vec::new(),
        env_remove: Vec::new(),
        shell: ShellConfig::detect(),
    }
}

/// A reapable stand-in for the `while true; do sleep 1; done` body these `exec_argv` fixtures
/// need, and the second half of the crate's `LEAK-FAIL` story (TOOL-042's residual).
///
/// ## Why a fixture that leaves a one-second `sleep` behind turns an UNRELATED test red
///
/// `exec_argv` deliberately kills by SINGLE PID and never `killpg` — pi's `execCommand` /
/// `killProcess` (`exec.ts:34-63` @v0.83.0) spawns without `detached` and calls a bare,
/// un-negated `proc.kill(...)`, so a grandchild of the spawned `sh` is upstream-correct to
/// survive. `exec_argv_kill_signals_only_the_single_pid_never_the_process_group` exists to
/// prove exactly that. The three fixtures below therefore leave a live `sleep 1` behind by
/// DESIGN — and until this helper landed, none of them reaped it, so the process outlived the
/// whole test binary by up to a second.
///
/// That survivor is what converts nextest's per-test pipe accounting into a red on a test that
/// spawns nothing at all. macOS has no `pipe2(2)`, so Rust's `anon_pipe` is `pipe(2)` followed
/// by a separate `ioctl(FIOCLEX)` — the two are NOT atomic. **nextest** creates the stdout and
/// stderr pipes for every test process in its own address space, from a thread pool, while
/// concurrently spawning other test processes; a spawn landing inside another pipe's
/// pre-`FIOCLEX` window inherits that pipe's WRITE end at some fd above 2, where no `dup2` in
/// `build_argv_command` touches it. The test process that inherited it passes it on to every
/// child it forks. When the inheriting test exits the stray fd goes with it — but a surviving
/// GRANDCHILD keeps it, and the victim named by `LEAK-FAIL` is whichever test that pipe
/// belonged to, which is why the victim is arbitrary and is usually a test with no spawn in it.
/// Measured at HEAD before this fix: 16 leaks over ~120 runs of the spawn-dense subset, naming
/// nine different victims including `read_variant_probe_uses_existence_not_readability`,
/// `edit_rechecks_cancellation_after_the_write_lands` and the pure source-scan
/// `shell_probe_loops_reap_on_the_error_arm_not_just_the_deadline` — none of which fork.
///
/// cyrup owns neither half of the race (it is std's non-atomic CLOEXEC inside nextest's
/// process), but it owns the AMPLIFIER: a stray fd is only observable for as long as some
/// process holds it, and `leak-timeout` is 500 ms (`.config/nextest.toml:42`). A grandchild
/// reaped before its fixture returns closes the window; a `sleep 1` left running does not.
///
/// ## The shape
///
/// [`Self::reapable_sleep_loop`] keeps the loop semantics the fixtures assert on — a shell that
/// stays alive across signals with a real forked descendant — and adds only `echo $!` into a
/// marker file, the same record-and-reap pattern
/// `a_dropped_exec_future_kills_the_whole_process_group_not_just_the_direct_child` and
/// `exec_argv_does_not_hang_on_a_backgrounded_descendant_holding_the_pipe_open` already use.
#[cfg(unix)]
struct SleeperMarker {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl SleeperMarker {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("cyrup-sleeper-{tag}-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Self { path }
    }

    /// `<prefix>while true; do sleep 1 & echo $! > MARKER; wait; done` — behaviourally the same
    /// forever-loop-over-a-forked-`sleep` as the literal it replaces (`wait` with no argument
    /// waits for the one background job, and is interrupted by a trapped signal exactly as the
    /// foreground `sleep` was), with the current descendant's pid recorded so [`Self::reap`]
    /// can kill it.
    fn reapable_sleep_loop(&self, prefix: &str) -> String {
        format!(
            "{prefix}while true; do sleep 1 & echo $! > {}; wait; done",
            self.path.display()
        )
    }

    /// SIGKILL the recorded descendant and WAIT for it to actually be gone, if it got as far as
    /// recording itself. Best-effort by construction: the marker is absent when the shell was
    /// killed before its first iteration, which is the case where there is nothing to reap.
    ///
    /// The bounded wait is the load-bearing half. `kill(2)` only QUEUES the signal; the fds the
    /// process holds are released when it is torn down, which is what the leak window cares
    /// about — returning the instant `kill` returns would leave exactly the race this helper
    /// exists to close. Bounded rather than unbounded so a reap that cannot complete degrades to
    /// today's behaviour instead of hanging the test.
    fn reap(&self) {
        if let Ok(text) = std::fs::read_to_string(&self.path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            let _ = kill_pid(pid);
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while pid_exists(pid) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Liveness probe that does NOT perturb what it measures: `kill(pid, 0)` performs the
/// permission/existence check and delivers nothing. `terminate_pid` cannot be used here — its
/// `SIGTERM` would kill a `sleep` that the assertion needs to observe as still alive.
#[cfg(unix)]
#[allow(unsafe_code)]
fn pid_exists(pid: u32) -> bool {
    // SAFETY: `kill(2)` with signal 0 reads its two integer arguments and touches no memory.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Is `pid` currently in the process-global detached-child registry?
#[cfg(unix)]
fn is_tracked(pid: u32) -> bool {
    tracked_detached_child_pids().contains(&pid)
}

/// Poll until `pid` is gone, up to `deadline`. A `killpg`'d process is a zombie until reaped and
/// `kill(pid, 0)` succeeds on a zombie, so a single sample would be racy.
#[cfg(unix)]
async fn wait_gone(pid: u32, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if !pid_exists(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
