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
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{
    search_input_spans, stack_rows, title_lines, title_wrapped_height, Selector, SelectorOutcome,
};
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
    /// The live selector bindings, so the hint row names the keys the user actually has bound
    /// (`keyHint` → `keyText` → `getKeybindings().getKeys(...)`, `keybinding-hints.ts:34-44`).
    /// Defaults to the stock table and is refreshed from whatever keymap routed the last key,
    /// exactly as [`crate::selector::ListSelector`] does.
    keymap: SelectKeymap,
}

impl TextInputSelector {
    /// Build with an empty buffer, the given `title` prompt and optional `placeholder` hint.
    pub fn new(title: String, placeholder: Option<String>) -> Self {
        TextInputSelector {
            title,
            placeholder,
            buffer: String::new(),
            cursor: 0,
            keymap: SelectKeymap::default(),
        }
    }

    /// Bind the hint row to the app's live `tui.select.*` table, so it names the keys the user has
    /// actually bound rather than the stock defaults.
    ///
    /// Mirrors [`crate::selector::ListSelector::with_hints`], which is `ListSelector`'s equivalent
    /// keymap-taking builder (there the keymap and the opt-in to the hint row arrive together,
    /// because only some kinds draw one; here the row is unconditional — `ExtensionInputComponent`
    /// always builds it, `extension-input.ts:66-68` — so only the keymap is a parameter).
    ///
    /// Without this the row was built from `SelectKeymap::default()` and only corrected inside
    /// [`Selector::handle`], i.e. after the first keystroke. The FIRST paint of a `ui.input` dialog
    /// is precisely the moment its "how do I submit this?" row matters, and upstream has no such
    /// window: `keyHint` resolves through `keyText` → `getKeybindings().getKeys(...)` on every
    /// render (`keybinding-hints.ts:34-44`), against the one live table.
    pub fn with_keymap(mut self, keymap: &SelectKeymap) -> Self {
        self.keymap = keymap.clone();
        self
    }

    /// The keyboard-hint row `ExtensionInputComponent` puts above the bottom border (E6 —
    /// `extension-input.ts:66-68`):
    /// `` new Text(`${keyHint("tui.select.confirm","submit")}  ${keyHint("tui.select.cancel","cancel")}`, 1, 0) ``.
    ///
    /// Two pairs only — this component has no `↑↓ navigate` (there is nothing to navigate), which
    /// is why it is NOT covered by the [`crate::selector::SelectorKind::draws_hint_row`] gating
    /// batch 3 added: that flag selects which kinds get `ListSelector`'s three-pair
    /// navigate/select/cancel row, and `SelectorKind::ExtensionInput` never constructs a
    /// `ListSelector` at all (`app.rs` opens it as this component). The row is this component's
    /// own, exactly as it is upstream.
    ///
    /// Each pair is two-tone — `dim` key, `muted` description (`keybinding-hints.ts:42-44`) — via
    /// [`crate::chrome::key_hint_spans`], and the leading space is the `paddingX = 1` of the
    /// wrapping `Text`.
    fn hint_line(&self, theme: &UiTheme) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        if let Some(keys) = self.keymap.keys_label(SelectAction::Confirm) {
            spans.extend(crate::chrome::key_hint_spans(&keys, "submit", theme));
        }
        if let Some(keys) = self.keymap.keys_label(SelectAction::Cancel) {
            if spans.len() > 1 {
                spans.push(Span::raw("  "));
            }
            spans.extend(crate::chrome::key_hint_spans(&keys, "cancel", theme));
        }
        Line::from(spans)
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
    fn desired_height(&self, width: u16) -> u16 {
        // Top rule + blank + (auto-sizing, wrapped) title + blank + input line + blank + hint row +
        // blank + bottom rule (E5/E6/E7 — see `render`).
        title_wrapped_height(&self.title, width).saturating_add(8)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = title_wrapped_height(&self.title, area.width);
        // E6 + E7. `ExtensionInputComponent`'s full child list (`extension-input.ts:47-70`):
        //   `DynamicBorder`(:47) · `Spacer`(:48) · titleText(:50-51) · `Spacer`(:52) ·
        //   `Input`(:63-64) · `Spacer`(:65) · hint(:66-68) · `Spacer`(:69) · `DynamicBorder`(:70).
        // Nine rows. cyrup drew four of them (rule/title/input/rule): the four `Spacer(1)`s are E7
        // and the hint row is E6. All heights are natural and the blanks unconditional;
        // `stack_rows` clips top-first exactly as pi's layout engine does (see its doc).
        let [top, _, title_area, _, body, _, hint, _, bottom] =
            stack_rows(area, [1, 1, title_h, 1, 1, 1, 1, 1, 1]);
        let rule = |w: u16| "─".repeat(w.max(1) as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(rule(top.width), theme.border_style()))),
            top,
        );
        frame.render_widget(
            Paragraph::new(title_lines(&self.title))
                .style(theme.accent_style().add_modifier(Modifier::BOLD))
                .wrap(ratatui::widgets::Wrap { trim: false }),
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
            Paragraph::new(vec![self.hint_line(theme)]).style(theme.base_style()),
            hint,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(rule(bottom.width), theme.border_style()))),
            bottom,
        );
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Keep the hint row honest: adopt whatever table actually routed this key (the same
        // refresh `ListSelector::handle` does).
        self.keymap = keymap.clone();
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
