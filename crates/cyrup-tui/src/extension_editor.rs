//! An extension `ui.editor` dialog rendered INLINE in the running TUI — Pi's DEFAULT
//! `ExtensionEditorComponent` (`modes/interactive/components/extension-editor.ts`), NOT a teardown
//! to `$VISUAL`/`$EDITOR`. Reuses the SAME multi-line [`InputEditor`] the main chat input already
//! is, exactly like Pi reuses its own shared `Editor` component
//! (`new Editor(tui, getEditorTheme(), options)`, `extension-editor.ts:65`): word navigation, the
//! kill ring, undo, wrap-aware cursor motion all come for free, and `Enter` submits / `Shift+Enter`
//! inserts a newline, matching Pi's `keyHint("tui.select.confirm", "submit")` +
//! `keyHint("tui.input.newLine", "newline")` hint row exactly.
//!
//! `$VISUAL`/`$EDITOR` is reachable ONLY via the explicit `Ctrl+G` (`app.editor.external`)
//! keybinding (`extension-editor.ts:107-121`) — a genuine escape hatch, never the default path. Since
//! [`Selector::handle`] cannot itself tear the terminal down and spawn a blocking child (it has no
//! access to the chrome's terminal handle), pressing `Ctrl+G` here only returns
//! [`SelectorOutcome::OpenExternalEditor`] (a request signal, never resolving the dialog); the chrome
//! (`crate::app::App::open_external_editor_for_selector`) performs the actual teardown, seeded via
//! [`Selector::external_edit_text`], and feeds a clean-exit result back via
//! [`Selector::apply_external_edit`] WITHOUT closing the dialog (Pi `this.editor.setText(newContent)`,
//! `extension-editor.ts:152` — the dialog stays open; only `Enter`/`Esc` resolve it).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::component::Component;
use crate::editor::{EditorOutcome, InputEditor};
use crate::keymap::SelectKeymap;
use crate::selector::{title_lines, title_wrapped_height, Selector, SelectorOutcome};
use crate::theme::UiTheme;

/// The embedded editor's body-row budget (Pi's real terminal-sized `Editor` has no such cap; a
/// fixed live-region slot does — spec/tui/05 §3). Clamped so a one-line seed doesn't waste rows and
/// a huge one doesn't blow the live region.
const MIN_BODY_ROWS: u16 = 3;
const MAX_BODY_ROWS: u16 = 14;

/// The hint row shown beneath the embedded editor (Pi's exact four affordances,
/// `extension-editor.ts:80-90`: `tui.select.confirm` "submit", `tui.input.newLine` "newline",
/// `tui.select.cancel` "cancel", `app.editor.external` "external editor" — literal text rather than
/// a live keymap lookup, matching the plain-text hint rows the sibling extension selectors already
/// use).
const HINT: &str = "enter submit  shift+enter newline  esc cancel  ctrl+g external editor";

/// The input-slot occupant for a loaded extension's `ui.editor` dialog (`SelectorKind::
/// ExtensionEditor`). Wraps an [`InputEditor`] seeded with the guest's `initial` text and labeled
/// `title` (Pi `editor(title, prefill)`, `types.ts:216`).
pub struct ExtensionEditorSelector {
    title: String,
    editor: InputEditor,
    /// Set by `Ctrl+G`; drained by [`Self::take_external_editor_request`].
    external_editor_requested: bool,
}

impl ExtensionEditorSelector {
    /// Build seeded with `initial`, labeled `title`.
    pub fn new(title: String, initial: &str) -> Self {
        let mut editor = InputEditor::new();
        editor.set_text(initial);
        editor.set_focused(true);
        // T9 (TUI-FIDELITY §2): Pi builds this dialog's editor as
        // `new Editor(tui, getEditorTheme(), options)` (v0.84.1 `components/extension-editor.ts:70`)
        // and never reassigns `borderColor`, so its rule stays `getEditorTheme().borderColor` =
        // `theme.fg("borderMuted", …)` (`theme.ts:1301-1304`). Only the *chat* editor is repainted
        // per reasoning level (`interactive-mode.ts:3990-3993`) — this one was inheriting
        // `InputEditor`'s `"medium"` thinking colour.
        editor.use_muted_border();
        Self { title, editor, external_editor_requested: false }
    }

    /// Whether `Ctrl+G` was pressed since the last drain — the chrome checks this via
    /// [`Selector::external_edit_text`] instead (a `Some` return already implies the request), so
    /// this exists purely for direct unit testing of the selector in isolation.
    #[cfg(test)]
    fn external_editor_requested(&self) -> bool {
        self.external_editor_requested
    }

    fn body_rows(&self) -> u16 {
        (self.editor.line_count().clamp(usize::from(MIN_BODY_ROWS), usize::from(MAX_BODY_ROWS))) as u16
    }
}

