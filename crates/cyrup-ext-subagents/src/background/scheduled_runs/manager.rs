//! SUBA-016 part B — the durable half's driver: one store, one tick task, one lifecycle.
//!
//! Ports pi `ScheduledRunManager` (`runs/background/scheduled-runs.ts:519-1001` @ `v0.68.0`),
//! minus the per-schedule timer map (see [`super::trigger`]'s `[CYRUP-DELTA]`).
//!
//! # Why the timer lives here and not in `spawn_retention_sweep`
//!
//! `extension/executor/notices.rs`'s `spawn_retention_sweep` is a ONE-SHOT: its own doc header is
//! *"Why delayed and detached, rather than at install or **on an interval**"*, and it explains
//! that a session ending inside the delay simply never sweeps. That is an acceptable degradation
//! for a retention sweep and an unacceptable one for a schedule — "your nightly run did not
//! happen because the session was young" is not a behaviour anyone asked for.
//!
//! The shape this DOES take is
//! [`crate::background::wait_subscriptions::WaitSubscriptionManager`]'s reconcile timer, which is
//! the in-tree convention for *"a durable store with a periodic reconciler, armed on
//! `SessionStart` and disposed on `SessionShutdown`"* — and scheduled runs are literally that.
//! Copied exactly, including:
//!
//! * the timer armed from [`ScheduledRunManager::restore`] rather than `new`, because
//!   `tokio::spawn` needs a runtime and the executor is constructed outside one;
//! * the `is_finished()` idempotence, so a second `restore` reuses the task;
//! * the [`Weak`] back-reference, because a strong handle inside the task's own closure is a
//!   reference cycle: `Drop` would never run and the task would outlive its manager.
//!
//! # Dispose does not touch the store, and does not release `active.lock`
//!
//! The RECORDS stay on disk — that is the entire point of a cwd-keyed store, and the next
//! session's `restore()` is what picks them up. The in-flight `active.lock` stays too: releasing
//! it at dispose would let the next session double-launch a schedule whose run is still going.
//! [`super::trigger::restore_one`]'s stale-claim recovery is what clears it, on the next start.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::ceiling_gate::process_ceiling_resolver;
use super::store::ScheduleStore;
use super::tool::{
    ScheduledRunAction, ScheduledRunActionContext, ScheduledRunActionOutcome,
    ScheduledRunActionParams, ScheduledRunError, handle_scheduled_run_action,
};
use super::trigger::{SCHEDULE_TICK, ScheduleFireContext};

/// The manager's home on the executor — one slot, shared by every reader.
///
/// `Arc<Mutex<Option<…>>>` rather than a bare `Option` for the reason
/// [`crate::extension::executor::wait_subscriptions::WaitSubscriptionSlot`] gives: the tool arm
/// must reach whichever manager is current at DISPATCH time, not whichever one existed when the
/// tool was built, and the two are re-created on independent `SessionStart` edges.
pub type ScheduledRunSlot = Arc<Mutex<Option<Arc<ScheduledRunManager>>>>;

/// Everything the manager is built from.
pub struct ScheduledRunDeps {
    /// The project's store, already rooted (`scheduled_run_store_path` + the configured
    /// `storeRoot`).
    pub store: ScheduleStore,
    /// §SUBTASK2's pinned identity plus §5's launch seam.
    pub fire: ScheduleFireContext,
    /// The session's own cwd.
    pub cwd: std::path::PathBuf,
    /// pi `resolveMaxPending(config)` (`:387-390`), already resolved.
    pub max_pending: u32,
}

