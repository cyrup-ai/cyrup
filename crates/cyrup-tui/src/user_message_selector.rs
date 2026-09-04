//! The `/fork` "Fork from Message" picker (spec/tui/05 §6; a 1:1 port of
//! `components/user-message-selector.ts`, 155 lines).
//!
//! **S22.** This component is nothing like the shared list engine, and cyrup routed `/fork` through
//! [`ListSelector::data`](crate::selector::ListSelector) anyway. Two structural differences:
//!
//! 1. **Three lines per entry, not one.** `UserMessageList.render` (`:49-70`) pushes the message
//!    line, then a `muted` `  Message {i} of {N}` metadata line, then a blank — for every visible
//!    entry. cyrup drew a single two-column row whose right column read `message 3` (lowercase, no
//!    ` of N`), sourced from `app.rs`.
//! 2. **The header sits ABOVE the top rule.** `UserMessageSelectorComponent`'s constructor
//!    (`:122-144`) is `Spacer` · `bold("Fork from Message")` · muted subtitle · `Spacer` ·
//!    `DynamicBorder` · `Spacer` · list · `Spacer` · `DynamicBorder`. The title is **bold only** —
//!    no accent — and there is a subtitle line cyrup had no counterpart for at all. This is why
//!    `SelectorKind::UserMessage` is excluded from
//!    [`SelectorKind::envelope_spacers`](crate::selector::SelectorKind::envelope_spacers): the shape
//!    is different, not just the spacer count.
//!
//! The cursor is `theme.fg("accent", "› ")` (U+203A, `:57`) — not the `"→ "` every other picker
//! uses — and the selected message is `theme.bold(...)` on top of it.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{Selector, SelectorOutcome, border_rule_line, centered_window};
use crate::text_width::truncate_line_to_width;
use crate::theme::UiTheme;

/// `maxVisible` (`user-message-selector.ts:19`) — 10 messages.
const MAX_VISIBLE: usize = 10;

/// The muted subtitle under the title (`:126`), verbatim.
const SUBTITLE: &str =
    "Select a user message to copy the active path up to that point into a new session";

/// One forkable user message (`UserMessageItem`, `:5-9`).
#[derive(Clone, Debug)]
pub struct UserMessageRow {
    /// The session entry id — the confirm value (`message.id`).
    pub id: String,
    /// The message text (normalized to one line at render, `:54`).
    pub text: String,
}

/// The `/fork` picker.
pub struct UserMessageSelector {
    messages: Vec<UserMessageRow>,
    selected: usize,
}

impl UserMessageSelector {
    /// Build from the forkable messages in **chronological order** (oldest first, `:22-23`),
    /// preselecting `initial_id` if it is present, else the most recent (`:24-26`).
    pub fn new(messages: Vec<UserMessageRow>, initial_id: Option<&str>) -> Self {
        let initial = initial_id.and_then(|id| messages.iter().position(|m| m.id == id));
        let selected = initial.unwrap_or_else(|| messages.len().saturating_sub(1));
        UserMessageSelector { messages, selected }
    }

    /// The highlighted entry id, if any (test/inspection).
    pub fn current_id(&self) -> Option<&str> {
        self.messages.get(self.selected).map(|m| m.id.as_str())
    }

    /// The highlighted index (test/inspection).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// `message.text.replace(/\n/g, " ").trim()` (`:54`) — **bare `\n` only**, one space each, then
    /// trimmed. Deliberately NOT `select-list.ts`'s `normalizeToSingleLine`, which collapses a *run*
    /// of `[\r\n]+` to a single space: this one leaves `\r` alone and emits one space per newline.
    fn normalize(text: &str) -> String {
        text.replace('\n', " ").trim().to_string()
    }

