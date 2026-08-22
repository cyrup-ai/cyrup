//! `runVerifyCommand` (pi `acceptance.ts:1134-1208`) — REAL subprocess execution of a declared
//! `verify[]` command, observing its actual exit code.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::super::aggregate::trim_output;
use super::super::types::{AcceptanceVerifyCommand, AcceptanceVerifyResult, VerifyRunStatus};
use super::super::verify::redact::redact_verify_env;

// --------------------------------------------------------------------------------------------
// runVerifyCommand (acceptance.ts:1134-1208) — REAL subprocess execution
// --------------------------------------------------------------------------------------------

/// `DEFAULT_VERIFY_TIMEOUT_MS` (`acceptance.ts:1032`): the bound applied to a `verify[]`
/// command that declares no `timeoutMs` of its own (`command.timeoutMs ?? 120_000`,
/// `acceptance.ts:1090,1179`). A declared `timeoutMs` wins; this only keeps a hung *undeclared*
/// verification command from blocking the acceptance gate — and therefore the whole run —
/// indefinitely.
pub const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 120_000;

/// `"Acceptance verification timed out."` (`acceptance.ts:1174,1203`) — upstream's last-resort
/// `stderr` for a killed verify command, used when the command printed nothing to stderr of its
/// own and no `abortMessage` was supplied.
pub const VERIFY_TIMED_OUT_MESSAGE: &str = "Acceptance verification timed out.";

/// The `stderr` for the one timeout shape upstream has no counterpart for: the command EXITED,
/// but something it backgrounded still holds its stdout/stderr, so the capture could not reach
/// EOF before the deadline. Upstream reaches the same `timed-out` verdict here (its `"close"`
/// event likewise never fires) but cannot distinguish the case; naming it keeps a genuinely
/// confusing failure diagnosable.
pub const VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE: &str =
    "Acceptance verification timed out: the command exited, but a process it backgrounded \
         still holds its stdout/stderr.";

/// `command.cwd ? path.resolve(defaultCwd, command.cwd) : defaultCwd`
/// (`acceptance.ts:1078,1137`) — `path.resolve` returns an absolute segment verbatim and joins
/// a relative one onto the base, which is what `Path::join` does.
#[must_use]
pub(crate) fn resolve_verify_cwd(command: &AcceptanceVerifyCommand, default_cwd: &Path) -> PathBuf {
    match command.cwd.as_deref() {
        Some(rel) => {
            let path = Path::new(rel);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                default_cwd.join(path)
            }
        }
        Option::None => default_cwd.to_path_buf(),
    }
}

/// `runVerifyCommand` (`acceptance.ts:1134-1208`): execute one `verify[]` command as a REAL
/// shell subprocess, observing its real exit code — never the child's own claim about it.
///
/// **G80 — every captured stream leaves this function REDACTED.** Upstream wraps each of
/// `stdout`/`stderr` in `redactVerifyEnv(…, command.env)` before `trimOutput`
/// (`acceptance.ts:1173-1174,1194-1195,1203-1204`), and so does this. The output of a verify
/// command is attacker-adjacent by construction — it is whatever `cargo test`/`curl`/a build
/// script printed, running with the orchestrator's full environment — and it lands verbatim in
/// the acceptance ledger, which lands in a transcript. Redacting before trimming (not after) is
/// also upstream's order and matters: a secret straddling the 12 000-char truncation point must
/// be masked while it is still whole.
pub async fn run_verify_command(
    command: &AcceptanceVerifyCommand,
    default_cwd: &Path,
) -> AcceptanceVerifyResult {
    run_verify_command_with_cancel(command, default_cwd, &cyrup_core::CancelToken::new()).await
}

