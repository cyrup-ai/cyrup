//! The transcript / history view (R-10-028; arch-10 §2 `chrome/messages.rs`).
//!
//! Renders the conversation: user messages, assistant text (including the in-flight streaming
//! partial), and tool-execution start/end markers. The view owns a flat list of [`Entry`]s plus a
//! single mutable streaming buffer for the assistant turn currently being produced.
//!
//! ## Dependency note (driving from `AgentSessionEvent`)
//! `cyrup-tui` depends directly on `cyrup-provider` (Cargo.toml), so the streaming-delta variants of
//! `StreamEvent` *are* pattern-matched: [`crate::app::App::ingest_event`] folds
//! `MessageUpdate`'s `assistant_message_event` and appends `TextDelta { delta, .. }` here via
//! [`push_assistant_delta`](TranscriptView::push_assistant_delta), so the viewport grows
//! token-by-token like Pi's interactive stream. The terminal assistant message is recovered via
//! `StreamEvent::terminal_message()` (yielding a `&cyrup_core::AssistantMessage`) and replaces the
//! partial on `Done`/`Error`. The neutral [`TranscriptView`] API
//! ([`push_user`](TranscriptView::push_user),
//! [`push_assistant_delta`](TranscriptView::push_assistant_delta), …) remains the integration seam
//! an adapter feeds.

use cyrup_core::Content;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::bash::BashExecution;
use crate::component::Component;
use crate::theme::UiTheme;

/// A committed transcript entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A user submission.
    User(String),
    /// A finalized assistant message.
    Assistant(String),
    /// A finished tool execution (`tool-execution.ts`): name + an optional one-line argument summary
    /// + an optional result body (rendered as a unified diff when it looks like one) + error flag.
    Tool(ToolRun),
    /// A status / notification line (model change, compaction, queue, …).
    Status(String),
    /// A bordered info block (`/hotkeys`, `/changelog`, `/session`, `/debug`): a top `DynamicBorder`,
    /// a bold-accent `title`, a blank, the `markdown` body, then a bottom `DynamicBorder`
    /// (interactive-mode.ts:5502-5507).
    Block { title: String, markdown: String },
    /// A finished `!`/`!!` bash execution (`bash-execution.ts`): the command header + output block,
    /// committed to scrollback when the process exits.
    Bash(BashExecution),
    /// A skill-invocation message (`skill-invocation-message.ts`): a bold `[skill]` label + the skill
    /// name, with the skill block content rendered as markdown below.
    SkillInvocation { name: String, content: String },
    /// A custom (extension) message (`custom-message.ts`): a bracketed type `label` + a markdown
    /// `body`, styled distinctly from a plain user message.
    Custom { label: String, body: String },
    /// A branch-summary message (`branch-summary-message.ts`): a bold `[branch]` label + the
    /// `**Branch Summary**` markdown body produced when navigating away from a branch.
    BranchSummary(String),
    /// A compaction-summary message (`compaction-summary-message.ts`): a bold `[compaction]` label
    /// noting the pre-compaction token count + the `**Compacted from N tokens**` summary markdown.
    CompactionSummary { tokens_before: u64, summary: String },
}

/// One tool execution, shown live in the viewport while it runs (`tool-execution.ts` pending box) and
/// committed to scrollback when the turn ends. `expanded` rendering shows the full result body; the
/// collapsed form shows only the marker line + a truncated preview (`Ctrl+O` toggles, `app.tools.expand`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRun {
    /// Tool name (`read`, `bash`, `edit`, …).
    pub name: String,
    /// A one-line argument summary (e.g. a file path or command), if derivable.
    pub args_summary: Option<String>,
    /// The result/output body (multi-line allowed). `None` while still running.
    pub result: Option<String>,
    /// Whether the tool failed.
    pub is_error: bool,
    /// Whether the execution has finished (drives the `⚙ …`-running vs `✓/✗`-done marker).
    pub done: bool,
}

