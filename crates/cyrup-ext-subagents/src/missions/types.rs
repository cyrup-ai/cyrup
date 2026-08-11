//! The mission subsystem's data model — a 1:1 port of
//! `pi-subagents/src/missions/types.ts` (157 lines @v0.43.0).
//!
//! **Cite this file's upstream by its full path.** `pi-subagents` v0.43.0 has exactly three
//! `types.ts` files — `src/missions/types.ts`, `src/shared/types.ts` and `src/watchdog/types.ts`
//! — so a bare "types.ts" citation is ambiguous across all three. This module ports the FIRST one
//! only; nothing here comes from `src/shared/types.ts` or `src/watchdog/types.ts`.
//!
//! # What a mission is
//!
//! A **mission** is a durable, on-disk record of an objective that outlives any single subagent
//! run: a title + objective, a status, an append-only list of the runs launched against it, the
//! decisions those runs surfaced, the artifacts they wrote, and the delivery receipts (PRs, CI
//! runs, deployments, releases) that closed it out. A mission with `goal` set additionally carries
//! a token BUDGET and accumulated USAGE, which together drive the turn-end continuation notices
//! [`crate::missions::goal_driver`] raises.
//!
//! # Field order is load-bearing
//!
//! Every struct below declares its fields in the exact order
//! `store.ts`'s corresponding `parse*` function emits them in its returned object literal, because
//! `serde`'s derived `Serialize` writes struct fields in declaration order and upstream's
//! `writePrivateAtomicJson` writes `JSON.stringify(payload, null, 2)` of the PARSED (i.e.
//! re-ordered) value — so a cyrup-written `<missionId>.json` is byte-comparable with a
//! pi-written one. Reordering a field here silently breaks that.
//!
//! Optional fields are `#[serde(skip_serializing_if = "Option::is_none")]` for the same reason:
//! upstream's `...(x ? { x } : {})` spread OMITS an absent key rather than writing `null`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// pi `MISSION_STATUSES` (`missions/types.ts:1-9`) — the ordered status vocabulary, used verbatim
/// in the `must be one of …` validation message (`store.ts:64`, `actions.ts:101`), which is why
/// this is an ordered array and not just the enum's variant set.
pub const MISSION_STATUSES: [MissionStatus; 7] = [
    MissionStatus::Planned,
    MissionStatus::Active,
    MissionStatus::Waiting,
    MissionStatus::NeedsDecision,
    MissionStatus::Completed,
    MissionStatus::Failed,
    MissionStatus::Cancelled,
];

/// The three statuses `store.ts:38`'s `TERMINAL_MISSION_STATUSES` treats as terminal — a mission in
/// one of these is eligible for retention pruning and is never advanced by a later run's status.
pub const TERMINAL_MISSION_STATUSES: [MissionStatus; 3] =
    [MissionStatus::Completed, MissionStatus::Failed, MissionStatus::Cancelled];

/// pi `MissionStatus` (`missions/types.ts:11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    /// Created but not yet launched against.
    Planned,
    /// At least one run is in flight, or the mission is a live goal mission.
    Active,
    /// Every linked run is paused; the mission is waiting on a human or a follow-up.
    Waiting,
    /// An open decision is blocking progress.
    NeedsDecision,
    /// Terminal: the objective was met.
    Completed,
    /// Terminal: the objective was not met.
    Failed,
    /// Terminal: abandoned.
    Cancelled,
}

impl MissionStatus {
    /// The on-the-wire string (`"needs_decision"`, …) — the same spelling
    /// [`MISSION_STATUSES`]-joined validation messages use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::NeedsDecision => "needs_decision",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Membership in `store.ts:38`'s `TERMINAL_MISSION_STATUSES`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Parse from the wire spelling; `None` for anything outside the vocabulary.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        MISSION_STATUSES.into_iter().find(|s| s.as_str() == value)
    }
}

/// pi `MissionRunMode` (`missions/types.ts:12`). Note this is a SUPERSET of the subagent run modes
/// — `workflow`/`scheduled`/`external` name run shapes that are not `single`/`parallel`/`chain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionRunMode {
    /// One agent invocation.
    Single,
    /// A static parallel fan-out.
    Parallel,
    /// A linear chain.
    Chain,
    /// A `workflowScript` run.
    Workflow,
    /// A scheduled run.
    Scheduled,
    /// Anything attached from outside the subagent executor (including a management action's own
    /// `mode: "management"`, which `lifecycle.ts:94` maps onto this variant).
    External,
}

