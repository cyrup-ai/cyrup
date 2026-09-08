//! pi `snapshotTrackedMutations` (`:67-76`) / `collectTrackedMutationEvidence` (`:78-105`).
//!
//! Evidence collection can never fail a run: both entry points return records, never `Result`s.
//! Upstream wraps both bodies in `try/catch` and degrades to `unavailable` (`:73-75`, `:102-104`);
//! here the fallible body lives in a private `try_*` sibling and the public function folds its
//! error into the record — no `?` escapes either public function.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::repo;
use super::types::{TrackedMutationEvidence, TrackedMutationSnapshot, TrackedMutationSource};

/// pi `snapshotTrackedMutations` (`:67-76`): the tracked-file baseline, taken BEFORE the first
/// child spawns — which tracked files are already dirty, and a per-file fingerprint of how.
///
/// **Cannot fail.** Every repository error becomes `unavailable: Some(message)` with empty
/// `dirty_files` (`:73-75`), and [`collect_tracked_mutation_evidence`] short-circuits on that
/// (`:79-81`). The run this describes has already ended by the time the evidence is read; a
/// failure to characterise it must never become a second failure. Returns a
/// [`TrackedMutationSnapshot`], never a `Result`.
///
/// A non-repository cwd is the ordinary case, not an error: `gix::discover` fails, and the
/// snapshot records why.
#[must_use]
pub fn snapshot_tracked_mutations(cwd: &Path) -> TrackedMutationSnapshot {
    match try_snapshot(cwd) {
        Ok(snapshot) => snapshot,
        Err(message) => TrackedMutationSnapshot {
            source: TrackedMutationSource::TrackedFiles,
            tracked_only: true,
            cwd: cwd.to_path_buf(),
            git_root: None,
            dirty_files: Vec::new(),
            fingerprints: BTreeMap::new(),
            truncated: false,
            unavailable: Some(message),
        },
    }
}

fn try_snapshot(cwd: &Path) -> Result<TrackedMutationSnapshot, String> {
    let repository = repo::open_repository(cwd)?;
    let workdir = repo::required_workdir(&repository)?.to_path_buf();
    let head_tree = repo::head_tree(&repository)?;
    let changed = repo::list_changed_tracked_files(&repository)?;
    let mut fingerprints = BTreeMap::new();
    for file in &changed.paths {
        fingerprints.insert(
            file.clone(),
            repo::fingerprint_path(&workdir, &head_tree, file)?,
        );
    }
    Ok(TrackedMutationSnapshot {
        source: TrackedMutationSource::TrackedFiles,
        tracked_only: true,
        cwd: cwd.to_path_buf(),
        // [CYRUP-DELTA, fills an upstream hole] `shared/types.ts:588` declares `gitRoot` and
        // `mutation-evidence.ts` never writes it — `gix::discover` already resolved the work-dir,
        // so recording it costs nothing (upstream would have needed a second `git` invocation).
        git_root: Some(workdir),
        dirty_files: changed.paths,
        fingerprints,
        truncated: changed.truncated,
        unavailable: None,
    })
}

/// pi `collectTrackedMutationEvidence` (`:78-105`): the diff of two snapshots — which tracked
/// files changed between `snapshot` and now.
///
/// Same failure policy as [`snapshot_tracked_mutations`]: an unavailable snapshot short-circuits
/// to empty evidence carrying the same message (`:79-81`), and any collect-time repository error
/// degrades to `unavailable` (`:102-104`) rather than failing the run being described.
#[must_use]
pub fn collect_tracked_mutation_evidence(
    snapshot: &TrackedMutationSnapshot,
    cwd: &Path,
) -> TrackedMutationEvidence {
    if let Some(unavailable) = &snapshot.unavailable {
        return unavailable_evidence(unavailable.clone());
    }
    match try_collect(snapshot, cwd) {
        Ok(evidence) => evidence,
        Err(message) => unavailable_evidence(message),
    }
}

