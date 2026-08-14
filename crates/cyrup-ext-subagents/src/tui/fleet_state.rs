//! The FleetView-relevant projection of pi-subagents' `SubagentState` — the shapes
//! `src/tui/fleet.ts` and `src/tui/fleet-status.ts` read, ported from
//! `src/shared/types.ts` (`@v0.43.0`).
//!
//! Upstream keeps these interfaces in `shared/types.ts`, not in the three `fleet*.ts` files:
//! `ForegroundRunControl` (`:1503-1543`), `ForegroundChildControl` (`:1481-1501`),
//! `ForegroundResumeRun` (`:1470-1479`), `ForegroundResumeChild` (`:1430-1468`),
//! `NestedRunSummary` (`:1121-…`), `NestedStepSummary` (`:1087-1119`) and `SubagentState`
//! (`:1560-…`). This module ports **only the fields the FleetView surfaces actually read** — a
//! faithful subset, not a truncation of convenience: every field below is named at a concrete
//! `fleet.ts` / `fleet-status.ts` line, and every field of those interfaces that no fleet surface
//! reads is deliberately absent.
//!
//! # Why a projection rather than reusing the live extension types directly
//!
//! cyrup's live foreground registry (`extension.rs`'s `ForegroundControlEntry`) carries four
//! fields — the interrupt token, `current_agent`, `current_index`, `current_activity_state` — the
//! same subset `background::fleet_view::ForegroundFleetEntry` already documents as delta 4 of the
//! text fleet view. The FleetView reads considerably more than that. Porting the upstream *model*
//! in full, and letting the producer fill in what it actually has (everything else defaulting to
//! `None`), keeps the ported rendering/selection logic byte-faithful to upstream instead of
//! silently deleting the branches that read a field cyrup does not populate yet. A field that is
//! always `None` renders exactly as upstream renders an absent optional — which is the correct
//! behaviour, and is visibly recoverable the moment the producer starts supplying it.
//!
//! The background half needs no such projection: [`AsyncRunView`] wraps cyrup's own
//! [`RunStatus`] verbatim, because `RunStatus` already IS pi's `AsyncStatus`/`AsyncRunSummary`
//! shape (mode, state, steps, telemetry, session file, timestamps) — see
//! `background/fleet_view.rs`'s own deltas 1-3 for the three step-level fields it lacks
//! (`label`, `phase`, per-step `context`).

use std::path::PathBuf;

use crate::background::{ActivityState, RunMode, RunPaths, RunStatus, StepState};
use crate::fork_context::ContextMode;

// =================================================================================================
// Nested descendants (pi `NestedRunSummary` / `NestedStepSummary`, shared/types.ts:1087-1160)
// =================================================================================================

/// pi `NestedStepSummary` (`shared/types.ts:1087-1119`), restricted to the nine fields
/// `fleet-status.ts:100-123` reads when building a nested tree row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NestedStepView {
    /// pi `agent` — the row's name (`fleet-status.ts:106`).
    pub agent: String,
    /// pi `status` — one of pi's eight step statuses, kept as the rendered string because
    /// `nestedStatusGlyph` (`fleet-status.ts:89-95`) compares it against literals including
    /// `"completed"` and `"rejected"`, which cyrup's [`StepState`] does not enumerate.
    pub status: String,
    /// pi `model` (`:103`).
    pub model: Option<String>,
    /// pi `thinking` (`:103`).
    pub thinking: Option<String>,
    /// pi `startedAt` (`:110`).
    pub started_at: Option<i64>,
    /// pi `currentTool` (`:82`).
    pub current_tool: Option<String>,
    /// pi `currentPath` (`:83`).
    pub current_path: Option<String>,
    /// pi `activityState` (`:84-85`).
    pub activity_state: Option<ActivityState>,
    /// pi `lastActivityAt` — read by the fleet-status render key (`fleet-status.ts:514`).
    pub last_activity_at: Option<i64>,
}

