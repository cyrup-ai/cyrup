//! [`HandoffError`] — one variant per `throw` in pi `runs/shared/parallel-handoff.ts` @v0.68.0.
//!
//! Upstream signals every failure with `throw new Error(<string>)`. The strings themselves are
//! the model-facing prose an `isError: true` tool result renders, so each `#[error(...)]` below
//! reproduces pi's wording verbatim — the crate's established convention, recorded on
//! [`crate::error::SubagentError::Management`]. What changes is the SHAPE: a closed enum instead
//! of a string, so a caller can branch on `NotFound` vs `RunMismatch` without matching prose.

use std::path::PathBuf;

use super::model::CommitSha;

/// Every way reading, validating, writing or attesting a parallel-handoff manifest can fail.
#[derive(thiserror::Error, Debug)]
pub enum HandoffError {
    /// pi `boundedString:213` — `${label} must be a non-empty string.`
    #[error("{label} must be a non-empty string.")]
    Empty {
        /// The field label pi interpolates (e.g. `merge.attestedBy`).
        label: &'static str,
    },

    /// pi `boundedString:216` — `${label} must be at most ${maxBytes} bytes.`
    #[error("{label} must be at most {max} bytes.")]
    TooLong {
        /// The field label.
        label: &'static str,
        /// The byte ceiling.
        max: usize,
    },

    /// pi `boundedString:215` — `${label} must not contain newlines.`
    #[error("{label} must not contain newlines.")]
    Newline {
        /// The field label.
        label: &'static str,
    },

    /// pi `commitString:222` — `${label} must be a full 40-character commit SHA.`
    #[error("{label} must be a full 40-character commit SHA.")]
    NotACommit {
        /// The field label.
        label: &'static str,
    },

    /// pi `manifestDigestString:228` — `${label} must be a full 64-character SHA-256 digest.`
    #[error("{label} must be a full 64-character SHA-256 digest.")]
    NotADigest {
        /// The field label.
        label: &'static str,
    },

    /// pi `attestationTimestamp:245` — `attestedAt must be a valid timestamp.`
    #[error("attestedAt must be a valid timestamp.")]
    BadTimestamp,

    /// pi `normalizeMergeEvidence:252` — `merge.prNumber must be a positive integer.`
    #[error("merge.prNumber must be a positive integer.")]
    BadPrNumber,

    /// pi `normalizeLaneBinding:53` — `${label} has unsupported fields: ${unknown.join(", ")}.`
    #[error("{label} has unsupported fields: {fields}.")]
    UnknownFields {
        /// The record label.
        label: String,
        /// Comma-joined field names, in upstream's order.
        fields: String,
    },

    /// pi `validateManifestIdentity:71-111` — the whole family of
    /// `Invalid parallel handoff manifest '<path>': <defect>.` messages.
    #[error("Invalid parallel handoff manifest '{path}': {detail}")]
    InvalidManifest {
        /// The manifest path pi interpolates.
        path: PathBuf,
        /// Which structural rule was broken.
        detail: ManifestDefect,
    },

    /// pi `:71` — the version/groups guard, which carries no per-defect clause.
    #[error("Invalid parallel handoff manifest: {0}")]
    UnreadableManifest(PathBuf),

    /// pi `recordParallelHandoffMerge:439` / `:457` —
    /// `Parallel handoff manifest not found: ${manifestPath}`.
    #[error("Parallel handoff manifest not found: {0}")]
    NotFound(PathBuf),

    /// pi `:136` / `:165` —
    /// `Managed worktree handoff belongs to run '${found}', not '${expected}'.`
    #[error("Managed worktree handoff belongs to run '{found}', not '{expected}'.")]
    RunMismatch {
        /// The run id stored in the manifest.
        found: String,
        /// The run id the caller asked for.
        expected: String,
    },

    /// pi `:141` — `Parallel handoff has multiple children matching workflow identity '<id>'.`
    #[error("Parallel handoff has multiple children matching workflow identity '{0}'.")]
    AmbiguousChild(String),

