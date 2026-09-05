//! Per-mode signal handling (arch-11 §5; R-11-010/018) — a literal port of pi's three
//! `registerSignalHandlers` sites.
//!
//! pi registers **`SIGTERM`, plus `SIGHUP` off Windows** in every host, and the handler body differs
//! only in what "shut down" means for that host:
//!
//! * `modes/print-mode.ts:48-64` — `killTrackedDetachedChildren()`, then
//!   `disposeRuntime().finally(() => process.exit(signal === "SIGHUP" ? 129 : 143))`. The FIRST
//!   delivery exits the process.
//! * `modes/rpc/rpc-mode.ts:365-379` → `shutdown(signal === "SIGHUP" ? 129 : 143, signal)`
//!   (`:723-740`): dispose the runtime host, detach stdin, and `process.exit(exitCode)`. Again the
//!   first delivery exits — and the re-entrancy guard at `:723-726`
//!   (`if (shuttingDown) process.exit(exitCode)`) is what a SECOND delivery hits while the first
//!   `await runtimeHost.dispose()` is still pending: a hard exit that skips teardown entirely.
//! * `modes/interactive/interactive-mode.ts:3648-3667` → `shutdown({ fromSignal: true })`
//!   (`:3559-3580`): dispose the runtime, drain input, stop the TUI, `process.exit(0)`. The exit is
//!   the *loop's* teardown, not the handler's — which is exactly cyrup's shape, where firing the
//!   [`CancelToken`] breaks `App::run`, `main` disposes the runtime and returns 0.
//!
//! So: interactive is driven through the cancel token (its teardown owns the terminal restore and
//! must not race a `process::exit` from this task), and every non-interactive host disposes and
//! exits from the handler itself with pi's code. SIGINT is cyrup-only — see [`ShutdownSignal`].
//!
//! # `killTrackedDetachedChildren` — SEAM-S03, PORTED
//!
//! All three pi handlers open with `killTrackedDetachedChildren()` as their FIRST statement, before
//! any dispose/shutdown: `modes/print-mode.ts:55`, `modes/rpc/rpc-mode.ts:373`,
//! `modes/interactive/interactive-mode.ts:3663` @v0.83.0. It drains a process-global registry —
//! `const trackedDetachedChildPids = new Set<number>()`, `utils/shell.ts:180`, with
//! `trackDetachedChildPid`/`untrackDetachedChildPid`/`killTrackedDetachedChildren` at `:182-195` —
//! and `killProcessTree`s every pid in it (`shell.ts:200-225`: on unix a
//! `process.kill(-pid, "SIGKILL")`, i.e. the whole process GROUP, falling back to the bare pid). The
//! registry is filled by the bash tool at spawn (`core/tools/bash.ts:108`,
//! `if (child.pid) trackDetachedChildPid(child.pid);`, right beside its
//! `detached: process.platform !== "win32"`) and drained in that spawn's `finally` (`bash.ts:142`) —
//! so at signal time it holds exactly the bash children still running.
//!
//! cyrup mirrors that end to end. The registry is `TRACKED_DETACHED_CHILD_PIDS` in
//! `crates/cyrup-tools/src/ops/local/tracking.rs`, a sibling module of the `setsid` and `killpg`
//! primitives it needs; `LocalProc::exec` enrolls its shell at spawn and — this is the
//! JS→Rust half — unenrolls it from `KillTreeOnDrop::drop`, not from a statement after the
//! `select!` loop, because an abandoned future never reaches that statement and would leak the pid
//! for the life of the process. [`kill_tracked_detached_children`] is called below as the first act
//! of the handler, before the abort/dispose sequence, and again as the first act of the repeat
//! watcher.
//!
//! **Why the repeat watcher needs its own call, when pi's repeat path has none.** pi's second
//! delivery is a bare `process.exit(exitCode)` behind the `shuttingDown` guard (`rpc-mode.ts:723`),
//! and that is safe upstream precisely because its FIRST delivery already SIGKILLed every tracked
//! group synchronously. cyrup's repeat is a hard `process::exit` on the interactive host too, where
//! pi's is inert (`if (this.isShuttingDown) return`, `interactive-mode.ts:3560`) — the CYRUP-DELTA
//! [`spawn_abort_on_signal`] already documents. Since the interactive run loop keeps executing after
//! the first delivery, a bash child spawned in that window would be orphaned by cyrup's escalation
//! and not by pi's. Draining again closes a hole cyrup's own delta opened rather than adding
//! behaviour pi lacks.
//!
//! What this replaces is worth recording, because it is still the fallback on every non-signal path:
//! `LocalProc::exec` `setsid`s its shell into its own process group and `killpg(SIGKILL)`s that
//! group from a `cancel.cancelled()` select arm, where `cancel` is
//! `self.session_cancel.child_token()`, and `runtime.dispose()` ends in `session_cancel.cancel()`
//! (`cyrup-session-svc/src/session.rs`, inside `dispose_with`). That route is LATE (after the whole
//! `session_shutdown` fanout) and scoped to the CURRENT session, where the registry is drained first
//! and is process-global — so it also covers groups left by a session that `/new`, `/fork`,
//! `switchSession` or `reload` has already replaced.
//!
//! RESIDUAL — pi's interactive host also drains from two EMERGENCY paths: `emergencyTerminalExit`
//! (`interactive-mode.ts:3605`) and its `process.on("uncaughtException")` handler (`:3631`). cyrup
//! has no analog of either site — no panic hook and no emergency terminal-restore path exists in
//! this crate at all — so there is nothing to add the call to from here. Those are a `cyrup-tui` /
//! `main.rs` concern; the drain they would need is now exported and ready.

