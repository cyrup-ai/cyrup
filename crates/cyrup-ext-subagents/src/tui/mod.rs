//! TUI rendering of live/async progress, fork-context badges, nested-fanout indentation,
//! control-notice debounce/actionability/dedup semantics, and the optional, gracefully
//! degrading "intercom" out-of-band result-delivery path (func-SA §5.5; arch-SA §6.7).
//!
//! This module (`tui/mod.rs`) owns only the **shared, renderer-agnostic data types** used across
//! the `tui/` subtree (arch-SA §3.7/§4.6): [`SubagentProgressSnapshot`], [`NestedRunSummary`],
//! [`RunSource`], [`ControlNoticeKey`], [`ControlNoticeKind`], and [`ControlNotice`]. These are
//! pure, `Clone + Debug` data — no I/O, no locking, no rendering logic. The stateful/behavioral
//! pieces that consume them live in sibling modules owned by later phases, not this file:
//!
//! - `tui/notices.rs` (later phase) — `ControlNoticeState`: the debounce-then-actionability-
//!   recheck state machine (R-SA-116), at-most-once delivery dedup (R-SA-115/122), and the
//!   async-immediate/foreground-debounced dispatch split (R-SA-117/118).
//! - `tui/render.rs` (later phase) — the fold-to-aggregate nested renderer over
//!   `&[NestedRunSummary]` (R-SA-112/113), the render-tick activity-glyph gate (R-SA-109), and the
//!   fork-badge presentational branch (R-SA-110/111).
//! - `tui/intercom.rs` (later phase) — `ClarifyRequest`, `ForkBadgeState`, `IntercomResultPayload`,
//!   and the allowlisted, timeout-bounded out-of-band result-delivery path (R-SA-119/120/123-125).
//!
//! # Relationship to `background::RunStatus`
//!
//! [`SubagentProgressSnapshot::status`] holds a full [`background::RunStatus`] (the on-disk
//! lifecycle/step/parallel-group record, func-SA §4.5) rather than re-deriving a narrower
//! TUI-only status enum: the snapshot is a renderer-facing *view* rebuilt from "the last-seen
//! NDJSON event" (func-SA §4.6) for a foreground run, or copied wholesale from a polled
//! `status.json` for a background run — in both cases the authoritative lifecycle state already
//! lives in `RunStatus`, so duplicating it here would only invite drift. `current_agent`,
//! `current_step_index`, `total_steps`, `current_tool`, `turn_count`, `tool_count`, and
//! `recent_output` are true renderer-only fields with no `RunStatus` equivalent — they come
//! directly from folding NDJSON progress events (`exec::ndjson::SubagentEvent`, a sibling
//! module).
//!
//! # `ContextMode` / fork-badge sourcing (R-SA-110/111)
//!
//! [`SubagentProgressSnapshot::context`] is [`crate::fork_context::ContextMode`] — the single
//! canonical type owned by `fork_context.rs` (arch-SA §6.6) and referenced here, never
//! re-declared. Per R-SA-111 the badge must reflect the run's *resolved* context (the output of
//! `ForkContextResolver::resolve`), not whatever the caller did or didn't request at the call
//! site — callers populating this field MUST source it from that resolution, never from the
//! caller's raw, possibly-omitted request.

/// Optional out-of-band result delivery ("intercom") and the foreground clarify/ask single-slot
/// pause primitive (R-SA-119/120/123/124/125) — see [`intercom`] for the full subsystem doc,
/// including why live clarify-dialog wiring against `LiveHostServices` is deliberately deferred
/// to a later phase rather than implemented in this crate today.
pub mod intercom;

use std::time::Instant;

use crate::background::{self, RunMode, RunStatus};
use crate::fork_context::ContextMode;

/// Pure fold-to-aggregate rendering functions (R-SA-106..113): nested/indented subagent output,
/// the fork-badge presentational helper, and activity-glyph gating. See that module's own doc for
/// why every function there is deliberately terminal-free and `cyrup-tui`-free.
pub mod render;

