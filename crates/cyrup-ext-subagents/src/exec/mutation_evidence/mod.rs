//! Tracked-mutation evidence + timeout recovery — the port of pi
//! `runs/shared/mutation-evidence.ts` (199 LOC).
//!
//! # Why a timed-out child needs evidence
//!
//! A child killed by its deadline (or torn down by a stop) may have already mutated the worktree:
//! half an edit, a written file, a partially-applied refactor. The parent that killed it sees only
//! "timed out" — it cannot tell a child that died reading from a child that died mid-write, and
//! resuming (or launching a dependent stage) over a dirty worktree silently builds on unreviewed
//! partial work. This module is the bounded answer:
//!
//! 1. **snapshot** — before the first child spawns, record which tracked files are already dirty
//!    and a per-file fingerprint of *how* ([`snapshot_tracked_mutations`]);
//! 2. **collect** — after the run settles, diff the worktree against that snapshot and name the
//!    tracked files the child (and only the child) changed
//!    ([`collect_tracked_mutation_evidence`]);
//! 3. **summarize** — for a deadline kill, fold the evidence plus the run's live context (active
//!    tool, requested report, artifact paths) into one bounded, operator-facing
//!    [`TimeoutRecoverySummary`] whose `message` is spliced into the delivered output
//!    ([`build_timeout_recovery_summary`]);
//! 4. **project** — narrow that summary to the parent-facing [`TimeoutRecoveryProjection`] before
//!    it crosses into a tool result, so session/transcript/artifact paths never leak into a wait
//!    or job view ([`project_timeout_recovery`], [`format_timeout_recovery_lines`]).
//!
//! Only **tracked** files count: an untracked build artefact is not evidence of a mutation the
//! parent needs to review, and the record says so about itself
//! ([`TrackedMutationSnapshot::tracked_only`]).
//!
//! Where upstream shells out to `git` (`gitOutput`, `mutation-evidence.ts:17-19`), this module
//! uses `gix` in-process — see [`repo`]'s own doc for the full rationale (the evidence path runs
//! at a deadline kill, where a subprocess is the one thing that must not be spawned).
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs        facade + this narrative; no logic
//! types.rs      TrackedMutationSnapshot, TrackedMutationFingerprint, TrackedMutationEvidence,
//!               ReportStatus, Termination, RecoveryNeeded, TimeoutRecoverySummary,
//!               TimeoutRecoveryProjection + the three verbatim bounds
//! repo.rs       the gix layer — changed-tracked-files listing + per-path fingerprinting
//! snapshot.rs   snapshot_tracked_mutations (:67-76) / collect_tracked_mutation_evidence (:78-105)
//! summary.rs    build_timeout_recovery_summary (:151-199) + format_path_list (:107-111)
//! project.rs    project_timeout_recovery (:114-135) + format_timeout_recovery_lines (:138-149)
//! ```

mod project;
mod repo;
mod snapshot;
mod summary;
mod types;

pub use project::{
    format_timeout_recovery_lines, format_timeout_recovery_lines_from_json,
    project_timeout_recovery,
};
pub use snapshot::{collect_tracked_mutation_evidence, snapshot_tracked_mutations};
pub use summary::{TimeoutRecoveryInput, build_timeout_recovery_summary};
pub use types::{
    RecoveryNeeded, RecoveryReason, ReportStatus, Termination, TimeoutRecoveryProjection,
    TimeoutRecoverySummary, TrackedMutationEvidence, TrackedMutationFingerprint,
    TrackedMutationSnapshot,
};
