//! The live **subagent fleet inspector** — Rust port of pi-subagents `src/tui/fleet.ts`
//! (`@v0.43.0`, 879 lines), the full-screen overlay `/subagents-fleet` and `Ctrl+Alt+F` open
//! (`slash/slash-commands.ts:633-649,713-722`).
//!
//! A two-pane frame: a roster of every child this session can see on the left, and on the right
//! either that child's live transcript (rendered by [`super::fleet_transcript`]) or, when no
//! transcript is reachable, a structured detail block. Live controls sit on top of it — `s` steers
//! the selected background child, `D` stops it, `H` opens the Herdr inspector, `x`/`Ctrl+O`
//! expands tool output, `r` refreshes.
//!
//! # The four things this module owns
//!
//! 1. **[`collect_fleet_snapshot`]** (pi `:136-230`) — the roster. Three sources in pi's exact
//!    order: live foreground controls (newest first, expanded per active child), then background
//!    runs (in-memory tracked jobs first, then on-disk history not already tracked, ordered by
//!    [`order_fleet_async_runs`]), then settled foreground runs whose control entry is gone. A
//!    failure anywhere in the background half becomes [`FleetSnapshot::error`] and is rendered as
//!    a warning — it never empties the roster.
//! 2. **The detail pane** — [`detail_lines`] (pi `:309-318`) over the three per-kind builders
//!    (`:240-307`), and [`structured_header`] (pi `:425-440`) for the transcript-backed form.
//! 3. **[`transcript_target`]** (pi `:342-377`) — which file the transcript pane reads, and the
//!    trusted roots it must resolve inside. This is the security-relevant half: the roots come
//!    from the artifacts directory and the run directory, never from the transcript path itself.
//! 4. **[`SubagentFleetComponent`]** (pi `:476-841`) — the whole interactive state machine:
//!    selection with key preservation across refreshes, detail scrolling with auto-follow, the
//!    steer-draft line editor with its `Tab` mode cycle, the stop confirmation, the busy latch,
//!    and the transcript render cache keyed on `(path, size:mtime, width, expandedTools)`.
//!
//! # Transport difference (stated once, with its reason)
//!
//! pi's component implements `pi-tui`'s `Component` interface — `render(width): string[]` with
//! ANSI escapes, `handleInput(data: string)` fed by the TUI's raw input pump, a `setInterval`
//! re-render tick, and `ctx.ui.custom(factory, { overlay: true, … })` to host it
//! (`fleet.ts:869-875`). cyrup's extension host has no interactive-overlay seam:
//! [`cyrup_ext::HostServices::custom`] takes a serialized spec and returns an optional serialized
//! result (`cyrup-ext/src/host/services.rs:205`), there is no input subscription, and this crate
//! must not depend on `cyrup-tui` (arch-SA §1.1/§6.1).
//!
//! So the component here is a **plain owned state machine**: [`SubagentFleetComponent::refresh`],
//! [`SubagentFleetComponent::handle_input`] and [`SubagentFleetComponent::render`] are ordinary
//! methods an owner drives, and [`SubagentFleetComponent::render`] returns
//! [`Line<'static>`] values (paintable by a terminal owner, or flattened with
//! [`super::fleet_theme::lines_text`] for a text surface). Every behaviour above the
//! render/input boundary — every key binding, every state transition, every line of the frame — is
//! ported unchanged. [`open_subagent_fleet`] ports `showFleet`'s control flow
//! (`slash-commands.ts:633-649`), including its re-entrancy guard and its `!ctx.hasUI` fallback to
//! the v0.34.0 text fleet view.
//!
//! # Honest deltas vs. pi
//!
//! 1. **No steer delivery-mode transport.** The `Tab` cycle over
//!    [`SteerDeliveryMode`] (`steer`/`follow_up`/`auto`, pi `:628`) is ported and shown in the
//!    prompt, but cyrup's own steer entry point
//!    ([`crate::extension::SubagentExecutor::control_steer`]) takes no delivery mode, so all three
//!    currently deliver identically. The mode reaches the owner verbatim, on
//!    [`FleetPendingAction::Steer`], so wiring it is a one-line change the day the control channel
//!    grows the field.
//! 2. **No Herdr inspector.** `handleHerdrInspectorAction` lives in `src/inspectors/herdr/`, a
//!    subtree this crate does not port. pi already makes `inspect` OPTIONAL on its handler bundle
//!    (`fleet.ts:51`), so `H` takes pi's own "Herdr inspector controls are unavailable in this
//!    context." branch (`:692`) rather than a cyrup-invented one.
//! 3. **Per-step `label`/`phase`/`context` and nested descendants.** Same three gaps
//!    `background/fleet_view.rs` documents as its deltas 1-2: cyrup's [`StepStatus`] carries no
//!    user-facing label, no phase tag and no per-step fork context, so `label (agent)` collapses to
//!    the bare agent name and per-step context badges are absent.
//! 4. **No markdown/highlighting in the transcript pane** — see
//!    [`super::fleet_transcript`]'s own deltas 1-2.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::text::{Line, Span};

use super::fleet_state::{
    AsyncRunView, FleetState, ForegroundControlView, ForegroundResumeChildView,
    ForegroundResumeRunView, step_status_label,
};
use super::fleet_status::{FLEET_STATUS_WIDGET_KEY, collapse_whitespace};
use super::fleet_theme::{self as th, Role};
use super::fleet_transcript::{
    FleetTranscript, FleetTranscriptEvent, FleetTranscriptReadOptions, read_fleet_transcript,
    render_fleet_transcript,
};
use crate::background::{RunPaths, RunState, RunStatus};
use crate::fork_context::ContextMode;

// =================================================================================================
// Tunables (pi `fleet.ts:20-23`)
// =================================================================================================

/// pi `REFRESH_MS` (`fleet.ts:20`) — the inspector's own re-render cadence, distinct from the
/// status widget's 500 ms ([`super::fleet_status::REFRESH_MS`]).
pub const REFRESH_MS: u64 = 750;
/// pi `MAX_RECENT_ASYNC_RUNS` (`fleet.ts:21`).
pub const MAX_RECENT_ASYNC_RUNS: usize = 20;
/// pi `MAX_FLEET_HISTORY_CANDIDATES` (`fleet.ts:22`) — how many on-disk runs the history scan
/// considers before ordering.
pub const MAX_FLEET_HISTORY_CANDIDATES: usize = 100;
/// pi `TRANSCRIPT_LINES` (`fleet.ts:23`).
pub const TRANSCRIPT_LINES: usize = 200;

/// pi's minimum viable width (`fleet.ts:789`).
const MIN_WIDTH: usize = 36;

// =================================================================================================
// Item model (pi `fleet.ts:32-61`)
// =================================================================================================

/// Which of pi's three item kinds a roster row is (`fleet.ts:32-36`).
///
/// No `PartialEq`: the `Async` variant embeds a whole [`RunStatus`], which is not comparable, and
/// pi never compares items structurally either — selection is preserved by [`FleetItem::key`]
/// (`fleet.ts:527`), which is exactly what a stable key is for.
#[derive(Clone, Debug)]
pub enum FleetItemKind {
    /// pi `"foreground-active"` — a live foreground run (or one of its active children).
    ForegroundActive {
        /// The live control entry.
        control: Box<ForegroundControlView>,
        /// The specific active child, when the run tracks children independently.
        active_child_index: Option<usize>,
    },
    /// pi `"foreground-recent"` — a settled foreground child still worth inspecting.
    ForegroundRecent {
        /// The settled run.
        run: Box<ForegroundResumeRunView>,
        /// The settled child.
        child: Box<ForegroundResumeChildView>,
    },
    /// pi `"async"` — a background run, either whole or one of its steps.
    Async {
        /// The background run.
        run: Box<AsyncRunView>,
        /// The step index, when this row is one step of a multi-step run.
        step_index: Option<usize>,
    },
}

/// pi `FleetItem` (`fleet.ts:32-36`) — one roster row.
#[derive(Clone, Debug)]
pub struct FleetItem {
    /// pi `key` — the stable identity selection is preserved across refreshes by (`fleet.ts:527`).
    pub key: String,
    /// pi `kind`, carrying its variant-specific payload.
    pub kind: FleetItemKind,
    /// pi `runId`.
    pub run_id: String,
    /// pi `index` — the flat child/step index, when the row is a child rather than a whole run.
    pub index: Option<usize>,
    /// pi `agent`.
    pub agent: String,
    /// pi `state` — the rendered lowercase state string [`status_glyph`] switches on.
    pub state: String,
    /// pi `updatedAt`.
    pub updated_at: i64,
    /// pi `description`.
    pub description: Option<String>,
}

/// pi `FleetSnapshot` (`fleet.ts:38-41`).
#[derive(Clone, Debug, Default)]
pub struct FleetSnapshot {
    /// The roster, in pi's own order.
    pub items: Vec<FleetItem>,
    /// The one error the background half is allowed to fail with, surfaced as a warning rather
    /// than an empty roster (`fleet.ts:189,208,316,755`).
    pub error: Option<String>,
}

/// pi `FleetActionResult` (`fleet.ts:43-46`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetActionResult {
    /// The message to show under the header.
    pub text: String,
    /// pi `isError` — selects the error colour over success.
    pub is_error: bool,
}

impl FleetActionResult {
    /// A success notice.
    #[must_use]
    pub fn ok(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false }
    }
    /// An error notice.
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }
}

/// pi `SteerDeliveryMode` (`runs/background/control-channel.ts`), cycled by `Tab` in the steer
/// prompt (`fleet.ts:628`). See delta 1 for its current cyrup transport status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SteerDeliveryMode {
    /// pi `"steer"` — the default (`fleet.ts:488,543`).
    #[default]
    Steer,
    /// pi `"follow_up"`.
    FollowUp,
    /// pi `"auto"`.
    Auto,
}

impl SteerDeliveryMode {
    /// The wire/display string (pi's own union members, shown at `fleet.ts:559`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::FollowUp => "follow_up",
            Self::Auto => "auto",
        }
    }
    /// pi's `modes[(modes.indexOf(this.steerMode) + 1) % modes.length]` (`fleet.ts:628-629`).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Steer => Self::FollowUp,
            Self::FollowUp => Self::Auto,
            Self::Auto => Self::Steer,
        }
    }
}

/// The address one fleet action targets — pi's `{ runId, asyncDir, index? }` argument bundle
/// (`fleet.ts:49-51`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetActionTarget {
    /// pi `runId`.
    pub run_id: String,
    /// pi `asyncDir`.
    pub async_dir: PathBuf,
    /// pi `index`.
    pub index: Option<usize>,
}

// pi's `FleetActionHandlers` interface (`fleet.ts:48-52`) has no Rust counterpart here, and that
// is a shape difference rather than a dropped feature: pi's three handlers are `async`, and
// `handleInput` fires them through `runAction(...)` (`fleet.ts:585-597`), which it can do because a
// JS closure may start a promise from a synchronous handler. A Rust `handle_input` cannot `.await`,
// and cyrup's control ops (`SubagentExecutor::control_steer`/`control_stop`) are all `async`. So the
// component RETURNS the action it wants performed — [`FleetPendingAction`], carried on
// [`FleetInputOutcome::RunAction`] — and the owner awaits it and feeds the answer back through
// [`SubagentFleetComponent::finish_action`]. Every guard pi applies before dispatching (the
// actionable-state check, the `!this.options.actions` check, the `!actions?.inspect` check, the
// busy latch) is applied on this side of that boundary, unchanged.

/// pi `FleetViewOptions` (`fleet.ts:54-61`), minus the two fields that have no cyrup counterpart:
/// `markdownTheme` (delta 4) and the `asyncDirRoot`/`resultsDir` pair, which the caller resolves
/// into [`FleetState::history_jobs`] before constructing the component
/// ([`collect_fleet_history`] is that resolution).
#[derive(Clone, Copy, Debug)]
pub struct FleetViewOptions {
    /// pi `refreshMs`.
    pub refresh_ms: u64,
    /// pi `options.limit` — how many terminal background runs the roster keeps
    /// (`fleet.ts:182,204`).
    pub limit: usize,
}

impl Default for FleetViewOptions {
    fn default() -> Self {
        Self { refresh_ms: REFRESH_MS, limit: MAX_RECENT_ASYNC_RUNS }
    }
}

// =================================================================================================
// Snapshot collection (pi `fleet.ts:63-230`)
// =================================================================================================

/// pi `belongsToCurrentSession` (`fleet.ts:63-65`): no current session means "everything belongs".
#[must_use]
pub fn belongs_to_current_session(
    session_id: Option<&str>,
    current_session_id: Option<&str>,
) -> bool {
    match current_session_id {
        None => true,
        Some(current) => session_id == Some(current),
    }
}

/// pi `orderFleetAsyncRuns` (`fleet.ts:128-134`): every active run newest-first, then at most
/// `terminal_limit` terminal runs newest-first.
#[must_use]
pub fn order_fleet_async_runs(runs: &[AsyncRunView], terminal_limit: usize) -> Vec<AsyncRunView> {
    let mut active: Vec<AsyncRunView> = runs.iter().filter(|r| r.is_active()).cloned().collect();
    let mut terminal: Vec<AsyncRunView> =
        runs.iter().filter(|r| !r.is_active()).cloned().collect();
    active.sort_by_key(|run| std::cmp::Reverse(run.updated_at()));
    terminal.sort_by_key(|run| std::cmp::Reverse(run.updated_at()));
    terminal.truncate(terminal_limit);
    active.extend(terminal);
    active
}

