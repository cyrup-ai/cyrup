//! The forty-one gates — pi `buildManagedEntry` (`:596-711` @`v0.68.0`), plus the two entry kinds
//! that never reach them (`buildUnknownGitEntry:713`, `buildMissingMetadataEntry:719`).
//!
//! # The rule this file exists to enforce
//!
//! **Absence of proof is never proof of safety.** Every gate below returns a
//! [`Verdict`] and stops; the only way to reach [`Verdict::Safe`] is to fall off the end of all
//! of them. There is no `Default`, no fallible unwrap that could produce `Safe`, and no early
//! `Ok(Safe)`.
//!
//! # And the one it exists to make impossible
//!
//! Nothing here mutates. The only `git` calls are `rev-parse --verify`, `status --porcelain`,
//! `diff --quiet` and `merge-base --is-ancestor`, plus the patch validator's temporary-index
//! re-capture. `git worktree remove`, `git branch -D` and `git worktree prune` do not appear in
//! this module and `crate::spawn::worktree::cleanup_worktrees` is deliberately not imported.

use std::path::{Path, PathBuf};

use super::git;
use super::gitwt::GitWorktreeRecord;
use super::metadata::{MetadataRecord, RunStateInspection, inspect_run_state};
use super::model::{CleanupPlanEntry, CleanupPreconditions, CleanupSource, CommitId, StatusDigest};
use super::paths::{comparable_path, inspect_path, path_inside, resolve_existing_path, same_path};
use super::verdict::{IneligibleReason, SafeNote, StaleReason, UnknownReason, Verdict};

/// The leaf-name prefix [`crate::spawn::worktree`]'s `build_worktree_path` writes.
///
/// See [`IneligibleReason::NotAManagedWorktreeName`] for why containment needs it.
pub(crate) const MANAGED_WORKTREE_PREFIX: &str = "cyrup-worktree-";

/// Everything the classifier needs that is constant across one plan.
pub(crate) struct ClassifyContext<'a> {
    /// The realpath'd repository toplevel.
    pub(crate) repo_root: &'a Path,
    /// The managed-worktree base directory, resolved by the SAME function the create side calls.
    pub(crate) base_dir: &'a Path,
    /// pi `cleanupContainmentInvalid` (`:229-234`), precomputed.
    pub(crate) containment_invalid: bool,
    /// `git rev-parse HEAD` at the repository root.
    pub(crate) target_head: &'a str,
    /// `comparable_path(repo_root)`.
    pub(crate) root_path: &'a Path,
    /// Every git worktree row, including the repository root's own.
    pub(crate) all_git_records: &'a [GitWorktreeRecord],
    /// Epoch millis.
    pub(crate) now: i64,
    /// The foreground ownership probe. Required, never `Option` — see
    /// [`super::BuildCleanupPlanInput::foreground_ownership`].
    pub(crate) foreground_ownership: super::ForegroundOwnershipProbe<'a>,
}

/// The mutable half of an entry that the gates fill in as they learn things — pi mutates
/// `entry.preconditions` in place at `:658-666`.
struct EntryFacts {
    worktree_head: Option<String>,
    branch_tip: Option<CommitId>,
    base_commit: Option<CommitId>,
    status_digest: Option<StatusDigest>,
    patch_path: Option<PathBuf>,
}

impl EntryFacts {
    fn empty() -> Self {
        Self {
            worktree_head: None,
            branch_tip: None,
            base_commit: None,
            status_digest: None,
            patch_path: None,
        }
    }
}

