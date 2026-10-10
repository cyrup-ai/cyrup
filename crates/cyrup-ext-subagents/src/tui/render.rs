//! Pure fold-to-aggregate rendering functions (func-SA §5.5; arch-SA §6.7): nested/indented
//! subagent output, a fork-badge helper, and activity-glyph gating for a running-vs-idle
//! indicator.
//!
//! # Why this module is pure
//!
//! Every function here is `data in -> Vec<Line<'static>> out` — no I/O, no locking, no clock
//! reads beyond a caller-supplied `tick` counter, no dependency on a live terminal or a
//! `cyrup-tui` type. This is deliberate, not incidental: arch-SA §6.7 requires the nested-render
//! fold to be "a pure function over `&[NestedRunSummary]` returning renderable lines, callable
//! from both the persistent background-widget renderer and the foreground inline tool-result
//! renderer" (R-SA-112/113), and the crate-boundary rule restated throughout this crate's docs
//! (`tui/mod.rs`, `fork_context.rs`) is that `cyrup-ext-subagents` never depends on `cyrup-tui`
//! directly — this crate emits renderable [`ratatui::text::Line`] values through the ordinary
//! session-event sink, and whichever crate owns the live terminal (`cyrup-tui`, out of scope
//! here) is responsible for actually painting them. Keeping these functions pure and terminal-
//! free is exactly what makes them unit-testable via
//! `cyrup_test_support::tui::TestTerminal` without a live terminal, per this phase's own
//! testing instructions.
//!
//! # Requirement coverage
//!
//! - **R-SA-106** (live re-render on every foreground event) and **R-SA-107** (persistent
//!   background-progress region) are satisfied by these functions being cheap, deterministic,
//!   pure folds — the *caller* (a later phase's live event-consumer loop, not this file) is
//!   responsible for actually invoking [`render_progress_header`]/[`render_nested_children`] on
//!   every observed event/poll tick; this module supplies the render primitive, not the
//!   re-render trigger.
//! - **R-SA-108** (bounded detail, fold overflow to summary) is realized by
//!   [`render_background_region`], which caps the number of fully-detailed top-level runs shown
//!   at [`MAX_DETAILED_RUNS`] and folds the remainder into one aggregate suffix line.
//! - **R-SA-109** (activity glyph stops when not running) is realized by
//!   [`activity_glyph`]/[`is_actively_running`] — the glyph is gated strictly on
//!   `background::RunState::Running`, never animated for `Paused`/`Complete`/`Failed`/`Queued`.
//! - **R-SA-110/111** (fork badge presence, reflects *resolved* not *requested* context) is
//!   realized by [`fork_badge_span`]/[`fork_badge_text`], which take
//!   [`crate::fork_context::ContextMode`] — always the *resolved* value per that module's own
//!   contract (`tui/mod.rs`'s `SubagentProgressSnapshot::context` doc), never a raw caller
//!   request — as their only input.
//! - **R-SA-112** (nested fanout rendered indented, depth-capped at 2) and **R-SA-113** (overflow
//!   folds to one aggregate suffix line, never silently truncated or unbounded) are realized by
//!   [`render_nested_children`]/[`fold_nested_summaries`].

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use std::collections::HashMap;

use crate::background::{RunId, RunMode, RunState, StepState};
use crate::fork_context::ContextMode;
use crate::tui::events::{AsyncJobSnapshot, WorkflowLaneRow, WorkflowLaneState};
use crate::tui::{NestedRunSummary, SubagentProgressSnapshot};

// =================================================================================================
// Tunable render constants
// =================================================================================================

/// Maximum recursion depth for nested-fanout rendering (R-SA-112 target: 2 — a top-level run's
/// direct children render at depth 1, grandchildren at depth 2; anything at or beyond depth 2's
/// *own* children collapses into one aggregate line rather than recursing further). Kept as a
/// crate-local constant (not sourced from [`crate::registration::SubagentExtensionConfig`]) since
/// R-SA-112 fixes this as a renderer property, not a user-configurable knob.
pub const MAX_NESTED_DEPTH: usize = 2;

/// Maximum number of sibling entries rendered in full at any one nesting level before the
/// remainder folds into a single aggregate suffix line (R-SA-113). Applies independently at every
/// level of the recursion, not just the top.
pub const MAX_CHILDREN_PER_LEVEL: usize = 5;

/// Maximum number of top-level background runs shown fully detailed in the persistent
/// background-progress region before the remainder folds into one compact summary line
/// (R-SA-108 target: 4).
pub const MAX_DETAILED_RUNS: usize = 4;

/// Braille spinner frames for the activity glyph (R-SA-109). A crate-local copy — deliberately
/// not sourced from `cyrup_tui::SPINNER_FRAMES` (which is the identical sequence,
/// `crates/cyrup-tui/src/status_indicator.rs:28`) because this crate has zero dependency on
/// `cyrup-tui` (arch-SA §1.1/§6.1): duplicating this small, stable, purely-cosmetic constant is
/// the correct trade against introducing a crate dependency solely for ten characters.
pub const ACTIVITY_GLYPH_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The static glyph shown for a non-running (terminal or paused) entry in place of the animated
/// spinner (R-SA-109: the glyph MUST stop animating, not merely freeze on an arbitrary frame).
pub const IDLE_GLYPH: &str = "•";

/// The fork-badge text appended to a run header whose resolved context is
/// [`ContextMode::Fork`] (R-SA-110).
pub const FORK_BADGE_TEXT: &str = "[fork]";

// =================================================================================================
// Activity glyph (R-SA-109)
// =================================================================================================

/// Whether a run in the given lifecycle state counts as "actively running" for activity-glyph
/// purposes (R-SA-109). Only [`RunState::Running`] is active — [`RunState::Queued`] has not
/// started real work yet, and [`RunState::Paused`]/[`RunState::Complete`]/[`RunState::Failed`]
/// are all non-animating per this requirement (a queued run has no activity to animate either;
/// it renders with the idle glyph until it actually starts).
#[must_use]
pub fn is_actively_running(state: RunState) -> bool {
    matches!(state, RunState::Running)
}

/// Selects the activity glyph for a run in the given lifecycle state at the given render tick
/// (R-SA-109). `tick` is an opaque, caller-owned monotonic counter (e.g. an 80ms-interval phase
/// index, mirroring `cyrup-tui`'s own `SPINNER_INTERVAL` cadence) — this function performs no
/// clock reads of its own, keeping it pure and deterministic for a given `(state, tick)` pair.
///
/// Returns [`IDLE_GLYPH`] for every non-[`RunState::Running`] state, satisfying R-SA-109's "MUST
/// stop animating" clause: a caller that stops advancing `tick` for a terminal run still renders
/// a fixed, non-spinning glyph rather than an animated one frozen mid-spin.
#[must_use]
pub fn activity_glyph(state: RunState, tick: usize) -> &'static str {
    if is_actively_running(state) {
        let idx = tick % ACTIVITY_GLYPH_FRAMES.len();
        ACTIVITY_GLYPH_FRAMES
            .get(idx)
            .copied()
            .unwrap_or(IDLE_GLYPH)
    } else {
        IDLE_GLYPH
    }
}

/// The [`Style`] the activity glyph renders with — accent-colored and bold while running, dimmed
/// once idle, so a plain-text-only assertion (no color inspection) still has a distinct glyph to
/// key off, while a color-aware test/renderer gets a visibly different treatment too.
#[must_use]
pub fn activity_glyph_style(state: RunState) -> Style {
    if is_actively_running(state) {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    }
}

// =================================================================================================
// Fork badge (R-SA-110/111)
// =================================================================================================

/// Builds the fork-badge [`Span`], if any, for a run whose *resolved* context mode is `context`
/// (R-SA-110/111). Returns `None` for [`ContextMode::Fresh`] (no badge — R-SA-110's "runs with
/// `context: Fresh` MUST NOT show it"); `Some` for [`ContextMode::Fork`].
///
/// Callers MUST source `context` from the run's actually-*resolved* context
/// (`SubagentProgressSnapshot::context`, itself sourced from
/// `fork_context::ForkContextResolver::resolve`'s output per that field's own doc) — never from
/// whatever the caller requested at the call site, satisfying R-SA-111 even when `context` was
/// omitted by the caller and independently resolved per-agent (DI-SA-3).
#[must_use]
pub fn fork_badge_span(context: ContextMode) -> Option<Span<'static>> {
    match context {
        ContextMode::Fresh => None,
        ContextMode::Fork => Some(Span::styled(
            format!(" {FORK_BADGE_TEXT}"),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )),
    }
}

/// The plain-text form of the fork badge, if any — `""` for [`ContextMode::Fresh`], `" [fork]"`
/// for [`ContextMode::Fork`]. A convenience wrapper over [`fork_badge_span`] for call sites that
/// only need the text (e.g. building a plain [`String`] header rather than a styled [`Span`]).
#[must_use]
pub fn fork_badge_text(context: ContextMode) -> &'static str {
    match context {
        ContextMode::Fresh => "",
        ContextMode::Fork => " [fork]",
    }
}

// =================================================================================================
// Run-header line (activity glyph + agent/run-id + fork badge)
// =================================================================================================

