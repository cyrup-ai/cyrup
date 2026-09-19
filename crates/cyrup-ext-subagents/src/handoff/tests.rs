//! Unit tests for the manifest model, the anti-forgery rule and the writer.
//!
//! The end-to-end REACHABILITY test — the one that proves a real `worktree: true` fan-out
//! publishes a real file through the real background runner — lives in
//! `crates/cyrup-it/tests/subagents/background_runner_main_integration.rs`. These are the
//! cheap, same-file companions: the on-disk SPELLING of the two hand-written serde impls (which
//! a `rename_all` typo would otherwise sail past every round-trip test), and the trust-laundering
//! rule whose absence would be silently exploitable.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

use super::*;
use crate::spawn::worktree::{WorktreeInfo, WorktreeSetup};

fn lane(raw: &str) -> LaneId {
    LaneId::parse(raw).expect("valid lane id")
}

fn sha(byte: char) -> CommitSha {
    CommitSha::parse(&byte.to_string().repeat(40)).expect("valid sha")
}

fn digest_of(manifest: &Manifest) -> ManifestDigest {
    manifest_facts_digest(manifest).expect("digest")
}

fn cleanup_task(index: u32) -> WorktreeCleanupTask {
    WorktreeCleanupTask {
        index,
        path: PathBuf::from(format!("/tmp/wt-{index}")),
        branch: format!("cyrup-parallel-run-{index}"),
        provider: Some(crate::workflows::ManagedWorktreeProvider::Native),
        naming: None,
        worktree_removed: false,
        branch_removed: false,
        preserved: Some(true),
        reason: Some(CLEANUP_PENDING_REASON.to_string()),
        errors: None,
    }
}

fn child(index: u32, status: ChildStatus) -> Child {
    Child {
        index,
        task_index: index,
        agent: format!("worker-{index}"),
        workflow_key: None,
        run_id: None,
        lane: None,
        status,
        summary: "done".to_string(),
        output_path: None,
        structured_output: None,
        structured_output_path: None,
        session_path: None,
        patch: Patch {
            path: PathBuf::from(format!("/tmp/task-{index}.patch")),
            branch: format!("cyrup-parallel-run-{index}"),
            changed: true,
            diff_stat: " a.rs | 2 +-".to_string(),
            files_changed: 1,
            insertions: 1,
            deletions: 1,
            error: None,
        },
    }
}

/// One settled group, one terminal child, one preserved worktree.
fn manifest() -> Manifest {
    Manifest {
        version: ManifestVersion,
        run_id: lane("run-1"),
        mode: HandoffMode::Parallel,
        source: HandoffSource::Async,
        cwd: PathBuf::from("/repo"),
        created_at: 1_758_153_600_000,
        updated_at: 1_758_153_611_000,
        groups: vec![Group {
            step_index: 0,
            base_commit: "abc123".to_string(),
            repo_root: PathBuf::from("/repo"),
            children: vec![child(0, ChildStatus::Completed)],
            lane_bindings: None,
            cleanup: WorktreeCleanupReport {
                state: CleanupState::Partial,
                tasks: vec![cleanup_task(0)],
                pruned: false,
                errors: None,
            },
        }],
        merge: None,
        supersession: None,
        cleanup_eligibility: None,
    }
}

fn setup() -> WorktreeSetup {
    WorktreeSetup {
        cwd: PathBuf::from("/repo"),
        base_commit: "abc123".to_string(),
        worktrees: vec![WorktreeInfo {
            path: PathBuf::from("/tmp/wt-0"),
            agent_cwd: PathBuf::from("/tmp/wt-0"),
            branch: "cyrup-parallel-run-0".to_string(),
            index: 0,
            node_modules_linked: false,
            synthetic_paths: Vec::new(),
        }],
    }
}

// =================================================================================================
// The on-disk spelling — the interface itself
// =================================================================================================