/// The scrolling conversation history.
///
/// Per ADR-0001 / R-ARCH-TUI-003 the inline viewport holds only the **active** region (the
/// in-flight streaming turn). Entries are *committed* the moment they are pushed and are buffered in
/// `pending` until the shell drains them via [`drain_committed`](TranscriptView::drain_committed),
/// at which point they are emitted to the terminal's native scrollback with `Terminal::insert_before`
/// and never re-rendered inside the viewport.
#[derive(Default)]
pub struct TranscriptView {
    /// Committed entries awaiting a flush to native scrollback (drained, not re-rendered).
    pending: Vec<Entry>,
    /// The assistant turn currently streaming, if any (the only entry the viewport renders).
    streaming: Option<String>,
    /// Tool executions for the active turn, rendered live in the viewport until the turn ends, then
    /// committed (`tool-execution.ts` keeps tool components live in the message region). Honors the
    /// shared `tool_expanded` flag so `Ctrl+O` visibly expands/collapses in-flight tool output.
    active_tools: Vec<ToolRun>,
    /// Whether tool output renders expanded (full result) vs collapsed (`Ctrl+O`, `app.tools.expand`).
    pub tool_expanded: bool,
    /// The live `!`/`!!` bash execution, if one is running/just-finished and not yet committed
    /// (`bash-execution.ts` — the component stays live in the message region, then scrolls up).
    bash: Option<BashExecution>,
    /// Live key labels (`Esc` / `Ctrl+O`) for the bash block's cancel + expand hints, set by the app
    /// from the keymap so rebinds reflect. `None` falls back to the Pi defaults.
    bash_cancel_hint: Option<String>,
    bash_expand_hint: Option<String>,
    /// How many visual lines the user has paged **up** from the tail of the active region
    /// (`PageUp`/`PageDown`, spec/tui/07). `0` keeps the newest text pinned to the bottom (the
    /// default auto-scroll); paging up reveals earlier streamed/tool/bash output before it commits to
    /// native scrollback. Reset to `0` whenever new content lands so live streaming stays visible.
    scroll_offset: usize,
}

impl TranscriptView {
    /// An empty transcript.
    pub fn new() -> Self {
        TranscriptView::default()
    }

    /// Committed entries not yet flushed to scrollback (test/inspection access).
    pub fn pending(&self) -> &[Entry] {
        &self.pending
    }

    /// The current streaming partial, if a turn is in flight.
    pub fn streaming(&self) -> Option<&str> {
        self.streaming.as_deref()
    }

    /// True while an assistant turn is actively streaming **or** a tool/bash run is live in the viewport.
    pub fn has_active(&self) -> bool {
        self.streaming.is_some() || !self.active_tools.is_empty() || self.bash.is_some()
    }

    /// Start a live `!`/`!!` bash execution block (replaces any prior uncommitted one). `cancel_hint`
    /// / `expand_hint` are the live key labels for the running + expand hints.
    pub fn start_bash(
        &mut self,
        command: impl Into<String>,
        excluded: bool,
        cancel_hint: Option<String>,
        expand_hint: Option<String>,
    ) {
        self.bash = Some(BashExecution::new(command, excluded));
        self.bash_cancel_hint = cancel_hint;
        self.bash_expand_hint = expand_hint;
    }

    /// Append a streamed chunk to the live bash block (`appendOutput`). No-op if none is live.
    pub fn bash_append(&mut self, chunk: &str) {
        if let Some(b) = self.bash.as_mut() {
            b.append_output(chunk);
        }
    }

    /// Mark the live bash block finished (`setComplete`). No-op if none is live.
    pub fn bash_complete(&mut self, exit_code: Option<i32>, cancelled: bool) {
        if let Some(b) = self.bash.as_mut() {
            b.set_complete(exit_code, cancelled);
        }
    }

    /// Whether a bash block is live (running or finished-but-uncommitted).
    pub fn has_bash(&self) -> bool {
        self.bash.is_some()
    }

    /// Whether the live bash block is still running.
    pub fn bash_running(&self) -> bool {
        self.bash.as_ref().is_some_and(BashExecution::is_running)
    }

    /// The live bash block (test/inspection access).
    pub fn bash(&self) -> Option<&BashExecution> {
        self.bash.as_ref()
    }