/// SUBA-028 — [`run_verify_command`] with the caller's cancellation token: pi's
/// `options.signal` (`acceptance.ts:1134`), whose listener is the SAME `abortVerification`
/// the per-command timeout fires (`:1180-1181`: `if (options.signal?.aborted)
/// abortVerification(); else addEventListener("abort", abortVerification, { once: true })`).
///
/// Reproducing that identity is the whole design here: the cancellation arm below is the
/// timeout arm — same group SIGTERM→SIGKILL escalation, same bounded output drain, same
/// `timed-out` result — because upstream has literally one abort function. An
/// ALREADY-cancelled token therefore aborts the command immediately after spawn, which is
/// upstream's `signal.aborted` branch, not a separate early return.
pub async fn run_verify_command_with_cancel(
    command: &AcceptanceVerifyCommand,
    default_cwd: &Path,
    cancel: &cyrup_core::CancelToken,
) -> AcceptanceVerifyResult {
    let started = Instant::now();
    let cwd: PathBuf = resolve_verify_cwd(command, default_cwd);
    let mut cmd = crate::exec::acceptance::lattice::verify::shell_command(&command.command);
    cmd.current_dir(&cwd);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if let Some(env) = &command.env {
        for (key, value) in env {
            cmd.env(key, value);
        }
    }
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let cwd_str = Some(cwd.display().to_string());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            // `child.on("error", …)` (`acceptance.ts:1198-1205`) — the error TEXT is redacted
            // too, because a spawn failure echoes the command line back (`sh: -c: …`) and a
            // verify command may legitimately carry a credential in its own argv.
            return AcceptanceVerifyResult::unmemoized(
                command,
                cwd_str,
                Some(1),
                if command.allow_failure == Some(true) {
                    VerifyRunStatus::AllowedFailure
                } else {
                    VerifyRunStatus::Failed
                },
                Option::None,
                Some(redact_verify_env(&err.to_string(), command.env.as_ref())),
                started.elapsed().as_millis(),
            );
        }
    };

    let timeout = Duration::from_millis(command.timeout_ms.unwrap_or(DEFAULT_VERIFY_TIMEOUT_MS));

    // Never race a `self`-consuming `wait_with_output()` against the timeout: the elapsed arm
    // would drop the only `Child` handle and abandon a live process group. Drain the pipes on
    // their own tasks, keep the `Child`, and kill on expiry (`abortVerification`,
    // `acceptance.ts:1164-1178`).
    let stdout_task = child.stdout.take().map(crate::exec::acceptance::lattice::verify::spawn_pipe_drain);
    let stderr_task = child.stderr.take().map(crate::exec::acceptance::lattice::verify::spawn_pipe_drain);

    // ONE absolute deadline over exit AND output collection — see `crate::exec::acceptance::lattice::verify::drained_by` for why
    // the post-`wait()` drain must be inside it (upstream `acceptance.ts:742-759`).
    let deadline = tokio::time::Instant::now() + timeout;

    // `biased;` is load-bearing (and is why the JS→Rust shapes differ): an unbiased `select!`
    // whose exit arm and cancel arm are BOTH ready picks at random, so a command that had
    // already finished when the token was cancelled would report `timed-out` about half the
    // time. Upstream cannot express that race at all — `finish` sets `settled` and
    // `abortVerification` returns early on it, so a completed command is never re-reported as
    // aborted. Polling the exit first reproduces that ordering.
    let waited = tokio::select! {
        biased;
        result = child.wait() => Some(result),
        () = tokio::time::sleep_until(deadline) => None,
        // SUBA-028 / pi's `options.signal` listener (`acceptance.ts:1180-1181`) — the same
        // `abortVerification` the timeout arm above fires, hence the same `None`.
        () = cancel.cancelled() => None,
    };

    let Some(waited) = waited else {
        // `abortVerification` (`acceptance.ts:1164-1178`): SIGTERM, then a hard SIGKILL a
        // second later, targeting the command's own process GROUP (it leads one — see
        // `process_group(0)` above), so the descendants the command spawned die with it.
        // `terminate_on_timeout` returns only once the process is CONFIRMED reaped.
        let _ = crate::spawn::signal::terminate_on_timeout(&mut child).await;
        // Upstream's timeout `finish(...)` still reports the output collected SO FAR
        // (`acceptance.ts:1173-1174`) — its `stdout`/`stderr` accumulators are the same ones
        // the `"close"` arm would have used. Collect the drains rather than discarding them,
        // but keep them bounded: a descendant that escaped the process group can still hold the
        // pipes open, and this arm must never be the thing that hangs.
        let (out_bytes, err_bytes) = crate::exec::acceptance::lattice::verify::drained_by(
            tokio::time::Instant::now() + crate::spawn::signal::TIMEOUT_SIGTERM_GRACE,
            stdout_task,
            stderr_task,
        )
        .await
        .unwrap_or_default();
        return AcceptanceVerifyResult::unmemoized(
            command,
            cwd_str,
            Option::None,
            VerifyRunStatus::TimedOut,
            trim_output_after(&out_bytes, command.env.as_ref()),
            // `stderr || options.abortMessage || "Acceptance verification timed out."`
            // (`acceptance.ts:1174`) — JS `||` treats an EMPTY captured stderr as absent, which
            // `trim_output_after` already spells as `None`. This crate threads no abort
            // signal/message into the runner (`input.signal`/`input.abortMessage` are unported),
            // so the literal fallback is the one that applies.
            trim_output_after(&err_bytes, command.env.as_ref())
                .or_else(|| Some(VERIFY_TIMED_OUT_MESSAGE.to_string())),
            started.elapsed().as_millis(),
        );
    };

    // A command that EXITED while a descendant still holds its pipes is reported TIMED OUT at
    // the deadline, never awaited unbounded — upstream reaches the same verdict by the same
    // route (its `"close"` event never fires either, so only `abortVerification` resolves the
    // promise); see `crate::exec::acceptance::lattice::verify::drained_by`.
    let Some((out_bytes, err_bytes)) = crate::exec::acceptance::lattice::verify::drained_by(
        deadline + crate::spawn::signal::TIMEOUT_SIGTERM_GRACE,
        stdout_task,
        stderr_task,
    )
    .await
    else {
        return AcceptanceVerifyResult::unmemoized(
            command,
            cwd_str,
            Option::None,
            VerifyRunStatus::TimedOut,
            Option::None,
            Some(VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE.to_string()),
            started.elapsed().as_millis(),
        );
    };

    match waited {
        Ok(status_code) => {
            let exit_code = status_code.code();
            let passed = exit_code == Some(0);
            let status = if passed {
                VerifyRunStatus::Passed
            } else if command.allow_failure == Some(true) {
                VerifyRunStatus::AllowedFailure
            } else {
                VerifyRunStatus::Failed
            };
            // `trimOutput(redactVerifyEnv(stdout, command.env))` / same for stderr
            // (`acceptance.ts:1194-1195`) — redact FIRST, trim second.
            AcceptanceVerifyResult::unmemoized(
                command,
                cwd_str,
                exit_code,
                status,
                trim_output_after(&out_bytes, command.env.as_ref()),
                trim_output_after(&err_bytes, command.env.as_ref()),
                started.elapsed().as_millis(),
            )
        }
        Err(err) => AcceptanceVerifyResult::unmemoized(
            command,
            cwd_str,
            Some(1),
            if command.allow_failure == Some(true) {
                VerifyRunStatus::AllowedFailure
            } else {
                VerifyRunStatus::Failed
            },
            Option::None,
            Some(redact_verify_env(&err.to_string(), command.env.as_ref())),
            started.elapsed().as_millis(),
        ),
    }
}