/// Renders one run's header line: `{activity-glyph} {agent-or-run-id}{fork-badge}` — the
/// single shared building block every other function in this module composes with indentation
/// (R-SA-112) to build a full nested tree, and that the top-level foreground/background regions
/// (R-SA-106/107) use directly for a non-nested single run.
///
/// `label` is the human-facing identifier to show (an agent name for
/// [`NestedRunSummary`]/[`SubagentProgressSnapshot`], which both carry one) — kept as a plain
/// `&str` parameter rather than requiring a specific snapshot type so this one function serves
/// every caller shape in this module.
#[must_use]
pub fn render_run_header_line(
    label: &str,
    state: RunState,
    context: ContextMode,
    tick: usize,
) -> Line<'static> {
    let glyph = activity_glyph(state, tick);
    let glyph_style = activity_glyph_style(state);
    let mut spans = vec![
        Span::styled(glyph.to_string(), glyph_style),
        Span::raw(" "),
        Span::raw(label.to_string()),
    ];
    if let Some(badge) = fork_badge_span(context) {
        spans.push(badge);
    }
    Line::from(spans)
}

// =================================================================================================
// Nested fold (R-SA-112/113)
// =================================================================================================

/// Recursively renders `summaries` as indented [`Line`]s, capped at [`MAX_NESTED_DEPTH`] levels
/// and [`MAX_CHILDREN_PER_LEVEL`] siblings per level (R-SA-112/113). `tick` drives the activity
/// glyph (R-SA-109) uniformly across every nested entry; `depth` is the caller's current
/// indentation depth (top-level callers pass `0`).
///
/// - Each rendered entry indents two spaces per `depth` beyond the top level, so children of the
///   top-level run appear indented under their parent, never flattened into the top-level list
///   (R-SA-112's "MUST be rendered visually indented/nested under their parent step's entry, not
///   flattened").
/// - Once `depth >= MAX_NESTED_DEPTH`, no further recursion occurs even if a summary at that
///   depth has its own non-empty `children` — instead, if that summary has children, its own line
///   is followed immediately by one aggregate line summarizing the collapsed subtree size
///   (R-SA-112's "grandchild-of-grandchild and deeper collapses to an aggregate summary line").
/// - Within any one level, at most [`MAX_CHILDREN_PER_LEVEL`] siblings render in full; any
///   remaining siblings at that level fold into one aggregate suffix line rather than being
///   silently truncated or allowed to grow the region unbounded (R-SA-113).
#[must_use]
pub fn render_nested_children(
    summaries: &[NestedRunSummary],
    depth: usize,
    tick: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    fold_nested_into(summaries, depth, tick, &mut out);
    out
}

/// Same fold as [`render_nested_children`], but returns the accumulated lines directly rather
/// than appending to a caller-supplied buffer — the ergonomic entry point most callers want; kept
/// as a distinct name so the internal accumulator-style helper ([`fold_nested_into`]) can recurse
/// without repeated `Vec` reallocation/concatenation at each level.
#[must_use]
pub fn fold_nested_summaries(summaries: &[NestedRunSummary], tick: usize) -> Vec<Line<'static>> {
    render_nested_children(summaries, 0, tick)
}

fn fold_nested_into(
    summaries: &[NestedRunSummary],
    depth: usize,
    tick: usize,
    out: &mut Vec<Line<'static>>,
) {
    if summaries.is_empty() {
        return;
    }

    let indent = "  ".repeat(depth.saturating_add(1));
    let (visible, overflow) = split_at_budget(summaries, MAX_CHILDREN_PER_LEVEL);

    for child in visible {
        let mut header =
            render_run_header_line(&child.agent, child.status.state, child_context(child), tick);
        prepend_indent(&mut header, &indent);
        out.push(header);

        if child.children.is_empty() {
            continue;
        }

        if depth.saturating_add(1) >= MAX_NESTED_DEPTH {
            // Depth cap reached (R-SA-112): collapse this child's own subtree into one aggregate
            // line rather than recursing further, no matter how deep it actually goes.
            let count = count_subtree(&child.children);
            out.push(aggregate_line(&indent, count));
        } else {
            fold_nested_into(&child.children, depth.saturating_add(1), tick, out);
        }
    }

    if !overflow.is_empty() {
        out.push(aggregate_line(&indent, overflow.len()));
    }
}

/// Splits `items` into `(visible, overflow)` where `visible` is at most `budget` long. Pure
/// slicing helper kept separate so [`fold_nested_into`]'s main body stays readable — never
/// panics: `budget.min(items.len())` is always a valid split point.
fn split_at_budget<T>(items: &[T], budget: usize) -> (&[T], &[T]) {
    let split = budget.min(items.len());
    items.split_at(split)
}

/// Counts every summary in `summaries` plus all of their descendants, recursively — used to
/// report an accurate collapsed-subtree size in the depth-cap aggregate line (R-SA-112) rather
/// than just the immediate child count.
fn count_subtree(summaries: &[NestedRunSummary]) -> usize {
    summaries
        .iter()
        .map(|s| 1 + count_subtree(&s.children))
        .sum()
}

/// Builds one aggregate "+N more" suffix line at the given indent (R-SA-113/112's overflow-fold
/// contract) — always a single line, regardless of how large `count` is, so the rendered region
/// never grows unbounded from overflow alone.
fn aggregate_line(indent: &str, count: usize) -> Line<'static> {
    let noun = if count == 1 { "run" } else { "runs" };
    Line::from(vec![
        Span::raw(indent.to_string()),
        Span::styled(
            format!("… +{count} more {noun}"),
            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
    ])
}

/// Re-indents an already-built [`Line`] by prepending `indent` as a leading raw [`Span`] — kept
/// as a small helper so [`render_run_header_line`] itself stays indent-agnostic and reusable by
/// non-nested (top-level, zero-indent) callers.
fn prepend_indent(line: &mut Line<'static>, indent: &str) {
    if indent.is_empty() {
        return;
    }
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(indent.to_string()));
    spans.append(&mut line.spans);
    line.spans = spans;
}

/// [`NestedRunSummary`] carries no context-mode field of its own (arch-SA §3.7: it is
/// deliberately narrower than [`SubagentProgressSnapshot`], carrying only enough state for one
/// summary line). Nested fanout children are, in this port's design, always spawned by their
/// parent step's own resolved context decision rather than independently re-resolving
/// fork/fresh, so there is no per-child resolved [`ContextMode`] to surface here; nested entries
/// therefore never render a fork badge of their own — only top-level run headers do
/// ([`render_progress_header`]). This is a deliberate, narrow scope decision for this render
/// module, not a gap: adding a resolved-context field to `NestedRunSummary` (if a future
/// requirement needs per-child fork badges) is a `tui/mod.rs` data-model change owned by that
/// module, not something this pure-render file can or should paper over by guessing.
fn child_context(_child: &NestedRunSummary) -> ContextMode {
    ContextMode::Fresh
}

// =================================================================================================
// Top-level progress header + background region (R-SA-106/107/108)
// =================================================================================================

/// Renders the full header + nested-children block for one top-level run snapshot: the run's own
/// header line (activity glyph, agent name, fork badge per R-SA-109/110/111) followed by its
/// fold-to-aggregate nested children (R-SA-112/113).
///
/// This is the single entry point both the inline foreground tool-result renderer (R-SA-106) and
/// the persistent background-progress region (R-SA-107) call for one run's full block — the only
/// difference between those two call sites is which region of the terminal the caller places the
/// returned lines into, not anything about how the lines themselves are built.
#[must_use]
pub fn render_progress_header(
    snapshot: &SubagentProgressSnapshot,
    tick: usize,
) -> Vec<Line<'static>> {
    let label = snapshot
        .current_agent
        .as_deref()
        .unwrap_or(snapshot.run_id.as_ref());
    let mut lines = vec![render_run_header_line(
        label,
        snapshot.status.state,
        snapshot.context,
        tick,
    )];
    lines.extend(render_nested_children(&snapshot.children, 0, tick));
    lines
}

/// Renders the persistent background-progress region for a set of tracked runs (R-SA-107): up to
/// [`MAX_DETAILED_RUNS`] runs render fully detailed (header + nested children), and any remaining
/// tracked runs beyond that cap fold into one compact aggregate summary line rather than growing
/// the region unboundedly (R-SA-108).
///
/// `snapshots` is taken in caller-supplied order (typically spawn order or most-recently-active
/// first — this function imposes no reordering of its own, matching every other fold in this
/// module's "never reorder, only cap/fold" discipline). Pure and deterministic for a given
/// `(snapshots, tick)` pair, so it composes cleanly with a caller's own render-tick scheduling
/// (R-SA-144's "no more than one extra render pass per NDJSON event" cadence discipline lives in
/// that caller, not here).
#[must_use]
pub fn render_background_region(
    snapshots: &[SubagentProgressSnapshot],
    tick: usize,
) -> Vec<Line<'static>> {
    if snapshots.is_empty() {
        return Vec::new();
    }

    let (detailed, overflow) = split_at_budget(snapshots, MAX_DETAILED_RUNS);
    let mut out = Vec::new();
    for snapshot in detailed {
        out.extend(render_progress_header(snapshot, tick));
    }
    if !overflow.is_empty() {
        let running = overflow
            .iter()
            .filter(|s| is_actively_running(s.status.state))
            .count();
        let noun = if overflow.len() == 1 { "run" } else { "runs" };
        out.push(Line::from(vec![Span::styled(
            format!(
                "… +{} more {noun} tracked ({running} running)",
                overflow.len()
            ),
            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
        )]));
    }
    out
}

/// Flattens a rendered [`Vec<Line>`] to plain text, one row per line, with no trailing styling
/// information — a small convenience for callers/tests that want a plain-text assertion surface
/// without going through a full [`ratatui::backend::TestBackend`] paint. Every function in this
/// module is designed to be equally testable either way (plain-text via this helper, or grid-
/// painted via `cyrup_test_support::tui::TestTerminal`), per this phase's own testing
/// instructions.
#[must_use]
pub fn lines_to_plain_text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

