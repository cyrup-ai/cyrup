//! The WRITER — pi `writeParallelHandoffGroup` (`:497-612`), `recordParallelHandoffMerge`
//! (`:437-453`) and `recordParallelHandoffSupersession` (`:455-470`) @v0.68.0.
//!
//! This is the half cyrup has never had: `background/async_retention/scan.rs` has read
//! `<run_dir>/handoff.json` since it was ported, and nothing produced one.
//!
//! Upstream's `input: {...}` object literals become named parameter structs. That is both the
//! Rust idiom and what keeps pi's `Omit<Parameters<typeof writeParallelHandoffGroup>[0], …>`
//! trick (`:619`) expressible: here it is a struct that CONTAINS the shared half, rather than a
//! mapped type.

use std::path::{Path, PathBuf};

use super::error::HandoffError;
use super::evidence::{
    cleanup_eligibility_for_evidence, has_active_children, manifest_facts_digest, reference_for,
    same_merge_evidence, same_supersession_evidence, validate_manifest_for_lane_evidence,
};
use super::model::{
    Child, ChildStatus, CleanupState, Group, HandoffMode, HandoffReference, HandoffSource, LaneId,
    Manifest, ManifestVersion, MergeEvidence, Patch, StoredCleanupEligibility,
    SupersessionEvidence, WorktreeCleanupIntent, WorktreeCleanupReport, WorktreeCleanupTask,
};
use super::read::read_manifest;
use crate::spawn::worktree::{WorktreeDiff, WorktreeSetup};
use crate::workflows::{WorkflowKey, WorkflowLaneMetadata};

/// pi's `cleanup pending durable handoff capture` (`:583`).
///
/// Written on EVERY task of a fresh group, BEFORE any removal is attempted. That ordering is the
/// whole safety story: a crash between the two writes leaves the worktree recorded and preserved
/// rather than orphaned and forgotten.
pub const CLEANUP_PENDING_REASON: &str = "cleanup pending durable handoff capture";

/// One settled child, as the caller of [`write_group`] knows it — pi `ParallelHandoffResult`
/// (`:26-36`).
#[derive(Clone, Debug, Default)]
pub struct HandoffResult {
    /// The agent that ran.
    pub agent: String,
    /// The child's outcome.
    pub status: Option<ChildStatus>,
    /// pi `result.output || result.error || "(no output)"`.
    pub summary: String,
    /// Where the child's `output:` handoff landed.
    pub output_path: Option<PathBuf>,
    /// The child's validated structured output. `Some(Value::Null)` still serializes `null`.
    pub structured_output: Option<serde_json::Value>,
    /// Where the structured output was persisted.
    pub structured_output_path: Option<PathBuf>,
    /// The child's session transcript.
    pub session_path: Option<PathBuf>,
    /// Stable workflow identity.
    pub workflow_key: Option<WorkflowKey>,
    /// The child's own run id.
    pub run_id: Option<LaneId>,
    /// Launch-declared lane metadata.
    pub lane: Option<WorkflowLaneMetadata>,
}

/// Everything one [`write_group`] call needs — pi's `input` object (`:497-511`).
pub struct WriteGroup<'a> {
    /// Where the manifest lives.
    pub manifest_path: &'a Path,
    /// The owning run.
    pub run_id: &'a LaneId,
    /// How the run was launched.
    pub mode: HandoffMode,
    /// Which process is writing.
    pub source: HandoffSource,
    /// The shared working directory children were launched from.
    pub cwd: &'a Path,
    /// This group's position in the chain.
    pub step_index: u32,
    /// The flat index the group's first child occupies.
    pub flat_start_index: u32,
    /// The worktree allocation this group ran in.
    pub setup: &'a WorktreeSetup,
    /// One captured diff per task, in task order. A short list is tolerated: a missing entry
    /// becomes a `changed: false` patch carrying pi's `missingDiff` error text (`:475-494`).
    pub diffs: &'a [WorktreeDiff],
    /// The removal ledger, when cleanup has already run. `None` writes pi's fresh
    /// all-preserved ledger (`:574-584`).
    pub cleanup: Option<&'a WorktreeCleanupReport>,
    /// One entry per settled child, in task order.
    pub results: &'a [HandoffResult],
    /// Launch identities, persisted ONLY while `results` is empty (`:570`).
    pub lane_bindings: Option<&'a [super::model::LaneBinding]>,
    /// Epoch millis. Injected so a test can pin it.
    pub now: i64,
}