/// `trimOutput(redactVerifyEnv(<captured bytes>, env))` (`acceptance.ts:1194-1195`) as one
/// step, in upstream's order: the raw capture is decoded, REDACTED whole, and only then
/// trimmed/truncated. Doing it the other way round would let the 12 000-char truncation split
/// a secret and smuggle its prefix through.
#[must_use]
fn trim_output_after(
    captured: &[u8],
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<String> {
    let decoded = String::from_utf8_lossy(captured);
    trim_output(&redact_verify_env(&decoded, env))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::testsupport::temp_dir;


    // ---- runVerifyCommand (acceptance.ts:713-767) — the SECOND copy of the runner ----

    /// SUBA-027 regression, mirror of
    /// `super::super::tests::a_verify_command_that_daemonizes_still_returns_within_its_timeout`.
    ///
    /// This module is the second copy of the verify runner, so it carried the identical
    /// unbounded post-`wait()` drain and the identical hang. Both copies now share
    /// [`crate::exec::acceptance::lattice::verify::drained_by`], and this test is what keeps them from drifting apart
    /// again: a command that exits 0 while a backgrounded descendant still holds its
    /// stdout/stderr must settle as [`VerifyRunStatus::TimedOut`] on its own deadline, exactly
    /// as upstream's `abortVerification` → `finish({status: "timed-out", …})` does
    /// (`acceptance.ts:742-759` @v0.34.0).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_daemonizing_verify_command_times_out_instead_of_hanging_the_model_copy() {
        let dir = temp_dir();
        let command = AcceptanceVerifyCommand {
            id: "daemonizes".into(),
            command: "sleep 300 & echo $! > descendant; exit 0".into(),
            timeout_ms: Some(200),
            cwd: Option::None,
            env: Option::None,
            allow_failure: Option::None,
        };

        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            run_verify_command(&command, dir.path()),
        )
        .await
        .expect(
            "run_verify_command must honor timeoutMs even when a backgrounded grandchild \
                 still holds the stdout/stderr pipe",
        );

        assert_eq!(
            result.status,
            VerifyRunStatus::TimedOut,
            "upstream resolves this shape through abortVerification's finish(), i.e. as \
                 timed-out, never as a pass"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the call must return on its own deadline, not the descendant's lifetime, got {:?}",
            started.elapsed()
        );

        // Clean up the deliberately-daemonised descendant this test created.
        let pid_path = dir.path().join("descendant");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(raw) = std::fs::read_to_string(&pid_path)
                && let Ok(pid) = raw.trim().parse::<u32>()
            {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

}
