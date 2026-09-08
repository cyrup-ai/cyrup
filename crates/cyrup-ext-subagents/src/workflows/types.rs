//! The workflow family's shared, plain data shapes — declared once here (SCOPE_3d) so SCOPE_3e/3f
//! consume them rather than redeclaring: the child-summary vocabulary (`shared/types.ts:178-208`),
//! the script trace/child-result records (`scripted-workflow.ts:997-1038`) and the preflight lane
//! hint (`shared/types.ts:115-122`).

use super::bounded::Bounded;
use super::key::WorkflowKey;

/// The run-level workflow state word — pi `WorkflowChildSummary["workflowState"]`
/// (`shared/types.ts:195`): the six-word lifecycle of the workflow run itself, as distinct from
/// the eight-word per-child vocabulary of [`WorkflowChildState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowState {
    /// Declared but not yet running.
    Queued,
    /// The script is executing.
    Running,
    /// Every lane settled and the script returned.
    Completed,
    /// The script (or a required child) failed.
    Failed,
    /// Interrupted — soft, resumable.
    Paused,
    /// An explicit stop request ended the run.
    Stopped,
}

/// One workflow child's state word — pi `WorkflowChildSummary.children[].state`
/// (`shared/types.ts:205`), eight variants.
///
/// Note the vocabulary: the settled word here is **`completed`**, while
/// [`crate::background::StepState::Complete`] serializes as **`complete`** — two adjacent enums,
/// two spellings of one idea (SCOPE_3d §0.19/§0.20). They are distinct Rust types and are never
/// `as_str()`-bridged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowChildState {
    /// Declared (a status step exists) but not yet dispatched.
    Pending,
    /// Currently executing — the only state that may carry an `activity`.
    Running,
    /// Finished without failure.
    Completed,
    /// Finished with a failure.
    Failed,
    /// Interrupted mid-flight.
    Paused,
    /// An explicit stop ended it.
    Stopped,
    /// The child's acceptance verdict was `rejected` — produced only by the settled-children pass
    /// of the summary builder (`workflow-child-summary.ts:105`); cyrup's
    /// [`crate::background::StepState`] has no `Rejected`, so the step pass's `rejected` arm is
    /// dead here.
    Rejected,
    /// The child detached into its own background run.
    Detached,
}

impl WorkflowChildState {
    /// pi `TERMINAL_STATES` (`workflow-child-summary.ts:5`) — six of the eight states:
    /// everything but `pending` and `running`.
    ///
    /// Named `..._for_inventory` because `paused` counts as terminal HERE even though
    /// [`crate::background::StepState::is_terminal`] excludes it: the inventory sweep asks "did we
    /// learn this child's outcome", not "can it resume". The two predicates must never be unified.
    #[must_use]
    pub fn is_terminal_for_inventory(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

/// Bounded foreground activity for one running workflow child — pi `WorkflowChildActivity`
/// (`shared/types.ts:178-188`): a current tool name plus eight numeric counters, and nothing else.
///
/// The counters are [`serde_json::Number`]s rather than a narrower integer type because upstream
/// accepts ANY finite non-negative JS number on the read path
/// (`workflow-child-summary.ts:36`) — a fractional counter must round-trip, not be rejected or
/// silently rounded. The finite/non-negative rule is enforced by the two producers
/// ([`crate::workflows::workflow_child_activity`] filters, `parse_workflow_child_summary`
/// validates), exactly as upstream splits it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowChildActivity {
    /// The tool currently executing, bounded to 256 UTF-8 bytes by construction (pi `:24`'s
    /// `bounded(progress.currentTool, 256)` check, dissolved into the type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<Bounded<256>>,
    /// Epoch-millis the current tool started (pi `currentToolStartedAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_started_at: Option<serde_json::Number>,
    /// Epoch-millis of the most recent observed activity (pi `lastActivityAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<serde_json::Number>,
    /// Elapsed child wall-clock milliseconds (pi `durationMs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<serde_json::Number>,
    /// Count of tool calls started (pi `toolCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<serde_json::Number>,
    /// Count of assistant turns completed (pi `turnCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<serde_json::Number>,
    /// Total tokens observed (pi `tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<serde_json::Number>,
    /// Input-side tokens (pi `inputTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<serde_json::Number>,
    /// Output-side tokens (pi `outputTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<serde_json::Number>,
}

/// One row of a [`WorkflowChildSummary`]'s inventory — pi
/// `WorkflowChildSummary["children"][number]` (`shared/types.ts:196-206`).
///
/// The five identity fields carry their per-field limits in their TYPES (SCOPE_3 §A.4): 256 UTF-8
/// bytes for `run_id`/`agent`/`session_name`/`model`, 32 for `thinking` — the one odd limit
/// (pi `workflow-child-summary.ts:99`). Over-long values are rejected at construction, never
/// truncated.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowChildRow {
    /// The child's stable workflow key (pi `childId`), grammar-checked by construction.
    pub child_id: WorkflowKey,
    /// The child's state word.
    pub state: WorkflowChildState,
    /// The child's own background run id, when launch resolution produced one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Bounded<256>>,
    /// Canonical child agent name, when known (and, on the step pass, only once launch resolved —
    /// `workflow-child-summary.ts:96`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Bounded<256>>,
    /// Human-readable display name for the child session, when derived at launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<Bounded<256>>,
    /// The model the child ran (or is running) on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Bounded<256>>,
    /// The child's effective thinking level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Bounded<32>>,
    /// Present only while a synchronous foreground child is running — set exclusively by the
    /// live-progress pass (`workflow-child-summary.ts:121-132`), and rejected by the reader on any
    /// non-`running` row (`:163`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<WorkflowChildActivity>,
}

