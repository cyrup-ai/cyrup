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
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;

use crate::bash::BashExecution;
use crate::component::Component;
use crate::theme::UiTheme;

/// A committed transcript entry.
///
/// `Eq` is intentionally omitted: [`ToolRun`] carries the raw `serde_json::Value` call args / result
/// (so each tool can render its Pi-specific `renderCall`/`renderResult`), and `Value` is `PartialEq`
/// but not `Eq` (floats). Nothing in the crate needs a total `Eq` on entries.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A user submission.
    User(String),
    /// A finalized assistant message.
    Assistant(String),
    /// A finished tool execution (`tool-execution.ts`): the tool name + the raw call args + the raw
    /// result value + error flag. Each built-in dispatches to its Pi-specific rich render
    /// (`core/tools/{read,write,edit,bash,grep,find,ls}.ts` `renderCall`/`renderResult`).
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
/// committed to scrollback when the turn ends. The block is tinted by execution state
/// (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`), and each built-in renders its own Pi-specific
/// `renderCall` header + `renderResult` body. `expanded` (`Ctrl+O`, `app.tools.expand`) shows the full
/// result; the collapsed form shows each tool's preview (bash tail-5, grep head-15, find/ls head-20, a
/// hidden read/write body, …).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRun {
    /// Tool name (`read`, `bash`, `edit`, …).
    pub name: String,
    /// The raw tool-call arguments (`renderCall(args)`) — the path/command/pattern/offset/limit/…
    /// each tool's header is built from. `Value::Null` when a start was missed.
    pub args: Value,
    /// The raw tool result (`{content, details, terminate}`; `renderResult(result)`) — carries the
    /// per-tool `details` (edit `diff`, bash/read/grep/find/ls `truncation`, …). `None` while running.
    pub result: Option<Value>,
    /// Whether the tool failed.
    pub is_error: bool,
    /// Whether the execution has finished (drives the pending→success/error background tint).
    pub done: bool,
    /// Wall-clock start of the run, set on [`TranscriptView::push_tool_start`] — the basis for the
    /// bash `Took …` duration line (`formatDuration`, bash.ts:197/284-288).
    started_at: Option<std::time::Instant>,
    /// Frozen run duration in milliseconds, set on [`TranscriptView::push_tool_end`]. Rendered as the
    /// bash `Took {d}s` footer once the command finishes.
    duration_ms: Option<u64>,
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
    /// Horizontal padding (columns) applied to user/assistant/thinking output (Pi `outputPad`, `0`|`1`,
    /// default `1` — settings-manager.ts:1186; `Markdown(content, outputPad, 0)` /
    /// `Box(outputPad, 1)`, assistant-message.ts:103, user-message.ts:31). Toggled live from
    /// `/settings` → "Output padding" (F12). Left-indents each message line; `0` renders flush-left.
    output_pad: usize,
}

impl TranscriptView {
    /// An empty transcript.
    pub fn new() -> Self {
        // Pi's `outputPad` defaults to 1 (settings-manager.ts:1186), unlike the `0` a derived
        // `Default` would give — seed it explicitly so fresh transcripts match Pi's default padding.
        TranscriptView { output_pad: 1, ..TranscriptView::default() }
    }

    /// The horizontal output padding (`outputPad`, columns) applied to user/assistant messages — read
    /// by the shell to pass into [`entry_lines`] when flushing committed entries to scrollback.
    pub fn output_pad(&self) -> usize {
        self.output_pad
    }

