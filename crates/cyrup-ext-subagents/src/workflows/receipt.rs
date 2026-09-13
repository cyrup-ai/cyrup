//! The workflow terminal evidence record — ports pi
//! `workflows/workflow-receipt.ts` (366 LOC): the durable `workflow-receipt.json` a settled
//! workflow leaves behind, naming every child's retained run id and resumability verdict.
//!
//! Declares the six shared types [`SCOPE_3.md`](../../../../../../.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_3.md)
//! §A names for this task: [`WorkflowReceiptState`], [`WorkflowReceiptResume`],
//! [`WorkflowReceiptEntry`], [`WorkflowReceipt`], [`WorkflowRecoveryAction`],
//! [`WorkflowTerminalResolution`].

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::background::RunDir;
use crate::background::atomic::write_private_atomic_json_blocking;
use crate::background::result_index::errno;
use crate::identity::RunDirName;

use super::bounded::Bounded;
use super::child_summary::parse_workflow_child_summary;
use super::host_step::{HOST_STEP_MAX_COUNT, HostStepNode, assert_unique_host_step_ids};
use super::key::WorkflowKey;
use super::lane_metadata::{assert_workflow_lane_key, normalize_workflow_lane_metadata};
use super::permit::WorkflowResourceProvenance;
use super::scripted::WorkflowReceiptResumeReference;
use super::types::{
    AcceptanceRecoveryMetadata, WorkflowChildSummary, WorkflowContinuation, WorkflowLaneMetadata,
    WorkflowRequestedContext, WorkflowResolvedContext, WorkflowResumability,
    WorkflowScriptChildResult, WorkflowTerminalOutcome, WorkflowTerminalOutcomeReason,
};

/// pi `WORKFLOW_RECEIPT_VERSION` (`:12`).
pub const WORKFLOW_RECEIPT_VERSION: u32 = 1;
/// pi `WORKFLOW_RECEIPT_FILE` (`:13`) — a fixed single component; the caller-supplied half of the
/// path is the run id, guarded by [`RunDirName`] (§0.9).
pub const WORKFLOW_RECEIPT_FILE: &str = "workflow-receipt.json";
/// pi `MAX_WORKFLOW_RECEIPT_BYTES` (`:14`) — the read cap, enforced from the file's metadata size
/// before any allocation, never by reading and then measuring.
const MAX_WORKFLOW_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;

/// The literal `version: 1` on a [`WorkflowReceipt`] — pi `WorkflowReceipt.version`
/// (`shared/types.ts:228`). A unit type, the same shape as `SummaryVersion`/`PreflightVersion`/
/// `LaneMetadataVersion`/`HostStepVersion`: "this receipt is version 1" is a parse outcome, not a
/// check every reader must remember. [`WORKFLOW_RECEIPT_VERSION`] stays a `pub const` too:
/// `parse_workflow_receipt`'s own hand-walk writes upstream's exact rejection text
/// (`unsupported version.`), not this type's own generic Deserialize message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkflowReceiptVersion;

impl WorkflowReceiptVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = WORKFLOW_RECEIPT_VERSION;
}

impl serde::Serialize for WorkflowReceiptVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for WorkflowReceiptVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported workflow receipt version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// pi `WorkflowReceiptState` (`shared/types.ts:131`) — four words.
///
/// **One letter off [`super::WorkflowState`]'s `"completed"`** (§0.5's 🔄 finding): this enum's
/// settled word is `"complete"`. The two are never bridged through each other — see
/// [`crate::workflows::settlement`]'s `workflow_state`/`workflow_receipt_state` pair, which are
/// two independent exhaustive `match`es over the SAME [`crate::background::RunState`], each with
/// its own wire vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowReceiptState {
    /// The workflow completed.
    Complete,
    /// The workflow failed.
    Failed,
    /// The workflow is paused, awaiting an explicit resume.
    Paused,
    /// The workflow was stopped.
    Stopped,
}

/// pi `WorkflowTerminalResolution` (`shared/types.ts:133`) — three words describing how a
/// detached/paused workflow's terminal settlement resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowTerminalResolution {
    /// The workflow settled while paused; JavaScript workflow continuation was not persisted.
    SettledAwaitingResume,
    /// A workflow child failed.
    FailedChild,
    /// A workflow child was interrupted (or stopped).
    InterruptedChild,
}

/// The literal `call: "runs.run"` on a [`WorkflowRecoveryAction`] (`shared/types.ts:141`) — the
/// same single-variant-enum discipline `permit.rs`'s `WorkflowResourceProvenanceKind` etc. already
/// use for a fixed-literal field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkflowRecoveryCall {
    /// The only value.
    #[serde(rename = "runs.run")]
    RunsRun,
}

/// The nested `resume` argument on a [`WorkflowRecoveryAction`] — pi
/// `WorkflowRecoveryAction.resume` (`shared/types.ts:142`): `{ workflowRunId, key, latest: true }`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecoveryResume {
    /// The workflow this action resumes.
    pub workflow_run_id: Bounded<256>,
    /// The child key to resume — matches [`WorkflowRecoveryAction::key`] (`parseRecovery`'s
    /// `reference.key === key` check, `workflow-receipt.ts:290`).
    pub key: WorkflowKey,
    /// Always `true` — a keyed resume with `latest: false` is not a shape upstream produces or
    /// accepts (`resolveWorkflowReceiptResumeEntry` throws otherwise, `:341`).
    pub latest: bool,
}

/// A single recovery hint for a resumable workflow child — pi `WorkflowRecoveryAction`
/// (`shared/types.ts:140-145`), produced ONLY by
/// [`crate::workflows::workflow_recovery_actions`] and read back by this module's own
/// `parse_recovery`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecoveryAction {
    /// The resumable child's key.
    pub key: WorkflowKey,
    /// Always `"runs.run"`.
    pub call: WorkflowRecoveryCall,
    /// The resume argument a caller replays verbatim.
    pub resume: WorkflowRecoveryResume,
    /// Always `true`.
    pub task_required: bool,
}

/// The `latestRunId` + `resumability` pair as ONE value — pi
/// `WorkflowReceiptEntryResumability` (`shared/types.ts:210-212`), a union over **two flat
/// sibling keys**.
///
/// Not `{ latest_run_id: Option<_>, resumability: WorkflowResumability }`: that struct admits
/// `resumable` with no run id, which is exactly what `buildWorkflowReceipt` throws on
/// (`workflow-receipt.ts:57`) and what `assertResumableEntry` throws on twice (`:349-352`). Here
/// both throws are unrepresentable and the two `asserts entry is …` predicates dissolve into
/// `matches!`.
///
/// [`WorkflowResumability`] is the *nested object alone*; this type is that object plus its
/// sibling run id, which is why it is a second type and not a widening of the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowReceiptResume {
    /// `{ latestRunId: <id>, resumability: { state: "resumable" } }`.
    Resumable {
        /// Required by the arm — the retained run to resume.
        latest_run_id: Bounded<256>,
    },
    /// `{ latestRunId?: <id>, resumability: { state: "not-resumable", reason } }`.
    NotResumable {
        /// Optional on this arm (`:212`): a child that produced no run id has none.
        latest_run_id: Option<Bounded<256>>,
        /// Required by the arm — never blank (`:290`).
        reason: String,
    },
}

