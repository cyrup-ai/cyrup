//! SUBA-016 part B — the trigger: when a schedule is due, and the launch/skip/miss/finish state
//! machine it drives.
//!
//! Ports pi `runs/background/scheduled-runs.ts`'s `nextAfter`/`nextRunAt`/`duePlannedAt`
//! (`:396-415`), `runDue` (`:712-723`), `launch` (`:834-900`), `finishRun` (`:902-929`),
//! `recordMissed` (`:931-946`), `restore`/`restoreOne` (`:743-786`) and `snapshotContext`
//! (`:466-479`), all @ `v0.68.0`.
//!
//! # `[CYRUP-DELTA]` — one tick for the store, not one `setTimeout` per schedule
//!
//! Upstream arms a `setTimeout` per schedule (`arm`, `:788-804`) and re-arms it on every create,
//! resume, restore, fire-skip, launch outcome and finish. Reproducing that literally in Rust is
//! one `tokio::time::sleep_until` task per schedule plus a `HashMap<key, JoinHandle>`, which
//! multiplies tasks by schedules and gives every `arm`/`clear_timer` pair a cancellation race.
//!
//! cyrup drives ONE tick for the whole store, on the in-tree precedent
//! ([`crate::background::wait_subscriptions::WaitSubscriptionManager`]'s reconcile timer). The
//! externally observable contract is identical to within one tick: a schedule whose `nextRunAt`
//! is `<= now` fires, one whose `nextRunAt` is `> now` does not. `arm`, `clear_timer`, `fire`,
//! `timerKey` and `stopTimers` therefore have no analog here — but [`restore_one`] DOES, because
//! the tick decides *when* while `restore_one` decides *what state a crash left a schedule in*.
//!
//! Upstream's `MAX_TIMER_DELAY_MS = 2_147_483_647` (`:31`) is **deliberately dropped**: it is the
//! i32 ceiling on a JS `setTimeout` delay, and [`tokio::time::sleep`] takes a [`std::time::Duration`]
//! with no such ceiling. Carrying it would silently cap a legitimate 30-day schedule at 24.8 days.
//!
//! # Which session a fired schedule belongs to
//!
//! **The session that is LIVE when it fires, not the one that created it.** Nothing upstream reads
//! a creating session off the record — [`ScheduleRecord`] has no `createdBySessionId` field, and
//! `ownerSessionFile` (`:58`) is consulted only when `sessionOnly === true` (`:501`), where it is
//! an execution GATE rather than an attribution.
//!
//! The consequence is the reason this is written down rather than left to inference: the run's
//! `status.session_id` and its completion owner are **this process, this session**, because
//! attributing a fresh run to a dead session strands its result behind
//! [`crate::background::delivery::OwnershipSnapshot::readable_sessions`], which enumerates the
//! current session plus claimed predecessors and nothing else.
//!
//! And the counterweight: a `sessionOnly` schedule does **not** fire in a later session at all —
//! [`tick_due_schedules`] filters it out (`:715`) and [`restore_one`] returns early (`:748`). So
//! "a schedule created in one session fires in a later one" is true only for `sessionOnly != true`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::schedule::{
    ScheduleCatchUp, ScheduleDueReason, ScheduleId, ScheduleRecord, ScheduleRunId,
    ScheduleRunRecord, ScheduleRunState, ScheduleTrigger, ScheduleVersion, schedule_timestamp,
};
use super::store::{ScheduleStore, ScheduleStoreError};
use crate::identity::SessionId;

/// How often the trigger task wakes to look for due schedules.
///
/// **Chosen against the smallest schedulable interval, deliberately.**
/// [`super::schedule::parse_schedule_interval`] accepts `m|h|d|w` with a minimum of `1m`
/// (60 000 ms). A 30 s tick keeps worst-case lateness at HALF the smallest legal interval, so a
/// `every: "1m"` schedule fires every 60-90 s and never skips a slot. A 60 s tick would halve the
/// wakeups but make worst-case lateness equal the whole smallest interval, i.e. a one-minute
/// schedule drifting to a two-minute cadence — which is the one interval a user picks precisely
/// because they care about the cadence. The wakeup itself is one `read_dir` plus at most
/// `maxPending` (20) small JSON reads, so halving it buys nothing measurable.
///
/// A `pub const` rather than a parameter of [`tick_due_schedules`] (which takes `now`, so every
/// test drives it directly and none of them sleeps): the tick task reads it once.
pub const SCHEDULE_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// pi `STALE_LAUNCH_CLAIM_MS = 5 * 60_000` (`scheduled-runs.ts:34`) — how long a launch claim with
/// no attached async run may sit before [`restore_one`] recovers it.
pub const STALE_LAUNCH_CLAIM_MS: i64 = 5 * 60_000;

/// pi `DEFAULT_MAX_PENDING = 20` (`scheduled-runs.ts:32`).
pub const DEFAULT_MAX_PENDING: u32 = 20;

/// pi's stale-claim recovery message (`scheduled-runs.ts:770`), verbatim.
pub const STALE_LAUNCH_CLAIM_ERROR: &str =
    "Recovered a stale launch claim before an async run was attached.";

// =================================================================================================
// SUBTASK2 — the pinned session identity
// =================================================================================================

/// pi `snapshotContext` (`scheduled-runs.ts:466-479`) — the PROPERTY, not the `Proxy`.
///
/// Both values upstream pins are pinned: the session id (`:468`/`:472`) and the session FILE
/// (`:469`/`:473`). Pinning only the id is the likely partial port and it is wrong, because the
/// FILE is what [`schedule_belongs_to_session`] reads (`:503`) — an id-only pin would leave the
/// `sessionOnly` gate reading live state, which is the precise bug the pin exists to prevent.
///
/// Re-taken on each `SessionStart` edge exactly as `selectProject` (`:948-967`) re-snapshots on
/// each binding, and therefore constant for the whole of any one fire. That is what keeps a fired
/// run's `status.session_id`, its completion owner and the `sessionOnly` gate from observing three
/// different identities across one launch:
/// [`crate::extension::executor::SubagentExecutor::current_session_id`] is documented to read
/// "straight off the bound P-1 backend on every call", and a session SWITCH inside one process
/// moves the live id.
///
/// The in-tree precedent is [`crate::background::delivery::OwnershipSnapshot`], taken once before
/// a drain loop for exactly this reason.
///
/// ⚠ The invariant a reviewer checks by grep: **zero** `current_session_id` calls under
/// `background/scheduled_runs/`.
/// The all-`None` [`Default`] is the HEADLESS case — no session, no file — which
/// [`SubagentExecutor::install_scheduled_runs`](crate::extension::executor::SubagentExecutor::install_scheduled_runs)
/// produces for a host with no bound session. A `sessionOnly` schedule then belongs to nobody and
/// fires for nobody, which is the correct degradation: a gate that cannot identify the owner must
/// refuse, not admit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduleSessionSnapshot {
    session_id: Option<SessionId>,
    session_file: Option<PathBuf>,
}

impl ScheduleSessionSnapshot {
    /// Capture both pinned values. The file is normalized here, once, per
    /// `normalizedSessionFile` (`:487-491`).
    #[must_use]
    pub fn new(session_id: Option<SessionId>, session_file: Option<&Path>) -> Self {
        Self {
            session_id,
            session_file: normalized_session_file(session_file),
        }
    }