impl MissionRunMode {
    /// The on-the-wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Parallel => "parallel",
            Self::Chain => "chain",
            Self::Workflow => "workflow",
            Self::Scheduled => "scheduled",
            Self::External => "external",
        }
    }

    /// Parse from the wire spelling (`store.ts:33`'s `MISSION_RUN_MODES` set membership).
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        [
            Self::Single,
            Self::Parallel,
            Self::Chain,
            Self::Workflow,
            Self::Scheduled,
            Self::External,
        ]
        .into_iter()
        .find(|m| m.as_str() == value)
    }
}

/// pi `MissionArtifactKind` (`missions/types.ts:13`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionArtifactKind {
    /// A background run's `status.json`.
    Status,
    /// A delivered output file (a child's `outputPath`/`savedOutputPath`/structured output).
    Output,
    /// A patch/diff.
    Patch,
    /// A parallel-handoff manifest.
    Manifest,
    /// A review document.
    Review,
    /// A free-form note.
    Note,
    /// Anything else (transcripts, `events.jsonl`, …).
    Other,
}

impl MissionArtifactKind {
    /// The on-the-wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Output => "output",
            Self::Patch => "patch",
            Self::Manifest => "manifest",
            Self::Review => "review",
            Self::Note => "note",
            Self::Other => "other",
        }
    }

    /// Parse from the wire spelling (`store.ts:34`'s `MISSION_ARTIFACT_KINDS`).
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        [
            Self::Status,
            Self::Output,
            Self::Patch,
            Self::Manifest,
            Self::Review,
            Self::Note,
            Self::Other,
        ]
        .into_iter()
        .find(|k| k.as_str() == value)
    }
}

/// pi `MissionReceiptKind` (`missions/types.ts:14`) — the four delivery-evidence classes a mission
/// can close against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionReceiptKind {
    /// A pull request.
    PullRequest,
    /// A CI run.
    Ci,
    /// A deployment.
    Deployment,
    /// A release.
    Release,
}

impl MissionReceiptKind {
    /// The on-the-wire string (`"pull_request"`, …).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PullRequest => "pull_request",
            Self::Ci => "ci",
            Self::Deployment => "deployment",
            Self::Release => "release",
        }
    }

    /// Parse from the wire spelling (`store.ts:35`'s `MISSION_RECEIPT_KINDS`).
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::PullRequest, Self::Ci, Self::Deployment, Self::Release]
            .into_iter()
            .find(|k| k.as_str() == value)
    }
}

/// pi `MissionReceiptStatus` (`missions/types.ts:15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionReceiptStatus {
    /// Created, not yet ready.
    Pending,
    /// Ready for review/merge/promotion.
    Ready,
    /// Landed.
    Succeeded,
    /// Did not land.
    Failed,
}

impl MissionReceiptStatus {
    /// The on-the-wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Parse from the wire spelling (`store.ts:36`'s `MISSION_RECEIPT_STATUSES`).
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::Pending, Self::Ready, Self::Succeeded, Self::Failed]
            .into_iter()
            .find(|s| s.as_str() == value)
    }
}

/// pi `MissionGoalStatus` (`missions/types.ts:16`). Note the hyphen in `budget-exhausted` — it is
/// the ONLY hyphenated string in this file's vocabulary, hence the explicit `rename`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissionGoalStatus {
    /// Turn-end continuation notices are raised for this mission.
    #[serde(rename = "active")]
    Active,
    /// Notices are suppressed until the goal is resumed.
    #[serde(rename = "paused")]
    Paused,
    /// Accumulated usage reached the configured budget; notices stop.
    #[serde(rename = "budget-exhausted")]
    BudgetExhausted,
}

impl MissionGoalStatus {
    /// The on-the-wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::BudgetExhausted => "budget-exhausted",
        }
    }

    /// Parse from the wire spelling (`store.ts:86`'s three-way equality test).
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::Active, Self::Paused, Self::BudgetExhausted]
            .into_iter()
            .find(|s| s.as_str() == value)
    }
}

