//! Fleet + transcript status views — the Rust port of pi `runs/background/fleet-view.ts`
//! (`@v0.34.0`, 515 lines).
//!
//! # What this is, and what it is NOT
//!
//! Upstream has TWO things whose file names contain "fleet", and only ONE of them is in scope here:
//!
//! * **`src/runs/background/fleet-view.ts`** — present at v0.34.0, the *text renderer* behind
//!   `subagent({ action: "status", view: "fleet" | "transcript" })`. That is what this module ports.
//! * **`src/tui/fleet.ts` / `fleet-status.ts` / `fleet-transcript.ts`** — the interactive
//!   FleetView TUI surface. Those files **do not exist at v0.34.0** (verified:
//!   `git ls-tree -r --name-only v0.34.0 | grep -i fleet` returns exactly one path, the one above;
//!   the same command at v0.43.0 returns four). They are a v0.35+ subtree and belong to a later
//!   part of this port, NOT here.
//!
//! # The two entry points
//!
//! * [`format_fleet`] ports `inspectSubagentFleet` (`fleet-view.ts:295-338`) — the read-only
//!   "everything currently in flight" surface: live foreground runs first, then every queued/running
//!   background run with its per-step lines and the exact `subagent({...})` command hints an LLM
//!   reads to drill in.
//! * [`format_async_run_transcript`] ports `formatAsyncRunTranscript` (`:398-449`) — the tail of one
//!   run's (or one child's) transcript, resolved through pi's own three-source fallback ladder:
//!   the step/run output log, then the step's `recentOutput` ring in `status.json`, then the
//!   persisted session JSONL.
//!
//! Both are bounded reads. [`read_text_tail`] seeks to the last [`TRANSCRIPT_TAIL_BYTES`] of a file
//! rather than reading it whole (pi `:92-119`), and every path a caller can influence goes through
//! [`read_contained_text_tail`]'s trusted-root + symlink + real-path gate (pi `:121-148`) so a
//! `sessionFile` recorded in a run's `status.json` cannot be used to read an arbitrary file.
//!
//! # Honest deltas vs. pi
//!
//! Same discipline as [`super::run_status`]'s own documented subset — render what cyrup's status
//! schema actually carries, never fake a field:
//!
//! 1. **No per-step `label`/`phase`.** pi's `AsyncJobStep` carries user-facing `label` and a
//!    `phase` group tag; cyrup's [`StepStatus`] carries neither, so the `[phase] label (agent)`
//!    decoration collapses to the bare agent name. Identical to `run_status.rs`'s existing delta.
//! 2. **No nested-descendant lines.** pi interleaves `formatNestedRunStatusLines` under each step;
//!    cyrup's [`StepStatus::nested_run_ids`] holds ids, not reconciled summaries, and rendering them
//!    would mean a recursive reconcile this view deliberately does not do.
//! 3. **No run-level `outputFile`.** pi's `AsyncStatus.outputFile` has no cyrup counterpart (cyrup
//!    keeps the output path per step, on [`super::StepTelemetry::output_file`]), so the no-index
//!    transcript form falls back to the per-step ladder rather than a run-wide output artifact.
//! 4. **Foreground rows are thinner.** pi's `foregroundControls` entry carries `updatedAt`, token
//!    and turn counters; cyrup's `ForegroundControlEntry` carries the interrupt token, the live
//!    message-route coordinates and the activity state. [`ForegroundFleetEntry`] is exactly that
//!    subset, and the row shape is otherwise pi's verbatim.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::run_status::{ActiveRun, progress_label, run_mode_label, run_state_label, step_state_label};
use super::{ActivityState, RunPaths, RunStatus, StepStatus};

/// pi `DEFAULT_TRANSCRIPT_LINES` (`fleet-view.ts:21`).
pub const DEFAULT_TRANSCRIPT_LINES: usize = 80;
/// pi `MAX_TRANSCRIPT_LINES` (`fleet-view.ts:22`).
pub const MAX_TRANSCRIPT_LINES: usize = 500;
/// pi `TRANSCRIPT_TAIL_BYTES` (`fleet-view.ts:23`) — a transcript tail never reads more than the
/// last 256 KiB of a file, however large the file has grown.
const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// pi `transcriptLineLimit` (`fleet-view.ts:53-57`): an absent value is
/// [`DEFAULT_TRANSCRIPT_LINES`]; anything else is truncated toward zero and clamped into
/// `1..=`[`MAX_TRANSCRIPT_LINES`].
#[must_use]
pub fn transcript_line_limit(value: Option<i64>) -> usize {
    match value {
        None => DEFAULT_TRANSCRIPT_LINES,
        // `Math.max(1, Math.min(MAX, Math.trunc(value)))` — a JSON integer already arrives
        // truncated, so only the clamp survives the port.
        Some(v) => {
            let clamped = v.clamp(1, MAX_TRANSCRIPT_LINES as i64);
            usize::try_from(clamped).unwrap_or(DEFAULT_TRANSCRIPT_LINES)
        }
    }
}

