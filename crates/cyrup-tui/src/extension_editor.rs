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

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::component::Component;
use crate::editor::{EditorOutcome, InputEditor};
use crate::keymap::{Action, EditorAction, Keymap, SelectAction, SelectKeymap};
use crate::selector::{
    Selector, SelectorOutcome, border_rule, stack_rows, title_lines, title_wrapped_height,
};
use crate::theme::UiTheme;

/// The input-slot occupant for a loaded extension's `ui.editor` dialog (`SelectorKind::
/// ExtensionEditor`). Wraps an [`InputEditor`] seeded with the guest's `initial` text and labeled
/// `title` (Pi `editor(title, prefill)`, `types.ts:216`).
pub struct ExtensionEditorSelector {
    title: String,
    editor: InputEditor,
    /// Set by `Ctrl+G`; cleared by [`Selector::apply_external_edit`].
    external_editor_requested: bool,
    /// The host terminal's row count, fed by [`Selector::set_terminal_height`] every frame. `24`
    /// until the first one lands (pi's own `terminalHeight ?? 24` fallback,
    /// `config-selector.ts:264-266`). Drives E12's body budget below.
    term_rows: u16,
    /// The live `tui.select.*` table, so the hint row names the user's own submit/cancel keys from
    /// the FIRST paint (`keyHint` re-resolves through `keyText` on every render,
    /// `keybinding-hints.ts:34-44`). Refreshed from whatever table routed the last key.
    select_keymap: SelectKeymap,
    /// The live app table, for the `app.editor.external` hint pair. Same reason.
    app_keymap: Keymap,
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
        Self {
            title,
            editor,
            external_editor_requested: false,
            term_rows: 24,
            select_keymap: SelectKeymap::default(),
            app_keymap: Keymap::default(),
        }
    }

    /// Bind the hint row to the app's live `tui.select.*` and app-level tables (E9), so the first
    /// paint already names the keys the user actually has bound. Upstream never shows a stale label:
    /// every `keyHint(...)` in `extension-editor.ts:83-89` resolves through `keyText` →
    /// `getKeybindings().getKeys(...)` on each render against the one live table
    /// (`keybinding-hints.ts:34-44`). The twin of
    /// [`crate::text_input::TextInputSelector::with_keymap`], which needs only the select table
    /// because its two pairs are both `tui.select.*`; this row also needs `app.editor.external`.
    #[must_use]
    pub fn with_keymaps(mut self, select: &SelectKeymap, app: &Keymap) -> Self {
        self.select_keymap = select.clone();
        self.app_keymap = app.clone();
        self
    }

    /// The hint row (`extension-editor.ts:83-90`), composed live:
    ///
    /// ```text
    /// const hint = keyHint("tui.select.confirm", "submit") + "  " +
    ///              keyHint("tui.input.newLine",  "newline") + "  " +
    ///              keyHint("tui.select.cancel",  "cancel")  +
    ///              `  ${keyHint("app.editor.external", "external editor")}`;
    /// this.addChild(new Text(hint, 1, 0));
    /// ```
    ///
    /// E9 is two defects in one row. `keyHint` is `theme.fg("dim", keyText(kb)) +
    /// theme.fg("muted", ` ${description}`)` (`keybinding-hints.ts:43`) — a **two-tone** pair, dim
    /// key and muted description — and the `1` in `new Text(hint, 1, 0)` is `paddingX`, so the row
    /// is inset one column (`text.ts:64-71`). cyrup rendered one flat `muted` span at column 0. It
    /// was also a `const &str`, so a rebound `Shift+Enter` or `Ctrl+G` could never be reflected
    /// (SYS-6). `tui.input.newLine` maps onto cyrup's `editor.newLine`
    /// ([`EditorAction::NewLine`]), read off the embedded editor's own keymap.
    fn hint_pairs(&self) -> Vec<(String, &'static str)> {
        let mut pairs: Vec<(String, &'static str)> = Vec::with_capacity(4);
        if let Some(k) = self.select_keymap.keys_label(SelectAction::Confirm) {
            pairs.push((k, "submit"));
        }
        if let Some(k) = self.editor.keymap_ref().keys_label(EditorAction::NewLine) {
            pairs.push((k, "newline"));
        }
        if let Some(k) = self.select_keymap.keys_label(SelectAction::Cancel) {
            pairs.push((k, "cancel"));
        }
        if let Some(k) = self.app_keymap.keys_label(Action::ExternalEditor) {
            pairs.push((k, "external editor"));
        }
        pairs
    }

    /// The hint row's plain text — the exact string `new Text(hint, 1, 0)` is constructed from once
    /// its colours are stripped, so it wraps identically to [`Self::hint_lines`] and can be measured
    /// without a theme.
    fn hint_text(&self) -> String {
        self.hint_pairs()
            .iter()
            .map(|(keys, desc)| format!("{} {desc}", crate::chrome::format_key_text(keys, false)))
            .collect::<Vec<_>>()
            .join("  ")
    }

    /// How many ROWS the hint occupies at `width` (E16).
    ///
    /// `this.addChild(new Text(hint, 1, 0))` (`extension-editor.ts:90`), and `Text.render` WRAPS:
    /// `const contentWidth = Math.max(1, width - this.paddingX * 2)` then
    /// `wrapTextWithAnsi(normalizedText, contentWidth)`, pushing one output row per wrapped line
    /// (`text.ts:60-87`). It is a multi-row component, not a single line.
    ///
    /// cyrup returned one non-wrapping [`Line`] in a plain `Paragraph`, so at any width below the
    /// row's own the tail was CLIPPED. That is a regression, not an inherited limitation: the
    /// `const HINT` this replaced was 69 columns and fit an 80-column terminal, while the live row
    /// is 87 (`enter submit  shift+enter/ctrl+j newline  escape/ctrl+c cancel  ctrl+g external
    /// editor`, plus the one-column inset) — so `ctrl+g external editor`, the only affordance a user
    /// cannot guess, vanished on the single most common terminal width.
    fn hint_rows(&self, width: u16) -> u16 {
        let plain = Line::from(Span::raw(self.hint_text()));
        crate::transcript::text_lines_of(&plain, usize::from(width), 1)
            .len()
            .clamp(1, usize::from(u16::MAX)) as u16
    }

    /// The hint's rendered rows, two-tone and wrapped — [`Self::hint_rows`] of them.
    fn hint_lines(&self, theme: &UiTheme, width: u16) -> Vec<Line<'static>> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, (keys, desc)) in self.hint_pairs().iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.extend(crate::chrome::key_hint_spans(keys, desc, theme));
        }
        crate::transcript::text_lines_of(&Line::from(spans), usize::from(width), 1)
    }

    /// Whether `Ctrl+G` was pressed since the last drain — the chrome checks this via
    /// [`Selector::external_edit_text`] instead (a `Some` return already implies the request), so
    /// this exists purely for direct unit testing of the selector in isolation.
    #[cfg(test)]
    fn external_editor_requested(&self) -> bool {
        self.external_editor_requested
    }

    /// The embedded editor's TEXT rows (E12).
    ///
    /// `extension-editor.ts:70` builds the dialog's body as `new Editor(tui, getEditorTheme(),
    /// options)` — the very same class the chat input is — so `editor.ts:499-501`'s
    /// `maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3))` governs it too, and it is
    /// windowed by `layoutLines.slice(scrollOffset, scrollOffset + maxVisibleLines)` (`:519`).
    /// cyrup instead had its own hardcoded `MIN_BODY_ROWS: u16 = 3` / `MAX_BODY_ROWS: u16 = 14`
    /// pair, which overflowed a short terminal (14 rows where pi gives 7 at 24 rows) and truncated
    /// a tall one (14 where pi gives 24 at 80).
    ///
    /// Fixing E3 in `app::region_constraints` does NOT reach here: that function's editor branch is
    /// the `state.selector.is_none()` arm, and this dialog IS the selector — its rows come from
    /// [`Selector::desired_height`]. The two now share
    /// [`crate::app::max_visible_editor_lines`].
    fn body_rows(&self, width: u16) -> u16 {
        let cap = crate::app::max_visible_editor_lines(self.term_rows);
        // Measured at the width it RENDERS at, and over VISUAL (wrapped) lines — `layoutLines`, not
        // `state.lines` (`editor.ts:497`, `:519`). E15's rule applied to the dialog's own slot.
        // At least one row, exactly as pi's `layoutText` always emits at least one `LayoutLine`
        // (`editor.ts:905-915` pushes an empty cursor line for an empty buffer).
        (self
            .editor
            .visual_line_count(usize::from(self.editor.layout_width(width)))
            .clamp(1, usize::from(cap))) as u16
    }
}

