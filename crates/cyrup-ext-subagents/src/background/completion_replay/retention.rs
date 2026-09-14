//! The opportunistic sweep: expired records, invalid records, and orphan archives.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::background::RunId;
use crate::background::result_index::errno;

use super::record::{parse_replay, validate_replay_record};
use super::{ARCHIVE_DIR_NAME, REPLAY_DIR_NAME, replay_file_name};

/// pi `lastCleanupByResultsDir` (`completion-replay.ts:13`) — a module-level `Map`.
///
/// Process-global by design: the throttle exists to stop N concurrent completions in one results
/// dir from each walking the directory, so it MUST be shared across every caller in the process.
/// Keyed by `results_dir` because that is the unit being swept; two cwds throttle independently.
///
/// A [`std::sync::Mutex`] with poison recovery — this crate's convention for a short non-`await`
/// critical section (`wait_completions/record.rs`'s store is the nearest in-crate example):
/// `.lock().unwrap_or_else(PoisonError::into_inner)`, never `.unwrap()`, because the crate denies
/// `clippy::unwrap_used`. The guard is taken and released before the sweep's first `.await`.
static LAST_CLEANUP_BY_RESULTS_DIR: LazyLock<Mutex<HashMap<PathBuf, i64>>> =
    LazyLock::new(Mutex::default);

/// pi `runIdFromReplayFile` (`:149-157`) — the run id a replay file name encodes, or `None`.
///
/// The decode is only half of it: upstream RE-ENCODES and compares (`:153`), and that comparison is
/// what rejects `%2f`-lowercase, over-encoded and otherwise non-canonical names. Port the
/// comparison, not just the decode — [`crate::identity::decode_uri_component`] is deliberately
/// permissive about escape case, exactly as JS is, and is harmless only because of this round-trip.
fn run_id_from_replay_file(file: &str) -> Option<RunId> {
    let stem = file.strip_suffix(crate::identity::ResultFileName::EXTENSION)?;
    let run_id = RunId::from_token(crate::identity::decode_uri_component(stem)?);
    (replay_file_name(&run_id) == file).then_some(run_id)
}

