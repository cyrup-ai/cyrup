//! Index removal.
//!
//! Ports pi `removeResultIndex` (`result-files.ts:185-219`) and `removeMissionObserverIndex`
//! (`:222-229`).

use std::path::Path;

use crate::background::RunId;
use crate::identity::SessionId;

use super::paths;

/// Clear every index entry for one run.
///
/// pi `removeResultIndex` (`result-files.ts:185-219`).
///
/// # Why this must run on delete
///
/// Under the index-driven enumerator, a leftover index entry is not a harmless stale file: it is a
/// permanent **candidate**. Every subsequent scan re-reads it, fails to find a payload, and tries
/// again — so an unremoved index turns a delivered-and-deleted run into an unbounded re-scan cost
/// and keeps resurrecting it in candidate lists. Deleting the payload without the indexes is
/// therefore not "leaving some cruft"; it is a leak with a per-scan price.
///
/// # Every unlink is independently best-effort
///
/// pi wraps each `rmSync` in its own `try`/`catch` with the comment "Index cleanup must not affect
/// result delivery." That is the contract: this runs **after** a successful delivery, so failing
/// here must never turn a delivered result into an error. Nothing is returned and nothing can
/// fail.
///
/// Both the session and pending fan-outs are swept, not just the current write location — an entry
/// written under a previous encoding rule must still be removable.
pub async fn remove_result_index(
    results_dir: &Path,
    session_id: Option<&SessionId>,
    run_id: &RunId,
    tool_call_id: Option<&str>,
) {
    if let Some(session_id) = session_id {
        // pi `:188-193` — every session-index alias.
        for index_path in paths::result_index_paths(results_dir, session_id, run_id) {
            remove_quietly(&index_path).await;
        }
        // pi `:194-200` — every staged-payload alias. A staged copy left behind would be
        // re-promoted by the next reader and re-delivered as a fresh completion.
        for pending_path in paths::result_pending_paths(results_dir, session_id, run_id) {
            remove_quietly(&pending_path).await;
        }
        // The promoted payload, for the same reason as the staged one: it is the file a delivered
        // completion was read from, and leaving it would re-deliver the run on the next scan.
        // [CYRUP-DELTA] upstream unlinks the ROOT payload here (`:186-187`); this port promotes
        // into a session partition, so the owned fan-out is the equivalent sweep. The root path is
        // swept below for payloads written by builds that predate the partition.
        for owned_path in paths::result_owned_paths(results_dir, session_id, run_id) {
            remove_quietly(&owned_path).await;
        }
    }
    // pi `:186-187` — the legacy root payload. Unconditional and session-independent: an older
    // build wrote it there, and it is addressed by run id alone.
    remove_quietly(&crate::identity::ResultFileName::for_run(run_id).resolve_in(results_dir)).await;
    // pi `:202-206` — the run accelerator, removable without a session.
    remove_quietly(&paths::run_index_path(results_dir, run_id)).await;
    // pi `:207-213` — the tool-call accelerator, when the caller knows the id.
    if let Some(tool_call_id) = tool_call_id.filter(|id| !id.is_empty()) {
        remove_quietly(&paths::tool_call_index_path(results_dir, tool_call_id, run_id)).await;
    }
    // pi `:214-218` — the observer band is always cleared, mission-bound or not: an unconditional
    // unlink of a path that usually does not exist is cheaper and safer than deciding first.
    remove_quietly(&paths::mission_observer_path(results_dir, run_id)).await;
}

/// Clear only the mission observer entry for one run.
///
/// pi `removeMissionObserverIndex` (`result-files.ts:222-229`), called from the watcher once the
/// observer band has run successfully (`result-watcher.ts:425`) — **before** the ownership check,
/// so a foreign-session result that this instance merely observed still retires its own
/// cross-session obligation without touching the payload its owner still needs.
pub async fn remove_mission_observer_index(results_dir: &Path, run_id: &RunId) {
    remove_quietly(&paths::mission_observer_path(results_dir, run_id)).await;
}