/// pi `buildEntryBase` (`:521-551`) + `blockedEntry` (`:553-555`), fused: a [`Verdict`] and the
/// facts gathered so far become the serialized entry.
#[allow(clippy::too_many_arguments)]
fn finish(
    path: PathBuf,
    branch: String,
    source: CleanupSource,
    record: Option<&MetadataRecord>,
    target_head: &str,
    facts: &EntryFacts,
    verdict: &Verdict,
) -> CleanupPlanEntry {
    let recorded_base_dir = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    CleanupPlanEntry {
        decision: verdict.decision(),
        state: verdict.state(),
        reasons: verdict.reasons(),
        source,
        will_delete_branch: verdict.delete_branch(),
        run_id: record.map(|record| record.manifest.run_id.as_str().to_string()),
        handoff_path: record.map(|record| record.manifest_path.clone()),
        task_index: record.map(|record| record.task.index),
        base_commit: facts.base_commit.as_ref().map_or_else(
            || {
                record
                    .map(|record| record.group.base_commit.clone())
                    .filter(|commit| !commit.trim().is_empty())
            },
            |commit| Some(commit.as_str().to_string()),
        ),
        patch_path: facts.patch_path.clone(),
        target_ref: Some(target_head.to_string()),
        preconditions: CleanupPreconditions {
            path: path.clone(),
            branch: branch.clone(),
            worktree_head: facts.worktree_head.clone(),
            branch_tip: facts.branch_tip.clone(),
            base_commit: facts.base_commit.clone().or_else(|| {
                record
                    .map(|record| record.group.base_commit.clone())
                    .filter(|commit| !commit.trim().is_empty())
                    .map(|commit| CommitId::resolved(&commit))
            }),
            status_digest: facts.status_digest.clone(),
            recorded_base_dir,
            target_ref: target_head.to_string(),
        },
        path,
        branch,
    }
}

/// pi `buildUnknownGitEntry` (`:713-717`) — a git worktree no handoff metadata claims.
pub(crate) fn unknown_git_entry(
    git_record: &GitWorktreeRecord,
    target_head: &str,
) -> CleanupPlanEntry {
    let mut facts = EntryFacts::empty();
    if !git_record.head.is_empty() {
        facts.worktree_head = Some(git_record.head.clone());
    }
    finish(
        resolve_existing_path(&git_record.path),
        git_record.branch.clone().unwrap_or_default(),
        CleanupSource::Git,
        None,
        target_head,
        &facts,
        &Verdict::Unknown(UnknownReason::NoMatchingMetadata),
    )
}

/// pi `buildMissingMetadataEntry` (`:719-724`) — metadata for a worktree git does not list.
///
/// `None` when the task records BOTH removals: there is nothing left to say about a worktree and
/// branch that are already gone (pi `:720`).
pub(crate) fn missing_metadata_entry(
    record: &MetadataRecord,
    target_head: &str,
) -> Option<CleanupPlanEntry> {
    if record.task.worktree_removed && record.task.branch_removed {
        return None;
    }
    Some(finish(
        record.worktree_path(),
        record.task.branch.clone(),
        CleanupSource::Metadata,
        Some(record),
        target_head,
        &EntryFacts::empty(),
        &Verdict::Stale(StaleReason::NotPresentInGit),
    ))
}