/// The literal `version: 1` on a [`WorkflowChildSummary`] — pi `WorkflowChildSummary.version`
/// (`shared/types.ts:191`).
///
/// A unit type rather than a `u32` field so "this summary is version 1" is a *parse* outcome, not
/// a value a reader has to remember to check — the same shape as
/// `background::result_index`'s `IndexVersion`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SummaryVersion;

impl SummaryVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for SummaryVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for SummaryVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported workflow child summary version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// The versioned workflow child inventory — pi `WorkflowChildSummary` (`shared/types.ts:190-208`),
/// the direct producer of `WaitCompletion.workflowChildren` (SCOPE_3h reads it, SCOPE_3g writes
/// it onto the terminal payload).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowChildSummary {
    /// Always `1`; any other value fails to parse.
    pub version: SummaryVersion,
    /// The parent tool call this workflow run answers to — required, ≤ 4096 UTF-8 bytes (pi
    /// `MAX_REQUIRED_ID_BYTES`, `workflow-child-summary.ts:6`).
    pub parent_tool_call_id: Bounded<4096>,
    /// The workflow run's own id — same requiredness and bound as `parent_tool_call_id`.
    pub workflow_run_id: Bounded<4096>,
    /// Whether the child inventory is settled: once `true`, no row may remain non-terminal (the
    /// builder's sweep forces stragglers to `stopped`/`failed`).
    pub inventory_complete: bool,
    /// The run-level workflow state.
    pub workflow_state: WorkflowState,
    /// The per-child inventory, in **insertion order** (pi `[...rows.values()]` preserves JS `Map`
    /// insertion order; the builder's map is a `Vec` for the same reason — SCOPE_3d §0.16).
    pub children: Vec<WorkflowChildRow>,
}

/// The operation word on a workflow script trace entry — pi
/// `WorkflowScriptTraceEntry.operation` (`scripted-workflow.ts:1024`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowScriptOperation {
    /// A `runs.run(...)` child launch — the only operation the summary and chat-progress builders
    /// read.
    Run,
    /// A `runs.status(...)` poll.
    Status,
    /// A `runs.steer(...)` delivery.
    Steer,
    /// A `runs.host(...)` host-command execution.
    Host,
}

/// The state word on a workflow script trace entry — pi `WorkflowScriptTraceEntry.state`
/// (`scripted-workflow.ts:1026`), nine variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowScriptTraceState {
    /// The operation began.
    Started,
    /// The operation finished successfully.
    Completed,
    /// The operation failed.
    Failed,
    /// The child detached into a background run.
    Detached,
    /// The child was stopped.
    Stopped,
    /// A lane result was reused rather than re-run — updates display metadata only, never state.
    Reused,
    /// The operation was queued behind a capacity gate.
    Queued,
    /// A steer message was delivered.
    Delivered,
    /// A steer message missed its target.
    Missed,
}