/// The typed, serializable render-payload shapes `cyrup-tui` consumes for the live subagent
/// surfaces (C19/C20/C21): the foreground live-progress payload streamed through the host
/// `ToolUpdateSink`, the inline subagent-result surface payload, and the persistent async-jobs
/// widget feed — plus the NDJSON-folding accumulator behind the foreground live sink. See that
/// module's own doc for the clearly-labeled remaining `cyrup-tui`-side rendering step.
pub mod events;

/// `ControlNoticeState`: the debounce/actionability/dedup state machine for control notices
/// (R-SA-114-118/121/122; arch-SA §6.7). See that module's own docs for the full design.
pub mod notices;

// =================================================================================================
// RunSource (func-SA §4.6's `SubagentRunHandle.source`; shared by every type below)
// =================================================================================================

/// Which execution path produced a given run's activity, as observed from the TUI/notices layer
/// — foreground (the parent tool call is blocked, synchronously streaming this run's NDJSON) or
/// async/background (a detached second-hop process the orchestrator polls out-of-band).
///
/// This is a purely descriptive tag over the *rendering/notification* path, distinct from
/// [`background::RunState`] (the run's own lifecycle state) and from [`RunMode`] (single/
/// parallel/chain shape) — a run can be `RunSource::Async` while its `RunMode` is `Single`, or
/// `RunSource::Foreground` while its `RunMode` is `Chain`, independently.
///
/// Drives two behavioral splits downstream (owned by `tui/notices.rs`, a later phase, not this
/// file): R-SA-116/117 (foreground control notices are debounced; async ones are delivered
/// immediately) and R-SA-118 (async notices may trigger a new orchestrator turn; foreground ones
/// never do, since the orchestrator is already mid-turn when a foreground notice fires).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunSource {
    /// The parent tool call is synchronously blocked on this run, streaming its NDJSON live.
    Foreground,
    /// This run is a detached background job, tracked and polled out-of-band (§5.4 func-SA).
    Async,
}

// =================================================================================================
// SubagentProgressSnapshot (func-SA §4.6; arch-SA §3.7)
// =================================================================================================

/// The renderable state of one subagent run, rebuilt from the last-seen NDJSON event for a
/// foreground run, or from the most recently polled `status.json` for a background run
/// (func-SA §4.6).
///
/// This type is the sole input to every rendering path in `tui/render.rs` (later phase): the
/// persistent background-progress region (R-SA-107/108), the inline foreground tool-result
/// region (R-SA-106), the activity-glyph gate (R-SA-109), and the fork badge (R-SA-110/111).
/// Constructing and keeping it up to date is the responsibility of the exec/background call
/// sites that observe live events, not this module.
#[derive(Clone, Debug)]
pub struct SubagentProgressSnapshot {
    /// This run's identity — shared with [`background::RunId`] (the same value that names the
    /// run's `RunDir` on disk for background runs, or the value minted at spawn time for
    /// foreground runs).
    pub run_id: background::RunId,

    /// Which shape of run this is (single/parallel/chain), mirroring [`RunMode`].
    pub mode: RunMode,

    /// The run's actually-*resolved* fork context — `Fresh` or `Fork` — sourced from
    /// [`crate::fork_context::ForkContextResolver::resolve`]'s output, never from the caller's
    /// raw/omitted request (R-SA-111). Drives the fork-badge presentational branch in
    /// `tui/render.rs`: present when `context == ContextMode::Fork`, absent when `Fresh`
    /// (R-SA-110).
    pub context: ContextMode,

    /// Foreground (synchronously streamed) or async/background (polled out-of-band) — drives
    /// the R-SA-106/107 rendering-region split and the R-SA-116/117 notice-debounce split.
    pub source: RunSource,

    /// The run's full on-disk-shaped lifecycle/step/parallel-group status record
    /// (func-SA §4.5). See the module-level doc for why this snapshot embeds the whole
    /// [`RunStatus`] rather than a narrower duplicate enum.
    pub status: RunStatus,

    /// The fully-qualified name of the agent persona currently executing (or about to execute),
    /// if known — `None` before the first NDJSON progress event has arrived.
    pub current_agent: Option<String>,

    /// Zero-based index of the chain/parallel step currently active, if this run is a `Chain` or
    /// `Parallel` shape and progress has begun.
    pub current_step_index: Option<u32>,