use std::sync::Arc;

use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{AgentSessionRuntime, AppMode, flush_session_writes};
use cyrup_tools::kill_tracked_detached_children;

/// The runtime a spawned watcher tears down, re-read **at signal time** instead of captured at
/// spawn time.
///
/// SEAM-059's rule — *"dereference the CURRENT session, never the startup `Arc`"* — held inside
/// one runtime (`/new`, `/fork`, `switchSession` and `reload` all replace the session behind an
/// `AgentSessionRuntime`, which is why the watcher goes through `runtime.session().await`). It did
/// **not** hold across runtimes, and the ACP front-end is the host that has more than one: its
/// `AcpHost::runtime_ready` fires on every `session/new` and every `session/load`, so arming a
/// watcher per runtime left N watchers racing one `std::process::exit` on a single SIGTERM. The
/// stale one's runtime is already disposed, so its `dispose()` returns immediately and it can exit
/// the process while the LIVE session's `dispose_with` is still fanning `session_shutdown` out to
/// extensions and awaiting the fsync drain — truncating the very shutdown `ACP-005`/`ACP-023`
/// exist to guarantee, and re-emitting `session_shutdown` for a dead session on the way out.
///
/// So the target is a slot: [`crate::acp_host::BinaryAcpHost`] arms **one** watcher and later
/// runtimes replace what it points at. A `std::sync::Mutex` rather than a `watch` channel because
/// the watcher reads it exactly once, synchronously, after the signal arrives — there is nothing
/// to await on and no change notification anyone wants.
///
/// A poisoned lock is recovered rather than propagated: this value is read on the way out of the
/// process, and refusing to dispose the session because some unrelated task panicked while holding
/// the slot would be the worst possible reading of a poison flag.
#[derive(Clone, Default)]
pub struct RuntimeSlot(Arc<std::sync::Mutex<Option<Arc<AgentSessionRuntime>>>>);

impl RuntimeSlot {
    /// An empty slot, for a watcher armed before any runtime exists.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A slot already pointing at `runtime` — the fixed-target case every non-ACP host uses.
    #[must_use]
    pub fn of(runtime: Arc<AgentSessionRuntime>) -> Self {
        let slot = Self::new();
        slot.set(runtime);
        slot
    }

    /// Point the slot at `runtime`, replacing whatever it held.
    ///
    /// Dropping the previous `Arc` here does not dispose it: the caller replacing a runtime is the
    /// one that owns its teardown (`cyrup_acp::SessionManager::install`), and this slot is only a
    /// borrow of whatever is current.
    pub fn set(&self, runtime: Arc<AgentSessionRuntime>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
    }

