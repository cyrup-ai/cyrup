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
//! this file is deliberately pure types plus the narrow, pure helpers ([`RunId::new`],
//! [`RunPaths::for_run`], [`RunState`]'s transition guard) that those later files build on.
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

pub mod atomic;
pub mod control;
pub mod cascade;
pub mod spawn_detached;
pub mod parent_anchor;
pub mod reconcile;
pub mod runner_main;
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

pub mod tracker;
pub mod run_status;
pub mod fleet_view;
pub mod wait;
pub mod resume_guidance;

use std::path::{Path, PathBuf};

use cyrup_core::{ModelId, Usage};

use crate::exec::SingleResult;

// =================================================================================================
// RunId (func-SA §4.5, R-SA-072)
// =================================================================================================

/// An opaque run-id token minted at spawn time — a short, URL/filesystem-safe random token with
/// **no pre-flight uniqueness check** against the filesystem or any registry (R-SA-072): the
/// child run directory's existence is established later, via `mkdir` at spawn time, not by this
/// type.
///
/// # Entropy
///
/// func-SA §4.5 illustratively targets an "8-hex-character random token"; arch-SA §3.6's sketch
/// widens that to "8+ char". Neither figure is itself the safety property — the actual
/// requirement is that two runs minted concurrently (including across process boundaries, e.g.
/// two orchestrator instances on the same machine racing a background spawn) never collide. An
/// 8-hex-character token is only 32 bits of entropy, which starts exhibiting non-negligible
/// birthday-collision probability well within the run-id volumes a long-lived multi-session cyrup
/// deployment could plausibly mint. This implementation instead derives the token from a random
/// (v4) UUID's 128 bits of entropy — collision probability at any volume this system could
/// plausibly reach is negligible — and renders it as the UUID's 32-hex-digit simple form (no
/// hyphens), which is still a compact, filesystem-safe, purely-hex token satisfying the letter of
/// both documents' "short hex token" shape while exceeding their stated entropy floor rather than
/// undershooting it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RunId(std::sync::Arc<str>);

impl RunId {
    /// Mints a fresh [`RunId`] from 128 bits of random entropy. Never checks the filesystem or
    /// any other registry for pre-existence (R-SA-072) — the caller is responsible for creating
    /// the run directory via `mkdir` at spawn time, which is where an astronomically-unlikely
    /// collision would surface (as an `mkdir` failure), not here.
    #[must_use]
    pub fn new() -> Self {
        let token = uuid::Uuid::new_v4().as_simple().to_string();
        Self(std::sync::Arc::from(token.as_str()))
    }

    /// Wraps an already-known run-id token (e.g. one parsed back from a run directory name or a
    /// CLI argument) without minting new entropy. Does **not** validate the token's shape —
    /// safe-token validation (no path separators, no `..`, non-empty) against filesystem lookups
    /// is `background/control.rs`'s job (R-SA-087), not this constructor's; this is a plain,
    /// infallible wrap so callers that already trust their source (e.g. round-tripping a value
    /// this process itself minted and serialized) are not forced through a `Result`.
    #[must_use]
    pub fn from_token(token: impl Into<std::sync::Arc<str>>) -> Self {
        Self(token.into())
    }

    /// Borrows the token as a plain `&str` — the form used for directory-name construction,
    /// display, and safe-token validation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    /// Mints a fresh random [`RunId`], identical to [`RunId::new`]. Provided so `RunId` composes
    /// naturally with `#[derive(Default)]` call sites elsewhere in the crate; every `Default`
    /// call still mints fresh entropy, never a fixed/empty sentinel.
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RunId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// =================================================================================================
// RunMode (func-SA §4.5)
// =================================================================================================

/// Which shape of run this is — mirrors [`crate::spawn::chain_graph::RunnerStep`]'s three-way
/// discriminant at the whole-run granularity rather than the per-step granularity: a `Chain` run
/// may itself contain `ParallelGroup`/`DynamicGroup` steps internally, but the run *as a whole* is
/// tagged by its outermost shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunMode {
    /// One agent invocation, no fan-out.
    Single,
    /// A static-width parallel fan-out group, run standalone (not as one step of a larger chain).
    Parallel,
    /// A linear sequence of steps (`ChainGraph`), each possibly itself a `ParallelGroup`/
    /// `DynamicGroup`.
    Chain,
}

// =================================================================================================
// RunState / StepState — monotone-forward-only lifecycle (func-SA §4.5)
// =================================================================================================

/// The overall lifecycle state of a background run (func-SA §4.5's `RunStatus.state`).
///
/// Transitions are **monotone-forward-only**: once a run reaches a given state, it may only move
/// to a state ranked strictly higher by [`RunState::rank`], with the sole documented exception of
/// `Paused -> Running` (interrupt-then-resume, R-SA-084/086, which resumes the *same* logical run
/// record in place rather than minting a new one) and `Paused` itself being reachable from
/// `Running` (interrupt, R-SA-084) even though `Paused` does not rank above `Running` in the
/// terminal-progress sense — see [`RunState::can_transition_to`] for the exact, non-linear
/// adjacency this enum actually permits; "monotone-forward" here means "never silently regresses
/// past a state some reader may already have observed and acted on as final", not a strict total
/// order.
///
/// # Enforcement (R-SA-075/077's forward-only requirement)
///
/// `RunState` is a plain, freely-constructible `enum` — not a family of distinct marker types —
/// because [`RunStatus`] must remain a single, uniformly `serde`-(de)serializable struct that
/// round-trips through `status.json` regardless of which state it currently holds (a type-state
/// encoding, with one Rust type per lifecycle state, would make `RunStatus` itself generic over
/// state and break that uniform (de)serialization contract, plus every `HashMap<RunId,
/// RunStatus>`/`Vec<RunStatus>` call site elsewhere in this subsystem). Forward-only-ness is
/// therefore enforced **at the mutation boundary**, not at the type level: every in-process state
/// change MUST go through [`RunState::try_advance`] (which returns `Err` on a disallowed
/// transition rather than silently permitting it), and every on-disk write MUST go through
/// [`RunStatus::advance_state`], which calls `try_advance` before touching `self.state`. A caller
/// that bypasses these methods and assigns `.state` directly defeats the guard — by convention,
/// no code in this crate does that (`rg 'run_status.state =' -- '!*/background/mod.rs'` prior to a
/// merge is the intended lint until a stricter type-level encoding is judged worth the
/// serialization complexity it would add).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Minted and about to be spawned, but the detached runner has not yet written its own
    /// initial status (covers the R-SA-090 grace window before hop-2 confirms).
    Queued,
    /// The detached runner has written its initial `status.json` and is actively executing steps.
    Running,
    /// An interrupt was consumed (R-SA-084): every then-running step was marked `Paused`, not
    /// `Failed`. A soft, resumable, non-terminal state.
    Paused,
    /// Every step finished without a run-ending failure.
    Complete,
    /// The run ended in failure — either a step's own failure or a synthesized failure from
    /// stale-dead reconciliation (R-SA-092).
    Failed,
    /// G77 — pi `stopRunner` (`runs/background/subagent-runner.ts:2955-2984` @v0.43.0): an explicit
    /// user/agent **stop** request (`control/stop.json`, pi `StopRequest`
    /// `runs/background/control-channel.ts:49-53`) was consumed. Every then-running-or-pending step
    /// was marked [`StepState::Stopped`] with `exitCode: 1` and the literal
    /// [`STOP_MESSAGE`](crate::background::control::STOP_MESSAGE), and the run's terminal record is
    /// `state: "stopped"` (pi `statusPayload.state = "stopped"`, `subagent-runner.ts:2959`).
    ///
    /// **A first-class terminal state, NOT an alias for [`Self::Failed`] or [`Self::Paused`].**
    /// Upstream distinguishes all three at every reader: `stopRunner`'s own guard is
    /// `if (stopped || timedOut || interrupted || state !== "running") return`
    /// (`subagent-runner.ts:2955-2986`), so a stop and a timeout and an interrupt are mutually exclusive
    /// verdicts; `resolveSubagentResultStatus` ranks `"stopped"` above `"paused"` and above the
    /// `success`/exit-code fallbacks (`intercom/result-intercom.ts:31-35`);
    /// `resolveGroupedStatus` gives `"stopped"` its own precedence slot between `"failed"` and
    /// `"paused"` (`result-intercom.ts:84-87`); `async-resume.ts:406` REFUSES to resume a stopped
    /// run (`"was stopped and cannot be resumed"`) where a paused one is exactly what `resume`
    /// exists for; and `notify.ts:210` renders a fourth `status` word for it. Folding `Stopped`
    /// into `Failed` would silently make a stopped run look resumable-or-not identically to a
    /// crashed one and would erase the distinct user-visible string at every one of those sites.
    Stopped,
}

impl RunState {
    /// A rank used only to describe "how far along" a state is for documentation/diagnostic
    /// purposes (e.g. UI ordering). **Not** consulted by [`RunState::can_transition_to`] — the
    /// actual transition table is the explicit adjacency list below, because the true allowed-
    /// transition graph is not a simple linear order (`Paused` can both follow `Running` and
    /// precede a fresh `Running` again on resume, R-SA-086).
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            RunState::Queued => 0,
            RunState::Running => 1,
            RunState::Paused => 2,
            RunState::Complete => 3,
            RunState::Failed => 3,
            RunState::Stopped => 3,
        }
    }

    /// `true` once a run in this state will never again be mutated by the runner that owns it
    /// (`Complete`/`Failed`/`Stopped`). `Paused` is deliberately **not** terminal — R-SA-084 is
    /// explicit that interrupt is a soft, resumable pause, never a terminal state.
    ///
    /// [`RunState::Stopped`] **is** terminal in its own right (G77): pi treats a stopped run as
    /// finished-and-non-resumable (`async-resume.ts:406` throws rather than reviving it,
    /// `chain-root-attachment.ts:60`'s `TERMINAL_STATES` set contains `"stopped"`, and
    /// `stale-run-reconciler.ts:292`'s `isTerminalState` returns true for it) — it is neither a
    /// pause nor a failure.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, RunState::Complete | RunState::Failed | RunState::Stopped)
    }

    /// Returns `true` if a transition from `self` to `next` is permitted.
    ///
    /// The explicit adjacency (not a derived `<` on [`RunState::rank`]) is:
    /// - `Queued -> Running | Failed` (spawn confirmed and the runner took over; or the runner
    ///   itself failed to even start, e.g. a spawn-level I/O error surfaced before any step ran).
    /// - `Running -> Paused | Complete | Failed` (interrupt; normal completion; step/stale-dead
    ///   failure).
    /// - `Paused -> Running | Failed` (resume respawns and steers execution forward again,
    ///   R-SA-086; or a paused run is later reconciled to `Failed` by long-staleness reconciliation,
    ///   R-SA-091, if it is never resumed).
    /// - `Queued -> Stopped` / `Running -> Stopped` (G77): an explicit stop request was consumed.
    ///   Upstream's `stopRunner` gate is `statusPayload.state !== "running"` returns early
    ///   (`subagent-runner.ts:2955-2986`), and the parent-side `stopAsyncRun` only accepts a target whose
    ///   reconciled state is `"running"` or `"queued"` (`async-stop-action.ts:41`) — so those are
    ///   exactly the two predecessors, and a `Paused` run is deliberately NOT stoppable (upstream
    ///   returns `"No running or queued async run was found"` for it).
    /// - `Complete`/`Failed`/`Stopped` are terminal: no outgoing transition is permitted, including
    ///   to themselves — a caller that already observed a terminal state and tries to write the same
    ///   terminal state again should treat that as a no-op at a layer above this guard, not as a
    ///   fresh "transition".
    #[must_use]
    pub fn can_transition_to(self, next: RunState) -> bool {
        matches!(
            (self, next),
            (RunState::Queued, RunState::Running)
                | (RunState::Queued, RunState::Failed)
                | (RunState::Queued, RunState::Stopped)
                | (RunState::Running, RunState::Paused)
                | (RunState::Running, RunState::Complete)
                | (RunState::Running, RunState::Failed)
                | (RunState::Running, RunState::Stopped)
                | (RunState::Paused, RunState::Running)
                | (RunState::Paused, RunState::Failed)
        )
    }

    /// Attempts to advance from `self` to `next`, returning the new state on success or
    /// [`RunStateTransitionError`] if the transition is not permitted by
    /// [`RunState::can_transition_to`]. This is the single choke point every in-process state
    /// mutation in this subsystem MUST route through (see the enforcement note on the type
    /// itself).
    ///
    /// # Errors
    ///
    /// Returns [`RunStateTransitionError`] if `next` is not reachable from `self`.
    pub fn try_advance(self, next: RunState) -> Result<RunState, RunStateTransitionError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(RunStateTransitionError { from: self, to: next })
        }
    }
}

impl Default for RunState {
    /// A freshly minted run always starts `Queued` (before the detached runner has written its
    /// own initial status).
    fn default() -> Self {
        RunState::Queued
    }
}

/// A disallowed [`RunState`] transition was attempted (forward-only enforcement, R-SA-075/077).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("illegal run-state transition: {from:?} -> {to:?}")]
pub struct RunStateTransitionError {
    /// The state the transition was attempted from.
    pub from: RunState,
    /// The state the transition attempted to reach.
    pub to: RunState,
}

/// The lifecycle state of one step within a run (func-SA §4.5's `StepStatus.status`).
///
/// Distinct from [`RunState`]: a step starts `Pending` (not yet dispatched) rather than `Queued`,
/// and has no `Paused` predecessor requirement — a step is marked `Paused` directly from
/// `Running` on interrupt (R-SA-084), mirroring but not reusing `RunState`'s transition table,
/// since a step never independently resumes (resume re-selects and re-drives it via the parent
/// run, R-SA-086) the way a whole run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    /// Declared (present in the chain's step list) but not yet dispatched.
    Pending,
    /// A child subprocess for this step is currently spawned and running.
    Running,
    /// Interrupted mid-flight (R-SA-084) — end timestamp recorded, not terminal.
    Paused,
    /// Finished without failure.
    Complete,
    /// Finished with a failure (including a synthesized stale-dead failure, R-SA-092).
    Failed,
    /// G77 — pi `step.status = "stopped"` (`subagent-runner.ts:2967`): this step was still
    /// `Running` or `Pending` when an explicit stop request landed, so the runner marked it
    /// `stopped` (with `exitCode: 1` and the stop message as its `error`) rather than `failed` or
    /// `paused`. Terminal and non-resumable, exactly like [`RunState::Stopped`].
    Stopped,
}

impl StepState {
    /// `true` for `Complete`/`Failed`/`Stopped` — mirrors [`RunState::is_terminal`]'s exclusion of
    /// `Paused` for the identical reason (R-SA-084: pause is soft and resumable, never terminal),
    /// and its inclusion of `Stopped` for the reason documented there (pi
    /// `chain-root-attachment.ts:61`'s `TERMINAL_STEP_STATUSES` set lists `"stopped"` alongside
    /// `"complete"`/`"completed"`/`"failed"`/`"paused"`).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, StepState::Complete | StepState::Failed | StepState::Stopped)
    }
}

// =================================================================================================
// Live activity telemetry (func-SA §4.5; pi `subagent-runner.ts:1430-1581`, `shared/types.ts`)
// =================================================================================================
//
// pi's detached runner folds each child NDJSON event into the run's `status.json` on a live cadence
// (`updateStepFromChildEvent`, `subagent-runner.ts:2706-2861`) so a reader watching the file sees
// `currentTool`/`recentTools`/`recentOutput`/`turnCount`/`toolCount`/`tokens`/`activityState`/
// `lastActivityAt` per step, plus the top-level roll-ups those feed (`syncTopLevelCurrentTool`,
// `statusPayload.toolCount`/`turnCount`/`totalTokens`). This section is the Rust port of that data
// model, plus the pure per-event fold [`apply_child_event_to_step`] the detached runner
// (`background/runner_main.rs`) drives from the child's real stdout events.

/// pi's `ActivityState` (`shared/types.ts:156`): a run/step that is idle-but-long-running or has
/// tripped a needs-attention control heuristic. Absent (`None` on the carrying field) is pi's
/// "neither" default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    /// The step is still actively producing events but has been running a long time (pi's
    /// `active_long_running`).
    ActiveLongRunning,
    /// The step tripped a needs-attention heuristic (repeated mutating-tool failures, idle past
    /// threshold — pi's `needs_attention`).
    NeedsAttention,
}

/// Accumulated per-step (and run-wide) token totals mirroring pi's `{ input, output, total }`
/// telemetry shape (`subagent-runner.ts:1502-1507`). Kept distinct from [`cyrup_core::Usage`] (the
/// richer, cost-bearing accounting record on [`StepStatus::usage`]) because pi's live telemetry
/// carries only these three integers on `step.tokens`/`statusPayload.totalTokens`, and reproducing
/// that exact on-the-wire shape is what a status-reading UI expects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    /// Input tokens accumulated so far.
    pub input: u64,
    /// Output tokens accumulated so far.
    pub output: u64,
    /// `input + output` accumulated so far (pi carries the redundant total explicitly).
    pub total: u64,
}

impl TokenTotals {
    /// Fold one turn's `(input, output)` into the running totals (pi's additive accumulation,
    /// `subagent-runner.ts:1502-1507`).
    fn add(&mut self, input: u64, output: u64) {
        self.input = self.input.saturating_add(input);
        self.output = self.output.saturating_add(output);
        self.total = self.input.saturating_add(self.output);
    }
}