/// pi `buildManagedEntry` (`:596-711`): all forty-one gates, in upstream's evaluation order.
///
/// The ordering is load-bearing and is NOT rearranged for readability. A cheap check moved above
/// an expensive one would change which reason the operator is shown, and several gates read as
/// preconditions of the next (the path must exist before it can be tested for containment; the
/// run must be terminal before its worktree's cleanliness means anything).
pub(crate) async fn managed_entry(
    ctx: &ClassifyContext<'_>,
    git_record: &GitWorktreeRecord,
    record: &MetadataRecord,
) -> CleanupPlanEntry {
    let metadata_worktree_path = record.worktree_path();
    let worktree_path = resolve_existing_path(&metadata_worktree_path);
    let branch = git_record.branch.clone().unwrap_or_default();
    let mut facts = EntryFacts::empty();

    macro_rules! blocked {
        ($verdict:expr) => {
            return finish(
                worktree_path.clone(),
                branch.clone(),
                CleanupSource::Both,
                Some(record),
                ctx.target_head,
                &facts,
                &$verdict,
            )
        };
    }

    // 1-3 — the manifest must identify itself. `runId`/`source`/`repoRoot` are typed, so the two
    // upstream checks that a string is present and non-blank (`:613`, `:614`) are discharged by
    // deserialization; what remains is the blank-after-trim case the types still admit.
    if record.manifest.run_id.as_str().trim().is_empty() {
        blocked!(Verdict::Unknown(UnknownReason::NoOwningRunId));
    }
    if record.group.repo_root.as_os_str().is_empty() {
        blocked!(Verdict::Unknown(UnknownReason::NoRepositoryRoot));
    }

    // 4 — metadata and git must name the SAME directory.
    if !same_path(&metadata_worktree_path, &git_record.path) {
        blocked!(Verdict::Unknown(UnknownReason::MetadataPathMismatch));
    }

    // 5-7 — the path git knows.
    let git_inspection = inspect_path(&git_record.path);
    if let Some(detail) = git_inspection.error {
        blocked!(Verdict::Unknown(UnknownReason::GitPathUninspectable {
            detail
        }));
    }
    if git_inspection.missing {
        blocked!(Verdict::Stale(StaleReason::GitPathMissing));
    }
    if git_inspection.symlink {
        blocked!(Verdict::Unknown(UnknownReason::GitPathIsSymlink));
    }

    // 8-11 — the path metadata records.
    let inspection = inspect_path(&worktree_path);
    if let Some(detail) = inspection.error {
        blocked!(Verdict::Unknown(UnknownReason::WorktreePathUninspectable {
            detail
        }));
    }
    if inspection.missing {
        blocked!(Verdict::Stale(StaleReason::WorktreePathMissing));
    }
    if inspection.symlink {
        blocked!(Verdict::Unknown(UnknownReason::WorktreePathIsSymlink));
    }
    let Some(realpath) = inspection.realpath.clone().filter(|_| inspection.directory) else {
        blocked!(Verdict::Unknown(UnknownReason::WorktreePathNotADirectory));
    };

    // 12 — never anything under the agent's extensions directory.
    let extensions_dir = crate::paths::agent_dir().join("extensions");
    if path_inside(&extensions_dir, &realpath, false) {
        blocked!(Verdict::Ineligible(IneligibleReason::InsideExtensionsDir {
            extensions_dir
        }));
    }

    // 13 — a STRICT child of the managed base directory, and nothing else.
    if ctx.containment_invalid || !path_inside(ctx.base_dir, &realpath, true) {
        blocked!(Verdict::Ineligible(IneligibleReason::OutsideBaseDir {
            base_dir: ctx.base_dir.to_path_buf()
        }));
    }
    // 13b — the cyrup tightening. See `IneligibleReason::NotAManagedWorktreeName`.
    if !realpath
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(MANAGED_WORKTREE_PREFIX))
    {
        blocked!(Verdict::Ineligible(
            IneligibleReason::NotAManagedWorktreeName {
                expected_prefix: MANAGED_WORKTREE_PREFIX
            }
        ));
    }

    // 14 — never the repository root.
    if ctx.root_path == comparable_path(&realpath) {
        blocked!(Verdict::Ineligible(IneligibleReason::IsRepositoryRoot));
    }

    // 15-16 — the branch.
    let Some(git_branch) = git_record.branch.as_deref() else {
        blocked!(Verdict::Unknown(UnknownReason::DetachedWorktree));
    };
    if record.task.branch != git_branch {
        blocked!(Verdict::Unknown(UnknownReason::MetadataBranchMismatch {
            metadata: record.task.branch.clone(),
            git: git_branch.to_string()
        }));
    }

    // 17-19 — the ledger must say "still here, on purpose, pending cleanup".
    if record.task.worktree_removed {
        blocked!(Verdict::Stale(StaleReason::AlreadyRecordedRemoved));
    }
    if record.task.preserved != Some(true) {
        blocked!(Verdict::Unknown(UnknownReason::NotMarkedPreserved));
    }
    if record.group.cleanup.state != crate::handoff::model::CleanupState::Partial {
        blocked!(Verdict::Unknown(UnknownReason::CleanupStateNotPartial));
    }

    // 20-21 — two reasons that mean "something still needs this worktree". Substring matches on
    // lowercased text, exactly as upstream (`:636-638`): the reason is free prose written by the
    // cleanup side, and the sibling module's `CLEANUP_PENDING_REASON` is one of the two literals.
    let cleanup_reason = record
        .task
        .reason
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    if cleanup_reason.contains("retained child resume") {
        blocked!(Verdict::Ineligible(IneligibleReason::RetainedChildResume));
    }
    if cleanup_reason.contains("cleanup pending durable handoff capture") {
        blocked!(Verdict::Ineligible(
            IneligibleReason::DurableCaptureStillPending
        ));
    }

    // 22 — never plan against a different repository.
    if comparable_path(&resolve_existing_path(&record.group.repo_root))
        != comparable_path(ctx.repo_root)
    {
        blocked!(Verdict::Unknown(UnknownReason::RepoRootMismatch));
    }

    // 23 — the same branch checked out in another worktree.
    if ctx.all_git_records.iter().any(|candidate| {
        candidate.branch.as_deref() == Some(git_branch)
            && comparable_path(&candidate.path) != comparable_path(&git_record.path)
    }) {
        blocked!(Verdict::Unknown(UnknownReason::BranchCheckedOutElsewhere));
    }

    // 24 — the branch the repository root itself has checked out.
    let root_branch = ctx
        .all_git_records
        .iter()
        .find(|candidate| comparable_path(&candidate.path) == ctx.root_path)
        .and_then(|candidate| candidate.branch.as_deref());
    if root_branch == Some(git_branch) {
        blocked!(Verdict::Ineligible(
            IneligibleReason::BranchCheckedOutAtRepoRoot
        ));
    }

    // 25-27 — the owning run must be PROVABLY terminal.
    match inspect_run_state(record, ctx.now, ctx.foreground_ownership).await {
        RunStateInspection::Unknown(reason) => blocked!(Verdict::Unknown(reason)),
        RunStateInspection::Active(reason) => blocked!(Verdict::Active(reason)),
        RunStateInspection::Stale(reason) => blocked!(Verdict::Stale(reason)),
        RunStateInspection::Terminal => {}
    }

    // 28-29 — the base commit.
    let base_commit = record.group.base_commit.trim();
    if base_commit.is_empty() {
        blocked!(Verdict::Unknown(UnknownReason::NoBaseCommit));
    }
    let resolved_base = match git::resolve_revision(ctx.repo_root, base_commit).await {
        Ok(value) => CommitId::resolved(&value),
        Err(detail) => {
            blocked!(Verdict::Unknown(UnknownReason::BaseCommitUnresolvable {
                detail
            }));
        }
    };
    facts.base_commit = Some(resolved_base.clone());

    // 30-31 — the branch tip must exist and must be what the worktree has checked out.
    let branch_tip =
        match git::resolve_revision(ctx.repo_root, &format!("refs/heads/{git_branch}")).await {
            Ok(value) => CommitId::resolved(&value),
            Err(detail) => {
                blocked!(Verdict::Unknown(UnknownReason::BranchTipUnresolvable {
                    detail
                }));
            }
        };
    facts.branch_tip = Some(branch_tip.clone());
    facts.worktree_head = Some(git_record.head.clone());
    if branch_tip.as_str() != git_record.head {
        blocked!(Verdict::Unknown(UnknownReason::BranchTipHeadMismatch));
    }

    // 32-33 — THE gate, and the one that must never answer `Remove`.
    //
    // `--untracked-files=all` is not optional, for a reason that was checked rather than assumed:
    // `status.showUntrackedFiles = no` is an ordinary operator setting (commonly carried in
    // `~/.gitconfig` on large repositories), and under it a bare `git status --porcelain=v1`
    // prints NOTHING for a worktree full of unsaved new files. Passing the flag explicitly
    // overrides that config, so this gate cannot be silenced by the user's own git settings. The
    // flag also expands an untracked directory into its files, which matters less but costs
    // nothing.
    let status = git::run(
        &worktree_path,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .await;
    if status.status != Some(0) {
        blocked!(Verdict::Unknown(UnknownReason::StatusCheckFailed {
            detail: git::failure(
                &status,
                &format!("git -C {} status --porcelain=v1", worktree_path.display())
            )
        }));
    }
    facts.status_digest = Some(StatusDigest::of(&status.stdout));
    if !status.stdout.trim().is_empty() {
        blocked!(Verdict::Dirty);
    }

    // 34-35 — the two divergence probes. Both are legitimately 0-or-1, so anything else is a
    // failure of the CHECK and therefore Unknown, not a clean answer.
    let diff = git::run(
        &worktree_path,
        &["diff", "--quiet", resolved_base.as_str(), "--"],
    )
    .await;
    if !matches!(diff.status, Some(0 | 1)) {
        blocked!(Verdict::Unknown(UnknownReason::DiffCheckFailed {
            detail: git::failure(&diff, "git diff")
        }));
    }
    let ancestor = git::run(
        ctx.repo_root,
        &[
            "merge-base",
            "--is-ancestor",
            &git_record.head,
            ctx.target_head,
        ],
    )
    .await;
    if !matches!(ancestor.status, Some(0 | 1)) {
        blocked!(Verdict::Unknown(UnknownReason::AncestryCheckFailed {
            detail: git::failure(&ancestor, "git merge-base --is-ancestor")
        }));
    }
    let branch_tip_is_ancestor = ancestor.status == Some(0);

    // 36-37 — committed divergence survives ONLY through a validated durable patch or an
    // already-merged branch tip.
    if diff.status != Some(0) {
        match is_patch_captured(record, &worktree_path, resolved_base.as_str()).await {
            PatchCapture::Invalid(reason) => blocked!(Verdict::Ineligible(reason)),
            PatchCapture::Validated(path) => facts.patch_path = Some(path),
            PatchCapture::None if !branch_tip_is_ancestor => {
                blocked!(Verdict::Ineligible(IneligibleReason::UnmergedDivergence));
            }
            PatchCapture::None => {}
        }
    }

    // 38-40 — the manifest and every durable report must live OUTSIDE the worktree. Removing a
    // worktree that contains its own evidence would delete the record of what was lost.
    let mut durable: Vec<PathBuf> = vec![record.manifest_path.clone()];
    durable.extend(record.report_paths());
    for report_path in durable {
        let report = inspect_path(&report_path);
        if let Some(detail) = report.error {
            blocked!(Verdict::Unknown(
                UnknownReason::DurableHandoffPathUnusable {
                    detail: format!("durable handoff path could not be inspected: {detail}")
                }
            ));
        }
        let comparable = report.realpath.clone().unwrap_or_else(|| {
            if report.symlink {
                resolve_existing_path(&report_path)
            } else {
                report.resolved.clone()
            }
        });
        if path_inside(&realpath, &comparable, false) {
            blocked!(Verdict::Ineligible(
                IneligibleReason::DurableHandoffPathInsideWorktree { path: report_path }
            ));
        }
        if report.missing {
            blocked!(Verdict::Unknown(
                UnknownReason::DurableHandoffPathUnusable {
                    detail: format!("durable handoff path is missing: {}", report_path.display())
                }
            ));
        }
        if report.symlink {
            blocked!(Verdict::Unknown(
                UnknownReason::DurableHandoffPathUnusable {
                    detail: format!(
                        "durable handoff path is a symlink: {}",
                        report_path.display()
                    )
                }
            ));
        }
    }

    // 41 — the same test for the patch, which pi checks separately because a MISSING patch is not
    // itself disqualifying here (gate 36 already decided whether it had to exist).
    for durable_path in record.output_paths() {
        let inspection = inspect_path(&durable_path);
        let comparable = inspection.realpath.clone().unwrap_or_else(|| {
            if inspection.symlink {
                resolve_existing_path(&durable_path)
            } else {
                inspection.resolved.clone()
            }
        });
        if path_inside(&realpath, &comparable, false) {
            blocked!(Verdict::Ineligible(
                IneligibleReason::DurableOutputPathInsideWorktree { path: durable_path }
            ));
        }
    }

    // Everything above passed. This is the ONLY construction of `Verdict::Safe` in the crate.
    let verdict = Verdict::Safe {
        delete_branch: branch_tip_is_ancestor,
        note: (!branch_tip_is_ancestor).then_some(SafeNote::BranchNotAncestorOfTarget),
    };
    finish(
        worktree_path,
        branch,
        CleanupSource::Both,
        Some(record),
        ctx.target_head,
        &facts,
        &verdict,
    )
}