/// The two HAND-WRITTEN serde impls ([`TreeEquivalence`], [`BlockReason`]) plus the tagged
/// [`CleanupEligibility`]'s kebab-case rename are the on-disk contract. A round-trip test would
/// pass with any spelling; this pins the literal bytes.
#[test]
fn on_disk_spelling_is_upstreams() {
    let mut stored = manifest();
    stored.cleanup_eligibility = Some(StoredCleanupEligibility::new(
        CleanupEligibility::TerminalBlocked {
            reason: BlockReason::StoredMergeEvidenceStale,
        },
    ));
    stored.merge = Some(MergeEvidence {
        pr_number: PrNumber::parse(42).unwrap(),
        reviewed_head: sha('a'),
        merge_commit: sha('b'),
        tree_equivalent: TreeEquivalence::Yes,
        post_merge_checks: PostMergeChecks::Recorded,
        attested_by: AttestationActor::parse("reviewer").unwrap(),
        attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
        manifest_digest: None,
    });
    let json = serde_json::to_string(&stored).expect("serialize");

    assert!(
        json.contains(
            r#""cleanupEligibility":{"state":"terminal-blocked","reason":"stored merge evidence is stale"}"#
        ),
        "tagged kebab-case eligibility: {json}"
    );
    assert!(
        json.contains(r#""treeEquivalent":true"#),
        "bool arm: {json}"
    );
    assert!(
        json.contains(r#""postMergeChecks":"recorded""#),
        "camelCase enum: {json}"
    );
    assert!(json.contains(r#""version":1"#), "literal version: {json}");
    assert!(
        json.contains(r#""stepIndex":0"#) && json.contains(r#""baseCommit":"abc123""#),
        "camelCase records: {json}"
    );
    // The three spellings the retention reader pins.
    assert!(json.contains(r#""cleanup":{"state":"partial""#), "{json}");
    // Absent-not-null: an unset optional is ELIDED, never written as `null`.
    assert!(!json.contains("null"), "no nulls on the wire: {json}");

    let round_tripped: Manifest = serde_json::from_str(&json).expect("round trip");
    assert_eq!(round_tripped, stored);
}

/// `treeEquivalent` is a mixed bool/string union: `true`, `false`, `"unknown"`, and NOTHING else
/// — in particular never `null`, which an `Option<bool>` would have admitted.
#[test]
fn tree_equivalence_is_a_three_state_union() {
    for (value, expected) in [
        ("true", TreeEquivalence::Yes),
        ("false", TreeEquivalence::No),
        ("\"unknown\"", TreeEquivalence::Unknown),
    ] {
        let parsed: TreeEquivalence = serde_json::from_str(value).expect(value);
        assert_eq!(parsed, expected);
        assert_eq!(serde_json::to_string(&parsed).unwrap(), value);
    }
    assert!(serde_json::from_str::<TreeEquivalence>("null").is_err());
    assert!(serde_json::from_str::<TreeEquivalence>("\"yes\"").is_err());
}

/// An unknown key in a record cyrup itself wrote is corruption, and the one deliberate
/// strictness divergence from pi (see the module doc's `[CYRUP-DELTA]`).
#[test]
fn unknown_fields_are_refused() {
    let json = serde_json::to_string(&manifest()).unwrap();
    let corrupted = json.replace(r#""version":1"#, r#""version":1,"surprise":true"#);
    assert!(serde_json::from_str::<Manifest>(&corrupted).is_err());
}

/// A manifest version this build does not read must fail to parse, not round-trip.
#[test]
fn only_version_one_parses() {
    let json = serde_json::to_string(&manifest())
        .unwrap()
        .replace(r#""version":1"#, r#""version":2"#);
    assert!(serde_json::from_str::<Manifest>(&json).is_err());
}

// =================================================================================================
// Newtypes
// =================================================================================================

#[test]
fn newtypes_enforce_pis_bounds() {
    assert!(CommitSha::parse("abc").is_err());
    assert!(CommitSha::parse(&"g".repeat(40)).is_err());
    assert_eq!(
        CommitSha::parse(&"A".repeat(40)).unwrap().as_str(),
        "a".repeat(40),
        "pi lowercases before the pattern test"
    );
    assert!(ManifestDigest::parse(&"a".repeat(63)).is_err());
    assert!(ManifestDigest::parse(&"a".repeat(64)).is_ok());
    assert!(AttestationActor::parse("  ").is_err());
    assert!(AttestationActor::parse("a\nb").is_err());
    assert!(AttestationActor::parse(&"x".repeat(129)).is_err());
    assert!(PrNumber::parse(0).is_err());
    assert!(PrNumber::parse(-1).is_err());
    assert_eq!(PrNumber::parse(42).unwrap().get(), 42);
    // `BoundedReason` TRUNCATES rather than rejecting (pi `.slice(0, 256)`).
    assert_eq!(
        BoundedReason::parse(&"r".repeat(300))
            .unwrap()
            .as_str()
            .len(),
        256
    );
}

/// [CYRUP-DELTA] verification: RFC 3339 only. `Date.parse("Dec 2 1999")` is finite in pi and is
/// refused here, deliberately.
#[test]
fn attestation_timestamps_are_rfc3339_only() {
    for good in [
        "2026-09-18T00:00:00Z",
        "2026-09-18T00:00:00.123Z",
        "2026-09-18T00:00:00+02:00",
        "2026-09-18T23:59:60-07:00",
    ] {
        assert!(AttestationTimestamp::parse(good).is_ok(), "{good}");
    }
    for bad in [
        "Dec 2 1999",
        "2026-09-18",
        "2026-13-01T00:00:00Z",
        "2026-09-18T24:00:00Z",
        "not a date at all",
        "2026-09-18T00:00:00",
    ] {
        assert!(AttestationTimestamp::parse(bad).is_err(), "{bad}");
    }
}

/// A newtype deserializes THROUGH its own parser, so a hand-edited file cannot smuggle a value
/// the constructor would have refused.
#[test]
fn deserialization_goes_through_parse() {
    assert!(serde_json::from_str::<CommitSha>("\"nope\"").is_err());
    assert!(serde_json::from_str::<PrNumber>("0").is_err());
    assert!(serde_json::from_str::<AttestationTimestamp>("\"whenever\"").is_err());
}

// =================================================================================================
// The safety core
// =================================================================================================

/// pi `:295-297`. `detached`, `pending` and `running` are NOT terminal.
#[test]
fn terminal_child_statuses_match_upstream() {
    for status in [
        ChildStatus::Completed,
        ChildStatus::Complete,
        ChildStatus::Failed,
        ChildStatus::Paused,
        ChildStatus::Stopped,
        ChildStatus::Rejected,
    ] {
        assert!(status.is_terminal(), "{status:?}");
    }
    for status in [
        ChildStatus::Pending,
        ChildStatus::Running,
        ChildStatus::Detached,
    ] {
        assert!(!status.is_terminal(), "{status:?}");
    }
}

/// pi `:330` — a group with cleanup tasks and ZERO children is ACTIVE. An allocated-but-unsettled
/// worktree blocks removal.
#[test]
fn a_group_with_no_children_but_tasks_is_active() {
    let mut empty = manifest();
    empty.groups[0].children.clear();
    assert!(has_active_children(&empty));
    assert_eq!(
        cleanup_eligibility_for_evidence(&empty),
        CleanupEligibility::Active
    );

    let mut detached = manifest();
    detached.groups[0].children[0].status = ChildStatus::Detached;
    assert!(has_active_children(&detached), "detached is not terminal");
}

/// **The anti-forgery rule** (pi `:313-324`). Hand-editing `cleanupEligibility` to
/// `terminal-eligible` in a manifest carrying NO evidence yields `unknown`, which
/// [`format_stored_cleanup`] renders as "removal is not safe".
#[test]
fn a_forged_eligibility_collapses_to_unknown() {
    let mut forged = manifest();
    forged.cleanup_eligibility = Some(StoredCleanupEligibility::new(
        CleanupEligibility::TerminalEligible,
    ));
    assert_eq!(
        trusted_cleanup_eligibility(&forged),
        CleanupEligibility::Unknown,
        "a stored verdict is re-derived, never believed"
    );
    let rendered = format_stored_cleanup(Path::new("/tmp/handoff.json"), Some(&forged));
    assert!(
        rendered.contains("Cleanup eligibility: unknown"),
        "{rendered}"
    );
    assert!(rendered.contains("removal is not safe"), "{rendered}");
}

/// The one permitted disagreement: a stored eligible verdict DOWNGRADED to a stale-evidence
/// blocker is honoured (pi's `/ evidence is stale$/` regex at `:320`, here
/// [`BlockReason::is_stale`]), because it is a fact about the evidence rather than a different
/// opinion about the verdict.
#[test]
fn a_stale_downgrade_is_honoured() {
    let mut stale = manifest();
    stale.merge = Some(MergeEvidence {
        pr_number: PrNumber::parse(7).unwrap(),
        reviewed_head: sha('a'),
        merge_commit: sha('b'),
        tree_equivalent: TreeEquivalence::Yes,
        post_merge_checks: PostMergeChecks::Recorded,
        attested_by: AttestationActor::parse("reviewer").unwrap(),
        attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
        // A digest for a DIFFERENT manifest — i.e. the groups changed after attestation.
        manifest_digest: Some(ManifestDigest::parse(&"f".repeat(64)).unwrap()),
    });
    stale.cleanup_eligibility = Some(StoredCleanupEligibility::new(
        CleanupEligibility::TerminalEligible,
    ));
    assert_eq!(
        trusted_cleanup_eligibility(&stale),
        CleanupEligibility::TerminalBlocked {
            reason: BlockReason::StoredMergeEvidenceStale
        }
    );
}

/// pi `:359-385`, arm by arm.
#[test]
fn eligibility_derivation_matches_upstream() {
    let base = manifest();
    assert_eq!(
        cleanup_eligibility_for_evidence(&base),
        CleanupEligibility::TerminalBlocked {
            reason: BlockReason::NoEvidenceRecorded
        }
    );

    let fresh = digest_of(&base);
    let merge = |tree, checks| MergeEvidence {
        pr_number: PrNumber::parse(7).unwrap(),
        reviewed_head: sha('a'),
        merge_commit: sha('b'),
        tree_equivalent: tree,
        post_merge_checks: checks,
        attested_by: AttestationActor::parse("reviewer").unwrap(),
        attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
        manifest_digest: Some(fresh.clone()),
    };

    let mut unattested = base.clone();
    unattested.merge = Some(merge(TreeEquivalence::Unknown, PostMergeChecks::Recorded));
    assert_eq!(
        cleanup_eligibility_for_evidence(&unattested),
        CleanupEligibility::TerminalBlocked {
            reason: BlockReason::TreeEquivalenceNotAttested
        }
    );

    let mut diverged = base.clone();
    diverged.merge = Some(merge(TreeEquivalence::No, PostMergeChecks::Recorded));
    assert_eq!(
        cleanup_eligibility_for_evidence(&diverged),
        CleanupEligibility::TerminalBlocked {
            reason: BlockReason::TreeNotEquivalent
        }
    );

    let mut unchecked = base.clone();
    unchecked.merge = Some(merge(TreeEquivalence::Yes, PostMergeChecks::Unknown));
    assert_eq!(
        cleanup_eligibility_for_evidence(&unchecked),
        CleanupEligibility::TerminalBlocked {
            reason: BlockReason::PostMergeChecksNotRecorded
        }
    );

    let mut eligible = base.clone();
    eligible.merge = Some(merge(TreeEquivalence::Yes, PostMergeChecks::Recorded));
    assert_eq!(
        cleanup_eligibility_for_evidence(&eligible),
        CleanupEligibility::TerminalEligible
    );

    // Supersession is checked BEFORE merge (pi `:369` before `:378`).
    let mut superseded = eligible.clone();
    superseded.supersession = Some(SupersessionEvidence {
        superseded_by: lane("run-2"),
        attested_by: AttestationActor::parse("reviewer").unwrap(),
        attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
        manifest_digest: Some(fresh.clone()),
    });
    assert_eq!(
        cleanup_eligibility_for_evidence(&superseded),
        CleanupEligibility::SupersededEligible
    );
}

/// The digest covers `{version, runId, mode, source, cwd, groups}` ONLY — so recording evidence
/// does NOT invalidate it, but a later group write does.
#[test]
fn the_digest_covers_only_manifest_facts() {
    let base = manifest();
    let mut with_evidence = base.clone();
    with_evidence.updated_at += 1_000;
    with_evidence.cleanup_eligibility =
        Some(StoredCleanupEligibility::new(CleanupEligibility::Unknown));
    with_evidence.supersession = Some(SupersessionEvidence {
        superseded_by: lane("run-2"),
        attested_by: AttestationActor::parse("r").unwrap(),
        attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
        manifest_digest: None,
    });
    assert_eq!(digest_of(&base), digest_of(&with_evidence));

    let mut with_new_group = base.clone();
    with_new_group.groups[0]
        .children
        .push(child(1, ChildStatus::Completed));
    with_new_group.groups[0].cleanup.tasks.push(cleanup_task(1));
    assert_ne!(digest_of(&base), digest_of(&with_new_group));
}

// =================================================================================================
// Validation the types cannot make
// =================================================================================================

#[tokio::test]
async fn cross_record_validation_matches_upstream() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");

    let mut duplicate_index = manifest();
    duplicate_index.groups[0]
        .children
        .push(child(0, ChildStatus::Completed));
    duplicate_index.groups[0].children[1].task_index = 1;
    duplicate_index.groups[0]
        .cleanup
        .tasks
        .push(cleanup_task(1));
    tokio::fs::write(&path, serde_json::to_vec(&duplicate_index).unwrap())
        .await
        .unwrap();
    let err = read_manifest(&path).await.expect_err("duplicate index");
    assert!(
        err.to_string().contains("duplicate child index 0."),
        "{err}"
    );

    let mut orphan_child = manifest();
    orphan_child.groups[0].children[0].task_index = 9;
    tokio::fs::write(&path, serde_json::to_vec(&orphan_child).unwrap())
        .await
        .unwrap();
    let err = read_manifest(&path).await.expect_err("orphan child");
    assert!(
        err.to_string()
            .contains("children[0] has no cleanup task for taskIndex 9."),
        "{err}"
    );

    // An absent file is `Ok(None)`, not an error (pi `:123`).
    assert!(
        read_manifest(&dir.path().join("nope.json"))
            .await
            .unwrap()
            .is_none()
    );
}

// =================================================================================================
// The writer
// =================================================================================================

fn write_input<'a>(
    path: &'a Path,
    run: &'a LaneId,
    setup: &'a WorktreeSetup,
    results: &'a [HandoffResult],
    cleanup: Option<&'a WorktreeCleanupReport>,
) -> WriteGroup<'a> {
    WriteGroup {
        manifest_path: path,
        run_id: run,
        mode: HandoffMode::Parallel,
        source: HandoffSource::Async,
        cwd: Path::new("/repo"),
        step_index: 0,
        flat_start_index: 0,
        setup,
        diffs: &[],
        cleanup,
        results,
        lane_bindings: None,
        now: 1_758_153_600_000,
    }
}

/// A FRESH manifest has no `cleanupEligibility` key at all (pi `:602-608`), every task is
/// `preserved: true` with pi's pending reason, and a child with no captured diff still gets a
/// patch row carrying the reason.
#[tokio::test]
async fn a_fresh_group_is_written_preserved_and_without_an_eligibility_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    let run = lane("run-1");
    let setup = setup();
    let results = vec![HandoffResult {
        agent: "worker".to_string(),
        status: Some(ChildStatus::Completed),
        summary: "done".to_string(),
        ..HandoffResult::default()
    }];

    let reference = write_group(write_input(&path, &run, &setup, &results, None))
        .await
        .expect("write");
    assert_eq!(reference.child_count, 1);
    assert_eq!(reference.changed_patches, 0);
    assert_eq!(reference.cleanup_state, CleanupState::Partial);
    assert!(reference.cleanup_eligibility.is_none());

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(!raw.contains("cleanupEligibility"), "{raw}");
    assert!(raw.contains(CLEANUP_PENDING_REASON), "{raw}");
    assert!(
        raw.contains("diff artifact unavailable; no patch was captured"),
        "a task with no captured diff still records why: {raw}"
    );

    let stored = read_manifest(&path).await.unwrap().expect("present");
    assert_eq!(stored.groups.len(), 1);
    assert_eq!(stored.groups[0].cleanup.tasks[0].preserved, Some(true));
}

/// The deliberate DOUBLE write around cleanup: the second call merges by `stepIndex` (pi
/// `:587-589`), replacing the group rather than appending, and `createdAt` survives.
#[tokio::test]
async fn the_second_write_replaces_the_group_by_step_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    let run = lane("run-1");
    let setup = setup();
    let results = vec![HandoffResult {
        agent: "worker".to_string(),
        status: Some(ChildStatus::Completed),
        summary: "done".to_string(),
        ..HandoffResult::default()
    }];

    write_group(write_input(&path, &run, &setup, &results, None))
        .await
        .expect("phase 1");
    let report = WorktreeCleanupReport {
        state: CleanupState::Complete,
        tasks: vec![WorktreeCleanupTask {
            index: 0,
            path: PathBuf::from("/tmp/wt-0"),
            branch: "cyrup-parallel-run-0".to_string(),
            provider: Some(crate::workflows::ManagedWorktreeProvider::Native),
            naming: None,
            worktree_removed: true,
            branch_removed: true,
            preserved: None,
            reason: None,
            errors: None,
        }],
        pruned: true,
        errors: None,
    };
    let mut second = write_input(&path, &run, &setup, &results, Some(&report));
    second.now = 1_758_153_611_000;
    let reference = write_group(second).await.expect("phase 2");

    assert_eq!(reference.group_count, 1, "merged, not appended");
    assert_eq!(reference.cleanup_state, CleanupState::Complete);
    let stored = read_manifest(&path).await.unwrap().expect("present");
    assert_eq!(stored.created_at, 1_758_153_600_000, "createdAt survives");
    assert_eq!(stored.updated_at, 1_758_153_611_000);
    assert_eq!(stored.groups[0].cleanup.state, CleanupState::Complete);
}

/// Identity is `runId` AND `mode` AND `source` (pi `:514-516`).
#[tokio::test]
async fn a_manifest_for_a_different_run_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    let setup = setup();
    let results = vec![HandoffResult {
        agent: "worker".to_string(),
        status: Some(ChildStatus::Completed),
        summary: "done".to_string(),
        ..HandoffResult::default()
    }];
    let first = lane("run-1");
    write_group(write_input(&path, &first, &setup, &results, None))
        .await
        .expect("write");

    let other = lane("run-2");
    let err = write_group(write_input(&path, &other, &setup, &results, None))
        .await
        .expect_err("foreign run");
    assert!(matches!(err, HandoffError::ForeignManifest(_)), "{err}");

    let mut wrong_source = write_input(&path, &first, &setup, &results, None);
    wrong_source.source = HandoffSource::Foreground;
    assert!(matches!(
        write_group(wrong_source).await.expect_err("foreign source"),
        HandoffError::ForeignManifest(_)
    ));
}

// =================================================================================================
// Evidence
// =================================================================================================

async fn settled_manifest(path: &Path) -> LaneId {
    let run = lane("run-1");
    let setup = setup();
    let results = vec![HandoffResult {
        agent: "worker".to_string(),
        status: Some(ChildStatus::Completed),
        summary: "done".to_string(),
        ..HandoffResult::default()
    }];
    write_group(write_input(path, &run, &setup, &results, None))
        .await
        .expect("write");
    run
}

fn merge_evidence() -> MergeEvidence {
    MergeEvidence {
        pr_number: PrNumber::parse(42).unwrap(),
        reviewed_head: sha('a'),
        merge_commit: sha('b'),
        tree_equivalent: TreeEquivalence::Yes,
        post_merge_checks: PostMergeChecks::Recorded,
        attested_by: AttestationActor::parse("reviewer").unwrap(),
        attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
        // A caller-supplied digest is DISCARDED; this value must never reach disk.
        manifest_digest: Some(ManifestDigest::parse(&"0".repeat(64)).unwrap()),
    }
}

/// The recorder stamps the digest itself (pi `:442`), a duplicate identical record is idempotent,
/// a different reviewed head is stale (`:445`), and the same head with different details is a
/// conflict (`:446`).
#[tokio::test]
async fn merge_evidence_is_recorder_stamped_and_append_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    let run = settled_manifest(&path).await;

    let outcome = record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &run,
        merge: merge_evidence(),
        now: 1,
    })
    .await
    .expect("record");
    let stored_digest = outcome
        .manifest
        .merge
        .as_ref()
        .unwrap()
        .manifest_digest
        .clone()
        .expect("stamped");
    assert_ne!(
        stored_digest.as_str(),
        "0".repeat(64),
        "a caller-supplied manifestDigest is overwritten, never trusted"
    );
    assert_eq!(
        outcome.reference.cleanup_eligibility,
        Some(CleanupEligibility::TerminalEligible)
    );
    assert!(
        outcome
            .text
            .contains("Cleanup eligibility: terminal-eligible")
    );

    // Idempotent re-record of IDENTICAL evidence.
    record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &run,
        merge: merge_evidence(),
        now: 2,
    })
    .await
    .expect("idempotent");

    let mut different_head = merge_evidence();
    different_head.reviewed_head = sha('c');
    let err = record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &run,
        merge: different_head,
        now: 3,
    })
    .await
    .expect_err("stale");
    assert!(
        matches!(err, HandoffError::StaleReviewedHead { .. }),
        "{err}"
    );

    let mut different_details = merge_evidence();
    different_details.pr_number = PrNumber::parse(99).unwrap();
    let err = record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &run,
        merge: different_details,
        now: 4,
    })
    .await
    .expect_err("conflict");
    assert!(matches!(err, HandoffError::ConflictingMerge), "{err}");
}

