//! SUBA-016 — SCHEDULED RUNS: the persisted schedule, its on-disk store, the capability-ceiling
//! gate that governs whether a schedule may be persisted at all, and — since part B — the trigger
//! that makes a schedule actually FIRE and the nine `schedule.*` tool actions.
//!
//! Ports pi `runs/background/scheduled-runs.ts` (1012 LOC @ `v0.68.0`). Part A landed the
//! persistence half; part B landed [`trigger`], [`tool`] and [`manager`] against the handoff
//! stated below, and every clause of it held.
//!
//! # What a schedule is
//!
//! A named, durable instruction to run a workflow script in a project directory at a time, or on
//! an interval. It is the only artefact in this crate that is expected to outlive the process
//! that created it, the session that created it, and the terminal that created it. Every decision
//! in this module falls out of that one sentence.
//!
//! # DECISION — the store is keyed by CWD, never by session
//!
//! [`scheduled_run_store_path`] accepts a session id and discards it, exactly as upstream's
//! `_sessionId` parameter does (`:98`). A schedule outliving its session is the entire point of
//! scheduling, so session-keying the store would silently lose every schedule at the end of the
//! turn that created it. The parameter is kept — with its underscore and with a
//! [`crate::identity::SessionId`] type — so a caller that holds a session hands it over and is
//! answered "yes, and it does not matter", rather than discovering later that it was never
//! threaded through.
//!
//! This module is the counter-example that proves this programme's rule is *scoping*, not
//! *session-keying everything*. Session enters this file in exactly one place, and it is not the
//! path: it is the AUTHORITY to write to it ([`ceiling_gate`]).
//!
//! # DECISION — the default store is PROJECT-LOCAL, not a fifth run-scratch sibling
//!
//! `background/artifact_roots.rs` keys four directories per cwd under
//! [`crate::background::temp_root_dir`] (`async`, `results`, `scratch`, `wait-subscriptions`), and
//! `wait_subscriptions`' own module doc argues at length for adding a fourth. **That precedent
//! must not be followed here.** That tree's own doc calls it *"reboot-disposable run scratch"*,
//! and with `CYRUP_HOME` unset — its only state outside tests — it resolves under
//! [`std::env::temp_dir`]. A schedule whose whole purpose is to fire in six hours, tomorrow or
//! next week cannot live in a directory the OS is entitled to clear on reboot or by a tmp reaper.
//!
//! Upstream reached the same conclusion first: `scheduledRunStorePath`'s default branch is the
//! ONLY store in `pi-subagents` that lands in the project tree, while every neighbour in that
//! file's vicinity (`ASYNC_DIR`, `RESULTS_DIR`, `wait-subscriptions`, `model-exclusions.json`) is
//! temp-rooted. The default is therefore
//! [`crate::artifacts::project_subagents_dir`]`/`[`SCHEDULES_SUBDIR`] — a fourth leaf on the
//! existing `<cwd>/.cyrup-subagents/<leaf>` family, beside `artifacts` and `chain-runs`.
//!
//! # DECISION — version intolerance is a PER-RECORD failure, not a per-store one
//!
//! [`schedule::ScheduleVersion`] rejects any value but `1` at deserialize time, so a future
//! record can never be half-read and can never panic. What differs from upstream is what the
//! STORE does with that: upstream's `list` is `ids().map(find)` over a throwing parser, so one
//! unreadable record makes every schedule in the project unlistable. Here
//! [`store::ScheduleStore::list`] skips it and reports it, while
//! [`store::ScheduleStore::get`] — a named, addressed record — still errors. The full argument is
//! in `store.rs`'s own module doc.
//!
//! # DECISION — `0600` on `events.jsonl` is enforced at this call site
//!
//! Upstream appends its event log with `{ mode: 0o600 }` (`:375`, `:381`);
//! [`crate::jsonl::BoundedJsonlWriter`], the crate's mandatory `.jsonl` append path, sets no mode.
//! `store.rs` chmods the file after opening it rather than adding a `mode` option to the shared
//! primitive, because a chmod also repairs a file an earlier build left world-readable while an
//! `OpenOptions::mode` — which applies only at creation — would not.
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs           facade, the format constants, and these decisions
//! schedule.rs      the records, their parsed identities, the one parser, and the two
//!                  trigger-input parsers (`at` / `every`) with the epoch <-> ISO boundary
//! store.rs         the store path, the containment guard, and every primitive the trigger drives
//! ceiling_gate.rs  the create-time capability-ceiling gate, alone and on purpose
//! trigger.rs       the trigger algebra, the launch/skip/miss/finish state machine, the pinned
//!                  session identity, and crash recovery
//! tool.rs          the nine `schedule.*` actions
//! manager.rs       one store, one tick task, one lifecycle
//! ```
//!
//! `ceiling_gate.rs` is its own file rather than a function beside
//! [`store::ScheduleStore::write`] for a concrete reason: a gate that lives next to the writer
//! will, sooner or later, be called FROM the writer — and that wedges every schedule with a run
//! in flight when a ceiling is registered. See that file's own doc.
//!
//! # <a id="the-handoff"></a>The part-B handoff, as it was stated and as it landed
//!
//! 1. **This module owns the FILE FORMAT and the DIRECTORY.**
//!    [`schedule::ScheduleRecord`], [`schedule::ScheduleRunRecord`],
//!    [`schedule::ScheduleHistory`], [`schedule::ScheduleEvent`], `schedule.json` /
//!    `history.json` / `runs/<id>.json` / `events.jsonl` / `active.lock` as paths and primitives,
//!    [`scheduled_run_store_path`], [`schedule::ScheduleId`] and the containment guard. **Part B
//!    writes no new on-disk format.**
//! 2. **This module owns the create-time gate as a CALLABLE.**
//!    [`ceiling_gate::schedule_persistence_refusal`] is `pub` and its production caller is part
//!    B's `create` action. It is not invoked from the write path here, and must not be added to
//!    it there.
//! 3. **Part B owns every decision that consults a clock or a session.** `parseScheduledRunTime`,
//!    `parseScheduleInterval`, `nextAfter`, `nextRunAt`, `duePlannedAt`, `hasPendingScheduleWork`,
//!    the launch/skip/miss/finish state machine, stale-launch-claim recovery
//!    (`STALE_LAUNCH_CLAIM_MS`), the pinned-session identity, `scheduleBelongsToSession`, the
//!    `maxPending` cap, the nine `schedule.*` tool actions, `ScheduledRunsConfig`, and the tool
//!    description's schedule mention. All of it landed in [`trigger`], [`tool`], [`manager`],
//!    `registration::SubagentExtensionConfig::scheduled_runs` and
//!    `registration::tool_description` — with ONE correction to the last clause: the four
//!    `schedule*` verbs it named (`schedule`, `schedule-list`, `schedule-status`,
//!    `schedule-cancel`) are `v0.34.0` vocabulary that no longer exists upstream, so what was
//!    restored is `v0.68.0`'s `schedule.*`, and the guard that policed the deletion became a
//!    mechanical membership check instead of a hard-coded denylist.
//! 4. **Part B does not re-key the store.** If it finds itself wanting a session in the path,
//!    that is the bug the first decision above exists to prevent.

