//! [`WaitSubscriptionManager`] — the lifecycle: arm, restore, reconcile, settle, sweep, dispose.
//!
//! Ports pi `createWaitSubscriptionManager` (`wait-subscriptions.ts:99-347` @`7fe9dee1`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::background::wait_completions::{WaitCompletion, WaitCompletionStore};
use crate::background::{ActivityState, RunId, RunState, RunStatus, run_status};
use crate::error::SubagentError;
use crate::identity::SessionId;

use super::record::{
    ArmWaitSubscriptionInput, SubscriptionToken, SubscriptionVersion, WaitSubscriptionRecord,
    WaitTargetKind, parse_record, subscription_file,
};
use super::{FOREIGN_SWEEP_GRACE_MS, FOREIGN_SWEEP_INTERVAL_MS, RECONCILE_INTERVAL};

// =================================================================================================
// The settle vocabulary
// =================================================================================================

/// How one subscription ended — pi's six `settle(record, <string>, …)` spellings, as the set of
/// mutually exclusive outcomes they encode.
///
/// An enum and not a `&str`, for the same reason
/// [`crate::background::wait::WaitVerdict`](crate::background::wait::WaitVerdict) is not a `bool`:
/// every one of upstream's strings is a distinct `settle` site with a distinct meaning, and a
/// reader's `match` must be forced to decide between them. The `as_str` below is the ONLY place
/// the wire words live, so a rename cannot drift between the message and the `details.outcome`.
///
/// | upstream | line | meaning |
/// |---|---|---|
/// | `"timed out"` | `:208` | `now() >= record.expiresAt` |
/// | `"could not be reconciled"` | `:216`, `:241` | run/record disagreement (fg) or exact run gone (async) |
/// | `"needs attention"` | `:221`, `:245` | supervisor request / `needsAttention(run)` |
/// | `"completed"` / `"failed"` | `:226` | the two terminal foreground arms |
/// | `run.state` verbatim | `:257` | async terminal states (`complete` renders `completed`) |
/// | `"reconciliation failed"` | `:272` | `reconcileRecord` threw |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionOutcome {
    /// pi `"timed out"` (`:208`).
    TimedOut,
    /// pi `"could not be reconciled"` (`:216`, `:241`).
    CouldNotBeReconciled,
    /// pi `"needs attention"` (`:221`, `:245`).
    NeedsAttention,
    /// pi `"completed"` (`:226`), and `run.state === "complete"`'s rename at `:257`.
    Completed,
    /// pi `"failed"` (`:226`), and [`RunState::Failed`] rendered verbatim at `:257`.
    Failed,
    /// [`RunState::Stopped`] rendered verbatim at `:257`.
    Stopped,
    /// [`RunState::Paused`] rendered verbatim at `:257`.
    ///
    /// `paused` is NOT terminal for cyrup ([`RunState::is_terminal`] excludes it) but upstream's
    /// `:248` gate is `state !== "queued" && state !== "running"`, which lets a paused run settle
    /// its subscription. Ported as written: an interrupted run is exactly the state a caller who
    /// asked to be woken wants to hear about.
    Paused,
    /// pi `"reconciliation failed"` (`:272`).
    ReconciliationFailed,
}

impl SubscriptionOutcome {
    /// The wire word — pi's literal string at each `settle` site.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SubscriptionOutcome::TimedOut => "timed out",
            SubscriptionOutcome::CouldNotBeReconciled => "could not be reconciled",
            SubscriptionOutcome::NeedsAttention => "needs attention",
            SubscriptionOutcome::Completed => "completed",
            SubscriptionOutcome::Failed => "failed",
            SubscriptionOutcome::Stopped => "stopped",
            SubscriptionOutcome::Paused => "paused",
            SubscriptionOutcome::ReconciliationFailed => "reconciliation failed",
        }
    }

    /// pi `:257`'s `run.state === "complete" ? "completed" : run.state`.
    ///
    /// `None` for the two ACTIVE states, which is upstream's `:248` guard expressed as a total
    /// function rather than as an `if` the caller could forget: a queued or running run has no
    /// outcome, so there is nothing to settle.
    #[must_use]
    pub fn from_run_state(state: RunState) -> Option<Self> {
        match state {
            RunState::Queued | RunState::Running => None,
            RunState::Complete => Some(SubscriptionOutcome::Completed),
            RunState::Failed => Some(SubscriptionOutcome::Failed),
            RunState::Stopped => Some(SubscriptionOutcome::Stopped),
            RunState::Paused => Some(SubscriptionOutcome::Paused),
        }
    }
}

// pi's `settle` detail strings, verbatim. Named constants rather than inline literals because
// three of them are asserted by this module's own tests and by the executor's notifier.
const DETAIL_TIMED_OUT: &str =
    "The targeted run may still be active; inspect its status before taking follow-up action.";
const DETAIL_FOREGROUND_GONE: &str =
    "The remembered foreground run disappeared before completion was confirmed.";
const DETAIL_FOREGROUND_ATTENTION: &str =
    "Reply to the pending supervisor request or inspect the run status.";
const DETAIL_INSPECT_STATUS: &str = "Inspect the run status for its final output.";
const DETAIL_ASYNC_GONE: &str =
    "The exact async run disappeared before a terminal result was confirmed.";
const DETAIL_ASYNC_ATTENTION: &str =
    "Inspect the run status and answer any pending supervisor request.";
const DETAIL_RECONCILIATION_FAILED: &str =
    "The targeted run could not be reconciled. Inspect its status before taking follow-up action.";

/// pi `throw new Error(...)` at `:292`, verbatim — the one thing [`WaitSubscriptionManager::arm`]
/// refuses.
pub const NO_SESSION_IDENTITY: &str = "A wait subscription requires an active session identity.";

// =================================================================================================
// The three injected seams
// =================================================================================================

/// pi's `state.currentSessionId` (`:157`, `:181`, `:206`, `:266`, `:291`, `:315`, `:327`).
///
/// Injected rather than captured, because every one of those seven reads happens at a DIFFERENT
/// time and pi re-reads the live value each time. A manager that captured the session at
/// construction would keep settling into a session that has since been switched away.
pub trait SubscriptionSessions: Send + Sync {
    /// The live orchestrator session, or `None` when there is none (headless, unpersisted, or
    /// mid-rebuild). `None` is pi's falsy `state.currentSessionId`: `reconcile` still sweeps but
    /// settles nothing, and `arm` refuses.
    fn current_session_id(&self) -> Option<SessionId>;
}

/// Stands in for pi's `pi.sendMessage({...}, { triggerTurn: true })` (`:189-199`).
///
/// # Why a `bool`, and why the caller must honour it
///
/// pi's `sendMessage` is a synchronous in-process call that essentially cannot fail, so upstream
/// removes the record FIRST (`:183`) and merely logs a send failure (`:201`). cyrup's send crosses
/// a process boundary — [`cyrup_ext::host::HostServices::inject_message_ack`] — whose own
/// [`cyrup_ext::host::InjectOutcome::Accepted`] doc says it is *"the ONLY licence for the caller
/// to destroy its own copy of the data."* So this returns whether the wake actually reached the
/// session, and [`WaitSubscriptionManager`] removes the record only on `true`. See the module doc
/// for the full ordering decision.
#[async_trait::async_trait]
pub trait SubscriptionNotifier: Send + Sync {
    /// Deliver one wake. `true` means the session has it.
    async fn notify(
        &self,
        record: &WaitSubscriptionRecord,
        outcome: SubscriptionOutcome,
        detail: &str,
        completion: Option<&WaitCompletion>,
    ) -> bool;
}

/// Stands in for pi's `state.foregroundRuns?.get(record.runId)` (`:212`, `:329`).
///
/// # Why this cannot be a plain map lookup in cyrup
///
/// pi has ONE `state.foregroundRuns`. cyrup has TWO maps and they are disjoint by construction
/// (`extension/executor/mod.rs:188-192`: an entry in the settled history is created at the exact
/// point the live control entry is removed):
///
/// * `foreground_controls` holds LIVE runs and carries `current_activity_state`/`current_tool`,
///   which is what `:220`'s `contact_supervisor` test needs;
/// * `foreground_runs` — cyrup's literal `state.foregroundRuns` — holds SETTLED runs and carries
///   NEITHER of those fields, so `:220` is unrepresentable against it, but it DOES carry the
///   per-child `status` string `:224-227` needs.
///
/// Both types are `pub(crate)` inside `extension/executor/`, which sits ABOVE `background/`, so
/// this module cannot name either. The probe is therefore implemented up there and injected down
/// here — the same shape [`crate::background::wait::WaitDeps`] already uses for the completion
/// bus, the completion store and the inline-answer ledger.
pub trait ForegroundSubscriptionProbe: Send + Sync {
    /// The facts `:211-228` reads off a foreground run, or `None` when neither map knows it
    /// (upstream's `!run` arm).
    fn probe(&self, run_id: &RunId) -> Option<ForegroundTargetState>;
}

