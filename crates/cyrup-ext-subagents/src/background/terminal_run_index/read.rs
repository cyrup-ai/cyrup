//! The reader — [`read_recent_terminal_run_index`].
//!
//! Ports pi `readRecentTerminalRunIndex` and its helpers (`terminal-run-index.ts:63-138`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::background::{RunDir, RunId, control::read_status_file};
use crate::error::SubagentError;
use crate::identity::SessionId;

use super::entry::{TerminalRunIndexEntry, index_root, is_indexed_state, session_index_dir};

/// The run is simply not there: `ENOENT` or `ENOTDIR`.
///
/// pi's two listing helpers swallow exactly this pair and rethrow everything else
/// (`markerFiles` `:80-84`, `sessionDirs` `:93-97`). Deliberately NARROWER than
/// [`crate::background::result_index`]'s `errno::is_absent`, which also folds in `ENAMETOOLONG`
/// for its alias fan-out — this index has no aliases, and upstream rethrows `ENAMETOOLONG` here.
fn is_absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    ) || error.raw_os_error() == Some(libc::ENOTDIR)
}

/// pi `removeInvalidMarker` (`:63-70`): best-effort — "an advisory marker can remain for a later
/// read when cleanup is contended."
async fn remove_invalid_marker(marker: &Path) {
    let _ = tokio::fs::remove_file(marker).await;
}

