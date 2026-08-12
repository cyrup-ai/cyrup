//! The always-on fleet **status widget** — Rust port of pi-subagents `src/tui/fleet-status.ts`
//! (`@v0.43.0`, 564 lines).
//!
//! This is the compact surface that lives under (or above) the editor while subagents are running:
//! collapsed it is one line — `3 active agents · ↓ 12.4k tokens · ↓/← to inspect` — and once the
//! user presses `↓`/`←` on an empty editor it expands into a selectable roster whose `Enter` opens
//! the full inspector ([`super::fleet`]).
//!
//! # The three things this module owns
//!
//! 1. **[`collect_fleet_status_entries`]** (pi `:147-239`) — fold live foreground controls and
//!    active background jobs into one `startedAt`-ordered [`FleetStatusEntry`] list. Note this is
//!    a DIFFERENT projection from the inspector's [`super::fleet::collect_fleet_snapshot`]: it
//!    keeps only ACTIVE work (`isActiveState`, pi `:71-73`), it never reads on-disk history, and
//!    it applies pi's chain-pending filter (`:219`) so a chain shows only the step actually
//!    running.
//! 2. **The collapsed tree** ([`fleet_tree_rows`], pi `:130-138`, over [`nested_fleet_rows`], pi
//!    `:97-128`) — one owner row per entry plus at most four nested leaves and an overflow row,
//!    which is what pi's `MAX_AGENT_ROWS = 6` comment (`:9-10`) describes: "one owner, four direct
//!    leaves, and overflow".
//! 3. **[`SubagentFleetStatus`]** (pi `:241-564`) — the widget's state machine: activation on
//!    `↓`/`←` with an empty editor, roster movement, `Enter` to open the inspector, `Esc`/any
//!    other key to deactivate, the `inspectorOpen`/`fleetInspectorOpen` mutual exclusion, and the
//!    [`SubagentFleetStatus::render_key`] change-detector that suppresses redundant repaints.
//!
//! # Transport differences (stated once, with their reasons)
//!
//! * **Registration/refresh.** pi holds an `ExtensionContext`, registers a widget factory through
//!   `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, factory, { placement })`, subscribes to raw
//!   terminal input via `ctx.ui.onTerminalInput`, and drives both from a 500 ms `setInterval`
//!   (`:271-350`). cyrup's extension host exposes `set_widget(&Value)` as a **fire-and-forget
//!   payload**, not a factory + input subscription
//!   (`cyrup-ext/src/host/services.rs:241`, `:283`), and this crate must not depend on
//!   `cyrup-tui` (arch-SA §1.1/§6.1). So this port keeps pi's ENTIRE state machine and rendering,
//!   and exposes it as three plain calls the owner drives: [`SubagentFleetStatus::refresh`],
//!   [`SubagentFleetStatus::handle_key`] and [`SubagentFleetStatus::widget_payload`]. What pi's
//!   interval does, the owner's poll does; what pi's `onTerminalInput` does, the owner's key
//!   forwarding does.
//! * **Stale-context detection.** pi's `isStaleExtensionContextError` (`:140-145`) sniffs two
//!   error message substrings because "Pi currently exposes stale contexts as plain Errors without
//!   a stable code or subtype" (its own comment). cyrup has no throwing context — `has_ui` is a
//!   plain `bool` on [`cyrup_ext::HostCtx`] — so the whole class of error that function exists to
//!   swallow cannot occur, and [`SubagentFleetStatus::set_ui_available`] is the total replacement
//!   for `getActiveUiContext`/`clearUiRegistration`'s error handling.
//! * **Editor focus.** pi's `editorHasFocus` (`:480-491`) duck-types the focused component because
//!   "pi-tui exposes focus mutation but no focus getter" and "instanceof is unreliable across jiti
//!   module boundaries". cyrup has neither a `focusedComponent` handle here nor jiti, so focus is
//!   an explicit input: the owner passes it to [`SubagentFleetStatus::handle_key`], which applies
//!   pi's identical `if (this.active) this.deactivate(); return undefined;` branch when it is
//!   false.
//!
//! # Honest delta vs. pi
//!
//! **No `workflow` mode.** pi's `collectFleetStatusEntries` has a dedicated `job.mode ===
//! "workflow"` arm (`:184-196`) rendering `latest emit: <formatWorkflowJsonPreview(...)>`.
//! cyrup's [`RunMode`] is `Single`/`Parallel`/`Chain` — scripted workflows
//! (`src/workflows/scripted-workflow.ts`) are not ported, so there is no state for that arm to
//! read and it is absent rather than faked. Every other arm is verbatim.

use ratatui::text::{Line, Span};

use super::fleet_state::{
    AsyncRunView, FleetState, NestedRunView, NestedStepView,
};
use super::fleet_theme::{self as th, Role};
use crate::background::{ActivityState, RunMode, StepState};

// =================================================================================================
// Constants + placement (pi `fleet-status.ts:7-49`)
// =================================================================================================

/// pi `FLEET_STATUS_WIDGET_KEY` (`fleet-status.ts:7`) — the key the widget registers under, and
/// the key `openSubagentFleet` clears before opening the inspector (`fleet.ts:846`).
pub const FLEET_STATUS_WIDGET_KEY: &str = "subagent-fleet-status";

/// pi `MAX_AGENT_ROWS` (`fleet-status.ts:10`), with upstream's own comment: "Six rows fit the
/// accepted collapsed hierarchy: one owner, four direct leaves, and overflow."
pub const MAX_AGENT_ROWS: usize = 6;

/// pi `REFRESH_MS` (`fleet-status.ts:11`) — the widget's own poll cadence, distinct from the
/// inspector's 750 ms ([`super::fleet::REFRESH_MS`]).
pub const REFRESH_MS: u64 = 500;

/// pi's `MAX_NESTED_VISIBLE`, inlined upstream as the literal `4` at `fleet-status.ts:125`.
const MAX_NESTED_VISIBLE: usize = 4;

/// pi `FleetViewPlacement` (`shared/types.ts`) — where the widget sits relative to the editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FleetViewPlacement {
    /// pi `"belowEditor"` — the default (`fleet-status.ts:48,268`).
    #[default]
    BelowEditor,
    /// pi `"aboveEditor"`.
    AboveEditor,
}

impl FleetViewPlacement {
    /// The wire string pi passes as `{ placement }` to `setWidget` (`fleet-status.ts:342`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BelowEditor => "belowEditor",
            Self::AboveEditor => "aboveEditor",
        }
    }
}

/// pi `resolveFleetViewPlacement(value)` (`fleet-status.ts:47-49`): ONLY the exact string
/// `"aboveEditor"` selects above; every other value — including a misspelling, a non-string, or
/// nothing at all — falls back to below.
#[must_use]
pub fn resolve_fleet_view_placement(value: Option<&str>) -> FleetViewPlacement {
    if value == Some("aboveEditor") {
        FleetViewPlacement::AboveEditor
    } else {
        FleetViewPlacement::BelowEditor
    }
}

/// pi `formatFleetElapsed(ms)` (`fleet-status.ts:51-53`) — whole seconds, never negative.
#[must_use]
pub fn format_fleet_elapsed(ms: i64) -> String {
    let secs = (ms as f64 / 1000.0).round().max(0.0);
    format!("{secs:.0}s")
}

