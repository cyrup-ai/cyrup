//! A reusable single-line [`Input`] editing surface — the port of pi's `Input` component
//! (`pi/packages/tui/src/components/input.ts:19-376` @v0.83.0) minus `render` — plus the
//! single-field [`TextInputSelector`] that wraps it for the `ui.input` extension dialog
//! (`SelectorKind::ExtensionInput`).
//!
//! Upstream every search box in every selector IS an `Input` (`new Input()` at
//! `model-selector.ts:117`, `session-selector.ts:332` and `:718`, `config-selector.ts:263`,
//! `oauth-selector.ts:76`, `scoped-models-selector.ts:139`, `settings-list.ts:70`,
//! `tree-selector.ts:1289`, `login-dialog.ts:55`, `extension-input.ts:63`), so all of them get the
//! same Emacs-grade line editor: word motion, the kill ring, undo with typing coalescing,
//! bracketed paste and grapheme-granular stepping. cyrup had seven private
//! insert-plus-backspace copies instead; they now embed this one type.
//!
//! Keys resolve through [`EditorKeymap::action_for`], never an inline `match key.code` — pi's
//! `handleInput` re-reads `getKeybindings()` on every keystroke (`input.ts:86`), so a
//! `keybindings.json` rebind reaches a search field the same way it reaches the editor.

use unicode_segmentation::UnicodeSegmentation;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::editor::kill_ring::{kill_ring_push, kill_ring_rotate};
use crate::editor::undo::{push_bounded, should_snapshot_for_type, UNDO_CAP};
use crate::editor::word_nav::{
    byte_seg_first_punct, byte_seg_is_whitespace, byte_seg_last_punct_end, byte_word_segments,
    find_word_backward, find_word_forward,
};
use crate::keymap::{EditorAction, EditorKeymap, SelectAction, SelectKeymap};
use crate::selector::{
    border_rule, input_line_spans, stack_rows, title_lines, title_wrapped_height, Selector,
    SelectorOutcome,
};
use crate::theme::UiTheme;

/// `lastAction` (`input.ts:34`): `"kill" | "yank" | "type-word" | null`. Gates kill-ring
/// accumulation, yank-pop eligibility and undo coalescing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    None,
    Kill,
    Yank,
    TypeWord,
}

/// What one key did to an [`Input`] — the return of [`Input::handle_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOutcome {
    /// Not an `Input` binding: the fall-through past `handleInput`'s last `if` (`input.ts:202-210`,
    /// where a control character is rejected), or an editor action with no single-line meaning.
    Ignored,
    /// The cursor moved; `value` is unchanged, so a host must NOT re-run its filter hook.
    ///
    /// [CYRUP-DELTA] pi re-filters unconditionally after handing a key to the search input
    /// (`model-selector.ts:410-411`, `this.searchInput.handleInput(...)` then
    /// `this.filterModels(...)`), which resets the highlight to the top on a bare `Left`. Splitting
    /// the outcome keeps the caret keys non-destructive; every value-changing key still reports
    /// [`Self::Edited`] and every host still re-filters on those.
    Moved,
    /// `value` changed — the host runs its post-edit hook (`on_query_changed`, `apply_filter`, …).
    Edited,
}

/// A single-line text editor: buffer, caret, kill ring and undo stack, with pi's `Input` key
/// surface (`input.ts:86-210`).
///
/// The caret is a **byte** offset into [`Self::value`] and is an invariant of the type: it is only
/// ever moved to a grapheme, word or kill-span boundary, all of which are char boundaries. Every
/// mutation nevertheless goes through [`Self::splice`], which rebuilds the string from two
/// `str::get` slices and no-ops on a bad index, so a violated invariant degrades to a dropped
/// keystroke instead of the panic `String::insert`/`replace_range` would raise (no-panic policy,
/// R-00-009).
pub struct Input {
    value: String,
    /// Byte offset into [`Self::value`]; always a char boundary (see the type doc).
    cursor: usize,
    /// `killRing` (`input.ts:33`), oldest-first with the most recent entry LAST — the orientation
    /// `kill-ring.ts` uses, so `peek` is `last`.
    kill_ring: Vec<String>,
    last_action: LastAction,
    /// `undoStack` (`input.ts:37`), pi's `UndoStack<InputState>` over `{ value, cursor }`.
    undo: Vec<(String, usize)>,
    /// The live `tui.editor.*` table, refreshed from the host on every key
    /// (`getKeybindings()`, `input.ts:86`).
    keymap: EditorKeymap,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// An empty field with the caret at 0 and the stock editor bindings.
    pub fn new() -> Self {
        Input {
            value: String::new(),
            cursor: 0,
            kill_ring: Vec::new(),
            last_action: LastAction::None,
            undo: Vec::new(),
            keymap: EditorKeymap::default(),
        }
    }

