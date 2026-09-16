//! SCOPE_10 — the wire shapes of the async status snapshot: the caps, the omission counters, the
//! two vocabularies and the bounded node record every projector in [`super::project`] fills in.
//!
//! Ports pi `runs/shared/async-status-projection.ts:8-100,143-171` (`@7fe9dee1`). Nothing here
//! reads a run, a session or the clock — this file is the SHAPE, and [`super::project`] is the
//! only thing that decides what goes in it.
//!
//! # Field order is the wire order, deliberately
//!
//! Upstream builds every object with `...(x ? { x } : {})` spreads, so the JSON key order is the
//! order of the OBJECT LITERAL, not of the `interface`. serde emits fields in declaration order,
//! so each struct below is declared in its literal's order (for [`AsyncStatusSnapshotHostStep`]
//! those two orders differ upstream — `projectHostStep`'s literal puts `state` second, the
//! interface puts it fourth, and the literal is what a reader parsing the bytes sees).
//!
//! # `Option::is_none` skipping is not cosmetic
//!
//! The same spreads mean an absent value is an ABSENT KEY upstream, never `null`. Every optional
//! field therefore carries `skip_serializing_if = "Option::is_none"`; a `null` in this document
//! would be a port defect, not a formatting difference.

use crate::workflows::{HostStepMonitorKind, HostStepState, HostStepVerdict};
use crate::workflows::{sanitize_display_text, truncate_display};

/// pi `DEFAULT_MAX_RUNS` (`async-status-projection.ts:11`).
pub(crate) const DEFAULT_MAX_RUNS: usize = 20;
/// pi `DEFAULT_MAX_CHILDREN_PER_NODE` (`:12`).
pub(crate) const DEFAULT_MAX_CHILDREN_PER_NODE: usize = 8;
/// pi `DEFAULT_MAX_DEPTH` (`:13`).
pub(crate) const DEFAULT_MAX_DEPTH: usize = 3;
/// pi `DEFAULT_MAX_STRING_LENGTH` (`:14`).
pub(crate) const DEFAULT_MAX_STRING_LENGTH: usize = 160;
/// pi `DEFAULT_MAX_SERIALIZED_BYTES` (`:15`) — 32 KiB.
pub(crate) const DEFAULT_MAX_SERIALIZED_BYTES: usize = 32 * 1024;
/// The floor `resolveCaps` clamps `maxSerializedBytes` up to (`:149`): a caller cannot ask for a
/// budget so small that the envelope alone would not fit.
pub(crate) const MIN_MAX_SERIALIZED_BYTES: usize = 256;

/// JS `Number.MAX_SAFE_INTEGER`, the bound `publicTime`/`publicCount` test with
/// `Number.isSafeInteger` (`:165-171`).
///
/// [CYRUP-DELTA, mechanism] upstream's inputs are JS `number`s, so "is this a safe integer" is a
/// real question there and a mostly-vacuous one here (cyrup's sources are already `i64`/`u64`).
/// The bound is kept anyway, because it is not vacuous at the TOP of the range: an `i64`
/// timestamp above 2^53-1 would round when a JS reader parsed this document back, so upstream's
/// filter is the correct thing to reproduce rather than the thing to drop.
pub(crate) const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// pi `AsyncStatusSnapshotState` (`async-status-projection.ts:17`) — the eight words a node's
/// lifecycle collapses onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AsyncStatusSnapshotState {
    /// Declared, not started.
    Queued,
    /// In flight.
    Running,
    /// Finished successfully.
    Complete,
    /// Finished with a failure.
    Failed,
    /// Finished with an outcome that is neither clean success nor clean failure — and the
    /// fall-through for any word this build does not recognise (`:177`).
    Partial,
    /// Interrupted mid-flight (soft pause).
    Paused,
    /// Terminated by an explicit stop.
    Stopped,
    /// Refused at a checkpoint.
    Rejected,
}

impl AsyncStatusSnapshotState {
    /// pi `normalizeState` (`:173-178`), including both of its rewrites and its fall-through:
    /// `"completed" -> complete`, `"pending" -> queued`, an unrecognised word ->
    /// [`Self::Partial`]. **Never an error** — a snapshot is a report, and a report that throws
    /// on an unfamiliar status word is worse than one that says "partial".
    #[must_use]
    pub fn from_wire(value: &str) -> Self {
        match value {
            "completed" | "complete" => Self::Complete,
            "pending" | "queued" => Self::Queued,
            "running" => Self::Running,
            "failed" => Self::Failed,
            "partial" => Self::Partial,
            "paused" => Self::Paused,
            "stopped" => Self::Stopped,
            "rejected" => Self::Rejected,
            // `isAsyncStatusSnapshotState(value)` was false (`:176`).
            _ => Self::Partial,
        }
    }