/// pi `NestedRunSummary` (`shared/types.ts:1121-…`), restricted to what
/// `fleet-status.ts:75-127,508-515` reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NestedRunView {
    /// pi `id` — the fallback label when neither `agent` nor `agents` is set (`:78`).
    pub id: String,
    /// pi `agent` (`:76`).
    pub agent: Option<String>,
    /// pi `agents` (`:77`).
    pub agents: Vec<String>,
    /// pi `mode` — only `parallel`/`chain` expand their `steps` (`:100`).
    pub mode: Option<RunMode>,
    /// pi `state` — kept as the rendered string for the same reason as [`NestedStepView::status`].
    pub state: String,
    /// pi `model` (`:115`).
    pub model: Option<String>,
    /// pi `thinking` (`:115`).
    pub thinking: Option<String>,
    /// pi `startedAt` (`:122`).
    pub started_at: Option<i64>,
    /// pi `lastUpdate` — read by the fleet-status render key (`:513`).
    pub last_update: Option<i64>,
    /// pi `currentTool` (`:82`).
    pub current_tool: Option<String>,
    /// pi `currentPath` (`:83`).
    pub current_path: Option<String>,
    /// pi `activityState` (`:84-85`).
    pub activity_state: Option<ActivityState>,
    /// pi `steps` (`:100`).
    pub steps: Vec<NestedStepView>,
    /// pi `parentStepIndex` (from `NestedRunAddress`) — the owner step a nested run hangs off,
    /// used by `fleet-status.ts:161-163,231-232` to attach nested children to the right row.
    pub parent_step_index: Option<usize>,
}

impl NestedStepView {
    /// Project one on-disk [`crate::background::StepStatus`] onto the nested-tree row shape
    /// `fleet-status.ts:100-123` renders.
    #[must_use]
    pub fn from_step(step: &crate::background::StepStatus) -> Self {
        Self {
            agent: step.agent.clone(),
            status: step_status_label(step.status).to_string(),
            model: step.model.as_ref().map(|m| m.as_str().to_string()),
            thinking: step.telemetry.thinking.clone(),
            started_at: step.started_at,
            current_tool: step.telemetry.current_tool.clone(),
            current_path: step.telemetry.current_path.clone(),
            activity_state: step.telemetry.activity_state,
            last_activity_at: step.telemetry.last_activity_at,
        }
    }
}

impl NestedRunView {
    /// Project one nested run's own on-disk [`RunStatus`] onto pi's `NestedRunSummary` shape
    /// (`statusToSummary`'s nested half, `runs/background/async-status.ts:291`).
    ///
    /// ONE level deep, deliberately: pi's own nested summary carries its `steps` but not its
    /// grandchildren's, and reading further would be the recursive reconcile
    /// [`crate::background::fleet_view`] declines to do. `parent_step_index` is the owner step this
    /// run hangs off, which is what `fleet-status.ts:161-163` attaches the row to.
    #[must_use]
    pub fn from_run_status(
        id: impl Into<String>,
        status: &RunStatus,
        parent_step_index: Option<usize>,
    ) -> Self {
        // pi's nested row prefers a single `agent` and falls back to the `agents` list (`:76-78`);
        // a one-step run has the former, a multi-step run the latter.
        let agents: Vec<String> = status.steps.iter().map(|s| s.agent.clone()).collect();
        let single = (agents.len() == 1).then(|| agents.first().cloned()).flatten();
        // The run's telemetry rolls up from whichever step is live (`RunStatus::sync_top_level_
        // telemetry`), so the model/thinking shown is that step's.
        let live = status
            .current_step
            .and_then(|i| status.steps.get(i))
            .or_else(|| status.steps.first());
        Self {
            id: id.into(),
            agent: single,
            agents: if agents.len() == 1 { Vec::new() } else { agents },
            mode: Some(status.mode),
            state: crate::background::run_status::run_state_label(status.state).to_string(),
            model: live.and_then(|s| s.model.as_ref().map(|m| m.as_str().to_string())),
            thinking: live.and_then(|s| s.telemetry.thinking.clone()),
            started_at: Some(status.started_at),
            last_update: Some(status.last_update),
            current_tool: status.telemetry.current_tool.clone(),
            // `RunTelemetry` carries no run-level `current_path` (only the per-step telemetry
            // does), so it comes from the same live step the model does.
            current_path: live.and_then(|s| s.telemetry.current_path.clone()),
            activity_state: status.telemetry.activity_state,
            steps: status.steps.iter().map(NestedStepView::from_step).collect(),
            parent_step_index,
        }
    }
}