/// pi `removeBestEffort` (`:159-163`) — `rm -f`, every failure swallowed.
pub(super) async fn remove_best_effort(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

/// pi `cleanupCompletionReplayIfDue` (`:248-254`). `true` if the sweep RAN.
///
/// # `interval_ms` is a throttle, not a timer
///
/// This returns `false` — cheaply, without touching the filesystem — when called again inside the
/// interval. That is what makes it safe to call on every completion write (`:207`) AND from a
/// schedule ([`crate::extension::SubagentExecutor::install_completion_watcher`]'s detached sweep):
/// whichever fires first does the work and the other is one map lookup. Nothing here starts a timer
/// or spawns a task.
///
/// The map is updated BEFORE the sweep (`:251`), not after, so a long sweep cannot admit a second
/// concurrent one. The decision and the insert are taken in ONE lock acquisition, which is then
/// released before the sweep's `.await` — a `std::sync::MutexGuard` must never be held across one.
pub async fn cleanup_completion_replay_if_due(
    results_dir: &Path,
    now: i64,
    max_age_ms: i64,
    interval_ms: i64,
) -> bool {
    {
        let mut last = LAST_CLEANUP_BY_RESULTS_DIR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = last.get(results_dir).copied()
            && now.saturating_sub(previous) < interval_ms
        {
            return false;
        }
        last.insert(results_dir.to_path_buf(), now);
    }
    cleanup_completion_replay(results_dir, now, max_age_ms).await;
    true
}

/// pi `cleanupCompletionReplay` (`:257-287`). Best-effort; never surfaces an error.
///
/// # The replay dir's per-file decision (`:260-276`)
///
/// 1. the file name does not round-trip through [`super::replay_file_name`] → SKIP entirely
///    (`:261-262`);
/// 2. parsed but [`validate_replay_record`] rejected → remove, **no age check** (`:267-268`);
/// 3. valid and `expires_at <= now` → remove the record AND its archive (`:269-271`);
/// 4. UNparseable and `now - mtime > max_age_ms` → remove (`:272-273`);
/// 5. otherwise keep.
///
/// Row 4 is the one that must not be simplified into row 2. An unparseable file may be a record
/// written by a NEWER build whose version this one does not know; deleting it on sight would make
/// two cyrup versions sharing a directory destroy each other's records. The age check is the
/// concession that lets them coexist — and it is exactly why [`super::parse_replay`] must not take
/// the caller's run id, which would move row 2's inputs into row 4.
///
/// # The archive dir (`:278-286`) sweeps on mtime ALONE
///
/// Every archive older than `max_age_ms` goes, with no record consulted. That is safe only because
/// both writes happen in the same call with the same `ttl_ms` (`:195`, `:202`, `:207`), so a
/// record's `expires_at` and its archive's age-out coincide. A caller passing a `max_age_ms`
/// SHORTER than the `ttl_ms` a record was written with would strand a live record pointing at a
/// deleted archive — [`super::read_completion_archive`] returns `Ok(None)` there and the completion
/// still replays without its text, which is the intended degradation.
///
/// One bad entry must never abort the sweep (`:275`, `:284`) and a missing directory is not an
/// error (`:277`, `:286`): every per-file operation is individually swallowed, using
/// [`crate::background::result_index::errno`]'s named predicates rather than a fresh `matches!` on
/// `ErrorKind`.
pub async fn cleanup_completion_replay(results_dir: &Path, now: i64, max_age_ms: i64) {
    sweep_replay_dir(results_dir, now, max_age_ms).await;
    sweep_archive_dir(&results_dir.join(ARCHIVE_DIR_NAME), now, max_age_ms).await;
}

/// The four-way decision of [`cleanup_completion_replay`]'s replay half (`:258-277`).
async fn sweep_replay_dir(results_dir: &Path, now: i64, max_age_ms: i64) {
    let replay_dir = results_dir.join(REPLAY_DIR_NAME);
    let Ok(mut listing) = tokio::fs::read_dir(&replay_dir).await else {
        return; // `:277` — the replay directory may not exist yet
    };
    while let Ok(Some(entry)) = listing.next_entry().await {
        let file_name = entry.file_name();
        let Some(file) = file_name.to_str() else {
            continue; // a non-UTF-8 name cannot be a `safeRunFile`
        };
        let Some(run_id) = run_id_from_replay_file(file) else {
            continue; // row 1
        };
        let path = entry.path();
        let record = match tokio::fs::read(&path).await {
            Ok(bytes) => parse_replay(&bytes),
            // `:275` — one unreadable entry must not block the sweep. Absence is the ordinary race
            // with a concurrent reader that already reaped it.
            Err(error) => {
                if !errno::is_ignorable_listing_error(&error) {
                    tracing::warn!(path = %path.display(), %error, "replay sweep: unreadable record");
                }
                continue;
            }
        };
        let safe = record
            .clone()
            .and_then(|record| validate_replay_record(results_dir, &run_id, record));
        match (record, safe) {
            // Row 2: it parsed, but it is not addressable as this file's run — remove on sight.
            (Some(_), None) => remove_best_effort(&path).await,
            // Row 3: expired. The archive goes with it, which is what keeps the two files'
            // lifetimes tied together.
            (_, Some(safe)) if safe.expires_at <= now => {
                remove_best_effort(&path).await;
                remove_best_effort(&safe.archive_path).await;
            }
            // Row 4: unparseable — possibly a NEWER build's record — so only age decides.
            (None, _) => {
                if older_than(&path, now, max_age_ms).await {
                    remove_best_effort(&path).await;
                }
            }
            // Row 5: valid and live.
            (Some(_), Some(_)) => {}
        }
    }
}

/// The archive half (`:278-286`): mtime alone, no record consulted.
async fn sweep_archive_dir(archive_dir: &Path, now: i64, max_age_ms: i64) {
    let Ok(mut listing) = tokio::fs::read_dir(archive_dir).await else {
        return; // `:286` — the archive directory may not exist yet
    };
    while let Ok(Some(entry)) = listing.next_entry().await {
        let path = entry.path();
        if older_than(&path, now, max_age_ms).await {
            remove_best_effort(&path).await;
        }
    }
}

/// pi's `now - fs.statSync(filePath).mtimeMs > maxAgeMs` (`:272`, `:283`).
///
/// A file whose mtime cannot be read at all is NOT swept: upstream's `catch` skips the entry
/// (`:275`, `:284`), and "I could not tell how old this is" must never resolve to "delete it".
async fn older_than(path: &Path, now: i64, max_age_ms: i64) -> bool {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return false; // an mtime before 1970 is not an age this policy can reason about
    };
    let mtime_ms = i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX);
    now.saturating_sub(mtime_ms) > max_age_ms
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::{
        CompletionReplayWrite, completion_archive_path, completion_replay_path,
        write_completion_replay,
    };
    use super::*;
    use crate::background::wait_completions::WaitCompletion;
    use crate::identity::SessionId;
    use serde_json::json;

    fn session() -> SessionId {
        SessionId::parse("s1").expect("non-empty")
    }

    fn completion(run: &str) -> WaitCompletion {
        WaitCompletion {
            run_id: run.to_string(),
            ..WaitCompletion::default()
        }
    }

    async fn write(dir: &Path, run: &str, now: i64, ttl_ms: i64) {
        write_completion_replay(&CompletionReplayWrite {
            results_dir: dir,
            run_id: &RunId::from_token(run),
            session_id: &session(),
            completion: &completion(run),
            data: &json!({}),
            now,
            ttl_ms,
        })
        .await
        .expect("write");
    }

    /// `:249-250` — inside the interval the call is a MAP LOOKUP, not a directory walk. That is
    /// what makes it safe to drive retention from the write path on every completion.
    #[tokio::test]
    async fn cleanup_is_throttled_to_the_interval() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        assert!(
            cleanup_completion_replay_if_due(dir, 1_000, 600_000, 60_000).await,
            "the first call in a directory always sweeps"
        );
        assert!(
            !cleanup_completion_replay_if_due(dir, 60_999, 600_000, 60_000).await,
            "one millisecond inside the interval: refused"
        );
        assert!(
            cleanup_completion_replay_if_due(dir, 61_000, 600_000, 60_000).await,
            "exactly at the interval: `now - last < interval` is false, so it sweeps"
        );

        // Keyed by results dir — a second cwd throttles independently, or one busy project would
        // silence every other project's retention.
        let other = tempfile::tempdir().expect("tempdir");
        assert!(cleanup_completion_replay_if_due(other.path(), 61_000, 600_000, 60_000).await);
    }

    /// The four-way age policy. The row that matters most is the last one: an UNPARSEABLE record
    /// inside the window SURVIVES, because it may be a newer build's record and two cyrup versions
    /// sharing a directory must not destroy each other's state.
    #[tokio::test]
    async fn cleanup_removes_records_past_max_age() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        // The clock has to be the REAL one: row 4 compares against a file's mtime, which no test
        // can choose, so every other row's `expires_at` must be expressed against the same epoch.
        let now = crate::time::now_epoch_millis();
        // Row 3: expired.
        write(dir, "expired", now, 10).await;
        // Row 5: live.
        write(dir, "live", now, 600_000).await;
        // Row 4: an unparseable file — a record a NEWER build wrote, as far as this one can tell.
        let replay_dir = dir.join(REPLAY_DIR_NAME);
        let future_version = replay_dir.join("future.json");
        tokio::fs::write(&future_version, br#"{"version":99}"#.as_slice())
            .await
            .expect("write");

        cleanup_completion_replay(dir, now + 1_000, 600_000).await;
        assert!(
            !completion_replay_path(dir, &RunId::from_token("expired")).exists(),
            "an expired record is removed"
        );
        assert!(
            !completion_archive_path(dir, &RunId::from_token("expired")).exists(),
            "and so is its archive — the two lifetimes are tied"
        );
        assert!(
            completion_replay_path(dir, &RunId::from_token("live")).exists(),
            "a live record survives"
        );
        assert!(
            future_version.exists(),
            "an unparseable record INSIDE the age window survives: it may be a newer build's"
        );

        // Now age it out: the same file, a `max_age_ms` it is past.
        cleanup_completion_replay(dir, now + 1_000_000, 0).await;
        assert!(
            !future_version.exists(),
            "past max_age_ms the unparseable record is finally swept"
        );

        // A missing directory is not an error (`:277`, `:286`).
        let empty = tempfile::tempdir().expect("tempdir");
        cleanup_completion_replay(empty.path(), now, 0).await;
    }

    /// The `parse_replay`/`validate_replay_record` split, observable. A record whose `runId`
    /// disagrees with its FILE NAME must hit row 2 — removed on sight, no age check — not row 4.
    /// Folding the run-id check into `parse_replay` would make it unparseable instead, and it would
    /// then survive the whole retention window pretending to be a future version.
    #[tokio::test]
    async fn a_replay_whose_run_id_disagrees_with_its_file_name_is_removed_on_sight() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        // A well-formed, UNEXPIRED, internally-consistent record for `other`, filed under `mine`.
        write(dir, "other", 1_000, 600_000).await;
        let misfiled = completion_replay_path(dir, &RunId::from_token("mine"));
        tokio::fs::rename(
            completion_replay_path(dir, &RunId::from_token("other")),
            &misfiled,
        )
        .await
        .expect("rename");
        assert!(misfiled.exists());

        // `max_age_ms` is enormous, so row 4 could not possibly fire: only row 2 can remove this.
        cleanup_completion_replay(dir, 2_000, i64::MAX / 4).await;
        assert!(
            !misfiled.exists(),
            "a record filed under another run's name is removed immediately, without an age check"
        );
    }

    /// `runIdFromReplayFile`'s round-trip (`:149-157`). The decode alone is permissive; the
    /// re-encode-and-compare is what rejects a non-canonical name, and a name this sweep cannot
    /// attribute is SKIPPED rather than deleted — it is not this format's file.
    #[test]
    fn only_canonically_encoded_replay_file_names_are_attributed() {
        assert_eq!(
            run_id_from_replay_file("a%20b.json")
                .as_ref()
                .map(RunId::as_str),
            Some("a b")
        );
        assert_eq!(
            run_id_from_replay_file("plain.json")
                .as_ref()
                .map(RunId::as_str),
            Some("plain")
        );
        assert!(
            run_id_from_replay_file("a%20b.txt").is_none(),
            "wrong extension"
        );
        assert!(
            run_id_from_replay_file("a%2fb.json").is_none(),
            "lowercase hex decodes but does not re-encode to itself"
        );
        assert!(
            run_id_from_replay_file("a%zz.json").is_none(),
            "a malformed escape is a URIError upstream"
        );
        assert!(
            run_id_from_replay_file("%FF.json").is_none(),
            "an escape decoding to invalid UTF-8 is a URIError too"
        );
    }
}