    /// The runtime to tear down, cloned out so the guard is released before any `.await`.
    ///
    /// `None` only in the window before the first runtime exists — see `AcpHost::runtime_ready`'s
    /// doc for why that window is empty (no session means no tracked bash group and nothing to
    /// dispose).
    fn current(&self) -> Option<Arc<AgentSessionRuntime>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Which shutdown signal was delivered, so a REPEAT delivery can exit with the conventional code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
    Hangup,
}

impl ShutdownSignal {
    /// pi's own exit codes: `process.exit(signal === "SIGHUP" ? 129 : 143)`
    /// (`print-mode.ts:52-62`, `rpc-mode.ts:374`), i.e. the shell's `128 + signum` convention.
    /// SIGINT is 130 by that same convention.
    const fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
    }

    /// Whether pi's handler set covers this signal. pi registers `["SIGTERM", "SIGHUP"]` and NOTHING
    /// else (print-mode.ts:49-51, rpc-mode.ts:366-369, interactive-mode.ts:3652-3655).
    ///
    /// CYRUP-DELTA — SIGINT. pi installs no `SIGINT` listener in any host: its Ctrl-C is a TUI key
    /// event (`handleCtrlC`, interactive-mode.ts:3539-3546) because the terminal is in raw mode, and
    /// a literal `kill -INT` therefore takes Node's default (immediate death). cyrup's watcher is a
    /// tokio `ctrl_c()` future, which necessarily *intercepts* the signal — it cannot decline to
    /// handle it and leave the default in place — so the choice is between an immediate exit and the
    /// graceful abort this has always done. It stays graceful: `AgentSession::abort` + the cancel
    /// token, no process exit, so the mode's own teardown runs. A repeat still escalates below.
    const fn is_pi_shutdown_signal(self) -> bool {
        matches!(self, Self::Terminate | Self::Hangup)
    }
}

/// Await one shutdown signal (SIGINT/Ctrl-C, or on Unix SIGTERM/SIGHUP) and report which arrived.
///
/// A fresh set of streams is created per call so this can be awaited again for the SECOND delivery;
/// tokio's underlying handler is installed process-wide and stays installed either way.
async fn wait_for_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        // If a handler cannot be installed, fall back to Ctrl-C alone rather than failing startup.
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) {
            (Ok(mut sigterm), Ok(mut sighup)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => ShutdownSignal::Interrupt,
                    _ = sigterm.recv() => ShutdownSignal::Terminate,
                    _ = sighup.recv() => ShutdownSignal::Hangup,
                }
            }
            (Ok(mut sigterm), Err(_)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => ShutdownSignal::Interrupt,
                    _ = sigterm.recv() => ShutdownSignal::Terminate,
                }
            }
            (Err(_), _) => {
                let _ = tokio::signal::ctrl_c().await;
                ShutdownSignal::Interrupt
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        ShutdownSignal::Interrupt
    }
}

/// Whether the FIRST delivery of `signal` exits the process from the handler, and with what code —
/// pi's per-host decision, isolated so it is unit-testable without delivering real signals.
///
/// `None` means "do not exit here": either the host is interactive (pi's `shutdown({fromSignal})`
/// ends in `process.exit(0)` *after* the TUI teardown, which in cyrup is the run loop's job — this
/// task only fires the cancel token that starts it, interactive-mode.ts:3559-3580), or the signal is
/// SIGINT, which pi does not handle at all (see [`ShutdownSignal::is_pi_shutdown_signal`]).
fn first_delivery_exit_code(host: AppMode, signal: ShutdownSignal) -> Option<i32> {
    if host == AppMode::Interactive || !signal.is_pi_shutdown_signal() {
        return None;
    }
    Some(signal.exit_code())
}