    /// A field pre-filled with `value`, caret at the end — the shape every `setValue` call site
    /// upstream wants (`model-selector.ts:119`, `session-selector.ts:721`).
    pub fn with_value(value: String) -> Self {
        let mut input = Self::new();
        input.cursor = value.len();
        input.value = value;
        input
    }

    /// The current text.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The caret's byte offset into [`Self::value`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// `setValue` (`input.ts:43-46`): replace the text and clamp the caret —
    /// `this.cursor = Math.min(this.cursor, value.length)`, then snapped down to a char boundary so
    /// the invariant survives a shorter replacement that lands mid-codepoint.
    pub fn set_value(&mut self, value: String) {
        self.value = value;
        self.cursor = self.snap(self.cursor.min(self.value.len()));
    }

    /// Empty the field and park the caret at 0 (the `Ctrl+C`-clears-the-query path in
    /// `scoped-models-selector.ts`).
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.last_action = LastAction::None;
    }

    /// Adopt the app's live editor bindings, so word motion / kill ring / undo answer to whatever
    /// the user has in `keybindings.json` (`getKeybindings()` on every key, `input.ts:86`).
    pub fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        self.keymap = keymap.clone();
    }

    /// Route one key — `handleInput` (`input.ts:86-210`) resolved through
    /// [`EditorKeymap::action_for`], **minus** the cancel/submit arms (`:89-104`): in cyrup the
    /// wrapping selector resolves [`SelectAction::Cancel`]/[`SelectAction::Confirm`] through its
    /// [`SelectKeymap`] *before* delegating here, so [`EditorAction::Submit`] must fall through as
    /// [`InputOutcome::Ignored`] or a dialog loses its Enter key.
    ///
    /// `handleInput` tests exactly the ids below and no others, so every remaining editor binding
    /// ([`EditorAction::CursorUp`]/`CursorDown`, `PageUp`/`PageDown`, `NewLine`, `Submit`, `Tab`,
    /// `JumpForward`/`JumpBackward`, `HistoryPrevious`/`HistoryNext`) is inert in a single-line
    /// field and reports `Ignored`, leaving the host free to bind it.
    pub fn handle_key(&mut self, key: &KeyEvent) -> InputOutcome {
        match self.keymap.action_for(key) {
            // `tui.editor.undo` (`:95-98`).
            Some(EditorAction::Undo) => {
                self.undo();
                InputOutcome::Edited
            }
            // Deletion (`:107-135`).
            Some(EditorAction::DeleteCharBackward) => {
                self.delete_char_backward();
                InputOutcome::Edited
            }
            Some(EditorAction::DeleteCharForward) => {
                self.delete_char_forward();
                InputOutcome::Edited
            }
            Some(EditorAction::DeleteWordBackward) => {
                self.delete_word_backward();
                InputOutcome::Edited
            }
            Some(EditorAction::DeleteWordForward) => {
                self.delete_word_forward();
                InputOutcome::Edited
            }
            Some(EditorAction::DeleteToLineStart) => {
                self.delete_to_line_start();
                InputOutcome::Edited
            }
            Some(EditorAction::DeleteToLineEnd) => {
                self.delete_to_line_end();
                InputOutcome::Edited
            }
            // Kill-ring actions (`:138-145`).
            Some(EditorAction::Yank) => {
                self.yank();
                InputOutcome::Edited
            }
            Some(EditorAction::YankPop) => {
                self.yank_pop();
                InputOutcome::Edited
            }
            // Cursor movement (`:148-190`). Each clears `lastAction`, which is what breaks a
            // kill run and disqualifies the next Alt+Y.
            Some(EditorAction::CursorLeft) => {
                self.last_action = LastAction::None;
                self.cursor = self.cursor.saturating_sub(self.prev_grapheme_len());
                InputOutcome::Moved
            }
            Some(EditorAction::CursorRight) => {
                self.last_action = LastAction::None;
                self.cursor =
                    self.cursor.saturating_add(self.next_grapheme_len()).min(self.value.len());
                InputOutcome::Moved
            }
            Some(EditorAction::CursorLineStart) => {
                self.last_action = LastAction::None;
                self.cursor = 0;
                InputOutcome::Moved
            }
            Some(EditorAction::CursorLineEnd) => {
                self.last_action = LastAction::None;
                self.cursor = self.value.len();
                InputOutcome::Moved
            }
            Some(EditorAction::CursorWordLeft) => {
                self.move_word_left();
                InputOutcome::Moved
            }
            Some(EditorAction::CursorWordRight) => {
                self.move_word_right();
                InputOutcome::Moved
            }
            Some(_) => InputOutcome::Ignored,
            // "Regular character input — accept printable characters including Unicode, but reject
            // control characters" (`:202-210`). crossterm has already decoded the byte stream, so
            // the C0/DEL/C1 scan is `char::is_control` plus the modifier test that distinguishes
            // `Ctrl+W` from `w`.
            None => match key.code {
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
                        && !c.is_control() =>
                {
                    self.insert_char(c);
                    InputOutcome::Edited
                }
                _ => InputOutcome::Ignored,
            },
        }
    }

    /// `handlePaste` (`input.ts:362-372`): one undo snapshot, then the text with `\r`/`\n` stripped
    /// and every tab expanded to four spaces, inserted at the caret.
    pub fn paste(&mut self, text: &str) {
        self.last_action = LastAction::None;
        self.push_undo();
        let mut clean = String::with_capacity(text.len());
        for c in text.chars() {
            match c {
                '\r' | '\n' => {}
                '\t' => clean.push_str("    "),
                _ => clean.push(c),
            }
        }
        if self.splice(self.cursor, self.cursor, &clean) {
            self.cursor = self.cursor.saturating_add(clean.len());
        }
    }

    // ---- string plumbing -------------------------------------------------------------------

    /// The largest char boundary `<= at` — the guard that keeps [`Self::cursor`]'s invariant true
    /// even when a host hands in an arbitrary offset.
    fn snap(&self, at: usize) -> usize {
        (0..=at.min(self.value.len())).rev().find(|i| self.value.is_char_boundary(*i)).unwrap_or(0)
    }

    /// Replace `value[start..end]` with `text`, rebuilt from two `str::get` slices. Returns `false`
    /// (leaving the buffer untouched) when either offset is not a char boundary — the no-panic
    /// stand-in for `String::replace_range`, which would abort the TUI instead.
    fn splice(&mut self, start: usize, end: usize, text: &str) -> bool {
        let (Some(head), Some(tail)) = (self.value.get(..start), self.value.get(end..)) else {
            return false;
        };
        let mut next = String::with_capacity(head.len() + text.len() + tail.len());
        next.push_str(head);
        next.push_str(text);
        next.push_str(tail);
        self.value = next;
        true
    }

    /// The byte length of the grapheme cluster ending at the caret (`input.ts:151-155`'s
    /// `lastGrapheme.segment.length`), `0` at the start of the field.
    fn prev_grapheme_len(&self) -> usize {
        self.value.get(..self.cursor).and_then(|s| s.graphemes(true).next_back()).map_or(0, str::len)
    }

    /// The byte length of the grapheme cluster starting at the caret (`input.ts:161-165`), `0` at
    /// the end of the field.
    fn next_grapheme_len(&self) -> usize {
        self.value.get(self.cursor..).and_then(|s| s.graphemes(true).next()).map_or(0, str::len)
    }

    /// `pushUndo` (`input.ts:338-340`), bounded exactly as the multi-line editor's stack is.
    fn push_undo(&mut self) {
        push_bounded(&mut self.undo, (self.value.clone(), self.cursor), UNDO_CAP);
    }

    /// `killRing.push` with the caller's direction and pi's `lastAction === "kill"` accumulate flag.
    fn push_kill(&mut self, text: &str, prepend: bool, accumulate: bool) {
        kill_ring_push(&mut self.kill_ring, text, prepend, accumulate);
    }

    // ---- editing ---------------------------------------------------------------------------

    /// `insertCharacter` (`input.ts:213-222`). The undo snapshot is taken only at a coalescing
    /// boundary (whitespace, or a non-typing previous action), and `cursor += char.length` is
    /// `len_utf8` here — this is the INSERT path, which steps by the inserted character, not by a
    /// grapheme cluster.
    fn insert_char(&mut self, c: char) {
        if should_snapshot_for_type(c, self.last_action == LastAction::TypeWord) {
            self.push_undo();
        }
        self.last_action = LastAction::TypeWord;
        let mut buf = [0u8; 4];
        let encoded = c.encode_utf8(&mut buf);
        let len = encoded.len();
        if self.splice(self.cursor, self.cursor, encoded) {
            self.cursor = self.cursor.saturating_add(len);
        }
    }

    /// `handleBackspace` (`input.ts:224-235`): one whole grapheme cluster back.
    fn delete_char_backward(&mut self) {
        self.last_action = LastAction::None;
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let start = self.cursor.saturating_sub(self.prev_grapheme_len());
        if self.splice(start, self.cursor, "") {
            self.cursor = start;
        }
    }

    /// `handleForwardDelete` (`input.ts:237-247`): one whole grapheme cluster forward, caret fixed.
    fn delete_char_forward(&mut self) {
        self.last_action = LastAction::None;
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let end = self.cursor.saturating_add(self.next_grapheme_len()).min(self.value.len());
        self.splice(self.cursor, end, "");
    }

    /// `deleteToLineStart` (`input.ts:249-257`) — Ctrl+U. Kills backward, so it PREPENDS when
    /// accumulating onto a previous kill.
    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted = self.value.get(..self.cursor).unwrap_or("").to_string();
        self.push_kill(&deleted, true, self.last_action == LastAction::Kill);
        self.last_action = LastAction::Kill;
        if self.splice(0, self.cursor, "") {
            self.cursor = 0;
        }
    }

    /// `deleteToLineEnd` (`input.ts:259-266`) — Ctrl+K. Kills forward, so it APPENDS.
    fn delete_to_line_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let deleted = self.value.get(self.cursor..).unwrap_or("").to_string();
        self.push_kill(&deleted, false, self.last_action == LastAction::Kill);
        self.last_action = LastAction::Kill;
        let end = self.value.len();
        self.splice(self.cursor, end, "");
    }

    /// `deleteWordBackwards` (`input.ts:268-287`) — Ctrl+W. `wasKill` is captured BEFORE the word
    /// move, because the move clears `lastAction` and would otherwise break the accumulate run
    /// (upstream's own comment at `:272`).
    fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let was_kill = self.last_action == LastAction::Kill;
        self.push_undo();
        let delete_from = self.word_left();
        let deleted = self.value.get(delete_from..self.cursor).unwrap_or("").to_string();
        self.push_kill(&deleted, true, was_kill);
        self.last_action = LastAction::Kill;
        if self.splice(delete_from, self.cursor, "") {
            self.cursor = delete_from;
        }
    }

    /// `deleteWordForward` (`input.ts:289-307`) — Alt+D. The forward twin, appending to the ring.
    fn delete_word_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let was_kill = self.last_action == LastAction::Kill;
        self.push_undo();
        let delete_to = self.word_right();
        let deleted = self.value.get(self.cursor..delete_to).unwrap_or("").to_string();
        self.push_kill(&deleted, false, was_kill);
        self.last_action = LastAction::Kill;
        self.splice(self.cursor, delete_to, "");
    }

    /// `yank` (`input.ts:309-318`) — Ctrl+Y: insert the ring top at the caret.
    fn yank(&mut self) {
        let Some(text) = self.kill_ring.last().cloned() else { return };
        if text.is_empty() {
            return;
        }
        self.push_undo();
        if self.splice(self.cursor, self.cursor, &text) {
            self.cursor = self.cursor.saturating_add(text.len());
        }
        self.last_action = LastAction::Yank;
    }

    /// `yankPop` (`input.ts:320-336`) — Alt+Y: only straight after a yank and with ≥2 entries.
    /// Deletes the just-yanked text (still the ring top before the rotation), rotates, re-inserts.
    ///
    /// The delete is `prevText.length` **bytes** back from the caret, pi's slice arithmetic in the
    /// crate's index unit; the `cursor >= prev.len()` guard covers the one way that can be wrong —
    /// a host calling [`Self::set_value`] between the yank and the yank-pop.
    fn yank_pop(&mut self) {
        if self.last_action != LastAction::Yank || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo();
        let prev = self.kill_ring.last().cloned().unwrap_or_default();
        if self.cursor >= prev.len() {
            let start = self.cursor.saturating_sub(prev.len());
            if self.splice(start, self.cursor, "") {
                self.cursor = start;
            }
        }
        kill_ring_rotate(&mut self.kill_ring);
        let text = self.kill_ring.last().cloned().unwrap_or_default();
        if self.splice(self.cursor, self.cursor, &text) {
            self.cursor = self.cursor.saturating_add(text.len());
        }
        self.last_action = LastAction::Yank;
    }

    /// `undo` (`input.ts:342-348`): pop one snapshot, restore BOTH value and caret, clear
    /// `lastAction`. No redo (pi parity).
    fn undo(&mut self) {
        let Some((value, cursor)) = self.undo.pop() else { return };
        self.value = value;
        self.cursor = self.snap(cursor.min(self.value.len()));
        self.last_action = LastAction::None;
    }

    // ---- word motion -----------------------------------------------------------------------

    /// `moveWordBackwards` (`input.ts:350-354`).
    fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = LastAction::None;
        self.cursor = self.word_left();
    }

    /// `moveWordForwards` (`input.ts:356-360`).
    fn move_word_right(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.last_action = LastAction::None;
        self.cursor = self.word_right();
    }

    /// `findWordBackward(this.value, this.cursor)` (`word-navigation.ts:22-68`) over BYTE offsets —
    /// the same walk the multi-line editor runs over char columns, with no atomic paste markers
    /// because a single-line field has none.
    fn word_left(&self) -> usize {
        if self.cursor == 0 {
            return 0;
        }
        let Some(before) = self.value.get(..self.cursor) else { return self.cursor };
        let segs = byte_word_segments(before);
        find_word_backward(
            segs,
            self.cursor,
            &|seg| byte_seg_is_whitespace(before, seg),
            &|seg| byte_seg_last_punct_end(before, seg),
        )
    }

    /// `findWordForward(this.value, this.cursor)` (`word-navigation.ts:76-114`).
    fn word_right(&self) -> usize {
        let len = self.value.len();
        if self.cursor >= len {
            return len;
        }
        let Some(after) = self.value.get(self.cursor..) else { return self.cursor };
        let segs = byte_word_segments(after);
        find_word_forward(
            &segs,
            self.cursor,
            &|seg| byte_seg_is_whitespace(after, seg),
            &|seg| byte_seg_first_punct(after, seg),
        )
    }
}