/// pi `TextTailResult` (`fleet-view.ts:46-51`).
struct TextTail {
    path: PathBuf,
    lines: Vec<String>,
    truncated: bool,
    error: Option<String>,
}

impl TextTail {
    fn empty(path: &Path) -> Self {
        Self { path: path.to_path_buf(), lines: Vec::new(), truncated: false, error: None }
    }
    fn failed(path: &Path, error: String) -> Self {
        Self { path: path.to_path_buf(), lines: Vec::new(), truncated: false, error: Some(error) }
    }
}

/// pi `readTextTail` (`fleet-view.ts:92-119`): stat, seek to `size - min(size, TRANSCRIPT_TAIL_BYTES)`,
/// read that window, drop the (possibly partial) first line when the window did not start at byte 0,
/// drop a trailing empty line, and keep the last `max_lines`. A missing file is an EMPTY tail with no
/// error (pi's `isNotFoundError` branch); any other I/O failure is reported as `error`.
fn read_text_tail(path: &Path, max_lines: usize) -> TextTail {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TextTail::empty(path),
        Err(e) => return TextTail::failed(path, e.to_string()),
    };
    let size = meta.len();
    if size == 0 {
        return TextTail::empty(path);
    }
    let bytes_to_read = size.min(TRANSCRIPT_TAIL_BYTES);
    let start = size.saturating_sub(bytes_to_read);
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) => return TextTail::failed(path, e.to_string()),
    };
    if let Err(e) = file.seek(SeekFrom::Start(start)) {
        return TextTail::failed(path, e.to_string());
    }
    let mut buf = Vec::new();
    if let Err(e) = file.take(bytes_to_read).read_to_end(&mut buf) {
        return TextTail::failed(path, e.to_string());
    }
    let content = String::from_utf8_lossy(&buf).into_owned();
    let mut lines: Vec<String> = content.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let truncated = start > 0 || lines.len() > max_lines;
    let kept = lines.split_off(lines.len().saturating_sub(max_lines));
    TextTail { path: path.to_path_buf(), lines: kept, truncated, error: None }
}

/// pi `readContainedTextTail` (`fleet-view.ts:121-148`): refuse outright when there is no trusted
/// root, when the resolved path is outside every trusted root, when it is a symlink, or when it is
/// not a regular file — and re-check containment against the REAL (symlink-resolved) path of both
/// sides before any byte is read.
fn read_contained_text_tail(
    path: &Path,
    max_lines: usize,
    trusted_roots: &[PathBuf],
    label: &str,
) -> TextTail {
    if trusted_roots.is_empty() {
        return TextTail::failed(
            path,
            format!("Refusing to read {label} transcript path without a trusted root: {}", path.display()),
        );
    }
    let resolved = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if !trusted_roots.iter().any(|root| path_within(root, &resolved)) {
        return TextTail::failed(
            path,
            format!("Refusing to read {label} transcript path outside trusted roots: {}", path.display()),
        );
    }
    let lstat = match std::fs::symlink_metadata(&resolved) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TextTail::empty(path),
        Err(e) => return TextTail::failed(path, e.to_string()),
    };
    if lstat.file_type().is_symlink() {
        return TextTail::failed(
            path,
            format!("Refusing to read symlink {label} transcript path: {}", path.display()),
        );
    }
    if !lstat.is_file() {
        return TextTail::failed(
            path,
            format!("Refusing to read non-file {label} transcript path: {}", path.display()),
        );
    }
    let real_path = match std::fs::canonicalize(&resolved) {
        Ok(p) => p,
        Err(e) => return TextTail::failed(path, e.to_string()),
    };
    let real_roots: Vec<PathBuf> = trusted_roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    if !real_roots.iter().any(|root| path_within(root, &real_path)) {
        return TextTail::failed(
            path,
            format!("Refusing to read {label} transcript path outside trusted roots: {}", path.display()),
        );
    }
    read_text_tail(&real_path, max_lines)
}

/// pi `pathWithin` (`fleet-view.ts:75-79`), on already-absolute inputs.
fn path_within(base: &Path, candidate: &Path) -> bool {
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let candidate = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    candidate == base || candidate.starts_with(&base)
}

// =================================================================================================
// Activity / model labels (pi `shared/status-format.ts` + `shared/formatters.ts`)
// =================================================================================================

/// pi `formatActivityAge` (`status-format.ts:5-9`).
fn format_activity_age(ms: i64) -> String {
    if ms < 1000 {
        "now".to_string()
    } else if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else {
        format!("{}m", ms / 60_000)
    }
}