impl Selector for ExtensionEditorSelector {
    fn desired_height(&self, width: u16) -> u16 {
        title_wrapped_height(&self.title, width)
            .saturating_add(self.body_rows(width))
            .saturating_add(2) // InputEditor's own top+bottom rule (Component::render)
            .saturating_add(2) // this selector's own top rule + BOTTOM rule (E5)
            .saturating_add(self.hint_rows(width)) // the hint `Text`, N wrapped rows (E16)
            .saturating_add(4) // the four envelope `Spacer(1)` rows (E7)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = title_wrapped_height(&self.title, area.width);
        let body_h = self.body_rows(area.width).saturating_add(2);
        let hint_h = self.hint_rows(area.width);
        // E5 + E7. `ExtensionEditorComponent`'s full child list (`extension-editor.ts:62-95`):
        //   `DynamicBorder`(:62) · `Spacer`(:63) · title(:66) · `Spacer`(:67) · `Editor`(:78) ·
        //   `Spacer`(:80) · hint(:83-90) · `Spacer`(:92) · `DynamicBorder`(:95).
        // Identical to `extension-input.ts:47-70`'s shape. E5 is `:95` — the dialog opened with a
        // rule and never closed, bleeding into the footer; `:62` alone was ported. E7 is the four
        // spacers. All heights are natural and the blanks unconditional; `stack_rows` fills the
        // regions from the TOP and starves the trailing ones, so the visible rows are a prefix of
        // the natural render, exactly as pi's layout engine does (see its doc).
        let [top, _, title_area, _, body, _, hint, _, bottom] =
            stack_rows(area, [1, 1, title_h, 1, body_h, 1, hint_h, 1, 1]);
        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(
            // E11: `new Text(theme.fg("accent", title), 1, 0)` (`extension-editor.ts:66`).
            // `theme.fg` is colour-only (`theme.ts:372-376`); nothing bolds this title.
            Paragraph::new(title_lines(&self.title))
                .style(theme.accent_style())
                .wrap(ratatui::widgets::Wrap { trim: false }),
            title_area,
        );
        self.editor.render(frame, body, theme);
        // E9: two-tone `keyHint` pairs, inset one column. E16: WRAPPED, one frame row per wrapped
        // line, exactly as `Text.render` emits them (`text.ts:60-87`). See [`Self::hint_rows`].
        frame.render_widget(
            Paragraph::new(self.hint_lines(theme, area.width)).style(theme.base_style()),
            hint,
        );
        // E5: the closing `DynamicBorder` (`extension-editor.ts:95`).
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn set_terminal_height(&mut self, rows: u16) {
        self.term_rows = rows.max(1);
        // E17: the embedded `Editor` caps ITSELF from `this.tui.terminal.rows` (`editor.ts:499-501`)
        // — it reads the same `tui` the dialog was constructed with (`extension-editor.ts:70`), not
        // the dialog's slot. Without this the body would size to `max(5, floor(term * 0.3))` here
        // and then draw only `max(5, floor(24 * 0.3)) = 7` of those rows.
        self.editor.set_terminal_height(rows.max(1));
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Keep the hint row honest: adopt whatever table actually routed this key (the refresh
        // `ListSelector::handle` / `TextInputSelector::handle` already do).
        self.select_keymap = keymap.clone();
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Render into a `w`×`h` buffer and return the rows, trailing whitespace trimmed.
    fn rows_at(sel: &mut ExtensionEditorSelector, w: u16, h: u16) -> Vec<String> {
        let theme = UiTheme::dark();
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        term.draw(|f| sel.render(f, f.area(), &theme))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..buf.area.width {
                    if let Some(cell) = buf.cell((x, y)) {
                        line.push_str(cell.symbol());
                    }
                }
                line.trim_end().to_string()
            })
            .collect()
    }

    fn is_rule(row: &str) -> bool {
        !row.is_empty() && row.chars().all(|c| c == '─')
    }

    /// Render at `w`×`h` and hand back the `Buffer`, so assertions can read STYLE.
    fn buffer_at(sel: &mut ExtensionEditorSelector, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let theme = UiTheme::dark();
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        term.draw(|f| sel.render(f, f.area(), &theme))
            .expect("draw");
        term.backend().buffer().clone()
    }

    /// **E9.** The hint row is two-tone `keyHint` pairs, inset one column, resolved live.
    ///
    /// ```text
    /// const hint = keyHint("tui.select.confirm", "submit") + "  " +
    ///              keyHint("tui.input.newLine",  "newline") + "  " +
    ///              keyHint("tui.select.cancel",  "cancel")  +
    ///              `  ${keyHint("app.editor.external", "external editor")}`;
    /// this.addChild(new Text(hint, 1, 0));                      // extension-editor.ts:83-90
    /// ```
    ///
    /// `keyHint` is `theme.fg("dim", keyText(kb)) + theme.fg("muted", ` ${description}`)`
    /// (`keybinding-hints.ts:43`) and the `1` is `paddingX` (`text.ts:64-71`). cyrup rendered one
    /// flat `muted` span at column 0, from a compile-time `const &str` that no rebind could ever
    /// reach.
    ///
    /// **E16** rides along, because it is the same row and the width is the point. At the STOCK
    /// tables the row is
    /// `enter submit  shift+enter/ctrl+j newline  escape/ctrl+c cancel  ctrl+g external editor` —
    /// 86 columns, 87 with the inset — so on an 80-column terminal it MUST occupy two rows.
    /// `this.addChild(new Text(hint, 1, 0))` (`extension-editor.ts:90`) and `Text.render` wraps at
    /// `contentWidth = width - paddingX * 2` and pushes one row per wrapped line (`text.ts:64-87`).
    /// cyrup emitted a single non-wrapping `Line` in a plain `Paragraph`, so `ctrl+g external
    /// editor` — the one affordance a user cannot guess — was clipped off the right edge.
    ///
    /// Run at 80, not at the 100 this test used to use. 100 is the one width in the plausible range
    /// where the defect is invisible: the row fits, and the test's own comment conceded as much
    /// while asserting nothing about it.
    #[test]
    fn the_hint_row_is_two_tone_key_pairs_inset_one_column() {
        const W: u16 = 80;
        let mut sel = ExtensionEditorSelector::new("t".to_string(), "")
            .with_keymaps(&SelectKeymap::default(), &Keymap::default());
        let h = sel.desired_height(W);

        // E16, in `desired_height`: the envelope is 2 dialog rules + 4 `Spacer(1)` + 1 title row +
        // the embedded editor's own 2 rules + its 1 text row = 10, so a ONE-row hint would make
        // this 11. It is 12.
        assert_eq!(
            h, 12,
            "E16: the wrapped hint is worth TWO rows in `desired_height`"
        );

        let buf = buffer_at(&mut sel, W, h);
        let text_of = |y: u16| {
            let mut s = String::new();
            for x in 0..W {
                s.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            s.trim_end().to_string()
        };
        // … hint(2 rows) · Spacer(:92) · DynamicBorder(:95).
        let hint_y = h - 4;
        let (first, second) = (text_of(hint_y), text_of(hint_y + 1));
        let joined = format!("{first}\n{second}");

        // Both rows carry the `paddingX = 1` inset — `Text.render` prefixes `leftMargin` to EVERY
        // wrapped line (`text.ts:70-76`), not just the first.
        assert!(
            first.starts_with(' '),
            "E9: inset one column (`new Text(hint, 1, 0)`): {first:?}"
        );
        assert!(
            second.starts_with(' '),
            "E16: the wrapped row is inset too: {second:?}"
        );
        assert!(
            !second.trim().is_empty(),
            "E16: the row wrapped, it did not clip: {joined:?}"
        );

        // All four upstream affordances survive the wrap, in upstream order.
        for want in ["submit", "newline", "cancel", "external editor"] {
            assert!(
                joined.contains(want),
                "the `{want}` pair (`:83-89`) is missing: {joined:?}"
            );
        }
        assert!(
            joined.find("submit") < joined.find("newline")
                && joined.find("newline") < joined.find("cancel")
                && joined.find("cancel") < joined.find("external editor"),
            "upstream order (`:83-89`): {joined:?}"
        );
        // The one that used to be clipped: `ctrl+g external editor` is 22 columns and the row runs
        // out at 78, so it can only be present if the hint really wrapped.
        assert!(
            joined.contains("ctrl+g") && joined.contains("external editor"),
            "E16: the external-editor pair is exactly what a clip drops: {joined:?}"
        );
        // `keyText` joins EVERY key bound to the action with `/` (`keybinding-hints.ts:29-36`), and
        // `tui.input.newLine` ships with two (`tui/src/keybindings.ts:137`,
        // `defaultKeys: ["shift+enter", "ctrl+j"]`) — so this row is `EditorKeymap::keys_label`, not
        // the first-key `key_label`.
        assert!(
            first.contains("shift+enter/ctrl+j newline"),
            "every key bound to `tui.input.newLine`, joined with `/`: {first:?}"
        );

        // Two-tone: the KEY is `dim`, its description `muted` — two different colours on one row.
        let theme = UiTheme::dark();
        let key_x = first.find("enter").expect("the confirm key label") as u16;
        let desc_x = first.find("submit").expect("its description") as u16;
        assert_eq!(
            buf.cell((key_x, hint_y)).expect("cell").fg,
            theme.dim_style().fg.expect("dim fg"),
            "E9: the key is `dim` (`keybinding-hints.ts:43`): {first:?}"
        );
        assert_eq!(
            buf.cell((desc_x, hint_y)).expect("cell").fg,
            theme.muted_style().fg.expect("muted fg"),
            "E9: the description is `muted`: {first:?}"
        );
        assert_ne!(
            theme.dim_style().fg,
            theme.muted_style().fg,
            "sanity: `dim` and `muted` are distinct tokens, so the two-tone check has teeth"
        );
        // E16 must not cost E9 its colours: the WRAPPED row is two-tone too. `wrapTextWithAnsi`
        // carries the active ANSI runs across the break (`utils.ts:770-798`).
        let ext_x = second
            .find("external editor")
            .expect("the wrapped description") as u16;
        assert_eq!(
            buf.cell((ext_x, hint_y + 1)).expect("cell").fg,
            theme.muted_style().fg.expect("muted fg"),
            "E16: the wrapped description keeps its `muted` colour: {second:?}"
        );
    }

    /// MIRROR of E9. `keyHint` re-resolves through `keyText` → `getKeybindings().getKeys(...)` on
    /// every render (`keybinding-hints.ts:34-44`), so a rebind shows up — including on the FIRST
    /// paint, before any keystroke has told the dialog which table is live.
    #[test]
    fn the_hint_row_names_the_users_own_keys_on_the_first_paint() {
        const W: u16 = 100;
        let mut rebound = SelectKeymap::default();
        rebound.set_action(SelectAction::Confirm, vec![crate::keymap::Key::ctrl('s')]);
        let mut app = Keymap::default();
        app.set_action(Action::ExternalEditor, vec![crate::keymap::Key::ctrl('x')]);

        let mut sel =
            ExtensionEditorSelector::new("t".to_string(), "").with_keymaps(&rebound, &app);
        let h = sel.desired_height(W);
        let rows = rows_at(&mut sel, W, h);
        let hint = &rows[usize::from(h) - 3];
        assert!(
            hint.contains("ctrl+s submit"),
            "the rebound confirm key: {hint:?}"
        );
        assert!(
            hint.contains("ctrl+x external editor"),
            "the rebound external key: {hint:?}"
        );
        assert!(
            !hint.contains("enter submit"),
            "the stock label must be gone: {hint:?}"
        );
    }

    /// **E11.** The dialog title is plain accent — `new Text(theme.fg("accent", title), 1, 0)`
    /// (`extension-editor.ts:66`), and `theme.fg` (`theme.ts:372-376`) applies a colour and nothing
    /// else. cyrup added `Modifier::BOLD`.
    #[test]
    fn the_dialog_title_is_accent_without_bold() {
        let mut sel = ExtensionEditorSelector::new("Commit message".to_string(), "");
        let h = sel.desired_height(60);
        let buf = buffer_at(&mut sel, 60, h);
        // Title row = child index 2, inset one column by its `paddingX = 1`.
        let cell = buf.cell((1, 2)).expect("cell");
        assert_eq!(cell.symbol(), "C", "the title row");
        assert_eq!(
            cell.fg,
            UiTheme::dark().accent_style().fg.expect("accent fg")
        );
        assert!(
            !cell.modifier.contains(ratatui::style::Modifier::BOLD),
            "E11: `theme.fg` is colour-only — nothing bolds this title"
        );
    }

    /// **E12.** The embedded body obeys the SAME `max(5, floor(terminalRows * 0.3))` cap as the chat
    /// editor, because `extension-editor.ts:70` builds it as `new Editor(tui, getEditorTheme(),
    /// options)` — the same class, so `editor.ts:499-501` and `:519` apply verbatim.
    ///
    /// cyrup had its own `MIN_BODY_ROWS: u16 = 3` / `MAX_BODY_ROWS: u16 = 14`, which overflowed a
    /// short terminal (14 rows where pi gives 7) and truncated a tall one (14 where pi gives 24).
    /// Fixing E3 in `app::region_constraints` does not reach here: this dialog IS the selector, so
    /// its rows come from `desired_height`, not from that function's editor branch.
    #[test]
    fn the_embedded_editor_body_scales_with_the_terminal_height() {
        // Envelope around the body: 2 rules + 4 spacers + 1 title + 2 hint rows (E16 — at width 60
        // the 86-column hint wraps in two) = 9, plus the embedded editor's own 2 rules ⇒
        // `desired_height = body_text_rows + 11`.
        for (term_rows, want_body) in [(24u16, 7u16), (80, 24), (10, 5), (40, 12)] {
            let mut sel = ExtensionEditorSelector::new("t".to_string(), &"x\n".repeat(60));
            sel.set_terminal_height(term_rows);
            assert_eq!(
                sel.desired_height(60),
                want_body + 11,
                "at {term_rows} terminal rows the body is max(5, floor({term_rows} * 0.3)) = \
                 {want_body} text rows"
            );

            // E17: and the embedded `Editor` must actually DRAW those rows. It caps ITSELF from
            // `this.tui.terminal.rows` (`editor.ts:499-501`) — the same `tui` the dialog was
            // constructed with (`extension-editor.ts:70`) — so the dialog has to hand its own
            // terminal height down. Reserving the slot and filling it are two different fixes; with
            // only the first, a tall terminal reserves 24 rows and the editor draws its default 7.
            let h = sel.desired_height(60);
            let rows = rows_at(&mut sel, 60, h);
            // A `createScrollBorder` indicator is a rule too — it only OPENS with `───`
            // (`editor.ts:261`) — so "is entirely `─`" would walk straight past the embedded
            // editor's scrolled top rule and measure the wrong pair of rows.
            let any_rule = |r: &String| r.starts_with('─');
            let find_rule_from = |from: usize| {
                rows.iter()
                    .enumerate()
                    .skip(from)
                    .find(|(_, r)| any_rule(r))
                    .map(|(i, _)| i)
            };
            let first = find_rule_from(0).expect("the dialog's top rule");
            let body_top = find_rule_from(first + 1).expect("the embedded editor's top rule");
            let body_bottom =
                find_rule_from(body_top + 1).expect("the embedded editor's bottom rule");
            assert_eq!(
                body_bottom - body_top - 1,
                usize::from(want_body),
                "at {term_rows} terminal rows the embedded editor reserves {want_body} text rows: \
                 {rows:?}"
            );
            // …and it FILLS them. The region being `want_body` tall is `desired_height`'s doing;
            // only the embedded editor's own scroll rule reports how many rows it actually drew.
            // `"x\n".repeat(60)` is 61 layout lines with the caret on the last, so the rows hidden
            // above are `61 - want_body` — and 54 (its `?? 24` default's 7) if the dialog never
            // handed its terminal height down.
            assert!(
                rows[body_top].starts_with(&format!("─── ↑ {} more ", 61 - want_body)),
                "at {term_rows} terminal rows the embedded editor DRAWS {want_body} text rows: \
                 {rows:?}"
            );
        }
    }

    /// MIRROR of E12. The cap is a ceiling: a two-line buffer is two rows at every terminal height,
    /// never padded up to a floor cyrup invented (`MIN_BODY_ROWS = 3`) and never stretched to the
    /// cap. pi slices `layoutLines`, which is simply shorter than `maxVisibleLines` here.
    #[test]
    fn a_short_buffer_is_not_padded_up_to_the_cap() {
        for term_rows in [10u16, 24, 80] {
            let mut sel = ExtensionEditorSelector::new("t".to_string(), "one\ntwo");
            sel.set_terminal_height(term_rows);
            assert_eq!(
                sel.desired_height(60),
                2 + 11,
                "two lines stay two at {term_rows} rows"
            );
        }
    }

    /// E5 + E7. `ExtensionEditorComponent`'s children (`extension-editor.ts:62-95`):
    /// `DynamicBorder`(:62) · `Spacer`(:63) · title(:66) · `Spacer`(:67) · `Editor`(:78) ·
    /// `Spacer`(:80) · hint(:83-90) · `Spacer`(:92) · `DynamicBorder`(:95).
    ///
    /// E5 is that last child: cyrup laid out exactly `[top, title, body, hint]` and rendered the
    /// hint last, so the dialog opened with a rule and never closed — it bled straight into the
    /// footer. E7 is the four blank rows. The embedded `InputEditor` draws its OWN rules inside the
    /// body region, which is why the row count below is `1+1+1+1+3+1+2+1+1 = 12`.
    ///
    /// That `3` is E12: an empty buffer is ONE `LayoutLine` (`editor.ts:905-915`), so the embedded
    /// `Editor` is `1 rule + 1 text + 1 rule`. The old count of 13 came from cyrup's own
    /// `MIN_BODY_ROWS: u16 = 3` floor, which has no upstream counterpart — pi pads nothing.
    ///
    /// The `2` is E16: at width 60 the 86-column hint is a two-row `Text` (`text.ts:64-87`).
    #[test]
    fn envelope_has_four_spacers_a_hint_row_and_a_closing_border() {
        let mut sel = ExtensionEditorSelector::new("edit demo".to_string(), "");
        let h = sel.desired_height(60);
        assert_eq!(
            h, 12,
            "title + 3 editor rows + 2 hint rows + 2 rules + 4 spacers"
        );
        let rows = rows_at(&mut sel, 60, h);
        assert!(
            is_rule(&rows[0]),
            "the opening DynamicBorder (:62): {rows:?}"
        );
        assert_eq!(rows[1], "", "Spacer(1) (:63): {rows:?}");
        assert!(rows[2].contains("edit demo"), "the title (:66): {rows:?}");
        assert_eq!(rows[3], "", "Spacer(1) (:67): {rows:?}");
        assert_eq!(rows[7], "", "Spacer(1) after the editor (:80): {rows:?}");
        assert!(
            rows[8].contains("submit"),
            "the hint row (:83-90): {rows:?}"
        );
        assert!(
            rows[9].contains("external editor"),
            "E16: the hint's WRAPPED second row (`text.ts:64-87`), not a spacer: {rows:?}"
        );
        assert_eq!(rows[10], "", "Spacer(1) (:92): {rows:?}");
        assert!(
            is_rule(&rows[11]),
            "E5 — the CLOSING DynamicBorder (:95), which the dialog never drew: {rows:?}"
        );
        // Exactly four blank rows belong to the ENVELOPE. Rows 4..=6 are the `InputEditor`'s own
        // region (its two rules plus its empty text line) and are not counted.
        let envelope_blanks = rows
            .iter()
            .enumerate()
            .filter(|(i, r)| !(4..=6).contains(i) && r.is_empty())
            .count();
        assert_eq!(
            envelope_blanks, 4,
            "exactly four envelope spacers: {rows:?}"
        );
    }

    /// MIRROR of the E5 assertion. The rule the dialog always had is the one at the TOP; a test
    /// that only counted `─` rows would pass with the bottom border still missing, so this pins the
    /// count at two and stays green either way once the top rule exists.
    #[test]
    fn the_dialog_is_delimited_by_exactly_two_rules() {
        let mut sel = ExtensionEditorSelector::new("t".to_string(), "");
        let h = sel.desired_height(60);
        let rows = rows_at(&mut sel, 60, h);
        // The embedded `InputEditor` draws its own two rules inside the body region, so the
        // envelope's own pair is the first row and the last row specifically.
        assert!(
            is_rule(&rows[0]) && is_rule(&rows[h as usize - 1]),
            "{rows:?}"
        );
    }

    /// Height ladder. pi renders a dialog `Container` at its natural height and paints only the
    /// first `allocatedHeight` of those lines (`packages/tui/src/layout.ts:113,307-310`), so a
    /// short slot shows a strict PREFIX of the tall render and a one-row resize moves exactly one
    /// row. That is the property pinned here, at every height from 1 to natural.
    ///
    /// It replaces a weaker ladder that asserted the opposite of upstream — that below the natural
    /// height the `Spacer(1)` rows are "dropped wholesale" so row 1 is the title rather than
    /// `extension-editor.ts:63`'s blank. pi drops no `Spacer`, ever: the component has no height
    /// input to drop one on.
    ///
    /// The prefix is checked over the envelope's own rows, i.e. everything down to the start of the
    /// `Editor` child (`extension-editor.ts:78`) at index 4. Inside that region cyrup's
    /// [`InputEditor`] is a rect-driven widget that re-fits its own pair of rules to whatever
    /// height it is handed, so its interior does not clip like a fixed line vector; that widget is
    /// not part of this envelope and is unchanged here.
    #[test]
    fn a_short_slot_shows_a_strict_prefix_of_the_natural_render() {
        /// Index of the `Editor` child (`extension-editor.ts:78`); rows before it are the envelope.
        const ENVELOPE_HEAD: usize = 4;
        let natural_h = ExtensionEditorSelector::new("t".to_string(), "").desired_height(60);
        let full = rows_at(
            &mut ExtensionEditorSelector::new("t".to_string(), ""),
            60,
            natural_h,
        );
        for h in 1..=natural_h {
            let mut sel = ExtensionEditorSelector::new("t".to_string(), "");
            let rows = rows_at(&mut sel, 60, h);
            assert_eq!(rows.len(), usize::from(h));
            let head = usize::from(h).min(ENVELOPE_HEAD);
            assert_eq!(
                rows[..head],
                full[..head],
                "@h={h}: the rows must be the first {h} of the natural render {full:?}"
            );
        }
        // And concretely, the first four rows are `DynamicBorder`(:62) · `Spacer`(:63) ·
        // title(:66) · `Spacer`(:67) — the blank at index 1 is upstream's, not a lost row.
        assert!(is_rule(&full[0]), "{full:?}");
        assert_eq!(full[1], "", "{full:?}");
        assert!(full[2].contains('t'), "{full:?}");
        assert_eq!(full[3], "", "{full:?}");
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
            assert_eq!(
                sel.handle(&key(KeyCode::Char(c), KeyModifiers::NONE), &km),
                SelectorOutcome::Redraw
            );
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
        assert_eq!(
            sel.handle(&key(KeyCode::Esc, KeyModifiers::NONE), &km),
            SelectorOutcome::Cancel
        );
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