    /// Set the horizontal output padding live (Pi `onOutputPadChange` → `this.outputPad = padding`,
    /// interactive-mode.ts:4127-4136). The `/settings` "Output padding" row drives this; the live region
    /// re-renders with the new indent on the next draw, and subsequently-committed messages flush with
    /// it (already-scrolled-off native scrollback is immutable, an accepted consequence of the
    /// content-sized-viewport architecture).
    pub fn set_output_pad(&mut self, pad: usize) {
        self.output_pad = pad;
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

    /// Record a tool starting (live in the viewport): name + the raw call args (`ToolExecutionStart`).
    /// The args drive the per-tool `renderCall` header (path/command/pattern/range/…).
    pub fn push_tool_start(&mut self, name: impl Into<String>, args: Value) {
        self.active_tools.push(ToolRun {
            name: name.into(),
            args,
            result: None,
            is_error: false,
            done: false,
            started_at: Some(std::time::Instant::now()),
            duration_ms: None,
        });
    }

    /// Update the latest still-running tool's partial result (`ToolExecutionUpdate`): the raw partial
    /// result value, rendered by the tool's `renderResult` with `isPartial = true`.
    pub fn push_tool_update(&mut self, partial: Option<Value>) {
        if let Some(run) = self.active_tools.iter_mut().rev().find(|r| !r.done)
            && partial.is_some()
        {
            run.result = partial;
        }
    }

    /// Record a tool finishing: attach the raw result/error to the matching live run (the latest run
    /// with that name still running, else a fresh done entry so a missed start never drops the result).
    /// Freezes the run duration for the bash `Took …` footer.
    pub fn push_tool_end(&mut self, name: impl Into<String>, is_error: bool, result: Option<Value>) {
        let name = name.into();
        if let Some(run) = self.active_tools.iter_mut().rev().find(|r| !r.done && r.name == name) {
            run.done = true;
            run.is_error = is_error;
            run.result = result;
            run.duration_ms = run.started_at.map(|s| s.elapsed().as_millis() as u64);
        } else {
            self.active_tools.push(ToolRun {
                name,
                args: Value::Null,
                result,
                is_error,
                done: true,
                started_at: None,
                duration_ms: None,
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

    /// Progressively commit the LEADING run of already-finished tool executions to scrollback WHILE
    /// the turn is still live, so the inline viewport keeps only the actively-running tail (the
    /// currently-executing tool + any tools queued behind it) instead of stacking every completed
    /// tool of a long multi-tool turn until `AgentEnd`. This is the faithful port of Pi's behavior:
    /// each `ToolExecutionComponent` (packages/coding-agent/src/modes/interactive/components/
    /// tool-execution.ts:13) is a persistent child that, as later tool/text lines append below it,
    /// scrolls up past the diff's viewport top and lives in native scrollback thereafter
    /// (packages/tui/src/tui.ts:1455 `if (firstChanged < prevViewportTop) fullRender`). cyrup's
    /// idiomatic-ratatui equivalent (ADR-0001) is `Terminal::insert_before` for each finished entry —
    /// driven here by moving the finished leading tools into `pending` for the next
    /// [`drain_committed`](Self::drain_committed) → `insert_before` flush.
    ///
    /// Only the LEADING contiguous run of `done` tools is drained (stopping at the first still-running
    /// tool), so scrollback order always equals call order even under hypothetical parallel/interleaved
    /// tools — a still-running earlier tool blocks committing a finished later one ahead of it.
    ///
    /// Guarded on `streaming.is_none()`: a tool is never committed ahead of still-uncommitted assistant
    /// text of the same step. The confirmed event ordering (the assistant stream's terminal
    /// `StreamEvent::Done` → `commit_assistant` fires BEFORE any `ToolExecutionStart` of that step)
    /// keeps `streaming` clear whenever a tool finishes, so this guard is a safety net that also holds
    /// under interleaving.
    pub fn commit_finished_leading_tools(&mut self) {
        if self.streaming.is_some() {
            return;
        }
        let split = self
            .active_tools
            .iter()
            .position(|run| !run.done)
            .unwrap_or(self.active_tools.len());
        for run in self.active_tools.drain(..split) {
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
            let mut md =
                crate::markdown::render(&body, width.saturating_sub(11 + self.output_pad).max(1), theme);
            if md.is_empty() {
                md.push(Line::default());
            }
            if let Some(first) = md.first_mut() {
                first.spans.insert(0, Span::styled("assistant: ", theme.accent_style()));
            }
            if let Some(last) = md.last_mut() {
                last.spans.push(Span::styled("▌", theme.dim_style()));
            }
            pad_lines(&mut md, self.output_pad);
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

/// Pi's default read/write byte + line truncation limits (`truncate.ts:11-12`).
const DEFAULT_MAX_BYTES: u64 = 50 * 1024;
/// The `app.tools.expand` key label used in every `… to expand` hint. Pi renders the live keybinding
/// (`keyHint`); cyrup's transcript has no keymap handle, so it uses the immutable default (Ctrl+O), the
/// same literal the existing bash/tool hints use.
const EXPAND_KEY: &str = "ctrl+o";

/// Render one tool execution into styled lines by dispatching on the tool name to its Pi-specific
/// `renderCall`/`renderResult` (`tool-execution.ts` composes each built-in's renderers, not a generic
/// one-liner): edit → a self-diff (`edit.ts:390`), bash → an output tail + truncation + `Took …`
/// (`bash.ts:440`), read → a line-range header + a hidden-until-expanded body (`read.ts:329/339`),
/// write → a content preview (`write.ts:227`), grep/find/ls → a match/entry list with limit notices
/// (`grep.ts:370`, `find.ts:359`, `ls.ts:210`). The whole block is tinted by execution state
/// (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`, tool-execution.ts:253-258) — the bg is the state
/// affordance (Pi has no gear/check glyph), preceded by an untinted blank (the component's `Spacer(1)`,
/// tool-execution.ts:63).
pub(crate) fn tool_lines(
    run: &ToolRun,
    expanded: bool,
    width: usize,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let mut block: Vec<Line<'static>> = Vec::new();
    match run.name.as_str() {
        "read" => render_read(run, expanded, theme, &mut block),
        "write" => render_write(run, expanded, theme, &mut block),
        "edit" => render_edit(run, theme, &mut block),
        "bash" => render_bash(run, expanded, theme, &mut block),
        "grep" => render_grep(run, expanded, theme, &mut block),
        "find" => render_find(run, expanded, theme, &mut block),
        "ls" => render_ls(run, expanded, theme, &mut block),
        _ => render_generic(run, theme, &mut block),
    }
    // The block is state-tinted (bg-only); a leading untinted blank stands in for the component Spacer.
    let bg = theme.tool_bg_style(Style::default(), run.done, run.is_error);
    let mut out = vec![Line::default()];
    out.extend(finalize_block(block, width, bg));
    out
}

/// Apply the shared tool-block chrome to each already-fg-styled line: a 1-column left inset (Pi's
/// `Box(1, 1)` padding, tool-execution.ts:68) + a right-pad to `width` so the state tint fills the full
/// content width (`applyBackgroundToLine`, markdown.ts:216), then patch the state background on.
fn finalize_block(mut lines: Vec<Line<'static>>, width: usize, bg: Style) -> Vec<Line<'static>> {
    for line in &mut lines {
        line.spans.insert(0, Span::raw(" "));
        let len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        if len < width {
            line.spans.push(Span::raw(" ".repeat(width - len)));
        }
        line.style = line.style.patch(bg);
    }
    lines
}

// --- per-tool renderers ------------------------------------------------------------------------
//
// Each pushes fg-styled logical lines into `out`; `tool_lines` adds the leading spacer + state tint.

/// `read` — header `read <path>:<range>` + (only when expanded/error) the file body (`read.ts:74-201`).
fn render_read(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let mut spans = vec![Span::styled("read ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
    if let Some(range) = read_line_range(&run.args) {
        spans.push(Span::styled(range, theme.warning_style()));
    }
    out.push(Line::from(spans));
    // `formatReadResult`: nothing below the header when collapsed & not an error (read.ts:173-175).
    let Some(result) = &run.result else { return };
    if !expanded && !run.is_error {
        return;
    }
    let output = result_text(result);
    let all = trim_trailing_empty(output.split('\n').collect());
    let total = all.len();
    let shown = if expanded { total } else { total.min(10) };
    out.push(Line::default());
    for l in all.iter().take(shown) {
        out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, theme));
    }
    push_read_truncation(result, theme, out);
}

/// `write` — header `write <path>` + a content preview from the call args (`write.ts:131-179`).
fn render_write(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let mut spans = vec![Span::styled("write ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
    out.push(Line::from(spans));
    match str_arg(&run.args, &["content"]) {
        StrArg::Invalid => {
            out.push(Line::default());
            out.push(Line::styled(
                "[invalid content arg - expected string]".to_string(),
                theme.error_style(),
            ));
        }
        StrArg::Missing => {}
        StrArg::Value(content) => {
            let display = content.replace('\r', "");
            let all = trim_trailing_empty(display.split('\n').collect());
            let total = all.len();
            let shown = if expanded { total } else { total.min(10) };
            out.push(Line::default());
            for l in all.iter().take(shown) {
                out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
            }
            let remaining = total.saturating_sub(shown);
            if remaining > 0 {
                out.push(more_lines_hint(remaining, Some(total), theme));
            }
        }
    }
    // `formatWriteResult` shows output only on error (write.ts:164-179).
    if run.is_error && let Some(result) = &run.result {
        push_error_body(result, theme, out);
    }
}

/// `edit` — header `edit <path>` + the result self-diff (`edit.ts:200-227/363-431`, rendered via
/// [`crate::diff::render_diff`], the port of `diff.ts`).
fn render_edit(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let mut spans = vec![Span::styled("edit ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
    out.push(Line::from(spans));
    let Some(result) = &run.result else { return };
    if run.is_error {
        push_error_body(result, theme, out);
        return;
    }
    if let Some(diff) = result
        .get("details")
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        out.push(Line::default());
        out.extend(crate::diff::render_diff(diff, theme));
    }
}

/// `bash` — header `$ <command> (timeout Ns)` + the output tail (collapsed = last 5 visual lines) +
/// truncation notices + a `Took {d}s` footer (`bash.ts:201-289/430-464`).
fn render_bash(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    // Header: `$ command`, bold, + a muted ` (timeout Ns)` suffix (`formatBashCall`).
    let title = theme.tool_title_style();
    let mut spans = Vec::new();
    match str_arg(&run.args, &["command"]) {
        StrArg::Invalid => {
            spans.push(Span::styled("$ ".to_string(), title));
            spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style()));
        }
        StrArg::Missing => {
            spans.push(Span::styled("$ ".to_string(), title));
            spans.push(Span::styled("...".to_string(), theme.tool_output_style()));
        }
        StrArg::Value(cmd) => spans.push(Span::styled(format!("$ {cmd}"), title)),
    }
    if let Some(t) = run.args.get("timeout").and_then(Value::as_f64).filter(|t| *t != 0.0) {
        // `${timeout}s` (bash.ts:204): JS renders an integer number without a trailing `.0`.
        let disp = if t.fract() == 0.0 { format!("{}", t as i64) } else { format!("{t}") };
        spans.push(Span::styled(format!(" (timeout {disp}s)"), theme.muted_style()));
    }
    out.push(Line::from(spans));

    if let Some(result) = &run.result {
        let raw = result_text(result);
        let output = strip_bash_footer(raw.trim(), result, run.done);
        if !output.is_empty() {
            out.push(Line::default());
            let all: Vec<&str> = output.split('\n').collect();
            let total = all.len();
            if expanded {
                for l in &all {
                    out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
                }
            } else {
                let shown = total.min(5);
                let skipped = total - shown;
                if skipped > 0 {
                    out.push(Line::styled(
                        format!("... ({skipped} earlier lines, {EXPAND_KEY} to expand)"),
                        theme.muted_style(),
                    ));
                }
                for l in all.iter().skip(skipped) {
                    out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
                }
            }
        }
        push_bash_warnings(result, theme, out);
        if let Some(ms) = run.duration_ms {
            out.push(Line::styled(
                format!("Took {}", format_duration(ms)),
                theme.muted_style(),
            ));
        }
    }
}

/// `grep` — header `grep /<pattern>/ in <path> (glob) limit N` + matching lines (head-15) + a
/// `[Truncated: …]` notice (`grep.ts:68-121/370-379`).
fn render_grep(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let title = theme.tool_title_style();
    let outp = theme.tool_output_style();
    let mut spans = vec![Span::styled("grep ".to_string(), title)];
    match str_arg(&run.args, &["pattern"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => spans.push(Span::styled("//".to_string(), theme.accent_style())),
        StrArg::Value(p) => spans.push(Span::styled(format!("/{p}/"), theme.accent_style())),
    }
    spans.push(Span::styled(" in ".to_string(), outp));
    push_search_path(&run.args, theme, &mut spans);
    if let StrArg::Value(glob) = str_arg(&run.args, &["glob"]) {
        spans.push(Span::styled(format!(" ({glob})"), outp));
    }
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" limit {limit}"), outp));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 15, theme, out);
    push_grep_warnings(run.result.as_ref(), theme, out);
}

/// `find` — header `find <pattern> in <path> (limit N)` + matching paths (head-20) + a `[Truncated: …]`
/// notice (`find.ts:59-107/359-368`).
fn render_find(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let title = theme.tool_title_style();
    let outp = theme.tool_output_style();
    let mut spans = vec![Span::styled("find ".to_string(), title)];
    match str_arg(&run.args, &["pattern"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => {}
        StrArg::Value(p) => spans.push(Span::styled(p, theme.accent_style())),
    }
    spans.push(Span::styled(" in ".to_string(), outp));
    push_search_path(&run.args, theme, &mut spans);
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" (limit {limit})"), outp));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 20, theme, out);
    push_find_warnings(run.result.as_ref(), theme, out);
}

/// `ls` — header `ls <path> (limit N)` + entries (head-20) + a `[Truncated: …]` notice
/// (`ls.ts:52-93/210-219`).
fn render_ls(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let mut spans = vec![Span::styled("ls ".to_string(), theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["path"], Some("."), theme));
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" (limit {limit})"), theme.tool_output_style()));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 20, theme, out);
    push_ls_warnings(run.result.as_ref(), theme, out);
}