/// pi `writeParallelHandoffGroup` (`:497-612`) — merge one group into the manifest and rewrite it
/// atomically.
///
/// Merge-by-`stepIndex`: an existing group with the same index is REPLACED and the list re-sorted
/// (`:587-589`), which is what makes the deliberate double write around cleanup idempotent.
/// `merge`/`supersession` survive a rewrite (`:599-600`), and `cleanupEligibility` is recomputed
/// only if the existing manifest already carried cleanup metadata (`:602-608`) — a fresh manifest
/// deliberately has NO `cleanupEligibility` key at all.
///
/// # Errors
///
/// [`HandoffError::ForeignManifest`] when the file on disk belongs to another run/mode/source,
/// the lane-binding validation errors, [`HandoffError::Io`] on a write failure, plus
/// [`read_manifest`]'s own errors.
pub async fn write_group(input: WriteGroup<'_>) -> Result<HandoffReference, HandoffError> {
    let existing = read_manifest(input.manifest_path).await?;
    if let Some(existing) = existing.as_ref()
        && (existing.run_id != *input.run_id
            || existing.mode != input.mode
            || existing.source != input.source)
    {
        // pi `:514-516`. runId, mode AND source are all identity: writing a parallel group into a
        // chain run's manifest would silently reattribute the worktrees.
        return Err(HandoffError::ForeignManifest(
            input.manifest_path.to_path_buf(),
        ));
    }

    let bindings = validate_lane_bindings(&input)?;
    let group = build_group(&input, bindings)?;

    let mut groups: Vec<Group> = existing
        .as_ref()
        .map(|manifest| {
            manifest
                .groups
                .iter()
                .filter(|candidate| candidate.step_index != input.step_index)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    groups.push(group);
    groups.sort_by_key(|group| group.step_index);

    let mut manifest = Manifest {
        version: ManifestVersion,
        run_id: input.run_id.clone(),
        mode: input.mode,
        source: input.source,
        cwd: input.cwd.to_path_buf(),
        created_at: existing
            .as_ref()
            .map_or(input.now, |manifest| manifest.created_at),
        updated_at: input.now,
        groups,
        merge: existing.as_ref().and_then(|m| m.merge.clone()),
        supersession: existing.as_ref().and_then(|m| m.supersession.clone()),
        cleanup_eligibility: None,
    };

    // pi `carriesCleanupMetadata` (`:602-604`): the key is (re)computed only for a manifest that
    // already reasons about cleanup. Its ABSENCE on a fresh manifest is the signal `referenceFor`
    // `hasOwnProperty`-tests.
    let carries_cleanup_metadata = existing.as_ref().is_some_and(|existing| {
        existing.merge.is_some()
            || existing.supersession.is_some()
            || existing.cleanup_eligibility.is_some()
    });
    if carries_cleanup_metadata {
        manifest.cleanup_eligibility = Some(StoredCleanupEligibility::new(
            cleanup_eligibility_for_evidence(&manifest),
        ));
    }

    write_manifest(input.manifest_path, &manifest).await?;
    Ok(reference_for(input.manifest_path, &manifest))
}

/// pi `:518-527` — every lane binding must name a real worktree, at a unique task index, with a
/// flat index consistent with `flatStartIndex`.
fn validate_lane_bindings<'a>(
    input: &'a WriteGroup<'a>,
) -> Result<Option<&'a [super::model::LaneBinding]>, HandoffError> {
    let Some(bindings) = input.lane_bindings else {
        return Ok(None);
    };
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for binding in bindings {
        if !seen.insert(binding.task_index) {
            return Err(HandoffError::DuplicateBindingTaskIndex(binding.task_index));
        }
        if !input
            .setup
            .worktrees
            .iter()
            .any(|worktree| worktree.index == binding.task_index)
        {
            return Err(HandoffError::BindingWithoutWorktree(binding.task_index));
        }
        if binding.index != input.flat_start_index.saturating_add(binding.task_index) {
            return Err(HandoffError::BindingIndexMismatch {
                index: binding.index,
                task_index: binding.task_index,
            });
        }
    }
    Ok(Some(bindings))
}