fn try_collect(
    snapshot: &TrackedMutationSnapshot,
    cwd: &Path,
) -> Result<TrackedMutationEvidence, String> {
    let repository = repo::open_repository(cwd)?;
    let workdir = repo::required_workdir(&repository)?.to_path_buf();
    let head_tree = repo::head_tree(&repository)?;
    let current = repo::list_changed_tracked_files(&repository)?;

    let start_dirty: BTreeSet<&str> = snapshot.dirty_files.iter().map(String::as_str).collect();
    // The union of both endpoints' dirty lists (pi `:86` — both halves can name the same path).
    let candidates: BTreeSet<&str> = snapshot
        .dirty_files
        .iter()
        .chain(current.paths.iter())
        .map(String::as_str)
        .collect();

    let mut changed_files: Vec<String> = Vec::new();
    for file in candidates {
        if !start_dirty.contains(file) {
            // pi `:89` — a file that was CLEAN at snapshot time counts as changed on appearance
            // alone, UNLESS the snapshot was truncated. Past the 500-path cut, appearance proves
            // nothing: the file may have been dirty all along and simply not recorded. Dropping
            // this guard turns a large dirty repository into a false accusation against the child.
            if !snapshot.truncated {
                changed_files.push(file.to_string());
            }
            continue;
        }
        // Dirty at snapshot time: only a MOVED fingerprint counts — and a dirty file with no
        // recorded fingerprint counts as moved (`same_fingerprint`'s deliberate `None ⇒ false`).
        let now = repo::fingerprint_path(&workdir, &head_tree, file)?;
        if !repo::same_fingerprint(snapshot.fingerprints.get(file), &now) {
            changed_files.push(file.to_string());
        }
    }
    changed_files.sort(); // pi `:94` — rendered and compared; must be stable

    Ok(TrackedMutationEvidence {
        source: TrackedMutationSource::TrackedFiles,
        tracked_only: true,
        attempted_mutation: !changed_files.is_empty(), // pi `:99`
        changed_files,
        truncated: snapshot.truncated || current.truncated, // pi `:100`
        unavailable: None,
    })
}

