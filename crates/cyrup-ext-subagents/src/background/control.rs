//! File-based interrupt/resume/append-step control protocol for the background/async job system
//! (func-SA §5.4; arch-SA §6.5). Implements R-SA-079..087 and R-SA-094..097.
//!
//! This module is the sole owner of every *control-plane* operation an orchestrator issues
//! against an already-spawned (or already-terminal) background run:
//!
//! - [`interrupt`] — dual-channel (file + best-effort signal) soft-pause request (R-SA-081),
//!   idempotent consumption semantics on the runner side (R-SA-083), Paused-not-Failed outcome
//!   and descendant propagation (R-SA-084, the propagation-to-descendants *step* of which is
//!   [`runner_main.rs`]'s job once written — this module only *delivers* the request; see the
//!   deferred-work note below).
//! - [`resume`] — the R-SA-085/086 fork: steer an already-`Running` run's live child (no
//!   respawn) vs. genuinely re-spawn a fresh detached-runner-spawned child seeded from a
//!   terminal run's persisted transcript (hard failure, no silent fallback, if no transcript
//!   exists).
//! - [`append_step`] — the R-SA-094/095/096 enqueue-then-consume protocol: validate a new
//!   output-name binding against completed-plus-declared-plus-other-still-pending names
//!   (re-scanned from disk at validation time, never cached, to close the two-concurrent-appends
//!   race), then atomically enqueue a [`ChainAppendRequest`] file for the runner's own
//!   step-execution loop to consume at-most-once, in enqueue (creation) order.
//! - [`reconcile_before_control_op`] — the R-SA-079 gate every one of the above (plus a bare
//!   `status` read) MUST run first: never act on a `status.json` that might be stale relative to
//!   an authoritative terminal [`super::ResultFile`] or a since-dead pid.
//! - [`validate_safe_token`] — the R-SA-087 path-safety gate every function above runs incoming
//!   run-id/step-index/profile-name tokens through before any filesystem lookup.
//! - [`poll_root_attachment`] — the R-SA-097 "attach a new chain's first step to another run's
//!   result, by polling, never by spawning" primitive.
//!
//! # Deferred to later phases (do not implement here)
//!
//! - **The detached runner's own control-inbox watcher task** (the *consuming* side of
//!   [`interrupt`]'s dual-channel delivery: `notify`-based watch + ~250ms poll fallback + the
//!   mandatory synchronous startup check, R-SA-082) and **the runner's per-iteration
//!   append-request consumption** (the *consuming* side of [`append_step`]'s enqueue, R-SA-095's
//!   "list, read, delete, then extend the in-loop step list" sequence) both live in
//!   `background/runner_main.rs`, which does not exist yet as of this file. This module supplies
//!   the exact on-disk shapes ([`InterruptRequest`], [`ChainAppendRequest`]) and the
//!   orchestrator-side write/enqueue halves those consumers read; the consumer implementations
//!   themselves are `runner_main.rs`'s job, per this crate's module-layout doc (arch-SA §2.2).
//!   [`watch_control_inbox_once`] below is provided as the reusable *primitive* (real
//!   `notify::PollWatcher` + immediate synchronous startup check) both this module's own tests
//!   and, later, `runner_main.rs` can build on — it does not itself decide interrupt semantics.
//! - **Marking running steps `Paused`, appending the `run.paused` event, and forwarding to live
//!   nested-async descendants before signaling the actively-spawned child subprocess(es)**
//!   (R-SA-084's runner-side reaction to having consumed an [`InterruptRequest`]) is
//!   `runner_main.rs`'s job — this module's [`interrupt`] only performs the two *delivery*
//!   actions R-SA-081 itself specifies (write the file, best-effort-signal the pid); it does not
//!   run inside the runner process and cannot itself flip any step's state.
//! - **Full stale-run liveness reconciliation** (`Liveness::{Alive,Dead,Unknown}` via a real
//!   zero-signal `kill(pid, 0)` probe, the 24h long-staleness-to-Failed threshold, and
//!   synthesized-failure-result construction on dead-but-claiming-Running, R-SA-088/089/091/092)
//!   is `background/reconcile.rs`'s job, per arch-SA §2.2/§9's coverage table, which does not
//!   exist yet as of this file. [`reconcile_before_control_op`] here implements only the narrow
//!   slice R-SA-079 itself requires of *this* module — "run reconciliation before acting on
//!   cached state" — via the R-SA-088 top-level algorithm shape (ResultFile-authoritative, else
//!   return status as-is otherwise) with a minimal, self-contained liveness probe. When
//!   `reconcile.rs` lands, its fuller `reconcile()` (grace-window/staleness-threshold-aware,
//!   synthesized-failure-writing) is the intended long-term replacement for this function's body;
//!   this module should be updated at that point to delegate to it rather than duplicate the
//!   probe. Documented here rather than left silently inconsistent.
//! - **Combined cross-namespace run-id ambiguity detection** (R-SA-080, `SHOULD`) spans the
//!   foreground-control/async/nested-async id namespaces this crate's later phases still need to
//!   define; this module exposes [`super::error::AmbiguousRunId`]-shaped failures for the single
//!   namespace it *does* own (background run ids under one `AsyncRoot`) but does not attempt the
//!   cross-namespace merge, which has no owner module yet.
//! - **The orchestrator-side shared poller** that periodically invokes reconciliation for every
//!   *tracked* run (R-SA-093) is `background/tracker.rs`'s job — this module's functions are all
//!   synchronous, single-call control operations invoked on demand (e.g. from a slash command or
//!   tool call), never a background loop themselves.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cyrup_core::ModelId;

use crate::error::SubagentError;
use crate::exec::SingleResult;
use crate::spawn::chain_graph::RunnerStep;

use super::atomic::write_atomic_json;
use super::{ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, StepState};

// =================================================================================================
// Safe-token path validation (R-SA-087)
// =================================================================================================

/// Validates `token` as a "safe token" (R-SA-087): non-empty, no path separators (`/` or `\`),
/// and no `..` component — before it is used to construct ANY filesystem path. This MUST be
/// called on every externally supplied run-id/step-index/profile-name-shaped string before it
/// participates in a [`Path::join`] anywhere in this module.
///
/// This is a pure string check — it does not touch the filesystem. Verifying that an already-
/// constructed path stays inside its configured root is [`validate_contains_root`]'s separate,
/// complementary job (R-SA-087's second clause).
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `token` is empty or contains a path separator or
/// a `..` component.
pub fn validate_safe_token(token: &str) -> Result<(), SubagentError> {
    if token.is_empty() {
        return Err(SubagentError::UnsafePathToken(
            "token must not be empty".to_string(),
        ));
    }
    if token.contains('/') || token.contains('\\') {
        return Err(SubagentError::UnsafePathToken(format!(
            "token must not contain a path separator: {token:?}"
        )));
    }
    if token.contains("..") {
        return Err(SubagentError::UnsafePathToken(format!(
            "token must not contain \"..\": {token:?}"
        )));
    }
    Ok(())
}

/// Verifies that `resolved` (a path already constructed from a validated token) lies strictly
/// inside `root` (R-SA-087's second clause: "resolved paths MUST be verified to remain inside the
/// configured async/results roots"). This is a lexical containment check (`starts_with` on the
/// path components), not a symlink-resolving filesystem check — both `root` and `resolved` in
/// every call site in this module are constructed by plain [`Path::join`] from an already-
/// safe-token-validated component, so a lexical check is sufficient defense-in-depth without
/// requiring `resolved` to already exist on disk (which it frequently does not yet, e.g. a
/// not-yet-created `RunDir`).
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `resolved` does not lie inside `root`.
pub fn validate_contains_root(root: &Path, resolved: &Path) -> Result<(), SubagentError> {
    if resolved.starts_with(root) {
        Ok(())
    } else {
        Err(SubagentError::UnsafePathToken(format!(
            "resolved path {resolved:?} escapes configured root {root:?}"
        )))
    }
}

/// Convenience composition of [`validate_safe_token`] + [`RunPaths::for_run`] +
/// [`validate_contains_root`] (against both `async_root` and `results_dir`): the one call every
/// public function in this module makes to turn an untrusted `run_id` string into a trusted
/// [`RunPaths`], per R-SA-087's "before any filesystem lookup" ordering requirement.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `run_id_token` fails [`validate_safe_token`] or
/// if either derived path would escape its configured root.
pub fn resolve_run_paths(
    async_root: &Path,
    results_dir: &Path,
    run_id_token: &str,
) -> Result<RunPaths, SubagentError> {
    validate_safe_token(run_id_token)?;
    let run_id = RunId::from_token(run_id_token);
    let paths = RunPaths::for_run(async_root, results_dir, &run_id);
    validate_contains_root(async_root, &paths.run_dir)?;
    validate_contains_root(results_dir, &paths.result)?;
    Ok(paths)
}

// =================================================================================================
// Reconciliation gate (R-SA-079) — narrow slice; see module docs' deferred-work note
// =================================================================================================

/// Runs the R-SA-079-mandated reconciliation gate before returning (or letting a caller act on)
/// `status.json`'s claimed state, per the R-SA-088 top-level algorithm shape:
///
/// 1. If the terminal [`ResultFile`] exists, it is ALWAYS authoritative: if `status` still claims
///    a non-terminal state, repair it in place (state + `ended_at`) and persist the repair before
///    returning.
/// 2. Otherwise, if `status.json` does not exist yet at all, synthesize the R-SA-090 provisional
///    status (grace window immediately after spawn) rather than declaring the run failed.
/// 3. Otherwise, read `status.json` as-is: this module's own narrow slice of R-SA-079 stops here
///    — the *fuller* liveness-probe/long-staleness-to-Failed reconciliation (R-SA-089/091/092) is
///    `background/reconcile.rs`'s job once it exists (see module docs).
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] (wrapping the underlying I/O error) if `status.json` exists
/// but cannot be read or parsed, and no [`ResultFile`] exists to fall back on.
pub async fn reconcile_before_control_op(paths: &RunPaths) -> Result<RunStatus, SubagentError> {
    if let Some(result) = read_result_file(&paths.result).await? {
        let mut status = read_status_file(&paths.status).await?.unwrap_or_else(|| {
            RunStatus::queued(result.run_id.clone(), result.mode, None)
        });
        if !status.state.is_terminal() {
            // Repair status.json from the authoritative ResultFile (R-SA-077's "readers MUST
            // treat presence of the ResultFile as authoritative... repairing status.json from it
            // if needed"). `advance_state` only succeeds along the legal transition graph; if the
            // in-memory state is already unreachable-from (e.g. corrupt/foreign status.json), we
            // still want the caller to observe the authoritative terminal state, so we fall back
            // to constructing a fresh, already-terminal status rather than surfacing a transition
            // error for what is fundamentally a repair operation, not a normal mutation.
            if status.state.can_transition_to(result.state) {
                let _ = status.advance_state(result.state);
            } else {
                status = terminal_status_from_result(&result, status.pid);
            }
            write_atomic_json(&paths.status, &status)
                .await
                .map_err(SubagentError::Spawn)?;
        }
        return Ok(status);
    }

    match read_status_file(&paths.status).await? {
        Some(status) => Ok(status),
        None => {
            // No status.json yet and no ResultFile: either the run is still within the R-SA-090
            // spawn grace window (the caller-supplied provisional status at the spawn call site
            // covers that case and is not re-derivable here without a known pid/mode), or the run
            // id simply does not exist. This module cannot distinguish those two cases from disk
            // state alone — that requires the spawn call site's own provisional status, which is
            // an orchestrator-in-memory concern outside this file's scope (see `background/mod.rs`
            // `RunStatus::provisional`). Surfacing "not found" here is correct for every control
            // op except the narrow spawn-grace-window race, which callers that just confirmed a
            // spawn are expected to bridge with their own provisional status rather than calling
            // this reconciliation gate at all in that instant.
            Err(SubagentError::Spawn(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no status.json or result file at {:?}", paths.run_dir),
            )))
        }
    }
}

/// Constructs an already-terminal [`RunStatus`] shape from an authoritative [`ResultFile`], for
/// the rare repair path where the on-disk `status.json` cannot legally transition to the result's
/// terminal state (e.g. a corrupt or foreign status record). Every step is marked terminal to
/// match `result.state`.
fn terminal_status_from_result(result: &ResultFile, pid: Option<u32>) -> RunStatus {
    let now = now_epoch_millis();
    RunStatus {
        run_id: result.run_id.clone(),
        mode: result.mode,
        state: result.state,
        pid,
        // Carry the authoritative ResultFile's own `cwd`/`sessionFile` through the repair (pi's
        // `status.cwd ?? result.cwd` fallback, `background/async-resume.ts:323,345,373`, has
        // nothing to fall back FROM here otherwise — this is the terminal-repair path where
        // `status.json` itself could not legally advance, so the ResultFile is the only surviving
        // source of truth for either field).
        cwd: Some(result.cwd.clone()),
        session_file: result.session_file.clone(),
        started_at: now,
        ended_at: Some(now),
        last_update: now,
        current_step: None,
        chain_step_count: None,
        pending_appends: None,
        steps: Vec::new(),
        parallel_groups: None,
        telemetry: crate::background::RunTelemetry::default(),
    }
}

async fn read_status_file(path: &Path) -> Result<Option<RunStatus>, SubagentError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, e))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

async fn read_result_file(path: &Path) -> Result<Option<ResultFile>, SubagentError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, e))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

// =================================================================================================
// InterruptRequest (R-SA-081/082/083/084)
// =================================================================================================

/// The on-disk control-inbox request record (func-SA §4.5): `{ type: "interrupt", ts, source,
/// reason }`. Its mere presence at [`RunPaths::control_inbox`] IS the pending-interrupt state
/// (R-SA-083: "the file's existence is the sole state") — deletion is the at-most-once
/// consumption primitive, not a separate acknowledgement field on this struct.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptRequest {
    /// Always `"interrupt"` — a discriminant tag kept as a plain string field (rather than a unit
    /// enum) so the on-disk shape matches func-SA §4.5's literal `{ type: "interrupt", ... }`
    /// wire form byte-for-byte, in case a future request kind is ever added to the same inbox
    /// slot.
    #[serde(rename = "type")]
    pub kind: String,
    /// Wall-clock creation time (epoch milliseconds), for diagnostics and for the runner's own
    /// `run.paused` event payload.
    pub ts: i64,
    /// Free-form identifier of who/what requested the interrupt (e.g. `"user"`,
    /// `"orchestrator-tool-call"`, a nested-run id forwarding its own interrupt downward).
    pub source: String,
    /// Optional human-readable reason, surfaced in TUI control notices.
    pub reason: Option<String>,
}

impl InterruptRequest {
    /// Constructs a fresh interrupt request stamped with the current wall-clock time.
    #[must_use]
    pub fn new(source: impl Into<String>, reason: Option<String>) -> Self {
        Self {
            kind: "interrupt".to_string(),
            ts: now_epoch_millis(),
            source: source.into(),
            reason,
        }
    }
}

/// The observable outcome of one [`interrupt`] call — distinguishes "this call actually wrote a
/// fresh request" from "a request (or an already-paused/terminal state) made this call a no-op",
/// so callers/tests can assert idempotency precisely (R-SA-083).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptOutcome {
    /// A fresh [`InterruptRequest`] file was written and (if a pid was known and alive) a
    /// best-effort wake-up signal was sent.
    Delivered,
    /// A pending interrupt request already existed at the control inbox — this call left it in
    /// place untouched and performed no new write (idempotent no-op, R-SA-083's "duplicate
    /// consumption... MUST be silently absorbed" restated at the delivery side: a duplicate
    /// *request* is likewise absorbed, not re-delivered as a second file).
    AlreadyPending,
    /// The run is not in a state `interrupt` can act on (R-SA-079's "requires Running state" —
    /// interrupting a `Queued`, already-`Paused`, or terminal run is a no-op, not an error, since
    /// a second interrupt racing a first one's consumption is exactly the idempotency case
    /// R-SA-083 requires to be silently absorbed rather than treated as a protocol violation).
    NotRunning,
}

