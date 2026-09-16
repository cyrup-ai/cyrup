//! Durable wait subscriptions — `{ id, nonBlocking: true }` wakes that survive across turns, and
//! across processes.
//!
//! Ports pi `runs/background/wait-subscriptions.ts` (348 LOC @ `7fe9dee1`, stable across
//! `v0.60.0…v0.67.0`). Narrows `PARITY-GAPS` **VL-S8**.
//!
//! # What this adds, and what it does NOT replace
//!
//! cyrup already has the IN-PROCESS half of the wake: [`crate::background::watch::CompletionBus`]
//! publishes every observed terminal result and [`crate::background::wait`] `select!`s on it while
//! a wait is blocked. That mechanism is untouched. What it cannot do is survive the end of the
//! turn — a `wait` that returns has no subscription left, so an orchestrator that wants to be told
//! *later* has to block *now*. This module is the durable half: a small record on disk, one file
//! per armed subscription, reconciled on a 1 s timer and on every observed completion, which fires
//! a `subagent-wait-subscription` message into the session — including in a LATER turn, and
//! including when the payload has already been delivered and unlinked (it resolves through
//! SCOPE_4's [`crate::background::completion_replay`] record).
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs      facade: the constants (two of them ARE format), the decisions, this narrative
//! record.rs   WaitSubscriptionRecord + its types + the tolerant parser
//! manager.rs  arm / restore / reconcile / settle / sweep / dispose, and the three injected seams
//! format.rs   format_wait_subscriptions + the wake message's content and details
//! ```
//!
//! # The six session comparisons
//!
//! Upstream makes SIX comparisons against `state.currentSessionId`, and two of them point in
//! opposite directions on the same field. They are different PHASES, not a contradiction to be
//! simplified away:
//!
//! | # | pi | phase | rule |
//! |---|---|---|---|
//! | 1 | `:72` (`parseRecord`) | parse | a record with no session is invalid — here a TYPE, see [`WaitSubscriptionRecord::session_id`] |
//! | 2 | `:157` (`sweepExpiredForeignSubscriptions`) | sweep | **skip** the current session: it keeps its own expired records so they can still settle "timed out" |
//! | 3 | `:206` (`reconcileRecord`, first statement) | delivery | **require** the current session |
//! | 4 | `:215` (foreground branch) | agreement | the RUN's session must equal the RECORD's |
//! | 5 | `:327` (`restore`) | load | only this session's records enter the in-memory map |
//! | 6 | `:181` (`settle`) | re-check | reachable from `reconcile`'s catch arm after a THROW, i.e. after a point where the session could have been swapped |
//!
//! #6 is redundant with #3 on the `reconcileRecord` path and is NOT redundant overall. Upstream
//! has both; so does this port.
//!
//! cyrup adds a SEVENTH that upstream gets for free: the async branch's session re-check. Upstream
//! passes `sessionId` into `listAsyncRuns`, which filters; cyrup's
//! [`crate::background::run_status::reconcile_by_id`] applies no filter at all, so the gate is
//! re-applied explicitly. It is the async twin of #4 — see
//! [`WaitSubscriptionManager::reconcile_async`](manager) for the full argument.
//!
//! # DECISION — where the records live on disk
//!
//! Upstream (`:106`) is
//! `path.join(path.dirname(asyncDirRoot), "wait-subscriptions")`, which for pi resolves to
//! `<TEMP_ROOT_DIR>/wait-subscriptions`: pi's `ASYNC_DIR` is one flat, non-cwd-keyed directory
//! (`shared/types.ts:2733`), so `dirname` IS the temp root.
//!
//! **A literal port is a bug in cyrup.** cyrup's async root is
//! `<run_scratch>/async/<cwd_key>` ([`crate::background::run_artifact_roots_in`]), so
//! `dirname(async_root)` is `<run_scratch>/async` — shared by EVERY cwd, and one level too high.
//!
//! The chosen shape is **a fourth cwd-keyed sibling**: `<run_scratch>/wait-subscriptions/<cwd_key>`
//! ([`crate::background::wait_subscriptions_dir_in`]), mirroring `ASYNC_SUBDIR`/`RESULTS_SUBDIR`/
//! `SCRATCH_SUBDIR` (`artifact_roots.rs:27,35,47`). Two reasons it wins over the alternative
//! (`<results_dir>/wait-subscriptions/`, the way [`crate::background::completion_replay`] hangs
//! its own two directories off the results dir):
//!
//! 1. it is the structural analog of upstream's *sibling-of-the-async-root*, and it preserves the
//!    cwd-outer/session-inner layering `background/wait.rs:87-99` states outright;
//! 2. the results dir is SWEPT — `spawn_retention_sweep` (`notices.rs:633`) runs
//!    `cleanup_result_indexes` and `cleanup_completion_replay_if_due` over it — and a subscription
//!    record is not a result. Putting a caller's live wake registration inside a directory two
//!    retention reapers already walk invites exactly the deletion this module exists to prevent.
//!
//! This is an ON-DISK FORMAT decision: a later build that resolves the directory differently does
//! not see the records this one wrote.
//!
//! # DECISION — the settle ORDER: acknowledge, then unlink
//!
//! pi's `settle` removes the record (`:183`) and only then sends (`:189`). That is safe for pi
//! because `pi.sendMessage` is a synchronous in-process call. cyrup's send crosses a process
//! boundary, and [`cyrup_ext::host::InjectOutcome::Accepted`] is documented as *"the ONLY licence
//! for the caller to destroy its own copy of the data."* So this port AWAITS the acknowledgement
//! first and clears the record only once the session has it — upstream's option (a). The full
//! trade (duplicate wake versus lost wake) is argued at
//! [`WaitSubscriptionManager::settle`](manager).
//!
//! # DECISION — the reconcile member is registered LAST
//!
//! The wake edge is a fourth member of `extension/executor/notices.rs`'s
//! `CompositeCompletionObserver`, registered AFTER
//! [`crate::background::wait_completions::WaitCompletionStore`], the mission sync and the
//! completion bus. pi registers four listeners on one event with no documented ordering guarantee
//! (`extension/index.ts:648-659`), so "last" is an assertion this port makes rather than one it
//! inherits — and the reason is concrete: the store is member #1, and a subscription that
//! reconciles must find the record the store is about to write already there, or its `read_completion`
//! falls through a rung for no reason.
//!
//! **`[CYRUP-DELTA]`** upstream subscribes to SIX channels (`:277-284`). cyrup has ONE real edge —
//! the completion observer, which is the result-file observation — plus the 1 s timer. The other
//! five upstream channels (intercom detach requests, the two control channels, the result-intercom
//! channel, the foreground-complete event) have no in-process bus in cyrup to hang off; the timer
//! is what covers them, at a 1 s latency floor rather than 0.
//!
//! # DECISION — dispose is bound to `SessionShutdown` only
//!
//! Upstream disposes at `extension/index.ts:1009` (shutdown) and nowhere else, and this port does
//! the same. cyrup's session-SWITCH path (`native_impl.rs`'s `SessionBeforeSwitch`/
//! `SessionBeforeFork`) moves `current_session_id()` under a live manager, which upstream's
//! single-session model never does — but that case is already covered without a second dispose:
//! `reconcileRecord`'s `:206` gate makes every now-foreign in-memory record inert immediately, and
//! the subsequent `SessionStart` calls [`WaitSubscriptionManager::restore`], whose first act is an
//! unconditional `clear()`. Adding a dispose there would only tear down the reconcile timer that
//! `restore` is about to re-arm.
//!
//! # `[CYRUP-DELTA]` — the foreground attention hole
//!
//! A subscription armed against a foreground run whose supervisor request arrives *after* the run
//! settles is not detectable, because the field that would report it
//! (`ForegroundHistoryChild`'s missing `activity_state`/`current_tool`) is not persisted. Upstream
//! has the same hole for a different reason — `RESTORABLE`
//! (`extension/executor/foreground_history/record.rs:27-31`) deliberately never persists
//! `"detached"` — so this narrows an existing upstream boundary rather than opening a new one.
//! See [`ForegroundSubscriptionProbe`] for the two-map mechanism that closes the rest of it.
//!
//! # The async-retention coupling, now owned
//!
//! Upstream's async retention reaper reads this directory: `async-retention.ts:307-316`'s
//! `parseWaitRunIds` protects a run referenced by a live subscription from being reaped, re-read
//! before each destructive action, and skips the whole pass as `"wait-references-unknown"` when
//! any record is unparseable.
//!
//! The reader half now exists as
//! [`crate::background::async_retention::wait_run_ids`], which consumes [`parse_record`] against
//! the DIRECTORY (never [`WaitSubscriptionManager::armed`], which is narrowed to this session by
//! construction and would leave another instance's waited-on run unprotected in a shared root),
//! and whose `None` is upstream's `safe: false`. The re-read before each destructive action and
//! the pass-level abort are sweep control flow and belong to the sweep.

