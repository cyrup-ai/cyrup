//! Every acceptance enum and shape ported from pi's `shared/types.ts:639-802`: levels,
//! evidence kinds, config-input and resolved shapes, and the report/runtime-check/ledger records.

// --------------------------------------------------------------------------------------------
// Enums (shared/types.ts:639-650)
// --------------------------------------------------------------------------------------------

/// `AcceptanceLevel` (`shared/types.ts:639` @v0.43.0:
/// `"auto" | "none" | "attested" | "checked" | "verified"`) — `auto` is the "infer" sentinel;
/// every other variant is a concrete provenance level. Ordering rank is
/// `none < attested < checked < verified` ([`level_rank`]); `Auto` has no rank.
///
/// **`reviewed` is NOT a level.** Up to v0.34.0 the union carried a sixth member `"reviewed"`;
/// v0.43.0 removed it, because `reviewed` is an ACHIEVED ledger status (something an
/// independent reviewer produces) and never a requestable acceptance level. A policy that wants
/// independent review declares `acceptance.review.required` instead, and
/// `validateAcceptanceInput` now rejects the string `"reviewed"` outright with
/// [`crate::exec::acceptance::model::validate_input::EXPLICIT_REVIEWED_UNAVAILABLE`] (`acceptance.ts:54,181,195-196`). The achieved status still
/// exists — see [`AcceptanceLedgerStatus::Reviewed`], which is deliberately NOT an
/// [`AcceptanceEvidenceStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceLevel {
    Auto,
    None,
    Attested,
    Checked,
    Verified,
}

impl AcceptanceLevel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AcceptanceLevel::Auto => "auto",
            AcceptanceLevel::None => "none",
            AcceptanceLevel::Attested => "attested",
            AcceptanceLevel::Checked => "checked",
            AcceptanceLevel::Verified => "verified",
        }
    }
}

/// `LEVEL_RANK` (acceptance.ts:28-33 @v0.43.0) — `None` for `Auto` (unranked).
pub(crate) fn level_rank(level: AcceptanceLevel) -> Option<u8> {
    match level {
        AcceptanceLevel::Auto => Option::None,
        AcceptanceLevel::None => Some(0),
        AcceptanceLevel::Attested => Some(1),
        AcceptanceLevel::Checked => Some(2),
        AcceptanceLevel::Verified => Some(3),
    }
}

/// `AcceptanceEvidenceKind` (shared/types.ts:641-650).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceEvidenceKind {
    ChangedFiles,
    TestsAdded,
    CommandsRun,
    ValidationOutput,
    ResidualRisks,
    NoStagedFiles,
    DiffSummary,
    ReviewFindings,
    ManualNotes,
}

impl AcceptanceEvidenceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AcceptanceEvidenceKind::ChangedFiles => "changed-files",
            AcceptanceEvidenceKind::TestsAdded => "tests-added",
            AcceptanceEvidenceKind::CommandsRun => "commands-run",
            AcceptanceEvidenceKind::ValidationOutput => "validation-output",
            AcceptanceEvidenceKind::ResidualRisks => "residual-risks",
            AcceptanceEvidenceKind::NoStagedFiles => "no-staged-files",
            AcceptanceEvidenceKind::DiffSummary => "diff-summary",
            AcceptanceEvidenceKind::ReviewFindings => "review-findings",
            AcceptanceEvidenceKind::ManualNotes => "manual-notes",
        }
    }

    /// Parse one authored `evidence[]` entry. `None` for anything not in
    /// `AcceptanceEvidenceKind` (shared/types.ts:641-650) — [`crate::exec::acceptance::model::validate_input::validate_acceptance_input`] has already
    /// rejected such an entry with `evidence[i] is not a supported evidence kind.` by the time
    /// [`crate::exec::acceptance::lower_acceptance_input`] calls this, so the `None` arm is a total-function
    /// guard rather than a reachable policy path.
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        match text {
            "changed-files" => Some(AcceptanceEvidenceKind::ChangedFiles),
            "tests-added" => Some(AcceptanceEvidenceKind::TestsAdded),
            "commands-run" => Some(AcceptanceEvidenceKind::CommandsRun),
            "validation-output" => Some(AcceptanceEvidenceKind::ValidationOutput),
            "residual-risks" => Some(AcceptanceEvidenceKind::ResidualRisks),
            "no-staged-files" => Some(AcceptanceEvidenceKind::NoStagedFiles),
            "diff-summary" => Some(AcceptanceEvidenceKind::DiffSummary),
            "review-findings" => Some(AcceptanceEvidenceKind::ReviewFindings),
            "manual-notes" => Some(AcceptanceEvidenceKind::ManualNotes),
            _ => Option::None,
        }
    }
}