/// Delivers an interrupt request against the run identified by `run_id_token` (R-SA-079/081).
///
/// Runs the R-SA-079 reconciliation gate first. If the reconciled status is not `Running`,
/// returns [`InterruptOutcome::NotRunning`] without writing anything — this covers BOTH "nothing
/// to interrupt" (`Queued`/terminal) AND the idempotency case R-SA-083 requires: a run that has
/// already consumed a prior interrupt and moved to `Paused` produces the identical no-op outcome
/// as one that was never interrupted, satisfying "a second interrupt on an already-pausing run is
/// a no-op, not an error" without this function needing to distinguish "already paused" from
/// "never was running" — both are simply not-Running.
///
/// If already `Running`, checks whether a request is already pending (a fresh, not-yet-consumed
/// [`InterruptRequest`] file already sitting at the control inbox — the window between two
/// interrupt calls racing each other before the runner has consumed the first) and returns
/// [`InterruptOutcome::AlreadyPending`] without a second write in that case. Otherwise performs
/// the two independent R-SA-081 delivery actions: (a) atomically write a fresh
/// [`InterruptRequest`] file, (b) best-effort send a wake-up signal (`SIGUSR2` on Unix) to the
/// tracked pid — a signal-delivery failure that specifically indicates "no such process" (not
/// "unsupported") triggers removal of the just-written request file, since the file's presence
/// would otherwise wrongly claim a live run has a pending interrupt when the process backing it
/// is already gone.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `run_id_token` is unsafe, or an I/O error
/// (wrapped in [`SubagentError::Spawn`]) if the reconciliation read or the request write fails.
pub async fn interrupt(
    async_root: &Path,
    results_dir: &Path,
    run_id_token: &str,
    source: impl Into<String>,
    reason: Option<String>,
) -> Result<InterruptOutcome, SubagentError> {
    let paths = resolve_run_paths(async_root, results_dir, run_id_token)?;
    let status = reconcile_before_control_op(&paths).await?;

    if status.state != RunState::Running {
        return Ok(InterruptOutcome::NotRunning);
    }

    if tokio::fs::try_exists(&paths.control_inbox)
        .await
        .map_err(SubagentError::Spawn)?
    {
        return Ok(InterruptOutcome::AlreadyPending);
    }

    if let Some(parent) = paths.control_inbox.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(SubagentError::Spawn)?;
    }
    write_atomic_json(&paths.control_inbox, &InterruptRequest::new(source, reason))
        .await
        .map_err(SubagentError::Spawn)?;

    deliver_wakeup_signal(&paths, status.pid).await?;

    Ok(InterruptOutcome::Delivered)
}

/// Performs R-SA-081's clause (b): opportunistically send a best-effort wake-up signal
/// (`SIGUSR2` on Unix — a notification-only signal distinct from the SIGINT/SIGTERM/SIGKILL
/// *termination* ladder `spawn::signal::terminate` drives against a run's own child
/// subprocess(es); this signal targets the detached RUNNER process itself, to nudge it into
/// checking its control inbox sooner than its next poll tick) to `pid`, if known.
///
/// A signal-send failure that specifically indicates "no such process" (`ESRCH` on Unix) removes
/// the just-written [`InterruptRequest`] file — the runner is already gone, so leaving the file
/// behind would misrepresent a dead run as having a pending, eventually-actionable interrupt.
/// Every other failure class (including "signal type unsupported" on a non-Unix target, and
/// `EPERM`-class ambiguous failures) is swallowed: the file-inbox channel remains authoritative
/// regardless of whether the signal itself was delivered (R-SA-081's explicit "opportunistically…
/// best-effort").
async fn deliver_wakeup_signal(paths: &RunPaths, pid: Option<u32>) -> Result<(), SubagentError> {
    deliver_wakeup_signal_to(&paths.control_inbox, pid).await;
    Ok(())
}

/// [`deliver_wakeup_signal`] addressed by the request-file path rather than a whole [`RunPaths`],
/// so the nested-descendant cascade (which only ever holds a descendant's directory) sends the
/// identical best-effort signal with the identical ESRCH-removes-the-stale-request semantics.
async fn deliver_wakeup_signal_to(control_inbox: &Path, pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };

    #[cfg(unix)]
    {
        let nix_pid = nix::unistd::Pid::from_raw(pid as nix::libc::pid_t);
        match nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGUSR2) {
            Ok(()) => {}
            Err(nix::errno::Errno::ESRCH) => {
                // The tracked pid is already gone: the file-inbox request we just wrote is now
                // stale/misleading (a dead run can never consume it) — remove it. `remove_file`
                // itself racing a concurrent removal (e.g. a second interrupt call, or the runner
                // itself finishing an in-flight consumption) is swallowed: "already gone" is a
                // success outcome for a removal, not a failure to propagate.
                let _ = tokio::fs::remove_file(control_inbox).await;
            }
            Err(_other) => {
                // EPERM-class or any other ambiguous failure: swallowed per R-SA-081's
                // "opportunistically... best-effort" — the file inbox stays authoritative.
            }
        }
    }
    #[cfg(not(unix))]
    {
        // No portable SIGUSR2-equivalent notification signal on non-Unix targets: this is the
        // "signal unsupported" case R-SA-081 explicitly distinguishes from "process gone" — the
        // request file is deliberately left in place (the runner's poll-fallback watch, R-SA-082,
        // still picks it up within its fixed interval even with no signal wake-up).
        let _ = control_inbox;
        let _ = pid;
    }
}

// =================================================================================================
// TimeoutRequest — the second control-inbox verb (pi `control-channel.ts` @v0.34.0)
// =================================================================================================

/// The on-disk `control/timeout.json` request record — pi's `TimeoutRequest`
/// (`src/runs/background/control-channel.ts:41` @v0.34.0): `{ type: "timeout", ts, source,
/// reason }`, the exact sibling shape of [`InterruptRequest`], sitting in the exact same control
/// inbox directory under a different file name.
///
/// # Why a second verb rather than reusing `interrupt`
///
/// The two verbs are NOT interchangeable and the difference is observable in the run's terminal
/// record. An interrupt is a soft, *resumable* pause: the run ends `Paused`, every unfinished
/// step is marked `Paused`, and `resume` can pick it back up. A timeout is *terminal failure*:
/// the run ends `Failed` with `timedOut`, every unfinished step is marked `Failed` with the
/// timeout message, and there is nothing to resume. Upstream keeps them as two files precisely so
/// an ancestor whose OWN deadline expired can fail its whole subtree rather than leave a forest of
/// descendants sitting in a resumable-but-never-resumed `Paused` state (see
/// `background::cascade`, which is this verb's production writer).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeoutRequest {
    /// Always `"timeout"` — the discriminant, kept as a plain string field for the same
    /// byte-for-byte on-disk-shape reason [`InterruptRequest::kind`] is.
    #[serde(rename = "type")]
    pub kind: String,
    /// Wall-clock creation time (epoch milliseconds).
    pub ts: i64,
    /// Who/what imposed the timeout — `"ancestor-timeout"` for the cascade path, matching pi's
    /// own literal (`subagent-runner.ts:1585` @v0.34.0).
    pub source: String,
    /// Optional human-readable reason, surfaced in the timed-out run's error text.
    pub reason: Option<String>,
}

impl TimeoutRequest {
    /// Constructs a fresh timeout request stamped with the current wall-clock time.
    #[must_use]
    pub fn new(source: impl Into<String>, reason: Option<String>) -> Self {
        Self {
            kind: "timeout".to_string(),
            ts: now_epoch_millis(),
            source: source.into(),
            reason,
        }
    }
}

/// `<run_dir>/control/` — the control inbox directory (pi `controlInboxDir`).
#[must_use]
pub fn control_inbox_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("control")
}

/// `<run_dir>/control/interrupt.json` (pi `interruptRequestPath`). Identical to
/// [`RunPaths::control_inbox`] but derived from a bare run directory, which is what the
/// nested-descendant cascade has to work with: a descendant's async dir is discovered from the
/// nested-run registry, never resolved through this run's own `async_root`/`results_dir` pair.
#[must_use]
pub fn interrupt_request_path(run_dir: &Path) -> PathBuf {
    control_inbox_dir(run_dir).join("interrupt.json")
}

/// `<run_dir>/control/timeout.json` (pi `timeoutRequestPath`).
#[must_use]
pub fn timeout_request_path(run_dir: &Path) -> PathBuf {
    control_inbox_dir(run_dir).join("timeout.json")
}

// =================================================================================================
// StopRequest (G77) — the control inbox's FOURTH verb (pi `control-channel.ts:49-53,123-125,
// 281-290,519-530,593-601` @v0.43.0)
// =================================================================================================

/// pi's literal stop message (`subagent-runner.ts:1972` @v0.43.0: `const stopMessage = "Subagent
/// stopped by user.";`), stamped onto the stopped run's `error`, onto every step it stopped, and —
/// when the child produced no output of its own — onto the child's `finalOutput`
/// (`subagent-runner.ts:917`/`:1595-1596`).
pub const STOP_MESSAGE: &str = "Subagent stopped by user.";

/// The on-disk `control/stop.json` request record — pi's `StopRequest`
/// (`control-channel.ts:49-53` @v0.43.0): `{ type: "stop", ts, source, reason }`, the exact sibling
/// shape of [`InterruptRequest`]/[`TimeoutRequest`], in the same control inbox under a third file
/// name.
///
/// # Why a THIRD verb rather than reusing `interrupt` or `timeout`
///
/// All three are mutually exclusive verdicts upstream, and the difference is observable in the
/// run's terminal record. `stopRunner`'s own guard is `if (stopped || timedOut || interrupted ||
/// statusPayload.state !== "running") return;` (`subagent-runner.ts:2956`) and `timeoutRunner`'s is
/// the mirror image (`:2986`), so at most one of the three ever fires for a run:
///
/// | verb | terminal `state` | unfinished steps | resumable? |
/// |---|---|---|---|
/// | `interrupt` | `Paused` | `Paused` | yes (`resume`) |
/// | `timeout` | `Failed` (`timedOut`) | `Failed` | no |
/// | `stop` | `Stopped` | `Stopped` (`exitCode: 1`, [`STOP_MESSAGE`]) | **no** — `async-resume.ts:406` refuses explicitly |
///
/// The inbox drain order is likewise fixed and load-bearing: stop, THEN timeout, THEN interrupt
/// (`control-channel.ts:653-655`), so when several land together the hardest, least-resumable
/// verdict wins.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopRequest {
    /// Always `"stop"` — the discriminant, a plain string field for the same byte-for-byte
    /// on-disk-shape reason [`InterruptRequest::kind`] is.
    #[serde(rename = "type")]
    pub kind: String,
    /// Wall-clock creation time (epoch milliseconds).
    pub ts: i64,
    /// Who/what requested the stop — pi's own literals are `"stop-action"` for the
    /// `subagent({action:"stop"})` path (`async-stop-action.ts:44`) and `"ancestor-stop"` for the
    /// descendant cascade (`subagent-runner.ts:2300`).
    pub source: String,
    /// Optional human-readable reason.
    pub reason: Option<String>,
}

impl StopRequest {
    /// Constructs a fresh stop request stamped with the current wall-clock time.
    #[must_use]
    pub fn new(source: impl Into<String>, reason: Option<String>) -> Self {
        Self {
            kind: "stop".to_string(),
            ts: now_epoch_millis(),
            source: source.into(),
            reason,
        }
    }
}

/// `<run_dir>/control/stop.json` (pi `stopRequestPath`, `control-channel.ts:123-125` @v0.43.0).
#[must_use]
pub fn stop_request_path(run_dir: &Path) -> PathBuf {
    control_inbox_dir(run_dir).join("stop.json")
}

/// The observable outcome of one [`stop`] call — pi `stopAsyncRun`
/// (`runs/foreground/async-stop-action.ts:23-64` @v0.43.0) returns an `AgentToolResult` whose
/// `isError` flag and text encode exactly these three cases; this enum is that trichotomy, with the
/// user-facing prose left to the caller so the tool-result shaping stays in `extension.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    /// A [`StopRequest`] file was written into the target's control inbox (pi's `"Stop requested
    /// for async run {id}."` success result, `async-stop-action.ts:56`).
    Requested,
    /// The reconciled status is neither `Running` nor `Queued` — pi's
    /// `if (!status || (status.state !== "running" && status.state !== "queued"))` guard
    /// (`async-stop-action.ts:41`), whose message is `"No running or queued async run was found for
    /// '{id}'."` and whose `isError` is `true`. Note this makes a stop of an already-`Paused` run an
    /// ERROR upstream, not a silent no-op the way a duplicate `interrupt` is.
    NotStoppable,
}

/// Delivers a stop request against the run identified by `run_id_token` (G77; pi `stopAsyncRun`,
/// `runs/foreground/async-stop-action.ts:23-64` @v0.43.0).
///
/// Runs the same R-SA-079 reconciliation gate every other control op runs (upstream's own first act
/// is `reconcileAsyncRun(target.asyncDir, { kill }).status`, `:39`), then applies upstream's
/// actionability guard verbatim: only a `Running` or `Queued` run may be stopped. Unlike
/// [`interrupt`] there is no already-pending short-circuit — upstream writes the request
/// unconditionally via `writeAtomicJson` (`control-channel.ts:287-288`), which is idempotent by
/// construction because the request is a single well-known path whose mere existence is the state.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `run_id_token` is unsafe, or an I/O error
/// (wrapped in [`SubagentError::Spawn`]) if the reconciliation read or the request write fails —
/// upstream's own `catch` around `deliverStopRequest` (`:58-63`) surfaces the same class of failure
/// as `"Failed to stop async run {id}: {message}"`.
pub async fn stop(
    async_root: &Path,
    results_dir: &Path,
    run_id_token: &str,
    source: impl Into<String>,
    reason: Option<String>,
) -> Result<StopOutcome, SubagentError> {
    let paths = resolve_run_paths(async_root, results_dir, run_id_token)?;
    let status = match reconcile_before_control_op(&paths).await {
        Ok(status) => status,
        // pi's guard is `if (!status || (status.state !== "running" && status.state !== "queued"))`
        // (`async-stop-action.ts:41`) — an ABSENT record and a non-actionable one collapse onto the
        // SAME "No running or queued async run was found for '{id}'." refusal. This gate reports an
        // absent record as a `NotFound` I/O error rather than a `None`, so it is folded back here;
        // every other error class (a genuine read/permission failure) still propagates.
        Err(SubagentError::Spawn(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StopOutcome::NotStoppable);
        }
        Err(err) => return Err(err),
    };

    if !matches!(status.state, RunState::Running | RunState::Queued) {
        return Ok(StopOutcome::NotStoppable);
    }

    deliver_stop_request(&paths.run_dir, source, reason).await?;
    Ok(StopOutcome::Requested)
}

/// Parent side, addressed by run DIRECTORY: atomically write an [`InterruptRequest`] into
/// `run_dir`'s control inbox and send the same best-effort `SIGUSR2` wake-up [`interrupt`] does
/// (pi `deliverInterruptRequest`, `control-channel.ts` @v0.34.0).
///
/// This is the lower-level sibling of [`interrupt`]: no reconciliation gate, no
/// already-pending check, no `run_id` token resolution — the caller has already established that
/// the target is a live descendant and holds its directory directly. [`interrupt`] remains the
/// entry point for every *externally* addressed run.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the control directory cannot be created or the request
/// cannot be written.
pub async fn deliver_interrupt_request(
    run_dir: &Path,
    pid: Option<u32>,
    source: impl Into<String>,
    reason: Option<String>,
) -> Result<PathBuf, SubagentError> {
    let path = interrupt_request_path(run_dir);
    write_control_request(&path, &InterruptRequest::new(source, reason)).await?;
    deliver_wakeup_signal_to(&path, pid).await;
    Ok(path)
}

