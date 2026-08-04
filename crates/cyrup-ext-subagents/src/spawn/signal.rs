//! SIGINT → SIGTERM → SIGKILL kill-escalation state machine (func-SA R-SA-059; arch-SA §6.4).
//!
//! [`terminate`] drives a real, spawned [`tokio::process::Child`] through the exact three-stage
//! signal ladder mandated by R-SA-059: send `SIGINT` immediately, then race `child.wait()`
//! against a ~1000ms grace timer; if the child is still alive, send `SIGTERM` and race a further
//! ~3000-4000ms grace timer; if STILL alive, send `SIGKILL` and unconditionally await `wait()` (a
//! signal a well-behaved OS can never fail to deliver). This is genuine OS-signal escalation
//! against a real child process (arch-SA §1.1 item 3) — never a cooperative in-process token
//! standing in for termination.
//!
//! Each grace-period wait is ALSO raced against the caller's `cyrup_core::CancelToken`
//! (`tokio_util::sync::CancellationToken`) so that an already-cancelled run does not sit out a
//! grace period it no longer needs to honor precisely — a cancellation firing mid-wait advances
//! the state machine to the next escalation stage immediately rather than waiting out the full
//! timer (arch-SA §6.3's rationale for `spawn::signal`, restated here as the concrete mechanism).
//! The token never substitutes for the signal delivery itself: every stage still sends its own
//! real OS signal regardless of whether the token observation or the timer is what ended the
//! wait.
//!
//! Every stage targets the child's process GROUP when the child leads one (which is always the
//! case for a [`crate::spawn::SpawnedChild`], since `SpawnedChild::spawn` sets
//! `Command::process_group(0)`), and only otherwise the bare pid — see [`send_signal`] for why
//! that distinction is load-bearing rather than cosmetic: a pid-only ladder cannot reach the
//! descendants a subagent is itself blocked on, which both defeats stage 1 and orphans the whole
//! subtree at stage 3.
//!
//! On non-Unix targets there is no direct `SIGINT`/`SIGTERM` process-group equivalent; per
//! R-SA-059's own fallback clause and the workspace convention already established by
//! `cyrup_tools::ops::local::terminate_pid`/`send_sigterm_tree`, the "graceful" stages become
//! best-effort no-ops that report `false` (nothing was actually sent) so callers can skip the
//! pointless grace wait, and the final stage falls back to `tokio::process::Child::start_kill`
//! (the platform's closest force-terminate primitive) with the same overall timing shape.
//!
//! This module is defined directly against `tokio::process::Child` and takes no dependency on
//! `crate::spawn::mod`'s not-yet-implemented `SpawnedChild`/`ChildSpawnSpec` types — `spawn/mod.rs`
//! wires `terminate` into `SpawnedChild::terminate` during Phase 3 of this crate's build-out.

use std::time::Duration;

use cyrup_core::CancelToken;
use tokio::process::Child;

/// Grace period after `SIGINT` before escalating to `SIGTERM` (R-SA-059: "~1000ms").
pub const SIGINT_GRACE: Duration = Duration::from_millis(1000);

/// Grace period after `SIGTERM` before escalating to `SIGKILL` (R-SA-059: "~3000-4000ms"; the
/// midpoint of that range is used as the single concrete default).
pub const SIGTERM_GRACE: Duration = Duration::from_millis(3500);

/// Which rung of the escalation ladder [`terminate`] reached before the child was confirmed
/// gone. Exposed so callers/tests can assert the escalation actually walked through the stages
/// it claims to, not merely that the process eventually exited somehow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationStage {
    /// The child exited after `SIGINT` alone, within [`SIGINT_GRACE`].
    Sigint,
    /// The child exited after `SIGTERM`, within [`SIGTERM_GRACE`] of that signal.
    Sigterm,
    /// The child required `SIGKILL` (or the non-Unix force-terminate equivalent) to exit.
    Sigkill,
}

/// The outcome of a full [`terminate`] run: the real, observed OS exit status plus which
/// escalation rung actually confirmed termination.
#[derive(Debug)]
pub struct TerminationOutcome {
    /// The real `wait()`-observed exit status of the child process.
    pub status: std::process::ExitStatus,
    /// The escalation stage that was in force when the child was confirmed gone.
    pub stage: EscalationStage,
}

