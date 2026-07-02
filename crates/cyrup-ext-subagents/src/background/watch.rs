//! `ResultsDir` filesystem-watch completion notification (func-SA §5.4 R-SA-098..103; arch-SA
//! §6.5's `background/watch.rs`).
//!
//! This module is the ORCHESTRATOR-side half of the background/async job system's completion
//! signal: it never runs inside the detached runner process (`background/runner_main.rs::run`
//! owns that side — writing the terminal [`ResultFile`] into `ResultsDir` as its very last
//! file-writing act, R-SA-077, which is what makes anything in THIS module observable at all).
//! Everything here is pure orchestrator-process, file-watching, dedup, and classification logic —
//! zero process-spawning, zero in-process handle to the runner (R-SA-098's own explicit "never by
//! the orchestrator holding any live handle to the runner process").
//!
//! # R-SA-098: native watch + poll fallback, never a live-handle assumption
//!
//! [`ResultsWatcher::install`] uses `notify::PollWatcher` configured with a fixed poll interval
//! ([`RESULTS_DIR_POLL_INTERVAL`]). `PollWatcher` is deliberately the single implementation for
//! BOTH the "OS-level filesystem-change notification" and the "fixed-interval poll fallback" halves
//! of R-SA-098's requirement — it already falls back to pure polling on platforms/conditions where
//! a native backend (inotify/FSEvents/ReadDirectoryChangesW) is unavailable or hits a
//! resource-exhaustion-class error (`EMFILE`/`ENOSPC`), so there is no second, separately
//! maintained poll-only code path to keep in sync with a native-only one. This mirrors
//! `background::control::watch_control_inbox`'s identical choice for the control-inbox watch
//! (R-SA-082) and `cyrup_resources::theme::ThemeWatcher`'s established workspace convention.
//!
//! # R-SA-099: parse, verify-session, dedup, notify, delete-last
//!
//! [`ResultsWatcher::scan`] performs the parse+session-filter+dedup+notify portion of R-SA-099's
//! fixed processing order in one pass:
//!
//! 1. **Parse** — a result file that fails to deserialize as [`ResultFile`] is silently skipped
//!    (never deleted), matching this crate's "malformed on-disk state degrades gracefully"
//!    convention.
//! 2. **Verify session membership** — `scan_for_session` takes an explicit `belongs_to_session`
//!    predicate over [`RunId`] rather than hard-coding session-identity logic into this module
//!    (this crate keeps `cyrup-session` usage scoped to `fork_context.rs`'s narrow, purpose-built
//!    dependency per arch-SA §2.1/§6.6 — `watch.rs` has no business knowing what a "session" is).
//!    A result that does not belong to the current session is deferred/skipped WITHOUT being
//!    marked seen and WITHOUT being deleted, so a later scan (once the caller's session-tracking
//!    state catches up, or once a DIFFERENT orchestrator instance that DOES own that session polls
//!    it) can still pick it up.
//! 3. **Dedup** — an in-memory seen-set keyed by the R-SA-099-specified composite (run id / agent /
//!    timestamp) with a bounded TTL ([`DEDUP_TTL`], target ~10 minutes) guards against re-notifying
//!    for the same result twice within one orchestrator process's lifetime.
//! 4. **Notify** — the caller receives every not-yet-seen, session-matching result as a
//!    [`CompletionNotification`] and is responsible for actually delivering it (re-entering the
//!    normal turn/prompt path, R-SA-101 — see that section below for why this module stops short
//!    of performing the delivery itself).
//!
//! Deletion is **explicitly the caller's own separate act**, via [`ResultsWatcher::delete_after_notify`],
//! called only AFTER the caller's own downstream delivery has succeeded (R-SA-099: "Deletion MUST
//! happen last (after notification), accepting that a crash between notification and deletion
//! causes at most one duplicate re-notification on restart, never a lost one").
//!
//! # R-SA-100: OR'd terminal-state classification
//!
//! [`classify_outcome`] classifies a [`ResultFile`] using two independently-populated signals —
//! the explicit `state` field and the `success` flag — OR'd together rather than relying on either
//! alone, since `state == Complete && !success` (every step individually failed acceptance, but the
//! run itself finished without a run-ending crash) is a real, legitimate combination this crate's
//! own `runner_main::finish_run` can produce. A `Paused` state is NEVER reclassified as `Failed`
//! regardless of `success` (R-SA-100's explicit carve-out).
//!
//! # R-SA-101: re-enters the normal turn/prompt path (deferred to a later phase/file)
//!
//! R-SA-101 requires that consuming a [`CompletionNotification`] for chat/UI delivery "re-enter the
//! orchestrator's normal turn/prompt-handling path (not merely display inert text) so that the
//! parent LLM/agent sees and can act on the background result on its own initiative." This module
//! deliberately stops at producing the plain, inert [`CompletionNotification`] payload — actually
//! injecting that payload into a live session's turn loop requires a handle to session/agent-turn
//! machinery this crate does not hold here (per this crate's own "ZERO dependency on `cyrup-agent`,
//! and `cyrup-session` usage scoped only to `fork_context.rs`/lineage-append" boundary, arch-SA
//! §2.1/§12 item 10). **That hand-off wiring belongs to a later phase's `background/tracker.rs`
//! (R-SA-093's shared poller, which already owns per-session tracked-run bookkeeping) and/or
//! `registration/`'s extension-facade code, which hold the actual `HostCtx`/session-event-sink
//! reference** — this module supplies [`CompletionNotification`] as the exact, complete payload
//! that hand-off consumes, and nothing about its shape needs to change when that later phase is
//! implemented.
//!
//! # R-SA-102: bounded retry-in-place on transient processing failure
//!
//! A caller may fail to actually DELIVER a [`CompletionNotification`] after `scan`/`scan_for_session`
//! returns it (e.g. the later-phase turn-re-entry hand-off above hits a transient error). R-SA-102
//! says such a failure SHOULD leave the result file in place for retry on the next cycle rather than
//! losing the notification — but SHOULD NOT retry indefinitely without any bound. [`ResultsWatcher`]
//! supports this directly: [`ResultsWatcher::record_processing_failure`] lets the caller tell this
//! watcher "I saw this one but could not process it", which (a) un-marks it from the seen-set so the
//! NEXT `scan`/`scan_for_session` call re-surfaces it (retry-in-place — the file itself was never
//! touched, so nothing above needs to re-read it from disk) and (b) increments a per-key attempt
//! counter capped at [`MAX_PROCESSING_ATTEMPTS`]; once a key exceeds the bound,
//! [`ResultsWatcher::scan_for_session`] stops re-surfacing it as a normal [`CompletionNotification`]
//! and instead reports it once via [`CompletionNotification::exhausted`] so the caller can log/alert
//! rather than spinning forever on a permanently-broken result file.
//!
//! # R-SA-103: mode-agnostic mechanics
//!
//! Nothing in this module reads or branches on `interactive`/`print`/`json`/`rpc` mode — the
//! on-disk mechanics (watch, parse, dedup, classify, retry-bound) are identical regardless of which
//! mode's caller drives them; R-SA-103 only changes what SURFACES the resulting
//! [`CompletionNotification`]s (a persistent TUI widget vs. a tagged RPC event vs. nothing at all,
//! for `print`/`json`'s "a subsequent separate invocation observes results" case), which is
//! entirely the calling layer's concern, not this module's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use super::{ResultFile, RunId, RunState};
use crate::error::SubagentError;