/// Spawn the shutdown watcher for `host`.
///
/// First delivery — pi's handler body, in pi's order: stop the in-flight run (the CURRENT session's
/// `AgentSession::abort` + `cancel`, cyrup's analog of pi aborting the agent through its
/// disposal), then for a non-interactive host `runtime.dispose()` (the `session_shutdown{quit}`
/// emission extensions rely on — `AgentSessionRuntime::dispose`, pi
/// `runtimeHost.dispose()` at print-mode.ts:57 / rpc-mode.ts:733) and `process::exit` with pi's
/// code. For an interactive host the cancel token is the whole handler: `App::run` breaks, restores
/// the terminal, and `main` disposes the runtime and returns 0, matching
/// interactive-mode.ts:3559-3580 — exiting from here instead would race the terminal restore and
/// leave the user's shell in raw mode.
///
/// Second delivery — pi's re-entrancy guard, `if (shuttingDown) process.exit(exitCode)`
/// (rpc-mode.ts:723-726; print-mode.ts reaches the same place through its `disposed` flag,
/// `:41-46`): a hard exit with the REPEAT signal's code, and it is armed *concurrently* with the
/// first delivery's dispose, exactly as pi's handler can re-enter while the first `await` is
/// pending. Without it a wedged teardown is unkillable by anything but SIGKILL.
///
/// CYRUP-DELTA — pi's interactive host swallows the repeat instead (`if (this.isShuttingDown)
/// return`, interactive-mode.ts:3560-3561). cyrup escalates there too: pi's first delivery has
/// already reached `process.exit` inside its own handler by then, whereas cyrup's interactive exit
/// is owned by the run loop, so leaving the repeat inert would make a stalled loop immune to
/// `kill` — the SEAM-047 symptom this function exists to remove.
///
/// Returns the task handle; dropping it does not stop the watcher (it lives for the run). The binary
/// keeps it alive for the duration of the active mode.
pub fn spawn_abort_on_signal(
    runtime: Arc<AgentSessionRuntime>,
    cancel: CancelToken,
    host: AppMode,
) -> tokio::task::JoinHandle<()> {
    spawn_abort_on_signal_slot(RuntimeSlot::of(runtime), cancel, host)
}