/// pi `formatActivityLabel` (`status-format.ts:11-21`) — the one-phrase "how alive is this child"
/// label the fleet rows lead with.
#[must_use]
pub fn format_activity_label(
    last_activity_at: Option<i64>,
    activity_state: Option<ActivityState>,
    now: i64,
) -> Option<String> {
    let Some(last) = last_activity_at else {
        return match activity_state {
            Some(ActivityState::NeedsAttention) => Some("needs attention".to_string()),
            Some(ActivityState::ActiveLongRunning) => Some("active but long-running".to_string()),
            _ => None,
        };
    };
    let age = format_activity_age((now - last).max(0));
    match activity_state {
        Some(ActivityState::NeedsAttention) => Some(format!("no activity for {age}")),
        Some(ActivityState::ActiveLongRunning) => {
            Some(format!("active but long-running · last activity {age} ago"))
        }
        _ if age == "now" => Some("active now".to_string()),
        _ => Some(format!("active {age} ago")),
    }
}

/// pi `formatTokens` (`shared/formatters.ts`), duplicated from `registration/cost.rs`'s private copy
/// rather than cross-imported: that one is `fn`-private to the cost walk and this module must not
/// reach into it.
fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{}k", (n as f64 / 1000.0).round() as u64)
    }
}

/// pi `formatModelThinking` (`shared/formatters.ts:19-29`): drop the provider prefix from the model
/// ref, and append `thinking <level>` when a thinking level is known. cyrup's [`super::ModelId`]
/// never carries pi's `:thinking-high` style suffix (the level rides on `StepTelemetry::thinking`),
/// so only the explicit-level half of pi's two sources applies.
fn format_model_thinking(model: Option<&str>, thinking: Option<&str>) -> String {
    const THINKING_LEVELS: [&str; 4] = ["off", "low", "medium", "high"];
    let display_model = model.map(|m| match m.rfind('/') {
        Some(i) => m.get(i.saturating_add(1)..).unwrap_or(m),
        None => m,
    });
    let display_thinking = thinking
        .map(str::trim)
        .filter(|t| THINKING_LEVELS.contains(t));
    [display_model.map(str::to_string), display_thinking.map(|t| format!("thinking {t}"))]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The inputs pi's `formatActivityFacts` (`fleet-view.ts:207-226`) reads, gathered off either a
/// [`StepStatus`], a whole [`RunStatus`], or a live foreground entry.
#[derive(Default)]
struct ActivityFacts<'a> {
    activity_state: Option<ActivityState>,
    last_activity_at: Option<i64>,
    current_tool: Option<&'a str>,
    current_tool_started_at: Option<i64>,
    current_path: Option<&'a str>,
    turn_count: Option<u64>,
    tool_count: Option<u64>,
    total_tokens: Option<u64>,
}

/// pi `formatActivityFacts` (`fleet-view.ts:207-226`) — `<activity> | tool X 4s | ~/p | 3 turns |
/// 9 tools | 12.4k tok`, with every absent fact simply skipped and `None` when nothing at all is
/// known.
fn format_activity_facts(input: &ActivityFacts<'_>, now: i64) -> Option<String> {
    let mut facts: Vec<String> = Vec::new();
    match (input.current_tool, input.current_tool_started_at) {
        (Some(tool), Some(started)) => facts.push(format!(
            "tool {tool} {}",
            crate::background::wait::format_duration(u64::try_from((now - started).max(0)).unwrap_or(0))
        )),
        (Some(tool), None) => facts.push(format!("tool {tool}")),
        _ => {}
    }
    if let Some(path) = input.current_path {
        facts.push(crate::exec::tool_call_summary::shorten_path(path));
    }
    if let Some(turns) = input.turn_count {
        facts.push(format!("{turns} turns"));
    }
    if let Some(tools) = input.tool_count {
        facts.push(format!("{tools} tools"));
    }
    if let Some(total) = input.total_tokens.filter(|t| *t != 0) {
        facts.push(format!("{} tok", format_tokens(total)));
    }
    let activity = format_activity_label(input.last_activity_at, input.activity_state, now);
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

fn step_activity_facts<'a>(step: &'a StepStatus) -> ActivityFacts<'a> {
    ActivityFacts {
        activity_state: step.telemetry.activity_state,
        last_activity_at: step.telemetry.last_activity_at,
        current_tool: step.telemetry.current_tool.as_deref(),
        current_tool_started_at: step.telemetry.current_tool_started_at,
        current_path: step.telemetry.current_path.as_deref(),
        turn_count: step.telemetry.turn_count,
        tool_count: step.telemetry.tool_count,
        total_tokens: step.telemetry.tokens.as_ref().map(|t| t.total),
    }
}

fn run_activity_facts<'a>(status: &'a RunStatus) -> ActivityFacts<'a> {
    ActivityFacts {
        activity_state: status.telemetry.activity_state,
        last_activity_at: status.telemetry.last_activity_at,
        current_tool: status.telemetry.current_tool.as_deref(),
        current_tool_started_at: None,
        current_path: None,
        turn_count: status.telemetry.turn_count,
        tool_count: status.telemetry.tool_count,
        total_tokens: status.telemetry.total_tokens.as_ref().map(|t| t.total),
    }
}

