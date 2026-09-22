//! Restore [`super::record::ForegroundHistoryRun`]s from disk into the live in-memory map at
//! session start.
//!
//! Port of [`restoreForegroundRunHistory`
//! (`foreground-history.ts:149-161`)](../../../../../../../workspace/pi-subagents/src/runs/foreground/foreground-history.ts).

use std::path::Path;

use crate::background::delivery::SessionGate;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::default_results_dir_in;
use crate::identity::SessionId;

use super::persist::{read_index, sort_and_bound};

impl SubagentExecutor {
    /// pi `restoreForegroundRunHistory` (`:149-161`). Returns the number of runs newly inserted.
    ///
    /// STRICT: no current session identity restores NOTHING. This is the OPPOSITE arm from
    /// `control_steer`'s async gate (`control.rs:602`) and `resume_tracking`'s
    /// (`status.rs:91`) — both PERMISSIVE. Do not unify them: restoring a foreign session's runs
    /// into this process's fleet on a headless host is precisely the failure this programme
    /// exists to prevent, and "no session identity" is exactly when a shared per-cwd results dir
    /// cannot tell two instances apart.
    pub(crate) fn restore_foreground_run_history(&self, results_dir: &Path, limit: usize) -> usize {
        let current = SessionId::parse_opt(self.current_session_id().as_deref());
        let runs = sort_and_bound(
            read_index(results_dir)
                .into_iter()
                .filter(|run| SessionGate::Strict.admits(current.as_ref(), Some(&run.session_id)))
                .collect(),
            limit,
        );
        let mut map = self
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut restored = 0usize;
        for run in runs {
            // An id already remembered in THIS process's live memory wins; restore never
            // clobbers live state with a stale on-disk copy.
            if let std::collections::hash_map::Entry::Vacant(slot) = map.entry(run.run_id.clone()) {
                slot.insert(run);
                restored += 1;
            }
        }
        restored
    }

    /// The results-dir-resolving wrapper `native_impl.rs`'s `SessionStart` handler calls
    /// (WORKFLOW_7 §3.1): resolves `results_dir` exactly as [`Self::resume_tracking`] does
    /// (`status.rs:27-29`).
    pub(crate) async fn restore_foreground_run_history_for(&self, cwd: &Path) {
        let roots = self.config_snapshot().await.roots;
        let results_dir = default_results_dir_in(&roots, cwd);
        self.restore_foreground_run_history(
            &results_dir,
            super::record::MAX_REMEMBERED_FOREGROUND_RUNS,
        );
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
    use crate::background::{RunId, RunMode};
    use crate::extension::executor::foreground_history::persist::persist_foreground_run_history_from;
    use crate::extension::executor::foreground_history::record::ForegroundHistoryChild;
    use crate::extension::executor::foreground_history::record::ForegroundHistoryRun;
    use crate::extension::executor::foreground_history::record::foreground_runs_snapshot_of;
    use crate::extension::testsupport::FixedSessionIdHost;
    use std::sync::Arc;

    fn with_session(executor: &SubagentExecutor, session_id: &str) {
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some(session_id.to_string()),
            file: None,
        }));
    }

    /// Writes a `foreground-history.json` file with the exact envelope shape production code
    /// reads, so a schema drift between this seed and the real reader would show up here too.
    fn seed_history_file(dir: &Path, runs: Vec<ForegroundHistoryRun>) {
        #[derive(serde::Serialize)]
        struct Envelope {
            version: u32,
            runs: Vec<ForegroundHistoryRun>,
        }
        let path = dir.join("foreground-history.json");
        std::fs::write(
            path,
            serde_json::to_vec(&Envelope { version: 1, runs }).expect("serialize seed history"),
        )
        .expect("write seed history file");
    }

    fn run(run_id: RunId, session_id: &str) -> ForegroundHistoryRun {
        ForegroundHistoryRun {
            run_id,
            mode: RunMode::Single,
            cwd: std::path::PathBuf::from("/tmp/project"),
            session_id: SessionId::parse(session_id).expect("non-empty"),
            updated_at: 123,
            children: vec![ForegroundHistoryChild {
                agent: "scout".to_string(),
                index: 0,
                status: "completed".to_string(),
                updated_at: Some(123),
                context: None,
                model: None,
                thinking: None,
                session_file: None,
                transcript_path: None,
                saved_output_path: None,
                artifact_output_path: None,
                error: None,
                output_save_error: None,
                transcript_error: None,
                final_output: None,
                tokens: None,
                tool_count: None,
            }],
        }
    }

    /// STRICT: with no current session identity, restore inserts NOTHING at all — even though
    /// the on-disk file has entries.
    #[test]
    fn restore_with_no_session_identity_restores_zero_runs() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        seed_history_file(dir.path(), vec![run(RunId::from_token("r1"), "session-a")]);

        let restored = executor.restore_foreground_run_history(dir.path(), 50);
        assert_eq!(restored, 0);
        assert!(foreground_runs_snapshot_of(&executor.foreground_runs).is_empty());
    }

    /// Only runs whose recorded `session_id` equals the live one are restored; a foreign-session
    /// run on disk is silently skipped.
    #[test]
    fn restore_admits_only_runs_owned_by_the_current_session() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        seed_history_file(
            dir.path(),
            vec![
                run(RunId::from_token("mine"), "session-a"),
                run(RunId::from_token("theirs"), "session-b"),
            ],
        );

        let restored = executor.restore_foreground_run_history(dir.path(), 50);
        assert_eq!(restored, 1);
        let runs = foreground_runs_snapshot_of(&executor.foreground_runs);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id.as_str(), "mine");
    }

    /// A run id already remembered in LIVE memory is never clobbered by a stale on-disk copy.
    #[test]
    fn restore_never_overwrites_an_already_remembered_run() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = RunId::from_token("r1");
        seed_history_file(dir.path(), vec![run(run_id.clone(), "session-a")]);

        // Seed live memory FIRST, with a run id that also exists on disk but a different
        // `updated_at`, so a clobber would be observable.
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[
                &crate::extension::executor::foreground_history::record::test_single_result(
                    "scout", 0,
                ),
            ],
        );
        let live_updated_at = foreground_runs_snapshot_of(&executor.foreground_runs)[0].updated_at;

        let restored = executor.restore_foreground_run_history(dir.path(), 50);
        assert_eq!(
            restored, 0,
            "the id was already live — nothing NEW was inserted"
        );
        let runs = foreground_runs_snapshot_of(&executor.foreground_runs);
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].updated_at, live_updated_at,
            "the live entry must survive untouched, not be overwritten by the stale file"
        );
    }

    /// A full round trip: persist, then a FRESH executor (a new process, or this same one after
    /// `SessionStart`) restores it back.
    #[tokio::test]
    async fn persist_then_restore_round_trips_a_settled_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let writer = SubagentExecutor::new();
        with_session(&writer, "session-a");
        let run_id = RunId::new();
        writer.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[
                &crate::extension::executor::foreground_history::record::test_single_result(
                    "scout", 0,
                ),
            ],
        );
        persist_foreground_run_history_from(&writer.foreground_runs, dir.path(), 50);

        let reader = SubagentExecutor::new();
        with_session(&reader, "session-a");
        let restored = reader.restore_foreground_run_history(dir.path(), 50);
        assert_eq!(restored, 1);
        let runs = foreground_runs_snapshot_of(&reader.foreground_runs);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, run_id);
        assert_eq!(runs[0].children[0].status, "completed");
    }
}