// =================================================================================================
// Progressive async-jobs widget tier (SUBA-162 — pi `src/tui/render.ts:2516-2820` @ad11b7ab)
// =================================================================================================
//
// pi's mounted async widget picks one of three tiers per render (`fitAdaptiveWidgetLines`,
// `:2765-2820`): the FULL block when it fits, a one-line card when the terminal is too short, and
// otherwise a PROGRESSIVE card — a summary header plus one line per visible job — whose height is
// LOCKED so the editor below it does not jump on every progress tick. Upstream holds the lock in
// module state (`let widgetLayoutSession`, `:2530`). Here it is a `&mut Option<WidgetLayoutSession>`
// the caller owns and threads through every call, so this module stays `data in -> lines out`:
// state in, state out, no `static`.

/// pi `RESERVED_NON_WIDGET_ROWS` (`render.ts:2528` @ad11b7ab): the terminal rows the editor, footer
/// and transcript keep for themselves before the widget may claim any.
pub const RESERVED_NON_WIDGET_ROWS: usize = 19;

/// pi `process.stdout.rows || 30` (`render.ts:2537,2542` @ad11b7ab). \[CYRUP-DELTA] The extension
/// has no terminal-size accessor (`cyrup_ext::host::HostServices` carries none), so the publisher
/// always passes upstream's own fallback.
pub const ASYNC_WIDGET_FALLBACK_ROWS: usize = 30;

/// pi `process.stdout.columns || 120` (`render.ts:2546` @ad11b7ab); see
/// [`ASYNC_WIDGET_FALLBACK_ROWS`].
pub const ASYNC_WIDGET_FALLBACK_COLUMNS: usize = 120;

/// pi `estimateAvailableWidgetRows()` (`render.ts:2536-2539` @ad11b7ab):
/// `max(1, rows - RESERVED_NON_WIDGET_ROWS)`.
#[must_use]
pub fn estimate_available_widget_rows(rows: usize) -> usize {
    rows.saturating_sub(RESERVED_NON_WIDGET_ROWS).max(1)
}

/// pi `collapsedWidgetLineBudget(rows)` (`render.ts:2741-2743` @ad11b7ab):
/// `max(10, min(14, floor(rows * 0.35)))` — the progressive card's cap.
#[must_use]
pub fn collapsed_widget_line_budget(rows: usize) -> usize {
    (rows.saturating_mul(35) / 100).clamp(10, 14)
}

/// pi `AsyncWidgetLayout` (`shared/types.ts:2695` @ad11b7ab, `588d2cfd`/#2738), the
/// `asyncWidgetLayout` config key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AsyncWidgetLayout {
    /// `"adaptive"` (the default): the full block while it fits, the progressive card otherwise.
    #[default]
    Adaptive,
    /// `"rows"`: never the full block — always the progressive card (or the one-line card on a
    /// terminal too short for it), `render.ts:2798`.
    Rows,
}

/// pi `WidgetRenderTier` (`render.ts:2516` @ad11b7ab).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetRenderTier {
    /// The full per-run block ([`render_background_region`]).
    Full,
    /// The one-line card ([`build_single_line_widget_lines`]).
    SingleLine,
    /// The header-plus-job-lines card with a locked height.
    Progressive,
}

/// pi `WidgetLayoutSession` (`render.ts:2518-2526` @ad11b7ab): the tier chosen at lock time and,
/// for the progressive tier, the locked height, the root-job count it was locked against
/// (`8d804895`/#2662) and the sticky visible-job keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetLayoutSession {
    /// pi `expanded`.
    pub expanded: bool,
    /// pi `rows` — the terminal height the session was taken at.
    pub rows: usize,
    /// pi `columns`.
    pub columns: usize,
    /// pi `tier`.
    pub tier: WidgetRenderTier,
    /// pi `lockedRows`.
    pub locked_rows: Option<usize>,
    /// pi `rootJobCount`.
    pub root_job_count: Option<usize>,
    /// pi `visibleJobKeys`.
    pub visible_job_keys: Vec<RunId>,
}

/// The terminal facts pi's widget reads from `process.stdout` and `ui.getToolsExpanded()`
/// (`render.ts:2541-2553,2950` @ad11b7ab), passed in rather than read so this module stays pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AsyncWidgetViewport {
    /// Terminal rows.
    pub rows: usize,
    /// Terminal columns.
    pub columns: usize,
    /// pi `ui.getToolsExpanded()`.
    pub expanded: bool,
}

impl Default for AsyncWidgetViewport {
    fn default() -> Self {
        Self {
            rows: ASYNC_WIDGET_FALLBACK_ROWS,
            columns: ASYNC_WIDGET_FALLBACK_COLUMNS,
            expanded: false,
        }
    }
}

impl WidgetLayoutSession {
    /// pi `widgetSessionMatches(expanded)` (`render.ts:2549-2553` @ad11b7ab).
    fn matches(&self, viewport: &AsyncWidgetViewport) -> bool {
        self.expanded == viewport.expanded
            && self.rows == viewport.rows
            && self.columns == viewport.columns
    }
}

/// pi `widgetJobTree(jobs, now)` (`render.ts:2869-2884` @ad11b7ab): attach each job to its loaded
/// workflow parent, so a workflow's lanes count and render under it; orphans stay top-level, and
/// so does a live job whose parent is not running (`keepLiveRoot`, `:2876`).
///
/// \[CYRUP-DELTA] Degenerate in production today: nothing fills
/// [`AsyncJobSnapshot::parent_workflow_run_id`], so every job is a root.
#[derive(Clone, Debug, Default)]
pub struct WidgetJobTree<'a> {
    /// pi `roots`, in input order.
    pub roots: Vec<&'a AsyncJobSnapshot>,
    children: HashMap<RunId, Vec<&'a AsyncJobSnapshot>>,
    size: usize,
}

impl<'a> WidgetJobTree<'a> {
    /// Build the tree over `jobs`.
    #[must_use]
    pub fn build(jobs: &'a [AsyncJobSnapshot]) -> Self {
        let parents: HashMap<&RunId, &AsyncJobSnapshot> = jobs
            .iter()
            .filter(|job| job.mode == RunMode::Workflow)
            .map(|job| (&job.run_id, job))
            .collect();
        let mut tree = Self {
            roots: Vec::new(),
            children: HashMap::new(),
            size: jobs.len(),
        };
        for job in jobs {
            let parent = job
                .parent_workflow_run_id
                .as_ref()
                .and_then(|id| parents.get(id).copied());
            let keep_live_root = is_progressive_active_job(job)
                && parent.is_none_or(|parent| parent.state != RunState::Running);
            match parent {
                Some(parent) if parent.run_id != job.run_id && !keep_live_root => tree
                    .children
                    .entry(parent.run_id.clone())
                    .or_default()
                    .push(job),
                _ => tree.roots.push(job),
            }
        }
        tree
    }

    /// pi `projectionFor(job).children` — the jobs attached under `job`.
    #[must_use]
    pub fn children_of(&self, job: &AsyncJobSnapshot) -> &[&'a AsyncJobSnapshot] {
        self.children.get(&job.run_id).map_or(&[], Vec::as_slice)
    }
}

/// pi `formatAgentRunningLabel(count)` (`shared/status-format.ts:39-41` @ad11b7ab).
#[must_use]
pub fn format_agent_running_label(count: usize) -> String {
    if count == 1 {
        "1 agent running".to_string()
    } else {
        format!("{count} agents running")
    }
}

/// pi `runningLeafAgentCount(job, projectionFor)` (`render.ts:2634-2648` @ad11b7ab,
/// `7e07a22d`/#2584): the leaf entries the fleet's roster makes for this job, so the widget
/// header agrees with the fleet's `N active agents`
/// ([`crate::tui::fleet_status::active_leaf_agent_count`]).
///
/// - A workflow counts only its loaded child runs, recursively, whatever its own state.
/// - Any other job counts 0 unless it is running.
/// - A running job counts its active steps, synthesised from [`AsyncJobSnapshot::agents`] when it
///   has no step detail; a sequential chain (no live parallel group) excludes every pending step
///   but the current one. With neither steps nor agents it counts 1.
///
/// \[CYRUP-DELTA] cyrup's [`StepState`] has no `queued`, so the active step set is
/// `running | pending`.
#[must_use]
pub fn running_leaf_agent_count(job: &AsyncJobSnapshot, tree: &WidgetJobTree<'_>) -> usize {
    running_leaf_agent_count_within(job, tree, tree.size)
}

/// [`running_leaf_agent_count`] with a recursion budget: a parent link can only form a tree from a
/// root, but this is public, so a cycle handed in directly must still terminate.
fn running_leaf_agent_count_within(
    job: &AsyncJobSnapshot,
    tree: &WidgetJobTree<'_>,
    budget: usize,
) -> usize {
    if job.mode == RunMode::Workflow {
        let Some(budget) = budget.checked_sub(1) else {
            return 0;
        };
        return tree
            .children_of(job)
            .iter()
            .map(|child| running_leaf_agent_count_within(child, tree, budget))
            .sum();
    }
    if job.state != RunState::Running {
        return 0;
    }
    let sequential = job.mode == RunMode::Chain && !job.active_parallel_group;
    let current = job.current_step_index.map_or(0, |index| index as usize);
    let synthesised: Vec<StepState>;
    let steps: &[StepState] = if job.step_states.is_empty() {
        synthesised = (0..job.agents.len())
            .map(|index| {
                if sequential && index != current {
                    StepState::Pending
                } else {
                    StepState::Running
                }
            })
            .collect();
        &synthesised
    } else {
        &job.step_states
    };
    if steps.is_empty() {
        return 1;
    }
    steps
        .iter()
        .enumerate()
        .filter(|(offset, step)| {
            matches!(step, StepState::Running | StepState::Pending)
                && !(**step == StepState::Pending && sequential && *offset != current)
        })
        .count()
}