/// `"required" | "recommended"` (shared/types.ts:656).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateSeverity {
    Required,
    Recommended,
}

/// SUBA-082 — `AcceptanceRole` (`shared/types.ts:31` @v0.64.0: `"read-only" | "writer"`), the
/// agent-declared role that REPLACES agent-name guessing inside `inferLevel`
/// (`runs/shared/acceptance.ts:100-104` @v0.64.0): `readOnlyAgent` is `role === "read-only" ||
/// (role === undefined && /\b(?:reviewer|oracle|scout|researcher|analyst)\b/.test(agent))`, and
/// `writeTask` gains a `role === "writer" && !readOnlyTask` arm while its `\bworker\b` name arm is
/// gated on `role === undefined`. Declared on an agent file as `acceptanceRole:` frontmatter
/// (`agents.ts:2046-2050` @v0.64.0) and carried on
/// [`crate::discovery::types::AgentDefinition::acceptance_role`]. It does NOT grant or revoke tools
/// (`docs/agents.md:327` @v0.64.0) — it only steers acceptance inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceRole {
    ReadOnly,
    Writer,
}

impl AcceptanceRole {
    /// The wire/frontmatter spelling (`read-only` / `writer`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AcceptanceRole::ReadOnly => "read-only",
            AcceptanceRole::Writer => "writer",
        }
    }

    /// The exact-spelling parse upstream applies to the frontmatter value
    /// (`agents.ts:2048` @v0.64.0: `=== "read-only" || === "writer"`, no trimming, no case
    /// folding). `None` for anything else — the CALLER decides whether that is an error (it is,
    /// for a non-blank frontmatter value) or an absence (a blank one).
    #[must_use]
    pub fn parse_exact(raw: &str) -> Option<Self> {
        match raw {
            "read-only" => Some(AcceptanceRole::ReadOnly),
            "writer" => Some(AcceptanceRole::Writer),
            _ => Option::None,
        }
    }
}

// --------------------------------------------------------------------------------------------
// Config-input shapes (shared/types.ts:652-685)
// --------------------------------------------------------------------------------------------

/// One acceptance criterion as authored: either a bare `must` string or a full [`AcceptanceGate`]
/// (types.ts `Array<string | AcceptanceGate>`).
#[derive(Debug, Clone, PartialEq)]
pub enum CriterionInput {
    Text(String),
    Gate(AcceptanceGate),
}

/// `AcceptanceGate` (shared/types.ts:652-657).
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptanceGate {
    pub id: Option<String>,
    pub must: Option<String>,
    pub evidence: Option<Vec<AcceptanceEvidenceKind>>,
    pub severity: Option<GateSeverity>,
}

/// `AcceptanceVerifyCommand` (shared/types.ts:659-666).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceVerifyCommand {
    pub id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_failure: Option<bool>,
}