/// Target poll-interval fallback for [`ResultsWatcher`], used when native filesystem notification
/// is unavailable or fails (R-SA-098: "a fixed-interval poll fallback... used when native
/// notification is unavailable or fails, e.g. resource-exhaustion errors"). `notify::PollWatcher`
/// is simultaneously both the native-notification-preferring path AND the poll-fallback path (see
/// module docs), so there is no separate branch to maintain.
pub const RESULTS_DIR_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Bounded TTL for the dedup seen-set (R-SA-099: "a bounded TTL (target ~10 minutes)") — after
/// this long, a given `(run_id, agent, timestamp)` composite key is evicted from the seen-set and
/// would (in the practically-unreachable case the SAME result file somehow reappeared) be treated
/// as new again, rather than the seen-set growing unbounded over a long-lived orchestrator
/// process's lifetime.
pub const DEDUP_TTL: Duration = Duration::from_secs(10 * 60);

/// Upper bound on retry-in-place attempts for a single result file before this watcher gives up
/// re-surfacing it as an ordinary notification and instead reports it as
/// [`CompletionNotification::exhausted`] (R-SA-102: "SHOULD NOT retry indefinitely without any
/// bound"). Chosen generously (well above any plausible number of transient-failure poll cycles a
/// genuinely-recoverable condition would need) since the cost of one extra retry is negligible and
/// the failure mode being guarded against is "spins forever", not "retries slightly too many
/// times".
pub const MAX_PROCESSING_ATTEMPTS: u32 = 20;

// =================================================================================================
// CompletionNotification
// =================================================================================================

/// One notified-but-not-yet-deleted result, surfaced to the caller's own turn/prompt-handling path
/// (R-SA-101: "MUST re-enter the orchestrator's normal turn/prompt-handling path"). This module
/// does not itself know how to inject a message into a live session — that is a later phase's
/// `background/tracker.rs`/`registration/` responsibility (see module docs); this type is the
/// plain, inert payload that hand-off is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionNotification {
    /// The parsed terminal result.
    pub result: ResultFile,
    /// The on-disk path this result was read from — needed by
    /// [`ResultsWatcher::delete_after_notify`] to delete the correct file.
    pub result_path: PathBuf,
    /// `true` if this notification is being surfaced only because [`MAX_PROCESSING_ATTEMPTS`] was
    /// exceeded (R-SA-102's retry bound) — the caller should treat this as a terminal
    /// "give up, log/alert" signal rather than attempting normal turn-re-entry delivery again, since
    /// every prior attempt already failed.
    pub exhausted: bool,
}

