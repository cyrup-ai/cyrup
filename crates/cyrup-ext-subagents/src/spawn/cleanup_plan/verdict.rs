//! The classifier's answer, as ONE value — pi's `(state, decision)` pair collapsed.
//!
//! Upstream pairs `state` and `decision` by hand at fourteen `blockedEntry(...)` call sites
//! (`worktree-cleanup-plan.ts:596-724` @`v0.68.0`). All forty-one arms were enumerated and the
//! mapping is a **total function**: `safe -> remove`; `ineligible | dirty | active -> keep`;
//! `stale | unknown -> unknown`. Nothing else occurs.
//!
//! So in Rust it is one value, not two fields. Carrying two independent fields would let a future
//! arm spell `state: "dirty", decision: "remove"` — the single worst bug this module can have.
//! Here [`Verdict::decision`] is an exhaustive `match`, so adding a variant without deciding what
//! it means is a compile error.
//!
//! The reason enums carry their interpolated data rather than a pre-formatted `String`, and their
//! `Display` impls render pi's verbatim sentences: those strings are the model-facing and
//! operator-facing explanation, so they are pinned, but the *data* stays structured.

use std::path::PathBuf;

use super::model::{CleanupDecision, CleanupState};

/// Why a worktree is structurally not a cleanup candidate. Always [`CleanupDecision::Keep`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IneligibleReason {
    /// pi `:627`.
    InsideExtensionsDir {
        /// `<agent_dir>/extensions`.
        extensions_dir: PathBuf,
    },
    /// pi `:628` — not a strict child of the managed base directory.
    OutsideBaseDir {
        /// The base directory containment was tested against.
        base_dir: PathBuf,
    },
    /// The worktree's leaf name is not one this build creates.
    ///
    /// [CYRUP-DELTA], behavioural, and it only ever NARROWS the removable set. pi's containment
    /// check is "strict child of the base dir" and that is sufficient upstream, because pi's base
    /// dir is `<dirname(repoRoot)>/worktrees/<basename(repoRoot)>` — a directory pi created for
    /// exactly this purpose (`worktree.ts:648-651`, `buildNativeWorktreePath:684-686`). cyrup's
    /// base dir ladder is `configured -> $CYRUP_SUBAGENTS_WORKTREE_DIR -> std::env::temp_dir()`
    /// (`spawn/worktree.rs`'s `resolve_worktree_base_dir_path`) with no `worktrees/` rung and no
    /// per-repo level, so with nothing configured the base IS the shared system temp directory
    /// and "contained in /tmp" is a near-vacuous guarantee. The second conjunct is the leaf
    /// prefix `build_worktree_path` itself writes (`spawn/worktree.rs`,
    /// `cyrup-worktree-<runId>-<index>`), which restores the property pi's layout gives for free:
    /// *this directory is one this build made*.
    NotAManagedWorktreeName {
        /// The prefix every cyrup-created worktree leaf carries.
        expected_prefix: &'static str,
    },
    /// pi `:630`.
    IsRepositoryRoot,
    /// pi `:637`.
    RetainedChildResume,
    /// pi `:638` — the reason every freshly-preserved task is written with.
    DurableCaptureStillPending,
    /// pi `:645`.
    BranchCheckedOutAtRepoRoot,
    /// pi `:679` — a stored patch exists but no longer represents the worktree.
    PatchValidationFailed {
        /// The validator's own message.
        detail: String,
    },
    /// pi `:685`.
    UnmergedDivergence,
    /// pi `:694`.
    DurableHandoffPathInsideWorktree {
        /// The evidence file that would be destroyed with the worktree.
        path: PathBuf,
    },
    /// pi `:702`.
    DurableOutputPathInsideWorktree {
        /// The output/patch file that would be destroyed with the worktree.
        path: PathBuf,
    },
}