fn unavailable_evidence(message: String) -> TrackedMutationEvidence {
    TrackedMutationEvidence {
        source: TrackedMutationSource::TrackedFiles,
        tracked_only: true,
        changed_files: Vec::new(),
        attempted_mutation: false,
        truncated: false,
        unavailable: Some(message),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::exec::mutation_evidence::types::TrackedMutationFingerprint;

    /// Initialise a real (non-shell) repository with one committed tracked file, entirely through
    /// `gix` — no `git` binary is invoked anywhere in this module, tests included (the module's
    /// own DoD: it shells out to nothing).
    fn init_repo_with_commit(dir: &Path) -> gix::Repository {
        let repo = gix::init(dir).expect("init repository");
        std::fs::write(dir.join("tracked.txt"), "committed\n").unwrap();

        // Build the tree object directly (`tree-editor` is not a default `gix` feature, and this
        // task deliberately takes default features only): one blob entry at `tracked.txt`.
        let blob = repo.write_blob("committed\n").expect("write blob").detach();
        let tree = repo
            .write_object(&gix::objs::Tree {
                entries: vec![gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: "tracked.txt".into(),
                    oid: blob,
                }],
            })
            .expect("write tree")
            .detach();

        // Commit with an explicit signature: a fresh temp repository has no author/committer
        // configuration, and `Repository::commit` fails without one.
        let signature = gix::actor::SignatureRef {
            name: "mutation-evidence-test".into(),
            email: "test@example.invalid".into(),
            time: "946684800 +0000",
        };
        repo.commit_as(signature, signature, "HEAD", "seed", tree, Vec::<gix::ObjectId>::new())
            .expect("commit");

        // Refresh the on-disk index from HEAD so the status baseline (HEAD-tree vs index vs
        // worktree) sees a clean repository — `commit_as` writes objects and refs, not the index.
        let tree_id = repo.head_tree_id().expect("head tree id").detach();
        let mut index = repo.index_from_tree(&tree_id).expect("index from tree");
        index
            .write(gix::index::write::Options::default())
            .expect("write index");
        gix::open(repo.path()).expect("reopen")
    }

    #[test]
    fn a_non_repository_cwd_yields_unavailable_and_collect_short_circuits() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let snapshot = snapshot_tracked_mutations(dir.path());
        assert!(snapshot.unavailable.is_some(), "not a repository");
        assert!(snapshot.dirty_files.is_empty());
        assert!(snapshot.fingerprints.is_empty());
        assert_eq!(snapshot.git_root, None);

        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert_eq!(evidence.unavailable, snapshot.unavailable);
        assert!(evidence.changed_files.is_empty());
        assert!(!evidence.attempted_mutation);
    }

    #[test]
    fn a_clean_repository_snapshot_is_empty_and_a_child_write_is_reported() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let _repo = init_repo_with_commit(dir.path());

        let snapshot = snapshot_tracked_mutations(dir.path());
        assert_eq!(snapshot.unavailable, None);
        assert!(snapshot.dirty_files.is_empty(), "{:?}", snapshot.dirty_files);
        assert!(!snapshot.truncated);
        assert_eq!(
            snapshot.git_root.as_deref().map(|p| p.canonicalize().unwrap()),
            Some(dir.path().canonicalize().unwrap())
        );

        // No mutation → no evidence.
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert!(evidence.changed_files.is_empty());
        assert!(!evidence.attempted_mutation);

        // The "child" mutates a tracked file → reported, sorted, attempted.
        std::fs::write(dir.path().join("tracked.txt"), "mutated by child\n").unwrap();
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert_eq!(evidence.changed_files, vec!["tracked.txt".to_string()]);
        assert!(evidence.attempted_mutation);
        assert_eq!(evidence.unavailable, None);
    }

    #[test]
    fn untracked_files_never_appear_in_snapshot_or_evidence() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let _repo = init_repo_with_commit(dir.path());
        std::fs::write(dir.path().join("untracked-artifact.log"), "noise\n").unwrap();

        let snapshot = snapshot_tracked_mutations(dir.path());
        assert_eq!(snapshot.unavailable, None);
        assert!(
            snapshot.dirty_files.is_empty(),
            "untracked files must not appear: {:?}",
            snapshot.dirty_files
        );

        std::fs::write(dir.path().join("another-untracked.log"), "more noise\n").unwrap();
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert!(
            evidence.changed_files.is_empty(),
            "untracked files must not appear: {:?}",
            evidence.changed_files
        );
    }

    #[test]
    fn a_file_dirty_at_snapshot_time_is_reported_only_when_its_fingerprint_moved() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let _repo = init_repo_with_commit(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "dirty before child\n").unwrap();

        let snapshot = snapshot_tracked_mutations(dir.path());
        assert_eq!(snapshot.dirty_files, vec!["tracked.txt".to_string()]);
        assert!(snapshot.fingerprints.contains_key("tracked.txt"));

        // Untouched by the child → fingerprint identical → NOT reported.
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert!(evidence.changed_files.is_empty(), "{:?}", evidence.changed_files);
        assert!(!evidence.attempted_mutation);

        // Child touches it further → fingerprint moved → reported.
        std::fs::write(dir.path().join("tracked.txt"), "child changed it again\n").unwrap();
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert_eq!(evidence.changed_files, vec!["tracked.txt".to_string()]);
    }

    #[test]
    fn a_dirty_file_with_no_recorded_fingerprint_is_reported() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let _repo = init_repo_with_commit(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "dirty before child\n").unwrap();

        let mut snapshot = snapshot_tracked_mutations(dir.path());
        // Simulate an older/foreign snapshot that recorded the dirty path without a fingerprint.
        let removed: Option<TrackedMutationFingerprint> =
            snapshot.fingerprints.remove("tracked.txt");
        assert!(removed.is_some());

        // pi `:62-65`'s `None ⇒ false`: no recorded fingerprint counts as changed.
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        assert_eq!(evidence.changed_files, vec!["tracked.txt".to_string()]);
    }

    #[test]
    fn a_truncated_snapshot_does_not_report_a_newly_appearing_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let _repo = init_repo_with_commit(dir.path());

        let mut snapshot = snapshot_tracked_mutations(dir.path());
        assert!(snapshot.dirty_files.is_empty());
        // Simulate the 500-path cut: the snapshot says it is truncated.
        snapshot.truncated = true;

        // A file that was clean at snapshot time appears dirty now…
        std::fs::write(dir.path().join("tracked.txt"), "appeared after snapshot\n").unwrap();
        let evidence = collect_tracked_mutation_evidence(&snapshot, dir.path());
        // …but past the cut, appearance proves nothing (pi `:89`).
        assert!(
            evidence.changed_files.is_empty(),
            "a truncated snapshot must not accuse on appearance alone: {:?}",
            evidence.changed_files
        );
        // The truncation itself is carried forward (pi `:100`).
        assert!(evidence.truncated);
    }
}
