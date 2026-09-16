//! What one retention pass did, and the durable record it leaves behind.
//!
//! Ports pi `AsyncRetentionResult` (`:103-119`), `increment` (`:536-538`), `compactError`
//! (`:125-128`) and `appendMaintenanceLog` (`:513-534`).
//!
//! # The report is the only thing anyone outside the sweep ever sees
//!
//! [`cleanup_async_retention`](super::cleanup_async_retention) never returns a
//! [`std::io::Result`]: upstream returns the report unconditionally (`:650`, `:688-693`) and every
//! per-candidate failure is captured into [`AsyncRetentionResult::errors`] (`:836`, `:894`). This
//! is [`crate::registration::doctor::DoctorRunner::run`]'s own "no `Result` return type at all"
//! discipline — a reaper that refuses to report because one directory was unreadable is worse than
//! one that reports the unreadable directory.
//!
//! # And the maintenance log is the only thing anyone in ANOTHER PROCESS ever sees
//!
//! The pass runs detached, 60 s after a session installs its completion watcher
//! (`extension/executor/notices.rs`'s schedule). Nothing is awaiting it and nothing holds
//! a handle to its report. [`append_maintenance_log`] is therefore not telemetry — it is the
//! ONLY channel by which `/subagents-doctor` (a different process, minutes or days later) can
//! answer "is retention running, and is it healthy". [`read_last_maintenance_line`] is the reader
//! half, and [`crate::registration::doctor::AsyncRetentionDoctor`] is its one production consumer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// pi `LOG_NAME` (`:22`) — `<maintenance_root>/async-retention-maintenance.jsonl`.
pub const MAINTENANCE_LOG_NAME: &str = "async-retention-maintenance.jsonl";

/// The byte budget one generation of the maintenance log may reach before [`append_maintenance_log`]
/// rotates it, so the whole record is bounded at `2 × MAINTENANCE_LOG_CAP_BYTES`.
///
/// # [CYRUP-DELTA] upstream's log is UNBOUNDED, and this module exists to bound things
///
/// `appendMaintenanceLog` (`:513-534`) is a bare `fs.appendFileSync` into a shared directory that
/// nothing ever truncates. An uncapped append inside the very tree this task exists to keep
/// bounded is self-defeating, so cyrup caps it.
///
/// # Why rotation and not [`crate::jsonl::BoundedJsonlWriter`] alone
///
/// That primitive is the crate's append-with-cap writer and it is used here — but its cap
/// behaviour is "stop writing" (`jsonl.rs`: *"once the budget is reached, further `write_line`
/// calls become a silent no-op"*), which is exactly right for `events.jsonl`, where the PREFIX is
/// the record. Here the **last** line is the whole answer: a writer that froze at the cap would
/// freeze `/subagents-doctor`'s view of retention at whatever the pass six months ago reported,
/// with no way to tell a stale answer from a current one. So the cap is enforced by rotating one
/// generation aside (`<name>.1`) and starting a fresh file — bounded, and never stale.
///
/// 1 MiB is ~2,500 passes at the ~400-byte line this module writes; at one pass per session start
/// that is a long, and still bounded, history.
pub const MAINTENANCE_LOG_CAP_BYTES: u64 = 1024 * 1024;

/// pi `compactError`'s `slice(0, 240)` (`:127`), in **bytes on a `char` boundary** rather than
/// UTF-16 code units.
pub const MAX_ERROR_BYTES: usize = 240;