impl AcceptanceVerifyCommand {
    /// A bare shell command with no per-command overrides — every optional field left unset so
    /// [`crate::exec::acceptance::model::verify::run::run_verify_command`] applies the run-level `cwd`, the inherited
    /// environment and [`crate::exec::acceptance::model::verify::run::DEFAULT_VERIFY_TIMEOUT_MS`], exactly as an authored entry that
    /// declares only `{ id, command }` does.
    ///
    /// `id` defaults to the command text itself. Upstream *requires* an explicit `id`
    /// (`acceptance.ts:209` — `verify[i].id is required.`) and
    /// [`crate::exec::acceptance::model::validate_input::validate_acceptance_input`] enforces that before [`crate::exec::acceptance::lower_acceptance_input`]
    /// runs, so this fallback is only ever reached by callers constructing a contract in Rust
    /// rather than from an authored `acceptance` param.
    #[must_use]
    pub fn shell(command: impl Into<String>) -> Self {
        let command = command.into();
        Self {
            id: command.clone(),
            command,
            timeout_ms: Option::None,
            cwd: Option::None,
            env: Option::None,
            allow_failure: Option::None,
        }
    }
}

impl From<String> for AcceptanceVerifyCommand {
    fn from(command: String) -> Self {
        Self::shell(command)
    }
}

impl From<&str> for AcceptanceVerifyCommand {
    fn from(command: &str) -> Self {
        Self::shell(command)
    }
}

/// A declared command compares equal to the bare command string it runs, so callers that only
/// care about *which shell commands a contract will execute* (the property that mattered when
/// [`crate::exec::acceptance::VerifyCommand`] was still a `String`) keep expressing that directly. The
/// per-command overrides are deliberately NOT part of this comparison — use the derived
/// `PartialEq` on two `AcceptanceVerifyCommand`s for full structural equality.
impl PartialEq<String> for AcceptanceVerifyCommand {
    fn eq(&self, other: &String) -> bool {
        self.command == *other
    }
}

impl PartialEq<&str> for AcceptanceVerifyCommand {
    fn eq(&self, other: &&str) -> bool {
        self.command == *other
    }
}

/// `AcceptanceReviewGate` (shared/types.ts:668-672).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceReviewGate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// `AcceptanceReviewGate | false` (shared/types.ts:679) — `Disabled` is the `false` shorthand.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ReviewSetting {
    Disabled(bool),
    Gate(AcceptanceReviewGate),
}

/// `AcceptanceConfig` (shared/types.ts:674-682).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AcceptanceConfig {
    pub level: Option<AcceptanceLevel>,
    pub criteria: Option<Vec<CriterionInput>>,
    pub evidence: Option<Vec<AcceptanceEvidenceKind>>,
    pub verify: Option<Vec<AcceptanceVerifyCommand>>,
    pub review: Option<ReviewSetting>,
    pub stop_rules: Option<Vec<String>>,
    pub reason: Option<String>,
}

/// `AcceptanceInput = Exclude<AcceptanceLevel, "none" | "verified"> | false | AcceptanceConfig` (shared/types.ts:685).
#[derive(Debug, Clone, PartialEq)]
pub enum AcceptanceInput {
    Level(AcceptanceLevel),
    Disabled,
    Config(AcceptanceConfig),
}

// --------------------------------------------------------------------------------------------
// Resolved shapes (shared/types.ts:687-704)
// --------------------------------------------------------------------------------------------

/// `ResolvedAcceptanceGate` (shared/types.ts:687-692).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAcceptanceGate {
    pub id: String,
    pub must: String,
    pub evidence: Vec<AcceptanceEvidenceKind>,
    pub severity: GateSeverity,
}

/// `ResolvedAcceptanceConfig` (shared/types.ts:694-704).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAcceptanceConfig {
    pub level: AcceptanceLevel,
    pub explicit: bool,
    pub inferred_reason: Vec<String>,
    pub criteria: Vec<ResolvedAcceptanceGate>,
    pub evidence: Vec<AcceptanceEvidenceKind>,
    pub verify: Vec<AcceptanceVerifyCommand>,
    pub review: Option<ReviewSetting>,
    pub stop_rules: Vec<String>,
    pub reason: Option<String>,
}