    /// Toggle the live bash block's expansion (`Ctrl+O`); returns the new state if a block is live.
    pub fn toggle_bash_expanded(&mut self) -> Option<bool> {
        self.bash.as_mut().map(|b| {
            let next = !b.expanded();
            b.set_expanded(next);
            next
        })
    }

    /// Commit the live bash block to scrollback (called once it has finished). A still-running block
    /// is committed as-is (e.g. on interrupt). No-op when none is live.
    pub fn commit_bash(&mut self) {
        if let Some(b) = self.bash.take() {
            self.pending.push(Entry::Bash(b));
        }
    }

    /// Take every committed entry, leaving the pending buffer empty. The shell renders the returned
    /// entries into native scrollback exactly once (R-ARCH-TUI-003), so they are not shown again in
    /// the inline viewport.
    pub fn drain_committed(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.pending)
    }

    /// Append a user message. When `text` is a `<skill …>` block (a `/skill:name` expansion), it is
    /// split into a collapsible `[skill]` invocation message plus the trailing user message, exactly
    /// as Pi renders the `user` role (`parseSkillBlock` → `SkillInvocationMessageComponent` +
    /// `UserMessageComponent`, interactive-mode.ts:3112-3132). Plain text falls through to a single
    /// user entry.
    pub fn push_user(&mut self, text: impl Into<String>) {
        let text = text.into();
        if let Some(block) = parse_skill_block(&text) {
            self.pending
                .push(Entry::SkillInvocation { name: block.name, content: block.content });
            if let Some(user_message) = block.user_message {
                self.pending.push(Entry::User(user_message));
            }
        } else {
            self.pending.push(Entry::User(text));
        }
        // A fresh prompt jumps the active region back to the tail (spec/tui/07 auto-scroll).
        self.scroll_offset = 0;
    }

    /// Page the active region up by `page` visual lines (`PageUp`): reveal earlier streamed/tool/bash
    /// output. Clamped against the content height at render time.
    pub fn page_up(&mut self, page: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(page.max(1));
    }

    /// Page the active region down by `page` visual lines (`PageDown`); `0` is the pinned tail.
    pub fn page_down(&mut self, page: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(page.max(1));
    }

    /// The current page-scroll offset from the tail (test/inspection access).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Append a chunk of assistant text to the in-flight streaming buffer (R-10-028 streaming).
    pub fn push_assistant_delta(&mut self, delta: &str) {
        match &mut self.streaming {
            Some(buf) => buf.push_str(delta),
            None => self.streaming = Some(delta.to_string()),
        }
    }

    /// Finalize the assistant turn. If `text` is given it replaces the streaming buffer (e.g. the
    /// authoritative terminal message); otherwise the accumulated streaming buffer is committed.
    pub fn commit_assistant(&mut self, text: Option<String>) {
        let final_text = text.or_else(|| self.streaming.take());
        self.streaming = None;
        if let Some(t) = final_text
            && !t.is_empty()
        {
            self.pending.push(Entry::Assistant(t));
        }
    }

    /// Drop any in-flight streaming partial without committing (abort, R-10-030).
    pub fn discard_streaming(&mut self) {
        self.streaming = None;
    }

    /// Record a tool starting (live in the viewport): name + optional one-line argument summary.
    pub fn push_tool_start(&mut self, name: impl Into<String>, args_summary: Option<String>) {
        self.active_tools.push(ToolRun {
            name: name.into(),
            args_summary,
            result: None,
            is_error: false,
            done: false,
        });
    }

    /// Update the latest still-running tool's partial result (`ToolExecutionUpdate`).
    pub fn push_tool_update(&mut self, partial: Option<String>) {
        if let Some(run) = self.active_tools.iter_mut().rev().find(|r| !r.done)
            && partial.is_some()
        {
            run.result = partial;
        }
    }

    /// Record a tool finishing: attach the result/error to the matching live run (the latest run with
    /// that name still running, else a fresh done entry so a missed start never drops the result).
    pub fn push_tool_end(
        &mut self,
        name: impl Into<String>,
        is_error: bool,
        result: Option<String>,
    ) {
        let name = name.into();
        if let Some(run) = self.active_tools.iter_mut().rev().find(|r| !r.done && r.name == name) {
            run.done = true;
            run.is_error = is_error;
            run.result = result;
        } else {
            self.active_tools.push(ToolRun {
                name,
                args_summary: None,
                result,
                is_error,
                done: true,
            });
        }
    }

    /// Commit the active turn's tool executions into scrollback (called when the turn ends). Each
    /// becomes an [`Entry::Tool`]; still-running tools are committed as-is (marked done).
    pub fn commit_tools(&mut self) {
        for mut run in self.active_tools.drain(..) {
            run.done = true;
            self.pending.push(Entry::Tool(run));
        }
    }

    /// The active (live) tool executions for the current turn (test/inspection access).
    pub fn active_tools(&self) -> &[ToolRun] {
        &self.active_tools
    }

    /// Toggle the tool-output expansion (`Ctrl+O`); returns the new state.
    pub fn toggle_tool_expanded(&mut self) -> bool {
        self.tool_expanded = !self.tool_expanded;
        self.tool_expanded
    }

    /// Record a status / notification line.
    pub fn push_status(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Status(text.into()));
    }

    /// Push a bordered info block (`/hotkeys`, `/changelog`, `/session`, `/debug`).
    pub fn push_block(&mut self, title: impl Into<String>, markdown: impl Into<String>) {
        self.pending.push(Entry::Block { title: title.into(), markdown: markdown.into() });
    }

    /// Push a skill-invocation message (`skill-invocation-message.ts`): a `[skill]` label + the skill
    /// name header, with the skill block content rendered as markdown.
    pub fn push_skill_invocation(&mut self, name: impl Into<String>, content: impl Into<String>) {
        self.pending.push(Entry::SkillInvocation { name: name.into(), content: content.into() });
    }

    /// Push a custom (extension) message (`custom-message.ts`): a bracketed type `label` + a markdown
    /// `body`.
    pub fn push_custom_message(&mut self, label: impl Into<String>, body: impl Into<String>) {
        self.pending.push(Entry::Custom { label: label.into(), body: body.into() });
    }

    /// Push a branch-summary message (`branch-summary-message.ts`): the `**Branch Summary**` body
    /// produced when navigating away from / abandoning a branch.
    pub fn push_branch_summary(&mut self, summary: impl Into<String>) {
        self.pending.push(Entry::BranchSummary(summary.into()));
    }

    /// Push a compaction-summary message (`compaction-summary-message.ts`): the pre-compaction token
    /// count + the `**Compacted from N tokens**` summary body.
    pub fn push_compaction_summary(&mut self, tokens_before: u64, summary: impl Into<String>) {
        self.pending
            .push(Entry::CompactionSummary { tokens_before, summary: summary.into() });
    }

    /// Build the styled lines the inline viewport renders: **only** the active streaming partial,
    /// rendered as markdown (spec/tui/06 §8). Committed entries live in native scrollback (see
    /// [`drain_committed`](Self::drain_committed)).
    ///
    /// Pi renders the in-flight assistant message **inline** with no surrounding box/title
    /// (`assistant-message.ts:84-93`); a dim soft cursor `▌` trails the last grapheme while the turn
    /// streams (spec/tui/01 §3) — the hardware cursor stays on the editor. The buffer is run through
    /// [`trim_partial_closing_fence`](crate::markdown::trim_partial_closing_fence) so a streaming code
    /// fence does not flicker open/closed (`markdown.ts:25-48`).
    /// The number of visual lines the active turn occupies at `width` — the message region's content
    /// height, used to **content-size** the inline viewport (ADR-0001 commitment #1, audit #1) so the
    /// empty turn never balloons into a void. `0` when nothing is streaming.
    pub fn content_height(&self, width: usize, theme: &UiTheme) -> usize {
        // Measure WRAPPED display rows, not logical lines: `markdown::render` emits ONE un-wrapped
        // `Line` per prose paragraph (width is only consumed for tables/hr/code), so counting
        // `lines().len()` under-counts a long streaming paragraph and the region is sized too short —
        // clipping the newest text + the `▌` caret. `wrapped_height` measures with the SAME word-wrap
        // `render` applies (`Paragraph::line_count`), so the active turn grows + stays tail-anchored
        // (spec/tui/01 §3 overflow; the doc at [`TranscriptView::render`]).
        wrapped_height(&self.lines(width, theme), width)
    }

    fn lines(&self, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if let Some(partial) = &self.streaming {
            let body = crate::markdown::trim_partial_closing_fence(partial);
            let mut md = crate::markdown::render(&body, width.saturating_sub(11).max(1), theme);
            if md.is_empty() {
                md.push(Line::default());
            }
            if let Some(first) = md.first_mut() {
                first.spans.insert(0, Span::styled("assistant: ", theme.accent_style()));
            }
            if let Some(last) = md.last_mut() {
                last.spans.push(Span::styled("▌", theme.dim_style()));
            }
            lines.extend(md);
        }
        // Live tool executions render below the streaming partial, honoring the expand flag so
        // `Ctrl+O` toggles their result body in the viewport before the turn commits.
        for run in &self.active_tools {
            lines.extend(tool_lines(run, self.tool_expanded, width, theme));
        }
        // The live `!`/`!!` bash block renders last (`bash-execution.ts` sits in the message region).
        if let Some(b) = &self.bash {
            lines.extend(b.render_lines(
                width,
                theme,
                self.bash_cancel_hint.as_deref(),
                self.bash_expand_hint.as_deref(),
            ));
        }
        lines
    }
}

