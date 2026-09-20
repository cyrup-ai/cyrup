//! Reading and validating a manifest — pi `parallel-handoff.ts:70-190` @v0.68.0.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::error::{HandoffError, ManifestDefect};
use super::model::{Child, LaneBinding, LaneId, Manifest};

/// pi `readParallelHandoffManifest` (`:122-126`): absent is `Ok(None)`, present-but-invalid is an
/// error.
///
/// # Errors
///
/// [`HandoffError::Io`] on an unreadable file, [`HandoffError::Json`] on malformed bytes, or
/// [`HandoffError::InvalidManifest`] when a structural rule fails.
pub async fn read_manifest(manifest_path: &Path) -> Result<Option<Manifest>, HandoffError> {
    let bytes = match tokio::fs::read(manifest_path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(HandoffError::Io(err)),
    };
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(HandoffError::Json)?;
    validate_manifest_identity(&manifest, manifest_path)?;
    Ok(Some(manifest))
}

/// pi `validateManifestIdentity` (`:70-120`) — the checks the TYPES cannot make, because they are
/// cross-record: uniqueness of an index across the whole manifest, and every child/binding having
/// a matching cleanup task.
///
/// The per-field checks upstream performs here (`version === 1`, `runId` non-empty, `index` a
/// non-negative safe integer, `lane` shape, unknown-field rejection) are all discharged by
/// [`Manifest`]'s own deserialization, which is why they do not appear below.
///
/// # Errors
///
/// [`HandoffError::InvalidManifest`] naming the exact defect.
pub fn validate_manifest_identity(
    manifest: &Manifest,
    manifest_path: &Path,
) -> Result<(), HandoffError> {
    let defect = |detail: ManifestDefect| HandoffError::InvalidManifest {
        path: manifest_path.to_path_buf(),
        detail,
    };

    let mut workflow_keys: HashSet<&str> = HashSet::new();
    let mut child_run_ids: HashSet<&str> = HashSet::new();
    let mut child_indexes: HashSet<u32> = HashSet::new();

    for (group_index, group) in manifest.groups.iter().enumerate() {
        let cleanup_indexes: HashSet<u32> =
            group.cleanup.tasks.iter().map(|task| task.index).collect();

        if let Some(bindings) = group.lane_bindings.as_ref() {
            let mut binding_task_indexes: HashSet<u32> = HashSet::new();
            for (binding_index, binding) in bindings.iter().enumerate() {
                assert_binding_lane_key(binding, group_index, binding_index).map_err(&defect)?;
                if !binding_task_indexes.insert(binding.task_index) {
                    return Err(defect(ManifestDefect::DuplicateBindingTaskIndex {
                        group: group_index,
                        task_index: binding.task_index,
                    }));
                }
                if !cleanup_indexes.contains(&binding.task_index) {
                    return Err(defect(ManifestDefect::BindingWithoutCleanupTask {
                        group: group_index,
                        binding: binding_index,
                        task_index: binding.task_index,
                    }));
                }
            }
        }

        // LANES_2 — the retained naming evidence is bounded by its TYPE on the way in
        // ([`crate::workflows::WorktreeNaming`]'s `Bounded` fields plus `deny_unknown_fields`),
        // but pi's `\n`/`\r`/NUL rule (`lane-metadata.ts:23`, reached through
        // `normalizeWorktreeNaming:83-96`) is a lane-metadata helper rule rather than a bound, so
        // it is applied here, where the containing record is validated. A branch name carrying a
        // newline would otherwise be printed verbatim into the manual `git branch -D <branch>`
        // lines a discard renders (pi `parallel-handoff.ts:730-738`).
        for (task_index, task) in group.cleanup.tasks.iter().enumerate() {
            if let Some(naming) = task.naming.as_ref() {
                naming
                    .validate(&format!(
                        "groups[{group_index}].cleanup.tasks[{task_index}].naming"
                    ))
                    .map_err(|error| {
                        defect(ManifestDefect::InvalidCleanupNaming(
                            error.message().to_string(),
                        ))
                    })?;
            }
        }

        let mut task_indexes: HashSet<u32> = HashSet::new();
        for (child_index, child) in group.children.iter().enumerate() {
            assert_child_lane_key(child, group_index, child_index).map_err(&defect)?;
            if !task_indexes.insert(child.task_index) {
                return Err(defect(ManifestDefect::DuplicateChildTaskIndex {
                    group: group_index,
                    task_index: child.task_index,
                }));
            }
            if !child_indexes.insert(child.index) {
                return Err(defect(ManifestDefect::DuplicateChildIndex(child.index)));
            }
            if !cleanup_indexes.contains(&child.task_index) {
                return Err(defect(ManifestDefect::ChildWithoutCleanupTask {
                    group: group_index,
                    child: child_index,
                    task_index: child.task_index,
                }));
            }
            if let Some(key) = child.workflow_key.as_ref()
                && !workflow_keys.insert(key.as_str())
            {
                return Err(defect(ManifestDefect::DuplicateWorkflowKey(
                    key.as_str().to_string(),
                )));
            }
            if let Some(run_id) = child.run_id.as_ref()
                && !child_run_ids.insert(run_id.as_str())
            {
                return Err(defect(ManifestDefect::DuplicateChildRunId(
                    run_id.as_str().to_string(),
                )));
            }
        }
    }
    Ok(())
}

