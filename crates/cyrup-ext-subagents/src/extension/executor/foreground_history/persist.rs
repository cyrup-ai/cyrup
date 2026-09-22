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

/// The `&self`-free persist entry point — pi `persistForegroundRunHistory` (`:136-147`), and the
/// ONE implementation of the merge/bound/write rule in this crate.
///
/// Merge-by-`run_id` is the contract: a run this process remembers REPLACES the same id on disk;
/// an id only the file knows (written by a sibling process sharing this results dir) survives
/// untouched. [`is_persistable`] is applied to every remembered run FIRST, so the never-persist
/// rule for a `"detached"` child holds on this path exactly as it does on the `&self` one — there
/// is no second writer that could forget it.
///
/// # Why it takes the map and not the executor
///
/// `foreground.rs`'s detached continuation (`spawn_detached_foreground_continuation`) owns the
/// drive future of a child that outlived its tool call, and it deliberately captures the ONE
/// `Arc` field the reconcile writes rather than the executor: `run_foreground_impl` takes `&self`,
/// [`SubagentExecutor`] carries no self-`Arc` slot, and a handle to the executor would let a
/// detached child pin a shutdown alive. That task can therefore reconcile the settled child into
/// the in-memory map but had nothing to call to get it onto DISK, so a run that settled after its
/// session's last ordinary foreground settle was not restorable across a restart. This is what it
/// calls.
///
/// [`SubagentExecutor::persist_foreground_run_history`] delegates here, so the two entry points
/// cannot drift in what they merge, what they bound, or what they refuse to write.
pub(crate) fn persist_foreground_run_history_from(
    foreground_runs: &std::sync::Mutex<HashMap<RunId, ForegroundHistoryRun>>,
    results_dir: &Path,
    limit: usize,
) {
    let mut merged: HashMap<RunId, ForegroundHistoryRun> = read_index(results_dir)
        .into_iter()
        .map(|run| (run.run_id.clone(), run))
        .collect();
    for run in super::record::foreground_runs_snapshot_of(foreground_runs) {
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

/// [`SubagentExecutor::persist_foreground_run_history_for`]'s body without the executor: resolve
/// `results_dir` from the roots the caller already holds, then persist at the standard bound.
///
/// The resolution rule lives HERE, once, for the same reason the merge rule does — a redirected
/// root must be honoured identically on both paths (the `background/run_history.rs:56-63` lesson).
/// A caller that has no `Roots` in hand has no business guessing one.
pub(crate) fn persist_foreground_run_history_in(
    foreground_runs: &std::sync::Mutex<HashMap<RunId, ForegroundHistoryRun>>,
    roots: &crate::paths::Roots,
    cwd: &Path,
) {
    persist_foreground_run_history_from(
        foreground_runs,
        &default_results_dir_in(roots, cwd),
        super::record::MAX_REMEMBERED_FOREGROUND_RUNS,
    );
}

impl SubagentExecutor {
    /// The results-dir-resolving wrapper `foreground.rs`'s settle path calls (WORKFLOW_7 §3.2):
    /// resolves `results_dir` exactly as [`Self::resume_tracking`] does (`status.rs:27-29`), so a
    /// redirected root is honoured here too (the `background/run_history.rs:56-63` lesson).
    pub(crate) async fn persist_foreground_run_history_for(&self, cwd: &Path) {
        let roots = self.config_snapshot().await.roots;
        persist_foreground_run_history_in(&self.foreground_runs, &roots, cwd);
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
    use crate::extension::executor::foreground_history::record::foreground_runs_snapshot_of;
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

        persist_foreground_run_history_from(&executor.foreground_runs, dir.path(), 50);

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
            foreground_runs_snapshot_of(&executor.foreground_runs)[0].children[0].status,
            "detached",
            "remembered in memory with its real status"
        );

        persist_foreground_run_history_from(&executor.foreground_runs, dir.path(), 50);
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
            foreground_runs_snapshot_of(&executor.foreground_runs)[0].children[0]
                .final_output
                .is_some()
        );

        persist_foreground_run_history_from(&executor.foreground_runs, dir.path(), 50);
        let persisted = read_index(dir.path());
        assert_eq!(
            persisted[0].children[0].final_output, None,
            "an output path exists, so the persisted record must carry no inline final_output"
        );
    }

    /// R-VLS11b-02 — the `&self`-free entry point and the `&self` one are ONE implementation:
    /// over the same map and the same results dir they must produce the SAME BYTES, down to the
    /// merge with what was already on disk and the `sort_and_bound` order.
    ///
    /// The map is seeded directly rather than through `remember_foreground_run` so each run has a
    /// distinct `updated_at` — `sort_and_bound` orders by it, and two runs remembered in the same
    /// millisecond would make the comparison depend on `HashMap` iteration order rather than on
    /// the rule under test.
    ///
    /// MUTATION: let the free entry point skip `is_persistable` and the detached run below reaches
    /// disk on one path and not the other, so the two files differ; re-implement the merge there
    /// and the pre-existing foreign run is dropped from one of them.
    #[tokio::test]
    async fn the_free_entry_point_writes_the_same_bytes_the_executor_path_does() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        // Two sandboxed roots over ONE project cwd, so each path resolves its own file through the
        // same `default_results_dir_in` arithmetic and the two can be compared.
        let self_home = tempfile::tempdir().expect("tempdir");
        let map_home = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        let self_roots = crate::paths::Roots::sandboxed(self_home.path());
        let map_roots = crate::paths::Roots::sandboxed(map_home.path());
        let via_self = default_results_dir_in(&self_roots, project.path());
        let via_map = default_results_dir_in(&map_roots, project.path());
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = self_roots.clone();
        }

        // A run only the FILE knows, seeded identically into both dirs, so the merge leg is
        // exercised on both paths.
        let foreign = run(
            RunId::from_token("foreign-run"),
            "session-b",
            50,
            "completed",
        );
        for dir in [via_self.as_path(), via_map.as_path()] {
            crate::background::atomic::write_private_atomic_json_blocking(
                &history_path(dir),
                &ForegroundHistoryIndex {
                    version: HistoryVersion,
                    runs: vec![foreign.clone()],
                },
            )
            .expect("seed an existing history file");
        }

        {
            let mut map = executor
                .foreground_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (token, updated_at, status) in [
                ("run-old", 100_i64, "completed"),
                ("run-new", 300, "failed"),
                ("run-mid", 200, "stopped"),
                // Never persistable on EITHER path — the rule under test.
                ("run-detached", 400, "detached"),
            ] {
                let id = RunId::from_token(token);
                map.insert(id.clone(), run(id, "session-a", updated_at, status));
            }
        }

        // The `&self` path (what every ordinary foreground settle calls) …
        executor
            .persist_foreground_run_history_for(project.path())
            .await;
        // … and the `&self`-free one the detached continuation calls.
        persist_foreground_run_history_in(&executor.foreground_runs, &map_roots, project.path());

        let from_self = std::fs::read(history_path(&via_self)).expect("the &self path wrote");
        let from_map = std::fs::read(history_path(&via_map)).expect("the free path wrote");
        assert_eq!(
            from_map, from_self,
            "the two entry points must be one implementation, byte for byte"
        );

        let persisted = read_index(&via_map);
        assert_eq!(
            persisted
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-new", "run-mid", "run-old", "foreign-run"],
            "newest first by updated_at, with the detached run refused and the foreign run merged"
        );
    }

    /// §0.6/§2.1 on the `&self`-free path: a `detached` run reaches the in-memory map (the
    /// detached continuation's whole job) and NEVER reaches disk — the never-persist rule is not
    /// something a second writer may forget.
    ///
    /// MUTATION: drop the `is_persistable` guard from `persist_foreground_run_history_from` and a
    /// detached run is written.
    #[test]
    fn the_free_entry_point_never_writes_a_detached_run() {
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

        persist_foreground_run_history_from(&executor.foreground_runs, dir.path(), 50);
        assert!(
            read_index(dir.path()).is_empty(),
            "a detached run must never reach disk, on either entry point"
        );

        // …and once the continuation has RECONCILED it to a settled status, the same call does
        // persist it — which is the whole point of giving that task an entry point.
        {
            let mut map = executor
                .foreground_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = map.get_mut(&run_id)
                && let Some(child) = entry.children.first_mut()
            {
                child.status = "completed".to_string();
            }
        }
        persist_foreground_run_history_from(&executor.foreground_runs, dir.path(), 50);
        let persisted = read_index(dir.path());
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].run_id, run_id);
        assert_eq!(persisted[0].children[0].status, "completed");
    }

    /// `persist_foreground_run_history_in` resolves the results dir from the ROOTS it is handed —
    /// the same rule `persist_foreground_run_history_for` applies through the executor's config —
    /// so a redirected root is honoured on the `&self`-free path too.
    ///
    /// MUTATION: resolve against the process environment instead of the supplied roots and the
    /// file lands outside the redirected root, where the restore pass will never look for it.
    #[tokio::test]
    async fn the_cwd_entry_point_resolves_the_results_dir_from_the_supplied_roots() {
        let home = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        let roots = crate::paths::Roots::sandboxed(home.path());

        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = roots.clone();
        }
        executor.remember_foreground_run(
            &RunId::new(),
            RunMode::Single,
            project.path(),
            &[&test_single_result("scout", 0)],
        );

        persist_foreground_run_history_in(&executor.foreground_runs, &roots, project.path());

        let expected = default_results_dir_in(&roots, project.path());
        assert_eq!(read_index(&expected).len(), 1, "wrote under {expected:?}");
        // And the `&self` wrapper, which resolves the same roots out of the config, agrees.
        let bytes = std::fs::read(history_path(&expected)).expect("read");
        executor
            .persist_foreground_run_history_for(project.path())
            .await;
        assert_eq!(
            std::fs::read(history_path(&expected)).expect("read"),
            bytes,
            "both wrappers resolve to the same file and write the same bytes"
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
        persist_foreground_run_history_from(&executor.foreground_runs, dir.path(), 50);

        let persisted = read_index(dir.path());
        assert_eq!(persisted.len(), 2, "the foreign run must survive the merge");
        assert!(persisted.iter().any(|r| r.run_id == foreign.run_id));
        assert!(persisted.iter().any(|r| r.run_id == mine));
    }
}
