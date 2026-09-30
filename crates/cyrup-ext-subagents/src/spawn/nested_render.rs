//! SUBA-134 — pi `runs/shared/nested-render.ts` (@v0.71.0): the indented `↳` tree of nested
//! descendant runs the status report prints under a run or a step (`formatNestedRunStatusLines`),
//! with its depth/line budget folding the remainder into a `+N nested runs (…)` aggregate.
//!
//! Distinct from the TUI's fleet tree (`tui/fleet_status.rs`, pi `tui/fleet-status.ts`): that one
//! names a nested run by agent only, this one by its child session name first
//! (`nestedRunLabel`, `nested-render.ts:52-57`).

use super::nested_events::{NestedRunSummary, NestedStepSummary, TokenUsage};
use crate::background::ActivityState;

/// pi `NestedRunCounts` (`nested-render.ts:5-15`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct NestedRunCounts {
    total: usize,
    running: usize,
    paused: usize,
    complete: usize,
    failed: usize,
    partial: usize,
    rejected: usize,
    stopped: usize,
    queued: usize,
}

impl NestedRunCounts {
    fn add(&mut self, other: Self) {
        self.total += other.total;
        self.running += other.running;
        self.paused += other.paused;
        self.complete += other.complete;
        self.failed += other.failed;
        self.partial += other.partial;
        self.rejected += other.rejected;
        self.stopped += other.stopped;
        self.queued += other.queued;
    }
}

/// Every nested run under `child` one level down: its own children, then its steps' children —
/// the argument order pi builds at `nested-render.ts:21` and `:96`.
fn descendants_of(child: &NestedRunSummary) -> Vec<NestedRunSummary> {
    let mut out: Vec<NestedRunSummary> = child.children.clone().unwrap_or_default();
    out.extend(step_children(child));
    out
}

fn step_children(child: &NestedRunSummary) -> Vec<NestedRunSummary> {
    child
        .steps
        .iter()
        .flatten()
        .flat_map(|step| step.children.iter().flatten().cloned())
        .collect()
}

/// pi `countNestedRuns` (`nested-render.ts:17-34`).
fn count_nested_runs(children: &[NestedRunSummary]) -> NestedRunCounts {
    let mut counts = NestedRunCounts::default();
    for child in children {
        counts.total += 1;
        match child.state.as_str() {
            "running" => counts.running += 1,
            "paused" => counts.paused += 1,
            "complete" => counts.complete += 1,
            "failed" => counts.failed += 1,
            "partial" => counts.partial += 1,
            "rejected" => counts.rejected += 1,
            "stopped" => counts.stopped += 1,
            "queued" => counts.queued += 1,
            // The sanitizer admits no other state (`nested_events::sanitize_state`).
            _ => {}
        }
        counts.add(count_nested_runs(&descendants_of(child)));
    }
    counts
}

/// pi `formatNestedAggregate` (`nested-render.ts:36-50`).
#[must_use]
pub fn format_nested_aggregate(children: &[NestedRunSummary]) -> Option<String> {
    let counts = count_nested_runs(children);
    if counts.total == 0 {
        return None;
    }
    let parts: Vec<String> = [
        (counts.running, "running"),
        (counts.paused, "paused"),
        (counts.failed, "failed"),
        (counts.partial, "partial"),
        (counts.rejected, "rejected"),
        (counts.stopped, "stopped"),
        (counts.complete, "complete"),
        (counts.queued, "queued"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, word)| format!("{count} {word}"))
    .collect();
    Some(format!(
        "+{} nested run{}{}",
        counts.total,
        if counts.total == 1 { "" } else { "s" },
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    ))
}

/// pi `nestedRunLabel` (`nested-render.ts:52-57`): the child session's name, else the agent, else a
/// compact agents summary, else the run id.
#[must_use]
pub fn nested_run_label(run: &NestedRunSummary) -> String {
    if let Some(name) = run.session_name.as_deref().map(str::trim)
        && !name.is_empty()
    {
        return name.to_string();
    }
    if let Some(agent) = run.agent.as_deref().filter(|agent| !agent.is_empty()) {
        return agent.to_string();
    }
    if let Some(agents) = run.agents.as_ref().filter(|agents| !agents.is_empty()) {
        if let [only] = agents.as_slice() {
            return only.clone();
        }
        let head = agents
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return if agents.len() > 2 {
            format!("{head} +{}", agents.len() - 2)
        } else {
            head
        };
    }
    run.id.clone()
}

