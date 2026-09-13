//! Auto-drain: finish the session's outstanding background work at `agent_end`, so a HEADLESS run
//! does not lose a completion when the turn ends.
//!
//! Ports pi `runs/background/auto-drain.ts` (68 LOC).
//!
//! # Why this exists
//!
//! A background subagent run is a detached OS process whose completion arrives as a terminal
//! [`crate::background::ResultFile`] some time after the spawning tool call returned. An
//! interactive session ends its turn and is woken later by [`crate::background::watch`]. A
//! headless run (`cyrup -p …`) has no later turn: once `agent_end` fires the process is on its way
//! out, and a completion that lands after it is simply lost. Upstream's answer is a bounded drain
//! at the boundary. The `wait` tool ([`crate::background::wait`]) is the manual form of this;
//! auto-drain is the form that does not depend on the model remembering to call it.
//!
//! # Why it is safe to block here
//!
//! Two bounds, both upstream's: [`DEFAULT_AUTO_DRAIN_TIMEOUT_MS`], and the fact that
//! [`crate::background::run_status::list_active_runs`] RECONCILES every run it lists (R-SA-079),
//! so a child whose runner died is classified terminal on the next probe rather than waited on
//! forever. Note the drain is deliberately NOT cancellable by the turn's own token — the turn is
//! already ending, and upstream passes `undefined` for the `AbortSignal` (`auto-drain.ts:58`).
//! SIGINT on the process remains the operator's escape.
//!
//! # Why it does not spin
//!
//! The wait underneath runs with [`crate::background::wait::WaitDeps::stop_on_attention`] OFF
//! (`auto-drain.ts:61`): the drain loop re-enters the wait while `has_outstanding_work` stays
//! true, so a wait that returned early on a needs-attention run would come straight back — a hot
//! loop at 100% CPU for the full deadline, at the exact moment a headless process is trying to
//! exit. With the flag off, attention is waited through, bounded by the deadline.

use std::path::PathBuf;

use crate::background::run_status::list_active_runs;
use crate::background::wait::{WaitDeps, WaitOutcome, WaitParams, wait_for_subagents};
use crate::identity::SessionId;

/// pi `DEFAULT_AUTO_DRAIN_TIMEOUT_MS` (`auto-drain.ts:7`) — 30 minutes.
pub const DEFAULT_AUTO_DRAIN_TIMEOUT_MS: u64 = 30 * 60 * 1000;

/// pi `AutoDrainDeps.hasWork` (`auto-drain.ts:18`) — "is there anything left in this session?".
///
/// A trait object rather than upstream's optional-closure field: `async_trait` is already a direct
/// dependency, an `async` closure field is not expressible as a plain struct member, and this seam
/// is exactly what a test must replace.
#[async_trait::async_trait]
pub trait DrainProbe: Send + Sync {
    /// `Ok(true)` while the session still owns active background work at `now_ms`.
    ///
    /// # Errors
    ///
    /// A work-discovery fault. Upstream's `hasWork` THROWS here — `listAsyncRuns` rethrows
    /// anything that is not a not-found as ``Failed to list async runs in '<root>': <msg>``
    /// (`async-status.ts`'s trailing `catch`) — and that throw propagates out of
    /// `drainOutstandingWork` unchanged rather than reading as "nothing to drain", which
    /// `test/unit/auto-drain.test.ts` pins as "propagates work-discovery errors". Collapsing a
    /// fault to `false` would report a CLEAN drain for a session whose outstanding work could not
    /// be enumerated — the one outcome this subsystem exists to prevent.
    async fn has_outstanding_work(
        &self,
        session_id: &SessionId,
        now_ms: i64,
    ) -> Result<bool, String>;
}

/// pi `AutoDrainDeps.wait` (`auto-drain.ts:15-19`) — one bounded wait-for-everything.
///
/// Returns the whole [`crate::background::wait::WaitOutcome`], because the drain's caller wants
/// the same three facts pi's `AgentToolResult<Details>` carries: upstream reads
/// `waitResult.isError` (`auto-drain.ts:73`) and `resultText(waitResult)` (`:23-25`, `:74`) off
/// one value. Keeping `Result` at this one boundary would reintroduce the `Ok`/`Err` flip
/// [`crate::background::wait::wait_for_subagents`] no longer makes — and would re-break the
/// timeout, since a window that merely elapsed is NOT an error upstream and must let the loop
/// reach its own deadline check.
#[async_trait::async_trait]
pub trait DrainWaiter: Send + Sync {
    /// Block until everything tracked at entry is finished, or `timeout_ms` elapses.
    async fn wait_all(&self, timeout_ms: u64) -> WaitOutcome;
}

