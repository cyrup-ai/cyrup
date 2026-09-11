//! Index-driven candidate enumeration — the replacement for `read_dir(results_dir)`.
//!
//! Ports pi `listIndexFiles` (`result-files.ts:382-395`), `resultFilesFromIndexDir` (`:396-412`),
//! `resultFilesForSession` (`:414-420`), `pendingResultFilesForSession` (`:422-446`),
//! `resultCandidateFilesForSession` (`:448-455`), the tool-call pair (`:457-463`) and the mission
//! observer pair (`:465-471`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::identity::{ResultFileName, SessionId};

use super::entry::ResultIndexEntry;
use super::errno;
use super::locate::{self, PayloadResolution, PayloadState};
use super::paths;

/// List the `.json` files directly inside one index directory.
///
/// pi `listIndexFiles` (`result-files.ts:382-395`).
///
/// # Errors
///
/// Propagates a genuine I/O fault. A missing directory **or a permission fault** yields an empty
/// list — enumeration is a best-effort sweep across many directories, most of which will not
/// exist, and one unreadable partition must not abort the scan. This is deliberately laxer than
/// [`locate::read_result_index_for_session_run`], which rethrows permission faults because a
/// direct question deserves a direct answer.
async fn list_index_files(dir: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(error) if errno::is_ignorable_listing_error(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        if entry
            .file_type()
            .await
            .is_ok_and(|file_type| file_type.is_file())
        {
            files.push(path);
        }
    }
    Ok(files)
}

/// Resolve every index entry in one directory to the payload file it names.
///
/// pi `resultFilesFromIndexDir` (`result-files.ts:396-412`).
///
/// `include_pending` distinguishes the two upstream call shapes: `resultFilesForSession` wants
/// only payloads that are already public, while `resultCandidateFilesForSession` — what the
/// watcher uses — also wants staged ones, because a staged payload is a completion that simply has
/// not been promoted yet.
///
/// An unparseable entry is **unlinked** (pi `:402`): an index file that cannot be read has no
/// value and would otherwise be re-examined on every scan forever.
async fn result_files_from_index_dir(
    results_dir: &Path,
    dir: &Path,
    include_pending: bool,
) -> std::io::Result<BTreeSet<ResultFileName>> {
    let mut candidates = BTreeSet::new();
    for entry_path in list_index_files(dir).await? {
        let Some(entry) = read_index_entry(&entry_path).await else {
            continue;
        };
        if let Some(file) = indexed_result_file(results_dir, &entry, include_pending).await {
            candidates.insert(file);
        }
    }
    Ok(candidates)
}

/// Read one index entry, unlinking it if it is unparseable (pi `:402`): an index file that cannot
/// be read has no value and would otherwise be re-examined on every scan forever.
async fn read_index_entry(entry_path: &Path) -> Option<ResultIndexEntry> {
    let bytes = match tokio::fs::read(entry_path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if !errno::is_absent(&error) {
                tracing::warn!(path = %entry_path.display(), %error, "ignoring invalid async result index");
            }
            return None;
        }
    };
    match ResultIndexEntry::parse(&bytes) {
        Some(entry) => Some(entry),
        None => {
            let _ = tokio::fs::remove_file(entry_path).await;
            None
        }
    }
}

/// pi `indexedResultFile` (`result-files.ts:376-380`).
async fn indexed_result_file(
    results_dir: &Path,
    entry: &ResultIndexEntry,
    include_pending: bool,
) -> Option<ResultFileName> {
    let location = locate::result_payload_location_from_index(results_dir, entry).await?;
    if include_pending || location.state != PayloadState::Staged {
        Some(location.file)
    } else {
        None
    }
}

/// One enumerated candidate: an index entry plus what became of the payload it names.
///
/// # `Absent` is a candidate, not a non-candidate
///
/// The enumeration this replaces returned bare [`ResultFileName`]s and dropped any entry whose
/// payload did not resolve. That single `?` was a silent, permanent loss: the run had completed,
/// its index entries stayed on disk forever, every later scan re-read them and dropped them again,
/// and nothing ever told anyone. Nineteen such entries were found on one machine in a day. Here
/// the failure to resolve is carried in the value, so the watcher decides what to do about it.
///
/// Carrying the identity is also what makes an owned payload resolvable at all: a file NAME cannot
/// locate `result-owned/<enc(session)>/<enc(run)>.json`, only `(session_id, run_id)` can.
#[derive(Debug)]
pub struct ResultCandidate {
    /// The run this candidate is for.
    pub run_id: RunId,
    /// The session that owns it — the partition key, and half of the owned path.
    pub session_id: SessionId,
    /// The recorded payload file name.
    pub file: ResultFileName,
    /// The run's own directory, when the index recorded one. The source of the recovery text and
    /// the artifact pointer when a payload turns out to be gone.
    pub async_dir: Option<PathBuf>,
    /// When the index entry was written (epoch millis).
    pub written_at: i64,
    /// Where the payload is, or why it could not be reached.
    pub payload: PayloadResolution,
}