/// What one pass of [`cleanup_async_retention`](super::cleanup_async_retention) did.
///
/// pi `AsyncRetentionResult` (`:103-119`). Fourteen upstream fields; the cyrup deltas are called
/// out on the fields themselves.
///
/// `skipped` is a [`BTreeMap`] and not a `HashMap` **on purpose**: this value is serialised into
/// the maintenance log and rendered into a doctor report, and both must be byte-deterministic for
/// the same pass — the same reason [`crate::registration::doctor::DoctorReport`] fixes its check
/// order rather than letting completion order decide it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncRetentionResult {
    /// pi `acquired` (`:104`) — did this pass hold the cross-instance lock? `false` means another
    /// instance in this same shared root was sweeping and this pass did nothing.
    pub acquired: bool,
    /// pi `scanned` (`:105`) — candidates **considered**, incremented BEFORE any fallible work
    /// (`:758`). Not candidates successfully processed; see [`Self::errored_candidates`].
    pub scanned: usize,
    /// pi `repairedRuns` (`:106`) — runs whose `status.json` still claimed `Running` and that
    /// [`crate::background::reconcile::reconcile_now`] repaired on the way past.
    pub repaired_runs: usize,
    /// pi `deletedRuns` (`:107`) — run trees retired (renamed onto a tombstone) and deleted in
    /// this pass.
    pub deleted_runs: usize,
    /// pi `deletedResults` (`:108`).
    ///
    /// **[CYRUP-DELTA] always `0`.** The results half of upstream's sweep (`:839-896`) is not this
    /// module's: `<results_dir>/result-index/` belongs to
    /// [`crate::background::result_index::cleanup_result_indexes`] and
    /// `<results_dir>/completion-replay/` to
    /// [`crate::background::completion_replay::cleanup_completion_replay_if_due`], both of which
    /// already run — on the SAME schedule, two stages earlier
    /// (`extension/executor/notices.rs`). The field is kept rather than dropped so the
    /// maintenance log's shape stays upstream's and a reader can tell "zero because nothing was
    /// due" from "absent because this build has no such concept".
    pub deleted_results: usize,
    /// pi `reapedTombstones` (`:109`) — `.deleting-run-*` trees left by an earlier, interrupted
    /// pass that this pass completed.
    pub reaped_tombstones: usize,
    /// **[CYRUP-DELTA], and the field that makes the report checkable.**
    ///
    /// Candidates that exited the per-candidate loop through its error arm (`:834-837`).
    /// Upstream counts them nowhere, which is precisely why its own report does not balance: `:758`
    /// increments `scanned` before the `try`, and the `catch` increments neither a `skipped` bucket
    /// nor a deleted counter. With this field the accounting identity holds exactly:
    ///
    /// ```text
    /// scanned == (skip counts raised inside the candidate loop)
    ///            + deleted_runs + reaped_tombstones + errored_candidates
    /// ```
    ///
    /// Note it counts candidates, not [`Self::errors`] entries: an "absent" fault is not reported
    /// (`:836` filters `ENOENT`) but the candidate still left the loop that way, so the identity
    /// needs it counted.
    pub errored_candidates: usize,
    /// pi `skipped` (`:110`) — reason → count. The run-side reasons are
    /// [`super::SkipReason::as_str`]'s closed vocabulary; the sweep-side ones
    /// ([`super::sweep::skip_reasons`]) are the pass-level outcomes that belong to no single
    /// candidate.
    pub skipped: BTreeMap<String, usize>,
    /// pi `errors` (`:111`) — each already through [`compact_error`], so one pathological
    /// `io::Error` cannot make the log line unbounded.
    pub errors: Vec<String>,
    /// pi `rawReads` (`:112`) — every entry the async-root listing returned, including the ones
    /// the usability filter rejected. The honest cost of the pass, and the number that shows the
    /// budget bounds the OUTPUT and not the read.
    pub raw_reads: usize,
    /// pi `sourceExhausted` (`:113`), keyed `"runs"` as upstream keys it (`worker.mjs`'s
    /// `record("runs", runScan)`).
    ///
    /// **[CYRUP-DELTA] on the meaning, because upstream's is vacuous.** `streamDirWindow` returns
    /// `exhausted: true` on every path it can return at all, so upstream's value carries no
    /// information. cyrup reports the property a reader actually wants and that this pass can
    /// answer: `true` when the whole name order was covered this pass (the window wrapped, or came
    /// back under budget), `false` when the budget cut it short and there is more next pass. That
    /// is what distinguishes "nothing to do" from "more to do".
    pub source_exhausted: BTreeMap<String, bool>,
    /// pi `discoveryDurationMs` (`:114`) — the scan.
    ///
    /// Measured with [`std::time::Instant`], never a difference of
    /// [`crate::time::now_epoch_millis`] calls: upstream needs `Math.max(0, …)` (`:689-690`)
    /// because a wall clock can step backwards mid-pass, and a monotonic clock removes the need
    /// rather than clamping after the fact.
    pub discovery_duration_ms: u64,
    /// pi `commitDurationMs` (`:115`) — the destructive half, from the first candidate to the
    /// cursor write.
    pub commit_duration_ms: u64,
    /// pi `cancelled` (`:116`) — the pass observed its [`cyrup_core::CancelToken`] fired and
    /// stopped at a checkpoint.
    ///
    /// Real, not a placeholder: [`crate::extension::SubagentExecutor::stop_completion_watcher`]
    /// fires that token before aborting the task, so a session teardown that lands mid-pass gets a
    /// clean stop at the next checkpoint rather than only a dropped future.
    pub cancelled: bool,
    /// pi `durationMs` (`:118`) — the whole call, [`std::time::Instant`]-measured.
    pub duration_ms: u64,
}