// =================================================================================================
// DedupKey / SeenEntry
// =================================================================================================

/// The dedup key R-SA-099 specifies: "a composite of run id/agent/timestamp". `agent` here is the
/// top-level [`ResultFile::agent`]; `timestamp` is the result file's own on-disk mtime (epoch
/// seconds) at the moment it was first observed, which — combined with run id and agent — is
/// stable across repeated polls of the SAME still-undeleted file (a file is never rewritten in
/// place once written, per `runner_main::finish_run`'s single write-and-done contract, so its mtime
/// does not change between polls) while still changing if a run id were ever hypothetically reused
/// for a genuinely new, later result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DedupKey {
    run_id: String,
    agent: String,
    mtime_epoch_secs: i64,
}

/// One dedup entry: when it was first inserted, for TTL eviction (R-SA-099).
struct SeenEntry {
    inserted_at: Instant,
}

/// One retry-bookkeeping entry (R-SA-102): how many times processing of this key has been
/// reported as failed via [`ResultsWatcher::record_processing_failure`], and when the FIRST such
/// failure was recorded (for the identical TTL eviction policy as [`SeenEntry`] — a key whose
/// owning result file was never actually deleted but which also stopped being polled for
/// [`DEDUP_TTL`] should not pin memory forever).
struct RetryEntry {
    attempts: u32,
    inserted_at: Instant,
}

// =================================================================================================
// ResultsWatcher
// =================================================================================================

/// The orchestrator-side `ResultsDir` watch state: an in-memory, bounded-TTL seen-set (R-SA-099)
/// guarding against re-notifying for the same result twice, plus a bounded per-key retry-attempt
/// counter (R-SA-102) tracked in a SEPARATE map so "currently suppressed as already-notified" and
/// "how many processing attempts has this key burned" are independent pieces of state — a key can
/// be absent from `seen` (so the next scan will surface it again) while still present in
/// `retry_attempts` (so that next surfacing knows it is not the FIRST attempt and can eventually
/// reach [`MAX_PROCESSING_ATTEMPTS`]). Collapsing these into one map (an earlier version of this
/// type tried that) creates a contradiction: re-inserting into a single seen-map immediately after
/// `record_processing_failure` makes the very next `scan` see the key as already-present and
/// suppress it again, which defeats retry-in-place entirely. Both maps are shared behind an
/// `Arc<AsyncMutex<..>>` internally so they can be driven from a `notify` callback (which runs on
/// an arbitrary background thread, not necessarily a `tokio` task) as well as read/polled from
/// async code.
pub struct ResultsWatcher {
    results_dir: PathBuf,
    seen: Arc<AsyncMutex<HashMap<DedupKey, SeenEntry>>>,
    retry_attempts: Arc<AsyncMutex<HashMap<DedupKey, RetryEntry>>>,
}

impl ResultsWatcher {
    /// Construct a watcher over `results_dir` (does not itself install any filesystem watch — call
    /// [`ResultsWatcher::install`] for that; this constructor is infallible and purely sets up the
    /// in-memory dedup state, so a caller can hold a [`ResultsWatcher`] value before deciding
    /// whether/when to actually attach a live watch).
    #[must_use]
    pub fn new(results_dir: PathBuf) -> Self {
        Self {
            results_dir,
            seen: Arc::new(AsyncMutex::new(HashMap::new())),
            retry_attempts: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    /// Borrows the directory this watcher is scoped to.
    #[must_use]
    pub fn results_dir(&self) -> &std::path::Path {
        &self.results_dir
    }

    /// Install a real `notify::PollWatcher` over this watcher's `results_dir` (R-SA-098),
    /// forwarding every observed filesystem event as a bare wake-up notification on the returned
    /// channel — mirroring `control::watch_control_inbox`'s identical "notify only, let the
    /// receiver re-scan" pattern, since `notify` does not guarantee events are coalesced 1:1 with
    /// actual result-file arrivals (a burst of writes can coalesce to fewer wake-ups, and a single
    /// write can sometimes surface as more than one event — the receiver is expected to always
    /// re-`scan`/`scan_for_session` on each wake-up rather than trying to interpret the event
    /// payload itself).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] if the underlying `notify` watcher cannot be constructed or
    /// attached to `results_dir` (e.g. it does not exist yet — callers are expected to `mkdir` the
    /// results directory before installing a watch on it, mirroring how `AsyncRoot`/`ResultsDir`
    /// are established once by extension initialization).
    pub fn install(
        &self,
    ) -> Result<(notify::PollWatcher, tokio::sync::mpsc::UnboundedReceiver<()>), SubagentError>
    {
        use notify::Watcher;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let cfg = notify::Config::default()
            .with_poll_interval(RESULTS_DIR_POLL_INTERVAL)
            .with_compare_contents(false);
        let mut watcher = notify::PollWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            },
            cfg,
        )
        .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

        watcher
            .watch(&self.results_dir, notify::RecursiveMode::NonRecursive)
            .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

        Ok((watcher, rx))
    }