impl WorkflowReceiptResume {
    /// `latest_run_id` on either arm, for the recovery/resume readers.
    #[must_use]
    pub fn latest_run_id(&self) -> Option<&Bounded<256>> {
        match self {
            Self::Resumable { latest_run_id } => Some(latest_run_id),
            Self::NotResumable { latest_run_id, .. } => latest_run_id.as_ref(),
        }
    }

    /// pi `entry.resumability.state === "resumable"` — the ONE predicate
    /// `workflowRecoveryActions` (`workflow-settlement.ts:164`) and `parseRecovery`
    /// (`workflow-receipt.ts:270`) both test.
    #[must_use]
    pub fn is_resumable(&self) -> bool {
        matches!(self, Self::Resumable { .. })
    }

    /// pi's post-construction throw at `buildWorkflowReceipt:57`, made structural: a `resumable`
    /// verdict with no retained run id cannot be represented past this constructor.
    ///
    /// # Errors
    ///
    /// `Workflow receipt child '<key>' is resumable but has no retained run id.`, verbatim.
    fn from_parts(
        resumability: WorkflowResumability,
        latest_run_id: Option<Bounded<256>>,
        key: &WorkflowKey,
    ) -> Result<Self, WorkflowReceiptError> {
        match resumability {
            WorkflowResumability::Resumable => {
                let Some(latest_run_id) = latest_run_id else {
                    return Err(WorkflowReceiptError::Invalid(format!(
                        "Workflow receipt child '{}' is resumable but has no retained run id.",
                        key.as_str()
                    )));
                };
                Ok(Self::Resumable { latest_run_id })
            }
            WorkflowResumability::NotResumable { reason } => {
                Ok(Self::NotResumable {
                    latest_run_id,
                    reason,
                })
            }
        }
    }
}

/// One receipt entry — pi `WorkflowReceiptEntry` (`shared/types.ts:214-226`).
///
/// Field order is upstream's literal spread order (`workflow-receipt.ts:58-70`), because the
/// serialized object is what a pi build reads back.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowReceiptEntry {
    /// The child's workflow key — the map key too; the builder rejects a mismatch (`:230`).
    pub key: WorkflowKey,
    /// Bounded lane metadata, re-validated on read (SUBTASK0a).
    pub lane: Option<WorkflowLaneMetadata>,
    /// A partial-terminal outcome the child ended in.
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// Canonical child agent name.
    pub agent: Option<Bounded<256>>,
    /// The context the launch requested.
    pub requested_context: Option<WorkflowRequestedContext>,
    /// The context launch resolution produced.
    pub resolved_context: Option<WorkflowResolvedContext>,
    /// A reference to the child's saved output file.
    pub output_reference: Option<String>,
    /// Acceptance-recovery metadata for a rejected child (pi `acceptanceRecovery`, renamed from
    /// the child result's `recovery` at `:66`).
    pub acceptance_recovery: Option<AcceptanceRecoveryMetadata>,
    /// External-CLI adapter receipt metadata — opaque JSON, exactly as
    /// [`WorkflowScriptChildResult::external_adapter`] already is
    /// ([CYRUP-DELTA, representation]); its producer, `parseExternalCliReceiptMetadata`
    /// (`workflow-receipt.ts:112-198`), is unported (§0.3).
    pub external_adapter: Option<serde_json::Value>,
    /// The resume lineage — always present, possibly empty (`:69`).
    pub continuation: WorkflowContinuation,
    /// `latestRunId` + `resumability`, as one value.
    pub resume: WorkflowReceiptResume,
}

impl serde::Serialize for WorkflowReceiptEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("key", self.key.as_str())?;
        if let Some(lane) = &self.lane {
            map.serialize_entry("lane", lane)?;
        }
        if let Some(terminal_outcome) = &self.terminal_outcome {
            map.serialize_entry("terminalOutcome", terminal_outcome)?;
        }
        if let Some(agent) = &self.agent {
            map.serialize_entry("agent", agent.as_str())?;
        }
        if let Some(requested_context) = &self.requested_context {
            map.serialize_entry("requestedContext", requested_context)?;
        }
        if let Some(resolved_context) = &self.resolved_context {
            map.serialize_entry("resolvedContext", resolved_context)?;
        }
        if let Some(output_reference) = &self.output_reference {
            map.serialize_entry("outputReference", output_reference)?;
        }
        if let Some(acceptance_recovery) = &self.acceptance_recovery {
            map.serialize_entry("acceptanceRecovery", acceptance_recovery)?;
        }
        if let Some(external_adapter) = &self.external_adapter {
            map.serialize_entry("externalAdapter", external_adapter)?;
        }
        map.serialize_entry("continuation", &self.continuation)?;
        match &self.resume {
            WorkflowReceiptResume::Resumable { latest_run_id } => {
                map.serialize_entry("latestRunId", latest_run_id.as_str())?;
                map.serialize_entry(
                    "resumability",
                    &serde_json::json!({ "state": "resumable" }),
                )?;
            }
            WorkflowReceiptResume::NotResumable {
                latest_run_id,
                reason,
            } => {
                if let Some(latest_run_id) = latest_run_id {
                    map.serialize_entry("latestRunId", latest_run_id.as_str())?;
                }
                map.serialize_entry(
                    "resumability",
                    &serde_json::json!({ "state": "not-resumable", "reason": reason }),
                )?;
            }
        }
        map.end()
    }
}

/// The versioned, durable evidence record a settled workflow leaves behind — pi `WorkflowReceipt`
/// (`shared/types.ts:227-238`).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReceipt {
    /// Always `1`; any other value fails to parse.
    pub version: WorkflowReceiptVersion,
    /// This receipt's run — bounded, non-blank (§0.9's `RunDirName` guards the PATH; this is the
    /// receipt's own recorded copy of that same id).
    pub workflow_run_id: Bounded<256>,
    /// The workflow's terminal state.
    pub state: WorkflowReceiptState,
    /// Epoch-millis this receipt was written — any finite JS number, fractional allowed (mirrors
    /// `WorkflowChildActivity`'s counters' own rationale for the same representation).
    pub created_at: serde_json::Number,
    /// The entries, in **insertion order** — see `child_summary.rs:173` for why this is a `Vec`
    /// and not a map. The key lives ON the entry, so there is one source of truth for it.
    /// Serializes as the JSON **object** upstream writes, keyed by `entry.key`.
    #[serde(serialize_with = "serialize_entries_as_object")]
    pub entries: Vec<WorkflowReceiptEntry>,
    /// The resource this workflow resolved from, when any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<WorkflowResourceProvenance>,
    /// The workflow's host steps — an EMPTY list is omitted, not written as `[]` (rule 13).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub host_steps: Vec<HostStepNode>,
    /// The final child inventory, when the caller attached one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_children: Option<WorkflowChildSummary>,
    /// How a paused/detached settlement resolved, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_resolution: Option<WorkflowTerminalResolution>,
    /// A partial-terminal outcome, when the run ended that way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// Recovery hints for every resumable child, derived from `entries` at settlement time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Vec<WorkflowRecoveryAction>>,
}