/// pi `assertWorkflowLaneKey` (`lane-metadata.ts:71-74`) reached from `:106`.
fn assert_child_lane_key(
    child: &Child,
    group_index: usize,
    child_index: usize,
) -> Result<(), ManifestDefect> {
    lane_key_agrees(
        child.lane.as_ref().map(|lane| lane.key.as_str()),
        child
            .workflow_key
            .as_ref()
            .map(crate::workflows::WorkflowKey::as_str),
        format!("groups[{group_index}].children[{child_index}].lane"),
    )
}

/// pi `assertWorkflowLaneKey` reached from `:60` (via `normalizeLaneBinding`).
fn assert_binding_lane_key(
    binding: &LaneBinding,
    group_index: usize,
    binding_index: usize,
) -> Result<(), ManifestDefect> {
    lane_key_agrees(
        binding.lane.as_ref().map(|lane| lane.key.as_str()),
        binding
            .workflow_key
            .as_ref()
            .map(crate::workflows::WorkflowKey::as_str),
        format!("groups[{group_index}].laneBindings[{binding_index}].lane"),
    )
}

fn lane_key_agrees(
    lane_key: Option<&str>,
    workflow_key: Option<&str>,
    label: String,
) -> Result<(), ManifestDefect> {
    let (Some(lane_key), Some(workflow_key)) = (lane_key, workflow_key) else {
        return Ok(());
    };
    if lane_key == workflow_key {
        return Ok(());
    }
    Err(ManifestDefect::LaneKeyMismatch {
        label,
        lane_key: lane_key.to_string(),
        workflow_key: workflow_key.to_string(),
    })
}

// pi's `resolveParallelHandoffChild` (`parallel-handoff.ts:128-160` @v0.68.0) and its
// `workflowKey`/`childRunId` selector are NOT ported here, and the omission is deliberate rather
// than an oversight: at v0.68.0 it has NO production caller — `git grep resolveParallelHandoffChild
// v0.68.0 -- src` finds only its own definition. (An earlier note here named
// `retained-children.ts:68` as its caller; that line calls `resolveRetainedWorktreeCwd`, which is
// ported directly below.) Landing a selector nothing invokes is the "tested machinery with no
// production caller" failure this programme has shipped five times.
//
// `children.list` IS ported (`background/retained_children.rs`); its cwd rung is
// [`resolve_retained_worktree_cwd`], which therefore has two production callers: the `resume`
// revive (`extension/executor/control.rs`) and the retained-children predicate.

/// The manifest must be the one the caller named — pi's `manifest.runId !== runId` throw
/// (`parallel-handoff.ts:141`).
fn require_run(manifest: &Manifest, run_id: &LaneId) -> Result<(), HandoffError> {
    if manifest.run_id == *run_id {
        return Ok(());
    }
    Err(HandoffError::RunMismatch {
        found: manifest.run_id.as_str().to_string(),
        expected: run_id.as_str().to_string(),
    })
}