    /// The pinned session id.
    #[must_use]
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// The pinned, normalized session file.
    #[must_use]
    pub fn session_file(&self) -> Option<&Path> {
        self.session_file.as_deref()
    }
}

/// pi `normalizedSessionFile` (`scheduled-runs.ts:487-491`).
///
/// `path::absolute` (never `canonicalize`: the file may not exist yet, and upstream's
/// `path.resolve` does not touch the filesystem either), then — **on Windows only** —
/// lowercased. The `cfg!(windows)` guard is load-bearing: lowercasing a POSIX path makes two
/// distinct sessions compare equal.
#[must_use]
pub fn normalized_session_file(value: Option<&Path>) -> Option<PathBuf> {
    let value = value?;
    if value.as_os_str().is_empty() {
        return None;
    }
    let absolute = std::path::absolute(value).unwrap_or_else(|_| value.to_path_buf());
    if cfg!(windows) {
        Some(PathBuf::from(absolute.to_string_lossy().to_lowercase()))
    } else {
        Some(absolute)
    }
}

/// pi `scheduleBelongsToSession` (`scheduled-runs.ts:500-505`).
///
/// A non-`sessionOnly` schedule belongs to everyone; a `sessionOnly` one belongs only to the
/// session whose FILE matches the one recorded at create time.
#[must_use]
pub fn schedule_belongs_to_session(
    schedule: &ScheduleRecord,
    session: &ScheduleSessionSnapshot,
) -> bool {
    if schedule.session_only != Some(true) {
        return true;
    }
    let owner = normalized_session_file(schedule.owner_session_file.as_deref());
    owner.is_some() && owner.as_deref() == session.session_file()
}

// =================================================================================================
// The launch seam
// =================================================================================================

/// The future a launcher hands back so the trigger can settle the run when it ends.
///
/// `[CYRUP-DELTA]`: upstream settles through `handleAsyncCompletion` (`:585-610`), fed by the
/// extension's async-completion event. cyrup's scheduled run is driven by a task in THIS process
/// (see [`crate::extension::executor::workflow_launch`] for why), so the completion edge is that
/// task's own result — strictly more direct, and it cannot be missed by a subscriber that was not
/// installed yet.
pub type ScheduleRunCompletion =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

/// What one fire hands the launcher.
pub struct ScheduleLaunchRequest<'a> {
    /// The schedule being fired, as it stands AFTER the claim.
    pub schedule: &'a ScheduleRecord,
    /// The [`ScheduleRunRecord`] id this launch belongs to — NOT the run id (§R15-2).
    pub run_id: &'a ScheduleRunId,
    /// The effective quiet flag for this fire: the schedule's own for a timer/run-due fire, the
    /// caller's for a manual one (`:877`).
    pub quiet: bool,
    /// The identity pinned for this binding (§SUBTASK2).
    pub session: &'a ScheduleSessionSnapshot,
}

/// What the launcher hands back — pi's `result.details.asyncId ?? result.details.runId` and
/// `result.details.asyncDir` (`:879-880`).
pub struct ScheduleLaunchOutcome {
    /// The spawned run's id.
    pub async_id: String,
    /// Its run directory.
    pub async_dir: PathBuf,
    /// The settle edge, when the launcher has one.
    pub completion: Option<ScheduleRunCompletion>,
}

/// The two seams a fire needs from outside `background/`: the per-session spawn budget and the
/// run launch itself.
///
/// A trait rather than two closures so a test stub is one type with two obviously-paired methods —
/// and so that the budget gate cannot drift to the wrong side of the overlap lock, which is the
/// one ordering §5 is about.
#[async_trait::async_trait]
pub trait ScheduleLauncher: Send + Sync {
    /// §5 — charge ONE spawn against the live session's budget
    /// ([`crate::extension::executor::SubagentExecutor::reserve_subagent_spawns`]), consulted
    /// BEFORE the `active.lock` so a refusal costs no lock.
    ///
    /// **This is not politeness — without it the schedule path is unbilled.** `Tool::execute`
    /// dispatches `action` and returns ABOVE the spawn-budget charge every other execution mode
    /// pays, so a `schedule.run`/`schedule.run-due` that launched a run through `route_action`
    /// would spend a child the session never paid for. `spawn_budget.rs`'s "EVERY route into
    /// execution charges here" is true only because a fired schedule charges here too.
    ///
    /// # Errors
    ///
    /// `reserve_subagent_spawns`' own over-limit sentence, which becomes the run record's `error`.
    fn reserve_spawn_slot(&self) -> Result<(), String>;

    /// Start the run. Returns as soon as it is addressable on disk; the run itself is still going.
    ///
    /// # Errors
    ///
    /// Anything that prevented the run from starting. The caller rolls the claim back (§1.4 step
    /// 7) and records `FailedLaunch`.
    async fn launch(
        &self,
        request: ScheduleLaunchRequest<'_>,
    ) -> Result<ScheduleLaunchOutcome, String>;
}

/// Everything a fire reads that is not the store or the clock.
///
/// Carries the pinned snapshot, so **nothing below this type reads a live session**.
#[derive(Clone)]
pub struct ScheduleFireContext {
    /// §SUBTASK2's pinned identity.
    pub session: ScheduleSessionSnapshot,
    /// §5's budget gate and the launch itself.
    pub launcher: Arc<dyn ScheduleLauncher>,
}

// =================================================================================================
// §1.1 — the trigger algebra
// =================================================================================================

/// pi `hasPendingScheduleWork` (`scheduled-runs.ts:392-394`) — the `maxPending` predicate.
#[must_use]
pub fn has_pending_schedule_work(schedule: &ScheduleRecord) -> bool {
    schedule.active_run_id.is_some() || schedule.trigger.next_run_at().is_some()
}

/// pi `nextRunAt` (`scheduled-runs.ts:403-409`).
///
/// # Errors
///
/// `Schedule '<id>' has invalid nextRunAt.` — an unparseable stamp is an ERROR, not "no next run".
/// Reading it as `None` would silently retire a live schedule.
pub fn next_run_at(schedule: &ScheduleRecord) -> Result<Option<i64>, String> {
    let Some(value) = schedule.trigger.next_run_at() else {
        return Ok(None);
    };
    super::schedule::parse_schedule_timestamp(value)
        .map(Some)
        .ok_or_else(|| format!("Schedule '{}' has invalid nextRunAt.", schedule.id))
}

/// pi `nextAfter` (`scheduled-runs.ts:396-401`).
///
/// `None` for a one-shot. For an interval, `planned + every_ms` advanced by WHOLE PERIODS while
/// the result is `<= now` — the `while`, not a single add: a process asleep for six intervals must
/// land in the future, not six fires behind.
#[must_use]
pub fn next_after(trigger: &ScheduleTrigger, planned_at: i64, now: i64) -> Option<String> {
    let ScheduleTrigger::Interval { every_ms, .. } = trigger else {
        return None;
    };
    let every_ms = *every_ms;
    if every_ms <= 0 {
        // Unreachable through `parse_schedule_interval` (which refuses `< 1`) and through
        // `parse_schedule` (which requires an integer), but a hand-edited record could reach it
        // and a zero step would spin this loop forever.
        return None;
    }
    let mut next = planned_at.saturating_add(every_ms);
    while next <= now {
        next = next.saturating_add(every_ms);
    }
    Some(schedule_timestamp(next))
}