/// One entry in a step's recent-tool ring (pi's `recentTools` element,
/// `subagent-runner.ts:1448-1449`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentTool {
    /// The tool's name.
    pub tool: String,
    /// A compact preview of the tool's arguments (pi's `currentToolArgs`).
    pub args: String,
    /// Wall-clock epoch-millis the tool call ended.
    pub end_ms: i64,
}

/// Bounded cap on a step's `recent_tools`/`recent_output` rings so a long-running step's telemetry
/// can never grow `status.json` without limit (pi slices to a recent window per append,
/// `subagent-runner.ts:1878,1914` @v0.34.0).
const RECENT_RING_CAP: usize = 20;

/// The live per-step activity telemetry pi folds from child events (`subagent-runner.ts:2706-2861`),
/// carried on [`StepStatus`] via `#[serde(flatten)]` so these fields serialize at the SAME top level
/// of each `status.json` step object pi writes them at (`currentTool`, `recentTools`, … are direct
/// members of the step object, not a nested sub-object — `shared/types.ts:598-632`). Every field is
/// `#[serde(default)]` + skip-if-empty so (a) an older `status.json` written before this data model
/// existed still deserializes, and (b) a step that has produced no telemetry yet serializes to the
/// exact same lean object shape it did before this model was added (no test asserting an exact
/// pre-telemetry step shape regresses).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepTelemetry {
    /// The tool currently executing in this step, if any (pi `step.currentTool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// A compact preview of the current tool's arguments (pi `step.currentToolArgs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_args: Option<String>,
    /// Epoch-millis the current tool started (pi `step.currentToolStartedAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_started_at: Option<i64>,
    /// The filesystem path the current tool is operating on, when derivable (pi `step.currentPath`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    /// The most recent completed tool calls, oldest-first, capped at [`RECENT_RING_CAP`]
    /// (pi `step.recentTools`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_tools: Vec<RecentTool>,
    /// The most recent lines of assistant/tool output, oldest-first, capped at [`RECENT_RING_CAP`]
    /// (pi `step.recentOutput`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_output: Vec<String>,
    /// Count of tool calls started in this step (pi `step.toolCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    /// Count of assistant turns completed in this step (pi `step.turnCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// Accumulated token totals for this step (pi `step.tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenTotals>,
    /// The step's derived activity state (pi `step.activityState`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<ActivityState>,
    /// Epoch-millis of the most recent observed activity for this step (pi `step.lastActivityAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// The active model's thinking level for this step, when reported (pi `step.thinking`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// The file this step wrote its final output to, when a file-output handoff was configured
    /// (pi `step.outputFile`/the run-level `outputFile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<PathBuf>,
    /// G90: how many steering messages this step has ACCEPTED (pi `step.steerCount`,
    /// `subagent-runner.ts:1761`). Written by the runner's steer router, read by
    /// `run_status`'s per-step line — it is the only way a caller can tell that
    /// `action: "steer"` actually landed on a child rather than merely being queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_count: Option<u64>,
    /// G90: epoch-millis of this step's most recent accepted steer (pi `step.lastSteerAt`,
    /// `subagent-runner.ts:1762`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_steer_at: Option<i64>,
}

/// The run-wide activity roll-ups pi maintains on the top-level `statusPayload`
/// (`subagent-runner.ts:2085-2120`, `shared/types.ts:576-638`), carried on [`RunStatus`] via
/// `#[serde(flatten)]` so they serialize at the SAME top level of `status.json` pi writes them at
/// (`status.currentTool`, `status.toolCount`, …). Same `#[serde(default)]` + skip-if-empty
/// backward-compatibility discipline as [`StepTelemetry`].
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTelemetry {
    /// The tool currently executing anywhere in the run (pi `syncTopLevelCurrentTool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// Total tool calls started across every step (pi `statusPayload.toolCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    /// Highest per-step turn count observed (pi `statusPayload.turnCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// Token totals summed across every step (pi `statusPayload.totalTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<TokenTotals>,
    /// The run's derived activity state (pi `statusPayload.activityState`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<ActivityState>,
    /// Epoch-millis of the most recent observed activity anywhere in the run
    /// (pi `statusPayload.lastActivityAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// The workflow-graph snapshot for this run (pi `statusPayload.workflowGraph`,
    /// `shared/types.ts:597`) — node ids, phases, group-status precedence, `currentNodeId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_graph: Option<WorkflowGraphSnapshot>,
    /// G90: total steering messages accepted anywhere in this run (pi `statusPayload.steerCount`,
    /// `subagent-runner.ts:1766`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_count: Option<u64>,
    /// G90: epoch-millis of the run's most recent accepted steer (pi `statusPayload.lastSteerAt`,
    /// `subagent-runner.ts:1767`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_steer_at: Option<i64>,
}

/// Fold one child NDJSON [`crate::exec::ndjson::SubagentEvent`] into a step's live telemetry — the
/// Rust port of pi's `updateStepFromChildEvent` (`subagent-runner.ts:2706-2861`), scoped to the
/// per-step [`StepTelemetry`] fields (the top-level roll-ups are the caller's job via
/// [`RunStatus::sync_top_level_telemetry`], mirroring pi's `syncTopLevelCurrentTool` living outside
/// the per-event fold). `now` is the caller's single epoch-millis reading for this event so every
/// field stamped by one event agrees on one timestamp (pi reads `Date.now()` once per call).
///
/// Only the telemetry-bearing event kinds are acted on; every other event kind (turn boundaries,
/// compaction, retries, unknown) simply bumps `last_activity_at`, exactly like pi's own fall-through.
pub fn apply_child_event_to_step(
    step: &mut StepStatus,
    event: &crate::exec::ndjson::SubagentEvent,
    now: i64,
) {
    use crate::exec::ndjson::SubagentEvent;
    match event {
        SubagentEvent::ToolExecutionStart { tool_name, args, .. } => {
            step.telemetry.tool_count = Some(step.telemetry.tool_count.unwrap_or(0).saturating_add(1));
            step.telemetry.current_tool = Some(tool_name.clone());
            step.telemetry.current_tool_args = Some(preview_tool_args(args));
            step.telemetry.current_tool_started_at = Some(now);
            step.telemetry.current_path = resolve_current_path(args);
        }
        SubagentEvent::ToolExecutionEnd { result, .. } => {
            if let Some(tool) = step.telemetry.current_tool.take() {
                let args = step.telemetry.current_tool_args.take().unwrap_or_default();
                push_bounded(
                    &mut step.telemetry.recent_tools,
                    RecentTool { tool, args, end_ms: now },
                );
            }
            // A tool result carries text output pi folds into `recentOutput` (its own
            // `tool_result_end` branch, `subagent-runner.ts:1456-1460`; cyrup's wire union collapses
            // the result payload onto `tool_execution_end`, so the fold happens here).
            let text = extract_event_text(result);
            append_recent_output(&mut step.telemetry.recent_output, &text);
            step.telemetry.current_tool_args = None;
            step.telemetry.current_tool_started_at = None;
            step.telemetry.current_path = None;
        }
        SubagentEvent::MessageEnd { message }
            if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant") =>
        {
            let text = message.get("content").map(extract_event_text).unwrap_or_default();
            append_recent_output(&mut step.telemetry.recent_output, &text);
            step.telemetry.turn_count =
                Some(step.telemetry.turn_count.unwrap_or(0).saturating_add(1));
            if let Some(usage) = event.assistant_usage() {
                let mut tokens = step.telemetry.tokens.unwrap_or_default();
                tokens.add(usage.input, usage.output);
                step.telemetry.tokens = Some(tokens);
            }
        }
        _ => {}
    }
    step.telemetry.last_activity_at = Some(now);
}

/// Push `item` onto a bounded recent-ring, dropping the oldest entry once [`RECENT_RING_CAP`] is
/// exceeded (pi keeps only a recent window, `subagent-runner.ts:1878,1914` @v0.34.0).
fn push_bounded<T>(ring: &mut Vec<T>, item: T) {
    ring.push(item);
    if ring.len() > RECENT_RING_CAP {
        let overflow = ring.len() - RECENT_RING_CAP;
        ring.drain(..overflow);
    }
}

/// Append the last few non-empty lines of `text` to a step's `recent_output` ring, bounded
/// (pi's `appendRecentStepOutput(step, text.split("\n").slice(-10))`, `subagent-runner.ts:1460`).
fn append_recent_output(ring: &mut Vec<String>, text: &str) {
    let tail: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(10)
        .map(str::to_string)
        .collect();
    for line in tail.into_iter().rev() {
        push_bounded(ring, line);
    }
}

/// Extract a compact preview of a tool call's arguments for `currentToolArgs`/a recent-tool entry
/// (pi's `extractToolArgsPreview`, `subagent-runner.ts:1440`). A best-effort compact JSON rendering
/// truncated so one pathological argument blob can never bloat `status.json`.
fn preview_tool_args(args: &serde_json::Value) -> String {
    const MAX: usize = 160;
    let rendered = match args {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if rendered.chars().count() > MAX {
        rendered.chars().take(MAX).collect::<String>() + "…"
    } else {
        rendered
    }
}

/// Derive the filesystem path a tool is operating on from its arguments, when one of pi's
/// well-known path-bearing argument keys is present (`resolveCurrentPath`,
/// `subagent-runner.ts:1437`). Returns `None` when no path-like argument is found.
fn resolve_current_path(args: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 6] = ["path", "file", "filePath", "file_path", "filename", "target"];
    let object = args.as_object()?;
    KEYS.iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

/// Extract human-readable text out of an opaque event payload `Value` — either a bare string, or a
/// `[{type:"text", text:"…"}, …]` content array (the `AssistantMessage.content`/tool-result shape),
/// mirroring pi's `extractTextFromContent` (`subagent-runner.ts`). Returns the empty string for any
/// shape carrying no text.
fn extract_event_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            out
        }
        _ => String::new(),
    }
}

// =================================================================================================
// StepStatus / ParallelGroupStatus (func-SA §4.5)
// =================================================================================================

/// Per-step status entry inside a [`RunStatus`] (func-SA §4.5).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepStatus {
    /// The fully-qualified agent name this step invokes.
    pub agent: String,
    /// This step's own lifecycle state.
    pub status: StepState,
    /// Path to the persisted session transcript this step's child was run against, once known —
    /// the seed `resume` (R-SA-085) reads back from when reviving a terminal run.
    pub session_file: Option<PathBuf>,
    /// The model actually used for the attempt that finished (or is currently running).
    pub model: Option<ModelId>,
    /// Every model attempted for this step, in fallback-ladder order (R-SA-038).
    pub attempted_models: Vec<ModelId>,
    /// Token/cost usage accumulated for this step so far.
    pub usage: Usage,
    /// The step's own error message, if `status == Failed`.
    pub error: Option<String>,
    /// Run-ids of any further background runs this step itself spawned (R-SA-104's nested
    /// descendants) — populated only for a step whose agent itself delegated to a background
    /// subagent. Kept as bare [`RunId`]s rather than full nested [`RunStatus`] snapshots so this
    /// struct stays cheap to (de)serialize on every status write; a reader that needs the nested
    /// run's own status re-reads it via [`RunPaths::nested`].
    pub nested_run_ids: Vec<RunId>,
    /// Wall-clock start time (epoch milliseconds) once this step began running.
    pub started_at: Option<i64>,
    /// Wall-clock end time (epoch milliseconds) once this step reached a terminal or paused
    /// state.
    pub ended_at: Option<i64>,
    /// Live activity telemetry folded from this step's child events (pi
    /// `subagent-runner.ts:2706-2861`) — flattened so its members serialize at the same top level
    /// of the `status.json` step object pi writes them at (`shared/types.ts:598-632`).
    #[serde(flatten, default)]
    pub telemetry: StepTelemetry,
}

impl StepStatus {
    /// A freshly declared, not-yet-dispatched step for `agent`.
    #[must_use]
    pub fn pending(agent: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            status: StepState::Pending,
            session_file: None,
            model: None,
            attempted_models: Vec::new(),
            usage: Usage::default(),
            error: None,
            nested_run_ids: Vec::new(),
            started_at: None,
            ended_at: None,
            telemetry: StepTelemetry::default(),
        }
    }
}

/// Status of one concurrently-dispatched member of a `ParallelGroup`/`DynamicGroup` step
/// (func-SA §4.5's `RunStatus.parallel_groups`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelGroupStatus {
    /// Index of the owning `ParallelGroup`/`DynamicGroup` step within the run's overall step
    /// list.
    pub group_step_index: usize,
    /// Status of each concurrently-dispatched child within this group, in fixed dispatch-order
    /// (R-SA-051: result/status ordering is always preserved, never reordered by completion
    /// order).
    pub children: Vec<StepStatus>,
}

// =================================================================================================
// RunStatus (func-SA §4.5, R-SA-075/076/077/090/094)
// =================================================================================================

/// The on-disk `status.json` record for one background run (func-SA §4.5; arch-SA §3.6).
///
/// Written via [`atomic::write_atomic_json`] (R-SA-076) at every observable state change
/// (R-SA-075) by the detached runner (`background/runner_main.rs`, later phase); read by the
/// orchestrator's poller (`background/tracker.rs`) and by `status`/`interrupt`/`resume`/
/// `append-step` control handlers (`background/control.rs`) after first running stale-run
/// reconciliation (R-SA-079, `background/reconcile.rs`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    /// This run's identity.
    pub run_id: RunId,
    /// The ORCHESTRATOR session that launched this run (pi `AsyncStatus.sessionId`,
    /// `shared/types.ts:1249`, written by the runner from `config.sessionId` at
    /// `subagent-runner.ts:2088`).
    ///
    /// Recorded so a LATER session reading the same async root can tell whose runs these are: pi's
    /// `listAsyncRuns` drops every on-disk run whose `sessionId` differs from the caller's
    /// (`async-status.ts:432`), which is what keeps `/subagents-fleet` and the active-run listings
    /// scoped to the current session instead of showing every run the project ever launched.
    /// `None` for a run launched with no resolvable parent-session anchor (a headless or
    /// unpersisted orchestrator), and for a status synthesized by reconciliation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Which shape of run this is.
    pub mode: RunMode,
    /// Current overall lifecycle state (monotone-forward, see [`RunState`]).
    pub state: RunState,
    /// The detached runner's own OS pid, once known. `None` only in the brief window between
    /// spawn-call-site provisional-status synthesis (R-SA-090) and the runner's own first real
    /// status write, if the caller chooses not to pre-populate a known pid (the reference spawn
    /// call site always knows the pid immediately from `Child::id()`, so in practice this is
    /// `Some` from the very first write; the `Option` exists for the reconciliation code path,
    /// `background/reconcile.rs`, which may synthesize a status for a run whose pid was never
    /// successfully recorded).
    pub pid: Option<u32>,
    /// The working directory this run's steps actually execute in (pi `AsyncStatus.cwd`,
    /// `shared/types.ts:592`), set once by the detached runner at its very first status write and
    /// never changed thereafter (a run's cwd is fixed for its whole lifetime). `resume`'s
    /// terminal-revival branch (R-SA-085, `Self::revive_from_transcript`) reads this back so a
    /// revived child spawns in the SAME directory the original run did, rather than whatever cwd
    /// happens to be current at resume time (pi `target.cwd ?? requestCwd`,
    /// `background/async-resume.ts:323,345,373` @v0.34.0 + `subagent-executor.ts:890`). `None` only for a
    /// synthesized/repaired status that never had a chance to observe the real value (mirrors
    /// `pid`'s own `None` carve-out immediately above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The run-wide persisted session-transcript path, once known (pi `AsyncStatus.sessionFile`,
    /// `shared/types.ts:636`, populated at `subagent-runner.ts:2411` once the run's effective
    /// session file is resolved). Read back by the SAME terminal-revival branch as
    /// [`Self::cwd`] as the run-level fallback beneath a resolved step's own `sessionFile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    /// Wall-clock spawn-confirmed time (epoch milliseconds).
    pub started_at: i64,
    /// Wall-clock terminal time (epoch milliseconds), set once `state` becomes `Complete`/
    /// `Failed`.
    pub ended_at: Option<i64>,
    /// Wall-clock time of the most recent status write — the value long-staleness reconciliation
    /// (R-SA-091) compares against its threshold.
    pub last_update: i64,
    /// Index of the step currently running (or about to run), if any.
    pub current_step: Option<usize>,
    /// Total number of steps currently known for a `Chain` run (grows as append-requests are
    /// consumed, R-SA-095).
    pub chain_step_count: Option<usize>,
    /// Count of still-unconsumed chain-append requests, recomputed by directory listing each time
    /// it changes (R-SA-094) — never a separately maintained counter that could drift from the
    /// actual `append-requests/` directory contents. This module does not itself recompute the
    /// count (that requires a filesystem read, owned by `background/control.rs`); it only carries
    /// the field.
    pub pending_appends: Option<usize>,
    /// Per-step status, in step order.
    pub steps: Vec<StepStatus>,
    /// Per-parallel-group child status, for any `ParallelGroup`/`DynamicGroup` steps.
    pub parallel_groups: Option<Vec<ParallelGroupStatus>>,
    /// SUBA-057 — pi `AsyncStatus.displayDismissedAt?: number` (`shared/types.ts:1293-1294`
    /// @v0.47.1, whose own doc comment reads *"Display-only dismissal marker for a reload-orphaned
    /// workflow."*). Epoch milliseconds, stamped by
    /// [`SubagentExecutor::control_dismiss`](crate::extension::SubagentExecutor::control_dismiss)
    /// (pi `dismissRecoveredWorkflow`, `runs/foreground/async-dismiss-action.ts:66`).
    ///
    /// **Display-only, and terminates nothing.** Its whole contract is that three readers honour it
    /// and one erases it:
    ///
    /// * [`crate::background::reconcile::reconcile`] returns
    ///   [`ReconcileAction::DisplayDismissed`](crate::background::reconcile::ReconcileAction::DisplayDismissed)
    ///   for a dismissed run instead of a normal outcome — pi's `return { status: null, … }`
    ///   (`stale-run-reconciler.ts:359-361`);
    /// * [`crate::background::run_status::list_active_runs`] drops it from the active set (pi
    ///   `async-status.ts:455-458`), which is what makes it vanish from `/subagents-fleet` and
    ///   `{action:"status"}`;
    /// * the single-run status view renders the `State: display-dismissed` report instead (pi
    ///   `run-status.ts:332-345`);
    /// * and a genuine terminal repair from a [`ResultFile`] **clears** it (pi `delete
    ///   terminalStatus.displayDismissedAt`, `stale-run-reconciler.ts:169`), so a dismissed run
    ///   whose result lands later comes back with its real outcome rather than staying hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_dismissed_at: Option<i64>,
    /// Run-wide live activity roll-ups + the workflow-graph snapshot (pi's top-level
    /// `statusPayload` telemetry, `subagent-runner.ts:2085-2120`) — flattened so its members
    /// serialize at the same top level of `status.json` pi writes them at.
    #[serde(flatten, default)]
    pub telemetry: RunTelemetry,
}

