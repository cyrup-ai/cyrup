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
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::autocomplete::{Autocomplete, CompletionContext};
use crate::commands::CommandRegistry;
use crate::component::Component;
use crate::keymap::{EditorAction, EditorKeymap};
use crate::theme::UiTheme;

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
    /// First **visual** line of the render window (`editor.ts:288` `scrollOffset`). The editor shows
    /// at most `maxVisibleLines` rows; anything above/below is scrolled out and announced by the
    /// `─── ↑ N more ` / `─── ↓ N more ` rules ([`scroll_border`], `editor.ts:259-268`). Kept in range
    /// and re-pointed at the caret every render (`editor.ts:507-516`) and reset to `0` whenever the
    /// buffer is replaced wholesale (`editor.ts:471`, `:449`).
    scroll_offset: usize,
    /// The host TERMINAL's row count, from which the editor derives its own visible-row budget
    /// (`editor.ts:499-501`):
    ///
    /// ```text
    /// const terminalRows = this.tui.terminal.rows;
    /// const maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3));
    /// ```
    ///
    /// The cap is INSIDE the component upstream, read from `this.tui` inside a `render(width)` that
    /// takes no height at all (`:499-501`) — the editor is never told how tall its slot is and never
    /// trusts a container to have windowed it. cyrup derived the same number solely from
    /// `area.height - 2`, i.e. from whatever rect the caller happened to hand it, so an
    /// [`InputEditor`] rendered anywhere other than [`crate::app::region_constraints`]'s slot — a
    /// bare `Component::render` into a taller rect, an embedder's own layout — silently lost the cap
    /// and drew as many rows as it was given.
    ///
    /// `24` until [`Self::set_terminal_height`] lands, pi's own `?? 24` fallback for a missing
    /// terminal height (`config-selector.ts:264-266`). The rect still participates: the render
    /// window is `min(area.height - 2, max(5, floor(rows * 0.3)))`, so a slot CLIPPED shorter than
    /// the editor asked for still degrades correctly — something pi's height-free `render(width)`
    /// cannot express.
    term_rows: u16,
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
    /// The current reasoning level (`off`/`minimal`/`low`/`medium`/`high`/`xhigh`/`max`) — the editor's
    /// top/bottom rule color is the primary always-visible thinking-level signal
    /// (`interactive-mode.ts:3533-3541`, spec/tui/03 §3.3). Recolored green in bash mode. Updated by
    /// the app on `ThinkingLevelChanged`; `"medium"` until set.
    thinking_level: String,
    /// Whether this editor's rule is owned by a reasoning level at all (T9, TUI-FIDELITY §2).
    ///
    /// Pi's shared `Editor` takes its rule colour from `getEditorTheme().borderColor` =
    /// `theme.fg("borderMuted", …)` (v0.84.1 `theme.ts:1301-1304`, `tui/src/components/editor.ts:348`).
    /// Only the *chat* editor is then reassigned per thinking level / bash mode
    /// (`interactive-mode.ts:3990-3993`). An `ExtensionEditorComponent` — `new Editor(tui,
    /// getEditorTheme(), options)`, `components/extension-editor.ts:70` — never is, so it keeps
    /// `borderMuted`. `true` (the chat editor) by default; the extension-editor dialog clears it via
    /// [`InputEditor::use_muted_border`].
    thinking_level_owns_border: bool,
    /// Horizontal padding, in columns, applied INSIDE the top/bottom rules (Pi `editorPaddingX` →
    /// `CustomEditor({paddingX})`, `tui/src/components/editor.ts:349,484-489`). `0` (Pi's default)
    /// keeps the historical flush layout. The rules themselves still span the full width — Pi pads
    /// only the text rows (`editor.ts:522` left/right pad vs `:530` `horizontal.repeat(width)`).
    padding_x: u16,
    /// Whether the terminal's real (hardware) cursor is placed on the caret each frame (Pi
    /// `showHardwareCursor` → `TUI.setShowHardwareCursor`, `tui/src/tui.ts:346-352,1659-1663`).
    /// **Off** by default, as Pi's (`tui.ts:312`, `settings-manager.ts:1182` — only an explicit
    /// setting or `PI_HARDWARE_CURSOR=1` turns it on): the always-drawn reverse-video soft cursor is
    /// the caret the user sees, and Pi calls `terminal.hideCursor()` on every frame while this is
    /// false. Ratatui couples position and visibility (`Terminal::draw` hides the cursor whenever
    /// `Frame::set_cursor_position` was not called), so this flag gates that call.
    show_hardware_cursor: bool,
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
            scroll_offset: 0,
            term_rows: 24,
            preferred_visual_col: None,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            thinking_level: "medium".to_string(),
            thinking_level_owns_border: true,
            padding_x: 0,
            show_hardware_cursor: false,
        }
    }

    /// Set the horizontal editor padding (Pi `CustomEditor.setPaddingX`,
    /// `tui/src/components/editor.ts:370-376`): non-finite → `0`, else `max(0, floor(padding))`.
    /// The `/settings` grid cycles `0..=3` and [`cyrup_config::SettingsManager::set_editor_padding_x`]
    /// clamps the persisted value to that range; this setter reproduces Pi's own coercion so a
    /// hand-edited `editorPaddingX` can never render negative or fractional padding.
    pub fn set_padding_x(&mut self, padding: i64) {
        self.padding_x = padding.clamp(0, u16::MAX as i64) as u16;
    }

    /// The current horizontal padding (Pi `getPaddingX`, `editor.ts:365-367`).
    #[must_use]
    pub fn padding_x(&self) -> u16 {
        self.padding_x
    }

    /// Show/hide the terminal's real cursor (Pi `TUI.setShowHardwareCursor`, `tui/src/tui.ts:346`).
    /// See [`Self::show_hardware_cursor`] for why this lives on the editor in cyrup.
    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        self.show_hardware_cursor = enabled;
    }

    /// Whether the hardware cursor is placed each frame (Pi `getShowHardwareCursor`, `tui.ts:342`).
    ///
    /// Cyrup has no standalone `TUI` object owning terminal state — ratatui's `Terminal` does, and
    /// the ONLY component that asks for a cursor position is this editor
    /// ([`Self::cursor_in`] is the sole `Frame::set_cursor_position` caller in the crate), so the
    /// flag rides here rather than on a `TUI` port that does not exist.
    #[must_use]
    pub fn show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
    }

    /// The padding actually applied at render width `width`, clamped exactly as Pi clamps it
    /// (`editor.ts:483-484`: `min(paddingX, floor((width - 1) / 2))`) so the content column can
    /// never be squeezed out of existence on a narrow terminal.
    pub fn effective_padding(&self, width: u16) -> u16 {
        self.padding_x.min(width.saturating_sub(1) / 2)
    }

    /// The text layout width inside `width` columns, after padding (Pi `editor.ts:485-489`):
    /// `contentWidth = max(1, width - 2 * paddingX)`, and one column is reserved for the
    /// end-of-line cursor cell ONLY when there is no padding for it to overflow into.
    ///
    /// **Public because measurement and render must agree** (E15). Pi computes `layoutWidth` ONCE
    /// per `render` and feeds the same number to `this.lastWidth` and to `layoutText()`
    /// (`editor.ts:489-497`), so the row count the container reserves is by construction the row
    /// count the editor draws. cyrup splits those two across `app::region_constraints` (which sizes
    /// the slot) and [`Component::render`] (which fills it), and the former used to measure at a
    /// hardcoded `width - 1` while the latter wrapped at `width - 2 * paddingX`: with
    /// `editorPaddingX > 0` the render produced MORE rows than the slot had, and the surplus — the
    /// caret row included — was clipped. Both sides now call this.
    pub fn layout_width(&self, width: u16) -> u16 {
        let pad = self.effective_padding(width);
        let content = width.saturating_sub(pad.saturating_mul(2)).max(1);
        if pad > 0 { content } else { content.saturating_sub(1).max(1) }
    }

    /// Set the reasoning level driving the editor's rule color (spec/tui/03 §3.3). Called by the app
    /// on `ThinkingLevelChanged` / thinking-selector confirm.
    pub fn set_thinking_level(&mut self, level: impl Into<String>) {
        self.thinking_level = level.into();
    }

    /// Detach this editor's rule from the reasoning level, leaving it on the `borderMuted` role —
    /// the state Pi's shared `Editor` is *born* in (`getEditorTheme().borderColor`, v0.84.1
    /// `theme.ts:1301-1304`) and never leaves unless something reassigns `borderColor`
    /// (`interactive-mode.ts:3990-3993`, which only ever touches the chat editor). Used by the
    /// `ui.editor` extension dialog, whose Pi counterpart is a bare
    /// `new Editor(tui, getEditorTheme(), options)` (`components/extension-editor.ts:70`).
    pub fn use_muted_border(&mut self) {
        self.thinking_level_owns_border = false;
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
        // `editor.ts:471`: "Reset scroll - render() will adjust to show cursor".
        self.scroll_offset = 0;
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
        self.scroll_offset = 0;
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

    /// Tell the editor how many rows the host TERMINAL has — pi's `this.tui.terminal.rows`
    /// (`editor.ts:500`). Drives [`term_rows`](Self::term_rows), i.e. the `max(5, floor(rows *
    /// 0.3))` window the editor caps itself at. The app publishes this every `draw`; an embedder
    /// that never calls it gets pi's `?? 24` default.
    pub fn set_terminal_height(&mut self, rows: u16) {
        self.term_rows = rows.max(1);
    }

    /// The editor's own visible-row budget at the current [`term_rows`](Self::term_rows) —
    /// `Math.max(5, Math.floor(terminalRows * 0.3))` (`editor.ts:501`).
    pub fn max_visible_lines(&self) -> u16 {
        crate::app::max_visible_editor_lines(self.term_rows)
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

    /// Seed the prompt history with an already-submitted line — Pi's `editor.addToHistory?.(text)`
    /// on the `populateHistory` replay path (interactive-mode.ts:3387). Same skip rules as a live
    /// submission; call in chronological order so the newest replayed prompt ends up first.
    pub fn push_history(&mut self, text: &str) {
        self.add_to_history(text);
    }

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
        // The caret's COLUMN is the visible width of the text before it, not its char count. Pi never
        // does this arithmetic — it emits a zero-width `CURSOR_MARKER` *inside* the row string
        // (`editor.ts:550`) and lets the terminal advance the cursor by the real cell widths — so a
        // char-count offset is a cyrup-only bug: one emoji ahead of the caret and the hardware cursor
        // (and hence the IME candidate window) lands a column left of the reverse-video cell.
        let before_width: usize = self
            .lines
            .get(vl.logical)
            .map(|l| {
                let s: String = l.iter().skip(vl.start).take(vcol).collect();
                Span::raw(s).width()
            })
            .unwrap_or(0);
        // The text rows start `editorPaddingX` columns in (`Padding::horizontal` on the render
        // block), so the caret must too — Pi prefixes the same `leftPadding` (`editor.ts:522`).
        let x = area
            .x
            .saturating_add(self.effective_padding(area.width))
            .saturating_add(before_width.min(u16::MAX as usize) as u16);
        // The caret rides the SCROLLED window: row `vi` is drawn at `vi - scrollOffset` inside the
        // rules (`editor.ts:519` slices `layoutLines` from `scrollOffset`).
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(vi.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16);
        let max_x = area.x.saturating_add(area.width).saturating_sub(1);
        let max_y = area.y.saturating_add(area.height).saturating_sub(1);
        Some((x.min(max_x), y.min(max_y)))
    }
}