/// JS `Number.prototype.toFixed(1)` — rounds half AWAY FROM ZERO, where Rust's `{:.1}` rounds half
/// to even. `(1.25).toFixed(1)` is `"1.3"` in JS and `format!("{:.1}", 1.25)` is `"1.2"` in Rust,
/// so a token count landing exactly on a half-tenth would render one digit apart from upstream.
/// Duplicated (rather than cross-imported) per this crate's established convention for tiny
/// formatters — see `background/fleet_view.rs`'s own note on its private `format_tokens` copy.
fn to_fixed_1(value: f64) -> String {
    format!("{:.1}", (value * 10.0).round() / 10.0)
}

/// pi `formatFleetTokens(count)` (`fleet-status.ts:55-61`) — `↓ 1.2M tokens` / `↓ 3.4k tokens` /
/// `↓ 42 tokens`.
#[must_use]
pub fn format_fleet_tokens(count: u64) -> String {
    let compact = if count >= 1_000_000 {
        format!("{}M", to_fixed_1(count as f64 / 1_000_000.0))
    } else if count >= 1_000 {
        format!("{}k", to_fixed_1(count as f64 / 1_000.0))
    } else {
        count.to_string()
    };
    format!("↓ {compact} tokens")
}

/// pi `isActiveState(value)` (`fleet-status.ts:71-73`).
#[must_use]
pub fn is_active_state(value: &str) -> bool {
    matches!(value, "running" | "queued" | "pending")
}

// =================================================================================================
// Entries + the collapsed tree (pi `fleet-status.ts:17-138`)
// =================================================================================================

/// pi `FleetStatusEntry` (`fleet-status.ts:17-26`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetStatusEntry {
    /// pi `key` — the roster identity, and the value handed to `openInspector`
    /// (`fleet-status.ts:399`) which the inspector uses as its `initialKey`.
    pub key: String,
    /// pi `agent`.
    pub agent: String,
    /// pi `modelThinking`.
    pub model_thinking: Option<String>,
    /// pi `description`.
    pub description: Option<String>,
    /// pi `startedAt` — the list's primary sort key (`fleet-status.ts:238`).
    pub started_at: i64,
    /// pi `tokens`.
    pub tokens: u64,
    /// pi `state`.
    pub state: String,
    /// pi `nestedChildren`.
    pub nested_children: Vec<NestedRunView>,
}

/// pi `FleetNestedRow` (`fleet-status.ts:28-35`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetNestedRow {
    /// pi `name`.
    pub name: String,
    /// pi `state`.
    pub state: String,
    /// pi `modelThinking`.
    pub model_thinking: Option<String>,
    /// pi `activity`.
    pub activity: Option<String>,
    /// pi `startedAt`.
    pub started_at: Option<i64>,
    /// pi `overflow` — set only on the synthetic `… +N more nested leaves` row (`:126`).
    pub overflow: Option<usize>,
}

/// pi `FleetTreeRow` (`fleet-status.ts:37-39`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetTreeRow {
    /// pi `{ kind: "owner", entry }`.
    Owner(FleetStatusEntry),
    /// pi `{ kind: "nested", ownerKey, row, last }`.
    Nested {
        /// pi `ownerKey`.
        owner_key: String,
        /// pi `row`.
        row: FleetNestedRow,
        /// pi `last` — selects `└─` over `├─` (`:453`).
        last: bool,
    },
}

/// pi `nestedRunLabel(run)` (`fleet-status.ts:75-79`): the agent, else a compact agents summary,
/// else the run id.
#[must_use]
pub fn nested_run_label(run: &NestedRunView) -> String {
    if let Some(agent) = run.agent.as_ref() {
        return agent.clone();
    }
    if run.agents.is_empty() {
        return run.id.clone();
    }
    if run.agents.len() == 1 {
        return run.agents.first().cloned().unwrap_or_default();
    }
    let head = run
        .agents
        .get(..2)
        .unwrap_or(&run.agents)
        .join(", ");
    if run.agents.len() > 2 {
        format!("{head} +{}", run.agents.len().saturating_sub(2))
    } else {
        head
    }
}

/// pi `nestedActivity(node)` (`fleet-status.ts:81-87`) — the current tool, else the last path
/// segment of the current path, else the attention/long-running label, else nothing.
fn nested_activity(
    current_tool: Option<&str>,
    current_path: Option<&str>,
    activity_state: Option<ActivityState>,
) -> Option<String> {
    if let Some(tool) = current_tool {
        return Some(format!("tool {tool}"));
    }
    if let Some(path) = current_path {
        return path.rsplit(['/', '\\']).next().map(str::to_string);
    }
    match activity_state {
        Some(ActivityState::NeedsAttention) => Some("needs attention".to_string()),
        Some(ActivityState::ActiveLongRunning) => Some("long-running".to_string()),
        _ => None,
    }
}

/// pi `nestedStatusGlyph(state, theme)` (`fleet-status.ts:89-95`). Note this glyph set differs
/// from the inspector's ([`super::fleet::status_glyph`]): here `failed`/`rejected` are the ONLY
/// error states and everything unrecognised falls to the warning square, whereas the inspector's
/// default arm is the error cross.
#[must_use]
pub fn nested_status_glyph(state: &str) -> Span<'static> {
    match state {
        "running" => th::fg(Role::Accent, "●"),
        "queued" | "pending" => th::fg(Role::Muted, "◦"),
        "complete" | "completed" => th::fg(Role::Success, "✓"),
        "failed" | "rejected" => th::fg(Role::Error, "✗"),
        _ => th::fg(Role::Warning, "■"),
    }
}

/// pi `nestedFleetRows(children)` (`fleet-status.ts:97-128`): flatten each nested child — expanding
/// a `parallel`/`chain` child into its own steps — then keep the first four and fold the rest into
/// one overflow row.
#[must_use]
pub fn nested_fleet_rows(children: &[NestedRunView]) -> Vec<FleetNestedRow> {
    let mut rows: Vec<FleetNestedRow> = Vec::new();
    for child in children {
        let expands = matches!(child.mode, Some(RunMode::Parallel | RunMode::Chain));
        let steps: &[NestedStepView] = if expands { &child.steps } else { &[] };
        if !steps.is_empty() {
            for step in steps {
                rows.push(FleetNestedRow {
                    name: step.agent.clone(),
                    state: step.status.clone(),
                    model_thinking: non_empty(format_model_thinking(
                        step.model.as_deref(),
                        step.thinking.as_deref(),
                    )),
                    activity: nested_activity(
                        step.current_tool.as_deref(),
                        step.current_path.as_deref(),
                        step.activity_state,
                    ),
                    started_at: step.started_at,
                    overflow: None,
                });
            }
            continue;
        }
        rows.push(FleetNestedRow {
            name: nested_run_label(child),
            state: child.state.clone(),
            model_thinking: non_empty(format_model_thinking(
                child.model.as_deref(),
                child.thinking.as_deref(),
            )),
            activity: nested_activity(
                child.current_tool.as_deref(),
                child.current_path.as_deref(),
                child.activity_state,
            ),
            started_at: child.started_at,
            overflow: None,
        });
    }
    let hidden = rows.len().saturating_sub(MAX_NESTED_VISIBLE);
    rows.truncate(MAX_NESTED_VISIBLE);
    if hidden > 0 {
        rows.push(FleetNestedRow {
            name: format!("… +{hidden} more nested leaves"),
            state: "complete".to_string(),
            overflow: Some(hidden),
            ..FleetNestedRow::default()
        });
    }
    rows
}