/// Parent side, addressed by run DIRECTORY: atomically write a [`TimeoutRequest`] into `run_dir`'s
/// control inbox (pi `deliverTimeoutRequest`, `control-channel.ts:536-545` @v0.34.0).
///
/// Note the deliberate asymmetry with [`deliver_interrupt_request`], faithful to upstream:
/// `deliverTimeoutRequest` sends NO wake-up signal. `SIGUSR2` is the interrupt fast-path only; a
/// timeout is picked up on the target's next control-inbox watch/poll tick
/// ([`CONTROL_INBOX_POLL_INTERVAL`]), because a timeout is by construction not latency-critical —
/// the deadline it enforces has already passed.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the control directory cannot be created or the request
/// cannot be written.
pub async fn deliver_timeout_request(
    run_dir: &Path,
    source: impl Into<String>,
    reason: Option<String>,
) -> Result<PathBuf, SubagentError> {
    let path = timeout_request_path(run_dir);
    write_control_request(&path, &TimeoutRequest::new(source, reason)).await?;
    Ok(path)
}

/// Parent side, addressed by run DIRECTORY: atomically write a [`StopRequest`] into `run_dir`'s
/// control inbox (G77; pi `deliverStopRequest` → `requestAsyncStop`, `control-channel.ts:281-290,
/// 593-601` @v0.43.0).
///
/// Like [`deliver_timeout_request`] and unlike [`deliver_interrupt_request`], this sends **no**
/// wake-up signal — upstream's `deliverStopRequest` accepts a `pid`/`kill` pair in its input shape
/// but its whole body is `requestAsyncStop(input.asyncDir, …)` (`control-channel.ts:600`); the file
/// inbox is the entire channel. The runner's own poll/watch tick picks it up and `stopRunner`
/// aborts the live children from inside.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the control directory cannot be created or the request
/// cannot be written.
pub async fn deliver_stop_request(
    run_dir: &Path,
    source: impl Into<String>,
    reason: Option<String>,
) -> Result<PathBuf, SubagentError> {
    let path = stop_request_path(run_dir);
    write_control_request(&path, &StopRequest::new(source, reason)).await?;
    Ok(path)
}

/// Shared "mkdir -p the control dir, then atomically write the request" step both dir-addressed
/// deliver functions perform.
async fn write_control_request<T: serde::Serialize + Sync>(
    path: &Path,
    request: &T,
) -> Result<(), SubagentError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(SubagentError::Spawn)?;
    }
    write_atomic_json(path, request)
        .await
        .map_err(SubagentError::Spawn)
}

// =================================================================================================
// steer (G90) — the control inbox's THIRD verb, pi `control-channel.ts:48-55,121-189` @v0.34.0
// =================================================================================================

/// One parent-to-runner steering request (pi `SteerRequest`, `control-channel.ts:48-55` @v0.34.0).
///
/// Unlike [`InterruptRequest`]/[`TimeoutRequest`], steering is a **queue, not a flag**: several
/// distinct guidance messages can be in flight at once and each must be delivered exactly once, so
/// a steer request is a uniquely-named file inside a DIRECTORY rather than a single well-known
/// path whose mere existence is the whole state. That is why [`steer_requests_dir`] exists and
/// there is no `steer_request_path`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerRequest {
    /// Always `"steer"` — the discriminant, a plain string field for the same byte-for-byte
    /// on-disk-shape reason [`InterruptRequest::kind`] is.
    #[serde(rename = "type")]
    pub kind: String,
    /// Unique request id. Also the dedup key and (base64url-encoded) half of the file name, so two
    /// requests written in the same millisecond cannot collide.
    ///
    /// For requests this crate MINTS (see [`request_async_steer`]) it is
    /// `<16-hex monotonic sequence>-<uuid>`, which makes it the TIEBREAK that orders same-millisecond
    /// guidance — see [`ts`](Self::ts). Ids that arrive from elsewhere are accepted verbatim; the
    /// only requirements are uniqueness and non-emptiness.
    pub id: String,
    /// Wall-clock creation time (epoch milliseconds). The primary sort key on consumption, so
    /// guidance is delivered in the order the parent produced it.
    ///
    /// It is not a SUFFICIENT sort key. A millisecond is an eternity next to two `request_async_steer`
    /// calls in a loop, so same-`ts` requests are routine rather than exotic, and the tiebreak
    /// decides what the user actually sees. pi ties on `base64url(randomUUID())`
    /// (`consumeSteerRequestsFromDir` sorts file names, `control-channel.ts:437`; the name is
    /// `steerRequestFileName`, `:85-87`) — i.e. two corrections typed in quick succession reach the
    /// child in RANDOM order, and the second can be overridden by the first. cyrup keeps `ts` as the
    /// primary key, exactly as upstream, and makes the tiebreak deterministic by minting a
    /// monotonic sequence into [`id`](Self::id).
    ///
    /// [CYRUP-DELTA: ordering only. The key, the file name and the wire shape are pi's; only the
    /// id's internal FORM changes, which upstream treats as an opaque unique string.]
    pub ts: i64,
    /// The guidance itself, already trimmed and known non-empty.
    pub message: String,
    /// The specific child (flat step index) this is aimed at. `None` means "every currently
    /// running child", which is what the runner fans it out to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_index: Option<usize>,
    /// Who produced it — `"steer-action"` for the `action: "steer"` tool verb, matching pi's own
    /// literal (`subagent-executor.ts:620` @v0.34.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// `<run_dir>/control/steer-requests/` — the parent-written queue the runner drains (pi
/// `steerRequestsDir`, `control-channel.ts:76-78`).
#[must_use]
pub fn steer_requests_dir(run_dir: &Path) -> PathBuf {
    control_inbox_dir(run_dir).join("steer-requests")
}

/// `<run_dir>/control/steer-targets/<index>/` — the per-child inbox the runner routes an accepted
/// request into (pi `stepSteerInboxDir`, `control-channel.ts:81-83`). Two directories rather than
/// one because the two hops have different addressees: the parent does not know which child is
/// running, and the child must not have to filter a queue that is not its own.
#[must_use]
pub fn step_steer_inbox_dir(run_dir: &Path, index: usize) -> PathBuf {
    control_inbox_dir(run_dir).join("steer-targets").join(index.to_string())
}

/// pi `steerRequestFileName` (`control-channel.ts:85-87`): `<ts zero-padded to 13>-<base64url(id)>.json`.
///
/// The zero-padding is what makes a plain lexicographic directory listing sort by time, which is
/// exactly what [`consume_steer_requests_from_dir`] relies on before its explicit re-sort — and the
/// base64url of the id keeps an arbitrary caller-supplied id from ever producing a path separator.
fn steer_request_file_name(request: &SteerRequest) -> String {
    use base64::Engine as _;
    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(request.id.as_bytes());
    format!("{:013}-{id}.json", request.ts.max(0))
}

/// pi `writeSteerRequestToDir` (`control-channel.ts:89-93`).
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the directory cannot be created or the file written.
pub async fn write_steer_request_to_dir(
    dir: &Path,
    request: &SteerRequest,
) -> Result<PathBuf, SubagentError> {
    let path = dir.join(steer_request_file_name(request));
    write_control_request(&path, request).await?;
    Ok(path)
}

/// Mint the next steer-request id: `<16-hex monotonic sequence>-<uuid>`.
///
/// Both halves are load-bearing and neither replaces the other:
///
/// - the **sequence** is what makes ordering deterministic. It is the tiebreak
///   [`consume_steer_requests_from_dir`] falls back on when two requests share a `ts`, which is the
///   common case rather than the rare one — see [`SteerRequest::ts`]. Zero-padded to 16 hex digits
///   so a plain lexicographic comparison IS a numeric one (it cannot overflow in practice: at one
///   steer per nanosecond it would take ~584 years).
/// - the **uuid** is what keeps ids unique across PROCESSES. The counter is per-process, so two
///   parents steering the same run would otherwise mint colliding ids and one request would
///   overwrite the other's file.
///
/// `Relaxed` is sufficient: the only requirement is that each caller gets a distinct, increasing
/// value, which `fetch_add` guarantees on its own — no other memory is being published through it.
fn next_steer_request_id() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{seq:016x}-{}", uuid::Uuid::new_v4().as_simple())
}

/// Parent side: pi `requestAsyncSteer` (`control-channel.ts:121-140`) — validate, stamp, and drop
/// one steering request into `run_dir`'s steer queue.
///
/// # Errors
///
/// Returns [`SubagentError::Management`] for an empty message (pi's `throw new Error("steer message
/// must not be empty.")`), or [`SubagentError::Spawn`] for an I/O failure.
pub async fn request_async_steer(
    run_dir: &Path,
    message: &str,
    target_index: Option<usize>,
    source: Option<&str>,
) -> Result<PathBuf, SubagentError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(SubagentError::Management("steer message must not be empty.".to_string()));
    }
    // pi additionally rejects a non-integer/negative `targetIndex`; both are unrepresentable in
    // `Option<usize>`, so that guard has no surviving branch here.
    let request = SteerRequest {
        kind: "steer".to_string(),
        id: next_steer_request_id(),
        ts: now_epoch_millis(),
        message: message.to_string(),
        target_index,
        source: source.map(str::to_string),
    };
    write_steer_request_to_dir(&steer_requests_dir(run_dir), &request).await
}

/// Runner side: pi `enqueueStepSteer` (`control-channel.ts:142-145`) — hand one accepted request to
/// exactly one child by copying it into that child's own inbox with `target_index` pinned.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the child inbox cannot be created or written.
pub async fn enqueue_step_steer(
    run_dir: &Path,
    index: usize,
    request: &SteerRequest,
) -> Result<PathBuf, SubagentError> {
    let pinned = SteerRequest {
        kind: "steer".to_string(),
        target_index: Some(index),
        ..request.clone()
    };
    write_steer_request_to_dir(&step_steer_inbox_dir(run_dir, index), &pinned).await
}

/// pi `consumeSteerRequestsFromDir` (`control-channel.ts:165-185`): read every `*.json` in `dir` in
/// name order, DELETE each one before returning it (so a crash mid-delivery loses a request rather
/// than replaying it), skip anything that fails to parse or validate, and return the survivors
/// sorted by `(ts, id)`.
///
/// A missing directory is an empty list, never an error — the common case is that nobody has ever
/// steered this run.
pub async fn consume_steer_requests_from_dir(dir: &Path) -> Vec<SteerRequest> {
    let mut names: Vec<std::ffi::OsString> = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return Vec::new();
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".json") {
            names.push(name);
        }
    }
    names.sort();

    let mut out: Vec<SteerRequest> = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let parsed = tokio::fs::read(&path)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SteerRequest>(&bytes).ok())
            .filter(|r| r.kind == "steer" && !r.id.trim().is_empty() && !r.message.trim().is_empty());
        // Removal is the consumption primitive and happens whether or not the parse succeeded —
        // a malformed request that stayed on disk would be re-read on every single tick forever.
        if tokio::fs::remove_file(&path).await.is_err() {
            // Lost the race to a concurrent consumer: it already owns this request, so this
            // consumer must NOT also deliver it.
            continue;
        }
        if let Some(mut request) = parsed {
            request.id = request.id.trim().to_string();
            request.message = request.message.trim().to_string();
            out.push(request);
        }
    }
    out.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));
    out
}

/// pi `consumeSteerRequests` (`control-channel.ts:187-189`): drain the RUN-level queue.
pub async fn consume_steer_requests(run_dir: &Path) -> Vec<SteerRequest> {
    consume_steer_requests_from_dir(&steer_requests_dir(run_dir)).await
}

/// Non-consuming read of a pending [`TimeoutRequest`] — [`check_control_inbox_now`]'s sibling,
/// with the identical "the file's existence IS the state, reading never deletes" contract.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the file exists but cannot be read or parsed.
pub async fn check_timeout_inbox_now(
    paths: &RunPaths,
) -> Result<Option<TimeoutRequest>, SubagentError> {
    read_control_request(&timeout_request_path(&paths.run_dir)).await
}

/// Idempotent, at-most-once consumption of a pending [`TimeoutRequest`] — pi
/// `consumeTimeoutRequest` (`control-channel.ts:209` @v0.34.0), and the exact
/// read-then-unconditionally-delete discipline [`consume_interrupt_request`] documents at length.
/// A missing file is `Ok(None)`, never an error; losing the delete race against a concurrent
/// consumer still returns the contents this caller observed.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] for a genuine I/O failure other than "file does not exist".
pub async fn consume_timeout_request(
    paths: &RunPaths,
) -> Result<Option<TimeoutRequest>, SubagentError> {
    let path = timeout_request_path(&paths.run_dir);
    let request = match read_control_request::<TimeoutRequest>(&path).await? {
        Some(request) => request,
        None => return Ok(None),
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) | Err(_) => Ok(Some(request)),
    }
}

/// Non-consuming read of a pending [`StopRequest`] (G77) — [`check_control_inbox_now`]'s and
/// [`check_timeout_inbox_now`]'s sibling, with the identical "the file's existence IS the state,
/// reading never deletes" contract.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the file exists but cannot be read or parsed.
pub async fn check_stop_inbox_now(paths: &RunPaths) -> Result<Option<StopRequest>, SubagentError> {
    read_control_request(&stop_request_path(&paths.run_dir)).await
}

/// Idempotent, at-most-once consumption of a pending [`StopRequest`] (G77) — pi
/// `consumeStopRequest` (`control-channel.ts:519-530` @v0.43.0), and the exact
/// read-then-unconditionally-delete discipline [`consume_interrupt_request`] documents at length. A
/// missing file is `Ok(None)`, never an error; losing the delete race against a concurrent consumer
/// still returns the contents this caller observed (upstream's own comment on that branch reads
/// "Already removed by a concurrent check — still counts as consumed").
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] for a genuine I/O failure other than "file does not exist".
pub async fn consume_stop_request(paths: &RunPaths) -> Result<Option<StopRequest>, SubagentError> {
    let path = stop_request_path(&paths.run_dir);
    let request = match read_control_request::<StopRequest>(&path).await? {
        Some(request) => request,
        None => return Ok(None),
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) | Err(_) => Ok(Some(request)),
    }
}

async fn read_control_request<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<Option<T>, SubagentError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, e))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

// =================================================================================================
// Control-inbox watch primitive (R-SA-082) — reusable building block for runner_main.rs
// =================================================================================================

/// Performs the R-SA-082-mandated **synchronous** control-inbox check: does an [`InterruptRequest`]
/// already exist at `paths.control_inbox` right now, read as a plain, immediate filesystem call
/// with no watcher involved. This is the "one synchronous inbox check immediately at watcher
/// startup to catch a request written in the race window before the watcher attaches" step
/// R-SA-082 requires — callers (this module's own tests, and later `runner_main.rs`) MUST invoke
/// this once before installing any asynchronous watch, never rely on the watch alone to observe a
/// request written concurrently with (or just before) watcher installation.
///
/// Returns the parsed [`InterruptRequest`] if present, `None` if the inbox is currently empty.
/// Does NOT delete the file — reading is non-consuming; only [`consume_interrupt_request`]
/// deletes.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the file exists but cannot be read or parsed as valid
/// JSON.
pub async fn check_control_inbox_now(
    paths: &RunPaths,
) -> Result<Option<InterruptRequest>, SubagentError> {
    match tokio::fs::read(&paths.control_inbox).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, e))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