/// pi `MissionGoal` (`missions/types.ts:18-20`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionGoal {
    /// Whether continuation notices are live, paused, or budget-exhausted.
    pub status: MissionGoalStatus,
}

/// pi `MissionTokenBudget` (`missions/types.ts:22-24`) — a POSITIVE integer token ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionTokenBudget {
    /// The ceiling, in tokens. Validated `>= 1`.
    pub tokens: u64,
}

/// pi `MissionTokenUsage` (`missions/types.ts:26-28`) — a NON-NEGATIVE integer token count (0 is
/// legal here, unlike [`MissionTokenBudget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionTokenUsage {
    /// Tokens consumed so far.
    pub tokens: u64,
}

/// pi `MissionRunLink` (`missions/types.ts:30-40`) — one subagent run attached to a mission.
///
/// Field order matches `parseRunLink`'s returned literal (`store.ts:122-132`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionRunLink {
    /// The run's id.
    pub run_id: String,
    /// The run's shape.
    pub mode: MissionRunMode,
    /// The background run directory, when this run is/was async.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_dir: Option<String>,
    /// Zero-based child index within the run, for a per-child link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_index: Option<u64>,
    /// The agent persona, when the run resolved to exactly one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The run's last-observed status. Deliberately a free-form string, not an enum: upstream
    /// copies whatever `status.json`'s `state` says (`actions.ts:260`, `goal-driver.ts:45`), which
    /// is a wider vocabulary than [`MissionStatus`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// ISO-8601 start timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// ISO-8601 completion timestamp, once the run settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Tokens this run consumed, folded into the mission's usage total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<MissionTokenUsage>,
}

/// The open/resolved state of a [`MissionDecision`] (pi's inline `"open" | "resolved"` union,
/// `missions/types.ts:44`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionDecisionStatus {
    /// Still blocking.
    Open,
    /// Answered.
    Resolved,
}

impl MissionDecisionStatus {
    /// The on-the-wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }
}

/// pi `MissionDecision` (`missions/types.ts:42-52`).
///
/// Field order matches `parseDecision`'s returned literal (`store.ts:139-149`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionDecision {
    /// A generated id, validated by the same rule as a mission id.
    pub id: String,
    /// Open or resolved.
    pub status: MissionDecisionStatus,
    /// One-line question.
    pub title: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// Longer prompt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Candidate answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// The recommended answer — also what [`crate::missions::goal_driver`] surfaces as the next
    /// ready action when a mission has an open decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    /// ISO-8601 resolution timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    /// The answer that was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

/// pi `MissionArtifact` (`missions/types.ts:54-58`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionArtifact {
    /// What kind of file this is.
    pub kind: MissionArtifactKind,
    /// Its path. Compared for dedup with `path.resolve` on both sides (`store.ts:418`).
    pub path: String,
    /// Optional human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// pi `MissionReceipt` (`missions/types.ts:60-67`) — delivery evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionReceipt {
    /// PR / CI / deployment / release.
    pub kind: MissionReceiptKind,
    /// Its current status.
    pub status: MissionReceiptStatus,
    /// Human title.
    pub title: String,
    /// Absolute URL. Validated by constructing a WHATWG URL (`store.ts:170-174`).
    pub url: String,
    /// ISO-8601 creation timestamp — assigned by the store, never by the caller (which is why
    /// `MissionUpdateInput::add_receipts` carries the receipt WITHOUT this field).
    pub created_at: String,
    /// Optional human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A [`MissionReceipt`] as a CALLER supplies it — pi's `Omit<MissionReceipt, "createdAt">`
/// (`missions/types.ts:156`, `actions.ts:60`). The store stamps `created_at` itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionReceiptInput {
    /// PR / CI / deployment / release.
    pub kind: MissionReceiptKind,
    /// Its current status.
    pub status: MissionReceiptStatus,
    /// Human title.
    pub title: String,
    /// Absolute URL.
    pub url: String,
    /// Optional human label.
    pub description: Option<String>,
}