/// Drive `child` through the SIGINT → SIGTERM → SIGKILL escalation ladder (R-SA-059) and return
/// once the OS process is CONFIRMED gone — never a fire-and-forget signal send.
///
/// Each stage:
/// 1. Sends the stage's signal (best-effort on non-Unix; see module docs).
/// 2. Races `child.wait()` against BOTH the stage's grace-period timer AND `cancel` becoming
///    cancelled — whichever of the three resolves first ends the wait. A grace-timer or
///    cancellation firing first means "still alive, escalate"; `wait()` resolving first means
///    "confirmed gone, done".
///
/// The final `SIGKILL` (or non-Unix force-terminate) stage is unconditional and its `wait()` is
/// NEVER raced against a timer or the cancel token — `SIGKILL` cannot be caught, ignored, or
/// blocked by a conforming OS, so this wait is bounded by the OS's own guarantee, not by this
/// function inventing an artificial timeout that could return before the process is actually
/// gone (which would violate the "confirmed gone" contract every caller of this function relies
/// on, e.g. before treating a worktree or run directory as safe to reuse/clean up).
///
/// # Errors
///
/// Returns an `Err` only if `child.wait()` itself fails at the OS/tokio level (e.g. the child
/// was already reaped out-of-band by something else holding the same pid) — signal SEND failures
/// (most commonly `ESRCH`, the process already exited in the race window between the liveness
/// check and the signal syscall) are deliberately swallowed and treated as "nothing to escalate
/// from", exactly mirroring `cyrup_ext::caps::proc::ProcCaps::kill`'s and
/// `cyrup_tools::ops::local::terminate_pid`'s existing try-and-ignore convention for the same
/// class of benign race.
pub async fn terminate(
    mut child: Child,
    cancel: &CancelToken,
) -> std::io::Result<TerminationOutcome> {
    // Stage 1: SIGINT, raced against SIGINT_GRACE and `cancel`.
    send_sigint(&child);
    if let Some(status) = race_wait(&mut child, SIGINT_GRACE, cancel).await? {
        return Ok(TerminationOutcome {
            status,
            stage: EscalationStage::Sigint,
        });
    }

    // Stage 2: SIGTERM, raced against SIGTERM_GRACE and `cancel`.
    send_sigterm(&child);
    if let Some(status) = race_wait(&mut child, SIGTERM_GRACE, cancel).await? {
        return Ok(TerminationOutcome {
            status,
            stage: EscalationStage::Sigterm,
        });
    }

    // Stage 3: SIGKILL (or the non-Unix force-terminate equivalent) — unconditional, never
    // raced against a timer or `cancel`; `wait()` here is bounded only by the OS's own guarantee
    // that a KILL signal cannot be caught/ignored/blocked.
    send_sigkill(&mut child);
    let status = child.wait().await?;
    Ok(TerminationOutcome {
        status,
        stage: EscalationStage::Sigkill,
    })
}

/// Race `child.wait()` against a grace-period timer and `cancel` becoming cancelled.
///
/// Returns `Ok(Some(status))` if the child was confirmed to exit within the race; `Ok(None)` if
/// either the timer or the cancellation fired first (still alive — the caller must escalate);
/// `Err` only on a genuine `wait()` I/O failure.
async fn race_wait(
    child: &mut Child,
    grace: Duration,
    cancel: &CancelToken,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    tokio::select! {
        biased;
        result = child.wait() => Ok(Some(result?)),
        () = tokio::time::sleep(grace) => Ok(None),
        () = cancel.cancelled() => Ok(None),
    }
}

/// Send `SIGINT` to the child — and to its process group when it leads one, see [`send_signal`]
/// — (R-SA-059 stage 1). Best-effort on non-Unix: there is no portable
/// `SIGINT`-equivalent primitive for an arbitrary child process there, so this is a no-op and the
/// escalation proceeds straight to the (also best-effort) `SIGTERM` stage after paying out its
/// own grace period — a slightly longer overall wait than the Unix path, but never a skipped
/// stage, since [`terminate`]'s caller-facing contract (stage timing) is a floor, not a ceiling.
#[cfg_attr(not(unix), allow(unused_variables))]
fn send_sigint(child: &Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            send_signal(pid, nix::sys::signal::Signal::SIGINT);
        }
    }
}