// =================================================================================================
// Live foreground runs (pi `ForegroundRunControl` / `ForegroundChildControl`)
// =================================================================================================

/// pi `ForegroundChildControl` (`shared/types.ts:1481-1501`) — one independently-tracked child of
/// a live foreground parallel run, as `fleet.ts:145-157` and `fleet-status.ts:151-164` read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForegroundChildView {
    /// pi `index` — the flat child index; the roster sorts on it (`fleet.ts:145`).
    pub index: usize,
    /// pi `agent`.
    pub agent: String,
    /// pi `description` — the caller-facing task shown on the roster row (`fleet.ts:156`).
    pub description: Option<String>,
    /// pi `startedAt` — the elapsed-time reference (`fleet.ts:404`).
    pub started_at: i64,
    /// pi `updatedAt` — the roster's sort key (`fleet.ts:153`).
    pub updated_at: i64,
    /// pi `model` (`fleet.ts:243`).
    pub model: Option<String>,
    /// pi `thinking` (`fleet.ts:243`).
    pub thinking: Option<String>,
    /// pi `tokens` (`fleet.ts:255,403`).
    pub tokens: Option<u64>,
    /// pi `toolCount` (`fleet.ts:254,403`).
    pub tool_count: Option<u64>,
    /// pi `turnCount` (`fleet.ts:253`).
    pub turn_count: Option<u64>,
    /// pi `currentTool` (`fleet.ts:252`).
    pub current_tool: Option<String>,
    /// pi `currentPath` (`fleet.ts:252`).
    pub current_path: Option<String>,
}

/// pi `ForegroundRunControl` (`shared/types.ts:1503-1543`) — one live foreground run, as
/// `fleet.ts:142-172` and `fleet-status.ts:149-179` read it.
///
/// See the module doc for why fields cyrup's `ForegroundControlEntry` does not yet carry are
/// present-but-`None` rather than deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundControlView {
    /// pi `runId`.
    pub run_id: String,
    /// pi `sessionId`.
    pub session_id: Option<String>,
    /// pi `mode` — the agent label when no `currentAgent` is known (`fleet.ts:166`).
    pub mode: RunMode,
    /// pi `startedAt` (`fleet-status.ts:174`).
    pub started_at: i64,
    /// pi `updatedAt` — the outer sort key (`fleet.ts:142`).
    pub updated_at: i64,
    /// pi `cwd` — the artifacts root a live transcript target resolves against (`fleet.ts:344`).
    pub cwd: Option<PathBuf>,
    /// pi `currentAgent` (`fleet.ts:166`).
    pub current_agent: Option<String>,
    /// pi `currentIndex` (`fleet.ts:162,165`).
    pub current_index: Option<usize>,
    /// pi `description` (`fleet.ts:170`).
    pub description: Option<String>,
    /// pi `currentActivityState`.
    pub activity_state: Option<ActivityState>,
    /// pi `currentTool` (`fleet.ts:252`).
    pub current_tool: Option<String>,
    /// pi `currentPath` (`fleet.ts:252`).
    pub current_path: Option<String>,
    /// pi `turnCount` (`fleet.ts:253`).
    pub turn_count: Option<u64>,
    /// pi `toolCount` (`fleet.ts:254`).
    pub tool_count: Option<u64>,
    /// pi `tokens` (`fleet.ts:255`).
    pub tokens: Option<u64>,
    /// pi `model` (`fleet.ts:243`).
    pub model: Option<String>,
    /// pi `thinking` (`fleet.ts:243`).
    pub thinking: Option<String>,
    /// pi `activeChildren` — a `Map<number, ForegroundChildControl>` upstream; a `Vec` here, sorted
    /// by [`ForegroundChildView::index`] exactly where `fleet.ts:145` sorts the map's values.
    pub active_children: Vec<ForegroundChildView>,
    /// pi `nestedChildren` (`fleet-status.ts:161-163,177`).
    pub nested_children: Vec<NestedRunView>,
}

