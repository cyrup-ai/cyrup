//! The lattice gate's `verify[]` runner: REAL subprocess execution and the pipe draining that
//! keeps a daemonizing command from holding the gate open.

use std::path::Path;

use super::contract::VerifyCommand;

// ============================================================================================
// R-SA-032 / DI-SA-5: verify[] REAL subprocess execution
// ============================================================================================

/// R-SA-032 / DI-SA-5 (MUST) — actually execute every command in `commands`, IN ORDER, as a real
/// OS subprocess each, observing each one's real exit code. Returns one
/// [`crate::exec::acceptance::model::AcceptanceVerifyResult`] per command, always in the same order as `commands` — this
/// function does NOT short-circuit on the first failure (a caller wants to see every command's real
/// outcome for a rejected run's detail text, not just the first one that failed), but callers
/// deciding overall pass/fail MUST require that no result
/// [`rejects`](crate::exec::acceptance::model::AcceptanceVerifyResult::rejects) (see [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`]) — NOT that
/// every result exited 0, since a declared `allowFailure: true` command that exits nonzero is
/// [`crate::exec::acceptance::model::VerifyRunStatus::AllowedFailure`] and still must not reject the run
/// (`acceptance.ts:1193,1297`).
///
/// # This is a THIN LOOP over the one runner, not a second one
///
/// Upstream has exactly ONE verify runner (`runMemoizedVerifyCommand` -> `runVerifyCommand`,
/// `pi-subagents/src/runs/shared/acceptance.ts:1072-1208` @v0.43.0) producing exactly ONE result
/// type (`AcceptanceVerifyResult`, `shared/types.ts:736-758`), and BOTH of its live gates
/// (`runs/foreground/execution.ts:1696-1706`, `runs/background/subagent-runner.ts:1628-1640`) go
/// through it. This crate used to carry a SECOND transcription of that runner here, writing a
/// second result type (`VerifyCommandResult`) that had no field for any of upstream's memoization
/// evidence — so `artifactPath`/`cacheKey`/`memoized`/`envKeys`/`envHash`/`workspaceState` were
/// dropped outright on the live foreground path and `artifactError` was downgraded to a
/// `tracing::debug!`. Both paths now go through [`crate::exec::acceptance::model::run_memoized_verify_command`]; this
/// function is the sequential loop upstream writes inline at `acceptance.ts:1288-1296`.
///
/// `default_cwd` is the run-level working directory: used verbatim for a command declaring no
/// `cwd`, and as the base a relative declared `cwd` resolves against (`acceptance.ts:1078`).
pub async fn run_verify_commands(
    commands: &[VerifyCommand],
    default_cwd: &Path,
) -> Vec<crate::exec::acceptance::model::AcceptanceVerifyResult> {
    run_verify_commands_memoized(commands, default_cwd, None).await
}

/// G80 — [`run_verify_commands`] with upstream's per-workspace MEMOIZATION armed
/// (`runMemoizedVerifyCommand`, `pi-subagents/src/runs/shared/acceptance.ts:1072-1132` @v0.43.0).
///
/// This is the live foreground gate's entry point: pi calls `evaluateAcceptance({ …, artifactsDir,
/// runId })` for a single run (`runs/foreground/execution.ts:1704-1705`) and for every background
/// step (`runs/background/subagent-runner.ts:1638-1639`), and those are the two call sites whose
/// verify results are memoized. `memo: None` reproduces the un-memoized behavior exactly — no
/// artifact is read, none is written, every command executes, and none of the seven evidence fields
/// is stamped — which is also what pi's chain group gate does (`chain-execution.ts:1037-1046`
/// passes neither field).
///
/// A memo HIT replays the recorded `exit_code`/`status`/`stdout`/`stderr`/`duration_ms` without
/// spawning anything; the cache is keyed on the command's text, its resolved repo-relative cwd, its
/// declared env key names, a hash of the whole effective environment, its timeout, its
/// `allow_failure` flag, the repository `HEAD` and a hash of the full working-tree diff
/// (`acceptance.ts:1091-1101`). Any edit anywhere in the tree therefore invalidates every memo,
/// which is what makes replaying a `cargo test` result safe.
pub async fn run_verify_commands_memoized(
    commands: &[VerifyCommand],
    default_cwd: &Path,
    memo: Option<crate::exec::acceptance::model::VerifyMemoContext<'_>>,
) -> Vec<crate::exec::acceptance::model::AcceptanceVerifyResult> {
    run_verify_commands_memoized_with_cancel(
        commands,
        default_cwd,
        memo,
        &cyrup_core::CancelToken::new(),
    )
    .await
}

/// SUBA-028 — [`run_verify_commands_memoized`] with the caller's cancellation token, pi's
/// `input.signal` (`acceptance.ts:1290`).
///
/// The break is placed exactly where upstream puts it (`:1295`): AFTER the command's own result is
/// pushed, not before the command runs. That ordering is not cosmetic — the running command's own
/// abort (below) produces a `timed-out` result that upstream records, so checking first would
/// silently drop the evidence of what was interrupted.
pub async fn run_verify_commands_memoized_with_cancel(
    commands: &[VerifyCommand],
    default_cwd: &Path,
    memo: Option<crate::exec::acceptance::model::VerifyMemoContext<'_>>,
    cancel: &cyrup_core::CancelToken,
) -> Vec<crate::exec::acceptance::model::AcceptanceVerifyResult> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        results.push(
            crate::exec::acceptance::model::run_memoized_verify_command_with_cancel(command, default_cwd, memo, cancel)
                .await,
        );
        // pi `if (input.signal?.aborted) break;` (`acceptance.ts:1295`).
        if cancel.is_cancelled() {
            break;
        }
    }
    results
}


