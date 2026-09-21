//! The workflow runtime's foundations (SCOPE_3d): the family's shared types and the three leaf
//! modules of pi `src/workflows/` — the child-summary builder/reader, the chat-progress
//! projection, and the resource registry with its permit.
//!
//! A leaf module family: no I/O beyond `chat_progress`'s repository discovery, exhaustively
//! unit-testable, and consumed by the workflow gates (SCOPE_3e), the script executor (SCOPE_3f),
//! the terminal writers (SCOPE_3g) and the wait projection (SCOPE_3h).
//!
//! # Why these are types
//!
//! Upstream validates ONE identifier grammar at eleven sites across ten files and carries nine
//! bounded-string helpers, because in TypeScript a shared newtype costs more than a copied regex.
//! In Rust that trade runs the other way, and this crate had already begun paying the duplicate
//! price (two hand-rolled copies of the same grammar in `missions/`). The result was a class of
//! bug that is invisible at the type level:
//!
//! | Invariant | Was | Failure it permitted | Now |
//! |---|---|---|---|
//! | a key matches the workflow grammar | a regex/scan re-tested at each site (7 in this task's three files alone) | one forgotten `.test()` admits a traversal-shaped key | [`WorkflowKey`] |
//! | an identity field fits its bound | positional `bounded(value, maxBytes)` calls | the 32/256 limit transposition | [`Bounded`] (bytes) |
//! | a display string fits a UTF-16 bound | `.length` vs `Buffer.byteLength`, mixed per site | silently divergent units on non-ASCII | [`BoundedUtf16`] |
//! | a resolved script is extension-owned | an empty frozen object + a module-private `WeakMap` | a forged/replayed permit | [`WorkflowResourcePermit`] (private fields, one constructor, no `Clone`/`Deserialize`) |
//!
//! Each type has exactly one fallible constructor and no `From<String>`, so the invariant holds by
//! construction rather than by every caller remembering it.
//!
//! # Serde is a construction path
//!
//! Every newtype here that appears in a persisted struct deserializes **through** its validating
//! constructor rather than via a derive: on-disk data is written by other processes — including
//! older and newer builds — so deserialization is precisely where an invariant would otherwise be
//! bypassed. `Serialize` stays transparent; writing is not a trust boundary and the on-disk shape
//! must remain a bare JSON string. (This is `identity/`'s discipline, and SCOPE_3d §0.9's
//! resolution of SCOPE_3 §A.3's per-field-attribute question in its favour.)
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs            facade + the invariant table. No logic.
//! key.rs            WorkflowKey — the ONE grammar (§A.3)
//! bounded.rs        Bounded<N> / BoundedUtf16<N> — the rejecting bounded-string family (§A.4)
//! types.rs          the shared data shapes: summary/trace/child-result/preflight-lane/preflight
//! child_summary.rs  workflow_child_summary / parse_workflow_child_summary + live progress
//! chat_progress.rs  the live-card projection + row builder (git identity via gix)
//! stable_json.rs    stable_json / stable_json_digest (launch-contract.ts:9-23)
//! permit.rs         WorkflowResourcePermit + authority validation + provenance
//! resources.rs      the session-scoped resource registry, builtins, and resolution
//! display_text.rs   sanitize_display_text + §A.4's truncating family, preview included
//!                   (SCOPE_3e SUBTASK0)
//! host_step.rs      HostStepNode + its monitor/state/verdict vocabulary (SCOPE_3e SUBTASK0)
//! preflight.rs      the preflight normalizer, advisory warnings, formatters (SCOPE_3e SUBTASK1)
//! checklist.rs      the checklist projection + formatters (SCOPE_3e SUBTASK2)
//! host_command.rs   the runs.host executor — the family's one shell/write module (SCOPE_3e SUBTASK3)
//! lane_metadata.rs  normalize_workflow_lane_metadata / assert_workflow_lane_key (WORKFLOW_3 SUBTASK0a)
//!                   + ManagedWorktreeProvider / WorktreeNaming, the manifest's cleanup-task
//!                   naming evidence (lane-metadata.ts:9-11,76-96 — LANES_2)
//! receipt.rs        the durable terminal evidence record: build/write/read (WORKFLOW_3 SUBTASK1)
//! settlement.rs     how a settled workflow's terminal payload is assembled (WORKFLOW_3 SUBTASK2)
//! ```