impl ResultCandidate {
    /// Resolve one index entry into a candidate, entitlement included.
    async fn resolve(results_dir: &Path, entry: ResultIndexEntry) -> Self {
        let payload =
            locate::resolve_payload(results_dir, &entry.session_id, &entry.run_id, &entry.file)
                .await;
        Self {
            run_id: entry.run_id,
            session_id: entry.session_id,
            file: entry.file,
            async_dir: entry.async_dir,
            written_at: entry.written_at,
            payload,
        }
    }
}

/// Every candidate in one index directory, resolved.
async fn result_candidates_from_index_dir(
    results_dir: &Path,
    dir: &Path,
) -> std::io::Result<Vec<ResultCandidate>> {
    let mut candidates = Vec::new();
    for entry_path in list_index_files(dir).await? {
        let Some(entry) = read_index_entry(&entry_path).await else {
            continue;
        };
        candidates.push(ResultCandidate::resolve(results_dir, entry).await);
    }
    Ok(candidates)
}

/// Every public payload belonging to one session.
///
/// pi `resultFilesForSession` (`result-files.ts:414-420`).
///
/// # Errors
///
/// As [`list_index_files`].
pub async fn result_files_for_session(
    results_dir: &Path,
    session_id: &SessionId,
) -> std::io::Result<BTreeSet<ResultFileName>> {
    let mut files = BTreeSet::new();
    for dir in paths::session_index_dirs(results_dir, session_id) {
        files.extend(result_files_from_index_dir(results_dir, &dir, false).await?);
    }
    Ok(files)
}

/// Staged payloads belonging to one session, discovered by reading the staging directory.
///
/// pi `pendingResultFilesForSession` (`result-files.ts:422-446`).
///
/// This is the one enumeration that reads payloads rather than index entries: a staged file may
/// have been written before its index landed (the write order is payload-then-index), so the
/// staging directory is authoritative for "what exists but is not yet public". Each payload's own
/// `sessionId` is re-checked against the caller's — the directory name is an encoded, possibly
/// hashed segment, so contents decide.
///
/// # Errors
///
/// As [`list_index_files`].
async fn pending_result_files_for_session(
    results_dir: &Path,
    session_id: &SessionId,
) -> std::io::Result<BTreeSet<ResultFileName>> {
    let mut files = BTreeSet::new();
    for dir in paths::pending_session_dirs(results_dir, session_id) {
        for path in list_index_files(&dir).await? {
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                tracing::warn!(path = %path.display(), "ignoring invalid pending async result");
                continue;
            };
            let owner = value.get("sessionId").and_then(serde_json::Value::as_str);
            if owner != Some(session_id.as_str()) {
                continue;
            }
            if let Some(run_id) = locate::payload_run_id(&value) {
                files.insert(ResultFileName::for_run(&RunId::from_token(run_id.as_str())));
            }
        }
    }
    Ok(files)
}

/// Every payload a session might need to be notified about: indexed (public **or** staged) plus
/// anything sitting in its staging directory.
///
/// pi `resultCandidateFilesForSession` (`result-files.ts:448-455`) — candidate source #1 for the
/// watcher.
///
/// # Errors
///
/// As [`list_index_files`].
pub async fn result_candidate_files_for_session(
    results_dir: &Path,
    session_id: &SessionId,
) -> std::io::Result<BTreeSet<ResultFileName>> {
    let mut files = BTreeSet::new();
    for dir in paths::session_index_dirs(results_dir, session_id) {
        files.extend(result_files_from_index_dir(results_dir, &dir, true).await?);
    }
    files.extend(pending_result_files_for_session(results_dir, session_id).await?);
    Ok(files)
}