/// One entry of a workflow script's execution trace — pi `WorkflowScriptTraceEntry`
/// (`scripted-workflow.ts:1023-1038`).
///
/// `key` stays a raw `String` here: the trace records what the script DID, and the two consumers
/// in this crate parse it at their own boundary (`WorkflowKey::parse(..).ok()` in a filter),
/// skipping entries the grammar rejects exactly as upstream's key-pattern `.test` guards do.
///
/// `lane` landed with SCOPE_3f's lane-metadata port, exactly the additive growth the SCOPE_3d
/// revision of this doc promised.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowScriptTraceEntry {
    /// Which script operation this entry records.
    pub operation: WorkflowScriptOperation,
    /// The workflow key the operation addressed.
    pub key: String,
    /// The operation's state word.
    pub state: WorkflowScriptTraceState,
    /// Canonical child agent name when resolved launch or result data is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The launched child's run id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Elapsed milliseconds for the operation, when settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Display-only phase label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Display-only row label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The operation's error text, when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Internal provenance for a generated `runs.lanes` child key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_lane_key: Option<String>,
    /// Bounded lane metadata attached to the operation, when the launch declared one
    /// (pi `WorkflowScriptTraceEntry.lane`, `scripted-workflow.ts:1036`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<WorkflowLaneMetadata>,
    /// A non-fatal warning attached to the operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// One settled workflow child result — pi `WorkflowScriptChildResult`
/// (`scripted-workflow.ts:997-1015`), narrowed to the plain-data core the summary builder reads
/// plus the identity/output fields any consumer needs.
///
/// `results` is a list of loosely-typed result objects because that is how the one consumer reads
/// it — `workflow-child-summary.ts:104` treats each element as `Record<string, unknown>` and
/// takes the first *object* element — and because pi's `SingleResult` carries fields
/// (`sessionName`, `thinking`) cyrup's does not. The optional metadata fields (`lane`,
/// `terminalOutcome`, `recovery`, `outputPathMapping`, `externalAdapter`, `resumability`,
/// `continuation`, `outputReference`, `requestedContext`, `resolvedContext`) landed with
/// SCOPE_3f — the additive growth the SCOPE_3d revision of this doc promised.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowScriptChildResult {
    /// The child's workflow key — raw, parsed by consumers (same rationale as
    /// [`WorkflowScriptTraceEntry::key`]).
    pub key: String,
    /// Whether the child succeeded.
    pub ok: bool,
    /// The child was stopped by an explicit stop request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// Canonical child agent name when launch resolution produced one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The child's own run id, when launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The child's delivered output text.
    #[serde(default)]
    pub output: String,
    /// The child's error text, when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The child detached into a background run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub detached: bool,
    /// The child was interrupted mid-flight.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// The child's structured output value, when captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    /// Bounded lane metadata the launch declared (pi `:1000`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<WorkflowLaneMetadata>,
    /// A partial-terminal outcome (budget/timeout) the child ended in (pi `:1001`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// The context the launch requested (pi `:1010`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_context: Option<WorkflowRequestedContext>,
    /// The context launch resolution produced (pi `:1011`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_context: Option<WorkflowResolvedContext>,
    /// A reference to the child's saved output file (pi `:1012`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_reference: Option<String>,
    /// Acceptance-recovery metadata for a rejected child (pi `:1013`) — the shape
    /// [`crate::workflows::scripted::is_acceptance_metadata_recovery`] reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<AcceptanceRecoveryMetadata>,
    /// The requested→saved output-path remap, when the host relocated the child's output
    /// (pi `:1014`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path_mapping: Option<WorkflowOutputPathMapping>,
    /// External-CLI adapter receipt metadata (pi `:1015`). Loosely typed: its producer (the
    /// external-adapter family) is unported, its shape is a seven-way safety union upstream, and
    /// every consumer in this crate treats it as opaque display data — so it crosses as JSON
    /// rather than as a parallel type that would immediately drift ([CYRUP-DELTA, representation]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_adapter: Option<serde_json::Value>,
    /// Whether the child can be resumed (pi `:1018`) — the reason lives ON the `not-resumable`
    /// variant, so a reasonless `not-resumable` is unrepresentable (SCOPE_3 §A.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumability: Option<WorkflowResumability>,
    /// The resume lineage: every run id this child continued through, oldest first (pi `:1019`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<WorkflowContinuation>,
    /// Artifact paths the child produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_paths: Vec<String>,
    /// The child's per-attempt result objects, loosely typed (see the struct doc). The summary
    /// builder reads `sessionName`/`model`/`thinking`/`acceptance.status` off the first object
    /// element.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<serde_json::Value>,
}