/// pi `duePlannedAt` (`scheduled-runs.ts:411-415`) — the CATCH-UP rule.
///
/// Returns `next` unchanged unless all three of (`next <= now`, `catchUp == latest`,
/// `trigger.kind == interval`) hold, in which case it returns the MOST RECENT missed occurrence.
/// That is what makes `catchUp: "latest"` fire **once**, at the latest slot, rather than replaying
/// a backlog.
///
/// # Errors
///
/// As [`next_run_at`].
pub fn due_planned_at(schedule: &ScheduleRecord, now: i64) -> Result<Option<i64>, String> {
    let Some(next) = next_run_at(schedule)? else {
        return Ok(None);
    };
    let ScheduleTrigger::Interval { every_ms, .. } = &schedule.trigger else {
        return Ok(Some(next));
    };
    if next > now || schedule.catch_up != ScheduleCatchUp::Latest || *every_ms <= 0 {
        return Ok(Some(next));
    }
    let missed = (now - next) / *every_ms;
    Ok(Some(next.saturating_add(missed.saturating_mul(*every_ms))))
}

// =================================================================================================
// §1.2 — the tick
// =================================================================================================

/// ONE tick — pi `runDue`'s body (`scheduled-runs.ts:712-723`), with upstream's own predicate and
/// upstream's own branch.
///
/// Pure with respect to time: `now` is INJECTED and never read inside, so every behavioural test
/// drives this directly instead of sleeping.
///
/// The predicate is upstream's verbatim — belongs to this session, not paused, and `nextRunAt` is
/// due. The branch is upstream's too: a schedule with no active run, `catchUp: "none"` and a
/// planned slot strictly in the past is [`record_missed`] (`:719`); everything else is
/// [`launch`] (`:720`).
///
/// # Errors
///
/// Anything the store or the trigger algebra raises. Upstream lets these escape `runDue` to
/// `handleToolCall`'s outer `catch`, which renders them as an error-flagged result (`:558-560`);
/// the `schedule.run-due` arm does the same, and the timer task logs and keeps ticking.
pub async fn tick_due_schedules(
    store: &ScheduleStore,
    ctx: &ScheduleFireContext,
    now: i64,
) -> Result<Vec<ScheduleRunRecord>, ScheduleStoreError> {
    let (schedules, diagnostics) = store.list().await;
    for diagnostic in diagnostics {
        tracing::warn!(%diagnostic, "skipping an unreadable schedule record during the due scan");
    }
    let mut runs = Vec::new();
    for mut schedule in schedules {
        if !schedule_belongs_to_session(&schedule, &ctx.session) || schedule.paused {
            continue;
        }
        let due = next_run_at(&schedule)
            .map_err(ScheduleStoreError::Refused)?
            .is_some_and(|next| next <= now);
        if !due {
            continue;
        }
        let Some(planned) = due_planned_at(&schedule, now).map_err(ScheduleStoreError::Refused)?
        else {
            continue;
        };
        if schedule.active_run_id.is_none()
            && schedule.catch_up == ScheduleCatchUp::None
            && planned < now
        {
            runs.push(
                record_missed(
                    store,
                    &mut schedule,
                    planned,
                    ScheduleDueReason::RunDue,
                    now,
                )
                .await?,
            );
        } else {
            runs.push(
                launch(
                    store,
                    ctx,
                    &mut schedule,
                    planned,
                    ScheduleDueReason::RunDue,
                    true,
                    None,
                    now,
                )
                .await?,
            );
        }
    }
    Ok(runs)
}

// =================================================================================================
// §1.4 — `launch`, the overlap lock
// =================================================================================================

/// pi `launch` (`scheduled-runs.ts:834-900`) — the correctness core, ported step for step.
///
/// 1. capture `next_run_at` BEFORE anything mutates the record;
/// 2. an already-active run is `Skipped` with NO lock taken;
/// 3. §5's spawn-budget charge, before the lock so a refusal costs no lock — and, like every
///    other outcome here, advancing `next_run_at` only when `advance`;
/// 4. `create_new(true)` + `0o600` on `active.lock` — the `O_EXCL` claim IS the cross-process
///    mutual exclusion that makes `overlap: "skip"` true when two cyrup instances share one cwd;
/// 5. on `EEXIST` **only**, the same `Skipped` path — any other I/O error propagates, because a
///    permissions fault must not read as "skipped";
/// 6. the claim: `active_run_id`, `last_run_id`, the advance, `write`, `schedule.run.started`;
/// 7. on launch failure: `FailedLaunch`, RE-READ the record (the in-memory copy is stale), clear
///    the claim, restore `next_run_at` when `!advance` (a manual run must not eat the next
///    scheduled slot), write, `schedule.run.failed`, and REMOVE the lock.
///
/// ⚠ The lock file is removed on exactly two paths: launch failure and [`finish_run`]. A
/// SUCCESSFUL launch leaves it held for the whole run, and a process that dies in between leaves
/// it on disk — which is what [`restore_one`]'s stale-claim recovery exists to clear. Do not
/// "fix" this with a `Drop` guard: a `Drop` releases the lock when the ORCHESTRATOR exits, while
/// the run it guards is still going.
///
/// # Errors
///
/// A store failure. A launcher failure is NOT an error here — it is a `FailedLaunch` record, which
/// is upstream's shape and is what lets `schedule.run` report it with `isError` rather than
/// throwing.
#[allow(clippy::too_many_arguments)]
pub async fn launch(
    store: &ScheduleStore,
    ctx: &ScheduleFireContext,
    schedule: &mut ScheduleRecord,
    planned: i64,
    due_reason: ScheduleDueReason,
    advance: bool,
    quiet_override: Option<bool>,
    now: i64,
) -> Result<ScheduleRunRecord, ScheduleStoreError> {
    // `:836` — captured before ANYTHING mutates the record.
    let next_run_at_before_claim = schedule.trigger.next_run_at().map(str::to_string);
    let mut run = ScheduleRunRecord {
        schema_version: ScheduleVersion,
        id: ScheduleRunId::mint(),
        schedule_id: schedule.id.clone(),
        planned_at: schedule_timestamp(planned),
        due_reason,
        state: ScheduleRunState::Running,
        started_at: Some(schedule_timestamp(now)),
        completed_at: None,
        async_id: None,
        async_dir: None,
        error: None,
    };

    // `:838-849` — an overlapping fire. No lock is taken on this path.
    if schedule.active_run_id.is_some() {
        return skip_overlap(store, schedule, run, planned, advance, now).await;
    }

    // §5 — the budget gate, deliberately ahead of the lock.
    if let Err(refusal) = ctx.launcher.reserve_spawn_slot() {
        run.state = ScheduleRunState::FailedLaunch;
        run.completed_at = Some(schedule_timestamp(now));
        run.error = Some(refusal);
        // ADVANCED even though the launch never happened: a schedule refused at the cap must not
        // hot-retry on every tick for the rest of the session.
        //
        // …but only when `advance`, exactly as every other outcome in this function. A MANUAL
        // fire (`schedule.run`, `advance = false`) did not consume the pending occurrence, and
        // eating it here contradicted step 7's own rollback two branches below. On a one-shot it
        // was worse than a skipped slot: `next_after` is `None` for `Once`, so a single manual
        // fire attempted at the spawn cap RETIRED the schedule outright and it could never run.
        if advance {
            schedule
                .trigger
                .set_next_run_at(next_after(&schedule.trigger, planned, now));
        }
        schedule.updated_at = schedule_timestamp(now);
        store.write(schedule).await?;
        store
            .write_run(schedule, &run, "schedule.run.failed")
            .await?;
        return Ok(run);
    }

    // `:850-869` — the `O_EXCL` claim.
    if !store.acquire_active_lock(&schedule.id, &run.id).await? {
        return skip_overlap(store, schedule, run, planned, advance, now).await;
    }

    // `:870-875` — the claim.
    schedule.active_run_id = Some(run.id.clone());
    schedule.last_run_id = Some(run.id.clone());
    if advance {
        schedule
            .trigger
            .set_next_run_at(next_after(&schedule.trigger, planned, now));
    }
    schedule.updated_at = schedule_timestamp(now);
    store.write(schedule).await?;
    store
        .write_run(schedule, &run, "schedule.run.started")
        .await?;

    // `:876-885` / `:886-899`.
    let quiet = quiet_override.unwrap_or_else(|| schedule.quiet == Some(true));
    let outcome = ctx
        .launcher
        .launch(ScheduleLaunchRequest {
            schedule,
            run_id: &run.id,
            quiet,
            session: &ctx.session,
        })
        .await;
    match outcome {
        Ok(outcome) => {
            run.async_id = Some(outcome.async_id);
            run.async_dir = Some(outcome.async_dir);
            store
                .write_run(schedule, &run, "schedule.run.attached_async")
                .await?;
            if let Some(completion) = outcome.completion {
                spawn_settle_task(
                    store.clone(),
                    schedule.id.clone(),
                    run.id.clone(),
                    completion,
                );
            }
            Ok(run)
        }
        Err(error) => {
            run.state = ScheduleRunState::FailedLaunch;
            run.completed_at = Some(schedule_timestamp(now));
            run.error = Some(error);
            // `:890` — the in-memory copy is stale; the store is authoritative.
            let mut latest = store.get(&schedule.id).await?;
            latest.active_run_id = None;
            if !advance && let Some(before) = next_run_at_before_claim {
                // `:892` — a manual run must not eat the next scheduled slot.
                latest.trigger.set_next_run_at(Some(before));
            }
            latest.updated_at = schedule_timestamp(now);
            store.write(&latest).await?;
            store
                .write_run(&latest, &run, "schedule.run.failed")
                .await?;
            store.release_active_lock(&latest.id).await?;
            *schedule = latest;
            Ok(run)
        }
    }
}