impl RunStatus {
    /// Constructs the initial `Queued` status for a freshly minted run, before the detached
    /// runner has written anything of its own. `started_at`/`last_update` are both set to "now".
    #[must_use]
    pub fn queued(run_id: RunId, mode: RunMode, pid: Option<u32>) -> Self {
        let now = crate::time::now_epoch_millis();
        Self {
            run_id,
            // The launching session is the runner's to record (it is the only process that knows
            // the anchor); a status built here carries none until it does.
            session_id: None,
            mode,
            state: RunState::Queued,
            pid,
            cwd: None,
            session_file: None,
            started_at: now,
            ended_at: None,
            last_update: now,
            current_step: None,
            chain_step_count: None,
            pending_appends: None,
            steps: Vec::new(),
            parallel_groups: None,
            // SUBA-057: a freshly minted run has never been display-dismissed.
            display_dismissed_at: None,
            telemetry: RunTelemetry::default(),
        }
    }

    /// Roll the per-step telemetry of the step at `flat_index` up into the top-level
    /// [`RunStatus::telemetry`] fields, mirroring pi's `syncTopLevelCurrentTool` +
    /// `statusPayload.toolCount`/`turnCount`/`totalTokens`/`lastActivityAt` maintenance
    /// (`subagent-runner.ts:2085-2120`). Recomputes `current_tool` from whichever step is currently
    /// running (a step with a live `current_tool`), sums `tool_count`/`total_tokens` across every
    /// step, and takes the max `turn_count` — so the roll-up is always internally consistent with
    /// the per-step fields rather than a separately drifting counter.
    pub fn sync_top_level_telemetry(&mut self, flat_index: usize) {
        self.current_step = Some(flat_index);
        let mut tool_total: u64 = 0;
        let mut turn_max: u64 = 0;
        let mut tokens = TokenTotals::default();
        let mut current_tool: Option<String> = None;
        let mut last_activity: Option<i64> = self.telemetry.last_activity_at;
        for step in &self.steps {
            tool_total = tool_total.saturating_add(step.telemetry.tool_count.unwrap_or(0));
            turn_max = turn_max.max(step.telemetry.turn_count.unwrap_or(0));
            if let Some(step_tokens) = step.telemetry.tokens {
                tokens.add(step_tokens.input, step_tokens.output);
            }
            if current_tool.is_none()
                && let Some(tool) = &step.telemetry.current_tool
            {
                current_tool = Some(tool.clone());
            }
            if let Some(activity) = step.telemetry.last_activity_at {
                last_activity = Some(last_activity.map_or(activity, |prev| prev.max(activity)));
            }
        }
        self.telemetry.current_tool = current_tool;
        self.telemetry.tool_count = (tool_total > 0).then_some(tool_total);
        self.telemetry.turn_count = (turn_max > 0).then_some(turn_max);
        self.telemetry.total_tokens = (tokens.total > 0).then_some(tokens);
        self.telemetry.last_activity_at = last_activity;
    }

    /// Constructs the **provisional, synthesized** status the spawn call site MUST supply for the
    /// R-SA-090 grace window: covers the short interval immediately after a confirmed successful
    /// hop-1 spawn but before the detached runner itself has had a chance to write its own real
    /// `status.json`. A reader (`background/reconcile.rs`) that finds no `status.json` on disk yet
    /// falls back to synthesizing a value equivalent to this constructor's output rather than
    /// declaring the run failed outright, provided it is still within the grace-window duration —
    /// the actual elapsed-time comparison against that duration is `reconcile.rs`'s job (this
    /// constructor only shapes the value, it does not itself know "how long ago" `started_at` was
    /// relative to "now").
    ///
    /// Identical to [`RunStatus::queued`] except documented as specifically the provisional-status
    /// shape a caller synthesizes rather than one persisted to disk by the runner itself — kept as
    /// a distinct, separately named constructor (rather than reusing `queued` silently) so call
    /// sites read as self-documenting about which of the two situations (real initial state vs.
    /// synthesized grace-window placeholder) they are in.
    #[must_use]
    pub fn provisional(run_id: RunId, mode: RunMode, pid: u32) -> Self {
        Self::queued(run_id, mode, Some(pid))
    }

    /// Attempts to advance `self.state` to `next`, updating `last_update` (and `ended_at` if
    /// `next` is terminal) on success. Returns [`RunStateTransitionError`] — leaving `self`
    /// entirely unmodified — if the transition is not permitted by
    /// [`RunState::can_transition_to`]. This is the sole sanctioned way to mutate `self.state`
    /// (see the enforcement note on [`RunState`] itself).
    ///
    /// # Errors
    ///
    /// Returns [`RunStateTransitionError`] if `next` is not reachable from `self.state`.
    pub fn advance_state(&mut self, next: RunState) -> Result<(), RunStateTransitionError> {
        let advanced = self.state.try_advance(next)?;
        self.state = advanced;
        let now = crate::time::now_epoch_millis();
        self.last_update = now;
        if advanced.is_terminal() {
            self.ended_at = Some(now);
        }
        Ok(())
    }

    /// Touches `last_update` to "now" without changing `state` — for status writes triggered by
    /// sub-state-level progress (tool activity milestones, model fallback, chain-append
    /// acceptance) that R-SA-075 requires a persisted update for even though the overall `state`
    /// enum value itself is unchanged.
    pub fn touch(&mut self) {
        self.last_update = crate::time::now_epoch_millis();
    }
}

// =================================================================================================
// ResultFile (func-SA §4.5, R-SA-077/166)
// =================================================================================================

/// The on-disk terminal result record for one finished background run (func-SA §4.5; arch-SA
/// §3.6). Written **exactly once**, at process exit, strictly **after** the final `status.json`
/// write (R-SA-077) — presence of this file in `ResultsDir` is the single authoritative "truly
/// done" signal for the run, decoupled from (and outranking) whatever `status.json` currently
/// claims.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFile {
    /// Identical to `run_id` — func-SA §4.5 documents both fields on the wire schema (the
    /// duplication mirrors the ported source format rather than being invented here); kept as two
    /// fields rather than collapsed to preserve exact on-disk shape compatibility.
    pub id: RunId,
    /// This run's identity.
    pub run_id: RunId,
    /// The top-level agent name for the run (for a `Chain`/`Parallel` run, the entry-point
    /// agent).
    pub agent: String,
    /// Which shape of run this was.
    pub mode: RunMode,
    /// The run's final lifecycle state at the moment this file was written — always terminal
    /// (`Complete`/`Failed`) or `Paused` (an interrupt-induced early stop is still recorded via a
    /// result file per R-SA-084's soft-pause semantics, not withheld until an eventual resume).
    pub state: RunState,
    /// Whether the run is considered an overall success. Distinct from `state == Complete`
    /// because a `Complete` run whose steps individually failed acceptance is still `success:
    /// false` — this field is the OR'd/derived signal R-SA-100 requires downstream consumers to
    /// have available without re-deriving it from `steps`.
    pub success: bool,
    /// The working directory the run executed in.
    pub cwd: PathBuf,
    /// Path to the top-level persisted session transcript, if any (fork-context or a
    /// session-persisting fresh run).
    pub session_file: Option<PathBuf>,
    /// Per-child results, in the same fixed order as [`RunStatus::steps`] (R-SA-051 ordering
    /// preserved).
    pub results: Vec<SingleResult>,
}

// =================================================================================================
// RunDir / RunPaths / AsyncRoot / ResultsDir (func-SA §4.5)
// =================================================================================================

/// The filesystem directory, keyed by run id, holding one background run's `status.json`,
/// `events.jsonl`, control-inbox files, append-request files, output/log files, and (once
/// terminal) its human-readable run-log — everything **except** the terminal [`ResultFile`]
/// itself, which lives in the sibling `ResultsDir` (func-SA §4.5 draws this distinction
/// deliberately: presence-in-`ResultsDir` being the sole "truly done" signal only works if the
/// result file is *not* just another entry inside the run's own, still-being-written directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDir(PathBuf);

impl RunDir {
    /// The directory for `run_id` underneath `async_root`. Pure path arithmetic — does not touch
    /// the filesystem (creating the directory via `mkdir` at spawn time is
    /// `spawn_detached.rs`'s job, per R-SA-072's "no pre-flight uniqueness check", not this
    /// constructor's).
    #[must_use]
    pub fn new(async_root: &Path, run_id: &RunId) -> Self {
        Self(async_root.join(run_id.as_str()))
    }

    /// Borrows the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for RunDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// All well-known file/subdirectory paths within one run's [`RunDir`], plus the sibling terminal
/// [`ResultFile`] path in `ResultsDir` (func-SA §4.5's full layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPaths {
    /// The run's own directory.
    pub run_dir: PathBuf,
    /// `<run_dir>/status.json` — [`RunStatus`], atomic writes only.
    pub status: PathBuf,
    /// `<run_dir>/events.jsonl` — append-only, size-capped event log (R-SA-136/146). Any writer
    /// appending to this path MUST go through [`crate::jsonl::BoundedJsonlWriter`] (the same
    /// shared primitive [`crate::spawn::SpawnedChild`]'s own per-attempt `.jsonl` tee uses) so the
    /// 50MB-default byte-budget cap is enforced identically here, not re-implemented per writer.
    pub events: PathBuf,
    /// `<run_dir>/control/interrupt.json` — present only while an [`crate::error::SubagentError`]-
    /// free pending interrupt request exists (R-SA-081); a later phase's `InterruptRequest` type
    /// lives in `background/control.rs`, not here.
    pub control_inbox: PathBuf,
    /// `<run_dir>/append-requests/` — directory of pending `ChainAppendRequest` files (R-SA-095).
    pub append_dir: PathBuf,
    /// `<run_dir>/runner.stdout.log` — the detached runner's own raw stdout.
    pub runner_stdout_log: PathBuf,
    /// `<run_dir>/runner.stderr.log` — the detached runner's own raw stderr.
    pub runner_stderr_log: PathBuf,
    /// `<run_dir>/<run_id>.md` — human-readable run-log summary.
    pub run_log_md: PathBuf,
    /// `<ResultsDir>/<run_id>.json` — the terminal [`ResultFile`]; presence is authoritative
    /// "done" (R-SA-077).
    pub result: PathBuf,
}

impl RunPaths {
    /// Derives every well-known path for `run_id` from `async_root` (holding the run's own
    /// directory and its contents) and `results_dir` (holding the sibling terminal result file).
    /// Pure path arithmetic only — never touches the filesystem.
    #[must_use]
    pub fn for_run(async_root: &Path, results_dir: &Path, run_id: &RunId) -> Self {
        let run_dir = RunDir::new(async_root, run_id);
        let dir = run_dir.as_path().to_path_buf();
        Self {
            status: dir.join("status.json"),
            events: dir.join("events.jsonl"),
            control_inbox: dir.join("control").join("interrupt.json"),
            append_dir: dir.join("append-requests"),
            runner_stdout_log: dir.join("runner.stdout.log"),
            runner_stderr_log: dir.join("runner.stderr.log"),
            run_log_md: dir.join(format!("{}.md", run_id.as_str())),
            result: results_dir.join(format!("{}.json", run_id.as_str())),
            run_dir: dir,
        }
    }

    /// Per-step output-log path, `<run_dir>/output-<n>.log` (func-SA §4.5).
    #[must_use]
    pub fn step_output_log(&self, step_index: usize) -> PathBuf {
        self.run_dir.join(format!("output-{step_index}.log"))
    }

    /// The distinct storage subpath for a nested background run keyed under this (root) run's id
    /// (R-SA-104's "SHOULD use a distinct storage subpath keyed under the root run's id"): a
    /// nested run started by a step of this run gets its own `run_dir`/`status`/etc. underneath
    /// `<this run's dir>/nested/<nested_run_id>/` rather than sharing a flat `AsyncRoot` slot
    /// indistinguishable from a top-level run. Still resolved as a plain [`RunPaths`] (nested runs
    /// use the identical on-disk shape as a root run — R-SA-104 is a storage-*location* nesting
    /// rule, not a schema difference) but with `results_dir` also nested underneath the parent,
    /// consistent with keeping a nested run's terminal-result "done" signal scoped under its
    /// parent's own tree rather than mixed into the shared top-level `ResultsDir`.
    #[must_use]
    pub fn nested(&self, nested_run_id: &RunId) -> Self {
        let nested_root = self.run_dir.join("nested");
        let nested_results = nested_root.join("results");
        Self::for_run(&nested_root, &nested_results, nested_run_id)
    }
}

// =================================================================================================
// Shared async-root / results-dir derivation + ensureAccessibleDir-equivalent (C7)
// =================================================================================================
//
// C7 root cause: the orchestrator (`extension.rs`) and the detached runner
// (`crates/cyrup/src/subagent_runner_cmd.rs`) each derived the run's `ResultsDir` independently and
// arrived at DIFFERENT directories, so every real background run's terminal `ResultFile` write
// targeted a directory the orchestrator never created (and never watched) — the run appeared to
// hang forever from the orchestrator's point of view. This section is the single shared source of
// truth both sides now agree on: the orchestrator derives the two roots here, creates them, and
// bakes their ABSOLUTE paths into `RunnerConfig` (`runner_main::RunnerConfig::async_root`/
// `results_dir`); the runner then rebuilds its `RunPaths` from those exact absolute roots rather
// than re-deriving them from the config-file path's own directory structure. Mirrors pi, where the
// orchestrator computes `resultPath`/`asyncDir` and passes them verbatim in the runner config
// (`async-execution.ts:701,966` @v0.34.0) and the runner reads them straight back
// (`subagent-runner.ts:1316` @v0.34.0) — never re-deriving `RESULTS_DIR`.

/// Path segment, under [`temp_root_dir`], holding one directory per background run (each
/// run's `status.json`, `events.jsonl`, control inbox, logs — everything EXCEPT the terminal
/// [`ResultFile`]). pi's `ASYNC_DIR` leaf is `"async-subagent-runs"`
/// (`shared/types.ts:1863` @v0.43.0); cyrup shortens it because the `<cwd_key>` level below it
/// already disambiguates, and `results_dir_for_async_root` pins the two leaves against each other.
const ASYNC_SUBDIR: &str = "async";

/// Path segment, under [`temp_root_dir`], holding the terminal [`ResultFile`] for every
/// run (a flat `<run_id>.json` per finished run). A DELIBERATE SIBLING of [`ASYNC_SUBDIR`], never a
/// child of it — "presence in this dir is the authoritative done signal" (R-SA-077) only works if
/// the results dir can be watched independently of the still-being-written run dir. Mirrors pi's
/// `RESULTS_DIR` leaf, `"async-subagent-results"` (`shared/types.ts:1862` @v0.43.0), shortened
/// for the same reason as [`ASYNC_SUBDIR`].
const RESULTS_SUBDIR: &str = "results";

/// One segment of a temp-scope id, with every character outside `[A-Za-z0-9._-]` collapsed to a
/// single `-` and leading/trailing `-` stripped; an empty result becomes `"unknown"`.
///
/// 1:1 with pi's `sanitizeTempScopeSegment` (`shared/types.ts:1807-1812` @v0.43.0):
/// `value.trim().replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown"`. The `+`
/// on the character class is why a RUN of illegal characters collapses to ONE `-` rather than one
/// per character.
fn sanitize_temp_scope_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(ch);
        } else {
            // Collapse a run of illegal characters to a single `-`, emitted lazily so a trailing
            // run never lands (matching the `replace(/-+$/g, "")` that follows upstream).
            pending_dash = !out.is_empty();
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The per-user scope segment that keeps two users' subagent scratch trees from colliding inside a
/// world-writable OS temp dir.
///
/// 1:1 with pi's `resolveTempScopeId` (`shared/types.ts:1814-1857` @v0.43.0), in its own precedence
/// order: the real uid (`uid-<n>`) first, then the first non-empty of `USERNAME`/`USER`/`LOGNAME`
/// (`user-<name>`), then the OS user info (`user-<name>`), then `USERPROFILE`/`HOME`
/// (`home-<path>`), then the OS home dir (`home-<path>`), and finally the literal `"shared"`.
///
/// [CYRUP-DELTA] cyrup stops at the uid branch on Unix and at the env branches elsewhere: pi's
/// `os.userInfo()` step exists because Node's `process.getuid` is undefined on Windows, and the
/// stdlib exposes no portable `userInfo` equivalent. Every branch upstream can actually reach on a
/// platform cyrup supports is present, so the resolved value is identical.
fn resolve_temp_scope_id() -> &'static str {
    /// The scope id cannot change within a process (the uid cannot), and [`temp_root_dir`] is on
    /// the path of every run-root derivation — without this each one re-read `/proc/self/status`.
    /// Matches upstream's own once-per-process evaluation: pi's `TEMP_ROOT_DIR` is a module-level
    /// `const`, so `resolveTempScopeId()` runs exactly once per process there too.
    static SCOPE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SCOPE_ID.get_or_init(resolve_temp_scope_id_uncached)
}