/// The typed activity state pi's `formatActivityLabel` switches on, from the sanitized wire string
/// (`nested_events::sanitize_summary_map` admits only these two).
#[must_use]
pub fn parse_activity_state(value: Option<&str>) -> Option<ActivityState> {
    match value {
        Some("active_long_running") => Some(ActivityState::ActiveLongRunning),
        Some("needs_attention") => Some(ActivityState::NeedsAttention),
        _ => None,
    }
}

/// The fields pi's `formatNestedActivity` (`nested-render.ts:59-77`) reads, off a run or a step.
struct NestedActivityInput<'a> {
    activity_state: Option<&'a str>,
    last_activity_at: Option<i64>,
    current_tool: Option<&'a str>,
    current_tool_started_at: Option<i64>,
    current_path: Option<&'a str>,
    turn_count: Option<i64>,
    tool_count: Option<i64>,
    total_tokens: Option<&'a TokenUsage>,
}

impl<'a> NestedActivityInput<'a> {
    fn of_run(run: &'a NestedRunSummary) -> Self {
        Self {
            activity_state: run.activity_state.as_deref(),
            last_activity_at: run.last_activity_at,
            current_tool: run.current_tool.as_deref(),
            current_tool_started_at: run.current_tool_started_at,
            current_path: run.current_path.as_deref(),
            turn_count: run.turn_count,
            tool_count: run.tool_count,
            total_tokens: run.total_tokens.as_ref(),
        }
    }

    fn of_step(step: &'a NestedStepSummary) -> Self {
        Self {
            activity_state: step.activity_state.as_deref(),
            last_activity_at: step.last_activity_at,
            current_tool: step.current_tool.as_deref(),
            current_tool_started_at: step.current_tool_started_at,
            current_path: step.current_path.as_deref(),
            turn_count: step.turn_count,
            tool_count: step.tool_count,
            total_tokens: None,
        }
    }
}

/// pi `formatNestedActivity` (`nested-render.ts:59-77`). `now` is upstream's `Date.now()`.
fn format_nested_activity(input: &NestedActivityInput<'_>, now: i64) -> Option<String> {
    let mut facts: Vec<String> = Vec::new();
    match (input.current_tool, input.current_tool_started_at) {
        (Some(tool), Some(started)) if !tool.is_empty() => facts.push(format!(
            "tool {tool} {}",
            crate::background::wait::format_duration(
                u64::try_from(now.saturating_sub(started).max(0)).unwrap_or(0)
            )
        )),
        (Some(tool), _) if !tool.is_empty() => facts.push(format!("tool {tool}")),
        _ => {}
    }
    if let Some(path) = input.current_path.filter(|path| !path.is_empty()) {
        facts.push(crate::exec::tool_call_summary::shorten_path(path));
    }
    if let Some(turns) = input.turn_count {
        facts.push(format!("{turns} turns"));
    }
    if let Some(tools) = input.tool_count {
        facts.push(format!("{tools} tools"));
    }
    // pi `formatTokenUsage(usage)` (`formatters.ts:23-27`): cyrup's nested `TokenUsage` carries no
    // `window`, so this is always the `<total> tok` arm.
    if let Some(tokens) = input.total_tokens {
        facts.push(format!(
            "{} tok",
            crate::formatters::format_tokens(u64::try_from(tokens.total).unwrap_or(0))
        ));
    }
    let activity = crate::background::fleet_view::format_activity_label(
        input.last_activity_at,
        parse_activity_state(input.activity_state),
        now,
    );
    if activity.is_none() && facts.is_empty() {
        return None;
    }
    Some(
        activity
            .into_iter()
            .chain(facts)
            .collect::<Vec<_>>()
            .join(" | "),
    )
}

/// pi `formatNestedRunStatusLines` options (`nested-render.ts:120`), defaults applied.
#[derive(Debug, Clone, Copy)]
pub struct NestedLinesOptions<'a> {
    /// Leading indent of the top level (default `"  "`).
    pub indent: &'a str,
    /// Deepest level rendered row by row; deeper levels fold into an aggregate (default 2).
    pub max_depth: usize,
    /// Line budget (default 40).
    pub max_lines: usize,
    /// Whether each run gets its `Status:` command hint (default `false`).
    pub command_hints: bool,
}

