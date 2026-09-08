//! RunDir / RunPaths / AsyncRoot / ResultsDir (func-SA §4.5)
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::path::{Path, PathBuf};

use super::RunId;

/// `status.json` — the one file name both [`RunPaths::for_run`] and [`RunDir::status`] must agree
/// on.
const STATUS_FILE_NAME: &str = "status.json";

/// The filesystem directory, keyed by run id, holding one background run's `status.json`,
/// `events.jsonl`, control-inbox files, append-request files, output/log files, and (once
/// terminal) its human-readable run-log — everything **except** the terminal [`ResultFile`](crate::background::ResultFile)
/// itself, which lives in the sibling `ResultsDir` (func-SA §4.5 draws this distinction
/// deliberately: presence-in-`ResultsDir` being the sole "truly done" signal only works if the
/// result file is *not* just another entry inside the run's own, still-being-written directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDir(PathBuf);

impl RunDir {
    /// The directory for `run_id` underneath `async_root`. Pure path arithmetic — does not touch
    /// the filesystem (creating the directory via `mkdir` at spawn time is
    /// `spawn_detached.rs`'s job, per R-SA-072's "no pre-flight uniqueness check", not this
    /// constructor's).
    #[must_use]
    pub fn new(async_root: &Path, run_id: &RunId) -> Self {
        Self(async_root.join(run_id.as_str()))
    }

    /// Borrows the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// A [`RunDir`] over a directory that already exists at a known path.
    ///
    /// [`RunDir::new`] derives the path from `(async_root, run_id)`; this adopts one that was
    /// recorded elsewhere — an index entry's `async_dir`, for instance. Both routes produce the
    /// same value for the same run, and having this one means a caller holding a run directory
    /// never has to re-derive `status.json` by joining a duplicated file-name literal.
    #[must_use]
    pub fn for_existing(run_dir: &Path) -> Self {
        Self(run_dir.to_path_buf())
    }

    /// `<run_dir>/status.json`, without requiring a results dir.
    ///
    /// pi `readStatus(asyncDir)` (`shared/utils.ts:141`) derives the status path from the run
    /// directory ALONE; [`RunPaths::for_run`] additionally needs a results dir it does not use for
    /// this field, which forced index readers to invent one.
    #[must_use]
    pub fn status(&self) -> PathBuf {
        self.0.join(STATUS_FILE_NAME)
    }
}

impl AsRef<Path> for RunDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// All well-known file/subdirectory paths within one run's [`RunDir`], plus the sibling terminal
/// [`ResultFile`](crate::background::ResultFile) path in `ResultsDir` (func-SA §4.5's full layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPaths {
    /// The run's own directory.
    pub run_dir: PathBuf,
    /// `<run_dir>/status.json` — [`RunStatus`](crate::background::RunStatus), atomic writes only.
    pub status: PathBuf,
    /// `<run_dir>/events.jsonl` — append-only, size-capped event log (R-SA-136/146). Any writer
    /// appending to this path MUST go through [`crate::jsonl::BoundedJsonlWriter`] (the same
    /// shared primitive [`crate::spawn::SpawnedChild`]'s own per-attempt `.jsonl` tee uses) so the
    /// 50MB-default byte-budget cap is enforced identically here, not re-implemented per writer.
    pub events: PathBuf,
    /// `<run_dir>/control/interrupt.json` — present only while an [`crate::error::SubagentError`]-
    /// free pending interrupt request exists (R-SA-081); a later phase's `InterruptRequest` type
    /// lives in `background/control.rs`, not here.
    pub control_inbox: PathBuf,
    /// `<run_dir>/append-requests/` — directory of pending `ChainAppendRequest` files (R-SA-095).
    pub append_dir: PathBuf,
    /// `<run_dir>/runner.stdout.log` — the detached runner's own raw stdout.
    pub runner_stdout_log: PathBuf,
    /// `<run_dir>/runner.stderr.log` — the detached runner's own raw stderr.
    pub runner_stderr_log: PathBuf,
    /// `<run_dir>/<run_id>.md` — human-readable run-log summary.
    pub run_log_md: PathBuf,
    /// The shared per-cwd results directory this run's terminal result belongs to.
    ///
    /// Carried explicitly because it is NOT derivable from any other field any more. Four callers
    /// used to recover it as `result.parent()`, which was correct only while the payload sat
    /// directly in the results root; now that a promoted payload lives under
    /// `result-owned/<enc(session)>/`, that inference would silently yield a session subdirectory.
    /// Storing the value removes the inference rather than asking every caller to remember why it
    /// no longer holds.
    pub results_dir: PathBuf,
    /// `<ResultsDir>/<run_id>.json` — where terminal results were published by builds predating
    /// the owned partition.
    ///
    /// **Not** where this build writes or, on its own, where a result is found: resolution goes
    /// through [`crate::background::result_index::resolve_payload`], which probes the owned
    /// location first and falls back here. Named `legacy_` so a reader that wants "the result"
    /// cannot get this by accident — the rename is what turned every such reader into a
    /// compile error to be decided one by one.
    pub legacy_result_root: PathBuf,
}