/// pi `ScheduledRunManager` (`scheduled-runs.ts:519`).
///
/// Held behind an [`Arc`] because the tick is a spawned `tokio` task holding a [`Weak`] back to it.
pub struct ScheduledRunManager {
    deps: ScheduledRunDeps,
    disposed: Mutex<bool>,
    /// The tick task's handle. See the module doc for why `.unref()` needs no analog.
    tick_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ScheduledRunManager {
    /// Build the manager, WITHOUT arming the tick — see the module doc.
    #[must_use]
    pub fn new(deps: ScheduledRunDeps) -> Arc<Self> {
        Arc::new(Self {
            deps,
            disposed: Mutex::new(false),
            tick_task: Mutex::new(None),
        })
    }

    /// The project's store, for the tool surface.
    #[must_use]
    pub fn store(&self) -> &ScheduleStore {
        &self.deps.store
    }

    /// The pinned identity and the launch seam, for the tool surface.
    #[must_use]
    pub fn fire(&self) -> &ScheduleFireContext {
        &self.deps.fire
    }

    /// pi's `disposed` flag.
    #[must_use]
    pub fn is_disposed(&self) -> bool {
        *self.disposed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Dispatch one `schedule.*` action against this manager's store and pinned session.
    ///
    /// This is the ONE place the action context is assembled, so the ceiling resolver, the
    /// `maxPending` value and the clock cannot differ between the timer's own `run-due` and a
    /// user-invoked one.
    ///
    /// # Errors
    ///
    /// As [`handle_scheduled_run_action`].
    pub async fn handle_action(
        &self,
        action: ScheduledRunAction,
        params: &ScheduledRunActionParams,
    ) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
        handle_scheduled_run_action(
            action,
            params,
            &ScheduledRunActionContext {
                store: &self.deps.store,
                fire: &self.deps.fire,
                cwd: &self.deps.cwd,
                max_pending: self.deps.max_pending,
                ceiling: &process_ceiling_resolver,
                now: crate::time::now_epoch_millis(),
            },
        )
        .await
    }

    /// pi `restore(store)` (`scheduled-runs.ts:743-745`) plus `arm` — the `SessionStart` edge.
    ///
    /// Runs [`super::trigger::restore_one`] over the whole store (crash recovery) and then arms
    /// the tick. Both halves matter and neither substitutes for the other: the tick decides
    /// *when*, `restore_one` decides *what state a crash left each schedule in*.
    pub async fn restore(self: &Arc<Self>) {
        super::trigger::restore(
            &self.deps.store,
            &self.deps.fire,
            crate::time::now_epoch_millis(),
        )
        .await;
        self.ensure_tick_timer();
    }

    /// One pass of the tick. Separated from the task so a test can drive it without a timer.
    async fn tick(&self) {
        match super::trigger::tick_due_schedules(
            &self.deps.store,
            &self.deps.fire,
            crate::time::now_epoch_millis(),
        )
        .await
        {
            Ok(runs) if !runs.is_empty() => {
                tracing::debug!(fired = runs.len(), "scheduled-run tick fired due schedules");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(
                %error,
                "scheduled-run tick failed; the next tick retries and the records are untouched"
            ),
        }
    }

    /// pi's `setTimeout` map, as ONE task — see [`super::trigger`]'s `[CYRUP-DELTA]`.
    fn ensure_tick_timer(self: &Arc<Self>) {
        let mut slot = self
            .tick_task
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        // WEAK, not strong: a strong handle inside the task would be a reference cycle through
        // the task's own closure, so `Drop` would never run and the task would outlive its
        // manager.
        let manager = Arc::downgrade(self);
        *slot = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(SCHEDULE_TICK).await;
                let Some(manager) = Weak::upgrade(&manager) else {
                    return;
                };
                if manager.is_disposed() {
                    return;
                }
                manager.tick().await;
            }
        }));
    }

    /// pi `stop()` (`scheduled-runs.ts:539-545`), on `SessionShutdown`.
    ///
    /// Stops the tick and refuses every later one. See the module doc for what it deliberately
    /// does NOT do.
    pub fn dispose(&self) {
        {
            let mut disposed = self.disposed.lock().unwrap_or_else(PoisonError::into_inner);
            if *disposed {
                return;
            }
            *disposed = true;
        }
        if let Some(task) = self
            .tick_task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl ScheduledRunManager {
    /// pi `referencedAsyncRunIds` (`scheduled-runs.ts:573-583`) — every async run id this
    /// project's schedules still point at.
    ///
    /// Its ONE consumer is the async-retention sweep's `protected_run_ids`
    /// (`extension/index.ts:612` upstream), and the coupling is not cosmetic: `schedule.delete`'s
    /// active-run guard opens `<async_dir>/status.json` and `schedule.history` renders each run's
    /// `async` id, so a reaped run directory would leave the guard unable to confirm the run ended
    /// — and a schedule with a live claim it can never clear is a schedule that can never be
    /// deleted.
    ///
    /// Read at SWEEP time, never snapshotted at install, for the reason
    /// [`crate::extension::executor::notices`]'s own doc gives for the other two sources: a set
    /// captured a minute ago protects the runs that were live then and leaves every run fired
    /// since unprotected, which is exactly the reap this must never perform.
    ///
    /// An unreadable schedule contributes nothing rather than failing the sweep — the pass's
    /// fail-closed guard is the WAIT-subscription set, which is fail-closed because an unreadable
    /// subscription might protect any run in the root; a schedule's references are bounded to its
    /// own history and an unreadable one is already being reported by every other surface.
    pub async fn referenced_async_run_ids(
        &self,
    ) -> std::collections::BTreeSet<crate::background::RunId> {
        let mut ids = std::collections::BTreeSet::new();
        let (records, diagnostics) = self.deps.store.list().await;
        for diagnostic in diagnostics {
            tracing::warn!(
                %diagnostic,
                "skipping an unreadable schedule while collecting retention-protected run ids"
            );
        }
        for record in records {
            match self.deps.store.history(&record.id).await {
                Ok(history) => {
                    for run in history {
                        if let Some(async_id) = run.async_id {
                            ids.insert(crate::background::RunId::from_token(async_id));
                        }
                    }
                }
                Err(error) => tracing::warn!(
                    schedule_id = %record.id,
                    %error,
                    "could not read a schedule's history while collecting retention-protected run ids"
                ),
            }
        }
        ids
    }
}