/// Merge evidence DELETES any stored supersession (pi `:449`) — merge supersedes supersession,
/// not the reverse — and a lane cannot supersede itself (`:462`).
#[tokio::test]
async fn supersession_rules_match_upstream() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    let run = settled_manifest(&path).await;

    let err = record_supersession(RecordSupersession {
        manifest_path: &path,
        lane_id: &run,
        supersession: SupersessionEvidence {
            superseded_by: run.clone(),
            attested_by: AttestationActor::parse("reviewer").unwrap(),
            attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
            manifest_digest: None,
        },
        now: 1,
    })
    .await
    .expect_err("self supersession");
    assert!(matches!(err, HandoffError::SelfSupersession), "{err}");

    let outcome = record_supersession(RecordSupersession {
        manifest_path: &path,
        lane_id: &run,
        supersession: SupersessionEvidence {
            superseded_by: lane("run-2"),
            attested_by: AttestationActor::parse("reviewer").unwrap(),
            attested_at: AttestationTimestamp::parse("2026-09-18T00:00:00Z").unwrap(),
            manifest_digest: None,
        },
        now: 2,
    })
    .await
    .expect("record");
    assert_eq!(
        outcome.reference.cleanup_eligibility,
        Some(CleanupEligibility::SupersededEligible)
    );

    let after_merge = record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &run,
        merge: merge_evidence(),
        now: 3,
    })
    .await
    .expect("merge");
    assert!(
        after_merge.manifest.supersession.is_none(),
        "recording a merge deletes stored supersession"
    );
}