/// Render one tool execution into styled lines (`tool-execution.ts`): a marker line
/// (`⚙ name(args)…` running · `✓ name(args)` ok · `✗ name(args)` error) plus, when there is a result,
/// either the full body (`expanded`) or a one-line preview (collapsed). A result that looks like a
/// unified diff is rendered via [`crate::diff::render_diff`] (`renderResult` diff path, `diff.ts`).
pub(crate) fn tool_lines(
    run: &ToolRun,
    expanded: bool,
    width: usize,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let header_fg = if run.is_error {
        theme.error_style()
    } else if run.done {
        theme.success_style()
    } else {
        theme.dim_style()
    };
    // The whole block is tinted by execution state (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`,
    // tool-execution.ts:253-258, spec/tui/06 §5.1) — the bg is the affordance, not a box (audit #7).
    let header_style = theme.tool_bg_style(header_fg, run.done, run.is_error);
    let body_style = theme.tool_bg_style(theme.muted_style(), run.done, run.is_error);
    let hint_style = theme.tool_bg_style(theme.dim_style(), run.done, run.is_error);
    let mark = if !run.done {
        "⚙"
    } else if run.is_error {
        "✗"
    } else {
        "✓"
    };
    let mut head = match &run.args_summary {
        Some(args) => format!("  {mark} {}({args})", run.name),
        None => format!("  {mark} {}", run.name),
    };
    if !run.done {
        head.push('…');
    }
    let mut out = vec![Line::styled(pad_to(head, width), header_style)];
    if let Some(result) = run.result.as_deref().filter(|r| !r.trim().is_empty()) {
        if looks_like_diff(result) {
            // Diffs always render in full (the change set is the point), 2-space indented.
            for mut line in crate::diff::render_diff(result, theme) {
                line.spans.insert(0, Span::styled("    ".to_string(), body_style));
                out.push(line);
            }
        } else {
            // The dominant agent surface is the spec block (spec/tui/06 §5.4), NOT a head-1 one-liner
            // (audit #7): collapsed shows the **tail** of the last `TOOL_PREVIEW_LINES` logical lines
            // (`bash-execution.ts:19`), expanded shows all; a `… N more lines (ctrl+o)` hint counts the
            // hidden head.
            let all: Vec<&str> = result.split('\n').collect();
            let total = all.len();
            let shown = if expanded { total } else { total.min(TOOL_PREVIEW_LINES) };
            let hidden = total.saturating_sub(shown);
            if hidden > 0 {
                out.push(Line::styled(
                    pad_to(format!("    … {hidden} more lines (ctrl+o)"), width),
                    hint_style,
                ));
            }
            for raw in all.into_iter().skip(hidden) {
                out.push(Line::styled(pad_to(format!("    {raw}"), width), body_style));
            }
        }
    }
    out
}