/// pi `hasOutstandingWork` (`auto-drain.ts:26-34`).
///
/// [`list_active_runs`] is pi's `listAsyncRuns(DIRS.async, { states: ["queued","running"],
/// sessionId, resultsDir })` — same filter, same reconciliation, same session semantics
/// (`background/run_status.rs`).
///
/// **Unported second disjunct.** Upstream also ORs in
/// `snapshotBackgroundWork(sessionId, nowMs).items.length > 0` (`:32`), the pluggable
/// background-work PROVIDER registry (`api/background-work.ts:163`). cyrup has no provider
/// registry at all — `grep -rn "background_work\|BackgroundWork" crates/cyrup-ext-subagents/src`
/// is empty — and it postdates the ported baseline. The disjunct is therefore constant `false`
/// here; when the registry lands it ORs in at this impl and nowhere else. That is also why
/// [`DrainProbe::has_outstanding_work`] keeps upstream's `now_ms` parameter even though this
/// filesystem impl has no injectable clock to hand it to.
pub struct FsDrainProbe {
    /// The per-cwd async root holding every run directory.
    pub async_root: PathBuf,
    /// The per-cwd results dir holding terminal result files.
    pub results_dir: PathBuf,
}

#[async_trait::async_trait]
impl DrainProbe for FsDrainProbe {
    async fn has_outstanding_work(
        &self,
        session_id: &SessionId,
        _now_ms: i64,
    ) -> Result<bool, String> {
        // A MISSING root never reaches the `map_err`: [`list_active_runs`] answers `Ok(vec![])`
        // for it itself, exactly as pi's `isNotFoundError(error) ⇒ []` does. What is left is
        // upstream's rethrow case — the root EXISTS and its listing failed — carrying pi's own
        // wording so the headless stderr line reads the same in both implementations.
        list_active_runs(
            &self.async_root,
            &self.results_dir,
            Some(session_id.as_str()),
        )
        .await
        .map(|runs| !runs.is_empty())
        .map_err(|error| {
            format!(
                "Failed to list async runs in '{}': {error}",
                self.async_root.display()
            )
        })
    }
}

/// [`wait_for_subagents`] in auto-drain mode — pi's `deps.wait ?? waitForSubagents`
/// (`auto-drain.ts:45`) with the drain's literal parameters (`:56-64`).
///
/// The caller constructs `deps` with `stop_on_attention: false`, `fail_on_failed_runs: true`,
/// `fail_on_attention: true` (upstream hard-codes those three at its call site) and
/// `enabled: true` — `drainOutstandingWork` passes no `enabled` at all and pi's check is
/// `deps.enabled === false` (`subagent-wait.ts:547`), so the drain runs even when the config/env
/// switch disables the `wait` TOOL.
pub struct SubagentDrainWaiter {
    /// The wait dependencies, pre-flagged for drain mode (see the type docs).
    pub deps: WaitDeps,
}

#[async_trait::async_trait]
impl DrainWaiter for SubagentDrainWaiter {
    async fn wait_all(&self, timeout_ms: u64) -> WaitOutcome {
        // pi `wait({ all: true, timeoutMs: remainingMs }, undefined, …)` (`auto-drain.ts:56-58`):
        // the `undefined` AbortSignal is a fresh, never-cancelled token — the turn is already
        // ending, so there is nothing left to abort the drain with.
        wait_for_subagents(
            &WaitParams {
                id: None,
                all: Some(true),
                timeout_ms: Some(timeout_ms),
            },
            &cyrup_core::CancelToken::new(),
            &self.deps,
        )
        .await
    }
}