/// Recording evidence while ANY child is non-terminal is refused (pi `:441`, `:460`). "Active"
/// means not-terminal, NOT "running": a `detached` child blocks too.
#[tokio::test]
async fn active_children_block_attestation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    let run = lane("run-1");
    let setup = setup();
    let results = vec![HandoffResult {
        agent: "worker".to_string(),
        status: Some(ChildStatus::Detached),
        summary: "gone".to_string(),
        ..HandoffResult::default()
    }];
    write_group(write_input(&path, &run, &setup, &results, None))
        .await
        .expect("write");

    let err = record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &run,
        merge: merge_evidence(),
        now: 1,
    })
    .await
    .expect_err("active");
    assert!(
        matches!(err, HandoffError::ActiveChildOwner { what } if what == "merge evidence"),
        "{err}"
    );
}

/// A lane id that is not the manifest's own run is refused before anything is written
/// (pi `:437`).
#[tokio::test]
async fn attestation_requires_the_owning_lane() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("handoff.json");
    settled_manifest(&path).await;
    let other = lane("run-9");
    let err = record_merge(RecordMerge {
        manifest_path: &path,
        lane_id: &other,
        merge: merge_evidence(),
        now: 1,
    })
    .await
    .expect_err("wrong lane");
    assert!(matches!(err, HandoffError::LaneRunMismatch { .. }), "{err}");
}

/// pi `parallelHandoffPath(baseDir, runId)` (`:615`) and [`crate::background::RunDir::handoff`]
/// are the two shapes, and the retention reader's path must equal the writer's.
#[test]
fn the_two_manifest_paths_are_upstreams() {
    let run = crate::background::RunId::from_token("abc");
    assert_eq!(
        handoff_manifest_path(Path::new("/artifacts"), &run),
        PathBuf::from("/artifacts/handoffs/abc.json")
    );
    assert_eq!(
        crate::background::RunDir::for_existing(Path::new("/runs/abc")).handoff(),
        PathBuf::from("/runs/abc/handoff.json")
    );
}