/// pi `fleetTreeRows(entries)` (`fleet-status.ts:130-138`).
#[must_use]
pub fn fleet_tree_rows(entries: &[FleetStatusEntry]) -> Vec<FleetTreeRow> {
    let mut rows: Vec<FleetTreeRow> = Vec::new();
    for entry in entries {
        rows.push(FleetTreeRow::Owner(entry.clone()));
        let nested = nested_fleet_rows(&entry.nested_children);
        let last_index = nested.len().saturating_sub(1);
        for (index, row) in nested.into_iter().enumerate() {
            rows.push(FleetTreeRow::Nested {
                owner_key: entry.key.clone(),
                row,
                last: index == last_index,
            });
        }
    }
    rows
}

/// pi `formatModelThinking` (`shared/formatters.ts:19-29`) — the same helper
/// `background/fleet_view.rs` ports privately; duplicated here rather than cross-imported for the
/// identical reason that file gives (its copy is `fn`-private to the text fleet view).
fn format_model_thinking(model: Option<&str>, thinking: Option<&str>) -> String {
    const THINKING_LEVELS: [&str; 4] = ["off", "low", "medium", "high"];
    let display_model = model.map(|m| match m.rfind('/') {
        Some(i) => m.get(i.saturating_add(1)..).unwrap_or(m),
        None => m,
    });
    let display_thinking = thinking
        .map(str::trim)
        .filter(|t| THINKING_LEVELS.contains(t));
    [
        display_model.map(str::to_string),
        display_thinking.map(|t| format!("thinking {t}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// JS `x || undefined` over a possibly-empty string (pi `formatModelThinking(...) || undefined`).
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// JS `description?.replace(/\s+/g, " ").trim()` (`fleet-status.ts:444`, `fleet.ts:435,723`).
#[must_use]
pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// =================================================================================================
// collectFleetStatusEntries (pi `fleet-status.ts:147-239`)
// =================================================================================================

/// pi `collectFleetStatusEntries(state)` (`fleet-status.ts:147-239`).
///
/// `now` is the wall clock in epoch millis (pi's `Date.now()`, `:183`), taken as a parameter so
/// this fold stays pure and testable.
#[must_use]
pub fn collect_fleet_status_entries(state: &FleetState, now: i64) -> Vec<FleetStatusEntry> {
    let mut entries: Vec<FleetStatusEntry> = Vec::new();

    for control in &state.foreground_controls {
        if !control.active_children.is_empty() {
            let mut children = control.active_children.clone();
            children.sort_by_key(|child| child.index);
            let single = children.len() == 1;
            for child in &children {
                let attached: Vec<NestedRunView> = control
                    .nested_children
                    .iter()
                    .filter(|nested| nested.parent_step_index == Some(child.index))
                    .cloned()
                    .collect();
                // pi `:161-163`: the step's own nested children, else — for a single-child run —
                // the run's whole nested set, else nothing.
                let nested_children = if attached.is_empty() {
                    if single { control.nested_children.clone() } else { Vec::new() }
                } else {
                    attached
                };
                entries.push(FleetStatusEntry {
                    key: format!("foreground-active:{}:{}", control.run_id, child.index),
                    agent: child.agent.clone(),
                    model_thinking: non_empty(format_model_thinking(
                        child.model.as_deref(),
                        child.thinking.as_deref(),
                    )),
                    description: child.description.clone(),
                    started_at: child.started_at,
                    tokens: child.tokens.unwrap_or(0),
                    state: "running".to_string(),
                    nested_children,
                });
            }
            continue;
        }
        entries.push(FleetStatusEntry {
            key: format!(
                "foreground-active:{}:{}",
                control.run_id,
                control.current_index.unwrap_or(0)
            ),
            agent: control
                .current_agent
                .clone()
                .unwrap_or_else(|| run_mode_label(control.mode).to_string()),
            model_thinking: non_empty(format_model_thinking(
                control.model.as_deref(),
                control.thinking.as_deref(),
            )),
            description: control.description.clone(),
            started_at: control.started_at,
            tokens: control.tokens.unwrap_or(0),
            state: "running".to_string(),
            nested_children: control.nested_children.clone(),
        });
    }

    for job in &state.tracked_jobs {
        let status = &job.status;
        if !is_active_state(job.state_label()) {
            continue;
        }
        let started_at = if status.started_at != 0 { status.started_at } else { now };
        let total_tokens = status
            .telemetry
            .total_tokens
            .as_ref()
            .map_or(0, |t| t.total);
        // pi `:197-215`: no steps at all → one run-level entry.
        if status.steps.is_empty() {
            entries.push(FleetStatusEntry {
                key: format!("async:{}", status.run_id.as_str()),
                agent: run_mode_label(status.mode).to_string(),
                model_thinking: None,
                description: job.description.clone(),
                started_at,
                tokens: total_tokens,
                state: job.state_label().to_string(),
                nested_children: job.nested_children.clone(),
            });
            continue;
        }
        let active_parallel_group = has_active_parallel_group(job);
        let single_step = status.steps.len() == 1;
        for (offset, step) in status.steps.iter().enumerate() {
            let label = step_label(step.status);
            if !is_active_state(label) {
                continue;
            }
            let index = offset;
            // pi `:219` — a chain shows only the step actually current, unless a parallel group
            // inside it is live.
            if step.status == StepState::Pending
                && status.mode == RunMode::Chain
                && !active_parallel_group
                && index != status.current_step.unwrap_or(0)
            {
                continue;
            }
            let nested_children: Vec<NestedRunView> = job
                .nested_children
                .iter()
                .filter(|nested| nested.parent_step_index == Some(index))
                .cloned()
                .collect();
            entries.push(FleetStatusEntry {
                key: format!("async:{}:{index}", status.run_id.as_str()),
                agent: step.agent.clone(),
                model_thinking: non_empty(format_model_thinking(
                    step.model.as_ref().map(cyrup_core::ModelId::as_str),
                    step.telemetry.thinking.as_deref(),
                )),
                description: job.description.clone(),
                started_at: step.started_at.unwrap_or(started_at),
                tokens: step
                    .telemetry
                    .tokens
                    .as_ref()
                    .map_or_else(|| if single_step { total_tokens } else { 0 }, |t| t.total),
                state: label.to_string(),
                nested_children,
            });
        }
    }

    // pi `:238` — `startedAt` ascending, ties broken by key.
    entries.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.key.cmp(&right.key))
    });
    entries
}

/// pi's `job.activeParallelGroup` marker (`fleet-status.ts:201,219`). cyrup keeps no such live
/// flag; the equivalent observable is "this run has a parallel group with a child still running or
/// pending", which is exactly the condition the flag exists to express.
fn has_active_parallel_group(job: &AsyncRunView) -> bool {
    job.status.parallel_groups.as_ref().is_some_and(|groups| {
        groups.iter().any(|group| {
            group
                .children
                .iter()
                .any(|child| matches!(child.status, StepState::Running | StepState::Pending))
        })
    })
}

/// The rendered lowercase mode string (pi `SubagentRunMode`).
fn run_mode_label(mode: RunMode) -> &'static str {
    crate::background::run_status::run_mode_label(mode)
}

