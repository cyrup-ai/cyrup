//! [`LocalProc`] — the default [`ProcOps`] backend over `tokio::process`.
//!
//! `LocalProc` runs commands through the detected shell, streams combined stdout+stderr, and kills
//! on cancel/timeout. The two `ProcOps` methods intentionally use DIFFERENT escalations, 1:1 with
//! their DIFFERENT real Pi consumers:
//! [`LocalProc::exec`] backs both the `bash` tool (`bash.ts:97-99`'s `createLocalBashOperations`,
//! which spawns with `detached: process.platform !== "win32"`) and the immediate-bash RPC seam
//! (`bash-executor.ts:108`'s `executeBashWithOperations`, which calls the SAME `BashOperations`),
//! and both paths' abort/timeout handlers call `killProcessTree` (`shell.ts:200-225`) — an
//! IMMEDIATE `killpg(SIGKILL)` (negated pid, whole process GROUP), no `SIGTERM`, no grace period,
//! ever. [`LocalProc::exec_argv`] backs the WASM `exec` capability grant instead, whose real
//! consumer is `exec.ts:34-63`'s `execCommand`/`killProcess` — spawned with `shell: false` and NO
//! `detached` option, and killed via a bare `proc.kill("SIGTERM")`/`proc.kill("SIGKILL")` (Node's
//! `ChildProcess.kill()` always signals only `this.pid`, never a negated/group pid, regardless of
//! `detached`). So `exec_argv`'s escalation is a `SIGTERM`-then-grace-then-`SIGKILL`
//! **single-pid** signal — the SAME single-pid mechanism `cyrup-ext`'s `proc.rs::kill` already uses
//! for the unrelated long-lived `proc` capability ([`terminate_pid`]/[`kill_pid`], reused directly
//! here) — NOT a process-group kill.

use super::command::{build_argv_command, build_command};
use super::guard::KillTreeOnDrop;
use super::signal::{kill_pid, send_sigkill_tree, terminate_pid};
use crate::error;
use crate::ops::{ArgvOutput, ArgvSpec, ExecSpec, ExitStatus, ProcOps, ShellConfig, Transport};
use cyrup_core::{CancelToken, ToolError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long [`LocalProc::exec_argv`] waits after `SIGTERM` before escalating to `SIGKILL` — Pi's
/// exact `killProcess` timing (`exec.ts:56-61`: `setTimeout(..., 5000)`). Mirrors
/// `cyrup-ext::caps::proc::DEFAULT_KILL_GRACE`. NOT used by [`LocalProc::exec`] (the `bash`
/// tool/immediate-bash backend), which mirrors `killProcessTree`'s immediate, graceless `SIGKILL`
/// instead (`shell.ts:200-225`) — see the module doc comment.
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(5);

/// How long [`LocalProc::exec`]/[`LocalProc::exec_argv`] wait, once the child process ITSELF has
/// exited, for its inherited stdout/stderr pipe(s) to fall idle before giving up on reading them —
/// Pi's exact `EXIT_STDIO_GRACE_MS` (`child-process.ts:16`), armed by `waitForChildProcess`
/// (`child-process.ts:49-137`). A short-lived command that backgrounds a descendant (`sh -c "(sleep
/// 5 &); exit 0"`) leaves that descendant holding our stdout/stderr pipe open long after the
/// process we spawned has exited, so waiting on EOF alone hangs forever — the exact class of hang
/// Pi's grace timer exists to close (earendil-works/pi#5303). The timer is armed the instant the
/// child exits and re-armed on every subsequent data chunk, so an actively-writing descendant keeps
/// us reading, while a quiet inherited handle releases us after this much idle time.
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

/// Local process operations.
pub struct LocalProc {
    shell: ShellConfig,
    /// SIGTERM→SIGKILL grace period; overridable ONLY for tests ([`Self::with_kill_grace`]) so the
    /// escalation path is exercisable without a real test waiting 5+ real seconds — production
    /// always gets Pi's real 5s via [`Self::new`].
    kill_grace: Duration,
}

impl LocalProc {
    pub fn new(shell: ShellConfig) -> Self {
        Self::with_kill_grace(shell, DEFAULT_KILL_GRACE)
    }

    /// Build with a caller-supplied SIGTERM→SIGKILL grace period (tests only).
    pub fn with_kill_grace(shell: ShellConfig, kill_grace: Duration) -> Self {
        Self { shell, kill_grace }
    }
}

fn exit_from(status: std::process::ExitStatus) -> ExitStatus {
    match status.code() {
        Some(code) => ExitStatus::Exited(code),
        // No exit code ⇒ died to a signal we did not send (the cancel/timeout branches break with
        // their own statuses before reaching here). Pi maps this to `exitCode: null` ⇒ success.
        None => ExitStatus::Signaled,
    }
}

/// Read one chunk; `None` on EOF/error (or never resolves when the reader is absent).
async fn read_chunk<R: tokio::io::AsyncRead + Unpin>(reader: &mut Option<R>) -> Option<Vec<u8>> {
    match reader {
        Some(r) => {
            let mut buf = [0u8; 8192];
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => Some(buf.get(..n).unwrap_or(&[]).to_vec()),
            }
        }
        None => std::future::pending().await,
    }
}

