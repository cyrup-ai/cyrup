//! The write protocol: stage session-private, index, then promote.
//!
//! Ports pi `writeResultIndexForData` (`result-files.ts:129-161`),
//! `writeIndexedPendingResultFile` (`:164-172`), `writePendingAsyncResultFile` (`:174-176`) and
//! `writeAsyncResultFile` (`:178-183`).
//!
//! Index-entry writing lives here rather than in a file of its own because
//! `write_indexed_pending_result_file` is its only non-test caller: the index write IS step 2 of
//! the protocol, not a separable job. Splitting them produced the `write.rs`/`stage.rs` pair this
//! layout removes.

use std::path::Path;

use crate::background::RunId;
use crate::background::atomic::write_atomic_json_creating_parent;
use crate::identity::{ResultFileName, SessionId};

use super::entry::{IndexVersion, ResultIndexEntry};
use super::locate::PayloadState;
use super::paths;
use super::promote::{self, PromotionState};

/// The marker file that makes a run mission-bound.
///
/// pi imports `MISSION_BINDING_FILE` from `missions/lifecycle.ts`; cyrup's equivalent constant
/// lives with the missions module. Named here so the observer-index condition reads the same as
/// upstream's (`result-files.ts:154`).
const MISSION_BINDING_FILE: &str = "mission.json";

/// Writes the session, run, tool-call and mission-observer index entries for one result payload.
///
/// pi `writeResultIndexForData` (`result-files.ts:129-161`).
///
/// # Silent on a missing identity — deliberately, and unlike the writer above it
///
/// pi returns early without error when the run id or session id is absent (`:132`), while
/// [`write_indexed_pending_result_file`] **throws** for the same condition (`:166`).
/// Both behaviours are ported and they must not be unified: this function is a best-effort helper
/// that any caller may invoke on any payload, whereas the staging writer is the single chokepoint
/// that must refuse to create an unattributable — and therefore permanently invisible — result.
///
/// # Index-failure isolation
///
/// The **session** index write is unguarded: it is the partition every reader enumerates, so a
/// failure there is a failed write. Every index after it is individually isolated (pi's three
/// separate `try`/`catch` blocks at `:145`, `:151`, `:157`), because they are lookup accelerators
/// — losing one degrades a query path, but losing the result is not an acceptable trade for it.
///
/// # Errors
///
/// Only a session-index write failure. Auxiliary index failures are logged and swallowed.
pub(crate) async fn write_result_index_for_data(request: &ResultWrite<'_>) -> std::io::Result<()> {
    let ResultWrite {
        results_dir,
        session_id,
        run_id,
        written_at,
        async_dir,
        tool_call_id,
    } = *request;
    let entry = ResultIndexEntry {
        version: IndexVersion,
        run_id: run_id.clone(),
        session_id: session_id.clone(),
        file: request.file(),
        written_at,
        async_dir: async_dir.map(Path::to_path_buf),
    };

    // The session partition — unguarded on purpose (see the doc comment).
    write_atomic_json_creating_parent(
        &paths::result_index_path(results_dir, session_id, run_id),
        &entry,
    )
    .await?;

    // pi `:145-149` — lookup by run id.
    if let Err(error) =
        write_atomic_json_creating_parent(&paths::run_index_path(results_dir, run_id), &entry).await
    {
        tracing::warn!(
            run_id = %run_id,
            %error,
            "failed to write async result run index; delivery is unaffected"
        );
    }

    // pi `:150-155` — lookup by tool call, only when the payload carries one.
    if let Some(tool_call_id) = tool_call_id.filter(|id| !id.is_empty())
        && let Err(error) = write_atomic_json_creating_parent(
            &paths::tool_call_index_path(results_dir, tool_call_id, run_id),
            &entry,
        )
        .await
    {
        tracing::warn!(
            run_id = %run_id,
            %error,
            "failed to write async result tool-call index; delivery is unaffected"
        );
    }

    // pi `:156-161` — the mission observer band, written ONLY for a mission-bound run. This is the
    // one index that is read cross-session, so writing it for every run would hand every instance
    // a candidate list containing every other instance's results and undo the partitioning.
    if let Some(async_dir) = async_dir
        && tokio::fs::try_exists(async_dir.join(MISSION_BINDING_FILE))
            .await
            .unwrap_or(false)
        && let Err(error) = write_atomic_json_creating_parent(
            &paths::mission_observer_path(results_dir, run_id),
            &entry,
        )
        .await
    {
        tracing::warn!(
            run_id = %run_id,
            %error,
            "failed to write async result mission-observer index; delivery is unaffected"
        );
    }

    Ok(())
}