/// The rendered lowercase step-status string (pi `AsyncJobStep.status`).
fn step_label(status: StepState) -> &'static str {
    crate::background::run_status::step_state_label(status)
}

// =================================================================================================
// SubagentFleetStatus (pi `fleet-status.ts:241-564`)
// =================================================================================================

/// pi `FleetStatusOptions` (`fleet-status.ts:41-45`).
#[derive(Clone, Copy, Debug)]
pub struct FleetStatusOptions {
    /// pi `refreshMs` (default [`REFRESH_MS`]).
    pub refresh_ms: u64,
    /// pi `maxAgentRows` (default [`MAX_AGENT_ROWS`]).
    pub max_agent_rows: usize,
    /// pi `placement` (default [`FleetViewPlacement::BelowEditor`]).
    pub placement: FleetViewPlacement,
}

impl Default for FleetStatusOptions {
    fn default() -> Self {
        Self {
            refresh_ms: REFRESH_MS,
            max_agent_rows: MAX_AGENT_ROWS,
            placement: FleetViewPlacement::default(),
        }
    }
}

/// What [`SubagentFleetStatus::handle_key`] tells its owner to do with the keystroke — pi's
/// `{ consume?: boolean; data?: string } | undefined` return (`fleet-status.ts:352`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetStatusKeyOutcome {
    /// pi `{ consume: true }` — the widget handled it; do not forward it to the editor.
    Consume,
    /// pi `undefined` — not ours; forward it.
    Pass,
    /// pi `{ consume: true }` from the `Key.enter` arm — additionally, OPEN the inspector for the
    /// carried roster key (pi `void Promise.resolve().then(() => this.openInspector(selectedKey))`,
    /// `:398-404`). The owner is responsible for calling
    /// [`SubagentFleetStatus::set_inspector_open`] around that work, exactly as pi's `finally`
    /// does.
    OpenInspector,
}

/// pi's `SubagentFleetStatus` class (`fleet-status.ts:241-564`) — the widget's whole state
/// machine, minus the `ExtensionContext`/`setInterval`/`onTerminalInput` plumbing the module doc
/// explains cyrup replaces with owner-driven calls.
#[derive(Debug)]
pub struct SubagentFleetStatus {
    /// pi `active` — false is the one-line collapsed form (`:414-418`).
    active: bool,
    /// pi `selectedKey`, seeded to `"main"` (`:249`).
    selected_key: String,
    /// pi `inspectorOpen` (`:250`).
    inspector_open: bool,
    /// pi `lastRenderKey` (`:251`).
    last_render_key: String,
    /// pi `entries` (`:252`).
    entries: Vec<FleetStatusEntry>,
    /// pi `widgetRegistered` (`:247`).
    widget_registered: bool,
    /// pi's `this.ui`/`this.ctx` pair collapsed to the one thing they are consulted for
    /// (`getActiveUiContext`, `:521-531`): whether a UI is attached at all.
    ui_available: bool,
    /// pi `maxAgentRows` (`:256`).
    max_agent_rows: usize,
    /// pi `placement` (`:257`).
    placement: FleetViewPlacement,
    /// pi `refreshMs` (`:255`) — kept so the owner's poll can honour the configured cadence.
    refresh_ms: u64,
}

impl Default for SubagentFleetStatus {
    fn default() -> Self {
        Self::new(FleetStatusOptions::default())
    }
}

impl SubagentFleetStatus {
    /// pi's constructor (`fleet-status.ts:259-269`).
    #[must_use]
    pub fn new(options: FleetStatusOptions) -> Self {
        Self {
            active: false,
            selected_key: "main".to_string(),
            inspector_open: false,
            last_render_key: String::new(),
            entries: Vec::new(),
            widget_registered: false,
            ui_available: false,
            max_agent_rows: options.max_agent_rows,
            placement: options.placement,
            refresh_ms: options.refresh_ms,
        }
    }

    /// pi `refreshMs` — the cadence the owner should poll [`Self::refresh`] at.
    #[must_use]
    pub fn refresh_ms(&self) -> u64 {
        self.refresh_ms
    }

    /// pi `placement`.
    #[must_use]
    pub fn placement(&self) -> FleetViewPlacement {
        self.placement
    }

    /// pi `setContext(ctx)`'s `if (!ctx.hasUI) return` gate + `dispose()`'s teardown
    /// (`fleet-status.ts:271-299`) collapsed into one setter: attaching a UI arms the widget,
    /// detaching one clears every piece of registration state pi's `clearUiRegistration` clears
    /// (`:533-563`).
    pub fn set_ui_available(&mut self, available: bool) {
        if self.ui_available == available {
            return;
        }
        self.ui_available = available;
        if !available {
            self.entries.clear();
            self.active = false;
            self.selected_key = "main".to_string();
            self.inspector_open = false;
            self.last_render_key = String::new();
            self.widget_registered = false;
        }
    }

    /// pi `inspectorOpen` — set by the owner around its `openInspector` await, mirroring pi's
    /// `try { … } finally { this.inspectorOpen = false; this.refresh(); }` (`:395-404`).
    pub fn set_inspector_open(&mut self, open: bool) {
        self.inspector_open = open;
    }

    /// pi `selectedKey` — the roster key `Enter` opens the inspector at.
    #[must_use]
    pub fn selected_key(&self) -> &str {
        &self.selected_key
    }

    /// pi `active`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// pi `entries`.
    #[must_use]
    pub fn entries(&self) -> &[FleetStatusEntry] {
        &self.entries
    }

    /// pi `widgetRegistered` — whether a widget payload is currently published.
    #[must_use]
    pub fn is_widget_registered(&self) -> bool {
        self.widget_registered
    }

    /// pi `refresh()` (`fleet-status.ts:301-350`), minus the `setWidget` call itself.
    ///
    /// Returns `true` when the owner should republish (or clear — see
    /// [`Self::is_widget_registered`]) the widget. pi's three early returns are all preserved: no
    /// UI at all, the inspector being open (either this widget's own `Enter` or the extension-wide
    /// `state.fleetInspectorOpen`), and an empty entry list all unregister the widget; an
    /// unchanged [`Self::render_key`] repaints nothing.
    pub fn refresh(&mut self, state: &FleetState, now: i64) -> bool {
        if !self.ui_available {
            return false;
        }
        self.entries = collect_fleet_status_entries(state, now);
        self.clamp_selection();
        if self.inspector_open || state.fleet_inspector_open {
            self.last_render_key = String::new();
            if self.widget_registered {
                self.widget_registered = false;
                return true;
            }
            return false;
        }
        if self.entries.is_empty() {
            self.active = false;
            self.selected_key = "main".to_string();
            self.last_render_key = String::new();
            if self.widget_registered {
                self.widget_registered = false;
                return true;
            }
            return false;
        }
        let render_key = self.render_key(now);
        if !self.widget_registered {
            self.widget_registered = true;
            self.last_render_key = render_key;
            return true;
        }
        if render_key == self.last_render_key {
            return false;
        }
        self.last_render_key = render_key;
        true
    }