/// The shared `skipped` tail of `:838-849` and `:857-869`.
async fn skip_overlap(
    store: &ScheduleStore,
    schedule: &mut ScheduleRecord,
    mut run: ScheduleRunRecord,
    planned: i64,
    advance: bool,
    now: i64,
) -> Result<ScheduleRunRecord, ScheduleStoreError> {
    run.state = ScheduleRunState::Skipped;
    run.completed_at = Some(schedule_timestamp(now));
    if advance {
        schedule
            .trigger
            .set_next_run_at(next_after(&schedule.trigger, planned, now));
        schedule.updated_at = schedule_timestamp(now);
        store.write(schedule).await?;
    }
    store
        .write_run(schedule, &run, "schedule.skipped_overlap")
        .await?;
    Ok(run)
}

/// The detached half of the settle edge — see [`ScheduleRunCompletion`].
fn spawn_settle_task(
    store: ScheduleStore,
    schedule_id: ScheduleId,
    run_id: ScheduleRunId,
    completion: ScheduleRunCompletion,
) {
    tokio::spawn(async move {
        let outcome = completion.await;
        let success = outcome.is_ok();
        let error = outcome.err();
        let settle = async {
            let mut schedule = store.get(&schedule_id).await?;
            let Some(mut run) = store
                .history(&schedule_id)
                .await?
                .into_iter()
                .find(|item| item.id == run_id)
            else {
                return Ok::<(), ScheduleStoreError>(());
            };
            finish_run(
                &store,
                &mut schedule,
                &mut run,
                success,
                error,
                crate::time::now_epoch_millis(),
            )
            .await
        };
        if let Err(error) = settle.await {
            tracing::warn!(
                schedule_id = %schedule_id,
                %error,
                "a scheduled run finished but its schedule could not be settled; the next \
                 SessionStart's restore will recover the claim"
            );
        }
    });
}

// =================================================================================================
// §1.1 — `finish_run` and `record_missed`
// =================================================================================================

/// pi `finishRun` (`scheduled-runs.ts:902-929`).
///
/// Settles the run, back-fills a `Skipped` record for an occurrence that came due WHILE the run
/// was in flight (`:905-918`), clears `active_run_id` and removes `active.lock`.
///
/// # Errors
///
/// A store failure, or the trigger algebra's invalid-`nextRunAt` refusal.
///
/// Takes no [`ScheduleFireContext`], and that absence is deliberate: upstream's `finishRun` ends
/// in `this.arm(...)` and drops the run from `observedAsyncIds`, and cyrup has neither — the tick
/// re-reads the store every period, and the settle edge is the launch's own completion future
/// rather than a subscription this would have to deregister from. A context parameter here would
/// be a threading obligation with nothing behind it.
pub async fn finish_run(
    store: &ScheduleStore,
    schedule: &mut ScheduleRecord,
    run: &mut ScheduleRunRecord,
    success: bool,
    error: Option<String>,
    now: i64,
) -> Result<(), ScheduleStoreError> {
    // `:905-918` — the occurrence that came due while this run was in flight.
    if next_run_at(schedule)
        .map_err(ScheduleStoreError::Refused)?
        .is_some_and(|next| next <= now)
        && let Some(planned) = due_planned_at(schedule, now).map_err(ScheduleStoreError::Refused)?
    {
        let skipped = ScheduleRunRecord {
            schema_version: ScheduleVersion,
            id: ScheduleRunId::mint(),
            schedule_id: schedule.id.clone(),
            planned_at: schedule_timestamp(planned),
            due_reason: ScheduleDueReason::Timer,
            state: ScheduleRunState::Skipped,
            started_at: None,
            completed_at: Some(schedule_timestamp(now)),
            async_id: None,
            async_dir: None,
            error: None,
        };
        schedule
            .trigger
            .set_next_run_at(next_after(&schedule.trigger, planned, now));
        store
            .write_run(schedule, &skipped, "schedule.skipped_overlap")
            .await?;
    }
    run.state = if success {
        ScheduleRunState::Completed
    } else {
        ScheduleRunState::FailedRun
    };
    run.completed_at = Some(schedule_timestamp(now));
    if !success && let Some(error) = error {
        run.error = Some(error);
    }
    schedule.active_run_id = None;
    schedule.updated_at = schedule_timestamp(now);
    store.write(schedule).await?;
    store.release_active_lock(&schedule.id).await?;
    let event = if success {
        "schedule.run.completed"
    } else {
        "schedule.run.failed"
    };
    store.write_run(schedule, run, event).await
}