/// The addressing and metadata one result write needs, separate from the payload itself.
///
/// A struct rather than a parameter list because the same six values travel together through
/// staging, indexing and promotion, and the two identity fields ([`SessionId`] and the run id)
/// must not be transposable at a call site. Named fields also make the eventual writers
/// (`runner_main/finish.rs`, `background/reconcile.rs`) read as a description of the result rather
/// than as a positional argument sequence.
#[derive(Clone, Copy, Debug)]
pub struct ResultWrite<'a> {
    /// The shared per-cwd results directory.
    pub results_dir: &'a Path,
    /// The **launching orchestrator's** session — the partition key. Required: a result that
    /// cannot be attributed cannot be indexed, and so could never be found or delivered.
    pub session_id: &'a SessionId,
    /// The run this result belongs to.
    pub run_id: &'a RunId,
    /// Epoch milliseconds stamped into the index entry; the retention sweep's age input.
    pub written_at: i64,
    /// The run's async directory, when known. Also decides whether the run is mission-bound and
    /// therefore enters the cross-session observer band.
    pub async_dir: Option<&'a Path>,
    /// The originating tool call, when there is one — populates the tool-call accelerator index.
    pub tool_call_id: Option<&'a str>,
}

impl<'a> ResultWrite<'a> {
    /// The public payload file name for this write.
    #[must_use]
    pub fn file(&self) -> ResultFileName {
        ResultFileName::for_run(self.run_id)
    }
}

/// Stage the payload session-private and index it — pi `writeIndexedPendingResultFile`
/// (`result-files.ts:164-172`).
///
/// # Visibility is the ordering guarantee
///
/// This is `pub(super)` on purpose, and that visibility is load-bearing rather than stylistic.
/// Making the three steps individually public would let a caller promote without indexing,
/// producing a public payload that no index references: invisible to the enumerator, unreachable
/// by any lookup, and undeletable by [`super::remove`] — a permanent leak in a directory that is
/// swept only by age. Only [`write_async_result_file`] and [`write_pending_async_result_file`] are
/// public, and both go through here.
///
/// # Errors
///
/// Propagates a staging or session-index write failure. The caller is expected to treat this as
/// fatal for the result.
pub(super) async fn write_indexed_pending_result_file(
    request: &ResultWrite<'_>,
    payload: &(impl serde::Serialize + Sync),
) -> std::io::Result<()> {
    // pi `:169` — the payload lands session-private FIRST. Until the index below exists, nothing
    // can find this file; until the promotion after it, nothing can find it by the public path.
    write_atomic_json_creating_parent(
        &paths::result_pending_path(request.results_dir, request.session_id, request.run_id),
        payload,
    )
    .await?;

    write_result_index_for_data(request).await
}

/// Stage and index without promoting — pi `writePendingAsyncResultFile` (`:174-176`).
///
/// Used mid-run, when a result exists but the run is not finished: the payload is recoverable by
/// its owning session but is not yet a completion anyone should be notified about.
///
/// # Errors
///
/// As [`write_indexed_pending_result_file`].
pub async fn write_pending_async_result_file(
    request: &ResultWrite<'_>,
    payload: &(impl serde::Serialize + Sync),
) -> std::io::Result<()> {
    write_indexed_pending_result_file(request, payload).await
}