    /// pi `handleKey(data)` (`fleet-status.ts:352-410`).
    ///
    /// `editor_has_focus` and `editor_text` are what pi reads off the live UI
    /// (`this.editorHasFocus()` `:357` and `ctx.ui.getEditorText()` `:363`); see the module doc for
    /// why they are parameters here.
    pub fn handle_key(
        &mut self,
        key: &FleetStatusKey,
        editor_has_focus: bool,
        editor_text: &str,
    ) -> FleetStatusKeyOutcome {
        if !self.ui_available || self.entries.is_empty() || key.is_release {
            return FleetStatusKeyOutcome::Pass;
        }
        if self.inspector_open {
            return FleetStatusKeyOutcome::Pass;
        }
        if !editor_has_focus {
            if self.active {
                self.deactivate();
            }
            return FleetStatusKeyOutcome::Pass;
        }

        if !self.active {
            let activates = key.code == FleetStatusKeyCode::Down || key.code == FleetStatusKeyCode::Left;
            if !activates || !editor_text.is_empty() {
                return FleetStatusKeyOutcome::Pass;
            }
            self.active = true;
            self.selected_key = "main".to_string();
            return FleetStatusKeyOutcome::Consume;
        }

        let roster = self.roster_keys();
        let selected_index = roster
            .iter()
            .position(|k| *k == self.selected_key)
            .unwrap_or(0);
        match key.code {
            FleetStatusKeyCode::Down | FleetStatusKeyCode::Char('j') => {
                let next = selected_index
                    .saturating_add(1)
                    .min(roster.len().saturating_sub(1));
                self.selected_key = roster.get(next).cloned().unwrap_or_else(|| "main".to_string());
                FleetStatusKeyOutcome::Consume
            }
            FleetStatusKeyCode::Up | FleetStatusKeyCode::Char('k') => {
                if selected_index == 0 {
                    self.deactivate();
                    return FleetStatusKeyOutcome::Consume;
                }
                self.selected_key = roster
                    .get(selected_index.saturating_sub(1))
                    .cloned()
                    .unwrap_or_else(|| "main".to_string());
                FleetStatusKeyOutcome::Consume
            }
            FleetStatusKeyCode::Escape => {
                self.deactivate();
                FleetStatusKeyOutcome::Consume
            }
            FleetStatusKeyCode::Enter => {
                if self.selected_key == "main" {
                    self.deactivate();
                    return FleetStatusKeyOutcome::Consume;
                }
                self.inspector_open = true;
                FleetStatusKeyOutcome::OpenInspector
            }
            _ => {
                self.deactivate();
                FleetStatusKeyOutcome::Pass
            }
        }
    }

    /// pi `render(width, theme)` (`fleet-status.ts:412-441`).
    #[must_use]
    pub fn render(&self, width: usize, now: i64) -> Vec<Line<'static>> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        if !self.active {
            let tokens: u64 = self
                .entries
                .iter()
                .fold(0u64, |total, entry| total.saturating_add(entry.tokens));
            let label = format!(
                "{} active {}",
                self.entries.len(),
                if self.entries.len() == 1 { "agent" } else { "agents" }
            );
            return vec![th::clip(
                &Line::from(vec![
                    th::raw("  "),
                    th::fg(Role::Muted, label),
                    th::raw(" · "),
                    th::fg(
                        Role::Dim,
                        format!("{} · ↓/← to inspect", format_fleet_tokens(tokens)),
                    ),
                ]),
                width,
            )];
        }

        let roster = self.roster_keys();
        let selected_index = roster
            .iter()
            .position(|k| *k == self.selected_key)
            .unwrap_or(0);
        let mut lines = vec![
            th::clip(
                &Line::from(vec![
                    th::raw("  "),
                    th::fg(Role::Dim, "↑↓/jk select · enter inspect · esc back"),
                ]),
                width,
            ),
            Line::from(Vec::<Span<'static>>::new()),
        ];
        lines.push(th::clip(
            &Line::from(vec![
                th::raw("  "),
                self.bullet(0, selected_index),
                th::raw(" main"),
            ]),
            width,
        ));

        let tree = fleet_tree_rows(&self.entries);
        let selected_tree_index = tree
            .iter()
            .position(|row| matches!(row, FleetTreeRow::Owner(entry) if entry.key == self.selected_key))
            .unwrap_or(0);
        let visible_count = self.max_agent_rows.min(tree.len());
        let start = if selected_tree_index < visible_count {
            0
        } else {
            selected_tree_index
                .saturating_sub(visible_count)
                .saturating_add(1)
        };
        let hidden_below = tree
            .len()
            .saturating_sub(start.saturating_add(visible_count));
        if start > 0 {
            lines.push(th::right_align_status(
                &Line::from(Vec::<Span<'static>>::new()),
                &Line::from(vec![th::fg(Role::Dim, format!("↑ {start} more"))]),
                width,
            ));
        }
        for row in tree.iter().skip(start).take(visible_count) {
            match row {
                FleetTreeRow::Owner(entry) => {
                    let owner_index = self
                        .entries
                        .iter()
                        .position(|candidate| candidate.key == entry.key)
                        .map_or(0, |i| i.saturating_add(1));
                    lines.push(self.render_entry(owner_index, selected_index, entry, width, now));
                }
                FleetTreeRow::Nested { row, last, .. } => {
                    lines.push(render_nested_row(row, *last, width, now));
                }
            }
        }
        if hidden_below > 0 {
            lines.push(th::right_align_status(
                &Line::from(Vec::<Span<'static>>::new()),
                &Line::from(vec![th::fg(Role::Dim, format!("↓ {hidden_below} more"))]),
                width,
            ));
        }
        lines
    }

    /// pi `renderEntry` (`fleet-status.ts:443-450`).
    fn render_entry(
        &self,
        roster_index: usize,
        selected_index: usize,
        entry: &FleetStatusEntry,
        width: usize,
        now: i64,
    ) -> Line<'static> {
        let description = entry.description.as_deref().map(collapse_whitespace);
        let agent = match entry.model_thinking.as_ref() {
            Some(mt) => format!("{} ({mt})", entry.agent),
            None => entry.agent.clone(),
        };
        let mut left = vec![
            th::raw("  "),
            self.bullet(roster_index, selected_index),
            th::raw(" "),
            th::fg(Role::Muted, agent),
            th::raw(format!(" · {}", entry.state)),
        ];
        if let Some(description) = description.filter(|d| !d.is_empty()) {
            left.push(th::raw(format!("  {description}")));
        }
        let elapsed = now.saturating_sub(entry.started_at);
        let right = Line::from(vec![th::fg(
            Role::Dim,
            format!(
                "{} · {}",
                format_fleet_elapsed(elapsed),
                format_fleet_tokens(entry.tokens)
            ),
        )]);
        th::right_align_status(&Line::from(left), &right, width)
    }

    /// pi `bullet` (`fleet-status.ts:462-464`).
    fn bullet(&self, roster_index: usize, selected_index: usize) -> Span<'static> {
        if roster_index == selected_index {
            th::fg(Role::Accent, ">")
        } else {
            th::raw(" ")
        }
    }

    /// pi `rosterKeys` (`fleet-status.ts:466-468`).
    fn roster_keys(&self) -> Vec<String> {
        let mut keys = vec!["main".to_string()];
        keys.extend(self.entries.iter().map(|entry| entry.key.clone()));
        keys
    }

    /// pi `clampSelection` (`fleet-status.ts:470-472`).
    fn clamp_selection(&mut self) {
        if !self.roster_keys().contains(&self.selected_key) {
            self.selected_key = "main".to_string();
        }
    }

    /// pi `deactivate` (`fleet-status.ts:474-478`).
    fn deactivate(&mut self) {
        self.active = false;
        self.selected_key = "main".to_string();
    }

    /// pi `getRenderKey()` (`fleet-status.ts:493-519`) — the change-detector. Collapsed (inactive)
    /// it keys on `(key, state, tokens)` only, so a token counter ticking upward still repaints
    /// but a mere clock tick does not; expanded it additionally keys on the agent, the model, the
    /// description, whole seconds elapsed, and the nested tree.
    #[must_use]
    pub fn render_key(&self, now: i64) -> String {
        let entries: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|entry| {
                if self.active {
                    serde_json::json!([
                        entry.key,
                        entry.agent,
                        entry.state,
                        entry.model_thinking,
                        entry.description,
                        (now.saturating_sub(entry.started_at) as f64 / 1000.0).round() as i64,
                        entry.tokens,
                        entry
                            .nested_children
                            .iter()
                            .map(|child| {
                                serde_json::json!([
                                    child.id,
                                    child.state,
                                    child.model,
                                    child.thinking,
                                    child.last_update,
                                    child
                                        .steps
                                        .iter()
                                        .map(|step| serde_json::json!([
                                            step.agent,
                                            step.status,
                                            step.model,
                                            step.thinking,
                                            step.last_activity_at
                                        ]))
                                        .collect::<Vec<_>>()
                                ])
                            })
                            .collect::<Vec<_>>()
                    ])
                } else {
                    serde_json::json!([entry.key, entry.state, entry.tokens])
                }
            })
            .collect();
        serde_json::json!({
            "active": self.active,
            "selected": self.selected_key,
            "inspectorOpen": self.inspector_open,
            "entries": entries,
        })
        .to_string()
    }

    /// The payload the owner hands to `HostServices::set_widget` — pi's
    /// `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, factory, { placement })` (`fleet-status.ts:329`)
    /// reduced to cyrup's fire-and-forget JSON shape (module doc, transport difference 1).
    ///
    /// `None` means "clear the widget", exactly as pi's
    /// `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, undefined)` (`:309,320`) does.
    #[must_use]
    pub fn widget_payload(&self, width: usize, now: i64) -> Option<serde_json::Value> {
        if !self.widget_registered {
            return None;
        }
        let lines: Vec<String> = self
            .render(width, now)
            .iter()
            .map(th::line_text)
            .collect();
        Some(serde_json::json!({
            "key": FLEET_STATUS_WIDGET_KEY,
            "placement": self.placement.as_str(),
            "widget": "box",
            "children": lines
                .into_iter()
                .map(|text| serde_json::json!({ "widget": "text", "text": text }))
                .collect::<Vec<_>>(),
        }))
    }
}

