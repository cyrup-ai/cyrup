//! The plan's on-disk contract, its newtypes, and its error taxonomy — pi
//! `worktree-cleanup-plan.ts:20-86` @`v0.68.0`.
//!
//! **The JSON is the interface.** Field names stay upstream's (`#[serde(rename_all =
//! "camelCase")]`); Rust identifiers do not have to. Unlike the manifest — which pi also writes —
//! this file is written and read only by cyrup, so every record carries
//! `#[serde(deny_unknown_fields)]`: an unrecognised key in a file this build wrote is corruption,
//! not forward compatibility.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// pi `WORKTREE_CLEANUP_PLAN_VERSION` (`:20`).
pub const WORKTREE_CLEANUP_PLAN_VERSION: u32 = 1;

/// pi `WORKTREE_CLEANUP_PLAN_TTL_MS` (`:21`) — 30 minutes.
///
/// Nothing enforces it yet, and that is upstream's state too: `worktree.cleanup` is plan-only at
/// `v0.68.0` (`subagent-executor.ts:6217-6222`), there is no apply path and no plan reader
/// anywhere in upstream. [`WorktreeCleanupPlan::expires_at`] is the forward contract a later
/// apply phase checks; it is written completely and read by nobody.
pub const WORKTREE_CLEANUP_PLAN_TTL_MS: i64 = 30 * 60 * 1000;

/// pi `MAX_DISCOVERED_HANDOFFS` (`:22`).
pub(crate) const MAX_DISCOVERED_HANDOFFS: usize = 256;
/// pi `MAX_PLAN_ENTRIES` (`:23`) — applied to groups, tasks and the final entry list.
pub(crate) const MAX_PLAN_ENTRIES: usize = 512;
/// pi `MAX_METADATA_FILE_BYTES` (`:24`) — 2 MiB, on both the manifest and the status file.
pub(crate) const MAX_METADATA_FILE_BYTES: u64 = 2 * 1024 * 1024;

// =================================================================================================
// Errors (pi throws in exactly six places, all fatal to the whole plan)
// =================================================================================================

/// Everything that aborts a plan BUILD, as opposed to blocking one worktree.
///
/// Upstream throws in exactly six places — `:743` (no repo path), `:746` (non-finite `now`),
/// `:189` (bad plan id), `:221` (empty base dir), `:129`/`:298` (`runGitChecked` and
/// `worktree list --porcelain`). Everything else in the module is a [`super::verdict::Verdict`]
/// on one entry or a warning on the plan. That is why this enum is small: a cleanup plan whose
/// job is to be cautious must degrade to "keep, here is why", not to an error.
///
/// pi's `:746` non-finite-timestamp throw has **no variant here**: it is discharged by the type,
/// because [`super::BuildCleanupPlanInput::now`] is an `i64` epoch-millis and cannot be NaN. The
/// variant is dropped rather than carried behind an `allow(dead_code)`.
#[derive(Debug, thiserror::Error)]
pub enum CleanupPlanError {
    /// pi `:743`. Kept as a variant although the Rust signature takes `repo: &Path`, because a
    /// path that trims to nothing still has to be refused at the tool boundary.
    #[error("worktree cleanup plan requires a repository path")]
    MissingRepo,

    /// pi `validatePlanId:189`, verbatim.
    #[error(
        "worktree cleanup plan id must contain only letters, numbers, dots, underscores, or hyphens"
    )]
    InvalidPlanId,

    /// pi `:221`, verbatim.
    #[error("worktree base directory cannot be empty")]
    EmptyBaseDir,

    /// pi `gitFailure` (`:145`) — the `stderr -> stdout -> "<command> failed"` ladder, verbatim,
    /// with no prefix added. Upstream surfaces these as `isError` prose the model reads.
    #[error("{0}")]
    Git(String),

    /// The plan file could not be persisted.
    #[error("failed to write worktree cleanup plan {path}: {source}")]
    Write {
        /// Where the write was attempted.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
}

// =================================================================================================
// Newtypes
// =================================================================================================

/// pi `validatePlanId` (`:187-192`) as a type.
///
/// The `^[A-Za-z0-9._-]+$` pattern plus the `.`/`..` carve-out is a **path-traversal guard**, not
/// a style rule: the value is interpolated straight into a filename (`:822`). As a newtype,
/// [`super::worktree_cleanup_plan_path`] cannot be called with an unvalidated string at all — the
/// guard moves from "remember to call the validator" to "does not typecheck", and a plan id of
/// `../../etc/passwd` is a compile-time impossibility rather than a runtime check someone can
/// forget to make.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PlanId(String);

impl PlanId {
    /// The only fallible constructor — pi `:187-192`.
    ///
    /// # Errors
    ///
    /// [`CleanupPlanError::InvalidPlanId`], carrying pi's verbatim sentence.
    pub fn parse(raw: &str) -> Result<Self, CleanupPlanError> {
        if raw.is_empty()
            || raw == "."
            || raw == ".."
            || !raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        {
            return Err(CleanupPlanError::InvalidPlanId);
        }
        Ok(Self(raw.to_string()))
    }