fn resolve_temp_scope_id_uncached() -> String {
    // pi `if (typeof getuid === "function") return `uid-${getuid()}`` — `process.getuid` is defined
    // on every Unix, so upstream never reaches a later branch there. Read from procfs because this
    // crate is `#![forbid(unsafe_code)]` and cannot call `libc::getuid`.
    if let Some(uid) = real_uid() {
        return format!("uid-{uid}");
    }
    // pi's second branch: the first non-empty of USERNAME/USER/LOGNAME, in that order.
    for key in ["USERNAME", "USER", "LOGNAME"] {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return format!("user-{}", sanitize_temp_scope_segment(&value.to_string_lossy()));
        }
    }
    // pi's fourth branch (`os.userInfo()`, its third, has no safe stdlib equivalent):
    // `env.USERPROFILE ?? env.HOME`.
    for key in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return format!("home-{}", sanitize_temp_scope_segment(&value.to_string_lossy()));
        }
    }
    // pi's last resort, verbatim.
    "shared".to_string()
}

/// The calling process's REAL uid (not the effective one), or `None` where it cannot be read
/// without `unsafe`.
///
/// `/proc/self/status`'s `Uid:` line is `Uid:\t<real>\t<effective>\t<saved>\t<fs>`; field 1 is the
/// real uid, which is what `process.getuid()` returns. Returns `None` on a platform with no procfs
/// (macOS, the BSDs, Windows), where [`resolve_temp_scope_id`] falls through to pi's own next
/// branch rather than inventing a constant that would collide across users.
fn real_uid() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    let real = line.split_whitespace().nth(1)?;
    if real.bytes().all(|b| b.is_ascii_digit()) {
        Some(real.to_string())
    } else {
        None
    }
}

/// The per-user root every subagent run-artifact directory hangs off:
/// `<os-temp-dir>/cyrup-subagents-<scope>`.
///
/// # This is a scratch root in the OS temp dir, NOT the user's home
///
/// pi puts all four of its run-scratch roots under `os.tmpdir()`:
///
/// ```text
/// TEMP_ROOT_DIR      = path.join(os.tmpdir(), `pi-subagents-${resolveTempScopeId()}`)
/// RESULTS_DIR        = path.join(TEMP_ROOT_DIR, "async-subagent-results")
/// ASYNC_DIR          = path.join(TEMP_ROOT_DIR, "async-subagent-runs")
/// CHAIN_RUNS_DIR     = path.join(TEMP_ROOT_DIR, "chain-runs")
/// TEMP_ARTIFACTS_DIR = path.join(TEMP_ROOT_DIR, "artifacts")
/// ```
///
/// (`shared/types.ts:1862-1866` @v0.43.0; byte-identical at the ported baseline —
/// `shared/types.ts:1097-1101` @v0.33.0 and `:1104-1108` @v0.34.0.)
///
/// This port previously resolved `<CYRUP_HOME|HOME>/.cyrup/subagents` instead, which is where the
/// 59,321-file / 551 MB pile in a developer's real `~/.cyrup/subagents` came from: run scratch that
/// upstream treats as reboot-disposable was being written into permanent user config, per-`cwd`
/// keyed, with nothing ever sweeping it. The two doc citations that justified the old layout were
/// both wrong — `shared/types.ts:958` and `:959` are fields of a run-input interface, not the
/// `RESULTS_DIR`/`ASYNC_DIR` constants.
///
/// `std::env::temp_dir()` is the exact analog of Node's `os.tmpdir()`: both read `TMPDIR` and both
/// fall back to `/tmp`. That is also the ONLY sandbox seam either side has — pi's `DIRS` are
/// module-level constants, so upstream scopes tests by passing explicit `asyncDirRoot`/`resultsDir`
/// options (`async-job-tracker.ts:57`, `async-resume.ts:385`, `fleet-view.ts:326` @v0.43.0) rather
/// than by moving this root.
///
/// # `CYRUP_HOME` is the sandbox seam, and only that
///
/// When `CYRUP_HOME` is set the whole tree relocates to `<CYRUP_HOME>/.cyrup/subagents`. That var
/// is cyrup-original — pi has no `PI_HOME` — so there is no upstream behaviour to diverge from, and
/// its meaning here is the same as in the crate's five other resolvers: "the root every cyrup path
/// resolves against". No production code path in this workspace sets it (`grep -rn CYRUP_HOME
/// crates/*/src` finds only resolvers and docs), so with it unset — its only state outside tests —
/// this function is pi's `TEMP_ROOT_DIR` exactly.
///
/// It earns its place because it is the ONE knob the crate's 19 already-`CYRUP_HOME`-sandboxed
/// integration tests set: honouring it here is what keeps their `TempDir` isolation covering the
/// run-scratch tree instead of letting them pile into the shared real temp root. Upstream's own
/// equivalent is passing explicit `asyncDirRoot`/`resultsDir` options (`async-job-tracker.ts:57`,
/// `async-resume.ts:385`, `fleet-view.ts:326` @v0.43.0) — pi's `DIRS` are module-level constants
/// that cannot be re-scoped by env at all.
///
/// `pub(crate)` so the artifacts/chain-runs housekeeping ([`crate::artifacts`]) can scope its own
/// per-`cwd` roots under the SAME temp root the async/results roots use, rather than re-deriving
/// (and risking drift from) this one resolution.
pub(crate) fn temp_root_dir() -> PathBuf {
    temp_root_dir_from(&|key| std::env::var(key).ok(), std::env::temp_dir())
}

/// The pure core of [`temp_root_dir`], with the two ambient inputs — the environment and the OS
/// temp dir — passed in, so both branches are provable without mutating process-global state.
/// Follows the crate's existing `native_supervisor::intercom_agent_dir_from` convention.
fn temp_root_dir_from(env: &dyn Fn(&str) -> Option<String>, os_temp_dir: PathBuf) -> PathBuf {
    if let Some(sandbox) = env("CYRUP_HOME").filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(sandbox).join(".cyrup").join("subagents");
    }
    os_temp_dir.join(format!("cyrup-subagents-{}", resolve_temp_scope_id()))
}

/// A filesystem-safe key derived from `cwd`, so distinct projects' async/result roots never collide
/// under the shared per-user [`temp_root_dir`] tree.
///
/// [CYRUP-DELTA] pi's `ASYNC_DIR`/`RESULTS_DIR` are FLAT — every project's runs share one directory
/// (`shared/types.ts:1863-1864` @v0.43.0), and a run is disambiguated only by its run id. cyrup
/// interposes this `cwd` key so `resume_tracking`'s `read_dir` over the async root cannot re-adopt
/// a run belonging to a different checkout.
///
/// `pub(crate)` for the same reason as [`temp_root_dir`]: [`crate::artifacts`] keys its
/// artifacts/chain-runs roots by the identical `cwd_key` so a project's artifacts sit beside its
/// async/results dirs under one per-`cwd` scope.
pub(crate) fn cwd_key(cwd: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The two sibling run-artifact roots for one working directory (C7): the `async_root` holding
/// per-run directories and the `results_dir` holding terminal [`ResultFile`]s. Both are always
/// keyed by the same `cwd` so a run's directory and its result file are guaranteed to belong to the
/// same project scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunArtifactRoots {
    /// `<temp_root_dir>/async/<cwd_key>` — passed as `RunPaths::for_run`'s `async_root`.
    pub async_root: PathBuf,
    /// `<temp_root_dir>/results/<cwd_key>` — passed as `RunPaths::for_run`'s `results_dir`.
    pub results_dir: PathBuf,
}

/// THE single derivation of the per-`cwd` async-root and results-dir that both the orchestrator
/// (at spawn time, `extension.rs`) and the runner (transitively, via the absolute paths this
/// function's output is baked into `RunnerConfig` as) agree on — the fix for C7's divergent
/// derivations. Pure path arithmetic; never touches the filesystem (creation is
/// [`ensure_accessible_dir`]'s job).
#[must_use]
pub fn run_artifact_roots(cwd: &Path) -> RunArtifactRoots {
    let home = temp_root_dir();
    let key = cwd_key(cwd);
    RunArtifactRoots {
        async_root: home.join(ASYNC_SUBDIR).join(&key),
        results_dir: home.join(RESULTS_SUBDIR).join(&key),
    }
}

/// Reconstruct the SIBLING results-dir for an `async_root` produced by [`run_artifact_roots`],
/// purely structurally (no `cwd`/env re-read). Given the standard layout
/// `<temp_root_dir>/async/<cwd_key>`, returns `<temp_root_dir>/results/<cwd_key>` —
/// i.e. it swaps the [`ASYNC_SUBDIR`] path segment for [`RESULTS_SUBDIR`] while PRESERVING the
/// `<cwd_key>` leaf, which is exactly what C7's pre-fix `async_root.parent()/results` derivation got
/// wrong (it dropped the `<cwd_key>` and nested `results` UNDER `async` instead of beside it).
///
/// This exists so the runner's config-path-structure fallback (used ONLY on the pre-config-read
/// error path in `crates/cyrup/src/subagent_runner_cmd.rs`, where no authoritative
/// `RunnerConfig::results_dir` has been read yet) still targets the SAME results dir the
/// orchestrator created. For a non-standard `async_root` that does not match the
/// `<...>/async/<key>` shape (e.g. a bare `<base>/async` used by lower-level unit fixtures), it
/// degrades to a `results` sibling of `async_root`'s own parent.
#[must_use]
pub fn results_dir_for_async_root(async_root: &Path) -> PathBuf {
    let parent = async_root.parent();
    let is_standard_layout = parent
        .and_then(Path::file_name)
        .is_some_and(|name| name == std::ffi::OsStr::new(ASYNC_SUBDIR));
    if is_standard_layout
        && let (Some(home), Some(key)) = (parent.and_then(Path::parent), async_root.file_name())
    {
        return home.join(RESULTS_SUBDIR).join(key);
    }
    async_root
        .parent()
        .unwrap_or(async_root)
        .join(RESULTS_SUBDIR)
}

/// `ensureAccessibleDir`-equivalent (pi `extension/index.ts:97-110`): create `dir` (and every
/// missing parent), then verify it is actually a READ+WRITE-accessible directory. On the rare
/// platform edge pi guards against — a directory created shortly after wake-from-sleep on Windows
/// with Azure AD/Entra ID can end up with a broken null DACL that makes it inaccessible to its own
/// creator — the directory is dropped and recreated once before giving up.
///
/// Called on BOTH sides of C7: the orchestrator ensures `async_root`/`results_dir` at spawn time,
/// and the runner ensures `results_dir` again immediately before the terminal [`ResultFile`] write
/// (so the authoritative "done" signal always lands even if the orchestrator's own creation was
/// skipped or the dir was since removed).
///
/// # Errors
///
/// Returns the underlying `io::Error` if the directory cannot be created, or a
/// [`std::io::ErrorKind::PermissionDenied`] error if it still fails the read+write accessibility
/// probe after a recreate attempt.
pub async fn ensure_accessible_dir(dir: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    if probe_dir_accessible(dir).await {
        return Ok(());
    }
    // Broken-ACL recovery (Windows Azure-AD null-DACL case): drop and recreate once. A cleanup
    // failure is deliberately best-effort — retry the mkdir/probe regardless, mirroring pi.
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir).await?;
    if probe_dir_accessible(dir).await {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "directory is not read/write accessible after recreate: {}",
                dir.display()
            ),
        ))
    }
}

/// Probe `dir` for read+write access the way pi's `fs.accessSync(R_OK | W_OK)` does, but portably:
/// confirm it is a listable directory (read) and that a uniquely-named probe file can be created
/// and removed inside it (write). Any failure returns `false`, which drives
/// [`ensure_accessible_dir`]'s recreate-once recovery.
async fn probe_dir_accessible(dir: &Path) -> bool {
    match tokio::fs::metadata(dir).await {
        Ok(meta) if meta.is_dir() => {}
        _ => return false,
    }
    let probe = dir.join(format!(
        ".cyrup-access-probe-{}",
        uuid::Uuid::new_v4().as_simple()
    ));
    match tokio::fs::write(&probe, b"").await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(&probe).await;
            true
        }
        Err(_) => false,
    }
}

// =================================================================================================
// Workflow-graph snapshot (pi `runs/shared/workflow-graph.ts:73-206`)
// =================================================================================================
//
// A faithful port of pi's `buildWorkflowGraphSnapshot`: given a run's declared step list plus its
// per-step results/statuses, produces the node-id/phase/group-status-precedence/dynamic-metadata/
// `currentNodeId` snapshot a status-reading UI renders (`shared/types.ts:33-65`). Node ids are the
// exact pi shapes: `step-<N>` for a sequential step or a group, `step-<N>-agent-<M>` for a static
// parallel child, `step-<N>-item-<key>` for a dynamic fan-out child. This is a pure function over
// plain data — no filesystem, no discovery — so it reproduces `workflow-graph.test.ts` scenario for
// scenario.

/// A workflow node's lifecycle state (pi `WorkflowNodeStatus`, `shared/types.ts:40`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeStatus {
    /// Declared but not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with a failure.
    Failed,
    /// Interrupted mid-flight (soft pause).
    Paused,
    /// G77 — pi `WorkflowNodeStatus` includes `"stopped"` (`shared/types.ts:40`), and the detached
    /// runner's own graph refresh normalizes a `"stopped"` step onto it and rolls it up over its
    /// children (`subagent-runner.ts:2163-2188`). Distinct from [`Self::Failed`]: upstream's
    /// `updateNode` tail is `if (node.error && node.status !== "stopped" && node.status !==
    /// "rejected") node.status = "failed"` — i.e. a stopped node keeps its status even though the
    /// stop stamped an `error` onto it, precisely so a stop is never re-rendered as a failure.
    Stopped,
    /// Detached (fire-and-forget) and no longer tracked inline.
    Detached,
}

/// A workflow node's structural kind (pi `WorkflowGraphNode.kind`, `shared/types.ts:37`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowNodeKind {
    /// A single sequential step.
    Step,
    /// A static-width parallel group container.
    ParallelGroup,
    /// A dynamic (runtime-width) fan-out group container.
    DynamicParallelGroup,
    /// One concurrently-dispatched child agent within a group.
    Agent,
}

/// The run-shape tag on a [`WorkflowGraphSnapshot`] (pi `WorkflowGraphSnapshot.mode`,
/// `shared/types.ts:78`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunMode {
    /// A linear chain.
    Chain,
    /// A standalone static parallel fan-out.
    Parallel,
    /// One single agent invocation.
    Single,
}

/// The `dynamic` metadata block on a dynamic-fan-out group node (pi `WorkflowGraphNode.dynamic`,
/// `shared/types.ts:45-51`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDynamicMeta {
    /// The named output the fan-out expands from.
    pub source_output: String,
    /// The JSON-pointer path within that output the array lives at.
    pub source_path: String,
    /// The per-item variable name (`item` by default).
    pub item_name: String,
    /// The optional `maxItems` cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
    /// The named output the collected results register under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collect_as: Option<String>,
}

/// One node of a [`WorkflowGraphSnapshot`] (pi `WorkflowGraphNode`, `shared/types.ts:42-74`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGraphNode {
    /// The stable node id (`step-<N>` / `step-<N>-agent-<M>` / `step-<N>-item-<key>`).
    pub id: String,
    /// The node's structural kind.
    pub kind: WorkflowNodeKind,
    /// The agent this node invokes, for a step/agent node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The declared phase this node belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The human-readable label.
    pub label: String,
    /// The node's lifecycle status.
    pub status: WorkflowNodeStatus,
    /// The node's flat (execution-order) index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat_index: Option<usize>,
    /// The node's declared step index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
    /// The item key, for a dynamic fan-out child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_key: Option<String>,
    /// The named output this node's result registers under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_name: Option<String>,
    /// Whether this node produces a structured (schema-validated) output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<bool>,
    /// This node's acceptance-ledger status, when evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_status: Option<String>,
    /// This node's error text, when failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The dynamic-fan-out metadata, for a dynamic group node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<WorkflowDynamicMeta>,
    /// This node's children, for a group container (present, possibly empty, only for a group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<WorkflowGraphNode>>,
}

/// A named phase grouping node ids (pi `WorkflowGraphSnapshot.phases[]`, `shared/types.ts:62`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    /// The phase title.
    pub title: String,
    /// The ids of every node in this phase, in declaration order.
    pub node_ids: Vec<String>,
}