impl RunPaths {
    /// Derives every well-known path for `run_id` from `async_root` (holding the run's own
    /// directory and its contents) and `results_dir` (holding the sibling terminal result file).
    /// Pure path arithmetic only — never touches the filesystem.
    #[must_use]
    pub fn for_run(async_root: &Path, results_dir: &Path, run_id: &RunId) -> Self {
        let run_dir = RunDir::new(async_root, run_id);
        let dir = run_dir.as_path().to_path_buf();
        Self {
            status: dir.join(STATUS_FILE_NAME),
            events: dir.join("events.jsonl"),
            control_inbox: dir.join("control").join("interrupt.json"),
            append_dir: dir.join("append-requests"),
            runner_stdout_log: dir.join("runner.stdout.log"),
            runner_stderr_log: dir.join("runner.stderr.log"),
            run_log_md: dir.join(format!("{}.md", run_id.as_str())),
            results_dir: results_dir.to_path_buf(),
            legacy_result_root: results_dir.join(format!("{}.json", run_id.as_str())),
            run_dir: dir,
        }
    }

    /// Where this run's terminal result payload actually is, if anywhere.
    ///
    /// The one way a holder of [`RunPaths`] should ask the question: it probes the owned location,
    /// the legacy root and the staging area, in that order, and promotes a staged payload on the
    /// way. `session_id` is required because the owned and staged locations are session-partitioned
    /// — a run id alone cannot address them.
    pub async fn resolve_result(
        &self,
        session_id: &crate::identity::SessionId,
        run_id: &RunId,
    ) -> Option<PathBuf> {
        crate::background::result_index::result_payload_path_for_session_run(
            &self.results_dir,
            session_id,
            run_id,
        )
        .await
        .ok()
        .flatten()
    }

    /// Per-step output-log path, `<run_dir>/output-<n>.log` (func-SA §4.5).
    #[must_use]
    pub fn step_output_log(&self, step_index: usize) -> PathBuf {
        self.run_dir.join(format!("output-{step_index}.log"))
    }