    /// A fresh opaque id — pi `randomUUID()` (`:812`).
    ///
    /// v4, deliberately not v7: the plan's creation time is already recorded inside it as
    /// `createdAt`, and a v7 id would additionally leak that timestamp into a FILENAME sitting in
    /// the operator's repository.
    #[must_use]
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Borrows the validated id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`PlanId::parse`], the crate idiom pinned at
/// [`crate::workflows::WorkflowKey`]'s hand-written impl (`workflows/key.rs:73-78`).
///
/// This is not belt-and-braces: a plan file is untrusted input, and the value read back out of it
/// is what a future apply phase would join back to a filename.
impl<'de> Deserialize<'de> for PlanId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A SHA-256 hex digest of the plan's content payload — pi `contentHash` (`:811`).
///
/// A newtype so it cannot be transposed with a [`CommitId`], which is also a hex string of a
/// similar shape. The two are never interchangeable and the compiler now says so.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    /// The explicit hex fold follows `workflows/stable_json.rs:55`'s own note: `sha2` 0.11's
    /// digest output does not implement `LowerHex`, so `{:x}` does not compile on it.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        use sha2::{Digest as _, Sha256};
        Self(
            Sha256::digest(bytes)
                .iter()
                .fold(String::with_capacity(64), |mut out, byte| {
                    use std::fmt::Write as _;
                    let _ = write!(out, "{byte:02x}");
                    out
                }),
        )
    }

    /// Borrows the digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A git object name that `git rev-parse --verify <x>^{commit}` resolved.
///
/// Exists so [`CleanupPreconditions`]'s four commit slots cannot be filled with an unresolved ref
/// by a later edit: the only constructor is [`CommitId::resolved`], and its one caller is the
/// branch of [`super::classify`] that just saw `rev-parse` exit 0.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl CommitId {
    /// The only constructor: `pub(crate)` and named for its precondition.
    pub(crate) fn resolved(value: &str) -> Self {
        Self(value.trim().to_string())
    }

    /// Borrows the resolved object name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A SHA-256 of `git status --porcelain=v1 --untracked-files=all` output — pi `:561`.
///
/// Deliberately the DIGEST and never the output. The plan file is written inside the operator's
/// repository; the porcelain output is a listing of their untracked filenames. A digest is enough
/// for a future apply phase to prove the worktree has not changed since the plan was built, and
/// it leaks nothing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StatusDigest(String);

impl StatusDigest {
    pub(crate) fn of(status_output: &str) -> Self {
        Self(ContentHash::of(status_output.as_bytes()).0)
    }

    /// Borrows the digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// =================================================================================================
// Wire enums (pi's string unions)
// =================================================================================================

/// pi `WorktreeCleanupPlanState` (`:26`).
///
/// Never stored as an independent field in memory — it is projected from
/// [`super::verdict::Verdict::state`] at serialization time, so a `state`/`decision` pair that
/// disagrees is unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupState {
    /// Every gate passed.
    Safe,
    /// Structurally not a cleanup candidate (outside the base dir, the repo root, …).
    Ineligible,
    /// Metadata and git disagree, or a marker aged out.
    Stale,
    /// Uncommitted or untracked work is present.
    Dirty,
    /// The owning run is still going.
    Active,
    /// Something could not be established. Absence of proof, never proof of safety.
    Unknown,
}

/// pi `WorktreeCleanupPlanDecision` (`:27`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupDecision {
    /// A later apply phase may remove this worktree.
    Remove,
    /// Keep it, with a reason the operator can read.
    Keep,
    /// Needs manual review.
    Unknown,
}

/// pi `WorktreeCleanupPlanSource` (`:28`) — where the entry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupSource {
    /// Git knows this worktree; no handoff metadata claims it.
    Git,
    /// Handoff metadata records it; git does not list it.
    Metadata,
    /// Both agree it exists — the only source a managed entry can have.
    Both,
}

/// pi `ForegroundRunOwnership` (`:29`).
///
/// Never serialized: it is the return type of the injected probe
/// ([`super::ForegroundOwnershipProbe`]), not a JSON field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundRunOwnership {
    /// The owning foreground run is in flight right now.
    Active,
    /// The owning foreground run settled and every child settled with it.
    Terminal,
    /// This process cannot prove either. Treated as non-removable.
    Unknown,
}

// =================================================================================================
// The plan
// =================================================================================================