impl AsyncRetentionResult {
    /// The accounting identity of this report's own doc, as a predicate.
    ///
    /// Sums only the skip reasons raised INSIDE the candidate loop: the pass-level reasons
    /// ([`super::sweep::skip_reasons`]) are raised before or after it and are deliberately
    /// excluded — counting them would make the identity fail on exactly the passes that did
    /// nothing.
    #[must_use]
    pub fn accounts_for_every_candidate(&self) -> bool {
        let skipped_in_loop: usize = self
            .skipped
            .iter()
            .filter(|(reason, _)| !super::sweep::is_pass_level_skip(reason))
            .map(|(_, count)| *count)
            .sum();
        self.scanned
            == skipped_in_loop
                .saturating_add(self.deleted_runs)
                .saturating_add(self.reaped_tombstones)
                .saturating_add(self.errored_candidates)
    }
}

/// pi `increment` (`:536-538`) — `record[key] = (record[key] ?? 0) + 1`.
pub fn increment(record: &mut BTreeMap<String, usize>, key: impl Into<String>) {
    *record.entry(key.into()).or_default() += 1;
}

/// pi `compactError` (`:125-128`) — collapse every run of whitespace to one space, then bound the
/// result at [`MAX_ERROR_BYTES`].
///
/// The bound is in **bytes, cut on a `char` boundary**. Upstream's `slice(0, 240)` counts UTF-16
/// code units; `&s[..240]` in Rust would panic on a multi-byte boundary and this crate denies
/// `clippy::indexing_slicing` for that exact reason, so the cut walks [`str::char_indices`].
///
/// The bound is not cosmetic: the report is serialised into an append-only log, so an unbounded
/// error string is an unbounded file.
#[must_use]
pub fn compact_error(error: &std::io::Error) -> String {
    compact_message(&error.to_string())
}

/// [`compact_error`] over an already-rendered message — the arm used for faults that are not an
/// [`std::io::Error`] (a `serde_json` failure, say).
#[must_use]
pub fn compact_message(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed
        .char_indices()
        .take_while(|(index, ch)| index + ch.len_utf8() <= MAX_ERROR_BYTES)
        .last()
    {
        Some((index, ch)) => collapsed
            .get(..index + ch.len_utf8())
            .unwrap_or_default()
            .to_string(),
        None => String::new(),
    }
}