    /// `UserMessageList.render` (`:33-78`): three lines per visible entry, plus the scroll row.
    fn body_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        if self.messages.is_empty() {
            // `:36-39` — this list is added to the container unwrapped, so no inset.
            return vec![Line::from(Span::styled(
                "  No user messages found",
                theme.muted_style(),
            ))];
        }
        let len = self.messages.len();
        let (start, end) = centered_window(self.selected, len, MAX_VISIBLE);
        let mut lines = Vec::with_capacity((end - start) * 3 + 1);
        for i in start..end {
            let Some(message) = self.messages.get(i) else {
                continue;
            };
            let is_selected = i == self.selected;
            // First line: `cursor + (isSelected ? bold(msg) : msg)`, the message hard-truncated to
            // `width - 2` to leave room for the two-column cursor (`:58-60`).
            let cursor = if is_selected {
                Span::styled("› ", theme.accent_style())
            } else {
                Span::raw("  ")
            };
            let style = if is_selected {
                theme.base_style().add_modifier(Modifier::BOLD)
            } else {
                theme.base_style()
            };
            let text = Line::from(Span::styled(Self::normalize(&message.text), style));
            let text = truncate_line_to_width(text, usize::from(width).saturating_sub(2), "...");
            let mut spans = vec![cursor];
            spans.extend(text.spans);
            lines.push(Line::from(spans));
            // Second line: the position metadata, muted, at two spaces (`:64-68`). Note the
            // capital `M` and the ` of {N}` tail — cyrup's `app.rs` produced `message 3`.
            lines.push(Line::from(Span::styled(
                format!("  Message {} of {len}", i + 1),
                theme.muted_style(),
            )));
            // Third line: blank between messages (`:69`) — unconditional, including after the last.
            lines.push(Line::from(""));
        }
        // `  (i/N)` scroll indicator, muted (`:73-76`).
        if start > 0 || end < len {
            lines.push(Line::from(Span::styled(
                format!("  ({}/{len})", self.selected + 1),
                theme.muted_style(),
            )));
        }
        lines
    }

    /// The header block ABOVE the top rule (`:122-131`): `Spacer` · bold title · muted subtitle ·
    /// `Spacer`. The title is `theme.bold("Fork from Message")` with **no** `theme.fg` — every other
    /// picker's title is accent+bold, this one is plain bold — and both rows are `new Text(…, 1, 0)`,
    /// i.e. inset one column (`text.ts:70-76`).
    fn header_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut out = vec![Line::from("")];
        out.push(Line::from(Span::styled(
            " Fork from Message",
            theme.base_style().add_modifier(Modifier::BOLD),
        )));
        // The subtitle is a `Text`, which WRAPS at `width - paddingX * 2` and prefixes the margin to
        // every row (`text.ts:64,70-76`) — `text_lines_of` is that behaviour.
        out.extend(crate::transcript::text_lines_of(
            &Line::from(Span::styled(SUBTITLE, theme.muted_style())),
            usize::from(width),
            1,
        ));
        out.push(Line::from(""));
        out
    }
}

impl Selector for UserMessageSelector {
    fn desired_height(&self, width: u16) -> u16 {
        let theme = UiTheme::default_ref();
        let header = self.header_lines(width, theme).len();
        let body = self.body_lines(width, theme).len();
        // header · `DynamicBorder`(:132) · `Spacer`(:133) · list · `Spacer`(:143) ·
        // `DynamicBorder`(:144).
        header
            .saturating_add(body)
            .saturating_add(4)
            .min(usize::from(u16::MAX)) as u16
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let mut lines = self.header_lines(area.width, theme);
        lines.push(border_rule_line(area.width, theme));
        lines.push(Line::from(""));
        lines.extend(self.body_lines(area.width, theme));
        lines.push(Line::from(""));
        lines.push(border_rule_line(area.width, theme));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        let len = self.messages.len();
        match keymap.action_for(key) {
            // Up/Down WRAP (`:84-90`).
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                if len > 0 {
                    self.selected = if self.selected == 0 {
                        len - 1
                    } else {
                        self.selected.saturating_sub(1)
                    };
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                if len > 0 {
                    self.selected = if self.selected.saturating_add(1) >= len {
                        0
                    } else {
                        self.selected + 1
                    };
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Confirm) => match self.current_id() {
                Some(id) => SelectorOutcome::Confirm(id.to_string()),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }
}