/// Whether `c` is a word char (alphanumeric or `_`), for word-motion class runs.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The **display width** of `s` in terminal cells — Pi's `visibleWidth` (`utils.ts:240-...`), which
/// is what every wrap and caret computation upstream measures with. `Span::width()` is ratatui's
/// `unicode_width` sum, the same primitive `transcript.rs` already uses for `Box.applyBg`.
///
/// Not `chars().count()`: a CJK ideograph is one `char` and TWO columns, a combining mark is one
/// `char` and ZERO, so a char count is neither an upper nor a lower bound on the cells a string
/// occupies.
fn display_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Whether a grapheme counts as whitespace — Pi's `isWhitespaceChar`, which is literally
/// `/\s/.test(char)` (`utils.ts:943-945`), i.e. *contains* a whitespace code point rather than *is*
/// one, hence `any` and not `all`.
///
/// [CYRUP-DELTA] JS `\s` and Rust's `char::is_whitespace` (Unicode `White_Space`) differ on exactly
/// two code points in either direction: `\s` includes U+FEFF (which `White_Space` does not) and
/// `White_Space` includes U+0085 NEL (which `\s` does not). Both are stripped before they can reach
/// the buffer — U+FEFF and U+0085 are `char::is_control()`/format characters that
/// [`sanitize_paste`] drops and that no key event produces — so the sets coincide on every input
/// this function can actually see.
fn is_whitespace_seg(g: &str) -> bool {
    g.chars().any(char::is_whitespace)
}