/// pi `asyncItems(run, description)` (`fleet.ts:109-126`): a run with no steps renders as one row;
/// otherwise one row per step.
#[must_use]
pub fn async_items(run: &AsyncRunView) -> Vec<FleetItem> {
    let updated_at = run.updated_at();
    if run.status.steps.is_empty() {
        return vec![FleetItem {
            key: format!("async:{}", run.status.run_id.as_str()),
            kind: FleetItemKind::Async { run: Box::new(run.clone()), step_index: None },
            run_id: run.status.run_id.as_str().to_string(),
            index: None,
            agent: crate::background::run_status::run_mode_label(run.status.mode).to_string(),
            state: run.state_label().to_string(),
            updated_at,
            description: run.description.clone(),
        }];
    }
    run.status
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| FleetItem {
            key: format!("async:{}:{index}", run.status.run_id.as_str()),
            kind: FleetItemKind::Async {
                run: Box::new(run.clone()),
                step_index: Some(index),
            },
            run_id: run.status.run_id.as_str().to_string(),
            index: Some(index),
            // Delta 3: pi renders `label (agent)` when the step carries a label.
            agent: step.agent.clone(),
            state: step_status_label(step.status).to_string(),
            updated_at: step.telemetry.last_activity_at.unwrap_or(updated_at),
            description: run.description.clone(),
        })
        .collect()
}

/// pi `collectFleetSnapshot(state, options)` (`fleet.ts:136-230`).
#[must_use]
pub fn collect_fleet_snapshot(state: &FleetState, options: &FleetViewOptions) -> FleetSnapshot {
    let mut items: Vec<FleetItem> = Vec::new();
    let mut active_foreground_ids: HashSet<String> = HashSet::new();

    // (1) Live foreground controls, newest first (pi `:142`).
    let mut controls: Vec<&ForegroundControlView> = state.foreground_controls.iter().collect();
    controls.sort_by_key(|control| std::cmp::Reverse(control.updated_at));
    for control in controls {
        active_foreground_ids.insert(control.run_id.clone());
        if !control.active_children.is_empty() {
            let mut children = control.active_children.clone();
            children.sort_by_key(|child| child.index);
            for child in &children {
                items.push(FleetItem {
                    key: format!("foreground-active:{}:{}", control.run_id, child.index),
                    kind: FleetItemKind::ForegroundActive {
                        control: Box::new(control.clone()),
                        active_child_index: Some(child.index),
                    },
                    run_id: control.run_id.clone(),
                    index: Some(child.index),
                    agent: child.agent.clone(),
                    state: "running".to_string(),
                    updated_at: child.updated_at,
                    description: child.description.clone(),
                });
            }
            continue;
        }
        items.push(FleetItem {
            key: format!(
                "foreground-active:{}:{}",
                control.run_id,
                control.current_index.unwrap_or(0)
            ),
            kind: FleetItemKind::ForegroundActive {
                control: Box::new(control.clone()),
                active_child_index: None,
            },
            run_id: control.run_id.clone(),
            index: control.current_index,
            agent: control
                .current_agent
                .clone()
                .unwrap_or_else(|| {
                    crate::background::run_status::run_mode_label(control.mode).to_string()
                }),
            state: "running".to_string(),
            updated_at: control.updated_at,
            description: control.description.clone(),
        });
    }

    // (2) Background runs: tracked first, then untracked on-disk history (pi `:174-209`).
    let tracked: Vec<AsyncRunView> = state
        .tracked_jobs
        .iter()
        .filter(|job| {
            belongs_to_current_session(job.session_id.as_deref(), state.current_session_id.as_deref())
        })
        .cloned()
        .collect();
    let mut active: Vec<AsyncRunView> = tracked.iter().filter(|j| j.is_active()).cloned().collect();
    let mut recent: Vec<AsyncRunView> =
        tracked.iter().filter(|j| !j.is_active()).cloned().collect();
    let by_update =
        |left: &AsyncRunView, right: &AsyncRunView| right.updated_at().cmp(&left.updated_at());
    active.sort_by(by_update);
    recent.sort_by(by_update);
    recent.truncate(options.limit);
    let mut tracked_runs = active;
    tracked_runs.extend(recent);

    let tracked_ids: HashSet<&str> = tracked_runs
        .iter()
        .map(|run| run.status.run_id.as_str())
        .collect();
    let mut runs = tracked_runs.clone();
    runs.extend(
        state
            .history_jobs
            .iter()
            .filter(|run| !tracked_ids.contains(run.status.run_id.as_str()))
            .cloned(),
    );
    for run in order_fleet_async_runs(&runs, options.limit) {
        items.extend(async_items(&run));
    }

    // (3) Settled foreground runs whose control entry is gone (pi `:211-228`).
    let mut recent_foreground: Vec<&ForegroundResumeRunView> = state
        .foreground_runs
        .iter()
        .filter(|run| {
            belongs_to_current_session(
                run.session_id.as_deref(),
                state.current_session_id.as_deref(),
            ) && !active_foreground_ids.contains(&run.run_id)
        })
        .collect();
    recent_foreground.sort_by_key(|run| std::cmp::Reverse(run.updated_at));
    for run in recent_foreground {
        for child in &run.children {
            items.push(FleetItem {
                key: format!("foreground-recent:{}:{}", run.run_id, child.index),
                kind: FleetItemKind::ForegroundRecent {
                    run: Box::new(run.clone()),
                    child: Box::new(child.clone()),
                },
                run_id: run.run_id.clone(),
                index: Some(child.index),
                agent: child.agent.clone(),
                state: child.status.clone(),
                updated_at: child.updated_at.unwrap_or(run.updated_at),
                description: None,
            });
        }
    }

    FleetSnapshot { items, error: state.scan_error.clone() }
}

/// pi's `listAsyncRuns(options.asyncDirRoot, { ...(state.currentSessionId ? { sessionId:
/// state.currentSessionId } : {}), entryLimit: MAX_FLEET_HISTORY_CANDIDATES, reconcile: false })`
/// (`fleet.ts:194-199`) — the on-disk history half of the roster.
///
/// Three of upstream's options are load-bearing here:
///
/// * **`sessionId`** — `listAsyncRuns` drops every on-disk run whose recorded `status.sessionId`
///   differs from the caller's (`async-status.ts:432`:
///   `if (options.sessionId && status.sessionId !== options.sessionId) continue;`). Note the exact
///   shape: with a current session, a run carrying NO recorded session is dropped too
///   (`undefined !== "abc"`), which is STRICTER than
///   [`belongs_to_current_session`]'s in-memory rule; with no current session the filter is off
///   entirely and everything on disk belongs. Without it, opening the inspector in one session
///   lists every run every other session in the same project ever launched.
/// * **`reconcile: false`** — the inspector reads what is on disk right now and never mutates it,
///   so opening the inspector can never repair, fail or re-terminalise a run. A directory whose
///   `status.json` is missing or unreadable is skipped, not fatal (pi's `if (!status) continue`).
/// * **`entryLimit`** — upstream sorts the candidate directories by their `status.json` mtime,
///   NEWEST FIRST, and keeps the first `entryLimit` (`async-status.ts:388-405`). It is not "the
///   first 100 the directory happened to list": on a project with more than
///   [`MAX_FLEET_HISTORY_CANDIDATES`] runs, readdir order would silently pin the roster to an
///   arbitrary — very possibly the oldest — subset.
///
/// # Errors
///
/// Returns the directory-listing error only when `async_root` exists but cannot be read — pi's own
/// `catch` at `:207`, which becomes [`FleetSnapshot::error`].
pub async fn collect_fleet_history(
    async_root: &Path,
    results_dir: &Path,
    current_session_id: Option<&str>,
) -> Result<Vec<AsyncRunView>, String> {
    let mut entries = match tokio::fs::read_dir(async_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    // pi's `entryLimit` pass (`async-status.ts:388-405`): collect every candidate with its
    // `status.json` mtime FIRST, sort newest-first, and only then take the limit. A directory whose
    // status file is missing is dropped here rather than counted against the budget, exactly as
    // upstream's `isNotFoundError ⇒ undefined` filter does.
    let mut candidates: Vec<(i128, RunPaths)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(file_type) = entry.file_type().await else { continue };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        let run_id = crate::background::RunId::from_token(name);
        let paths = RunPaths::for_run(async_root, results_dir, &run_id);
        let Ok(meta) = tokio::fs::metadata(&paths.status).await else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i128::try_from(d.as_millis()).unwrap_or(i128::MAX));
        candidates.push((mtime, paths));
    }
    candidates.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    candidates.truncate(MAX_FLEET_HISTORY_CANDIDATES);

    let mut runs: Vec<AsyncRunView> = Vec::new();
    for (_, paths) in candidates {
        let Ok(bytes) = tokio::fs::read(&paths.status).await else { continue };
        let Ok(status) = serde_json::from_slice::<RunStatus>(&bytes) else { continue };
        // pi `async-status.ts:432` — `if (options.sessionId && status.sessionId !==
        // options.sessionId) continue;`. The comparison is against the sessionId recorded ON THE
        // RUN ([`crate::background::RunStatus::session_id`], stamped by the detached runner from
        // the parent-session anchor), and an absent one loses to any present filter.
        if let Some(current) = current_session_id
            && status.session_id.as_deref() != Some(current)
        {
            continue;
        }
        let session_id = status.session_id.clone();
        runs.push(AsyncRunView {
            paths,
            status,
            session_id,
            description: None,
            context: None,
            nested_children: Vec::new(),
        });
    }
    Ok(runs)
}

// =================================================================================================
// Per-item projections (pi `fleet.ts:232-423`)
// =================================================================================================

/// pi `statusGlyph(item, theme)` (`fleet.ts:232-238`). Note the default arm is the ERROR cross,
/// unlike the fleet-status widget's warning square ([`super::fleet_status::nested_status_glyph`]).
#[must_use]
pub fn status_glyph(state: &str) -> Span<'static> {
    match state {
        "running" => th::fg(Role::Accent, "●"),
        "queued" | "pending" => th::fg(Role::Muted, "◦"),
        "complete" | "completed" => th::fg(Role::Success, "✓"),
        "paused" | "stopped" | "detached" => th::fg(Role::Warning, "■"),
        _ => th::fg(Role::Error, "✗"),
    }
}

/// pi `isActionableAsyncState(state)` (`fleet.ts:320-322`).
#[must_use]
pub fn is_actionable_async_state(state: &str) -> bool {
    matches!(state, "running" | "queued" | "pending")
}

/// pi `itemMode(item)` (`fleet.ts:385-387`).
#[must_use]
pub fn item_mode(item: &FleetItem) -> &'static str {
    match &item.kind {
        FleetItemKind::ForegroundActive { control, .. } => {
            crate::background::run_status::run_mode_label(control.mode)
        }
        FleetItemKind::ForegroundRecent { run, .. } => {
            crate::background::run_status::run_mode_label(run.mode)
        }
        FleetItemKind::Async { run, .. } => {
            crate::background::run_status::run_mode_label(run.status.mode)
        }
    }
}

/// pi `itemSource(item)` (`fleet.ts:389-392`).
#[must_use]
pub fn item_source(item: &FleetItem) -> &'static str {
    match &item.kind {
        FleetItemKind::Async { .. } => "background",
        FleetItemKind::ForegroundActive { .. } => "foreground · live",
        FleetItemKind::ForegroundRecent { .. } => "foreground · recent",
    }
}

/// pi `contextModeLabel(context)` (`runs/shared/context-mode.ts`) — the parenthesised fork/fresh
/// badge appended to an identity line.
#[must_use]
pub fn context_mode_label(context: Option<ContextMode>) -> Option<&'static str> {
    match context {
        Some(ContextMode::Fork) => Some("(fork)"),
        Some(ContextMode::Fresh) => Some("(fresh)"),
        None => None,
    }
}

/// pi `contextModeBadge(theme, context)` (`runs/shared/context-mode.ts`) — the roster's compact
/// coloured badge.
#[must_use]
pub fn context_mode_badge(context: Option<ContextMode>) -> Option<Span<'static>> {
    match context {
        Some(ContextMode::Fork) => Some(th::fg(Role::Warning, " ⑂")),
        Some(ContextMode::Fresh) => Some(th::fg(Role::Dim, " ○")),
        None => None,
    }
}

/// pi `itemContext(item)` (`fleet.ts:379-383`).
#[must_use]
pub fn item_context(item: &FleetItem) -> Option<ContextMode> {
    match &item.kind {
        // Delta 3: cyrup has no per-STEP context, so a step row shows the run's.
        FleetItemKind::Async { run, .. } => run.context,
        FleetItemKind::ForegroundRecent { child, .. } => child.context,
        FleetItemKind::ForegroundActive { .. } => None,
    }
}

/// pi `formatTokens` (`shared/formatters.ts`) — same private duplication rationale
/// `background/fleet_view.rs` gives for its own copy.
fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{}k", (n as f64 / 1000.0).round() as u64)
    }
}