/// [`RunMode`] has no `Default`, so both run-shaped views spell theirs out. `Single` is pi's own
/// baseline run shape (`SubagentRunMode`'s first member and the shape a bare `subagent(...)` call
/// produces), so an unpopulated view reads as a single run rather than an invented one.
impl Default for ForegroundControlView {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            session_id: None,
            mode: RunMode::Single,
            started_at: 0,
            updated_at: 0,
            cwd: None,
            current_agent: None,
            current_index: None,
            description: None,
            activity_state: None,
            current_tool: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            tokens: None,
            model: None,
            thinking: None,
            active_children: Vec::new(),
            nested_children: Vec::new(),
        }
    }
}

// =================================================================================================
// Settled foreground runs (pi `ForegroundResumeRun` / `ForegroundResumeChild`)
// =================================================================================================

/// pi `ForegroundResumeChild` (`shared/types.ts:1430-1468`) — one settled child of a foreground
/// run, as `fleet.ts:216-226,263-287` reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForegroundResumeChildView {
    /// pi `agent`.
    pub agent: String,
    /// pi `index`.
    pub index: usize,
    /// pi `status` — one of pi's `SubagentResultStatus` strings; kept as a string because
    /// `statusGlyph` (`fleet.ts:232-237`) compares it against literals including `"detached"`.
    pub status: String,
    /// pi `updatedAt` (`fleet.ts:223`).
    pub updated_at: Option<i64>,
    /// pi `context` — the fork/fresh badge (`fleet.ts:272,381`).
    pub context: Option<ContextMode>,
    /// pi `model` (`fleet.ts:266`).
    pub model: Option<String>,
    /// pi `thinking` (`fleet.ts:266`).
    pub thinking: Option<String>,
    /// pi `sessionFile` (`fleet.ts:276`).
    pub session_file: Option<PathBuf>,
    /// pi `transcriptPath` — the transcript-pane target (`fleet.ts:351-354`).
    pub transcript_path: Option<PathBuf>,
    /// pi `savedOutputPath` (`fleet.ts:265`).
    pub saved_output_path: Option<PathBuf>,
    /// pi `artifactPaths.outputPath` (`fleet.ts:265`).
    pub artifact_output_path: Option<PathBuf>,
    /// pi `error` (`fleet.ts:278`).
    pub error: Option<String>,
    /// pi `outputSaveError` (`fleet.ts:279`).
    pub output_save_error: Option<String>,
    /// pi `transcriptError` (`fleet.ts:280`).
    pub transcript_error: Option<String>,
    /// pi `finalOutput` — the tail rendered when no transcript file is reachable (`fleet.ts:284`).
    pub final_output: Option<String>,
    /// pi `tokens` (`fleet.ts:407`).
    pub tokens: Option<u64>,
    /// pi `toolCount` (`fleet.ts:408`).
    pub tool_count: Option<u64>,
}

/// pi `ForegroundResumeRun` (`shared/types.ts:1470-1479`) — a foreground run whose control entry
/// is gone but whose children are still inspectable, as `fleet.ts:211-228` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundResumeRunView {
    /// pi `runId`.
    pub run_id: String,
    /// pi `mode` (`fleet.ts:271`).
    pub mode: RunMode,
    /// pi `cwd` — resolves a relative `transcriptPath` (`fleet.ts:354`).
    pub cwd: PathBuf,
    /// pi `sessionId` — the current-session filter (`fleet.ts:212`).
    pub session_id: Option<String>,
    /// pi `updatedAt` (`fleet.ts:213,223`).
    pub updated_at: i64,
    /// pi `children`.
    pub children: Vec<ForegroundResumeChildView>,
}

/// Same rationale as [`ForegroundControlView`]'s own `Default`.
impl Default for ForegroundResumeRunView {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            mode: RunMode::Single,
            cwd: PathBuf::new(),
            session_id: None,
            updated_at: 0,
            children: Vec::new(),
        }
    }
}

// =================================================================================================
// Background runs (pi `AsyncRunSummary` / `AsyncJobState`)
// =================================================================================================