fn serialize_entries_as_object<S: serde::Serializer>(
    entries: &[WorkflowReceiptEntry],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap as _;
    let mut map = serializer.serialize_map(Some(entries.len()))?;
    for entry in entries {
        map.serialize_entry(entry.key.as_str(), entry)?;
    }
    map.end()
}

impl WorkflowReceipt {
    /// The by-key lookup upstream performs as `receipt.entries[key]` — pi
    /// `resolveWorkflowReceiptResumeEntry` (`:344`) and `parseRecovery` (`:270`).
    #[must_use]
    pub fn entry(&self, key: &WorkflowKey) -> Option<&WorkflowReceiptEntry> {
        self.entries.iter().find(|entry| entry.key == *key)
    }
}

/// `{ path, receipt }` — pi `publicResult.workflowReceipt` (`workflow-settlement.ts:235`),
/// present only when a receipt was actually written.
///
/// A struct, because a wait projector eventually reads `data.workflowReceipt.path` off a NESTED
/// object; a flattened `workflow_receipt_path` field on [`crate::background::ResultFile`] would
/// change the wire shape a pi-shaped consumer sees.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReceiptRef {
    /// Where the receipt was written.
    pub path: PathBuf,
    /// The receipt itself.
    pub receipt: WorkflowReceipt,
}

/// The ONE receipt reader. `read_workflow_receipt` adds only the requested-id cross-check;
/// `impl Deserialize for WorkflowReceipt` routes here too, so an embedded receipt cannot be
/// admitted by a laxer path than a file one (`identity/result_name.rs`'s discipline).
impl<'de> serde::Deserialize<'de> for WorkflowReceipt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        parse_workflow_receipt(&value, "<embedded>").map_err(serde::de::Error::custom)
    }
}

/// Why a receipt read failed — pi `readWorkflowReceipt`'s three throws
/// (`workflow-receipt.ts:302-308`).
///
/// `MayStillBeActive` is a VARIANT rather than a message prefix because upstream's own consumer
/// tests for it with `String.startsWith` (`runs/foreground/subagent-executor.ts:4355-4358`) to
/// decide whether to append a direct-resume hint. A `String` error here would make that consumer a
/// substring match on a product surface — SCOPE_3 §A.1 row 3.
#[derive(Debug, thiserror::Error)]
pub enum WorkflowReceiptError {
    /// The file is absent but `status.json` or `events.jsonl` is present (`:304`).
    #[error(
        "Workflow receipt '{workflow_run_id}' is not available because the workflow may still be \
         active or terminal receipt writing failed. Use direct child run IDs from status/events \
         for direct resume after the normal retained-child checks."
    )]
    MayStillBeActive {
        /// The requested run id, interpolated exactly as upstream does.
        workflow_run_id: String,
    },
    /// The file is absent and the run directory shows no sign of the run (`:306`).
    #[error("Workflow receipt '{workflow_run_id}' was not found.")]
    NotFound {
        /// The requested run id.
        workflow_run_id: String,
    },
    /// Any other I/O fault (`:308`).
    #[error("Workflow receipt '{workflow_run_id}' could not be read: {source}")]
    Unreadable {
        /// The requested run id.
        workflow_run_id: String,
        /// The underlying fault — `{ cause: error }` upstream, `#[source]` here.
        #[source]
        source: std::io::Error,
    },
    /// A structural rejection — the `Invalid workflow receipt '<source>': …` and
    /// `Workflow receipt '<source>' is stale: …` families, message verbatim.
    #[error("{0}")]
    Invalid(String),
}

/// pi `workflowReceiptPath(asyncDirRoot, workflowRunId)` (`:32-34`) — **infallible**: upstream's
/// `assertSafeRunId` throw moves to the boundary that produces `run` (§0.9).
#[must_use]
pub fn workflow_receipt_path(async_root: &Path, run: &RunDirName) -> PathBuf {
    run.resolve_in(async_root).join(WORKFLOW_RECEIPT_FILE)
}

/// The input to [`build_workflow_receipt`] — pi `buildWorkflowReceipt`'s single argument
/// (`workflow-receipt.ts:36-44`).
pub struct BuildWorkflowReceipt<'a> {
    /// This receipt's run — infallible, already a validated [`RunDirName`].
    pub workflow_run_id: &'a RunDirName,
    /// The receipt's terminal state.
    pub state: WorkflowReceiptState,
    /// Every settled child, in launch order. Every field this builder reads off a child already
    /// exists on [`WorkflowScriptChildResult`] — nothing is synthesized.
    pub children: &'a [WorkflowScriptChildResult],
    /// The workflow's host steps, when any (empty ⇒ omitted from the wire, rule 13).
    pub host_steps: &'a [HostStepNode],
    /// The final child inventory, when the caller has one to attach.
    pub workflow_children: Option<WorkflowChildSummary>,
    /// The resource this workflow resolved from, when any.
    pub resource: Option<WorkflowResourceProvenance>,
    /// A partial-terminal outcome, when the run ended that way.
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// Overrides the crate's own clock; `None` uses [`crate::time::now_epoch_millis`].
    pub created_at: Option<i64>,
}