/// pi `:532-585` — the group record itself.
fn build_group(
    input: &WriteGroup<'_>,
    bindings: Option<&[super::model::LaneBinding]>,
) -> Result<Group, HandoffError> {
    let mut children = Vec::with_capacity(input.results.len());
    for (task_index, result) in input.results.iter().enumerate() {
        let task_index = u32::try_from(task_index).unwrap_or(u32::MAX);
        let binding = bindings.and_then(|bindings| {
            bindings
                .iter()
                .find(|binding| binding.task_index == task_index)
        });
        // pi `result.runId ?? binding?.runId` (`:537-540`): the settled child's own identity wins,
        // and the launch binding is the fallback.
        let run_id = result
            .run_id
            .clone()
            .or_else(|| binding.and_then(|b| b.run_id.clone()));
        let workflow_key = result
            .workflow_key
            .clone()
            .or_else(|| binding.and_then(|b| b.workflow_key.clone()));
        let lane = result
            .lane
            .clone()
            .or_else(|| binding.and_then(|b| b.lane.clone()));
        // pi `assertWorkflowLaneKey` at `:541`. The crate's landed port is REUSED (a second
        // implementation of a cross-field relation is how the two drift); its prose is then
        // re-expressed as the structural defect a READ of this same file would report, so a
        // caller sees one vocabulary rather than two.
        if crate::workflows::assert_workflow_lane_key(
            lane.as_ref(),
            workflow_key.as_ref(),
            &format!("parallel handoff child {task_index}.lane"),
        )
        .is_err()
        {
            return Err(HandoffError::InvalidManifest {
                path: input.manifest_path.to_path_buf(),
                detail: super::error::ManifestDefect::LaneKeyMismatch {
                    label: format!("parallel handoff child {task_index}.lane"),
                    lane_key: lane
                        .as_ref()
                        .map(|lane| lane.key.as_str().to_string())
                        .unwrap_or_default(),
                    workflow_key: workflow_key
                        .as_ref()
                        .map(|key| key.as_str().to_string())
                        .unwrap_or_default(),
                },
            });
        }

        let patch = input
            .diffs
            .iter()
            .find(|diff| diff.index == task_index)
            .map_or_else(
                || missing_patch(input, task_index),
                |diff| Patch {
                    path: diff.patch_path.clone(),
                    branch: diff.branch.clone(),
                    // pi `:565`: any of the four counters non-zero.
                    changed: diff.files_changed > 0
                        || diff.insertions > 0
                        || diff.deletions > 0
                        || !diff.diff_stat.trim().is_empty(),
                    diff_stat: diff.diff_stat.clone(),
                    files_changed: diff.files_changed,
                    insertions: diff.insertions,
                    deletions: diff.deletions,
                    // The capture's OWN verdict, not a hardcoded success. A capture that failed
                    // wrote a zero-byte `.patch`; recording it here with no error would make the
                    // manifest assert that a task produced nothing, and would let
                    // `handoff_records_patch` certify that placeholder as durable evidence to
                    // the very gate that exists to refuse removing the worktree it came from.
                    error: diff.error.clone(),
                },
            );

        children.push(Child {
            index: input.flat_start_index.saturating_add(task_index),
            task_index,
            agent: result.agent.clone(),
            workflow_key,
            run_id,
            lane,
            status: result.status.unwrap_or(ChildStatus::Failed),
            summary: result.summary.clone(),
            output_path: result.output_path.clone(),
            structured_output: result.structured_output.clone(),
            structured_output_path: result.structured_output_path.clone(),
            session_path: result.session_path.clone(),
            patch,
        });
    }

    // pi `:586`: bindings are launch-time identity that child rows REPLACE, so they are persisted
    // only while there are no child rows yet.
    let lane_bindings = bindings
        .filter(|bindings| !bindings.is_empty() && input.results.is_empty())
        .map(<[super::model::LaneBinding]>::to_vec);

    let cleanup = input
        .cleanup
        .cloned()
        .unwrap_or_else(|| WorktreeCleanupReport {
            state: CleanupState::Partial,
            pruned: false,
            tasks: input
                .setup
                .worktrees
                .iter()
                .map(|worktree| WorktreeCleanupTask {
                    index: worktree.index,
                    path: worktree.path.clone(),
                    branch: worktree.branch.clone(),
                    // The allocator, recorded from phase 1 so a crash between the two writes
                    // still leaves a row a later discard can act on (pi `worktree.ts:1272`
                    // copies it onto every row; `:929` stamps it at allocation).
                    provider: Some(crate::workflows::ManagedWorktreeProvider::Native),
                    naming: None,
                    worktree_removed: false,
                    branch_removed: false,
                    preserved: Some(true),
                    reason: Some(CLEANUP_PENDING_REASON.to_string()),
                    errors: None,
                })
                .collect(),
            errors: None,
        });

    Ok(Group {
        step_index: input.step_index,
        base_commit: input.setup.base_commit.clone(),
        repo_root: input.setup.cwd.clone(),
        children,
        lane_bindings,
        cleanup,
    })
}