    /// pi `validateManifestForLaneEvidence:437` —
    /// `Lane '${laneId}' does not match manifest run '${manifest.runId}'.`
    #[error("Lane '{lane}' does not match manifest run '{run}'.")]
    LaneRunMismatch {
        /// The lane id the caller attested under.
        lane: String,
        /// The manifest's own run id.
        run: String,
    },

    /// pi `:438` — `Lane manifest is malformed.`
    #[error("Lane manifest is malformed.")]
    LaneManifestMalformed,

    /// pi `:439` — `Lane manifest has no managed worktree tasks.`
    #[error("Lane manifest has no managed worktree tasks.")]
    NoManagedWorktreeTasks,

    /// pi `:441` / `:460` — `Lane has an active child owner; <what> must be recorded after local
    /// reconciliation.`
    #[error("Lane has an active child owner; {what} must be recorded after local reconciliation.")]
    ActiveChildOwner {
        /// `merge evidence` or `supersession`, matching pi's two call sites verbatim.
        what: &'static str,
    },

    /// pi `:445` — `Lane manifest is stale: reviewed head is already recorded as ${head}.`
    #[error("Lane manifest is stale: reviewed head is already recorded as {existing}.")]
    StaleReviewedHead {
        /// The reviewed head already on file.
        existing: CommitSha,
    },

    /// pi `:446` — `Lane manifest already contains different merge evidence for this reviewed
    /// head.`
    #[error("Lane manifest already contains different merge evidence for this reviewed head.")]
    ConflictingMerge,

    /// pi `:465` — `Lane manifest already contains different supersession evidence.`
    #[error("Lane manifest already contains different supersession evidence.")]
    ConflictingSupersession,

    /// pi `:462` — `supersession.supersededBy must identify a different replacement lane.`
    #[error("supersession.supersededBy must identify a different replacement lane.")]
    SelfSupersession,

    /// pi `writeParallelHandoffGroup:515` —
    /// `Parallel handoff manifest belongs to a different run: ${manifestPath}`
    #[error("Parallel handoff manifest belongs to a different run: {0}")]
    ForeignManifest(PathBuf),

    /// pi `:521` — `Parallel handoff has duplicate lane binding taskIndex ${n}.`
    #[error("Parallel handoff has duplicate lane binding taskIndex {0}.")]
    DuplicateBindingTaskIndex(u32),

    /// pi `:522` — `Parallel handoff lane binding taskIndex ${n} has no managed worktree.`
    #[error("Parallel handoff lane binding taskIndex {0} has no managed worktree.")]
    BindingWithoutWorktree(u32),

    /// pi `:523` —
    /// `Parallel handoff lane binding index ${i} does not match taskIndex ${t}.`
    #[error("Parallel handoff lane binding index {index} does not match taskIndex {task_index}.")]
    BindingIndexMismatch {
        /// The binding's flat index.
        index: u32,
        /// The binding's task index.
        task_index: u32,
    },

    /// pi `:171` — `Async run '${runId}' child ${index} has no managed worktree cleanup record.`
    #[error("Async run '{run_id}' child {child_index} has no managed worktree cleanup record.")]
    NoCleanupRecord {
        /// The run id.
        run_id: String,
        /// The child's flat index.
        child_index: u32,
    },

    /// pi `:174` / `:188` — `Async run '${runId}' has an invalid managed worktree cwd.`
    #[error("Async run '{0}' has an invalid managed worktree cwd.")]
    InvalidManagedCwd(String),

    /// pi `:176` — `Async run '${runId}' required managed worktree was removed: ${cwd}`
    #[error("Async run '{run_id}' required managed worktree was removed: {cwd}")]
    ManagedWorktreeRemoved {
        /// The run id.
        run_id: String,
        /// The cwd that would have been used.
        cwd: PathBuf,
    },

    /// pi `:186` — `Async run '${runId}' required managed worktree cwd is missing: ${cwd}`
    #[error("Async run '{run_id}' required managed worktree cwd is missing: {cwd}")]
    ManagedWorktreeCwdMissing {
        /// The run id.
        run_id: String,
        /// The cwd that could not be resolved.
        cwd: PathBuf,
    },