/// Read one of a child's piped streams to EOF on its own task, so neither stream can fill its
/// kernel pipe buffer and deadlock the `child.wait()` the timeout races against.
pub(crate) fn spawn_pipe_drain<R>(mut pipe: R) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt as _;
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf).await;
        buf
    })
}

/// Collect a [`spawn_pipe_drain`] task's bytes, treating an absent pipe or a join failure as
/// "no output" — output capture is diagnostic detail here, never a reason to fail a command whose
/// real exit code was already observed.
async fn drained(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Collect BOTH [`spawn_pipe_drain`] tasks, but never past `deadline` — returning `None` (and
/// aborting both tasks, releasing this process's read ends) when the deadline passes first.
///
/// This is the single bound that keeps a verify command's own `timeoutMs` honest, used by the one
/// runner ([`crate::exec::acceptance::model::run_verify_command`]).
///
/// # Why an unbounded collect is a hang, not a slow path
///
/// `spawn_pipe_drain` reads to EOF, and a pipe reaches EOF only when the LAST write end closes —
/// including the copies every descendant inherited. `child.wait()` returns as soon as the DIRECT
/// child exits, so a routine `verify[]` entry like `./server &`, `npm run dev &` or any script that
/// daemonises leaves the write end held for the descendant's whole lifetime. Awaiting the drain
/// tasks after `wait()` with no bound therefore blocks `run_verify_commands` (which loops these
/// sequentially, with no outer timeout) forever, silently — worse than the abandoned-child bug
/// SUBA-027 fixed.
///
/// # Why the deadline is absolute rather than a fresh post-`wait()` grace
///
/// Upstream arms ONE `setTimeout(abortVerification, timeoutMs)` at spawn
/// (`pi-subagents/src/runs/shared/acceptance.ts:759` @v0.34.0) and settles on Node's `"close"`
/// event, which — exactly like `read_to_end` — waits for every stdio stream to close. When a
/// descendant holds them open, upstream's `"close"` never fires and `abortVerification`'s
/// `hardKill` `finish({status: "timed-out", …})` (`:742-758`) is what resolves the promise, 1000 ms
/// after the deadline. So upstream reports such a command as TIMED OUT at `timeoutMs + 1000ms`
/// regardless of the exit code it already observed, and this port does the same rather than
/// inventing a separate, shorter grace with a different verdict.
pub(crate) async fn drained_by(
    deadline: tokio::time::Instant,
    stdout_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let aborts: Vec<tokio::task::AbortHandle> = [stdout_task.as_ref(), stderr_task.as_ref()]
        .into_iter()
        .flatten()
        .map(tokio::task::JoinHandle::abort_handle)
        .collect();
    let collect = async move {
        let out = drained(stdout_task).await;
        let err = drained(stderr_task).await;
        (out, err)
    };
    match tokio::time::timeout_at(deadline, collect).await {
        Ok(pair) => Some(pair),
        Err(_elapsed) => {
            for handle in aborts {
                handle.abort();
            }
            None
        }
    }
}

/// Build the platform shell invocation for one `verify[]` command string.
pub(crate) fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::lattice::testsupport::passed;
    use crate::exec::acceptance::lattice::testsupport::vc;
    use crate::exec::acceptance::lattice::testsupport::vc_timeout;
    use std::time::Duration;


    /// SUBA-028 — the loop-level half (pi `if (input.signal?.aborted) break;`,
    /// `acceptance.ts:1295`): once cancelled, the REMAINING verify commands do not run at all.
    ///
    /// The break is post-push, so the interrupted command's own result survives — asserted here,
    /// because dropping it would hide what was interrupted.
    #[tokio::test]
    async fn a_cancelled_verify_sequence_records_the_aborted_command_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("second-ran");
        let cancel = cyrup_core::CancelToken::new();
        cancel.cancel();

        let results = run_verify_commands_memoized_with_cancel(
            &[
                vc_timeout("sleep 300", Duration::from_secs(30)),
                VerifyCommand::shell(&format!("touch {}", marker.display())),
            ],
            dir.path(),
            None,
            &cancel,
        )
        .await;

        assert_eq!(results.len(), 1, "only the interrupted command is recorded: {results:?}");
        assert_eq!(results[0].status, crate::exec::acceptance::model::VerifyRunStatus::TimedOut, "{results:?}");
        assert!(
            !marker.exists(),
            "the second command must never have run after the abort"
        );
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_verify_commands_executes_every_command_in_order_and_never_short_circuits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let commands = vec![
            vc("exit 1"), // fails
            vc("exit 0"), // still runs, passes
        ];
        let results = run_verify_commands(&commands, dir.path()).await;
        assert_eq!(results.len(), 2, "both commands must run even though the first failed");
        assert!(!passed(&results[0]));
        assert!(passed(&results[1]));
    }

}