/// Non-built-in tools fall back to Pi's `formatToolExecution` (tool-execution.ts:365-376): the bold
/// tool name + pretty-printed args + any text output.
fn render_generic(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    out.push(Line::styled(run.name.clone(), theme.tool_title_style()));
    if !run.args.is_null()
        && let Ok(pretty) = serde_json::to_string_pretty(&run.args)
    {
        out.push(Line::default());
        for l in pretty.split('\n') {
            out.push(Line::styled(l.to_string(), theme.tool_output_style()));
        }
    }
    if let Some(result) = &run.result {
        let output = result_text(result);
        if !output.trim().is_empty() {
            for l in output.split('\n') {
                out.push(Line::styled(l.to_string(), theme.tool_output_style()));
            }
        }
    }
}

// --- per-tool render helpers -------------------------------------------------------------------

/// A coalesced string argument (`args.file_path ?? args.path`, then Pi's `str()`): a present non-string
/// → [`StrArg::Invalid`] (`[invalid arg]`), absent/null/`""` → [`StrArg::Missing`], else the string.
enum StrArg {
    Invalid,
    Missing,
    Value(String),
}

/// `args[key0] ?? args[key1] ?? …` then `str()` (render-utils.ts:25-29): skip absent/JSON-null keys, a
/// non-string value is `Invalid`, an empty string is `Missing`.
fn str_arg(args: &Value, keys: &[&str]) -> StrArg {
    for k in keys {
        match args.get(k) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) => {
                return if s.is_empty() { StrArg::Missing } else { StrArg::Value(s.clone()) };
            }
            Some(_) => return StrArg::Invalid,
        }
    }
    StrArg::Missing
}