/// One line of `async-retention-maintenance.jsonl` — pi's object literal at `:514-533`, verbatim
/// except for the two fields the port does not have (`workerFailed`, there being no worker) and
/// the one it adds ([`AsyncRetentionResult::errored_candidates`]).
///
/// `errors` is a **count**, not the strings: upstream logs `result.errors.length` (`:527`) and the
/// strings stay in the in-memory report. cyrup keeps the first one as well, because the log is the
/// ONLY thing `/subagents-doctor` can read and a bare count gives an operator nothing to act on —
/// already bounded by [`compact_error`], so the line stays bounded too.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceLogLine {
    /// pi `at: new Date(now).toISOString()` — recorded here as the epoch millis the pass was
    /// handed, because this crate has no ISO-8601 formatter and inventing one to re-parse it in
    /// the doctor would be two conversions where zero are needed.
    pub at: i64,
    /// [`AsyncRetentionResult::acquired`].
    pub acquired: bool,
    /// [`AsyncRetentionResult::scanned`].
    pub scanned: usize,
    /// [`AsyncRetentionResult::repaired_runs`].
    pub repaired_runs: usize,
    /// [`AsyncRetentionResult::deleted_runs`].
    pub deleted_runs: usize,
    /// [`AsyncRetentionResult::deleted_results`].
    pub deleted_results: usize,
    /// [`AsyncRetentionResult::reaped_tombstones`].
    pub reaped_tombstones: usize,
    /// [`AsyncRetentionResult::errored_candidates`].
    pub errored_candidates: usize,
    /// [`AsyncRetentionResult::skipped`].
    pub skipped: BTreeMap<String, usize>,
    /// pi `deleted: deletedIds.slice(0, ASYNC_RETENTION_BATCH_SIZE)` (`:522`) — `run:<id>` /
    /// `run-tombstone:<id>` tokens, bounded by the batch size so one line cannot outgrow one pass.
    pub deleted: Vec<String>,
    /// pi `errors: result.errors.length` (`:527`).
    pub errors: usize,
    /// The first compacted error of the pass, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
    /// [`AsyncRetentionResult::raw_reads`].
    pub raw_reads: usize,
    /// [`AsyncRetentionResult::source_exhausted`].
    pub source_exhausted: BTreeMap<String, bool>,
    /// [`AsyncRetentionResult::discovery_duration_ms`].
    pub discovery_duration_ms: u64,
    /// [`AsyncRetentionResult::commit_duration_ms`].
    pub commit_duration_ms: u64,
    /// [`AsyncRetentionResult::cancelled`].
    pub cancelled: bool,
    /// [`AsyncRetentionResult::duration_ms`].
    pub duration_ms: u64,
}

impl MaintenanceLogLine {
    /// The line a finished pass writes.
    #[must_use]
    pub fn from_result(result: &AsyncRetentionResult, now: i64, deleted_ids: &[String]) -> Self {
        Self {
            at: now,
            acquired: result.acquired,
            scanned: result.scanned,
            repaired_runs: result.repaired_runs,
            deleted_runs: result.deleted_runs,
            deleted_results: result.deleted_results,
            reaped_tombstones: result.reaped_tombstones,
            errored_candidates: result.errored_candidates,
            skipped: result.skipped.clone(),
            deleted: deleted_ids
                .iter()
                .take(super::ASYNC_RETENTION_BATCH_SIZE)
                .cloned()
                .collect(),
            errors: result.errors.len(),
            first_error: result.errors.first().cloned(),
            raw_reads: result.raw_reads,
            source_exhausted: result.source_exhausted.clone(),
            discovery_duration_ms: result.discovery_duration_ms,
            commit_duration_ms: result.commit_duration_ms,
            cancelled: result.cancelled,
            duration_ms: result.duration_ms,
        }
    }
}

/// `<maintenance_root>/async-retention-maintenance.jsonl`.
#[must_use]
pub fn maintenance_log_path(maintenance_root: &Path) -> PathBuf {
    maintenance_root.join(MAINTENANCE_LOG_NAME)
}

