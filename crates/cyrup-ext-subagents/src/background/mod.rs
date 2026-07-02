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
//!   [`NestedRoute`] addressing type and [`RunPaths::nested`]'s pure subpath-naming rule; the
//!   actual recursive liveness roll-up belongs to `background/reconcile.rs`/`tracker.rs`.

/// The shared atomic-write primitive (`write_atomic_json`, R-SA-076/135) used by every other
/// module in this subsystem (status.json, config.json, meta.json, control-inbox files) so there
/// is exactly one temp-then-rename implementation, not one per call site.
pub mod atomic;

/// File-based interrupt/resume/append-step control protocol (R-SA-079..087, R-SA-094..097): dual-
/// channel interrupt delivery, delete-then-act idempotent consumption, the resume running-
/// selection-vs-terminal-revival fork, the append-step enqueue-then-consume race-safe protocol,
/// safe-token path validation, and root-attachment polling. See [`control`] for the full subsystem
/// doc.
pub mod control;

/// Hop-1 detached second-process spawn (`spawn_detached_runner`, R-SA-070/071): launches the
/// `cyrup` binary's internal `__subagent-runner --config <path>` subcommand as a genuinely
/// detached OS process (new process group / `DETACHED_PROCESS`, stdio redirected to files, the
/// resulting child handle dropped without ever being awaited). See that module's docs for why
/// "never awaited" is the entire point, not an oversight.
pub mod spawn_detached;

/// Stale-run liveness reconciliation (`reconcile`, R-SA-088..092): given a run id's resolved
/// [`RunPaths`], applies the exact five-step algorithm — `ResultFile` presence is always
/// authoritative; a missing `status.json` is provisional within the R-SA-090 spawn grace window
/// and failed thereafter; a non-`Running`/no-pid status passes through unchanged; a `Running`
/// status with a pid is probed via a real zero-signal liveness check (R-SA-089's three-outcome
/// `Alive`/`Dead`/`Unknown` classification, never collapsing `Unknown` into `Dead`) and, if dead or
/// alive-but-long-stale (R-SA-091), synthesizes a failure written to both files (R-SA-092). Every
/// other module that needs "is this run actually still alive" (`background/control.rs`'s
/// `status`/`interrupt`/`resume`/`append-step` handlers, R-SA-079; `background/tracker.rs`'s
/// shared poller, R-SA-093) calls this module's `reconcile`/`reconcile_now`, never re-derives the
/// algorithm.
pub mod reconcile;

/// Hop-2 detached-runner main loop (`run`, R-SA-073..077): reads+deletes the one-shot
/// `runner-config.json` handoff file, writes the initial `Running` status, drives the step loop
/// (interrupt/append-request checks every iteration, dispatch via the Phase-3 spawn boundary),
/// and — on every single exit path — writes the terminal `status.json` strictly before the
/// terminal [`ResultFile`] (R-SA-077), which is what makes [`watch`]'s orchestrator-side
/// completion notification observable at all. See [`runner_main`] for the full subsystem doc.
pub mod runner_main;

/// `ResultsDir` filesystem-watch completion notification (R-SA-098..103): a `notify`-crate watch
/// with poll-interval fallback over the shared `ResultsDir`, parse+session-verify+dedup+notify
/// processing per R-SA-099, OR'd dual-signal terminal-state classification (R-SA-100), and bounded
/// retry-in-place on transient processing failure (R-SA-102). Runs entirely in the ORCHESTRATOR
/// process — see [`watch`] for the full subsystem doc, including the explicit scope note on why
/// R-SA-101's turn/prompt-path re-entry is a later phase's hand-off, not implemented here.
pub mod watch;