/// Whether a grapheme offers a CJK line-break opportunity — Pi's `cjkBreakRegex` (`utils.ts:54-55`):
///
/// ```text
/// /[\p{Script_Extensions=Han}\p{Script_Extensions=Hiragana}\p{Script_Extensions=Katakana}
///   \p{Script_Extensions=Hangul}\p{Script_Extensions=Bopomofo}]/u
/// ```
///
/// Tested against the whole grapheme (the regex is unanchored, so it matches if ANY code point in
/// the cluster qualifies) — hence `any`, matching `cjkBreakRegex.test(grapheme)`.
///
/// [CYRUP-DELTA] Rust's standard library carries no `Script_Extensions` data and pulling in a
/// unicode-script crate for one predicate would be a new external dependency for a table that does
/// not move (these five scripts' blocks are stable since Unicode 13). The ranges below are the
/// assigned blocks of those five scripts plus the shared-ideographic code points
/// (`〄〇〡-〩〸-〻`) that `Script_Extensions=Han` picks up beyond `Script=Han`.
fn is_cjk_break(g: &str) -> bool {
    g.chars().map(u32::from).any(|c| {
        matches!(c,
            0x1100..=0x11FF   // Hangul Jamo
            | 0x2E80..=0x2EFF // CJK Radicals Supplement       (Han)
            | 0x2F00..=0x2FDF // Kangxi Radicals               (Han)
            | 0x3005          // 々 ideographic iteration mark (Han)
            | 0x3007          // 〇 ideographic number zero    (Han)
            | 0x3021..=0x3029 // 〡-〩 Hangzhou numerals        (Han)
            | 0x3038..=0x303B // 〸-〻                          (Han)
            | 0x3041..=0x309F // Hiragana (incl. the shared voiced-sound marks)
            | 0x30A0..=0x30FF // Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul Compatibility Jamo
            | 0x31A0..=0x31BF // Bopomofo Extended
            | 0x31F0..=0x31FF // Katakana Phonetic Extensions
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xA960..=0xA97F // Hangul Jamo Extended-A
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xD7B0..=0xD7FF // Hangul Jamo Extended-B
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFE30..=0xFE4F // CJK Compatibility Forms       (Han)
            | 0xFF66..=0xFF9F // Halfwidth Katakana
            | 0xFFA0..=0xFFDC // Halfwidth Hangul
            | 0x1B000..=0x1B16F // Kana Supplement / Extended-A / Small Kana Extension
            | 0x20000..=0x2FA1F // CJK Extensions B-F + Compatibility Supplement
            | 0x30000..=0x323AF // CJK Extensions G-H
        )
    })
}