/// pi `missingDiff` (`:475-494`) — a task whose diff was never captured still gets a patch row,
/// carrying the reason, so the manifest never has a hole where evidence should be.
///
/// [CYRUP-DELTA] upstream also CREATES the empty `.patch` file here. cyrup does not: its own
/// `diff_worktrees` (`spawn/worktree.rs:1316`) already writes an empty patch on a per-task capture
/// failure, so the file exists whenever a capture was attempted, and this arm is reached only when
/// no capture was attempted at all — where writing a zero-byte file into an artifacts directory
/// would fabricate evidence of a harvest that never ran.
fn missing_patch(input: &WriteGroup<'_>, task_index: u32) -> Patch {
    let branch = input
        .setup
        .worktrees
        .iter()
        .find(|worktree| worktree.index == task_index)
        .map(|worktree| worktree.branch.clone())
        .unwrap_or_default();
    Patch {
        path: input
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(
                "missing-diff-step-{}-task-{task_index}.patch",
                input.step_index
            )),
        branch,
        changed: false,
        diff_stat: String::new(),
        files_changed: 0,
        insertions: 0,
        deletions: 0,
        error: Some("diff artifact unavailable; no patch was captured".to_string()),
    }
}

/// The result of recording an attestation — pi `ParallelHandoffEvidenceResult` (`:432-436`).
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceOutcome {
    /// The manifest as it now stands on disk.
    pub manifest: Manifest,
    /// The compact reference for a status payload.
    pub reference: HandoffReference,
    /// The model-facing eligibility rendering.
    pub text: String,
}

/// Everything one [`record_merge`] call needs — pi's `input` object (`:437`).
pub struct RecordMerge<'a> {
    /// The manifest to attest against.
    pub manifest_path: &'a Path,
    /// The lane being attested. Must equal the manifest's own run id.
    pub lane_id: &'a LaneId,
    /// The attestation. Its `manifest_digest` is IGNORED and overwritten.
    pub merge: MergeEvidence,
    /// Epoch millis.
    pub now: i64,
}

