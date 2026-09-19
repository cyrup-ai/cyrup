//! The safety core — pi `parallel-handoff.ts:232-385` @v0.68.0.
//!
//! Three rules live here and none of them may be short-circuited:
//!
//! 1. **Evidence is digest-bound.** `manifestDigest` covers `{version, runId, mode, source, cwd,
//!    groups}` ONLY (`manifestFactsDigest:232-241`) — `createdAt`/`updatedAt`/`merge`/
//!    `supersession`/`cleanupEligibility` are deliberately excluded, so recording evidence does
//!    not invalidate it, but a later group write does.
//! 2. **Active children block everything.** A group with cleanup tasks and ZERO children is
//!    ACTIVE (`:330`) — an allocated-but-unsettled worktree blocks removal.
//! 3. **A stored verdict is never believed.** [`trusted_cleanup_eligibility`] re-derives it, and
//!    disagreement collapses to [`CleanupEligibility::Unknown`], which formats as
//!    "removal is not safe" (`:424`).

use super::error::HandoffError;
use super::model::{
    BlockReason, CleanupEligibility, CleanupState, Manifest, ManifestDigest, MergeEvidence,
    SupersessionEvidence, TreeEquivalence,
};

/// pi `manifestFactsDigest` (`:232-241`).
///
/// The hash is [`crate::workflows::stable_json_digest`], this crate's exact port of pi
/// `stableJsonDigest` (`launch-contract.ts:21-23`). Its own `[CYRUP-DELTA]` about byte-order key
/// sorting already covers this payload: the six keys are all lowercase-initial ASCII, so byte
/// order and `localeCompare` agree and a cyrup digest equals a pi digest for the same manifest.
///
/// # Errors
///
/// [`HandoffError::Json`] if the manifest's own groups cannot be re-serialized (structurally
/// impossible for a value that deserialized, kept because `to_value` is fallible).
pub fn manifest_facts_digest(manifest: &Manifest) -> Result<ManifestDigest, HandoffError> {
    let facts = serde_json::json!({
        "version": super::model::ManifestVersion::VALUE,
        "runId": manifest.run_id.as_str(),
        "mode": serde_json::to_value(manifest.mode).map_err(HandoffError::Json)?,
        "source": serde_json::to_value(manifest.source).map_err(HandoffError::Json)?,
        "cwd": manifest.cwd.to_string_lossy(),
        "groups": serde_json::to_value(&manifest.groups).map_err(HandoffError::Json)?,
    });
    ManifestDigest::parse(&crate::workflows::stable_json_digest(&facts))
}

/// pi `hasActiveChildren` (`:326-333`).
///
/// A group with cleanup tasks and no children at all is ACTIVE: the worktrees were allocated and
/// nothing has reported back. Do not weaken this to "no *running* children".
#[must_use]
pub fn has_active_children(manifest: &Manifest) -> bool {
    manifest.groups.iter().any(|group| {
        if group.children.is_empty() && !group.cleanup.tasks.is_empty() {
            return true;
        }
        group
            .children
            .iter()
            .any(|child| !child.status.is_terminal())
    })
}

/// pi `hasManagedWorktreeTasks` (`:335-337`).
#[must_use]
pub fn has_managed_worktree_tasks(manifest: &Manifest) -> bool {
    !manifest.groups.is_empty()
        && manifest
            .groups
            .iter()
            .any(|group| !group.cleanup.tasks.is_empty())
}

/// pi `hasValidStoredLaneShape` (`:299-311`).
///
/// Most of upstream's clauses are discharged by the types (`version`, a parsed `runId`, a
/// `CleanupState` that cannot be a third word, a `ChildStatus` that cannot be an unknown string).
/// What remains is genuinely about VALUES: a non-empty group list, non-blank `baseCommit`/
/// `repoRoot`/`path`/`branch`, and at least one managed task somewhere in the manifest.
#[must_use]
pub fn has_valid_stored_lane_shape(manifest: &Manifest) -> bool {
    if manifest.groups.is_empty() {
        return false;
    }
    let mut has_managed_task = false;
    for group in &manifest.groups {
        if group.base_commit.trim().is_empty() || group.repo_root.as_os_str().is_empty() {
            return false;
        }
        if group
            .cleanup
            .tasks
            .iter()
            .any(|task| task.path.as_os_str().is_empty() || task.branch.trim().is_empty())
        {
            return false;
        }
        if !group.cleanup.tasks.is_empty() {
            has_managed_task = true;
        }
    }
    has_managed_task
}