/// One background run as the FleetView sees it — pi's `AsyncRunSummary`
/// (`runs/background/async-status.ts`), which `fleet.ts:109-134` and the tracked-job projection
/// `trackedJobSummary` (`fleet.ts:67-107`) both produce.
///
/// cyrup's [`RunStatus`] already carries pi's whole `AsyncStatus` shape, so this wrapper adds only
/// the three things `RunStatus` does not: the run's directory, the session that owns it, and the
/// caller-facing description pi keeps on `AsyncJobState.description` rather than in `status.json`.
#[derive(Clone, Debug)]
pub struct AsyncRunView {
    /// The run's resolved on-disk paths. Holds pi's `asyncDir` as [`RunPaths::run_dir`] — the
    /// transcript pane's trusted root (`fleet.ts:372`) and the `output-<i>.log` base
    /// (`fleet.ts:294`) — plus the `events.jsonl`/`subagent-log-*.md` artifacts
    /// [`crate::background::fleet_view::format_async_run_transcript`] needs.
    pub paths: RunPaths,
    /// The reconciled on-disk status: pi's `state`/`mode`/`steps`/`startedAt`/`endedAt`/
    /// `lastUpdate`/`sessionFile`/`totalTokens`/`toolCount`, all in one.
    pub status: RunStatus,
    /// pi `sessionId` — the `belongsToCurrentSession` filter's input (`fleet.ts:63-65`).
    pub session_id: Option<String>,
    /// pi `AsyncJobState.description`, threaded onto the item as `FleetItem.description`
    /// (`fleet.ts:112,124,187`).
    pub description: Option<String>,
    /// pi `context` — the run-level fork/fresh badge (`fleet.ts:299-300,380`).
    pub context: Option<ContextMode>,
    /// pi `nestedChildren` (`fleet-status.ts:193,212`).
    pub nested_children: Vec<NestedRunView>,
}

impl AsyncRunView {
    /// pi `asyncDir` — the run's own directory.
    #[must_use]
    pub fn dir(&self) -> &std::path::Path {
        &self.paths.run_dir
    }

    /// pi `run.lastUpdate ?? run.endedAt ?? run.startedAt` (`fleet.ts:110,129`) — the value both
    /// the item's `updatedAt` and [`order_fleet_async_runs`](super::fleet::order_fleet_async_runs)
    /// sort on.
    #[must_use]
    pub fn updated_at(&self) -> i64 {
        self.status.last_update
    }

    /// The rendered lowercase state string (pi `AsyncRunSummary.state`).
    #[must_use]
    pub fn state_label(&self) -> &'static str {
        crate::background::run_status::run_state_label(self.status.state)
    }

    /// pi's `run.state === "queued" || run.state === "running"` active test (`fleet.ts:131`).
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self.status.state,
            crate::background::RunState::Queued | crate::background::RunState::Running
        )
    }
}

/// The rendered lowercase status string for one step (pi `AsyncJobStep.status`).
#[must_use]
pub fn step_status_label(state: StepState) -> &'static str {
    crate::background::run_status::step_state_label(state)
}

// =================================================================================================
// The whole projection (pi `SubagentState`, shared/types.ts:1560-…)
// =================================================================================================