/// The full workflow-graph snapshot (pi `WorkflowGraphSnapshot`, `shared/types.ts:76-82`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGraphSnapshot {
    /// The run's id.
    pub run_id: String,
    /// The run shape.
    pub mode: WorkflowRunMode,
    /// The declared phases.
    pub phases: Vec<WorkflowPhase>,
    /// The nodes, in declaration/execution order.
    pub nodes: Vec<WorkflowGraphNode>,
    /// The id of the currently-active node, when one is determinable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_node_id: Option<String>,
}

/// One declared sequential step, or one static-parallel/dynamic child spec, as an input to
/// [`build_workflow_graph_snapshot`] (pi's `SequentialStep`/parallel-task/`DynamicParallelStep`
/// fields, projected to only what the graph builder reads).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowTaskSpec {
    /// The agent name.
    pub agent: Option<String>,
    /// The declared phase.
    pub phase: Option<String>,
    /// The declared label.
    pub label: Option<String>,
    /// The named output (`as`).
    pub output_name: Option<String>,
    /// Whether this task produces a structured output (`Boolean(outputSchema)`).
    pub structured: bool,
}

/// The dynamic-fan-out shape of one declared step (pi `DynamicParallelStep`, projected).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowDynamicStepSpec {
    /// `expand.from.output`.
    pub expand_from_output: String,
    /// `expand.from.path`.
    pub expand_from_path: String,
    /// `expand.item` (`item` by default).
    pub item_name: Option<String>,
    /// `expand.maxItems`.
    pub max_items: Option<usize>,
    /// The whole step's `phase`/`label` (fallbacks for the group node).
    pub step_phase: Option<String>,
    /// The whole step's `label`.
    pub step_label: Option<String>,
    /// `parallel.phase`/`parallel.label` (the per-item template's phase/label).
    pub template_phase: Option<String>,
    /// `parallel.label`.
    pub template_label: Option<String>,
    /// `collect.as`.
    pub collect_as: String,
    /// `Boolean(collect.outputSchema)`.
    pub collect_structured: bool,
}

/// One declared workflow step, as an input to [`build_workflow_graph_snapshot`] — the union
/// pi's `ChainStep` is (`isParallelStep`/`isDynamicParallelStep`/sequential, `settings.ts`).
#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowInputStep {
    /// A single sequential step.
    Sequential(WorkflowTaskSpec),
    /// A static-width parallel group over these tasks.
    Parallel(Vec<WorkflowTaskSpec>),
    /// A dynamic (runtime-width) fan-out.
    Dynamic(WorkflowDynamicStepSpec),
}

/// One materialized dynamic fan-out child, keyed by its resolved `itemKey`
/// (pi `WorkflowGraphBuildInput.dynamicChildren`, `workflow-graph.ts:12`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowDynamicChild {
    /// The child's agent.
    pub agent: String,
    /// The child's label override.
    pub label: Option<String>,
    /// The child's flat index.
    pub flat_index: usize,
    /// The resolved item key (sanitized into the node id).
    pub item_key: String,
    /// The child's named output.
    pub output_name: Option<String>,
    /// Whether the child produces a structured output.
    pub structured: bool,
    /// The child's own error text.
    pub error: Option<String>,
}

/// A per-step result summary the builder reads for status derivation (pi
/// `WorkflowGraphBuildInput.results[]`, `workflow-graph.ts:8`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowResultSummary {
    /// The child's exit code (`0` = success).
    pub exit_code: Option<i32>,
    /// Whether the child detached.
    pub detached: bool,
    /// Whether the child was interrupted (soft pause).
    pub interrupted: bool,
    /// The child's error text.
    pub error: Option<String>,
    /// The child's acceptance-ledger status.
    pub acceptance_status: Option<String>,
}

/// A per-step status override the builder reads (pi `WorkflowGraphBuildInput.stepStatuses[]`,
/// `workflow-graph.ts:11`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowStepStatusInput {
    /// The raw status string (`"complete"`/`"running"`/… normalized by the builder).
    pub status: Option<String>,
    /// The step's error text.
    pub error: Option<String>,
}

/// A dynamic-group status override (pi `WorkflowGraphBuildInput.dynamicGroupStatuses`,
/// `workflow-graph.ts:13`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDynamicGroupStatus {
    /// The forced group status (empty-skip `completed`, aggregate-failure `failed`, …).
    pub status: WorkflowNodeStatus,
    /// The group's error text.
    pub error: Option<String>,
    /// The group's acceptance-ledger status.
    pub acceptance_status: Option<String>,
}

/// The full input to [`build_workflow_graph_snapshot`] (pi `WorkflowGraphBuildInput`,
/// `workflow-graph.ts:4-14`).
#[derive(Clone, Debug, Default)]
pub struct WorkflowGraphBuildInput {
    /// The run's id.
    pub run_id: String,
    /// The run shape (`chain` by default, mirroring pi's `input.mode ?? "chain"`).
    pub mode: Option<WorkflowRunMode>,
    /// The declared steps.
    pub steps: Vec<WorkflowInputStep>,
    /// Per-flat-index result summaries.
    pub results: Vec<WorkflowResultSummary>,
    /// The currently-running flat index.
    pub current_flat_index: Option<usize>,
    /// The currently-running step index.
    pub current_step_index: Option<usize>,
    /// Per-flat-index status overrides.
    pub step_statuses: Vec<WorkflowStepStatusInput>,
    /// Materialized dynamic children, keyed by step index.
    pub dynamic_children: std::collections::BTreeMap<usize, Vec<WorkflowDynamicChild>>,
    /// Dynamic-group status overrides, keyed by step index.
    pub dynamic_group_statuses: std::collections::BTreeMap<usize, WorkflowDynamicGroupStatus>,
}

/// Normalize a raw status string to a [`WorkflowNodeStatus`] (pi `normalizeStatus`,
/// `workflow-graph.ts:16-34`) — `None` for an unrecognized value so the caller can fall through.
fn normalize_workflow_status(status: Option<&str>) -> Option<WorkflowNodeStatus> {
    match status {
        Some("complete" | "completed") => Some(WorkflowNodeStatus::Completed),
        Some("running") => Some(WorkflowNodeStatus::Running),
        Some("failed") => Some(WorkflowNodeStatus::Failed),
        Some("paused") => Some(WorkflowNodeStatus::Paused),
        // G77: `workflow-graph.ts`'s own `normalizeStatus` has no `"stopped"` case, but the
        // detached runner's graph refresh — which in cyrup is FUSED into this same builder via
        // [`build_workflow_graph_from_runner_steps`] rather than living as a second local
        // normalizer — does (`subagent-runner.ts:2163-2166`: `if (status === "running" || … ||
        // status === "stopped" || …) return status`). Without this arm a stopped step falls
        // through to the result-derived/`pending` default and the graph silently reports a stopped
        // node as pending-or-failed.
        Some("stopped") => Some(WorkflowNodeStatus::Stopped),
        Some("detached") => Some(WorkflowNodeStatus::Detached),
        Some("pending") => Some(WorkflowNodeStatus::Pending),
        _ => None,
    }
}

/// Derive a status from a per-step result (pi `resultStatus`, `workflow-graph.ts:36-41`).
fn workflow_result_status(result: Option<&WorkflowResultSummary>) -> Option<WorkflowNodeStatus> {
    let result = result?;
    if result.detached {
        return Some(WorkflowNodeStatus::Detached);
    }
    if result.interrupted {
        return Some(WorkflowNodeStatus::Paused);
    }
    Some(if result.exit_code == Some(0) {
        WorkflowNodeStatus::Completed
    } else {
        WorkflowNodeStatus::Failed
    })
}

/// Resolve a node's status (pi `nodeStatus`, `workflow-graph.ts:43-47`): step-status override,
/// then result-derived, then running-if-current, else pending.
fn workflow_node_status(input: &WorkflowGraphBuildInput, flat_index: usize) -> WorkflowNodeStatus {
    normalize_workflow_status(
        input
            .step_statuses
            .get(flat_index)
            .and_then(|s| s.status.as_deref()),
    )
    .or_else(|| workflow_result_status(input.results.get(flat_index)))
    .unwrap_or(if input.current_flat_index == Some(flat_index) {
        WorkflowNodeStatus::Running
    } else {
        WorkflowNodeStatus::Pending
    })
}

/// Push a node id under its phase, creating the phase group in first-seen order (pi `pushPhase`,
/// `workflow-graph.ts:49-57`).
fn push_workflow_phase(phases: &mut Vec<WorkflowPhase>, phase: Option<&str>, node_id: &str) {
    let Some(phase) = phase else { return };
    if let Some(group) = phases.iter_mut().find(|candidate| candidate.title == phase) {
        group.node_ids.push(node_id.to_string());
    } else {
        phases.push(WorkflowPhase {
            title: phase.to_string(),
            node_ids: vec![node_id.to_string()],
        });
    }
}

/// Summarize a parallel group's child statuses with pi's explicit precedence (pi
/// `summarizeParallelStatuses`, `workflow-graph.ts:65-74`): running > stopped > failed > paused >
/// detached > all-completed > any-completed(=running) > pending.
///
/// G77 — the `stopped` slot sits immediately after `running`, matching the detached runner's own
/// child roll-up (`subagent-runner.ts:2181-2186`: `every completed` → `some running` → `some
/// stopped` → `some rejected` → `some failed` → `some paused`), which is the roll-up this fused
/// builder serves for a background run. `workflow-graph.ts`'s standalone version predates the
/// state and has no such branch; adding it here cannot change any other caller's result, because
/// [`WorkflowNodeStatus::Stopped`] is only ever produced by a stopped step.
fn summarize_parallel_statuses(statuses: &[WorkflowNodeStatus]) -> WorkflowNodeStatus {
    if statuses.contains(&WorkflowNodeStatus::Running) {
        return WorkflowNodeStatus::Running;
    }
    if statuses.contains(&WorkflowNodeStatus::Stopped) {
        return WorkflowNodeStatus::Stopped;
    }
    if statuses.contains(&WorkflowNodeStatus::Failed) {
        return WorkflowNodeStatus::Failed;
    }
    if statuses.contains(&WorkflowNodeStatus::Paused) {
        return WorkflowNodeStatus::Paused;
    }
    if statuses.contains(&WorkflowNodeStatus::Detached) {
        return WorkflowNodeStatus::Detached;
    }
    if !statuses.is_empty() && statuses.iter().all(|s| *s == WorkflowNodeStatus::Completed) {
        return WorkflowNodeStatus::Completed;
    }
    if statuses.contains(&WorkflowNodeStatus::Completed) {
        return WorkflowNodeStatus::Running;
    }
    WorkflowNodeStatus::Pending
}

/// pi `seqLabel` (`workflow-graph.ts:59-61`): the step's trimmed label, else its agent, else
/// `Step <n>`.
fn seq_label(step: &WorkflowTaskSpec, step_index: usize) -> String {
    let trimmed = step.label.as_deref().map(str::trim).filter(|s| !s.is_empty());
    trimmed
        .map(str::to_string)
        .or_else(|| step.agent.clone().filter(|a| !a.is_empty()))
        .unwrap_or_else(|| format!("Step {}", step_index + 1))
}

/// Sanitize a dynamic item key into a node-id-safe token (pi's
/// `task.itemKey.replace(/[^a-zA-Z0-9_-]/g, "-")`, `workflow-graph.ts:157`).
fn sanitize_item_key(item_key: &str) -> String {
    item_key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
        .collect()
}

/// Build a [`WorkflowGraphSnapshot`] from declared steps + per-step results/statuses — the Rust port
/// of pi's `buildWorkflowGraphSnapshot` (`workflow-graph.ts:73-206`), reproducing its exact node
/// ids, phase grouping, group-status precedence, dynamic metadata, and `currentNodeId` selection.
#[must_use]
pub fn build_workflow_graph_snapshot(input: &WorkflowGraphBuildInput) -> WorkflowGraphSnapshot {
    let mut nodes: Vec<WorkflowGraphNode> = Vec::new();
    let mut phases: Vec<WorkflowPhase> = Vec::new();
    let mut flat_index = 0usize;
    let mut current_node_id: Option<String> = None;

    for (step_index, step) in input.steps.iter().enumerate() {
        match step {
            WorkflowInputStep::Parallel(tasks) => {
                let group_id = format!("step-{step_index}");
                let mut children: Vec<WorkflowGraphNode> = Vec::new();
                let mut child_statuses: Vec<WorkflowNodeStatus> = Vec::new();
                for (task_index, task) in tasks.iter().enumerate() {
                    let status = workflow_node_status(input, flat_index);
                    child_statuses.push(status);
                    let child_id = format!("step-{step_index}-agent-{task_index}");
                    let label = task
                        .label
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .or_else(|| task.agent.clone().filter(|a| !a.is_empty()))
                        .unwrap_or_else(|| format!("Agent {}", task_index + 1));
                    let child = WorkflowGraphNode {
                        id: child_id.clone(),
                        kind: WorkflowNodeKind::Agent,
                        agent: task.agent.clone(),
                        phase: task.phase.clone(),
                        label,
                        status,
                        flat_index: Some(flat_index),
                        step_index: Some(step_index),
                        item_key: None,
                        output_name: task.output_name.clone(),
                        structured: Some(task.structured),
                        acceptance_status: input
                            .results
                            .get(flat_index)
                            .and_then(|r| r.acceptance_status.clone()),
                        error: input
                            .step_statuses
                            .get(flat_index)
                            .and_then(|s| s.error.clone())
                            .or_else(|| input.results.get(flat_index).and_then(|r| r.error.clone())),
                        dynamic: None,
                        children: None,
                    };
                    push_workflow_phase(&mut phases, task.phase.as_deref(), &child_id);
                    if status == WorkflowNodeStatus::Running
                        || input.current_flat_index == Some(flat_index)
                    {
                        current_node_id = Some(child_id.clone());
                    }
                    children.push(child);
                    flat_index += 1;
                }
                let group_status = summarize_parallel_statuses(&child_statuses);
                if input.current_step_index == Some(step_index) && current_node_id.is_none() {
                    current_node_id = Some(group_id.clone());
                }
                nodes.push(WorkflowGraphNode {
                    id: group_id,
                    kind: WorkflowNodeKind::ParallelGroup,
                    agent: None,
                    phase: None,
                    label: if tasks.len() == 1 {
                        "Parallel task".to_string()
                    } else {
                        format!("Parallel group ({})", tasks.len())
                    },
                    status: group_status,
                    flat_index: None,
                    step_index: Some(step_index),
                    item_key: None,
                    output_name: None,
                    structured: None,
                    acceptance_status: None,
                    error: None,
                    dynamic: None,
                    children: Some(children),
                });
                continue;
            }
            WorkflowInputStep::Dynamic(dynamic) => {
                let group_id = format!("step-{step_index}");
                let materialized = input.dynamic_children.get(&step_index);
                let group_override = input.dynamic_group_statuses.get(&step_index);
                let mut children: Vec<WorkflowGraphNode> = Vec::new();
                let mut child_statuses: Vec<WorkflowNodeStatus> = Vec::new();
                if let Some(materialized) = materialized {
                    for task in materialized {
                        let status = workflow_node_status(input, task.flat_index);
                        child_statuses.push(status);
                        let child_id =
                            format!("step-{step_index}-item-{}", sanitize_item_key(&task.item_key));
                        let phase = dynamic.template_phase.clone().or_else(|| dynamic.step_phase.clone());
                        let label = task
                            .label
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .or_else(|| {
                                dynamic
                                    .template_label
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| format!("{} {}", task.agent, task.item_key));
                        let child = WorkflowGraphNode {
                            id: child_id.clone(),
                            kind: WorkflowNodeKind::Agent,
                            agent: Some(task.agent.clone()),
                            phase: phase.clone(),
                            label,
                            status,
                            flat_index: Some(task.flat_index),
                            step_index: Some(step_index),
                            item_key: Some(task.item_key.clone()),
                            output_name: task.output_name.clone(),
                            structured: Some(task.structured),
                            acceptance_status: input
                                .results
                                .get(task.flat_index)
                                .and_then(|r| r.acceptance_status.clone()),
                            error: input
                                .step_statuses
                                .get(task.flat_index)
                                .and_then(|s| s.error.clone())
                                .or_else(|| {
                                    input.results.get(task.flat_index).and_then(|r| r.error.clone())
                                })
                                .or_else(|| task.error.clone()),
                            dynamic: None,
                            children: None,
                        };
                        push_workflow_phase(&mut phases, phase.as_deref(), &child_id);
                        if status == WorkflowNodeStatus::Running
                            || input.current_flat_index == Some(task.flat_index)
                        {
                            current_node_id = Some(child_id.clone());
                        }
                        children.push(child);
                    }
                }
                let group_status = group_override.map(|o| o.status).unwrap_or_else(|| {
                    if children.is_empty() {
                        if input.current_step_index == Some(step_index) {
                            WorkflowNodeStatus::Running
                        } else {
                            WorkflowNodeStatus::Pending
                        }
                    } else {
                        summarize_parallel_statuses(&child_statuses)
                    }
                });
                if input.current_step_index == Some(step_index) && current_node_id.is_none() {
                    current_node_id = Some(group_id.clone());
                }
                let label = dynamic
                    .step_label
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        dynamic
                            .template_label
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| format!("Dynamic fanout ({})", dynamic.collect_as));
                nodes.push(WorkflowGraphNode {
                    id: group_id,
                    kind: WorkflowNodeKind::DynamicParallelGroup,
                    agent: None,
                    phase: None,
                    label,
                    status: group_status,
                    flat_index: None,
                    step_index: Some(step_index),
                    item_key: None,
                    output_name: Some(dynamic.collect_as.clone()),
                    structured: Some(dynamic.collect_structured),
                    acceptance_status: group_override.and_then(|o| o.acceptance_status.clone()),
                    error: group_override.and_then(|o| o.error.clone()),
                    dynamic: Some(WorkflowDynamicMeta {
                        source_output: dynamic.expand_from_output.clone(),
                        source_path: dynamic.expand_from_path.clone(),
                        item_name: dynamic.item_name.clone().unwrap_or_else(|| "item".to_string()),
                        max_items: dynamic.max_items,
                        collect_as: Some(dynamic.collect_as.clone()),
                    }),
                    children: Some(children),
                });
                if let Some(materialized) = materialized
                    && let Some(max) = materialized.iter().map(|c| c.flat_index + 1).max()
                {
                    flat_index = flat_index.max(max);
                }
                continue;
            }
            WorkflowInputStep::Sequential(seq) => {
                let status = workflow_node_status(input, flat_index);
                let id = format!("step-{step_index}");
                nodes.push(WorkflowGraphNode {
                    id: id.clone(),
                    kind: WorkflowNodeKind::Step,
                    agent: seq.agent.clone(),
                    phase: seq.phase.clone(),
                    label: seq_label(seq, step_index),
                    status,
                    flat_index: Some(flat_index),
                    step_index: Some(step_index),
                    item_key: None,
                    output_name: seq.output_name.clone(),
                    structured: Some(seq.structured),
                    acceptance_status: input
                        .results
                        .get(flat_index)
                        .and_then(|r| r.acceptance_status.clone()),
                    error: input
                        .step_statuses
                        .get(flat_index)
                        .and_then(|s| s.error.clone())
                        .or_else(|| input.results.get(flat_index).and_then(|r| r.error.clone())),
                    dynamic: None,
                    children: None,
                });
                push_workflow_phase(&mut phases, seq.phase.as_deref(), &id);
                if status == WorkflowNodeStatus::Running
                    || input.current_flat_index == Some(flat_index)
                    || input.current_step_index == Some(step_index)
                {
                    current_node_id = Some(id);
                }
                flat_index += 1;
            }
        }
    }

    WorkflowGraphSnapshot {
        run_id: input.run_id.clone(),
        mode: input.mode.unwrap_or(WorkflowRunMode::Chain),
        phases,
        nodes,
        current_node_id,
    }
}