/// `rm -f` semantics: absent is success, and any other failure is swallowed.
async fn remove_quietly(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::debug!(
                path = %path.display(),
                %error,
                "index cleanup failed; delivery is unaffected"
            );
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
    use super::super::write::{ResultWrite, write_async_result_file, write_pending_async_result_file};

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        run_id: String,
        session_id: String,
    }

    fn payload() -> Payload {
        Payload { run_id: "run1".to_string(), session_id: "s1".to_string() }
    }

    #[tokio::test]
    async fn removing_clears_session_run_tool_call_and_observer_indexes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let async_dir = tmp.path().join("adir");
        tokio::fs::create_dir_all(&async_dir).await.expect("mkdir");
        tokio::fs::write(async_dir.join("mission.json"), b"{}").await.expect("bind");

        write_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: Some(&async_dir),
            tool_call_id: Some("tc1"),
        }, &payload())
        .await
        .expect("write");

        // All four exist first.
        assert!(paths::result_index_path(tmp.path(), &session("s1"), &run).is_file());
        assert!(paths::run_index_path(tmp.path(), &run).is_file());
        assert!(paths::tool_call_index_path(tmp.path(), "tc1", &run).is_file());
        assert!(paths::mission_observer_path(tmp.path(), &run).is_file());

        remove_result_index(tmp.path(), Some(&session("s1")), &run, Some("tc1")).await;

        assert!(!paths::result_index_path(tmp.path(), &session("s1"), &run).exists(), "session");
        assert!(!paths::run_index_path(tmp.path(), &run).exists(), "run");
        assert!(!paths::tool_call_index_path(tmp.path(), "tc1", &run).exists(), "tool-call");
        assert!(!paths::mission_observer_path(tmp.path(), &run).exists(), "observer");
    }

    #[tokio::test]
    async fn removing_also_clears_a_staged_payload() {
        // A surviving staged copy would be re-promoted and re-delivered as a new completion.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        }, &payload())
            .await
            .expect("write");
        assert!(paths::result_pending_path(tmp.path(), &session("s1"), &run).is_file());

        remove_result_index(tmp.path(), Some(&session("s1")), &run, None).await;
        assert!(!paths::result_pending_path(tmp.path(), &session("s1"), &run).exists());
    }

    #[tokio::test]
    async fn removing_without_a_session_still_clears_the_run_and_observer_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        }, &payload())
            .await
            .expect("write");

        remove_result_index(tmp.path(), None, &run, None).await;
        assert!(!paths::run_index_path(tmp.path(), &run).exists());
        // The session entry survives — there was no session to address it by.
        assert!(paths::result_index_path(tmp.path(), &session("s1"), &run).is_file());
    }

    #[tokio::test]
    async fn removing_twice_is_a_no_op() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        }, &payload())
            .await
            .expect("write");
        remove_result_index(tmp.path(), Some(&session("s1")), &run, None).await;
        // Must not panic or error — a crash between delivery and cleanup replays this.
        remove_result_index(tmp.path(), Some(&session("s1")), &run, None).await;
    }

    #[tokio::test]
    async fn removing_from_an_empty_dir_is_a_no_op() {
        let tmp = tempfile::tempdir().expect("tempdir");
        remove_result_index(tmp.path(), Some(&session("s1")), &RunId::from_token("x"), Some("t"))
            .await;
        remove_mission_observer_index(tmp.path(), &RunId::from_token("x")).await;
    }

    #[tokio::test]
    async fn the_observer_only_removal_leaves_the_payload_and_session_index_alone() {
        // The foreign-session case: observe, retire the mission obligation, touch nothing else.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let async_dir = tmp.path().join("adir");
        tokio::fs::create_dir_all(&async_dir).await.expect("mkdir");
        tokio::fs::write(async_dir.join("mission.json"), b"{}").await.expect("bind");
        write_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: Some(&async_dir),
            tool_call_id: None,
        }, &payload())
        .await
        .expect("write");

        remove_mission_observer_index(tmp.path(), &run).await;

        assert!(!paths::mission_observer_path(tmp.path(), &run).exists(), "observer cleared");
        assert!(
            paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file(),
            "payload must survive for its owner"
        );
        assert!(
            paths::result_index_path(tmp.path(), &session("s1"), &run).is_file(),
            "the owner's session index must survive"
        );
    }
}