/// The FleetView-relevant projection of pi's `SubagentState` — the single argument
/// [`collect_fleet_snapshot`](super::fleet::collect_fleet_snapshot) and
/// [`collect_fleet_status_entries`](super::fleet_status::collect_fleet_status_entries) take, where
/// upstream passes `state` itself.
#[derive(Clone, Debug, Default)]
pub struct FleetState {
    /// SUBA-048 — pi `state.artifactDirPreference ?? "project"` (`fleet.ts:334-340`'s
    /// `fleetArtifactsRoot`, seeded from `config.artifactDir` at `extension/index.ts:375`).
    /// `Default` is [`crate::artifacts::ArtifactDirPreference::Project`], pi's own default, so an
    /// unset config behaves exactly as before this field existed.
    pub artifact_dir_preference: crate::artifacts::ArtifactDirPreference,
    /// pi `state.baseCwd` — the artifacts-root fallback for transcript targeting
    /// (`fleet.ts:344,359,373`).
    pub base_cwd: PathBuf,
    /// pi `state.currentSessionId` — `belongsToCurrentSession`'s second argument
    /// (`fleet.ts:63-65`). `None` means "no session filter", exactly as pi's `null` does.
    pub current_session_id: Option<String>,
    /// pi `state.parentSessionFile` — the artifacts-dir resolver's first input (`fleet.ts:336`).
    pub parent_session_file: Option<PathBuf>,
    /// pi `state.foregroundControls` (a `Map` upstream; ordered by
    /// [`ForegroundControlView::updated_at`] descending at `fleet.ts:142`).
    pub foreground_controls: Vec<ForegroundControlView>,
    /// pi `state.foregroundRuns` — settled runs still worth inspecting (`fleet.ts:211`).
    pub foreground_runs: Vec<ForegroundResumeRunView>,
    /// pi `state.fleetJobs ?? state.asyncJobs` — the in-memory tracked background jobs
    /// (`fleet.ts:178`).
    pub tracked_jobs: Vec<AsyncRunView>,
    /// pi's `listAsyncRuns(options.asyncDirRoot, …)` result (`fleet.ts:194-199`) — on-disk runs
    /// this process is not tracking in memory. Kept as a separate list because pi filters it
    /// against the tracked ids before concatenating (`fleet.ts:193,199`).
    pub history_jobs: Vec<AsyncRunView>,
    /// pi `state.fleetInspectorOpen` (`fleet.ts:844-845`, `fleet-status.ts:306`) — while true the
    /// status widget unregisters itself so the two surfaces never render at once.
    pub fleet_inspector_open: bool,
    /// The failure pi's own `try`/`catch` around the background half of `collectFleetSnapshot`
    /// records (`fleet.ts:174-209`), carried on the state because the scan that can fail
    /// ([`super::fleet::collect_fleet_history`]) is `async` and therefore runs in the producer, not
    /// inside the synchronous fold. [`super::fleet::collect_fleet_snapshot`] copies it onto
    /// [`super::fleet::FleetSnapshot::error`], which is what the roster renders as a
    /// `Fleet scan warning:` line rather than as an empty roster.
    pub scan_error: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::background::{RunId, RunPaths, RunStatus, StepStatus};
    use cyrup_core::ModelId;

    fn step(agent: &str, state: StepState) -> StepStatus {
        let mut s = StepStatus::pending(agent.to_string());
        s.status = state;
        s
    }

    fn run(steps: Vec<StepStatus>) -> RunStatus {
        let mut status = RunStatus::queued(RunId::from_token("r1".to_string()), RunMode::Chain, None);
        status.steps = steps;
        status.started_at = 1_000;
        status.last_update = 2_000;
        status
    }

    /// `NestedRunView::from_run_status`'s single/multi-agent split — pi's nested row prefers a
    /// single `agent` and falls back to the `agents` LIST (`fleet-status.ts:76-78`). Populating
    /// both, or neither, is what makes a nested row render blank or doubled.
    #[test]
    fn a_one_step_nested_run_names_one_agent_and_a_multi_step_run_lists_them() {
        let one = NestedRunView::from_run_status("n1", &run(vec![step("solo", StepState::Running)]), None);
        assert_eq!(one.agent.as_deref(), Some("solo"));
        assert!(one.agents.is_empty(), "the list form must stay empty when there is a single agent");

        let many = NestedRunView::from_run_status(
            "n2",
            &run(vec![step("first", StepState::Complete), step("second", StepState::Running)]),
            Some(3),
        );
        assert_eq!(many.agent, None, "no single agent to name");
        assert_eq!(many.agents, vec!["first".to_string(), "second".to_string()]);
        assert_eq!(many.parent_step_index, Some(3), "the owner step the row hangs off");
        assert_eq!(many.steps.len(), 2, "one level deep, but that level in full");
    }

    /// The model/thinking a nested row shows comes from the LIVE step (`current_step`), not from
    /// step 0 — a chain that has advanced would otherwise report the finished step's model forever.
    #[test]
    fn the_nested_row_reads_its_model_from_the_current_step_not_the_first() {
        let mut first = step("first", StepState::Complete);
        first.model = Some(ModelId::from("stale-model"));
        first.telemetry.thinking = Some("off".to_string());
        let mut second = step("second", StepState::Running);
        second.model = Some(ModelId::from("live-model"));
        second.telemetry.thinking = Some("high".to_string());
        second.telemetry.current_path = Some("src/live.rs".to_string());

        let mut status = run(vec![first, second]);
        status.current_step = Some(1);
        let view = NestedRunView::from_run_status("n", &status, None);
        assert_eq!(view.model.as_deref(), Some("live-model"));
        assert_eq!(view.thinking.as_deref(), Some("high"));
        // `RunTelemetry` carries no run-level `current_path`, so it comes from the same live step.
        assert_eq!(view.current_path.as_deref(), Some("src/live.rs"));
    }