/// pi `buildWorkflowReceipt` (`workflow-receipt.ts:36-80`) — builds the record; writes nothing.
///
/// # Errors
///
/// [`WorkflowReceiptError::Invalid`] for a malformed child (a bad or duplicate key, a resumable
/// child with no retained run id), a `workflowChildren` whose run id disagrees with this receipt's,
/// too many host steps, or a duplicate host-step id.
pub fn build_workflow_receipt(
    input: BuildWorkflowReceipt<'_>,
) -> Result<WorkflowReceipt, WorkflowReceiptError> {
    // Rule 1 — structural: `input.workflow_run_id` is already a validated `&RunDirName`.
    let workflow_run_id_str = input.workflow_run_id.as_str();
    let workflow_run_id = Bounded::<256>::parse(workflow_run_id_str).ok_or_else(|| {
        WorkflowReceiptError::Invalid(
            "workflowRunId must be an exact workflow run id, not a path or prefix.".to_string(),
        )
    })?;
    // Rule 2.
    if let Some(children) = &input.workflow_children
        && children.workflow_run_id.as_str() != workflow_run_id_str
    {
        return Err(WorkflowReceiptError::Invalid(
            "workflowChildren workflowRunId does not match its receipt.".to_string(),
        ));
    }
    let mut entries: Vec<WorkflowReceiptEntry> = Vec::with_capacity(input.children.len());
    for child in input.children {
        // Rule 3.
        let key = WorkflowKey::parse(&child.key).map_err(|_| {
            WorkflowReceiptError::Invalid("workflow receipt child key is invalid.".to_string())
        })?;
        // Rule 4.
        if entries.iter().any(|entry| entry.key == key) {
            return Err(WorkflowReceiptError::Invalid(format!(
                "Workflow receipt has duplicate child key '{}'.",
                key.as_str()
            )));
        }
        // Rule 5 — the grammar/bound checks are already structural on
        // `WorkflowLaneMetadata::key` (SUBTASK0a's `types.rs` change); only the cross-key check
        // is a genuine runtime rule left to perform here.
        let lane_label = format!("workflow receipt child '{}'.lane", key.as_str());
        assert_workflow_lane_key(child.lane.as_ref(), Some(&key), &lane_label)
            .map_err(|e| WorkflowReceiptError::Invalid(e.to_string()))?;
        // Rule 6 — run-id lineage: continuation.run_ids ?? (run_id ? [run_id] : []), non-blank
        // trim filter, de-dup preserving first-seen order, latest = last.
        let raw_run_ids: Vec<&str> = match &child.continuation {
            Some(continuation) => continuation.run_ids.iter().map(String::as_str).collect(),
            None => child.run_id.as_deref().into_iter().collect(),
        };
        let mut run_ids: Vec<Bounded<256>> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for raw in raw_run_ids {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(bounded) = Bounded::<256>::parse(trimmed) else {
                continue;
            };
            if seen.insert(bounded.as_str().to_string()) {
                run_ids.push(bounded);
            }
        }
        let latest_run_id = run_ids.last().cloned();
        // Rule 7 — the two default reasons are verbatim strings.
        let resumability = child.resumability.clone().unwrap_or_else(|| {
            if child.run_id.is_some() {
                WorkflowResumability::NotResumable {
                    reason: "resumability was not recorded".to_string(),
                }
            } else {
                WorkflowResumability::NotResumable {
                    reason: "child produced no run id".to_string(),
                }
            }
        });
        // Rule 8.
        let resume = WorkflowReceiptResume::from_parts(resumability, latest_run_id, &key)?;
        // Rule 9 — the eight omit-when-absent spreads. A bad/over-long optional field is DROPPED
        // here, not an error — the same "drop, don't reject" rule `workflow_child_summary`'s own
        // doc states for build-side inputs.
        entries.push(WorkflowReceiptEntry {
            key: key.clone(),
            lane: child.lane.clone(),
            terminal_outcome: child.terminal_outcome,
            agent: child.agent.as_deref().and_then(Bounded::parse),
            requested_context: child.requested_context,
            resolved_context: child.resolved_context,
            output_reference: child.output_reference.clone(),
            acceptance_recovery: child.recovery.clone(),
            external_adapter: child.external_adapter.clone(),
            continuation: WorkflowContinuation {
                run_ids: run_ids.iter().map(|b| b.as_str().to_string()).collect(),
            },
            resume,
        });
    }
    // Rule 10.
    if input.host_steps.len() > HOST_STEP_MAX_COUNT {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Workflow receipt has more than {HOST_STEP_MAX_COUNT} host steps."
        )));
    }
    // Rule 11.
    assert_unique_host_step_ids(input.host_steps, "workflow receipt")
        .map_err(WorkflowReceiptError::Invalid)?;
    // Rule 12.
    let created_at = serde_json::Number::from(
        input.created_at.unwrap_or_else(crate::time::now_epoch_millis),
    );
    Ok(WorkflowReceipt {
        version: WorkflowReceiptVersion,
        workflow_run_id,
        state: input.state,
        created_at,
        entries,
        resource: input.resource,
        // Rule 13 — an empty array is omitted at serialize time via `skip_serializing_if`.
        host_steps: input.host_steps.to_vec(),
        workflow_children: input.workflow_children,
        workflow_resolution: None,
        terminal_outcome: input.terminal_outcome,
        recovery: None,
    })
}

/// pi `writeWorkflowReceipt(asyncDir, receipt)` (`:81-85`) — through
/// [`write_private_atomic_json_blocking`] (§0.7): the `0600`, parent-creating, blocking sibling of
/// the crate's public-mode atomic writer. `run_dir` is the ALREADY-RESOLVED run directory (e.g.
/// `run.resolve_in(async_root)`), never re-resolved here.
///
/// # Errors
///
/// An `io::Error` if serialization fails or the write/rename does not succeed.
pub fn write_workflow_receipt(run_dir: &Path, receipt: &WorkflowReceipt) -> io::Result<PathBuf> {
    let path = run_dir.join(WORKFLOW_RECEIPT_FILE);
    write_private_atomic_json_blocking(&path, receipt)?;
    Ok(path)
}

/// Reproduces pi `readWorkflowReceiptFile`'s three guards (`:87-107`), in order: the path must
/// name a regular file, its size must not exceed [`MAX_WORKFLOW_RECEIPT_BYTES`] (checked from
/// metadata before any allocation), and the read must not end early.
fn read_workflow_receipt_file(path: &Path) -> io::Result<Value> {
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workflow receipt is not a regular file.",
        ));
    }
    if metadata.len() > MAX_WORKFLOW_RECEIPT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workflow receipt exceeds the {MAX_WORKFLOW_RECEIPT_BYTES}-byte limit."),
        ));
    }
    let declared_len = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let mut buffer = Vec::with_capacity(declared_len);
    let read = file
        .by_ref()
        .take(metadata.len())
        .read_to_end(&mut buffer)?;
    if u64::try_from(read).unwrap_or(u64::MAX) != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "workflow receipt ended before its declared size.",
        ));
    }
    serde_json::from_slice(&buffer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// pi `readWorkflowReceipt` (`workflow-receipt.ts:297-338`).
///
/// **Returns `Result`, never `Option`.** An absent receipt for a workflow that recorded a path is
/// a FAULT, not an empty result — and the caller acts on WHICH fault: see
/// [`WorkflowReceiptError::MayStillBeActive`] (§0.6), whose variant identity replaces upstream's
/// `String.startsWith` test.
///
/// # Errors
///
/// [`WorkflowReceiptError`]: `MayStillBeActive`/`NotFound` for an absent file (branching on
/// `<run_dir>/status.json` or `<run_dir>/events.jsonl`), `Unreadable` for any other I/O fault,
/// `Invalid` for every structural rejection (including a `workflowRunId` mismatch, which — unlike
/// the generic hand-walk in [`parse_workflow_receipt`] — this function alone can check, since only
/// it knows the REQUESTED id).
pub fn read_workflow_receipt(
    async_root: &Path,
    run: &RunDirName,
) -> Result<WorkflowReceipt, WorkflowReceiptError> {
    let run_dir = run.resolve_in(async_root);
    let receipt_path = run_dir.join(WORKFLOW_RECEIPT_FILE);
    match read_workflow_receipt_file(&receipt_path) {
        Ok(value) => {
            let receipt =
                parse_workflow_receipt(&value, &receipt_path.display().to_string())?;
            if receipt.workflow_run_id.as_str() != run.as_str() {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Workflow receipt '{}' is stale: workflowRunId does not match.",
                    receipt_path.display()
                )));
            }
            Ok(receipt)
        }
        Err(error) => {
            if errno::is_absent(&error) {
                // The two run-directory probes reuse `RunDir`'s own path builders — do not
                // re-spell either file-name literal (§0.9).
                let dir = RunDir::for_existing(&run_dir);
                if dir.status().exists() || dir.events().exists() {
                    Err(WorkflowReceiptError::MayStillBeActive {
                        workflow_run_id: run.as_str().to_string(),
                    })
                } else {
                    Err(WorkflowReceiptError::NotFound {
                        workflow_run_id: run.as_str().to_string(),
                    })
                }
            } else {
                Err(WorkflowReceiptError::Unreadable {
                    workflow_run_id: run.as_str().to_string(),
                    source: error,
                })
            }
        }
    }
}