/// pi `WorktreeCleanupPlanPreconditions` (`:31-40`) — what a future apply phase must re-check
/// before acting on an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupPreconditions {
    /// The worktree root, as the entry names it.
    pub path: PathBuf,
    /// The branch checked out there, or `""` for a detached or unidentified worktree.
    pub branch: String,
    /// `git worktree list --porcelain`'s `HEAD` line for this worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_head: Option<String>,
    /// `refs/heads/<branch>` at plan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_tip: Option<CommitId>,
    /// The group's base commit, resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<CommitId>,
    /// A digest of the worktree's porcelain status — never the status itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_digest: Option<StatusDigest>,
    /// `dirname(entry.path)` at plan time (pi `:529`).
    pub recorded_base_dir: PathBuf,
    /// The repository HEAD the divergence check ran against.
    pub target_ref: String,
}

/// pi `WorktreeCleanupPlanEntry` (`:42-56`) — one worktree's verdict.
///
/// `will_delete_branch` is `Option` and absent on every non-removable entry, which is pi's own
/// shape (`:707` sets it only on the safe path) AND the invariant: a kept worktree can never
/// carry a branch-deletion flag, because [`super::verdict::Verdict::Safe`] is the only variant
/// that has the field at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupPlanEntry {
    /// The worktree root.
    pub path: PathBuf,
    /// The branch checked out there, or `""`.
    pub branch: String,
    /// What a future apply phase may do.
    pub decision: CleanupDecision,
    /// Why, as a classification.
    pub state: CleanupState,
    /// Why, in pi's own sentences, for the operator.
    pub reasons: Vec<String>,
    /// Which side of the git/metadata join produced this entry.
    pub source: CleanupSource,
    /// Whether the local branch would go with the worktree. Present only on a removable entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_delete_branch: Option<bool>,
    /// The owning run, when metadata claims this worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The manifest that claims it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_path: Option<PathBuf>,
    /// The cleanup task index inside that manifest's group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_index: Option<u32>,
    /// The group's base commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    /// A durable patch that was VALIDATED to still represent the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_path: Option<PathBuf>,
    /// The repository HEAD this entry was evaluated against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    /// What an apply phase must re-verify.
    pub preconditions: CleanupPreconditions,
}

/// pi `WorktreeCleanupPlan` (`:58-70`).
///
/// Field ORDER is wire order: the workspace's `serde_json` carries `preserve_order`
/// (root `Cargo.toml:181`), and [`WorktreeCleanupPlan::content_hash`] is computed over a payload
/// whose key order matches pi's `contentPayload` (`:726-736`). That makes the digest
/// **recomputable by this build** from the plan's own fields, which is what a future apply phase
/// needs. It is deliberately NOT a byte-parity claim against pi.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeCleanupPlan {
    /// Always [`WORKTREE_CLEANUP_PLAN_VERSION`].
    pub version: u32,
    /// This plan's opaque, path-safe identity.
    pub plan_id: PlanId,
    /// The realpath'd repository toplevel the plan was built for.
    pub repo_root: PathBuf,
    /// Epoch millis.
    pub created_at: i64,
    /// `created_at + `[`WORKTREE_CLEANUP_PLAN_TTL_MS`]. Written; read by nobody yet (see the
    /// constant's own note).
    pub expires_at: i64,
    /// Always exactly one element (pi `:804`) — the managed-worktree base directory.
    pub base_dirs: Vec<PathBuf>,
    /// Every handoff manifest consulted, sorted by comparable path.
    pub metadata_paths: Vec<PathBuf>,
    /// One verdict per worktree, sorted by comparable path then branch.
    pub entries: Vec<CleanupPlanEntry>,
    /// Stale entries git itself reports as prunable. A REPORT: nothing is pruned (pi `:862`).
    pub prune_candidates: Vec<PathBuf>,
    /// Deduped and sorted; the key is OMITTED when empty (pi `:808`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    /// SHA-256 over `{version, repoRoot, baseDirs, metadataPaths, entries, pruneCandidates,
    /// warnings?}` — the plan MINUS its identity and timestamps (pi `:726-736`).
    pub content_hash: ContentHash,
}

/// pi `CreatedWorktreeCleanupPlan` (`:82-85`).
#[derive(Clone, Debug)]
pub struct CreatedCleanupPlan {
    /// The plan itself.
    pub plan: WorktreeCleanupPlan,
    /// Where it was persisted.
    pub plan_path: PathBuf,
}

/// pi `worktreeCleanupPlanPath` (`:821-823`) —
/// `<repoRoot>/.cyrup-subagents/cleanup-plans/<planId>.json`.
///
/// Takes `&PlanId`, not `&str`: see [`PlanId`]. The project root differs — cyrup's is
/// `.cyrup-subagents` while pi's `PROJECT_SUBAGENTS_RELATIVE_DIR` is `.pi/subagents`
/// (`shared/artifacts.ts:6` @v0.68.0) — but that is an established crate-wide delta carried by
/// [`crate::artifacts::project_subagents_dir`], not a new one this module introduces.
#[must_use]
pub fn worktree_cleanup_plan_path(repo_root: &Path, plan_id: &PlanId) -> PathBuf {
    crate::artifacts::project_subagents_dir(repo_root)
        .join("cleanup-plans")
        .join(format!("{plan_id}.json"))
}
