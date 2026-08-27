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
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{
    border_rule, search_input_spans, stack_rows, title_lines, title_wrapped_height, Selector,
    SelectorOutcome,
};
use crate::theme::UiTheme;

/// A single-line text-input selector: `title` is the dialog prompt shown above the field.
///
/// A `placeholder` still travels the `ui.input(title, placeholder, opts)` wire (rpc-types.ts:
/// 233-240) and is still accepted by [`Self::new`], but — exactly as upstream — it is never
/// rendered: `ExtensionInputComponent` binds it as `_placeholder` and never references it again
/// (`extension-input.ts:36`), and the `Input` it builds has no placeholder concept at all
/// (`input.ts:378-446`). See E8 in [`Selector::render`].
pub struct TextInputSelector {
    title: String,
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
    /// Build with an empty buffer and the given `title` prompt. `_placeholder` is accepted so the
    /// `ui.input` wire field has somewhere to land and then discarded, which is what upstream does
    /// with it (`extension-input.ts:36` binds it as `_placeholder`).
    pub fn new(title: String, _placeholder: Option<String>) -> Self {
        TextInputSelector {
            title,
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
    #[must_use]
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
        // `stack_rows` fills the regions from the TOP and starves the trailing ones, so the visible
        // rows are a prefix of the natural render, exactly as pi's layout engine does (see its doc).
        let [top, _, title_area, _, body, _, hint, _, bottom] =
            stack_rows(area, [1, 1, title_h, 1, 1, 1, 1, 1, 1]);
        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(
            // E11: `new Text(theme.fg("accent", title), 1, 0)` (`extension-input.ts:50`).
            // `theme.fg` (`theme.ts:372-376`) applies a colour and nothing else — there is no
            // `theme.bold(...)` wrapper here, unlike e.g. `config-selector.ts:418-419` which does
            // compose the two. The `1` is `paddingX`, already carried by `title_lines`' leading
            // space.
            Paragraph::new(title_lines(&self.title))
                .style(theme.accent_style())
                .wrap(ratatui::widgets::Wrap { trim: false }),
            title_area,
        );
        // E10: `Input.render` opens with `const prompt = "> ";` (`input.ts:380`) — two columns, at
        // column 0, with no colour applied anywhere in the function, and `ExtensionInputComponent`
        // adds the `Input` as a bare child (`extension-input.ts:63-64`) with no padding wrapper to
        // shift it. cyrup drew a three-column accent `" > "`, i.e. one column in and cyan.
        //
        // E8: the caret is unconditional. `Input.render` always builds `cursorChar =
        // "\x1b[7m" + atCursor + "\x1b[27m"` with `atCursor` defaulting to a space at end-of-value
        // (`input.ts:426-437`), and the placeholder cannot suppress it because
        // `ExtensionInputComponent` never passes one on: it binds the parameter as `_placeholder`
        // and never reads it (`extension-input.ts:36`). cyrup swapped the caret out for muted
        // placeholder text whenever the buffer was empty — precisely the moment the user most needs
        // to see where typing will land, and the dialog showed no cursor at all.
        let mut spans = vec![Span::styled("> ", theme.base_style())];
        spans.extend(search_input_spans(&self.buffer, self.cursor, theme));
        frame.render_widget(Paragraph::new(Line::from(spans)), body);
        frame.render_widget(
            Paragraph::new(vec![self.hint_line(theme)]).style(theme.base_style()),
            hint,
        );
        frame.render_widget(border_rule(bottom.width, theme), bottom);
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use ratatui::style::Modifier;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Render at its natural height and hand back the ratatui `Buffer`, so assertions can read
    /// STYLE (the caret's `REVERSED`, the title's `BOLD`) and not only glyphs.
    fn buffer_of(sel: &mut TextInputSelector, w: u16) -> ratatui::buffer::Buffer {
        let theme = UiTheme::dark();
        let h = sel.desired_height(w);
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        term.draw(|f| sel.render(f, f.area(), &theme)).expect("draw");
        term.backend().buffer().clone()
    }

    fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        let mut s = String::new();
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        s
    }

    /// The `Input` row of the envelope. `ExtensionInputComponent`'s children are
    /// `DynamicBorder`(`extension-input.ts:47`) · `Spacer`(:48) · title(:50) · `Spacer`(:52) ·
    /// `Input`(:63) · … — the fifth child, index 4, for a one-line title.
    const INPUT_ROW: u16 = 4;

    /// **E8.** The caret is unconditional; the placeholder is never drawn.
    ///
    /// `Input.render` always builds one (`input.ts:426-437`):
    /// `const atCursor = cursorGrapheme?.segment ?? " "` then
    /// `` const cursorChar = `\x1b[7m${atCursor}\x1b[27m` `` — reverse video, with a space when the
    /// value is empty. And it cannot be suppressed by a placeholder, because
    /// `ExtensionInputComponent` binds that parameter as `_placeholder` and never reads it
    /// (`extension-input.ts:36`); `Input` has no placeholder concept at all.
    ///
    /// cyrup swapped the caret out for muted placeholder text whenever the buffer was empty — the
    /// one moment the user most needs to see where typing will land.
    #[test]
    fn an_empty_input_still_shows_its_caret_and_never_the_placeholder() {
        let mut sel = TextInputSelector::new("Name?".to_string(), Some("e.g. Ada".to_string()));
        let buf = buffer_of(&mut sel, 40);
        let row = row_text(&buf, INPUT_ROW);
        assert!(
            !row.contains("e.g. Ada"),
            "E8: `_placeholder` is discarded upstream (`extension-input.ts:36`): {row:?}"
        );
        // Column 2 is the cell right after the two-column `"> "` prompt: the caret.
        let caret = buf.cell((2, INPUT_ROW)).unwrap();
        assert!(
            caret.modifier.contains(Modifier::REVERSED),
            "E8: the reverse-video caret (`input.ts:437`) must be at the head of an empty field, \
             row {row:?}"
        );
    }

    /// MIRROR of E8. The caret is not merely *present*, it TRACKS the cursor: after typing and one
    /// `Left`, it lands on the character it is over (`atCursor` is the first grapheme after the
    /// cursor, `input.ts:426-431`) rather than staying at the head of the field.
    #[test]
    fn the_caret_follows_the_cursor_through_keystrokes() {
        let mut sel = TextInputSelector::new("Name?".to_string(), None);
        let km = SelectKeymap::default();
        for c in "abc".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        sel.handle(&key(KeyCode::Left), &km);
        let buf = buffer_of(&mut sel, 40);
        // `"> "` (2) + `ab` (2) ⇒ the caret is over the `c` at column 4.
        let caret = buf.cell((4, INPUT_ROW)).unwrap();
        assert!(caret.modifier.contains(Modifier::REVERSED), "{:?}", row_text(&buf, INPUT_ROW));
        assert_eq!(caret.symbol(), "c", "the caret highlights the character it is on");
        assert!(!buf.cell((2, INPUT_ROW)).unwrap().modifier.contains(Modifier::REVERSED));
    }

    /// **E10.** The prompt is a plain two-column `"> "` at column 0.
    ///
    /// `Input.render` opens with `const prompt = "> ";` (`input.ts:380`) and applies no colour to it
    /// anywhere in the function; `ExtensionInputComponent` adds the `Input` as a bare child
    /// (`extension-input.ts:63-64`), with no `Text` wrapper to inset it — unlike the title and hint
    /// rows, which are `new Text(..., 1, 0)`. cyrup drew a three-column accent `" > "`.
    #[test]
    fn the_input_prompt_is_a_plain_two_column_marker_at_column_zero() {
        let mut sel = TextInputSelector::new("Name?".to_string(), None);
        let km = SelectKeymap::default();
        for c in "hi".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        let buf = buffer_of(&mut sel, 40);
        let row = row_text(&buf, INPUT_ROW);
        assert!(row.starts_with("> hi"), "E10: `\"> \"` at column 0 (`input.ts:380`): {row:?}");
        let theme = UiTheme::dark();
        let prompt = buf.cell((0, INPUT_ROW)).unwrap();
        assert_ne!(
            prompt.fg,
            theme.accent_style().fg.unwrap(),
            "E10: the prompt is unstyled upstream, not accent: {row:?}"
        );
    }

    /// **E11.** The title is plain accent — colour only, no bold.
    ///
    /// `new Text(theme.fg("accent", title), 1, 0)` (`extension-input.ts:50`), and `theme.fg`
    /// (`theme.ts:372-376`) applies a colour and returns. Upstream composes the two when it wants
    /// both (`config-selector.ts:418-419` is `theme.fg(..., theme.bold(label))`); this title is not
    /// one of those.
    #[test]
    fn the_dialog_title_is_accent_without_bold() {
        let mut sel = TextInputSelector::new("Name?".to_string(), None);
        let buf = buffer_of(&mut sel, 40);
        let theme = UiTheme::dark();
        // The title row is child index 2, inset one column by its `paddingX = 1`.
        let cell = buf.cell((1, 2)).unwrap();
        assert_eq!(cell.symbol(), "N", "title row: {:?}", row_text(&buf, 2));
        assert_eq!(cell.fg, theme.accent_style().fg.unwrap(), "the title is accent");
        assert!(
            !cell.modifier.contains(Modifier::BOLD),
            "E11: `theme.fg` is colour-only — nothing bolds this title"
        );
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