fn parse_terminal_outcome(
    value: &Value,
    label: &str,
) -> Result<WorkflowTerminalOutcome, WorkflowReceiptError> {
    let Some(map) = value.as_object() else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "{label} must be an object."
        )));
    };
    if map.get("state").and_then(Value::as_str) != Some("partial") {
        return Err(WorkflowReceiptError::Invalid(format!("{label} is invalid.")));
    }
    let reason = match map.get("reason").and_then(Value::as_str) {
        Some("budget_exhausted") => WorkflowTerminalOutcomeReason::BudgetExhausted,
        Some("timeout") => WorkflowTerminalOutcomeReason::Timeout,
        _ => return Err(WorkflowReceiptError::Invalid(format!("{label} is invalid."))),
    };
    Ok(WorkflowTerminalOutcome::Partial { reason })
}

fn parse_acceptance_recovery_metadata(
    value: &Value,
    label: &str,
) -> Result<AcceptanceRecoveryMetadata, WorkflowReceiptError> {
    let invalid = || WorkflowReceiptError::Invalid(format!("{label} is invalid."));
    let Some(map) = value.as_object() else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "{label} must be an object."
        )));
    };
    if map.get("status").and_then(Value::as_str) != Some("available-for-review")
        || map.get("reason").and_then(Value::as_str) != Some("acceptance-metadata-rejected")
    {
        return Err(invalid());
    }
    let Some(report_path) = map
        .get("reportPath")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return Err(invalid());
    };
    let Some(report_hash) = map.get("reportHash").and_then(Value::as_str) else {
        return Err(invalid());
    };
    let hash_ok =
        report_hash.len() == 64 && report_hash.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if !hash_ok {
        return Err(invalid());
    }
    Ok(AcceptanceRecoveryMetadata {
        status: "available-for-review".to_string(),
        reason: "acceptance-metadata-rejected".to_string(),
        report_path: report_path.to_string(),
        report_hash: report_hash.to_string(),
    })
}

fn parse_workflow_resolution(
    value: &Value,
    source: &str,
) -> Result<WorkflowTerminalResolution, WorkflowReceiptError> {
    match value.as_str() {
        Some("settled-awaiting-resume") => Ok(WorkflowTerminalResolution::SettledAwaitingResume),
        Some("failed-child") => Ok(WorkflowTerminalResolution::FailedChild),
        Some("interrupted-child") => Ok(WorkflowTerminalResolution::InterruptedChild),
        _ => Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': workflowResolution is invalid."
        ))),
    }
}

/// pi `parseWorkflowResource` (`workflow-receipt.ts:255-268`), collapsed to a `serde_json`
/// deserialize plus upstream's ONE message: `WorkflowResourceProvenance`'s own field types
/// ([`WorkflowKey`] for `name`, [`WorkflowResourceVersion`](super::permit::WorkflowResourceVersion)
/// for `version`, [`WorkflowResourceId`](super::permit::WorkflowResourceId) for `id`) now enforce
/// all three of upstream's rules structurally (§0.10), and — like
/// upstream — unknown fields on the input are silently dropped, not rejected (no
/// `deny_unknown_fields` on that struct).
fn parse_workflow_resource(
    value: &Value,
    source: &str,
) -> Result<WorkflowResourceProvenance, WorkflowReceiptError> {
    serde_json::from_value(value.clone()).map_err(|_| {
        WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': resource is invalid."
        ))
    })
}