/// The rotated generation, `<log>.1`.
#[must_use]
fn rotated_log_path(maintenance_root: &Path) -> PathBuf {
    maintenance_root.join(format!("{MAINTENANCE_LOG_NAME}.1"))
}

/// pi `appendMaintenanceLog` (`:513-534`) — one JSON line per pass, written **unconditionally**
/// from `finish()` (`:691`), including on the `lock-busy` early return (`:703`).
///
/// That unconditionality is the point: a pass that could not acquire the lock, or that bailed on
/// unreadable wait subscriptions, is exactly the pass an operator needs to see, and it is the one a
/// "log only when something happened" writer would hide.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the maintenance root cannot be created, if the
/// rotation rename fails for a reason other than the file being absent, or if the append fails.
/// The sweep treats a failure here as non-fatal — the report it returns is unaffected.
pub async fn append_maintenance_log(
    maintenance_root: &Path,
    result: &AsyncRetentionResult,
    now: i64,
    deleted_ids: &[String],
) -> std::io::Result<()> {
    crate::background::ensure_accessible_dir(maintenance_root).await?;
    let path = maintenance_log_path(maintenance_root);

    // Rotate BEFORE the append, so the cap bounds the file this pass writes into rather than the
    // one it already overflowed.
    if let Ok(metadata) = tokio::fs::metadata(&path).await
        && metadata.len() >= MAINTENANCE_LOG_CAP_BYTES
    {
        match tokio::fs::rename(&path, rotated_log_path(maintenance_root)).await {
            Ok(()) => {}
            Err(error) if crate::background::result_index::errno::is_absent(&error) => {}
            Err(error) => return Err(error),
        }
    }

    let line = serde_json::to_string(&MaintenanceLogLine::from_result(result, now, deleted_ids))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut writer =
        crate::jsonl::BoundedJsonlWriter::create_with_cap(&path, MAINTENANCE_LOG_CAP_BYTES).await?;
    writer.write_line(&line).await
}

/// What [`read_last_maintenance_line`] found. **Three** outcomes, for
/// [`super::TombstoneMarkerState`]'s reason: "there is no log yet" (a fresh install) and "there is
/// a log I cannot read" are different conditions and the doctor renders them differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaintenanceLogRead {
    /// No maintenance root, no log file, or a log with no complete line in it yet.
    Absent,
    /// The log exists but its last line does not parse — a record written by a NEWER build, a
    /// half-written line from a process killed mid-append, or a corrupt file.
    Unreadable,
    /// The last complete line of the log.
    Line(Box<MaintenanceLogLine>),
}