    /// The distinct storage subpath for a nested background run keyed under this (root) run's id
    /// (R-SA-104's "SHOULD use a distinct storage subpath keyed under the root run's id"): a
    /// nested run started by a step of this run gets its own `run_dir`/`status`/etc. underneath
    /// `<this run's dir>/nested/<nested_run_id>/` rather than sharing a flat `AsyncRoot` slot
    /// indistinguishable from a top-level run. Still resolved as a plain [`RunPaths`] (nested runs
    /// use the identical on-disk shape as a root run — R-SA-104 is a storage-*location* nesting
    /// rule, not a schema difference) but with `results_dir` also nested underneath the parent,
    /// consistent with keeping a nested run's terminal-result "done" signal scoped under its
    /// parent's own tree rather than mixed into the shared top-level `ResultsDir`.
    #[must_use]
    pub fn nested(&self, nested_run_id: &RunId) -> Self {
        let nested_root = self.run_dir.join("nested");
        let nested_results = nested_root.join("results");
        Self::for_run(&nested_root, &nested_results, nested_run_id)
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

    #[test]
    fn run_dir_joins_async_root_and_run_id() {
        let async_root = PathBuf::from("/var/tmp/cyrup-subagents");
        let run_id = RunId::from_token("abc12345");
        let dir = RunDir::new(&async_root, &run_id);
        assert_eq!(
            dir.as_path(),
            Path::new("/var/tmp/cyrup-subagents/abc12345")
        );
    }

    #[test]
    fn run_paths_for_run_derives_every_well_known_path() {
        let async_root = PathBuf::from("/var/tmp/cyrup-subagents");
        let results_dir = PathBuf::from("/var/tmp/cyrup-subagents-results");
        let run_id = RunId::from_token("abc12345");

        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        assert_eq!(
            paths.run_dir,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345")
        );
        assert_eq!(
            paths.status,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/status.json")
        );
        assert_eq!(
            paths.events,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/events.jsonl")
        );
        assert_eq!(
            paths.control_inbox,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/control/interrupt.json")
        );
        assert_eq!(
            paths.append_dir,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/append-requests")
        );
        assert_eq!(
            paths.runner_stdout_log,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/runner.stdout.log")
        );
        assert_eq!(
            paths.runner_stderr_log,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/runner.stderr.log")
        );
        assert_eq!(
            paths.run_log_md,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/abc12345.md")
        );
        assert_eq!(
            paths.legacy_result_root,
            PathBuf::from("/var/tmp/cyrup-subagents-results/abc12345.json")
        );
    }

    #[test]
    fn run_paths_result_lives_outside_the_run_dir() {
        // func-SA §4.5's deliberate separation: the terminal ResultFile must NOT be a path
        // underneath run_dir, since "presence in ResultsDir" is the authoritative done-signal
        // and must be observable via a directory-watch scoped to ResultsDir alone.
        let async_root = PathBuf::from("/a");
        let results_dir = PathBuf::from("/b");
        let run_id = RunId::from_token("deadbeef");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        assert!(!paths.legacy_result_root.starts_with(&paths.run_dir));
    }

    #[test]
    fn run_paths_step_output_log_is_indexed_within_run_dir() {
        let paths = RunPaths::for_run(
            Path::new("/a"),
            Path::new("/b"),
            &RunId::from_token("cafef00d"),
        );
        assert_eq!(
            paths.step_output_log(3),
            PathBuf::from("/a/cafef00d/output-3.log")
        );
    }

    #[test]
    fn run_paths_nested_is_keyed_under_the_root_run_id() {
        // R-SA-104: nested background runs SHOULD use a distinct storage subpath keyed under the
        // root run's id.
        let root_id = RunId::from_token("root0001");
        let nested_id = RunId::from_token("child001");
        let root_paths = RunPaths::for_run(Path::new("/a"), Path::new("/b"), &root_id);

        let nested_paths = root_paths.nested(&nested_id);

        assert!(
            nested_paths.run_dir.starts_with(&root_paths.run_dir),
            "nested run_dir must live underneath the root run's own directory"
        );
        assert!(
            nested_paths.run_dir.ends_with("nested/child001"),
            "nested run_dir must be keyed by the nested run's own id: {:?}",
            nested_paths.run_dir
        );
        assert!(
            nested_paths.legacy_result_root.starts_with(&root_paths.run_dir),
            "nested results must be scoped under the root run's tree, not the shared top-level \
             ResultsDir"
        );
    }

    #[test]
    fn run_paths_nested_two_children_do_not_collide() {
        let root_id = RunId::from_token("root0002");
        let root_paths = RunPaths::for_run(Path::new("/a"), Path::new("/b"), &root_id);

        let a = root_paths.nested(&RunId::from_token("child00a"));
        let b = root_paths.nested(&RunId::from_token("child00b"));

        assert_ne!(a.run_dir, b.run_dir);
        assert_ne!(a.status, b.status);
        assert_ne!(a.legacy_result_root, b.legacy_result_root);
    }
}