fn parse_entry(
    value: &Value,
    key: &WorkflowKey,
    source: &str,
) -> Result<WorkflowReceiptEntry, WorkflowReceiptError> {
    let label = format!("entry '{}'", key.as_str());
    let Some(map) = value.as_object() else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': {label} must be an object."
        )));
    };
    if map.get("key").and_then(Value::as_str) != Some(key.as_str()) {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': {label} has a mismatched key."
        )));
    }
    // latestRunId — optional; present-but-blank/non-string is rejected.
    let latest_run_id = match map.get("latestRunId") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let Some(bounded) = value.as_str().and_then(Bounded::<256>::parse) else {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': {label} latestRunId must be non-empty."
                )));
            };
            Some(bounded)
        }
    };
    // continuation — required object with a runIds array of non-blank strings.
    let Some(run_ids_raw) = map
        .get("continuation")
        .and_then(Value::as_object)
        .and_then(|c| c.get("runIds"))
        .and_then(Value::as_array)
    else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': {label} continuation is missing."
        )));
    };
    let mut run_ids: Vec<Bounded<256>> = Vec::with_capacity(run_ids_raw.len());
    for run_id_value in run_ids_raw {
        let Some(bounded) = run_id_value.as_str().and_then(Bounded::<256>::parse) else {
            return Err(WorkflowReceiptError::Invalid(format!(
                "Invalid workflow receipt '{source}': {label} continuation contains an invalid \
                 run id."
            )));
        };
        run_ids.push(bounded);
    }
    if let Some(latest) = &latest_run_id
        && run_ids.last().map(Bounded::as_str) != Some(latest.as_str())
    {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Workflow receipt '{source}' entry '{}' is stale: latestRunId does not match its \
             continuation lineage.",
            key.as_str()
        )));
    }
    // resumability — required object with a `state` word and a conditional `reason`.
    let Some(resumability_map) = map.get("resumability").and_then(Value::as_object) else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': {label} resumability is missing."
        )));
    };
    let resume = match resumability_map.get("state").and_then(Value::as_str) {
        Some("resumable") => {
            let Some(latest_run_id) = latest_run_id.clone() else {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': {label} resumable entry has no \
                     retained run id."
                )));
            };
            WorkflowReceiptResume::Resumable { latest_run_id }
        }
        Some("not-resumable") => {
            let reason = resumability_map
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty());
            let Some(reason) = reason else {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': {label} non-resumable reason is \
                     missing."
                )));
            };
            WorkflowReceiptResume::NotResumable {
                latest_run_id,
                reason: reason.to_string(),
            }
        }
        _ => {
            return Err(WorkflowReceiptError::Invalid(format!(
                "Invalid workflow receipt '{source}': {label} resumability state is invalid."
            )));
        }
    };
    let terminal_outcome = match map.get("terminalOutcome") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_terminal_outcome(
            value,
            &format!("Invalid workflow receipt '{source}': {label} terminalOutcome"),
        )?),
    };
    let acceptance_recovery = match map.get("acceptanceRecovery") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_acceptance_recovery_metadata(
            value,
            &format!("Invalid workflow receipt '{source}': {label} acceptanceRecovery"),
        )?),
    };
    // externalAdapter — carried opaquely; its validator has no cyrup producer (§0.3).
    let external_adapter = map.get("externalAdapter").filter(|v| !v.is_null()).cloned();
    let lane_label = format!("Invalid workflow receipt '{source}': {label}.lane");
    let lane = normalize_workflow_lane_metadata(map.get("lane"), &lane_label)
        .map_err(|e| WorkflowReceiptError::Invalid(e.to_string()))?;
    assert_workflow_lane_key(lane.as_ref(), Some(key), &lane_label)
        .map_err(|e| WorkflowReceiptError::Invalid(e.to_string()))?;
    // The remaining "trust-cast" fields: upstream never validates them (a bare TS `as` cast,
    // `:296`), so a shape that does not match the expected type is treated as absent, never an
    // error.
    let agent = map.get("agent").and_then(Value::as_str).and_then(Bounded::parse);
    let requested_context = match map.get("requestedContext").and_then(Value::as_str) {
        Some("fresh") => Some(WorkflowRequestedContext::Fresh),
        Some("fork") => Some(WorkflowRequestedContext::Fork),
        _ => None,
    };
    let resolved_context = match map.get("resolvedContext").and_then(Value::as_str) {
        Some("fresh") => Some(WorkflowResolvedContext::Fresh),
        Some("fork") => Some(WorkflowResolvedContext::Fork),
        Some("mixed") => Some(WorkflowResolvedContext::Mixed),
        _ => None,
    };
    let output_reference = map
        .get("outputReference")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(WorkflowReceiptEntry {
        key: key.clone(),
        lane,
        terminal_outcome,
        agent,
        requested_context,
        resolved_context,
        output_reference,
        acceptance_recovery,
        external_adapter,
        continuation: WorkflowContinuation {
            run_ids: run_ids.iter().map(|b| b.as_str().to_string()).collect(),
        },
        resume,
    })
}

fn parse_recovery(
    value: &Value,
    workflow_run_id: &Bounded<256>,
    entries: &[WorkflowReceiptEntry],
    source: &str,
) -> Result<Vec<WorkflowRecoveryAction>, WorkflowReceiptError> {
    let Some(array) = value.as_array() else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': recovery must be an array."
        )));
    };
    array
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let invalid = || {
                WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': recovery[{index}] is invalid."
                ))
            };
            let Some(map) = item.as_object() else {
                return Err(invalid());
            };
            let Some(key_str) = map.get("key").and_then(Value::as_str) else {
                return Err(invalid());
            };
            let Some(entry) = entries.iter().find(|e| e.key.as_str() == key_str) else {
                return Err(invalid());
            };
            if map.get("call").and_then(Value::as_str) != Some("runs.run")
                || map.get("taskRequired").and_then(Value::as_bool) != Some(true)
            {
                return Err(invalid());
            }
            let Some(resume_map) = map.get("resume").and_then(Value::as_object) else {
                return Err(invalid());
            };
            if resume_map.get("workflowRunId").and_then(Value::as_str)
                != Some(workflow_run_id.as_str())
                || resume_map.get("key").and_then(Value::as_str) != Some(key_str)
                || resume_map.get("latest").and_then(Value::as_bool) != Some(true)
                || !entry.resume.is_resumable()
            {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': recovery[{index}] does not identify a \
                     resumable entry."
                )));
            }
            Ok(WorkflowRecoveryAction {
                key: entry.key.clone(),
                call: WorkflowRecoveryCall::RunsRun,
                resume: WorkflowRecoveryResume {
                    workflow_run_id: workflow_run_id.clone(),
                    key: entry.key.clone(),
                    latest: true,
                },
                task_required: true,
            })
        })
        .collect()
}