/// Drain all work owned by `session_id`, including work added while draining — pi
/// `drainOutstandingWork` (`auto-drain.ts:37-68`).
///
/// pi's "no session identity" throw (`:39`) is unrepresentable here rather than checked: the
/// parameter is `&SessionId`, and the caller's `let Some(session_id) = … else` is where that
/// refusal lands — a drain with nothing to scope by would drain another session's runs, which is
/// worse than not draining.
///
/// # Errors
///
/// Upstream's messages verbatim (`:43`, `:53`, `:66`):
///
/// * a non-positive timeout;
/// * a work-discovery fault from `probe` — pi's `hasWork` throw, propagated UNCHANGED (it is not
///   wrapped in the `Auto-drain failed for session …` phrasing, because upstream never catches it
///   to re-word it; see [`DrainProbe::has_outstanding_work`]);
/// * the deadline elapsed with work still active — this is ALSO what a window-elapsed wait falls
///   through to on the next lap, since [`crate::background::wait::WaitVerdict::WindowElapsed`] is
///   not an error (pi `:73-75`);
/// * the underlying wait reported an error — see [`WaitDeps::fail_on_failed_runs`] /
///   [`WaitDeps::fail_on_attention`], which are what make a failed child or unresolved attention
///   an error here rather than a quiet exit. A window-elapsed wait is deliberately **not** one of
///   these; it is the bullet above. (pi's `|| "bg_wait returned an error without details"`
///   fallback at `:66` is unreachable: cyrup's wait never returns an empty message.)
pub async fn drain_outstanding_work(
    session_id: &SessionId,
    timeout_ms: u64,
    now: &(dyn Fn() -> i64 + Send + Sync),
    probe: &dyn DrainProbe,
    waiter: &dyn DrainWaiter,
) -> Result<(), String> {
    // pi `:42-43` — `timeoutMs` must be a positive finite number; `u64` leaves only the zero case.
    if timeout_ms == 0 {
        return Err("Auto-drain timeoutMs must be a positive finite number.".to_string());
    }
    let deadline_at = now().saturating_add(i64::try_from(timeout_ms).unwrap_or(i64::MAX));

    // pi `:47` — the probe is re-consulted every lap, so work ADDED while draining extends the
    // drain (up to the fixed deadline), exactly as the doc comment above promises.
    while probe.has_outstanding_work(session_id, now()).await? {
        let remaining_ms = deadline_at.saturating_sub(now());
        if remaining_ms <= 0 {
            return Err(format!(
                "Auto-drain timed out after {timeout_ms}ms with background work still active in \
                 session '{session_id}'."
            ));
        }
        let outcome = waiter
            .wait_all(u64::try_from(remaining_ms).unwrap_or(0))
            .await;
        // pi `:73-75`. A WINDOW-ELAPSED wait is deliberately NOT an error here: the loop falls
        // through, re-probes, and the `remaining_ms <= 0` check above (`:194-200`) raises
        // upstream's own `Auto-drain timed out after {N}ms …` (`auto-drain.ts:58-59`) on the next
        // lap — which is the message a drain that ran out of time should carry, instead of the
        // wrapped `Auto-drain failed for session '…': Wait timed out after …` this branch used to
        // produce.
        if outcome.is_error() {
            return Err(format!(
                "Auto-drain failed for session '{session_id}': {}.",
                outcome.text
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::background::wait::WaitVerdict;

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    /// A probe answering a scripted sequence of "has work?" values (then `false` forever).
    struct ScriptedProbe {
        answers: Mutex<Vec<bool>>,
        calls: AtomicUsize,
    }

    impl ScriptedProbe {
        fn new(answers: &[bool]) -> Self {
            let mut answers: Vec<bool> = answers.to_vec();
            answers.reverse(); // pop() from the front of the script
            Self {
                answers: Mutex::new(answers),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl DrainProbe for ScriptedProbe {
        async fn has_outstanding_work(
            &self,
            _session_id: &SessionId,
            _now_ms: i64,
        ) -> Result<bool, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.answers.lock().expect("lock").pop().unwrap_or(false))
        }
    }

    /// A probe that cannot enumerate the session's work at all — upstream's throwing `hasWork`.
    struct FailingProbe;

    #[async_trait::async_trait]
    impl DrainProbe for FailingProbe {
        async fn has_outstanding_work(
            &self,
            _session_id: &SessionId,
            _now_ms: i64,
        ) -> Result<bool, String> {
            Err("provider reconcile failed".to_string())
        }
    }

    /// A waiter recording each granted timeout and answering from a script.
    ///
    /// Scripted as whole [`WaitOutcome`]s rather than `Result`s since WORKFLOW_5: the drain reads
    /// `is_error()` off the value, so a double that could only say `Ok`/`Err` could no longer
    /// express the outcome that matters most here — a WINDOW-ELAPSED wait, which is not an error
    /// and must let the loop reach its own deadline check.
    struct ScriptedWaiter {
        answers: Mutex<Vec<WaitOutcome>>,
        timeouts: Mutex<Vec<u64>>,
    }

    impl ScriptedWaiter {
        fn new(answers: Vec<WaitOutcome>) -> Self {
            let mut answers = answers;
            answers.reverse();
            Self {
                answers: Mutex::new(answers),
                timeouts: Mutex::new(Vec::new()),
            }
        }
    }

    /// A scripted resolution that is not an error — the `Ok(text)` of every pre-WORKFLOW_5 lap.
    fn lap(text: &str) -> WaitOutcome {
        WaitOutcome::plain(
            text.to_string(),
            WaitVerdict::Resolved {
                failed_runs: 0,
                attention_runs: 0,
                reported_as_error: false,
            },
        )
    }

    /// A scripted resolution the deps flags reported as an error — auto-drain's
    /// `failOnFailedRuns` case (pi `:751`), which is the only way `wait_all` can hand the drain an
    /// error now.
    fn failed_lap(text: &str) -> WaitOutcome {
        WaitOutcome::plain(
            text.to_string(),
            WaitVerdict::Resolved {
                failed_runs: 1,
                attention_runs: 0,
                reported_as_error: true,
            },
        )
    }

    #[async_trait::async_trait]
    impl DrainWaiter for ScriptedWaiter {
        async fn wait_all(&self, timeout_ms: u64) -> WaitOutcome {
            self.timeouts.lock().expect("lock").push(timeout_ms);
            self.answers
                .lock()
                .expect("lock")
                .pop()
                .unwrap_or_else(|| lap(""))
        }
    }

    #[tokio::test]
    async fn a_zero_timeout_is_refused_with_pi_wording() {
        let probe = ScriptedProbe::new(&[true]);
        let waiter = ScriptedWaiter::new(vec![]);
        let err = drain_outstanding_work(&session("s1"), 0, &|| 0, &probe, &waiter)
            .await
            .expect_err("zero is not a positive timeout");
        assert_eq!(
            err,
            "Auto-drain timeoutMs must be a positive finite number."
        );
        assert_eq!(
            probe.calls.load(Ordering::SeqCst),
            0,
            "refused before any probe"
        );
    }

    #[tokio::test]
    async fn no_outstanding_work_drains_immediately_without_waiting() {
        let probe = ScriptedProbe::new(&[false]);
        let waiter = ScriptedWaiter::new(vec![]);
        drain_outstanding_work(&session("s1"), 1000, &|| 0, &probe, &waiter)
            .await
            .expect("nothing to drain is success");
        assert!(
            waiter.timeouts.lock().expect("lock").is_empty(),
            "the wait was never entered"
        );
    }

    #[tokio::test]
    async fn work_added_while_draining_extends_the_loop_until_the_probe_clears() {
        // pi's stated contract: "including work added while draining" — two laps, then clear.
        let probe = ScriptedProbe::new(&[true, true, false]);
        let waiter = ScriptedWaiter::new(vec![lap("lap one"), lap("lap two")]);
        drain_outstanding_work(&session("s1"), 1000, &|| 0, &probe, &waiter)
            .await
            .expect("drained after two laps");
        assert_eq!(waiter.timeouts.lock().expect("lock").len(), 2);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn the_deadline_elapsing_with_work_still_active_is_pi_verbatim() {
        // A clock that jumps past the deadline after the first lap.
        let ticks = AtomicUsize::new(0);
        let now = move || -> i64 {
            let tick = ticks.fetch_add(1, Ordering::SeqCst);
            if tick == 0 { 0 } else { 10_000 }
        };
        let probe = ScriptedProbe::new(&[true, true]);
        let waiter = ScriptedWaiter::new(vec![lap("lap")]);
        let err = drain_outstanding_work(&session("sess-a"), 500, &now, &probe, &waiter)
            .await
            .expect_err("the deadline must end the drain");
        assert_eq!(
            err,
            "Auto-drain timed out after 500ms with background work still active in session \
             'sess-a'."
        );
    }

    #[tokio::test]
    async fn a_failing_wait_surfaces_as_pi_verbatim() {
        let probe = ScriptedProbe::new(&[true]);
        let waiter = ScriptedWaiter::new(vec![failed_lap("1 run failed")]);
        let err = drain_outstanding_work(&session("sess-a"), 1000, &|| 0, &probe, &waiter)
            .await
            .expect_err("a wait error must end the drain");
        assert_eq!(err, "Auto-drain failed for session 'sess-a': 1 run failed.");
    }

    #[tokio::test]
    async fn each_lap_waits_only_the_remaining_window() {
        // now() advances 100ms per call; deadline is start+1000. The second lap's granted window
        // must be strictly smaller than the first's.
        let ticks = AtomicUsize::new(0);
        let now = move || -> i64 {
            i64::try_from(ticks.fetch_add(1, Ordering::SeqCst)).unwrap_or(i64::MAX) * 100
        };
        let probe = ScriptedProbe::new(&[true, true, false]);
        let waiter = ScriptedWaiter::new(vec![lap(""), lap("")]);
        drain_outstanding_work(&session("s1"), 1000, &now, &probe, &waiter)
            .await
            .expect("drains");
        let timeouts = waiter.timeouts.lock().expect("lock").clone();
        assert_eq!(timeouts.len(), 2);
        assert!(
            timeouts[1] < timeouts[0],
            "the window must shrink between laps: {timeouts:?}"
        );
    }

    /// pi `test/unit/auto-drain.test.ts`, "propagates work-discovery errors": a `hasWork` fault
    /// ends the drain as an ERROR, verbatim and unwrapped. A session whose work cannot be
    /// enumerated has NOT been drained, and answering `false` would report a clean drain while
    /// discarding the only diagnostic that exists for this path.
    #[tokio::test]
    async fn a_work_discovery_fault_propagates_verbatim_instead_of_reading_as_no_work() {
        let waiter = ScriptedWaiter::new(vec![]);
        let err = drain_outstanding_work(&session("s1"), 1000, &|| 0, &FailingProbe, &waiter)
            .await
            .expect_err("a probe fault must not read as a clean drain");
        assert_eq!(
            err, "provider reconcile failed",
            "upstream never catches this throw, so it is not re-worded"
        );
        assert!(
            waiter.timeouts.lock().expect("lock").is_empty(),
            "the wait was never entered"
        );
    }

    /// The production probe over a real (empty) async root: no runs means no work, so the drain is
    /// a no-op — the shape every headless run with no background children pays. This is the
    /// BENIGN half of the errno split ([`list_active_runs`] answers `Ok(vec![])` for a missing
    /// root, pi's `isNotFoundError(error) ⇒ []`); the fault half is the test below.
    #[tokio::test]
    async fn the_fs_probe_reports_no_work_for_an_empty_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let probe = FsDrainProbe {
            async_root: tmp.path().join("async"),
            results_dir: tmp.path().join("results"),
        };
        assert_eq!(
            probe.has_outstanding_work(&session("s1"), 0).await,
            Ok(false),
            "a missing root is absence, not a fault"
        );
    }

    /// The FAULT half: a root that EXISTS but cannot be listed (here a plain file standing where
    /// the directory should be, so `read_dir` answers `ENOTDIR`) is upstream's rethrow case. It
    /// must surface with pi's own wording rather than collapsing to "no work".
    #[tokio::test]
    async fn the_fs_probe_surfaces_a_listing_fault_with_pi_wording() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_root = tmp.path().join("async");
        std::fs::write(&async_root, b"not a directory").expect("file where the root should be");
        let probe = FsDrainProbe {
            async_root: async_root.clone(),
            results_dir: tmp.path().join("results"),
        };

        let err = probe
            .has_outstanding_work(&session("s1"), 0)
            .await
            .expect_err("a root that exists but cannot be listed is a fault, not emptiness");
        assert!(
            err.starts_with(&format!(
                "Failed to list async runs in '{}': ",
                async_root.display()
            )),
            "got {err}"
        );
    }

    /// The production probe against a real on-disk run: scoped to the OWNING session only.
    #[tokio::test]
    async fn the_fs_probe_is_session_scoped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_root = tmp.path().join("async");
        let results_dir = tmp.path().join("results");
        let run_id = crate::background::RunId::new();
        let paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir");
        // This process's own pid, so the reconcile step inside `list_active_runs` sees a live
        // runner and keeps the run Running instead of repairing it to Failed.
        let mut status = crate::background::RunStatus::queued(
            run_id,
            crate::background::RunMode::Single,
            Some(std::process::id()),
        );
        status.state = crate::background::RunState::Running;
        status.session_id = Some(session("owner"));
        status.last_update = crate::time::now_epoch_millis();
        std::fs::write(&paths.status, serde_json::to_vec(&status).expect("ser")).expect("write");

        let probe = FsDrainProbe {
            async_root,
            results_dir,
        };
        assert_eq!(
            probe.has_outstanding_work(&session("owner"), 0).await,
            Ok(true),
            "the owning session sees its run"
        );
        assert_eq!(
            probe.has_outstanding_work(&session("other"), 0).await,
            Ok(false),
            "another session must not drain someone else's run"
        );
    }
}