/// pi `renderNestedRow` (`fleet-status.ts:452-460`).
#[must_use]
pub fn render_nested_row(
    row: &FleetNestedRow,
    last: bool,
    width: usize,
    now: i64,
) -> Line<'static> {
    let marker = if last { "└─" } else { "├─" };
    if let Some(overflow) = row.overflow {
        return th::clip(
            &Line::from(vec![
                th::raw(format!("    {marker} ")),
                th::fg(Role::Dim, format!("+{overflow} nested leaves")),
            ]),
            width,
        );
    }
    let model_thinking = row
        .model_thinking
        .as_ref()
        .map(|mt| format!(" ({mt})"))
        .unwrap_or_default();
    let activity = row
        .activity
        .as_ref()
        .map(|a| format!(" · {a}"))
        .unwrap_or_default();
    let mut spans = vec![
        th::raw(format!("    {marker} ")),
        nested_status_glyph(&row.state),
        th::raw(" "),
        th::fg(Role::Muted, format!("{}{model_thinking}", row.name)),
        th::raw(format!(" · {}{activity}", row.state)),
    ];
    if let Some(started_at) = row.started_at {
        spans.push(th::fg(
            Role::Dim,
            format!(" · {}", format_fleet_elapsed(now.saturating_sub(started_at))),
        ));
    }
    th::clip(&Line::from(spans), width)
}

// =================================================================================================
// Key input (pi's raw `data: string` from `ctx.ui.onTerminalInput`)
// =================================================================================================

/// The key codes `fleet-status.ts:362-390` matches on, via `matchesKey(data, …)` / `Key.enter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetStatusKeyCode {
    /// pi `matchesKey(data, "up")`.
    Up,
    /// pi `matchesKey(data, "down")`.
    Down,
    /// pi `matchesKey(data, "left")`.
    Left,
    /// pi `matchesKey(data, "escape")`.
    Escape,
    /// pi `matchesKey(data, Key.enter)`.
    Enter,
    /// A printable character — pi's `"j"`/`"k"` bindings, and the catch-all that deactivates.
    Char(char),
    /// Any other key — pi's final `this.deactivate(); return undefined;` (`:408-409`).
    Other,
}

/// One keystroke, as [`SubagentFleetStatus::handle_key`] consumes it — pi's raw `data: string`
/// plus the `isKeyRelease(data)` predicate it checks first (`fleet-status.ts:354`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetStatusKey {
    /// Which key.
    pub code: FleetStatusKeyCode,
    /// pi `isKeyRelease(data)` — a release event is ignored outright.
    pub is_release: bool,
}

impl FleetStatusKey {
    /// A press of `code`.
    #[must_use]
    pub const fn press(code: FleetStatusKeyCode) -> Self {
        Self { code, is_release: false }
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
    use crate::background::{RunId, RunState, RunStatus, StepStatus, TokenTotals};
    use crate::tui::fleet_state::{ForegroundChildView, ForegroundControlView};
    use std::path::PathBuf;

    fn control(run_id: &str, agent: &str, started_at: i64) -> ForegroundControlView {
        ForegroundControlView {
            run_id: run_id.to_string(),
            mode: RunMode::Single,
            started_at,
            updated_at: started_at,
            current_agent: Some(agent.to_string()),
            tokens: Some(1500),
            ..ForegroundControlView::default()
        }
    }

    fn async_view(run_id: &str, state: RunState, steps: Vec<StepStatus>) -> AsyncRunView {
        let mut status = RunStatus::queued(RunId::from_token(run_id.to_string()), RunMode::Chain, None);
        status.state = state;
        status.steps = steps;
        status.started_at = 1_000;
        status.last_update = 1_000;
        status.telemetry.total_tokens = Some(TokenTotals { input: 10, output: 10, total: 20 });
        AsyncRunView {
            paths: crate::background::RunPaths::for_run(
                &PathBuf::from("/tmp/async"),
                &PathBuf::from("/tmp/results"),
                &RunId::from_token(run_id.to_string()),
            ),
            status,
            session_id: None,
            description: Some("do the thing".to_string()),
            context: None,
            nested_children: Vec::new(),
        }
    }

    fn step(agent: &str, status: StepState) -> StepStatus {
        let mut s = StepStatus::pending(agent.to_string());
        s.status = status;
        s
    }

    // -----------------------------------------------------------------------------------------
    // Formatters (pi :47-73)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn placement_resolution_accepts_only_the_exact_above_editor_string() {
        assert_eq!(
            resolve_fleet_view_placement(Some("aboveEditor")),
            FleetViewPlacement::AboveEditor
        );
        assert_eq!(
            resolve_fleet_view_placement(Some("AboveEditor")),
            FleetViewPlacement::BelowEditor
        );
        assert_eq!(resolve_fleet_view_placement(None), FleetViewPlacement::BelowEditor);
    }