/// pi `widgetHeaderCounts(jobs)` (`render.ts:2555-2564` @ad11b7ab).
#[derive(Default)]
struct WidgetHeaderCounts {
    running: usize,
    queued: usize,
    complete: usize,
    failed: usize,
    paused: usize,
    stopped: usize,
    partial: usize,
}

impl WidgetHeaderCounts {
    fn of(jobs: &[&AsyncJobSnapshot]) -> Self {
        let mut counts = Self::default();
        for job in jobs {
            let slot = match job.state {
                RunState::Running => &mut counts.running,
                RunState::Queued => &mut counts.queued,
                RunState::Complete => &mut counts.complete,
                RunState::Failed => &mut counts.failed,
                RunState::Paused => &mut counts.paused,
                RunState::Stopped => &mut counts.stopped,
                RunState::Partial => &mut counts.partial,
            };
            *slot = slot.saturating_add(1);
        }
        counts
    }

    fn has_active(&self) -> bool {
        self.running > 0 || self.queued > 0
    }

    /// The header glyph: the spinner while anything runs, `●` while anything is queued, else `○`.
    fn glyph(&self, tick: usize) -> &'static str {
        if self.running > 0 {
            activity_glyph(RunState::Running, tick)
        } else if self.has_active() {
            "●"
        } else {
            "○"
        }
    }
}

/// pi `activeHeaderTone(theme, hasActive)` (`render.ts:1264-1266` @ad11b7ab).
fn header_tone(has_active: bool) -> Style {
    if has_active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        dim()
    }
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// pi `buildSingleLineWidgetLines(jobs)` (`render.ts:2566-2583` @ad11b7ab): the one-line card —
/// `{glyph} subagents ({running}/{total} running, …)`. Used for `asyncWidgetCollapsed`, for a
/// terminal too short for anything else, and for a one-row lock.
#[must_use]
pub fn build_single_line_widget_lines(
    jobs: &[&AsyncJobSnapshot],
    tick: usize,
) -> Vec<Line<'static>> {
    let counts = WidgetHeaderCounts::of(jobs);
    let total = jobs.len();
    let mut parts: Vec<String> = Vec::new();
    if counts.running > 0 {
        parts.push(format!("{}/{total} running", counts.running));
    }
    for (count, word) in [
        (counts.queued, "queued"),
        (counts.failed, "failed"),
        (counts.stopped, "stopped"),
        (counts.paused, "paused"),
        (counts.partial, "partial"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {word}"));
        }
    }
    if !counts.has_active() && counts.complete > 0 {
        parts.push(format!("{}/{total} done", counts.complete));
    }
    let summary = if parts.is_empty() {
        format!("{total} total")
    } else {
        parts.join(", ")
    };
    let tone = header_tone(counts.has_active());
    vec![Line::from(vec![
        Span::styled(counts.glyph(tick).to_string(), tone),
        Span::raw(" "),
        Span::styled("subagents", tone),
        Span::raw(format!(" ({summary})")),
    ])]
}

/// pi `progressiveHeaderLine(jobs, …)` (`render.ts:2650-2666` @ad11b7ab):
/// `{glyph} Async agents · {N agents running}, {M queued}` — or, once nothing is active, the
/// failed/stopped/paused/`done` parts, or `{n} total`. The running part is the LEAF-agent count
/// ([`running_leaf_agent_count`]), not the number of running jobs (`7e07a22d`/#2584).
#[must_use]
pub fn progressive_header_line(
    jobs: &[&AsyncJobSnapshot],
    tree: &WidgetJobTree<'_>,
    tick: usize,
) -> Line<'static> {
    let counts = WidgetHeaderCounts::of(jobs);
    let mut parts: Vec<String> = Vec::new();
    let running_agents: usize = jobs
        .iter()
        .map(|job| running_leaf_agent_count(job, tree))
        .sum();
    if running_agents > 0 {
        parts.push(format_agent_running_label(running_agents));
    }
    if counts.queued > 0 {
        parts.push(format!("{} queued", counts.queued));
    }
    if !counts.has_active() {
        for (count, word) in [
            (counts.failed, "failed"),
            (counts.stopped, "stopped"),
            (counts.paused, "paused"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {word}"));
            }
        }
        if counts.complete > 0 {
            parts.push(format!("{}/{} done", counts.complete, jobs.len()));
        }
    }
    let summary = if parts.is_empty() {
        format!("{} total", jobs.len())
    } else {
        parts.join(", ")
    };
    let tone = header_tone(counts.has_active());
    Line::from(vec![
        Span::styled(counts.glyph(tick).to_string(), tone),
        Span::raw(" "),
        Span::styled("Async agents", tone),
        Span::styled(" · ".to_string(), dim()),
        Span::styled(summary, dim()),
    ])
}

/// pi `isProgressiveActiveJob(job)` (`render.ts:2597-2599` @ad11b7ab).
fn is_progressive_active_job(job: &AsyncJobSnapshot) -> bool {
    matches!(job.state, RunState::Running | RunState::Queued)
}

/// pi `orderedWidgetJobs(jobs)` (`render.ts:2585-2591` @ad11b7ab): running, then queued, then the
/// rest, each in input order.
fn ordered_widget_jobs<'a>(jobs: &[&'a AsyncJobSnapshot]) -> Vec<&'a AsyncJobSnapshot> {
    let rank = |job: &AsyncJobSnapshot| match job.state {
        RunState::Running => 0u8,
        RunState::Queued => 1,
        _ => 2,
    };
    let mut ordered = jobs.to_vec();
    ordered.sort_by_key(|job| rank(job));
    ordered
}

/// pi `selectProgressiveJobKeys(jobs, previousKeys, bodyRows)` (`render.ts:2601-2632` @ad11b7ab):
/// sticky slot filling — previously visible active jobs, then other active jobs, then previously
/// visible finished jobs, then everything else.
fn select_progressive_job_keys(
    jobs: &[&AsyncJobSnapshot],
    previous: &[RunId],
    body_rows: usize,
) -> Vec<RunId> {
    if body_rows == 0 {
        return Vec::new();
    }
    let by_key: HashMap<&RunId, &AsyncJobSnapshot> =
        jobs.iter().map(|job| (&job.run_id, *job)).collect();
    let ordered = ordered_widget_jobs(jobs);
    let active = |key: &RunId| {
        by_key
            .get(key)
            .is_some_and(|job| is_progressive_active_job(job))
    };
    let mut selected: Vec<RunId> = Vec::new();
    let append = |key: &RunId, selected: &mut Vec<RunId>| {
        if !selected.contains(key) && by_key.contains_key(key) {
            selected.push(key.clone());
        }
    };
    for key in previous.iter().filter(|key| active(key)) {
        append(key, &mut selected);
        if selected.len() >= body_rows {
            return selected;
        }
    }
    for job in ordered.iter().filter(|job| is_progressive_active_job(job)) {
        append(&job.run_id, &mut selected);
        if selected.len() >= body_rows {
            return selected;
        }
    }
    for key in previous.iter().filter(|key| !active(key)) {
        append(key, &mut selected);
        if selected.len() >= body_rows {
            return selected;
        }
    }
    for job in &ordered {
        append(&job.run_id, &mut selected);
        if selected.len() >= body_rows {
            break;
        }
    }
    selected
}

/// pi `widgetStatusGlyph(job)` (`render.ts:1268-1275` @ad11b7ab).
fn widget_status_glyph(state: RunState, tick: usize) -> Span<'static> {
    let (glyph, style) = match state {
        RunState::Running => (activity_glyph(state, tick), activity_glyph_style(state)),
        RunState::Queued => ("◦", dim()),
        RunState::Complete => ("✓", Style::default().fg(Color::Green)),
        RunState::Paused | RunState::Stopped => ("■", Style::default().fg(Color::Yellow)),
        RunState::Failed | RunState::Partial => ("✗", Style::default().fg(Color::Red)),
    };
    Span::styled(glyph.to_string(), style)
}

/// pi `widgetJobName(job)` (`render.ts:1139-1145` @ad11b7ab), over the fields cyrup's row has:
/// `parallel`/`chain` by mode, else the agent, else the mode word.
fn widget_job_name(job: &AsyncJobSnapshot) -> String {
    match job.mode {
        RunMode::Parallel => "parallel".to_string(),
        RunMode::Chain => "chain".to_string(),
        RunMode::Single | RunMode::Workflow => job
            .agent
            .clone()
            .unwrap_or_else(|| crate::formatters::run_mode_label(job.mode).to_string()),
    }
}

/// pi `widgetActivity(job)` (`render.ts:1206-1222` @ad11b7ab): the current tool, `N turns`,
/// `N tools`, else a per-state fallback.
///
/// \[CYRUP-DELTA] The row cannot tell an absent turn/tool count from zero (both are `0`), so a
/// count is shown only once it is non-zero; no current path, tool duration or live status line is
/// carried.
fn widget_activity(job: &AsyncJobSnapshot) -> String {
    let mut facts: Vec<String> = Vec::new();
    if let Some(tool) = job.current_tool.as_deref() {
        facts.push(tool.to_string());
    }
    if job.turn_count > 0 {
        facts.push(format!("{} turns", job.turn_count));
    }
    if job.tool_count > 0 {
        facts.push(format!("{} tools", job.tool_count));
    }
    if !facts.is_empty() {
        return facts.join(" · ");
    }
    match job.state {
        RunState::Running => "thinking…",
        RunState::Queued => "queued…",
        RunState::Paused => "Paused",
        RunState::Stopped => "Stopped",
        RunState::Partial => "Partial",
        RunState::Failed => "Failed",
        RunState::Complete => "Done",
    }
    .to_string()
}