/// Send `SIGTERM` to the child — and to its process group when it leads one, see [`send_signal`]
/// — (R-SA-059 stage 2). Best-effort on non-Unix, matching [`send_sigint`]'s rationale.
#[cfg_attr(not(unix), allow(unused_variables))]
fn send_sigterm(child: &Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            send_signal(pid, nix::sys::signal::Signal::SIGTERM);
        }
    }
}

/// Force-terminate the child (R-SA-059 stage 3: `SIGKILL` on Unix, `tokio::process::Child::
/// start_kill`'s platform primitive — `TerminateProcess` — on non-Unix). Unlike the two graceful
/// stages above, this one is NOT allowed to be a true no-op on any platform: it is the one
/// escalation rung the whole ladder exists to guarantee eventually fires, so non-Unix falls back
/// to tokio's own portable `start_kill` rather than another best-effort signal send. On Unix this
/// too targets the process group when the child leads one ([`send_signal`]) — a pid-only `SIGKILL`
/// here is what would otherwise leak the child's entire descendant subtree.
fn send_sigkill(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            send_signal(pid, nix::sys::signal::Signal::SIGKILL);
            return;
        }
    }
    // Either non-Unix, or the pid was already unavailable (child already reaped out-of-band) —
    // fall back to tokio's own portable kill primitive. `start_kill` is idempotent against an
    // already-exited child (returns `Ok(())` or a benign `InvalidInput`, per tokio's own docs),
    // so ignoring its result here is deliberate, not a swallowed real failure.
    let _ = child.start_kill();
}