/// pi `recordParallelHandoffMerge` (`:437-453`).
///
/// Three rules, all load-bearing:
/// * the caller's `manifestDigest` is DISCARDED and re-stamped from the manifest's own facts
///   (`:442`), so evidence cannot be forged against a manifest it was not made for;
/// * a second merge naming a DIFFERENT reviewed head is `Lane manifest is stale` (`:445`), and
///   the same head with different details is a conflict (`:446`) — an identical re-record is
///   idempotent and allowed;
/// * merge evidence DELETES any stored supersession (`:449`). Merge supersedes supersession, not
///   the reverse (note that the eligibility DERIVATION checks supersession first, `:369`).
///
/// # Errors
///
/// [`HandoffError::NotFound`], the lane-evidence validation errors,
/// [`HandoffError::StaleReviewedHead`], [`HandoffError::ConflictingMerge`], or a write failure.
pub async fn record_merge(input: RecordMerge<'_>) -> Result<EvidenceOutcome, HandoffError> {
    let Some(manifest) = read_manifest(input.manifest_path).await? else {
        return Err(HandoffError::NotFound(input.manifest_path.to_path_buf()));
    };
    validate_manifest_for_lane_evidence(&manifest, input.lane_id, "merge evidence")?;
    if has_active_children(&manifest) {
        return Err(HandoffError::ActiveChildOwner {
            what: "merge evidence",
        });
    }

    let mut merge = input.merge;
    merge.manifest_digest = Some(manifest_facts_digest(&manifest)?);
    if let Some(existing) = manifest.merge.as_ref() {
        if existing.reviewed_head != merge.reviewed_head {
            return Err(HandoffError::StaleReviewedHead {
                existing: existing.reviewed_head.clone(),
            });
        }
        if !same_merge_evidence(existing, &merge) {
            return Err(HandoffError::ConflictingMerge);
        }
    }

    let mut updated = Manifest {
        merge: Some(merge),
        supersession: None,
        updated_at: input.now,
        ..manifest
    };
    updated.cleanup_eligibility = Some(StoredCleanupEligibility::new(
        cleanup_eligibility_for_evidence(&updated),
    ));
    finish_evidence(input.manifest_path, updated).await
}

/// Everything one [`record_supersession`] call needs — pi's `input` object (`:455`).
pub struct RecordSupersession<'a> {
    /// The manifest to attest against.
    pub manifest_path: &'a Path,
    /// The lane being superseded.
    pub lane_id: &'a LaneId,
    /// The attestation. Its `manifest_digest` is IGNORED and overwritten.
    pub supersession: SupersessionEvidence,
    /// Epoch millis.
    pub now: i64,
}

/// pi `recordParallelHandoffSupersession` (`:455-470`).
///
/// # Errors
///
/// [`HandoffError::NotFound`], the lane-evidence validation errors,
/// [`HandoffError::SelfSupersession`], [`HandoffError::ConflictingSupersession`], or a write
/// failure.
pub async fn record_supersession(
    input: RecordSupersession<'_>,
) -> Result<EvidenceOutcome, HandoffError> {
    let Some(manifest) = read_manifest(input.manifest_path).await? else {
        return Err(HandoffError::NotFound(input.manifest_path.to_path_buf()));
    };
    validate_manifest_for_lane_evidence(&manifest, input.lane_id, "supersession")?;
    if has_active_children(&manifest) {
        return Err(HandoffError::ActiveChildOwner {
            what: "supersession",
        });
    }

    let mut supersession = input.supersession;
    supersession.manifest_digest = Some(manifest_facts_digest(&manifest)?);
    // pi `:462` — a lane cannot supersede itself.
    if supersession.superseded_by == *input.lane_id {
        return Err(HandoffError::SelfSupersession);
    }
    if let Some(existing) = manifest.supersession.as_ref()
        && !same_supersession_evidence(existing, &supersession)
    {
        return Err(HandoffError::ConflictingSupersession);
    }

    let mut updated = Manifest {
        supersession: Some(supersession),
        updated_at: input.now,
        ..manifest
    };
    updated.cleanup_eligibility = Some(StoredCleanupEligibility::new(
        cleanup_eligibility_for_evidence(&updated),
    ));
    finish_evidence(input.manifest_path, updated).await
}

/// What a [`discard_preserved`] call did — pi's `{ manifest, text }` return (`:686`).
#[derive(Clone, Debug, PartialEq)]
pub struct DiscardOutcome {
    /// The manifest as it now stands on disk.
    pub manifest: Manifest,
    /// The model-facing rendering, including the manual commands for anything left behind.
    pub text: String,
}