// =================================================================================================
// `view: "fleet"` (pi `inspectSubagentFleet`, fleet-view.ts:233-338)
// =================================================================================================

/// One live foreground run as the fleet view sees it — the subset of pi's `foregroundControls`
/// entry that `extension.rs`'s own `ForegroundControlEntry` actually carries (delta 4 in the module
/// docs).
#[derive(Clone, Debug)]
pub struct ForegroundFleetEntry {
    /// The run's id (pi `control.runId`).
    pub run_id: String,
    /// The live message-route agent, when a child step is active (pi `control.currentAgent`).
    pub current_agent: Option<String>,
    /// The live child's flat index (pi `control.currentIndex`).
    pub current_index: Option<usize>,
    /// The run's live control activity state (pi `control.currentActivityState`).
    pub activity_state: Option<ActivityState>,
}

/// pi `formatForegroundFleetLines` (`fleet-view.ts:233-255`).
fn format_foreground_fleet_lines(controls: &[ForegroundFleetEntry], now: i64) -> Vec<String> {
    if controls.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["Foreground runs:".to_string()];
    for control in controls {
        let activity = format_activity_facts(
            &ActivityFacts { activity_state: control.activity_state, ..ActivityFacts::default() },
            now,
        );
        // pi `foregroundModeName`: a single-mode run shows its current agent in the mode slot.
        // cyrup's foreground registry does not record the run mode, so the current agent (when
        // known) is the whole label, exactly as pi's `mode === "single"` branch renders it.
        let mode = control.current_agent.clone().unwrap_or_else(|| "single".to_string());
        let current = control
            .current_agent
            .as_ref()
            .map(|agent| match control.current_index {
                Some(i) => format!(" | {agent} #{i}"),
                None => format!(" | {agent}"),
            })
            .unwrap_or_default();
        let activity_suffix = activity.map(|a| format!(" | {a}")).unwrap_or_default();
        lines.push(format!("- {} | running | {mode}{current}{activity_suffix}", control.run_id));
        lines.push(format!(
            "  status: subagent({{ action: \"status\", id: \"{}\" }})",
            control.run_id
        ));
        lines.push(
            "  transcript: live in the expanded foreground result; persisted session transcript \
             appears after completion when sessions are enabled."
                .to_string(),
        );
    }
    lines
}

/// pi `formatAsyncFleetLines` (`fleet-view.ts:257-293`).
fn format_async_fleet_lines(runs: &[ActiveRun], now: i64) -> Vec<String> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["Async runs:".to_string()];
    for run in runs {
        let status = &run.status;
        let activity = format_activity_facts(&run_activity_facts(status), now)
            .map(|a| format!(" | {a}"))
            .unwrap_or_default();
        let cwd = status
            .cwd
            .as_ref()
            .map_or_else(|| run.dir.clone(), Clone::clone);
        let pending = status
            .pending_appends
            .filter(|count| *count > 0)
            .map(|count| format!(" | {count} pending append{}", if count == 1 { "" } else { "s" }))
            .unwrap_or_default();
        lines.push(format!(
            "- {} | {}{activity} | {} | {}{pending} | {}",
            status.run_id,
            run_state_label(status.state),
            run_mode_label(status.mode),
            progress_label(status),
            crate::exec::tool_call_summary::shorten_path(&cwd.to_string_lossy())
        ));
        lines.push(format!("  status: subagent({{ action: \"status\", id: \"{}\" }})", status.run_id));
        lines.push(format!(
            "  transcript: subagent({{ action: \"status\", id: \"{}\", view: \"transcript\" }})",
            status.run_id
        ));
        for (index, step) in status.steps.iter().enumerate() {
            let step_activity = format_activity_facts(&step_activity_facts(step), now);
            let model_thinking = format_model_thinking(
                step.model.as_ref().map(super::ModelId::as_str),
                step.telemetry.thinking.as_deref(),
            );
            let mut parts = vec![format!("{index}. {}", step.agent), step_state_label(step.status).to_string()];
            if let Some(a) = step_activity {
                parts.push(a);
            }
            if !model_thinking.is_empty() {
                parts.push(model_thinking);
            }
            lines.push(format!("  {}", parts.join(" | ")));
            let output = run.dir.join(format!("output-{index}.log"));
            let output_exists = output.exists();
            if output_exists {
                lines.push(format!(
                    "    output: {}",
                    crate::exec::tool_call_summary::shorten_path(&output.to_string_lossy())
                ));
            }
            if let Some(session) = step.session_file.as_ref() {
                lines.push(format!(
                    "    session: {}",
                    crate::exec::tool_call_summary::shorten_path(&session.to_string_lossy())
                ));
            }
            if step.status == super::StepState::Running
                || !step.telemetry.recent_output.is_empty()
                || output_exists
            {
                lines.push(format!(
                    "    transcript: subagent({{ action: \"status\", id: \"{}\", index: {index}, \
                     view: \"transcript\" }})",
                    status.run_id
                ));
            }
        }
        for step in &status.steps {
            if let Some(error) = step.error.as_ref() {
                lines.push(format!("  error: {error}"));
            }
        }
        if let Some(session) = status.session_file.as_ref() {
            lines.push(format!(
                "  session: {}",
                crate::exec::tool_call_summary::shorten_path(&session.to_string_lossy())
            ));
        }
    }
    lines
}

