//! The background/async job system: a genuine second, detached OS-process hop. Run-id minting,
//! on-disk status/result schema, stale-run reconciliation by process-liveness probe,
//! interrupt/resume/chain-append as file-based control operations, and filesystem-watch-based
//! completion notification (func-SA §5.4; arch-SA §6.5).
//!
//! This module defines the shared, pure data types that every other file in this subsystem
//! (`atomic.rs`, and later phases' `spawn_detached.rs`/`runner_main.rs`/`control.rs`/
//! `reconcile.rs`/`tracker.rs`/`watch.rs`) reads, writes, and passes around: [`RunId`] (R-SA-072),
//! the [`RunDir`]/[`RunPaths`] filesystem layout (func-SA §4.5), [`RunStatus`]/[`RunState`]
//! (R-SA-075/077, monotone-forward transitions), [`StepStatus`]/[`StepState`], [`ResultFile`]
//! (R-SA-077/166), and [`RunMode`]. No spawn, reconciliation, or file-watch logic lives here —
//! the data model lives in the private submodules below — pure types plus the narrow, pure
//! helpers ([`RunId::new`], [`RunPaths::for_run`], [`RunState`]'s transition guard) that those
//! later files build on — re-exported here so every existing `crate::background::<Item>` path is
//! unchanged.
//!
//! # Deferred to later phases (do not implement here)
//!
//! - Detached hop-1 spawn (`process_group`/`DETACHED_PROCESS`, R-SA-070/071) — owned by
//!   `background/spawn_detached.rs`.
//! - The runner's hop-2 main loop (R-SA-073/075/077/095) — owned by `background/runner_main.rs`.
//! - Stale-run liveness reconciliation (R-SA-088..092), including the actual grace-window/
//!   staleness-threshold *decision* logic that consumes [`RunStatus::started_at`]/
//!   [`RunStatus::last_update`] — owned by `background/reconcile.rs`. This module supplies only
//!   the provisional-status *constructor* the spawn call site needs for R-SA-090 (see
//!   [`RunStatus::provisional`]); the grace-window comparison against wall-clock time at
//!   reconciliation time is `reconcile.rs`'s job, not this file's.
//! - Interrupt/resume/append-step file-based control protocol (R-SA-081..087, R-SA-094..097) —
//!   owned by `background/control.rs`.
//! - The orchestrator-side shared poller (R-SA-093/105) — owned by `background/tracker.rs`.
//! - Nested background-run storage-subpath *derivation as consumed by the runner* and root-run
//!   "am I fully done" recursive reconciliation (R-SA-104) — this module defines the
//!   [`crate::spawn::nested_events::NestedRoute`] addressing type and [`RunPaths::nested`]'s pure subpath-naming rule; the
//!   actual recursive liveness roll-up belongs to `background/reconcile.rs`/`tracker.rs`.

// SCOPE_9 — the per-SESSION active-async capacity pool and the active-run index it routes
// through. Registered here rather than under `extension/` for `terminal_run_index`'s reason: both
// are BACKGROUND-artifact writers, addressing run directories and a scratch root.
pub mod active_async_capacity;
pub mod active_run_index;
// SCOPE_10 — the bounded JSON snapshot of the current session's async runs. A BACKGROUND-artifact
// reader like `inspect_rpc` beside it: everything it projects comes from a run's reconciled
// `status.json` and the in-memory job list built from it.
// SCOPE_13 — async-root retention: the policy, the batched scan and the run tombstones. A
// BACKGROUND-artifact surface like the two index modules beside it: everything it addresses is a
// run directory under the per-`cwd` async root.
pub mod async_retention;
pub mod async_status_snapshot;
pub mod atomic;
pub mod auto_drain;
pub mod cascade;
pub mod child_identity;
pub mod child_stop;
pub mod completion_replay;
pub mod control;
pub mod delivery;
pub mod flat_index;
pub mod parent_anchor;
pub mod reconcile;
// The async recovery descriptor — the persisted launch contract of an async SINGLE run, written
// by `spawn_background_steps` before the runner exists and read back by `action: "resume"` and
// the retention scan. A BACKGROUND-artifact writer/reader like the index modules above it.
pub mod recovery_descriptor;
pub mod result_index;
// `children.list` — the retained children of a session's workflow runs, read off the workflow
// status files the async root holds, with resumability computed by the same descriptor/handoff
// readers `resume` uses. A BACKGROUND-artifact reader like `async_status_snapshot` beside it.
pub mod retained_children;
// `debug.run` — the run-lifecycle diagnostic dump over one async run's reconciled `status.json`
// and its active-capacity slot. A BACKGROUND-artifact reader like `retained_children` above it.
pub mod run_lifecycle_debug;
pub mod runner_main;
pub mod spawn_detached;
pub mod terminal_run_index;
pub mod watch;

