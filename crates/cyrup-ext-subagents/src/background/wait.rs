//! The `wait` tool (SUBA-004): block the current turn until outstanding background subagent runs
//! finish — a port of pi-subagents' `src/runs/background/wait.ts` (present at the ported
//! v0.33.x–v0.34.0 baseline; added upstream by `05019cd`, first shipped in v0.33.0).
//!
//! Background subagent runs are detached: hop-1 spawns a real hop-2 runner process and returns
//! immediately. In an interactive session the orchestrator can end its turn and be woken by a
//! completion notification. That does not work when the orchestrator is a skill that must run to
//! completion, and cannot work at all non-interactively (`cyrup -p …`), where the whole task is a
//! single turn: once it ends, nothing is left to receive the notification. `wait` closes that gap
//! by keeping the turn alive until a tracked run reaches a terminal state.
//!
//! # Why blocking here is safe (the two escape hatches)
//!
//! A blocking primitive with no way out would let one wedged child hang the orchestrator forever.
//! There are two independent ways this loop always terminates:
//!
//! 1. **Timeout** — [`WaitParams::timeout_ms`], defaulting to [`DEFAULT_TIMEOUT_MS`] (30 minutes,
//!    pi's `DEFAULT_TIMEOUT_MS`). On expiry the wait returns an error result naming the runs still
//!    in flight. The runs are detached and keep going; only the *waiting* stops.
//! 2. **Cancellation** — the host's [`CancelToken`] for this tool call (pi's `AbortSignal`). The
//!    sleep is a `select!` against [`CancelToken::cancelled`], so aborting the turn wakes the loop
//!    immediately rather than after the remaining poll interval, and it returns an error result
//!    naming what was still active. Dropping the future (the host abandoning the call) likewise
//!    tears everything down — this loop owns no task, no thread and no spawned work.
//!
//! A third, quieter guarantee: a run that trips the `needs_attention` heuristic ALSO ends the wait,
//! in either mode. A child that went idle or blocked on a decision would otherwise stall the loop
//! until the timeout, and the caller is exactly who has to act on it.
//!
//! # Wake mechanism (SUBA-034)
//!
//! pi subscribes to its in-process event bus so a completion wakes `wait` at once, with the poll as
//! a reconciliation fallback — and pi is explicit that the poll, not the event, is the source of
//! truth for what changed ("With no bus, `wait` degrades to pure polling"). cyrup now runs that
//! same two-part shape: [`WaitDeps::completion_bus`] carries the orchestrator's
//! [`super::watch::CompletionBus`], the loop `select!`s the subscription against the sleep, and
//! every wake — from either arm — re-reads authoritative state from disk through the same R-SA-079
//! reconciliation gate every other control action uses ([`super::run_status::list_active_runs`]).
//! The event is never itself the answer.
//!
//! **The delta that survives, and it is a latency floor rather than a mechanism difference.** pi's
//! publisher is the run itself (in-process), so upstream's wake is immediate. cyrup's runs are
//! detached OS processes whose only completion signal is the terminal [`super::ResultFile`]
//! (R-SA-077), and the in-process thing that first learns of it is
//! [`super::watch::ResultsWatcher`] — so a wake here is bounded below by that watcher's own
//! 500 ms [`super::watch::RESULTS_DIR_POLL_INTERVAL`]. What the bus removes is the SECOND,
//! independent [`DEFAULT_POLL_INTERVAL_MS`] (1 s, pi's own value) stacked on top of it. See
//! [`super::watch::CompletionBus`] for the `[CYRUP-DELTA]` and for why closing the remaining
//! 500 ms is a separate R-SA-098 decision.
//!
//! `completion_bus: None` is upstream's own no-bus degradation — pure polling, exactly as before.
//!
//! # Scoping (SUBA-031)
//!
//! pi scopes `subagent_wait` to `state.currentSessionId` (`activeRunsForSession` passes
//! `sessionId: deps.state.currentSessionId ?? undefined` into `listAsyncRuns`,
//! `subagent-wait.ts:265`), and cyrup now does the same through [`WaitDeps::session_id`]. The cwd
//! partition (`async_root`, derived per-cwd by [`super::run_artifact_roots`]) is still the outer
//! one; the session filter is the inner one, exactly as upstream layers them — pi's async root is
//! also per-scope and the session id narrows within it.
//!
//! This is the difference between "two cyrup sessions in the same repo block on each other's
//! background runs" and pi's behaviour, and it is also what makes the empty-set text
//! ("No active async runs **in this session**.") true; before the filter existed the message said
//! the opposite of what the code did.
//!
//! `session_id: None` (headless / unpersisted orchestrator) applies no filter, which is pi's own
//! falsy-`sessionId` path — see [`super::run_status::list_active_runs`] for why an unattributed run
//! is dropped when a filter IS supplied.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;

use super::run_status::{list_active_runs, ActiveRun};
use super::{ActivityState, RunState};

/// States that mean a run is still in flight (pi `ACTIVE_STATES`, `runs/background/wait.ts:55` @v0.34.0). Matches exactly
/// what [`list_active_runs`] already returns.
const fn is_active(state: RunState) -> bool {
    matches!(state, RunState::Queued | RunState::Running)
}

/// pi `DEFAULT_TIMEOUT_MS` (`runs/background/wait.ts:57` @v0.34.0) — 30 minutes.
pub const DEFAULT_TIMEOUT_MS: u64 = 30 * 60 * 1000;
/// pi `MIN_POLL_INTERVAL_MS` (`runs/background/wait.ts:58` @v0.34.0) — the floor a caller-supplied interval is clamped to.
pub const MIN_POLL_INTERVAL_MS: u64 = 250;
/// pi `DEFAULT_POLL_INTERVAL_MS` (`runs/background/wait.ts:59` @v0.34.0).
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 1000;

/// pi `WAIT_TOOL_ENABLED_ENV` (`runs/background/wait.ts:61` @v0.34.0), renamed into cyrup's `CYRUP_SUBAGENT_*` family.
pub const WAIT_TOOL_ENABLED_ENV: &str = "CYRUP_SUBAGENT_WAIT_TOOL_ENABLED";

