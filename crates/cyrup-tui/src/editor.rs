//! The multi-line input editor (spec/tui/03; arch-10 §3.6; gaps 23-31, 36).
//!
//! A char-indexed `Vec<Vec<char>>` buffer (one inner vec per logical line) so all cursor math is on
//! `char` indices and never byte-indexes UTF-8 (no-panic policy, R-00-009). Beyond insert/motion the
//! editor ports Pi's editor surface (`pi-tui/src/components/editor.ts`): word navigation, the kill
//! ring (Ctrl+W/U/K, Alt+D feed it; Ctrl+Y yanks; Alt+Y yank-pops), an undo stack (Ctrl+-) with
//! typing coalescing, prompt history recall (Up/Down at the buffer edges, 100-entry, draft save),
//! char-jump (Ctrl+]), bash-mode detection (leading `!`/`!!`), and the slash/path autocomplete popup
//! (`autocomplete.rs`). Keys resolve through [`EditorKeymap`] — the editor never compares keys inline
//! (R-10-018). Horizontal motion + backspace/forward-delete step over whole **grapheme clusters**
//! (emoji, ZWJ sequences, combining marks) via [`unicode_segmentation`], matching Pi's editor, which
//! treats a user-perceived character as one cursor unit (`pi-tui/src/components/editor.ts`). Vertical
//! Up/Down motion is **wrap-aware**: logical lines are wrapped into a visual-line map
//! ([`build_visual_line_map`]) and the cursor moves by *visual* line, preserving a **sticky preferred
//! column** ([`InputEditor::preferred_visual_col`]) across short/long/rewrapped lines, falling through
//! to history at the first visual line and to line-end at the last (spec/tui/03 §4.1-§4.2). Large
//! pastes collapse to atomic `[paste #N …]` markers ([`InputEditor::handle_paste`]) that expand back
//! to content on submit ([`InputEditor::expanded_text`], spec/tui/03 §5.5).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::autocomplete::{Autocomplete, CompletionContext};
use crate::commands::CommandRegistry;
use crate::component::Component;
use crate::keymap::{EditorAction, EditorKeymap};
use crate::theme::UiTheme;

/// The accent prompt glyph drawn at the head of the editor's first line (overview §1.1 glyph
/// vocabulary `prompt ›`; ADR-0001 live-region mockup). Two columns wide (`› `).
const PROMPT: &str = "› ";
/// Visible column width of [`PROMPT`].
const PROMPT_W: u16 = 2;

/// History ring capacity (`editor.ts:381`).
const HISTORY_CAP: usize = 100;

/// Outcome of feeding a key to the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    /// Enter pressed (or a slash-command accepted from the popup): the submitted text (buffer cleared).
    Submit(String),
    /// The buffer or cursor changed → request a re-render.
    Edited,
    /// The key was not handled by the editor.
    Ignored,
}

/// What the previous mutating op was, gating kill-ring accumulation and undo coalescing
/// (`editor.ts` `lastAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    None,
    Kill,
    Yank,
    Type,
}

/// Direction for char-jump mode (`editor.ts:307`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpDir {
    Forward,
    Backward,
}

/// An undo snapshot (deep clone of buffer + cursor, `undo-stack.ts`).
#[derive(Clone, Debug)]
struct Snapshot {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

/// A multi-line text editor with a block cursor, kill ring, undo, history, and autocomplete.
pub struct InputEditor {
    /// Lines as char vectors. Invariant: always at least one line.
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    focused: bool,
    keymap: EditorKeymap,
    kill_ring: Vec<String>,
    last_action: LastAction,
    undo: Vec<Snapshot>,
    history: VecDeque<String>,
    /// `-1` ⇒ not browsing history; otherwise an index into `history`.
    history_index: isize,
    history_draft: Option<Snapshot>,
    jump: Option<JumpDir>,
    registry: CommandRegistry,
    autocomplete: Option<Autocomplete>,
    /// Max visible rows in the autocomplete dropdown (Pi `autocompleteMaxVisible`, default 5, clamped
    /// 3–20; item #6). Plumbed from `settings.autocompleteMaxVisible` by the binary; applied to every
    /// opened popup's [`crate::SelectList`].
    autocomplete_max_visible: u16,
    /// The configurable autocomplete-popup key table (item #6; `tui.autocomplete.*`). The popup's
    /// navigate/accept/cancel keys are no longer hardcoded — a `keybindings.json` rebind flows through.
    autocomplete_keymap: crate::keymap::AutocompleteKeymap,
    cwd: PathBuf,
    /// Cached whole-tree file list for `@`-mention search (`autocomplete.ts` populates once, then
    /// fuzzy-filters in-process per keystroke). Lazily built on the first `@`-mention, invalidated on
    /// `set_cwd`. `None` until first needed.
    mention_files: Option<Vec<String>>,
    /// The layout width (in columns) used to wrap logical lines into **visual** lines for vertical
    /// motion (`editor.ts:1690` `build_visual_line_map(width)`). Updated every render; `80` until the
    /// first render. Vertical Up/Down resolve against the visual map computed at this width.
    view_width: usize,
    /// The **sticky preferred column** for vertical motion (`editor.ts:66`
    /// `preferred_visual_col`): the intended visual column Up/Down try to land on across short/long/
    /// rewrapped visual lines. `Some` while a vertical run is in progress; cleared by any horizontal
    /// motion, edit, or paste so the next vertical move re-seeds from the live cursor (spec/tui/03 §4.2).
    preferred_visual_col: Option<usize>,
    /// Large-paste store (`editor.ts:81` `pastes: id -> expanded content`): each entry is the full
    /// pasted text the buffer shows collapsed to a `[paste #N …]` marker. [`expanded_text`] substitutes
    /// markers back to content on submit (`expandPasteMarkers`, spec/tui/03 §5.5).
    pastes: BTreeMap<u32, String>,
    /// Monotonic id for the next large paste (`editor.ts:82` `paste_counter`).
    paste_counter: u32,
    /// The current reasoning level (`off`/`minimal`/`low`/`medium`/`high`/`xhigh`) — the editor's
    /// top/bottom rule color is the primary always-visible thinking-level signal
    /// (`interactive-mode.ts:3533-3541`, spec/tui/03 §3.3). Recolored green in bash mode. Updated by
    /// the app on `ThinkingLevelChanged`; `"medium"` until set.
    thinking_level: String,
}

/// One **visual** line of the wrapped editor: a contiguous slice of a logical line that fits the
/// layout width (`editor.ts:1690-1715` `VisualLine { logical_line, start_col, length }`). An empty
/// logical line yields exactly one zero-length visual line so the cursor still has a row to sit on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualLine {
    /// The logical line (`lines` index) this visual line is a slice of.
    pub logical: usize,
    /// The char column in the logical line where this visual line begins.
    pub start: usize,
    /// The number of chars on this visual line (0 for an empty logical line / trailing wrap).
    pub len: usize,
}