    /// Scan `results_dir` once and return every not-yet-notified [`CompletionNotification`],
    /// deduplicated against the bounded-TTL seen-set, with **no session-membership filtering** —
    /// every discovered, not-yet-seen result under `results_dir` is returned regardless of which
    /// session/orchestrator instance minted it.
    ///
    /// Most callers should prefer [`ResultsWatcher::scan_for_session`], which additionally applies
    /// R-SA-099's "verify it belongs to the current session... else defer/skip without deleting"
    /// requirement. This unfiltered variant exists for callers that are themselves scoped to a
    /// single-session `ResultsDir` already (so every result under it inherently belongs to the
    /// current session and a predicate would be a no-op), and for tests.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] only if `results_dir` itself cannot be listed at all (e.g.
    /// it does not exist) — an individual file's own read/parse failure never aborts the scan of
    /// its siblings.
    pub async fn scan(&self) -> Result<Vec<CompletionNotification>, SubagentError> {
        self.scan_for_session(|_run_id| true).await
    }

    /// Scan `results_dir` once and return every not-yet-notified, session-matching
    /// [`CompletionNotification`], performing R-SA-099's fixed "parse; verify it belongs to the
    /// current session (else defer/skip without deleting); deduplicate... ; if not a duplicate,
    /// emit" sequence in one pass.
    ///
    /// `belongs_to_session` is called with each discovered result's [`RunId`] and must return
    /// `true` iff that run belongs to the caller's current session. A result for which it returns
    /// `false` is skipped for THIS scan — left entirely untouched on disk and NOT marked seen —
    /// so a later scan (once the caller's own session-tracking state includes that run id, or a
    /// different, correctly-scoped orchestrator instance polls the same directory) can still pick
    /// it up. This module has no built-in notion of "session" itself (this crate's `cyrup-session`
    /// usage is scoped to `fork_context.rs`/lineage-append only, per arch-SA §2.1/§12 item 10) —
    /// the predicate is the caller's own session-tracked-run-id-set membership test.
    ///
    /// A result file that fails to parse is silently skipped (never deleted, per R-SA-099's "else
    /// defer/skip without deleting" — malformed state degrades gracefully rather than being
    /// dropped on the floor).
    ///
    /// A result whose retry-attempt count (via [`ResultsWatcher::record_processing_failure`]) has
    /// exceeded [`MAX_PROCESSING_ATTEMPTS`] is surfaced exactly once more with
    /// [`CompletionNotification::exhausted`] set, and is thereafter treated as if permanently seen
    /// (R-SA-102's bound).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] only if `results_dir` itself cannot be listed at all (e.g.
    /// it does not exist) — an individual file's own read/parse failure never aborts the scan of
    /// its siblings.
    pub async fn scan_for_session(
        &self,
        belongs_to_session: impl Fn(&RunId) -> bool,
    ) -> Result<Vec<CompletionNotification>, SubagentError> {
        self.evict_expired().await;

        let mut entries = match tokio::fs::read_dir(&self.results_dir).await {
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
            let Ok(metadata) = entry.metadata().await else {
                continue; // transient stat failure: skip this cycle, never delete.
            };
            let mtime_epoch_secs = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
                .unwrap_or(0);

            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue; // transient read failure: skip this cycle, never delete.
            };
            let Ok(result) = serde_json::from_slice::<ResultFile>(&bytes) else {
                continue; // malformed result file: skip, never delete (never silently lost).
            };

            if !belongs_to_session(&result.run_id) {
                // R-SA-099: "verify it belongs to the current session (else defer/skip without
                // deleting)" — do not mark seen, do not delete; a later, correctly-scoped scan may
                // still pick this up.
                continue;
            }

            let key = DedupKey {
                run_id: result.run_id.as_str().to_string(),
                agent: result.agent.clone(),
                mtime_epoch_secs,
            };

            match self.check_and_mark_seen(&key).await {
                SeenOutcome::AlreadyNotified => continue,
                SeenOutcome::Exhausted => {
                    found.push(CompletionNotification {
                        result,
                        result_path: path,
                        exhausted: true,
                    });
                }
                SeenOutcome::NewlySeen => {
                    found.push(CompletionNotification {
                        result,
                        result_path: path,
                        exhausted: false,
                    });
                }
            }
        }

