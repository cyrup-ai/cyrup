//! A small bounded argv runner: spawn one program (no shell), and guarantee it cannot outlive a
//! deadline, a stop signal or an output budget.
//!
//! This is the Rust shape of pi-subagents' `src/runs/shared/worktree-setup-command.ts`
//! `runSetupCommand` (@v0.71.0), minus the parts that are specific to its worktree-setup
//! transaction (progress snapshots, `processTree` proofs, `acceptedExitCodes` — the caller
//! decides which exit codes it accepts, since here the exit status is just data):
//!
//! | upstream                                          | here                                        |
//! |---------------------------------------------------|---------------------------------------------|
//! | `detached: true` owned POSIX process group        | `Command::process_group(0)`                 |
//! | `maxBuffer` (1 MiB) overflow → kill, `ENOBUFS`    | [`Bounds::max_bytes`] → [`BoundedError::OutputOverflow`] |
//! | `signal` aborted → kill tree, `ABORT_ERR`         | [`Bounds::cancel`] → [`BoundedError::Aborted`] |
//! | `deadlineAt` passed → kill tree, `ETIMEDOUT`      | [`Bounds::deadline`] → [`BoundedError::DeadlineExceeded`] |
//! | already aborted / past the deadline: never spawn  | the same pre-check                          |
//!
//! Killing goes through [`crate::spawn::signal::terminate_on_timeout`] (SIGTERM, one second of
//! grace, SIGKILL, confirmed reaped), followed by a `SIGKILL` of the whole process group so a
//! descendant that outlived the leader cannot keep the pipes (or the repository) busy.
//!
//! Output is never truncated silently: a program that writes past the budget is killed and the
//! call fails, so a caller can never mistake a cut-off `git diff` for a complete one.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Instant;

use cyrup_core::CancelToken;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// pi's default `maxBuffer` for setup commands (`spawnSync`'s 1 MiB).
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

/// What bounds one [`run_bounded_argv`] call.
#[derive(Debug, Clone)]
pub struct Bounds {
    /// The run's stop token (pi `signal`). `None` = this call cannot be stopped from outside.
    pub cancel: Option<CancelToken>,
    /// The absolute instant after which the program is killed (pi `deadlineAt`). `None` = none.
    pub deadline: Option<Instant>,
    /// Combined stdout + stderr byte budget; exceeding it kills the program.
    pub max_bytes: usize,
}

impl Bounds {
    /// No deadline, no stop token, pi's default 1 MiB output budget.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self {
            cancel: None,
            deadline: None,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl Default for Bounds {
    fn default() -> Self {
        Self::unbounded()
    }
}

/// A completed (exited on its own, within bounds) program.
#[derive(Debug)]
pub struct BoundedOutput {
    /// Captured standard output (at most [`Bounds::max_bytes`] together with stderr).
    pub stdout: Vec<u8>,
    /// Captured standard error.
    pub stderr: Vec<u8>,
    /// The exit code; `None` when the program was ended by a signal it did not cause us to send.
    pub status: Option<i32>,
}

/// Why a bounded program did not complete.
#[derive(Debug, thiserror::Error)]
pub enum BoundedError {
    /// The program could not be started or its pipes failed (pi: a spawn `error` event).
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// [`Bounds::cancel`] fired (pi `ABORT_ERR`). The program and its group are gone.
    #[error("aborted")]
    Aborted,
    /// [`Bounds::deadline`] passed (pi `ETIMEDOUT`). The program and its group are gone.
    #[error("deadline exceeded")]
    DeadlineExceeded,
    /// The program wrote more than [`Bounds::max_bytes`] (pi `ENOBUFS`). It was killed.
    #[error("output exceeds {max_bytes} bytes")]
    OutputOverflow {
        /// The budget that was exceeded.
        max_bytes: usize,
    },
}

enum Stop {
    Aborted,
    Deadline,
    Overflow,
}

/// Read one chunk from an optional pipe; EOF / error report `0`, which the caller treats as
/// closed. With the pipe `None` this pends forever, and the `select!` guard never polls it.
async fn read_chunk<R: AsyncRead + Unpin>(pipe: &mut Option<R>, buffer: &mut [u8]) -> usize {
    match pipe.as_mut() {
        Some(pipe) => pipe.read(buffer).await.unwrap_or(0),
        None => std::future::pending().await,
    }
}

/// Append as much of `chunk` as the shared budget allows; `true` when `chunk` did not fit.
fn capture(sink: &mut Vec<u8>, captured: &mut usize, max_bytes: usize, chunk: &[u8]) -> bool {
    let remaining = max_bytes.saturating_sub(*captured);
    let take = chunk.len().min(remaining);
    sink.extend_from_slice(chunk.get(..take).unwrap_or_default());
    *captured += take;
    chunk.len() > remaining
}

/// Run `program` with `args` in `cwd` (plus `env` on top of the inherited environment), bounded.
///
/// `input`, when present, is written to the program's stdin and stdin is then closed; when absent
/// stdin is the null device. The program leads its own process group.
///
/// # Errors
///
/// [`BoundedError::Aborted`] / [`BoundedError::DeadlineExceeded`] when the bound was already
/// exhausted (nothing is spawned) or fired while the program ran (it is killed and reaped before
/// this returns); [`BoundedError::OutputOverflow`] when the output budget is exceeded;
/// [`BoundedError::Io`] when the spawn or a pipe read fails.
pub async fn run_bounded_argv<S: AsRef<OsStr>>(
    program: &OsStr,
    args: &[S],
    cwd: &Path,
    env: &[(&str, &OsStr)],
    input: Option<Vec<u8>>,
    bounds: &Bounds,
) -> Result<BoundedOutput, BoundedError> {
    // pi `runSetupCommand`: `result.error = cancellation(); if (result.error) return result;`
    if bounds
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
    {
        return Err(BoundedError::Aborted);
    }
    if bounds.deadline.is_some_and(|at| Instant::now() >= at) {
        return Err(BoundedError::DeadlineExceeded);
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Backstop only: every path below kills the group explicitly and reaps the child.
        .kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value);
    }
    // pi `detached: true`: the program owns a process group, so the kill below reaches its
    // descendants (a git credential helper, a hook's children).
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let pgid: Option<i32> = child.id().and_then(|pid| i32::try_from(pid).ok());