    /// pi `terminalState` (`:180-182`) — everything except [`Self::Queued`]/[`Self::Running`].
    /// Gates the `endedAt` key at `:238`/`:275`/`:337`: a RUNNING node with a recorded end time
    /// must not carry one.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// pi `AsyncStatusSnapshotKind` (`:18`) WIDENED by the node's own `kind` declaration
/// (`:54`, `AsyncStatusSnapshotKind | "host-step"`).
///
/// [CYRUP-DELTA, mechanism] upstream spells the fourth arm as a union at the one field that can
/// hold it; TypeScript can widen a union inline and Rust cannot, so the four words are one enum
/// here. The wire vocabulary is identical, and no cyrup seam needs the three-word type on its
/// own — `kindForMode` (`:184-186`) only ever returns `subagent`/`workflow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AsyncStatusSnapshotKind {
    /// An ordinary background subagent run.
    Subagent,
    /// A workflow run.
    Workflow,
    /// One step inside a run (or one stage of its graph).
    Step,
    /// A host-owned monitor row (pi's `"host-step"` widening).
    HostStep,
}

/// pi `AsyncStatusSnapshotActivity` (`:35-42`) — the live-activity roll-up a node carries when it
/// has one. Emitted only when at least one member is present (`:216`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncStatusSnapshotActivity {
    /// pi `state` — the derived activity word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// pi `currentTool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// pi `lastActivityAt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// pi `currentToolStartedAt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_started_at: Option<i64>,
    /// pi `turnCount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// pi `toolCount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
}

impl AsyncStatusSnapshotActivity {
    /// pi `Object.keys(activity).length ? activity : undefined` (`:216`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.is_none()
            && self.current_tool.is_none()
            && self.last_activity_at.is_none()
            && self.current_tool_started_at.is_none()
            && self.turn_count.is_none()
            && self.tool_count.is_none()
    }
}

/// pi `AsyncStatusSnapshotHostStep` (`:44-54`) — the host-monitor metadata a `host-step` node
/// hangs off. Declared in `projectHostStep`'s literal order (`:303-314`), which is the wire order.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncStatusSnapshotHostStep {
    /// pi `kind` — the monitor category, never inferred.
    pub kind: HostStepMonitorKind,
    /// pi `state` — the host step's own lifecycle word, kept alongside the node's normalized
    /// [`AsyncStatusSnapshotState`] because the two vocabularies are different.
    pub state: HostStepState,
    /// pi `provider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// pi `role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// pi `verdict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<HostStepVerdict>,
    /// pi `reasonCode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// pi `detail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// pi `target`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// pi `stale` — `hostStep.freshness?.stale`, and only when the freshness record recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    /// pi `report` — the BASENAME of the report artifact, never its path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// pi `AsyncStatusSnapshotNode` (`:56-67`) — one run, step, graph stage, nested child or host
/// monitor in the bounded tree.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncStatusSnapshotNode {
    /// pi `id` — bounded through `publicText`, so it is safe to render verbatim.
    pub id: String,
    /// pi `kind`.
    pub kind: AsyncStatusSnapshotKind,
    /// pi `label`.
    pub label: String,
    /// pi `state`.
    pub state: AsyncStatusSnapshotState,
    /// pi `startedAt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// pi `updatedAt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// pi `endedAt` — present only for a [`AsyncStatusSnapshotState::is_terminal`] node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// pi `activity`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<AsyncStatusSnapshotActivity>,
    /// pi `hostStep`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_step: Option<AsyncStatusSnapshotHostStep>,
    /// pi `children` — absent (not an empty array) when the node has none it could retain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<AsyncStatusSnapshotNode>>,
}

/// pi `AsyncStatusSnapshotCaps` (`:69-75`) — the resolved bounds, echoed into the document so a
/// reader can tell a short snapshot from a complete one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncStatusSnapshotCaps {
    /// pi `maxRuns`.
    pub max_runs: usize,
    /// pi `maxChildrenPerNode`.
    pub max_children_per_node: usize,
    /// pi `maxDepth`.
    pub max_depth: usize,
    /// pi `maxStringLength`.
    pub max_string_length: usize,
    /// pi `maxSerializedBytes`.
    pub max_serialized_bytes: usize,
}

/// pi `AsyncStatusSnapshotOmitted` (`:77-81`) — what the caps cost, counted rather than hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncStatusSnapshotOmitted {
    /// Runs dropped by [`AsyncStatusSnapshotCaps::max_runs`] and by the byte-budget search.
    pub runs: usize,
    /// Children dropped by [`AsyncStatusSnapshotCaps::max_children_per_node`] or by depth.
    pub children: usize,
    /// Set the moment the serialized document is found to exceed
    /// [`AsyncStatusSnapshotCaps::max_serialized_bytes`] — BEFORE the search that fits it
    /// (`:370-371`), so it records "the budget bound this document", not "the search succeeded".
    pub byte_limit_exceeded: bool,
}