/// Collapsed tool/bash result preview length — the tail of this many logical lines
/// (`bash-execution.ts:19`; the tool path mirrors it, spec/tui/06 §5.4).
pub(crate) const TOOL_PREVIEW_LINES: usize = 20;

/// Right-pad `s` with spaces to `width` columns so a background tint fills the full content width
/// (`applyBackgroundToLine`, markdown.ts:216). Char-based; CJK visible-width is a tracked residual.
fn pad_to(s: String, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s
    } else {
        format!("{s}{:pad$}", "", pad = width - len)
    }
}

/// Heuristic: does `text` look like a pre-formatted unified diff (a majority of non-empty lines start
/// with `+`/`-`/space followed by a line-number column)? Mirrors the edit-tool `renderResult` diff
/// detection (`diff.ts` operates on exactly this shape).
fn looks_like_diff(text: &str) -> bool {
    let mut diffish = 0usize;
    let mut total = 0usize;
    for line in text.split('\n').filter(|l| !l.trim().is_empty()) {
        total += 1;
        let mut chars = line.chars();
        if let Some(c) = chars.next()
            && matches!(c, '+' | '-')
            && chars.next().map(|n| n == ' ' || n.is_ascii_digit()).unwrap_or(false)
        {
            diffish += 1;
        }
    }
    total > 0 && diffish * 2 >= total
}