impl std::fmt::Display for IneligibleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsideExtensionsDir { extensions_dir } => write!(
                f,
                "worktree real path is inside the extensions directory '{}'",
                extensions_dir.display()
            ),
            Self::OutsideBaseDir { base_dir } => write!(
                f,
                "worktree real path is outside configured base directory '{}'",
                base_dir.display()
            ),
            Self::NotAManagedWorktreeName { expected_prefix } => write!(
                f,
                "worktree directory name is not one this build creates (expected a '{expected_prefix}' prefix)"
            ),
            Self::IsRepositoryRoot => f.write_str("worktree is the repository root"),
            Self::RetainedChildResume => {
                f.write_str("retained child resume still requires this worktree cwd")
            }
            Self::DurableCaptureStillPending => {
                f.write_str("durable handoff capture is still pending for this worktree")
            }
            Self::BranchCheckedOutAtRepoRoot => {
                f.write_str("metadata-recorded branch is checked out at the repository root")
            }
            Self::PatchValidationFailed { detail } => {
                write!(f, "captured handoff patch failed validation: {detail}")
            }
            Self::UnmergedDivergence => f.write_str(
                "committed divergence is neither preserved by a handoff patch nor merged into the local target HEAD",
            ),
            Self::DurableHandoffPathInsideWorktree { path } => write!(
                f,
                "durable handoff path is inside the worktree: {}",
                path.display()
            ),
            Self::DurableOutputPathInsideWorktree { path } => write!(
                f,
                "durable report or patch path is inside the worktree: {}",
                path.display()
            ),
        }
    }
}

/// Why the owning run is not finished. Always [`CleanupDecision::Keep`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActiveReason {
    /// pi `:488` — the owning run's `status.json` says queued or running.
    OwningRunState {
        /// `queued` or `running`, as the status file spells it.
        state: &'static str,
    },
    /// pi `:493` — a group with zero children, or any child that is not terminal.
    NonTerminalChild,
    /// pi `:497`.
    ForegroundRunActive,
    /// pi `:508` — an active-run marker younger than 24 h.
    RecentActiveMarker,
}

impl std::fmt::Display for ActiveReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OwningRunState { state } => write!(f, "owning run is {state}"),
            Self::NonTerminalChild => f.write_str("owning handoff still has a non-terminal child"),
            Self::ForegroundRunActive => f.write_str("owning foreground run is still active"),
            Self::RecentActiveMarker => f.write_str("owning run still has a recent active marker"),
        }
    }
}

/// Metadata and reality disagree. Always [`CleanupDecision::Unknown`] — never `Remove`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StaleReason {
    /// pi `:619`.
    GitPathMissing,
    /// pi `:623`.
    WorktreePathMissing,
    /// pi `:633`.
    AlreadyRecordedRemoved,
    /// pi `:509` — the marker aged past `DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS`.
    ActiveMarkerStale {
        /// Marker age, in whole seconds, exactly as pi renders it (`Math.floor(ms / 1000)`).
        age_secs: i64,
    },
    /// pi `:723` — metadata names a worktree git has never heard of.
    NotPresentInGit,
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitPathMissing => f.write_str("Git worktree path is missing from disk"),
            Self::WorktreePathMissing => f.write_str("worktree path is missing from disk"),
            Self::AlreadyRecordedRemoved => {
                f.write_str("handoff metadata already records this worktree as removed")
            }
            Self::ActiveMarkerStale { age_secs } => {
                write!(f, "terminal active marker is stale ({age_secs}s old)")
            }
            Self::NotPresentInGit => f.write_str(
                "handoff metadata records a worktree that is not present in Git worktree state",
            ),
        }
    }
}