/// pi `AsyncStatusSnapshot` (`:83-90`) — the whole document.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncStatusSnapshot {
    /// Always [`super::ASYNC_STATUS_SNAPSHOT_KIND`]; a literal type upstream, a stamped field
    /// here (Rust has no string-literal type, and a reader keys on the VALUE either way).
    pub kind: String,
    /// Always [`super::ASYNC_STATUS_SNAPSHOT_VERSION`].
    pub version: u32,
    /// pi `generatedAt` — the ONE clock reading this document is stamped with.
    pub generated_at: i64,
    /// The resolved caps.
    pub caps: AsyncStatusSnapshotCaps,
    /// What the caps cost.
    pub omitted: AsyncStatusSnapshotOmitted,
    /// The retained runs, `updatedAt` descending.
    pub runs: Vec<AsyncStatusSnapshotNode>,
}

/// pi `AsyncStatusSnapshotOptions` (`:92-99`). Every member is optional and every default lives
/// in [`resolve_caps`], so `Default::default()` is upstream's `{}`.
///
/// `i64`, not `usize`: upstream's members are JS `number`s and `resolveCaps` explicitly clamps a
/// NEGATIVE one to zero (`Math.max(0, …)`). A `usize` parameter would make that clamp
/// unreachable and silently reject the input at the type level instead of bounding it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AsyncStatusSnapshotOptions {
    /// pi `generatedAt` — an explicit clock reading, for a caller that already took one.
    pub generated_at: Option<i64>,
    /// pi `maxRuns`.
    pub max_runs: Option<i64>,
    /// pi `maxChildrenPerNode`.
    pub max_children_per_node: Option<i64>,
    /// pi `maxDepth`.
    pub max_depth: Option<i64>,
    /// pi `maxStringLength`.
    pub max_string_length: Option<i64>,
    /// pi `maxSerializedBytes`.
    pub max_serialized_bytes: Option<i64>,
}

/// pi `resolveCaps` (`:143-151`): `Math.max(0, Math.floor(…))` for the four counts, and
/// `Math.max(256, …)` for the byte budget. A JSON integer arrives already floored, so only the
/// clamps survive the port.
#[must_use]
pub fn resolve_caps(options: &AsyncStatusSnapshotOptions) -> AsyncStatusSnapshotCaps {
    fn clamp(value: Option<i64>, default: usize) -> usize {
        match value {
            None => default,
            Some(v) => usize::try_from(v.max(0)).unwrap_or(default),
        }
    }
    AsyncStatusSnapshotCaps {
        max_runs: clamp(options.max_runs, DEFAULT_MAX_RUNS),
        max_children_per_node: clamp(options.max_children_per_node, DEFAULT_MAX_CHILDREN_PER_NODE),
        max_depth: clamp(options.max_depth, DEFAULT_MAX_DEPTH),
        max_string_length: clamp(options.max_string_length, DEFAULT_MAX_STRING_LENGTH),
        max_serialized_bytes: clamp(options.max_serialized_bytes, DEFAULT_MAX_SERIALIZED_BYTES)
            .max(MIN_MAX_SERIALIZED_BYTES),
    }
}

/// pi `publicText` (`:153-157`).
///
/// The `value.slice(0, maxLength * 4)` PRE-slice is the load-bearing part and is not an
/// optimisation: [`sanitize_display_text`] walks escape sequences character by character, so
/// without it a megabyte of CSI introducers would be sanitized in full to produce 160 characters.
/// Four UTF-16 units per retained unit is upstream's headroom for sequences that collapse.
///
/// A `None` (upstream's `typeof value !== "string"`) returns the fallback RAW — un-sanitized and
/// un-truncated — exactly as `:154` does. Callers pass a fallback they own.
#[must_use]
pub fn public_text(value: Option<&str>, fallback: &str, max_length: usize) -> String {
    let Some(value) = value else {
        return fallback.to_string();
    };
    let normalized = sanitize_display_text(&truncate_display(value, max_length.saturating_mul(4)));
    let source = if normalized.is_empty() {
        fallback
    } else {
        &normalized
    };
    truncate_display(source, max_length)
}

/// pi `publicOptionalText` (`:159-163`) — the same pre-slice, but an empty sanitized result is
/// ABSENT rather than a fallback.
#[must_use]
pub fn public_optional_text(value: Option<&str>, max_length: usize) -> Option<String> {
    let normalized = sanitize_display_text(&truncate_display(value?, max_length.saturating_mul(4)));
    if normalized.is_empty() {
        None
    } else {
        Some(truncate_display(&normalized, max_length))
    }
}

/// pi `publicTime` (`:165-167`) — a non-negative safe integer, or absent.
#[must_use]
pub fn public_time(value: Option<i64>) -> Option<i64> {
    value.filter(|v| (0..=MAX_SAFE_INTEGER).contains(v))
}

/// pi `publicCount` (`:169-171`) — the same test over a count.
#[must_use]
pub fn public_count(value: Option<u64>) -> Option<u64> {
    value.filter(|v| *v <= MAX_SAFE_INTEGER.unsigned_abs())
}
