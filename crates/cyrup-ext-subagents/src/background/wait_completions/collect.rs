//! [`collect_wait_completions`] — the three-rung resolution a `wait` reads its terminal
//! completions through.

use std::path::Path;

use crate::background::RunStatus;
use crate::background::completion_replay;
use crate::background::result_index::{self, errno};
use crate::error::SubagentError;
use crate::identity::ResultFileName;

use super::project::{WaitCompletion, to_wait_completion};
use super::record::WaitCompletionStore;

/// Terminal payloads for the runs a wait covered.
///
/// pi `collectWaitCompletions` (`wait-completions.ts:160-213`). Read-only by contract: the watcher
/// owns notification and cleanup, and a payload is written atomically
/// ([`crate::background::atomic`]), so a direct read never observes a torn write. Nothing here
/// mints or touches a [`crate::background::result_index::ConsumablePayload`] — a wait must not be
/// able to destroy what it reads.
///
/// # Resolution order, per run
///
/// 1. **the in-process record** ([`WaitCompletionStore`]) — the watcher may already have consumed
///    and deleted the payload;
/// 2. **the payload on disk**, addressed through the session index
///    ([`result_index::result_payload_path_for_session_run`]) when the run has a session and
///    falling back to the public path — this is what makes a **staged** payload visible to its
///    owner and another session's payload invisible; an unattributed run is not indexed anywhere,
///    so for it the public path is the only address;
/// 3. **the durable replay record** ([`completion_replay::read_completion_replay`]) when the
///    payload is absent and the in-process record has expired or belongs to an earlier process.
///    It is written before the unlink, which is why it can answer at all (pi `:202-209`).
///
/// # Errors
///
/// A read fault that is not absence, any JSON parse failure, and any projection rejection. An
/// access-denied fault from the index read is recovered once via
/// [`result_index::fallback_result_payload_path_for_session_run`] before being surfaced.
pub async fn collect_wait_completions(
    terminal: &[RunStatus],
    store: &WaitCompletionStore,
    results_dir: &Path,
) -> Result<Vec<WaitCompletion>, SubagentError> {
    let mut out = Vec::with_capacity(terminal.len());

    for run in terminal {
        if let Some(recorded) = store.get(&run.run_id) {
            out.push(recorded);
            continue;
        }

        let file = ResultFileName::for_run(&run.run_id);
        let public = file.resolve_in(results_dir);
        let path = match run.session_id.as_ref() {
            // An unattributed run is not indexed anywhere; the public path is the only address.
            None => public.clone(),
            Some(session) => {
                match result_index::result_payload_path_for_session_run(
                    results_dir,
                    session,
                    &run.run_id,
                )
                .await
                {
                    Ok(found) => found.unwrap_or_else(|| public.clone()),
                    // A denied index read retries once against the STAGED location alone — the
                    // recovery path is still reachable because it is addressed directly rather
                    // than through the unreadable index directory.
                    Err(error) if errno::is_access_denied(&error) => {
                        result_index::fallback_result_payload_path_for_session_run(
                            results_dir,
                            session,
                            &run.run_id,
                        )
                        .await
                        .unwrap_or_else(|| public.clone())
                    }
                    Err(error) => return Err(SubagentError::Spawn(error)),
                }
            }
        };

        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
                    SubagentError::Spawn(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error,
                    ))
                })?;
                let completion = to_wait_completion(&value, &run.run_id).map_err(|error| {
                    SubagentError::Spawn(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error,
                    ))
                })?;
                out.push(completion);
            }
            Err(error) if errno::is_absent(&error) => {
                // pi `:194-209`. The watcher consumed the file between the store check at the top
                // of this loop and this read.
                //
                // Rung 2 — the in-process record, RE-checked. Not redundant with the check above:
                // the watcher's drain loop runs on its own task and the window between the two
                // reads is real (pi `:194-196`).
                if let Some(late) = store.get(&run.run_id) {
                    out.push(late);
                    continue; // pi `:200` — one run contributes exactly one completion
                }
                // Rung 3 — the durable replay, written BEFORE the unlink so a `wait` landing after
                // delete-last, or in a LATER PROCESS entirely, still reports the completion
                // (pi `:203`).
                //
                // `session_id` is passed through as-is, `None` included: it is an OPTIONAL filter
                // (`completion-replay.ts:228`), and an unattributed run must still be able to read
                // its own record.
                if let Some(replay) = completion_replay::read_completion_replay(
                    results_dir,
                    &run.run_id,
                    completion_replay::ReplayReadFilter {
                        session_id: run.session_id.as_ref(),
                        now: None,
                    },
                )
                .await
                {
                    out.push(replay.completion);
                    continue;
                }
                // Nothing at all: the payload is gone and the record has expired or was never
                // written. The run contributes no completion and that is NOT an error — upstream's
                // own outcome, whose `if (replay)` at `:204` has no `else`.
            }
            Err(error) => return Err(SubagentError::Spawn(error)),
        }
    }

    Ok(out)
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
    use crate::background::{RunId, RunMode};
    use serde_json::json;

    fn run_status(run_id: &str, session: Option<&str>) -> RunStatus {
        let mut status = RunStatus::queued(RunId::from_token(run_id), RunMode::Single, Some(1));
        status.session_id =
            session.map(|raw| crate::identity::SessionId::parse(raw).expect("non-empty session"));
        status
    }

    /// Writes a payload at the PUBLIC path — the address rungs 2 and 3 both fall through to.
    async fn write_public(results_dir: &Path, run_id: &str, payload: &serde_json::Value) {
        let path = ResultFileName::for_run(&RunId::from_token(run_id)).resolve_in(results_dir);
        tokio::fs::write(&path, payload.to_string())
            .await
            .expect("write payload");
    }

    /// Rung 1, and its PRECEDENCE. The watcher may already have consumed and deleted the payload,
    /// so the record has to answer when nothing is on disk — but the record must also WIN when a
    /// file is present, because it is the projection taken at consumption time while whatever
    /// still sits at the public path may be a stale or foreign write. Checking the store first is
    /// therefore load-bearing ordering, not an optimisation.
    #[tokio::test]
    async fn the_in_process_record_answers_first_even_when_a_payload_is_on_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = WaitCompletionStore::default();
        store
            .record(
                &RunId::from_token("r1"),
                &json!({ "agent": "from-the-store" }),
                1_000,
                600_000,
                None,
            )
            .await;
        write_public(tmp.path(), "r1", &json!({ "agent": "from-the-disk" })).await;

        let collected =
            collect_wait_completions(&[run_status("r1", Some("s1"))], &store, tmp.path())
                .await
                .expect("collects");
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].run_id, "r1");
        assert_eq!(collected[0].agent.as_deref(), Some("from-the-store"));

        // And with the payload gone — the deleted-underneath-us case the store exists to survive.
        tokio::fs::remove_file(
            ResultFileName::for_run(&RunId::from_token("r1")).resolve_in(tmp.path()),
        )
        .await
        .expect("remove payload");
        let collected =
            collect_wait_completions(&[run_status("r1", Some("s1"))], &store, tmp.path())
                .await
                .expect("collects");
        assert_eq!(collected[0].agent.as_deref(), Some("from-the-store"));
    }

    /// Rung 3. An unattributed run is not indexed anywhere, so the public path is the only address
    /// — and rung 2 falls through to the same place when the index knows nothing about the run.
    /// Both spellings of "no index entry" must resolve, or a completed run reads as absent.
    #[tokio::test]
    async fn an_unindexed_run_resolves_at_the_public_path_with_or_without_a_session() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = WaitCompletionStore::default();
        write_public(tmp.path(), "no-session", &json!({ "agent": "a" })).await;
        write_public(tmp.path(), "with-session", &json!({ "agent": "b" })).await;

        let collected = collect_wait_completions(
            &[
                run_status("no-session", None),
                run_status("with-session", Some("s1")),
            ],
            &store,
            tmp.path(),
        )
        .await
        .expect("collects");

        let agents: Vec<Option<&str>> = collected
            .iter()
            .map(|completion| completion.agent.as_deref())
            .collect();
        assert_eq!(agents, vec![Some("a"), Some("b")]);
        // Order is the order asked, so a caller can zip completions back against its own runs.
        assert_eq!(collected[0].run_id, "no-session");
        assert_eq!(collected[1].run_id, "with-session");
    }

    /// Read-only by contract: "a wait must not be able to destroy what it reads". Nothing here
    /// mints or touches a `ConsumablePayload`, so a second collect over the same runs returns the
    /// same answer — which is what makes concurrent waits over one run safe.
    #[tokio::test]
    async fn collecting_neither_consumes_nor_mutates_the_payload() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = WaitCompletionStore::default();
        write_public(tmp.path(), "r1", &json!({ "agent": "a" })).await;
        let path = ResultFileName::for_run(&RunId::from_token("r1")).resolve_in(tmp.path());
        let before = tokio::fs::read(&path).await.expect("payload exists");

        let runs = [run_status("r1", None)];
        let first = collect_wait_completions(&runs, &store, tmp.path())
            .await
            .expect("collects");
        let second = collect_wait_completions(&runs, &store, tmp.path())
            .await
            .expect("collects again");

        assert_eq!(first, second);
        assert_eq!(
            tokio::fs::read(&path).await.expect("payload still exists"),
            before
        );
    }

    /// All three rungs missing: a run whose payload has vanished, whose in-process record has
    /// expired and for which no durable replay record exists contributes NOTHING and is not an
    /// error — absence is a normal outcome, and the surviving runs must still be collected around
    /// it (pi `:204`'s `if (replay)` has no `else`).
    #[tokio::test]
    async fn a_vanished_payload_with_no_record_contributes_nothing_and_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = WaitCompletionStore::default();
        write_public(tmp.path(), "present", &json!({ "agent": "a" })).await;

        let collected = collect_wait_completions(
            &[
                run_status("vanished", None),
                run_status("present", None),
                run_status("also-vanished", Some("s1")),
            ],
            &store,
            tmp.path(),
        )
        .await
        .expect("absence is not a failure");

        assert_eq!(collected.len(), 1, "only the run with a payload");
        assert_eq!(collected[0].run_id, "present");
    }

    /// THE acceptance test for SUBA-056, and the one that carries the whole task: a `wait` that
    /// arrives AFTER the completion was delivered and its payload unlinked still resolves.
    ///
    /// The store is deliberately EMPTY at collect time. That is the load-bearing part: with the
    /// in-process record present this passes on WORKFLOW_4 alone and proves nothing, so the record
    /// is written through a store that is then DROPPED — which is exactly the "later process" and
    /// "`DEDUP_TTL` elapsed" conditions, the two the durable tier exists for.
    #[tokio::test]
    async fn a_wait_after_cleanup_resolves_from_the_replay_record() {
        use crate::background::completion_replay::completion_replay_path;

        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("r1");
        let session = crate::identity::SessionId::parse("s1").expect("non-empty");

        // 1. The watcher observes the completion and records it — durably, before the unlink.
        {
            let watcher_store = WaitCompletionStore::default();
            watcher_store
                .record(
                    &run,
                    &json!({ "agent": "coder", "results": [{ "finalOutput": "the answer" }] }),
                    crate::time::now_epoch_millis(),
                    600_000,
                    Some(&crate::background::wait_completions::ReplayPersistence {
                        results_dir: tmp.path(),
                        session_id: &session,
                    }),
                )
                .await;
            assert!(completion_replay_path(tmp.path(), &run).exists());
        }
        // 2. Delete-last: the payload is gone. (It was never promoted here — an absent public path
        //    is precisely what `consume` leaves behind.)
        assert!(
            !ResultFileName::for_run(&run)
                .resolve_in(tmp.path())
                .exists()
        );

        // 3. A LATER process: a brand-new, empty store, which is all a restarted orchestrator has.
        let fresh = WaitCompletionStore::default();
        assert_eq!(
            fresh.get(&run),
            None,
            "nothing in memory — that is the point"
        );

        let collected =
            collect_wait_completions(&[run_status("r1", Some("s1"))], &fresh, tmp.path())
                .await
                .expect("collects");
        assert_eq!(collected.len(), 1, "the durable rung answered");
        assert_eq!(collected[0].run_id, "r1");
        assert_eq!(collected[0].agent.as_deref(), Some("coder"));
        // And it carries the archive path, so a caller can still reach the child's retained text
        // (`wait-subscriptions.ts:255` reads exactly this key).
        assert!(collected[0].archive_path.is_some());

        // An UNATTRIBUTED run reads its own record too: the session filter is OPTIONAL, and
        // `run.session_id` here is `None` (`completion-replay.ts:228`).
        let collected = collect_wait_completions(&[run_status("r1", None)], &fresh, tmp.path())
            .await
            .expect("collects");
        assert_eq!(collected.len(), 1);

        // A run belonging to ANOTHER session does not — the filter is optional, not absent.
        let collected =
            collect_wait_completions(&[run_status("r1", Some("other"))], &fresh, tmp.path())
                .await
                .expect("collects");
        assert!(collected.is_empty(), "a foreign session must not replay it");
    }

    /// The two faults that DO stop a collect, distinguished from absence above. Both surface as
    /// `InvalidData` so the caller can tell a corrupt payload from a missing one — and the caller
    /// (`wait.rs`) turns either into `WaitVerdict::CompletionsFailed` rather than propagating.
    #[tokio::test]
    async fn unparseable_json_and_a_corrupt_inventory_both_fail_the_read() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = WaitCompletionStore::default();

        let path = ResultFileName::for_run(&RunId::from_token("torn")).resolve_in(tmp.path());
        tokio::fs::write(&path, b"{not json".as_slice())
            .await
            .expect("write");
        let error = collect_wait_completions(&[run_status("torn", None)], &store, tmp.path())
            .await
            .expect_err("unparseable JSON is a read failure");
        assert!(
            matches!(&error, SubagentError::Spawn(io) if io.kind() == std::io::ErrorKind::InvalidData),
            "{error:?}"
        );

        // A `workflowChildren` belonging to another run — the projector's one strict seam, which
        // must reach the caller rather than degrade to an absent inventory.
        write_public(
            tmp.path(),
            "misfiled",
            &json!({ "workflowChildren": {
                "version": 1,
                "parentToolCallId": "tool-1",
                "workflowRunId": "someone-else",
                "inventoryComplete": true,
                "workflowState": "completed",
                "children": [],
            } }),
        )
        .await;
        let error = collect_wait_completions(&[run_status("misfiled", None)], &store, tmp.path())
            .await
            .expect_err("a corrupt inventory is a read failure");
        assert!(
            matches!(&error, SubagentError::Spawn(io) if io.kind() == std::io::ErrorKind::InvalidData),
            "{error:?}"
        );
    }
}