impl Default for NestedLinesOptions<'_> {
    fn default() -> Self {
        Self {
            indent: "  ",
            max_depth: 2,
            max_lines: 40,
            command_hints: false,
        }
    }
}

/// pi `formatNestedRunStatusLines` → `formatNestedRunLines` (`nested-render.ts:79-127`).
#[must_use]
pub fn format_nested_run_status_lines(
    children: &[NestedRunSummary],
    options: NestedLinesOptions<'_>,
    now: i64,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    append(&mut lines, children, 0, options.indent, &options, now);
    lines
}

/// The recursive `append` closure of `formatNestedRunLines` (`nested-render.ts:81-116`); each
/// `return` ends only the current call, as upstream's does.
fn append(
    lines: &mut Vec<String>,
    items: &[NestedRunSummary],
    depth: usize,
    indent: &str,
    options: &NestedLinesOptions<'_>,
    now: i64,
) {
    if items.is_empty() || lines.len() >= options.max_lines {
        return;
    }
    if depth > options.max_depth {
        if let Some(aggregate) = format_nested_aggregate(items)
            && lines.len() < options.max_lines
        {
            lines.push(format!("{indent}↳ {aggregate}"));
        }
        return;
    }
    for (index, child) in items.iter().enumerate() {
        if lines.len() >= options.max_lines {
            if let Some(aggregate) = format_nested_aggregate(items.get(index..).unwrap_or_default())
                && let Some(last) = lines.last_mut()
            {
                *last = format!("{indent}↳ {aggregate}");
            }
            return;
        }
        let activity = if child.state == "running" {
            format_nested_activity(&NestedActivityInput::of_run(child), now)
        } else {
            None
        };
        let error = child
            .error
            .as_deref()
            .filter(|error| !error.is_empty())
            .map(|error| format!(" | error: {error}"))
            .unwrap_or_default();
        // pi `formatModelThinking(child.model, child.thinking)`: cyrup's nested summary carries
        // neither field, so the ` | <model>` segment is always empty here.
        lines.push(format!(
            "{indent}↳ {} [{}] {}{}{error}",
            nested_run_label(child),
            child.id,
            child.state,
            activity.map(|a| format!(" | {a}")).unwrap_or_default(),
        ));
        if options.command_hints && lines.len() < options.max_lines {
            lines.push(format!(
                "{indent}  Status: subagent({{ action: \"status\", id: \"{}\" }})",
                child.id
            ));
        }
        if depth == options.max_depth {
            let mut below = step_children(child);
            below.extend(child.children.clone().unwrap_or_default());
            if let Some(aggregate) = format_nested_aggregate(&below)
                && lines.len() < options.max_lines
            {
                lines.push(format!("{indent}  ↳ {aggregate}"));
            }
            continue;
        }
        for (step_index, step) in child.steps.iter().flatten().enumerate() {
            if lines.len() >= options.max_lines {
                return;
            }
            let step_activity = if step.status == "running" {
                format_nested_activity(&NestedActivityInput::of_step(step), now)
            } else {
                None
            };
            lines.push(format!(
                "{indent}  {}. {} {}{}{}",
                step_index + 1,
                nested_step_display_name(step),
                step.status,
                step_activity.map(|a| format!(" | {a}")).unwrap_or_default(),
                step.error
                    .as_deref()
                    .filter(|error| !error.is_empty())
                    .map(|error| format!(" | error: {error}"))
                    .unwrap_or_default(),
            ));
            append(
                lines,
                step.children.as_deref().unwrap_or_default(),
                depth + 1,
                &format!("{indent}    "),
                options,
                now,
            );
        }
        append(
            lines,
            child.children.as_deref().unwrap_or_default(),
            depth + 1,
            &format!("{indent}  "),
            options,
            now,
        );
    }
}

/// pi `step.sessionName?.trim() || step.agent` (`nested-render.ts:110`, `run-status.ts:337`).
#[must_use]
pub fn nested_step_display_name(step: &NestedStepSummary) -> String {
    step.session_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&step.agent)
        .to_string()
}