    /// pi `writeWorktreeSetupHandoff:625` — `Cannot publish worktree allocation evidence without
    /// repository and base commit.`
    #[error("Cannot publish worktree allocation evidence without repository and base commit.")]
    SetupEvidenceIncomplete,

    /// An underlying filesystem failure (read, atomic write, `realpath`).
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// The manifest bytes were not the JSON this build reads. `Display` is the serde message
    /// alone, matching pi's bare `JSON.parse` throw reaching the caller unprefixed.
    #[error("{0}")]
    Json(#[source] serde_json::Error),
}

/// The per-defect clause of [`HandoffError::InvalidManifest`] — pi's structural checks at
/// `validateManifestIdentity:74-111`, each carrying the data pi interpolates.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ManifestDefect {
    /// pi `:74` — `runId must be a non-empty string.`
    #[error("runId must be a non-empty string.")]
    EmptyRunId,
    /// pi `:80` — `groups[${i}] is malformed.`
    #[error("groups[{0}] is malformed.")]
    MalformedGroup(usize),
    /// pi `:87` — `groups[${g}] has duplicate lane binding taskIndex ${t}.`
    #[error("groups[{group}] has duplicate lane binding taskIndex {task_index}.")]
    DuplicateBindingTaskIndex {
        /// The group's position.
        group: usize,
        /// The repeated task index.
        task_index: u32,
    },
    /// pi `:88` — `groups[${g}].laneBindings[${b}] has no cleanup task for taskIndex ${t}.`
    #[error(
        "groups[{group}].laneBindings[{binding}] has no cleanup task for taskIndex {task_index}."
    )]
    BindingWithoutCleanupTask {
        /// The group's position.
        group: usize,
        /// The binding's position.
        binding: usize,
        /// The task index with no cleanup row.
        task_index: u32,
    },
    /// pi `:99` — `groups[${g}] has duplicate child taskIndex ${t}.`
    #[error("groups[{group}] has duplicate child taskIndex {task_index}.")]
    DuplicateChildTaskIndex {
        /// The group's position.
        group: usize,
        /// The repeated task index.
        task_index: u32,
    },
    /// pi `:101` — `duplicate child index ${n}.` (manifest-wide, not per group).
    #[error("duplicate child index {0}.")]
    DuplicateChildIndex(u32),
    /// pi `:103` — `groups[${g}].children[${c}] has no cleanup task for taskIndex ${t}.`
    #[error("groups[{group}].children[{child}] has no cleanup task for taskIndex {task_index}.")]
    ChildWithoutCleanupTask {
        /// The group's position.
        group: usize,
        /// The child's position.
        child: usize,
        /// The task index with no cleanup row.
        task_index: u32,
    },
    /// pi `:108` — `duplicate workflow key '${k}'.`
    #[error("duplicate workflow key '{0}'.")]
    DuplicateWorkflowKey(String),
    /// pi `:110` — `duplicate child run id '${id}'.`
    #[error("duplicate child run id '{0}'.")]
    DuplicateChildRunId(String),
    /// LANES_2 — pi `normalizeWorktreeNaming`'s control-character rule
    /// (`lane-metadata.ts:83-96`, via `boundedNonEmptyString:23`) applied to a cleanup task's
    /// retained naming evidence. Carries [`crate::workflows::LaneMetadataError`]'s own sentence,
    /// which already names the offending field.
    #[error("{0}")]
    InvalidCleanupNaming(String),
    /// pi `assertWorkflowLaneKey` (`lane-metadata.ts:71-74`), reached from `:94`/`:106`:
    /// a child/binding whose `lane.key` disagrees with its `workflowKey`.
    #[error("{label} key '{lane_key}' must match workflowKey '{workflow_key}'.")]
    LaneKeyMismatch {
        /// The record label pi interpolates.
        label: String,
        /// The lane's own key.
        lane_key: String,
        /// The sibling `workflowKey`.
        workflow_key: String,
    },
}