/// pi `progressiveJobLine(job, …)` (`render.ts:2668-2694` @ad11b7ab), non-workflow branch:
/// `  {glyph} {name}{fork badge} · {status} · {activity}`, with `complete` shown as `done` and the
/// activity dropped when it only repeats the status.
///
/// \[CYRUP-DELTA] No lane signals, lane summary or stats (their projections are not ported), and a
/// workflow job renders through this same line rather than `compactWorkflowHeaderLine`.
fn progressive_job_line(job: &AsyncJobSnapshot, tick: usize) -> Line<'static> {
    let status = match job.state {
        RunState::Complete => "done",
        state => crate::background::run_status::run_state_label(state),
    };
    let activity = widget_activity(job);
    let mut spans = vec![
        Span::raw("  "),
        widget_status_glyph(job.state, tick),
        Span::raw(" "),
        Span::styled(
            widget_job_name(job),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(badge) = fork_badge_span(job.context) {
        spans.push(badge);
    }
    spans.push(Span::styled(" · ".to_string(), dim()));
    spans.push(Span::styled(status.to_string(), dim()));
    if activity.to_lowercase() != status {
        spans.push(Span::styled(" · ".to_string(), dim()));
        spans.push(Span::styled(activity, dim()));
    }
    Line::from(spans)
}

/// pi `compactWorkflowLaneLine(row, theme, "    ")` (`render.ts:1506-1516` @ad11b7ab), with
/// `workflowChecklistGlyph` (`:1302-1309`): `    {glyph} {key} · {agent} · {state}`, where a
/// complete lane shows no state and a running one shows `active`.
fn workflow_lane_line(row: &WorkflowLaneRow, tick: usize) -> Line<'static> {
    let (glyph, style) = match row.state {
        WorkflowLaneState::Running => (
            activity_glyph(RunState::Running, tick),
            activity_glyph_style(RunState::Running),
        ),
        WorkflowLaneState::Complete => ("✓", Style::default().fg(Color::Green)),
        WorkflowLaneState::Blocked => ("!", Style::default().fg(Color::Red)),
        WorkflowLaneState::Failed => ("✗", Style::default().fg(Color::Red)),
        WorkflowLaneState::Paused | WorkflowLaneState::Stopped => {
            ("■", Style::default().fg(Color::Yellow))
        }
        WorkflowLaneState::Queued => ("◦", dim()),
    };
    let state = match row.state {
        WorkflowLaneState::Complete => None,
        WorkflowLaneState::Running => Some("active"),
        WorkflowLaneState::Queued => Some("queued"),
        WorkflowLaneState::Blocked => Some("blocked"),
        WorkflowLaneState::Failed => Some("failed"),
        WorkflowLaneState::Paused => Some("paused"),
        WorkflowLaneState::Stopped => Some("stopped"),
    };
    let mut spans = vec![
        Span::raw("    "),
        Span::styled(glyph.to_string(), style),
        Span::raw(" "),
        Span::styled(
            row.key.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(agent) = row.agent.as_deref() {
        spans.push(Span::styled(format!(" · {agent}"), dim()));
    }
    if let Some(state) = state {
        spans.push(Span::styled(format!(" · {state}"), dim()));
    }
    Line::from(spans)
}

/// pi `progressiveHiddenLine(hiddenJobs)` (`render.ts:2696-2704` @ad11b7ab):
/// `  +N more (R running, Q queued, F finished)`, where finished is complete + failed + paused +
/// stopped (partial is not, as upstream).
fn progressive_hidden_line(hidden: &[&AsyncJobSnapshot]) -> Line<'static> {
    let counts = WidgetHeaderCounts::of(hidden);
    let mut parts: Vec<String> = Vec::new();
    if counts.running > 0 {
        parts.push(format!("{} running", counts.running));
    }
    if counts.queued > 0 {
        parts.push(format!("{} queued", counts.queued));
    }
    let finished = counts
        .complete
        .saturating_add(counts.failed)
        .saturating_add(counts.paused)
        .saturating_add(counts.stopped);
    if finished > 0 {
        parts.push(format!("{finished} finished"));
    }
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    };
    Line::from(vec![Span::styled(
        format!("  +{} more{suffix}", hidden.len()),
        dim(),
    )])
}

/// One progressive render: pi `buildProgressiveWidgetLines`' return (`render.ts:2706` @ad11b7ab).
struct ProgressiveRender {
    lines: Vec<Line<'static>>,
    visible_job_keys: Vec<RunId>,
    content_rows: usize,
}

/// pi `buildProgressiveWidgetLines(jobs, …, lockedRows, previousKeys, …)` (`render.ts:2706-2739`
/// @ad11b7ab): the header, the sticky visible job lines, the visible workflows' lane rows in the
/// rows left over (`9a5a2d5e`/#2583), then the `+N more` line — padded with `" "` rows to
/// `locked_rows`, with `content_rows` the rows the content itself fills.
fn build_progressive_widget_lines(
    jobs: &[&AsyncJobSnapshot],
    tree: &WidgetJobTree<'_>,
    locked_rows: usize,
    previous_keys: &[RunId],
    tick: usize,
) -> ProgressiveRender {
    let row_count = locked_rows.max(1);
    if row_count == 1 {
        return ProgressiveRender {
            lines: build_single_line_widget_lines(jobs, tick),
            visible_job_keys: Vec::new(),
            content_rows: 1,
        };
    }
    let body_rows = row_count - 1;
    let mut visible_job_keys = select_progressive_job_keys(jobs, previous_keys, body_rows);
    let hidden_of = |keys: &[RunId]| -> Vec<&AsyncJobSnapshot> {
        jobs.iter()
            .copied()
            .filter(|job| !keys.contains(&job.run_id))
            .collect()
    };
    let mut hidden = hidden_of(&visible_job_keys);
    if !hidden.is_empty() && visible_job_keys.len() >= body_rows {
        visible_job_keys.truncate(body_rows - 1);
        hidden = hidden_of(&visible_job_keys);
    }
    let visible: Vec<&AsyncJobSnapshot> = visible_job_keys
        .iter()
        .filter_map(|key| jobs.iter().copied().find(|job| &job.run_id == key))
        .collect();
    // Rows left after every visible job line show the visible workflows' lanes (#2583).
    let mut spare_rows = row_count
        .saturating_sub(1)
        .saturating_sub(visible.len())
        .saturating_sub(usize::from(!hidden.is_empty()));
    let mut lines = vec![progressive_header_line(jobs, tree, tick)];
    for job in &visible {
        lines.push(progressive_job_line(job, tick));
        if job.mode != RunMode::Workflow {
            continue;
        }
        let shown = job.workflow_lanes.len().min(spare_rows);
        lines.extend(
            job.workflow_lanes
                .iter()
                .take(shown)
                .map(|row| workflow_lane_line(row, tick)),
        );
        spare_rows -= shown;
    }
    if !hidden.is_empty() && lines.len() < row_count {
        lines.push(progressive_hidden_line(&hidden));
    }
    let content_rows = lines.len().min(row_count);
    lines.resize_with(row_count, || Line::from(" "));
    ProgressiveRender {
        lines,
        visible_job_keys,
        content_rows,
    }
}