/// pi `inspectSubagentFleet` (`fleet-view.ts:295-338`) — the whole read-only fleet surface.
///
/// `child_safe` is pi's `deps.childSafe` gate (`:296-302`): a fanout child has no business
/// enumerating its parent's entire async root, so it gets pi's exact refusal instead.
///
/// # Errors
///
/// Returns pi's child-safe refusal text as `Err` (cyrup surfaces `isError: true` as `Err`).
pub fn format_fleet(
    foreground: &[ForegroundFleetEntry],
    runs: &[ActiveRun],
    child_safe: bool,
    now: i64,
) -> Result<String, String> {
    if child_safe {
        return Err("Child-safe subagent fleet view is unavailable without an explicit run id. Use \
                    subagent({ action: \"status\", id: \"...\" }) for the delegated run you can see."
            .to_string());
    }
    let total = foreground.len().saturating_add(runs.len());
    if total == 0 {
        return Ok("No active subagent fleet. Background runs that already finished are available \
                   through completion notifications or subagent({ action: \"status\", id: \"...\" })."
            .to_string());
    }
    let mut lines = vec![format!("Subagent fleet: {total} active"), String::new()];
    let foreground_lines = format_foreground_fleet_lines(foreground, now);
    if !foreground_lines.is_empty() {
        lines.extend(foreground_lines);
        lines.push(String::new());
    }
    let async_lines = format_async_fleet_lines(runs, now);
    if !async_lines.is_empty() {
        lines.extend(async_lines);
        lines.push(String::new());
    }
    lines.push("Commands:".to_string());
    lines.push("  Refresh fleet: subagent({ action: \"status\", view: \"fleet\" })".to_string());
    lines.push(
        "  Tail run transcript: subagent({ action: \"status\", id: \"<run-id>\", view: \"transcript\" })"
            .to_string(),
    );
    lines.push(
        "  Tail child transcript: subagent({ action: \"status\", id: \"<run-id>\", index: 0, view: \"transcript\" })"
            .to_string(),
    );
    Ok(lines.join("\n").trim_end().to_string())
}

// =================================================================================================
// `view: "transcript"` (pi `formatAsyncRunTranscript`, fleet-view.ts:340-449)
// =================================================================================================