/// Delete-then-act, idempotent consumption of a pending [`InterruptRequest`] (R-SA-083): the
/// file's existence is the SOLE state, so consumption is "delete the file, then return whether
/// there was anything to consume" — never "read the file, act, then delete" (which would leave a
/// window where a crash after acting but before deleting causes reprocessing). Deleting an
/// already-absent file (a duplicate consumption after the run has already paused, or a race
/// against a concurrent consumer) is silently absorbed: `Ok(None)`, never an error — this is the
/// exact idempotency property R-SA-083 mandates ("a duplicate consumption after the run has
/// already paused MUST be silently absorbed, not re-processed").
///
/// Returns the request's parsed contents (for the caller to act on, e.g. build the `run.paused`
/// event payload) if a file was actually present and removed this call; `None` if there was
/// nothing pending (already consumed, or never existed).
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] only for a genuine I/O failure other than "file does not
/// exist" (e.g. a permissions error) — the delete-then-act ordering means a NotFound on the
/// delete itself is treated as "nothing to consume", not an error.
pub async fn consume_interrupt_request(
    paths: &RunPaths,
) -> Result<Option<InterruptRequest>, SubagentError> {
    // Read-then-delete rather than delete-then-read: we still want the request's CONTENTS (source/
    // reason/ts) for the runner's own `run.paused` event, but the file's *existence* remains the
    // sole authoritative state per R-SA-083 — so a read failure (including NotFound, meaning
    // nothing pending) short-circuits before any delete attempt, and a successful read is
    // unconditionally followed by an at-most-once delete regardless of what happens afterward in
    // the caller (deletion is the consumption primitive, not a side effect of successful
    // processing — a caller must not defer this delete until after it finishes reacting to the
    // request, or a crash mid-reaction would cause reprocessing on restart).
    let request = match check_control_inbox_now(paths).await? {
        Some(request) => request,
        None => return Ok(None),
    };

    match tokio::fs::remove_file(&paths.control_inbox).await {
        Ok(()) => Ok(Some(request)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Lost a race against a concurrent consumer between our read above and this delete —
            // the OTHER consumer's delete already happened. Idempotent absorption: we still
            // observed and return the request's contents (whoever called us gets to react to it
            // exactly once from ITS perspective), but do not treat the missing file as an error.
            Ok(Some(request))
        }
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

/// A real, reusable `notify::PollWatcher`-backed control-inbox watch (R-SA-082's filesystem-
/// notification-mechanism-plus-poll-fallback requirement) over a single run's
/// [`RunPaths::control_inbox`] file, at the fixed ~250ms target interval. `PollWatcher` itself
/// (mirroring `cyrup_resources::theme::ThemeWatcher`'s established convention in this workspace)
/// IS the poll-fallback mechanism — it does not depend on a native OS notification backend being
/// available, so there is no separate "native watch, then fall back to polling" branch to
/// maintain; R-SA-082's two-mechanism requirement is satisfied by (1) this watcher's ongoing poll
/// loop and (2) [`check_control_inbox_now`]'s mandatory synchronous startup check, called BEFORE
/// this function installs its watch (see the "watcher startup race guard" contract on
/// [`check_control_inbox_now`]'s own doc comment — this function does not call it internally, so
/// that ordering is visibly the caller's own responsibility, matching how `runner_main.rs`'s
/// documented main-loop shape performs its own explicit startup check first).
///
/// Sends `()` on `tx` every time the control-inbox path changes (create, write, or delete) —
/// deliberately a bare notification, not the request contents, since the receiver is expected to
/// re-check via [`check_control_inbox_now`]/[`consume_interrupt_request`] rather than trust the
/// watch event's own payload (which `notify` does not guarantee is coalesced 1:1 with actual
/// content changes).
///
/// Returns the live [`notify::PollWatcher`] (which the caller must keep alive for the duration of
/// the watch — dropping it stops the watch) alongside the receiver.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the underlying `notify` watcher cannot be constructed or
/// attached to `paths.control_inbox`'s parent directory.
pub fn watch_control_inbox(
    paths: &RunPaths,
) -> Result<
    (
        notify::PollWatcher,
        tokio::sync::mpsc::UnboundedReceiver<()>,
    ),
    SubagentError,
> {
    use notify::Watcher;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let cfg = notify::Config::default()
        .with_poll_interval(CONTROL_INBOX_POLL_INTERVAL)
        .with_compare_contents(true);
    let mut watcher = notify::PollWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        },
        cfg,
    )
    .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

    // Watch the control/ directory (not the file itself), since the file is created and deleted
    // repeatedly over the run's lifetime — watching a not-yet-existing file path is unreliable
    // across platforms, but the containing directory exists for the run's whole lifetime once the
    // run directory itself is created.
    let watch_target = paths
        .control_inbox
        .parent()
        .unwrap_or(&paths.run_dir)
        .to_path_buf();
    watcher
        .watch(&watch_target, notify::RecursiveMode::NonRecursive)
        .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

    Ok((watcher, rx))
}

/// Target poll interval for [`watch_control_inbox`] (R-SA-082: "target ~250ms").
pub const CONTROL_INBOX_POLL_INTERVAL: Duration = Duration::from_millis(250);

// =================================================================================================
// resume (R-SA-085/086)
// =================================================================================================

/// Which of the two R-SA-085/086 branches a [`resume`] call resolved to — exposed so
/// callers/tests can assert precisely which path was taken, not merely that resume "succeeded".
#[derive(Debug, PartialEq)]
pub enum ResumeOutcome {
    /// R-SA-086: the run was still `Running`; resume steered the live child at `step_index`
    /// rather than spawning anything new. No new process was started by this call — steering an
    /// already-live child is `runner_main.rs`'s in-process concern (it already owns the live
    /// `tokio::process::Child` handle); this function's job stops at *resolving and validating*
    /// which running step is being steered, returning the resolved index for the caller (the
    /// control layer that DOES have a way to signal the specific runner) to act on.
    SteerRunning {
        /// The resolved step index within the run's `steps` list that is being steered. Exactly
        /// one running child is ever selected — never a batch.
        step_index: usize,
    },
    /// R-SA-085: the run was terminal; resume MUST start a genuinely new detached-runner-spawned
    /// child subprocess, seeded from the persisted session-transcript at `session_file`. This
    /// variant carries everything the (later-phase) spawn call site needs to actually perform
    /// that spawn — this function's own job is resolving `session_file` and failing hard if it
    /// is absent, never performing the spawn itself (spawning is `spawn_detached.rs`'s job, a
    /// sibling module not yet written).
    RespawnFromTranscript {
        /// The resolved step index whose persisted transcript is being resumed from.
        step_index: usize,
        /// The persisted session-transcript file path to seed the new child from. Always
        /// `Some` by construction — [`resume`] never returns this variant without one, per
        /// R-SA-085's "MUST fail... if no persisted transcript file exists" (a missing transcript
        /// is [`SubagentError::ResumeNoTranscript`], never a variant carrying `None`).
        session_file: PathBuf,
    },
}

/// Resolves a `resume` request against the run identified by `run_id_token` (R-SA-079/085/086).
///
/// Runs the R-SA-079 reconciliation gate first, then dispatches on the reconciled overall
/// `state`:
///
/// - **`Running`** (R-SA-086): resolves exactly one running step — `requested_step_index` if
///   given (and that step is genuinely `Running`), or auto-selected as the single running step if
///   exactly one exists. A `Pending` (not-yet-started) step is never resumable, matching
///   R-SA-086's explicit exclusion: requesting one, or an index with no running step at all,
///   surfaces as [`SubagentError::AgentNotFound`] carrying a description of the bad index (NOT
///   [`SubagentError::ResumeNoTranscript`], which is reserved specifically for the terminal-
///   revival branch below) — reusing the existing "no such addressable target" variant rather
///   than inventing a fourth error kind for what is the identical failure shape (a resume target
///   that does not exist) as an unknown agent name.
/// - **Terminal (`Complete`/`Failed`) or `Paused`** (R-SA-085): resolves the step to revive —
///   `requested_step_index` if given, else the LAST step in the `steps` list (the most recent
///   attempt, mirroring "resume the run" meaning "continue where it left off" for a run with no
///   explicit step selector) — and requires that step to carry a `session_file`. **Fails outright
///   with [`SubagentError::ResumeNoTranscript`], with NO silent fallback to a fresh/empty
///   session, if no persisted transcript exists** for the resolved step. This is the one piece of
///   this function's contract this task's own instructions single out by name: "fails outright
///   with no silent fallback if no transcript exists" — enforced by returning `Err` rather than a
///   `RespawnFromTranscript { session_file: None, .. }` shape (which the type itself does not
///   even allow to be constructed, see [`ResumeOutcome::RespawnFromTranscript`]'s own doc).
/// - **`Queued`**: a run that has not even started yet has no running child to steer and no
///   terminal transcript to revive from — treated identically to the terminal-with-no-transcript
///   case, `Err(SubagentError::ResumeNoTranscript)`.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] for an unsafe `run_id_token`,
/// [`SubagentError::ResumeNoTranscript`] per the terminal-revival contract above,
/// [`SubagentError::AgentNotFound`] if `requested_step_index` (or the Running-branch auto-
/// selection) does not resolve to a genuinely running step, or a wrapped I/O error from the
/// reconciliation read.
pub async fn resume(
    async_root: &Path,
    results_dir: &Path,
    run_id_token: &str,
    requested_step_index: Option<usize>,
) -> Result<ResumeOutcome, SubagentError> {
    let paths = resolve_run_paths(async_root, results_dir, run_id_token)?;
    let status = reconcile_before_control_op(&paths).await?;

    if status.state == RunState::Running {
        return resolve_running_selection(&status, requested_step_index);
    }

    // G77 — pi `async-resume.ts:406` @v0.43.0: `if (state === "stopped") throw new Error(...)`.
    // Checked AFTER the `Running` branch and BEFORE the terminal-revival branch, exactly where
    // upstream checks it (its own `resolveResumeTarget` reads the reconciled state, refuses a
    // stopped one, and only then looks for a transcript). A stopped run's steps routinely still
    // carry `session_file`s, so without this guard `resolve_terminal_revival` would happily revive
    // a run the user explicitly killed.
    if status.state == RunState::Stopped {
        return Err(SubagentError::ResumeStopped(
            status.run_id.as_str().to_string(),
        ));
    }

    resolve_terminal_revival(&status, requested_step_index)
}

/// R-SA-086: select exactly one currently-`Running` step to steer, never spawning anything.
fn resolve_running_selection(
    status: &RunStatus,
    requested_step_index: Option<usize>,
) -> Result<ResumeOutcome, SubagentError> {
    if let Some(index) = requested_step_index {
        let is_running = status
            .steps
            .get(index)
            .is_some_and(|step| step.status == StepState::Running);
        return if is_running {
            Ok(ResumeOutcome::SteerRunning { step_index: index })
        } else {
            Err(SubagentError::AgentNotFound(format!(
                "step {index} is not currently Running (resume steering requires an active step, \
                 not a Pending/terminal one)"
            )))
        };
    }

    let running_indices: Vec<usize> = status
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.status == StepState::Running)
        .map(|(index, _)| index)
        .collect();

    match running_indices.as_slice() {
        [single] => Ok(ResumeOutcome::SteerRunning { step_index: *single }),
        [] => Err(SubagentError::AgentNotFound(
            "no running step to steer: the run reports Running overall but no individual step is \
             currently Running"
                .to_string(),
        )),
        _multiple => Err(SubagentError::AmbiguousRunId(format!(
            "{} steps are concurrently Running; an explicit step index is required to resume",
            running_indices.len()
        ))),
    }
}

/// R-SA-085: resolve which terminal/paused step to revive from, and hard-fail if it has no
/// persisted transcript — never falling back to a fresh/empty session.
fn resolve_terminal_revival(
    status: &RunStatus,
    requested_step_index: Option<usize>,
) -> Result<ResumeOutcome, SubagentError> {
    let index = match requested_step_index {
        Some(index) => index,
        None => {
            // No explicit selector: resume the most recent attempt, i.e. the last step in the
            // list. A run with zero steps at all (e.g. reconciled straight to Failed before any
            // step ever ran) has nothing to revive from — that is likewise a ResumeNoTranscript
            // condition, not a distinct "empty run" error, since the observable failure mode
            // ("nothing to resume from") is identical either way.
            match status.steps.len().checked_sub(1) {
                Some(last) => last,
                None => return Err(SubagentError::ResumeNoTranscript),
            }
        }
    };

    let step = status
        .steps
        .get(index)
        .ok_or_else(|| SubagentError::AgentNotFound(format!("no step at index {index}")))?;

    match step.session_file.clone() {
        Some(session_file) => Ok(ResumeOutcome::RespawnFromTranscript {
            step_index: index,
            session_file,
        }),
        // R-SA-085's hard-fail contract: NO silent fallback to a fresh/empty session when no
        // persisted transcript file exists for the selected step.
        None => Err(SubagentError::ResumeNoTranscript),
    }
}

// =================================================================================================
// append_step (R-SA-094/095/096)
// =================================================================================================

/// The on-disk pending chain-append request record (func-SA §4.5): `{ id, created_at, steps }`.
/// Written into `<run_dir>/append-requests/<created_at>-<id>.json` (func-SA §4.5's literal
/// filename shape — the creation-order-sortable prefix is what lets the runner's consumption loop
/// establish enqueue order purely from a directory listing, without needing to open and compare
/// every file's own `created_at` field).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainAppendRequest {
    /// A short random id disambiguating two requests enqueued at the identical millisecond
    /// (the filename's own sortable prefix is `created_at`, but two racing enqueues can land the
    /// same millisecond, so the id breaks the tie both in the filename and, redundantly, in this
    /// field for round-trip completeness).
    pub id: String,
    /// Wall-clock enqueue time (epoch milliseconds) — the sort key establishing enqueue order.
    pub created_at: i64,
    /// The steps to append, in the order they should be inserted into the chain's step list.
    pub steps: Vec<RunnerStep>,
}

impl ChainAppendRequest {
    /// Constructs a fresh append request for `steps`, stamped with the current wall-clock time
    /// and a fresh random id.
    #[must_use]
    pub fn new(steps: Vec<RunnerStep>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().as_simple().to_string(),
            created_at: now_epoch_millis(),
            steps,
        }
    }

    /// The on-disk filename for this request (func-SA §4.5: `<created_at>-<id>.json`), whose
    /// lexicographic sort order matches creation order because `created_at` is a fixed-width-
    /// enough decimal epoch-millis prefix that later timestamps always sort after earlier ones
    /// for any two requests enqueued within the lifetime of this process (epoch milliseconds does
    /// not roll over to a shorter decimal representation for many millennia, so lexicographic and
    /// numeric ordering coincide for any realistic pair of values).
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}-{}.json", self.created_at, self.id)
    }
}

/// Every output name a NEW append request's steps would need to avoid colliding with, computed
/// fresh from disk at validation time (R-SA-096: "the reserved-name set MUST be recomputed from
/// disk at validation time, not cached, since concurrent append calls are possible").
///
/// Unions three sources, exactly per this task's own instructions ("validates new output-name
/// bindings against completed-plus-declared-plus-other-pending names"):
/// 1. **Completed steps' outputs** — every already-`Complete`/`Failed` step in `status.steps`
///    that declared an `output` name (read from `status.json`, reconciled first by the caller).
/// 2. **Declared-but-not-yet-completed steps' outputs** — `status.chain_step_count`-implied
///    steps' own declared names are already covered by (1)'s scan over `status.steps` (every
///    step, regardless of its own `StepState`, is included), so this is folded into the same
///    scan rather than a separate pass — restated here because "declared" (present in the step
///    list at all) is a strictly broader set than "completed", and both matter: a still-`Pending`
///    or `Running` step's OWN declared output name is just as reserved as a completed one's.
/// 3. **Other still-pending append requests' output names** — a fresh directory listing of
///    `append_dir`, read and parsed on every call (never cached), covering exactly the
///    two-concurrent-appends race this function exists to close: if request A (not yet consumed
///    by the runner) already reserved name `"plan"`, request B's own validation — even though it
///    started enqueuing microseconds after A, before A's write is necessarily visible to a naive
///    in-memory cache — sees A's reservation because both re-read the SAME directory from disk at
///    validation time.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if `append_dir` exists but cannot be listed, or if an
/// individual request file inside it cannot be read (a request file that fails to PARSE as valid
/// JSON is skipped rather than aborting the whole scan, matching this crate's general "malformed
/// on-disk state degrades gracefully" no-panic convention — a corrupt sibling request must not
/// block a wholly unrelated new request from being validated).
pub async fn reserved_output_names(
    status: &RunStatus,
    append_dir: &Path,
) -> Result<HashSet<String>, SubagentError> {
    let mut reserved = HashSet::new();

    for step in step_output_names(status) {
        reserved.insert(step);
    }

    let mut entries = match tokio::fs::read_dir(append_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(reserved),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };

    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&path).await else {
            continue;
        };
        let Ok(request) = serde_json::from_slice::<ChainAppendRequest>(&bytes) else {
            continue;
        };
        for step in &request.steps {
            for name in runner_step_output_names(step) {
                reserved.insert(name);
            }
        }
    }

    Ok(reserved)
}