/// The terminal result write: stage -> index -> promote, in that order.
///
/// pi `writeAsyncResultFile` (`result-files.ts:178-183`).
///
/// # Why a missing session is a hard error
///
/// The caller must supply a [`SessionId`]; there is no `Option` here. pi throws for the same
/// condition (`:166`), and the reason is structural rather than stylistic: an unattributable
/// result cannot be indexed, and an unindexed result is invisible to every reader — it would be
/// written, never delivered, never found by a lookup, and never deleted except by the age sweep.
/// Failing loudly at the write is strictly better than producing a file whose only future is to
/// be garbage.
///
/// # Errors
///
/// Propagates a staging or session-index failure. A promotion failure is **not** an error — it is
/// reported as [`PayloadState::Staged`], because the payload is safely written and a later reader
/// will promote it ([`super::locate`] promotes on read).
pub async fn write_async_result_file(
    request: &ResultWrite<'_>,
    payload: &(impl serde::Serialize + Sync),
) -> std::io::Result<PayloadState> {
    write_indexed_pending_result_file(request, payload).await?;

    // pi `:180-182`: only `promoted` reports the payload published; `none` and `pending` both
    // report it still staged.
    let promotion = promote::promote_pending_result_file(
        request.results_dir,
        request.session_id,
        request.run_id,
        true,
    )
    .await;
    Ok(match promotion {
        PromotionState::Promoted => PayloadState::Owned,
        PromotionState::None | PromotionState::Pending => PayloadState::Staged,
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

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    #[derive(serde::Serialize)]
    struct Payload {
        run_id: String,
    }

    fn payload() -> Payload {
        Payload {
            run_id: "run1".to_string(),
        }
    }

    async fn write_for(dir: &Path, async_dir: Option<&Path>, tool_call: Option<&str>) -> RunId {
        let run = RunId::from_token("run1");
        write_result_index_for_data(&ResultWrite {
            results_dir: dir,
            session_id: &session("s1"),
            run_id: &run,
            written_at: 42,
            async_dir,
            tool_call_id: tool_call,
        })
        .await
        .expect("index write succeeds");
        run
    }

    #[tokio::test]
    async fn the_session_and_run_indexes_are_always_written() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = write_for(tmp.path(), None, None).await;

        let session_entry = paths::result_index_path(tmp.path(), &session("s1"), &run);
        assert!(session_entry.is_file(), "session index missing");
        assert!(
            paths::run_index_path(tmp.path(), &run).is_file(),
            "run index missing"
        );
    }

    #[tokio::test]
    async fn the_written_entry_round_trips_with_every_field() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = write_for(tmp.path(), Some(Path::new("/tmp/adir")), None).await;

        let bytes = tokio::fs::read(paths::result_index_path(tmp.path(), &session("s1"), &run))
            .await
            .expect("read back");
        let entry = ResultIndexEntry::parse(&bytes).expect("parses");
        assert_eq!(entry.run_id, run);
        assert_eq!(entry.session_id, session("s1"));
        assert_eq!(entry.file.as_str(), "run1.json");
        assert_eq!(entry.written_at, 42);
        assert_eq!(entry.async_dir.as_deref(), Some(Path::new("/tmp/adir")));
    }

    #[tokio::test]
    async fn the_tool_call_index_is_written_only_when_an_id_is_supplied() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = write_for(tmp.path(), None, Some("tc1")).await;
        assert!(paths::tool_call_index_path(tmp.path(), "tc1", &run).is_file());

        let tmp2 = tempfile::tempdir().expect("tempdir");
        let run2 = write_for(tmp2.path(), None, None).await;
        assert!(!paths::tool_call_index_dir(tmp2.path(), "tc1").exists());
        let _ = run2;
    }

    #[tokio::test]
    async fn an_empty_tool_call_id_writes_no_tool_call_index() {
        // pi's `nonEmptyString(data.toolCallId)` guard — "" is not an id.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_for(tmp.path(), None, Some("")).await;
        assert!(
            !paths::result_index_root(tmp.path())
                .join("tool-calls")
                .exists()
        );
    }

    #[tokio::test]
    async fn the_mission_observer_index_is_written_only_for_a_mission_bound_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run-a");
        tokio::fs::create_dir_all(&async_dir).await.expect("mkdir");

        // No binding file yet -> no observer entry.
        let run = write_for(tmp.path(), Some(&async_dir), None).await;
        assert!(
            !paths::mission_observer_path(tmp.path(), &run).exists(),
            "a non-mission run must not enter the cross-session observer band"
        );

        // Bind it, rewrite -> observer entry appears.
        tokio::fs::write(async_dir.join(MISSION_BINDING_FILE), b"{}")
            .await
            .expect("bind");
        let run = write_for(tmp.path(), Some(&async_dir), None).await;
        assert!(paths::mission_observer_path(tmp.path(), &run).is_file());
    }

    #[tokio::test]
    async fn a_session_id_that_is_a_path_lands_inside_the_results_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sid = session("/home/u/s.jsonl");
        let run = RunId::from_token("run1");
        write_result_index_for_data(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &sid,
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        })
        .await
        .expect("write");

        let written = paths::result_index_path(tmp.path(), &sid, &run);
        assert!(written.is_file());
        assert!(written.starts_with(tmp.path()), "escaped to {written:?}");
    }

    #[tokio::test]
    async fn a_terminal_write_stages_indexes_and_promotes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let state = write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");

        assert_eq!(state, PayloadState::Owned);
        assert!(
            paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file(),
            "promoted payload missing from the owning session's partition"
        );
        assert!(
            !tmp.path().join("run1.json").exists(),
            "nothing may be published into the shared results root any more"
        );
        assert!(
            paths::result_index_path(tmp.path(), &session("s1"), &run).is_file(),
            "session index missing"
        );
        assert!(
            !paths::result_pending_path(tmp.path(), &session("s1"), &run).exists(),
            "the staged copy must be gone after promotion"
        );
    }

    #[tokio::test]
    async fn the_index_exists_before_the_payload_is_public() {
        // The ordering guarantee: a pending write indexes but does not publish, so a reader can
        // always find a payload it can see, and can never see one it cannot find.
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
            &payload(),
        )
        .await
        .expect("write");

        assert!(paths::result_index_path(tmp.path(), &session("s1"), &run).is_file());
        assert!(paths::result_pending_path(tmp.path(), &session("s1"), &run).is_file());
        assert!(
            !paths::result_owned_path(tmp.path(), &session("s1"), &run).exists(),
            "must not be promoted yet"
        );
    }

    #[tokio::test]
    async fn two_sessions_stage_into_separate_partitions() {
        // The partitioning property this whole module exists for.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run_a = RunId::from_token("runa");
        let run_b = RunId::from_token("runb");
        write_pending_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run_a,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("a");
        write_pending_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s2"),
                run_id: &run_b,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("b");

        let dir_a = paths::pending_session_dir(tmp.path(), &session("s1"));
        let dir_b = paths::pending_session_dir(tmp.path(), &session("s2"));
        assert_ne!(dir_a, dir_b, "sessions must not share a staging directory");
        assert!(dir_a.join("runa.json").is_file());
        assert!(
            !dir_a.join("runb.json").exists(),
            "s1 must not see s2's payload"
        );
    }

    #[tokio::test]
    async fn a_terminal_write_reports_the_payload_public() {
        // `write_async_result_file` collapses PromotionState's three outcomes onto PayloadState's
        // two (pi `result-files.ts:180-182`); this pins the mapping for the success arm.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let state = write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");
        assert_eq!(state, PayloadState::Owned);
    }
}