/// pi `formatModelThinking` (`shared/formatters.ts:19-29`).
fn format_model_thinking(model: Option<&str>, thinking: Option<&str>) -> Option<String> {
    const THINKING_LEVELS: [&str; 4] = ["off", "low", "medium", "high"];
    let display_model = model.map(|m| match m.rfind('/') {
        Some(i) => m.get(i.saturating_add(1)..).unwrap_or(m),
        None => m,
    });
    let display_thinking = thinking
        .map(str::trim)
        .filter(|t| THINKING_LEVELS.contains(t));
    let joined = [
        display_model.map(str::to_string),
        display_thinking.map(|t| format!("thinking {t}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    if joined.is_empty() { None } else { Some(joined) }
}

/// pi `itemStats(item)` (`fleet.ts:394-423`) — the `model · 12.4k tok · 9 tools · 1m 20s` row.
#[must_use]
pub fn item_stats(item: &FleetItem, now: i64) -> Vec<String> {
    let model: Option<String>;
    let tokens: Option<u64>;
    let tools: Option<u64>;
    let duration_ms: Option<i64>;
    match &item.kind {
        FleetItemKind::ForegroundActive { control, active_child_index } => {
            let child = active_child_index
                .and_then(|i| control.active_children.iter().find(|c| c.index == i));
            let (m, t, tc, started) = match child {
                Some(child) => (
                    child.model.as_deref(),
                    child.tokens,
                    child.tool_count,
                    child.started_at,
                ),
                None => (
                    control.model.as_deref(),
                    control.tokens,
                    control.tool_count,
                    control.started_at,
                ),
            };
            let thinking = child.map_or(control.thinking.as_deref(), |c| c.thinking.as_deref());
            model = format_model_thinking(m, thinking);
            tokens = t;
            tools = tc;
            duration_ms = Some(now.saturating_sub(started).max(0));
        }
        FleetItemKind::ForegroundRecent { child, .. } => {
            model = format_model_thinking(child.model.as_deref(), child.thinking.as_deref());
            tokens = child.tokens;
            tools = child.tool_count;
            // pi leaves `durationMs` undefined on this branch (`fleet.ts:405-409`) — a settled
            // foreground child records no start/end pair the inspector can read.
            duration_ms = None;
        }
        FleetItemKind::Async { run, step_index } => {
            let step = step_index.and_then(|i| run.status.steps.get(i));
            model = format_model_thinking(
                step.and_then(|s| s.model.as_ref()).map(cyrup_core::ModelId::as_str),
                step.and_then(|s| s.telemetry.thinking.as_deref()),
            );
            tokens = step
                .and_then(|s| s.telemetry.tokens.as_ref().map(|t| t.total))
                .or_else(|| {
                    if item.index.is_none() {
                        run.status.telemetry.total_tokens.as_ref().map(|t| t.total)
                    } else {
                        None
                    }
                });
            tools = step
                .and_then(|s| s.telemetry.tool_count)
                .or_else(|| {
                    if item.index.is_none() { run.status.telemetry.tool_count } else { None }
                });
            let terminal_run = !is_actionable_async_state(&item.state);
            let end_time = run
                .status
                .ended_at
                .or(if terminal_run { Some(run.status.last_update) } else { None })
                .unwrap_or(now);
            duration_ms = step
                .and_then(|s| match (s.started_at, s.ended_at) {
                    (Some(start), Some(end)) => Some((end - start).max(0)),
                    _ => None,
                })
                .or_else(|| Some(end_time.saturating_sub(run.status.started_at).max(0)));
        }
    }
    let mut out: Vec<String> = Vec::new();
    if let Some(model) = model {
        out.push(model);
    }
    if let Some(tokens) = tokens {
        out.push(format!("{} tok", format_tokens(tokens)));
    }
    if let Some(tools) = tools {
        out.push(format!("{tools} tool{}", if tools == 1 { "" } else { "s" }));
    }
    if let Some(duration) = duration_ms {
        out.push(crate::background::wait::format_duration(
            u64::try_from(duration).unwrap_or(0),
        ));
    }
    out
}

// =================================================================================================
// Detail builders (pi `fleet.ts:240-318`)
// =================================================================================================

/// pi `foregroundActiveDetail(item)` (`fleet.ts:240-261`).
fn foreground_active_detail(
    item: &FleetItem,
    control: &ForegroundControlView,
    active_child_index: Option<usize>,
) -> Vec<String> {
    let child =
        active_child_index.and_then(|i| control.active_children.iter().find(|c| c.index == i));
    let (model, thinking, started_at, current_tool, current_path, turn_count, tool_count, tokens) =
        match child {
            Some(child) => (
                child.model.as_deref(),
                child.thinking.as_deref(),
                child.started_at,
                child.current_tool.as_deref(),
                child.current_path.as_deref(),
                child.turn_count,
                child.tool_count,
                child.tokens,
            ),
            None => (
                control.model.as_deref(),
                control.thinking.as_deref(),
                control.started_at,
                control.current_tool.as_deref(),
                control.current_path.as_deref(),
                control.turn_count,
                control.tool_count,
                control.tokens,
            ),
        };
    let mut lines = vec![
        format!("Run: {}", item.run_id),
        "Source: foreground".to_string(),
        "State: running".to_string(),
        format!("Mode: {}", crate::background::run_status::run_mode_label(control.mode)),
        match item.index {
            Some(index) => format!("Child: {index} ({})", item.agent),
            None => format!("Agent: {}", item.agent),
        },
    ];
    if let Some(model) = format_model_thinking(model, thinking) {
        lines.push(format!("Model: {model}"));
    }
    lines.push(format!("Started: {}", format_iso8601(started_at)));
    if let Some(tool) = current_tool {
        let path = current_path
            .map(|p| format!(" · {}", crate::exec::tool_call_summary::shorten_path(p)))
            .unwrap_or_default();
        lines.push(format!("Current tool: {tool}{path}"));
    }
    if let Some(turns) = turn_count {
        lines.push(format!("Turns: {turns}"));
    }
    if let Some(tools) = tool_count {
        lines.push(format!("Tools: {tools}"));
    }
    if let Some(tokens) = tokens {
        lines.push(format!("Tokens: {}", format_tokens(tokens)));
    }
    lines.push(String::new());
    lines.push("Transcript".to_string());
    lines.push(
        "Live foreground output remains in the expanded subagent tool result. Persisted output \
         and session paths appear here after the child settles."
            .to_string(),
    );
    lines
}

/// pi `foregroundRecentDetail(item)` (`fleet.ts:263-287`).
fn foreground_recent_detail(
    item: &FleetItem,
    run: &ForegroundResumeRunView,
    child: &ForegroundResumeChildView,
) -> Vec<String> {
    let output_path = child
        .artifact_output_path
        .as_ref()
        .or(child.saved_output_path.as_ref());
    let context = context_mode_label(child.context)
        .map(|c| format!(" {c}"))
        .unwrap_or_default();
    let mut lines = vec![
        format!("Run: {}", item.run_id),
        "Source: foreground".to_string(),
        format!("State: {}", child.status),
        format!("Mode: {}", crate::background::run_status::run_mode_label(run.mode)),
        format!("Child: {} ({}){context}", child.index, child.agent),
    ];
    if let Some(model) = format_model_thinking(child.model.as_deref(), child.thinking.as_deref()) {
        lines.push(format!("Model: {model}"));
    }
    lines.push(format!(
        "Updated: {}",
        format_iso8601(child.updated_at.unwrap_or(run.updated_at))
    ));
    if let Some(path) = output_path {
        lines.push(format!("Output: {}", path.display()));
    }
    if let Some(session) = child.session_file.as_ref() {
        lines.push(format!("Session: {}", session.display()));
    }
    if let Some(transcript) = child.transcript_path.as_ref() {
        lines.push(format!("Transcript file: {}", transcript.display()));
    }
    if let Some(error) = child.error.as_ref() {
        lines.push(format!("Error: {error}"));
    }
    if let Some(warning) = child.output_save_error.as_ref() {
        lines.push(format!("Output warning: {warning}"));
    }
    if let Some(warning) = child.transcript_error.as_ref() {
        lines.push(format!("Transcript warning: {warning}"));
    }
    lines.push(String::new());
    lines.push("Result transcript tail".to_string());
    let final_output = child.final_output.clone().unwrap_or_default();
    let output_lines: Vec<String> = final_output
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let tail = output_lines
        .get(output_lines.len().saturating_sub(TRANSCRIPT_LINES)..)
        .unwrap_or(&output_lines);
    if tail.is_empty() {
        lines.push("(no recovered output available)".to_string());
    } else {
        lines.extend(tail.iter().cloned());
    }
    lines
}

/// pi `asyncDetail(item)` (`fleet.ts:289-307`): the full transcript body when the run's status is
/// still readable, else pi's small "(status is no longer available)" block.
fn async_detail(item: &FleetItem, run: &AsyncRunView, step_index: Option<usize>) -> Vec<String> {
    // pi calls `readStatus(item.run.asyncDir)` here; cyrup's [`AsyncRunView`] already carries the
    // reconciled status it would return, so the read is not repeated. A run whose directory has
    // gone away is the `None` case below, driven by the caller dropping it from the snapshot.
    if (!run.status.steps.is_empty() || run.status.state != RunState::Queued)
        && let Ok(text) = crate::background::fleet_view::format_async_run_transcript(
            &run.status,
            &run.paths,
            step_index,
            Some(TRANSCRIPT_LINES as i64),
            &[],
        )
    {
        return text.split('\n').map(str::to_string).collect();
    }
    let run_context = context_mode_label(run.context)
        .map(|c| format!(" {c}"))
        .unwrap_or_default();
    let mut lines = vec![
        format!("Run: {}", item.run_id),
        "Source: async".to_string(),
        format!("State: {}", item.state),
        format!(
            "Mode: {}{run_context}",
            crate::background::run_status::run_mode_label(run.status.mode)
        ),
        match item.index {
            Some(index) => format!("Child: {index} ({}){run_context}", item.agent),
            None => format!("Agent: {}{run_context}", item.agent),
        },
    ];
    if let Some(index) = item.index {
        lines.push(format!("Output: {}", run.paths.step_output_log(index).display()));
    }
    let session = step_index
        .and_then(|i| run.status.steps.get(i))
        .and_then(|s| s.session_file.as_ref())
        .or(run.status.session_file.as_ref());
    if let Some(session) = session {
        lines.push(format!("Session: {}", session.display()));
    }
    lines.push(String::new());
    lines.push("Transcript".to_string());
    lines.push("(status is no longer available)".to_string());
    lines
}

/// pi `detailLines(item, error)` (`fleet.ts:309-318`).
#[must_use]
pub fn detail_lines(item: Option<&FleetItem>, error: Option<&str>) -> Vec<String> {
    let Some(item) = item else {
        return vec![
            error.map_or_else(
                || {
                    "No current-session foreground or recent async children.".to_string()
                },
                |e| format!("Fleet scan failed: {e}"),
            ),
            String::new(),
            "New runs appear here automatically while this inspector remains open.".to_string(),
        ];
    };
    let mut lines = match &item.kind {
        FleetItemKind::ForegroundActive { control, active_child_index } => {
            foreground_active_detail(item, control, *active_child_index)
        }
        FleetItemKind::ForegroundRecent { run, child } => {
            foreground_recent_detail(item, run, child)
        }
        FleetItemKind::Async { run, step_index } => async_detail(item, run, *step_index),
    };
    if let Some(error) = error {
        lines.insert(0, String::new());
        lines.insert(0, format!("Fleet scan warning: {error}"));
    }
    lines
}

/// `new Date(ms).toISOString()` (`fleet.ts:251,274`) — RFC 3339 with millisecond precision, in
/// UTC, which is what JS emits.
fn format_iso8601(epoch_millis: i64) -> String {
    let secs = epoch_millis.div_euclid(1000);
    let millis = epoch_millis.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days` — days since the Unix epoch to `(year, month, day)`.
/// Self-contained rather than pulling in a date crate for two format calls.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

// =================================================================================================
// Transcript targeting (pi `fleet.ts:330-377`)
// =================================================================================================

/// pi's `{ path, trustedRoots }` result (`fleet.ts:342`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptTarget {
    /// The file to read.
    pub path: PathBuf,
    /// The roots it must resolve inside — never derived from `path` itself.
    pub trusted_roots: Vec<PathBuf>,
}

/// pi `uniquePaths(values)` (`fleet.ts:330-332`).
fn unique_paths(values: Vec<Option<PathBuf>>) -> Vec<PathBuf> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for value in values.into_iter().flatten() {
        let resolved = std::path::absolute(&value).unwrap_or(value);
        if seen.insert(resolved.clone()) {
            out.push(resolved);
        }
    }
    out
}

/// pi `fleetArtifactsRoot(state, cwd)` (`fleet.ts:334-340`).
#[must_use]
pub fn fleet_artifacts_root(state: &FleetState, cwd: &Path) -> PathBuf {
    // SUBA-048 / pi `getArtifactsDir(state.parentSessionFile ?? null, cwd,
    // state.artifactDirPreference ?? "project")` (`fleet.ts:334-340`). The in-tree note that used
    // to sit here — "cyrup's resolver takes the project cwd directly, and `Some(cwd)` IS the
    // `project` preference" — recorded the defect: with no preference parameter there was no way
    // to express `session` or `temp` at all.
    crate::artifacts::resolve_artifacts_dir(
        state.parent_session_file.as_deref(),
        Some(cwd),
        cwd,
        state.artifact_dir_preference,
    )
}

/// pi `transcriptTarget(item, state)` (`fleet.ts:342-377`).
#[must_use]
pub fn transcript_target(item: &FleetItem, state: &FleetState) -> Option<TranscriptTarget> {
    match &item.kind {
        FleetItemKind::ForegroundActive { control, .. } => {
            let cwd = control.cwd.clone().unwrap_or_else(|| state.base_cwd.clone());
            let artifacts_root = fleet_artifacts_root(state, &cwd);
            // Delta 3 of `fleet_transcript`: pi's fifth `ArtifactPaths` field `transcriptPath` has
            // no cyrup analogue, and the `.jsonl` event stream is the artifact cyrup writes in its
            // place (`artifacts.rs:58`).
            let paths = crate::artifacts::artifact_paths(
                &artifacts_root,
                &item.run_id,
                &item.agent,
                Some(item.index.unwrap_or(0)),
            );
            Some(TranscriptTarget {
                path: paths.jsonl_path,
                trusted_roots: vec![artifacts_root],
            })
        }
        FleetItemKind::ForegroundRecent { run, child } => {
            let transcript_path = child.transcript_path.as_ref()?;
            let resolved = if transcript_path.is_absolute() {
                transcript_path.clone()
            } else {
                run.cwd.join(transcript_path)
            };
            Some(TranscriptTarget {
                path: resolved,
                trusted_roots: unique_paths(vec![
                    Some(fleet_artifacts_root(state, &run.cwd)),
                    Some(fleet_artifacts_root(state, &state.base_cwd)),
                ]),
            })
        }
        FleetItemKind::Async { run, step_index } => {
            // pi: the named step, else the sole step of a one-step run (`fleet.ts:363`).
            let index = step_index.or(if run.status.steps.len() == 1 { Some(0) } else { None })?;
            let step = run.status.steps.get(index)?;
            // cyrup's step records its output log rather than a separate transcript file.
            let transcript_path = step.telemetry.output_file.clone()?;
            let resolved = if transcript_path.is_absolute() {
                transcript_path
            } else {
                run.dir().join(transcript_path)
            };
            Some(TranscriptTarget {
                path: resolved,
                trusted_roots: unique_paths(vec![
                    Some(run.dir().to_path_buf()),
                    Some(fleet_artifacts_root(state, &state.base_cwd)),
                    run.status
                        .cwd
                        .as_ref()
                        .map(|cwd| fleet_artifacts_root(state, cwd)),
                ]),
            })
        }
    }
}

// =================================================================================================
// Header (pi `fleet.ts:425-440`)
// =================================================================================================

/// pi `structuredHeader(item, width, theme, conversationState)` (`fleet.ts:425-440`).
#[must_use]
pub fn structured_header(
    item: &FleetItem,
    width: usize,
    conversation_state: &str,
    now: i64,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(th::right_aligned(
        &Line::from(vec![
            th::raw(" "),
            status_glyph(&item.state),
            th::raw(" "),
            th::bold(item.agent.clone()),
        ]),
        &Line::from(vec![th::fg(Role::Dim, item.state.clone())]),
        width,
    ));
    let child = item
        .index
        .map(|index| format!(" · child {}", index.saturating_add(1)))
        .unwrap_or_default();
    let context = context_mode_label(item_context(item))
        .map(|c| format!(" {c}"))
        .unwrap_or_default();
    let run_id_head: String = item.run_id.chars().take(8).collect();
    let identity = format!(
        "{} · {run_id_head}{child} · {}{context}",
        item_source(item),
        item_mode(item)
    );
    lines.push(Line::from(vec![th::raw("  "), th::fg(Role::Dim, identity)]));
    let stats = item_stats(item, now);
    if !stats.is_empty() {
        lines.push(Line::from(vec![
            th::raw("  "),
            th::fg(Role::Muted, stats.join(" · ")),
        ]));
    }
    if let Some(description) = item.description.as_ref() {
        let task = collapse_whitespace(description);
        lines.push(Line::from(vec![
            th::raw("  "),
            th::fg(Role::Dim, "Task"),
            th::raw("  "),
            th::raw(task),
        ]));
    }
    lines.push(Line::from(vec![
        th::fg(Role::Accent, "Conversation"),
        th::raw(" "),
        th::fg(Role::Dim, format!("· {conversation_state}")),
    ]));
    lines.into_iter().map(|line| th::clip(&line, width)).collect()
}

// =================================================================================================
// Key input (pi's raw `data: string` in `handleInput`)
// =================================================================================================

/// The keys `fleet.ts:606-713` matches on. `Shift+K`/`Shift+J` arrive as `Char('K')`/`Char('J')`,
/// which is exactly how `matchesKey(data, Key.shift("k"))` distinguishes them from `"k"` on a real
/// terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetKey {
    /// pi `matchesKey(data, "up")`.
    Up,
    /// pi `matchesKey(data, "down")`.
    Down,
    /// pi `matchesKey(data, "home")`.
    Home,
    /// pi `matchesKey(data, "end")`.
    End,
    /// pi `matchesKey(data, "pageUp")`.
    PageUp,
    /// pi `matchesKey(data, "pageDown")`.
    PageDown,
    /// pi `matchesKey(data, "return")` / `\r` / `\n`.
    Enter,
    /// pi `matchesKey(data, "escape")`.
    Escape,
    /// pi `matchesKey(data, "tab")` / `\t`.
    Tab,
    /// pi `matchesKey(data, "backspace")` / `\x7f`.
    Backspace,
    /// pi `matchesKey(data, "ctrl+c")` — treated exactly like Escape everywhere it appears.
    CtrlC,
    /// pi `matchesKey(data, "ctrl+o")`.
    CtrlO,
    /// A printable character.
    Char(char),
}

/// What [`SubagentFleetComponent::handle_input`] tells its owner happened — pi's component calls
/// `this.done(undefined)` and `this.tui.requestRender()` directly; cyrup returns the intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetInputOutcome {
    /// Nothing changed; no repaint needed (pi's silent `return`).
    Ignored,
    /// State changed; the owner should repaint (pi `this.tui.requestRender()`).
    Rerender,
    /// pi `this.done(undefined)` — close the inspector.
    Close,
    /// pi's `runAction(...)` — the owner must perform this action and feed the result back through
    /// [`SubagentFleetComponent::finish_action`].
    RunAction(Box<FleetPendingAction>),
}

/// One action the component wants performed (pi's `this.options.actions!.…` calls,
/// `fleet.ts:624,651,693`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetPendingAction {
    /// pi `actions.steer({ runId, asyncDir, index?, message, mode })`.
    Steer {
        /// The target run/child.
        target: FleetActionTarget,
        /// The typed message.
        message: String,
        /// The `Tab`-selected delivery mode.
        mode: SteerDeliveryMode,
    },
    /// pi `actions.stop({ runId, asyncDir, index? })`.
    Stop {
        /// The target run/child.
        target: FleetActionTarget,
    },
    /// pi `actions.inspect!({ runId, asyncDir, index? })`.
    Inspect {
        /// The target run/child.
        target: FleetActionTarget,
    },
}

// =================================================================================================
// The component (pi `fleet.ts:476-841`)
// =================================================================================================

/// pi's `FleetTranscriptCache` (`fleet.ts:458-465`).
#[derive(Clone, Debug)]
struct TranscriptCache {
    path: PathBuf,
    fingerprint: String,
    width: usize,
    expanded_tools: bool,
    transcript: FleetTranscript,
    body: Vec<Line<'static>>,
}

/// pi `transcriptFingerprint(filePath)` (`fleet.ts:467-474`) — `size:mtimeMs`, or `"missing"`.
fn transcript_fingerprint(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis());
            format!("{}:{mtime}", meta.len())
        }
        Err(_) => "missing".to_string(),
    }
}