/// Extracts every declared output name from `status.steps` — func-SA's `StepStatus` does not
/// itself carry the ORIGINAL step spec's `output`/`collect` name (only the agent/result), so this
/// reads the name the run was ORIGINALLY configured with via... actually, `StepStatus` has no
/// such field. See the doc note on the call site: this crate's `StepStatus` (background/mod.rs)
/// deliberately does not carry a step's declared output name (it is a `RunnerStep`/
/// `SingleStepSpec` concern, not a status-snapshot concern). The authoritative set of declared
/// names therefore must come from the run's ORIGINAL step list, not from `StepStatus` — but this
/// module has no access to that original `Vec<RunnerStep>` from `status.json` alone (the runner
/// config file that carried it is one-shot and deleted after being read, per R-SA-073).
///
/// Given that, this function conservatively returns an EMPTY set for the "completed/declared"
/// half of the union: the append-validation contract this module can honor from `status.json`
/// alone is "does not collide with another still-pending append request's output name"
/// (R-SA-096's third source, fully implemented by [`reserved_output_names`]'s directory scan).
/// Colliding with an EARLIER, already-consumed step's declared name additionally requires the
/// runner itself (which alone holds the live, growing `Vec<RunnerStep>` / `OutputRegistry`) to
/// reject an append at CONSUMPTION time if a later, cross-checked name collision is found — this
/// is `runner_main.rs`'s complementary responsibility (see module docs' deferred-work note), not
/// a silent gap: `append_step`'s ENQUEUE-time validation (this module) and the runner's
/// CONSUME-time re-validation together are what R-SA-096 as a whole describes ("reject... at
/// request-enqueue time... re-validate... at request-enqueue time" — restated once more at
/// consumption is a deliberate belt-and-suspenders the runner performs with the fuller
/// information only it has).
fn step_output_names(_status: &RunStatus) -> Vec<String> {
    Vec::new()
}

/// Every output name a single [`RunnerStep`] declares (`SingleStep`'s `output`,
/// `DynamicGroup`'s `collect`; a `ParallelGroup` has no single aggregate name of its own — each
/// of its inner `SingleStepSpec`s may independently declare its own `output`, scanned
/// recursively).
fn runner_step_output_names(step: &RunnerStep) -> Vec<String> {
    match step {
        RunnerStep::SingleStep(single) => single.output.clone().into_iter().collect(),
        RunnerStep::DynamicGroup(dynamic) => vec![dynamic.collect.clone()],
        RunnerStep::ParallelGroup(parallel) => parallel
            .steps
            .iter()
            .filter_map(|single| single.output.clone())
            .collect(),
        // A root-attachment step's imported output IS referenceable under its `output` name
        // (R-SA-097: pi's `outputName`/`as`), so an appended step naming `{outputs.<that name>}`
        // must see it declared here just like a `SingleStep`'s own `output`.
        RunnerStep::ImportAsyncRoot(spec) => spec.output.clone().into_iter().collect(),
    }
}

/// The observable outcome of an [`append_step`] call.
#[derive(Debug, PartialEq)]
pub enum AppendOutcome {
    /// The request was validated and atomically enqueued at `append_dir/<file_name>`, awaiting
    /// consumption by the runner's own step-execution loop.
    Enqueued {
        /// The enqueued request's on-disk file name (creation-order-sortable).
        file_name: String,
    },
}

/// Validates and enqueues a chain-append request against the run identified by `run_id_token`
/// (R-SA-079/094/095/096).
///
/// Runs the R-SA-079 reconciliation gate first, then re-validates (R-SA-096) that the target run
/// is genuinely `mode: Chain` AND (`state: Running` OR already carries pending unconsumed
/// appends, i.e. `pending_appends > 0` — a run mid-way through consuming a prior append batch is
/// still a valid append target even if its `state` field has not yet ticked back to `Running`
/// from whatever transient value the runner last wrote): any other combination (wrong mode,
/// non-Running with zero pending appends) rejects the request before any file is written.
///
/// Rejects (before writing) any output name in `new_steps` that collides with the reserved set
/// computed fresh from disk by [`reserved_output_names`] — this is the exact
/// two-concurrent-appends race guard: both racing calls re-scan the SAME `append_dir` at their
/// own validation instant, so whichever call's write actually lands first (the loser observes the
/// winner's file in its own re-scan, since the winner's atomic write completes-or-doesn't as a
/// whole per [`write_atomic_json`]'s temp-then-rename guarantee) is the one that succeeds; the
/// other observes the collision and is rejected — **exactly one of two colliding concurrent
/// append calls succeeds**, never both, never neither.
///
/// On successful validation, atomically writes a fresh [`ChainAppendRequest`] into
/// `append_dir` and recomputes `pending_appends` (R-SA-094: "recomputed by directory listing...
/// each time it changes, so it is always consistent with the actual on-disk request files") by
/// listing `append_dir` after the write, persisting the updated count into `status.json` via the
/// normal atomic-write path (a `touch`-only status update — `chain_step_count` itself is NOT
/// incremented here, since the steps are not yet consumed; only the runner's own consumption,
/// R-SA-095, grows `chain_step_count`).
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] for an unsafe `run_id_token`,
/// [`SubagentError::MalformedSettings`] if the target run is not `mode: Chain` or is not
/// (`Running` or carrying pending appends), or [`SubagentError::AcceptanceRejected`] (reused here
/// as the "request rejected by validation" shape — an output-name collision is, like an
/// acceptance rejection, a validation failure discovered before any state mutation, not a
/// filesystem/protocol-level error) if any new step's output name collides with the reserved set.
pub async fn append_step(
    async_root: &Path,
    results_dir: &Path,
    run_id_token: &str,
    new_steps: Vec<RunnerStep>,
) -> Result<AppendOutcome, SubagentError> {
    let paths = resolve_run_paths(async_root, results_dir, run_id_token)?;

    tokio::fs::create_dir_all(&paths.append_dir)
        .await
        .map_err(SubagentError::Spawn)?;

    // R-SA-096's "re-scan disk, not cache" requirement is necessary but NOT sufficient to close
    // the two-concurrent-appends race on its own: two callers can both re-scan the SAME (still-
    // empty-of-each-other's-write) directory an instant apart and both conclude a name is free
    // before either has written anything. Re-scanning disk closes the "stale in-memory cache"
    // failure mode; it does not by itself provide mutual exclusion between the scan and the
    // write. Since the orchestrator and a background runner are genuinely separate OS processes
    // (this crate's whole mandated mechanism, func-SA §1.1) communicating only via files, an
    // in-process `tokio::sync::Mutex` cannot serialize this critical section either — only a
    // filesystem-level primitive can. `acquire_append_lock` below provides that: an atomic,
    // `O_EXCL`-equivalent exclusive file creation (`OpenOptions::create_new`) used as a real
    // cross-process advisory lock, held only across the validate-then-write critical section
    // (never across the initial reconciliation read above, which is read-only and does not need
    // to be serialized).
    let _lock = acquire_append_lock(&paths.append_dir).await?;

    let status = reconcile_before_control_op(&paths).await?;

    let has_pending_appends = status.pending_appends.unwrap_or(0) > 0;
    if status.mode != RunMode::Chain {
        return Err(SubagentError::MalformedSettings(format!(
            "append-step requires mode=Chain, got {:?}",
            status.mode
        )));
    }
    if status.state != RunState::Running && !has_pending_appends {
        return Err(SubagentError::MalformedSettings(format!(
            "append-step requires state=Running (or pending unconsumed appends), got {:?}",
            status.state
        )));
    }

    let reserved = reserved_output_names(&status, &paths.append_dir).await?;
    let new_names: Vec<String> = new_steps.iter().flat_map(runner_step_output_names).collect();
    for name in &new_names {
        if reserved.contains(name) {
            return Err(SubagentError::AcceptanceRejected(format!(
                "output name {name:?} collides with an already-completed step, an already-\
                 declared step, or another still-pending append request"
            )));
        }
    }
    // Also guard against a collision WITHIN this single request's own steps (two of the new steps
    // declaring the same output name) — not a concurrency race, just a plain validation error,
    // but it is the identical failure mode from the runner's downstream `OutputRegistry`
    // perspective (a duplicate name is a duplicate name regardless of which request introduced
    // the second occurrence), so it is rejected identically rather than silently accepted here
    // and only discovered later at consumption time.
    let mut seen_within_request = HashSet::new();
    for name in &new_names {
        if !seen_within_request.insert(name.clone()) {
            return Err(SubagentError::AcceptanceRejected(format!(
                "output name {name:?} is declared by more than one step within the same append \
                 request"
            )));
        }
    }

    let request = ChainAppendRequest::new(new_steps);
    let file_name = request.file_name();
    let request_path = paths.append_dir.join(&file_name);
    write_atomic_json(&request_path, &request)
        .await
        .map_err(SubagentError::Spawn)?;

    // R-SA-094: recompute pending_appends by directory listing, persist into status.json.
    let pending_count = count_pending_appends(&paths.append_dir).await?;
    let mut status = status;
    status.pending_appends = Some(pending_count);
    status.touch();
    write_atomic_json(&paths.status, &status)
        .await
        .map_err(SubagentError::Spawn)?;

    // `_lock` is dropped here (end of scope), releasing the exclusive lock file for the next
    // waiting caller — an explicit `drop` is unnecessary but the lock's whole purpose is spent by
    // this point, so leaving it to the end of the block (rather than a manual early drop) keeps
    // the entire validate-through-write sequence inside the critical section, including the
    // `pending_appends` recompute-and-persist step, which must also observe/produce a consistent
    // view relative to any other racing append.
    Ok(AppendOutcome::Enqueued { file_name })
}

/// A real, cross-process advisory lock over one run's `append_dir`, implemented as an atomically-
/// created lock file (`append_dir/.lock`) using `OpenOptions::create_new` — which maps to a
/// single atomic `open(..., O_CREAT | O_EXCL)` syscall on POSIX, so "did I just create this file"
/// is itself race-free between any number of concurrent callers, in-process or cross-process
/// (exactly the guarantee this critical section needs, since the orchestrator and a detached
/// runner are genuinely separate OS processes per this crate's mandated mechanism).
///
/// Holding the returned guard keeps the lock file in place; dropping it removes the lock file,
/// releasing the lock for the next waiter. Acquisition retries with a short bounded backoff
/// (rather than failing immediately on first contention) since the whole point of this primitive
/// is to let racing concurrent appends serialize through the critical section rather than
/// spuriously reject one another — the OUTCOME of "exactly one wins" is still enforced by the
/// output-name validation inside the lock, not by lock-acquisition failure.
struct AppendLockGuard {
    lock_path: PathBuf,
}

impl Drop for AppendLockGuard {
    fn drop(&mut self) {
        // Best-effort removal: if this somehow fails (e.g. the run directory was concurrently
        // torn down), there is nothing more this drop can safely do — a stale lock file would
        // only affect a hypothetical future append against an already-gone run directory, which
        // is a non-issue in practice, and panicking from `Drop` is never acceptable in this
        // crate's no-panic policy.
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// Maximum number of acquisition attempts before giving up (matches the bounded-retry philosophy
/// already established by `atomic::rename_with_backoff`, restated here for the append-lock's own
/// distinct contention window).
const APPEND_LOCK_MAX_ATTEMPTS: u32 = 100;

/// Base delay between append-lock acquisition attempts.
const APPEND_LOCK_RETRY_DELAY: Duration = Duration::from_millis(5);

async fn acquire_append_lock(append_dir: &Path) -> Result<AppendLockGuard, SubagentError> {
    let lock_path = append_dir.join(".lock");
    for attempt in 0..APPEND_LOCK_MAX_ATTEMPTS {
        let lock_path_for_task = lock_path.clone();
        let created = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path_for_task)
        })
        .await
        .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

        match created {
            Ok(_file) => return Ok(AppendLockGuard { lock_path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if attempt + 1 >= APPEND_LOCK_MAX_ATTEMPTS {
                    return Err(SubagentError::Spawn(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "append-lock acquisition timed out under sustained contention",
                    )));
                }
                tokio::time::sleep(APPEND_LOCK_RETRY_DELAY).await;
            }
            Err(e) => return Err(SubagentError::Spawn(e)),
        }
    }
    // Unreachable in practice (the loop above always either returns or errors before exhausting
    // its iteration count), but a plain, non-panicking fallback keeps this function total without
    // relying on the compiler's own loop-exhaustion analysis.
    Err(SubagentError::Spawn(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "append-lock acquisition exhausted its retry budget",
    )))
}

/// R-SA-094: recomputes the pending-append count by listing `append_dir` — never a separately
/// maintained counter that could drift from the actual on-disk request files. Counts every
/// `*.json` entry, matching [`reserved_output_names`]'s own filter.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if `append_dir` exists but cannot be listed. A missing
/// `append_dir` (nothing ever enqueued yet) is not an error — returns `0`.
pub async fn count_pending_appends(append_dir: &Path) -> Result<usize, SubagentError> {
    let mut entries = match tokio::fs::read_dir(append_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };
    let mut count = 0usize;
    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            count += 1;
        }
    }
    Ok(count)
}

/// Lists every currently-pending [`ChainAppendRequest`] in `append_dir`, sorted by creation order
/// (R-SA-095: "sorted by creation order") — the exact ordering the runner's own consumption loop
/// (`runner_main.rs`, later phase) MUST list/read/delete in. Provided here as the shared,
/// reusable listing primitive so the runner's consumption implementation and this module's own
/// [`reserved_output_names`]/tests read the SAME requests in the SAME order rather than each
/// re-deriving the sort independently.
///
/// Sorting is by filename (which is `<created_at>-<id>.json`, so a lexicographic sort on the
/// filename IS a creation-order sort — see [`ChainAppendRequest::file_name`]'s own doc for why
/// that coincidence holds), not by re-parsing each file's own `created_at` field, so a request
/// whose contents fail to parse can still be positioned correctly (and then skipped, not
/// silently reordered to the end) by a caller that wants to report a diagnostic for it.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if `append_dir` exists but cannot be listed. A missing
/// `append_dir` returns an empty vector, not an error.
pub async fn list_pending_appends(
    append_dir: &Path,
) -> Result<Vec<(PathBuf, Option<ChainAppendRequest>)>, SubagentError> {
    let mut entries = match tokio::fs::read_dir(append_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };

    let mut found = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let parsed = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<ChainAppendRequest>(&bytes).ok(),
            Err(_) => None,
        };
        found.push((path, parsed));
    }
    found.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(found)
}

// =================================================================================================
// Root-attachment polling (R-SA-097)
// =================================================================================================

/// The observable outcome of one [`poll_root_attachment`] tick.
#[derive(Debug, PartialEq)]
pub enum AttachmentPoll {
    /// The target run's terminal [`ResultFile`] was found — authoritative, regardless of what its
    /// `status.json` currently claims (R-SA-097: "MUST treat presence of the target's ResultFile
    /// as authoritative regardless of what its status.json currently says").
    Ready(ResultFile),
    /// The target run has not reached a terminal, result-bearing state yet — keep polling.
    StillWaiting,
    /// The target's `status.json` went terminal, but its `ResultFile` is still missing, AND the
    /// grace period (R-SA-097: "target ~1000ms") since first observing that terminal status has
    /// already elapsed — this covers the same non-atomic status-then-result write ordering as
    /// R-SA-077, but only declares failure once the grace window has genuinely passed, never on
    /// the very first observation of "terminal but no result yet".
    Failed,
}

/// Grace period after observing a target run's `status.json` go terminal before declaring
/// "terminal but result missing" a failure (R-SA-097: "target ~1000ms").
pub const ROOT_ATTACHMENT_GRACE: Duration = Duration::from_millis(1000);