/// pi `recordMissed` (`scheduled-runs.ts:931-946`) — reachable only when `catch_up == None`.
///
/// # Errors
///
/// A store failure.
pub async fn record_missed(
    store: &ScheduleStore,
    schedule: &mut ScheduleRecord,
    planned: i64,
    due_reason: ScheduleDueReason,
    now: i64,
) -> Result<ScheduleRunRecord, ScheduleStoreError> {
    let run = ScheduleRunRecord {
        schema_version: ScheduleVersion,
        id: ScheduleRunId::mint(),
        schedule_id: schedule.id.clone(),
        planned_at: schedule_timestamp(planned),
        due_reason,
        state: ScheduleRunState::Missed,
        started_at: None,
        completed_at: Some(schedule_timestamp(now)),
        async_id: None,
        async_dir: None,
        error: None,
    };
    schedule
        .trigger
        .set_next_run_at(next_after(&schedule.trigger, planned, now));
    schedule.updated_at = schedule_timestamp(now);
    store.write(schedule).await?;
    store.write_run(schedule, &run, "schedule.missed").await?;
    Ok(run)
}

// =================================================================================================
// §1.6 — crash recovery
// =================================================================================================

/// pi `restore` (`scheduled-runs.ts:743-745`) — [`restore_one`] over every record in the store.
///
/// Runs on the `SessionStart` edge. Errors are logged per schedule rather than propagated: one
/// wedged schedule must not stop the other nineteen from being recovered.
pub async fn restore(store: &ScheduleStore, ctx: &ScheduleFireContext, now: i64) {
    let (schedules, diagnostics) = store.list().await;
    for diagnostic in diagnostics {
        tracing::warn!(%diagnostic, "skipping an unreadable schedule record during restore");
    }
    for mut schedule in schedules {
        let id = schedule.id.clone();
        if let Err(error) = restore_one(store, ctx, &mut schedule, now).await {
            tracing::warn!(schedule_id = %id, %error, "failed to restore a schedule");
        }
    }
}