/// What [`is_patch_captured`] found.
enum PatchCapture {
    /// No usable patch was recorded — pi's `return {}` (`:592`).
    None,
    /// A patch that still faithfully represents the worktree.
    Validated(PathBuf),
    /// A patch that exists but cannot be trusted. Always `Ineligible/Keep`.
    Invalid(IneligibleReason),
}

/// pi `isPatchCaptured` (`:591-608`).
///
/// This is the one place a plan build touches git with a WRITE-shaped command (`read-tree`,
/// `add -A`), and it does so against a temporary `GIT_INDEX_FILE` — see
/// [`crate::spawn::worktree::run_git_env`], which exists for exactly this reason. The worktree's
/// real index is never touched.
async fn is_patch_captured(
    record: &MetadataRecord,
    worktree_path: &Path,
    base_commit: &str,
) -> PatchCapture {
    let Some(patch_path) = record.patch_path() else {
        return PatchCapture::None;
    };
    if record.child.patch.error.is_some() || !record.child.patch.changed {
        return PatchCapture::None;
    }
    let inspection = inspect_path(&patch_path);
    if let Some(detail) = inspection.error {
        return PatchCapture::Invalid(IneligibleReason::PatchValidationFailed {
            detail: format!("captured patch cannot be inspected: {detail}"),
        });
    }
    let Some(realpath) = inspection.realpath.clone().filter(|_| !inspection.missing) else {
        return PatchCapture::Invalid(IneligibleReason::PatchValidationFailed {
            detail: "captured handoff patch is missing".to_string(),
        });
    };
    let Ok(meta) = std::fs::metadata(&patch_path) else {
        return PatchCapture::Invalid(IneligibleReason::PatchValidationFailed {
            detail: "captured patch cannot be inspected".to_string(),
        });
    };
    if !meta.is_file() {
        return PatchCapture::Invalid(IneligibleReason::PatchValidationFailed {
            detail: "captured handoff patch is not a file".to_string(),
        });
    }
    if path_inside(worktree_path, &realpath, false) {
        return PatchCapture::Invalid(IneligibleReason::PatchValidationFailed {
            detail: "durable handoff patch lives inside the worktree".to_string(),
        });
    }
    if meta.len() == 0 {
        return PatchCapture::Invalid(IneligibleReason::PatchValidationFailed {
            detail: "captured handoff patch is empty".to_string(),
        });
    }
    match crate::spawn::worktree::validate_worktree_patch_represents_current_worktree(
        worktree_path,
        base_commit,
        &patch_path,
    )
    .await
    {
        Some(detail) => PatchCapture::Invalid(IneligibleReason::PatchValidationFailed { detail }),
        None => PatchCapture::Validated(patch_path),
    }
}