// --------------------------------------------------------------------------------------------
// Report / runtime-check / ledger shapes (shared/types.ts:706-802)
// --------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CriterionStatus {
    Satisfied,
    NotSatisfied,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CriterionReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub status: CriterionStatus,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRunResult {
    Passed,
    Failed,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CommandRunReport {
    pub command: String,
    pub result: CommandRunResult,
    pub summary: String,
}

/// `AcceptanceReport` (shared/types.ts:706-726).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria_satisfied: Option<Vec<CriterionReport>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_added_or_updated: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands_run: Option<Vec<CommandRunReport>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_output: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_risks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_staged_files: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_findings: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeCheckStatus {
    Passed,
    Failed,
    NotApplicable,
}

/// `AcceptanceRuntimeCheck` (shared/types.ts:730-734).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptanceRuntimeCheck {
    pub id: String,
    pub status: RuntimeCheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyRunStatus {
    Passed,
    Failed,
    TimedOut,
    AllowedFailure,
}

/// `AcceptanceVerifyResult` (`shared/types.ts:736-758` @v0.43.0).
///
/// The trailing seven fields are the memoization EVIDENCE upstream stamps onto every result
/// that went through [`crate::exec::acceptance::model::verify::memo::run_memoized_verify_command`] (`acceptance.ts:1106,1112,1128-1129`).
/// They are all `Option` and all `skip_serializing_if`-omitted, exactly like upstream's `?:`
/// members, so a result produced without a memo context serializes byte-for-byte as it did
/// before this port.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceVerifyResult {
    pub id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub status: VerifyRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub duration_ms: u128,
    /// `artifactPath` (`shared/types.ts:745`) — where this run's memo artifact was read from/written
    /// to. Cleared (`delete evidenced.artifactPath`, `acceptance.ts:1129`) when the write
    /// itself failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    /// `cacheKey` (`shared/types.ts:746`) — the sha256 over the memo identity (command text, repo-
    /// relative cwd, declared env key names, full effective-env hash, timeout, `allowFailure`,
    /// `HEAD`, working-tree diff hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    /// `memoized` (`shared/types.ts:747`) — `Some(true)` when this result was REPLAYED from the memo
    /// artifact instead of executed, `Some(false)` when it was executed under an active memo
    /// context, `None` when no memo context applied at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memoized: Option<bool>,
    /// `envKeys` (`shared/types.ts:748`) — the sorted key names of the command's OWN declared `env`
    /// (`Object.keys(command.env ?? {}).sort()`, `acceptance.ts:1088`). Names only; no values,
    /// so a secret-bearing override never reaches the ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_keys: Option<Vec<String>>,
    /// `envHash` (`shared/types.ts:749`) — sha256 over the whole EFFECTIVE environment
    /// (`acceptance.ts:1089`), so a changed secret invalidates the memo without the value ever
    /// being written down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_hash: Option<String>,
    /// `workspaceState` (`shared/types.ts:750-756`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_state: Option<VerifyWorkspaceState>,
    /// `artifactError` (`shared/types.ts:757`) — set when the memo artifact could not be written
    /// (`acceptance.ts:1128`). Never fails the verification itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_error: Option<String>,
}

/// `VerifyWorkspaceState.kind` (`acceptance.ts:1039`) — the single discriminant upstream
/// declares. A workspace that is not a git checkout produces no state at all (and therefore no
/// memoization), rather than a second variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyWorkspaceKind {
    GitTracked,
}

/// `VerifyWorkspaceState` (`acceptance.ts:1038-1044`): the identity of the working tree a
/// verify command's result is memoized AGAINST. `head` + `diff_hash` together pin both the
/// committed and the uncommitted state, so any edit to the tree invalidates every memo.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyWorkspaceState {
    pub kind: VerifyWorkspaceKind,
    pub repo_root: String,
    pub cwd_relative: String,
    pub head: String,
    pub diff_hash: String,
}