/// The ONE receipt reader. Shaped exactly like
/// [`super::child_summary::parse_workflow_child_summary`]: a hand walk over [`serde_json::Value`],
/// rejections in upstream's source order with upstream's messages verbatim.
///
/// Does NOT check `workflowRunId` against an external expectation — that cross-check needs the
/// REQUESTED id, which only [`read_workflow_receipt`] (the file reader) has; this function is also
/// the route [`WorkflowReceipt`]'s `Deserialize` impl takes for an EMBEDDED receipt, which has no
/// such external id to compare against.
///
/// # Errors
///
/// [`WorkflowReceiptError::Invalid`] for every structural rejection, in `readWorkflowReceipt`'s
/// source order: not an object; unsupported version; workflow not terminal; `createdAt` invalid;
/// `entries` not an object; per-entry; `workflowChildren.workflowRunId` mismatch; `hostSteps` not
/// an array / over the count ceiling / a duplicate id; `workflowResolution`/`resource`/
/// `terminalOutcome`/`recovery` each invalid.
fn parse_workflow_receipt(
    value: &Value,
    source: &str,
) -> Result<WorkflowReceipt, WorkflowReceiptError> {
    let Some(map) = value.as_object() else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': expected an object."
        )));
    };
    let version_ok = map
        .get("version")
        .and_then(Value::as_f64)
        .is_some_and(|version| version == f64::from(WORKFLOW_RECEIPT_VERSION));
    if !version_ok {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': unsupported version."
        )));
    }
    let Some(workflow_run_id) = map
        .get("workflowRunId")
        .and_then(Value::as_str)
        .and_then(Bounded::<256>::parse)
    else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': workflowRunId is invalid."
        )));
    };
    let state = match map.get("state").and_then(Value::as_str) {
        Some("complete") => WorkflowReceiptState::Complete,
        Some("failed") => WorkflowReceiptState::Failed,
        Some("paused") => WorkflowReceiptState::Paused,
        Some("stopped") => WorkflowReceiptState::Stopped,
        _ => {
            return Err(WorkflowReceiptError::Invalid(format!(
                "Workflow receipt '{source}' is stale: workflow is not terminal."
            )));
        }
    };
    let Some(created_at) = map
        .get("createdAt")
        .and_then(Value::as_number)
        .filter(|n| n.as_f64().is_some_and(f64::is_finite))
        .cloned()
    else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': createdAt is invalid."
        )));
    };
    let Some(entries_map) = map.get("entries").and_then(Value::as_object) else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Invalid workflow receipt '{source}': entries must be an object."
        )));
    };
    let mut entries: Vec<WorkflowReceiptEntry> = Vec::with_capacity(entries_map.len());
    for (key, entry_value) in entries_map {
        let parsed_key = WorkflowKey::parse(key).map_err(|_| {
            WorkflowReceiptError::Invalid(format!(
                "Invalid workflow receipt '{source}': entry key is invalid."
            ))
        })?;
        entries.push(parse_entry(entry_value, &parsed_key, source)?);
    }
    let workflow_children = parse_workflow_child_summary(map.get("workflowChildren"))
        .map_err(|e| WorkflowReceiptError::Invalid(e.to_string()))?;
    if let Some(children) = &workflow_children
        && children.workflow_run_id.as_str() != workflow_run_id.as_str()
    {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Workflow receipt '{source}' is stale: workflowChildren.workflowRunId does not \
             match."
        )));
    }
    let host_steps = match map.get("hostSteps") {
        None => Vec::new(),
        Some(value) => {
            let Some(array) = value.as_array() else {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': hostSteps must be an array."
                )));
            };
            if array.len() > HOST_STEP_MAX_COUNT {
                return Err(WorkflowReceiptError::Invalid(format!(
                    "Invalid workflow receipt '{source}': hostSteps exceeds \
                     {HOST_STEP_MAX_COUNT} entries."
                )));
            }
            let mut parsed = Vec::with_capacity(array.len());
            for node in array {
                let host_step: HostStepNode =
                    serde_json::from_value(node.clone()).map_err(|e| {
                        WorkflowReceiptError::Invalid(format!(
                            "Invalid workflow receipt '{source}': hostSteps entry is invalid: {e}"
                        ))
                    })?;
                parsed.push(host_step);
            }
            assert_unique_host_step_ids(&parsed, source).map_err(WorkflowReceiptError::Invalid)?;
            parsed
        }
    };
    let workflow_resolution = match map.get("workflowResolution") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_workflow_resolution(value, source)?),
    };
    let resource = match map.get("resource") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_workflow_resource(value, source)?),
    };
    let terminal_outcome = match map.get("terminalOutcome") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_terminal_outcome(
            value,
            &format!("Invalid workflow receipt '{source}': terminalOutcome"),
        )?),
    };
    let recovery = match map.get("recovery") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_recovery(value, &workflow_run_id, &entries, source)?),
    };
    Ok(WorkflowReceipt {
        version: WorkflowReceiptVersion,
        workflow_run_id,
        state,
        created_at,
        entries,
        resource,
        host_steps,
        workflow_children,
        workflow_resolution,
        terminal_outcome,
        recovery,
    })
}

/// A receipt entry proven resumable — pi's
/// `WorkflowReceiptEntry & { latestRunId: string; resumability: { state: "resumable" } }`
/// (`workflow-receipt.ts:340`), whose two `asserts entry is …` throws (`:349-352`) are structural
/// here. Owns its data (rather than borrowing) because [`resolve_workflow_receipt_resume_entry`]
/// reads the receipt it comes from as a local value.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumableEntry {
    entry: WorkflowReceiptEntry,
    latest_run_id: Bounded<256>,
}

impl ResumableEntry {
    /// The resumable entry itself.
    #[must_use]
    pub fn entry(&self) -> &WorkflowReceiptEntry {
        &self.entry
    }

    /// The entry's retained run id — always present on this proven-resumable view.
    #[must_use]
    pub fn latest_run_id(&self) -> &Bounded<256> {
        &self.latest_run_id
    }
}

/// The input to [`resolve_workflow_receipt_resume_entry`]/[`resolve_workflow_receipt_resume`] —
/// pi `resolveWorkflowReceiptResumeEntry`'s single argument (`workflow-receipt.ts:339-343`).
pub struct ResolveWorkflowReceiptResume<'a> {
    /// The keyed resume reference a workflow script's `runs.resolveResume(...)` argument named.
    pub reference: &'a WorkflowReceiptResumeReference,
    /// The async root the receipt's run directory lives under.
    pub async_root: &'a Path,
    /// An injected business check — e.g. "the child is not still running" — pi's
    /// `assertResumable` callback (`:343`, `:347`): an expected rejection, so `Result`, not a
    /// panic.
    pub assert_resumable: Option<&'a AssertResumableCheck>,
}

/// The callback shape [`ResolveWorkflowReceiptResume::assert_resumable`] carries — factored into
/// its own alias per clippy's `type_complexity`.
pub type AssertResumableCheck = dyn Fn(&str) -> Result<(), String>;

/// pi `resolveWorkflowReceiptResumeEntry` (`workflow-receipt.ts:339-358`).
///
/// # Errors
///
/// [`WorkflowReceiptError::Invalid`] when `latest` is not `true`, the key is malformed, the
/// receipt's run id is malformed, the receipt has no such child key, the child is not resumable,
/// or the caller's own `assert_resumable` check rejects it. Propagates
/// [`read_workflow_receipt`]'s own errors otherwise.
pub fn resolve_workflow_receipt_resume_entry(
    input: ResolveWorkflowReceiptResume<'_>,
) -> Result<ResumableEntry, WorkflowReceiptError> {
    if !input.reference.latest {
        return Err(WorkflowReceiptError::Invalid(
            "Keyed workflow receipt resume requires latest: true.".to_string(),
        ));
    }
    let key = WorkflowKey::parse(&input.reference.key).map_err(|_| {
        WorkflowReceiptError::Invalid("keyed resume key is invalid.".to_string())
    })?;
    let trimmed_run_id = input.reference.workflow_run_id.trim();
    let Some(run_dir_name) = RunDirName::parse(trimmed_run_id) else {
        return Err(WorkflowReceiptError::Invalid(
            "workflowRunId must be an exact workflow run id, not a path or prefix.".to_string(),
        ));
    };
    let receipt = read_workflow_receipt(input.async_root, &run_dir_name)?;
    let Some(entry) = receipt.entry(&key).cloned() else {
        return Err(WorkflowReceiptError::Invalid(format!(
            "Workflow receipt '{}' has no child key '{}'.",
            receipt.workflow_run_id.as_str(),
            key.as_str()
        )));
    };
    let latest_run_id = match &entry.resume {
        WorkflowReceiptResume::Resumable { latest_run_id } => latest_run_id.clone(),
        WorkflowReceiptResume::NotResumable { reason, .. } => {
            return Err(WorkflowReceiptError::Invalid(format!(
                "Workflow receipt '{}' child '{}' is not resumable: {reason}.",
                receipt.workflow_run_id.as_str(),
                key.as_str()
            )));
        }
    };
    if let Some(assert_resumable) = input.assert_resumable {
        assert_resumable(latest_run_id.as_str()).map_err(WorkflowReceiptError::Invalid)?;
    }
    Ok(ResumableEntry {
        entry,
        latest_run_id,
    })
}

