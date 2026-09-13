//! Persist [`ForegroundHistoryRun`]s to `<results_dir>/foreground-history.json`.
//!
//! Port of [`persistForegroundRunHistory`
//! (`foreground-history.ts:136-147`)](../../../../../../../workspace/pi-subagents/src/runs/foreground/foreground-history.ts),
//! `readIndex` (`:102-112`), `sortAndBound` (`:132-134`) and the persist-eligible half of
//! `compactChild`/`compactRun` (`:27-99`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::default_results_dir_in;

use super::record::{
    ForegroundHistoryChild, ForegroundHistoryRun, HistoryVersion, RESTORABLE, bounded_tail,
};

/// `<results_dir>/foreground-history.json` — pi `historyPath` (`:19-21`).
fn history_path(results_dir: &Path) -> PathBuf {
    results_dir.join("foreground-history.json")
}

/// The on-disk envelope — pi's inline `ForegroundHistoryIndex` interface (`:15-18`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ForegroundHistoryIndex {
    version: HistoryVersion,
    #[serde(default)]
    runs: Vec<ForegroundHistoryRun>,
}

/// pi `readIndex` (`:102-112`): a missing file, a parse failure, OR a version mismatch (folded
/// into the parse failure by [`HistoryVersion`]'s own `Deserialize`) all collapse to an empty
/// list — never a panic, never a partially-trusted record.
pub(crate) fn read_index(results_dir: &Path) -> Vec<ForegroundHistoryRun> {
    match std::fs::read(history_path(results_dir)) {
        Ok(bytes) => serde_json::from_slice::<ForegroundHistoryIndex>(&bytes)
            .map(|index| index.runs)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// pi `sortAndBound` (`:132-134`) — newest first, then truncate. Sort BEFORE bounding, or the
/// cap keeps an arbitrary subset instead of the N most recent.
pub(crate) fn sort_and_bound(
    mut runs: Vec<ForegroundHistoryRun>,
    limit: usize,
) -> Vec<ForegroundHistoryRun> {
    runs.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    runs.truncate(limit);
    runs
}

/// pi `compactChild` (`:27-61`), narrowed to cyrup's own field set: the ONE transform persist
/// applies beyond copying — drop `final_output` when an output path exists, else cap it at 64
/// KiB of bytes (pi's `...(!outputPath && child.finalOutput ? { finalOutput: boundedTail(...) } :
/// {})`, `:28,57`).
fn compact_child(mut child: ForegroundHistoryChild) -> ForegroundHistoryChild {
    let has_output_path = child.artifact_output_path.is_some() || child.saved_output_path.is_some();
    child.final_output = if has_output_path {
        None
    } else {
        child.final_output.map(|text| bounded_tail(&text))
    };
    child
}

/// pi `compactRun` (`:88-99`), the transform half: apply [`compact_child`] to every child.
/// `session_id` needs no unconditional check here — [`ForegroundHistoryRun::session_id`]'s TYPE
/// already requires it.
fn compact_run(run: ForegroundHistoryRun) -> ForegroundHistoryRun {
    ForegroundHistoryRun {
        children: run.children.into_iter().map(compact_child).collect(),
        ..run
    }
}

/// pi `compactRun`'s own eligibility guard (`:89`): a run persists only when it has at least one
/// child and EVERY child's status is one of the four [`RESTORABLE`] strings.
fn is_persistable(run: &ForegroundHistoryRun) -> bool {
    !run.children.is_empty()
        && run
            .children
            .iter()
            .all(|c| RESTORABLE.contains(&c.status.as_str()))
}

impl SubagentExecutor {
    /// pi `persistForegroundRunHistory` (`:136-147`). Merge-by-`run_id` is the contract: a run
    /// this process remembers REPLACES the same id on disk; an id only the file knows (written by
    /// a sibling process sharing this results dir) survives untouched.
    pub(crate) fn persist_foreground_run_history(&self, results_dir: &Path, limit: usize) {
        let mut merged: HashMap<RunId, ForegroundHistoryRun> = read_index(results_dir)
            .into_iter()
            .map(|run| (run.run_id.clone(), run))
            .collect();
        for run in self.foreground_runs_snapshot() {
            if !is_persistable(&run) {
                continue;
            }
            merged.insert(run.run_id.clone(), compact_run(run));
        }
        let runs = sort_and_bound(merged.into_values().collect(), limit);
        // pi `writePrivateAtomicJson` (`:146`) — 0600, creates the parent dir, atomic rename.
        // Best-effort: a write failure here must never surface as a tool/run failure — exactly
        // `record_run_history`'s own best-effort contract.
        let _ = crate::background::atomic::write_private_atomic_json_blocking(
            &history_path(results_dir),
            &ForegroundHistoryIndex {
                version: HistoryVersion,
                runs,
            },
        );
    }

    /// The results-dir-resolving wrapper `foreground.rs`'s settle path calls (WORKFLOW_7 §3.2):
    /// resolves `results_dir` exactly as [`Self::resume_tracking`] does (`status.rs:27-29`), so a
    /// redirected root is honoured here too (the `background/run_history.rs:56-63` lesson).
    pub(crate) async fn persist_foreground_run_history_for(&self, cwd: &Path) {
        let roots = self.config_snapshot().await.roots;
        let results_dir = default_results_dir_in(&roots, cwd);
        self.persist_foreground_run_history(
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
    use crate::background::RunMode;
    use crate::extension::executor::foreground_history::record::test_single_result;
    use crate::extension::testsupport::FixedSessionIdHost;
    use crate::identity::SessionId;
    use std::sync::Arc;

    fn with_session(executor: &SubagentExecutor, session_id: &str) {
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some(session_id.to_string()),
            file: None,
        }));
    }

    fn run(run_id: RunId, session_id: &str, updated_at: i64, status: &str) -> ForegroundHistoryRun {
        ForegroundHistoryRun {
            run_id,
            mode: RunMode::Single,
            cwd: PathBuf::from("/tmp/project"),
            session_id: SessionId::parse(session_id).expect("non-empty"),
            updated_at,
            children: vec![ForegroundHistoryChild {
                agent: "scout".to_string(),
                index: 0,
                status: status.to_string(),
                updated_at: Some(updated_at),
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
                final_output: Some("hello".to_string()),
                tokens: None,
                tool_count: None,
            }],
        }
    }

    /// `read_index` on a directory with no history file at all is an empty list, not an error.
    #[test]
    fn read_index_of_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_index(dir.path()).is_empty());
    }

    /// A file whose `version` is not 1 (or is otherwise malformed) restores as empty — a parse
    /// outcome, never a panic.
    #[test]
    fn read_index_of_a_wrong_version_file_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(history_path(dir.path()), br#"{"version":2,"runs":[]}"#)
            .expect("write malformed history file");
        assert!(read_index(dir.path()).is_empty());
    }

    /// `sort_and_bound`: newest first, sort BEFORE truncate — the cap keeps the N most recent by
    /// `updated_at`, not an arbitrary N.
    #[test]
    fn sort_and_bound_keeps_the_most_recent_by_updated_at() {
        let runs = vec![
            run(RunId::from_token("old"), "s", 100, "completed"),
            run(RunId::from_token("new"), "s", 300, "completed"),
            run(RunId::from_token("mid"), "s", 200, "completed"),
        ];
        let bounded = sort_and_bound(runs, 2);
        assert_eq!(
            bounded
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "mid"]
        );
    }

    /// Only `RESTORABLE` runs (every child one of the four statuses) survive `persist`; a run
    /// with a `detached` child is skipped entirely, and a `final_output` is capped/dropped exactly
    /// as `compact_child` prescribes.
    #[test]
    fn persist_writes_only_restorable_runs_and_compacts_final_output() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");

        let completed_id = RunId::new();
        executor.remember_foreground_run(
            &completed_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&test_single_result("scout", 0)],
        );

        executor.persist_foreground_run_history(dir.path(), 50);

        let persisted = read_index(dir.path());
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].run_id, completed_id);
        assert_eq!(persisted[0].children[0].status, "completed");
        // No artifact/saved-output path was set on this fixture, so `final_output` survives
        // (bounded, not dropped) rather than being elided.
        assert_eq!(
            persisted[0].children[0].final_output.as_deref(),
            Some("done")
        );
    }

    /// §0.6/§2.1: a `detached` run is remembered in memory (visible to the live fleet for the
    /// life of the process) but is NEVER written to disk — `detached` is not one of the four
    /// [`RESTORABLE`] statuses.
    #[test]
    fn persist_never_writes_a_detached_run_though_it_stays_remembered_in_memory() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");

        let mut detached = test_single_result("scout", 0);
        detached.detached = true;
        let run_id = RunId::new();
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&detached],
        );
        assert_eq!(
            executor.foreground_runs_snapshot()[0].children[0].status,
            "detached",
            "remembered in memory with its real status"
        );

        executor.persist_foreground_run_history(dir.path(), 50);
        assert!(
            read_index(dir.path()).is_empty(),
            "a detached run must never reach disk"
        );
    }

    /// pi's `...(!outputPath && child.finalOutput ? {...} : {})` (`:57`): once an output path
    /// exists (an artifact path OR a saved-output path), `final_output` is DROPPED from the
    /// persisted record entirely — never merely capped.
    #[test]
    fn persist_drops_final_output_entirely_once_an_output_path_exists() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");

        let mut result = test_single_result("scout", 0);
        result.saved_output_path = Some("/tmp/project/out.md".to_string());
        let run_id = RunId::new();
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&result],
        );
        // In memory, `final_output` is still carried raw (pi's own remember-time shape).
        assert!(
            executor.foreground_runs_snapshot()[0].children[0]
                .final_output
                .is_some()
        );

        executor.persist_foreground_run_history(dir.path(), 50);
        let persisted = read_index(dir.path());
        assert_eq!(
            persisted[0].children[0].final_output, None,
            "an output path exists, so the persisted record must carry no inline final_output"
        );
    }

    /// Merge-by-id: a run this process persists REPLACES the same id already on disk; an id the
    /// in-memory map does not know about (written by a sibling process) survives untouched.
    #[test]
    fn persist_merges_by_run_id_rather_than_overwriting_the_whole_file() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");

        let foreign = run(
            RunId::from_token("foreign-run"),
            "session-b",
            50,
            "completed",
        );
        crate::background::atomic::write_private_atomic_json_blocking(
            &history_path(dir.path()),
            &ForegroundHistoryIndex {
                version: HistoryVersion,
                runs: vec![foreign.clone()],
            },
        )
        .expect("seed an existing history file");

        let mine = RunId::new();
        executor.remember_foreground_run(
            &mine,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&test_single_result("scout", 0)],
        );
        executor.persist_foreground_run_history(dir.path(), 50);

        let persisted = read_index(dir.path());
        assert_eq!(persisted.len(), 2, "the foreign run must survive the merge");
        assert!(persisted.iter().any(|r| r.run_id == foreign.run_id));
        assert!(persisted.iter().any(|r| r.run_id == mine));
    }
}