/// `renderToolPath` (render-utils.ts:75-85): `[invalid arg]` for a non-string, the `emptyFallback`
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent. Hyperlinks are a
/// terminal escape the cell grid does not carry (tracked residual).
fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), theme.accent_style()),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), theme.accent_style()),
    }
}

/// The `" in <path>"` tail shared by grep/find (`path = shortenPath(rawPath || ".")` in `toolOutput`, a
/// non-string → `[invalid arg]`). The caller has already pushed the `" in "` label span.
fn push_search_path(args: &Value, theme: &UiTheme, spans: &mut Vec<Span<'static>>) {
    match str_arg(args, &["path"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => {
            spans.push(Span::styled(shorten_path("."), theme.tool_output_style()));
        }
        StrArg::Value(p) => spans.push(Span::styled(shorten_path(&p), theme.tool_output_style())),
    }
}

/// `formatReadLineRange` (read.ts:67-72): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_i64);
    let limit = args.get("limit").and_then(Value::as_i64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1);
    Some(match limit {
        Some(l) => format!(":{start}-{}", start + l - 1),
        None => format!(":{start}"),
    })
}

/// A `... (N more lines[, M total], ctrl+o to expand)` hint (read/write/grep/find/ls collapsed tail).
fn more_lines_hint(remaining: usize, total: Option<usize>, theme: &UiTheme) -> Line<'static> {
    let text = match total {
        Some(t) => format!("... ({remaining} more lines, {t} total, {EXPAND_KEY} to expand)"),
        None => format!("... ({remaining} more lines, {EXPAND_KEY} to expand)"),
    };
    Line::styled(text, theme.muted_style())
}