/// SUBA-051 — pi `DEFAULT_ASYNC_TIMEOUT_MS = 30 * 60 * 1000`
/// (`runs/background/async-execution.ts:131` @v0.47.1), applied at `:782` as
/// `timeoutMs: a.defaultTimeoutMs ?? DEFAULT_ASYNC_TIMEOUT_MS` when building an async CHILD step.
/// Landed in `635c1bd` ("fix: add default async child timeouts", fixes #978), released v0.47.0.
///
/// Upstream applies this to CHILDREN only and deliberately leaves the composite PARENT
/// (`chain`/`tasks`/graph roots) unbounded — a parent's job is to outlive its children. cyrup
/// reproduces that split by applying the constant on the async SINGLE path
/// (`extension.rs::spawn_background`) and NOT inside `spawn_background_steps`, which the composite
/// entry points share. It is deliberately NOT applied to foreground runs, which already have their
/// own default.
///
/// Without it a background child that wedges — a hung `cargo test`, a non-terminating model, a
/// retry loop — burns tokens and CPU until a human notices and issues `interrupt`.
pub const DEFAULT_ASYNC_CHILD_TIMEOUT_MS: u64 = 30 * 60 * 1000;

pub mod fleet_view;
// SCOPE_12 — the inspect RPC surface, a sibling of `completion_replay`/`fleet_view` (it composes
// both). Registered here rather than under `extension/` because it is a BACKGROUND-artifact
// reader: everything it touches is a run's on-disk status, payload, replay record and archive.
pub mod inspect_rpc;
pub mod resume_guidance;
pub mod run_status;
// SUBA-016 part A — scheduled runs: the persisted schedule record, its project-local
// store, and the create-time capability-ceiling gate. A BACKGROUND-artifact surface like
// `completion_replay`/`wait_subscriptions` beside it, with one decision that deliberately
// differs from both: its store is PROJECT-local, never under the reboot-disposable run
// scratch root (see that module's own doc).
pub mod scheduled_runs;
pub mod tracker;
pub mod wait;
pub mod wait_completions;
pub mod wait_subscriptions;

// Kept here (not in a submodule): sibling modules resolve `super::ModelId` through this private
// import binding (`fleet_view.rs:472,644`), exactly as they did before the decomposition.
use cyrup_core::ModelId;

// The shared background data model, decomposed by concern (same facade pattern as
// `runner_main/mod.rs`): private modules, every item re-exported at `background::` so no
// consumer path changes.
mod artifact_roots;
mod records;
mod run_history;
mod run_id;
mod run_id_resolver;
mod run_paths;
mod state;
mod telemetry;
mod workflow_graph;

pub use artifact_roots::{
    RunArtifactRoots, active_async_capacity_root_in, active_async_capacity_session_dir,
    attempt_scratch_dir, attempt_scratch_dir_in, ensure_accessible_dir, results_dir_for_async_root,
    run_artifact_roots, run_artifact_roots_in, wait_subscriptions_dir_in,
};
pub(crate) use artifact_roots::{cwd_key, temp_root_dir, temp_root_dir_from};
pub use records::{ParallelGroupStatus, ResultFile, RunStatus, ScheduleOrigin, StepStatus};
pub use recovery_descriptor::{
    DescriptorVersion, LaunchContractDigest, LaunchInputs, ModelOrigin, RecoveryDescriptor,
    RecoveryDescriptorError,
};
pub use retained_children::{
    Resumability, RetainedChild, RetainedChildState, format_retained_children,
    list_retained_children,
};
pub use run_history::{
    RunHistoryEntry, record_run_history, run_history_path, run_history_path_for,
};
pub use run_id::RunId;
pub use run_id_resolver::{
    AsyncRunLocation, ResolveRunIdError, find_async_run_prefix_matches, resolve_async_run_dir,
    resolve_async_run_id,
};
pub use run_paths::{RunDir, RunPaths};
pub use state::{RunMode, RunState, RunStateTransitionError, StepState};
pub use telemetry::{
    ActivityState, RecentTool, RunTelemetry, StepTelemetry, TokenTotals, apply_child_event_to_step,
};
pub use workflow_graph::{
    WorkflowDynamicChild, WorkflowDynamicGroupStatus, WorkflowDynamicMeta, WorkflowDynamicStepSpec,
    WorkflowGraphBuildInput, WorkflowGraphNode, WorkflowGraphSnapshot, WorkflowInputStep,
    WorkflowNodeKind, WorkflowNodeStatus, WorkflowPhase, WorkflowResultSummary, WorkflowRunMode,
    WorkflowStepStatusInput, WorkflowTaskSpec, build_workflow_graph_snapshot,
    workflow_graph_from_run,
};