    /// Total number of steps currently known for this run, if applicable — mirrors
    /// [`RunStatus::chain_step_count`] but recomputed at the renderer-facing granularity used by
    /// step-progress display (e.g. "step 2 of 5"); may grow over the run's lifetime as
    /// chain-append requests are consumed (R-SA-095).
    pub total_steps: Option<u32>,

    /// The name of the tool the child is currently invoking, if the most recent NDJSON event
    /// carried one — cleared once the corresponding tool-result event arrives.
    pub current_tool: Option<String>,

    /// Running count of agent turns observed so far for this run, folded from NDJSON progress
    /// events.
    pub turn_count: u32,

    /// Running count of tool invocations observed so far for this run, folded from NDJSON
    /// progress events.
    pub tool_count: u32,

    /// A short, renderer-facing excerpt of the most recent textual output the child has produced
    /// (already truncated/sanitized by the caller before being placed here — this type performs
    /// no truncation of its own).
    pub recent_output: Option<String>,

    /// Nested fanout children of this run (parallel/dynamic-group sub-runs), rendered indented
    /// and depth-capped by the renderer (R-SA-112/113) — this type itself imposes no depth or
    /// count limit; that bound lives entirely in the `tui/render.rs` fold function.
    pub children: Vec<NestedRunSummary>,

    /// Wall-clock instant of the most recently observed activity for this run — the input to the
    /// "needs attention" staleness heuristic (R-SA-114), which is computed by comparing this
    /// value against an attention threshold, never by this type itself.
    pub last_activity_at: Instant,
}

// =================================================================================================
// NestedRunSummary (func-SA §4.6; arch-SA §3.7; R-SA-112/113)
// =================================================================================================

/// A compact, recursively-nestable summary of one fanout child run, used to render nested
/// parallel/dynamic-group fanout indented under its parent step's entry (R-SA-112).
///
/// Deliberately narrower than [`SubagentProgressSnapshot`] — a nested child only needs enough
/// state to render one summary line (agent name, status, and its own children), not the full
/// renderer-input surface (current tool, turn/tool counts, recent-output excerpt) a top-level
/// snapshot carries. This type carries no depth or line-budget cap of its own; the recursive
/// fold-to-aggregate renderer in `tui/render.rs` (later phase) is solely responsible for
/// enforcing the depth-2 cap and per-level line budget (R-SA-112/113) when walking `children`.
#[derive(Clone, Debug)]
pub struct NestedRunSummary {
    /// This child run's identity.
    pub run_id: background::RunId,

    /// The fully-qualified name of the agent persona this child run executes.
    pub agent: String,

    /// This child's full lifecycle/step/parallel-group status record — same rationale as
    /// [`SubagentProgressSnapshot::status`].
    pub status: RunStatus,

    /// This child's own nested fanout children, if it is itself a `Parallel`/`Chain`-with-
    /// nested-groups run — recursion is bounded only by the renderer, never by this type.
    pub children: Vec<NestedRunSummary>,
}

// =================================================================================================
// Control notices (func-SA §4.6/§5.5; arch-SA §3.7; R-SA-114-122)
// =================================================================================================