#[async_trait::async_trait]
impl ProcOps for LocalProc {
    async fn exec(
        &self,
        mut spec: ExecSpec,
        cancel: CancelToken,
        timeout: Option<Duration>,
        on_data: &mut (dyn for<'a> FnMut(&'a [u8]) + Send),
    ) -> Result<ExitStatus, ToolError> {
        if spec.shell.program.as_os_str().is_empty() {
            spec.shell = self.shell.clone();
        }
        // Pi checks `signal?.aborted` before EVER spawning (bash.ts:86-88: `if (signal?.aborted) {
        // throw new Error("aborted"); }`, ahead of even the cwd check below) — an already-cancelled
        // token must guarantee zero process execution, not just a kill-after-spawn race. Report it
        // as `Ok(Killed)`, the SAME outcome the mid-run cancel branch below reports (bash.ts's outer
        // catch maps BOTH the pre-spawn and the post-spawn `Error("aborted")` to the identical
        // `"Command aborted"` text, bash.ts:407-411) — every caller (`BashTool::execute`,
        // `bash.rs:315`; `run_bash`, `cyrup-session-svc/src/bash.rs:58`) already renders `Killed`
        // correctly, so this needs no new wiring.
        if cancel.is_cancelled() {
            return Ok(ExitStatus::Killed);
        }
        // Pi checks the cwd exists before spawning (bash.ts:70-74) so the model gets an actionable
        // message instead of a raw spawn error.
        if tokio::fs::metadata(&spec.cwd).await.is_err() {
            return Err(error::invalid(format!(
                "Working directory does not exist: {}\nCannot execute bash commands.",
                error::show(&spec.cwd)
            )));
        }
        let stdin_command = if spec.shell.transport == Transport::Stdin {
            Some(spec.command.clone())
        } else {
            None
        };

        let std_cmd = build_command(&spec);
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| error::io(&format!("spawn {}", error::show(&spec.shell.program)), &e))?;
        // Declared AFTER `child` on purpose: locals drop in reverse declaration order, so an
        // abandoned future runs this `killpg` while `child` is still un-reaped and the pid is still
        // ours. See [`KillTreeOnDrop`] for why `kill_on_drop` alone leaves the group behind.
        let mut kill_guard = KillTreeOnDrop::arm(child.id());

        if let Some(command) = stdin_command
            && let Some(mut stdin) = child.stdin.take()
        {
            let _ = stdin.write_all(command.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        let timeout_fut = async {
            match timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(timeout_fut);

        // Immediate, graceless SIGKILL-the-whole-tree escalation — Pi's real `killProcessTree`
        // (`shell.ts:200-225`: `process.kill(-pid, "SIGKILL")`), called unconditionally by BOTH real
        // consumers of this method on abort/timeout: the `bash` tool's `onAbort`/timeout handler
        // (`bash.ts:111-121`) and the immediate-bash RPC seam (`bash-executor.ts:108` calling the
        // SAME `BashOperations.exec`). Unlike [`Self::exec_argv`] (which backs the WASM `exec`
        // capability's DIFFERENT real consumer, `exec.ts:52-63`'s `killProcess`), there is no
        // `SIGTERM`, no grace window, and no escalation step here — `pending` only records which
        // trigger (cancel vs timeout) asked for termination so the eventual `ExitStatus` reports the
        // right reason.
        let mut pending: Option<ExitStatus> = None;

        // Idle-grace fallback (Pi `waitForChildProcess`, `child-process.ts:49-137`): once the child
        // process itself exits, if its stdio isn't already fully drained (a backgrounded descendant
        // may still hold the pipe open), arm a timer that finalizes after `EXIT_STDIO_GRACE` of
        // silence — re-armed on every subsequent chunk — instead of waiting on EOF forever.
        let mut exit_status: Option<ExitStatus> = None;
        let mut idle_armed = false;
        let idle_grace = tokio::time::sleep(EXIT_STDIO_GRACE);
        tokio::pin!(idle_grace);

        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if pending.is_none() => {
                    send_sigkill_tree(&mut child);
                    pending = Some(ExitStatus::Killed);
                }
                _ = &mut timeout_fut, if pending.is_none() => {
                    send_sigkill_tree(&mut child);
                    pending = Some(ExitStatus::TimedOut);
                }
                _ = &mut idle_grace, if idle_armed => {
                    // The child exited and its inherited stdio has been quiet for
                    // `EXIT_STDIO_GRACE` — a still-open pipe held by a backgrounded descendant is
                    // not coming back with more output soon enough to justify hanging forever.
                    break match pending {
                        Some(reason) => reason,
                        None => exit_status.unwrap_or(ExitStatus::Exited(-1)),
                    };
                }
                chunk = read_chunk(&mut stdout) => {
                    match chunk {
                        Some(data) => {
                            on_data(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stdout = None;
                            if let Some(done) = exit_status
                                && stderr.is_none() {
                                    break pending.unwrap_or(done);
                                }
                        }
                    }
                }
                chunk = read_chunk(&mut stderr) => {
                    match chunk {
                        Some(data) => {
                            on_data(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stderr = None;
                            if let Some(done) = exit_status
                                && stdout.is_none() {
                                    break pending.unwrap_or(done);
                                }
                        }
                    }
                }
                s = child.wait(), if exit_status.is_none() => {
                    let mapped = match s {
                        Ok(st) => exit_from(st),
                        Err(_) => ExitStatus::Exited(-1),
                    };
                    exit_status = Some(mapped);
                    if stdout.is_none() && stderr.is_none() {
                        break pending.unwrap_or(mapped);
                    }
                    idle_armed = true;
                    idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                }
            }
        };
        // Every break above is reached only after `child.wait()` has been observed (`exit_status`
        // is `Some` on the EOF breaks, and `idle_armed` — the only gate on the idle-grace break —
        // is set in the `child.wait()` arm itself), so the child is reaped here and the group must
        // not be signalled again.
        kill_guard.disarm();
        Ok(status)
    }

    async fn exec_argv(
        &self,
        spec: ArgvSpec,
        cancel: CancelToken,
        timeout: Option<Duration>,
    ) -> Result<ArgvOutput, ToolError> {
        let std_cmd = build_argv_command(&spec);
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);
        // Pi `execCommand` never rejects on a bad command — its caller maps a spawn failure to
        // `{code:1}` (exec.ts:99-105). Here we surface the spawn error and let the grant layer
        // (`LiveHostServices::exec`) apply that Pi mapping.
        let mut child = cmd
            .spawn()
            .map_err(|e| error::io(&format!("spawn {}", spec.program), &e))?;

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        // Buffer each stream separately (Pi accumulates `stdout`/`stderr` strings, exec.ts:47-48,81-87).
        let mut out_buf: Vec<u8> = Vec::new();
        let mut err_buf: Vec<u8> = Vec::new();

        let timeout_fut = async {
            match timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(timeout_fut);

        // SIGTERM-then-grace-then-SIGKILL escalation (Pi `killProcess`, `exec.ts:52-63`) — see
        // `LocalProc::exec`'s identical loop above for the full rationale.
        let mut pending: Option<ExitStatus> = None;
        let grace = tokio::time::sleep(self.kill_grace);
        tokio::pin!(grace);
        // Guards the grace arm below from re-firing on every subsequent loop iteration: a
        // `tokio::time::Sleep` that has already elapsed keeps reporting `Ready` on every re-poll
        // until it is `.reset()` to a future deadline, and (unlike the cancel/timeout arms above)
        // the grace arm below deliberately does NOT reset `grace` — it needs to fire exactly once.
        let mut sigkill_sent = false;

        // Idle-grace fallback (Pi `waitForChildProcess`, `child-process.ts:49-137`) — see
        // `LocalProc::exec`'s identical block above for the full rationale.
        let mut exit_status: Option<ExitStatus> = None;
        let mut idle_armed = false;
        let idle_grace = tokio::time::sleep(EXIT_STDIO_GRACE);
        tokio::pin!(idle_grace);

        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if pending.is_none() => {
                    // SINGLE-PID SIGTERM (Pi's bare `proc.kill("SIGTERM")`, `exec.ts:55` — NEVER a
                    // negated/group pid; `exec.ts`'s spawn never sets `detached`, so there IS no
                    // group to target here, unlike `LocalProc::exec`'s group `send_sigkill_tree`).
                    // Skip the grace-period wait entirely when nothing was actually sent (non-unix,
                    // or the pid is already gone) — waiting it out has zero chance of a graceful
                    // exit landing, mirroring `ProcCaps::kill`'s identical fix for this bug class
                    // (`cyrup-ext/src/caps/proc.rs:748-751`, commit `0790ace`) — indeed the SAME
                    // `terminate_pid` call, reused directly rather than merely mirrored.
                    let sigterm_sent = child.id().is_some_and(|pid| terminate_pid(pid).unwrap_or(false));
                    pending = Some(ExitStatus::Killed);
                    let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };
                    grace.as_mut().reset(tokio::time::Instant::now() + wait);
                }
                _ = &mut timeout_fut, if pending.is_none() => {
                    let sigterm_sent = child.id().is_some_and(|pid| terminate_pid(pid).unwrap_or(false));
                    pending = Some(ExitStatus::TimedOut);
                    let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };
                    grace.as_mut().reset(tokio::time::Instant::now() + wait);
                }
                _ = &mut grace, if pending.is_some() && !sigkill_sent => {
                    // Grace elapsed (or was skipped outright, above) with NO natural exit yet (a
                    // graceful mid-grace exit already broke the loop via the `child.wait()`/drain
                    // arms below, which keep the REAL code). Force it — but, unlike a `break` here,
                    // KEEP DRAINING: the persistent `child.wait()` arm below still captures the REAL
                    // post-SIGKILL status (Pi: SIGKILL ⇒ `exit(null)` ⇒ `code ?? 0`, matched by
                    // `exit_from`'s no-code-⇒-`Signaled` mapping), and the `read_chunk` arms keep
                    // pumping whatever bytes the child already wrote into the pipe before it died.
                    // Breaking immediately here (the previous behavior) silently dropped exactly
                    // that trailing output — bytes already sitting in the kernel pipe buffer at the
                    // instant of SIGKILL are still readable via the still-open read end even after
                    // the writer is gone, but only if something keeps calling `read_chunk` — mirrors
                    // `LocalProc::exec`'s cancel/timeout arms above, which never `break` either.
                    // SINGLE-PID SIGKILL (Pi's bare `proc.kill("SIGKILL")`, `exec.ts:59` — same
                    // never-a-group-pid rationale as the SIGTERM arms above).
                    sigkill_sent = true;
                    if let Some(pid) = child.id() {
                        let _ = kill_pid(pid);
                    }
                }
                _ = &mut idle_grace, if idle_armed => {
                    // `idle_armed` is only set after `child.wait()` already captured the real
                    // status below, so `exit_status` is always `Some` here — Pi's `killed` never
                    // masks the real code (`child-process.ts:73-80`).
                    break exit_status.unwrap_or(ExitStatus::Exited(-1));
                }
                chunk = read_chunk(&mut stdout) => {
                    match chunk {
                        Some(data) => {
                            out_buf.extend_from_slice(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stdout = None;
                            if let Some(done) = exit_status
                                && stderr.is_none() {
                                    break done;
                                }
                        }
                    }
                }
                chunk = read_chunk(&mut stderr) => {
                    match chunk {
                        Some(data) => {
                            err_buf.extend_from_slice(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stderr = None;
                            if let Some(done) = exit_status
                                && stdout.is_none() {
                                    break done;
                                }
                        }
                    }
                }
                s = child.wait(), if exit_status.is_none() => {
                    let mapped = match s {
                        Ok(st) => exit_from(st),
                        Err(_) => ExitStatus::Exited(-1),
                    };
                    exit_status = Some(mapped);
                    if stdout.is_none() && stderr.is_none() {
                        break mapped;
                    }
                    idle_armed = true;
                    idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                }
            }
        };
        // `killed` (Pi's `killed` local, exec.ts:49) is set the instant a SIGTERM/SIGKILL
        // escalation is INITIATED — orthogonal to `status`, which always carries the REAL observed
        // code/signal outcome above (see `ArgvOutput` doc comment).
        Ok(ArgvOutput {
            status,
            stdout: out_buf,
            stderr: err_buf,
            killed: pending.is_some(),
        })
    }
}