/// Something could not be established. Always [`CleanupDecision::Unknown`].
///
/// This is the largest arm on purpose: **absence of proof is never proof of safety**, so every
/// unreadable, ambiguous or unresolvable condition lands here rather than falling through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    /// pi `:613`.
    NoOwningRunId,
    /// pi `:615`.
    NoRepositoryRoot,
    /// pi `:616`.
    MetadataPathMismatch,
    /// pi `:618`.
    GitPathUninspectable {
        /// The OS error.
        detail: String,
    },
    /// pi `:620`.
    GitPathIsSymlink,
    /// pi `:622`.
    WorktreePathUninspectable {
        /// The OS error.
        detail: String,
    },
    /// pi `:624`.
    WorktreePathIsSymlink,
    /// pi `:625`.
    WorktreePathNotADirectory,
    /// pi `:631`.
    DetachedWorktree,
    /// pi `:632`.
    MetadataBranchMismatch {
        /// What the manifest's cleanup task records.
        metadata: String,
        /// What `git worktree list --porcelain` reports.
        git: String,
    },
    /// pi `:634`.
    NotMarkedPreserved,
    /// pi `:635`.
    CleanupStateNotPartial,
    /// pi `:639`.
    RepoRootMismatch,
    /// pi `:642`.
    BranchCheckedOutElsewhere,
    /// pi `:486` — `status.json`'s run id is not the manifest's.
    StatusRunIdMismatch {
        /// What the status file claims.
        status: String,
        /// What the manifest claims.
        manifest: String,
    },
    /// pi `:490` — a run state outside pi's terminal set.
    ///
    /// Reachable because cyrup's [`crate::background::RunState`] is a CLOSED enum where pi's
    /// `AsyncStatus["state"]` is an open string union (pi has `partial` and `rejected`; cyrup has
    /// neither). See [`super::metadata`] for the fallback that keeps this sentence alive.
    UnknownRunState {
        /// The raw state string from the status file.
        state: String,
    },
    /// pi `:485` / `:471-478` — the status file could not be read or parsed.
    StatusUnreadable {
        /// pi's own message for the failure.
        detail: String,
    },
    /// pi `:492`.
    AsyncStatusMissing {
        /// The manifest whose sibling `status.json` is absent.
        manifest_path: PathBuf,
    },
    /// pi `:498` — the foreground probe answered anything but `terminal`.
    ForegroundOwnershipNotProvable,
    /// pi `:655`.
    NoBaseCommit,
    /// pi `:657`.
    BaseCommitUnresolvable {
        /// git's own failure text.
        detail: String,
    },
    /// pi `:661`.
    BranchTipUnresolvable {
        /// git's own failure text.
        detail: String,
    },
    /// pi `:664`.
    BranchTipHeadMismatch,
    /// pi `:667`.
    StatusCheckFailed {
        /// git's own failure text.
        detail: String,
    },
    /// pi `:672`.
    DiffCheckFailed {
        /// git's own failure text.
        detail: String,
    },
    /// pi `:674`.
    AncestryCheckFailed {
        /// git's own failure text.
        detail: String,
    },
    /// pi `:695`/`:696`, plus the un-inspectable arm at `:694`.
    DurableHandoffPathUnusable {
        /// Which evidence file, and what is wrong with it.
        detail: String,
    },
    /// pi `:714` — a git worktree with no (or more than one) matching metadata record.
    NoMatchingMetadata,
}

impl std::fmt::Display for UnknownReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOwningRunId => f.write_str("handoff metadata has no valid owning run id"),
            Self::NoRepositoryRoot => f.write_str("handoff metadata has no repository root"),
            Self::MetadataPathMismatch => {
                f.write_str("metadata worktree path does not identify the Git worktree")
            }
            Self::GitPathUninspectable { detail } => {
                write!(f, "Git worktree path could not be inspected: {detail}")
            }
            Self::GitPathIsSymlink => {
                f.write_str("Git worktree path is a symlink; cleanup requires a real directory")
            }
            Self::WorktreePathUninspectable { detail } => {
                write!(f, "worktree path could not be inspected: {detail}")
            }
            Self::WorktreePathIsSymlink => {
                f.write_str("worktree path is a symlink; cleanup requires a real directory")
            }
            Self::WorktreePathNotADirectory => f.write_str("worktree path is not a directory"),
            Self::DetachedWorktree => {
                f.write_str("detached worktrees have no metadata-recorded branch")
            }
            Self::MetadataBranchMismatch { metadata, git } => write!(
                f,
                "metadata branch '{metadata}' does not match Git branch '{git}'"
            ),
            Self::NotMarkedPreserved => {
                f.write_str("handoff metadata does not mark this worktree as preserved")
            }
            Self::CleanupStateNotPartial => f.write_str(
                "handoff cleanup state does not indicate a preserved worktree pending cleanup",
            ),
            Self::RepoRootMismatch => {
                f.write_str("handoff repository root does not match the requested repository")
            }
            Self::BranchCheckedOutElsewhere => {
                f.write_str("metadata-recorded branch is checked out in another Git worktree")
            }
            Self::StatusRunIdMismatch { status, manifest } => write!(
                f,
                "status run id '{status}' does not match manifest run '{manifest}'"
            ),
            Self::UnknownRunState { state } => {
                write!(f, "owning run has unknown state '{state}'")
            }
            Self::StatusUnreadable { detail } => f.write_str(detail),
            Self::AsyncStatusMissing { manifest_path } => write!(
                f,
                "async status is missing beside {}",
                manifest_path.display()
            ),
            Self::ForegroundOwnershipNotProvable => {
                f.write_str("foreground owning-run state is not provably terminal")
            }
            Self::NoBaseCommit => f.write_str("handoff metadata has no base commit"),
            Self::BaseCommitUnresolvable { detail } => write!(
                f,
                "handoff base commit is not a valid local commit: {detail}"
            ),
            Self::BranchTipUnresolvable { detail } => {
                write!(f, "branch tip could not be resolved: {detail}")
            }
            Self::BranchTipHeadMismatch => f.write_str("branch tip and worktree HEAD do not match"),
            Self::StatusCheckFailed { detail } => write!(f, "git status failed: {detail}"),
            Self::DiffCheckFailed { detail } => {
                write!(f, "git diff safety check failed: {detail}")
            }
            Self::AncestryCheckFailed { detail } => {
                write!(f, "local merge safety check failed: {detail}")
            }
            Self::DurableHandoffPathUnusable { detail } => f.write_str(detail),
            Self::NoMatchingMetadata => {
                f.write_str("no matching extension-owned handoff metadata was found")
            }
        }
    }
}