/// The distinguishing key for a control notice: which run it concerns, which of the two notice
/// kinds it is, and upstream's own `controlNotificationKey` discriminant. Dedup (R-SA-115: at most
/// once per distinct attention identity; R-SA-122: persists across a hot-reload of the
/// orchestrating extension for the process's lifetime) is keyed on this type — `tui/notices.rs`'s
/// `ControlNoticeState.delivered: HashSet<ControlNoticeKey>` is the sole consumer of that dedup
/// contract, but the key type itself lives here since [`ControlNotice`] embeds it.
///
/// # Why `notification_key` exists (SUBA-N05)
///
/// Upstream keys its `visibleControlNotices` dedup set on
/// `controlNotificationKey(event, childIntercomTarget)` (`shared/subagent-control.ts:142-145`
/// @v0.34.0) — `"{childKey}:{type}:{reason}"`, where `childKey` is the child's intercom target when
/// the bridge is live, else `"{runId}:{index}"` (or bare `runId` for an index-less event). Its
/// pending-timer map is keyed on `"{runId}:{that}"` (`extension/control-notices.ts:23-26` @v0.34.0).
///
/// Before SUBA-N05 this struct was only `(run_id, kind)`, which is STRICTLY coarser on two axes
/// upstream distinguishes: the `reason` (an `idle` attention notice and a later
/// `mutating_failures`/`supervisor_request` one about the same run collapsed into a single
/// delivery) and the child index (every step of a chain/parallel run shared one key). Carrying
/// upstream's rendered key verbatim restores both. `run_id` is retained alongside it because it is
/// also the id [`ControlNotice`]'s actionability re-check looks up in the live-run projection, and
/// because it is upstream's own timer-key prefix; it is redundant *as dedup input* (`childKey` is
/// derived from the run id either way), so including it in the hash cannot merge or split any
/// upstream key class.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ControlNoticeKey {
    /// The run this notice concerns.
    pub run_id: background::RunId,
    /// Which kind of notice this is.
    pub kind: ControlNoticeKind,
    /// pi `controlNotificationKey(event, childIntercomTarget)` — see the type doc. Built by
    /// [`crate::exec::control::control_notification_key`], never hand-formatted at a call site.
    pub notification_key: String,
}

/// The two control-notice kinds this subsystem surfaces (func-SA §5.5).
///
/// `ActiveLongRunning` and `NeedsAttention` are deliberately kept as sibling variants of one
/// enum, rather than two separate notice types, because both share the identical
/// key/debounce/dedup/delivery machinery in `tui/notices.rs` — only the human-facing `reason`/
/// `message` text differs between them. Per R-SA-114, `NeedsAttention` is a staleness heuristic
/// (no new NDJSON activity for longer than an attention threshold) that MUST NOT be conflated
/// with, or override, an intentional `Paused` (soft-interrupt) or `Failed` terminal
/// [`background::RunState`] — a run already `Paused` or `Failed` is not eligible to additionally
/// surface as `NeedsAttention`; that exclusion is enforced by the (later-phase) code that
/// constructs `ControlNotice` values, not by this enum itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlNoticeKind {
    /// The run has been active for longer than a "this is taking a while" threshold — informational,
    /// not actionable.
    ActiveLongRunning,
    /// The run has produced no new NDJSON activity for longer than the attention threshold and
    /// is not in an intentional `Paused`/`Failed` state (R-SA-114) — may warrant the orchestrator
    /// (or, transitively, a human) taking a look.
    NeedsAttention,
}

/// One control notice ready for delivery through the transcript sink (func-SA §4.6; R-SA-121:
/// delivered control notices MUST appear as distinguishable, non-tool-result transcript entries,
/// never folded into the inline tool-result rendering of the run that triggered them).
///
/// Construction, debounce (R-SA-116), immediate-vs-debounced dispatch by [`RunSource`]
/// (R-SA-117), and turn-triggering (R-SA-118) are all `tui/notices.rs` responsibilities (a later
/// phase); this type is the plain data payload that machinery operates on.
#[derive(Clone, Debug)]
pub struct ControlNotice {
    /// The `(run_id, kind)` identity used for dedup (R-SA-115/122).
    pub key: ControlNoticeKey,

    /// Foreground notices are debounced-then-actionability-rechecked (R-SA-116); async notices
    /// are delivered immediately (R-SA-117) and may trigger a new orchestrator turn (R-SA-118).
    pub source: RunSource,

    /// The agent persona active at the moment this notice was raised, if known — used by the
    /// R-SA-116 actionability re-check to detect that the run has since advanced to a different
    /// agent (in which case the notice is dropped, not delivered stale).
    pub agent: Option<String>,

    /// The step index active at the moment this notice was raised, if known — used by the same
    /// R-SA-116 re-check for step-level staleness.
    pub step_index: Option<u32>,

    /// A short, machine-oriented reason code/summary for why this notice fired (e.g. the
    /// staleness duration for `NeedsAttention`).
    pub reason: String,

    /// The full human-facing message to surface in the transcript entry.
    pub message: String,
}