/// A [`MissionDecision`] as a CALLER supplies it — pi's
/// `Omit<MissionDecision, "id" | "status" | "createdAt">` (`missions/types.ts:155`). The store
/// generates the id, forces `status: "open"`, and stamps `created_at`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MissionDecisionInput {
    /// One-line question.
    pub title: String,
    /// Longer prompt text.
    pub prompt: Option<String>,
    /// Candidate answers.
    pub options: Option<Vec<String>>,
    /// The recommended answer.
    pub recommendation: Option<String>,
}

/// The `schemaVersion: 1` literal every persisted mission shape carries.
pub const MISSION_SCHEMA_VERSION: u8 = 1;

/// serde helper: the constant `1` written for `schemaVersion` on serialize.
const fn schema_version_one() -> u8 {
    MISSION_SCHEMA_VERSION
}

/// pi `MissionRecord` (`missions/types.ts:69-89`) — the durable on-disk mission.
///
/// Field order matches `parseMissionRecord`'s returned literal (`store.ts:202-222`) so a
/// cyrup-written `<missionId>.json` is key-for-key comparable with a pi-written one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionRecord {
    /// Always `1`.
    #[serde(default = "schema_version_one")]
    pub schema_version: u8,
    /// The mission id (a v4 UUID at creation; validated against the id pattern on every read).
    pub id: String,
    /// Short human title.
    pub title: String,
    /// The objective prose. Required — a record whose `objective` is absent but whose legacy
    /// STRING `goal` field is present takes its objective from there (`store.ts:79-82`).
    pub objective: String,
    /// Goal mode, when enabled. Requires [`Self::budget`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<MissionGoal>,
    /// Token ceiling for goal mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<MissionTokenBudget>,
    /// Accumulated token usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<MissionTokenUsage>,
    /// The mission's lifecycle status.
    pub status: MissionStatus,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 last-update timestamp; also the list sort key (descending).
    pub updated_at: String,
    /// Every run attached to this mission, in attach order.
    pub runs: Vec<MissionRunLink>,
    /// Every decision raised against it.
    pub decisions: Vec<MissionDecision>,
    /// Every artifact produced for it.
    pub artifacts: Vec<MissionArtifact>,
    /// Every delivery receipt recorded against it.
    pub receipts: Vec<MissionReceipt>,
    /// The project root the mission was created in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The session that owns it — the goal driver only raises notices for missions owned by the
    /// CURRENT session (`goal-driver.ts:128`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_id: Option<String>,
    /// The latest run's delivered text, bounded to 2000 chars by `lifecycle.ts:148`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// An opaque acceptance ledger carried through verbatim (`acceptance?: unknown` upstream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<serde_json::Value>,
    /// De-duplicated, trimmed labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

/// pi `MissionIndexEntry` (`missions/types.ts:91-100`) — one pointer file in the cross-project
/// global index. Field order matches `parseIndexEntry` (`store.ts:282-291`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionIndexEntry {
    /// Always `1`.
    #[serde(default = "schema_version_one")]
    pub schema_version: u8,
    /// The mission this pointer refers to.
    pub mission_id: String,
    /// The project root the record lives under.
    pub project_root: String,
    /// The absolute path of the `<missionId>.json` record.
    pub record_path: String,
    /// A copy of the record's title, so `mission.list --global` renders without opening every
    /// record.
    pub title: String,
    /// A copy of the record's status.
    pub status: MissionStatus,
    /// A copy of the record's `updatedAt`; also the global list's sort key.
    pub updated_at: String,
    /// The last attached run's id, when the record has any runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}

/// pi `MissionStoreConfig` (`missions/types.ts:102-108`) — the `config.missions` block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionStoreConfig {
    /// `false` disables AUTOMATIC mission creation on launch (`lifecycle.ts:65`). It does NOT
    /// disable explicit `mission`/`missionId` parameters or the `mission.*` actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Overrides the default `<projectRoot>/.cyrup-subagents/missions` record directory.
    /// `~/`-prefixed and relative values are expanded (`store.ts:225-228`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// `false` suppresses writing the cross-project pointer index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_index: Option<bool>,
    /// Overrides the default `<agentDir>/missions/index` pointer directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_index_dir: Option<String>,
    /// How many TERMINAL missions to retain before pruning the oldest (default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain_terminal: Option<u64>,
}