mod bounded;
mod chat_progress;
mod checklist;
mod child_summary;
mod display_text;
mod host_command;
mod host_step;
mod key;
mod lane_metadata;
mod permit;
mod preflight;
mod receipt;
mod resources;
pub mod scripted;
mod settlement;
mod stable_json;
mod types;

pub use bounded::{Bounded, BoundedUtf16};
pub use chat_progress::{
    GitRepositoryIdentity, RepoRelation, ResolveWorkflowChatProgressInput,
    ResolvedWorkflowChatProgressMode, WorkflowChatProgressMode, WorkflowChatProgressProjection,
    WorkflowChatProgressRow, WorkflowChatProgressRowState, WorkflowPreflightLaneLookup,
    build_workflow_chat_progress_rows, is_same_git_repository, is_same_git_repository_identity,
    resolve_git_repository_identity, resolve_workflow_chat_progress,
};
pub use checklist::{
    BottleneckOptions, WorkflowChecklistAcceptance, WorkflowChecklistContext,
    WorkflowChecklistInput, WorkflowChecklistItem, WorkflowChecklistItemKind,
    WorkflowChecklistPhase, WorkflowChecklistProjection, WorkflowChecklistReview,
    WorkflowChecklistReviewResult, WorkflowChecklistState, WorkflowChecklistStep,
    WorkflowChecklistTraceEntry, WorkflowChecklistWatchdog, format_workflow_checklist_bottleneck,
    format_workflow_checklist_phase, format_workflow_checklist_summary,
    format_workflow_checklist_text, project_workflow_checklist,
};
pub use child_summary::{
    WorkflowChildLiveProgress, WorkflowChildProgressInput, WorkflowChildSummaryError,
    WorkflowChildSummaryInput, carry_step_settle_times, parse_workflow_child_summary,
    workflow_child_activity, workflow_child_progress, workflow_child_summary,
    workflow_step_statuses,
};
pub use display_text::{
    preview_display_text, sanitize_display_text, truncate_display, truncate_to_bytes,
};
pub use host_command::{
    HostCommandKind, WorkflowHostCommandParams, WorkflowHostCommandResult, WorkflowHostCommandRole,
    WorkflowHostCommandState, execute_workflow_host_command,
    normalize_workflow_host_command_params, resolve_workflow_host_output_claim_path,
};
pub use host_step::{
    HOST_STEP_MAX_COUNT, HostStepFreshness, HostStepKind, HostStepMonitorKind, HostStepNode,
    HostStepState, HostStepVerdict, HostStepVersion, assert_unique_host_step_ids,
};
pub use key::{WorkflowKey, WorkflowKeyError};
pub use lane_metadata::{
    LaneMetadataError, ManagedWorktreeProvider, WORKFLOW_LANE_CLAIM_MAX_BYTES,
    WORKFLOW_LANE_CLAIMS_MAX, WORKFLOW_LANE_KEY_MAX_BYTES, WORKFLOW_LANE_OUTPUT_PATH_MAX_BYTES,
    WORKFLOW_LANE_OUTPUT_PATHS_MAX, WORKFLOW_LANE_SOURCE_REF_MAX_BYTES,
    WORKTREE_STATUS_BRANCH_MAX_BYTES, WORKTREE_STATUS_NAMING_LABEL_MAX_BYTES,
    WORKTREE_STATUS_PATH_MAX_BYTES, WorktreeNaming, WorktreeNamingCollision,
    assert_workflow_lane_key, normalize_workflow_lane_metadata,
};
pub use permit::{
    WorkflowResourceAuthority, WorkflowResourceConsumption, WorkflowResourceExpansionState,
    WorkflowResourceHostAuthority, WorkflowResourceId, WorkflowResourceInvocation,
    WorkflowResourcePermit, WorkflowResourcePermitInput, WorkflowResourceProvenance,
    WorkflowResourceProvenanceKind, WorkflowResourceVersion,
};
pub use preflight::{
    WORKFLOW_PREFLIGHT_MAX_BYTES, WORKFLOW_PREFLIGHT_MAX_CLAIMS, WORKFLOW_PREFLIGHT_MAX_DEPTH,
    WORKFLOW_PREFLIGHT_MAX_LANES, WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH,
    WORKFLOW_PREFLIGHT_MAX_WARNINGS, WORKFLOW_PREFLIGHT_VERSION, WorkflowPreflightValidationResult,
    WorkflowTraceLike, annotate_workflow_preflight_trace, format_workflow_preflight,
    format_workflow_preflight_plan_summary, format_workflow_preflight_summary,
    format_workflow_preflight_warning_summary, format_workflow_preflight_warnings,
    normalize_workflow_preflight, validate_workflow_preflight, workflow_key_matches_preflight_lane,
    workflow_preflight_lane_for_runtime_key, workflow_preflight_warnings,
};
pub use receipt::{
    BuildWorkflowReceipt, ResolveWorkflowReceiptResume, ResumableEntry, WORKFLOW_RECEIPT_FILE,
    WORKFLOW_RECEIPT_VERSION, WorkflowReceipt, WorkflowReceiptEntry, WorkflowReceiptError,
    WorkflowReceiptRef, WorkflowReceiptResume, WorkflowReceiptState, WorkflowReceiptVersion,
    WorkflowRecoveryAction, WorkflowRecoveryCall, WorkflowRecoveryResume,
    WorkflowTerminalResolution, build_workflow_receipt, read_workflow_receipt,
    resolve_workflow_receipt_resume, resolve_workflow_receipt_resume_entry, workflow_receipt_path,
    write_workflow_receipt,
};
/// pi `normalizeWorkflowArgs` (`workflow-resources.ts:128-138`), named here as upstream names it
/// at its second call site — a persisted schedule's `target.args`
/// (`runs/background/scheduled-runs.ts:293`), read by
/// [`crate::background::scheduled_runs::parse_schedule_target`].
pub(crate) use resources::normalize_args as normalize_workflow_args;
pub use resources::{
    ResolvedWorkflowResource, WorkflowResourceDefinition, WorkflowResourceExpansion,
    WorkflowResourceRegistration, WorkflowResourceRegistry, WorkflowResourceResolution,
    WorkflowResourceResolve,
};
pub use settlement::{
    DetachedChildSettlement, EVIDENCE_PERSISTENCE_FAILED, INTERRUPTED_DETACHED_CHILD,
    PlanWorkflowSettlement, SettledWorkflowStatus, UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION,
    WorkflowBudgetSignals, WorkflowCompletionEvent, WorkflowSettlementPlan,
    apply_detached_child_settlement, classify_workflow_settlement, find_workflow_settlement_step,
    output_path_mappings_of, plan_workflow_settlement, promote_settled_paused_workflow,
    with_workflow_children, workflow_output_path_mapping_summary, workflow_recovery_actions,
    workflow_terminal_outcome_for_result,
};
pub use stable_json::{stable_json, stable_json_digest};
pub use types::{
    AcceptanceRecoveryMetadata, LaneMetadataVersion, WorkflowContinuation, WorkflowLaneMetadata,
    WorkflowLaneMode, WorkflowOutputPathMapping, WorkflowRequestedContext, WorkflowResolvedContext,
    WorkflowResumability, WorkflowTerminalOutcome, WorkflowTerminalOutcomeReason,
};
pub use types::{
    PreflightVersion, SummaryVersion, WorkflowChildActivity, WorkflowChildRow, WorkflowChildState,
    WorkflowChildSummary, WorkflowPreflight, WorkflowPreflightCoverage, WorkflowPreflightLane,
    WorkflowPreflightMode, WorkflowScriptChildResult, WorkflowScriptOperation,
    WorkflowScriptTraceEntry, WorkflowScriptTraceState, WorkflowState,
};