/// Shared head-N list body for grep/find/ls (`\n` + first N output lines + a `… more` hint).
fn push_list_output(
    run: &ToolRun,
    expanded: bool,
    head: usize,
    theme: &UiTheme,
    out: &mut Vec<Line<'static>>,
) {
    let Some(result) = &run.result else { return };
    let output = result_text(result);
    let output = output.trim();
    if output.is_empty() {
        return;
    }
    let all: Vec<&str> = output.split('\n').collect();
    let total = all.len();
    let shown = if expanded { total } else { total.min(head) };
    out.push(Line::default());
    for l in all.iter().take(shown) {
        out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, theme));
    }
}

/// Push an error body (`\n` + the result text in the error color): edit/write on failure.
fn push_error_body(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let text = result_text(result);
    if text.trim().is_empty() {
        return;
    }
    out.push(Line::default());
    for l in text.split('\n') {
        out.push(Line::styled(l.to_string(), theme.error_style()));
    }
}

/// Extract the tool result's display text (`getTextOutput`, render-utils.ts:39-64): join the `text`
/// blocks of `{content:[…]}` (an `image` block → `[image]`), else a `text`/`output`/`stdout`/`message`
/// string field, else a bare string/array. Carriage returns are stripped.
fn result_text(result: &Value) -> String {
    match result {
        Value::String(s) => s.replace('\r', ""),
        Value::Object(o) => {
            if let Some(content) = o.get("content") {
                return content_blocks_text(content);
            }
            for k in ["text", "output", "stdout", "message"] {
                if let Some(Value::String(s)) = o.get(k) {
                    return s.replace('\r', "");
                }
            }
            String::new()
        }
        Value::Array(_) => content_blocks_text(result),
        _ => String::new(),
    }
}