/// The last line of `<maintenance_root>/async-retention-maintenance.jsonl`.
///
/// The reader half of [`append_maintenance_log`] and the ONLY route by which a later process — in
/// practice `/subagents-doctor`, via [`crate::registration::doctor::AsyncRetentionDoctor`] — can
/// see what retention has been doing in this scope.
///
/// Never returns a [`std::io::Result`]: an unreadable diagnostic is a diagnostic finding, not a
/// caller's error, and the one caller renders all three outcomes.
pub async fn read_last_maintenance_line(maintenance_root: &Path) -> MaintenanceLogRead {
    let Ok(bytes) = tokio::fs::read(maintenance_log_path(maintenance_root)).await else {
        return MaintenanceLogRead::Absent;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return MaintenanceLogRead::Unreadable;
    };
    let Some(last) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
        return MaintenanceLogRead::Absent;
    };
    serde_json::from_str::<MaintenanceLogLine>(last)
        .map_or(MaintenanceLogRead::Unreadable, |line| {
            MaintenanceLogRead::Line(Box::new(line))
        })
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

    fn sample() -> AsyncRetentionResult {
        let mut result = AsyncRetentionResult {
            acquired: true,
            scanned: 4,
            deleted_runs: 1,
            reaped_tombstones: 1,
            errored_candidates: 1,
            ..AsyncRetentionResult::default()
        };
        increment(&mut result.skipped, "recent");
        result
    }

    #[test]
    fn the_report_accounts_for_every_candidate() {
        let result = sample();
        assert!(
            result.accounts_for_every_candidate(),
            "one reap, one tombstone reap, one skip and one errored candidate must sum to scanned"
        );
    }

    /// The predicate must be able to say NO. Without this, `accounts_for_every_candidate` could be
    /// a hard-coded `true` and every test in this crate that calls it — here and throughout
    /// `sweep` — would still pass.
    #[test]
    fn the_accounting_identity_rejects_a_report_that_does_not_balance() {
        let mut lost_a_candidate = sample();
        lost_a_candidate.scanned += 1;
        assert!(
            !lost_a_candidate.accounts_for_every_candidate(),
            "a candidate that was considered and then landed in no bucket is exactly the hole \
             `errored_candidates` exists to close"
        );

        let mut double_counted = sample();
        double_counted.deleted_runs += 1;
        assert!(
            !double_counted.accounts_for_every_candidate(),
            "and a bucket claiming more than was scanned is equally wrong"
        );

        // A candidate-level skip really is counted by the identity — the mirror image of
        // `a_pass_level_skip_is_excluded_from_the_accounting_identity` below.
        let mut extra_skip = sample();
        increment(&mut extra_skip.skipped, "recheck-wait-reference");
        assert!(!extra_skip.accounts_for_every_candidate());
        assert!(
            !super::super::sweep::is_pass_level_skip("recheck-wait-reference"),
            "`recheck-` keys belong to a candidate, so they must never be excluded"
        );
    }

    #[test]
    fn a_pass_level_skip_is_excluded_from_the_accounting_identity() {
        // `lock-busy` is raised BEFORE the candidate loop and belongs to no candidate; counting it
        // would break the identity on exactly the passes that scanned nothing.
        let mut result = AsyncRetentionResult::default();
        increment(&mut result.skipped, "lock-busy");
        assert_eq!(result.scanned, 0);
        assert!(result.accounts_for_every_candidate());
    }

    #[test]
    fn the_report_serialises_deterministically() {
        let mut left = AsyncRetentionResult::default();
        let mut right = AsyncRetentionResult::default();
        for reason in ["recent", "non-terminal", "active-index", "resumable"] {
            increment(&mut left.skipped, reason);
        }
        for reason in ["resumable", "active-index", "non-terminal", "recent"] {
            increment(&mut right.skipped, reason);
        }
        assert_eq!(
            serde_json::to_string(&left).unwrap(),
            serde_json::to_string(&right).unwrap(),
            "a BTreeMap makes insertion order irrelevant; a HashMap would not"
        );
    }

    #[test]
    fn an_error_string_is_compacted_and_bounded() {
        let compacted = compact_message("two\n\n  words\tapart");
        assert_eq!(compacted, "two words apart");

        // A multi-byte error message: the cut must land on a `char` boundary, at or below the cap.
        let wide = "é".repeat(400);
        let bounded = compact_message(&wide);
        assert!(bounded.len() <= MAX_ERROR_BYTES);
        assert_eq!(bounded.len(), 240, "240 bytes == 120 two-byte chars");
        assert!(bounded.chars().all(|ch| ch == 'é'));
    }

    #[test]
    fn compacting_an_io_error_goes_through_the_same_bound() {
        let error = std::io::Error::other("a  b\n c");
        assert_eq!(compact_error(&error), "a b c");
    }

    #[tokio::test]
    async fn the_maintenance_log_appends_one_line_per_pass() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("maint");

        append_maintenance_log(&root, &sample(), 10, &["run:a".to_string()])
            .await
            .unwrap();
        append_maintenance_log(&root, &sample(), 20, &["run:b".to_string()])
            .await
            .unwrap();

        let text = tokio::fs::read_to_string(maintenance_log_path(&root))
            .await
            .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            serde_json::from_str::<MaintenanceLogLine>(line).unwrap();
        }

        match read_last_maintenance_line(&root).await {
            MaintenanceLogRead::Line(line) => {
                assert_eq!(line.at, 20);
                assert_eq!(line.deleted, vec!["run:b".to_string()]);
                assert!(line.acquired);
            }
            other => panic!("expected the last line, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_deleted_list_is_bounded_by_the_batch_size() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("maint");
        let ids: Vec<String> = (0..super::super::ASYNC_RETENTION_BATCH_SIZE * 3)
            .map(|index| format!("run:{index}"))
            .collect();

        append_maintenance_log(&root, &AsyncRetentionResult::default(), 1, &ids)
            .await
            .unwrap();

        match read_last_maintenance_line(&root).await {
            MaintenanceLogRead::Line(line) => assert_eq!(
                line.deleted.len(),
                super::super::ASYNC_RETENTION_BATCH_SIZE,
                "one line can never outgrow one pass"
            ),
            other => panic!("expected a line, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_absent_log_reads_as_absent_and_a_corrupt_one_as_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("maint");
        assert_eq!(
            read_last_maintenance_line(&root).await,
            MaintenanceLogRead::Absent
        );

        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(maintenance_log_path(&root), b"{not json\n")
            .await
            .unwrap();
        assert_eq!(
            read_last_maintenance_line(&root).await,
            MaintenanceLogRead::Unreadable
        );

        // A file with no complete line in it yet is ABSENT, not Unreadable: a fresh install and a
        // log truncated to nothing are the same condition, and the doctor renders "retention has
        // not run here" rather than "retention is broken".
        tokio::fs::write(maintenance_log_path(&root), b"\n   \n")
            .await
            .unwrap();
        assert_eq!(
            read_last_maintenance_line(&root).await,
            MaintenanceLogRead::Absent
        );

        // Bytes that are not UTF-8 at all are Unreadable — the half-written tail of a process
        // killed mid-append.
        tokio::fs::write(maintenance_log_path(&root), [0xffu8, 0xfe, 0x00].as_slice())
            .await
            .unwrap();
        assert_eq!(
            read_last_maintenance_line(&root).await,
            MaintenanceLogRead::Unreadable
        );

        // And the LAST complete line wins even when an earlier one is corrupt, which is what makes
        // the log self-repairing across a crash.
        let good =
            serde_json::to_string(&MaintenanceLogLine::from_result(&sample(), 7, &[])).unwrap();
        tokio::fs::write(
            maintenance_log_path(&root),
            format!("{{truncated\n{good}\n"),
        )
        .await
        .unwrap();
        match read_last_maintenance_line(&root).await {
            MaintenanceLogRead::Line(line) => assert_eq!(line.at, 7),
            other => panic!("expected the last, complete line, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_log_rotates_at_the_cap_instead_of_freezing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("maint");
        crate::background::ensure_accessible_dir(&root)
            .await
            .unwrap();
        // A log already at the cap: an append-with-cap writer alone would drop every further line
        // and freeze the doctor's view at this content forever.
        tokio::fs::write(
            maintenance_log_path(&root),
            vec![b'x'; usize::try_from(MAINTENANCE_LOG_CAP_BYTES).unwrap()],
        )
        .await
        .unwrap();

        append_maintenance_log(&root, &sample(), 99, &[])
            .await
            .unwrap();

        match read_last_maintenance_line(&root).await {
            MaintenanceLogRead::Line(line) => assert_eq!(line.at, 99, "the newest pass is visible"),
            other => panic!("expected the fresh line, got {other:?}"),
        }
        assert!(
            tokio::fs::metadata(root.join(format!("{MAINTENANCE_LOG_NAME}.1")))
                .await
                .is_ok(),
            "one generation is kept, so the whole record stays bounded at 2x the cap"
        );
    }
}
