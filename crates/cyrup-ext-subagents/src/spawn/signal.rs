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
//! [`terminate_on_timeout`] is the same module's *timeout* counterpart: `SIGTERM`, then a hard
//! `SIGKILL` [`TIMEOUT_SIGTERM_GRACE`] later, matching upstream `abortVerification`
//! (`pi-subagents/src/runs/shared/acceptance.ts:742-758` @v0.34.0) and the kill-on-expiry
//! semantics of `runWorktreeSetupHook`'s `spawnSync(…, { timeout })`
//! (`pi-subagents/src/runs/shared/worktree.ts:323-329`). Both entry points share the one
//! group-aware [`send_signal`] target-selection rule below; there is deliberately no second
//! signalling path in this crate.
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
//! `cyrup_tools::ops::local::terminate_pid`, the "graceful" stages become best-effort no-ops that
//! report `false` (nothing was actually sent) so [`terminate`] skips the pointless grace wait —
//! sitting out a stage's full grace period waiting for a reaction to a signal that was never sent
//! is pure latency with a zero chance of paying off. The final stage is NOT allowed the same
//! excuse: it is the rung the whole ladder exists to guarantee, so non-Unix runs upstream's own
//! win32 tree kill — `taskkill /F /T /PID <pid>`, the `/T` being the flag that reaches descendants
//! (pi `killProcessTree`, `packages/coding-agent/src/utils/shell.ts:200-212` @v0.83.0) — before
//! falling back to `tokio::process::Child::start_kill`. `start_kill` ALONE would be a silent
//! divergence, not a graceful degradation: it is `TerminateProcess` against the direct pid only,
//! where the Unix arm deliberately goes through [`send_signal`] → `kill(-pgid, SIGKILL)` against
//! the whole process group precisely so the child's descendants die with it.
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

/// The two inter-rung grace periods [`terminate`] pays out, as one injectable bundle.
///
/// Production always uses [`EscalationGraces::default`], i.e. the R-SA-059 constants above; nothing
/// outside tests ever supplies a different value, and [`terminate`] itself is unchanged.
///
/// It exists because the *stage* a run reached is a real behavioural assertion (a SIGINT-obeying
/// child must not be escalated to SIGTERM) while the grace period it was measured against is a
/// WALL-CLOCK race against the OS reaping that child. On a machine loaded enough to preempt the
/// waiter for a full second — routine when this crate's own suite is spawning dozens of real
/// subprocesses in parallel — the child dies to `SIGINT` exactly as intended, but `child.wait()`
/// does not get scheduled inside [`SIGINT_GRACE`], the ladder escalates, and a correct
/// implementation reports [`EscalationStage::Sigterm`]. Injecting a generous grace lets a test keep
/// the meaningful assertion (which rung ended it) without betting it on the scheduler; the
/// production constants stay exactly where R-SA-059 puts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscalationGraces {
    /// How long stage 1 waits after `SIGINT` before escalating to `SIGTERM`.
    pub sigint: Duration,
    /// How long stage 2 waits after `SIGTERM` before escalating to `SIGKILL`.
    pub sigterm: Duration,
}

impl Default for EscalationGraces {
    fn default() -> Self {
        Self { sigint: SIGINT_GRACE, sigterm: SIGTERM_GRACE }
    }
}

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
    /// SUBA-023 — the NAME of the signal that killed the child (`"SIGKILL"`, `"SIGTERM"`, …), or
    /// `None` when it exited normally.
    ///
    /// `ExitStatus::signal()` yields a bare number, which is what made signal attribution in run
    /// results coarse ("failed" rather than "killed by SIGKILL") and escalation-ladder debugging
    /// harder than it needs to be. The mapping is [`signal_name`]; it is derived from the observed
    /// status rather than from [`Self::stage`] deliberately — a child can die of a signal nobody in
    /// this ladder sent (an external `kill`, an OOM kill, a `SIGSEGV`), and reporting the rung we
    /// happened to be on would misattribute exactly the cases worth debugging.
    pub signal_name: Option<&'static str>,
}