/// pi `cleanupEligibilityForEvidence` (`:359-385`) — the derivation, in upstream's exact order.
///
/// Note the two orderings that are load-bearing:
/// * supersession is checked BEFORE merge (`:369` before `:378`), so a lane replaced by another
///   is `superseded-eligible` even when it also carries merge evidence;
/// * `record_merge` nevertheless DELETES any stored supersession (`:449`), so merge evidence
///   recorded later supersedes supersession, not the reverse.
#[must_use]
pub fn cleanup_eligibility_for_evidence(manifest: &Manifest) -> CleanupEligibility {
    if has_active_children(manifest) {
        return CleanupEligibility::Active;
    }
    let digest = manifest_facts_digest(manifest).ok();

    // pi normalizes the stored `merge` object here and blocks with
    // `BlockReason::StoredMergeEvidenceInvalid` if normalization throws (`:363-367`).
    //
    // [CYRUP-DELTA] on THIS side a malformed `merge` object cannot reach this function: the field
    // is typed, so `read_manifest` refuses the whole file instead. The behavioural difference is
    // real and worth stating — pi answers "removal is not safe, because the merge evidence is
    // invalid" where cyrup answers "this manifest is unreadable" — but both are fail-CLOSED, and
    // the alternative (a lenient `merge` that parses garbage so it can be rejected later) would
    // mean carrying an unvalidated attestation through the type system precisely so it could be
    // re-validated at every read. The arm below is what remains reachable: evidence that cannot be
    // judged because the manifest's own facts will not digest.
    if manifest.merge.is_some() && digest.is_none() {
        return CleanupEligibility::TerminalBlocked {
            reason: BlockReason::StoredMergeEvidenceInvalid,
        };
    }

    if let Some(supersession) = manifest.supersession.as_ref() {
        let Some(expected) = digest.as_ref() else {
            return CleanupEligibility::TerminalBlocked {
                reason: BlockReason::StoredSupersessionEvidenceInvalid,
            };
        };
        if supersession.manifest_digest.as_ref() != Some(expected) {
            return CleanupEligibility::TerminalBlocked {
                reason: BlockReason::StoredSupersessionEvidenceStale,
            };
        }
        return CleanupEligibility::SupersededEligible;
    }

    let Some(merge) = manifest.merge.as_ref() else {
        return CleanupEligibility::TerminalBlocked {
            reason: BlockReason::NoEvidenceRecorded,
        };
    };
    if merge.manifest_digest.as_ref() != digest.as_ref() {
        return CleanupEligibility::TerminalBlocked {
            reason: BlockReason::StoredMergeEvidenceStale,
        };
    }
    match merge.tree_equivalent {
        TreeEquivalence::Yes => {}
        TreeEquivalence::Unknown => {
            return CleanupEligibility::TerminalBlocked {
                reason: BlockReason::TreeEquivalenceNotAttested,
            };
        }
        TreeEquivalence::No => {
            return CleanupEligibility::TerminalBlocked {
                reason: BlockReason::TreeNotEquivalent,
            };
        }
    }
    if merge.post_merge_checks != super::model::PostMergeChecks::Recorded {
        return CleanupEligibility::TerminalBlocked {
            reason: BlockReason::PostMergeChecksNotRecorded,
        };
    }
    CleanupEligibility::TerminalEligible
}

/// pi `trustedStoredCleanupEligibility` (`:313-324`) — **the anti-forgery rule.**
///
/// An attacker (or a careless hand-edit) who sets `cleanupEligibility` to `terminal-eligible` in
/// the file gets [`CleanupEligibility::Unknown`] back, which formats as "removal is not safe"
/// (`:424`), because the verdict is re-derived from the evidence and the two must agree. The one
/// permitted disagreement is a DOWNGRADE to a stale-evidence blocker — a fact about the evidence
/// rather than a different opinion about the verdict (pi's `/ evidence is stale$/` regex at
/// `:320`, here [`BlockReason::is_stale`]).
#[must_use]
pub fn trusted_cleanup_eligibility(manifest: &Manifest) -> CleanupEligibility {
    let stored = manifest
        .cleanup_eligibility
        .as_ref()
        .map_or(CleanupEligibility::Unknown, |value| value.stored().clone());
    if !has_valid_stored_lane_shape(manifest) {
        return CleanupEligibility::Unknown;
    }
    if has_active_children(manifest) {
        return CleanupEligibility::Active;
    }
    match stored {
        CleanupEligibility::Active => cleanup_eligibility_for_evidence(manifest),
        CleanupEligibility::TerminalEligible | CleanupEligibility::SupersededEligible => {
            let current = cleanup_eligibility_for_evidence(manifest);
            let agrees = current == stored;
            let stale_downgrade = matches!(
                &current,
                CleanupEligibility::TerminalBlocked { reason } if reason.is_stale()
            );
            if agrees || stale_downgrade {
                current
            } else {
                CleanupEligibility::Unknown
            }
        }
        other => other,
    }
}