        Ok(found)
    }

    /// Delete a result file whose [`CompletionNotification`] has ALREADY been successfully
    /// delivered to the caller's own downstream notification path (R-SA-099: "Deletion MUST happen
    /// last (after notification), accepting that a crash between notification and deletion causes
    /// at most one duplicate re-notification on restart, never a lost one"). Never call this before
    /// the notification has actually been delivered — a caller that deletes first and then fails to
    /// deliver would violate R-SA-099's own explicit ordering rationale. Also clears any retry-bound
    /// bookkeeping for this result, since it is now gone from disk and cannot be re-scanned.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] only for a genuine I/O failure OTHER than the file already
    /// being absent (a concurrent second watcher, or a prior successful delete, racing this one) —
    /// a `NotFound` on delete is treated as an already-achieved goal, not an error, matching this
    /// crate's general delete-then-act idempotency convention.
    pub async fn delete_after_notify(
        &self,
        notification: &CompletionNotification,
    ) -> Result<(), SubagentError> {
        // Read the mtime BEFORE deleting: once the file is gone, `mtime_epoch_secs_of` can no
        // longer recover the value the dedup/retry keys were built from (the read would return
        // `None`), which would make the cleanup below silently miss the real entry.
        let mtime_epoch_secs = mtime_epoch_secs_of(&notification.result_path).await;

        let outcome = match tokio::fs::remove_file(&notification.result_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SubagentError::Spawn(e)),
        };
        if outcome.is_ok() {
            // The file is gone: no future scan can ever re-read it, so its retry-attempt
            // bookkeeping (if any — most results are never retried at all) is now dead weight.
            // Clearing it here rather than waiting for TTL eviction keeps the retry map's size
            // bounded by "currently-failing results", not "every result that ever failed once".
            // If the mtime could not be read (file already gone before this call, e.g. a
            // concurrent double-delete), fall back to `0`, matching every other mtime-read
            // failure's degrade-gracefully convention in this module — the entry this would have
            // targeted, if any, is simply left to expire via [`DEDUP_TTL`] instead.
            let key = DedupKey {
                run_id: notification.result.run_id.as_str().to_string(),
                agent: notification.result.agent.clone(),
                mtime_epoch_secs: mtime_epoch_secs.unwrap_or(0),
            };
            self.retry_attempts.lock().await.remove(&key);
        }
        outcome
    }

    /// R-SA-102: tell this watcher that downstream processing of an already-returned
    /// [`CompletionNotification`] failed transiently, so the underlying result file should be
    /// retried on the next scan rather than treated as permanently delivered. This un-marks the
    /// result's dedup key (the next `scan`/`scan_for_session` call will re-surface it, since the
    /// file itself was never deleted) and increments its attempt counter, bounded at
    /// [`MAX_PROCESSING_ATTEMPTS`] — once exceeded, the NEXT scan surfaces it one final time with
    /// [`CompletionNotification::exhausted`] set and stops retrying it thereafter (this method is a
    /// no-op for a key that has already reached the exhausted state, so a caller cannot
    /// accidentally resurrect infinite retries by calling this repeatedly against an exhausted key).
    pub async fn record_processing_failure(&self, notification: &CompletionNotification) {
        let key = DedupKey {
            run_id: notification.result.run_id.as_str().to_string(),
            agent: notification.result.agent.clone(),
            mtime_epoch_secs: mtime_epoch_secs_of(&notification.result_path)
                .await
                .unwrap_or(0),
        };

        // Un-mark from the dedup seen-set: this is the entire "retry-in-place" mechanism — the
        // next `scan`/`scan_for_session` call will treat this key as unseen again and re-surface
        // it, since the underlying result file itself was never touched. Kept as a SEPARATE map
        // from the attempt counter below (see [`ResultsWatcher`]'s own doc note on why merging
        // them is a bug: re-inserting into a single seen-map here would make the very next scan
        // see the key as already-present and suppress it again).
        self.seen.lock().await.remove(&key);

        let mut retry_attempts = self.retry_attempts.lock().await;
        let attempts = retry_attempts
            .get(&key)
            .map_or(0, |entry| entry.attempts)
            .saturating_add(1);
        retry_attempts.insert(key, RetryEntry { attempts, inserted_at: Instant::now() });
        // R-SA-102's bound itself is enforced on the READ side, by `check_and_mark_seen` comparing
        // this stored `attempts` count against `MAX_PROCESSING_ATTEMPTS` the next time this key is
        // scanned — this method's own job is only to record that one more failure happened.
    }

    async fn check_and_mark_seen(&self, key: &DedupKey) -> SeenOutcome {
        let mut seen = self.seen.lock().await;
        if seen.contains_key(key) {
            return SeenOutcome::AlreadyNotified;
        }

        let prior_attempts = self
            .retry_attempts
            .lock()
            .await
            .get(key)
            .map_or(0, |entry| entry.attempts);

        seen.insert(key.clone(), SeenEntry { inserted_at: Instant::now() });

        if prior_attempts >= MAX_PROCESSING_ATTEMPTS {
            SeenOutcome::Exhausted
        } else {
            SeenOutcome::NewlySeen
        }
    }

    /// Evict every seen-set/retry-attempt entry older than [`DEDUP_TTL`] (R-SA-099's bounded-TTL
    /// requirement, extended identically to the retry-attempt map) — called at the top of every
    /// scan so neither map grows without bound across a long-lived orchestrator process's
    /// lifetime.
    async fn evict_expired(&self) {
        let now = Instant::now();
        self.seen
            .lock()
            .await
            .retain(|_, entry| now.duration_since(entry.inserted_at) < DEDUP_TTL);
        self.retry_attempts
            .lock()
            .await
            .retain(|_, entry| now.duration_since(entry.inserted_at) < DEDUP_TTL);
    }
}