/// pi `discardPreservedWorktrees` (`:684-741`) — remove the worktrees a fan-out deliberately
/// KEPT, and rewrite the manifest's removal ledger to say what actually happened.
///
/// Only tasks that are `preserved` AND not already doubly-removed are touched (`:693`). Every
/// other row is copied through untouched, so a group that was already cleaned up is a no-op
/// rather than a second round of `git worktree remove` against paths that no longer exist.
///
/// Three rules that a transliteration loses, all of them load-bearing:
/// * a task whose report row does not show BOTH removals is forced back to `preserved: true`
///   with a reason (`:710-712`) — the ledger never claims a removal that did not happen;
/// * `cleanup.state` becomes [`CleanupState::Complete`] only if EVERY task in the group (not just
///   the attempted ones) is doubly-removed **and** the prune succeeded (`:714-717`);
/// * the caller is handed the manual `git status --short` / `worktree remove --force` /
///   `branch -D` lines for whatever remains (`:730-738`), because a partial discard that says
///   nothing leaves an operator with orphan worktrees and no way to find them.
///
/// **Serialization is the caller's job.** Upstream only ever calls this inside
/// `withWorktreeTransaction` (`subagent-executor.ts:6261`); here that is
/// [`crate::spawn::worktree::worktree_turn`], held by `route_action`'s `worktree.discard` arm
/// across this whole read-modify-write.
///
/// That turn is process-local, so it does not exclude a second cyrup process — and the filter
/// below matches on `preserved`, which every row carries during the window between the harvest's
/// two manifest writes ([`CLEANUP_PENDING_REASON`]). A discard issued inside that window can
/// therefore target a group that is still settling. Narrowing the filter would NOT fix it: those
/// same rows are what a crash between the two writes leaves behind, and acting on them is the
/// whole reason this function exists. What actually protects the work is that
/// [`crate::spawn::worktree::cleanup_worktrees`] re-probes each worktree at removal time and
/// refuses a dirty one that the authority decision does not clear, rather than trusting the
/// ledger's snapshot. This matches upstream, whose transaction is likewise process-local.
///
/// # Errors
///
/// [`HandoffError::NotFound`] when no manifest is at `manifest_path`, the manifest's own
/// validation errors, or a write failure.
pub async fn discard_preserved(
    manifest_path: &Path,
    authorization: super::model::DiscardAuthorization,
) -> Result<DiscardOutcome, HandoffError> {
    let Some(mut manifest) = read_manifest(manifest_path).await? else {
        return Err(HandoffError::NotFound(manifest_path.to_path_buf()));
    };

    let mut attempted = 0usize;
    for group in &mut manifest.groups {
        let pending: Vec<&WorktreeCleanupTask> = group
            .cleanup
            .tasks
            .iter()
            .filter(|task| {
                task.preserved == Some(true) && !(task.worktree_removed && task.branch_removed)
            })
            .collect();
        if pending.is_empty() {
            continue;
        }
        attempted += pending.len();

        // pi rebuilds a synthetic `WorktreeSetup` from the LEDGER, not from live state (`:697`):
        // the manifest is the durable record and the worktrees it names may well be all that is
        // left of the fan-out that created them.
        let setup = WorktreeSetup {
            cwd: group.repo_root.clone(),
            base_commit: group.base_commit.clone(),
            worktrees: pending
                .iter()
                .map(|task| crate::spawn::worktree::WorktreeInfo {
                    path: task.path.clone(),
                    agent_cwd: task.path.clone(),
                    branch: task.branch.clone(),
                    index: task.index,
                    node_modules_linked: false,
                    synthetic_paths: Vec::new(),
                })
                .collect(),
        };
        let report = crate::spawn::worktree::cleanup_worktrees(
            &setup,
            &WorktreeCleanupIntent::Discard { authorization },
        )
        .await;

        // pi `:708-712`.
        let updates: std::collections::HashMap<u32, WorktreeCleanupTask> = report
            .tasks
            .iter()
            .map(|task| {
                let row = if task.worktree_removed && task.branch_removed {
                    task.clone()
                } else {
                    WorktreeCleanupTask {
                        preserved: Some(true),
                        reason: task
                            .reason
                            .clone()
                            .or_else(|| Some("discard cleanup remains incomplete".to_string())),
                        ..task.clone()
                    }
                };
                (task.index, row)
            })
            .collect();

        let tasks: Vec<WorktreeCleanupTask> = group
            .cleanup
            .tasks
            .iter()
            .map(|task| {
                updates
                    .get(&task.index)
                    .cloned()
                    .unwrap_or_else(|| task.clone())
            })
            .collect();
        let all_removed = tasks
            .iter()
            .all(|task| task.worktree_removed && task.branch_removed);
        group.cleanup = WorktreeCleanupReport {
            // pi `:714-717` — `complete` needs the whole group removed AND a successful prune.
            state: if all_removed && report.pruned {
                CleanupState::Complete
            } else {
                CleanupState::Partial
            },
            tasks,
            pruned: report.pruned,
            errors: report.errors,
        };
    }

    manifest.updated_at = crate::time::now_epoch_millis();
    write_manifest(manifest_path, &manifest).await?;

    Ok(DiscardOutcome {
        text: format_discard(manifest_path, &manifest, attempted),
        manifest,
    })
}

