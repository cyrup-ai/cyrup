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
//! [`cyrup_test_support::tui::TestTerminal`] without a live terminal, per this phase's own
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

use crate::background::{RunMode, RunState};
use crate::fork_context::ContextMode;
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
pub const ACTIVITY_GLYPH_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
        ACTIVITY_GLYPH_FRAMES.get(idx).copied().unwrap_or(IDLE_GLYPH)
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
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
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
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
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
        let mut header = render_run_header_line(&child.agent, child.status.state, child_context(child), tick);
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
pub fn render_progress_header(snapshot: &SubagentProgressSnapshot, tick: usize) -> Vec<Line<'static>> {
    let label = snapshot.current_agent.as_deref().unwrap_or(snapshot.run_id.as_ref());
    let mut lines = vec![render_run_header_line(label, snapshot.status.state, snapshot.context, tick)];
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
pub fn render_background_region(snapshots: &[SubagentProgressSnapshot], tick: usize) -> Vec<Line<'static>> {
    if snapshots.is_empty() {
        return Vec::new();
    }

    let (detailed, overflow) = split_at_budget(snapshots, MAX_DETAILED_RUNS);
    let mut out = Vec::new();
    for snapshot in detailed {
        out.extend(render_progress_header(snapshot, tick));
    }
    if !overflow.is_empty() {
        let running = overflow.iter().filter(|s| is_actively_running(s.status.state)).count();
        let noun = if overflow.len() == 1 { "run" } else { "runs" };
        out.push(Line::from(vec![Span::styled(
            format!("… +{} more {noun} tracked ({running} running)", overflow.len()),
            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
        )]));
    }
    out
}

/// Renders a compact one-line summary for a run mode label — used by callers that need a short
/// descriptor (e.g. `"chain"`/`"parallel"`/`"single"`) alongside a header line without pulling in
/// a full [`SubagentProgressSnapshot`]. Kept trivial and separate from [`render_run_header_line`]
/// since not every call site wants the mode label inline.
#[must_use]
pub fn run_mode_label(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Single => "single",
        RunMode::Parallel => "parallel",
        RunMode::Chain => "chain",
    }
}