/// Outcome of checking-then-marking a dedup key as seen — distinguishes "brand new" from "already
/// notified, suppress" from "retry-bound exceeded, surface one last time as exhausted".
enum SeenOutcome {
    NewlySeen,
    AlreadyNotified,
    Exhausted,
}

/// Reads a path's on-disk mtime as epoch seconds, or `None` if the file is gone/unreadable (e.g.
/// deleted concurrently between the caller's original scan and its later
/// [`ResultsWatcher::record_processing_failure`] call) — never a panic on a missing file.
async fn mtime_epoch_secs_of(path: &std::path::Path) -> Option<i64> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
}

// =================================================================================================
// classify_outcome (R-SA-100)
// =================================================================================================

/// R-SA-100: classify a [`ResultFile`] as completed/failed/paused using at least two
/// independently-populated OR'd signals — here, the explicit `state` field OR'd against the
/// `success`/exit-indicator-derived signal, since `state == Complete && !success` (every step
/// individually failed acceptance, but the run itself finished without a run-ending crash) is a
/// real, legitimate combination `runner_main::finish_run` can produce, and a classifier that
/// looked at ONLY `state` would misreport that case as unconditionally "completed" when a caller
/// may specifically want to distinguish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifiedOutcome {
    Completed,
    Failed,
    Paused,
}