/// Every candidate belonging to one session, each carrying its identity and its payload's fate.
///
/// The watcher's candidate source #1 (pi `resultCandidateFilesForSession`,
/// `result-files.ts:448-455`), in the shape the watcher actually needs: a candidate whose payload
/// did not resolve is RETURNED, because "this run finished and its result is gone" is a fact the
/// orchestrator must hear, not one to drop on the floor.
///
/// The staging directory is swept too (pi `:452`), for payloads written before their index landed.
///
/// # Errors
///
/// As [`list_index_files`].
pub async fn result_candidates_for_session(
    results_dir: &Path,
    session_id: &SessionId,
) -> std::io::Result<Vec<ResultCandidate>> {
    let mut candidates: Vec<ResultCandidate> = Vec::new();
    for dir in paths::session_index_dirs(results_dir, session_id) {
        candidates.extend(result_candidates_from_index_dir(results_dir, &dir).await?);
    }
    // A staged payload whose index has not landed yet has no entry to enumerate, so it is
    // discovered by its own directory and resolved the same way.
    for file in pending_result_files_for_session(results_dir, session_id).await? {
        let run_id = RunId::from_token(file.stem());
        if candidates.iter().any(|c| c.run_id == run_id) {
            continue;
        }
        let payload = locate::resolve_payload(results_dir, session_id, &run_id, &file).await;
        candidates.push(ResultCandidate {
            run_id,
            session_id: session_id.clone(),
            file,
            async_dir: None,
            written_at: 0,
            payload,
        });
    }
    Ok(candidates)
}

/// Candidates recorded against one tool call.
///
/// pi `resultCandidateFilesForToolCall` (`result-files.ts:461-463`).
///
/// # Errors
///
/// As [`list_index_files`].
pub async fn result_candidate_files_for_tool_call(
    results_dir: &Path,
    tool_call_id: &str,
) -> std::io::Result<BTreeSet<ResultFileName>> {
    let dir = paths::tool_call_index_dir(results_dir, tool_call_id);
    result_files_from_index_dir(results_dir, &dir, true).await
}