/// The number of WRAPPED display rows `lines` occupy at `width`, using the **same** word-wrap
/// `ratatui`'s `Paragraph::render` applies with `.wrap(Wrap { trim: false })`. ratatui 0.30's
/// `Paragraph::line_count(width)` runs the identical `WordWrapper` the renderer does, so the measured
/// height EXACTLY matches what render produces — the fix for the PROSE-WRAP truncation: a single
/// long paragraph (one logical [`Line`]) wraps to many display rows, and both the content-sized live
/// viewport ([`Transcript::content_height`]) and the scrollback flush must size to the wrapped count,
/// not the logical line count. `width == 0` degrades to the logical count (nothing renders anyway).
pub(crate) fn wrapped_height(lines: &[Line<'static>], width: usize) -> usize {
    if width == 0 {
        return lines.len();
    }
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.min(u16::MAX as usize) as u16)
}

/// Render a single committed [`Entry`] into its styled scrollback line(s) at content `width`
/// (R-ARCH-TUI-003). Used by the shell to feed drained entries into `Terminal::insert_before`.
///
/// Assistant bodies render as **markdown** (spec/tui/06 §2) — multiple lines — with an `assistant: `
/// accent label prefixed onto the first line so the conversation stays grep-legible. User/tool/status
/// entries stay one line each.
pub(crate) fn entry_lines(entry: &Entry, theme: &UiTheme, width: usize) -> Vec<Line<'static>> {
    match entry {
        Entry::User(text) => {
            // The rich user-message render (`user-message.ts`): the submitted text is rendered as
            // multi-line **markdown** (Pi `UserMessageComponent` wraps a `Markdown` in a
            // `userMessageBg` box), with a `you:` accent label prefixed onto the first line so the
            // conversation stays grep-legible — mirroring the assistant arm. (Pi's `userMessageBg`
            // box background and the OSC-133 shell-zone markers wrapping the block are terminal
            // shell-integration escapes that the ratatui cell grid / `insert_before` scrollback model
            // cannot carry; the markdown body + user accent is the in-crate fidelity.)
            let mut md = crate::markdown::render(text, width.saturating_sub(5).max(1), theme);
            if md.is_empty() {
                md.push(Line::default());
            }
            if let Some(first) = md.first_mut() {
                first.spans.insert(0, Span::styled("you: ", theme.user_style()));
            }
            // Project the `userMessageBg` role onto the block (audit #6: the bg roles were dead; the
            // "ratatui can't carry bg" claim was wrong — `insert_before` writes cell bg fine). No-op
            // when the theme omits the role (terminal default shows through).
            let user_bg = theme.user_message_bg_style();
            for line in &mut md {
                line.style = line.style.patch(user_bg);
            }
            md
        }
        Entry::Assistant(text) => {
            let mut md = crate::markdown::render(text, width.saturating_sub(11).max(1), theme);
            if md.is_empty() {
                md.push(Line::default());
            }
            if let Some(first) = md.first_mut() {
                first.spans.insert(0, Span::styled("assistant: ", theme.accent_style()));
            }
            md
        }
        Entry::Tool(run) => {
            // Committed tools render in their last (expanded-at-commit) form; a diff result always
            // renders in full. We commit with the at-the-time expand flag captured by the caller —
            // here we always show the full body so finalized scrollback keeps the complete record.
            tool_lines(run, true, width, theme)
        }
        Entry::Bash(b) => {
            // Committed bash blocks render in full (the complete record), like committed tools.
            let mut full = b.clone();
            full.set_expanded(true);
            full.render_lines(width, theme, None, None)
        }
        Entry::SkillInvocation { name, content } => {
            // `[skill]` label + bold name header, full content as markdown (the committed/expanded
            // form — `skill-invocation-message.ts` expanded branch).
            labeled_message_lines("skill", &format!("**{name}**"), content, theme, width)
        }
        Entry::Custom { label, body } => {
            // A bracketed extension-type label + the markdown body (`custom-message.ts`).
            labeled_message_lines(label, "", body, theme, width)
        }
        Entry::BranchSummary(summary) => {
            labeled_message_lines("branch", "**Branch Summary**", summary, theme, width)
        }
        Entry::CompactionSummary { tokens_before, summary } => {
            let header = format!("**Compacted from {} tokens**", group_thousands(*tokens_before));
            labeled_message_lines("compaction", &header, summary, theme, width)
        }
        Entry::Status(text) => vec![Line::styled(format!("• {text}"), theme.dim_style())],
        Entry::Block { title, markdown } => {
            let rule = "─".repeat(width.max(1));
            let bold = theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD);
            let mut out: Vec<Line<'static>> = vec![
                Line::default(),
                Line::styled(rule.clone(), theme.border_style()),
                Line::styled(title.clone(), bold),
                Line::default(),
            ];
            out.extend(crate::markdown::render(markdown, width.max(1), theme));
            out.push(Line::styled(rule, theme.border_style()));
            out
        }
    }
}