/// pi `validateTranscriptIndex` + `selectTranscriptStep` (`fleet-view.ts:363-385`): an explicit
/// index must be in range; otherwise prefer the run's `currentStep` while it is running, then fall
/// back to the sole step of a one-step run.
///
/// # Errors
///
/// Returns pi's out-of-range message when an explicit `index` names no step.
fn select_transcript_step(
    status: &RunStatus,
    index: Option<usize>,
) -> Result<(Option<usize>, Option<String>), String> {
    let steps = &status.steps;
    if let Some(i) = index
        && i >= steps.len()
    {
        return Err(format!(
            "Transcript index {i} is out of range for {} child step{}.",
            steps.len(),
            if steps.len() == 1 { "" } else { "s" }
        ));
    }
    let mut selected = index;
    if selected.is_none() {
        if status.state == super::RunState::Running
            && let Some(cur) = status.current_step
            && cur < steps.len()
        {
            selected = Some(cur);
        } else if steps.len() == 1 {
            selected = Some(0);
        }
    }
    let hint = if index.is_none() && steps.len() > 1 {
        Some(format!(
            "Tip: pass index to inspect a specific child transcript ({}).",
            steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{i}={}", s.agent))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    } else {
        None
    };
    Ok((selected, hint))
}

/// pi `stepStateLine` (`fleet-view.ts:387-399`).
fn step_state_line(status: &RunStatus, index: Option<usize>, now: i64) -> Option<String> {
    let index = index?;
    let step = status.steps.get(index)?;
    let label = if status.mode == super::RunMode::Parallel { "Agent" } else { "Step" };
    let mut parts = vec![
        format!("{label}: {index} ({})", step.agent),
        step_state_label(step.status).to_string(),
    ];
    if let Some(a) = format_activity_facts(&step_activity_facts(step), now) {
        parts.push(a);
    }
    let model_thinking = format_model_thinking(
        step.model.as_ref().map(super::ModelId::as_str),
        step.telemetry.thinking.as_deref(),
    );
    if !model_thinking.is_empty() {
        parts.push(model_thinking);
    }
    if let Some(error) = step.error.as_ref() {
        parts.push(format!("error: {error}"));
    }
    Some(parts.join(" | "))
}

/// pi `appendKnownArtifacts` (`fleet-view.ts:401-411`).
fn append_known_artifacts(lines: &mut Vec<String>, artifacts: &[(&str, String)]) {
    if artifacts.is_empty() {
        return;
    }
    lines.push("Artifacts:".to_string());
    for (label, value) in artifacts {
        lines.push(format!("  {label}: {value}"));
    }
}

/// pi `appendTranscriptBody` (`fleet-view.ts:413-420`).
fn append_transcript_body(lines: &mut Vec<String>, source: &str, body: &[String], truncated: bool) {
    lines.push(format!("{source}{}:", if truncated { " (tail truncated)" } else { "" }));
    if body.is_empty() {
        lines.push("  (no transcript lines available yet)".to_string());
        return;
    }
    for line in body {
        lines.push(format!("  {line}"));
    }
}

/// pi `sessionMessageLine` + `readSessionTranscriptTail` (`fleet-view.ts:176-205`): the last
/// resort — parse the persisted session JSONL tail into `role: text` lines, counting (not
/// swallowing) malformed records.
fn read_session_transcript_tail(
    session_file: &Path,
    max_lines: usize,
    trusted_roots: &[PathBuf],
) -> (Vec<String>, Vec<String>) {
    let tail = read_contained_text_tail(
        session_file,
        max_lines.saturating_mul(4).max(max_lines),
        trusted_roots,
        "session",
    );
    let mut warnings = Vec::new();
    if let Some(error) = tail.error.as_ref() {
        warnings.push(format!("Session read failed for {}: {error}", session_file.display()));
    }
    let mut lines: Vec<String> = Vec::new();
    let mut malformed = 0usize;
    for raw in &tail.lines {
        if raw.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(parsed) => {
                if let Some(line) = session_message_line(&parsed) {
                    lines.push(line);
                }
            }
            Err(_) => malformed = malformed.saturating_add(1),
        }
    }
    if malformed > 0 {
        warnings.push(format!(
            "Skipped {malformed} malformed session tail line{}.",
            if malformed == 1 { "" } else { "s" }
        ));
    }
    let kept = lines.split_off(lines.len().saturating_sub(max_lines));
    (kept, warnings)
}

/// pi `sessionMessageLine` (`fleet-view.ts:176-185`) + `contentText` (`:157-174`).
fn session_message_line(record: &serde_json::Value) -> Option<String> {
    let message = record.get("message").filter(|m| m.is_object()).unwrap_or(record);
    let role = message.get("role")?.as_str()?;
    let text = content_text(message.get("content")).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(format!("{role}: {text}"))
}

/// pi `contentText` (`fleet-view.ts:157-174`) — flatten a message's content into plain text,
/// summarising tool calls/results rather than dropping them.
fn content_text(content: Option<&serde_json::Value>) -> String {
    let Some(content) = content else { return String::new() };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let Some(parts) = content.as_array() else { return String::new() };
    parts
        .iter()
        .map(|part| {
            let Some(entry) = part.as_object() else { return String::new() };
            if let Some(text) = entry.get("text").and_then(serde_json::Value::as_str) {
                return text.to_string();
            }
            let kind = entry.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
            if kind == "toolCall" || kind == "tool_call" {
                let name = entry
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| entry.get("toolName").and_then(serde_json::Value::as_str))
                    .unwrap_or("tool");
                let args = entry
                    .get("args")
                    .map(|a| format!(" {}", stringify_json_preview(a)))
                    .unwrap_or_default();
                return format!("[tool: {name}{args}]");
            }
            if kind == "toolResult" || kind == "tool_result" {
                let result = entry
                    .get("result")
                    .map(|r| format!(": {}", stringify_json_preview(r)))
                    .unwrap_or_default();
                return format!("[tool result{result}]");
            }
            entry
                .get("content")
                .map(stringify_json_preview)
                .unwrap_or_default()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// pi `stringifyJsonPreview` (`fleet-view.ts:150-155`) — 240 chars, then a single-char ellipsis.
fn stringify_json_preview(value: &serde_json::Value) -> String {
    let raw = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default());
    if raw.chars().count() > 240 {
        let head: String = raw.chars().take(240).collect();
        format!("{head}…")
    } else {
        raw
    }
}

/// pi `formatAsyncRunTranscript` (`fleet-view.ts:398-449`) — the whole `view: "transcript"` body
/// for one background run, with pi's three-source fallback ladder (step/run output log →
/// `recentOutput` ring → persisted session JSONL) and its bounded, containment-checked reads.
///
/// # Errors
///
/// Returns pi's out-of-range message when an explicit `index` names no step.
pub fn format_async_run_transcript(
    status: &RunStatus,
    paths: &RunPaths,
    index: Option<usize>,
    lines_param: Option<i64>,
    session_roots: &[PathBuf],
) -> Result<String, String> {
    let line_limit = transcript_line_limit(lines_param);
    let (selected, hint) = select_transcript_step(status, index)?;
    let async_dir = paths.run_dir.as_path();

    let mut lines: Vec<String> = vec![
        format!("Run: {}", status.run_id),
        format!("State: {}", run_state_label(status.state)),
        format!("Mode: {}", run_mode_label(status.mode)),
    ];
    let now = super::now_epoch_millis();
    if let Some(line) = step_state_line(status, selected, now) {
        lines.push(line);
    }
    if let Some(hint) = hint {
        lines.push(hint);
    }

    // pi's `outputPaths`: the selected child's own `output-<i>.log`, else the run-level output
    // artifact (delta 3 — cyrup keeps that per step, so the no-index form falls back to the step's
    // own recorded `output_file`).
    let output_paths: Vec<PathBuf> = match selected {
        Some(i) => vec![paths.step_output_log(i)],
        None => status
            .steps
            .first()
            .and_then(|s| s.telemetry.output_file.clone())
            .into_iter()
            .collect(),
    };
    let session_file = match selected {
        Some(i) => status.steps.get(i).and_then(|s| s.session_file.clone()),
        None => status.session_file.clone(),
    };

    let mut artifacts: Vec<(&str, String)> = output_paths
        .iter()
        .map(|p| ("Output", p.display().to_string()))
        .collect();
    if let Some(session) = session_file.as_ref() {
        artifacts.push(("Session", session.display().to_string()));
    }
    if paths.events.exists() {
        artifacts.push(("Events", paths.events.display().to_string()));
    }
    if paths.run_log_md.exists() {
        artifacts.push(("Log", paths.run_log_md.display().to_string()));
    }
    append_known_artifacts(&mut lines, &artifacts);

    let mut warnings: Vec<String> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let mut source = "Transcript tail".to_string();
    let mut truncated = false;
    for output_path in &output_paths {
        let tail = read_contained_text_tail(output_path, line_limit, &[async_dir.to_path_buf()], "output");
        if let Some(error) = tail.error.as_ref() {
            warnings.push(format!("Output read failed for {}: {error}", tail.path.display()));
        }
        if tail.lines.is_empty() {
            continue;
        }
        source = format!("Transcript tail from {}", tail.path.display());
        truncated = tail.truncated;
        body = tail.lines;
        break;
    }
    if body.is_empty()
        && let Some(step) = selected.and_then(|i| status.steps.get(i))
        && !step.telemetry.recent_output.is_empty()
    {
        let recent = &step.telemetry.recent_output;
        body = recent
            .get(recent.len().saturating_sub(line_limit)..)
            .unwrap_or(recent)
            .to_vec();
        source = "Recent output from status.json".to_string();
    }
    if body.is_empty()
        && let Some(session) = session_file.as_ref()
    {
        let (session_lines, session_warnings) =
            read_session_transcript_tail(session, line_limit, session_roots);
        warnings.extend(session_warnings);
        if !session_lines.is_empty() {
            source = format!("Session transcript tail from {}", session.display());
        }
        body = session_lines;
    }

    if !warnings.is_empty() {
        lines.push("Warnings:".to_string());
        for warning in &warnings {
            lines.push(format!("  {warning}"));
        }
    }
    append_transcript_body(&mut lines, &source, &body, truncated);
    Ok(lines.join("\n"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::background::{RunId, RunMode, RunState, StepState};

    fn status_with(steps: Vec<StepStatus>) -> RunStatus {
        let mut status = RunStatus::queued(RunId::from_token("run1234".to_string()), RunMode::Chain, Some(1));
        status.state = RunState::Running;
        status.steps = steps;
        status
    }

    #[test]
    fn transcript_line_limit_defaults_and_clamps_like_pi() {
        assert_eq!(transcript_line_limit(None), 80);
        assert_eq!(transcript_line_limit(Some(0)), 1);
        assert_eq!(transcript_line_limit(Some(-5)), 1);
        assert_eq!(transcript_line_limit(Some(120)), 120);
        assert_eq!(transcript_line_limit(Some(9999)), 500);
    }

    #[test]
    fn activity_label_matches_pis_five_branches() {
        let now = 1_000_000;
        assert_eq!(format_activity_label(None, None, now), None);
        assert_eq!(
            format_activity_label(None, Some(ActivityState::NeedsAttention), now).as_deref(),
            Some("needs attention")
        );
        assert_eq!(
            format_activity_label(None, Some(ActivityState::ActiveLongRunning), now).as_deref(),
            Some("active but long-running")
        );
        assert_eq!(format_activity_label(Some(now), None, now).as_deref(), Some("active now"));
        assert_eq!(
            format_activity_label(Some(now - 5_000), None, now).as_deref(),
            Some("active 5s ago")
        );
        assert_eq!(
            format_activity_label(Some(now - 5_000), Some(ActivityState::NeedsAttention), now).as_deref(),
            Some("no activity for 5s")
        );
        assert_eq!(
            format_activity_label(Some(now - 120_000), Some(ActivityState::ActiveLongRunning), now)
                .as_deref(),
            Some("active but long-running · last activity 2m ago")
        );
    }

    #[test]
    fn empty_fleet_renders_pis_sentinel_and_child_safe_refuses() {
        assert_eq!(
            format_fleet(&[], &[], false, 0).unwrap(),
            "No active subagent fleet. Background runs that already finished are available through \
             completion notifications or subagent({ action: \"status\", id: \"...\" })."
        );
        let err = format_fleet(&[], &[], true, 0).unwrap_err();
        assert!(err.starts_with("Child-safe subagent fleet view is unavailable"), "{err}");
    }

    #[test]
    fn fleet_renders_the_foreground_block_and_the_command_footer() {
        let text = format_fleet(
            &[ForegroundFleetEntry {
                run_id: "fg0001".to_string(),
                current_agent: Some("reviewer".to_string()),
                current_index: Some(2),
                activity_state: Some(ActivityState::NeedsAttention),
            }],
            &[],
            false,
            0,
        )
        .unwrap();
        assert!(text.starts_with("Subagent fleet: 1 active"), "{text}");
        assert!(text.contains("Foreground runs:"), "{text}");
        assert!(
            text.contains("- fg0001 | running | reviewer | reviewer #2 | needs attention"),
            "{text}"
        );
        assert!(text.contains("  status: subagent({ action: \"status\", id: \"fg0001\" })"), "{text}");
        assert!(
            text.contains("  Tail child transcript: subagent({ action: \"status\", id: \"<run-id>\", index: 0, view: \"transcript\" })"),
            "{text}"
        );
    }

    #[test]
    fn transcript_rejects_an_out_of_range_index_with_pis_message() {
        let status = status_with(vec![StepStatus::pending("alpha")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = RunPaths::for_run(dir.path(), dir.path(), &status.run_id);
        let err = format_async_run_transcript(&status, &paths, Some(3), None, &[]).unwrap_err();
        assert_eq!(err, "Transcript index 3 is out of range for 1 child step.");
    }

    #[test]
    fn transcript_reads_the_step_output_log_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut status = status_with(vec![StepStatus::pending("alpha")]);
        status.current_step = Some(0);
        let paths = RunPaths::for_run(dir.path(), dir.path(), &status.run_id);
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir");
        std::fs::write(paths.step_output_log(0), "line one\nline two\nline three\n").expect("write");

        let text = format_async_run_transcript(&status, &paths, None, Some(2), &[]).unwrap();
        assert!(text.contains("Run: run1234"), "{text}");
        assert!(text.contains("Step: 0 (alpha)"), "{text}");
        assert!(text.contains("Artifacts:"), "{text}");
        assert!(text.contains("  line two"), "{text}");
        assert!(text.contains("  line three"), "{text}");
        assert!(!text.contains("  line one"), "the 2-line limit must drop the oldest line: {text}");
    }

    #[test]
    fn transcript_falls_back_to_the_recent_output_ring_then_reports_no_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut step = StepStatus::pending("alpha");
        step.status = StepState::Running;
        step.telemetry.recent_output = vec!["ring a".to_string(), "ring b".to_string()];
        let mut status = status_with(vec![step]);
        status.current_step = Some(0);
        let paths = RunPaths::for_run(dir.path(), dir.path(), &status.run_id);

        let text = format_async_run_transcript(&status, &paths, None, None, &[]).unwrap();
        assert!(text.contains("Recent output from status.json:"), "{text}");
        assert!(text.contains("  ring b"), "{text}");

        let bare = status_with(vec![StepStatus::pending("alpha")]);
        let bare_paths = RunPaths::for_run(dir.path(), dir.path(), &bare.run_id);
        let text = format_async_run_transcript(&bare, &bare_paths, None, None, &[]).unwrap();
        assert!(text.contains("(no transcript lines available yet)"), "{text}");
    }

    #[test]
    fn a_session_transcript_outside_every_trusted_root_is_refused_not_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = dir.path().join("secrets.jsonl");
        std::fs::write(&outside, "{\"role\":\"user\",\"content\":\"leak me\"}\n").expect("write");
        let (lines, warnings) = read_session_transcript_tail(&outside, 10, &[]);
        assert!(lines.is_empty(), "no trusted root must mean no read: {lines:?}");
        assert!(
            warnings.iter().any(|w| w.contains("without a trusted root")),
            "{warnings:?}"
        );

        let (lines, warnings) = read_session_transcript_tail(&outside, 10, &[dir.path().to_path_buf()]);
        assert_eq!(lines, vec!["user: leak me".to_string()], "{warnings:?}");
    }

    #[test]
    fn a_multi_step_run_with_no_index_emits_pis_child_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut status = status_with(vec![StepStatus::pending("alpha"), StepStatus::pending("beta")]);
        status.current_step = None;
        status.state = RunState::Paused;
        let paths = RunPaths::for_run(dir.path(), dir.path(), &status.run_id);
        let text = format_async_run_transcript(&status, &paths, None, None, &[]).unwrap();
        assert!(
            text.contains("Tip: pass index to inspect a specific child transcript (0=alpha, 1=beta)."),
            "{text}"
        );
    }
}