/// Join a `content` block array into text (`text` blocks concatenated with `\n`; `image` → `[image]`).
fn content_blocks_text(content: &Value) -> String {
    match content {
        Value::Array(items) => {
            let mut parts = Vec::new();
            for it in items {
                if let Some(obj) = it.as_object() {
                    let ty = obj.get("type").and_then(Value::as_str);
                    if matches!(ty, Some("text") | None)
                        && let Some(Value::String(t)) = obj.get("text")
                    {
                        parts.push(t.replace('\r', ""));
                        continue;
                    }
                    if ty == Some("image") {
                        parts.push("[image]".to_string());
                    }
                } else if let Some(s) = it.as_str() {
                    parts.push(s.replace('\r', ""));
                }
            }
            parts.join("\n")
        }
        Value::String(s) => s.replace('\r', ""),
        _ => String::new(),
    }
}

/// Drop trailing empty lines (`trimTrailingEmptyLines`, read.ts:79-85 / write.ts:123-129).
fn trim_trailing_empty(mut lines: Vec<&str>) -> Vec<&str> {
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// The truncation object from `result.details.truncation` when `truncated` is set.
fn truncation(result: &Value) -> Option<&Value> {
    let t = result.get("details")?.get("truncation")?;
    (t.get("truncated") == Some(&Value::Bool(true))).then_some(t)
}

fn tnum(t: &Value, key: &str) -> u64 {
    t.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// `formatSize` (truncate.ts:61-69): `B` / `KB` / `MB`.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `formatDuration` (bash.ts:197-199): `{s}.{tenths}s`.
fn format_duration(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// read `renderResult` truncation footer (read.ts:190-199).
fn push_read_truncation(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(t) = truncation(result) else { return };
    let max_bytes = t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES);
    let msg = if t.get("firstLineExceedsLimit") == Some(&Value::Bool(true)) {
        format!("[First line exceeds {} limit]", format_size(max_bytes))
    } else if t.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
        format!(
            "[Truncated: showing {} of {} lines ({} line limit)]",
            tnum(t, "outputLines"),
            tnum(t, "totalLines"),
            tnum(t, "maxLines"),
        )
    } else {
        format!("[Truncated: {} lines shown ({} limit)]", tnum(t, "outputLines"), format_size(max_bytes))
    };
    out.push(Line::styled(msg, theme.warning_style()));
}

/// Strip the `\n\n[Showing lines … Full output: <path>]` footer bash bakes into the text but re-renders
/// as a warning (bash.ts:226-231): only when finished + truncated + a `fullOutputPath` is present.
fn strip_bash_footer(output: &str, result: &Value, done: bool) -> String {
    let full = result.get("details").and_then(|d| d.get("fullOutputPath")).and_then(Value::as_str);
    if done
        && truncation(result).is_some()
        && let Some(path) = full
        && output.ends_with(']')
        && let Some(idx) = output.rfind("\n\n[")
        && output[idx..].contains(path)
    {
        return output[..idx].trim_end().to_string();
    }
    output.to_string()
}

/// bash `renderResult` truncation + full-output warnings (bash.ts:267-282).
fn push_bash_warnings(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let full = result.get("details").and_then(|d| d.get("fullOutputPath")).and_then(Value::as_str);
    let trunc = truncation(result);
    if trunc.is_none() && full.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(p) = full {
        warns.push(format!("Full output: {p}"));
    }
    if let Some(t) = trunc {
        if t.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
            warns.push(format!("Truncated: showing {} of {} lines", tnum(t, "outputLines"), tnum(t, "totalLines")));
        } else {
            let max_bytes = t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES);
            warns.push(format!("Truncated: {} lines shown ({} limit)", tnum(t, "outputLines"), format_size(max_bytes)));
        }
    }
    out.push(Line::styled(format!("[{}]", warns.join(". ")), theme.warning_style()));
}