/// pi's `SubagentFleetComponent` (`fleet.ts:476-841`) — see the module doc for the one transport
/// difference (owner-driven calls in place of `Component` + `setInterval` + `ctx.ui.custom`).
#[derive(Debug)]
pub struct SubagentFleetComponent {
    snapshot: FleetSnapshot,
    selected: usize,
    selected_key: Option<String>,
    detail_scroll: usize,
    detail_auto_follow: bool,
    detail_line_count: usize,
    detail_viewport_height: usize,
    body_height: usize,
    expanded_tools: bool,
    action_notice: Option<FleetActionResult>,
    steer_draft: Option<String>,
    steer_mode: SteerDeliveryMode,
    stop_confirming: bool,
    action_busy: bool,
    transcript_cache: Option<TranscriptCache>,
    state: FleetState,
    options: FleetViewOptions,
    /// pi's `this.tui.terminal?.rows ?? 32` (`fleet.ts:791`).
    terminal_rows: usize,
    /// Whether the owner supplied an action bundle at all — pi's `!this.options.actions` guard.
    has_actions: bool,
    /// Whether that bundle can service `H` — pi's `!this.options.actions?.inspect` guard.
    has_inspect: bool,
}

impl SubagentFleetComponent {
    /// pi's constructor (`fleet.ts:501-522`), minus the `setInterval` arming (the owner polls at
    /// [`FleetViewOptions::refresh_ms`] instead).
    #[must_use]
    pub fn new(
        state: FleetState,
        options: FleetViewOptions,
        initial_key: Option<String>,
        has_actions: bool,
        has_inspect: bool,
    ) -> Self {
        let mut component = Self {
            snapshot: FleetSnapshot::default(),
            selected: 0,
            selected_key: initial_key,
            detail_scroll: 0,
            detail_auto_follow: true,
            detail_line_count: 0,
            detail_viewport_height: 8,
            body_height: 8,
            expanded_tools: false,
            action_notice: None,
            steer_draft: None,
            steer_mode: SteerDeliveryMode::Steer,
            stop_confirming: false,
            action_busy: false,
            transcript_cache: None,
            state,
            options,
            terminal_rows: 32,
            has_actions,
            has_inspect,
        };
        component.refresh();
        component
    }

    /// pi `this.tui.terminal?.rows` (`fleet.ts:791`) — the owner reports its height.
    pub fn set_terminal_rows(&mut self, rows: usize) {
        self.terminal_rows = rows;
    }

    /// Replace the state the snapshot is folded from — what pi gets for free by holding a live
    /// reference to `state`. The owner calls this before [`Self::invalidate`] on each poll tick.
    pub fn set_state(&mut self, state: FleetState) {
        self.state = state;
    }

    /// pi `refresh()` (`fleet.ts:524-530`): re-collect, then re-find the previously selected key so
    /// the cursor does not jump when rows appear or disappear above it.
    pub fn refresh(&mut self) {
        let previous_key = self
            .snapshot
            .items
            .get(self.selected)
            .map(|item| item.key.clone())
            .or_else(|| self.selected_key.clone());
        self.snapshot = collect_fleet_snapshot(&self.state, &self.options);
        let preserved = previous_key
            .as_ref()
            .and_then(|key| self.snapshot.items.iter().position(|item| item.key == *key));
        self.selected = match preserved {
            Some(index) => index,
            None => self
                .selected
                .min(self.snapshot.items.len().saturating_sub(1)),
        };
        self.selected_key = self
            .snapshot
            .items
            .get(self.selected)
            .map(|item| item.key.clone());
    }

    /// pi `invalidate()` (`fleet.ts:832-835`) — drop the transcript cache, then refresh.
    pub fn invalidate(&mut self) {
        self.transcript_cache = None;
        self.refresh();
    }

    /// The snapshot currently rendered.
    #[must_use]
    pub fn snapshot(&self) -> &FleetSnapshot {
        &self.snapshot
    }

    /// The selected item, if any.
    #[must_use]
    pub fn selected_item(&self) -> Option<&FleetItem> {
        self.snapshot.items.get(self.selected)
    }

    /// pi `expandedTools` (`fleet.ts:485`).
    #[must_use]
    pub fn expanded_tools(&self) -> bool {
        self.expanded_tools
    }

    /// pi `steerDraft` (`fleet.ts:487`) — `Some` while the steer prompt is open.
    #[must_use]
    pub fn steer_draft(&self) -> Option<&str> {
        self.steer_draft.as_deref()
    }

    /// pi `steerMode` (`fleet.ts:488`).
    #[must_use]
    pub fn steer_mode(&self) -> SteerDeliveryMode {
        self.steer_mode
    }

    /// pi `stopConfirming` (`fleet.ts:489`).
    #[must_use]
    pub fn stop_confirming(&self) -> bool {
        self.stop_confirming
    }

    /// pi `actionNotice` (`fleet.ts:486`).
    #[must_use]
    pub fn action_notice(&self) -> Option<&FleetActionResult> {
        self.action_notice.as_ref()
    }

    /// pi's `.then(result => this.setActionNotice(result))` / `.catch(...)` / `.finally(...)`
    /// chain (`fleet.ts:590-596`) collapsed into one call the owner makes when the action it was
    /// handed by [`FleetInputOutcome::RunAction`] settles.
    pub fn finish_action(&mut self, result: FleetActionResult) {
        self.action_busy = false;
        self.set_action_notice(result);
    }

    /// pi `setActionNotice(result)` (`fleet.ts:576-583`).
    fn set_action_notice(&mut self, result: FleetActionResult) {
        self.action_notice = Some(result);
        self.reset_action_input();
        self.detail_auto_follow = false;
        self.detail_scroll = 0;
        self.refresh();
    }

    /// pi `resetActionInput()` (`fleet.ts:541-545`).
    fn reset_action_input(&mut self) {
        self.steer_draft = None;
        self.steer_mode = SteerDeliveryMode::Steer;
        self.stop_confirming = false;
    }

    /// pi `moveSelection(delta)` (`fleet.ts:532-539`).
    fn move_selection(&mut self, delta: isize) -> FleetInputOutcome {
        if self.snapshot.items.is_empty() {
            return FleetInputOutcome::Ignored;
        }
        let max = self.snapshot.items.len().saturating_sub(1) as isize;
        let next = (self.selected as isize).saturating_add(delta).clamp(0, max);
        self.selected = usize::try_from(next).unwrap_or(0);
        self.selected_key = self
            .snapshot
            .items
            .get(self.selected)
            .map(|item| item.key.clone());
        self.detail_auto_follow = true;
        self.reset_action_input();
        FleetInputOutcome::Rerender
    }