/// Render a labeled extension/system message (`skill`/`custom`/`branch`/`compaction` variants,
/// `{skill-invocation,custom,branch-summary,compaction-summary}-message.ts`): a bold-accent
/// `[label]` line, then the optional bold `header` + the `body` rendered as markdown. The committed
/// scrollback form is the *expanded* render (the complete record), like committed tools.
fn labeled_message_lines(
    label: &str,
    header: &str,
    body: &str,
    theme: &UiTheme,
    width: usize,
) -> Vec<Line<'static>> {
    let bold = theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD);
    let mut out = vec![Line::styled(format!("[{label}]"), bold)];
    let md_src = if header.is_empty() {
        body.to_string()
    } else if body.is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n{body}")
    };
    if !md_src.is_empty() {
        out.extend(crate::markdown::render(&md_src, width.max(1), theme));
    }
    out
}

/// Group an integer with `,` thousands separators (Pi `Number.toLocaleString()` for the compaction
/// token count). Pure ASCII; never allocates beyond the result.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

impl Component for TranscriptView {
    /// Render the active turn **inline** (no box/title — `assistant-message.ts:84-93`, spec/tui/01 §3):
    /// the streaming partial is a wrapped `Paragraph` filling the region, tail-anchored so the newest
    /// text stays visible as it grows (spec/tui/01 §3 overflow).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let lines = self.lines(area.width as usize, theme);
        // Auto-scroll: keep the tail (newest text) visible when content exceeds the region height,
        // minus any user page-up offset (clamped so it can never scroll past the top).
        let inner_h = area.height as usize;
        let total = lines.len();
        let max_scroll = total.saturating_sub(inner_h);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
        let scroll = max_scroll.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16;
        let para = Paragraph::new(lines)
            .style(theme.base_style())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, area);
    }
}