/// pi `MissionStoreLocation` (`missions/types.ts:110-116`) — the fully resolved, absolute
/// filesystem placement every store function takes as its first argument.
///
/// [`PathBuf`] rather than upstream's `string` because every consumer immediately does path
/// arithmetic on these; the persisted binding (`lifecycle.rs`) serializes them back as strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionStoreLocation {
    /// The absolute project root.
    pub project_root: PathBuf,
    /// Where `<missionId>.json` records and `<missionId>/state.json` live.
    pub mission_dir: PathBuf,
    /// Where the cross-project pointer files live.
    pub global_index_dir: PathBuf,
    /// Whether to maintain the pointer index at all.
    pub write_global_index: bool,
    /// Per-location terminal-retention override.
    pub retain_terminal: Option<u64>,
}

/// pi `MissionListResult` (`missions/types.ts:118-121`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionListResult {
    /// Successfully parsed records, newest `updatedAt` first.
    pub records: Vec<MissionRecord>,
    /// One warning per skipped corrupt file — never an error: a corrupt neighbour must not make
    /// the whole list fail.
    pub warnings: Vec<String>,
}

/// pi `GlobalMissionIndexRecord` (`missions/types.ts:123-126`), which `extends MissionIndexEntry`
/// — flattened here, since Rust has no interface inheritance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalMissionIndexRecord {
    /// The pointer's own contents. `#[serde(flatten)]` reproduces upstream's `{ ...entry, stale }`
    /// spread, so the JSON shape is FLAT exactly as `mission.list --global`'s `details` is.
    #[serde(flatten)]
    pub entry: MissionIndexEntry,
    /// `true` when the pointed-at record exists but could not be validated / disagreed about its
    /// own id. (A pointer whose record is MISSING is deleted rather than marked stale.)
    pub stale: bool,
    /// Why it is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
}

/// pi `GlobalMissionListResult` (`missions/types.ts:128-131`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalMissionListResult {
    /// Pointer entries, newest `updatedAt` first.
    pub entries: Vec<GlobalMissionIndexRecord>,
    /// One warning per skipped/removed pointer.
    pub warnings: Vec<String>,
}

/// pi `MissionCreateInput` (`missions/types.ts:133-141`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionCreateInput {
    /// Required, trimmed.
    pub title: String,
    /// Required, trimmed.
    pub objective: String,
    /// `Some(true)` enables goal mode — and then [`Self::budget`] is REQUIRED
    /// (`store.ts:355`). Upstream tests `input.goal === true` exactly, so `Some(false)` behaves
    /// like `None`.
    pub goal: Option<bool>,
    /// The token ceiling.
    pub budget: Option<MissionTokenBudget>,
    /// Initial status; defaults to [`MissionStatus::Planned`].
    pub status: Option<MissionStatus>,
    /// Labels.
    pub labels: Option<Vec<String>>,
    /// The creating session.
    pub owner_session_id: Option<String>,
}

/// The three-way `goal?: MissionGoal | false` field of pi's `MissionUpdateInput`
/// (`missions/types.ts:146`). `None` on [`MissionUpdateInput::goal`] is upstream's `undefined`
/// ("leave the current goal alone").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionGoalUpdate {
    /// upstream `false` — clear goal mode.
    Disable,
    /// upstream an object — set goal mode to this status.
    Set(MissionGoal),
}

/// pi `MissionUpdateInput` (`missions/types.ts:143-157`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissionUpdateInput {
    /// Replace the title (trimmed).
    pub title: Option<String>,
    /// Replace the objective (trimmed).
    pub objective: Option<String>,
    /// Enable/disable/retune goal mode.
    pub goal: Option<MissionGoalUpdate>,
    /// Replace the budget.
    pub budget: Option<MissionTokenBudget>,
    /// Replace usage EXPLICITLY. When absent, usage is RECOMPUTED as the sum over
    /// `runs[].usage.tokens` (`store.ts:443-445`) — that recompute is not optional.
    pub usage: Option<MissionTokenUsage>,
    /// Replace the status.
    pub status: Option<MissionStatus>,
    /// Replace the summary.
    pub summary: Option<String>,
    /// Replace the labels.
    pub labels: Option<Vec<String>>,
    /// Replace the acceptance ledger.
    pub acceptance: Option<serde_json::Value>,
    /// Upsert run links, keyed on `(runId, childIndex)`.
    pub add_runs: Vec<MissionRunLink>,
    /// Upsert artifacts, keyed on `(kind, resolved path)`.
    pub add_artifacts: Vec<MissionArtifact>,
    /// Append decisions (always as NEW, open decisions with fresh ids).
    pub add_decisions: Vec<MissionDecisionInput>,
    /// Upsert receipts, keyed on `(kind, url)`; an existing receipt keeps its ORIGINAL
    /// `createdAt` (`store.ts:428`).
    pub add_receipts: Vec<MissionReceiptInput>,
}