/// A single-line text-input selector: `title` is the dialog prompt shown above the field.
///
/// A `placeholder` still travels the `ui.input(title, placeholder, opts)` wire (rpc-types.ts:
/// 233-240) and is still accepted by [`Self::new`], but — exactly as upstream — it is never
/// rendered: `ExtensionInputComponent` binds it as `_placeholder` and never references it again
/// (`extension-input.ts:36`), and the `Input` it builds has no placeholder concept at all
/// (`input.ts:378-446`). See E8 in [`Selector::render`].
pub struct TextInputSelector {
    title: String,
    /// The field itself — pi's `new Input()` (`extension-input.ts:63`).
    input: Input,
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
        TextInputSelector { title, input: Input::new(), keymap: SelectKeymap::default() }
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
        self.input.value()
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
        frame.render_widget(
            Paragraph::new(Line::from(input_line_spans(
                self.input.value(),
                self.input.cursor(),
                body.width,
                theme,
            ))),
            body,
        );
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
        // Submit/cancel first — pi's `Input` owns those arms itself (`input.ts:89-104`), but in
        // cyrup the selector resolves them and `Input::handle_key` deliberately ignores
        // `EditorAction::Submit`. Everything else is the shared editing surface.
        match key.code {
            KeyCode::Enter => SelectorOutcome::Confirm(self.input.value().to_string()),
            KeyCode::Esc => SelectorOutcome::Cancel,
            _ => match self.input.handle_key(key) {
                InputOutcome::Ignored => SelectorOutcome::Ignored,
                _ => SelectorOutcome::Redraw,
            },
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        self.input.set_editor_keymap(keymap);
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        self.input.paste(text);
        SelectorOutcome::Redraw
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