const WAIT_TOOL_TRUE_VALUES: [&str; 5] = ["1", "true", "yes", "on", "enabled"];
const WAIT_TOOL_FALSE_VALUES: [&str; 5] = ["0", "false", "no", "off", "disabled"];

/// pi `parseWaitToolEnabledEnv` (`runs/background/wait.ts:70-77` @v0.34.0): a set vocabulary, and anything else is a hard
/// configuration error rather than a silently-ignored value.
///
/// # Errors
///
/// Returns pi's message when the value is set but is none of the accepted spellings.
pub fn parse_wait_tool_enabled_env(value: Option<&str>) -> Result<Option<bool>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = value.trim().to_lowercase();
    if WAIT_TOOL_TRUE_VALUES.contains(&normalized.as_str()) {
        return Ok(Some(true));
    }
    if WAIT_TOOL_FALSE_VALUES.contains(&normalized.as_str()) {
        return Ok(Some(false));
    }
    Err(format!(
        "{WAIT_TOOL_ENABLED_ENV} must be one of true/false, 1/0, yes/no, on/off, or \
         enabled/disabled."
    ))
}

/// pi's `config.waitTool`, which accepts either a bare boolean or `{ enabled?: boolean }`
/// (`configWaitToolEnabled`, `runs/background/wait.ts:78-88` @v0.34.0).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WaitToolSetting {
    /// `"waitTool": true` / `false`.
    Enabled(bool),
    /// `"waitTool": { "enabled": … }`.
    Object {
        /// The gate; omitted means "no opinion", so the default (enabled) applies.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
    },
}

impl WaitToolSetting {
    /// The configured opinion, if any.
    #[must_use]
    pub fn enabled(&self) -> Option<bool> {
        match self {
            WaitToolSetting::Enabled(flag) => Some(*flag),
            WaitToolSetting::Object { enabled } => *enabled,
        }
    }
}

/// pi `resolveWaitToolConfig` (`runs/background/wait.ts:90-94` @v0.34.0): env wins over `config.waitTool`, and the tool is
/// enabled when neither says otherwise.
///
/// # Errors
///
/// Propagates [`parse_wait_tool_enabled_env`]'s rejection of an unrecognized env value.
pub fn resolve_wait_tool_enabled(
    config: Option<&WaitToolSetting>,
    env_value: Option<&str>,
) -> Result<bool, String> {
    Ok(parse_wait_tool_enabled_env(env_value)?
        .or_else(|| config.and_then(WaitToolSetting::enabled))
        .unwrap_or(true))
}

/// The `wait` tool's parameter object (pi `WaitParams`, `runs/background/wait.ts:96-108` @v0.34.0).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WaitParams {
    /// Run id (or unambiguous prefix) to wait for. Omitted = every active run in this cwd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Block until EVERY initially-active run is terminal. Default `false` = return as soon as the
    /// first one finishes, so a fleet manager can spawn a replacement and wait again. Ignored when
    /// `id` targets a single run (which always means "wait for that one").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
    /// Give up after this many milliseconds ([`DEFAULT_TIMEOUT_MS`] when unset or non-positive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Injected environment for [`wait_for_subagents`] — the two run-storage roots, the poll cadence,
/// and the config gate.
#[derive(Clone, Debug)]
pub struct WaitDeps {
    /// The per-cwd async root holding every run directory ([`super::run_artifact_roots`]).
    pub async_root: PathBuf,
    /// The per-cwd results dir holding terminal [`super::ResultFile`]s.
    pub results_dir: PathBuf,
    /// Poll cadence, clamped to at least [`MIN_POLL_INTERVAL_MS`]. Tests drive this well below the
    /// 1s production default.
    pub poll_interval: Duration,
    /// `false` makes the tool return immediately without blocking (pi `deps.enabled`).
    pub enabled: bool,
    /// SUBA-031 — the live orchestrator session (pi `deps.state.currentSessionId`), narrowing every
    /// listing this wait performs. `None` applies no filter; see the module docs.
    pub session_id: Option<String>,
    /// SUBA-034 — the in-process completion bus this wait wakes on (pi's event-bus subscription,
    /// `runs/background/wait.ts`'s `onAsyncComplete` listener). `None` is upstream's own documented
    /// no-bus degradation to pure polling, and is what every construction that has no live watcher
    /// (tests, headless embedders) supplies.
    pub completion_bus: Option<crate::background::watch::CompletionBus>,
}

impl WaitDeps {
    /// Production defaults for `cwd`: the shared per-cwd roots and pi's 1s poll interval. The
    /// caller supplies the live session id (pi `deps.state.currentSessionId`); pass `None` only
    /// when there genuinely is no session identity.
    #[must_use]
    pub fn for_cwd(
        cwd: &std::path::Path,
        enabled: bool,
        session_id: Option<String>,
        roots: &crate::paths::Roots,
    ) -> Self {
        let roots = super::run_artifact_roots_in(roots, cwd);
        Self {
            async_root: roots.async_root,
            results_dir: roots.results_dir,
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            enabled,
            session_id,
            completion_bus: None,
        }
    }

    /// SUBA-034 — attach the orchestrator's live completion bus, so this wait wakes on the
    /// observation of a terminal result instead of re-deriving it one poll interval later.
    ///
    /// Separate from [`Self::for_cwd`] because the bus is owned by the extension (it is the same
    /// handle [`crate::background::watch::install_completion_watcher_with_observer`] publishes
    /// into), while `for_cwd` is reachable from contexts that have no watcher at all.
    #[must_use]
    pub fn with_completion_bus(
        mut self,
        bus: Option<crate::background::watch::CompletionBus>,
    ) -> Self {
        self.completion_bus = bus;
        self
    }
}

/// pi `formatDuration` (`shared/formatters.ts:49-53`).
#[must_use]
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        #[allow(clippy::cast_precision_loss)] // sub-minute values; the `.1` render needs a float
        let seconds = ms as f64 / 1000.0;
        return format!("{seconds:.1}s");
    }
    format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
}