/// pi `validateManifestForLaneEvidence` (`:436-453`) — the gate both attestation recorders pass
/// through before anything is written.
///
/// # Errors
///
/// [`HandoffError::LaneRunMismatch`], [`HandoffError::LaneManifestMalformed`],
/// [`HandoffError::NoManagedWorktreeTasks`] or [`HandoffError::ActiveChildOwner`].
pub fn validate_manifest_for_lane_evidence(
    manifest: &Manifest,
    lane_id: &super::model::LaneId,
    what: &'static str,
) -> Result<(), HandoffError> {
    if manifest.run_id != *lane_id {
        return Err(HandoffError::LaneRunMismatch {
            lane: lane_id.as_str().to_string(),
            run: manifest.run_id.as_str().to_string(),
        });
    }
    if !has_valid_stored_lane_shape(manifest) {
        return Err(HandoffError::LaneManifestMalformed);
    }
    if manifest.groups.is_empty() || !has_managed_worktree_tasks(manifest) {
        return Err(HandoffError::NoManagedWorktreeTasks);
    }
    if has_active_children(manifest) {
        return Err(HandoffError::ActiveChildOwner { what });
    }
    Ok(())
}

/// pi `sameMergeEvidence` (`:388-396`) — deliberately EXCLUDES `manifestDigest`, because the
/// recorder stamps a fresh one on every attempt and an idempotent re-record must still compare
/// equal.
#[must_use]
pub fn same_merge_evidence(left: &MergeEvidence, right: &MergeEvidence) -> bool {
    left.pr_number == right.pr_number
        && left.reviewed_head == right.reviewed_head
        && left.merge_commit == right.merge_commit
        && left.tree_equivalent == right.tree_equivalent
        && left.post_merge_checks == right.post_merge_checks
        && left.attested_by == right.attested_by
        && left.attested_at == right.attested_at
}

/// pi `sameSupersessionEvidence` (`:398-400`).
#[must_use]
pub fn same_supersession_evidence(
    left: &SupersessionEvidence,
    right: &SupersessionEvidence,
) -> bool {
    left.superseded_by == right.superseded_by
        && left.attested_by == right.attested_by
        && left.attested_at == right.attested_at
}

/// pi `referenceFor` (`:195-204`).
///
/// `cleanup_eligibility` is projected only when the manifest CARRIES the key (pi's
/// `hasOwnProperty` test), and the projected value is the re-derived, trusted one — never the
/// stored string.
#[must_use]
pub fn reference_for(
    manifest_path: &std::path::Path,
    manifest: &Manifest,
) -> super::model::HandoffReference {
    let children = manifest.groups.iter().flat_map(|group| &group.children);
    let child_count = u32::try_from(children.clone().count()).unwrap_or(u32::MAX);
    let changed =
        u32::try_from(children.filter(|child| child.patch.changed).count()).unwrap_or(u32::MAX);
    super::model::HandoffReference {
        version: super::model::ManifestVersion,
        path: manifest_path.to_path_buf(),
        group_count: u32::try_from(manifest.groups.len()).unwrap_or(u32::MAX),
        child_count,
        changed_patches: changed,
        cleanup_state: if manifest
            .groups
            .iter()
            .all(|group| group.cleanup.state == CleanupState::Complete)
        {
            CleanupState::Complete
        } else {
            CleanupState::Partial
        },
        cleanup_eligibility: manifest
            .cleanup_eligibility
            .as_ref()
            .map(|_| trusted_cleanup_eligibility(manifest)),
    }
}