/// pi `resolveRetainedWorktreeCwd` (`:162-190`) — where a RESUMED child must run, when its
/// worktree was preserved rather than removed.
///
/// Three checks guard a symlinked worktree redirecting a child's cwd outside the sandbox, and all
/// three are load-bearing (`:172-188`):
/// 1. the repo-relative prefix must be neither absolute nor start with `..`;
/// 2. `cleanup.path` must be a directory and NOT a symlink (an `lstat`, not a `stat`);
/// 3. the realpath'd target must be inside the realpath'd worktree root.
///
/// The containment predicate REUSES [`crate::spawn::worktree`]'s existing
/// `normalize_comparable_cwd`/`lexical_normalize` rather than adding a second path-escape
/// implementation to this crate.
///
/// # Errors
///
/// [`HandoffError::NoCleanupRecord`], [`HandoffError::InvalidManagedCwd`],
/// [`HandoffError::ManagedWorktreeRemoved`] or [`HandoffError::ManagedWorktreeCwdMissing`], plus
/// [`read_manifest`]'s own errors.
pub async fn resolve_retained_worktree_cwd(
    manifest_path: &Path,
    run_id: &LaneId,
    child_index: u32,
) -> Result<Option<PathBuf>, HandoffError> {
    let Some(manifest) = read_manifest(manifest_path).await? else {
        return Ok(None);
    };
    require_run(&manifest, run_id)?;
    let Some((group, child)) = manifest.groups.iter().find_map(|group| {
        group
            .children
            .iter()
            .find(|child| child.index == child_index)
            .map(|child| (group, child))
    }) else {
        return Ok(None);
    };
    let Some(cleanup) = group
        .cleanup
        .tasks
        .iter()
        .find(|task| task.index == child.task_index)
    else {
        return Err(HandoffError::NoCleanupRecord {
            run_id: run_id.as_str().to_string(),
            child_index,
        });
    };

    let repo_root = crate::spawn::worktree::normalize_comparable_cwd(&group.repo_root);
    let shared_cwd = crate::spawn::worktree::normalize_comparable_cwd(&manifest.cwd);
    let Some(relative) = repo_relative(&repo_root, &shared_cwd) else {
        return Err(HandoffError::InvalidManagedCwd(run_id.as_str().to_string()));
    };
    let required_cwd = cleanup.path.join(&relative);
    if cleanup.worktree_removed || cleanup.branch_removed {
        return Err(HandoffError::ManagedWorktreeRemoved {
            run_id: run_id.as_str().to_string(),
            cwd: required_cwd,
        });
    }

    let missing = || HandoffError::ManagedWorktreeCwdMissing {
        run_id: run_id.as_str().to_string(),
        cwd: required_cwd.clone(),
    };
    // pi `lstatSync(cleanup.path)`: a SYMLINKED worktree root is refused outright, which is why
    // this is `symlink_metadata` and not `metadata`.
    let root_meta = tokio::fs::symlink_metadata(&cleanup.path)
        .await
        .map_err(|_| missing())?;
    if !root_meta.is_dir() {
        return Err(missing());
    }
    let worktree_root = tokio::fs::canonicalize(&cleanup.path)
        .await
        .map_err(|_| missing())?;
    let target_meta = tokio::fs::metadata(&required_cwd)
        .await
        .map_err(|_| missing())?;
    if !target_meta.is_dir() {
        return Err(missing());
    }
    let resolved = tokio::fs::canonicalize(&required_cwd)
        .await
        .map_err(|_| missing())?;
    if repo_relative(&worktree_root, &resolved).is_none() {
        return Err(HandoffError::InvalidManagedCwd(run_id.as_str().to_string()));
    }
    Ok(Some(required_cwd))
}

/// pi `path.relative(root, candidate)` + the `pathInside` predicate (`:157-160`, `:173`), folded
/// into one: `Some(relative)` iff `candidate` is `root` or below it, `None` for an escape.
///
/// Built on [`crate::spawn::worktree::lexical_normalize`] so the `..`-collapsing rule is this
/// crate's ONE implementation.
fn repo_relative(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let root = crate::spawn::worktree::lexical_normalize(Path::new(""), root);
    let candidate = crate::spawn::worktree::lexical_normalize(Path::new(""), candidate);
    candidate
        .strip_prefix(&root)
        .ok()
        .map(std::path::Path::to_path_buf)
}

/// pi `handoffRecordsPatch` (`runs/shared/worktree.ts:1131-1149` @v0.68.0) — does the manifest at
/// `manifest_path` actually RECORD `patch_path` as a successfully captured patch?
///
/// This is the durability half of the preserve gate. An in-memory capture row says a `.patch` was
/// written; only the manifest says a future reader can find it. A worktree whose work is
/// recoverable solely through this process's heap is not recoverable, so a removal that relies on
/// the patch must first see the patch named on disk.
///
/// Every failure answers `false`, which is the refusing answer: no manifest path, no file there, a
/// manifest this crate's types reject, or no child row naming this patch without an `error`.
/// [`Manifest`]'s own `deny_unknown_fields` deserialization makes it stricter than upstream's
/// hand-rolled probe — a manifest cyrup cannot fully understand is one it will not delete work on
/// the strength of.
pub(crate) async fn handoff_records_patch(manifest_path: Option<&Path>, patch_path: &Path) -> bool {
    let Some(manifest_path) = manifest_path else {
        return false;
    };
    let Ok(Some(manifest)) = read_manifest(manifest_path).await else {
        return false;
    };
    let Ok(resolved) = std::path::absolute(patch_path) else {
        return false;
    };
    manifest.groups.iter().any(|group| {
        group.children.iter().any(|child| {
            child.patch.error.is_none()
                && std::path::absolute(&child.patch.path).is_ok_and(|path| path == resolved)
        })
    })
}