    // The payload is written from its own task so a program that never reads stdin cannot wedge
    // the select loop below. A failed write (EPIPE: the program exited without reading) is not
    // an error in itself — the exit status and output decide the outcome.
    let stdin_writer = child.stdin.take().zip(input).map(|(mut stdin, payload)| {
        tokio::spawn(async move {
            let _ = stdin.write_all(&payload).await;
            let _ = stdin.shutdown().await;
        })
    });

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut out_chunk = vec![0u8; 65536];
    let mut err_chunk = vec![0u8; 65536];
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut captured = 0usize;
    let mut exit: Option<std::io::Result<std::process::ExitStatus>> = None;
    let mut stop: Option<Stop> = None;

    let deadline = bounds.deadline.map(tokio::time::Instant::from_std);
    let cancel = bounds.cancel.clone().unwrap_or_default();
    let has_cancel = bounds.cancel.is_some();

    while stop.is_none() && (exit.is_none() || stdout_pipe.is_some() || stderr_pipe.is_some()) {
        let stdout_open = stdout_pipe.is_some();
        let stderr_open = stderr_pipe.is_some();
        tokio::select! {
            biased;
            read = read_chunk(&mut stdout_pipe, &mut out_chunk), if stdout_open => {
                if read == 0 {
                    stdout_pipe = None;
                } else if capture(&mut stdout, &mut captured, bounds.max_bytes, out_chunk.get(..read).unwrap_or_default()) {
                    stop = Some(Stop::Overflow);
                }
            }
            read = read_chunk(&mut stderr_pipe, &mut err_chunk), if stderr_open => {
                if read == 0 {
                    stderr_pipe = None;
                } else if capture(&mut stderr, &mut captured, bounds.max_bytes, err_chunk.get(..read).unwrap_or_default()) {
                    stop = Some(Stop::Overflow);
                }
            }
            status = child.wait(), if exit.is_none() => {
                exit = Some(status);
            }
            () = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                stop = Some(Stop::Deadline);
            }
            () = cancel.cancelled(), if has_cancel => {
                stop = Some(Stop::Aborted);
            }
        }
    }

    if let Some(writer) = stdin_writer {
        writer.abort();
    }

    if let Some(reason) = stop {
        // Kill and REAP: this returns only once the OS process is gone, so a stopped command can
        // never still hold the repository when the caller reports the failure.
        let _ = crate::spawn::signal::terminate_on_timeout(&mut child).await;
        sweep_group(pgid);
        return Err(match reason {
            Stop::Aborted => BoundedError::Aborted,
            Stop::Deadline => BoundedError::DeadlineExceeded,
            Stop::Overflow => BoundedError::OutputOverflow {
                max_bytes: bounds.max_bytes,
            },
        });
    }

    let status =
        exit.ok_or_else(|| std::io::Error::other("child exit status was not observed"))??;
    Ok(BoundedOutput {
        stdout,
        stderr,
        status: status.code(),
    })
}

/// `SIGKILL` the program's whole process group. `ESRCH` (already empty) is the normal case.
#[cfg(unix)]
fn sweep_group(pgid: Option<i32>) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    if let Some(pgid) = pgid.filter(|pgid| *pgid > 1) {
        let _ = kill(Pid::from_raw(-pgid), Signal::SIGKILL);
    }
}