    /// pi `scrollDetail(delta)` (`fleet.ts:599-604`).
    fn scroll_detail(&mut self, delta: isize) -> FleetInputOutcome {
        let max_scroll = self
            .detail_line_count
            .saturating_sub(self.detail_viewport_height);
        let next = (self.detail_scroll as isize)
            .saturating_add(delta)
            .clamp(0, max_scroll as isize);
        self.detail_scroll = usize::try_from(next).unwrap_or(0);
        self.detail_auto_follow = self.detail_scroll >= max_scroll;
        FleetInputOutcome::Rerender
    }

    /// pi `selectedAsyncAction()` (`fleet.ts:547-553`): fleet controls apply only to an actionable
    /// current-session background child.
    fn selected_async_action(&self) -> Result<FleetActionTarget, String> {
        let Some(item) = self.snapshot.items.get(self.selected) else {
            return Err("No child is selected.".to_string());
        };
        let FleetItemKind::Async { run, .. } = &item.kind else {
            return Err(
                "Fleet controls are available for current-session top-level async runs only."
                    .to_string(),
            );
        };
        if !is_actionable_async_state(run.state_label())
            || !is_actionable_async_state(&item.state)
        {
            return Err(format!(
                "Selected child is {}; controls require a running or queued async child.",
                item.state
            ));
        }
        Ok(FleetActionTarget {
            run_id: item.run_id.clone(),
            async_dir: run.dir().to_path_buf(),
            index: item.index,
        })
    }

    /// pi's `if ("reason" in target || !this.options.actions)` guard, shared by `s`/`D`/`H` and by
    /// both confirm paths (`fleet.ts:620,647,680,692,698`).
    fn resolve_action_target(&self, unavailable: &str) -> Result<FleetActionTarget, String> {
        let target = self.selected_async_action()?;
        if !self.has_actions {
            return Err(unavailable.to_string());
        }
        Ok(target)
    }

    /// pi `handleInput(data)` (`fleet.ts:606-713`), branch for branch and in pi's own order.
    pub fn handle_input(&mut self, key: FleetKey) -> FleetInputOutcome {
        // --- steer draft (pi `:607-643`) ---
        if let Some(draft) = self.steer_draft.clone() {
            match key {
                FleetKey::Escape | FleetKey::CtrlC => {
                    self.reset_action_input();
                    return FleetInputOutcome::Rerender;
                }
                FleetKey::Enter => {
                    let message = draft.trim().to_string();
                    if message.is_empty() {
                        self.set_action_notice(FleetActionResult::error(
                            "Steer message cannot be empty.",
                        ));
                        return FleetInputOutcome::Rerender;
                    }
                    match self
                        .resolve_action_target("Fleet controls are unavailable in this context.")
                    {
                        Err(reason) => {
                            self.set_action_notice(FleetActionResult::error(reason));
                            return FleetInputOutcome::Rerender;
                        }
                        Ok(target) => {
                            if self.action_busy {
                                return FleetInputOutcome::Ignored;
                            }
                            self.action_busy = true;
                            self.action_notice = None;
                            return FleetInputOutcome::RunAction(Box::new(
                                FleetPendingAction::Steer {
                                    target,
                                    message,
                                    mode: self.steer_mode,
                                },
                            ));
                        }
                    }
                }
                FleetKey::Tab => {
                    self.steer_mode = self.steer_mode.next();
                    return FleetInputOutcome::Rerender;
                }
                FleetKey::Backspace => {
                    let mut next = draft;
                    next.pop();
                    self.steer_draft = Some(next);
                    return FleetInputOutcome::Rerender;
                }
                FleetKey::Char(c) if c >= ' ' && c != '\u{7f}' => {
                    let mut next = draft;
                    next.push(c);
                    self.steer_draft = Some(next);
                    return FleetInputOutcome::Rerender;
                }
                _ => return FleetInputOutcome::Ignored,
            }
        }

        // --- stop confirmation (pi `:644-659`) ---
        if self.stop_confirming {
            match key {
                FleetKey::Enter | FleetKey::Char('y') | FleetKey::Char('Y') => {
                    match self
                        .resolve_action_target("Fleet controls are unavailable in this context.")
                    {
                        Err(reason) => {
                            self.set_action_notice(FleetActionResult::error(reason));
                            return FleetInputOutcome::Rerender;
                        }
                        Ok(target) => {
                            if self.action_busy {
                                return FleetInputOutcome::Ignored;
                            }
                            self.action_busy = true;
                            self.action_notice = None;
                            return FleetInputOutcome::RunAction(Box::new(
                                FleetPendingAction::Stop { target },
                            ));
                        }
                    }
                }
                FleetKey::Escape
                | FleetKey::CtrlC
                | FleetKey::Char('n')
                | FleetKey::Char('N')
                | FleetKey::Backspace => {
                    self.reset_action_input();
                    return FleetInputOutcome::Rerender;
                }
                _ => return FleetInputOutcome::Ignored,
            }
        }

        // --- normal mode (pi `:660-712`) ---
        match key {
            FleetKey::Escape | FleetKey::CtrlC | FleetKey::Char('q') => FleetInputOutcome::Close,
            FleetKey::Char('K') => self.scroll_detail(-1),
            FleetKey::Char('J') => self.scroll_detail(1),
            FleetKey::Up | FleetKey::Char('k') => self.move_selection(-1),
            FleetKey::Down | FleetKey::Char('j') => self.move_selection(1),
            FleetKey::Home => {
                let len = self.snapshot.items.len() as isize;
                self.move_selection(-len)
            }
            FleetKey::End => {
                let len = self.snapshot.items.len() as isize;
                self.move_selection(len)
            }
            FleetKey::PageUp => {
                let height = self.detail_viewport_height as isize;
                self.scroll_detail(-height)
            }
            FleetKey::PageDown => {
                let height = self.detail_viewport_height as isize;
                self.scroll_detail(height)
            }
            FleetKey::Char('r') | FleetKey::Char('R') => {
                self.transcript_cache = None;
                self.refresh();
                FleetInputOutcome::Rerender
            }
            FleetKey::Char('s') => {
                match self.resolve_action_target("Fleet controls are unavailable in this context.")
                {
                    Err(reason) => self.set_action_notice(FleetActionResult::error(reason)),
                    Ok(_) => {
                        self.action_notice = None;
                        self.steer_draft = Some(String::new());
                        self.detail_auto_follow = false;
                        self.detail_scroll = 0;
                    }
                }
                FleetInputOutcome::Rerender
            }
            FleetKey::Char('H') => {
                let resolved = self.selected_async_action().and_then(|target| {
                    if self.has_actions && self.has_inspect {
                        Ok(target)
                    } else {
                        Err(
                            "Herdr inspector controls are unavailable in this context."
                                .to_string(),
                        )
                    }
                });
                match resolved {
                    Err(reason) => {
                        self.set_action_notice(FleetActionResult::error(reason));
                        FleetInputOutcome::Rerender
                    }
                    Ok(target) => {
                        if self.action_busy {
                            return FleetInputOutcome::Ignored;
                        }
                        self.action_busy = true;
                        self.action_notice = None;
                        FleetInputOutcome::RunAction(Box::new(FleetPendingAction::Inspect {
                            target,
                        }))
                    }
                }
            }
            FleetKey::Char('D') => {
                match self.resolve_action_target("Fleet controls are unavailable in this context.")
                {
                    Err(reason) => self.set_action_notice(FleetActionResult::error(reason)),
                    Ok(_) => {
                        self.action_notice = None;
                        self.stop_confirming = true;
                        self.detail_auto_follow = false;
                        self.detail_scroll = 0;
                    }
                }
                FleetInputOutcome::Rerender
            }
            FleetKey::Char('x') | FleetKey::Char('X') | FleetKey::CtrlO => {
                self.expanded_tools = !self.expanded_tools;
                self.transcript_cache = None;
                FleetInputOutcome::Rerender
            }
            _ => FleetInputOutcome::Ignored,
        }
    }