/// Fixed poll interval for [`poll_root_attachment`] (R-SA-097: "target ~500ms").
pub const ROOT_ATTACHMENT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Performs ONE poll tick against `target_paths` (R-SA-097): attaching a new chain's first step
/// to another run's (still-running or already-terminal) result is done by POLLING the target's
/// `status.json`/`ResultFile` at a fixed interval — never by spawning any process. This function
/// is the single-tick primitive; the caller (a later phase's chain-graph root-attachment step
/// dispatcher) is responsible for the surrounding `tokio::time::interval`-driven loop at
/// [`ROOT_ATTACHMENT_POLL_INTERVAL`] and for tracking `terminal_first_observed_at` across
/// repeated calls to this function (threaded through as the `terminal_first_observed_at`
/// parameter/return pair below) so the [`ROOT_ATTACHMENT_GRACE`] window is measured from the
/// FIRST observation of a terminal `status.json`, not re-started on every tick.
///
/// Returns the poll outcome plus the (possibly newly set) `terminal_first_observed_at` timestamp
/// for the caller to pass into its next tick.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] only for a genuine I/O failure reading either file (not
/// "file does not exist", which is the normal `StillWaiting` case for a run that has not started
/// writing its status yet yet).
pub async fn poll_root_attachment(
    target_paths: &RunPaths,
    terminal_first_observed_at: Option<i64>,
) -> Result<(AttachmentPoll, Option<i64>), SubagentError> {
    if let Some(result) = read_result_file(&target_paths.result).await? {
        return Ok((AttachmentPoll::Ready(result), terminal_first_observed_at));
    }

    let status = read_status_file(&target_paths.status).await?;
    let now = now_epoch_millis();

    match status {
        Some(status) if status.state.is_terminal() => {
            let first_observed = terminal_first_observed_at.unwrap_or(now);
            let elapsed_ms = now.saturating_sub(first_observed);
            if elapsed_ms >= i64::try_from(ROOT_ATTACHMENT_GRACE.as_millis()).unwrap_or(i64::MAX) {
                Ok((AttachmentPoll::Failed, Some(first_observed)))
            } else {
                Ok((AttachmentPoll::StillWaiting, Some(first_observed)))
            }
        }
        // Running/Queued/Paused, or no status.json written yet at all: keep waiting, and reset
        // the terminal-first-observed marker since the target is (as of this tick) not terminal.
        _ => Ok((AttachmentPoll::StillWaiting, None)),
    }
}

// =================================================================================================
// wait_for_imported_async_root — R-SA-097's polling loop around poll_root_attachment
// =================================================================================================

/// One imported async-root outcome, synthesized as a new chain's first step's result (R-SA-097; pi
/// `chain-root-attachment.ts`'s `ImportedAsyncRootResult`). This is the loop-level product of
/// polling another already-launched run to a terminal state and reading back the child result the
/// attachment targeted; the background runner
/// (`background/runner_main.rs::run_inner`) folds it into a [`SingleResult`] for THIS chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedAsyncRootResult {
    /// The imported step's agent name — the target child's own agent, falling back to the target
    /// run's top-level agent, then to the caller-supplied display name.
    pub agent: String,
    /// The imported final output text (or, for a failure, the error/summary that stands in for it,
    /// mirroring pi's `output || error || ""` fallback).
    pub output: String,
    /// Whether the imported root completed successfully.
    pub success: bool,
    /// `0` on success, `1` otherwise (mirrors pi's `exitCode`).
    pub exit_code: i32,
    /// The failure message, present only when `!success`.
    pub error: Option<String>,
    /// The target child's persisted session-transcript path, if any.
    pub session_file: Option<PathBuf>,
    /// The model the target child's winning attempt used, if recorded.
    pub model: Option<ModelId>,
    /// Every model the target child attempted, if recorded.
    pub attempted_models: Vec<ModelId>,
    /// The target child's validated structured output, if any.
    pub structured_output: Option<serde_json::Value>,
}

/// Poll `target_paths` at `poll_interval` until the attached async root goes terminal, then return
/// its outcome synthesized as one [`ImportedAsyncRootResult`] (R-SA-097; pi
/// `waitForImportedAsyncRoot`). This is the loop wrapping [`poll_root_attachment`]'s single-tick
/// primitive: `AttachmentPoll::Ready` (the target's terminal `ResultFile` is authoritative) →
/// [`build_imported_result`]; `AttachmentPoll::Failed` (terminal `status.json` but no result file
/// even after the grace window) → [`output_from_terminal_status`]; `AttachmentPoll::StillWaiting` →
/// sleep and poll again.
///
/// `run_id` is the target's run-id token (used only for the diagnostic messages pi's own fallbacks
/// embed); `index` selects which child within a multi-child target result to import; `fallback_agent`
/// is the display agent name used only when neither the result file nor the status file names one.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] for a genuine I/O failure reading the target's status/result
/// files, or if the target run's directory does not exist at all while it is still non-terminal
/// (pi's `directory does not exist` guard — a target that never started).
pub async fn wait_for_imported_async_root(
    target_paths: &RunPaths,
    run_id: &str,
    index: usize,
    fallback_agent: &str,
    poll_interval: Duration,
) -> Result<ImportedAsyncRootResult, SubagentError> {
    let mut terminal_first_observed_at: Option<i64> = None;
    loop {
        let (outcome, next_observed) =
            poll_root_attachment(target_paths, terminal_first_observed_at).await?;
        match outcome {
            AttachmentPoll::Ready(result) => {
                return Ok(build_imported_result(&result, run_id, index, fallback_agent));
            }
            AttachmentPoll::Failed => {
                // The target's status.json went terminal but no ResultFile ever landed (past the
                // grace window). Re-read the (still-present) status so the synthesized failure can
                // name the target step's own agent/error, exactly as pi's `outputFromTerminalStatus`
                // reads them off the terminal status.
                let status = read_status_file(&target_paths.status).await?;
                return Ok(output_from_terminal_status(
                    status.as_ref(),
                    target_paths,
                    run_id,
                    index,
                    fallback_agent,
                ));
            }
            AttachmentPoll::StillWaiting => {
                terminal_first_observed_at = next_observed;
                // pi's `!status && !fs.existsSync(root.asyncDir)` guard: a target that is not
                // terminal AND whose run directory does not exist never started — surface that as a
                // hard error rather than polling forever against a directory that will never appear.
                if !tokio::fs::try_exists(&target_paths.run_dir).await.unwrap_or(false) {
                    return Err(SubagentError::Spawn(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Attached async root '{run_id}' directory does not exist: {}",
                            target_paths.run_dir.display()
                        ),
                    )));
                }
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}

/// The terminal-state classification pi's `resultState` performs, adapted to this crate's
/// [`ResultFile`]/[`SingleResult`] shapes (which carry an `exit_code` rather than pi's `success`
/// boolean per child): a child that exited `0` is `Complete`; a child that exited non-zero is
/// `Paused` iff the whole run is paused, else `Failed`; absent a child, the run's own
/// state/`success` decides.
///
/// G77 — the two `stopped` branches are upstream's own, in upstream's own order
/// (`chain-root-attachment.ts:85-93` @v0.43.0):
/// 1. `if (child?.stopped === true) return "stopped";` — FIRST, ahead of the child's `success`
///    check, so a stopped child is never re-read as complete-or-failed;
/// 2. `if (child?.success === false) return result.state === "stopped" ? "stopped" : result.state
///    === "paused" ? "paused" : "failed";` — the non-zero-exit child inherits the RUN's stop before
///    it inherits its pause;
/// 3. the childless fallthrough passes `"stopped"` straight through alongside
///    `complete`/`failed`/`paused` (`:90`).
fn imported_state(result: &ResultFile, child: Option<&SingleResult>) -> RunState {
    if let Some(child) = child {
        if child.stopped {
            return RunState::Stopped;
        }
        if child.exit_code == 0 {
            return RunState::Complete;
        }
        return match result.state {
            RunState::Stopped => RunState::Stopped,
            RunState::Paused => RunState::Paused,
            _ => RunState::Failed,
        };
    }
    match result.state {
        RunState::Complete => RunState::Complete,
        RunState::Failed => RunState::Failed,
        RunState::Paused => RunState::Paused,
        RunState::Stopped => RunState::Stopped,
        RunState::Queued | RunState::Running => {
            if result.success { RunState::Complete } else { RunState::Failed }
        }
    }
}

/// Build the imported result from the target's authoritative terminal [`ResultFile`] (pi's
/// `buildImportedResult`): pick the child at `index`, classify it, and project its
/// agent/output/error/session/model fields into an [`ImportedAsyncRootResult`].
fn build_imported_result(
    result: &ResultFile,
    run_id: &str,
    index: usize,
    fallback_agent: &str,
) -> ImportedAsyncRootResult {
    let child = result.results.get(index);
    let success = imported_state(result, child) == RunState::Complete;

    let agent = child
        .map(|c| c.agent.clone())
        .filter(|a| !a.is_empty())
        .or_else(|| Some(result.agent.clone()).filter(|a| !a.is_empty()))
        .unwrap_or_else(|| fallback_agent.to_string());

    let output_text = child.and_then(|c| c.final_output.clone()).unwrap_or_default();
    let error = child.and_then(|c| c.error.clone()).or_else(|| {
        if success {
            None
        } else {
            Some(format!(
                "Attached async root {run_id} did not complete successfully."
            ))
        }
    });
    // pi's `success ? output : (output || error || "")`: keep the real output whenever it exists
    // (or the run succeeded); only substitute the error text for an empty output on failure.
    let output = if success || !output_text.is_empty() {
        output_text
    } else {
        error.clone().unwrap_or_default()
    };

    ImportedAsyncRootResult {
        agent,
        output,
        success,
        exit_code: i32::from(!success),
        error,
        // `SingleResult` carries no per-child session file, so the target run's top-level
        // `session_file` is the only faithful source here (a single-mode target's child session IS
        // the run's session; for a multi-child target it is absent, matching pi's own
        // `child?.sessionFile ?? … ?? status?.sessionFile` chain collapsing to the run session).
        session_file: result.session_file.clone(),
        model: child.and_then(|c| c.model.clone()),
        attempted_models: child.map(|c| c.attempted_models.clone()).unwrap_or_default(),
        structured_output: child.and_then(|c| c.structured_output.clone()),
    }
}

/// Build a failed imported result when the target went terminal but never wrote a `ResultFile` (pi's
/// `outputFromTerminalStatus`): name the target step's agent/error off the terminal `status.json`
/// when available, else fall back to a "ended without a result file" diagnostic naming the missing
/// result path.
fn output_from_terminal_status(
    status: Option<&RunStatus>,
    target_paths: &RunPaths,
    run_id: &str,
    index: usize,
    fallback_agent: &str,
) -> ImportedAsyncRootResult {
    let step = status.and_then(|s| s.steps.get(index));
    let agent = step
        .map(|s| s.agent.clone())
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| fallback_agent.to_string());
    let message = step.and_then(|s| s.error.clone()).unwrap_or_else(|| {
        format!(
            "Attached async root {run_id} ended without a result file at {}.",
            target_paths.result.display()
        )
    });
    ImportedAsyncRootResult {
        agent,
        output: message.clone(),
        success: false,
        exit_code: 1,
        error: Some(message),
        session_file: step.and_then(|s| s.session_file.clone()),
        model: None,
        attempted_models: Vec::new(),
        structured_output: None,
    }
}

// =================================================================================================
// Time helper (mirrors background/mod.rs's private now_epoch_millis; duplicated rather than
// exposed cross-module since arch-SA keeps each background/*.rs file's own timestamp policy
// self-contained rather than introducing a shared time-utility module for one three-line helper)
// =================================================================================================

fn now_epoch_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