impl Selector for ExtensionEditorSelector {
    fn desired_height(&self, width: u16) -> u16 {
        title_wrapped_height(&self.title, width)
            .saturating_add(self.body_rows())
            .saturating_add(2) // InputEditor's own top+bottom rule (Component::render)
            .saturating_add(2) // this selector's own top rule + trailing hint line
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = title_wrapped_height(&self.title, area.width);
        let body_h = self.body_rows().saturating_add(2);
        let [top, title_area, body, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(title_h),
            Constraint::Length(body_h),
            Constraint::Length(1),
        ])
        .areas(area);
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
        self.editor.render(frame, body, theme);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(HINT, theme.muted_style()))),
            hint,
        );
    }

    fn handle(&mut self, key: &KeyEvent, _keymap: &SelectKeymap) -> SelectorOutcome {
        if key.code == KeyCode::Esc {
            return SelectorOutcome::Cancel;
        }
        // `Ctrl+G` (`app.editor.external`) — request-only; `Selector::handle` has no terminal
        // access to perform the actual teardown+spawn+restore (see module docs).
        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.external_editor_requested = true;
            return SelectorOutcome::OpenExternalEditor;
        }
        match self.editor.handle_key(key) {
            EditorOutcome::Submit(text) => SelectorOutcome::Confirm(text),
            EditorOutcome::Edited => SelectorOutcome::Redraw,
            EditorOutcome::Ignored => SelectorOutcome::Ignored,
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn external_edit_text(&self) -> Option<String> {
        Some(self.editor.text())
    }

    fn apply_external_edit(&mut self, text: &str) {
        self.editor.set_text(text);
        self.external_editor_requested = false;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    #[test]
    fn seeds_the_buffer_with_initial_text_and_carries_the_title() {
        let sel = ExtensionEditorSelector::new("edit demo".to_string(), "seed text");
        assert_eq!(sel.editor.text(), "seed text");
        assert_eq!(sel.title, "edit demo");
    }

    #[test]
    fn enter_confirms_with_the_current_buffer_text_not_the_seed() {
        let mut sel = ExtensionEditorSelector::new("t".to_string(), "seed");
        let km = SelectKeymap::default();
        for c in " more".chars() {
            assert_eq!(sel.handle(&key(KeyCode::Char(c), KeyModifiers::NONE), &km), SelectorOutcome::Redraw);
        }
        assert_eq!(
            sel.handle(&key(KeyCode::Enter, KeyModifiers::NONE), &km),
            SelectorOutcome::Confirm("seed more".to_string())
        );
    }

    #[test]
    fn esc_cancels_without_confirming() {
        let mut sel = ExtensionEditorSelector::new("t".to_string(), "seed");
        let km = SelectKeymap::default();
        assert_eq!(sel.handle(&key(KeyCode::Esc, KeyModifiers::NONE), &km), SelectorOutcome::Cancel);
    }

    /// L4 review §3 (TUI `ui.editor` always tears down to `$EDITOR`): `Ctrl+G` must be a REQUEST,
    /// never an immediate dialog resolution — Pi's `openExternalEditor` never calls
    /// `onSubmitCallback`/`onCancelCallback` (`extension-editor.ts:119-157`), it only ever mutates
    /// `this.editor`'s own buffer.
    #[test]
    fn ctrl_g_requests_the_external_editor_without_resolving_the_dialog() {
        let mut sel = ExtensionEditorSelector::new("t".to_string(), "seed");
        let km = SelectKeymap::default();
        assert!(!sel.external_editor_requested());
        assert_eq!(
            sel.handle(&key(KeyCode::Char('g'), KeyModifiers::CONTROL), &km),
            SelectorOutcome::OpenExternalEditor
        );
        assert!(sel.external_editor_requested());
        assert_eq!(sel.external_edit_text(), Some("seed".to_string()));
    }

    /// A clean external-editor exit rewrites the SAME buffer (Pi `this.editor.setText(newContent)`)
    /// — the dialog is untouched otherwise (title, request flag cleared).
    #[test]
    fn apply_external_edit_rewrites_the_buffer_and_clears_the_request_flag() {
        let mut sel = ExtensionEditorSelector::new("t".to_string(), "seed");
        let km = SelectKeymap::default();
        sel.handle(&key(KeyCode::Char('g'), KeyModifiers::CONTROL), &km);
        assert!(sel.external_editor_requested());
        sel.apply_external_edit("edited by nano");
        assert_eq!(sel.editor.text(), "edited by nano");
        assert!(!sel.external_editor_requested());
        // The dialog is still open and confirms with the NEW text, not the seed.
        assert_eq!(
            sel.handle(&key(KeyCode::Enter, KeyModifiers::NONE), &km),
            SelectorOutcome::Confirm("edited by nano".to_string())
        );
    }
}
