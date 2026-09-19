//! The parallel-handoff manifest — the coordination layer over a worktree fan-out.
//!
//! Port of pi `src/runs/shared/parallel-handoff.ts` @`v0.68.0` (741 lines, read in full). A
//! `subagent({tasks:[…], worktree:true})` fan-out allocates N isolated git worktrees; this module
//! is what records, durably and atomically, **what each lane claimed, what it produced, whether
//! it merged, and whether removing its worktree is safe**. Without it cyrup could allocate
//! worktrees and then never diff, clean up, or record anything — the user was left finding orphan
//! branches by hand.
//!
//! # Where it sits
//!
//! Beside `spawn/` and `background/` rather than inside either, because all three of the
//! foreground executor, the detached background runner and
//! [`crate::background::async_retention`] touch it. Under `spawn/` the retention reader would
//! have to import from `spawn`; under `background/` the foreground `/parallel` path would have to
//! import from `background`.
//!
//! # The on-disk format IS the interface
//!
//! JSON field names stay upstream's (`#[serde(rename_all = "camelCase")]`); Rust identifiers do
//! not. Three readers already depend on the exact spelling — this crate's retention scan (which
//! pins `version: 1`, a non-empty `groups`, and `groups[].cleanup.state`), pi itself, and the
//! cleanup-plan builder. See [`model`] for the record definitions and the one
//! `[CYRUP-DELTA]` on strictness.
//!
//! # The safety rules, and where each one lives
//!
//! | rule | pi | here |
//! |---|---|---|
//! | a worktree holding uncommitted work is never removed | `worktree.ts:1197-1231` | [`crate::spawn::worktree::cleanup_worktrees`]'s preserve gate: both probes run, and removal proceeds only behind a captured patch that is error-free, non-empty, RECORDED IN THIS MANIFEST ([`handoff_records_patch`]) and still representative |
//! | the manifest is written TWICE around cleanup | `:4425`/`:4427` | [`crate::spawn::chain_graph`]'s `run_parallel_group`; never collapse them |
//! | a stored `cleanupEligibility` is never trusted | `:313-324` | [`trusted_cleanup_eligibility`] |
//! | active children block everything | `:326-333` | [`has_active_children`] |
//! | evidence is digest-bound and recorder-stamped | `:442`/`:461` | [`record_merge`], [`record_supersession`] |
//! | a resumed child's cwd may not escape its worktree | `:172-188` | [`resolve_retained_worktree_cwd`], consulted by `revive_from_transcript` (`extension/executor/control.rs`) AHEAD of the run's own `status.cwd` |
//! | `groups` is never written empty | — | [`write_group`]; an empty array is unresolved-FOREVER to the retention reader |
//!
//! # What is NOT redeclared here
//!
//! * lane metadata — [`crate::workflows::WorkflowLaneMetadata`] is already the complete port of
//!   pi `lane-metadata.ts:49-74`, already `camelCase` + `deny_unknown_fields`, already keyed on
//!   [`crate::workflows::WorkflowKey`];
//! * the canonical-JSON digest — [`crate::workflows::stable_json_digest`];
//! * the path-escape predicate — [`crate::spawn::worktree`]'s `lexical_normalize` /
//!   `normalize_comparable_cwd`. A second one in this crate would be a security bug waiting.

pub mod error;
pub mod evidence;
pub mod format;
pub mod model;
pub mod path;
pub mod read;
pub mod write;

pub use error::{HandoffError, ManifestDefect};
pub use evidence::{
    cleanup_eligibility_for_evidence, has_active_children, has_managed_worktree_tasks,
    has_valid_stored_lane_shape, manifest_facts_digest, reference_for, trusted_cleanup_eligibility,
    validate_manifest_for_lane_evidence,
};
pub use format::{format_error, format_reference, format_stored_cleanup};
pub use model::{
    AttestationActor, AttestationTimestamp, BlockReason, BoundedReason, Child, ChildStatus,
    CleanupEligibility, CleanupState, CommitSha, DiscardAuthorization, DiscardAuthorizationKind,
    Group, HandoffMode, HandoffReference, HandoffSource, LaneBinding, LaneId, Manifest,
    ManifestDigest, ManifestVersion, MergeEvidence, Patch, PostMergeChecks, PrNumber,
    PreserveEvidence, StoredCleanupEligibility, SupersessionEvidence, TreeEquivalence,
    WorktreeCleanupIntent, WorktreeCleanupReport, WorktreeCleanupTask,
};
pub use path::handoff_manifest_path;
pub(crate) use read::handoff_records_patch;
pub use read::{read_manifest, resolve_retained_worktree_cwd, validate_manifest_identity};
pub use write::{
    CLEANUP_PENDING_REASON, DiscardOutcome, EvidenceOutcome, HandoffResult, RecordMerge,
    RecordSupersession, WriteGroup, discard_preserved, record_merge, record_supersession,
    write_group,
};

#[cfg(test)]
mod tests;