/// A preflight lane's advisory mode word — pi `WorkflowPreflightMode` (`shared/types.ts:112`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowPreflightMode {
    /// The lane mutates the tree.
    Mutation,
    /// The lane reviews another lane's product.
    Review,
    /// The lane gathers information only.
    Scout,
    /// The lane gates a later stage.
    Gate,
}

/// Bounded, display-only lane hints supplied alongside a `workflowScript` launch — pi
/// `WorkflowPreflightLane` (`shared/types.ts:115-122`). Declared here (SCOPE_3d) because
/// [`crate::workflows::WorkflowChatProgressRow`] carries one; SCOPE_3e's preflight module is its
/// producer and normalizer.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPreflightLane {
    /// The lane's plan-root key, grammar-checked by construction (pi validates it at
    /// `workflow-preflight.ts:11`; the check dissolves into the type at SCOPE_3e's parse
    /// boundary).
    pub key: WorkflowKey,
    /// The lane's advisory mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<WorkflowPreflightMode>,
    /// Why this lane exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// What the lane claims to cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<Vec<String>>,
    /// What output the lane is expected to produce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_output: Option<String>,
    /// Why the lane is independent of its siblings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub independence: Option<String>,
}

/// pi `WorkflowPreflightCoverage` (`shared/types.ts:111`) — whether the declared lanes claim to
/// cover the whole plan. `Partial` is the DEFAULT when `coverage` is omitted
/// (`workflow-preflight.ts:106`), not an error.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowPreflightCoverage {
    /// The declared lanes cover the whole plan.
    Complete,
    /// The declared lanes cover only part of the plan — the omission default.
    #[default]
    Partial,
}

impl WorkflowPreflightCoverage {
    /// The serde word, for the preflight formatters that print it verbatim.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
        }
    }
}

/// The literal `version: 1` on a [`WorkflowPreflight`] — pi `WorkflowPreflight.version`
/// (`shared/types.ts:126`). A unit type, as [`SummaryVersion`]: "this preflight is version 1" is
/// a parse outcome, not a field a reader must remember to check. (SCOPE_3e's normalizer still
/// validates the raw JSON field itself, because its rejection message — `preflight.version must
/// be 1.` — is upstream's, not serde's.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreflightVersion;

impl PreflightVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for PreflightVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for PreflightVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported workflow preflight version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// pi `WorkflowPreflight` (`shared/types.ts:125-129`) — the versioned, display-only workflow
/// launch plan. "It never grants launch authority" (`:124`): nothing in this family consults it
/// to decide whether a child may run; it only produces advisory warnings and rendered text.
///
/// Field order is upstream's literal `{ version, coverage, lanes }` — SCOPE_3e's
/// `normalize_workflow_preflight` measures this struct's canonical JSON against
/// `WORKFLOW_PREFLIGHT_MAX_BYTES`, so the serialized shape is load-bearing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPreflight {
    /// Always `1`; any other value fails to parse.
    pub version: PreflightVersion,
    /// Whether the lanes claim to cover the whole plan.
    pub coverage: WorkflowPreflightCoverage,
    /// The declared lanes, in declaration order.
    pub lanes: Vec<WorkflowPreflightLane>,
}

/// The literal `version: 1` on a [`WorkflowLaneMetadata`] — pi `WorkflowLaneMetadata.version`
/// (`shared/types.ts:169`). Unit type, same rationale as [`PreflightVersion`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaneMetadataVersion;

impl LaneMetadataVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for LaneMetadataVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for LaneMetadataVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported workflow lane metadata version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// A lane's advisory mode word — pi `WorkflowLaneMode` (`shared/types.ts:166`). The same four
/// words as [`WorkflowPreflightMode`] but a DISTINCT upstream type on a distinct record
/// (`WorkflowLaneMetadata.mode` vs `WorkflowPreflightLane.mode`); kept distinct here so neither
/// can grow a word without the other noticing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowLaneMode {
    /// The lane mutates the tree.
    Mutation,
    /// The lane reviews another lane's product.
    Review,
    /// The lane gathers information only.
    Scout,
    /// The lane gates a later stage.
    Gate,
}