/// The extra operator-facing line pi appends when a safe entry's branch must survive (`:709`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SafeNote {
    /// pi `:709` — the worktree may go, the branch may not.
    BranchNotAncestorOfTarget,
}

impl std::fmt::Display for SafeNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BranchNotAncestorOfTarget => f.write_str(
                "local branch tip is not an ancestor of target HEAD; future apply must retain the branch",
            ),
        }
    }
}

/// One worktree's verdict: pi's `state` and `decision`, inseparable.
///
/// There is deliberately **no `Default`**: the only path to [`Verdict::Safe`] is falling off the
/// end of every gate in [`super::classify`], so a classifier that forgets to decide cannot
/// accidentally produce a removable entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every gate passed (pi `:705-710`).
    ///
    /// `delete_branch` is pi `willDeleteBranch` (`:707`) and exists ONLY on this variant, so a
    /// non-removable entry cannot carry a branch-deletion flag.
    Safe {
        /// Whether the branch tip is already an ancestor of the target HEAD.
        delete_branch: bool,
        /// pi's second reason line, when the branch must be retained.
        note: Option<SafeNote>,
    },
    /// Structurally not a candidate.
    Ineligible(IneligibleReason),
    /// pi `:669` — the gate that must never be `Remove`.
    Dirty,
    /// The owning run has not finished.
    Active(ActiveReason),
    /// Metadata and reality disagree.
    Stale(StaleReason),
    /// Nothing could be established.
    Unknown(UnknownReason),
}

impl Verdict {
    /// pi's `state` field.
    pub(crate) fn state(&self) -> CleanupState {
        match self {
            Self::Safe { .. } => CleanupState::Safe,
            Self::Ineligible(_) => CleanupState::Ineligible,
            Self::Dirty => CleanupState::Dirty,
            Self::Active(_) => CleanupState::Active,
            Self::Stale(_) => CleanupState::Stale,
            Self::Unknown(_) => CleanupState::Unknown,
        }
    }

    /// pi's `decision` field — the total function this whole type exists to encode.
    ///
    /// The `match` is exhaustive over [`Verdict`], so adding a variant without saying what it
    /// decides does not compile.
    pub(crate) fn decision(&self) -> CleanupDecision {
        match self {
            Self::Safe { .. } => CleanupDecision::Remove,
            Self::Ineligible(_) | Self::Dirty | Self::Active(_) => CleanupDecision::Keep,
            Self::Stale(_) | Self::Unknown(_) => CleanupDecision::Unknown,
        }
    }

    /// Whether a future apply phase may delete the local branch with the worktree.
    ///
    /// `None` on every non-`Safe` verdict, which is what elides pi's `willDeleteBranch` key from
    /// a kept entry.
    pub(crate) fn delete_branch(&self) -> Option<bool> {
        match self {
            Self::Safe { delete_branch, .. } => Some(*delete_branch),
            _ => None,
        }
    }

    /// pi's `reasons` array, in pi's own sentences.
    pub(crate) fn reasons(&self) -> Vec<String> {
        match self {
            Self::Safe { note, .. } => {
                let mut reasons = vec![
                    "extension-owned metadata, terminal run, contained clean worktree, and local \
                     divergence checks passed"
                        .to_string(),
                ];
                if let Some(note) = note {
                    reasons.push(note.to_string());
                }
                reasons
            }
            Self::Ineligible(reason) => vec![reason.to_string()],
            Self::Dirty => vec!["worktree has uncommitted or untracked changes".to_string()],
            Self::Active(reason) => vec![reason.to_string()],
            Self::Stale(reason) => vec![reason.to_string()],
            Self::Unknown(reason) => vec![reason.to_string()],
        }
    }
}
