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

use crate::component::Component;
use crate::theme::UiTheme;

/// A committed transcript entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A user submission.
    User(String),
    /// A finalized assistant message.
    Assistant(String),
    /// A tool began executing.
    ToolStart { name: String },
    /// A tool finished (`is_error` failed).
    ToolEnd { name: String, is_error: bool },
    /// A status / notification line (model change, compaction, queue, …).
    Status(String),
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

    /// True while an assistant turn is actively streaming (the viewport's only conversation content).
    pub fn has_active(&self) -> bool {
        self.streaming.is_some()
    }

    /// Take every committed entry, leaving the pending buffer empty. The shell renders the returned
    /// entries into native scrollback exactly once (R-ARCH-TUI-003), so they are not shown again in
    /// the inline viewport.
    pub fn drain_committed(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.pending)
    }

    /// Append a user message.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::User(text.into()));
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

    /// Record a tool starting.
    pub fn push_tool_start(&mut self, name: impl Into<String>) {
        self.pending.push(Entry::ToolStart { name: name.into() });
    }

    /// Record a tool finishing.
    pub fn push_tool_end(&mut self, name: impl Into<String>, is_error: bool) {
        self.pending.push(Entry::ToolEnd { name: name.into(), is_error });
    }

    /// Record a status / notification line.
    pub fn push_status(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Status(text.into()));
    }

    /// Build the styled lines the inline viewport renders: **only** the active streaming partial.
    /// Committed entries live in native scrollback (see [`drain_committed`](Self::drain_committed)).
    ///
    /// Pi renders the in-flight assistant message **inline** with no surrounding box/title
    /// (`assistant-message.ts:84-93`); a dim soft cursor `▌` trails the last grapheme while the turn
    /// streams (spec/tui/01 §3) — the hardware cursor stays on the editor.
    fn lines(&self, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if let Some(partial) = &self.streaming {
            lines.push(Line::from(vec![
                Span::styled("assistant: ", theme.accent_style()),
                Span::styled(partial.clone(), theme.assistant_style()),
                Span::styled("▌", theme.dim_style()),
            ]));
        }
        lines
    }
}

/// Render a single committed [`Entry`] into one styled scrollback line (R-ARCH-TUI-003). Used by the
/// shell to feed drained entries into `Terminal::insert_before`.
pub(crate) fn entry_line(entry: &Entry, theme: &UiTheme) -> Line<'static> {
    match entry {
        Entry::User(text) => label_line("you", text, theme.user_style(), theme.base_style()),
        Entry::Assistant(text) => {
            label_line("assistant", text, theme.accent_style(), theme.assistant_style())
        }
        Entry::ToolStart { name } => Line::styled(format!("  ⚙ {name}…"), theme.dim_style()),
        Entry::ToolEnd { name, is_error } => {
            let style = if *is_error { theme.error_style() } else { theme.dim_style() };
            let mark = if *is_error { "✗" } else { "✓" };
            Line::styled(format!("  {mark} {name}"), style)
        }
        Entry::Status(text) => Line::styled(format!("• {text}"), theme.dim_style()),
    }
}

/// Build a `label: text` line with a styled label and styled body.
fn label_line(
    label: &str,
    text: &str,
    label_style: ratatui::style::Style,
    body_style: ratatui::style::Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), label_style),
        Span::styled(text.to_string(), body_style),
    ])
}

impl Component for TranscriptView {
    /// Render the active turn **inline** (no box/title — `assistant-message.ts:84-93`, spec/tui/01 §3):
    /// the streaming partial is a wrapped `Paragraph` filling the region, tail-anchored so the newest
    /// text stays visible as it grows (spec/tui/01 §3 overflow).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let lines = self.lines(theme);
        // Auto-scroll: keep the tail (newest text) visible when content exceeds the region height.
        let inner_h = area.height as usize;
        let total = lines.len();
        let scroll = total.saturating_sub(inner_h).min(u16::MAX as usize) as u16;
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