/// The ONLY facts `:211-228` reads off a foreground run, lifted out of the two executor-private
/// types that hold them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForegroundTargetState {
    /// pi `run.sessionId` (`:215`). `Option` because the LIVE map's own field is optional
    /// (`notices.rs:68`); a `None` here fails `:215`'s agreement check exactly as a mismatch does.
    pub session_id: Option<SessionId>,
    /// pi `run.children` (`:219`, `:225`). One entry per child.
    pub children: Vec<ForegroundTargetChild>,
}

/// One child of a foreground run, narrowed to pi's `:219-225` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundTargetChild {
    /// pi `child.status` (`:219`, `:225`).
    pub state: ForegroundChildState,
    /// pi `child.activityState` (`:220`). Always `None` for a settled child — the field is not
    /// persisted; see the module doc's `[CYRUP-DELTA]`.
    pub activity_state: Option<ActivityState>,
    /// pi `child.currentTool` (`:220`). `None` for a settled child, same reason.
    pub current_tool: Option<String>,
}

/// pi's `child.status`, narrowed to the three distinctions `:219-226` actually draws.
///
/// Upstream compares the raw `SubagentResultStatus` string against `"detached"` and `"failed"` and
/// lumps every other spelling (`completed`, `stopped`, …) into the final `else`. The enum makes
/// that partition explicit so the probe's implementor has to decide which bucket a status lands
/// in, rather than passing a string this module would silently mis-bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundChildState {
    /// pi `"detached"` — the child is still in flight. `detached.length > 0` is what keeps a
    /// foreground subscription armed (`:224`).
    Detached,
    /// pi `"failed"` — flips `:226`'s terminal arm from `completed` to `failed`.
    Failed,
    /// Every other terminal spelling (`completed`, `stopped`, …), which upstream's `:226` `else`
    /// collapses into one bucket.
    Settled,
}

// =================================================================================================
// The manager
// =================================================================================================

/// Everything [`WaitSubscriptionManager`] needs that it does not own.
pub struct WaitSubscriptionDeps {
    /// Where the `<token>.json` records live. See the module doc for why this is NOT
    /// `dirname(async_root)`.
    pub subscriptions_dir: PathBuf,
    /// The per-cwd async root, for the async branch's [`run_status::reconcile_by_id`].
    pub async_root: PathBuf,
    /// The per-cwd results dir, for the completion read.
    pub results_dir: PathBuf,
    /// pi `state.currentSessionId`.
    pub sessions: Arc<dyn SubscriptionSessions>,
    /// pi `pi.sendMessage`.
    pub notifier: Arc<dyn SubscriptionNotifier>,
    /// pi `state.foregroundRuns`.
    pub foreground: Arc<dyn ForegroundSubscriptionProbe>,
    /// The executor-owned consumed-payload record, so a fired subscription can report a completion
    /// whose payload this process's watcher already delivered and deleted. Shared with
    /// [`crate::background::wait::WaitDeps::wait_completions`]; an empty store is the documented
    /// degradation, not an error.
    pub wait_completions: Arc<WaitCompletionStore>,
}

/// pi's closure-captured mutable state (`:108-112`), as one lock.
///
/// `std::sync::Mutex` rather than `tokio::sync::Mutex`, matching `foreground_controls` and every
/// other synchronous registry in this crate: no critical section here spans an `.await`.
#[derive(Default)]
struct ManagerState {
    /// pi `state.waitSubscriptions` (`:108`), narrowed to THIS session by construction.
    ///
    /// A `BTreeMap` rather than a `HashMap`: [`WaitSubscriptionManager::armed`] hands its values
    /// straight to [`super::format_wait_subscriptions`], and a deterministic base order makes that
    /// function's `created_at` sort stable instead of permuting equal-timestamped rows run to run.
    subscriptions: BTreeMap<SubscriptionToken, WaitSubscriptionRecord>,
    /// pi `unresolvedRestoredForegroundTokens` (`:110`).
    unresolved_restored_foreground: BTreeSet<SubscriptionToken>,
    /// pi `disposed` (`:111`).
    disposed: bool,
    /// pi `lastForeignSweepAt` (`:112`).
    last_foreign_sweep_at: i64,
}