// =================================================================================================
// Tests
// =================================================================================================

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::spawn::chain_graph::SingleStepSpec;
    use std::sync::Arc;

    // ---- steer queue ordering ----

    /// Steer messages must be delivered in the order the parent produced them, even when several
    /// are queued inside the same millisecond — which is the NORMAL case for two calls in a loop,
    /// not an exotic one.
    ///
    /// Before the monotonic sequence in [`next_steer_request_id`], same-`ts` requests tied on a
    /// random UUID, so the consumer's order was a coin flip: a user sending two corrections in
    /// quick succession could have the second applied before the first. It also made
    /// `tests/steer_delivery_integration.rs` fail about one run in four, which is the more
    /// expensive symptom — an intermittently red suite makes every future green meaningless.
    ///
    /// Twenty requests in a tight loop makes a random tiebreak essentially impossible to pass
    /// (20! orderings), so this does not rely on getting unlucky to detect a regression.
    #[tokio::test]
    async fn steer_requests_queued_in_the_same_millisecond_are_consumed_in_creation_order() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

        let expected: Vec<String> = (0..20).map(|i| format!("guidance-{i:02}")).collect();
        for message in &expected {
            request_async_steer(&run_dir, message, None, Some("steer-action"))
                .await
                .expect("queued");
        }

        let drained = consume_steer_requests(&run_dir).await;
        let delivered: Vec<String> = drained.into_iter().map(|r| r.message).collect();
        assert_eq!(
            delivered, expected,
            "guidance must be delivered in the order it was produced; a `ts`-only sort leaves \
             same-millisecond requests in random order"
        );
    }

    /// The sequence is only the TIEBREAK — `ts` still dominates, exactly as upstream sorts
    /// (`consumeSteerRequestsFromDir`, `control-channel.ts:437`, over a `ts`-prefixed file name).
    /// A request with an older `ts` sorts first even when its id was minted later, which is what
    /// keeps ordering correct across processes, where the per-process counter means nothing.
    #[tokio::test]
    async fn an_older_timestamp_still_wins_over_a_later_minted_sequence() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_dir = dir.path().join("run");
        let queue = steer_requests_dir(&run_dir);

        // Minted in ascending-sequence order, written with DESCENDING timestamps.
        let first_id = next_steer_request_id();
        let second_id = next_steer_request_id();
        assert!(first_id < second_id, "the sequence must be ascending");

        for (id, ts, message) in [
            (first_id, 2_000_i64, "newer-ts-earlier-seq"),
            (second_id, 1_000_i64, "older-ts-later-seq"),
        ] {
            write_steer_request_to_dir(
                &queue,
                &SteerRequest {
                    kind: "steer".to_string(),
                    id,
                    ts,
                    message: message.to_string(),
                    target_index: None,
                    source: None,
                },
            )
            .await
            .expect("written");
        }

        let delivered: Vec<String> = consume_steer_requests(&run_dir)
            .await
            .into_iter()
            .map(|r| r.message)
            .collect();
        assert_eq!(
            delivered,
            vec!["older-ts-later-seq".to_string(), "newer-ts-earlier-seq".to_string()],
            "`ts` is the primary key; the sequence only breaks ties within one millisecond"
        );
    }

    fn temp_roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        (dir, async_root, results_dir)
    }

    async fn write_running_status(
        paths: &RunPaths,
        run_id: &RunId,
        mode: RunMode,
        pid: Option<u32>,
        steps: Vec<super::super::StepStatus>,
    ) -> RunStatus {
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        let mut status = RunStatus::queued(run_id.clone(), mode, pid);
        status.advance_state(RunState::Running).expect("Queued -> Running");
        status.steps = steps;
        write_atomic_json(&paths.status, &status).await.expect("write status");
        status
    }

    fn single_step(agent: &str, output: Option<&str>) -> SingleStepSpec {
        SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task: format!("do {agent}"),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: output.map(str::to_string),
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // G77 — the `stop` control verb (pi `stopAsyncRun` / `StopRequest` / `consumeStopRequest`)
    // ---------------------------------------------------------------------------------------

    /// `stop` writes a real `control/stop.json` for a `Running` run, and `consume_stop_request`
    /// reads-then-deletes it exactly once (pi `consumeStopRequest`, `control-channel.ts:519-530`).
    #[tokio::test]
    async fn stop_writes_a_real_stop_request_that_is_consumed_exactly_once() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("stoprun00001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, Some(4242), Vec::new()).await;

        assert_eq!(
            stop(&async_root, &results_dir, run_id.as_str(), "stop-action", None)
                .await
                .expect("stop resolves"),
            StopOutcome::Requested
        );

        // The file lands at pi's own path, with pi's own discriminant and source.
        let path = stop_request_path(&paths.run_dir);
        assert_eq!(path, paths.run_dir.join("control").join("stop.json"));
        let raw: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&path).await.expect("stop.json exists"))
                .expect("valid json");
        assert_eq!(raw["type"], serde_json::json!("stop"));
        assert_eq!(raw["source"], serde_json::json!("stop-action"));

        // Non-consuming read leaves it in place…
        assert!(check_stop_inbox_now(&paths).await.expect("read").is_some());
        assert!(path.exists());
        // …consumption removes it and is idempotent thereafter.
        let consumed = consume_stop_request(&paths).await.expect("consume").expect("was pending");
        assert_eq!(consumed.kind, "stop");
        assert!(!path.exists());
        assert!(consume_stop_request(&paths).await.expect("second consume").is_none());
    }

    /// pi `stopAsyncRun`'s actionability guard (`async-stop-action.ts:41`): only `running` or
    /// `queued`. A `Paused` run is refused — NOT silently absorbed the way a duplicate `interrupt`
    /// is — and nothing is written.
    #[tokio::test]
    async fn stop_refuses_a_run_that_is_neither_running_nor_queued() {
        let (_dir, async_root, results_dir) = temp_roots();

        // Queued IS stoppable.
        let queued_id = RunId::from_token("stopqueued01");
        let queued_paths = RunPaths::for_run(&async_root, &results_dir, &queued_id);
        tokio::fs::create_dir_all(&queued_paths.run_dir).await.expect("mkdir");
        write_atomic_json(
            &queued_paths.status,
            &RunStatus::queued(queued_id.clone(), RunMode::Single, None),
        )
        .await
        .expect("write status");
        assert_eq!(
            stop(&async_root, &results_dir, queued_id.as_str(), "stop-action", None)
                .await
                .expect("stop resolves"),
            StopOutcome::Requested
        );

        // Paused is NOT.
        let paused_id = RunId::from_token("stoppaused01");
        let paused_paths = RunPaths::for_run(&async_root, &results_dir, &paused_id);
        let mut paused = write_running_status(
            &paused_paths,
            &paused_id,
            RunMode::Single,
            Some(11),
            Vec::new(),
        )
        .await;
        paused.advance_state(RunState::Paused).expect("Running -> Paused");
        write_atomic_json(&paused_paths.status, &paused).await.expect("write");
        assert_eq!(
            stop(&async_root, &results_dir, paused_id.as_str(), "stop-action", None)
                .await
                .expect("stop resolves"),
            StopOutcome::NotStoppable
        );
        assert!(
            !stop_request_path(&paused_paths.run_dir).exists(),
            "a refused stop must not leave a request file behind"
        );
    }

    /// G77 — pi `async-resume.ts:406` @v0.43.0: a stopped run is NOT resumable, and the refusal is
    /// its own error with its own verbatim sentence, never the no-transcript one. The run below
    /// deliberately HAS a persisted transcript, which is exactly the case a
    /// `ResumeNoTranscript`-only implementation would happily revive.
    #[tokio::test]
    async fn resume_refuses_a_stopped_run_even_when_a_transcript_exists() {
        let (dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("stopresume01");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let transcript = dir.path().join("session.jsonl");
        tokio::fs::write(&transcript, b"{}\n").await.expect("write transcript");

        let mut step = super::super::StepStatus::pending("worker");
        step.status = StepState::Stopped;
        step.session_file = Some(transcript.clone());
        let mut status =
            write_running_status(&paths, &run_id, RunMode::Single, Some(9), vec![step]).await;
        status.advance_state(RunState::Stopped).expect("Running -> Stopped");
        write_atomic_json(&paths.status, &status).await.expect("write");

        let err = resume(&async_root, &results_dir, run_id.as_str(), None)
            .await
            .expect_err("a stopped run must not be resumable");
        assert!(
            matches!(err, SubagentError::ResumeStopped(ref id) if id == run_id.as_str()),
            "the refusal must be its own variant, not ResumeNoTranscript: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Async run 'stopresume01' was stopped and cannot be resumed. Start a new run instead."
        );
    }

    /// G77 — `imported_state` (pi `resultState`, `chain-root-attachment.ts:85-93`): a child's own
    /// `stopped` flag short-circuits ahead of its exit code, and a failing child inherits the run's
    /// stop before it inherits a pause.
    #[test]
    fn imported_state_propagates_stopped_ahead_of_exit_code_and_pause() {
        fn result_with(state: RunState, child: crate::exec::SingleResult) -> ResultFile {
            ResultFile {
                id: RunId::from_token("importstop01"),
                run_id: RunId::from_token("importstop01"),
                agent: "worker".to_string(),
                mode: RunMode::Single,
                state,
                success: false,
                cwd: PathBuf::from("/tmp"),
                session_file: None,
                results: vec![child],
            }
        }
        fn child(exit_code: i32, stopped: bool) -> crate::exec::SingleResult {
            crate::exec::SingleResult {
                agent: "worker".to_string(),
                task: String::new(),
                exit_code,
                usage: cyrup_core::Usage::default(),
                model: None,
                attempted_models: Vec::new(),
                model_attempts: Vec::new(),
                final_output: None,
                structured_output: None,
                acceptance: None,
                detached: false,
                interrupted: false,
                timed_out: false,
                stopped,
                process_signal: None,
                error: None,
                saved_output_path: None,
                tool_calls: Vec::new(),
                output_truncated: false,
                control_events: Vec::new(),
                progress: None,
            }
        }

        // `child.stopped === true` wins even over a clean exit (`:87` precedes `:88`).
        let r = result_with(RunState::Complete, child(0, true));
        assert_eq!(imported_state(&r, r.results.first()), RunState::Stopped);

        // A failing child inherits the run's stop (`:89`'s `result.state === "stopped"` arm),
        // ahead of the pause arm.
        let r = result_with(RunState::Stopped, child(1, false));
        assert_eq!(imported_state(&r, r.results.first()), RunState::Stopped);

        // …and with NO child at all, the run's stop passes straight through (`:90`).
        let mut r = result_with(RunState::Stopped, child(1, false));
        r.results.clear();
        assert_eq!(imported_state(&r, None), RunState::Stopped);

        // The pre-G77 relations are untouched.
        let r = result_with(RunState::Paused, child(1, false));
        assert_eq!(imported_state(&r, r.results.first()), RunState::Paused);
        let r = result_with(RunState::Failed, child(1, false));
        assert_eq!(imported_state(&r, r.results.first()), RunState::Failed);
    }

    // ---------------------------------------------------------------------------------------
    // safe-token validation
    // ---------------------------------------------------------------------------------------

    #[test]
    fn validate_safe_token_rejects_empty() {
        assert!(validate_safe_token("").is_err());
    }

    #[test]
    fn validate_safe_token_rejects_path_separators() {
        assert!(validate_safe_token("abc/def").is_err());
        assert!(validate_safe_token("abc\\def").is_err());
    }

    #[test]
    fn validate_safe_token_rejects_dotdot() {
        assert!(validate_safe_token("../etc/passwd").is_err());
        assert!(validate_safe_token("..").is_err());
    }

    #[test]
    fn validate_safe_token_accepts_plain_hex_token() {
        assert!(validate_safe_token("deadbeefcafef00d").is_ok());
    }

    #[test]
    fn resolve_run_paths_rejects_traversal_before_any_filesystem_access() {
        let async_root = PathBuf::from("/tmp/does-not-exist-async-root");
        let results_dir = PathBuf::from("/tmp/does-not-exist-results-dir");
        let result = resolve_run_paths(&async_root, &results_dir, "../../etc/passwd");
        assert!(matches!(result, Err(SubagentError::UnsafePathToken(_))));
    }

    // ---------------------------------------------------------------------------------------
    // interrupt: idempotency (second interrupt on an already-pausing run is a no-op)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn interrupt_on_running_run_delivers_and_writes_control_inbox_file() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;

        let outcome = interrupt(&async_root, &results_dir, "run00001", "user", None)
            .await
            .expect("interrupt succeeds");

        assert_eq!(outcome, InterruptOutcome::Delivered);
        assert!(
            tokio::fs::try_exists(&paths.control_inbox).await.expect("check exists"),
            "control inbox file must exist after a delivered interrupt"
        );
    }

    #[tokio::test]
    async fn interrupt_is_idempotent_second_call_is_a_no_op_not_an_error() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00002");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;

        let first = interrupt(&async_root, &results_dir, "run00002", "user", None)
            .await
            .expect("first interrupt succeeds");
        assert_eq!(first, InterruptOutcome::Delivered);

        let contents_after_first = tokio::fs::read(&paths.control_inbox)
            .await
            .expect("control inbox readable after first interrupt");

        // A second, near-simultaneous interrupt call against the SAME still-pending request must
        // be a silent no-op, not an error, and must not clobber the original request's contents.
        let second = interrupt(&async_root, &results_dir, "run00002", "user", None)
            .await
            .expect("second interrupt does not error");
        assert_eq!(
            second,
            InterruptOutcome::AlreadyPending,
            "a second interrupt while one is already pending must be a no-op, not a fresh delivery"
        );

        let contents_after_second = tokio::fs::read(&paths.control_inbox)
            .await
            .expect("control inbox still readable");
        assert_eq!(
            contents_after_first, contents_after_second,
            "the second call must not have rewritten the pending request"
        );
    }

    #[tokio::test]
    async fn interrupt_on_already_paused_run_is_a_no_op_not_an_error() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00003");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        status.advance_state(RunState::Paused).expect("Running -> Paused");
        write_atomic_json(&paths.status, &status).await.expect("write paused status");

        let outcome = interrupt(&async_root, &results_dir, "run00003", "user", None)
            .await
            .expect("interrupt on a paused run does not error");

        assert_eq!(
            outcome,
            InterruptOutcome::NotRunning,
            "interrupting an already-Paused run must be a silent no-op (R-SA-083 idempotency)"
        );
        assert!(
            !tokio::fs::try_exists(&paths.control_inbox).await.expect("check exists"),
            "no control-inbox file should be written for a not-Running run"
        );
    }

    #[tokio::test]
    async fn interrupt_on_terminal_run_is_a_no_op_not_an_error() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00004");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&paths.status, &status).await.expect("write complete status");

        let outcome = interrupt(&async_root, &results_dir, "run00004", "user", None)
            .await
            .expect("interrupt on a terminal run does not error");
        assert_eq!(outcome, InterruptOutcome::NotRunning);
    }

    // ---------------------------------------------------------------------------------------
    // interrupt: delete-then-act consumption idempotency
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn consume_interrupt_request_is_idempotent_on_repeated_calls() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00005");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(paths.control_inbox.parent().expect("has parent"))
            .await
            .expect("mkdir control dir");
        write_atomic_json(&paths.control_inbox, &InterruptRequest::new("user", None))
            .await
            .expect("write interrupt request");

        let first = consume_interrupt_request(&paths).await.expect("first consume");
        assert!(first.is_some(), "first consumption must observe the request");
        assert!(
            !tokio::fs::try_exists(&paths.control_inbox).await.expect("check exists"),
            "file must be deleted after consumption (delete-then-act)"
        );

        let second = consume_interrupt_request(&paths).await.expect("second consume does not error");
        assert!(
            second.is_none(),
            "a duplicate consumption after the file is gone must be silently absorbed, not \
             re-processed or errored"
        );
    }

    #[tokio::test]
    async fn check_control_inbox_now_does_not_consume() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00006");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(paths.control_inbox.parent().expect("has parent"))
            .await
            .expect("mkdir control dir");
        write_atomic_json(&paths.control_inbox, &InterruptRequest::new("user", None))
            .await
            .expect("write interrupt request");

        let checked = check_control_inbox_now(&paths).await.expect("check succeeds");
        assert!(checked.is_some());
        assert!(
            tokio::fs::try_exists(&paths.control_inbox).await.expect("check exists"),
            "a non-consuming check must leave the file in place"
        );
    }

    // ---------------------------------------------------------------------------------------
    // resume: running-selection vs. terminal-revival
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn resume_on_running_run_steers_the_single_live_child_without_respawning() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00007");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut running_step = super::super::StepStatus::pending("researcher");
        running_step.status = StepState::Running;
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![running_step]).await;

        let outcome = resume(&async_root, &results_dir, "run00007", None)
            .await
            .expect("resume on a running run succeeds");

        assert_eq!(outcome, ResumeOutcome::SteerRunning { step_index: 0 });
    }

    #[tokio::test]
    async fn resume_on_running_run_with_pending_child_does_not_select_pending() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00008");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut running_step = super::super::StepStatus::pending("researcher");
        running_step.status = StepState::Running;
        let pending_step = super::super::StepStatus::pending("writer"); // status = Pending
        write_running_status(
            &paths,
            &run_id,
            RunMode::Chain,
            None,
            vec![running_step, pending_step],
        )
        .await;

        let outcome = resume(&async_root, &results_dir, "run00008", None)
            .await
            .expect("resume auto-selects the sole running step");

        assert_eq!(
            outcome,
            ResumeOutcome::SteerRunning { step_index: 0 },
            "the Pending step at index 1 must never be auto-selected as resumable"
        );
    }

    #[tokio::test]
    async fn resume_requesting_a_pending_step_index_explicitly_fails() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00009");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let pending_step = super::super::StepStatus::pending("writer");
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![pending_step]).await;

        let result = resume(&async_root, &results_dir, "run00009", Some(0)).await;
        assert!(
            result.is_err(),
            "explicitly requesting a Pending (not-yet-started) step must fail, never silently \
             resume it (R-SA-086: pending children MUST NOT be resumable)"
        );
    }

    /// The key differentiator this task calls out by name: resume distinguishing "select an
    /// already-running child" from "revive a terminal child from its persisted transcript", using
    /// a REAL persisted-transcript fixture (an actual file on disk, not a mocked path).
    #[tokio::test]
    async fn resume_on_terminal_run_revives_from_real_persisted_transcript_fixture() {
        let (dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00010");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        // A REAL transcript fixture file on disk (not a mocked/asserted-only path).
        let transcript_path = dir.path().join("session-abc123.jsonl");
        tokio::fs::write(&transcript_path, b"{\"type\":\"entry\"}\n")
            .await
            .expect("write real transcript fixture");

        let mut terminal_step = super::super::StepStatus::pending("researcher");
        terminal_step.status = StepState::Complete;
        terminal_step.session_file = Some(transcript_path.clone());
        let mut status =
            write_running_status(&paths, &run_id, RunMode::Single, None, vec![terminal_step]).await;
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&paths.status, &status).await.expect("write complete status");

        let outcome = resume(&async_root, &results_dir, "run00010", None)
            .await
            .expect("resume on a terminal run with a real transcript succeeds");

        match outcome {
            ResumeOutcome::RespawnFromTranscript { step_index, session_file } => {
                assert_eq!(step_index, 0);
                assert_eq!(session_file, transcript_path);
                assert!(
                    tokio::fs::try_exists(&session_file).await.expect("check exists"),
                    "the resolved transcript path must point at a REAL file on disk"
                );
            }
            other => panic!("expected RespawnFromTranscript, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resume_on_terminal_run_with_no_transcript_fails_outright_no_silent_fallback() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00011");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let mut terminal_step = super::super::StepStatus::pending("researcher");
        terminal_step.status = StepState::Failed;
        terminal_step.session_file = None; // NO transcript persisted
        let mut status =
            write_running_status(&paths, &run_id, RunMode::Single, None, vec![terminal_step]).await;
        status.advance_state(RunState::Failed).expect("Running -> Failed");
        write_atomic_json(&paths.status, &status).await.expect("write failed status");

        let result = resume(&async_root, &results_dir, "run00011", None).await;

        assert!(
            matches!(result, Err(SubagentError::ResumeNoTranscript)),
            "resume on a terminal step with no persisted transcript must fail outright with \
             ResumeNoTranscript, never silently fall back to a fresh/empty session: got {result:?}"
        );
    }

    #[tokio::test]
    async fn resume_on_paused_run_also_revives_from_transcript_not_steer() {
        let (dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00012");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let transcript_path = dir.path().join("session-paused.jsonl");
        tokio::fs::write(&transcript_path, b"{}\n").await.expect("write fixture");

        let mut paused_step = super::super::StepStatus::pending("researcher");
        paused_step.status = StepState::Paused;
        paused_step.session_file = Some(transcript_path.clone());
        let mut status =
            write_running_status(&paths, &run_id, RunMode::Single, None, vec![paused_step]).await;
        status.advance_state(RunState::Paused).expect("Running -> Paused");
        write_atomic_json(&paths.status, &status).await.expect("write paused status");

        let outcome = resume(&async_root, &results_dir, "run00012", None)
            .await
            .expect("resume on a Paused run succeeds");

        assert!(
            matches!(outcome, ResumeOutcome::RespawnFromTranscript { .. }),
            "Paused is a terminal-for-resume-purposes revival target, not a steer target: {outcome:?}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // append_step: the concurrent-append-race scenario
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn append_step_enqueues_a_request_file_and_updates_pending_appends() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00013");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![]).await;

        let outcome = append_step(
            &async_root,
            &results_dir,
            "run00013",
            vec![RunnerStep::SingleStep(single_step("writer", Some("draft")))],
        )
        .await
        .expect("append succeeds");

        let AppendOutcome::Enqueued { file_name } = outcome;
        assert!(
            tokio::fs::try_exists(paths.append_dir.join(&file_name)).await.expect("check exists"),
            "the enqueued request file must exist on disk"
        );

        let status: RunStatus = serde_json::from_slice(
            &tokio::fs::read(&paths.status).await.expect("read status"),
        )
        .expect("parse status");
        assert_eq!(
            status.pending_appends,
            Some(1),
            "pending_appends must be recomputed from disk, not left unset"
        );
    }

    #[tokio::test]
    async fn append_step_rejects_non_chain_mode() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00014");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;

        let result = append_step(
            &async_root,
            &results_dir,
            "run00014",
            vec![RunnerStep::SingleStep(single_step("writer", None))],
        )
        .await;
        assert!(result.is_err(), "append-step against a non-Chain run must be rejected");
    }

    #[tokio::test]
    async fn append_step_rejects_output_name_colliding_with_a_pending_request() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00015");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![]).await;

        append_step(
            &async_root,
            &results_dir,
            "run00015",
            vec![RunnerStep::SingleStep(single_step("writer", Some("plan")))],
        )
        .await
        .expect("first append succeeds");

        let second = append_step(
            &async_root,
            &results_dir,
            "run00015",
            vec![RunnerStep::SingleStep(single_step("reviewer", Some("plan")))],
        )
        .await;

        assert!(
            second.is_err(),
            "a second append declaring the SAME output name as an already-pending request must \
             be rejected"
        );
    }

    #[tokio::test]
    async fn append_step_rejects_duplicate_output_name_within_one_request() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00016");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![]).await;

        let result = append_step(
            &async_root,
            &results_dir,
            "run00016",
            vec![
                RunnerStep::SingleStep(single_step("writer", Some("dup"))),
                RunnerStep::SingleStep(single_step("reviewer", Some("dup"))),
            ],
        )
        .await;
        assert!(result.is_err());
    }

    /// The concurrent-append-race scenario this task specifically calls out: two near-
    /// simultaneous append requests both trying to bind the SAME output name against the SAME
    /// running chain — exactly one must succeed, the other must be rejected, and there must be
    /// exactly one request file on disk afterward (never zero, never two).
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_appends_binding_the_same_output_name_exactly_one_succeeds() {
        let (_dir, async_root, results_dir) = temp_roots();
        let async_root = Arc::new(async_root);
        let results_dir = Arc::new(results_dir);
        let run_id = RunId::from_token("run00017");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![]).await;

        let mut handles = Vec::new();
        for agent in ["writer-a", "writer-b", "writer-c", "writer-d"] {
            let async_root = Arc::clone(&async_root);
            let results_dir = Arc::clone(&results_dir);
            handles.push(tokio::spawn(async move {
                append_step(
                    &async_root,
                    &results_dir,
                    "run00017",
                    vec![RunnerStep::SingleStep(single_step(agent, Some("shared-name")))],
                )
                .await
            }));
        }

        let mut successes = 0usize;
        let mut failures = 0usize;
        for handle in handles {
            match handle.await.expect("task does not panic") {
                Ok(_) => successes += 1,
                Err(_) => failures += 1,
            }
        }

        assert_eq!(
            successes, 1,
            "exactly one of the racing concurrent appends binding the same output name must \
             succeed, got {successes} successes and {failures} failures"
        );
        assert_eq!(failures, 3);

        // Confirm exactly one request file landed on disk — never zero, never more than one.
        let mut entries = tokio::fs::read_dir(&paths.append_dir).await.expect("list append_dir");
        let mut count = 0usize;
        while entries.next_entry().await.expect("readdir entry").is_some() {
            count += 1;
        }
        assert_eq!(count, 1, "exactly one append-request file must exist on disk after the race");
    }

    #[tokio::test]
    async fn append_step_allows_distinct_output_names_from_concurrent_requests() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00018");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![]).await;

        let first = append_step(
            &async_root,
            &results_dir,
            "run00018",
            vec![RunnerStep::SingleStep(single_step("writer", Some("draft-a")))],
        )
        .await;
        let second = append_step(
            &async_root,
            &results_dir,
            "run00018",
            vec![RunnerStep::SingleStep(single_step("reviewer", Some("draft-b")))],
        )
        .await;

        assert!(first.is_ok(), "distinct output names must both be accepted");
        assert!(second.is_ok(), "distinct output names must both be accepted");
    }

    #[tokio::test]
    async fn list_pending_appends_returns_creation_order() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00019");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Chain, None, vec![]).await;

        append_step(
            &async_root,
            &results_dir,
            "run00019",
            vec![RunnerStep::SingleStep(single_step("first", Some("a")))],
        )
        .await
        .expect("first append");
        tokio::time::sleep(Duration::from_millis(5)).await;
        append_step(
            &async_root,
            &results_dir,
            "run00019",
            vec![RunnerStep::SingleStep(single_step("second", Some("b")))],
        )
        .await
        .expect("second append");

        let listed = list_pending_appends(&paths.append_dir).await.expect("list succeeds");
        assert_eq!(listed.len(), 2);
        let names: Vec<String> = listed
            .iter()
            .filter_map(|(_, request)| request.as_ref())
            .flat_map(|request| {
                request
                    .steps
                    .iter()
                    .flat_map(runner_step_output_names)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()], "must be in creation order");
    }

    // ---------------------------------------------------------------------------------------
    // reconcile_before_control_op
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn reconcile_repairs_status_from_authoritative_result_file() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00020");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        // status.json still claims Running...
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        // ...but an authoritative terminal ResultFile already exists.
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
        let result = ResultFile {
            id: run_id.clone(),
            run_id: run_id.clone(),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            results: Vec::new(),
        };
        write_atomic_json(&paths.result, &result).await.expect("write result file");

        let reconciled = reconcile_before_control_op(&paths).await.expect("reconcile succeeds");

        assert_eq!(
            reconciled.state,
            RunState::Complete,
            "reconciliation must repair status.json from the authoritative ResultFile"
        );

        // The repair must also have been persisted back to disk.
        let on_disk: RunStatus = serde_json::from_slice(
            &tokio::fs::read(&paths.status).await.expect("read repaired status"),
        )
        .expect("parse repaired status");
        assert_eq!(on_disk.state, RunState::Complete);
    }

    #[tokio::test]
    async fn reconcile_returns_status_as_is_when_no_result_file_exists() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00021");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;

        let reconciled = reconcile_before_control_op(&paths).await.expect("reconcile succeeds");
        assert_eq!(reconciled.state, RunState::Running);
    }

    #[tokio::test]
    async fn reconcile_errors_when_neither_status_nor_result_exists() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00022");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let result = reconcile_before_control_op(&paths).await;
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------------------------------
    // root-attachment polling
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn poll_root_attachment_ready_when_result_file_present() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00023");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
        let result = ResultFile {
            id: run_id.clone(),
            run_id: run_id.clone(),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            results: Vec::new(),
        };
        write_atomic_json(&paths.result, &result).await.expect("write result");

        let (outcome, _) = poll_root_attachment(&paths, None).await.expect("poll succeeds");
        assert!(matches!(outcome, AttachmentPoll::Ready(_)));
    }

    #[tokio::test]
    async fn poll_root_attachment_still_waiting_when_running() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00024");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;

        let (outcome, _) = poll_root_attachment(&paths, None).await.expect("poll succeeds");
        assert_eq!(outcome, AttachmentPoll::StillWaiting);
    }

    #[tokio::test]
    async fn poll_root_attachment_still_waiting_immediately_after_going_terminal() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00025");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&paths.status, &status).await.expect("write terminal status");

        // No ResultFile written yet — must be StillWaiting (within grace), not Failed, on the
        // very first observation.
        let (outcome, first_observed) =
            poll_root_attachment(&paths, None).await.expect("poll succeeds");
        assert_eq!(outcome, AttachmentPoll::StillWaiting);
        assert!(first_observed.is_some());
    }

    #[tokio::test]
    async fn poll_root_attachment_fails_after_grace_period_elapses_with_no_result() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00026");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&paths.status, &status).await.expect("write terminal status");

        // Simulate the grace period having already elapsed by backdating
        // `terminal_first_observed_at` well beyond ROOT_ATTACHMENT_GRACE.
        let backdated = now_epoch_millis() - 5_000;
        let (outcome, _) = poll_root_attachment(&paths, Some(backdated))
            .await
            .expect("poll succeeds");
        assert_eq!(
            outcome,
            AttachmentPoll::Failed,
            "terminal-but-no-result must be declared Failed only after the grace period elapses"
        );
    }

    // ---------------------------------------------------------------------------------------
    // wait_for_imported_async_root: R-SA-097 root-attachment loop (mirrors pi
    // chain-root-attachment.test.ts)
    // ---------------------------------------------------------------------------------------

    fn imported_child(agent: &str, output: Option<&str>, exit_code: i32, error: Option<&str>) -> SingleResult {
        SingleResult {
            agent: agent.to_string(),
            task: String::new(),
            exit_code,
            usage: cyrup_core::Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: output.map(str::to_string),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            stopped: false,
            process_signal: None,
            error: error.map(str::to_string),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        }
    }

    fn imported_result_file(
        run_id: &RunId,
        state: RunState,
        success: bool,
        session_file: Option<PathBuf>,
        children: Vec<SingleResult>,
    ) -> ResultFile {
        ResultFile {
            id: run_id.clone(),
            run_id: run_id.clone(),
            agent: "root-agent".to_string(),
            mode: RunMode::Single,
            state,
            success,
            cwd: PathBuf::from("/tmp"),
            session_file,
            results: children,
        }
    }

    #[tokio::test]
    async fn wait_for_imported_async_root_imports_a_completed_child_result() {
        let (_dir, async_root, results_dir) = temp_roots();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
        let run_id = RunId::from_token("root-run-a");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let session_file = _dir.path().join("child.jsonl");
        tokio::fs::write(&session_file, b"").await.expect("write session file");

        let mut status = write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&paths.status, &status).await.expect("write terminal status");
        let result = imported_result_file(
            &run_id,
            RunState::Complete,
            true,
            Some(session_file.clone()),
            vec![imported_child("worker", Some("root output"), 0, None)],
        );
        write_atomic_json(&paths.result, &result).await.expect("write result");

        let imported = wait_for_imported_async_root(
            &paths,
            "root-run-a",
            0,
            "fallback",
            Duration::from_millis(1),
        )
        .await
        .expect("import succeeds");

        assert_eq!(imported.agent, "worker");
        assert_eq!(imported.output, "root output");
        assert_eq!(imported.exit_code, 0);
        assert!(imported.success);
        assert_eq!(imported.session_file.as_deref(), Some(session_file.as_path()));
    }

    #[tokio::test]
    async fn wait_for_imported_async_root_waits_for_a_running_child_to_finish() {
        let (dir, async_root, results_dir) = temp_roots();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
        let run_id = RunId::from_token("root-run-late");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;

        // Write the terminal result only after the loop has already begun polling a still-running
        // target — proving the loop keeps polling and picks up the late result file.
        let result_path = paths.result.clone();
        let late_run_id = run_id.clone();
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let result = imported_result_file(
                &late_run_id,
                RunState::Complete,
                true,
                None,
                vec![imported_child("worker", Some("late root output"), 0, None)],
            );
            write_atomic_json(&result_path, &result).await.expect("write late result");
        });

        let imported = wait_for_imported_async_root(
            &paths,
            "root-run-late",
            0,
            "fallback",
            Duration::from_millis(5),
        )
        .await
        .expect("import succeeds");
        writer.await.expect("writer task joins");
        drop(dir);

        assert_eq!(imported.output, "late root output");
        assert_eq!(imported.exit_code, 0);
        assert!(imported.success);
    }

    #[tokio::test]
    async fn wait_for_imported_async_root_imports_a_failed_child_as_a_failure() {
        let (_dir, async_root, results_dir) = temp_roots();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
        let run_id = RunId::from_token("root-run-fail");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = write_running_status(&paths, &run_id, RunMode::Single, None, vec![]).await;
        status.advance_state(RunState::Failed).expect("Running -> Failed");
        write_atomic_json(&paths.status, &status).await.expect("write terminal status");
        let result = imported_result_file(
            &run_id,
            RunState::Failed,
            false,
            None,
            vec![imported_child("worker", Some("root failed"), 1, Some("root failed"))],
        );
        write_atomic_json(&paths.result, &result).await.expect("write result");

        let imported = wait_for_imported_async_root(
            &paths,
            "root-run-fail",
            0,
            "fallback",
            Duration::from_millis(1),
        )
        .await
        .expect("import succeeds");

        assert_eq!(imported.exit_code, 1);
        assert!(!imported.success);
        assert_eq!(imported.error.as_deref(), Some("root failed"));
        assert_eq!(imported.output, "root failed");
    }

    #[tokio::test]
    async fn wait_for_imported_async_root_fails_a_terminal_root_with_no_result_file() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("root-run-noresult");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = write_running_status(
            &paths,
            &run_id,
            RunMode::Single,
            None,
            vec![super::super::StepStatus::pending("worker")],
        )
        .await;
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        write_atomic_json(&paths.status, &status).await.expect("write terminal status");

        // No ResultFile is ever written. The loop polls: on the first tick it observes terminal
        // status (StillWaiting, within grace); after the grace window elapses it reports Failed and
        // synthesizes the "ended without a result file" diagnostic. A ~1s wall wait is inherent to
        // ROOT_ATTACHMENT_GRACE (the single-tick grace math itself is unit-tested separately by
        // `poll_root_attachment_fails_after_grace_period_elapses_with_no_result`).
        let imported = wait_for_imported_async_root(
            &paths,
            "root-run-noresult",
            0,
            "fallback",
            Duration::from_millis(20),
        )
        .await
        .expect("import succeeds");

        assert_eq!(imported.exit_code, 1);
        assert!(!imported.success);
        assert!(
            imported.error.as_deref().unwrap_or_default().contains("ended without a result file"),
            "expected an `ended without a result file` diagnostic, got: {:?}",
            imported.error
        );
    }

    // ---------------------------------------------------------------------------------------
    // control-inbox watcher (real notify::PollWatcher against a real directory)
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watch_control_inbox_observes_a_real_file_write() {
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00027");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(paths.control_inbox.parent().expect("has parent"))
            .await
            .expect("mkdir control dir");

        let (_watcher, mut rx) = watch_control_inbox(&paths).expect("watcher installs");

        write_atomic_json(&paths.control_inbox, &InterruptRequest::new("user", None))
            .await
            .expect("write request");

        let notified = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(
            notified.is_ok() && notified.expect("timeout checked above").is_some(),
            "a real filesystem write to the control inbox must be observed by the watcher"
        );
    }

    #[tokio::test]
    async fn check_control_inbox_now_catches_a_pre_existing_request_before_any_watch_attaches() {
        // R-SA-082's startup-race-guard property: a request written BEFORE the watcher is even
        // installed must still be observable via the synchronous startup check.
        let (_dir, async_root, results_dir) = temp_roots();
        let run_id = RunId::from_token("run00028");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(paths.control_inbox.parent().expect("has parent"))
            .await
            .expect("mkdir control dir");
        write_atomic_json(&paths.control_inbox, &InterruptRequest::new("race-writer", None))
            .await
            .expect("write request BEFORE any watcher exists");

        let found = check_control_inbox_now(&paths).await.expect("check succeeds");
        assert!(
            found.is_some(),
            "a request written before the watcher attaches must still be caught by the \
             synchronous startup check"
        );
    }
}