/// Orchestrator-side shared poller (`JobTracker`, R-SA-093/105): one `tokio::time::interval`-
/// driven task per owning extension instance, self-starting on the first tracked job and
/// self-stopping once the tracked-job map empties. Tails newly-appended `events.jsonl` bytes per
/// tracked job via a per-run byte cursor and invokes [`reconcile::reconcile_now`] every tick. See
/// [`tracker`] for the full subsystem doc.
pub mod tracker;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        }
    }

    /// `true` once a run in this state will never again be mutated by the runner that owns it
    /// (`Complete`/`Failed`). `Paused` is deliberately **not** terminal — R-SA-084 is explicit
    /// that interrupt is a soft, resumable pause, never a terminal state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, RunState::Complete | RunState::Failed)
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
    /// - `Complete`/`Failed` are terminal: no outgoing transition is permitted, including to
    ///   themselves — a caller that already observed a terminal state and tries to write the same
    ///   terminal state again should treat that as a no-op at a layer above this guard, not as a
    ///   fresh "transition".
    #[must_use]
    pub fn can_transition_to(self, next: RunState) -> bool {
        matches!(
            (self, next),
            (RunState::Queued, RunState::Running)
                | (RunState::Queued, RunState::Failed)
                | (RunState::Running, RunState::Paused)
                | (RunState::Running, RunState::Complete)
                | (RunState::Running, RunState::Failed)
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
}

impl StepState {
    /// `true` for `Complete`/`Failed` — mirrors [`RunState::is_terminal`]'s exclusion of `Paused`
    /// for the identical reason (R-SA-084: pause is soft and resumable, never terminal).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, StepState::Complete | StepState::Failed)
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
}

impl RunStatus {
    /// Constructs the initial `Queued` status for a freshly minted run, before the detached
    /// runner has written anything of its own. `started_at`/`last_update` are both set to "now".
    #[must_use]
    pub fn queued(run_id: RunId, mode: RunMode, pid: Option<u32>) -> Self {
        let now = now_epoch_millis();
        Self {
            run_id,
            mode,
            state: RunState::Queued,
            pid,
            started_at: now,
            ended_at: None,
            last_update: now,
            current_step: None,
            chain_step_count: None,
            pending_appends: None,
            steps: Vec::new(),
            parallel_groups: None,
        }
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
        let now = now_epoch_millis();
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
        self.last_update = now_epoch_millis();
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
/// itself, which lives in the sibling [`ResultsDir`] (func-SA §4.5 draws this distinction
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
// Time helper
// =================================================================================================

/// Current wall-clock time as epoch milliseconds, clamped to `i64::MAX`/`0` on the (practically
/// unreachable on any real system clock) chance the conversion would otherwise overflow/underflow
/// — never a panic. Kept private: every other module in this crate obtains a timestamp through
/// [`RunStatus`]'s own constructors/mutators rather than calling this directly, so there is one
/// place the epoch-conversion policy lives.
/// Public re-export of [`now_epoch_millis`]'s exact policy, for callers OUTSIDE this module that
/// still need to stamp an individual [`StepStatus`]/[`ParallelGroupStatus`] field directly (e.g.
/// `runner_main.rs`'s per-step `started_at`/`ended_at` bookkeeping, which mutates fields nested
/// inside `RunStatus.steps`/`RunStatus.parallel_groups` that no [`RunStatus`] method itself
/// exposes a setter for) — kept as a thin wrapper around the private [`now_epoch_millis`] rather
/// than making that function itself `pub`, so the module's own doc comment ("kept private: every
/// other module... obtains a timestamp through `RunStatus`'s own constructors/mutators") stays
/// accurate for every USE CASE that constructor/mutator surface already covers, while still
/// giving the narrow, genuinely-uncovered case (stamping a nested per-step field the top-level
/// `RunStatus` API has no setter for) a sanctioned entry point instead of a second, independently
/// reimplemented clamp-never-panic conversion.
#[must_use]
pub fn now_epoch_millis_pub() -> i64 {
    now_epoch_millis()
}

fn now_epoch_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        // A system clock set before the Unix epoch is not something this crate can do anything
        // sane about; 0 is a safe, non-panicking floor rather than propagating an error type
        // through every status-mutating call site for a condition that indicates a broken host
        // clock, not a bug in this crate's own logic.
        Err(_) => 0,
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
    // now_epoch_millis
    // ---------------------------------------------------------------------------------------

    #[test]
    fn now_epoch_millis_is_positive_and_monotonic_enough_for_ordering() {
        let a = now_epoch_millis();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = now_epoch_millis();
        assert!(a > 0);
        assert!(b >= a);
    }
}