/// Build a [`WorkflowGraphBuildInput`] from a background run's already-flattened [`crate::spawn::chain_graph::RunnerStep`] list
/// plus its live [`RunStatus`], so the detached runner (`background/runner_main.rs`) can embed a
/// live workflow-graph snapshot in `status.json`. Each [`crate::spawn::chain_graph::RunnerStep`] projects to a
/// [`WorkflowInputStep`]; per-step status/errors come straight off `status.steps`
/// ([`StepState`] normalized to the graph's own vocabulary). Note the cyrup [`crate::spawn::chain_graph::SingleStepSpec`]
/// carries no `phase`/`label`, so those degrade to `None`/agent-name here — the richer, phase/label-
/// bearing [`build_workflow_graph_snapshot`] path is exercised directly by the chain-plan layer that
/// does have that metadata.
#[must_use]
pub fn workflow_graph_from_run(
    steps: &[crate::spawn::chain_graph::RunnerStep],
    status: &RunStatus,
) -> WorkflowGraphSnapshot {
    use crate::spawn::chain_graph::RunnerStep;
    let mut input_steps: Vec<WorkflowInputStep> = Vec::new();
    let mut results: Vec<WorkflowResultSummary> = Vec::new();
    let mut step_statuses: Vec<WorkflowStepStatusInput> = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        let step_status = status.steps.get(index);
        let status_str = step_status.map(|s| match s.status {
            StepState::Pending => "pending",
            StepState::Running => "running",
            StepState::Paused => "paused",
            StepState::Complete => "complete",
            StepState::Failed => "failed",
            StepState::Stopped => "stopped",
        });
        step_statuses.push(WorkflowStepStatusInput {
            status: status_str.map(str::to_string),
            error: step_status.and_then(|s| s.error.clone()),
        });
        results.push(WorkflowResultSummary::default());
        match step {
            RunnerStep::SingleStep(spec) => {
                input_steps.push(WorkflowInputStep::Sequential(WorkflowTaskSpec {
                    agent: Some(spec.agent.clone()),
                    phase: None,
                    label: None,
                    output_name: spec.output.clone(),
                    structured: spec.structured_output_schema.is_some(),
                }));
            }
            RunnerStep::ImportAsyncRoot(spec) => {
                input_steps.push(WorkflowInputStep::Sequential(WorkflowTaskSpec {
                    agent: Some(spec.agent.clone()),
                    phase: None,
                    label: None,
                    output_name: spec.output.clone(),
                    structured: false,
                }));
            }
            RunnerStep::ParallelGroup(group) => {
                let tasks = group
                    .steps
                    .iter()
                    .map(|s| WorkflowTaskSpec {
                        agent: Some(s.agent.clone()),
                        phase: None,
                        label: None,
                        output_name: s.output.clone(),
                        structured: s.structured_output_schema.is_some(),
                    })
                    .collect();
                input_steps.push(WorkflowInputStep::Parallel(tasks));
            }
            RunnerStep::DynamicGroup(dynamic) => {
                input_steps.push(WorkflowInputStep::Dynamic(WorkflowDynamicStepSpec {
                    expand_from_output: dynamic.expand.clone(),
                    expand_from_path: String::new(),
                    item_name: None,
                    max_items: None,
                    step_phase: None,
                    step_label: None,
                    template_phase: None,
                    template_label: None,
                    collect_as: dynamic.collect.clone(),
                    collect_structured: dynamic.template.structured_output_schema.is_some(),
                }));
            }
        }
    }

    let mode = match status.mode {
        RunMode::Single => WorkflowRunMode::Single,
        RunMode::Parallel => WorkflowRunMode::Parallel,
        RunMode::Chain => WorkflowRunMode::Chain,
    };

    build_workflow_graph_snapshot(&WorkflowGraphBuildInput {
        run_id: status.run_id.as_str().to_string(),
        mode: Some(mode),
        steps: input_steps,
        results,
        current_flat_index: status.current_step,
        current_step_index: status.current_step,
        step_statuses,
        dynamic_children: std::collections::BTreeMap::new(),
        dynamic_group_statuses: std::collections::BTreeMap::new(),
    })
}

// =================================================================================================
// Run-id PREFIX resolution over the async/results dirs (pi `run-id-resolver.ts` async slice +
// `async-resume.ts::findAsyncRunPrefixMatches`)
// =================================================================================================
//
// pi's control ops accept a run-id PREFIX and resolve it against the on-disk async/results dirs,
// erroring on ambiguity and returning the resolved location (`resolveSubagentRunId`'s async branch,
// `run-id-resolver.ts:54-83`; `findAsyncRunPrefixMatches`). This is the background/async slice of
// that resolver — the only namespace this crate's background subsystem owns (the foreground-control
// and nested-async namespaces pi also merges are separate subsystems). Both an EXACT id and a unique
// PREFIX resolve; an ambiguous prefix is a hard error naming every match, exactly like pi.

/// The on-disk location a resolved background run id maps to (pi `AsyncRunLocation`,
/// `async-resume.ts`). At least one of `async_dir`/`result_path` is always `Some` (a run is
/// resolvable iff its run dir OR its terminal result file exists).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsyncRunLocation {
    /// The run's own directory (`<async_root>/<id>`), when it exists on disk.
    pub async_dir: Option<PathBuf>,
    /// The run's terminal result file (`<results_dir>/<id>.json`), when it exists on disk.
    pub result_path: Option<PathBuf>,
    /// The fully-resolved (non-prefix) run id.
    pub resolved_id: RunId,
}

/// Why a run-id (prefix) failed to resolve (pi throws with these exact ambiguity/safety messages).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveRunIdError {
    /// The id token was empty or contained a path separator / `..` (pi `assertSafeNestedId`).
    #[error("'{0}' is not a safe id token")]
    UnsafeToken(String),
    /// The prefix matched more than one run (pi's "Ambiguous subagent run id prefix" throw).
    #[error(
        "Ambiguous subagent run id prefix '{prefix}' matched: {}. Provide a longer id.",
        matches.join(", ")
    )]
    Ambiguous {
        /// The ambiguous prefix as supplied.
        prefix: String,
        /// Every matched `async:<id>` label, in sorted order.
        matches: Vec<String>,
    },
}

/// A safe run-id token (pi `assertSafeNestedId`, `nested-events.ts`): non-empty, no path separator,
/// no `..`.
fn is_safe_run_id_token(token: &str) -> bool {
    !token.is_empty()
        && !token.contains('/')
        && !token.contains('\\')
        && !token.contains("..")
}

/// The exact-id location for `id`, if either its run dir or its terminal result file exists (pi
/// `exactAsyncLocation`, `run-id-resolver.ts:19-28`). Pure filesystem existence checks only.
fn exact_async_location(id: &str, async_root: &Path, results_dir: &Path) -> Option<AsyncRunLocation> {
    let async_dir = async_root.join(id);
    let result_path = results_dir.join(format!("{id}.json"));
    let async_exists = async_dir.exists();
    let result_exists = result_path.exists();
    if !async_exists && !result_exists {
        return None;
    }
    Some(AsyncRunLocation {
        async_dir: async_exists.then_some(async_dir),
        result_path: result_exists.then_some(result_path),
        resolved_id: RunId::from_token(id),
    })
}

/// Every background run whose id starts with `prefix`, gathered from BOTH the async run-dir tree and
/// the results-dir (`<id>.json`) tree (pi `findAsyncRunPrefixMatches`, `async-resume.ts`). Returns a
/// de-duplicated, id-sorted list of locations.
#[must_use]
pub fn find_async_run_prefix_matches(
    prefix: &str,
    async_root: &Path,
    results_dir: &Path,
) -> Vec<AsyncRunLocation> {
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(async_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix) {
                ids.insert(name);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(results_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".json")
                && stem.starts_with(prefix)
            {
                ids.insert(stem.to_string());
            }
        }
    }
    ids.into_iter()
        .filter_map(|id| exact_async_location(&id, async_root, results_dir))
        .collect()
}

/// Resolve `id` (an EXACT id or a unique PREFIX) to its on-disk [`AsyncRunLocation`] over
/// `async_root`/`results_dir` — the background/async slice of pi's `resolveSubagentRunId`
/// (`run-id-resolver.ts:54-83`). An exact match wins outright; otherwise a prefix that matches
/// exactly one run resolves, a prefix matching several is [`ResolveRunIdError::Ambiguous`], and a
/// prefix matching none is `Ok(None)`.
///
/// # Errors
///
/// [`ResolveRunIdError::UnsafeToken`] for an unsafe id token, or [`ResolveRunIdError::Ambiguous`]
/// when a prefix matches more than one run.
pub fn resolve_async_run_id(
    id: &str,
    async_root: &Path,
    results_dir: &Path,
) -> Result<Option<AsyncRunLocation>, ResolveRunIdError> {
    if !is_safe_run_id_token(id) {
        return Err(ResolveRunIdError::UnsafeToken(id.to_string()));
    }
    if let Some(exact) = exact_async_location(id, async_root, results_dir) {
        return Ok(Some(exact));
    }
    let mut matches = find_async_run_prefix_matches(id, async_root, results_dir);
    if matches.len() > 1 {
        let labels = matches
            .iter()
            .map(|m| format!("async:{}", m.resolved_id.as_str()))
            .collect::<Vec<_>>();
        return Err(ResolveRunIdError::Ambiguous {
            prefix: id.to_string(),
            matches: labels,
        });
    }
    Ok(matches.pop())
}

/// One line of `run-history.jsonl` (pi `RunEntry`, `run-history.ts:5-12`): the agent, its (200-char-
/// capped) task, a **seconds** epoch timestamp, an `"ok"`/`"error"` status, the run duration in
/// milliseconds, and — only when nonzero — the failing exit code. Field names match pi's exact
/// on-disk keys (`agent`/`task`/`ts`/`status`/`duration`/`exit`) so a reader of either runtime's
/// history file sees the identical shape.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunHistoryEntry {
    /// The agent this entry records.
    pub agent: String,
    /// The task text, truncated to pi's 200-character cap.
    pub task: String,
    /// Epoch **seconds** (pi's `Math.floor(Date.now() / 1000)`).
    pub ts: i64,
    /// `"ok"` for a clean exit, `"error"` otherwise (pi's `exitCode === 0 ? "ok" : "error"`).
    pub status: String,
    /// The run's duration in milliseconds.
    pub duration: i64,
    /// The failing exit code, present only when nonzero (pi's `...(exitCode !== 0 ? { exit } : {})`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
}

/// `getHistoryPath()` (`runs/shared/run-history.ts:23-25` @v0.43.0):
/// `path.join(getAgentDir(), "run-history.jsonl")`.
///
/// Run history is DELIBERATELY not under [`temp_root_dir`]: it is the one thing this module writes
/// that is meant to outlive a reboot (it is what `--force`/staleness checks and the cost report
/// read back), so it belongs in the agent dir with the rest of the user's durable state. The
/// previous `<home>/.cyrup/subagents/run-history.jsonl` was neither — it sat in the run-scratch
/// tree that this module now correctly treats as disposable.
#[must_use]
pub fn run_history_path() -> PathBuf {
    crate::paths::agent_dir().join("run-history.jsonl")
}

/// The run-history file for a run whose per-run directories hang off `async_root`.
///
/// For every run that actually lives in this user's canonical scratch root ([`temp_root_dir`]) —
/// which is every production run, since `resolve_background_storage_roots` derives its roots from
/// [`run_artifact_roots`] or from an inherited nested route that is itself under that root — this
/// is exactly pi's unconditional [`run_history_path`].
///
/// [CYRUP-DELTA] a run whose roots were REDIRECTED somewhere else records its history beside those
/// roots instead of in the real user's agent dir. This is the same principle C7 established for the
/// results dir: **the runner honours the absolute roots it was handed and never re-derives a path
/// the orchestrator did not choose.** History was the one write still ignoring that, and the cost
/// was measurable — a full workspace gate put 136 lines of synthetic test history (`researcher` /
/// `"do the thing"` / `scout`) into a developer's real `~/.cyrup/agent/run-history.jsonl`, because
/// in-process `run()` callers hand it a `TempDir` for every path EXCEPT this one.
#[must_use]
pub fn run_history_path_for(async_root: &Path) -> PathBuf {
    if async_root.starts_with(temp_root_dir()) {
        return run_history_path();
    }
    async_root
        .parent()
        .unwrap_or(async_root)
        .join("run-history.jsonl")
}

/// Append one [`RunHistoryEntry`] per `result` to `run-history.jsonl` (pi's `recordRun`,
/// `run-history.ts:132-153`) — best-effort: a missing directory is created, and every I/O or
/// serialization failure is silently swallowed so history recording can never fail a run (pi wraps
/// the whole thing in a `try {} catch {}` for exactly this reason). `run_started_at` is the run's
/// epoch-millis start, used to derive each entry's `duration`.
pub async fn record_run_history(async_root: &Path, run_started_at: i64, results: &[SingleResult]) {
    record_run_history_at(&run_history_path_for(async_root), run_started_at, results).await;
}