/// Classify `result` per R-SA-100 — a `Paused` state is NEVER reclassified as `Failed` regardless
/// of `success` (R-SA-100: "a paused (interrupted) run MUST NOT be classified as failed" —
/// `success` is not a meaningful signal for a `Paused` run in `runner_main::finish_run`'s own
/// construction, since a paused run's `success` is computed from whatever partial `results` had
/// accumulated before the interrupt, which is not the signal this classification cares about).
#[must_use]
pub fn classify_outcome(result: &ResultFile) -> ClassifiedOutcome {
    match result.state {
        RunState::Paused => ClassifiedOutcome::Paused,
        RunState::Complete if result.success => ClassifiedOutcome::Completed,
        RunState::Complete => ClassifiedOutcome::Failed, // state says done, success says no
        RunState::Failed => ClassifiedOutcome::Failed,
        RunState::Queued | RunState::Running => {
            // Should not occur for a genuinely terminal ResultFile (finish_run only ever writes
            // Complete/Failed/Paused) — classified as Failed defensively rather than panicking or
            // returning an Option, since a caller consuming this classification has no sane
            // "still running" bucket to put a RESULT FILE'S OWN state into (its very presence
            // already means the run reached SOME terminal write).
            ClassifiedOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::RunMode;
    use crate::background::atomic::write_atomic_json;

    fn temp_results_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let results_dir = dir.path().join("results");
        (dir, results_dir)
    }

    fn sample_result(run_id: &str, state: RunState, success: bool) -> ResultFile {
        ResultFile {
            id: RunId::from_token(run_id),
            run_id: RunId::from_token(run_id),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state,
            success,
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            results: Vec::new(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // scan: R-SA-098/099 basic detection + dedup
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn scan_finds_a_freshly_written_result_file() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00001", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00001.json"), &result)
            .await
            .expect("write result");

        let watcher = ResultsWatcher::new(results_dir);
        let found = watcher.scan().await.expect("scan succeeds");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].result.run_id, result.run_id);
        assert!(!found[0].exhausted);
    }

    #[tokio::test]
    async fn scan_does_not_renotify_an_already_seen_result() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00002", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00002.json"), &result)
            .await
            .expect("write result");

        let watcher = ResultsWatcher::new(results_dir);
        let first = watcher.scan().await.expect("first scan");
        assert_eq!(first.len(), 1);

        // The file is still on disk (not deleted) but a second scan without an intervening
        // `delete_after_notify` must NOT re-surface it (R-SA-099 dedup).
        let second = watcher.scan().await.expect("second scan");
        assert!(
            second.is_empty(),
            "a second scan before deletion must not re-notify for the same result"
        );
    }

    #[tokio::test]
    async fn scan_for_session_defers_a_result_that_does_not_belong_to_the_session() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00009", RunState::Complete, true);
        let path = results_dir.join("run00009.json");
        write_atomic_json(&path, &result).await.expect("write result");

        let watcher = ResultsWatcher::new(results_dir);

        // A predicate that rejects every run id: the result must be skipped, NOT deleted, NOT
        // marked seen.
        let found = watcher
            .scan_for_session(|_| false)
            .await
            .expect("scan succeeds");
        assert!(
            found.is_empty(),
            "a result outside the current session must not be notified"
        );
        assert!(
            path.exists(),
            "a deferred (not-yet-owned) result must not be deleted"
        );

        // Once the predicate recognizes the run id (simulating the caller's session-tracking
        // catching up), the very same still-on-disk file must now be picked up.
        let found_second = watcher
            .scan_for_session(|id| id.as_str() == "run00009")
            .await
            .expect("scan succeeds");
        assert_eq!(
            found_second.len(),
            1,
            "a previously-deferred result must be pickable up once the session predicate matches"
        );
    }

    #[tokio::test]
    async fn delete_after_notify_removes_the_file_and_tolerates_a_double_delete() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00003", RunState::Complete, true);
        let path = results_dir.join("run00003.json");
        write_atomic_json(&path, &result).await.expect("write result");

        let watcher = ResultsWatcher::new(results_dir);
        let found = watcher.scan().await.expect("scan");
        watcher
            .delete_after_notify(&found[0])
            .await
            .expect("delete succeeds");
        assert!(!path.exists());

        // A second delete of the same (now-absent) file must not error (idempotent).
        watcher
            .delete_after_notify(&found[0])
            .await
            .expect("double delete does not error");
    }

    #[tokio::test]
    async fn malformed_result_file_is_skipped_not_deleted() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let path = results_dir.join("garbage.json");
        tokio::fs::write(&path, b"not valid json").await.expect("write garbage");

        let watcher = ResultsWatcher::new(results_dir);
        let found = watcher.scan().await.expect("scan does not error on a malformed sibling");
        assert!(found.is_empty());
        assert!(
            path.exists(),
            "a malformed result file must be left in place, never silently deleted"
        );
    }

    #[tokio::test]
    async fn non_json_sibling_files_are_ignored() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        tokio::fs::write(results_dir.join("README.md"), b"not a result")
            .await
            .expect("write sibling");

        let watcher = ResultsWatcher::new(results_dir);
        let found = watcher.scan().await.expect("scan succeeds");
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn scan_over_missing_directory_returns_empty_not_error() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let missing = dir.path().join("does-not-exist-yet");

        let watcher = ResultsWatcher::new(missing);
        let found = watcher.scan().await.expect("missing dir is not an error");
        assert!(found.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-102: bounded retry-in-place on transient processing failure
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn record_processing_failure_makes_the_result_reappear_on_the_next_scan() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00010", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00010.json"), &result)
            .await
            .expect("write result");

        let watcher = ResultsWatcher::new(results_dir);
        let first = watcher.scan().await.expect("first scan");
        assert_eq!(first.len(), 1);

        // Without recording a failure, a second scan sees nothing (normal dedup).
        let second = watcher.scan().await.expect("second scan");
        assert!(second.is_empty());

        // Simulate the caller failing to process the notification (retry-in-place, R-SA-102).
        watcher.record_processing_failure(&first[0]).await;

        let third = watcher.scan().await.expect("third scan after recorded failure");
        assert_eq!(
            third.len(),
            1,
            "a result whose processing failed must be retried on the next scan"
        );
        assert!(!third[0].exhausted, "still well under the retry bound");
    }

    #[tokio::test]
    async fn processing_failure_bound_eventually_marks_the_result_exhausted() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00011", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00011.json"), &result)
            .await
            .expect("write result");

        let watcher = ResultsWatcher::new(results_dir);

        // Drive scan -> record_processing_failure in a loop past the bound; the result must
        // eventually be surfaced with `exhausted: true` and never spin forever.
        let mut last_exhausted = false;
        for _ in 0..(MAX_PROCESSING_ATTEMPTS + 5) {
            let found = watcher.scan().await.expect("scan");
            let Some(notification) = found.into_iter().next() else {
                continue;
            };
            last_exhausted = notification.exhausted;
            watcher.record_processing_failure(&notification).await;
        }

        assert!(
            last_exhausted,
            "R-SA-102: processing that fails indefinitely must eventually be reported exhausted, \
             not retried without bound"
        );
    }

    // ---------------------------------------------------------------------------------------
    // classify_outcome: R-SA-100 OR'd signal classification
    // ---------------------------------------------------------------------------------------

    #[test]
    fn classify_outcome_paused_is_never_failed_regardless_of_success_flag() {
        let paused = sample_result("run00004", RunState::Paused, false);
        assert_eq!(classify_outcome(&paused), ClassifiedOutcome::Paused);
        let paused_success_true = sample_result("run00005", RunState::Paused, true);
        assert_eq!(classify_outcome(&paused_success_true), ClassifiedOutcome::Paused);
    }

    #[test]
    fn classify_outcome_complete_with_success_false_is_failed_not_completed() {
        let result = sample_result("run00006", RunState::Complete, false);
        assert_eq!(classify_outcome(&result), ClassifiedOutcome::Failed);
    }

    #[test]
    fn classify_outcome_complete_with_success_true_is_completed() {
        let result = sample_result("run00007", RunState::Complete, true);
        assert_eq!(classify_outcome(&result), ClassifiedOutcome::Completed);
    }

    #[test]
    fn classify_outcome_failed_state_is_always_failed() {
        let result = sample_result("run00012", RunState::Failed, false);
        assert_eq!(classify_outcome(&result), ClassifiedOutcome::Failed);
    }

    // ---------------------------------------------------------------------------------------
    // install: real notify::PollWatcher against a real tempdir
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_observes_a_real_filesystem_write() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");

        let watcher = ResultsWatcher::new(results_dir.clone());
        let (_native_watcher, mut rx) = watcher.install().expect("watcher installs");

        let result = sample_result("run00008", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00008.json"), &result)
            .await
            .expect("write result");

        let notified = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(
            notified.is_ok() && notified.expect("timeout checked above").is_some(),
            "a real filesystem write to ResultsDir must be observed by the watcher"
        );
    }

    /// The load-bearing test this task calls for: a real `notify` watcher against a real tempdir,
    /// writing a real [`ResultFile`], asserting the watcher fires exactly once (dedup) even under a
    /// simulated duplicate-event scenario.
    ///
    /// `notify` (and file systems generally) offer no guarantee of exactly-one-event-per-write —
    /// a single atomic `write_atomic_json` (temp-write + rename) can itself surface as more than
    /// one raw OS-level filesystem event, and a `PollWatcher`'s own poll tick can independently
    /// observe the same still-new file across more than one tick before it is deleted. This test
    /// deliberately drains EVERY wake-up the watcher produces within a bounded settle window
    /// (simulating however many duplicate raw events a real backend might coalesce or fail to
    /// coalesce) and, for EACH wake-up, re-scans `results_dir` — proving that no matter how many
    /// raw notify events a single result-file write generates, [`ResultsWatcher::scan`]'s own
    /// seen-set dedup (R-SA-099) still yields the result as a [`CompletionNotification`] EXACTLY
    /// ONCE across the whole sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_fires_exactly_once_under_duplicate_filesystem_events() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");

        let watcher = ResultsWatcher::new(results_dir.clone());
        let (_native_watcher, mut rx) = watcher.install().expect("watcher installs");

        let result = sample_result("run00099", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00099.json"), &result)
            .await
            .expect("write result");

        // Drain every wake-up the watcher produces within a bounded settle window. A real
        // PollWatcher backend may emit more than one raw event for a single logical write (the
        // temp-write, the rename, and/or more than one poll tick observing the same new file
        // before anything consumes it) — that is exactly the "simulated duplicate-event scenario"
        // this test exercises, using genuine OS/filesystem-driven duplication rather than a
        // hand-rolled fake.
        let mut wake_ups = 0u32;
        let mut total_notifications: Vec<CompletionNotification> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(())) => {
                    wake_ups += 1;
                    let found = watcher.scan().await.expect("scan succeeds on each wake-up");
                    total_notifications.extend(found);
                }
                Ok(None) => break, // channel closed (watcher dropped)
                Err(_) => break,   // settle-window timeout: no more events arriving
            }
        }

        assert!(
            wake_ups >= 1,
            "the watcher must have observed at least one real filesystem event"
        );
        assert_eq!(
            total_notifications.len(),
            1,
            "exactly one CompletionNotification must be produced across the ENTIRE sequence of \
             (possibly duplicate) wake-ups, proving R-SA-099 dedup absorbs duplicate filesystem \
             events rather than re-notifying per raw event: got {total_notifications:?} across \
             {wake_ups} wake-up(s)"
        );
        assert_eq!(total_notifications[0].result.run_id, result.run_id);

        // One more explicit scan (simulating the shared poller's next scheduled tick, independent
        // of any further filesystem event) must still find nothing new — dedup holds even after
        // the event stream has quieted down, not just across the immediate burst.
        let after = watcher.scan().await.expect("post-settle scan");
        assert!(
            after.is_empty(),
            "a scan after the event burst has settled must not re-notify"
        );
    }
}