/// A run flagged as needing the orchestrator's attention (pi `needsAttention`, `runs/background/wait.ts:203-205` @v0.34.0).
fn needs_attention(run: &ActiveRun) -> bool {
    run.status.telemetry.activity_state == Some(ActivityState::NeedsAttention)
}

fn run_id_of(run: &ActiveRun) -> &str {
    run.status.run_id.as_str()
}

/// pi `matchesId` (`runs/background/wait.ts:198-200` @v0.34.0): exact id, or id prefix.
fn matches_id(run: &ActiveRun, id: &str) -> bool {
    run_id_of(run) == id || run_id_of(run).starts_with(id)
}

/// Queued/running runs for this cwd, optionally narrowed to `id` (pi `activeRunsForSession`).
async fn active_runs(id: Option<&str>, deps: &WaitDeps) -> Result<Vec<ActiveRun>, String> {
    let runs = list_active_runs(
        &deps.async_root,
        &deps.results_dir,
        deps.session_id.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(match id {
        Some(id) => runs.into_iter().filter(|run| matches_id(run, id)).collect(),
        None => runs,
    })
}

/// Every initially-tracked run that has reached a TERMINAL state, reconciled from disk — pi's
/// `terminal` binding (`subagent-wait.ts:613`: `allRunsForSession(…).filter(run =>
/// !ACTIVE_STATES.includes(run.state) && initialAsyncIds.has(run.id))`).
///
/// Built by re-reading each initially-tracked run's own reconciled status rather than re-listing,
/// because [`list_active_runs`] deliberately surfaces only the active ones.
///
/// Returned as the statuses themselves rather than as a rendered summary because upstream derives
/// TWO independent things from this one list — `summarizeTerminalRuns(terminal, …)` at `:616` and
/// `formatResumeFirstFailedRunsNote(terminal)` at `:617` (SUBA-060) — and reading the run tree
/// twice for them would be both slower and racy against a run that changes state in between.
async fn terminal_runs_for(initial_ids: &[String], deps: &WaitDeps) -> Vec<super::RunStatus> {
    let mut terminal = Vec::new();
    for id in initial_ids {
        let paths = super::RunPaths::for_run(
            &deps.async_root,
            &deps.results_dir,
            &super::RunId::from_token(id.clone()),
        );
        // Best effort: a run whose record vanished simply contributes nothing to the summary. The
        // wait itself has already resolved; this is reporting only (pi wraps the whole block in a
        // `try`/`catch` with the same "summary is best-effort" comment).
        let Ok(status) = super::control::reconcile_before_control_op(&paths).await else {
            continue;
        };
        if is_active(status.state) {
            continue;
        }
        terminal.push(status);
    }
    terminal
}

/// pi `summarizeTerminalRuns` (`subagent-wait.ts:616`) — the `Outcome: …` bucket counts.
fn summarize_terminal_runs(terminal: &[super::RunStatus]) -> (usize, String) {
    let mut complete = 0usize;
    let mut failed = 0usize;
    let mut paused = 0usize;
    // G77: stopped runs are counted and reported in their own bucket, never folded into `failed`.
    let mut stopped = 0usize;
    for status in terminal {
        match status.state {
            RunState::Complete => complete += 1,
            RunState::Failed => failed += 1,
            RunState::Paused => paused += 1,
            RunState::Stopped => stopped += 1,
            RunState::Queued | RunState::Running => {}
        }
    }
    let finished = complete + failed + paused + stopped;
    let mut parts: Vec<String> = Vec::new();
    if complete > 0 {
        parts.push(format!("{complete} complete"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    // G77: placed between `failed` and `paused`, matching the fixed bucket order pi renders status
    // counts in (`result-intercom.ts:57-66 formatStatusCounts`: completed, failed, stopped, paused,
    // detached).
    if stopped > 0 {
        parts.push(format!("{stopped} stopped"));
    }
    if paused > 0 {
        parts.push(format!("{paused} paused"));
    }
    (finished, parts.join(", "))
}

fn join_ids(runs: &[ActiveRun]) -> String {
    runs.iter().map(run_id_of).collect::<Vec<_>>().join(", ")
}

fn join_ids_with_state(runs: &[ActiveRun]) -> String {
    runs.iter()
        .map(|run| format!("{} ({})", run_id_of(run), super::run_status::run_state_label(run.status.state)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Block until the targeted background runs finish, the timeout elapses, or the turn is aborted —
/// pi `waitForSubagents` (`runs/background/wait.ts:264-394` @v0.34.0).
///
/// Returns `Ok(text)` for a resolved wait and `Err(text)` for the three non-resolutions (a listing
/// failure, an ambiguous id prefix, a timeout, an abort), matching how every other control action
/// in this crate maps onto the tool result's error flag.
///
/// # Errors
///
/// See above — the `Err` payload is the human-readable summary, never a bare error code.
pub async fn wait_for_subagents(
    params: &WaitParams,
    cancel: &CancelToken,
    deps: &WaitDeps,
) -> Result<String, String> {
    if !deps.enabled {
        return Ok(format!(
            "Wait tool is disabled by config.waitTool or {WAIT_TOOL_ENABLED_ENV}; returning \
             immediately without blocking background subagent runs. Active runs keep going, and \
             you can inspect them with subagent({{ action: \"status\" }}) or wait for completion \
             notifications."
        ));
    }

    let poll_interval = deps.poll_interval.max(Duration::from_millis(MIN_POLL_INTERVAL_MS));
    let timeout = Duration::from_millis(match params.timeout_ms {
        Some(ms) if ms > 0 => ms,
        _ => DEFAULT_TIMEOUT_MS,
    });
    let started_at = Instant::now();

    // A single named run always means "wait until that one is done", regardless of `all`.
    let wait_for_all = params.id.is_some() || params.all == Some(true);

    // SUBA-034: subscribe BEFORE the first listing, never after. A completion observed between the
    // snapshot below and a later subscription would be missed by BOTH — the snapshot still shows
    // the run active and the subscription starts after the edge — and the wait would then pay the
    // full poll interval anyway, which is exactly the latency this closes. `broadcast::Receiver`
    // only delivers values sent after `subscribe()`, so this ordering is the whole correctness
    // argument for the wake.
    let mut wake = deps.completion_bus.as_ref().map(super::watch::CompletionBus::subscribe);

    let mut active = active_runs(params.id.as_deref(), deps).await?;

    if active.is_empty() {
        return Ok(match &params.id {
            Some(id) => format!("No active run matched \"{id}\". Nothing to wait for."),
            None => "No active async runs in this session. Nothing to wait for.".to_string(),
        });
    }

    let mut effective_id = params.id.clone();
    if let Some(id) = params.id.as_deref() {
        let exact: Vec<ActiveRun> =
            active.iter().filter(|run| run_id_of(run) == id).cloned().collect();
        if exact.len() == 1 {
            active = exact;
        } else if active.len() > 1 {
            return Err(format!(
                "Ambiguous async run id prefix \"{id}\" matched {} active runs: {}. Pass a longer \
                 id.",
                active.len(),
                join_ids(&active)
            ));
        }
        // Narrow to the single resolved id so later polls cannot pick up a different prefix match.
        effective_id = active.first().map(|run| run_id_of(run).to_string());
    }

    // The set of runs in flight when the wait began. In first-completion mode we return as soon as
    // any of THESE leaves the active set — a run spawned by a concurrent turn does not satisfy it.
    let initial_ids: Vec<String> = active.iter().map(|run| run_id_of(run).to_string()).collect();
    let initial_count = initial_ids.len();
    let mut pending: Vec<ActiveRun> =
        active.iter().filter(|run| !needs_attention(run)).cloned().collect();
    let mut attention: Vec<ActiveRun> =
        active.iter().filter(|run| needs_attention(run)).cloned().collect();

    let done = |pending: &[ActiveRun], attention: &[ActiveRun]| -> bool {
        // A run needing attention always breaks the wait, in either mode: the caller has to act on
        // it (nudge/resume/interrupt) and blocking longer helps nothing.
        if !attention.is_empty() {
            return true;
        }
        if wait_for_all {
            return pending.iter().all(|run| !initial_ids.iter().any(|id| id == run_id_of(run)));
        }
        let still_active_initial =
            pending.iter().filter(|run| initial_ids.iter().any(|id| id == run_id_of(run))).count();
        still_active_initial < initial_count
    };

    while !done(&pending, &attention) {
        if cancel.is_cancelled() {
            return Err(format!(
                "Wait aborted after {}. Still active: {}.",
                format_duration(elapsed_ms(started_at)),
                join_ids_with_state(&pending)
            ));
        }
        let elapsed = started_at.elapsed();
        if elapsed >= timeout {
            return Err(format!(
                "Wait timed out after {} with {} run(s) still active: {}. The runs are detached \
                 and keep going; call wait again or inspect with subagent({{ action: \"status\" \
                 }}).",
                format_duration(timeout.as_millis().try_into().unwrap_or(u64::MAX)),
                pending.len(),
                join_ids_with_state(&pending)
            ));
        }

        // Escape hatch #1 + #2, in one place: never sleep past the deadline, and wake instantly on
        // cancellation instead of after the remaining poll interval.
        //
        // SUBA-034 adds the third arm: a completion observed by this process's own
        // [`super::watch::ResultsWatcher`] ends the sleep immediately, with the poll kept underneath
        // as reconciliation — pi's own arrangement, and pi is explicit that the poll, not the
        // event, is the source of truth for what changed. The loop re-reads the run tree from disk
        // on every iteration regardless of WHICH arm woke it, so a spurious or unrelated event
        // costs one extra listing and can never resolve a wait that has not actually finished.
        //
        // `biased;` is load-bearing: with a cancelled token AND a ready wake, an unbiased
        // `select!` picks at random, so an aborted turn could take the wake arm, do one more
        // listing, and only notice the cancellation on the next iteration. Upstream cannot express
        // that race at all (JS awaits settle in order), so the bias is what preserves its
        // behaviour — cancellation first, then the event, then the timer.
        let slice = poll_interval.min(timeout - elapsed);
        let mut wake_closed = false;
        match wake.as_mut() {
            Some(receiver) => {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    // A `Lagged` receiver is a wake-up, not an error: it means MORE completions
                    // landed than the bus buffered, which is the strongest possible signal that
                    // something changed. `Closed` — the watcher was torn down mid-wait — is the one
                    // outcome that must not be treated as an edge: `recv` would then return
                    // instantly forever and spin this loop at 100% CPU, so it retires the
                    // subscription instead and the remainder of the wait polls.
                    outcome = receiver.recv() => {
                        wake_closed = matches!(
                            outcome,
                            Err(tokio::sync::broadcast::error::RecvError::Closed)
                        );
                    }
                    () = tokio::time::sleep(slice) => {}
                }
            }
            None => {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    () = tokio::time::sleep(slice) => {}
                }
            }
        }
        if wake_closed {
            wake = None;
            // Pay the interval this iteration would otherwise have skipped, so retiring the
            // subscription costs the same cadence a wait that never had a bus pays.
            tokio::select! {
                biased;
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(slice) => {}
            }
        }

        active = active_runs(effective_id.as_deref(), deps).await?;
        pending = active.iter().filter(|run| !needs_attention(run)).cloned().collect();
        attention = active
            .iter()
            .filter(|run| needs_attention(run) && initial_ids.iter().any(|id| id == run_id_of(run)))
            .cloned()
            .collect();
    }

    let terminal_runs = terminal_runs_for(&initial_ids, deps).await;
    let (finished_count, terminal_summary) = summarize_terminal_runs(&terminal_runs);
    // SUBA-060 / pi `resumeGuidance = formatResumeFirstFailedRunsNote(terminal)`
    // (`subagent-wait.ts:617`), interpolated at `:642`/`:660` immediately after the outcome clause
    // and before the attention note. Empty unless a failed run actually has a revivable child
    // session, so an ordinary wait is unchanged.
    let resume_guidance = super::resume_guidance::format_resume_first_failed_runs_note(&terminal_runs);
    let attention_note = if attention.is_empty() {
        String::new()
    } else {
        format!(
            " {} run(s) need attention: {} — inspect with subagent({{ action: \"status\" }}) then \
             nudge/resume/interrupt.",
            attention.len(),
            join_ids(&attention)
        )
    };
    let elapsed = format_duration(elapsed_ms(started_at));
    let outcome = if terminal_summary.is_empty() {
        String::new()
    } else {
        format!(" Outcome: {terminal_summary}.")
    };

    if wait_for_all {
        let scope = match &params.id {
            Some(id) => format!("run \"{id}\""),
            None => format!("{initial_count} async run(s)"),
        };
        let status = if attention.is_empty() { "done" } else { "attention required" };
        let notification = if attention.is_empty() {
            "Completion events have been observed; inspect status if the notification is not \
             visible yet."
        } else {
            "Relevant completion/control events have been observed; inspect status if the \
             notification is not visible yet."
        };
        return Ok(format!(
            "Waited {elapsed} for {scope}; \
             {status}.{outcome}{resume_guidance}{attention_note} {notification}"
        ));
    }

    // First-completion mode.
    let still_running = pending
        .iter()
        .filter(|run| initial_ids.iter().any(|id| id == run_id_of(run)))
        .count();
    let remainder = if still_running > 0 {
        format!(" {still_running} run(s) still in flight — call wait again to catch the next one.")
    } else if attention.is_empty() {
        " No runs remain in flight.".to_string()
    } else {
        " No other runs are waitable until attention is handled.".to_string()
    };
    let progress = if !attention.is_empty() && finished_count == 0 {
        format!("{} of {initial_count} run(s) need attention", attention.len())
    } else {
        format!("{finished_count} of {initial_count} run(s) finished")
    };
    let notification = if finished_count > 0 {
        " Completion events for the finished run(s) have been observed; inspect status if the \
         notification is not visible yet."
    } else {
        " Relevant control events have been observed; inspect status if the notification is not \
         visible yet."
    };
    Ok(format!(
        "Waited {elapsed}; \
         {progress}.{outcome}{resume_guidance}{attention_note}{remainder}{notification}"
    ))
}

fn elapsed_ms(since: Instant) -> u64 {
    since.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::background::{RunId, RunMode, RunPaths, RunStatus};

    struct Fixture {
        _dir: tempfile::TempDir,
        async_root: PathBuf,
        results_dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let async_root = dir.path().join("async");
            let results_dir = dir.path().join("results");
            std::fs::create_dir_all(&async_root).expect("mkdir async");
            std::fs::create_dir_all(&results_dir).expect("mkdir results");
            Self { _dir: dir, async_root, results_dir }
        }

        fn deps(&self, enabled: bool) -> WaitDeps {
            WaitDeps {
                async_root: self.async_root.clone(),
                results_dir: self.results_dir.clone(),
                poll_interval: Duration::from_millis(MIN_POLL_INTERVAL_MS),
                enabled,
                session_id: None,
                // SUBA-034: the no-bus shape is upstream's own documented degradation, and it is
                // what every pre-existing test in this module exercises — so the polling path stays
                // covered exactly as before and only the two new tests opt into a bus.
                completion_bus: None,
            }
        }

        fn paths(&self, run_id: &RunId) -> RunPaths {
            RunPaths::for_run(&self.async_root, &self.results_dir, run_id)
        }

        /// Write a `status.json` for a run in the given state — the on-disk shape
        /// `list_active_runs` reads.
        fn write_status(&self, run_id: &RunId, state: RunState, attention: bool) {
            self.write_status_for_session(run_id, state, attention, None);
        }

        /// SUBA-031 — the same writer, with the run's OWN recorded orchestrator session
        /// (pi `AsyncStatus.sessionId`, stamped from `config.sessionId` at
        /// `subagent-runner.ts:2088`).
        fn write_status_for_session(
            &self,
            run_id: &RunId,
            state: RunState,
            attention: bool,
            session_id: Option<&str>,
        ) {
            let paths = self.paths(run_id);
            std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
            let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
            status.state = state;
            status.session_id = session_id.map(str::to_string);
            if attention {
                status.telemetry.activity_state = Some(ActivityState::NeedsAttention);
            }
            std::fs::write(
                &paths.status,
                serde_json::to_string(&status).expect("status serializes"),
            )
            .expect("write status.json");
        }

        /// Write the authoritative terminal `ResultFile` — the real completion signal a detached
        /// hop-2 runner emits, and the one thing that flips a run terminal for every reader.
        fn settle(async_root: &std::path::Path, results_dir: &std::path::Path, run_id: &RunId) {
            let paths = RunPaths::for_run(async_root, results_dir, run_id);
            let result = crate::background::ResultFile {
                id: run_id.clone(),
                run_id: run_id.clone(),
                agent: "worker".to_string(),
                mode: RunMode::Single,
                state: RunState::Complete,
                success: true,
                cwd: PathBuf::from("/tmp"),
                session_file: None,
                results: Vec::new(),
            };
            std::fs::write(&paths.result, serde_json::to_string(&result).expect("serializes"))
                .expect("write result file");
        }
    }

    #[tokio::test]
    async fn with_no_active_runs_wait_returns_immediately() {
        let fx = Fixture::new();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true))
            .await
            .expect("no runs is not an error");
        assert_eq!(text, "No active async runs in this session. Nothing to wait for.");
    }

    /// SUBA-031 — `wait` is scoped to the SESSION, not merely to the cwd, so two cyrup sessions in
    /// the same repository no longer block on each other's background runs.
    ///
    /// The first assertion is the control and it is not decoration: it proves the foreign run is on
    /// disk, active, and would otherwise be waited on — without it, the scoped assertion below
    /// would pass just as happily against an empty async root.
    ///
    /// The message is asserted too, because it was the visible half of the defect: the empty-set
    /// text has always said "in this session" while the scope was the cwd, so before this change
    /// the two sentences the tool could produce were both wrong for the same run.
    #[tokio::test]
    async fn wait_ignores_another_sessions_background_run() {
        let fx = Fixture::new();
        let foreign = RunId::new();
        fx.write_status_for_session(&foreign, RunState::Running, false, Some("session-b"));

        // Control: with no session identity (pi's falsy `sessionId`) the run IS in scope, so the
        // wait blocks on it — which is exactly what session A used to do.
        let unscoped = tokio::time::timeout(
            Duration::from_millis(700),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await;
        assert!(
            unscoped.is_err(),
            "an unscoped wait must block on the foreign run, got {unscoped:?}"
        );

        // Scoped to session A: the foreign run is out of scope, so the wait returns at once with
        // the empty-set text — which is now true rather than merely printed.
        let deps = WaitDeps {
            session_id: Some("session-a".to_string()),
            ..fx.deps(true)
        };
        let text = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps),
        )
        .await
        .expect("a session-scoped wait must not block on another session's run")
        .expect("no in-scope runs is not an error");
        assert_eq!(text, "No active async runs in this session. Nothing to wait for.");
    }

    /// THE behavior SUBA-004 exists for: `wait` actually BLOCKS while a background run is in
    /// flight, and returns only once that run's terminal result appears.
    #[tokio::test]
    async fn wait_blocks_until_the_background_run_actually_settles() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        // Prove it is still blocking a full poll interval in: nothing has settled yet.
        let early = tokio::time::timeout(
            Duration::from_millis(700),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await;
        assert!(
            early.is_err(),
            "wait must still be blocking while the run is Running, got {early:?}"
        );

        // Now settle the run the way a real detached runner does, and the SAME call must return.
        let settle = {
            let run_id = run_id.clone();
            let async_root = fx.async_root.clone();
            let results_dir = fx.results_dir.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                Fixture::settle(&async_root, &results_dir, &run_id);
            })
        };

        let text = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("wait must resolve once the run settles")
        .expect("a settled run is not an error");
        settle.await.expect("settler task");

        assert!(
            text.contains("1 of 1 run(s) finished"),
            "the summary must report the finished run: {text}"
        );
        assert!(text.contains("Outcome: 1 complete."), "and how it came out: {text}");
        assert!(
            !text.contains("Resume-first"),
            "SUBA-060 guidance is for FAILED runs only; a complete run must not carry it: {text}"
        );
    }

    /// SUBA-060 — a run that FAILS while the wait is in flight, and whose child session was
    /// persisted, must come back with pi's resume-first sentence naming the literal `resume` call
    /// (`subagent-wait.ts:617`). Without it the orchestrator's default response to "1 failed" is to
    /// spawn a replacement child and re-pay for every turn the failed one already took.
    ///
    /// The sibling assertion in
    /// [`wait_blocks_until_the_background_run_actually_settles`](Self) is the vacuous-pass guard on
    /// the other side: it pins that a COMPLETE run does not get the sentence, so this test cannot
    /// pass merely because the note is emitted unconditionally.
    #[tokio::test]
    async fn a_failed_run_with_a_persisted_child_session_returns_resume_first_guidance() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let transcript = fx._dir.path().join("child-session.jsonl");
        std::fs::write(&transcript, b"{}").expect("write child transcript");

        let settle = {
            let run_id = run_id.clone();
            let paths = fx.paths(&run_id);
            let transcript = transcript.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let mut status = RunStatus::queued(run_id, RunMode::Single, Some(1));
                status.state = RunState::Failed;
                status.ended_at = Some(crate::time::now_epoch_millis());
                let mut step = crate::background::StepStatus::pending("worker");
                step.status = crate::background::StepState::Failed;
                step.session_file = Some(transcript);
                status.steps = vec![step];
                std::fs::write(
                    &paths.status,
                    serde_json::to_string(&status).expect("status serializes"),
                )
                .expect("write failed status");
            })
        };

        let text = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("wait must resolve once the run reaches a terminal state")
        .expect("a failed run is reported, not errored");
        settle.await.expect("settler task");

        assert!(text.contains("Outcome: 1 failed."), "the run must be seen as failed: {text}");
        assert!(
            text.contains(&format!(
                " Resume-first: failed run \"{}\" has a persisted child session. Revive the \
                 original run with subagent({{ action: \"resume\", id: \"{}\", message: \
                 \"Continue from the persisted child session and report the result.\" }}) before \
                 reporting failure or launching a replacement. Launch a replacement only if revive \
                 fails or the user explicitly asks for one.",
                run_id.as_str(),
                run_id.as_str()
            )),
            "pi's verbatim resume-first note must be present: {text}"
        );
    }

    /// Escape hatch #1: a wedged run cannot hold the orchestrator past the caller's timeout, and
    /// the message says the runs keep going so the caller knows nothing was killed.
    #[tokio::test]
    async fn a_hung_run_is_released_by_the_timeout_and_reported_as_still_active() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let started = Instant::now();
        let err = wait_for_subagents(
            &WaitParams { timeout_ms: Some(400), ..WaitParams::default() },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await
        .expect_err("a timeout is reported as an error result");
        assert!(started.elapsed() >= Duration::from_millis(350), "it must actually have waited");
        assert!(started.elapsed() < Duration::from_secs(5), "and not far past the deadline");
        assert!(err.starts_with("Wait timed out after "), "got: {err}");
        assert!(err.contains("1 run(s) still active"), "got: {err}");
        assert!(err.contains(run_id.as_str()), "the wedged run must be named: {err}");
        assert!(err.contains("The runs are detached and keep going"), "got: {err}");
    }

    /// Escape hatch #2: cancelling the turn releases the wait promptly — well inside one production
    /// poll interval, proving the sleep really is a `select!` on the token and not a fixed sleep.
    #[tokio::test]
    async fn cancelling_the_turn_releases_the_wait_without_waiting_out_the_poll_interval() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let cancel = CancelToken::new();
        let canceller = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                cancel.cancel();
            })
        };

        let deps = WaitDeps {
            // A 30s poll interval: if cancellation did not wake the sleep, this test would hang.
            poll_interval: Duration::from_secs(30),
            ..fx.deps(true)
        };
        let started = Instant::now();
        let err = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &cancel, &deps),
        )
        .await
        .expect("cancellation must break the wait, not wait out the 30s poll")
        .expect_err("an aborted wait is reported as an error result");
        canceller.await.expect("canceller task");

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation must wake the sleep immediately, took {:?}",
            started.elapsed()
        );
        assert!(err.starts_with("Wait aborted after "), "got: {err}");
        assert!(err.contains(run_id.as_str()), "the still-active run must be named: {err}");
    }

    /// A child that needs attention breaks the wait immediately in either mode — otherwise a stuck
    /// child stalls the loop until the timeout, which is exactly what the caller must not sit
    /// through.
    #[tokio::test]
    async fn a_run_needing_attention_breaks_the_wait_at_once() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, true);

        let started = Instant::now();
        let text = wait_for_subagents(
            &WaitParams { all: Some(true), ..WaitParams::default() },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await
        .expect("attention resolves the wait");
        assert!(started.elapsed() < Duration::from_secs(2), "it must not have polled at all");
        assert!(text.contains("attention required"), "got: {text}");
        assert!(text.contains("1 run(s) need attention"), "got: {text}");
        assert!(text.contains(run_id.as_str()), "got: {text}");
    }

    /// First-completion (default) vs `all: true`: with two runs in flight, settling ONE releases
    /// the default wait and leaves `all: true` still blocking. Both runs must be active when the
    /// wait starts — a run that was already terminal is not in the initial set and satisfies
    /// nothing (pi's `initialIds` rule).
    #[tokio::test]
    async fn first_completion_returns_early_while_all_true_keeps_waiting() {
        let fx = Fixture::new();
        let a = RunId::new();
        let b = RunId::new();
        fx.write_status(&a, RunState::Running, false);
        fx.write_status(&b, RunState::Running, false);

        // `all: true` is NOT satisfied while both are running.
        let blocked = tokio::time::timeout(
            Duration::from_millis(800),
            wait_for_subagents(
                &WaitParams { all: Some(true), ..WaitParams::default() },
                &CancelToken::new(),
                &fx.deps(true),
            ),
        )
        .await;
        assert!(blocked.is_err(), "all:true must block while both run, got {blocked:?}");

        let settle = {
            let a = a.clone();
            let async_root = fx.async_root.clone();
            let results_dir = fx.results_dir.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                Fixture::settle(&async_root, &results_dir, &a);
            })
        };

        let text = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("first-completion must return as soon as ONE of the two settles")
        .expect("not an error");
        settle.await.expect("settler task");
        assert!(text.contains("1 of 2 run(s) finished"), "got: {text}");
        assert!(text.contains("1 run(s) still in flight"), "got: {text}");

        // ...and `all: true` is STILL not satisfied, because run b never settled.
        let still_blocked = tokio::time::timeout(
            Duration::from_millis(800),
            wait_for_subagents(
                &WaitParams { all: Some(true), ..WaitParams::default() },
                &CancelToken::new(),
                &fx.deps(true),
            ),
        )
        .await;
        assert!(
            still_blocked.is_err(),
            "all:true must keep waiting on the unsettled run, got {still_blocked:?}"
        );
        assert!(!b.as_str().is_empty(), "the second run id is real");
    }

    /// An id that matches several active runs is rejected rather than silently waiting on whichever
    /// one sorted first (pi's ambiguity guard).
    #[tokio::test]
    async fn an_ambiguous_id_prefix_is_rejected() {
        let fx = Fixture::new();
        // `RunId::from_token` keeps the literal token, so two ids can share a prefix on purpose.
        let a = RunId::from_token("abc111".to_string());
        let b = RunId::from_token("abc222".to_string());
        fx.write_status(&a, RunState::Running, false);
        fx.write_status(&b, RunState::Running, false);

        let err = wait_for_subagents(
            &WaitParams { id: Some("abc".to_string()), ..WaitParams::default() },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await
        .expect_err("an ambiguous prefix must be rejected");
        assert!(err.starts_with("Ambiguous async run id prefix \"abc\" matched 2 active runs:"));
        assert!(err.ends_with("Pass a longer id."), "got: {err}");
    }

    #[tokio::test]
    async fn an_unmatched_id_says_so_instead_of_blocking() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);
        let text = wait_for_subagents(
            &WaitParams { id: Some("nope".to_string()), ..WaitParams::default() },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await
        .expect("no match is not an error");
        assert_eq!(text, "No active run matched \"nope\". Nothing to wait for.");
    }

    /// The config/env gate: disabled means "return at once without blocking", never "silently
    /// pretend the runs finished".
    #[tokio::test]
    async fn a_disabled_wait_returns_immediately_without_blocking() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);
        let started = Instant::now();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(false))
            .await
            .expect("disabled is not an error");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(text.starts_with("Wait tool is disabled by config.waitTool or "), "got: {text}");
        assert!(text.contains("Active runs keep going"), "got: {text}");
    }

    #[test]
    fn the_enabled_gate_resolves_env_over_config_and_defaults_to_on() {
        assert_eq!(resolve_wait_tool_enabled(None, None), Ok(true));
        assert_eq!(resolve_wait_tool_enabled(Some(&WaitToolSetting::Enabled(false)), None), Ok(false));
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Object { enabled: Some(false) }), None),
            Ok(false)
        );
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Object { enabled: None }), None),
            Ok(true)
        );
        // Env wins over config, in both directions.
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Enabled(false)), Some("on")),
            Ok(true)
        );
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Enabled(true)), Some("disabled")),
            Ok(false)
        );
        // An unrecognized value is a configuration ERROR, not a silently-ignored one.
        assert_eq!(
            resolve_wait_tool_enabled(None, Some("maybe")),
            Err("CYRUP_SUBAGENT_WAIT_TOOL_ENABLED must be one of true/false, 1/0, yes/no, on/off, \
                 or enabled/disabled."
                .to_string())
        );
    }

    #[test]
    fn wait_tool_setting_accepts_both_pi_shapes() {
        assert_eq!(
            serde_json::from_value::<WaitToolSetting>(serde_json::json!(false)).expect("bool form"),
            WaitToolSetting::Enabled(false)
        );
        assert_eq!(
            serde_json::from_value::<WaitToolSetting>(serde_json::json!({"enabled": true}))
                .expect("object form"),
            WaitToolSetting::Object { enabled: Some(true) }
        );
    }

    #[test]
    fn format_duration_matches_pi() {
        assert_eq!(format_duration(999), "999ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(59_999), "60.0s");
        assert_eq!(format_duration(65_000), "1m5s");
        assert_eq!(format_duration(DEFAULT_TIMEOUT_MS), "30m0s");
    }

    // =============================================================================================
    // SUBA-034 — the event wake
    // =============================================================================================

    /// The item's own Verify, expressed against the mechanism rather than against wall-clock luck:
    /// a wait whose poll interval is FIVE SECONDS must still return promptly once the completion
    /// is observed and published, because the bus arm — not the timer — is what releases it.
    ///
    /// Deliberately not asserting "~50 ms": cyrup's publisher is the orchestrator's result-file
    /// watcher rather than the run itself, so the production floor is that watcher's own 500 ms
    /// cadence (see the module docs' `[CYRUP-DELTA]`). What IS asserted is the property the port
    /// adds — that the second, independent 1 s wait cadence no longer applies — and the 5 s /
    /// 2 s gap makes a regression to pure polling fail this test rather than merely slow it.
    #[tokio::test]
    async fn a_published_completion_releases_a_wait_long_before_its_poll_interval() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-bus-wake");
        fx.write_status(&run, RunState::Running, false);

        let bus = crate::background::watch::CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_secs(5),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };

        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            // The completion is the FILE — published second, exactly as the watcher does it, so a
            // waiter that woke on the event and re-read the tree finds terminal state waiting.
            Fixture::settle(&async_root, &results_dir, &settle_run);
            bus.publish(crate::background::watch::CompletionEvent {
                run_id: settle_run,
                outcome: crate::background::watch::ClassifiedOutcome::Completed,
            });
        });

        let started = Instant::now();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps)
            .await
            .expect("wait resolves on the published completion");
        let elapsed = started.elapsed();
        settler.await.expect("settler task");

        assert!(
            elapsed < Duration::from_secs(2),
            "the bus arm must release the wait; a pure-poll regression would take ~5s, took {elapsed:?}"
        );
        assert!(
            text.contains("1 of 1 run(s) finished"),
            "the wake must still resolve through the on-disk reconciliation, not from the event: {text}"
        );
    }

    /// The event is a WAKE-UP, never the answer. A publish for a run that has NOT settled must
    /// leave the wait blocked — the poll under it stays the source of truth, which is upstream's
    /// own stated contract for the same subscription.
    ///
    /// Asserts presence before absence: the wait is first shown to be genuinely resolvable (it
    /// completes once the run really settles), so the "still blocked" half cannot pass because the
    /// wait was broken outright.
    #[tokio::test]
    async fn a_spurious_wake_does_not_resolve_a_wait_whose_run_is_still_active() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-bus-spurious");
        fx.write_status(&run, RunState::Running, false);

        let bus = crate::background::watch::CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_secs(5),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };

        let cancel = CancelToken::new();
        let params = WaitParams::default();
        let mut waiting = Box::pin(wait_for_subagents(&params, &cancel, &deps));

        // Three wakes with nothing settled: each one costs the loop a listing and must put it
        // straight back to sleep.
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            bus.publish(crate::background::watch::CompletionEvent {
                run_id: RunId::from_token("run-bus-spurious"),
                outcome: crate::background::watch::ClassifiedOutcome::Completed,
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(60), &mut waiting).await.is_err(),
                "a wake for a still-running run must not resolve the wait"
            );
        }

        // …and the wait really was resolvable all along.
        Fixture::settle(&fx.async_root, &fx.results_dir, &run);
        bus.publish(crate::background::watch::CompletionEvent {
            run_id: run,
            outcome: crate::background::watch::ClassifiedOutcome::Completed,
        });
        let text = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("the wait resolves once the run genuinely settles")
            .expect("wait ok");
        assert!(text.contains("1 of 1 run(s) finished"), "{text}");
    }

    /// A torn-down watcher (`SessionStart` replacing the session's watch, or the extension
    /// dropping it) closes every outstanding receiver. That must retire the subscription and leave
    /// the wait polling — NOT spin the loop, which is what an un-retired `Closed` receiver does
    /// (`recv` returns instantly, forever).
    #[tokio::test]
    async fn a_closed_bus_degrades_to_polling_instead_of_spinning() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-bus-closed");
        fx.write_status(&run, RunState::Running, false);

        let bus = crate::background::watch::CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_millis(MIN_POLL_INTERVAL_MS),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };
        // Every sender gone ⇒ the receiver this wait takes will report `Closed` on its first recv.
        drop(bus);

        // The run is still active when the wait starts, so the wait genuinely enters the loop with
        // a dead subscription — the condition this test exists for — and only the POLL can release
        // it.
        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Fixture::settle(&async_root, &results_dir, &settle_run);
        });

        let started = Instant::now();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps)
            .await
            .expect("wait still resolves with a dead bus");
        settler.await.expect("settler task");
        assert!(text.contains("1 of 1 run(s) finished"), "{text}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a closed bus must degrade to the poll, not wedge the wait"
        );
    }

    #[test]
    fn active_states_are_exactly_queued_and_running() {
        assert!(is_active(RunState::Queued));
        assert!(is_active(RunState::Running));
        assert!(!is_active(RunState::Paused));
        assert!(!is_active(RunState::Complete));
        assert!(!is_active(RunState::Failed));
        // G77: a stopped run is terminal, so it is never "active" — `wait` must not block on one.
        assert!(!is_active(RunState::Stopped));
    }
}