/// `AcceptanceReviewResult["status"]` (`shared/types.ts:761` @v0.43.0:
/// `"review-required" | "reviewed" | "blockers"`).
///
/// v0.34.0 spelled this `"no-blockers" | "blockers" | "needs-parent-decision"`. v0.43.0 renamed
/// both non-`blockers` members so the review outcome shares the LEDGER's own vocabulary: a
/// reviewer that signed off yields `reviewed` (which is exactly the ledger status the run then
/// takes) and an absent/incomplete review yields `review-required` (likewise). See
/// [`crate::exec::acceptance::model::evaluate::evaluate_acceptance`]'s review block, `acceptance.ts:1318-1336`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewResultStatus {
    ReviewRequired,
    Reviewed,
    Blockers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewFindingSeverity {
    Blocker,
    NonBlocking,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewFinding {
    pub severity: ReviewFindingSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub issue: String,
    pub rationale: String,
}

/// `AcceptanceReviewResult` (shared/types.ts:760-768).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptanceReviewResult {
    pub status: ReviewResultStatus,
    pub findings: Vec<ReviewFinding>,
}

/// `AcceptanceEvidenceStatus` (`shared/types.ts:770-777` @v0.43.0) — the strictly EVIDENCE-derived
/// half of the ledger's status: how far the child's own report plus the orchestrator's own
/// structural/verify checks carried this run, with review deliberately excluded.
///
/// v0.43.0 split this out of `AcceptanceLedgerStatus` so a run whose evidence genuinely reached
/// `verified` still reads as `verified` on `evidenceStatus` even while `status` sits at
/// `review-required` waiting for an independent reviewer. Before the split there was one field,
/// so "the review has not happened yet" ERASED the evidence level that had already been earned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceEvidenceStatus {
    Pending,
    NotRequired,
    Claimed,
    Attested,
    Checked,
    Verified,
    Rejected,
}

/// `AcceptanceLedgerStatus` (`shared/types.ts:779-783` @v0.43.0) —
/// `AcceptanceEvidenceStatus | "review-required" | "reviewed" | "accepted"`. Rust has no union
/// type, so the evidence members are restated here and [`AcceptanceEvidenceStatus`] converts
/// into this enum via [`From`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceLedgerStatus {
    Pending,
    NotRequired,
    Claimed,
    Attested,
    Checked,
    Verified,
    Rejected,
    ReviewRequired,
    Reviewed,
    Accepted,
}

impl From<AcceptanceEvidenceStatus> for AcceptanceLedgerStatus {
    fn from(status: AcceptanceEvidenceStatus) -> Self {
        match status {
            AcceptanceEvidenceStatus::Pending => AcceptanceLedgerStatus::Pending,
            AcceptanceEvidenceStatus::NotRequired => AcceptanceLedgerStatus::NotRequired,
            AcceptanceEvidenceStatus::Claimed => AcceptanceLedgerStatus::Claimed,
            AcceptanceEvidenceStatus::Attested => AcceptanceLedgerStatus::Attested,
            AcceptanceEvidenceStatus::Checked => AcceptanceLedgerStatus::Checked,
            AcceptanceEvidenceStatus::Verified => AcceptanceLedgerStatus::Verified,
            AcceptanceEvidenceStatus::Rejected => AcceptanceLedgerStatus::Rejected,
        }
    }
}

/// `AcceptanceLedger` (`shared/types.ts:785-800` @v0.43.0, subset actually populated by
/// `evaluateAcceptance`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceLedger {
    pub status: AcceptanceLedgerStatus,
    /// `evidenceStatus` (`shared/types.ts:787`) — moves in lockstep with `status` through the
    /// attestation/checked/verified rungs and is then FROZEN: `evaluateAcceptance`'s review
    /// block (`acceptance.ts:1318-1336`) rewrites only `status`, never this field.
    pub evidence_status: AcceptanceEvidenceStatus,
    pub explicit: bool,
    pub inferred_reason: Vec<String>,
    pub criteria: Vec<SerializableGate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_report: Option<AcceptanceReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_report_parse_error: Option<String>,
    pub runtime_checks: Vec<AcceptanceRuntimeCheck>,
    pub verify_runs: Vec<AcceptanceVerifyResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_result: Option<AcceptanceReviewResult>,
}