impl MissionUpdateInput {
    /// `true` when no field was set — pi's `Object.keys(update).length === 0` guard in
    /// `validateMissionUpdate` (`actions.ts:232`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.objective.is_none()
            && self.goal.is_none()
            && self.budget.is_none()
            && self.usage.is_none()
            && self.status.is_none()
            && self.summary.is_none()
            && self.labels.is_none()
            && self.acceptance.is_none()
            && self.add_runs.is_empty()
            && self.add_artifacts.is_empty()
            && self.add_decisions.is_empty()
            && self.add_receipts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn mission_status_wire_spellings_match_upstream_vocabulary() {
        let joined: Vec<&str> = MISSION_STATUSES.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            joined,
            vec![
                "planned",
                "active",
                "waiting",
                "needs_decision",
                "completed",
                "failed",
                "cancelled"
            ]
        );
    }

    #[test]
    fn goal_status_serializes_budget_exhausted_with_a_hyphen() {
        let json = serde_json::to_string(&MissionGoal { status: MissionGoalStatus::BudgetExhausted })
            .unwrap();
        assert_eq!(json, r#"{"status":"budget-exhausted"}"#);
    }

    #[test]
    fn receipt_kind_serializes_pull_request_with_an_underscore() {
        assert_eq!(MissionReceiptKind::PullRequest.as_str(), "pull_request");
        assert_eq!(
            serde_json::to_string(&MissionReceiptKind::PullRequest).unwrap(),
            r#""pull_request""#
        );
    }

    #[test]
    fn terminal_statuses_are_exactly_completed_failed_cancelled() {
        for status in MISSION_STATUSES {
            assert_eq!(
                status.is_terminal(),
                TERMINAL_MISSION_STATUSES.contains(&status),
                "{status:?}"
            );
        }
    }

    #[test]
    fn absent_optional_fields_are_omitted_not_nulled() {
        let record = MissionRecord {
            schema_version: MISSION_SCHEMA_VERSION,
            id: "m1".to_string(),
            title: "t".to_string(),
            objective: "o".to_string(),
            goal: None,
            budget: None,
            usage: None,
            status: MissionStatus::Planned,
            created_at: "1970-01-01T00:00:00.000Z".to_string(),
            updated_at: "1970-01-01T00:00:00.000Z".to_string(),
            runs: Vec::new(),
            decisions: Vec::new(),
            artifacts: Vec::new(),
            receipts: Vec::new(),
            cwd: None,
            owner_session_id: None,
            summary: None,
            acceptance: None,
            labels: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("null"), "{json}");
        assert!(!json.contains("goal"), "{json}");
        // Field ORDER is load-bearing (see module docs): schemaVersion first, then id/title.
        assert!(json.starts_with(r#"{"schemaVersion":1,"id":"m1","title":"t","objective":"o","status":"planned""#), "{json}");
    }

    #[test]
    fn update_input_is_empty_only_when_nothing_was_set() {
        assert!(MissionUpdateInput::default().is_empty());
        let with_status =
            MissionUpdateInput { status: Some(MissionStatus::Active), ..Default::default() };
        assert!(!with_status.is_empty());
        let with_receipt = MissionUpdateInput {
            add_receipts: vec![MissionReceiptInput {
                kind: MissionReceiptKind::Ci,
                status: MissionReceiptStatus::Pending,
                title: "t".to_string(),
                url: "https://example.com".to_string(),
                description: None,
            }],
            ..Default::default()
        };
        assert!(!with_receipt.is_empty());
    }
}