    /// pi `actionLines()` (`fleet.ts:555-569`).
    fn action_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if self.action_busy {
            lines.push(Line::from(vec![th::fg(Role::Accent, "Action pending...")]));
        }
        if let Some(draft) = self.steer_draft.as_ref() {
            lines.push(Line::from(vec![
                th::fg(
                    Role::Accent,
                    format!("Steer message ({}): {draft}", self.steer_mode.as_str()),
                ),
                th::fg(Role::Dim, "▌"),
            ]));
            lines.push(Line::from(vec![th::fg(
                Role::Dim,
                "Enter sends · Tab changes mode · Esc cancels · Backspace edits",
            )]));
        } else if self.stop_confirming {
            let run_id = self
                .snapshot
                .items
                .get(self.selected)
                .map_or_else(|| "selected run".to_string(), |item| item.run_id.clone());
            lines.push(Line::from(vec![th::fg(
                Role::Warning,
                format!("Confirm stop for async run {run_id}?"),
            )]));
            lines.push(Line::from(vec![th::fg(
                Role::Dim,
                "Stop ends the run; use interrupt for a resumable pause. Enter/Y confirms · N \
                 returns · Esc cancels",
            )]));
        } else if let Some(notice) = self.action_notice.as_ref() {
            let role = if notice.is_error { Role::Error } else { Role::Success };
            lines.push(Line::from(vec![th::fg(role, notice.text.clone())]));
        }
        lines
    }

    /// pi `withActionLines(body)` (`fleet.ts:571-574`).
    fn with_action_lines(&self, body: Vec<Line<'static>>) -> Vec<Line<'static>> {
        let action_lines = self.action_lines();
        if action_lines.is_empty() {
            return body;
        }
        let mut out = action_lines;
        out.push(Line::from(Vec::<Span<'static>>::new()));
        out.extend(body);
        out
    }

    /// pi `rosterLines(width)` (`fleet.ts:715-727`).
    fn roster_lines(&self, width: usize) -> Vec<Line<'static>> {
        if self.snapshot.items.is_empty() {
            return vec![Line::from(vec![th::fg(Role::Dim, "No tracked children")])];
        }
        let start = self
            .selected
            .saturating_add(1)
            .saturating_sub(self.body_height)
            .min(self.snapshot.items.len().saturating_sub(self.body_height));
        self.snapshot
            .items
            .iter()
            .enumerate()
            .skip(start)
            .take(self.body_height)
            .map(|(index, item)| {
                let marker = if index == self.selected {
                    th::fg(Role::Accent, "›")
                } else {
                    th::raw(" ")
                };
                let agent = if index == self.selected {
                    th::bold(item.agent.clone())
                } else {
                    th::raw(item.agent.clone())
                };
                let identity = item
                    .description
                    .as_deref()
                    .map(collapse_whitespace)
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| item.run_id.chars().take(8).collect());
                let mut left = vec![
                    marker,
                    th::raw(" "),
                    status_glyph(&item.state),
                    th::raw(" "),
                    agent,
                ];
                if let Some(badge) = context_mode_badge(item_context(item)) {
                    left.push(badge);
                }
                left.push(th::raw(" "));
                left.push(th::fg(Role::Dim, format!("· {identity}")));
                th::right_aligned(
                    &Line::from(left),
                    &Line::from(vec![th::fg(Role::Dim, item.state.clone())]),
                    width,
                )
            })
            .collect()
    }

    /// pi `renderedTranscript(target, width)` (`fleet.ts:729-744`) — the cache lookup and its
    /// four-part key.
    fn rendered_transcript(
        &mut self,
        target: &TranscriptTarget,
        width: usize,
    ) -> (FleetTranscript, Vec<Line<'static>>) {
        let roots: Vec<String> = target
            .trusted_roots
            .iter()
            .map(|r| r.display().to_string())
            .collect();
        let fingerprint = format!(
            "{}|{}",
            roots.join("\0"),
            transcript_fingerprint(&target.path)
        );
        if let Some(cache) = self.transcript_cache.as_ref()
            && cache.path == target.path
            && cache.fingerprint == fingerprint
            && cache.width == width
            && cache.expanded_tools == self.expanded_tools
        {
            return (cache.transcript.clone(), cache.body.clone());
        }
        let transcript = read_fleet_transcript(
            &target.path,
            &FleetTranscriptReadOptions {
                trusted_roots: target.trusted_roots.clone(),
                ..FleetTranscriptReadOptions::default()
            },
        );
        let body = if transcript.events.is_empty() {
            Vec::new()
        } else {
            render_fleet_transcript(&transcript, width, self.expanded_tools)
        };
        self.transcript_cache = Some(TranscriptCache {
            path: target.path.clone(),
            fingerprint,
            width,
            expanded_tools: self.expanded_tools,
            transcript: transcript.clone(),
            body: body.clone(),
        });
        (transcript, body)
    }

    /// pi `wrappedDetail(width)` (`fleet.ts:746-786`) — the transcript-backed form when the
    /// selected child has readable events, else the structured fallback with pi's per-prefix
    /// styling.
    fn wrapped_detail(&mut self, width: usize, now: i64) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
        let selected = self.snapshot.items.get(self.selected).cloned();
        let mut transcript_warning: Option<String> = None;
        if let Some(item) = selected.as_ref()
            && let Some(target) = transcript_target(item, &self.state)
        {
            let (transcript, mut body) = self.rendered_transcript(&target, width);
            transcript_warning = transcript.warning.clone();
            if !transcript.events.is_empty() {
                if let Some(error) = self.snapshot.error.as_ref() {
                    body.insert(0, Line::from(Vec::<Span<'static>>::new()));
                    body.insert(
                        0,
                        Line::from(vec![th::fg(
                            Role::Warning,
                            format!("Fleet scan warning: {error}"),
                        )]),
                    );
                }
                let conversation_state = match transcript.events.last() {
                    Some(FleetTranscriptEvent::Assistant { .. }) => "assistant response".to_string(),
                    Some(FleetTranscriptEvent::User { .. }) => "supervisor message".to_string(),
                    Some(FleetTranscriptEvent::Tool(tool)) => format!(
                        "{} · {}",
                        tool.name,
                        match tool.status {
                            super::fleet_transcript::ToolStatus::Running => "running",
                            super::fleet_transcript::ToolStatus::Complete => "complete",
                            super::fleet_transcript::ToolStatus::Error => "error",
                        }
                    ),
                    _ => "activity".to_string(),
                };
                let header = structured_header(item, width, &conversation_state, now);
                return (header, self.with_action_lines(body));
            }
        }

        let mut raw = detail_lines(selected.as_ref(), self.snapshot.error.as_deref());
        if let Some(warning) = transcript_warning {
            raw.insert(0, String::new());
            raw.insert(0, format!("Transcript preview warning: {warning}"));
        }
        let mut lines: Vec<Line<'static>> = Vec::new();
        for line in raw {
            let styled = style_detail_line(&line);
            let wrapped = th::wrap_line(&styled, width.max(1));
            if wrapped.is_empty() {
                lines.push(Line::from(Vec::<Span<'static>>::new()));
            } else {
                lines.extend(wrapped);
            }
        }
        (Vec::new(), self.with_action_lines(lines))
    }

    /// pi `render(width)` (`fleet.ts:788-830`) — the whole framed two-pane view.
    pub fn render(&mut self, width: usize, now: i64) -> Vec<Line<'static>> {
        if width < MIN_WIDTH {
            return vec![th::clip(
                &Line::from(vec![th::raw(
                    "Subagent fleet needs at least 36 columns. Esc closes.",
                )]),
                width,
            )];
        }
        let inner_width = width.saturating_sub(2);
        self.body_height = ((self.terminal_rows as f64 * 0.85) as usize)
            .saturating_sub(6)
            .max(2);
        let roster_width = (((inner_width.saturating_sub(1)) as f64 * 0.38) as usize).clamp(22, 46);
        let detail_width = inner_width
            .saturating_sub(roster_width)
            .saturating_sub(1)
            .max(1);
        let roster = self.roster_lines(roster_width);
        let (detail_header_all, detail_body) = self.wrapped_detail(detail_width, now);
        let header_budget = self.body_height.saturating_sub(1);
        let detail_header: Vec<Line<'static>> =
            detail_header_all.into_iter().take(header_budget).collect();
        self.detail_viewport_height = self.body_height.saturating_sub(detail_header.len()).max(1);
        self.detail_line_count = detail_body.len();
        let max_detail_scroll = detail_body
            .len()
            .saturating_sub(self.detail_viewport_height);
        // pi `:801-802` — auto-follow pins the view to the tail; otherwise the existing offset
        // is only clamped back into range. Both arms land on the same value.
        if self.detail_auto_follow || self.detail_scroll > max_detail_scroll {
            self.detail_scroll = max_detail_scroll;
        }
        let mut visible_details = detail_header;
        visible_details.extend(
            detail_body
                .into_iter()
                .skip(self.detail_scroll)
                .take(self.detail_viewport_height),
        );

        let mut lines: Vec<Line<'static>> = vec![Line::from(vec![th::fg(
            Role::Border,
            format!("╭{}╮", "─".repeat(inner_width)),
        )])];
        let selected = self.snapshot.items.get(self.selected);
        let title = Line::from(vec![
            th::raw(" "),
            th::bold("Subagent fleet inspector"),
            th::raw(" "),
            th::fg(Role::Dim, "· live controls"),
        ]);
        let selected_status = match selected {
            Some(item) => Line::from(vec![
                status_glyph(&item.state),
                th::raw(format!(" {} · {} ", item.agent, item.state)),
            ]),
            None => Line::from(vec![th::fg(Role::Dim, "no children ")]),
        };
        let mut header_row = vec![th::fg(Role::Border, "│")];
        header_row.extend(th::right_aligned(&title, &selected_status, inner_width).spans);
        header_row.push(th::fg(Role::Border, "│"));
        lines.push(Line::from(header_row));
        lines.push(Line::from(vec![th::fg(
            Role::Border,
            format!(
                "├{}┬{}┤",
                "─".repeat(roster_width),
                "─".repeat(detail_width)
            ),
        )]));
        for index in 0..self.body_height {
            let mut row = vec![th::fg(Role::Border, "│")];
            row.extend(
                th::fit(
                    roster
                        .get(index)
                        .unwrap_or(&Line::from(Vec::<Span<'static>>::new())),
                    roster_width,
                )
                .spans,
            );
            row.push(th::fg(Role::Border, "│"));
            row.extend(
                th::fit(
                    visible_details
                        .get(index)
                        .unwrap_or(&Line::from(Vec::<Span<'static>>::new())),
                    detail_width,
                )
                .spans,
            );
            row.push(th::fg(Role::Border, "│"));
            lines.push(Line::from(row));
        }
        lines.push(Line::from(vec![th::fg(
            Role::Border,
            format!(
                "├{}┴{}┤",
                "─".repeat(roster_width),
                "─".repeat(detail_width)
            ),
        )]));
        let position = if self.snapshot.items.is_empty() {
            "0/0".to_string()
        } else {
            format!(
                "{}/{}",
                self.selected.saturating_add(1),
                self.snapshot.items.len()
            )
        };
        let footer = format!(
            " ↑↓/jk agent · H Herdr · s steer · D stop · x/Ctrl+O tools · r refresh · Esc close · \
             {position}"
        );
        let mut footer_row = vec![th::fg(Role::Border, "│")];
        footer_row
            .extend(th::fit(&Line::from(vec![th::fg(Role::Dim, footer)]), inner_width).spans);
        footer_row.push(th::fg(Role::Border, "│"));
        lines.push(Line::from(footer_row));
        lines.push(Line::from(vec![th::fg(
            Role::Border,
            format!("╰{}╯", "─".repeat(inner_width)),
        )]));
        lines
            .into_iter()
            .map(|line| th::clip(&line, width))
            .collect()
    }
}

/// pi's per-prefix styling of the structured detail block (`fleet.ts:773-781`).
fn style_detail_line(line: &str) -> Line<'static> {
    const BOLD_PREFIXES: [&str; 6] = ["Run:", "State:", "Mode:", "Source:", "Child:", "Agent:"];
    const BOLD_PREFIX_MODEL: &str = "Model:";
    const ACCENT_PREFIXES: [&str; 2] = ["Transcript", "Result transcript tail"];
    const MUTED_PREFIXES: [&str; 4] =
        ["Output:", "Session:", "Transcript file:", "Artifacts:"];
    // The BRANCH ORDER is load-bearing and is upstream's, not a tidier one. `fleet.ts:773-781` is a
    // single nested ternary evaluated top-down: bold, then `/^(Transcript|Result transcript tail)/`,
    // then the muted set, then `/^Transcript preview warning:/`. Because the accent test is a bare
    // `^Transcript` prefix and runs SECOND, it captures two lines that a naive reading assigns
    // elsewhere:
    //
    //   * `"Transcript preview warning: …"` starts with `Transcript`, so it paints ACCENT and
    //     upstream's trailing warning branch is unreachable dead code. It is kept below in its
    //     upstream position so the shape stays diffable against `:779-780`.
    //   * `"Transcript file: …"` likewise paints ACCENT, never muted, despite `Transcript file`
    //     appearing in the muted alternation at `:778`.
    //
    // Ordering the warning and muted tests ahead of the accent test — which reads more natural —
    // repaints both of those lines (yellow and gray respectively) against upstream's cyan.
    if BOLD_PREFIXES.iter().any(|p| line.starts_with(p)) || line.starts_with(BOLD_PREFIX_MODEL) {
        return Line::from(vec![th::bold(line.to_string())]);
    }
    if ACCENT_PREFIXES.iter().any(|p| line.starts_with(p)) {
        return Line::from(vec![th::fg(Role::Accent, line.to_string())]);
    }
    if MUTED_PREFIXES.iter().any(|p| line.starts_with(p)) {
        return Line::from(vec![th::fg(Role::Muted, line.to_string())]);
    }
    if line.starts_with("Transcript preview warning:") {
        return Line::from(vec![th::fg(Role::Warning, line.to_string())]);
    }
    Line::from(vec![th::raw(line.to_string())])
}

// =================================================================================================
// openSubagentFleet (pi `fleet.ts:843-879` + `showFleet`, slash-commands.ts:633-649)
// =================================================================================================

/// What [`open_subagent_fleet`] decided to do — pi `showFleet`'s three outcomes
/// (`slash/slash-commands.ts:633-649`).
#[derive(Debug)]
pub enum FleetOpenOutcome {
    /// pi's `if (!ctx.hasUI) { await runSlashSubagent(pi, ctx, { action: "status", view: "fleet" });
    /// return; }` (`:635-638`) — the caller must render the v0.34.0 text fleet view instead.
    NoUiFallback,
    /// pi's `if (fleetOpen) { ctx.ui.notify("Subagent fleet inspector is already open.", "info");
    /// return; }` (`:639-642`).
    AlreadyOpen,
    /// pi's `openSubagentFleet(ctx, state, …)` (`fleet.ts:843-879`): the component, plus the
    /// widget key the caller must clear first (`fleet.ts:846`).
    Opened {
        /// The constructed inspector.
        component: Box<SubagentFleetComponent>,
        /// pi `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, undefined)` — the status widget MUST be
        /// cleared before the overlay opens so the two surfaces never render at once.
        clear_widget_key: &'static str,
    },
}

/// pi `showFleet(ctx)` (`slash/slash-commands.ts:633-649`) composed with `openSubagentFleet`
/// (`fleet.ts:843-879`).
///
/// `fleet_open` is upstream's closure-scoped `let fleetOpen = false` (`:632`) — the caller owns it
/// because it must survive across invocations, exactly as pi's does. `state.fleet_inspector_open`
/// is pi's separate `state.fleetInspectorOpen` latch (`fleet.ts:844-845`), which the status widget
/// reads to unregister itself; the caller sets and restores it around the overlay, matching pi's
/// `try/finally`.
#[must_use]
pub fn open_subagent_fleet(
    has_ui: bool,
    fleet_open: bool,
    state: FleetState,
    options: FleetViewOptions,
    initial_key: Option<String>,
    has_actions: bool,
    has_inspect: bool,
) -> FleetOpenOutcome {
    if !has_ui {
        return FleetOpenOutcome::NoUiFallback;
    }
    if fleet_open {
        return FleetOpenOutcome::AlreadyOpen;
    }
    FleetOpenOutcome::Opened {
        component: Box::new(SubagentFleetComponent::new(
            state,
            options,
            initial_key,
            has_actions,
            has_inspect,
        )),
        clear_widget_key: FLEET_STATUS_WIDGET_KEY,
    }
}

/// pi's `firstToolResultText(result, fallback)` (`fleet.ts:324-328`) — project one control-op
/// result onto a [`FleetActionResult`]. cyrup's control ops return `Result<String, String>`, whose
/// `Err` IS pi's `isError: true` (the convention `background/fleet_view.rs` already documents).
#[must_use]
pub fn action_result_from_control(result: Result<String, String>, fallback: &str) -> FleetActionResult {
    match result {
        Ok(text) if text.trim().is_empty() => FleetActionResult::error(fallback),
        Ok(text) => FleetActionResult::ok(text),
        Err(text) if text.trim().is_empty() => FleetActionResult::error(fallback),
        Err(text) => FleetActionResult::error(text),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::background::{RunId, RunMode, StepState, StepStatus};
    use crate::tui::fleet_state::ForegroundChildView;
    use ratatui::style::{Modifier, Style};

    fn run_paths(id: &str) -> RunPaths {
        RunPaths::for_run(
            Path::new("/tmp/async"),
            Path::new("/tmp/results"),
            &RunId::from_token(id.to_string()),
        )
    }

    fn async_run(id: &str, state: RunState, steps: Vec<StepStatus>, last_update: i64) -> AsyncRunView {
        let mut status = RunStatus::queued(RunId::from_token(id.to_string()), RunMode::Chain, None);
        status.state = state;
        status.steps = steps;
        status.started_at = 1_000;
        status.last_update = last_update;
        AsyncRunView {
            paths: run_paths(id),
            status,
            session_id: None,
            description: None,
            context: None,
            nested_children: Vec::new(),
        }
    }

    fn step(agent: &str, state: StepState) -> StepStatus {
        let mut s = StepStatus::pending(agent.to_string());
        s.status = state;
        s
    }

    fn control(run_id: &str, agent: &str, updated_at: i64) -> ForegroundControlView {
        ForegroundControlView {
            run_id: run_id.to_string(),
            mode: RunMode::Single,
            started_at: 0,
            updated_at,
            current_agent: Some(agent.to_string()),
            ..ForegroundControlView::default()
        }
    }

    // -----------------------------------------------------------------------------------------
    // collectFleetSnapshot (pi :136-230)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn snapshot_orders_foreground_newest_first_then_background_then_settled() {
        let state = FleetState {
            foreground_controls: vec![control("fg-old", "a", 100), control("fg-new", "b", 900)],
            tracked_jobs: vec![async_run("bg", RunState::Running, vec![step("s0", StepState::Running)], 500)],
            foreground_runs: vec![ForegroundResumeRunView {
                run_id: "settled".into(),
                updated_at: 50,
                children: vec![ForegroundResumeChildView {
                    agent: "old".into(),
                    index: 0,
                    status: "complete".into(),
                    ..ForegroundResumeChildView::default()
                }],
                ..ForegroundResumeRunView::default()
            }],
            ..FleetState::default()
        };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(
            snapshot.items.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
            vec![
                "foreground-active:fg-new:0",
                "foreground-active:fg-old:0",
                "async:bg:0",
                "foreground-recent:settled:0",
            ]
        );
    }

    #[test]
    fn an_active_foreground_run_hides_its_own_settled_twin() {
        let state = FleetState {
            foreground_controls: vec![control("dup", "a", 100)],
            foreground_runs: vec![ForegroundResumeRunView {
                run_id: "dup".into(),
                updated_at: 50,
                children: vec![ForegroundResumeChildView {
                    agent: "old".into(),
                    index: 0,
                    status: "complete".into(),
                    ..ForegroundResumeChildView::default()
                }],
                ..ForegroundResumeRunView::default()
            }],
            ..FleetState::default()
        };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].key, "foreground-active:dup:0");
    }

    #[test]
    fn active_children_expand_into_one_row_each_sorted_by_index() {
        let mut c = control("fan", "a", 100);
        c.active_children = vec![
            ForegroundChildView { index: 2, agent: "c".into(), updated_at: 5, ..Default::default() },
            ForegroundChildView { index: 0, agent: "a".into(), updated_at: 5, ..Default::default() },
        ];
        let state = FleetState { foreground_controls: vec![c], ..FleetState::default() };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(
            snapshot.items.iter().map(|i| i.agent.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
    }

    #[test]
    fn session_filter_drops_other_sessions_but_keeps_untagged_when_none_is_current() {
        let mut mine = async_run("mine", RunState::Running, Vec::new(), 100);
        mine.session_id = Some("S1".into());
        let mut theirs = async_run("theirs", RunState::Running, Vec::new(), 200);
        theirs.session_id = Some("S2".into());
        let state = FleetState {
            current_session_id: Some("S1".into()),
            tracked_jobs: vec![mine.clone(), theirs.clone()],
            ..FleetState::default()
        };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].run_id, "mine");

        let no_session = FleetState {
            current_session_id: None,
            tracked_jobs: vec![mine, theirs],
            ..FleetState::default()
        };
        assert_eq!(
            collect_fleet_snapshot(&no_session, &FleetViewOptions::default()).items.len(),
            2
        );
    }

    #[test]
    fn history_runs_already_tracked_in_memory_are_not_duplicated() {
        let state = FleetState {
            tracked_jobs: vec![async_run("r", RunState::Running, Vec::new(), 100)],
            history_jobs: vec![
                async_run("r", RunState::Running, Vec::new(), 90),
                async_run("other", RunState::Complete, Vec::new(), 80),
            ],
            ..FleetState::default()
        };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(
            snapshot.items.iter().map(|i| i.run_id.as_str()).collect::<Vec<_>>(),
            vec!["r", "other"]
        );
    }

    #[test]
    fn terminal_background_runs_are_capped_by_the_limit() {
        let history: Vec<AsyncRunView> = (0..5)
            .map(|i| async_run(&format!("t{i}"), RunState::Complete, Vec::new(), i))
            .collect();
        let state = FleetState { history_jobs: history, ..FleetState::default() };
        let snapshot =
            collect_fleet_snapshot(&state, &FleetViewOptions { limit: 2, ..Default::default() });
        // Newest first, capped at two.
        assert_eq!(
            snapshot.items.iter().map(|i| i.run_id.as_str()).collect::<Vec<_>>(),
            vec!["t4", "t3"]
        );
    }

    #[test]
    fn a_multi_step_background_run_yields_one_row_per_step() {
        let run = async_run(
            "chain",
            RunState::Running,
            vec![step("a", StepState::Complete), step("b", StepState::Running)],
            100,
        );
        let state = FleetState { tracked_jobs: vec![run], ..FleetState::default() };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(
            snapshot.items.iter().map(|i| (i.key.as_str(), i.state.as_str())).collect::<Vec<_>>(),
            vec![("async:chain:0", "complete"), ("async:chain:1", "running")]
        );
    }

    // -----------------------------------------------------------------------------------------
    // Projections (pi :232-440)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn status_glyph_default_arm_is_the_error_cross() {
        assert_eq!(th::line_text(&Line::from(status_glyph("running"))), "●");
        assert_eq!(th::line_text(&Line::from(status_glyph("pending"))), "◦");
        assert_eq!(th::line_text(&Line::from(status_glyph("completed"))), "✓");
        assert_eq!(th::line_text(&Line::from(status_glyph("detached"))), "■");
        assert_eq!(th::line_text(&Line::from(status_glyph("failed"))), "✗");
        assert_eq!(th::line_text(&Line::from(status_glyph("anything else"))), "✗");
    }

    #[test]
    fn actionable_states_are_exactly_upstreams_three() {
        assert!(is_actionable_async_state("running"));
        assert!(is_actionable_async_state("queued"));
        assert!(is_actionable_async_state("pending"));
        assert!(!is_actionable_async_state("complete"));
        assert!(!is_actionable_async_state("stopped"));
    }

    #[test]
    fn iso8601_matches_the_javascript_form() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_iso8601(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
    }

    #[test]
    fn empty_snapshot_detail_reads_upstreams_two_sentences() {
        let lines = detail_lines(None, None);
        assert_eq!(lines[0], "No current-session foreground or recent async children.");
        assert_eq!(lines[2], "New runs appear here automatically while this inspector remains open.");
        let failed = detail_lines(None, Some("boom"));
        assert_eq!(failed[0], "Fleet scan failed: boom");
    }

    #[test]
    fn a_failed_history_scan_surfaces_as_the_snapshots_error_not_an_empty_roster() {
        // pi `fleet.ts:207-209` — the background half's `catch`.
        let state = FleetState {
            foreground_controls: vec![control("r", "a", 0)],
            scan_error: Some("permission denied".into()),
            ..FleetState::default()
        };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        assert_eq!(snapshot.items.len(), 1, "the live half must survive a failed scan");
        assert_eq!(snapshot.error.as_deref(), Some("permission denied"));
    }

    #[test]
    fn a_scan_warning_is_prepended_to_a_real_items_detail() {
        let state = FleetState {
            foreground_controls: vec![control("r", "a", 0)],
            ..FleetState::default()
        };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        let lines = detail_lines(snapshot.items.first(), Some("scan blew up"));
        assert_eq!(lines[0], "Fleet scan warning: scan blew up");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "Run: r");
    }

    #[test]
    fn structured_header_carries_source_run_prefix_child_and_mode() {
        let mut c = control("abcdefghijkl", "coder", 0);
        // pi takes the roster row's description from the ACTIVE CHILD when one exists
        // (`fleet.ts:156`), and only from the run itself otherwise (`:170`).
        c.description = Some("run-level, not used here".into());
        c.active_children = vec![ForegroundChildView {
            index: 1,
            agent: "coder".into(),
            description: Some("do   the\nthing".into()),
            ..Default::default()
        }];
        let state = FleetState { foreground_controls: vec![c], ..FleetState::default() };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        let header = structured_header(&snapshot.items[0], 100, "assistant response", 10_000);
        let text = th::lines_text(&header);
        assert!(text.contains("foreground · live · abcdefgh · child 2 · single"), "{text}");
        assert!(text.contains("Task  do the thing"), "{text}");
        assert!(text.contains("Conversation · assistant response"), "{text}");
    }

    // -----------------------------------------------------------------------------------------
    // The component (pi :476-841)
    // -----------------------------------------------------------------------------------------

    fn busy_state() -> FleetState {
        FleetState {
            tracked_jobs: vec![async_run(
                "bg",
                RunState::Running,
                vec![step("a", StepState::Running)],
                100,
            )],
            foreground_controls: vec![control("fg", "coder", 200)],
            ..FleetState::default()
        }
    }

    fn component() -> SubagentFleetComponent {
        SubagentFleetComponent::new(busy_state(), FleetViewOptions::default(), None, true, false)
    }

    // -----------------------------------------------------------------------------------------
    // Painted-cell style assertions — the half `lines_text` cannot see
    // -----------------------------------------------------------------------------------------

    #[test]
    fn the_five_status_glyphs_paint_five_distinct_roles() {
        for (state, glyph, role) in [
            ("running", "●", Role::Accent),
            ("queued", "◦", Role::Muted),
            ("complete", "✓", Role::Success),
            ("paused", "■", Role::Warning),
            ("boom", "✗", Role::Error),
        ] {
            let span = status_glyph(state);
            assert_eq!(span.content.as_ref(), glyph, "{state} glyph");
            let line = Line::from(vec![span]);
            assert!(
                th::paints_as(th::painted_style(std::slice::from_ref(&line), 4, glyph), role),
                "{state} must paint as {role:?}"
            );
        }
    }

    #[test]
    fn the_frame_chrome_paints_its_upstream_roles() {
        let mut c = component();
        let frame = c.render(100, 10_000);
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "╭"), Role::Border),
            "the box corner is border-coloured"
        );
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "· live controls"), Role::Dim),
            "the title suffix is dim"
        );
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "↑↓/jk agent"), Role::Dim),
            "the footer is dim"
        );
        // The title itself is bold and UNCOLOURED — a colour here would be a repaint.
        let title = th::painted_style(&frame, 100, "Subagent fleet inspector");
        assert!(title.add_modifier.contains(Modifier::BOLD), "the title is bold");
        assert_eq!(title.fg, Some(ratatui::style::Color::Reset), "the title takes no colour");
    }

    #[test]
    fn the_selected_roster_row_paints_its_marker_accent_and_its_agent_bold() {
        let mut c = component();
        let frame = c.render(100, 10_000);
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "›"), Role::Accent),
            "the selection marker is accent"
        );
        // `coder` is painted TWICE: once in the frame header's selected-status readout (unstyled,
        // `fleet.ts:811`) and once as the roster row's agent (bold while selected, `fleet.ts:722`).
        // Same characters, different cells — invisible to a flattened-text assertion.
        assert!(
            !th::painted_style_nth(&frame, 100, "coder", 0)
                .add_modifier
                .contains(Modifier::BOLD),
            "the header readout is not bold"
        );
        assert!(
            th::painted_style_nth(&frame, 100, "coder", 1)
                .add_modifier
                .contains(Modifier::BOLD),
            "the SELECTED roster row's agent is bold"
        );

        // Move the selection off it: the very same roster cell must lose its bold. The header now
        // reads the newly selected row, so the roster occurrence is the only `coder` left.
        c.handle_input(FleetKey::Down);
        let frame = c.render(100, 10_000);
        assert_eq!(th::painted_style_nth_of(&frame, 100, "coder", 1), None);
        assert!(
            !th::painted_style_nth(&frame, 100, "coder", 0)
                .add_modifier
                .contains(Modifier::BOLD),
            "an unselected roster row's agent must not be bold"
        );
        // …and the newly selected row's agent gained it.
        assert!(
            th::painted_style_nth(&frame, 100, "a · bg", 0)
                .add_modifier
                .contains(Modifier::BOLD),
            "the newly selected roster row's agent is bold"
        );
    }

    #[test]
    fn the_empty_roster_notice_paints_dim() {
        let mut c = SubagentFleetComponent::new(
            FleetState::default(),
            FleetViewOptions::default(),
            None,
            false,
            false,
        );
        let frame = c.render(100, 10_000);
        assert!(th::paints_as(th::painted_style(&frame, 100, "No tracked children"), Role::Dim));
        assert!(th::paints_as(th::painted_style(&frame, 100, "no children"), Role::Dim));
    }

    #[test]
    fn the_steer_prompt_and_the_stop_confirmation_paint_different_colours() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        let frame = c.render(100, 10_000);
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "Steer message"), Role::Accent),
            "the steer prompt is accent"
        );
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "Enter sends"), Role::Dim),
            "its hint is dim"
        );

        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('D'));
        let frame = c.render(100, 10_000);
        assert!(
            th::paints_as(th::painted_style(&frame, 100, "Confirm stop"), Role::Warning),
            "the stop confirmation is WARNING, not accent — same pane, different colour"
        );
    }

    #[test]
    fn an_action_notice_paints_success_or_error_by_outcome() {
        let mut c = component();
        c.finish_action(FleetActionResult::ok("steered"));
        let frame = c.render(100, 10_000);
        assert!(th::paints_as(th::painted_style(&frame, 100, "steered"), Role::Success));

        let mut c = component();
        c.finish_action(FleetActionResult::error("nope"));
        let frame = c.render(100, 10_000);
        assert!(th::paints_as(th::painted_style(&frame, 100, "nope"), Role::Error));
    }

    #[test]
    fn escape_and_q_close_the_inspector() {
        let mut c = component();
        assert_eq!(c.handle_input(FleetKey::Escape), FleetInputOutcome::Close);
        assert_eq!(c.handle_input(FleetKey::CtrlC), FleetInputOutcome::Close);
        assert_eq!(c.handle_input(FleetKey::Char('q')), FleetInputOutcome::Close);
    }

    #[test]
    fn selection_moves_and_clamps_at_both_ends() {
        let mut c = component();
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("foreground-active:fg:0".into()));
        c.handle_input(FleetKey::Down);
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("async:bg:0".into()));
        c.handle_input(FleetKey::Down);
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("async:bg:0".into()));
        c.handle_input(FleetKey::Home);
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("foreground-active:fg:0".into()));
        c.handle_input(FleetKey::End);
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("async:bg:0".into()));
    }

    #[test]
    fn selection_survives_a_refresh_that_reorders_the_roster() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("async:bg:0".into()));
        let mut next = busy_state();
        next.foreground_controls.push(control("fg2", "other", 900));
        c.set_state(next);
        c.invalidate();
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("async:bg:0".into()));
    }

    #[test]
    fn x_and_ctrl_o_toggle_expanded_tools() {
        let mut c = component();
        assert!(!c.expanded_tools());
        c.handle_input(FleetKey::Char('x'));
        assert!(c.expanded_tools());
        c.handle_input(FleetKey::CtrlO);
        assert!(!c.expanded_tools());
        c.handle_input(FleetKey::Char('X'));
        assert!(c.expanded_tools());
    }

    #[test]
    fn steer_is_refused_on_a_foreground_child_and_offered_on_a_background_one() {
        let mut c = component();
        // Foreground row selected: pi's "async runs only" refusal.
        c.handle_input(FleetKey::Char('s'));
        assert!(c.steer_draft().is_none());
        assert!(
            c.action_notice()
                .unwrap()
                .text
                .contains("current-session top-level async runs only")
        );
        // Move to the background row and try again.
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        assert_eq!(c.steer_draft(), Some(""));
    }

    #[test]
    fn the_steer_prompt_edits_cycles_mode_and_sends() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        for ch in "hi".chars() {
            c.handle_input(FleetKey::Char(ch));
        }
        c.handle_input(FleetKey::Char('X'));
        assert_eq!(c.steer_draft(), Some("hiX"));
        c.handle_input(FleetKey::Backspace);
        assert_eq!(c.steer_draft(), Some("hi"));
        assert_eq!(c.steer_mode(), SteerDeliveryMode::Steer);
        c.handle_input(FleetKey::Tab);
        assert_eq!(c.steer_mode(), SteerDeliveryMode::FollowUp);
        c.handle_input(FleetKey::Tab);
        assert_eq!(c.steer_mode(), SteerDeliveryMode::Auto);
        c.handle_input(FleetKey::Tab);
        assert_eq!(c.steer_mode(), SteerDeliveryMode::Steer);
        let outcome = c.handle_input(FleetKey::Enter);
        match outcome {
            FleetInputOutcome::RunAction(action) => match *action {
                FleetPendingAction::Steer { message, mode, target } => {
                    assert_eq!(message, "hi");
                    assert_eq!(mode, SteerDeliveryMode::Steer);
                    assert_eq!(target.run_id, "bg");
                    assert_eq!(target.index, Some(0));
                }
                other => panic!("unexpected action {other:?}"),
            },
            other => panic!("unexpected outcome {other:?}"),
        }
        c.finish_action(FleetActionResult::ok("steered"));
        assert!(c.steer_draft().is_none());
        assert_eq!(c.action_notice().map(|n| n.text.clone()), Some("steered".into()));
    }

    #[test]
    fn an_empty_steer_message_is_refused_without_dispatching() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        c.handle_input(FleetKey::Char(' '));
        assert_eq!(c.handle_input(FleetKey::Enter), FleetInputOutcome::Rerender);
        assert!(c.action_notice().unwrap().is_error);
        assert_eq!(c.action_notice().unwrap().text, "Steer message cannot be empty.");
    }

    #[test]
    fn escape_cancels_the_steer_prompt() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        c.handle_input(FleetKey::Escape);
        assert!(c.steer_draft().is_none());
    }

    #[test]
    fn stop_requires_a_confirmation_and_n_backs_out() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('D'));
        assert!(c.stop_confirming());
        c.handle_input(FleetKey::Char('n'));
        assert!(!c.stop_confirming());
        c.handle_input(FleetKey::Char('D'));
        let outcome = c.handle_input(FleetKey::Char('y'));
        assert!(matches!(
            outcome,
            FleetInputOutcome::RunAction(ref a) if matches!(**a, FleetPendingAction::Stop { .. })
        ));
    }

    #[test]
    fn herdr_is_unavailable_without_an_inspect_handler() {
        let mut c = component();
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('H'));
        assert_eq!(
            c.action_notice().map(|n| n.text.clone()),
            Some("Herdr inspector controls are unavailable in this context.".into())
        );
    }

    #[test]
    fn herdr_dispatches_when_an_inspect_handler_exists() {
        let mut c = SubagentFleetComponent::new(
            busy_state(),
            FleetViewOptions::default(),
            None,
            true,
            true,
        );
        c.handle_input(FleetKey::Down);
        let outcome = c.handle_input(FleetKey::Char('H'));
        assert!(matches!(
            outcome,
            FleetInputOutcome::RunAction(ref a) if matches!(**a, FleetPendingAction::Inspect { .. })
        ));
    }

    #[test]
    fn controls_are_unavailable_without_an_action_bundle() {
        let mut c = SubagentFleetComponent::new(
            busy_state(),
            FleetViewOptions::default(),
            None,
            false,
            false,
        );
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        assert_eq!(
            c.action_notice().map(|n| n.text.clone()),
            Some("Fleet controls are unavailable in this context.".into())
        );
    }

    #[test]
    fn a_terminal_background_child_refuses_controls() {
        let state = FleetState {
            tracked_jobs: vec![async_run(
                "done",
                RunState::Complete,
                vec![step("a", StepState::Complete)],
                100,
            )],
            ..FleetState::default()
        };
        let mut c =
            SubagentFleetComponent::new(state, FleetViewOptions::default(), None, true, false);
        c.handle_input(FleetKey::Char('s'));
        assert!(
            c.action_notice()
                .unwrap()
                .text
                .contains("controls require a running or queued async child")
        );
    }

    #[test]
    fn initial_key_selects_that_row() {
        let c = SubagentFleetComponent::new(
            busy_state(),
            FleetViewOptions::default(),
            Some("async:bg:0".into()),
            true,
            false,
        );
        assert_eq!(c.selected_item().map(|i| i.key.clone()), Some("async:bg:0".into()));
    }

    // -----------------------------------------------------------------------------------------
    // render (pi :788-830)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn a_narrow_terminal_renders_upstreams_one_line_refusal() {
        let mut c = component();
        // 36 columns is exactly upstream's floor (`fleet.ts:789`); one under it refuses.
        let lines = c.render(35, 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            th::line_text(&lines[0]),
            // `truncateToWidth(msg, 35)` with pi's default "..." ellipsis.
            "Subagent fleet needs at least 36..."
        );
        // Twenty columns clips further, still to exactly the budget.
        let narrow = c.render(20, 0);
        assert_eq!(th::line_text(&narrow[0]), "Subagent fleet ne...");
        // And 36 columns renders the real frame.
        assert!(c.render(36, 0).len() > 1);
    }

    #[test]
    fn the_frame_carries_the_title_roster_and_footer() {
        let mut c = component();
        c.set_terminal_rows(40);
        let lines = c.render(120, 10_000);
        let text = th::lines_text(&lines);
        assert!(text.contains("Subagent fleet inspector"), "{text}");
        assert!(text.contains("· live controls"), "{text}");
        assert!(text.contains("› ● coder"), "{text}");
        assert!(text.contains("↑↓/jk agent · H Herdr · s steer · D stop"), "{text}");
        assert!(text.contains("1/2"), "{text}");
        // Every rendered row is exactly the frame width.
        for line in &lines {
            assert_eq!(th::line_width(line), 120, "{}", th::line_text(line));
        }
    }

    #[test]
    fn an_empty_fleet_renders_the_no_children_frame() {
        let mut c = SubagentFleetComponent::new(
            FleetState::default(),
            FleetViewOptions::default(),
            None,
            true,
            false,
        );
        let text = th::lines_text(&c.render(100, 0));
        assert!(text.contains("no children"), "{text}");
        assert!(text.contains("No tracked children"), "{text}");
        assert!(text.contains("0/0"), "{text}");
    }

    #[test]
    fn the_steer_prompt_renders_above_the_detail_body() {
        let mut c = component();
        c.set_terminal_rows(40);
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('s'));
        c.handle_input(FleetKey::Char('g'));
        let text = th::lines_text(&c.render(120, 0));
        assert!(text.contains("Steer message (steer): g"), "{text}");
        assert!(text.contains("Enter sends · Tab changes mode"), "{text}");
    }

    #[test]
    fn the_stop_confirmation_renders_its_warning() {
        let mut c = component();
        c.set_terminal_rows(40);
        c.handle_input(FleetKey::Down);
        c.handle_input(FleetKey::Char('D'));
        let text = th::lines_text(&c.render(160, 0));
        assert!(text.contains("Confirm stop for async run bg?"), "{text}");
        assert!(
            text.contains(
                "Stop ends the run; use interrupt for a resumable pause. Enter/Y confirms · N returns · Esc cancels"
            ),
            "{text}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // showFleet (slash-commands.ts:633-649)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn show_fleet_falls_back_to_the_text_view_without_a_ui() {
        assert!(matches!(
            open_subagent_fleet(
                false,
                false,
                FleetState::default(),
                FleetViewOptions::default(),
                None,
                true,
                false
            ),
            FleetOpenOutcome::NoUiFallback
        ));
    }

    #[test]
    fn show_fleet_refuses_a_second_concurrent_open() {
        assert!(matches!(
            open_subagent_fleet(
                true,
                true,
                FleetState::default(),
                FleetViewOptions::default(),
                None,
                true,
                false
            ),
            FleetOpenOutcome::AlreadyOpen
        ));
    }

    #[test]
    fn show_fleet_opens_and_names_the_widget_key_to_clear() {
        match open_subagent_fleet(
            true,
            false,
            busy_state(),
            FleetViewOptions::default(),
            None,
            true,
            false,
        ) {
            FleetOpenOutcome::Opened { component, clear_widget_key } => {
                assert_eq!(clear_widget_key, FLEET_STATUS_WIDGET_KEY);
                assert_eq!(component.snapshot().items.len(), 2);
            }
            other => panic!("unexpected outcome {other:?}"),
        }
    }

    #[test]
    fn control_results_project_onto_action_results() {
        assert_eq!(
            action_result_from_control(Ok("done".into()), "fallback"),
            FleetActionResult::ok("done")
        );
        assert_eq!(
            action_result_from_control(Err("nope".into()), "fallback"),
            FleetActionResult::error("nope")
        );
        assert_eq!(
            action_result_from_control(Ok("  ".into()), "fallback"),
            FleetActionResult::error("fallback")
        );
    }

    #[test]
    fn transcript_target_for_a_background_step_trusts_the_run_dir() {
        let mut run = async_run("bg", RunState::Running, vec![step("a", StepState::Running)], 100);
        if let Some(first) = run.status.steps.first_mut() {
            first.telemetry.output_file = Some(PathBuf::from("output-0.log"));
        }
        let state = FleetState { tracked_jobs: vec![run], ..FleetState::default() };
        let snapshot = collect_fleet_snapshot(&state, &FleetViewOptions::default());
        let target = transcript_target(&snapshot.items[0], &state).unwrap();
        assert!(target.path.ends_with("output-0.log"));
        assert!(target.trusted_roots.iter().any(|r| r.ends_with("bg")));
    }

    /// The single style carried by a one-span detail line.
    fn only_style(line: &Line<'static>) -> Style {
        assert_eq!(line.spans.len(), 1, "detail lines carry exactly one span");
        line.spans[0].style
    }

    /// `fleet.ts:773-781` is a top-down ternary whose SECOND test is a bare `^Transcript` prefix.
    /// These are the two lines that test captures away from the branches that name them, and they
    /// are invisible to any assertion that flattens the pane to text — the styles differ, the
    /// characters do not.
    #[test]
    fn the_bare_transcript_prefix_paints_before_the_warning_and_muted_branches() {
        let accent = th::style(Role::Accent);

        // `Transcript preview warning:` reaches the accent test first, so upstream's trailing
        // warning branch never runs. Yellow here would be a repaint.
        assert_eq!(
            only_style(&style_detail_line("Transcript preview warning: truncated")),
            accent,
            "a preview warning paints accent, not warning"
        );
        assert_ne!(
            only_style(&style_detail_line("Transcript preview warning: truncated")),
            th::style(Role::Warning)
        );

        // `Transcript file:` is listed in the muted alternation at `:778` but never reaches it.
        assert_eq!(
            only_style(&style_detail_line("Transcript file: /tmp/run.log")),
            accent,
            "a transcript path paints accent, not muted"
        );
        assert_ne!(
            only_style(&style_detail_line("Transcript file: /tmp/run.log")),
            th::style(Role::Muted)
        );
    }

    /// The branches the reordering above must NOT have disturbed.
    #[test]
    fn the_remaining_detail_prefixes_keep_their_upstream_roles() {
        for bold in ["Run: x", "State: x", "Mode: x", "Source: x", "Child: x", "Agent: x", "Model: x"] {
            assert_eq!(
                only_style(&style_detail_line(bold)),
                Style::default().add_modifier(Modifier::BOLD),
                "{bold} is bold"
            );
        }
        for muted in ["Output: x", "Session: x", "Artifacts: x"] {
            assert_eq!(only_style(&style_detail_line(muted)), th::style(Role::Muted), "{muted}");
        }
        assert_eq!(
            only_style(&style_detail_line("Result transcript tail")),
            th::style(Role::Accent)
        );
        // Anything unmatched stays unstyled.
        assert_eq!(only_style(&style_detail_line("plain text")), Style::default());
    }
}