/// pi `restoreOne` (`scheduled-runs.ts:747-786`) — what state a crash left this schedule in.
///
/// * a `Running` record whose `async_dir` reports a TERMINAL status is settled through
///   [`finish_run`]. `ENOENT` is swallowed (the run directory is gone); any other read error
///   propagates.
/// * **the stale-claim recovery** (`:761-772`): an `active_run_id` with no record, a record that
///   is not `Running`, or a `Running` record with **no `async_id`** whose
///   `started_at + STALE_LAUNCH_CLAIM_MS <= now` is marked `FailedLaunch` with
///   [`STALE_LAUNCH_CLAIM_ERROR`], the claim cleared and `active.lock` DELETED. This is the only
///   thing that unwedges a schedule whose process died between the lock and the launch.
/// * then, unless paused: a `next_run_at` in the past with no active run and `catch_up == None`
///   is [`record_missed`] (`:777-784`).
///
/// `[CYRUP-DELTA]` upstream's terminal set is `complete|failed|stopped|rejected`; cyrup's
/// [`crate::background::RunState`] has no `rejected`, so the set is its three-member
/// [`crate::background::RunState::is_terminal`].
///
/// # Errors
///
/// A store failure, an unreadable `status.json` that is not simply absent, or the trigger
/// algebra's invalid-`nextRunAt` refusal.
pub async fn restore_one(
    store: &ScheduleStore,
    ctx: &ScheduleFireContext,
    schedule: &mut ScheduleRecord,
    now: i64,
) -> Result<(), ScheduleStoreError> {
    if !schedule_belongs_to_session(schedule, &ctx.session) {
        return Ok(());
    }
    if schedule.active_run_id.is_some() {
        let active_id = schedule.active_run_id.clone();
        let mut run = store
            .history(&schedule.id)
            .await?
            .into_iter()
            .find(|item| Some(&item.id) == active_id.as_ref());

        if let Some(record) = run.as_mut()
            && record.state == ScheduleRunState::Running
            && let Some(async_dir) = record.async_dir.clone()
        {
            // `:758` — ENOENT is swallowed, every other error propagates.
            // `read_status_file` already maps "not found" to `Ok(None)`.
            let status =
                crate::background::control::read_status_file(&async_dir.join("status.json"))
                    .await
                    .map_err(|error| ScheduleStoreError::Refused(error.to_string()))?;
            if let Some(status) = status
                && status.state.is_terminal()
            {
                let success = status.state == crate::background::RunState::Complete;
                finish_run(store, schedule, record, success, status.error.clone(), now).await?;
            }
        }

        let started_at = run
            .as_ref()
            .and_then(|record| record.started_at.as_deref())
            .and_then(super::schedule::parse_schedule_timestamp);
        let stale = match run.as_ref() {
            None => true,
            Some(record) if record.state != ScheduleRunState::Running => true,
            Some(record) => {
                record.async_id.is_none()
                    && started_at.is_some_and(|started| started + STALE_LAUNCH_CLAIM_MS <= now)
            }
        };
        if schedule.active_run_id.is_some() && stale {
            if let Some(record) = run.as_mut()
                && record.state == ScheduleRunState::Running
            {
                record.state = ScheduleRunState::FailedLaunch;
                record.completed_at = Some(schedule_timestamp(now));
                record.error = Some(STALE_LAUNCH_CLAIM_ERROR.to_string());
                store
                    .write_run(schedule, record, "schedule.run.failed")
                    .await?;
            }
            schedule.active_run_id = None;
            schedule.updated_at = schedule_timestamp(now);
            store.write(schedule).await?;
            store.release_active_lock(&schedule.id).await?;
        }
    }
    if schedule.paused {
        return Ok(());
    }
    let Some(next) = next_run_at(schedule).map_err(ScheduleStoreError::Refused)? else {
        return Ok(());
    };
    if schedule.active_run_id.is_none() && next < now && schedule.catch_up == ScheduleCatchUp::None
    {
        record_missed(store, schedule, next, ScheduleDueReason::Timer, now).await?;
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

    use super::*;
    use crate::background::scheduled_runs::schedule::{
        ScheduleOverlapSkip, ScheduleTarget, ScheduleVersion,
    };

    /// What one stub launch was asked to do, and what it answered.
    #[derive(Default)]
    struct StubState {
        launches: Vec<ScheduleId>,
        reservations: usize,
    }

    /// A [`ScheduleLauncher`] that records instead of running.
    ///
    /// The production launcher spawns a real `RunMode::Workflow` run; these rows are about the
    /// state machine AROUND that launch — the lock, the claim, the rollback, the catch-up
    /// arithmetic — so a stub is the right seam and the integration suite
    /// (`tests/scheduled_runs_integration.rs`) covers the production one.
    struct StubLauncher {
        state: Mutex<StubState>,
        /// `None` succeeds; `Some(message)` fails the launch with that message.
        fail_launch: Option<String>,
        /// `None` admits; `Some(message)` refuses the spawn budget with that message.
        refuse_budget: Option<String>,
    }

    impl StubLauncher {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(StubState::default()),
                fail_launch: None,
                refuse_budget: None,
            })
        }

        fn failing(message: &str) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(StubState::default()),
                fail_launch: Some(message.to_string()),
                refuse_budget: None,
            })
        }

        fn at_cap(message: &str) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(StubState::default()),
                fail_launch: None,
                refuse_budget: Some(message.to_string()),
            })
        }

        fn launches(&self) -> usize {
            self.state.lock().expect("stub state").launches.len()
        }

        fn reservations(&self) -> usize {
            self.state.lock().expect("stub state").reservations
        }
    }

    #[async_trait::async_trait]
    impl ScheduleLauncher for StubLauncher {
        fn reserve_spawn_slot(&self) -> Result<(), String> {
            self.state.lock().expect("stub state").reservations += 1;
            match &self.refuse_budget {
                Some(message) => Err(message.clone()),
                None => Ok(()),
            }
        }

        async fn launch(
            &self,
            request: ScheduleLaunchRequest<'_>,
        ) -> Result<ScheduleLaunchOutcome, String> {
            self.state
                .lock()
                .expect("stub state")
                .launches
                .push(request.schedule.id.clone());
            if let Some(message) = &self.fail_launch {
                return Err(message.clone());
            }
            Ok(ScheduleLaunchOutcome {
                async_id: format!("run-{}", request.run_id),
                async_dir: request.schedule.cwd.join("async").join("run"),
                // `None`: these rows never settle, which is exactly what keeps the claim held so
                // the overlap and stale-claim rows have something to observe.
                completion: None,
            })
        }
    }

    fn context(launcher: Arc<StubLauncher>) -> ScheduleFireContext {
        ScheduleFireContext {
            session: ScheduleSessionSnapshot::default(),
            launcher,
        }
    }

    fn store_in(root: &Path) -> ScheduleStore {
        ScheduleStore::new(root.join("schedules"), Some(root.to_path_buf()))
    }

    /// An interval schedule due at `next_run_at`.
    fn interval_schedule(id: &str, cwd: &Path, every_ms: i64, next_run_at: i64) -> ScheduleRecord {
        ScheduleRecord {
            schema_version: ScheduleVersion,
            id: ScheduleId::parse(id).expect("legal id"),
            name: id.to_string(),
            cwd: cwd.to_path_buf(),
            trigger: ScheduleTrigger::Interval {
                every: "1h".to_string(),
                every_ms,
                anchor_at: schedule_timestamp(next_run_at - every_ms),
                next_run_at: schedule_timestamp(next_run_at),
            },
            target: ScheduleTarget {
                workflow_script: "return 1;".to_string(),
                args: serde_json::Map::new(),
                base_ref: None,
            },
            overlap: ScheduleOverlapSkip,
            catch_up: ScheduleCatchUp::Latest,
            timeout_ms: None,
            paused: false,
            session_only: None,
            quiet: None,
            owner_session_file: None,
            created_at: schedule_timestamp(0),
            updated_at: schedule_timestamp(0),
            active_run_id: None,
            last_run_id: None,
        }
    }

    const HOUR: i64 = 3_600_000;
    const NOW: i64 = 1_800_000_000_000;

    /// SUBTASK1's first row: a schedule whose `nextRunAt` is in the past FIRES, without anyone
    /// asking, and leaves the exclusive claim behind for the run it started.
    #[tokio::test]
    async fn a_due_schedule_fires() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let ctx = context(Arc::clone(&launcher));
        let schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 1);
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert_eq!(runs.len(), 1, "exactly one run record");
        assert_eq!(runs[0].state, ScheduleRunState::Running);
        assert_eq!(launcher.launches(), 1, "exactly one launch");
        let stored = store.get(&schedule.id).await.expect("reread");
        assert_eq!(
            stored.active_run_id.as_ref(),
            Some(&runs[0].id),
            "the claim names the run record, not the run"
        );
        assert_eq!(
            store
                .active_lock_holder(&schedule.id)
                .await
                .expect("lock read"),
            Some(runs[0].id.clone()),
            "a successful launch LEAVES the lock held for the whole run"
        );
    }

    /// The bound: `nextRunAt > now` is not due, and the tick must not touch it at all.
    #[tokio::test]
    async fn a_not_yet_due_schedule_does_not_fire() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let ctx = context(Arc::clone(&launcher));
        let schedule = interval_schedule("nightly", dir.path(), HOUR, NOW + 1);
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert!(runs.is_empty(), "nothing is due");
        assert_eq!(launcher.launches(), 0);
        assert_eq!(
            launcher.reservations(),
            0,
            "a schedule that is not due must not even consult the spawn budget"
        );
        assert_eq!(
            store
                .active_lock_holder(&schedule.id)
                .await
                .expect("lock read"),
            None
        );
        assert_eq!(store.get(&schedule.id).await.expect("reread"), schedule);
    }

    /// §5 — the budget gate binds, it is consulted BEFORE the lock, and a refused fire advances
    /// rather than hot-retrying on every tick.
    #[tokio::test]
    async fn a_fired_schedule_is_refused_when_the_session_is_at_its_spawn_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::at_cap("Subagent spawn limit reached.");
        let ctx = context(Arc::clone(&launcher));
        let schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 1);
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, ScheduleRunState::FailedLaunch);
        assert_eq!(
            runs[0].error.as_deref(),
            Some("Subagent spawn limit reached."),
            "the budget's own sentence becomes the run record's error"
        );
        assert_eq!(launcher.launches(), 0, "no launch was attempted");
        assert_eq!(
            store
                .active_lock_holder(&schedule.id)
                .await
                .expect("lock read"),
            None,
            "a refusal costs NO lock — the gate is ahead of the claim"
        );
        let stored = store.get(&schedule.id).await.expect("reread");
        let next = next_run_at(&stored).expect("parses").expect("armed");
        assert!(
            next > NOW,
            "a schedule refused at the cap must not hot-retry every tick: {next} <= {NOW}"
        );
    }

    /// A one-shot armed at `at`, for the rows that need a trigger `next_after` retires.
    fn once_schedule(id: &str, cwd: &Path, at: i64) -> ScheduleRecord {
        let mut schedule = interval_schedule(id, cwd, HOUR, at);
        schedule.trigger = ScheduleTrigger::Once {
            at: schedule_timestamp(at),
            next_run_at: Some(schedule_timestamp(at)),
        };
        schedule
    }

    /// §5's refusal must honour `advance` like every other outcome in `launch`.
    ///
    /// A MANUAL fire (`schedule.run`, `advance = false`) that never launched did not consume the
    /// pending occurrence. The budget branch advanced unconditionally, so a manual fire attempted
    /// at the cap silently ate the next scheduled slot — and on a ONE-SHOT, where `next_after` is
    /// `None`, it retired the schedule outright: one refused manual fire and it could never run.
    #[tokio::test]
    async fn a_manual_fire_refused_at_the_cap_keeps_its_pending_occurrence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::at_cap("Subagent spawn limit reached.");
        let ctx = context(Arc::clone(&launcher));

        let mut interval = interval_schedule("nightly", dir.path(), HOUR, NOW + HOUR);
        store.write(&interval).await.expect("seed");
        let armed = interval.trigger.next_run_at().expect("armed").to_string();
        let run = launch(
            &store,
            &ctx,
            &mut interval,
            NOW,
            ScheduleDueReason::Manual,
            false,
            None,
            NOW,
        )
        .await
        .expect("a budget refusal is a record, not an Err");
        assert_eq!(run.state, ScheduleRunState::FailedLaunch);
        assert_eq!(
            store
                .get(&interval.id)
                .await
                .expect("reread")
                .trigger
                .next_run_at(),
            Some(armed.as_str()),
            "a manual fire that never launched must not eat the next scheduled slot"
        );

        // The one-shot, where the same bug was fatal rather than merely lossy.
        let mut once = once_schedule("once", dir.path(), NOW + HOUR);
        store.write(&once).await.expect("seed");
        let run = launch(
            &store,
            &ctx,
            &mut once,
            NOW,
            ScheduleDueReason::Manual,
            false,
            None,
            NOW,
        )
        .await
        .expect("a budget refusal is a record, not an Err");
        assert_eq!(run.state, ScheduleRunState::FailedLaunch);
        let stored = store.get(&once.id).await.expect("reread");
        assert_eq!(
            stored.trigger.next_run_at(),
            Some(schedule_timestamp(NOW + HOUR).as_str()),
            "a one-shot refused at the cap is still PENDING — retiring it here would mean one \
             refused manual fire destroyed the schedule"
        );
        assert!(
            has_pending_schedule_work(&stored),
            "and it still counts as pending work"
        );

        // A TIMER fire (`advance = true`) still advances, so this fix cannot reintroduce the
        // hot-retry the branch exists to prevent.
        let mut ticked = interval_schedule("ticked", dir.path(), HOUR, NOW - 1);
        store.write(&ticked).await.expect("seed");
        launch(
            &store,
            &ctx,
            &mut ticked,
            NOW - 1,
            ScheduleDueReason::Timer,
            true,
            None,
            NOW,
        )
        .await
        .expect("launch");
        let next = next_run_at(&store.get(&ticked.id).await.expect("reread"))
            .expect("parses")
            .expect("armed");
        assert!(
            next > NOW,
            "a schedule refused at the cap on a TIMER fire must still move past now: {next} <= {NOW}"
        );
    }

    /// `overlap: "skip"`, the in-process half: a second fire while a run is still active is
    /// recorded as skipped and launches nothing.
    #[tokio::test]
    async fn a_second_fire_while_a_run_is_active_is_skipped_not_doubled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let ctx = context(Arc::clone(&launcher));
        let schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 1);
        store.write(&schedule).await.expect("seed");

        tick_due_schedules(&store, &ctx, NOW).await.expect("fire 1");
        // A second tick a full period later, with the first run still unsettled.
        let second = tick_due_schedules(&store, &ctx, NOW + HOUR)
            .await
            .expect("fire 2");

        assert_eq!(second.len(), 1);
        assert_eq!(second[0].state, ScheduleRunState::Skipped);
        assert_eq!(
            launcher.launches(),
            1,
            "exactly one launch across both ticks"
        );
        let history = store.history(&schedule.id).await.expect("history");
        assert!(
            history
                .iter()
                .any(|run| run.state == ScheduleRunState::Skipped),
            "the skip is recorded: {history:?}"
        );
    }

    /// `overlap: "skip"`, the CROSS-PROCESS half — the `O_EXCL` claim. A lock another instance
    /// already holds makes this fire skip, and the pre-existing lock is NOT removed.
    #[tokio::test]
    async fn a_concurrent_fire_loses_the_exclusive_lock_and_skips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let ctx = context(Arc::clone(&launcher));
        let schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 1);
        store.write(&schedule).await.expect("seed");
        // Another instance got there first. Its claim is on disk; ours is not in memory.
        let foreign = ScheduleRunId::mint();
        assert!(
            store
                .acquire_active_lock(&schedule.id, &foreign)
                .await
                .expect("foreign lock"),
            "precondition: the foreign instance really took the lock"
        );

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, ScheduleRunState::Skipped);
        assert_eq!(launcher.launches(), 0, "no second launch");
        assert_eq!(
            store
                .active_lock_holder(&schedule.id)
                .await
                .expect("lock read"),
            Some(foreign),
            "the OTHER instance's lock must survive: releasing it here would let the next tick \
             double-launch the run it guards"
        );
    }

    /// The failure rollback (`:886-899`): the claim is cleared, the lock is released, and a
    /// MANUAL fire (`advance = false`) restores the slot it had not consumed.
    #[tokio::test]
    async fn a_failed_launch_releases_the_lock_and_restores_the_next_run_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::failing("the runner refused");
        let ctx = context(Arc::clone(&launcher));
        let mut schedule = interval_schedule("nightly", dir.path(), HOUR, NOW + HOUR);
        store.write(&schedule).await.expect("seed");
        let before = schedule.trigger.next_run_at().expect("armed").to_string();

        let run = launch(
            &store,
            &ctx,
            &mut schedule,
            NOW,
            ScheduleDueReason::Manual,
            false,
            None,
            NOW,
        )
        .await
        .expect("launch returns a record, never an Err, for a launcher failure");

        assert_eq!(run.state, ScheduleRunState::FailedLaunch);
        assert_eq!(run.error.as_deref(), Some("the runner refused"));
        let stored = store.get(&schedule.id).await.expect("reread");
        assert_eq!(stored.active_run_id, None, "the claim is cleared");
        assert_eq!(
            stored.trigger.next_run_at(),
            Some(before.as_str()),
            "a manual run that failed must not eat the next scheduled slot"
        );
        assert_eq!(
            store
                .active_lock_holder(&schedule.id)
                .await
                .expect("lock read"),
            None,
            "the lock is released on the failure path"
        );
    }

    /// §1.6 — the ONLY thing that unwedges a schedule whose process died between the lock and the
    /// launch.
    #[tokio::test]
    async fn a_stale_launch_claim_is_recovered_on_restore() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let ctx = context(StubLauncher::ok());
        let mut schedule = interval_schedule("nightly", dir.path(), HOUR, NOW + HOUR);
        let run_id = ScheduleRunId::mint();
        schedule.active_run_id = Some(run_id.clone());
        schedule.last_run_id = Some(run_id.clone());
        store.write(&schedule).await.expect("seed");
        store
            .acquire_active_lock(&schedule.id, &run_id)
            .await
            .expect("seed lock");
        let claim = ScheduleRunRecord {
            schema_version: ScheduleVersion,
            id: run_id.clone(),
            schedule_id: schedule.id.clone(),
            planned_at: schedule_timestamp(NOW - 6 * 60_000),
            due_reason: ScheduleDueReason::Timer,
            state: ScheduleRunState::Running,
            started_at: Some(schedule_timestamp(NOW - 6 * 60_000)),
            completed_at: None,
            // NO `async_id` — the process died before the run was attached.
            async_id: None,
            async_dir: None,
            error: None,
        };
        store
            .write_run(&schedule, &claim, "schedule.run.started")
            .await
            .expect("seed run");

        restore_one(&store, &ctx, &mut schedule, NOW)
            .await
            .expect("restore");

        let stored = store.get(&schedule.id).await.expect("reread");
        assert_eq!(stored.active_run_id, None, "the wedged claim is cleared");
        let recovered = store
            .history(&schedule.id)
            .await
            .expect("history")
            .into_iter()
            .find(|run| run.id == run_id)
            .expect("the claim's own record");
        assert_eq!(recovered.state, ScheduleRunState::FailedLaunch);
        assert_eq!(
            recovered.error.as_deref(),
            Some(STALE_LAUNCH_CLAIM_ERROR),
            "upstream's verbatim recovery sentence"
        );
        assert_eq!(
            store
                .active_lock_holder(&schedule.id)
                .await
                .expect("lock read"),
            None,
            "the stale lock is removed, or nothing could ever fire again"
        );
    }

    /// `catchUp: "latest"` fires ONCE, at the most recent missed slot — never a backlog, and never
    /// the oldest one.
    #[tokio::test]
    async fn catch_up_latest_fires_once_at_the_latest_missed_slot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let ctx = context(Arc::clone(&launcher));
        // Armed five hours ago, on an hourly interval, and we wake HALF AN HOUR past a slot: five
        // occurrences came and went while the process was down.
        let now = NOW + HOUR / 2;
        let schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 5 * HOUR);
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, now)
            .await
            .expect("the tick must succeed");

        assert_eq!(runs.len(), 1, "ONE record, not five: {runs:?}");
        assert_eq!(launcher.launches(), 1, "ONE launch, not five");
        assert_eq!(
            runs[0].planned_at,
            schedule_timestamp(NOW),
            "the LATEST slot at or before now (5 periods on from the armed one), never the oldest \
             and never a slot in the future"
        );
        assert_ne!(
            runs[0].planned_at,
            schedule_timestamp(NOW - 5 * HOUR),
            "firing at the ARMED slot would mean the backlog was replayed from the beginning"
        );
    }

    /// `catchUp: "none"` records the missed occurrence instead of running it, and still moves the
    /// schedule forward.
    #[tokio::test]
    async fn catch_up_none_records_a_missed_occurrence_instead_of_firing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let ctx = context(Arc::clone(&launcher));
        let mut schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 5 * HOUR);
        schedule.catch_up = ScheduleCatchUp::None;
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, ScheduleRunState::Missed);
        assert_eq!(launcher.launches(), 0, "a missed occurrence does NOT run");
        let stored = store.get(&schedule.id).await.expect("reread");
        let next = next_run_at(&stored).expect("parses").expect("armed");
        assert!(next > NOW, "the schedule moved past now: {next} <= {NOW}");
    }

    /// `nextAfter`'s `while`, not a single add: a process asleep for six intervals must land in
    /// the FUTURE, not six fires behind.
    #[tokio::test]
    async fn a_schedule_asleep_for_many_intervals_lands_in_the_future() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let ctx = context(StubLauncher::ok());
        let mut schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 6 * HOUR);
        store.write(&schedule).await.expect("seed");

        let planned = due_planned_at(&schedule, NOW)
            .expect("parses")
            .expect("due");
        launch(
            &store,
            &ctx,
            &mut schedule,
            planned,
            ScheduleDueReason::Timer,
            true,
            None,
            NOW,
        )
        .await
        .expect("launch");

        let stored = store.get(&schedule.id).await.expect("reread");
        let next = next_run_at(&stored).expect("parses").expect("armed");
        assert!(
            next > NOW,
            "a single `+= every_ms` would leave this five hours in the past: {next} <= {NOW}"
        );
        assert!(
            next <= NOW + HOUR,
            "and it must land on the NEXT slot, not skip ahead: {next} > {}",
            NOW + HOUR
        );
    }

    /// The counterweight to "a schedule outlives its session": a `sessionOnly` schedule does NOT
    /// fire for a session that is not its owner. Without this row, a change that dropped the
    /// ownership gate entirely would pass every other row in this module.
    #[tokio::test]
    async fn a_session_only_schedule_does_not_fire_in_a_later_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        // A LATER session: it has an identity of its own, and it is not the owner's.
        let ctx = ScheduleFireContext {
            session: ScheduleSessionSnapshot::new(
                SessionId::parse("session-b"),
                Some(&dir.path().join("session-b.jsonl")),
            ),
            launcher: Arc::clone(&launcher) as Arc<dyn ScheduleLauncher>,
        };
        let mut schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 1);
        schedule.session_only = Some(true);
        schedule.owner_session_file = Some(dir.path().join("session-a.jsonl"));
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert!(runs.is_empty(), "a foreign session fires nothing: {runs:?}");
        assert_eq!(launcher.launches(), 0);
        assert_eq!(
            store.get(&schedule.id).await.expect("reread"),
            schedule,
            "the record is untouched"
        );
    }

    /// The same schedule DOES fire for the session that owns it — so the row above is proving the
    /// gate, not proving that `sessionOnly` schedules never fire at all.
    #[tokio::test]
    async fn a_session_only_schedule_fires_for_its_own_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let launcher = StubLauncher::ok();
        let owner = dir.path().join("session-a.jsonl");
        let ctx = ScheduleFireContext {
            session: ScheduleSessionSnapshot::new(SessionId::parse("session-a"), Some(&owner)),
            launcher: Arc::clone(&launcher) as Arc<dyn ScheduleLauncher>,
        };
        let mut schedule = interval_schedule("nightly", dir.path(), HOUR, NOW - 1);
        schedule.session_only = Some(true);
        schedule.owner_session_file = Some(owner);
        store.write(&schedule).await.expect("seed");

        let runs = tick_due_schedules(&store, &ctx, NOW)
            .await
            .expect("the tick must succeed");

        assert_eq!(runs.len(), 1);
        assert_eq!(launcher.launches(), 1);
    }

    /// The tick's period is a deliberate choice against the smallest schedulable interval
    /// (`parse_schedule_interval`'s `1m` floor), not whatever the first test needed.
    #[test]
    fn the_tick_is_at_most_half_the_smallest_schedulable_interval() {
        let smallest = super::super::schedule::parse_schedule_interval("1m")
            .expect("`1m` is the smallest legal interval");
        assert_eq!(smallest, 60_000);
        let tick = i64::try_from(SCHEDULE_TICK.as_millis()).expect("tick fits");
        assert!(
            tick * 2 <= smallest,
            "worst-case lateness is one tick; at more than half the smallest legal interval a \
             `1m` schedule would drift to a slower cadence than it asked for ({tick}ms tick vs \
             {smallest}ms interval)"
        );
    }

    /// The JS `setTimeout` i32 ceiling is deliberately absent: a 30-day schedule must arm at 30
    /// days, not be silently capped at 24.8.
    #[test]
    fn a_thirty_day_interval_is_not_capped_at_the_js_timer_ceiling() {
        let thirty_days = super::super::schedule::parse_schedule_interval("30d").expect("legal");
        assert!(
            thirty_days > 2_147_483_647,
            "precondition: 30 days really does exceed the i32 ceiling upstream carries"
        );
        let trigger = ScheduleTrigger::Interval {
            every: "30d".to_string(),
            every_ms: thirty_days,
            anchor_at: schedule_timestamp(NOW),
            next_run_at: schedule_timestamp(NOW + thirty_days),
        };
        let next = next_after(&trigger, NOW, NOW).expect("an interval always re-arms");
        assert_eq!(
            super::super::schedule::parse_schedule_timestamp(&next),
            Some(NOW + thirty_days),
            "the full 30 days, not MAX_TIMER_DELAY_MS"
        );
    }
}