/// Join the `Text` content blocks of a message body into a single string (drops thinking/tool/image
/// blocks). Operates on `cyrup_core::Content`, which is in the dependency set.
pub fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// A skill block parsed out of a submitted/replayed user message (Pi `ParsedSkillBlock`,
/// agent-session.ts:103).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSkillBlock {
    /// The skill name (`<skill name="…">`).
    pub name: String,
    /// The skill location (`location="…">`) — the on-disk path the skill expanded from.
    pub location: String,
    /// The skill block body (markdown between the open/close tags).
    pub content: String,
    /// The trailing user message after the block, if any (`\n\n{message}`).
    pub user_message: Option<String>,
}

/// Parse a `<skill name="…" location="…">\n…\n</skill>(\n\n{userMessage})?` block out of message text
/// (Pi `parseSkillBlock`, agent-session.ts:114, a hand-port of its anchored regex — no regex dep).
/// Returns `None` for any text that is not exactly such a block.
pub fn parse_skill_block(text: &str) -> Option<ParsedSkillBlock> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = rest.split_once('"')?;
    if name.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(" location=\"")?;
    let (location, rest) = rest.split_once('"')?;
    if location.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(">\n")?;
    // Non-greedy: the body runs to the FIRST `\n</skill>` (`[\s\S]*?`).
    let close = rest.find("\n</skill>")?;
    let content = rest[..close].to_string();
    let after = &rest[close + "\n</skill>".len()..];
    let user_message = if after.is_empty() {
        None
    } else {
        // Must be `\n\n{message}` to end (the regex's optional `(?:\n\n([\s\S]+))?$`).
        let um = after.strip_prefix("\n\n")?.trim();
        (!um.is_empty()).then(|| um.to_string())
    };
    Some(ParsedSkillBlock { name: name.to_string(), location: location.to_string(), content, user_message })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod skill_tests {
    use super::*;

    #[test]
    fn parses_a_skill_block_with_trailing_user_message() {
        let text = "<skill name=\"deploy\" location=\"/skills/deploy.md\">\nRun the deploy steps.\n</skill>\n\nplease deploy prod";
        let block = parse_skill_block(text).expect("should parse");
        assert_eq!(block.name, "deploy");
        assert_eq!(block.location, "/skills/deploy.md");
        assert_eq!(block.content, "Run the deploy steps.");
        assert_eq!(block.user_message.as_deref(), Some("please deploy prod"));
    }

    #[test]
    fn parses_a_skill_block_without_user_message() {
        let text = "<skill name=\"lint\" location=\"/s/lint.md\">\nlint body\nmore\n</skill>";
        let block = parse_skill_block(text).expect("should parse");
        assert_eq!(block.name, "lint");
        assert_eq!(block.content, "lint body\nmore");
        assert_eq!(block.user_message, None);
    }

    #[test]
    fn plain_text_is_not_a_skill_block() {
        assert_eq!(parse_skill_block("just a normal message"), None);
        // A single newline after `</skill>` (not `\n\n`) is not a valid trailer.
        assert_eq!(
            parse_skill_block("<skill name=\"x\" location=\"y\">\nz\n</skill>\noops"),
            None
        );
    }

    #[test]
    fn push_user_splits_a_skill_block_into_two_entries() {
        let mut view = TranscriptView::new();
        view.push_user(
            "<skill name=\"deploy\" location=\"/s/d.md\">\nbody\n</skill>\n\nrun it",
        );
        let entries = view.pending();
        assert!(matches!(entries.first(), Some(Entry::SkillInvocation { name, .. }) if name == "deploy"));
        assert!(matches!(entries.get(1), Some(Entry::User(t)) if t == "run it"));
    }

    #[test]
    fn push_user_keeps_plain_text_as_one_entry() {
        let mut view = TranscriptView::new();
        view.push_user("hello world");
        let entries = view.pending();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries.first(), Some(Entry::User(t)) if t == "hello world"));
    }
}