/// grep `renderResult` warnings (grep.ts:110-119).
fn push_grep_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let details = result.get("details");
    let match_limit = details.and_then(|d| d.get("matchLimitReached")).and_then(Value::as_u64);
    let lines_trunc = details.and_then(|d| d.get("linesTruncated")) == Some(&Value::Bool(true));
    let trunc = truncation(result);
    if match_limit.is_none() && trunc.is_none() && !lines_trunc {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = match_limit {
        warns.push(format!("{n} matches limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    if lines_trunc {
        warns.push("some lines truncated".to_string());
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// find `renderResult` warnings (find.ts:98-105).
fn push_find_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let result_limit =
        result.get("details").and_then(|d| d.get("resultLimitReached")).and_then(Value::as_u64);
    let trunc = truncation(result);
    if result_limit.is_none() && trunc.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = result_limit {
        warns.push(format!("{n} results limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// ls `renderResult` warnings (ls.ts:84-91).
fn push_ls_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let entry_limit =
        result.get("details").and_then(|d| d.get("entryLimitReached")).and_then(Value::as_u64);
    let trunc = truncation(result);
    if entry_limit.is_none() && trunc.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = entry_limit {
        warns.push(format!("{n} entries limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// `shortenPath` (render-utils.ts:10-17): replace a leading `$HOME` with `~`.
fn shorten_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
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
/// Left-indent every line by `pad` columns — the horizontal half of Pi's `outputPad` message padding
/// (`Markdown(content, outputPad, 0)` / `Box(outputPad, 1)`). A no-op at `pad == 0` (flush-left).
fn pad_lines(lines: &mut [Line<'static>], pad: usize) {
    if pad == 0 {
        return;
    }
    let indent = " ".repeat(pad);
    for line in lines.iter_mut() {
        line.spans.insert(0, Span::raw(indent.clone()));
    }
}

pub(crate) fn entry_lines(
    entry: &Entry,
    theme: &UiTheme,
    width: usize,
    output_pad: usize,
) -> Vec<Line<'static>> {
    match entry {
        Entry::User(text) => {
            // The rich user-message render (`user-message.ts`): the submitted text is rendered as
            // multi-line **markdown** (Pi `UserMessageComponent` wraps a `Markdown` in a
            // `userMessageBg` box), with a `you:` accent label prefixed onto the first line so the
            // conversation stays grep-legible — mirroring the assistant arm. (Pi's `userMessageBg`
            // box background and the OSC-133 shell-zone markers wrapping the block are terminal
            // shell-integration escapes that the ratatui cell grid / `insert_before` scrollback model
            // cannot carry; the markdown body + user accent is the in-crate fidelity.)
            let mut md =
                crate::markdown::render(text, width.saturating_sub(5 + output_pad).max(1), theme);
            if md.is_empty() {
                md.push(Line::default());
            }
            if let Some(first) = md.first_mut() {
                first.spans.insert(0, Span::styled("you: ", theme.user_style()));
            }
            // The `outputPad` horizontal padding (Pi `Box(outputPad, 1)`, user-message.ts:31).
            pad_lines(&mut md, output_pad);
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
            let mut md =
                crate::markdown::render(text, width.saturating_sub(11 + output_pad).max(1), theme);
            if md.is_empty() {
                md.push(Line::default());
            }
            if let Some(first) = md.first_mut() {
                first.spans.insert(0, Span::styled("assistant: ", theme.accent_style()));
            }
            // The `outputPad` horizontal padding (Pi `Markdown(content, outputPad, 0)`,
            // assistant-message.ts:103).
            pad_lines(&mut md, output_pad);
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
mod output_pad_tests {
    use super::*;

    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// F12: a fresh transcript defaults to Pi's `outputPad = 1` and `set_output_pad` drives it.
    #[test]
    fn output_pad_defaults_to_one_and_is_settable() {
        let mut view = TranscriptView::new();
        assert_eq!(view.output_pad(), 1, "Pi's default outputPad is 1");
        view.set_output_pad(0);
        assert_eq!(view.output_pad(), 0);
    }

    /// `outputPad` left-indents user/assistant committed lines; `0` renders flush-left, `1` prepends a
    /// single leading column ahead of the `you:`/`assistant:` accent label.
    #[test]
    fn output_pad_left_indents_committed_messages() {
        let theme = UiTheme::dark();
        // pad = 1 → a leading space before the label.
        let u1 = entry_lines(&Entry::User("hello".into()), &theme, 80, 1);
        assert!(line_text(&u1[0]).starts_with(" you: "), "pad=1 user: {:?}", line_text(&u1[0]));
        let a1 = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 1);
        assert!(
            line_text(&a1[0]).starts_with(" assistant: "),
            "pad=1 assistant: {:?}",
            line_text(&a1[0])
        );
        // pad = 0 → flush-left (no leading space).
        let u0 = entry_lines(&Entry::User("hello".into()), &theme, 80, 0);
        assert!(line_text(&u0[0]).starts_with("you: "), "pad=0 user: {:?}", line_text(&u0[0]));
        let a0 = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 0);
        assert!(
            line_text(&a0[0]).starts_with("assistant: "),
            "pad=0 assistant: {:?}",
            line_text(&a0[0])
        );
    }

    /// The live streaming partial honors the pad too (Pi keeps the outputPad on the in-flight
    /// `AssistantMessageComponent`). Rendering the active region with pad=1 vs pad=0 shifts the line.
    #[test]
    fn output_pad_indents_the_live_streaming_partial() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_assistant_delta("streaming answer");
        let padded = view.lines(80, &theme);
        assert!(
            line_text(&padded[0]).starts_with(" assistant: "),
            "pad=1 live: {:?}",
            line_text(&padded[0])
        );
        view.set_output_pad(0);
        let flush = view.lines(80, &theme);
        assert!(
            line_text(&flush[0]).starts_with("assistant: "),
            "pad=0 live: {:?}",
            line_text(&flush[0])
        );
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod progressive_commit_tests {
    use super::*;

    fn tool_names(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool(run) => Some(run.name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The primary SCREEN-FILL fix: finished tools commit progressively (leaving `active_tools`), so a
    /// long multi-tool turn never stacks the whole turn in the live viewport. `content_height` (which
    /// sizes the viewport) stays bounded to the running tail.
    #[test]
    fn finished_tools_commit_progressively_and_content_height_stays_bounded() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        let tall_before = {
            // Simulate 20 finished tool calls arriving one at a time (the reported storm).
            for i in 0..20u32 {
                let name = format!("read_{i}");
                view.push_tool_start(name.clone(), serde_json::json!({ "path": format!("file_{i}.md") }));
                view.push_tool_end(
                    name,
                    false,
                    Some(format!("body of file {i}\nsecond line\nthird").into()),
                );
                // The app drains finished-leading tools after every ToolExecutionEnd.
                view.commit_finished_leading_tools();
            }
            view.content_height(80, &theme)
        };
        // All 20 committed to `pending` (headed for native scrollback), none left live.
        assert_eq!(view.active_tools().len(), 0, "finished tools must not accumulate live");
        assert_eq!(tool_names(view.pending()).len(), 20, "all finished tools should be committed");
        // The live region measured near-empty (no tail): bounded, not full-screen.
        assert!(tall_before <= 1, "content_height ballooned to {tall_before}; must stay bounded");
        // Commit order equals call order in scrollback.
        assert_eq!(tool_names(view.pending()).first().copied(), Some("read_0"));
        assert_eq!(tool_names(view.pending()).last().copied(), Some("read_19"));
    }

    /// Only the LEADING run of finished tools commits: a still-running earlier tool blocks committing a
    /// finished later one ahead of it, so scrollback order = call order even when tools interleave.
    #[test]
    fn only_leading_finished_run_commits_running_tool_blocks() {
        let mut view = TranscriptView::new();
        view.push_tool_start("a", Value::Null); // will stay running
        view.push_tool_start("b", Value::Null);
        view.push_tool_start("c", Value::Null);
        // `b` finishes first, but `a` is still running ahead of it.
        view.push_tool_end("b", false, Some("b-result".into()));
        view.commit_finished_leading_tools();
        assert!(view.pending().is_empty(), "nothing commits while the leading tool `a` runs");
        assert_eq!(view.active_tools().len(), 3, "all three stay live until `a` finishes");

        // `a` finishes → the leading run `a`, `b` commits (in order), `c` stays live.
        view.push_tool_end("a", false, Some("a-result".into()));
        view.commit_finished_leading_tools();
        assert_eq!(tool_names(view.pending()), vec!["a", "b"], "leading finished run commits in order");
        assert_eq!(view.active_tools().len(), 1, "still-running `c` stays live");
        assert_eq!(view.active_tools()[0].name, "c");
    }

    /// The `streaming.is_none()` guard: a finished tool never commits ahead of uncommitted assistant
    /// text of the same step (SCROLLBACK-ORDER safety).
    #[test]
    fn streaming_partial_blocks_tool_commit() {
        let mut view = TranscriptView::new();
        view.push_assistant_delta("thinking about the next step");
        view.push_tool_start("read", Value::Null);
        view.push_tool_end("read", false, Some("result".into()));
        view.commit_finished_leading_tools();
        assert!(view.pending().is_empty(), "tool must not commit while assistant text is streaming");
        assert_eq!(view.active_tools().len(), 1, "the finished tool stays live behind the stream");

        // Once the assistant text commits (streaming cleared), the tool is free to commit after it.
        view.commit_assistant(None);
        view.commit_finished_leading_tools();
        assert!(
            matches!(view.pending().first(), Some(Entry::Assistant(_))),
            "assistant text commits before the tool row"
        );
        assert_eq!(tool_names(view.pending()), vec!["read"], "the tool commits after the stream");
    }
}