    /// With no `current_step` at all the projection falls back to the first step rather than
    /// dropping the model — pi's `?? steps[0]`.
    #[test]
    fn a_run_with_no_current_step_falls_back_to_the_first() {
        let mut only = step("only", StepState::Pending);
        only.model = Some(ModelId::from("m0"));
        let mut status = run(vec![only]);
        status.current_step = None;
        assert_eq!(NestedRunView::from_run_status("n", &status, None).model.as_deref(), Some("m0"));

        // …and a run with no steps at all projects no model rather than panicking.
        let empty = NestedRunView::from_run_status("n", &run(Vec::new()), None);
        assert_eq!(empty.model, None);
        assert_eq!(empty.agent, None);
        assert!(empty.agents.is_empty());
        assert!(empty.steps.is_empty());
    }

    /// `NestedStepView::from_step` must carry every telemetry field the nested tree row renders —
    /// a dropped one shows as a permanently idle child.
    #[test]
    fn a_nested_step_view_carries_its_whole_telemetry_row() {
        let mut s = step("worker", StepState::Running);
        s.model = Some(ModelId::from("m"));
        s.started_at = Some(500);
        s.telemetry.thinking = Some("low".to_string());
        s.telemetry.current_tool = Some("bash".to_string());
        s.telemetry.current_path = Some("a/b.rs".to_string());
        s.telemetry.activity_state = Some(ActivityState::NeedsAttention);
        s.telemetry.last_activity_at = Some(900);

        let view = NestedStepView::from_step(&s);
        assert_eq!(view.agent, "worker");
        assert_eq!(view.status, step_status_label(StepState::Running));
        assert_eq!(view.model.as_deref(), Some("m"));
        assert_eq!(view.thinking.as_deref(), Some("low"));
        assert_eq!(view.started_at, Some(500));
        assert_eq!(view.current_tool.as_deref(), Some("bash"));
        assert_eq!(view.current_path.as_deref(), Some("a/b.rs"));
        assert_eq!(view.activity_state, Some(ActivityState::NeedsAttention));
        assert_eq!(view.last_activity_at, Some(900));
    }

    /// The labels are the strings the roster prints; they must stay lowercase and distinct, since
    /// the status glyph/role is selected from them.
    #[test]
    fn every_step_state_has_its_own_lowercase_label() {
        let states = [
            StepState::Pending,
            StepState::Running,
            StepState::Paused,
            StepState::Complete,
            StepState::Failed,
            StepState::Stopped,
        ];
        let labels: Vec<&str> = states.iter().copied().map(step_status_label).collect();
        for label in &labels {
            assert_eq!(*label, label.to_lowercase(), "{label} must be lowercase");
            assert!(!label.is_empty());
        }
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "each state renders distinctly: {labels:?}");
    }

    /// [`AsyncRunView`]'s four accessors are what the roster sorts and labels by.
    #[test]
    fn an_async_run_view_reports_its_dir_timestamps_and_activity() {
        let mut status = run(vec![step("a", StepState::Running)]);
        status.state = crate::background::RunState::Running;
        let view = AsyncRunView {
            paths: RunPaths::for_run(
                std::path::Path::new("/tmp/async"),
                std::path::Path::new("/tmp/results"),
                &RunId::from_token("r1".to_string()),
            ),
            status,
            session_id: None,
            description: None,
            context: None,
            nested_children: Vec::new(),
        };
        assert_eq!(view.updated_at(), 2_000, "the run's own last_update");
        assert!(view.is_active(), "a Running run holds the roster row open");
        assert_eq!(view.state_label(), crate::background::run_status::run_state_label(crate::background::RunState::Running));
        assert!(view.dir().ends_with("r1"));
    }
}