/// Candidates in the cross-session mission observer band.
///
/// pi `missionObserverResultCandidateFiles` (`result-files.ts:469-471`) — candidate source #3 for
/// the watcher, and the **only** enumeration that deliberately spans sessions. Mission
/// reconciliation needs to see a completion whichever instance launched it; serving that from this
/// narrow, explicitly-populated index is what makes it possible without widening the ownership
/// predicate to every result in the directory.
///
/// # Errors
///
/// As [`list_index_files`].
pub async fn mission_observer_result_candidate_files(
    results_dir: &Path,
) -> std::io::Result<BTreeSet<ResultFileName>> {
    let dir = paths::mission_observer_dir(results_dir);
    result_files_from_index_dir(results_dir, &dir, true).await
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::write::{
        ResultWrite, write_async_result_file, write_pending_async_result_file,
    };
    use super::*;

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        run_id: String,
        session_id: String,
    }

    fn payload(run: &str, sess: &str) -> Payload {
        Payload {
            run_id: run.to_string(),
            session_id: sess.to_string(),
        }
    }

    fn names(files: &BTreeSet<ResultFileName>) -> Vec<&str> {
        files.iter().map(ResultFileName::as_str).collect()
    }

    #[tokio::test]
    async fn an_empty_results_dir_enumerates_to_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let found = result_candidate_files_for_session(tmp.path(), &session("s1"))
            .await
            .expect("a missing index tree is not an error");
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn a_session_sees_only_its_own_results() {
        // THE property this whole module exists for.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &RunId::from_token("runa"),
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("runa", "s1"),
        )
        .await
        .expect("a");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s2"),
                run_id: &RunId::from_token("runb"),
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("runb", "s2"),
        )
        .await
        .expect("b");

        let a = result_candidate_files_for_session(tmp.path(), &session("s1"))
            .await
            .expect("a");
        let b = result_candidate_files_for_session(tmp.path(), &session("s2"))
            .await
            .expect("b");
        assert_eq!(names(&a), vec!["runa.json"]);
        assert_eq!(names(&b), vec!["runb.json"]);
    }

    #[tokio::test]
    async fn a_third_session_sees_nothing_even_though_both_payloads_are_public() {
        // Both payloads are sitting in the shared results dir; only the index scopes them.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &RunId::from_token("runa"),
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("runa", "s1"),
        )
        .await
        .expect("a");
        assert!(
            paths::result_owned_path(tmp.path(), &session("s1"), &RunId::from_token("runa"))
                .is_file(),
            "the payload is promoted into its owner's partition"
        );

        let c = result_candidate_files_for_session(tmp.path(), &session("s3"))
            .await
            .expect("c");
        assert!(c.is_empty(), "a foreign session must enumerate nothing");
    }

    #[tokio::test]
    async fn candidates_include_staged_payloads_but_public_only_does_not() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        let candidates = result_candidate_files_for_session(tmp.path(), &session("s1"))
            .await
            .expect("c");
        assert_eq!(
            names(&candidates),
            vec!["run1.json"],
            "a staged result is still a completion"
        );

        // `result_files_for_session` promotes on read (locate does), so re-stage for the negative.
        let tmp2 = tempfile::tempdir().expect("tempdir");
        let staged = paths::result_pending_path(tmp2.path(), &session("s1"), &run);
        tokio::fs::create_dir_all(staged.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(&staged, br#"{"runId":"run1","sessionId":"s1"}"#)
            .await
            .expect("seed");
        let public_only = result_files_for_session(tmp2.path(), &session("s1"))
            .await
            .expect("p");
        assert!(
            public_only.is_empty(),
            "no index entry, so nothing public to report"
        );
    }

    #[tokio::test]
    async fn a_staged_payload_with_no_index_is_still_discovered() {
        // The write order is payload-then-index, so the staging dir is authoritative for the gap.
        let tmp = tempfile::tempdir().expect("tempdir");
        let staged =
            paths::result_pending_path(tmp.path(), &session("s1"), &RunId::from_token("run1"));
        tokio::fs::create_dir_all(staged.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(&staged, br#"{"runId":"run1","sessionId":"s1"}"#)
            .await
            .expect("seed");

        let found = result_candidate_files_for_session(tmp.path(), &session("s1"))
            .await
            .expect("f");
        assert_eq!(names(&found), vec!["run1.json"]);
    }

    #[tokio::test]
    async fn a_staged_payload_claiming_another_session_is_not_returned() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let staged =
            paths::result_pending_path(tmp.path(), &session("s1"), &RunId::from_token("run1"));
        tokio::fs::create_dir_all(staged.parent().expect("parent"))
            .await
            .expect("mkdir");
        // Sitting in s1's directory but claiming s2 — contents decide, not the directory name.
        tokio::fs::write(&staged, br#"{"runId":"run1","sessionId":"s2"}"#)
            .await
            .expect("seed");

        let found = result_candidate_files_for_session(tmp.path(), &session("s1"))
            .await
            .expect("f");
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn an_unparseable_index_entry_is_swept_not_re_read_forever() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = paths::session_index_dir(tmp.path(), &session("s1"));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let junk = dir.join("garbage.json");
        tokio::fs::write(&junk, b"not json").await.expect("seed");

        let found = result_candidate_files_for_session(tmp.path(), &session("s1"))
            .await
            .expect("f");
        assert!(found.is_empty());
        assert!(!junk.exists(), "a junk index entry must be removed");
    }

    #[tokio::test]
    async fn non_json_files_in_an_index_dir_are_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = paths::session_index_dir(tmp.path(), &session("s1"));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        tokio::fs::write(dir.join("README.txt"), b"hi")
            .await
            .expect("seed");

        assert!(
            result_candidate_files_for_session(tmp.path(), &session("s1"))
                .await
                .expect("f")
                .is_empty()
        );
        assert!(
            dir.join("README.txt").exists(),
            "a non-index file must not be touched"
        );
    }

    #[tokio::test]
    async fn the_mission_observer_band_spans_sessions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let async_dir = tmp.path().join("adir");
        tokio::fs::create_dir_all(&async_dir).await.expect("mkdir");
        tokio::fs::write(async_dir.join("mission.json"), b"{}")
            .await
            .expect("bind");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: Some(&async_dir),
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        // s2 cannot see it as its own...
        assert!(
            result_candidate_files_for_session(tmp.path(), &session("s2"))
                .await
                .expect("s2")
                .is_empty()
        );
        // ...but the observer band exposes it to every instance.
        let observed = mission_observer_result_candidate_files(tmp.path())
            .await
            .expect("obs");
        assert_eq!(names(&observed), vec!["run1.json"]);
    }

    #[tokio::test]
    async fn a_non_mission_run_is_not_in_the_observer_band() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &RunId::from_token("run1"),
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");
        assert!(
            mission_observer_result_candidate_files(tmp.path())
                .await
                .expect("obs")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn tool_call_candidates_are_scoped_to_their_id() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &RunId::from_token("run1"),
                written_at: 1,
                async_dir: None,
                tool_call_id: Some("tc1"),
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        let found = result_candidate_files_for_tool_call(tmp.path(), "tc1")
            .await
            .expect("f");
        assert_eq!(names(&found), vec!["run1.json"]);
        assert!(
            result_candidate_files_for_tool_call(tmp.path(), "tc2")
                .await
                .expect("f")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_session_whose_id_is_a_path_enumerates_normally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sid = session("/home/u/s.jsonl");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &sid,
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "/home/u/s.jsonl"),
        )
        .await
        .expect("write");

        let found = result_candidate_files_for_session(tmp.path(), &sid)
            .await
            .expect("f");
        assert_eq!(names(&found), vec!["run1.json"]);
    }
}