pub mod ceiling_gate;
pub mod manager;
pub mod schedule;
pub mod store;
pub mod tool;
pub mod trigger;

#[cfg(test)]
mod test_fixtures;

/// pi's `schemaVersion: 1` on every record this module persists (`:48`, `:65`, `:365`, `:375`).
pub const SCHEDULE_VERSION: u32 = 1;

/// pi `MAX_HISTORY = 100` (`scheduled-runs.ts:33`) — the per-schedule run-history cap.
pub const MAX_HISTORY: usize = 100;

/// The leaf under [`crate::artifacts::project_subagents_dir`] (`:99`'s `"schedules"`).
pub const SCHEDULES_SUBDIR: &str = "schedules";

pub use ceiling_gate::{
    CeilingResolver, MALFORMED_CEILING_SOURCE, SCHEDULE_CEILING_REFUSAL, UNKNOWN_SESSION_KEY,
    ceiling_lookup_key, ceiling_resolver_from, process_ceiling_resolver,
    schedule_persistence_refusal,
};
pub use manager::{ScheduledRunDeps, ScheduledRunManager, ScheduledRunSlot};
pub use schedule::{
    SCHEDULE_ID_ERROR, ScheduleCatchUp, ScheduleDueReason, ScheduleEvent, ScheduleHistory,
    ScheduleId, ScheduleOverlapSkip, ScheduleRecord, ScheduleRunId, ScheduleRunRecord,
    ScheduleRunState, ScheduleTarget, ScheduleTrigger, ScheduleVersion, parse_schedule,
    parse_schedule_history, parse_schedule_interval, parse_schedule_target,
    parse_schedule_timestamp, parse_scheduled_run_time, schedule_timestamp,
};
pub use store::{
    ACTIVE_LOCK_FILE, EVENTS_FILE, HISTORY_FILE, RUNS_SUBDIR, SCHEDULE_FILE, ScheduleStore,
    ScheduleStoreError, project_key, scheduled_run_store_path,
};
pub use tool::{
    SCHEDULED_RUN_ACTIONS, SCHEDULED_RUNS_DISABLED, ScheduledRunAction, ScheduledRunActionContext,
    ScheduledRunActionOutcome, ScheduledRunActionParams, ScheduledRunError,
    handle_scheduled_run_action,
};
pub use trigger::{
    DEFAULT_MAX_PENDING, SCHEDULE_TICK, STALE_LAUNCH_CLAIM_ERROR, STALE_LAUNCH_CLAIM_MS,
    ScheduleFireContext, ScheduleLaunchOutcome, ScheduleLaunchRequest, ScheduleLauncher,
    ScheduleRunCompletion, ScheduleSessionSnapshot, due_planned_at, finish_run,
    has_pending_schedule_work, launch, next_after, next_run_at, normalized_session_file,
    record_missed, restore, restore_one, schedule_belongs_to_session, tick_due_schedules,
};