/// Bounded, versioned lane metadata attached to a workflow child launch — pi
/// `WorkflowLaneMetadata` (`shared/types.ts:168-175`). The guest validates the raw object
/// (`validateLaneMetadata`, `scripted-workflow.ts:606-624`) before it ever reaches this type.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowLaneMetadata {
    /// Always `1`; any other value fails to parse.
    pub version: LaneMetadataVersion,
    /// The lane's key — raw, consumers parse ([`WorkflowScriptTraceEntry::key`]'s rationale).
    pub key: String,
    /// The lane's advisory mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<WorkflowLaneMode>,
    /// The upstream source ref the lane claims to build on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// What the lane claims to cover (max 20 × 160 bytes, guest-enforced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<Vec<String>>,
    /// Output paths the lane claims (max 10 × 256 bytes, guest-enforced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_paths: Option<Vec<String>>,
}

/// A partial-terminal outcome — pi `WorkflowTerminalOutcome` (`shared/types.ts:135-138`):
/// `state` is always `"partial"`, so only the reason varies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum WorkflowTerminalOutcome {
    /// `{ state: "partial", reason }`.
    #[serde(rename = "partial")]
    Partial {
        /// Why the run ended partial.
        reason: WorkflowTerminalOutcomeReason,
    },
}

/// The `reason` word on [`WorkflowTerminalOutcome`] (`shared/types.ts:137`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowTerminalOutcomeReason {
    /// The run exhausted its usage budget.
    BudgetExhausted,
    /// The run hit its deadline.
    Timeout,
}

/// Acceptance-recovery metadata for a rejected child — pi `AcceptanceRecoveryMetadata`
/// (`shared/types.ts`). `status`/`reason` are single-literal unions upstream but stay `String`
/// here: the value crosses process boundaries as JSON written by other (older/newer) builds, and
/// upstream itself gates on runtime equality (`scripted-workflow.ts:1147-1148`), which
/// [`crate::workflows::scripted::is_acceptance_metadata_recovery`] reproduces — a parse-time
/// rejection would turn an unknown status into a lost child result instead of a non-match.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceRecoveryMetadata {
    /// Upstream literal: `"available-for-review"`.
    pub status: String,
    /// Upstream literal: `"acceptance-metadata-rejected"`.
    pub reason: String,
    /// The saved recovery report path.
    pub report_path: String,
    /// The recovery report's content hash.
    pub report_hash: String,
}

/// The requested→saved output-path remap — pi `WorkflowScriptChildResult.outputPathMapping`
/// (`scripted-workflow.ts:1014`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowOutputPathMapping {
    /// The path the launch requested.
    pub requested_path: String,
    /// The path the host actually saved to.
    pub saved_path: String,
}

/// The context a workflow child launch requested — pi `"fresh" | "fork"`
/// (`scripted-workflow.ts:1010`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowRequestedContext {
    /// A fresh child session.
    Fresh,
    /// A fork of the parent session.
    Fork,
}

/// The context launch resolution produced — pi `"fresh" | "fork" | "mixed"`
/// (`scripted-workflow.ts:1011`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowResolvedContext {
    /// Every attempt ran fresh.
    Fresh,
    /// Every attempt ran forked.
    Fork,
    /// Attempts mixed contexts.
    Mixed,
}

/// Whether a workflow child can be resumed — pi `WorkflowScriptChildResult.resumability`
/// (`scripted-workflow.ts:1018`): `{ state: "resumable" } | { state: "not-resumable"; reason }`.
/// The reason lives ON the variant that has one — an `Option<String>` beside a flag would admit a
/// reasonless `not-resumable`, which the source cannot produce (SCOPE_3 §A.1).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state")]
pub enum WorkflowResumability {
    /// The child retains a resumable session.
    #[serde(rename = "resumable")]
    Resumable,
    /// The child cannot be resumed, and why.
    #[serde(rename = "not-resumable")]
    NotResumable {
        /// Why the child cannot be resumed.
        reason: String,
    },
}

/// The resume lineage — pi `WorkflowScriptChildResult.continuation` (`scripted-workflow.ts:1019`):
/// every run id the child continued through, oldest first, deduplicated.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowContinuation {
    /// The lineage run ids.
    pub run_ids: Vec<String>,
}