/// Word-aware wrap of one logical line into `(start_col, len)` visual segments fitting `width`
/// **display columns** — a 1:1 port of `wordWrapLine` (`editor.ts:114-206`). An empty line yields
/// one zero-length segment. `width` is assumed `>= 1` (callers clamp). The returned columns are
/// char indices into `line` (cyrup's buffer is `Vec<char>`, so char indices are its `string.slice`),
/// and the segments tile the line contiguously.
///
/// The three things the previous implementation got wrong, all from measuring `n - start <= width`
/// over a `&[char]` — a CHAR COUNT:
///
/// 1. **Width.** Upstream accumulates `visibleWidth(grapheme)` (`:139-143`), so 24 CJK ideographs
///    are 48 columns, not 24. At a layout width of 39 the char count said "fits", the map reported
///    one visual line, four ideographs rendered past the right edge and — because
///    [`Self::cursor_in`] resolves the caret through that same map — the caret left the frame.
/// 2. **Granularity.** Upstream iterates GRAPHEMES and breaks at a cluster's own start index
///    (`:157-160`), so a break never lands inside a cluster. Breaking at `start + width` char-wise
///    put `👨` on one row and a bare `\u{200d}👩‍👧‍👦` on the next.
/// 3. **CJK break opportunities.** Upstream records a wrap opportunity between any two adjacent
///    non-space graphemes when either is CJK (`:191-198`), because CJK text has no spaces to break
///    at. Without it a whole CJK paragraph is one unbreakable "word".
///
/// The loop below is upstream's, statement for statement: an overflow check that first tries to
/// backtrack to the last recorded opportunity and otherwise force-breaks at the current cluster's
/// start (`:145-161`), then the advance and the opportunity bookkeeping (`:180-199`).
///
/// [CYRUP-DELTA] `:163-178` handles a single segment wider than `maxWidth` by *recursively*
/// re-wrapping it, which upstream needs because its segmenter merges a whole `[paste #N …]` marker
/// into one atomic segment. cyrup's segments are plain extended grapheme clusters — never composite
/// — so there is nothing to re-wrap: an over-wide cluster (a wide emoji at `width == 1`) is
/// indivisible and takes a row of its own. That is where upstream's recursion converges for a
/// splittable segment, and it is also the case upstream cannot express at all: `wordWrapLine("👨",
/// 1)` recurses on itself forever.
fn word_wrap_line(line: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let n = line.len();
    // `if (!line || maxWidth <= 0) return [{ text: "", startIndex: 0, endIndex: 0 }]` (`:115-117`).
    if n == 0 {
        return vec![(0, 0)];
    }
    let s: String = line.iter().collect();
    // `if (lineWidth <= maxWidth) return [{ text: line, ... }]` (`:119-122`).
    if display_width(&s) <= width {
        return vec![(0, n)];
    }

    // `const segments = [...graphemeSegmenter.segment(line)]` (`:125`), carrying each cluster's
    // start index — `seg.index` upstream, a char column here.
    let mut segs: Vec<(usize, &str)> = Vec::with_capacity(n);
    let mut col = 0usize;
    for g in s.graphemes(true) {
        segs.push((col, g));
        col += g.chars().count();
    }

    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    // `wrapOppIndex` / `wrapOppWidth` (`:131-133`), as one `Option` so `-1` cannot leak.
    let mut wrap_opp: Option<(usize, usize)> = None;

    for i in 0..segs.len() {
        let Some(&(char_index, grapheme)) = segs.get(i) else { continue };
        let g_width = display_width(grapheme);
        let is_ws = is_whitespace_seg(grapheme);

        // "Overflow check before advancing" (`:145-161`).
        if current_width + g_width > width {
            match wrap_opp {
                // "Backtrack to last wrap opportunity (the remaining content plus the current
                // grapheme still fits within maxWidth)" (`:147-153`).
                Some((opp_index, opp_width))
                    if current_width.saturating_sub(opp_width) + g_width <= width =>
                {
                    chunks.push((chunk_start, opp_index.saturating_sub(chunk_start)));
                    chunk_start = opp_index;
                    current_width = current_width.saturating_sub(opp_width);
                }
                // "No viable wrap opportunity: force-break at current position" (`:154-160`).
                _ if chunk_start < char_index => {
                    chunks.push((chunk_start, char_index - chunk_start));
                    chunk_start = char_index;
                    current_width = 0;
                }
                _ => {}
            }
            wrap_opp = None;
        }

        // `if (gWidth > maxWidth)` (`:163`) — see the [CYRUP-DELTA] above.
        if g_width > width {
            if chunk_start < char_index {
                chunks.push((chunk_start, char_index - chunk_start));
            }
            chunk_start = char_index;
            current_width = g_width;
            wrap_opp = None;
            continue;
        }

        // "Advance" (`:181`).
        current_width += g_width;

        // "Record wrap opportunity" (`:183-199`): whitespace followed by non-whitespace (multiple
        // spaces join; the break point is after the last space), or a boundary where either side is
        // CJK.
        if let Some(&(next_index, next)) = segs.get(i + 1)
            // Upstream spells this as two arms — whitespace→non-whitespace (`:187-189`) and the CJK
            // boundary (`:190-198`) — that assign the same pair. Merged into one predicate because
            // clippy's `if_same_then_else` rejects the duplicated arm; `is_ws || cjk || cjk` under a
            // shared `!next_is_ws` is exactly the disjunction of the two upstream guards.
            && !is_whitespace_seg(next)
            && (is_ws || is_cjk_break(grapheme) || is_cjk_break(next))
        {
            wrap_opp = Some((next_index, current_width));
        }
    }

    // "Push final chunk" (`:202`).
    chunks.push((chunk_start, n.saturating_sub(chunk_start)));
    chunks
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


/// One scroll-indicator rule, a 1:1 port of `createScrollBorder` (`editor.ts:259-268`):
///
/// ```text
/// const indicator = `─── ${direction} ${hiddenLineCount} more `;
/// const remaining = availableWidth - visibleWidth(indicator);
/// if (remaining >= 0) return indicator + "─".repeat(remaining);
/// const ellipsis = "...".slice(0, availableWidth);
/// return sliceByColumn(indicator, 0, availableWidth - visibleWidth(ellipsis), true) + ellipsis;
/// ```
///
/// `direction` is `'↑'` (rows scrolled off the top) or `'↓'` (rows still below).
///
/// **The trailing `true` is `strict`, not a pad flag.** `sliceByColumn(line, startCol, length,
/// strict = false)` (`utils.ts:1195-1197`) forwards to `sliceWithWidth`, whose `strict` drops a
/// grapheme that would straddle the end column (`:1224`, `const fits = !strict || currentCol + w <=
/// endCol`); it returns `{ text, width }` and pads NOTHING. The `pad` parameter that does exist
/// upstream belongs to `truncateToWidth`/`finalizeTruncatedResult`, a different function. So
/// `createScrollBorder`'s fallback is not padded upstream and is not padded here — the loop below is
/// that strict slice, and it is equivalent statement for statement: upstream skips a non-fitting
/// grapheme and then breaks on `currentCol >= endCol`, which for a strictly-increasing column count
/// is the same set of graphemes this `break` keeps.
///
/// The result is nevertheless always exactly `width` display columns, and that is a property of the
/// indicator's ALPHABET rather than of the slice: `─`, the space, `↑`/`↓` (East-Asian Ambiguous,
/// hence narrow) and the decimal digits are every one of them a single column, so `strict` never has
/// a wide grapheme to reject. [`tests::the_scroll_rule_is_exactly_as_wide_as_it_is_asked_for`] pins
/// it across the whole width range, because cyrup — unlike pi, which composes each row from scratch
/// — paints this string OVER the `Block`'s already-drawn rule, and a short string would leak the
/// `─`s underneath.
fn scroll_border(direction: char, hidden: usize, width: u16) -> String {
    let avail = usize::from(width);
    let indicator = format!("─── {direction} {hidden} more ");
    let indicator_w = display_width(&indicator);
    if avail >= indicator_w {
        let mut out = indicator;
        out.push_str(&"─".repeat(avail - indicator_w));
        return out;
    }
    // Too narrow for the whole indicator: keep as many leading columns as fit, then `...` (itself
    // truncated to the available width on a truly tiny terminal).
    let ellipsis: String = "...".chars().take(avail).collect();
    let budget = avail.saturating_sub(display_width(&ellipsis));
    let mut out = String::new();
    let mut used = 0usize;
    for g in indicator.graphemes(true) {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push_str(&ellipsis);
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
        // (`editor.ts:471` `layout_width = content_width - 1`) — unless `editorPaddingX` gave the
        // caret padding to overflow into (`editor.ts:489`).
        let pad = self.effective_padding(area.width);
        self.view_width = self.layout_width(area.width) as usize;
        // The rule color is the primary always-visible mode signal (spec/tui/03 §3.3): bash-green
        // while the buffer starts with `!`, else the escalating thinking-level color. The previous
        // hardwired bright-blue accent-on-focus was wrong (audit #3).
        let rule_style = if self.is_bash_mode() {
            theme.bash_mode_style()
        } else if self.thinking_level_owns_border {
            theme.thinking_border_style(&self.thinking_level)
        } else {
            // T9: an editor nobody reassigned keeps `getEditorTheme().borderColor` = `borderMuted`
            // (Pi `theme.ts:1301-1304` → `tui/src/components/editor.ts:348`).
            theme.border_muted_style()
        };
        // `editorPaddingX` insets the TEXT only: ratatui's `Block` draws its top/bottom rules across
        // the full `area` and applies `Padding` to the inner area the `Paragraph` fills, which is
        // exactly Pi's split (`editor.ts:522` pads the text rows; `:530` repeats the rule glyph
        // `width` times).
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_set(border::PLAIN)
            .border_style(rule_style)
            .padding(Padding::horizontal(pad));
        // A reverse-video soft cursor cell makes the caret visible every idle frame (Pi
        // `editor.ts:545-564`). Without it the body row paints blank, because the hardware cursor
        // (`set_cursor_position`) is invisible in a headless buffer.
        //
        // **E1 — there is no prompt glyph.** cyrup used to open row 0 with an accent `› `. Pi's
        // `Editor.render` (`editor.ts:482-601`) emits only `${leftPadding}${displayText}${padding}
        // ${lineRightPadding}` (`:578`); nothing anywhere in the chat editor's construction adds a
        // leading glyph — the chat editor is a bare `new CustomEditor(this.ui, getEditorTheme(),
        // this.keybindings, {…})` (`interactive-mode.ts:563-566`) and `CustomEditor`
        // (`components/custom-editor.ts`, 90 lines) overrides `handleInput` ONLY, with no `render`.
        // The `›` upstream *does* draw is the SELECTED-ROW cursor of the list selectors
        // (`session-selector.ts:476`, `tree-selector.ts:689`, `user-message-selector.ts:57`), a
        // different component. Removing it also fixes **E2**: row 0 was `PROMPT_W + view_width`
        // columns wide inside a `view_width`-wide area (last character clipped, end-of-line caret off
        // the right edge) while rows 1..n started two columns to its left — a permanent ragged left
        // edge. Every row now starts flush at `leftPadding`, exactly as `:578` does.
        let base = theme.base_style();
        let cursor_style = base.add_modifier(Modifier::REVERSED);
        // Expand each LOGICAL line into its wrapped VISUAL lines at `view_width` (`editor.ts:1690`
        // `build_visual_line_map`, the same primitive vertical motion uses) and emit one ratatui
        // `Line` per visual line — so text past the width flows onto the next row instead of clipping
        // (the `Paragraph` has no `.wrap`, so it renders exactly the rows we build). The soft cursor
        // rides its VISUAL row/col, not the logical column.
        let map = self.visual_line_map();
        // **E13 — the caret survives focus loss.** Pi gates `focused` on the zero-width hardware
        // `CURSOR_MARKER` alone (`editor.ts:537,550`); the reverse-video cell is emitted purely from
        // `layoutLine.hasCursor`, which `layoutText` sets from the cursor position and never consults
        // `focused` (`editor.ts:905-960`). cyrup used to set `cursor_vl = usize::MAX` when unfocused,
        // so clicking away from the terminal (`FocusLost`) erased the caret entirely. The
        // focus-gated half lives on in [`Self::cursor_in`], which is cyrup's `CURSOR_MARKER`.
        let cursor_vl = self.current_visual_line(&map);
        // **E4 — the visible window scrolls; it does not clip.** Pi slices `layoutLines` to
        // `maxVisibleLines` after moving `scrollOffset` to keep the caret inside
        // (`editor.ts:499-519`).
        //
        // **E17 — the cap is the component's own.** Upstream reads `this.tui.terminal.rows` inside a
        // `render(width)` that takes no height (`:499-501`); the budget is intrinsic and the
        // container is never consulted. cyrup took `area.height - 2` alone — correct only for as
        // long as the one caller happened to size the slot from the same formula, and silently
        // uncapped for every other caller. It is now `min(rect, intrinsic)`: the intrinsic budget is
        // pi's, and the rect stays in the `min` so a slot CLIPPED shorter than the editor asked for
        // still degrades correctly rather than overdrawing its neighbours.
        let max_visible = usize::from(area.height.saturating_sub(2))
            .min(usize::from(self.max_visible_lines()))
            .max(1);
        if cursor_vl < self.scroll_offset {
            self.scroll_offset = cursor_vl;
        } else if cursor_vl >= self.scroll_offset.saturating_add(max_visible) {
            self.scroll_offset = cursor_vl.saturating_add(1).saturating_sub(max_visible);
        }
        self.scroll_offset = self.scroll_offset.min(map.len().saturating_sub(max_visible));
        let mut lines: Vec<Line> = Vec::with_capacity(max_visible);
        for (vi, vl) in map.iter().enumerate().skip(self.scroll_offset).take(max_visible) {
            let mut spans: Vec<Span> = Vec::new();
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
                // The highlighted cell is one whole GRAPHEME, not one `char`: Pi takes
                // `afterGraphemes[0].segment` and slices the rest past it (`editor.ts:555-559`), so a
                // ZWJ emoji is inverted as a unit instead of leaving its continuation chars
                // un-highlighted beside a reversed base character.
                let rest: String = seg.iter().skip(vcol).collect();
                match rest.graphemes(true).next() {
                    Some(g) => {
                        spans.push(Span::styled(g.to_string(), cursor_style));
                        let after: String = rest.chars().skip(g.chars().count()).collect();
                        spans.push(Span::styled(after, base));
                    }
                    // End-of-line caret: a reverse-video space (Pi `editor.ts:563`).
                    None => spans.push(Span::styled(" ", cursor_style)),
                }
            } else {
                spans.push(Span::styled(seg.iter().collect::<String>(), base));
            }
            lines.push(Line::from(spans));
        }
        let shown = lines.len();
        let para = Paragraph::new(lines).block(block).style(base);
        frame.render_widget(para, area);
        // E4's other half: the rules ANNOUNCE the hidden rows. `createScrollBorder`
        // (`editor.ts:259-268`) replaces the plain `─`-repeat with `─── ↑ N more ───…` at the top
        // when `scrollOffset > 0` (`:526-528`) and `─── ↓ N more ───…` at the bottom when content
        // remains below (`:582-585`). The `Block` above already painted a plain rule on both edges;
        // these overwrite it in place, which is byte-identical to pi choosing one string or the other
        // (both are exactly `width` columns).
        if self.scroll_offset > 0 && area.height >= 1 {
            let text = scroll_border('↑', self.scroll_offset, area.width);
            let row = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, rule_style))), row);
        }
        let below = map.len().saturating_sub(self.scroll_offset.saturating_add(shown));
        if below > 0 && area.height >= 2 {
            let text = scroll_border('↓', below, area.width);
            let row = Rect {
                x: area.x,
                y: area.y.saturating_add(area.height.saturating_sub(1)),
                width: area.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, rule_style))), row);
        }
        // Pi hides the terminal's real cursor unless `showHardwareCursor` is on (`tui.ts:1659-1663`
        // `if (this.showHardwareCursor) showCursor() else hideCursor()`); ratatui's `Terminal::draw`
        // hides it for us whenever no position was set, so the gate is the call itself.
        if self.show_hardware_cursor
            && let Some((x, y)) = self.cursor_in(area)
        {
            frame.set_cursor_position((x, y));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    /// `word_wrap_line` over a `&str`, as `(chunk text)` — the shape `wordWrapLine` returns
    /// (`editor.ts:114-206` yields `{ text, startIndex, endIndex }`).
    fn wrap(s: &str, width: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        word_wrap_line(&chars, width)
            .into_iter()
            .map(|(start, len)| chars.iter().skip(start).take(len).collect())
            .collect()
    }

    /// The chunks tile the line: contiguous, gap-free, and reassembling to the input. Upstream gets
    /// this for free from `line.slice(chunkStart, …)` + a final `line.slice(chunkStart)` (`:202`).
    fn assert_tiles(s: &str, width: usize) {
        let chars: Vec<char> = s.chars().collect();
        let segs = word_wrap_line(&chars, width);
        let mut at = 0usize;
        for (start, len) in &segs {
            assert_eq!(*start, at, "chunk {segs:?} of {s:?}@{width} is not contiguous");
            at += len;
        }
        assert_eq!(at, chars.len(), "chunks {segs:?} do not cover {s:?}");
    }

    // -------------------------------------------------------------- wordWrapLine ---------------

    /// The two early returns (`editor.ts:115-122`): an empty line is one empty chunk, and a line
    /// that already fits is one chunk covering the whole line.
    #[test]
    fn a_line_that_fits_is_one_chunk() {
        assert_eq!(word_wrap_line(&[], 10), vec![(0, 0)]);
        assert_eq!(wrap("hello", 10), vec!["hello"]);
        // "fits" is measured in COLUMNS: 5 ideographs are 10 of them.
        assert_eq!(wrap("日本語です", 10), vec!["日本語です"]);
        assert_eq!(wrap("日本語です", 9).len(), 2, "…and 9 columns is one short");
    }

    /// Whitespace is the primary wrap opportunity, and the break lands AFTER the space run so the
    /// trailing space stays on the wrapped row (`wrapOppIndex = next.index`, `editor.ts:187-189`).
    #[test]
    fn wrapping_breaks_after_the_last_space_that_fits() {
        assert_eq!(wrap("aaa bbb ccc", 5), vec!["aaa ", "bbb ", "ccc"]);
        assert_eq!(wrap("aaa  bbb", 5), vec!["aaa  ", "bbb"], "a run of spaces joins (`:187`)");
        // And it is GREEDY, not balanced: at width 7 the tail `"bbb ccc"` is exactly 7 columns, so
        // the backtrack at the second space finds it already fits and never fires
        // (`currentWidth - wrapOppWidth + gWidth <= maxWidth`, `editor.ts:147`).
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa ", "bbb ccc"]);
        assert_tiles("aaa bbb ccc", 5);
        assert_tiles("aaa bbb ccc", 7);
    }

    /// A word longer than the width force-breaks at the current grapheme's own start index
    /// (`editor.ts:154-160`).
    #[test]
    fn an_overlong_word_force_breaks_at_the_width() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_tiles("abcdefghij", 4);
    }

    /// **The display-width bug.** `visibleWidth(grapheme)` (`editor.ts:139-143`), not a char count:
    /// 24 ideographs are 48 columns and cannot be one 39-column row. The break also needs
    /// `cjkBreakRegex` (`utils.ts:54`, used at `editor.ts:191-198`) — CJK has no spaces to break at,
    /// so without the CJK opportunity the whole run would be one unbreakable "word".
    #[test]
    fn cjk_is_measured_and_broken_in_columns() {
        let cjk: String = "日本語".chars().cycle().take(24).collect();
        let rows = wrap(&cjk, 39);
        assert_eq!(rows.len(), 2, "48 columns do not fit 39: {rows:?}");
        assert_eq!(rows[0].chars().count(), 19, "19 ideographs are 38 columns; a 20th would be 40");
        assert_eq!(rows.concat(), cjk);
        assert_tiles(&cjk, 39);
        for r in &rows {
            assert!(display_width(r) <= 39, "row overflows: {r:?}");
        }
    }

    /// The CJK opportunity is a BOUNDARY rule — it fires when either side is CJK
    /// (`editor.ts:194-198`), so Latin text abutting CJK may break between them.
    #[test]
    fn a_latin_cjk_boundary_is_a_wrap_opportunity() {
        // `word` is 4 columns and each ideograph 2, so at width 4 the opportunity recorded at the
        // `d`→`日` boundary is what puts `word` on a row of its own; the two that follow come from
        // the CJK-to-CJK opportunities.
        let rows = wrap("word日本語", 4);
        assert_eq!(rows, vec!["word", "日本", "語"], "the boundary breaks: {rows:?}");
        assert_tiles("word日本語", 4);
        // Contrast: an all-Latin run of the same length has NO opportunity anywhere, so it
        // force-breaks mid-"word" instead.
        assert_eq!(wrap("wordabcdef", 4), vec!["word", "abcd", "ef"]);
    }

    /// A grapheme CLUSTER is atomic: the break never lands inside one, whatever the width.
    #[test]
    fn a_cluster_is_never_split() {
        const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let line = format!("{}{FAMILY}", "a".repeat(38));
        let rows = wrap(&line, 39);
        assert_eq!(rows, vec!["a".repeat(38), FAMILY.to_string()], "torn cluster: {rows:?}");
        assert_tiles(&line, 39);

        // …including when the cluster ALONE is wider than the width: it is indivisible, so it takes
        // a row of its own rather than recursing forever the way `editor.ts:163-178` would.
        let rows = wrap(&format!("ab{FAMILY}cd"), 1);
        assert_eq!(rows, vec!["a", "b", FAMILY, "c", "d"], "{rows:?}");
        assert_tiles(&format!("ab{FAMILY}cd"), 1);
    }

    /// Every produced row fits, and the rows always tile the input — swept over a spread of widths
    /// and mixed scripts, because the failure mode of the old char-count wrap was silent overflow
    /// rather than a crash.
    #[test]
    fn every_wrapped_row_fits_its_width() {
        const CASES: [&str; 5] = [
            "the quick brown fox jumps over the lazy dog",
            "日本語のテキストは空白で区切られていません",
            "mixed 日本語 and latin with  double  spaces",
            "e\u{301}combining\u{301}marks\u{301}everywhere",
            "supercalifragilisticexpialidocious",
        ];
        for s in CASES {
            for width in 1..=45usize {
                assert_tiles(s, width);
                for row in wrap(s, width) {
                    // A single over-wide cluster is the one legal exception (see above): it cannot
                    // be split, so it is emitted alone.
                    let clusters = row.graphemes(true).count();
                    assert!(
                        display_width(&row) <= width || clusters == 1,
                        "{s:?}@{width}: row {row:?} is {} columns",
                        display_width(&row)
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------- createScrollBorder ------------

    /// The wide path (`editor.ts:262-263`): the indicator, then `─` to the requested width.
    #[test]
    fn the_scroll_rule_reads_as_an_indicator_padded_with_rule() {
        let s = scroll_border('↑', 6, 20);
        assert!(s.starts_with("─── ↑ 6 more "), "{s:?}");
        assert_eq!(display_width(&s), 20, "{s:?}");
        assert!(s.ends_with('─'), "the remainder is rule, not blank: {s:?}");
    }

    /// The narrow path (`editor.ts:265-267`): a strict slice of the indicator plus `...`, itself
    /// clipped on a terminal too narrow even for that.
    #[test]
    fn a_terminal_too_narrow_for_the_indicator_gets_an_ellipsis() {
        assert_eq!(scroll_border('↓', 5, 10), "─── ↓ 5...", "{:?}", scroll_border('↓', 5, 10));
        assert_eq!(scroll_border('↓', 5, 2), "..");
        assert_eq!(scroll_border('↓', 5, 0), "");
    }

    /// The invariant the render depends on: the string is EXACTLY `width` columns for every width
    /// and every hidden count, so it overwrites the `Block`'s pre-painted rule with no `─` leaking
    /// out from underneath. (Upstream can be one column short here — `strict` may reject a wide
    /// grapheme at the boundary and nothing pads afterwards — but the indicator's alphabet is
    /// entirely single-column, so the case does not arise. See [`scroll_border`].)
    #[test]
    fn the_scroll_rule_is_exactly_as_wide_as_it_is_asked_for() {
        for direction in ['↑', '↓'] {
            for hidden in [0usize, 1, 9, 10, 99, 1234, 1_000_000] {
                for width in 0..=120u16 {
                    let s = scroll_border(direction, hidden, width);
                    assert_eq!(
                        display_width(&s),
                        usize::from(width),
                        "scroll_border({direction:?}, {hidden}, {width}) = {s:?}"
                    );
                }
            }
        }
    }
}