/// [`spawn_abort_on_signal`] against a [`RuntimeSlot`] the caller can re-point.
///
/// Identical in every respect except which runtime the teardown reaches: this one reads the slot
/// once, when the signal arrives. It exists for the ACP host, which builds a runtime per
/// `session/new` and must arm exactly **one** watcher across all of them — see [`RuntimeSlot`] for
/// what a watcher per runtime costs.
pub fn spawn_abort_on_signal_slot(
    target: RuntimeSlot,
    cancel: CancelToken,
    host: AppMode,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let first = wait_for_signal().await;

        // SEAM-S03 — pi's handler body opens with `killTrackedDetachedChildren()` (print-mode.ts:55,
        // rpc-mode.ts:373, interactive-mode.ts:3663 @v0.83.0), so this is genuinely first: before
        // the repeat watcher, before the abort, before the dispose. It is synchronous and takes no
        // lock across the `killpg` loop, so unlike everything below it there is no `.await` at which
        // this task could be dropped without it having run. Placing it ahead of the `tokio::spawn`
        // costs a `killpg` per live bash group in the window where a second signal has no stream
        // listening — a window that exists at HEAD regardless (each `wait_for_signal` builds fresh
        // streams) and that tokio's already-installed process-wide handler keeps from being fatal.
        kill_tracked_detached_children();

        // Arm the repeat watcher BEFORE the (awaiting) teardown below, so a second signal lands
        // while the first is still disposing — pi's handler is re-entrant for the same reason.
        let repeat = tokio::spawn(async move {
            // Drain again: pi's repeat path is a bare `process.exit` (rpc-mode.ts:723) and can
            // afford to be, because its first delivery already SIGKILLed every tracked group AND
            // its interactive repeat is inert. cyrup's repeat hard-exits on the interactive host
            // too (the CYRUP-DELTA below), where the run loop keeps executing after the first
            // delivery — so a bash child spawned in that window would be orphaned by cyrup's
            // escalation and by nothing upstream. This closes a hole cyrup's own delta opened.
            let again = wait_for_signal().await;
            kill_tracked_detached_children();
            // PERF-004 §3.5: this arm hard-exits without reaching `runtime.dispose()`, so it is
            // the one path that must drain the session fsync queue itself. Synchronous on
            // purpose — it is already a hard exit and one flush round is ~200 µs.
            flush_session_writes();
            std::process::exit(again.exit_code());
        });

        // SEAM-059 (which this function's rewrite was told to land with): dereference the CURRENT
        // session, never the startup `Arc`. pi's handlers reach the agent through the runtime host
        // (`runtimeHost.dispose()`, print-mode.ts:57 / rpc-mode.ts:733), so a `/new`, `/fork`,
        // `switchSession` or `reload` earlier in the run cannot leave the signal aborting a disposed
        // session while the live turn runs on to completion.
        //
        // Read ONCE, here: the abort and the dispose below must reach the same runtime, and
        // re-reading the slot between them would let a `session/new` landing mid-teardown split the
        // two across a live and a dead session.
        let runtime = target.current();
        if let Some(runtime) = runtime.as_ref() {
            runtime.session().await.abort();
        }
        cancel.cancel();

        if let Some(code) = first_delivery_exit_code(host, first) {
            if let Some(runtime) = runtime.as_ref() {
                runtime.dispose().await;
            }
            // `dispose` already drained the session fsync queue (PERF-004 §3.5); this covers
            // anything appended between it returning and the exit below.
            flush_session_writes();
            std::process::exit(code);
        }

        // Interactive / SIGINT: the run loop owns the exit. Keep the repeat watcher alive.
        let _ = repeat.await;
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// An unarmed slot answers `None` rather than blocking or panicking, which is what makes a
    /// watcher armable before the first runtime exists — the shape `crate::acp_host` needs so it
    /// can arm ONE watcher and re-point it, instead of one watcher per `session/new` racing
    /// `std::process::exit`.
    #[test]
    fn an_empty_runtime_slot_is_none_not_a_panic() {
        assert!(RuntimeSlot::new().current().is_none());
    }

    /// The shell's `128 + signum` convention, and pi's literal
    /// `process.exit(signal === "SIGHUP" ? 129 : 143)` (`print-mode.ts:52-62`).
    #[test]
    fn repeat_signal_exit_codes_match_pi() {
        assert_eq!(ShutdownSignal::Interrupt.exit_code(), 130);
        assert_eq!(ShutdownSignal::Terminate.exit_code(), 143);
        assert_eq!(ShutdownSignal::Hangup.exit_code(), 129);
    }

    /// SEAM-047: the FIRST SIGTERM/SIGHUP must terminate a non-interactive host with pi's code
    /// (print-mode.ts:52-62, rpc-mode.ts:374) — it used to be absorbed, leaving `--mode rpc` alive
    /// until SIGKILL. Interactive stays with the run loop (interactive-mode.ts:3559-3580), and
    /// SIGINT keeps cyrup's graceful abort (pi registers no SIGINT handler).
    #[test]
    fn first_sigterm_and_sighup_exit_non_interactive_hosts() {
        // `ACP-006` — `AppMode::Acp` is a non-interactive host and takes the same three rows.
        // `first_delivery_exit_code` needed no change to answer correctly for it (it is `!=
        // Interactive`), so this line is coverage: it is what fails if `Acp` is ever folded into
        // the interactive arm, where a SIGTERM would leave the editor's agent process alive.
        for host in [AppMode::Rpc, AppMode::Print, AppMode::Json, AppMode::Acp] {
            assert_eq!(
                first_delivery_exit_code(host, ShutdownSignal::Terminate),
                Some(143),
                "{host:?} must exit 143 on the FIRST SIGTERM"
            );
            assert_eq!(
                first_delivery_exit_code(host, ShutdownSignal::Hangup),
                Some(129),
                "{host:?} must exit 129 on the FIRST SIGHUP"
            );
            assert_eq!(
                first_delivery_exit_code(host, ShutdownSignal::Interrupt),
                None,
                "{host:?} keeps the graceful SIGINT abort"
            );
        }
        for signal in [
            ShutdownSignal::Terminate,
            ShutdownSignal::Hangup,
            ShutdownSignal::Interrupt,
        ] {
            assert_eq!(
                first_delivery_exit_code(AppMode::Interactive, signal),
                None,
                "interactive teardown owns the exit ({signal:?})"
            );
        }
    }
}