impl Default for InputEditor {
    fn default() -> Self {
        InputEditor::new()
    }
}

impl InputEditor {
    /// A fresh, empty, focused editor.
    pub fn new() -> Self {
        InputEditor {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            focused: true,
            keymap: EditorKeymap::default(),
            kill_ring: Vec::new(),
            last_action: LastAction::None,
            undo: Vec::new(),
            history: VecDeque::new(),
            history_index: -1,
            history_draft: None,
            jump: None,
            registry: CommandRegistry::new(),
            autocomplete: None,
            autocomplete_max_visible: crate::select_list::DEFAULT_MAX_VISIBLE,
            autocomplete_keymap: crate::keymap::AutocompleteKeymap::default(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            mention_files: None,
            view_width: 80,
            preferred_visual_col: None,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            thinking_level: "medium".to_string(),
        }
    }

    /// Set the reasoning level driving the editor's rule color (spec/tui/03 §3.3). Called by the app
    /// on `ThinkingLevelChanged` / thinking-selector confirm.
    pub fn set_thinking_level(&mut self, level: impl Into<String>) {
        self.thinking_level = level.into();
    }

    /// Replace the command registry used for slash autocomplete (rebuilt on `/reload`).
    pub fn set_registry(&mut self, registry: CommandRegistry) {
        self.registry = registry;
    }

    /// The editor keymap (for `/hotkeys` to resolve editor-action key labels, `getEditorKeyDisplay`).
    pub fn keymap_ref(&self) -> &EditorKeymap {
        &self.keymap
    }

    /// Merge a JSON keybindings document into the editor keymap **and** the autocomplete-popup keymap
    /// (R-10-018; the `editor.*` + `tui.autocomplete.*` ids). Called by the binary at boot with the
    /// user's `keybindings.json` so custom editor + popup bindings take effect (item #6).
    pub fn merge_keybindings_json(&mut self, json: &str) -> Result<(), crate::TuiError> {
        self.keymap.merge_json(json)?;
        self.autocomplete_keymap.merge_json(json)
    }

    /// Set the max visible rows in the autocomplete dropdown (Pi `autocompleteMaxVisible`, item #6):
    /// clamped to 3–20 and applied to any already-open popup + every future one. Called by the binary
    /// from `settings.autocompleteMaxVisible`.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.autocomplete_max_visible = n.clamp(3, 20);
        if let Some(ac) = self.autocomplete.as_mut() {
            ac.list.set_max_visible(self.autocomplete_max_visible);
        }
    }

    /// Install a freshly-computed autocomplete popup, applying the configured `autocompleteMaxVisible`
    /// dropdown height (item #6). Centralises the `self.autocomplete = Some(ac)` assignment so every
    /// entry point (slash / `@`-mention / forced Tab) honours the setting.
    fn open_popup(&mut self, mut ac: Autocomplete) {
        ac.list.set_max_visible(self.autocomplete_max_visible);
        self.autocomplete = Some(ac);
    }