use std::time::Duration;

mod format;
mod manager;
mod record;

pub use format::{
    SUBSCRIPTION_MESSAGE_CUSTOM_TYPE, format_wait_subscriptions, subscription_message_content,
    subscription_message_details,
};
pub use manager::{
    ForegroundChildState, ForegroundSubscriptionProbe, ForegroundTargetChild,
    ForegroundTargetState, NO_SESSION_IDENTITY, SubscriptionNotifier, SubscriptionOutcome,
    SubscriptionSessions, WaitSubscriptionDeps, WaitSubscriptionManager,
};
pub use record::{
    ArmWaitSubscriptionInput, SubscriptionToken, SubscriptionVersion, WaitSubscriptionRecord,
    WaitTargetKind, parse_record, subscription_file,
};

/// pi `SUBSCRIPTION_VERSION` (`:22`). An ON-DISK FORMAT constant — see [`SubscriptionVersion`],
/// which is how it is actually enforced.
pub const SUBSCRIPTION_VERSION: u32 = 1;

/// pi `RECONCILE_INTERVAL_MS` (`:23`) — the reconcile cadence under the completion-observer edge.
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

/// pi `FOREIGN_SWEEP_INTERVAL_MS` (`:29`), with upstream's own reasoning (`:24-28`):
///
/// > How often the subscriptions directory is re-scanned for expired records left by other
/// > sessions. Rare compared to `RECONCILE_INTERVAL_MS` because it costs a directory read, and
/// > nothing depends on the sweep being prompt.
///
/// A THROTTLE, not a timer: it is checked on the reconcile tick, exactly as
/// [`crate::background::completion_replay::CLEANUP_INTERVAL_MS`] is checked on the write path.
pub const FOREIGN_SWEEP_INTERVAL_MS: i64 = 60_000;

/// pi `FOREIGN_SWEEP_GRACE_MS` (`:37`) — one day — with upstream's own reasoning (`:30-36`):
///
/// > How far past expiry a record armed by another session is kept before it is swept. The owning
/// > session settles its own expired records with a "timed out" notice on the next `restore()`, so
/// > removing one the moment it expires would take that notice away from a session that simply had
/// > not resumed yet. A day is long enough for an ordinary return and short enough to bound the
/// > directory.
///
/// This is also an ON-DISK FORMAT constant in the weak sense that matters: two builds running
/// concurrently over one directory must agree on it, or the shorter-grace build reaps records the
/// longer-grace build is still waiting to settle.
pub const FOREIGN_SWEEP_GRACE_MS: i64 = 24 * 60 * 60 * 1000;
