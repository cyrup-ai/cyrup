//! The workflow-script runtime's public data shapes — pi `scripted-workflow.ts:997-1101`. The two
//! shared record types ([`WorkflowScriptChildResult`], [`WorkflowScriptTraceEntry`]) live in
//! [`crate::workflows::types`] (SCOPE_3d declared them; SCOPE_3f grew them additively) — this file
//! declares only what no earlier task owns. No parallel trace type exists here.

use crate::workflows::{WorkflowScriptChildResult, WorkflowScriptTraceEntry};

/// One static-validation finding — pi `WorkflowScriptValidationError`
/// (`scripted-workflow.ts:24-28`). `line`/`column` are 1-based and already adjusted for the
/// `(async () => {` wrapper line by the guest validator.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowScriptValidationError {
    /// The finding text — a re-prompt, byte-verbatim from the guest validator.
    pub message: String,
    /// 1-based source line, when the finding is located.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 1-based source column, when the finding is located.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

/// pi `WorkflowScriptValidationResult` (`scripted-workflow.ts:30-33`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowScriptValidationResult {
    /// `true` exactly when `errors` is empty.
    pub ok: bool,
    /// Deduplicated findings, in discovery order.
    pub errors: Vec<WorkflowScriptValidationError>,
}

/// One stage of a materialized `runs.lanes` plan — pi `WorkflowLanePlanStage`
/// (`scripted-workflow.ts:1041-1049`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowLanePlanStage {
    /// The stage's own key within its lane.
    pub key: String,
    /// The generated child key (`<laneKey>.<stageKey>`).
    pub generated_key: String,
    /// The requested agent, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Display-only phase label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Display-only row label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The declared `as` output name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_name: Option<String>,
    /// Whether the stage declared an `outputSchema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<bool>,
}

/// One lane of a materialized `runs.lanes` plan — pi `WorkflowLanePlan`
/// (`scripted-workflow.ts:1051-1054`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowLanePlan {
    /// The lane key.
    pub key: String,
    /// The lane's stages, in declaration order.
    pub stages: Vec<WorkflowLanePlanStage>,
}

/// `runs.steer` delivery mode — pi `WorkflowSteerOptions.mode` (`scripted-workflow.ts:1057`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSteerMode {
    /// Interrupt at the next safe point.
    Steer,
    /// Wait for the next turn boundary.
    FollowUp,
    /// Follow up mid-turn, deliver immediately between turns.
    Auto,
}

/// pi `WorkflowSteerOptions` (`scripted-workflow.ts:1056-1060`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSteerOptions {
    /// Delivery mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<WorkflowSteerMode>,
    /// Target child index for multi-child runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Acknowledgement timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_timeout_ms: Option<u64>,
}

/// The steer receipt's state word — pi `WorkflowSteerResult.state`
/// (`scripted-workflow.ts:1063`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowSteerState {
    /// Queued for delivery.
    Queued,
    /// Delivered to the child.
    Delivered,
    /// The message missed its target.
    Missed,
    /// Delivery failed.
    Failed,
}

/// One per-target delivery record on a steer receipt — pi `WorkflowSteerResult.targets[]`
/// (`scripted-workflow.ts:1066`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowSteerTarget {
    /// The child index addressed.
    pub index: u32,
    /// The per-target state word (loose upstream: any string).
    pub state: String,
    /// Why the target was not reached, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// pi `WorkflowSteerResult` (`scripted-workflow.ts:1062-1069`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSteerResult {
    /// The steered key.
    pub key: String,
    /// The receipt state.
    pub state: WorkflowSteerState,
    /// The steer request id, when assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Queued/delivered refinement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<String>,
    /// Per-target delivery records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<WorkflowSteerTarget>>,
    /// The failure text, when failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A keyed workflow-receipt resume reference — pi `WorkflowReceiptResumeReference`