    /// Override the working directory used for path completion (defaults to the process cwd). Clears
    /// the cached `@`-mention file list so the new tree is re-enumerated on the next mention.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
        self.mention_files = None;
    }

    /// Inject the `@`-mention candidate file list directly (test seam / an async populator). Bypasses
    /// the lazy `fd`/walk source.
    pub fn set_mention_files(&mut self, files: Vec<String>) {
        self.mention_files = Some(files);
    }

    /// Focus state (R-10-009 — focused inputs drive the hardware cursor).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Whether an autocomplete popup is currently open.
    pub fn autocomplete_open(&self) -> bool {
        self.autocomplete.is_some()
    }

    /// The active autocomplete popup, if any (for the chrome to render it below the editor).
    pub fn autocomplete(&self) -> Option<&Autocomplete> {
        self.autocomplete.as_ref()
    }

    /// True while the buffer's first line — **after leading whitespace** — begins with `!` (bash mode
    /// → green border, spec/tui/03 §7.1). Mirrors Pi's `text.trimStart().startsWith("!")`
    /// (interactive-mode.ts:2525): a leading indent before `!` (e.g. `  !ls`) still enters bash mode,
    /// matching the dispatcher, which `trim()`s before the `!` check (item #5 "trim_start on bash").
    pub fn is_bash_mode(&self) -> bool {
        self.lines
            .first()
            .and_then(|l| l.iter().find(|c| !c.is_whitespace()))
            .is_some_and(|c| *c == '!')
    }

    /// The full buffer text with `\n` line joins.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    /// Whether the buffer is empty (no chars on any line).
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }

    /// Number of buffer lines (≥ 1) — used to size the editor area.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The cursor position as `(row, col)` in chars.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// The prompt history (most-recent first), for inspection/tests.
    pub fn history(&self) -> &VecDeque<String> {
        &self.history
    }

    /// The kill-ring top (most recent killed text), for inspection/tests.
    pub fn kill_ring_top(&self) -> Option<&str> {
        self.kill_ring.last().map(String::as_str)
    }

    /// Replace the buffer contents and move the cursor to the end.
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines.get(self.row).map_or(0, Vec::len);
    }

    /// Clear the buffer back to a single empty line and reset transient state.
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.autocomplete = None;
        self.jump = None;
        self.last_action = LastAction::None;
        self.preferred_visual_col = None;
        self.pastes.clear();
        self.exit_history();
    }

    /// The char length of the current line (0 if somehow out of range).
    fn cur_len(&self) -> usize {
        self.lines.get(self.row).map_or(0, Vec::len)
    }

    // ---- visual-line map (wrap-aware vertical motion, spec/tui/03 §4) -----------------------

    /// Record the layout width used to wrap lines for vertical motion (set every render; also a test
    /// seam). `0` is clamped to `1` so wrapping never divides by zero.
    pub fn set_view_width(&mut self, width: usize) {
        self.view_width = width.max(1);
    }

    /// Build the wrap-aware visual-line map at the current [`view_width`](Self::view_width)
    /// (`editor.ts:1690` `build_visual_line_map`). Each logical line expands into one or more
    /// [`VisualLine`]s via word-aware wrapping; the result is in reading order and always non-empty
    /// (at least one zero-length visual line for the single empty buffer line).
    pub fn visual_line_map(&self) -> Vec<VisualLine> {
        let width = self.view_width.max(1);
        let mut map = Vec::with_capacity(self.lines.len());
        for (logical, line) in self.lines.iter().enumerate() {
            for (start, len) in word_wrap_line(line, width) {
                map.push(VisualLine { logical, start, len });
            }
        }
        if map.is_empty() {
            map.push(VisualLine { logical: 0, start: 0, len: 0 });
        }
        map
    }

    /// The number of **visual** (wrapped) lines the buffer occupies at `width` columns — the total
    /// [`VisualLine`] count of the wrap map built at an arbitrary width (`editor.ts:1690`
    /// `build_visual_line_map(width).length`, the same primitive vertical motion uses). The app sizes
    /// the editor slot from this so a long/pasted logical line grows the box one row per wrapped
    /// visual line instead of clipping. Independent of [`view_width`](Self::view_width) so height
    /// measurement and render agree when passed the same width. Always `>= 1`.
    pub fn visual_line_count(&self, width: usize) -> usize {
        let width = width.max(1);
        let count: usize = self.lines.iter().map(|line| word_wrap_line(line, width).len()).sum();
        count.max(1)
    }

    /// Map the cursor `(row, col)` to its index in `map` (`editor.ts:1742` `find_current_visual_line`):
    /// the visual line of the cursor's logical row that contains `col`; when `col` sits exactly on a
    /// wrap boundary it belongs to the *start* of the following visual line, and an end-of-line cursor
    /// rides the last visual line of the row.
    fn current_visual_line(&self, map: &[VisualLine]) -> usize {
        let mut fallback = 0;
        for (i, vl) in map.iter().enumerate() {
            if vl.logical != self.row {
                continue;
            }
            fallback = i;
            if self.col >= vl.start && self.col < vl.start + vl.len {
                return i;
            }
        }
        fallback
    }

    /// Vertical Up by one **visual** line, preserving the sticky preferred column (spec/tui/03 §4.2).
    /// At the first visual line the cursor falls through to line-start (history is handled by the
    /// caller before this runs).
    fn move_up_visual(&mut self) {
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        let here = map.get(cur).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let goal = self.preferred_visual_col.unwrap_or(self.col.saturating_sub(here.start));
        self.preferred_visual_col = Some(goal);
        if cur == 0 {
            // First visual line: fall through to line-start (spec/tui/03 §5.1).
            self.col = 0;
            return;
        }
        if let Some(target) = map.get(cur - 1) {
            self.row = target.logical;
            self.col = target.start + goal.min(target.len);
        }
    }

    /// Vertical Down by one **visual** line, preserving the sticky preferred column (spec/tui/03 §4.2).
    /// At the last visual line the cursor falls through to line-end (history is handled by the caller).
    fn move_down_visual(&mut self) {
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        let here = map.get(cur).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let goal = self.preferred_visual_col.unwrap_or(self.col.saturating_sub(here.start));
        self.preferred_visual_col = Some(goal);
        if cur + 1 >= map.len() {
            // Last visual line: fall through to line-end (spec/tui/03 §5.1).
            self.col = self.cur_len();
            return;
        }
        if let Some(target) = map.get(cur + 1) {
            self.row = target.logical;
            self.col = target.start + goal.min(target.len);
        }
    }

    /// Drop the sticky vertical-motion column (called by every non-vertical motion/edit so the next
    /// Up/Down re-seeds the goal column from the live cursor).
    fn reset_preferred_col(&mut self) {
        self.preferred_visual_col = None;
    }

    // ---- large-paste markers (spec/tui/03 §5.5) --------------------------------------------

    /// Handle a (bracketed) paste (`editor.ts:615` `handlePaste`): sanitize, then either collapse a
    /// **large** paste (`> 10` lines or `> 1000` chars) to an atomic `[paste #N …]` marker stored in
    /// [`pastes`](Self::pastes), or insert a small paste verbatim. The marker keeps the buffer compact;
    /// [`expanded_text`](Self::expanded_text) restores the full content on submit.
    pub fn handle_paste(&mut self, raw: &str) {
        let text = sanitize_paste(raw);
        let line_count = text.split('\n').count();
        let char_count = text.chars().count();
        if line_count > 10 || char_count > 1000 {
            self.paste_counter += 1;
            let id = self.paste_counter;
            let marker = if line_count > 1 {
                format!("[paste #{id} +{line_count} lines]")
            } else {
                format!("[paste #{id} {char_count} chars]")
            };
            self.pastes.insert(id, text);
            self.push_undo_for(LastAction::None);
            for c in marker.chars() {
                self.insert_char(c);
            }
            self.last_action = LastAction::None;
            self.reset_preferred_col();
            self.exit_history();
            self.update_autocomplete();
        } else {
            self.insert_str(&text);
        }
    }

    /// The buffer text with every `[paste #N …]` marker expanded back to its stored content
    /// (`expandPasteMarkers`, `editor.ts`). Submission uses this so the model receives the full paste.
    pub fn expanded_text(&self) -> String {
        let text = self.text();
        if self.pastes.is_empty() {
            return text;
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            if let Some((_, content, end)) = self.marker_at(&chars, i) {
                out.push_str(content);
                i = end;
            } else if let Some(c) = chars.get(i) {
                out.push(*c);
                i += 1;
            } else {
                break;
            }
        }
        out
    }

    /// If a `[paste #N …]` marker for a known id starts at `chars[i]`, return its `(id, content, end)`
    /// where `end` is the char index just past the closing `]`. Bounds-checked throughout (no-panic).
    fn marker_at<'a>(&'a self, chars: &[char], i: usize) -> Option<(u32, &'a str, usize)> {
        const PREFIX: [char; 8] = ['[', 'p', 'a', 's', 't', 'e', ' ', '#'];
        for (k, pc) in PREFIX.iter().enumerate() {
            if chars.get(i + k) != Some(pc) {
                return None;
            }
        }
        let mut j = i + PREFIX.len();
        let mut id: u32 = 0;
        let mut digits = 0;
        while let Some(&c) = chars.get(j).filter(|c| c.is_ascii_digit()) {
            id = id.saturating_mul(10).saturating_add(c.to_digit(10).unwrap_or(0));
            j += 1;
            digits += 1;
        }
        if digits == 0 {
            return None;
        }
        // Scan to the closing `]` on the same marker (a stray `[` aborts — no nested marker).
        while let Some(&c) = chars.get(j) {
            if c == ']' || c == '[' {
                break;
            }
            j += 1;
        }
        if chars.get(j) != Some(&']') {
            return None;
        }
        let content = self.pastes.get(&id)?;
        Some((id, content.as_str(), j + 1))
    }

    /// If `col` falls inside (or on either edge of) a complete `[paste #N …]` marker on the current
    /// line, return its `(start_col, end_col, paste_id)` so deletion removes it atomically
    /// (`segmentWithMarkers`, spec/tui/03 §5.5).
    fn marker_covering(&self, col: usize) -> Option<(usize, usize, u32)> {
        let line = self.lines.get(self.row)?;
        let mut i = 0;
        while i < line.len() {
            if let Some((id, _, end)) = self.marker_at(line, i) {
                if col >= i && col <= end {
                    return Some((i, end, id));
                }
                i = end;
            } else {
                i += 1;
            }
        }
        None
    }

    /// Snapshot the buffer + cursor for undo.
    fn snapshot(&self) -> Snapshot {
        Snapshot { lines: self.lines.clone(), row: self.row, col: self.col }
    }

    /// Push an undo snapshot, coalescing consecutive typing into one unit (fish-style,
    /// `editor.ts:1082-1095`): a run of `Type` actions shares a single snapshot.
    fn push_undo_for(&mut self, action: LastAction) {
        let coalesce = action == LastAction::Type && self.last_action == LastAction::Type;
        if !coalesce {
            self.undo.push(self.snapshot());
            // Bound the stack so a long session does not grow unbounded.
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    /// Push an undo snapshot for a typed character, honoring Pi's fish-style **whitespace boundary**
    /// (`editor.ts:1085-1094`, item #5 "undo-whitespace coalescing"): consecutive word characters
    /// coalesce into one unit, but a whitespace character *always* captures the state before itself
    /// (`isWhitespaceChar(char) || lastAction !== "type-word"`), so a single `Ctrl+-` removes the last
    /// word without swallowing the whole line, and each space is a separate undo step.
    fn push_undo_for_type(&mut self, c: char) {
        if c.is_whitespace() || self.last_action != LastAction::Type {
            self.undo.push(self.snapshot());
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    /// Restore the most recent undo snapshot (Ctrl+-). No redo (Pi parity, `editor.ts:1974-1984`).
    fn undo(&mut self) {
        if let Some(snap) = self.undo.pop() {
            self.lines = snap.lines;
            self.row = snap.row.min(self.lines.len().saturating_sub(1));
            self.col = self.col.min(self.cur_len());
            self.exit_history();
        }
    }

    // ---- insertion -------------------------------------------------------------------------

    /// Insert a printable character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let col = self.col.min(self.cur_len());
        if let Some(line) = self.lines.get_mut(self.row) {
            line.insert(col, c);
            self.col = col + 1;
        }
        self.preferred_visual_col = None;
    }

    /// Insert a string (e.g. a paste) char by char.
    pub fn insert_str(&mut self, s: &str) {
        self.push_undo_for(LastAction::None);
        for c in s.chars() {
            if c == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(c);
            }
        }
        self.last_action = LastAction::None;
        self.exit_history();
        self.update_autocomplete();
    }

    /// Split the current line at the cursor (newline).
    pub fn insert_newline(&mut self) {
        let col = self.col.min(self.cur_len());
        let tail = match self.lines.get_mut(self.row) {
            Some(line) => line.split_off(col),
            None => Vec::new(),
        };
        let next = self.row + 1;
        self.lines.insert(next, tail);
        self.row = next;
        self.col = 0;
    }

    // ---- deletion --------------------------------------------------------------------------

    /// Backspace: delete the whole grapheme cluster before the cursor (emoji/ZWJ/combining marks
    /// removed as one unit, `editor.ts`), joining lines at column 0.
    pub fn backspace(&mut self) {
        // A large-paste marker is an atomic segment: backspacing anywhere across it (or just after its
        // closing `]`) removes the whole marker and drops its stored content (spec/tui/03 §5.5).
        // Backspace deletes the marker only when the cursor is *inside* or just-after it
        // (`col > start`); at `col == start` it falls through to delete the preceding char.
        if self.col > 0
            && let Some((s, e, id)) =
                self.marker_covering(self.col).filter(|&(s, _, _)| self.col > s)
        {
            if let Some(line) = self.lines.get_mut(self.row) {
                line.drain(s..e.min(line.len()));
            }
            self.pastes.remove(&id);
            self.col = s;
            return;
        }
        if self.col > 0 {
            let start = self.prev_grapheme(self.col);
            if let Some(line) = self.lines.get_mut(self.row) {
                let end = self.col.min(line.len());
                if start < end {
                    line.drain(start..end);
                }
            }
            self.col = start;
        } else if self.row > 0 && self.row < self.lines.len() {
            let cur = self.lines.remove(self.row);
            let prev_row = self.row - 1;
            if let Some(prev) = self.lines.get_mut(prev_row) {
                let join = prev.len();
                prev.extend(cur);
                self.row = prev_row;
                self.col = join;
            }
        }
    }

    /// Forward-delete: delete the whole grapheme cluster at the cursor (one user-perceived char),
    /// joining the next line at end-of-line.
    pub fn delete(&mut self) {
        let len = self.cur_len();
        // Forward-delete removes a whole marker when the cursor is inside or just-before it
        // (`col < end`); at `col == end` it falls through to delete the following char.
        if self.col < len
            && let Some((s, e, id)) =
                self.marker_covering(self.col).filter(|&(_, e, _)| self.col < e)
        {
            if let Some(line) = self.lines.get_mut(self.row) {
                line.drain(s..e.min(line.len()));
            }
            self.pastes.remove(&id);
            self.col = s;
            return;
        }
        if self.col < len {
            let end = self.next_grapheme(self.col);
            if let Some(line) = self.lines.get_mut(self.row) {
                let end = end.min(line.len());
                if self.col < end {
                    line.drain(self.col..end);
                }
            }
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            if let Some(line) = self.lines.get_mut(self.row) {
                line.extend(next);
            }
        }
    }

    /// Delete from the word-left boundary to the cursor, feeding the kill ring (Ctrl+W,
    /// `editor.ts:1479`). Coalesces with a preceding kill (prepend).
    fn delete_word_backward(&mut self) {
        let target = self.word_left_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range(target, (self.row, self.col));
        self.push_kill(&killed, false);
        self.row = target.0;
        self.col = target.1;
    }

    /// Delete from the cursor to the word-right boundary, feeding the kill ring (Alt+D).
    fn delete_word_forward(&mut self) {
        let target = self.word_right_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range((self.row, self.col), target);
        self.push_kill(&killed, true);
    }

    /// Delete from line start to the cursor, feeding the kill ring (Ctrl+U).
    fn delete_to_line_start(&mut self) {
        if self.col == 0 {
            return;
        }
        let killed = self.take_range((self.row, 0), (self.row, self.col));
        self.push_kill(&killed, false);
        self.col = 0;
    }

    /// Delete from the cursor to line end, feeding the kill ring (Ctrl+K).
    fn delete_to_line_end(&mut self) {
        let len = self.cur_len();
        if self.col >= len {
            return;
        }
        let killed = self.take_range((self.row, self.col), (self.row, len));
        self.push_kill(&killed, true);
    }

    /// Remove and return the text between two positions, **crossing logical lines** (item #5: cross-
    /// line word/char ops). A same-line range drains within the row; a multi-line range removes the
    /// tail of `start.0`, every whole line strictly between, and the head of `end.0`, then joins the
    /// two boundary rows — so a `Ctrl+W`/`Alt+D` (or `Backspace`/`Delete`) at a line edge deletes into
    /// the neighbouring line and re-joins it (`editor.ts` word/char deletion; `word-navigation.ts`
    /// returns cross-line targets). The removed text carries the `\n`s so it yanks back verbatim.
    fn take_range(&mut self, start: (usize, usize), end: (usize, usize)) -> String {
        // Normalize so `start <= end`.
        let (start, end) =
            if (start.0, start.1) <= (end.0, end.1) { (start, end) } else { (end, start) };
        if start == end {
            return String::new();
        }
        // Same-line: drain within the row.
        if start.0 == end.0 {
            let Some(line) = self.lines.get_mut(start.0) else { return String::new() };
            let lo = start.1.min(line.len());
            let hi = end.1.min(line.len());
            if lo >= hi {
                return String::new();
            }
            return line.drain(lo..hi).collect();
        }
        // Multi-line: guard the boundary rows.
        if start.0 >= self.lines.len() || end.0 >= self.lines.len() {
            return String::new();
        }
        let start_col = start.1.min(self.lines.get(start.0).map_or(0, Vec::len));
        let end_col = end.1.min(self.lines.get(end.0).map_or(0, Vec::len));

        // Collect the removed text (start tail + whole inner rows + end head), `\n`-joined, so it
        // yanks back verbatim.
        let mut killed = String::new();
        if let Some(first) = self.lines.get(start.0) {
            killed.extend(first.iter().skip(start_col));
        }
        for r in (start.0 + 1)..end.0 {
            killed.push('\n');
            if let Some(row) = self.lines.get(r) {
                killed.extend(row.iter());
            }
        }
        killed.push('\n');
        if let Some(last) = self.lines.get(end.0) {
            killed.extend(last.iter().take(end_col));
        }

        // Splice: keep the head of `start.0`, append the tail of `end.0`, drop the rows between.
        let tail: Vec<char> = self
            .lines
            .get(end.0)
            .map(|l| l.iter().skip(end_col).copied().collect())
            .unwrap_or_default();
        if let Some(first) = self.lines.get_mut(start.0) {
            first.truncate(start_col);
            first.extend(tail);
        }
        self.lines.drain((start.0 + 1)..=end.0);
        killed
    }

    /// Push killed text onto the ring, accumulating into the top entry when the previous action was
    /// also a kill (prepend for backward kills, append for forward kills; `kill-ring.ts`).
    fn push_kill(&mut self, text: &str, append: bool) {
        if self.last_action == LastAction::Kill
            && let Some(top) = self.kill_ring.last_mut() {
                if append {
                    top.push_str(text);
                } else {
                    *top = format!("{text}{top}");
                }
                return;
            }
        self.kill_ring.push(text.to_string());
    }

    /// Yank the kill-ring top at the cursor (Ctrl+Y, `editor.ts:1852`).
    fn yank(&mut self) {
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                if c == '\n' {
                    self.insert_newline();
                } else {
                    self.insert_char(c);
                }
            }
        }
    }

    /// Yank-pop: only after a yank with ≥2 ring entries — delete the just-yanked text, rotate the
    /// ring, and insert the new top (Alt+Y, `editor.ts:1867`).
    fn yank_pop(&mut self) {
        if self.last_action != LastAction::Yank || self.kill_ring.len() < 2 {
            return;
        }
        // Delete the previously-yanked text (the current ring top) backward from the cursor.
        if let Some(prev) = self.kill_ring.last().cloned() {
            let n = prev.chars().count();
            for _ in 0..n {
                self.backspace();
            }
        }
        // Rotate: move the top to the front (so a fresh top becomes current).
        if let Some(top) = self.kill_ring.pop() {
            self.kill_ring.insert(0, top);
        }
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                self.insert_char(c);
            }
        }
    }

    // ---- motion ----------------------------------------------------------------------------

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col = self.prev_grapheme(self.col);
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.cur_len();
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.cur_len() {
            self.col = self.next_grapheme(self.col);
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// The previous grapheme-cluster boundary strictly left of char-column `col` on the current line
    /// (emoji/ZWJ/combining marks step as one unit; `editor.ts` grapheme motion). `0` if none.
    fn prev_grapheme(&self, col: usize) -> usize {
        let Some(line) = self.lines.get(self.row) else { return col.saturating_sub(1) };
        grapheme_boundaries(line).into_iter().rfind(|&b| b < col).unwrap_or(0)
    }

    /// The next grapheme-cluster boundary strictly right of char-column `col` on the current line.
    /// Clamps to the line length when `col` is already at/after the last cluster.
    fn next_grapheme(&self, col: usize) -> usize {
        let Some(line) = self.lines.get(self.row) else { return col + 1 };
        let len = line.len();
        grapheme_boundaries(line).into_iter().find(|&b| b > col).unwrap_or(len)
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.cur_len();
    }

    /// The word-left target `(row, col)` (`word-navigation.ts`): skip a whitespace run, then consume
    /// one word/punctuation segment; honor punctuation sub-boundaries inside a word. At col 0 step to
    /// the previous line's end.
    fn word_left_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let mut i = self.col;
        if i == 0 {
            if self.row > 0 {
                let prev_len = self.lines.get(self.row - 1).map_or(0, Vec::len);
                return (self.row - 1, prev_len);
            }
            return (self.row, 0);
        }
        // Skip whitespace immediately left of the cursor.
        while i > 0 && line.get(i - 1).is_some_and(|c| c.is_whitespace()) {
            i -= 1;
        }
        // Consume one class run (word chars OR punctuation chars).
        if let Some(&c) = line.get(i.wrapping_sub(1)) {
            let want_word = is_word_char(c);
            while i > 0 {
                match line.get(i - 1) {
                    Some(&c) if is_word_char(c) == want_word && !c.is_whitespace() => i -= 1,
                    _ => break,
                }
            }
        }
        (self.row, i)
    }

    /// The word-right target (mirror of [`word_left_target`](Self::word_left_target)).
    fn word_right_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let len = line.len();
        let mut i = self.col;
        if i >= len {
            if self.row + 1 < self.lines.len() {
                return (self.row + 1, 0);
            }
            return (self.row, len);
        }
        while i < len && line.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if let Some(&c) = line.get(i) {
            let want_word = is_word_char(c);
            while i < len {
                match line.get(i) {
                    Some(&c) if is_word_char(c) == want_word && !c.is_whitespace() => i += 1,
                    _ => break,
                }
            }
        }
        (self.row, i)
    }

    fn move_word_left(&mut self) {
        let (r, c) = self.word_left_target();
        self.row = r;
        self.col = c;
    }

    fn move_word_right(&mut self) {
        let (r, c) = self.word_right_target();
        self.row = r;
        self.col = c;
    }

    // ---- char-jump -------------------------------------------------------------------------

    /// Jump the cursor to the next/previous occurrence of `target` on the current line, skipping the
    /// current position (case-sensitive, `editor.ts:1990-2018`).
    fn jump_to(&mut self, dir: JumpDir, target: char) {
        let Some(line) = self.lines.get(self.row) else { return };
        match dir {
            JumpDir::Forward => {
                if let Some(off) = line.iter().enumerate().skip(self.col + 1).find_map(|(i, &c)| {
                    (c == target).then_some(i)
                }) {
                    self.col = off;
                }
            }
            JumpDir::Backward => {
                if let Some(off) = (0..self.col)
                    .rev()
                    .find(|&i| line.get(i) == Some(&target))
                {
                    self.col = off;
                }
            }
        }
    }

    // ---- history ---------------------------------------------------------------------------

    /// Add a raw submitted line to history (skip blank + consecutive-dup, `editor.ts:381-391`).
    fn add_to_history(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.history.front().map(String::as_str) == Some(text) {
            return;
        }
        self.history.push_front(text.to_string());
        while self.history.len() > HISTORY_CAP {
            self.history.pop_back();
        }
    }

    /// Whether Up should enter / continue history browsing (`editor.ts:809-822`): at row 0 and the
    /// buffer is empty, already browsing, or the cursor is at col 0.
    fn history_up_eligible(&self) -> bool {
        self.row == 0 && (self.is_empty() || self.history_index >= 0 || self.col == 0)
    }

    /// Older history entry (Up). On first entry, snapshot the draft.
    fn history_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index < 0 {
            self.history_draft = Some(self.snapshot());
        }
        let next = (self.history_index + 1).min(self.history.len() as isize - 1);
        self.history_index = next;
        if let Some(entry) = self.history.get(next as usize).cloned() {
            self.set_text(&entry);
            self.row = 0;
            self.col = 0; // cursor at start on Up (setTextInternal placement)
        }
    }

    /// Newer history entry (Down). Past the newest restores the saved draft.
    fn history_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        self.history_index -= 1;
        if self.history_index < 0 {
            // Restore the draft.
            if let Some(draft) = self.history_draft.take() {
                self.lines = draft.lines;
                self.row = draft.row.min(self.lines.len().saturating_sub(1));
                self.col = draft.col.min(self.cur_len());
            } else {
                self.clear();
            }
        } else if let Some(entry) = self.history.get(self.history_index as usize).cloned() {
            self.set_text(&entry);
            self.move_end();
        }
    }

    /// Stop browsing history (any edit/newline/submit, `editor.ts`).
    fn exit_history(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }

    // ---- autocomplete ----------------------------------------------------------------------

    /// The buffer lines as `String`s, for the autocomplete engine.
    fn lines_as_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Recompute the popup after an edit: auto-open for slash **and** `@`-mention context, otherwise
    /// update an already-open popup or close it (spec/tui/04 §5 — bare path does not auto-pop without
    /// Tab; `@`-mention auto-pops on `@`, `autocomplete.ts:101`).
    fn update_autocomplete(&mut self) {
        let was_open = self.autocomplete.is_some();
        // `@`-mention search auto-pops the moment an `@` token forms (whole-tree fuzzy file search).
        if let Some(ac) = self.compute_mention() {
            self.open_popup(ac);
            return;
        }
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            false,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.context == CompletionContext::Slash || was_open => self.open_popup(ac),
            _ => self.autocomplete = None,
        }
    }

    /// The text left of the cursor on the current line (the autocomplete context window).
    fn before_cursor(&self) -> String {
        self.lines.get(self.row).map_or(String::new(), |line| line.iter().take(self.col).collect())
    }

    /// Compute the `@`-mention popup for the current cursor, lazily enumerating the tree on first use
    /// (`autocomplete.ts:719-772`). `None` when the trailing token is not an `@`-mention or nothing
    /// matches.
    fn compute_mention(&mut self) -> Option<Autocomplete> {
        let before = self.before_cursor();
        crate::autocomplete::mention_query(&before)?;
        if self.mention_files.is_none() {
            self.mention_files = Some(crate::autocomplete::list_files(&self.cwd, 2000));
        }
        let files = self.mention_files.as_deref().unwrap_or(&[]);
        crate::autocomplete::mention_autocomplete(&before, files)
    }

    /// Trigger completion explicitly (Tab with no popup): force path completion, or slash completion
    /// while typing a `/name` (spec/tui/04 §5). A single forced match auto-applies (§3.7 item 10).
    fn trigger_completion(&mut self) -> EditorOutcome {
        // `@`-mention takes precedence on an explicit Tab too (whole-tree fuzzy file search).
        if let Some(ac) = self.compute_mention() {
            self.open_popup(ac);
            return EditorOutcome::Edited;
        }
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            true,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.list.len() == 1 && ac.context == CompletionContext::Path => {
                self.open_popup(ac);
                self.accept_completion();
                EditorOutcome::Edited
            }
            Some(ac) => {
                self.open_popup(ac);
                EditorOutcome::Edited
            }
            None => EditorOutcome::Ignored,
        }
    }

    /// Apply the selected popup item to the buffer (Tab/Enter accept). Leaves the popup state to the
    /// caller (Tab keeps editing + recomputes; Enter on a slash item submits).
    fn accept_completion(&mut self) {
        let Some(ac) = self.autocomplete.as_ref() else { return };
        if let Some(applied) = ac.apply(&self.lines_as_strings(), self.row, self.col) {
            self.lines = applied.lines.iter().map(|s| s.chars().collect()).collect();
            if self.lines.is_empty() {
                self.lines.push(Vec::new());
            }
            self.row = applied.cursor_line.min(self.lines.len().saturating_sub(1));
            self.col = applied.cursor_col.min(self.cur_len());
        }
    }

    // ---- key handling ----------------------------------------------------------------------

    /// Feed a key. Resolves an [`EditorAction`] via the keymap (R-10-018); printable chars insert.
    /// While a popup is open, navigation/accept/cancel route first (spec/tui/04 §5).
    pub fn handle_key(&mut self, ev: &KeyEvent) -> EditorOutcome {
        // 1. Char-jump mode consumes the next printable char (or cancels).
        if let Some(dir) = self.jump.take() {
            if let KeyCode::Char(c) = ev.code
                && !ev.modifiers.contains(KeyModifiers::CONTROL) {
                    self.jump_to(dir, c);
                    return EditorOutcome::Edited;
                }
            return EditorOutcome::Edited; // any other key cancels jump
        }

        // 2. Popup-open routing (before normal editing).
        if self.autocomplete.is_some()
            && let Some(outcome) = self.handle_popup_key(ev) {
                return outcome;
            }

        // 3. Resolve a bound editor action.
        if let Some(action) = self.keymap.action_for(ev) {
            return self.apply_editor_action(action);
        }

        // 4. Printable insert (no Ctrl/Super; Alt+char already routed via keymap or ignored).
        if let KeyCode::Char(c) = ev.code
            && !ev.modifiers.contains(KeyModifiers::CONTROL)
                && !ev.modifiers.contains(KeyModifiers::SUPER)
                && !ev.modifiers.contains(KeyModifiers::ALT)
            {
                self.push_undo_for_type(c);
                self.insert_char(c);
                self.last_action = LastAction::Type;
                self.exit_history();
                self.update_autocomplete();
                return EditorOutcome::Edited;
            }
        EditorOutcome::Ignored
    }

    /// Route a key while the popup is open through the configurable [`AutocompleteKeymap`] (item #6 —
    /// the nav/accept/cancel keys are no longer hardcoded). Returns `Some` if consumed; `None` to
    /// fall through to normal editing.
    fn handle_popup_key(&mut self, ev: &KeyEvent) -> Option<EditorOutcome> {
        use crate::keymap::AutocompleteAction as A;
        match self.autocomplete_keymap.action_for(ev)? {
            A::Cancel => {
                self.autocomplete = None;
                Some(EditorOutcome::Edited)
            }
            A::Previous => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_up();
                }
                Some(EditorOutcome::Edited)
            }
            A::Next => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_down();
                }
                Some(EditorOutcome::Edited)
            }
            A::Accept => {
                // Accept, keep editing (no submit), then recompute (may close if out of context).
                self.accept_completion();
                self.update_autocomplete();
                Some(EditorOutcome::Edited)
            }
            A::AcceptSubmit => {
                let is_slash = self
                    .autocomplete
                    .as_ref()
                    .is_some_and(|ac| ac.context == CompletionContext::Slash);
                self.accept_completion();
                self.autocomplete = None;
                if is_slash {
                    // Accepting a slash item with Enter submits (spec/tui/04 §5, edge 15). The accept
                    // appended a trailing space; submit trims it (Pi trims in the submit handler).
                    let text = self.text().trim().to_string();
                    self.add_to_history(&text);
                    self.clear();
                    Some(EditorOutcome::Submit(text))
                } else {
                    self.update_autocomplete();
                    Some(EditorOutcome::Edited)
                }
            }
        }
    }

    /// Dispatch a resolved editor action.
    fn apply_editor_action(&mut self, action: EditorAction) -> EditorOutcome {
        use EditorAction as E;
        // Any non-vertical action re-seeds the sticky goal column on the next Up/Down (spec/tui/03 §4.2).
        if !matches!(action, E::CursorUp | E::CursorDown) {
            self.reset_preferred_col();
        }
        match action {
            E::CursorLeft => {
                self.move_left();
                self.last_action = LastAction::None;
                EditorOutcome::Edited
            }
            E::CursorRight => {
                self.move_right();
                self.last_action = LastAction::None;
                EditorOutcome::Edited
            }
            E::CursorUp => {
                // History recall only fires on the first visual line (`history_up_eligible` already
                // requires `row == 0` + empty/browsing/col-0, which is always the first visual line).
                if self.history_up_eligible() {
                    self.history_older();
                } else {
                    self.move_up_visual();
                }
                EditorOutcome::Edited
            }
            E::CursorDown => {
                if self.history_index >= 0 {
                    self.history_newer();
                } else {
                    self.move_down_visual();
                }
                EditorOutcome::Edited
            }
            E::CursorWordLeft => {
                self.move_word_left();
                EditorOutcome::Edited
            }
            E::CursorWordRight => {
                self.move_word_right();
                EditorOutcome::Edited
            }
            E::CursorLineStart => {
                self.move_home();
                EditorOutcome::Edited
            }
            E::CursorLineEnd => {
                self.move_end();
                EditorOutcome::Edited
            }
            E::DeleteCharBackward => {
                self.push_undo_for(LastAction::None);
                self.backspace();
                self.last_action = LastAction::None;
                self.exit_history();
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteCharForward => {
                self.push_undo_for(LastAction::None);
                self.delete();
                self.last_action = LastAction::None;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteWordBackward => {
                self.push_undo_for(LastAction::Kill);
                self.delete_word_backward();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteWordForward => {
                self.push_undo_for(LastAction::Kill);
                self.delete_word_forward();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteToLineStart => {
                self.push_undo_for(LastAction::Kill);
                self.delete_to_line_start();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteToLineEnd => {
                self.push_undo_for(LastAction::Kill);
                self.delete_to_line_end();
                self.last_action = LastAction::Kill;
                EditorOutcome::Edited
            }
            E::Yank => {
                self.push_undo_for(LastAction::None);
                self.yank();
                self.last_action = LastAction::Yank;
                self.exit_history();
                EditorOutcome::Edited
            }
            E::YankPop => {
                self.yank_pop();
                self.last_action = LastAction::Yank;
                EditorOutcome::Edited
            }
            E::Undo => {
                self.undo();
                self.last_action = LastAction::None;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::NewLine => {
                self.push_undo_for(LastAction::None);
                self.insert_newline();
                self.last_action = LastAction::None;
                self.exit_history();
                EditorOutcome::Edited
            }
            E::Submit => {
                // Backslash-Enter → soft newline (Pi `editor.ts:796-802`, spec/tui/03 §5.7): a
                // workaround for terminals without Shift+Enter. If the char immediately before the
                // cursor is a literal backslash, delete it and insert a newline INSTEAD of submitting
                // (Pi `handleBackspace()` + `addNewLine()`), so `foo\<Enter>` breaks the line.
                if self.col > 0
                    && self.lines.get(self.row).and_then(|l| l.get(self.col - 1)) == Some(&'\\')
                {
                    self.push_undo_for(LastAction::None);
                    self.backspace();
                    self.insert_newline();
                    self.last_action = LastAction::None;
                    self.exit_history();
                    self.update_autocomplete();
                    return EditorOutcome::Edited;
                }
                // Expand large-paste markers back to their full content before the agent sees the text
                // (`expandPasteMarkers`, spec/tui/03 §5.5).
                let text = self.expanded_text();
                if text.trim().is_empty() {
                    return EditorOutcome::Edited;
                }
                self.add_to_history(&text);
                self.clear();
                EditorOutcome::Submit(text)
            }
            E::Tab => self.trigger_completion(),
            E::JumpForward => {
                self.jump = Some(JumpDir::Forward);
                EditorOutcome::Edited
            }
            E::JumpBackward => {
                self.jump = Some(JumpDir::Backward);
                EditorOutcome::Edited
            }
        }
    }

    /// The hardware-cursor position inside `area` (for IME placement, R-10-009), if focused. Accounts
    /// for the top border rule drawn by [`Component::render`]; the editor has no side borders.
    pub fn cursor_in(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.focused {
            return None;
        }
        // Map the logical caret `(row, col)` to its VISUAL `(vrow, vcol)` via the wrap map (built at
        // the render `view_width`) so the hardware cursor lands on the wrapped row/column, matching
        // the reverse-video soft-cursor cell drawn in `render` (Pi `editor.ts:545-551`).
        let map = self.visual_line_map();
        let vi = self.current_visual_line(&map);
        let vl = map.get(vi).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let vcol = self.col.saturating_sub(vl.start);
        // Only the first visual row carries the prompt-glyph offset (`› `); later rows start flush.
        let prompt = if vi == 0 { PROMPT_W } else { 0 };
        let x = area
            .x
            .saturating_add(prompt)
            .saturating_add(vcol.min(u16::MAX as usize) as u16);
        let y = area.y.saturating_add(1).saturating_add(vi.min(u16::MAX as usize) as u16);
        let max_x = area.x.saturating_add(area.width).saturating_sub(1);
        let max_y = area.y.saturating_add(area.height).saturating_sub(1);
        Some((x.min(max_x), y.min(max_y)))
    }
}

/// Whether `c` is a word char (alphanumeric or `_`), for word-motion class runs.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Word-aware wrap of one logical line into `(start_col, len)` visual segments fitting `width`
/// (`wordWrapLine`, `editor.ts:114-206`): break at the last whitespace boundary that fits; force-break
/// an overlong word at column granularity. An empty line yields one zero-length segment. `width` is
/// assumed `>= 1` (callers clamp). Columns are char indices into `line`.
fn word_wrap_line(line: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let n = line.len();
    if n == 0 {
        return vec![(0, 0)];
    }
    let mut segs = Vec::new();
    let mut start = 0;
    while start < n {
        if n - start <= width {
            segs.push((start, n - start));
            break;
        }
        let hard_end = start + width;
        // Last whitespace boundary strictly after `start` and within the window — break *after* it so
        // the trailing space stays on the wrapped line (matching Pi's greedy word wrap).
        let mut end = hard_end;
        let mut i = hard_end;
        while i > start {
            if line.get(i - 1).is_some_and(|c| c.is_whitespace()) {
                end = i;
                break;
            }
            i -= 1;
        }
        // No whitespace in the window ⇒ force-break the overlong word at the hard edge.
        if end <= start {
            end = hard_end;
        }
        segs.push((start, end - start));
        start = end;
    }
    if segs.is_empty() {
        segs.push((0, 0));
    }
    segs
}

/// Sanitize a bracketed-paste payload (`editor.ts:1142-1179`): normalize `\r\n`/`\r` to `\n`, expand
/// tabs to four spaces, and drop control bytes other than `\n`.
fn sanitize_paste(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(unified.len());
    for c in unified.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}


/// The grapheme-cluster boundaries of `line` expressed as **char-column** indices, including the
/// leading `0` and trailing line length (`unicode_segmentation` extended grapheme clusters — the
/// boundaries Pi's editor steps the cursor over). A pure-ASCII line yields every column.
fn grapheme_boundaries(line: &[char]) -> Vec<usize> {
    let s: String = line.iter().collect();
    let mut bounds = Vec::with_capacity(line.len() + 1);
    bounds.push(0usize);
    let mut col = 0usize;
    for g in s.graphemes(true) {
        col += g.chars().count();
        bounds.push(col);
    }
    bounds
}

impl Component for InputEditor {
    /// Render the editor with **top + bottom rules only** (no side bars, no title) — Pi
    /// `editor.ts:476,517,575` (spec/tui/03 §3.1). The rule color flips to bash-green while the buffer
    /// starts with `!` (spec/tui/03 §7.1); otherwise it uses the border role, accented when focused.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // Record the layout width so vertical (visual-line) motion wraps the same way it is drawn.
        // The editor has no side borders; one column is reserved for the end-of-line cursor cell
        // (`editor.ts:471` `layout_width = content_width - 1`).
        self.view_width = (area.width.saturating_sub(1)).max(1) as usize;
        // The rule color is the primary always-visible mode signal (spec/tui/03 §3.3): bash-green
        // while the buffer starts with `!`, else the escalating thinking-level color. The previous
        // hardwired bright-blue accent-on-focus was wrong (audit #3).
        let rule_style = if self.is_bash_mode() {
            theme.bash_mode_style()
        } else {
            theme.thinking_border_style(&self.thinking_level)
        };
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_set(border::PLAIN)
            .border_style(rule_style);
        // An accent prompt glyph `›` anchors the editor's first line; a reverse-video soft cursor cell
        // makes the caret visible every idle frame (overview §1.1 glyph vocab `prompt ›`; spec/tui/03
        // §3.4 reverse-video cursor; Pi `editor.ts:545-551`). Without these the body row paints blank
        // because the hardware cursor (`set_cursor_position`) is invisible in a headless buffer.
        let base = theme.base_style();
        let prompt_style = theme.accent_style();
        let cursor_style = base.add_modifier(Modifier::REVERSED);
        // Expand each LOGICAL line into its wrapped VISUAL lines at `view_width` (`editor.ts:1690`
        // `build_visual_line_map`, the same primitive vertical motion uses) and emit one ratatui
        // `Line` per visual line — so text past the width flows onto the next row instead of clipping
        // (the `Paragraph` has no `.wrap`, so it renders exactly the rows we build). The soft cursor
        // rides its VISUAL row/col, not the logical column.
        let map = self.visual_line_map();
        let cursor_vl = if self.focused { self.current_visual_line(&map) } else { usize::MAX };
        let mut lines: Vec<Line> = Vec::with_capacity(map.len());
        for (vi, vl) in map.iter().enumerate() {
            let mut spans: Vec<Span> = Vec::new();
            // The prompt glyph anchors only the very first visual row (Pi first-line prompt).
            if vi == 0 {
                spans.push(Span::styled(PROMPT, prompt_style));
            }
            // The chars this visual line slices out of its logical line.
            let seg: Vec<char> = self
                .lines
                .get(vl.logical)
                .map(|l| l.iter().skip(vl.start).take(vl.len).copied().collect())
                .unwrap_or_default();
            if vi == cursor_vl {
                // Cursor column within THIS visual line (0 at a wrap boundary; == len at line end).
                let vcol = self.col.saturating_sub(vl.start).min(seg.len());
                let before: String = seg.iter().take(vcol).collect();
                spans.push(Span::styled(before, base));
                match seg.get(vcol) {
                    Some(c) => {
                        spans.push(Span::styled(c.to_string(), cursor_style));
                        let after: String = seg.iter().skip(vcol + 1).collect();
                        spans.push(Span::styled(after, base));
                    }
                    // End-of-line caret: a reverse-video space (Pi `editor.ts:550`).
                    None => spans.push(Span::styled(" ", cursor_style)),
                }
            } else {
                spans.push(Span::styled(seg.iter().collect::<String>(), base));
            }
            lines.push(Line::from(spans));
        }
        let para = Paragraph::new(lines).block(block).style(base);
        frame.render_widget(para, area);
        if let Some((x, y)) = self.cursor_in(area) {
            frame.set_cursor_position((x, y));
        }
    }
}