/// SUBA-023 — map a raw Unix signal number to its POSIX name.
///
/// Covers the signals a subagent child can realistically die of: this ladder's own three, the
/// terminal/job-control set a user's Ctrl-C or shell can deliver, and the fault signals that mean
/// "the child crashed" rather than "someone stopped it". An unrecognized number returns `None`
/// rather than a fabricated name — a wrong name is worse than a number.
#[must_use]
pub fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        // 17/19/23 and 18/20 differ across platforms; only the portable Linux/macOS-agreeing
        // members of the job-control set are named here.
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        _ => return None,
    })
}

/// SUBA-023 — the signal name for an observed exit status, or `None` for a normal exit.
///
/// On non-Unix there is no signal concept at all, so this is always `None` — which is exactly what
/// a `TerminationOutcome` should report there.
#[must_use]
pub fn signal_name_of(status: &std::process::ExitStatus) -> Option<&'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal().and_then(signal_name)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
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
    child: Child,
    cancel: &CancelToken,
) -> std::io::Result<TerminationOutcome> {
    terminate_with_graces(child, cancel, EscalationGraces::default()).await
}

/// [`terminate`] with the two inter-rung grace periods supplied explicitly — see
/// [`EscalationGraces`] for why that knob exists and why production never turns it.
///
/// The escalation itself is byte-identical to [`terminate`]'s: same signals, same order, same
/// cancellation racing, same unconditional final `SIGKILL` whose `wait()` is never timed out.
///
/// # Errors
///
/// Identical to [`terminate`]'s: `Err` only from a genuine `child.wait()` I/O failure.
pub async fn terminate_with_graces(
    mut child: Child,
    cancel: &CancelToken,
    graces: EscalationGraces,
) -> std::io::Result<TerminationOutcome> {
    // Stage 1: SIGINT, raced against the SIGINT grace and `cancel`. The grace is skipped outright
    // when nothing was actually sent (non-Unix, or the child was already reaped out-of-band so it
    // has no pid left to signal) — see [`send_sigint`]: waiting out a grace period for a reaction
    // to a signal that was never delivered cannot succeed, it only delays the rung that can.
    let sigint_grace = grace_if_sent(send_sigint(&child), graces.sigint);
    if let Some(status) = race_wait(&mut child, sigint_grace, cancel).await? {
        return Ok(TerminationOutcome {
            signal_name: signal_name_of(&status),
            status,
            stage: EscalationStage::Sigint,
        });
    }

    // Stage 2: SIGTERM, raced against the SIGTERM grace and `cancel` — same skip-if-nothing-was-sent
    // rule as stage 1.
    let sigterm_grace = grace_if_sent(send_sigterm(&child), graces.sigterm);
    if let Some(status) = race_wait(&mut child, sigterm_grace, cancel).await? {
        return Ok(TerminationOutcome {
            signal_name: signal_name_of(&status),
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
        signal_name: signal_name_of(&status),
        status,
        stage: EscalationStage::Sigkill,
    })
}

/// Grace period between the timeout-triggered `SIGTERM` and the unconditional hard `SIGKILL` that
/// follows it — upstream `abortVerification`
/// (`pi-subagents/src/runs/shared/acceptance.ts:742-758` @v0.34.0) sends `child.kill("SIGTERM")`
/// and immediately arms a `setTimeout(… , 1000)` that sends `child.kill("SIGKILL")`, so 1000ms is
/// the ported constant, not a cyrup invention.
pub const TIMEOUT_SIGTERM_GRACE: Duration = Duration::from_millis(1000);

/// Hard-stop a child that blew through its OWN timeout: `SIGTERM`, then `SIGKILL` after
/// [`TIMEOUT_SIGTERM_GRACE`], returning only once the OS process is CONFIRMED reaped.
///
/// This is the two-rung sibling of [`terminate`]'s three-rung ladder, and it exists because
/// upstream draws the same distinction: a *cancellation* walks the polite
/// `SIGINT -> SIGTERM -> SIGKILL` escalation, but a *timeout* is already the caller's declared
/// patience running out, so upstream's `abortVerification` skips straight to `SIGTERM` + a 1s hard
/// `SIGKILL` (`acceptance.ts:742-758`), and `runWorktreeSetupHook`'s `spawnSync(…, { timeout })`
/// (`worktree.ts:323-329`) likewise kills on expiry rather than escalating gently.
///
/// Callers MUST still own the `Child` when they call this — the bug this function exists to make
/// unrepresentable is racing `tokio::time::timeout(…, child.wait_with_output())`, whose
/// `self`-consuming future swallows the only handle to the process, so the timeout arm drops it
/// and (with no `kill_on_drop`) leaves the whole process group running for the machine's uptime.
/// Take the child's pipes out and drain them separately, keep the `Child` binding, race
/// `child.wait()`, and call this on expiry.
///
/// Signal delivery goes through [`send_signal`], so a child that leads its own process group
/// (every `verify[]` command does — `exec::acceptance` sets `Command::process_group(0)`) is
/// signalled as `kill(-pgid, …)`, reaching the descendants it spawned. A child that does not lead
/// a group (the worktree setup hook) gets the bare pid, exactly as upstream's `spawnSync` timeout
/// does.
///
/// # Errors
///
/// Returns `Err` only if `child.wait()` itself fails at the OS/tokio level. Signal-send failures
/// (`ESRCH` for a process that exited in the race window) are swallowed, per [`send_signal`].
pub async fn terminate_on_timeout(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    terminate_on_timeout_with_grace(child, TIMEOUT_SIGTERM_GRACE).await
}

/// [`terminate_on_timeout`] with the hard-kill grace supplied explicitly. Production always passes
/// [`TIMEOUT_SIGTERM_GRACE`]; the parameter exists for the same reason [`EscalationGraces`] does —
/// so a test can prove WHICH rung ended the child without racing the OS's reaping against a
/// one-second wall clock.
///
/// # Errors
///
/// Identical to [`terminate_on_timeout`]'s.
pub async fn terminate_on_timeout_with_grace(
    child: &mut Child,
    grace: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    // Rung 1: SIGTERM, raced against the hard-kill timer only — a timeout path has no grace period
    // left to honor beyond the one upstream itself arms, and none at all when the SIGTERM was never
    // actually sent (non-Unix; see [`send_sigterm`]).
    let grace = grace_if_sent(send_sigterm(child), grace);
    tokio::select! {
        biased;
        result = child.wait() => return result,
        () = tokio::time::sleep(grace) => {}
    }

    // Rung 2: SIGKILL — unconditional, and its `wait()` is never raced, so this function's
    // "confirmed gone" contract is bounded by the OS's own guarantee rather than another timer.
    send_sigkill(child);
    child.wait().await
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

/// The grace period a stage should actually wait, given whether that stage's signal was really
/// sent: the stage's own grace when it was, and [`Duration::ZERO`] when it was not.
///
/// A stage that sent nothing has nothing to wait for — no signal was delivered, so no reaction can
/// arrive, and the wait is pure latency in front of the rung that CAN end the child. This is the
/// same rule (and the same `Ok(false)` "nothing was sent" convention it keys off) that
/// `cyrup_tools::ops::local::terminate_pid` states for its own callers, which gate their grace wait
/// on the returned bool exactly this way (`ops/local.rs:898,904`).
///
/// The `wait()` itself is still raced even against a ZERO grace rather than skipped outright: a
/// child that has ALREADY exited must be reported at the rung it actually died on, and `race_wait`
/// is `biased` with `child.wait()` first, so a ready exit status wins over a zero-length timer.
const fn grace_if_sent(sent: bool, grace: Duration) -> Duration {
    if sent { grace } else { Duration::ZERO }
}

/// Send `SIGINT` to the child — and to its process group when it leads one, see [`send_signal`]
/// — (R-SA-059 stage 1). Returns whether a REAL signal was actually sent.
///
/// `false` means nothing was delivered at all: either this is a non-Unix target (there is no
/// portable `SIGINT`-equivalent primitive for an arbitrary child process there — Windows'
/// `GenerateConsoleCtrlEvent` reaches only console groups this detached child is not in), or the
/// child had already been reaped out-of-band and has no pid left to signal. [`terminate`] gates
/// this stage's grace period on that bool ([`grace_if_sent`]): paying out a full ~1000ms waiting
/// for a reaction to a signal nobody sent is latency with a zero chance of paying off, which on
/// Windows previously cost two of the ladder's three rungs in pure delay.
fn send_sigint(child: &Child) -> bool {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            send_signal(pid, nix::sys::signal::Signal::SIGINT);
            return true;
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = child;
        false
    }
}

/// Send `SIGTERM` to the child — and to its process group when it leads one, see [`send_signal`]
/// — (R-SA-059 stage 2). Returns whether a REAL signal was actually sent, with exactly
/// [`send_sigint`]'s meaning and exactly its grace-skipping consequence.
fn send_sigterm(child: &Child) -> bool {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            send_signal(pid, nix::sys::signal::Signal::SIGTERM);
            return true;
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = child;
        false
    }
}