/// (`scripted-workflow.ts:1071-1075`). `latest` is always `true` (the guest validator rejects
/// anything else), so the field is a marker constant on serialization.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReceiptResumeReference {
    /// The producing workflow run.
    pub workflow_run_id: String,
    /// The child key within that run's receipt.
    pub key: String,
    /// Always `true`.
    pub latest: bool,
}

/// A resolved resume reference — pi `WorkflowResolvedResumeReference`
/// (`scripted-workflow.ts:1077-1080`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowResolvedResumeReference {
    /// The retained run id to resume.
    pub run_id: String,
    /// The known lineage, oldest first, when the resolver has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_ids: Option<Vec<String>>,
}

/// A captured console line's level — pi `WorkflowScriptResult["console"][number]["level"]`
/// (`scripted-workflow.ts:1085`): exactly four variants, a domain enum rather than a string
/// (SCOPE_3 §A.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowConsoleLevel {
    /// `console.log`.
    Log,
    /// `console.info`.
    Info,
    /// `console.warn`.
    Warn,
    /// `console.error`.
    Error,
}

impl WorkflowConsoleLevel {
    /// Parse the guest's level word; anything unrecognized is dropped by the caller exactly as
    /// upstream drops it (`scripted-workflow.ts:1951-1953`).
    #[must_use]
    pub fn parse(level: &str) -> Option<Self> {
        match level {
            "log" => Some(Self::Log),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// One captured console line (`scripted-workflow.ts:1085`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowConsoleEntry {
    /// The console level.
    pub level: WorkflowConsoleLevel,
    /// The formatted line.
    pub text: String,
}

/// A completed workflow script's full result — pi `WorkflowScriptResult`
/// (`scripted-workflow.ts:1082-1088`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowScriptResult {
    /// The script's persisted return value (`null` when the script returned `undefined`).
    pub value: serde_json::Value,
    /// Every `emit(...)` value, in order.
    pub emits: Vec<serde_json::Value>,
    /// Every captured console line, in order.
    pub console: Vec<WorkflowConsoleEntry>,
    /// The host-owned execution trace.
    pub trace: Vec<WorkflowScriptTraceEntry>,
    /// Every settled child, in launch order.
    pub children: Vec<WorkflowScriptChildResult>,
}

/// The partial result a FAILED workflow still yields — pi `Omit<WorkflowScriptResult, "value">`
/// (`scripted-workflow.ts:1091`). **The partial is the point**: a failed workflow still yields
/// its trace, children, console and emits, and SCOPE_3g's settlement reads them.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowScriptPartial {
    /// Every `emit(...)` value delivered before the failure.
    pub emits: Vec<serde_json::Value>,
    /// Every captured console line.
    pub console: Vec<WorkflowConsoleEntry>,
    /// The host-owned execution trace.
    pub trace: Vec<WorkflowScriptTraceEntry>,
    /// Every child that settled before the failure, in launch order.
    pub children: Vec<WorkflowScriptChildResult>,
}

/// The workflow error kind — pi `WorkflowScriptError.errorKind`
/// (`scripted-workflow.ts:1092`): exactly `"detached-child" | "timeout"`, and completion errors
/// carry **none** (`:1844`, `:1846`) — hence `Option<WorkflowScriptErrorKind>` at the carrier,
/// never an open string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkflowScriptErrorKind {
    /// A launched child detached into its own background run.
    #[serde(rename = "detached-child")]
    DetachedChild,
    /// The workflow hit its own deadline.
    #[serde(rename = "timeout")]
    Timeout,
}

/// pi `class WorkflowScriptError` (`scripted-workflow.ts:1090-1100`) — a struct error carrying
/// the partial as a FIELD, never a bare `String`.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct WorkflowScriptError {
    /// The failure text — a re-prompt, byte-verbatim where upstream's is.
    pub message: String,
    /// Everything the run produced before failing.
    pub partial: WorkflowScriptPartial,
    /// The error kind, when the failure is a detach or the workflow deadline.
    pub error_kind: Option<WorkflowScriptErrorKind>,
}