/// Serializable projection of a [`ResolvedAcceptanceGate`] for the ledger (evidence rendered as
/// wire strings).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SerializableGate {
    pub id: String,
    pub must: String,
    pub evidence: Vec<String>,
    pub severity: String,
}

impl SerializableGate {
    pub(crate) fn from_gate(gate: &ResolvedAcceptanceGate) -> Self {
        Self {
            id: gate.id.clone(),
            must: gate.must.clone(),
            evidence: gate.evidence.iter().map(|k| k.as_str().to_string()).collect(),
            severity: match gate.severity {
                GateSeverity::Required => "required".to_string(),
                GateSeverity::Recommended => "recommended".to_string(),
            },
        }
    }
}

impl AcceptanceVerifyResult {
    /// Whether this result REJECTS the run — upstream's
    /// `verifyRuns.some((run) => run.status === "failed" || run.status === "timed-out")`
    /// (`acceptance.ts:1297`, and identically `:1361`). A passed command and an
    /// `allowed-failure` command both return `false`: `allowFailure` is exactly the authored
    /// opt-out from rejection, and upstream's status ternary already folded it in
    /// (`acceptance.ts:1193`).
    #[must_use]
    pub fn rejects(&self) -> bool {
        matches!(
            self.status,
            VerifyRunStatus::Failed | VerifyRunStatus::TimedOut
        )
    }