/// Force-terminate the child and, with it, its whole descendant subtree (R-SA-059 stage 3:
/// `SIGKILL` to the process group on Unix, `taskkill /F /T /PID` on non-Unix). Unlike the two
/// graceful stages above, this one is NOT allowed to be a no-op — or a narrower kill — on any
/// platform: it is the one escalation rung the whole ladder exists to guarantee eventually fires,
/// and the SUBTREE is the thing it must reach.
///
/// On Unix that means [`send_signal`]'s `kill(-pgid, SIGKILL)` whenever the child leads its own
/// group, which a [`crate::spawn::SpawnedChild`] always does. On non-Unix it means upstream's own
/// win32 tree kill — pi `killProcessTree` runs
/// `spawn("taskkill", ["/F", "/T", "/PID", String(pid)], { stdio: "ignore", detached: true,
/// windowsHide: true })` (`packages/coding-agent/src/utils/shell.ts:200-212` @v0.83.0), the `/T`
/// being precisely the tree flag — fire-and-forget, exactly as upstream leaves it, since the
/// `child.wait()` in [`terminate_with_graces`] is what actually confirms the death.
///
/// `tokio::process::Child::start_kill` still runs afterward as the backstop, but it is NOT the
/// tree kill and must never be mistaken for one: it is `TerminateProcess` against the DIRECT pid
/// only, so on its own it reaps the re-exec'd `cyrup` child and orphans everything that child
/// spawned (the bash-tool command, `cargo`/`npm`/`git`, any nested subagent) into processes
/// nothing holds a handle to — inside a worktree the caller is about to treat as safe to clean up.
/// It is also the whole story on the Unix path when the pid is already gone.
pub(crate) fn send_sigkill(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            send_signal(pid, nix::sys::signal::Signal::SIGKILL);
            return;
        }
    }
    #[cfg(not(unix))]
    {
        if let Some(pid) = child.id() {
            // Fire-and-forget, mirroring upstream's `spawn(...)` (never a blocking `output()`):
            // the confirmation of death is the caller's own `child.wait()`, not this command's
            // exit code, and `taskkill` failing (the tree already gone) is the benign, expected
            // race — the same one the Unix arm swallows as `ESRCH`.
            let _ = std::process::Command::new("taskkill")
                .args(win32_tree_kill_argv(pid))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
    // Either non-Unix (where the tree kill above has already gone out and this is the direct-pid
    // backstop), or the pid was already unavailable (child already reaped out-of-band) — fall back
    // to tokio's own portable kill primitive. `start_kill` is idempotent against an already-exited
    // child (returns `Ok(())` or a benign `InvalidInput`, per tokio's own docs), so ignoring its
    // result here is deliberate, not a swallowed real failure.
    let _ = child.start_kill();
}

/// The exact argv pi's `killProcessTree` passes to `taskkill` on win32
/// (`packages/coding-agent/src/utils/shell.ts:204` @v0.83.0:
/// `["/F", "/T", "/PID", String(pid)]`), and the in-workspace twin of
/// `cyrup_tools::ops::local::kill_process_tree`'s `not(unix)` arm (`ops/local.rs:458-459`).
///
/// `/T` is the load-bearing flag and the reason this is a named function rather than an inline
/// array: it is what makes the kill reach the child's DESCENDANTS, which is the entire behavioural
/// difference between stage 3 and a bare `TerminateProcess`. Compiled on Unix too — but only under
/// `cfg(test)` — so the ported literal is pinned by a test on every platform, not just the one
/// platform that runs it.
#[cfg(any(not(unix), test))]
fn win32_tree_kill_argv(pid: u32) -> [String; 4] {
    ["/F".to_string(), "/T".to_string(), "/PID".to_string(), pid.to_string()]
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

    /// SUBA-023 — signal-name attribution on [`TerminationOutcome`].
    ///
    /// THE USER ACTION: a subagent run dies and the run record says "failed". Before the fix
    /// `TerminationOutcome` carried only `status` + `stage` and there was no `ExitStatus::signal()`
    /// name mapping anywhere in the module, so escalation-ladder debugging had a bare number at
    /// best — and the run surface had nothing at all.
    ///
    /// The mapping is derived from the OBSERVED status rather than from the rung reached, because a
    /// child can die of a signal this ladder never sent (external `kill`, OOM, `SIGSEGV`) and
    /// reporting the rung would misattribute exactly the cases worth debugging.
    #[test]
    fn signal_numbers_map_to_their_posix_names() {
        assert_eq!(signal_name(2), Some("SIGINT"));
        assert_eq!(signal_name(9), Some("SIGKILL"));
        assert_eq!(signal_name(15), Some("SIGTERM"));
        assert_eq!(signal_name(11), Some("SIGSEGV"));
        assert_eq!(signal_name(6), Some("SIGABRT"));
        // An unrecognized number reports nothing rather than a fabricated name.
        assert_eq!(signal_name(64), None);
        assert_eq!(signal_name(0), None);
    }

    /// The status-side half, exercised against REAL exit statuses rather than a constructed one:
    /// a normally-exiting child reports no signal, and a `SIGKILL`ed one reports `SIGKILL`.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_killed_child_reports_its_signal_name_and_a_clean_exit_reports_none() {
        use std::os::unix::process::ExitStatusExt as _;

        // Clean exit: no signal.
        let clean = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .status()
            .expect("sh runs");
        assert_eq!(signal_name_of(&clean), None, "a normal exit names no signal");

        // Signalled exit, constructed from the same representation `wait()` produces.
        let killed = std::process::ExitStatus::from_raw(9);
        assert_eq!(signal_name_of(&killed), Some("SIGKILL"));
        let termed = std::process::ExitStatus::from_raw(15);
        assert_eq!(signal_name_of(&termed), Some("SIGTERM"));
    }

    /// Production pins: the injectable [`EscalationGraces`] must not become a place where the
    /// R-SA-059 constants quietly drift. Every non-test call goes through
    /// [`EscalationGraces::default`], so this is what keeps the real ladder at 1000ms/3500ms while
    /// the tests below deliberately run it at other values.
    #[test]
    fn the_default_graces_are_exactly_the_r_sa_059_constants() {
        let graces = EscalationGraces::default();
        assert_eq!(graces.sigint, SIGINT_GRACE);
        assert_eq!(graces.sigterm, SIGTERM_GRACE);
        assert_eq!(SIGINT_GRACE, Duration::from_millis(1000));
        assert_eq!(SIGTERM_GRACE, Duration::from_millis(3500));
        assert_eq!(TIMEOUT_SIGTERM_GRACE, Duration::from_millis(1000));
    }

    /// A generous stage-1 grace for the tests whose claim is "this child dies at rung 1", so the
    /// claim is not silently also a bet that the OS reaps it inside the production 1000ms.
    const GENEROUS: Duration = Duration::from_secs(30);

    /// The two graceful rungs must REPORT whether they actually sent anything, and the ladder must
    /// gate that rung's grace period on the answer.
    ///
    /// Before the fix `send_sigint`/`send_sigterm` had a `#[cfg(unix)]` body and NO `not(unix)`
    /// counterpart at all, so on Windows they compiled to empty functions that returned `()` —
    /// indistinguishable, at the call site, from a signal that really went out. `terminate` then
    /// paid out the full 1000ms and 3500ms grace periods waiting for a reaction to two signals that
    /// were never sent: 4.5s of pure latency in front of the only rung that can end the child
    /// there. The bool is what makes "nothing was sent" observable, and [`grace_if_sent`] is what
    /// acts on it.
    ///
    /// Both arms are asserted: on Unix a live child really is signalled (`true`); on non-Unix
    /// nothing is sent (`false`). The already-reaped case is the one that is `false` on EVERY
    /// platform — no pid, nothing to signal — so it exercises the skip wiring on this host too.
    #[tokio::test]
    async fn graceful_rungs_report_whether_a_signal_was_really_sent_and_gate_their_grace_on_it() {
        // A genuinely long-lived child on either platform. `timeout.exe` is deliberately NOT used
        // for the non-Unix leg: it refuses to run at all with stdin redirected, which would make
        // the "live child" half of this test silently assert against an already-exited process.
        let mut cmd = if cfg!(unix) {
            let mut cmd = tokio::process::Command::new("sleep");
            cmd.arg("30");
            cmd
        } else {
            let mut cmd = tokio::process::Command::new("ping");
            cmd.args(["-n", "31", "127.0.0.1"]);
            cmd
        };
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().expect("the sleeper spawns");

        // A LIVE child: Unix delivers a real signal, non-Unix has no equivalent to deliver.
        assert_eq!(
            send_sigint(&child),
            cfg!(unix),
            "a live child is really signalled on Unix and really is not on non-Unix — and the \
             ladder must be told which"
        );
        assert_eq!(send_sigterm(&child), cfg!(unix));

        // ...and the grace period follows the report, on whichever platform this runs.
        assert_eq!(
            grace_if_sent(send_sigint(&child), GENEROUS),
            if cfg!(unix) { GENEROUS } else { Duration::ZERO },
            "a rung that sent nothing must not wait for a reaction to it"
        );

        // An already-reaped child has no pid to signal on ANY platform: nothing is sent, so no
        // grace may be paid out. This half of the assertion runs everywhere.
        send_sigkill(&mut child);
        let _ = child.wait().await.expect("the sleeper is reaped");
        assert!(child.id().is_none(), "a reaped child has no pid left to signal");
        assert!(!send_sigint(&child), "nothing can be sent to a reaped child");
        assert!(!send_sigterm(&child), "nothing can be sent to a reaped child");
        assert_eq!(grace_if_sent(send_sigint(&child), GENEROUS), Duration::ZERO);
        assert_eq!(grace_if_sent(send_sigterm(&child), GENEROUS), Duration::ZERO);
    }

    /// Stage 3 must kill the SUBTREE on every platform, not just on Unix.
    ///
    /// The Unix arm goes through [`send_signal`] → `kill(-pgid, SIGKILL)` specifically so the
    /// child's descendants die with it. Before the fix the non-Unix arm was `child.start_kill()`
    /// alone — `TerminateProcess` against the direct pid — which reaps the re-exec'd `cyrup` child
    /// and ORPHANS everything it spawned into the worktree the caller is about to clean up. This
    /// pins the ported upstream remedy: pi `killProcessTree` runs `taskkill /F /T /PID <pid>`
    /// (`shell.ts:200-212` @v0.83.0), and `/T` — the flag whose absence IS the bug — is the whole
    /// point of the argv.
    ///
    /// The argv is compiled (and therefore pinned) on Unix as well as non-Unix, so this assertion
    /// is not silently skipped on the platform where the suite actually runs.
    #[test]
    fn stage_three_targets_the_whole_tree_on_non_unix_via_pis_own_taskkill_argv() {
        let argv = win32_tree_kill_argv(4242);
        assert_eq!(
            argv,
            ["/F".to_string(), "/T".to_string(), "/PID".to_string(), "4242".to_string()],
            "stage 3's non-Unix argv is pi `killProcessTree`'s literal, `/T` (tree) included"
        );
        assert!(
            argv.contains(&"/T".to_string()),
            "dropping /T turns stage 3 back into a direct-pid kill that orphans the subtree"
        );
    }

    /// [`grace_if_sent`]'s own contract, in isolation: a sent signal keeps its full grace, an
    /// unsent one gets none. Mirrors `cyrup_tools::ops::local::terminate_pid`'s `Ok(false)`
    /// convention and its callers' `if sent { grace } else { Duration::ZERO }` gate.
    #[test]
    fn an_unsent_signal_earns_no_grace_period() {
        assert_eq!(grace_if_sent(true, SIGINT_GRACE), SIGINT_GRACE);
        assert_eq!(grace_if_sent(true, SIGTERM_GRACE), SIGTERM_GRACE);
        assert_eq!(grace_if_sent(false, SIGINT_GRACE), Duration::ZERO);
        assert_eq!(grace_if_sent(false, SIGTERM_GRACE), Duration::ZERO);
        assert_eq!(grace_if_sent(false, TIMEOUT_SIGTERM_GRACE), Duration::ZERO);
    }

    /// A normal child that traps NOTHING dies to plain `SIGINT` almost immediately — the
    /// escalation must not walk any further than stage 1 in that case, and the OS process must
    /// really be gone afterward (`kill -0` check, not just our own bookkeeping).
    ///
    /// Run against a deliberately generous [`EscalationGraces::sigint`]: the assertion that matters
    /// is WHICH RUNG ended the child, and measuring that against the 1000ms production constant
    /// turns it into a wall-clock race with process reaping that a loaded machine loses (observed:
    /// escalation to `Sigterm` after exactly 1.01s, with SIGINT having worked perfectly).
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
        let graces = EscalationGraces { sigint: GENEROUS, ..EscalationGraces::default() };
        let started = tokio::time::Instant::now();
        let outcome = terminate_with_graces(child, &cancel, graces)
            .await
            .expect("terminate confirms real exit");

        assert_eq!(
            outcome.stage,
            EscalationStage::Sigint,
            "a SIGINT-obeying child (sleep has no trap) must not require escalation past stage 1"
        );
        assert!(
            started.elapsed() < graces.sigint,
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
        // Both graces must genuinely elapse here (the child ignores both signals), so they are
        // injected SHORT — a lower bound is load-robust in a way an upper bound is not, and the
        // production constants would cost this test 4.5s of pure sleeping for no extra proof.
        let graces =
            EscalationGraces { sigint: Duration::from_millis(200), sigterm: Duration::from_millis(300) };
        let started = tokio::time::Instant::now();
        let outcome = terminate_with_graces(child, &cancel, graces)
            .await
            .expect("terminate still confirms real exit via the SIGKILL escalation");

        assert_eq!(
            outcome.stage,
            EscalationStage::Sigkill,
            "a SIGINT-and-SIGTERM-ignoring child must require the full escalation to SIGKILL"
        );
        assert!(
            // BOTH grace periods genuinely elapsed before the SIGKILL stage fired — not just one,
            // and not a short-circuited near-zero total.
            started.elapsed() >= graces.sigint + graces.sigterm,
            "both grace-period legs must be genuinely waited out before escalating to SIGKILL, \
             got {:?} (expected >= {:?})",
            started.elapsed(),
            graces.sigint + graces.sigterm
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
        // Stage 1's grace is SHORT here because this test needs it to genuinely elapse (the child
        // ignores SIGINT), and stage 2's is GENEROUS because the claim under test is that SIGTERM
        // is what ended it — which must not double as a bet on reaping beating a 3500ms clock.
        let graces = EscalationGraces { sigint: Duration::from_millis(200), sigterm: GENEROUS };
        let started = tokio::time::Instant::now();
        let outcome = terminate_with_graces(child, &cancel, graces)
            .await
            .expect("terminate confirms real exit");

        assert_eq!(
            outcome.stage,
            EscalationStage::Sigterm,
            "a SIGINT-ignoring-but-SIGTERM-obeying child must stop at stage 2, no SIGKILL needed"
        );
        assert!(
            started.elapsed() >= graces.sigint,
            "the SIGINT grace period must be genuinely waited out first, got {:?}",
            started.elapsed()
        );
        assert!(
            started.elapsed() < graces.sigint + graces.sigterm,
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

        // Both graces are injected GENEROUS precisely so the "short-circuited" claim is provable
        // without a tight wall-clock bound: paying out even one of these legs would take 30s, so
        // returning in far less proves the cancellation skipped them — an assertion that holds no
        // matter how badly loaded the machine is, unlike a `< 1000ms` bound on a real process kill.
        let graces = EscalationGraces { sigint: GENEROUS, sigterm: GENEROUS };
        let started = tokio::time::Instant::now();
        let outcome = terminate_with_graces(child, &cancel, graces)
            .await
            .expect("terminate still confirms real exit via SIGKILL");

        assert_eq!(outcome.stage, EscalationStage::Sigkill);
        assert!(
            started.elapsed() < graces.sigint,
            "an already-cancelled token must short-circuit both grace waits, not pay either out \
             in full: got {:?}",
            started.elapsed()
        );
        assert_pid_gone(pid);
    }

    /// SUBA-027: the timeout path's own two-rung ladder. A child that does not trap anything dies
    /// to the first `SIGTERM`, well inside [`TIMEOUT_SIGTERM_GRACE`] — the hard-kill timer must
    /// not be paid out for a cooperative child.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_on_timeout_stops_at_sigterm_for_a_normal_child() {
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("30");
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().expect("sleep spawns");
        let pid = child.id().expect("live child has a pid");

        // Generous grace for the same reason as the SIGINT rung above: the claim is "SIGTERM alone
        // ended it", not "the OS reaps it inside one second".
        let started = tokio::time::Instant::now();
        terminate_on_timeout_with_grace(&mut child, GENEROUS)
            .await
            .expect("terminate_on_timeout confirms a real exit");

        assert!(
            started.elapsed() < GENEROUS,
            "a SIGTERM-obeying child must not cost the hard-kill grace period, got {:?}",
            started.elapsed()
        );
        assert_pid_gone(pid);
    }

    /// SUBA-027: a child that traps and ignores `SIGTERM` still dies, via the unconditional
    /// `SIGKILL` rung armed [`TIMEOUT_SIGTERM_GRACE`] later — this is the rung whose absence let a
    /// hung `verify[]` command survive its own timeout indefinitely.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_on_timeout_escalates_to_sigkill_when_sigterm_is_ignored() {
        let marker = tempfile::NamedTempFile::new()
            .expect("real tempfile for the trap-installed marker")
            .into_temp_path();
        let marker_path = marker.to_path_buf();
        std::fs::remove_file(&marker_path).ok();

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            &format!(
                "trap '' TERM; touch '{}'; while true; do sleep 1; done",
                marker_path.display()
            ),
        ]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().expect("the SIGTERM-trapping shell spawns");
        let pid = child.id().expect("live child has a pid");
        wait_for_marker(&marker_path, Duration::from_secs(10)).await;

        // A SHORT grace here: this test's assertion is a LOWER bound (the grace was genuinely paid
        // out before SIGKILL), which stays true under any load.
        let grace = Duration::from_millis(200);
        let started = tokio::time::Instant::now();
        terminate_on_timeout_with_grace(&mut child, grace)
            .await
            .expect("terminate_on_timeout still confirms a real exit via SIGKILL");

        assert!(
            started.elapsed() >= grace,
            "the hard-kill grace period must be genuinely waited out before SIGKILL, got {:?}",
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