/// The path-explicit core of [`record_run_history`], so tests can target a private temp path
/// without mutating process-global `CYRUP_HOME`/`HOME` (the lib crate is `#![forbid(unsafe_code)]`,
/// which blocks the `unsafe { set_var }` an env override would otherwise require in a `src/` test).
async fn record_run_history_at(path: &Path, run_started_at: i64, results: &[SingleResult]) {
    if results.is_empty() {
        return;
    }
    let now = crate::time::now_epoch_millis();
    let duration = (now - run_started_at).max(0);
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = tokio::fs::create_dir_all(parent).await;

    let mut buf = String::new();
    for result in results {
        let entry = RunHistoryEntry {
            agent: result.agent.clone(),
            task: result.task.chars().take(200).collect(),
            ts: now / 1000,
            status: if result.exit_code == 0 { "ok" } else { "error" }.to_string(),
            duration,
            exit: (result.exit_code != 0).then_some(result.exit_code),
        };
        if let Ok(line) = serde_json::to_string(&entry) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    if buf.is_empty() {
        return;
    }

    use tokio::io::AsyncWriteExt;
    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        // `tokio::fs::File` buffers writes internally and does NOT flush on drop, so a bare
        // `write_all` can leave the bytes sitting in tokio's buffer (with the backing write still
        // dispatched to the blocking pool) when the handle is dropped — the write then never lands
        // and a reader sees an empty file. Flush explicitly so the entries are durable before the
        // handle drops; still best-effort, so a flush error is swallowed like every other I/O error.
        if file.write_all(buf.as_bytes()).await.is_ok() {
            let _ = file.flush().await;
        }
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
    use std::collections::HashSet;

    // ---------------------------------------------------------------------------------------
    // Run-id PREFIX resolution (pi `run-id-resolver.ts` async slice)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn resolve_async_run_id_resolves_exact_and_unique_prefix_and_errors_on_ambiguous() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        std::fs::create_dir_all(&async_root).expect("mkdir async_root");
        std::fs::create_dir_all(&results_dir).expect("mkdir results_dir");

        // One run present as a run DIRECTORY, another present only as a terminal RESULT file.
        std::fs::create_dir_all(async_root.join("deadbeef0001")).expect("mkdir run dir");
        std::fs::write(results_dir.join("cafef00d0002.json"), b"{}").expect("write result file");

        // An EXACT id (the dir-backed run) resolves.
        let exact = resolve_async_run_id("deadbeef0001", &async_root, &results_dir)
            .expect("no error")
            .expect("exact id resolves");
        assert_eq!(exact.resolved_id.as_str(), "deadbeef0001");
        assert!(exact.async_dir.is_some());

        // An EXACT id backed only by its terminal result file resolves via that file.
        let exact_result = resolve_async_run_id("cafef00d0002", &async_root, &results_dir)
            .expect("no error")
            .expect("result-backed id resolves");
        assert_eq!(exact_result.resolved_id.as_str(), "cafef00d0002");
        assert!(exact_result.result_path.is_some());

        // A unique PREFIX resolves to the single matching run (the load-bearing behavior this task
        // calls for: control ops accept a run-id prefix, not only an exact id).
        let by_prefix = resolve_async_run_id("deadbeef", &async_root, &results_dir)
            .expect("no error")
            .expect("unique prefix resolves");
        assert_eq!(by_prefix.resolved_id.as_str(), "deadbeef0001");

        // A prefix matching zero runs resolves to `None`, not an error.
        let miss = resolve_async_run_id("zzzz", &async_root, &results_dir).expect("no error");
        assert!(miss.is_none());

        // A second run sharing the `deadbeef` prefix makes that prefix AMBIGUOUS — a hard error.
        std::fs::create_dir_all(async_root.join("deadbeef9999")).expect("mkdir second run dir");
        let ambiguous = resolve_async_run_id("deadbeef", &async_root, &results_dir);
        assert!(
            matches!(ambiguous, Err(ResolveRunIdError::Ambiguous { .. })),
            "a prefix matching >1 run must be a hard Ambiguous error: {ambiguous:?}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Run-history recording (pi `run-history.ts`)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn record_run_history_appends_one_ok_and_one_error_line() {
        // Hermetic: write into a private temp path via the path-explicit core, so this never
        // touches the real `~/.cyrup` and needs no `unsafe { set_var }` (blocked by the lib's
        // `#![forbid(unsafe_code)]`).
        let home = tempfile::tempdir().expect("real tempdir");
        let history_path = home.path().join("run-history.jsonl");

        let ok = SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            agent: "researcher".to_string(),
            task: "look into the thing".to_string(),
            exit_code: 0,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: Some("done".to_string()),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };
        let mut bad = ok.clone();
        bad.agent = "writer".to_string();
        bad.exit_code = 7;

        record_run_history_at(&history_path, crate::time::now_epoch_millis() - 1234, &[ok, bad]).await;

        let contents = std::fs::read_to_string(&history_path).expect("history file exists");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "one line per result: {contents:?}");

        let first: RunHistoryEntry = serde_json::from_str(lines[0]).expect("parse first entry");
        assert_eq!(first.agent, "researcher");
        assert_eq!(first.status, "ok");
        assert!(first.exit.is_none(), "a clean exit omits `exit`");
        assert!(first.duration >= 0);

        let second: RunHistoryEntry = serde_json::from_str(lines[1]).expect("parse second entry");
        assert_eq!(second.agent, "writer");
        assert_eq!(second.status, "error");
        assert_eq!(second.exit, Some(7), "a nonzero exit records `exit`");
    }

    // ---------------------------------------------------------------------------------------
    // RunId
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_id_new_produces_a_purely_hex_token() {
        let id = RunId::new();
        assert!(
            id.as_str().chars().all(|c| c.is_ascii_hexdigit()),
            "token must be purely hex: {}",
            id.as_str()
        );
    }

    #[test]
    fn run_id_new_token_is_at_least_8_hex_chars() {
        // func-SA §4.5's illustrative floor ("8-hex-character random token"); this
        // implementation's 32-hex-digit UUIDv4 simple form comfortably exceeds it.
        let id = RunId::new();
        assert!(
            id.as_str().len() >= 8,
            "token too short: {} chars",
            id.as_str().len()
        );
    }

    #[test]
    fn run_id_new_has_no_path_separators_or_dots() {
        // Directory-name safety: a RunId is used verbatim as a path component (RunDir::new).
        let id = RunId::new();
        assert!(!id.as_str().contains('/'));
        assert!(!id.as_str().contains('\\'));
        assert!(!id.as_str().contains(".."));
        assert!(!id.as_str().is_empty());
    }

    #[test]
    fn run_id_new_is_unique_across_many_mintings() {
        // No pre-flight uniqueness check is performed (R-SA-072) — this test instead verifies
        // that the entropy source itself makes collisions practically unobservable across a
        // large sample, which is the property that makes skipping a uniqueness check safe.
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            let id = RunId::new();
            assert!(
                seen.insert(id.clone()),
                "duplicate RunId minted: {}",
                id.as_str()
            );
        }
    }

    #[test]
    fn run_id_display_matches_as_str() {
        let id = RunId::new();
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn run_id_from_token_round_trips_via_serde() {
        let id = RunId::from_token("deadbeefcafef00d");
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(json, "\"deadbeefcafef00d\"", "serde(transparent) as bare string");
        let back: RunId = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, id);
    }

    #[test]
    fn run_id_default_mints_fresh_entropy_each_time() {
        let a = RunId::default();
        let b = RunId::default();
        assert_ne!(a, b, "Default must not be a fixed/empty sentinel");
    }

    // ---------------------------------------------------------------------------------------
    // RunState forward-only transitions
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_state_default_is_queued() {
        assert_eq!(RunState::default(), RunState::Queued);
    }

    #[test]
    fn run_state_queued_can_advance_to_running_or_failed_only() {
        assert!(RunState::Queued.can_transition_to(RunState::Running));
        assert!(RunState::Queued.can_transition_to(RunState::Failed));
        assert!(!RunState::Queued.can_transition_to(RunState::Complete));
        assert!(!RunState::Queued.can_transition_to(RunState::Paused));
        assert!(!RunState::Queued.can_transition_to(RunState::Queued));
    }

    #[test]
    fn run_state_running_can_advance_to_paused_complete_or_failed() {
        assert!(RunState::Running.can_transition_to(RunState::Paused));
        assert!(RunState::Running.can_transition_to(RunState::Complete));
        assert!(RunState::Running.can_transition_to(RunState::Failed));
        assert!(!RunState::Running.can_transition_to(RunState::Queued));
        assert!(!RunState::Running.can_transition_to(RunState::Running));
    }

    #[test]
    fn run_state_paused_can_resume_to_running_or_reconcile_to_failed() {
        assert!(RunState::Paused.can_transition_to(RunState::Running));
        assert!(RunState::Paused.can_transition_to(RunState::Failed));
        assert!(!RunState::Paused.can_transition_to(RunState::Complete));
        assert!(!RunState::Paused.can_transition_to(RunState::Queued));
        assert!(!RunState::Paused.can_transition_to(RunState::Paused));
    }

    #[test]
    fn run_state_terminal_states_permit_no_outgoing_transition() {
        for terminal in [RunState::Complete, RunState::Failed] {
            for candidate in [
                RunState::Queued,
                RunState::Running,
                RunState::Paused,
                RunState::Complete,
                RunState::Failed,
            ] {
                assert!(
                    !terminal.can_transition_to(candidate),
                    "{terminal:?} must not transition to {candidate:?}, terminal states are sinks"
                );
            }
        }
    }

    #[test]
    fn run_state_is_terminal_excludes_paused() {
        assert!(!RunState::Queued.is_terminal());
        assert!(!RunState::Running.is_terminal());
        assert!(
            !RunState::Paused.is_terminal(),
            "R-SA-084: interrupt is a soft pause, never terminal"
        );
        assert!(RunState::Complete.is_terminal());
        assert!(RunState::Failed.is_terminal());
    }

    #[test]
    fn run_state_try_advance_ok_on_legal_transition() {
        let result = RunState::Queued.try_advance(RunState::Running);
        assert_eq!(result, Ok(RunState::Running));
    }

    #[test]
    fn run_state_try_advance_err_on_illegal_transition() {
        let result = RunState::Complete.try_advance(RunState::Running);
        assert_eq!(
            result,
            Err(RunStateTransitionError {
                from: RunState::Complete,
                to: RunState::Running,
            })
        );
    }

    #[test]
    fn run_state_try_advance_err_message_is_human_readable() {
        let err = RunState::Failed
            .try_advance(RunState::Queued)
            .expect_err("Failed -> Queued must be illegal");
        let message = err.to_string();
        assert!(message.contains("Failed"));
        assert!(message.contains("Queued"));
    }

    /// G77 — `Stopped` is terminal in its own right, reachable only from `Running`/`Queued`, and a
    /// dead end thereafter. Every claim is checked against a named upstream site.
    #[test]
    fn stopped_is_a_first_class_terminal_run_state_not_an_alias() {
        // Terminal (pi `chain-root-attachment.ts:60` TERMINAL_STATES, `stale-run-reconciler.ts:292`
        // isTerminalState) — and distinct from the other two terminal states.
        assert!(RunState::Stopped.is_terminal());
        assert_ne!(RunState::Stopped, RunState::Failed);
        assert_ne!(RunState::Stopped, RunState::Paused);
        assert_eq!(RunState::Stopped.rank(), RunState::Failed.rank());

        // Reachable from exactly the two states pi's `stopAsyncRun` guard accepts
        // (`async-stop-action.ts:41`: `state !== "running" && state !== "queued"` is the refusal).
        assert!(RunState::Running.can_transition_to(RunState::Stopped));
        assert!(RunState::Queued.can_transition_to(RunState::Stopped));
        assert!(
            !RunState::Paused.can_transition_to(RunState::Stopped),
            "a paused run is not stoppable upstream — `stopAsyncRun` answers `No running or queued \
             async run was found`"
        );

        // A dead end: no outgoing transition at all, including to itself.
        for next in [
            RunState::Queued,
            RunState::Running,
            RunState::Paused,
            RunState::Complete,
            RunState::Failed,
            RunState::Stopped,
        ] {
            assert!(
                !RunState::Stopped.can_transition_to(next),
                "Stopped is terminal; Stopped -> {next:?} must be rejected"
            );
            RunState::Stopped
                .try_advance(next)
                .expect_err("every outgoing transition from Stopped is illegal");
        }
    }

    /// G77 — the per-step counterpart (pi `subagent-runner.ts:2967` `step.status = "stopped"`;
    /// `chain-root-attachment.ts:61` TERMINAL_STEP_STATUSES).
    #[test]
    fn stopped_is_a_first_class_terminal_step_state_not_an_alias() {
        assert!(StepState::Stopped.is_terminal());
        assert_ne!(StepState::Stopped, StepState::Failed);
        assert_ne!(StepState::Stopped, StepState::Paused);
        // The pre-existing terminality relations are untouched.
        assert!(!StepState::Pending.is_terminal());
        assert!(!StepState::Running.is_terminal());
        assert!(!StepState::Paused.is_terminal());
    }

    /// G77 — the wire spelling is `"stopped"` on both enums, which is what a pi-shaped
    /// `status.json`/result file round-trips through.
    #[test]
    fn stopped_serializes_as_the_lowercase_pi_wire_word() {
        assert_eq!(
            serde_json::to_value(RunState::Stopped).expect("serialize"),
            serde_json::json!("stopped")
        );
        assert_eq!(
            serde_json::to_value(StepState::Stopped).expect("serialize"),
            serde_json::json!("stopped")
        );
        assert_eq!(
            serde_json::from_value::<RunState>(serde_json::json!("stopped")).expect("deserialize"),
            RunState::Stopped
        );
        assert_eq!(
            serde_json::from_value::<StepState>(serde_json::json!("stopped")).expect("deserialize"),
            StepState::Stopped
        );
    }

    /// G77 — the workflow-graph projection: a stopped step normalizes to
    /// [`WorkflowNodeStatus::Stopped`] (pi `subagent-runner.ts:2163-2166`) and rolls up over a
    /// parallel group ahead of `failed`/`paused` (`:2181-2186`).
    #[test]
    fn workflow_graph_projects_and_rolls_up_stopped() {
        assert_eq!(
            normalize_workflow_status(Some("stopped")),
            Some(WorkflowNodeStatus::Stopped)
        );
        assert_eq!(
            summarize_parallel_statuses(&[WorkflowNodeStatus::Failed, WorkflowNodeStatus::Stopped]),
            WorkflowNodeStatus::Stopped,
            "stopped outranks failed in the runner's own child roll-up"
        );
        assert_eq!(
            summarize_parallel_statuses(&[WorkflowNodeStatus::Running, WorkflowNodeStatus::Stopped]),
            WorkflowNodeStatus::Running,
            "…but running still outranks stopped"
        );
        // Unchanged without a stopped child.
        assert_eq!(
            summarize_parallel_statuses(&[WorkflowNodeStatus::Failed, WorkflowNodeStatus::Paused]),
            WorkflowNodeStatus::Failed
        );
    }

    #[test]
    fn run_state_full_lifecycle_walks_queued_running_paused_running_complete() {
        // A realistic full lifecycle: spawn -> run -> interrupt -> resume -> finish.
        let mut state = RunState::Queued;
        for next in [
            RunState::Running,
            RunState::Paused,
            RunState::Running,
            RunState::Complete,
        ] {
            state = state.try_advance(next).expect("each hop is legal");
        }
        assert_eq!(state, RunState::Complete);
    }

    #[test]
    fn run_state_full_lifecycle_walks_queued_running_failed() {
        let mut state = RunState::Queued;
        for next in [RunState::Running, RunState::Failed] {
            state = state.try_advance(next).expect("each hop is legal");
        }
        assert_eq!(state, RunState::Failed);
    }

    #[test]
    fn step_state_is_terminal_excludes_paused() {
        assert!(!StepState::Pending.is_terminal());
        assert!(!StepState::Running.is_terminal());
        assert!(!StepState::Paused.is_terminal());
        assert!(StepState::Complete.is_terminal());
        assert!(StepState::Failed.is_terminal());
    }

    // ---------------------------------------------------------------------------------------
    // RunStatus construction / mutation guard
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_status_queued_starts_in_queued_state_with_no_end_time() {
        let status = RunStatus::queued(RunId::new(), RunMode::Single, Some(4242));
        assert_eq!(status.state, RunState::Queued);
        assert_eq!(status.pid, Some(4242));
        assert!(status.ended_at.is_none());
        assert_eq!(status.started_at, status.last_update);
        assert!(status.steps.is_empty());
    }

    #[test]
    fn run_status_provisional_matches_queued_shape() {
        let run_id = RunId::new();
        let provisional = RunStatus::provisional(run_id.clone(), RunMode::Chain, 99);
        assert_eq!(provisional.run_id, run_id);
        assert_eq!(provisional.mode, RunMode::Chain);
        assert_eq!(provisional.state, RunState::Queued);
        assert_eq!(provisional.pid, Some(99));
    }

    #[test]
    fn run_status_advance_state_updates_last_update_and_leaves_ended_at_unset_for_non_terminal() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        let before = status.last_update;
        std::thread::sleep(std::time::Duration::from_millis(2));

        status
            .advance_state(RunState::Running)
            .expect("Queued -> Running is legal");

        assert_eq!(status.state, RunState::Running);
        assert!(status.last_update >= before);
        assert!(
            status.ended_at.is_none(),
            "Running is not terminal, ended_at must stay unset"
        );
    }

    #[test]
    fn run_status_advance_state_sets_ended_at_on_terminal_transition() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        status.advance_state(RunState::Running).expect("legal");
        status.advance_state(RunState::Complete).expect("legal");

        assert_eq!(status.state, RunState::Complete);
        assert!(status.ended_at.is_some());
    }

    #[test]
    fn run_status_advance_state_rejects_illegal_transition_and_leaves_state_unmodified() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        let before = status.clone();

        let result = status.advance_state(RunState::Complete);

        assert!(result.is_err(), "Queued -> Complete must be rejected");
        assert_eq!(
            status, before,
            "a rejected transition must leave the status entirely unmodified"
        );
    }

    #[test]
    fn run_status_advance_state_rejects_transition_out_of_terminal_state() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        status.advance_state(RunState::Running).expect("legal");
        status.advance_state(RunState::Failed).expect("legal");

        let result = status.advance_state(RunState::Running);
        assert!(
            result.is_err(),
            "a terminal Failed run must never transition again, even back to Running"
        );
    }

    #[test]
    fn run_status_touch_advances_last_update_without_changing_state() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        status.advance_state(RunState::Running).expect("legal");
        let before = status.last_update;
        std::thread::sleep(std::time::Duration::from_millis(2));

        status.touch();

        assert_eq!(status.state, RunState::Running, "touch never changes state");
        assert!(status.last_update >= before);
    }

    #[test]
    fn run_status_round_trips_through_json() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Parallel, Some(7));
        status.steps.push(StepStatus::pending("researcher"));
        status.chain_step_count = Some(3);
        status.pending_appends = Some(0);

        let json = serde_json::to_string(&status).expect("serializes");
        let back: RunStatus = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, status);
    }

    // ---------------------------------------------------------------------------------------
    // StepStatus / ParallelGroupStatus
    // ---------------------------------------------------------------------------------------

    #[test]
    fn step_status_pending_has_no_timestamps_or_usage_yet() {
        let step = StepStatus::pending("reviewer");
        assert_eq!(step.agent, "reviewer");
        assert_eq!(step.status, StepState::Pending);
        assert!(step.started_at.is_none());
        assert!(step.ended_at.is_none());
        assert!(step.attempted_models.is_empty());
        assert!(step.nested_run_ids.is_empty());
    }

    #[test]
    fn parallel_group_status_round_trips_through_json() {
        let group = ParallelGroupStatus {
            group_step_index: 2,
            children: vec![StepStatus::pending("a"), StepStatus::pending("b")],
        };
        let json = serde_json::to_string(&group).expect("serializes");
        let back: ParallelGroupStatus = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, group);
    }

    // ---------------------------------------------------------------------------------------
    // ResultFile
    // ---------------------------------------------------------------------------------------

    #[test]
    fn result_file_round_trips_through_json() {
        let run_id = RunId::new();
        let result = ResultFile {
            id: run_id.clone(),
            run_id,
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp/project"),
            session_file: None,
            results: Vec::new(),
        };
        let json = serde_json::to_string(&result).expect("serializes");
        let back: ResultFile = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, result);
    }

    // ---------------------------------------------------------------------------------------
    // RunDir / RunPaths
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_dir_joins_async_root_and_run_id() {
        let async_root = PathBuf::from("/var/tmp/cyrup-subagents");
        let run_id = RunId::from_token("abc12345");
        let dir = RunDir::new(&async_root, &run_id);
        assert_eq!(dir.as_path(), Path::new("/var/tmp/cyrup-subagents/abc12345"));
    }

    #[test]
    fn run_paths_for_run_derives_every_well_known_path() {
        let async_root = PathBuf::from("/var/tmp/cyrup-subagents");
        let results_dir = PathBuf::from("/var/tmp/cyrup-subagents-results");
        let run_id = RunId::from_token("abc12345");

        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        assert_eq!(
            paths.run_dir,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345")
        );
        assert_eq!(
            paths.status,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/status.json")
        );
        assert_eq!(
            paths.events,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/events.jsonl")
        );
        assert_eq!(
            paths.control_inbox,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/control/interrupt.json")
        );
        assert_eq!(
            paths.append_dir,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/append-requests")
        );
        assert_eq!(
            paths.runner_stdout_log,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/runner.stdout.log")
        );
        assert_eq!(
            paths.runner_stderr_log,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/runner.stderr.log")
        );
        assert_eq!(
            paths.run_log_md,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345/abc12345.md")
        );
        assert_eq!(
            paths.result,
            PathBuf::from("/var/tmp/cyrup-subagents-results/abc12345.json")
        );
    }

    #[test]
    fn run_paths_result_lives_outside_the_run_dir() {
        // func-SA §4.5's deliberate separation: the terminal ResultFile must NOT be a path
        // underneath run_dir, since "presence in ResultsDir" is the authoritative done-signal
        // and must be observable via a directory-watch scoped to ResultsDir alone.
        let async_root = PathBuf::from("/a");
        let results_dir = PathBuf::from("/b");
        let run_id = RunId::from_token("deadbeef");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        assert!(!paths.result.starts_with(&paths.run_dir));
    }

    #[test]
    fn run_paths_step_output_log_is_indexed_within_run_dir() {
        let paths = RunPaths::for_run(
            Path::new("/a"),
            Path::new("/b"),
            &RunId::from_token("cafef00d"),
        );
        assert_eq!(
            paths.step_output_log(3),
            PathBuf::from("/a/cafef00d/output-3.log")
        );
    }

    #[test]
    fn run_paths_nested_is_keyed_under_the_root_run_id() {
        // R-SA-104: nested background runs SHOULD use a distinct storage subpath keyed under the
        // root run's id.
        let root_id = RunId::from_token("root0001");
        let nested_id = RunId::from_token("child001");
        let root_paths = RunPaths::for_run(Path::new("/a"), Path::new("/b"), &root_id);

        let nested_paths = root_paths.nested(&nested_id);

        assert!(
            nested_paths.run_dir.starts_with(&root_paths.run_dir),
            "nested run_dir must live underneath the root run's own directory"
        );
        assert!(
            nested_paths.run_dir.ends_with("nested/child001"),
            "nested run_dir must be keyed by the nested run's own id: {:?}",
            nested_paths.run_dir
        );
        assert!(
            nested_paths.result.starts_with(&root_paths.run_dir),
            "nested results must be scoped under the root run's tree, not the shared top-level \
             ResultsDir"
        );
    }

    #[test]
    fn run_paths_nested_two_children_do_not_collide() {
        let root_id = RunId::from_token("root0002");
        let root_paths = RunPaths::for_run(Path::new("/a"), Path::new("/b"), &root_id);

        let a = root_paths.nested(&RunId::from_token("child00a"));
        let b = root_paths.nested(&RunId::from_token("child00b"));

        assert_ne!(a.run_dir, b.run_dir);
        assert_ne!(a.status, b.status);
        assert_ne!(a.result, b.result);
    }

    // ---------------------------------------------------------------------------------------
    // RunMode
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_mode_serializes_as_camel_case() {
        assert_eq!(
            serde_json::to_string(&RunMode::Single).expect("serializes"),
            "\"single\""
        );
        assert_eq!(
            serde_json::to_string(&RunMode::Parallel).expect("serializes"),
            "\"parallel\""
        );
        assert_eq!(
            serde_json::to_string(&RunMode::Chain).expect("serializes"),
            "\"chain\""
        );
    }

    // ---------------------------------------------------------------------------------------
    // Shared async-root / results-dir derivation + ensureAccessibleDir (C7)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_artifact_roots_places_results_beside_async_keyed_by_the_same_cwd() {
        // The two roots MUST share a subagents-home and a cwd key, differing only in the
        // async/results segment — the exact invariant C7's divergent derivation broke.
        let a = run_artifact_roots(Path::new("/some/project/a"));
        let b = run_artifact_roots(Path::new("/some/project/b"));

        let key = a.async_root.file_name().expect("async root has a key leaf");
        assert_eq!(
            a.results_dir.file_name(),
            Some(key),
            "both roots must be keyed by the same cwd"
        );
        assert_eq!(a.async_root.parent().and_then(Path::file_name), Some(std::ffi::OsStr::new("async")));
        assert_eq!(a.results_dir.parent().and_then(Path::file_name), Some(std::ffi::OsStr::new("results")));
        assert_eq!(
            a.async_root.parent().and_then(Path::parent),
            a.results_dir.parent().and_then(Path::parent),
            "async/ and results/ must be siblings under one shared subagents home"
        );

        // Distinct cwds get distinct keys, so distinct roots — but the same shared home.
        assert_ne!(a.async_root, b.async_root);
        assert_ne!(a.results_dir, b.results_dir);
        assert_eq!(
            a.async_root.parent().and_then(Path::parent),
            b.async_root.parent().and_then(Path::parent),
            "the subagents-home prefix is shared across cwds"
        );
    }

    /// pi `TEMP_ROOT_DIR = path.join(os.tmpdir(), `pi-subagents-${resolveTempScopeId()}`)`
    /// (`shared/types.ts:1862` @v0.43.0, and byte-identical at the ported baseline
    /// `shared/types.ts:1104` @v0.34.0). Every run-scratch root hangs off the OS TEMP dir.
    ///
    /// This port used to resolve `<CYRUP_HOME|HOME>/.cyrup/subagents` instead, and that single
    /// wrong base is what accumulated 59,321 files / 551 MB of synthetic-run residue
    /// (`fleetrun0001`, 21,076 `cwd`-keyed dirs) inside a real developer's `~/.cyrup`. The test
    /// asserts the property that made that possible is gone: the root is under the temp dir, and
    /// nothing this module derives is under the home dir.
    #[test]
    fn temp_root_dir_lives_under_the_os_temp_dir_and_never_under_home() {
        // Production shape: CYRUP_HOME unset -> `<os-temp>/cyrup-subagents-<scope>`, pi's
        // `TEMP_ROOT_DIR`. Proven through the pure core so a stray ambient CYRUP_HOME (this crate's
        // integration tests set one) can never make the assertion vacuous.
        let os_temp = PathBuf::from("/os-temp");
        let root = temp_root_dir_from(&|_| None, os_temp.clone());
        assert!(
            root.starts_with(&os_temp),
            "the subagent scratch root must hang off the OS temp dir (pi TEMP_ROOT_DIR), got {root:?}"
        );
        let leaf = root
            .file_name()
            .and_then(|n| n.to_str())
            .expect("the temp root always has a leaf");
        assert!(
            leaf.starts_with("cyrup-subagents-"),
            "the leaf mirrors pi's `pi-subagents-<scope>` under cyrup's rebrand, got {leaf:?}"
        );
        assert!(
            leaf.len() > "cyrup-subagents-".len(),
            "the per-user scope segment must be non-empty so two users never share a scratch root"
        );

        // THE REGRESSION: with no CYRUP_HOME sandbox, nothing this module derives may land under
        // the real user's `~/.cyrup` — that is where the 59,321-file pile came from.
        assert!(
            !root.to_string_lossy().contains(".cyrup"),
            "production run scratch must never be written into the user's config dir, got {root:?}"
        );
        if std::env::var_os("CYRUP_HOME").is_none()
            && let Some(home) = std::env::var_os("HOME")
        {
            let dot_cyrup = PathBuf::from(home).join(".cyrup");
            let derived = run_artifact_roots(Path::new("/some/project"));
            for path in [&derived.async_root, &derived.results_dir] {
                assert!(
                    !path.starts_with(&dot_cyrup),
                    "with no CYRUP_HOME sandbox, run scratch must never resolve into the real \
                     user config dir, got {path:?}"
                );
            }
        }

        // Sandbox shape: CYRUP_HOME wins outright and relocates the whole tree.
        let sandbox = temp_root_dir_from(
            &|k| (k == "CYRUP_HOME").then(|| "/sandbox".to_string()),
            os_temp,
        );
        assert_eq!(sandbox, PathBuf::from("/sandbox/.cyrup/subagents"));
    }

    /// `sanitizeTempScopeSegment` (`shared/types.ts:1807-1812` @v0.43.0):
    /// `.trim().replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown"`.
    ///
    /// The `+` quantifier is the load-bearing detail — a RUN of illegal characters collapses to one
    /// `-`, not one per character — and so is the `|| "unknown"` fallback, without which an
    /// all-illegal username would produce a bare `cyrup-subagents-` shared by every such user.
    #[test]
    fn sanitize_temp_scope_segment_matches_pis_regex_pipeline() {
        assert_eq!(sanitize_temp_scope_segment("alice"), "alice");
        assert_eq!(sanitize_temp_scope_segment("  alice  "), "alice");
        // A run of illegal chars collapses to a SINGLE dash (the `+` quantifier).
        assert_eq!(sanitize_temp_scope_segment("a///b"), "a-b");
        assert_eq!(sanitize_temp_scope_segment("a/b"), "a-b");
        // `. _ -` survive; everything else does not.
        assert_eq!(sanitize_temp_scope_segment("a.b_c-d"), "a.b_c-d");
        assert_eq!(
            sanitize_temp_scope_segment("/home/d o/m"),
            "home-d-o-m",
            "leading and trailing dashes are stripped after collapsing"
        );
        // Empty after sanitising -> the literal "unknown", never an empty segment.
        assert_eq!(sanitize_temp_scope_segment("///"), "unknown");
        assert_eq!(sanitize_temp_scope_segment(""), "unknown");
        assert_eq!(sanitize_temp_scope_segment("   "), "unknown");
    }

    /// `resolveTempScopeId` (`shared/types.ts:1814-1857` @v0.43.0) prefers `uid-<n>` wherever
    /// `process.getuid` exists, which is every Unix. The scope must be stable within a process (two
    /// calls that disagreed would split a session's runs across two roots) and must never be empty.
    #[test]
    fn resolve_temp_scope_id_is_stable_and_non_empty() {
        let first = resolve_temp_scope_id();
        assert_eq!(first, resolve_temp_scope_id(), "the scope id must be stable");
        assert!(!first.is_empty());
        assert!(
            !first.contains(std::path::MAIN_SEPARATOR),
            "the scope id is ONE path segment; a separator would silently deepen the tree: {first}"
        );
        #[cfg(target_os = "linux")]
        assert!(
            first.starts_with("uid-"),
            "on Linux pi's first branch (`uid-${{getuid()}}`) always wins, got {first}"
        );
    }

    /// `getHistoryPath()` (`runs/shared/run-history.ts:23-25` @v0.43.0):
    /// `path.join(getAgentDir(), "run-history.jsonl")` — the DURABLE agent dir, not the disposable
    /// temp root. This port had it under `<home>/.cyrup/subagents/`, i.e. inside the run-scratch
    /// tree, where a temp sweep would have silently discarded the user's run history.
    #[test]
    fn run_history_path_is_the_agent_dir_not_the_scratch_root() {
        let path = run_history_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("run-history.jsonl")
        );
        assert_eq!(path.parent(), Some(crate::paths::agent_dir().as_path()));
        assert!(
            !path.starts_with(temp_root_dir()),
            "durable run history must not live inside the disposable scratch root: {path:?}"
        );
    }

    /// [`run_history_path_for`]: a run in the canonical scratch root records to the agent dir (pi's
    /// unconditional `getHistoryPath()`); a run whose roots were REDIRECTED records beside those
    /// roots instead.
    ///
    /// The second half is the regression: in-process `run()` callers hand the runner a `TempDir`
    /// for every path except this one, so re-deriving it wrote 136 lines of synthetic test history
    /// (`researcher`, `"do the thing"`, `scout`) into a real `~/.cyrup/agent/run-history.jsonl` on
    /// one full workspace gate.
    #[test]
    fn run_history_path_for_follows_a_redirected_run_and_never_the_real_agent_dir() {
        // Canonical root -> pi's unconditional agent-dir path, unchanged.
        let canonical = run_artifact_roots(Path::new("/some/project")).async_root;
        assert_eq!(
            run_history_path_for(&canonical),
            run_history_path(),
            "a run under the canonical scratch root keeps pi's `getAgentDir()/run-history.jsonl`"
        );

        // Redirected root (what every in-process `run()` test hands over) -> beside those roots.
        let redirected = Path::new("/some/tempdir/async");
        assert_eq!(
            run_history_path_for(redirected),
            PathBuf::from("/some/tempdir/run-history.jsonl")
        );
        assert_ne!(
            run_history_path_for(redirected),
            run_history_path(),
            "a redirected run must NEVER append to the real user's durable run history"
        );

        // A root with no parent degrades to the root itself rather than panicking.
        assert_eq!(
            run_history_path_for(Path::new("/")),
            PathBuf::from("/run-history.jsonl")
        );
    }

    #[test]
    fn results_dir_for_async_root_recovers_the_orchestrator_sibling_for_the_standard_layout() {
        // Standard layout: <home>/.cyrup/subagents/async/<key>  ->  .../results/<key>.
        let roots = run_artifact_roots(Path::new("/home/me/project"));
        let recovered = results_dir_for_async_root(&roots.async_root);
        assert_eq!(
            recovered, roots.results_dir,
            "the structural fallback must reconstruct EXACTLY the orchestrator's results dir, \
             preserving the cwd key (the C7 pre-fix derivation dropped it)"
        );
    }

    #[test]
    fn results_dir_for_async_root_never_nests_results_under_async() {
        // C7's specific bug: the old `async_root.parent()/results` for a standard async_root
        // nested `results` UNDER `async`. The fix must NOT.
        let roots = run_artifact_roots(Path::new("/home/me/project"));
        let recovered = results_dir_for_async_root(&roots.async_root);
        assert!(
            !recovered.starts_with(&roots.async_root),
            "results dir must never live underneath the async root: {recovered:?}"
        );
        // Explicitly reject the exact wrong path the pre-fix code produced.
        let wrong = roots.async_root.parent().unwrap().join("results");
        assert_ne!(recovered, wrong.join(roots.async_root.file_name().unwrap()));
    }

    #[test]
    fn results_dir_for_async_root_degrades_for_a_non_standard_async_root() {
        // A bare `<base>/async` (no per-cwd key beneath a subagents home) is not the standard
        // layout; the fallback degrades to a `results` sibling of async_root's parent, matching
        // the fixed-layout fixtures the low-level runner subcommand's own unit tests assume.
        let recovered = results_dir_for_async_root(Path::new("/base/async"));
        assert_eq!(recovered, PathBuf::from("/base/results"));
    }

    #[tokio::test]
    async fn ensure_accessible_dir_creates_a_missing_nested_directory() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let nested = dir.path().join("a").join("b").join("results");
        assert!(!nested.exists());
        ensure_accessible_dir(&nested).await.expect("creates the nested dir");
        assert!(nested.is_dir(), "the full nested path must exist and be a directory");
    }

    #[tokio::test]
    async fn ensure_accessible_dir_is_idempotent_on_an_existing_writable_dir() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let target = dir.path().join("results");
        ensure_accessible_dir(&target).await.expect("first call creates");
        // A probe file must NOT be left behind by the accessibility check.
        ensure_accessible_dir(&target).await.expect("second call is a no-op");
        let mut leftover_probes = 0usize;
        let mut entries = tokio::fs::read_dir(&target).await.expect("readdir");
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_string_lossy().starts_with(".cyrup-access-probe-") {
                leftover_probes += 1;
            }
        }
        assert_eq!(leftover_probes, 0, "the write probe must always be cleaned up");
    }
}