/// pi `:722-739` — the discard rendering, including the manual recovery commands.
fn format_discard(manifest_path: &Path, manifest: &Manifest, attempted: usize) -> String {
    let path = manifest_path.to_string_lossy();
    let mut lines: Vec<String> = if attempted == 0 {
        vec![format!("No preserved worktrees remain in {path}.")]
    } else {
        vec![
            format!(
                "Discard processed {attempted} preserved worktree{}.",
                if attempted == 1 { "" } else { "s" }
            ),
            format!("Manifest: {path}"),
        ]
    };
    let remaining: Vec<(&Group, &WorktreeCleanupTask)> = manifest
        .groups
        .iter()
        .flat_map(|group| group.cleanup.tasks.iter().map(move |task| (group, task)))
        .filter(|(_, task)| {
            task.preserved == Some(true) && !(task.worktree_removed && task.branch_removed)
        })
        .collect();
    if !remaining.is_empty() {
        lines.push(String::new());
        lines.push(
            "Some worktrees remain. Inspect and remove them manually if appropriate:".to_string(),
        );
        for (group, task) in remaining {
            // pi's `JSON.stringify(<string>)` quoting — the commands are meant to be pasted, and
            // a path with a space in it must survive that.
            let root = json_quote(&group.repo_root.to_string_lossy());
            lines.push(format!("  git -C {root} status --short"));
            lines.push(format!(
                "  git -C {root} worktree remove --force {}",
                json_quote(&task.path.to_string_lossy())
            ));
            lines.push(format!(
                "  git -C {root} branch -D {}",
                json_quote(&task.branch)
            ));
        }
    }
    lines.join("\n")
}

fn json_quote(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

async fn finish_evidence(
    manifest_path: &Path,
    manifest: Manifest,
) -> Result<EvidenceOutcome, HandoffError> {
    write_manifest(manifest_path, &manifest).await?;
    let reference = reference_for(manifest_path, &manifest);
    let text = super::format::format_stored_cleanup(manifest_path, Some(&manifest));
    Ok(EvidenceOutcome {
        manifest,
        reference,
        text,
    })
}

/// The one write path. `groups` is asserted non-empty because this crate's own retention reader
/// treats an empty `groups` array as unresolved-FOREVER (`background/async_retention/scan.rs`),
/// which would pin the run directory against retention permanently.
async fn write_manifest(manifest_path: &Path, manifest: &Manifest) -> Result<(), HandoffError> {
    debug_assert!(
        !manifest.groups.is_empty(),
        "a handoff manifest is never written with an empty `groups` array"
    );
    crate::background::atomic::write_atomic_json_creating_parent(manifest_path, manifest)
        .await
        .map_err(HandoffError::Io)
}