    #[test]
    fn elapsed_and_token_formatting_match_upstream() {
        assert_eq!(format_fleet_elapsed(0), "0s");
        assert_eq!(format_fleet_elapsed(-5000), "0s");
        assert_eq!(format_fleet_elapsed(1499), "1s");
        assert_eq!(format_fleet_elapsed(1500), "2s");
        assert_eq!(format_fleet_tokens(42), "↓ 42 tokens");
        assert_eq!(format_fleet_tokens(1_234), "↓ 1.2k tokens");
        assert_eq!(format_fleet_tokens(2_500_000), "↓ 2.5M tokens");
        // JS `toFixed(1)` rounds half AWAY FROM ZERO; Rust's `{:.1}` rounds half to even, which
        // would print "1.2k" here.
        assert_eq!(format_fleet_tokens(1_250), "↓ 1.3k tokens");
    }

    #[test]
    fn active_state_set_is_exactly_upstreams_three() {
        assert!(is_active_state("running"));
        assert!(is_active_state("queued"));
        assert!(is_active_state("pending"));
        assert!(!is_active_state("complete"));
        assert!(!is_active_state("paused"));
    }

    // -----------------------------------------------------------------------------------------
    // collectFleetStatusEntries (pi :147-239)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn foreground_control_without_children_yields_one_entry() {
        let state = FleetState {
            foreground_controls: vec![control("run-a", "coder", 500)],
            ..FleetState::default()
        };
        let entries = collect_fleet_status_entries(&state, 10_000);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "foreground-active:run-a:0");
        assert_eq!(entries[0].agent, "coder");
        assert_eq!(entries[0].state, "running");
        assert_eq!(entries[0].tokens, 1500);
    }

    #[test]
    fn foreground_active_children_expand_and_sort_by_index() {
        let mut c = control("run-b", "coder", 100);
        c.active_children = vec![
            ForegroundChildView { index: 1, agent: "b".into(), started_at: 300, ..Default::default() },
            ForegroundChildView { index: 0, agent: "a".into(), started_at: 200, ..Default::default() },
        ];
        let state = FleetState { foreground_controls: vec![c], ..FleetState::default() };
        let entries = collect_fleet_status_entries(&state, 10_000);
        assert_eq!(
            entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
            vec!["foreground-active:run-b:0", "foreground-active:run-b:1"]
        );
    }

    #[test]
    fn only_active_background_runs_and_active_steps_are_listed() {
        let state = FleetState {
            tracked_jobs: vec![
                async_view("r1", RunState::Running, vec![step("a", StepState::Running), step("b", StepState::Complete)]),
                async_view("r2", RunState::Complete, vec![step("c", StepState::Complete)]),
            ],
            ..FleetState::default()
        };
        let entries = collect_fleet_status_entries(&state, 10_000);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "async:r1:0");
        assert_eq!(entries[0].agent, "a");
        assert_eq!(entries[0].description.as_deref(), Some("do the thing"));
    }

    #[test]
    fn a_chain_hides_pending_steps_that_are_not_the_current_one() {
        let mut job = async_view(
            "r3",
            RunState::Running,
            vec![step("a", StepState::Pending), step("b", StepState::Pending)],
        );
        job.status.mode = RunMode::Chain;
        job.status.current_step = Some(1);
        let state = FleetState { tracked_jobs: vec![job], ..FleetState::default() };
        let entries = collect_fleet_status_entries(&state, 10_000);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "async:r3:1");
    }

    #[test]
    fn a_run_with_no_steps_yields_one_run_level_entry_labelled_by_mode() {
        let state = FleetState {
            tracked_jobs: vec![async_view("r4", RunState::Queued, Vec::new())],
            ..FleetState::default()
        };
        let entries = collect_fleet_status_entries(&state, 10_000);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "async:r4");
        assert_eq!(entries[0].agent, "chain");
        assert_eq!(entries[0].tokens, 20);
    }

    #[test]
    fn entries_sort_by_started_at_then_key() {
        let state = FleetState {
            foreground_controls: vec![control("z", "late", 900), control("a", "early", 100)],
            ..FleetState::default()
        };
        let entries = collect_fleet_status_entries(&state, 10_000);
        assert_eq!(
            entries.iter().map(|e| e.agent.as_str()).collect::<Vec<_>>(),
            vec!["early", "late"]
        );
    }

    // -----------------------------------------------------------------------------------------
    // Nested tree (pi :97-138)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn nested_rows_cap_at_four_and_fold_the_rest() {
        let children: Vec<NestedRunView> = (0..7)
            .map(|i| NestedRunView {
                id: format!("n{i}"),
                agent: Some(format!("a{i}")),
                state: "running".into(),
                ..NestedRunView::default()
            })
            .collect();
        let rows = nested_fleet_rows(&children);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[4].overflow, Some(3));
        assert_eq!(rows[4].name, "… +3 more nested leaves");
    }

    #[test]
    fn a_parallel_nested_child_expands_into_its_steps() {
        let child = NestedRunView {
            id: "n0".into(),
            mode: Some(RunMode::Parallel),
            state: "running".into(),
            steps: vec![
                NestedStepView { agent: "x".into(), status: "running".into(), ..Default::default() },
                NestedStepView { agent: "y".into(), status: "pending".into(), ..Default::default() },
            ],
            ..NestedRunView::default()
        };
        let rows = nested_fleet_rows(std::slice::from_ref(&child));
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["x", "y"]);
    }

    #[test]
    fn nested_run_label_falls_back_agent_then_agents_then_id() {
        let mut run = NestedRunView { id: "the-id".into(), ..NestedRunView::default() };
        assert_eq!(nested_run_label(&run), "the-id");
        run.agents = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        assert_eq!(nested_run_label(&run), "a, b +2");
        run.agents = vec!["solo".into()];
        assert_eq!(nested_run_label(&run), "solo");
        run.agent = Some("named".into());
        assert_eq!(nested_run_label(&run), "named");
    }

    #[test]
    fn tree_rows_mark_only_the_last_nested_row_as_last() {
        let entry = FleetStatusEntry {
            key: "k".into(),
            agent: "a".into(),
            nested_children: vec![
                NestedRunView { id: "n0".into(), agent: Some("x".into()), state: "running".into(), ..Default::default() },
                NestedRunView { id: "n1".into(), agent: Some("y".into()), state: "running".into(), ..Default::default() },
            ],
            ..FleetStatusEntry::default()
        };
        let rows = fleet_tree_rows(std::slice::from_ref(&entry));
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[1], FleetTreeRow::Nested { last: false, .. }));
        assert!(matches!(rows[2], FleetTreeRow::Nested { last: true, .. }));
    }

    // -----------------------------------------------------------------------------------------
    // The widget state machine (pi :241-564)
    // -----------------------------------------------------------------------------------------

    fn armed_widget(state: &FleetState) -> SubagentFleetStatus {
        let mut widget = SubagentFleetStatus::default();
        widget.set_ui_available(true);
        widget.refresh(state, 10_000);
        widget
    }

    #[test]
    fn refresh_registers_the_widget_only_when_there_is_work() {
        let empty = FleetState::default();
        let mut widget = SubagentFleetStatus::default();
        widget.set_ui_available(true);
        assert!(!widget.refresh(&empty, 0));
        assert!(!widget.is_widget_registered());

        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        assert!(widget.refresh(&busy, 1_000));
        assert!(widget.is_widget_registered());
        assert!(widget.widget_payload(80, 1_000).is_some());
    }

    #[test]
    fn refresh_unregisters_while_the_inspector_is_open() {
        let mut busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        assert!(widget.is_widget_registered());
        busy.fleet_inspector_open = true;
        assert!(widget.refresh(&busy, 10_000));
        assert!(!widget.is_widget_registered());
        assert!(widget.widget_payload(80, 10_000).is_none());
    }

    #[test]
    fn an_unchanged_render_key_suppresses_the_repaint() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        assert!(!widget.refresh(&busy, 10_500));
    }

    #[test]
    fn collapsed_row_reads_agent_count_and_token_total() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0), control("b", "tester", 0)],
            ..FleetState::default()
        };
        let widget = armed_widget(&busy);
        let text = th::lines_text(&widget.render(80, 10_000));
        assert!(text.contains("2 active agents"), "{text}");
        assert!(text.contains("↓ 3.0k tokens"), "{text}");
        assert!(text.contains("↓/← to inspect"), "{text}");
    }

    #[test]
    fn down_on_an_empty_editor_activates_and_consumes() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        assert_eq!(
            widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, ""),
            FleetStatusKeyOutcome::Consume
        );
        assert!(widget.is_active());
        assert_eq!(widget.selected_key(), "main");
    }

    #[test]
    fn down_with_editor_text_present_is_passed_through() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        assert_eq!(
            widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "hello"),
            FleetStatusKeyOutcome::Pass
        );
        assert!(!widget.is_active());
    }

    #[test]
    fn losing_editor_focus_deactivates() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "");
        assert!(widget.is_active());
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), false, "");
        assert!(!widget.is_active());
    }

    #[test]
    fn up_from_main_deactivates_and_enter_on_a_row_opens_the_inspector() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "");
        // main -> first entry
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "");
        assert_eq!(widget.selected_key(), "foreground-active:a:0");
        assert_eq!(
            widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Enter), true, ""),
            FleetStatusKeyOutcome::OpenInspector
        );
        // Back at main, Up deactivates rather than wrapping.
        widget.set_inspector_open(false);
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Up), true, "");
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Up), true, "");
        assert!(!widget.is_active());
    }

    #[test]
    fn enter_on_main_just_deactivates() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "");
        assert_eq!(
            widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Enter), true, ""),
            FleetStatusKeyOutcome::Consume
        );
        assert!(!widget.is_active());
    }

    #[test]
    fn a_key_release_is_ignored() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        let release = FleetStatusKey { code: FleetStatusKeyCode::Down, is_release: true };
        assert_eq!(widget.handle_key(&release, true, ""), FleetStatusKeyOutcome::Pass);
        assert!(!widget.is_active());
    }

    #[test]
    fn expanded_render_lists_main_plus_the_roster() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "");
        let text = th::lines_text(&widget.render(100, 10_000));
        assert!(text.contains("↑↓/jk select · enter inspect · esc back"), "{text}");
        assert!(text.contains("> main"), "{text}");
        assert!(text.contains("coder · running"), "{text}");
        assert!(text.contains("10s"), "{text}");
    }

    // -----------------------------------------------------------------------------------------
    // Painted-cell style assertions — the half `lines_text` cannot see
    // -----------------------------------------------------------------------------------------

    #[test]
    fn the_five_nested_status_glyphs_paint_five_distinct_roles() {
        // Note the DEFAULT arm is the warning square, unlike `fleet.rs`'s error cross — identical
        // call shape, different colour, and only a painted assertion can tell them apart.
        for (state, glyph, role) in [
            ("running", "●", Role::Accent),
            ("queued", "◦", Role::Muted),
            ("completed", "✓", Role::Success),
            ("failed", "✗", Role::Error),
            ("detached", "■", Role::Warning),
        ] {
            let line = Line::from(vec![nested_status_glyph(state)]);
            assert!(
                th::paints_as(th::painted_style(std::slice::from_ref(&line), 4, glyph), role),
                "{state} must paint as {role:?}"
            );
        }
    }

    #[test]
    fn the_collapsed_row_paints_its_count_muted_and_its_hint_dim() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0), control("b", "tester", 0)],
            ..FleetState::default()
        };
        let widget = armed_widget(&busy);
        let lines = widget.render(80, 10_000);
        assert!(th::paints_as(th::painted_style(&lines, 80, "2 active agents"), Role::Muted));
        assert!(th::paints_as(th::painted_style(&lines, 80, "↓/← to inspect"), Role::Dim));
    }

    #[test]
    fn the_expanded_roster_paints_its_selection_bullet_accent_and_its_agent_muted() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        widget.handle_key(&FleetStatusKey::press(FleetStatusKeyCode::Down), true, "");
        let lines = widget.render(100, 10_000);
        assert!(
            th::paints_as(th::painted_style(&lines, 100, ">"), Role::Accent),
            "the selection bullet is accent"
        );
        assert!(
            th::paints_as(th::painted_style(&lines, 100, "coder"), Role::Muted),
            "an agent name is muted"
        );
        assert!(
            th::paints_as(
                th::painted_style(&lines, 100, "↑↓/jk select · enter inspect · esc back"),
                Role::Dim
            ),
            "the key hint is dim"
        );
        // The state suffix rides an UNSTYLED span (`fleet-status.ts:452`), not the agent's muted
        // one — a distinction no text assertion can make.
        let state = th::painted_style(&lines, 100, "running");
        assert_eq!(state.fg, Some(ratatui::style::Color::Reset));
    }

    #[test]
    fn detaching_the_ui_clears_every_piece_of_registration_state() {
        let busy = FleetState {
            foreground_controls: vec![control("a", "coder", 0)],
            ..FleetState::default()
        };
        let mut widget = armed_widget(&busy);
        widget.set_ui_available(false);
        assert!(!widget.is_widget_registered());
        assert!(widget.entries().is_empty());
        assert_eq!(widget.selected_key(), "main");
        assert!(!widget.refresh(&busy, 10_000));
    }
}