/// pi's `WaitSubscriptionManager` (`:46-51`) — the durable half of the wake.
///
/// Held behind an `Arc` because the 1 s reconcile timer ([`Self::restore`] arms it) is a spawned
/// `tokio` task that holds a [`Weak`] back-reference.
pub struct WaitSubscriptionManager {
    deps: WaitSubscriptionDeps,
    state: Mutex<ManagerState>,
    /// pi's `setInterval` handle (`:286`). See the module doc for why `.unref()` needs no analog.
    reconcile_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl WaitSubscriptionManager {
    /// pi `createWaitSubscriptionManager` (`:99-103`), minus the timer — which is armed by
    /// [`Self::restore`], not here. See the module doc.
    #[must_use]
    pub fn new(deps: WaitSubscriptionDeps) -> Arc<Self> {
        Arc::new(Self {
            deps,
            state: Mutex::new(ManagerState::default()),
            reconcile_task: Mutex::new(None),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// pi's `disposed` flag, read without holding the lock across anything.
    #[must_use]
    pub fn is_disposed(&self) -> bool {
        self.lock().disposed
    }

    /// The current-session records, for [`super::format_wait_subscriptions`].
    ///
    /// Session-scoped BY CONSTRUCTION: the only two writers are [`Self::arm`] (which stamps the
    /// live session) and [`Self::restore`] (behind `:327`'s session gate). The foreign sweep never
    /// inserts.
    #[must_use]
    pub fn armed(&self) -> Vec<WaitSubscriptionRecord> {
        self.lock().subscriptions.values().cloned().collect()
    }

    // ---------------------------------------------------------------------------------------
    // arm
    // ---------------------------------------------------------------------------------------

    /// pi `arm` (`:290-308`): mint a token, write the record, remember it.
    ///
    /// # Errors
    ///
    /// [`SubagentError::Management`] carrying [`NO_SESSION_IDENTITY`] when there is no live
    /// session (pi's `throw` at `:292`), or a [`SubagentError::Spawn`] when the record cannot be
    /// written — a subscription that is not on disk is not durable, so a failed write must be a
    /// refusal and never a silent in-memory-only arm.
    pub async fn arm(
        &self,
        input: ArmWaitSubscriptionInput,
    ) -> Result<WaitSubscriptionRecord, SubagentError> {
        let Some(session_id) = self.deps.sessions.current_session_id() else {
            return Err(SubagentError::Management(NO_SESSION_IDENTITY.to_string()));
        };
        let created_at = crate::time::now_epoch_millis();
        let record = WaitSubscriptionRecord {
            version: SubscriptionVersion,
            token: SubscriptionToken::mint(),
            session_id,
            target_kind: input.target_kind,
            run_id: input.run_id,
            requested_id: input.requested_id,
            created_at,
            // `saturating_add` rather than `+`: `timeout_ms` is caller-supplied and the sum is the
            // value every later comparison is made against. An overflowing timeout must read as
            // "effectively never", not wrap into the past and fire immediately.
            expires_at: created_at
                .saturating_add(i64::try_from(input.timeout_ms).unwrap_or(i64::MAX)),
        };
        // pi `fs.mkdirSync(subscriptionsDir, { recursive: true })` (`:304`).
        crate::background::ensure_accessible_dir(&self.deps.subscriptions_dir)
            .await
            .map_err(SubagentError::Spawn)?;
        // pi `writeAtomicJson` (`:305`) — temp file in the same parent, then rename, so a
        // concurrent session's sweep never reads a half-written record.
        crate::background::atomic::write_atomic_json(
            &subscription_file(&self.deps.subscriptions_dir, &record.token),
            &record,
        )
        .await
        .map_err(SubagentError::Spawn)?;
        self.lock()
            .subscriptions
            .insert(record.token.clone(), record.clone());
        Ok(record)
    }

    // ---------------------------------------------------------------------------------------
    // restore
    // ---------------------------------------------------------------------------------------

    /// pi `restore` (`:309-336`): clear, force-sweep, re-read this session's records, reconcile.
    ///
    /// Also arms the 1 s reconcile timer if it is not already running — see the module doc for why
    /// the timer lives here rather than in [`Self::new`].
    pub async fn restore(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            state.subscriptions.clear();
            state.unresolved_restored_foreground.clear();
        }
        // pi `:312-314` — UNCONDITIONAL: a process that starts without a session identity should
        // still clear records nobody can act on.
        self.sweep_expired_foreign_subscriptions(true).await;
        let Some(current) = self.deps.sessions.current_session_id() else {
            return;
        };
        for (file_name, record) in self.read_subscription_dir().await {
            // pi `:326` — a record may only be addressed by the name it claims.
            if file_name != subscription_file_name(&record.token) {
                continue;
            }
            // pi `:327` — the FIFTH session gate. Only this session's records enter the map, which
            // is what makes `format_wait_subscriptions` session-scoped without filtering.
            if record.session_id != current {
                continue;
            }
            // pi `:329` — the one-shot grace. A foreground record whose run is not (yet) in the
            // registry is remembered as unresolved so the first reconcile does NOT settle it
            // "could not be reconciled". Without this, every foreground subscription restored
            // before `restore_foreground_run_history_for` repopulates the registry dies on the
            // first pass, and the ordering between the two restores becomes load-bearing.
            let unresolved = record.target_kind == WaitTargetKind::Foreground
                && self.deps.foreground.probe(&record.run_id).is_none();
            let mut state = self.lock();
            if unresolved {
                state
                    .unresolved_restored_foreground
                    .insert(record.token.clone());
            }
            state.subscriptions.insert(record.token.clone(), record);
        }
        self.ensure_reconcile_timer();
        // pi `:335`.
        self.reconcile().await;
    }

    // ---------------------------------------------------------------------------------------
    // reconcile
    // ---------------------------------------------------------------------------------------

    /// pi `reconcile` (`:261-275`). Never propagates: every failure settles or is logged.
    pub async fn reconcile(&self) {
        if self.is_disposed() {
            return;
        }
        // pi `:263-265` — BEFORE the session check: with no current session every record is
        // foreign, and expired ones should still be cleaned up.
        self.sweep_expired_foreign_subscriptions(false).await;
        let Some(current) = self.deps.sessions.current_session_id() else {
            return;
        };
        // A snapshot, because `settle` mutates the map from inside the loop — pi takes the same
        // copy at `:267` (`[...subscriptions.values()]`) for the same reason.
        let records: Vec<WaitSubscriptionRecord> =
            self.lock().subscriptions.values().cloned().collect();
        for record in records {
            if let Err(error) = self.reconcile_record(&record, &current).await {
                // pi `:271-272` — a reconciliation that THREW settles the subscription rather than
                // leaving the caller waiting on a run nothing will look at again.
                tracing::warn!(
                    token = %record.token,
                    run_id = %record.run_id.as_str(),
                    %error,
                    "failed to reconcile wait subscription"
                );
                self.settle(
                    &record,
                    SubscriptionOutcome::ReconciliationFailed,
                    DETAIL_RECONCILIATION_FAILED,
                    None,
                )
                .await;
            }
        }
    }

    /// pi `reconcileRecord` (`:205-259`).
    async fn reconcile_record(
        &self,
        record: &WaitSubscriptionRecord,
        current: &SessionId,
    ) -> Result<(), SubagentError> {
        // pi `:206` — the THIRD session gate, and the FIRST statement. Note this is the OPPOSITE
        // comparison to the foreign sweep's `:157`: the sweep skips the current session, delivery
        // requires it. Different phases, not a contradiction.
        if &record.session_id != current {
            return Ok(());
        }
        // pi `:207-210`.
        if crate::time::now_epoch_millis() >= record.expires_at {
            self.settle(
                record,
                SubscriptionOutcome::TimedOut,
                DETAIL_TIMED_OUT,
                None,
            )
            .await;
            return Ok(());
        }
        // VL-S11b — BOTH arms now have a production producer. The foreground arm was ported
        // against a record format no cyrup surface could write: `bg_wait`'s candidate set was
        // async-only, which `background/wait.rs`'s own `[CYRUP-DELTA]` stated. `/subagents-detach`
        // is that producer — a detached foreground run is remembered in
        // `SubagentExecutor::foreground_runs`, `active_detached_foreground_runs` selects it, and
        // `{ id, nonBlocking: true }` arms [`WaitTargetKind::Foreground`] against it — so this
        // branch is reached from production rather than from this module's tests alone.
        match record.target_kind {
            WaitTargetKind::Foreground => {
                self.reconcile_foreground(record).await;
                Ok(())
            }
            WaitTargetKind::Async => self.reconcile_async(record).await,
        }
    }

    /// pi `:211-229` — the foreground branch.
    async fn reconcile_foreground(&self, record: &WaitSubscriptionRecord) {
        let run = self.deps.foreground.probe(&record.run_id);
        {
            let mut state = self.lock();
            match run {
                // pi `:213` — the run is still missing and this token was restored unresolved:
                // return WITHOUT settling and try again on the next pass.
                None if state.unresolved_restored_foreground.contains(&record.token) => return,
                // pi `:214` — the run appeared, so the grace is spent.
                Some(_) => {
                    state.unresolved_restored_foreground.remove(&record.token);
                }
                None => {}
            }
        }
        // pi `:215` — the FOURTH session gate: the run's own session must agree with the record's.
        // This is the same contents-decide discipline the result index applies to hashed addresses.
        let Some(run) = run.filter(|run| run.session_id.as_ref() == Some(&record.session_id))
        else {
            self.settle(
                record,
                SubscriptionOutcome::CouldNotBeReconciled,
                DETAIL_FOREGROUND_GONE,
                None,
            )
            .await;
            return;
        };
        // pi `:219`.
        let detached: Vec<&ForegroundTargetChild> = run
            .children
            .iter()
            .filter(|child| child.state == ForegroundChildState::Detached)
            .collect();
        // pi `:220` — a pending supervisor request on a still-detached child.
        if detached.iter().any(|child| {
            child.activity_state == Some(ActivityState::NeedsAttention)
                && child.current_tool.as_deref() == Some("contact_supervisor")
        }) {
            self.settle(
                record,
                SubscriptionOutcome::NeedsAttention,
                DETAIL_FOREGROUND_ATTENTION,
                None,
            )
            .await;
            return;
        }
        // pi `:224-227` — every child has landed.
        if detached.is_empty() {
            let failed = run
                .children
                .iter()
                .any(|child| child.state == ForegroundChildState::Failed);
            let outcome = if failed {
                SubscriptionOutcome::Failed
            } else {
                SubscriptionOutcome::Completed
            };
            self.settle(record, outcome, DETAIL_INSPECT_STATUS, None)
                .await;
        }
        // Otherwise: still detached, still armed. pi `:228`'s bare `return`.
    }

    /// pi `:231-258` — the async branch.
    ///
    /// # Why this is NOT [`run_status::list_active_runs`]
    ///
    /// Upstream calls `listAsyncRuns({ sessionId, runId, exactRunId: true, … })` with NO `states:`
    /// option, deliberately, because `:248`'s `run.state !== "queued" && !== "running"` is the
    /// whole point: it needs the run back precisely WHEN it has gone terminal.
    /// [`run_status::list_active_runs`] is hard-filtered to `Queued | Running`
    /// (`run_status.rs:724`), so a literal port would settle every completed run as
    /// `"could not be reconciled"` — the exact opposite of the feature.
    /// [`run_status::reconcile_by_id`] applies the same R-SA-079 reconciliation gate, returns the
    /// [`RunStatus`] in ANY state, and maps "neither file exists" to `Ok(None)` — which is
    /// upstream's `if (!run)`. It carries two follow-through obligations, both discharged below.
    async fn reconcile_async(&self, record: &WaitSubscriptionRecord) -> Result<(), SubagentError> {
        let resolved = run_status::reconcile_by_id(
            &self.deps.async_root,
            &self.deps.results_dir,
            record.run_id.as_str(),
        )
        .await?;
        // pi `:239-243`.
        let Some((status, _paths)) = resolved else {
            self.settle(
                record,
                SubscriptionOutcome::CouldNotBeReconciled,
                DETAIL_ASYNC_GONE,
                None,
            )
            .await;
            return Ok(());
        };
        // Obligation 1 — upstream's `exactRunId: true` (`:234`) plus its `runs.find(c => c.id ===
        // record.runId)` (`:239`). `reconcile_by_id` resolves a PREFIX, so a prefix that now
        // matches a DIFFERENT run must never satisfy this subscription.
        //
        // Obligation 2 — upstream gets the session filter for free by passing `sessionId` into
        // `listAsyncRuns` (which applies it at `async-status.ts:432`, cyrup
        // `run_status.rs:719-723`). `reconcile_by_id` applies NONE, so it is re-applied here.
        // This is the async twin of the foreground branch's `:215` gate.
        if status.run_id != record.run_id || status.session_id.as_ref() != Some(&record.session_id)
        {
            self.settle(
                record,
                SubscriptionOutcome::CouldNotBeReconciled,
                DETAIL_ASYNC_GONE,
                None,
            )
            .await;
            return Ok(());
        }
        // pi `:244-247`.
        if async_needs_attention(&status) {
            self.settle(
                record,
                SubscriptionOutcome::NeedsAttention,
                DETAIL_ASYNC_ATTENTION,
                None,
            )
            .await;
            return Ok(());
        }
        // pi `:248` — the gate, expressed as `from_run_state`'s `None` for the two active states.
        let Some(outcome) = SubscriptionOutcome::from_run_state(status.state) else {
            return Ok(());
        };
        let completion = self.read_completion(&status).await;
        // pi `:255` — SUBA-060's per-run guidance, whose ONLY upstream call site is this line.
        let detail =
            crate::background::resume_guidance::format_resume_first_failed_run_detail(&status)
                .unwrap_or_else(|| DETAIL_INSPECT_STATUS.to_string());
        // pi `:256`.
        let archive_detail = completion
            .as_ref()
            .and_then(|completion| completion.archive_path.as_deref())
            .map(|path| format!(" Completion archive: {path}."))
            .unwrap_or_default();
        self.settle(
            record,
            outcome,
            &format!("{detail}{archive_detail}"),
            completion,
        )
        .await;
        Ok(())
    }

    /// pi `:249-254` — the completion this wake replays.
    ///
    /// **`[CYRUP-DELTA]`** upstream reads the durable replay record ALONE
    /// (`readCompletionReplay`, `:251`). cyrup routes through
    /// [`crate::background::wait_completions::collect_wait_completions`], the three-rung reader
    /// SCOPE_3/SCOPE_4 landed, whose THIRD rung is exactly that call. The reason is that rungs 1
    /// and 2 answer strictly more cases: when a subscription fires before the watcher has
    /// delivered, the payload is still on disk and the replay record does not exist yet, so
    /// upstream reports no completion at all where cyrup reports the real one. Routing through the
    /// shared reader also means there is exactly one implementation of the resolution order in
    /// this crate rather than two that can drift.
    ///
    /// Never fails the settle: pi wraps its own read in `try`/`catch` and logs (`:252-254`),
    /// because a wake with no completion attached is still a wake.
    async fn read_completion(&self, status: &RunStatus) -> Option<WaitCompletion> {
        match crate::background::wait_completions::collect_wait_completions(
            std::slice::from_ref(status),
            &self.deps.wait_completions,
            &self.deps.results_dir,
        )
        .await
        {
            Ok(completions) => completions.into_iter().next(),
            Err(error) => {
                tracing::warn!(
                    run_id = %status.run_id.as_str(),
                    %error,
                    "failed to read completion for wait subscription"
                );
                None
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // settle / remove
    // ---------------------------------------------------------------------------------------

    /// pi `settle` (`:180-203`) — with upstream's remove-then-send ORDER DELIBERATELY REVERSED.
    ///
    /// # The ordering decision, and why it is not upstream's
    ///
    /// pi removes the record (`:183`) and only then sends (`:189`), accepting the loss on a failed
    /// send (`:201`) because its `sendMessage` is a synchronous in-process call. cyrup's send
    /// crosses a process boundary, and [`cyrup_ext::host::InjectOutcome::Accepted`] is documented
    /// as *"the ONLY licence for the caller to destroy its own copy of the data"* — so the wake is
    /// AWAITED first and the record is cleared only once the session has it.
    ///
    /// The trade this makes is duplicate-wake risk (accepted, then the process dies before the
    /// unlink) against lost-wake risk (unlinked, then the injection never lands). A duplicate wake
    /// is a redundant notice the orchestrator can ignore; a lost wake is a caller blocked forever
    /// on a run that already finished. And the duplicate window is narrow by construction:
    /// [`Self::remove`] drops the in-memory entry unconditionally, so only a `restore()` in a
    /// LATER process can re-read a record whose unlink failed.
    async fn settle(
        &self,
        record: &WaitSubscriptionRecord,
        outcome: SubscriptionOutcome,
        detail: &str,
        completion: Option<WaitCompletion>,
    ) {
        // pi `:181` — the SIXTH session comparison, redundant with `:206` on the `reconcileRecord`
        // path and NOT redundant overall: `settle` is also reachable from `reconcile`'s catch arm
        // (`:272`), which runs after a `reconcileRecord` THROW — i.e. after a point where the
        // session could have been swapped. Upstream has both, deliberately.
        if self.is_disposed()
            || self.deps.sessions.current_session_id().as_ref() != Some(&record.session_id)
        {
            return;
        }
        let delivered = self
            .deps
            .notifier
            .notify(record, outcome, detail, completion.as_ref())
            .await;
        if !delivered {
            // pi `:201` logs and drops. cyrup keeps the record armed instead: the next reconcile
            // re-delivers once the session is available again, which is the whole point of taking
            // the acknowledged seam.
            tracing::warn!(
                token = %record.token,
                run_id = %record.run_id.as_str(),
                "wait subscription wake was not accepted by the session; it remains armed"
            );
            return;
        }
        self.remove(record).await;
    }

    /// pi `remove` (`:170-178`), with the two halves reordered so the in-memory drop cannot be
    /// skipped by a failing unlink — see [`Self::settle`] for why that matters here and not
    /// upstream.
    async fn remove(&self, record: &WaitSubscriptionRecord) {
        {
            let mut state = self.lock();
            state.subscriptions.remove(&record.token);
            state.unresolved_restored_foreground.remove(&record.token);
        }
        let path = subscription_file(&self.deps.subscriptions_dir, &record.token);
        if let Err(error) = tokio::fs::remove_file(&path).await
            && !crate::background::result_index::errno::is_absent(&error)
        {
            // pi `:174` rethrows here; cyrup cannot, because the wake has already been delivered.
            // The record is out of the live map, so this process will not re-fire it; a later
            // process's `restore()` might, which is the documented duplicate window.
            tracing::warn!(
                path = %path.display(),
                %error,
                "failed to unlink a fired wait subscription record"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // the foreign sweep
    // ---------------------------------------------------------------------------------------

    /// pi `sweepExpiredForeignSubscriptions` (`:134-168`): remove expired records armed by ANOTHER
    /// session.
    ///
    /// Only the owning session can be woken, so [`Self::restore`] never loads a foreign record and
    /// [`Self::reconcile_record`] returns before its timeout branch. Nothing reconciles such a
    /// record and nothing expires it, so a session that arms a wait and never comes back (forked,
    /// renamed, deleted) would leave one file here per wait, forever.
    ///
    /// Kept for [`FOREIGN_SWEEP_GRACE_MS`] past expiry so an owner that resumes late still finds
    /// its record and gets the timeout notice — which is exactly why `:157` skips the CURRENT
    /// session even when its record is expired.
    ///
    /// **Never throws** (pi `:132`): it runs from the reconcile timer. Every I/O fault is logged
    /// (unless it is mere absence) and the sweep moves on.
    async fn sweep_expired_foreign_subscriptions(&self, force: bool) {
        let swept_at = crate::time::now_epoch_millis();
        {
            let mut state = self.lock();
            // pi `:136-137` — a THROTTLE, not a timer: the sweep costs a directory read and
            // nothing depends on it being prompt.
            if !force && swept_at - state.last_foreign_sweep_at < FOREIGN_SWEEP_INTERVAL_MS {
                return;
            }
            state.last_foreign_sweep_at = swept_at;
        }
        let current = self.deps.sessions.current_session_id();
        for (file_name, record) in self.read_subscription_dir().await {
            // pi `:154`.
            if file_name != subscription_file_name(&record.token) {
                continue;
            }
            // pi `:155-157` — the SECOND session gate, and the OPPOSITE comparison to `:206`. The
            // owning session keeps its own expired records so `reconcile_record` can still settle
            // them with the "timed out" notice callers expect.
            if current.as_ref() == Some(&record.session_id) {
                continue;
            }
            // pi `:158`.
            if swept_at < record.expires_at.saturating_add(FOREIGN_SWEEP_GRACE_MS) {
                continue;
            }
            let path = subscription_file(&self.deps.subscriptions_dir, &record.token);
            if let Err(error) = tokio::fs::remove_file(&path).await
                && !crate::background::result_index::errno::is_absent(&error)
            {
                // pi `:162-165` — losing this race is harmless: another session may have swept the
                // same expired record. Anything else is reported and retried on the next sweep
                // rather than interrupting the caller.
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "failed to remove an expired foreign wait subscription"
                );
            }
        }
    }

    /// The shared directory read behind `:138-153` and `:316-334`: every `*.json` that parses,
    /// paired with its own file name so the caller can apply the name/token agreement check.
    ///
    /// Absence is silent (`:141-144`, `:320`); every other fault is logged once and yields an
    /// empty listing, because BOTH callers are contractually non-throwing.
    async fn read_subscription_dir(&self) -> Vec<(String, WaitSubscriptionRecord)> {
        let mut entries = match tokio::fs::read_dir(&self.deps.subscriptions_dir).await {
            Ok(entries) => entries,
            Err(error) => {
                if !crate::background::result_index::errno::is_absent(&error) {
                    tracing::warn!(
                        path = %self.deps.subscriptions_dir.display(),
                        %error,
                        "failed to scan wait subscriptions"
                    );
                }
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                        continue;
                    };
                    // pi `:140`/`:318` — `.filter((file) => file.endsWith(".json"))`.
                    if !name.ends_with(crate::identity::ResultFileName::EXTENSION) {
                        continue;
                    }
                    match tokio::fs::read(entry.path()).await {
                        // pi `:150-153`/`:331-333` — an unparseable record is skipped, never fatal.
                        Ok(bytes) => {
                            if let Some(record) = parse_record(&bytes) {
                                out.push((name, record));
                            } else {
                                tracing::warn!(
                                    path = %entry.path().display(),
                                    "ignoring invalid wait subscription"
                                );
                            }
                        }
                        Err(error) => {
                            if !crate::background::result_index::errno::is_absent(&error) {
                                tracing::warn!(
                                    path = %entry.path().display(),
                                    %error,
                                    "ignoring unreadable wait subscription"
                                );
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(
                        path = %self.deps.subscriptions_dir.display(),
                        %error,
                        "failed to scan wait subscriptions"
                    );
                    break;
                }
            }
        }
        out
    }

    // ---------------------------------------------------------------------------------------
    // the timer, and dispose
    // ---------------------------------------------------------------------------------------

    /// pi `setInterval(reconcile, 1000)` (`:286`).
    ///
    /// Armed from [`Self::restore`] rather than from [`Self::new`] because `tokio::spawn` needs a
    /// runtime and the executor is constructed outside one; `restore` runs on the `SessionStart`
    /// edge, which is where every other cyrup timer in this subsystem is armed
    /// (`notices.rs:494-498`'s retention sweep). Idempotent: a second `restore` reuses the task.
    fn ensure_reconcile_timer(self: &Arc<Self>) {
        let mut slot = self
            .reconcile_task
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        // WEAK, not strong: a strong handle inside the task would be a reference cycle through the
        // task's own closure, so `Drop` would never run and the task would outlive its manager.
        let manager = Arc::downgrade(self);
        *slot = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(RECONCILE_INTERVAL).await;
                let Some(manager) = Weak::upgrade(&manager) else {
                    return;
                };
                if manager.is_disposed() {
                    return;
                }
                manager.reconcile().await;
            }
        }));
    }

    /// pi `dispose` (`:338-346`): stop the timer, drop the map, refuse every later settle.
    ///
    /// **`[CYRUP-DELTA]`** pi's `interval.unref?.()` (`:287`) has no analog and needs none — a
    /// `tokio` task does not hold the runtime open the way a libuv timer holds the loop. The
    /// unsubscribe loop at `:342-344` likewise has none: cyrup's wake edge is a
    /// [`crate::background::watch::CompletionObserver`] member owned by the composite the executor
    /// rebuilds on every `SessionStart`, so the old registration is dropped rather than
    /// individually unsubscribed — the same argument `notices.rs:480-484` already makes for the
    /// completion watcher's own re-install.
    pub fn dispose(&self) {
        {
            let mut state = self.lock();
            if state.disposed {
                return;
            }
            state.disposed = true;
            state.subscriptions.clear();
            state.unresolved_restored_foreground.clear();
        }
        self.abort_reconcile_timer();
    }

    fn abort_reconcile_timer(&self) {
        if let Some(task) = self
            .reconcile_task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

/// pi's `Pick<WaitSubscriptionManager, "arm">` (`wait-tool.ts:12`) — the one method the `wait`
/// tool is handed, as the trait `background/wait.rs` declares so it need not name this type.
#[async_trait::async_trait]
impl crate::background::wait::WaitSubscriptionArming for WaitSubscriptionManager {
    async fn arm(
        &self,
        input: ArmWaitSubscriptionInput,
    ) -> Result<WaitSubscriptionRecord, SubagentError> {
        WaitSubscriptionManager::arm(self, input).await
    }
}

/// So a forgotten [`WaitSubscriptionManager::dispose`] cannot leak the reconcile task — the same
/// belt-and-braces `CompletionWatcherHandle` uses (`watch/install.rs:37`).
impl Drop for WaitSubscriptionManager {
    fn drop(&mut self) {
        self.abort_reconcile_timer();
    }
}

/// pi `needsAttention` (`:81-83`) — run-level OR any step.
///
/// **Deliberately NOT [`crate::background::wait`]'s own `needs_attention`**, which is run-level
/// only. The step disjunct is load-bearing here: `RunStatus::sync_top_level_telemetry`
/// (`records.rs:398-425`) rolls up `current_tool`, `tool_count`, `turn_count`, `total_tokens` and
/// `last_activity_at` — and NOT `activity_state`. A step can therefore be
/// [`ActivityState::NeedsAttention`] while `RunTelemetry::activity_state` is `None`, and reusing
/// the blocking wait's predicate would silently drop exactly that case.
///
/// **`[CYRUP-DELTA]`** that `background/wait.rs`'s predicate is missing the same disjunct is a
/// genuine divergence affecting the BLOCKING wait. It is a SEPARATE defect and out of scope here;
/// it is recorded at this seam so the next reader does not re-derive it as new.
fn async_needs_attention(status: &RunStatus) -> bool {
    status.telemetry.activity_state == Some(ActivityState::NeedsAttention)
        || status
            .steps
            .iter()
            .any(|step| step.telemetry.activity_state == Some(ActivityState::NeedsAttention))
}

/// The basename a record must be stored under — `:154`/`:326`'s right-hand side.
fn subscription_file_name(token: &SubscriptionToken) -> String {
    format!("{token}{}", crate::identity::ResultFileName::EXTENSION)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::background::wait_subscriptions::{SUBSCRIPTION_VERSION, format_wait_subscriptions};
    use crate::background::{RunMode, RunPaths, StepStatus};

    const SESSION_A: &str = "sess-a";
    const SESSION_B: &str = "sess-b";
    const TOKEN_A: &str = "11111111-1111-4111-8111-111111111111";
    const TOKEN_B: &str = "22222222-2222-4222-9222-222222222222";
    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("non-empty session id")
    }

    /// pi `state.currentSessionId`, switchable mid-test — which is the only way to reach `:206`
    /// (a record loaded under one session, reconciled under another).
    #[derive(Default)]
    struct TestSessions(Mutex<Option<SessionId>>);

    impl TestSessions {
        fn set(&self, raw: Option<&str>) {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = raw.map(session);
        }
    }

    impl SubscriptionSessions for TestSessions {
        fn current_session_id(&self) -> Option<SessionId> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[derive(Clone, Debug)]
    struct Captured {
        token: SubscriptionToken,
        outcome: SubscriptionOutcome,
        detail: String,
        completion: Option<WaitCompletion>,
    }

    /// pi `pi.sendMessage`. `accept` is cyrup's own addition — the acknowledged seam's `false`,
    /// which must leave the record ARMED rather than clearing it.
    struct TestNotifier {
        accept: std::sync::atomic::AtomicBool,
        captured: Mutex<Vec<Captured>>,
    }

    impl Default for TestNotifier {
        fn default() -> Self {
            Self {
                accept: std::sync::atomic::AtomicBool::new(true),
                captured: Mutex::new(Vec::new()),
            }
        }
    }

    impl TestNotifier {
        fn captured(&self) -> Vec<Captured> {
            self.captured
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn only(&self) -> Captured {
            let captured = self.captured();
            assert_eq!(captured.len(), 1, "expected exactly one wake: {captured:?}");
            captured[0].clone()
        }
    }

    #[async_trait::async_trait]
    impl SubscriptionNotifier for TestNotifier {
        async fn notify(
            &self,
            record: &WaitSubscriptionRecord,
            outcome: SubscriptionOutcome,
            detail: &str,
            completion: Option<&WaitCompletion>,
        ) -> bool {
            self.captured
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Captured {
                    token: record.token.clone(),
                    outcome,
                    detail: detail.to_string(),
                    completion: completion.cloned(),
                });
            self.accept.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// pi `state.foregroundRuns`, as the injected two-map probe.
    #[derive(Default)]
    struct TestProbe(Mutex<HashMap<RunId, ForegroundTargetState>>);

    impl TestProbe {
        fn set(&self, run_id: &RunId, state: ForegroundTargetState) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(run_id.clone(), state);
        }
    }

    impl ForegroundSubscriptionProbe for TestProbe {
        fn probe(&self, run_id: &RunId) -> Option<ForegroundTargetState> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(run_id)
                .cloned()
        }
    }

    fn child(state: ForegroundChildState) -> ForegroundTargetChild {
        ForegroundTargetChild {
            state,
            activity_state: None,
            current_tool: None,
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        subs: PathBuf,
        async_root: PathBuf,
        results_dir: PathBuf,
        sessions: Arc<TestSessions>,
        notifier: Arc<TestNotifier>,
        probe: Arc<TestProbe>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let subs = dir.path().join("wait-subscriptions");
            let async_root = dir.path().join("async");
            let results_dir = dir.path().join("results");
            for path in [&subs, &async_root, &results_dir] {
                std::fs::create_dir_all(path).expect("mkdir");
            }
            let sessions = Arc::new(TestSessions::default());
            sessions.set(Some(SESSION_A));
            Self {
                _dir: dir,
                subs,
                async_root,
                results_dir,
                sessions,
                notifier: Arc::new(TestNotifier::default()),
                probe: Arc::new(TestProbe::default()),
            }
        }

        /// A manager over this fixture's directories, sharing its three seams. The
        /// [`WaitCompletionStore`] is FRESH and EMPTY every time, which is what starves rungs 1-2
        /// of the three-rung completion read so the replay test cannot pass vacuously.
        fn manager(&self) -> Arc<WaitSubscriptionManager> {
            WaitSubscriptionManager::new(WaitSubscriptionDeps {
                subscriptions_dir: self.subs.clone(),
                async_root: self.async_root.clone(),
                results_dir: self.results_dir.clone(),
                sessions: Arc::clone(&self.sessions) as Arc<dyn SubscriptionSessions>,
                notifier: Arc::clone(&self.notifier) as Arc<dyn SubscriptionNotifier>,
                foreground: Arc::clone(&self.probe) as Arc<dyn ForegroundSubscriptionProbe>,
                wait_completions: Arc::new(WaitCompletionStore::default()),
            })
        }

        fn paths(&self, run_id: &RunId) -> RunPaths {
            RunPaths::for_run(&self.async_root, &self.results_dir, run_id)
        }

        /// The on-disk shape `reconcile_by_id` reads.
        fn write_status(&self, run_id: &RunId, state: RunState, session_id: Option<&str>) {
            self.write_status_with(run_id, state, session_id, |_| {});
        }

        fn write_status_with(
            &self,
            run_id: &RunId,
            state: RunState,
            session_id: Option<&str>,
            mutate: impl FnOnce(&mut RunStatus),
        ) {
            let paths = self.paths(run_id);
            std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
            let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
            status.state = state;
            status.session_id = SessionId::parse_opt(session_id);
            mutate(&mut status);
            std::fs::write(
                &paths.status,
                serde_json::to_string(&status).expect("status serializes"),
            )
            .expect("write status.json");
        }

        /// The authoritative terminal payload — the thing rung 2 of the completion read finds.
        fn write_result(&self, run_id: &RunId, session_id: Option<&str>) {
            let paths = self.paths(run_id);
            let result = crate::background::ResultFile {
                schedule_origin: None,
                id: run_id.clone(),
                run_id: run_id.clone(),
                agent: "worker".to_string(),
                mode: RunMode::Single,
                state: RunState::Complete,
                success: true,
                cwd: PathBuf::from("/tmp"),
                session_file: None,
                session_id: SessionId::parse_opt(session_id),
                completion_owner_id: None,
                results: Vec::new(),
                workflow_children: None,
                workflow_receipt: None,
            };
            std::fs::write(
                &paths.legacy_result_root,
                serde_json::to_string(&result).expect("result serializes"),
            )
            .expect("write result file");
        }

        fn record(
            &self,
            token: &str,
            session_id: &str,
            target_kind: WaitTargetKind,
            run_id: &str,
            expires_at: i64,
        ) -> WaitSubscriptionRecord {
            WaitSubscriptionRecord {
                version: SubscriptionVersion,
                token: SubscriptionToken::parse(token).expect("test token"),
                session_id: session(session_id),
                target_kind,
                run_id: RunId::from_token(run_id.to_string()),
                requested_id: run_id.to_string(),
                created_at: crate::time::now_epoch_millis(),
                expires_at,
            }
        }

        /// Write a record under an EXPLICIT file name, so the name/token disagreement case is
        /// reachable.
        fn write_record_as(&self, file_name: &str, record: &WaitSubscriptionRecord) -> PathBuf {
            let path = self.subs.join(file_name);
            std::fs::write(
                &path,
                serde_json::to_vec(record).expect("record serializes"),
            )
            .expect("write record");
            path
        }

        fn write_record(&self, record: &WaitSubscriptionRecord) -> PathBuf {
            self.write_record_as(&format!("{}.json", record.token), record)
        }

        fn record_path(&self, token: &SubscriptionToken) -> PathBuf {
            subscription_file(&self.subs, token)
        }
    }

    fn exists(path: &Path) -> bool {
        std::fs::metadata(path).is_ok()
    }

    // =============================================================================================
    // arm
    // =============================================================================================

    /// pi `:291-292` — a subscription with no owner could never be woken, so arming refuses
    /// rather than persisting a record nobody can act on.
    #[tokio::test]
    async fn arming_without_a_session_identity_is_an_error() {
        let fx = Fixture::new();
        fx.sessions.set(None);
        let manager = fx.manager();
        let error = manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Async,
                run_id: RunId::from_token("run-1"),
                requested_id: "run".to_string(),
                timeout_ms: 60_000,
            })
            .await
            .expect_err("no session identity");
        assert_eq!(error.to_string(), NO_SESSION_IDENTITY);
        // And nothing was written: the refusal is total, not "in memory only".
        assert_eq!(std::fs::read_dir(&fx.subs).expect("read subs").count(), 0);
    }

    // =============================================================================================
    // the wake
    // =============================================================================================

    /// The feature: a run that goes terminal fires its subscription, the wake names the outcome,
    /// and the record is GONE afterwards.
    #[tokio::test]
    async fn an_armed_subscription_fires_when_the_run_completes() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        fx.write_status(&run, RunState::Running, Some(SESSION_A));
        let manager = fx.manager();
        let record = manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Async,
                run_id: run.clone(),
                requested_id: "run".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");
        let path = fx.record_path(&record.token);
        assert!(
            exists(&path),
            "the record is on disk the moment it is armed"
        );

        // Still running: nothing fires.
        manager.reconcile().await;
        assert!(fx.notifier.captured().is_empty());
        assert!(exists(&path));

        // Terminal. Driven by an explicit `reconcile()`, never by sleeping out the 1 s timer.
        fx.write_status(&run, RunState::Complete, Some(SESSION_A));
        fx.write_result(&run, Some(SESSION_A));
        manager.reconcile().await;

        let fired = fx.notifier.only();
        assert_eq!(fired.token, record.token);
        assert_eq!(fired.outcome, SubscriptionOutcome::Completed);
        assert!(!exists(&path), "a fired record is unlinked");
        assert!(manager.armed().is_empty());
    }

    /// §6.1 — the async branch must find a run that is ALREADY terminal when it first looks. Built
    /// to fail against a `list_active_runs`-based implementation: that listing is hard-filtered to
    /// `Queued | Running` (`run_status.rs:724`), so a completed run is simply absent from it and a
    /// literal port settles this as "could not be reconciled".
    #[tokio::test]
    async fn an_async_run_that_is_terminal_is_found_and_settled() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        // Terminal BEFORE the subscription is ever looked at.
        fx.write_status(&run, RunState::Failed, Some(SESSION_A));
        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Async,
            "run-1",
            crate::time::now_epoch_millis() + 10 * HOUR_MS,
        );
        fx.write_record(&record);

        let manager = fx.manager();
        manager.restore().await;

        let fired = fx.notifier.only();
        assert_eq!(
            fired.outcome,
            SubscriptionOutcome::Failed,
            "a terminal run's own state is the outcome, verbatim (pi `:257`)"
        );
        assert_ne!(fired.outcome, SubscriptionOutcome::CouldNotBeReconciled);
        assert!(!exists(&fx.record_path(&record.token)));
    }

    /// §6.2 — `needsAttention` is run-level OR any STEP (`:81-83`). `sync_top_level_telemetry`
    /// does not roll `activity_state` up, so a step can need attention while the run's own
    /// telemetry says nothing. Fails against `background/wait.rs:407`'s run-level-only predicate.
    #[tokio::test]
    async fn a_step_level_needs_attention_settles_the_subscription() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        fx.write_status_with(&run, RunState::Running, Some(SESSION_A), |status| {
            let mut step = StepStatus::pending("worker");
            step.telemetry.activity_state = Some(ActivityState::NeedsAttention);
            status.steps = vec![step];
            // Deliberately left alone: the RUN-level field says nothing.
            assert_eq!(status.telemetry.activity_state, None);
        });
        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Async,
            "run-1",
            crate::time::now_epoch_millis() + 10 * HOUR_MS,
        );
        fx.write_record(&record);

        let manager = fx.manager();
        manager.restore().await;

        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::NeedsAttention);
        assert_eq!(fired.detail, DETAIL_ASYNC_ATTENTION);
    }

    /// §8 / SCOPE_4 — the hard case: the payload is gone and this process's in-process record is
    /// EMPTY, so only the durable replay record can answer. Without rung 3 the wake carries no
    /// completion and no archive sentence at all.
    #[tokio::test]
    async fn an_armed_subscription_fires_after_cleanup_via_the_replay_record() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        fx.write_status(&run, RunState::Complete, Some(SESSION_A));
        // NO result payload on disk: `write_result` is deliberately not called, which is the state
        // `deliver_pending_completions` leaves behind after its delete-last.
        let now = crate::time::now_epoch_millis();
        let written = crate::background::completion_replay::write_completion_replay(
            &crate::background::completion_replay::CompletionReplayWrite {
                results_dir: &fx.results_dir,
                run_id: &run,
                session_id: &session(SESSION_A),
                completion: &WaitCompletion {
                    run_id: run.as_str().to_string(),
                    ..WaitCompletion::default()
                },
                data: &serde_json::json!({ "runId": run.as_str(), "state": "complete" }),
                now,
                ttl_ms: 10 * HOUR_MS,
            },
        )
        .await
        .expect("replay written");
        let archive_path = written
            .completion
            .archive_path
            .clone()
            .expect("the replay record carries its archive path (pi `:196`)");

        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Async,
            "run-1",
            now + 10 * HOUR_MS,
        );
        fx.write_record(&record);

        // A FRESH manager, so its `WaitCompletionStore` is empty and rungs 1-2 cannot answer.
        let manager = fx.manager();
        manager.restore().await;

        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::Completed);
        let completion = fired
            .completion
            .expect("the replay record is what supplies the completion");
        assert_eq!(
            completion.archive_path.as_deref(),
            Some(archive_path.as_str())
        );
        // pi `:256` — the archive sentence is appended to the detail, not left on the payload.
        assert!(
            fired
                .detail
                .ends_with(&format!(" Completion archive: {archive_path}.")),
            "{}",
            fired.detail
        );
    }

    /// The durability claim itself: the subscription must cross a manager instance, not merely
    /// live in one process's map. Manager #1 arms and is disposed; manager #2 is built over the
    /// SAME directory with a fresh, empty completion store and finds the record on disk.
    #[tokio::test]
    async fn a_subscription_survives_a_turn_boundary() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        fx.write_status(&run, RunState::Running, Some(SESSION_A));

        let first = fx.manager();
        let record = first
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Async,
                run_id: run.clone(),
                requested_id: "run".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");
        first.dispose();
        assert!(
            first.armed().is_empty(),
            "dispose drops the in-memory map — the on-disk record is what survives"
        );

        // The turn boundary. Everything in-process is gone.
        fx.write_status(&run, RunState::Complete, Some(SESSION_A));
        fx.write_result(&run, Some(SESSION_A));
        let second = fx.manager();
        second.restore().await;

        let fired = fx.notifier.only();
        assert_eq!(fired.token, record.token);
        assert_eq!(fired.outcome, SubscriptionOutcome::Completed);
        assert!(!exists(&fx.record_path(&record.token)));
    }

    // =============================================================================================
    // the session gates
    // =============================================================================================

    /// Gate `:215` and its ASYNC twin, which upstream never spells out because `listAsyncRuns`
    /// applies the filter for it. Both must settle "could not be reconciled".
    #[tokio::test]
    async fn a_record_whose_run_reports_a_different_session_is_dropped() {
        // --- the async branch: `reconcile_by_id` applies NO session filter of its own ---
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        // The run exists, is terminal, and would satisfy the subscription — except that it belongs
        // to another session.
        fx.write_status(&run, RunState::Complete, Some(SESSION_B));
        fx.write_result(&run, Some(SESSION_B));
        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Async,
            "run-1",
            crate::time::now_epoch_millis() + 10 * HOUR_MS,
        );
        fx.write_record(&record);
        let manager = fx.manager();
        manager.restore().await;
        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::CouldNotBeReconciled);
        assert_eq!(fired.detail, DETAIL_ASYNC_GONE);

        // --- the foreground branch: pi `:215` verbatim ---
        let fx = Fixture::new();
        let run = RunId::from_token("run-fg");
        fx.probe.set(
            &run,
            ForegroundTargetState {
                session_id: Some(session(SESSION_B)),
                children: vec![child(ForegroundChildState::Detached)],
            },
        );
        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Foreground,
            "run-fg",
            crate::time::now_epoch_millis() + 10 * HOUR_MS,
        );
        fx.write_record(&record);
        let manager = fx.manager();
        manager.restore().await;
        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::CouldNotBeReconciled);
        assert_eq!(fired.detail, DETAIL_FOREGROUND_GONE);
    }

    /// Gate `:206` — the OPPOSITE comparison to the foreign sweep's `:157`.
    ///
    /// Reached the only way it can be: a record loaded under session B, then the session switches
    /// to A underneath the live manager. The record is expired, so without `:206` it would settle
    /// "timed out" into the wrong session — and note the file is NOT swept either (it is inside
    /// the grace window), which is exactly why the foreign sweep has to exist at all.
    #[tokio::test]
    async fn delivery_requires_the_current_session() {
        let fx = Fixture::new();
        fx.sessions.set(Some(SESSION_B));
        let record = fx.record(
            TOKEN_B,
            SESSION_B,
            WaitTargetKind::Foreground,
            "run-fg",
            // Expired an hour ago: well inside FOREIGN_SWEEP_GRACE_MS.
            crate::time::now_epoch_millis() - HOUR_MS,
        );
        let path = fx.write_record(&record);

        let manager = fx.manager();
        fx.notifier
            .accept
            .store(false, std::sync::atomic::Ordering::SeqCst);
        manager.restore().await;
        assert_eq!(manager.armed().len(), 1, "B's own record loaded under B");
        // Its own session DOES settle it (the control): `:206` is not "nothing ever fires".
        assert_eq!(fx.notifier.only().outcome, SubscriptionOutcome::TimedOut);

        // The session switches. The record is still in the map.
        fx.sessions.set(Some(SESSION_A));
        fx.notifier
            .captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        manager.reconcile().await;
        assert!(
            fx.notifier.captured().is_empty(),
            "`:206` returns BEFORE the timeout branch for a foreign record"
        );
        assert!(exists(&path), "and nothing unlinked it either");
    }

    /// §1 — `:157` is the FOREIGN SWEEP, not general enumeration. The sweep deletes only OTHER
    /// sessions' expired records, and the current session keeps its own so `reconcileRecord` can
    /// still settle them with the "timed out" notice callers expect.
    ///
    /// The notifier REFUSES the wake here, which is what keeps A's record on disk through its own
    /// (correct) timeout settle — so the surviving file proves the sweep skipped it rather than
    /// proving the timeout never ran. The captured timeout is the other half of that proof.
    #[tokio::test]
    async fn enumeration_skips_the_current_session() {
        let fx = Fixture::new();
        let long_ago = crate::time::now_epoch_millis() - FOREIGN_SWEEP_GRACE_MS - HOUR_MS;
        let mine = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Foreground,
            "run-a",
            long_ago,
        );
        let theirs = fx.record(
            TOKEN_B,
            SESSION_B,
            WaitTargetKind::Foreground,
            "run-b",
            long_ago,
        );
        let mine_path = fx.write_record(&mine);
        let theirs_path = fx.write_record(&theirs);

        fx.notifier
            .accept
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let manager = fx.manager();
        // `restore` force-sweeps (`:314`).
        manager.restore().await;

        assert!(
            !exists(&theirs_path),
            "another session's long-expired record is swept"
        );
        assert!(
            exists(&mine_path),
            "the owning session keeps its own expired record (pi `:155-157`)"
        );
        assert_eq!(
            fx.notifier.only().outcome,
            SubscriptionOutcome::TimedOut,
            "and it is still expired — the survival above is the sweep's doing, not luck"
        );
    }

    /// `:158` — the grace is what makes `:157` non-vacuous for a session that simply has not
    /// resumed yet. A foreign record one hour past expiry stays; the same record a day and an hour
    /// past expiry goes.
    #[tokio::test]
    async fn a_foreign_record_within_the_grace_window_is_not_swept() {
        let fx = Fixture::new();
        let now = crate::time::now_epoch_millis();
        let fresh = fx.record(
            TOKEN_A,
            SESSION_B,
            WaitTargetKind::Foreground,
            "run-b",
            now - HOUR_MS,
        );
        let stale = fx.record(
            TOKEN_B,
            SESSION_B,
            WaitTargetKind::Foreground,
            "run-b",
            now - FOREIGN_SWEEP_GRACE_MS - HOUR_MS,
        );
        let fresh_path = fx.write_record(&fresh);
        let stale_path = fx.write_record(&stale);

        let manager = fx.manager();
        manager.restore().await;

        assert!(
            exists(&fresh_path),
            "inside the grace: the owner may still resume and want its timeout notice"
        );
        assert!(!exists(&stale_path), "past the grace: swept");
        assert!(
            fx.notifier.captured().is_empty(),
            "a foreign record is never settled by this session, expired or not"
        );
    }

    /// `:154`/`:326` — a record may only be addressed by the name it claims. The cheapest possible
    /// defence against a record being moved or copied under another token's name.
    #[tokio::test]
    async fn a_subscription_file_whose_name_disagrees_with_its_token_is_ignored() {
        let fx = Fixture::new();
        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Foreground,
            "run-fg",
            crate::time::now_epoch_millis() + 10 * HOUR_MS,
        );
        // Written under a DIFFERENT (but perfectly valid) token's name.
        let misfiled = fx.write_record_as(&format!("{TOKEN_B}.json"), &record);

        let manager = fx.manager();
        manager.restore().await;

        assert!(manager.armed().is_empty(), "restore skips it (`:326`)");
        assert!(fx.notifier.captured().is_empty());
        assert!(
            exists(&misfiled),
            "and the sweep `continue`s past it too (`:154`) rather than unlinking a file it \
             could not identify"
        );
    }

    // =============================================================================================
    // the foreground branch
    // =============================================================================================

    /// §6.4 — `unresolvedRestoredForegroundTokens` (`:110`, `:213-214`, `:329`). Without this
    /// one-shot grace, every foreground subscription restored before the foreground registry is
    /// repopulated settles "could not be reconciled" on the first pass.
    ///
    /// The second half is the control that makes the grace non-vacuous: an ARMED (not restored)
    /// foreground record whose run is absent settles immediately, because the grace is
    /// restore-only.
    #[tokio::test]
    async fn a_restored_foreground_record_whose_run_is_absent_is_not_settled_on_the_first_pass() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-fg");
        let record = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Foreground,
            "run-fg",
            crate::time::now_epoch_millis() + 10 * HOUR_MS,
        );
        let path = fx.write_record(&record);

        // The registry is still empty — exactly the window between `restore()` and
        // `restore_foreground_run_history_for`.
        let manager = fx.manager();
        manager.restore().await;
        assert!(
            fx.notifier.captured().is_empty(),
            "the restored-foreground grace holds the settle off"
        );
        assert_eq!(manager.armed().len(), 1);
        assert!(exists(&path));

        // A second pass while the run is STILL missing keeps holding.
        manager.reconcile().await;
        assert!(fx.notifier.captured().is_empty());

        // The registry repopulates. The grace is spent and the run settles normally.
        fx.probe.set(
            &run,
            ForegroundTargetState {
                session_id: Some(session(SESSION_A)),
                children: vec![child(ForegroundChildState::Settled)],
            },
        );
        manager.reconcile().await;
        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::Completed);
        assert!(!exists(&path));

        // --- the control: an ARMED record gets no grace ---
        let fx = Fixture::new();
        let manager = fx.manager();
        manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Foreground,
                run_id: RunId::from_token("run-fg"),
                requested_id: "run-fg".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");
        manager.reconcile().await;
        assert_eq!(
            fx.notifier.only().outcome,
            SubscriptionOutcome::CouldNotBeReconciled,
            "the grace is restore-only (`:329`), not a general 'wait for the run to appear'"
        );
    }

    /// pi `:219-227` — the three foreground arms, each reached for the right reason.
    #[tokio::test]
    async fn the_foreground_branch_distinguishes_attention_failure_and_completion() {
        // A detached child with a pending supervisor request (`:220`).
        let fx = Fixture::new();
        let run = RunId::from_token("run-fg");
        fx.probe.set(
            &run,
            ForegroundTargetState {
                session_id: Some(session(SESSION_A)),
                children: vec![ForegroundTargetChild {
                    state: ForegroundChildState::Detached,
                    activity_state: Some(ActivityState::NeedsAttention),
                    current_tool: Some("contact_supervisor".to_string()),
                }],
            },
        );
        let manager = fx.manager();
        manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Foreground,
                run_id: run.clone(),
                requested_id: "run-fg".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");
        manager.reconcile().await;
        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::NeedsAttention);
        assert_eq!(fired.detail, DETAIL_FOREGROUND_ATTENTION);

        // A detached child WITHOUT a supervisor request keeps the subscription armed (`:228`).
        let fx = Fixture::new();
        fx.probe.set(
            &run,
            ForegroundTargetState {
                session_id: Some(session(SESSION_A)),
                children: vec![ForegroundTargetChild {
                    state: ForegroundChildState::Detached,
                    activity_state: Some(ActivityState::NeedsAttention),
                    // Attention, but not a supervisor request: pi's `:220` needs BOTH terms.
                    current_tool: Some("bash".to_string()),
                }],
            },
        );
        let manager = fx.manager();
        manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Foreground,
                run_id: run.clone(),
                requested_id: "run-fg".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");
        manager.reconcile().await;
        assert!(fx.notifier.captured().is_empty());

        // Every child landed, one of them failed (`:225-226`).
        fx.probe.set(
            &run,
            ForegroundTargetState {
                session_id: Some(session(SESSION_A)),
                children: vec![
                    child(ForegroundChildState::Settled),
                    child(ForegroundChildState::Failed),
                ],
            },
        );
        manager.reconcile().await;
        let fired = fx.notifier.only();
        assert_eq!(fired.outcome, SubscriptionOutcome::Failed);
        assert_eq!(fired.detail, DETAIL_INSPECT_STATUS);
    }

    // =============================================================================================
    // settle's own contract
    // =============================================================================================

    /// The cyrup ordering decision: the wake is AWAITED and the record cleared only on acceptance.
    /// A refused injection leaves the subscription armed so the next reconcile re-delivers —
    /// where upstream, whose send cannot fail, simply logs the loss.
    #[tokio::test]
    async fn a_wake_the_session_refuses_leaves_the_subscription_armed() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        fx.write_status(&run, RunState::Complete, Some(SESSION_A));
        fx.notifier
            .accept
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let manager = fx.manager();
        let record = manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Async,
                run_id: run.clone(),
                requested_id: "run".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");

        manager.reconcile().await;
        assert_eq!(fx.notifier.captured().len(), 1);
        assert!(
            exists(&fx.record_path(&record.token)),
            "unaccepted: the record must NOT be destroyed"
        );
        assert_eq!(manager.armed().len(), 1);

        // The session comes back. The same wake is re-delivered and now lands.
        fx.notifier
            .accept
            .store(true, std::sync::atomic::Ordering::SeqCst);
        manager.reconcile().await;
        assert_eq!(fx.notifier.captured().len(), 2);
        assert!(!exists(&fx.record_path(&record.token)));
        assert!(manager.armed().is_empty());
    }

    /// `dispose` refuses every later settle (`:181`'s `disposed` term) and stops the map.
    #[tokio::test]
    async fn a_disposed_manager_settles_nothing() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1");
        fx.write_status(&run, RunState::Complete, Some(SESSION_A));
        let manager = fx.manager();
        manager
            .arm(ArmWaitSubscriptionInput {
                target_kind: WaitTargetKind::Async,
                run_id: run,
                requested_id: "run".to_string(),
                timeout_ms: 10 * HOUR_MS as u64,
            })
            .await
            .expect("armed");
        manager.dispose();
        manager.reconcile().await;
        assert!(fx.notifier.captured().is_empty());
        assert!(manager.armed().is_empty());
        // Idempotent (`:339`).
        manager.dispose();
    }

    // =============================================================================================
    // the render's other end
    // =============================================================================================

    /// §9 — `format_wait_subscriptions` does no filtering because `restore`'s `:327` gate already
    /// did it. That is a property of `restore`, and a regression there is invisible at the
    /// formatter, so both ends are asserted: this is the `restore` end.
    #[tokio::test]
    async fn format_renders_only_this_sessions_subscriptions() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-fg");
        // Live and detached, so neither record settles during the restore's reconcile.
        fx.probe.set(
            &run,
            ForegroundTargetState {
                session_id: Some(session(SESSION_A)),
                children: vec![child(ForegroundChildState::Detached)],
            },
        );
        let now = crate::time::now_epoch_millis();
        let mine = fx.record(
            TOKEN_A,
            SESSION_A,
            WaitTargetKind::Foreground,
            "run-fg",
            now + HOUR_MS,
        );
        let theirs = fx.record(
            TOKEN_B,
            SESSION_B,
            WaitTargetKind::Foreground,
            "run-fg",
            now + HOUR_MS,
        );
        fx.write_record(&mine);
        let theirs_path = fx.write_record(&theirs);

        let manager = fx.manager();
        manager.restore().await;

        let armed = manager.armed();
        assert_eq!(armed.len(), 1, "`:327` admits only this session's records");
        assert_eq!(armed[0].token, mine.token);
        let rendered = format_wait_subscriptions(&armed, now).expect("one armed subscription");
        assert!(
            rendered.starts_with("Armed wait subscriptions (1):"),
            "{rendered}"
        );
        assert!(rendered.contains(TOKEN_A), "{rendered}");
        assert!(!rendered.contains(TOKEN_B), "{rendered}");
        assert!(
            exists(&theirs_path),
            "and B's record is left alone, not swept — it has not expired"
        );
    }

    /// The format constant is `1` and nothing else parses, which is what makes a record written by
    /// a future build a SKIP rather than a half-read.
    #[test]
    fn the_subscription_version_is_one() {
        assert_eq!(SUBSCRIPTION_VERSION, 1);
        assert_eq!(FOREIGN_SWEEP_GRACE_MS, 24 * 60 * 60 * 1000);
        assert_eq!(FOREIGN_SWEEP_INTERVAL_MS, 60_000);
        assert_eq!(RECONCILE_INTERVAL, std::time::Duration::from_secs(1));
    }
}