/// Flattens a rendered [`Vec<Line>`] to plain text, one row per line, with no trailing styling
/// information — a small convenience for callers/tests that want a plain-text assertion surface
/// without going through a full [`ratatui::backend::TestBackend`] paint. Every function in this
/// module is designed to be equally testable either way (plain-text via this helper, or grid-
/// painted via [`cyrup_test_support::tui::TestTerminal`]), per this phase's own testing
/// instructions.
#[must_use]
pub fn lines_to_plain_text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
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
        NestedRunSummary { run_id: RunId::new(), agent: agent.to_string(), status: status(state), children }
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
        assert_ne!(running_frame_0, running_frame_1, "running glyph must advance across ticks");
        assert_eq!(running_frame_0, ACTIVITY_GLYPH_FRAMES[0]);
        assert_eq!(running_frame_1, ACTIVITY_GLYPH_FRAMES[1]);
    }

    #[test]
    fn activity_glyph_is_static_idle_for_every_non_running_state() {
        for state in [RunState::Queued, RunState::Paused, RunState::Complete, RunState::Failed] {
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
        for state in [RunState::Queued, RunState::Running, RunState::Complete, RunState::Failed] {
            let snap = snapshot("scout", state, ContextMode::Fork, vec![]);
            let header = render_run_header_line(
                snap.current_agent.as_deref().unwrap_or("?"),
                snap.status.state,
                snap.context,
                0,
            );
            let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.contains(FORK_BADGE_TEXT), "state {state:?} must still show fork badge");
        }
        let fresh_snap = snapshot("scout", RunState::Running, ContextMode::Fresh, vec![]);
        let header = render_run_header_line("scout", fresh_snap.status.state, fresh_snap.context, 0);
        let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains(FORK_BADGE_TEXT), "fresh context must never show fork badge");
    }

    // ---- R-SA-112/113: nested fold, depth cap, overflow aggregation ----

    #[test]
    fn nested_children_render_indented_under_parent() {
        let children = vec![nested("worker-a", RunState::Running, vec![]), nested("worker-b", RunState::Complete, vec![])];
        let lines = render_nested_children(&children, 0, 0);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.starts_with("  "), "child line must be indented: {text:?}");
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
        let texts: Vec<String> =
            lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();

        // Expect: "c" header line, then "g" header line (depth 1 -> within cap), then an
        // aggregate line collapsing everything at/under "g"'s children (the grandchild "gg"
        // subtree), since depth+1 (2) >= MAX_NESTED_DEPTH (2) once we'd otherwise recurse into
        // "g"'s children.
        assert!(texts.iter().any(|t| t.contains('c')), "expected child 'c' rendered: {texts:?}");
        assert!(texts.iter().any(|t| t.contains('g') && !t.contains("gg")), "expected grandchild 'g' rendered: {texts:?}");
        assert!(
            texts.iter().any(|t| t.contains("more")),
            "expected an aggregate overflow/collapse line beyond the depth cap: {texts:?}"
        );
        // The deepest literal agent name ("gg") must never appear as its own rendered header —
        // it must have been folded into the aggregate line instead.
        assert!(!texts.iter().any(|t| t.trim_start().starts_with("gg")), "grandchild-of-grandchild must not get its own line: {texts:?}");
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
        let last_text: String =
            lines.last().expect("non-empty").spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(last_text.contains("+3 more"), "expected '+3 more' aggregate line, got {last_text:?}");
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
        let children: Vec<_> = (0..500).map(|i| nested(&format!("w{i}"), RunState::Complete, vec![])).collect();
        let lines = render_nested_children(&children, 0, 0);
        assert_eq!(lines.len(), MAX_CHILDREN_PER_LEVEL + 1);
    }

    // ---- R-SA-108: bounded detail + fold-to-summary overflow in the background region ----

    #[test]
    fn background_region_details_up_to_cap_then_folds_overflow() {
        let mut snapshots = Vec::new();
        for i in 0..(MAX_DETAILED_RUNS + 2) {
            snapshots.push(snapshot(&format!("agent-{i}"), RunState::Running, ContextMode::Fresh, vec![]));
        }
        let lines = render_background_region(&snapshots, 0);
        // MAX_DETAILED_RUNS header lines + 1 aggregate line (no nested children in this fixture).
        assert_eq!(lines.len(), MAX_DETAILED_RUNS + 1);
        let last_text: String =
            lines.last().expect("non-empty").spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(last_text.contains("+2 more"), "expected overflow aggregate, got {last_text:?}");
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
        let texts: Vec<String> =
            lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
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
        assert_eq!(lines.len(), 2, "expected parent header + one nested child line");
        let plain = lines_to_plain_text(&lines);
        assert!(plain[0].contains("orchestrated-worker"));
        assert!(plain[0].contains(FORK_BADGE_TEXT));
        assert!(plain[1].contains("child-1"));
        assert!(plain[1].starts_with("  "), "nested child must be indented under parent");
    }

    #[test]
    fn run_mode_label_covers_every_variant() {
        assert_eq!(run_mode_label(RunMode::Single), "single");
        assert_eq!(run_mode_label(RunMode::Parallel), "parallel");
        assert_eq!(run_mode_label(RunMode::Chain), "chain");
    }

    // ---- Determinism: same input -> byte-identical output across repeated calls ----

    #[test]
    fn rendering_is_pure_and_deterministic() {
        let snap = snapshot(
            "det",
            RunState::Running,
            ContextMode::Fork,
            vec![nested("c1", RunState::Complete, vec![nested("c2", RunState::Running, vec![])])],
        );
        let a = lines_to_plain_text(&render_progress_header(&snap, 5));
        let b = lines_to_plain_text(&render_progress_header(&snap, 5));
        assert_eq!(a, b, "identical input at the same tick must render byte-identical output");
    }

    // ---- Grid-painted assertions via cyrup-test-support's TestBackend wrapper ----
    // Mirrors crates/cyrup-tui/tests/assembled_render.rs's whole-buffer text-grid pattern, scoped
    // down to this module's pure render output rather than a live `App`.

    #[test]
    fn nested_tree_paints_into_a_real_test_backend_grid() {
        let children = vec![
            nested("alpha", RunState::Running, vec![]),
            nested("beta", RunState::Complete, vec![nested("beta-child", RunState::Complete, vec![])]),
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
        assert!(grid.contains(FORK_BADGE_TEXT), "grid missing fork badge:\n{grid}");
        assert!(grid.contains("alpha"), "grid missing nested child 'alpha':\n{grid}");
        assert!(grid.contains("beta"), "grid missing nested child 'beta':\n{grid}");
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
        assert!(!grid.contains("fork"), "fresh context must never paint a fork badge:\n{grid}");
    }
}