/// The session partitions to scan — pi `sessionDirs` (`:87-98`): the single write-keyed directory
/// when a session filter is supplied, every subdirectory of the index root otherwise.
///
/// # Errors
///
/// A listing failure other than `ENOENT`/`ENOTDIR` (pi rethrows, `:93-97`).
async fn session_dirs(
    async_root: &Path,
    session_id: Option<&SessionId>,
) -> Result<Vec<PathBuf>, SubagentError> {
    if let Some(session_id) = session_id {
        return Ok(vec![session_index_dir(async_root, session_id)]);
    }
    let mut dirs = Vec::new();
    let mut entries = match tokio::fs::read_dir(index_root(async_root)).await {
        Ok(entries) => entries,
        Err(e) if is_absent(&e) => return Ok(dirs),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };
    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
        if entry
            .file_type()
            .await
            .map_err(SubagentError::Spawn)?
            .is_dir()
        {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// One directory's `.json` markers — pi `markerFiles` (`:74-85`). Returned as `(dir, name)` so the
/// caller can sort on the NAME alone, exactly as upstream does.
///
/// # Errors
///
/// A listing failure other than `ENOENT`/`ENOTDIR`.
async fn marker_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, SubagentError> {
    let mut files = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if is_absent(&e) => return Ok(files),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };
    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
        if !entry
            .file_type()
            .await
            .map_err(SubagentError::Spawn)?
            .is_file()
        {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && name.ends_with(".json")
        {
            files.push((dir.to_path_buf(), name.to_string()));
        }
    }
    Ok(files)
}

/// The newest `limit` candidate marker paths across `dirs` — pi `recentMarkerFiles` (`:102-107`):
/// sort by file name DESCENDING, then slice, so the bound applies to candidates BEFORE validation.
///
/// `[CYRUP-DELTA]` upstream sorts with `localeCompare`, cyrup with byte order. Both agree on the
/// zero-padded ASCII timestamp prefix, so chronology is identical; the two can only differ in the
/// tie-break among markers sharing a millisecond, where neither ordering is semantically
/// meaningful.
async fn recent_marker_files(
    dirs: &[PathBuf],
    limit: Option<usize>,
) -> Result<Vec<PathBuf>, SubagentError> {
    let mut markers: Vec<(PathBuf, String)> = Vec::new();
    for dir in dirs {
        markers.extend(marker_files(dir).await?);
    }
    markers.sort_by(|(_, left), (_, right)| right.cmp(left));
    if let Some(limit) = limit {
        markers.truncate(limit);
    }
    Ok(markers
        .into_iter()
        .map(|(dir, name)| dir.join(name))
        .collect())
}

/// The run ids of recently-terminal runs, newest first — pi `readRecentTerminalRunIndex`
/// (`:108-138`).
///
/// `session_id: None` is the **OPTIONAL** strictness class: no filter at all, which is different
/// from the PERMISSIVE class control operations use. pi's literal test is
/// `options.sessionId !== undefined && entry.sessionId !== options.sessionId` (`:120`) — absent
/// means "every session", present means "exactly this one", and an entry that cannot be attributed
/// is never silently admitted.
///
/// `limit` bounds the CANDIDATE markers, before validation (`:111`), so the result may be shorter
/// — that is the whole point: a bounded read rather than a bounded-output full scan. `Some(0)` is
/// empty; `None` is unbounded.
///
/// # The delete policy (`:113-137`)
///
/// A marker is unlinked when it is unparseable, foreign to the requested session, disagreeing
/// with the run's live `status.json` (missing, non-indexed state, different session, or a
/// different non-empty recorded run id), or a duplicate of a newer marker for the same run. The
/// one deliberate NON-trigger: a marker whose read races its own deletion (`ENOENT`) is skipped
/// without a second unlink.
///
/// # Errors
///
/// Propagates a status-read fault, matching upstream: `readStatus` throws for anything but ENOENT
/// (`shared/utils.ts:169-195`) and this function does not catch it. The consumer degrades to its
/// existing full scan on `Err`, so a single corrupt `status.json` costs a slow enumeration rather
/// than a wrong answer.
pub async fn read_recent_terminal_run_index(
    async_root: &Path,
    session_id: Option<&SessionId>,
    limit: Option<usize>,
) -> Result<Vec<RunId>, SubagentError> {
    let dirs = session_dirs(async_root, session_id).await?;
    let candidates = recent_marker_files(&dirs, limit).await?;

    let mut run_ids: Vec<RunId> = Vec::new();
    let mut seen: BTreeSet<RunId> = BTreeSet::new();
    for marker in candidates {
        // pi `:113-118` — the catch swallows a NON-ENOENT read error and falls through with
        // `entry === undefined`, so EACCES/garbage DOES reach `removeInvalidMarker` while a
        // marker deleted underneath us does not.
        let entry = match tokio::fs::read(&marker).await {
            Ok(bytes) => TerminalRunIndexEntry::parse(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => None,
        };
        let Some(entry) =
            entry.filter(|entry| session_id.is_none_or(|wanted| &entry.session_id == wanted))
        else {
            remove_invalid_marker(&marker).await;
            continue;
        };

        // pi `:124-130` — re-verify against the LIVE status, addressed by the ENTRY's run id
        // (never the marker path's): the marker name encodes the directory leaf, and the two can
        // legitimately disagree. Note pi's run-id clause is guarded (`status.runId && …`), so an
        // empty recorded run id does NOT invalidate the marker.
        let status = read_status_file(&RunDir::new(async_root, &entry.run_id).status()).await?;
        let agrees = status.as_ref().is_some_and(|status| {
            is_indexed_state(status.state)
                && status.session_id.as_ref() == Some(&entry.session_id)
                && (status.run_id.as_str().is_empty() || status.run_id == entry.run_id)
        });
        if !agrees {
            remove_invalid_marker(&marker).await;
            continue;
        }

        // pi `:131-134` — candidates are newest-first, so the duplicate that gets unlinked is
        // always the OLDER marker.
        if seen.contains(&entry.run_id) {
            remove_invalid_marker(&marker).await;
            continue;
        }
        seen.insert(entry.run_id.clone());
        run_ids.push(entry.run_id);
    }
    Ok(run_ids)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::update::update_terminal_run_index;
    use super::*;
    use crate::background::{RunMode, RunState, RunStatus};

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    /// Write a run directory with a live `status.json` AND its index marker, the way the
    /// producers do it.
    async fn settle_run(
        async_root: &Path,
        run: &str,
        state: RunState,
        session_id: &str,
        ended_at: i64,
    ) {
        let async_dir = async_root.join(run);
        tokio::fs::create_dir_all(&async_dir)
            .await
            .expect("mkdir run dir");
        let mut status = RunStatus::queued(RunId::from_token(run), RunMode::Single, Some(1));
        status.state = state;
        status.session_id = Some(session(session_id));
        status.ended_at = Some(ended_at);
        tokio::fs::write(
            async_dir.join("status.json"),
            serde_json::to_vec(&status).expect("ser"),
        )
        .await
        .expect("write status");
        update_terminal_run_index(&async_dir, &status)
            .await
            .expect("index write");
    }

    fn marker_count(async_root: &Path) -> usize {
        let Ok(sessions) = std::fs::read_dir(index_root(async_root)) else {
            return 0;
        };
        sessions
            .flatten()
            .filter_map(|s| std::fs::read_dir(s.path()).ok())
            .map(|files| files.count())
            .sum()
    }

    fn ids(runs: &[RunId]) -> Vec<&str> {
        runs.iter().map(RunId::as_str).collect()
    }

    #[tokio::test]
    async fn an_absent_index_reads_as_empty_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runs = read_recent_terminal_run_index(tmp.path(), None, None)
            .await
            .expect("ok");
        assert!(runs.is_empty());
        let scoped = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert!(scoped.is_empty());
    }

    #[tokio::test]
    async fn what_the_writer_indexes_the_reader_returns_newest_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "old1", RunState::Complete, "s1", 100).await;
        settle_run(tmp.path(), "new1", RunState::Failed, "s1", 200).await;
        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert_eq!(ids(&runs), vec!["new1", "old1"]);
    }

    #[tokio::test]
    async fn a_paused_run_is_in_the_index() {
        // §1.1 — the predicate is NOT `RunState::is_terminal`; a paused run has stopped moving
        // and must be listed.
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "paused1", RunState::Paused, "s1", 10).await;
        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert_eq!(ids(&runs), vec!["paused1"]);
    }

    #[tokio::test]
    async fn the_session_filter_is_optional_absent_means_every_session() {
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "runa", RunState::Complete, "s1", 100).await;
        settle_run(tmp.path(), "runb", RunState::Complete, "s2", 200).await;

        let all = read_recent_terminal_run_index(tmp.path(), None, None)
            .await
            .expect("ok");
        assert_eq!(
            ids(&all),
            vec!["runb", "runa"],
            "no filter lists every session"
        );

        let scoped = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert_eq!(ids(&scoped), vec!["runa"]);
        assert_eq!(
            marker_count(tmp.path()),
            2,
            "a scoped read of one partition deletes nothing"
        );
    }

    #[tokio::test]
    async fn limit_bounds_the_candidates_before_validation() {
        // pi `:108-111` — `limit: N` slices the SORTED candidate list first, so an invalid marker
        // inside the window shrinks the result rather than pulling in the next-oldest candidate.
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "run1", RunState::Complete, "s1", 100).await;
        settle_run(tmp.path(), "run2", RunState::Complete, "s1", 200).await;
        settle_run(tmp.path(), "run3", RunState::Complete, "s1", 300).await;
        // Invalidate the NEWEST candidate's live status by removing its run directory.
        tokio::fs::remove_dir_all(tmp.path().join("run3"))
            .await
            .expect("rm run3");

        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), Some(2))
            .await
            .expect("ok");
        assert_eq!(
            ids(&runs),
            vec!["run2"],
            "limit=2 candidates were run3 (invalid) and run2 — run1 must NOT backfill"
        );

        // `Some(0)` is an empty read, not "unbounded".
        let none = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), Some(0))
            .await
            .expect("ok");
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn an_unparseable_marker_is_unlinked_and_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "good1", RunState::Complete, "s1", 100).await;
        let dir = session_index_dir(tmp.path(), &session("s1"));
        std::fs::write(dir.join("0000000000000200-junk.json"), b"not json").expect("junk");

        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert_eq!(ids(&runs), vec!["good1"]);
        assert!(
            !dir.join("0000000000000200-junk.json").exists(),
            "junk marker must be unlinked"
        );
    }

    #[tokio::test]
    async fn a_marker_disagreeing_with_the_live_status_is_unlinked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "run1", RunState::Complete, "s1", 100).await;
        // The run "resumes": its live status flips back to Running, so the marker now lies.
        let mut status = RunStatus::queued(RunId::from_token("run1"), RunMode::Single, Some(1));
        status.state = RunState::Running;
        status.session_id = Some(session("s1"));
        tokio::fs::write(
            tmp.path().join("run1").join("status.json"),
            serde_json::to_vec(&status).expect("ser"),
        )
        .await
        .expect("rewrite status");

        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert!(runs.is_empty());
        assert_eq!(
            marker_count(tmp.path()),
            0,
            "the stale marker must be unlinked"
        );
    }

    #[tokio::test]
    async fn a_marker_whose_run_status_is_gone_is_unlinked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "run1", RunState::Complete, "s1", 100).await;
        tokio::fs::remove_dir_all(tmp.path().join("run1"))
            .await
            .expect("rm run dir");

        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert!(runs.is_empty());
        assert_eq!(marker_count(tmp.path()), 0);
    }

    #[tokio::test]
    async fn a_foreign_session_marker_under_the_requested_partition_is_unlinked() {
        // pi `:119-122` — an entry whose OWN sessionId differs from the requested one is invalid
        // even though it sits in the requested partition (hash collisions, hand-moved files).
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "run1", RunState::Complete, "s2", 100).await;
        // Copy s2's marker into s1's partition.
        let s2_dir = session_index_dir(tmp.path(), &session("s2"));
        let s1_dir = session_index_dir(tmp.path(), &session("s1"));
        std::fs::create_dir_all(&s1_dir).expect("mkdir s1");
        let marker = std::fs::read_dir(&s2_dir)
            .expect("ls")
            .next()
            .expect("one")
            .expect("entry");
        std::fs::copy(marker.path(), s1_dir.join(marker.file_name())).expect("copy");

        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert!(runs.is_empty());
        assert!(
            !s1_dir.join(marker.file_name()).exists(),
            "the foreign marker must be unlinked"
        );
        assert!(
            s2_dir.join(marker.file_name()).exists(),
            "s2's own marker is untouched"
        );
    }

    #[tokio::test]
    async fn duplicates_keep_the_newest_marker_and_unlink_the_older() {
        // pi `:131-134` — the candidates are newest-first, so `seen` catches the OLDER duplicate.
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "run1", RunState::Complete, "s1", 100).await;
        // The same run reaches a terminal state again later (e.g. resumed then re-completed).
        settle_run(tmp.path(), "run1", RunState::Complete, "s1", 200).await;
        assert_eq!(marker_count(tmp.path()), 2, "two markers for one run");

        let runs = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None)
            .await
            .expect("ok");
        assert_eq!(ids(&runs), vec!["run1"], "one row per run");
        assert_eq!(
            marker_count(tmp.path()),
            1,
            "the older duplicate must be unlinked"
        );
        let dir = session_index_dir(tmp.path(), &session("s1"));
        let survivor = std::fs::read_dir(dir)
            .expect("ls")
            .next()
            .expect("one")
            .expect("entry");
        assert!(
            survivor
                .file_name()
                .to_string_lossy()
                .starts_with("0000000000000200-"),
            "the NEWEST marker survives: {:?}",
            survivor.file_name()
        );
    }

    #[tokio::test]
    async fn a_corrupt_status_file_propagates_as_an_error() {
        // pi: `readStatus` throws for anything but ENOENT and `readRecentTerminalRunIndex` does
        // not catch it — the consumer's fallback scan is the degradation path, not a silent skip.
        let tmp = tempfile::tempdir().expect("tempdir");
        settle_run(tmp.path(), "run1", RunState::Complete, "s1", 100).await;
        tokio::fs::write(tmp.path().join("run1").join("status.json"), b"garbage")
            .await
            .expect("corrupt");

        let result = read_recent_terminal_run_index(tmp.path(), Some(&session("s1")), None).await;
        assert!(
            result.is_err(),
            "a corrupt status must propagate, got {result:?}"
        );
    }
}