#[cfg(not(unix))]
fn sweep_group(_pgid: Option<i32>) {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::time::Duration;

    use super::*;

    fn sleep_argv(seconds: &str) -> (&'static OsStr, Vec<String>) {
        (OsStr::new("sleep"), vec![seconds.to_string()])
    }

    async fn run(
        program: &OsStr,
        args: &[String],
        bounds: &Bounds,
    ) -> Result<BoundedOutput, BoundedError> {
        run_bounded_argv(program, args, Path::new("."), &[], None, bounds).await
    }

    /// Without bounds a program's own exit is the only end, and its output and status come back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_program_that_exits_on_its_own_returns_its_output() {
        let args = vec![
            "-c".to_string(),
            "echo out; echo err >&2; exit 3".to_string(),
        ];
        let output = run(OsStr::new("sh"), &args, &Bounds::unbounded())
            .await
            .expect("completes");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "err");
        assert_eq!(output.status, Some(3));
    }

    /// stdin reaches the program and is closed after the payload.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn input_is_written_to_stdin_and_closed() {
        let output = run_bounded_argv(
            OsStr::new("cat"),
            &[] as &[&str],
            Path::new("."),
            &[],
            Some(b"payload".to_vec()),
            &Bounds::unbounded(),
        )
        .await
        .expect("completes");
        assert_eq!(output.stdout, b"payload");
    }

    /// SUBA-130: a hung program is ended AT the deadline, not when it decides to exit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hung_program_is_killed_at_the_deadline() {
        let (program, args) = sleep_argv("30");
        let bounds = Bounds {
            deadline: Some(Instant::now() + Duration::from_millis(300)),
            ..Bounds::unbounded()
        };
        let started = Instant::now();
        let err = run(program, &args, &bounds).await.expect_err("deadline");
        assert!(matches!(err, BoundedError::DeadlineExceeded), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "must end at the deadline, not at the program's own end: {:?}",
            started.elapsed()
        );
    }

    /// SUBA-130: the stop token ends the program promptly, long before any deadline.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_stop_token_ends_a_hung_program_promptly() {
        let (program, args) = sleep_argv("30");
        let cancel = CancelToken::new();
        let bounds = Bounds {
            cancel: Some(cancel.clone()),
            deadline: Some(Instant::now() + Duration::from_secs(600)),
            ..Bounds::unbounded()
        };
        let trigger = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });
        let started = Instant::now();
        let err = run(program, &args, &bounds).await.expect_err("aborted");
        trigger.await.unwrap();
        assert!(matches!(err, BoundedError::Aborted), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "stop must be prompt: {:?}",
            started.elapsed()
        );
    }

    /// An already-cancelled token or already-passed deadline never spawns (pi's pre-check).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_exhausted_bound_never_spawns() {
        let marker = tempfile::tempdir().unwrap();
        let file = marker.path().join("ran");
        let args = vec!["-c".to_string(), format!("touch '{}'", file.display())];

        let cancel = CancelToken::new();
        cancel.cancel();
        let stopped = Bounds {
            cancel: Some(cancel),
            ..Bounds::unbounded()
        };
        assert!(matches!(
            run(OsStr::new("sh"), &args, &stopped).await,
            Err(BoundedError::Aborted)
        ));

        let expired = Bounds {
            deadline: Some(Instant::now()),
            ..Bounds::unbounded()
        };
        assert!(matches!(
            run(OsStr::new("sh"), &args, &expired).await,
            Err(BoundedError::DeadlineExceeded)
        ));
        assert!(!file.exists(), "nothing may be spawned past the bound");
    }

    /// SUBA-130: output past the budget kills the program and fails the call — never a silently
    /// truncated result.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_past_the_cap_fails_and_kills() {
        // `yes` would write forever; it only ends because the runner kills it.
        let bounds = Bounds {
            max_bytes: 1024,
            ..Bounds::unbounded()
        };
        let started = Instant::now();
        let err = run(OsStr::new("yes"), &[], &bounds)
            .await
            .expect_err("overflow");
        assert!(
            matches!(err, BoundedError::OutputOverflow { max_bytes: 1024 }),
            "{err:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// Descendants die with the leader: the group, not just the pid, is killed.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_descendant_dies_with_its_leader() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        // The shell forks `sleep` into the background and waits; killing only the shell's pid
        // would orphan the sleeper.
        let args = vec![
            "-c".to_string(),
            format!("sleep 300 & echo $! > '{}'; wait", pid_file.display()),
        ];
        let bounds = Bounds {
            deadline: Some(Instant::now() + Duration::from_millis(800)),
            ..Bounds::unbounded()
        };
        let err = run(OsStr::new("sh"), &args, &bounds)
            .await
            .expect_err("deadline");
        assert!(matches!(err, BoundedError::DeadlineExceeded), "{err:?}");
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("the shell published the sleeper's pid")
            .trim()
            .parse()
            .unwrap();
        // A killed orphan is a zombie until init reaps it (a container's PID 1 may not), so
        // "gone" means absent OR in state `Z`.
        let is_gone = |pid: i32| match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat
                .rsplit(") ")
                .next()
                .is_some_and(|rest| rest.starts_with('Z')),
            Err(_) => true,
        };
        let mut gone = false;
        for _ in 0..100 {
            if is_gone(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            gone,
            "background descendant {pid} must be killed with the group"
        );
    }
}