/// pi `resolveWorkflowReceiptResume` (`workflow-receipt.ts:359-366`).
///
/// # Errors
///
/// See [`resolve_workflow_receipt_resume_entry`].
pub fn resolve_workflow_receipt_resume(
    input: ResolveWorkflowReceiptResume<'_>,
) -> Result<Bounded<256>, WorkflowReceiptError> {
    Ok(resolve_workflow_receipt_resume_entry(input)?
        .latest_run_id()
        .clone())
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
    use crate::background::RunId;

    fn key(raw: &str) -> WorkflowKey {
        WorkflowKey::parse(raw).expect("valid key")
    }

    fn run_dir_name(raw: &str) -> RunDirName {
        RunDirName::parse(raw).expect("valid run dir name")
    }

    fn child(key: &str) -> WorkflowScriptChildResult {
        WorkflowScriptChildResult {
            key: key.to_string(),
            ok: true,
            run_id: Some(format!("{key}-run")),
            ..WorkflowScriptChildResult::default()
        }
    }

    #[test]
    fn build_workflow_receipt_produces_a_resumable_entry_from_a_bare_child() {
        let run = run_dir_name("run-1");
        let receipt = build_workflow_receipt(BuildWorkflowReceipt {
            workflow_run_id: &run,
            state: WorkflowReceiptState::Complete,
            children: &[child("a")],
            host_steps: &[],
            workflow_children: None,
            resource: None,
            terminal_outcome: None,
            created_at: Some(1_000),
        })
        .expect("builds");
        assert_eq!(receipt.workflow_run_id.as_str(), "run-1");
        assert_eq!(receipt.entries.len(), 1);
        let entry = receipt.entry(&key("a")).expect("entry present");
        assert_eq!(
            entry.resume,
            WorkflowReceiptResume::NotResumable {
                latest_run_id: Some(Bounded::parse("a-run").expect("fits")),
                reason: "resumability was not recorded".to_string(),
            }
        );
    }

    #[test]
    fn build_workflow_receipt_rejects_a_duplicate_child_key() {
        let run = run_dir_name("run-1");
        let err = build_workflow_receipt(BuildWorkflowReceipt {
            workflow_run_id: &run,
            state: WorkflowReceiptState::Complete,
            children: &[child("a"), child("a")],
            host_steps: &[],
            workflow_children: None,
            resource: None,
            terminal_outcome: None,
            created_at: Some(1_000),
        })
        .expect_err("duplicate key rejected");
        assert_eq!(
            err.to_string(),
            "Workflow receipt has duplicate child key 'a'."
        );
    }

    #[test]
    fn build_workflow_receipt_rejects_a_resumable_child_with_no_run_id() {
        let run = run_dir_name("run-1");
        let mut resumable_but_bare = child("a");
        resumable_but_bare.run_id = None;
        resumable_but_bare.resumability = Some(WorkflowResumability::Resumable);
        let err = build_workflow_receipt(BuildWorkflowReceipt {
            workflow_run_id: &run,
            state: WorkflowReceiptState::Complete,
            children: &[resumable_but_bare],
            host_steps: &[],
            workflow_children: None,
            resource: None,
            terminal_outcome: None,
            created_at: Some(1_000),
        })
        .expect_err("resumable-but-bare rejected");
        assert_eq!(
            err.to_string(),
            "Workflow receipt child 'a' is resumable but has no retained run id."
        );
    }

    #[test]
    fn write_then_read_round_trips_a_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path();
        let run_id = RunId::new();
        let run = RunDirName::for_run(&run_id);
        let run_dir = run.resolve_in(async_root);
        std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

        let mut resumable = child("a");
        resumable.resumability = Some(WorkflowResumability::Resumable);
        let receipt = build_workflow_receipt(BuildWorkflowReceipt {
            workflow_run_id: &run,
            state: WorkflowReceiptState::Complete,
            children: &[resumable],
            host_steps: &[],
            workflow_children: None,
            resource: None,
            terminal_outcome: None,
            created_at: Some(42),
        })
        .expect("builds");

        let path = write_workflow_receipt(&run_dir, &receipt).expect("writes");
        assert_eq!(path, run_dir.join(WORKFLOW_RECEIPT_FILE));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the receipt is written owner-only");
        }

        let read_back = read_workflow_receipt(async_root, &run).expect("reads");
        assert_eq!(read_back, receipt);

        let entry = read_back.entry(&key("a")).expect("entry present");
        assert!(entry.resume.is_resumable());
    }

    #[test]
    fn read_workflow_receipt_reports_may_still_be_active_when_status_json_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path();
        let run_id = RunId::new();
        let run = RunDirName::for_run(&run_id);
        let run_dir = run.resolve_in(async_root);
        std::fs::create_dir_all(&run_dir).expect("mkdir run dir");
        std::fs::write(run_dir.join("status.json"), b"{}").expect("write status.json");

        let err = read_workflow_receipt(async_root, &run).expect_err("no receipt yet");
        assert!(matches!(err, WorkflowReceiptError::MayStillBeActive { .. }));
    }

    #[test]
    fn read_workflow_receipt_reports_not_found_when_nothing_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = RunId::new();
        let run = RunDirName::for_run(&run_id);

        let err = read_workflow_receipt(dir.path(), &run).expect_err("nothing at all exists");
        assert!(matches!(err, WorkflowReceiptError::NotFound { .. }));
    }

    #[test]
    fn workflow_recovery_action_json_shape_matches_upstream() {
        let action = WorkflowRecoveryAction {
            key: key("a"),
            call: WorkflowRecoveryCall::RunsRun,
            resume: WorkflowRecoveryResume {
                workflow_run_id: Bounded::parse("run-1").expect("fits"),
                key: key("a"),
                latest: true,
            },
            task_required: true,
        };
        assert_eq!(
            serde_json::to_value(&action).expect("serializes"),
            serde_json::json!({
                "key": "a",
                "call": "runs.run",
                "resume": { "workflowRunId": "run-1", "key": "a", "latest": true },
                "taskRequired": true,
            })
        );
    }

    #[test]
    fn receipt_state_wire_word_is_one_letter_off_workflow_state() {
        assert_eq!(
            serde_json::to_value(WorkflowReceiptState::Complete).expect("ser"),
            serde_json::json!("complete")
        );
        assert_eq!(
            serde_json::to_value(crate::workflows::WorkflowState::Completed).expect("ser"),
            serde_json::json!("completed")
        );
    }
}
