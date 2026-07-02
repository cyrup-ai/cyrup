//! A reusable single-field text-input [`Selector`] (spec/tui/05 §3.1 extension; L4 review §2.1). The
//! input-slot occupant for a plain "type text, `Enter` confirms" flow — used by the `ui.input`
//! extension dialog (`SelectorKind::ExtensionInput`). Generalizes the ad hoc rename buffer
//! [`crate::session_selector::SessionSelector`] already carries inline (`renaming:
//! Option<(String, String)>`, `session_selector.rs:108-109,371-396`), which is otherwise the only
//! free-text input component in the crate.
//!
//! Unlike [`crate::selector::ListSelector`] there is no list to navigate: every printable key inserts
//! at the cursor, `Backspace`/`Delete`/arrows edit the buffer, `Enter` confirms with the buffer's
//! current text (even empty — Pi's `input` allows an empty submit), `Esc` cancels (Pi `undefined`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::SelectKeymap;
use crate::selector::{search_input_spans, Selector, SelectorOutcome};
use crate::theme::UiTheme;

/// A single-line text-input selector: `title` is the dialog prompt shown above the field; `placeholder`
/// (dim, shown only while the buffer is empty) mirrors Pi's `input(title, placeholder, opts)`
/// (rpc-types.ts:233-240).
pub struct TextInputSelector {
    title: String,
    placeholder: Option<String>,
    buffer: String,
    /// Byte offset into `buffer` (always a char boundary).
    cursor: usize,
}

impl TextInputSelector {
    /// Build with an empty buffer, the given `title` prompt and optional `placeholder` hint.
    pub fn new(title: String, placeholder: Option<String>) -> Self {
        TextInputSelector { title, placeholder, buffer: String::new(), cursor: 0 }
    }

    /// The current buffer text (test/inspection).
    pub fn text(&self) -> &str {
        &self.buffer
    }

    fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        let Some(ch) = self.buffer[..self.cursor].chars().next_back() else { return };
        let start = self.cursor - ch.len_utf8();
        self.buffer.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn delete_forward(&mut self) {
        let Some(ch) = self.buffer[self.cursor..].chars().next() else { return };
        let end = self.cursor + ch.len_utf8();
        self.buffer.replace_range(self.cursor..end, "");
    }

    fn cursor_left(&mut self) {
        if let Some(ch) = self.buffer[..self.cursor].chars().next_back() {
            self.cursor -= ch.len_utf8();
        }
    }

    fn cursor_right(&mut self) {
        if let Some(ch) = self.buffer[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }
}

impl Selector for TextInputSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // Top rule + title + input line + bottom rule.
        4
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let [top, title_area, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let rule = |w: u16| "─".repeat(w.max(1) as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(rule(top.width), theme.border_style()))),
            top,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", self.title),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ))),
            title_area,
        );
        let mut spans = vec![Span::styled(" > ", theme.accent_style())];
        if self.buffer.is_empty() && let Some(hint) = &self.placeholder {
            spans.push(Span::styled(hint.clone(), theme.muted_style()));
        } else {
            spans.extend(search_input_spans(&self.buffer, self.cursor, theme));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), body);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(rule(bottom.width), theme.border_style()))),
            bottom,
        );
    }

    fn handle(&mut self, key: &KeyEvent, _keymap: &SelectKeymap) -> SelectorOutcome {
        match key.code {
            KeyCode::Enter => SelectorOutcome::Confirm(self.buffer.clone()),
            KeyCode::Esc => SelectorOutcome::Cancel,
            KeyCode::Backspace => {
                self.backspace();
                SelectorOutcome::Redraw
            }
            KeyCode::Delete => {
                self.delete_forward();
                SelectorOutcome::Redraw
            }
            KeyCode::Left => {
                self.cursor_left();
                SelectorOutcome::Redraw
            }
            KeyCode::Right => {
                self.cursor_right();
                SelectorOutcome::Redraw
            }
            KeyCode::Home => {
                self.cursor = 0;
                SelectorOutcome::Redraw
            }
            KeyCode::End => {
                self.cursor = self.buffer.len();
                SelectorOutcome::Redraw
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                SelectorOutcome::Redraw
            }
            _ => SelectorOutcome::Ignored,
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn types_and_confirms() {
        let mut sel = TextInputSelector::new("Name?".to_string(), None);
        let km = SelectKeymap::default();
        for c in "hi".chars() {
            assert_eq!(sel.handle(&key(KeyCode::Char(c)), &km), SelectorOutcome::Redraw);
        }
        assert_eq!(sel.text(), "hi");
        assert_eq!(sel.handle(&key(KeyCode::Enter), &km), SelectorOutcome::Confirm("hi".to_string()));
    }

    #[test]
    fn empty_submit_confirms_empty_string() {
        let mut sel = TextInputSelector::new("Name?".to_string(), Some("placeholder".to_string()));
        let km = SelectKeymap::default();
        assert_eq!(sel.handle(&key(KeyCode::Enter), &km), SelectorOutcome::Confirm(String::new()));
    }

    #[test]
    fn escape_cancels() {
        let mut sel = TextInputSelector::new("Name?".to_string(), None);
        let km = SelectKeymap::default();
        sel.handle(&key(KeyCode::Char('x')), &km);
        assert_eq!(sel.handle(&key(KeyCode::Esc), &km), SelectorOutcome::Cancel);
    }

    #[test]
    fn backspace_and_cursor_motion() {
        let mut sel = TextInputSelector::new("Name?".to_string(), None);
        let km = SelectKeymap::default();
        for c in "abc".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        sel.handle(&key(KeyCode::Left), &km);
        sel.handle(&key(KeyCode::Backspace), &km);
        assert_eq!(sel.text(), "ac");
        sel.handle(&key(KeyCode::Home), &km);
        sel.handle(&key(KeyCode::Delete), &km);
        assert_eq!(sel.text(), "c");
    }
}