/// Send `signal` to the child via `nix::sys::signal::kill`, swallowing the result.
///
/// # Target selection: the child's process GROUP whenever the child leads one
///
/// [`crate::spawn::SpawnedChild::spawn`] puts every subagent child in its own process group
/// (`Command::process_group(0)`), precisely so the ladder "can target exactly this child **and any
/// of its own descendants**". Signalling only the direct pid does not honor that intent, and the
/// gap is not academic — a subagent child is a re-exec'd `cyrup` binary that spends most of its
/// life blocked in `wait(2)` on a descendant IT spawned (a bash-tool command, `cargo`/`npm`/`git`,
/// a nested subagent). A pid-only `SIGINT` leaves that descendant running, so the child cannot
/// finish its graceful shutdown inside [`SIGINT_GRACE`] and the ladder escalates for no reason;
/// worse, a pid-only stage-3 `SIGKILL` reaps the direct child and ORPHANS the entire subtree into
/// a process group nothing holds a handle to (and which `process_group(0)` has already detached
/// from the terminal's foreground group, so the user's own Ctrl-C cannot reach it either).
///
/// So: if the child is genuinely its own process-group LEADER (`getpgid(pid) == pid`), signal the
/// negated pid — POSIX `kill(-pgid, sig)`, i.e. every member of that group, which by construction
/// is exactly this child plus the descendants it did not deliberately detach. Otherwise (the child
/// shares its caller's group — [`terminate`]'s contract is a bare `tokio::process::Child`, and this
/// module's own tests spawn such children) fall back to the single pid, which is also what upstream
/// `pi-subagents`' `trySignalChild` → `child.kill(sig)` does. Upstream can afford single-pid kills
/// because it never passes `detached`, so its children stay in pi's own group and a terminal signal
/// reaches the whole tree anyway; cyrup's deliberate `process_group(0)` divergence is what makes
/// group-targeting mandatory here.
///
/// Never negate a pid whose group we do not lead: `kill(-pgid, …)` against a group we merely belong
/// to would signal the parent orchestrator (and, in tests, the test runner) as well.
///
/// A send failure (overwhelmingly `ESRCH`: the process already exited in the race between our
/// own liveness assumption and this syscall) must NOT abort the escalation — the next stage's
/// own signal send is itself a no-op against an already-dead pid, and the FINAL `wait()` is what
/// actually confirms termination, never this send. This mirrors `cyrup_ext::caps::proc::
/// ProcCaps::kill`'s and `cyrup_tools::ops::local::terminate_pid`'s identical try-and-ignore
/// convention for the same benign race. `getpgid` failing is treated the same way: fall back to
/// the single pid rather than skipping the send.
#[cfg(unix)]
fn send_signal(pid: u32, signal: nix::sys::signal::Signal) {
    let raw = pid as nix::libc::pid_t;
    let nix_pid = nix::unistd::Pid::from_raw(raw);
    // The pid cannot be recycled out from under this check: `terminate` still owns the
    // `tokio::process::Child`, so an exited child is a zombie holding its pid until `wait()`.
    let target = match nix::unistd::getpgid(Some(nix_pid)) {
        Ok(pgid) if pgid.as_raw() == raw => nix::unistd::Pid::from_raw(-raw),
        _ => nix_pid,
    };
    let _ = nix::sys::signal::kill(target, signal);
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    /// A normal child that traps NOTHING dies to plain `SIGINT` almost immediately — the
    /// escalation must not walk any further than stage 1 in that case, and the OS process must
    /// really be gone afterward (`kill -0` check, not just our own bookkeeping).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_stops_at_sigint_for_a_normal_child() {
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("30");
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("sleep spawns");
        let pid = child.id().expect("live child has a pid");

        let cancel = CancelToken::new();
        let started = tokio::time::Instant::now();
        let outcome = terminate(child, &cancel)
            .await
            .expect("terminate confirms real exit");

        assert_eq!(
            outcome.stage,
            EscalationStage::Sigint,
            "a SIGINT-obeying child (sleep has no trap) must not require escalation past stage 1"
        );
        assert!(
            started.elapsed() < SIGINT_GRACE,
            "a plain SIGINT-obeying child dies well before the SIGINT grace period elapses, got {:?}",
            started.elapsed()
        );
        assert_pid_gone(pid);
    }

    /// The full escalation ladder, exercised deterministically: a shell that traps and ignores
    /// BOTH `SIGINT` and `SIGTERM` cannot be stopped by anything short of `SIGKILL`. Asserts via
    /// elapsed wall-clock time that the full SIGINT-grace, SIGTERM-grace, SIGKILL sequence
    /// genuinely ran (not merely that the process eventually died somehow), and that the OS
    /// process is really gone afterward.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_escalates_through_sigint_and_sigterm_to_sigkill() {
        let marker = tempfile::NamedTempFile::new()
            .expect("real tempfile for the trap-installed marker")
            .into_temp_path();
        let marker_path = marker.to_path_buf();
        std::fs::remove_file(&marker_path).ok();

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            &format!(
                "trap '' INT TERM; touch '{}'; while true; do sleep 1; done",
                marker_path.display()
            ),
        ]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("the signal-trapping shell spawns");
        let pid = child.id().expect("live child has a pid");

        // Poll for the marker file the shell touches immediately AFTER `trap '' INT TERM` takes
        // effect, rather than sleeping a fixed guess — a fixed sleep can be too short under heavy
        // concurrent test-suite CPU contention (the shell may not have reached the `trap` builtin
        // yet), which would let SIGINT race the trap install and kill the child before the trap
        // takes effect, falsely short-circuiting the escalation this test exists to exercise.
        wait_for_marker(&marker_path, Duration::from_secs(10)).await;

        let cancel = CancelToken::new();
        let started = tokio::time::Instant::now();
        let outcome = terminate(child, &cancel)
            .await
            .expect("terminate still confirms real exit via the SIGKILL escalation");

        assert_eq!(
            outcome.stage,
            EscalationStage::Sigkill,
            "a SIGINT-and-SIGTERM-ignoring child must require the full escalation to SIGKILL"
        );
        assert!(
            // Both grace periods (SIGINT_GRACE then SIGTERM_GRACE) genuinely elapsed before the
            // SIGKILL stage fired — not just one, and not a short-circuited near-zero total.
            started.elapsed() >= SIGINT_GRACE + SIGTERM_GRACE,
            "both grace-period legs must be genuinely waited out before escalating to SIGKILL, \
             got {:?} (expected >= {:?})",
            started.elapsed(),
            SIGINT_GRACE + SIGTERM_GRACE
        );
        assert_pid_gone(pid);
    }

    /// A child that obeys `SIGTERM` but ignores `SIGINT` stops at stage 2: the SIGINT grace
    /// period genuinely elapses (the child never reacts to it), but SIGTERM then kills it well
    /// within the SIGTERM grace period — no SIGKILL escalation needed.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_stops_at_sigterm_when_the_child_ignores_only_sigint() {
        let marker = tempfile::NamedTempFile::new()
            .expect("real tempfile for the trap-installed marker")
            .into_temp_path();
        let marker_path = marker.to_path_buf();
        std::fs::remove_file(&marker_path).ok();

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            &format!(
                "trap '' INT; touch '{}'; while true; do sleep 1; done",
                marker_path.display()
            ),
        ]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("the SIGINT-ignoring shell spawns");
        let pid = child.id().expect("live child has a pid");
        wait_for_marker(&marker_path, Duration::from_secs(10)).await;

        let cancel = CancelToken::new();
        let started = tokio::time::Instant::now();
        let outcome = terminate(child, &cancel)
            .await
            .expect("terminate confirms real exit");

        assert_eq!(
            outcome.stage,
            EscalationStage::Sigterm,
            "a SIGINT-ignoring-but-SIGTERM-obeying child must stop at stage 2, no SIGKILL needed"
        );
        assert!(
            started.elapsed() >= SIGINT_GRACE,
            "the SIGINT grace period must be genuinely waited out first, got {:?}",
            started.elapsed()
        );
        assert!(
            started.elapsed() < SIGINT_GRACE + SIGTERM_GRACE,
            "SIGTERM must kill it well within the SIGTERM grace period, no SIGKILL escalation, \
             got {:?}",
            started.elapsed()
        );
        assert_pid_gone(pid);
    }

    /// An already-cancelled `CancelToken` short-circuits every grace-period wait immediately —
    /// the escalation still walks SIGINT -> SIGTERM -> SIGKILL (every stage still sends its real
    /// signal), but none of the timers are paid out in full, so total elapsed time stays far
    /// below what waiting out both real grace periods would cost.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_skips_grace_waits_once_cancelled() {
        let marker = tempfile::NamedTempFile::new()
            .expect("real tempfile for the trap-installed marker")
            .into_temp_path();
        let marker_path = marker.to_path_buf();
        std::fs::remove_file(&marker_path).ok();

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            &format!(
                "trap '' INT TERM; touch '{}'; while true; do sleep 1; done",
                marker_path.display()
            ),
        ]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("the signal-trapping shell spawns");
        let pid = child.id().expect("live child has a pid");
        wait_for_marker(&marker_path, Duration::from_secs(10)).await;

        let cancel = CancelToken::new();
        cancel.cancel();

        let started = tokio::time::Instant::now();
        let outcome = terminate(child, &cancel)
            .await
            .expect("terminate still confirms real exit via SIGKILL");

        assert_eq!(outcome.stage, EscalationStage::Sigkill);
        assert!(
            started.elapsed() < SIGINT_GRACE,
            "an already-cancelled token must short-circuit both grace waits, not pay either out \
             in full: got {:?}",
            started.elapsed()
        );
        assert_pid_gone(pid);
    }

    /// Independently verify at the OS level (never just trust the function's own return value)
    /// that `pid` no longer exists — `kill -0` sends no signal, it only probes existence.
    #[cfg(unix)]
    fn assert_pid_gone(pid: u32) {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "kill -0 on pid {pid} must fail after terminate() returns — the OS process must be \
             really gone, not merely reported as such"
        );
    }

    /// Poll for `path` to exist, up to `timeout`. Used to deterministically confirm a spawned
    /// shell has genuinely reached its `trap` builtin (which the shell scripts in these tests
    /// `touch` a marker file immediately after) before this test sends any signal — replacing a
    /// fixed-duration sleep that can be too short under heavy concurrent test-suite CPU
    /// contention (many other tests in this crate spawn real subprocesses at the same time) and
    /// would otherwise let a signal race the trap install, falsely short-circuiting the
    /// escalation these tests exist to exercise.
    #[cfg(unix)]
    async fn wait_for_marker(path: &std::path::Path, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "marker file {} never appeared within {:?} — the child shell did not reach its trap \
             install in time (system may be extremely overloaded)",
            path.display(),
            timeout
        );
    }
}