    /// Everything upstream's `finish(...)` (`acceptance.ts:1150-1163`) resolves, with NO
    /// memoization evidence attached — the plain shape a command executed outside a memo
    /// context reports. [`crate::exec::acceptance::model::verify::memo::run_memoized_verify_command`] stamps the evidence on afterwards.
    pub(crate) fn unmemoized(
        command: &AcceptanceVerifyCommand,
        cwd: Option<String>,
        exit_code: Option<i32>,
        status: VerifyRunStatus,
        stdout: Option<String>,
        stderr: Option<String>,
        duration_ms: u128,
    ) -> Self {
        Self {
            id: command.id.clone(),
            command: command.command.clone(),
            cwd,
            exit_code,
            status,
            stdout,
            stderr,
            duration_ms,
            artifact_path: Option::None,
            cache_key: Option::None,
            memoized: Option::None,
            env_keys: Option::None,
            env_hash: Option::None,
            workspace_state: Option::None,
            artifact_error: Option::None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::evaluate::EvaluateAcceptanceInput;
    use crate::exec::acceptance::model::evaluate::acceptance_failure_message;
    use crate::exec::acceptance::model::evaluate::evaluate_acceptance;
    use crate::exec::acceptance::model::level::AcceptanceResolveInput;
    use crate::exec::acceptance::model::testsupport::cfg;
    use crate::exec::acceptance::model::testsupport::report_text;
    use crate::exec::acceptance::model::testsupport::resolve;
    use crate::exec::acceptance::model::testsupport::temp_dir;
    use serde_json::json;


    #[test]
    fn explicit_acceptance_strengthens_inferred_policy() {
        let resolved = resolve(AcceptanceResolveInput {
            agent_name: "reviewer".into(),
            task: Some("Review-only.".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Verified),
                verify: Some(vec![AcceptanceVerifyCommand {
                    id: "ok".into(),
                    command: "node --version".into(),
                    timeout_ms: None,
                    cwd: None,
                    env: None,
                    allow_failure: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(resolved.level, AcceptanceLevel::Verified);
        assert_eq!(resolved.verify.first().map(|v| v.id.as_str()), Some("ok"));
    }


    #[test]
    fn explicit_none_with_reason_disables_inferred_gates() {
        let resolved = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::None),
                reason: Some("parent is doing manual acceptance".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(resolved.level, AcceptanceLevel::None);
        assert!(resolved.evidence.is_empty());
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn verified_mode_runs_real_verify_commands() {
        let dir = temp_dir();
        let passing = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Verified),
                verify: Some(vec![AcceptanceVerifyCommand {
                    id: "pass".into(),
                    command: "exit 0".into(),
                    timeout_ms: Some(10_000),
                    cwd: None,
                    env: None,
                    allow_failure: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        });
        let pass_ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &passing,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(pass_ledger.status, AcceptanceLedgerStatus::Verified);
        assert_eq!(pass_ledger.verify_runs.first().map(|r| r.status), Some(VerifyRunStatus::Passed));

        let failing = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Verified),
                verify: Some(vec![AcceptanceVerifyCommand {
                    id: "fail".into(),
                    command: "exit 7".into(),
                    timeout_ms: Some(10_000),
                    cwd: None,
                    env: None,
                    allow_failure: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        });
        let fail_ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &failing,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(fail_ledger.status, AcceptanceLedgerStatus::Rejected);
        // The child's own commandsRun claim of "passed" is IRRELEVANT: the orchestrator observed
        // a real nonzero exit.
        assert_eq!(
            fail_ledger.child_report.as_ref().and_then(|r| r.commands_run.as_ref()).and_then(|c| c.first()).map(|c| c.result.clone()),
            Some(CommandRunResult::Passed)
        );
        assert_eq!(fail_ledger.verify_runs.first().map(|r| r.status), Some(VerifyRunStatus::Failed));
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    /// G78 — the review gate hangs off `acceptance.review` (`acceptance.ts:1318-1336`
    /// @v0.43.0), NOT off a `level === "reviewed"` that no longer exists, and it moves ONLY
    /// `status`: `evidence_status` keeps the `checked` the child's evidence actually earned in
    /// all three outcomes. Before the split, "the reviewer has not answered yet" erased that.
    async fn review_gate_records_reviewer_outcomes_without_disturbing_evidence_status() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                    agent: Some("reviewer".into()),
                    focus: None,
                    required: Some(true),
                })),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(acceptance.level, AcceptanceLevel::Checked);

        let reviewed = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: Some(AcceptanceReviewResult {
                status: ReviewResultStatus::Reviewed,
                findings: vec![],
            }),
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(reviewed.status, AcceptanceLedgerStatus::Reviewed);
        assert_eq!(reviewed.evidence_status, AcceptanceEvidenceStatus::Checked);
        assert!(acceptance_failure_message(&reviewed).is_none());

        let blockers = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: Some(AcceptanceReviewResult {
                status: ReviewResultStatus::Blockers,
                findings: vec![ReviewFinding {
                    severity: ReviewFindingSeverity::Blocker,
                    file: None,
                    issue: "Missing test".into(),
                    rationale: "Acceptance requires test evidence.".into(),
                }],
            }),
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(blockers.status, AcceptanceLedgerStatus::Rejected);
        assert_eq!(blockers.evidence_status, AcceptanceEvidenceStatus::Checked);
        assert_eq!(
            acceptance_failure_message(&blockers).as_deref(),
            Some("Acceptance review found blockers.")
        );

        let unavailable = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        // `review-required` is NOT `rejected` (`acceptance.ts:1334`): the run is waiting on a
        // reviewer, so it neither passes nor fails the acceptance gate on its own.
        assert_eq!(unavailable.status, AcceptanceLedgerStatus::ReviewRequired);
        assert_eq!(unavailable.evidence_status, AcceptanceEvidenceStatus::Checked);
        assert!(acceptance_failure_message(&unavailable).is_none());
        assert_eq!(
            unavailable.review_result.as_ref().map(|r| r.status),
            Some(ReviewResultStatus::ReviewRequired)
        );
        assert_eq!(
            unavailable
                .review_result
                .as_ref()
                .and_then(|r| r.findings.first())
                .map(|f| f.issue.as_str()),
            Some("Independent review has not been supplied.")
        );
    }


    // ---- G78: the checked rung's v0.43.0 rewrite (`acceptance.ts:1268-1278`) ----

    /// G78 — the headline behavioural change of the `evaluateAcceptance` rewrite: a FAILED
    /// structural check on the `checked` rung no longer returns.
    ///
    /// v0.34.0's rung read `if (checks.some(failed)) { status = "rejected"; return ledger; }`,
    /// so a policy whose criteria the child under-reported never ran its `verify[]` commands at
    /// all — the ledger came back with an EMPTY `verifyRuns` and the parent could not tell a
    /// build that was never attempted from one that passed. v0.43.0 declines to PROMOTE on a
    /// failed check and falls through (`acceptance.ts:1274-1277`); the single rejection point is
    /// the combined check at `:1308-1312`, below the verify rung.
    ///
    /// The assertion that pins it is `verify_runs`: a re-introduced early return leaves it
    /// empty. The `runtime_checks` assertions additionally pin the rung's CONTENT and ORDER —
    /// `checkCriteriaSatisfied(...)` first, then `runStructuralChecks(...)`
    /// (`acceptance.ts:1271-1272`) — so dropping or transposing either half is caught too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_criterion_does_not_short_circuit_the_verify_rung() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Verified),
                criteria: Some(vec![CriterionInput::Gate(AcceptanceGate {
                    id: Some("regression".into()),
                    must: Some("Regression is covered".into()),
                    evidence: None,
                    severity: None,
                })]),
                verify: Some(vec![AcceptanceVerifyCommand {
                    id: "unit".into(),
                    command: "exit 0".into(),
                    timeout_ms: Some(10_000),
                    cwd: None,
                    env: None,
                    allow_failure: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(acceptance.level, AcceptanceLevel::Verified);

        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            // Every declared evidence kind IS present; only the declared CRITERION is reported
            // unsatisfied, so the rung fails on exactly one check.
            output: &report_text(
                json!({"criteriaSatisfied": [
                    {"id": "regression", "status": "not-satisfied", "evidence": "test missing"}
                ]}),
                "acceptance-report",
            ),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;

        // The run IS rejected — but only at `acceptance.ts:1308-1312`, after the verify rung.
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Rejected);
        // THE point of this test: the REAL `verify[]` subprocess still ran and its result is on
        // the ledger. An early return on the failed check leaves this empty.
        assert_eq!(
            ledger.verify_runs.len(),
            1,
            "a failed structural check must not skip the verify[] rung: {ledger:?}"
        );
        assert_eq!(ledger.verify_runs[0].status, VerifyRunStatus::Passed);
        assert_eq!(ledger.verify_runs[0].id, "unit");
        assert_eq!(ledger.verify_runs[0].exit_code, Some(0));

        // The rung's own output: criteria checks FIRST, structural evidence checks after
        // (`acceptance.ts:1271-1272`), all of it on one list.
        let ids: Vec<&str> = ledger
            .runtime_checks
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(
            ids.first(),
            Some(&"criterion:regression"),
            "checkCriteriaSatisfied's checks come first: {ids:?}"
        );
        assert!(
            ids.iter().skip(1).all(|id| id.starts_with("evidence:") || *id == "no-staged-files"),
            "runStructuralChecks' checks follow them: {ids:?}"
        );
        assert!(
            ids.iter().any(|id| id.starts_with("evidence:")),
            "the structural half must actually have run: {ids:?}"
        );
        assert_eq!(
            ledger
                .runtime_checks
                .iter()
                .filter(|c| c.status == RuntimeCheckStatus::Failed)
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["criterion:regression"],
            "exactly the criterion check failed; everything else passed: {ledger:?}"
        );
        assert!(
            acceptance_failure_message(&ledger)
                .unwrap()
                .contains("Required criterion 'regression' was reported as not-satisfied")
        );
    }

}