/// The lines of `rendered`, cut to `rows`.
fn first_rows(mut lines: Vec<Line<'static>>, rows: usize) -> Vec<Line<'static>> {
    lines.truncate(rows);
    lines
}

/// pi `fitAdaptiveWidgetLines(jobs, buildLines, …, expanded, frame, projectionFor, layout)`
/// (`render.ts:2765-2820` @ad11b7ab), with the layout session as a parameter instead of module
/// state.
///
/// 1. Expanded: drop the session and return the full lines.
/// 2. A matching one-line session stays one line.
/// 3. A matching progressive session keeps its LOCKED height across content-only updates (#186);
///    it GROWS, up to the compact cap, when a job would otherwise be hidden; and it SHRINKS to
///    content only when root jobs leave (`8d804895`/#2662), so a finished job that is still listed
///    keeps its row.
/// 4. [`AsyncWidgetLayout::Adaptive`]: the full lines while they fit the available rows.
/// 5. Two or fewer available rows: the one-line card.
/// 6. Otherwise lock a fresh progressive card to the rows its content fills at the cap.
///
/// `jobs` are the tree's roots. \[CYRUP-DELTA] The full tier is returned as built: pi's
/// `fitWidgetLineBudget` truncation is not ported (cyrup's full block is at most
/// [`MAX_DETAILED_RUNS`] + 1 lines, under every budget it could apply), nor is the single-job
/// `stageProgress` exception (no stage projection is carried).
pub fn fit_adaptive_widget_lines(
    jobs: &[&AsyncJobSnapshot],
    tree: &WidgetJobTree<'_>,
    full_lines: impl FnOnce() -> Vec<Line<'static>>,
    viewport: &AsyncWidgetViewport,
    layout: AsyncWidgetLayout,
    session: &mut Option<WidgetLayoutSession>,
    tick: usize,
) -> Vec<Line<'static>> {
    if viewport.expanded {
        *session = None;
        return full_lines();
    }
    let available_rows = estimate_available_widget_rows(viewport.rows);
    let cap_rows = available_rows.min(collapsed_widget_line_budget(viewport.rows));
    let fresh = |tier: WidgetRenderTier| WidgetLayoutSession {
        expanded: viewport.expanded,
        rows: viewport.rows,
        columns: viewport.columns,
        tier,
        locked_rows: None,
        root_job_count: None,
        visible_job_keys: Vec::new(),
    };

    if let Some(current) = session.as_mut().filter(|current| current.matches(viewport)) {
        match (current.tier, current.locked_rows) {
            (WidgetRenderTier::SingleLine, _) => {
                return build_single_line_widget_lines(jobs, tick);
            }
            (WidgetRenderTier::Progressive, Some(locked_rows)) => {
                let mut rendered = build_progressive_widget_lines(
                    jobs,
                    tree,
                    locked_rows,
                    &current.visible_job_keys,
                    tick,
                );
                let mut next_locked = locked_rows;
                if rendered.visible_job_keys.len() < jobs.len() && locked_rows < cap_rows {
                    rendered = build_progressive_widget_lines(
                        jobs,
                        tree,
                        cap_rows,
                        &current.visible_job_keys,
                        tick,
                    );
                    next_locked = locked_rows.max(rendered.content_rows);
                } else if jobs.len() < current.root_job_count.unwrap_or(0) {
                    next_locked = rendered.content_rows;
                }
                current.locked_rows = Some(next_locked);
                current.root_job_count = Some(jobs.len());
                current.visible_job_keys = rendered.visible_job_keys;
                return first_rows(rendered.lines, next_locked);
            }
            _ => {}
        }
    }

    if layout == AsyncWidgetLayout::Adaptive {
        let lines = full_lines();
        if lines.len() <= available_rows {
            *session = Some(fresh(WidgetRenderTier::Full));
            return lines;
        }
    }

    if available_rows <= 2 {
        *session = Some(fresh(WidgetRenderTier::SingleLine));
        return build_single_line_widget_lines(jobs, tick);
    }

    // Lock to the rows the content fills so the fixed-height card has no blank padding.
    let rendered = build_progressive_widget_lines(jobs, tree, cap_rows, &[], tick);
    let locked_rows = rendered.content_rows;
    *session = Some(WidgetLayoutSession {
        locked_rows: Some(locked_rows),
        root_job_count: Some(jobs.len()),
        visible_job_keys: rendered.visible_job_keys,
        ..fresh(WidgetRenderTier::Progressive)
    });
    first_rows(rendered.lines, locked_rows)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use std::time::Instant;

    use ratatui::Frame;
    use ratatui::layout::Rect;
    use ratatui::widgets::{Paragraph, Widget};

    use super::*;
    use crate::background::{RunId, RunMode, RunState, RunStatus};

    fn status(state: RunState) -> RunStatus {
        RunStatus::queued(RunId::new(), RunMode::Single, Some(1234)).with_state_for_test(state)
    }

    // `RunStatus` enforces monotone-forward transitions through `advance()`; tests need to land
    // directly on an arbitrary state without walking the whole transition graph, so this test
    // module builds statuses via a tiny local constructor rather than fighting the production
    // transition guard (which is doing its job correctly elsewhere).
    trait TestStatusExt {
        fn with_state_for_test(self, state: RunState) -> RunStatus;
    }
    impl TestStatusExt for RunStatus {
        fn with_state_for_test(mut self, state: RunState) -> RunStatus {
            self.state = state;
            self
        }
    }

    fn nested(agent: &str, state: RunState, children: Vec<NestedRunSummary>) -> NestedRunSummary {
        NestedRunSummary {
            run_id: RunId::new(),
            agent: agent.to_string(),
            status: status(state),
            children,
        }
    }

    fn snapshot(
        agent: &str,
        state: RunState,
        context: ContextMode,
        children: Vec<NestedRunSummary>,
    ) -> SubagentProgressSnapshot {
        SubagentProgressSnapshot {
            run_id: RunId::new(),
            mode: RunMode::Single,
            context,
            source: crate::tui::RunSource::Foreground,
            status: status(state),
            current_agent: Some(agent.to_string()),
            current_step_index: None,
            total_steps: None,
            current_tool: None,
            turn_count: 0,
            tool_count: 0,
            recent_output: None,
            children,
            last_activity_at: Instant::now(),
        }
    }

    // ---- R-SA-109: activity glyph gating ----

    #[test]
    fn activity_glyph_animates_only_while_running() {
        let running_frame_0 = activity_glyph(RunState::Running, 0);
        let running_frame_1 = activity_glyph(RunState::Running, 1);
        assert_ne!(
            running_frame_0, running_frame_1,
            "running glyph must advance across ticks"
        );
        assert_eq!(running_frame_0, ACTIVITY_GLYPH_FRAMES[0]);
        assert_eq!(running_frame_1, ACTIVITY_GLYPH_FRAMES[1]);
    }

    #[test]
    fn activity_glyph_is_static_idle_for_every_non_running_state() {
        for state in [
            RunState::Queued,
            RunState::Paused,
            RunState::Complete,
            RunState::Failed,
        ] {
            let g0 = activity_glyph(state, 0);
            let g7 = activity_glyph(state, 7);
            assert_eq!(g0, IDLE_GLYPH, "state {state:?} must render the idle glyph");
            assert_eq!(g0, g7, "state {state:?} must not animate across ticks");
        }
    }

    #[test]
    fn is_actively_running_true_only_for_running() {
        assert!(is_actively_running(RunState::Running));
        assert!(!is_actively_running(RunState::Queued));
        assert!(!is_actively_running(RunState::Paused));
        assert!(!is_actively_running(RunState::Complete));
        assert!(!is_actively_running(RunState::Failed));
    }

    // ---- R-SA-110/111: fork badge ----

    #[test]
    fn fork_badge_absent_for_fresh_context() {
        assert!(fork_badge_span(ContextMode::Fresh).is_none());
        assert_eq!(fork_badge_text(ContextMode::Fresh), "");
    }

    #[test]
    fn fork_badge_present_for_fork_context() {
        let span = fork_badge_span(ContextMode::Fork).expect("fork context must render a badge");
        assert!(span.content.contains("fork"));
        assert_eq!(fork_badge_text(ContextMode::Fork), " [fork]");
    }

    #[test]
    fn fork_badge_reflects_resolved_context_regardless_of_agent_or_state() {
        // R-SA-111: the badge is driven purely by the resolved `ContextMode` passed in, never by
        // any other field — verify it is insensitive to run state/agent identity.
        for state in [
            RunState::Queued,
            RunState::Running,
            RunState::Complete,
            RunState::Failed,
        ] {
            let snap = snapshot("scout", state, ContextMode::Fork, vec![]);
            let header = render_run_header_line(
                snap.current_agent.as_deref().unwrap_or("?"),
                snap.status.state,
                snap.context,
                0,
            );
            let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                text.contains(FORK_BADGE_TEXT),
                "state {state:?} must still show fork badge"
            );
        }
        let fresh_snap = snapshot("scout", RunState::Running, ContextMode::Fresh, vec![]);
        let header =
            render_run_header_line("scout", fresh_snap.status.state, fresh_snap.context, 0);
        let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            !text.contains(FORK_BADGE_TEXT),
            "fresh context must never show fork badge"
        );
    }

    // ---- R-SA-112/113: nested fold, depth cap, overflow aggregation ----

    #[test]
    fn nested_children_render_indented_under_parent() {
        let children = vec![
            nested("worker-a", RunState::Running, vec![]),
            nested("worker-b", RunState::Complete, vec![]),
        ];
        let lines = render_nested_children(&children, 0, 0);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                text.starts_with("  "),
                "child line must be indented: {text:?}"
            );
        }
    }

    #[test]
    fn nested_depth_beyond_cap_collapses_to_aggregate_line() {
        // depth 0 (top) -> depth 1 (child) -> depth 2 (grandchild) -> depth 3 (great-grandchild,
        // beyond MAX_NESTED_DEPTH=2) must collapse at the point where depth reaches the cap.
        let great_grandchild = nested("gg", RunState::Complete, vec![]);
        let grandchild = nested("g", RunState::Complete, vec![great_grandchild]);
        let child = nested("c", RunState::Complete, vec![grandchild]);
        let top = vec![child];

        let lines = render_nested_children(&top, 0, 0);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // Expect: "c" header line, then "g" header line (depth 1 -> within cap), then an
        // aggregate line collapsing everything at/under "g"'s children (the grandchild "gg"
        // subtree), since depth+1 (2) >= MAX_NESTED_DEPTH (2) once we'd otherwise recurse into
        // "g"'s children.
        assert!(
            texts.iter().any(|t| t.contains('c')),
            "expected child 'c' rendered: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains('g') && !t.contains("gg")),
            "expected grandchild 'g' rendered: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("more")),
            "expected an aggregate overflow/collapse line beyond the depth cap: {texts:?}"
        );
        // The deepest literal agent name ("gg") must never appear as its own rendered header —
        // it must have been folded into the aggregate line instead.
        assert!(
            !texts.iter().any(|t| t.trim_start().starts_with("gg")),
            "grandchild-of-grandchild must not get its own line: {texts:?}"
        );
    }

    #[test]
    fn nested_overflow_beyond_per_level_budget_folds_to_one_aggregate_line() {
        let mut children = Vec::new();
        for i in 0..(MAX_CHILDREN_PER_LEVEL + 3) {
            children.push(nested(&format!("worker-{i}"), RunState::Complete, vec![]));
        }
        let lines = render_nested_children(&children, 0, 0);
        // MAX_CHILDREN_PER_LEVEL full lines + exactly one aggregate suffix line.
        assert_eq!(lines.len(), MAX_CHILDREN_PER_LEVEL + 1);
        let last_text: String = lines
            .last()
            .expect("non-empty")
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            last_text.contains("+3 more"),
            "expected '+3 more' aggregate line, got {last_text:?}"
        );
    }

    #[test]
    fn empty_nested_children_render_nothing() {
        let lines = render_nested_children(&[], 0, 0);
        assert!(lines.is_empty());
    }

    #[test]
    fn nested_render_never_grows_unbounded_for_large_flat_fanout() {
        // A pathologically large flat fan-out (R-SA-113: "never allowed to grow the rendered
        // region unbounded") must still collapse to bounded output.
        let children: Vec<_> = (0..500)
            .map(|i| nested(&format!("w{i}"), RunState::Complete, vec![]))
            .collect();
        let lines = render_nested_children(&children, 0, 0);
        assert_eq!(lines.len(), MAX_CHILDREN_PER_LEVEL + 1);
    }

    // ---- R-SA-108: bounded detail + fold-to-summary overflow in the background region ----

    #[test]
    fn background_region_details_up_to_cap_then_folds_overflow() {
        let mut snapshots = Vec::new();
        for i in 0..(MAX_DETAILED_RUNS + 2) {
            snapshots.push(snapshot(
                &format!("agent-{i}"),
                RunState::Running,
                ContextMode::Fresh,
                vec![],
            ));
        }
        let lines = render_background_region(&snapshots, 0);
        // MAX_DETAILED_RUNS header lines + 1 aggregate line (no nested children in this fixture).
        assert_eq!(lines.len(), MAX_DETAILED_RUNS + 1);
        let last_text: String = lines
            .last()
            .expect("non-empty")
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            last_text.contains("+2 more"),
            "expected overflow aggregate, got {last_text:?}"
        );
    }

    #[test]
    fn background_region_empty_when_no_runs_tracked() {
        assert!(render_background_region(&[], 0).is_empty());
    }

    #[test]
    fn background_region_under_cap_shows_every_run_with_no_aggregate_line() {
        let snapshots = vec![
            snapshot("a", RunState::Running, ContextMode::Fresh, vec![]),
            snapshot("b", RunState::Complete, ContextMode::Fork, vec![]),
        ];
        let lines = render_background_region(&snapshots, 0);
        assert_eq!(lines.len(), 2);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(!texts.iter().any(|t| t.contains("more")));
    }

    // ---- R-SA-106/107: header + nested composition, plain-text flattening ----

    #[test]
    fn render_progress_header_composes_own_header_with_nested_children() {
        let snap = snapshot(
            "orchestrated-worker",
            RunState::Running,
            ContextMode::Fork,
            vec![nested("child-1", RunState::Running, vec![])],
        );
        let lines = render_progress_header(&snap, 0);
        assert_eq!(
            lines.len(),
            2,
            "expected parent header + one nested child line"
        );
        let plain = lines_to_plain_text(&lines);
        assert!(plain[0].contains("orchestrated-worker"));
        assert!(plain[0].contains(FORK_BADGE_TEXT));
        assert!(plain[1].contains("child-1"));
        assert!(
            plain[1].starts_with("  "),
            "nested child must be indented under parent"
        );
    }

    // ---- Determinism: same input -> byte-identical output across repeated calls ----

    #[test]
    fn rendering_is_pure_and_deterministic() {
        let snap = snapshot(
            "det",
            RunState::Running,
            ContextMode::Fork,
            vec![nested(
                "c1",
                RunState::Complete,
                vec![nested("c2", RunState::Running, vec![])],
            )],
        );
        let a = lines_to_plain_text(&render_progress_header(&snap, 5));
        let b = lines_to_plain_text(&render_progress_header(&snap, 5));
        assert_eq!(
            a, b,
            "identical input at the same tick must render byte-identical output"
        );
    }

    // ---- Grid-painted assertions via cyrup-test-support's TestBackend wrapper ----
    // Mirrors crates/cyrup-tui/tests/assembled_render.rs's whole-buffer text-grid pattern, scoped
    // down to this module's pure render output rather than a live `App`.

    #[test]
    fn nested_tree_paints_into_a_real_test_backend_grid() {
        let children = vec![
            nested("alpha", RunState::Running, vec![]),
            nested(
                "beta",
                RunState::Complete,
                vec![nested("beta-child", RunState::Complete, vec![])],
            ),
        ];
        let top = snapshot("root", RunState::Running, ContextMode::Fork, children);
        let lines = render_progress_header(&top, 0);

        let mut term = cyrup_test_support::tui::TestTerminal::new(60, 10);
        term.draw(|frame: &mut Frame| {
            let area = Rect::new(0, 0, 60, lines.len() as u16);
            let text: Vec<Line<'static>> = lines.clone();
            Paragraph::new(text).render(area, frame.buffer_mut());
        });

        let grid = term.snapshot();
        assert!(grid.contains("root"), "grid missing root label:\n{grid}");
        assert!(
            grid.contains(FORK_BADGE_TEXT),
            "grid missing fork badge:\n{grid}"
        );
        assert!(
            grid.contains("alpha"),
            "grid missing nested child 'alpha':\n{grid}"
        );
        assert!(
            grid.contains("beta"),
            "grid missing nested child 'beta':\n{grid}"
        );
    }

    #[test]
    fn fresh_context_never_paints_fork_badge_into_grid() {
        let snap = snapshot("plain-run", RunState::Running, ContextMode::Fresh, vec![]);
        let lines = render_progress_header(&snap, 0);

        let mut term = cyrup_test_support::tui::TestTerminal::new(40, 3);
        term.draw(|frame: &mut Frame| {
            let area = Rect::new(0, 0, 40, lines.len() as u16);
            Paragraph::new(lines.clone()).render(area, frame.buffer_mut());
        });

        let grid = term.snapshot();
        assert!(grid.contains("plain-run"), "grid missing label:\n{grid}");
        assert!(
            !grid.contains("fork"),
            "fresh context must never paint a fork badge:\n{grid}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // SUBA-162 — the progressive async-widget tier (pi `render.ts:2516-2820` @ad11b7ab)
    // -----------------------------------------------------------------------------------------

    use crate::tui::events::{AsyncJobSnapshot, AsyncWidgetRender, render_async_jobs_widget};

    fn job(id: &str, mode: RunMode, state: RunState, steps: &[StepState]) -> AsyncJobSnapshot {
        let status = RunStatus::queued(RunId::from_token(id.to_string()), mode, None);
        let mut job =
            AsyncJobSnapshot::from_run_status(&status, Some(id.to_string()), ContextMode::Fresh);
        job.state = state;
        job.step_states = steps.to_vec();
        job
    }

    fn running(id: &str) -> AsyncJobSnapshot {
        job(
            id,
            RunMode::Single,
            RunState::Running,
            &[StepState::Running],
        )
    }

    fn rows_view() -> AsyncWidgetRender {
        AsyncWidgetRender {
            layout: AsyncWidgetLayout::Rows,
            ..AsyncWidgetRender::default()
        }
    }

    fn paint(jobs: &[AsyncJobSnapshot], session: &mut Option<WidgetLayoutSession>) -> Vec<String> {
        lines_to_plain_text(&render_async_jobs_widget(jobs, &rows_view(), session, 0))
    }

    fn header_count(jobs: &[AsyncJobSnapshot]) -> usize {
        let tree = WidgetJobTree::build(jobs);
        tree.roots
            .iter()
            .map(|job| running_leaf_agent_count(job, &tree))
            .sum()
    }

    /// The Verify's first clause in widget terms: a four-lane workflow beside one single run reads
    /// `5 agents running` — four lanes plus the single — and the workflow's OWN steps are not
    /// counted. Mutations killed: the pre-#2584 rule (running root jobs: `2 agents running`), and
    /// the workflow arm removed (the workflow counts its own two steps: 3).
    #[test]
    fn a_workflow_counts_its_loaded_children_not_itself() {
        let workflow = job(
            "wf",
            RunMode::Workflow,
            RunState::Running,
            &[StepState::Running, StepState::Running],
        );
        let mut jobs = vec![workflow];
        for lane in ["l1", "l2", "l3", "l4"] {
            let mut child = running(lane);
            child.parent_workflow_run_id = Some(RunId::from_token("wf"));
            jobs.push(child);
        }
        jobs.push(running("solo"));

        let tree = WidgetJobTree::build(&jobs);
        assert_eq!(tree.roots.len(), 2, "the lanes hang under the workflow");
        assert_eq!(running_leaf_agent_count(&jobs[0], &tree), 4);
        let mut session = None;
        let lines = paint(&jobs, &mut session);
        assert!(
            lines[0].ends_with(" Async agents · 5 agents running"),
            "{lines:?}"
        );
    }

    /// pi `:2640-2647`: a sequential chain counts only its current step; a live parallel group
    /// lifts the exclusion. Mutation killed: dropping the `sequential && offset != current`
    /// exclusion (3 instead of 1).
    #[test]
    fn a_sequential_chain_counts_only_its_current_step() {
        let mut chain = job(
            "chain",
            RunMode::Chain,
            RunState::Running,
            &[
                StepState::Complete,
                StepState::Running,
                StepState::Pending,
                StepState::Pending,
            ],
        );
        chain.current_step_index = Some(1);
        assert_eq!(header_count(std::slice::from_ref(&chain)), 1);
        let mut session = None;
        let lines = paint(std::slice::from_ref(&chain), &mut session);
        assert!(lines[0].ends_with(" · 1 agent running"), "{lines:?}");

        let mut grouped = job(
            "grouped",
            RunMode::Chain,
            RunState::Running,
            &[StepState::Running, StepState::Pending, StepState::Pending],
        );
        grouped.current_step_index = Some(0);
        grouped.active_parallel_group = true;
        assert_eq!(header_count(std::slice::from_ref(&grouped)), 3);
    }

    /// pi `:2641-2645`: a running job with no step detail counts its `agents` (a sequential chain
    /// only its current one), and with no agents either counts 1; a queued job counts 0 and shows
    /// as `N queued`. Mutations killed: the pre-#2584 rule (the three-agent parallel counts 1),
    /// the empty-steps arm returning 0, and the `state != Running` guard removed (the queued job
    /// counts 1).
    #[test]
    fn a_job_with_no_step_detail_counts_its_agents() {
        let mut parallel = job("par", RunMode::Parallel, RunState::Running, &[]);
        parallel.agents = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(header_count(std::slice::from_ref(&parallel)), 3);

        let mut chain = job("ch", RunMode::Chain, RunState::Running, &[]);
        chain.agents = vec!["a".into(), "b".into(), "c".into()];
        chain.current_step_index = Some(2);
        assert_eq!(header_count(std::slice::from_ref(&chain)), 1);

        let bare = job("bare", RunMode::Single, RunState::Running, &[]);
        assert_eq!(header_count(std::slice::from_ref(&bare)), 1);

        let queued = job(
            "q",
            RunMode::Single,
            RunState::Queued,
            &[StepState::Pending],
        );
        assert_eq!(header_count(std::slice::from_ref(&queued)), 0);

        let mut session = None;
        let lines = paint(&[parallel, queued], &mut session);
        assert!(
            lines[0].ends_with(" Async agents · 3 agents running, 1 queued"),
            "{lines:?}"
        );
    }

    /// pi `:2659-2664`: with nothing active the header reports outcomes; with none of those
    /// either, `N total`.
    #[test]
    fn the_progressive_header_reports_outcomes_once_nothing_is_active() {
        let jobs = [
            job("a", RunMode::Single, RunState::Complete, &[]),
            job("b", RunMode::Single, RunState::Failed, &[]),
        ];
        let refs: Vec<&AsyncJobSnapshot> = jobs.iter().collect();
        let tree = WidgetJobTree::build(&jobs);
        let header = lines_to_plain_text(&[progressive_header_line(&refs, &tree, 0)]);
        assert_eq!(header[0], "○ Async agents · 1 failed, 1/2 done");
        let partial = [job("p", RunMode::Single, RunState::Partial, &[])];
        let refs: Vec<&AsyncJobSnapshot> = partial.iter().collect();
        let tree = WidgetJobTree::build(&partial);
        let header = lines_to_plain_text(&[progressive_header_line(&refs, &tree, 0)]);
        assert_eq!(header[0], "○ Async agents · 1 total");
    }

    /// #2583: the card locks to the rows its content fills, with no blank padding. Mutation
    /// killed: locking at the cap (10 rows, 6 of them blank).
    #[test]
    fn the_progressive_card_locks_to_its_content_rows() {
        let jobs = [running("a"), running("b"), running("c")];
        let mut session = None;
        let lines = paint(&jobs, &mut session);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines.iter().all(|l| !l.trim().is_empty()), "{lines:?}");
        assert_eq!(session.as_ref().and_then(|s| s.locked_rows), Some(4));
    }

    /// The Verify's "the locked card keeps its height as jobs finish": a finished job keeps a row
    /// (listed after the active ones, as `done`), so the card neither shrinks nor goes blank.
    /// Mutation killed: dropping `select_progressive_job_keys`' passes for inactive jobs (the
    /// finished jobs vanish and the card is padded with blank rows).
    #[test]
    fn the_locked_card_keeps_its_height_as_jobs_finish() {
        let mut jobs = vec![running("a"), running("b"), running("c")];
        let mut session = None;
        assert_eq!(paint(&jobs, &mut session).len(), 4);

        jobs[0].state = RunState::Complete;
        jobs[1].state = RunState::Complete;
        let lines = paint(&jobs, &mut session);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[1].contains("c · running"), "{lines:?}");
        assert!(lines[2].ends_with("a · done"), "{lines:?}");
        assert!(lines[3].ends_with("b · done"), "{lines:?}");

        jobs[2].state = RunState::Complete;
        let lines = paint(&jobs, &mut session);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines.iter().all(|l| !l.trim().is_empty()), "{lines:?}");
        assert!(lines[0].ends_with(" Async agents · 3/3 done"), "{lines:?}");
    }

    /// #186: the height holds across a content-only update. A workflow's lane rows fill the rows
    /// left after the job lines (#2583); when a lane drops out of the projection the card keeps
    /// its locked height (padded) rather than jumping. Mutations killed: no lane fill (the first
    /// card is 2 rows), and sizing every pass to its content (the second card is 3 rows).
    #[test]
    fn workflow_lanes_fill_spare_rows_and_the_lock_holds_across_content_only_updates() {
        use crate::tui::events::{WorkflowLaneRow, WorkflowLaneState};
        let lane = |key: &str, agent: &str, state| WorkflowLaneRow {
            key: key.to_string(),
            agent: Some(agent.to_string()),
            state,
        };
        let mut workflow = job("wf", RunMode::Workflow, RunState::Running, &[]);
        workflow.workflow_lanes = vec![
            lane("plan", "planner", WorkflowLaneState::Complete),
            lane("build", "coder", WorkflowLaneState::Running),
            lane("review", "reviewer", WorkflowLaneState::Queued),
        ];
        let mut session = None;
        let lines = paint(std::slice::from_ref(&workflow), &mut session);
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert_eq!(lines[2], "    ✓ plan · planner");
        assert!(lines[3].ends_with(" build · coder · active"), "{lines:?}");
        assert_eq!(lines[4], "    ◦ review · reviewer · queued");

        workflow.workflow_lanes.truncate(1);
        let lines = paint(std::slice::from_ref(&workflow), &mut session);
        assert_eq!(lines.len(), 5, "the locked height holds: {lines:?}");
        assert_eq!(lines[2], "    ✓ plan · planner");
    }

    /// The Verify's "grows, up to the cap, when a job starts while jobs are hidden". Mutations
    /// killed: deleting the grow branch (the two-job card stays 2 rows, `+2 more`), and growing
    /// past the cap (12 jobs at the 11 available rows).
    #[test]
    fn the_locked_card_grows_up_to_the_cap_when_a_job_starts_while_jobs_are_hidden() {
        let mut jobs = vec![running("j00")];
        let mut session = None;
        assert_eq!(paint(&jobs, &mut session).len(), 2);

        jobs.push(running("j01"));
        let lines = paint(&jobs, &mut session);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(
            lines[1].contains("j00") && lines[2].contains("j01"),
            "{lines:?}"
        );

        for index in 2..12 {
            jobs.push(running(&format!("j{index:02}")));
        }
        let lines = paint(&jobs, &mut session);
        assert_eq!(
            lines.len(),
            collapsed_widget_line_budget(ASYNC_WIDGET_FALLBACK_ROWS),
            "{lines:?}"
        );
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[9], "  +4 more (4 running)");
        assert!(lines.iter().all(|l| !l.trim().is_empty()), "{lines:?}");
    }

    /// `8d804895`/#2662: the lock shrinks to content when root jobs leave, so a departed job leaves
    /// no blank rows. Mutation killed: deleting the `jobs.len() < root_job_count` branch (7 rows,
    /// 5 of them blank).
    #[test]
    fn the_card_shrinks_when_root_jobs_leave() {
        let jobs: Vec<AsyncJobSnapshot> = (0..6).map(|i| running(&format!("s{i}"))).collect();
        let mut session = None;
        assert_eq!(paint(&jobs, &mut session).len(), 7);
        for _ in 0..2 {
            let lines = paint(&jobs[..1], &mut session);
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(lines.iter().all(|l| !l.trim().is_empty()), "{lines:?}");
        }
    }

    /// pi `renderWidget`'s `resetWidgetLayoutSession()` on an empty list (`render.ts:3093`).
    /// Mutation killed: skipping the reset.
    #[test]
    fn an_empty_job_list_resets_the_session() {
        let mut session = None;
        assert_eq!(
            paint(&[running("a"), running("b"), running("c")], &mut session).len(),
            4
        );
        assert!(session.is_some());
        assert!(paint(&[], &mut session).is_empty());
        assert!(session.is_none());
    }

    /// pi `:2798` (#2738): the adaptive layout keeps the full tier while it fits, byte for byte
    /// the pre-SUBA-162 widget. Mutation killed: treating `Adaptive` as `Rows` (the output gains
    /// the `Async agents` header).
    #[test]
    fn the_adaptive_layout_keeps_the_full_tier_when_it_fits() {
        let jobs = [running("a"), running("b"), running("c")];
        let mut session = None;
        let lines = render_async_jobs_widget(&jobs, &AsyncWidgetRender::default(), &mut session, 3);
        let snapshots: Vec<SubagentProgressSnapshot> = jobs
            .iter()
            .map(AsyncJobSnapshot::to_progress_snapshot)
            .collect();
        assert_eq!(
            lines_to_plain_text(&lines),
            lines_to_plain_text(&render_background_region(&snapshots, 3))
        );
        assert_eq!(session.map(|s| s.tier), Some(WidgetRenderTier::Full));
    }

    /// pi `:2984-2986`: the collapsed card counts EVERY job, and leaves the session alone.
    #[test]
    fn the_collapsed_widget_is_one_line_over_every_job() {
        let jobs = [
            running("a"),
            job("b", RunMode::Single, RunState::Queued, &[]),
            job("c", RunMode::Single, RunState::Failed, &[]),
        ];
        let view = AsyncWidgetRender {
            collapsed: true,
            ..rows_view()
        };
        let mut session = None;
        let lines = lines_to_plain_text(&render_async_jobs_widget(&jobs, &view, &mut session, 0));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].ends_with(" subagents (1/3 running, 1 queued, 1 failed)"),
            "{lines:?}"
        );
        assert!(session.is_none());
    }
}
